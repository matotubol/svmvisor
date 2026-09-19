//! Copied card-to-resident boot handoff; UEFI 2.11 §§4.1, 7.4.2 and 9.1.1.
//! Parent retains this runtime-data allocation until reset after child SUCCESS.
//! Child copies numeric inputs before returning and retains no parent pointers.

use crate::endpoint::TerminalEndpoint;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentBootOptions {
    pub magic: [u8; 8],
    pub version: u32,
    pub size: u32,
    pub journal_base: u64,
    pub boot_id: u32,
    pub rust_entered: u32,
    pub armed: u32,
    pub reserved0: u32,
    pub failure: u64,
    pub preparation_stage: u32,
    pub preparation_reason: u32,
    pub preparation_status: u64,
    pub preparation_address: u64,
    pub reserved: [u64; 7],
}

const _: () = {
    assert!(core::mem::size_of::<ResidentBootOptions>() == 128);
    assert!(core::mem::offset_of!(ResidentBootOptions, journal_base) == 16);
    assert!(core::mem::offset_of!(ResidentBootOptions, armed) == 32);
    assert!(core::mem::offset_of!(ResidentBootOptions, failure) == 40);
    assert!(core::mem::offset_of!(ResidentBootOptions, preparation_stage) == 48);
    assert!(core::mem::offset_of!(ResidentBootOptions, preparation_status) == 56);
    assert!(core::mem::offset_of!(ResidentBootOptions, preparation_address) == 64);
    assert!(core::mem::offset_of!(ResidentBootOptions, reserved) == 72);
};

impl ResidentBootOptions {
    pub const fn new(journal_base: u64, boot_id: u32) -> Self {
        Self {
            magic: *b"SVMBOT01",
            version: 2,
            size: 128,
            journal_base,
            boot_id,
            rust_entered: 0,
            armed: 0,
            reserved0: 0,
            failure: 0,
            preparation_stage: 0,
            preparation_reason: 0,
            preparation_status: 0,
            preparation_address: 0,
            reserved: [0; 7],
        }
    }

    /// Version 3 reuses the seven reserved words for an immutable numeric
    /// endpoint descriptor. Versions 1/2 retain their exact zero-reserved ABI.
    pub fn with_terminal(mut self, endpoint: TerminalEndpoint) -> Option<Self> {
        if !self.is_valid_header()
            || !endpoint.valid()
            || endpoint.bar0_host_page != self.journal_base
            || endpoint.boot_id != self.boot_id
        {
            return None;
        }
        self.version = 3;
        self.reserved = [
            endpoint.config_page,
            endpoint.bar0_host_page,
            endpoint.fpga_build_id,
            endpoint.rom_build_id,
            endpoint.mmio_config_msr,
            u64::from(endpoint.bar0_raw) | (u64::from(endpoint.segment_bdf) << 32),
            u64::from(endpoint.boot_id)
                | (u64::from(endpoint.command) << 32)
                | (u64::from(endpoint.version) << 48)
                | (u64::from(endpoint.reserved) << 56),
        ];
        Some(self)
    }

    pub fn terminal_endpoint(&self) -> Option<TerminalEndpoint> {
        if self.version != 3 {
            return None;
        }
        let endpoint = TerminalEndpoint {
            config_page: self.reserved[0],
            bar0_host_page: self.reserved[1],
            fpga_build_id: self.reserved[2],
            rom_build_id: self.reserved[3],
            mmio_config_msr: self.reserved[4],
            bar0_raw: self.reserved[5] as u32,
            segment_bdf: (self.reserved[5] >> 32) as u32,
            boot_id: self.reserved[6] as u32,
            command: (self.reserved[6] >> 32) as u16,
            version: (self.reserved[6] >> 48) as u8,
            reserved: (self.reserved[6] >> 56) as u8,
        };
        (endpoint.valid()
            && endpoint.bar0_host_page == self.journal_base
            && endpoint.boot_id == self.boot_id)
            .then_some(endpoint)
    }

    pub fn is_valid_header(&self) -> bool {
        self.magic == *b"SVMBOT01"
            && matches!(self.version, 1 | 2 | 3)
            && self.size == 128
            && self.journal_base != 0
            && self.journal_base <= 0xfffff000
            && self.journal_base & 4095 == 0
            && self.reserved0 == 0
            && self.terminal_fields_valid()
            && (self.version >= 2 || self.preparation_empty())
            && self.rust_entered <= 1
            && self.armed <= 1
    }

    fn terminal_fields_valid(&self) -> bool {
        if self.version != 3 {
            return self.reserved == [0; 7];
        }
        self.terminal_endpoint().is_some()
    }

    pub fn is_armed(&self) -> bool {
        self.is_valid_header()
            && self.rust_entered == 1
            && self.armed == 1
            && self.failure == 0
            && self.preparation_empty()
    }

    pub fn preparation_empty(&self) -> bool {
        self.preparation_stage == 0
            && self.preparation_reason == 0
            && self.preparation_status == 0
            && self.preparation_address == 0
    }

    /// Snapshot exports only three journal words. Compress conventional EFI
    /// status losslessly and retain a complete 48-bit address, otherwise leave
    /// the caller's legacy full-width status record intact.
    pub fn preparation_words(&self) -> Option<[u32; 3]> {
        if !self.is_valid_header()
            || self.version < 2
            || self.rust_entered != 1
            || self.armed != 0
            || self.failure == 0
            || !(1..=255).contains(&self.preparation_stage)
            || self.preparation_reason > 255
            || self.preparation_address >> 48 != 0
            || self.preparation_status & 0x7fff_ffff_8000_0000 != 0
        {
            return None;
        }
        Some([
            self.preparation_stage
                | (self.preparation_reason << 8)
                | (((self.preparation_address >> 32) as u32) << 16),
            self.preparation_status as u32 | ((self.preparation_status >> 32) as u32 & 0x8000_0000),
            self.preparation_address as u32,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_options_are_versioned_immutable_numeric_inputs() {
        let endpoint = TerminalEndpoint {
            config_page: 0xe012_a000,
            bar0_host_page: 0xd000_0000,
            fpga_build_id: 0x1234_5678_9abc_def0,
            rom_build_id: 0xaffe_beef_8765_4321,
            mmio_config_msr: 0xe000_0021,
            bar0_raw: 0xd000_0000,
            segment_bdf: 0x12a,
            boot_id: 42,
            command: 2,
            version: 1,
            reserved: 0,
        };
        let legacy = ResidentBootOptions::new(endpoint.bar0_host_page, 42);
        assert!(legacy.terminal_endpoint().is_none());
        let options = legacy.with_terminal(endpoint).unwrap();
        assert_eq!(options.version, 3);
        assert_eq!(options.terminal_endpoint(), Some(endpoint));
        assert!(options.is_valid_header());
        assert_eq!(options.reserved[5], 0x0000_012a_d000_0000);
        assert_eq!(options.reserved[6], 0x0001_0002_0000_002a);
        for (index, bit) in [(0, 4096), (1, 4096), (4, 1), (5, 1), (6, 1u64 << 56)] {
            let mut bad = options;
            bad.reserved[index] ^= bit;
            assert!(!bad.is_valid_header());
        }
        let mut bad = options;
        bad.version = 2;
        assert!(!bad.is_valid_header());
        bad = options;
        bad.boot_id += 1;
        assert!(!bad.is_valid_header());
        bad = options;
        bad.journal_base += 4096;
        assert!(!bad.is_valid_header());
        assert!(legacy.with_terminal(TerminalEndpoint { command: 0, ..endpoint }).is_none());
        assert!(
            legacy.with_terminal(TerminalEndpoint { segment_bdf: 0x10000, ..endpoint }).is_none()
        );
        let mut failed = options;
        failed.rust_entered = 1;
        failed.failure = 1;
        failed.preparation_stage = 6;
        assert_eq!(failed.preparation_words(), Some([6, 0, 0]));
    }

    #[test]
    fn preparation_record_is_versioned_and_lossless_or_refused() {
        let mut options = ResidentBootOptions::new(0xd0000000, 42);
        options.version = 1;
        assert!(options.is_valid_header());
        options.preparation_stage = 6;
        assert!(!options.is_valid_header());
        options.version = 2;
        options.rust_entered = 1;
        options.failure = 0x8000000000000009;
        options.preparation_reason = 2;
        options.preparation_status = 0x8000000000000003;
        options.preparation_address = 0x123456789abc;
        assert_eq!(options.preparation_words(), Some([0x12340206, 0x80000003, 0x56789abc]));
        options.armed = 1;
        assert!(!options.is_armed());
        assert!(options.preparation_words().is_none());
        options.armed = 0;
        options.preparation_address = 1 << 48;
        assert!(options.preparation_words().is_none());
        options.preparation_address = 0;
        options.preparation_status = 1 << 32;
        assert!(options.preparation_words().is_none());
    }
}
