//! Owned clock controls tied to one immutable emulator host CPU identity.
use core::arch::{asm, x86_64::__cpuid};
use svmvisor_hypervisor::arch::x86_64::clock::{
    ClockCapabilities, ClockPlan, TSC_AUX_MSR, TSC_RATIO_MSR,
};

#[repr(C)]
pub struct Switch {
    pub flags: u64,
    pub guest_aux: u64,
    pub guest_ratio: u64,
    pub host_aux: u64,
    pub host_ratio: u64,
    pub exit_aux: u64,
    pub exit_ratio: u64,
    pub restored_aux: u64,
    pub restored_ratio: u64,
    pub completed: u64,
}
const _: () = {
    assert!(core::mem::size_of::<Switch>() == 80);
    assert!(core::mem::offset_of!(Switch, flags) == 0);
    assert!(core::mem::offset_of!(Switch, guest_aux) == 8);
    assert!(core::mem::offset_of!(Switch, guest_ratio) == 16);
    assert!(core::mem::offset_of!(Switch, host_aux) == 24);
    assert!(core::mem::offset_of!(Switch, host_ratio) == 32);
    assert!(core::mem::offset_of!(Switch, exit_aux) == 40);
    assert!(core::mem::offset_of!(Switch, exit_ratio) == 48);
    assert!(core::mem::offset_of!(Switch, restored_aux) == 56);
    assert!(core::mem::offset_of!(Switch, restored_ratio) == 64);
    assert!(core::mem::offset_of!(Switch, completed) == 72);
};
pub struct State {
    capabilities: ClockCapabilities,
    plan: ClockPlan,
    cpu: u32,
}
impl State {
    /// # Safety
    /// Owned stopped emulator CPU at CPL0, valid AMD CPUID and SVM already
    /// established. Only capability-gated MSRs are read. APM 3.44 15.30.5;
    /// AMD PPR 57896 revision 3.00, MSRC000_0103/0104 definitions.
    pub unsafe fn capture() -> Self {
        assert!(__cpuid(0x80000000).eax >= 0x8000000a);
        let capabilities = ClockCapabilities::detect(
            __cpuid(1).edx,
            __cpuid(0x80000001).edx,
            __cpuid(0x8000000a).edx,
        )
        .unwrap();
        let host_aux = if capabilities.rdtscp() {
            Some(unsafe { read_msr(TSC_AUX_MSR) })
        } else {
            None
        };
        let host_ratio = if capabilities.scaling() {
            Some(unsafe { read_msr(TSC_RATIO_MSR) })
        } else {
            None
        };
        let guest_aux = host_aux.map_or(0, |value| value as u32 ^ 0x53564d01);
        let plan = ClockPlan::admit(capabilities, host_aux, host_ratio, guest_aux).unwrap();
        let cpu = crate::host_smp::current_cpu() as u32;
        if cpu == 0 {
            crate::print(if capabilities.scaling() {
                "clock-ratio=identity-owned\n"
            } else {
                "SKIP clock-ratio unsupported\n"
            });
        }
        Self {
            capabilities,
            plan,
            cpu,
        }
    }
    /// Reuse this physical CPU's captured evidence with a distinct guest AUX.
    pub fn for_guest(&self, identity: u32) -> Self {
        Self {
            capabilities: self.capabilities,
            cpu: self.cpu,
            plan: ClockPlan::admit(
                self.capabilities,
                self.plan.host_aux(),
                self.plan.host_ratio(),
                identity,
            )
            .unwrap(),
        }
    }
    pub fn capabilities(&self) -> &ClockCapabilities {
        &self.capabilities
    }
    pub fn plan(&self) -> &ClockPlan {
        &self.plan
    }
    pub fn guest_aux(&self) -> Option<u64> {
        self.plan.guest_aux()
    }
    /// Serialized source sample naming the CPU that captured this clock owner.
    ///
    /// # Safety
    /// Same exclusively owned CPL0 CPU as capture/entry, with host clock MSRs
    /// restored and migration prohibited. CPU identity comes from the admitted
    /// CPUID APIC ID 0/1. This is not general topology discovery. APM2 rev.3.44
    /// sections 7.6.4 and 13.2.4: RDTSC is not serializing, hence CPUID on both
    /// sides. No invariant frequency or physical APIC/TSC ratio is inferred.
    pub unsafe fn sample(&self) -> svmvisor_hypervisor::svm::apic_scheduler::ClockSample {
        assert_eq!(self.cpu as usize, crate::host_smp::current_cpu());
        assert_eq!(self.plan.tsc_offset(), 0);
        let low: u32;
        let high: u32;
        let _ = __cpuid(0);
        unsafe {
            asm!("rdtsc", out("eax") low, out("edx") high, options(nostack));
        }
        let _ = __cpuid(0);
        svmvisor_hypervisor::svm::apic_scheduler::ClockSample {
            ticks: u64::from(low) | (u64::from(high) << 32),
            cpu: self.cpu,
        }
    }
    pub fn prepare(&self) -> Switch {
        assert_eq!(self.cpu as usize, crate::host_smp::current_cpu());
        Switch {
            flags: u64::from(self.capabilities.rdtscp())
                | (u64::from(self.capabilities.scaling()) << 1),
            guest_aux: self.plan.guest_aux().unwrap_or(0),
            guest_ratio: self.plan.guest_ratio().unwrap_or(0),
            host_aux: 0,
            host_ratio: 0,
            exit_aux: 0,
            exit_ratio: 0,
            restored_aux: 0,
            restored_ratio: 0,
            completed: 0,
        }
    }
    pub fn verify(&self, observed: &Switch) {
        assert_eq!(observed.completed, 1);
        let aux = self.capabilities.rdtscp();
        let ratio = self.capabilities.scaling();
        self.plan
            .validate_restored(
                aux.then_some(observed.host_aux),
                ratio.then_some(observed.host_ratio),
            )
            .unwrap();
        self.plan
            .validate_restored(
                aux.then_some(observed.restored_aux),
                ratio.then_some(observed.restored_ratio),
            )
            .unwrap();
        assert_eq!(aux.then_some(observed.exit_aux), self.plan.guest_aux());
        assert_eq!(
            ratio.then_some(observed.exit_ratio),
            self.plan.guest_ratio()
        );
    }
}
unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    }
    u64::from(low) | (u64::from(high) << 32)
}
