//! Shared stopped-VMCB setup for the emulator's bounded guest profiles.
use crate::{field, memory};
use svmvisor_hypervisor::{
    arch::x86_64::capabilities::ValidatedCapabilities,
    guest::state::ValidatedGuestState,
    svm::permission_maps::{Iopm, Msrpm},
    svm::vmcb::{InstructionIntercept, Vmcb},
};
// Host-only, permanently intercepting maps; never included in guest backing.
static IOPM: Iopm = Iopm::new();
static MSRPM: Msrpm = Msrpm::new();
/// # Safety
/// Exclusive stopped aligned VMCB and owned prepared mappings/descriptors.
/// Host/SVM/xstate entry prerequisites are separately established by the caller.
/// APM vol.2 rev.3.44 sections15.5/15.12 and AppendixB. No firmware calls.
pub unsafe fn initialize(
    control: *mut Vmcb,
    prepared: &memory::Prepared,
    caps: &ValidatedCapabilities,
    state: &ValidatedGuestState,
    idt: Option<(u64, u32)>,
) {
    unsafe {
        control.write(Vmcb::new());
        let v = &mut *control;
        v.set_tsc_offset_zero();
        v.set_guest_asid(1, caps).unwrap();
        v.set_nested_root(prepared.nested_root, &caps.address_policy())
            .unwrap();
        v.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
            &caps.address_policy(),
        )
        .unwrap();
        for intercept in [
            InstructionIntercept::Cpuid,
            InstructionIntercept::Hlt,
            InstructionIntercept::Vmrun,
            InstructionIntercept::Vmmcall,
            InstructionIntercept::Xsetbv,
            InstructionIntercept::Msr,
            InstructionIntercept::Ioio,
        ] {
            v.set_instruction_intercept(intercept, true);
        }
        v.set_synthetic_state(state);
        v.set_guest_descriptors(&prepared.descriptors);
        field(control, 0x008, u32::MAX.to_le_bytes());
        field(control, 0x668, 0x0007040600070406u64.to_le_bytes());
        field(control, 0x05c, [1]);
        field(control, 0x090, 1u64.to_le_bytes());
        field(control, 0x560, 0x400u64.to_le_bytes());
        field(control, 0x568, 0xffff0ff0u64.to_le_bytes());
        if let Some((base, limit)) = idt {
            field(control, 0x484, limit.to_le_bytes());
            field(control, 0x488, base.to_le_bytes());
        }
    }
}

/// Install a fixture-owned MSRPM while reusing the sole intercepting IOPM.
/// # Safety
/// Stopped exclusive VMCB; map is aligned identity-mapped host-only storage and
/// remains live/immutable through entry. APM2 15.11,15.12; all unrelated MSRs
/// remain intercepted. No guest address may reach this map.
pub unsafe fn permissions(
    control: *mut Vmcb,
    caps: &ValidatedCapabilities,
    map: Option<*const Msrpm>,
) {
    unsafe {
        (&mut *control)
            .set_permission_maps(
                core::ptr::addr_of!(IOPM) as u64,
                map.unwrap_or(core::ptr::addr_of!(MSRPM)) as u64,
                &caps.address_policy(),
            )
            .unwrap()
    };
}
