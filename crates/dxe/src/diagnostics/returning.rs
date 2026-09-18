//! Parent-only, bounded encoding for returning detail 7. NativeResult is unchanged.
//! See docs/native-returning-diagnostics-contract.md for the exported word map.

use crate::{
    delivery::returning::Delivery,
    diagnostics::outcome::{self as returning_outcome, ReturningOutcome},
};

pub const DETAIL: u32 = 7;
pub const ENCODING_OVERFLOW: u32 = 1 << 29;
pub const NON_BOOLEAN_FLAGS: u32 = 1 << 30;
pub const INVALID_INNER_HEADER: u32 = 1 << 31;

/// A value copy taken before lifecycle notifications are armed. No child address
/// or service dependency survives here; classification stays with the existing
/// independent returning_outcome predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReturningDiagnostics {
    result: ReturningOutcome,
    refusal: u32,
    entries_exits: u32,
    metadata: u32,
}

impl ReturningDiagnostics {
    pub fn capture(report: &Delivery) -> Self {
        let inner = report.inner;
        let mut overflow = false;
        let refusal = bounded(inner.refusal, u32::MAX, &mut overflow);
        let entries = bounded(inner.attempted_entries, u16::MAX as u32, &mut overflow);
        let exits = bounded(inner.completed_exits, u16::MAX as u32, &mut overflow);
        let mut metadata = bounded(inner.outcome, 15, &mut overflow)
            | (bounded(report.stage as u64, 7, &mut overflow) << 4);
        let flags = [
            inner.rust_entered,
            inner.rust_completed,
            inner.cleanup_complete,
            inner.restoration_complete,
            inner.canary_called,
            inner.canary_observed,
        ];
        for (index, value) in flags.into_iter().enumerate() {
            // A malformed 2 is neither a true marker nor silently accepted as 0.
            metadata |= ((value == 1) as u32) << (7 + index);
            if value > 1 {
                metadata |= NON_BOOLEAN_FLAGS;
            }
        }
        metadata |= ((inner.canary_failures != 0) as u32) << 13;
        if overflow {
            metadata |= ENCODING_OVERFLOW;
        }
        if !inner.valid_header() {
            metadata |= INVALID_INNER_HEADER;
        }
        Self {
            result: returning_outcome::classify(report),
            refusal,
            entries_exits: entries | (exits << 16),
            metadata,
        }
    }

    pub const fn result_bits(self) -> u32 {
        self.result as u32
    }

    pub fn immediate_record(self, sequence: u32, boot_id: u32, tsc: u64) -> [u32; 8] {
        let words = self.journal_words([0; 3]);
        [
            sequence,
            boot_id,
            tsc as u32,
            (tsc >> 32) as u32,
            words[0],
            words[1],
            words[2],
            ((DETAIL | self.result_bits()) << 16) | 0x10,
        ]
    }

    /// Journal words 4/5/6, exported by the existing RTL as snapshot 10/11/9.
    /// A set overflow bit invalidates exact interpretation of the numeric bundle;
    /// saturated values must never be presented as exact values/refusal names.
    pub fn journal_words(self, lifecycle_counts: [u32; 3]) -> [u32; 3] {
        let mut metadata = self.metadata;
        let mut overflow = false;
        for (index, count) in lifecycle_counts.into_iter().enumerate() {
            metadata |= bounded(count as u64, 31, &mut overflow) << (14 + index * 5);
        }
        if overflow {
            metadata |= ENCODING_OVERFLOW;
        }
        [self.refusal, self.entries_exits, metadata]
    }
}

fn bounded(value: u64, maximum: u32, overflow: &mut bool) -> u32 {
    if value > maximum as u64 {
        *overflow = true;
        maximum
    } else {
        value as u32
    }
}
