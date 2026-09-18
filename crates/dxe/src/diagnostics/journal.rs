//! BAR0 v1: docs/minimal-baremetal-bringup-roadmap.md, BAR0 phase journal.

use uefi_raw::Status;

pub trait JournalIo {
    fn read(&mut self, offset: u64) -> Result<u32, Status>;
    fn write(&mut self, offset: u64, value: u32) -> Result<(), Status>;
}

/// Publish all eight DWORDs, then commit and flush posted writes by readback.
/// The new sequence must differ from the last committed sequence so old data
/// cannot acknowledge a lost commit. Callers serialize access through BY_DRIVER.
pub fn commit(io: &mut impl JournalIo, record: [u32; 8]) -> Result<(), Status> {
    use svmvisor_hypervisor::host::resident::terminal::{self, CommitError};
    // Firmware keeps its Status-facing boundary; all transports share the core
    // sequence, staging, fence, echo and bounded-poll implementation.
    struct Adapter<'a, I>(&'a mut I);
    impl<I: JournalIo> terminal::JournalIo for Adapter<'_, I> {
        type Error = Status;
        fn read(&mut self, offset: u64) -> Result<u32, Status> {
            self.0.read(offset)
        }
        fn write(&mut self, offset: u64, value: u32) -> Result<(), Status> {
            self.0.write(offset, value)
        }
    }
    terminal::commit_record(&mut Adapter(io), record).map_err(|error| match error {
        CommitError::Access(status) => status,
        CommitError::Sequence => Status::INVALID_PARAMETER,
        CommitError::Echo => Status::DEVICE_ERROR,
        CommitError::Timeout => Status::TIMEOUT,
    })
}
