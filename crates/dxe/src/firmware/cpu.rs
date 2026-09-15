//! Observational AMD APM RDTSC and CPUID Fn0000_0001_EBX initial APIC ID.
pub(crate) fn sample() -> (u64, u32) {
    // x64 UEFI executes on a CPU supporting CPUID/RDTSC. No control-state writes.
    unsafe {
        (
            core::arch::x86_64::_rdtsc(),
            core::arch::x86_64::__cpuid(1).ebx >> 24,
        )
    }
}
