//! Host-testable policy core for the record-only live AMD-IOMMU slice.
//!
//! Firmware protocol access remains in the UEFI binary adapter.  These modules
//! accept copied ACPI, PCI, MMIO, and memory-map values and perform no hardware
//! access themselves.

pub mod locator;
pub mod ranges;
pub mod registers;
