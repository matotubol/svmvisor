use svmvisor_hypervisor::{
    memory::address::EncryptionState,
    arch::x86_64::capabilities::{CapabilityEvidence, CpuVendor, EvidenceFlag, OptionalFeatures},
    svm::exit::ExitSnapshot,
    boot::probe::*,
    boot::xstate::*,
    arch::x86_64::xstate::*,
};

static SAVED: XstateArea = XstateArea::new();

#[test]
fn typed_plan_and_original_image_validation_cannot_be_skipped() {
    let e = evidence();
    let mut other = e;
    other.original.cr0 ^= 1 << 16;
    assert!(matches!(
        Transaction::prepare(
            e,
            memory(e),
            captured(e),
            plan(other),
            &SAVED,
            0xffbf,
            0x1000
        ),
        Err(Error::Observation)
    ));
    let mut bad = XstateArea::new();
    bad.bytes_mut()[520] = 1;
    assert!(matches!(
        Transaction::prepare(e, memory(e), captured(e), plan(e), &bad, 0xffbf, 0x1000),
        Err(Error::Xstate(_))
    ));
}

#[test]
fn gif_cannot_be_enabled_until_host_environment_and_image_reported_restored() {
    let mut tx = prepared();
    arm(&mut tx);
    return_guest(&mut tx, 0x81);
    tx.begin_restore_hsave().unwrap();
    tx.observe_hsave_restored(masked_state()).unwrap();
    assert_eq!(tx.begin_restore_gif(), Err(Error::Order));
    tx.begin_restore_pre_gif_host().unwrap();
    let ack = || AdapterRestoreAcknowledgement {
        lease: evidence().original.lease,
    };
    assert_eq!(
        tx.observe_pre_gif_host_restored(masked_state(), SAVED.bytes(), ack()),
        Err(Error::Observation)
    );
    let mut if_enabled = pre_gif_state();
    if_enabled.rflags |= 1 << 9;
    assert_eq!(
        tx.observe_pre_gif_host_restored(if_enabled, SAVED.bytes(), ack()),
        Err(Error::Observation)
    );
    assert_eq!(
        tx.observe_pre_gif_host_restored(pre_gif_state(), &[1; 4096], ack()),
        Err(Error::SavedImage)
    );
    assert_eq!(tx.begin_restore_gif(), Err(Error::Order));
    tx.observe_pre_gif_host_restored(pre_gif_state(), SAVED.bytes(), ack())
        .unwrap();
    tx.begin_restore_gif().unwrap();
}
fn evidence() -> AdmissionEvidence {
    let lease = CpuLease {
        cpu_id: 7,
        generation: 19,
    };
    AdmissionEvidence {
        capabilities: CapabilityEvidence {
            vendor: CpuVendor::Amd,
            svm: EvidenceFlag::Set,
            nested_paging: EvidenceFlag::Set,
            svm_revision: Some(1),
            asid_count: Some(8),
            physical_address_bits: Some(48),
            vm_cr_svmdis: EvidenceFlag::Clear,
            hypervisor_present: EvidenceFlag::Clear,
            encryption: EncryptionState::Unencrypted {
                encryption_bit: None,
            },
            optional: OptionalFeatures::default(),
        },
        context: Context::FirmwareApplication,
        tpl: 4,
        privilege_level: 0,
        active_processors: 1,
        ownership: Ownership::Exclusive(lease),
        gif: GifEvidence::EstablishedSet,
        original: HostObservation {
            lease,
            efer: 0x4500,
            vm_hsave_pa: 0,
            cr0: 0x80000039,
            cr3: 0x800000,
            cr4: 0x406f8,
            rflags: 0x202,
            dr7: 0x400,
            xcr0: Some(3),
            xss: None,
        },
    }
}
fn memory(e: AdmissionEvidence) -> MemoryEvidence {
    MemoryEvidence {
        lease: e.original.lease,
        allocation_base: 0x200000,
        allocation_bytes: 0x8000,
        hsave_pa: 0x200000,
        vmcb_pa: 0x201000,
    }
}
fn captured(e: AdmissionEvidence) -> HostObservation {
    HostObservation {
        efer: e.original.efer & !(1 << 14),
        cr0: e.original.cr0 & !12,
        ..e.original
    }
}
fn prepared() -> Transaction<'static> {
    let e = evidence();
    Transaction::prepare(e, memory(e), captured(e), plan(e), &SAVED, 0xffbf, 0x1000).unwrap()
}
fn armed_state(hsave: u64) -> HostObservation {
    HostObservation {
        efer: captured(evidence()).efer | EFER_SVME,
        vm_hsave_pa: hsave,
        ..captured(evidence())
    }
}
fn arm(tx: &mut Transaction<'_>) {
    tx.begin_enable().unwrap();
    tx.observe_enabled(armed_state(0)).unwrap();
    tx.begin_install_hsave().unwrap();
    tx.observe_armed(armed_state(0x200000)).unwrap();
}
fn return_guest(tx: &mut Transaction<'_>, code: u64) {
    tx.begin_entry().unwrap();
    tx.observe_exit(
        evidence().original.lease,
        ExitSnapshot {
            code,
            info1: 0,
            info2: 0,
            rip: 0x1000,
            nrip: 0,
        },
    )
    .unwrap();
}
fn restore(tx: &mut Transaction<'_>) {
    tx.begin_restore_hsave().unwrap();
    tx.observe_hsave_restored(masked_state()).unwrap();
    pre_gif(tx);
    tx.begin_restore_gif().unwrap();
    tx.acknowledge_stgi(pre_gif_state()).unwrap();
    tx.begin_restore_host().unwrap();
    tx.observe_host_restored(evidence().original, SAVED.bytes())
        .unwrap();
}

#[test]
fn completed_order_is_distinct_from_preentry_abort() {
    let mut tx = prepared();
    arm(&mut tx);
    return_guest(&mut tx, 0x81);
    restore(&mut tx);
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Ok(Outcome::ExpectedExitAndRestorationObserved)
    );
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Err(Error::Order)
    );
    let mut aborted = prepared();
    aborted
        .observe_abort_restored(evidence().original, SAVED.bytes())
        .unwrap();
    assert_eq!(
        aborted.release_memory(evidence().original.lease),
        Ok(Outcome::AbortedBeforeEntry)
    );
}

#[test]
fn unknown_or_existing_ownership_and_unsupported_context_are_rejected() {
    for change in 0..10 {
        let mut e = evidence();
        match change {
            0 => e.ownership = Ownership::Unknown,
            1 => e.original.efer |= EFER_SVME,
            2 => e.original.vm_hsave_pa = 0x400000,
            3 => e.context = Context::AfterExitBootServices,
            4 => e.context = Context::FirmwareCallback,
            5 => e.tpl = 16,
            6 => e.active_processors = 2,
            7 => e.gif = GifEvidence::Unknown,
            8 => e.capabilities.hypervisor_present = EvidenceFlag::Unknown,
            _ => e.privilege_level = 3,
        }
        assert!(
            Transaction::prepare(e, memory(e), captured(e), plan(e), &SAVED, 0xffbf, 0x1000)
                .is_err(),
            "change {change}"
        );
    }
}

#[test]
fn memory_must_belong_to_this_attempt_and_contain_distinct_aligned_pages() {
    for change in 0..5 {
        let e = evidence();
        let mut m = memory(e);
        match change {
            0 => m.vmcb_pa = m.hsave_pa,
            1 => m.hsave_pa += 1,
            2 => m.vmcb_pa = m.allocation_base + m.allocation_bytes,
            3 => m.lease.generation += 1,
            _ => m.allocation_bytes = 4096,
        }
        assert!(Transaction::prepare(e, m, captured(e), plan(e), &SAVED, 0xffbf, 0x1000).is_err());
    }
}

#[test]
fn failed_enable_and_install_readbacks_require_exact_rollback() {
    for during_install in [false, true] {
        let mut tx = prepared();
        tx.begin_enable().unwrap();
        if during_install {
            tx.observe_enabled(armed_state(0)).unwrap();
            tx.begin_install_hsave().unwrap();
            assert_eq!(
                tx.observe_armed(armed_state(0x400000)),
                Err(Error::Observation)
            );
        } else {
            assert_eq!(
                tx.observe_enabled(captured(evidence())),
                Err(Error::Observation)
            );
        }
        assert_eq!(
            tx.release_memory(evidence().original.lease),
            Err(Error::Order)
        );
        assert_eq!(
            tx.observe_abort_restored(armed_state(0), SAVED.bytes()),
            Err(Error::Observation)
        );
        tx.observe_abort_restored(evidence().original, SAVED.bytes())
            .unwrap();
        assert_eq!(
            tx.release_memory(evidence().original.lease),
            Ok(Outcome::AbortedBeforeEntry)
        );
    }
}

#[test]
fn entry_attempt_without_observed_return_never_allows_cleanup_or_free() {
    let mut tx = prepared();
    arm(&mut tx);
    tx.begin_entry().unwrap();
    assert_eq!(
        tx.observe_abort_restored(evidence().original, SAVED.bytes()),
        Err(Error::Order)
    );
    assert_eq!(tx.begin_restore_hsave(), Err(Error::Order));
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Err(Error::Order)
    );
}

#[test]
fn gif_acknowledgement_must_precede_svme_clear_and_final_restore() {
    let mut tx = prepared();
    arm(&mut tx);
    return_guest(&mut tx, 0x81);
    assert_eq!(tx.begin_restore_host(), Err(Error::Order));
    tx.begin_restore_hsave().unwrap();
    assert_eq!(
        tx.observe_hsave_restored(armed_state(0x200000)),
        Err(Error::Observation)
    );
    tx.observe_hsave_restored(masked_state()).unwrap();
    assert_eq!(tx.begin_restore_host(), Err(Error::Order));
    pre_gif(&mut tx);
    tx.begin_restore_gif().unwrap();
    assert_eq!(
        tx.acknowledge_stgi(captured(evidence())),
        Err(Error::Observation)
    );
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Err(Error::Order)
    );
    tx.acknowledge_stgi(pre_gif_state()).unwrap();
    tx.begin_restore_host().unwrap();
    assert_eq!(
        tx.observe_host_restored(armed_state(0), SAVED.bytes()),
        Err(Error::Observation)
    );
    tx.observe_host_restored(evidence().original, SAVED.bytes())
        .unwrap();
}

#[test]
fn restoration_checks_control_values_cpu_lease_and_saved_bytes() {
    let mut tx = prepared();
    arm(&mut tx);
    return_guest(&mut tx, 0x81);
    tx.begin_restore_hsave().unwrap();
    tx.observe_hsave_restored(masked_state()).unwrap();
    pre_gif(&mut tx);
    tx.begin_restore_gif().unwrap();
    tx.acknowledge_stgi(pre_gif_state()).unwrap();
    tx.begin_restore_host().unwrap();
    for change in 0..9 {
        let mut observed = evidence().original;
        match change {
            0 => observed.lease.cpu_id += 1,
            1 => observed.lease.generation += 1,
            2 => observed.cr0 ^= 8,
            3 => observed.cr3 ^= 4096,
            4 => observed.cr4 ^= 0x40000,
            5 => observed.rflags ^= 0x200,
            6 => observed.dr7 ^= 1,
            7 => observed.xcr0 = Some(7),
            _ => observed.xss = Some(1),
        }
        assert_eq!(
            tx.observe_host_restored(observed, SAVED.bytes()),
            Err(Error::Observation)
        );
    }
    assert_eq!(
        tx.observe_host_restored(evidence().original, &[0; 512]),
        Err(Error::SavedImage)
    );
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Err(Error::Order)
    );
    tx.observe_host_restored(evidence().original, SAVED.bytes())
        .unwrap();
}

#[test]
fn unexpected_exit_can_be_restored_but_cannot_report_expected_completion() {
    let mut tx = prepared();
    arm(&mut tx);
    return_guest(&mut tx, u64::MAX);
    restore(&mut tx);
    assert_eq!(
        tx.release_memory(evidence().original.lease),
        Ok(Outcome::UnexpectedExitAndRestorationObserved { code: u64::MAX })
    );
}

#[test]
fn out_of_order_and_wrong_cpu_events_leave_transaction_unadvanced() {
    let mut tx = prepared();
    assert_eq!(tx.begin_entry(), Err(Error::Order));
    assert_eq!(tx.stage(), Stage::Prepared);
    arm(&mut tx);
    tx.begin_entry().unwrap();
    let lease = CpuLease {
        cpu_id: 8,
        ..evidence().original.lease
    };
    assert_eq!(
        tx.observe_exit(
            lease,
            ExitSnapshot {
                code: 0x81,
                info1: 0,
                info2: 0,
                rip: 0x1000,
                nrip: 0
            }
        ),
        Err(Error::Ownership)
    );
    assert_eq!(tx.stage(), Stage::EntryAttempted);
}

fn plan(e: AdmissionEvidence) -> FirmwareXstatePlan {
    FirmwareXstatePlan::validate(FirmwareXstateEvidence {
        max_basic_leaf: 0xd,
        capabilities: XstateCapabilities {
            leaf1_ecx: (1 << 26) | (1 << 27),
            leaf1_edx: 1 | (1 << 23) | (1 << 24) | (1 << 25) | (1 << 26),
            supported_xcr0: 3,
            enabled_size: 576,
            max_size: 576,
            ..XstateCapabilities::default()
        },
        leaf_d1_eax: 0,
        supported_xss: 0,
        original: FirmwareXstateControls {
            cr0: e.original.cr0,
            cr4: e.original.cr4,
            efer: e.original.efer,
            xcr0: e.original.xcr0,
            xss: e.original.xss,
        },
    })
    .unwrap()
}
fn masked_state() -> HostObservation {
    HostObservation {
        rflags: evidence().original.rflags & !(1 << 9),
        ..armed_state(0)
    }
}
fn pre_gif_state() -> HostObservation {
    HostObservation {
        efer: evidence().original.efer | EFER_SVME,
        rflags: evidence().original.rflags & !(1 << 9),
        ..evidence().original
    }
}
fn pre_gif(tx: &mut Transaction<'_>) {
    tx.begin_restore_pre_gif_host().unwrap();
    tx.observe_pre_gif_host_restored(
        pre_gif_state(),
        SAVED.bytes(),
        AdapterRestoreAcknowledgement {
            lease: evidence().original.lease,
        },
    )
    .unwrap();
}
