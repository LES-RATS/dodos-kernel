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
