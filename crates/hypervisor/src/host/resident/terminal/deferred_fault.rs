//! The first-fault latch: one diagnostic payload captured per runtime and held until published.

use core::sync::atomic::{AtomicU32, Ordering};

/// Per-runtime first-fault latch. No global guard or allocation is needed;
/// a fault interrupting the writer never spins on that interrupted writer.
/// Publication failure leaves the immutable payload available for later retry.
pub struct DeferredFault {
    state: AtomicU32,
    words: [AtomicU32; 19],
    failures: AtomicU32,
}

impl DeferredFault {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            words: [const { AtomicU32::new(0) }; 19],
            failures: AtomicU32::new(0),
        }
    }
    pub fn capture(&self, words: [u32; 19]) {
        if self.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return;
        }
        for (to, from) in self.words.iter().zip(words) {
            to.store(from, Ordering::Relaxed);
        }
        self.state.store(2, Ordering::Release);
    }
    pub fn pending(&self) -> Option<[u32; 19]> {
        if self.state.load(Ordering::Acquire) != 2 {
            return None;
        }
        Some(core::array::from_fn(|i| self.words[i].load(Ordering::Relaxed)))
    }
    pub fn published(&self) {
        self.state.store(3, Ordering::Release);
    }
    pub fn failed(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }
    pub fn status(&self) -> u64 {
        self.state.load(Ordering::Acquire) as u64
            | ((self.failures.load(Ordering::Relaxed) as u64) << 32)
    }
}

impl Default for DeferredFault {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use svmvisor_card_abi::journal::diagnostic_payload;

    use crate::host::resident::terminal::TerminalControl;

    #[test]
    fn deferred_fault_survives_contention_missing_cpu_and_reentrant_capture() {
        let control = TerminalControl::new();
        assert!(control.publish_ready(2));
        assert!(control.claim(0, 2));
        assert!(control.acknowledge(0, 2));
        let latch = DeferredFault::new();
        let first = diagnostic_payload(17, 3, true, 1, 0, 2, [1, 2, 3, 4, 5, 6], 7);
        let guard = control.diagnostic_lock().unwrap();
        latch.capture(first);
        assert!(control.diagnostic_lock().is_none());
        latch.failed();
        latch.capture([99; 19]);
        assert_eq!(latch.pending(), Some(first));
        assert_eq!(latch.status(), 2 | (1 << 32));
        assert!(!control.all_acknowledged(2));
        drop(guard);
        let _guard = control.diagnostic_lock().unwrap();
        // Publication is permitted by the lifetime guard despite missing peer.
        assert_eq!(latch.pending(), Some(first));
        latch.published();
        assert_eq!(latch.pending(), None);
        latch.capture([88; 19]);
        assert_eq!(latch.status(), 3 | (1 << 32));
        let interrupted = DeferredFault::new();
        interrupted.state.store(1, Ordering::Relaxed);
        interrupted.capture(first); // Returns immediately, never waits on itself.
        assert_eq!(interrupted.pending(), None);
    }
}
