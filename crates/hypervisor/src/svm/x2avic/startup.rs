//! x2AVIC software INIT/SIPI transport.
//!
//! Native fixed IPIs belong to hardware AVIC; incomplete ones are classified
//! and fanned out by `ipi`. This module publishes only checked INIT/SIPI
//! commands; the destination owns stopped CPU state and must separately apply
//! the virtual APIC reset and settle interrupt-source ownership before
//! applying INIT (`registers::commit_init`). The route lock protects
//! publication only.
use super::{NativeX2AvicProfile, ipi::Inventory};
use crate::{
    arch::x86_64::registers::GuestRegisters,
    svm::{events::ExternalInterruptError, vmcb::Vmcb},
};
use core::sync::atomic::{AtomicU64, Ordering};

/// Immutable native CPU inventory for software INIT/SIPI publication and
/// incomplete-IPI classification. Captured CPUs are running continuations,
/// never relabeled as cold APs.
pub struct NativeIcr {
    inventory: Inventory,
    route_failure: Option<NativeRouteFailure>,
}

/// Check-site evidence only; neither a guest state mutation nor another route
/// attempt. Recipient observations are copied while the route guard is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRouteFailure {
    pub value: u64,
    pub source: u32,
    pub predicate: NativeRoutePredicate,
    pub recipient: Option<NativeRouteRecipient>,
}

/// Startup-route refusal predicate, a stable wire code. Values 3
/// (unassigned physical destination), 7 (foreign match), 8 (duplicate match)
/// and 10 (selected self) are retired with the separate physical-only
/// matching; decoders keep their names for older images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeRoutePredicate {
    /// Self or all-including-self shorthand (Table 16-4 p644).
    DestinationForm = 1,
    /// The destination selects the sending CPU.
    SelfDestination = 2,
    RecipientNotReady = 4,
    RecipientModeInvalid = 5,
    /// Destination FFFF_FFFFh without shorthand (16.13 p660).
    Broadcast = 6,
    /// No admitted CPU matches the destination.
    NoMatch = 9,
    MailboxMismatch = 11,
    QueueBusy = 12,
    RouteBusy = 13,
    InitVector = 14,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRouteRecipient {
    pub identity: u32,
    pub mode: Option<NativeDestinationMode>,
    pub init_count: u32,
    pub cause: NativeDestinationCause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeDestinationCause { Observed = 0, GuestControl = 1, GuestInit = 2 }

#[derive(Debug, PartialEq, Eq)]
pub enum NativeIcrError {
    InvalidTopology,
    PendingState(ExternalInterruptError),
    UnsupportedMode,
    MailboxBusy,
    MailboxNotReady,
    MailboxMismatch,
    RoutingBusy,
    UnsupportedStartupEncoding,
    /// A startup IPI whose destination form is not admitted: a self or
    /// all-including-self shorthand, destination FFFF_FFFFh, the sending CPU
    /// itself, or no admitted CPU.
    UnownedStartup {
        value: u64,
    },
}

impl NativeIcr {
    /// Software completion of x2AVIC's non-fixed IPI exit. This owner never
    /// forwards a guest ICR to the physical LAPIC; only INIT/SIPI publication
    /// is admitted. APM2 rev3.44 15.29.9.1 and 16.5/Table16-4.
    pub fn route_x2avic_startup(
        &mut self,
        value: u64,
        mailboxes: &[NativeStartupMailbox],
        kick: impl FnOnce(u32),
    ) -> Result<(), NativeIcrError> {
        if !matches!((value >> 8) & 7, 5 | 6) {
            return Err(NativeIcrError::UnsupportedStartupEncoding);
        }
        self.route_startup(value, mailboxes, kick)
    }

    /// Bind the actual immutable native inventory, never firmware ordinal IDs.
    /// No guest CPU is relabeled Cold and no remote CPU state is borrowed.
    pub fn admit(source: u32, ids: &[u32]) -> Result<Self, NativeIcrError> {
        let inventory = Inventory::admit(source, ids).ok_or(NativeIcrError::InvalidTopology)?;
        Ok(Self { inventory, route_failure: None })
    }

    /// The admitted inventory, for incomplete-IPI classification and
    /// software fixed-IPI fan-out.
    pub fn inventory(&self) -> &Inventory { &self.inventory }

    pub fn route_failure(&self) -> Option<NativeRouteFailure> { self.route_failure }

    fn reject_route(&mut self, value: u64, predicate: NativeRoutePredicate,
        recipient: Option<NativeRouteRecipient>, error: NativeIcrError) -> NativeIcrError
    {
        let source = self.inventory.source_id();
        self.route_failure = Some(NativeRouteFailure { value, source, predicate, recipient });
        error
    }

}

impl NativeIcr {
    fn route_startup(
        &mut self,
        value: u64,
        mailboxes: &[NativeStartupMailbox],
        kick: impl FnOnce(u32),
    ) -> Result<(), NativeIcrError> {
        use NativeIcrError as E;
        use NativeRoutePredicate as P;
        self.route_failure = None;
        let command = (value >> 8) & 7;
        if !matches!(command, 5 | 6) {
            return Err(E::UnsupportedStartupEncoding);
        }
        // APM2 rev3.44 Table 16-4 p644: INIT and STARTUP are valid only with
        // "Destination or all excluding self"; the self and all-including-self
        // shorthands are not.
        let shorthand = (value >> 18) & 3;
        if !matches!(shorthand, 0 | 3) {
            return Err(self.reject_route(value, P::DestinationForm, None, E::UnownedStartup { value }));
        }
        let broadcast = shorthand == 3;
        // DEST FFFF_FFFFh addresses every APIC including the sender (16.13
        // p660, 16.14 p662), which Table 16-4 does not admit either.
        if !broadcast && value >> 32 == u64::from(u32::MAX) {
            return Err(self.reject_route(value, P::Broadcast, None, E::UnownedStartup { value }));
        }
        // The same target computation as a fixed IPI: shorthand 11, or a
        // physical or logical (16.14 p662) destination. Table 16-4's
        // "Destination" covers both destination modes.
        let targets = self.inventory.targets(value);
        if targets & (1 << self.inventory.source_slot()) != 0 {
            return Err(self.reject_route(value, P::SelfDestination, None, E::UnownedStartup { value }));
        }
        if targets == 0 {
            return Err(self.reject_route(value, P::NoMatch, None, E::UnownedStartup { value }));
        }
        if command == 5 && value as u8 != 0 {
            return Err(self.reject_route(value, P::InitVector, None, E::UnsupportedStartupEncoding));
        }
        if let Err(error) = self.validate_mailboxes(mailboxes) {
            return Err(self.reject_route(value, P::MailboxMismatch, None, error));
        }
        {
            // This lock orders software destination metadata and publication.
            // It does not quiesce hardware AVIC IPIs or remote software IRR
            // publications. Release it before the private notification or any
            // target wait. The ICR write has completed, so a busy lease is
            // waited for (`ROUTE_WAIT_ATTEMPTS`) rather than refused at once.
            let routes = match lock_routes_within(mailboxes, ROUTE_WAIT_ATTEMPTS) {
                Ok(routes) => routes,
                Err(error) => return Err(self.reject_route(value, P::RouteBusy, None, error)),
            };
            for mailbox in routes.mailboxes {
                if !mailbox.is_ready() {
                    return Err(self.reject_route(value, P::RecipientNotReady,
                        Some(mailbox.route_recipient()), E::MailboxNotReady));
                }
                // Native guest execution requires one identity-preserving
                // x2APIC profile on every producer and destination.
                if let Err(error) = mailbox.destination_mode() {
                    return Err(self.reject_route(value, P::RecipientModeInvalid,
                        Some(mailbox.route_recipient()), error));
                }
            }
            if command == 5 && value & 0xc000 == 0x8000 {
                // Compatibility completion for legacy INIT deassert. APM2
                // Table16-4 excludes this encoding; PPR57896 rev3 p61 defines
                // its bits but not its effect. Linux v6.19 lapic.c
                // __apic_accept_irq performs no target action, and QEMU
                // 67cd056 apic_deliver returns before CPU INIT. This is not a
                // claim of measured Zen5 silicon behavior. Keep all routing
                // ownership checks, but publish nothing and never kick/reset.
                return Ok(());
            }
            let command = if command == 5 {
                NativeStartupCommand::Init
            } else {
                NativeStartupCommand::Sipi(value as u8)
            };
            // All native producers and target completion hold this same route
            // guard. Preflight every selected FIFO before any publication;
            // a full recipient cannot leave part of a multi-target IPI
            // committed.
            let mut next = [0u64; 32];
            for (slot, target) in routes.mailboxes.iter().enumerate() {
                if targets & (1 << slot) == 0 { continue; }
                let queue = target.queue.load(Ordering::Acquire);
                let Some(entry) = (0..4).find(|i| queue >> (i * 16) & 0xffff == 0) else {
                    return Err(self.reject_route(value, P::QueueBusy,
                        Some(target.route_recipient()), E::MailboxBusy));
                };
                next[slot] = queue | (u64::from(command.encode()) << (entry * 16));
            }
            for (slot, target) in routes.mailboxes.iter().enumerate() {
                if targets & (1 << slot) != 0 { target.queue.store(next[slot], Ordering::Release); }
            }
        }
        // The native notifier already broadcasts a private wake to all peers;
        // only selected mailboxes carry guest commands. The sentinel tells
        // other adapters to issue that all-excluding-self wake as well.
        let single = !broadcast && targets.count_ones() == 1;
        kick(if single { self.inventory.ids()[targets.trailing_zeros() as usize] } else { u32::MAX });
        Ok(())
    }

    fn validate_mailboxes(&self, mailboxes: &[NativeStartupMailbox]) -> Result<(), NativeIcrError> {
        let ids = self.inventory.ids();
        if mailboxes.len() != ids.len()
            || mailboxes
                .iter()
                .zip(ids)
                .any(|(slot, id)| slot.identity() != *id)
        {
            return Err(NativeIcrError::MailboxMismatch);
        }
        Ok(())
    }

}

/// Native guest CPU lifecycle. No Cold state exists: admission is a captured
/// guest that has already entered its resident runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStartupState {
    Running,
    AwaitSipi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStartupCommand {
    Init,
    Sipi(u8),
}

impl NativeStartupCommand {
    fn encode(self) -> u16 {
        match self {
            Self::Init => 0x8000,
            Self::Sipi(vector) => 0x8100 | u16::from(vector),
        }
    }
    fn decode(value: u16) -> Self {
        if value & 0x100 == 0 {
            Self::Init
        } else {
            Self::Sipi(value as u8)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStartupEffect {
    Init,
    Started,
    Ignored,
}

/// Committed physical destination matching on one admitted native LAPIC.
/// Only x2APIC (APM2 rev3.44 16.13) is supported. The value is retained in
/// the mailbox and diagnostic wire formats, so it keeps its original encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum NativeDestinationMode {
    X2Apic = 4,
}

impl NativeDestinationMode {
    fn decode(raw: u64) -> Result<Self, NativeIcrError> {
        match raw {
            4 => Ok(Self::X2Apic),
            0 => Err(NativeIcrError::MailboxNotReady),
            _ => Err(NativeIcrError::InvalidTopology),
        }
    }
}

/// Bounded global routing exclusion in the first mailbox's retained padding.
/// Every CPU must pass the same complete ordered mailbox slice: sub-slices or
/// aliases with a different first entry are not interchangeable lock domains.
/// A target holds it only for its destination record and queue completion,
/// after its INIT/SIPI commit; a source holds it across recipient checks and
/// FIFO publication; low-memory guest reads hold it across their fixed-MTRR
/// sampling. Nobody holds it while notifying, waiting for a target, calling
/// firmware, allocating, committing guest CPU or LAPIC state, or resuming a
/// guest.
pub struct NativeRouteGuard<'a> {
    mailboxes: &'a [NativeStartupMailbox],
    gate: &'a AtomicU64,
    // A CPU-local hardware commit must not move its guard to another thread.
    _local: core::marker::PhantomData<*mut ()>,
}

/// Attempts of a route-lease wait that cannot be retried: an incomplete-IPI
/// source (its ICR write has completed) and a target that has applied a
/// command. Every holder's critical section is a few atomic operations or
/// MSR accesses, far below this bound, so reaching it means a lost holder.
pub const ROUTE_WAIT_ATTEMPTS: u32 = 1 << 24;

/// Acquire the one routing lock, with a bounded contention refusal before any
/// guest or hardware mutation. APM2 15.28/16.5 and the coherent retained shared
/// memory contract of NativeStartupMailbox. Readiness is checked by the source
/// while holding the lock; target initialization may acquire it before ACK.
/// For callers that retry the unchanged guest instruction on refusal.
pub fn try_lock_routes(
    mailboxes: &[NativeStartupMailbox],
) -> Result<NativeRouteGuard<'_>, NativeIcrError> {
    lock_routes_within(mailboxes, 64)
}

/// `try_lock_routes` with `attempts` acquisition attempts, each followed by a
/// PAUSE. The caller holds no other lease while it waits.
pub fn lock_routes_within(
    mailboxes: &[NativeStartupMailbox],
    attempts: u32,
) -> Result<NativeRouteGuard<'_>, NativeIcrError> {
    if mailboxes.is_empty() || mailboxes.len() > 32 {
        return Err(NativeIcrError::MailboxMismatch);
    }
    let gate = &mailboxes
        .first()
        .ok_or(NativeIcrError::MailboxMismatch)?
        .route_gate;
    for _ in 0..attempts {
        if gate
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(NativeRouteGuard {
                mailboxes,
                gate,
                _local: core::marker::PhantomData,
            });
        }
        core::hint::spin_loop();
    }
    Err(NativeIcrError::RoutingBusy)
}

impl Drop for NativeRouteGuard<'_> {
    fn drop(&mut self) {
        self.gate.store(0, Ordering::Release);
    }
}

/// Validated publication which borrows its routing guard. Only the owning
/// target prepares this, and consumes it before releasing the guard. At arm
/// it records the initial x2APIC observation; after a guest INIT it records
/// the INIT (the mode itself never changes). Dropping an unused token is
/// permitted only when no corresponding hardware change occurred.
#[must_use]
pub struct NativeDestinationCommit<'a> {
    destination: &'a AtomicU64,
    history: &'a AtomicU64,
    mode: NativeDestinationMode,
    _guard: core::marker::PhantomData<&'a NativeRouteGuard<'a>>,
}

/// The lease-free part of `NativeRouteGuard::prepare_destination_mode`:
/// `slot` exists and names an admitted CPU. Mailbox identities never change
/// after publication, so a destination checks this before its INIT commit
/// and takes the route lease only afterwards, for the record itself.
pub fn validate_destination_slot(
    mailboxes: &[NativeStartupMailbox],
    slot: usize,
) -> Result<&NativeStartupMailbox, NativeIcrError> {
    let mailbox = mailboxes.get(slot).ok_or(NativeIcrError::MailboxMismatch)?;
    if mailbox.identity() == u32::MAX {
        return Err(NativeIcrError::InvalidTopology);
    }
    Ok(mailbox)
}

impl NativeRouteGuard<'_> {
    pub fn prepare_destination_mode(
        &self,
        slot: usize,
        mode: NativeDestinationMode,
    ) -> Result<NativeDestinationCommit<'_>, NativeIcrError> {
        let mailbox = validate_destination_slot(self.mailboxes, slot)?;
        Ok(NativeDestinationCommit {
            destination: &mailbox.destination,
            history: &mailbox.destination_history,
            mode,
            _guard: core::marker::PhantomData,
        })
    }
}

impl NativeDestinationCommit<'_> {
    /// Infallible metadata commit immediately after the admitted local physical
    /// commit. The borrowed guard remains held until after this release store.
    pub fn commit_destination_mode(self) {
        self.commit_destination_mode_from(NativeDestinationCause::Observed);
    }

    /// Same guarded commit, retaining whether a mode came from guest INIT,
    /// a guest control write, or the initial hardware observation.
    pub fn commit_destination_mode_from(self, cause: NativeDestinationCause) {
        let old = self.history.load(Ordering::Relaxed);
        let count = ((old >> 2) as u32).saturating_add(u32::from(cause == NativeDestinationCause::GuestInit));
        self.history.store((u64::from(count) << 2) | cause as u64, Ordering::Relaxed);
        self.destination.store(self.mode as u64, Ordering::Release);
    }
}

/// Cache-line-sized, shared resident transport. The platform validates and maps
/// the same retained backing in every runtime and excludes it from all NPTs.
/// Immutable APIC identity and readiness bind the one destination consumer;
/// numeric identity alone cannot establish memory ownership. Four FIFO entries
/// bound INIT/SIPI arrival order. Busy/contention refuses before source commit.
/// Producers never touch target CPU state. A successful publication requires an
/// unconditional host kick; the destination services while stopped, and parks
/// AwaitSipi without VMRUN. No offline, rebind or storage reuse is supported.
/// Acquire/release atomics publish readiness and retain arrivals racing service.
#[repr(C, align(64))]
pub struct NativeStartupMailbox {
    identity: u32,
    reserved: u32,
    queue: AtomicU64,
    ready: AtomicU64,
    destination: AtomicU64,
    route_gate: AtomicU64,
    destination_history: AtomicU64,
}

const _: () = assert!(core::mem::size_of::<NativeStartupMailbox>() == 64);
const _: () = assert!(core::mem::align_of::<NativeStartupMailbox>() == 64);
const _: () = assert!(core::mem::offset_of!(NativeStartupMailbox, queue) == 8);
const _: () = assert!(core::mem::offset_of!(NativeStartupMailbox, ready) == 16);
const _: () = assert!(core::mem::offset_of!(NativeStartupMailbox, destination) == 24);
const _: () = assert!(core::mem::offset_of!(NativeStartupMailbox, route_gate) == 32);
const _: () = assert!(core::mem::offset_of!(NativeStartupMailbox, destination_history) == 40);

impl NativeStartupMailbox {
    pub const fn new(identity: u32) -> Self {
        Self {
            identity,
            reserved: 0,
            queue: AtomicU64::new(0),
            ready: AtomicU64::new(0),
            destination: AtomicU64::new(0),
            route_gate: AtomicU64::new(0),
            destination_history: AtomicU64::new(0),
        }
    }
    pub const fn identity(&self) -> u32 {
        self.identity
    }

    // Only the route owner calls this while holding the shared guard. No
    // remote hardware reads and no sampling after releasing that guard.
    fn route_recipient(&self) -> NativeRouteRecipient {
        let history = self.destination_history.load(Ordering::Relaxed);
        NativeRouteRecipient { identity: self.identity(), mode: self.destination_mode().ok(),
            init_count: (history >> 2) as u32,
            cause: match history & 3 { 1 => NativeDestinationCause::GuestControl,
                2 => NativeDestinationCause::GuestInit, _ => NativeDestinationCause::Observed } }
    }

    /// An observation only. Source routing additionally holds the shared guard
    /// until queue publication; a bare acquire load is not a routing lease.
    pub fn destination_mode(&self) -> Result<NativeDestinationMode, NativeIcrError> {
        NativeDestinationMode::decode(self.destination.load(Ordering::Acquire))
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) != 0
    }

    /// Sole target publishes only after actual captured guest entry/ACK and
    /// successful admission of its host kick, stopped reset and retained state.
    pub fn mark_running(&self) {
        // Transport-only callers use the required x2APIC destination profile.
        // Native runtimes must explicitly prepare/commit their admitted mode
        // under try_lock_routes before publishing their actual guest ACK.
        // Only the sole target initializes an unknown mode, before readiness;
        // source routing cannot pass its all-ready check during this interval.
        let _ = self.destination.compare_exchange(
            0,
            NativeDestinationMode::X2Apic as u64,
            Ordering::Release,
            Ordering::Relaxed,
        );
        self.ready.store(1, Ordering::Release);
    }

    pub fn publish(&self, command: NativeStartupCommand) -> Result<(), NativeIcrError> {
        if self.ready.load(Ordering::Acquire) == 0 {
            return Err(NativeIcrError::MailboxNotReady);
        }
        let mut queue = self.queue.load(Ordering::Acquire);
        // A competing producer or consumer can cause bounded refusal. The FIFO
        // adds no lock of its own; native source routing holds its separate
        // guard. Refusal never changes the queue or the instruction.
        for _ in 0..4 {
            let entries = (0..4)
                .find(|index| queue >> (index * 16) & 0xffff == 0)
                .ok_or(NativeIcrError::MailboxBusy)?;
            let next = queue | (u64::from(command.encode()) << (entries * 16));
            match self
                .queue
                .compare_exchange(queue, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(current) => queue = current,
            }
        }
        Err(NativeIcrError::MailboxBusy)
    }

    /// Sole target peeks before all fallible CPU/LAPIC preparation. The command
    /// remains present on failure, preserving stopped evidence and FIFO order.
    pub fn peek(&self) -> Option<NativeStartupCommand> {
        let head = self.queue.load(Ordering::Acquire) as u16;
        (head != 0).then(|| NativeStartupCommand::decode(head))
    }

    /// Sole destination completes exactly the peeked command, after applying
    /// it locally. Other CPUs may only append: at most three successful appends
    /// can race this removal, so four CAS attempts suffice without an ABA loss.
    pub fn complete(&self, command: NativeStartupCommand) -> Result<(), NativeIcrError> {
        let mut queue = self.queue.load(Ordering::Acquire);
        for _ in 0..4 {
            if queue as u16 != command.encode() {
                return Err(NativeIcrError::MailboxMismatch);
            }
            match self.queue.compare_exchange(
                queue,
                queue >> 16,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(current) => queue = current,
            }
        }
        Err(NativeIcrError::MailboxMismatch)
    }
}

/// One exclusively stopped native target. The runtime separately owns LAPIC
/// INIT effects (APM2 Table16-2), pending physical events, live DR0-3/xstate,
/// EFER logical shadow, and ASID/flush discipline. It preflights those
/// resources before `apply_x2avic`, then commits their infallible effects
/// before acknowledging the mailbox. It must never enter this guest in
/// AwaitSipi.
/// APM2 Table14-1/2 and 15.27.8 define CPU INIT state and real16 SIPI addressing.
pub struct NativeStartupTarget<'a> {
    pub vmcb: &'a mut Vmcb,
    pub frame: &'a mut GuestRegisters,
    pub state: &'a mut NativeStartupState,
    pub signature: u32,
}

impl NativeStartupTarget<'_> {
    /// The exact armed x2AVIC control/page contract and no pending event.
    /// Read-only; SIPI outside AwaitSipi is ignored, one start per INIT
    /// (MPspec 1.4 B.4.2 cross-check; APM2 15.27.8 defines the address and
    /// mode but no duplicate-SIPI rule). Never restart a running AP.
    pub fn validate_x2avic(
        &self, command: NativeStartupCommand, profile: &NativeX2AvicProfile,
    ) -> Result<NativeStartupEffect, NativeIcrError> {
        self.vmcb.validate_external_interrupt_conflicts().map_err(NativeIcrError::PendingState)?;
        self.vmcb.validate_native_x2avic(profile).map_err(NativeIcrError::PendingState)?;
        Ok(match command {
            NativeStartupCommand::Init => NativeStartupEffect::Init,
            NativeStartupCommand::Sipi(_) if *self.state == NativeStartupState::AwaitSipi => NativeStartupEffect::Started,
            NativeStartupCommand::Sipi(_) => NativeStartupEffect::Ignored,
        })
    }

    /// CPU-state commit only. For INIT, the runtime first commits the LAPIC
    /// side (`registers::commit_init`: physical timer/LVT reset, level-source
    /// retirement, EOI acceleration and the backing-page reset). Racing remote
    /// IRR publication does not require a blanket fabric drain when page/table
    /// identity remains stable; it follows the backing reset's per-bank
    /// ordering. The INIT commit leaves V_TPR 0, matching the reset backing
    /// TPR, and clears every VMCB clean bit (`Vmcb::initialize_ap_after_init`).
    pub fn apply_x2avic(
        &mut self, command: NativeStartupCommand, profile: &NativeX2AvicProfile,
    ) -> Result<NativeStartupEffect, NativeIcrError> {
        let effect = self.validate_x2avic(command, profile)?;
        self.commit_effect(command, effect);
        Ok(effect)
    }

    fn commit_effect(&mut self, command: NativeStartupCommand, effect: NativeStartupEffect) {
        match effect {
            NativeStartupEffect::Init => {
                self.vmcb.initialize_ap_after_init();
                *self.frame = GuestRegisters {
                    rdx: u64::from(self.signature),
                    ..GuestRegisters::default()
                };
                *self.state = NativeStartupState::AwaitSipi;
            }
            NativeStartupEffect::Started => {
                let NativeStartupCommand::Sipi(vector) = command else {
                    unreachable!()
                };
                self.vmcb.start_ap_from_sipi(vector);
                *self.state = NativeStartupState::Running;
            }
            NativeStartupEffect::Ignored => {}
        }
    }
}
