//! Post-EBS cache-sample collection gate.

use crate::svm::native_cache::MAX_CACHE_CPUS;

/// Post-EBS collection gate. Firmware may synchronize MTRRs in its final
/// callbacks, so no guest may consume the bank until every owned CPU sampled
/// it and the BSP admitted the complete capture. This gate never grants MSR
/// write permission or replaces the bank/domain checks.
pub struct CacheSurvey {
    sampled: core::sync::atomic::AtomicU32,
    admitted: core::sync::atomic::AtomicBool,
    failed: core::sync::atomic::AtomicBool,
}

impl CacheSurvey {
    pub const fn new() -> Self {
        Self {
            sampled: core::sync::atomic::AtomicU32::new(0),
            admitted: core::sync::atomic::AtomicBool::new(false),
            failed: core::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn sampled(&self, slot: usize) -> bool {
        use core::sync::atomic::Ordering;
        slot < MAX_CACHE_CPUS && self.sampled.load(Ordering::Acquire) & (1 << slot) != 0
    }

    pub fn failed(&self) -> bool {
        self.failed.load(core::sync::atomic::Ordering::Acquire)
    }
    pub fn admitted(&self) -> bool {
        !self.failed() && self.admitted.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Sole serial capture writer publishes after its complete bank write.
    pub fn complete_sample(&self, slot: usize) -> bool {
        use core::sync::atomic::Ordering;
        slot < MAX_CACHE_CPUS
            && !self.failed.load(Ordering::Acquire)
            && self.sampled.fetch_or(1 << slot, Ordering::AcqRel) & (1 << slot) == 0
    }
    /// BSP only, after full bank/topology/owner admission succeeded.
    pub fn admit(&self, count: usize) -> bool {
        use core::sync::atomic::Ordering;
        if !(1..=MAX_CACHE_CPUS).contains(&count)
            || self.failed.load(Ordering::Acquire)
            || self.sampled.load(Ordering::Acquire) != u32::MAX >> (MAX_CACHE_CPUS - count)
        {
            return false;
        }
        self.admitted.store(true, Ordering::Release);
        true
    }
    pub fn abort(&self) {
        self.failed.store(true, core::sync::atomic::Ordering::Release);
    }
}

impl Default for CacheSurvey {
    fn default() -> Self {
        Self::new()
    }
}
