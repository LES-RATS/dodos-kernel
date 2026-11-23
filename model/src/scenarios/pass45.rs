//! Pass 45: review of pass 44 (R6 line words, capped timer priority,
//! checker blind spots, D-171 Harm::accepts, and recovery counters).

use super::pass17::kill;
use super::*;
use crate::system::Event;
use dodos_abi::{MemberState, Owner, Ownership};

// ---------------------------------------------------------------------------
// 1. R6 and line words (finding 6, D-150)
// ---------------------------------------------------------------------------

#[test]
fn r6_stops_at_line_word_naming_dead_generation() {
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, kill(victim));
    // Wait until the IOAPIC holder has already run R1 for the fencing victim,
    // before the victim is fenced and R6 can start.
    assert!(m.step_until(|m| {
        m.sys
            .node(holder)
            .reclaims
            .get(&victim)
            .is_some_and(|r| r.r1.contains_key(&ioapic))
    }));
    assert_eq!(m.state(victim), MemberState::Fencing);
    let gv = m.sys.canonical().generation(victim);
    m.sys.edit_table(|t| {
        let taken = Ownership::taken(Owner::Node(victim), gv).raw();
        t.lines.insert(9, taken);
    });
    assert!(m.sys.node(holder).r6_held_back(victim, gv));
    m.run(40 * MS);
    assert_eq!(m.state(victim), MemberState::Fenced);
    assert!(m.sys.canonical().lines.contains_key(&9));
}

#[test]
fn checker_rejects_r6_write_of_line_word() {
    // Even the controller holder cannot write free a line word during R6
    // (after R5 has acknowledged the dead generation), nor can a non-holder.
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    let non_holder = (0..3u16).find(|&n| n != holder && n != victim).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, kill(victim));
    assert!(m.step_until(|m| {
        m.sys
            .node(holder)
            .reclaims
            .get(&victim)
            .is_some_and(|r| r.r1.contains_key(&ioapic))
    }));
    let gv = m.sys.canonical().generation(victim);
    m.sys.edit_table(|t| {
        let taken = Ownership::taken(Owner::Node(victim), gv).raw();
        t.lines.insert(9, taken);
    });
    // Wait until victim is Fenced and all live nodes have acknowledged it (R5).
    m.run(40 * MS);
    assert_eq!(m.state(victim), MemberState::Fenced);
    assert_eq!(
        m.sys.canonical().beacons[holder as usize].acked[victim as usize],
        gv
    );
    assert_eq!(
        m.sys.canonical().beacons[non_holder as usize].acked[victim as usize],
        gv
    );

    let taken = Ownership::taken(Owner::Node(victim), gv).raw();
    let unclaimed = Ownership::free(Owner::Unclaimed).raw();

    // Case 1: controller holder journals an R6 free of line 9 after R5 ack.
    let mut sys_holder = m.sys.clone();
    sys_holder.edit_table(|t| {
        let write = t.journal(holder, crate::table::Word::Line(9), taken, unclaimed);
        t.lines.remove(&9);
        t.writes.push(write);
    });
    assert_eq!(
        sys_holder.node(holder).check(),
        Err("52: write of an ownership word")
    );
    assert!(
        sys_holder
            .check()
            .is_err_and(|e| e.ends_with("52: write of an ownership word"))
    );

    // Case 2: non-holder journals an R6 free of line 9.
    let mut sys_other = m.sys.clone();
    sys_other.edit_table(|t| {
        let write = t.journal(non_holder, crate::table::Word::Line(9), taken, unclaimed);
        t.lines.remove(&9);
        t.writes.push(write);
    });
    assert_eq!(
        sys_other.node(non_holder).check(),
        Err("52: write of an ownership word")
    );
    assert!(
        sys_other
            .check()
            .is_err_and(|e| e.ends_with("52: write of an ownership word"))
    );
}

#[test]
fn r1_controller_holder_frees_line_word_on_holder_death_and_takeover() {
    // When the IOAPIC holder itself claims a line and is killed, the successor
    // takes over the IOAPIC role, runs R1 in `after_take`, frees the line word,
    // and R6 then transitions the dead holder to Dead.
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let killer = (0..3u16).find(|&n| n != holder).unwrap();
    assert!(matches!(
        super::pass18::claim_line(&mut m, holder, 5, 210),
        Ok(Output::Msi { .. })
    ));
    assert!(m.sys.canonical().lines.contains_key(&5));

    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(holder));
    m.run(60 * MS);
    assert_eq!(m.state(holder), MemberState::Dead);
    assert!(!m.sys.canonical().lines.contains_key(&5));
}

// ---------------------------------------------------------------------------
// 2. Capped timer interrupts run before anything else on that core (finding 1)
// ---------------------------------------------------------------------------

#[test]
fn wake_drains_due_events_at_now_before_ipi() {
    // 16 past timers at `t1` + 1 timer at `t1 + 200_000` (exact wake instant
    // with PSCI exit latency 200_000): `Wake` must drain every due event at or
    // before `now`, leaving 0 due timers before `Event::Ipi` runs.
    let mach = machine(Arch::Aarch64, 2, &[&[0], &[1]]);
    let mut m = S::on(&mach);
    let method = dodos_abi::IDLE_METHOD_PSCI | ((1u64 << 16) << 32);
    let latency = 200_000u64;
    let entry: [u64; 4] = [latency, 400_000, method, dodos_abi::IDLE_FLAG_LOSES_CONTEXT];
    m.ok(
        1,
        Syscall::NodeIdle {
            node: node_cap(1),
            core: 1,
            words: entry.to_vec(),
        },
    );
    m.ok(1, m.retype(1, ObjectKind::Notify, 0, 150));
    for i in 0..17u32 {
        m.ok(1, m.retype(1, ObjectKind::Timer, 0, 160 + i));
    }
    let t1 = m.sys.now() + 2 * MS;
    let wake_at = t1 + latency;
    for i in 0..16u32 {
        assert_eq!(
            m.call(
                1,
                Syscall::TimerSet {
                    timer: s(160 + i),
                    notify: s(150),
                    deadline: t1,
                    tolerance: 0,
                },
            ),
            Ok(Output::WasArmed(0))
        );
    }
    assert_eq!(
        m.call(
            1,
            Syscall::TimerSet {
                timer: s(176),
                notify: s(150),
                deadline: wake_at,
                tolerance: 0,
            },
        ),
        Ok(Output::WasArmed(0))
    );
    m.ok(1, Syscall::ThreadSuspend(s(dodos_abi::START_SLOT_THREAD)));
    for _ in 0..200 {
        if matches!(m.sys.hardware(1), crate::system::Hardware::Asleep { .. }) {
            break;
        }
        let events = m.sys.events();
        let next = events
            .iter()
            .copied()
            .find(|e| matches!(e, Event::Sleep { core: 1 } | Event::PowerDown { core: 1 }))
            .or_else(|| events.into_iter().next());
        if let Some(ev) = next {
            m.sys.fire(ev);
        } else {
            m.sys.move_clock((m.sys.now() + 100_000).min(t1 - 1));
        }
    }
    assert!(matches!(
        m.sys.hardware(1),
        crate::system::Hardware::Asleep { .. }
    ));
    m.sys.move_clock(wake_at);
    m.sys
        .node_mut(1)
        .cores[0]
        .ipis
        .insert(crate::cores::Ipi::Reschedule);
    m.sys.fire(Event::Wake { core: 1 });
    assert_eq!(m.sys.now(), wake_at);
    assert_eq!(m.sys.node(1).timers.values().flatten().count(), 0);
    assert!(
        m.sys.node(1).cores[0]
            .hw_timer
            .is_none_or(|t| t > wake_at)
    );
    let events = m.sys.events();
    assert!(!events.contains(&Event::Timer {
        node: 1,
        position: 0
    }));
    assert!(events.contains(&Event::Ipi {
        node: 1,
        position: 0
    }));
    assert_eq!(m.sys.check(), Ok(()));
}

#[test]
fn capped_timer_interrupt_offers_only_timer_on_draining_core() {
    let mut m = S::two();
    m.ok(0, m.retype(0, ObjectKind::Notify, 0, 150));
    for i in 0..17u32 {
        m.ok(0, m.retype(0, ObjectKind::Timer, 0, 160 + i));
    }
    let deadline = m.sys.now() + 5 * MS;
    for i in 0..17u32 {
        assert_eq!(
            m.call(
                0,
                Syscall::TimerSet {
                    timer: s(160 + i),
                    notify: s(150),
                    deadline,
                    tolerance: 0,
                },
            ),
            Ok(Output::WasArmed(0))
        );
    }
    m.run(deadline - m.sys.now() - 1);
    m.sys.move_clock(deadline);
    m.sys
        .node_mut(0)
        .cores[0]
        .ipis
        .insert(crate::cores::Ipi::Reschedule);
    m.sys
        .node_mut(1)
        .cores[0]
        .ipis
        .insert(crate::cores::Ipi::Reschedule);
    m.sys.fire(Event::Timer {
        node: 0,
        position: 0,
    });
    assert_eq!(m.sys.node(0).cores[0].hw_timer, Some(deadline));
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 1);
    let events = m.sys.events();
    assert!(events.contains(&Event::Timer {
        node: 0,
        position: 0
    }));
    assert!(
        !events.contains(&Event::Ipi {
            node: 0,
            position: 0
        }),
        "draining core must offer only Event::Timer, got {events:?}"
    );
    assert!(
        events.contains(&Event::Ipi {
            node: 1,
            position: 0
        }),
        "other cores must not be affected, got {events:?}"
    );
    assert_eq!(m.sys.check(), Ok(()));

    // After the follow-up timer interrupt drains the 17th timer, the IPI runs.
    m.sys.fire(Event::Timer {
        node: 0,
        position: 0,
    });
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 0);
    assert!(m.sys.events().contains(&Event::Ipi {
        node: 0,
        position: 0
    }));
    assert_eq!(m.sys.check(), Ok(()));
}
