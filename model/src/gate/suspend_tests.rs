//! EXPERIMENTAL: the suspend draft's questions, answered from the model
//! (not in the conformance set: ignored by `verify`, run with
//! `--ignored gate::suspend_tests --nocapture`).

use super::suspend::*;

const BASE: Variant = Variant {
    serve_before_park: true,
    abandon_by_any_running: false,
    watchdog: false,
    firmware_may_return: true,
    start_before_units: false,
    alarm_wakes_parked: false,
    stub_checks_member: true,
    start_free_cores: false,
    self_accusation: false,
};

fn report(name: &str, start: &State, v: Variant) -> Findings {
    let f = explore(start, v);
    eprintln!(
        "{name}: {} states; slept {}; healthy states that can no longer sleep {}; isolation break {}; wedge {}; healthy wedge {}",
        f.states,
        f.slept,
        f.healthy_no_sleep,
        f.isolation
            .as_ref()
            .map_or("none".to_string(), |p| format!("{p:?}")),
        f.wedge
            .as_ref()
            .map_or("none".to_string(), |(p, _)| format!("{} steps", p.len())),
        f.healthy_wedge
            .as_ref()
            .map_or("none".to_string(), |p| format!("{p:?}")),
    );
    f
}

/// Three nodes of one core; two units, unit 0 held by node 0 (*S*), unit 1
/// by node 1; node 1 has a record open to unit 0 and node 0 one to unit 1:
/// each holder is itself a requester (Q3).
fn holders_requesting() -> State {
    State::new(3, 1, 2, &[(1, 0), (0, 1)])
}

#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn fixed_point_1_the_units_before_any_core() {
    let start = State::new(2, 1, 1, &[]);
    let kept = report("fixed point 1 kept", &start, BASE);
    assert!(kept.isolation.is_none());
    let removed = report(
        "fixed point 1 removed (D-151)",
        &start,
        Variant {
            start_before_units: true,
            ..BASE
        },
    );
    assert!(
        removed.isolation.is_some(),
        "starting cores before the units broke nothing"
    );
}

#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q3_holders_that_are_requesters() {
    let start = holders_requesting();
    let ordered = report("Q3 serve before park", &start, BASE);
    assert!(ordered.healthy_wedge.is_none(), "a healthy machine wedged");
    assert!(ordered.slept);
    let unordered = report(
        "Q3 park at once",
        &start,
        Variant {
            serve_before_park: false,
            ..BASE
        },
    );
    let _ = unordered;
}

#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q4_who_abandons_if_s_hangs() {
    let start = State::new(3, 1, 1, &[]);
    let only_s = report("Q4 abandon by S only", &start, BASE);
    let any = report(
        "Q4 abandon by any running core",
        &start,
        Variant {
            abandon_by_any_running: true,
            ..BASE
        },
    );
    let watchdog = report(
        "Q4 a watchdog core of the next live node",
        &start,
        Variant {
            watchdog: true,
            abandon_by_any_running: true,
            ..BASE
        },
    );
    let alarm = report(
        "Q4 parked cores woken by an alarm at the bound",
        &start,
        Variant {
            alarm_wakes_parked: true,
            abandon_by_any_running: true,
            ..BASE
        },
    );
    eprintln!(
        "Q4 alarm wedge: {:?}",
        alarm.wedge.map(|(p, s)| (p, s.cores))
    );
    eprintln!(
        "Q4 wedges: S only {:?}\n  any running {:?}\n  watchdog {:?}",
        only_s.wedge.map(|(p, s)| (p, s.cores)),
        any.wedge.map(|(p, s)| (p, s.cores)),
        watchdog.wedge.map(|(p, s)| (p, s.cores)),
    );
}

#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q5_a_firmware_that_returns() {
    let start = State::new(2, 1, 1, &[]);
    let f = report("Q5 firmware returns", &start, BASE);
    assert!(f.isolation.is_none() && f.healthy_wedge.is_none());
}

// Q2: the park's bound. Q3's exploration shows that serving never waits for
// another record (a holder serves whatever its own records), so the worst
// case is one hop: the notice (the ring's interrupt, or the sweep if it is
// lost), a core's answer, and one unit request (the unit's wait and two
// periods of the reserve). `STALE_NS` is below the sweep for the defaults,
// so it cannot be the bound: a lost notice alone outlasts it.
#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q2_the_park_bound() {
    use dodos_abi::{
        CORE_ANSWER_NS, CORE_ANSWER_RANGE_NS, IOMMU_WAIT_NS, RECOVERY_PERIOD_NS, STALE_NS,
        STALE_RANGE_NS, SWEEP_NS, SWEEP_RANGE_NS,
    };
    let bound = |sweep: u64, answer: u64| sweep + answer + IOMMU_WAIT_NS + 2 * RECOVERY_PERIOD_NS;
    let default = bound(SWEEP_NS, CORE_ANSWER_NS);
    let widest = bound(SWEEP_RANGE_NS.1, CORE_ANSWER_RANGE_NS.1);
    let narrowest = bound(SWEEP_RANGE_NS.0, CORE_ANSWER_RANGE_NS.0);
    eprintln!(
        "Q2: bound = sweep + core answer + IOMMU wait + 2 reserve periods: \
         {default} ns by default, {narrowest} to {widest} ns over the ranges; \
         STALE_NS {STALE_NS} ns (range {STALE_RANGE_NS:?})"
    );
    assert!(STALE_NS < default, "STALE_NS covers a lost notice");
    // Even the widest STALE_NS is below the narrowest bound but for a
    // sweep at its least: a start-up check `STALE_NS >= bound` would refuse
    // every default machine.
    assert!(STALE_RANGE_NS.1 < bound(SWEEP_NS, CORE_ANSWER_NS));
    // One hop: no healthy state of the holders-that-request machine needs
    // a record served before another can be (each serve is enabled as soon
    // as its holder runs).
    let start = holders_requesting();
    let f = report("Q2 one hop", &start, BASE);
    assert!(f.healthy_wedge.is_none());
}

// Q6: the resume path's authority. No core runs while the word reads
// *asleep* (every other core confirmed parked at the last call, which then
// parks its own), under every variant that holds, the alarm of Q4's answer
// included; so only the firmware's resume vector reaches the resume path,
// and the word *asleep* is the whole condition it needs. A node's gate can
// never call it: no node runs then.
#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q6_nothing_runs_while_asleep() {
    let start = State::new(3, 1, 1, &[(1, 0)]).with_free_core();
    for (name, v) in [
        ("Q6 base", BASE),
        (
            "Q6 with the alarm",
            Variant {
                alarm_wakes_parked: true,
                abandon_by_any_running: true,
                ..BASE
            },
        ),
        (
            "Q6 with self-accusation",
            Variant {
                self_accusation: true,
                ..BASE
            },
        ),
    ] {
        let f = report(name, &start, v);
        if let Some((path, state)) = &f.no_sleep_example {
            eprintln!(
                "{name}: can no longer sleep after {path:?}: {:?}",
                state.cores
            );
        }
        assert!(f.asleep_runner.is_none(), "{name}: a core runs asleep");
        assert!(f.slept, "{name}");
    }
}

// Q7: the entry stub tells a live node's return from a fresh start by the
// start request the resume path wrote (the node, its generation, the
// resume bit) against the member word: live with that generation, it
// returns to the node; starting (a fresh start's request names the next
// generation), it enters fresh; anything else, it parks again. A stub that
// trusts the resume bit alone returns a core into a node that accused
// itself between the resume path's start and the core's entry.
#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q7_the_entry_stub_reads_the_member_word() {
    let start = State::new(2, 2, 1, &[]);
    let accusing = Variant {
        self_accusation: true,
        ..BASE
    };
    let checked = report("Q7 stub reads the member word", &start, accusing);
    assert!(checked.entered_not_live.is_none());
    assert!(checked.healthy_wedge.is_none());
    let trusting = report(
        "Q7 stub trusts the resume bit",
        &start,
        Variant {
            stub_checks_member: false,
            ..accusing
        },
    );
    eprintln!("Q7 break: {:?}", trusting.entered_not_live);
    assert!(trusting.entered_not_live.is_some());
}

// Q10: the firmware resumes on a core of no node (x86-64: a class-0
// bootstrap core left out of every node, parked since the start). The
// resume path runs there, enables the units, starts the live nodes' cores
// and parks again; the protocol ends. Starting that core as a node's would
// run it for no node.
#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q10_a_resume_on_a_core_of_no_node() {
    let start = State::new(2, 1, 1, &[]).with_free_core();
    let f = report("Q10 resume on a free core", &start, BASE);
    assert!(
        f.ended_after_free_wake,
        "a resume on the free core never ends"
    );
    assert!(f.free_runs.is_none() && f.isolation.is_none() && f.healthy_wedge.is_none());
    let started = report(
        "Q10 the free core started",
        &start,
        Variant {
            start_free_cores: true,
            ..BASE
        },
    );
    assert!(started.free_runs.is_some());
}

// Q9: an ITS that keeps its collection and device tables on chip loses
// them across the sleep. In the gate model: the device's ITS word reads
// *attached* and the ITS mapped it; the sleep empties the ITS (and the
// units' caches); the rebuild issues the MAPD again for every ITS word of a
// live node reading *attached* (D-152 item 8). With no ITS command in
// flight at the park (holders serve before they park, Q3), the machine
// holds (G) after the rebuild and every attached word is mapped again;
// a command in flight at the sleep is lost with the queue, and a mapping
// whose word already reads *attached* is reissued by the rebuild, so
// nothing but the answer is lost.
#[test]
#[ignore = "experimental: the draft of whole-machine suspend"]
fn q9_an_its_with_its_tables_on_chip() {
    use super::Machine;
    use super::devices::Bound;
    use super::faulty::Op;
    let sleep_and_rebuild = |m: &mut Machine| {
        m.dev.its_mapped.clear();
        m.iotlb.clear();
        m.dev.context_cache.clear();
        m.dev.iec.clear();
        for submission in m.unit.submissions.iter_mut().filter(|s| !s.complete) {
            submission.its.clear();
        }
        for d in 0..usize::from(super::devices::DEVICES) {
            let (state, table, k) = m.dev.its[d];
            let live =
                m.dev.ids[d].is_some_and(|(o, _)| m.gate.members[usize::from(o)].state.writes());
            if state == Bound::Attached && live {
                m.dev.its_mapped.insert(d as u16, (table, k));
            }
        }
    };
    let mut m = Machine::new(2, 2, 8).with_units();
    for op in [
        Op::ItsRetype {
            n: 1,
            first: 12,
            k: 0,
        },
        Op::ItsClaim {
            n: 1,
            device: 1,
            first: 12,
            event: 0,
            entry: 1,
        },
        Op::Accept {
            h: 0,
            slot: 1,
            entry: 1,
        },
        Op::Submit { h: 0 },
    ] {
        m.step(&op).expect("the mapping asked and queued");
    }
    // Asleep with the MAPD still queued: lost with the queue, reissued by
    // the rebuild since the word reads *attached*.
    sleep_and_rebuild(&mut m);
    m.check().expect("the rebuild holds (G)");
    assert_eq!(m.dev.its_mapped.get(&1), Some(&(12, 0)));
    eprintln!("Q9: a lost MAPD of an attached word is reissued by the rebuild");
}
