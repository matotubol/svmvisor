//! SVM permission-map contents, from AMD APM vol. 2 rev. 3.44, chapter 15
//! (I/O intercepts and MSR permissions, table 15-8). A set bit intercepts access.
//!
//! These values supply aligned storage, not physical allocation, WB mappings,
//! or a live VMCB. IOIO_PROT/MSR_PROT must also be enabled by the caller.

pub const IOPM_BYTES: usize = 12 * 1024;
pub const MSRPM_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Allow,
    Intercept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsrAccess {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionMapError {
    EmptyIoRange,
    IoRangeOutsidePorts,
    UnsupportedMsr,
}

/// One bit per byte-wide port. The three architectural overrun bits and all
/// remaining padding stay intercepted, including for I/O crossing port 65535.
#[repr(C, align(4096))]
pub struct Iopm {
    bytes: [u8; IOPM_BYTES],
}

impl Iopm {
    pub const fn new() -> Self {
        Self {
            bytes: [0xff; IOPM_BYTES],
        }
    }

    pub const fn bytes(&self) -> &[u8; IOPM_BYTES] {
        &self.bytes
    }

    /// Set a contiguous number of byte-wide ports. This count describes a port
    /// range, not an instruction operand width. Invalid ranges change no bits.
    pub fn set_range(
        &mut self,
        first_port: u16,
        port_count: u32,
        permission: Permission,
    ) -> Result<(), PermissionMapError> {
        if port_count == 0 {
            return Err(PermissionMapError::EmptyIoRange);
        }
        let first = u32::from(first_port);
        if port_count > 65536 - first {
            return Err(PermissionMapError::IoRangeOutsidePorts);
        }
        for port in first..first + port_count {
            set_bit(&mut self.bytes, port as usize, permission);
        }
        Ok(())
    }
}

impl Default for Iopm {
    fn default() -> Self {
        Self::new()
    }
}

/// Two bits per covered MSR, read then write. The final 2 KiB are reserved and
/// always remain set. Unsupported MSRs intercept when MSR_PROT is enabled.
#[repr(C, align(4096))]
pub struct Msrpm {
    bytes: [u8; MSRPM_BYTES],
}

impl Msrpm {
    /// Trusted native first boot: execute covered native MSRs directly except
    /// EFER and monitor/SVM controls. APM2 rev3.44 15.11/Table15-8 and 15.30.
    /// Protect C00101xx by default, including VM_CR, VM_HSAVE_PA, lock keys
    /// and encrypted-guest controls. Its ordinary OSVW registers stay native.
    /// TSC_RATIO is protected host scaling state; guest access/nested scaling
    /// is unsupported, and the guest's SVM/TscRateMsr enumeration is hidden.
    /// This intercept is not a guest register emulation owner. Out-of-map
    /// indices still intercept in hardware.
    /// Requires the native VMCB intercept profile; not a sandbox MSR policy.
    pub fn native_boot() -> Self {
        let mut map = Self::new();
        map.bytes[..0x1800].fill(0);
        // Two adjacent access bits at the Table15-8 EFER and TSC_RATIO slots.
        map.bytes[0x820] = 3;
        map.bytes[0x841] = 3;
        // C0010100..C00101ff: 256 registers, two bits each.
        map.bytes[0x1040..0x1080].fill(0xff);
        // PPR57896 rev3.00 pp.87/217: advertised OSVW length/status are RW
        // hardware state, not monitor controls. Preserve firmware values and
        // native write/fault semantics; do not synthesize workaround metadata.
        // Exact C0010140/141 slots share the low four bits of this byte.
        map.bytes[0x1050] = 0xf0;
        // Admission requires disabled SME/SNP/VMPL/host multi-key modes.
        // Keep SYS_CFG writes stopped before hardware changes that invariant.
        // Reads remain native. PPR57896 rev3.00 p202; APM2 7.10.2/.9.
        map.set(
            crate::arch::x86_64::encryption::SYS_CFG,
            MsrAccess::Write,
            Permission::Intercept,
        )
        .expect("covered SYS_CFG MSR");
        map
    }

    /// Fixed native x2APIC startup ownership. Install before entry on every
    /// admitted CPU, after enabling x2APIC. Reads and ordinary APIC registers
    /// remain native; APIC_BASE writes cannot bypass the ICR owner by changing
    /// mode/base. APM2 rev3.44 15.11/Table15-8, 16.11 and 16.13.
    pub fn intercept_native_startup(&mut self) {
        // Version presentation and the hidden AMD extension space share the
        // guest owner on both buses. No alternative WRMSR may reach APIC410.
        for index in 0x800..=0x8ff {
            if super::ipi::native_guest_apic_msr(index) {
                for access in [MsrAccess::Read, MsrAccess::Write] {
                    self.set(index, access, Permission::Intercept)
                        .expect("covered guest APIC presentation");
                }
            }
        }
        self.set(0x830, MsrAccess::Write, Permission::Intercept)
            .expect("covered x2APIC ICR");
        self.set(0x1b, MsrAccess::Write, Permission::Intercept)
            .expect("covered APIC_BASE");
    }

    pub const fn new() -> Self {
        Self {
            bytes: [0xff; MSRPM_BYTES],
        }
    }

    pub const fn bytes(&self) -> &[u8; MSRPM_BYTES] {
        &self.bytes
    }

    /// Change only the selected read or write bit. Unsupported MSRs leave the
    /// complete map unchanged.
    pub fn set(
        &mut self,
        msr: u32,
        access: MsrAccess,
        permission: Permission,
    ) -> Result<(), PermissionMapError> {
        let (byte_base, index) = match msr {
            0x0000_0000..=0x0000_1fff => (0x0000, msr),
            0xc000_0000..=0xc000_1fff => (0x0800, msr - 0xc000_0000),
            0xc001_0000..=0xc001_1fff => (0x1000, msr - 0xc001_0000),
            _ => return Err(PermissionMapError::UnsupportedMsr),
        };
        let access_bit = match access {
            MsrAccess::Read => 0,
            MsrAccess::Write => 1,
        };
        let bit = byte_base * 8 + index as usize * 2 + access_bit;
        set_bit(&mut self.bytes, bit, permission);
        Ok(())
    }
}

impl Default for Msrpm {
    fn default() -> Self {
        Self::new()
    }
}

fn set_bit(bytes: &mut [u8], bit: usize, permission: Permission) {
    let mask = 1 << (bit % 8);
    let byte = &mut bytes[bit / 8];
    match permission {
        Permission::Allow => *byte &= !mask,
        Permission::Intercept => *byte |= mask,
    }
}
