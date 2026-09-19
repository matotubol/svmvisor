//! The card journal: the device access trait and record commits.

use core::sync::atomic::Ordering;

/// Fixed journal operation; adapters expose only admitted device DWORDs.
pub trait JournalIo {
    type Error;
    fn read(&mut self, offset: u64) -> Result<u32, Self::Error>;
    fn write(&mut self, offset: u64, value: u32) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitError<E> {
    Access(E),
    Sequence,
    Echo,
    Timeout,
}

/// Existing USER3 record wire format, shared by resident and BSP preparation.
// Inlined as it was inside the hypervisor: the resident payload links without LTO.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn diagnostic_payload(
    sequence: u32,
    event: u8,
    fault: bool,
    boot: u32,
    apic: u32,
    tsc: u64,
    context: [u64; 6],
    aux: u32,
) -> [u32; 19] {
    let mut payload = [0; 19];
    payload[0] = sequence;
    payload[1] = event as u32 | 0x100 | (u32::from(fault) << 16);
    payload[2] = boot;
    payload[3] = apic;
    payload[4] = tsc as u32;
    payload[5] = (tsc >> 32) as u32;
    for (i, value) in context.into_iter().enumerate() {
        payload[6 + i * 2] = value as u32;
        payload[7 + i * 2] = (value >> 32) as u32;
    }
    payload[18] = aux;
    payload
}

/// Caller owns the validated endpoint/lifetime and serializes this bank.
/// No partial payload is published: the final sequence write is the commit.
pub fn commit_diagnostic<I: JournalIo>(
    io: &mut I,
    slot: usize,
    payload: [u32; 19],
) -> Result<(), CommitError<I::Error>> {
    if slot >= 32 || payload[0] == 0 {
        return Err(CommitError::Sequence);
    }
    let window = 0x600 + slot as u64 * 0x50;
    for (i, value) in payload.into_iter().enumerate() {
        io.write(window + i as u64 * 4, value).map_err(CommitError::Access)?;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    io.write(window + 76, payload[0]).map_err(CommitError::Access)?;
    io.read(0).map_err(CommitError::Access)?;
    Ok(())
}

/// Stage exactly eight DWORDs and verify a distinct sequence and complete echo.
/// Only a serialized owner with a proven device lifetime may invoke this.
pub fn commit_record<I: JournalIo>(
    io: &mut I,
    record: [u32; 8],
) -> Result<(), CommitError<I::Error>> {
    if io.read(0x2c).map_err(CommitError::Access)? == record[0] {
        return Err(CommitError::Sequence);
    }
    for (i, value) in record.iter().enumerate() {
        io.write(0x40 + i as u64 * 4, *value).map_err(CommitError::Access)?;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    io.write(0x60, record[0]).map_err(CommitError::Access)?;
    for _ in 0..1024 {
        if io.read(0x2c).map_err(CommitError::Access)? == record[0] {
            for (i, value) in record.iter().enumerate() {
                if io.read(0x80 + i as u64 * 4).map_err(CommitError::Access)? != *value {
                    return Err(CommitError::Echo);
                }
            }
            return if io.read(0x24).map_err(CommitError::Access)? == 0 {
                Ok(())
            } else {
                Err(CommitError::Echo)
            };
        }
    }
    Err(CommitError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MockJournal {
        staged: [u32; 8],
        committed: [u32; 8],
        writes: usize,
        polls: usize,
        drop_commit: bool,
        corrupt: bool,
    }
    impl JournalIo for MockJournal {
        type Error = ();
        fn read(&mut self, offset: u64) -> Result<u32, ()> {
            if offset == 0x2c {
                self.polls += 1;
                return Ok(self.committed[0]);
            }
            if offset == 0x24 {
                return Ok(0);
            }
            if (0x80..=0x9c).contains(&offset) {
                return Ok(self.committed[((offset - 0x80) / 4) as usize]);
            }
            Err(())
        }
        fn write(&mut self, offset: u64, value: u32) -> Result<(), ()> {
            self.writes += 1;
            if (0x40..0x60).contains(&offset) {
                self.staged[((offset - 0x40) / 4) as usize] = value;
                return Ok(());
            }
            if offset == 0x60 && value == self.staged[0] {
                if !self.drop_commit {
                    self.committed = self.staged;
                    if self.corrupt {
                        self.committed[7] ^= 1;
                    }
                }
                return Ok(());
            }
            Err(())
        }
    }
    fn journal() -> MockJournal {
        MockJournal {
            staged: [0; 8],
            committed: [0; 8],
            writes: 0,
            polls: 0,
            drop_commit: false,
            corrupt: false,
        }
    }
    #[test]
    fn commit_distinguishes_complete_lost_corrupt_and_stale_evidence() {
        let record = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut good = journal();
        assert_eq!(commit_record(&mut good, record), Ok(()));
        assert_eq!(good.writes, 9);
        assert_eq!(good.committed, record);
        let mut lost = journal();
        lost.drop_commit = true;
        assert_eq!(commit_record(&mut lost, record), Err(CommitError::Timeout));
        assert_eq!((lost.writes, lost.polls), (9, 1025));
        let mut corrupt = journal();
        corrupt.corrupt = true;
        assert_eq!(commit_record(&mut corrupt, record), Err(CommitError::Echo));
        assert_eq!(corrupt.writes, 9);
        let mut stale = journal();
        stale.committed[0] = 1;
        assert_eq!(commit_record(&mut stale, record), Err(CommitError::Sequence));
        assert_eq!(stale.writes, 0);
    }
    #[test]
    fn diagnostic_commit_never_publishes_partial_wide_record() {
        struct Device {
            writes: usize,
            fail: usize,
            staged: [u32; 19],
            published: Option<[u32; 19]>,
            drained: bool,
        }
        impl JournalIo for Device {
            type Error = usize;
            fn read(&mut self, offset: u64) -> Result<u32, usize> {
                assert_eq!(offset, 0);
                self.drained = true;
                Ok(0x4a4d5653)
            }
            fn write(&mut self, offset: u64, value: u32) -> Result<(), usize> {
                let n = self.writes;
                self.writes += 1;
                if n == self.fail {
                    return Err(n);
                }
                assert_eq!(offset, 0xfb0 + n as u64 * 4);
                if n < 19 {
                    self.staged[n] = value;
                } else {
                    assert_eq!(value, self.staged[0]);
                    self.published = Some(self.staged);
                }
                Ok(())
            }
        }
        let payload = diagnostic_payload(
            1,
            12,
            true,
            0x87654321,
            0x34,
            u64::MAX,
            [
                8,
                0xc0010010,
                0xfedcba9876543210,
                0x123456789abcdef0,
                0x8000000000000003,
                1 | (24u64 << 32),
            ],
            4,
        );
        assert_eq!(payload[10], 0x76543210);
        assert_eq!(payload[11], 0xfedcba98);
        assert_eq!(payload[12], 0x9abcdef0);
        assert_eq!(payload[13], 0x12345678);
        for fail in 0..=20 {
            let mut io =
                Device { writes: 0, fail, staged: [0; 19], published: None, drained: false };
            let result = commit_diagnostic(&mut io, 31, payload);
            if fail < 20 {
                assert_eq!(result, Err(CommitError::Access(fail)));
                assert!(io.published.is_none());
                assert!(!io.drained);
            } else {
                assert_eq!(result, Ok(()));
                assert_eq!(io.published, Some(payload));
                assert!(io.drained);
            }
        }
        let mut io =
            Device { writes: 0, fail: 20, staged: [0; 19], published: None, drained: false };
        assert_eq!(commit_diagnostic(&mut io, 32, payload), Err(CommitError::Sequence));
        let mut invalid = payload;
        invalid[0] = 0;
        assert_eq!(commit_diagnostic(&mut io, 31, invalid), Err(CommitError::Sequence));
        assert_eq!(io.writes, 0);
    }
}
