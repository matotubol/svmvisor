//! Thin EFI ABI adapter for the separately qualified memory-attribute backend.
//!
//! Constructing this adapter does not install a protocol or change the native
//! admission path. The owner must keep it pinned and alive until it has removed
//! the interface and established that no firmware caller can still use it.

use core::marker::PhantomPinned;
use core::pin::Pin;

use svmvisor_hypervisor::sync::TryLock;
use svmvisor_memory_attributes::{ACCESS_MASK, Attributes, Error, PAGE_SIZE};
use uefi_raw::Status;
use uefi_raw::protocol::memory_protection::MemoryAttributeProtocol;
use uefi_raw::table::boot::MemoryAttribute;

/// A standard three-slot interface followed by private, serialized state.
///
/// Raw EFI callers must obey the usual valid-pointer and lifetime contract:
/// `This` must be the pointer returned by this exact adapter instance; any Get
/// output must be writable, aligned and disjoint from the adapter/backend.
/// Registration, removal, CPU rendezvous and backing-memory access qualification
/// are the owner's responsibilities. The lock rejects reentry instead of
/// spinning at firmware TPL or while an interrupted call holds the lock.
#[repr(C)]
pub struct Adapter<A: Attributes + Send> {
    protocol: MemoryAttributeProtocol,
    backend: TryLock<A>,
    _pinned: PhantomPinned,
}

impl<A: Attributes + Send> Adapter<A> {
    pub const fn new(backend: A) -> Self {
        Self {
            protocol: MemoryAttributeProtocol {
                get_memory_attributes: Self::get,
                set_memory_attributes: Self::set,
                clear_memory_attributes: Self::clear,
            },
            backend: TryLock::new(backend),
            _pinned: PhantomPinned,
        }
    }

    /// Obtain the standard interface at a stable address. Retaining its pointer
    /// past the adapter's lifetime is invalid, even if firmware still lists it.
    pub fn protocol_ptr(self: Pin<&Self>) -> *const MemoryAttributeProtocol {
        // Derive This from the complete allocation, not a subfield borrow:
        // callbacks recover and access state beyond the three protocol slots.
        core::ptr::from_ref(self.get_ref()).cast()
    }

    fn invoke<R>(&self, operation: impl FnOnce(&mut A) -> Result<R, Error>) -> Result<R, Error> {
        // The backend is never exposed elsewhere after construction. Its Memory
        // contract supplies paging and system-wide synchronization beyond this
        // instance's callback lock.
        self.backend.with(operation).unwrap_or(Err(Error::AccessDenied))
    }

    unsafe extern "efiapi" fn get(
        this: *const MemoryAttributeProtocol,
        base: u64,
        length: u64,
        output: *mut MemoryAttribute,
    ) -> Status {
        // Keep EDK2's public validation precedence where it is specified by our
        // compatibility profile. Null/unaligned This and output are rejected.
        if base & (PAGE_SIZE - 1) != 0 || length & (PAGE_SIZE - 1) != 0 {
            return Status::UNSUPPORTED;
        }
        if length == 0 || output.is_null() || !output.is_aligned() {
            return Status::INVALID_PARAMETER;
        }
        if this.is_null() || !(this as *const Self).is_aligned() {
            return Status::INVALID_PARAMETER;
        }
        // SAFETY: An EFI caller must supply this instance's interface pointer.
        // repr(C) puts that field first; checking alignment does not authenticate
        // an arbitrary non-null pointer or make it safe to dereference.
        let adapter = unsafe { &*this.cast::<Self>() };
        match adapter.invoke(|backend| backend.get(base, length)) {
            Ok(attributes) => {
                // SAFETY: The callback contract requires a valid writable,
                // disjoint output. Failures deliberately leave it unchanged.
                unsafe { output.write(MemoryAttribute::from_bits_retain(attributes)) };
                Status::SUCCESS
            }
            Err(error) => status(error),
        }
    }

    unsafe extern "efiapi" fn set(
        this: *const MemoryAttributeProtocol,
        base: u64,
        length: u64,
        attributes: MemoryAttribute,
    ) -> Status {
        // SAFETY: Forward the EFI caller's pointer contract unchanged.
        unsafe { Self::change(this, base, length, attributes.bits(), false) }
    }

    unsafe extern "efiapi" fn clear(
        this: *const MemoryAttributeProtocol,
        base: u64,
        length: u64,
        attributes: MemoryAttribute,
    ) -> Status {
        // SAFETY: Forward the EFI caller's pointer contract unchanged.
        unsafe { Self::change(this, base, length, attributes.bits(), true) }
    }

    unsafe fn change(
        this: *const MemoryAttributeProtocol,
        base: u64,
        length: u64,
        attributes: u64,
        clear: bool,
    ) -> Status {
        if attributes == 0 || attributes & !ACCESS_MASK != 0 || length == 0 {
            return Status::INVALID_PARAMETER;
        }
        if base & (PAGE_SIZE - 1) != 0 || length & (PAGE_SIZE - 1) != 0 {
            return Status::UNSUPPORTED;
        }
        if this.is_null() || !(this as *const Self).is_aligned() {
            return Status::INVALID_PARAMETER;
        }
        // SAFETY: Same repr(C)/valid-instance contract as Get.
        let adapter = unsafe { &*this.cast::<Self>() };
        match adapter.invoke(|backend| {
            if clear {
                backend.clear(base, length, attributes)
            } else {
                backend.set(base, length, attributes)
            }
        }) {
            Ok(()) => Status::SUCCESS,
            Err(error) => status(error),
        }
    }
}

fn status(error: Error) -> Status {
    Status(Status::ERROR_BIT | error.code())
}
