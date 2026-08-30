//! Pure, hardware-access-free AMD-IOMMU locator validation.

pub const IOMMU_MMIO_ALIGNMENT: u64 = 1 << 14;
pub const MCFG_BASE_ALIGNMENT: u64 = 1 << 20;
pub const MIN_IOMMU_CAPABILITY_OFFSET: u16 = 0x40;
pub const MAX_IOMMU_CAPABILITY_OFFSET: u16 = 0xe8;

const ECAM_BUS_STRIDE: u64 = 1 << 20;
const ECAM_DEVICE_STRIDE: u64 = 1 << 15;
const ECAM_FUNCTION_STRIDE: u64 = 1 << 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IvhdSource {
    Type10,
    Type11,
    Type40,
}

impl IvhdSource {
    pub const fn from_entry_type(entry_type: u8) -> Result<Self, LocatorError> {
        match entry_type {
            0x10 => Ok(Self::Type10),
            0x11 => Ok(Self::Type11),
            0x40 => Ok(Self::Type40),
            _ => Err(LocatorError::UnsupportedIvhdType(entry_type)),
        }
    }

    #[must_use]
    pub const fn entry_type(self) -> u8 {
        match self {
            Self::Type10 => 0x10,
            Self::Type11 => 0x11,
            Self::Type40 => 0x40,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IvhdInput {
    pub source_index: usize,
    pub source: IvhdSource,
    pub segment_group: u16,
    pub device_id: u16,
    pub capability_offset: u16,
    pub mmio_base: u64,
    pub efr_image: Option<u64>,
    pub efr2_image: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciBdf {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciBdf {
    #[must_use]
    pub const fn decode(device_id: u16) -> Self {
        Self {
            bus: (device_id >> 8) as u8,
            device: ((device_id >> 3) & 0x1f) as u8,
            function: (device_id & 0x07) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IommuUnit {
    pub segment_group: u16,
    pub device_id: u16,
    pub bdf: PciBdf,
    pub capability_offset: u16,
    pub mmio_base: u64,
    pub type10_present: bool,
    pub type11_present: bool,
    pub type40_present: bool,
    pub efr_image: Option<u64>,
    pub efr2_image: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgInput {
    pub source_index: usize,
    pub base_address: u64,
    pub segment_group: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgRange {
    pub start: u64,
    pub end_exclusive: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McfgWitness {
    pub allocation: McfgInput,
    pub usable_range: McfgRange,
    pub function_address: u64,
    pub capability_address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocatorError {
    UnsupportedIvhdType(u8),
    MissingIvhd,
    MultipleIommuUnits,
    DuplicateIvhdSource(IvhdSource),
    InvalidCapabilityOffset {
        source_index: usize,
        offset: u16,
    },
    MisalignedMmioBase {
        source_index: usize,
        address: u64,
    },
    InvalidExtendedFeatureShape {
        source_index: usize,
        source: IvhdSource,
    },
    ConflictingCapabilityOffset,
    ConflictingMmioBase,
    ConflictingEfrImage,
    ConflictingEfr2Image,
    InvalidMcfgBusRange(usize),
    MisalignedMcfgBase {
        source_index: usize,
        address: u64,
    },
    MissingMcfgCoverage,
    AmbiguousMcfgCoverage {
        first_source_index: usize,
        other_source_index: usize,
    },
    ArithmeticOverflow(&'static str),
}

pub fn derive_unique_unit(inputs: &[IvhdInput]) -> Result<IommuUnit, LocatorError> {
    let Some(first) = inputs.first().copied() else {
        return Err(LocatorError::MissingIvhd);
    };
    validate_ivhd(first)?;

    let mut unit = IommuUnit {
        segment_group: first.segment_group,
        device_id: first.device_id,
        bdf: PciBdf::decode(first.device_id),
        capability_offset: first.capability_offset,
        mmio_base: first.mmio_base,
        type10_present: false,
        type11_present: false,
        type40_present: false,
        efr_image: None,
        efr2_image: None,
    };
    merge_source(&mut unit, first)?;

    for input in &inputs[1..] {
        validate_ivhd(*input)?;
        if input.segment_group != unit.segment_group || input.device_id != unit.device_id {
            return Err(LocatorError::MultipleIommuUnits);
        }
        if input.capability_offset != unit.capability_offset {
            return Err(LocatorError::ConflictingCapabilityOffset);
        }
        if input.mmio_base != unit.mmio_base {
            return Err(LocatorError::ConflictingMmioBase);
        }
        merge_source(&mut unit, *input)?;
    }
    Ok(unit)
}

fn validate_ivhd(input: IvhdInput) -> Result<(), LocatorError> {
    if input.capability_offset < MIN_IOMMU_CAPABILITY_OFFSET
        || input.capability_offset > MAX_IOMMU_CAPABILITY_OFFSET
        || !input.capability_offset.is_multiple_of(4)
    {
        return Err(LocatorError::InvalidCapabilityOffset {
            source_index: input.source_index,
            offset: input.capability_offset,
        });
    }
    if !input.mmio_base.is_multiple_of(IOMMU_MMIO_ALIGNMENT) {
        return Err(LocatorError::MisalignedMmioBase {
            source_index: input.source_index,
            address: input.mmio_base,
        });
    }
    let feature_shape_valid = match input.source {
        IvhdSource::Type10 => input.efr_image.is_none() && input.efr2_image.is_none(),
        IvhdSource::Type11 | IvhdSource::Type40 => {
            input.efr_image.is_some() && input.efr2_image.is_some()
        }
    };
    if !feature_shape_valid {
        return Err(LocatorError::InvalidExtendedFeatureShape {
            source_index: input.source_index,
            source: input.source,
        });
    }
    Ok(())
}

fn merge_source(unit: &mut IommuUnit, input: IvhdInput) -> Result<(), LocatorError> {
    let present = match input.source {
        IvhdSource::Type10 => &mut unit.type10_present,
        IvhdSource::Type11 => &mut unit.type11_present,
        IvhdSource::Type40 => &mut unit.type40_present,
    };
    if *present {
        return Err(LocatorError::DuplicateIvhdSource(input.source));
    }
    *present = true;

    if let Some(observed) = input.efr_image {
        if let Some(expected) = unit.efr_image {
            if observed != expected {
                return Err(LocatorError::ConflictingEfrImage);
            }
        } else {
            unit.efr_image = Some(observed);
        }
    }
    if let Some(observed) = input.efr2_image {
        if let Some(expected) = unit.efr2_image {
            if observed != expected {
                return Err(LocatorError::ConflictingEfr2Image);
            }
        } else {
            unit.efr2_image = Some(observed);
        }
    }
    Ok(())
}

pub fn mcfg_usable_range(input: McfgInput) -> Result<McfgRange, LocatorError> {
    if input.start_bus > input.end_bus {
        return Err(LocatorError::InvalidMcfgBusRange(input.source_index));
    }
    if !input.base_address.is_multiple_of(MCFG_BASE_ALIGNMENT) {
        return Err(LocatorError::MisalignedMcfgBase {
            source_index: input.source_index,
            address: input.base_address,
        });
    }
    let start_offset = u64::from(input.start_bus) * ECAM_BUS_STRIDE;
    let end_offset = (u64::from(input.end_bus) + 1) * ECAM_BUS_STRIDE;
    let start = input
        .base_address
        .checked_add(start_offset)
        .ok_or(LocatorError::ArithmeticOverflow("MCFG usable range start"))?;
    let end_exclusive = input
        .base_address
        .checked_add(end_offset)
        .ok_or(LocatorError::ArithmeticOverflow("MCFG usable range end"))?;
    Ok(McfgRange {
        start,
        end_exclusive,
    })
}

pub fn derive_unique_mcfg_witness(
    unit: &IommuUnit,
    allocations: &[McfgInput],
) -> Result<McfgWitness, LocatorError> {
    let mut selected: Option<(McfgInput, McfgRange)> = None;
    for allocation in allocations {
        let range = mcfg_usable_range(*allocation)?;
        if allocation.segment_group == unit.segment_group
            && allocation.start_bus <= unit.bdf.bus
            && unit.bdf.bus <= allocation.end_bus
        {
            if let Some((first, _)) = selected {
                return Err(LocatorError::AmbiguousMcfgCoverage {
                    first_source_index: first.source_index,
                    other_source_index: allocation.source_index,
                });
            }
            selected = Some((*allocation, range));
        }
    }
    let Some((allocation, usable_range)) = selected else {
        return Err(LocatorError::MissingMcfgCoverage);
    };

    let bus_offset = u64::from(unit.bdf.bus) * ECAM_BUS_STRIDE;
    let device_offset = u64::from(unit.bdf.device) * ECAM_DEVICE_STRIDE;
    let function_offset = u64::from(unit.bdf.function) * ECAM_FUNCTION_STRIDE;
    let function_offset = bus_offset
        .checked_add(device_offset)
        .and_then(|value| value.checked_add(function_offset))
        .ok_or(LocatorError::ArithmeticOverflow("ECAM function offset"))?;
    let function_address = allocation
        .base_address
        .checked_add(function_offset)
        .ok_or(LocatorError::ArithmeticOverflow("ECAM function address"))?;
    let capability_address = function_address
        .checked_add(u64::from(unit.capability_offset))
        .ok_or(LocatorError::ArithmeticOverflow("ECAM capability address"))?;

    Ok(McfgWitness {
        allocation,
        usable_range,
        function_address,
        capability_address,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ivhd(source: IvhdSource) -> IvhdInput {
        let (efr_image, efr2_image) = match source {
            IvhdSource::Type10 => (None, None),
            IvhdSource::Type11 | IvhdSource::Type40 => (Some(0x1234), Some(0x5678)),
        };
        IvhdInput {
            source_index: source.entry_type() as usize,
            source,
            segment_group: 0,
            device_id: 0x0002,
            capability_offset: 0x40,
            mmio_base: 0xf760_0000,
            efr_image,
            efr2_image,
        }
    }

    #[test]
    fn coalesces_three_canonical_ivhd_sources() {
        let unit = derive_unique_unit(&[
            ivhd(IvhdSource::Type10),
            ivhd(IvhdSource::Type11),
            ivhd(IvhdSource::Type40),
        ])
        .unwrap();
        assert_eq!(
            (unit.bdf.bus, unit.bdf.device, unit.bdf.function),
            (0, 0, 2)
        );
        assert!(unit.type10_present && unit.type11_present && unit.type40_present);
        assert_eq!(unit.efr_image, Some(0x1234));
    }

    #[test]
    fn rejects_zero_multiple_and_duplicate_units() {
        assert_eq!(derive_unique_unit(&[]), Err(LocatorError::MissingIvhd));
        let mut other = ivhd(IvhdSource::Type11);
        other.device_id = 0x0102;
        assert!(matches!(
            derive_unique_unit(&[ivhd(IvhdSource::Type10), other]),
            Err(LocatorError::MultipleIommuUnits)
        ));
        assert_eq!(
            derive_unique_unit(&[ivhd(IvhdSource::Type10), ivhd(IvhdSource::Type10)]),
            Err(LocatorError::DuplicateIvhdSource(IvhdSource::Type10))
        );
    }

    #[test]
    fn rejects_conflicting_ivhd_witnesses() {
        let first = ivhd(IvhdSource::Type11);
        let mut conflict = ivhd(IvhdSource::Type40);
        conflict.mmio_base += IOMMU_MMIO_ALIGNMENT;
        assert!(matches!(
            derive_unique_unit(&[first, conflict]),
            Err(LocatorError::ConflictingMmioBase)
        ));
        conflict = ivhd(IvhdSource::Type40);
        conflict.efr2_image = Some(0x9999);
        assert!(matches!(
            derive_unique_unit(&[first, conflict]),
            Err(LocatorError::ConflictingEfr2Image)
        ));
    }

    #[test]
    fn validates_capability_and_mmio_alignment() {
        for offset in [0x3c, 0x42, 0xec] {
            let mut input = ivhd(IvhdSource::Type10);
            input.capability_offset = offset;
            assert!(matches!(
                derive_unique_unit(&[input]),
                Err(LocatorError::InvalidCapabilityOffset { .. })
            ));
        }
        let mut input = ivhd(IvhdSource::Type10);
        input.mmio_base += 0x1000;
        assert!(matches!(
            derive_unique_unit(&[input]),
            Err(LocatorError::MisalignedMmioBase { .. })
        ));
    }

    #[test]
    fn nonzero_start_bus_remains_bus_zero_relative() {
        let mut only = ivhd(IvhdSource::Type10);
        only.device_id = 0x20fa;
        let unit = derive_unique_unit(&[only]).unwrap();
        let allocation = McfgInput {
            source_index: 7,
            base_address: 0xe000_0000,
            segment_group: 0,
            start_bus: 0x20,
            end_bus: 0x2f,
        };
        let witness = derive_unique_mcfg_witness(&unit, &[allocation]).unwrap();
        assert_eq!(witness.usable_range.start, 0xe200_0000);
        assert_eq!(witness.usable_range.end_exclusive, 0xe300_0000);
        assert_eq!(witness.function_address, 0xe20f_a000);
        assert_eq!(witness.capability_address, 0xe20f_a040);
    }

    #[test]
    fn requires_exactly_one_covering_mcfg_allocation() {
        let unit = derive_unique_unit(&[ivhd(IvhdSource::Type10)]).unwrap();
        let base = McfgInput {
            source_index: 1,
            base_address: 0xe000_0000,
            segment_group: 0,
            start_bus: 0,
            end_bus: 0xff,
        };
        assert!(matches!(
            derive_unique_mcfg_witness(&unit, &[]),
            Err(LocatorError::MissingMcfgCoverage)
        ));
        let duplicate = McfgInput {
            source_index: 2,
            ..base
        };
        assert!(matches!(
            derive_unique_mcfg_witness(&unit, &[base, duplicate]),
            Err(LocatorError::AmbiguousMcfgCoverage { .. })
        ));
        let allocation = McfgInput {
            source_index: 0,
            base_address: !(MCFG_BASE_ALIGNMENT - 1),
            segment_group: 0,
            start_bus: 1,
            end_bus: 1,
        };
        assert!(matches!(
            mcfg_usable_range(allocation),
            Err(LocatorError::ArithmeticOverflow(_))
        ));
    }
}
