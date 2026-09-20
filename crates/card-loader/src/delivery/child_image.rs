//! Digest-bound PE child delivery through normal UEFI image services.
//! This module does not relocate PE bytes, change permissions, or admit SVM.

use core::{ptr, slice};

use sha2::{Digest, Sha256};
use svmvisor_card_abi::{
    boot_options::ResidentBootOptions,
    envelope::{Envelope, EnvelopeError, HEADER_BYTES, ImageKind, PeMetadata, parse_pe_kind},
    native_result::NativeResult,
};
use uefi_raw::{
    Handle, Status,
    protocol::{device_path::DevicePathProtocol, loaded_image::LoadedImageProtocol},
    table::boot::{BootServices, MemoryType},
};

const MAX_PATH: usize = 4096;
const PATH_TRAILER: usize = 56;

const _: () = assert!(PATH_TRAILER <= 64);

#[derive(Clone, Copy)]
pub struct Pin {
    header: [u8; HEADER_BYTES],
    kind: ImageKind,
    pub payload_bytes: usize,
    pub metadata: PeMetadata,
    digest: [u8; 32],
}

impl Pin {
    pub fn parse_resident(header: &[u8]) -> Result<Self, Status> {
        Self::parse_kind(header, ImageKind::ResidentBoot)
    }

    fn parse_kind(header: &[u8], kind: ImageKind) -> Result<Self, Status> {
        let envelope = Envelope::parse(header, kind).map_err(envelope_status)?;
        let mut saved = [0; HEADER_BYTES];
        for (d, s) in saved.iter_mut().zip(header) {
            *d = *s;
        }
        Ok(Self {
            header: saved,
            kind,
            payload_bytes: envelope.payload_bytes,
            metadata: envelope.metadata,
            digest: envelope.digest,
        })
    }

    pub fn verify(&self, pe: &[u8]) -> Result<(), Status> {
        if pe.len() != self.payload_bytes || Sha256::digest(pe).as_slice() != self.digest {
            return Err(bad());
        }
        if parse_pe_kind(pe, self.kind).map_err(envelope_status)? != self.metadata {
            return Err(bad());
        }
        Ok(())
    }
}

pub struct State {
    pool: *mut u8,
    child: Handle,
    exit_data: *mut u8,
    retained: bool,
    resident_options: Option<ResidentBootOptions>,
}

impl State {
    pub const fn new() -> Self {
        Self {
            pool: ptr::null_mut(),
            child: ptr::null_mut(),
            exit_data: ptr::null_mut(),
            retained: false,
            resident_options: None,
        }
    }

    pub fn resident_options(&self) -> Option<ResidentBootOptions> {
        self.resident_options
    }

    pub fn is_retained(&self) -> bool {
        self.retained
    }

    pub fn is_clean(&self) -> bool {
        self.pool.is_null() && self.child.is_null() && self.exit_data.is_null()
    }

    /// Retry only retained ownership, before closing the controller or unloading
    /// the parent. Failed child unload keeps its LoadOptions pool alive.
    /// # Safety
    /// Boot services remain live; serialize this state across firmware callbacks.
    pub unsafe fn cleanup(&mut self, boot_services: &BootServices) -> Result<(), Status> {
        // SUCCESS can leave callbacks/EBS hooks into the runtime child, even if
        // its acknowledgement is malformed. Never unload it or its inputs.
        if self.retained {
            return Err(Status::UNSUPPORTED);
        }
        if !self.child.is_null() {
            let status = unsafe { (boot_services.unload_image)(self.child) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.child = ptr::null_mut();
        }
        if !self.exit_data.is_null() {
            let status = unsafe { (boot_services.free_pool)(self.exit_data) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.exit_data = ptr::null_mut();
        }
        if !self.pool.is_null() {
            let status = unsafe { (boot_services.free_pool)(self.pool) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            self.pool = ptr::null_mut();
        }
        Ok(())
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Delivery {
    /// 0 initial, 1 header, 2 copied+verified, 3 LoadImage, 4 StartImage returned.
    pub stage: u32,
    pub load_status: Option<Status>,
    pub start_status: Option<Status>,
    pub inner: NativeResult,
    pub operation_status: Status,
    pub cleanup_status: Status,
}

impl Delivery {
    pub fn status(&self) -> Status {
        if self.cleanup_status != Status::SUCCESS {
            self.cleanup_status
        } else {
            self.operation_status
        }
    }
}

/// The parent must serialize invocation, own its controller and keep memory
/// decoding enabled. The numeric journal range
/// was obtained and checked through the owned controller's BAR0 descriptor.
/// Any child SUCCESS permanently retains image/input/controller ownership;
/// report SUCCESS additionally requires its explicit armed acknowledgement.
/// # Safety
/// All handles/protocols belong to this live firmware, TPL_APPLICATION, BSP.
/// No reference to `state` may be formed by reentrant callbacks during this call.
/// Serialize state, and keep memory decoding through reset after
/// is_retained becomes true. No cleanup/unload may revoke the installed hook.
pub unsafe fn execute_resident(
    state: &mut State,
    boot_services: &BootServices,
    parent: Handle,
    controller: Handle,
    pin: &Pin,
    options: ResidentBootOptions,
    read: impl FnMut(u64) -> Result<u32, Status>,
) -> Delivery {
    unsafe { execute_inner(state, boot_services, parent, controller, pin, Some(options), read) }
}

/// Development delivery (`card-resident-dev-loader`): no header is compiled
/// into the parent. The 128 bytes at the start of the slot become the pin after
/// the unchanged `Pin::parse_resident` structural policy accepts them; the
/// shared path then re-reads the header, requires it to be identical, and binds
/// the child to its SHA-256 and PE metadata exactly like the pinned parent.
/// What is given up: the ROM no longer names one exact payload.
/// # Safety
/// See execute_resident.
#[cfg(feature = "card-resident-dev-loader")]
pub unsafe fn execute_resident_dev(
    state: &mut State,
    boot_services: &BootServices,
    parent: Handle,
    controller: Handle,
    options: ResidentBootOptions,
    mut read: impl FnMut(u64) -> Result<u32, Status>,
) -> Delivery {
    let pin = if state.is_clean() {
        read_header(&mut read).and_then(|header| Pin::parse_resident(&header))
    } else {
        Err(Status::NOT_READY)
    };
    match pin {
        Ok(pin) => unsafe {
            execute_inner(state, boot_services, parent, controller, &pin, Some(options), read)
        },
        // Same stage-0 report a pinned parent gives for a header mismatch.
        Err(error) => Delivery {
            stage: 0,
            load_status: None,
            start_status: None,
            inner: NativeResult::new(),
            operation_status: error,
            cleanup_status: Status::SUCCESS,
        },
    }
}

unsafe fn execute_inner(
    state: &mut State,
    boot_services: &BootServices,
    parent: Handle,
    controller: Handle,
    pin: &Pin,
    options: Option<ResidentBootOptions>,
    mut read: impl FnMut(u64) -> Result<u32, Status>,
) -> Delivery {
    let mut report = Delivery {
        stage: 0,
        load_status: None,
        start_status: None,
        inner: NativeResult::new(),
        operation_status: Status::SUCCESS,
        cleanup_status: Status::SUCCESS,
    };
    if !state.is_clean() {
        report.operation_status = Status::NOT_READY;
        return report;
    }
    let operation = (|| -> Result<(), Status> {
        if (pin.kind == ImageKind::ResidentBoot) != options.is_some()
            || options.is_some_and(|o| {
                !o.is_valid_header() || o.rust_entered != 0 || o.armed != 0 || o.failure != 0
            })
        {
            return Err(Status::INVALID_PARAMETER);
        }
        if parent.is_null() || controller.is_null() {
            return Err(Status::INVALID_PARAMETER);
        }
        let header = read_header(&mut read)?;
        if header != pin.header {
            return Err(bad());
        }
        report.stage = 1;
        let rounded = (pin.payload_bytes + 7) & !7;
        let result_offset = rounded + MAX_PATH + 64;
        let total = result_offset + core::mem::size_of::<NativeResult>();
        status(unsafe {
            (boot_services.allocate_pool)(
                if options.is_some() {
                    MemoryType::RUNTIME_SERVICES_DATA
                } else {
                    MemoryType::LOADER_DATA
                },
                total,
                &mut state.pool,
            )
        })?;
        if state.pool.is_null() {
            return Err(Status::DEVICE_ERROR);
        }
        // The pool owns all rounded reads, the bounded copied device path, and
        // the aligned result. No external byte pointer is executed directly.
        let buffer = unsafe { slice::from_raw_parts_mut(state.pool, (pin.payload_bytes + 3) & !3) };
        for (i, chunk) in buffer.chunks_exact_mut(4).enumerate() {
            for (d, s) in chunk.iter_mut().zip(read((HEADER_BYTES + i * 4) as u64)?.to_le_bytes()) {
                *d = s;
            }
        }
        let pe = buffer.get(..pin.payload_bytes).ok_or_else(bad)?;
        pin.verify(pe)?;
        report.stage = 2;
        let path = unsafe { state.pool.add(rounded) };
        unsafe { copy_path(boot_services, parent, controller, path, &pin.digest, pin.kind) }?;
        let mailbox = unsafe { state.pool.add(result_offset).cast::<NativeResult>() };
        unsafe {
            if let Some(options) = options {
                mailbox.cast::<ResidentBootOptions>().write(options);
            } else {
                mailbox.write(NativeResult::new());
            }
        };
        let loaded = unsafe {
            (boot_services.load_image)(
                false.into(),
                parent,
                path.cast(),
                pe.as_ptr(),
                pe.len(),
                &mut state.child,
            )
        };
        report.load_status = Some(loaded);
        report.stage = 3;
        // SECURITY_VIOLATION explicitly may return a live loaded handle. Cleanup
        // unloads it and never calls StartImage when LoadImage did not succeed.
        status(loaded)?;
        if state.child.is_null() {
            return Err(Status::DEVICE_ERROR);
        }
        unsafe {
            set_options(
                boot_services,
                parent,
                state.child,
                mailbox,
                pin.metadata.image_bytes,
                pin.kind,
            )
        }?;
        let mut exit_size = 0;
        let mut exit_data = ptr::null_mut();
        let started =
            unsafe { (boot_services.start_image)(state.child, &mut exit_size, &mut exit_data) };
        state.exit_data = exit_data.cast();
        report.start_status = Some(started);
        report.stage = 4;
        if let Some(original) = options {
            let observed = unsafe { mailbox.cast::<ResidentBootOptions>().read() };
            state.resident_options = Some(observed);
            if !started.is_error() {
                state.retained = true;
                // The pool and any ExitData remain retained on this path; no
                // fallible cleanup can revoke a possibly installed EBS hook.
                if started != Status::SUCCESS
                    || !observed.is_armed()
                    || observed.version != original.version
                    || observed.reserved != original.reserved
                    || observed.journal_base != original.journal_base
                    || observed.boot_id != original.boot_id
                {
                    return Err(Status::PROTOCOL_ERROR);
                }
                return Ok(());
            }
            // The exact child sets entered before its own errors, distinguishing
            // firmware pre-entry denial from auto-unloaded driver errors.
            if started.is_error()
                && !(observed.rust_entered == 0
                    && matches!(started, Status::SECURITY_VIOLATION | Status::INVALID_PARAMETER))
            {
                state.child = ptr::null_mut();
            }
            return Err(started);
        }
        report.inner = unsafe { mailbox.read() };
        // Drivers returning an error are automatically unloaded by firmware.
        // The two documented pre-entry failures leave the fresh image loaded.
        // The pinned child never returns either code on an ordinary path.
        if started.is_error()
            && !(report.inner.rust_entered == 0
                && (started == Status::SECURITY_VIOLATION || started == Status::INVALID_PARAMETER))
        {
            state.child = ptr::null_mut();
        }
        if !report.inner.is_valid_header() {
            return Err(bad());
        }
        if started != Status::UNSUPPORTED {
            return Err(if started == Status::SUCCESS { Status::PROTOCOL_ERROR } else { started });
        }
        // Unsupported with no Rust marker is an ordinary assembly refusal, not
        // evidence that the child performed a probe or completed restoration.
        Ok(())
    })();
    if let Err(error) = operation {
        report.operation_status = error;
    }
    if !state.retained {
        if let Err(error) = unsafe { state.cleanup(boot_services) } {
            report.cleanup_status = error;
        }
    }
    report
}

fn read_header(
    read: &mut impl FnMut(u64) -> Result<u32, Status>,
) -> Result<[u8; HEADER_BYTES], Status> {
    let mut header = [0u8; HEADER_BYTES];
    for (i, chunk) in header.chunks_exact_mut(4).enumerate() {
        for (d, s) in chunk.iter_mut().zip(read((i * 4) as u64)?.to_le_bytes()) {
            *d = s;
        }
    }
    Ok(header)
}

unsafe fn set_options(
    boot_services: &BootServices,
    parent: Handle,
    child: Handle,
    result: *mut NativeResult,
    image_bytes: u32,
    kind: ImageKind,
) -> Result<(), Status> {
    let mut raw = ptr::null_mut();
    status(unsafe {
        (boot_services.open_protocol)(
            child,
            &LoadedImageProtocol::GUID,
            &mut raw,
            parent,
            ptr::null_mut(),
            2,
        )
    })?;
    let operation = (|| {
        let loaded =
            unsafe { raw.cast::<LoadedImageProtocol>().as_mut() }.ok_or(Status::DEVICE_ERROR)?;
        if loaded.parent_handle != parent
            || loaded.image_base.is_null()
            || loaded.image_size != u64::from(image_bytes)
            || loaded.image_code_type
                != if kind == ImageKind::Returning {
                    MemoryType::BOOT_SERVICES_CODE
                } else {
                    MemoryType::RUNTIME_SERVICES_CODE
                }
            || loaded.image_data_type
                != if kind == ImageKind::Returning {
                    MemoryType::BOOT_SERVICES_DATA
                } else {
                    MemoryType::RUNTIME_SERVICES_DATA
                }
            || loaded.load_options_size != 0
            || !loaded.load_options.is_null()
        {
            return Err(bad());
        }
        loaded.load_options_size = core::mem::size_of::<NativeResult>() as u32;
        loaded.load_options = result.cast();
        Ok(())
    })();
    // UEFI 2.11 §7.3.9: GET_PROTOCOL needs no CloseProtocol. The resident
    // policy retains no interface borrow; preserve the historical returning path.
    if kind == ImageKind::ResidentBoot {
        return operation;
    }
    let closed = status(unsafe {
        (boot_services.close_protocol)(child, &LoadedImageProtocol::GUID, parent, ptr::null_mut())
    });
    closed.and(operation)
}

unsafe fn copy_path(
    boot_services: &BootServices,
    parent: Handle,
    controller: Handle,
    dest: *mut u8,
    digest: &[u8; 32],
    kind: ImageKind,
) -> Result<(), Status> {
    let mut raw = ptr::null_mut();
    status(unsafe {
        (boot_services.open_protocol)(
            controller,
            &DevicePathProtocol::GUID,
            &mut raw,
            parent,
            ptr::null_mut(),
            2,
        )
    })?;
    let operation = (|| {
        if raw.is_null() {
            return Err(Status::DEVICE_ERROR);
        }
        let source = raw.cast::<u8>();
        let mut offset = 0usize;
        // Firmware's live DevicePathProtocol contract supplies readable nodes
        // through End Entire. Reject multi-instance and oversized paths.
        loop {
            if offset > MAX_PATH - 4 {
                return Err(bad());
            }
            let node = unsafe { slice::from_raw_parts(source.add(offset), 4) };
            let length = usize::from(read_u16(node, 2)?);
            if length < 4 || length > MAX_PATH - offset {
                return Err(bad());
            }
            if node.first() == Some(&0x7f) {
                if node.get(1) != Some(&0xff) || length != 4 {
                    return Err(bad());
                }
                break;
            }
            unsafe { ptr::copy_nonoverlapping(source.add(offset), dest.add(offset), length) };
            offset += length;
        }
        // Vendor media node names this exact payload, following the real card's
        // device path for firmware security-policy classification.
        let prefix = [
            4, 3, 52, 0, 0x48, 0x97, 0xd8, 0x60, 0xa2, 0x3e, 0x10, 0x4a, 0x9f, 0xf1, 0x3c, 0xc9,
            0x85, 0xfb, 0x85, 0x2e,
        ];
        unsafe {
            ptr::copy_nonoverlapping(prefix.as_ptr(), dest.add(offset), 20);
            ptr::copy_nonoverlapping(digest.as_ptr(), dest.add(offset + 20), 32);
            ptr::copy_nonoverlapping([0x7f, 0xff, 4, 0].as_ptr(), dest.add(offset + 52), 4);
        }
        Ok(())
    })();
    // UEFI 2.11 §7.3.9: GET_PROTOCOL needs no CloseProtocol. The resident
    // policy retains no interface borrow; preserve the historical returning path.
    if kind == ImageKind::ResidentBoot {
        return operation;
    }
    let closed = status(unsafe {
        (boot_services.close_protocol)(
            controller,
            &DevicePathProtocol::GUID,
            parent,
            ptr::null_mut(),
        )
    });
    closed.and(operation)
}

fn status(s: Status) -> Result<(), Status> {
    if s == Status::SUCCESS { Ok(()) } else { Err(s) }
}

fn read_u16(b: &[u8], o: usize) -> Result<u16, Status> {
    let Some(&[a, b]) = b.get(o..o.checked_add(2).ok_or_else(bad)?) else {
        return Err(bad());
    };
    Ok(u16::from_le_bytes([a, b]))
}

/// Whatever is wrong with an envelope or its PE, firmware sees the one status.
fn envelope_status(_: EnvelopeError) -> Status {
    bad()
}

fn bad() -> Status {
    Status::COMPROMISED_DATA
}
