//! Native guarded reads with no retained exception handler between operations.
//!
//! Each table-entry read registers the bounded probe, performs one source load,
//! and removes the callback before returning. A firmware setter can therefore
//! run between reads without inheriting the probe's unmatched-fault policy.

use super::firmware::QualifiedTableReader;
use super::probe::{
    CpuArchProtocol, MAX_PROBE_READS, NativeProbe, ProbeError, ProbeProfile, RamExtent,
    validate_source,
};
use core::arch::asm;
use svmvisor_memory_attributes::{Config, Error};

pub struct TransientTableReader<'a> {
    cpu: *mut CpuArchProtocol,
    profile: ProbeProfile,
    extents: &'a [RamExtent],
    reads: usize,
    last_error: Option<ProbeError>,
}

// SAFETY: Plain metadata and a retained firmware pointer are transferable; every
// dereference/registration/load/removal is through NativeProbe, which checks the
// complete owner CPU identity first. This object never retains a live handler
// and its destruction does not invoke CPU-local firmware or free shared state.
unsafe impl Send for TransientTableReader<'_> {}

impl<'a> TransientTableReader<'a> {
    /// # Safety
    /// Every NativeProbe::register qualification must remain valid for each
    /// invocation for this object's lifetime, on the admitted BSP only. The
    /// caller must permit a finite local interrupt-disabled interval at its
    /// current firmware TPL; this wrapper saves/restores the original RFLAGS.
    /// original table contents and paging context must remain stable across a
    /// complete Get or change, except the qualified setter's intended updates.
    /// Safe borrowed reads do not establish ownership of the tables or target.
    /// NXE/1GiB fields must reflect actual enabled/supported capabilities. The
    /// protocol, extents and implementing image must remain resident; removal
    /// must synchronously quiesce its callback. Fault/cleanup failure policy is
    /// NativeProbe's explicit fail-stop policy, never a successful read.
    pub unsafe fn new(
        cpu: *mut CpuArchProtocol,
        profile: ProbeProfile,
        extents: &'a [RamExtent],
    ) -> Self {
        Self {
            cpu,
            profile,
            extents,
            reads: 0,
            last_error: None,
        }
    }

    pub fn last_error(&self) -> Option<ProbeError> {
        self.last_error
    }
    pub fn reads(&self) -> usize {
        self.reads
    }
}

// SAFETY: The unsafe constructor requires independent stable-table/profile and
// physical-source qualification. Actual loads are always guarded; temporary
// registration is fully removed before data can escape or a setter can run.
unsafe impl QualifiedTableReader for TransientTableReader<'_> {
    fn config(&self) -> Config {
        Config {
            root: self.profile.root,
            physical_bits: self.profile.physical_bits,
            nxe: self.profile.nxe,
            page1gb: self.profile.page1gb,
        }
    }

    fn read_entry(&mut self, address: u64) -> Result<u64, Error> {
        if self.reads >= MAX_PROBE_READS {
            return Err(Error::OutOfResources);
        }
        if let Err(error) = validate_source(self.profile, self.extents, address) {
            self.last_error = Some(error);
            return Err(map_error(error));
        }
        // Refuse off-owner/privilege calls before changing local interrupt state
        // or invoking any firmware. Full topology identity is profile-qualified.
        let cs: u16;
        unsafe {
            asm!("mov {:x}, cs",out(reg)cs,options(nomem,nostack,preserves_flags));
        }
        if cs & 3 != 0 {
            return Err(Error::AccessDenied);
        }
        let maximum = core::arch::x86_64::__cpuid(0).eax;
        let topology = core::arch::x86_64::__cpuid_count(0x0b, 0);
        if maximum < 0x0b
            || topology.ebx & 0xffff == 0
            || (topology.ecx >> 8) & 0xff == 0
            || topology.edx != self.profile.bsp_apic_id
        {
            self.last_error = Some(ProbeError::WrongCpu);
            return Err(Error::AccessDenied);
        }
        self.reads += 1;
        // No change to firmware TPL. Every returning path restores this scope;
        // the underlying probe still independently refuses unsafe flag states.
        let _interrupts = unsafe { InterruptScope::enter() };
        // SAFETY: The constructor requires the qualification to hold at every
        // call. NativeProbe refuses an off-owner CPU before accessing its slot.
        let mut probe = match unsafe { NativeProbe::register(self.cpu, self.profile, self.extents) }
        {
            Ok(probe) => probe,
            Err(error) => {
                self.last_error = Some(error);
                return Err(map_error(error));
            }
        };
        let value = probe.read_u64(address);
        if let Err(error) = probe.try_close() {
            self.last_error = Some(error);
            // NativeProbe's Drop cannot release an uncertain live callback. It
            // takes the explicit terminal path, so no apparent Err handoff can
            // unload code still reachable from the exception dispatcher.
            drop(probe);
            return Err(map_error(error));
        }
        drop(probe);
        match value {
            Ok(value) => {
                self.last_error = None;
                Ok(value)
            }
            Err(error) => {
                self.last_error = Some(error);
                Err(map_error(error))
            }
        }
    }
}

struct InterruptScope(u64);
impl InterruptScope {
    unsafe fn enter() -> Self {
        let flags: u64;
        unsafe {
            asm!("pushfq; pop {}; cli",out(reg)flags);
        }
        Self(flags)
    }
}
impl Drop for InterruptScope {
    fn drop(&mut self) {
        unsafe {
            asm!("push {}; popfq",in(reg)self.0);
        }
    }
}

fn map_error(error: ProbeError) -> Error {
    match error {
        ProbeError::Capacity => Error::OutOfResources,
        ProbeError::WrongCpu | ProbeError::Busy => Error::AccessDenied,
        ProbeError::ReaderFault | ProbeError::InvalidSource | ProbeError::SourceOutsideRam => {
            Error::Unsupported
        }
        ProbeError::InvalidProfile | ProbeError::ControlState => Error::Unsupported,
        ProbeError::Registration(_) | ProbeError::Removal(_) => Error::DeviceError,
    }
}
