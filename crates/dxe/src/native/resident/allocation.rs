//! Boot-time ownership of the separately retained raw resident allocation.
//!
//! UEFI 2.11 7.2.1, 7.2.2 and Table 7.10: a runtime delivery driver allocates
//! RuntimeServicesCode using AllocateAnyPages, trims owned pages with FreePages,
//! and retains the selected runtime extent after EBS. No Reserved allocation,
//! loaded-runtime-PE registration occurs. The explicit low-runtime platform
//! profile instead uses AllocateMaxAddress below1GiB. UEFI2.11 7.2.1's AnyPages
//! requirement applies to drivers not targeted for a specific implementation;
//! this opt-in profile requires actual firmware success and full map admission.
//! The complete raw arena uses Code because its first entry executes under the
//! firmware CR3. Current writable/executable identity backing must be admitted
//! separately; memory-type metadata alone is insufficient. The resident owner
//! supplies private W^X mappings after takeover.
use super::{
    delivery::{ARENA_BYTES, LayoutError, Payload, valid_arena},
    memory::{ResidentMemoryError, validate_runtime_coverage},
};
use svmvisor_hypervisor::{boot::memory::MemoryDescriptor, memory::address::AddressPolicy};
use uefi_raw::{
    Status,
    table::boot::{AllocateType, BootServices, MemoryType},
};

const PAGE_BYTES: u64 = 4096;
const ARENA_PAGES: usize = ARENA_BYTES / PAGE_BYTES as usize;
const RESERVATION_PAGES: usize = ARENA_PAGES * 2;
use svmvisor_hypervisor::host::resident::MAX_RESIDENT_CPUS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationError {
    Firmware(Status),
    Address(u64),
    Cleanup(Status, u64),
    Released,
    Layout(LayoutError),
    Map(ResidentMemoryError),
}

impl AllocationError {
    /// Stable preparation reason, underlying EFI status and relevant address.
    pub fn diagnostic(self) -> (u32, u64, u64) {
        match self {
            Self::Firmware(status) => (1, status.0 as u64, 0),
            Self::Address(address) => (2, 0, address),
            Self::Cleanup(status, address) => (3, status.0 as u64, address),
            Self::Released => (4, 0, 0),
            Self::Layout(_) => (5, 0, 0),
            Self::Map(_) => (6, 0, 0),
        }
    }
}

const LOW_RUNTIME_MAX: u64 = 0x3fff_ffff;
const PLATFORM_MAXIMUM: Option<u64> =
    if cfg!(feature = "native-resident-low-runtime") { Some(LOW_RUNTIME_MAX) } else { None };

// A narrow internal interface permits ownership/failure tests without fake
// BootServices function tables or host physical-address dereferences.
trait PageServices {
    fn allocate_code(&mut self, pages: usize, maximum: Option<u64>) -> Result<u64, Status>;
    fn free_pages(&mut self, base: u64, pages: usize) -> Result<(), Status>;
}
struct FirmwarePages<'a>(&'a BootServices);
impl PageServices for FirmwarePages<'_> {
    fn allocate_code(&mut self, pages: usize, maximum: Option<u64>) -> Result<u64, Status> {
        let mut base = maximum.unwrap_or(0);
        let status = unsafe {
            (self.0.allocate_pages)(
                if maximum.is_some() { AllocateType::MAX_ADDRESS } else { AllocateType::ANY_PAGES },
                MemoryType::RUNTIME_SERVICES_CODE,
                pages,
                &mut base,
            )
        };
        if status == Status::SUCCESS { Ok(base) } else { Err(status) }
    }
    fn free_pages(&mut self, base: u64, pages: usize) -> Result<(), Status> {
        let status = unsafe { (self.0.free_pages)(base, pages) };
        if status == Status::SUCCESS { Ok(()) } else { Err(status) }
    }
}

struct OwnedPages<S: PageServices> {
    services: S,
    base: u64,
    pages: usize,
    keep_pages: usize,
}
impl<S: PageServices> OwnedPages<S> {
    /// Portable single-image AnyPages owner used by the allocator tests.
    #[cfg(test)]
    fn allocate(services: S) -> Result<Self, AllocationError> {
        Self::allocate_count(services, 1)
    }
    #[cfg(test)]
    fn allocate_count(services: S, count: usize) -> Result<Self, AllocationError> {
        Self::allocate_count_at(services, count, None)
    }
    fn allocate_count_at(
        mut services: S,
        count: usize,
        maximum: Option<u64>,
    ) -> Result<Self, AllocationError> {
        if !(1..=MAX_RESIDENT_CPUS).contains(&count) {
            return Err(AllocationError::Address(0));
        }
        let keep_pages = count * ARENA_PAGES;
        let reservation = if count == 1 { RESERVATION_PAGES } else { keep_pages + 512 };
        let base =
            services.allocate_code(reservation, maximum).map_err(AllocationError::Firmware)?;
        let mut owned = Self { services, base, pages: reservation, keep_pages };
        let placement = if maximum.is_some_and(|limit| {
            base.checked_add(reservation as u64 * PAGE_BYTES - 1).is_none_or(|end| end > limit)
        }) {
            Err(AllocationError::Address(base))
        } else {
            owned.trim()
        };
        if let Err(error) = placement {
            // Retain exact remaining ownership when a partial trim fails.
            // Drop retries a failed release, but the reported cleanup failure
            // is never silently converted into successful cleanup.
            owned.release().map_err(|status| AllocationError::Cleanup(status, owned.base))?;
            return Err(error);
        }
        Ok(owned)
    }

    fn trim(&mut self) -> Result<(), AllocationError> {
        let arena = select_pool(self.base, self.keep_pages / ARENA_PAGES)
            .ok_or(AllocationError::Address(self.base))?;
        let prefix = ((arena - self.base) / PAGE_BYTES) as usize;
        if prefix != 0 {
            self.services
                .free_pages(self.base, prefix)
                .map_err(|status| AllocationError::Cleanup(status, self.base))?;
            self.base = arena;
            self.pages -= prefix;
        }
        let suffix = self.pages - self.keep_pages;
        if suffix != 0 {
            self.services.free_pages(arena + self.keep_pages as u64 * PAGE_BYTES, suffix).map_err(
                |status| {
                    AllocationError::Cleanup(status, arena + self.keep_pages as u64 * PAGE_BYTES)
                },
            )?;
            self.pages = self.keep_pages;
        }
        Ok(())
    }

    fn release(&mut self) -> Result<(), Status> {
        if self.pages != 0 {
            self.services.free_pages(self.base, self.pages)?;
            self.pages = 0;
        }
        Ok(())
    }

    fn publish(mut self) -> PublishedArena {
        let arena =
            PublishedArena { base: self.base, bytes: self.keep_pages * PAGE_BYTES as usize };
        self.pages = 0;
        arena
    }

    fn register_and_publish(
        mut self,
        register: impl FnOnce(u64, usize) -> Result<(), Status>,
    ) -> Result<PublishedArena, Status> {
        if self.pages != self.keep_pages {
            return Err(Status::NOT_READY);
        }
        if let Err(error) = register(self.base, self.keep_pages * PAGE_BYTES as usize) {
            self.release()?;
            return Err(error);
        }
        Ok(self.publish())
    }
}
impl<S: PageServices> Drop for OwnedPages<S> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn select_arena(base: u64) -> Option<u64> {
    if base & (PAGE_BYTES - 1) != 0 {
        return None;
    }
    let allocation_end = base.checked_add((RESERVATION_PAGES as u64) * PAGE_BYTES)?;
    let mut selected = base.max(0x100000);
    if selected & 0x1fffff > 0x100000 {
        selected = selected.checked_add(0x1fffff)? & !0x1fffff;
    }
    (valid_arena(selected) && selected.checked_add(ARENA_BYTES as u64)? <= allocation_end)
        .then_some(selected)
}
fn select_pool(base: u64, count: usize) -> Option<u64> {
    if count == 1 {
        return select_arena(base);
    }
    if !(2..=MAX_RESIDENT_CPUS).contains(&count) || base & 4095 != 0 {
        return None;
    }
    let chosen = base.max(0x100000).checked_add(0x1fffff)? & !0x1fffff;
    let bytes = count as u64 * ARENA_BYTES as u64;
    let end = chosen.checked_add(bytes)?;
    (end <= 0x40000000 && end <= base.checked_add(bytes + 0x200000)?).then_some(chosen)
}

/// Rollback owner used only while Boot Services are live. `release` is explicit
/// so callers can report cleanup failure; Drop is a best-effort fallback.
pub struct RuntimeArena<'a> {
    owned: OwnedPages<FirmwarePages<'a>>,
}

/// Allocator-independent retained extent. It has no firmware reference and no
/// destructor or freeing operation. Publication transfers lifetime, not CPU
/// execution authority; the caller still owns every launch-admission gate.
#[derive(Debug, PartialEq, Eq)]
pub struct PublishedArena {
    base: u64,
    bytes: usize,
}
impl PublishedArena {
    pub const fn base(&self) -> u64 {
        self.base
    }
    pub const fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Obtain one bounded arena using the selected runtime allocation profile.
/// Generic AnyPages above the package's 1GiB limit is refused and cleaned up.
/// The explicit platform profile requests MaxAddress below1GiB, checks the
/// whole returned extent, and still requires firmware success; no fallback
/// memory type or unowned physical allocation is used.
///
/// # Safety
/// `services` must be the current live firmware table, called by a runtime
/// delivery driver at an allocation-permitted TPL. Boot Services remain live
/// and serialized until the returned owner is released or published. Functions
/// obey UEFI 2.11 7.2.1/7.2.2, including allocation alignment and ownership.
pub unsafe fn allocate(services: &BootServices) -> Result<RuntimeArena<'_>, AllocationError> {
    OwnedPages::allocate_count_at(FirmwarePages(services), 1, PLATFORM_MAXIMUM)
        .map(|owned| RuntimeArena { owned })
}
/// Allocate one retained pool for a bounded set of private relocated copies.
/// # Safety
/// Same live Boot Services and serialized ownership requirements as `allocate`.
/// This does not activate processors or authorize reuse of their firmware state.
pub unsafe fn allocate_for_processors(
    services: &BootServices,
    count: usize,
) -> Result<RuntimeArena<'_>, AllocationError> {
    OwnedPages::allocate_count_at(FirmwarePages(services), count, PLATFORM_MAXIMUM)
        .map(|owned| RuntimeArena { owned })
}

impl RuntimeArena<'_> {
    pub const fn base(&self) -> u64 {
        self.owned.base
    }
    pub const fn bytes(&self) -> usize {
        self.owned.keep_pages * PAGE_BYTES as usize
    }
    pub const fn processors(&self) -> usize {
        self.owned.keep_pages / ARENA_PAGES
    }
    pub fn slot_base(&self, slot: usize) -> Option<u64> {
        (slot < self.processors()).then(|| self.base() + slot as u64 * ARENA_BYTES as u64)
    }

    pub fn release(&mut self) -> Result<(), Status> {
        self.owned.release()
    }

    /// Consume an actual post-allocation map using the same admission as NPT.
    /// The caller must collect it after all allocation/trim operations; a later
    /// map-changing service invalidates its freshness, not the allocation owner.
    pub fn validate_map(
        &self,
        policy: AddressPolicy,
        descriptors: &[MemoryDescriptor],
    ) -> Result<(), AllocationError> {
        if self.owned.pages != self.owned.keep_pages {
            return Err(AllocationError::Released);
        }
        let monitor = policy
            .validate(self.base(), self.bytes() as u64, PAGE_BYTES)
            .map_err(|_| AllocationError::Address(self.base()))?;
        validate_runtime_coverage(monitor, descriptors, policy.physical_bits())
            .map_err(AllocationError::Map)
    }

    /// Initialize and relocate the checked raw package into this exact owner.
    ///
    /// # Safety
    /// Every owned byte must currently be identity-mapped readable/writable,
    /// unencrypted and exclusive to this call. The package cannot alias this
    /// arena. Verify current permissions independently of map attributes before
    /// calling; UEFI 2.11 7.2.3 reports capabilities, not active permissions.
    /// The destination's entry/code must separately be proven executable before
    /// transferring control. The source is a separately linked audited raw
    /// payload, never a copy of an initialized runtime delivery PE (8.4.1).
    pub unsafe fn initialize(&mut self, package: &Payload<'_>) -> Result<(), AllocationError> {
        if self.processors() != 1 {
            return Err(AllocationError::Address(self.base()));
        }
        unsafe { self.initialize_slot(package, 0) }
    }
    /// Relocate an immutable package into exactly one disjoint pool slot.
    /// # Safety
    /// Same complete current identity/RW/X/cache admission as `initialize`,
    /// applied independently to this slot. No slot may be executing during load.
    pub unsafe fn initialize_slot(
        &mut self,
        package: &Payload<'_>,
        slot: usize,
    ) -> Result<(), AllocationError> {
        if self.owned.pages != self.owned.keep_pages {
            return Err(AllocationError::Released);
        }
        let base = self.slot_base(slot).ok_or(AllocationError::Address(self.base()))?;
        let bytes = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, ARENA_BYTES) };
        package.load(bytes, base).map_err(AllocationError::Layout)
    }

    /// Permanently transfer this allocation out of Boot Services cleanup.
    /// The returned metadata alone does not authorize execution. Call only when
    /// the runtime owns all persistent state and the actual map, permissions,
    /// and linked code closure have passed launch admission. A rejected launch
    /// before this point uses `release`; no freeing exists after this point.
    pub fn publish(self) -> Result<PublishedArena, AllocationError> {
        if self.owned.pages != self.owned.keep_pages {
            return Err(AllocationError::Released);
        }
        Ok(self.owned.publish())
    }

    /// Keep rollback ownership through a fallible firmware publication. The
    /// callback must publish no reference on error; success is followed only
    /// by infallible lifetime transfer. UEFI2.11 7.5.6 configuration tables.
    pub fn register_and_publish(
        self,
        register: impl FnOnce(u64, usize) -> Result<(), Status>,
    ) -> Result<PublishedArena, Status> {
        self.owned.register_and_publish(register)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    #[test]
    fn platform_pool_limits_full_reservation_and_reports_real_failure() {
        // Exact highest legal 26MiB reservation for this 24-CPU target.
        let base = 0x40000000 - 26 * 0x100000;
        let (provider, record) = mock(base, 0);
        let owner = OwnedPages::allocate_count_at(provider, 24, Some(LOW_RUNTIME_MAX)).unwrap();
        assert_eq!(owner.pages, 24 * ARENA_PAGES);
        assert_eq!(record.borrow().maximums, [Some(LOW_RUNTIME_MAX)]);
        assert_eq!(owner.base + owner.pages as u64 * PAGE_BYTES, 0x3fe00000);
        drop(owner);
        assert!(record.borrow().live.is_empty());

        // A malformed firmware response crossing the bound is fully released.
        let (provider, record) = mock(base + 4096, 0);
        let error =
            OwnedPages::allocate_count_at(provider, 24, Some(LOW_RUNTIME_MAX)).err().unwrap();
        assert_eq!(error.diagnostic(), (2, 0, base + 4096));
        assert!(record.borrow().live.is_empty());
        assert_eq!(record.borrow().calls.len(), 2);

        let (mut provider, record) = mock(0, 0);
        provider.base = Err(Status::OUT_OF_RESOURCES);
        let error =
            OwnedPages::allocate_count_at(provider, 24, Some(LOW_RUNTIME_MAX)).err().unwrap();
        assert_eq!(error.diagnostic(), (1, Status::OUT_OF_RESOURCES.0 as u64, 0));
        assert_eq!(record.borrow().calls, [Call::Allocate(26 * 256)]);
    }

    #[test]
    fn portable_high_allocation_reports_address_and_does_not_retry_or_leak() {
        let (provider, record) = mock(0x1_2340_0000, 0);
        let error = OwnedPages::allocate_count(provider, 24).err().unwrap();
        assert_eq!(error.diagnostic(), (2, 0, 0x1_2340_0000));
        assert_eq!(record.borrow().maximums, [None]);
        assert!(record.borrow().live.is_empty());
        assert_eq!(record.borrow().calls.len(), 2);
    }

    #[test]
    fn failed_registration_releases_pool_and_success_retains_it() {
        let (provider, record) = mock(0x200000, 0);
        let owner = OwnedPages::allocate(provider).unwrap();
        assert_eq!(
            owner.register_and_publish(|_, _| Err(Status::OUT_OF_RESOURCES)),
            Err(Status::OUT_OF_RESOURCES)
        );
        assert!(record.borrow().live.is_empty());
        let (provider, record) = mock(0x200000, 0);
        let owner = OwnedPages::allocate(provider).unwrap();
        let published = owner
            .register_and_publish(|base, bytes| {
                assert_eq!(base, 0x200000);
                assert_eq!(bytes, 0x100000);
                Ok(())
            })
            .unwrap();
        assert_eq!(published.base(), 0x200000);
        assert!(!record.borrow().live.is_empty());
    }

    #[test]
    fn processor_pools_preserve_exact_contiguous_ownership_and_slot_layout() {
        for count in [2, 3, 24, 32] {
            for base in [0x180000, 0x200000, 0x3ff000] {
                let (provider, record) = mock(base, 0);
                let owner = OwnedPages::allocate_count(provider, count).unwrap();
                assert_eq!(owner.base & 0x1fffff, 0);
                assert_eq!(owner.pages, count * ARENA_PAGES);
                for slot in 0..count {
                    assert!(svmvisor_hypervisor::host::resident::valid_pool_slot(
                        owner.base + slot as u64 * ARENA_BYTES as u64,
                        owner.base,
                        count as u64 * ARENA_BYTES as u64,
                        slot as u64,
                        slot as u64
                    ));
                }
                drop(owner);
                assert!(record.borrow().live.is_empty());
            }
        }
    }
    #[test]
    fn processor_pool_bounds_and_trim_failure_do_not_leak_or_double_free() {
        for count in [0, 33, usize::MAX] {
            let (provider, record) = mock(0x300000, 0);
            assert!(matches!(
                OwnedPages::allocate_count(provider, count),
                Err(AllocationError::Address(0))
            ));
            assert!(record.borrow().calls.is_empty());
        }
        for fail in [1, 2, 0b110] {
            let (provider, record) = mock(0x380000, fail);
            assert!(OwnedPages::allocate_count(provider, 24).is_err());
            assert!(record.borrow().live.is_empty());
        }
        assert_eq!(select_pool(0x3e000000, 32), Some(0x3e000000));
        assert_eq!(select_pool(0x3e001000, 32), None);
        assert_eq!(select_pool(u64::MAX & !4095, 2), None);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Allocate(usize),
        Free(u64, usize),
    }
    #[derive(Default)]
    struct Record {
        calls: Vec<Call>,
        maximums: Vec<Option<u64>>,
        // Successful allocation pages remain owned until a successful free.
        live: Vec<u64>,
        free_attempts: usize,
        fail_mask: u64,
    }
    struct Mock {
        base: Result<u64, Status>,
        record: Rc<RefCell<Record>>,
    }
    impl PageServices for Mock {
        fn allocate_code(&mut self, pages: usize, maximum: Option<u64>) -> Result<u64, Status> {
            let mut r = self.record.borrow_mut();
            r.calls.push(Call::Allocate(pages));
            r.maximums.push(maximum);
            let base = self.base?;
            r.live.extend((0..pages).map(|i| base + i as u64 * PAGE_BYTES));
            Ok(base)
        }
        fn free_pages(&mut self, base: u64, pages: usize) -> Result<(), Status> {
            let mut r = self.record.borrow_mut();
            r.calls.push(Call::Free(base, pages));
            let attempt = r.free_attempts;
            r.free_attempts += 1;
            if r.fail_mask & (1 << attempt) != 0 {
                return Err(Status::DEVICE_ERROR);
            }
            for i in 0..pages {
                let page = base + i as u64 * PAGE_BYTES;
                let index = r
                    .live
                    .iter()
                    .position(|p| *p == page)
                    .expect("free must touch an allocated, not already freed page");
                r.live.remove(index);
            }
            Ok(())
        }
    }
    fn mock(base: u64, fail_mask: u64) -> (Mock, Rc<RefCell<Record>>) {
        let record = Rc::new(RefCell::new(Record { fail_mask, ..Record::default() }));
        (Mock { base: Ok(base), record: record.clone() }, record)
    }

    #[test]
    fn every_page_offset_selects_one_contiguous_megabyte_in_one_window() {
        for base in (0..0x600000).step_by(4096) {
            let selected = select_arena(base).unwrap();
            assert!(valid_arena(selected));
            assert!(selected >= base);
            assert!(selected + ARENA_BYTES as u64 <= base + 0x200000);
        }
        assert_eq!(select_arena(0x3ff00000), Some(0x3ff00000));
        for base in [0x3ff01000, 0x40000000, u64::MAX & !4095, 0x200001] {
            assert_eq!(select_arena(base), None);
        }
    }

    #[test]
    fn prefix_and_suffix_are_freed_before_exact_owner_publication() {
        let (mock, record) = mock(0x380000, 0);
        let owner = OwnedPages::allocate(mock).unwrap();
        assert_eq!(owner.base, 0x400000);
        assert_eq!(owner.pages, ARENA_PAGES);
        assert_eq!(
            record.borrow().calls.as_slice(),
            &[Call::Allocate(512), Call::Free(0x380000, 128), Call::Free(0x500000, 128),]
        );
        let published = owner.publish();
        assert_eq!(published.base(), 0x400000);
        assert_eq!(published.bytes(), ARENA_BYTES);
        assert_eq!(record.borrow().calls.len(), 3, "publication must not free");
        assert_eq!(record.borrow().live.len(), ARENA_PAGES);
        drop(published);
        assert_eq!(record.borrow().calls.len(), 3, "retained metadata has no cleanup");
    }

    #[test]
    fn rejected_launch_releases_only_selected_pages_without_double_free() {
        let (mock, record) = mock(0x380000, 0);
        let mut owner = OwnedPages::allocate(mock).unwrap();
        owner.release().unwrap();
        owner.release().unwrap();
        drop(owner);
        let r = record.borrow();
        assert!(r.live.is_empty());
        assert_eq!(r.calls.last(), Some(&Call::Free(0x400000, 256)));
        assert_eq!(r.calls.len(), 4);
    }

    #[test]
    fn trim_failure_rolls_back_exact_remaining_allocation() {
        for fail_mask in [1, 2] {
            let (mock, record) = mock(0x380000, fail_mask);
            assert!(matches!(
                OwnedPages::allocate(mock),
                Err(AllocationError::Cleanup(Status::DEVICE_ERROR, _))
            ));
            let r = record.borrow();
            assert!(r.live.is_empty());
            let remaining =
                if fail_mask == 1 { Call::Free(0x380000, 512) } else { Call::Free(0x400000, 384) };
            assert_eq!(r.calls.last(), Some(&remaining));
        }
    }

    #[test]
    fn failed_cleanup_retains_ownership_for_drop_retry_and_reports_failure() {
        // Prefix succeeds, suffix and explicit rollback fail, Drop then frees
        // the remaining 384 pages exactly once.
        let (mock, record) = mock(0x380000, 0b110);
        assert!(matches!(
            OwnedPages::allocate(mock),
            Err(AllocationError::Cleanup(Status::DEVICE_ERROR, _))
        ));
        let r = record.borrow();
        assert!(r.live.is_empty());
        assert_eq!(r.calls[3..], [Call::Free(0x400000, 384), Call::Free(0x400000, 384)]);
    }

    #[test]
    fn explicit_release_failure_preserves_live_owner_until_retry() {
        let (mock, record) = mock(0x400000, 0b10);
        let mut owner = OwnedPages::allocate(mock).unwrap();
        assert_eq!(owner.release(), Err(Status::DEVICE_ERROR));
        assert_eq!(owner.pages, ARENA_PAGES);
        assert_eq!(record.borrow().live.len(), ARENA_PAGES);
        owner.release().unwrap();
        assert!(record.borrow().live.is_empty());
    }

    #[test]
    fn unsupported_anypages_address_is_freed_before_error() {
        let (mock, record) = mock(0x40000000, 0);
        assert!(matches!(OwnedPages::allocate(mock), Err(AllocationError::Address(0x40000000))));
        let r = record.borrow();
        assert!(r.live.is_empty());
        assert_eq!(r.calls.as_slice(), &[Call::Allocate(512), Call::Free(0x40000000, 512)]);
    }

    #[test]
    fn allocation_failure_never_creates_a_cleanup_obligation() {
        let (mut mock, record) = mock(0, 0);
        mock.base = Err(Status::OUT_OF_RESOURCES);
        assert!(matches!(
            OwnedPages::allocate(mock),
            Err(AllocationError::Firmware(Status::OUT_OF_RESOURCES))
        ));
        assert_eq!(record.borrow().calls.as_slice(), &[Call::Allocate(512)]);
    }

    mod firmware_boundary_tests {
        use super::*;
        use core::{
            mem::{MaybeUninit, size_of},
            ptr,
        };

        #[derive(Default)]
        struct FirmwareRecord {
            allocation: Option<(u32, u32, usize, u64)>,
            frees: Vec<(u64, usize)>,
            returned_base: u64,
            fail_allocation: bool,
        }
        std::thread_local! {
            static FIRMWARE: RefCell<FirmwareRecord> = RefCell::new(FirmwareRecord::default());
        }
        unsafe extern "efiapi" fn unused() {
            panic!("unexpected firmware service in runtime allocation test")
        }
        unsafe extern "efiapi" fn allocate_pages(
            kind: AllocateType,
            memory: MemoryType,
            count: usize,
            address: *mut u64,
        ) -> Status {
            FIRMWARE.with(|record| {
                let mut record = record.borrow_mut();
                record.allocation = Some((kind.0, memory.0, count, unsafe { *address }));
                if record.fail_allocation {
                    return Status::OUT_OF_RESOURCES;
                }
                unsafe { *address = record.returned_base };
                Status::SUCCESS
            })
        }
        unsafe extern "efiapi" fn free_pages(address: u64, count: usize) -> Status {
            FIRMWARE.with(|record| record.borrow_mut().frees.push((address, count)));
            Status::SUCCESS
        }
        fn services(base: u64, fail: bool) -> BootServices {
            FIRMWARE.with(|record| {
                *record.borrow_mut() = FirmwareRecord {
                    returned_base: base,
                    fail_allocation: fail,
                    ..FirmwareRecord::default()
                }
            });
            let mut table = MaybeUninit::<BootServices>::uninit();
            unsafe {
                // Existing native firmware fixtures use this table pattern:
                // unused service pointers are non-NULL and never invoked.
                for index in 0..size_of::<BootServices>() / size_of::<usize>() {
                    table
                        .as_mut_ptr()
                        .cast::<usize>()
                        .add(index)
                        .write(unused as *const () as usize);
                }
                ptr::addr_of_mut!((*table.as_mut_ptr()).header).write(core::mem::zeroed());
                ptr::addr_of_mut!((*table.as_mut_ptr()).allocate_pages).write(allocate_pages);
                ptr::addr_of_mut!((*table.as_mut_ptr()).free_pages).write(free_pages);
                table.assume_init()
            }
        }
        #[test]
        fn public_processor_allocator_uses_firmware_policy_and_rolls_back_exact_pages() {
            let table = services(0x380000, false);
            let owner = unsafe { allocate_for_processors(&table, 24) }.unwrap();
            assert_eq!(
                (owner.base(), owner.bytes(), owner.processors()),
                (0x400000, 0x1800000, 24)
            );
            drop(owner);
            let expected_policy = if cfg!(feature = "native-resident-low-runtime") {
                (1, 0x3fff_ffff)
            } else {
                (0, 0)
            };
            FIRMWARE.with(|record| {
                let record = record.borrow();
                // Independent UEFI numeric values: AnyPages=0, MaxAddress=1,
                // RuntimeServicesCode=5; 24 MiB plus 2 MiB alignment slack.
                assert_eq!(
                    record.allocation,
                    Some((expected_policy.0, 5, 6656, expected_policy.1))
                );
                assert_eq!(record.frees, [(0x380000, 128), (0x1c00000, 384), (0x400000, 6144)]);
            });

            let table = services(0x40000000, false);
            assert!(matches!(
                unsafe { allocate_for_processors(&table, 24) },
                Err(AllocationError::Address(0x40000000))
            ));
            FIRMWARE.with(|record| assert_eq!(record.borrow().frees, [(0x40000000, 6656)]));

            let table = services(0, true);
            assert!(matches!(
                unsafe { allocate_for_processors(&table, 24) },
                Err(AllocationError::Firmware(Status::OUT_OF_RESOURCES))
            ));
            FIRMWARE.with(|record| assert!(record.borrow().frees.is_empty()));
        }
    }
}
