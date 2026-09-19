//! Digest-bound PE child delivery through normal UEFI image services.
//! This module does not relocate PE bytes, change permissions, or admit SVM.

use core::{ptr, slice};

use sha2::{Digest, Sha256};
use svmvisor_card_abi::{
    boot_options::ResidentBootOptions,
    envelope::{
        DIGEST_BYTES, DIGEST_OFFSET, ENTRY_RVA_OFFSET, FILE_ALIGNMENT, FILE_ALIGNMENT_OFFSET,
        FLAGS_OFFSET, FLAGS_RESIDENT_BOOT, FLAGS_RETURNING, HEADER_BYTES, HEADER_BYTES_OFFSET,
        HEADERS_BYTES_OFFSET, IMAGE_BYTES_OFFSET, MACHINE_AMD64, MACHINE_OFFSET, MAX_IMAGE_BYTES,
        MAX_SECTIONS, MIN_PE_BYTES, OPTIONAL_MAGIC_OFFSET, OPTIONAL_MAGIC_PE32_PLUS,
        PAYLOAD_BYTES_OFFSET, PAYLOAD_OFFSET_OFFSET, RESERVED_OFFSET, RESERVED_WORD_OFFSET,
        RESIDENT_BOOT_MAGIC, RETURNING_MAGIC, SECTION_ALIGNMENT, SECTION_ALIGNMENT_OFFSET,
        SECTIONS_OFFSET, SLOT_BYTES, SLOT_BYTES_OFFSET, SUBSYSTEM_BOOT_SERVICE_DRIVER,
        SUBSYSTEM_OFFSET, SUBSYSTEM_RUNTIME_DRIVER, VERSION, VERSION_OFFSET,
    },
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
    /// All 128 bytes are compiled into the parent. No card-supplied metadata is
    /// trusted before exact comparison with this immutable build input.
    pub fn parse(header: &[u8]) -> Result<Self, Status> {
        Self::parse_kind(header, ImageKind::Returning)
    }

    pub fn parse_resident(header: &[u8]) -> Result<Self, Status> {
        Self::parse_kind(header, ImageKind::ResidentBoot)
    }

    fn parse_kind(header: &[u8], kind: ImageKind) -> Result<Self, Status> {
        if header.len() != HEADER_BYTES
            || header.get(..8) != Some(kind.magic())
            || read_u32(header, VERSION_OFFSET)? != VERSION
            || read_u32(header, HEADER_BYTES_OFFSET)? != HEADER_BYTES as u32
            || read_u64(header, SLOT_BYTES_OFFSET)? != SLOT_BYTES as u64
            || read_u64(header, PAYLOAD_OFFSET_OFFSET)? != HEADER_BYTES as u64
            || read_u64(header, FLAGS_OFFSET)? != kind.flags()
            || read_u16(header, MACHINE_OFFSET)? != MACHINE_AMD64
            || read_u16(header, SUBSYSTEM_OFFSET)? != kind.subsystem()
            || read_u16(header, OPTIONAL_MAGIC_OFFSET)? != OPTIONAL_MAGIC_PE32_PLUS
            || read_u16(header, RESERVED_WORD_OFFSET)? != 0
            || header.get(RESERVED_OFFSET..).ok_or_else(bad)?.iter().any(|b| *b != 0)
        {
            return Err(bad());
        }
        let bytes = read_u64(header, PAYLOAD_BYTES_OFFSET)?;
        if !(MIN_PE_BYTES as u64..=(SLOT_BYTES - HEADER_BYTES) as u64).contains(&bytes) {
            return Err(bad());
        }
        let mut saved = [0; HEADER_BYTES];
        for (d, s) in saved.iter_mut().zip(header) {
            *d = *s;
        }
        let mut digest = [0; DIGEST_BYTES];
        for (d, s) in digest
            .iter_mut()
            .zip(header.get(DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES).ok_or_else(bad)?)
        {
            *d = *s;
        }
        let metadata = PeMetadata {
            entry_rva: read_u32(header, ENTRY_RVA_OFFSET)?,
            image_bytes: read_u32(header, IMAGE_BYTES_OFFSET)?,
            headers_bytes: read_u32(header, HEADERS_BYTES_OFFSET)?,
            section_alignment: read_u32(header, SECTION_ALIGNMENT_OFFSET)?,
            file_alignment: read_u32(header, FILE_ALIGNMENT_OFFSET)?,
            sections: read_u32(header, SECTIONS_OFFSET)?,
        };
        metadata.validate(bytes as usize)?;
        Ok(Self { header: saved, kind, payload_bytes: bytes as usize, metadata, digest })
    }

    pub fn verify(&self, pe: &[u8]) -> Result<(), Status> {
        if pe.len() != self.payload_bytes || Sha256::digest(pe).as_slice() != self.digest {
            return Err(bad());
        }
        if parse_pe_kind(pe, self.kind)? != self.metadata {
            return Err(bad());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeMetadata {
    pub entry_rva: u32,
    pub image_bytes: u32,
    pub headers_bytes: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub sections: u32,
}

impl PeMetadata {
    fn validate(&self, bytes: usize) -> Result<(), Status> {
        if self.section_alignment != SECTION_ALIGNMENT
            || self.file_alignment != FILE_ALIGNMENT
            || self.image_bytes == 0
            || self.image_bytes > MAX_IMAGE_BYTES
            || self.image_bytes & (SECTION_ALIGNMENT - 1) != 0
            || self.headers_bytes == 0
            || self.headers_bytes as usize > bytes
            || self.headers_bytes & (FILE_ALIGNMENT - 1) != 0
            || self.headers_bytes > self.image_bytes
            || self.entry_rva < self.headers_bytes
            || self.entry_rva >= self.image_bytes
            || self.sections == 0
            || self.sections > MAX_SECTIONS
        {
            return Err(bad());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Returning,
    ResidentBoot,
}

impl ImageKind {
    fn subsystem(self) -> u16 {
        if self == Self::Returning {
            SUBSYSTEM_BOOT_SERVICE_DRIVER
        } else {
            SUBSYSTEM_RUNTIME_DRIVER
        }
    }

    fn magic(self) -> &'static [u8; 8] {
        if self == Self::Returning { &RETURNING_MAGIC } else { &RESIDENT_BOOT_MAGIC }
    }

    fn flags(self) -> u64 {
        if self == Self::Returning { FLAGS_RETURNING } else { FLAGS_RESIDENT_BOOT }
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

/// Deliberately narrow AMD64 PE32+ policy. Firmware performs final PE/COFF and
/// security validation; digest binding covers every file byte including overlays.
pub fn parse_pe(pe: &[u8]) -> Result<PeMetadata, Status> {
    parse_pe_kind(pe, ImageKind::Returning)
}

/// The parent must serialize invocation, own its controller and keep memory
/// decoding enabled. Native child contract: normal inner completion/refusal and
/// early assembly refusal return EFI_UNSUPPORTED. It installs no persistent
/// interfaces. The mailbox markers only describe Rust inner execution.
/// # Safety
/// All handles/protocols belong to this live firmware, TPL_APPLICATION, BSP.
/// No reference to `state` may be formed by reentrant callbacks during this call.
pub unsafe fn execute(
    state: &mut State,
    boot_services: &BootServices,
    parent: Handle,
    controller: Handle,
    pin: &Pin,
    read: impl FnMut(u64) -> Result<u32, Status>,
) -> Delivery {
    unsafe { execute_inner(state, boot_services, parent, controller, pin, None, read) }
}

/// Same firmware ownership requirements as execute. The numeric journal range
/// was obtained and checked through the owned controller's BAR0 descriptor.
/// Any child SUCCESS permanently retains image/input/controller ownership;
/// report SUCCESS additionally requires its explicit armed acknowledgement.
/// # Safety
/// See execute; serialize state, and keep memory decoding through reset after
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

fn parse_pe_kind(pe: &[u8], kind: ImageKind) -> Result<PeMetadata, Status> {
    if pe.len() < MIN_PE_BYTES || pe.len() > SLOT_BYTES - HEADER_BYTES || read_u16(pe, 0)? != 0x5a4d
    {
        return Err(bad());
    }
    let base = read_u32(pe, 0x3c)? as usize;
    if base < 64
        || base > pe.len().saturating_sub(24)
        || pe.get(base..base + 4) != Some(b"PE\0\0")
        || read_u16(pe, base + 4)? != MACHINE_AMD64
        || read_u16(pe, base + 20)? != 240
        || read_u16(pe, base + 22)? & 3 != 2
    {
        return Err(bad());
    }
    let opt = base + 24;
    if read_u16(pe, opt)? != OPTIONAL_MAGIC_PE32_PLUS
        || read_u16(pe, opt + 68)? != kind.subsystem()
        || read_u32(pe, opt + 108)? != 16
    {
        return Err(bad());
    }
    let meta = PeMetadata {
        entry_rva: read_u32(pe, opt + 16)?,
        image_bytes: read_u32(pe, opt + 56)?,
        headers_bytes: read_u32(pe, opt + 60)?,
        section_alignment: read_u32(pe, opt + 32)?,
        file_alignment: read_u32(pe, opt + 36)?,
        sections: u32::from(read_u16(pe, base + 6)?),
    };
    meta.validate(pe.len())?;
    let table = opt + 240;
    if table + meta.sections as usize * 40 > meta.headers_bytes as usize {
        return Err(bad());
    }
    // No imports, TLS callbacks, delay imports or CLR initialization in this
    // one-shot child. A position-independent image may have no base fixups;
    // if a directory is present it must be wholly backed by initialized data.
    for directory in [1, 9, 13, 14] {
        if read_u64(pe, opt + 112 + directory * 8)? != 0 {
            return Err(bad());
        }
    }
    let reloc = read_u32(pe, opt + 112 + 5 * 8)?;
    let reloc_size = read_u32(pe, opt + 116 + 5 * 8)?;
    if (reloc == 0) != (reloc_size == 0)
        || (reloc != 0 && reloc_size < 8)
        || reloc.checked_add(reloc_size).is_none_or(|e| e > meta.image_bytes)
    {
        return Err(bad());
    }
    let mut previous_virtual = meta.headers_bytes;
    let mut previous_raw = meta.headers_bytes;
    let mut entry = false;
    let mut relocation = reloc == 0 && reloc_size == 0;
    for i in 0..meta.sections as usize {
        let s = table + i * 40;
        let virtual_size = read_u32(pe, s + 8)?;
        let va = read_u32(pe, s + 12)?;
        let raw_size = read_u32(pe, s + 16)?;
        let raw = read_u32(pe, s + 20)?;
        let flags = read_u32(pe, s + 36)?;
        let extent = virtual_size.max(raw_size);
        let end = va.checked_add(extent).ok_or_else(bad)?;
        if extent == 0
            || va & (SECTION_ALIGNMENT - 1) != 0
            || va < previous_virtual
            || end > meta.image_bytes
            || raw_size & (FILE_ALIGNMENT - 1) != 0
            || (raw_size != 0
                && (raw & (FILE_ALIGNMENT - 1) != 0
                    || raw < previous_raw
                    || raw.checked_add(raw_size).is_none_or(|e| e as usize > pe.len())))
            || flags & 0xa0000000 == 0xa0000000
        {
            return Err(bad());
        }
        previous_virtual = end;
        if raw_size != 0 {
            previous_raw = raw + raw_size;
        }
        if meta.entry_rva >= va && meta.entry_rva < end {
            if flags & 0xe0000020 != 0x60000020
                || meta.entry_rva - va >= raw_size
                || meta.entry_rva - va >= virtual_size
            {
                return Err(bad());
            }
            entry = true;
        }
        if reloc >= va && reloc.checked_add(reloc_size).is_some_and(|e| e <= va + raw_size) {
            relocation = true;
        }
    }
    if !entry || !relocation {
        return Err(bad());
    }
    Ok(meta)
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

fn read_u32(b: &[u8], o: usize) -> Result<u32, Status> {
    let Some(&[a, b, c, d]) = b.get(o..o.checked_add(4).ok_or_else(bad)?) else {
        return Err(bad());
    };
    Ok(u32::from_le_bytes([a, b, c, d]))
}

fn read_u64(b: &[u8], o: usize) -> Result<u64, Status> {
    let Some(&[a, b, c, d, e, f, g, h]) = b.get(o..o.checked_add(8).ok_or_else(bad)?) else {
        return Err(bad());
    };
    Ok(u64::from_le_bytes([a, b, c, d, e, f, g, h]))
}

fn bad() -> Status {
    Status::COMPROMISED_DATA
}
