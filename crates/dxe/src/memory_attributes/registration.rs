//! Explicit registration of an already qualified, resident MAP adapter.
//!
//! No driver entry path calls this module automatically. The adapter has static
//! storage because firmware can retain interface pointers after lookup. Neither
//! a failed uninstall nor dropping this bookkeeping object frees that storage.
//! Its implementing image must remain resident under the constructor contract.

use core::{ffi::c_void, marker::PhantomData, pin::Pin, ptr};
use svmvisor_memory_attributes::Attributes;
use uefi_raw::table::boot::{BootServices, InterfaceType};
use uefi_raw::{Handle, Status, protocol::memory_protection::MemoryAttributeProtocol};

use crate::memory_attributes::Adapter;

/// Raw operations on the protocol database, separate from allocation/lifetime.
///
/// # Safety
/// Implementations must conform to UEFI protocol-handler semantics, preserve the
/// supplied GUID/interface/handle, and never retain an interface after an
/// error-status install. Success must describe the actual publication/removal. The caller
/// supplies valid resident pointers and the required platform execution context.
pub unsafe trait ProtocolDatabase {
    /// # Safety
    /// The required firmware CPU, TPL and Boot Services lifetime must hold.
    unsafe fn locate(&mut self) -> (Status, *mut c_void);
    /// # Safety
    /// Interface must remain valid and resident for all possible consumers.
    unsafe fn install(&mut self, interface: *const c_void) -> (Status, Handle);
    /// # Safety
    /// Handle/interface must identify this provider; consumers must be quiescent.
    unsafe fn uninstall(&mut self, handle: Handle, interface: *const c_void) -> Status;
}

pub struct BootServicesDatabase<'fw> {
    services: &'fw BootServices,
    _not_send_sync: PhantomData<*mut ()>,
}

impl<'fw> BootServicesDatabase<'fw> {
    /// # Safety
    /// A genuine conforming Boot Services table must stay live for this borrow.
    pub unsafe fn new(services: &'fw BootServices) -> Self {
        Self { services, _not_send_sync: PhantomData }
    }
}

// SAFETY: Direct calls preserve the standard signatures and arguments. Platform
// and residency requirements are exposed at the unsafe operations below.
unsafe impl ProtocolDatabase for BootServicesDatabase<'_> {
    unsafe fn locate(&mut self) -> (Status, *mut c_void) {
        let mut interface = ptr::null_mut();
        let status = unsafe {
            (self.services.locate_protocol)(
                &MemoryAttributeProtocol::GUID,
                ptr::null_mut(),
                &mut interface,
            )
        };
        (status, interface)
    }

    unsafe fn install(&mut self, interface: *const c_void) -> (Status, Handle) {
        let mut handle = ptr::null_mut();
        let status = unsafe {
            (self.services.install_protocol_interface)(
                &mut handle,
                &MemoryAttributeProtocol::GUID,
                InterfaceType::NATIVE_INTERFACE,
                interface,
            )
        };
        (status, handle)
    }

    unsafe fn uninstall(&mut self, handle: Handle, interface: *const c_void) -> Status {
        unsafe {
            (self.services.uninstall_protocol_interface)(
                handle,
                &MemoryAttributeProtocol::GUID,
                interface,
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationState {
    Unpublished,
    Published,
    /// Firmware reported successful installation without a usable handle.
    /// Publication cannot be safely reversed using an invented handle.
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    AlreadyPublished,
    ExistingProvider,
    MalformedExistingProvider,
    Lookup(Status),
    Install(Status),
    InvalidInstalledHandle,
    Uninstall(Status),
    Indeterminate,
}

/// Bookkeeping over permanently retained adapter storage. No implicit cleanup.
///
/// Failed removal keeps the same handle, interface and Published state, enabling
/// explicit retry. Forgetting/dropping this value cannot reclaim the adapter.
/// A successfully removed interface may still have cached consumers; storage
/// intentionally remains resident and this API offers no reclamation operation.
pub struct ResidentRegistration<A: Attributes + Send + 'static, D: ProtocolDatabase> {
    adapter: Pin<&'static Adapter<A>>,
    database: D,
    state: RegistrationState,
    handle: Handle,
    _not_send_sync: PhantomData<*mut ()>,
}

impl<A: Attributes + Send + 'static, D: ProtocolDatabase> ResidentRegistration<A, D> {
    /// # Safety
    /// The adapter's backend must already be qualified for all permitted EFI
    /// callers. Its code image, allocation, backend dependencies and callback
    /// state must remain resident for every possible consumer, including failed
    /// removal and cached pointers. A returning/unloadable child image does not
    /// meet this contract merely because its data was leaked to static storage.
    pub unsafe fn new(adapter: Pin<&'static Adapter<A>>, database: D) -> Self {
        Self {
            adapter,
            database,
            state: RegistrationState::Unpublished,
            handle: ptr::null_mut(),
            _not_send_sync: PhantomData,
        }
    }

    pub fn state(&self) -> RegistrationState {
        self.state
    }

    pub fn interface(&self) -> *const MemoryAttributeProtocol {
        self.adapter.protocol_ptr()
    }

    /// Publish only when one exact lookup reports NOT_FOUND. Existing providers,
    /// malformed success results, warnings and all other errors refuse.
    ///
    /// # Safety
    /// Run on the permitted firmware CPU at TPL <= NOTIFY with Boot Services
    /// live. Serialize publication against other installers (the locate/install
    /// pair is not atomic). All constructor residency requirements remain true.
    pub unsafe fn install(&mut self) -> Result<(), RegistrationError> {
        match self.state {
            RegistrationState::Published => return Err(RegistrationError::AlreadyPublished),
            RegistrationState::Indeterminate => return Err(RegistrationError::Indeterminate),
            RegistrationState::Unpublished => {}
        }
        let (status, existing) = unsafe { self.database.locate() };
        if status == Status::SUCCESS {
            return Err(
                if existing.is_null() || !existing.cast::<MemoryAttributeProtocol>().is_aligned() {
                    RegistrationError::MalformedExistingProvider
                } else {
                    RegistrationError::ExistingProvider
                },
            );
        }
        if status != Status::NOT_FOUND {
            return Err(RegistrationError::Lookup(status));
        }
        let interface = self.interface().cast();
        let (status, handle) = unsafe { self.database.install(interface) };
        if status != Status::SUCCESS {
            // A warning is a non-error result; conservatively retain possible
            // publication without retrying and creating a second interface.
            if !status.is_error() {
                self.handle = handle;
                self.state = RegistrationState::Indeterminate;
            }
            return Err(RegistrationError::Install(status));
        }
        if handle.is_null() {
            self.state = RegistrationState::Indeterminate;
            return Err(RegistrationError::InvalidInstalledHandle);
        }
        self.handle = handle;
        self.state = RegistrationState::Published;
        Ok(())
    }

    /// Explicit removal. This never frees the provider or permits image unload.
    ///
    /// # Safety
    /// Correct CPU/TPL/lifetime must hold. Establish consumer quiescence before
    /// calling and retain the resident image even if firmware reports failure.
    pub unsafe fn uninstall(&mut self) -> Result<(), RegistrationError> {
        match self.state {
            RegistrationState::Unpublished => return Ok(()),
            RegistrationState::Indeterminate => return Err(RegistrationError::Indeterminate),
            RegistrationState::Published => {}
        }
        let interface = self.interface().cast();
        let status = unsafe { self.database.uninstall(self.handle, interface) };
        if status != Status::SUCCESS {
            if !status.is_error() {
                self.state = RegistrationState::Indeterminate;
            }
            return Err(RegistrationError::Uninstall(status));
        }
        self.handle = ptr::null_mut();
        self.state = RegistrationState::Unpublished;
        Ok(())
    }
}
