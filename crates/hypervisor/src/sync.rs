//! Non-blocking lock for state shared between CPUs without a scheduler.
//!
//! Acquisition never waits. Each caller decides whether contention means a
//! bounded retry, a refusal or a stop; a guard must never be held while waiting
//! for another CPU. No allocation, formatting or firmware call occurs.

use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

/// A value serialized by one atomic flag. `repr(C)` keeps the flag first so
/// shared-memory owners containing this type keep a fixed, asserted layout.
#[repr(C)]
pub struct TryLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: every shared access to `value` goes through the single guard that
// won the flag, so only `Send` data is ever handed between CPUs.
unsafe impl<T: Send> Sync for TryLock<T> {}

impl<T> TryLock<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// `None` means another owner currently holds the lock.
    pub fn try_lock(&self) -> Option<TryLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(TryLockGuard { lock: self })
    }

    /// Run `operation` under the lock, or return `None` without running it.
    pub fn with<R>(&self, operation: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut guard = self.try_lock()?;
        Some(operation(&mut guard))
    }

    /// Exclusive access through `&mut self`, for single-writer initialization.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

pub struct TryLockGuard<'a, T> {
    lock: &'a TryLock<T>,
}

impl<T> Deref for TryLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: this guard is the only holder of the flag.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for TryLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this guard is the only holder of the flag, and `&mut self`
        // excludes any other borrow obtained through it.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for TryLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::TryLock;

    #[test]
    fn contention_refuses_without_running_and_release_reopens() {
        let lock = TryLock::new(1u32);
        let mut guard = lock.try_lock().unwrap();
        *guard += 1;
        assert!(lock.try_lock().is_none());
        assert_eq!(lock.with(|_| panic!("ran while held")), None::<()>);
        drop(guard);
        assert_eq!(lock.with(|value| { *value += 1; *value }), Some(3));
        assert_eq!(*lock.try_lock().unwrap(), 3);
    }

    #[test]
    fn exclusive_initialization_needs_no_flag() {
        let mut lock = TryLock::new([0u8; 4]);
        lock.get_mut()[2] = 7;
        let guard = lock.try_lock().unwrap();
        assert_eq!(*guard, [0, 0, 7, 0]);
    }
}
