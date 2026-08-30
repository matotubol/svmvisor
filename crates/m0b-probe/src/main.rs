//! Removable-media UEFI adapter for the M0b record-only inventory slice.
//!
//! Application-issued measurements are read-only. UEFI MP Services owns AP
//! wake/termination, so schema v6 records post-dispatch state and does not claim
//! that firmware preserved pre-dispatch control state. Its live AMD-IOMMU slice
//! uses only bounded UEFI PCI/MMIO reads derived from the same-run IVRS, and its
//! system-register slice uses only the allowlisted `RDMSR` sites from
//! `docs/m0b-msr-read-policy.md`.

#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#![deny(unsafe_code)]
#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("svmvisor-m0b-probe is a UEFI-only application; use cargo build-m0b-probe");
}

#[cfg(target_os = "uefi")]
mod firmware {
    extern crate alloc;

    mod iommu_live;

    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use core::arch::asm;
    use core::arch::x86_64::__cpuid_count;
    use core::cell::UnsafeCell;
    use core::convert::TryFrom;
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
    use core::time::Duration;
    use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
    use uefi::mem::memory_map::{MemoryAttribute, MemoryMap, MemoryType};
    use uefi::prelude::{Status, entry};
    use uefi::proto::ProtocolPointer;
    use uefi::proto::device_path::DevicePath;
    use uefi::proto::loaded_image::LoadedImage;
    use uefi::proto::media::block::BlockIO;
    use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, RegularFile};
    use uefi::proto::media::fs::SimpleFileSystem;
    use uefi::proto::pi::mp::MpServices;
    use uefi::table::cfg::ConfigTableEntry;
    use uefi::{CStr16, CString16, Handle, runtime, system};

    use svmvisor_m0b_probe::acpi::{
        AcpiError, AcpiLimits, MCFG_FIXED_LENGTH, MadtMpTopologyCrossCheck, RootTable,
        RootTableKind, Rsdp, SDT_HEADER_LENGTH, WhitelistedTable, WhitelistedTableKind,
        cross_check_madt_with_mp_services, inspect_sdt_prefix, parse_whitelisted_table,
        rsdp_declared_length,
    };
    use svmvisor_m0b_probe::evidence::{
        AcpiDirectoryRecord, AcpiEvidence, AcpiRootEvidence, AcpiRootReferences,
        AcpiSdtHeaderEvidence, AcpiTableEvidence, AcpiTablesEvidence, AmdIommuLiveEvidence,
        AmdIommuMmioEvidence, FadtBodyEvidence, IommuAccessEvidence, IommuConfiguredRangeEvidence,
        IommuLocatorEvidence, IommuMmioCommonEvidence, IommuOwnershipEvidence, IommuPciEvidence,
        IvhdDeviceEntryEvidence, IvhdEvidence, IvmdEvidence, IvrsBlockEvidence, IvrsBodyEvidence,
        MadtBodyEvidence, MadtEntryEvidence, MadtMpCrossCheckEvidence, McfgAllocationEvidence,
        McfgBodyEvidence, MemoryDescriptorBindingEvidence, PROFILE_BINDING_FILE,
        PROFILE_BINDING_PREFIX, RawBytesEvidence, RsdpEvidence,
    };
    use svmvisor_m0b_probe::{
        AP_MEASUREMENT_TIMEOUT_POLICY, ConfigTableRecord, CpuidRegisters, CpuidSource, Evidence,
        MemoryDescriptorRecord, MemoryMapEvidence, MpServicesEvidence, MsrAccessEvidence,
        ProcessorConsistencyEvidence, ProcessorDispatch, ProcessorObservation, ProcessorRecord,
        SinkRecord, SystemRegistersSection, TimestampRecord, VmCrEvidence, collect_cpuid,
        parse_profile_binding, render_json, should_read_vm_cr,
    };
    use svmvisor_m0b_probe::msr::{
        self, SystemRegisterInventory, SystemRegistersEvidence, VariableMtrrPair,
    };

    const VM_CR_MSR: u32 = 0xc001_0114;
    const PROFILE_BINDING_LENGTH: usize = PROFILE_BINDING_PREFIX.len() + 64 + 1;
    const MAX_CONFIGURATION_TABLES: usize = 64;
    const MAX_UNIQUE_ROOT_POINTERS: usize = 256;
    const MAX_TOTAL_ACPI_BYTES: usize = 4 * 1024 * 1024;
    const MAX_MEMORY_DESCRIPTORS: usize = 4 * 1024;
    const MAX_MP_PROCESSORS: usize = 256;
    const UEFI_PAGE_SIZE: u64 = 4 * 1024;
    const ACPI_RECLAIM_MEMORY_TYPE: u32 = 9;
    const ACPI_NON_VOLATILE_MEMORY_TYPE: u32 = 10;
    const ACPI2_GUID: &str = "8868e871-e4f1-11d3-bc22-0080c73c8881";
    const FAILURE_SCREEN_SECONDS: u64 = 20;

    struct OwnedConfigTable {
        kind: &'static str,
        guid: String,
        address: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RootReference {
        Rsdt,
        Xsdt,
    }

    struct CapturedRoot {
        address: u64,
        raw: Vec<u8>,
        entries: Vec<u64>,
    }

    struct CapturedDirectoryEntry {
        address: u64,
        referenced_by: Vec<RootReference>,
        header_raw: Vec<u8>,
        signature: [u8; 4],
        declared_length: usize,
        revision: u8,
        table_raw: Option<Vec<u8>>,
    }

    struct CapturedAcpi {
        rsdp_address: u64,
        rsdp_raw: Vec<u8>,
        rsdt: CapturedRoot,
        xsdt: CapturedRoot,
        directory: Vec<CapturedDirectoryEntry>,
        topology: MadtMpTopologyCrossCheck,
    }

    struct PhysicalAcpiReader<'a> {
        memory_descriptors: &'a [MemoryDescriptorRecord],
        physical_address_bits: u8,
        copied_bytes: usize,
    }

    struct AcpiErrorDetail {
        source: &'static str,
        error: AcpiError,
    }

    struct ProbeError {
        status: Status,
        message: &'static str,
        acpi_detail: Option<AcpiErrorDetail>,
    }

    impl ProbeError {
        const fn new(status: Status, message: &'static str) -> Self {
            Self {
                status,
                message,
                acpi_detail: None,
            }
        }

        fn with_acpi_detail(
            status: Status,
            message: &'static str,
            source: &'static str,
            error: AcpiError,
        ) -> Self {
            Self {
                status,
                message,
                acpi_detail: Some(AcpiErrorDetail { source, error }),
            }
        }
    }

    const fn whitelisted_table_label(kind: WhitelistedTableKind) -> &'static str {
        match kind {
            WhitelistedTableKind::Madt => "MADT/APIC",
            WhitelistedTableKind::Mcfg => "MCFG",
            WhitelistedTableKind::Ivrs => "IVRS",
            WhitelistedTableKind::Fadt => "FADT/FACP",
        }
    }

    const fn root_table_label(kind: RootTableKind) -> &'static str {
        match kind {
            RootTableKind::Rsdt => "RSDT",
            RootTableKind::Xsdt => "XSDT",
        }
    }

    struct AuthorizedSink {
        record: SinkRecord,
        file_system_handle: Handle,
    }

    #[repr(align(8))]
    struct FileInfoStorage([u8; 512]);

    struct HardwareCpuid;

    impl CpuidSource for HardwareCpuid {
        fn cpuid(&mut self, leaf: u32, subleaf: u32) -> CpuidRegisters {
            // CPUID is mandatory in x86_64 mode. `collect_cpuid` bounds all optional
            // queries by the maximum basic/extended leaves first.
            let result = __cpuid_count(leaf, subleaf);
            CpuidRegisters {
                eax: result.eax,
                ebx: result.ebx,
                ecx: result.ecx,
                edx: result.edx,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct CurrentProcessorMeasurement {
        cpu: svmvisor_m0b_probe::CpuInventory,
        vm_cr: VmCrEvidence<'static>,
        system_registers: SystemRegistersEvidence<'static>,
    }

    #[derive(Clone, Copy)]
    struct ApMeasurementResult {
        reported_processor_number: Option<usize>,
        who_am_i_status: Option<usize>,
        measurement: CurrentProcessorMeasurement,
    }

    /// One private result slot used by exactly one blocking StartupThisAP call.
    ///
    /// The BSP initializes the slot and does not inspect `result` until the
    /// firmware call has returned and the AP has published completion with a
    /// release store. The AP never allocates and never retains this pointer.
    struct ApMeasurementSlot {
        mp_services: *const MpServices,
        result: UnsafeCell<MaybeUninit<ApMeasurementResult>>,
        completion: AtomicU8,
    }

    impl ApMeasurementSlot {
        const fn new(mp_services: *const MpServices) -> Self {
            Self {
                mp_services,
                result: UnsafeCell::new(MaybeUninit::uninit()),
                completion: AtomicU8::new(0),
            }
        }
    }

    struct ProcessorInventoryCollection {
        total: usize,
        enabled: usize,
        bsp_processor_number: usize,
        records: Vec<ProcessorRecord>,
        observations: Vec<ProcessorObservation>,
    }

    #[entry]
    fn main() -> Status {
        if let Err(error) = uefi::helpers::init() {
            return error.status();
        }

        uefi::println!("svmvisor M0b inventory: STARTING");
        match run_probe() {
            Ok(output_file) => {
                uefi::println!("svmvisor M0b inventory: CAPTURED");
                uefi::println!("QUALIFICATION REMAINS BLOCKED");
                uefi::println!("Evidence: {}", output_file);
                Status::SUCCESS
            }
            Err(error) => {
                uefi::println!("svmvisor M0b inventory: FAILED");
                uefi::println!("{} ({})", error.message, error.status);
                if let Some(detail) = &error.acpi_detail {
                    uefi::println!("ACPI detail [{}]: {:?}", detail.source, detail.error);
                }
                uefi::println!(
                    "Returning to firmware in {} seconds.",
                    FAILURE_SCREEN_SECONDS
                );
                boot::stall(Duration::from_secs(FAILURE_SCREEN_SECONDS));
                error.status
            }
        }
    }

    fn run_probe() -> Result<String, ProbeError> {
        let authorized_sink = inspect_and_authorize_image_volume()?;
        let binding_bytes =
            read_profile_binding(authorized_sink.file_system_handle, authorized_sink.record)?;
        let target_profile_manifest_sha256 =
            parse_profile_binding(&binding_bytes).map_err(|_| {
                ProbeError::new(
                    Status::SECURITY_VIOLATION,
                    "the M0a profile binding marker is not canonical schema v1",
                )
            })?;

        let collected_at = collect_timestamp()?;
        let output_file = format!(
            "\\svmvisor-m0b-{:04}{:02}{:02}T{:02}{:02}{:02}-{:09}.json",
            collected_at.year,
            collected_at.month,
            collected_at.day,
            collected_at.hour,
            collected_at.minute,
            collected_at.second,
            collected_at.nanosecond,
        );
        let output_path = CString16::try_from(output_file.as_str()).map_err(|_| {
            ProbeError::new(
                Status::INVALID_PARAMETER,
                "the evidence output path is invalid",
            )
        })?;
        let partial_file = format!("{output_file}.partial");
        let partial_path = CString16::try_from(partial_file.as_str()).map_err(|_| {
            ProbeError::new(
                Status::INVALID_PARAMETER,
                "the temporary evidence output path is invalid",
            )
        })?;
        ensure_output_absent(
            authorized_sink.file_system_handle,
            authorized_sink.record,
            output_path.as_ref(),
        )?;
        ensure_output_absent(
            authorized_sink.file_system_handle,
            authorized_sink.record,
            partial_path.as_ref(),
        )?;

        let firmware_vendor = system::firmware_vendor().to_string();
        let firmware_revision = system::firmware_revision();
        let uefi_revision = system::uefi_revision();

        let owned_config_tables = collect_configuration_tables()?;
        let config_tables = owned_config_tables
            .iter()
            .map(|table| ConfigTableRecord {
                kind: table.kind,
                guid: table.guid.as_str(),
                address: table.address,
            })
            .collect::<Vec<_>>();

        let processor_inventory = collect_processor_inventory()?;
        let processor_total = processor_inventory.total;
        let processor_enabled = processor_inventory.enabled;
        let processor_records = &processor_inventory.records;
        let bsp_observation = processor_inventory
            .observations
            .iter()
            .find(|observation| {
                observation.is_bsp
                    && observation.processor_number == processor_inventory.bsp_processor_number
            })
            .copied()
            .ok_or_else(|| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the complete processor inventory has no canonical BSP observation",
                )
            })?;
        let cpu = bsp_observation.cpu;
        let vm_cr = bsp_observation.vm_cr;
        // The processor inventory contains every RDMSR of the run: the v4
        // live-IOMMU slice below performs PCI/MMIO reads only. The counters
        // are therefore final before the evidence record is assembled.
        let system_registers = SystemRegistersSection {
            bsp: bsp_observation.system_registers,
            access: MsrAccessEvidence {
                read_operations: MSR_READ_OPERATIONS.load(Ordering::Relaxed),
                read_bytes: MSR_READ_BYTES.load(Ordering::Relaxed),
                write_operations: 0,
            },
        };
        let processor_consistency = ProcessorConsistencyEvidence {
            bsp_processor_number: processor_inventory.bsp_processor_number,
            timeout_microseconds_per_ap: AP_MEASUREMENT_TIMEOUT_POLICY.timeout_microseconds(),
            observations: &processor_inventory.observations,
        };
        if processor_inventory.observations.len() != processor_enabled {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "the enabled-processor observation set is incomplete",
            ));
        }
        if processor_total != processor_records.len() {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services did not return canonical processor counts",
            ));
        }
        let mp_services = MpServicesEvidence::Observed {
            total: processor_total,
            enabled: processor_enabled,
            processors: processor_records,
        };
        if processor_enabled == 0 {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services reported no enabled processors",
            ));
        }

        let (memory_descriptors, memory_meta, memory_error) = collect_memory_map();
        if memory_error.is_some() {
            return Err(ProbeError::new(
                Status::UNSUPPORTED,
                "the UEFI memory map is required to bound ACPI physical reads",
            ));
        }
        let (descriptor_size, descriptor_version) = memory_meta.ok_or_else(|| {
            ProbeError::new(
                Status::ABORTED,
                "the UEFI memory map did not return canonical metadata",
            )
        })?;
        let memory_map = MemoryMapEvidence::Observed {
            descriptor_size,
            descriptor_version,
            descriptors: &memory_descriptors,
        };

        let captured_acpi = collect_acpi(
            &owned_config_tables,
            &memory_descriptors,
            cpu.physical_address_bits(),
            processor_total,
            processor_enabled,
            processor_records,
        )?;

        let cpu_physical_address_bits = cpu.physical_address_bits().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the live-IOMMU slice requires an enumerated CPU physical-address width",
            )
        })?;
        let live_iommu_capture = iommu_live::collect_live_amd_iommu(
            &captured_acpi,
            &memory_descriptors,
            cpu_physical_address_bits,
        )?;
        let configured_range_evidence =
            project_configured_ranges(&live_iommu_capture, &memory_descriptors)?;
        let amd_iommu_live = project_live_iommu_evidence(
            &live_iommu_capture,
            &configured_range_evidence,
            &memory_descriptors,
        )?;

        let json = with_acpi_evidence(&captured_acpi, processor_records, |acpi| {
            let evidence = Evidence {
                target_profile_manifest_sha256: &target_profile_manifest_sha256,
                output_file: output_file.as_str(),
                collected_at,
                sink: authorized_sink.record,
                firmware_vendor: firmware_vendor.as_str(),
                firmware_revision,
                uefi_revision_major: uefi_revision.major(),
                uefi_revision_minor: uefi_revision.minor(),
                config_tables: &config_tables,
                cpu: &cpu,
                vm_cr,
                mp_services,
                processor_consistency,
                memory_map,
                acpi,
                amd_iommu_live,
                system_registers,
            };
            render_json(&evidence)
        })?
        .map_err(|_| {
            ProbeError::new(
                Status::COMPROMISED_DATA,
                "the evidence failed strict preflight or the canonical render cap",
            )
        })?;

        write_new_evidence_file(
            authorized_sink.file_system_handle,
            authorized_sink.record,
            partial_path.as_ref(),
            output_path.as_ref(),
            json.as_bytes(),
        )?;

        Ok(output_file)
    }

    fn project_memory_binding(
        witness: svmvisor_m0b_probe::iommu::ranges::DescriptorWitness,
        memory_records: &[MemoryDescriptorRecord],
    ) -> Result<MemoryDescriptorBindingEvidence, ProbeError> {
        let descriptor = memory_records
            .get(witness.descriptor_index)
            .copied()
            .ok_or_else(|| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an IOMMU memory-map witness has an invalid descriptor index",
                )
            })?;
        if descriptor.memory_type != witness.descriptor.memory_type
            || descriptor.physical_start != witness.descriptor.physical_start
            || descriptor.page_count != witness.descriptor.page_count
            || descriptor.attributes != witness.descriptor.attributes
        {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "an IOMMU memory-map witness changed before evidence projection",
            ));
        }
        Ok(MemoryDescriptorBindingEvidence {
            descriptor_index: witness.descriptor_index,
            descriptor,
            requested_start: witness.requested_range.start,
            requested_end_exclusive: witness.requested_range.end_exclusive,
        })
    }

    fn project_configured_ranges(
        capture: &iommu_live::LiveAmdIommuCapture,
        memory_records: &[MemoryDescriptorRecord],
    ) -> Result<Vec<IommuConfiguredRangeEvidence>, ProbeError> {
        let iommu_live::LiveMmioCapture::Observed {
            configured_ranges, ..
        } = &capture.mmio
        else {
            return Ok(Vec::new());
        };
        let mut projected = Vec::new();
        projected
            .try_reserve_exact(configured_ranges.len())
            .map_err(|_| {
                ProbeError::new(
                    Status::OUT_OF_RESOURCES,
                    "configured IOMMU range evidence could not be allocated",
                )
            })?;
        for range in configured_ranges {
            let memory_binding = match range.memory_witness {
                Some(witness) => Some(project_memory_binding(witness, memory_records)?),
                None => None,
            };
            projected.push(IommuConfiguredRangeEvidence {
                source_offset: range.source_offset,
                raw: range.raw,
                enabled: range.enabled,
                base: range.base,
                length: range.length,
                alignment: range.alignment,
                validated_range: range.validated_range,
                memory_binding,
            });
        }
        Ok(projected)
    }

    fn project_mmio_common<'a>(
        common: &'a iommu_live::LiveMmioCommon,
        memory_records: &[MemoryDescriptorRecord],
    ) -> Result<IommuMmioCommonEvidence<'a>, ProbeError> {
        let aperture_length_bytes = common
            .aperture_witness
            .requested_range
            .end_exclusive
            .checked_sub(common.aperture_witness.requested_range.start)
            .ok_or_else(|| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the IOMMU aperture witness is reversed",
                )
            })?;
        Ok(IommuMmioCommonEvidence {
            aperture_length_bytes,
            aperture_memory_binding: project_memory_binding(
                common.aperture_witness,
                memory_records,
            )?,
            stable_offsets: &common.plan.stable_offsets,
            feature_dependent_offsets: &common.feature_dependent_offsets,
            first_snapshot: common.first_snapshot,
            second_snapshot: common.second_snapshot,
            stable: true,
            status_offset: common.plan.status_offset,
            status_raw: common.status_raw,
        })
    }

    fn project_live_iommu_evidence<'a>(
        capture: &'a iommu_live::LiveAmdIommuCapture,
        configured_ranges: &'a [IommuConfiguredRangeEvidence],
        memory_records: &[MemoryDescriptorRecord],
    ) -> Result<AmdIommuLiveEvidence<'a>, ProbeError> {
        let access = capture.access;
        let mmio = match &capture.mmio {
            iommu_live::LiveMmioCapture::Disabled => AmdIommuMmioEvidence::Disabled,
            iommu_live::LiveMmioCapture::EfrConflict {
                common,
                expected,
                live,
            } => AmdIommuMmioEvidence::EfrConflict {
                common: project_mmio_common(common, memory_records)?,
                expected: *expected,
                live: *live,
            },
            iommu_live::LiveMmioCapture::Observed {
                common,
                features,
                decoded,
                ..
            } => AmdIommuMmioEvidence::Observed {
                common: project_mmio_common(common, memory_records)?,
                features: *features,
                configured_ranges,
                decoded: *decoded,
            },
        };
        Ok(AmdIommuLiveEvidence {
            access: IommuAccessEvidence {
                root_bridge_handle_count: access.root_bridge_handle_count,
                matching_segment_handle_count: access.matching_segment_handle_count,
                full_match_handle_count: access.full_match_handle_count,
                pci_read_operations: access.pci_read_operations,
                pci_read_bytes: access.pci_read_bytes,
                mmio_read_operations: access.mmio_read_operations,
                mmio_read_bytes: access.mmio_read_bytes,
                pci_write_operations: access.pci_write_operations,
                mmio_write_operations: access.mmio_write_operations,
                direct_ecam_access: access.direct_ecam_access,
                direct_mmio_access: access.direct_mmio_access,
                cf8_cfc_access: access.cf8_cfc_access,
                configured_pointer_dereferences: access.configured_pointer_dereferences,
            },
            locator: IommuLocatorEvidence {
                ivhd_sources: &capture.ivhd_sources,
                unit: capture.unit,
                mcfg_allocations: &capture.mcfg_allocations,
                mcfg_witness: capture.mcfg_witness,
                ecam_memory_binding: project_memory_binding(
                    capture.ecam_memory_witness,
                    memory_records,
                )?,
            },
            pci: IommuPciEvidence {
                selected_root_bridge_handle_index: capture.selected_root_bridge_handle_index,
                segment_group: capture.selected_root_bridge_segment,
                bdf: capture.unit.bdf,
                identity_raw: capture.identity_raw,
                identity_decoded: capture.identity_decoded,
                capability_chain: &capture.capability_chain,
                capability_first_raw: capture.capability_first_raw,
                capability_second_raw: capture.capability_second_raw,
                capability_stable: true,
                capability_decoded: capture.capability_decoded,
            },
            mmio,
            ownership: IommuOwnershipEvidence {
                assessed: false,
                claim: false,
                requester_dma_isolation_claim: false,
                interrupt_remapping_claim: false,
                pci_isolation_claim: false,
            },
        })
    }

    /// Open a boot-time protocol without the driver-disconnect side effects of
    /// `OpenProtocol(EXCLUSIVE)`.
    ///
    /// Every handle passed here is either the running image handle, a protocol
    /// handle located from its live device path, or the firmware's current MP
    /// Services handle. The BSP serializes protocol use except for the explicitly
    /// AP-safe MP Services WhoAmI call inside a blocking StartupThisAP callback.
    /// The application never uninstalls or reinstalls protocols and drops every
    /// returned scope before returning to firmware. Those invariants keep each
    /// interface live for the scope.
    #[allow(unsafe_code)]
    fn open_protocol_get<P: ProtocolPointer + ?Sized>(
        handle: Handle,
    ) -> uefi::Result<ScopedProtocol<P>> {
        // SAFETY: The function-level invariants above cover the protocol and
        // handle lifetime required by GET_PROTOCOL. Unlike EXCLUSIVE, this
        // attribute does not disconnect drivers or mutate controller binding.
        unsafe {
            boot::open_protocol::<P>(
                OpenProtocolParams {
                    handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }
    }

    fn read_profile_binding(
        file_system_handle: Handle,
        expected_sink: SinkRecord,
    ) -> Result<[u8; PROFILE_BINDING_LENGTH], ProbeError> {
        let binding_path = CString16::try_from(PROFILE_BINDING_FILE).map_err(|_| {
            ProbeError::new(
                Status::INVALID_PARAMETER,
                "the profile binding path is invalid",
            )
        })?;
        let mut protocol =
            open_protocol_get::<SimpleFileSystem>(file_system_handle).map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "the loaded image volume has no usable Simple File System protocol",
                )
            })?;
        let file_system = protocol.get_mut().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the loaded image volume returned a null Simple File System interface",
            )
        })?;
        let mut root = file_system.open_volume().map_err(|_| {
            ProbeError::new(
                Status::SECURITY_VIOLATION,
                "the profile binding volume could not be opened",
            )
        })?;
        let file = root
            .open(
                binding_path.as_ref(),
                FileMode::Read,
                FileAttribute::empty(),
            )
            .map_err(|_| {
                ProbeError::new(
                    Status::SECURITY_VIOLATION,
                    "the exact M0a profile binding marker is missing or unreadable",
                )
            })?;
        let mut file = file.into_regular_file().ok_or_else(|| {
            ProbeError::new(
                Status::SECURITY_VIOLATION,
                "the M0a profile binding marker is not a regular file",
            )
        })?;

        // One extra byte detects trailing data without allocating from an
        // untrusted on-media file-size field.
        let mut bounded = [0_u8; PROFILE_BINDING_LENGTH + 1];
        let mut read = 0;
        while read < bounded.len() {
            let count = file.read(&mut bounded[read..]).map_err(|_| {
                ProbeError::new(
                    Status::SECURITY_VIOLATION,
                    "the exact M0a profile binding marker could not be read completely",
                )
            })?;
            if count == 0 {
                break;
            }
            read += count;
        }
        if read != PROFILE_BINDING_LENGTH {
            return Err(ProbeError::new(
                Status::SECURITY_VIOLATION,
                "the M0a profile binding marker has a non-canonical length",
            ));
        }

        drop(file);
        drop(root);
        drop(protocol);
        validate_sink_unchanged(file_system_handle, expected_sink)?;

        let mut binding = [0_u8; PROFILE_BINDING_LENGTH];
        binding.copy_from_slice(&bounded[..PROFILE_BINDING_LENGTH]);
        Ok(binding)
    }

    fn ensure_output_absent(
        file_system_handle: Handle,
        expected_sink: SinkRecord,
        path: &CStr16,
    ) -> Result<(), ProbeError> {
        validate_sink_unchanged(file_system_handle, expected_sink)?;
        let mut protocol =
            open_protocol_get::<SimpleFileSystem>(file_system_handle).map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "the evidence sink file system became unavailable",
                )
            })?;
        let file_system = protocol.get_mut().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the evidence sink returned a null Simple File System interface",
            )
        })?;
        let mut root = file_system.open_volume().map_err(|error| {
            ProbeError::new(
                error.status(),
                "the evidence sink volume could not be opened",
            )
        })?;
        match root.open(path, FileMode::Read, FileAttribute::empty()) {
            Ok(existing) => {
                drop(existing);
                Err(ProbeError::new(
                    Status::ALREADY_STARTED,
                    "the evidence output already exists; refusing to overwrite it",
                ))
            }
            Err(error) if error.status() == Status::NOT_FOUND => Ok(()),
            Err(error) => Err(ProbeError::new(
                error.status(),
                "the evidence sink could not check the exclusive output path",
            )),
        }
    }

    fn validate_sink_unchanged(
        file_system_handle: Handle,
        expected: SinkRecord,
    ) -> Result<(), ProbeError> {
        let block_io = open_protocol_get::<BlockIO>(file_system_handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the selected evidence medium could not be revalidated",
            )
        })?;
        let block_io = block_io.get().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the selected evidence medium returned a null Block I/O interface",
            )
        })?;
        let media = block_io.media();
        let current = SinkRecord {
            media_id: media.media_id(),
            removable_media: media.is_removable_media(),
            media_present: media.is_media_present(),
            logical_partition: media.is_logical_partition(),
            read_only: media.is_read_only(),
            block_size: media.block_size(),
            last_block: media.last_block(),
        };
        if current != expected {
            return Err(ProbeError::new(
                Status::MEDIA_CHANGED,
                "the selected removable evidence medium changed during capture",
            ));
        }
        Ok(())
    }

    fn write_new_evidence_file(
        file_system_handle: Handle,
        expected_sink: SinkRecord,
        partial_path: &CStr16,
        committed_path: &CStr16,
        content: &[u8],
    ) -> Result<(), ProbeError> {
        validate_sink_unchanged(file_system_handle, expected_sink)?;
        let mut protocol =
            open_protocol_get::<SimpleFileSystem>(file_system_handle).map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "the evidence sink file system became unavailable",
                )
            })?;
        let file_system = protocol.get_mut().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the evidence sink returned a null Simple File System interface",
            )
        })?;
        let mut root = file_system.open_volume().map_err(|error| {
            ProbeError::new(
                error.status(),
                "the evidence sink volume could not be opened",
            )
        })?;

        for path in [partial_path, committed_path] {
            match root.open(path, FileMode::Read, FileAttribute::empty()) {
                Ok(existing) => {
                    drop(existing);
                    return Err(ProbeError::new(
                        Status::ALREADY_STARTED,
                        "an evidence output path appeared before creation; refusing to overwrite it",
                    ));
                }
                Err(error) if error.status() == Status::NOT_FOUND => {}
                Err(error) => {
                    return Err(ProbeError::new(
                        error.status(),
                        "the evidence sink could not repeat the exclusive output-path checks",
                    ));
                }
            }
        }

        let file = root
            .open(
                partial_path,
                FileMode::CreateReadWrite,
                FileAttribute::empty(),
            )
            .map_err(|error| {
                ProbeError::new(error.status(), "the evidence output could not be created")
            })?;
        let mut file = file.into_regular_file().ok_or_else(|| {
            ProbeError::new(
                Status::INVALID_PARAMETER,
                "the evidence output path did not create a regular file",
            )
        })?;
        file.set_position(RegularFile::END_OF_FILE)
            .map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "the newly created evidence output could not be inspected",
                )
            })?;
        let existing_size = file.get_position().map_err(|error| {
            ProbeError::new(
                error.status(),
                "the newly created evidence output could not be inspected",
            )
        })?;
        if existing_size != 0 {
            return Err(ProbeError::new(
                Status::ALREADY_STARTED,
                "the evidence output was not empty at creation; refusing to overwrite it",
            ));
        }
        file.set_position(0).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the newly created evidence output could not be positioned",
            )
        })?;

        file.write(content).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the complete evidence record could not be written",
            )
        })?;
        file.flush().map_err(|error| {
            ProbeError::new(
                error.status(),
                "the evidence output could not be flushed to removable media",
            )
        })?;
        validate_sink_unchanged(file_system_handle, expected_sink)?;

        let metadata = file.get_boxed_info::<FileInfo>().map_err(|error| {
            ProbeError::new(
                error.status(),
                "the flushed partial evidence file could not be inspected for commit",
            )
        })?;
        let file_size = metadata.file_size();
        let physical_size = metadata.physical_size();
        let create_time = *metadata.create_time();
        let last_access_time = *metadata.last_access_time();
        let modification_time = *metadata.modification_time();
        let attribute = metadata.attribute();
        drop(metadata);

        let mut rename_storage = FileInfoStorage([0_u8; 512]);
        let rename_info = FileInfo::new(
            &mut rename_storage.0,
            file_size,
            physical_size,
            create_time,
            last_access_time,
            modification_time,
            attribute,
            committed_path,
        )
        .map_err(|_| {
            ProbeError::new(
                Status::OUT_OF_RESOURCES,
                "the evidence commit record could not be constructed",
            )
        })?;
        validate_sink_unchanged(file_system_handle, expected_sink)?;
        file.set_info(rename_info).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the validated partial evidence file could not be committed",
            )
        })?;
        // Successful SetInfo is the terminal commit. There are deliberately no
        // fallible operations after the accepted final filename is published.
        Ok(())
    }

    fn inspect_and_authorize_image_volume() -> Result<AuthorizedSink, ProbeError> {
        let image_handle = boot::image_handle();
        let loaded_image = open_protocol_get::<LoadedImage>(image_handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the probe cannot inspect its loaded-image protocol",
            )
        })?;
        let loaded_image = loaded_image.get().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the probe received a null loaded-image protocol",
            )
        })?;
        let device_handle = loaded_image.device().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the probe image is not associated with a storage device",
            )
        })?;
        let device_path = open_protocol_get::<DevicePath>(device_handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the probe image device has no usable device path",
            )
        })?;
        let device_path = device_path.get().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the probe image device returned a null device path",
            )
        })?;
        let mut remaining_path: &DevicePath = device_path;
        let file_system_handle = boot::locate_device_path::<SimpleFileSystem>(&mut remaining_path)
            .map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "the probe image path does not resolve to a Simple File System volume",
                )
            })?;
        let block_io = open_protocol_get::<BlockIO>(file_system_handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the loaded-image file-system handle has no inspectable Block I/O media",
            )
        })?;
        let block_io = block_io.get().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the loaded-image volume returned a null Block I/O interface",
            )
        })?;
        let media = block_io.media();
        let sink = SinkRecord {
            media_id: media.media_id(),
            removable_media: media.is_removable_media(),
            media_present: media.is_media_present(),
            logical_partition: media.is_logical_partition(),
            read_only: media.is_read_only(),
            block_size: media.block_size(),
            last_block: media.last_block(),
        };

        sink.authorize().map_err(|error| match error {
            svmvisor_m0b_probe::SinkAuthorizationError::NotRemovable => ProbeError::new(
                Status::SECURITY_VIOLATION,
                "the loaded-image volume is not firmware-reported removable media",
            ),
            svmvisor_m0b_probe::SinkAuthorizationError::MediaAbsent => ProbeError::new(
                Status::NO_MEDIA,
                "the selected removable evidence medium is absent",
            ),
            svmvisor_m0b_probe::SinkAuthorizationError::ReadOnly => ProbeError::new(
                Status::WRITE_PROTECTED,
                "the selected removable evidence medium is read-only",
            ),
            svmvisor_m0b_probe::SinkAuthorizationError::InvalidBlockSize => ProbeError::new(
                Status::DEVICE_ERROR,
                "the selected removable evidence medium reports an invalid block size",
            ),
        })?;

        Ok(AuthorizedSink {
            record: sink,
            file_system_handle,
        })
    }

    fn collect_configuration_tables() -> Result<Vec<OwnedConfigTable>, ProbeError> {
        system::with_config_table(|tables| {
            if tables.len() > MAX_CONFIGURATION_TABLES {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the UEFI configuration-table count exceeds the capture cap",
                ));
            }

            let mut owned = Vec::<OwnedConfigTable>::new();
            owned.try_reserve_exact(tables.len()).map_err(|_| {
                ProbeError::new(
                    Status::OUT_OF_RESOURCES,
                    "the bounded configuration-table directory could not be allocated",
                )
            })?;
            for table in tables {
                let kind = if table.guid == ConfigTableEntry::ACPI2_GUID {
                    "acpi2-rsdp"
                } else if table.guid == ConfigTableEntry::ACPI_GUID {
                    "acpi1-rsdp"
                } else if table.guid == ConfigTableEntry::SMBIOS3_GUID {
                    "smbios3-entry-point"
                } else if table.guid == ConfigTableEntry::SMBIOS_GUID {
                    "smbios-entry-point"
                } else if table.guid == ConfigTableEntry::ESRT_GUID {
                    "esrt"
                } else {
                    "other"
                };
                owned.push(OwnedConfigTable {
                    kind,
                    guid: table.guid.to_string(),
                    address: table.address as usize as u64,
                });
            }
            Ok(owned)
        })
    }

    impl<'a> PhysicalAcpiReader<'a> {
        fn new(
            memory_descriptors: &'a [MemoryDescriptorRecord],
            physical_address_bits: Option<u8>,
        ) -> Result<Self, ProbeError> {
            let physical_address_bits = physical_address_bits.ok_or_else(|| {
                ProbeError::new(
                    Status::UNSUPPORTED,
                    "CPUID did not enumerate the physical-address width needed for ACPI reads",
                )
            })?;
            if !(32..=63).contains(&physical_address_bits) {
                return Err(ProbeError::new(
                    Status::UNSUPPORTED,
                    "CPUID reported an unsupported physical-address width",
                ));
            }
            Ok(PhysicalAcpiReader {
                memory_descriptors,
                physical_address_bits,
                copied_bytes: 0,
            })
        }

        fn validate_range(&self, address: u64, length: usize) -> Result<usize, ProbeError> {
            if length == 0 {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI structure requested a zero-length physical read",
                ));
            }
            let length_u64 = u64::try_from(length).map_err(|_| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI physical-read length could not be represented",
                )
            })?;
            let end_exclusive = address.checked_add(length_u64).ok_or_else(|| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI physical-read range overflowed",
                )
            })?;
            let architectural_end = 1_u64 << self.physical_address_bits;
            if end_exclusive > architectural_end {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI pointer exceeds the enumerated physical-address width",
                ));
            }

            let mut covered = false;
            for descriptor in self.memory_descriptors {
                if descriptor.memory_type != ACPI_RECLAIM_MEMORY_TYPE
                    && descriptor.memory_type != ACPI_NON_VOLATILE_MEMORY_TYPE
                {
                    continue;
                }
                let descriptor_length = descriptor
                    .page_count
                    .checked_mul(UEFI_PAGE_SIZE)
                    .ok_or_else(|| {
                        ProbeError::new(
                            Status::COMPROMISED_DATA,
                            "an ACPI UEFI memory descriptor length overflowed",
                        )
                    })?;
                let descriptor_end = descriptor
                    .physical_start
                    .checked_add(descriptor_length)
                    .ok_or_else(|| {
                        ProbeError::new(
                            Status::COMPROMISED_DATA,
                            "an ACPI UEFI memory descriptor range overflowed",
                        )
                    })?;
                if address >= descriptor.physical_start && end_exclusive <= descriptor_end {
                    if descriptor.attributes & MemoryAttribute::READ_PROTECT.bits() != 0 {
                        return Err(ProbeError::new(
                            Status::SECURITY_VIOLATION,
                            "an ACPI physical-read range is UEFI read-protected",
                        ));
                    }
                    covered = true;
                    break;
                }
            }
            if !covered {
                return Err(ProbeError::new(
                    Status::SECURITY_VIOLATION,
                    "an ACPI pointer is outside UEFI ACPI reclaim or non-volatile memory",
                ));
            }

            usize::try_from(address).map_err(|_| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI physical address could not be represented by this UEFI image",
                )
            })
        }

        /// Copy immutable firmware-description bytes after checking the complete
        /// physical range against the same-run UEFI memory map and CPUID width.
        #[allow(unsafe_code)]
        fn copy(&mut self, address: u64, length: usize) -> Result<Vec<u8>, ProbeError> {
            let source_address = self.validate_range(address, length)?;
            let new_total = self.copied_bytes.checked_add(length).ok_or_else(|| {
                ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the cumulative ACPI physical-read count overflowed",
                )
            })?;
            if new_total > MAX_TOTAL_ACPI_BYTES {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the cumulative ACPI physical-read cap was exceeded",
                ));
            }

            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|_| {
                ProbeError::new(
                    Status::OUT_OF_RESOURCES,
                    "a bounded ACPI physical-read buffer could not be allocated",
                )
            })?;
            bytes.resize(length, 0);
            // SAFETY: `validate_range` proves the complete source extent is within
            // one live UEFI ACPI_RECLAIM or ACPI_NON_VOLATILE descriptor and below
            // the CPUID physical-address limit. x86_64 UEFI exposes those physical
            // firmware-description ranges in the boot-time flat address space.
            // `bytes` owns a distinct destination allocation of exactly `length`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source_address as *const u8,
                    bytes.as_mut_ptr(),
                    length,
                );
            }
            self.copied_bytes = new_total;
            Ok(bytes)
        }
    }

    fn copy_rsdp(
        reader: &mut PhysicalAcpiReader<'_>,
        address: u64,
        limits: &AcpiLimits,
    ) -> Result<Vec<u8>, ProbeError> {
        let v1_prefix = reader.copy(address, 20)?;
        let revision = v1_prefix[15];
        let declared_length = match revision {
            0 => 20,
            1 => {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the selected ACPI RSDP revision is unsupported",
                ));
            }
            _ => {
                let v2_prefix = reader.copy(address, 36)?;
                rsdp_declared_length(&v2_prefix, limits).map_err(|error| {
                    ProbeError::with_acpi_detail(
                        Status::COMPROMISED_DATA,
                        "the selected ACPI RSDP prefix is malformed",
                        "RSDP-prefix",
                        error,
                    )
                })?
            }
        };
        let raw = reader.copy(address, declared_length)?;
        Rsdp::parse(&raw, limits).map_err(|error| {
            ProbeError::with_acpi_detail(
                Status::COMPROMISED_DATA,
                "the selected ACPI RSDP failed bounded validation",
                "RSDP",
                error,
            )
        })?;
        Ok(raw)
    }

    fn copy_complete_sdt(
        reader: &mut PhysicalAcpiReader<'_>,
        address: u64,
        limits: &AcpiLimits,
    ) -> Result<Vec<u8>, ProbeError> {
        let header = reader.copy(address, SDT_HEADER_LENGTH)?;
        let prefix = inspect_sdt_prefix(&header, limits).map_err(|error| {
            ProbeError::with_acpi_detail(
                Status::COMPROMISED_DATA,
                "an ACPI SDT header failed bounded validation",
                "SDT-header",
                error,
            )
        })?;
        reader.copy(address, prefix.length)
    }

    fn capture_root(
        reader: &mut PhysicalAcpiReader<'_>,
        address: u64,
        kind: RootTableKind,
        limits: &AcpiLimits,
    ) -> Result<CapturedRoot, ProbeError> {
        let raw = copy_complete_sdt(reader, address, limits)?;
        let entries = RootTable::parse(&raw, kind, limits)
            .map_err(|error| {
                ProbeError::with_acpi_detail(
                    Status::COMPROMISED_DATA,
                    "an ACPI root table failed signature, checksum, or entry validation",
                    root_table_label(kind),
                    error,
                )
            })?
            .table_pointers;
        Ok(CapturedRoot {
            address,
            raw,
            entries,
        })
    }

    fn collect_acpi(
        config_tables: &[OwnedConfigTable],
        memory_descriptors: &[MemoryDescriptorRecord],
        physical_address_bits: Option<u8>,
        mp_total: usize,
        mp_enabled: usize,
        mp_processors: &[ProcessorRecord],
    ) -> Result<CapturedAcpi, ProbeError> {
        let mut selected = None;
        for table in config_tables
            .iter()
            .filter(|table| table.kind == "acpi2-rsdp")
        {
            if selected.replace(table).is_some() {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "multiple ACPI 2.0 configuration-table entries are ambiguous",
                ));
            }
        }
        let selected = selected.ok_or_else(|| {
            ProbeError::new(
                Status::NOT_FOUND,
                "the required ACPI 2.0 configuration-table entry is absent",
            )
        })?;
        if selected.guid != ACPI2_GUID {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "the selected ACPI 2.0 configuration-table GUID is non-canonical",
            ));
        }

        let limits = AcpiLimits::default();
        let mut reader = PhysicalAcpiReader::new(memory_descriptors, physical_address_bits)?;
        let rsdp_raw = copy_rsdp(&mut reader, selected.address, &limits)?;
        let root_pointers = Rsdp::parse(&rsdp_raw, &limits)
            .and_then(|rsdp| rsdp.root_pointers())
            .map_err(|error| {
                ProbeError::with_acpi_detail(
                    Status::COMPROMISED_DATA,
                    "the selected ACPI RSDP does not identify distinct bounded roots",
                    "RSDP-roots",
                    error,
                )
            })?;
        let rsdt_address = root_pointers.rsdt.ok_or_else(|| {
            ProbeError::new(
                Status::COMPROMISED_DATA,
                "the selected ACPI RSDP does not provide the required RSDT",
            )
        })?;
        let xsdt_address = root_pointers.xsdt.ok_or_else(|| {
            ProbeError::new(
                Status::COMPROMISED_DATA,
                "the selected ACPI RSDP does not provide the required XSDT",
            )
        })?;
        let rsdt = capture_root(&mut reader, rsdt_address, RootTableKind::Rsdt, &limits)?;
        let xsdt = capture_root(&mut reader, xsdt_address, RootTableKind::Xsdt, &limits)?;

        let mut directory = Vec::<CapturedDirectoryEntry>::new();
        directory
            .try_reserve_exact(MAX_UNIQUE_ROOT_POINTERS)
            .map_err(|_| {
                ProbeError::new(
                    Status::OUT_OF_RESOURCES,
                    "the bounded ACPI table directory could not be allocated",
                )
            })?;
        for (reference, entries) in [
            (RootReference::Rsdt, rsdt.entries.as_slice()),
            (RootReference::Xsdt, xsdt.entries.as_slice()),
        ] {
            for address in entries {
                if let Some(existing) = directory.iter_mut().find(|entry| entry.address == *address)
                {
                    existing.referenced_by.push(reference);
                    continue;
                }
                if directory.len() >= MAX_UNIQUE_ROOT_POINTERS {
                    return Err(ProbeError::new(
                        Status::COMPROMISED_DATA,
                        "the unique ACPI root-pointer cap was exceeded",
                    ));
                }
                let header_raw = reader.copy(*address, SDT_HEADER_LENGTH)?;
                let prefix = inspect_sdt_prefix(&header_raw, &limits).map_err(|error| {
                    ProbeError::with_acpi_detail(
                        Status::COMPROMISED_DATA,
                        "an ACPI directory entry has a malformed SDT header",
                        "ACPI-directory-SDT-header",
                        error,
                    )
                })?;
                directory.push(CapturedDirectoryEntry {
                    address: *address,
                    referenced_by: alloc::vec![reference],
                    signature: prefix.signature,
                    declared_length: prefix.length,
                    revision: header_raw[8],
                    header_raw,
                    table_raw: None,
                });
            }
        }

        let mut seen_whitelist = [false; 4];
        for entry in &mut directory {
            let Some(kind) = WhitelistedTableKind::from_signature(entry.signature) else {
                continue;
            };
            let kind_index = match kind {
                WhitelistedTableKind::Madt => 0,
                WhitelistedTableKind::Mcfg => 1,
                WhitelistedTableKind::Ivrs => 2,
                WhitelistedTableKind::Fadt => 3,
            };
            if seen_whitelist[kind_index] {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "multiple physical tables use one whitelisted ACPI signature",
                ));
            }
            let raw = reader.copy(entry.address, entry.declared_length)?;
            if raw[..SDT_HEADER_LENGTH] != entry.header_raw {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "an ACPI SDT header changed during the bounded capture",
                ));
            }
            parse_whitelisted_table(&raw, &limits).map_err(|error| {
                ProbeError::with_acpi_detail(
                    Status::COMPROMISED_DATA,
                    "a whitelisted ACPI table failed strict bounded validation",
                    whitelisted_table_label(kind),
                    error,
                )
            })?;
            entry.table_raw = Some(raw);
            seen_whitelist[kind_index] = true;
        }
        if seen_whitelist.iter().any(|present| !present) {
            return Err(ProbeError::new(
                Status::NOT_FOUND,
                "one or more required MADT, MCFG, IVRS, or FADT tables are absent",
            ));
        }

        let madt_raw = directory
            .iter()
            .find(|entry| entry.signature == *b"APIC")
            .and_then(|entry| entry.table_raw.as_deref())
            .ok_or_else(|| {
                ProbeError::new(
                    Status::NOT_FOUND,
                    "the validated MADT is unavailable for topology comparison",
                )
            })?;
        let madt = match parse_whitelisted_table(madt_raw, &limits).map_err(|error| {
            ProbeError::with_acpi_detail(
                Status::COMPROMISED_DATA,
                "the MADT could not be reparsed for topology comparison",
                "MADT/APIC",
                error,
            )
        })? {
            WhitelistedTable::Madt(madt) => madt,
            _ => {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "the APIC directory entry did not decode as MADT",
                ));
            }
        };
        let topology =
            cross_check_madt_with_mp_services(&madt, mp_total, mp_enabled, mp_processors, &limits)
                .map_err(|error| {
                    ProbeError::with_acpi_detail(
                        Status::COMPROMISED_DATA,
                        "MADT and MP Services topology could not be compared canonically",
                        "MADT/MP-topology",
                        error,
                    )
                })?;

        Ok(CapturedAcpi {
            rsdp_address: selected.address,
            rsdp_raw,
            rsdt,
            xsdt,
            directory,
            topology,
        })
    }

    fn acpi_projection_error(message: &'static str) -> ProbeError {
        ProbeError::new(Status::COMPROMISED_DATA, message)
    }

    fn acpi_projection_parser_error(
        message: &'static str,
        source: &'static str,
        error: AcpiError,
    ) -> ProbeError {
        ProbeError::with_acpi_detail(Status::COMPROMISED_DATA, message, source, error)
    }

    fn acpi_projection_allocation_error() -> ProbeError {
        ProbeError::new(
            Status::OUT_OF_RESOURCES,
            "a bounded ACPI evidence projection could not be allocated",
        )
    }

    fn read_projection_u16(raw: &[u8], offset: usize) -> Result<u16, ProbeError> {
        let end = offset
            .checked_add(2)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field offset overflowed"))?;
        let bytes = raw
            .get(offset..end)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field is truncated"))?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_projection_u32(raw: &[u8], offset: usize) -> Result<u32, ProbeError> {
        let end = offset
            .checked_add(4)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field offset overflowed"))?;
        let bytes = raw
            .get(offset..end)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field is truncated"))?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_projection_array<const N: usize>(
        raw: &[u8],
        offset: usize,
    ) -> Result<[u8; N], ProbeError> {
        let end = offset
            .checked_add(N)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field offset overflowed"))?;
        raw.get(offset..end)
            .ok_or_else(|| acpi_projection_error("an ACPI evidence field is truncated"))?
            .try_into()
            .map_err(|_| acpi_projection_error("an ACPI evidence field has the wrong width"))
    }

    fn project_sdt_header(header: svmvisor_m0b_probe::acpi::SdtHeader) -> AcpiSdtHeaderEvidence {
        AcpiSdtHeaderEvidence {
            signature: header.signature,
            length: header.length,
            revision: header.revision,
            checksum: header.checksum,
            oem_id: header.oem_id,
            oem_table_id: header.oem_table_id,
            oem_revision: header.oem_revision,
            creator_id: header.creator_id,
            creator_revision: header.creator_revision,
        }
    }

    fn project_root_references(
        references: &[RootReference],
    ) -> Result<AcpiRootReferences, ProbeError> {
        match references {
            [RootReference::Rsdt] => Ok(AcpiRootReferences::Rsdt),
            [RootReference::Xsdt] => Ok(AcpiRootReferences::Xsdt),
            [RootReference::Rsdt, RootReference::Xsdt] => Ok(AcpiRootReferences::RsdtAndXsdt),
            _ => Err(acpi_projection_error(
                "an ACPI directory reference set is non-canonical",
            )),
        }
    }

    fn find_captured_table(
        captured: &CapturedAcpi,
        signature: [u8; 4],
    ) -> Result<&CapturedDirectoryEntry, ProbeError> {
        captured
            .directory
            .iter()
            .find(|entry| entry.signature == signature)
            .ok_or_else(|| acpi_projection_error("a required captured ACPI table is absent"))
    }

    fn with_acpi_evidence<R>(
        captured: &CapturedAcpi,
        mp_processors: &[ProcessorRecord],
        callback: impl for<'e> FnOnce(AcpiEvidence<'e>) -> R,
    ) -> Result<R, ProbeError> {
        let limits = AcpiLimits::default();
        let rsdp = Rsdp::parse(&captured.rsdp_raw, &limits).map_err(|error| {
            acpi_projection_parser_error(
                "the captured RSDP could not be projected into evidence",
                "RSDP-projection",
                error,
            )
        })?;
        let xsdt_address = rsdp.xsdt_address.ok_or_else(|| {
            acpi_projection_error("the captured ACPI 2.0 RSDP has no XSDT address")
        })?;
        let extended_checksum = rsdp.extended_checksum.ok_or_else(|| {
            acpi_projection_error("the captured ACPI 2.0 RSDP has no extended checksum")
        })?;
        let rsdp_reserved = read_projection_array::<3>(&captured.rsdp_raw, 33)?;

        let rsdt = RootTable::parse(&captured.rsdt.raw, RootTableKind::Rsdt, &limits).map_err(
            |error| {
                acpi_projection_parser_error(
                    "the captured RSDT could not be projected into evidence",
                    "RSDT-projection",
                    error,
                )
            },
        )?;
        let xsdt = RootTable::parse(&captured.xsdt.raw, RootTableKind::Xsdt, &limits).map_err(
            |error| {
                acpi_projection_parser_error(
                    "the captured XSDT could not be projected into evidence",
                    "XSDT-projection",
                    error,
                )
            },
        )?;
        if rsdt.table_pointers != captured.rsdt.entries
            || xsdt.table_pointers != captured.xsdt.entries
        {
            return Err(acpi_projection_error(
                "an ACPI root changed between validation and evidence projection",
            ));
        }

        let mut directory_records = Vec::new();
        directory_records
            .try_reserve_exact(captured.directory.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for entry in &captured.directory {
            directory_records.push(AcpiDirectoryRecord {
                address: entry.address,
                referenced_by: project_root_references(&entry.referenced_by)?,
                header_raw: RawBytesEvidence {
                    bytes: &entry.header_raw,
                },
                signature: entry.signature,
                declared_length: u32::try_from(entry.declared_length).map_err(|_| {
                    acpi_projection_error("an ACPI declared length exceeds the evidence width")
                })?,
                revision: entry.revision,
            });
        }

        let madt_capture = find_captured_table(captured, *b"APIC")?;
        let mcfg_capture = find_captured_table(captured, *b"MCFG")?;
        let ivrs_capture = find_captured_table(captured, *b"IVRS")?;
        let fadt_capture = find_captured_table(captured, *b"FACP")?;
        let madt_raw = madt_capture
            .table_raw
            .as_deref()
            .ok_or_else(|| acpi_projection_error("the captured MADT raw bytes are absent"))?;
        let mcfg_raw = mcfg_capture
            .table_raw
            .as_deref()
            .ok_or_else(|| acpi_projection_error("the captured MCFG raw bytes are absent"))?;
        let ivrs_raw = ivrs_capture
            .table_raw
            .as_deref()
            .ok_or_else(|| acpi_projection_error("the captured IVRS raw bytes are absent"))?;
        let fadt_raw = fadt_capture
            .table_raw
            .as_deref()
            .ok_or_else(|| acpi_projection_error("the captured FADT raw bytes are absent"))?;

        let madt = match parse_whitelisted_table(madt_raw, &limits).map_err(|error| {
            acpi_projection_parser_error(
                "the captured MADT could not be projected into evidence",
                "MADT/APIC-projection",
                error,
            )
        })? {
            WhitelistedTable::Madt(table) => table,
            _ => return Err(acpi_projection_error("the captured APIC table is not MADT")),
        };
        let mcfg = match parse_whitelisted_table(mcfg_raw, &limits).map_err(|error| {
            acpi_projection_parser_error(
                "the captured MCFG could not be projected into evidence",
                "MCFG-projection",
                error,
            )
        })? {
            WhitelistedTable::Mcfg(table) => table,
            _ => {
                return Err(acpi_projection_error(
                    "the captured MCFG table changed kind",
                ));
            }
        };
        let ivrs = match parse_whitelisted_table(ivrs_raw, &limits).map_err(|error| {
            acpi_projection_parser_error(
                "the captured IVRS could not be projected into evidence",
                "IVRS-projection",
                error,
            )
        })? {
            WhitelistedTable::Ivrs(table) => table,
            _ => {
                return Err(acpi_projection_error(
                    "the captured IVRS table changed kind",
                ));
            }
        };
        let fadt = match parse_whitelisted_table(fadt_raw, &limits).map_err(|error| {
            acpi_projection_parser_error(
                "the captured FADT could not be projected into evidence",
                "FADT/FACP-projection",
                error,
            )
        })? {
            WhitelistedTable::Fadt(table) => table,
            _ => return Err(acpi_projection_error("the captured FACP table is not FADT")),
        };

        let mut madt_entries = Vec::new();
        madt_entries
            .try_reserve_exact(madt.entries.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for entry in &madt.entries {
            let raw = entry.raw_bytes();
            let projected = match entry.entry_type {
                0 => MadtEntryEvidence::ProcessorLocalApic {
                    offset: entry.offset,
                    raw,
                    acpi_processor_uid: *raw.get(2).ok_or_else(|| {
                        acpi_projection_error("a MADT Local APIC entry is truncated")
                    })?,
                    apic_id: *raw.get(3).ok_or_else(|| {
                        acpi_projection_error("a MADT Local APIC entry is truncated")
                    })?,
                    flags: read_projection_u32(raw, 4)?,
                },
                9 => MadtEntryEvidence::ProcessorLocalX2Apic {
                    offset: entry.offset,
                    raw,
                    reserved: read_projection_u16(raw, 2)?,
                    x2apic_id: read_projection_u32(raw, 4)?,
                    flags: read_projection_u32(raw, 8)?,
                    acpi_processor_uid: read_projection_u32(raw, 12)?,
                },
                entry_type => MadtEntryEvidence::Other {
                    entry_type,
                    offset: entry.offset,
                    raw,
                },
            };
            madt_entries.push(projected);
        }

        let mut mcfg_allocations = Vec::new();
        mcfg_allocations
            .try_reserve_exact(mcfg.allocations.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for (index, allocation) in mcfg.allocations.iter().enumerate() {
            let offset = index
                .checked_mul(16)
                .and_then(|value| MCFG_FIXED_LENGTH.checked_add(value))
                .ok_or_else(|| acpi_projection_error("an MCFG allocation offset overflowed"))?;
            mcfg_allocations.push(McfgAllocationEvidence {
                offset,
                base_address: allocation.base_address,
                segment_group: allocation.segment_group,
                start_bus: allocation.start_bus,
                end_bus: allocation.end_bus,
                reserved: read_projection_array::<4>(allocation.raw_bytes(), 12)?,
                window_end_exclusive: allocation.address_range.end_exclusive,
            });
        }

        let ivhd_count = ivrs
            .entries
            .iter()
            .filter(|entry| matches!(entry, svmvisor_m0b_probe::acpi::IvrsEntry::Ivhd(_)))
            .count();
        let mut ivhd_device_lists = Vec::<Vec<IvhdDeviceEntryEvidence<'_>>>::new();
        ivhd_device_lists
            .try_reserve_exact(ivhd_count)
            .map_err(|_| acpi_projection_allocation_error())?;
        for block in &ivrs.entries {
            if let svmvisor_m0b_probe::acpi::IvrsEntry::Ivhd(ivhd) = block {
                let mut devices = Vec::new();
                devices
                    .try_reserve_exact(ivhd.device_entries.len())
                    .map_err(|_| acpi_projection_allocation_error())?;
                for device in &ivhd.device_entries {
                    let raw = device.raw_bytes();
                    let uid_length = if device.entry_type == 0xf0 {
                        Some(*raw.get(21).ok_or_else(|| {
                            acpi_projection_error("an IVHD F0h device entry is truncated")
                        })?)
                    } else {
                        None
                    };
                    devices.push(IvhdDeviceEntryEvidence {
                        offset: device.offset,
                        entry_type: device.entry_type,
                        raw,
                        uid_length,
                    });
                }
                ivhd_device_lists.push(devices);
            }
        }
        let mut ivrs_blocks = Vec::new();
        ivrs_blocks
            .try_reserve_exact(ivrs.entries.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        let mut ivhd_index = 0_usize;
        for block in &ivrs.entries {
            match block {
                svmvisor_m0b_probe::acpi::IvrsEntry::Ivhd(ivhd) => {
                    let devices = ivhd_device_lists.get(ivhd_index).ok_or_else(|| {
                        acpi_projection_error("the IVHD device projection is incomplete")
                    })?;
                    ivhd_index += 1;
                    ivrs_blocks.push(IvrsBlockEvidence::Ivhd(IvhdEvidence {
                        offset: ivhd.offset,
                        entry_type: ivhd.entry_type,
                        flags: ivhd.flags,
                        raw: ivhd.raw_bytes(),
                        header_length: if ivhd.entry_type == 0x10 { 24 } else { 40 },
                        device_id: ivhd.device_id,
                        capability_offset: ivhd.capability_offset,
                        iommu_base_address: ivhd.iommu_base_address,
                        pci_segment_group: ivhd.pci_segment_group,
                        iommu_info: ivhd.iommu_info,
                        feature_info: ivhd.feature_info,
                        extended_feature_image: ivhd.extended_feature_image,
                        extended_feature_image_2: ivhd.extended_feature_image_2,
                        device_entries: devices,
                    }));
                }
                svmvisor_m0b_probe::acpi::IvrsEntry::Ivmd(ivmd) => {
                    ivrs_blocks.push(IvrsBlockEvidence::Ivmd(IvmdEvidence {
                        offset: ivmd.offset,
                        entry_type: ivmd.entry_type,
                        flags: ivmd.flags,
                        raw: ivmd.raw_bytes(),
                        device_id: ivmd.device_id,
                        auxiliary_data_or_end_device_id: ivmd.auxiliary_data_or_end_device_id,
                        pci_segment_group: ivmd.pci_segment_group,
                        reserved_or_segment_area: ivmd.reserved_or_segment_area,
                        start_address: ivmd.start_address,
                        memory_length: ivmd.memory_length,
                        memory_end_exclusive: ivmd.memory_range.end_exclusive,
                    }));
                }
            }
        }

        let mut mp_enabled_ids = Vec::new();
        mp_enabled_ids
            .try_reserve_exact(mp_processors.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for processor in mp_processors.iter().filter(|processor| processor.enabled) {
            mp_enabled_ids.push(processor.processor_id);
        }
        mp_enabled_ids.sort_unstable();
        let mut madt_enabled_ids = Vec::new();
        madt_enabled_ids
            .try_reserve_exact(madt.processors.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for processor in madt
            .processors
            .iter()
            .filter(|processor| processor.enabled())
        {
            madt_enabled_ids.push(u64::from(processor.apic_id));
        }
        madt_enabled_ids.sort_unstable();
        if captured.topology.mp_enabled_count != mp_enabled_ids.len()
            || captured.topology.madt_enabled_count != madt_enabled_ids.len()
        {
            return Err(acpi_projection_error(
                "the topology counts changed between validation and evidence projection",
            ));
        }
        let mut missing_from_madt = Vec::new();
        missing_from_madt
            .try_reserve_exact(mp_enabled_ids.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for processor_id in &mp_enabled_ids {
            if madt_enabled_ids.binary_search(processor_id).is_err() {
                missing_from_madt.push(*processor_id);
            }
        }
        let mut missing_from_mp = Vec::new();
        missing_from_mp
            .try_reserve_exact(madt_enabled_ids.len())
            .map_err(|_| acpi_projection_allocation_error())?;
        for processor_id in &madt_enabled_ids {
            if mp_enabled_ids.binary_search(processor_id).is_err() {
                missing_from_mp.push(*processor_id);
            }
        }
        let topology_consistent = missing_from_madt.is_empty()
            && missing_from_mp.is_empty()
            && mp_enabled_ids.len() == madt_enabled_ids.len();

        let fadt_iapc_boot_arch = fadt
            .iapc_boot_arch
            .ok_or_else(|| acpi_projection_error("the captured FADT lacks IAPC_BOOT_ARCH"))?;
        let fadt_minor_version = fadt
            .minor_version
            .ok_or_else(|| acpi_projection_error("the captured FADT lacks minor version"))?;
        let fadt_x_firmware_ctrl = fadt
            .x_firmware_ctrl
            .ok_or_else(|| acpi_projection_error("the captured FADT lacks X_FIRMWARE_CTRL"))?;
        let fadt_x_dsdt = fadt
            .x_dsdt
            .ok_or_else(|| acpi_projection_error("the captured FADT lacks X_DSDT"))?;

        let acpi = AcpiEvidence {
            selection_address: captured.rsdp_address,
            rsdp: RsdpEvidence {
                address: captured.rsdp_address,
                raw: RawBytesEvidence {
                    bytes: rsdp.raw_bytes(),
                },
                checksum: rsdp.checksum,
                oem_id: rsdp.oem_id,
                revision: rsdp.revision,
                length: rsdp.length,
                rsdt_address: rsdp.rsdt_address,
                xsdt_address,
                extended_checksum,
                reserved: rsdp_reserved,
            },
            rsdt: AcpiRootEvidence {
                address: captured.rsdt.address,
                raw: RawBytesEvidence {
                    bytes: rsdt.raw_bytes(),
                },
                header: project_sdt_header(rsdt.sdt.header),
                entries: &rsdt.table_pointers,
            },
            xsdt: AcpiRootEvidence {
                address: captured.xsdt.address,
                raw: RawBytesEvidence {
                    bytes: xsdt.raw_bytes(),
                },
                header: project_sdt_header(xsdt.sdt.header),
                entries: &xsdt.table_pointers,
            },
            directory: &directory_records,
            tables: AcpiTablesEvidence {
                madt: AcpiTableEvidence {
                    address: madt_capture.address,
                    referenced_by: project_root_references(&madt_capture.referenced_by)?,
                    raw: RawBytesEvidence {
                        bytes: madt.raw_bytes(),
                    },
                    header: project_sdt_header(madt.sdt.header),
                    body: MadtBodyEvidence {
                        local_apic_address: madt.local_apic_address,
                        flags: madt.flags,
                        entries: &madt_entries,
                    },
                },
                mcfg: AcpiTableEvidence {
                    address: mcfg_capture.address,
                    referenced_by: project_root_references(&mcfg_capture.referenced_by)?,
                    raw: RawBytesEvidence {
                        bytes: mcfg.raw_bytes(),
                    },
                    header: project_sdt_header(mcfg.sdt.header),
                    body: McfgBodyEvidence {
                        reserved: mcfg.reserved,
                        allocations: &mcfg_allocations,
                    },
                },
                ivrs: AcpiTableEvidence {
                    address: ivrs_capture.address,
                    referenced_by: project_root_references(&ivrs_capture.referenced_by)?,
                    raw: RawBytesEvidence {
                        bytes: ivrs.raw_bytes(),
                    },
                    header: project_sdt_header(ivrs.sdt.header),
                    body: IvrsBodyEvidence {
                        iv_info: ivrs.iv_info,
                        reserved: ivrs.reserved,
                        blocks: &ivrs_blocks,
                    },
                },
                fadt: AcpiTableEvidence {
                    address: fadt_capture.address,
                    referenced_by: project_root_references(&fadt_capture.referenced_by)?,
                    raw: RawBytesEvidence {
                        bytes: fadt.raw_bytes(),
                    },
                    header: project_sdt_header(fadt.sdt.header),
                    body: FadtBodyEvidence {
                        firmware_ctrl_32: fadt.firmware_ctrl,
                        dsdt_32: fadt.dsdt,
                        preferred_pm_profile: fadt.preferred_pm_profile,
                        sci_interrupt: fadt.sci_interrupt,
                        iapc_boot_arch: fadt_iapc_boot_arch,
                        flags: fadt.flags,
                        minor_version: fadt_minor_version,
                        x_firmware_ctrl: fadt_x_firmware_ctrl,
                        x_dsdt: fadt_x_dsdt,
                    },
                },
            },
            madt_mp_cross_check: MadtMpCrossCheckEvidence {
                mp_enabled_ids: &mp_enabled_ids,
                madt_enabled_ids: &madt_enabled_ids,
                missing_from_madt: &missing_from_madt,
                missing_from_mp: &missing_from_mp,
                consistent: topology_consistent,
            },
        };

        Ok(callback(acpi))
    }

    fn collect_timestamp() -> Result<TimestampRecord, ProbeError> {
        let time = runtime::get_time().map_err(|error| {
            ProbeError::new(
                error.status(),
                "UEFI time is unavailable; a collision-resistant evidence path cannot be formed",
            )
        })?;
        Ok(TimestampRecord {
            year: time.year(),
            month: time.month(),
            day: time.day(),
            hour: time.hour(),
            minute: time.minute(),
            second: time.second(),
            nanosecond: time.nanosecond(),
            timezone_minutes: time.time_zone(),
        })
    }

    /// Collect the BSP directly and each enabled AP through one blocking,
    /// sequential StartupThisAP call with no MP Services timeout. No AP is enabled,
    /// disabled, or promoted to BSP, and an incomplete or ambiguously identified
    /// dispatch aborts the run.
    fn collect_processor_inventory() -> Result<ProcessorInventoryCollection, ProbeError> {
        let handle = boot::get_handle_for_protocol::<MpServices>().map_err(|error| {
            ProbeError::new(
                error.status(),
                "MP Services is required for the per-processor inventory",
            )
        })?;
        let protocol_scope = open_protocol_get::<MpServices>(handle).map_err(|error| {
            ProbeError::new(
                error.status(),
                "the MP Services protocol could not be opened read-only",
            )
        })?;
        let protocol = protocol_scope.get().ok_or_else(|| {
            ProbeError::new(
                Status::UNSUPPORTED,
                "the MP Services protocol returned a null interface",
            )
        })?;
        let count = protocol.get_number_of_processors().map_err(|error| {
            ProbeError::new(
                error.status(),
                "MP Services did not return processor counts",
            )
        })?;
        if count.total == 0
            || count.total > MAX_MP_PROCESSORS
            || count.enabled == 0
            || count.enabled > count.total
        {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services returned out-of-policy processor counts",
            ));
        }

        let mut records = Vec::new();
        records.try_reserve_exact(count.total).map_err(|_| {
            ProbeError::new(
                Status::OUT_OF_RESOURCES,
                "the bounded MP Services record allocation failed",
            )
        })?;
        for processor_number in 0..count.total {
            let information = protocol
                .get_processor_info(processor_number)
                .map_err(|error| {
                    ProbeError::new(
                        error.status(),
                        "MP Services processor information is incomplete",
                    )
                })?;
            records.push(ProcessorRecord {
                processor_number,
                processor_id: information.processor_id,
                is_bsp: information.is_bsp(),
                enabled: information.is_enabled(),
                healthy: information.is_healthy(),
                package: information.location.package,
                core: information.location.core,
                thread: information.location.thread,
            });
        }
        for (index, record) in records.iter().enumerate() {
            if records[..index]
                .iter()
                .any(|prior| prior.processor_id == record.processor_id)
            {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "MP Services returned a duplicate processor hardware ID",
                ));
            }
        }

        let enabled_records = records.iter().filter(|record| record.enabled).count();
        let bsp_records = records.iter().filter(|record| record.is_bsp).count();
        let bsp_processor_number = protocol.who_am_i().map_err(|error| {
            ProbeError::new(error.status(), "MP Services WhoAmI failed on the BSP")
        })?;
        if enabled_records != count.enabled
            || bsp_records != 1
            || bsp_processor_number >= records.len()
            || !records[bsp_processor_number].is_bsp
            || !records[bsp_processor_number].enabled
        {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services returned an inconsistent BSP or enabled-processor snapshot",
            ));
        }
        if records
            .iter()
            .any(|record| record.enabled && !record.healthy)
        {
            return Err(ProbeError::new(
                Status::DEVICE_ERROR,
                "refusing to dispatch an enabled processor marked unhealthy by firmware",
            ));
        }

        let mut observations = Vec::new();
        observations.try_reserve_exact(count.enabled).map_err(|_| {
            ProbeError::new(
                Status::OUT_OF_RESOURCES,
                "the bounded per-processor observation allocation failed",
            )
        })?;
        for record in records.iter().filter(|record| record.enabled) {
            let (dispatch, who_am_i_processor_number, measurement) = if record.is_bsp {
                (
                    ProcessorDispatch::BspDirect,
                    bsp_processor_number,
                    collect_current_processor_measurement(),
                )
            } else {
                let (reported_processor_number, measurement) =
                    collect_one_ap(protocol, record.processor_number)?;
                (
                    ProcessorDispatch::StartupThisApSuccess,
                    reported_processor_number,
                    measurement,
                )
            };
            observations.push(ProcessorObservation {
                processor_number: record.processor_number,
                processor_id: record.processor_id,
                who_am_i_processor_number,
                is_bsp: record.is_bsp,
                dispatch,
                cpu: measurement.cpu,
                vm_cr: measurement.vm_cr,
                system_registers: measurement.system_registers,
            });
        }
        if observations.len() != count.enabled {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "not every enabled processor produced one complete observation",
            ));
        }
        revalidate_processor_snapshot(
            protocol,
            count.total,
            count.enabled,
            bsp_processor_number,
            &records,
        )?;

        Ok(ProcessorInventoryCollection {
            total: count.total,
            enabled: count.enabled,
            bsp_processor_number,
            records,
            observations,
        })
    }

    #[allow(unsafe_code)]
    fn collect_one_ap(
        protocol: &MpServices,
        processor_number: usize,
    ) -> Result<(usize, CurrentProcessorMeasurement), ProbeError> {
        let slot = ApMeasurementSlot::new(protocol as *const MpServices);
        let argument = (&slot as *const ApMeasurementSlot)
            .cast_mut()
            .cast::<c_void>();
        protocol
            .startup_this_ap(
                processor_number,
                collect_ap_measurement,
                argument,
                None,
                // The same closed policy supplies both this UEFI argument and
                // the raw evidence value. F7's finite-timeout recovery may
                // reset the target AP with INIT/SIPI.
                AP_MEASUREMENT_TIMEOUT_POLICY.uefi_timeout(),
            )
            .map_err(|error| {
                ProbeError::new(
                    error.status(),
                    "a blocking StartupThisAP measurement did not complete",
                )
            })?;
        if slot.completion.load(Ordering::Acquire) != 1 {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "StartupThisAP returned without a published measurement",
            ));
        }

        // SAFETY: The AP initialized `result` before its release-store to
        // completion. The acquire-load above observed that store, and the
        // blocking StartupThisAP call has returned, so the AP no longer uses the
        // stack-backed slot.
        let result = unsafe { (*slot.result.get()).assume_init_read() };
        let Some(reported_processor_number) = result.reported_processor_number else {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "the AP callback identity did not match its StartupThisAP target",
            ));
        };
        if result.who_am_i_status.is_some() || reported_processor_number != processor_number {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "the AP callback identity did not match its StartupThisAP target",
            ));
        }
        Ok((reported_processor_number, result.measurement))
    }

    /// Bracket dispatch with a second read-only MP Services enumeration. PI
    /// permits processor information to change during a boot session, so a
    /// schema-v3 record is emitted only when the count, every processor record,
    /// and BSP identity still match the pre-dispatch enumeration.
    fn revalidate_processor_snapshot(
        protocol: &MpServices,
        expected_total: usize,
        expected_enabled: usize,
        expected_bsp_processor_number: usize,
        expected_records: &[ProcessorRecord],
    ) -> Result<(), ProbeError> {
        let count = protocol.get_number_of_processors().map_err(|error| {
            ProbeError::new(
                error.status(),
                "MP Services post-dispatch processor recount failed",
            )
        })?;
        if count.total != expected_total
            || count.enabled != expected_enabled
            || expected_records.len() != expected_total
        {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services processor counts changed during dispatch",
            ));
        }

        for expected in expected_records {
            let information = protocol
                .get_processor_info(expected.processor_number)
                .map_err(|error| {
                    ProbeError::new(
                        error.status(),
                        "MP Services post-dispatch processor information is incomplete",
                    )
                })?;
            let observed = ProcessorRecord {
                processor_number: expected.processor_number,
                processor_id: information.processor_id,
                is_bsp: information.is_bsp(),
                enabled: information.is_enabled(),
                healthy: information.is_healthy(),
                package: information.location.package,
                core: information.location.core,
                thread: information.location.thread,
            };
            if observed != *expected {
                return Err(ProbeError::new(
                    Status::COMPROMISED_DATA,
                    "MP Services processor information changed during dispatch",
                ));
            }
        }

        let bsp_processor_number = protocol.who_am_i().map_err(|error| {
            ProbeError::new(
                error.status(),
                "MP Services post-dispatch BSP WhoAmI failed",
            )
        })?;
        if bsp_processor_number != expected_bsp_processor_number {
            return Err(ProbeError::new(
                Status::COMPROMISED_DATA,
                "MP Services BSP identity changed during dispatch",
            ));
        }
        Ok(())
    }

    /// AP callback: one AP-safe WhoAmI call, bounded CPUID, the conditional
    /// named VM_CR read, and the same allowlisted system-register reads. It
    /// performs no allocation, formatting, logging, filesystem access, or
    /// control-state write.
    #[allow(unsafe_code)]
    extern "efiapi" fn collect_ap_measurement(argument: *mut c_void) {
        if argument.is_null() {
            return;
        }
        // SAFETY: The BSP passes a live, uniquely assigned slot to one blocking
        // StartupThisAP call. The protocol scope and slot both outlive this
        // callback, and the BSP does not inspect result until completion.
        let slot = unsafe { &*argument.cast::<ApMeasurementSlot>() };
        let Some(protocol) = (unsafe { slot.mp_services.as_ref() }) else {
            return;
        };
        let (reported_processor_number, who_am_i_status) = match protocol.who_am_i() {
            Ok(number) => (Some(number), None),
            Err(error) => (None, Some(error.status().0)),
        };
        let result = ApMeasurementResult {
            reported_processor_number,
            who_am_i_status,
            measurement: collect_current_processor_measurement(),
        };
        // SAFETY: This callback is the only writer and the BSP waits for the
        // release/acquire completion handoff before reading the initialized
        // result.
        unsafe { (*slot.result.get()).write(result) };
        slot.completion.store(1, Ordering::Release);
    }

    fn collect_current_processor_measurement() -> CurrentProcessorMeasurement {
        let mut cpuid_source = HardwareCpuid;
        let cpu = collect_cpuid(&mut cpuid_source);
        let vm_cr = match read_vm_cr_if_enumerated(&cpu) {
            Some(raw) => VmCrEvidence::Observed(raw),
            None => VmCrEvidence::NotAttempted {
                reason: "cpu-is-not-authentic-amd-or-svm-is-not-enumerated",
            },
        };
        let system_registers = collect_system_registers(&cpu);
        CurrentProcessorMeasurement {
            cpu,
            vm_cr,
            system_registers,
        }
    }

    type MemoryMapCollection = (
        Vec<MemoryDescriptorRecord>,
        Option<(usize, u32)>,
        Option<usize>,
    );

    fn collect_memory_map() -> MemoryMapCollection {
        let map = match boot::memory_map(MemoryType::LOADER_DATA) {
            Ok(map) => map,
            Err(error) => return (Vec::new(), None, Some(error.status().0)),
        };
        let meta = map.meta();
        let mut descriptors = Vec::new();
        if descriptors
            .try_reserve_exact(MAX_MEMORY_DESCRIPTORS)
            .is_err()
        {
            return (Vec::new(), None, Some(Status::OUT_OF_RESOURCES.0));
        }
        for descriptor in map.entries() {
            if descriptors.len() >= MAX_MEMORY_DESCRIPTORS {
                return (Vec::new(), None, Some(Status::COMPROMISED_DATA.0));
            }
            descriptors.push(MemoryDescriptorRecord {
                memory_type: descriptor.ty.0,
                physical_start: descriptor.phys_start,
                virtual_start: descriptor.virt_start,
                page_count: descriptor.page_count,
                attributes: descriptor.att.bits(),
            });
        }
        (descriptors, Some((meta.desc_size, meta.desc_version)), None)
    }

    /// Read AMD VM_CR only after checking the architectural enumeration gates.
    ///
    /// The UEFI application executes at CPL0. `should_read_vm_cr` additionally
    /// requires AuthenticAMD, the SVM CPUID bit, and the SVM capability leaf. AMD
    /// APM Volume 2 rev. 3.44 section 15.30.1 defines MSR C001_0114h and guarantees
    /// its availability independently of EFER.SVME.
    ///
    /// This is deliberately not inlined: BSP and AP callers must share the one
    /// allowlisted RDMSR instruction site checked in the final PE image.
    #[inline(never)]
    #[allow(unsafe_code)]
    fn read_vm_cr_if_enumerated(cpu: &svmvisor_m0b_probe::CpuInventory) -> Option<u64> {
        if !should_read_vm_cr(cpu) {
            return None;
        }

        let low: u32;
        let high: u32;
        // SAFETY: The checks above restrict this named read to an architecturally
        // enumerated AMD VM_CR MSR. RDMSR has no write side effect.
        unsafe {
            asm!(
                "rdmsr",
                in("ecx") VM_CR_MSR,
                out("eax") low,
                out("edx") high,
                options(nomem, nostack, preserves_flags),
            );
        }
        MSR_READ_OPERATIONS.fetch_add(1, Ordering::Relaxed);
        MSR_READ_BYTES.fetch_add(8, Ordering::Relaxed);
        Some((u64::from(high) << 32) | u64::from(low))
    }

    /// Run-wide counters covering every allowlisted `RDMSR` site, including the
    /// pre-existing VM_CR site. Incremented only inside site functions, so the
    /// totals cannot drift from the executed reads. No write counterpart exists.
    static MSR_READ_OPERATIONS: AtomicU64 = AtomicU64::new(0);
    static MSR_READ_BYTES: AtomicU64 = AtomicU64::new(0);

    /// One named, non-inlined, counter-instrumented `RDMSR` site per reviewed
    /// MSR address. Each generated function carries its literal address in a
    /// `mov ecx, imm` immediately before `rdmsr`, so the PE/instruction safety
    /// gate can pin every site to its reviewed address. The address constants
    /// come from the pure `msr` module so code and policy cannot drift.
    macro_rules! system_register_read_site {
        ($name:ident, $msr:expr) => {
            #[inline(never)]
            #[allow(unsafe_code)]
            fn $name() -> u64 {
                let low: u32;
                let high: u32;
                // SAFETY: This is one reviewed read-only allowlisted site from
                // docs/m0b-msr-read-policy.md. The address is architecturally
                // enumerated by the caller's gates (AMD plus SVM capability for
                // every site, MTRRcap for the MTRR families, and the pinned-PPR
                // family/model for the IORR quartet). RDMSR at CPL0 has no
                // write side effect.
                unsafe {
                    asm!(
                        "rdmsr",
                        in("ecx") $msr,
                        out("eax") low,
                        out("edx") high,
                        options(nomem, nostack, preserves_flags),
                    );
                }
                MSR_READ_OPERATIONS.fetch_add(1, Ordering::Relaxed);
                MSR_READ_BYTES.fetch_add(8, Ordering::Relaxed);
                (u64::from(high) << 32) | u64::from(low)
            }
        };
    }

    system_register_read_site!(read_mtrr_cap, msr::MTRR_CAP_MSR);
    system_register_read_site!(read_mtrr_def_type, msr::MTRR_DEF_TYPE_MSR);
    system_register_read_site!(read_pat, msr::PAT_MSR);
    system_register_read_site!(read_mtrr_phys_base_0, msr::MTRR_PHYS_BASE_0_MSR);
    system_register_read_site!(read_mtrr_phys_mask_0, msr::MTRR_PHYS_BASE_0_MSR + 1);
    system_register_read_site!(read_mtrr_phys_base_1, msr::MTRR_PHYS_BASE_0_MSR + 2);
    system_register_read_site!(read_mtrr_phys_mask_1, msr::MTRR_PHYS_BASE_0_MSR + 3);
    system_register_read_site!(read_mtrr_phys_base_2, msr::MTRR_PHYS_BASE_0_MSR + 4);
    system_register_read_site!(read_mtrr_phys_mask_2, msr::MTRR_PHYS_BASE_0_MSR + 5);
    system_register_read_site!(read_mtrr_phys_base_3, msr::MTRR_PHYS_BASE_0_MSR + 6);
    system_register_read_site!(read_mtrr_phys_mask_3, msr::MTRR_PHYS_BASE_0_MSR + 7);
    system_register_read_site!(read_mtrr_phys_base_4, msr::MTRR_PHYS_BASE_0_MSR + 8);
    system_register_read_site!(read_mtrr_phys_mask_4, msr::MTRR_PHYS_BASE_0_MSR + 9);
    system_register_read_site!(read_mtrr_phys_base_5, msr::MTRR_PHYS_BASE_0_MSR + 10);
    system_register_read_site!(read_mtrr_phys_mask_5, msr::MTRR_PHYS_BASE_0_MSR + 11);
    system_register_read_site!(read_mtrr_phys_base_6, msr::MTRR_PHYS_BASE_0_MSR + 12);
    system_register_read_site!(read_mtrr_phys_mask_6, msr::MTRR_PHYS_BASE_0_MSR + 13);
    system_register_read_site!(read_mtrr_phys_base_7, msr::MTRR_PHYS_BASE_0_MSR + 14);
    system_register_read_site!(read_mtrr_phys_mask_7, msr::MTRR_PHYS_BASE_0_MSR + 15);
    system_register_read_site!(read_mtrr_fix_64k_00000, msr::MTRR_FIX_64K_00000_MSR);
    system_register_read_site!(read_mtrr_fix_16k_80000, msr::MTRR_FIX_16K_80000_MSR);
    system_register_read_site!(read_mtrr_fix_16k_a0000, msr::MTRR_FIX_16K_A0000_MSR);
    system_register_read_site!(read_mtrr_fix_4k_0, msr::MTRR_FIX_4K_00000_MSR);
    system_register_read_site!(read_mtrr_fix_4k_1, msr::MTRR_FIX_4K_00000_MSR + 1);
    system_register_read_site!(read_mtrr_fix_4k_2, msr::MTRR_FIX_4K_00000_MSR + 2);
    system_register_read_site!(read_mtrr_fix_4k_3, msr::MTRR_FIX_4K_00000_MSR + 3);
    system_register_read_site!(read_mtrr_fix_4k_4, msr::MTRR_FIX_4K_00000_MSR + 4);
    system_register_read_site!(read_mtrr_fix_4k_5, msr::MTRR_FIX_4K_00000_MSR + 5);
    system_register_read_site!(read_mtrr_fix_4k_6, msr::MTRR_FIX_4K_00000_MSR + 6);
    system_register_read_site!(read_mtrr_fix_4k_7, msr::MTRR_FIX_4K_00000_MSR + 7);
    system_register_read_site!(read_sys_cfg, msr::SYS_CFG_MSR);
    system_register_read_site!(read_hwcr, msr::HWCR_MSR);
    system_register_read_site!(read_iorr_base_0, msr::IORR_BASE_0_MSR);
    system_register_read_site!(read_iorr_mask_0, msr::IORR_MASK_0_MSR);
    system_register_read_site!(read_iorr_base_1, msr::IORR_BASE_1_MSR);
    system_register_read_site!(read_iorr_mask_1, msr::IORR_MASK_1_MSR);
    system_register_read_site!(read_top_mem, msr::TOP_MEM_MSR);
    system_register_read_site!(read_tom2, msr::TOM2_MSR);
    system_register_read_site!(read_smm_base, msr::SMM_BASE_MSR);
    system_register_read_site!(read_smm_addr, msr::SMM_ADDR_MSR);
    system_register_read_site!(read_smm_mask, msr::SMM_MASK_MSR);

    const VARIABLE_MTRR_READERS: [(fn() -> u64, fn() -> u64); msr::MAX_VARIABLE_MTRR_PAIRS] = [
        (read_mtrr_phys_base_0, read_mtrr_phys_mask_0),
        (read_mtrr_phys_base_1, read_mtrr_phys_mask_1),
        (read_mtrr_phys_base_2, read_mtrr_phys_mask_2),
        (read_mtrr_phys_base_3, read_mtrr_phys_mask_3),
        (read_mtrr_phys_base_4, read_mtrr_phys_mask_4),
        (read_mtrr_phys_base_5, read_mtrr_phys_mask_5),
        (read_mtrr_phys_base_6, read_mtrr_phys_mask_6),
        (read_mtrr_phys_base_7, read_mtrr_phys_mask_7),
    ];

    const FIXED_MTRR_READERS: [fn() -> u64; msr::FIXED_MTRR_COUNT] = [
        read_mtrr_fix_64k_00000,
        read_mtrr_fix_16k_80000,
        read_mtrr_fix_16k_a0000,
        read_mtrr_fix_4k_0,
        read_mtrr_fix_4k_1,
        read_mtrr_fix_4k_2,
        read_mtrr_fix_4k_3,
        read_mtrr_fix_4k_4,
        read_mtrr_fix_4k_5,
        read_mtrr_fix_4k_6,
        read_mtrr_fix_4k_7,
    ];

    const IORR_READERS: [(fn() -> u64, fn() -> u64); msr::IORR_RANGE_COUNT] = [
        (read_iorr_base_0, read_iorr_mask_0),
        (read_iorr_base_1, read_iorr_mask_1),
    ];

    /// Collect the reviewed system-register allowlist on the current processor.
    ///
    /// Allocation-free and shared by the BSP path and the AP callback. Every
    /// gate is checked before its reads; a `VCNT` beyond the reviewed site
    /// bound records zero pairs and lets strict preflight fail the capture
    /// closed rather than reading outside the allowlist.
    fn collect_system_registers(
        cpu: &svmvisor_m0b_probe::CpuInventory,
    ) -> SystemRegistersEvidence<'static> {
        if !msr::should_read_system_registers(cpu) {
            return SystemRegistersEvidence::NotAttempted {
                reason: "cpu-is-not-authentic-amd-or-svm-is-not-enumerated",
            };
        }
        let mtrr_cap = read_mtrr_cap();
        let capability = msr::decode_mtrr_cap(mtrr_cap);
        let pair_count = capability.map(|cap| cap.vcnt).unwrap_or(0);
        let fixed_supported = capability.map(|cap| cap.fix).unwrap_or(false);

        let mut variable_mtrr_pairs =
            [VariableMtrrPair { base: 0, mask: 0 }; msr::MAX_VARIABLE_MTRR_PAIRS];
        let read_pairs = usize::from(pair_count).min(msr::MAX_VARIABLE_MTRR_PAIRS);
        for (index, pair) in variable_mtrr_pairs.iter_mut().enumerate() {
            if index < read_pairs {
                let (read_base, read_mask) = VARIABLE_MTRR_READERS[index];
                *pair = VariableMtrrPair {
                    base: read_base(),
                    mask: read_mask(),
                };
            }
        }

        let mut fixed_mtrr = [0_u64; msr::FIXED_MTRR_COUNT];
        if fixed_supported {
            for (index, register) in fixed_mtrr.iter_mut().enumerate() {
                *register = FIXED_MTRR_READERS[index]();
            }
        }

        let mut iorr_base = [0_u64; msr::IORR_RANGE_COUNT];
        let mut iorr_mask = [0_u64; msr::IORR_RANGE_COUNT];
        let iorr_not_attempted_reason = if msr::iorr_documented_by_pinned_ppr(cpu) {
            for (index, (read_base, read_mask)) in IORR_READERS.iter().enumerate() {
                iorr_base[index] = read_base();
                iorr_mask[index] = read_mask();
            }
            None
        } else {
            Some("cpu-family-model-not-documented-by-pinned-ppr")
        };

        SystemRegistersEvidence::Observed(SystemRegisterInventory {
            mtrr_cap,
            mtrr_def_type: read_mtrr_def_type(),
            pat: read_pat(),
            variable_mtrr_pairs,
            variable_mtrr_pair_count: pair_count,
            fixed_mtrr,
            fixed_mtrr_observed: fixed_supported,
            sys_cfg: read_sys_cfg(),
            hwcr: read_hwcr(),
            top_mem: read_top_mem(),
            tom2: read_tom2(),
            smm_base: read_smm_base(),
            smm_addr: read_smm_addr(),
            smm_mask: read_smm_mask(),
            iorr_base,
            iorr_mask,
            iorr_not_attempted_reason,
        })
    }
}
