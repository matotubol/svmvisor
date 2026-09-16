//! One-shot terminal evidence. PPR57896 rev3.00 pp40-41,210; APM2 rev3.44
//! 15.17/Table15-10,15.21.8,15.28. No resumable dispatcher performs transport.
use core::sync::atomic::{AtomicU32, Ordering};

pub const PCI_VENDOR_DEVICE: u32 = 0x0666_10ee;
pub const PCI_CLASS_REVISION: u32 = 0xff00_0003;
pub const CONTROL_OFFSET: u64 = 0x800;

/// # Safety
/// PPR57896 rev3.00 2.1.6.1 pp40-41: `page` is an admitted UC-mapped
/// configuration function page, still routed by the validated C0010058 MSR.
/// Caller owns access lifetime; offset is a naturally aligned DWORD <=4092.
pub unsafe fn read_config_dword(page: u64, offset: u16) -> u32 {
    debug_assert!(offset <= 4092 && offset & 3 == 0);
    let value: u32;
    unsafe { core::arch::asm!("mov eax, dword ptr [{address}]", address=in(reg) page+u64::from(offset),
        out("eax") value, options(nostack, preserves_flags)); }
    value
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalEndpoint {
    pub config_page: u64,
    pub bar0_host_page: u64,
    pub fpga_build_id: u64,
    pub rom_build_id: u64,
    pub mmio_config_msr: u64,
    pub bar0_raw: u32,
    pub segment_bdf: u32,
    pub boot_id: u32,
    pub command: u16,
    pub version: u8,
    pub reserved: u8,
}
impl TerminalEndpoint {
    /// Complete admitted MMCONFIG aperture, including upstream bridge config.
    /// PPR57896 2.1.6.1: BusRange field gives log2(number of buses), each1MiB.
    pub fn config_aperture(&self) -> Option<(u64,u64)> {
        if !self.valid() { return None; }
        Some((self.mmio_config_msr & 0x0000_ffff_fff0_0000,
            (1u64 << 20) << ((self.mmio_config_msr >> 2) & 15)))
    }
    /// PPR57896 p210 -> pp40-41. Initial target supports segment0, <=256 buses.
    pub fn config_page_from_msr(msr: u64, segment_bdf: u32) -> Option<u64> {
        let buses = ((msr >> 2) & 15) as u32;
        if msr & !0x0000_ffff_fff0_003d != 0 || msr & 1 == 0 || buses > 8
            || segment_bdf > 0xffff || (segment_bdf >> 8) >= (1u32 << buses)
        { return None; }
        let base = msr & 0x0000_ffff_fff0_0000;
        if base & ((1u64 << (20+buses))-1) != 0 { return None; }
        let page = base.checked_add(u64::from(segment_bdf) << 12)?;
        (base >= 0x100000 && page <= 0xffff_f000).then_some(page)
    }
    pub fn valid(&self) -> bool {
        self.version == 1 && self.reserved == 0
            && Self::config_page_from_msr(self.mmio_config_msr, self.segment_bdf) == Some(self.config_page)
            && self.bar0_host_page >= 0x100000 && self.bar0_host_page <= 0xffff_f000
            && self.bar0_host_page & 4095 == 0
            && u64::from(self.bar0_raw) == self.bar0_host_page
            && self.config_page != self.bar0_host_page && self.command & 2 != 0
            && self.fpga_build_id != 0 && self.rom_build_id != 0
    }
}

/// Shared backing follows all 32 startup mailboxes. Only atomics are accessed
/// after entry; one reporter owns transport and every participant stays stopped.
#[repr(C, align(64))]
pub struct TerminalControl {
    ready: AtomicU32,
    expected: AtomicU32,
    owner: AtomicU32,
    request: AtomicU32,
    acknowledged: AtomicU32,
    outcome: AtomicU32,
    initial_acknowledged: AtomicU32,
    diagnostic_gate: AtomicU32,
    diagnostic_revoked: AtomicU32,
    reserved: [u32; 55],
}
impl TerminalControl {
    pub const fn new() -> Self {
        Self { ready: AtomicU32::new(0), expected: AtomicU32::new(0),
            owner: AtomicU32::new(0), request: AtomicU32::new(0),
            acknowledged: AtomicU32::new(0), outcome: AtomicU32::new(0),
            initial_acknowledged: AtomicU32::new(0), diagnostic_gate: AtomicU32::new(0),
            diagnostic_revoked: AtomicU32::new(0), reserved: [0;55] }
    }
    pub fn publish_ready(&self, count: usize) -> bool {
        let Some(mask) = cpu_mask(count) else { return false; };
        if self.ready.load(Ordering::Acquire) != 0 || self.owner.load(Ordering::Acquire) != 0 { return false; }
        self.expected.store(mask, Ordering::Relaxed);
        self.ready.compare_exchange(0,1,Ordering::Release,Ordering::Relaxed).is_ok()
    }
    /// Monotonic initial-entry proof; guest INIT/SIPI never clears these bits.
    pub fn initial_ack(&self, slot: usize, count: usize) -> bool {
        let Some(mask) = cpu_mask(count) else { return false; };
        if slot >= count { return false; }
        let seen = self.initial_acknowledged.fetch_or(1<<slot,Ordering::AcqRel) | (1<<slot);
        if seen == mask { self.publish_ready(count) } else { false }
    }
    pub fn ready(&self, count: usize) -> bool {
        self.ready.load(Ordering::Acquire) == 1
            && cpu_mask(count) == Some(self.expected.load(Ordering::Relaxed))
    }
    pub fn requested(&self) -> bool { self.request.load(Ordering::Acquire) == 1 }
    pub fn claim(&self, slot: usize, count: usize) -> bool {
        if slot >= count || !self.ready(count) { return false; }
        if self.owner.compare_exchange(0, slot as u32 + 1, Ordering::AcqRel, Ordering::Acquire).is_err() { return false; }
        self.request.store(1, Ordering::Release);
        true
    }
    pub fn acknowledge(&self, slot: usize, count: usize) -> bool {
        if slot >= count || !self.ready(count) || !self.requested() { return false; }
        self.acknowledged.fetch_or(1 << slot, Ordering::Release);
        true
    }
    pub fn all_acknowledged(&self, count: usize) -> bool {
        self.ready(count) && self.requested()
            && cpu_mask(count) == Some(self.acknowledged.load(Ordering::Acquire))
    }
    pub fn finish(&self, result: u32) { self.outcome.store(result, Ordering::Release); }
    pub fn diagnostic_lock(&self) -> Option<DiagnosticGuard<'_>> {
        self.diagnostic_gate.compare_exchange(0,1,Ordering::Acquire,Ordering::Relaxed)
            .ok().map(|_| DiagnosticGuard(self))
    }
    pub fn diagnostic_revoked(&self) -> bool {
        self.diagnostic_revoked.load(Ordering::Acquire) != 0
    }
    pub fn diagnostic_revoke(&self) { self.diagnostic_revoked.store(1,Ordering::Release); }
    pub fn diagnostic_snapshot(&self) -> [u64;6] {
        [self.expected.load(Ordering::Acquire) as u64,
         self.acknowledged.load(Ordering::Acquire) as u64,
         self.owner.load(Ordering::Acquire) as u64,
         self.outcome.load(Ordering::Acquire) as u64,
         self.initial_acknowledged.load(Ordering::Acquire) as u64, 0]
    }
}
/// Nonblocking lifetime exclusion shared by publication and native config I/O.
pub struct DiagnosticGuard<'a>(&'a TerminalControl);
impl Drop for DiagnosticGuard<'_> {
    fn drop(&mut self) { self.0.diagnostic_gate.store(0,Ordering::Release); }
}
impl Default for TerminalControl { fn default() -> Self { Self::new() } }
pub fn cpu_mask(count: usize) -> Option<u32> {
    if count == 32 { Some(u32::MAX) } else if (1..32).contains(&count) { Some((1u32 << count)-1) } else { None }
}

/// Fixed journal operation; adapters expose only admitted device DWORDs.
pub trait JournalIo {
    type Error;
    fn read(&mut self, offset: u64) -> Result<u32, Self::Error>;
    fn write(&mut self, offset: u64, value: u32) -> Result<(), Self::Error>;
}

/// Per-runtime first-fault latch. No global guard or allocation is needed;
/// a fault interrupting the writer never spins on that interrupted writer.
/// Publication failure leaves the immutable payload available for later retry.
pub struct DeferredFault {
    state: AtomicU32,
    words: [AtomicU32; 19],
    failures: AtomicU32,
}
impl DeferredFault {
    pub const fn new() -> Self {
        Self { state: AtomicU32::new(0), words: [const { AtomicU32::new(0) };19],
            failures: AtomicU32::new(0) }
    }
    pub fn capture(&self, words: [u32;19]) {
        if self.state.compare_exchange(0,1,Ordering::Acquire,Ordering::Relaxed).is_err() { return; }
        for (to, from) in self.words.iter().zip(words) { to.store(from,Ordering::Relaxed); }
        self.state.store(2,Ordering::Release);
    }
    pub fn pending(&self) -> Option<[u32;19]> {
        if self.state.load(Ordering::Acquire) != 2 { return None; }
        Some(core::array::from_fn(|i| self.words[i].load(Ordering::Relaxed)))
    }
    pub fn published(&self) { self.state.store(3,Ordering::Release); }
    pub fn failed(&self) { self.failures.fetch_add(1,Ordering::Relaxed); }
    pub fn status(&self) -> u64 {
        self.state.load(Ordering::Acquire) as u64 | ((self.failures.load(Ordering::Relaxed) as u64)<<32)
    }
}

/// Existing USER3 record wire format, shared by resident and BSP preparation.
pub fn diagnostic_payload(sequence:u32,event:u8,fault:bool,boot:u32,apic:u32,tsc:u64,
    context:[u64;6],aux:u32)->[u32;19] {
    let mut payload=[0;19];
    payload[0]=sequence; payload[1]=event as u32|0x100|(u32::from(fault)<<16);
    payload[2]=boot;payload[3]=apic;payload[4]=tsc as u32;payload[5]=(tsc>>32) as u32;
    for (i,value) in context.into_iter().enumerate(){payload[6+i*2]=value as u32;payload[7+i*2]=(value>>32)as u32;}
    payload[18]=aux;payload
}
/// Caller owns the validated endpoint/lifetime and serializes this bank.
/// No partial payload is published: the final sequence write is the commit.
pub fn commit_diagnostic<I:JournalIo>(io:&mut I,slot:usize,payload:[u32;19])->Result<(),CommitError<I::Error>> {
    if slot>=32 || payload[0]==0 {return Err(CommitError::Sequence);}
    let window=0x600+slot as u64*0x50;
    for(i,value)in payload.into_iter().enumerate(){io.write(window+i as u64*4,value).map_err(CommitError::Access)?;}
    core::sync::atomic::fence(Ordering::SeqCst);
    io.write(window+76,payload[0]).map_err(CommitError::Access)?;
    io.read(0).map_err(CommitError::Access)?;Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitError<E> { Access(E), Sequence, Echo, Timeout }
/// Stage exactly eight DWORDs and verify a distinct sequence and complete echo.
/// Only a serialized owner with a proven device lifetime may invoke this.
pub fn commit_record<I: JournalIo>(io: &mut I, record: [u32;8]) -> Result<(), CommitError<I::Error>> {
    if io.read(0x2c).map_err(CommitError::Access)? == record[0] { return Err(CommitError::Sequence); }
    for (i,value) in record.iter().enumerate() { io.write(0x40+i as u64*4,*value).map_err(CommitError::Access)?; }
    core::sync::atomic::fence(Ordering::SeqCst);
    io.write(0x60,record[0]).map_err(CommitError::Access)?;
    for _ in 0..1024 {
        if io.read(0x2c).map_err(CommitError::Access)? == record[0] {
            for (i,value) in record.iter().enumerate() {
                if io.read(0x80+i as u64*4).map_err(CommitError::Access)? != *value { return Err(CommitError::Echo); }
            }
            return if io.read(0x24).map_err(CommitError::Access)? == 0 { Ok(()) } else { Err(CommitError::Echo) };
        }
    }
    Err(CommitError::Timeout)
}

/// Stable software diagnostic codes; no additional guest reads are performed.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FetchReadFailure {
    MemoryMap = 0x30, AddressPolicy = 0x31, MonitorRange = 0x32,
    HostPat = 0x33, MtrrCapture = 0x34, ReadShape = 0x35,
    RamAdmission = 0x36, FixedMtrrControl = 0x37, FixedMtrrRange = 0x38,
    PhysicalMemoryNotWb = 0x39, ScratchAliasOccupied = 0x3a, MemoryControlBusy = 0x3b,
}

pub fn fetch_failure_code(error: super::fetch::FetchError) -> u16 {
    use super::fetch::FetchError as F;
    use crate::host::paging::WalkError as W;
    match error {
        F::UnsupportedExit => 1, F::UnsupportedMode => 2,
        F::AddressOverflow => 3, F::UnsupportedCacheControl => 4,
        F::NotExecutable => 5, F::PrivilegeMismatch => 6,
        F::NonWriteBackInstruction => 7, F::UnreadableInstruction { .. } => 8,
        F::SegmentLimit => 9,
        F::Walk(w) => match w {
            W::UnsupportedPhysicalWidth => 0x10, W::FiveLevelUnsupported => 0x11,
            W::NoncanonicalAddress => 0x12, W::InvalidCr3 => 0x13,
            W::UnreadableTable { level, .. } => 0x14 | ((level as u16) << 8),
            W::NotPresent { level } => 0x15 | ((level as u16) << 8),
            W::ReservedEntry { level } => 0x16 | ((level as u16) << 8),
            W::UnsupportedEntryBits { level } => 0x17 | ((level as u16) << 8),
            W::OneGiBUnsupported => 0x18, W::IncompleteWalk => 0x19,
        },
    }
}

/// kind10 preserves canonical48 RIP and exact software predicate in 64 bits.
/// Noncanonical RIP falls back to the original full-width RIP-only record.
fn detailed_fetch(rip: u64, failure: u64) -> Option<u64> {
    let low = rip & 0xffff_ffff_ffff;
    let canonical = ((low << 16) as i64 >> 16) as u64;
    let base = failure & 255;
    let level = failure >> 8;
    let valid = if (0x14..=0x17).contains(&base) {
        (1..=4).contains(&level)
    } else {
        level == 0 && ((1..=9).contains(&base) || (0x10..=0x13).contains(&base)
            || (0x18..=0x19).contains(&base) || (0x30..=0x3a).contains(&base))
    };
    (canonical == rip && valid).then_some(low | (failure << 48))
}

/// Stable EFER diagnostic, derived only from the already stopped state.
/// The payload is one full-width operand; it does not export the whole VMCB.
pub fn efer_failure(error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::vmcb::Vmcb, logical: u64, instruction_bytes: [u8; 2]) -> (u64, u64)
{
    msr_failure_context(error, vmcb, logical, Some(instruction_bytes), 0xf108)
}

/// Hardware NRIP diagnostics contain actual continuation evidence, never invented bytes.
pub fn efer_nrip_failure(error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::vmcb::Vmcb, logical: u64) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, None, 0xf108)
}

/// VM_CR uses the same stopped-instruction errors but a distinct register identity.
pub fn vmcr_failure(error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::vmcb::Vmcb, logical: u64, instruction_bytes: [u8; 2]) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, Some(instruction_bytes), 0xf109)
}

pub fn vmcr_nrip_failure(error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::vmcb::Vmcb, logical: u64) -> (u64, u64) {
    msr_failure_context(error, vmcb, logical, None, 0xf109)
}

fn instruction(e: crate::svm::exit::ResumeError) -> u64 { use crate::svm::exit::ResumeError as R; match e {
        R::ExitDoesNotPermitCandidate => 0, R::NripNotEstablished => 1,
        R::NonCanonicalRip => 2, R::NonCanonicalNrip => 3,
        R::InvalidInstructionLength => 4, R::UnsupportedInstructionBytes => 5,
    } }
fn pending(e: crate::svm::events::ExternalInterruptError) -> (u64,u64) { use crate::svm::events::ExternalInterruptError as P; match e {
        P::ReservedVector { vector } => (0,vector.into()),
        P::InvalidTaskPriority { priority } => (1,priority.into()),
        P::RequestNotQueued => (2,0), P::RequestNotArmed => (3,0),
        P::RequestAlreadyConsumed => (4,0), P::PendingInjection => (5,0),
        P::NestedDeliveryUnsupported => (6,0), P::PendingVirtualInterrupt => (7,0),
        P::UnsupportedControl { control } => (8,control),
        P::UnsupportedNestedControl { control } => (9,control),
        P::ControlMismatch => (10,0), P::InvalidEntry => (11,0),
        P::GuestShutdown => (12,0), P::InconsistentVirtualInterruptExit => (13,0),
    } }

fn msr_failure_context(error: crate::svm::dispatch::NativeEferError,
    vmcb: &crate::vmcb::Vmcb, logical: u64, instruction_bytes: Option<[u8; 2]>, tag: u64) -> (u64, u64)
{
    use crate::svm::{dispatch::NativeEferError as E, events::MsrFaultError as M, exit::ResumeError as R};
    let info = vmcb.exit_snapshot().info1;
    let instruction_value = |e| match e {
        R::UnsupportedInstructionBytes => instruction_bytes.map(u16::from_le_bytes).map(u64::from).unwrap_or(0),
        R::NonCanonicalRip => vmcb.exit_snapshot().rip,
        // EFER checked_instruction already proved addition does not overflow;
        // this error describes RIP+2, never the unrelated hardware NRIP field.
        R::NonCanonicalNrip => if instruction_bytes.is_some() { vmcb.exit_snapshot().rip.wrapping_add(2) } else { vmcb.exit_snapshot().nrip },
        R::NripNotEstablished => vmcb.exit_snapshot().nrip,
        R::InvalidInstructionLength => if instruction_bytes.is_some() { 2 } else { vmcb.exit_snapshot().nrip },
        R::ExitDoesNotPermitCandidate => info,
    };
    let (reason,value) = match error {
        E::UnsupportedInitialState => (1,logical),
        E::BackingMismatch => (2,u64::from_le_bytes(vmcb.bytes()[0x4d0..0x4d8].try_into().unwrap())),
        E::UnsupportedMode => (3,u64::from_le_bytes(vmcb.bytes()[0x558..0x560].try_into().unwrap())),
        E::UnsupportedDebugState => (4,vmcb.guest_rflags()),
        E::UnsupportedMsr { index, .. } => (5,index.into()),
        E::UnsupportedValue { value } => (6,value),
        E::Instruction(e) => ((if instruction_bytes.is_some() { 0x10 } else { 0x50 })+instruction(e),instruction_value(e)),
        E::PendingState(e) => { let (r,v)=pending(e); (0x20+r,v) },
        E::Fault(M::Instruction(e)) => ((if instruction_bytes.is_some() { 0x30 } else { 0x60 })+instruction(e),instruction_value(e)),
        E::Fault(M::State(e)) => { let (r,v)=pending(e); (0x40+r,v) },
    };
    let direction = match error {
        E::UnsupportedMsr { write, .. } => u64::from(write),
        _ => match info { 0 => 0, 1 => 1, _ => 2 },
    };
    (tag | ((reason | (direction << 9)) << 16),value)
}

/// SYSCFG stage85 carries both DWORD operands when possible, otherwise one
/// explicitly typed full-width value. No diagnostic path reads guest memory.
pub fn syscfg_failure(error: crate::svm::native_syscfg::SyscfgError,
    vmcb: &crate::vmcb::Vmcb, instruction: Option<[u8; 2]>) -> (u64, u64) {
    use crate::svm::native_syscfg::SyscfgError as E;
    let write = vmcb.exit_snapshot().info1 == 1;
    let (reason, requested, current) = match error {
        E::Boundary(error) => {
            let (tag, value) = msr_failure_context(error, vmcb, 0, instruction, 0);
            return syscfg_context((tag >> 16) & 255, 3, write, value);
        }
        E::UnsupportedProfile { signature, physical_bits } =>
            return syscfg_context(0x80, 3, write, u64::from(signature) | (u64::from(physical_bits) << 32)),
        E::CurrentReserved { requested, current } => (0x81, requested, current),
        E::CurrentEncryption { requested, current } => (0x82, requested, current),
        E::RequestedReserved { requested, current } => (0x83, requested, current),
        E::UnsupportedChange { requested, current } => (0x84, requested, current),
    };
    syscfg_operands(reason, write, requested, current)
}

pub fn syscfg_operands(reason: u64, write: bool, requested: u64, current: u64) -> (u64, u64) {
    if current > u32::MAX as u64 { syscfg_context(reason, 2, write, current) }
    else if requested > u32::MAX as u64 { syscfg_context(reason, 1, write, requested) }
    else { syscfg_context(reason, 0, write, requested | (current << 32)) }
}
fn syscfg_context(reason: u64, mode: u64, write: bool, value: u64) -> (u64, u64) {
    (0xf10d | ((reason | (mode << 8) | (u64::from(write) << 10)) << 16), value)
}
fn valid_syscfg_reason(reason: u64) -> bool {
    (1..=6).contains(&reason) || (0x10..=0x15).contains(&reason)
        || (0x20..=0x2d).contains(&reason) || (0x30..=0x35).contains(&reason)
        || (0x40..=0x4d).contains(&reason) || (0x50..=0x55).contains(&reason)
        || (0x60..=0x65).contains(&reason) || (0x80..=0x85).contains(&reason)
}

/// Compact platform IDs, with an explicit lossless ICR fallback for wider IDs.
/// Recipient evidence was captured at the rejecting check under the route lock.
/// Only the x2APIC form exists; bit4 stays set so decoders keep its meaning.
pub fn route_failure(f: crate::svm::ipi::NativeRouteFailure) -> (u64, u64) {
    let mut code = f.predicate as u64 | 1 << 4;
    let dest = f.value >> 32;
    let wide = dest > 255 || f.source > 255 || f.recipient.is_some_and(|r| r.identity > 255);
    let value = if wide { code |= 1 << 5; f.value } else {
        let mut v = f.value as u32 as u64 | (dest << 32) | ((f.source as u64) << 40);
        if let Some(r) = f.recipient {
            code |= (r.init_count.min(31) as u64) << 6;
            v |= (r.identity as u64) << 48 | (r.mode.map_or(0, |m| m as u64) << 56)
                | ((r.cause as u64) << 59);
        }
        v
    };
    (0xf10b | (code << 16), value)
}

/// Target-local startup evidence. Wide observations retain all 64 bits and
/// explicitly omit CPU identity. No diagnostic performs another hardware read.
pub fn startup_failure(reason: u8, detail: u8, value: u64, identity: u32) -> (u64, u64) {
    let wide = value > u32::MAX as u64;
    let code = reason as u64 | ((detail as u64) << 3) | (u64::from(wide) << 10);
    (0xf10c | (code << 16), if wide { value } else { value | ((identity as u64) << 32) })
}

pub fn startup_pending_failure(error: crate::svm::events::ExternalInterruptError, identity: u32) -> (u64, u64) {
    let (reason, value) = pending(error);
    startup_failure(6, reason as u8, value, identity)
}

/// Select one explicitly typed full-width context; full stop state remains local.
/// Kinds 8 and 13 belonged to retired xAPIC MMIO stops; they are never exported
/// now, and only the offline snapshot decoder still reads them from old images.
pub fn stop_words(slot: usize, code: u64, rip: u64, info1: u64, info2: u64) -> Option<[u32;3]> {
    if slot >= 32 { return None; }
    if code == 0x7c && info1 & 0xffff == 0xf10d {
        let d = info1 >> 16;
        let reason = d & 255;
        let mode = (d >> 8) & 3;
        if d > 0x7ff || !valid_syscfg_reason(reason)
            || (mode == 3 && reason > 0x80)
            || (mode != 3 && (!(0x81..=0x85).contains(&reason) || d & 0x400 == 0)) { return None; }
        return Some([0x1000_0085 | ((slot as u32) << 8) | ((d as u32) << 13),
            info2 as u32, (info2 >> 32) as u32]);
    }
    let (exit, kind, value) = if info1 & 0xffff == 0xf10b {
        let d = info1 >> 16;
        if d > 0x7ff || !(1..=14).contains(&(d & 15)) || d & 16 == 0 || code != 0x7c {
            return None;
        }
        (d as u32, 14, info2)
    } else if info1 & 0xffff == 0xf10c {
        let d = info1 >> 16;
        if d > 0x7ff || d & 7 == 0 { return None; }
        (d as u32, 15, info2)
    } else if code == 0x7c && matches!(info1 & 0xffff, 0xf108 | 0xf109) {
        let diagnostic = info1 >> 16;
        let reason = diagnostic & 0x1ff;
        let vmcr = info1 & 0xffff == 0xf109;
        // The fixed VM_CR profile has no initial-state or backing-mismatch error.
        if vmcr && matches!(reason, 1 | 2) { return None; }
        if diagnostic >> 9 > 2 || !((1..=6).contains(&reason)
            || (0x10..=0x15).contains(&reason) || (0x20..=0x2d).contains(&reason)
            || (0x30..=0x35).contains(&reason) || (0x40..=0x4d).contains(&reason)
            || (0x50..=0x54).contains(&reason) || (0x60..=0x64).contains(&reason)) { return None; }
        (diagnostic as u32,if vmcr { 12 } else { 11 },info2)
    } else if code > 0x7ff { (0x7ff, 4, code) } else {
        let (kind,value) = match info1 {
            0xf001 => detailed_fetch(rip, info2).map_or((1,rip), |v| (10,v)), 0xf102 => (5,rip), 0xf103 => (6,rip),
            0xf104 => (2,info2), 0xf105 => (7,rip), 0xf107 => (9,info2),
            // A runtime context mismatch carries a host pointer, never a GPA.
            0xf10a => (0,rip), _ if code == 0x400 => (3,info2), _ => (0,rip),
        }; (code as u32,kind,value)
    };
    Some([0x1000_0083 | ((slot as u32)<<8) | (exit<<13) | (kind<<24), value as u32, (value>>32) as u32])
}
const _: () = {
    assert!(core::mem::size_of::<TerminalEndpoint>() == 56);
    assert!(core::mem::offset_of!(TerminalEndpoint, mmio_config_msr) == 32);
    assert!(core::mem::offset_of!(TerminalEndpoint, bar0_raw) == 40);
    assert!(core::mem::offset_of!(TerminalEndpoint, version) == 54);
    assert!(core::mem::size_of::<TerminalControl>() == 256);
    assert!(CONTROL_OFFSET == 32 * core::mem::size_of::<crate::svm::ipi::NativeStartupMailbox>() as u64);
    assert!(CONTROL_OFFSET + core::mem::size_of::<TerminalControl>() as u64 <= 4096);
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn retired_xapic_kinds_are_never_exported() {
        // Former kind8 is an ordinary NPF GPA observation.
        let npf=stop_words(0,0x400,0x1234,0xf106,0xfee00030).unwrap();
        assert_eq!((npf[0]>>24)&15,3); assert_eq!(npf[1],0xfee00030);
        // A context mismatch keeps its RIP on every exit, including NPF.
        for code in [0x400,0x7c] {
            let words=stop_words(0,code,0x5678,0xf10a,0xdead_beef).unwrap();
            assert_eq!((words[0]>>24)&15,0); assert_eq!(words[1],0x5678);
        }
        // Former kind13 detail tags no longer select a typed context.
        assert_eq!((stop_words(0,0x400,0,0xf10a|(1<<16),9).unwrap()[0]>>24)&15,3);
    }
    #[test] fn vmcr_diagnostics_preserve_distinct_identity_and_full_operand() {
        use crate::svm::{dispatch::NativeEferError as E, exit::ResumeError as R};
        let vmcb = crate::vmcb::Vmcb::new();
        let before = *vmcb.bytes();
        let (tag, value) = vmcr_failure(E::UnsupportedValue { value: u64::MAX }, &vmcb, 0x10, [15,48]);
        let words = stop_words(0, 0x7c, 0, tag, value).unwrap();
        assert_eq!((words[0] >> 24) & 15, 12);
        assert_eq!((words[0] >> 13) & 0x7ff, 6);
        assert_eq!([words[1], words[2]], [u32::MAX; 2]);
        let (tag, value) = vmcr_nrip_failure(E::Instruction(R::InvalidInstructionLength), &vmcb, 0x10);
        let words = stop_words(0, 0x7c, 0, tag, value).unwrap();
        assert_eq!((words[0] >> 13) & 0x7ff, 0x54);
        for reason in [1u64, 2, 0x55, 0x65, 0x600] {
            assert!(stop_words(0, 0x7c, 0, 0xf109 | (reason << 16), 0).is_none());
        }
        assert_eq!(vmcb.bytes(), &before);
    }
    #[test] fn efer_error_payload_is_full_width_and_instruction_is_explicit() {
        use crate::svm::{dispatch::NativeEferError as E, exit::ResumeError as R, events::ExternalInterruptError as P};
        let vmcb = crate::vmcb::Vmcb::new();
        let before = *vmcb.bytes();
        for (error, reason, value) in [
            (E::UnsupportedValue { value: 0xfedc_ba98_7654_3210 },6,0xfedc_ba98_7654_3210),
            (E::Instruction(R::UnsupportedInstructionBytes),0x15,0x320f),
            (E::PendingState(P::UnsupportedControl { control: u64::MAX }),0x28,u64::MAX),
            (E::PendingState(P::GuestShutdown),0x2c,0),
        ] {
            let (tag,payload)=efer_failure(error,&vmcb,0xd01,[0x0f,0x32]);
            assert_eq!(payload,value);
            let words=stop_words(23,0x7c,0xffff_ffff_ffff_ffff,tag,payload).unwrap();
            assert_eq!((words[0]>>24)&15,11);
            assert_eq!((words[0]>>13)&0x7ff,reason);
            assert_eq!(words[1] as u64 | ((words[2] as u64)<<32),value);
        }
        assert_eq!(vmcb.bytes(),&before);
        let mut boundary = crate::vmcb::Vmcb::new();
        // Deliberately stale hardware NRIP must not replace proposed RIP+2.
        unsafe {
            let bytes = (&mut boundary as *mut crate::vmcb::Vmcb).cast::<u8>();
            core::ptr::copy_nonoverlapping(0x7fff_ffff_ffffu64.to_le_bytes().as_ptr(), bytes.add(0x578), 8);
            core::ptr::copy_nonoverlapping(0x1234u64.to_le_bytes().as_ptr(), bytes.add(0xc8), 8);
        }
        assert_eq!(efer_failure(E::Instruction(R::NonCanonicalNrip), &boundary, 0x500, [15,50]).1,
            0x8000_0000_0001);
        for diagnostic in [0,7,0x16,0x2e,0x36,0x4e,0x600,0x10000] {
            assert!(stop_words(0,0x7c,0,0xf108 | (diagnostic<<16),0).is_none());
        }
    }
    #[test] fn detailed_fetch_retains_rip_and_qualified_failure_only() {
        use super::super::fetch::FetchError;
        use crate::host::paging::WalkError;
        assert_eq!(fetch_failure_code(FetchError::Walk(WalkError::UnreadableTable { level: 3, address: 123 })), 0x314);
        for rip in [0, 0x7fff_ffff_ffff, 0xffff_8000_0000_0000, 0xffff_f800_b363_797a] {
            let words = stop_words(23, 0x7c, rip, 0xf001, 0x314).unwrap();
            assert_eq!((words[0] >> 24) & 15, 10);
            let context = words[1] as u64 | ((words[2] as u64) << 32);
            assert_eq!(context >> 48, 0x314);
            assert_eq!(((context << 16) as i64 >> 16) as u64, rip);
        }
        for failure in [0, 0x14, 0x514, 0x130, 0x3b, u64::MAX] {
            let words = stop_words(0, 0x7c, 0x1234, 0xf001, failure).unwrap();
            assert_eq!((words[0] >> 24) & 15, 1);
            assert_eq!(words[1], 0x1234);
        }
        let words = stop_words(0, 0x72, 0x8000_0000_0000, 0xf001, 4).unwrap();
        assert_eq!((words[0] >> 24) & 15, 1);
        assert_eq!(words[2], 0x8000);
    }

    #[test]
    fn deferred_fault_survives_contention_missing_cpu_and_reentrant_capture() {
        let control=TerminalControl::new();
        assert!(control.publish_ready(2));
        assert!(control.claim(0,2));
        assert!(control.acknowledge(0,2));
        let latch=DeferredFault::new();
        let first=diagnostic_payload(17,3,true,1,0,2,[1,2,3,4,5,6],7);
        let guard=control.diagnostic_lock().unwrap();
        latch.capture(first);
        assert!(control.diagnostic_lock().is_none()); latch.failed();
        latch.capture([99;19]);
        assert_eq!(latch.pending(),Some(first));
        assert_eq!(latch.status(),2|(1<<32));
        assert!(!control.all_acknowledged(2));
        drop(guard);
        let _guard=control.diagnostic_lock().unwrap();
        // Publication is permitted by the lifetime guard despite missing peer.
        assert_eq!(latch.pending(),Some(first)); latch.published();
        assert_eq!(latch.pending(),None);
        latch.capture([88;19]); assert_eq!(latch.status(),3|(1<<32));
        let interrupted=DeferredFault::new();
        interrupted.state.store(1,Ordering::Relaxed);
        interrupted.capture(first); // Returns immediately, never waits on itself.
        assert_eq!(interrupted.pending(),None);
    }
    #[test] fn terminal_requires_all_cpus_and_never_reopens() {
        let c=TerminalControl::new(); assert!(!c.claim(0,2)); assert!(c.publish_ready(2));
        assert!(c.claim(0,2)); assert!(!c.claim(1,2)); assert!(c.acknowledge(0,2));
        assert!(!c.all_acknowledged(2)); assert!(c.acknowledge(1,2));
        assert!(c.all_acknowledged(2)); c.finish(2); assert!(c.requested()); assert!(!c.publish_ready(2));
    }
    #[test] fn fixed_record_preserves_wide_values_and_synthetic_context() {
        assert_eq!(stop_words(31,u64::MAX,5,0,0),Some([0x14ff_ff83,u32::MAX,u32::MAX]));
        let words=stop_words(2,0x7c,0x1234,0xf104,0xc0010114).unwrap();
        assert_eq!((words[0]>>24)&15,2); assert_eq!(words[1],0xc0010114);
        assert!(stop_words(32,0,0,0,0).is_none());
    }
    #[test] fn configuration_route_is_enabled_bounded_segment_zero() {
        assert_eq!(TerminalEndpoint::config_page_from_msr(0xe0000021,0x12a),Some(0xe012a000));
        for (m,b) in [(0xe0000020,0),(0xe0000023,0),(0xe0100021,0),(0xe0000001,0x100),(0xe0000021,0x10000),(0x1e0000021,0)] {
            assert!(TerminalEndpoint::config_page_from_msr(m,b).is_none());
        }
    }
    #[test] fn initial_ack_gate_cannot_open_with_missing_cpu_or_lose_proof_on_restart() {
        let c=TerminalControl::new();
        for slot in 0..31 { assert!(!c.initial_ack(slot,32)); }
        assert!(!c.ready(32)); assert!(!c.claim(0,32));
        assert!(c.initial_ack(31,32)); assert!(c.ready(32));
        assert!(!c.initial_ack(2,32)); assert!(c.ready(32));
        assert!(c.claim(31,32));
        for slot in 0..31 { assert!(c.acknowledge(slot,32)); }
        assert!(!c.all_acknowledged(32)); assert!(c.acknowledge(31,32));
        assert!(c.all_acknowledged(32));
    }
    struct MockJournal { staged:[u32;8], committed:[u32;8], writes:usize, polls:usize, drop_commit:bool, corrupt:bool }
    impl JournalIo for MockJournal {
        type Error=();
        fn read(&mut self,offset:u64)->Result<u32,()> {
            if offset==0x2c {self.polls+=1;return Ok(self.committed[0]);}
            if offset==0x24 {return Ok(0);}
            if (0x80..=0x9c).contains(&offset) {return Ok(self.committed[((offset-0x80)/4) as usize]);}
            Err(())
        }
        fn write(&mut self,offset:u64,value:u32)->Result<(),()> {
            self.writes+=1;
            if (0x40..0x60).contains(&offset) {self.staged[((offset-0x40)/4) as usize]=value;return Ok(());}
            if offset==0x60 && value==self.staged[0] {
                if !self.drop_commit {self.committed=self.staged;if self.corrupt {self.committed[7]^=1;}}
                return Ok(());
            }
            Err(())
        }
    }
    fn journal()->MockJournal {MockJournal {staged:[0;8],committed:[0;8],writes:0,polls:0,drop_commit:false,corrupt:false}}
    #[test] fn commit_distinguishes_complete_lost_corrupt_and_stale_evidence() {
        let record=[1,2,3,4,5,6,7,8];
        let mut good=journal(); assert_eq!(commit_record(&mut good,record),Ok(())); assert_eq!(good.writes,9);
        assert_eq!(good.committed,record);
        let mut lost=journal();lost.drop_commit=true;
        assert_eq!(commit_record(&mut lost,record),Err(CommitError::Timeout));
        assert_eq!((lost.writes,lost.polls),(9,1025));
        let mut corrupt=journal();corrupt.corrupt=true;
        assert_eq!(commit_record(&mut corrupt,record),Err(CommitError::Echo));assert_eq!(corrupt.writes,9);
        let mut stale=journal();stale.committed[0]=1;
        assert_eq!(commit_record(&mut stale,record),Err(CommitError::Sequence));assert_eq!(stale.writes,0);
    }
    #[test]
    fn diagnostic_commit_never_publishes_partial_wide_record() {
        struct Device { writes:usize, fail:usize, staged:[u32;19], published:Option<[u32;19]>, drained:bool }
        impl JournalIo for Device {
            type Error=usize;
            fn read(&mut self,offset:u64)->Result<u32,usize>{assert_eq!(offset,0);self.drained=true;Ok(0x4a4d5653)}
            fn write(&mut self,offset:u64,value:u32)->Result<(),usize>{
                let n=self.writes;self.writes+=1;if n==self.fail{return Err(n);}
                assert_eq!(offset,0xfb0+n as u64*4);
                if n<19{self.staged[n]=value;}else{assert_eq!(value,self.staged[0]);self.published=Some(self.staged);}
                Ok(())
            }
        }
        let payload=diagnostic_payload(1,12,true,0x87654321,0x34,u64::MAX,
            [8,0xc0010010,0xfedcba9876543210,0x123456789abcdef0,0x8000000000000003,1|(24u64<<32)],4);
        assert_eq!(payload[10],0x76543210);assert_eq!(payload[11],0xfedcba98);
        assert_eq!(payload[12],0x9abcdef0);assert_eq!(payload[13],0x12345678);
        for fail in 0..=20 {
            let mut io=Device{writes:0,fail,staged:[0;19],published:None,drained:false};
            let result=commit_diagnostic(&mut io,31,payload);
            if fail<20{assert_eq!(result,Err(CommitError::Access(fail)));assert!(io.published.is_none());assert!(!io.drained);}
            else{assert_eq!(result,Ok(()));assert_eq!(io.published,Some(payload));assert!(io.drained);}
        }
        let mut io=Device{writes:0,fail:20,staged:[0;19],published:None,drained:false};
        assert_eq!(commit_diagnostic(&mut io,32,payload),Err(CommitError::Sequence));
        let mut invalid=payload;invalid[0]=0;
        assert_eq!(commit_diagnostic(&mut io,31,invalid),Err(CommitError::Sequence));assert_eq!(io.writes,0);
    }

}
