//! Eager extended-state ownership with private scratch on each emulator CPU.
use crate::{finish, print};
use core::{
    arch::{
        asm,
        x86_64::{__cpuid, __cpuid_count},
    },
    ptr,
};
use svmvisor_hypervisor::{
    registers::GuestRegisters,
    vmcb::Vmcb,
    xstate::{XsetbvFault, XstateArea, XstateCapabilities, XstateLayout},
};

#[repr(C)]
struct SwitchContext {
    host: *mut u8,
    guest: *mut u8,
    observed: *mut u8,
    profile: u64,
    flags: u64,
    host_seed: *const u8,
    caller: *mut u8,
    clock: *mut crate::clock::Switch,
    host_auxiliary: *mut Vmcb,
    observed_auxiliary: *mut Vmcb,
    guest_xcr0: u64,
    observed_guest_xcr0: u64,
    observed_host_xcr0: u64,
}
// Keep the private Rust/assembly bridge layout checked at compile time.
const _: () = {
    assert!(core::mem::size_of::<SwitchContext>() == 104);
    assert!(core::mem::align_of::<SwitchContext>() == 8);
    assert!(core::mem::offset_of!(SwitchContext, host) == 0);
    assert!(core::mem::offset_of!(SwitchContext, guest) == 8);
    assert!(core::mem::offset_of!(SwitchContext, observed) == 16);
    assert!(core::mem::offset_of!(SwitchContext, profile) == 24);
    assert!(core::mem::offset_of!(SwitchContext, flags) == 32);
    assert!(core::mem::offset_of!(SwitchContext, host_seed) == 40);
    assert!(core::mem::offset_of!(SwitchContext, caller) == 48);
    assert!(core::mem::offset_of!(SwitchContext, clock) == 56);
    assert!(core::mem::offset_of!(SwitchContext, host_auxiliary) == 64);
    assert!(core::mem::offset_of!(SwitchContext, observed_auxiliary) == 72);
    assert!(core::mem::offset_of!(SwitchContext, guest_xcr0) == 80);
    assert!(core::mem::offset_of!(SwitchContext, observed_guest_xcr0) == 88);
    assert!(core::mem::offset_of!(SwitchContext, observed_host_xcr0) == 96);
};
// Each physical emulator CPU owns its bridge scratch exclusively. No mutable
// reference to the whole array is formed while another CPU can be executing.
struct HostScratch {
    auxiliary: Vmcb,
    observed_auxiliary: Vmcb,
    host: XstateArea,
    observed: XstateArea,
    seed: XstateArea,
    caller: XstateArea,
}
impl HostScratch {
    const fn new() -> Self {
        Self {
            auxiliary: Vmcb::new(),
            observed_auxiliary: Vmcb::new(),
            host: XstateArea::new(),
            observed: XstateArea::new(),
            seed: XstateArea::new(),
            caller: XstateArea::new(),
        }
    }
}
// Legacy images retain their original one-CPU arena budget. Only the actual
// concurrent profile reserves the second CPU's private bridge scratch.
const HOST_CPUS: usize = if cfg!(feature = "concurrent-smp") {
    2
} else {
    1
};
static mut SCRATCH: [HostScratch; HOST_CPUS] = [const { HostScratch::new() }; HOST_CPUS];
fn scratch(cpu: usize) -> *mut HostScratch {
    assert!(cpu < HOST_CPUS);
    unsafe { ptr::addr_of_mut!(SCRATCH).cast::<HostScratch>().add(cpu) }
}
/// Guest-owned eager state; separate storage survives interleaved vCPU entries.
pub struct GuestState {
    area: XstateArea,
    expected: XstateArea,
    xcr0: u64,
}
impl GuestState {
    pub const fn new() -> Self {
        Self {
            area: XstateArea::new(),
            expected: XstateArea::new(),
            xcr0: 1,
        }
    }
    /// Architectural reset value before fixture seeding; reset_owned selects
    /// the legacy fixture's full mask. INIT does not reseed this owner.
    pub fn guest_xcr0(&self) -> u64 {
        self.xcr0
    }
}
static mut DEFAULT_GUEST: GuestState = GuestState::new();

unsafe extern "C" {
    fn xstate_enable(mask: u64);
    fn guest_run(control: *mut Vmcb, frame: *mut GuestRegisters, context: *mut SwitchContext);
}

pub struct State {
    layout: XstateLayout,
    mxcsr_mask: u32,
    cpu: usize,
}
impl State {
    /// Inert extent of this CPU's host save/switch backing for ownership audit.
    #[cfg(feature="uefi-smp")]
    pub fn host_backing(&self)->(u64,u64) {
        (scratch(self.cpu) as u64,core::mem::size_of::<HostScratch>() as u64)
    }
    /// # Safety
    /// Exclusive ownership of this fixed CPU and its scratch, before any entry.
    /// CPUID/XCR0/XSAVE rules follow APM2 chapter11 and section13.3.
    pub unsafe fn install() -> Self {
        let cpu = crate::host_smp::current_cpu();
        let host = scratch(cpu);
        let basic = __cpuid(1);
        let has_xsave = basic.ecx & (1 << 26) != 0;
        assert!(!has_xsave || __cpuid(0).eax >= 0xd);
        let leaf = if has_xsave {
            __cpuid_count(0xd, 0)
        } else {
            __cpuid_count(0, 0)
        };
        let avx = if has_xsave {
            __cpuid_count(0xd, 2)
        } else {
            __cpuid_count(0, 0)
        };
        let layout = XstateLayout::detect(XstateCapabilities {
            leaf1_ecx: basic.ecx,
            leaf1_edx: basic.edx,
            supported_xcr0: if has_xsave {
                (u64::from(leaf.edx) << 32) | u64::from(leaf.eax)
            } else {
                0
            },
            enabled_size: if has_xsave { leaf.ebx } else { 0 },
            max_size: if has_xsave { leaf.ecx } else { 0 },
            avx_size: avx.eax,
            avx_offset: avx.ebx,
            avx_flags: avx.ecx,
        })
        .unwrap();
        let cr4: u64;
        unsafe {
            asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            asm!("mov cr4, {}", in(reg) (cr4 | 0x600 | if layout.uses_xsave() { 1 << 18 } else { 0 }), options(nostack));
            xstate_enable(if layout.uses_xsave() {
                layout.mask()
            } else {
                0
            });
            let efer: u32;
            asm!("rdmsr", in("ecx") 0xc0000080u32, out("eax") efer, out("edx") _, options(nostack));
            assert_eq!(efer & (1 << 14), 0);
            if layout.uses_xsave() {
                let low: u32;
                let high: u32;
                asm!("xgetbv", in("ecx") 0u32, out("eax") low, out("edx") high, options(nostack));
                layout
                    .validate_xcr0((u64::from(high) << 32) | u64::from(low))
                    .unwrap();
                layout
                    .validate_enabled_size(__cpuid_count(0xd, 0).ebx)
                    .unwrap();
            }
            asm!("fxsave64 [{}]", in(reg) ptr::addr_of_mut!((*host).caller), options(nostack));
        }
        let reported = unsafe {
            ptr::read_unaligned(
                ptr::addr_of!((*host).caller)
                    .cast::<u8>()
                    .add(28)
                    .cast::<u32>(),
            )
        };
        let mxcsr_mask = svmvisor_hypervisor::xstate::effective_mxcsr_mask(reported).unwrap();
        if cpu == 0 {
            print(if layout.mask() == 7 {
                "xstate-profile=xsave-avx\n"
            } else if layout.uses_xsave() {
                "xstate-profile=xsave-sse\n"
            } else {
                "xstate-profile=fxsave\n"
            });
        }
        Self {
            layout,
            mxcsr_mask,
            cpu,
        }
    }
    pub fn guest_cr4(&self) -> u64 {
        0x620 | if self.layout.uses_xsave() { 1 << 18 } else { 0 }
    }
    pub fn layout(&self) -> XstateLayout {
        self.layout
    }
    pub fn uses_xsave(&self) -> bool {
        self.layout.uses_xsave()
    }
    /// Apply an intercepted XSETBV to exclusively owned stopped guest state.
    /// Rejection leaves the complete owner unchanged. Changing XCR0 changes
    /// enablement, not component contents (APM2 11.5.2/11.5.7/11.5.8). The
    /// bridge still preserves every admitted component, including legacy SSE
    /// instructions' XMM state when XCR0.SSE is clear. RIP, flags and any
    /// architectural fault delivery are committed by the exit handler.
    pub fn set_guest_xcr0(
        &self,
        guest: &mut GuestState,
        cr4: u64,
        cpl: u8,
        ecx: u32,
        value: u64,
    ) -> Result<(), XsetbvFault> {
        self.layout.validate_guest_xcr0(cr4, cpl, ecx, value)?;
        guest.xcr0 = value;
        Ok(())
    }
    pub unsafe fn reset(&self, session: usize) {
        assert_eq!(self.cpu, 0, "default guest is BSP-only");
        unsafe { self.reset_owned(&mut *ptr::addr_of_mut!(DEFAULT_GUEST), session) };
    }
    /// # Safety
    /// Exclusive stopped guest state and this CPU's private seed; no entry or
    /// outstanding state-area borrow. Seeds fixture state before Cold admission,
    /// never on INIT (APM2 14.1/15.5).
    pub unsafe fn reset_owned(&self, guest: &mut GuestState, session: usize) {
        assert_eq!(self.cpu, crate::host_smp::current_cpu());
        let host = scratch(self.cpu);
        guest.xcr0 = self.layout.mask();
        unsafe {
            seed(
                &mut *ptr::addr_of_mut!((*host).seed),
                self.layout,
                self.mxcsr_mask,
                session,
                true,
            );
            seed(
                &mut guest.area,
                self.layout,
                self.mxcsr_mask,
                session,
                false,
            );
            (&*ptr::addr_of!((*host).seed))
                .validate(self.layout, self.mxcsr_mask)
                .unwrap();
            (&guest.area)
                .validate(self.layout, self.mxcsr_mask)
                .unwrap();
            ptr::copy_nonoverlapping(&guest.area, &mut guest.expected, 1);
        }
    }
    /// The assembly bridge records host state immediately upon restoration,
    /// before any Rust instruction can use SIMD registers. Every entry also
    /// restores the original caller state before returning through the C ABI.
    pub unsafe fn run(
        &self,
        control: *mut Vmcb,
        frame: &mut GuestRegisters,
        mutation: u8,
        clock: &crate::clock::State,
    ) {
        assert_eq!(self.cpu, 0, "default guest is BSP-only");
        unsafe {
            self.run_inner(
                control,
                frame,
                mutation,
                clock,
                &mut *ptr::addr_of_mut!(DEFAULT_GUEST),
                false,
            )
        };
    }
    /// # Safety
    /// Caller exclusively owns the stopped identity-mapped VMCB, GPR frame,
    /// guest areas and this physical CPU/HSAVE. No outstanding borrows or
    /// entry on this same CPU; other CPUs use disjoint scratch. This opt-in bridge switches auxiliary VMLOAD/VMSAVE state too;
    /// the same VMCB must remain valid throughout (APM2 15.5.2/15.7/AppendixB).
    pub unsafe fn run_owned(
        &self,
        control: *mut Vmcb,
        frame: &mut GuestRegisters,
        mutation: u8,
        clock: &crate::clock::State,
        guest: &mut GuestState,
    ) {
        unsafe { self.run_inner(control, frame, mutation, clock, guest, true) };
    }
    unsafe fn run_inner(
        &self,
        control: *mut Vmcb,
        frame: &mut GuestRegisters,
        mutation: u8,
        clock: &crate::clock::State,
        guest: &mut GuestState,
        auxiliary: bool,
    ) {
        assert_eq!(self.cpu, crate::host_smp::current_cpu());
        let host = scratch(self.cpu);
        let mut clock_switch = clock.prepare();
        let mut context = unsafe {
            SwitchContext {
                host: ptr::addr_of_mut!((*host).host).cast(),
                guest: ptr::addr_of_mut!(guest.area).cast(),
                observed: ptr::addr_of_mut!((*host).observed).cast(),
                profile: if self.layout.uses_xsave() {
                    self.layout.mask()
                } else {
                    0
                },
                flags: u64::from(cfg!(feature = "xstate-broken-restore"))
                    | (u64::from(cfg!(feature = "xstate-broken-host-restore")) << 1),
                host_seed: ptr::addr_of!((*host).seed).cast(),
                caller: ptr::addr_of_mut!((*host).caller).cast(),
                clock: &mut clock_switch,
                host_auxiliary: if auxiliary {
                    ptr::addr_of_mut!((*host).auxiliary)
                } else {
                    ptr::null_mut()
                },
                observed_auxiliary: if auxiliary {
                    ptr::addr_of_mut!((*host).observed_auxiliary)
                } else {
                    ptr::null_mut()
                },
                guest_xcr0: guest.xcr0,
                observed_guest_xcr0: 0,
                observed_host_xcr0: 0,
            }
        };
        unsafe {
            guest_run(control, frame, &mut context);
        }
        if self.layout.uses_xsave() {
            assert_eq!(context.observed_guest_xcr0, guest.xcr0);
            assert_eq!(context.observed_host_xcr0, self.layout.mask());
        }
        clock.verify(&clock_switch);
        if auxiliary {
            // APM2 15.5.2/AppendixB: only architecturally saved auxiliary fields.
            for (begin, end) in [
                (0x440, 0x460),
                (0x470, 0x480),
                (0x490, 0x4a0),
                (0x600, 0x640),
            ] {
                unsafe {
                    let expected = &(&*ptr::addr_of!((*host).auxiliary)).bytes()[begin..end];
                    let observed =
                        &(&*ptr::addr_of!((*host).observed_auxiliary)).bytes()[begin..end];
                    if expected != observed {
                        // Bounded failure-only evidence; preserve the exact assertion.
                        // Qword values plus absolute VMCB offset identify every byte.
                        print("AUXILIARY-MISMATCH range-begin=");
                        crate::hex(begin as u64);
                        print("AUXILIARY-MISMATCH range-end=");
                        crate::hex(end as u64);
                        print("AUXILIARY-MISMATCH guest-exit=");
                        crate::hex((&*control).exit_snapshot().code);
                        for index in (0..end - begin).step_by(8) {
                            let a =
                                u64::from_le_bytes(expected[index..index + 8].try_into().unwrap());
                            let b =
                                u64::from_le_bytes(observed[index..index + 8].try_into().unwrap());
                            if a != b {
                                print("AUXILIARY-MISMATCH offset=");
                                crate::hex((begin + index) as u64);
                                print("AUXILIARY-MISMATCH expected=");
                                crate::hex(a);
                                print("AUXILIARY-MISMATCH observed=");
                                crate::hex(b);
                            }
                        }
                    }
                    assert_eq!(expected, observed);
                }
            }
        }
        unsafe {
            (&*ptr::addr_of!((*host).host))
                .validate(self.layout, self.mxcsr_mask)
                .unwrap();
            (&guest.area)
                .validate(self.layout, self.mxcsr_mask)
                .unwrap();
            (&*ptr::addr_of!((*host).observed))
                .validate(self.layout, self.mxcsr_mask)
                .unwrap();
            if mutation == 1 {
                let expected = (&mut guest.expected).bytes_mut();
                for byte in &mut expected[144..152] {
                    *byte = byte.wrapping_add(1);
                }
                for byte in &mut expected[400..416] {
                    *byte ^= 1;
                }
                if let Some(offset) = self.layout.avx_offset() {
                    for byte in &mut expected[offset + 240..offset + 256] {
                        *byte ^= 1;
                    }
                }
            }
            if mutation == 2 {
                let expected = (&mut guest.expected).bytes_mut();
                expected[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
                expected[2..4].copy_from_slice(&0x3800u16.to_le_bytes());
                expected[4] = 0x80;
                expected[32..40].copy_from_slice(&0x8000000000000000u64.to_le_bytes());
                expected[40..42].copy_from_slice(&0x4000u16.to_le_bytes());
            }
            if mutation == 3 {
                // Actual legacy XORPS while XCR0.SSE is clear still modifies
                // XMM15. Unlike VEX encoding, it preserves the upper YMM half.
                for byte in &mut guest.expected.bytes_mut()[400..416] {
                    *byte ^= 1;
                }
            }
            if !matches_state(
                &*ptr::addr_of!((*host).seed),
                &*ptr::addr_of!((*host).observed),
                self.layout,
                false,
            ) {
                print("FAIL xstate-host-isolation\n");
                finish(false);
            }
            if !matches_state(&guest.expected, &guest.area, self.layout, mutation == 2) {
                print("FAIL xstate-guest-isolation\n");
                finish(false);
            }
        }
    }
}
fn seed(area: &mut XstateArea, layout: XstateLayout, mask: u32, session: usize, host: bool) {
    area.reset(layout, mask).unwrap();
    let bytes = area.bytes_mut();
    bytes[0..2].copy_from_slice(&(if host { 0x037fu16 } else { 0x077fu16 }).to_le_bytes());
    bytes[4] = 0xff;
    bytes[24..28].copy_from_slice(&(if host { 0x1f80u32 } else { 0x3f80u32 }).to_le_bytes());
    let base = if host { 0xa0u8 } else { 0x20u8 };
    for register in 0..8 {
        for lane in 0..8 {
            bytes[32 + register * 16 + lane] = base
                .wrapping_add(session as u8)
                .wrapping_add((register * 8 + lane) as u8);
        }
        bytes[32 + register * 16 + 8..32 + register * 16 + 10].fill(0xff);
    }
    for index in 160..416 {
        bytes[index] = base.wrapping_add(session as u8).wrapping_add(index as u8);
    }
    if let Some(offset) = layout.avx_offset() {
        for index in 0..256 {
            bytes[offset + index] = base
                .wrapping_add(session as u8)
                .wrapping_add(index as u8)
                .wrapping_add(0x31);
        }
    }
    if layout.uses_xsave() {
        bytes[512..520].copy_from_slice(&layout.mask().to_le_bytes());
    }
}
fn matches_state(
    expected: &XstateArea,
    actual: &XstateArea,
    layout: XstateLayout,
    x87_only: bool,
) -> bool {
    let a = expected.bytes();
    let b = actual.bytes();
    if a[0..5] != b[0..5] || a[24..28] != b[24..28] || a[160..416] != b[160..416] {
        return false;
    }
    for register in 0..if x87_only { 1 } else { 8 } {
        let offset = 32 + register * 16;
        if a[offset..offset + 10] != b[offset..offset + 10] {
            return false;
        }
    }
    if let Some(offset) = layout.avx_offset() {
        if a[offset..offset + 256] != b[offset..offset + 256] {
            return false;
        }
    }
    true
}
