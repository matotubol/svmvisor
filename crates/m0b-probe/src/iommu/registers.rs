//! Pure decoders and allowlist planning for live AMD-IOMMU register reads.
//!
//! This module consumes values already read by the firmware adapter.  It does
//! not expose a write operation or dereference any configured physical pointer.

use alloc::vec::Vec;

pub const AMD_VENDOR_ID: u16 = 0x1022;
pub const IOMMU_CAPABILITY_ID: u8 = 0x0f;
pub const IOMMU_CAPABILITY_TYPE: u8 = 3;
pub const MAX_CAPABILITY_LINKS: usize = 48;

pub const DEVICE_TABLE_BASE_OFFSET: u16 = 0x0000;
pub const COMMAND_BUFFER_BASE_OFFSET: u16 = 0x0008;
pub const EVENT_LOG_BASE_OFFSET: u16 = 0x0010;
pub const CONTROL_OFFSET: u16 = 0x0018;
pub const EXCLUSION_OR_COMPLETION_BASE_OFFSET: u16 = 0x0020;
pub const EXCLUSION_OR_COMPLETION_LIMIT_OFFSET: u16 = 0x0028;
pub const EXTENDED_FEATURE_OFFSET: u16 = 0x0030;
pub const DEVICE_TABLE_SEGMENT_1_OFFSET: u16 = 0x0100;
pub const EXTENDED_FEATURE_2_OFFSET: u16 = 0x01a0;
pub const STATUS_OFFSET: u16 = 0x2020;

pub const BASE_STABLE_MMIO_OFFSETS: [u16; 6] = [
    DEVICE_TABLE_BASE_OFFSET,
    COMMAND_BUFFER_BASE_OFFSET,
    EVENT_LOG_BASE_OFFSET,
    CONTROL_OFFSET,
    EXCLUSION_OR_COMPLETION_BASE_OFFSET,
    EXCLUSION_OR_COMPLETION_LIMIT_OFFSET,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciIdentityRaw {
    pub vendor_device: u32,
    pub command_status: u32,
    pub class_revision: u32,
    pub header_type: u8,
    pub first_capability_pointer: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciIdentity {
    pub vendor_id: u16,
    pub device_id: u16,
    pub capabilities_list_present: bool,
    pub class_code: u8,
    pub subclass: u8,
    pub programming_interface: u8,
    pub revision_id: u8,
    pub header_layout: u8,
    pub multifunction: bool,
    pub first_capability_pointer: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityLink {
    pub offset: u8,
    pub capability_id: u8,
    pub next: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciCapabilityRaw {
    pub header: u32,
    pub base_low: u32,
    pub base_high: u32,
    pub range: u32,
    pub miscellaneous_0: u32,
    pub miscellaneous_1: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciCapability {
    pub capability_id: u8,
    pub next_pointer: u8,
    pub capability_type: u8,
    pub capability_revision: u8,
    pub extended_feature_register_supported: bool,
    pub capability_extension_supported: bool,
    pub mmio_enabled: bool,
    pub decoded_mmio_base: u64,
    pub range: u32,
    pub iommu_physical_address_width: u8,
    pub miscellaneous_1: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtendedFeatureImages {
    pub efr: u64,
    pub efr2: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtendedFeatureComparison {
    NotSupported,
    Match {
        live: ExtendedFeatureImages,
        performance_counters_supported: bool,
        device_table_segments_supported: u8,
    },
    Conflict {
        expected: ExtendedFeatureImages,
        live: ExtendedFeatureImages,
    },
}

/// Exact stable-register plan. Status is deliberately excluded and sampled once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MmioReadPlan {
    pub stable_offsets: Vec<u16>,
    pub status_offset: u16,
    pub active_device_table_segments: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableMmioSnapshot {
    pub device_table_base: u64,
    pub command_buffer_base: u64,
    pub event_log_base: u64,
    pub control: u64,
    pub exclusion_or_completion_base: u64,
    pub exclusion_or_completion_limit: u64,
    pub extended_feature: Option<u64>,
    pub extended_feature_2: Option<u64>,
    /// Entries 0..N correspond to device-table segments 1..=N.
    pub device_table_segments: [Option<u64>; 7],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedLiveState {
    pub iommu_enabled: bool,
    pub event_log_enabled: bool,
    pub command_buffer_enabled: bool,
    pub device_table_segment_encoding: u8,
    pub event_log_running: bool,
    pub command_buffer_running: bool,
    pub event_overflow: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterError {
    FunctionAbsent,
    WrongVendor(u16),
    MissingCapabilitiesList,
    WrongClass {
        class_code: u8,
        subclass: u8,
        programming_interface: u8,
    },
    UnsupportedHeaderLayout(u8),
    EmptyCapabilityChain,
    CapabilityChainTooLong(usize),
    InvalidCapabilityPointer(u8),
    UnexpectedCapabilityLink {
        expected: u8,
        observed: u8,
    },
    CyclicCapabilityChain(u8),
    TargetCapabilityMissing,
    WrongTargetCapabilityId(u8),
    TrailingCapabilityReads,
    WrongCapabilityId(u8),
    WrongCapabilityType(u8),
    MissingMiscellaneous1,
    UnexpectedMiscellaneous1,
    InvalidIommuPhysicalAddressWidth(u8),
    MmioBaseMismatch {
        expected: u64,
        observed: u64,
    },
    UnstablePciCapability,
    MissingExtendedFeatureImages,
    UnexpectedExtendedFeatureImages,
    MissingIvrsExtendedFeatureImages,
    UnexpectedIvrsExtendedFeatureImages,
    ExtendedFeatureConflict,
    ReservedDeviceTableSegmentEncoding(u8),
    DeviceTableSegmentationExceedsSupport {
        enabled: u8,
        supported: u8,
    },
    SnapshotShapeMismatch,
    UnstableMmioSnapshot,
}

pub fn decode_and_validate_identity(raw: PciIdentityRaw) -> Result<PciIdentity, RegisterError> {
    let vendor_id = raw.vendor_device as u16;
    let device_id = (raw.vendor_device >> 16) as u16;
    if vendor_id == 0xffff || vendor_id == 0 {
        return Err(RegisterError::FunctionAbsent);
    }
    if vendor_id != AMD_VENDOR_ID {
        return Err(RegisterError::WrongVendor(vendor_id));
    }
    let capabilities_list_present = raw.command_status & (1 << 20) != 0;
    if !capabilities_list_present {
        return Err(RegisterError::MissingCapabilitiesList);
    }
    let class_code = (raw.class_revision >> 24) as u8;
    let subclass = (raw.class_revision >> 16) as u8;
    let programming_interface = (raw.class_revision >> 8) as u8;
    if (class_code, subclass, programming_interface) != (0x08, 0x06, 0x00) {
        return Err(RegisterError::WrongClass {
            class_code,
            subclass,
            programming_interface,
        });
    }
    let header_layout = raw.header_type & 0x7f;
    if header_layout != 0 {
        return Err(RegisterError::UnsupportedHeaderLayout(header_layout));
    }
    validate_capability_pointer(raw.first_capability_pointer)?;
    Ok(PciIdentity {
        vendor_id,
        device_id,
        capabilities_list_present,
        class_code,
        subclass,
        programming_interface,
        revision_id: raw.class_revision as u8,
        header_layout,
        multifunction: raw.header_type & 0x80 != 0,
        first_capability_pointer: raw.first_capability_pointer,
    })
}

/// Validate the exact bounded link sequence read up to the IVRS-selected target.
pub fn validate_capability_chain(
    first_pointer: u8,
    target_offset: u16,
    links: &[CapabilityLink],
) -> Result<(), RegisterError> {
    if links.is_empty() {
        return Err(RegisterError::EmptyCapabilityChain);
    }
    if links.len() > MAX_CAPABILITY_LINKS {
        return Err(RegisterError::CapabilityChainTooLong(links.len()));
    }
    let target = u8::try_from(target_offset).map_err(|_| RegisterError::TargetCapabilityMissing)?;
    validate_capability_pointer(target)?;
    let mut expected = first_pointer;
    validate_capability_pointer(expected)?;
    for (index, link) in links.iter().copied().enumerate() {
        if link.offset != expected {
            return Err(RegisterError::UnexpectedCapabilityLink {
                expected,
                observed: link.offset,
            });
        }
        if links[..index]
            .iter()
            .any(|prior| prior.offset == link.offset)
        {
            return Err(RegisterError::CyclicCapabilityChain(link.offset));
        }
        if link.offset == target {
            if link.capability_id != IOMMU_CAPABILITY_ID {
                return Err(RegisterError::WrongTargetCapabilityId(link.capability_id));
            }
            if link.next != 0 {
                validate_capability_pointer(link.next)?;
                if links[..=index]
                    .iter()
                    .any(|observed| observed.offset == link.next)
                {
                    return Err(RegisterError::CyclicCapabilityChain(link.next));
                }
            }
            return if index + 1 == links.len() {
                Ok(())
            } else {
                Err(RegisterError::TrailingCapabilityReads)
            };
        }
        if link.next == 0 {
            return Err(RegisterError::TargetCapabilityMissing);
        }
        validate_capability_pointer(link.next)?;
        expected = link.next;
    }
    Err(RegisterError::TargetCapabilityMissing)
}

pub fn decode_and_validate_capability(
    raw: PciCapabilityRaw,
    expected_mmio_base: u64,
) -> Result<PciCapability, RegisterError> {
    let capability_id = raw.header as u8;
    if capability_id != IOMMU_CAPABILITY_ID {
        return Err(RegisterError::WrongCapabilityId(capability_id));
    }
    let capability_type = ((raw.header >> 16) & 0x7) as u8;
    if capability_type != IOMMU_CAPABILITY_TYPE {
        return Err(RegisterError::WrongCapabilityType(capability_type));
    }
    let capability_extension_supported = raw.header & (1 << 28) != 0;
    match (capability_extension_supported, raw.miscellaneous_1) {
        (true, None) => return Err(RegisterError::MissingMiscellaneous1),
        (false, Some(_)) => return Err(RegisterError::UnexpectedMiscellaneous1),
        _ => {}
    }
    let iommu_physical_address_width = ((raw.miscellaneous_0 >> 8) & 0x7f) as u8;
    if !matches!(iommu_physical_address_width, 40 | 48 | 52) {
        return Err(RegisterError::InvalidIommuPhysicalAddressWidth(
            iommu_physical_address_width,
        ));
    }
    let decoded_mmio_base =
        (u64::from(raw.base_high) << 32) | u64::from(raw.base_low & 0xffff_c000);
    if decoded_mmio_base != expected_mmio_base {
        return Err(RegisterError::MmioBaseMismatch {
            expected: expected_mmio_base,
            observed: decoded_mmio_base,
        });
    }
    Ok(PciCapability {
        capability_id,
        next_pointer: ((raw.header >> 8) & 0xff) as u8,
        capability_type,
        capability_revision: ((raw.header >> 19) & 0x1f) as u8,
        extended_feature_register_supported: raw.header & (1 << 27) != 0,
        capability_extension_supported,
        mmio_enabled: raw.base_low & 1 != 0,
        decoded_mmio_base,
        range: raw.range,
        iommu_physical_address_width,
        miscellaneous_1: raw.miscellaneous_1,
    })
}

pub fn require_stable_pci_capability(
    first: PciCapabilityRaw,
    second: PciCapabilityRaw,
) -> Result<(), RegisterError> {
    if first == second {
        Ok(())
    } else {
        Err(RegisterError::UnstablePciCapability)
    }
}

pub fn compare_extended_features(
    supported: bool,
    expected: Option<ExtendedFeatureImages>,
    live: Option<ExtendedFeatureImages>,
) -> Result<ExtendedFeatureComparison, RegisterError> {
    match (supported, expected, live) {
        (false, None, None) => Ok(ExtendedFeatureComparison::NotSupported),
        (false, Some(_), _) => Err(RegisterError::UnexpectedIvrsExtendedFeatureImages),
        (false, None, Some(_)) => Err(RegisterError::UnexpectedExtendedFeatureImages),
        (true, None, _) => Err(RegisterError::MissingIvrsExtendedFeatureImages),
        (true, Some(_), None) => Err(RegisterError::MissingExtendedFeatureImages),
        (true, Some(expected), Some(live)) if expected == live => {
            Ok(ExtendedFeatureComparison::Match {
                live,
                performance_counters_supported: live.efr & (1 << 9) != 0,
                device_table_segments_supported: ((live.efr >> 38) & 0x3) as u8,
            })
        }
        (true, Some(expected), Some(live)) => {
            Ok(ExtendedFeatureComparison::Conflict { expected, live })
        }
    }
}

/// Construct only the offsets authorized by the reviewed policy.
pub fn build_mmio_read_plan(
    control: u64,
    features: ExtendedFeatureComparison,
) -> Result<MmioReadPlan, RegisterError> {
    let enabled_segments = ((control >> 34) & 0x7) as u8;
    if enabled_segments > 3 {
        return Err(RegisterError::ReservedDeviceTableSegmentEncoding(
            enabled_segments,
        ));
    }
    let mut offsets = Vec::with_capacity(15);
    offsets.extend_from_slice(&BASE_STABLE_MMIO_OFFSETS);
    let supported_segments = match features {
        ExtendedFeatureComparison::NotSupported => 0,
        ExtendedFeatureComparison::Match {
            device_table_segments_supported,
            ..
        } => {
            offsets.push(EXTENDED_FEATURE_OFFSET);
            offsets.push(EXTENDED_FEATURE_2_OFFSET);
            device_table_segments_supported
        }
        ExtendedFeatureComparison::Conflict { .. } => {
            return Err(RegisterError::ExtendedFeatureConflict);
        }
    };
    if enabled_segments > supported_segments {
        return Err(RegisterError::DeviceTableSegmentationExceedsSupport {
            enabled: enabled_segments,
            supported: supported_segments,
        });
    }
    let active_segments = 1_u8 << enabled_segments;
    for segment in 1..active_segments {
        offsets.push(DEVICE_TABLE_SEGMENT_1_OFFSET + 8 * u16::from(segment - 1));
    }
    Ok(MmioReadPlan {
        stable_offsets: offsets,
        status_offset: STATUS_OFFSET,
        active_device_table_segments: active_segments,
    })
}

pub fn validate_snapshot_shape(
    snapshot: &StableMmioSnapshot,
    plan: &MmioReadPlan,
) -> Result<(), RegisterError> {
    let has_efr = plan.stable_offsets.contains(&EXTENDED_FEATURE_OFFSET);
    if snapshot.extended_feature.is_some() != has_efr
        || snapshot.extended_feature_2.is_some() != has_efr
    {
        return Err(RegisterError::SnapshotShapeMismatch);
    }
    let additional = usize::from(plan.active_device_table_segments.saturating_sub(1));
    for (index, value) in snapshot.device_table_segments.iter().enumerate() {
        if value.is_some() != (index < additional) {
            return Err(RegisterError::SnapshotShapeMismatch);
        }
    }
    Ok(())
}

pub fn require_stable_mmio_snapshot(
    first: &StableMmioSnapshot,
    second: &StableMmioSnapshot,
    plan: &MmioReadPlan,
) -> Result<(), RegisterError> {
    validate_snapshot_shape(first, plan)?;
    validate_snapshot_shape(second, plan)?;
    if first == second {
        Ok(())
    } else {
        Err(RegisterError::UnstableMmioSnapshot)
    }
}

#[must_use]
pub const fn decode_live_state(control: u64, status: u64) -> DecodedLiveState {
    DecodedLiveState {
        iommu_enabled: control & 1 != 0,
        event_log_enabled: control & (1 << 2) != 0,
        command_buffer_enabled: control & (1 << 12) != 0,
        device_table_segment_encoding: ((control >> 34) & 0x7) as u8,
        event_log_running: status & (1 << 3) != 0,
        command_buffer_running: status & (1 << 4) != 0,
        event_overflow: status & 1 != 0,
    }
}

#[must_use]
pub const fn required_aperture_bytes(features: ExtendedFeatureComparison) -> Option<u64> {
    match features {
        ExtendedFeatureComparison::NotSupported => Some(16 * 1024),
        ExtendedFeatureComparison::Match {
            performance_counters_supported: true,
            ..
        } => Some(512 * 1024),
        ExtendedFeatureComparison::Match {
            performance_counters_supported: false,
            ..
        } => Some(16 * 1024),
        ExtendedFeatureComparison::Conflict { .. } => None,
    }
}

fn validate_capability_pointer(pointer: u8) -> Result<(), RegisterError> {
    if (0x40..=0xfc).contains(&pointer) && pointer.is_multiple_of(4) {
        Ok(())
    } else {
        Err(RegisterError::InvalidCapabilityPointer(pointer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_raw() -> PciIdentityRaw {
        PciIdentityRaw {
            vendor_device: 0x1234_1022,
            command_status: 1 << 20,
            class_revision: 0x0806_0001,
            header_type: 0x80,
            first_capability_pointer: 0x40,
        }
    }

    fn capability_raw() -> PciCapabilityRaw {
        PciCapabilityRaw {
            header: 0x180b_000f,
            base_low: 0xf760_0001,
            base_high: 0,
            range: 0,
            miscellaneous_0: 48 << 8,
            miscellaneous_1: Some(0),
        }
    }

    fn snapshot(efr: bool, additional_segments: usize) -> StableMmioSnapshot {
        let mut segments = [None; 7];
        for value in segments.iter_mut().take(additional_segments) {
            *value = Some(0x1000);
        }
        StableMmioSnapshot {
            device_table_base: 0,
            command_buffer_base: 0,
            event_log_base: 0,
            control: 0,
            exclusion_or_completion_base: 0,
            exclusion_or_completion_limit: 0,
            extended_feature: efr.then_some(0),
            extended_feature_2: efr.then_some(0),
            device_table_segments: segments,
        }
    }

    #[test]
    fn validates_amd_iommu_identity() {
        let identity = decode_and_validate_identity(identity_raw()).unwrap();
        assert_eq!(identity.vendor_id, AMD_VENDOR_ID);
        assert_eq!((identity.class_code, identity.subclass), (0x08, 0x06));
        assert!(identity.multifunction);
        let mut bad = identity_raw();
        bad.class_revision = 0x0604_0000;
        assert!(matches!(
            decode_and_validate_identity(bad),
            Err(RegisterError::WrongClass { .. })
        ));
    }

    #[test]
    fn capability_chain_is_bounded_aligned_and_targeted() {
        let links = [
            CapabilityLink {
                offset: 0x40,
                capability_id: 1,
                next: 0x60,
            },
            CapabilityLink {
                offset: 0x60,
                capability_id: IOMMU_CAPABILITY_ID,
                next: 0,
            },
        ];
        assert_eq!(validate_capability_chain(0x40, 0x60, &links), Ok(()));
        let mut invalid_target_next = links;
        invalid_target_next[1].next = 0x41;
        assert_eq!(
            validate_capability_chain(0x40, 0x60, &invalid_target_next),
            Err(RegisterError::InvalidCapabilityPointer(0x41))
        );
        let mut target_cycle = links;
        target_cycle[1].next = 0x40;
        assert_eq!(
            validate_capability_chain(0x40, 0x60, &target_cycle),
            Err(RegisterError::CyclicCapabilityChain(0x40))
        );
        let mut cycle = links;
        cycle[1].capability_id = 2;
        cycle[1].next = 0x40;
        assert!(matches!(
            validate_capability_chain(0x40, 0x80, &cycle),
            Err(RegisterError::TargetCapabilityMissing)
        ));
    }

    #[test]
    fn capability_decode_enforces_type_conditional_misc_and_base() {
        let cap = decode_and_validate_capability(capability_raw(), 0xf760_0000).unwrap();
        assert!(cap.mmio_enabled && cap.extended_feature_register_supported);
        assert_eq!(cap.capability_revision, 1);
        assert_eq!(cap.iommu_physical_address_width, 48);
        let mut bad = capability_raw();
        bad.miscellaneous_1 = None;
        assert_eq!(
            decode_and_validate_capability(bad, 0xf760_0000),
            Err(RegisterError::MissingMiscellaneous1)
        );
        assert!(matches!(
            decode_and_validate_capability(capability_raw(), 0xf768_0000),
            Err(RegisterError::MmioBaseMismatch { .. })
        ));
    }

    #[test]
    fn enable_zero_is_valid_pci_only_state() {
        let mut raw = capability_raw();
        raw.base_low &= !1;
        let cap = decode_and_validate_capability(raw, 0xf760_0000).unwrap();
        assert!(!cap.mmio_enabled);
    }

    #[test]
    fn efr_comparison_retains_conflicts_and_selects_aperture() {
        let expected = ExtendedFeatureImages {
            efr: 1 << 9,
            efr2: 0,
        };
        let matched = compare_extended_features(true, Some(expected), Some(expected)).unwrap();
        assert_eq!(required_aperture_bytes(matched), Some(512 * 1024));
        let live = ExtendedFeatureImages { efr: 0, efr2: 1 };
        let conflict = compare_extended_features(true, Some(expected), Some(live)).unwrap();
        assert!(matches!(
            conflict,
            ExtendedFeatureComparison::Conflict { .. }
        ));
        assert_eq!(required_aperture_bytes(conflict), None);
    }

    #[test]
    fn read_plan_contains_only_reviewed_offsets() {
        let features = ExtendedFeatureComparison::Match {
            live: ExtendedFeatureImages {
                efr: (2_u64 << 38) | (1 << 9),
                efr2: 0,
            },
            performance_counters_supported: true,
            device_table_segments_supported: 2,
        };
        let plan = build_mmio_read_plan(2_u64 << 34, features).unwrap();
        assert_eq!(plan.active_device_table_segments, 4);
        assert_eq!(
            &plan.stable_offsets[plan.stable_offsets.len() - 3..],
            &[0x100, 0x108, 0x110]
        );
        for forbidden in [0x38, 0x2000, 0x2008, 0x2010, 0x2018, 0x2030] {
            assert!(!plan.stable_offsets.contains(&forbidden));
        }
        assert_eq!(plan.status_offset, STATUS_OFFSET);
    }

    #[test]
    fn segmentation_is_feature_gated_and_reserved_values_fail() {
        assert!(matches!(
            build_mmio_read_plan(1_u64 << 34, ExtendedFeatureComparison::NotSupported),
            Err(RegisterError::DeviceTableSegmentationExceedsSupport { .. })
        ));
        assert!(matches!(
            build_mmio_read_plan(4_u64 << 34, ExtendedFeatureComparison::NotSupported),
            Err(RegisterError::ReservedDeviceTableSegmentEncoding(4))
        ));
    }

    #[test]
    fn stable_snapshot_shape_and_equality_are_enforced() {
        let features = ExtendedFeatureComparison::Match {
            live: ExtendedFeatureImages { efr: 0, efr2: 0 },
            performance_counters_supported: false,
            device_table_segments_supported: 1,
        };
        let plan = build_mmio_read_plan(1_u64 << 34, features).unwrap();
        let first = snapshot(true, 1);
        assert_eq!(require_stable_mmio_snapshot(&first, &first, &plan), Ok(()));
        let mut changed = first;
        changed.control = 1;
        assert_eq!(
            require_stable_mmio_snapshot(&first, &changed, &plan),
            Err(RegisterError::UnstableMmioSnapshot)
        );
    }

    #[test]
    fn control_and_status_bits_decode_without_claiming_ownership() {
        let state = decode_live_state((1 << 12) | (1 << 2) | 1, (1 << 4) | (1 << 3) | 1);
        assert!(state.iommu_enabled);
        assert!(state.command_buffer_enabled && state.event_log_enabled);
        assert!(state.command_buffer_running && state.event_log_running);
        assert!(state.event_overflow);
    }
}
