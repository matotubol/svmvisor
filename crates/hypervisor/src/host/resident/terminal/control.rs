//! The shared terminal control page: ready, claim, acknowledge and the diagnostic gate.

use core::sync::atomic::{AtomicU32, Ordering};

pub const CONTROL_OFFSET: u64 = 0x800;

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

const _: () = {
    assert!(core::mem::size_of::<TerminalControl>() == 256);
    assert!(
        CONTROL_OFFSET
            == 32
                * core::mem::size_of::<crate::svm::x2avic::startup::NativeStartupMailbox>() as u64
    );
    assert!(CONTROL_OFFSET + core::mem::size_of::<TerminalControl>() as u64 <= 4096);
};

impl TerminalControl {
    pub const fn new() -> Self {
        Self {
            ready: AtomicU32::new(0),
            expected: AtomicU32::new(0),
            owner: AtomicU32::new(0),
            request: AtomicU32::new(0),
            acknowledged: AtomicU32::new(0),
            outcome: AtomicU32::new(0),
            initial_acknowledged: AtomicU32::new(0),
            diagnostic_gate: AtomicU32::new(0),
            diagnostic_revoked: AtomicU32::new(0),
            reserved: [0; 55],
        }
    }
    pub fn publish_ready(&self, count: usize) -> bool {
        let Some(mask) = cpu_mask(count) else {
            return false;
        };
        if self.ready.load(Ordering::Acquire) != 0 || self.owner.load(Ordering::Acquire) != 0 {
            return false;
        }
        self.expected.store(mask, Ordering::Relaxed);
        self.ready.compare_exchange(0, 1, Ordering::Release, Ordering::Relaxed).is_ok()
    }
    /// Monotonic initial-entry proof; guest INIT/SIPI never clears these bits.
    pub fn initial_ack(&self, slot: usize, count: usize) -> bool {
        let Some(mask) = cpu_mask(count) else {
            return false;
        };
        if slot >= count {
            return false;
        }
        let seen = self.initial_acknowledged.fetch_or(1 << slot, Ordering::AcqRel) | (1 << slot);
        if seen == mask { self.publish_ready(count) } else { false }
    }
    pub fn ready(&self, count: usize) -> bool {
        self.ready.load(Ordering::Acquire) == 1
            && cpu_mask(count) == Some(self.expected.load(Ordering::Relaxed))
    }
    pub fn requested(&self) -> bool {
        self.request.load(Ordering::Acquire) == 1
    }
    pub fn claim(&self, slot: usize, count: usize) -> bool {
        if slot >= count || !self.ready(count) {
            return false;
        }
        if self
            .owner
            .compare_exchange(0, slot as u32 + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.request.store(1, Ordering::Release);
        true
    }
    pub fn acknowledge(&self, slot: usize, count: usize) -> bool {
        if slot >= count || !self.ready(count) || !self.requested() {
            return false;
        }
        self.acknowledged.fetch_or(1 << slot, Ordering::Release);
        true
    }
    pub fn all_acknowledged(&self, count: usize) -> bool {
        self.ready(count)
            && self.requested()
            && cpu_mask(count) == Some(self.acknowledged.load(Ordering::Acquire))
    }
    pub fn finish(&self, result: u32) {
        self.outcome.store(result, Ordering::Release);
    }
    pub fn diagnostic_lock(&self) -> Option<DiagnosticGuard<'_>> {
        self.diagnostic_gate
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| DiagnosticGuard(self))
    }
    pub fn diagnostic_revoked(&self) -> bool {
        self.diagnostic_revoked.load(Ordering::Acquire) != 0
    }
    pub fn diagnostic_revoke(&self) {
        self.diagnostic_revoked.store(1, Ordering::Release);
    }
    pub fn diagnostic_snapshot(&self) -> [u64; 6] {
        [
            self.expected.load(Ordering::Acquire) as u64,
            self.acknowledged.load(Ordering::Acquire) as u64,
            self.owner.load(Ordering::Acquire) as u64,
            self.outcome.load(Ordering::Acquire) as u64,
            self.initial_acknowledged.load(Ordering::Acquire) as u64,
            0,
        ]
    }
}

impl Default for TerminalControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Nonblocking lifetime exclusion shared by publication and native config I/O.
pub struct DiagnosticGuard<'a>(&'a TerminalControl);

impl Drop for DiagnosticGuard<'_> {
    fn drop(&mut self) {
        self.0.diagnostic_gate.store(0, Ordering::Release);
    }
}

pub fn cpu_mask(count: usize) -> Option<u32> {
    if count == 32 {
        Some(u32::MAX)
    } else if (1..32).contains(&count) {
        Some((1u32 << count) - 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_requires_all_cpus_and_never_reopens() {
        let c = TerminalControl::new();
        assert!(!c.claim(0, 2));
        assert!(c.publish_ready(2));
        assert!(c.claim(0, 2));
        assert!(!c.claim(1, 2));
        assert!(c.acknowledge(0, 2));
        assert!(!c.all_acknowledged(2));
        assert!(c.acknowledge(1, 2));
        assert!(c.all_acknowledged(2));
        c.finish(2);
        assert!(c.requested());
        assert!(!c.publish_ready(2));
    }
    #[test]
    fn initial_ack_gate_cannot_open_with_missing_cpu_or_lose_proof_on_restart() {
        let c = TerminalControl::new();
        for slot in 0..31 {
            assert!(!c.initial_ack(slot, 32));
        }
        assert!(!c.ready(32));
        assert!(!c.claim(0, 32));
        assert!(c.initial_ack(31, 32));
        assert!(c.ready(32));
        assert!(!c.initial_ack(2, 32));
        assert!(c.ready(32));
        assert!(c.claim(31, 32));
        for slot in 0..31 {
            assert!(c.acknowledge(slot, 32));
        }
        assert!(!c.all_acknowledged(32));
        assert!(c.acknowledge(31, 32));
        assert!(c.all_acknowledged(32));
    }
}
