//! The native raw resident payload uses the existing checked package format.
//!
//! Share only the firmware-independent parser/relocator, not the emulator's
//! opt-in protocol, ExitBootServices owner, or execution policy. This raw image
//! is never registered as a loaded runtime PE: UEFI 2.11 8.4.1 automatic loaded
//! image relocation must not rewrite the monitor's physical host pointers.

#[path = "../../../../firmware-handoff/src/layout.rs"]
mod layout;

pub use layout::{ARENA_BYTES, HANDOFF_OFFSET, LayoutError, Payload, valid_arena};
