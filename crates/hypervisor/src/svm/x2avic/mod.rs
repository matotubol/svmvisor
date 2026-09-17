//! Exclusive ordinary-SVM x2AVIC guest interrupt controller.
//!
//! AMD APM2 3.44 15.29, Tables15-22..29; 16.10 and Table16-2.
//! This namespace does not allocate, map, enable physical x2APIC or establish
//! a safe loader continuation. Hardware effects reach it only through
//! `arch::x86_64::apic` (physical x2APIC registers and the AVIC doorbell).
//! Its owners are:
//!
//! - this module: capability admission, the address-checked entry profile;
//! - `backing`: the per-vCPU hardware backing page;
//! - `table`: the shared physical-ID table;
//! - `exit`: AVIC_INCOMPLETE_IPI/AVIC_NOACCEL decoding;
//! - `registers`: the MSR interception profile, intercepted x2APIC register
//!   and APIC_BASE emulation, the captured initial register interface,
//!   physical timer/LVT mirroring and guest INIT;
//! - `irq`: the host IRQ bridge's physical source/EOI ledger, capture and
//!   software/level EOI completion;
//! - `ipi`: the admitted CPU inventory, AVIC_INCOMPLETE_IPI policy and
//!   software fixed-IPI fan-out;
//! - `startup`: the software INIT/SIPI mailbox transport and CPU-state commit.
use crate::memory::address::{AddressError, AddressPolicy};

mod backing;
mod exit;
pub mod ipi;
pub mod irq;
pub mod registers;
pub mod startup;
mod table;

pub use backing::BackingPage;
pub use exit::AvicExit;
pub use table::PhysicalIdTable;

pub const PAGE_BYTES: usize = 4096;
pub const MAX_ID: u16 = 511;
pub const ENABLE_BITS: u64 = 3 << 30;
/// V_NMI_ENABLE, offset 60h bit 26 (APM2 rev3.44 Table B-1 p740, 15.21.10
/// p536). VMRUN loads V_NMI/V_NMI_MASK from the VMCB when it is set, and
/// enabling it requires the NMI intercept (else #VMEXIT(INVALID)). The armed
/// profile always sets it: guest NMI IPIs (`ipi`) and re-presented platform
/// NMIs are delivered through V_NMI, gated at arm on CPUID Fn8000_000A
/// EDX[VNMI] (bit 25, PPR57896 p101 NmiVirt).
pub const V_NMI_ENABLE: u64 = 1 << 26;
pub const NATIVE_CONTROL: u64 = ENABLE_BITS | (1 << 24) | V_NMI_ENABLE;
/// The only admitted guest APIC version register value: six standard LVTs and
/// no AMD extension exposure (APM2 16.3.4, Table 16-2).
pub const GUEST_APIC_VERSION: u32 = 0x0005_0010;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    MissingCapability,
    Address(AddressError),
    InvalidId,
    AliasedPages,
    Occupied,
    InvalidOffset,
    InvalidVector,
    /// Retired (`BackingPage::enqueue`).
    MixedTrigger,
    UnsupportedVersion,
    InvalidExit,
    /// The captured APIC_BASE is not enabled x2APIC at FEE0_0000h.
    UnsupportedApicBase,
}

/// Logical x2APIC ID of an x2APIC ID: APM2 rev3.44 16.14 p662,
/// `logical_id[15:0] = 1 << x2APIC_ID[3:0]`, `cluster_id[15:0] =
/// x2APIC_ID[19:4]`; x2AVIC derives the same value (15.29.5.3 p574).
pub(crate) const fn logical_x2apic_id(id: u32) -> u32 {
    (((id >> 4) & 0xffff) << 16) | (1 << (id & 15))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X2AvicCapabilities(());
impl X2AvicCapabilities {
    /// Evidence must be sampled on every admitted CPU before activation.
    /// APM2 rev3.44 15.29.7 p640 / PPR57896 p101 Fn8000_000A_EDX: NP (0),
    /// AVIC (13), x2AVIC (18) and NmiVirt/VNMI (25). VNMI is required because
    /// the armed profile always enables V_NMI_ENABLE (guest NMI IPIs and
    /// re-presented platform NMIs); a CPU without it cannot be armed.
    pub fn admit(cpuid1_ecx: u32, svm_edx: u32) -> Result<Self, Error> {
        let required = 1 | (1 << 13) | (1 << 18) | (1 << 25);
        if cpuid1_ecx & (1 << 21) == 0 || svm_edx & required != required {
            return Err(Error::MissingCapability);
        }
        Ok(Self(()))
    }
}

/// Address-checked entry configuration, not allocation or WB/lifetime proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeX2AvicProfile {
    backing: u64,
    table: u64,
    maximum: u16,
}
impl NativeX2AvicProfile {
    pub fn new(
        _capabilities: X2AvicCapabilities,
        backing: u64,
        table: u64,
        maximum: u16,
        policy: &AddressPolicy,
    ) -> Result<Self, Error> {
        if maximum > MAX_ID { return Err(Error::InvalidId); }
        for address in [backing, table] {
            if address == 0 { return Err(Error::Address(AddressError::EmptyRange)); }
            policy.validate(address, PAGE_BYTES as u64, PAGE_BYTES as u64).map_err(Error::Address)?;
        }
        if backing == table { return Err(Error::AliasedPages); }
        Ok(Self { backing, table, maximum })
    }
    pub const fn backing_address(self) -> u64 { self.backing }
    pub const fn table_address(self) -> u64 { self.table }
    pub const fn maximum_id(self) -> u16 { self.maximum }
    pub const fn table_control(self) -> u64 { self.table | self.maximum as u64 }
}
