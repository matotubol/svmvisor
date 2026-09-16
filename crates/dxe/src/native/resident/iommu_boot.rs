//! Native ACPI inventory adapter. Discovery and NPT protection do not transfer
//! IOMMU ownership or enable any interrupt route.
use super::*;
use resident::iommu::{self, FirmwareReader};
use svmvisor_hypervisor::svm::iommu::{Unit, MAX_IOMMUS};

static mut IVRS: [u8; iommu::MAX_IVRS_BYTES] = [0; iommu::MAX_IVRS_BYTES];
static mut UNITS: [Option<Unit>; MAX_IOMMUS] = [None; MAX_IOMMUS];

struct Reader<'a> { map: &'a [MemoryDescriptor], cfg: PagingConfig, mt: &'a Mtrrs, pat: u64 }
impl FirmwareReader for Reader<'_> {
    fn read(&mut self, address: u64, destination: &mut [u8]) -> Result<(), iommu::Error> {
        use iommu::Error::Address;
        let end = address.checked_add(destination.len() as u64).ok_or(Address)?;
        if destination.is_empty() || destination.len() > iommu::MAX_IVRS_BYTES { return Err(Address); }
        let mut cursor = address;
        for d in self.map {
            let last = d.physical_start.checked_add(d.page_count.checked_mul(4096).ok_or(Address)?).ok_or(Address)?;
            if last <= cursor { continue; }
            // ACPI reclaim/NVS are readable firmware RAM, never generic MMIO.
            if d.physical_start > cursor || !matches!(d.memory_type, 1..=7 | 9 | 10)
                || d.attributes & 8 == 0 || d.attributes & 0x2000 != 0 { return Err(Address); }
            cursor = last.min(end);
            if cursor == end { break; }
        }
        if cursor != end || self.pat & 255 != 6 { return Err(Address); }
        for page in (address & !4095..end).step_by(4096) {
            let translated = paging::translate(self.cfg, page, |entry_address| {
                if !ram_span(self.map, entry_address, 8) || !self.mt.page_is_wb(entry_address & !4095) {
                    return None;
                }
                // Same admitted flat firmware paging-structure reader as mapped().
                let entry = unsafe { (entry_address as *const u64).read_volatile() };
                (entry & 0x18 == 0).then_some(entry)
            }).map_err(|_| Address)?;
            if translated.physical_address != page || !self.mt.page_is_wb(page)
                || (self.pat >> (translated.pat_index * 8)) & 255 != 6 { return Err(Address); }
        }
        // Every source page was classified and its current identity mapping checked.
        unsafe { ptr::copy(address as *const u8, destination.as_mut_ptr(), destination.len()); }
        Ok(())
    }
}

/// Serialized BSP preparation, valid UEFI system table and current stable map.
/// Retain a copy before the loader can reclaim ACPI-reclaim storage. No MMIO or
/// physical IOMMU register is accessed and no DMA table is changed here.
pub(super) unsafe fn discover(table: *mut SystemTable, map: &[MemoryDescriptor],
    cfg: PagingConfig, mt: &Mtrrs, pat: u64, policy: &AddressPolicy) -> Result<(), Status>
{
    let system = unsafe { &*table };
    let count = system.number_of_configuration_table_entries;
    let tables = system.configuration_table;
    if count == 0 || count > 256 || tables.is_null()
        || tables as usize % core::mem::align_of::<uefi_raw::table::configuration::ConfigurationTable>() != 0 {
        return Err(Status::UNSUPPORTED);
    }
    unsafe { mapped(map,cfg,mt,pat,tables as u64,
        (count * core::mem::size_of::<uefi_raw::table::configuration::ConfigurationTable>()) as u64,
        false,false) }.map_err(unsupported)?;
    let tables = unsafe { core::slice::from_raw_parts(tables,count) };
    let rsdp = iommu::rsdp_address(tables).map_err(|_| Status::UNSUPPORTED)?;
    let mut reader = Reader { map, cfg, mt, pat };
    let bytes = unsafe { &mut *ptr::addr_of_mut!(IVRS) };
    let count = iommu::load_ivrs(rsdp,&mut reader,bytes).map_err(|e| { trace_detail(&e); Status::UNSUPPORTED })?;
    let inventory = iommu::discover_ivrs(&bytes[..count],policy)
        .map_err(|e| { trace_detail(&e); Status::UNSUPPORTED })?;
    inventory.admit_x2avic().map_err(|e| { trace_detail(&e); Status::UNSUPPORTED })?;
    let mut units = [None; MAX_IOMMUS];
    for (slot, description) in inventory.units().enumerate() { units[slot] = Some(description.unit); }
    unsafe { UNITS = units; }
    Ok(())
}

/// All consumers stopped, before publication of each rebuilt NPT. This excludes
/// guest access to the discovered IOMMU controls; an absent runtime completion
/// owner must stop on the resulting fault, never forward a control write.
pub(super) fn protect(npt: &mut svmvisor_hypervisor::memory::npt::IdentityNpt<'_>)
    -> Result<(), svmvisor_hypervisor::memory::npt::IdentityNptError>
{
    for unit in unsafe { &*ptr::addr_of!(UNITS) }.iter().flatten() {
        for page in (unit.mmio.base()..=unit.mmio.last_byte()).step_by(4096) {
            npt.trap_page(page)?;
        }
    }
    Ok(())
}
