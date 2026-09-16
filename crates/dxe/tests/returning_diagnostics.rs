#![cfg(feature = "card-returning-loader")]
use svmvisor_dxe::{
    delivery::returning::Delivery,
    diagnostics::native_result::NativeResult,
    diagnostics::returning::{
        ENCODING_OVERFLOW, INVALID_INNER_HEADER, NON_BOOLEAN_FLAGS, ReturningDiagnostics,
    },
    diagnostics::outcome::classify,
};
use uefi_raw::Status;

fn delivered(inner: NativeResult) -> Delivery {
    Delivery {
        stage: 4,
        load_status: Some(Status::SUCCESS),
        start_status: Some(Status::UNSUPPORTED),
        inner,
        operation_status: Status::SUCCESS,
        cleanup_status: Status::SUCCESS,
    }
}

#[test]
fn multi_exit_wire_keeps_complete_and_partial_counts_exact() {
    let completed = NativeResult {
        rust_entered: 1, rust_completed: 1, cleanup_complete: 1,
        outcome: 12, attempted_entries: 65, completed_exits: 65,
        restoration_complete: 1, adapter_checks: 15,
        canary_observed: 1, canary_called: 1,
        ..NativeResult::new()
    };
    let wire = ReturningDiagnostics::capture(&delivered(completed));
    assert_eq!(wire.result_bits(), 0x2000);
    assert_eq!(wire.journal_words([1; 3]), [0, 0x0041_0041, 0x0108_5fcc]);
    let partial = NativeResult {
        outcome: 7, refusal: 0x4303, attempted_entries: 17, completed_exits: 17,
        adapter_checks: 7,
        ..completed
    };
    let wire = ReturningDiagnostics::capture(&delivered(partial));
    assert_eq!(wire.result_bits(), 0x8000);
    assert_eq!(wire.journal_words([1; 3]), [0x4303, 0x0011_0011, 0x0108_5fc7]);
}

#[test]
fn refusal_is_full_width_and_each_bounded_field_has_an_explicit_overflow_marker() {
    let base = NativeResult {
        rust_entered: 1,
        rust_completed: 1,
        cleanup_complete: 1,
        outcome: 1,
        refusal: 0x4000_4132,
        ..NativeResult::new()
    };
    let captured = ReturningDiagnostics::capture(&delivered(base));
    assert_eq!(captured.result_bits(), 0x4000);
    assert_eq!(
        captured.journal_words([1, 2, 3]),
        [0x4000_4132, 0, 0x0310_43c1]
    );

    let boundaries = delivered(NativeResult {
        outcome: 15,
        refusal: u32::MAX as u64,
        attempted_entries: u16::MAX as u64,
        completed_exits: u16::MAX as u64,
        ..base
    });
    let mut boundaries = boundaries;
    boundaries.stage = 7;
    let words = ReturningDiagnostics::capture(&boundaries).journal_words([31; 3]);
    assert_eq!(words, [u32::MAX, u32::MAX, 0x1fff_c3ff]);
    assert_eq!(words[2] & ENCODING_OVERFLOW, 0);

    for inner in [
        NativeResult {
            refusal: 0x1_0000_0000,
            ..base
        },
        NativeResult {
            outcome: 16,
            ..base
        },
        NativeResult {
            attempted_entries: 0x1_0000,
            ..base
        },
        NativeResult {
            completed_exits: 0x1_0000,
            ..base
        },
        NativeResult {
            refusal: u64::MAX,
            outcome: u64::MAX,
            attempted_entries: u64::MAX,
            completed_exits: u64::MAX,
            ..base
        },
    ] {
        let report = delivered(inner);
        let captured = ReturningDiagnostics::capture(&report);
        let words = captured.journal_words([0; 3]);
        assert_eq!(captured.result_bits(), classify(&report) as u32);
        assert_ne!(words[2] & ENCODING_OVERFLOW, 0, "{inner:?}");
        assert_eq!(words[0], inner.refusal.min(u32::MAX as u64) as u32);
        assert_eq!(words[1] & 0xffff, inner.attempted_entries.min(65535) as u32);
        assert_eq!(words[1] >> 16, inner.completed_exits.min(65535) as u32);
        assert_eq!(words[2] & 15, inner.outcome.min(15) as u32);
    }
    let mut report = delivered(base);
    report.stage = u32::MAX;
    let words = ReturningDiagnostics::capture(&report).journal_words([0; 3]);
    assert_ne!(words[2] & ENCODING_OVERFLOW, 0);
    assert_eq!((words[2] >> 4) & 7, 7);
    for counts in [[32, 0, 0], [0, 32, 0], [0, 0, 32], [u32::MAX; 3]] {
        let words = captured.journal_words(counts);
        assert_ne!(words[2] & ENCODING_OVERFLOW, 0);
        for (index, count) in counts.into_iter().enumerate() {
            assert_eq!((words[2] >> (14 + 5 * index)) & 31, count.min(31));
        }
    }
}

#[test]
fn non_boolean_markers_never_become_true_and_header_errors_remain_explicit() {
    for index in 0..6 {
        for malformed in [2, u64::MAX] {
            let mut inner = NativeResult::new();
            let fields = [
                &mut inner.rust_entered,
                &mut inner.rust_completed,
                &mut inner.cleanup_complete,
                &mut inner.restoration_complete,
                &mut inner.canary_called,
                &mut inner.canary_observed,
            ];
            *fields.into_iter().nth(index).unwrap() = malformed;
            let words = ReturningDiagnostics::capture(&delivered(inner)).journal_words([0; 3]);
            assert_ne!(words[2] & NON_BOOLEAN_FLAGS, 0);
            assert_eq!(words[2] & (1 << (7 + index)), 0);
            assert_eq!(words[2] & ENCODING_OVERFLOW, 0);
        }
    }
    for inner in [
        NativeResult {
            magic: 0,
            ..NativeResult::new()
        },
        NativeResult {
            version: 2,
            ..NativeResult::new()
        },
        NativeResult {
            bytes: 0,
            ..NativeResult::new()
        },
        NativeResult {
            reserved: [0, 1],
            ..NativeResult::new()
        },
    ] {
        let captured = ReturningDiagnostics::capture(&delivered(inner));
        assert_eq!(captured.result_bits(), 0x8000);
        assert_ne!(captured.journal_words([0; 3])[2] & INVALID_INNER_HEADER, 0);
    }
    // Presence covers the complete failure mask, not only its low 32 bits.
    for mask in [1, 1 << 63, u64::MAX] {
        let captured = ReturningDiagnostics::capture(&delivered(NativeResult {
            canary_failures: mask,
            ..NativeResult::new()
        }));
        assert_ne!(captured.journal_words([0; 3])[2] & (1 << 13), 0);
        assert_eq!(captured.journal_words([0; 3])[2] & ENCODING_OVERFLOW, 0);
    }
}

#[test]
fn capture_is_a_value_copy_and_does_not_reclassify_delivery_failure() {
    let mut report = delivered(NativeResult::new());
    report.cleanup_status = Status::DEVICE_ERROR;
    let captured = ReturningDiagnostics::capture(&report);
    report.inner.refusal = 19;
    report.inner.rust_entered = 1;
    assert_ne!(captured, ReturningDiagnostics::capture(&report));
    assert_eq!(captured.result_bits(), 0x8000);
    assert_eq!(
        captured.immediate_record(3, 77, 0x1234_5678_8765_4321),
        [3, 77, 0x8765_4321, 0x1234_5678, 0, 0, 0x40, 0x8007_0010]
    );
}
