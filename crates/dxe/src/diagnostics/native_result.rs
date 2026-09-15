//! Parent-owned result ABI for the returning PE child. No pointers are carried.
//!
//! Rust markers cover the inner routine only. The parent observes StartImage
//! return independently; no marker proves the outer assembly restored its caller.
/// Fixed multi-exit diagnostic profile: 32 CPUID/query rounds and a final stop.
/// These constants bind child observations to the independent parent classifier.
pub const MULTI_EXIT_OUTCOME: u64 = 12;
pub const MULTI_EXIT_ENTRIES: u64 = 65;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeResult {
    pub magic: u64,
    pub version: u32,
    pub bytes: u32,
    pub rust_entered: u64,
    pub rust_completed: u64,
    pub outcome: u64,
    pub refusal: u64,
    pub attempted_entries: u64,
    pub completed_exits: u64,
    pub restoration_complete: u64,
    pub cleanup_complete: u64,
    pub adapter_checks: u64,
    pub canary_failures: u64,
    pub canary_observed: u64,
    pub canary_called: u64,
    pub reserved: [u64; 2],
}
impl NativeResult {
    pub const fn new() -> Self {
        Self {
            magic: u64::from_le_bytes(*b"SVMRES01"),
            version: 1,
            bytes: 128,
            rust_entered: 0,
            rust_completed: 0,
            outcome: 0,
            refusal: 0,
            attempted_entries: 0,
            completed_exits: 0,
            restoration_complete: 0,
            cleanup_complete: 0,
            adapter_checks: 0,
            canary_failures: 0,
            canary_observed: 0,
            canary_called: 0,
            reserved: [0; 2],
        }
    }
    pub fn valid_header(&self) -> bool {
        self.magic == Self::new().magic
            && self.version == 1
            && self.bytes == 128
            && self.reserved == [0; 2]
    }
}
impl Default for NativeResult {
    fn default() -> Self {
        Self::new()
    }
}
const _: () = assert!(core::mem::size_of::<NativeResult>() == 128);
