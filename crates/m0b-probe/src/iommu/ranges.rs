//! Pure range and UEFI memory-map checks for read-only IOMMU inspection.
//!
//! This module never maps or dereferences an address.  Callers supply neutral
//! copies of the relevant memory descriptors and receive an auditable witness
//! identifying the one descriptor that contains a proposed access.

pub const EFI_MEMORY_MAPPED_IO: u32 = 11;
pub const EFI_MEMORY_UC: u64 = 1 << 0;
pub const EFI_MEMORY_RP: u64 = 1 << 13;
pub const EFI_PAGE_SIZE: u64 = 4096;
pub const IOMMU_MINIMUM_APERTURE_LENGTH: u64 = 16 * 1024;
pub const IOMMU_PERFORMANCE_COUNTER_APERTURE_LENGTH: u64 = 512 * 1024;
pub const MAX_MEMORY_DESCRIPTORS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl PhysicalRange {
    #[must_use]
    pub const fn length(self) -> u64 {
        self.end_exclusive.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end_exclusive <= self.end_exclusive
    }
}

/// Firmware-neutral subset of one UEFI memory descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryDescriptor {
    pub memory_type: u32,
    pub physical_start: u64,
    pub page_count: u64,
    pub attributes: u64,
}

/// Evidence that one and only one acceptable descriptor covered a range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorWitness {
    pub descriptor_index: usize,
    pub descriptor: MemoryDescriptor,
    pub descriptor_range: PhysicalRange,
    pub requested_range: PhysicalRange,
}

/// An address and length decoded from a configuration register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfiguredRange {
    pub base: u64,
    pub length: u64,
    pub required_alignment: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeError {
    InvalidPhysicalAddressWidth { width: u8 },
    ZeroLength,
    ReversedRange { start: u64, end_exclusive: u64 },
    ArithmeticOverflow,
    AddressExceedsPhysicalWidth { width: u8, range: PhysicalRange },
    InvalidAlignment { alignment: u64 },
    MisalignedAddress { address: u64, alignment: u64 },
    UnconfiguredEnabledRange,
    ZeroDescriptorPages { descriptor_index: usize },
    DescriptorRangeOverflow { descriptor_index: usize },
    DescriptorLimitExceeded { count: usize, limit: usize },
    NoCoveringDescriptor,
    MultipleCoveringDescriptors { count: usize },
    WrongMemoryType { descriptor_index: usize, found: u32 },
    MissingUncacheableAttribute { descriptor_index: usize },
    ReadProtected { descriptor_index: usize },
}

/// Form a non-empty half-open range and ensure every byte is representable by
/// the stated architectural physical-address width.
pub fn checked_physical_range(
    start: u64,
    length: u64,
    physical_address_width: u8,
) -> Result<PhysicalRange, RangeError> {
    if !(1..=64).contains(&physical_address_width) {
        return Err(RangeError::InvalidPhysicalAddressWidth {
            width: physical_address_width,
        });
    }
    if length == 0 {
        return Err(RangeError::ZeroLength);
    }
    let end_exclusive = start
        .checked_add(length)
        .ok_or(RangeError::ArithmeticOverflow)?;
    let range = PhysicalRange {
        start,
        end_exclusive,
    };
    if physical_address_width < 64 && end_exclusive > (1_u64 << u32::from(physical_address_width)) {
        return Err(RangeError::AddressExceedsPhysicalWidth {
            width: physical_address_width,
            range,
        });
    }
    Ok(range)
}

/// Validate a range already decoded from a device-table or log-base register.
/// The stricter of the CPU and IOMMU physical-address widths is authoritative.
pub fn validate_configured_range(
    configured: ConfiguredRange,
    iommu_physical_address_width: u8,
    cpu_physical_address_width: u8,
) -> Result<PhysicalRange, RangeError> {
    validate_alignment(configured.base, configured.required_alignment)?;
    validate_width(iommu_physical_address_width)?;
    validate_width(cpu_physical_address_width)?;
    checked_physical_range(
        configured.base,
        configured.length,
        iommu_physical_address_width.min(cpu_physical_address_width),
    )
}

/// Validate an inherited configured range without treating a disabled, zero
/// base register as a physical pointer. A nonzero disabled value is still
/// checked and returned because it is useful record-only evidence.
pub fn validate_enabled_configured_range(
    enabled: bool,
    configured: ConfiguredRange,
    iommu_physical_address_width: u8,
    cpu_physical_address_width: u8,
) -> Result<Option<PhysicalRange>, RangeError> {
    if configured.base == 0 {
        return if enabled {
            Err(RangeError::UnconfiguredEnabledRange)
        } else {
            Ok(None)
        };
    }
    validate_configured_range(
        configured,
        iommu_physical_address_width,
        cpu_physical_address_width,
    )
    .map(Some)
}

/// Require exactly one same-run UEFI memory descriptor to contain the complete
/// requested range. This generic witness makes no memory-type or attribute
/// claim and does not map or dereference the described memory.
pub fn memory_descriptor_witness(
    descriptors: &[MemoryDescriptor],
    requested_range: PhysicalRange,
    physical_address_width: u8,
) -> Result<DescriptorWitness, RangeError> {
    if descriptors.len() > MAX_MEMORY_DESCRIPTORS {
        return Err(RangeError::DescriptorLimitExceeded {
            count: descriptors.len(),
            limit: MAX_MEMORY_DESCRIPTORS,
        });
    }
    let requested_length = requested_range
        .end_exclusive
        .checked_sub(requested_range.start)
        .ok_or(RangeError::ReversedRange {
            start: requested_range.start,
            end_exclusive: requested_range.end_exclusive,
        })?;
    let requested_range = checked_physical_range(
        requested_range.start,
        requested_length,
        physical_address_width,
    )?;

    let mut match_index = None;
    let mut match_range = None;
    let mut match_count = 0_usize;
    for (index, descriptor) in descriptors.iter().copied().enumerate() {
        let descriptor_range = descriptor_physical_range(index, descriptor)?;
        if descriptor_range.contains(requested_range) {
            match_count = match_count
                .checked_add(1)
                .ok_or(RangeError::ArithmeticOverflow)?;
            if match_index.is_none() {
                match_index = Some(index);
                match_range = Some(descriptor_range);
            }
        }
    }

    if match_count == 0 {
        return Err(RangeError::NoCoveringDescriptor);
    }
    if match_count != 1 {
        return Err(RangeError::MultipleCoveringDescriptors { count: match_count });
    }
    let descriptor_index = match_index.ok_or(RangeError::NoCoveringDescriptor)?;
    let descriptor = descriptors[descriptor_index];

    Ok(DescriptorWitness {
        descriptor_index,
        descriptor,
        descriptor_range: match_range.ok_or(RangeError::NoCoveringDescriptor)?,
        requested_range,
    })
}

/// Require exactly one UEFI `EfiMemoryMappedIO` descriptor with UC set and RP
/// clear to contain the complete requested range.
pub fn memory_mapped_io_witness(
    descriptors: &[MemoryDescriptor],
    requested_range: PhysicalRange,
    physical_address_width: u8,
) -> Result<DescriptorWitness, RangeError> {
    let witness = memory_descriptor_witness(descriptors, requested_range, physical_address_width)?;
    let descriptor_index = witness.descriptor_index;
    let descriptor = witness.descriptor;
    if descriptor.memory_type != EFI_MEMORY_MAPPED_IO {
        return Err(RangeError::WrongMemoryType {
            descriptor_index,
            found: descriptor.memory_type,
        });
    }
    if descriptor.attributes & EFI_MEMORY_UC == 0 {
        return Err(RangeError::MissingUncacheableAttribute { descriptor_index });
    }
    if descriptor.attributes & EFI_MEMORY_RP != 0 {
        return Err(RangeError::ReadProtected { descriptor_index });
    }

    Ok(witness)
}

/// Validate a derived ECAM function or capability witness. This computes no
/// ECAM address; the caller supplies the already checked witness range.
pub fn ecam_descriptor_witness(
    descriptors: &[MemoryDescriptor],
    ecam_range: PhysicalRange,
    physical_address_width: u8,
) -> Result<DescriptorWitness, RangeError> {
    memory_mapped_io_witness(descriptors, ecam_range, physical_address_width)
}

#[must_use]
pub const fn iommu_aperture_length(performance_counters_supported: bool) -> u64 {
    if performance_counters_supported {
        IOMMU_PERFORMANCE_COUNTER_APERTURE_LENGTH
    } else {
        IOMMU_MINIMUM_APERTURE_LENGTH
    }
}

/// Validate alignment and complete descriptor coverage for the architecturally
/// required IOMMU register aperture.
pub fn iommu_aperture_witness(
    descriptors: &[MemoryDescriptor],
    mmio_base: u64,
    performance_counters_supported: bool,
    physical_address_width: u8,
) -> Result<DescriptorWitness, RangeError> {
    let length = iommu_aperture_length(performance_counters_supported);
    validate_alignment(mmio_base, length)?;
    let range = checked_physical_range(mmio_base, length, physical_address_width)?;
    memory_mapped_io_witness(descriptors, range, physical_address_width)
}

fn validate_width(width: u8) -> Result<(), RangeError> {
    if (1..=64).contains(&width) {
        Ok(())
    } else {
        Err(RangeError::InvalidPhysicalAddressWidth { width })
    }
}

fn validate_alignment(address: u64, alignment: u64) -> Result<(), RangeError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(RangeError::InvalidAlignment { alignment });
    }
    if address & (alignment - 1) != 0 {
        return Err(RangeError::MisalignedAddress { address, alignment });
    }
    Ok(())
}

fn descriptor_physical_range(
    descriptor_index: usize,
    descriptor: MemoryDescriptor,
) -> Result<PhysicalRange, RangeError> {
    if descriptor.page_count == 0 {
        return Err(RangeError::ZeroDescriptorPages { descriptor_index });
    }
    let length = descriptor
        .page_count
        .checked_mul(EFI_PAGE_SIZE)
        .ok_or(RangeError::DescriptorRangeOverflow { descriptor_index })?;
    let end_exclusive = descriptor
        .physical_start
        .checked_add(length)
        .ok_or(RangeError::DescriptorRangeOverflow { descriptor_index })?;
    Ok(PhysicalRange {
        start: descriptor.physical_start,
        end_exclusive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn descriptor(start: u64, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor {
            memory_type: EFI_MEMORY_MAPPED_IO,
            physical_start: start,
            page_count: pages,
            attributes: EFI_MEMORY_UC,
        }
    }

    #[test]
    fn checked_ranges_reject_zero_overflow_and_physical_width_crossing() {
        assert_eq!(
            checked_physical_range(0x1000, 0x2000, 48).unwrap(),
            PhysicalRange {
                start: 0x1000,
                end_exclusive: 0x3000,
            }
        );
        assert_eq!(
            checked_physical_range(0, 1, 0),
            Err(RangeError::InvalidPhysicalAddressWidth { width: 0 })
        );
        assert_eq!(
            checked_physical_range(0, 0, 48),
            Err(RangeError::ZeroLength)
        );
        assert_eq!(
            checked_physical_range(u64::MAX, 1, 64),
            Err(RangeError::ArithmeticOverflow)
        );
        assert!(matches!(
            checked_physical_range((1_u64 << 40) - 0x1000, 0x2000, 40),
            Err(RangeError::AddressExceedsPhysicalWidth { width: 40, .. })
        ));
    }

    #[test]
    fn exactly_one_acceptable_descriptor_produces_a_witness() {
        let descriptors = [descriptor(0xe000_0000, 1), descriptor(0xf760_0000, 128)];
        let requested = PhysicalRange {
            start: 0xf760_0040,
            end_exclusive: 0xf760_0058,
        };
        let witness = memory_mapped_io_witness(&descriptors, requested, 48).unwrap();
        assert_eq!(witness.descriptor_index, 1);
        assert_eq!(witness.requested_range, requested);
        assert_eq!(witness.descriptor_range.length(), 512 * 1024);
    }

    #[test]
    fn generic_descriptor_witness_retains_ram_without_mmio_claims() {
        let descriptor = MemoryDescriptor {
            memory_type: 7,
            physical_start: 0x4000,
            page_count: 4,
            attributes: 0,
        };
        let requested = PhysicalRange {
            start: 0x5000,
            end_exclusive: 0x7000,
        };
        let witness = memory_descriptor_witness(&[descriptor], requested, 48).unwrap();
        assert_eq!(witness.descriptor, descriptor);
        assert_eq!(witness.requested_range, requested);
        assert!(matches!(
            memory_descriptor_witness(&[descriptor, descriptor], requested, 48),
            Err(RangeError::MultipleCoveringDescriptors { count: 2 })
        ));
        assert!(matches!(
            memory_descriptor_witness(
                &[descriptor],
                PhysicalRange {
                    start: 0x5000,
                    end_exclusive: 0x9000,
                },
                48,
            ),
            Err(RangeError::NoCoveringDescriptor)
        ));
    }

    #[test]
    fn missing_and_overlapping_descriptor_coverage_are_rejected() {
        let requested = PhysicalRange {
            start: 0xf760_0000,
            end_exclusive: 0xf760_4000,
        };
        assert_eq!(
            memory_mapped_io_witness(&[], requested, 48),
            Err(RangeError::NoCoveringDescriptor)
        );
        let duplicate = [descriptor(0xf760_0000, 4), descriptor(0xf760_0000, 128)];
        assert_eq!(
            memory_mapped_io_witness(&duplicate, requested, 48),
            Err(RangeError::MultipleCoveringDescriptors { count: 2 })
        );
        assert!(matches!(
            memory_mapped_io_witness(
                &duplicate,
                PhysicalRange {
                    start: 2,
                    end_exclusive: 1,
                },
                48
            ),
            Err(RangeError::ReversedRange { .. })
        ));
    }

    #[test]
    fn descriptor_type_uc_and_rp_policy_is_strict() {
        let range = PhysicalRange {
            start: 0x1000,
            end_exclusive: 0x2000,
        };
        let mut candidate = descriptor(0x1000, 1);
        candidate.memory_type = 10;
        assert!(matches!(
            memory_mapped_io_witness(&[candidate], range, 48),
            Err(RangeError::WrongMemoryType { found: 10, .. })
        ));
        candidate.memory_type = EFI_MEMORY_MAPPED_IO;
        candidate.attributes = 0;
        assert!(matches!(
            memory_mapped_io_witness(&[candidate], range, 48),
            Err(RangeError::MissingUncacheableAttribute { .. })
        ));
        candidate.attributes = EFI_MEMORY_UC | EFI_MEMORY_RP;
        assert!(matches!(
            memory_mapped_io_witness(&[candidate], range, 48),
            Err(RangeError::ReadProtected { .. })
        ));
    }

    #[test]
    fn malformed_descriptors_are_rejected_without_wrapping() {
        let range = PhysicalRange {
            start: 0x1000,
            end_exclusive: 0x2000,
        };
        assert_eq!(
            memory_mapped_io_witness(&[descriptor(0x1000, 0)], range, 48),
            Err(RangeError::ZeroDescriptorPages {
                descriptor_index: 0,
            })
        );
        assert_eq!(
            memory_mapped_io_witness(&[descriptor(u64::MAX - 0xfff, 2)], range, 48),
            Err(RangeError::DescriptorRangeOverflow {
                descriptor_index: 0,
            })
        );
        let too_many = vec![descriptor(0, 1); MAX_MEMORY_DESCRIPTORS + 1];
        assert!(matches!(
            memory_mapped_io_witness(&too_many, range, 48),
            Err(RangeError::DescriptorLimitExceeded { .. })
        ));
    }

    #[test]
    fn iommu_aperture_uses_16k_or_complete_aligned_512k() {
        let descriptors = [descriptor(0xf760_0000, 128)];
        let small = iommu_aperture_witness(&descriptors, 0xf760_0000, false, 48).unwrap();
        assert_eq!(small.requested_range.length(), 16 * 1024);
        let large = iommu_aperture_witness(&descriptors, 0xf760_0000, true, 48).unwrap();
        assert_eq!(large.requested_range.length(), 512 * 1024);

        assert!(matches!(
            iommu_aperture_witness(&descriptors, 0xf760_4000, true, 48),
            Err(RangeError::MisalignedAddress { alignment, .. })
                if alignment == 512 * 1024
        ));
        let short = [descriptor(0xf760_0000, 127)];
        assert_eq!(
            iommu_aperture_witness(&short, 0xf760_0000, true, 48),
            Err(RangeError::NoCoveringDescriptor)
        );
    }

    #[test]
    fn ecam_witness_uses_the_same_readable_mmio_policy() {
        let descriptors = [descriptor(0xe000_0000, 256)];
        let ecam = PhysicalRange {
            start: 0xe000_2040,
            end_exclusive: 0xe000_2058,
        };
        assert_eq!(
            ecam_descriptor_witness(&descriptors, ecam, 48)
                .unwrap()
                .requested_range,
            ecam
        );
    }

    #[test]
    fn decoded_configured_ranges_use_both_widths_and_do_not_dereference() {
        let configured = ConfiguredRange {
            base: 0x1234_5000,
            length: 0x4000,
            required_alignment: 0x1000,
        };
        assert_eq!(
            validate_configured_range(configured, 48, 52).unwrap(),
            PhysicalRange {
                start: 0x1234_5000,
                end_exclusive: 0x1234_9000,
            }
        );
        assert!(matches!(
            validate_configured_range(
                ConfiguredRange {
                    base: 0x1234_5001,
                    ..configured
                },
                48,
                52
            ),
            Err(RangeError::MisalignedAddress { .. })
        ));
        assert!(matches!(
            validate_configured_range(
                ConfiguredRange {
                    base: (1_u64 << 40) - 0x1000,
                    length: 0x2000,
                    required_alignment: 0x1000,
                },
                40,
                52
            ),
            Err(RangeError::AddressExceedsPhysicalWidth { width: 40, .. })
        ));
    }

    #[test]
    fn disabled_zero_ranges_are_absent_but_enabled_zero_ranges_block() {
        let zero = ConfiguredRange {
            base: 0,
            length: 0x1000,
            required_alignment: 0x1000,
        };
        assert_eq!(
            validate_enabled_configured_range(false, zero, 48, 48),
            Ok(None)
        );
        assert_eq!(
            validate_enabled_configured_range(true, zero, 48, 48),
            Err(RangeError::UnconfiguredEnabledRange)
        );
    }
}
