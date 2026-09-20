use svmvisor_card_loader::diagnostics::{
    journal::JournalIo,
    trace::{EventKind::*, MAX_CALLBACK_RECORDS, Trace},
};
use uefi_raw::Status;

struct Journal {
    last: [u32; 8],
    staged: [u32; 8],
    history: Vec<[u32; 8]>,
    lost: bool,
    bad_magic: bool,
    writes: usize,
    reads: usize,
}

impl Journal {
    fn new() -> Self {
        Self {
            last: [2, 0, 0, 0, 0, 0, 0, 0],
            staged: [0; 8],
            history: Vec::new(),
            lost: false,
            bad_magic: false,
            writes: 0,
            reads: 0,
        }
    }
}

impl JournalIo for Journal {
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        self.reads += 1;
        Ok(match offset {
            0 => {
                if self.bad_magic {
                    0
                } else {
                    0x4a4d5653
                }
            }
            4 => 0x10001,
            0x024 => 0,
            0x02c => self.last[0],
            0x080..=0x09c => self.last[((offset - 0x080) / 4) as usize],
            _ => panic!("unexpected read"),
        })
    }

    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
        self.writes += 1;
        if offset == 0x060 {
            assert_eq!(value, self.staged[0]);
            if !self.lost {
                self.last = self.staged;
                self.history.push(self.last);
            }
        } else {
            self.staged[((offset - 0x040) / 4) as usize] = value;
        }
        Ok(())
    }
}

#[test]
fn same_boot_id_fresh_timestamps_and_ordered_counts() {
    let mut trace = Trace::new(77);
    let mut io = Journal::new();
    trace.record(&mut io, ReadyToBoot, 0x1234567887654321, 5).unwrap();
    trace.record(&mut io, AfterReadyToBoot, 0x1234567998765432, 5).unwrap();
    trace.record(&mut io, ExitBootServices, 0x12345680a9876543, 5).unwrap();
    assert_eq!(
        io.history,
        [
            [3, 77, 0x87654321, 0x12345678, 1, 0, 5, 0x00040028],
            [4, 77, 0x98765432, 0x12345679, 0x10001, 0, 5, 0x00040035],
            [5, 77, 0xa9876543, 0x12345680, 0x10001, 1, 5, 0x00040040],
        ]
    );
    assert_eq!(io.writes, 27);
}

#[test]
fn missing_reversed_and_duplicate_events_survive_in_final_snapshot() {
    let cases: &[(&[svmvisor_card_loader::diagnostics::trace::EventKind], u32, [u32; 2])] = &[
        (&[ReadyToBoot, ExitBootServices], 0x08040040, [1, 1]),
        (&[ExitBootServices], 0x09040040, [0, 1]),
        (&[AfterReadyToBoot, ReadyToBoot, ExitBootServices], 0x11040040, [0x10001, 1]),
        (&[ReadyToBoot, AfterReadyToBoot, ReadyToBoot, ExitBootServices], 0x14040040, [0x10002, 1]),
        (&[ReadyToBoot, ReadyToBoot, AfterReadyToBoot, ExitBootServices], 0x04040040, [0x10002, 1]),
        (
            &[ReadyToBoot, AfterReadyToBoot, AfterReadyToBoot, ExitBootServices],
            0x04040040,
            [0x20001, 1],
        ),
        (
            &[ReadyToBoot, AfterReadyToBoot, ExitBootServices, ExitBootServices],
            0x04040040,
            [0x10001, 2],
        ),
    ];
    for &(events, detail, counts) in cases {
        let mut trace = Trace::new(1);
        let mut io = Journal::new();
        for &event in events {
            trace.record(&mut io, event, 1, 0).unwrap();
        }
        assert_eq!(io.last[7], detail);
        assert_eq!(io.last[4..6], counts);
        let reads = io.reads;
        let writes = io.writes;
        for late in [ReadyToBoot, AfterReadyToBoot] {
            assert_eq!(trace.record(&mut io, late, 2, 0), Err(Status::ABORTED));
        }
        assert_eq!((io.reads, io.writes), (reads, writes));
    }
}

#[test]
fn failed_commit_is_reported_by_next_record_and_mapping_mismatch_writes_nothing() {
    let mut trace = Trace::new(1);
    let mut io = Journal::new();
    io.lost = true;
    assert_eq!(trace.record(&mut io, ReadyToBoot, 1, 0), Err(Status::TIMEOUT));
    io.lost = false;
    trace.record(&mut io, AfterReadyToBoot, 2, 0).unwrap();
    trace.record(&mut io, ExitBootServices, 3, 0).unwrap();
    assert_eq!(io.last[7], 0x02040040);
    let writes = io.writes;
    io.bad_magic = true;
    assert_eq!(trace.record(&mut io, ExitBootServices, 4, 0), Err(Status::DEVICE_ERROR));
    assert_eq!(io.writes, writes);
}

#[test]
fn record_budget_reserves_after_and_exit_slots_without_extra_bus_access() {
    let mut trace = Trace::new(1);
    let mut io = Journal::new();
    for _ in 0..MAX_CALLBACK_RECORDS - 2 {
        trace.record(&mut io, ReadyToBoot, 1, 0).unwrap();
    }
    let reads = io.reads;
    assert_eq!(trace.record(&mut io, ReadyToBoot, 1, 0), Err(Status::ABORTED));
    assert_eq!(io.reads, reads);
    trace.record(&mut io, AfterReadyToBoot, 2, 0).unwrap();
    let reads = io.reads;
    assert_eq!(trace.record(&mut io, AfterReadyToBoot, 2, 0), Err(Status::ABORTED));
    assert_eq!(io.reads, reads);
    trace.record(&mut io, ExitBootServices, 3, 0).unwrap();
    let reads = io.reads;
    assert_eq!(trace.record(&mut io, ExitBootServices, 4, 0), Err(Status::ABORTED));
    assert_eq!(io.reads, reads);
    assert_eq!(io.writes, MAX_CALLBACK_RECORDS as usize * 9);
}
