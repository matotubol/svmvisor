//! Bounded, read-only ACPI validation for the AMD Milestone 0b probe.
//!
//! This module deliberately does not map physical memory. The firmware adapter
//! must copy a bounded candidate object into a byte slice before calling these
//! parsers. Successful values retain the exact validated bytes so integration
//! code can hash and record them without reconstructing firmware data.

use crate::evidence::ProcessorRecord;
use alloc::vec::Vec;
use core::convert::TryFrom;

pub const RSDP_V1_LENGTH: usize = 20;
pub const RSDP_V2_MIN_LENGTH: usize = 36;
pub const SDT_HEADER_LENGTH: usize = 36;
pub const MADT_FIXED_LENGTH: usize = SDT_HEADER_LENGTH + 8;
pub const MCFG_FIXED_LENGTH: usize = SDT_HEADER_LENGTH + 8;
pub const IVRS_FIXED_LENGTH: usize = SDT_HEADER_LENGTH + 12;
pub const FADT_V1_LENGTH: usize = 116;

const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";
const RSDT_SIGNATURE: [u8; 4] = *b"RSDT";
const XSDT_SIGNATURE: [u8; 4] = *b"XSDT";
const MADT_SIGNATURE: [u8; 4] = *b"APIC";
const MCFG_SIGNATURE: [u8; 4] = *b"MCFG";
const IVRS_SIGNATURE: [u8; 4] = *b"IVRS";
const FADT_SIGNATURE: [u8; 4] = *b"FACP";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WhitelistedTableKind {
    Madt,
    Mcfg,
    Ivrs,
    Fadt,
}

impl WhitelistedTableKind {
    #[must_use]
    pub const fn signature(self) -> [u8; 4] {
        match self {
            Self::Madt => MADT_SIGNATURE,
            Self::Mcfg => MCFG_SIGNATURE,
            Self::Ivrs => IVRS_SIGNATURE,
            Self::Fadt => FADT_SIGNATURE,
        }
    }

    #[must_use]
    pub const fn from_signature(signature: [u8; 4]) -> Option<Self> {
        match signature {
            MADT_SIGNATURE => Some(Self::Madt),
            MCFG_SIGNATURE => Some(Self::Mcfg),
            IVRS_SIGNATURE => Some(Self::Ivrs),
            FADT_SIGNATURE => Some(Self::Fadt),
            _ => None,
        }
    }
}

/// Explicit allocation and input-length limits for firmware-controlled data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiLimits {
    pub max_rsdp_length: usize,
    pub max_table_length: usize,
    pub max_root_entries: usize,
    pub max_madt_entries: usize,
    pub max_madt_processors: usize,
    pub max_mcfg_allocations: usize,
    pub max_ivrs_entries: usize,
    pub max_ivhd_device_entries: usize,
    pub max_mp_processors: usize,
}

impl Default for AcpiLimits {
    fn default() -> Self {
        Self {
            max_rsdp_length: 4 * 1024,
            max_table_length: 1024 * 1024,
            max_root_entries: 256,
            max_madt_entries: 512,
            max_madt_processors: 256,
            max_mcfg_allocations: 256,
            max_ivrs_entries: 256,
            max_ivhd_device_entries: 4 * 1024,
            max_mp_processors: 256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcpiError {
    Truncated {
        structure: &'static str,
        needed: usize,
        available: usize,
    },
    InvalidRsdpSignature {
        found: [u8; 8],
    },
    UnexpectedTableSignature {
        expected: [u8; 4],
        found: [u8; 4],
    },
    UnsupportedTableSignature {
        found: [u8; 4],
    },
    UnsupportedRsdpRevision {
        revision: u8,
    },
    InvalidLength {
        structure: &'static str,
        length: usize,
        minimum: usize,
        maximum: usize,
    },
    ChecksumMismatch {
        structure: &'static str,
    },
    NonZeroReservedBytes {
        structure: &'static str,
        offset: usize,
        length: usize,
    },
    NonZeroReservedBits {
        structure: &'static str,
        value: u64,
        reserved_mask: u64,
    },
    ArithmeticOverflow {
        structure: &'static str,
    },
    EntryPayloadMisaligned {
        structure: &'static str,
        payload_length: usize,
        entry_width: usize,
    },
    EntryLimitExceeded {
        structure: &'static str,
        count: usize,
        limit: usize,
    },
    NullPointer {
        structure: &'static str,
        index: usize,
    },
    MissingRootPointer,
    DuplicateRootPointer {
        address: u64,
    },
    DuplicateTablePointer {
        address: u64,
        first_index: usize,
        duplicate_index: usize,
    },
    InvalidSubtableLength {
        table: &'static str,
        entry_type: u8,
        length: usize,
        minimum: usize,
    },
    UnexpectedSubtableLength {
        table: &'static str,
        entry_type: u8,
        length: usize,
        expected: usize,
    },
    UnsupportedIvrsEntryType {
        entry_type: u8,
    },
    InvalidIvrsRevision {
        revision: u8,
    },
    IvrsRevisionDoesNotAllowIvhd {
        revision: u8,
        entry_type: u8,
    },
    UnsupportedIvhdDeviceEntryType {
        entry_type: u8,
    },
    UnsupportedIvhdSpecialDeviceVariety {
        variety: u8,
    },
    IvhdDeviceEntryNotAllowed {
        ivhd_type: u8,
        entry_type: u8,
    },
    IvhdFixedDeviceEntryAfterVariable {
        offset: usize,
        entry_type: u8,
    },
    InvalidIvhdUidFormat {
        uid_format: u8,
    },
    InvalidIvhdUidLength {
        uid_format: u8,
        uid_length: u8,
    },
    InvalidIvhdCapabilityOffset {
        offset: u16,
    },
    MisalignedIvhdBase {
        address: u64,
    },
    InvalidIvhdPciSegmentGroup {
        entry_type: u8,
        segment_group: u16,
    },
    IvhdRequiresEfrSupport {
        entry_type: u8,
    },
    IvhdMissingDeviceEntry {
        offset: usize,
        entry_type: u8,
    },
    IvrsMissingIvhd,
    IvmdWithoutPrecedingIvhd {
        offset: usize,
        entry_type: u8,
    },
    InvalidIvmdFlagCombination {
        flags: u8,
    },
    InvalidIvmdDeviceRange {
        start_device_id: u16,
        end_device_id: u16,
    },
    ZeroIvmdMemoryLength {
        entry_type: u8,
    },
    DuplicateMadtApicId {
        apic_id: u32,
    },
    DuplicateMadtProcessorUid {
        processor_uid: u32,
    },
    InvalidMcfgBusRange {
        segment_group: u16,
        start_bus: u8,
        end_bus: u8,
    },
    OverlappingMcfgBusRange {
        segment_group: u16,
        first_index: usize,
        duplicate_index: usize,
    },
    MisalignedMcfgBase {
        address: u64,
    },
    DuplicateMpProcessorId {
        processor_id: u64,
    },
    MpServicesCountMismatch {
        reported_total: usize,
        record_count: usize,
        reported_enabled: usize,
        enabled_record_count: usize,
    },
}

/// A checked, half-open physical address range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl PhysicalRange {
    #[must_use]
    pub fn length(self) -> u64 {
        self.end_exclusive - self.start
    }
}

/// Check pointer addition before a firmware adapter attempts a bounded copy.
pub fn checked_physical_range(
    start: u64,
    length: u64,
    structure: &'static str,
) -> Result<PhysicalRange, AcpiError> {
    let end_exclusive = start
        .checked_add(length)
        .ok_or(AcpiError::ArithmeticOverflow { structure })?;
    Ok(PhysicalRange {
        start,
        end_exclusive,
    })
}

/// Inspect an RSDP prefix before allocating or copying its declared extent.
/// The complete parser remains responsible for both checksums.
pub fn rsdp_declared_length(bytes: &[u8], limits: &AcpiLimits) -> Result<usize, AcpiError> {
    ensure_available(bytes, RSDP_V1_LENGTH, "RSDP")?;
    let signature = read_array::<8>(bytes, 0, "RSDP")?;
    if signature != RSDP_SIGNATURE {
        return Err(AcpiError::InvalidRsdpSignature { found: signature });
    }
    match bytes[15] {
        0 => Ok(RSDP_V1_LENGTH),
        1 => Err(AcpiError::UnsupportedRsdpRevision { revision: 1 }),
        _ => {
            ensure_available(bytes, RSDP_V2_MIN_LENGTH, "RSDP v2 prefix")?;
            let length = usize::try_from(read_u32(bytes, 20, "RSDP v2 prefix")?).map_err(|_| {
                AcpiError::ArithmeticOverflow {
                    structure: "RSDP length",
                }
            })?;
            validate_length(
                "RSDP v2",
                length,
                RSDP_V2_MIN_LENGTH,
                limits.max_rsdp_length,
            )?;
            Ok(length)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SdtPrefix {
    pub signature: [u8; 4],
    pub length: usize,
}

impl SdtPrefix {
    #[must_use]
    pub fn whitelisted_kind(self) -> Option<WhitelistedTableKind> {
        WhitelistedTableKind::from_signature(self.signature)
    }

    pub fn physical_range_at(self, address: u64) -> Result<PhysicalRange, AcpiError> {
        let length = u64::try_from(self.length).map_err(|_| AcpiError::ArithmeticOverflow {
            structure: "SDT prefix length conversion",
        })?;
        checked_physical_range(address, length, "SDT physical range")
    }
}

/// Parse only the fixed header fields required to classify and bound a later
/// full-table copy. No checksum claim is made until `ValidatedSdt::parse`.
pub fn inspect_sdt_prefix(bytes: &[u8], limits: &AcpiLimits) -> Result<SdtPrefix, AcpiError> {
    ensure_available(bytes, SDT_HEADER_LENGTH, "SDT header")?;
    let length = usize::try_from(read_u32(bytes, 4, "SDT header")?).map_err(|_| {
        AcpiError::ArithmeticOverflow {
            structure: "SDT length",
        }
    })?;
    validate_length("SDT", length, SDT_HEADER_LENGTH, limits.max_table_length)?;
    Ok(SdtPrefix {
        signature: read_array::<4>(bytes, 0, "SDT header")?,
        length,
    })
}

/// Inspect a fixed SDT header and enforce the allocation cap before the full
/// table is copied from its physical address.
pub fn sdt_declared_length(bytes: &[u8], limits: &AcpiLimits) -> Result<usize, AcpiError> {
    inspect_sdt_prefix(bytes, limits).map(|prefix| prefix.length)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootPointers {
    pub rsdt: Option<u64>,
    pub xsdt: Option<u64>,
}

impl RootPointers {
    #[must_use]
    pub fn preferred(self) -> Option<(RootTableKind, u64)> {
        self.xsdt
            .map(|address| (RootTableKind::Xsdt, address))
            .or_else(|| self.rsdt.map(|address| (RootTableKind::Rsdt, address)))
    }
}

/// A validated ACPI Root System Description Pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rsdp<'a> {
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub rsdt_address: u32,
    pub length: u32,
    pub xsdt_address: Option<u64>,
    pub extended_checksum: Option<u8>,
    raw: &'a [u8],
}

impl<'a> Rsdp<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        ensure_available(bytes, RSDP_V1_LENGTH, "RSDP")?;
        let signature = read_array::<8>(bytes, 0, "RSDP")?;
        if signature != RSDP_SIGNATURE {
            return Err(AcpiError::InvalidRsdpSignature { found: signature });
        }
        if !checksum_is_zero(&bytes[..RSDP_V1_LENGTH]) {
            return Err(AcpiError::ChecksumMismatch { structure: "RSDP" });
        }

        let checksum = bytes[8];
        let oem_id = read_array::<6>(bytes, 9, "RSDP")?;
        let revision = bytes[15];
        let rsdt_address = read_u32(bytes, 16, "RSDP")?;

        match revision {
            0 => Ok(Self {
                revision,
                checksum,
                oem_id,
                rsdt_address,
                length: RSDP_V1_LENGTH as u32,
                xsdt_address: None,
                extended_checksum: None,
                raw: &bytes[..RSDP_V1_LENGTH],
            }),
            1 => Err(AcpiError::UnsupportedRsdpRevision { revision }),
            _ => {
                ensure_available(bytes, RSDP_V2_MIN_LENGTH, "RSDP v2")?;
                let length_u32 = read_u32(bytes, 20, "RSDP v2")?;
                let length = rsdp_declared_length(bytes, limits)?;
                ensure_available(bytes, length, "RSDP v2")?;
                let raw = &bytes[..length];
                if !checksum_is_zero(raw) {
                    return Err(AcpiError::ChecksumMismatch {
                        structure: "RSDP extended",
                    });
                }
                require_zero_bytes(raw, 33, 3, "RSDP v2 reserved")?;
                Ok(Self {
                    revision,
                    checksum,
                    oem_id,
                    rsdt_address,
                    length: length_u32,
                    xsdt_address: Some(read_u64(raw, 24, "RSDP v2")?),
                    extended_checksum: Some(raw[32]),
                    raw,
                })
            }
        }
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }

    pub fn physical_range_at(&self, address: u64) -> Result<PhysicalRange, AcpiError> {
        checked_physical_range(address, u64::from(self.length), "RSDP physical range")
    }

    /// Return non-null roots and reject a corrupt RSDP that aliases its two
    /// differently encoded root-table pointers.
    pub fn root_pointers(&self) -> Result<RootPointers, AcpiError> {
        let rsdt = (self.rsdt_address != 0).then_some(u64::from(self.rsdt_address));
        let xsdt = self.xsdt_address.filter(|address| *address != 0);
        if rsdt.is_none() && xsdt.is_none() {
            return Err(AcpiError::MissingRootPointer);
        }
        if let (Some(rsdt), Some(xsdt)) = (rsdt, xsdt)
            && rsdt == xsdt
        {
            return Err(AcpiError::DuplicateRootPointer { address: rsdt });
        }
        Ok(RootPointers { rsdt, xsdt })
    }
}

/// Raw ACPI System Description Table header fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: [u8; 4],
    pub creator_revision: u32,
}

/// A checksum-valid table truncated to its declared length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedSdt<'a> {
    pub header: SdtHeader,
    raw: &'a [u8],
}

impl<'a> ValidatedSdt<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        let length = sdt_declared_length(bytes, limits)?;
        let length_u32 = read_u32(bytes, 4, "SDT header")?;
        ensure_available(bytes, length, "SDT")?;
        let raw = &bytes[..length];
        if !checksum_is_zero(raw) {
            return Err(AcpiError::ChecksumMismatch { structure: "SDT" });
        }
        Ok(Self {
            header: SdtHeader {
                signature: read_array::<4>(raw, 0, "SDT header")?,
                length: length_u32,
                revision: raw[8],
                checksum: raw[9],
                oem_id: read_array::<6>(raw, 10, "SDT header")?,
                oem_table_id: read_array::<8>(raw, 16, "SDT header")?,
                oem_revision: read_u32(raw, 24, "SDT header")?,
                creator_id: read_array::<4>(raw, 28, "SDT header")?,
                creator_revision: read_u32(raw, 32, "SDT header")?,
            },
            raw,
        })
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }

    pub fn physical_range_at(&self, address: u64) -> Result<PhysicalRange, AcpiError> {
        checked_physical_range(address, u64::from(self.header.length), "SDT physical range")
    }

    fn require_signature(self, expected: [u8; 4]) -> Result<Self, AcpiError> {
        if self.header.signature != expected {
            return Err(AcpiError::UnexpectedTableSignature {
                expected,
                found: self.header.signature,
            });
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootTableKind {
    Rsdt,
    Xsdt,
}

impl RootTableKind {
    #[must_use]
    pub const fn signature(self) -> [u8; 4] {
        match self {
            Self::Rsdt => RSDT_SIGNATURE,
            Self::Xsdt => XSDT_SIGNATURE,
        }
    }

    #[must_use]
    pub const fn entry_width(self) -> usize {
        match self {
            Self::Rsdt => 4,
            Self::Xsdt => 8,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootTable<'a> {
    pub sdt: ValidatedSdt<'a>,
    pub kind: RootTableKind,
    pub table_pointers: Vec<u64>,
}

impl<'a> RootTable<'a> {
    pub fn parse(
        bytes: &'a [u8],
        kind: RootTableKind,
        limits: &AcpiLimits,
    ) -> Result<Self, AcpiError> {
        let sdt = ValidatedSdt::parse(bytes, limits)?.require_signature(kind.signature())?;
        let payload_length = sdt.raw.len() - SDT_HEADER_LENGTH;
        let entry_width = kind.entry_width();
        if !payload_length.is_multiple_of(entry_width) {
            return Err(AcpiError::EntryPayloadMisaligned {
                structure: match kind {
                    RootTableKind::Rsdt => "RSDT",
                    RootTableKind::Xsdt => "XSDT",
                },
                payload_length,
                entry_width,
            });
        }
        let count = payload_length / entry_width;
        enforce_count("root table pointers", count, limits.max_root_entries)?;
        let mut table_pointers = Vec::with_capacity(count);
        for index in 0..count {
            let entry_offset = checked_index_offset(
                SDT_HEADER_LENGTH,
                index,
                entry_width,
                "root table entry offset",
            )?;
            let address = match kind {
                RootTableKind::Rsdt => {
                    u64::from(read_u32(sdt.raw, entry_offset, "RSDT table pointer")?)
                }
                RootTableKind::Xsdt => read_u64(sdt.raw, entry_offset, "XSDT table pointer")?,
            };
            if address == 0 {
                return Err(AcpiError::NullPointer {
                    structure: "root table entry",
                    index,
                });
            }
            checked_physical_range(address, SDT_HEADER_LENGTH as u64, "SDT pointer range")?;
            if let Some(first_index) = table_pointers.iter().position(|seen| *seen == address) {
                return Err(AcpiError::DuplicateTablePointer {
                    address,
                    first_index,
                    duplicate_index: index,
                });
            }
            table_pointers.push(address);
        }
        Ok(Self {
            sdt,
            kind,
            table_pointers,
        })
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.sdt.raw_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MadtProcessorKind {
    LocalApic,
    LocalX2Apic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MadtProcessor {
    pub kind: MadtProcessorKind,
    pub processor_uid: u32,
    pub apic_id: u32,
    pub flags: u32,
}

impl MadtProcessor {
    #[must_use]
    pub fn enabled(self) -> bool {
        self.flags & 1 != 0
    }

    #[must_use]
    pub fn online_capable(self) -> bool {
        self.flags & 2 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MadtEntry<'a> {
    pub offset: usize,
    pub entry_type: u8,
    raw: &'a [u8],
}

impl<'a> MadtEntry<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Madt<'a> {
    pub sdt: ValidatedSdt<'a>,
    pub local_apic_address: u32,
    pub flags: u32,
    pub entries: Vec<MadtEntry<'a>>,
    pub processors: Vec<MadtProcessor>,
}

impl<'a> Madt<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        let sdt = ValidatedSdt::parse(bytes, limits)?.require_signature(MADT_SIGNATURE)?;
        Self::from_sdt(sdt, limits)
    }

    fn from_sdt(sdt: ValidatedSdt<'a>, limits: &AcpiLimits) -> Result<Self, AcpiError> {
        validate_length(
            "MADT",
            sdt.raw.len(),
            MADT_FIXED_LENGTH,
            limits.max_table_length,
        )?;
        let local_apic_address = read_u32(sdt.raw, 36, "MADT")?;
        let flags = read_u32(sdt.raw, 40, "MADT")?;
        let mut entries = Vec::new();
        let mut processors = Vec::new();
        let mut offset = MADT_FIXED_LENGTH;
        while offset < sdt.raw.len() {
            enforce_count(
                "MADT entries",
                checked_increment(entries.len(), "MADT entry count")?,
                limits.max_madt_entries,
            )?;
            ensure_range(sdt.raw, offset, 2, "MADT entry header")?;
            let entry_type = sdt.raw[offset];
            let length = usize::from(sdt.raw[offset + 1]);
            if length < 2 {
                return Err(AcpiError::InvalidSubtableLength {
                    table: "MADT",
                    entry_type,
                    length,
                    minimum: 2,
                });
            }
            let end = offset
                .checked_add(length)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "MADT entry range",
                })?;
            if end > sdt.raw.len() {
                return Err(AcpiError::Truncated {
                    structure: "MADT entry",
                    needed: end,
                    available: sdt.raw.len(),
                });
            }
            let raw = &sdt.raw[offset..end];
            let processor = match entry_type {
                0 => {
                    require_exact_subtable_length("MADT", entry_type, length, 8)?;
                    Some(MadtProcessor {
                        kind: MadtProcessorKind::LocalApic,
                        processor_uid: u32::from(raw[2]),
                        apic_id: u32::from(raw[3]),
                        flags: read_u32(raw, 4, "MADT Local APIC")?,
                    })
                }
                9 => {
                    require_exact_subtable_length("MADT", entry_type, length, 16)?;
                    require_zero_bytes(raw, 2, 2, "MADT Local x2APIC reserved")?;
                    Some(MadtProcessor {
                        kind: MadtProcessorKind::LocalX2Apic,
                        processor_uid: read_u32(raw, 12, "MADT Local x2APIC")?,
                        apic_id: read_u32(raw, 4, "MADT Local x2APIC")?,
                        flags: read_u32(raw, 8, "MADT Local x2APIC")?,
                    })
                }
                _ => None,
            };
            if let Some(processor) = processor {
                enforce_count(
                    "MADT processor records",
                    checked_increment(processors.len(), "MADT processor count")?,
                    limits.max_madt_processors,
                )?;
                if (processor.enabled() || processor.online_capable())
                    && processors.iter().any(|seen: &MadtProcessor| {
                        (seen.enabled() || seen.online_capable())
                            && seen.apic_id == processor.apic_id
                    })
                {
                    return Err(AcpiError::DuplicateMadtApicId {
                        apic_id: processor.apic_id,
                    });
                }
                if processors
                    .iter()
                    .any(|seen| seen.processor_uid == processor.processor_uid)
                {
                    return Err(AcpiError::DuplicateMadtProcessorUid {
                        processor_uid: processor.processor_uid,
                    });
                }
                processors.push(processor);
            }
            entries.push(MadtEntry {
                offset,
                entry_type,
                raw,
            });
            offset = end;
        }
        Ok(Self {
            sdt,
            local_apic_address,
            flags,
            entries,
            processors,
        })
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.sdt.raw_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgAllocation<'a> {
    pub base_address: u64,
    pub segment_group: u16,
    pub start_bus: u8,
    pub end_bus: u8,
    pub reserved: u32,
    pub address_range: PhysicalRange,
    raw: &'a [u8],
}

impl<'a> McfgAllocation<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mcfg<'a> {
    pub sdt: ValidatedSdt<'a>,
    pub reserved: [u8; 8],
    pub allocations: Vec<McfgAllocation<'a>>,
}

impl<'a> Mcfg<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        let sdt = ValidatedSdt::parse(bytes, limits)?.require_signature(MCFG_SIGNATURE)?;
        Self::from_sdt(sdt, limits)
    }

    fn from_sdt(sdt: ValidatedSdt<'a>, limits: &AcpiLimits) -> Result<Self, AcpiError> {
        validate_length(
            "MCFG",
            sdt.raw.len(),
            MCFG_FIXED_LENGTH,
            limits.max_table_length,
        )?;
        let reserved = read_array::<8>(sdt.raw, 36, "MCFG reserved")?;
        require_zero_bytes(sdt.raw, 36, 8, "MCFG table reserved")?;
        let payload_length = sdt.raw.len() - MCFG_FIXED_LENGTH;
        if !payload_length.is_multiple_of(16) {
            return Err(AcpiError::EntryPayloadMisaligned {
                structure: "MCFG",
                payload_length,
                entry_width: 16,
            });
        }
        let count = payload_length / 16;
        enforce_count("MCFG allocations", count, limits.max_mcfg_allocations)?;
        let mut allocations: Vec<McfgAllocation<'a>> = Vec::with_capacity(count);
        for index in 0..count {
            let offset = checked_index_offset(MCFG_FIXED_LENGTH, index, 16, "MCFG entry")?;
            let end = offset
                .checked_add(16)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "MCFG entry range",
                })?;
            let raw = &sdt.raw[offset..end];
            let base_address = read_u64(raw, 0, "MCFG allocation")?;
            if base_address & ((1_u64 << 20) - 1) != 0 {
                return Err(AcpiError::MisalignedMcfgBase {
                    address: base_address,
                });
            }
            let segment_group = read_u16(raw, 8, "MCFG allocation")?;
            let start_bus = raw[10];
            let end_bus = raw[11];
            let allocation_reserved = read_u32(raw, 12, "MCFG allocation reserved")?;
            require_zero_bytes(raw, 12, 4, "MCFG allocation reserved")?;
            if start_bus > end_bus {
                return Err(AcpiError::InvalidMcfgBusRange {
                    segment_group,
                    start_bus,
                    end_bus,
                });
            }
            // PCI Firmware defines the MCFG base relative to bus 0 even when
            // this allocation begins at a nonzero bus.  Only the decoded bus
            // interval is a usable ECAM aperture for this allocation.
            let window_start_offset = u64::from(start_bus).checked_mul(1_u64 << 20).ok_or(
                AcpiError::ArithmeticOverflow {
                    structure: "MCFG allocation start offset",
                },
            )?;
            let window_end_offset = (u64::from(end_bus) + 1).checked_mul(1_u64 << 20).ok_or(
                AcpiError::ArithmeticOverflow {
                    structure: "MCFG allocation end offset",
                },
            )?;
            let window_start = base_address.checked_add(window_start_offset).ok_or(
                AcpiError::ArithmeticOverflow {
                    structure: "MCFG allocation start",
                },
            )?;
            let byte_length = window_end_offset - window_start_offset;
            let address_range =
                checked_physical_range(window_start, byte_length, "MCFG allocation range")?;
            for (first_index, prior) in allocations.iter().enumerate() {
                if prior.segment_group == segment_group
                    && start_bus <= prior.end_bus
                    && prior.start_bus <= end_bus
                {
                    return Err(AcpiError::OverlappingMcfgBusRange {
                        segment_group,
                        first_index,
                        duplicate_index: index,
                    });
                }
            }
            allocations.push(McfgAllocation {
                base_address,
                segment_group,
                start_bus,
                end_bus,
                reserved: allocation_reserved,
                address_range,
                raw,
            });
        }
        Ok(Self {
            reserved,
            sdt,
            allocations,
        })
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.sdt.raw_bytes()
    }
}

/// One IVHD device entry. Its exact raw extent follows AMD publication 48882,
/// Rev. 3.11, section 5.2, Table 103 rather than a guessed uniform size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvhdDeviceEntry<'a> {
    pub offset: usize,
    pub entry_type: u8,
    raw: &'a [u8],
}

impl<'a> IvhdDeviceEntry<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }
}

/// IVHD types 10h, 11h, and 40h from AMD publication 48882 Rev. 3.11.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ivhd<'a> {
    pub offset: usize,
    pub entry_type: u8,
    pub flags: u8,
    pub device_id: u16,
    pub capability_offset: u16,
    pub iommu_base_address: u64,
    pub pci_segment_group: u16,
    pub iommu_info: u16,
    pub feature_info: u32,
    pub extended_feature_image: Option<u64>,
    pub extended_feature_image_2: Option<u64>,
    pub device_entries: Vec<IvhdDeviceEntry<'a>>,
    raw: &'a [u8],
}

impl<'a> Ivhd<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }
}

/// One exact 32-byte IVMD type 20h, 21h, or 22h record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ivmd<'a> {
    pub offset: usize,
    pub entry_type: u8,
    pub flags: u8,
    pub device_id: u16,
    pub auxiliary_data_or_end_device_id: u16,
    pub pci_segment_group: Option<u16>,
    pub reserved_or_segment_area: [u8; 8],
    pub start_address: u64,
    pub memory_length: u64,
    pub memory_range: PhysicalRange,
    raw: &'a [u8],
}

impl<'a> Ivmd<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IvrsEntry<'a> {
    Ivhd(Ivhd<'a>),
    Ivmd(Ivmd<'a>),
}

impl<'a> IvrsEntry<'a> {
    #[must_use]
    pub fn offset(&self) -> usize {
        match self {
            Self::Ivhd(entry) => entry.offset,
            Self::Ivmd(entry) => entry.offset,
        }
    }

    #[must_use]
    pub fn entry_type(&self) -> u8 {
        match self {
            Self::Ivhd(entry) => entry.entry_type,
            Self::Ivmd(entry) => entry.entry_type,
        }
    }

    #[must_use]
    pub fn flags(&self) -> u8 {
        match self {
            Self::Ivhd(entry) => entry.flags,
            Self::Ivmd(entry) => entry.flags,
        }
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        match self {
            Self::Ivhd(entry) => entry.raw_bytes(),
            Self::Ivmd(entry) => entry.raw_bytes(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ivrs<'a> {
    pub sdt: ValidatedSdt<'a>,
    pub iv_info: u32,
    pub reserved: [u8; 8],
    pub entries: Vec<IvrsEntry<'a>>,
}

impl<'a> Ivrs<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        let sdt = ValidatedSdt::parse(bytes, limits)?.require_signature(IVRS_SIGNATURE)?;
        Self::from_sdt(sdt, limits)
    }

    fn from_sdt(sdt: ValidatedSdt<'a>, limits: &AcpiLimits) -> Result<Self, AcpiError> {
        validate_length(
            "IVRS",
            sdt.raw.len(),
            IVRS_FIXED_LENGTH,
            limits.max_table_length,
        )?;
        let revision = sdt.header.revision;
        if !matches!(revision, 1 | 2) {
            return Err(AcpiError::InvalidIvrsRevision { revision });
        }
        let iv_info = read_u32(sdt.raw, 36, "IVRS IVinfo")?;
        require_zero_bits(u64::from(iv_info), 0xff80_001c, "IVRS IVinfo reserved bits")?;
        let reserved = read_array::<8>(sdt.raw, 40, "IVRS reserved")?;
        require_zero_bytes(sdt.raw, 40, 8, "IVRS table reserved")?;
        let mut entries = Vec::new();
        let mut device_entry_count = 0_usize;
        let mut has_ivhd = false;
        let mut offset = IVRS_FIXED_LENGTH;
        while offset < sdt.raw.len() {
            enforce_count(
                "IVRS entries",
                checked_increment(entries.len(), "IVRS entry count")?,
                limits.max_ivrs_entries,
            )?;
            ensure_range(sdt.raw, offset, 4, "IVRS entry header")?;
            let entry_type = sdt.raw[offset];
            let length = usize::from(read_u16(sdt.raw, offset + 2, "IVRS entry")?);
            if length < 4 {
                return Err(AcpiError::InvalidSubtableLength {
                    table: "IVRS",
                    entry_type,
                    length,
                    minimum: 4,
                });
            }
            let end = offset
                .checked_add(length)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "IVRS entry range",
                })?;
            if end > sdt.raw.len() {
                return Err(AcpiError::Truncated {
                    structure: "IVRS entry",
                    needed: end,
                    available: sdt.raw.len(),
                });
            }
            let raw = &sdt.raw[offset..end];
            if revision == 1 && entry_type == 0x40 {
                return Err(AcpiError::IvrsRevisionDoesNotAllowIvhd {
                    revision,
                    entry_type,
                });
            }
            let entry = match entry_type {
                0x10 | 0x11 | 0x40 => {
                    let ivhd = parse_ivhd(raw, offset, iv_info, limits, &mut device_entry_count)?;
                    has_ivhd = true;
                    IvrsEntry::Ivhd(ivhd)
                }
                0x20..=0x22 => {
                    if !has_ivhd {
                        return Err(AcpiError::IvmdWithoutPrecedingIvhd { offset, entry_type });
                    }
                    IvrsEntry::Ivmd(parse_ivmd(raw, offset)?)
                }
                _ => return Err(AcpiError::UnsupportedIvrsEntryType { entry_type }),
            };
            entries.push(entry);
            offset = end;
        }
        if !has_ivhd {
            return Err(AcpiError::IvrsMissingIvhd);
        }
        Ok(Self {
            iv_info,
            reserved,
            sdt,
            entries,
        })
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.sdt.raw_bytes()
    }
}

fn parse_ivhd<'a>(
    raw: &'a [u8],
    offset: usize,
    iv_info: u32,
    limits: &AcpiLimits,
    total_device_entries: &mut usize,
) -> Result<Ivhd<'a>, AcpiError> {
    let entry_type = raw[0];
    // Publication 48882 Rev. 3.11 section 5.2: type 10h has a 24-byte
    // header; types 11h and 40h extend it with two 64-bit EFR images.
    let header_length = match entry_type {
        0x10 => 24,
        0x11 | 0x40 => 40,
        _ => return Err(AcpiError::UnsupportedIvrsEntryType { entry_type }),
    };
    require_subtable_length("IVHD", entry_type, raw.len(), header_length)?;

    let flags = raw[1];
    let capability_offset = read_u16(raw, 6, "IVHD capability offset")?;
    if capability_offset == 0 || capability_offset & 3 != 0 {
        return Err(AcpiError::InvalidIvhdCapabilityOffset {
            offset: capability_offset,
        });
    }
    let iommu_base_address = read_u64(raw, 8, "IVHD IOMMU base")?;
    if iommu_base_address & ((1_u64 << 14) - 1) != 0 {
        return Err(AcpiError::MisalignedIvhdBase {
            address: iommu_base_address,
        });
    }
    checked_physical_range(
        iommu_base_address,
        1_u64 << 14,
        "IVHD minimum IOMMU register range",
    )?;
    let pci_segment_group = read_u16(raw, 16, "IVHD PCI segment")?;
    let iommu_info = read_u16(raw, 18, "IVHD IOMMU info")?;
    require_zero_bits(
        u64::from(iommu_info),
        0xe0e0,
        "IVHD IOMMU info reserved bits",
    )?;
    let feature_info = read_u32(raw, 20, "IVHD feature info")?;
    match entry_type {
        0x10 => {
            if iv_info & 1 == 0 {
                require_zero_bits(
                    u64::from(feature_info),
                    u64::from(u32::MAX),
                    "IVHD type 10h feature field reserved without EFRSup",
                )?;
            }
        }
        0x11 | 0x40 => {
            if iv_info & 1 == 0 {
                return Err(AcpiError::IvhdRequiresEfrSupport { entry_type });
            }
            require_zero_bits(
                u64::from(flags),
                0xc0,
                "IVHD type 11h/40h flags reserved bits",
            )?;
            if pci_segment_group != 0 {
                return Err(AcpiError::InvalidIvhdPciSegmentGroup {
                    entry_type,
                    segment_group: pci_segment_group,
                });
            }
            require_zero_bits(
                u64::from(feature_info),
                0xf000_1ffe,
                "IVHD type 11h/40h attributes reserved bits",
            )?;
        }
        _ => return Err(AcpiError::UnsupportedIvrsEntryType { entry_type }),
    }

    let mut device_entries = Vec::new();
    let mut seen_variable_device_entry = false;
    let mut device_offset = header_length;
    while device_offset < raw.len() {
        let device_type = raw[device_offset];
        let table_offset =
            offset
                .checked_add(device_offset)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "IVHD device table offset",
                })?;
        if seen_variable_device_entry && device_type < 0x80 {
            return Err(AcpiError::IvhdFixedDeviceEntryAfterVariable {
                offset: table_offset,
                entry_type: device_type,
            });
        }
        if entry_type != 0x40 && device_type >= 0x80 {
            return Err(AcpiError::IvhdDeviceEntryNotAllowed {
                ivhd_type: entry_type,
                entry_type: device_type,
            });
        }
        let device_length = match device_type {
            0x00..=0x3f => 4,
            0x40..=0x7f => 8,
            0xf0 => {
                ensure_range(raw, device_offset, 22, "IVHD F0h device entry prefix")?;
                22_usize
                    .checked_add(usize::from(raw[device_offset + 21]))
                    .ok_or(AcpiError::ArithmeticOverflow {
                        structure: "IVHD F0h device entry length",
                    })?
            }
            _ => {
                return Err(AcpiError::UnsupportedIvhdDeviceEntryType {
                    entry_type: device_type,
                });
            }
        };
        let end =
            device_offset
                .checked_add(device_length)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "IVHD device entry range",
                })?;
        if end > raw.len() {
            return Err(AcpiError::Truncated {
                structure: "IVHD device entry",
                needed: end,
                available: raw.len(),
            });
        }
        let device_raw = &raw[device_offset..end];
        validate_ivhd_device_entry(device_raw)?;
        *total_device_entries =
            total_device_entries
                .checked_add(1)
                .ok_or(AcpiError::ArithmeticOverflow {
                    structure: "IVHD device entry count",
                })?;
        enforce_count(
            "IVHD device entries",
            *total_device_entries,
            limits.max_ivhd_device_entries,
        )?;
        device_entries.push(IvhdDeviceEntry {
            offset: table_offset,
            entry_type: device_type,
            raw: device_raw,
        });
        seen_variable_device_entry |= device_type >= 0x80;
        device_offset = end;
    }

    if device_entries.is_empty() {
        return Err(AcpiError::IvhdMissingDeviceEntry { offset, entry_type });
    }

    Ok(Ivhd {
        offset,
        entry_type,
        flags,
        device_id: read_u16(raw, 4, "IVHD DeviceID")?,
        capability_offset,
        iommu_base_address,
        pci_segment_group,
        iommu_info,
        feature_info,
        extended_feature_image: (header_length == 40)
            .then(|| read_u64(raw, 24, "IVHD EFR image"))
            .transpose()?,
        extended_feature_image_2: (header_length == 40)
            .then(|| read_u64(raw, 32, "IVHD EFR image 2"))
            .transpose()?,
        device_entries,
        raw,
    })
}

fn validate_ivhd_device_entry(raw: &[u8]) -> Result<(), AcpiError> {
    let entry_type = raw[0];
    match entry_type {
        0x01..=0x03 => Ok(()),
        0x04 => require_zero_bytes(raw, 3, 1, "IVHD end-of-range DTE setting"),
        0x42 | 0x43 => {
            require_zero_bytes(raw, 4, 1, "IVHD alias reserved byte")?;
            require_zero_bytes(raw, 7, 1, "IVHD alias trailing reserved byte")
        }
        0x46 | 0x47 => require_zero_bits(
            u64::from(read_u32(raw, 4, "IVHD extended DTE setting")?),
            0x7fff_fff8,
            "IVHD extended DTE setting reserved bits",
        ),
        0x48 => {
            require_zero_bytes(raw, 1, 2, "IVHD special-device reserved DeviceID")?;
            let variety = raw[7];
            if !matches!(variety, 1 | 2) {
                return Err(AcpiError::UnsupportedIvhdSpecialDeviceVariety { variety });
            }
            Ok(())
        }
        0xf0 => {
            let uid_format = raw[20];
            let uid_length = raw[21];
            if uid_format > 2 {
                return Err(AcpiError::InvalidIvhdUidFormat { uid_format });
            }
            if (uid_format == 0) != (uid_length == 0) {
                return Err(AcpiError::InvalidIvhdUidLength {
                    uid_format,
                    uid_length,
                });
            }
            Ok(())
        }
        _ => Err(AcpiError::UnsupportedIvhdDeviceEntryType { entry_type }),
    }
}

fn parse_ivmd(raw: &[u8], offset: usize) -> Result<Ivmd<'_>, AcpiError> {
    let entry_type = raw[0];
    require_exact_subtable_length("IVMD", entry_type, raw.len(), 32)?;
    let flags = raw[1];
    require_zero_bits(u64::from(flags), 0xf0, "IVMD flags reserved bits")?;
    if flags & 0x08 == 0 && flags & 0x06 == 0 && flags & 1 != 0 {
        return Err(AcpiError::InvalidIvmdFlagCombination { flags });
    }
    let device_id = read_u16(raw, 4, "IVMD DeviceID")?;
    let auxiliary_data_or_end_device_id = read_u16(raw, 6, "IVMD auxiliary/end DeviceID")?;
    let reserved_or_segment_area = read_array::<8>(raw, 8, "IVMD reserved/segment area")?;
    let pci_segment_group = match entry_type {
        0x20 => {
            require_zero_bytes(raw, 4, 12, "IVMD type 20h reserved fields")?;
            None
        }
        0x21 => {
            require_zero_bytes(raw, 6, 2, "IVMD type 21h auxiliary field")?;
            require_zero_bytes(raw, 10, 6, "IVMD type 21h reserved field")?;
            Some(read_u16(raw, 8, "IVMD PCI segment")?)
        }
        0x22 => {
            require_zero_bytes(raw, 10, 6, "IVMD type 22h reserved field")?;
            if device_id > auxiliary_data_or_end_device_id {
                return Err(AcpiError::InvalidIvmdDeviceRange {
                    start_device_id: device_id,
                    end_device_id: auxiliary_data_or_end_device_id,
                });
            }
            Some(read_u16(raw, 8, "IVMD PCI segment")?)
        }
        _ => return Err(AcpiError::UnsupportedIvrsEntryType { entry_type }),
    };
    let start_address = read_u64(raw, 16, "IVMD start address")?;
    let memory_length = read_u64(raw, 24, "IVMD memory length")?;
    if memory_length == 0 {
        return Err(AcpiError::ZeroIvmdMemoryLength { entry_type });
    }
    let memory_range = checked_physical_range(start_address, memory_length, "IVMD memory range")?;
    Ok(Ivmd {
        offset,
        entry_type,
        flags,
        device_id,
        auxiliary_data_or_end_device_id,
        pci_segment_group,
        reserved_or_segment_area,
        start_address,
        memory_length,
        memory_range,
        raw,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fadt<'a> {
    pub sdt: ValidatedSdt<'a>,
    pub firmware_ctrl: u32,
    pub dsdt: u32,
    pub preferred_pm_profile: u8,
    pub sci_interrupt: u16,
    pub smi_command: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub iapc_boot_arch: Option<u16>,
    pub flags: u32,
    pub minor_version: Option<u8>,
    pub x_firmware_ctrl: Option<u64>,
    pub x_dsdt: Option<u64>,
}

impl<'a> Fadt<'a> {
    pub fn parse(bytes: &'a [u8], limits: &AcpiLimits) -> Result<Self, AcpiError> {
        let sdt = ValidatedSdt::parse(bytes, limits)?.require_signature(FADT_SIGNATURE)?;
        Self::from_sdt(sdt, limits)
    }

    fn from_sdt(sdt: ValidatedSdt<'a>, limits: &AcpiLimits) -> Result<Self, AcpiError> {
        validate_length(
            "FADT",
            sdt.raw.len(),
            FADT_V1_LENGTH,
            limits.max_table_length,
        )?;
        let x_firmware_ctrl = (sdt.raw.len() >= 140)
            .then(|| read_u64(sdt.raw, 132, "FADT X_FIRMWARE_CTRL"))
            .transpose()?;
        let x_dsdt = (sdt.raw.len() >= 148)
            .then(|| read_u64(sdt.raw, 140, "FADT X_DSDT"))
            .transpose()?;
        let iapc_boot_arch = (sdt.raw.len() >= 111)
            .then(|| read_u16(sdt.raw, 109, "FADT IAPC_BOOT_ARCH"))
            .transpose()?;
        let minor_version = (sdt.raw.len() >= 132).then(|| sdt.raw[131]);
        let result = Self {
            firmware_ctrl: read_u32(sdt.raw, 36, "FADT FIRMWARE_CTRL")?,
            dsdt: read_u32(sdt.raw, 40, "FADT DSDT")?,
            preferred_pm_profile: sdt.raw[45],
            sci_interrupt: read_u16(sdt.raw, 46, "FADT SCI_INT")?,
            smi_command: read_u32(sdt.raw, 48, "FADT SMI_CMD")?,
            acpi_enable: sdt.raw[52],
            acpi_disable: sdt.raw[53],
            iapc_boot_arch,
            flags: read_u32(sdt.raw, 112, "FADT flags")?,
            minor_version,
            x_firmware_ctrl,
            x_dsdt,
            sdt,
        };
        if let Some(address) = result.firmware_control_address() {
            checked_physical_range(address, 8, "FADT FACS pointer range")?;
        }
        if let Some(address) = result.dsdt_address() {
            checked_physical_range(address, SDT_HEADER_LENGTH as u64, "FADT DSDT pointer range")?;
        }
        Ok(result)
    }

    #[must_use]
    pub fn firmware_control_address(&self) -> Option<u64> {
        self.x_firmware_ctrl
            .filter(|address| *address != 0)
            .or_else(|| (self.firmware_ctrl != 0).then_some(u64::from(self.firmware_ctrl)))
    }

    #[must_use]
    pub fn dsdt_address(&self) -> Option<u64> {
        self.x_dsdt
            .filter(|address| *address != 0)
            .or_else(|| (self.dsdt != 0).then_some(u64::from(self.dsdt)))
    }

    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        self.sdt.raw_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WhitelistedTable<'a> {
    Madt(Madt<'a>),
    Mcfg(Mcfg<'a>),
    Ivrs(Ivrs<'a>),
    Fadt(Fadt<'a>),
}

impl<'a> WhitelistedTable<'a> {
    #[must_use]
    pub fn raw_bytes(&self) -> &'a [u8] {
        match self {
            Self::Madt(table) => table.raw_bytes(),
            Self::Mcfg(table) => table.raw_bytes(),
            Self::Ivrs(table) => table.raw_bytes(),
            Self::Fadt(table) => table.raw_bytes(),
        }
    }
}

/// Validate and parse only the record-only table whitelist for this slice.
pub fn parse_whitelisted_table<'a>(
    bytes: &'a [u8],
    limits: &AcpiLimits,
) -> Result<WhitelistedTable<'a>, AcpiError> {
    let sdt = ValidatedSdt::parse(bytes, limits)?;
    match sdt.header.signature {
        MADT_SIGNATURE => Madt::from_sdt(sdt, limits).map(WhitelistedTable::Madt),
        MCFG_SIGNATURE => Mcfg::from_sdt(sdt, limits).map(WhitelistedTable::Mcfg),
        IVRS_SIGNATURE => Ivrs::from_sdt(sdt, limits).map(WhitelistedTable::Ivrs),
        FADT_SIGNATURE => Fadt::from_sdt(sdt, limits).map(WhitelistedTable::Fadt),
        found => Err(AcpiError::UnsupportedTableSignature { found }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorEnabledMismatch {
    pub processor_id: u64,
    pub madt_enabled: bool,
    pub mp_enabled: bool,
}

/// Evidence-only comparison. A match does not claim processor ownership or
/// qualify SVM; it only compares firmware-described identifiers and state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MadtMpTopologyCrossCheck {
    pub madt_processor_count: usize,
    pub madt_enabled_count: usize,
    pub mp_processor_count: usize,
    pub mp_enabled_count: usize,
    pub count_matches: bool,
    pub enabled_count_matches: bool,
    pub madt_only_apic_ids: Vec<u32>,
    pub mp_only_processor_ids: Vec<u64>,
    pub enabled_mismatches: Vec<ProcessorEnabledMismatch>,
}

impl MadtMpTopologyCrossCheck {
    #[must_use]
    pub fn identifiers_match(&self) -> bool {
        self.madt_only_apic_ids.is_empty() && self.mp_only_processor_ids.is_empty()
    }

    #[must_use]
    pub fn enabled_states_match(&self) -> bool {
        self.enabled_mismatches.is_empty()
    }

    #[must_use]
    pub fn complete_match(&self) -> bool {
        self.count_matches
            && self.enabled_count_matches
            && self.identifiers_match()
            && self.enabled_states_match()
    }
}

pub fn cross_check_madt_with_mp_services(
    madt: &Madt<'_>,
    reported_total: usize,
    reported_enabled: usize,
    mp_processors: &[ProcessorRecord],
    limits: &AcpiLimits,
) -> Result<MadtMpTopologyCrossCheck, AcpiError> {
    enforce_count(
        "MP Services processor records",
        mp_processors.len(),
        limits.max_mp_processors,
    )?;
    let enabled_record_count = mp_processors
        .iter()
        .filter(|processor| processor.enabled)
        .count();
    if reported_total != mp_processors.len() || reported_enabled != enabled_record_count {
        return Err(AcpiError::MpServicesCountMismatch {
            reported_total,
            record_count: mp_processors.len(),
            reported_enabled,
            enabled_record_count,
        });
    }
    for (index, processor) in mp_processors.iter().enumerate() {
        if mp_processors[..index]
            .iter()
            .any(|seen| seen.processor_id == processor.processor_id)
        {
            return Err(AcpiError::DuplicateMpProcessorId {
                processor_id: processor.processor_id,
            });
        }
    }

    let madt_enabled_count = madt
        .processors
        .iter()
        .filter(|processor| processor.enabled())
        .count();
    let mut madt_only_apic_ids = Vec::new();
    let mut enabled_mismatches = Vec::new();
    for processor in &madt.processors {
        match mp_processors
            .iter()
            .find(|mp| mp.processor_id == u64::from(processor.apic_id))
        {
            Some(mp) if mp.enabled != processor.enabled() => {
                enabled_mismatches.push(ProcessorEnabledMismatch {
                    processor_id: mp.processor_id,
                    madt_enabled: processor.enabled(),
                    mp_enabled: mp.enabled,
                });
            }
            Some(_) => {}
            None => madt_only_apic_ids.push(processor.apic_id),
        }
    }
    let mut mp_only_processor_ids = Vec::new();
    for processor in mp_processors {
        if !madt
            .processors
            .iter()
            .any(|madt| u64::from(madt.apic_id) == processor.processor_id)
        {
            mp_only_processor_ids.push(processor.processor_id);
        }
    }
    Ok(MadtMpTopologyCrossCheck {
        madt_processor_count: madt.processors.len(),
        madt_enabled_count,
        mp_processor_count: mp_processors.len(),
        mp_enabled_count: enabled_record_count,
        count_matches: madt.processors.len() == mp_processors.len(),
        enabled_count_matches: madt_enabled_count == enabled_record_count,
        madt_only_apic_ids,
        mp_only_processor_ids,
        enabled_mismatches,
    })
}

fn validate_length(
    structure: &'static str,
    length: usize,
    minimum: usize,
    maximum: usize,
) -> Result<(), AcpiError> {
    if length < minimum || length > maximum {
        return Err(AcpiError::InvalidLength {
            structure,
            length,
            minimum,
            maximum,
        });
    }
    Ok(())
}

fn enforce_count(structure: &'static str, count: usize, limit: usize) -> Result<(), AcpiError> {
    if count > limit {
        return Err(AcpiError::EntryLimitExceeded {
            structure,
            count,
            limit,
        });
    }
    Ok(())
}

fn checked_increment(value: usize, structure: &'static str) -> Result<usize, AcpiError> {
    value
        .checked_add(1)
        .ok_or(AcpiError::ArithmeticOverflow { structure })
}

fn require_subtable_length(
    table: &'static str,
    entry_type: u8,
    length: usize,
    minimum: usize,
) -> Result<(), AcpiError> {
    if length < minimum {
        return Err(AcpiError::InvalidSubtableLength {
            table,
            entry_type,
            length,
            minimum,
        });
    }
    Ok(())
}

fn require_exact_subtable_length(
    table: &'static str,
    entry_type: u8,
    length: usize,
    expected: usize,
) -> Result<(), AcpiError> {
    if length != expected {
        return Err(AcpiError::UnexpectedSubtableLength {
            table,
            entry_type,
            length,
            expected,
        });
    }
    Ok(())
}

fn require_zero_bytes(
    bytes: &[u8],
    offset: usize,
    length: usize,
    structure: &'static str,
) -> Result<(), AcpiError> {
    ensure_range(bytes, offset, length, structure)?;
    if bytes[offset..offset + length].iter().any(|byte| *byte != 0) {
        return Err(AcpiError::NonZeroReservedBytes {
            structure,
            offset,
            length,
        });
    }
    Ok(())
}

fn require_zero_bits(
    value: u64,
    reserved_mask: u64,
    structure: &'static str,
) -> Result<(), AcpiError> {
    if value & reserved_mask != 0 {
        return Err(AcpiError::NonZeroReservedBits {
            structure,
            value,
            reserved_mask,
        });
    }
    Ok(())
}

fn checksum_is_zero(bytes: &[u8]) -> bool {
    bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}

fn checked_index_offset(
    base: usize,
    index: usize,
    width: usize,
    structure: &'static str,
) -> Result<usize, AcpiError> {
    index
        .checked_mul(width)
        .and_then(|relative| base.checked_add(relative))
        .ok_or(AcpiError::ArithmeticOverflow { structure })
}

fn ensure_available(bytes: &[u8], needed: usize, structure: &'static str) -> Result<(), AcpiError> {
    if bytes.len() < needed {
        return Err(AcpiError::Truncated {
            structure,
            needed,
            available: bytes.len(),
        });
    }
    Ok(())
}

fn ensure_range(
    bytes: &[u8],
    offset: usize,
    length: usize,
    structure: &'static str,
) -> Result<(), AcpiError> {
    let end = offset
        .checked_add(length)
        .ok_or(AcpiError::ArithmeticOverflow { structure })?;
    ensure_available(bytes, end, structure)
}

fn read_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
    structure: &'static str,
) -> Result<[u8; N], AcpiError> {
    ensure_range(bytes, offset, N, structure)?;
    let mut value = [0_u8; N];
    value.copy_from_slice(&bytes[offset..offset + N]);
    Ok(value)
}

fn read_u16(bytes: &[u8], offset: usize, structure: &'static str) -> Result<u16, AcpiError> {
    Ok(u16::from_le_bytes(read_array::<2>(
        bytes, offset, structure,
    )?))
}

fn read_u32(bytes: &[u8], offset: usize, structure: &'static str) -> Result<u32, AcpiError> {
    Ok(u32::from_le_bytes(read_array::<4>(
        bytes, offset, structure,
    )?))
}

fn read_u64(bytes: &[u8], offset: usize, structure: &'static str) -> Result<u64, AcpiError> {
    Ok(u64::from_le_bytes(read_array::<8>(
        bytes, offset, structure,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn repair_checksum(bytes: &mut [u8], checksum_offset: usize) {
        bytes[checksum_offset] = 0;
        let sum = bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[checksum_offset] = 0_u8.wrapping_sub(sum);
    }

    fn make_sdt(signature: [u8; 4], body: &[u8]) -> Vec<u8> {
        let length = SDT_HEADER_LENGTH + body.len();
        let mut bytes = vec![0_u8; length];
        bytes[0..4].copy_from_slice(&signature);
        bytes[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        bytes[8] = 2;
        bytes[10..16].copy_from_slice(b"SVMVIS");
        bytes[16..24].copy_from_slice(b"M0BTEST ");
        bytes[36..].copy_from_slice(body);
        repair_checksum(&mut bytes, 9);
        bytes
    }

    fn make_rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut bytes = vec![0_u8; RSDP_V2_MIN_LENGTH];
        bytes[0..8].copy_from_slice(&RSDP_SIGNATURE);
        bytes[9..15].copy_from_slice(b"SVMVIS");
        bytes[15] = 2;
        bytes[16..20].copy_from_slice(&rsdt.to_le_bytes());
        bytes[20..24].copy_from_slice(&(RSDP_V2_MIN_LENGTH as u32).to_le_bytes());
        bytes[24..32].copy_from_slice(&xsdt.to_le_bytes());
        repair_checksum(&mut bytes[..RSDP_V1_LENGTH], 8);
        repair_checksum(&mut bytes, 32);
        bytes
    }

    fn processor_record(processor_id: u64, enabled: bool) -> ProcessorRecord {
        ProcessorRecord {
            processor_number: processor_id as usize,
            processor_id,
            is_bsp: processor_id == 0,
            enabled,
            healthy: true,
            package: 0,
            core: processor_id as u32,
            thread: 0,
        }
    }

    #[test]
    fn default_limits_match_the_evidence_verifier_contract() {
        assert_eq!(
            AcpiLimits::default(),
            AcpiLimits {
                max_rsdp_length: 4 * 1024,
                max_table_length: 1024 * 1024,
                max_root_entries: 256,
                max_madt_entries: 512,
                max_madt_processors: 256,
                max_mcfg_allocations: 256,
                max_ivrs_entries: 256,
                max_ivhd_device_entries: 4 * 1024,
                max_mp_processors: 256,
            }
        );
    }

    #[test]
    fn parses_rsdp_v1_and_v2_and_retains_declared_raw_bytes() {
        let limits = AcpiLimits::default();
        let mut v1 = vec![0_u8; RSDP_V1_LENGTH + 8];
        v1[0..8].copy_from_slice(&RSDP_SIGNATURE);
        v1[9..15].copy_from_slice(b"SVMVIS");
        v1[16..20].copy_from_slice(&0x1234_u32.to_le_bytes());
        repair_checksum(&mut v1[..RSDP_V1_LENGTH], 8);
        let parsed_v1 = Rsdp::parse(&v1, &limits).unwrap();
        assert_eq!(parsed_v1.raw_bytes().len(), RSDP_V1_LENGTH);
        assert_eq!(parsed_v1.root_pointers().unwrap().rsdt, Some(0x1234));

        let v2 = make_rsdp_v2(0x1234, 0x1_0000_2000);
        let parsed_v2 = Rsdp::parse(&v2, &limits).unwrap();
        assert_eq!(parsed_v2.revision, 2);
        assert_eq!(parsed_v2.raw_bytes(), v2);
        assert_eq!(parsed_v2.root_pointers().unwrap().xsdt, Some(0x1_0000_2000));
    }

    #[test]
    fn rejects_truncated_bad_signature_and_bad_rsdp_checksums() {
        let limits = AcpiLimits::default();
        assert!(matches!(
            Rsdp::parse(&[0; 19], &limits),
            Err(AcpiError::Truncated { .. })
        ));
        let mut bad_signature = make_rsdp_v2(0x1000, 0x2000);
        bad_signature[0] = b'X';
        assert!(matches!(
            Rsdp::parse(&bad_signature, &limits),
            Err(AcpiError::InvalidRsdpSignature { .. })
        ));
        let mut bad_v1_checksum = make_rsdp_v2(0x1000, 0x2000);
        bad_v1_checksum[10] ^= 1;
        assert_eq!(
            Rsdp::parse(&bad_v1_checksum, &limits),
            Err(AcpiError::ChecksumMismatch { structure: "RSDP" })
        );
        let mut bad_extended_checksum = make_rsdp_v2(0x1000, 0x2000);
        bad_extended_checksum[33] ^= 1;
        assert_eq!(
            Rsdp::parse(&bad_extended_checksum, &limits),
            Err(AcpiError::ChecksumMismatch {
                structure: "RSDP extended"
            })
        );
    }

    #[test]
    fn rejects_nonzero_rsdp_v2_reserved_bytes_after_checksum_validation() {
        let mut bytes = make_rsdp_v2(0x1000, 0x2000);
        bytes[34] = 1;
        repair_checksum(&mut bytes, 32);
        assert_eq!(
            Rsdp::parse(&bytes, &AcpiLimits::default()),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "RSDP v2 reserved",
                offset: 33,
                length: 3,
            })
        );
    }

    #[test]
    fn rejects_rsdp_revision_one_but_accepts_backward_compatible_later_revisions() {
        let limits = AcpiLimits::default();
        let mut revision_one = make_rsdp_v2(0x1000, 0x2000);
        revision_one[15] = 1;
        repair_checksum(&mut revision_one[..RSDP_V1_LENGTH], 8);
        repair_checksum(&mut revision_one, 32);
        assert_eq!(
            rsdp_declared_length(&revision_one, &limits),
            Err(AcpiError::UnsupportedRsdpRevision { revision: 1 })
        );
        assert_eq!(
            Rsdp::parse(&revision_one, &limits),
            Err(AcpiError::UnsupportedRsdpRevision { revision: 1 })
        );

        let mut revision_three = make_rsdp_v2(0x1000, 0x2000);
        revision_three[15] = 3;
        repair_checksum(&mut revision_three[..RSDP_V1_LENGTH], 8);
        repair_checksum(&mut revision_three, 32);
        assert_eq!(
            rsdp_declared_length(&revision_three, &limits),
            Ok(RSDP_V2_MIN_LENGTH)
        );
        assert_eq!(Rsdp::parse(&revision_three, &limits).unwrap().revision, 3);
    }

    #[test]
    fn rejects_duplicate_or_missing_root_pointers() {
        let limits = AcpiLimits::default();
        let duplicate_bytes = make_rsdp_v2(0x2000, 0x2000);
        let duplicate = Rsdp::parse(&duplicate_bytes, &limits).unwrap();
        assert_eq!(
            duplicate.root_pointers(),
            Err(AcpiError::DuplicateRootPointer { address: 0x2000 })
        );
        let missing_bytes = make_rsdp_v2(0, 0);
        let missing = Rsdp::parse(&missing_bytes, &limits).unwrap();
        assert_eq!(missing.root_pointers(), Err(AcpiError::MissingRootPointer));
    }

    #[test]
    fn validates_sdt_length_checksum_and_input_truncation() {
        let limits = AcpiLimits::default();
        let valid = make_sdt(*b"TEST", &[1, 2, 3, 4]);
        let parsed = ValidatedSdt::parse(&valid, &limits).unwrap();
        assert_eq!(parsed.header.signature, *b"TEST");
        assert_eq!(parsed.raw_bytes(), valid);

        let mut bad_checksum = valid.clone();
        bad_checksum[37] ^= 1;
        assert!(matches!(
            ValidatedSdt::parse(&bad_checksum, &limits),
            Err(AcpiError::ChecksumMismatch { .. })
        ));
        assert!(matches!(
            ValidatedSdt::parse(&valid[..valid.len() - 1], &limits),
            Err(AcpiError::Truncated { .. })
        ));
        let mut too_short = valid.clone();
        too_short[4..8].copy_from_slice(&35_u32.to_le_bytes());
        assert!(matches!(
            ValidatedSdt::parse(&too_short, &limits),
            Err(AcpiError::InvalidLength { .. })
        ));
    }

    #[test]
    fn prefix_inspection_enforces_caps_before_full_table_copies() {
        let limits = AcpiLimits {
            max_rsdp_length: 64,
            max_table_length: 128,
            ..AcpiLimits::default()
        };
        let mut rsdp = make_rsdp_v2(0x1000, 0x2000);
        rsdp[20..24].copy_from_slice(&65_u32.to_le_bytes());
        assert!(matches!(
            rsdp_declared_length(&rsdp, &limits),
            Err(AcpiError::InvalidLength { .. })
        ));

        let mut header = make_sdt(*b"TEST", &[]);
        header[4..8].copy_from_slice(&129_u32.to_le_bytes());
        assert!(matches!(
            sdt_declared_length(&header, &limits),
            Err(AcpiError::InvalidLength { .. })
        ));
        assert!(matches!(
            sdt_declared_length(&header[..35], &limits),
            Err(AcpiError::Truncated { .. })
        ));

        let madt_header = make_sdt(MADT_SIGNATURE, &[0; 8]);
        let prefix = inspect_sdt_prefix(&madt_header[..SDT_HEADER_LENGTH], &limits).unwrap();
        assert_eq!(prefix.whitelisted_kind(), Some(WhitelistedTableKind::Madt));
        assert_eq!(prefix.length, MADT_FIXED_LENGTH);
    }

    #[test]
    fn root_tables_reject_duplicate_null_overflow_and_caps() {
        let limits = AcpiLimits::default();
        let mut body = Vec::new();
        body.extend_from_slice(&0x1000_u64.to_le_bytes());
        body.extend_from_slice(&0x1000_u64.to_le_bytes());
        let duplicate = make_sdt(XSDT_SIGNATURE, &body);
        assert!(matches!(
            RootTable::parse(&duplicate, RootTableKind::Xsdt, &limits),
            Err(AcpiError::DuplicateTablePointer { .. })
        ));

        let null = make_sdt(RSDT_SIGNATURE, &0_u32.to_le_bytes());
        assert!(matches!(
            RootTable::parse(&null, RootTableKind::Rsdt, &limits),
            Err(AcpiError::NullPointer { .. })
        ));

        let overflow = make_sdt(XSDT_SIGNATURE, &(u64::MAX - 8).to_le_bytes());
        assert!(matches!(
            RootTable::parse(&overflow, RootTableKind::Xsdt, &limits),
            Err(AcpiError::ArithmeticOverflow { .. })
        ));

        let capped = AcpiLimits {
            max_root_entries: 1,
            ..limits
        };
        let mut two_body = Vec::new();
        two_body.extend_from_slice(&0x1000_u64.to_le_bytes());
        two_body.extend_from_slice(&0x2000_u64.to_le_bytes());
        let two = make_sdt(XSDT_SIGNATURE, &two_body);
        assert!(matches!(
            RootTable::parse(&two, RootTableKind::Xsdt, &capped),
            Err(AcpiError::EntryLimitExceeded { .. })
        ));
    }

    #[test]
    fn root_table_rejects_misaligned_payload() {
        let table = make_sdt(RSDT_SIGNATURE, &[1, 2, 3]);
        assert!(matches!(
            RootTable::parse(&table, RootTableKind::Rsdt, &AcpiLimits::default()),
            Err(AcpiError::EntryPayloadMisaligned { .. })
        ));
    }

    fn make_madt(processors: &[(u8, u8, u32)]) -> Vec<u8> {
        let mut body = vec![0_u8; 8];
        body[0..4].copy_from_slice(&0xfee0_0000_u32.to_le_bytes());
        body[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (uid, apic_id, flags) in processors {
            body.extend_from_slice(&[0, 8, *uid, *apic_id]);
            body.extend_from_slice(&flags.to_le_bytes());
        }
        make_sdt(MADT_SIGNATURE, &body)
    }

    fn make_ivhd(entry_type: u8, device_entries: &[u8]) -> Vec<u8> {
        let header_length = match entry_type {
            0x10 => 24,
            0x11 | 0x40 => 40,
            _ => panic!("unsupported test IVHD type"),
        };
        let mut bytes = vec![0_u8; header_length];
        bytes[0] = entry_type;
        bytes[1] = 1;
        bytes[4..6].copy_from_slice(&0x42_u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&0x40_u16.to_le_bytes());
        bytes[8..16].copy_from_slice(&0xfed8_0000_u64.to_le_bytes());
        bytes.extend_from_slice(device_entries);
        let length = bytes.len() as u16;
        bytes[2..4].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    fn make_ivrs_table(iv_info: u32, entries: &[u8]) -> Vec<u8> {
        let mut body = vec![0_u8; 12];
        body[0..4].copy_from_slice(&iv_info.to_le_bytes());
        body.extend_from_slice(entries);
        make_sdt(IVRS_SIGNATURE, &body)
    }

    fn make_ivrs_table_with_ivmd(iv_info: u32, ivmd: &[u8]) -> Vec<u8> {
        let mut entries = make_ivhd(0x10, &[1, 0, 0, 0]);
        entries.extend_from_slice(ivmd);
        make_ivrs_table(iv_info, &entries)
    }

    fn set_sdt_revision(bytes: &mut [u8], revision: u8) {
        bytes[8] = revision;
        repair_checksum(bytes, 9);
    }

    fn make_ivmd(entry_type: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; 32];
        bytes[0] = entry_type;
        bytes[2..4].copy_from_slice(&32_u16.to_le_bytes());
        if matches!(entry_type, 0x21 | 0x22) {
            bytes[4..6].copy_from_slice(&1_u16.to_le_bytes());
        }
        if entry_type == 0x22 {
            bytes[6..8].copy_from_slice(&2_u16.to_le_bytes());
        }
        bytes[16..24].copy_from_slice(&0x1000_u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x1000_u64.to_le_bytes());
        bytes
    }

    fn make_f0_device_entry(uid_format: u8, uid: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0_u8; 22];
        bytes[0] = 0xf0;
        bytes[20] = uid_format;
        bytes[21] = u8::try_from(uid.len()).unwrap();
        bytes.extend_from_slice(uid);
        bytes
    }

    #[test]
    fn parses_madt_and_cross_checks_mp_services() {
        let limits = AcpiLimits::default();
        let bytes = make_madt(&[(0, 0, 1), (1, 1, 1)]);
        let madt = Madt::parse(&bytes, &limits).unwrap();
        assert_eq!(madt.entries.len(), 2);
        assert_eq!(madt.processors.len(), 2);
        assert_eq!(madt.processors[1].apic_id, 1);
        assert!(madt.processors[1].enabled());
        let mp = [processor_record(0, true), processor_record(1, true)];
        let comparison = cross_check_madt_with_mp_services(&madt, 2, 2, &mp, &limits).unwrap();
        assert!(comparison.complete_match());
    }

    #[test]
    fn parses_madt_x2apic_processor_records() {
        let mut body = vec![0_u8; 8];
        body.extend_from_slice(&[9, 16, 0, 0]);
        body.extend_from_slice(&0x1234_u32.to_le_bytes());
        body.extend_from_slice(&3_u32.to_le_bytes());
        body.extend_from_slice(&0x55_u32.to_le_bytes());
        let bytes = make_sdt(MADT_SIGNATURE, &body);
        let madt = Madt::parse(&bytes, &AcpiLimits::default()).unwrap();
        assert_eq!(
            madt.processors,
            vec![MadtProcessor {
                kind: MadtProcessorKind::LocalX2Apic,
                processor_uid: 0x55,
                apic_id: 0x1234,
                flags: 3,
            }]
        );
        assert!(madt.processors[0].online_capable());
    }

    #[test]
    fn madt_ignores_unusable_processor_entries_for_apic_id_uniqueness() {
        let limits = AcpiLimits::default();
        let bytes = make_madt(&[(0, 0, 1), (24, 0, 0), (25, 0, 0)]);
        let madt = Madt::parse(&bytes, &limits).unwrap();

        assert_eq!(madt.processors.len(), 3);
        assert_eq!(
            madt.processors
                .iter()
                .map(|processor| processor.apic_id)
                .collect::<Vec<_>>(),
            vec![0, 0, 0]
        );
        assert_eq!(
            madt.processors
                .iter()
                .map(|processor| processor.flags)
                .collect::<Vec<_>>(),
            vec![1, 0, 0]
        );

        let duplicate_usable = make_madt(&[(0, 0, 1), (1, 0, 2)]);
        assert_eq!(
            Madt::parse(&duplicate_usable, &limits),
            Err(AcpiError::DuplicateMadtApicId { apic_id: 0 })
        );

        let duplicate_uid = make_madt(&[(0, 0, 1), (24, 1, 0), (24, 1, 0)]);
        assert_eq!(
            Madt::parse(&duplicate_uid, &limits),
            Err(AcpiError::DuplicateMadtProcessorUid { processor_uid: 24 })
        );
    }

    #[test]
    fn madt_processor_entries_require_exact_lengths_and_zero_x2apic_reserved_field() {
        let limits = AcpiLimits::default();

        let mut overlong_apic_body = vec![0_u8; 8];
        overlong_apic_body.extend_from_slice(&[0, 9, 0, 0, 1, 0, 0, 0, 0]);
        let overlong_apic = make_sdt(MADT_SIGNATURE, &overlong_apic_body);
        assert_eq!(
            Madt::parse(&overlong_apic, &limits),
            Err(AcpiError::UnexpectedSubtableLength {
                table: "MADT",
                entry_type: 0,
                length: 9,
                expected: 8,
            })
        );

        let mut overlong_x2apic_body = vec![0_u8; 8];
        overlong_x2apic_body.extend_from_slice(&[9, 17, 0, 0]);
        overlong_x2apic_body.extend_from_slice(&0x1234_u32.to_le_bytes());
        overlong_x2apic_body.extend_from_slice(&1_u32.to_le_bytes());
        overlong_x2apic_body.extend_from_slice(&0x55_u32.to_le_bytes());
        overlong_x2apic_body.push(0);
        let overlong_x2apic = make_sdt(MADT_SIGNATURE, &overlong_x2apic_body);
        assert_eq!(
            Madt::parse(&overlong_x2apic, &limits),
            Err(AcpiError::UnexpectedSubtableLength {
                table: "MADT",
                entry_type: 9,
                length: 17,
                expected: 16,
            })
        );

        let mut reserved_x2apic_body = vec![0_u8; 8];
        reserved_x2apic_body.extend_from_slice(&[9, 16, 1, 0]);
        reserved_x2apic_body.extend_from_slice(&0x1234_u32.to_le_bytes());
        reserved_x2apic_body.extend_from_slice(&1_u32.to_le_bytes());
        reserved_x2apic_body.extend_from_slice(&0x55_u32.to_le_bytes());
        let reserved_x2apic = make_sdt(MADT_SIGNATURE, &reserved_x2apic_body);
        assert_eq!(
            Madt::parse(&reserved_x2apic, &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "MADT Local x2APIC reserved",
                offset: 2,
                length: 2,
            })
        );
    }

    #[test]
    fn topology_cross_check_records_differences_and_rejects_duplicate_mp_ids() {
        let limits = AcpiLimits::default();
        let bytes = make_madt(&[(0, 0, 1), (1, 1, 0)]);
        let madt = Madt::parse(&bytes, &limits).unwrap();
        let mp = [processor_record(0, false), processor_record(2, true)];
        let comparison = cross_check_madt_with_mp_services(&madt, 2, 1, &mp, &limits).unwrap();
        assert!(!comparison.complete_match());
        assert_eq!(comparison.madt_only_apic_ids, vec![1]);
        assert_eq!(comparison.mp_only_processor_ids, vec![2]);
        assert_eq!(comparison.enabled_mismatches.len(), 1);

        let duplicate = [processor_record(0, true), processor_record(0, true)];
        assert_eq!(
            cross_check_madt_with_mp_services(&madt, 2, 2, &duplicate, &limits),
            Err(AcpiError::DuplicateMpProcessorId { processor_id: 0 })
        );
    }

    #[test]
    fn rejects_malformed_truncated_and_duplicate_madt_entries() {
        let limits = AcpiLimits::default();
        let mut malformed_body = vec![0_u8; 8];
        malformed_body.extend_from_slice(&[0, 1]);
        let malformed = make_sdt(MADT_SIGNATURE, &malformed_body);
        assert!(matches!(
            Madt::parse(&malformed, &limits),
            Err(AcpiError::InvalidSubtableLength { .. })
        ));

        let mut truncated_body = vec![0_u8; 8];
        truncated_body.extend_from_slice(&[0, 8, 0, 0]);
        let truncated = make_sdt(MADT_SIGNATURE, &truncated_body);
        assert!(matches!(
            Madt::parse(&truncated, &limits),
            Err(AcpiError::Truncated { .. })
        ));

        let duplicate = make_madt(&[(0, 3, 1), (1, 3, 1)]);
        assert_eq!(
            Madt::parse(&duplicate, &limits),
            Err(AcpiError::DuplicateMadtApicId { apic_id: 3 })
        );
    }

    #[test]
    fn parses_mcfg_ivrs_and_fadt_without_accessing_described_hardware() {
        let limits = AcpiLimits::default();
        let mut mcfg_body = vec![0_u8; 8];
        mcfg_body.extend_from_slice(&0xe000_0000_u64.to_le_bytes());
        mcfg_body.extend_from_slice(&0_u16.to_le_bytes());
        mcfg_body.extend_from_slice(&[0, 0x7f]);
        mcfg_body.extend_from_slice(&0_u32.to_le_bytes());
        let mcfg_bytes = make_sdt(MCFG_SIGNATURE, &mcfg_body);
        let mcfg = Mcfg::parse(&mcfg_bytes, &limits).unwrap();
        assert_eq!(mcfg.allocations.len(), 1);
        assert_eq!(mcfg.allocations[0].address_range.length(), 128 << 20);

        let mut device_entries = vec![0x01, 0, 0, 0];
        device_entries.extend_from_slice(&[0x42, 0, 0, 0, 0, 0, 0, 0]);
        let mut f0 = vec![0_u8; 22];
        f0[0] = 0xf0;
        f0[20] = 2;
        f0[21] = 3;
        f0.extend_from_slice(b"UID");
        device_entries.extend_from_slice(&f0);
        let mut ivrs_body = vec![0_u8; 12];
        ivrs_body[0..4].copy_from_slice(&0x1201_u32.to_le_bytes());
        ivrs_body.extend_from_slice(&make_ivhd(0x40, &device_entries));
        let mut ivmd = vec![0_u8; 32];
        ivmd[0] = 0x21;
        ivmd[2..4].copy_from_slice(&32_u16.to_le_bytes());
        ivmd[4..6].copy_from_slice(&1_u16.to_le_bytes());
        ivmd[8..10].copy_from_slice(&3_u16.to_le_bytes());
        ivmd[16..24].copy_from_slice(&0x1000_u64.to_le_bytes());
        ivmd[24..32].copy_from_slice(&0x2000_u64.to_le_bytes());
        ivrs_body.extend_from_slice(&ivmd);
        let ivrs_bytes = make_sdt(IVRS_SIGNATURE, &ivrs_body);
        let ivrs = Ivrs::parse(&ivrs_bytes, &limits).unwrap();
        assert_eq!(ivrs.iv_info, 0x1201);
        let IvrsEntry::Ivhd(ivhd) = &ivrs.entries[0] else {
            panic!("expected IVHD");
        };
        assert_eq!(ivhd.device_entries.len(), 3);
        assert_eq!(ivhd.device_entries[0].raw_bytes().len(), 4);
        assert_eq!(ivhd.device_entries[1].raw_bytes().len(), 8);
        assert_eq!(ivhd.device_entries[2].raw_bytes().len(), 25);
        let IvrsEntry::Ivmd(ivmd) = &ivrs.entries[1] else {
            panic!("expected IVMD");
        };
        assert_eq!(ivmd.pci_segment_group, Some(3));
        assert_eq!(ivmd.memory_range.length(), 0x2000);

        let mut fadt_body = vec![0_u8; 148 - SDT_HEADER_LENGTH];
        fadt_body[4..8].copy_from_slice(&0x1234_u32.to_le_bytes());
        fadt_body[73..75].copy_from_slice(&0x55aa_u16.to_le_bytes());
        fadt_body[95] = 7;
        fadt_body[104..112].copy_from_slice(&0x1_0000_2000_u64.to_le_bytes());
        let fadt_bytes = make_sdt(FADT_SIGNATURE, &fadt_body);
        let fadt = Fadt::parse(&fadt_bytes, &limits).unwrap();
        assert_eq!(fadt.dsdt_address(), Some(0x1_0000_2000));
        assert_eq!(fadt.iapc_boot_arch, Some(0x55aa));
        assert_eq!(fadt.minor_version, Some(7));

        let short_fadt_bytes =
            make_sdt(FADT_SIGNATURE, &[0_u8; FADT_V1_LENGTH - SDT_HEADER_LENGTH]);
        let short_fadt = Fadt::parse(&short_fadt_bytes, &limits).unwrap();
        assert_eq!(short_fadt.iapc_boot_arch, Some(0));
        assert_eq!(short_fadt.minor_version, None);
    }

    #[test]
    fn mcfg_base_is_relative_to_bus_zero_for_nonzero_start_bus() {
        let mut body = vec![0_u8; 8];
        body.extend_from_slice(&0xe000_0000_u64.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&[0x80, 0x8f]);
        body.extend_from_slice(&0_u32.to_le_bytes());

        let table = make_sdt(MCFG_SIGNATURE, &body);
        let mcfg = Mcfg::parse(&table, &AcpiLimits::default()).unwrap();

        assert_eq!(
            mcfg.allocations[0].address_range,
            PhysicalRange {
                start: 0xe800_0000,
                end_exclusive: 0xe900_0000,
            }
        );
    }

    #[test]
    fn rejects_mcfg_range_errors_and_truncated_ivrs_entries() {
        let limits = AcpiLimits::default();
        let mut bad_bus_body = vec![0_u8; 8];
        bad_bus_body.extend_from_slice(&0xe000_0000_u64.to_le_bytes());
        bad_bus_body.extend_from_slice(&0_u16.to_le_bytes());
        bad_bus_body.extend_from_slice(&[5, 4]);
        bad_bus_body.extend_from_slice(&0_u32.to_le_bytes());
        let bad_bus = make_sdt(MCFG_SIGNATURE, &bad_bus_body);
        assert!(matches!(
            Mcfg::parse(&bad_bus, &limits),
            Err(AcpiError::InvalidMcfgBusRange { .. })
        ));

        let mut overflow_body = vec![0_u8; 8];
        overflow_body.extend_from_slice(&(!((1_u64 << 20) - 1)).to_le_bytes());
        overflow_body.extend_from_slice(&0_u16.to_le_bytes());
        overflow_body.extend_from_slice(&[0, 1]);
        overflow_body.extend_from_slice(&0_u32.to_le_bytes());
        let overflow = make_sdt(MCFG_SIGNATURE, &overflow_body);
        assert!(matches!(
            Mcfg::parse(&overflow, &limits),
            Err(AcpiError::ArithmeticOverflow { .. })
        ));

        let mut ivrs_body = vec![0_u8; 12];
        ivrs_body.extend_from_slice(&[0x10, 0, 8, 0]);
        let ivrs = make_sdt(IVRS_SIGNATURE, &ivrs_body);
        assert!(matches!(
            Ivrs::parse(&ivrs, &limits),
            Err(AcpiError::Truncated { .. })
        ));
    }

    #[test]
    fn mcfg_requires_zero_table_and_allocation_reserved_fields() {
        let limits = AcpiLimits::default();
        let mut table_reserved_body = vec![0_u8; 8];
        table_reserved_body[3] = 1;
        let table_reserved = make_sdt(MCFG_SIGNATURE, &table_reserved_body);
        assert_eq!(
            Mcfg::parse(&table_reserved, &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "MCFG table reserved",
                offset: 36,
                length: 8,
            })
        );

        let mut allocation_body = vec![0_u8; 8];
        allocation_body.extend_from_slice(&0xe000_0000_u64.to_le_bytes());
        allocation_body.extend_from_slice(&0_u16.to_le_bytes());
        allocation_body.extend_from_slice(&[0, 0x7f]);
        allocation_body.extend_from_slice(&1_u32.to_le_bytes());
        let allocation_reserved = make_sdt(MCFG_SIGNATURE, &allocation_body);
        assert_eq!(
            Mcfg::parse(&allocation_reserved, &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "MCFG allocation reserved",
                offset: 12,
                length: 4,
            })
        );
    }

    #[test]
    fn mcfg_rejects_overlapping_bus_ranges_within_one_segment() {
        let mut body = vec![0_u8; 8];
        body.extend_from_slice(&0xe000_0000_u64.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&[0, 0x7f]);
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0xf000_0000_u64.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&[0x7f, 0xff]);
        body.extend_from_slice(&0_u32.to_le_bytes());
        let table = make_sdt(MCFG_SIGNATURE, &body);
        assert_eq!(
            Mcfg::parse(&table, &AcpiLimits::default()),
            Err(AcpiError::OverlappingMcfgBusRange {
                segment_group: 0,
                first_index: 0,
                duplicate_index: 1,
            })
        );
    }

    #[test]
    fn ivrs_rejects_reserved_variable_device_types_and_bad_ivmd_lengths() {
        let limits = AcpiLimits::default();
        let mut reserved_body = vec![0_u8; 12];
        reserved_body.extend_from_slice(&make_ivhd(0x10, &[0x80, 0, 0, 0]));
        let reserved = make_sdt(IVRS_SIGNATURE, &reserved_body);
        assert_eq!(
            Ivrs::parse(&reserved, &limits),
            Err(AcpiError::IvhdDeviceEntryNotAllowed {
                ivhd_type: 0x10,
                entry_type: 0x80,
            })
        );

        let mut short_f0 = vec![0_u8; 22];
        short_f0[0] = 0xf0;
        short_f0[21] = 1;
        let mut short_f0_body = vec![0_u8; 12];
        short_f0_body[0] = 1;
        short_f0_body.extend_from_slice(&make_ivhd(0x40, &short_f0));
        let short_f0_table = make_sdt(IVRS_SIGNATURE, &short_f0_body);
        assert!(matches!(
            Ivrs::parse(&short_f0_table, &limits),
            Err(AcpiError::Truncated { .. })
        ));

        let mut short_ivmd = vec![0_u8; 31];
        short_ivmd[0] = 0x20;
        short_ivmd[2..4].copy_from_slice(&31_u16.to_le_bytes());
        let mut short_ivmd_body = vec![0_u8; 12];
        short_ivmd_body.extend_from_slice(&make_ivhd(0x10, &[1, 0, 0, 0]));
        short_ivmd_body.extend_from_slice(&short_ivmd);
        let short_ivmd_table = make_sdt(IVRS_SIGNATURE, &short_ivmd_body);
        assert!(matches!(
            Ivrs::parse(&short_ivmd_table, &limits),
            Err(AcpiError::UnexpectedSubtableLength {
                table: "IVMD",
                entry_type: 0x20,
                length: 31,
                expected: 32,
            })
        ));
    }

    #[test]
    fn type_40_ivhd_enforces_device_entry_order_and_uid_format_contract() {
        let limits = AcpiLimits::default();

        let mut variable_then_fixed = make_f0_device_entry(0, &[]);
        variable_then_fixed.extend_from_slice(&[1, 0, 0, 0]);
        let out_of_order = make_ivhd(0x40, &variable_then_fixed);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &out_of_order), &limits),
            Err(AcpiError::IvhdFixedDeviceEntryAfterVariable {
                offset: IVRS_FIXED_LENGTH + 40 + 22,
                entry_type: 1,
            })
        );

        let reserved_format = make_ivhd(0x40, &make_f0_device_entry(3, &[]));
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &reserved_format), &limits),
            Err(AcpiError::InvalidIvhdUidFormat { uid_format: 3 })
        );

        let absent_uid_with_bytes = make_ivhd(0x40, &make_f0_device_entry(0, b"X"));
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &absent_uid_with_bytes), &limits),
            Err(AcpiError::InvalidIvhdUidLength {
                uid_format: 0,
                uid_length: 1,
            })
        );

        let present_uid_without_bytes = make_ivhd(0x40, &make_f0_device_entry(2, &[]));
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &present_uid_without_bytes), &limits),
            Err(AcpiError::InvalidIvhdUidLength {
                uid_format: 2,
                uid_length: 0,
            })
        );
    }

    #[test]
    fn ivrs_enforces_revision_ivinfo_and_table_reserved_contract() {
        let limits = AcpiLimits::default();
        let fixed_entry = [1, 0, 0, 0];
        let type_10 = make_ivhd(0x10, &fixed_entry);

        let mut bad_revision = make_ivrs_table(0, &type_10);
        set_sdt_revision(&mut bad_revision, 3);
        assert_eq!(
            Ivrs::parse(&bad_revision, &limits),
            Err(AcpiError::InvalidIvrsRevision { revision: 3 })
        );

        let type_40 = make_ivhd(0x40, &fixed_entry);
        let mut revision_one_type_40 = make_ivrs_table(1, &type_40);
        set_sdt_revision(&mut revision_one_type_40, 1);
        assert_eq!(
            Ivrs::parse(&revision_one_type_40, &limits),
            Err(AcpiError::IvrsRevisionDoesNotAllowIvhd {
                revision: 1,
                entry_type: 0x40,
            })
        );

        let ivinfo_reserved = make_ivrs_table(1 << 2, &type_10);
        assert_eq!(
            Ivrs::parse(&ivinfo_reserved, &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVRS IVinfo reserved bits",
                value: 1 << 2,
                reserved_mask: 0xff80_001c,
            })
        );

        let mut reserved_body = vec![0_u8; 12];
        reserved_body[4] = 1;
        let table_reserved = make_sdt(IVRS_SIGNATURE, &reserved_body);
        assert_eq!(
            Ivrs::parse(&table_reserved, &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "IVRS table reserved",
                offset: 40,
                length: 8,
            })
        );
    }

    #[test]
    fn ivrs_requires_ivhd_coverage_and_orders_ivmd_after_an_ivhd() {
        let limits = AcpiLimits::default();
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &[]), &limits),
            Err(AcpiError::IvrsMissingIvhd)
        );

        let ivmd = make_ivmd(0x20);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &ivmd), &limits),
            Err(AcpiError::IvmdWithoutPrecedingIvhd {
                offset: IVRS_FIXED_LENGTH,
                entry_type: 0x20,
            })
        );

        let empty_ivhd = make_ivhd(0x10, &[]);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &empty_ivhd), &limits),
            Err(AcpiError::IvhdMissingDeviceEntry {
                offset: IVRS_FIXED_LENGTH,
                entry_type: 0x10,
            })
        );

        let mut repeated_ivmds = make_ivhd(0x10, &[1, 0, 0, 0]);
        repeated_ivmds.extend_from_slice(&make_ivmd(0x20));
        repeated_ivmds.extend_from_slice(&make_ivmd(0x21));
        let repeated_ivmd_table = make_ivrs_table(0, &repeated_ivmds);
        let parsed = Ivrs::parse(&repeated_ivmd_table, &limits).unwrap();
        assert_eq!(parsed.entries.len(), 3);
    }

    #[test]
    fn ivhd_enforces_documented_header_encodings_and_reserved_masks() {
        let limits = AcpiLimits::default();
        let fixed_entry = [1, 0, 0, 0];

        let mut bad_capability = make_ivhd(0x10, &fixed_entry);
        bad_capability[6..8].copy_from_slice(&0x42_u16.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &bad_capability), &limits),
            Err(AcpiError::InvalidIvhdCapabilityOffset { offset: 0x42 })
        );

        let mut bad_base = make_ivhd(0x10, &fixed_entry);
        bad_base[8..16].copy_from_slice(&0xfed8_0001_u64.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &bad_base), &limits),
            Err(AcpiError::MisalignedIvhdBase {
                address: 0xfed8_0001,
            })
        );

        let mut bad_info = make_ivhd(0x10, &fixed_entry);
        bad_info[18..20].copy_from_slice(&0x20_u16.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &bad_info), &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVHD IOMMU info reserved bits",
                value: 0x20,
                reserved_mask: 0xe0e0,
            })
        );

        let mut bad_flags = make_ivhd(0x11, &fixed_entry);
        bad_flags[1] = 0x40;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &bad_flags), &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVHD type 11h/40h flags reserved bits",
                value: 0x40,
                reserved_mask: 0xc0,
            })
        );

        let mut bad_segment = make_ivhd(0x11, &fixed_entry);
        bad_segment[16..18].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &bad_segment), &limits),
            Err(AcpiError::InvalidIvhdPciSegmentGroup {
                entry_type: 0x11,
                segment_group: 1,
            })
        );

        let mut bad_attributes = make_ivhd(0x11, &fixed_entry);
        bad_attributes[20..24].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(1, &bad_attributes), &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVHD type 11h/40h attributes reserved bits",
                value: 2,
                reserved_mask: 0xf000_1ffe,
            })
        );

        let type_11 = make_ivhd(0x11, &fixed_entry);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &type_11), &limits),
            Err(AcpiError::IvhdRequiresEfrSupport { entry_type: 0x11 })
        );

        let reserved_device_type = make_ivhd(0x10, &[0x05, 0, 0, 0]);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &reserved_device_type), &limits),
            Err(AcpiError::UnsupportedIvhdDeviceEntryType { entry_type: 0x05 })
        );

        let bad_extended_setting = make_ivhd(0x10, &[0x46, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &bad_extended_setting), &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVHD extended DTE setting reserved bits",
                value: 8,
                reserved_mask: 0x7fff_fff8,
            })
        );

        let bad_special_variety = make_ivhd(0x10, &[0x48, 0, 0, 0, 0, 0, 0, 3]);
        assert_eq!(
            Ivrs::parse(&make_ivrs_table(0, &bad_special_variety), &limits),
            Err(AcpiError::UnsupportedIvhdSpecialDeviceVariety { variety: 3 })
        );
    }

    #[test]
    fn ivmd_enforces_flags_reserved_fields_and_device_ranges() {
        let limits = AcpiLimits::default();

        let mut bad_flag_bits = make_ivmd(0x20);
        bad_flag_bits[1] = 0x10;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &bad_flag_bits), &limits),
            Err(AcpiError::NonZeroReservedBits {
                structure: "IVMD flags reserved bits",
                value: 0x10,
                reserved_mask: 0xf0,
            })
        );

        let mut invalid_unity = make_ivmd(0x20);
        invalid_unity[1] = 1;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &invalid_unity), &limits),
            Err(AcpiError::InvalidIvmdFlagCombination { flags: 1 })
        );

        let mut type_20_reserved = make_ivmd(0x20);
        type_20_reserved[4] = 1;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &type_20_reserved), &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "IVMD type 20h reserved fields",
                offset: 4,
                length: 12,
            })
        );

        let mut type_21_reserved = make_ivmd(0x21);
        type_21_reserved[6] = 1;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &type_21_reserved), &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "IVMD type 21h auxiliary field",
                offset: 6,
                length: 2,
            })
        );

        let mut type_22_reserved = make_ivmd(0x22);
        type_22_reserved[10] = 1;
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &type_22_reserved), &limits),
            Err(AcpiError::NonZeroReservedBytes {
                structure: "IVMD type 22h reserved field",
                offset: 10,
                length: 6,
            })
        );

        let mut reversed_range = make_ivmd(0x22);
        reversed_range[4..6].copy_from_slice(&3_u16.to_le_bytes());
        assert_eq!(
            Ivrs::parse(&make_ivrs_table_with_ivmd(0, &reversed_range), &limits),
            Err(AcpiError::InvalidIvmdDeviceRange {
                start_device_id: 3,
                end_device_id: 2,
            })
        );
    }

    #[test]
    fn ivmd_rejects_zero_memory_length() {
        let mut zero_length = make_ivmd(0x21);
        zero_length[24..32].fill(0);
        assert_eq!(
            Ivrs::parse(
                &make_ivrs_table_with_ivmd(0, &zero_length),
                &AcpiLimits::default(),
            ),
            Err(AcpiError::ZeroIvmdMemoryLength { entry_type: 0x21 })
        );
    }

    #[test]
    fn parses_extended_ivhd_headers_from_amd_rev_3_11() {
        let mut type_11 = make_ivhd(0x11, &[1, 0, 0, 0]);
        type_11[24..32].copy_from_slice(&0x1122_u64.to_le_bytes());
        type_11[32..40].copy_from_slice(&0x3344_u64.to_le_bytes());
        let mut type_40 = make_ivhd(0x40, &[1, 0, 0, 0]);
        type_40[24..32].copy_from_slice(&0x5566_u64.to_le_bytes());
        type_40[32..40].copy_from_slice(&0x7788_u64.to_le_bytes());
        let mut body = vec![0_u8; 12];
        body[0] = 1;
        body.extend_from_slice(&type_11);
        body.extend_from_slice(&type_40);
        let bytes = make_sdt(IVRS_SIGNATURE, &body);
        let ivrs = Ivrs::parse(&bytes, &AcpiLimits::default()).unwrap();
        let IvrsEntry::Ivhd(first) = &ivrs.entries[0] else {
            panic!("expected IVHD");
        };
        let IvrsEntry::Ivhd(second) = &ivrs.entries[1] else {
            panic!("expected IVHD");
        };
        assert_eq!(first.extended_feature_image, Some(0x1122));
        assert_eq!(first.extended_feature_image_2, Some(0x3344));
        assert_eq!(second.extended_feature_image, Some(0x5566));
        assert_eq!(second.extended_feature_image_2, Some(0x7788));
    }

    #[test]
    fn whitelist_rejects_non_slice_tables_and_raw_bytes_are_exact() {
        let limits = AcpiLimits::default();
        let hpet = make_sdt(*b"HPET", &[]);
        assert_eq!(
            parse_whitelisted_table(&hpet, &limits),
            Err(AcpiError::UnsupportedTableSignature { found: *b"HPET" })
        );
        let madt_bytes = make_madt(&[(0, 0, 1)]);
        let parsed = parse_whitelisted_table(&madt_bytes, &limits).unwrap();
        assert_eq!(parsed.raw_bytes(), madt_bytes);
    }

    #[test]
    fn checked_pointer_arithmetic_rejects_wraparound() {
        assert_eq!(
            checked_physical_range(u64::MAX - 7, 8, "test"),
            Err(AcpiError::ArithmeticOverflow { structure: "test" })
        );
        assert_eq!(
            checked_physical_range(0x1000, 0x20, "test").unwrap(),
            PhysicalRange {
                start: 0x1000,
                end_exclusive: 0x1020
            }
        );
    }
}
