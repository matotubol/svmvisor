//! Resident host: the bridge ABI shared with DXE and the resident assembly
//! (`abi`), stopped-guest instruction fetch (`fetch`), terminal evidence
//! (`terminal`) and the per-CPU runtime linked into the resident payload
//! (`runtime`).

#[cfg(feature = "resident-runtime")]
pub use crate::host::resident::runtime::prepare;
pub use crate::{
    guest::continuation::NATIVE_BOOTSTRAP_ACK as BOOTSTRAP_ACK,
    host::resident::abi::{
        ArmRuntime, BridgeContext, CACHE_CAPTURE_OFFSET, CACHE_OWNER_OFFSET, DIRECTORY_VERSION,
        Dispatch, Enter, MAX_RESIDENT_CPUS, PrepareRuntime, ResidentDirectory, STARTUP_PAGE_OFFSET,
        TAKEOVER_CAPTURED_REGISTER, TAKEOVER_TAG, X2AVIC_BACKING_ALIASES_OFFSET,
        X2AVIC_TABLE_OFFSET, captured_register_refusal, svmvisor_resident_callback,
        svmvisor_resident_guest_ack, svmvisor_resident_guest_after_ack,
        svmvisor_resident_guest_resume, valid_pool_slot,
    },
};

mod abi;
pub mod fetch;
#[cfg(feature = "resident-runtime")]
mod runtime;
pub mod terminal;
