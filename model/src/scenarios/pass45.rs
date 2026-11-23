//! Pass 45: review of pass 44 (R6 line words, capped timer priority,
//! checker blind spots, D-171 Harm::accepts, and recovery counters).

use super::pass17::{fresh, kill, resume, seal};
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
    super::explore::quiet(&mut m);
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
    m.sys.node_mut(1).cores[0]
        .ipis
        .insert(crate::cores::Ipi::Reschedule);
    m.sys.fire(Event::Wake { core: 1 });
    assert_eq!(m.sys.now(), wake_at);
    assert_eq!(m.sys.node(1).timers.values().flatten().count(), 0);
    assert!(m.sys.node(1).cores[0].hw_timer.is_none_or(|t| t > wake_at));
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
    m.sys.node_mut(0).cores[0]
        .ipis
        .insert(crate::cores::Ipi::Reschedule);
    m.sys.node_mut(1).cores[0]
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
        "a draining core takes no IPI before its due events, got {events:?}"
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

// ---------------------------------------------------------------------------
// 3. Checker blind spots (finding 2)
// ---------------------------------------------------------------------------

#[test]
fn silent_running_owned_core_rejected_by_check() {
    let mut m = S::two();
    m.ok(1, m.retype(1, ObjectKind::Notify, 0, 150));
    m.ok(1, m.retype(1, ObjectKind::Timer, 0, 151));
    let deadline = m.sys.now() + MS;
    assert_eq!(
        m.call(
            1,
            Syscall::TimerSet {
                timer: s(151),
                notify: s(150),
                deadline,
                tolerance: 0,
            },
        ),
        Ok(Output::WasArmed(0))
    );
    m.sys.move_clock(deadline + 500 * MS);
    m.sys.node_mut(1).cores[0].silent = true;
    assert_eq!(m.sys.hardware(1), crate::system::Hardware::Running);
    assert_eq!(
        m.sys.check(),
        Err("SILENT: a running owned core is marked silent without parking".to_string())
    );
}

#[test]
fn timer_interrupt_due_step_checked_by_event_log() {
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
    m.sys.fire(Event::Timer {
        node: 0,
        position: 0,
    });
    // Under mutation `.take(TIME_EVENT_STEP)` -> `.take(1)` in `Model::due`,
    // `System::check()` must fail here.
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 1);
}

// ---------------------------------------------------------------------------
// 4. Harm::accepts (finding 3, D-171)
// ---------------------------------------------------------------------------

#[test]
fn harm_accepts_coresilent_only_for_d171_orphaned_fresh_start() {
    // Part A (review44_repro_3a): 1-core orphaned fresh start must NOT be
    // accepted with CrashReason::CoreSilent.
    let mut m1 = S::two();
    m1.supervisor(0, m1.t[0]);
    m1.ok(0, kill(1));
    m1.run(40 * MS);
    assert_eq!(m1.state(1), MemberState::Dead);
    m1.ok(0, fresh(1, 1 << 1));
    m1.hang(0, false);
    let g1 = m1.sys.member(1).generation();
    let acc1 = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        1,
        g1,
    );
    assert!(!m1.harm.accepts(
        1,
        acc1,
        Some((dodos_abi::CrashReason::CoreSilent as u64, g1, 1)),
        m1.sys.canonical(),
    ));

    // Part B (review44_repro_3a): 2-core orphaned fresh start that reaches
    // MemberState::Live inside one System::advance (without sample_bounds)
    // must NOT be accepted with CrashReason::CoreSilent.
    let mach = machine(Arch::X86_64, 3, &[&[0], &[1, 2]]);
    let mut m2 = S::on(&mach);
    m2.supervisor(0, m2.t[0]);
    m2.ok(0, kill(1));
    m2.run(40 * MS);
    assert_eq!(m2.state(1), MemberState::Dead);
    m2.ok(0, fresh(1, 0b110));
    for _ in 0..20 {
        let ev = m2
            .sys
            .events()
            .into_iter()
            .find(|&e| !matches!(e, Event::EntryStep { core: 2 }));
        let Some(ev) = ev else { break };
        m2.sys.fire(ev);
        if m2.sys.node(0).starts.is_empty() && m2.state(1) == MemberState::Starting {
            break;
        }
    }
    assert_eq!(m2.state(1), MemberState::Starting);
    m2.hang(0, false);
    m2.sys.advance(m2.sys.now() + 10 * MS);
    assert_eq!(m2.state(1), MemberState::Live);
    let g2 = m2.sys.member(1).generation();
    let acc2 = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        1,
        g2,
    );
    assert!(!m2.harm.accepts(
        1,
        acc2,
        Some((dodos_abi::CrashReason::CoreSilent as u64, g2, 1)),
        m2.sys.canonical(),
    ));
    // Even if the now-Live node self-accuses CoreSilent without sample_bounds
    // having run, the model records that it was not a D-171 start timeout.
    m2.sys
        .node_mut(1)
        .accuse_self(dodos_abi::CrashReason::CoreSilent as u32);
    assert!(!m2.harm.accepts(
        1,
        acc2,
        m2.sys.canonical().beacons[1].panic,
        m2.sys.canonical(),
    ));

    // Part C: 2-core orphaned resume must NOT be accepted with CoreSilent.
    let mut m3 = S::on(&mach);
    m3.supervisor(0, m3.t[0]);
    m3.ok(0, seal(1));
    m3.run(40 * MS);
    assert_eq!(m3.state(1), MemberState::Sealed);
    m3.ok(0, resume(1));
    m3.hang(0, false);
    let g3 = m3.sys.member(1).generation();
    let acc3 = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        1,
        g3,
    );
    assert!(!m3.harm.accepts(
        1,
        acc3,
        Some((dodos_abi::CrashReason::CoreSilent as u64, g3, 1)),
        m3.sys.canonical(),
    ));

    // Part D: 2-core orphaned fresh start accusing CoreSilent BEFORE START_NS
    // from start_entered is rejected; once at or after START_NS with a core
    // still awaiting entry, genuine D-171 self-accusation IS accepted.
    let mut m4 = S::on(&mach);
    m4.supervisor(0, m4.t[0]);
    m4.ok(0, kill(1));
    m4.run(40 * MS);
    assert_eq!(m4.state(1), MemberState::Dead);
    m4.ok(0, fresh(1, 0b110));
    let is_core2_start = |sys: &System, ev: Event| match ev {
        Event::EntryStep { core: 2 } => true,
        Event::Action(i) => matches!(
            sys.actions().get(i),
            Some((_, crate::HwAction::Start { core: 2, .. }))
        ),
        _ => false,
    };
    for _ in 0..200 {
        let ev = m4
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m4.sys, ev));
        let Some(ev) = ev else { break };
        m4.sys.fire(ev);
        if m4.sys.has(1) && m4.sys.node(1).start_entered.is_some() {
            break;
        }
    }
    assert_eq!(m4.state(1), MemberState::Starting);
    assert!(m4.sys.node(1).cores[1].awaiting_entry);
    // Hang node 0 while core 2 is still un-entered, and cancel any pending
    // start action for core 2 by hanging before node 0 starts it.
    m4.hang(0, false);
    let g4 = m4.sys.member(1).generation();
    let acc4 = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        1,
        g4,
    );
    // Before START_NS has elapsed, no D-171 accusation is recorded or accepted.
    assert!(!m4.harm.accepts(
        1,
        acc4,
        Some((dodos_abi::CrashReason::CoreSilent as u64, g4, 1)),
        m4.sys.canonical(),
    ));
    // Advance past START_NS from start_entered while holding back core 2's
    // start: node 1 self-accuses CoreSilent via D-171 and records it.
    let first_entry = m4.sys.node(1).start_entered.expect("start_entered set");
    let limit = first_entry + dodos_abi::START_NS + 50 * MS;
    while m4.sys.now() <= limit && m4.sys.canonical().beacons[1].panic.is_none() {
        if let Some(ev) = m4
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m4.sys, ev))
        {
            m4.sys.fire(ev);
        } else {
            m4.sys.move_clock((m4.sys.now() + 5 * MS).min(limit + 1));
        }
    }
    let panic4 = m4.sys.canonical().beacons[1].panic;
    assert!(matches!(
        panic4,
        Some((r, g, _)) if r == dodos_abi::CrashReason::CoreSilent as u64 && g == g4
    ));
    assert!(
        m4.sys
            .canonical()
            .start_timeout_accusations
            .0
            .contains(&(1, g4))
    );
    assert!(m4.harm.accepts(1, acc4, panic4, m4.sys.canonical()));
}

// ---------------------------------------------------------------------------
// 5. Recovery counters (findings 4 and 5)
// ---------------------------------------------------------------------------

#[test]
fn healthy_sweep_does_not_set_time_event_step_16_in_recovery() {
    let mut m = S::two();
    if m.sys.node(0).next_sweep <= m.sys.now() + 2 * MS {
        let past = m.sys.node(0).next_sweep.saturating_sub(m.sys.now()) + MS;
        m.run(past);
    }
    m.ok(0, m.retype(0, ObjectKind::Notify, 0, 150));
    for i in 0..17u32 {
        m.ok(0, m.retype(0, ObjectKind::Timer, 0, 160 + i));
    }
    let sweep_at = m.sys.node(0).next_sweep;
    for i in 0..17u32 {
        assert_eq!(
            m.call(
                0,
                Syscall::TimerSet {
                    timer: s(160 + i),
                    notify: s(150),
                    deadline: sweep_at,
                    tolerance: 0,
                },
            ),
            Ok(Output::WasArmed(0))
        );
    }
    m.run(sweep_at - m.sys.now() - 1);
    m.sys.move_clock(sweep_at);
    assert!(!m.sys.node(0).recovery_in_progress(None));
    assert!(!m.sys.node(0).core0_recovery_due_now());
    m.sys.fire(Event::Timer {
        node: 0,
        position: 0,
    });
    assert_eq!(m.sys.check(), Ok(()));
    m.sample_bounds();
    assert!(m.bounds.time_event_step_16);
    assert!(!m.bounds.time_event_step_16_in_recovery);
}

#[test]
fn planned_role_hand_and_stale_pending_step_not_counted_as_recovery() {
    // Part A (review44_repro_5): planned RoleHand of a unit role on healthy
    // Live nodes must NOT count as `recovery_in_progress`.
    let mut m = super::units::three();
    let unit = super::units::role_slot(&m, 1, super::units::IOMMU_ROLE);
    assert!(!m.sys.node(1).recovery_in_progress(None));
    assert!(!m.sys.node(2).recovery_in_progress(Some(1)));
    assert_eq!(
        m.call(
            1,
            Syscall::RoleHand {
                role: unit,
                node: node_cap(2),
            },
        ),
        WAITS
    );
    assert!(!m.sys.node(1).recovery_in_progress(None));
    assert!(m.step_until(
        |m| m.sys.role(super::units::IOMMU_ROLE).state() == dodos_abi::RoleState::Handing
    ));
    assert!(!m.sys.node(1).recovery_in_progress(None));
    assert!(!m.sys.node(2).recovery_in_progress(Some(1)));

    // Part B (review44_repro_5): an earlier non-recovery flush that hit
    // PENDING_STEP must NOT cause a later 1-message recovery flush to latch
    // `pending_step_32_in_recovery`.
    let mut m2 = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m2.supervisor(2, m2.t[2]);
    let ut0 = untyped_slot(m2.sys.node(0), m2.t[0]);
    for i in 0..72u32 {
        m2.ok(0, retype_from(ut0, ObjectKind::Frame, 1, 200 + i));
    }
    let reply1 = m2.retype(1, ObjectKind::Reply, 0, 150);
    m2.ok(1, reply1);
    m2.ok(1, place_first(1, 1));
    let rx1 = m2.thread_on(1, 160, 1);
    let rights = Rights::READ.union(Rights::SEND).bits();
    for batch in 0..(72u32 / 4) {
        let first = batch * 4;
        assert_eq!(
            m2.call_by(
                1,
                rx1,
                Syscall::Recv {
                    endpoint: s(dodos_abi::START_SLOT_MAILBOX),
                    reply: s(150),
                    range: crate::Range {
                        first: s(200 + first),
                        count: 4,
                    },
                    wait: 0,
                },
            ),
            WAITS
        );
        assert_eq!(
            m2.call(
                0,
                Syscall::NodeSend {
                    node: node_cap(1),
                    message: crate::Message {
                        caps: (first..first + 4)
                            .map(|i| crate::Transfer::Derive(s(200 + i), rights))
                            .collect(),
                        ..crate::Message::default()
                    },
                    doorbell: SlotAddr::NONE,
                },
            ),
            WAITS
        );
        m2.settle();
        assert_eq!(m2.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    }
    for i in 0..dodos_abi::RING_USER_ENTRIES as u32 {
        m2.ok(1, Syscall::GrantSignal(s(200 + i)));
    }
    for i in 0..33u32 {
        m2.ok(1, Syscall::GrantSignal(s(236 + i)));
    }
    let ring_used = |m: &S| {
        m.sys
            .node(1)
            .ring_of(1, 0, true)
            .produced
            .wrapping_sub(m.sys.node(1).ring_consumed(0, true))
    };
    for _ in 0..500 {
        if ring_used(&m2) == 0 {
            break;
        }
        let ev = m2.sys.events().into_iter().find(|&ev| {
            !matches!(
                ev,
                Event::Timer { node: 1, .. } | Event::Ipi { node: 1, .. }
            )
        });
        match ev {
            Some(ev) => m2.sys.fire(ev),
            None => m2.sys.move_clock(m2.sys.now() + 100_000),
        }
    }
    assert_eq!(ring_used(&m2), 0);
    let rdue = m2.sys.node(1).reserve_due().expect("reserve due");
    m2.sys.move_clock(rdue);
    m2.sys.fire(Event::Timer {
        node: 1,
        position: 0,
    });
    assert_eq!(m2.sys.check(), Ok(()));
    m2.sample_bounds();
    assert!(m2.bounds.pending_step_32);
    assert!(!m2.bounds.pending_step_32_in_recovery);
    assert_eq!(
        m2.sys.canonical().uses[1]
            .values()
            .filter(|u| u.signal_pending)
            .count(),
        1
    );
    m2.ok(2, seal(0));
    m2.run(12 * MS);
    assert!(!m2.bounds.pending_step_32_in_recovery);
}
