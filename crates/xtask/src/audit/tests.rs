use super::*;

// ---- line grammar -----------------------------------------------------

#[test]
fn line_grammar_matches_the_original_expressions() {
    assert_eq!(
        label("0000000000100010 <svmvisor_resident_enter>:"),
        Some("svmvisor_resident_enter")
    );
    assert_eq!(label("  10 <a>: "), None);
    assert_eq!(label("  10 <>:"), None);
    assert_eq!(label("  10: <a>:"), None);
    assert_eq!(instruction("  10366d:      \tcli"), Some("cli"));
    assert_eq!(instruction("10366d:cli"), None);
    assert_eq!(instruction("payload.elf:\tfile format elf64-x86-64"), None);
    assert_eq!(instruction("Disassembly of section .text:"), None);
    assert_eq!(normalized("rep\t\tmovsq\t(%rsi), %es:(%rdi)"), "rep movsq (%rsi), %es:(%rdi)");
    assert!(names_debug_register("movq %rax, %dr7"));
    assert!(names_debug_register("movq %dr3, %rax"));
    assert!(!names_debug_register("movq %rax, %dr8"));
    assert!(!names_debug_register("movq %rax, %dr0x"));
    assert!(names_extended_state_register("movaps %xmm0, %xmm1"));
    assert!(names_extended_state_register("fld %st(1)"));
    assert!(names_extended_state_register("movq %mm0, %rax"));
    assert!(!names_extended_state_register("movq %rax, %rbx"));
    assert!(
        is_padding("int3")
            && is_padding("nop")
            && is_padding("nopw %cs:(%rax,%rax)")
            && is_padding("nopl (%rax)")
    );
    assert!(!is_padding("nopx") && !is_padding("int3 ") && !is_padding("clgi"));
}

#[test]
fn extended_state_audit_counts_and_rejects() {
    let ok = "  10: \tvmrun\n  13: \tvmsave\n  16: \tmovq\t%rax, %rbx\n";
    assert_eq!(audit_extended_state(ok), Ok(3));
    for bad in [
        "  10: \tmovaps\t%xmm0, %xmm1\n",
        "  10: \tfninit\n",
        "  10: \txsave\t(%rax)\n",
        "  10: \tvzeroupper\n",
        "  10: \txsetbv\n",
        "  10: \tmovq\t%mm0, %rax\n",
    ] {
        assert!(
            audit_extended_state(bad)
                .unwrap_err()
                .starts_with("unowned extended-state instruction"),
            "{bad}"
        );
    }
    assert_eq!(audit_extended_state("\n"), Err("empty disassembly".into()));
}

#[test]
fn coff_audit_checks_relocations_sections_and_span() {
    fn object(relocations: u16) -> Vec<u8> {
        let mut data = vec![0u8; 20 + 2 * 40];
        data[2..4].copy_from_slice(&2u16.to_le_bytes());
        data[20..25].copy_from_slice(b".text");
        data[60..66].copy_from_slice(b".rdata");
        data[60 + 32..60 + 34].copy_from_slice(&relocations.to_le_bytes());
        data
    }
    let listing = |section: u32, end: u32| {
        format!(
            "SYMBOL TABLE:\n[ 4](sec  {section})(fl 0x00)(ty   0)(scl   2) (nx 0) 0x00000010 svmvisor_ap_wait\n\
             [ 5](sec  2)(fl 0x00)(ty   0)(scl   2) (nx 0) 0x{end:08x} svmvisor_ap_wait_end\n"
        )
    };
    let result = audit_copied_ap_wait(&object(0), &listing(2, 0x110)).unwrap();
    assert_eq!(result.get("copied_wait_bytes"), Some(&Value::Int(0x100)));
    assert_eq!(
        audit_copied_ap_wait(&object(1), &listing(2, 0x110)),
        Err("copied AP code has COFF relocations".into())
    );
    for bad in [listing(1, 0x110), listing(2, 0x10), listing(2, 0x10 + 3841)] {
        assert_eq!(
            audit_copied_ap_wait(&object(0), &bad),
            Err("invalid copied AP wait span".into())
        );
    }
    assert_eq!(
        audit_copied_ap_wait(&object(0), "SYMBOL TABLE:\n"),
        Err("missing AP audit symbol svmvisor_ap_wait".into())
    );
    assert!(audit_runtime_driver(&[0; 16]).is_err());
}

// ---- debug reset audit (test_debug_reset_audit.py) ---------------------

const BODY: &str = "00001000 <svmvisor_resident_reset_guest_debug>:
1000: xorl %eax, %eax
1002: movq %rax, %dr0
1005: movq %rax, %dr1
1008: movq %rax, %dr2
100b: movq %rax, %dr3
100e: retq
100f: nop
";

#[test]
fn exact_zero_only_helper_passes() {
    let result = audit_debug_reset(BODY).unwrap();
    assert_eq!(
        result.get("zeroed_live_registers"),
        Some(&Value::strs(&["DR0", "DR1", "DR2", "DR3"]))
    );
}

#[test]
fn other_debug_write_or_read_and_malformed_helper_fail() {
    for text in [
        BODY.replace("xorl %eax, %eax", "movl $1, %eax"),
        BODY.replace("%dr2", "%dr7"),
        BODY.replace("1008: movq %rax, %dr2\n", ""),
        format!("{BODY}1010: movq %rax, %dr0\n"),
        format!("{BODY}00002000 <other>:\n2000: movq %rax, %dr0\n"),
        format!("{BODY}00002000 <other>:\n2000: movq %dr3, %rax\n"),
    ] {
        assert!(audit_debug_reset(&text).is_err(), "{text}");
    }
}

// ---- host fault audit (test_host_fault_audit.py) -----------------------

const COMMON: &str = "  10366d:      \tcli
  10366e:      \tclgi
  103671:      \tcld
  103672:      \tmovb\t$0x1, %al
  103674:      \txchgb\t%al, 0x1f9d6(%rip)      # 0x123050 <svmvisor_resident_fault_latched>
  10367a:      \ttestb\t%al, %al
  10367c:      \tjne\t0x1036b9 <svmvisor_resident_fault_stop>
  103682:      \tmovq\t%cr2, %r8
  103686:      \tmovq\t%cr3, %r9
  10368a:      \tmovq\t%rsp, %rsi
  10368d:      \tleaq\t0x1f974(%rip), %rdi     # 0x123008 <svmvisor_resident_fault_record>
  103694:      \tmovl\t$0x7, %ecx
  103699:      \trep\t\tmovsq\t(%rsi), %es:(%rdi)
  10369c:      \tmovq\t%r8, (%rdi)
  10369f:      \tmovq\t%r9, 0x8(%rdi)
  1036a3:      \tleaq\t0x1f95e(%rip), %rdi     # 0x123008 <svmvisor_resident_fault_record>
  1036aa:      \tmovq\t%r8, %rsi
  1036ad:      \tmovq\t%r9, %rdx
  1036b0:      \tandq\t$-0x10, %rsp
  1036b4:      \tcallq\t0x119150 <svmvisor_resident_host_fault>
";

fn gate_to(vector: u32, fault: u32) -> String {
    format!(
        "0000000000100{vector:03x} <svmvisor_resident_irq_{vector}>:\n\
  1002d3:      \tclgi\n\
  1002d6:      \tcmpl\t$0x1, 0x22d27(%rip)     # 0x123004 <svmvisor_resident_irq_window>\n\
  1002dd:      \tjne\t0x102d0d <svmvisor_resident_fault_{fault}>\n\
  1002e3:      \tmovl\t$0x0, 0x22d17(%rip)     # 0x123004 <svmvisor_resident_irq_window>\n\
  1002ed:      \tmovl\t$0x{vector:x}, 0x22d09(%rip)    # 0x123000 <svmvisor_resident_irq_vector>\n\
  1002f7:      \tandq\t$-0x201, 0x10(%rsp)     # imm = 0xFDFF\n\
  100300:      \tiretq\n\n"
    )
}

fn gate(vector: u32) -> String {
    gate_to(vector, vector)
}

fn sx(target: &str) -> String {
    format!(
        "0000000000100277 <svmvisor_resident_sx>:\n\
  100277:      \tclgi\n\
  10027a:      \tcmpq\t$0x1, (%rsp)\n\
  10027f:      \tjne\t0x102cfd <{target}>\n\
  100285:      \tlock\n\
  100286:      \tincq\t0x7ae33(%rip)           # 0x17b0c0 <svmvisor_resident_init_acks>\n\
  10028d:      \taddq\t$0x8, %rsp\n\
  100291:      \tiretq\n\n"
    )
}

fn nmi() -> String {
    "00000000001002c0 <svmvisor_resident_nmi>:\n\
  1002c0:      \tclgi\n\
  1002c3:      \tmovl\t$0x1, 0x7adf3(%rip)     # 0x17b0c0 <svmvisor_resident_nmi_pending>\n\
  1002cd:      \tiretq\n\n"
        .into()
}

fn nmi_with(needle: &str, replacement: &str) -> String {
    image().replace(&nmi(), &nmi().replacen(needle, replacement, 1))
}

fn image_with(gates: &[u32], sx_body: &str) -> String {
    let mut text = String::from(
        "0000000000100010 <svmvisor_resident_enter>:\n  100071:      \tltrw\t%ax\n  100078:      \tlidtq\t(%rax)\n\n\
000000000010011b <svmvisor_resident_vmrun>:\n  10011b:      \tvmrun\n\n",
    );
    text.push_str(sx_body);
    text.push_str(&nmi());
    for &vector in gates {
        if vector != 18 {
            text.push_str(&gate(vector));
        }
    }
    for vector in 0..256u32 {
        text.push_str(&format!("0000000000102{vector:03x} <svmvisor_resident_fault_{vector}>:\n"));
        if !ERROR_CODE_VECTORS.contains(&vector) {
            text.push_str("  102c01:      \tpushq\t$0x0\n");
        }
        text.push_str(&format!("  102c03:      \tpushq\t$0x{vector:x}\n"));
        text.push_str("  102c05:      \tjmp\t0x10366d <svmvisor_resident_fault_common>\n\n");
    }
    text.push_str("000000000010366d <svmvisor_resident_fault_common>:\n");
    text.push_str(COMMON);
    text.push('\n');
    text
}

fn all_gates() -> Vec<u32> {
    (16..256).collect()
}

fn image() -> String {
    image_with(&all_gates(), &sx("svmvisor_resident_irq_30"))
}

#[test]
fn window_gates_and_sx_chain_pass() {
    let result = audit_host_fault(&image()).unwrap();
    assert_eq!(result.get("irq_window_gates"), Some(&Value::Int(239)));
    assert_eq!(
        result.get("terminal_only_vectors_below_32"),
        Some(&Value::ints((0..2).chain(3..16).chain([18])))
    );
    assert_eq!(
        result.get("returning_nmi_gate"),
        Some(&Value::Map(vec![
            ("vector".into(), Value::Int(2)),
            ("symbol".into(), Value::str("svmvisor_resident_nmi")),
            ("flag".into(), Value::str("svmvisor_resident_nmi_pending")),
        ]))
    );
    assert_eq!(
        result.get("sx_non_init_path"),
        Some(&Value::strs(&["svmvisor_resident_irq_30", "svmvisor_resident_fault_30"]))
    );
}

#[test]
fn alignment_padding_after_the_last_gate_is_ignored() {
    let tail = "\tiretq\n";
    let padded =
        gate(255).replace(tail, &format!("{tail}  100301:      \tint3\n  100302:      \tnop\n"));
    let result = audit_host_fault(&image().replace(&gate(255), &padded)).unwrap();
    assert_eq!(result.get("irq_window_gates"), Some(&Value::Int(239)));
    // Code after the gate's IRETQ is not padding.
    let extra = gate(255).replace(tail, &format!("{tail}  100301:      \tclgi\n"));
    assert!(audit_host_fault(&image().replace(&gate(255), &extra)).is_err());
}

#[test]
fn missing_wrong_or_bypassing_gates_fail() {
    let good = image();
    let default_sx = sx("svmvisor_resident_irq_30");
    let cases = [
        // vector 17 left as a bare stub
        image_with(&all_gates().into_iter().filter(|&v| v != 17).collect::<Vec<_>>(), &default_sx),
        // the old 32-255 set
        image_with(&(32..256).collect::<Vec<_>>(), &default_sx),
        // wrong fallthrough stub
        good.replace(&gate(21), &gate_to(21, 22)),
        // wrong recorded vector
        good.replace(&gate(40), &gate(40).replace("$0x28", "$0x29")),
        good.replace(&gate(200), &gate(200).replace("\tandq\t$-0x201", "\tandq\t$-0x1")),
        // #MC must stay terminal
        format!("{good}{}", gate(18)),
        // #SX skips the window check
        image_with(&all_gates(), &sx("svmvisor_resident_fault_30")),
        image_with(&all_gates(), &default_sx.replace("\tlock\n", "")),
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_ne!(text, &good, "case {index} did not mutate the image");
        assert!(audit_host_fault(text).is_err(), "case {index}");
    }
}

#[test]
fn nmi_gate_is_exact_and_vector_2_is_not_terminal() {
    let good = image();
    // The returning gate takes vector 2 out of the terminal-only list.
    assert!(terminal_only_vectors_below_32().all(|vector| vector != 2));
    assert_eq!(terminal_only_vectors_below_32().count(), 16);
    let padded = nmi_with("\tiretq\n", "\tiretq\n  1002cf:      \tnop\n");
    assert!(audit_host_fault(&padded).is_ok());
    let cases = [
        // missing symbol
        good.replace(&nmi(), ""),
        good.replace("<svmvisor_resident_nmi>:", "<svmvisor_resident_nmi_gate>:"),
        // extra instruction before, inside and after the sequence
        nmi_with("\tclgi\n", "\tcli\n  1002c0:      \tclgi\n"),
        nmi_with("\tiretq\n", "\tpushq\t%rax\n  1002cd:      \tiretq\n"),
        nmi_with("\tiretq\n", "\tiretq\n  1002cf:      \tclgi\n"),
        // missing CLGI, or a return that is not IRETQ
        nmi_with("1002c0:      \tclgi\n", ""),
        nmi_with("\tiretq\n", "\tretq\n"),
        // wrong flag symbol, offset, value or width
        nmi_with("<svmvisor_resident_nmi_pending>", "<svmvisor_resident_irq_window>"),
        nmi_with("<svmvisor_resident_nmi_pending>", "<svmvisor_resident_nmi_pending+0x4>"),
        nmi_with("$0x1,", "$0x0,"),
        nmi_with("\tmovl\t", "\tmovq\t"),
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_ne!(text, &good, "case {index} did not mutate the image");
        let error = audit_host_fault(text).unwrap_err();
        assert!(
            error.starts_with("NMI gate differs from the flag-and-return sequence"),
            "case {index}: {error}"
        );
    }
    // The unused vector-2 terminal stub stays linked and checked.
    let stub = "  102c01:      \tpushq\t$0x0\n  102c03:      \tpushq\t$0x2\n";
    assert!(
        audit_host_fault(&good.replacen(stub, "  102c03:      \tpushq\t$0x2\n", 1))
            .unwrap_err()
            .starts_with("host fault vector2 frame mismatch")
    );
}

#[test]
fn fault_frames_and_capture_sequence_are_enforced() {
    let good = image();
    for (needle, replacement, expected) in [
        (
            "  102c03:      \tpushq\t$0xe\n",
            "  102c01:      \tpushq\t$0x0\n  102c03:      \tpushq\t$0xe\n",
            "host fault vector14 frame mismatch",
        ),
        ("\tcld\n", "\tnop\n", "host fault capture sequence mismatch"),
        (
            "<svmvisor_resident_host_fault>",
            "<other>",
            "host fault latch/record/callback targets differ",
        ),
        (
            "\tltrw\t%ax\n  100078:      \tlidtq\t(%rax)\n",
            "\tlidtq\t(%rax)\n  100078:      \tltrw\t%ax\n",
            "private IDT selected before private TSS",
        ),
    ] {
        let text = good.replacen(needle, replacement, 1);
        assert_ne!(text, good);
        assert!(audit_host_fault(&text).unwrap_err().starts_with(expected), "{expected}");
    }
}
