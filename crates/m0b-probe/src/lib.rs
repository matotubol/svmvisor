//! Host-testable core for the read-only Milestone 0b UEFI inventory probe.
//!
//! Firmware access stays in the binary adapter. This library accepts already
//! observed values, decodes only architecturally named fields, and emits the
//! canonical JSON record written to the selected removable evidence sink.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod acpi;
pub mod cpuid;
pub mod evidence;
pub mod iommu;
pub mod msr;
pub mod processor;

pub use cpuid::{
    CpuInventory, CpuidRegisters, CpuidSource, capability_cpuid_equal, collect_cpuid,
    cpuid_identity_matches_processor_id, extended_apic_id, initial_apic_id, should_read_vm_cr,
};
pub use evidence::{
    ConfigTableRecord, Evidence, MemoryDescriptorRecord, MemoryMapEvidence, MpServicesEvidence,
    MsrAccessEvidence, ProcessorRecord, SinkAuthorizationError, SinkRecord, SystemRegistersSection,
    TimestampRecord, VmCrEvidence, parse_profile_binding, render_json,
};
pub use msr::{
    SystemRegisterInventory, SystemRegistersEvidence, expected_read_operations,
    iorr_documented_by_pinned_ppr, should_read_system_registers, validate_inventory,
};
pub use processor::{
    AP_MEASUREMENT_TIMEOUT_POLICY, ApMeasurementTimeoutPolicy, ProcessorConsistencyEvidence,
    ProcessorDispatch, ProcessorObservation,
};
