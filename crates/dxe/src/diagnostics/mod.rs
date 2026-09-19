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
pub mod resident_boot;
#[cfg(feature = "card-returning-loader")]
pub mod returning_detail;
pub mod trace;
