#![cfg(feature = "memory-attribute-firmware")]

use std::collections::BTreeMap;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use svmvisor_dxe::memory_attribute_firmware::{
    CPU_ARCH_PROTOCOL_GUID, CpuArchProtocol, CpuArchSetter, FirmwareAttributes,
    QualifiedCpuContext, QualifiedTableReader, Range, Setter,
};
use svmvisor_memory_attributes::{
    ACCESS_MASK, Attributes, Config, EXECUTE_PROTECT as XP, Error, PAGE_SIZE, READ_ONLY as RO,
    READ_PROTECT as RP,
};
use uefi_raw::{Status, guid};

const NX: u64 = 1 << 63;
const BASE: u64 = 0x400000;
const TWO_MIB: u64 = 1 << 21;
const ONE_GIB: u64 = 1 << 30;
const OWNED: &[Range] = &[Range {
    base: 0,
    length: 1 << 40,
}];

struct State {
    config: Config,
    entries: BTreeMap<u64, u64>,
    calls: Vec<(u64, u64, u64)>,
    reads: usize,
    read_failure: Option<usize>,
    status: Status,
    before_error: bool,
    after_error: bool,
    after_checks: usize,
    do_nothing: bool,
    change_root: bool,
    corrupt_cache: bool,
}

impl State {
    fn new() -> Self {
        Self {
            config: Config {
                root: 0x1000,
                physical_bits: 48,
                nxe: true,
                page1gb: true,
            },
            entries: BTreeMap::new(),
            calls: Vec::new(),
            reads: 0,
            read_failure: None,
            status: Status::SUCCESS,
            before_error: false,
            after_error: false,
            after_checks: 0,
            do_nothing: false,
            change_root: false,
            corrupt_cache: false,
        }
    }

    fn map(&mut self, base: u64, size: u64, attributes: u64, extra: u64) -> u64 {
        self.entries
            .insert(0x1000 + ((base >> 39) & 511) * 8, 0x2003);
        let pdpt = 0x2000 + ((base >> 30) & 511) * 8;
        let pd = 0x3000 + ((base >> 21) & 511) * 8;
        let slot = match size {
            ONE_GIB => pdpt,
            TWO_MIB => {
                self.entries.insert(pdpt, 0x3003);
                pd
            }
            PAGE_SIZE => {
                self.entries.insert(pdpt, 0x3003);
                self.entries.insert(pd, 0x4003);
                0x4000 + ((base >> 12) & 511) * 8
            }
            _ => panic!("unsupported fixture size"),
        };
        let mut value = base | 3 | extra | if size > PAGE_SIZE { 0x80 } else { 0 };
        if attributes & RP != 0 {
            value &= !1;
        }
        if attributes & RO != 0 {
            value &= !2;
        }
        if attributes & XP != 0 {
            value |= NX;
        }
        self.entries.insert(slot, value);
        slot
    }

    fn leaf(&self, base: u64) -> Option<u64> {
        let mut table = self.config.root;
        for level in (1u32..=4).rev() {
            let slot = table + ((base >> (12 + 9 * (level - 1))) & 511) * 8;
            let value = *self.entries.get(&slot)?;
            if level == 1 || value & 0x80 != 0 {
                return Some(slot);
            }
            table = value & 0x000f_ffff_ffff_f000;
        }
        None
    }
}

#[derive(Clone)]
struct Reader(Arc<Mutex<State>>);

// SAFETY: Host entries are owned BTreeMap values, never native pointers. Tests
// serialize this fixture; only the modeled synchronous callback changes them.
unsafe impl QualifiedTableReader for Reader {
    fn config(&self) -> Config {
        self.0.lock().unwrap().config
    }
    fn read_entry(&mut self, physical: u64) -> Result<u64, Error> {
        let mut state = self.0.lock().unwrap();
        state.reads += 1;
        if state.read_failure == Some(state.reads) {
            return Err(Error::DeviceError);
        }
        Ok(*state.entries.get(&physical).unwrap_or(&0))
    }
}

struct Context(Arc<Mutex<State>>);

// SAFETY: This context authorizes a host callback that only mutates BTreeMap
// data. There are no control registers, firmware pointers or hardware side effects.
unsafe impl QualifiedCpuContext for Context {
    fn verify(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
        if self.0.lock().unwrap().before_error {
            Err(Error::AccessDenied)
        } else {
            Ok(())
        }
    }
    fn verify_after(&mut self) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        state.after_checks += 1;
        if state.after_error {
            Err(Error::DeviceError)
        } else {
            Ok(())
        }
    }
}

#[repr(C)]
struct MockCpu {
    protocol: CpuArchProtocol,
    state: Arc<Mutex<State>>,
}

unsafe extern "efiapi" fn cpu_set(
    this: *mut CpuArchProtocol,
    base: u64,
    length: u64,
    mask: u64,
) -> Status {
    // SAFETY: Every test passes the first field of a live, boxed MockCpu.
    let mock = unsafe { &*this.cast::<MockCpu>() };
    let mut state = mock.state.lock().unwrap();
    state.calls.push((base, length, mask));
    if !state.do_nothing {
        let slot = state
            .leaf(base)
            .expect("the bridge must query before its callback");
        let value = state.entries.get_mut(&slot).unwrap();
        *value = (*value | 3) & !NX;
        if mask & RP != 0 {
            *value &= !1;
        }
        if mask & RO != 0 {
            *value &= !2;
        }
        if mask & XP != 0 {
            *value |= NX;
        }
        if state.corrupt_cache {
            *state.entries.get_mut(&slot).unwrap() ^= 1 << 3;
        }
    }
    if state.change_root {
        state.config.root = 0x5000;
    }
    state.status
}

type Bridge = FirmwareAttributes<'static, Reader, CpuArchSetter<Context>>;

fn fixture(
    base: u64,
    size: u64,
    attrs: u64,
    extra: u64,
    owned: &'static [Range],
    protected: &'static [Range],
) -> (Box<MockCpu>, Arc<Mutex<State>>, Bridge) {
    let state = Arc::new(Mutex::new(State::new()));
    state.lock().unwrap().map(base, size, attrs, extra);
    let mut cpu = Box::new(MockCpu {
        protocol: CpuArchProtocol {
            opaque_slots: [0; 7],
            set_memory_attributes: cpu_set,
            number_of_timers: 1,
            dma_buffer_alignment: 4,
        },
        state: state.clone(),
    });
    let pointer = NonNull::from(cpu.as_mut()).cast::<CpuArchProtocol>();
    // SAFETY: The boxed MockCpu stays alive in the returned tuple for the test.
    let setter = unsafe { CpuArchSetter::new(pointer, Context(state.clone())) };
    let bridge = FirmwareAttributes::new(Reader(state.clone()), setter, owned, protected).unwrap();
    (cpu, state, bridge)
}

#[test]
fn cpu_protocol_guid_and_slot_layout_match_pi() {
    assert_eq!(
        CPU_ARCH_PROTOCOL_GUID,
        guid!("26baccb1-6f42-11d4-bce7-0080c73c8881")
    );
    assert_eq!(
        std::mem::offset_of!(CpuArchProtocol, set_memory_attributes),
        7 * std::mem::size_of::<usize>()
    );
    assert_eq!(
        std::mem::offset_of!(CpuArchProtocol, number_of_timers),
        8 * std::mem::size_of::<usize>()
    );
    assert_eq!(
        std::mem::offset_of!(CpuArchProtocol, dma_buffer_alignment),
        8 * std::mem::size_of::<usize>() + 4
    );
}

#[test]
fn all_protection_combinations_get_set_clear_through_actual_efi_callback() {
    let masks = [RP, RO, XP, RP | RO, RP | XP, RO | XP, ACCESS_MASK];
    for mask in masks {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        assert_eq!(bridge.set(BASE, PAGE_SIZE, mask), Ok(()));
        assert_eq!(bridge.get(BASE, PAGE_SIZE), Ok(mask));
        assert_eq!(bridge.clear(BASE, PAGE_SIZE, mask), Ok(()));
        assert_eq!(bridge.get(BASE, PAGE_SIZE), Ok(0));
        assert_eq!(
            state.lock().unwrap().calls,
            [(BASE, PAGE_SIZE, mask), (BASE, PAGE_SIZE, 0)]
        );
        assert!(!bridge.is_poisoned());
    }
}

#[test]
fn incremental_changes_preserve_unmentioned_protections() {
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, RO, 0, OWNED, &[]);
    bridge.set(BASE, PAGE_SIZE, XP).unwrap();
    bridge.clear(BASE, PAGE_SIZE, RO).unwrap();
    assert_eq!(
        state.lock().unwrap().calls,
        [(BASE, PAGE_SIZE, RO | XP), (BASE, PAGE_SIZE, XP)]
    );
}

#[test]
fn whole_large_leaves_preserve_non_access_flags_and_never_request_cache_bits() {
    for (base, size) in [(BASE, TWO_MIB), (ONE_GIB, ONE_GIB)] {
        // PAT-large, global, accessed/dirty, PWT/PCD, low and high software bits.
        let extra =
            (1 << 12) | (1 << 8) | (1 << 5) | (1 << 6) | (1 << 3) | (1 << 4) | (1 << 9) | (1 << 52);
        let (_cpu, state, mut bridge) = fixture(base, size, 0, extra, OWNED, &[]);
        let before = {
            let s = state.lock().unwrap();
            s.entries[&s.leaf(base).unwrap()]
        };
        bridge.set(base, size, RO | XP).unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.calls, [(base, size, RO | XP)]);
        assert_eq!(
            s.entries[&s.leaf(base).unwrap()] & !(NX | 3),
            before & !(NX | 3)
        );
    }
}

#[test]
fn partial_huge_leaf_and_multiple_small_leaves_refuse_changes() {
    let (_cpu, state, mut bridge) = fixture(BASE, TWO_MIB, 0, 0, OWNED, &[]);
    assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::Unsupported));
    assert_eq!(
        bridge.set(BASE + PAGE_SIZE, PAGE_SIZE, RO),
        Err(Error::Unsupported)
    );
    assert!(state.lock().unwrap().calls.is_empty());
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    state.lock().unwrap().map(BASE + PAGE_SIZE, PAGE_SIZE, 0, 0);
    assert_eq!(bridge.set(BASE, 2 * PAGE_SIZE, RO), Err(Error::Unsupported));
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn inherited_restrictions_are_reported_but_not_changed() {
    for restriction in [RO, RP, XP] {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        {
            let mut s = state.lock().unwrap();
            let parent = s.entries.get_mut(&0x1000).unwrap();
            if restriction == RO {
                *parent &= !2;
            }
            if restriction == RP {
                *parent &= !1;
            }
            if restriction == XP {
                *parent |= NX;
            }
        }
        assert_eq!(bridge.get(BASE, PAGE_SIZE), Ok(restriction));
        assert_eq!(
            bridge.clear(BASE, PAGE_SIZE, restriction),
            Err(Error::Unsupported)
        );
        assert_eq!(bridge.set(BASE, PAGE_SIZE, restriction), Ok(()));
        assert!(state.lock().unwrap().calls.is_empty());
    }
}

#[test]
fn noops_need_no_policy_grant_and_may_cover_partial_or_multiple_leaves() {
    let (_cpu, state, mut bridge) = fixture(BASE, TWO_MIB, RO, 0, &[], &[]);
    assert_eq!(bridge.set(BASE + PAGE_SIZE, PAGE_SIZE, RO), Ok(()));
    assert_eq!(bridge.clear(BASE, TWO_MIB, XP), Ok(()));
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn mixed_permissions_and_absent_mappings_never_reach_firmware() {
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    state
        .lock()
        .unwrap()
        .map(BASE + PAGE_SIZE, PAGE_SIZE, RO, 0);
    assert_eq!(bridge.get(BASE, 2 * PAGE_SIZE), Err(Error::NoMapping));
    assert_eq!(bridge.set(BASE, 2 * PAGE_SIZE, RO), Err(Error::Unsupported));
    assert_eq!(
        bridge.set(BASE + 2 * PAGE_SIZE, PAGE_SIZE, RO),
        Err(Error::Unsupported)
    );
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn nonidentity_reserved_and_unsupported_nx_encodings_are_rejected() {
    for bits in [1 << 59, 1 << 48] {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, bits, OWNED, &[]);
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::Unsupported));
        assert!(state.lock().unwrap().calls.is_empty());
    }
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    {
        let mut s = state.lock().unwrap();
        let slot = s.leaf(BASE).unwrap();
        *s.entries.get_mut(&slot).unwrap() += PAGE_SIZE;
    }
    assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::Unsupported));
    assert!(state.lock().unwrap().calls.is_empty());
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    state.lock().unwrap().config.nxe = false;
    assert_eq!(bridge.set(BASE, PAGE_SIZE, XP), Err(Error::Unsupported));
    assert_eq!(bridge.clear(BASE, PAGE_SIZE, XP), Err(Error::Unsupported));
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn owned_protected_and_source_table_policy_prevents_callback() {
    for (owned, protected) in [
        (&[][..], &[][..]),
        (
            OWNED,
            &[Range {
                base: BASE,
                length: PAGE_SIZE,
            }][..],
        ),
    ] {
        // Leak only small immutable host policy fixtures to match retained policy lifetime.
        let owned = Box::leak(owned.to_vec().into_boxed_slice());
        let protected = Box::leak(protected.to_vec().into_boxed_slice());
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, owned, protected);
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::AccessDenied));
        assert!(state.lock().unwrap().calls.is_empty());
    }
    let (_cpu, state, mut bridge) = fixture(0x1000, PAGE_SIZE, 0, 0, OWNED, &[]);
    assert_eq!(bridge.set(0x1000, PAGE_SIZE, RO), Err(Error::AccessDenied));
    assert!(state.lock().unwrap().calls.is_empty());
}

#[test]
fn policy_rejects_overflow_unaligned_zero_and_overlapping_intervals() {
    let invalid = [
        vec![Range {
            base: u64::MAX - 4095,
            length: PAGE_SIZE,
        }],
        vec![Range {
            base: 1,
            length: PAGE_SIZE,
        }],
        vec![Range { base: 0, length: 0 }],
        vec![
            Range {
                base: BASE,
                length: 2 * PAGE_SIZE,
            },
            Range {
                base: BASE + PAGE_SIZE,
                length: PAGE_SIZE,
            },
        ],
    ];
    for policy in invalid {
        let (_cpu, state, _) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        struct Never;
        impl Setter for Never {
            fn assign(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
                panic!("invalid policy")
            }
        }
        assert!(matches!(
            FirmwareAttributes::new(Reader(state), Never, &policy, &[]),
            Err(Error::InvalidParameter)
        ));
    }
}

#[test]
fn parameter_failures_happen_before_reads_or_callbacks() {
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    for (base, length, mask, error) in [
        (BASE, PAGE_SIZE, 0, Error::InvalidParameter),
        (BASE, PAGE_SIZE, 1, Error::InvalidParameter),
        (BASE, 0, RO, Error::InvalidParameter),
        (BASE + 1, PAGE_SIZE, RO, Error::Unsupported),
        (BASE, PAGE_SIZE + 1, RO, Error::Unsupported),
        (
            u64::MAX - PAGE_SIZE + 1,
            PAGE_SIZE,
            RO,
            Error::InvalidParameter,
        ),
    ] {
        assert_eq!(bridge.set(base, length, mask), Err(error));
    }
    let s = state.lock().unwrap();
    assert_eq!(s.reads, 0);
    assert!(s.calls.is_empty());
}

#[test]
fn table_read_failures_on_both_walks_prevent_callback() {
    for failed_read in 1..=8 {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        state.lock().unwrap().read_failure = Some(failed_read);
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::DeviceError));
        assert!(state.lock().unwrap().calls.is_empty());
    }
}

#[test]
fn context_rejection_prevents_firmware_call() {
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    state.lock().unwrap().before_error = true;
    assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::AccessDenied));
    assert!(state.lock().unwrap().calls.is_empty());
    assert_eq!(bridge.setter().last_status(), None);
}

#[test]
fn firmware_errors_preserve_raw_status_and_poison_every_subsequent_operation() {
    for (status, mapped) in [
        (Status::INVALID_PARAMETER, Error::InvalidParameter),
        (Status::UNSUPPORTED, Error::Unsupported),
        (Status::OUT_OF_RESOURCES, Error::OutOfResources),
        (Status::ACCESS_DENIED, Error::AccessDenied),
        (Status::NO_MAPPING, Error::NoMapping),
        (Status::DEVICE_ERROR, Error::DeviceError),
        (Status(Status::ERROR_BIT | 0x5678), Error::DeviceError),
        (Status(1), Error::DeviceError),
    ] {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        state.lock().unwrap().status = status;
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(mapped));
        assert_eq!(bridge.setter().last_status(), Some(status));
        assert!(bridge.is_poisoned());
        assert!(bridge.setter().is_poisoned());
        assert_eq!(bridge.get(BASE, PAGE_SIZE), Err(Error::AccessDenied));
        assert_eq!(bridge.clear(BASE, PAGE_SIZE, RO), Err(Error::AccessDenied));
        assert_eq!(bridge.set(BASE, PAGE_SIZE, XP), Err(Error::AccessDenied));
        let s = state.lock().unwrap();
        assert_eq!(s.calls.len(), 1);
        assert_eq!(s.after_checks, 1);
    }
}

#[test]
fn claimed_success_is_checked_for_actual_attributes_original_root_and_context() {
    for mode in 0..5 {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        {
            let mut s = state.lock().unwrap();
            match mode {
                0 => s.do_nothing = true,
                1 => s.change_root = true,
                2 => s.after_error = true,
                3 => s.read_failure = Some(9),
                4 => s.corrupt_cache = true,
                _ => unreachable!(),
            }
        }
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::DeviceError));
        assert!(bridge.is_poisoned());
        assert_eq!(bridge.setter().last_status(), Some(Status::SUCCESS));
        assert_eq!(state.lock().unwrap().calls.len(), 1);
        assert_eq!(bridge.get(BASE, PAGE_SIZE), Err(Error::AccessDenied));
    }
}

#[test]
fn firmware_error_and_context_failure_are_both_retained() {
    let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
    {
        let mut s = state.lock().unwrap();
        s.status = Status::OUT_OF_RESOURCES;
        s.after_error = true;
    }
    assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::OutOfResources));
    assert_eq!(
        bridge.setter().last_status(),
        Some(Status::OUT_OF_RESOURCES)
    );
    assert_eq!(bridge.setter().last_after_error(), Some(Error::DeviceError));
    assert!(bridge.is_poisoned());
}

#[test]
fn unsupported_one_gib_and_invalid_root_configuration_prevent_callback() {
    let (_cpu, state, mut bridge) = fixture(ONE_GIB, ONE_GIB, 0, 0, OWNED, &[]);
    state.lock().unwrap().config.page1gb = false;
    assert_eq!(bridge.set(ONE_GIB, ONE_GIB, RO), Err(Error::Unsupported));
    assert!(state.lock().unwrap().calls.is_empty());
    for config in [
        Config {
            root: 0x1001,
            physical_bits: 48,
            nxe: true,
            page1gb: true,
        },
        Config {
            root: 0x1000,
            physical_bits: 53,
            nxe: true,
            page1gb: true,
        },
    ] {
        let (_cpu, state, mut bridge) = fixture(BASE, PAGE_SIZE, 0, 0, OWNED, &[]);
        state.lock().unwrap().config = config;
        assert_eq!(bridge.set(BASE, PAGE_SIZE, RO), Err(Error::Unsupported));
        let s = state.lock().unwrap();
        assert_eq!(s.reads, 0);
        assert!(s.calls.is_empty());
    }
}
