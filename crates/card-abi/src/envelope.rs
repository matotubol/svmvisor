//! The card image envelope: the 128-byte header at the start of the card's 1 MiB payload slot.
//!
//! Three envelope kinds share bytes `0x000..0x050`. All integers are little-endian.
//!
//! | Offset | Width | Field |
//! | --- | --- | --- |
//! | `0x000` | 8 | magic: `SVMCRD01`, `SVMPE001` or `SVMBPE01` |
//! | `0x008` | 4 | envelope version, 1 |
//! | `0x00c` | 4 | header bytes, 128 |
//! | `0x010` | 8 | payload bytes |
//! | `0x018` | 8 | slot bytes, 0x10_0000 |
//! | `0x020` | 8 | payload offset within the slot, 128 |
//! | `0x028` | 8 | flags: exactly the one bit of the envelope kind |
//! | `0x030` | 32 | SHA-256 of the payload bytes |
//!
//! | Magic | Flags | Payload |
//! | --- | --- | --- |
//! | `SVMCRD01` | `1 << 0` | an `SVMRELO1` relocatable package; bytes `0x050..0x080` are zero |
//! | `SVMPE001` | `1 << 1` | a PE32+ child that returns, subsystem 11 (boot service driver) |
//! | `SVMBPE01` | `1 << 2` | a PE32+ child that stays resident, subsystem 12 (runtime driver) |
//!
//! The two PE kinds continue with metadata that must equal the payload's own PE headers:
//!
//! | Offset | Width | Field |
//! | --- | --- | --- |
//! | `0x050` | 2 | COFF machine, 0x8664 |
//! | `0x052` | 2 | PE subsystem, 11 or 12 |
//! | `0x054` | 2 | optional header magic, 0x020b |
//! | `0x056` | 2 | reserved, zero |
//! | `0x058` | 4 | entry point RVA |
//! | `0x05c` | 4 | image bytes |
//! | `0x060` | 4 | headers bytes |
//! | `0x064` | 4 | section alignment, 4096 |
//! | `0x068` | 4 | file alignment, 512 |
//! | `0x06c` | 4 | section count |
//! | `0x070` | 16 | reserved, zero |
//!
//! The payload follows the header; the rest of the slot is erased flash (`0xff`).
//!
//! The PE envelopes are written by `firmware/card/package-payload.py` (Python, `struct` format
//! `<8sII4Q32s4H6I16s`). The `svmvisor-card-loader` test
//! `optional_python_actual_slot_matches_rust_parser` cross-checks a slot that script produced
//! against the loader's parser.

pub const HEADER_BYTES: usize = 128;
pub const SLOT_BYTES: usize = 0x10_0000;
pub const DIGEST_BYTES: usize = 32;

pub const PACKAGE_MAGIC: [u8; 8] = *b"SVMCRD01";
pub const RETURNING_MAGIC: [u8; 8] = *b"SVMPE001";
pub const RESIDENT_BOOT_MAGIC: [u8; 8] = *b"SVMBPE01";

pub const VERSION: u32 = 1;

pub const FLAGS_PACKAGE: u64 = 1 << 0;
pub const FLAGS_RETURNING: u64 = 1 << 1;
pub const FLAGS_RESIDENT_BOOT: u64 = 1 << 2;

pub const VERSION_OFFSET: usize = 0x008;
pub const HEADER_BYTES_OFFSET: usize = 0x00c;
pub const PAYLOAD_BYTES_OFFSET: usize = 0x010;
pub const SLOT_BYTES_OFFSET: usize = 0x018;
pub const PAYLOAD_OFFSET_OFFSET: usize = 0x020;
pub const FLAGS_OFFSET: usize = 0x028;
pub const DIGEST_OFFSET: usize = 0x030;
/// `SVMCRD01` only: everything from here to the end of the header is zero.
pub const PACKAGE_RESERVED_OFFSET: usize = 0x050;
pub const MACHINE_OFFSET: usize = 0x050;
pub const SUBSYSTEM_OFFSET: usize = 0x052;
pub const OPTIONAL_MAGIC_OFFSET: usize = 0x054;
pub const RESERVED_WORD_OFFSET: usize = 0x056;
pub const ENTRY_RVA_OFFSET: usize = 0x058;
pub const IMAGE_BYTES_OFFSET: usize = 0x05c;
pub const HEADERS_BYTES_OFFSET: usize = 0x060;
pub const SECTION_ALIGNMENT_OFFSET: usize = 0x064;
pub const FILE_ALIGNMENT_OFFSET: usize = 0x068;
pub const SECTIONS_OFFSET: usize = 0x06c;
pub const RESERVED_OFFSET: usize = 0x070;

pub const MACHINE_AMD64: u16 = 0x8664;
pub const OPTIONAL_MAGIC_PE32_PLUS: u16 = 0x020b;
pub const SUBSYSTEM_BOOT_SERVICE_DRIVER: u16 = 11;
pub const SUBSYSTEM_RUNTIME_DRIVER: u16 = 12;

// The narrow PE policy a payload has to meet before firmware ever sees it.
pub const SECTION_ALIGNMENT: u32 = 4096;
pub const FILE_ALIGNMENT: u32 = 512;
pub const MAX_IMAGE_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_SECTIONS: u32 = 16;
pub const MIN_PE_BYTES: usize = 512;
pub const MIN_PACKAGE_BYTES: usize = 64;

const _: () = {
    assert!(RESERVED_OFFSET + 16 == HEADER_BYTES);
    assert!(DIGEST_OFFSET + DIGEST_BYTES == MACHINE_OFFSET);
    assert!(HEADER_BYTES + MIN_PE_BYTES <= SLOT_BYTES);
};
