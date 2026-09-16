//! Optional parent-owned synchronous LoadOptions result, never an admission input.
use core::{
    mem::{align_of, size_of},
    ptr,
};
use svmvisor_dxe::diagnostics::native_result::NativeResult;
use uefi_raw::{
    Handle, Status, protocol::loaded_image::LoadedImageProtocol, table::boot::BootServices,
};

pub(crate) struct Mailbox(*mut NativeResult);
impl Mailbox {
    /// Firmware supplies a live LoadedImage protocol. Nonempty LoadOptions must
    /// be readable/writable and remain owned by the synchronous parent until
    /// StartImage returns. Only the exact pristine ABI is accepted.
    pub(crate) unsafe fn attach(
        image: Handle,
        services: &BootServices,
    ) -> Result<Option<Self>, Status> {
        let mut interface = ptr::null_mut();
        let opened = unsafe {
            (services.open_protocol)(
                image,
                &LoadedImageProtocol::GUID,
                &mut interface,
                image,
                ptr::null_mut(),
                2,
            )
        };
        if opened != Status::SUCCESS {
            return Err(opened);
        }
        let result = (|| {
            let loaded = unsafe { interface.cast::<LoadedImageProtocol>().as_ref() }
                .ok_or(Status::DEVICE_ERROR)?;
            if loaded.load_options_size == 0 && loaded.load_options.is_null() {
                return Ok(None);
            }
            if loaded.load_options_size != size_of::<NativeResult>() as u32
                || loaded.load_options.is_null()
                || loaded.load_options.addr() % align_of::<NativeResult>() != 0
            {
                return Err(Status::UNSUPPORTED);
            }
            let result = loaded.load_options.cast::<NativeResult>().cast_mut();
            if unsafe { result.read() } != NativeResult::new() {
                return Err(Status::UNSUPPORTED);
            }
            Ok(Some(Self(result)))
        })();
        let closed = unsafe {
            (services.close_protocol)(image, &LoadedImageProtocol::GUID, image, ptr::null_mut())
        };
        if closed != Status::SUCCESS {
            return Err(closed);
        }
        if let Ok(Some(mailbox)) = &result {
            unsafe { (*mailbox.0).rust_entered = 1 };
        }
        result
    }

    /// Inner completion only. The parent's separate StartImage result observes
    /// the later assembly epilogue's return; this marker cannot prove it.
    pub(crate) unsafe fn complete(self, mut result: NativeResult) {
        result.rust_entered = 1;
        result.rust_completed = 1;
        unsafe { self.0.write(result) };
    }
}
