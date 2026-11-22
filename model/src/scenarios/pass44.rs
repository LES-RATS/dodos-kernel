//! Pass 44: D-171, review findings from pass 43, and pressure tests under recovery.

use super::pass17::{fresh, kill};
use super::*;
use crate::nodes::HwAction;
use crate::system::Event;
use dodos_abi::{CrashReason, START_NS, START_SLOT_ASPACE, START_SLOT_CSPACE};

fn is_core2_start(sys: &System, ev: Event) -> bool {
    match ev {
        Event::Action(i) => sys.actions()[i].1 == HwAction::Start { core: 2 },
        Event::EntryStep { core: 2 } => true,
        _ => false,
    }
}

fn next_timer_instant(sys: &System) -> Option<u64> {
    sys.models()
        .flat_map(|(_, model)| {
            (0..model.core_count())
                .filter(|&p| !model.is_silent(p))
                .filter_map(|p| model.timer_of(p))
        })
        .min()
}

/// Helper: node 1 starts on cores [1, 2], core 2 runs a thread so its beacon
/// holds `idle_until == 0`, then node 1 is killed and fresh-started on [1, 2]
/// with core 2's start held back until core 1 enters `core_entered`.
fn setup_staggered_fresh_start() -> (S, u64) {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1, 2]]);
    mach.membership.stale_ms = 40;
    mach.membership.sweep_ms = 100;
    let mut m = S::on(&mach);

    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(1, retype_from(ut1, ObjectKind::Thread, 0, 160));
    m.ok(1, retype_from(ut1, ObjectKind::Sched, 0, 161));
    m.ok(
        1,
        Syscall::ThreadConfigure {
            thread: s(160),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: SlotAddr::NONE,
        },
    );
    m.ok(
        1,
        Syscall::SchedConfigure {
            sched: s(161),
            node: node_cap(1),
            core: 2,
            budget: 50 * MS,
            period: 100 * MS,
            class: 1,
        },
    );
    m.ok(
        1,
        Syscall::SchedBind {
            sched: s(161),
            thread: s(160),
        },
    );
    m.ok(1, Syscall::ThreadResume(s(160)));
    m.run(2 * MS);
    assert_eq!(m.sys.canonical().core_beacons[2].idle_until.0, 0);

    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);

    m.ok(0, fresh(1, 0b110));
    for _ in 0..200 {
        let ev = m
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m.sys, ev));
        let Some(ev) = ev else { break };
        m.sys.fire(ev);
        assert_eq!(m.sys.check(), Ok(()));
        if m.sys.has(1) && !m.sys.node(1).is_silent(0) {
            break;
        }
    }
    assert_eq!(m.state(1), MemberState::Starting);
    assert!(m.sys.node(1).is_silent(1));
    let first_entry = m.sys.node(1).start_entered.expect("first entry recorded");
    (m, first_entry)
}

#[test]
fn exp_2b_staggered_fresh_start_does_not_falsely_self_accuse_before_start_ns() {
    let (mut m, _first_entry) = setup_staggered_fresh_start();

    // Advance 80 ms (past STALE_NS = 40 ms, well before START_NS = 1000 ms)
    // while holding core 2's start events: D-171 prevents false self-accusation.
    let target = m.sys.now() + 80 * MS;
    while m.sys.now() < target {
        if let Some(ev) = m
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m.sys, ev))
        {
            m.sys.fire(ev);
            assert_eq!(m.sys.check(), Ok(()));
        } else {
            m.sys.move_clock((m.sys.now() + 500_000).min(target));
        }
    }
    assert_eq!(m.sys.canonical().beacons[1].panic, None);
    assert_eq!(m.state(1), MemberState::Starting);

    // Let core 2 enter now: node 1 transitions cleanly to Live.
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.canonical().beacons[1].panic, None);
}

#[test]
fn unentered_core_at_fresh_start_self_accuses_at_start_ns() {
    let (mut m, first_entry) = setup_staggered_fresh_start();
    let generation = m.sys.canonical().generation(1);

    // Keep core 2 from ever entering, and advance up to just before START_NS.
    let before_deadline = first_entry + START_NS - 1;
    while m.sys.now() < before_deadline {
        if let Some(ev) = m
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m.sys, ev))
        {
            m.sys.fire(ev);
            assert_eq!(m.sys.check(), Ok(()));
        } else {
            let next =
                next_timer_instant(&m.sys).map_or(before_deadline, |t| t.min(before_deadline));
            m.sys.move_clock(next.max(m.sys.now() + 1));
        }
        assert_eq!(
            m.sys.canonical().beacons[1].panic,
            None,
            "node 1 must not self-accuse before first_entry + START_NS (at now={})",
            m.sys.now()
        );
    }

    // Advance across START_NS: node 1 must accuse itself no earlier than
    // first_entry + START_NS and no later than one timer event after it.
    let mut accused_at = None;
    let mut node1_timers_after_deadline = 0usize;
    let limit = first_entry + START_NS + 50 * MS;
    while m.sys.now() <= limit {
        if let Some(ev) = m
            .sys
            .events()
            .into_iter()
            .find(|&ev| !is_core2_start(&m.sys, ev))
        {
            let is_n1_timer = matches!(ev, Event::Timer { node: 1, .. });
            if is_n1_timer && m.sys.now() >= first_entry + START_NS {
                node1_timers_after_deadline += 1;
            }
            m.sys.fire(ev);
            assert_eq!(m.sys.check(), Ok(()));
            if m.sys.canonical().beacons[1].panic.is_some() {
                accused_at = Some(m.sys.now());
                break;
            }
        } else {
            let next = next_timer_instant(&m.sys).map_or(limit + 1, |t| t.min(limit + 1));
            if next > limit {
                break;
            }
            m.sys.move_clock(next.max(m.sys.now() + 1));
        }
    }
    let accused_at = accused_at.expect("node 1 must accuse itself after START_NS");
    assert!(
        accused_at >= first_entry + START_NS,
        "self-accusation happened early: {accused_at} < {}",
        first_entry + START_NS
    );
    assert_eq!(
        node1_timers_after_deadline, 1,
        "self-accusation must happen on the first timer event at/after START_NS"
    );
    assert_eq!(
        m.sys.canonical().beacons[1].panic,
        Some((CrashReason::CoreSilent as u64, generation, 1))
    );
}

#[test]
fn d171_checker_rejects_early_core_silent_self_accusation_for_unentered_core() {
    let (mut m, first_entry) = setup_staggered_fresh_start();
    assert!(m.sys.now() < first_entry + START_NS);
    let generation = m.sys.canonical().generation(1);
    m.sys.edit_table(|t| {
        t.beacons[1].panic = Some((CrashReason::CoreSilent as u64, generation, 1));
    });
    let err = m
        .sys
        .check()
        .expect_err("checker must reject early CoreSilent self-accusation for un-entered core");
    assert!(err.contains("D-171"), "unexpected error message: {err}");
}
