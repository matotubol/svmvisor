//! Native resident memory-map admission and identity NPT preparation.
//!
//! UEFI 2.11 7.2.1 and Table 7.10: the delivery runtime driver must acquire
//! legal RuntimeServicesCode/Data storage before ReadyToBoot. AllocatePages
//! does not permit drivers to request EfiReservedMemoryType. A map record is
//! not evidence that allocation happened, that pages are currently accessible,
//! or that a raw payload is exempt from loaded-image virtual-address fixups.
//! Those facts and effective WB caching remain the firmware adapter's duties.
use svmvisor_hypervisor::{
    arch::x86_64::capabilities::EvidenceFlag,
    boot::memory::{MemoryDescriptor, MemoryError, ValidatedMemoryMap},
    memory::{
        address::{AddressPolicy, PhysicalRange},
        npt::{IdentityNpt, IdentityNptError, NptEvidence, TableStorage},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentMemoryError {
    Map(MemoryError),
    MonitorUncovered,
    MonitorNotRuntime,
    MonitorNotWriteBack,
    Npt(IdentityNptError),
}

/// Check actual supplied map records for continuous retained WB-capable runtime
/// coverage. This metadata check neither allocates pages nor proves current
/// access permissions or effective cache type. Allocation and NPT callers use
/// the same admission rule.
pub fn validate_runtime_coverage(
    monitor: PhysicalRange,
    descriptors: &[MemoryDescriptor],
    physical_bits: u8,
) -> Result<(), ResidentMemoryError> {
    use ResidentMemoryError as E;
    let _map = ValidatedMemoryMap::new(descriptors, physical_bits.min(40)).map_err(E::Map)?;
    let end = monitor.last_byte() + 1;
    let mut covered = monitor.base();
    for d in descriptors {
        let descriptor_end = d.physical_start + d.page_count * 4096;
        if descriptor_end <= covered || d.physical_start >= end {
            continue;
        }
        if d.physical_start > covered {
            return Err(E::MonitorUncovered);
        }
        // EFI_RUNTIME_SERVICES_CODE=5 / DATA=6, EFI_MEMORY_RUNTIME=bit63.
        if !matches!(d.memory_type, 5 | 6) || d.attributes & (1 << 63) == 0 {
            return Err(E::MonitorNotRuntime);
        }
        if d.attributes & 8 == 0 {
            return Err(E::MonitorNotWriteBack);
        }
        covered = descriptor_end.min(end);
    }
    if covered != end {
        return Err(E::MonitorUncovered);
    }
    Ok(())
}

/// Validate the complete sorted map against the bounded physical aperture and
/// require continuous, OS-retained runtime coverage of the monitor. Build only
/// after every metadata check passes. Refusal leaves all NPT storage unchanged.
/// The table storage and every persistent host object must be inside
/// `monitor`; the NPT builder checks table containment. This trusted first-boot
/// map lets the guest use existing RAM/devices through its original CR3. It is
/// not a general device model or a DMA-isolation claim. MMIO absent from the
/// firmware map still needs platform admission; an out-of-aperture NPF stops.
pub fn prepare_identity_npt<'a>(
    storage: &'a mut TableStorage,
    table_base: u64,
    policy: AddressPolicy,
    monitor: PhysicalRange,
    descriptors: &[MemoryDescriptor],
    evidence: NptEvidence,
    one_gib_pages: EvidenceFlag,
    source_pat: u64,
) -> Result<IdentityNpt<'a>, ResidentMemoryError> {
    use ResidentMemoryError as E;
    validate_runtime_coverage(monitor, descriptors, policy.physical_bits())?;
    IdentityNpt::new(storage, table_base, policy, monitor, evidence, one_gib_pages, source_pat)
        .map_err(E::Npt)
}
