//! Bounded, allocation-free lifecycle records; independent of UEFI event APIs.

use uefi_raw::Status;

use crate::diagnostics::journal::{JournalIo, commit};

pub const TRACE_DETAIL: u32 = 4;
pub const MAX_CALLBACK_RECORDS: u8 = 16;
const _: () = assert!(MAX_CALLBACK_RECORDS <= 31);

pub struct Trace {
    boot_id: u32,
    ready: u32,
    after: u32,
    exits: u32,
    anomalies: u32,
    attempts: u8,
    failed: bool,
}

impl Trace {
    pub const fn new(boot_id: u32) -> Self {
        Self { boot_id, ready: 0, after: 0, exits: 0, anomalies: 0, attempts: 0, failed: false }
    }

    pub fn has_exited(&self) -> bool {
        self.exits != 0
    }

    pub fn record(
        &mut self,
        io: &mut impl JournalIo,
        event: EventKind,
        tsc: u64,
        cpu: u32,
    ) -> Result<(), Status> {
        // Reserve slots for AfterReadyToBoot and ExitBootServices. After exit,
        // only repeated exit notifications may touch the device.
        let limit = match event {
            EventKind::ReadyToBoot => MAX_CALLBACK_RECORDS - 2,
            EventKind::AfterReadyToBoot => MAX_CALLBACK_RECORDS - 1,
            EventKind::ExitBootServices => MAX_CALLBACK_RECORDS,
        };
        if self.attempts >= limit
            || (self.has_exited() && !matches!(event, EventKind::ExitBootServices))
        {
            return Err(Status::ABORTED);
        }
        self.attempts += 1;
        let (phase, count) = match event {
            EventKind::ReadyToBoot => {
                if self.after != 0 {
                    self.anomalies |= 1 << 12;
                }
                self.ready += 1;
                (0x28, self.ready)
            }
            EventKind::AfterReadyToBoot => {
                if self.ready == 0 {
                    self.anomalies |= (1 << 8) | (1 << 12);
                }
                self.after += 1;
                (0x35, self.after)
            }
            EventKind::ExitBootServices => {
                if self.ready == 0 {
                    self.anomalies |= 1 << 8;
                }
                if self.after == 0 {
                    self.anomalies |= 1 << 11;
                }
                self.exits += 1;
                (0x40, self.exits)
            }
        };
        // Sticky anomalies survive into the final snapshot: missing ready (8),
        // duplicate group (10), missing after at exit (11), reversed order (12).
        if count > 1 {
            self.anomalies |= 1 << 10;
        }
        let detail = TRACE_DETAIL | self.anomalies | (self.failed as u32) << 9;
        let words = [self.ready | (self.after << 16), self.exits, cpu];
        let result = (|| {
            if io.read(0)? != 0x4a4d5653 || io.read(4)? & !0x00020000 != 0x00010001 {
                return Err(Status::DEVICE_ERROR);
            }
            let seq = io.read(0x02c)?;
            commit(
                io,
                [
                    seq.wrapping_add(1),
                    self.boot_id,
                    tsc as u32,
                    (tsc >> 32) as u32,
                    words[0],
                    words[1],
                    words[2],
                    phase | detail << 16,
                ],
            )
        })();
        self.failed |= result.is_err();
        result
    }
}

#[derive(Clone, Copy)]
pub enum EventKind {
    ReadyToBoot,
    AfterReadyToBoot,
    ExitBootServices,
}
