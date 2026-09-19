//! The admitted first-boot PCI endpoint: its wire description and configuration reads.

pub const PCI_VENDOR_DEVICE: u32 = 0x0666_10ee;
pub const PCI_CLASS_REVISION: u32 = 0xff00_0003;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalEndpoint {
    pub config_page: u64,
    pub bar0_host_page: u64,
    pub fpga_build_id: u64,
    pub rom_build_id: u64,
    pub mmio_config_msr: u64,
    pub bar0_raw: u32,
    pub segment_bdf: u32,
    pub boot_id: u32,
    pub command: u16,
    pub version: u8,
    pub reserved: u8,
}

const _: () = {
    assert!(core::mem::size_of::<TerminalEndpoint>() == 56);
    assert!(core::mem::offset_of!(TerminalEndpoint, mmio_config_msr) == 32);
    assert!(core::mem::offset_of!(TerminalEndpoint, bar0_raw) == 40);
    assert!(core::mem::offset_of!(TerminalEndpoint, version) == 54);
};

// `config_aperture` and `valid` are `#[inline]` so the resident payload, which links without LTO,
// keeps compiling them into the hypervisor's own code as it did when they lived there.
impl TerminalEndpoint {
    /// Complete admitted MMCONFIG aperture, including upstream bridge config.
    /// PPR57896 2.1.6.1: BusRange field gives log2(number of buses), each1MiB.
    #[inline]
    pub fn config_aperture(&self) -> Option<(u64, u64)> {
        if !self.valid() {
            return None;
        }
        Some((
            self.mmio_config_msr & 0x0000_ffff_fff0_0000,
            (1u64 << 20) << ((self.mmio_config_msr >> 2) & 15),
        ))
    }
    /// PPR57896 p210 -> pp40-41. Initial target supports segment0, <=256 buses.
    pub fn config_page_from_msr(msr: u64, segment_bdf: u32) -> Option<u64> {
        let buses = ((msr >> 2) & 15) as u32;
        if msr & !0x0000_ffff_fff0_003d != 0
            || msr & 1 == 0
            || buses > 8
            || segment_bdf > 0xffff
            || (segment_bdf >> 8) >= (1u32 << buses)
        {
            return None;
        }
        let base = msr & 0x0000_ffff_fff0_0000;
        if base & ((1u64 << (20 + buses)) - 1) != 0 {
            return None;
        }
        let page = base.checked_add(u64::from(segment_bdf) << 12)?;
        (base >= 0x100000 && page <= 0xffff_f000).then_some(page)
    }
    #[inline]
    pub fn valid(&self) -> bool {
        self.version == 1
            && self.reserved == 0
            && Self::config_page_from_msr(self.mmio_config_msr, self.segment_bdf)
                == Some(self.config_page)
            && self.bar0_host_page >= 0x100000
            && self.bar0_host_page <= 0xffff_f000
            && self.bar0_host_page & 4095 == 0
            && u64::from(self.bar0_raw) == self.bar0_host_page
            && self.config_page != self.bar0_host_page
            && self.command & 2 != 0
            && self.fpga_build_id != 0
            && self.rom_build_id != 0
    }
}

/// # Safety
/// PPR57896 rev3.00 2.1.6.1 pp40-41: `page` is an admitted UC-mapped
/// configuration function page, still routed by the validated C0010058 MSR.
/// Caller owns access lifetime; offset is a naturally aligned DWORD <=4092.
pub unsafe fn read_config_dword(page: u64, offset: u16) -> u32 {
    debug_assert!(offset <= 4092 && offset & 3 == 0);
    let value: u32;
    unsafe {
        core::arch::asm!("mov eax, dword ptr [{address}]", address=in(reg) page+u64::from(offset),
        out("eax") value, options(nostack, preserves_flags));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_route_is_enabled_bounded_segment_zero() {
        assert_eq!(TerminalEndpoint::config_page_from_msr(0xe0000021, 0x12a), Some(0xe012a000));
        for (m, b) in [
            (0xe0000020, 0),
            (0xe0000023, 0),
            (0xe0100021, 0),
            (0xe0000001, 0x100),
            (0xe0000021, 0x10000),
            (0x1e0000021, 0),
        ] {
            assert!(TerminalEndpoint::config_page_from_msr(m, b).is_none());
        }
    }
}
