//! Exercise production registration, notifications, Trace, and journal commit.
//! Only the returned report source, clock, and physical journal bus are replaced.

#![cfg(feature = "card-returning-loader")]

#[path = "../src/firmware/lifecycle.rs"]
mod lifecycle;

mod cpu {
    pub(crate) fn sample() -> (u64, u32) {
        (0x1234_5678_8765_4321, 99)
    }
}
mod pci_io {
    pub(crate) fn status_result(status: uefi_raw::Status) -> Result<(), uefi_raw::Status> {
        if status.is_error() { Err(status) } else { Ok(()) }
    }
}
mod card_returning_adapter {
    pub(crate) fn diagnostics() -> Option<super::ReturningDiagnostics> {
        super::STATE.lock().unwrap().diagnostics
    }
    pub(crate) fn result_bits() -> u32 {
        0
    }
}
mod mmio {
    #[derive(Clone, Copy)]
    pub(crate) struct JournalMapping;
}

use core::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr,
};
use std::sync::Mutex;

use svmvisor_card_abi::native_result::NativeResult;
use svmvisor_dxe::{
    delivery::child_image::Delivery,
    diagnostics::{
        journal::{self, JournalIo},
        returning_detail::ReturningDiagnostics,
    },
};
use uefi_raw::{
    Event, Guid, Status,
    table::boot::{BootServices, EventNotifyFn, EventType, Tpl},
};

static STATE: Mutex<Bus> = Mutex::new(Bus::new());

struct Bus {
    diagnostics: Option<ReturningDiagnostics>,
    events: Vec<(usize, EventNotifyFn, usize, bool)>,
    firmware_calls: usize,
    reads: usize,
    writes: usize,
    staged: [u32; 8],
    last: [u32; 8],
    history: Vec<[u32; 8]>,
    drop_commit: bool,
    bad_magic: bool,
}

impl Bus {
    const fn new() -> Self {
        Self {
            diagnostics: None,
            events: Vec::new(),
            firmware_calls: 0,
            reads: 0,
            writes: 0,
            staged: [0; 8],
            last: [2, 0, 0, 0, 0, 0, 0, 0],
            history: Vec::new(),
            drop_commit: false,
            bad_magic: false,
        }
    }
}

impl JournalIo for mmio::JournalMapping {
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        let mut bus = STATE.lock().unwrap();
        bus.reads += 1;
        Ok(match offset {
            0 => {
                if bus.bad_magic {
                    0
                } else {
                    0x4a4d5653
                }
            }
            4 => 0x10001,
            0x024 => 0,
            0x02c => bus.last[0],
            0x080..=0x09c => bus.last[((offset - 0x080) / 4) as usize],
            _ => panic!("unexpected journal read {offset:#x}"),
        })
    }

    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
        let mut bus = STATE.lock().unwrap();
        bus.writes += 1;
        match offset {
            0x040..=0x05c => bus.staged[((offset - 0x040) / 4) as usize] = value,
            0x060 => {
                assert_eq!(value, bus.staged[0]);
                if !bus.drop_commit {
                    bus.last = bus.staged;
                    let committed = bus.last;
                    bus.history.push(committed);
                }
            }
            _ => panic!("unexpected journal write {offset:#x}"),
        }
        Ok(())
    }
}

unsafe extern "efiapi" fn create_event(
    ty: EventType,
    tpl: Tpl,
    callback: Option<EventNotifyFn>,
    context: *mut c_void,
    group: *mut Guid,
    out: *mut Event,
) -> Status {
    assert_eq!(ty, EventType::NOTIFY_SIGNAL);
    assert_eq!(tpl, Tpl::NOTIFY);
    let kind = context as usize;
    let expected = [
        uefi_raw::guid!("7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b"),
        uefi_raw::guid!("3a2a00ad-98b9-4cdf-a478-702777f1c10b"),
        uefi_raw::guid!("27abf055-b1b8-4c26-8048-748f37baa2df"),
    ];
    assert_eq!(unsafe { *group }, expected[kind]);
    let (event, callback, writes) = {
        let mut bus = STATE.lock().unwrap();
        bus.firmware_calls += 1;
        let event = (bus.events.len() + 1) * 16;
        let callback = callback.unwrap();
        bus.events.push((event, callback, kind, true));
        (event, callback, bus.writes)
    };
    unsafe {
        *out = event as Event;
        // Register must remain inactive even if firmware dispatches here.
        callback(event as Event, context);
    }
    assert_eq!(STATE.lock().unwrap().writes, writes);
    Status::SUCCESS
}

unsafe extern "efiapi" fn close_event(event: Event) -> Status {
    let mut bus = STATE.lock().unwrap();
    bus.firmware_calls += 1;
    let saved = bus.events.iter_mut().find(|e| e.0 == event as usize).unwrap();
    assert!(saved.3);
    saved.3 = false;
    Status::SUCCESS
}

unsafe extern "efiapi" fn unused_service() {
    panic!("unexpected firmware service");
}

fn services() -> BootServices {
    // Same host fixture convention as driver_binding: unused pointer slots are
    // non-null but never called. Used slots have their exact UEFI signatures.
    let mut raw = MaybeUninit::<BootServices>::uninit();
    unsafe {
        let words = raw.as_mut_ptr().cast::<usize>();
        for i in 0..size_of::<BootServices>() / size_of::<usize>() {
            words.add(i).write(unused_service as *const () as usize);
        }
        ptr::addr_of_mut!((*raw.as_mut_ptr()).header).write(core::mem::zeroed());
        ptr::addr_of_mut!((*raw.as_mut_ptr()).create_event_ex).write(create_event);
        ptr::addr_of_mut!((*raw.as_mut_ptr()).close_event).write(close_event);
        raw.assume_init()
    }
}

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

fn completed() -> NativeResult {
    NativeResult {
        rust_entered: 1,
        rust_completed: 1,
        outcome: 2,
        attempted_entries: 1,
        completed_exits: 1,
        restoration_complete: 1,
        cleanup_complete: 1,
        adapter_checks: 15,
        canary_observed: 1,
        canary_called: 1,
        ..NativeResult::new()
    }
}

fn begin(services: &BootServices, report: Delivery) {
    let diagnostics = ReturningDiagnostics::capture(&report);
    let mut bus = Bus::new();
    bus.diagnostics = Some(diagnostics);
    *STATE.lock().unwrap() = bus;
    journal::commit(&mut mmio::JournalMapping, diagnostics.immediate_record(3, 77, 1)).unwrap();
    lifecycle::register(services, mmio::JournalMapping, 77).unwrap();
    assert!(!lifecycle::has_exited());
    assert_eq!(STATE.lock().unwrap().history.len(), 1);
    // Prove registration copied the evidence: callbacks cannot depend on a
    // surviving adapter source or child mailbox after notifications are armed.
    STATE.lock().unwrap().diagnostics = None;
}

fn signal(kind: usize) {
    let (event, calls) = {
        let bus = STATE.lock().unwrap();
        (*bus.events.iter().find(|e| e.2 == kind && e.3).unwrap(), bus.firmware_calls)
    };
    unsafe { (event.1)(event.0 as Event, kind as *mut c_void) };
    assert_eq!(STATE.lock().unwrap().firmware_calls, calls);
}

fn last() -> [u32; 8] {
    STATE.lock().unwrap().last
}

fn finish(services: &BootServices) {
    lifecycle::unregister(services).unwrap();
    assert!(STATE.lock().unwrap().events.iter().all(|e| !e.3));
}

fn counts(record: [u32; 8]) -> [u32; 3] {
    [(record[6] >> 14) & 31, (record[6] >> 19) & 31, (record[6] >> 24) & 31]
}

#[test]
fn real_callbacks_retain_diagnostics_across_publication_failures_anomalies_and_budget() {
    let services = services();
    let refused = NativeResult {
        rust_entered: 1,
        rust_completed: 1,
        cleanup_complete: 1,
        outcome: 1,
        refusal: 0x4132,
        ..NativeResult::new()
    };
    // Independent golden words, not encoder-derived expected values. Include an
    // early pristine mailbox, called refusal, success, and actual failure counts.
    let cases = [
        (NativeResult::new(), 0x4000, [0, 0, 0x40]),
        (refused, 0x4000, [0x4132, 0, 0x3c1]),
        (
            NativeResult { canary_called: 1, canary_observed: 1, ..refused },
            0x4000,
            [0x4132, 0, 0x1bc1],
        ),
        (completed(), 0x2000, [0, 0x0001_0001, 0x1fc2]),
        (
            NativeResult {
                outcome: 3,
                refusal: 0x4312,
                attempted_entries: 3,
                completed_exits: 2,
                cleanup_complete: 0,
                canary_failures: 1 << 63,
                ..completed()
            },
            0x8000,
            [0x4312, 0x0002_0003, 0x3dc3],
        ),
        (NativeResult { refusal: u64::MAX, ..refused }, 0x4000, [u32::MAX, 0, 0x2000_03c1]),
        (NativeResult { canary_called: 2, ..refused }, 0x8000, [0x4132, 0, 0x4000_03c1]),
        (NativeResult { version: 9, ..refused }, 0x8000, [0x4132, 0, 0x8000_03c1]),
    ];
    for (inner, result_bits, expected_words) in cases {
        begin(&services, delivered(inner));
        let immediate = last();
        assert_eq!(immediate[4..7], expected_words);
        assert_eq!(immediate[7], ((result_bits | 7) << 16) | 0x10);
        for (kind, phase, expected_counts, count_bits) in [
            (0, 0x28, [1, 0, 0], 0x4000),
            (1, 0x35, [1, 1, 0], 0x0008_4000),
            (2, 0x40, [1, 1, 1], 0x0108_4000),
        ] {
            signal(kind);
            let record = last();
            assert_eq!(record[0], 4 + kind as u32);
            assert_eq!(record[1..4], [77, 0x8765_4321, 0x1234_5678]);
            assert_eq!(record[4..6], expected_words[..2]);
            assert_eq!(record[6], expected_words[2] | count_bits);
            assert_eq!(record[7], ((result_bits | 7) << 16) | phase);
            assert_eq!(counts(record), expected_counts);
        }
        assert!(lifecycle::has_exited());
        assert_eq!(STATE.lock().unwrap().writes, 36);
        finish(&services);
    }

    begin(&services, delivered(refused));
    let before = last();
    STATE.lock().unwrap().drop_commit = true;
    signal(0); // Lost commit still consumes the Ready count and one attempt.
    assert_eq!(last(), before);
    STATE.lock().unwrap().drop_commit = false;
    signal(0); // Repeated notification retries publication; no new child attempt.
    assert_eq!(last()[7], 0x4607_0028); // Refused, prior failure, duplicate Ready.
    signal(1);
    signal(2);
    signal(2); // Repeated ExitBootServices notification retains evidence/counts.
    assert_eq!(last()[7], 0x4607_0040);
    assert_eq!(last()[4..6], [0x4132, 0]);
    assert_eq!(counts(last()), [2, 1, 2]);
    let bus_accesses = {
        let bus = STATE.lock().unwrap();
        (bus.reads, bus.writes)
    };
    signal(0);
    signal(1);
    assert_eq!(
        {
            let bus = STATE.lock().unwrap();
            (bus.reads, bus.writes)
        },
        bus_accesses
    );
    finish(&services);

    for (events, detail, expected_counts) in [
        (&[2][..], 0x4907, [0, 0, 1]),
        (&[1, 0, 2][..], 0x5107, [1, 1, 1]),
        (&[0, 1, 0, 2][..], 0x5407, [2, 1, 1]),
    ] {
        begin(&services, delivered(refused));
        for &kind in events {
            signal(kind);
        }
        assert_eq!(last()[7], (detail << 16) | 0x40);
        assert_eq!(counts(last()), expected_counts);
        assert_eq!(last()[4..6], [0x4132, 0]);
        finish(&services);
    }

    begin(&services, delivered(refused));
    STATE.lock().unwrap().bad_magic = true;
    signal(0);
    assert_eq!(STATE.lock().unwrap().writes, 9); // Immediate only.
    STATE.lock().unwrap().bad_magic = false;
    signal(1);
    signal(2);
    assert_eq!(last()[7], 0x4207_0040);
    assert_eq!(counts(last()), [1, 1, 1]);
    finish(&services);

    begin(&services, delivered(refused));
    for _ in 0..14 {
        signal(0);
    }
    signal(0); // Reserved After/Exit slots cannot be consumed by another Ready.
    signal(1);
    signal(1); // Exit slot remains reserved.
    signal(2);
    let reads = STATE.lock().unwrap().reads;
    signal(2);
    assert_eq!(STATE.lock().unwrap().reads, reads);
    assert_eq!(STATE.lock().unwrap().writes, 9 * 17); // Immediate + 16 callbacks.
    assert_eq!(counts(last()), [14, 1, 1]);
    assert_eq!(last()[6] & 0x2000_0000, 0);
    finish(&services);

    begin(&services, delivered(refused));
    for _ in 0..16 {
        signal(2);
    }
    assert_eq!(counts(last()), [0, 0, 16]);
    assert_eq!(last()[6] & 0x2000_0000, 0);
    let reads = STATE.lock().unwrap().reads;
    signal(2);
    assert_eq!(STATE.lock().unwrap().reads, reads);
    finish(&services);
}
