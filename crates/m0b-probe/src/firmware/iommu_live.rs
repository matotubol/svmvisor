//! Safe UEFI adapter for the record-only live AMD-IOMMU inventory slice.
//!
//! Hardware access in this module is limited to `Pci.Read` and `Memory.Read`
//! through one firmware-owned PCI Root Bridge I/O protocol.  ECAM addresses
//! are corroborating witnesses only, and configured physical pointers are
//! range-checked but never dereferenced.

extern crate alloc;

use alloc::vec::Vec;
use uefi::boot::{self, ScopedProtocol};
use uefi::prelude::Status;
use uefi::proto::pci::PciIoAddress;
use uefi::proto::pci::root_bridge::PciRootBridgeIo;

use svmvisor_m0b_probe::MemoryDescriptorRecord;
use svmvisor_m0b_probe::acpi::{AcpiLimits, IvrsEntry, WhitelistedTable, parse_whitelisted_table};
use svmvisor_m0b_probe::iommu::locator::{
    IommuUnit, IvhdInput, IvhdSource, McfgInput, McfgWitness, derive_unique_mcfg_witness,
    derive_unique_unit,
};
use svmvisor_m0b_probe::iommu::ranges::{
    ConfiguredRange, DescriptorWitness, MemoryDescriptor, PhysicalRange, checked_physical_range,
    ecam_descriptor_witness, iommu_aperture_witness, memory_descriptor_witness,
    validate_enabled_configured_range,
};
use svmvisor_m0b_probe::iommu::registers::{
    BASE_STABLE_MMIO_OFFSETS, CONTROL_OFFSET, CapabilityLink, DEVICE_TABLE_SEGMENT_1_OFFSET,
    DecodedLiveState, EXTENDED_FEATURE_2_OFFSET, EXTENDED_FEATURE_OFFSET,
    ExtendedFeatureComparison, ExtendedFeatureImages, MAX_CAPABILITY_LINKS, MmioReadPlan,
    PciCapability, PciCapabilityRaw, PciIdentity, PciIdentityRaw, RegisterError, STATUS_OFFSET,
    StableMmioSnapshot, build_mmio_read_plan, compare_extended_features,
    decode_and_validate_capability, decode_and_validate_identity, decode_live_state,
    require_stable_mmio_snapshot, require_stable_pci_capability, validate_capability_chain,
};

use super::{CapturedAcpi, ProbeError, find_captured_table, open_protocol_get};

const MAX_ROOT_BRIDGE_HANDLES: usize = 64;
const PCI_FUNCTION_BYTES: u64 = 4096;
const CONFIGURED_BASE_MASK: u64 = 0x000f_ffff_ffff_f000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LiveIommuAccess {
    pub(super) root_bridge_handle_count: usize,
    pub(super) matching_segment_handle_count: usize,
    pub(super) full_match_handle_count: usize,
    pub(super) pci_read_operations: usize,
    pub(super) pci_read_bytes: usize,
    pub(super) mmio_read_operations: usize,
    pub(super) mmio_read_bytes: usize,
    pub(super) pci_write_operations: usize,
    pub(super) mmio_write_operations: usize,
    pub(super) direct_ecam_access: bool,
    pub(super) direct_mmio_access: bool,
    pub(super) cf8_cfc_access: bool,
    pub(super) configured_pointer_dereferences: usize,
}

impl LiveIommuAccess {
    const fn new() -> Self {
        Self {
            root_bridge_handle_count: 0,
            matching_segment_handle_count: 0,
            full_match_handle_count: 0,
            pci_read_operations: 0,
            pci_read_bytes: 0,
            mmio_read_operations: 0,
            mmio_read_bytes: 0,
            pci_write_operations: 0,
            mmio_write_operations: 0,
            direct_ecam_access: false,
            direct_mmio_access: false,
            cf8_cfc_access: false,
            configured_pointer_dereferences: 0,
        }
    }

    fn count_pci(&mut self, bytes: usize) -> Result<(), ProbeError> {
        self.pci_read_operations = self
            .pci_read_operations
            .checked_add(1)
            .ok_or_else(|| compromised("the live-IOMMU PCI read counter overflowed"))?;
        self.pci_read_bytes = self
            .pci_read_bytes
            .checked_add(bytes)
            .ok_or_else(|| compromised("the live-IOMMU PCI byte counter overflowed"))?;
        Ok(())
    }

    fn count_mmio(&mut self) -> Result<(), ProbeError> {
        self.mmio_read_operations = self
            .mmio_read_operations
            .checked_add(1)
            .ok_or_else(|| compromised("the live-IOMMU MMIO read counter overflowed"))?;
        self.mmio_read_bytes = self
            .mmio_read_bytes
            .checked_add(8)
            .ok_or_else(|| compromised("the live-IOMMU MMIO byte counter overflowed"))?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ConfiguredRangeObservation {
    pub(super) source_offset: u16,
    pub(super) raw: u64,
    pub(super) enabled: bool,
    pub(super) base: u64,
    pub(super) length: u64,
    pub(super) alignment: u64,
    pub(super) validated_range: Option<PhysicalRange>,
    pub(super) memory_witness: Option<DescriptorWitness>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LiveMmioCommon {
    pub(super) aperture_witness: DescriptorWitness,
    pub(super) plan: MmioReadPlan,
    pub(super) feature_dependent_offsets: Vec<u16>,
    pub(super) first_snapshot: StableMmioSnapshot,
    pub(super) second_snapshot: StableMmioSnapshot,
    pub(super) status_raw: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LiveMmioCapture {
    Disabled,
    EfrConflict {
        common: LiveMmioCommon,
        expected: ExtendedFeatureImages,
        live: ExtendedFeatureImages,
    },
    Observed {
        common: LiveMmioCommon,
        features: ExtendedFeatureComparison,
        configured_ranges: Vec<ConfiguredRangeObservation>,
        decoded: DecodedLiveState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LiveAmdIommuCapture {
    pub(super) access: LiveIommuAccess,
    pub(super) ivhd_sources: Vec<IvhdInput>,
    pub(super) unit: IommuUnit,
    pub(super) mcfg_allocations: Vec<McfgInput>,
    pub(super) mcfg_witness: McfgWitness,
    pub(super) ecam_memory_witness: DescriptorWitness,
    pub(super) preflight_aperture_witness: DescriptorWitness,
    pub(super) selected_root_bridge_handle_index: usize,
    pub(super) selected_root_bridge_segment: u16,
    pub(super) identity_raw: PciIdentityRaw,
    pub(super) identity_decoded: PciIdentity,
    pub(super) capability_chain: Vec<CapabilityLink>,
    pub(super) capability_first_raw: PciCapabilityRaw,
    pub(super) capability_second_raw: PciCapabilityRaw,
    pub(super) capability_decoded: PciCapability,
    pub(super) mmio: LiveMmioCapture,
}

struct SelectedProbe {
    scope: ScopedProtocol<PciRootBridgeIo>,
    handle_index: usize,
    identity_raw: PciIdentityRaw,
    identity: PciIdentity,
    links: Vec<CapabilityLink>,
    capability_raw: PciCapabilityRaw,
    capability: PciCapability,
}

/// Collect one bounded, read-only live AMD-IOMMU observation.
pub(super) fn collect_live_amd_iommu(
    captured_acpi: &CapturedAcpi,
    memory_records: &[MemoryDescriptorRecord],
    cpu_physical_address_bits: u8,
) -> Result<LiveAmdIommuCapture, ProbeError> {
    let (ivhd_sources, unit, mcfg_allocations, mcfg_witness) =
        derive_same_run_locator(captured_acpi)?;
    let descriptors = copy_memory_descriptors(memory_records)?;

    let function_range = checked_physical_range(
        mcfg_witness.function_address,
        PCI_FUNCTION_BYTES,
        cpu_physical_address_bits,
    )
    .map_err(|_| compromised("the IVRS-derived ECAM function range is invalid"))?;
    if function_range.start < mcfg_witness.usable_range.start
        || function_range.end_exclusive > mcfg_witness.usable_range.end_exclusive
        || mcfg_witness.capability_address < function_range.start
        || mcfg_witness
            .capability_address
            .checked_add(0x18)
            .is_none_or(|end| end > function_range.end_exclusive)
    {
        return Err(compromised(
            "the IVRS-derived ECAM witnesses escape the selected MCFG allocation",
        ));
    }
    let ecam_memory_witness =
        ecam_descriptor_witness(&descriptors, function_range, cpu_physical_address_bits)
            .map_err(|_| compromised("the ECAM function lacks one readable MMIO descriptor"))?;

    let expected_pc_sup = unit.efr_image.is_some_and(|efr| efr & (1 << 9) != 0);
    let _ivrs_preflight_aperture_witness = iommu_aperture_witness(
        &descriptors,
        unit.mmio_base,
        expected_pc_sup,
        cpu_physical_address_bits,
    )
    .map_err(|_| compromised("the IVRS-derived IOMMU aperture failed memory-map preflight"))?;

    let mut access = LiveIommuAccess::new();
    let handles = boot::find_handles::<PciRootBridgeIo>().map_err(|error| {
        ProbeError::new(
            error.status(),
            "EFI PCI Root Bridge I/O protocol handles are unavailable",
        )
    })?;
    if handles.is_empty() || handles.len() > MAX_ROOT_BRIDGE_HANDLES {
        return Err(compromised(
            "the PCI Root Bridge I/O handle count is outside the reviewed bound",
        ));
    }
    for (index, handle) in handles.iter().enumerate() {
        if handles[..index].contains(handle) {
            return Err(compromised(
                "EFI returned a duplicate PCI Root Bridge I/O handle",
            ));
        }
    }
    access.root_bridge_handle_count = handles.len();

    let mut selected: Option<SelectedProbe> = None;
    for (handle_index, handle) in handles.iter().copied().enumerate() {
        let mut scope = open_protocol_get::<PciRootBridgeIo>(handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "a PCI Root Bridge I/O protocol could not be opened read-only",
            )
        })?;
        let root = scope
            .get_mut()
            .ok_or_else(|| compromised("a PCI Root Bridge I/O handle returned a null interface"))?;
        if root.segment_nr() != u32::from(unit.segment_group) {
            continue;
        }
        access.matching_segment_handle_count += 1;
        let identity_raw = read_identity(root, &unit, &mut access)?;
        let identity = match decode_and_validate_identity(identity_raw) {
            Ok(identity) => identity,
            Err(
                RegisterError::FunctionAbsent
                | RegisterError::WrongVendor(_)
                | RegisterError::MissingCapabilitiesList
                | RegisterError::WrongClass { .. }
                | RegisterError::UnsupportedHeaderLayout(_),
            ) => continue,
            Err(_) => {
                return Err(compromised(
                    "an AMD-IOMMU-class PCI function has malformed identity metadata",
                ));
            }
        };
        let links = read_and_validate_capability_chain(root, &unit, identity, &mut access)?;
        let capability_raw = read_capability(root, &unit, &mut access)?;
        let terminal_link = links.last().copied().ok_or_else(|| {
            compromised("the IVRS capability chain did not retain its terminal link")
        })?;
        let terminal_header =
            u32::from(terminal_link.capability_id) | (u32::from(terminal_link.next) << 8);
        if capability_raw.header & 0xffff != terminal_header {
            return Err(compromised(
                "the terminal PCI capability link does not match the IOMMU capability header",
            ));
        }
        let capability = decode_and_validate_capability(capability_raw, unit.mmio_base)
            .map_err(|_| compromised("the IVRS-selected IOMMU PCI capability is inconsistent"))?;
        access.full_match_handle_count += 1;
        if selected.is_none() {
            selected = Some(SelectedProbe {
                scope,
                handle_index,
                identity_raw,
                identity,
                links,
                capability_raw,
                capability,
            });
        }
    }
    if access.full_match_handle_count != 1 {
        return Err(compromised(
            "the IVRS-selected IOMMU did not have exactly one full root-bridge match",
        ));
    }
    let mut selected = selected
        .ok_or_else(|| compromised("the unique live IOMMU root-bridge match was not retained"))?;

    let effective_width =
        cpu_physical_address_bits.min(selected.capability.iommu_physical_address_width);
    let preflight_aperture_witness = iommu_aperture_witness(
        &descriptors,
        unit.mmio_base,
        expected_pc_sup,
        effective_width,
    )
    .map_err(|_| compromised("the IOMMU aperture exceeds the live PCI or CPU address width"))?;

    // This is the exact matching GET_PROTOCOL scope retained continuously
    // through every Memory.Read and the final outer PCI snapshot.
    let root = selected
        .scope
        .get_mut()
        .ok_or_else(|| compromised("the selected PCI Root Bridge I/O interface became null"))?;
    if root.segment_nr() != u32::from(unit.segment_group) {
        return Err(compromised(
            "the selected PCI root-bridge segment changed before live inspection",
        ));
    }

    let mmio = if selected.capability.mmio_enabled {
        collect_mmio(
            root,
            &unit,
            selected.capability,
            &descriptors,
            effective_width,
            cpu_physical_address_bits,
            &mut access,
        )?
    } else {
        LiveMmioCapture::Disabled
    };

    let capability_second_raw = read_capability(root, &unit, &mut access)?;
    let capability_second =
        decode_and_validate_capability(capability_second_raw, unit.mmio_base)
            .map_err(|_| compromised("the outer IOMMU PCI capability snapshot is inconsistent"))?;
    require_stable_pci_capability(selected.capability_raw, capability_second_raw)
        .map_err(|_| compromised("the IOMMU PCI capability changed during live inspection"))?;
    if capability_second != selected.capability {
        return Err(compromised(
            "the decoded IOMMU PCI capability changed during live inspection",
        ));
    }

    Ok(LiveAmdIommuCapture {
        access,
        ivhd_sources,
        unit,
        mcfg_allocations,
        mcfg_witness,
        ecam_memory_witness,
        preflight_aperture_witness,
        selected_root_bridge_handle_index: selected.handle_index,
        selected_root_bridge_segment: unit.segment_group,
        identity_raw: selected.identity_raw,
        identity_decoded: selected.identity,
        capability_chain: selected.links,
        capability_first_raw: selected.capability_raw,
        capability_second_raw,
        capability_decoded: selected.capability,
        mmio,
    })
}

fn derive_same_run_locator(
    captured: &CapturedAcpi,
) -> Result<(Vec<IvhdInput>, IommuUnit, Vec<McfgInput>, McfgWitness), ProbeError> {
    let limits = AcpiLimits::default();
    let ivrs_capture = find_captured_table(captured, *b"IVRS")?;
    let ivrs_raw = ivrs_capture
        .table_raw
        .as_deref()
        .ok_or_else(|| compromised("the validated IVRS raw bytes are unavailable"))?;
    let ivrs = match parse_whitelisted_table(ivrs_raw, &limits).map_err(|error| {
        ProbeError::with_acpi_detail(
            Status::COMPROMISED_DATA,
            "the live-IOMMU slice could not revalidate IVRS",
            "IVRS-live-IOMMU",
            error,
        )
    })? {
        WhitelistedTable::Ivrs(ivrs) => ivrs,
        _ => return Err(compromised("the captured IVRS changed table kind")),
    };
    let mut ivhd_sources = Vec::new();
    for (source_index, entry) in ivrs.entries.iter().enumerate() {
        if let IvrsEntry::Ivhd(ivhd) = entry {
            ivhd_sources.push(IvhdInput {
                source_index,
                source: IvhdSource::from_entry_type(ivhd.entry_type)
                    .map_err(|_| compromised("IVRS contains an unsupported IVHD type"))?,
                segment_group: ivhd.pci_segment_group,
                device_id: ivhd.device_id,
                capability_offset: ivhd.capability_offset,
                mmio_base: ivhd.iommu_base_address,
                efr_image: ivhd.extended_feature_image,
                efr2_image: ivhd.extended_feature_image_2,
            });
        }
    }
    let unit = derive_unique_unit(&ivhd_sources)
        .map_err(|_| compromised("IVRS does not derive exactly one consistent IOMMU unit"))?;

    let mcfg_capture = find_captured_table(captured, *b"MCFG")?;
    let mcfg_raw = mcfg_capture
        .table_raw
        .as_deref()
        .ok_or_else(|| compromised("the validated MCFG raw bytes are unavailable"))?;
    let mcfg = match parse_whitelisted_table(mcfg_raw, &limits).map_err(|error| {
        ProbeError::with_acpi_detail(
            Status::COMPROMISED_DATA,
            "the live-IOMMU slice could not revalidate MCFG",
            "MCFG-live-IOMMU",
            error,
        )
    })? {
        WhitelistedTable::Mcfg(mcfg) => mcfg,
        _ => return Err(compromised("the captured MCFG changed table kind")),
    };
    let mcfg_allocations = mcfg
        .allocations
        .iter()
        .enumerate()
        .map(|(source_index, allocation)| McfgInput {
            source_index,
            base_address: allocation.base_address,
            segment_group: allocation.segment_group,
            start_bus: allocation.start_bus,
            end_bus: allocation.end_bus,
        })
        .collect::<Vec<_>>();
    let witness = derive_unique_mcfg_witness(&unit, &mcfg_allocations)
        .map_err(|_| compromised("MCFG does not provide one exact IVRS ECAM witness"))?;
    Ok((ivhd_sources, unit, mcfg_allocations, witness))
}

fn copy_memory_descriptors(
    records: &[MemoryDescriptorRecord],
) -> Result<Vec<MemoryDescriptor>, ProbeError> {
    if records.len() > svmvisor_m0b_probe::iommu::ranges::MAX_MEMORY_DESCRIPTORS {
        return Err(compromised(
            "the memory descriptor count exceeds the live-IOMMU bound",
        ));
    }
    Ok(records
        .iter()
        .map(|record| MemoryDescriptor {
            memory_type: record.memory_type,
            physical_start: record.physical_start,
            page_count: record.page_count,
            attributes: record.attributes,
        })
        .collect())
}

fn read_identity(
    root: &mut PciRootBridgeIo,
    unit: &IommuUnit,
    access: &mut LiveIommuAccess,
) -> Result<PciIdentityRaw, ProbeError> {
    let address = pci_address(unit);
    Ok(PciIdentityRaw {
        vendor_device: pci_read::<u32>(root, address.with_register(0x00), access)?,
        command_status: pci_read::<u32>(root, address.with_register(0x04), access)?,
        class_revision: pci_read::<u32>(root, address.with_register(0x08), access)?,
        header_type: pci_read::<u8>(root, address.with_register(0x0e), access)?,
        first_capability_pointer: pci_read::<u8>(root, address.with_register(0x34), access)?,
    })
}

fn read_and_validate_capability_chain(
    root: &mut PciRootBridgeIo,
    unit: &IommuUnit,
    identity: PciIdentity,
    access: &mut LiveIommuAccess,
) -> Result<Vec<CapabilityLink>, ProbeError> {
    let target = u8::try_from(unit.capability_offset)
        .map_err(|_| compromised("the IVRS capability offset is not conventional PCI space"))?;
    let base = pci_address(unit);
    let mut links = Vec::new();
    let mut current = identity.first_capability_pointer;
    for _ in 0..MAX_CAPABILITY_LINKS {
        if !(0x40..=0xfc).contains(&current)
            || !current.is_multiple_of(4)
            || links
                .iter()
                .any(|link: &CapabilityLink| link.offset == current)
        {
            return Err(compromised(
                "the conventional PCI capability chain is invalid or cyclic",
            ));
        }
        let raw = pci_read::<u16>(root, base.with_register(current), access)?;
        let link = CapabilityLink {
            offset: current,
            capability_id: raw as u8,
            next: (raw >> 8) as u8,
        };
        links.push(link);
        if current == target {
            validate_capability_chain(
                identity.first_capability_pointer,
                unit.capability_offset,
                &links,
            )
            .map_err(|_| compromised("the IVRS capability is not in the exact PCI chain"))?;
            return Ok(links);
        }
        if link.next == 0 {
            break;
        }
        current = link.next;
    }
    Err(compromised(
        "the bounded PCI capability chain did not reach the IVRS capability",
    ))
}

fn read_capability(
    root: &mut PciRootBridgeIo,
    unit: &IommuUnit,
    access: &mut LiveIommuAccess,
) -> Result<PciCapabilityRaw, ProbeError> {
    let cap = u8::try_from(unit.capability_offset)
        .map_err(|_| compromised("the IVRS capability offset is outside PCI space"))?;
    let base = pci_address(unit);
    let header = pci_read::<u32>(root, base.with_register(cap), access)?;
    let miscellaneous_1 = if header & (1 << 28) != 0 {
        Some(pci_read::<u32>(
            root,
            base.with_register(cap + 0x14),
            access,
        )?)
    } else {
        None
    };
    Ok(PciCapabilityRaw {
        header,
        base_low: pci_read::<u32>(root, base.with_register(cap + 0x04), access)?,
        base_high: pci_read::<u32>(root, base.with_register(cap + 0x08), access)?,
        range: pci_read::<u32>(root, base.with_register(cap + 0x0c), access)?,
        miscellaneous_0: pci_read::<u32>(root, base.with_register(cap + 0x10), access)?,
        miscellaneous_1,
    })
}

fn collect_mmio(
    root: &mut PciRootBridgeIo,
    unit: &IommuUnit,
    capability: PciCapability,
    descriptors: &[MemoryDescriptor],
    effective_width: u8,
    cpu_width: u8,
    access: &mut LiveIommuAccess,
) -> Result<LiveMmioCapture, ProbeError> {
    let mut first = read_base_snapshot(root, unit.mmio_base, access)?;
    let live_images = if capability.extended_feature_register_supported {
        let efr = mmio_read(root, unit.mmio_base, EXTENDED_FEATURE_OFFSET, access)?;
        let efr2 = mmio_read(root, unit.mmio_base, EXTENDED_FEATURE_2_OFFSET, access)?;
        first.extended_feature = Some(efr);
        first.extended_feature_2 = Some(efr2);
        Some(ExtendedFeatureImages { efr, efr2 })
    } else {
        None
    };
    let expected_images = match (unit.efr_image, unit.efr2_image) {
        (Some(efr), Some(efr2)) => Some(ExtendedFeatureImages { efr, efr2 }),
        (None, None) => None,
        _ => return Err(compromised("IVRS has a partial extended-feature image")),
    };
    let features = compare_extended_features(
        capability.extended_feature_register_supported,
        expected_images,
        live_images,
    )
    .map_err(|_| compromised("PCI and IVRS extended-feature gates disagree"))?;

    // A conflict may only strengthen the range bound: witness the larger
    // aperture required by either the same-run IVRS or live EFR image.
    let required_pc_sup = expected_images.is_some_and(|images| images.efr & (1 << 9) != 0)
        || live_images.is_some_and(|images| images.efr & (1 << 9) != 0);
    let aperture_witness = iommu_aperture_witness(
        descriptors,
        unit.mmio_base,
        required_pc_sup,
        effective_width,
    )
    .map_err(|_| compromised("the live EFR-selected IOMMU aperture is not safely mapped"))?;

    let plan = match features {
        ExtendedFeatureComparison::Conflict { .. } => {
            let mut stable_offsets = BASE_STABLE_MMIO_OFFSETS.to_vec();
            stable_offsets.push(EXTENDED_FEATURE_OFFSET);
            stable_offsets.push(EXTENDED_FEATURE_2_OFFSET);
            MmioReadPlan {
                stable_offsets,
                status_offset: STATUS_OFFSET,
                active_device_table_segments: 1,
            }
        }
        _ => build_mmio_read_plan(first.control, features)
            .map_err(|_| compromised("the live IOMMU MMIO plan is not architecturally valid"))?,
    };
    read_first_segments(root, unit.mmio_base, &plan, &mut first, access)?;
    let second = read_snapshot(root, unit.mmio_base, &plan, access)?;
    require_stable_mmio_snapshot(&first, &second, &plan)
        .map_err(|_| compromised("the inherited IOMMU configuration changed during sampling"))?;

    let second_images = match (second.extended_feature, second.extended_feature_2) {
        (Some(efr), Some(efr2)) => Some(ExtendedFeatureImages { efr, efr2 }),
        (None, None) => None,
        _ => return Err(compromised("the second EFR snapshot is incomplete")),
    };
    let second_features = compare_extended_features(
        capability.extended_feature_register_supported,
        expected_images,
        second_images,
    )
    .map_err(|_| compromised("the second extended-feature snapshot is invalid"))?;
    if second_features != features {
        return Err(compromised(
            "the extended-feature comparison changed during sampling",
        ));
    }
    if !matches!(features, ExtendedFeatureComparison::Conflict { .. }) {
        let rebuilt = build_mmio_read_plan(second.control, second_features)
            .map_err(|_| compromised("the second MMIO plan is invalid"))?;
        if rebuilt != plan {
            return Err(compromised(
                "the live MMIO read plan changed during sampling",
            ));
        }
    }

    let status_raw = mmio_read(root, unit.mmio_base, STATUS_OFFSET, access)?;
    let feature_dependent_offsets = plan
        .stable_offsets
        .iter()
        .copied()
        .filter(|offset| (DEVICE_TABLE_SEGMENT_1_OFFSET..=0x0130).contains(offset))
        .collect::<Vec<_>>();
    let common = LiveMmioCommon {
        aperture_witness,
        plan,
        feature_dependent_offsets,
        first_snapshot: first,
        second_snapshot: second,
        status_raw,
    };
    match features {
        ExtendedFeatureComparison::Conflict { expected, live } => {
            Ok(LiveMmioCapture::EfrConflict {
                common,
                expected,
                live,
            })
        }
        _ => {
            let decoded = decode_live_state(second.control, status_raw);
            let configured_ranges = decode_configured_ranges(
                &second,
                &common.plan,
                decoded,
                capability.iommu_physical_address_width,
                cpu_width,
                descriptors,
            )?;
            Ok(LiveMmioCapture::Observed {
                common,
                features,
                configured_ranges,
                decoded,
            })
        }
    }
}

fn read_base_snapshot(
    root: &mut PciRootBridgeIo,
    base: u64,
    access: &mut LiveIommuAccess,
) -> Result<StableMmioSnapshot, ProbeError> {
    Ok(StableMmioSnapshot {
        device_table_base: mmio_read(root, base, 0x0000, access)?,
        command_buffer_base: mmio_read(root, base, 0x0008, access)?,
        event_log_base: mmio_read(root, base, 0x0010, access)?,
        control: mmio_read(root, base, CONTROL_OFFSET, access)?,
        exclusion_or_completion_base: mmio_read(root, base, 0x0020, access)?,
        exclusion_or_completion_limit: mmio_read(root, base, 0x0028, access)?,
        extended_feature: None,
        extended_feature_2: None,
        device_table_segments: [None; 7],
    })
}

fn read_first_segments(
    root: &mut PciRootBridgeIo,
    base: u64,
    plan: &MmioReadPlan,
    snapshot: &mut StableMmioSnapshot,
    access: &mut LiveIommuAccess,
) -> Result<(), ProbeError> {
    for segment in 1..plan.active_device_table_segments {
        let index = usize::from(segment - 1);
        let offset = DEVICE_TABLE_SEGMENT_1_OFFSET + 8 * u16::from(segment - 1);
        snapshot.device_table_segments[index] = Some(mmio_read(root, base, offset, access)?);
    }
    Ok(())
}

fn read_snapshot(
    root: &mut PciRootBridgeIo,
    base: u64,
    plan: &MmioReadPlan,
    access: &mut LiveIommuAccess,
) -> Result<StableMmioSnapshot, ProbeError> {
    let mut snapshot = read_base_snapshot(root, base, access)?;
    if plan.stable_offsets.contains(&EXTENDED_FEATURE_OFFSET) {
        snapshot.extended_feature = Some(mmio_read(root, base, EXTENDED_FEATURE_OFFSET, access)?);
        snapshot.extended_feature_2 =
            Some(mmio_read(root, base, EXTENDED_FEATURE_2_OFFSET, access)?);
    }
    read_first_segments(root, base, plan, &mut snapshot, access)?;
    Ok(snapshot)
}

fn decode_configured_ranges(
    snapshot: &StableMmioSnapshot,
    plan: &MmioReadPlan,
    decoded: DecodedLiveState,
    iommu_width: u8,
    cpu_width: u8,
    descriptors: &[MemoryDescriptor],
) -> Result<Vec<ConfiguredRangeObservation>, ProbeError> {
    let mut ranges = Vec::new();
    ranges.push(configured_device_table(
        0x0000,
        snapshot.device_table_base,
        decoded.iommu_enabled,
        false,
        iommu_width,
        cpu_width,
        descriptors,
    )?);
    ranges.push(configured_log_buffer(
        0x0008,
        snapshot.command_buffer_base,
        decoded.iommu_enabled && decoded.command_buffer_enabled,
        iommu_width,
        cpu_width,
        descriptors,
    )?);
    ranges.push(configured_log_buffer(
        0x0010,
        snapshot.event_log_base,
        decoded.iommu_enabled && decoded.event_log_enabled,
        iommu_width,
        cpu_width,
        descriptors,
    )?);
    for segment in 1..plan.active_device_table_segments {
        let index = usize::from(segment - 1);
        let offset = DEVICE_TABLE_SEGMENT_1_OFFSET + 8 * u16::from(segment - 1);
        let raw = snapshot.device_table_segments[index]
            .ok_or_else(|| compromised("an active device-table segment is absent"))?;
        ranges.push(configured_device_table(
            offset,
            raw,
            decoded.iommu_enabled,
            true,
            iommu_width,
            cpu_width,
            descriptors,
        )?);
    }
    Ok(ranges)
}

fn configured_device_table(
    source_offset: u16,
    raw: u64,
    enabled: bool,
    segmented: bool,
    iommu_width: u8,
    cpu_width: u8,
    descriptors: &[MemoryDescriptor],
) -> Result<ConfiguredRangeObservation, ProbeError> {
    let size_mask = if segmented { 0xff } else { 0x1ff };
    let entries = (raw & size_mask) + 1;
    finish_configured_range(
        source_offset,
        raw,
        enabled,
        raw & CONFIGURED_BASE_MASK,
        entries
            .checked_mul(4096)
            .ok_or_else(|| compromised("a device-table length overflowed"))?,
        4096,
        iommu_width,
        cpu_width,
        descriptors,
    )
}

fn configured_log_buffer(
    source_offset: u16,
    raw: u64,
    enabled: bool,
    iommu_width: u8,
    cpu_width: u8,
    descriptors: &[MemoryDescriptor],
) -> Result<ConfiguredRangeObservation, ProbeError> {
    let length_code = ((raw >> 56) & 0x0f) as u8;
    if !(8..=15).contains(&length_code) {
        return Err(compromised(
            "an inherited IOMMU command/event buffer has a reserved length",
        ));
    }
    finish_configured_range(
        source_offset,
        raw,
        enabled,
        raw & CONFIGURED_BASE_MASK,
        1_u64 << (u32::from(length_code) + 4),
        4096,
        iommu_width,
        cpu_width,
        descriptors,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_configured_range(
    source_offset: u16,
    raw: u64,
    enabled: bool,
    base: u64,
    length: u64,
    alignment: u64,
    iommu_width: u8,
    cpu_width: u8,
    descriptors: &[MemoryDescriptor],
) -> Result<ConfiguredRangeObservation, ProbeError> {
    let validated_range = validate_enabled_configured_range(
        enabled,
        ConfiguredRange {
            base,
            length,
            required_alignment: alignment,
        },
        iommu_width,
        cpu_width,
    )
    .map_err(|_| compromised("an inherited IOMMU configured range is invalid"))?;
    let memory_witness = validated_range
        .map(|range| {
            memory_descriptor_witness(descriptors, range, iommu_width.min(cpu_width))
                .map_err(|_| compromised("an inherited IOMMU range lacks one memory-map owner"))
        })
        .transpose()?;
    Ok(ConfiguredRangeObservation {
        source_offset,
        raw,
        enabled,
        base,
        length,
        alignment,
        validated_range,
        memory_witness,
    })
}

fn pci_address(unit: &IommuUnit) -> PciIoAddress {
    PciIoAddress::new(unit.bdf.bus, unit.bdf.device, unit.bdf.function)
}

fn pci_read<T: uefi::proto::pci::PciIoUnit>(
    root: &mut PciRootBridgeIo,
    address: PciIoAddress,
    access: &mut LiveIommuAccess,
) -> Result<T, ProbeError> {
    access.count_pci(core::mem::size_of::<T>())?;
    root.pci().read_one::<T>(address).map_err(|error| {
        ProbeError::new(
            error.status(),
            "a bounded PCI Root Bridge I/O configuration read failed",
        )
    })
}

fn mmio_read(
    root: &mut PciRootBridgeIo,
    base: u64,
    offset: u16,
    access: &mut LiveIommuAccess,
) -> Result<u64, ProbeError> {
    let address = base
        .checked_add(u64::from(offset))
        .ok_or_else(|| compromised("an allowlisted IOMMU MMIO address overflowed"))?;
    access.count_mmio()?;
    root.memory().read_one::<u64>(address).map_err(|error| {
        ProbeError::new(
            error.status(),
            "an allowlisted PCI Root Bridge I/O memory read failed",
        )
    })
}

fn compromised(message: &'static str) -> ProbeError {
    ProbeError::new(Status::COMPROMISED_DATA, message)
}
