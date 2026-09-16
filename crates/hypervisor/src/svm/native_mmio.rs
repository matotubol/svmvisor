//! Bounded native DWORD MOV decoder for owned UC device pages.
//! The callback is the only device owner; this module never accesses MMIO.
use super::{events::ExternalInterruptError, exit::ResumeError, vmcb::Vmcb};
use crate::arch::x86_64::registers::GuestRegisters;

fn field(vmcb: &Vmcb, offset: usize) -> u64 {
    u64::from_le_bytes(vmcb.bytes()[offset..offset + 8].try_into().unwrap())
}

/// Native MMIO refusals preserve stopped state. They are not guest #PF/#GP.
#[derive(Debug, PartialEq, Eq)]
pub enum NativeMmioError<OwnerError> {
    Fetch(crate::host::resident::fetch::FetchError),
    Instruction,
    UnsupportedMode,
    Operand,
    NestedFault,
    CacheType,
    PendingState(ExternalInterruptError),
    Continuation(ResumeError),
    Register(OwnerError),
}

/// Complete a final-data device NPF through its transactional register owner.
///
/// APM2 rev3.44 15.25.6,16.3.2 and APM3 rev3.37 1.2/1.4, MOV pp240–242.
/// Supports supervisor long64 DWORD MOV 89/8B/C7 /0 with one optional REX.W=0,
/// ModRM/SIB, signed displacements and RIP-relative addressing. Legacy prefixes,
/// other widths/opcodes, register-only operands and reads into ESP are refused.
/// EFER.UAIE applies only to the decoded default-DS data operand; default-SS
/// operands and instruction addresses retain normal canonical checks.
/// Each needed instruction byte is fetched from current stopped guest paging;
/// the complete operand is walked independently and matched to EXITINFO2.
/// Neither instruction nor MMIO backing is ever dereferenced as a guest pointer.
///
/// `read` must obey the owned WB RAM-reader contract of resident::fetch. Caller
/// owns coherent guest tables/instruction bytes throughout this operation and
/// admits `mmio_page` as an absent NPT page in the current installed root and
/// verifies the complete page has MTRR type UC. Under that prerequisite,
/// APM2 Table7-11 combines guest PAT UC/UC-/WP/WT/WB to UC; WC stays WC.
/// `access` owns all device admission and shared register semantics: Some(value) writes, None reads. It must
/// preserve all owners on Err and must not complete RIP. It is invoked only
/// after every decoder, mapping, event and continuation check; following success
/// this adapter commits the DWORD destination/RIP and consumes RF/shadow without
/// further fallible work. Reads zero-extend their destination; writes retain GPRs.
#[allow(clippy::too_many_arguments)]
pub fn handle_native_mmio<OwnerError>(
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    physical_bits: u8,
    pat: u64,
    mmio_page: u64,
    read: impl FnMut(u64, usize) -> Option<u64>,
    access: impl FnOnce(&mut Vmcb, u16, Option<u32>) -> Result<u32, OwnerError>,
) -> Result<(), NativeMmioError<OwnerError>> {
    handle_native_mmio_detailed(vmcb, frame, physical_bits, pat, mmio_page, read, access)
        .map_err(|failure| failure.error)
}

/// Exact check-site evidence, collected without additional guest or device reads.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeMmioFailure<OwnerError> {
    pub error: NativeMmioError<OwnerError>,
    pub predicate: u16,
    pub operand: u64,
}

/// The same MMIO owner with bounded terminal evidence; see `handle_native_mmio`.
#[allow(clippy::too_many_arguments)]
pub fn handle_native_mmio_detailed<OwnerError>(
    vmcb: &mut Vmcb, frame: &mut GuestRegisters, physical_bits: u8, pat: u64,
    mmio_page: u64, mut read: impl FnMut(u64, usize) -> Option<u64>,
    access: impl FnOnce(&mut Vmcb, u16, Option<u32>) -> Result<u32, OwnerError>,
) -> Result<(), NativeMmioFailure<OwnerError>> {
    let mut predicate = 0;
    let mut operand = 0;
    native_mmio_inner(vmcb, frame, physical_bits, pat, mmio_page, None, &mut read, access,
        &mut predicate, &mut operand).map_err(|error| NativeMmioFailure { error, predicate, operand })
}

#[allow(clippy::too_many_arguments)]
fn native_mmio_inner<OwnerError>(
    vmcb: &mut Vmcb, frame: &mut GuestRegisters, physical_bits: u8, pat: u64,
    mmio_page: u64, profile: Option<&super::x2avic::NativeX2AvicProfile>, mut read: impl FnMut(u64, usize) -> Option<u64>,
    access: impl FnOnce(&mut Vmcb, u16, Option<u32>) -> Result<u32, OwnerError>,
    predicate: &mut u16, operand: &mut u64,
) -> Result<(), NativeMmioError<OwnerError>> {
    use crate::host::resident::fetch;
    use NativeMmioError as E;
    let exit = vmcb.exit_snapshot();
    for (failed, reason, value, mode) in [
        (exit.code != 0x400, 4, exit.code, false),
        (field(vmcb, 0x90) != 1, 5, field(vmcb, 0x90), false),
        (field(vmcb, 0x4d0) & (1 << 10) == 0, 6, field(vmcb, 0x4d0), true),
        (vmcb.bytes()[0x413] & 2 == 0, 7, u16::from_le_bytes([vmcb.bytes()[0x412], vmcb.bytes()[0x413]]) as u64, true),
        (vmcb.bytes()[0x4cb] != 0, 8, vmcb.bytes()[0x4cb] as u64, true),
        (vmcb.guest_rflags() & ((1 << 8) | (1 << 17)) != 0, 9, vmcb.guest_rflags(), true),
        // APM2 3.1.3 (p51), 5.7.3 (p166), 18.1-2 (pp678-679):
        // CET does not change ordinary MOV or its sequential continuation.
        // Keep CET's required WP=1 invariant: the effective R/W check below
        // then forbids ordinary writes to shadow-stack pages (leaf R/W=0).
        // Bit24 remains reserved on this AMD profile; it is not Intel PKS.
        (field(vmcb, 0x548) & (1 << 24) != 0
            || (field(vmcb, 0x548) & (1 << 23) != 0 && field(vmcb, 0x558) & (1 << 16) == 0),
            10, field(vmcb, 0x548), true),
    ] {
        if failed { *predicate = reason; *operand = value;
            return Err(if mode { E::UnsupportedMode } else { E::NestedFault }); }
    }
    if mmio_page & 4095 != 0 || mmio_page == 0 {
        *predicate = 11; *operand = mmio_page;
        return Err(E::Operand);
    }
    vmcb.validate_external_interrupt_conflicts()
        .map_err(E::PendingState)?;
    match profile {
        Some(profile) => vmcb.validate_native_x2avic(profile),
        None => vmcb.validate_virtual_interrupt_controls(),
    }.map_err(E::PendingState)?;
    let mut instruction = [0; 15];
    let mut length = 0;
    let mut byte = || {
        let result = fetch::long_instruction_byte(vmcb, physical_bits, pat, length, &mut read)
            .map_err(E::Fetch)?;
        instruction[length] = result;
        length += 1;
        Ok(result)
    };
    let decoded_result = decode_native_mov(vmcb, frame, &mut byte);
    drop(byte);
    *predicate = 0x20;
    *operand = exit.rip.wrapping_add(length as u64);
    let decoded = match decoded_result {
        Ok(decoded) => decoded,
        Err(E::Instruction) => {
            *predicate = 0x10 + length.min(7) as u16;
            *operand = instruction[..length.min(7)].iter().enumerate()
                .fold((length as u64) << 56, |v, (i, b)| v | ((*b as u64) << (8*i)));
            return Err(E::Instruction);
        }
        Err(error) => return Err(error),
    };
    *predicate = 0x30;
    *operand = exit.rip.wrapping_add(length as u64);
    let next = exit
        .native_mmio_continuation(&instruction[..length])
        .map_err(|e| {
            *operand = match e {
                ResumeError::NonCanonicalRip => exit.rip,
                ResumeError::NripNotEstablished => exit.nrip,
                ResumeError::ExitDoesNotPermitCandidate => exit.code,
                ResumeError::InvalidInstructionLength => length as u64,
                _ => *operand,
            };
            E::Continuation(e)
        })?;
    let linear = if decoded.rip_relative {
        next.address().wrapping_add(decoded.address)
    } else {
        decoded.address
    };
    // APM2 rev3.44 5.10.1-2 (pp168-169): in long64, UAIE ignores
    // bits63:57 only for DS/ES data references. Sign-extend bit56 so the
    // existing four-level walker still checks bits56:48 against bit47.
    // Do this after the full wrapping effective-address calculation, never
    // to registers, instruction addresses or arbitrary data-walker inputs.
    let linear = if decoded.segment == NativeSegment::Data && field(vmcb, 0x4d0) & (1 << 20) != 0 {
        ((linear << 7) as i64 >> 7) as u64
    } else {
        linear
    };
    *predicate = 0x21; *operand = linear;
    let translated = fetch::long_translation(vmcb, physical_bits, pat, linear, false, &mut read)
        .map_err(E::Fetch)?;
    // Restrict this native admission to supervisor operands. Protection-key,
    // SMAP and user-access emulation do not become implicit MMIO support.
    for (failed, reason, value) in [
        (translated.user, 12, linear),
        (translated.remaining_bytes < 4, 13, linear),
        (decoded.write.is_some() && !translated.writable && field(vmcb, 0x558) & (1 << 16) != 0, 14, linear),
    ] {
        if failed { *predicate = reason; *operand = value; return Err(E::Operand); }
    }
    if !matches!((pat >> (translated.pat_index * 8)) & 255, 0 | 4 | 5 | 6 | 7) {
        *predicate = 15; *operand = pat;
        return Err(E::CacheType);
    }
    *predicate = 0x18; *operand = translated.physical_address;
    let offset = translated.physical_address.checked_sub(mmio_page)
        .filter(|&offset| offset <= 4092 && offset & 3 == 0).ok_or(E::Operand)? as u16;
    // Missing nested mapping, final data (not page walk), baseline US=1;
    // reject fetch/reserved/GMET/RMP/shadow-stack and unknown information bits.
    let expected_info = (1u64 << 32) | 4 | if decoded.write.is_some() { 2 } else { 0 };
    if exit.info1 != expected_info {
        *predicate = 0x19; *operand = exit.info1; return Err(E::NestedFault);
    }
    if exit.info2 != translated.physical_address {
        *predicate = 0x1a; *operand = translated.physical_address; return Err(E::NestedFault);
    }
    *predicate = 0x40;
    *operand = (offset as u64) | ((decoded.write.is_some() as u64) << 16)
        | ((decoded.write.unwrap_or(0) as u64) << 32);
    let result = access(vmcb, offset, decoded.write).map_err(E::Register)?;
    let mut rax = vmcb.guest_rax();
    if decoded.write.is_none() {
        if decoded.register == 0 {
            rax = result as u64;
        } else {
            *frame_register(frame, decoded.register) = result as u64;
        }
    }
    vmcb.commit_emulated_instruction(rax, next);
    vmcb.complete_native_instruction_state();
    Ok(())
}

struct NativeMov {
    address: u64,
    rip_relative: bool,
    segment: NativeSegment,
    register: u8,
    write: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeSegment {
    Data,
    Stack,
}

impl NativeSegment {
    fn for_base(register: u8) -> Self {
        // APM3 rev3.37 1.2.4 (p11), 1.4.4 (p23): only the actual
        // rSP/rBP base selects SS. REX.B-extended R12/R13 select DS;
        // an index register does not select the default segment.
        if matches!(register, 4 | 5) {
            Self::Stack
        } else {
            Self::Data
        }
    }
}

fn decode_native_mov<OwnerError>(
    vmcb: &Vmcb,
    frame: &GuestRegisters,
    byte: &mut impl FnMut() -> Result<u8, NativeMmioError<OwnerError>>,
) -> Result<NativeMov, NativeMmioError<OwnerError>> {
    use NativeMmioError::Instruction;
    let first = byte()?;
    let (rex, opcode) = if (0x40..=0x47).contains(&first) {
        (first, byte()?)
    } else {
        (0, first)
    };
    if !matches!(opcode, 0x89 | 0x8b | 0xc7) {
        return Err(Instruction);
    }
    let modrm = byte()?;
    let mode = modrm >> 6;
    let rm = modrm & 7;
    let register = ((modrm >> 3) & 7) | ((rex & 4) << 1);
    if mode == 3 || (opcode == 0xc7 && register != 0) || (opcode == 0x8b && register == 4) {
        return Err(Instruction);
    }
    let mut rip_relative = false;
    let mut segment = NativeSegment::Data;
    let mut displacement32 = mode == 2;
    let mut address = if rm == 4 {
        let sib = byte()?;
        let base = sib & 7;
        let index = ((sib >> 3) & 7) | ((rex & 2) << 2);
        let mut address = if mode == 0 && base == 5 {
            displacement32 = true;
            0
        } else {
            let register = base | ((rex & 1) << 3);
            segment = NativeSegment::for_base(register);
            native_register(vmcb, frame, register)
        };
        if index != 4 {
            address = address
                .wrapping_add(native_register(vmcb, frame, index).wrapping_shl((sib >> 6) as u32));
        }
        address
    } else if mode == 0 && rm == 5 {
        rip_relative = true;
        displacement32 = true;
        0
    } else {
        let register = rm | ((rex & 1) << 3);
        segment = NativeSegment::for_base(register);
        native_register(vmcb, frame, register)
    };
    if mode == 1 {
        address = address.wrapping_add(byte()? as i8 as i64 as u64);
    } else if displacement32 {
        address = address.wrapping_add(native_dword(byte)? as i32 as i64 as u64);
    }
    let write = match opcode {
        0x89 => Some(native_register(vmcb, frame, register) as u32),
        0xc7 => Some(native_dword(byte)?),
        _ => None,
    };
    Ok(NativeMov {
        address,
        rip_relative,
        segment,
        register,
        write,
    })
}

fn native_dword<OwnerError>(
    byte: &mut impl FnMut() -> Result<u8, NativeMmioError<OwnerError>>,
) -> Result<u32, NativeMmioError<OwnerError>> {
    Ok(u32::from_le_bytes([byte()?, byte()?, byte()?, byte()?]))
}

fn native_register(vmcb: &Vmcb, frame: &GuestRegisters, index: u8) -> u64 {
    match index {
        0 => vmcb.guest_rax(),
        1 => frame.rcx,
        2 => frame.rdx,
        3 => frame.rbx,
        4 => vmcb.guest_rsp(),
        5 => frame.rbp,
        6 => frame.rsi,
        7 => frame.rdi,
        8 => frame.r8,
        9 => frame.r9,
        10 => frame.r10,
        11 => frame.r11,
        12 => frame.r12,
        13 => frame.r13,
        14 => frame.r14,
        15 => frame.r15,
        _ => unreachable!("four-bit register"),
    }
}

fn frame_register(frame: &mut GuestRegisters, index: u8) -> &mut u64 {
    match index {
        1 => &mut frame.rcx,
        2 => &mut frame.rdx,
        3 => &mut frame.rbx,
        5 => &mut frame.rbp,
        6 => &mut frame.rsi,
        7 => &mut frame.rdi,
        8 => &mut frame.r8,
        9 => &mut frame.r9,
        10 => &mut frame.r10,
        11 => &mut frame.r11,
        12 => &mut frame.r12,
        13 => &mut frame.r13,
        14 => &mut frame.r14,
        15 => &mut frame.r15,
        _ => unreachable!("decoder refuses ESP; caller handles EAX"),
    }
}


/// Native source-device adapter. Rechecks the exact installed x2AVIC profile.
/// All other contracts, including transactional callback refusal and UC page
/// admission, are identical to `handle_native_mmio`. A callback must validate
/// its entire operation before its first hardware write; the decoder cannot
/// roll back device side effects.
#[allow(clippy::too_many_arguments)]
pub fn handle_x2avic_mmio_detailed<E>(
    vmcb: &mut Vmcb, frame: &mut GuestRegisters, physical_bits: u8, pat: u64,
    mmio_page: u64, profile: &super::x2avic::NativeX2AvicProfile,
    mut read: impl FnMut(u64, usize) -> Option<u64>,
    access: impl FnOnce(&mut Vmcb, u16, Option<u32>) -> Result<u32, E>,
) -> Result<(), NativeMmioFailure<E>> {
    let mut predicate = 0;
    let mut operand = 0;
    native_mmio_inner(vmcb, frame, physical_bits, pat, mmio_page, Some(profile),
        &mut read, access, &mut predicate, &mut operand)
        .map_err(|error| NativeMmioFailure { error, predicate, operand })
}
