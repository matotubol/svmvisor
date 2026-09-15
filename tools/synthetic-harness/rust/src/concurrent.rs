//! Two-host/two-guest conformance using flat or admitted post-EBS ownership.
use crate::{clock, execution, field, hex, host_smp, memory, print, xstate};
use core::{arch::asm, ptr, sync::atomic::{AtomicBool, AtomicU64, Ordering}};
use svmvisor_hypervisor::{
    arch::x86_64::{capabilities::{EvidenceFlag, ValidatedCapabilities}, registers::GuestRegisters},
    boot::ownership::OwnershipRecord,
    guest::state::GuestStateRequest,
    memory::npt::NptEvidence,
    svm::{dispatch::{handle_exit_with_instruction, DispatchOutcome, StopReason},
        ipi::{IpiMailbox, IpiTarget, MailboxTarget, StartupState},
        apic_scheduler::{ClockRate, ScheduledApic, WaitOutcome}, local_apic::LocalApic,
        permission_maps::{MsrAccess, Msrpm, Permission}, vmcb::Vmcb,
        x2apic::FixtureApic},
};
static mut CONTROLS: [Vmcb; 2] = [Vmcb::new(), Vmcb::new()];
static mut EXTENDED: [xstate::GuestState; 2] = [xstate::GuestState::new(), xstate::GuestState::new()];
static mut MAP: Msrpm = Msrpm::new();
static mut STARTUP_MAP: Msrpm = Msrpm::new();
static MAILBOXES: [IpiMailbox; 2] = [IpiMailbox::new(0), IpiMailbox::new_cold_ap()];
static PARK_READY: [AtomicU64;2] = [const { AtomicU64::new(0) };2];
static RACE_READY: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy)]
struct Shared { prepared: *const memory::Prepared, caps: *const ValidatedCapabilities }
static mut SHARED: Shared = Shared { prepared: ptr::null(), caps: ptr::null() };
#[derive(Clone, Copy)]
struct Metrics { entries:u64, intrs:u64, running:u64, acks:u64, sent:u64,
    delivered:u64, eois:u64, queries:u64, race:u64, min:u64, max:u64, last:u64,
    startup_sent:u64, startup_applied:u64, real:u64, long:u64, hlts:u64, wakes:u64,
    before_park:u64, after_park:u64, after_poll:u64,
    timer_acks:u64, timer_intrs:u64, ipi_intrs:u64, both_intrs:u64, voluntary_timer_acks:u64,
    timer_delivered:u64,timer_eois:u64,timer_programs:u64,timer_hlts:u64,timer_wakes:u64,
    spin_intrs:u64,spin_progress:u64,spin_no_progress:u64,spin_sessions:u64,
    idle_returns:u64,idle_timer_acks:u64,idle_ipi_acks:u64,watchdogs:u64,
    min_late:u64,max_late:u64,min_idle:u64,max_idle:u64 }
impl Metrics { const fn new()->Self { Self {entries:0,intrs:0,running:0,acks:0,sent:0,
    delivered:0,eois:0,queries:0,race:0,min:u64::MAX,max:0,last:0,
    startup_sent:0,startup_applied:0,real:0,long:0,hlts:0,wakes:0,
    before_park:0,after_park:0,after_poll:0,
    timer_acks:0,timer_intrs:0,ipi_intrs:0,both_intrs:0,voluntary_timer_acks:0,
    timer_delivered:0,timer_eois:0,timer_programs:0,timer_hlts:0,timer_wakes:0,
    spin_intrs:0,spin_progress:0,spin_no_progress:0,spin_sessions:0,
    idle_returns:0,idle_timer_acks:0,idle_ipi_acks:0,watchdogs:0,
    min_late:u64::MAX,max_late:0,min_idle:u64::MAX,max_idle:0} } }
static mut METRICS: [Metrics;2] = [Metrics::new(),Metrics::new()];
#[cfg(feature="uefi-smp")]
static mut OWNED_RANGES: [[(u64,u64);8];2] = [[(0,0);8];2];
unsafe extern "C" {
    static guest_concurrent_start:u8;
    static guest_concurrent_end:u8;
    static guest_concurrent_handler:u8;
    static guest_concurrent_bsp:u8;
    static guest_concurrent_long:u8;
    static guest_concurrent_hlt:u8;
    static guest_concurrent_wait_start:u8;
    static guest_concurrent_wait_end:u8;
    static guest_concurrent_timer_handler:u8;
    static guest_concurrent_timer_hlt:u8;
    static guest_concurrent_spin:u8;
    static guest_concurrent_spin_end:u8;
}
fn address(p:*const u8)->u64 { 0x1000 + p as u64 - ptr::addr_of!(guest_concurrent_start) as u64 }
fn word(v:&Vmcb,offset:usize)->u64 {u64::from_le_bytes(v.bytes()[offset..offset+8].try_into().unwrap())}
fn flags(v:&Vmcb)->u64 {word(v,0x570)}
fn digest(v:&Vmcb)->u64 {v.bytes().iter().fold(0xcbf29ce484222325,|h,b|(h^u64::from(*b)).wrapping_mul(0x100000001b3))}
struct FailureWriter;
impl core::fmt::Write for FailureWriter {
    fn write_str(&mut self,text:&str)->core::fmt::Result { print(text);Ok(()) }
}
fn bounded_wait(mut ready:impl FnMut()->bool) {
    for _ in 0..100_000_000 { if ready(){return} core::hint::spin_loop(); }
    panic!("concurrent host handshake timeout");
}
// The interrupt gate touches host-owned source counters only. An actual host
// sleep cannot retire a guest instruction or manufacture an APIC request.
unsafe fn idle_once(v:&Vmcb,frame:&GuestRegisters,metrics:&mut Metrics)->host_smp::Sources {
    let saved=digest(v);let saved_frame=*frame;
    let before=host_smp::now();
    let sources=unsafe{host_smp::idle()};
    let span=host_smp::now().checked_sub(before).unwrap();assert!(span>0);
    metrics.min_idle=metrics.min_idle.min(span);metrics.max_idle=metrics.max_idle.max(span);
    assert_eq!(digest(v),saved);assert_eq!(*frame,saved_frame);
    assert!(sources.timer+sources.ipi>0);
    metrics.idle_returns+=1;metrics.timer_acks+=sources.timer;metrics.acks+=sources.ipi;
    metrics.idle_timer_acks+=sources.timer;metrics.idle_ipi_acks+=sources.ipi;
    sources
}
extern "C" fn ap_entry() {
    unsafe {
        let state=xstate::State::install();
        let clock=clock::State::capture().for_guest(1);
        drive(1,&state,&clock);
    }
}
/// # Safety
/// BSP exclusively owns stopped flat or retained post-EBS fixture memory and has installed
/// host tables/SVM. AP startup is admitted by host_smp before publication. The
/// immutable Prepared/code/map lifetime extends past join. Each CPU exclusively
/// owns its indexed VMCB/frame/XSTATE/stack/APIC. APM2rev3.44 ch7,15,16.
pub unsafe fn run(ownership:Option<&OwnershipRecord<'_>>, caps:&ValidatedCapabilities,
    state:&xstate::State, clock:&clock::State) {

    unsafe {host_smp::prepare(ownership)};
    let source=ptr::addr_of!(guest_concurrent_start);
    let code=unsafe{core::slice::from_raw_parts(source,ptr::addr_of!(guest_concurrent_end) as usize-source as usize)};
    let prepared=unsafe{memory::prepare(caps.address_policy(),code,ownership,NptEvidence{
        nx_supported:EvidenceFlag::Set,host_nxe:EvidenceFlag::Set,host_four_level:EvidenceFlag::Set})};
    unsafe {
        memory::prepare_smp_descriptors();
        memory::install_idt(&[(0x50,address(ptr::addr_of!(guest_concurrent_handler)),true),
            (0x51,address(ptr::addr_of!(guest_concurrent_timer_handler)),true)]);
        for map in [ptr::addr_of_mut!(MAP),ptr::addr_of_mut!(STARTUP_MAP)] {
            for access in [MsrAccess::Read,MsrAccess::Write] {(&mut *map).set(0xc0000100,access,Permission::Allow).unwrap();}
        }
        for access in [MsrAccess::Read,MsrAccess::Write] {(&mut *ptr::addr_of_mut!(STARTUP_MAP)).set(0xc0000080,access,Permission::Allow).unwrap();}
        for id in 0..2 {
            let control=ptr::addr_of_mut!(CONTROLS).cast::<Vmcb>().add(id);
            let guest=GuestStateRequest {rip:address(ptr::addr_of!(guest_concurrent_bsp)),rsp:if id==0{0x9000}else{0xb000},rflags:2,
                cr0:0x80010033,cr3:prepared.guest_cr3,cr4:state.guest_cr4(),efer:0x1500,rax:0}
                .validate_with_xstate(&caps.address_policy(),state.layout()).unwrap();
            execution::initialize(control,&prepared,caps,&guest,Some((0x3000,4095)));
            crate::x2apic::admit(control);
            field(control,0x004,u32::MAX.to_le_bytes());
            (&mut *control).enable_physical_interrupt_virtualization().unwrap();
            execution::permissions(control,caps,Some(if id==0{ptr::addr_of!(MAP)}else{ptr::addr_of!(STARTUP_MAP)}));
        }
        SHARED=Shared{prepared:&prepared,caps};
        host_smp::start(ap_entry);
        drive(0,state,&clock.for_guest(0));
        host_smp::join();
        memory::verify_smp_stacks();
    }
    #[cfg(feature="uefi-smp")]
    {
        let ranges=unsafe{ptr::addr_of!(OWNED_RANGES).read()};
        let arena=ownership.unwrap().arena();
        let names=["host-rsp","hsave","vmcb","guest-xstate","gpr","controller","host-xstate","clock"];
        for id in 0..2 {
            for (index,(base,length)) in ranges[id].iter().copied().enumerate() {
                assert!(base>=arena.base() && length>0 && base.checked_add(length).unwrap()<=arena.last_byte()+1);
                for (other,bytes) in ranges[1-id] { assert!(base+length<=other || other+bytes<=base); }
                print(if id==0{"UEFI-SMP cpu0-"}else{"UEFI-SMP cpu1-"});print(names[index]);print("=");hex(base);
            }
        }
        print("PASS uefi-smp-owned-ranges=16 arena-contained-cross-cpu-disjoint\n");
    }
    for id in 0..2 {
        let m=unsafe{ptr::addr_of!(METRICS).cast::<Metrics>().add(id).read()};
        assert_eq!(m.queries,64); assert_eq!(m.delivered,32); assert_eq!(m.eois,32);
        assert_eq!(m.sent,40); assert!(m.running>0); assert!(m.intrs>0);
        assert_eq!(m.race,u64::from(id==1));
        assert_eq!(m.startup_sent,if id==0{3}else{0});
        assert_eq!(m.startup_applied,if id==1{2}else{0});
        assert_eq!(m.real,u64::from(id==1));assert_eq!(m.long,u64::from(id==1));
        assert_eq!(m.hlts,8);assert_eq!(m.wakes,8);
        assert_eq!((m.before_park,m.after_park,m.after_poll),(3,3,2));
        assert_eq!((m.timer_delivered,m.timer_eois,m.timer_programs),(16,16,16));
        assert_eq!((m.timer_hlts,m.timer_wakes,m.spin_sessions,m.watchdogs),(8,8,8,2));
        assert!(m.spin_intrs>=8 && m.spin_progress>=8);
        assert_eq!(m.timer_acks,m.timer_intrs+m.voluntary_timer_acks+m.idle_timer_acks);
        assert_eq!(m.intrs,m.timer_intrs+m.ipi_intrs-m.both_intrs);
        assert_eq!(m.timer_acks,host_smp::timer_acknowledgements(id));
        assert!(host_smp::timer_cancel_active_races(id)<=host_smp::timer_cancel_races(id));
        assert!(host_smp::timer_cancel_races(id)<=m.timer_acks);
        assert_eq!(m.idle_returns,host_smp::idle_returns(id));
        assert!(host_smp::idle_timer_wakes(id)+host_smp::idle_ipi_wakes(id)>=m.idle_returns);
        assert!(host_smp::idle_ipi_wakes(id)>0);
        assert!(m.idle_returns>=18 && m.idle_timer_acks>=10);
        assert_eq!(m.entries,if id==0{251}else{249}+m.intrs);
        assert!(!MAILBOXES[id].pending());
        for (name,value) in [("entries",m.entries),("intrs",m.intrs),("running-intrs",m.running),
            ("acks",m.acks),("sent",m.sent),("delivered",m.delivered),("eois",m.eois),
            ("queries",m.queries),("entry-races",m.race),
            ("startup-sent",m.startup_sent),("startup-applied",m.startup_applied),
            ("real-starts",m.real),("long-starts",m.long),("hlts",m.hlts),("wakes",m.wakes),
            ("before-park-races",m.before_park),("after-park-races",m.after_park),("after-poll-races",m.after_poll),("min-entry-exit-tsc",m.min),
            ("max-entry-exit-tsc",m.max),("last-guest-tsc",m.last),
            ("timer-acks",m.timer_acks),("timer-intrs",m.timer_intrs),("ipi-intrs",m.ipi_intrs),
            ("both-source-intrs",m.both_intrs),
            ("voluntary-timer-acks",m.voluntary_timer_acks),
            ("timer-delivered",m.timer_delivered),("timer-eois",m.timer_eois),
            ("timer-programs",m.timer_programs),("timer-hlts",m.timer_hlts),("timer-wakes",m.timer_wakes),
            ("spin-intrs",m.spin_intrs),("spin-progress",m.spin_progress),
            ("spin-no-progress",m.spin_no_progress),("spin-sessions",m.spin_sessions),
            ("idle-returns",m.idle_returns),("idle-timer-acks",m.idle_timer_acks),("idle-ipi-acks",m.idle_ipi_acks),
            ("idle-timer-witnesses",host_smp::idle_timer_wakes(id)),("idle-ipi-witnesses",host_smp::idle_ipi_wakes(id)),
            ("watchdog-only-wakes",m.watchdogs),("min-deadline-lateness-tsc",m.min_late),
            ("max-deadline-lateness-tsc",m.max_late),("min-host-idle-tsc",m.min_idle),("max-host-idle-tsc",m.max_idle),
            ("timer-cancel-races",host_smp::timer_cancel_races(id)),("timer-cancel-active-races",host_smp::timer_cancel_active_races(id))] {
            print(if id==0{"CONCURRENT cpu0-"}else{"CONCURRENT cpu1-"});print(name);print("=");hex(value);
        }
    }
    assert_eq!(memory::concurrent_word(4),8);assert_eq!(memory::concurrent_word(5),8);
    assert!(memory::concurrent_word(9)>=memory::concurrent_word(8));
    print("PASS concurrent-smp=8 two-host-two-guest-xapic-x2apic\n");
    print("PASS concurrent-ipi=64 handler-eoi-iretq-coalescing-running-simultaneous-hlt\n");
    print("PASS concurrent-startup=1 target-owned-init-sipi-real16-protected32-long64\n");
    print("PASS concurrent-hlt=16 bidirectional-before-park-after-park-after-empty-poll\n");
    print("PASS concurrent-entry-race=1 publish-after-drain-before-vmrun\n");
    print("PASS concurrent-ownership=2 vmcb-gpr-xstate-fsbase-clock-stack-host-restored\n");
    print("PASS concurrent-clock=8 per-cpu-monotonic-ordered-shared-handoff\n");
    print("PASS concurrent-timer=32 two-cpu-xapic-x2apic-spin-hlt-eoi-iretq\n");
    print("PASS concurrent-idle=32 guest-hlt-sessions-actual-host-sti-hlt-cli-source-witness\n");
    print("PASS concurrent-watchdog=4 host-wake-without-guest-delivery\n");
}
unsafe fn drive(id:usize,state:&xstate::State,clock:&clock::State) {
    assert_eq!(host_smp::current_cpu(),id);
    let debug0:u64;let debug1:u64;let debug2:u64;let debug3:u64;
    unsafe {asm!("mov {},dr0",out(reg)debug0,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr1",out(reg)debug1,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr2",out(reg)debug2,options(nomem,nostack,preserves_flags));
        asm!("mov {},dr3",out(reg)debug3,options(nomem,nostack,preserves_flags));}
    assert_eq!([debug0,debug1,debug2,debug3],[0;4]);
    let prepared=unsafe{&*ptr::addr_of!(SHARED).read().prepared};
    let control=unsafe{ptr::addr_of_mut!(CONTROLS).cast::<Vmcb>().add(id)};
    let extended=unsafe{&mut *ptr::addr_of_mut!(EXTENDED).cast::<xstate::GuestState>().add(id)};
    let metrics=unsafe{&mut *ptr::addr_of_mut!(METRICS).cast::<Metrics>().add(id)};
    unsafe{state.reset_owned(extended,id)};
    let mut frame=GuestRegisters::default();
    let mut apic=FixtureApic::admit_fixed_cpu(LocalApic::admit_enabled(),
        if id==0{0xfee00900}else{0xfee00800},id as u8).unwrap();
    let signature=svmvisor_hypervisor::svm::emulation::cpuid(1,0)[0];
    if id==1 {
        let mut startup=StartupState::Cold;
        bounded_wait(|| {
            let mut target=IpiTarget {apic:&mut apic,vmcb:unsafe{&mut *control},
                frame:&mut frame,startup:&mut startup,signature};
            metrics.startup_applied+=MAILBOXES[id].drain_startup(&mut target).unwrap() as u64;
            metrics.acks+=unsafe{host_smp::acknowledge()};
            startup==StartupState::Running
        });
        assert_eq!(metrics.startup_applied,2);
    }
    let mut apic=ScheduledApic::admit(apic,ClockRate::new(1,1024).unwrap(),unsafe{clock.sample()}).unwrap();
    #[cfg(feature="uefi-smp")]
    unsafe {
        let rsp:u64;let save_low:u32;let save_high:u32;
        asm!("mov {},rsp",out(reg)rsp,options(nomem,nostack));
        asm!("rdmsr",in("ecx")0xc0010117u32,out("eax")save_low,out("edx")save_high,options(nomem,nostack));
        let ranges=[(rsp,1),(u64::from(save_low)|(u64::from(save_high)<<32),4096),
            (control as u64,core::mem::size_of::<Vmcb>() as u64),
            (extended as *const _ as u64,core::mem::size_of_val(extended) as u64),
            (ptr::addr_of!(frame) as u64,core::mem::size_of_val(&frame) as u64),
            (ptr::addr_of!(apic) as u64,core::mem::size_of_val(&apic) as u64),
            state.host_backing(),
            (clock as *const _ as u64,core::mem::size_of_val(clock) as u64)];
        ptr::addr_of_mut!(OWNED_RANGES).cast::<[(u64,u64);8]>().add(id).write(ranges);
    }
    let mut race=false;
    let spin=address(ptr::addr_of!(guest_concurrent_spin))..address(ptr::addr_of!(guest_concurrent_spin_end));
    let mut spin_progress=0;
    let mut spin_intrs=0;
    let mut deadline=None;
    for _ in 0..4096 {
        // Only this CPU mutates its local controller and VMCB. Drain before arm;
        // every later remote publication independently sends a physical kick.
        if !apic.apic().controller().delivery_armed(){
            if apic.apic().controller().software_enabled(){
                apic.drain_mailbox(unsafe{&*control},&MAILBOXES[id]).unwrap();
            }else{
                // INIT leaves SVR disabled. Guest readiness follows its own SVR
                // write, so no fixed transport is admitted during this phase.
                assert_eq!(id,1);assert_eq!(metrics.queries,0);
                assert!(!MAILBOXES[id].pending());
            }
        }
        if race {
            assert!(!MAILBOXES[id].pending());
            RACE_READY.store(true,Ordering::Release);
            bounded_wait(||MAILBOXES[id].pending());
            metrics.race+=1;race=false;
        }
        if !apic.apic().controller().delivery_armed(){
            if let Some(vector)=apic.arm_pending(unsafe{&mut *control}).unwrap(){
                if vector==0x51 {
                    assert!(spin.contains(&unsafe{&*control}.guest_rip()));
                    assert!(spin_intrs>0 && spin_progress>0,"timer session lacks physical running progress");
                    let late=apic.last_sample().ticks.checked_sub(deadline.unwrap()).unwrap();
                    metrics.min_late=metrics.min_late.min(late);metrics.max_late=metrics.max_late.max(late);
                }else{assert_eq!(vector,0x50);}
            }
        }
        let armed_control=unsafe{&*control}.virtual_interrupt_control();
        let before=unsafe{clock.sample()}.ticks;
        let old_cr8:u64;let new_cr8:u64;let second_cr8:u64;let stopped_host_rflags:u64;
        unsafe{asm!("mov {},cr8",out(reg)old_cr8,options(nomem,nostack,preserves_flags));
            host_smp::arm_timer();
            state.run_owned(control,&mut frame,0,clock,extended);
            // Distinct live output registers retain two immediately adjacent
            // architectural reads before any Rust, CPUID, or MMIO diagnostic.
            asm!("mov {first},cr8", "mov {second},cr8", "pushfq", "pop {flags}",
                first=out(reg)new_cr8,second=out(reg)second_cr8,
                flags=out(reg)stopped_host_rflags,options(preserves_flags));}
        if old_cr8 != new_cr8 {
            // Failure-only bounded evidence: this CPU owns the stopped VMCB.
            // Preserve the restoration assertion and the successful hot path.
            let stopped=unsafe{&*control};
            let snapshot=stopped.exit_snapshot();
            for (name,value) in [("cpu",id as u64),
                ("cpuid1-ebx",core::arch::x86_64::__cpuid(1).ebx as u64),
                ("old-cr8",old_cr8),("new-cr8",new_cr8),
                ("second-cr8",second_cr8),("stopped-host-rflags",stopped_host_rflags),
                ("host-tpr",unsafe{host_smp::diagnostic_tpr()} as u64),
                ("exit",snapshot.code),("rip",snapshot.rip),
                ("int-control",stopped.virtual_interrupt_control()),
                ("generation",frame.r13),("phase",frame.r11),
                ("prior-entries",metrics.entries)] {
                print("CONCURRENT CR8 FAILURE ");print(name);print("=");hex(value);
            }
        }
        assert_eq!(old_cr8,new_cr8);
        let span=unsafe{clock.sample()}.ticks.checked_sub(before).unwrap();
        metrics.entries+=1;metrics.min=metrics.min.min(span);metrics.max=metrics.max.max(span);
        let v=unsafe{&mut *control};
        let snap=v.exit_snapshot();
        let event_injection=u64::from_le_bytes(v.bytes()[0xa8..0xb0].try_into().unwrap());
        let sources=unsafe{host_smp::cancel_and_acknowledge()};
        let acknowledged=sources.ipi+sources.timer;
        metrics.acks+=sources.ipi;metrics.timer_acks+=sources.timer;
        if snap.code!=0x60{metrics.voluntary_timer_acks+=sources.timer;}
        let saved_rip=v.guest_rip();let saved_flags=flags(v);let saved_frame=frame;
        v.clear_event_injection_after_exit().unwrap();
        if apic.apic().controller().delivery_armed() {
            let observed=apic.settle_after_exit(v,unsafe{clock.sample()}).map(|outcome|outcome.consumed);
            let failed=match &observed {Err(_)=>true,Ok(Some(vector))=>*vector!=0x50 && *vector!=0x51,Ok(None)=>false};
            if failed {
                // No successful-loop logging; keep the original refusal/assertion.
                print("CONCURRENT OBSERVE FAILURE result=");
                let _=core::fmt::write(&mut FailureWriter,format_args!("{:?}\n",observed));
                for (name,value) in [("cpu",id as u64),("exit",snap.code),("rip",snap.rip),("rsp",v.guest_rsp()),
                    ("generation",frame.r13),("phase",frame.r11),
                    ("int-control-before",armed_control),("int-control-after",v.virtual_interrupt_control()),
                    ("event-injection",event_injection),
                    ("exit-int-info",u64::from_le_bytes(v.bytes()[0x88..0x90].try_into().unwrap())),
                    ("guest-rflags",flags(v)),("local-tpr",apic.apic().controller().task_priority() as u64),
                    ("old-cr8",old_cr8),("new-cr8",new_cr8),
                    ("second-cr8",second_cr8),("stopped-host-rflags",stopped_host_rflags),
                    ("host-tpr",unsafe{host_smp::diagnostic_tpr()} as u64),
                    ("acknowledged",acknowledged),("entries",metrics.entries)] {
                    print("CONCURRENT OBSERVE FAILURE ");print(name);print("=");hex(value);
                }
            }
            if let Some(vector)=observed.unwrap() {
                if vector==0x50{metrics.delivered+=1;}else{assert_eq!(vector,0x51);metrics.timer_delivered+=1;}
            }
        }else{
            // Every stopped dispatch accounts for elapsed source time before
            // instruction completion, including unarmed startup and HLT exits.
            apic.service(v,unsafe{clock.sample()}).unwrap();
        }
        if snap.code==0x60 {
            assert!(acknowledged>0,"INTR without acknowledged owned F0/F1 source");
            assert!(sources.timer<=1);
            metrics.intrs+=1;metrics.timer_intrs+=sources.timer;metrics.ipi_intrs+=u64::from(sources.ipi>0);
            metrics.both_intrs+=u64::from(sources.timer>0 && sources.ipi>0);
            assert_eq!((v.guest_rip(),flags(v)),(saved_rip,saved_flags));assert_eq!(frame,saved_frame);
            if spin.contains(&snap.rip){
                assert_eq!(frame.r11,6);assert!(frame.rbp>=spin_progress);
                // Require actual F0 evidence for every successful timer session;
                // an unrelated F1 is a real exit but not timer-preemption proof.
                if sources.timer>0 {
                    spin_intrs+=1;metrics.spin_intrs+=1;
                    if frame.rbp==spin_progress{metrics.spin_no_progress+=1;}
                    metrics.spin_progress+=frame.rbp-spin_progress;spin_progress=frame.rbp;
                }
            }
            if sources.ipi>0 && (address(ptr::addr_of!(guest_concurrent_wait_start))..address(ptr::addr_of!(guest_concurrent_wait_end))).contains(&snap.rip) {
                assert_ne!(flags(v)&0x200,0);metrics.running+=1;
            }
            continue;
        }
        if snap.code==0x72 {
            assert_eq!(id,1);assert_eq!(metrics.real,0);assert_eq!(snap.rip,0);
            assert_eq!(&v.bytes()[0x410..0x412],&0x100u16.to_le_bytes());
            assert_eq!(word(v,0x418),0x1000);assert_eq!(word(v,0x558),0x10);
            assert_eq!(word(v,0x548),0);assert_eq!(word(v,0x550),0);
            assert_eq!(word(v,0x4d0),0x1000);assert_eq!(v.guest_rsp(),0);assert_eq!(v.guest_rax(),0);
            assert_eq!(frame,GuestRegisters{rdx:u64::from(signature),..GuestRegisters::default()});
            assert_eq!(word(v,0x448),0);assert_eq!(word(v,0x458),0);
            let instruction=unsafe{memory::installed_instruction(word(v,0x418)+snap.rip,2)};
            assert_eq!(instruction,&[0x0f,0xa2]);
            assert_eq!(handle_exit_with_instruction(snap,v,&mut frame,instruction).unwrap(),DispatchOutcome::ResumePrepared);
            assert_eq!(v.guest_rip(),2);metrics.real+=1;continue;
        }
        let instruction=unsafe{memory::installed_instruction(snap.rip,if snap.code==0x81{3}else if snap.code==0x78{1}else{2})};
        if snap.code==0x78 {
            if snap.rip==address(ptr::addr_of!(guest_concurrent_timer_hlt)) {
                assert_eq!((frame.r11,frame.r12),(7,5));assert_eq!(instruction,&[0xf4]);
                assert!(!apic.apic().controller().pending(0x51),"timer expired before guest park");
                assert!(deadline.is_some());metrics.timer_hlts+=1;
                apic.park_hlt(v,instruction).unwrap();
                let mut ready=false;
                for _ in 0..4096 {
                    unsafe{idle_once(v,&frame,metrics)};
                    apic.drain_mailbox(v,&MAILBOXES[id]).unwrap();
                    assert!(!apic.apic().controller().pending(0x50));
                    if let WaitOutcome::Ready{vector}=apic.poll_halted(v,unsafe{clock.sample()}).unwrap(){
                        assert_eq!(vector,0x51);assert_eq!(v.guest_rip(),snap.rip+1);
                        let late=apic.last_sample().ticks.checked_sub(deadline.unwrap()).unwrap();
                        metrics.min_late=metrics.min_late.min(late);metrics.max_late=metrics.max_late.max(late);
                        metrics.timer_wakes+=1;ready=true;break;
                    }
                }
                assert!(ready,"timer host idle budget");continue;
            }
            assert_eq!(snap.rip,address(ptr::addr_of!(guest_concurrent_hlt)));
            assert_eq!(frame.r11,if id==1{3}else{4});assert_eq!(frame.r12,3);
            assert_ne!(flags(v)&0x200,0);assert_eq!(instruction,&[0xf4]);
            metrics.hlts+=1;
            apic.drain_mailbox(v,&MAILBOXES[id]).unwrap();
            assert!(!MAILBOXES[id].pending());assert!(!apic.apic().controller().pending(0x50));
            let mode=(frame.r13-1)%3;
            if mode==0 {
                // Publication occurs after the final empty drain, before park.
                let saved=digest(v);let saved_frame=frame;
                PARK_READY[id].store(frame.r13,Ordering::Release);
                bounded_wait(||MAILBOXES[id].pending());
                assert_eq!(digest(v),saved);assert_eq!(frame,saved_frame);
                metrics.before_park+=1;
            }
            apic.park_hlt(v,instruction).unwrap();
            if mode!=0 {
                if mode==2 {
                    assert_eq!(apic.poll_halted(v,unsafe{clock.sample()}).unwrap(),WaitOutcome::Parked);
                    let mut witnessed=false;
                    for _ in 0..1024 {
                        let sources=unsafe{idle_once(v,&frame,metrics)};
                        assert!(!MAILBOXES[id].pending());
                        assert_eq!(apic.poll_halted(v,unsafe{clock.sample()}).unwrap(),WaitOutcome::Parked);
                        assert!(!apic.apic().controller().pending(0x50));
                        assert!(!apic.apic().controller().pending(0x51));
                        if sources.timer>0 && sources.ipi==0{witnessed=true;break;}
                    }
                    assert!(witnessed,"watchdog-only idle budget");metrics.watchdogs+=1;
                    metrics.after_poll+=1;
                }else{metrics.after_park+=1;}
                PARK_READY[id].store(frame.r13,Ordering::Release);
            }
            // After-park publication races actual sleep with no mailbox wait.
            // STI's shadow covers HLT; a watchdog can return the host while the
            // guest remains halted. Only the re-drained APIC owner retires HLT.
            let mut ready=false;
            for _ in 0..4096 {
                unsafe{idle_once(v,&frame,metrics)};
                apic.drain_mailbox(v,&MAILBOXES[id]).unwrap();
                match apic.poll_halted(v,unsafe{clock.sample()}).unwrap(){
                    WaitOutcome::Ready{vector}=>{
                        assert_eq!(vector,0x50);assert_eq!(v.guest_rip(),snap.rip+1);
                        metrics.wakes+=1;ready=true;break;
                    }
                    WaitOutcome::Parked=>assert_eq!(v.guest_rip(),snap.rip),
                }
            }
            assert!(ready,"IPI host idle budget");
            continue;
        }
        if snap.code==0x81 {
            let call=v.guest_rax();assert!(call<=1,"concurrent guest failure");
            if id==1 && metrics.long==0 {
                assert_eq!(snap.rip,address(ptr::addr_of!(guest_concurrent_long))+2);
                assert_eq!(call,0);assert_eq!(metrics.real,1);assert_eq!(frame.r9,1);
                assert_eq!(u16::from_le_bytes(v.bytes()[0x412..0x414].try_into().unwrap())&0x600,0x200);
                assert_eq!(word(v,0x558),0x80010033);assert_eq!(word(v,0x548),0x620);
                assert_eq!(word(v,0x550),prepared.guest_cr3);assert_eq!(word(v,0x4d0)&0x1d00,0x1d00);
                assert_eq!(v.guest_rsp(),0xb000);
                assert_eq!(handle_exit_with_instruction(snap,v,&mut frame,instruction).unwrap(),DispatchOutcome::ResumePrepared);
                unsafe{execution::permissions(control,&*ptr::addr_of!(SHARED).read().caps,Some(ptr::addr_of!(MAP)))};
                metrics.long+=1;continue;
            }
            let outcome=handle_exit_with_instruction(snap,v,&mut frame,instruction).unwrap();
            if call==1 {
                assert_eq!(outcome,DispatchOutcome::Stop(StopReason::Requested));
                assert_eq!(frame.r13,9);assert_eq!(frame.r12,6);
                assert!(!apic.apic().controller().pending(0x50));assert!(!apic.apic().controller().in_service(0x50));
                assert!(!apic.apic().controller().pending(0x51));assert!(!apic.apic().controller().in_service(0x51));
                assert_eq!(apic.apic().controller().timer_remaining(),0);
                return;
            }
            assert_eq!(outcome,DispatchOutcome::ResumePrepared);
            assert_eq!(frame.r10,0x73564d4350553030+id as u64);
            assert_eq!(frame.r8,0x83764d4350553030+id as u64);
            assert_eq!(frame.rsi,0xd000+id as u64*8);
            assert_eq!(frame.r14,0xd000+(1-id) as u64*8);
            assert_eq!(frame.r9,id as u64);assert_eq!(frame.r15,u64::from(frame.r13>=5));
            assert_eq!(frame.r13,metrics.queries/8+1);assert_eq!(frame.r11,metrics.queries%8);
            assert_eq!(frame.r12,if frame.r11<=3{frame.r11}else if frame.r11==4{3+id as u64}else if frame.r11==5{4}else{frame.r11-1});
            assert_eq!(v.guest_rsp(),if id==0{0x9000}else{0xb000});
            assert!(frame.rbp>=metrics.last);metrics.last=frame.rbp;
            if frame.r11==0 {
                apic.drain_mailbox(v,&MAILBOXES[id]).unwrap();
                assert!(apic.apic().controller().pending(0x50));assert_eq!(flags(v)&0x200,0);
            } else {
                assert!(!apic.apic().controller().pending(0x50));assert!(!apic.apic().controller().in_service(0x50));
            }
            if frame.r11>=6 {
                assert!(!apic.apic().controller().pending(0x51));assert!(!apic.apic().controller().in_service(0x51));
                assert_eq!(apic.apic().controller().timer_remaining(),0);
                if frame.r11==6 {
                    assert!(spin_intrs>0 && spin_progress>0);metrics.spin_sessions+=1;
                }
            }
            metrics.queries+=1;
            if frame.r13==1 && frame.r11==1 {
                if id==1{race=true}else{bounded_wait(||RACE_READY.load(Ordering::Acquire));}
            }
            continue;
        }
        assert!(snap.code==0x7c||snap.code==0x400,"unsupported concurrent VMEXIT");
        let eoi=if snap.code==0x7c{frame.rcx==0x80b}else{frame.rbx==0xc0b0};
        let timer_program=if snap.code==0x7c{frame.rcx==0x838}else{frame.rbx==0xc380};
        let eoi_vector=if eoi{apic.apic().controller().eoi_target()}else{None};
        let low_icr=if snap.code==0x7c{frame.rcx==0x830}else{frame.rbx==0xc300};
        let startup_command=low_icr && v.guest_rax()&0x700!=0;
        if low_icr && !startup_command && ((id==0 && frame.r11==3)||(id==1 && frame.r11==4)) {
            bounded_wait(||PARK_READY[1-id].load(Ordering::Acquire)==frame.r13);
        }
        let mut target=MailboxTarget::new(&MAILBOXES[1-id]);
        if snap.code==0x7c {
            apic.handle_msr_with_mailbox(v,&mut frame,instruction,&mut target).unwrap();
        }else{
            apic.handle_mmio_with_mailbox(v,&mut frame,instruction,prepared.mmio_mapping.as_ref().unwrap(),&mut target).unwrap();
        }
        if target.published(){if startup_command{metrics.startup_sent+=1;}else{metrics.sent+=1;}unsafe{host_smp::kick(1-id)};}
        if timer_program {
            assert_eq!(v.guest_rax(),16384);assert!(frame.r11==6||frame.r11==7);
            deadline=apic.deadline().unwrap();assert!(deadline.is_some());metrics.timer_programs+=1;
            if frame.r11==6{spin_progress=0;spin_intrs=0;}
        }
        if eoi {
            match eoi_vector {Some(0x50)=>metrics.eois+=1,Some(0x51)=>metrics.timer_eois+=1,_=>panic!("EOI without owned vector")}
            assert!(!apic.apic().controller().in_service(0x50));assert!(!apic.apic().controller().in_service(0x51));
        }
    }
    panic!("concurrent VMEXIT budget");
}
