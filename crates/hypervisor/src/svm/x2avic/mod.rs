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

pub(crate) use crate::svm::x2avic::profile::logical_x2apic_id;
pub use crate::svm::x2avic::{
    backing::BackingPage,
    exit::AvicExit,
    profile::{
        ENABLE_BITS, Error, GUEST_APIC_VERSION, MAX_ID, NATIVE_CONTROL, NativeX2AvicProfile,
        V_NMI_ENABLE, X2AvicCapabilities,
    },
    table::PhysicalIdTable,
};

mod backing;
mod exit;
pub mod ipi;
pub mod irq;
mod profile;
pub mod registers;
pub mod startup;
mod table;
