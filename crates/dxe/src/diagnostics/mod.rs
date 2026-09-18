//! Journal encoding, lifecycle traces, and native child result classification.

pub mod journal;
#[cfg(any(
    feature = "card-returning-loader",
    feature = "card-resident",
    feature = "native-preflight"
))]
pub mod native_result;
#[cfg(feature = "card-returning-loader")]
pub mod outcome;
#[cfg(feature = "card-returning-loader")]
pub mod returning;
pub mod trace;

pub mod resident_boot;
