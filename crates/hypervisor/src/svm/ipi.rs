//! Native xAPIC/x2APIC startup ownership and bounded two-vCPU fixture ICR routing.
//!
//! APM2 rev3.44 14.1.3/Table14-1/2, 15.27.8, 16.5/Table16-4, 16.10,
//! 16.13. The target borrows the existing sole APIC, VMCB and GPR owners.
//! Stopped startup and atomic transport share the checked ICR adapters. Only
//! the destination CPU mutates its VMCB and the sole LocalApic IRR/ISR owner.
//! Cold startup is deliberately distinct from reinitializing a running CPU.
//! The ordinary native profile refuses guest startup/reset. An explicit
//! diagnostic profile adds retained FIFO transport, target-owned INIT/SIPI,
//! and an ICR readback under a separate INIT-to-#SX notification contract.
//! Neither native profile uses the fixture's synthetic APIC controller.
use super::{
    events::ExternalInterruptError,
    vmcb::Vmcb,
    x2apic::{ApicMode, FixtureApic, QueueError},
};
use crate::arch::x86_64::registers::GuestRegisters;
use core::sync::atomic::{AtomicU64, Ordering};

/// APM2 rev3.44 16.9/Table16-5: CPUID.1:ECX[21] gates x2APIC,
/// not legacy xAPIC. Enabled/base/ownership checks are separate admission.
pub const fn native_apic_mode_supported(cpuid1_ecx: u32, selected_x2: bool) -> bool {
    !selected_x2 || cpuid1_ecx & (1 << 21) != 0
}

/// Native processor assignments with this CPU's captured runnable continuation. This is
/// deliberately separate from the fixture's never-entered `Cold` AP. It owns
/// startup refusal by default; `enable_startup` admits a distinct diagnostic
/// guest-reset profile with explicit runtime resource ownership.
/// Native CPUs retain their enabled physical bus, with APIC_BASE/ICR writes
/// intercepted before these continuations run. xAPIC additionally traps MMIO. External INIT sources remain unsupported.
pub struct NativeIcr {
    source: u32,
    assigned: [u32; 32],
    count: usize,
    startup_overlay: Option<u64>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeRoutePredicate {
    DestinationForm = 1, SelfDestination = 2, DestinationUnassigned = 3,
    RecipientNotReady = 4, RecipientModeInvalid = 5, Broadcast = 6,
    ForeignMatch = 7, DuplicateMatch = 8, NoMatch = 9, SelectedSelf = 10,
    MailboxMismatch = 11, QueueBusy = 12, RouteBusy = 13, InitVector = 14,
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
pub enum NativeDestinationCause { Observed = 0, GuestControl = 1, GuestInit = 2, GuestPromotion = 3 }

#[derive(Debug, PartialEq, Eq)]
pub enum NativeIcrError {
    InvalidTopology,
    Instruction(super::exit::ResumeError),
    PendingState(ExternalInterruptError),
    Fault(super::events::MsrFaultError),
    UnsupportedMode,
    UnsupportedDebugState,
    UnsupportedMsr,
    UnsupportedRegister,
    ApicBaseChange,
    MailboxBusy,
    MailboxNotReady,
    MailboxMismatch,
    RoutingBusy,
    UnsupportedStartupEncoding,
    OverlayNotEnabled,
    OverlayAlreadyEnabled,
    /// The named captured guest is Running; its reset state is not owned.
    RunningStartup {
        destination: u32,
        command: u8,
    },
    /// Remote resident assignment; preparation alone does not prove entry.
    AssignedStartup {
        destination: u32,
        command: u8,
    },
    /// Unsupported logical, self, explicit broadcast, or unadmitted startup
    /// destinations. The native startup owner supports all-excluding-self.
    UnownedStartup {
        value: u64,
    },
}

impl NativeIcr {
    /// Bind the actual immutable native inventory, never firmware ordinal IDs.
    /// No guest CPU is relabeled Cold and no remote CPU state is borrowed.
    pub fn admit(source: u32, ids: &[u32]) -> Result<Self, NativeIcrError> {
        if ids.is_empty() || ids.len() > 32 || !ids.contains(&source) || ids.contains(&u32::MAX) {
            return Err(NativeIcrError::InvalidTopology);
        }
        let mut assigned = [0; 32];
        for (index, id) in ids.iter().enumerate() {
            if ids[..index].contains(id) {
                return Err(NativeIcrError::InvalidTopology);
            }
            assigned[index] = *id;
        }
        Ok(Self {
            source,
            assigned,
            count: ids.len(),
            startup_overlay: None,
            route_failure: None,
        })
    }

    pub fn route_failure(&self) -> Option<NativeRouteFailure> { self.route_failure }

    pub fn clear_route_failure(&mut self) { self.route_failure = None; }

    fn reject_route(&mut self, value: u64, predicate: NativeRoutePredicate,
        recipient: Option<NativeRouteRecipient>, error: NativeIcrError) -> NativeIcrError
    {
        self.route_failure = Some(NativeRouteFailure { value, source: self.source, predicate, recipient });
        error
    }

    fn admit_write(&self, value: u64) -> Result<(), NativeIcrError> {
        let command = ((value >> 8) & 7) as u8;
        if !matches!(command, 5 | 6) {
            // Fixed, SMI and NMI keep their native physical/logical/shorthand
            // semantics, including ordinary native interrupt prioritization.
            return Ok(());
        }
        let shorthand = (value >> 18) & 3;
        let destination = if shorthand == 1 {
            self.source
        } else {
            (value >> 32) as u32
        };
        if (shorthand == 1 || (shorthand == 0 && value & (1 << 11) == 0))
            && self.assigned[..self.count].contains(&destination)
        {
            return Err(if destination == self.source {
                NativeIcrError::RunningStartup {
                    destination,
                    command,
                }
            } else {
                NativeIcrError::AssignedStartup {
                    destination,
                    command,
                }
            });
        }
        Err(NativeIcrError::UnownedStartup { value })
    }
}

/// Complete only native ICR/APIC_BASE WRMSR with exact owned instruction bytes.
/// APM2 rev3.44 15.7, 15.11, 15.21.5, 16.11.3 and 16.13. `apic_base` is the
/// owning CPU's admitted unchanged hardware MSR (x2APIC enabled). The callback
/// writes ICR on this same CPU, once, without faults, allocation or firmware.
/// It is invoked only after every fallible stopped-state and command check.
/// Startup refusal leaves all registers, RIP, flags and physical APIC unchanged;
/// it never sends physical INIT, consumes a pending INIT, or advances past it.
/// Reserved x2APIC bits and CPL violations prepare #GP at the original RIP.
/// Caller must account for EVENTINJ after actual entry. No reset/offline/rebind
/// or xAPIC mode change is supported; ordinary ICR reads remain direct native.
pub fn handle_native_x2apic_write(
    owner: &NativeIcr,
    apic_base: u64,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
    instruction: &[u8],
    physical_write: impl FnOnce(u64),
) -> Result<super::dispatch::NativeMsrOutcome, NativeIcrError> {
    use super::dispatch::NativeMsrOutcome;
    use NativeIcrError as E;
    let Some(access) = prepare_native_apic_access(apic_base, vmcb, frame, instruction, false, false)?
    else {
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    };
    let value = access.value;
    let next = access.next;
    if frame.rcx as u32 == 0x1b {
        if value != apic_base {
            return Err(E::ApicBaseChange);
        }
        // Writing the retained value completes without changing hardware mode.
    } else {
        owner.admit_write(value)?;
        physical_write(value);
    }
    vmcb.commit_emulated_instruction(vmcb.guest_rax(), next);
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

struct NativeApicAccess {
    index: u32,
    write: bool,
    value: u64,
    next: super::exit::ResumeCandidate,
}

/// Shared stopped-instruction admission for ordinary and opt-in native APIC
/// owners. All permission/fault/RIP checks precede any mailbox or hardware write.
fn prepare_native_apic_access(
    apic_base: u64,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
    instruction: &[u8],
    startup: bool,
    presentation: bool,
) -> Result<Option<NativeApicAccess>, NativeIcrError> {
    use NativeIcrError as E;
    let exit = vmcb.exit_snapshot();
    exit.validate_msr_instruction(instruction)
        .map_err(E::Instruction)?;
    let index = frame.rcx as u32;
    let write = exit.info1 == 1;
    if if presentation { !native_guest_apic_msr(index) } else {
        !matches!(index, 0x1b | 0x830) || (!startup && !write) || (!write && index == 0x1b)
    } {
        return Err(E::UnsupportedMsr);
    }
    vmcb.validate_external_interrupt_conflicts()
        .map_err(E::PendingState)?;
    vmcb.validate_virtual_interrupt_controls()
        .map_err(E::PendingState)?;
    let controls = 0xf;
    if vmcb.virtual_interrupt_control() & !controls != 0 {
        return Err(E::PendingState(ExternalInterruptError::ControlMismatch));
    }
    if !presentation && (apic_base & 0x800 == 0 || !vmcb.guest_in_64_bit_code()) {
        return Err(E::UnsupportedMode);
    }
    let value = ((frame.rdx as u32 as u64) << 32) | vmcb.guest_rax() as u32 as u64;
    if vmcb.bytes()[0x4cb] != 0
        || (index == 0x830 && apic_base & 0xc00 != 0xc00)
        || (write && index == 0x830 && validate_x2apic_bits(value).is_err())
        || (presentation && (index != 0x803 || write || apic_base & 0xc00 != 0xc00))
    {
        vmcb.queue_msr_general_protection(instruction)
            .map_err(E::Fault)?;
        return Ok(None);
    }
    if presentation && !super::dispatch::native_startup_instruction_mode(vmcb, instruction.len()) {
        return Err(E::UnsupportedMode);
    }
    if vmcb.guest_rflags() & (1 << 8) != 0 {
        return Err(E::UnsupportedDebugState);
    }
    let next = exit.msr_continuation(instruction).map_err(E::Instruction)?;
    Ok(Some(NativeApicAccess {
        index,
        write,
        value,
        next,
    }))
}

/// Guest-visible version and the unadvertised AMD extended APIC space.
/// APM2 rev3.44 16.11/Table16-6 maps each offset to 800h + offset/16.
pub const fn native_guest_apic_msr(index: u32) -> bool {
    matches!(index, 0x803 | 0x840..=0x842 | 0x848..=0x853)
}

/// Present a conventional LAPIC version and fault accesses to absent extension
/// MSRs. APM2 rev3.44 16.11.1: reserved/read-only writes and unavailable x2APIC
/// accesses raise #GP. PPR57896 APIC030 bit31 advertises the hidden space.
/// All stopped-state checks precede the sole version read; no hardware write
/// is possible. A prepared fault retains RIP/GPRs; a legal read zero-extends
/// EDX:EAX and completes only the decoded RDMSR. The callback must not fault,
/// allocate or call firmware, and reads MSR803 on this owning x2APIC CPU.
pub fn handle_native_guest_apic_msr(
    apic_base: u64,
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    physical_read: impl FnOnce(u32) -> u64,
) -> Result<super::dispatch::NativeMsrOutcome, NativeIcrError> {
    use super::dispatch::NativeMsrOutcome;
    let Some(access) = prepare_native_apic_access(apic_base, vmcb, frame, instruction, true, true)? else {
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    };
    let value = physical_read(access.index) & !(1 << 31);
    frame.rdx = value >> 32;
    vmcb.commit_emulated_instruction(value as u32 as u64, access.next);
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

impl NativeIcr {
    /// Explicit native startup profile: each ready destination owns INIT
    /// interception and host R_INIT-to-#SX acknowledgment. Ordinary physical
    /// IRQs, SVR and TPR stay native; only ICR readback hides host notifications.
    /// Applying guest INIT still requires separate physical reset quiescence.
    pub fn enable_startup(&mut self, icr: u64) -> Result<(), NativeIcrError> {
        if self.startup_overlay.is_some() {
            return Err(NativeIcrError::OverlayAlreadyEnabled);
        }
        self.startup_overlay = Some(icr & !0x0003_1000);
        Ok(())
    }

    /// Target-only shadow commit after INIT resource preflight.
    /// APM2 Table16-2: guest ICR=0. Physical INIT notifications remain hidden;
    /// the runtime resets real SVR to FFh, without a separate SVR overlay.
    pub fn reset_after_init(&mut self) -> Result<(), NativeIcrError> {
        if self.startup_overlay.is_none() {
            return Err(NativeIcrError::OverlayNotEnabled);
        }
        self.startup_overlay = Some(0);
        Ok(())
    }
}

/// Opt-in diagnostic native INIT/SIPI route. APM2 14.1.3, 15.27.8, 16.5,
/// Table16-4 and 16.13. Explicit physical remote assignments and shorthand
/// all-excluding-self are admitted; other destinations stay stopped. Publication is the last
/// fallible step: exactly one owned INIT notification follows, then completion.
/// Every assignment must already own R_INIT and the private #SX gate because
/// the runtime's private notification broadcasts to all except the source. Neither
/// callback may fail or allocate. `physical_write` receives only ordinary ICR
/// writes, never INIT/SIPI. ICR readback preserves the completed guest value.
/// All other LAPIC registers, including TPR/SVR/EOI, retain native execution.
/// AwaitSipi targets remain ready for
/// mailbox traffic without being mislabeled Running. Repeated INIT is permitted.
/// Physical IRQ delivery and guest-reset quiescence belong to the runtime,
/// not this pure CPU/transport owner. Notification counts may coalesce.
#[allow(clippy::too_many_arguments)]
pub fn handle_native_x2apic_startup_access(
    owner: &mut NativeIcr,
    apic_base: u64,
    mailboxes: &[NativeStartupMailbox],
    vmcb: &mut Vmcb,
    frame: &mut GuestRegisters,
    instruction: &[u8],
    physical_write: impl FnOnce(u32, u64),
    kick: impl FnOnce(u32),
) -> Result<super::dispatch::NativeMsrOutcome, NativeIcrError> {
    use super::dispatch::NativeMsrOutcome;
    use NativeIcrError as E;
    owner.clear_route_failure();
    let Some(icr) = owner.startup_overlay else {
        return Err(E::OverlayNotEnabled);
    };
    let Some(access) = prepare_native_apic_access(apic_base, vmcb, frame, instruction, true, false)?
    else {
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    };
    let mut result_rax = vmcb.guest_rax();
    if !access.write {
        let value = icr;
        result_rax = value as u32 as u64;
        frame.rdx = value >> 32;
    } else if access.index == 0x1b {
        if access.value != apic_base {
            return Err(E::ApicBaseChange);
        }
    } else {
        let value = access.value;
        owner.route_startup(value, mailboxes, || physical_write(0x830, value), kick)?;
        owner.startup_overlay = Some(value);
    }
    vmcb.commit_emulated_instruction(result_rax, access.next);
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

/// APIC_BASE mode following for the opt-in native startup owner. APM2 rev3.44
/// 16.9/Table16-5/Figure16-32: preserve enabled xAPIC or promote to x2APIC.
/// Disable/relocation stop before commit because ready mailbox targets must
/// remain physically reachable by INIT. Illegal x2APIC->xAPIC prepares #GP.
/// Hardware and guest mode always agree; native LDR/DFR and lowest-priority
/// semantics are never translated into a different physical bus.
/// `promote` changes local APIC_BASE once then returns actual hardware ICR;
/// no fault/allocation/firmware is allowed. Its upper half supplies the newly
/// initialized destination; the guest's lower ICR shadow survives promotion.
pub fn handle_native_apic_base(
    owner: &mut NativeIcr,
    apic_base: &mut u64,
    vmcb: &mut Vmcb,
    frame: &GuestRegisters,
    instruction: &[u8],
    x2apic_supported: bool,
    promote: impl FnOnce(u64) -> u64,
) -> Result<super::dispatch::NativeMsrOutcome, NativeIcrError> {
    use super::dispatch::NativeMsrOutcome;
    use NativeIcrError as E;
    if owner.startup_overlay.is_none() || frame.rcx as u32 != 0x1b {
        return Err(E::UnsupportedMsr);
    }
    let Some(access) = prepare_native_apic_access(*apic_base, vmcb, frame, instruction, true, false)?
    else {
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    };
    let value = access.value;
    let old_mode = *apic_base & 0xc00;
    let mode = value & 0xc00;
    if mode == 0x400 || (old_mode == 0xc00 && mode == 0x800) {
        vmcb.queue_msr_general_protection(instruction)
            .map_err(E::Fault)?;
        return Ok(NativeMsrOutcome::GeneralProtectionPrepared);
    }
    // The advertised interface is absent. Refuse without a physical MSR or
    // invented architectural fault: firmware may mask the capability bit.
    if mode & 0x400 != 0 && !x2apic_supported {
        return Err(E::UnsupportedMode);
    }
    if value & !0x400 != *apic_base & !0x400 || mode & 0x800 == 0 {
        return Err(E::ApicBaseChange);
    }
    if value != *apic_base {
        let physical_icr = promote(value);
        owner.startup_overlay =
            Some((owner.startup_overlay.unwrap() & 0xffff_ffff) | (physical_icr & !0xffff_ffff));
        *apic_base = value;
    }
    vmcb.commit_emulated_instruction(vmcb.guest_rax(), access.next);
    vmcb.complete_native_instruction_state();
    Ok(NativeMsrOutcome::Completed)
}

impl NativeIcr {
    /// Check the fixed native LAPIC aperture and bus against immutable IDs.
    /// xAPIC physical broadcast ID FFh cannot name an individual target.
    pub fn admit_apic_base(&self, value: u64) -> Result<(), NativeIcrError> {
        if value & !0xd00 != 0xfee0_0000
            || value & 0x800 == 0
            || (value & 0x400 == 0 && self.assigned[..self.count].iter().any(|&id| id >= 255))
        {
            return Err(NativeIcrError::UnsupportedMode);
        }
        Ok(())
    }

    fn route_startup(
        &mut self,
        value: u64,
        mailboxes: &[NativeStartupMailbox],
        physical_write: impl FnOnce(),
        kick: impl FnOnce(u32),
    ) -> Result<(), NativeIcrError> {
        use NativeIcrError as E;
        use NativeRoutePredicate as P;
        self.route_failure = None;
        let command = (value >> 8) & 7;
        if !matches!(command, 5 | 6) {
            physical_write();
            return Ok(());
        }
        let destination = (value >> 32) as u32;
        // APM2 rev3.44 16.5/Table16-4 pp643-644: INIT/SIPI permit all
        // excluding self. Shorthand11 ignores destination and DM. Self and
        // all-including-self are not valid shorthand for these message types.
        let shorthand = (value >> 18) & 3;
        let broadcast = shorthand == 3;
        if !broadcast && (shorthand != 0 || value & (1 << 11) != 0) {
            return Err(self.reject_route(value, P::DestinationForm, None, E::UnownedStartup { value }));
        }
        if !broadcast && destination == self.source {
            return Err(self.reject_route(value, P::SelfDestination, None, E::UnownedStartup { value }));
        }
        if !broadcast && !self.assigned[..self.count].contains(&destination) {
            return Err(self.reject_route(value, P::DestinationUnassigned, None, E::UnownedStartup { value }));
        }
        if command == 5 && value as u8 != 0 {
            return Err(self.reject_route(value, P::InitVector, None, E::UnsupportedStartupEncoding));
        }
        if let Err(error) = self.validate_mailboxes(mailboxes) {
            return Err(self.reject_route(value, P::MailboxMismatch, None, error));
        }
        {
            // The same lock covers every target's physical APIC/control/reset
            // commit. Destination selection and queue publication therefore
            // have one ordering relative to those commits. Release before the
            // private all-excluding-self notification or any target wait.
            let routes = match try_lock_routes(mailboxes) {
                Ok(routes) => routes,
                Err(error) => return Err(self.reject_route(value, P::RouteBusy, None, error)),
            };
            let mut targets = 0u32;
            for (slot, mailbox) in routes.mailboxes.iter().enumerate() {
                if !mailbox.is_ready() {
                    return Err(self.reject_route(value, P::RecipientNotReady,
                        Some(mailbox.route_recipient()), E::MailboxNotReady));
                }
                let mode = match mailbox.destination_mode() {
                    Ok(mode) => mode,
                    Err(error) => return Err(self.reject_route(value, P::RecipientModeInvalid,
                        Some(mailbox.route_recipient()), error)),
                };
                // Guest xAPIC exposes conventional eight-bit IDs. The physical
                // owner must normalize AMD's extension before publishing ready
                // and retain that setting across guest INIT. A narrow physical
                // mode is an ownership invariant failure, not a guest alias.
                if mode == NativeDestinationMode::ExtendedXApic4 {
                    return Err(self.reject_route(value, P::RecipientModeInvalid,
                        Some(mailbox.route_recipient()), E::UnsupportedMode));
                }
                if !broadcast && mode.is_broadcast(destination) {
                    return Err(self.reject_route(value, P::Broadcast,
                        Some(mailbox.route_recipient()), E::UnownedStartup { value }));
                }
                if if broadcast { mailbox.identity() != self.source } else { mailbox.identity() == destination } {
                    if !broadcast && targets != 0 {
                        return Err(self.reject_route(value, P::DuplicateMatch,
                            Some(mailbox.route_recipient()), E::UnownedStartup { value }));
                    }
                    targets |= 1 << slot;
                }
            }
            // Exact admitted guest identities select only owned mailboxes.
            if targets == 0 && !broadcast {
                return Err(self.reject_route(value, P::NoMatch, None, E::UnownedStartup { value }));
            }
            if command == 5 && value & 0xc000 == 0x8000 {
                // Compatibility completion for legacy INIT deassert. APM2
                // Table16-4 excludes this encoding; PPR57896 rev3 p61 defines
                // its bits but not its effect. Linux v6.19 lapic.c
                // __apic_accept_irq performs no target action, and QEMU
                // 67cd056 apic_deliver returns before CPU INIT. This is not a
                // claim of measured Zen5 silicon behavior. Keep all routing
                // ownership checks, but publish nothing and never kick/reset.
                // The caller still commits checked instruction/ICR readback.
                return Ok(());
            }
            let command = if command == 5 {
                NativeStartupCommand::Init
            } else {
                NativeStartupCommand::Sipi(value as u8)
            };
            // All native producers and target completion hold this same route
            // guard. Preflight every selected FIFO before any publication;
            // a full recipient cannot leave half of a broadcast committed.
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
        kick(if broadcast { u32::MAX } else { destination });
        Ok(())
    }

    fn validate_mailboxes(&self, mailboxes: &[NativeStartupMailbox]) -> Result<(), NativeIcrError> {
        if mailboxes.len() != self.count
            || mailboxes
                .iter()
                .zip(&self.assigned)
                .any(|(slot, id)| slot.identity() != *id)
        {
            return Err(NativeIcrError::MailboxMismatch);
        }
        Ok(())
    }

    /// Register backend after the checked native DWORD MMIO decoder has
    /// admitted stopped instruction, operand, events and continuation. APM2
    /// 16.3/16.5: the two ICR halves share the existing shadow and mailbox;
    /// native register callback retains xAPIC LDR/DFR and device IRQ ownership.
    /// `physical` uses local xAPIC only. ICR low writes receive canonical ICR
    /// (destination in bits63:32), so it must write high then low; all other
    /// accesses receive a DWORD. Callbacks cannot fault/fail after publication.
    /// ID mutation and unsupported/reserved registers stop before callback.
    /// Caller validates enabled xAPIC mode and waits boundedly for idle ICR
    /// before any instruction that might send. No register/RIP is committed here.
    pub fn xapic_access(
        &mut self,
        apic_base: u64,
        mailboxes: &[NativeStartupMailbox],
        offset: u16,
        write: Option<u32>,
        physical: impl FnOnce(u16, Option<u64>) -> u32,
        kick: impl FnOnce(u32),
    ) -> Result<u32, NativeIcrError> {
        use NativeIcrError as E;
        self.route_failure = None;
        if apic_base & 0xc00 != 0x800 {
            return Err(E::UnsupportedMode);
        }
        let icr = self.startup_overlay.ok_or(E::OverlayNotEnabled)?;
        if offset == 0x310 {
            if let Some(value) = write {
                self.startup_overlay = Some((icr & 0xffff_ffff) | (u64::from(value >> 24) << 32));
            }
            return Ok(((icr >> 32) as u32) << 24);
        }
        if offset == 0x300 {
            if let Some(value) = write {
                // Status and reserved bits are not software writable (Fig16-9).
                let value = (icr & !0xffff_ffff) | u64::from(value & 0x000c_cfff);
                self.route_startup(
                    value,
                    mailboxes,
                    || {
                        physical(0x300, Some(value));
                    },
                    kick,
                )?;
                self.startup_overlay = Some(value);
                return Ok(0);
            }
            return Ok(icr as u32 | (physical(0x300, None) & 0x31000));
        }
        let known = matches!(offset, 0x20 | 0x30 | 0x80 | 0x90 | 0xa0 | 0xb0 | 0xc0 | 0xd0 | 0xe0 | 0xf0
            | 0x100..=0x280 | 0x320..=0x390 | 0x3e0);
        if offset & 15 != 0 || !known || (matches!(offset, 0x20 | 0x30) && write.is_some()) {
            return Err(E::UnsupportedRegister);
        }
        if offset == 0x30 {
            // PPR57896 rev3.00 APIC030: bit31 advertises the extension space.
            // Physical APIC410 is host-owned; the guest has no such registers.
            return Ok(physical(offset, None) & !(1 << 31));
        }
        if offset == 0xe0 {
            // APM2 Fig16-21: conventional DFR low28 read as ones. PPR57896
            // APIC0E0 marks the physical field reserved; write model bits only.
            return Ok(physical(offset, write.map(|v| u64::from(v & 0xf000_0000))) | 0x0fff_ffff);
        }
        Ok(physical(offset, write.map(u64::from)))
    }
}

/// Native guest CPU lifecycle. Unlike `StartupState`, no Cold state exists:
/// admission is a captured guest that has already entered its resident runtime.
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

/// Actual physical destination matching on one admitted native LAPIC.
/// PPR57896 rev3.00 p64 APIC410[ExtApicIdEn] defines the two extended
/// xAPIC widths; APM2 rev3.44 16.6.1/16.13 defines ordinary xAPIC/x2APIC.
/// This records real physical state. Four-bit mode is a takeover observation;
/// the runtime normalizes it before guest startup routing becomes available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum NativeDestinationMode {
    XApic = 1,
    ExtendedXApic4 = 2,
    ExtendedXApic8 = 3,
    X2Apic = 4,
}

/// A local extended-xAPIC observation outside the one reviewed PPR profile.
/// `actual` retains the complete observed DWORD, including reserved bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeXApicProfileError {
    Signature { actual: u32 },
    Version { actual: u32 },
    Feature { actual: u32 },
    ReservedControl { actual: u32 },
}

/// Admit the observed local layout and actual destination width without writes.
/// PPR57896 rev3.00 (Family1Ah Model44h B0), APIC030/400/410 pp56/64:
/// bit2 selects eight-bit versus four-bit destination matching; bits31:3
/// are reserved. This does not establish immutable CPU identity or inventory
/// ownership, which remain separate admission requirements.
///
/// APM2 rev3.44 16.5/Table16-4: physical bootstrap all-excluding-self does
/// not depend on individual destination IDs. Later guest INIT/SIPI use
/// exact guest identity routing. The physical owner separately normalizes the
/// observed four-bit mode before ordinary guest IPIs can execute.
pub fn admit_native_xapic_extended_profile(
    signature: u32,
    version: u32,
    feature: u32,
    control: u32,
) -> Result<NativeDestinationMode, NativeXApicProfileError> {
    use NativeXApicProfileError as E;
    if signature != 0x00b4_0f40 {
        return Err(E::Signature { actual: signature });
    }
    if version != 0x8105_0010 {
        return Err(E::Version { actual: version });
    }
    if feature != 0x0004_0007 {
        return Err(E::Feature { actual: feature });
    }
    if control & !7 != 0 {
        return Err(E::ReservedControl { actual: control });
    }
    Ok(if control & 4 == 0 {
        NativeDestinationMode::ExtendedXApic4
    } else {
        NativeDestinationMode::ExtendedXApic8
    })
}

impl NativeDestinationMode {
    fn decode(raw: u64) -> Result<Self, NativeIcrError> {
        match raw {
            1 => Ok(Self::XApic),
            2 => Ok(Self::ExtendedXApic4),
            3 => Ok(Self::ExtendedXApic8),
            4 => Ok(Self::X2Apic),
            0 => Err(NativeIcrError::MailboxNotReady),
            _ => Err(NativeIcrError::InvalidTopology),
        }
    }

    fn is_broadcast(self, destination: u32) -> bool {
        match self {
            Self::ExtendedXApic4 => destination & 15 == 15,
            Self::XApic | Self::ExtendedXApic8 => destination & 255 == 255,
            Self::X2Apic => destination == u32::MAX,
        }
    }

}

/// Bounded global routing exclusion in the first mailbox's retained padding.
/// Every CPU must pass the same complete ordered mailbox slice: sub-slices or
/// aliases with a different first entry are not interchangeable lock domains.
/// Targets hold this across physical APIC mode/control/reset commits and their
/// matching metadata publication. Sources hold it across destination selection
/// and FIFO publication. Neither side holds it while notifying, waiting for a
/// target, calling firmware, allocating, or resuming a guest.
pub struct NativeRouteGuard<'a> {
    mailboxes: &'a [NativeStartupMailbox],
    gate: &'a AtomicU64,
    // A CPU-local hardware commit must not move its guard to another thread.
    _local: core::marker::PhantomData<*mut ()>,
}

/// Acquire the one routing lock, with a bounded contention refusal before any
/// guest or hardware mutation. APM2 15.28/16.5 and the coherent retained shared
/// memory contract of NativeStartupMailbox. Readiness is checked by the source
/// while holding the lock; target initialization may acquire it before ACK.
pub fn try_lock_routes(
    mailboxes: &[NativeStartupMailbox],
) -> Result<NativeRouteGuard<'_>, NativeIcrError> {
    if mailboxes.is_empty() || mailboxes.len() > 32 {
        return Err(NativeIcrError::MailboxMismatch);
    }
    let gate = &mailboxes
        .first()
        .ok_or(NativeIcrError::MailboxMismatch)?
        .route_gate;
    for _ in 0..64 {
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
/// target prepares this after all other fallible checks. It performs the
/// infallible physical write/reset, then consumes this token before releasing
/// the guard, completing its command or resuming the guest. Dropping an unused
/// token is permitted only when no corresponding hardware change occurred.
#[must_use]
pub struct NativeDestinationCommit<'a> {
    destination: &'a AtomicU64,
    history: &'a AtomicU64,
    mode: NativeDestinationMode,
    _guard: core::marker::PhantomData<&'a NativeRouteGuard<'a>>,
}

impl NativeRouteGuard<'_> {
    pub fn prepare_destination_mode(
        &self,
        slot: usize,
        mode: NativeDestinationMode,
    ) -> Result<NativeDestinationCommit<'_>, NativeIcrError> {
        let mailbox = self
            .mailboxes
            .get(slot)
            .ok_or(NativeIcrError::MailboxMismatch)?;
        if mailbox.identity() == u32::MAX
            || (mode != NativeDestinationMode::X2Apic && mailbox.identity() >= 255)
        {
            return Err(NativeIcrError::InvalidTopology);
        }
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
    /// a guest control write, promotion, or the initial hardware observation.
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
                2 => NativeDestinationCause::GuestInit, 3 => NativeDestinationCause::GuestPromotion,
                _ => NativeDestinationCause::Observed } }
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
        // Legacy pure transport fixtures assume ordinary eight-bit matching.
        // Native runtimes must explicitly prepare/commit their observed mode
        // under try_lock_routes before publishing their actual guest ACK.
        // Only the sole target initializes an unknown mode, before readiness;
        // source routing cannot pass its all-ready check during this interval.
        let _ = self.destination.compare_exchange(
            0,
            NativeDestinationMode::XApic as u64,
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
/// EFER logical shadow, and ASID/flush discipline. It preflights those resources
/// before `apply`, then commits their infallible effects before acknowledging
/// the mailbox. It must never enter this guest in AwaitSipi.
/// APM2 Table14-1/2 and 15.27.8 define CPU INIT state and real16 SIPI addressing.
pub struct NativeStartupTarget<'a> {
    pub vmcb: &'a mut Vmcb,
    pub frame: &'a mut GuestRegisters,
    pub state: &'a mut NativeStartupState,
    pub signature: u32,
}

impl NativeStartupTarget<'_> {
    pub fn validate(
        &self,
        command: NativeStartupCommand,
    ) -> Result<NativeStartupEffect, NativeIcrError> {
        let effect = match command {
            NativeStartupCommand::Init => NativeStartupEffect::Init,
            NativeStartupCommand::Sipi(_) if *self.state == NativeStartupState::AwaitSipi => {
                NativeStartupEffect::Started
            }
            // One start per INIT: MPspec1.4 B.4.2 cross-check, as in the cold
            // fixture owner. APM2 15.27.8 defines the address/mode, without an
            // explicit duplicate-SIPI rule. Never restart an already running AP.
            NativeStartupCommand::Sipi(_) => NativeStartupEffect::Ignored,
        };
        if effect == NativeStartupEffect::Ignored {
            return Ok(effect);
        }
        self.vmcb
            .validate_external_interrupt_conflicts()
            .map_err(NativeIcrError::PendingState)?;
        self.vmcb
            .validate_virtual_interrupt_controls()
            .map_err(NativeIcrError::PendingState)?;
        if self.vmcb.virtual_interrupt_control() & !0xf != 0 {
            return Err(NativeIcrError::PendingState(
                ExternalInterruptError::ControlMismatch,
            ));
        }
        Ok(effect)
    }

    pub fn apply(
        &mut self,
        command: NativeStartupCommand,
    ) -> Result<NativeStartupEffect, NativeIcrError> {
        let effect = self.validate(command)?;
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
        Ok(effect)
    }
}

pub(crate) trait IpiRoute {
    fn deliver(&mut self, source: &FixtureApic, value: u64) -> Result<(), IpiError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupState {
    /// Caller admits a fresh AP which has never executed guest instructions.
    Cold,
    AwaitSipi,
    /// Runnable after SIPI; this is not proof that the AP has executed yet.
    Running,
}

#[derive(Debug, PartialEq, Eq)]
pub enum IpiError {
    ReservedBits,
    UnsupportedDelivery,
    UnsupportedDestination,
    DisabledSource,
    ArmedSource,
    TargetNotRunning,
    UnsupportedInit,
    StartupWithoutInit,
    StartupStateMismatch,
    PendingState(ExternalInterruptError),
    TargetQueue(QueueError),
}

/// Exclusively stopped remote context. The caller binds these fields to the
/// same vCPU for its entire lifetime, never restores `Cold` after entry, and
/// owns separate extended/debug state and ASID/flush discipline per vCPU.
/// `signature` is the admitted guest RESET/INIT family/model/stepping value.
/// The cold profile has zero debug addresses, no pending events,
/// and supplied initialized auxiliary state. INIT retains that auxiliary state;
/// the VMM must not reinitialize x87/SSE/XCR0 on the INIT operation. CR0.CD/NW
/// are retained from the admitted target VMCB; the other CR0 bits become ET.
pub struct IpiTarget<'a> {
    pub apic: &'a mut FixtureApic,
    pub vmcb: &'a mut Vmcb,
    pub frame: &'a mut GuestRegisters,
    pub startup: &'a mut StartupState,
    pub signature: u32,
}

/// x2APIC reserved-field writes require #GP(0), not a routing refusal.
/// Legacy MMIO uses the same bounded mask but refuses unsupported encodings.
pub(crate) fn validate_x2apic_bits(value: u64) -> Result<(), IpiError> {
    // VEC, MT, DM, Level, Trigger, Shorthand. DS and RRS are MBZ in x2APIC.
    if value as u32 & !0x000c_cfff != 0 || matches!((value >> 8) & 7, 1 | 3 | 7) {
        return Err(IpiError::ReservedBits);
    }
    Ok(())
}

impl IpiRoute for IpiTarget<'_> {
    fn deliver(&mut self, source: &FixtureApic, value: u64) -> Result<(), IpiError> {
        if source.mode() == ApicMode::Disabled {
            return Err(IpiError::DisabledSource);
        }
        if source.controller().delivery_armed() {
            return Err(IpiError::ArmedSource);
        }
        validate_destination(source, value, self.apic.identity())?;
        self.apply(value as u32)
    }
}

/// Shared checked routing admission; remote startup never fabricates a source
/// APIC merely to call the stopped execution owner.
fn validate_destination(source: &FixtureApic, value: u64, identity: u32) -> Result<(), IpiError> {
    let low = value as u32;
    if low & ((1 << 11) | (3 << 18)) != 0
        || value >> 32 != u64::from(identity)
        || source.identity() == identity
    {
        return Err(IpiError::UnsupportedDestination);
    }
    if low & !0x0000_47ff != 0 {
        return Err(IpiError::UnsupportedDelivery);
    }
    Ok(())
}

impl IpiTarget<'_> {
    /// Apply a previously admitted command to this owner's stopped state.
    fn apply(&mut self, low: u32) -> Result<(), IpiError> {
        // Level is ignored for edge messages (Table16-4). Edge only means the
        // existing controller needs no TMR or remote-EOI ownership.
        let vector = low as u8;
        match (low >> 8) & 7 {
            0 => {
                if *self.startup != StartupState::Running {
                    return Err(IpiError::TargetNotRunning);
                }
                self.vmcb
                    .validate_external_interrupt_conflicts()
                    .map_err(IpiError::PendingState)?;
                self.vmcb
                    .validate_virtual_interrupt_controls()
                    .map_err(IpiError::PendingState)?;
                if self.vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
                    return Err(IpiError::PendingState(
                        ExternalInterruptError::PendingVirtualInterrupt,
                    ));
                }
                self.apic.queue(vector).map_err(IpiError::TargetQueue)?;
            }
            5 => {
                // Never erase a running AP's retained state or fabricate its
                // architectural INIT reset. Only the admitted fresh AP exists.
                if vector != 0 || self.apic.identity() != 1 || *self.startup != StartupState::Cold {
                    return Err(IpiError::UnsupportedInit);
                }
                self.validate_startup_target()?;
                self.vmcb.initialize_cold_ap();
                *self.frame = GuestRegisters {
                    rdx: self.signature as u64,
                    ..GuestRegisters::default()
                };
                self.apic.initialize_cold_ap();
                *self.startup = StartupState::AwaitSipi;
            }
            6 => {
                if self.apic.identity() != 1 || *self.startup == StartupState::Cold {
                    return Err(IpiError::StartupWithoutInit);
                }
                if *self.startup == StartupState::AwaitSipi {
                    self.validate_startup_target()?;
                    self.vmcb.start_ap_from_sipi(vector);
                    *self.startup = StartupState::Running;
                }
                // Bounded fixture rule cross-checked against Intel MPspec1.4
                // Appendix B.4.2 (one startup per INIT/RESET); AMD15.27.8 gives
                // startup addressing but does not explicitly specify duplicate
                // SIPIs. Do not reset CS:IP or erase running AP register state.
            }
            _ => return Err(IpiError::UnsupportedDelivery),
        }
        Ok(())
    }
}

impl IpiTarget<'_> {
    fn validate_startup_target(&self) -> Result<(), IpiError> {
        self.vmcb
            .validate_external_interrupt_conflicts()
            .map_err(IpiError::PendingState)?;
        self.vmcb
            .validate_virtual_interrupt_controls()
            .map_err(IpiError::PendingState)?;
        if self.vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
            return Err(IpiError::PendingState(
                ExternalInterruptError::PendingVirtualInterrupt,
            ));
        }
        if self.apic.controller().delivery_armed() {
            return Err(IpiError::TargetQueue(QueueError::Controller(
                super::local_apic::Error::Armed,
            )));
        }
        Ok(())
    }
}

/// Bounded cross-CPU request transport, not architectural IRR or ISR.
///
/// One immutable identity names an admitted fixture CPU: `new` binds an
/// already running CPU, while `new_cold_ap` owns a one-time startup admission.
/// Producers never borrow that CPU's VMCB/APIC. Every accepted ICR must be
/// followed by an unconditional host kick, including when its bit coalesces.
/// The receiver drains while stopped and arms locally before VMRUN; a request
/// racing after that drain remains pending with a physical kick. The platform
/// owner must keep INTR interception and host IF enabled at guest entry, then
/// actually acknowledge the host interrupt after restoring host state.
///
/// Acquire/release RMWs retain a concurrent publication on either side of a
/// drain. The four words are not a global snapshot; no arrival ordering between
/// vectors is promised. No reset/offline/rebind is allowed while producers live.
/// AMD APM2 rev3.44 15.13.1, 15.21.1/4, 16.5; software transport policy.
pub struct IpiMailbox {
    identity: u8,
    pending: [AtomicU64; 4],
    startup: AtomicU64,
}

const INIT_ACCEPTED: u64 = 1;
const INIT_APPLIED: u64 = 2;
const SIPI_ACCEPTED: u64 = 4;
const SIPI_APPLIED: u64 = 8;
const STARTUP_DISABLED: u64 = 1 << 63;

impl IpiMailbox {
    pub const fn new(identity: u8) -> Self {
        assert!(identity <= 1);
        Self {
            identity,
            pending: [const { AtomicU64::new(0) }; 4],
            startup: AtomicU64::new(STARTUP_DISABLED),
        }
    }

    /// Immutable identity1 cold AP. Unlike the fixed-only `new`, this admits
    /// exactly one INIT and the first subsequent SIPI. Accepted and applied
    /// flags are separate and monotonic; the first vector cannot be replaced.
    /// INIT/SIPI never use the IRR bitmap (APM2 16.2/16.6.3). No reset/rebind is
    /// supported. Pre-start fixed delivery is a bounded refusal, not a claim
    /// to implement the architectural holding of other events during INIT.
    pub const fn new_cold_ap() -> Self {
        Self {
            identity: 1,
            pending: [const { AtomicU64::new(0) }; 4],
            startup: AtomicU64::new(0),
        }
    }

    /// Destination-owner preparation only. Acceptance is not guest execution.
    /// A single snapshot applies at most INIT then SIPI; later publications
    /// remain pending and require another local drain plus their host kick.
    /// Every fallible preflight precedes any state change or applied flag.
    /// The caller permanently binds this mailbox to the same destination's
    /// VMCB/APIC/frame/startup and auxiliary state, with exactly one destination
    /// owner. A matching numeric identity alone does not establish that binding.
    pub fn drain_startup(&self, target: &mut IpiTarget<'_>) -> Result<usize, IpiError> {
        if target.apic.identity() != u32::from(self.identity) {
            return Err(IpiError::UnsupportedDestination);
        }
        let control = self.startup.load(Ordering::Acquire);
        if control & STARTUP_DISABLED != 0 {
            return Err(IpiError::UnsupportedInit);
        }
        let expected = if control & SIPI_APPLIED != 0 {
            StartupState::Running
        } else if control & INIT_APPLIED != 0 {
            StartupState::AwaitSipi
        } else {
            StartupState::Cold
        };
        if *target.startup != expected {
            return Err(IpiError::StartupStateMismatch);
        }
        let init = control & (INIT_ACCEPTED | INIT_APPLIED) == INIT_ACCEPTED;
        let sipi = control & (SIPI_ACCEPTED | SIPI_APPLIED) == SIPI_ACCEPTED;
        if !init && !sipi {
            return Ok(0);
        }
        target.validate_startup_target()?;
        let mut count = 0;
        if init {
            target.apply(0x500).expect("preflighted cold INIT");
            self.startup.fetch_or(INIT_APPLIED, Ordering::AcqRel);
            count += 1;
        }
        if sipi {
            target
                .apply(0x600 | ((control >> 8) as u8 as u32))
                .expect("preflighted first SIPI");
            self.startup.fetch_or(SIPI_APPLIED, Ordering::Release);
            count += 1;
        }
        Ok(count)
    }

    fn publish_startup(&self, low: u32) -> Result<(), IpiError> {
        if self.startup.load(Ordering::Acquire) & STARTUP_DISABLED != 0 {
            return Err(IpiError::UnsupportedDelivery);
        }
        if (low >> 8) & 7 == 5 {
            if low as u8 != 0 {
                return Err(IpiError::UnsupportedInit);
            }
            return self
                .startup
                .compare_exchange(0, INIT_ACCEPTED, Ordering::AcqRel, Ordering::Acquire)
                .map(|_| ())
                .map_err(|_| IpiError::UnsupportedInit);
        }
        let mut control = self.startup.load(Ordering::Acquire);
        // Before first SIPI publication the owner can change only INIT_APPLIED.
        // A competing producer can only publish the first SIPI. Thus two CAS
        // attempts suffice: no spin lock or unbounded VM-exit retry loop.
        for _ in 0..2 {
            if control & INIT_ACCEPTED == 0 {
                return Err(IpiError::StartupWithoutInit);
            }
            if control & SIPI_ACCEPTED != 0 {
                return Ok(()); // Duplicate still requires an unconditional kick.
            }
            let next = control | SIPI_ACCEPTED | (u64::from(low as u8) << 8);
            match self
                .startup
                .compare_exchange(control, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(actual) => control = actual,
            }
        }
        assert!(
            control & SIPI_ACCEPTED != 0,
            "monotonic startup publication"
        );
        Ok(())
    }

    /// Observation only; a false result cannot justify skipping a host kick.
    pub fn pending(&self) -> bool {
        let control = self.startup.load(Ordering::Acquire);
        control & (INIT_ACCEPTED | INIT_APPLIED) == INIT_ACCEPTED
            || control & (SIPI_ACCEPTED | SIPI_APPLIED) == SIPI_ACCEPTED
            || self
                .pending
                .iter()
                .any(|word| word.load(Ordering::Acquire) != 0)
    }

    /// Transfer requests into the same CPU's existing IRR while it is stopped.
    /// All fallible admission checks precede consuming transport bits. The
    /// caller has settled any real prior entry/exit, including blocked V_IRQ.
    /// Returns the number of transported vectors (including IRR coalescing),
    /// not guest handler completions. Publication may race this bounded drain.
    pub fn drain_into(&self, apic: &mut FixtureApic, vmcb: &Vmcb) -> Result<usize, IpiError> {
        if apic.identity() != u32::from(self.identity) {
            return Err(IpiError::UnsupportedDestination);
        }
        if self.startup.load(Ordering::Acquire) & (STARTUP_DISABLED | SIPI_APPLIED) == 0 {
            return Err(IpiError::TargetNotRunning);
        }
        vmcb.validate_external_interrupt_conflicts()
            .map_err(IpiError::PendingState)?;
        vmcb.validate_virtual_interrupt_controls()
            .map_err(IpiError::PendingState)?;
        if vmcb.virtual_interrupt_control() & (1 << 8) != 0 {
            return Err(IpiError::PendingState(
                ExternalInterruptError::PendingVirtualInterrupt,
            ));
        }
        if apic.mode() == ApicMode::Disabled {
            return Err(IpiError::TargetQueue(QueueError::Disabled));
        }
        if apic.controller().delivery_armed() {
            return Err(IpiError::TargetQueue(QueueError::Controller(
                super::local_apic::Error::Armed,
            )));
        }
        if !apic.controller().software_enabled() {
            return Err(IpiError::TargetQueue(QueueError::Controller(
                super::local_apic::Error::SoftwareDisabled,
            )));
        }
        let mut count = 0;
        for (index, word) in self.pending.iter().enumerate() {
            let mut bits = word.swap(0, Ordering::AcqRel);
            while bits != 0 {
                let bit = bits.trailing_zeros();
                let vector = (index * 64 + bit as usize) as u8;
                // Only checked publication can set a bit. All queue error
                // conditions were preflighted under the local exclusive borrow.
                apic.queue(vector).expect("admitted mailbox queue");
                bits &= bits - 1;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// One source instruction's remote transport endpoint. It carries no mutable
/// destination CPU state. A fresh value lets the caller observe whether the
/// adapter accepted an ICR and consequently must issue a host kick.
pub struct MailboxTarget<'a> {
    mailbox: &'a IpiMailbox,
    published: bool,
}
impl<'a> MailboxTarget<'a> {
    pub const fn new(mailbox: &'a IpiMailbox) -> Self {
        Self {
            mailbox,
            published: false,
        }
    }
    pub const fn published(&self) -> bool {
        self.published
    }
}
impl IpiRoute for MailboxTarget<'_> {
    fn deliver(&mut self, source: &FixtureApic, value: u64) -> Result<(), IpiError> {
        if source.mode() == ApicMode::Disabled {
            return Err(IpiError::DisabledSource);
        }
        if source.controller().delivery_armed() {
            return Err(IpiError::ArmedSource);
        }
        let low = value as u32;
        validate_destination(source, value, u32::from(self.mailbox.identity))?;
        if matches!((low >> 8) & 7, 5 | 6) {
            self.mailbox.publish_startup(low)?;
            self.published = true;
            return Ok(());
        }
        // Only fixed edge physical unicast. Level is ignored for edge messages.
        if low & !0x40ff != 0 {
            return Err(IpiError::UnsupportedDelivery);
        }
        if self.mailbox.startup.load(Ordering::Acquire) & (STARTUP_DISABLED | SIPI_APPLIED) == 0 {
            return Err(IpiError::TargetNotRunning);
        }
        let vector = low as u8;
        super::events::PendingExternalInterrupt::new(vector).map_err(|e| {
            IpiError::TargetQueue(QueueError::Controller(super::local_apic::Error::Interrupt(
                e,
            )))
        })?;
        self.mailbox.pending[vector as usize / 64]
            .fetch_or(1u64 << (vector % 64), Ordering::AcqRel);
        self.published = true;
        Ok(())
    }
}
