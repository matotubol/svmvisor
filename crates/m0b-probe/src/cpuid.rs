//! Bounded CPUID collection and AMD SVM feature decoding.

/// `AuthenticAMD` in the EBX, EDX, ECX byte order returned by CPUID leaf 0.
pub const AUTHENTIC_AMD: [u8; 12] = *b"AuthenticAMD";

/// Raw output registers for one `(leaf, subleaf)` CPUID query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuidRegisters {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// Narrow interface implemented by the UEFI adapter and deterministic fakes.
pub trait CpuidSource {
    fn cpuid(&mut self, leaf: u32, subleaf: u32) -> CpuidRegisters;
}

/// Whitelisted raw CPUID leaves used by the first M0b inventory slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInventory {
    pub leaf_0000_0000: CpuidRegisters,
    pub leaf_0000_0001: Option<CpuidRegisters>,
    pub leaf_0000_0007_subleaf_0: Option<CpuidRegisters>,
    pub leaf_8000_0000: CpuidRegisters,
    pub leaf_8000_0001: Option<CpuidRegisters>,
    pub leaf_8000_0002_to_0004: Option<[CpuidRegisters; 3]>,
    pub leaf_8000_0008: Option<CpuidRegisters>,
    pub leaf_8000_000a: Option<CpuidRegisters>,
    pub leaf_8000_001e: Option<CpuidRegisters>,
    pub leaf_8000_001f: Option<CpuidRegisters>,
    pub brand: Option<[u8; 48]>,
}

impl CpuInventory {
    #[must_use]
    pub fn max_basic_leaf(&self) -> u32 {
        self.leaf_0000_0000.eax
    }

    #[must_use]
    pub fn max_extended_leaf(&self) -> u32 {
        self.leaf_8000_0000.eax
    }

    #[must_use]
    pub fn vendor(&self) -> [u8; 12] {
        let mut vendor = [0_u8; 12];
        vendor[0..4].copy_from_slice(&self.leaf_0000_0000.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&self.leaf_0000_0000.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&self.leaf_0000_0000.ecx.to_le_bytes());
        vendor
    }

    #[must_use]
    pub fn is_authentic_amd(&self) -> bool {
        self.vendor() == AUTHENTIC_AMD
    }

    #[must_use]
    pub fn family_model_stepping(&self) -> Option<(u16, u16, u8)> {
        let eax = self.leaf_0000_0001?.eax;
        let stepping = (eax & 0x0f) as u8;
        let base_model = ((eax >> 4) & 0x0f) as u16;
        let base_family = ((eax >> 8) & 0x0f) as u16;
        let extended_model = ((eax >> 16) & 0x0f) as u16;
        let extended_family = ((eax >> 20) & 0xff) as u16;
        let family = if base_family == 0x0f {
            base_family + extended_family
        } else {
            base_family
        };
        let model = if base_family == 0x06 || base_family == 0x0f {
            base_model | (extended_model << 4)
        } else {
            base_model
        };
        Some((family, model, stepping))
    }

    #[must_use]
    pub fn svm_cpuid_supported(&self) -> Option<bool> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_0001.map(|leaf| leaf.ecx & (1 << 2) != 0)
    }

    /// Whether AMD's extended topology definitions are architecturally valid.
    #[must_use]
    pub fn topology_extensions_supported(&self) -> Option<bool> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_0001.map(|leaf| leaf.ecx & (1 << 22) != 0)
    }

    #[must_use]
    pub fn physical_address_bits(&self) -> Option<u8> {
        self.leaf_8000_0008.map(|leaf| (leaf.eax & 0xff) as u8)
    }

    #[must_use]
    pub fn svm_revision(&self) -> Option<u8> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_000a.map(|leaf| (leaf.eax & 0xff) as u8)
    }

    #[must_use]
    pub fn svm_asid_count(&self) -> Option<u32> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_000a.map(|leaf| leaf.ebx)
    }

    #[must_use]
    pub fn svm_feature(&self, bit: u8) -> Option<bool> {
        if !self.is_authentic_amd() {
            return None;
        }
        let mask = 1_u32.checked_shl(u32::from(bit))?;
        self.leaf_8000_000a.map(|leaf| leaf.edx & mask != 0)
    }

    #[must_use]
    pub fn memory_encryption_feature(&self, bit: u8) -> Option<bool> {
        if !self.is_authentic_amd() {
            return None;
        }
        let mask = 1_u32.checked_shl(u32::from(bit))?;
        self.leaf_8000_001f.map(|leaf| leaf.eax & mask != 0)
    }

    #[must_use]
    pub fn c_bit_position(&self) -> Option<u8> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_001f.map(|leaf| (leaf.ebx & 0x3f) as u8)
    }

    #[must_use]
    pub fn physical_address_reduction(&self) -> Option<u8> {
        if !self.is_authentic_amd() {
            return None;
        }
        self.leaf_8000_001f
            .map(|leaf| ((leaf.ebx >> 6) & 0x3f) as u8)
    }
}

/// Collect only leaves that the CPU first reports as enumerated.
///
/// The maximum-leaf checks are part of the safety boundary: an unavailable
/// leaf is retained as unavailable and is never converted to an all-zero,
/// falsely-negative feature record.
pub fn collect_cpuid(source: &mut impl CpuidSource) -> CpuInventory {
    let leaf_0000_0000 = source.cpuid(0x0000_0000, 0);
    let max_basic = leaf_0000_0000.eax;
    let leaf_0000_0001 = (max_basic >= 0x0000_0001).then(|| source.cpuid(0x0000_0001, 0));
    let leaf_0000_0007_subleaf_0 = (max_basic >= 0x0000_0007).then(|| source.cpuid(0x0000_0007, 0));

    let leaf_8000_0000 = source.cpuid(0x8000_0000, 0);
    let max_extended = leaf_8000_0000.eax;
    let leaf_8000_0001 = (max_extended >= 0x8000_0001).then(|| source.cpuid(0x8000_0001, 0));
    let (leaf_8000_0002_to_0004, brand) = if max_extended >= 0x8000_0004 {
        let leaves = [
            source.cpuid(0x8000_0002, 0),
            source.cpuid(0x8000_0003, 0),
            source.cpuid(0x8000_0004, 0),
        ];
        let mut bytes = [0_u8; 48];
        for (index, leaf) in leaves.iter().enumerate() {
            let offset = index * 16;
            bytes[offset..offset + 4].copy_from_slice(&leaf.eax.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&leaf.ebx.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&leaf.ecx.to_le_bytes());
            bytes[offset + 12..offset + 16].copy_from_slice(&leaf.edx.to_le_bytes());
        }
        (Some(leaves), Some(bytes))
    } else {
        (None, None)
    };
    let leaf_8000_0008 = (max_extended >= 0x8000_0008).then(|| source.cpuid(0x8000_0008, 0));
    let leaf_8000_000a = (max_extended >= 0x8000_000a).then(|| source.cpuid(0x8000_000a, 0));
    let leaf_8000_001e = (max_extended >= 0x8000_001e).then(|| source.cpuid(0x8000_001e, 0));
    let leaf_8000_001f = (max_extended >= 0x8000_001f).then(|| source.cpuid(0x8000_001f, 0));

    CpuInventory {
        leaf_0000_0000,
        leaf_0000_0001,
        leaf_0000_0007_subleaf_0,
        leaf_8000_0000,
        leaf_8000_0001,
        leaf_8000_0002_to_0004,
        leaf_8000_0008,
        leaf_8000_000a,
        leaf_8000_001e,
        leaf_8000_001f,
        brand,
    }
}

/// Whether the firmware adapter may execute the single named `VM_CR` read.
///
/// A non-AMD CPU, absent extended leaf, or masked SVM bit never reaches RDMSR.
#[must_use]
pub fn should_read_vm_cr(inventory: &CpuInventory) -> bool {
    inventory.is_authentic_amd()
        && inventory.svm_cpuid_supported() == Some(true)
        && inventory.leaf_8000_000a.is_some()
}

/// Compare the collected CPUID capability image while excluding only fields
/// that architecturally identify the executing logical processor.
///
/// CPUID leaf 1 EBX[31:24] is the initial APIC ID. Extended leaf
/// `8000_001Eh` carries the extended APIC ID in EAX, the core ID in EBX[7:0],
/// and the node ID in ECX[7:0]. The APIC-ID witnesses are validated separately
/// against UEFI MP Services. Core and node identity are retained raw but are not
/// equated with MP Services package/core/thread locations. Every other collected
/// bit, optional-leaf presence, and raw register is compared.
#[must_use]
pub fn capability_cpuid_equal(reference: &CpuInventory, observed: &CpuInventory) -> bool {
    fn registers_equal(
        reference: CpuidRegisters,
        observed: CpuidRegisters,
        masks: [u32; 4],
    ) -> bool {
        (reference.eax & masks[0]) == (observed.eax & masks[0])
            && (reference.ebx & masks[1]) == (observed.ebx & masks[1])
            && (reference.ecx & masks[2]) == (observed.ecx & masks[2])
            && (reference.edx & masks[3]) == (observed.edx & masks[3])
    }

    fn optional_registers_equal(
        reference: Option<CpuidRegisters>,
        observed: Option<CpuidRegisters>,
        masks: [u32; 4],
    ) -> bool {
        match (reference, observed) {
            (Some(reference), Some(observed)) => registers_equal(reference, observed, masks),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }

    const ALL: [u32; 4] = [u32::MAX; 4];
    const LEAF_1: [u32; 4] = [u32::MAX, 0x00ff_ffff, u32::MAX, u32::MAX];
    const LEAF_8000_001E: [u32; 4] = [0, 0xffff_ff00, 0xffff_ff00, u32::MAX];

    registers_equal(reference.leaf_0000_0000, observed.leaf_0000_0000, ALL)
        && optional_registers_equal(reference.leaf_0000_0001, observed.leaf_0000_0001, LEAF_1)
        && optional_registers_equal(
            reference.leaf_0000_0007_subleaf_0,
            observed.leaf_0000_0007_subleaf_0,
            ALL,
        )
        && registers_equal(reference.leaf_8000_0000, observed.leaf_8000_0000, ALL)
        && optional_registers_equal(reference.leaf_8000_0001, observed.leaf_8000_0001, ALL)
        && match (
            reference.leaf_8000_0002_to_0004,
            observed.leaf_8000_0002_to_0004,
        ) {
            (Some(reference), Some(observed)) => reference
                .iter()
                .zip(observed.iter())
                .all(|(reference, observed)| registers_equal(*reference, *observed, ALL)),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
        && optional_registers_equal(reference.leaf_8000_0008, observed.leaf_8000_0008, ALL)
        && optional_registers_equal(reference.leaf_8000_000a, observed.leaf_8000_000a, ALL)
        && optional_registers_equal(
            reference.leaf_8000_001e,
            observed.leaf_8000_001e,
            LEAF_8000_001E,
        )
        && optional_registers_equal(reference.leaf_8000_001f, observed.leaf_8000_001f, ALL)
}

/// Legacy eight-bit APIC identity reported by CPUID leaf 1.
#[must_use]
pub fn initial_apic_id(inventory: &CpuInventory) -> Option<u8> {
    inventory
        .leaf_0000_0001
        .map(|leaf| ((leaf.ebx >> 24) & 0xff) as u8)
}

/// Extended APIC identity reported by AMD CPUID leaf `8000_001Eh`.
///
/// Merely reaching that leaf number is insufficient: AMD defines its topology
/// fields only when `Fn8000_0001_ECX.TopologyExtensions` is set.
#[must_use]
pub fn extended_apic_id(inventory: &CpuInventory) -> Option<u32> {
    if inventory.topology_extensions_supported() != Some(true) {
        return None;
    }
    inventory.leaf_8000_001e.map(|leaf| leaf.eax)
}

/// Bind a same-processor CPUID observation to the x86 processor ID returned by
/// UEFI MP Services.
///
/// PI defines only the low eight bits of `ProcessorId` for IA32/X64 and reserves
/// the rest. AMD's extended APIC ID is mode-dependent, so this portable binding
/// requires zero PI reserved bits and compares the low byte of both CPUID APIC
/// witnesses. The callback's `WhoAmI` result separately binds the measurement to
/// the requested MP Services processor number.
#[must_use]
pub fn cpuid_identity_matches_processor_id(inventory: &CpuInventory, processor_id: u64) -> bool {
    let processor_id_low8 = processor_id as u8;
    processor_id >> 8 == 0
        && initial_apic_id(inventory) == Some(processor_id_low8)
        && extended_apic_id(inventory).map(|id| id as u8) == Some(processor_id_low8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[derive(Default)]
    struct FakeCpuid {
        calls: Vec<(u32, u32)>,
        max_basic: u32,
        max_extended: u32,
        amd: bool,
        svm: bool,
        svm_features: u32,
    }

    impl CpuidSource for FakeCpuid {
        fn cpuid(&mut self, leaf: u32, subleaf: u32) -> CpuidRegisters {
            self.calls.push((leaf, subleaf));
            match leaf {
                0 => {
                    let vendor = if self.amd {
                        *b"AuthenticAMD"
                    } else {
                        *b"GenuineIntel"
                    };
                    CpuidRegisters {
                        eax: self.max_basic,
                        ebx: u32::from_le_bytes(vendor[0..4].try_into().unwrap()),
                        edx: u32::from_le_bytes(vendor[4..8].try_into().unwrap()),
                        ecx: u32::from_le_bytes(vendor[8..12].try_into().unwrap()),
                    }
                }
                1 => CpuidRegisters {
                    eax: 0x00aa_0f12,
                    ..Default::default()
                },
                0x8000_0000 => CpuidRegisters {
                    eax: self.max_extended,
                    ..Default::default()
                },
                0x8000_0001 => CpuidRegisters {
                    ecx: (u32::from(self.svm) << 2)
                        | (u32::from(self.max_extended >= 0x8000_001e) << 22),
                    ..Default::default()
                },
                0x8000_0008 => CpuidRegisters {
                    eax: 52,
                    ..Default::default()
                },
                0x8000_000a => CpuidRegisters {
                    eax: 2,
                    ebx: 32_768,
                    edx: self.svm_features,
                    ..Default::default()
                },
                0x8000_001f => CpuidRegisters {
                    eax: 0b11,
                    ebx: 47 | (5 << 6),
                    ..Default::default()
                },
                _ => CpuidRegisters::default(),
            }
        }
    }

    #[test]
    fn unavailable_leaves_are_not_queried_or_faked_as_false() {
        let mut source = FakeCpuid {
            max_basic: 1,
            max_extended: 0x8000_0001,
            ..Default::default()
        };
        let inventory = collect_cpuid(&mut source);

        assert_eq!(inventory.leaf_8000_000a, None);
        assert_eq!(inventory.svm_feature(0), None);
        assert!(!source.calls.contains(&(0x8000_000a, 0)));
        assert!(!source.calls.contains(&(0x0000_0007, 0)));
    }

    #[test]
    fn amd_svm_fields_decode_without_losing_raw_bits() {
        let feature_bits = (1 << 0) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 31);
        let mut source = FakeCpuid {
            max_basic: 7,
            max_extended: 0x8000_001f,
            amd: true,
            svm: true,
            svm_features: feature_bits,
            ..Default::default()
        };
        let inventory = collect_cpuid(&mut source);

        assert_eq!(inventory.vendor(), AUTHENTIC_AMD);
        assert_eq!(inventory.svm_cpuid_supported(), Some(true));
        assert_eq!(inventory.physical_address_bits(), Some(52));
        assert_eq!(inventory.svm_revision(), Some(2));
        assert_eq!(inventory.svm_asid_count(), Some(32_768));
        assert_eq!(inventory.svm_feature(0), Some(true));
        assert_eq!(inventory.svm_feature(2), Some(false));
        assert_eq!(inventory.svm_feature(32), None);
        assert_eq!(inventory.leaf_8000_000a.unwrap().edx, feature_bits);
        assert_eq!(inventory.c_bit_position(), Some(47));
        assert_eq!(inventory.physical_address_reduction(), Some(5));
        assert_eq!(inventory.memory_encryption_feature(32), None);
        assert!(should_read_vm_cr(&inventory));
    }

    #[test]
    fn vendor_and_svm_bit_both_gate_vm_cr_read() {
        for (amd, svm) in [(false, true), (true, false), (false, false)] {
            let mut source = FakeCpuid {
                max_basic: 1,
                max_extended: 0x8000_000a,
                amd,
                svm,
                ..Default::default()
            };
            assert!(!should_read_vm_cr(&collect_cpuid(&mut source)));
        }
    }

    #[test]
    fn non_amd_vendor_keeps_amd_specific_decodes_unavailable() {
        let mut source = FakeCpuid {
            max_basic: 7,
            max_extended: 0x8000_001f,
            amd: false,
            svm: true,
            svm_features: u32::MAX,
            ..Default::default()
        };
        let inventory = collect_cpuid(&mut source);

        assert!(inventory.leaf_8000_000a.is_some());
        assert!(inventory.leaf_8000_001f.is_some());
        assert_eq!(inventory.svm_cpuid_supported(), None);
        assert_eq!(inventory.svm_revision(), None);
        assert_eq!(inventory.svm_asid_count(), None);
        assert_eq!(inventory.svm_feature(0), None);
        assert_eq!(inventory.memory_encryption_feature(0), None);
        assert_eq!(inventory.c_bit_position(), None);
        assert_eq!(inventory.physical_address_reduction(), None);
    }

    #[test]
    fn missing_svm_capability_leaf_gates_vm_cr_even_when_svm_bit_is_set() {
        let mut source = FakeCpuid {
            max_basic: 1,
            max_extended: 0x8000_0001,
            amd: true,
            svm: true,
            ..Default::default()
        };
        assert!(!should_read_vm_cr(&collect_cpuid(&mut source)));
    }

    #[test]
    fn family_model_stepping_uses_extended_amd_encoding() {
        let mut source = FakeCpuid {
            max_basic: 1,
            max_extended: 0x8000_0000,
            ..Default::default()
        };
        let inventory = collect_cpuid(&mut source);
        assert_eq!(inventory.family_model_stepping(), Some((0x19, 0xa1, 2)));
    }

    #[test]
    fn capability_comparison_masks_only_processor_identity_fields() {
        let mut source = FakeCpuid {
            max_basic: 7,
            max_extended: 0x8000_001f,
            amd: true,
            svm: true,
            svm_features: 0x83,
            ..Default::default()
        };
        let reference = collect_cpuid(&mut source);
        let mut observed = reference;
        observed.leaf_0000_0001.as_mut().unwrap().ebx ^= 0xff00_0000;
        observed.leaf_8000_001e = Some(CpuidRegisters {
            eax: 0x23,
            ebx: 0x0000_0304,
            ecx: 0x0000_0205,
            edx: 0,
        });
        let mut reference_with_topology = reference;
        reference_with_topology.leaf_8000_001e = Some(CpuidRegisters {
            eax: 0x01,
            ebx: 0x0000_0301,
            ecx: 0x0000_0201,
            edx: 0,
        });
        assert!(capability_cpuid_equal(&reference_with_topology, &observed));

        observed.leaf_8000_000a.as_mut().unwrap().edx ^= 1 << 7;
        assert!(!capability_cpuid_equal(&reference_with_topology, &observed));
    }

    #[test]
    fn processor_identity_requires_both_apic_witnesses() {
        let mut source = FakeCpuid {
            max_basic: 7,
            max_extended: 0x8000_001f,
            amd: true,
            svm: true,
            ..Default::default()
        };
        let mut inventory = collect_cpuid(&mut source);
        inventory.leaf_0000_0001.as_mut().unwrap().ebx = 0x1b00_0000;
        inventory.leaf_8000_001e.as_mut().unwrap().eax = 0x1b;
        assert_eq!(initial_apic_id(&inventory), Some(0x1b));
        assert_eq!(extended_apic_id(&inventory), Some(0x1b));
        assert!(cpuid_identity_matches_processor_id(&inventory, 0x1b));
        assert!(!cpuid_identity_matches_processor_id(&inventory, 0x1a));

        inventory.leaf_8000_001e.as_mut().unwrap().eax = 0x0000_011b;
        assert!(cpuid_identity_matches_processor_id(&inventory, 0x1b));
        assert!(!cpuid_identity_matches_processor_id(&inventory, 0x11b));

        inventory.leaf_8000_0001.as_mut().unwrap().ecx &= !(1 << 22);
        assert_eq!(extended_apic_id(&inventory), None);
        assert!(!cpuid_identity_matches_processor_id(&inventory, 0x1b));

        inventory.leaf_8000_001e = None;
        assert!(!cpuid_identity_matches_processor_id(&inventory, 0x1b));
    }
}
