//! Disposable OVMF MP failure injection; neither production PE is modified.
//! PI1.10 MP first slot is GetNumberOfProcessors. Delegate initial discovery,
//! then fail its third invocation in the separate stage19 admission pass.
use super::*;
type Counts=unsafe extern "efiapi" fn(*const c_void,*mut usize,*mut usize)->Status;
static mut ADDRESS:*mut Counts=ptr::null_mut();
static mut ORIGINAL:Option<Counts>=None;
static mut CALLS:usize=0;
unsafe extern "efiapi" fn counts(mp:*const c_void,total:*mut usize,enabled:*mut usize)->Status {
    unsafe{CALLS+=1;if CALLS==3 {marker("PASS injected-admission-mp-device-error\n");return Status::DEVICE_ERROR;}
        ORIGINAL.unwrap()(mp,total,enabled)}
}
pub unsafe fn install(){
    let vendor=__cpuid(0x40000000);assert_eq!([vendor.ebx,vendor.ecx,vendor.edx],[0x54474354,0x43544743,0x47435447]);
    let mut raw=ptr::null_mut();
    check(unsafe{(services().locate_protocol)(&guid!("3fdda605-a76e-4f46-ad29-12f4531b3d08"),ptr::null_mut(),&mut raw)},"locate actual OVMF MP");
    assert!(!raw.is_null());unsafe{ADDRESS=raw.cast();ORIGINAL=Some(ADDRESS.read());ADDRESS.write(counts);}
}
pub unsafe fn restore(){unsafe{ADDRESS.write(ORIGINAL.unwrap());let calls=CALLS;assert_eq!(calls,3);}}
