use std::vec::Vec;

use svmvisor_card_loader::diagnostics::journal::{JournalIo, commit};
use uefi_raw::Status;

const RECORD: [u32; 8] =
    [1, 0x12345678, 0x89abcdef, 0x01234567, 0x76543210, 0xfedcba98, 31, 0x00010010];

struct Mock {
    writes: Vec<(u64, u32)>,
    staged: [u32; 8],
    last: [u32; 8],
    drop_commit: bool,
    fail_at: Option<u64>,
    corrupt: bool,
    polls: usize,
}

impl Mock {
    fn new() -> Self {
        Self {
            writes: Vec::new(),
            staged: [0; 8],
            last: [0; 8],
            drop_commit: false,
            fail_at: None,
            corrupt: false,
            polls: 0,
        }
    }
}

impl JournalIo for Mock {
    fn read(&mut self, offset: u64) -> Result<u32, Status> {
        if offset == 0x02c {
            self.polls += 1;
            return Ok(self.last[0]);
        }
        if offset == 0x024 {
            return Ok(0);
        }
        Ok(self.last[((offset - 0x080) / 4) as usize])
    }

    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
        if self.fail_at == Some(offset) {
            return Err(Status::DEVICE_ERROR);
        }
        self.writes.push((offset, value));
        if offset == 0x060 {
            if !self.drop_commit {
                self.last = self.staged;
            }
            if self.corrupt {
                self.last[7] ^= 1;
            }
        } else {
            self.staged[((offset - 0x040) / 4) as usize] = value;
        }
        Ok(())
    }
}

#[test]
fn exact_layout_and_fresh_staging_on_each_commit() {
    let mut io = Mock::new();
    commit(&mut io, RECORD).unwrap();
    for i in 0..8 {
        assert_eq!(io.writes[i], (0x040 + i as u64 * 4, RECORD[i]));
    }
    assert_eq!(io.writes[8], (0x060, 1));
    let mut next = RECORD;
    next[0] = 2;
    next[7] = 0x13;
    commit(&mut io, next).unwrap();
    assert_eq!(io.writes.len(), 18);
    assert_eq!(io.last, next);
}

#[test]
fn lost_commit_is_bounded_and_old_sequence_cannot_acknowledge() {
    let mut io = Mock::new();
    io.drop_commit = true;
    assert_eq!(commit(&mut io, RECORD), Err(Status::TIMEOUT));
    assert_eq!(io.polls, 1025); // one precheck + exactly 1024 polls
    io.last = RECORD;
    io.writes.clear();
    assert_eq!(commit(&mut io, RECORD), Err(Status::INVALID_PARAMETER));
    assert!(io.writes.is_empty());
}

#[test]
fn staging_failure_never_commits_and_corruption_is_not_success() {
    for i in 0..9 {
        let mut io = Mock::new();
        io.fail_at = Some(0x040 + i * 4);
        assert_eq!(commit(&mut io, RECORD), Err(Status::DEVICE_ERROR));
        assert!(!io.writes.iter().any(|v| v.0 == 0x060));
    }
    let mut io = Mock::new();
    io.corrupt = true;
    assert_eq!(commit(&mut io, RECORD), Err(Status::DEVICE_ERROR));
}
