#![cfg(feature = "memory-attribute-provider")]

use std::{
    ffi::c_void,
    pin::Pin,
    sync::{Arc, Mutex},
};
use svmvisor_dxe::{memory_attribute_registration::*, memory_attributes::Adapter};
use svmvisor_memory_attributes::{Attributes, Error};
use uefi_raw::{Handle, Status};

struct Backend;
impl Attributes for Backend {
    fn get(&mut self, _: u64, _: u64) -> Result<u64, Error> {
        Ok(0)
    }
    fn set(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
        Ok(())
    }
    fn clear(&mut self, _: u64, _: u64, _: u64) -> Result<(), Error> {
        Ok(())
    }
}

struct State {
    lookup: Status,
    existing: usize,
    install: Status,
    handle: usize,
    remove: Status,
    calls: Vec<(&'static str, usize, usize)>,
    exposed: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            lookup: Status::NOT_FOUND,
            existing: 0,
            install: Status::SUCCESS,
            handle: 0x1000,
            remove: Status::SUCCESS,
            calls: Vec::new(),
            exposed: 0,
        }
    }
}
struct Database(Arc<Mutex<State>>);
// SAFETY: A supplied-data model records publication/removal without invoking EFI.
unsafe impl ProtocolDatabase for Database {
    unsafe fn locate(&mut self) -> (Status, *mut c_void) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(("locate", 0, 0));
        (
            state.lookup,
            std::ptr::without_provenance_mut(state.existing),
        )
    }
    unsafe fn install(&mut self, interface: *const c_void) -> (Status, Handle) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(("install", 0, interface.addr()));
        if state.install == Status::SUCCESS {
            state.exposed = interface.addr();
        }
        (
            state.install,
            std::ptr::without_provenance_mut(state.handle),
        )
    }
    unsafe fn uninstall(&mut self, handle: Handle, interface: *const c_void) -> Status {
        let mut state = self.0.lock().unwrap();
        state
            .calls
            .push(("uninstall", handle.addr(), interface.addr()));
        assert_eq!(state.exposed, interface.addr());
        if state.remove == Status::SUCCESS {
            state.exposed = 0;
        }
        state.remove
    }
}

fn setup(state: State) -> (ResidentRegistration<Backend, Database>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(state));
    let adapter = Box::leak(Box::new(Adapter::new(Backend)));
    // SAFETY: Intentionally resident test allocation and process-lifetime code;
    // no native invocation. The adapter will never move or be reclaimed.
    let pinned = unsafe { Pin::new_unchecked(&*adapter) };
    let registration = unsafe { ResidentRegistration::new(pinned, Database(state.clone())) };
    (registration, state)
}

#[test]
fn publishes_exact_interface_and_uninstalls_exact_pair() {
    let (mut registration, state) = setup(State::default());
    let expected = registration.interface().addr();
    unsafe {
        registration.install().unwrap();
    }
    assert_eq!(registration.state(), RegistrationState::Published);
    unsafe {
        assert_eq!(
            registration.install(),
            Err(RegistrationError::AlreadyPublished)
        );
    }
    unsafe {
        registration.uninstall().unwrap();
        registration.uninstall().unwrap();
    }
    assert_eq!(registration.state(), RegistrationState::Unpublished);
    assert_eq!(
        state.lock().unwrap().calls,
        vec![
            ("locate", 0, 0),
            ("install", 0, expected),
            ("uninstall", 0x1000, expected)
        ]
    );
}

#[test]
fn existing_or_malformed_provider_is_never_replaced() {
    for (existing, error) in [
        (0x1000, RegistrationError::ExistingProvider),
        (0, RegistrationError::MalformedExistingProvider),
        (1, RegistrationError::MalformedExistingProvider),
    ] {
        let (mut registration, state) = setup(State {
            lookup: Status::SUCCESS,
            existing,
            ..State::default()
        });
        unsafe {
            assert_eq!(registration.install(), Err(error));
        }
        assert_eq!(registration.state(), RegistrationState::Unpublished);
        assert_eq!(state.lock().unwrap().calls, vec![("locate", 0, 0)]);
    }
}

#[test]
fn lookup_error_and_warning_never_trigger_installation() {
    for status in [
        Status::ACCESS_DENIED,
        Status::DEVICE_ERROR,
        Status::WARN_UNKNOWN_GLYPH,
    ] {
        let (mut registration, state) = setup(State {
            lookup: status,
            existing: 1,
            ..State::default()
        });
        unsafe {
            assert_eq!(
                registration.install(),
                Err(RegistrationError::Lookup(status))
            );
        }
        assert_eq!(state.lock().unwrap().calls, vec![("locate", 0, 0)]);
    }
}

#[test]
fn install_failure_preserves_resident_storage_and_allows_retry() {
    let (mut registration, state) = setup(State {
        install: Status::OUT_OF_RESOURCES,
        ..State::default()
    });
    let pointer = registration.interface();
    unsafe {
        assert_eq!(
            registration.install(),
            Err(RegistrationError::Install(Status::OUT_OF_RESOURCES))
        );
    }
    assert_eq!(registration.state(), RegistrationState::Unpublished);
    state.lock().unwrap().install = Status::SUCCESS;
    unsafe {
        registration.install().unwrap();
    }
    assert_eq!(registration.interface(), pointer);
    unsafe {
        registration.uninstall().unwrap();
    }
}

#[test]
fn removal_failure_retains_handle_interface_and_publication_for_retry() {
    let (mut registration, state) = setup(State {
        remove: Status::ACCESS_DENIED,
        ..State::default()
    });
    unsafe {
        registration.install().unwrap();
    }
    unsafe {
        assert_eq!(
            registration.uninstall(),
            Err(RegistrationError::Uninstall(Status::ACCESS_DENIED))
        );
    }
    assert_eq!(registration.state(), RegistrationState::Published);
    assert_eq!(
        state.lock().unwrap().exposed,
        registration.interface().addr()
    );
    state.lock().unwrap().remove = Status::SUCCESS;
    unsafe {
        registration.uninstall().unwrap();
    }
    let state = state.lock().unwrap();
    assert_eq!(state.calls[2], state.calls[3]);
    assert_eq!(state.exposed, 0);
}

#[test]
fn malformed_success_is_retained_as_indeterminate_without_invented_cleanup() {
    let (mut registration, state) = setup(State {
        handle: 0,
        ..State::default()
    });
    unsafe {
        assert_eq!(
            registration.install(),
            Err(RegistrationError::InvalidInstalledHandle)
        );
    }
    assert_eq!(registration.state(), RegistrationState::Indeterminate);
    unsafe {
        assert_eq!(
            registration.install(),
            Err(RegistrationError::Indeterminate)
        );
        assert_eq!(
            registration.uninstall(),
            Err(RegistrationError::Indeterminate)
        );
    }
    drop(registration);
    let state = state.lock().unwrap();
    assert_ne!(state.exposed, 0);
    assert_eq!(state.calls.len(), 2);
}

#[test]
fn unexpected_install_warning_never_allows_retry_or_assumes_nonpublication() {
    let (mut registration, state) = setup(State {
        install: Status::WARN_UNKNOWN_GLYPH,
        ..State::default()
    });
    unsafe {
        assert_eq!(
            registration.install(),
            Err(RegistrationError::Install(Status::WARN_UNKNOWN_GLYPH))
        );
        assert_eq!(
            registration.install(),
            Err(RegistrationError::Indeterminate)
        );
        assert_eq!(
            registration.uninstall(),
            Err(RegistrationError::Indeterminate)
        );
    }
    assert_eq!(registration.state(), RegistrationState::Indeterminate);
    assert_eq!(state.lock().unwrap().calls.len(), 2);
}

#[test]
fn unexpected_removal_warning_does_not_reuse_a_possibly_removed_handle() {
    let (mut registration, state) = setup(State {
        remove: Status::WARN_UNKNOWN_GLYPH,
        ..State::default()
    });
    unsafe {
        registration.install().unwrap();
        assert_eq!(
            registration.uninstall(),
            Err(RegistrationError::Uninstall(Status::WARN_UNKNOWN_GLYPH))
        );
        assert_eq!(
            registration.uninstall(),
            Err(RegistrationError::Indeterminate)
        );
        assert_eq!(
            registration.install(),
            Err(RegistrationError::Indeterminate)
        );
    }
    assert_eq!(registration.state(), RegistrationState::Indeterminate);
    assert_eq!(state.lock().unwrap().calls.len(), 3);
}

#[test]
fn dropping_bookkeeping_does_not_free_a_published_adapter() {
    let (mut registration, state) = setup(State::default());
    unsafe {
        registration.install().unwrap();
    }
    let pointer = registration.interface();
    drop(registration);
    assert_eq!(state.lock().unwrap().exposed, pointer.addr());
    let mut attributes = uefi_raw::table::boot::MemoryAttribute::RUNTIME;
    // SAFETY: The intentionally resident adapter and code are still live.
    unsafe {
        assert_eq!(
            ((*pointer).get_memory_attributes)(pointer, 0, 4096, &mut attributes),
            Status::SUCCESS
        );
    }
    assert_eq!(attributes.bits(), 0);
}
