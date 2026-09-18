//! Audits over `llvm-objdump -d --no-show-raw-insn` text of the linked resident
//! payload, and over the COFF objects of the DXE bootstrap assembly.
//!
//! The line grammar below is a hand-written equivalent of the regular
//! expressions of the audited original (quoted next to each matcher); std has
//! no regex engine and these shapes are small and fixed.

use std::collections::HashMap;

use crate::json::Value;

const ERROR_CODE_VECTORS: [u32; 10] = [8, 10, 11, 12, 13, 14, 17, 21, 29, 30];
const NMI_VECTOR: i64 = 2;
const NMI_GATE: &str = "svmvisor_resident_nmi";
const NMI_FLAG: &str = "svmvisor_resident_nmi_pending";

/// One element of a full-match line pattern.
enum Token {
    Literal(String),
    /// `(?:literal)?`
    Optional(&'static str),
    /// `[0-9a-f]+`. Greedy without backtracking, which is exact because no
    /// following token starts with a hexadecimal digit.
    Hex,
}

/// Permit only the exact successful guest-INIT DR0-3 clearing helper.
pub fn audit_debug_reset(text: &str) -> Result<Value, String> {
    const SYMBOL: &str = "svmvisor_resident_reset_guest_debug";
    let mut owner: Option<&str> = None;
    let mut body: Vec<String> = Vec::new();
    for line in lines(text) {
        if let Some(name) = label(line) {
            owner = Some(name);
            continue;
        }
        let Some(raw) = instruction(line) else { continue };
        let value = normalized(raw);
        if names_debug_register(&value) && owner != Some(SYMBOL) {
            return Err(format!("unowned debug-register instruction: {value}"));
        }
        if owner == Some(SYMBOL) {
            // Alignment padding after RET is not part of the helper body.
            if body.last().is_some_and(|last| last == "retq") {
                if names_debug_register(&value) {
                    return Err("debug instruction after reset helper return".into());
                }
                continue;
            }
            body.push(value);
        }
    }
    let expected = [
        "xorl %eax, %eax",
        "movq %rax, %dr0",
        "movq %rax, %dr1",
        "movq %rax, %dr2",
        "movq %rax, %dr3",
        "retq",
    ];
    if body != expected {
        return Err(format!(
            "guest debug reset helper differs from audited zero-only sequence: {body:?}"
        ));
    }
    Ok(Value::Map(vec![
        ("symbol".into(), Value::str(SYMBOL)),
        ("zeroed_live_registers".into(), Value::strs(&["DR0", "DR1", "DR2", "DR3"])),
        ("other_debug_register_instructions".into(), Value::Int(0)),
    ]))
}

/// Check all vector frames and the bounded first-record capture ABI.
pub fn audit_host_fault(text: &str) -> Result<Value, String> {
    let mut bodies: HashMap<&str, Vec<String>> = HashMap::new();
    let mut owner: Option<&str> = None;
    for line in lines(text) {
        if let Some(name) = label(line) {
            owner = Some(name);
            bodies.insert(name, Vec::new());
        }
        if let (Some(raw), Some(owner)) = (instruction(line), owner) {
            bodies.get_mut(owner).unwrap().push(normalized(raw));
        }
    }
    let to_common = branch("jmp", "svmvisor_resident_fault_common");
    for vector in 0..256u32 {
        let mut body = bodies.get(format!("svmvisor_resident_fault_{vector}").as_str());
        if vector == 255 && body.is_none() {
            body = bodies.get("svmvisor_resident_fault");
        }
        let mut pushes = Vec::new();
        if !ERROR_CODE_VECTORS.contains(&vector) {
            pushes.push("pushq $0x0".to_string());
        }
        pushes.push(format!("pushq $0x{vector:x}"));
        let sound = body.is_some_and(|body| {
            body.split_last().is_some_and(|(last, frame)| {
                frame == pushes.as_slice() && matches(&to_common, last)
            })
        });
        if !sound {
            return Err(format!(
                "host fault vector{vector} frame mismatch: {}",
                python_list(body.map(Vec::as_slice))
            ));
        }
    }
    let empty = Vec::new();
    let common = bodies.get("svmvisor_resident_fault_common").unwrap_or(&empty);
    let checks = [
        "cli",
        "clgi",
        "cld",
        "movb $0x1, %al",
        "testb %al, %al",
        "movq %cr2, %r8",
        "movq %cr3, %r9",
        "movq %rsp, %rsi",
        "movl $0x7, %ecx",
        "rep movsq (%rsi), %es:(%rdi)",
        "movq %r8, (%rdi)",
        "movq %r9, 0x8(%rdi)",
        "movq %r8, %rsi",
        "movq %r9, %rdx",
        "andq $-0x10, %rsp",
    ];
    if checks.iter().any(|item| !common.iter().any(|line| line == item)) || common.len() != 20 {
        return Err(format!("host fault capture sequence mismatch: {common:?}"));
    }
    if !(common[4].contains("xchgb %al,")
        && common[4].contains("svmvisor_resident_fault_latched")
        && common[6].ends_with("<svmvisor_resident_fault_stop>")
        && [10, 15].iter().all(|&index| common[index].contains("svmvisor_resident_fault_record"))
        && common[19].ends_with("<svmvisor_resident_host_fault>"))
    {
        return Err("host fault latch/record/callback targets differ".into());
    }
    // Far-return local label may split the symbol in some objdump versions.
    let ordered = lines(text).collect::<Vec<_>>().join("\n");
    let begin =
        ordered.find("<svmvisor_resident_enter>:").ok_or("missing svmvisor_resident_enter")?;
    let end =
        ordered.find("<svmvisor_resident_vmrun>:").ok_or("missing svmvisor_resident_vmrun")?;
    let setup = ordered.get(begin..end).unwrap_or("");
    let load_tss = setup.find("ltr").ok_or("private TSS is never loaded before VMRUN")?;
    let load_idt = setup.find("lidt").ok_or("private IDT is never loaded before VMRUN")?;
    if load_tss > load_idt {
        return Err("private IDT selected before private TSS".into());
    }
    // Returning IRQ gates (irq.S): every vector 16-255 except 18 (#MC) takes
    // a window interrupt without an error code and otherwise falls through,
    // frame unchanged, to its own fault stub. Gate 30 is reached only from
    // the #SX gate when the first stack word is not the INIT error code 1.
    let shaped = |body: Option<&[String]>, expected: &[Vec<Token>]| {
        body.is_some_and(|body| {
            body.len() == expected.len()
                && expected.iter().zip(body).all(|(pattern, line)| matches(pattern, line))
        })
    };
    let gates: Vec<u32> = (16..256).filter(|&vector| vector != 18).collect();
    for &vector in &gates {
        let expected = [
            vec![literal("clgi")],
            rip_operand("cmpl $0x1, ".into(), "svmvisor_resident_irq_window"),
            branch("jne", &format!("svmvisor_resident_fault_{vector}")),
            rip_operand("movl $0x0, ".into(), "svmvisor_resident_irq_window"),
            rip_operand(format!("movl $0x{vector:x}, "), "svmvisor_resident_irq_vector"),
            vec![literal("andq $-0x201, 0x10(%rsp)"), Token::Optional(" # imm = 0xFDFF")],
            vec![literal("iretq")],
        ];
        let body = returning(&bodies, &format!("svmvisor_resident_irq_{vector}"));
        if !shaped(body, &expected) {
            return Err(format!(
                "IRQ gate vector{vector} differs from the window check: {}",
                python_list(body)
            ));
        }
    }
    if bodies.contains_key("svmvisor_resident_irq_18") {
        return Err("vector 18 must stay the terminal #MC stub".into());
    }
    let sx = returning(&bodies, "svmvisor_resident_sx").unwrap_or(&[]);
    let expected = [
        vec![literal("clgi")],
        vec![literal("cmpq $0x1, (%rsp)")],
        branch("jne", "svmvisor_resident_irq_30"),
        vec![literal("lock")],
        rip_operand("incq ".into(), "svmvisor_resident_init_acks"),
        vec![literal("addq $0x8, %rsp")],
        vec![literal("iretq")],
    ];
    if !shaped(Some(sx), &expected) {
        return Err(format!(
            "unexpected #SX bypasses the window check and host fault reporter: {sx:?}"
        ));
    }
    // Returning NMI gate (irq.S): the IDT names it for vector 2 in place of
    // the terminal stub, which stays linked (fault.S offsets table) and is
    // still checked above. It may only flag the NMI and return with GIF clear.
    let nmi = returning(&bodies, NMI_GATE);
    let expected = [
        vec![literal("clgi")],
        rip_operand("movl $0x1, ".into(), NMI_FLAG),
        vec![literal("iretq")],
    ];
    if !shaped(nmi, &expected) {
        return Err(format!(
            "NMI gate differs from the flag-and-return sequence: {}",
            python_list(nmi)
        ));
    }
    Ok(Value::Map(vec![
        ("vectors".into(), Value::Int(256)),
        (
            "hardware_error_vectors".into(),
            Value::ints(ERROR_CODE_VECTORS.iter().map(|&vector| vector as i64)),
        ),
        ("copied_frame_qwords".into(), Value::Int(7)),
        ("copied_control_registers".into(), Value::strs(&["CR2", "CR3"])),
        ("private_tss_loaded_before_idt".into(), Value::Bool(true)),
        ("recursive_callback_blocked".into(), Value::Bool(true)),
        ("irq_window_gates".into(), Value::Int(gates.len() as i64)),
        ("irq_window_gate_vectors".into(), Value::ints([16, 255])),
        ("terminal_only_vectors_below_32".into(), Value::ints(terminal_only_vectors_below_32())),
        (
            "returning_nmi_gate".into(),
            Value::Map(vec![
                ("vector".into(), Value::Int(NMI_VECTOR)),
                ("symbol".into(), Value::str(NMI_GATE)),
                ("flag".into(), Value::str(NMI_FLAG)),
            ]),
        ),
        (
            "sx_non_init_path".into(),
            Value::strs(&["svmvisor_resident_irq_30", "svmvisor_resident_fault_30"]),
        ),
    ]))
}

/// No FP/SIMD/xstate instruction may be linked; returns the instruction count.
pub fn audit_extended_state(text: &str) -> Result<u64, String> {
    let mut count = 0;
    for line in lines(text) {
        let Some(instruction) = instruction(line) else { continue };
        let mnemonic = instruction.split_whitespace().next().ok_or("malformed disassembly line")?;
        count += 1;
        if (names_extended_state_register(instruction)
            || ["f", "v", "xsave", "xrstor", "xsetbv"]
                .iter()
                .any(|prefix| mnemonic.starts_with(prefix)))
            && !["vmrun", "vmload", "vmsave"].contains(&mnemonic)
        {
            return Err(format!("unowned extended-state instruction: {instruction}"));
        }
    }
    if count == 0 {
        return Err("empty disassembly".into());
    }
    Ok(count)
}

/// The PE optional-header subsystem must be EFI runtime driver (12).
pub fn audit_runtime_driver(pe: &[u8]) -> Result<(), String> {
    let short = "shim is not a complete PE image";
    let offset = u32::from_le_bytes(pe.get(0x3c..0x40).ok_or(short)?.try_into().unwrap()) as usize;
    let field = offset.checked_add(24 + 68).ok_or(short)?;
    let subsystem = u16::from_le_bytes(pe.get(field..field + 2).ok_or(short)?.try_into().unwrap());
    if subsystem != 12 {
        return Err("shim is not EFI runtime driver".into());
    }
    Ok(())
}

/// The AP wait loop is copied as bytes out of `.rdata`: it must carry no COFF
/// relocations and fit the startup page budget.
pub fn audit_copied_ap_wait(object: &[u8], listing: &str) -> Result<Value, String> {
    let short = "truncated COFF object";
    let u16_at = |offset: usize| -> Result<usize, String> {
        Ok(u16::from_le_bytes(object.get(offset..offset + 2).ok_or(short)?.try_into().unwrap())
            as usize)
    };
    let section_count = u16_at(2)?;
    let optional_size = u16_at(16)?;
    let mut copied_sections = Vec::new();
    for index in 0..section_count {
        let offset = 20 + optional_size + index * 40;
        let name = object.get(offset..offset + 8).ok_or(short)?;
        let name = &name[..name.iter().rposition(|byte| *byte != 0).map_or(0, |last| last + 1)];
        if name == b".rdata" {
            if u16_at(offset + 32)? != 0 {
                return Err("copied AP code has COFF relocations".into());
            }
            copied_sections.push(index as u64 + 1);
        }
    }
    let (section, start) = coff_symbol(listing, "svmvisor_ap_wait")?;
    let (end_section, end) = coff_symbol(listing, "svmvisor_ap_wait_end")?;
    if !copied_sections.contains(&section)
        || end_section != section
        || end <= start
        || end - start > 3840
    {
        return Err("invalid copied AP wait span".into());
    }
    Ok(Value::Map(vec![
        ("copied_wait_bytes".into(), Value::Int((end - start) as i64)),
        ("copied_section_relocations".into(), Value::Int(0)),
    ]))
}

/// `\s*[0-9a-f]+ <([^>]+)>:` (full match).
fn label(line: &str) -> Option<&str> {
    let rest = address(line)?.strip_prefix(" <")?;
    let end = rest.find('>')?;
    (end > 0 && &rest[end..] == ">:").then(|| &rest[..end])
}

/// `\s*[0-9a-f]+:\s+(.+)` (full match).
fn instruction(line: &str) -> Option<&str> {
    let rest = address(line)?.strip_prefix(':')?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let body = rest.trim_start();
    if !body.is_empty() {
        return Some(body);
    }
    // Backtracking leaves the final whitespace character to `(.+)`.
    let last = rest.char_indices().last()?.0;
    (last > 0).then(|| &rest[last..])
}

/// After optional leading whitespace, split a non-empty `[0-9a-f]+` run off.
fn address(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let digits = line.find(|character| !is_hex(character)).unwrap_or(line.len());
    (digits > 0).then(|| &line[digits..])
}

/// `re.sub(r'\s+', ' ', value).strip()`.
fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `re.search(r'%dr[0-7]\b', value)`.
fn names_debug_register(value: &str) -> bool {
    value.match_indices("%dr").any(|(index, _)| {
        let mut rest = value[index + 3..].chars();
        matches!(rest.next(), Some('0'..='7'))
            && !rest.next().is_some_and(|next| next.is_alphanumeric() || next == '_')
    })
}

/// `re.search(r'%(?:[xyz]mm|mm|st)[0-9(]', instruction)`.
fn names_extended_state_register(instruction: &str) -> bool {
    instruction.match_indices('%').any(|(index, _)| {
        let rest = &instruction[index + 1..];
        ["xmm", "ymm", "zmm", "mm", "st"].iter().any(|register| {
            rest.strip_prefix(register)
                .and_then(|tail| tail.chars().next())
                .is_some_and(|next| next.is_ascii_digit() || next == '(')
        })
    })
}

fn matches(pattern: &[Token], mut line: &str) -> bool {
    for token in pattern {
        match token {
            Token::Literal(text) => match line.strip_prefix(text.as_str()) {
                Some(rest) => line = rest,
                None => return false,
            },
            Token::Optional(text) => line = line.strip_prefix(text).unwrap_or(line),
            Token::Hex => {
                let digits = line.find(|character| !is_hex(character)).unwrap_or(line.len());
                if digits == 0 {
                    return false;
                }
                line = &line[digits..];
            }
        }
    }
    line.is_empty()
}

fn is_hex(character: char) -> bool {
    matches!(character, '0'..='9' | 'a'..='f')
}

/// `{prefix}-?0x[0-9a-f]+\(%rip\) # 0x[0-9a-f]+ <{symbol}>`
fn rip_operand(prefix: String, symbol: &str) -> Vec<Token> {
    vec![
        literal(prefix),
        Token::Optional("-"),
        literal("0x"),
        Token::Hex,
        literal("(%rip) # 0x"),
        Token::Hex,
        literal(format!(" <{symbol}>")),
    ]
}

/// `{mnemonic} 0x[0-9a-f]+ <{symbol}>`
fn branch(mnemonic: &str, symbol: &str) -> Vec<Token> {
    vec![literal(format!("{mnemonic} 0x")), Token::Hex, literal(format!(" <{symbol}>"))]
}

fn literal(text: impl Into<String>) -> Token {
    Token::Literal(text.into())
}

fn python_list(body: Option<&[String]>) -> String {
    match body {
        Some(body) => format!("{body:?}"),
        None => "None".into(),
    }
}

/// Alignment padding after IRETQ (before the next object) is not part of the
/// gate; anything else after it is.
fn returning<'a>(bodies: &'a HashMap<&str, Vec<String>>, name: &str) -> Option<&'a [String]> {
    let body = bodies.get(name)?;
    let Some(end) = body.iter().position(|line| line == "iretq") else { return Some(body) };
    if !body[end + 1..].iter().all(|line| is_padding(line)) {
        return Some(body);
    }
    Some(&body[..=end])
}

/// `int3|nop[lw]?(?: .*)?` (full match).
fn is_padding(line: &str) -> bool {
    let tail = |rest: &str| rest.is_empty() || rest.starts_with(' ');
    line == "int3"
        || line
            .strip_prefix("nop")
            .is_some_and(|rest| tail(rest) || rest.strip_prefix(['l', 'w']).is_some_and(tail))
}

/// Vectors below 32 whose IDT gate can only stop: every exception vector
/// below the window gates except the returning NMI gate, plus #MC (18).
fn terminal_only_vectors_below_32() -> impl Iterator<Item = i64> {
    (0..32).filter(|&vector| (vector < 16 && vector != NMI_VECTOR) || vector == 18)
}

/// `\(sec\s+(\d+)\).*?0x([0-9a-f]+) NAME$` searched per line (re.M).
fn coff_symbol(listing: &str, name: &str) -> Result<(u64, u64), String> {
    let found = lines(listing).find_map(|line| {
        let head = line.strip_suffix(name)?.strip_suffix(' ')?;
        let digits = head.len() - head.trim_end_matches(is_hex).len();
        let value = &head[head.len() - digits..];
        let before = head[..head.len() - digits].strip_suffix("0x")?;
        if digits == 0 {
            return None;
        }
        before.match_indices("(sec").find_map(|(index, _)| {
            let rest = &before[index + 4..];
            let number = rest.trim_start();
            let end = number.find(|character: char| !character.is_ascii_digit())?;
            (number.len() < rest.len() && end > 0 && number[end..].starts_with(')'))
                .then(|| (number[..end].parse().ok(), u64::from_str_radix(value, 16).ok()))
        })
    });
    match found {
        Some((Some(section), Some(value))) => Ok((section, value)),
        _ => Err(format!("missing AP audit symbol {name}")),
    }
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line))
}

#[cfg(test)]
mod tests;
