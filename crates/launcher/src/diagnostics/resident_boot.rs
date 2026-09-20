//! The resident launcher's failure encoders: admission, AP-start and takeover refusals as the
//! preparation address and journal words recorded for the parent.

/// Exact stage19 rejection. Filled from existing samples; only the serialized
/// BSP publishes it after the blocking MP owner has acquired callback completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionFailure {
    pub operation: u32,
    pub predicate: u32,
    pub processor: u32,
    pub apic_id: u32,
    pub item: u64,
    pub observed: u64,
    pub expected: u64,
    pub status: u64,
}

impl AdmissionFailure {
    pub const fn new(
        operation: u32,
        predicate: u32,
        item: u64,
        observed: u64,
        expected: u64,
        status: u64,
    ) -> Self {
        Self {
            operation,
            predicate,
            processor: u32::MAX,
            apic_id: u32::MAX,
            item,
            observed,
            expected,
            status,
        }
    }

    pub fn contexts(self, count: u32) -> [u64; 6] {
        [
            self.predicate as u64,
            self.item,
            self.observed,
            self.expected,
            self.status,
            self.processor as u64 | ((count as u64) << 32),
        ]
    }
}

/// AP-owned BOOT header observation, acquired by the BSP only after the AP
/// publishes its failed bit. Assembly writes reason at BOOT+120 and observation
/// at BOOT+128; the reserved DWORD is initialized once by BSP preparation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApFailureObservation {
    pub reason: u32,
    pub reserved: u32,
    pub observation: u64,
}

const _: () = {
    assert!(core::mem::size_of::<ApFailureObservation>() == 16);
    assert!(core::mem::offset_of!(ApFailureObservation, observation) == 8);
};

/// Preparation address of a stage-16/17 slot refusal (reasons 33-36): slot in
/// bits 40-47, operation in 32-39, refused predicate in the low DWORD. It stays
/// below 48 bits, so `preparation_words` keeps it whole, and its low 40 bits
/// equal reason 32's `operation<<32|predicate`, which binds the bank record.
pub const fn slot_preparation_address(slot: u32, operation: u32, predicate: u32) -> u64 {
    ((slot as u64) & 0xff) << 40 | ((operation as u64) & 0xff) << 32 | predicate as u64
}

/// Version1 stage0x82: reason8, slot5, (count-1)5, version6 above stage8.
/// Boundary reasons1..33 select a precise predicate and its sampled value.
/// Rust0x40..0x7f carries callback return0..63 and captured rawCR0 as context;
/// 0x83 carries a full-width larger return. Wait0x81/0x82 carries respectively
/// observed/expected identity or observed/required ACK mask in low/high DWORDs.
/// This record describes refusal evidence only, never successful guest entry.
pub fn ap_failure_words(
    slot: u32,
    processor_count: u32,
    sample: ApFailureObservation,
) -> Option<[u32; 3]> {
    if !(1..=32).contains(&processor_count)
        || slot >= processor_count
        || sample.reserved != 0
        || !matches!(sample.reason, 1..=33 | 0x40..=0x7f | 0x81..=0x83)
        || (sample.reason == 0x81
            && (sample.observation as u32 > 255
                || sample.observation >> 32 > 255
                || sample.observation as u32 == (sample.observation >> 32) as u32))
        || (sample.reason == 0x82
            && (sample.observation >> 32 != 1u64 << slot
                || sample.observation as u32 & (1u32 << slot) != 0))
        || (sample.reason == 0x83 && sample.observation <= 63)
    {
        return None;
    }
    Some([
        0x0400_0082 | (sample.reason << 8) | (slot << 16) | ((processor_count - 1) << 21),
        sample.observation as u32,
        (sample.observation >> 32) as u32,
    ])
}

/// Stage84 preserves a BSP arm refusal that the register-preserving callback
/// cannot return in RAX. APs already preserve this code in their BOOT record.
/// TagA1, truncated flag55, reason54:48, register47:32, observed low32
/// (`host::resident::TAKEOVER_TAG`). Reasons 1-4 (retired xAPIC MMIO
/// offsets) stay decodable for older images; reason 5 names the x2APIC MSR
/// of a refused captured register (`captured_register_refusal`).
pub fn takeover_failure_words(slot: u32, count: u32, code: u64) -> Option<[u32; 3]> {
    use svmvisor_hypervisor::{
        host::resident::{TAKEOVER_CAPTURED_REGISTER, TAKEOVER_TAG},
        svm::x2avic::registers::CaptureRefusal,
    };
    let reason = (code >> 48) & 0x7f;
    let offset = (code >> 32) & 0xffff;
    let valid = match reason {
        1 => (0x480..=0x4f0).contains(&offset) && offset & 15 == 0,
        2 => (0x500..=0x530).contains(&offset) && offset & 15 == 0,
        3 | 4 => offset == 0x410,
        TAKEOVER_CAPTURED_REGISTER => CaptureRefusal::captured(offset as u32),
        _ => false,
    };
    if code >> 56 != TAKEOVER_TAG || !valid || !(1..=32).contains(&count) || slot >= count {
        return None;
    }
    Some([0x0400_0084 | (slot << 16) | ((count - 1) << 21), code as u32, (code >> 32) as u32])
}

#[cfg(test)]
mod tests {
    use super::*;

    use svmvisor_card_abi::{boot_options::ResidentBootOptions, journal::diagnostic_payload};

    #[test]
    fn takeover_refusals_preserve_raw_code_and_reject_bad_shape() {
        use super::takeover_failure_words;
        for (reason, offset) in [(1u64, 0x480u64), (1, 0x4f0), (2, 0x500), (3, 0x410), (4, 0x410)] {
            for wide in [0, 1u64 << 55] {
                let code = (0xa1u64 << 56) | wide | (reason << 48) | (offset << 32) | 0xfedcba98;
                let words = takeover_failure_words(3, 24, code).unwrap();
                assert_eq!(words[0], 0x04000084 | (3 << 16) | (23 << 21));
                assert_eq!(words[1] as u64 | ((words[2] as u64) << 32), code);
            }
        }
        for code in [
            0,
            0xa100041000000000,
            0xa101041000000000,
            0xa102048000000000,
            0xa103040000000000,
            0xa105041000000000,
            0xa105080000000000,
            0xa105083100000000,
            0xa105083f00000000,
        ] {
            assert!(takeover_failure_words(0, 24, code).is_none());
        }
        // Arm code 11 with its refused register: every captured MSR, and a
        // value with high bits (flag 55, low half kept).
        use svmvisor_hypervisor::{
            host::resident::captured_register_refusal, svm::x2avic::registers::CaptureRefusal,
        };
        for msr in [0x808u32, 0x80f, 0x830, 0x832, 0x835, 0x837, 0x838, 0x83e] {
            for value in [0x0001_2345u64, 0x0000_0001_0000_00ef] {
                let code = captured_register_refusal(CaptureRefusal { msr, value });
                assert_eq!(
                    code,
                    (0xa1 << 56)
                        | (u64::from(value >> 32 != 0) << 55)
                        | (5 << 48)
                        | (u64::from(msr) << 32)
                        | (value & 0xffff_ffff)
                );
                let words = takeover_failure_words(0, 2, code).unwrap();
                assert_eq!(words[1] as u64 | ((words[2] as u64) << 32), code);
            }
        }
        let code = 0xa103041000000000;
        for (slot, count) in [(0, 0), (0, 33), (1, 1), (32, 32)] {
            assert!(takeover_failure_words(slot, count, code).is_none());
        }
    }

    #[test]
    fn ap_failure_record_is_bounded_versioned_and_lossless() {
        for (reason, observation) in [
            (11, 0x8001003bu64),
            (17, 0x100000001),
            (0x46, 0xe0010033),
            (0x81, (31u64 << 32) | 30),
            (0x82, 1u64 << 63),
            (0x83, u64::MAX),
        ] {
            let sample = ApFailureObservation { reason, reserved: 0, observation };
            assert_eq!(
                ap_failure_words(31, 32, sample),
                Some([0x07ff0082 | (reason << 8), observation as u32, (observation >> 32) as u32])
            );
            assert!(ap_failure_words(32, 32, sample).is_none());
            assert!(ap_failure_words(0, 0, sample).is_none());
            assert!(ap_failure_words(0, 33, sample).is_none());
            assert!(
                ap_failure_words(0, 1, ApFailureObservation { reserved: 1, ..sample }).is_none()
            );
        }
        for reason in [0, 34, 0x3f, 0x80, 0x84, 256] {
            assert!(
                ap_failure_words(
                    0,
                    1,
                    ApFailureObservation { reason, reserved: 0, observation: 0 }
                )
                .is_none()
            );
        }
        assert!(
            ap_failure_words(
                0,
                1,
                ApFailureObservation { reason: 0x83, reserved: 0, observation: 63 }
            )
            .is_none()
        );
    }

    #[test]
    fn admission_record_binds_compact_class_to_lossless_wire_operands() {
        for processor in [7, u32::MAX] {
            let failure = AdmissionFailure {
                operation: 4,
                predicate: 8,
                processor,
                apic_id: 0x34,
                item: 0xc0010010,
                observed: 0xfedcba9876543210,
                expected: 0x123456789abcdef0,
                status: 48,
            };
            let payload = diagnostic_payload(
                1,
                12,
                true,
                42,
                failure.apic_id,
                0,
                failure.contexts(24),
                failure.operation,
            );
            assert_eq!(payload[1], 0x1010c);
            assert_eq!(payload[3], 0x34);
            assert_eq!(payload[18], 4);
            let mut decoded = [0; 6];
            for i in 0..6 {
                decoded[i] = payload[6 + i * 2] as u64 | ((payload[7 + i * 2] as u64) << 32);
            }
            assert_eq!(
                decoded,
                [
                    8,
                    0xc0010010,
                    0xfedcba9876543210,
                    0x123456789abcdef0,
                    48,
                    processor as u64 | (24u64 << 32)
                ]
            );
            let mut options = ResidentBootOptions::new(0xd0000000, 42);
            options.rust_entered = 1;
            options.failure = 0x8000000000000003;
            options.preparation_stage = 19;
            options.preparation_reason = 32;
            options.preparation_status = options.failure;
            options.preparation_address =
                (failure.operation as u64) << 32 | failure.predicate as u64;
            assert_eq!(options.preparation_words(), Some([0x00042013, 0x80000003, 8]));
        }
    }

    #[test]
    fn slot_refusal_record_keeps_slot_operation_predicate_and_source_code() {
        // Stage 16, reason 33: slot 17's host closure refused predicate 438
        // (entry not present, level 2) with host_closure code 24.
        assert_eq!(slot_preparation_address(17, 9, 438), 0x0000_1109_0000_01b6);
        assert_eq!(slot_preparation_address(17, 9, 438) & 0xff_ffff_ffff, (9u64 << 32) | 438);
        let mut options = ResidentBootOptions::new(0xd0000000, 42);
        options.rust_entered = 1;
        options.failure = 0x8000000000000003;
        options.preparation_stage = 16;
        options.preparation_reason = 33;
        options.preparation_status = 24;
        options.preparation_address = slot_preparation_address(17, 9, 438);
        assert_eq!(options.preparation_words(), Some([0x11092110, 24, 438]));
        // Stage 17, reason 35: slot 31's NPT refused with code 64+16+9.
        options.preparation_stage = 17;
        options.preparation_reason = 35;
        options.preparation_status = 89;
        options.preparation_address = slot_preparation_address(31, 10, 636);
        assert_eq!(options.preparation_words(), Some([0x1f0a2311, 89, 636]));
        assert_eq!(slot_preparation_address(0x1ff, 0x1ff, 1) >> 48, 0);
    }
}
