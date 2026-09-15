//! Integer-only contract shared with the separately reviewed returning assembly.
#[repr(C)]
#[derive(Default)]
pub struct Context {
    pub guest_vmcb: u64,
    pub host_extra: u64,
    pub observed_extra: u64,
    pub hsave: u64,
    pub original_xstate: u64,
    pub observed_xstate: u64,
    pub guest_xstate: u64,
    pub mask: u64,
    pub negative_flags: u64,
    pub original: [u64; 8],
    pub observed: [u64; 8],
    pub exit_code: u64,
    pub guest_rax: u64,
    pub abi_failures: u64,
    pub original_xcr0: u64,
    pub observed_xcr0: u64,
    pub original_xss: u64,
    pub observed_xss: u64,
    pub outer_xstate: u64,
    pub reserved: u64,
    pub original_dr6: u64,
    pub observed_dr6: u64,
    pub guest_gprs: [u64; 14],
    pub private_idt: u64,
    pub original_idtr: [u8; 16],
    pub observed_idtr: [u8; 16],
    pub fault_kind: u64,
    pub fault_vector: u64,
    pub fault_rip: u64,
    pub fault_error: u64,
    pub fault_recovered: u64,
    pub fault_stage: u64,
    pub vmrun_attempts: u64,
    pub checkpoint_efer: u64,
    pub checkpoint_hsave: u64,
    pub mutation_stage: u64,
    pub checkpoint_rflags: u64,
    pub checkpoint_extra: u64,
    pub checkpoint_xstate: u64,
    pub guest_exit_rip: u64,
    pub guest_capture_stage: u64,
}
const _: () = assert!(core::mem::size_of::<Context>() == 560);
const _: () = assert!(core::mem::offset_of!(Context, original) == 72);
const _: () = assert!(core::mem::offset_of!(Context, observed) == 136);
const _: () = assert!(core::mem::offset_of!(Context, abi_failures) == 216);
const _: () = assert!(core::mem::offset_of!(Context, outer_xstate) == 256);
const _: () = assert!(core::mem::offset_of!(Context, guest_gprs) == 288);
const _: () = assert!(core::mem::offset_of!(Context, private_idt) == 400);
const _: () = assert!(core::mem::offset_of!(Context, original_idtr) == 408);
const _: () = assert!(core::mem::offset_of!(Context, fault_kind) == 440);
const _: () = assert!(core::mem::offset_of!(Context, fault_stage) == 480);
const _: () = assert!(core::mem::offset_of!(Context, observed_idtr) == 424);
const _: () = assert!(core::mem::offset_of!(Context, fault_vector) == 448);
const _: () = assert!(core::mem::offset_of!(Context, fault_rip) == 456);
const _: () = assert!(core::mem::offset_of!(Context, fault_error) == 464);
const _: () = assert!(core::mem::offset_of!(Context, fault_recovered) == 472);
const _: () = assert!(core::mem::offset_of!(Context, vmrun_attempts) == 488);
const _: () = assert!(core::mem::offset_of!(Context, checkpoint_efer) == 496);
const _: () = assert!(core::mem::offset_of!(Context, checkpoint_hsave) == 504);
const _: () = assert!(core::mem::offset_of!(Context, mutation_stage) == 512);
const _: () = assert!(core::mem::offset_of!(Context, checkpoint_rflags) == 520);

const _: () = assert!(core::mem::offset_of!(Context, checkpoint_extra) == 528);

const _: () = assert!(core::mem::offset_of!(Context, checkpoint_xstate) == 536);

const _: () = assert!(core::mem::offset_of!(Context, guest_exit_rip) == 544);
const _: () = assert!(core::mem::offset_of!(Context, guest_capture_stage) == 552);

impl Context {
    /// Check this fixture's recorded bounded abort, not arbitrary fault recovery.
    pub fn recovered_host_fault(
        &self,
        vector: u64,
        pc: u64,
        original_idtr: &[u8; 10],
        current_idtr: &[u8; 10],
    ) -> bool {
        self.fault_recovered == 1
            && self.fault_stage == 4
            && self.vmrun_attempts == 0
            && self.fault_vector == vector
            && self.fault_rip == pc
            && self.fault_error == 0
            && self.exit_code == 0
            && self.guest_rax == 0
            && self.original_idtr[..10] == original_idtr[..]
            && self.observed_idtr[..10] == original_idtr[..]
            && current_idtr == original_idtr
    }
    /// Exact recorded checkpoint and cleanup for the two allowlisted armed faults.
    pub fn restored_armed_checkpoint(&self) -> bool {
        matches!(self.fault_kind, 4 | 5)
            && self.mutation_stage == 2
            && self.checkpoint_rflags & (1 << 9) == 0
            && self.original[4] & 0x1000 == 0
            && self.checkpoint_efer == (self.original[4] | 0x1000)
            && self.checkpoint_hsave == self.hsave
            && self.hsave != self.original[5]
            && self.hsave != 0
            && self.hsave & 4095 == 0
            && self.restored_controls()
    }
    /// This checkpoint is after guest VMLOAD, before guest xstate or VMRUN.
    pub fn restored_loaded_checkpoint(&self) -> bool {
        matches!(self.fault_kind, 7 | 8)
            && self.mutation_stage == 4
            && self.checkpoint_rflags & (1 << 9) == 0
            && self.original[4] & 0x1000 == 0
            && self.checkpoint_efer == (self.original[4] | 0x1000)
            && self.checkpoint_hsave == self.hsave
            && self.hsave != self.original[5]
            && self.hsave != 0
            && self.hsave & 4095 == 0
            && self.checkpoint_extra != 0
            && self.checkpoint_extra & 4095 == 0
            && self.restored_controls()
    }
    /// Exact post-xstate-load checkpoint, with both separately owned captures.
    pub fn restored_xstate_checkpoint(&self) -> bool {
        matches!(self.fault_kind, 10 | 11)
            && self.mutation_stage == 6
            && self.checkpoint_rflags & (1 << 9) == 0
            && self.original[4] & 0x1000 == 0
            && self.checkpoint_efer == (self.original[4] | 0x1000)
            && self.checkpoint_hsave == self.hsave
            && self.hsave != self.original[5]
            && self.hsave != 0
            && self.hsave & 4095 == 0
            && self.checkpoint_extra != 0
            && self.checkpoint_extra & 4095 == 0
            && self.checkpoint_xstate != 0
            && self.checkpoint_xstate & 4095 == 0
            && ![
                self.original_xstate,
                self.observed_xstate,
                self.guest_xstate,
                self.outer_xstate,
                self.checkpoint_extra,
            ]
            .contains(&self.checkpoint_xstate)
            && self.restored_controls()
    }
    /// Only the allowlisted fault after one completed VMMCALL and its captures.
    pub fn recovered_post_exit_fault(
        &self,
        vector: u64,
        pc: u64,
        expected_rip: u64,
        original_idtr: &[u8; 10],
        current_idtr: &[u8; 10],
    ) -> bool {
        matches!(self.fault_kind, 13 | 14)
            && self.fault_recovered == 1
            && self.fault_stage == 4
            && self.vmrun_attempts == 1
            && self.guest_capture_stage == 1
            && self.exit_code == 0x81
            && self.guest_exit_rip == expected_rip
            && self.guest_rax == 0x51554d5552455455
            && self.fault_vector == vector
            && self.fault_rip == pc
            && self.fault_error == 0
            && self.original_idtr[..10] == original_idtr[..]
            && self.observed_idtr[..10] == original_idtr[..]
            && current_idtr == original_idtr
    }
    pub fn restored_post_exit_checkpoint(&self) -> bool {
        matches!(self.fault_kind, 13 | 14)
            && self.mutation_stage == 8
            && self.checkpoint_rflags & (1 << 9) == 0
            && self.original[4] & 0x1000 == 0
            && self.checkpoint_efer == (self.original[4] | 0x1000)
            && self.checkpoint_hsave == self.hsave
            && self.hsave != self.original[5]
            && self.hsave != 0
            && self.hsave & 4095 == 0
            && self.checkpoint_extra != 0
            && self.checkpoint_extra & 4095 == 0
            && self.checkpoint_xstate != 0
            && self.checkpoint_xstate & 4095 == 0
            && ![
                self.original_xstate,
                self.observed_xstate,
                self.guest_xstate,
                self.outer_xstate,
                self.checkpoint_extra,
            ]
            .contains(&self.checkpoint_xstate)
            && self.restored_controls()
    }
    pub fn restored_controls(&self) -> bool {
        self.original == self.observed
            && self.original_xcr0 == self.observed_xcr0
            && self.original_xss == self.observed_xss
            && self.original_dr6 == self.observed_dr6
    }
}

/// A present ring-zero 64-bit interrupt gate, preserving the current code selector.
pub fn interrupt_gate(address: u64, selector: u16) -> [u8; 16] {
    let mut gate = [0; 16];
    gate[..2].copy_from_slice(&(address as u16).to_le_bytes());
    gate[2..4].copy_from_slice(&selector.to_le_bytes());
    gate[5] = 0x8e;
    gate[6..8].copy_from_slice(&((address >> 16) as u16).to_le_bytes());
    gate[8..12].copy_from_slice(&((address >> 32) as u32).to_le_bytes());
    gate
}

/// Compare only architecturally saved extra-state fields, excluding reserved
/// bytes and unrelated VMCB guest state not written by VMSAVE.
pub fn extra_state_matches(original: &[u8], observed: &[u8]) -> bool {
    if original.len() != 4096 || observed.len() != 4096 {
        return false;
    }
    [
        (0x440, 0x460),
        (0x470, 0x480),
        (0x490, 0x4a0),
        (0x600, 0x640),
    ]
    .iter()
    .all(|&(start, end)| original[start..end] == observed[start..end])
}

/// Deliberate, canonical syscall targets; no SYSCALL executes in this window.
pub const LOADED_EXTRA_CANARIES: [(usize, u64); 2] = [(0x608, 0x12345000), (0x610, 0x23456000)];

/// Require actual VMSAVE evidence of both changed targets and host restoration.
pub fn loaded_extra_restored(original: &[u8], checkpoint: &[u8], observed: &[u8]) -> bool {
    if !extra_state_matches(original, observed) || checkpoint.len() != 4096 {
        return false;
    }
    // The loaded fixture inherits all other defined extra state from the host.
    if [
        (0x440, 0x460),
        (0x470, 0x480),
        (0x490, 0x4a0),
        (0x600, 0x608),
        (0x618, 0x640),
    ]
    .iter()
    .any(|&(start, end)| original[start..end] != checkpoint[start..end])
    {
        return false;
    }
    LOADED_EXTRA_CANARIES.iter().all(|&(offset, value)| {
        let expected = value.to_le_bytes();
        checkpoint[offset..offset + 8] == expected && original[offset..offset + 8] != expected
    })
}

/// Compare captured TR/LDTR to the pre-entry prepared guest descriptors, other
/// fields to the host-derived fixture, and separately require host restoration.
pub fn post_exit_extra_restored(
    original: &[u8],
    prepared_descriptors: &[u8; 32],
    checkpoint: &[u8],
    observed: &[u8],
) -> bool {
    extra_state_matches(original, observed)
        && checkpoint.len() == 4096
        && checkpoint[0x470..0x480] == prepared_descriptors[..16]
        && checkpoint[0x490..0x4a0] == prepared_descriptors[16..]
        && [(0x440, 0x460), (0x600, 0x608), (0x618, 0x640)]
            .iter()
            .all(|&(start, end)| checkpoint[start..end] == original[start..end])
        && LOADED_EXTRA_CANARIES.iter().all(|&(offset, value)| {
            checkpoint[offset..offset + 8] == value.to_le_bytes()
                && original[offset..offset + 8] != value.to_le_bytes()
        })
}

/// The executed guest performs FNINIT/FLD1 and clears XMM0/XMM15 (and YMM15).
/// Compare all semantic state against that independent expected result.
pub fn post_exit_xstate_restored(
    original: &[u8],
    source: &[u8],
    checkpoint: &[u8],
    observed: &[u8],
    mask: u64,
    avx_offset: Option<usize>,
) -> bool {
    if source.len() != 4096
        || !matches!(mask, 0 | 3 | 7)
        || !xstate_payload_matches(original, observed, mask, avx_offset)
    {
        return false;
    }
    let mut expected = [0u8; 4096];
    expected.copy_from_slice(source);
    expected[..24].fill(0);
    expected[..2].copy_from_slice(&0x37fu16.to_le_bytes());
    expected[2..4].copy_from_slice(&0x3800u16.to_le_bytes());
    expected[4] = 0x80;
    expected[32..160].fill(0);
    expected[32..40].copy_from_slice(&0x8000000000000000u64.to_le_bytes());
    expected[40..42].copy_from_slice(&0x3fffu16.to_le_bytes());
    expected[160..176].fill(0);
    expected[400..416].fill(0);
    if mask != 0 {
        expected[512..520].copy_from_slice(&mask.to_le_bytes());
    }
    if mask == 7 {
        let Some(offset) = avx_offset.filter(|&x| x >= 576 && x <= 3840) else {
            return false;
        };
        expected[offset + 240..offset + 256].fill(0);
    }
    xstate_payload_matches(&expected, checkpoint, mask, avx_offset)
        && original[400..416].iter().any(|&v| v != 0)
        && (mask != 7
            || original[avx_offset.unwrap() + 240..avx_offset.unwrap() + 256]
                .iter()
                .any(|&v| v != 0))
}

/// Known guest vector payloads differ from the assembly's nonvolatile canaries.
pub const LOADED_XMM15: [u64; 2] = [0x13579bdf2468ace0, 0x0fedcba987654321];
pub const LOADED_YMM15_HIGH: [u64; 2] = [0x1122334455667788, 0x8877665544332211];

/// Actual checkpoint must contain the intended noninit guest vector payload,
/// differ from original host state, and leave the final observed host restored.
pub fn loaded_xstate_restored(
    original: &[u8],
    guest: &[u8],
    checkpoint: &[u8],
    observed: &[u8],
    mask: u64,
    avx_offset: Option<usize>,
) -> bool {
    if !matches!(mask, 0 | 3 | 7)
        || !xstate_payload_matches(guest, checkpoint, mask, avx_offset)
        || !xstate_payload_matches(original, observed, mask, avx_offset)
    {
        return false;
    }
    let canary = |offset: usize, expected: [u64; 2]| {
        expected.iter().enumerate().all(|(i, value)| {
            let range = offset + i * 8..offset + i * 8 + 8;
            checkpoint[range.clone()] == value.to_le_bytes()
                && original[range.clone()] != checkpoint[range]
        })
    };
    if mask != 0 && (checkpoint[512] & 2 == 0 || guest[512] & 2 == 0) {
        return false;
    }
    if !canary(160 + 15 * 16, LOADED_XMM15) {
        return false;
    }
    if mask == 7 {
        if checkpoint[512] & 4 == 0 || guest[512] & 4 == 0 {
            return false;
        }
        return canary(avx_offset.unwrap() + 15 * 16, LOADED_YMM15_HIGH);
    }
    true
}

/// Standard-format saved-state comparison for this fixture. Absent XSAVE
/// components denote architectural init; their stale payload bytes are ignored.
/// Reserved metadata and x87 register padding are never state evidence.
pub fn xstate_payload_matches(
    original: &[u8],
    observed: &[u8],
    mask: u64,
    avx_offset: Option<usize>,
) -> bool {
    if original.len() != 4096 || observed.len() != 4096 || !matches!(mask, 0 | 1 | 3 | 7) {
        return false;
    }
    if [original, observed]
        .iter()
        .any(|bytes| u32::from_le_bytes(bytes[24..28].try_into().unwrap()) & !0xffff != 0)
    {
        return false;
    }
    let present = |bytes: &[u8]| -> Option<u64> {
        if mask == 0 {
            return Some(3);
        }
        let bits = u64::from_le_bytes(bytes[512..520].try_into().ok()?);
        if bits & !mask != 0 || bytes[520..576].iter().any(|&b| b != 0) {
            None
        } else {
            Some(bits)
        }
    };
    let (Some(a), Some(b)) = (present(original), present(observed)) else {
        return false;
    };
    let x87_init = |bytes: &[u8]| bytes[..5] == [0x7f, 3, 0, 0, 0];
    if a & 1 != 0 && b & 1 != 0 {
        if original[..5] != observed[..5] {
            return false;
        }
        let top = (u16::from_le_bytes(original[2..4].try_into().unwrap()) >> 11) as usize & 7;
        for i in 0..8 {
            if original[4] & (1 << ((top + i) & 7)) != 0
                && original[32 + i * 16..42 + i * 16] != observed[32 + i * 16..42 + i * 16]
            {
                return false;
            }
        }
    } else if a & 1 != 0 && !x87_init(original) || b & 1 != 0 && !x87_init(observed) {
        return false;
    }
    let component = |range: core::ops::Range<usize>, bit: u64| {
        original[range.clone()]
            .iter()
            .zip(&observed[range])
            .all(|(&x, &y)| {
                (if a & bit != 0 { x } else { 0 }) == (if b & bit != 0 { y } else { 0 })
            })
    };
    // XSAVE writes MXCSR when either SSE or AVX is requested, independently of BV.
    if mask == 0 || mask & 6 != 0 {
        if original[24..28] != observed[24..28] || !component(160..416, 2) {
            return false;
        }
    }
    if mask & 4 != 0 {
        let Some(offset) = avx_offset else {
            return false;
        };
        if offset < 576 || offset > 4096 - 256 || !component(offset..offset + 256, 4) {
            return false;
        }
    }
    true
}

#[repr(C)]
#[derive(Default)]
pub struct Fixture {
    pub callback: u64,
    pub opaque: u64,
    pub mask: u64,
    pub original_fx: u64,
    pub original_xsave: u64,
    pub observed_fx: u64,
    pub observed_xsave: u64,
    pub status: u64,
    pub original_cr4: u64,
    pub original_xcr0: u64,
    pub observed_cr4: u64,
    pub observed_xcr0: u64,
    pub original_flags: u64,
    pub observed_flags: u64,
    pub abi_failures: u64,
    pub original_cr0: u64,
    pub observed_cr0: u64,
    pub original_efer: u64,
    pub observed_efer: u64,
    pub reserved: u64,
}
const _: () = assert!(core::mem::size_of::<Fixture>() == 160);
impl Fixture {
    pub fn restored_controls(&self) -> bool {
        self.status == 0
            && self.abi_failures == 0
            && self.original_cr4 == self.observed_cr4
            && self.original_xcr0 == self.observed_xcr0
            && self.original_flags == self.observed_flags
            && self.original_cr0 == self.observed_cr0
            && self.original_efer == self.observed_efer
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_exit_requires_success_capture_and_completed_restoration() {
        let mut c = Context {
            fault_kind: 13,
            fault_recovered: 1,
            fault_stage: 4,
            vmrun_attempts: 1,
            guest_capture_stage: 1,
            exit_code: 0x81,
            guest_exit_rip: 0x1020,
            guest_rax: 0x51554d5552455455,
            fault_vector: 6,
            fault_rip: 0x1234,
            mutation_stage: 8,
            hsave: 0x4000,
            checkpoint_hsave: 0x4000,
            checkpoint_efer: 0x1000,
            checkpoint_extra: 0x8000,
            checkpoint_xstate: 0x9000,
            ..Context::default()
        };
        let valid =
            |c: &Context| c.recovered_post_exit_fault(6, 0x1234, 0x1020, &[0; 10], &[0; 10]);
        assert!(valid(&c) && c.restored_post_exit_checkpoint());
        c.vmrun_attempts = 0;
        assert!(!valid(&c));
        c.vmrun_attempts = 1;
        c.guest_capture_stage = 0;
        assert!(!valid(&c));
        c.guest_capture_stage = 1;
        c.exit_code = 0xffffffff;
        assert!(!valid(&c));
        c.exit_code = 0x81;
        c.guest_exit_rip += 1;
        assert!(!valid(&c));
        c.guest_exit_rip -= 1;
        c.fault_kind = 15;
        assert!(!valid(&c));
        c.fault_kind = 13;
        c.mutation_stage = 7;
        assert!(!c.restored_post_exit_checkpoint());
        c.mutation_stage = 8;
        c.original_xstate = c.checkpoint_xstate;
        assert!(!c.restored_post_exit_checkpoint());
    }

    #[test]
    fn post_exit_extra_uses_prepared_guest_descriptors_and_host_fields() {
        let original = [0u8; 4096];
        let mut checkpoint = original;
        let descriptors = [0x31u8; 32];
        checkpoint[0x470..0x480].copy_from_slice(&descriptors[..16]);
        checkpoint[0x490..0x4a0].copy_from_slice(&descriptors[16..]);
        for (offset, value) in LOADED_EXTRA_CANARIES {
            checkpoint[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let valid =
            |cp: &[u8], obs: &[u8]| post_exit_extra_restored(&original, &descriptors, cp, obs);
        assert!(valid(&checkpoint, &original));
        assert!(!valid(&checkpoint, &checkpoint));
        checkpoint[0x490] = 0;
        assert!(!valid(&checkpoint, &original));
        checkpoint[0x490] = 0x31;
        checkpoint[0x448] = 1;
        assert!(!valid(&checkpoint, &original));
    }

    #[test]
    fn post_exit_xstate_requires_executed_fld1_and_zero_vectors() {
        for mask in [0, 3, 7] {
            let mut source = [0u8; 4096];
            source[..2].copy_from_slice(&0x37fu16.to_le_bytes());
            source[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
            let mut original = source;
            original[400..416].fill(0x42);
            if mask != 0 {
                original[512] = mask as u8;
            }
            if mask == 7 {
                original[816..832].fill(0x31);
            }
            let mut checkpoint = source;
            checkpoint[2..4].copy_from_slice(&0x3800u16.to_le_bytes());
            checkpoint[4] = 0x80;
            checkpoint[32..40].copy_from_slice(&0x8000000000000000u64.to_le_bytes());
            checkpoint[40..42].copy_from_slice(&0x3fffu16.to_le_bytes());
            if mask != 0 {
                checkpoint[512] = 1;
            } // SSE/AVX init may be absent.
            let valid = |cp: &[u8], obs: &[u8]| {
                post_exit_xstate_restored(&original, &source, cp, obs, mask, Some(576))
            };
            assert!(valid(&checkpoint, &original));
            assert!(!valid(&source, &original));
            assert!(!valid(&checkpoint, &checkpoint));
            checkpoint[40] ^= 1;
            assert!(!valid(&checkpoint, &original));
            checkpoint[40] ^= 1;
            if mask != 0 {
                checkpoint[512] = mask as u8;
            }
            checkpoint[400] = 1;
            assert!(!valid(&checkpoint, &original));
        }
    }

    #[test]
    fn xstate_checkpoint_requires_completed_cleanup_and_independent_capture() {
        let mut c = Context {
            fault_kind: 10,
            hsave: 0x4000,
            checkpoint_hsave: 0x4000,
            checkpoint_extra: 0x8000,
            checkpoint_xstate: 0x9000,
            checkpoint_efer: 0x1d00,
            mutation_stage: 6,
            ..Context::default()
        };
        c.original[4] = 0xd00;
        c.observed = c.original;
        assert!(c.restored_xstate_checkpoint());
        c.mutation_stage = 5;
        assert!(!c.restored_xstate_checkpoint());
        c.mutation_stage = 6;
        for kind in [7, 8, 9, 12] {
            c.fault_kind = kind;
            assert!(!c.restored_xstate_checkpoint());
        }
        c.fault_kind = 11;
        assert!(c.restored_xstate_checkpoint());
        c.guest_xstate = c.checkpoint_xstate;
        assert!(!c.restored_xstate_checkpoint());
        c.guest_xstate = 0;
        c.observed[5] = 0x4000;
        assert!(!c.restored_xstate_checkpoint());
    }

    #[test]
    fn loaded_vectors_require_actual_changed_payload_and_complete_restoration() {
        for mask in [0, 3, 7] {
            let mut original = [0u8; 4096];
            original[..2].copy_from_slice(&0x37fu16.to_le_bytes());
            original[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
            if mask != 0 {
                original[512] = mask as u8;
            }
            let mut guest = original;
            for (i, v) in LOADED_XMM15.iter().enumerate() {
                guest[400 + i * 8..408 + i * 8].copy_from_slice(&v.to_le_bytes());
            }
            if mask == 7 {
                for (i, v) in LOADED_YMM15_HIGH.iter().enumerate() {
                    guest[816 + i * 8..824 + i * 8].copy_from_slice(&v.to_le_bytes());
                }
            }
            let valid = |checkpoint: &[u8], observed: &[u8]| {
                loaded_xstate_restored(&original, &guest, checkpoint, observed, mask, Some(576))
            };
            assert!(valid(&guest, &original));
            assert!(!valid(&original, &original)); // No actual load.
            assert!(!valid(&guest, &guest)); // No restore.
            let mut damaged = guest;
            damaged[400] ^= 1;
            assert!(!valid(&damaged, &original));
            damaged = guest;
            damaged[160] = 3;
            assert!(!valid(&damaged, &original));
            if mask == 7 {
                damaged = guest;
                damaged[816] ^= 1;
                assert!(!valid(&damaged, &original));
            }
            if mask != 0 {
                damaged = guest;
                damaged[512] &= !2;
                assert!(!valid(&damaged, &original));
            }
            assert!(!valid(&guest[..4095], &original));
        }
    }

    #[test]
    fn loaded_checkpoint_requires_completed_cleanup_and_owned_observation() {
        let mut c = Context {
            fault_kind: 7,
            hsave: 0x4000,
            checkpoint_hsave: 0x4000,
            checkpoint_extra: 0x8000,
            checkpoint_efer: 0x1d00,
            mutation_stage: 4,
            ..Context::default()
        };
        c.original[4] = 0xd00;
        c.observed = c.original;
        assert!(c.restored_loaded_checkpoint());
        for kind in [0, 4, 5, 9] {
            c.fault_kind = kind;
            assert!(!c.restored_loaded_checkpoint());
        }
        c.fault_kind = 8;
        c.mutation_stage = 3;
        assert!(!c.restored_loaded_checkpoint());
        c.mutation_stage = 4;
        c.checkpoint_extra = 1;
        assert!(!c.restored_loaded_checkpoint());
        c.checkpoint_extra = 0x8000;
        c.checkpoint_rflags = 1 << 9;
        assert!(!c.restored_loaded_checkpoint());
        c.checkpoint_rflags = 0;
        c.observed[4] |= 0x1000;
        assert!(!c.restored_loaded_checkpoint());
    }

    #[test]
    fn loaded_extra_evidence_rejects_unchanged_or_unrestored_targets() {
        let original = [0u8; 4096];
        let mut checkpoint = original;
        for (offset, value) in LOADED_EXTRA_CANARIES {
            checkpoint[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        assert!(loaded_extra_restored(&original, &checkpoint, &original));
        assert!(!loaded_extra_restored(&original, &original, &original));
        assert!(!loaded_extra_restored(
            &checkpoint,
            &checkpoint,
            &checkpoint
        ));
        assert!(!loaded_extra_restored(&original, &checkpoint, &checkpoint));
        let mut wrong_inherited = checkpoint;
        wrong_inherited[0x448] = 1; // FS base must remain host-derived.
        assert!(!loaded_extra_restored(
            &original,
            &wrong_inherited,
            &original
        ));
        for (offset, _) in LOADED_EXTRA_CANARIES {
            let mut corrupt = checkpoint;
            corrupt[offset] ^= 1;
            assert!(!loaded_extra_restored(&original, &corrupt, &original));
        }
        assert!(!loaded_extra_restored(
            &original,
            &checkpoint[..4095],
            &original
        ));
    }

    #[test]
    fn armed_checkpoint_requires_exact_binding_and_completed_restore() {
        let mut c = Context {
            fault_kind: 4,
            hsave: 0x4000,
            checkpoint_hsave: 0x4000,
            checkpoint_efer: 0x1d00,
            mutation_stage: 2,
            ..Context::default()
        };
        c.original[4] = 0xd00;
        c.observed = c.original;
        assert!(c.restored_armed_checkpoint());
        c.checkpoint_efer = 0x1000;
        assert!(!c.restored_armed_checkpoint());
        c.checkpoint_efer = 0x1d00;
        c.checkpoint_hsave = 0x5000;
        assert!(!c.restored_armed_checkpoint());
        c.checkpoint_hsave = 0x4000;
        c.mutation_stage = 1;
        assert!(!c.restored_armed_checkpoint());
        c.mutation_stage = 2;
        c.checkpoint_rflags = 1 << 9;
        assert!(!c.restored_armed_checkpoint());
        c.checkpoint_rflags = 0;
        c.observed[5] = 0x4000;
        assert!(!c.restored_armed_checkpoint());
        c.observed[5] = 0;
        c.original[5] = 0x4000;
        c.observed = c.original;
        assert!(!c.restored_armed_checkpoint());
    }

    #[test]
    fn bounded_fault_recovery_requires_exact_allowlist_and_restored_idtr() {
        let idtr = [3u8; 10];
        let mut c = Context {
            fault_recovered: 1,
            fault_stage: 4,
            fault_vector: 6,
            fault_rip: 0x1234,
            ..Context::default()
        };
        c.original_idtr[..10].copy_from_slice(&idtr);
        c.observed_idtr[..10].copy_from_slice(&idtr);
        assert!(c.recovered_host_fault(6, 0x1234, &idtr, &idtr));
        assert!(!c.recovered_host_fault(13, 0x1234, &idtr, &idtr));
        assert!(!c.recovered_host_fault(6, 0x1235, &idtr, &idtr));
        assert!(!c.recovered_host_fault(6, 0x1234, &idtr, &[0; 10]));
        c.fault_error = 1;
        assert!(!c.recovered_host_fault(6, 0x1234, &idtr, &idtr));
        c.fault_error = 0;
        c.fault_stage = 3;
        assert!(!c.recovered_host_fault(6, 0x1234, &idtr, &idtr));
        c.fault_stage = 4;
        c.vmrun_attempts = 1;
        assert!(!c.recovered_host_fault(6, 0x1234, &idtr, &idtr));
        c.vmrun_attempts = 0;
        c.fault_recovered = 0;
        assert!(!c.recovered_host_fault(6, 0x1234, &idtr, &idtr));
    }
    #[test]
    fn idt_gate_keeps_full_handler_address_and_zero_reserved_fields() {
        assert_eq!(
            interrupt_gate(0x123456789abcdef0, 0x38),
            [
                0xf0, 0xde, 0x38, 0, 0, 0x8e, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn control_comparison_checks_every_reported_register() {
        for index in 0..8 {
            let mut context = Context::default();
            context.observed[index] = 1;
            assert!(!context.restored_controls());
        }
        let mut context = Context::default();
        assert!(context.restored_controls());
        context.observed_dr6 = 1;
        assert!(!context.restored_controls());
        context.observed_dr6 = 0;
        context.observed_xcr0 = 1;
        assert!(!context.restored_controls());
        context.observed_xcr0 = 0;
        context.observed_xss = 1;
        assert!(!context.restored_controls());
    }

    #[test]
    fn enclosing_fixture_requires_completion_and_exact_controls() {
        let mut fixture = Fixture::default();
        assert!(fixture.restored_controls());
        fixture.status = 1;
        assert!(!fixture.restored_controls());
        fixture.status = 0;
        fixture.abi_failures = 1;
        assert!(!fixture.restored_controls());
        fixture.abi_failures = 0;
        fixture.observed_cr4 = 1;
        assert!(!fixture.restored_controls());
        fixture.observed_cr4 = 0;
        fixture.observed_xcr0 = 1;
        assert!(!fixture.restored_controls());
        fixture.observed_xcr0 = 0;
        fixture.observed_flags = 1;
        assert!(!fixture.restored_controls());
        fixture.observed_flags = 0;
        fixture.observed_cr0 = 1;
        assert!(!fixture.restored_controls());
        fixture.observed_cr0 = 0;
        fixture.observed_efer = 1;
        assert!(!fixture.restored_controls());
    }

    #[test]
    fn extra_state_checks_saved_fields_and_ignores_unwritten_bytes() {
        let original = [0u8; 4096];
        for offset in [0x440, 0x45f, 0x470, 0x47f, 0x490, 0x49f, 0x600, 0x63f] {
            let mut observed = original;
            observed[offset] = 1;
            assert!(!extra_state_matches(&original, &observed));
        }
        let mut observed = original;
        observed[0x500] = 1;
        assert!(extra_state_matches(&original, &observed));
        assert!(!extra_state_matches(&original[..4095], &observed));
    }

    #[test]
    fn xstate_components_validate_headers_and_ignore_absent_payloads() {
        let mut original = [0u8; 4096];
        original[..2].copy_from_slice(&0x37fu16.to_le_bytes());
        original[512] = 3;
        let mut observed = original;
        observed[512] = 2; // Explicit x87 init and absent x87 are equivalent.
        observed[32] = 91;
        assert!(xstate_payload_matches(&original, &observed, 3, None));
        observed[160] = 1;
        assert!(!xstate_payload_matches(&original, &observed, 3, None));
        observed = original;
        observed[520] = 1;
        assert!(!xstate_payload_matches(&original, &observed, 3, None));
        observed = original;
        observed[512] = 7;
        assert!(!xstate_payload_matches(&original, &observed, 3, None));
        original[512] = 7;
        observed = original;
        observed[640] = 1;
        assert!(!xstate_payload_matches(&original, &observed, 7, Some(640)));
        assert!(!xstate_payload_matches(&original, &original, 7, Some(4096)));
        assert!(!xstate_payload_matches(
            &original[..4095],
            &observed,
            7,
            Some(640)
        ));
    }
    #[test]
    fn legacy_x87_occupied_registers_ignore_padding() {
        let mut original = [0u8; 4096];
        original[4] = 1;
        let mut observed = original;
        observed[42] = 1;
        assert!(xstate_payload_matches(&original, &observed, 0, None));
        observed[32] = 1;
        assert!(!xstate_payload_matches(&original, &observed, 0, None));
    }
}
