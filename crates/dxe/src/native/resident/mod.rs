//! Inert preparation for the native ReadyToBoot callback's guest return.
//!
//! This module neither allocates nor captures hardware, registers a callback,
//! enables SVM, or authorizes launch. It connects the existing NativeBoundary
//! and core continuation validation without replacing native paging/state.

use svmvisor_hypervisor::{
    arch::x86_64::registers::GuestRegisters,
    boot::descriptors::{FirmwareSelectors, ParsedFirmwareGdt},
    guest::{
        continuation::{
            CONTINUATION_RFLAGS_MASK, NativeContinuationError, NativeContinuationRequest,
            prepare_native_with_efer,
        },
        state::GuestStateRequest,
    },
    host::descriptors::HostTablePointer,
    memory::address::{AddressPolicy, is_canonical_48},
    svm::{dispatch::NativeEfer, vmcb::Vmcb},
};

use crate::native::admission::boundary::NativeBoundary;

pub mod allocation;
pub mod bootstrap_paging;
pub mod bridge;
pub mod delivery;
pub mod launch;
pub mod memory;
pub mod physical;
pub mod processors;

/// Supplied actual capture at this callback invocation, not image-entry history.
/// The original stack record, GDT and auxiliary snapshot remain immutable.
/// Their hardware provenance, hidden-cache correspondence, full original xstate
/// image/capability validation, debugger/event ownership and same-CPU no-FP host
/// requirements are caller obligations. DR0–3 and qualified x87 pointer state
/// remain resident on the captured CPU; they are not synthesized here.
pub struct CallbackRequest<'a> {
    pub boundary: &'a NativeBoundary,
    /// Same-CPU feature admission for this exact captured logical EFER.
    /// The original boundary still requires a complete xstate capture.
    pub efer: NativeEfer,
    pub gdt: &'a ParsedFirmwareGdt<'a>,
    pub auxiliary: &'a Vmcb,
    pub dr6: u64,
    pub dr7: u64,
    pub pat: u64,
    pub stack: GuestStackSpan,
    pub sites: CallbackSites,
}

/// Description of the committed bootstrap state, not a runnable CPU token.
/// The guest first enters the ACK trampoline; only its later RET restores this
/// original native return PC/RSP/flags. A received ACK alone does not prove RET.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackPrepared {
    pub sites: CallbackSites,
    pub boundary_va: u64,
    pub saved_register_frame: u64,
    pub original_return_rip: u64,
    pub original_entry_rsp: u64,
    pub original_rflags: u64,
}

/// Addresses of the three symbols in the actually linked callback object.
/// `resume` starts MOV EAX,imm32 (five bytes); ACK is VMMCALL (three bytes).
/// Matching these offsets alone does not authenticate instructions or mappings.
/// The caller must bind these to the audited immutable linked callback bytes,
/// retain their complete epilogue executable through guest RET, and establish
/// that the original native page tables resolve them to that same backing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackSites {
    pub resume: u64,
    pub ack: u64,
    pub after_ack: u64,
}

/// Supplied retained guest linear stack extent. Numeric bounds are not evidence
/// of ownership, original contents, page permissions or NPT backing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestStackSpan {
    pub base: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackError {
    BoundaryShape,
    GdtMismatch,
    AuxiliarySelectorMismatch,
    OriginalFlags,
    StackRecipe,
    StackSpan,
    ReturnAddress,
    LinkedSites,
    Native(NativeContinuationError),
}

/// Validate and commit one never-entered stopped VMCB/register frame.
/// Every refusal leaves both destinations unchanged; source records are never
/// written. This combined operation permits the core token to borrow the local
/// derived frame without a self-referential or duplicate architectural owner.
///
/// All address checks are inert. The caller separately proves every original
/// stack byte, descriptor, page-table dependency, TLS page and instruction is
/// resident with the required guest/NPT permissions and exact native identity
/// backing. The whole callback frame and EFI return/shadow area must survive.
/// No Rust reference may remain live while the guest mutates its stack/state.
/// Numeric xstate metadata validation is not validation of the saved image or
/// raw x87 pointer fidelity. AMD APM2 rev3.44 15.5/15.7; UEFI2.11 2.3.4.2.
pub fn prepare_callback(
    request: CallbackRequest<'_>,
    policy: &AddressPolicy,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
) -> Result<CallbackPrepared, CallbackError> {
    use CallbackError as E;
    let b = request.boundary;
    if !b.has_valid_shape()
        || b.reserved.iter().any(|&byte| byte != 0)
        || b.gdtr.reserved != [0; 6]
        || b.idtr.reserved != [0; 6]
    {
        return Err(E::BoundaryShape);
    }
    if request.gdt.table() != (HostTablePointer { base: b.gdtr.base(), limit: b.gdtr.limit() })
        || request.gdt.selectors() != (FirmwareSelectors { cs: b.cs, ss: b.ss, ds: b.ds, es: b.es })
    {
        return Err(E::GdtMismatch);
    }
    for (offset, selector) in [(0x440, b.fs), (0x450, b.gs), (0x470, b.ldtr), (0x490, b.tr)] {
        let actual =
            u16::from_le_bytes(request.auxiliary.bytes()[offset..offset + 2].try_into().unwrap());
        if actual != selector {
            return Err(E::AuxiliarySelectorMismatch);
        }
    }
    // PUSHFQ cannot report RF/VM. TF, IOPL, AC and other unsupported state are
    // refused, never cleared to turn an unsupported original into valid input.
    const IF_DF: u64 = (1 << 9) | (1 << 10);
    let allowed_flags = CONTINUATION_RFLAGS_MASK | IF_DF | (1 << 21);
    if b.rflags & 2 == 0 || b.rflags & !allowed_flags != 0 {
        return Err(E::OriginalFlags);
    }
    let boundary_va = b as *const NativeBoundary as u64;
    // Original stack recipe in admission/boundary.S: push 128, subtract1472,
    // align down64, reserve64 before the 1408-byte NativeBoundary.
    let predicted = b
        .entry_rsp
        .checked_sub(1600)
        .and_then(|value| (value & !63).checked_add(64))
        .ok_or(E::StackRecipe)?;
    if b.entry_rsp & 15 != 8 || predicted != boundary_va {
        return Err(E::StackRecipe);
    }
    let guest_rsp = boundary_va.checked_sub(64).ok_or(E::StackRecipe)?;
    let saved_register_frame = b.entry_rsp.checked_sub(128).ok_or(E::StackRecipe)?;
    let required_end = b.entry_rsp.checked_add(40).ok_or(E::StackSpan)?;
    let boundary_end = boundary_va
        .checked_add(core::mem::size_of::<NativeBoundary>() as u64)
        .ok_or(E::StackSpan)?;
    let span_end = request.stack.base.checked_add(request.stack.bytes).ok_or(E::StackSpan)?;
    if request.stack.bytes == 0
        || request.stack.base == 0
        || !canonical_span(request.stack.base, request.stack.bytes)
        || !canonical_span(guest_rsp, required_end.checked_sub(guest_rsp).ok_or(E::StackSpan)?)
        || request.stack.base > guest_rsp
        || span_end < required_end
        || boundary_end > saved_register_frame
    {
        return Err(E::StackSpan);
    }
    if b.entry_rip == 0 || !is_canonical_48(b.entry_rip) {
        return Err(E::ReturnAddress);
    }
    let sites = request.sites;
    if sites.resume == 0
        || sites.resume.checked_add(5) != Some(sites.ack)
        || sites.ack.checked_add(3) != Some(sites.after_ack)
        || !canonical_span(sites.resume, 9)
    {
        return Err(E::LinkedSites);
    }
    let registers = GuestRegisters {
        rcx: b.gprs[1],
        rdx: b.gprs[2],
        rbx: b.gprs[3],
        rbp: b.gprs[4],
        rsi: b.gprs[5],
        rdi: b.gprs[6],
        r8: b.gprs[7],
        r9: b.gprs[8],
        r10: b.gprs[9],
        r11: b.gprs[10],
        r12: b.gprs[11],
        r13: b.gprs[12],
        // The original R14/R15 remain in the immutable firmware register frame.
        r14: boundary_va,
        r15: saved_register_frame,
    };
    let native = NativeContinuationRequest {
        entry: GuestStateRequest {
            rip: sites.resume,
            rsp: guest_rsp,
            rflags: b.rflags & !IF_DF,
            cr0: b.cr0,
            cr3: b.cr3,
            cr4: b.cr4,
            efer: b.efer,
            rax: b.gprs[0],
        },
        registers: &registers,
        gdt: request.gdt,
        idtr: HostTablePointer { base: b.idtr.base(), limit: b.idtr.limit() },
        auxiliary: request.auxiliary,
        cr2: b.cr2,
        dr6: request.dr6,
        dr7: request.dr7,
        cr8: b.cr8,
        pat: request.pat,
        xstate_profile: b.profile,
    };
    prepare_native_with_efer(native, policy, &request.efer)
        .map_err(E::Native)?
        .apply(vmcb, frame)
        .map_err(E::Native)?;
    Ok(CallbackPrepared {
        sites,
        boundary_va,
        saved_register_frame,
        original_return_rip: b.entry_rip,
        original_entry_rsp: b.entry_rsp,
        original_rflags: b.rflags,
    })
}

fn canonical_span(base: u64, bytes: u64) -> bool {
    let Some(last) = bytes.checked_sub(1).and_then(|length| base.checked_add(length)) else {
        return false;
    };
    is_canonical_48(base) && is_canonical_48(last) && base >> 47 == last >> 47
}
