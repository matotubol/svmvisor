//! Native resident preparation: allocation, delivery, launch admission, MP
//! inventory and the inert ReadyToBoot callback continuation.

pub use crate::native::resident::callback::{
    CallbackError, CallbackPrepared, CallbackRequest, CallbackSites, GuestStackSpan,
    prepare_callback,
};

pub mod allocation;
pub mod bootstrap_paging;
pub mod bridge;
mod callback;
pub mod delivery;
pub mod launch;
pub mod memory;
pub mod physical;
pub mod processors;
