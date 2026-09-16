use svmvisor_hypervisor::{
    memory::address::{AddressPolicy, EncryptionState, PhysicalRange},
    svm::iommu::*,
};

fn policy() -> AddressPolicy {
    AddressPolicy::new(
        48,
        EncryptionState::Unencrypted {
            encryption_bit: None,
        },
    )
    .unwrap()
}

#[test]
fn dma_merge_preserves_permissions_domain_root_and_owned_interrupt_policy() {
    let guest = DeviceTableEntry([
        0x12345000 | 3 | 4 << 9 | 1 << 61,
        0x1234 | 1 << 39,
        u64::MAX & !(3 << 54 | 1 << 59),
        0,
    ]);
    let owned = DeviceTableEntry([0, 0, 0xe000_0000_5678_0015, 0]);
    let merged = merge_dma(guest, owned, &policy(), 4).unwrap();
    assert_eq!(merged.0, [guest.0[0], guest.0[1], owned.0[2], 0]);
    for forbidden in [1 << 52, 1 << 55, 1 << 56, 1 << 7, 1 << 4] {
        let mut invalid = guest;
        invalid.0[0] |= forbidden;
        assert_eq!(
            merge_dma(invalid, owned, &policy(), 4),
            Err(Error::UnsupportedDmaMode)
        );
    }
    let mut invalid = guest;
    invalid.0[1] |= 1 << 32;
    assert_eq!(
        merge_dma(invalid, owned, &policy(), 4),
        Err(Error::UnsupportedDmaMode)
    );
    let disabled = DeviceTableEntry([0, 0, 0, 0]);
    assert_eq!(merge_dma(disabled, owned, &policy(), 4).unwrap().0[0], 0);
}

#[derive(Default)]
struct Backend {
    commands: Vec<Command>,
    effects: Vec<&'static str>,
    fail: bool,
    completion: Option<(u64, u64)>,
}
impl CommandBackend for Backend {
    fn read_command(&mut self, address: u64) -> Result<Command, Error> {
        self.commands
            .get(((address - 0x1000) / 16) as usize)
            .copied()
            .ok_or(Error::GuestMemory)
    }
    fn read_guest_dte(&mut self, _: u16) -> Result<DeviceTableEntry, Error> {
        Ok(DeviceTableEntry([0x9000 | 3 | 4 << 9 | 3 << 61, 37, 0, 0]))
    }
    fn read_owned_dte(&mut self, _: u16) -> Result<DeviceTableEntry, Error> {
        Ok(DeviceTableEntry([0, 0, 0xabc000, 0]))
    }
    fn publish_dma(&mut self, _: u16, d: DeviceTableEntry) -> Result<(), Error> {
        assert_eq!(d.0[1], 37);
        assert_eq!(d.0[2], 0xabc000);
        self.effects.push("publish");
        Ok(())
    }
    fn execute(&mut self, _: Command) -> Result<(), Error> {
        self.effects.push("execute");
        if self.fail {
            Err(Error::CompletionTimeout)
        } else {
            Ok(())
        }
    }
    fn update_interrupts(&mut self, _: u16) -> Result<(), Error> {
        self.effects.push("route");
        Ok(())
    }
    fn store_completion(&mut self, address: PhysicalRange, value: u64) -> Result<(), Error> {
        self.effects.push("store");
        self.completion = Some((address.base(), value));
        Ok(())
    }
}

#[test]
fn guest_commands_preserve_dma_then_wait_before_guest_completion() {
    let mut backend = Backend {
        commands: vec![
            Command::invalidate_device(2),
            Command([3 << 60 | 37 << 32, 0x9000]),
            Command([1 << 60 | 0x8005, 0xfeed]),
        ],
        ..Default::default()
    };
    let mut queue =
        GuestCommandQueue::new(policy().validate(0x1000, 4096, 4096).unwrap(), 0, 0).unwrap();
    queue.submit_tail(48).unwrap();
    assert_eq!(queue.service(&mut backend, &policy(), 4, 1), Ok(false));
    assert_eq!(queue.head(), 16);
    assert_eq!(queue.service(&mut backend, &policy(), 4, 64), Ok(true));
    assert_eq!(
        backend.effects,
        vec!["route", "publish", "execute", "execute", "execute", "store"]
    );
    assert_eq!(backend.completion, Some((0x8000, 0xfeed)));
}

#[test]
fn failure_retains_failed_head_and_prevents_replay_or_false_completion() {
    let mut backend = Backend {
        commands: vec![Command::invalidate_device(2)],
        fail: true,
        ..Default::default()
    };
    let mut queue =
        GuestCommandQueue::new(policy().validate(0x1000, 4096, 4096).unwrap(), 0, 16).unwrap();
    assert_eq!(
        queue.service(&mut backend, &policy(), 4, 64),
        Err(Error::CompletionTimeout)
    );
    assert_eq!(queue.head(), 0);
    assert!(queue.poisoned());
    assert_eq!(backend.completion, None);
    assert_eq!(
        queue.service(&mut backend, &policy(), 4, 64),
        Err(Error::Poisoned)
    );
    assert_eq!(backend.effects.len(), 3);
}

#[test]
fn tail_wrap_and_overlap_are_checked_without_changing_queue() {
    let range = policy().validate(0x1000, 4096, 4096).unwrap();
    let mut queue = GuestCommandQueue::new(range, 32, 4080).unwrap();
    assert_eq!(queue.submit_tail(16), Ok(()));
    assert_eq!(queue.submit_tail(32), Err(Error::QueueBusy));
    assert_eq!(queue.submit_tail(4096), Err(Error::InvalidQueue));
    assert_eq!(queue.submit_tail(17), Err(Error::InvalidQueue));
    assert_eq!(queue.tail(), 16);
}

#[test]
fn undefined_full_range_encoding_never_reaches_hardware() {
    let mut backend = Backend {
        commands: vec![Command([3 << 60, 0xffff_ffff_ffff_f001])],
        ..Default::default()
    };
    let mut queue =
        GuestCommandQueue::new(policy().validate(0x1000, 4096, 4096).unwrap(), 0, 16).unwrap();
    assert_eq!(
        queue.service(&mut backend, &policy(), 4, 64),
        Err(Error::UnsupportedCommand)
    );
    assert_eq!(queue.head(), 0);
    assert!(backend.effects.is_empty());
}

struct Ring {
    writes: Vec<(u32, Command)>,
    tail: u32,
    done: bool,
    sequence: u64,
    status: u64,
    polls: usize,
}
impl RingIo for Ring {
    fn head(&mut self) -> Result<u32, Error> {
        Ok(self.tail)
    }
    fn status(&mut self) -> Result<u64, Error> {
        Ok(self.status)
    }
    fn write(&mut self, o: u32, c: Command) -> Result<(), Error> {
        self.writes.push((o, c));
        if c.0[0] >> 60 == 1 {
            self.sequence = c.0[1];
        }
        Ok(())
    }
    fn clear_completion(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn completion(&mut self) -> Result<u64, Error> {
        self.polls += 1;
        Ok(if self.done { self.sequence } else { 0 })
    }
    fn publish_tail(&mut self, t: u32) -> Result<(), Error> {
        self.tail = t;
        Ok(())
    }
}

#[test]
fn hardware_completion_requires_store_and_overflow_never_means_success() {
    let completion = policy().validate(0x8000, 8, 8).unwrap();
    let mut ring = CommandRing::new(4096, 4080, completion).unwrap();
    let mut io = Ring {
        writes: vec![],
        tail: 4080,
        done: true,
        sequence: 0,
        status: 1 << 4,
        polls: 0,
    };
    ring.execute(Command::invalidate_interrupt(0xa0), &mut io, 4)
        .unwrap();
    assert_eq!(io.tail, 16);
    assert_eq!(io.writes[1].0, 0);
    assert_eq!(io.writes[1].1, Command([1 << 60 | 0x8005, 1]));
    io.done = false;
    assert_eq!(
        ring.execute(Command::invalidate_device(2), &mut io, 3),
        Err(Error::CompletionTimeout)
    );
    assert!(ring.poisoned());
    assert_eq!(io.polls, 4);
    let mut ring = CommandRing::new(4096, 0, completion).unwrap();
    io.status |= 1 << 9;
    assert_eq!(
        ring.execute(Command::invalidate_device(2), &mut io, 3),
        Err(Error::Hardware)
    );
}

#[test]
fn firmware_xt_requirement_cannot_be_overridden_by_other_feature_bits() {
    let mut unit = Unit {
        segment: 0,
        device_id: 2,
        capability: 0x40,
        mmio: policy().validate(0xf7600000, 0x4000, 0x4000).unwrap(),
        firmware_efr: 1 << 7 | 1 << 21,
        firmware_efr2: 0,
    };
    assert_eq!(unit.admit_x2avic(), Err(Error::MissingX2Apic));
    unit.firmware_efr |= 1 << 2;
    assert_eq!(unit.admit_x2avic(), Ok(()));
}

#[test]
fn live_capture_checks_pci_aperture_before_mmio_and_retains_control_drift() {
    struct Registers {
        low: u32,
        reads: Vec<u16>,
        controls: u8,
    }
    impl RegisterReader for Registers {
        fn pci_u32(&mut self, _: Unit, o: u16) -> Result<u32, Error> {
            Ok(match o {
                0x40 => 0x080b000f,
                0x44 => self.low,
                0x48 => 0,
                _ => return Err(Error::RegisterRead),
            })
        }
        fn mmio_u64(&mut self, _: Unit, o: u16) -> Result<u64, Error> {
            self.reads.push(o);
            Ok(match o {
                0x18 => {
                    self.controls += 1;
                    self.controls as u64
                }
                0x30 => 1 << 7,
                0x1a0 => 0x55,
                _ => o as u64,
            })
        }
    }
    let unit = Unit {
        segment: 0,
        device_id: 2,
        capability: 0x40,
        mmio: policy().validate(0xf7600000, 0x4000, 0x4000).unwrap(),
        firmware_efr: 1 << 7 | 1 << 21 | 1 << 2,
        firmware_efr2: 0,
    };
    let mut reader = Registers {
        low: 0xf7600000,
        reads: vec![],
        controls: 0,
    };
    assert_eq!(capture(unit, &mut reader), Err(Error::PciBase));
    assert!(reader.reads.is_empty());
    reader.low |= 1;
    let snapshot = capture(unit, &mut reader).unwrap();
    assert_eq!((snapshot.control_before, snapshot.control_after), (1, 2));
    assert_eq!(snapshot.ga_head, 0x2040);
    assert_eq!(snapshot.efr2, 0x55);
}
