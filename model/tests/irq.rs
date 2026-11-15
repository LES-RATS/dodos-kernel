//! Interrupts: one test per rule, on the start-up state of
//! `Machine::example` (its irq_control in slot 6, one IOAPIC of 24 inputs,
//! interrupt remapping on, the boot core's APIC ID 0).

use dodos_abi::ObjectKind::{Endpoint, Notify, Reply, Sched, Thread};
use dodos_abi::*;
use dodos_model::*;

const UT: u32 = 20;
const CONTROL: u32 = START_SLOT_IRQ_CONTROL;
const NT: u32 = 30;
const DONE: Outcome = Ok(Output::Done);

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

struct K {
    m: Model,
    t: ObjId,
}

impl K {
    fn on(machine: &Machine) -> Self {
        let (m, t) = Model::boot(machine).unwrap();
        let mut k = Self { m, t };
        k.make(Notify, NT);
        k
    }

    fn new() -> Self {
        Self::on(&Machine::example())
    }

    fn go(&mut self, call: Syscall) -> Outcome {
        let outcome = self.m.apply(self.t, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        assert_eq!(self.go(call), DONE);
    }

    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    fn make(&mut self, kind: ObjectKind, slot: u32) {
        self.ok(Syscall::UntypedRetype {
            untyped: s(UT),
            kind,
            size: 0,
            page: 0,
            memory: 0,
            destination: s(slot),
        });
    }

    fn arrive(&mut self, vector: u8) {
        self.m.interrupt(vector);
        assert_eq!(self.m.check(), Ok(()));
    }

    fn word(&mut self) -> u64 {
        match self.go(Syscall::Poll(s(NT))) {
            Ok(Output::Word(word)) => word,
            Err(Error::WouldWait) => 0,
            other => panic!("{other:?}"),
        }
    }
}

fn claim(source: IrqSource, vector: u64, slot: u32) -> Syscall {
    Syscall::IrqClaim {
        control: s(CONTROL),
        source: source.raw(),
        vector,
        destination: s(slot),
    }
}

fn msi() -> IrqSource {
    IrqSource::msi(0, 3, 1, 0)
}

fn bind(irq: u32, notify: u32) -> Syscall {
    Syscall::IrqBind {
        irq: s(irq),
        notify: if notify == 0 {
            SlotAddr::NONE
        } else {
            s(notify)
        },
    }
}

#[test]
fn irq_claim_returns_the_remappable_msi_of_its_handle_and_0_for_a_line() {
    let mut k = K::new();
    let msi_of = |handle: u64| 0xFEE0_0000 | handle << 5 | 1 << 4;
    for (vector, slot) in [(48, 40), (49, 41), (239, 42)] {
        let result = k.go(claim(msi(), vector, slot));
        assert_eq!(
            result,
            Ok(Output::Msi {
                address: msi_of(vector - 48),
                data: 0
            })
        );
    }
    let line = claim(IrqSource::line(9, true, true), 50, 43);
    assert_eq!(
        k.go(line),
        Ok(Output::Msi {
            address: 0,
            data: 0
        })
    );
}

#[test]
fn without_remapping_the_msi_names_the_apic_and_the_vector() {
    let mut machine = Machine::example();
    machine.remapping = false;
    machine.cores[0].apic_id = 5;
    let mut k = K::on(&machine);
    let result = k.go(claim(msi(), 60, 40));
    let address = 0xFEE0_0000 | 5 << 12;
    assert_eq!(result, Ok(Output::Msi { address, data: 60 }));
    // A core of APIC ID 256 cannot be reached this way, after the vector.
    machine.cores[0].apic_id = 256;
    let mut k = K::on(&machine);
    k.no(claim(msi(), 300, 40), Error::Invalid);
    k.no(claim(msi(), 60, 40), Error::Unsupported);
}

#[test]
fn irq_claim_needs_manage_on_an_irq_control() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: s(CONTROL),
        destination: s(31),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let weak = |control| Syscall::IrqClaim {
        control: s(control),
        source: msi().raw(),
        vector: 48,
        destination: s(40),
    };
    k.no(weak(31), Error::Denied);
    k.no(weak(NT), Error::Invalid);
}

#[test]
fn source_then_vector_are_judged_after_the_destination() {
    let mut k = K::new();
    let raw = |source: u64, vector, slot| Syscall::IrqClaim {
        control: s(CONTROL),
        source,
        vector,
        destination: s(slot),
    };
    // The destination first: slot 0.
    k.no(raw(0, 0, 0), Error::Invalid);
    // A source the format refuses; a GSI that no IOAPIC serves; a vector
    // outside 48 to 239.
    k.no(raw(3, 48, 40), Error::Invalid);
    k.no(
        raw(IrqSource::line(24, false, false).raw(), 48, 40),
        Error::Invalid,
    );
    for vector in [47, 240, 1 << 40] {
        k.no(raw(msi().raw(), vector, 40), Error::Invalid);
    }
    let line = raw(IrqSource::line(23, false, false).raw(), 48, 40);
    assert!(k.go(line).is_ok());
}

#[test]
fn claimed_vector_or_line_is_busy_before_a_full_destination() {
    let mut k = K::new();
    k.go(claim(IrqSource::line(4, false, false), 48, 40))
        .unwrap();
    k.no(claim(msi(), 48, 41), Error::Busy);
    k.no(claim(IrqSource::line(4, true, false), 49, 41), Error::Busy);
    k.no(claim(msi(), 48, NT), Error::Busy);
    k.no(claim(msi(), 49, NT), Error::NoSlot);
}

#[test]
fn irq_claim_checks_the_depth_last() {
    let mut k = K::new();
    let mut from = CONTROL;
    for slot in 100..163 {
        k.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(slot),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
        from = slot;
    }
    let deep = |vector| Syscall::IrqClaim {
        control: s(162),
        source: msi().raw(),
        vector,
        destination: s(40),
    };
    k.no(deep(300), Error::Invalid);
    k.no(deep(48), Error::Invalid);
    k.go(claim(msi(), 48, 40)).unwrap();
}

#[test]
fn revoking_the_control_capability_releases_its_vectors() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: s(CONTROL),
        destination: s(31),
        rights: Rights::ALL.bits(),
        badge: 0,
    });
    let through = Syscall::IrqClaim {
        control: s(31),
        source: msi().raw(),
        vector: 48,
        destination: s(40),
    };
    k.go(through).unwrap();
    k.ok(Syscall::CapRevoke(s(CONTROL)));
    k.go(claim(msi(), 48, 41)).unwrap();
}

#[test]
fn new_irq_is_idle_with_every_right() {
    let mut k = K::new();
    k.go(claim(msi(), 48, 40)).unwrap();
    k.no(Syscall::IrqAck(s(40)), Error::Busy);
    let inspected = Output::Inspect {
        kind: ObjectKind::Irq,
        rights: Rights::ALL,
        badge: 0,
    };
    assert_eq!(k.go(Syscall::CapInspect(s(40))), Ok(inspected));
}

#[test]
fn bound_irq_signals_once_until_it_is_acknowledged() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: s(NT),
        destination: s(31),
        rights: Rights::ALL.bits(),
        badge: 0b100,
    });
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, 31));
    k.arrive(48);
    assert_eq!(k.word(), 0b100);
    // Fired: an edge arrival is pending, not signalled.
    k.arrive(48);
    assert_eq!(k.word(), 0);
    // The acknowledgement signals the pending one, and it stays fired.
    k.ok(Syscall::IrqAck(s(40)));
    assert_eq!(k.word(), 0b100);
    k.ok(Syscall::IrqAck(s(40)));
    k.ok(Syscall::IrqAck(s(40)));
    k.arrive(48);
    assert_eq!(k.word(), 0b100);
}

#[test]
fn level_source_is_masked_while_fired_and_records_nothing() {
    let mut k = K::new();
    k.go(claim(IrqSource::line(5, true, false), 48, 40))
        .unwrap();
    k.ok(bind(40, NT));
    k.arrive(48);
    k.arrive(48);
    assert_eq!(k.word(), 1);
    k.ok(Syscall::IrqAck(s(40)));
    assert_eq!(k.word(), 0);
}

#[test]
fn binding_with_a_pending_arrival_signals_at_once_and_unbinding_keeps_pending() {
    let mut k = K::new();
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, 0));
    k.arrive(48);
    k.ok(bind(40, NT));
    assert_eq!(k.word(), 1);
    // Fired; unbound, then a new arrival is pending; bound again, it is
    // signalled.
    k.ok(bind(40, 0));
    k.arrive(48);
    k.ok(bind(40, NT));
    assert_eq!(k.word(), 1);
}

#[test]
fn irq_bind_is_busy_when_bound_and_checks_its_rights() {
    let mut k = K::new();
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, NT));
    k.no(bind(40, NT), Error::Busy);
    for (from, to, rights) in [(40, 41, Rights::WRITE), (NT, 32, Rights::RECV)] {
        k.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(to),
            rights: rights.bits(),
            badge: 0,
        });
    }
    k.no(bind(41, NT), Error::Denied);
    k.ok(bind(40, 0));
    k.no(bind(40, 32), Error::Denied);
    k.no(Syscall::IrqAck(s(40)), Error::Busy);
}

#[test]
fn arrival_on_an_unclaimed_vector_is_ignored() {
    let mut k = K::new();
    k.arrive(48);
    k.arrive(250);
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, NT));
    assert_eq!(k.word(), 0);
}

#[test]
fn destroying_the_notify_unbinds_the_irq_and_keeps_pending() {
    let mut k = K::new();
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, NT));
    k.arrive(48);
    k.arrive(48);
    k.ok(Syscall::CapDelete(s(NT)));
    k.no(Syscall::IrqAck(s(40)), Error::Busy);
    // The pending arrival is delivered to the next notify.
    k.make(Notify, NT);
    k.ok(bind(40, NT));
    assert_eq!(k.word(), 1);
}

#[test]
fn destroying_the_irq_frees_its_vector() {
    let mut k = K::new();
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, NT));
    k.ok(Syscall::CapDelete(s(40)));
    k.go(claim(msi(), 48, 41)).unwrap();
    k.arrive(48);
    assert_eq!(k.word(), 0);
}

#[test]
fn arrival_on_a_dying_notify_is_delivered_as_on_a_live_one() {
    let mut k = K::new();
    k.make(Endpoint, 33);
    k.make(Reply, 34);
    k.ok(Syscall::SchedConfigure {
        sched: s(START_SLOT_SCHED),
        core: s(16),
        budget: 5_000_000,
        period: 10_000_000,
        class: 1,
    });
    k.make(Sched, 35);
    k.ok(Syscall::SchedConfigure {
        sched: s(35),
        core: s(16),
        budget: 10_000,
        period: 10_000_000,
        class: 1,
    });
    k.make(Thread, 36);
    k.ok(Syscall::ThreadConfigure {
        thread: s(36),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    });
    k.ok(Syscall::SchedBind {
        sched: s(35),
        thread: s(36),
    });
    k.ok(Syscall::ThreadResume(s(36)));
    let receiver = k.m.object_at(k.t, s(36)).unwrap();
    k.ok(Syscall::NotifyBind {
        notify: s(NT),
        endpoint: s(33),
    });
    let recv = Syscall::Recv {
        endpoint: s(33),
        reply: s(34),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.m.apply(receiver, recv), Ok(Output::Waits));
    k.go(claim(msi(), 48, 40)).unwrap();
    k.ok(bind(40, NT));
    for slot in 60..93 {
        k.ok(Syscall::CapDerive {
            source: s(NT),
            destination: s(slot),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    k.arrive(48);
    let Some(Ok(Output::Message(delivery))) = k.m.woken(receiver) else {
        panic!("no notification");
    };
    assert_eq!(delivery.received.header.flags(), Header::FLAG_NOTIFICATION);
}
