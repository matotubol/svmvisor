//! Preparation-record diagnostics: the stage/failure recorder shims and the stable
//! error-to-code tables they carry.
use svmvisor_launcher::native::{admission::memory, resident};
use uefi_raw::Status;

#[cfg(feature = "native-resident-boot")]
use super::card_boot;

pub(super) fn preparation_map_failure(error: memory::MemoryMapError) -> Status {
    use memory::MemoryMapError::*;
    let (reason, status) = match error {
        Firmware(status) => (16, status.0 as u64),
        Bounds => (17, 0),
        Layout => (18, 0),
        RetryLimit => (19, 0),
        Cleanup(status) => (20, status.0 as u64),
        Released => (21, 0),
    };
    preparation_failure(reason, status, 0);
    Status::OUT_OF_RESOURCES
}

// Preparation diagnostics belong to the serialized BSP installer only. Other
// build profiles retain their existing behavior and do not access the card.
pub(super) fn preparation_step(stage: u32, address: u64) {
    #[cfg(feature = "native-resident-boot")]
    card_boot::preparation_step(stage, address);
    #[cfg(not(feature = "native-resident-boot"))]
    let _ = (stage, address);
}

pub(super) fn preparation_failure(reason: u32, status: u64, address: u64) {
    #[cfg(feature = "native-resident-boot")]
    card_boot::preparation_failure(reason, status, address);
    #[cfg(not(feature = "native-resident-boot"))]
    let _ = (reason, status, address);
}

// Stable numeric codes of the stage-16/17 refusals that the preparation record
// (reasons 34-36) and the admission-hint recorder (predicates 635-637) carry;
// read_snapshot.py names them. Enum order is not wire format; this table is.
pub(super) fn address_error_code(error: svmvisor_hypervisor::memory::address::AddressError) -> u64 {
    use svmvisor_hypervisor::memory::address::AddressError::*;
    match error {
        UnsupportedPhysicalWidth => 1,
        UnknownEncryption => 2,
        ActiveEncryptionUnsupported => 3,
        InvalidEncryptionBit => 4,
        EmptyRange => 5,
        InvalidAlignment => 6,
        Misaligned => 7,
        Overflow => 8,
        OutsidePhysicalWidth => 9,
        EncryptionBitEncoded => 10,
    }
}

pub(super) fn identity_npt_error_code(
    error: svmvisor_hypervisor::memory::npt::IdentityNptError,
) -> u64 {
    use svmvisor_hypervisor::memory::npt::IdentityNptError::*;
    match error {
        RequiredModeNotEstablished => 1,
        OneGiBPagesNotEstablished => 2,
        PatZeroNotWriteBack => 3,
        InvalidExclusion => 4,
        TableArenaOutsideExclusion => 5,
        GuestAddressOutsideWidth => 6,
        StorageBounds => 7,
        Address(error) => 16 + address_error_code(error),
    }
}

pub(super) fn resident_memory_error_code(error: resident::memory::ResidentMemoryError) -> u64 {
    use resident::memory::ResidentMemoryError::*;
    use svmvisor_hypervisor::boot::memory::MemoryError as M;
    match error {
        MonitorUncovered => 1,
        MonitorNotRuntime => 2,
        MonitorNotWriteBack => 3,
        Map(error) => {
            32 + match error {
                M::DescriptorCount => 1,
                M::PhysicalWidth => 2,
                M::EmptyDescriptor => 3,
                M::MisalignedDescriptor => 4,
                M::Overflow => 5,
                M::OutsidePhysicalWidth => 6,
                M::UnsortedOrOverlapping => 7,
                M::EmptyRange => 8,
                M::MisalignedEntry => 9,
                M::CopyTooLarge => 10,
                M::UncoveredRange => 11,
                M::UntrustedMemoryType => 12,
                M::ReadProtected => 13,
                M::MissingWriteBackCapability => 14,
                M::ReadFailed => 15,
                M::MonitorOverlap => 16,
            }
        }
        Npt(error) => 64 + identity_npt_error_code(error),
    }
}
