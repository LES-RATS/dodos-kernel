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

#[test]
fn timer_set_at_now_fires_immediately_and_check_47_requires_hw_timer_at_now() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.ok(k.retype(ObjectKind::Notify, 0, 150));
    k.ok(k.retype(ObjectKind::Timer, 0, 151));
    let now = k.m.now;
    let out = k.m.apply(
        k.t,
        Syscall::TimerSet {
            timer: s(151),
            notify: s(150),
            deadline: now,
            tolerance: 0,
        },
    );
    assert_eq!(out, Ok(Output::WasArmed(0)));
    assert_eq!(k.m.check(), Ok(()));
    // Under the reviewer's mutation (`instant < self.now` in `Model::timer_set`),
    // the timer stays armed at `now` while `hw_timer` is strictly after `now`,
    // so `k.m.check()` fails with `Err("47: armed timer")`.
    let timer = k.m.object_at(k.t, s(151)).expect("timer");
    assert!(k.m.timers[&timer].is_none());
}

#[test]
fn checks_43_and_47_reject_at_now_without_hardware_timer_due_at_now() {
    // Check 47: an armed timer at `now` is accepted only while `hw_timer <= Some(now)`.
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.ok(k.retype(ObjectKind::Notify, 0, 150));
    k.ok(k.retype(ObjectKind::Timer, 0, 151));
    let deadline = k.m.now + 5 * MS;
    assert_eq!(
        k.call(
            k.t,
            Syscall::TimerSet {
                timer: s(151),
                notify: s(150),
                deadline,
                tolerance: 0,
            },
        ),
        Ok(Output::WasArmed(0))
    );
    k.m.set_now(deadline);
    // Hardware timer was programmed to `deadline == now`: accepted.
    assert_eq!(k.m.cores[0].hw_timer, Some(deadline));
    assert_eq!(k.m.check(), Ok(()));
    // Clear or move hardware timer past `now`: check 47 must reject.
    k.m.cores[0].hw_timer = Some(deadline + 1);
    assert_eq!(k.m.check(), Err("47: armed timer".into()));
    k.m.cores[0].hw_timer = None;
    assert_eq!(k.m.check(), Err("47: armed timer".into()));

    // Check 43: a throttled context at `now` is accepted only while `hw_timer <= Some(now)`.
    let mut k2 = N::on(&one_node(Arch::X86_64, 1));
    k2.place_first(0);
    let context = k2.m.threads[&k2.t].home.expect("home context");
    let now = k2.m.now;
    {
        let sched = k2.m.scheds.get_mut(&context).unwrap();
        sched.remaining = 0;
        sched.throttled = Some((now, dodos_abi::sched_debt(0, 1)));
        let entered = sched.queue.map(|q| q.entered).unwrap_or(0);
        sched.queue = Some(crate::Queued {
            queue: crate::Queue::Slack(sched.class),
            key: now,
            entered,
        });
    }
    k2.m.cores[0].running = Some((context, false));
    k2.m.cores[0].hw_timer = Some(now);
    assert_eq!(k2.m.check(), Ok(()));
    k2.m.cores[0].hw_timer = Some(now + 1);
    assert_eq!(
        k2.m.check(),
        Err("43: parameters and state of a context".into())
    );
    k2.m.cores[0].hw_timer = None;
    assert_eq!(
        k2.m.check(),
        Err("43: parameters and state of a context".into())
    );
}

#[test]
fn exp_1c_nonzero_asleep_on_awake_core_fails_system_check() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    let tables1 = table_slot(m.sys.node(1), m.t[1]);
    m.ok(1, m.retype(1, ObjectKind::ASpace, 0, 150));
    m.ok(1, retype_from(ut1, ObjectKind::Frame, 1, 151));
    m.ok(
        1,
        Syscall::ASpaceMap(crate::Map {
            space: s(150),
            frame: s(151),
            untyped: s(tables1),
            address: 0x1000_0000,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    m.ok(1, retype_from(ut1, ObjectKind::Thread, 0, 160));
    m.ok(1, retype_from(ut1, ObjectKind::Sched, 0, 161));
    m.ok(
        1,
        Syscall::ThreadConfigure {
            thread: s(160),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(150),
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
            budget: 5 * MS,
            period: 10 * MS,
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
    m.sys.hang(2, false);
    assert_eq!(m.call(1, Syscall::ASpaceUnmap(s(151))), WAITS);
    let due =
        m.sys.node(1).cores[0].request.as_ref().unwrap().started + m.sys.canonical().core_answer_ns;
    m.sys.move_clock(due + 5 * MS);
    m.sys.node_mut(1).cores[0].asleep = 10 * MS;
    assert_eq!(
        m.sys.check(),
        Err("SLEEP: core.asleep disagrees with the hardware sleep state".into())
    );
}
