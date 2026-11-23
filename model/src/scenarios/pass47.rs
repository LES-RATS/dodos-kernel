//! Pass 47: review of pass 46 (every due event of every node before a
//! call, the tests that survived their mutations, the liveness check).

use super::pass46::draining;
use super::*;
use crate::cores::{Completes, Flush, Request, RequestState};

// ---------------------------------------------------------------------------
// 1. Every due event before any call, on every node (findings 1, 5, 6)
// ---------------------------------------------------------------------------

/// Node 0's core drains 17 timers due at one instant; a call of node 1 at
/// that instant takes them first.
#[test]
fn a_call_on_one_node_takes_the_due_events_of_another_node_first() {
    let mut m = S::two();
    draining(&mut m);
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(!m.sys.node(0).timer_draining(0));
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 0);
}

/// `review46::a_call_runs_before_a_core_answer_deadline`, turned around:
/// the `CORE_ANSWER_NS` due event of node 0's request is taken before a
/// call at its instant, and node 0 accuses itself there.
#[test]
fn a_call_takes_a_core_answer_deadline_of_another_node_first() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let now = m.sys.now();
    {
        let node = m.sys.node_mut(0);
        node.cores[0].request = Some(Request {
            flush: Flush::Nothing,
            asked: 0b10,
            acked: 0,
            started: now,
            completes: Completes::Barrier(0),
            waiter: None,
        });
        node.cores[0].request_state = RequestState::InUse;
    }
    m.sys.move_clock(now + m.sys.canonical().core_answer_ns);
    assert!(m.sys.node(0).due_now(0));
    assert!(!m.sys.node(0).timer_draining(0));
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(m.sys.node(0).table.beacons[0].panic.is_some());
}

/// The sweep is a due event of the first core's timer queue (The sweep):
/// a call of node 1 at its instant comes after node 0's sweep.
#[test]
fn a_call_takes_the_sweep_of_another_node_first() {
    let mut m = S::two();
    m.settle();
    let sweep = m.sys.node(0).next_sweep;
    m.sys.move_clock(sweep);
    assert!(m.sys.node(0).reserve_timer_due());
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(m.sys.node(0).next_sweep > sweep);
}
