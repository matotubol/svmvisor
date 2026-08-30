//! Canonical M0b inventory evidence model and JSON encoder.

use crate::cpuid::{CpuInventory, CpuidRegisters, extended_apic_id, initial_apic_id};
use crate::iommu::locator::{
    IommuUnit, IvhdInput, McfgInput, McfgWitness, PciBdf, derive_unique_mcfg_witness,
    derive_unique_unit,
};
use crate::iommu::ranges::{
    ConfiguredRange, DescriptorWitness, MemoryDescriptor, PhysicalRange, ecam_descriptor_witness,
    iommu_aperture_witness, memory_descriptor_witness, validate_enabled_configured_range,
};
use crate::iommu::registers::{
    BASE_STABLE_MMIO_OFFSETS, CapabilityLink, DEVICE_TABLE_SEGMENT_1_OFFSET, DecodedLiveState,
    EXTENDED_FEATURE_2_OFFSET, EXTENDED_FEATURE_OFFSET, ExtendedFeatureComparison,
    ExtendedFeatureImages, MmioReadPlan, PciCapability, PciCapabilityRaw, PciIdentity,
    PciIdentityRaw, STATUS_OFFSET, StableMmioSnapshot, build_mmio_read_plan,
    compare_extended_features, decode_and_validate_capability, decode_and_validate_identity,
    decode_live_state, require_stable_mmio_snapshot, require_stable_pci_capability,
    validate_capability_chain,
};
use crate::msr::{
    SystemRegisterInventory, SystemRegistersEvidence, decode_hwcr, decode_iorr_base,
    decode_iorr_mask, decode_mtrr_cap, decode_mtrr_def_type, decode_smm_mask, decode_sys_cfg,
    decode_variable_mtrr_base, decode_variable_mtrr_mask, expected_read_operations,
    smm_base_address, smm_tseg_base, tom2_address, top_mem_address, validate_inventory,
};
use crate::processor::{ProcessorConsistencyEvidence, ProcessorDispatch};
use alloc::{string::String, vec::Vec};
use core::fmt::{self, Write};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u16 = 6;
pub const COLLECTOR_VERSION: &str = "0.6.0";
pub const COLLECTOR_SLICE: &str = "read-only-per-processor-system-register-inventory";
pub const EVIDENCE_KIND: &str = "uefi-record-only-inventory-slice";
pub const PROFILE_BINDING_FILE: &str = "\\svmvisor-m0b-target.txt";
pub const PROFILE_BINDING_PREFIX: &[u8] = b"svmvisor-m0b-target-v1\n";
pub const MAX_RENDERED_JSON_BYTES: usize = 16 * 1024 * 1024;

const AMD_PPR_PUBLICATION: &str = "57896";
const AMD_PPR_REVISION: &str = "3.00";
const AMD_PPR_DATE: &str = "August 28, 2024";
const AMD_PPR_COVERAGE: &str = "AMD Family 1Ah Model 44h B0";
const AMD_PPR_SHA256: &str = "643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5";

const AMD_IOMMU_SPEC_PUBLISHER: &str = "AMD";
const AMD_IOMMU_SPEC_PUBLICATION: &str = "48882";
const AMD_IOMMU_SPEC_REVISION: &str = "3.11";
const AMD_IOMMU_SPEC_DATE: &str = "April 2026";
const AMD_IOMMU_SPEC_SHA256: &str =
    "f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22";

/// The selected image volume, checked before any evidence write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SinkRecord {
    pub media_id: u32,
    pub removable_media: bool,
    pub media_present: bool,
    pub logical_partition: bool,
    pub read_only: bool,
    pub block_size: u32,
    pub last_block: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SinkAuthorizationError {
    NotRemovable,
    MediaAbsent,
    ReadOnly,
    InvalidBlockSize,
}

impl SinkRecord {
    /// Authorize only the loaded image's present, writable, removable volume.
    pub fn authorize(self) -> Result<(), SinkAuthorizationError> {
        if !self.removable_media {
            return Err(SinkAuthorizationError::NotRemovable);
        }
        if !self.media_present {
            return Err(SinkAuthorizationError::MediaAbsent);
        }
        if self.read_only {
            return Err(SinkAuthorizationError::ReadOnly);
        }
        if self.block_size == 0 {
            return Err(SinkAuthorizationError::InvalidBlockSize);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimestampRecord {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
    pub timezone_minutes: Option<i16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigTableRecord<'a> {
    pub kind: &'a str,
    pub guid: &'a str,
    pub address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessorRecord {
    pub processor_number: usize,
    pub processor_id: u64,
    pub is_bsp: bool,
    pub enabled: bool,
    pub healthy: bool,
    pub package: u32,
    pub core: u32,
    pub thread: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpServicesEvidence<'a> {
    Observed {
        total: usize,
        enabled: usize,
        processors: &'a [ProcessorRecord],
    },
    Unavailable {
        uefi_status: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryDescriptorRecord {
    pub memory_type: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub page_count: u64,
    pub attributes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryMapEvidence<'a> {
    Observed {
        descriptor_size: usize,
        descriptor_version: u32,
        descriptors: &'a [MemoryDescriptorRecord],
    },
    Unavailable {
        uefi_status: usize,
    },
}

/// Only the architecturally enumerated VM_CR MSR is readable in this slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VmCrEvidence<'a> {
    Observed(u64),
    NotAttempted { reason: &'a str },
}

/// Raw firmware bytes retained with their length and SHA-256 digest.
///
/// The JSON encoder derives the envelope metadata from `bytes`; callers cannot
/// provide a digest or length that disagrees with the retained bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawBytesEvidence<'a> {
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiSdtHeaderEvidence {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpiRootReferences {
    Rsdt,
    Xsdt,
    RsdtAndXsdt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsdpEvidence<'a> {
    pub address: u64,
    pub raw: RawBytesEvidence<'a>,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub length: u32,
    pub rsdt_address: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiRootEvidence<'a> {
    pub address: u64,
    pub raw: RawBytesEvidence<'a>,
    pub header: AcpiSdtHeaderEvidence,
    pub entries: &'a [u64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiDirectoryRecord<'a> {
    pub address: u64,
    pub referenced_by: AcpiRootReferences,
    pub header_raw: RawBytesEvidence<'a>,
    pub signature: [u8; 4],
    pub declared_length: u32,
    pub revision: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MadtEntryEvidence<'a> {
    ProcessorLocalApic {
        offset: usize,
        raw: &'a [u8],
        acpi_processor_uid: u8,
        apic_id: u8,
        flags: u32,
    },
    ProcessorLocalX2Apic {
        offset: usize,
        raw: &'a [u8],
        reserved: u16,
        x2apic_id: u32,
        flags: u32,
        acpi_processor_uid: u32,
    },
    Other {
        entry_type: u8,
        offset: usize,
        raw: &'a [u8],
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MadtBodyEvidence<'a> {
    pub local_apic_address: u32,
    pub flags: u32,
    pub entries: &'a [MadtEntryEvidence<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgAllocationEvidence {
    pub offset: usize,
    pub base_address: u64,
    pub segment_group: u16,
    pub start_bus: u8,
    pub end_bus: u8,
    pub reserved: [u8; 4],
    pub window_end_exclusive: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgBodyEvidence<'a> {
    pub reserved: [u8; 8],
    pub allocations: &'a [McfgAllocationEvidence],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvhdDeviceEntryEvidence<'a> {
    pub offset: usize,
    pub entry_type: u8,
    pub raw: &'a [u8],
    pub uid_length: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvhdEvidence<'a> {
    pub offset: usize,
    pub entry_type: u8,
    pub flags: u8,
    pub raw: &'a [u8],
    pub header_length: usize,
    pub device_id: u16,
    pub capability_offset: u16,
    pub iommu_base_address: u64,
    pub pci_segment_group: u16,
    pub iommu_info: u16,
    pub feature_info: u32,
    pub extended_feature_image: Option<u64>,
    pub extended_feature_image_2: Option<u64>,
    pub device_entries: &'a [IvhdDeviceEntryEvidence<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvmdEvidence<'a> {
    pub offset: usize,
    pub entry_type: u8,
    pub flags: u8,
    pub raw: &'a [u8],
    pub device_id: u16,
    pub auxiliary_data_or_end_device_id: u16,
    pub pci_segment_group: Option<u16>,
    pub reserved_or_segment_area: [u8; 8],
    pub start_address: u64,
    pub memory_length: u64,
    pub memory_end_exclusive: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IvrsBlockEvidence<'a> {
    Ivhd(IvhdEvidence<'a>),
    Ivmd(IvmdEvidence<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvrsBodyEvidence<'a> {
    pub iv_info: u32,
    pub reserved: [u8; 8],
    pub blocks: &'a [IvrsBlockEvidence<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FadtBodyEvidence {
    pub firmware_ctrl_32: u32,
    pub dsdt_32: u32,
    pub preferred_pm_profile: u8,
    pub sci_interrupt: u16,
    pub iapc_boot_arch: u16,
    pub flags: u32,
    pub minor_version: u8,
    pub x_firmware_ctrl: u64,
    pub x_dsdt: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiTableEvidence<'a, T> {
    pub address: u64,
    pub referenced_by: AcpiRootReferences,
    pub raw: RawBytesEvidence<'a>,
    pub header: AcpiSdtHeaderEvidence,
    pub body: T,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiTablesEvidence<'a> {
    pub madt: AcpiTableEvidence<'a, MadtBodyEvidence<'a>>,
    pub mcfg: AcpiTableEvidence<'a, McfgBodyEvidence<'a>>,
    pub ivrs: AcpiTableEvidence<'a, IvrsBodyEvidence<'a>>,
    pub fadt: AcpiTableEvidence<'a, FadtBodyEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MadtMpCrossCheckEvidence<'a> {
    pub mp_enabled_ids: &'a [u64],
    pub madt_enabled_ids: &'a [u64],
    pub missing_from_madt: &'a [u64],
    pub missing_from_mp: &'a [u64],
    pub consistent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiEvidence<'a> {
    pub selection_address: u64,
    pub rsdp: RsdpEvidence<'a>,
    pub rsdt: AcpiRootEvidence<'a>,
    pub xsdt: AcpiRootEvidence<'a>,
    pub directory: &'a [AcpiDirectoryRecord<'a>],
    pub tables: AcpiTablesEvidence<'a>,
    pub madt_mp_cross_check: MadtMpCrossCheckEvidence<'a>,
}

/// Audit counters and negative access claims for the live-IOMMU slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuAccessEvidence {
    pub root_bridge_handle_count: usize,
    pub matching_segment_handle_count: usize,
    pub full_match_handle_count: usize,
    pub pci_read_operations: usize,
    pub pci_read_bytes: usize,
    pub mmio_read_operations: usize,
    pub mmio_read_bytes: usize,
    pub pci_write_operations: usize,
    pub mmio_write_operations: usize,
    pub direct_ecam_access: bool,
    pub direct_mmio_access: bool,
    pub cf8_cfc_access: bool,
    pub configured_pointer_dereferences: usize,
}

/// A schema-level copy of the exact same-run UEFI descriptor that authorized
/// a bounded address range.  The descriptor is deliberately retained in the
/// existing memory-map representation so the verifier can bind it by index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryDescriptorBindingEvidence {
    pub descriptor_index: usize,
    pub descriptor: MemoryDescriptorRecord,
    pub requested_start: u64,
    pub requested_end_exclusive: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuLocatorEvidence<'a> {
    pub ivhd_sources: &'a [IvhdInput],
    pub unit: IommuUnit,
    pub mcfg_allocations: &'a [McfgInput],
    pub mcfg_witness: McfgWitness,
    pub ecam_memory_binding: MemoryDescriptorBindingEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuPciEvidence<'a> {
    pub selected_root_bridge_handle_index: usize,
    pub segment_group: u16,
    pub bdf: PciBdf,
    pub identity_raw: PciIdentityRaw,
    pub identity_decoded: PciIdentity,
    pub capability_chain: &'a [CapabilityLink],
    pub capability_first_raw: PciCapabilityRaw,
    pub capability_second_raw: PciCapabilityRaw,
    pub capability_stable: bool,
    pub capability_decoded: PciCapability,
}

/// Fields shared by the conflict and successful MMIO variants.  The stable
/// configuration set is retained twice; Status is intentionally sampled once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuMmioCommonEvidence<'a> {
    pub aperture_length_bytes: u64,
    pub aperture_memory_binding: MemoryDescriptorBindingEvidence,
    pub stable_offsets: &'a [u16],
    pub feature_dependent_offsets: &'a [u16],
    pub first_snapshot: StableMmioSnapshot,
    pub second_snapshot: StableMmioSnapshot,
    pub stable: bool,
    pub status_offset: u16,
    pub status_raw: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuConfiguredRangeEvidence {
    pub source_offset: u16,
    pub raw: u64,
    pub enabled: bool,
    pub base: u64,
    pub length: u64,
    pub alignment: u64,
    pub validated_range: Option<PhysicalRange>,
    pub memory_binding: Option<MemoryDescriptorBindingEvidence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmdIommuMmioEvidence<'a> {
    /// PCI capability Enable was clear, so no MMIO access was attempted.
    Disabled,
    /// Live EFR/EFR2 disagreed with IVRS; no feature-dependent read or decode
    /// is represented by this variant.
    EfrConflict {
        common: IommuMmioCommonEvidence<'a>,
        expected: ExtendedFeatureImages,
        live: ExtendedFeatureImages,
    },
    /// The bounded MMIO snapshot and its permitted architectural decode.
    Observed {
        common: IommuMmioCommonEvidence<'a>,
        features: ExtendedFeatureComparison,
        configured_ranges: &'a [IommuConfiguredRangeEvidence],
        decoded: DecodedLiveState,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuOwnershipEvidence {
    pub assessed: bool,
    pub claim: bool,
    pub requester_dma_isolation_claim: bool,
    pub interrupt_remapping_claim: bool,
    pub pci_isolation_claim: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmdIommuLiveEvidence<'a> {
    pub access: IommuAccessEvidence,
    pub locator: IommuLocatorEvidence<'a>,
    pub pci: IommuPciEvidence<'a>,
    pub mmio: AmdIommuMmioEvidence<'a>,
    pub ownership: IommuOwnershipEvidence,
}

/// Run-wide access counters for the allowlisted `RDMSR` sites, including the
/// pre-existing VM_CR site. Writes are structurally impossible in the adapter
/// and pinned to zero here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MsrAccessEvidence {
    pub read_operations: u64,
    pub read_bytes: u64,
    pub write_operations: u64,
}

/// The schema-v6 per-processor system-register section. The top-level value is
/// the MP-identified BSP reference, mirroring the root `cpu`/`vm_cr` objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemRegistersSection<'a> {
    pub bsp: SystemRegistersEvidence<'a>,
    pub access: MsrAccessEvidence,
}

/// Borrowed view rendered to the exact JSON bytes written by the UEFI app.
pub struct Evidence<'a> {
    pub target_profile_manifest_sha256: &'a [u8; 64],
    pub output_file: &'a str,
    pub collected_at: TimestampRecord,
    pub sink: SinkRecord,
    pub firmware_vendor: &'a str,
    pub firmware_revision: u32,
    pub uefi_revision_major: u16,
    pub uefi_revision_minor: u16,
    pub config_tables: &'a [ConfigTableRecord<'a>],
    pub cpu: &'a CpuInventory,
    pub vm_cr: VmCrEvidence<'a>,
    pub mp_services: MpServicesEvidence<'a>,
    pub processor_consistency: ProcessorConsistencyEvidence<'a>,
    pub memory_map: MemoryMapEvidence<'a>,
    pub acpi: AcpiEvidence<'a>,
    pub amd_iommu_live: AmdIommuLiveEvidence<'a>,
    pub system_registers: SystemRegistersSection<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingError {
    WrongLength,
    WrongPrefix,
    NonLowercaseHex,
    MissingFinalNewline,
}

/// Parse the exact target-profile binding marker placed on removable media.
///
/// The digest is the lowercase SHA-256 of the immutable M0a bundle's
/// `manifest.json`. No whitespace, comments, alternate casing, or trailing data
/// are accepted.
pub fn parse_profile_binding(bytes: &[u8]) -> Result<[u8; 64], BindingError> {
    let expected_length = PROFILE_BINDING_PREFIX.len() + 64 + 1;
    if bytes.len() != expected_length {
        return Err(BindingError::WrongLength);
    }
    if !bytes.starts_with(PROFILE_BINDING_PREFIX) {
        return Err(BindingError::WrongPrefix);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(BindingError::MissingFinalNewline);
    }
    let digest_slice = &bytes[PROFILE_BINDING_PREFIX.len()..bytes.len() - 1];
    if !digest_slice
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(BindingError::NonLowercaseHex);
    }
    let mut digest = [0_u8; 64];
    digest.copy_from_slice(digest_slice);
    Ok(digest)
}

/// Serialize the evidence to deterministic, pretty-printed UTF-8 JSON.
pub fn render_json(evidence: &Evidence<'_>) -> Result<String, fmt::Error> {
    validate_v6_evidence(evidence)?;
    let mut output = BoundedJson::new();
    write_evidence(&mut output, evidence)?;
    Ok(output.finish())
}

fn validate_v6_evidence(evidence: &Evidence<'_>) -> fmt::Result {
    if !evidence.cpu.is_authentic_amd() || evidence.config_tables.len() > 64 {
        return Err(fmt::Error);
    }

    let selected_sources = evidence
        .config_tables
        .iter()
        .filter(|table| {
            table.kind == "acpi2-rsdp"
                && table.guid == "8868e871-e4f1-11d3-bc22-0080c73c8881"
                && table.address == evidence.acpi.selection_address
        })
        .count();
    if selected_sources != 1 {
        return Err(fmt::Error);
    }
    for (index, table) in evidence.config_tables.iter().enumerate() {
        if evidence.config_tables[..index]
            .iter()
            .any(|prior| prior.guid == table.guid && prior.address == table.address)
        {
            return Err(fmt::Error);
        }
    }

    match evidence.mp_services {
        MpServicesEvidence::Observed {
            total,
            enabled,
            processors,
        } => {
            let enabled_records = processors
                .iter()
                .filter(|processor| processor.enabled)
                .count();
            if processors.is_empty()
                || processors.len() > 256
                || total != processors.len()
                || enabled == 0
                || enabled != enabled_records
            {
                return Err(fmt::Error);
            }
            for (index, processor) in processors.iter().enumerate() {
                if processor.processor_number != index
                    || (processor.enabled && !processor.healthy)
                    || processors[..index]
                        .iter()
                        .any(|prior| prior.processor_id == processor.processor_id)
                {
                    return Err(fmt::Error);
                }
            }
        }
        MpServicesEvidence::Unavailable { .. } => return Err(fmt::Error),
    }
    validate_processor_consistency(evidence)?;
    match evidence.memory_map {
        MemoryMapEvidence::Observed { descriptors, .. } if descriptors.len() <= 4096 => {}
        MemoryMapEvidence::Observed { .. } | MemoryMapEvidence::Unavailable { .. } => {
            return Err(fmt::Error);
        }
    }

    validate_acpi_evidence(&evidence.acpi)?;
    validate_amd_iommu_live(evidence)?;
    validate_system_registers(evidence)
}

/// Validate the schema-v6 section: pinned zero writes, byte-exact counters,
/// per-observation gate consistency, and BSP/top-level equality.
fn validate_system_registers(evidence: &Evidence<'_>) -> fmt::Result {
    let section = &evidence.system_registers;
    let access = section.access;
    if access.write_operations != 0 || access.read_bytes != access.read_operations * 8 {
        return Err(fmt::Error);
    }
    let mut expected_operations = 0_u64;
    for observation in evidence.processor_consistency.observations {
        match observation.system_registers {
            SystemRegistersEvidence::Observed(inventory) => {
                validate_inventory(&inventory).map_err(|_| fmt::Error)?;
                expected_operations = expected_operations
                    .checked_add(expected_read_operations(&inventory))
                    .ok_or(fmt::Error)?;
            }
            SystemRegistersEvidence::NotAttempted { .. } => {}
        }
    }
    if access.read_operations != expected_operations {
        return Err(fmt::Error);
    }
    let bsp = evidence
        .processor_consistency
        .bsp_observation()
        .ok_or(fmt::Error)?;
    if bsp.system_registers != section.bsp {
        return Err(fmt::Error);
    }
    Ok(())
}

fn validate_amd_iommu_live(evidence: &Evidence<'_>) -> fmt::Result {
    let live = &evidence.amd_iommu_live;
    let access = live.access;
    if access.root_bridge_handle_count == 0
        || access.root_bridge_handle_count > 64
        || access.matching_segment_handle_count == 0
        || access.matching_segment_handle_count > access.root_bridge_handle_count
        || access.full_match_handle_count != 1
        || access.pci_read_operations == 0
        || access.pci_read_operations > 4096
        || access.pci_read_bytes == 0
        || access.pci_read_bytes > access.pci_read_operations.saturating_mul(8)
        || access.pci_write_operations != 0
        || access.mmio_write_operations != 0
        || access.direct_ecam_access
        || access.direct_mmio_access
        || access.cf8_cfc_access
        || access.configured_pointer_dereferences != 0
    {
        return Err(fmt::Error);
    }
    if live.ownership.assessed
        || live.ownership.claim
        || live.ownership.requester_dma_isolation_claim
        || live.ownership.interrupt_remapping_claim
        || live.ownership.pci_isolation_claim
    {
        return Err(fmt::Error);
    }

    let locator = &live.locator;
    if locator.ivhd_sources.is_empty()
        || locator.ivhd_sources.len() > 3
        || locator.mcfg_allocations.is_empty()
        || locator.mcfg_allocations.len() > 256
        || derive_unique_unit(locator.ivhd_sources).map_err(|_| fmt::Error)? != locator.unit
        || derive_unique_mcfg_witness(&locator.unit, locator.mcfg_allocations)
            .map_err(|_| fmt::Error)?
            != locator.mcfg_witness
    {
        return Err(fmt::Error);
    }
    validate_locator_acpi_binding(locator, &evidence.acpi)?;

    let physical_width = evidence.cpu.physical_address_bits().ok_or(fmt::Error)?;
    let descriptors = memory_descriptors(evidence.memory_map)?;
    let ecam_end = locator
        .mcfg_witness
        .function_address
        .checked_add(4096)
        .ok_or(fmt::Error)?;
    let ecam_witness = ecam_descriptor_witness(
        &descriptors,
        PhysicalRange {
            start: locator.mcfg_witness.function_address,
            end_exclusive: ecam_end,
        },
        physical_width,
    )
    .map_err(|_| fmt::Error)?;
    validate_descriptor_binding(
        locator.ecam_memory_binding,
        ecam_witness,
        evidence.memory_map,
    )?;

    let pci = &live.pci;
    let terminal_capability_link = pci.capability_chain.last().ok_or(fmt::Error)?;
    let terminal_link_header = u32::from(terminal_capability_link.capability_id)
        | (u32::from(terminal_capability_link.next) << 8);
    if pci.selected_root_bridge_handle_index >= access.root_bridge_handle_count
        || pci.segment_group != locator.unit.segment_group
        || pci.bdf != locator.unit.bdf
        || decode_and_validate_identity(pci.identity_raw).map_err(|_| fmt::Error)?
            != pci.identity_decoded
        || validate_capability_chain(
            pci.identity_decoded.first_capability_pointer,
            locator.unit.capability_offset,
            pci.capability_chain,
        )
        .is_err()
        || require_stable_pci_capability(pci.capability_first_raw, pci.capability_second_raw)
            .is_err()
        || !pci.capability_stable
        || pci.capability_first_raw.header & 0xffff != terminal_link_header
        || decode_and_validate_capability(pci.capability_first_raw, locator.unit.mmio_base)
            .map_err(|_| fmt::Error)?
            != pci.capability_decoded
    {
        return Err(fmt::Error);
    }

    let selected_pci_operations = access
        .matching_segment_handle_count
        .checked_mul(5)
        .and_then(|count| count.checked_add(pci.capability_chain.len()))
        .and_then(|count| {
            count.checked_add(if pci.capability_first_raw.miscellaneous_1.is_some() {
                12
            } else {
                10
            })
        })
        .ok_or(fmt::Error)?;
    let selected_pci_bytes = access
        .matching_segment_handle_count
        .checked_mul(14)
        .ok_or(fmt::Error)?
        .checked_add(
            pci.capability_chain
                .len()
                .checked_mul(2)
                .ok_or(fmt::Error)?,
        )
        .and_then(|count| {
            count.checked_add(if pci.capability_first_raw.miscellaneous_1.is_some() {
                48
            } else {
                40
            })
        })
        .ok_or(fmt::Error)?;
    if access.pci_read_operations != selected_pci_operations
        || access.pci_read_bytes != selected_pci_bytes
    {
        return Err(fmt::Error);
    }

    match live.mmio {
        AmdIommuMmioEvidence::Disabled => {
            if pci.capability_decoded.mmio_enabled
                || access.mmio_read_operations != 0
                || access.mmio_read_bytes != 0
            {
                return Err(fmt::Error);
            }
        }
        AmdIommuMmioEvidence::EfrConflict {
            common,
            expected,
            live: live_features,
        } => {
            if !pci.capability_decoded.mmio_enabled
                || !pci.capability_decoded.extended_feature_register_supported
                || locator.unit.efr_image != Some(expected.efr)
                || locator.unit.efr2_image != Some(expected.efr2)
                || compare_extended_features(true, Some(expected), Some(live_features))
                    .map_err(|_| fmt::Error)?
                    != (ExtendedFeatureComparison::Conflict {
                        expected,
                        live: live_features,
                    })
                || common.first_snapshot.extended_feature != Some(live_features.efr)
                || common.first_snapshot.extended_feature_2 != Some(live_features.efr2)
                || !common.feature_dependent_offsets.is_empty()
            {
                return Err(fmt::Error);
            }
            let mut offsets = Vec::from(BASE_STABLE_MMIO_OFFSETS);
            offsets.push(EXTENDED_FEATURE_OFFSET);
            offsets.push(EXTENDED_FEATURE_2_OFFSET);
            if common.stable_offsets != offsets.as_slice() {
                return Err(fmt::Error);
            }
            let plan = MmioReadPlan {
                stable_offsets: offsets,
                status_offset: STATUS_OFFSET,
                active_device_table_segments: 1,
            };
            validate_mmio_common(
                evidence,
                common,
                &plan,
                conflict_requires_large_aperture(expected, live_features),
            )?;
        }
        AmdIommuMmioEvidence::Observed {
            common,
            features,
            configured_ranges,
            decoded,
        } => {
            if !pci.capability_decoded.mmio_enabled {
                return Err(fmt::Error);
            }
            let expected = match (locator.unit.efr_image, locator.unit.efr2_image) {
                (Some(efr), Some(efr2)) => Some(ExtendedFeatureImages { efr, efr2 }),
                (None, None) => None,
                _ => return Err(fmt::Error),
            };
            let snapshot_features = match (
                common.first_snapshot.extended_feature,
                common.first_snapshot.extended_feature_2,
            ) {
                (Some(efr), Some(efr2)) => Some(ExtendedFeatureImages { efr, efr2 }),
                (None, None) => None,
                _ => return Err(fmt::Error),
            };
            let calculated_features = compare_extended_features(
                pci.capability_decoded.extended_feature_register_supported,
                expected,
                snapshot_features,
            )
            .map_err(|_| fmt::Error)?;
            if calculated_features != features
                || matches!(features, ExtendedFeatureComparison::Conflict { .. })
            {
                return Err(fmt::Error);
            }
            let plan = build_mmio_read_plan(common.first_snapshot.control, features)
                .map_err(|_| fmt::Error)?;
            if common.stable_offsets != plan.stable_offsets.as_slice() {
                return Err(fmt::Error);
            }
            let expected_feature_offsets = plan
                .stable_offsets
                .iter()
                .copied()
                .filter(|offset| *offset >= DEVICE_TABLE_SEGMENT_1_OFFSET)
                .filter(|offset| *offset < EXTENDED_FEATURE_2_OFFSET)
                .collect::<Vec<_>>();
            if common.feature_dependent_offsets != expected_feature_offsets.as_slice()
                || decoded != decode_live_state(common.first_snapshot.control, common.status_raw)
            {
                return Err(fmt::Error);
            }
            validate_configured_ranges(
                configured_ranges,
                common,
                &plan,
                pci.capability_decoded.iommu_physical_address_width,
                physical_width,
                evidence.memory_map,
            )?;
            let performance_counters_supported = matches!(
                features,
                ExtendedFeatureComparison::Match {
                    performance_counters_supported: true,
                    ..
                }
            );
            validate_mmio_common(evidence, common, &plan, performance_counters_supported)?;
        }
    }
    Ok(())
}

#[must_use]
const fn conflict_requires_large_aperture(
    expected: ExtendedFeatureImages,
    live: ExtendedFeatureImages,
) -> bool {
    (expected.efr | live.efr) & (1 << 9) != 0
}

fn validate_locator_acpi_binding(
    locator: &IommuLocatorEvidence<'_>,
    acpi: &AcpiEvidence<'_>,
) -> fmt::Result {
    let expected_ivhd_indices = acpi
        .tables
        .ivrs
        .body
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| matches!(block, IvrsBlockEvidence::Ivhd(_)).then_some(index))
        .collect::<Vec<_>>();
    if locator.ivhd_sources.len() != expected_ivhd_indices.len()
        || locator.mcfg_allocations.len() != acpi.tables.mcfg.body.allocations.len()
    {
        return Err(fmt::Error);
    }
    for (source, expected_source_index) in locator
        .ivhd_sources
        .iter()
        .zip(expected_ivhd_indices.iter().copied())
    {
        if source.source_index != expected_source_index {
            return Err(fmt::Error);
        }
        let IvrsBlockEvidence::Ivhd(ivhd) = acpi
            .tables
            .ivrs
            .body
            .blocks
            .get(source.source_index)
            .ok_or(fmt::Error)?
        else {
            return Err(fmt::Error);
        };
        if source.source.entry_type() != ivhd.entry_type
            || source.segment_group != ivhd.pci_segment_group
            || source.device_id != ivhd.device_id
            || source.capability_offset != ivhd.capability_offset
            || source.mmio_base != ivhd.iommu_base_address
            || source.efr_image != ivhd.extended_feature_image
            || source.efr2_image != ivhd.extended_feature_image_2
        {
            return Err(fmt::Error);
        }
    }
    for (expected_source_index, allocation) in locator.mcfg_allocations.iter().enumerate() {
        if allocation.source_index != expected_source_index {
            return Err(fmt::Error);
        }
        let mcfg = acpi
            .tables
            .mcfg
            .body
            .allocations
            .get(allocation.source_index)
            .ok_or(fmt::Error)?;
        if allocation.base_address != mcfg.base_address
            || allocation.segment_group != mcfg.segment_group
            || allocation.start_bus != mcfg.start_bus
            || allocation.end_bus != mcfg.end_bus
        {
            return Err(fmt::Error);
        }
    }
    Ok(())
}

fn memory_descriptors(
    memory_map: MemoryMapEvidence<'_>,
) -> Result<Vec<MemoryDescriptor>, fmt::Error> {
    let MemoryMapEvidence::Observed { descriptors, .. } = memory_map else {
        return Err(fmt::Error);
    };
    Ok(descriptors
        .iter()
        .map(|descriptor| MemoryDescriptor {
            memory_type: descriptor.memory_type,
            physical_start: descriptor.physical_start,
            page_count: descriptor.page_count,
            attributes: descriptor.attributes,
        })
        .collect())
}

fn validate_descriptor_binding(
    binding: MemoryDescriptorBindingEvidence,
    witness: DescriptorWitness,
    memory_map: MemoryMapEvidence<'_>,
) -> fmt::Result {
    let MemoryMapEvidence::Observed { descriptors, .. } = memory_map else {
        return Err(fmt::Error);
    };
    if descriptors.get(binding.descriptor_index) != Some(&binding.descriptor)
        || binding.descriptor_index != witness.descriptor_index
        || binding.requested_start != witness.requested_range.start
        || binding.requested_end_exclusive != witness.requested_range.end_exclusive
        || binding.descriptor.memory_type != witness.descriptor.memory_type
        || binding.descriptor.physical_start != witness.descriptor.physical_start
        || binding.descriptor.page_count != witness.descriptor.page_count
        || binding.descriptor.attributes != witness.descriptor.attributes
    {
        return Err(fmt::Error);
    }
    Ok(())
}

fn validate_mmio_common(
    evidence: &Evidence<'_>,
    common: IommuMmioCommonEvidence<'_>,
    plan: &MmioReadPlan,
    performance_counters_supported: bool,
) -> fmt::Result {
    if !common.stable
        || common.status_offset != STATUS_OFFSET
        || common.stable_offsets.len() > 15
        || require_stable_mmio_snapshot(&common.first_snapshot, &common.second_snapshot, plan)
            .is_err()
    {
        return Err(fmt::Error);
    }
    let expected_length = if performance_counters_supported {
        512 * 1024
    } else {
        16 * 1024
    };
    if common.aperture_length_bytes != expected_length {
        return Err(fmt::Error);
    }
    let descriptors = memory_descriptors(evidence.memory_map)?;
    let physical_width = evidence.cpu.physical_address_bits().ok_or(fmt::Error)?;
    let effective_width = physical_width.min(
        evidence
            .amd_iommu_live
            .pci
            .capability_decoded
            .iommu_physical_address_width,
    );
    let witness = iommu_aperture_witness(
        &descriptors,
        evidence.amd_iommu_live.locator.unit.mmio_base,
        performance_counters_supported,
        effective_width,
    )
    .map_err(|_| fmt::Error)?;
    validate_descriptor_binding(common.aperture_memory_binding, witness, evidence.memory_map)?;

    let operations = common
        .stable_offsets
        .len()
        .checked_mul(2)
        .and_then(|count| count.checked_add(1))
        .ok_or(fmt::Error)?;
    if evidence.amd_iommu_live.access.mmio_read_operations != operations
        || evidence.amd_iommu_live.access.mmio_read_bytes != operations.saturating_mul(8)
    {
        return Err(fmt::Error);
    }
    Ok(())
}

fn validate_configured_ranges(
    ranges: &[IommuConfiguredRangeEvidence],
    common: IommuMmioCommonEvidence<'_>,
    plan: &MmioReadPlan,
    iommu_physical_address_width: u8,
    cpu_physical_address_width: u8,
    memory_map: MemoryMapEvidence<'_>,
) -> fmt::Result {
    let expected_count = usize::from(plan.active_device_table_segments)
        .checked_add(2)
        .ok_or(fmt::Error)?;
    if ranges.len() != expected_count {
        return Err(fmt::Error);
    }
    let descriptors = memory_descriptors(memory_map)?;
    for (index, range) in ranges.iter().enumerate() {
        let expected_offset = match index {
            0 => 0x0000,
            1 => 0x0008,
            2 => 0x0010,
            _ => DEVICE_TABLE_SEGMENT_1_OFFSET
                .checked_add(
                    u16::try_from(index - 3)
                        .map_err(|_| fmt::Error)?
                        .checked_mul(8)
                        .ok_or(fmt::Error)?,
                )
                .ok_or(fmt::Error)?,
        };
        if range.source_offset != expected_offset
            || !common.stable_offsets.contains(&range.source_offset)
            || snapshot_value(common.first_snapshot, range.source_offset) != Some(range.raw)
        {
            return Err(fmt::Error);
        }
        let (expected_enabled, configured) = decode_configured_range(
            range.source_offset,
            range.raw,
            common.first_snapshot.control,
        )?;
        if range.enabled != expected_enabled
            || range.base != configured.base
            || range.length != configured.length
            || range.alignment != configured.required_alignment
        {
            return Err(fmt::Error);
        }
        let expected_range = validate_enabled_configured_range(
            expected_enabled,
            configured,
            iommu_physical_address_width,
            cpu_physical_address_width,
        )
        .map_err(|_| fmt::Error)?;
        if range.validated_range != expected_range {
            return Err(fmt::Error);
        }
        match (expected_range, range.memory_binding) {
            (None, None) => {}
            (Some(expected_range), Some(binding)) => {
                let witness = memory_descriptor_witness(
                    &descriptors,
                    expected_range,
                    iommu_physical_address_width.min(cpu_physical_address_width),
                )
                .map_err(|_| fmt::Error)?;
                validate_descriptor_binding(binding, witness, memory_map)?;
            }
            (None, Some(_)) | (Some(_), None) => return Err(fmt::Error),
        }
    }
    Ok(())
}

fn decode_configured_range(
    source_offset: u16,
    raw: u64,
    control: u64,
) -> Result<(bool, ConfiguredRange), fmt::Error> {
    const BASE_MASK: u64 = 0x000f_ffff_ffff_f000;
    const ALIGNMENT: u64 = 4096;
    let iommu_enabled = control & 1 != 0;
    let (enabled, length_units) = match source_offset {
        0x0000 => (
            iommu_enabled,
            (raw & 0x01ff).checked_add(1).ok_or(fmt::Error)?,
        ),
        0x0008 => {
            let length_code = ((raw >> 56) & 0x0f) as u32;
            if length_code < 8 {
                return Err(fmt::Error);
            }
            (
                iommu_enabled && control & (1 << 12) != 0,
                1_u64.checked_shl(length_code + 4).ok_or(fmt::Error)? / ALIGNMENT,
            )
        }
        0x0010 => {
            let length_code = ((raw >> 56) & 0x0f) as u32;
            if length_code < 8 {
                return Err(fmt::Error);
            }
            (
                iommu_enabled && control & (1 << 2) != 0,
                1_u64.checked_shl(length_code + 4).ok_or(fmt::Error)? / ALIGNMENT,
            )
        }
        0x0100..=0x0130 if (source_offset - 0x0100).is_multiple_of(8) => (
            iommu_enabled,
            (raw & 0x00ff).checked_add(1).ok_or(fmt::Error)?,
        ),
        _ => return Err(fmt::Error),
    };
    let length = length_units.checked_mul(ALIGNMENT).ok_or(fmt::Error)?;
    Ok((
        enabled,
        ConfiguredRange {
            base: raw & BASE_MASK,
            length,
            required_alignment: ALIGNMENT,
        },
    ))
}

fn snapshot_value(snapshot: StableMmioSnapshot, offset: u16) -> Option<u64> {
    match offset {
        0x0000 => Some(snapshot.device_table_base),
        0x0008 => Some(snapshot.command_buffer_base),
        0x0010 => Some(snapshot.event_log_base),
        0x0018 => Some(snapshot.control),
        0x0020 => Some(snapshot.exclusion_or_completion_base),
        0x0028 => Some(snapshot.exclusion_or_completion_limit),
        0x0030 => snapshot.extended_feature,
        0x01a0 => snapshot.extended_feature_2,
        0x0100..=0x0130 if (offset - 0x0100).is_multiple_of(8) => {
            snapshot.device_table_segments[usize::from((offset - 0x0100) / 8)]
        }
        _ => None,
    }
}

fn validate_processor_consistency(evidence: &Evidence<'_>) -> fmt::Result {
    let MpServicesEvidence::Observed {
        enabled,
        processors,
        ..
    } = evidence.mp_services
    else {
        return Err(fmt::Error);
    };
    let consistency = &evidence.processor_consistency;
    if consistency.timeout_microseconds_per_ap != 0
        || consistency.observations.is_empty()
        || consistency.observations.len() > 256
        || consistency.observations.len() != enabled
        || consistency.observations.iter().any(|observation| {
            !observation.cpu.is_authentic_amd()
                || initial_apic_id(&observation.cpu).is_none()
                || extended_apic_id(&observation.cpu).is_none()
        })
    {
        return Err(fmt::Error);
    }

    let mut observation_index = 0_usize;
    let mut bsp_count = 0_usize;
    for processor in processors {
        if processor.is_bsp {
            bsp_count = bsp_count.checked_add(1).ok_or(fmt::Error)?;
            if !processor.enabled
                || !processor.healthy
                || processor.processor_number != consistency.bsp_processor_number
            {
                return Err(fmt::Error);
            }
        }
        if !processor.enabled {
            continue;
        }
        let observation = consistency
            .observations
            .get(observation_index)
            .ok_or(fmt::Error)?;
        observation_index = observation_index.checked_add(1).ok_or(fmt::Error)?;
        if observation.processor_number != processor.processor_number
            || observation.processor_id != processor.processor_id
            || observation.who_am_i_processor_number != processor.processor_number
            || observation.is_bsp != processor.is_bsp
            || (processor.is_bsp && observation.dispatch != ProcessorDispatch::BspDirect)
            || (!processor.is_bsp
                && observation.dispatch != ProcessorDispatch::StartupThisApSuccess)
        {
            return Err(fmt::Error);
        }
    }
    if bsp_count != 1 || observation_index != consistency.observations.len() {
        return Err(fmt::Error);
    }

    let bsp = consistency.bsp_observation().ok_or(fmt::Error)?;
    if bsp.cpu != *evidence.cpu || bsp.vm_cr != evidence.vm_cr {
        return Err(fmt::Error);
    }
    Ok(())
}

fn validate_acpi_evidence(acpi: &AcpiEvidence<'_>) -> fmt::Result {
    if acpi.selection_address != acpi.rsdp.address
        || acpi.rsdp.raw.bytes.len() < 36
        || acpi.rsdp.raw.bytes.len() > 4096
        || usize::try_from(acpi.rsdp.length).map_err(|_| fmt::Error)? != acpi.rsdp.raw.bytes.len()
        || acpi.rsdt.entries.len() > 256
        || acpi.xsdt.entries.len() > 256
        || acpi.directory.len() > 256
    {
        return Err(fmt::Error);
    }
    validate_root(&acpi.rsdt, *b"RSDT")?;
    validate_root(&acpi.xsdt, *b"XSDT")?;

    let mut total_bytes = acpi
        .rsdp
        .raw
        .bytes
        .len()
        .checked_add(acpi.rsdt.raw.bytes.len())
        .and_then(|value| value.checked_add(acpi.xsdt.raw.bytes.len()))
        .ok_or(fmt::Error)?;
    for record in acpi.directory {
        if record.header_raw.bytes.len() != 36 {
            return Err(fmt::Error);
        }
        total_bytes = total_bytes
            .checked_add(record.header_raw.bytes.len())
            .ok_or(fmt::Error)?;
    }

    let tables = &acpi.tables;
    for (raw, header, signature) in [
        (tables.madt.raw, &tables.madt.header, *b"APIC"),
        (tables.mcfg.raw, &tables.mcfg.header, *b"MCFG"),
        (tables.ivrs.raw, &tables.ivrs.header, *b"IVRS"),
        (tables.fadt.raw, &tables.fadt.header, *b"FACP"),
    ] {
        if raw.bytes.len() < 36
            || raw.bytes.len() > 1_048_576
            || header.signature != signature
            || usize::try_from(header.length).map_err(|_| fmt::Error)? != raw.bytes.len()
        {
            return Err(fmt::Error);
        }
        total_bytes = total_bytes.checked_add(raw.bytes.len()).ok_or(fmt::Error)?;
    }
    if total_bytes > 4_194_304
        || tables.madt.body.entries.len() > 512
        || tables.mcfg.body.allocations.len() > 256
        || tables.ivrs.body.blocks.len() > 256
        || tables.fadt.raw.bytes.len() < 148
    {
        return Err(fmt::Error);
    }
    let madt_processors = tables
        .madt
        .body
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                MadtEntryEvidence::ProcessorLocalApic { .. }
                    | MadtEntryEvidence::ProcessorLocalX2Apic { .. }
            )
        })
        .count();
    if madt_processors > 256 {
        return Err(fmt::Error);
    }
    let ivhd_devices =
        tables
            .ivrs
            .body
            .blocks
            .iter()
            .try_fold(0_usize, |count, block| match block {
                IvrsBlockEvidence::Ivhd(ivhd) if !ivhd.device_entries.is_empty() => count
                    .checked_add(ivhd.device_entries.len())
                    .ok_or(fmt::Error),
                IvrsBlockEvidence::Ivhd(_) => Err(fmt::Error),
                IvrsBlockEvidence::Ivmd(_) => Ok(count),
            })?;
    if ivhd_devices > 4096 {
        return Err(fmt::Error);
    }

    let cross_check = &acpi.madt_mp_cross_check;
    for values in [
        cross_check.mp_enabled_ids,
        cross_check.madt_enabled_ids,
        cross_check.missing_from_madt,
        cross_check.missing_from_mp,
    ] {
        if values.len() > 256 || values.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(fmt::Error);
        }
    }
    let calculated_consistency =
        cross_check.missing_from_madt.is_empty() && cross_check.missing_from_mp.is_empty();
    if cross_check.consistent != calculated_consistency {
        return Err(fmt::Error);
    }
    Ok(())
}

fn validate_root(root: &AcpiRootEvidence<'_>, signature: [u8; 4]) -> fmt::Result {
    if root.raw.bytes.len() < 36
        || root.raw.bytes.len() > 1_048_576
        || root.header.signature != signature
        || usize::try_from(root.header.length).map_err(|_| fmt::Error)? != root.raw.bytes.len()
    {
        return Err(fmt::Error);
    }
    for (index, pointer) in root.entries.iter().enumerate() {
        if *pointer == 0 || root.entries[..index].contains(pointer) {
            return Err(fmt::Error);
        }
    }
    Ok(())
}

struct BoundedJson {
    inner: String,
}

impl BoundedJson {
    fn new() -> Self {
        Self {
            inner: String::with_capacity(64 * 1024),
        }
    }

    fn finish(self) -> String {
        self.inner
    }
}

impl Write for BoundedJson {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let new_len = self
            .inner
            .len()
            .checked_add(value.len())
            .ok_or(fmt::Error)?;
        if new_len > MAX_RENDERED_JSON_BYTES {
            return Err(fmt::Error);
        }
        self.inner.push_str(value);
        Ok(())
    }
}

fn write_evidence(output: &mut impl Write, evidence: &Evidence<'_>) -> fmt::Result {
    writeln!(output, "{{")?;
    writeln!(output, "  \"schema_version\": {SCHEMA_VERSION},")?;
    write!(output, "  \"evidence_kind\": ")?;
    write_json_string(output, EVIDENCE_KIND)?;
    writeln!(output, ",")?;
    writeln!(output, "  \"qualification_status\": \"blocked\",")?;
    writeln!(output, "  \"launch_authorized\": false,")?;
    writeln!(output, "  \"physical_candidate_flash_authorized\": false,")?;
    writeln!(output, "  \"control_state_writes_authorized\": false,")?;
    writeln!(output, "  \"process_introspection_authorized\": false,")?;
    writeln!(output, "  \"confidential_vm_claim\": false,")?;
    writeln!(output, "  \"amd_iommu_ownership_claim\": false,")?;
    writeln!(output, "  \"pci_isolation_claim\": false,")?;
    writeln!(output, "  \"collector\": {{")?;
    writeln!(output, "    \"name\": \"svmvisor-m0b-probe\",")?;
    writeln!(output, "    \"version\": \"{COLLECTOR_VERSION}\",")?;
    writeln!(output, "    \"slice\": \"{COLLECTOR_SLICE}\"")?;
    writeln!(output, "  }},")?;
    write!(output, "  \"target_profile_manifest_sha256\": \"")?;
    write_ascii(output, evidence.target_profile_manifest_sha256)?;
    writeln!(output, "\",")?;

    writeln!(output, "  \"collected_at\": {{")?;
    let time = evidence.collected_at;
    writeln!(output, "    \"year\": {},", time.year)?;
    writeln!(output, "    \"month\": {},", time.month)?;
    writeln!(output, "    \"day\": {},", time.day)?;
    writeln!(output, "    \"hour\": {},", time.hour)?;
    writeln!(output, "    \"minute\": {},", time.minute)?;
    writeln!(output, "    \"second\": {},", time.second)?;
    writeln!(output, "    \"nanosecond\": {},", time.nanosecond)?;
    write!(output, "    \"timezone\": ")?;
    match time.timezone_minutes {
        Some(minutes) => writeln!(
            output,
            "{{ \"status\": \"observed\", \"minutes_from_utc\": {minutes} }}"
        )?,
        None => writeln!(output, "{{ \"status\": \"unspecified\" }}")?,
    }
    writeln!(output, "  }},")?;

    writeln!(output, "  \"sink\": {{")?;
    write!(output, "    \"output_file\": ")?;
    write_json_string(output, evidence.output_file)?;
    writeln!(output, ",")?;
    writeln!(
        output,
        "    \"selection\": \"loaded-image-volume-plus-profile-binding-marker\","
    )?;
    writeln!(
        output,
        "    \"media_id\": \"0x{:08x}\",",
        evidence.sink.media_id
    )?;
    writeln!(
        output,
        "    \"removable_media\": {},",
        evidence.sink.removable_media
    )?;
    writeln!(
        output,
        "    \"media_present\": {},",
        evidence.sink.media_present
    )?;
    writeln!(
        output,
        "    \"logical_partition\": {},",
        evidence.sink.logical_partition
    )?;
    writeln!(output, "    \"read_only\": {},", evidence.sink.read_only)?;
    writeln!(output, "    \"block_size\": {},", evidence.sink.block_size)?;
    writeln!(output, "    \"last_block\": {}", evidence.sink.last_block)?;
    writeln!(output, "  }},")?;

    writeln!(output, "  \"uefi\": {{")?;
    write!(output, "    \"firmware_vendor\": ")?;
    write_json_string(output, evidence.firmware_vendor)?;
    writeln!(output, ",")?;
    writeln!(
        output,
        "    \"firmware_revision\": {},",
        evidence.firmware_revision
    )?;
    writeln!(
        output,
        "    \"specification_revision\": {{ \"major\": {}, \"minor\": {} }},",
        evidence.uefi_revision_major, evidence.uefi_revision_minor
    )?;
    writeln!(output, "    \"configuration_tables\": [")?;
    for (index, table) in evidence.config_tables.iter().enumerate() {
        write!(output, "      {{ \"kind\": ")?;
        write_json_string(output, table.kind)?;
        write!(output, ", \"guid\": ")?;
        write_json_string(output, table.guid)?;
        write!(output, ", \"address\": \"0x{:016x}\" }}", table.address)?;
        writeln!(
            output,
            "{}",
            if index + 1 == evidence.config_tables.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "    ]")?;
    writeln!(output, "  }},")?;

    write_cpu(
        output,
        evidence.cpu,
        "bsp-identified-by-uefi-mp-services-and-cross-processor-compared",
    )?;
    writeln!(output, ",")?;
    write_vm_cr(output, evidence.vm_cr)?;
    writeln!(output, ",")?;
    write_mp_services(output, evidence.mp_services)?;
    writeln!(output, ",")?;
    write_processor_consistency(output, &evidence.processor_consistency)?;
    writeln!(output, ",")?;
    write_memory_map(output, evidence.memory_map)?;
    writeln!(output, ",")?;
    write_acpi(output, &evidence.acpi)?;
    writeln!(output, ",")?;
    write_amd_iommu_live(output, &evidence.amd_iommu_live)?;
    writeln!(output, ",")?;
    write_system_registers(output, &evidence.system_registers)?;
    writeln!(output, ",")?;

    writeln!(output, "  \"uncollected_blockers\": [")?;
    let blockers = [
        "amd-iommu-dte-requester-ownership-and-pci-isolation",
        "inherited-memory-encryption-state",
        "secure-boot-databases-option-rom-policy-and-tcg-log",
        "boot-driver-sysprep-recovery-and-hotkey-namespace",
        "ready-to-boot-after-ready-to-boot-exit-boot-services-order",
        "direct-watchdog-and-durable-attempt-lease",
        "mp-services-ap-dispatch-pre-measurement-control-state-preservation",
    ];
    for (index, blocker) in blockers.iter().enumerate() {
        write!(output, "    ")?;
        write_json_string(output, blocker)?;
        writeln!(
            output,
            "{}",
            if index + 1 == blockers.len() { "" } else { "," }
        )?;
    }
    writeln!(output, "  ]")?;
    writeln!(output, "}}")
}

fn write_amd_iommu_live(
    output: &mut impl Write,
    evidence: &AmdIommuLiveEvidence<'_>,
) -> fmt::Result {
    let status = match evidence.mmio {
        AmdIommuMmioEvidence::Disabled => "pci-observed-mmio-disabled",
        AmdIommuMmioEvidence::EfrConflict { .. } => "pci-observed-efr-conflict",
        AmdIommuMmioEvidence::Observed { .. } => "mmio-observed",
    };
    writeln!(output, "  \"amd_iommu_live\": {{")?;
    writeln!(output, "    \"status\": \"{status}\",")?;
    let access = evidence.access;
    writeln!(output, "    \"access\": {{")?;
    writeln!(
        output,
        "      \"pci_transport\": \"uefi-pci-root-bridge-io-pci-read\","
    )?;
    writeln!(
        output,
        "      \"mmio_transport\": \"uefi-pci-root-bridge-io-memory-read\","
    )?;
    writeln!(
        output,
        "      \"root_bridge_handle_count\": {},",
        access.root_bridge_handle_count
    )?;
    writeln!(
        output,
        "      \"matching_segment_handle_count\": {},",
        access.matching_segment_handle_count
    )?;
    writeln!(
        output,
        "      \"full_match_handle_count\": {},",
        access.full_match_handle_count
    )?;
    writeln!(
        output,
        "      \"pci_read_operations\": {},",
        access.pci_read_operations
    )?;
    writeln!(
        output,
        "      \"pci_read_bytes\": {},",
        access.pci_read_bytes
    )?;
    writeln!(
        output,
        "      \"mmio_read_operations\": {},",
        access.mmio_read_operations
    )?;
    writeln!(
        output,
        "      \"mmio_read_bytes\": {},",
        access.mmio_read_bytes
    )?;
    writeln!(
        output,
        "      \"pci_write_operations\": {},",
        access.pci_write_operations
    )?;
    writeln!(
        output,
        "      \"mmio_write_operations\": {},",
        access.mmio_write_operations
    )?;
    writeln!(
        output,
        "      \"direct_ecam_access\": {},",
        access.direct_ecam_access
    )?;
    writeln!(
        output,
        "      \"direct_mmio_access\": {},",
        access.direct_mmio_access
    )?;
    writeln!(
        output,
        "      \"cf8_cfc_access\": {},",
        access.cf8_cfc_access
    )?;
    writeln!(
        output,
        "      \"configured_pointer_dereferences\": {}",
        access.configured_pointer_dereferences
    )?;
    writeln!(output, "    }},")?;
    write_iommu_locator(output, &evidence.locator)?;
    writeln!(output, ",")?;
    write_iommu_pci(output, &evidence.pci)?;
    writeln!(output, ",")?;
    write_iommu_mmio(output, evidence.mmio, evidence.access.mmio_read_operations)?;
    writeln!(output, ",")?;
    let ownership = evidence.ownership;
    writeln!(output, "    \"ownership\": {{")?;
    writeln!(output, "      \"assessed\": {},", ownership.assessed)?;
    writeln!(output, "      \"claim\": {},", ownership.claim)?;
    writeln!(
        output,
        "      \"requester_dma_isolation_claim\": {},",
        ownership.requester_dma_isolation_claim
    )?;
    writeln!(
        output,
        "      \"interrupt_remapping_claim\": {},",
        ownership.interrupt_remapping_claim
    )?;
    writeln!(
        output,
        "      \"pci_isolation_claim\": {}",
        ownership.pci_isolation_claim
    )?;
    writeln!(output, "    }}")?;
    write!(output, "  }}")
}

fn write_iommu_locator(output: &mut impl Write, locator: &IommuLocatorEvidence<'_>) -> fmt::Result {
    writeln!(output, "    \"locator\": {{")?;
    writeln!(
        output,
        "      \"derivation\": \"same-run-validated-ivrs-mcfg-memory-map\","
    )?;
    writeln!(output, "      \"unique_unit_count\": 1,")?;
    writeln!(output, "      \"ivhd_sources\": [")?;
    for (index, source) in locator.ivhd_sources.iter().enumerate() {
        writeln!(output, "        {{")?;
        writeln!(
            output,
            "          \"block_index\": {},",
            source.source_index
        )?;
        writeln!(
            output,
            "          \"entry_type\": \"0x{:02x}\",",
            source.source.entry_type()
        )?;
        writeln!(
            output,
            "          \"segment_group\": \"0x{:04x}\",",
            source.segment_group
        )?;
        writeln!(
            output,
            "          \"device_id\": \"0x{:04x}\",",
            source.device_id
        )?;
        writeln!(
            output,
            "          \"capability_offset\": \"0x{:04x}\",",
            source.capability_offset
        )?;
        writeln!(
            output,
            "          \"iommu_base_address\": \"0x{:016x}\",",
            source.mmio_base
        )?;
        write!(output, "          \"extended_feature_image\": ")?;
        write_optional_hex64(output, source.efr_image)?;
        writeln!(output, ",")?;
        write!(output, "          \"extended_feature_image_2\": ")?;
        write_optional_hex64(output, source.efr2_image)?;
        writeln!(output)?;
        write!(output, "        }}")?;
        writeln!(
            output,
            "{}",
            if index + 1 == locator.ivhd_sources.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "      ],")?;
    let unit = locator.unit;
    writeln!(output, "      \"unit\": {{")?;
    writeln!(
        output,
        "        \"segment_group\": \"0x{:04x}\",",
        unit.segment_group
    )?;
    writeln!(
        output,
        "        \"device_id\": \"0x{:04x}\",",
        unit.device_id
    )?;
    write_bdf(output, "        ", "bdf", unit.bdf, true)?;
    writeln!(
        output,
        "        \"capability_offset\": \"0x{:04x}\",",
        unit.capability_offset
    )?;
    writeln!(
        output,
        "        \"iommu_base_address\": \"0x{:016x}\",",
        unit.mmio_base
    )?;
    writeln!(
        output,
        "        \"ivhd_type_10_present\": {},",
        unit.type10_present
    )?;
    writeln!(
        output,
        "        \"ivhd_type_11_present\": {},",
        unit.type11_present
    )?;
    writeln!(
        output,
        "        \"ivhd_type_40_present\": {},",
        unit.type40_present
    )?;
    write!(output, "        \"expected_extended_feature_image\": ")?;
    write_optional_hex64(output, unit.efr_image)?;
    writeln!(output, ",")?;
    write!(output, "        \"expected_extended_feature_image_2\": ")?;
    write_optional_hex64(output, unit.efr2_image)?;
    writeln!(output)?;
    writeln!(output, "      }},")?;
    let mcfg = locator.mcfg_witness;
    writeln!(output, "      \"mcfg\": {{")?;
    writeln!(
        output,
        "        \"allocation_index\": {},",
        mcfg.allocation.source_index
    )?;
    writeln!(
        output,
        "        \"base_address\": \"0x{:016x}\",",
        mcfg.allocation.base_address
    )?;
    writeln!(
        output,
        "        \"segment_group\": \"0x{:04x}\",",
        mcfg.allocation.segment_group
    )?;
    writeln!(
        output,
        "        \"start_bus\": \"0x{:02x}\",",
        mcfg.allocation.start_bus
    )?;
    writeln!(
        output,
        "        \"end_bus\": \"0x{:02x}\",",
        mcfg.allocation.end_bus
    )?;
    writeln!(
        output,
        "        \"usable_start\": \"0x{:016x}\",",
        mcfg.usable_range.start
    )?;
    writeln!(
        output,
        "        \"usable_end_exclusive\": \"0x{:016x}\",",
        mcfg.usable_range.end_exclusive
    )?;
    writeln!(
        output,
        "        \"ecam_function_address\": \"0x{:016x}\",",
        mcfg.function_address
    )?;
    writeln!(
        output,
        "        \"ecam_capability_address\": \"0x{:016x}\"",
        mcfg.capability_address
    )?;
    writeln!(output, "      }},")?;
    write_memory_descriptor_binding(
        output,
        "      ",
        "ecam_memory_binding",
        locator.ecam_memory_binding,
    )?;
    writeln!(output)?;
    write!(output, "    }}")
}

fn write_iommu_pci(output: &mut impl Write, pci: &IommuPciEvidence<'_>) -> fmt::Result {
    writeln!(output, "    \"pci\": {{")?;
    writeln!(
        output,
        "      \"selected_root_bridge_handle_index\": {},",
        pci.selected_root_bridge_handle_index
    )?;
    writeln!(
        output,
        "      \"segment_group\": \"0x{:04x}\",",
        pci.segment_group
    )?;
    write_bdf(output, "      ", "bdf", pci.bdf, true)?;
    writeln!(output, "      \"identity\": {{")?;
    let raw = pci.identity_raw;
    writeln!(output, "        \"raw\": {{")?;
    writeln!(
        output,
        "          \"vendor_device\": \"0x{:08x}\",",
        raw.vendor_device
    )?;
    writeln!(
        output,
        "          \"command_status\": \"0x{:08x}\",",
        raw.command_status
    )?;
    writeln!(
        output,
        "          \"class_revision\": \"0x{:08x}\",",
        raw.class_revision
    )?;
    writeln!(
        output,
        "          \"header_type\": \"0x{:02x}\",",
        raw.header_type
    )?;
    writeln!(
        output,
        "          \"first_capability_pointer\": \"0x{:02x}\"",
        raw.first_capability_pointer
    )?;
    writeln!(output, "        }},")?;
    let decoded = pci.identity_decoded;
    writeln!(output, "        \"decoded\": {{")?;
    writeln!(
        output,
        "          \"vendor_id\": \"0x{:04x}\",",
        decoded.vendor_id
    )?;
    writeln!(
        output,
        "          \"device_id\": \"0x{:04x}\",",
        decoded.device_id
    )?;
    writeln!(
        output,
        "          \"capabilities_list_present\": {},",
        decoded.capabilities_list_present
    )?;
    writeln!(
        output,
        "          \"class_code\": \"0x{:02x}\",",
        decoded.class_code
    )?;
    writeln!(
        output,
        "          \"subclass\": \"0x{:02x}\",",
        decoded.subclass
    )?;
    writeln!(
        output,
        "          \"programming_interface\": \"0x{:02x}\",",
        decoded.programming_interface
    )?;
    writeln!(
        output,
        "          \"revision_id\": \"0x{:02x}\",",
        decoded.revision_id
    )?;
    writeln!(
        output,
        "          \"header_layout\": \"0x{:02x}\",",
        decoded.header_layout
    )?;
    writeln!(
        output,
        "          \"multifunction\": {},",
        decoded.multifunction
    )?;
    writeln!(
        output,
        "          \"first_capability_pointer\": \"0x{:02x}\"",
        decoded.first_capability_pointer
    )?;
    writeln!(output, "        }}")?;
    writeln!(output, "      }},")?;
    writeln!(output, "      \"capability_chain\": [")?;
    for (index, link) in pci.capability_chain.iter().enumerate() {
        write!(
            output,
            "        {{ \"offset\": \"0x{:02x}\", \"capability_id\": \"0x{:02x}\", \"next\": \"0x{:02x}\" }}",
            link.offset, link.capability_id, link.next
        )?;
        writeln!(
            output,
            "{}",
            if index + 1 == pci.capability_chain.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "      ],")?;
    writeln!(output, "      \"capability\": {{")?;
    write_pci_capability_raw(
        output,
        "        ",
        "first_raw",
        pci.capability_first_raw,
        true,
    )?;
    write_pci_capability_raw(
        output,
        "        ",
        "second_raw",
        pci.capability_second_raw,
        true,
    )?;
    writeln!(output, "        \"stable\": {},", pci.capability_stable)?;
    write_pci_capability_decoded(output, pci.capability_decoded)?;
    writeln!(output)?;
    writeln!(output, "      }}")?;
    write!(output, "    }}")
}

fn write_iommu_mmio(
    output: &mut impl Write,
    mmio: AmdIommuMmioEvidence<'_>,
    read_operations: usize,
) -> fmt::Result {
    writeln!(output, "    \"mmio\": {{")?;
    match mmio {
        AmdIommuMmioEvidence::Disabled => {
            writeln!(
                output,
                "      \"status\": \"not-read-capability-disabled\","
            )?;
            writeln!(output, "      \"read_operations\": 0")?;
        }
        AmdIommuMmioEvidence::EfrConflict {
            common,
            expected,
            live,
        } => {
            writeln!(output, "      \"status\": \"efr-conflict\",")?;
            writeln!(output, "      \"read_operations\": {read_operations},")?;
            write_mmio_common(output, common, false)?;
            writeln!(output, ",")?;
            write_extended_feature_images(
                output,
                "      ",
                "expected_extended_features",
                expected,
                true,
            )?;
            write_extended_feature_images(output, "      ", "live_extended_features", live, false)?;
        }
        AmdIommuMmioEvidence::Observed {
            common,
            features,
            configured_ranges,
            decoded,
        } => {
            writeln!(output, "      \"status\": \"observed\",")?;
            writeln!(output, "      \"read_operations\": {read_operations},")?;
            write_mmio_common(output, common, true)?;
            writeln!(output, ",")?;
            write_extended_feature_comparison(output, features)?;
            writeln!(output, ",")?;
            write_configured_ranges(output, configured_ranges)?;
            writeln!(output, ",")?;
            write_decoded_live_state(output, decoded)?;
            writeln!(output)?;
        }
    }
    write!(output, "    }}")
}

fn write_configured_ranges(
    output: &mut impl Write,
    ranges: &[IommuConfiguredRangeEvidence],
) -> fmt::Result {
    writeln!(output, "      \"configured_ranges\": [")?;
    for (index, range) in ranges.iter().enumerate() {
        writeln!(output, "        {{")?;
        writeln!(
            output,
            "          \"source_offset\": \"0x{:04x}\",",
            range.source_offset
        )?;
        writeln!(output, "          \"raw\": \"0x{:016x}\",", range.raw)?;
        writeln!(output, "          \"enabled\": {},", range.enabled)?;
        writeln!(output, "          \"base\": \"0x{:016x}\",", range.base)?;
        writeln!(output, "          \"length\": {},", range.length)?;
        writeln!(output, "          \"alignment\": {},", range.alignment)?;
        write!(output, "          \"validated_range\": ")?;
        match range.validated_range {
            Some(validated) => write!(
                output,
                "{{ \"start\": \"0x{:016x}\", \"end_exclusive\": \"0x{:016x}\" }}",
                validated.start, validated.end_exclusive
            )?,
            None => write!(output, "null")?,
        }
        writeln!(output, ",")?;
        match range.memory_binding {
            Some(binding) => {
                write_memory_descriptor_binding(output, "          ", "memory_binding", binding)?
            }
            None => write!(output, "          \"memory_binding\": null")?,
        }
        writeln!(output)?;
        write!(output, "        }}")?;
        writeln!(
            output,
            "{}",
            if index + 1 == ranges.len() { "" } else { "," }
        )?;
    }
    write!(output, "      ]")
}

fn write_mmio_common(
    output: &mut impl Write,
    common: IommuMmioCommonEvidence<'_>,
    extended_feature_match: bool,
) -> fmt::Result {
    writeln!(output, "      \"aperture\": {{")?;
    writeln!(
        output,
        "        \"length_bytes\": {},",
        common.aperture_length_bytes
    )?;
    write_memory_descriptor_binding(
        output,
        "        ",
        "memory_binding",
        common.aperture_memory_binding,
    )?;
    writeln!(output)?;
    writeln!(output, "      }},")?;
    write_offset_array(
        output,
        "      ",
        "stable_offsets",
        common.stable_offsets,
        true,
    )?;
    write_offset_array(
        output,
        "      ",
        "feature_dependent_offsets",
        common.feature_dependent_offsets,
        true,
    )?;
    write_mmio_snapshot(
        output,
        "      ",
        "first_snapshot",
        common.first_snapshot,
        true,
    )?;
    write_mmio_snapshot(
        output,
        "      ",
        "second_snapshot",
        common.second_snapshot,
        true,
    )?;
    writeln!(output, "      \"stable\": {},", common.stable)?;
    writeln!(
        output,
        "      \"status_offset\": \"0x{:04x}\",",
        common.status_offset
    )?;
    writeln!(
        output,
        "      \"status_raw\": \"0x{:016x}\",",
        common.status_raw
    )?;
    write!(
        output,
        "      \"extended_feature_match\": {extended_feature_match}"
    )
}

fn write_memory_descriptor_binding(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    binding: MemoryDescriptorBindingEvidence,
) -> fmt::Result {
    writeln!(output, "{indent}\"{name}\": {{")?;
    writeln!(
        output,
        "{indent}  \"descriptor_index\": {},",
        binding.descriptor_index
    )?;
    let descriptor = binding.descriptor;
    writeln!(output, "{indent}  \"descriptor\": {{")?;
    writeln!(
        output,
        "{indent}    \"memory_type\": {},",
        descriptor.memory_type
    )?;
    writeln!(
        output,
        "{indent}    \"physical_start\": \"0x{:016x}\",",
        descriptor.physical_start
    )?;
    writeln!(
        output,
        "{indent}    \"virtual_start\": \"0x{:016x}\",",
        descriptor.virtual_start
    )?;
    writeln!(
        output,
        "{indent}    \"page_count\": {},",
        descriptor.page_count
    )?;
    writeln!(
        output,
        "{indent}    \"attributes\": \"0x{:016x}\"",
        descriptor.attributes
    )?;
    writeln!(output, "{indent}  }},")?;
    writeln!(
        output,
        "{indent}  \"requested_start\": \"0x{:016x}\",",
        binding.requested_start
    )?;
    writeln!(
        output,
        "{indent}  \"requested_end_exclusive\": \"0x{:016x}\"",
        binding.requested_end_exclusive
    )?;
    write!(output, "{indent}}}")
}

fn write_bdf(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    bdf: PciBdf,
    comma: bool,
) -> fmt::Result {
    writeln!(output, "{indent}\"{name}\": {{")?;
    writeln!(output, "{indent}  \"bus\": \"0x{:02x}\",", bdf.bus)?;
    writeln!(output, "{indent}  \"device\": \"0x{:02x}\",", bdf.device)?;
    writeln!(
        output,
        "{indent}  \"function\": \"0x{:02x}\",",
        bdf.function
    )?;
    writeln!(
        output,
        "{indent}  \"formatted\": \"{:02x}:{:02x}.{}\"",
        bdf.bus, bdf.device, bdf.function
    )?;
    writeln!(output, "{indent}}}{}", if comma { "," } else { "" })
}

fn write_pci_capability_raw(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    raw: PciCapabilityRaw,
    comma: bool,
) -> fmt::Result {
    writeln!(output, "{indent}\"{name}\": {{")?;
    writeln!(output, "{indent}  \"header\": \"0x{:08x}\",", raw.header)?;
    writeln!(
        output,
        "{indent}  \"base_low\": \"0x{:08x}\",",
        raw.base_low
    )?;
    writeln!(
        output,
        "{indent}  \"base_high\": \"0x{:08x}\",",
        raw.base_high
    )?;
    writeln!(output, "{indent}  \"range\": \"0x{:08x}\",", raw.range)?;
    writeln!(
        output,
        "{indent}  \"miscellaneous_0\": \"0x{:08x}\",",
        raw.miscellaneous_0
    )?;
    write!(output, "{indent}  \"miscellaneous_1\": ")?;
    match raw.miscellaneous_1 {
        Some(value) => write!(output, "\"0x{value:08x}\"")?,
        None => write!(output, "null")?,
    }
    writeln!(output)?;
    writeln!(output, "{indent}}}{}", if comma { "," } else { "" })
}

fn write_pci_capability_decoded(output: &mut impl Write, decoded: PciCapability) -> fmt::Result {
    writeln!(output, "        \"decoded\": {{")?;
    writeln!(
        output,
        "          \"capability_id\": \"0x{:02x}\",",
        decoded.capability_id
    )?;
    writeln!(
        output,
        "          \"next_pointer\": \"0x{:02x}\",",
        decoded.next_pointer
    )?;
    writeln!(
        output,
        "          \"capability_type\": \"0x{:02x}\",",
        decoded.capability_type
    )?;
    writeln!(
        output,
        "          \"capability_revision\": \"0x{:02x}\",",
        decoded.capability_revision
    )?;
    writeln!(
        output,
        "          \"extended_feature_register_supported\": {},",
        decoded.extended_feature_register_supported
    )?;
    writeln!(
        output,
        "          \"capability_extension_supported\": {},",
        decoded.capability_extension_supported
    )?;
    writeln!(
        output,
        "          \"mmio_enabled\": {},",
        decoded.mmio_enabled
    )?;
    writeln!(
        output,
        "          \"decoded_mmio_base\": \"0x{:016x}\",",
        decoded.decoded_mmio_base
    )?;
    writeln!(output, "          \"range\": \"0x{:08x}\",", decoded.range)?;
    writeln!(
        output,
        "          \"iommu_physical_address_width\": {},",
        decoded.iommu_physical_address_width
    )?;
    write!(output, "          \"miscellaneous_1\": ")?;
    match decoded.miscellaneous_1 {
        Some(value) => write!(output, "\"0x{value:08x}\"")?,
        None => write!(output, "null")?,
    }
    writeln!(output)?;
    write!(output, "        }}")
}

fn write_offset_array(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    offsets: &[u16],
    comma: bool,
) -> fmt::Result {
    write!(output, "{indent}\"{name}\": [")?;
    for (index, offset) in offsets.iter().enumerate() {
        if index != 0 {
            write!(output, ", ")?;
        }
        write!(output, "\"0x{offset:04x}\"")?;
    }
    writeln!(output, "]{}", if comma { "," } else { "" })
}

fn write_mmio_snapshot(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    snapshot: StableMmioSnapshot,
    comma: bool,
) -> fmt::Result {
    writeln!(output, "{indent}\"{name}\": {{")?;
    for (name, value) in [
        ("device_table_base", snapshot.device_table_base),
        ("command_buffer_base", snapshot.command_buffer_base),
        ("event_log_base", snapshot.event_log_base),
        ("control", snapshot.control),
        (
            "exclusion_base_or_completion_store_base",
            snapshot.exclusion_or_completion_base,
        ),
        (
            "exclusion_limit_or_completion_store_limit",
            snapshot.exclusion_or_completion_limit,
        ),
    ] {
        writeln!(output, "{indent}  \"{name}\": \"0x{value:016x}\",")?;
    }
    write!(output, "{indent}  \"extended_feature\": ")?;
    write_optional_hex64(output, snapshot.extended_feature)?;
    writeln!(output, ",")?;
    write!(output, "{indent}  \"extended_feature_2\": ")?;
    write_optional_hex64(output, snapshot.extended_feature_2)?;
    writeln!(output, ",")?;
    writeln!(output, "{indent}  \"device_table_segments\": [")?;
    let segment_count = snapshot
        .device_table_segments
        .iter()
        .filter(|value| value.is_some())
        .count();
    let mut rendered = 0_usize;
    for (index, value) in snapshot.device_table_segments.iter().enumerate() {
        let Some(value) = value else { continue };
        rendered += 1;
        write!(
            output,
            "{indent}    {{ \"segment\": {}, \"raw\": \"0x{value:016x}\" }}",
            index + 1
        )?;
        writeln!(
            output,
            "{}",
            if rendered == segment_count { "" } else { "," }
        )?;
    }
    writeln!(output, "{indent}  ]")?;
    writeln!(output, "{indent}}}{}", if comma { "," } else { "" })
}

fn write_extended_feature_images(
    output: &mut impl Write,
    indent: &str,
    name: &str,
    images: ExtendedFeatureImages,
    comma: bool,
) -> fmt::Result {
    writeln!(output, "{indent}\"{name}\": {{")?;
    writeln!(output, "{indent}  \"efr\": \"0x{:016x}\",", images.efr)?;
    writeln!(output, "{indent}  \"efr2\": \"0x{:016x}\"", images.efr2)?;
    writeln!(output, "{indent}}}{}", if comma { "," } else { "" })
}

fn write_extended_feature_comparison(
    output: &mut impl Write,
    features: ExtendedFeatureComparison,
) -> fmt::Result {
    writeln!(output, "      \"extended_features\": {{")?;
    match features {
        ExtendedFeatureComparison::NotSupported => {
            writeln!(output, "        \"status\": \"not-supported\"")?;
        }
        ExtendedFeatureComparison::Match {
            live,
            performance_counters_supported,
            device_table_segments_supported,
        } => {
            writeln!(output, "        \"status\": \"match\",")?;
            writeln!(output, "        \"live\": {{")?;
            writeln!(output, "          \"efr\": \"0x{:016x}\",", live.efr)?;
            writeln!(output, "          \"efr2\": \"0x{:016x}\"", live.efr2)?;
            writeln!(output, "        }},")?;
            writeln!(
                output,
                "        \"performance_counters_supported\": {},",
                performance_counters_supported
            )?;
            writeln!(
                output,
                "        \"device_table_segments_supported\": {}",
                device_table_segments_supported
            )?;
        }
        ExtendedFeatureComparison::Conflict { .. } => return Err(fmt::Error),
    }
    write!(output, "      }}")
}

fn write_decoded_live_state(output: &mut impl Write, state: DecodedLiveState) -> fmt::Result {
    writeln!(output, "      \"decoded_state\": {{")?;
    writeln!(
        output,
        "        \"iommu_enabled\": {},",
        state.iommu_enabled
    )?;
    writeln!(
        output,
        "        \"event_log_enabled\": {},",
        state.event_log_enabled
    )?;
    writeln!(
        output,
        "        \"command_buffer_enabled\": {},",
        state.command_buffer_enabled
    )?;
    writeln!(
        output,
        "        \"device_table_segment_encoding\": {},",
        state.device_table_segment_encoding
    )?;
    writeln!(
        output,
        "        \"event_log_running\": {},",
        state.event_log_running
    )?;
    writeln!(
        output,
        "        \"command_buffer_running\": {},",
        state.command_buffer_running
    )?;
    writeln!(
        output,
        "        \"event_overflow\": {}",
        state.event_overflow
    )?;
    write!(output, "      }}")
}

fn write_cpu(output: &mut impl Write, cpu: &CpuInventory, observation_scope: &str) -> fmt::Result {
    writeln!(output, "  \"cpu\": {{")?;
    write!(output, "    \"observation_scope\": ")?;
    write_json_string(output, observation_scope)?;
    writeln!(output, ",")?;
    write!(output, "    \"vendor\": ")?;
    write_trimmed_ascii_string(output, &cpu.vendor())?;
    writeln!(output, ",")?;
    writeln!(output, "    \"authentic_amd\": {},", cpu.is_authentic_amd())?;
    writeln!(
        output,
        "    \"max_basic_leaf\": \"0x{:08x}\",",
        cpu.max_basic_leaf()
    )?;
    writeln!(
        output,
        "    \"max_extended_leaf\": \"0x{:08x}\",",
        cpu.max_extended_leaf()
    )?;
    write!(output, "    \"brand\": ")?;
    match &cpu.brand {
        Some(brand) => {
            write!(output, "{{ \"status\": \"observed\", \"value\": ")?;
            write_trimmed_ascii_string(output, brand)?;
            writeln!(output, " }},")?;
        }
        None => writeln!(output, "{{ \"status\": \"not-enumerated\" }},")?,
    }
    write!(output, "    \"family_model_stepping\": ")?;
    match cpu.family_model_stepping() {
        Some((family, model, stepping)) => writeln!(
            output,
            "{{ \"status\": \"observed\", \"family\": {family}, \"model\": {model}, \"stepping\": {stepping} }},"
        )?,
        None => writeln!(output, "{{ \"status\": \"not-enumerated\" }},")?,
    }
    writeln!(output, "    \"raw_leaves\": [")?;
    let brand_leaves = cpu.leaf_8000_0002_to_0004;
    let leaves = [
        (0x0000_0000, 0, Some(cpu.leaf_0000_0000)),
        (0x0000_0001, 0, cpu.leaf_0000_0001),
        (0x0000_0007, 0, cpu.leaf_0000_0007_subleaf_0),
        (0x8000_0000, 0, Some(cpu.leaf_8000_0000)),
        (0x8000_0001, 0, cpu.leaf_8000_0001),
        (0x8000_0002, 0, brand_leaves.map(|leaves| leaves[0])),
        (0x8000_0003, 0, brand_leaves.map(|leaves| leaves[1])),
        (0x8000_0004, 0, brand_leaves.map(|leaves| leaves[2])),
        (0x8000_0008, 0, cpu.leaf_8000_0008),
        (0x8000_000a, 0, cpu.leaf_8000_000a),
        (0x8000_001e, 0, cpu.leaf_8000_001e),
        (0x8000_001f, 0, cpu.leaf_8000_001f),
    ];
    let observed_count = leaves
        .iter()
        .filter(|(_, _, value)| value.is_some())
        .count();
    let mut observed_index = 0;
    for (leaf, subleaf, registers) in leaves {
        let Some(registers) = registers else { continue };
        observed_index += 1;
        write_raw_leaf(output, leaf, subleaf, registers)?;
        writeln!(
            output,
            "{}",
            if observed_index == observed_count {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "    ],")?;
    writeln!(output, "    \"decoded\": {{")?;
    writeln!(
        output,
        "      \"semantics\": \"cpuid-capability-enumeration-not-enabled-state\","
    )?;
    let amd = cpu.is_authentic_amd();
    write_vendor_bool(output, "svm_capable", cpu.svm_cpuid_supported(), amd, true)?;
    write_observed_u8(
        output,
        "physical_address_bits",
        cpu.physical_address_bits(),
        true,
    )?;
    write_vendor_u8(output, "svm_revision", cpu.svm_revision(), amd, true)?;
    write_vendor_u32(output, "asid_count", cpu.svm_asid_count(), amd, true)?;
    write_vendor_bool(
        output,
        "nested_paging_capable",
        cpu.svm_feature(0),
        amd,
        true,
    )?;
    write_vendor_bool(output, "svm_lock_capable", cpu.svm_feature(2), amd, true)?;
    write_vendor_bool(output, "nrip_save_capable", cpu.svm_feature(3), amd, true)?;
    write_vendor_bool(
        output,
        "vmcb_clean_bits_capable",
        cpu.svm_feature(5),
        amd,
        true,
    )?;
    write_vendor_bool(
        output,
        "flush_by_asid_capable",
        cpu.svm_feature(6),
        amd,
        true,
    )?;
    write_vendor_bool(
        output,
        "decode_assists_capable",
        cpu.svm_feature(7),
        amd,
        true,
    )?;
    write_vendor_bool(
        output,
        "sme_capable",
        cpu.memory_encryption_feature(0),
        amd,
        true,
    )?;
    write_vendor_bool(
        output,
        "sev_capable",
        cpu.memory_encryption_feature(1),
        amd,
        true,
    )?;
    write_vendor_u8(output, "c_bit_position", cpu.c_bit_position(), amd, true)?;
    write_vendor_u8(
        output,
        "physical_address_reduction",
        cpu.physical_address_reduction(),
        amd,
        false,
    )?;
    writeln!(output, "    }}")?;
    write!(output, "  }}")
}

fn write_vm_cr(output: &mut impl Write, vm_cr: VmCrEvidence<'_>) -> fmt::Result {
    write!(output, "  \"vm_cr\": ")?;
    match vm_cr {
        VmCrEvidence::Observed(raw) => write!(
            output,
            "{{ \"status\": \"observed\", \"raw\": \"0x{raw:016x}\", \"dpd\": {}, \"r_init\": {}, \"dis_a20m\": {}, \"lock\": {}, \"svmdis\": {} }}",
            raw & (1 << 0) != 0,
            raw & (1 << 1) != 0,
            raw & (1 << 2) != 0,
            raw & (1 << 3) != 0,
            raw & (1 << 4) != 0,
        ),
        VmCrEvidence::NotAttempted { reason } => {
            write!(output, "{{ \"status\": \"not-attempted\", \"reason\": ")?;
            write_json_string(output, reason)?;
            write!(output, " }}")
        }
    }
}

fn write_system_registers(
    output: &mut impl Write,
    section: &SystemRegistersSection<'_>,
) -> fmt::Result {
    writeln!(output, "  \"system_registers\": {{")?;
    writeln!(
        output,
        "    \"scope\": \"all-enabled-healthy-processors-from-matching-pre-and-post-mp-services-enumerations\","
    )?;
    writeln!(output, "    \"policy\": {{")?;
    writeln!(
        output,
        "      \"document\": \"docs/m0b-msr-read-policy.md\","
    )?;
    writeln!(
        output,
        "      \"comparison\": \"exact-raw-and-observation-status-except-thread-scoped-smm-base\","
    )?;
    writeln!(output, "      \"callback_system_register_reads\": true,")?;
    writeln!(output, "      \"callback_control_state_writes\": false,")?;
    writeln!(output, "      \"ppr\": {{")?;
    writeln!(output, "        \"publisher\": \"AMD\",")?;
    writeln!(output, "        \"publication\": \"{AMD_PPR_PUBLICATION}\",")?;
    writeln!(output, "        \"revision\": \"{AMD_PPR_REVISION}\",")?;
    writeln!(output, "        \"date\": \"{AMD_PPR_DATE}\",")?;
    writeln!(output, "        \"coverage\": \"{AMD_PPR_COVERAGE}\",")?;
    writeln!(output, "        \"sha256\": \"{AMD_PPR_SHA256}\"")?;
    writeln!(output, "      }}")?;
    writeln!(output, "    }},")?;
    writeln!(output, "    \"access\": {{")?;
    writeln!(
        output,
        "      \"msr_read_operations\": {},",
        section.access.read_operations
    )?;
    writeln!(
        output,
        "      \"msr_read_bytes\": {},",
        section.access.read_bytes
    )?;
    writeln!(
        output,
        "      \"msr_write_operations\": {}",
        section.access.write_operations
    )?;
    writeln!(output, "    }},")?;
    write!(output, "    \"bsp\": ")?;
    write_system_registers_value(output, section.bsp, "    ")?;
    writeln!(output)?;
    write!(output, "  }}")
}

fn write_system_registers_fragment(
    output: &mut impl Write,
    evidence: SystemRegistersEvidence<'_>,
) -> fmt::Result {
    write!(output, "  \"system_registers\": ")?;
    write_system_registers_value(output, evidence, "  ")
}

fn write_system_registers_value(
    output: &mut impl Write,
    evidence: SystemRegistersEvidence<'_>,
    indent: &str,
) -> fmt::Result {
    match evidence {
        SystemRegistersEvidence::NotAttempted { reason } => {
            write!(output, "{{ \"status\": \"not-attempted\", \"reason\": ")?;
            write_json_string(output, reason)?;
            write!(output, " }}")
        }
        SystemRegistersEvidence::Observed(inventory) => {
            write_system_register_inventory(output, &inventory, indent)
        }
    }
}

#[allow(clippy::too_many_lines)]
fn write_system_register_inventory(
    output: &mut impl Write,
    inventory: &SystemRegisterInventory,
    indent: &str,
) -> fmt::Result {
    let field = alloc::format!("{indent}  ");
    let field = field.as_str();
    let cap = decode_mtrr_cap(inventory.mtrr_cap).map_err(|_| fmt::Error)?;
    let def_type = decode_mtrr_def_type(inventory.mtrr_def_type);
    let sys_cfg = decode_sys_cfg(inventory.sys_cfg);
    let hwcr = decode_hwcr(inventory.hwcr);
    let smm_mask = decode_smm_mask(inventory.smm_mask);
    writeln!(output, "{{")?;
    writeln!(output, "{field}\"status\": \"observed\",")?;
    writeln!(
        output,
        "{field}\"mtrr_cap\": {{ \"raw\": \"0x{:016x}\", \"vcnt\": {}, \"fix\": {}, \"wc\": {}, \"smrr\": {} }},",
        inventory.mtrr_cap, cap.vcnt, cap.fix, cap.wc, cap.smrr
    )?;
    writeln!(
        output,
        "{field}\"mtrr_def_type\": {{ \"raw\": \"0x{:016x}\", \"mem_type\": \"0x{:02x}\", \"fixed_range_enable\": {}, \"mtrr_def_type_en\": {} }},",
        inventory.mtrr_def_type, def_type.mem_type, def_type.fixed_range_enable,
        def_type.mtrr_def_type_en
    )?;
    writeln!(output, "{field}\"pat\": {{ \"raw\": \"0x{:016x}\" }},", inventory.pat)?;
    writeln!(output, "{field}\"variable_mtrrs\": {{")?;
    writeln!(
        output,
        "{field}  \"pair_count\": {},",
        inventory.variable_mtrr_pair_count
    )?;
    writeln!(output, "{field}  \"pairs\": [")?;
    for index in 0..usize::from(inventory.variable_mtrr_pair_count) {
        let pair = inventory.variable_mtrr_pairs[index];
        let (mem_type, phys_base) = decode_variable_mtrr_base(pair.base);
        let (valid, phys_mask) = decode_variable_mtrr_mask(pair.mask);
        writeln!(
            output,
            "{field}    {{ \"index\": {}, \"base_raw\": \"0x{:016x}\", \"mask_raw\": \"0x{:016x}\", \"mem_type\": \"0x{:02x}\", \"phys_base\": \"0x{:016x}\", \"phys_mask\": \"0x{:016x}\", \"valid\": {} }}{}",
            index,
            pair.base,
            pair.mask,
            mem_type,
            phys_base,
            phys_mask,
            valid,
            if index + 1 == usize::from(inventory.variable_mtrr_pair_count) {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "{field}  ]")?;
    writeln!(output, "{field}}},")?;
    if inventory.fixed_mtrr_observed {
        write!(
            output,
            "{field}\"fixed_mtrrs\": {{ \"status\": \"observed\", \"raw\": ["
        )?;
        for (index, raw) in inventory.fixed_mtrr.iter().enumerate() {
            write!(
                output,
                "\"0x{raw:016x}\"{}",
                if index + 1 == inventory.fixed_mtrr.len() {
                    ""
                } else {
                    ", "
                }
            )?;
        }
        writeln!(output, "] }},")?;
    } else {
        writeln!(
            output,
            "{field}\"fixed_mtrrs\": {{ \"status\": \"not-attempted\", \"reason\": \"mtrrcap-fix-clear\" }},"
        )?;
    }
    writeln!(
        output,
        "{field}\"sys_cfg\": {{ \"raw\": \"0x{:016x}\", \"mtrr_fix_dram_en\": {}, \"mtrr_fix_dram_mod_en\": {}, \"mtrr_var_dram_en\": {}, \"mtrr_tom2_en\": {}, \"tom2_force_mem_type_wb\": {}, \"smee_raw\": {}, \"secure_nested_paging_en_raw\": {}, \"vmpl_en_raw\": {}, \"hmkee_raw\": {}, \"encryption_state_claim\": false }},",
        inventory.sys_cfg,
        sys_cfg.mtrr_fix_dram_en,
        sys_cfg.mtrr_fix_dram_mod_en,
        sys_cfg.mtrr_var_dram_en,
        sys_cfg.mtrr_tom2_en,
        sys_cfg.tom2_force_mem_type_wb,
        sys_cfg.smee,
        sys_cfg.secure_nested_paging_en,
        sys_cfg.vmpl_en,
        sys_cfg.hmkee
    )?;
    writeln!(
        output,
        "{field}\"hwcr\": {{ \"raw\": \"0x{:016x}\", \"smm_lock\": {}, \"smm_pg_cfg_lock\": {} }},",
        inventory.hwcr, hwcr.smm_lock, hwcr.smm_pg_cfg_lock
    )?;
    writeln!(
        output,
        "{field}\"top_mem\": {{ \"raw\": \"0x{:016x}\", \"tom\": \"0x{:016x}\" }},",
        inventory.top_mem,
        top_mem_address(inventory.top_mem)
    )?;
    writeln!(
        output,
        "{field}\"tom2\": {{ \"raw\": \"0x{:016x}\", \"tom2\": \"0x{:016x}\" }},",
        inventory.tom2,
        tom2_address(inventory.tom2)
    )?;
    writeln!(
        output,
        "{field}\"smm_base\": {{ \"raw\": \"0x{:016x}\", \"smm_base_address\": \"0x{:08x}\" }},",
        inventory.smm_base,
        smm_base_address(inventory.smm_base)
    )?;
    writeln!(
        output,
        "{field}\"smm_addr\": {{ \"raw\": \"0x{:016x}\", \"tseg_base\": \"0x{:016x}\" }},",
        inventory.smm_addr,
        smm_tseg_base(inventory.smm_addr)
    )?;
    writeln!(
        output,
        "{field}\"smm_mask\": {{ \"raw\": \"0x{:016x}\", \"tseg_mask\": \"0x{:016x}\", \"a_valid\": {}, \"t_valid\": {}, \"a_close\": {}, \"t_close\": {}, \"am_type_io_wc\": {}, \"tm_type_io_wc\": {}, \"am_type_dram\": \"0x{:02x}\", \"tm_type_dram\": \"0x{:02x}\" }},",
        inventory.smm_mask,
        smm_mask.tseg_mask,
        smm_mask.a_valid,
        smm_mask.t_valid,
        smm_mask.a_close,
        smm_mask.t_close,
        smm_mask.am_type_io_wc,
        smm_mask.tm_type_io_wc,
        smm_mask.am_type_dram,
        smm_mask.tm_type_dram
    )?;
    match inventory.iorr_not_attempted_reason {
        Some(reason) => {
            write!(
                output,
                "{field}\"iorr\": {{ \"status\": \"not-attempted\", \"reason\": "
            )?;
            write_json_string(output, reason)?;
            writeln!(output, " }},")?;
        }
        None => {
            writeln!(output, "{field}\"iorr\": {{ \"status\": \"observed\", \"ranges\": [")?;
            for index in 0..inventory.iorr_base.len() {
                let base = decode_iorr_base(inventory.iorr_base[index]);
                let mask = decode_iorr_mask(inventory.iorr_mask[index]);
                writeln!(
                    output,
                    "{field}  {{ \"index\": {}, \"base_raw\": \"0x{:016x}\", \"mask_raw\": \"0x{:016x}\", \"phys_base\": \"0x{:016x}\", \"phys_mask\": \"0x{:016x}\", \"rd_mem\": {}, \"wr_mem\": {}, \"valid\": {} }}{}",
                    index,
                    inventory.iorr_base[index],
                    inventory.iorr_mask[index],
                    base.phys_base,
                    mask.phys_mask,
                    base.rd_mem,
                    base.wr_mem,
                    mask.valid,
                    if index + 1 == inventory.iorr_base.len() { "" } else { "," }
                )?;
            }
            writeln!(output, "{field}] }},")?;
        }
    }
    writeln!(output, "{field}\"smm_immutability_claim\": false")?;
    write!(output, "{indent}}}")
}

fn write_processor_consistency(
    output: &mut impl Write,
    consistency: &ProcessorConsistencyEvidence<'_>,
) -> fmt::Result {
    let bsp = consistency.bsp_observation().ok_or(fmt::Error)?;
    let observation_count = consistency.observations.len();
    writeln!(output, "  \"processor_consistency\": {{")?;
    writeln!(output, "    \"status\": \"observed\",")?;
    writeln!(
        output,
        "    \"scope\": \"all-enabled-healthy-processors-from-matching-pre-and-post-mp-services-enumerations\","
    )?;
    writeln!(output, "    \"dispatch\": {{")?;
    writeln!(
        output,
        "      \"method\": \"bsp-local-plus-blocking-sequential-startup-this-ap\","
    )?;
    writeln!(
        output,
        "      \"timeout_microseconds_per_ap\": {},",
        consistency.timeout_microseconds_per_ap
    )?;
    writeln!(
        output,
        "      \"callback_boot_services_table_calls\": false,"
    )?;
    writeln!(output, "      \"callback_mp_services_who_am_i\": true,")?;
    writeln!(output, "      \"callback_control_state_writes\": false,")?;
    writeln!(
        output,
        "      \"firmware_dispatch_mechanism\": \"opaque-uefi-mp-services\","
    )?;
    writeln!(
        output,
        "      \"firmware_dispatch_may_use_init_sipi_or_reset\": true,"
    )?;
    writeln!(
        output,
        "      \"pre_dispatch_vm_cr_preservation\": \"not-proven\","
    )?;
    writeln!(
        output,
        "      \"mp_services_revalidated_after_dispatch\": true"
    )?;
    writeln!(output, "    }},")?;
    writeln!(output, "    \"comparison_policy\": {{")?;
    writeln!(output, "      \"reference\": \"uefi-mp-services-bsp\",")?;
    writeln!(output, "      \"raw_cpuid_compared\": true,")?;
    writeln!(
        output,
        "      \"leaf_00000001_ebx_compared_mask\": \"0x00ffffff\","
    )?;
    writeln!(
        output,
        "      \"leaf_8000001e_eax_compared_mask\": \"0x00000000\","
    )?;
    writeln!(
        output,
        "      \"leaf_8000001e_ebx_compared_mask\": \"0xffffff00\","
    )?;
    writeln!(
        output,
        "      \"leaf_8000001e_ecx_compared_mask\": \"0xffffff00\","
    )?;
    writeln!(
        output,
        "      \"leaf_8000001e_edx_compared_mask\": \"0xffffffff\","
    )?;
    writeln!(
        output,
        "      \"all_other_collected_register_masks\": \"0xffffffff\","
    )?;
    writeln!(
        output,
        "      \"identity_comparison\": \"cpuid-apic-low8-matches-pi-processor-id-low8-with-zero-reserved-bits\","
    )?;
    writeln!(
        output,
        "      \"vm_cr_comparison\": \"exact-raw-and-observation-status\","
    )?;
    writeln!(
        output,
        "      \"system_registers_comparison\": \"exact-raw-and-observation-status-except-thread-scoped-smm-base\""
    )?;
    writeln!(output, "    }},")?;
    writeln!(
        output,
        "    \"bsp_processor_number\": {},",
        consistency.bsp_processor_number
    )?;
    writeln!(
        output,
        "    \"enabled_processor_count\": {observation_count},"
    )?;
    writeln!(output, "    \"observation_count\": {observation_count},")?;
    writeln!(output, "    \"all_enabled_processors_observed\": true,")?;
    writeln!(output, "    \"observations\": [")?;
    for (index, observation) in consistency.observations.iter().enumerate() {
        writeln!(output, "      {{")?;
        writeln!(
            output,
            "        \"processor_number\": {},",
            observation.processor_number
        )?;
        writeln!(
            output,
            "        \"processor_id\": \"0x{:016x}\",",
            observation.processor_id
        )?;
        writeln!(output, "        \"bsp\": {},", observation.is_bsp)?;
        let dispatch = match observation.dispatch {
            ProcessorDispatch::BspDirect => "bsp-direct",
            ProcessorDispatch::StartupThisApSuccess => "startup-this-ap-success",
        };
        writeln!(output, "        \"dispatch\": \"{dispatch}\",")?;
        writeln!(output, "        \"who_am_i\": {{")?;
        writeln!(output, "          \"status\": \"success\",")?;
        writeln!(
            output,
            "          \"processor_number\": {}",
            observation.who_am_i_processor_number
        )?;
        writeln!(output, "        }},")?;

        let mut cpu = String::new();
        write_cpu(
            &mut cpu,
            &observation.cpu,
            "processor-selected-by-uefi-mp-services",
        )?;
        write_indented_fragment(output, &cpu, "      ")?;
        writeln!(output, ",")?;

        let mut vm_cr = String::new();
        write_vm_cr(&mut vm_cr, observation.vm_cr)?;
        write_indented_fragment(output, &vm_cr, "      ")?;
        writeln!(output, ",")?;

        let mut system_registers = String::new();
        write_system_registers_fragment(&mut system_registers, observation.system_registers)?;
        write_indented_fragment(output, &system_registers, "      ")?;
        writeln!(output, ",")?;

        write!(output, "        \"leaf_00000001_initial_apic_id\": ")?;
        write_optional_hex8(output, initial_apic_id(&observation.cpu))?;
        writeln!(output, ",")?;
        write!(output, "        \"leaf_8000001e_extended_apic_id\": ")?;
        write_optional_hex32(output, extended_apic_id(&observation.cpu))?;
        writeln!(output, ",")?;
        writeln!(
            output,
            "        \"identity_matches_mp\": {},",
            observation.identity_matches_mp_services()
        )?;
        writeln!(
            output,
            "        \"cpuid_matches_bsp\": {},",
            observation.cpuid_matches(bsp)
        )?;
        writeln!(
            output,
            "        \"vm_cr_matches_bsp\": {},",
            observation.vm_cr_matches(bsp)
        )?;
        writeln!(
            output,
            "        \"system_registers_matches_bsp\": {}",
            observation.system_registers_matches(bsp)
        )?;
        writeln!(
            output,
            "      }}{}",
            if index + 1 == observation_count {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "    ],")?;
    writeln!(
        output,
        "    \"identity_consistent\": {},",
        consistency.identity_consistent()
    )?;
    writeln!(
        output,
        "    \"cpuid_consistent\": {},",
        consistency.cpuid_consistent()
    )?;
    writeln!(
        output,
        "    \"vm_cr_consistent\": {},",
        consistency.vm_cr_consistent()
    )?;
    writeln!(
        output,
        "    \"system_registers_consistent\": {},",
        consistency.system_registers_consistent()
    )?;
    writeln!(output, "    \"consistent\": {}", consistency.consistent())?;
    write!(output, "  }}")
}

fn write_indented_fragment(
    output: &mut impl Write,
    fragment: &str,
    additional_indent: &str,
) -> fmt::Result {
    for (index, line) in fragment.lines().enumerate() {
        if index != 0 {
            writeln!(output)?;
        }
        write!(output, "{additional_indent}{line}")?;
    }
    Ok(())
}

fn write_optional_hex8(output: &mut impl Write, value: Option<u8>) -> fmt::Result {
    match value {
        Some(value) => write!(output, "\"0x{value:02x}\""),
        None => write!(output, "null"),
    }
}

fn write_optional_hex32(output: &mut impl Write, value: Option<u32>) -> fmt::Result {
    match value {
        Some(value) => write!(output, "\"0x{value:08x}\""),
        None => write!(output, "null"),
    }
}

fn write_mp_services(output: &mut impl Write, mp: MpServicesEvidence<'_>) -> fmt::Result {
    writeln!(output, "  \"mp_services\": {{")?;
    match mp {
        MpServicesEvidence::Unavailable { uefi_status } => {
            writeln!(output, "    \"status\": \"unavailable\",")?;
            writeln!(output, "    \"uefi_status\": \"0x{uefi_status:016x}\"")?;
        }
        MpServicesEvidence::Observed {
            total,
            enabled,
            processors,
        } => {
            let enabled_records = processors
                .iter()
                .filter(|processor| processor.enabled)
                .count();
            writeln!(output, "    \"status\": \"observed\",")?;
            writeln!(output, "    \"total\": {total},")?;
            writeln!(output, "    \"enabled\": {enabled},")?;
            writeln!(output, "    \"record_count\": {},", processors.len())?;
            writeln!(output, "    \"enabled_record_count\": {enabled_records},")?;
            writeln!(
                output,
                "    \"counts_consistent\": {},",
                total == processors.len() && enabled == enabled_records
            )?;
            writeln!(output, "    \"processors\": [")?;
            for (index, processor) in processors.iter().enumerate() {
                write!(
                    output,
                    "      {{ \"processor_number\": {}, \"processor_id\": \"0x{:016x}\", \"id_semantics\": \"uefi-mp-services-processor-id\", \"bsp\": {}, \"enabled\": {}, \"healthy\": {}, \"location\": {{ \"package\": {}, \"core\": {}, \"thread\": {} }} }}",
                    processor.processor_number,
                    processor.processor_id,
                    processor.is_bsp,
                    processor.enabled,
                    processor.healthy,
                    processor.package,
                    processor.core,
                    processor.thread,
                )?;
                writeln!(
                    output,
                    "{}",
                    if index + 1 == processors.len() {
                        ""
                    } else {
                        ","
                    }
                )?;
            }
            writeln!(output, "    ]")?;
        }
    }
    write!(output, "  }}")
}

fn write_memory_map(output: &mut impl Write, map: MemoryMapEvidence<'_>) -> fmt::Result {
    writeln!(output, "  \"memory_map\": {{")?;
    match map {
        MemoryMapEvidence::Unavailable { uefi_status } => {
            writeln!(output, "    \"status\": \"unavailable\",")?;
            writeln!(
                output,
                "    \"phase\": \"collection-time-not-final-exit-boot-services-map\","
            )?;
            writeln!(output, "    \"uefi_status\": \"0x{uefi_status:016x}\"")?;
        }
        MemoryMapEvidence::Observed {
            descriptor_size,
            descriptor_version,
            descriptors,
        } => {
            writeln!(output, "    \"status\": \"observed\",")?;
            writeln!(
                output,
                "    \"phase\": \"collection-time-not-final-exit-boot-services-map\","
            )?;
            writeln!(output, "    \"descriptor_size\": {descriptor_size},")?;
            writeln!(output, "    \"descriptor_version\": {descriptor_version},")?;
            writeln!(output, "    \"descriptors\": [")?;
            for (index, descriptor) in descriptors.iter().enumerate() {
                write!(
                    output,
                    "      {{ \"type\": \"0x{:08x}\", \"physical_start\": \"0x{:016x}\", \"virtual_start\": \"0x{:016x}\", \"page_count\": {}, \"attributes\": \"0x{:016x}\" }}",
                    descriptor.memory_type,
                    descriptor.physical_start,
                    descriptor.virtual_start,
                    descriptor.page_count,
                    descriptor.attributes,
                )?;
                writeln!(
                    output,
                    "{}",
                    if index + 1 == descriptors.len() {
                        ""
                    } else {
                        ","
                    }
                )?;
            }
            writeln!(output, "    ]")?;
        }
    }
    write!(output, "  }}")
}

fn write_acpi(output: &mut impl Write, acpi: &AcpiEvidence<'_>) -> fmt::Result {
    if acpi.selection_address != acpi.rsdp.address {
        return Err(fmt::Error);
    }

    writeln!(output, "  \"acpi\": {{")?;
    writeln!(output, "    \"status\": \"observed\",")?;
    writeln!(
        output,
        "    \"semantics\": \"firmware-description-only-no-register-or-pci-access\","
    )?;
    writeln!(output, "    \"specification\": {{")?;
    writeln!(
        output,
        "      \"publisher\": \"{AMD_IOMMU_SPEC_PUBLISHER}\","
    )?;
    writeln!(
        output,
        "      \"publication\": \"{AMD_IOMMU_SPEC_PUBLICATION}\","
    )?;
    writeln!(output, "      \"revision\": \"{AMD_IOMMU_SPEC_REVISION}\",")?;
    writeln!(output, "      \"date\": \"{AMD_IOMMU_SPEC_DATE}\",")?;
    writeln!(output, "      \"sha256\": \"{AMD_IOMMU_SPEC_SHA256}\"")?;
    writeln!(output, "    }},")?;
    writeln!(output, "    \"limits\": {{")?;
    writeln!(output, "      \"max_rsdp_bytes\": 4096,")?;
    writeln!(output, "      \"max_sdt_bytes\": 1048576,")?;
    writeln!(output, "      \"max_total_acpi_bytes\": 4194304,")?;
    writeln!(output, "      \"max_configuration_tables\": 64,")?;
    writeln!(output, "      \"max_root_entries\": 256,")?;
    writeln!(output, "      \"max_unique_root_pointers\": 256,")?;
    writeln!(output, "      \"max_madt_entries\": 512,")?;
    writeln!(output, "      \"max_madt_processor_entries\": 256,")?;
    writeln!(output, "      \"max_mcfg_allocations\": 256,")?;
    writeln!(output, "      \"max_ivrs_blocks\": 256,")?;
    writeln!(output, "      \"max_ivrs_device_entries\": 4096")?;
    writeln!(output, "    }},")?;
    writeln!(output, "    \"selection\": {{")?;
    writeln!(output, "      \"source_kind\": \"acpi2-rsdp\",")?;
    writeln!(
        output,
        "      \"source_guid\": \"8868e871-e4f1-11d3-bc22-0080c73c8881\","
    )?;
    writeln!(
        output,
        "      \"address\": \"0x{:016x}\",",
        acpi.selection_address
    )?;
    writeln!(output, "      \"rule\": \"acpi2-preferred-over-acpi1\"")?;
    writeln!(output, "    }},")?;
    write_rsdp(output, &acpi.rsdp)?;
    writeln!(output, ",")?;
    writeln!(output, "    \"roots\": {{")?;
    write_root(output, "rsdt", 4, &acpi.rsdt, true)?;
    write_root(output, "xsdt", 8, &acpi.xsdt, false)?;
    writeln!(output, "    }},")?;
    write_directory(output, acpi.directory)?;
    writeln!(output, ",")?;
    write_acpi_tables(output, &acpi.tables)?;
    writeln!(output, ",")?;
    write_madt_mp_cross_check(output, &acpi.madt_mp_cross_check)?;
    writeln!(output, ",")?;
    writeln!(output, "    \"ivrs_claims\": {{")?;
    writeln!(output, "      \"table_present\": true,")?;
    writeln!(output, "      \"runtime_ownership_assessed\": false,")?;
    writeln!(output, "      \"runtime_ownership_claim\": false,")?;
    writeln!(output, "      \"pci_isolation_assessed\": false,")?;
    writeln!(output, "      \"pci_isolation_claim\": false,")?;
    writeln!(output, "      \"semantics\": \"firmware-description-only\"")?;
    writeln!(output, "    }}")?;
    write!(output, "  }}")
}

fn write_rsdp(output: &mut impl Write, rsdp: &RsdpEvidence<'_>) -> fmt::Result {
    writeln!(output, "    \"rsdp\": {{")?;
    writeln!(output, "      \"address\": \"0x{:016x}\",", rsdp.address)?;
    write!(output, "      \"raw\": ")?;
    write_raw_envelope(output, rsdp.raw)?;
    writeln!(output, ",")?;
    writeln!(output, "      \"signature\": \"RSD PTR \",")?;
    writeln!(output, "      \"checksum\": \"0x{:02x}\",", rsdp.checksum)?;
    write!(output, "      \"oem_id_hex\": \"")?;
    write_lower_hex_bytes(output, &rsdp.oem_id)?;
    writeln!(output, "\",")?;
    writeln!(output, "      \"revision\": {},", rsdp.revision)?;
    writeln!(output, "      \"length\": {},", rsdp.length)?;
    writeln!(
        output,
        "      \"rsdt_address\": \"0x{:08x}\",",
        rsdp.rsdt_address
    )?;
    writeln!(
        output,
        "      \"xsdt_address\": \"0x{:016x}\",",
        rsdp.xsdt_address
    )?;
    writeln!(
        output,
        "      \"extended_checksum\": \"0x{:02x}\",",
        rsdp.extended_checksum
    )?;
    write!(output, "      \"reserved_hex\": \"")?;
    write_lower_hex_bytes(output, &rsdp.reserved)?;
    writeln!(output, "\"")?;
    write!(output, "    }}")
}

fn write_root(
    output: &mut impl Write,
    kind: &str,
    entry_width: u8,
    root: &AcpiRootEvidence<'_>,
    comma: bool,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, kind)?;
    writeln!(output, ": {{")?;
    write!(output, "        \"kind\": ")?;
    write_json_string(output, kind)?;
    writeln!(output, ",")?;
    writeln!(output, "        \"address\": \"0x{:016x}\",", root.address)?;
    write!(output, "        \"raw\": ")?;
    write_raw_envelope(output, root.raw)?;
    writeln!(output, ",")?;
    write!(output, "        \"header\": ")?;
    write_sdt_header(output, &root.header)?;
    writeln!(output, ",")?;
    writeln!(output, "        \"entry_width\": {entry_width},")?;
    writeln!(output, "        \"entry_count\": {},", root.entries.len())?;
    writeln!(output, "        \"entries\": [")?;
    for (index, address) in root.entries.iter().enumerate() {
        write!(output, "          \"0x{address:016x}\"")?;
        writeln!(
            output,
            "{}",
            if index + 1 == root.entries.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "        ]")?;
    writeln!(output, "      }}{}", if comma { "," } else { "" })
}

fn write_directory(output: &mut impl Write, directory: &[AcpiDirectoryRecord<'_>]) -> fmt::Result {
    writeln!(output, "    \"directory\": [")?;
    for (index, record) in directory.iter().enumerate() {
        writeln!(output, "      {{")?;
        writeln!(
            output,
            "        \"address\": \"0x{:016x}\",",
            record.address
        )?;
        write!(output, "        \"referenced_by\": ")?;
        write_root_references(output, record.referenced_by)?;
        writeln!(output, ",")?;
        write!(output, "        \"header_raw\": ")?;
        write_raw_envelope(output, record.header_raw)?;
        writeln!(output, ",")?;
        write!(output, "        \"signature\": ")?;
        write_fixed_ascii_string(output, &record.signature)?;
        writeln!(output, ",")?;
        writeln!(
            output,
            "        \"declared_length\": {},",
            record.declared_length
        )?;
        writeln!(output, "        \"revision\": {}", record.revision)?;
        write!(output, "      }}")?;
        writeln!(
            output,
            "{}",
            if index + 1 == directory.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    write!(output, "    ]")
}

fn write_acpi_tables(output: &mut impl Write, tables: &AcpiTablesEvidence<'_>) -> fmt::Result {
    writeln!(output, "    \"tables\": {{")?;
    write_madt_table(output, &tables.madt)?;
    writeln!(output, ",")?;
    write_mcfg_table(output, &tables.mcfg)?;
    writeln!(output, ",")?;
    write_ivrs_table(output, &tables.ivrs)?;
    writeln!(output, ",")?;
    write_fadt_table(output, &tables.fadt)?;
    writeln!(output)?;
    write!(output, "    }}")
}

fn write_table_prefix<T>(
    output: &mut impl Write,
    name: &str,
    table: &AcpiTableEvidence<'_, T>,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, name)?;
    writeln!(output, ": {{")?;
    writeln!(output, "        \"address\": \"0x{:016x}\",", table.address)?;
    write!(output, "        \"referenced_by\": ")?;
    write_root_references(output, table.referenced_by)?;
    writeln!(output, ",")?;
    write!(output, "        \"raw\": ")?;
    write_raw_envelope(output, table.raw)?;
    writeln!(output, ",")?;
    write!(output, "        \"header\": ")?;
    write_sdt_header(output, &table.header)?;
    writeln!(output, ",")?;
    writeln!(output, "        \"body\": {{")
}

fn write_madt_table(
    output: &mut impl Write,
    table: &AcpiTableEvidence<'_, MadtBodyEvidence<'_>>,
) -> fmt::Result {
    write_table_prefix(output, "madt", table)?;
    let processor_entry_count = table
        .body
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                MadtEntryEvidence::ProcessorLocalApic { .. }
                    | MadtEntryEvidence::ProcessorLocalX2Apic { .. }
            )
        })
        .count();
    let enabled_processor_count = table
        .body
        .entries
        .iter()
        .filter(|entry| match entry {
            MadtEntryEvidence::ProcessorLocalApic { flags, .. }
            | MadtEntryEvidence::ProcessorLocalX2Apic { flags, .. } => flags & 1 != 0,
            MadtEntryEvidence::Other { .. } => false,
        })
        .count();
    writeln!(
        output,
        "          \"local_apic_address\": \"0x{:08x}\",",
        table.body.local_apic_address
    )?;
    writeln!(
        output,
        "          \"flags\": \"0x{:08x}\",",
        table.body.flags
    )?;
    writeln!(
        output,
        "          \"entry_count\": {},",
        table.body.entries.len()
    )?;
    writeln!(
        output,
        "          \"processor_entry_count\": {processor_entry_count},"
    )?;
    writeln!(
        output,
        "          \"enabled_processor_count\": {enabled_processor_count},"
    )?;
    writeln!(output, "          \"entries\": [")?;
    for (index, entry) in table.body.entries.iter().enumerate() {
        write_madt_entry(output, entry)?;
        writeln!(
            output,
            "{}",
            if index + 1 == table.body.entries.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "          ]")?;
    writeln!(output, "        }}")?;
    write!(output, "      }}")
}

fn write_madt_entry(output: &mut impl Write, entry: &MadtEntryEvidence<'_>) -> fmt::Result {
    match entry {
        MadtEntryEvidence::ProcessorLocalApic {
            offset,
            raw,
            acpi_processor_uid,
            apic_id,
            flags,
        } => {
            write!(
                output,
                "            {{ \"kind\": \"processor-local-apic\", \"type\": 0, \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
                raw.len(),
                checked_u32(*offset)?
            )?;
            write_sha256(output, raw)?;
            write!(
                output,
                "\", \"acpi_processor_uid\": {acpi_processor_uid}, \"apic_id\": \"0x{apic_id:02x}\", \"flags\": \"0x{flags:08x}\", \"enabled\": {}, \"online_capable\": {} }}",
                flags & 1 != 0,
                flags & 2 != 0
            )
        }
        MadtEntryEvidence::ProcessorLocalX2Apic {
            offset,
            raw,
            reserved,
            x2apic_id,
            flags,
            acpi_processor_uid,
        } => {
            write!(
                output,
                "            {{ \"kind\": \"processor-local-x2apic\", \"type\": 9, \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
                raw.len(),
                checked_u32(*offset)?
            )?;
            write_sha256(output, raw)?;
            write!(
                output,
                "\", \"reserved\": \"0x{reserved:04x}\", \"x2apic_id\": \"0x{x2apic_id:08x}\", \"flags\": \"0x{flags:08x}\", \"acpi_processor_uid\": {acpi_processor_uid}, \"enabled\": {}, \"online_capable\": {} }}",
                flags & 1 != 0,
                flags & 2 != 0
            )
        }
        MadtEntryEvidence::Other {
            entry_type,
            offset,
            raw,
        } => {
            write!(
                output,
                "            {{ \"kind\": \"other\", \"type\": {entry_type}, \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
                raw.len(),
                checked_u32(*offset)?
            )?;
            write_sha256(output, raw)?;
            write!(output, "\" }}")
        }
    }
}

fn write_mcfg_table(
    output: &mut impl Write,
    table: &AcpiTableEvidence<'_, McfgBodyEvidence<'_>>,
) -> fmt::Result {
    write_table_prefix(output, "mcfg", table)?;
    write!(output, "          \"reserved_hex\": \"")?;
    write_lower_hex_bytes(output, &table.body.reserved)?;
    writeln!(output, "\",")?;
    writeln!(
        output,
        "          \"allocation_count\": {},",
        table.body.allocations.len()
    )?;
    writeln!(output, "          \"allocations\": [")?;
    for (index, allocation) in table.body.allocations.iter().enumerate() {
        write!(
            output,
            "            {{ \"offset\": {}, \"base_address\": \"0x{:016x}\", \"segment_group\": {}, \"start_bus\": {}, \"end_bus\": {}, \"reserved_hex\": \"",
            checked_u32(allocation.offset)?,
            allocation.base_address,
            allocation.segment_group,
            allocation.start_bus,
            allocation.end_bus
        )?;
        write_lower_hex_bytes(output, &allocation.reserved)?;
        write!(
            output,
            "\", \"window_end_exclusive\": \"0x{:016x}\" }}",
            allocation.window_end_exclusive
        )?;
        writeln!(
            output,
            "{}",
            if index + 1 == table.body.allocations.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "          ]")?;
    writeln!(output, "        }}")?;
    write!(output, "      }}")
}

fn write_ivrs_table(
    output: &mut impl Write,
    table: &AcpiTableEvidence<'_, IvrsBodyEvidence<'_>>,
) -> fmt::Result {
    write_table_prefix(output, "ivrs", table)?;
    let device_entry_count = table.body.blocks.iter().try_fold(0_usize, |count, block| {
        let additional = match block {
            IvrsBlockEvidence::Ivhd(ivhd) => ivhd.device_entries.len(),
            IvrsBlockEvidence::Ivmd(_) => 0,
        };
        count.checked_add(additional).ok_or(fmt::Error)
    })?;
    writeln!(
        output,
        "          \"iv_info\": \"0x{:08x}\",",
        table.body.iv_info
    )?;
    write!(output, "          \"reserved_hex\": \"")?;
    write_lower_hex_bytes(output, &table.body.reserved)?;
    writeln!(output, "\",")?;
    writeln!(
        output,
        "          \"block_count\": {},",
        table.body.blocks.len()
    )?;
    writeln!(
        output,
        "          \"device_entry_count\": {device_entry_count},"
    )?;
    writeln!(output, "          \"blocks\": [")?;
    for (index, block) in table.body.blocks.iter().enumerate() {
        match block {
            IvrsBlockEvidence::Ivhd(ivhd) => write_ivhd(output, ivhd)?,
            IvrsBlockEvidence::Ivmd(ivmd) => write_ivmd(output, ivmd)?,
        }
        writeln!(
            output,
            "{}",
            if index + 1 == table.body.blocks.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    writeln!(output, "          ]")?;
    writeln!(output, "        }}")?;
    write!(output, "      }}")
}

fn write_ivhd(output: &mut impl Write, ivhd: &IvhdEvidence<'_>) -> fmt::Result {
    write!(
        output,
        "            {{ \"kind\": \"ivhd\", \"type\": \"0x{:02x}\", \"flags\": \"0x{:02x}\", \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
        ivhd.entry_type,
        ivhd.flags,
        checked_u16(ivhd.raw.len())?,
        checked_u32(ivhd.offset)?
    )?;
    write_sha256(output, ivhd.raw)?;
    write!(
        output,
        "\", \"header_length\": {}, \"device_id\": \"0x{:04x}\", \"capability_offset\": \"0x{:04x}\", \"iommu_base_address\": \"0x{:016x}\", \"pci_segment_group\": {}, \"iommu_info\": \"0x{:04x}\", \"feature_info\": \"0x{:08x}\", \"extended_feature_image\": ",
        ivhd.header_length,
        ivhd.device_id,
        ivhd.capability_offset,
        ivhd.iommu_base_address,
        ivhd.pci_segment_group,
        ivhd.iommu_info,
        ivhd.feature_info
    )?;
    write_optional_hex64(output, ivhd.extended_feature_image)?;
    write!(output, ", \"extended_feature_image_2\": ")?;
    write_optional_hex64(output, ivhd.extended_feature_image_2)?;
    writeln!(
        output,
        ", \"device_entry_count\": {}, \"device_entries\": [",
        ivhd.device_entries.len()
    )?;
    for (index, entry) in ivhd.device_entries.iter().enumerate() {
        write_ivhd_device_entry(output, entry)?;
        writeln!(
            output,
            "{}",
            if index + 1 == ivhd.device_entries.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    write!(output, "              ] }}")
}

fn write_ivhd_device_entry(
    output: &mut impl Write,
    entry: &IvhdDeviceEntryEvidence<'_>,
) -> fmt::Result {
    write!(
        output,
        "              {{ \"type\": \"0x{:02x}\", \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
        entry.entry_type,
        entry.raw.len(),
        checked_u32(entry.offset)?
    )?;
    write_sha256(output, entry.raw)?;
    write!(output, "\"")?;
    if let Some(uid_length) = entry.uid_length {
        write!(output, ", \"uid_length\": {uid_length}")?;
    }
    write!(output, " }}")
}

fn write_ivmd(output: &mut impl Write, ivmd: &IvmdEvidence<'_>) -> fmt::Result {
    write!(
        output,
        "            {{ \"kind\": \"ivmd\", \"type\": \"0x{:02x}\", \"flags\": \"0x{:02x}\", \"length\": {}, \"offset\": {}, \"raw_sha256\": \"",
        ivmd.entry_type,
        ivmd.flags,
        checked_u16(ivmd.raw.len())?,
        checked_u32(ivmd.offset)?
    )?;
    write_sha256(output, ivmd.raw)?;
    write!(
        output,
        "\", \"device_id\": \"0x{:04x}\", \"auxiliary_data_or_end_device_id\": \"0x{:04x}\", \"pci_segment_group\": ",
        ivmd.device_id, ivmd.auxiliary_data_or_end_device_id
    )?;
    match ivmd.pci_segment_group {
        Some(segment) => write!(output, "{segment}")?,
        None => write!(output, "null")?,
    }
    write!(output, ", \"reserved_or_segment_area_hex\": \"")?;
    write_lower_hex_bytes(output, &ivmd.reserved_or_segment_area)?;
    write!(
        output,
        "\", \"start_address\": \"0x{:016x}\", \"memory_length\": \"0x{:016x}\", \"end_exclusive\": \"0x{:016x}\" }}",
        ivmd.start_address, ivmd.memory_length, ivmd.memory_end_exclusive
    )
}

fn write_fadt_table(
    output: &mut impl Write,
    table: &AcpiTableEvidence<'_, FadtBodyEvidence>,
) -> fmt::Result {
    write_table_prefix(output, "fadt", table)?;
    writeln!(
        output,
        "          \"firmware_ctrl_32\": \"0x{:08x}\",",
        table.body.firmware_ctrl_32
    )?;
    writeln!(
        output,
        "          \"dsdt_32\": \"0x{:08x}\",",
        table.body.dsdt_32
    )?;
    writeln!(
        output,
        "          \"preferred_pm_profile\": {},",
        table.body.preferred_pm_profile
    )?;
    writeln!(
        output,
        "          \"sci_interrupt\": {},",
        table.body.sci_interrupt
    )?;
    writeln!(
        output,
        "          \"iapc_boot_arch\": \"0x{:04x}\",",
        table.body.iapc_boot_arch
    )?;
    writeln!(
        output,
        "          \"flags\": \"0x{:08x}\",",
        table.body.flags
    )?;
    writeln!(
        output,
        "          \"minor_version\": {},",
        table.body.minor_version
    )?;
    writeln!(
        output,
        "          \"x_firmware_ctrl\": \"0x{:016x}\",",
        table.body.x_firmware_ctrl
    )?;
    writeln!(
        output,
        "          \"x_dsdt\": \"0x{:016x}\",",
        table.body.x_dsdt
    )?;
    writeln!(output, "          \"registers_accessed\": false,")?;
    writeln!(output, "          \"pointers_followed\": false")?;
    writeln!(output, "        }}")?;
    write!(output, "      }}")
}

fn write_madt_mp_cross_check(
    output: &mut impl Write,
    cross_check: &MadtMpCrossCheckEvidence<'_>,
) -> fmt::Result {
    writeln!(output, "    \"madt_mp_cross_check\": {{")?;
    writeln!(
        output,
        "      \"scope\": \"processor-hardware-id-and-enabled-membership-only\","
    )?;
    write_u64_hex_array(output, "mp_enabled_ids", cross_check.mp_enabled_ids, true)?;
    write_u64_hex_array(
        output,
        "madt_enabled_ids",
        cross_check.madt_enabled_ids,
        true,
    )?;
    write_u64_hex_array(
        output,
        "missing_from_madt",
        cross_check.missing_from_madt,
        true,
    )?;
    write_u64_hex_array(output, "missing_from_mp", cross_check.missing_from_mp, true)?;
    writeln!(
        output,
        "      \"mp_enabled_count\": {},",
        cross_check.mp_enabled_ids.len()
    )?;
    writeln!(
        output,
        "      \"madt_enabled_count\": {},",
        cross_check.madt_enabled_ids.len()
    )?;
    writeln!(output, "      \"consistent\": {}", cross_check.consistent)?;
    write!(output, "    }}")
}

fn write_u64_hex_array(
    output: &mut impl Write,
    name: &str,
    values: &[u64],
    comma: bool,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, name)?;
    write!(output, ": [")?;
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            write!(output, ",")?;
        }
        write!(output, "\"0x{value:016x}\"")?;
    }
    writeln!(output, "]{}", if comma { "," } else { "" })
}

fn write_root_references(
    output: &mut impl Write,
    referenced_by: AcpiRootReferences,
) -> fmt::Result {
    match referenced_by {
        AcpiRootReferences::Rsdt => write!(output, "[\"rsdt\"]"),
        AcpiRootReferences::Xsdt => write!(output, "[\"xsdt\"]"),
        AcpiRootReferences::RsdtAndXsdt => write!(output, "[\"rsdt\",\"xsdt\"]"),
    }
}

fn write_sdt_header(output: &mut impl Write, header: &AcpiSdtHeaderEvidence) -> fmt::Result {
    write!(output, "{{ \"signature\": ")?;
    write_fixed_ascii_string(output, &header.signature)?;
    write!(
        output,
        ", \"length\": {}, \"revision\": {}, \"checksum\": \"0x{:02x}\", \"oem_id_hex\": \"",
        header.length, header.revision, header.checksum
    )?;
    write_lower_hex_bytes(output, &header.oem_id)?;
    write!(output, "\", \"oem_table_id_hex\": \"")?;
    write_lower_hex_bytes(output, &header.oem_table_id)?;
    write!(
        output,
        "\", \"oem_revision\": \"0x{:08x}\", \"creator_id\": \"0x{:08x}\", \"creator_revision\": \"0x{:08x}\" }}",
        header.oem_revision,
        u32::from_le_bytes(header.creator_id),
        header.creator_revision
    )
}

fn write_raw_envelope(output: &mut impl Write, raw: RawBytesEvidence<'_>) -> fmt::Result {
    write!(
        output,
        "{{ \"encoding\": \"lowercase-hex\", \"length_bytes\": {}, \"sha256\": \"",
        checked_u32(raw.bytes.len())?
    )?;
    write_sha256(output, raw.bytes)?;
    write!(output, "\", \"bytes\": \"")?;
    write_lower_hex_bytes(output, raw.bytes)?;
    write!(output, "\" }}")
}

fn write_sha256(output: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    let digest = Sha256::digest(bytes);
    write_lower_hex_bytes(output, digest.as_slice())
}

fn write_lower_hex_bytes(output: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    const INPUT_CHUNK_BYTES: usize = 2048;
    let capacity = bytes
        .len()
        .min(INPUT_CHUNK_BYTES)
        .checked_mul(2)
        .ok_or(fmt::Error)?;
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(capacity)
        .map_err(|_| fmt::Error)?;
    for chunk in bytes.chunks(INPUT_CHUNK_BYTES) {
        encoded.clear();
        for byte in chunk {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output.write_str(&encoded)?;
    }
    Ok(())
}

fn write_fixed_ascii_string(output: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    write!(output, "\"")?;
    for byte in bytes {
        match byte {
            b'\"' => write!(output, "\\\"")?,
            b'\\' => write!(output, "\\\\")?,
            0x20..=0x7e => write!(output, "{}", char::from(*byte))?,
            _ => return Err(fmt::Error),
        }
    }
    write!(output, "\"")
}

fn write_optional_hex64(output: &mut impl Write, value: Option<u64>) -> fmt::Result {
    match value {
        Some(value) => write!(output, "\"0x{value:016x}\""),
        None => write!(output, "null"),
    }
}

fn checked_u16(value: usize) -> Result<u16, fmt::Error> {
    u16::try_from(value).map_err(|_| fmt::Error)
}

fn checked_u32(value: usize) -> Result<u32, fmt::Error> {
    u32::try_from(value).map_err(|_| fmt::Error)
}

fn write_raw_leaf(
    output: &mut impl Write,
    leaf: u32,
    subleaf: u32,
    registers: CpuidRegisters,
) -> fmt::Result {
    write!(
        output,
        "      {{ \"leaf\": \"0x{leaf:08x}\", \"subleaf\": \"0x{subleaf:08x}\", \"eax\": \"0x{:08x}\", \"ebx\": \"0x{:08x}\", \"ecx\": \"0x{:08x}\", \"edx\": \"0x{:08x}\" }}",
        registers.eax, registers.ebx, registers.ecx, registers.edx
    )
}

fn write_vendor_bool(
    output: &mut impl Write,
    name: &str,
    value: Option<bool>,
    authentic_amd: bool,
    comma: bool,
) -> fmt::Result {
    if authentic_amd {
        write_observed_bool(output, name, value, comma)
    } else {
        write_status_only(output, name, "not-applicable-vendor", comma)
    }
}

fn write_vendor_u8(
    output: &mut impl Write,
    name: &str,
    value: Option<u8>,
    authentic_amd: bool,
    comma: bool,
) -> fmt::Result {
    if authentic_amd {
        write_observed_u8(output, name, value, comma)
    } else {
        write_status_only(output, name, "not-applicable-vendor", comma)
    }
}

fn write_vendor_u32(
    output: &mut impl Write,
    name: &str,
    value: Option<u32>,
    authentic_amd: bool,
    comma: bool,
) -> fmt::Result {
    if authentic_amd {
        write_observed_u32(output, name, value, comma)
    } else {
        write_status_only(output, name, "not-applicable-vendor", comma)
    }
}

fn write_status_only(
    output: &mut impl Write,
    name: &str,
    status: &str,
    comma: bool,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, name)?;
    write!(output, ": {{ \"status\": ")?;
    write_json_string(output, status)?;
    writeln!(output, " }}{}", if comma { "," } else { "" })
}

fn write_observed_bool(
    output: &mut impl Write,
    name: &str,
    value: Option<bool>,
    comma: bool,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, name)?;
    match value {
        Some(value) => write!(
            output,
            ": {{ \"status\": \"observed\", \"value\": {value} }}"
        )?,
        None => write!(output, ": {{ \"status\": \"not-enumerated\" }}")?,
    }
    writeln!(output, "{}", if comma { "," } else { "" })
}

fn write_observed_u8(
    output: &mut impl Write,
    name: &str,
    value: Option<u8>,
    comma: bool,
) -> fmt::Result {
    write_observed_u64(output, name, value.map(u64::from), comma)
}

fn write_observed_u32(
    output: &mut impl Write,
    name: &str,
    value: Option<u32>,
    comma: bool,
) -> fmt::Result {
    write_observed_u64(output, name, value.map(u64::from), comma)
}

fn write_observed_u64(
    output: &mut impl Write,
    name: &str,
    value: Option<u64>,
    comma: bool,
) -> fmt::Result {
    write!(output, "      ")?;
    write_json_string(output, name)?;
    match value {
        Some(value) => write!(
            output,
            ": {{ \"status\": \"observed\", \"value\": {value} }}"
        )?,
        None => write!(output, ": {{ \"status\": \"not-enumerated\" }}")?,
    }
    writeln!(output, "{}", if comma { "," } else { "" })
}

fn write_json_string(output: &mut impl Write, value: &str) -> fmt::Result {
    write!(output, "\"")?;
    for character in value.chars() {
        match character {
            '"' => write!(output, "\\\"")?,
            '\\' => write!(output, "\\\\")?,
            '\u{08}' => write!(output, "\\b")?,
            '\u{0c}' => write!(output, "\\f")?,
            '\n' => write!(output, "\\n")?,
            '\r' => write!(output, "\\r")?,
            '\t' => write!(output, "\\t")?,
            control if control <= '\u{1f}' => write!(output, "\\u{:04x}", control as u32)?,
            other => write!(output, "{other}")?,
        }
    }
    write!(output, "\"")
}

fn write_trimmed_ascii_string(output: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0 && *byte != b' ')
        .map_or(0, |index| index + 1);
    write!(output, "\"")?;
    for byte in &bytes[..end] {
        match byte {
            b'"' => write!(output, "\\\"")?,
            b'\\' => write!(output, "\\\\")?,
            0x20..=0x7e => write!(output, "{}", char::from(*byte))?,
            _ => write!(output, "\\u{:04x}", u32::from(*byte))?,
        }
    }
    write!(output, "\"")
}

fn write_ascii(output: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(output, "{}", char::from(*byte))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::{CpuidSource, collect_cpuid};
    use alloc::collections::BTreeSet;

    struct SampleCpu;

    struct NonAmdCpu;

    static RSDP_RAW: [u8; 36] = [0; 36];
    static RSDT_RAW: [u8; 40] = [0; 40];
    static XSDT_RAW: [u8; 44] = [0; 44];
    static HEADER_RAW: [u8; 36] = [0; 36];
    static MADT_RAW: [u8; 52] = [0; 52];
    static MCFG_RAW: [u8; 60] = [0; 60];
    static IVRS_RAW: [u8; 76] = [0; 76];
    static FADT_RAW: [u8; 148] = [0; 148];
    static MADT_ENTRY_RAW: [u8; 8] = [0; 8];
    static IVHD_RAW: [u8; 28] = [0; 28];
    static IVHD_DEVICE_RAW: [u8; 4] = [0; 4];
    static ROOT_ENTRIES: [u64; 1] = [0x3000];
    static ENABLED_IDS: [u64; 1] = [7];
    static NO_IDS: [u64; 0] = [];
    static MCFG_ALLOCATIONS: [McfgAllocationEvidence; 1] = [McfgAllocationEvidence {
        offset: 44,
        base_address: 0xe000_0000,
        segment_group: 0,
        start_bus: 0,
        end_bus: 0xff,
        reserved: [0; 4],
        window_end_exclusive: 0xf000_0000,
    }];
    static MADT_ENTRIES: [MadtEntryEvidence<'static>; 1] =
        [MadtEntryEvidence::ProcessorLocalApic {
            offset: 44,
            raw: &MADT_ENTRY_RAW,
            acpi_processor_uid: 0,
            apic_id: 7,
            flags: 1,
        }];
    static IVHD_DEVICE_ENTRIES: [IvhdDeviceEntryEvidence<'static>; 1] = [IvhdDeviceEntryEvidence {
        offset: 72,
        entry_type: 0,
        raw: &IVHD_DEVICE_RAW,
        uid_length: None,
    }];
    static IVRS_BLOCKS: [IvrsBlockEvidence<'static>; 1] = [IvrsBlockEvidence::Ivhd(IvhdEvidence {
        offset: 48,
        entry_type: 0x10,
        flags: 0,
        raw: &IVHD_RAW,
        header_length: 24,
        device_id: 0x0002,
        capability_offset: 0x40,
        iommu_base_address: 0xf760_0000,
        pci_segment_group: 0,
        iommu_info: 0,
        feature_info: 0,
        extended_feature_image: None,
        extended_feature_image_2: None,
        device_entries: &IVHD_DEVICE_ENTRIES,
    })];
    static IOMMU_IVHD_INPUTS: [IvhdInput; 1] = [IvhdInput {
        source_index: 0,
        source: crate::iommu::locator::IvhdSource::Type10,
        segment_group: 0,
        device_id: 0x0002,
        capability_offset: 0x40,
        mmio_base: 0xf760_0000,
        efr_image: None,
        efr2_image: None,
    }];
    static IOMMU_MCFG_INPUTS: [McfgInput; 1] = [McfgInput {
        source_index: 0,
        base_address: 0xe000_0000,
        segment_group: 0,
        start_bus: 0,
        end_bus: 0xff,
    }];
    static IOMMU_CAPABILITY_CHAIN: [CapabilityLink; 1] = [CapabilityLink {
        offset: 0x40,
        capability_id: 0x0f,
        next: 0,
    }];
    static MEMORY_DESCRIPTORS: [MemoryDescriptorRecord; 2] = [
        MemoryDescriptorRecord {
            memory_type: 11,
            physical_start: 0xe000_0000,
            virtual_start: 0,
            page_count: 65_536,
            attributes: 1,
        },
        MemoryDescriptorRecord {
            memory_type: 11,
            physical_start: 0xf760_0000,
            virtual_start: 0,
            page_count: 128,
            attributes: 1,
        },
    ];
    static DIRECTORY: [AcpiDirectoryRecord<'static>; 1] = [AcpiDirectoryRecord {
        address: 0x3000,
        referenced_by: AcpiRootReferences::RsdtAndXsdt,
        header_raw: RawBytesEvidence { bytes: &HEADER_RAW },
        signature: *b"APIC",
        declared_length: 48,
        revision: 5,
    }];

    fn sample_header(signature: [u8; 4], length: u32) -> AcpiSdtHeaderEvidence {
        AcpiSdtHeaderEvidence {
            signature,
            length,
            revision: 1,
            checksum: 0,
            oem_id: *b"AMD   ",
            oem_table_id: *b"TEST    ",
            oem_revision: 1,
            creator_id: *b"TEST",
            creator_revision: 1,
        }
    }

    fn sample_acpi() -> AcpiEvidence<'static> {
        AcpiEvidence {
            selection_address: 0x1000,
            rsdp: RsdpEvidence {
                address: 0x1000,
                raw: RawBytesEvidence { bytes: &RSDP_RAW },
                checksum: 0,
                oem_id: *b"AMD   ",
                revision: 2,
                length: 36,
                rsdt_address: 0x2000,
                xsdt_address: 0x2100,
                extended_checksum: 0,
                reserved: [0; 3],
            },
            rsdt: AcpiRootEvidence {
                address: 0x2000,
                raw: RawBytesEvidence { bytes: &RSDT_RAW },
                header: sample_header(*b"RSDT", 40),
                entries: &ROOT_ENTRIES,
            },
            xsdt: AcpiRootEvidence {
                address: 0x2100,
                raw: RawBytesEvidence { bytes: &XSDT_RAW },
                header: sample_header(*b"XSDT", 44),
                entries: &ROOT_ENTRIES,
            },
            directory: &DIRECTORY,
            tables: AcpiTablesEvidence {
                madt: AcpiTableEvidence {
                    address: 0x3000,
                    referenced_by: AcpiRootReferences::RsdtAndXsdt,
                    raw: RawBytesEvidence { bytes: &MADT_RAW },
                    header: sample_header(*b"APIC", 52),
                    body: MadtBodyEvidence {
                        local_apic_address: 0xfee0_0000,
                        flags: 1,
                        entries: &MADT_ENTRIES,
                    },
                },
                mcfg: AcpiTableEvidence {
                    address: 0x3100,
                    referenced_by: AcpiRootReferences::Xsdt,
                    raw: RawBytesEvidence { bytes: &MCFG_RAW },
                    header: sample_header(*b"MCFG", 60),
                    body: McfgBodyEvidence {
                        reserved: [0; 8],
                        allocations: &MCFG_ALLOCATIONS,
                    },
                },
                ivrs: AcpiTableEvidence {
                    address: 0x3200,
                    referenced_by: AcpiRootReferences::Xsdt,
                    raw: RawBytesEvidence { bytes: &IVRS_RAW },
                    header: sample_header(*b"IVRS", 76),
                    body: IvrsBodyEvidence {
                        iv_info: 0,
                        reserved: [0; 8],
                        blocks: &IVRS_BLOCKS,
                    },
                },
                fadt: AcpiTableEvidence {
                    address: 0x3300,
                    referenced_by: AcpiRootReferences::Xsdt,
                    raw: RawBytesEvidence { bytes: &FADT_RAW },
                    header: sample_header(*b"FACP", 148),
                    body: FadtBodyEvidence {
                        firmware_ctrl_32: 0,
                        dsdt_32: 0,
                        preferred_pm_profile: 1,
                        sci_interrupt: 9,
                        iapc_boot_arch: 0,
                        flags: 0,
                        minor_version: 5,
                        x_firmware_ctrl: 0,
                        x_dsdt: 0,
                    },
                },
            },
            madt_mp_cross_check: MadtMpCrossCheckEvidence {
                mp_enabled_ids: &ENABLED_IDS,
                madt_enabled_ids: &ENABLED_IDS,
                missing_from_madt: &NO_IDS,
                missing_from_mp: &NO_IDS,
                consistent: true,
            },
        }
    }

    fn sample_iommu_disabled() -> AmdIommuLiveEvidence<'static> {
        let unit = derive_unique_unit(&IOMMU_IVHD_INPUTS).unwrap();
        let mcfg_witness = derive_unique_mcfg_witness(&unit, &IOMMU_MCFG_INPUTS).unwrap();
        let identity_raw = PciIdentityRaw {
            vendor_device: 0x1234_1022,
            command_status: 1 << 20,
            class_revision: 0x0806_0001,
            header_type: 0,
            first_capability_pointer: 0x40,
        };
        let capability_raw = PciCapabilityRaw {
            header: 0x180b_000f,
            base_low: 0xf760_0000,
            base_high: 0,
            range: 0,
            miscellaneous_0: 48 << 8,
            miscellaneous_1: Some(0),
        };
        AmdIommuLiveEvidence {
            access: IommuAccessEvidence {
                root_bridge_handle_count: 1,
                matching_segment_handle_count: 1,
                full_match_handle_count: 1,
                pci_read_operations: 18,
                pci_read_bytes: 64,
                mmio_read_operations: 0,
                mmio_read_bytes: 0,
                pci_write_operations: 0,
                mmio_write_operations: 0,
                direct_ecam_access: false,
                direct_mmio_access: false,
                cf8_cfc_access: false,
                configured_pointer_dereferences: 0,
            },
            locator: IommuLocatorEvidence {
                ivhd_sources: &IOMMU_IVHD_INPUTS,
                unit,
                mcfg_allocations: &IOMMU_MCFG_INPUTS,
                mcfg_witness,
                ecam_memory_binding: MemoryDescriptorBindingEvidence {
                    descriptor_index: 0,
                    descriptor: MEMORY_DESCRIPTORS[0],
                    requested_start: 0xe000_2000,
                    requested_end_exclusive: 0xe000_3000,
                },
            },
            pci: IommuPciEvidence {
                selected_root_bridge_handle_index: 0,
                segment_group: 0,
                bdf: unit.bdf,
                identity_raw,
                identity_decoded: decode_and_validate_identity(identity_raw).unwrap(),
                capability_chain: &IOMMU_CAPABILITY_CHAIN,
                capability_first_raw: capability_raw,
                capability_second_raw: capability_raw,
                capability_stable: true,
                capability_decoded: decode_and_validate_capability(capability_raw, unit.mmio_base)
                    .unwrap(),
            },
            mmio: AmdIommuMmioEvidence::Disabled,
            ownership: IommuOwnershipEvidence {
                assessed: false,
                claim: false,
                requester_dma_isolation_claim: false,
                interrupt_remapping_claim: false,
                pci_isolation_claim: false,
            },
        }
    }

    fn sample_inventory() -> crate::msr::SystemRegisterInventory {
        crate::msr::SystemRegisterInventory {
            mtrr_cap: 0x508,
            mtrr_def_type: 0xc06,
            pat: 0x0007_0406_0007_0406,
            variable_mtrr_pairs: [
                crate::msr::VariableMtrrPair {
                    base: 6,
                    mask: 0x0000_ffff_f800_0800,
                },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
                crate::msr::VariableMtrrPair { base: 0, mask: 0 },
            ],
            variable_mtrr_pair_count: 8,
            fixed_mtrr: [0x0606_0606_0606_0606; crate::msr::FIXED_MTRR_COUNT],
            fixed_mtrr_observed: true,
            sys_cfg: (1 << 18) | (1 << 20) | (1 << 21),
            hwcr: 1,
            top_mem: 0x0000_0000_c000_0000,
            tom2: 0x0000_0001_4000_0000,
            smm_base: 0x0003_0000,
            smm_addr: 0x0000_0000_8000_0000,
            smm_mask: 0x0000_0000_e000_0003,
            iorr_base: [0x0000_000f_e000_0018, 0],
            iorr_mask: [0x0000_000f_e000_0800, 0],
            iorr_not_attempted_reason: None,
        }
    }

    fn sample_system_registers() -> SystemRegistersSection<'static> {
        let inventory = sample_inventory();
        let operations = crate::msr::expected_read_operations(&inventory);
        SystemRegistersSection {
            bsp: crate::msr::SystemRegistersEvidence::Observed(inventory),
            access: MsrAccessEvidence {
                read_operations: operations,
                read_bytes: operations * 8,
                write_operations: 0,
            },
        }
    }

    fn sample_mmio_common<'a>(
        offsets: &'a [u16],
        feature_dependent_offsets: &'a [u16],
        live: ExtendedFeatureImages,
    ) -> IommuMmioCommonEvidence<'a> {
        let snapshot = StableMmioSnapshot {
            device_table_base: 0,
            command_buffer_base: 0,
            event_log_base: 0,
            control: 0,
            exclusion_or_completion_base: 0,
            exclusion_or_completion_limit: 0,
            extended_feature: Some(live.efr),
            extended_feature_2: Some(live.efr2),
            device_table_segments: [None; 7],
        };
        IommuMmioCommonEvidence {
            aperture_length_bytes: 16 * 1024,
            aperture_memory_binding: MemoryDescriptorBindingEvidence {
                descriptor_index: 1,
                descriptor: MEMORY_DESCRIPTORS[1],
                requested_start: 0xf760_0000,
                requested_end_exclusive: 0xf760_4000,
            },
            stable_offsets: offsets,
            feature_dependent_offsets,
            first_snapshot: snapshot,
            second_snapshot: snapshot,
            stable: true,
            status_offset: STATUS_OFFSET,
            status_raw: 0,
        }
    }

    impl CpuidSource for SampleCpu {
        fn cpuid(&mut self, leaf: u32, _subleaf: u32) -> CpuidRegisters {
            match leaf {
                0 => CpuidRegisters {
                    eax: 7,
                    ebx: u32::from_le_bytes(*b"Auth"),
                    edx: u32::from_le_bytes(*b"enti"),
                    ecx: u32::from_le_bytes(*b"cAMD"),
                },
                1 => CpuidRegisters {
                    eax: 0x00a0_0f12,
                    ebx: 7 << 24,
                    ..Default::default()
                },
                0x8000_0000 => CpuidRegisters {
                    eax: 0x8000_001f,
                    ..Default::default()
                },
                0x8000_0001 => CpuidRegisters {
                    ecx: (1 << 2) | (1 << 22),
                    ..Default::default()
                },
                0x8000_0008 => CpuidRegisters {
                    eax: 52,
                    ..Default::default()
                },
                0x8000_000a => CpuidRegisters {
                    eax: 1,
                    ebx: 32_768,
                    edx: 0x8000_00e9,
                    ..Default::default()
                },
                0x8000_001e => CpuidRegisters {
                    eax: 7,
                    ebx: 0x0000_0107,
                    ecx: 7,
                    edx: 0,
                },
                0x8000_001f => CpuidRegisters {
                    eax: 3,
                    ebx: 47 | (5 << 6),
                    ..Default::default()
                },
                _ => CpuidRegisters::default(),
            }
        }
    }

    impl CpuidSource for NonAmdCpu {
        fn cpuid(&mut self, leaf: u32, _subleaf: u32) -> CpuidRegisters {
            match leaf {
                0 => CpuidRegisters {
                    eax: 7,
                    ebx: u32::from_le_bytes(*b"Genu"),
                    edx: u32::from_le_bytes(*b"ineI"),
                    ecx: u32::from_le_bytes(*b"ntel"),
                },
                0x8000_0000 => CpuidRegisters {
                    eax: 0x8000_001f,
                    ..Default::default()
                },
                0x8000_0001 => CpuidRegisters {
                    ecx: 1 << 2,
                    ..Default::default()
                },
                0x8000_000a | 0x8000_001f => CpuidRegisters {
                    eax: u32::MAX,
                    ebx: u32::MAX,
                    ecx: u32::MAX,
                    edx: u32::MAX,
                },
                _ => CpuidRegisters::default(),
            }
        }
    }

    #[test]
    fn non_amd_reserved_bits_are_not_labeled_as_amd_capabilities() {
        let cpu = collect_cpuid(&mut NonAmdCpu);
        let mut document = String::from("{");
        write_cpu(&mut document, &cpu, "test-scope").unwrap();
        document.push('}');
        let value: serde_json::Value = serde_json::from_str(&document).unwrap();

        for field in [
            "svm_capable",
            "svm_revision",
            "asid_count",
            "nested_paging_capable",
            "svm_lock_capable",
            "nrip_save_capable",
            "vmcb_clean_bits_capable",
            "flush_by_asid_capable",
            "decode_assists_capable",
            "sme_capable",
            "sev_capable",
            "c_bit_position",
            "physical_address_reduction",
        ] {
            assert_eq!(
                value["cpu"]["decoded"][field]["status"],
                "not-applicable-vendor"
            );
        }
    }

    #[test]
    fn binding_parser_is_exact_and_case_sensitive() {
        let valid = b"svmvisor-m0b-target-v1\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
        let parsed = parse_profile_binding(valid).unwrap();
        assert_eq!(
            &parsed,
            &valid[PROFILE_BINDING_PREFIX.len()..valid.len() - 1]
        );

        let mut uppercase = valid.to_vec();
        uppercase[PROFILE_BINDING_PREFIX.len() + 10] = b'A';
        assert_eq!(
            parse_profile_binding(&uppercase),
            Err(BindingError::NonLowercaseHex)
        );
        assert_eq!(
            parse_profile_binding(&valid[..valid.len() - 1]),
            Err(BindingError::WrongLength)
        );
    }

    #[test]
    fn sink_authorization_fails_closed() {
        let authorized = SinkRecord {
            media_id: 7,
            removable_media: true,
            media_present: true,
            logical_partition: true,
            read_only: false,
            block_size: 512,
            last_block: 4095,
        };
        assert_eq!(authorized.authorize(), Ok(()));

        let cases = [
            (
                SinkRecord {
                    removable_media: false,
                    ..authorized
                },
                SinkAuthorizationError::NotRemovable,
            ),
            (
                SinkRecord {
                    media_present: false,
                    ..authorized
                },
                SinkAuthorizationError::MediaAbsent,
            ),
            (
                SinkRecord {
                    read_only: true,
                    ..authorized
                },
                SinkAuthorizationError::ReadOnly,
            ),
            (
                SinkRecord {
                    block_size: 0,
                    ..authorized
                },
                SinkAuthorizationError::InvalidBlockSize,
            ),
        ];
        for (sink, expected) in cases {
            assert_eq!(sink.authorize(), Err(expected));
        }
    }

    #[test]
    fn rendered_json_is_valid_and_keeps_unknowns_explicit() {
        let mut source = SampleCpu;
        let cpu = collect_cpuid(&mut source);
        let digest = *b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let processors = [ProcessorRecord {
            processor_number: 0,
            processor_id: 7,
            is_bsp: true,
            enabled: true,
            healthy: true,
            package: 0,
            core: 0,
            thread: 0,
        }];
        let observations = [crate::processor::ProcessorObservation {
            processor_number: 0,
            processor_id: 7,
            who_am_i_processor_number: 0,
            is_bsp: true,
            dispatch: ProcessorDispatch::BspDirect,
            cpu,
            vm_cr: VmCrEvidence::Observed(0x18),
            system_registers: crate::msr::SystemRegistersEvidence::Observed(sample_inventory()),
        }];
        let descriptors = MEMORY_DESCRIPTORS;
        let tables = [ConfigTableRecord {
            kind: "acpi2-rsdp",
            guid: "8868e871-e4f1-11d3-bc22-0080c73c8881",
            address: 0x1000,
        }];
        let evidence = Evidence {
            target_profile_manifest_sha256: &digest,
            output_file: "\\svmvisor-m0b-20260806T210000-000000000.json",
            collected_at: TimestampRecord {
                year: 2026,
                month: 8,
                day: 6,
                hour: 21,
                minute: 0,
                second: 0,
                nanosecond: 0,
                timezone_minutes: None,
            },
            sink: SinkRecord {
                media_id: 7,
                removable_media: true,
                media_present: true,
                logical_partition: true,
                read_only: false,
                block_size: 512,
                last_block: 4095,
            },
            firmware_vendor: "firmware \"vendor\"",
            firmware_revision: 1,
            uefi_revision_major: 2,
            uefi_revision_minor: 100,
            config_tables: &tables,
            cpu: &cpu,
            vm_cr: VmCrEvidence::Observed(0x18),
            mp_services: MpServicesEvidence::Observed {
                total: 1,
                enabled: 1,
                processors: &processors,
            },
            processor_consistency: ProcessorConsistencyEvidence {
                bsp_processor_number: 0,
                timeout_microseconds_per_ap: 0,
                observations: &observations,
            },
            memory_map: MemoryMapEvidence::Observed {
                descriptor_size: 48,
                descriptor_version: 1,
                descriptors: &descriptors,
            },
            acpi: sample_acpi(),
            amd_iommu_live: sample_iommu_disabled(),
            system_registers: sample_system_registers(),
        };

        let json = render_json(&evidence).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["evidence_kind"], "uefi-record-only-inventory-slice");
        assert_eq!(value["qualification_status"], "blocked");
        assert_eq!(value["launch_authorized"], false);
        assert_eq!(value["sink"]["media_id"], "0x00000007");
        assert_eq!(value["vm_cr"]["svmdis"], true);
        assert_eq!(value["collected_at"]["timezone"]["status"], "unspecified");
        assert_eq!(value["cpu"]["raw_leaves"].as_array().unwrap().len(), 12);
        assert_eq!(value["schema_version"], 6);
        assert_eq!(value["collector"]["version"], "0.6.0");
        assert_eq!(value["amd_iommu_ownership_claim"], false);
        assert_eq!(value["pci_isolation_claim"], false);
        assert_eq!(
            value["collector"]["slice"],
            "read-only-per-processor-system-register-inventory"
        );
        assert_eq!(
            value["system_registers"]["policy"]["comparison"],
            "exact-raw-and-observation-status-except-thread-scoped-smm-base"
        );
        assert_eq!(
            value["processor_consistency"]["comparison_policy"]["system_registers_comparison"],
            "exact-raw-and-observation-status-except-thread-scoped-smm-base"
        );
        assert_eq!(value["processor_consistency"]["status"], "observed");
        assert_eq!(
            value["processor_consistency"]["observations"][0]["dispatch"],
            "bsp-direct"
        );
        assert_eq!(
            value["processor_consistency"]["observations"][0]["who_am_i"]["processor_number"],
            0
        );
        assert_eq!(
            value["processor_consistency"]["dispatch"]["pre_dispatch_vm_cr_preservation"],
            "not-proven"
        );
        assert_eq!(
            value["processor_consistency"]["observations"][0]["identity_matches_mp"],
            true
        );
        assert_eq!(value["processor_consistency"]["consistent"], true);
        assert_eq!(
            value["processor_consistency"]["observations"][0]["system_registers"]["status"],
            "observed"
        );
        assert_eq!(
            value["processor_consistency"]["observations"][0]["system_registers_matches_bsp"],
            true
        );
        assert_eq!(
            value["processor_consistency"]["system_registers_consistent"],
            true
        );
        assert_eq!(value["system_registers"]["bsp"]["hwcr"]["smm_lock"], true);
        assert_eq!(
            value["system_registers"]["bsp"]["mtrr_cap"]["vcnt"],
            8
        );
        assert_eq!(
            value["system_registers"]["bsp"]["smm_mask"]["tseg_mask"],
            "0x00000000e0000000"
        );
        assert_eq!(
            value["system_registers"]["bsp"]["iorr"]["ranges"][0]["valid"],
            true
        );
        assert_eq!(
            value["system_registers"]["bsp"]["sys_cfg"]["encryption_state_claim"],
            false
        );
        assert_eq!(
            value["system_registers"]["bsp"]["smm_immutability_claim"],
            false
        );
        assert_eq!(value["system_registers"]["access"]["msr_read_operations"], 42);
        assert_eq!(value["system_registers"]["access"]["msr_read_bytes"], 336);
        assert_eq!(
            value["system_registers"]["access"]["msr_write_operations"],
            0
        );
        assert_eq!(
            value["system_registers"]["policy"]["ppr"]["sha256"],
            AMD_PPR_SHA256
        );
        assert!(
            !value["uncollected_blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|blocker| blocker == "cross-processor-cpuid-and-vm-cr-consistency")
        );
        assert!(
            value["uncollected_blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|blocker| {
                    blocker == "mp-services-ap-dispatch-pre-measurement-control-state-preservation"
                })
        );
        let blockers = value["uncollected_blockers"].as_array().unwrap();
        let expected_blockers = [
            "amd-iommu-dte-requester-ownership-and-pci-isolation",
            "inherited-memory-encryption-state",
            "secure-boot-databases-option-rom-policy-and-tcg-log",
            "boot-driver-sysprep-recovery-and-hotkey-namespace",
            "ready-to-boot-after-ready-to-boot-exit-boot-services-order",
            "direct-watchdog-and-durable-attempt-lease",
            "mp-services-ap-dispatch-pre-measurement-control-state-preservation",
        ];
        assert_eq!(blockers.len(), expected_blockers.len());
        for (observed, expected) in blockers.iter().zip(expected_blockers) {
            assert_eq!(observed, expected);
        }
        assert_eq!(value["acpi"]["specification"]["publication"], "48882");
        assert_eq!(value["acpi"]["specification"]["revision"], "3.11");
        assert_eq!(
            value["acpi"]["rsdp"]["raw"]["bytes"],
            "00".repeat(RSDP_RAW.len())
        );
        assert_eq!(
            value["acpi"]["rsdp"]["raw"]["sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            value["acpi"]["tables"]["ivrs"]["body"]["blocks"][0]["iommu_base_address"],
            "0x00000000f7600000"
        );
        assert_eq!(
            value["acpi"]["ivrs_claims"]["runtime_ownership_claim"],
            false
        );
        assert_eq!(
            value["amd_iommu_live"]["status"],
            "pci-observed-mmio-disabled"
        );
        assert_eq!(value["amd_iommu_live"]["mmio"]["read_operations"], 0);
    }

    #[test]
    fn iommu_mmio_renderer_preserves_all_three_status_shapes() {
        let mut disabled = String::from("{\n");
        write_iommu_mmio(&mut disabled, AmdIommuMmioEvidence::Disabled, 0).unwrap();
        disabled.push_str("\n}");
        let disabled: serde_json::Value = serde_json::from_str(&disabled).unwrap();
        assert_eq!(disabled["mmio"]["status"], "not-read-capability-disabled");
        assert!(disabled["mmio"].get("first_snapshot").is_none());

        let offsets = [
            0x0000, 0x0008, 0x0010, 0x0018, 0x0020, 0x0028, 0x0030, 0x01a0,
        ];
        let expected = ExtendedFeatureImages { efr: 1, efr2: 2 };
        let observed = ExtendedFeatureImages { efr: 3, efr2: 4 };
        let common = sample_mmio_common(&offsets, &[], observed);
        let mut conflict = String::from("{\n");
        write_iommu_mmio(
            &mut conflict,
            AmdIommuMmioEvidence::EfrConflict {
                common,
                expected,
                live: observed,
            },
            17,
        )
        .unwrap();
        conflict.push_str("\n}");
        let conflict: serde_json::Value = serde_json::from_str(&conflict).unwrap();
        assert_eq!(conflict["mmio"]["status"], "efr-conflict");
        assert_eq!(conflict["mmio"]["extended_feature_match"], false);
        assert!(conflict["mmio"].get("decoded_state").is_none());
        assert_eq!(
            conflict["mmio"]["feature_dependent_offsets"],
            serde_json::json!([])
        );

        let mut observed_json = String::from("{\n");
        write_iommu_mmio(
            &mut observed_json,
            AmdIommuMmioEvidence::Observed {
                common,
                features: ExtendedFeatureComparison::Match {
                    live: observed,
                    performance_counters_supported: false,
                    device_table_segments_supported: 0,
                },
                configured_ranges: &[],
                decoded: decode_live_state(0, 0),
            },
            17,
        )
        .unwrap();
        observed_json.push_str("\n}");
        let observed_json: serde_json::Value = serde_json::from_str(&observed_json).unwrap();
        assert_eq!(observed_json["mmio"]["status"], "observed");
        assert_eq!(observed_json["mmio"]["extended_feature_match"], true);
        assert_eq!(
            observed_json["mmio"]["extended_features"]["status"],
            "match"
        );
        assert!(observed_json["mmio"].get("decoded_state").is_some());
        assert!(observed_json["mmio"].get("configured_ranges").is_some());
    }

    #[test]
    fn configured_ranges_are_exactly_decoded_and_memory_map_bound() {
        let control = 1 | (1 << 2) | (1 << 12);
        let device_table_raw = 0x1000;
        let command_raw = (8_u64 << 56) | 0x2000;
        let event_raw = (8_u64 << 56) | 0x3000;
        let snapshot = StableMmioSnapshot {
            device_table_base: device_table_raw,
            command_buffer_base: command_raw,
            event_log_base: event_raw,
            control,
            exclusion_or_completion_base: 0,
            exclusion_or_completion_limit: 0,
            extended_feature: None,
            extended_feature_2: None,
            device_table_segments: [None; 7],
        };
        let plan = build_mmio_read_plan(control, ExtendedFeatureComparison::NotSupported).unwrap();
        let descriptor = MemoryDescriptorRecord {
            memory_type: 7,
            physical_start: 0x1000,
            virtual_start: 0,
            page_count: 3,
            attributes: 0,
        };
        let descriptors = [descriptor];
        let memory_map = MemoryMapEvidence::Observed {
            descriptor_size: 48,
            descriptor_version: 1,
            descriptors: &descriptors,
        };
        let common = IommuMmioCommonEvidence {
            aperture_length_bytes: 16 * 1024,
            aperture_memory_binding: MemoryDescriptorBindingEvidence {
                descriptor_index: 1,
                descriptor: MEMORY_DESCRIPTORS[1],
                requested_start: 0xf760_0000,
                requested_end_exclusive: 0xf760_4000,
            },
            stable_offsets: &plan.stable_offsets,
            feature_dependent_offsets: &[],
            first_snapshot: snapshot,
            second_snapshot: snapshot,
            stable: true,
            status_offset: STATUS_OFFSET,
            status_raw: 0,
        };
        let configured = [
            IommuConfiguredRangeEvidence {
                source_offset: 0x0000,
                raw: device_table_raw,
                enabled: true,
                base: 0x1000,
                length: 4096,
                alignment: 4096,
                validated_range: Some(PhysicalRange {
                    start: 0x1000,
                    end_exclusive: 0x2000,
                }),
                memory_binding: Some(MemoryDescriptorBindingEvidence {
                    descriptor_index: 0,
                    descriptor,
                    requested_start: 0x1000,
                    requested_end_exclusive: 0x2000,
                }),
            },
            IommuConfiguredRangeEvidence {
                source_offset: 0x0008,
                raw: command_raw,
                enabled: true,
                base: 0x2000,
                length: 4096,
                alignment: 4096,
                validated_range: Some(PhysicalRange {
                    start: 0x2000,
                    end_exclusive: 0x3000,
                }),
                memory_binding: Some(MemoryDescriptorBindingEvidence {
                    descriptor_index: 0,
                    descriptor,
                    requested_start: 0x2000,
                    requested_end_exclusive: 0x3000,
                }),
            },
            IommuConfiguredRangeEvidence {
                source_offset: 0x0010,
                raw: event_raw,
                enabled: true,
                base: 0x3000,
                length: 4096,
                alignment: 4096,
                validated_range: Some(PhysicalRange {
                    start: 0x3000,
                    end_exclusive: 0x4000,
                }),
                memory_binding: Some(MemoryDescriptorBindingEvidence {
                    descriptor_index: 0,
                    descriptor,
                    requested_start: 0x3000,
                    requested_end_exclusive: 0x4000,
                }),
            },
        ];
        assert_eq!(
            validate_configured_ranges(&configured, common, &plan, 48, 52, memory_map),
            Ok(())
        );
        assert!(
            validate_configured_ranges(&configured[..2], common, &plan, 48, 52, memory_map)
                .is_err()
        );
        let mut forged = configured;
        forged[0].length = 8192;
        assert!(validate_configured_ranges(&forged, common, &plan, 48, 52, memory_map).is_err());
        let mut unbound = configured;
        unbound[1].memory_binding = None;
        assert!(validate_configured_ranges(&unbound, common, &plan, 48, 52, memory_map).is_err());
        let segmented_plan = MmioReadPlan {
            stable_offsets: plan.stable_offsets.clone(),
            status_offset: STATUS_OFFSET,
            active_device_table_segments: 2,
        };
        assert!(
            validate_configured_ranges(&configured, common, &segmented_plan, 48, 52, memory_map,)
                .is_err()
        );
        assert!(decode_configured_range(0x0008, 7_u64 << 56, control).is_err());

        let mut rendered = String::from("{\n");
        write_configured_ranges(&mut rendered, &configured).unwrap();
        rendered.push_str("\n}");
        let rendered: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            rendered["configured_ranges"][0]["memory_binding"]["descriptor_index"],
            0
        );
    }

    #[test]
    fn efr_conflict_uses_the_larger_aperture_requirement() {
        let none = ExtendedFeatureImages { efr: 0, efr2: 0 };
        let pc_sup = ExtendedFeatureImages {
            efr: 1 << 9,
            efr2: 0,
        };
        assert!(!conflict_requires_large_aperture(none, none));
        assert!(conflict_requires_large_aperture(pc_sup, none));
        assert!(conflict_requires_large_aperture(none, pc_sup));
    }

    #[test]
    fn vm_cr_bits_are_decoded_exactly() {
        let mut output = String::new();
        write_vm_cr(&mut output, VmCrEvidence::Observed(0b1_1111)).unwrap();
        let object_text = output.strip_prefix("  \"vm_cr\": ").unwrap();
        let value: serde_json::Value = serde_json::from_str(object_text).unwrap();
        for field in ["dpd", "r_init", "dis_a20m", "lock", "svmdis"] {
            assert_eq!(value[field], true);
        }
        assert_eq!(value["raw"], "0x000000000000001f");
    }

    #[test]
    fn json_string_escaping_is_not_lossy() {
        let mut output = String::new();
        write_json_string(&mut output, "quote=\" slash=\\ line=\n tab=\t").unwrap();
        let parsed: String = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed, "quote=\" slash=\\ line=\n tab=\t");
    }

    #[test]
    fn raw_envelope_fails_closed_at_rendered_json_limit() {
        let bytes = alloc::vec![0_u8; MAX_RENDERED_JSON_BYTES / 2];
        let mut output = BoundedJson::new();
        assert!(write_raw_envelope(&mut output, RawBytesEvidence { bytes: &bytes }).is_err());
    }

    #[test]
    fn raw_envelope_derives_length_digest_and_lowercase_hex() {
        let mut output = String::new();
        write_raw_envelope(
            &mut output,
            RawBytesEvidence {
                bytes: b"\x00\xab\xff",
            },
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["encoding"], "lowercase-hex");
        assert_eq!(value["length_bytes"], 3);
        assert_eq!(value["bytes"], "00abff");
        assert_eq!(
            value["sha256"],
            "de4be1eb1639e64d628aa53eb7e93074228bbe1eecfd0040aacdb83c9e5fed40"
        );
    }

    #[test]
    fn unobserved_mp_and_memory_map_are_errors_not_empty_successes() {
        let mut mp = String::new();
        write_mp_services(
            &mut mp,
            MpServicesEvidence::Unavailable {
                uefi_status: 0x8000_0000_0000_0003,
            },
        )
        .unwrap();
        assert!(mp.contains("\"status\": \"unavailable\""));

        let mut map = String::new();
        write_memory_map(
            &mut map,
            MemoryMapEvidence::Unavailable {
                uefi_status: 0x8000_0000_0000_0009,
            },
        )
        .unwrap();
        assert!(map.contains("not-final-exit-boot-services-map"));
        assert!(!map.contains("\"descriptors\""));
    }

    #[test]
    fn rendered_raw_cpuid_leaf_keys_are_unique() {
        let mut source = SampleCpu;
        let cpu = collect_cpuid(&mut source);
        let digest = *b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let processors = [ProcessorRecord {
            processor_number: 0,
            processor_id: 7,
            is_bsp: true,
            enabled: true,
            healthy: true,
            package: 0,
            core: 0,
            thread: 0,
        }];
        let vm_cr = VmCrEvidence::NotAttempted { reason: "test" };
        let system_registers = crate::msr::SystemRegistersEvidence::NotAttempted { reason: "test" };
        let observations = [crate::processor::ProcessorObservation {
            processor_number: 0,
            processor_id: 7,
            who_am_i_processor_number: 0,
            is_bsp: true,
            dispatch: ProcessorDispatch::BspDirect,
            cpu,
            vm_cr,
            system_registers,
        }];
        let config_tables = [ConfigTableRecord {
            kind: "acpi2-rsdp",
            guid: "8868e871-e4f1-11d3-bc22-0080c73c8881",
            address: 0x1000,
        }];
        let evidence = Evidence {
            target_profile_manifest_sha256: &digest,
            output_file: "\\svmvisor-m0b-test.json",
            collected_at: TimestampRecord {
                year: 2026,
                month: 8,
                day: 6,
                hour: 21,
                minute: 0,
                second: 0,
                nanosecond: 1,
                timezone_minutes: Some(0),
            },
            sink: SinkRecord {
                media_id: 7,
                removable_media: true,
                media_present: true,
                logical_partition: true,
                read_only: false,
                block_size: 512,
                last_block: 4095,
            },
            firmware_vendor: "test",
            firmware_revision: 1,
            uefi_revision_major: 2,
            uefi_revision_minor: 100,
            config_tables: &config_tables,
            cpu: &cpu,
            vm_cr,
            mp_services: MpServicesEvidence::Observed {
                total: 1,
                enabled: 1,
                processors: &processors,
            },
            processor_consistency: ProcessorConsistencyEvidence {
                bsp_processor_number: 0,
                timeout_microseconds_per_ap: 0,
                observations: &observations,
            },
            memory_map: MemoryMapEvidence::Observed {
                descriptor_size: 48,
                descriptor_version: 1,
                descriptors: &MEMORY_DESCRIPTORS,
            },
            acpi: sample_acpi(),
            amd_iommu_live: sample_iommu_disabled(),
            system_registers: SystemRegistersSection {
                bsp: system_registers,
                access: MsrAccessEvidence {
                    read_operations: 0,
                    read_bytes: 0,
                    write_operations: 0,
                },
            },
        };
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&evidence).unwrap()).unwrap();
        let leaves = value["cpu"]["raw_leaves"].as_array().unwrap();
        let keys = leaves
            .iter()
            .map(|leaf| {
                (
                    leaf["leaf"].as_str().unwrap(),
                    leaf["subleaf"].as_str().unwrap(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), leaves.len());
    }
}
