//! Descriptor-table register image captured by the entry boundary assembly.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableSnapshot {
    /// Exact ten-byte SGDT/SIDT image: u16 limit followed by u64 base.
    pub bytes: [u8; 10],
    pub reserved: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<TableSnapshot>() == 16);

impl TableSnapshot {
    pub fn limit(&self) -> u16 {
        u16::from_le_bytes([self.bytes[0], self.bytes[1]])
    }
    pub fn base(&self) -> u64 {
        u64::from_le_bytes(self.bytes[2..10].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_image_decodes_independent_little_endian_fixture() {
        let table = TableSnapshot {
            bytes: [0x34, 0x12, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
            reserved: [0; 6],
        };
        assert_eq!(table.limit(), 0x1234);
        assert_eq!(table.base(), 0x1122334455667788);
    }
    #[test]
    fn abi_layout_matches_assembly_storage() {
        assert_eq!(core::mem::offset_of!(TableSnapshot, bytes), 0);
        assert_eq!(core::mem::offset_of!(TableSnapshot, reserved), 10);
    }
}
