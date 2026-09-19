//! Package an already-linked ELF64 image for the checked loader
//! (`crates/card-abi/src/package.rs` is the consumer and the authority
//! on the "SVMRELO1" format).
//!
//! No relinking happens at the requested load address. Retained ELF RELA
//! records identify every absolute image pointer; PC-relative references stay
//! invariant. Only the deliberately small static x86-64 relocation vocabulary
//! is accepted.
//!
//! Arithmetic is done in `i128` so that every comparison has the unbounded
//! integer meaning of the audited original; no input can wrap a check.

use std::collections::{BTreeSet, HashMap};

pub const BASE: i128 = 0x100000;
pub const ARENA: i128 = 0x100000;
pub const LIMIT: i128 = 0xFF000;

/// Elf64_Shdr.
struct Section {
    kind: u32,
    flags: u64,
    address: i128,
    offset: i128,
    size: i128,
    link: u32,
    info: u32,
    entry_size: u64,
}

/// Elf64_Sym fields used here.
struct Symbol {
    section_index: u16,
    value: i128,
}

pub fn package(elf: &[u8], image: &[u8]) -> Result<Vec<u8>, String> {
    if elf.len() < 7 || &elf[..7] != b"\x7fELF\x02\x01\x01" {
        return Err("Expected little-endian ELF64".into());
    }
    let header = checked_slice(elf, 0, 64)?;
    let (kind, machine) = (u16_at(header, 16), u16_at(header, 18));
    let section_offset = u64_at(header, 40) as i128;
    let (section_entry_size, section_count) = (u16_at(header, 58), u16_at(header, 60));
    if (kind, machine) != (2, 62) || section_entry_size != 64 {
        return Err("Expected linked x86-64 ELF with ordinary section headers".into());
    }
    let mut sections = Vec::new();
    for index in 0..section_count as i128 {
        let raw = checked_slice(elf, section_offset + index * 64, 64)?;
        sections.push(Section {
            kind: u32_at(raw, 4),
            flags: u64_at(raw, 8),
            address: u64_at(raw, 16) as i128,
            offset: u64_at(raw, 24) as i128,
            size: u64_at(raw, 32) as i128,
            link: u32_at(raw, 40),
            info: u32_at(raw, 44),
            entry_size: u64_at(raw, 56),
        });
    }
    let mut symbols: HashMap<String, i128> = HashMap::new();
    let mut symbol_tables: HashMap<usize, Vec<Symbol>> = HashMap::new();
    for (index, section) in sections.iter().enumerate() {
        if section.kind != 2 {
            continue;
        }
        if section.entry_size != 24 || section.size % 24 != 0 {
            return Err("Invalid symbol table".into());
        }
        let strings = sections
            .get(section.link as usize)
            .ok_or("Symbol table names an absent string table")?;
        let names = checked_slice(elf, strings.offset, strings.size)?;
        let mut table = Vec::new();
        let mut offset = section.offset;
        while offset < section.offset + section.size {
            let raw = checked_slice(elf, offset, 24)?;
            let name_offset = (u32_at(raw, 0) as usize).min(names.len());
            let name = names[name_offset..].split(|byte| *byte == 0).next().unwrap_or(&[]);
            if !name.is_ascii() {
                return Err("Symbol name is not ASCII".into());
            }
            let value = u64_at(raw, 8) as i128;
            symbols.insert(String::from_utf8_lossy(name).into_owned(), value);
            table.push(Symbol { section_index: u16_at(raw, 6), value });
            offset += 24;
        }
        symbol_tables.insert(index, table);
    }
    let symbol =
        |name: &str| symbols.get(name).copied().ok_or(format!("Missing linker symbol {name}"));
    if symbols.get("image_start") != Some(&BASE) {
        return Err("Unexpected linked image base".into());
    }
    let image_bytes = image.len() as i128;
    let memory_bytes = symbol("image_bss_end")? - BASE;
    if !(0 < image_bytes && image_bytes <= memory_bytes && memory_bytes <= LIMIT) {
        return Err("Image/BSS overlaps handoff page".into());
    }
    if symbol("image_load_end")? - BASE != image_bytes {
        return Err("Flat image length differs from ELF load extent".into());
    }
    // Verify the supplied flat image is exactly the ELF's allocated file data.
    for section in &sections {
        if section.flags & 2 != 0 && section.kind != 8 && section.size != 0 {
            let offset = section.address - BASE;
            if checked_slice(image, offset, section.size)?
                != checked_slice(elf, section.offset, section.size)?
            {
                return Err("Flat image differs from ELF section".into());
            }
        }
    }
    let entry = symbol("entry_uefi")? - BASE;
    if !(0 <= entry && entry < image_bytes) {
        return Err("Entry outside initialized image".into());
    }
    let mut relocations: Vec<(i128, i128)> = Vec::new();
    let mut retained = 0u64;
    for section in &sections {
        if section.kind != 4 && section.kind != 9 {
            continue;
        }
        let target = sections
            .get(section.info as usize)
            .ok_or("Relocation table names an absent target section")?;
        if target.flags & 2 == 0 {
            continue;
        }
        if section.kind != 4 || section.entry_size != 24 || section.size % 24 != 0 {
            return Err("Only ELF64 RELA relocation tables are supported".into());
        }
        let table = symbol_tables
            .get(&(section.link as usize))
            .ok_or("Relocation table names an absent symbol table")?;
        let mut offset = section.offset;
        while offset < section.offset + section.size {
            let raw = checked_slice(elf, offset, 24)?;
            offset += 24;
            let address = u64_at(raw, 0) as i128;
            let info = u64_at(raw, 8);
            let addend = u64_at(raw, 16) as i64 as i128;
            let (kind, symbol_index) = (info & 0xFFFF_FFFF, (info >> 32) as usize);
            if kind == 0 {
                continue;
            }
            retained += 1;
            let symbol = table.get(symbol_index).ok_or("Relocation names an absent symbol")?;
            let value = symbol.value;
            if symbol.section_index == 0 || !(BASE <= value && value <= BASE + memory_bytes) {
                return Err(format!("Relocation targets unowned/undefined symbol at {address:#x}"));
            }
            if ![1, 2, 4, 9, 10, 11].contains(&kind) {
                return Err(format!("Unsupported absolute/relative relocation type {kind}"));
            }
            let width: i128 = if kind == 1 { 8 } else { 4 };
            let location = address - BASE;
            let raw = checked_slice(image, location, width)?;
            if address < target.address || address + width > target.address + target.size {
                return Err("Relocation lies outside its target section".into());
            }
            if kind == 9 {
                // lld may relax a GOT load to an invariant direct LEA. Otherwise
                // the linker-created GOT slot has no retained RELA of its own:
                // synthesize its pointer relocation from the resolved operand.
                let resolved = address + signed(raw) - addend;
                if resolved == value {
                    continue;
                }
                let slot = resolved - BASE;
                let owned_data = sections.iter().any(|s| {
                    s.flags & 3 == 3
                        && s.address <= resolved
                        && resolved + 8 <= s.address + s.size
                        && s.kind != 8
                });
                if resolved.rem_euclid(8) != 0
                    || !owned_data
                    || unsigned(checked_slice(image, slot, 8)?) != value
                {
                    return Err("GOTPCREL does not resolve to an owned matching GOT slot".into());
                }
                relocations.push((slot, 8));
                continue;
            }
            if kind == 2 || kind == 4 {
                // PC32/PLT32 addends contain -4 for the displacement field.
                let reference = value + addend + 4;
                if !(BASE <= reference && reference <= BASE + memory_bytes) {
                    return Err("PC-relative reference escapes image ownership".into());
                }
                if signed(raw) != value + addend - address {
                    return Err("Linked relative value differs from relocation".into());
                }
                continue;
            }
            let expected = value + addend;
            if !(BASE <= expected && expected <= BASE + memory_bytes) {
                return Err("Absolute reference escapes image ownership".into());
            }
            let linked = if kind == 11 { signed(raw) } else { unsigned(raw) };
            if linked != expected {
                return Err("Linked absolute value differs from relocation".into());
            }
            relocations.push((location, width));
        }
    }
    if retained == 0 || relocations.is_empty() {
        return Err("No retained absolute relocations; link with --emit-relocs".into());
    }
    let relocations: BTreeSet<(i128, i128)> = relocations.into_iter().collect();
    let mut end = 0;
    for (offset, width) in &relocations {
        if *offset < end {
            return Err("Overlapping or duplicate relocation writes".into());
        }
        end = offset + width;
    }
    let mut result = Vec::with_capacity(64 + image.len() + relocations.len() * 16);
    result.extend_from_slice(b"SVMRELO1");
    for field in [BASE, ARENA, image_bytes, memory_bytes, entry, relocations.len() as i128, 0] {
        result.extend_from_slice(&(field as u64).to_le_bytes());
    }
    result.extend_from_slice(image);
    for (offset, width) in &relocations {
        result.extend_from_slice(&(*offset as u64).to_le_bytes());
        result.extend_from_slice(&(*width as u64).to_le_bytes());
    }
    Ok(result)
}

/// Number of runtime relocations recorded in a package header.
pub fn relocation_count(package: &[u8]) -> u64 {
    u64_at(package, 48)
}

fn checked_slice(data: &[u8], offset: i128, size: i128) -> Result<&[u8], String> {
    if offset < 0 || size < 0 || offset + size > data.len() as i128 {
        return Err("ELF range lies outside file".into());
    }
    Ok(&data[offset as usize..(offset + size) as usize])
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn unsigned(raw: &[u8]) -> i128 {
    let mut bytes = [0u8; 8];
    bytes[..raw.len()].copy_from_slice(raw);
    u64::from_le_bytes(bytes) as i128
}

fn signed(raw: &[u8]) -> i128 {
    match raw.len() {
        4 => i32::from_le_bytes(raw.try_into().unwrap()) as i128,
        _ => i64::from_le_bytes(raw.try_into().unwrap()) as i128,
    }
}

#[cfg(test)]
mod tests {
    //! ELF fixtures test relocation semantics, including linker-synthesized GOT.

    use super::{BASE, package};

    const B: i64 = BASE as i64;

    fn put(buffer: &mut [u8], offset: usize, bytes: &[u8]) {
        buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    /// Elf64_Shdr: "<IIQQQQIIQQ".
    #[allow(clippy::too_many_arguments)]
    fn section(
        kind: u32,
        flags: u64,
        address: u64,
        offset: u64,
        size: u64,
        link: u32,
        info: u32,
        align: u64,
        entry: u64,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        for value in [flags, address, offset, size] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&link.to_le_bytes());
        out.extend_from_slice(&info.to_le_bytes());
        out.extend_from_slice(&align.to_le_bytes());
        out.extend_from_slice(&entry.to_le_bytes());
        out
    }

    fn fixture(
        relocations: &[(usize, u64, i64)],
        target: i64,
        got_value: Option<i64>,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut image = vec![0u8; 64];
        put(&mut image, 0, &target.to_le_bytes());
        if let Some(value) = got_value {
            put(&mut image, 24, &value.to_le_bytes());
        }
        for &(offset, kind, addend) in relocations {
            match kind {
                2 | 4 | 9 => {
                    let destination =
                        if kind == 9 && got_value.is_some() { B + 24 } else { target };
                    put(
                        &mut image,
                        offset,
                        &((destination + addend - (B + offset as i64)) as i32).to_le_bytes(),
                    );
                }
                10 | 11 => put(&mut image, offset, &((target + addend) as u32).to_le_bytes()),
                1 => put(&mut image, offset, &(target + addend).to_le_bytes()),
                _ => {}
            }
        }
        let names: &[u8] = b"\0image_start\0image_load_end\0image_bss_end\0entry_uefi\0target\0";
        let find = |name: &[u8]| {
            names.windows(name.len()).position(|window| window == name).unwrap() as u32
        };
        let mut syms = vec![0u8; 24];
        for (name, value) in [
            (&b"image_start"[..], B),
            (b"image_load_end", B + 64),
            (b"image_bss_end", B + 128),
            (b"entry_uefi", B + 16),
            (b"target", target),
        ] {
            // Elf64_Sym: "<IBBHQQ".
            syms.extend_from_slice(&find(name).to_le_bytes());
            syms.extend_from_slice(&[0x10, 0]);
            syms.extend_from_slice(&1u16.to_le_bytes());
            syms.extend_from_slice(&(value as u64).to_le_bytes());
            syms.extend_from_slice(&0u64.to_le_bytes());
        }
        let mut rela = Vec::new();
        for &(offset, kind, addend) in relocations {
            rela.extend_from_slice(&((B + offset as i64) as u64).to_le_bytes());
            rela.extend_from_slice(&((5u64 << 32) | kind).to_le_bytes());
            rela.extend_from_slice(&addend.to_le_bytes());
        }
        let content_start = 64 + 5 * 64;
        let sym_start = content_start + image.len() as u64;
        let str_start = sym_start + syms.len() as u64;
        let rela_start = str_start + names.len() as u64;
        let mut sections = vec![0u8; 64];
        sections.extend(section(1, 3, B as u64, content_start, 64, 0, 0, 8, 0));
        sections.extend(section(2, 0, 0, sym_start, syms.len() as u64, 3, 1, 8, 24));
        sections.extend(section(3, 0, 0, str_start, names.len() as u64, 0, 0, 1, 0));
        sections.extend(section(4, 0, 0, rela_start, rela.len() as u64, 2, 1, 8, 24));
        // Elf64_Ehdr: "<16sHHIQQQIHHHHHH".
        let mut header = Vec::new();
        header.extend_from_slice(b"\x7fELF\x02\x01\x01");
        header.extend_from_slice(&[0u8; 9]);
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&62u16.to_le_bytes());
        header.extend_from_slice(&1u32.to_le_bytes());
        header.extend_from_slice(&((B + 16) as u64).to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes());
        header.extend_from_slice(&64u64.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        for value in [64u16, 0, 0, 64, 5, 0] {
            header.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(header.len(), 64);
        let mut elf = header;
        elf.extend(sections);
        elf.extend_from_slice(&image);
        elf.extend(syms);
        elf.extend_from_slice(names);
        elf.extend(rela);
        (elf, image)
    }

    fn default_fixture() -> (Vec<u8>, Vec<u8>) {
        fixture(&[(0, 1, 0)], B + 32, None)
    }

    fn word(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    fn records(result: &[u8]) -> Vec<(u64, u64)> {
        let count = word(result, 48) as usize;
        let size = word(result, 24) as usize;
        (0..count)
            .map(|i| (word(result, 64 + size + i * 16), word(result, 64 + size + i * 16 + 8)))
            .collect()
    }

    fn run(
        relocations: &[(usize, u64, i64)],
        target: i64,
        got_value: Option<i64>,
    ) -> Result<Vec<u8>, String> {
        let (elf, image) = fixture(relocations, target, got_value);
        package(&elf, &image)
    }

    fn rejected(result: Result<Vec<u8>, String>, needle: &str) {
        let error = result.expect_err("fixture must be rejected");
        assert!(error.contains(needle), "{error}");
    }

    #[test]
    fn absolute_widths_and_boundary_pointer() {
        let result = run(&[(0, 1, 0), (8, 10, 0), (12, 11, 0)], B + 128, None).unwrap();
        assert_eq!(records(&result), [(0, 8), (8, 4), (12, 4)]);
        let header: Vec<u64> = (0..7).map(|i| word(&result, 8 + i * 8)).collect();
        assert_eq!(header, [B as u64, B as u64, 64, 128, 16, 3, 0]);
        // Byte-exact container: magic, 7 little-endian words, image, records.
        assert_eq!(&result[..8], b"SVMRELO1");
        assert_eq!(result.len(), 64 + 64 + 3 * 16);
        let (_, image) = fixture(&[(0, 1, 0), (8, 10, 0), (12, 11, 0)], B + 128, None);
        assert_eq!(&result[64..128], &image[..]);
    }

    #[test]
    fn relative_calls_stay_unchanged() {
        let result = run(&[(0, 1, 0), (8, 2, -4), (12, 4, -4)], B + 32, None).unwrap();
        assert_eq!(records(&result), [(0, 8)]);
    }

    #[test]
    fn synthetic_got_slot_is_relocated_and_deduplicated() {
        let result = run(&[(0, 1, 0), (8, 9, -4), (12, 9, -4)], B + 32, Some(B + 32)).unwrap();
        assert_eq!(records(&result), [(0, 8), (24, 8)]);
    }

    #[test]
    fn relaxed_got_reference_needs_no_slot() {
        let result = run(&[(0, 1, 0), (8, 9, -4)], B + 32, None).unwrap();
        assert_eq!(records(&result), [(0, 8)]);
    }

    #[test]
    fn mismatching_and_external_got_slots_rejected() {
        for slot in [B + 33, 0x400000] {
            rejected(run(&[(0, 1, 0), (8, 9, -4)], B + 32, Some(slot)), "GOTPCREL");
        }
    }

    #[test]
    fn external_absolute_symbol_rejected() {
        rejected(run(&[(0, 1, 0)], B + 129, None), "unowned");
    }

    #[test]
    fn addend_escaping_image_rejected() {
        rejected(run(&[(0, 1, 129)], B + 32, None), "escapes");
    }

    #[test]
    fn unsupported_relocation_rejected() {
        rejected(run(&[(0, 1, 0), (8, 6, 0)], B + 32, None), "Unsupported");
    }

    #[test]
    fn no_retained_relocations_rejected() {
        rejected(run(&[], B + 32, None), "emit-relocs");
    }

    #[test]
    fn flat_image_substitution_rejected() {
        let (elf, mut image) = default_fixture();
        *image.last_mut().unwrap() = b'X';
        rejected(package(&elf, &image), "differs");
    }

    #[test]
    fn truncated_or_foreign_files_rejected() {
        rejected(package(b"MZ", &[0; 64]), "little-endian ELF64");
        let (elf, image) = default_fixture();
        rejected(package(&elf[..200], &image), "outside file");
    }
}
