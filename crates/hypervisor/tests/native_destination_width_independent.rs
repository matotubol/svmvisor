use svmvisor_hypervisor::svm::ipi::{NativeIcr, NativeStartupMailbox, NativeStartupCommand as C, NativeDestinationMode as M, try_lock_routes};
fn forbidden(_:u16,_:Option<u64>)->u32 {panic!("physical ICR escaped")}
#[test]
fn destination16_requires_normalized_physical_modes_for_guest_exact_ids() {
    for modes in [[M::ExtendedXApic8;3], [M::ExtendedXApic8,M::ExtendedXApic4,M::ExtendedXApic8], [M::ExtendedXApic4,M::ExtendedXApic8,M::ExtendedXApic8], [M::ExtendedXApic8,M::ExtendedXApic8,M::ExtendedXApic4]] {
        let boxes=[NativeStartupMailbox::new(0),NativeStartupMailbox::new(16),NativeStartupMailbox::new(32)];
        for b in &boxes {b.mark_running();}
        {let routes=try_lock_routes(&boxes).unwrap(); for (slot,mode) in modes.into_iter().enumerate() {routes.prepare_destination_mode(slot,mode).unwrap().commit_destination_mode();}}
        let mut owner=NativeIcr::admit(0,&[0,16,32]).unwrap();owner.enable_startup(0).unwrap();
        owner.xapic_access(0xfee00900,&boxes,0x310,Some(16<<24),forbidden,|_|panic!()).unwrap();
        let unique=modes.iter().all(|m| *m==M::ExtendedXApic8);
        let mut kicks=Vec::new();
        let result=owner.xapic_access(0xfee00900,&boxes,0x300,Some(0xc500),forbidden,|id|kicks.push(id));
        assert_eq!(result.is_ok(),unique,"{modes:?}");
        assert_eq!(kicks,if unique {vec![16]} else {vec![]});
        assert_eq!(boxes[0].peek(),None);assert_eq!(boxes[2].peek(),None);
        assert_eq!(boxes[1].peek(),if unique{Some(C::Init)}else{None});
        assert_eq!(owner.xapic_access(0xfee00900,&boxes,0x300,None,|_,_|0,|_|panic!()),Ok(if unique{0xc500}else{0}));
    }
}
