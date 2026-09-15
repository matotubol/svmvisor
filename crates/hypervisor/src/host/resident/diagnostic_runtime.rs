//! Bounded, best-effort progress for the admitted first-boot PCI endpoint.
//! APM2 5.4/7.8.5/15.10 and PPR57896 2.1.6.1: dedicated host UC aliases;
//! every participating guest CPU traps target configuration writes. Publication
//! shares their lifetime guard and stops permanently before a forwarded write.
//! Firmware/SMM, reset and machine checks are outside this first-boot transport
//! guarantee. A missing record is never proof that the CPU reached no later code.
use super::*;
use core::sync::atomic::AtomicU32;

const CONFIG_ALIAS: u64 = 0xfb000;
const BAR_ALIAS: u64 = 0xfc000;
static READY: AtomicU32 = AtomicU32::new(0);
static SEQUENCE: AtomicU32 = AtomicU32::new(0);
static PAUSE_SAMPLE: AtomicU64 = AtomicU64::new(0);
static mut ENDPOINT: TerminalEndpoint = TerminalEndpoint {
    config_page:0, bar0_host_page:0, fpga_build_id:0, rom_build_id:0,
    mmio_config_msr:0, bar0_raw:0, segment_bdf:0, boot_id:0, command:0,
    version:0, reserved:0,
};
static mut SLOT: usize = 0;
static mut COUNT: usize = 0;

/// Before guest entry, current firmware root and exclusively owned host tables.
/// The caller has installed target config interception in every resident NPT.
pub(super) unsafe fn prepare(endpoint: TerminalEndpoint, slot:usize, count:usize) -> bool {
    let (pool,bytes) = unsafe { POOL };
    if slot >= count || count > 32 || !endpoint.valid() { return false; }
    let pat = unsafe { read_msr(0x277) };
    let Some(uc) = (0..8).find(|i| (pat >> (i*8)) & 255 == 0) else { return false; };
    let Some(mt) = (unsafe { native_mtrrs(__cpuid_count(0x80000008,0).eax as u8) }) else { return false; };
    for page in [endpoint.config_page,endpoint.bar0_host_page] {
        if (page < pool+bytes && pool < page+4096) || !mt.terminal_page_is_uc(page,0) { return false; }
    }
    let flags = 3 | (1u64<<63) | ((uc&1)<<3) | ((uc&2)<<3) | ((uc&4)<<5);
    let base = ptr::addr_of!(image_start) as u64;
    unsafe {
        for (offset,page) in [(CONFIG_ALIAS,endpoint.config_page),(BAR_ALIAS,endpoint.bar0_host_page)] {
            let entry = ptr::addr_of_mut!((*ptr::addr_of_mut!(TABLES)).0[3][(((base+offset)>>12)&511) as usize]);
            if entry.read() != 0 { return false; }
            entry.write(page|flags);
        }
        ENDPOINT = endpoint; SLOT = slot; COUNT = count;
    }
    READY.store(1,Ordering::Release);
    true
}

pub(super) fn available() -> bool { READY.load(Ordering::Acquire) == 1 }
pub(super) unsafe fn endpoint() -> Option<TerminalEndpoint> {
    if available() { Some(unsafe { ptr::addr_of!(ENDPOINT).read() }) } else { None }
}

/// No STATE or guest-memory access: also callable from the private fault IST.
/// Progress tries once. Faults make at most256 attempts to outlast another
/// CPU's short publication; an interrupted self-held guard still cannot unwind.
pub(super) unsafe fn record(event:u8,fault:bool,context:[u64;6],aux:u32) {
    if !available() { return; }
    if event == 5 {
        let lo:u32; let hi:u32;
        unsafe { asm!("rdtsc",out("eax")lo,out("edx")hi,options(nostack,preserves_flags)); }
        let now=u64::from(lo)|(u64::from(hi)<<32);
        let last=PAUSE_SAMPLE.load(Ordering::Relaxed);
        if last != 0 && now.wrapping_sub(last) < 50_000_000 { return; }
        PAUSE_SAMPLE.store(now,Ordering::Relaxed);
    }
    let control = unsafe { terminal_control() };
    if !control.ready(unsafe { COUNT }) || control.diagnostic_revoked() { return; }
    for _ in 0..if fault { 256 } else { 1 } {
        if let Some(_guard) = control.diagnostic_lock() {
            unsafe { record_locked(event,fault,context,aux); }
            return;
        }
        core::hint::spin_loop();
    }
}

/// Caller owns the shared diagnostic lifetime guard; immutable initialized
/// endpoint and dedicated aliases are independent of GuestReader scratch space.
unsafe fn record_locked(event:u8,fault:bool,context:[u64;6],aux:u32) {
    let Some((endpoint,bar)) = (unsafe { checked_endpoint_locked() }) else { return; };
    let mut seq = SEQUENCE.fetch_add(1,Ordering::Relaxed).wrapping_add(1);
    if seq == 0 { seq = SEQUENCE.fetch_add(1,Ordering::Relaxed).wrapping_add(1); }
    let lo:u32; let hi:u32;
    unsafe { asm!("rdtsc",out("eax")lo,out("edx")hi,options(nostack,preserves_flags)); }
    let payload=terminal::diagnostic_payload(seq,event,fault,endpoint.boot_id,
        unsafe{ASSIGNED_APIC_ID},lo as u64|((hi as u64)<<32),context,aux);
    let _=terminal::commit_diagnostic(&mut TerminalJournal(bar),unsafe{SLOT},payload);
}

/// Validate routing before touching the BAR. Caller holds the shared lifetime
/// guard from this check through its final same-device completion read.
unsafe fn checked_endpoint_locked() -> Option<(TerminalEndpoint,u64)> {
    let control = unsafe { terminal_control() };
    if !available() || !control.ready(unsafe { COUNT }) || control.diagnostic_revoked() { return None; }
    let endpoint = unsafe { ptr::addr_of!(ENDPOINT).read() };
    if unsafe { read_msr(terminal::MMIO_CONFIG_MSR) } != endpoint.mmio_config_msr {
        control.diagnostic_revoke(); return None;
    }
    let Some(mt) = (unsafe { native_mtrrs(PHYSICAL_BITS) }) else { return None; };
    if !mt.terminal_page_is_uc(endpoint.config_page,0) || !mt.terminal_page_is_uc(endpoint.bar0_host_page,0) { return None; }
    let base = ptr::addr_of!(image_start) as u64;
    let cfg = base+CONFIG_ALIAS;
    let read_cfg = |offset| unsafe { terminal::read_config_dword(cfg,offset) };
    if read_cfg(0) != terminal::PCI_VENDOR_DEVICE || read_cfg(8) != terminal::PCI_CLASS_REVISION
        || (read_cfg(0x0c)>>16)&0x7f != 0 || read_cfg(4)&2 == 0 || read_cfg(0x10) != endpoint.bar0_raw {
        control.diagnostic_revoke(); return None;
    }
    let bar = base+BAR_ALIAS;
    let read = |offset| unsafe { ((bar+offset) as *const u32).read_volatile() };
    if read(0) != 0x4a4d5653 || read(4) != 0x00030001
        || (u64::from(read(8)) | (u64::from(read(12))<<32)) != endpoint.fpga_build_id
        || (u64::from(read(16)) | (u64::from(read(20))<<32)) != endpoint.rom_build_id { return None; }
    Some((endpoint,bar))
}

/// Terminal owner only, all guests permanently stopped with IF/GIF clear.
/// Reuses the prepared UC mappings and the same lifetime owner as live records;
/// never installs a scratch PTE or accesses a guest-memory alias.
pub(super) unsafe fn export_terminal(words:[u32;3]) -> bool {
    if !available() || __cpuid_count(1,0).eax != 0x00b4_0f40 { return false; }
    let control = unsafe { terminal_control() };
    if !control.all_acknowledged(unsafe { COUNT })
        || control.diagnostic_snapshot()[2] != unsafe { SLOT as u64 }+1 { return false; }
    // Another CPU can still be finishing its final acknowledgement record.
    // Bound acquisition; failure retains the per-CPU first-fault records.
    for _ in 0..256 {
        if let Some(_guard) = control.diagnostic_lock() {
            let Some((endpoint,bar)) = (unsafe { checked_endpoint_locked() }) else { return false; };
            let mut io = TerminalJournal(bar);
            use terminal::JournalIo;
            if io.read(0x24) != Ok(0) || io.read(0x84) != Ok(endpoint.boot_id) { return false; }
            let Ok(sequence) = io.read(0x2c) else { return false; };
            let lo:u32; let hi:u32;
            unsafe { asm!("rdtsc",out("eax")lo,out("edx")hi,options(nostack,preserves_flags)); }
            return terminal::commit_record(&mut io,[sequence.wrapping_add(1),endpoint.boot_id,lo,hi,
                words[0],words[1],words[2],0x0008_0013]).is_ok();
        }
        core::hint::spin_loop();
    }
    false
}

struct TerminalJournal(u64);
impl terminal::JournalIo for TerminalJournal {
    type Error = ();
    fn read(&mut self,offset:u64)->Result<u32,()> {
        if offset>0x9c || offset&3!=0 {return Err(());}
        Ok(unsafe { ((self.0+offset) as *const u32).read_volatile() })
    }
    fn write(&mut self,offset:u64,value:u32)->Result<(),()> {
        if (!(0x40..=0x60).contains(&offset) && !(0x600..=0xffc).contains(&offset)) || offset&3!=0 {return Err(());}
        unsafe { ((self.0+offset) as *mut u32).write_volatile(value); }
        Ok(())
    }
}

/// Under lifetime guard, preserve the last usable record before native config
/// mutation. Permanent revocation is explicit; Windows retains its real write.
pub(super) unsafe fn revoke(address:u64,value:u64,width:u8,reason:u32) {
    let Some(endpoint) = (unsafe { endpoint() }) else { return; };
    unsafe { record_locked(6,false,[endpoint.config_page,endpoint.bar0_host_page,address,value,width as u64,0],reason); }
    unsafe { terminal_control() }.diagnostic_revoke();
}

/// Actual IOIO exit under GIF/IF=0; prepared instruction precedes native I/O.
pub(super) unsafe fn handle_io(state:&mut State,vmcb:&mut Vmcb) -> bool {
    let exit = vmcb.exit_snapshot();
    if !available() { return stop(state,exit.code,exit.rip,0xf200,0); }
    let Some(guard) = (unsafe { terminal_control() }).diagnostic_lock() else { return retry_routing(state,vmcb); };
    let prepared = match crate::svm::native_diagnostic_config::prepare_io(vmcb) {
        Ok(p) => p,
        Err(_) => { drop(guard); return stop(state,exit.code,exit.rip,0xf201,exit.info1); }
    };
    let port=prepared.port(); let width=prepared.width_bytes(); let mut value=prepared.output_value();
    if prepared.revoke() { unsafe { revoke(port as u64,value as u64,width,2); } }
    unsafe {
        if prepared.input() {
            value = match width {
                1 => { let v:u8; asm!("in al,dx",in("dx")port,out("al")v,options(nostack,preserves_flags)); v as u32 },
                2 => { let v:u16; asm!("in ax,dx",in("dx")port,out("ax")v,options(nostack,preserves_flags)); v as u32 },
                _ => { let v:u32; asm!("in eax,dx",in("dx")port,out("eax")v,options(nostack,preserves_flags)); v },
            };
        } else { match width {
            1 => asm!("out dx,al",in("dx")port,in("al")value as u8,options(nostack,preserves_flags)),
            2 => asm!("out dx,ax",in("dx")port,in("ax")value as u16,options(nostack,preserves_flags)),
            _ => asm!("out dx,eax",in("dx")port,in("eax")value,options(nostack,preserves_flags)),
        }}
    }
    prepared.commit(value); state.routing_retries=0; true
}

