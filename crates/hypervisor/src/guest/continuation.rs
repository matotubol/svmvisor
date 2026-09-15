//! Captured continuation inputs for the integer fixture and native DXE adapter.
//!
//! This is an input snapshot, not a second live register owner. On transfer,
//! RAX/RIP/RSP/RFLAGS belong to the VMCB and the remaining GPRs to its register
//! frame (AMD APM vol. 2 rev. 3.44 section 15.7). Discard the snapshot after
//! initialization; never refresh runtime state from it. The integer fixture ABI
//! does not capture SIMD, TLS, segment/control/debug registers. Native inputs
//! below compose separately captured state under explicit adapter obligations.

use crate::arch::x86_64::registers::GuestRegisters;
use crate::memory::address::{PhysicalRange, is_canonical_48};

/// CF, PF, AF, ZF, SF and OF, plus architectural fixed-one bit 1.
pub const CONTINUATION_RFLAGS_MASK: u64 = 0x8d7;

#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IntegerContinuation {
    pub registers: GuestRegisters,
    pub rax: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuationError {
    InvalidCodePage,
    InvalidStackPage,
    OverlappingPages,
    RipOutsideCode,
    RspOutsideStack,
    UnsupportedRflags,
}

impl IntegerContinuation {
    /// Check the fixture's identity-addressed code and stack bounds, including
    /// both eight-byte stack markers read after resume. These numeric checks do
    /// not prove allocation ownership or guest/NPT permissions. The caller must
    /// install the same pages, immutable executable code, and explicit guest CR3.
    pub fn validate_bounds(
        &self,
        code: PhysicalRange,
        stack: PhysicalRange,
    ) -> Result<(), ContinuationError> {
        if code.len() != 4096 || code.base() & 4095 != 0 {
            return Err(ContinuationError::InvalidCodePage);
        }
        if stack.len() != 4096 || stack.base() & 4095 != 0 {
            return Err(ContinuationError::InvalidStackPage);
        }
        if code.base() == stack.base() {
            return Err(ContinuationError::OverlappingPages);
        }
        if !is_canonical_48(self.rip) || self.rip < code.base() || self.rip > code.last_byte() {
            return Err(ContinuationError::RipOutsideCode);
        }
        if !is_canonical_48(self.rsp)
            || self.rsp & 7 != 0
            || self.rsp < stack.base()
            || self.rsp > stack.last_byte() - 15
        {
            return Err(ContinuationError::RspOutsideStack);
        }
        if self.rflags & 2 == 0 || self.rflags & !CONTINUATION_RFLAGS_MASK != 0 {
            return Err(ContinuationError::UnsupportedRflags);
        }
        Ok(())
    }
}

const _: () = {
    assert!(core::mem::size_of::<IntegerContinuation>() == 144);
    assert!(core::mem::align_of::<IntegerContinuation>() == 8);
    assert!(core::mem::offset_of!(IntegerContinuation, registers) == 0);
    assert!(core::mem::offset_of!(IntegerContinuation, rax) == 112);
    assert!(core::mem::offset_of!(IntegerContinuation, rip) == 120);
    assert!(core::mem::offset_of!(IntegerContinuation, rsp) == 128);
    assert!(core::mem::offset_of!(IntegerContinuation, rflags) == 136);
};

use crate::arch::x86_64::descriptors::SegmentState;
use crate::boot::descriptors::{CapturedSegment, ParsedFirmwareGdt};
use crate::guest::state::GuestStateRequest;
use crate::host::descriptors::HostTablePointer;
use crate::memory::address::{AddressError, AddressPolicy};
use crate::svm::vmcb::Vmcb;

/// Borrowed semantic view of the existing DXE native capture records, not a new
/// assembly ABI. `entry` describes the ACK trampoline, not the eventual firmware
/// return PC. Its EFER is the original native value, before enabling SVM.
///
/// The adapter owns actual capture, original xstate image/capability validation,
/// same-CPU residency of DR0..3 and no-ES x87 pointers, the complete native memory
/// map, debugger cooperation and asynchronous events. The parsed GDT supplies
/// CS/SS/DS/ES only under the platform's flat x64 hidden-cache correspondence
/// contract; parsing it does not independently observe those caches.
pub struct NativeContinuationRequest<'a> {
    pub entry: GuestStateRequest,
    pub registers: &'a GuestRegisters,
    pub gdt: &'a ParsedFirmwareGdt<'a>,
    pub idtr: HostTablePointer,
    /// Actual VMSAVE output on the captured CPU; unrelated VMCB bytes ignored.
    pub auxiliary: &'a Vmcb,
    pub cr2: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub cr8: u64,
    pub pat: u64,
    /// Actual native save profile: 0=FXSAVE64, 3/7=standard XSAVE64 mask.
    /// The adapter must validate the image and original XCR0/XSS separately.
    pub xstate_profile: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeContinuationError {
    Address(AddressError),
    NoncanonicalAddress,
    UnsupportedFlags,
    UnsupportedCr0,
    UnsupportedCr3,
    UnsupportedCr4,
    UnsupportedEfer,
    UnsupportedXstateProfile,
    UnsupportedDebug,
    InvalidCr8,
    InvalidPat,
    InvalidDescriptor,
    InvalidAuxiliary,
    DestinationEventState,
}

/// Validated numeric state. It is not launch authority or a portable snapshot:
/// some state remains resident on the adapter's exclusively owned CPU. Sources
/// cannot be changed through safe references while this token is alive.
pub struct PreparedNativeContinuation<'a> {
    pub(crate) request: NativeContinuationRequest<'a>,
    /// CS, SS, DS, ES in parser order, with admitted null segments expanded.
    pub(crate) segments: [SegmentState; 4],
}

/// Controls supported by the initial native continuation and its paging adapter.
/// AMD APM2 rev3.44 3.1.3/5.5.1/15.5.2: FSGSBASE changes instruction
/// availability, with FS/GS hidden state already owned by VMLOAD/VMSAVE. PCIDE
/// changes CR3's low-bit interpretation, not the four-level table format.
/// This validates captured state, not permission to enable a CPU capability.
/// SMEP/SMAP, LA57, protection keys and CET still require separate admission.
pub const fn native_cr4_supported(cr4: u64) -> bool {
    cr4 & !(0x7ff | (1 << 16) | (1 << 17) | (1 << 18)) == 0 && cr4 & 0x220 == 0x220
}

/// Prepare actual native ReadyToBoot state without changing either source or
/// destination. APM2 rev3.44 3.1,4.5,11.4/11.5,13.1,15.5.1/15.5.2, Appendix B.
/// This initial native policy admits four-level CPL0 long mode, inactive debug,
/// and the existing native 0/3/7 xstate profiles. It does not replace CR3/GDT or
/// normalize unsupported control bits. The caller proves executable code, full
/// stack/TLS/table mappings, stable hidden caches and event/CPU ownership.
pub fn prepare_native<'a>(
    request: NativeContinuationRequest<'a>,
    policy: &AddressPolicy,
) -> Result<PreparedNativeContinuation<'a>, NativeContinuationError> {
    let efer = crate::svm::dispatch::NativeEfer::admit(request.entry.efer, true)
        .map_err(|_| NativeContinuationError::UnsupportedEfer)?;
    prepare_native_with_efer(request, policy, &efer)
}

/// Prepare with the same feature-validated EFER policy used by the resident
/// runtime. The caller supplies same-CPU capability evidence to NativeEfer;
/// this token does not prove hardware capture or a complete xstate save.
/// APM2 rev3.44 3.1.7/15.5: preserve admitted controls and private SVME backing.
pub fn prepare_native_with_efer<'a>(
    request: NativeContinuationRequest<'a>,
    policy: &AddressPolicy,
    efer: &crate::svm::dispatch::NativeEfer,
) -> Result<PreparedNativeContinuation<'a>, NativeContinuationError> {
    use NativeContinuationError as E;
    let s = request.entry;
    if !is_canonical_48(s.rip) || !is_canonical_48(s.rsp) {
        return Err(E::NoncanonicalAddress);
    }
    // The bootstrap runs under the existing native IF=DF=TF=0 lease. ID is
    // retained if set; RF/VM cannot be independently captured using PUSHFQ.
    if s.rflags & 2 == 0 || s.rflags & !(CONTINUATION_RFLAGS_MASK | (1 << 21)) != 0 {
        return Err(E::UnsupportedFlags);
    }
    const CR0_ALLOWED: u64 = 0xe005_003f;
    if s.cr0 & !CR0_ALLOWED != 0
        || s.cr0 & 0x8000_0011 != 0x8000_0011
        || s.cr0 & 0xc != 0
        || (s.cr0 & (1 << 29) != 0 && s.cr0 & (1 << 30) == 0)
    {
        return Err(E::UnsupportedCr0);
    }
    if !native_cr4_supported(s.cr4) {
        return Err(E::UnsupportedCr4);
    }
    if !matches!(request.xstate_profile, 0 | 3 | 7)
        || (s.cr4 & (1 << 18) != 0) != (request.xstate_profile != 0)
    {
        return Err(E::UnsupportedXstateProfile);
    }
    // Match the captured logical value exactly; do not normalize controls or
    // accept a token for another capture. Hardware SVME is added at commit.
    if s.efer != efer.logical() || s.efer & 0x500 != 0x500 || s.efer & (1 << 12) != 0 {
        return Err(E::UnsupportedEfer);
    }
    // Preserve the entire captured CR3: a PCID when PCIDE is set, otherwise
    // PWT/PCD only. Bit63 is a MOV operand hint, never captured CR3 state; the
    // full root-page width/encryption policy below rejects it as well.
    if s.cr4 & (1 << 17) == 0 && s.cr3 & 0xfff & !0x18 != 0 {
        return Err(E::UnsupportedCr3);
    }
    policy
        .validate(s.cr3 & !0xfff, 4096, 4096)
        .map_err(E::Address)?;
    if request.dr6 >> 32 != 0
        || request.dr7 >> 32 != 0
        || request.dr7 & 0xff != 0
        || request.dr7 & ((3 << 11) | (1 << 13) | (3 << 14)) != 0
        || request.dr7 & (1 << 10) == 0
    {
        return Err(E::UnsupportedDebug);
    }
    if request.cr8 > 15 {
        return Err(E::InvalidCr8);
    }
    if request
        .pat
        .to_le_bytes()
        .iter()
        .any(|b| !matches!(*b, 0 | 1 | 4 | 5 | 6 | 7))
    {
        return Err(E::InvalidPat);
    }
    if !canonical_span(request.idtr.base, u64::from(request.idtr.limit) + 1) {
        return Err(E::NoncanonicalAddress);
    }
    let table = request.gdt.table();
    if !canonical_span(table.base, u64::from(table.limit) + 1) {
        return Err(E::NoncanonicalAddress);
    }
    let empty = SegmentState {
        selector: 0,
        attributes: 0,
        limit: 0,
        base: 0,
    };
    let mut segments = [empty; 4];
    for (out, captured) in segments.iter_mut().zip(request.gdt.segments()) {
        *out = match *captured {
            CapturedSegment::Null { selector } => SegmentState { selector, ..empty },
            CapturedSegment::Descriptor { decoded, .. } => {
                // Native UEFI's flat descriptor correspondence is required;
                // this does not admit a cached nonflat segment from stale GDT.
                if decoded.base != 0 || decoded.limit != u32::MAX {
                    return Err(E::InvalidDescriptor);
                }
                decoded
            }
        };
    }
    let bytes = request.auxiliary.bytes();
    for offset in [0x440, 0x450, 0x470, 0x490] {
        let attributes = u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap());
        let base = read_u64(bytes, offset + 8);
        if attributes & !0xfff != 0 || !is_canonical_48(base) {
            return Err(E::InvalidAuxiliary);
        }
        if offset == 0x470 {
            // An absent LDTR must be encoded as null, or present system LDT.
            if attributes != 0 && (attributes & 0x9f != 0x82) {
                return Err(E::InvalidAuxiliary);
            }
        } else if offset == 0x490 {
            // APM3 rev3.37 LTR p422 loads hidden TR from the available
            // descriptor, then marks the descriptor IN THE GDT busy. VMLOAD
            // and VMSAVE (pp499/507; APM2 15.5.1/.2) transfer all hidden TR
            // state, so preserve captured available/busy 64-bit types 9/B
            // exactly; never synthesize the memory descriptor's busy bit.
            // Type3 retains the existing dormant selector-zero native profile;
            // this is not admission of 16-bit task switching in long mode.
            if attributes & 0x10 != 0 || !matches!(attributes & 0xf, 3 | 9 | 0xb) {
                return Err(E::InvalidAuxiliary);
            }
        } else if attributes != 0
            && (attributes & 0x90 != 0x90 || (attributes & 8 != 0 && attributes & 2 == 0))
        {
            return Err(E::InvalidAuxiliary);
        }
    }
    for offset in [0x608, 0x610, 0x620, 0x630, 0x638] {
        if !is_canonical_48(read_u64(bytes, offset)) {
            return Err(E::InvalidAuxiliary);
        }
    }
    if read_u64(bytes, 0x628) >> 32 != 0 || read_u64(bytes, 0x618) >> 32 != 0 {
        return Err(E::InvalidAuxiliary);
    }
    Ok(PreparedNativeContinuation { request, segments })
}

impl PreparedNativeContinuation<'_> {
    /// Apply only to a never-entered, exclusively stopped bootstrap VMCB/frame.
    /// Refusal leaves every byte unchanged. The caller must establish the native
    /// xstate/DR0..3/system/event owners before publishing runnable state. This
    /// consumes the initial snapshot; later exits are the authoritative state.
    pub fn apply(
        self,
        vmcb: &mut Vmcb,
        frame: &mut GuestRegisters,
    ) -> Result<(), NativeContinuationError> {
        vmcb.apply_native_continuation(&self)?;
        *frame = *self.request.registers;
        Ok(())
    }
}

fn canonical_span(base: u64, len: u64) -> bool {
    let Some(last) = base.checked_add(len - 1) else {
        return false;
    };
    is_canonical_48(base) && is_canonical_48(last) && (base >> 47) == (last >> 47)
}

fn read_u64(bytes: &[u8; 4096], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// The native callback's fixed `mov eax, cookie; vmmcall` handshake.
pub const NATIVE_BOOTSTRAP_ACK: u64 = 0x5356_4d41;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapAckError {
    InvalidSites,
    InvalidInitialState,
    AlreadyAcknowledged,
    UnexpectedExit,
    StateMismatch,
    PendingEvent,
}

/// One-shot owner for the first native callback exit. This is a comparison
/// witness, never a source from which later guest state may be restored.
/// The adapter must bind the three addresses to immutable, guest-executable
/// linked code containing exactly B8 41 4D 56 53 0F 01 D9. Numeric equality
/// cannot prove code provenance, mapping or native capture.
pub struct NativeBootstrapAck {
    ack: u64,
    after_ack: u64,
    rsp: u64,
    rflags: u64,
    registers: GuestRegisters,
    acknowledged: bool,
}

impl NativeBootstrapAck {
    /// Bind a never-entered continuation after its architectural state has
    /// been committed. No guest or owner state changes on refusal.
    pub fn new(
        vmcb: &Vmcb,
        registers: &GuestRegisters,
        resume: u64,
        ack: u64,
        after_ack: u64,
    ) -> Result<Self, BootstrapAckError> {
        if !is_canonical_48(resume)
            || !is_canonical_48(after_ack)
            || resume.checked_add(5) != Some(ack)
            || ack.checked_add(3) != Some(after_ack)
        {
            return Err(BootstrapAckError::InvalidSites);
        }
        let rflags = read_u64(vmcb.bytes(), 0x570);
        if vmcb.guest_rip() != resume || rflags & (0x100 | 0x200 | 0x400) != 0 {
            return Err(BootstrapAckError::InvalidInitialState);
        }
        Ok(Self {
            ack,
            after_ack,
            rsp: vmcb.guest_rsp(),
            rflags,
            registers: *registers,
            acknowledged: false,
        })
    }

    pub const fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    /// Complete only the first stopped VMMCALL at the linked ACK address.
    /// AMD APM2 rev3.44 15.7.1 and Appendix C: instruction completion advances
    /// RIP; a refused or unrelated exit changes nothing. nRIP is not consumed:
    /// the adapter's immutable linked instruction is the byte authority.
    /// No registers or flags change (the guest epilogue restores originals).
    pub fn acknowledge(
        &mut self,
        vmcb: &mut Vmcb,
        registers: &GuestRegisters,
    ) -> Result<(), BootstrapAckError> {
        use BootstrapAckError as E;
        if self.acknowledged {
            return Err(E::AlreadyAcknowledged);
        }
        let exit = vmcb.exit_snapshot();
        if exit.code != 0x81 || exit.rip != self.ack {
            return Err(E::UnexpectedExit);
        }
        if vmcb.guest_rax() != NATIVE_BOOTSTRAP_ACK
            || vmcb.guest_rsp() != self.rsp
            || read_u64(vmcb.bytes(), 0x570) != self.rflags
            || registers != &self.registers
        {
            return Err(E::StateMismatch);
        }
        if vmcb.event_injection() != 0
            || read_u64(vmcb.bytes(), 0x088) != 0
            || read_u64(vmcb.bytes(), 0x068) != 0
            || vmcb.virtual_interrupt_control() & !(0xf | (1 << 24)) != 0
        {
            return Err(E::PendingEvent);
        }
        let next = exit
            .resume_candidate_from_instruction(&[0x0f, 0x01, 0xd9])
            .map_err(|_| E::UnexpectedExit)?;
        if next.address() != self.after_ack {
            return Err(E::UnexpectedExit);
        }
        vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
        self.acknowledged = true;
        Ok(())
    }
}
