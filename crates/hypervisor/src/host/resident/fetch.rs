//! Bounded stopped-guest instruction fetch for native CPUID/EFER dispatch.
//! Reuses the existing four-level paging parser; does not add a CPU decoder.
//! AMD APM2 rev3.44 4.8,5.3,7.8,15.7,15.25 and Appendix B.
use crate::{
    host::paging::{self, PagingConfig, WalkError},
    svm::vmcb::Vmcb,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    UnsupportedExit,
    UnsupportedMode,
    AddressOverflow,
    UnsupportedCacheControl,
    Walk(WalkError),
    NotExecutable,
    PrivilegeMismatch,
    NonWriteBackInstruction,
    UnreadableInstruction { address: u64 },
    SegmentLimit,
}

/// Opt-in startup fetch adds real16 and protected16/32 unpaged code to the
/// ordinary long64 walker. APM2 rev3.44 4.8/4.12,14.6,15.27.8. CS.base and
/// the complete two-byte instruction plus next IP are bounded before any read;
/// legacy paging, wrapping IP, VM86 and cache-disabled execution stay stopped.
/// The caller's physical reader must admit WB RAM at low SIPI addresses as
/// carefully as paging/instruction pages, including fixed-MTRR precedence.
pub fn startup_instruction(
    vmcb: &Vmcb,
    physical_bits: u8,
    pat: u64,
    mut read: impl FnMut(u64, usize) -> Option<u64>,
) -> Result<[u8; 2], FetchError> {
    if vmcb.guest_in_64_bit_code() {
        return instruction(vmcb, physical_bits, pat, read);
    }
    let exit = vmcb.exit_snapshot();
    if !matches!(exit.code, 0x72 | 0x7c) {
        return Err(FetchError::UnsupportedExit);
    }
    let cs = u16::from_le_bytes([vmcb.bytes()[0x412], vmcb.bytes()[0x413]]);
    if !(32..=52).contains(&physical_bits)
        || field(vmcb, 0x558) & 0xe000_0000 != 0
        || field(vmcb, 0x548) & ((1 << 12) | (1 << 17)) != 0
        || field(vmcb, 0x4d0) & (1 << 10) != 0
        || cs & 0x200 != 0
        || cs & 0x98 != 0x98
        || vmcb.bytes()[0x4cb] != 0
        || vmcb.guest_rflags() & (1 << 17) != 0
    {
        return Err(FetchError::UnsupportedMode);
    }
    let limit = u32::from_le_bytes(vmcb.bytes()[0x414..0x418].try_into().unwrap()) as u64;
    let ip_limit = if cs & 0x400 != 0 {
        u32::MAX as u64
    } else {
        u16::MAX as u64
    };
    let next = exit.rip.checked_add(2).ok_or(FetchError::AddressOverflow)?;
    if next > limit || next > ip_limit {
        return Err(FetchError::SegmentLimit);
    }
    let address = field(vmcb, 0x418)
        .checked_add(exit.rip)
        .ok_or(FetchError::AddressOverflow)?;
    let end = address.checked_add(1).ok_or(FetchError::AddressOverflow)?;
    if end > u32::MAX as u64 || end >= 1u64 << physical_bits {
        return Err(FetchError::AddressOverflow);
    }
    let mut bytes = [0; 2];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        let physical = address + offset as u64;
        *byte = read(physical, 1)
            .filter(|&value| value <= 255)
            .ok_or(FetchError::UnreadableInstruction { address: physical })? as u8;
    }
    Ok(bytes)
}

/// Fetch exactly two bytes at current guest RIP, independently walking each
/// byte so a page-crossing opcode may span noncontiguous physical pages.
/// `pat` is the stopped guest's G_PAT value, not the host PAT after VMEXIT.
/// `read` receives only widths 1 or 8 and must return zero-extended bytes or
/// a little-endian entry. It independently admits every complete physical
/// read as WB readable memory, outside monitor storage and MMIO, through an
/// owned host mapping. It must never dereference unchecked guest pointers.
///
/// Same-CPU stopped ownership/coherent tables and unchanged instruction backing
/// are caller preconditions. No A/D bits are changed or guest fault fabricated.
/// At most eight table reads and two byte reads occur; nRIP is never consumed.
/// Prefixes are retained, then refused by the existing exact-opcode dispatcher.
pub fn instruction(
    vmcb: &Vmcb,
    physical_bits: u8,
    pat: u64,
    mut read: impl FnMut(u64, usize) -> Option<u64>,
) -> Result<[u8; 2], FetchError> {
    let exit = vmcb.exit_snapshot();
    if exit.code != 0x72 && exit.code != 0x7c {
        return Err(FetchError::UnsupportedExit);
    }
    let mut bytes = [0; 2];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = instruction_byte(vmcb, physical_bits, pat, index, false, &mut read)?;
    }
    Ok(bytes)
}

/// Read exactly the hardware-decoded prefixed CPUID span (3..15 bytes).
/// The caller retains same-CPU stopped ownership and coherent instruction
/// backing through completion, and supplies the same independently admitted WB
/// physical reader as `instruction`. No byte is fetched past nRIP; a failed
/// read never authorizes completion. Two-byte native CPUID uses no fetch.
/// APM2 1.3,2.3,4.8,5.3,15.7.1: long-mode compatibility code uses the same
/// four-level walk after checked CS.base addition; legacy startup is unpaged.
/// Linear/IP wrap, VM86, cache-disabled execution and legacy paging are refused.
pub fn cpuid_instruction(vmcb: &Vmcb, physical_bits: u8, pat: u64,
    length: usize, startup_owned: bool, mut read: impl FnMut(u64, usize) -> Option<u64>)
    -> Result<[u8; 15], FetchError>
{
    let exit = vmcb.exit_snapshot();
    if exit.code != 0x72 { return Err(FetchError::UnsupportedExit); }
    if !(3..=15).contains(&length) || exit.rip.checked_add(length as u64) != Some(exit.nrip) {
        return Err(FetchError::AddressOverflow);
    }
    if !(32..=52).contains(&physical_bits)
        || !crate::svm::dispatch::native_cpuid_mode(vmcb, exit.nrip, startup_owned) {
        return Err(FetchError::UnsupportedMode);
    }
    if field(vmcb, 0x558) & 0x6000_0000 != 0 { return Err(FetchError::UnsupportedCacheControl); }
    let code64 = vmcb.guest_in_64_bit_code();
    let long_mode = field(vmcb, 0x4d0) & (1 << 10) != 0;
    let base = if code64 { 0 } else { field(vmcb, 0x418) };
    let start = base.checked_add(exit.rip).ok_or(FetchError::AddressOverflow)?;
    let end = start.checked_add(length as u64 - 1).ok_or(FetchError::AddressOverflow)?;
    if (!code64 && end > u32::MAX as u64)
        || (!long_mode && end >= 1u64 << physical_bits) {
        return Err(FetchError::AddressOverflow);
    }
    let mut bytes = [0; 15];
    for (index, byte) in bytes[..length].iter_mut().enumerate() {
        let address = start + index as u64;
        *byte = if long_mode {
            read_instruction_linear(vmcb, physical_bits, pat, address, false, true, &mut read)?
        } else {
            read(address, 1).filter(|&value| value <= 255)
                .ok_or(FetchError::UnreadableInstruction { address })? as u8
        };
    }
    Ok(bytes)
}

/// Owned-cache fallback for a stopped long64 guest with CD=1/NW=0. Caller
/// must own all physical cache/routing mutations, retain physical WB backing,
/// and enforce the current NPT permissions in every physical read. CD coherence
/// (APM2 15.25.8, PPR memory-type priority) permits those host WB aliases. Every
/// guest PAT selector and ordinary paging/permission check remains required.
#[cfg(any(feature = "resident-runtime", test))]
pub(crate) fn cache_disabled_instruction(vmcb: &Vmcb, physical_bits: u8, pat: u64,
    mut read: impl FnMut(u64, usize) -> Option<u64>) -> Result<[u8; 2], FetchError>
{
    if !matches!(vmcb.exit_snapshot().code, 0x72 | 0x7c)
        || field(vmcb, 0x558) & 0x6000_0000 != 0x4000_0000 {
        return Err(FetchError::UnsupportedCacheControl);
    }
    let mut bytes = [0; 2];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = instruction_byte(vmcb, physical_bits, pat, index, true, &mut read)?;
    }
    Ok(bytes)
}

/// One byte of a bounded stopped long64 instruction, under the same owned,
/// coherent WB physical-reader contract as `instruction`. The index is bounded
/// by the architectural 15-byte instruction limit.
fn instruction_byte(vmcb: &Vmcb, physical_bits: u8, pat: u64, index: usize,
    owned_cd: bool, mut read: impl FnMut(u64, usize) -> Option<u64>) -> Result<u8, FetchError> {
    if index >= 15 {
        return Err(FetchError::AddressOverflow);
    }
    let address = vmcb
        .exit_snapshot()
        .rip
        .checked_add(index as u64)
        .ok_or(FetchError::AddressOverflow)?;
    read_instruction_linear(vmcb, physical_bits, pat, address, owned_cd, false, &mut read)
}

fn read_instruction_linear(vmcb: &Vmcb, physical_bits: u8, pat: u64, address: u64,
    owned_cd: bool, cpuid_compat: bool, mut read: impl FnMut(u64, usize) -> Option<u64>)
    -> Result<u8, FetchError>
{
    let translated = translation(vmcb, physical_bits, pat, address, owned_cd, cpuid_compat, &mut read)?;
    if !translated.executable {
        return Err(FetchError::NotExecutable);
    }
    let cr4 = field(vmcb, 0x548);
    let cpl = vmcb.bytes()[0x4cb];
    if (cpl == 3 && !translated.user) || (cpl < 3 && cr4 & (1 << 20) != 0 && translated.user) {
        return Err(FetchError::PrivilegeMismatch);
    }
    if (pat >> (translated.pat_index * 8)) & 255 != 6 {
        return Err(FetchError::NonWriteBackInstruction);
    }
    read(translated.physical_address, 1)
        .filter(|&value| value <= 255)
        .map(|value| value as u8)
        .ok_or(FetchError::UnreadableInstruction {
            address: translated.physical_address,
        })
}

/// Resolve one long64 instruction address without dereferencing its backing.
/// The caller validates executable/privilege permissions and the leaf cache
/// type. Only table reads use the independently admitted WB RAM reader.
/// `pat` is the stopped guest's G_PAT. Each paging-structure access must select
/// WB from it; the reader separately proves compatible NPT/host PAT and MTRRs.
/// Guest AVL bits are ignored; host mapping admission retains its strict policy.
/// MPK does not apply to instruction fetch, so protection keys are ignored.
fn translation(vmcb: &Vmcb, physical_bits: u8, pat: u64, address: u64,
    owned_cd: bool, cpuid_compat: bool, mut read: impl FnMut(u64, usize) -> Option<u64>)
    -> Result<paging::Translation, FetchError>
{
    let efer = field(vmcb, 0x4d0);
    let cr0 = field(vmcb, 0x558);
    let cr4 = field(vmcb, 0x548);
    let cr3 = field(vmcb, 0x550);
    let cs = u16::from_le_bytes([vmcb.bytes()[0x412], vmcb.bytes()[0x413]]);
    let cpl = vmcb.bytes()[0x4cb];
    // Generic users require long64 CS.L=1/CS.D=0. The dedicated CPUID fetch
    // validates compatibility mode and CS.base + offset before this walk.
    if cr0 & 0x8000_0001 != 0x8000_0001
        || cr4 & (1 << 5) == 0
        || cr4 & (1 << 12) != 0
        || efer & 0x500 != 0x500
        || (cs & 0x600 != 0x200
            && !(cpuid_compat && vmcb.exit_snapshot().code == 0x72 && cs & 0x200 == 0))
        || cpl > 3
    {
        return Err(FetchError::UnsupportedMode);
    }
    let pcid = cr4 & (1 << 17) != 0;
    // APM2 rev3.44 7.8.1/Table7-9 (p228): reserved bits and unsupported
    // encodings are invalid in every PAT slot, even one not used by this walk.
    // 15.25.8/Tables15-19..20 (pp553-554): WB guest PAT + WB nested PAT +
    // WB MTRRs yields WB, while guest CR0.CD can still disable caching.
    if (cr0 & (1 << 30) != 0 && !owned_cd) || !valid_pat(pat) {
        return Err(FetchError::UnsupportedCacheControl);
    }
    let config = PagingConfig {
        cr3,
        physical_bits,
        la57: false,
        nxe: efer & (1 << 11) != 0,
        pcid,
        // Native runtime admission requires hardware's 1-GiB-page capability.
        page1gb: true,
    };
    let mut cache_control = false;
    let mut level = 4;
    // APM2 5.3.2/Fig5-16 and 5.5.1 (pp140-141,158): with PCIDE set,
    // CR3[11:0] is a PCID, not PCD/PWT. Otherwise CR3 selects the root type.
    let mut table_pat_index = if pcid { 0 } else { (cr3 >> 3) & 3 };
    let translated = paging::translate(config, address, |physical| {
        // Check the type of this table before reading it, including PA0.
        // APM2 7.8.4 (p230): a nonleaf's PCD/PWT selects PA0..PA3 for the
        // next lower table. A selector itself is not a memory-type encoding.
        if (pat >> (table_pat_index * 8)) & 255 != 6 {
            cache_control = true;
            return None;
        }
        let entry = read(physical, 8)?;
        let leaf = level == 1 || (level < 4 && entry & (1 << 7) != 0);
        level -= 1;
        if !leaf {
            table_pat_index = (entry >> 3) & 3;
        }
        // APM2 rev3.44 5.3.3-5.3.5, Figs5-20..23/5-29/5-34 and 5.4 (p156):
        // nonleaf62:52 and leaf58:52 are available to guest software. Leaf
        // 62:59 is also available with CR4.PKE=0. 5.6.7 (p165) explicitly
        // ignores MPK on instruction fetches, so leaf keys are ignored too.
        // Normalize only this local copy;
        // preserve physical bits51:12, NX63, PS, PAT and all lower checks in
        // the shared strict parser, without changing any host admission caller.
        Some(entry & !0x7ff0_0000_0000_0000)
    });
    if cache_control {
        return Err(FetchError::UnsupportedCacheControl);
    }
    translated.map_err(FetchError::Walk)
}

fn valid_pat(pat: u64) -> bool {
    // Family1Ah Model44h B0 PPR57896 rev3.00 PAT tables (pp171-172)
    // reserve UC- in slots other than PA2/PA6. Keep this native profile
    // conservative where that target is narrower than APM2 Table7-9.
    (0..8).all(|index| match (pat >> (index * 8)) & 255 {
        0 | 1 | 4 | 5 | 6 => true,
        7 => index == 2 || index == 6,
        _ => false,
    })
}

fn field(vmcb: &Vmcb, offset: usize) -> u64 {
    let mut bytes = [0; 8];
    for (destination, source) in bytes.iter_mut().zip(vmcb.bytes().iter().skip(offset)) {
        *destination = *source;
    }
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_owner_entrypoint_retains_generic_cd_refusal_nw_and_selected_pat_checks() {
        let mut stopped = vmcb(false, true);
        for (offset, value) in [(0x70, 0x7cu64), (0x558, 0xc0000001)] {
            unsafe { core::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(),
                (&mut stopped as *mut Vmcb).cast::<u8>().add(offset), 8); }
        }
        let read = |address: u64, bytes: usize| match (address, bytes) {
            (0x1000,8) => Some(0x2007), (0x2000,8) => Some(0x3007),
            (0x3000,8) => Some(0x4007), (0x4000,8) => Some(0x9007),
            (0x9000,1) => Some(0x0f), (0x9001,1) => Some(0x30), _ => None,
        };
        assert_eq!(instruction(&stopped,48,6,read), Err(FetchError::UnsupportedCacheControl));
        assert_eq!(cache_disabled_instruction(&stopped,48,6,read), Ok([0x0f,0x30]));
        assert_eq!(cache_disabled_instruction(&stopped,48,0,read), Err(FetchError::UnsupportedCacheControl));
        assert!(matches!(cache_disabled_instruction(&stopped,48,6,|address, bytes| {
            if address == 0x2000 { None } else { read(address, bytes) }
        }), Err(FetchError::Walk(_))));
        assert_eq!(cache_disabled_instruction(&stopped,48,6,|address, bytes| {
            if bytes == 1 { None } else { read(address, bytes) }
        }), Err(FetchError::UnreadableInstruction { address: 0x9000 }));
        assert_eq!(cache_disabled_instruction(&stopped,48,6,|address, bytes| {
            if address == 0x4000 { Some(0x8000_0000_0000_9007) } else { read(address, bytes) }
        }), Err(FetchError::NotExecutable));
        unsafe { core::ptr::copy_nonoverlapping(0xe0000001u64.to_le_bytes().as_ptr(),
            (&mut stopped as *mut Vmcb).cast::<u8>().add(0x558), 8); }
        assert_eq!(cache_disabled_instruction(&stopped,48,6,read), Err(FetchError::UnsupportedCacheControl));
    }

    fn vmcb(pke: bool, nxe: bool) -> Vmcb {
        let mut vmcb = Vmcb::new();
        for (offset, value) in [
            (0x410, 0x200u64 << 16),
            (0x4d0, 0x1500 | if nxe { 1 << 11 } else { 0 }),
            (0x548, 0x20 | if pke { 1 << 22 } else { 0 }),
            (0x550, 0x1000),
            (0x558, 0x8000_0001),
        ] {
            // Test-only construction of stopped state; never enters hardware.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    value.to_le_bytes().as_ptr(),
                    (&mut vmcb as *mut Vmcb).cast::<u8>().add(offset),
                    8,
                );
            }
        }
        vmcb
    }

    fn entries(leaf: usize) -> [u64; 4] {
        let mut entries = [0x2007, 0x3007, 0x4007, 0x9007];
        entries[leaf] = match leaf {
            1 => 0x4000_0087,
            2 => 0x20_0087,
            3 => 0x9007,
            _ => panic!("invalid leaf"),
        };
        entries
    }

    fn walk(vmcb: &Vmcb, entries: [u64; 4]) -> Result<paging::Translation, FetchError> {
        let mut reads = 0;
        let result = translation(vmcb, 48, 6, 0x123, false, false, |address, width| {
            assert_eq!(width, 8);
            assert_eq!(address, 0x1000 + reads * 4096);
            let entry = entries[reads as usize];
            reads += 1;
            Some(entry)
        });
        assert!(reads <= 4);
        result
    }

    #[test]
    fn every_instruction_leaf_pat_selector_uses_its_selected_type() {
        for leaf in 1..=3 {
            for selector in 0..8 {
                for memory_type in [0, 1, 4, 5, 6, 7] {
                    if memory_type == 7 && selector != 2 && selector != 6 {
                        continue;
                    }
                    // PA0 must remain WB for the root and intermediate tables.
                    // Use PA1 for those accesses when testing a non-WB PA0.
                    let mut state = vmcb(false, true);
                    let mut marked = entries(leaf);
                    let pat = (0x0606_0606_0606_0606 & !(255 << (selector * 8)))
                        | (memory_type << (selector * 8));
                    if selector == 0 {
                        unsafe {
                            core::ptr::write_unaligned(
                                (&mut state as *mut Vmcb)
                                    .cast::<u8>()
                                    .add(0x550)
                                    .cast::<u64>(),
                                0x1008,
                            );
                        }
                        for entry in &mut marked[..leaf] {
                            *entry |= 1 << 3;
                        }
                    }
                    let pat_bit = if leaf == 3 { 7 } else { 12 };
                    marked[leaf] |= ((selector & 3) << 3) | ((selector >> 2) << pat_bit);
                    let mut table_reads = 0;
                    let mut byte_reads = 0;
                    let original = *state.bytes();
                    let result = instruction_byte(&state, 48, pat, 0, false, |_, width| {
                        if width == 8 {
                            let entry = marked[table_reads];
                            table_reads += 1;
                            Some(entry)
                        } else {
                            byte_reads += 1;
                            Some(0x0f)
                        }
                    });
                    assert_eq!(table_reads, leaf + 1);
                    if memory_type == 6 {
                        assert_eq!(result, Ok(0x0f));
                        assert_eq!(byte_reads, 1);
                    } else {
                        assert_eq!(result, Err(FetchError::NonWriteBackInstruction));
                        assert_eq!(byte_reads, 0);
                    }
                    assert_eq!(*state.bytes(), original);
                }
            }
        }
    }

    #[test]
    fn every_table_selector_is_checked_before_reading_its_target() {
        for leaf in 1..=3 {
            for parent in 0..leaf {
                for selector in 0..4 {
                    for memory_type in [0, 1, 4, 5, 6, 7] {
                        if memory_type == 7 && selector != 2 {
                            continue;
                        }
                        let mut state = vmcb(false, true);
                        let mut marked = entries(leaf);
                        // All unrelated table accesses select the other WB slot.
                        let other = if selector == 0 { 1 } else { 0 };
                        unsafe {
                            core::ptr::write_unaligned(
                                (&mut state as *mut Vmcb)
                                    .cast::<u8>()
                                    .add(0x550)
                                    .cast::<u64>(),
                                0x1000 | (other << 3),
                            );
                        }
                        for entry in &mut marked[..leaf] {
                            *entry |= other << 3;
                        }
                        marked[parent] = (marked[parent] & !0x18) | (selector << 3);
                        let pat = (0x0606_0606_0606_0606 & !(255 << (selector * 8)))
                            | (memory_type << (selector * 8));
                        let mut reads = 0;
                        let result = translation(&state, 48, pat, 0, false, false, |_, width| {
                            assert_eq!(width, 8);
                            let entry = marked[reads];
                            reads += 1;
                            Some(entry)
                        });
                        if memory_type == 6 {
                            assert!(result.is_ok());
                            assert_eq!(reads, leaf + 1);
                        } else {
                            assert_eq!(result, Err(FetchError::UnsupportedCacheControl));
                            assert_eq!(reads, parent + 1);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn supervisor_keys_are_ignored_at_every_permission_depth_and_leaf_size() {
        for leaf in 1..=3 {
            for supervisor_level in 0..=leaf {
                for key in 0..16 {
                    let state = vmcb(true, true);
                    let before = *state.bytes();
                    let mut marked = entries(leaf);
                    marked[supervisor_level] &= !4;
                    let baseline = walk(&state, marked).unwrap();
                    marked[leaf] |= key << 59;
                    assert_eq!(walk(&state, marked), Ok(baseline));
                    assert!(!baseline.user);
                    assert_eq!(*state.bytes(), before);
                }
            }
        }
    }

    #[test]
    fn instruction_walk_ignores_guest_upper_bits_at_every_level_with_and_without_pke() {
        for leaf in 1..=3 {
            for pke in [false, true] {
                let vmcb = vmcb(pke, true);
                let original = *vmcb.bytes();
                let baseline = walk(&vmcb, entries(leaf)).unwrap();
                for index in 0..=leaf {
                    for bits in (52..=62).map(|bit| 1 << bit).chain([0x7ff0_0000_0000_0000]) {
                        let mut marked = entries(leaf);
                        marked[index] |= bits;
                        assert_eq!(walk(&vmcb, marked), Ok(baseline));
                    }
                }
                assert_eq!(*vmcb.bytes(), original);
            }
        }
    }

    #[test]
    fn avl_normalization_preserves_reserved_nx_ps_alignment_and_permissions() {
        for leaf in 1..=3 {
            for index in 0..=leaf {
                let mut marked = entries(leaf).map(|entry| entry | 0x7ff0_0000_0000_0000);
                for bit in 48..=51 {
                    marked[index] |= 1 << bit;
                    assert_eq!(
                        walk(&vmcb(false, true), marked),
                        Err(FetchError::Walk(WalkError::ReservedEntry {
                            level: (4 - index) as u8
                        }))
                    );
                    marked[index] &= !(1 << bit);
                }
                marked[index] |= 1 << 63;
                assert_eq!(
                    walk(&vmcb(false, false), marked),
                    Err(FetchError::Walk(WalkError::ReservedEntry {
                        level: (4 - index) as u8
                    }))
                );
                assert!(!walk(&vmcb(false, true), marked).unwrap().executable);
                marked[index] &= !(1 << 63);
                for flag in [2, 4] {
                    marked[index] &= !flag;
                    let result = walk(&vmcb(false, true), marked).unwrap();
                    assert_eq!(result.writable, flag != 2);
                    assert_eq!(result.user, flag != 4);
                    marked[index] |= flag;
                }
                marked[index] &= !1;
                assert_eq!(
                    walk(&vmcb(false, true), marked),
                    Err(FetchError::Walk(WalkError::NotPresent {
                        level: (4 - index) as u8
                    }))
                );
            }
            let mut marked = entries(leaf).map(|entry| entry | 0x7ff0_0000_0000_0000);
            marked[0] |= 1 << 7;
            assert_eq!(
                walk(&vmcb(false, true), marked),
                Err(FetchError::Walk(WalkError::ReservedEntry { level: 4 }))
            );
            if leaf < 3 {
                marked[0] &= !(1 << 7);
                let shift = 12 + 9 * (3 - leaf);
                for bit in 13..shift {
                    marked[leaf] |= 1 << bit;
                    assert_eq!(
                        walk(&vmcb(false, true), marked),
                        Err(FetchError::Walk(WalkError::ReservedEntry {
                            level: (4 - leaf) as u8
                        }))
                    );
                    marked[leaf] &= !(1 << bit);
                }
                marked[leaf] |= 1 << 12; // Large-page PAT remains valid.
                assert_eq!(walk(&vmcb(false, true), marked).unwrap().pat_index, 4);
            }
        }
    }
}
