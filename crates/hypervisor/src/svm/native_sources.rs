//! Retained direct-device interrupt routes and reverse level-EOI ownership.
//!
//! GA delivery writes virtual IRR directly and does not create a physical ISR
//! entry. Completion therefore uses retained source routing, never a physical
//! LAPIC EOI. No hardware route is enabled by this module.
use super::x2avic::BackingPage;
use crate::sync::{TryLock, TryLockGuard};

pub const MAX_ROUTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceId {
    pub segment: u16,
    pub requester: u16,
    pub index: u32,
}

/// A platform-qualified directed EOI register physical address and the vector
/// carried by the original physical source. It need not equal the guest vector.
/// Construction does not qualify a platform or map an MMIO aperture. Each CPU's
/// callback must establish its own UC host mapping; private host VAs must never
/// be stored in this shared cross-CPU route table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectedEoi {
    pub register: u64,
    pub source_vector: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    Edge,
    UnqualifiedLevel,
    Directed(DirectedEoi),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub source: SourceId,
    pub target: u16,
    pub vector: u8,
    pub completion: Completion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Poisoned,
    InvalidRoute,
    Full,
    SourceAlreadyOwned,
    TriggerConflict,
    EoiAlias,
    UnknownLevelSource,
    UnqualifiedEoi,
    PendingInterrupt,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompletionError<E> { Ownership(Error), Backend(E) }

struct Routes { entries: [Option<Route>; MAX_ROUTES], poisoned: bool }

/// Shared retained allocation, initialized before APs or source writers start.
/// Bounded try-lock: a contended VM exit stops or retries through its scheduler;
/// it must never spin while another CPU might require its service.
pub struct SharedRoutes(TryLock<Routes>);

impl Default for SharedRoutes { fn default() -> Self { Self::new() } }
impl SharedRoutes {
    pub const fn new() -> Self {
        Self(TryLock::new(Routes { entries: [None; MAX_ROUTES], poisoned: false }))
    }

    pub fn try_lock(&self) -> Result<Guard<'_>, Error> {
        self.0.try_lock().map(Guard).ok_or(Error::Busy)
    }
}

pub struct Guard<'a>(TryLockGuard<'a, Routes>);

impl Guard<'_> {
    fn routes(&self) -> &Routes { &self.0 }
    fn routes_mut(&mut self) -> &mut Routes { &mut self.0 }

    /// Reject a physical source that would alias any direct-route vector before
    /// its IRR/TMR publication. The lock must cover that publication as well.
    pub fn has_route(&self, target: u16, vector: u8) -> Result<bool, Error> {
        if self.routes().poisoned { return Err(Error::Poisoned); }
        Ok(self.routes().entries.iter().flatten().any(|r|
            r.target == target && r.vector == vector))
    }

    pub fn has_level(&self, target: u16, vector: u8) -> Result<bool, Error> {
        if self.routes().poisoned { return Err(Error::Poisoned); }
        Ok(self.routes().entries.iter().flatten().any(|r|
            r.target == target && r.vector == vector && r.completion != Completion::Edge))
    }

    /// Retain a NEW route and prepare TMR, before its hardware source is enabled.
    /// Caller must exclude guest execution and all IPI/device writers of this
    /// target vector until publication completes. Source identity, destination,
    /// register qualification and backing-page association must already have
    /// been checked by the hardware publisher. This is not a drain operation.
    /// Existing routes are deliberately not replaced: source disable plus an
    /// ordinary IOMMU CompletionWait has not established GA writer quiescence.
    pub fn install_stopped(&mut self, route: Route, backing: &BackingPage) -> Result<(), Error> {
        let routes = self.routes();
        if routes.poisoned { return Err(Error::Poisoned); }
        if route.target > super::x2avic::MAX_ID || route.vector < 16 {
            return Err(Error::InvalidRoute);
        }
        let level = route.completion != Completion::Edge;
        match route.completion {
            Completion::UnqualifiedLevel => return Err(Error::UnqualifiedEoi),
            Completion::Directed(eoi) if eoi.register == 0 || eoi.register & 3 != 0 =>
                return Err(Error::InvalidRoute),
            _ => {},
        }
        for old in routes.entries.iter().flatten() {
            if old.source == route.source { return Err(Error::SourceAlreadyOwned); }
            if old.target == route.target && old.vector == route.vector
                && (old.completion != Completion::Edge) != level {
                return Err(Error::TriggerConflict);
            }
            // One directed EOI may clear every RTE with the same physical
            // vector. It must never acknowledge a different virtual owner.
            if matches!((old.completion, route.completion),
                (Completion::Directed(a), Completion::Directed(b)) if a == b)
                && (old.target != route.target || old.vector != route.vector) {
                return Err(Error::EoiAlias);
            }
        }
        let slot = routes.entries.iter().position(Option::is_none).ok_or(Error::Full)?;
        if backing.is_pending(route.vector) || backing.is_in_service(route.vector) {
            return Err(Error::PendingInterrupt);
        }
        backing.prepare_trigger_stopped(route.vector, level).map_err(|_| Error::TriggerConflict)?;
        self.routes_mut().entries[slot] = Some(route);
        Ok(())
    }

    /// Complete the source side of an already performed virtual level EOI.
    /// All owners are checked before the first callback. Keep this guard held
    /// through every MMIO write so reprogramming cannot change reverse mapping.
    /// The callback owns mapped UC access and exclusion of simultaneous physical
    /// EOI broadcasts. A failed hardware callback poisons the owner: a partial
    /// EOI sequence must not be replayed or reported as a successful completion.
    pub fn complete_level<E>(&mut self, target: u16, vector: u8,
        mut write: impl FnMut(DirectedEoi) -> Result<(), E>,
    ) -> Result<(), CompletionError<E>> {
        use CompletionError::Ownership;
        if !self.has_level(target, vector).map_err(Ownership)? {
            return Err(Ownership(Error::UnknownLevelSource));
        }
        let matches = |r: &&Route| r.target == target && r.vector == vector;
        if self.routes().entries.iter().flatten().filter(matches)
            .any(|r| r.completion == Completion::UnqualifiedLevel) {
            return Err(Ownership(Error::UnqualifiedEoi));
        }
        for i in 0..MAX_ROUTES {
            let Some(route) = self.routes().entries[i] else { continue; };
            if route.target != target || route.vector != vector { continue; }
            let Completion::Directed(eoi) = route.completion else { continue; };
            if self.routes().entries[..i].iter().flatten().any(|old|
                old.target == target && old.vector == vector
                && old.completion == Completion::Directed(eoi)) { continue; }
            if let Err(error) = write(eoi) {
                self.routes_mut().poisoned = true;
                return Err(CompletionError::Backend(error));
            }
        }
        Ok(())
    }
}

const _: () = assert!(core::mem::size_of::<SharedRoutes>() <= 0x4000);
