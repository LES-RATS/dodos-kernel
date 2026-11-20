//! Pass 33: the decisions D-158 and D-159 in the kernel of a node and in
//! the machine. Each scenario names the sentence it holds, and fails
//! without the rule.

use super::pass17::kill;
use super::pass28::{B0, RESET, debugged};
use super::pass32::{FRAME, drive, from_node_1, pump, send};
use super::units::{IOMMU_ROLE, attached, role_slot};
use super::*;
use crate::rings::Body;
use dodos_abi::{FaultKind, NodeMessage, SCHED_MAX_DEBT, SCHED_MAX_PERIOD_NS, SCHED_MIN_BUDGET_NS};

const B: u64 = SCHED_MIN_BUDGET_NS;
const T: u64 = SCHED_MAX_PERIOD_NS;

/// A second thread whose context has the smallest budget and the longest
/// period runs alone on the node's one core (the first thread waits);
/// then the clock jumps by `stop` (a stop of the machine is time that
/// passed for the scheduler) and the core takes its timer interrupt. The
/// node, the context, and its deadline before the jump.
fn charged_after_a_stop(stop: u64) -> (N, ObjId, u64) {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.place_first(0);
    let mut calls = thread_calls(k.ut, k.m.node, 300, 0);
    for call in &mut calls {
        if let Syscall::SchedConfigure { budget, period, .. } = call {
            (*budget, *period) = (B, T);
        }
    }
    calls.into_iter().for_each(|call| k.ok(call));
    k.ok(k.retype(ObjectKind::Notify, 0, 211));
    assert_eq!(k.call(k.t, Syscall::Wait(s(211), 0)), WAITS);
    let other = k.m.object_at(k.t, s(300)).expect("the thread");
    let context = k.m.context_of(other).expect("its context");
    assert_eq!(k.m.cores[0].running, Some((context, true)));
    let sched = &k.m.scheds[&context];
    assert_eq!((sched.budget, sched.remaining), (B, B as i64));
    let deadline = sched.deadline;
    k.m.set_now(k.m.now + stop);
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    (k, context, deadline)
}

/// The context's remaining budget once its replenishment instant came.
fn replenished(k: &mut N, context: ObjId) -> i64 {
    let (instant, _) = k.m.scheds[&context].throttled.expect("throttled");
    k.m.set_now(instant);
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    k.m.scheds[&context].remaining
}

// D-159 (2), gap 50 (Exhaustion and replenishment): "Let n = min(1 +
// ⌊−r / B⌋, SCHED_MAX_DEBT)". A debt of exactly `SCHED_MAX_DEBT` budgets
// is not bounded: the instant is d + (n − 1) × T and the replenishment
// pays the whole debt, r += n × B.
#[test]
fn a_debt_of_exactly_sched_max_debt_budgets_is_paid_whole() {
    let (mut k, context, d) = charged_after_a_stop(SCHED_MAX_DEBT * B + 3);
    let (instant, debt) = k.m.scheds[&context].throttled.expect("throttled");
    assert_eq!((debt.budgets, debt.forgiven), (SCHED_MAX_DEBT, false));
    assert_eq!(instant, d + (SCHED_MAX_DEBT - 1) * T);
    assert_eq!(replenished(&mut k, context), B as i64 - 3);
    assert_eq!(k.m.scheds[&context].deadline, d + SCHED_MAX_DEBT * T);
}

// D-159 (2), gap 50: one budget more and n is bounded; "r = B when n was
// bounded by SCHED_MAX_DEBT: the rest of the debt is forgiven". Without
// the rule the context came back with r = −3 and was exhausted again.
#[test]
fn a_debt_beyond_sched_max_debt_budgets_is_bounded_and_the_rest_forgiven() {
    let (mut k, context, d) = charged_after_a_stop((SCHED_MAX_DEBT + 1) * B + 3);
    let (instant, debt) = k.m.scheds[&context].throttled.expect("throttled");
    assert_eq!((debt.budgets, debt.forgiven), (SCHED_MAX_DEBT, true));
    assert_eq!(instant, d + (SCHED_MAX_DEBT - 1) * T);
    assert_eq!(replenished(&mut k, context), B as i64);
    assert_eq!(k.m.scheds[&context].throttled, None);
}

// D-159 (2), gap 50: "with the smallest budget and the longest period, a
// stop of 22 minutes charged to a context made n × T pass 64 bits". A
// stop of 50 minutes, at which the model panicked, gives the same bounded
// exhaustion.
#[test]
fn a_stop_of_fifty_minutes_charged_to_the_smallest_budget_overflows_nothing() {
    let (mut k, context, d) = charged_after_a_stop(50 * 60 * 1_000 * MS);
    let (instant, debt) = k.m.scheds[&context].throttled.expect("throttled");
    assert_eq!((debt.budgets, debt.forgiven), (SCHED_MAX_DEBT, true));
    assert_eq!(instant, d + (SCHED_MAX_DEBT - 1) * T);
    assert_eq!(replenished(&mut k, context), B as i64);
}

/// The fault of kind 9 recorded in `thread`: words 3 and 4.
fn breakpoint_of(k: &N, thread: ObjId) -> Option<(u64, u64)> {
    k.m.threads[&thread]
        .recorded
        .filter(|&(kind, _)| kind == FaultKind::HardwareBreakpoint as u32)
        .map(|(_, words)| (words[3], words[4]))
}

// D-159 (3), gap 51 (Debugging, kind 9 from a #DB at CPL0): a kept kind 9
// is "raised at the end of the event in which the thread is runnable with
// a configured context"; a thread with no context and no mark "does not
// run, and raises nothing", so it keeps its kind 9. The #DB lands on the
// entry of the call by which the thread unbinds its own context: nothing
// is raised, the word is kept, and the fault is raised at the end of the
// event that binds a context to it again.
#[test]
fn a_kept_breakpoint_fault_waits_for_a_configured_context() {
    let mut k = debugged();
    k.place_first(0);
    let other = k.thread_on(300, 0);
    let thread = k.t;
    let unbind = Syscall::SchedUnbind(s(START_SLOT_SCHED));
    assert_eq!(k.m.call_with_debug(thread, B0, unbind), Some(DONE));
    assert_eq!(k.m.check(), Ok(()));
    assert!(!k.m.threads[&thread].lost, "not marked");
    assert_eq!(breakpoint_of(&k, thread), None, "raised with no context");
    assert!(k.m.threads[&thread].debug_kept.is_some());
    let bind = Syscall::SchedBind {
        sched: s(START_SLOT_SCHED),
        thread: SlotAddr::own(dodos_abi::START_SLOT_THREAD),
    };
    assert_eq!(k.call(other, bind), DONE);
    assert_eq!(breakpoint_of(&k, thread), Some((B0, RESET | B0)));
    assert_eq!(k.m.threads[&thread].debug_kept, None);
}

// D-159 (3), gap 51: a context that is not configured is no context to
// run on ("a thread whose own context was unbound or is not configured
// does not run, and raises nothing"): the kept kind 9 waits for its
// configuration.
#[test]
fn a_kept_breakpoint_fault_waits_while_its_context_is_not_configured() {
    let mut k = debugged();
    k.place_first(0);
    let other = k.thread_on(300, 0);
    let thread = k.t;
    let configure = |budget, period| Syscall::SchedConfigure {
        sched: s(START_SLOT_SCHED),
        node: node_cap(0),
        core: 0,
        budget,
        period,
        class: 1,
    };
    assert_eq!(k.m.call_with_debug(thread, B0, configure(0, 0)), Some(DONE));
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(breakpoint_of(&k, thread), None, "raised with no context");
    assert!(k.m.threads[&thread].debug_kept.is_some());
    assert_eq!(k.call(other, configure(MS, 10 * MS)), DONE);
    assert_eq!(breakpoint_of(&k, thread), Some((B0, RESET | B0)));
}

// D-159 (4) (Grants, release by the holder): "the grant may be one whose
// send Y has not answered yet: X frees the entry all the same, and the
// answer that comes later, naming a serial no entry holds unanswered for
// Y, changes nothing". Node 0 grants a frame to node 1; node 1's `Release`
// reaches node 0 before its `SendAnswer`: the entry is freed, with its
// anchor; the answer, "delivered" with a use index, then frees its credit
// and wakes the sender, and writes no entry.
#[test]
fn a_release_of_a_grant_not_yet_answered_frees_it_and_the_answer_changes_nothing() {
    let mut m = S::two();
    let retype = m.retype(0, ObjectKind::Frame, 1, FRAME);
    m.ok(0, retype);
    let rights = Rights::READ.union(Rights::WRITE);
    assert_eq!(m.call(0, send(1, FRAME, rights)), WAITS);
    let (grant, serial) = {
        let entries = m.sys.node(0).grant_entries();
        assert_eq!(entries.len(), 1);
        (entries[0].0, entries[0].1.2)
    };
    assert!(!m.sys.canonical().grants[0][&grant].answered);
    let (credit, credit_serial) = m.sys.node(0).peers[1]
        .credits
        .iter()
        .enumerate()
        .find_map(|(i, c)| c.as_ref().map(|c| (i as u8, c.serial)))
        .expect("the send's credit");
    let release = Body::Release { grant, serial };
    m.sys
        .inject(1, 0, from_node_1(NodeMessage::Release, None, release));
    let freed = |m: &S| m.sys.node(0).grant_entries().is_empty();
    assert!(drive(&mut m, &[0], freed), "the release frees the entry");
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.sys.node(0).refused.0, 0, "the release was refused");
    let before = m.sys.canonical().grants[0].clone();
    // The answer of the send, later: delivered, with a use index.
    let answer = Body::SendAnswer {
        delivered: true,
        dropped: false,
        uses: vec![Some(0)],
    };
    let answer = from_node_1(
        NodeMessage::SendAnswer,
        Some((credit, credit_serial)),
        answer,
    );
    m.sys.inject(1, 0, answer);
    let answered = |m: &S| m.sys.node(0).peers[1].credits[credit as usize].is_none();
    assert!(drive(&mut m, &[0], answered), "the answer frees its credit");
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.sys.canonical().grants[0], before, "the answer wrote");
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

// D-159 (5) (Roles, the planned hand-over of a unit role): "never while X
// rebuilds u: `role_hand` is then `busy`". Node 2 took over the unit of
// node 1, killed, and is asking the other nodes for their reports when it
// is asked to hand the role to node 3: `busy`, the rebuild untouched; once
// the rebuild has ended the same call is accepted.
#[test]
fn role_hand_is_busy_while_its_holder_rebuilds_the_unit() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    attached(&mut m, 0);
    m.supervisor(2, m.t[2]);
    m.sys.apply(2, m.t[2], kill(1)).expect("the kill");
    let to = m.sys.now() + 400 * MS;
    let asking = |m: &S| {
        let rebuild = m.sys.node(2).rebuilds.get(&IOMMU_ROLE);
        rebuild.is_some_and(|r| r.node > 0)
    };
    while !asking(&m) {
        assert!(m.sys.now() < to && pump(&mut m, to), "no rebuild");
    }
    let hand = Syscall::RoleHand {
        role: role_slot(&m, 2, IOMMU_ROLE),
        node: node_cap(3),
    };
    let before = m.sys.node(2).rebuilds.clone();
    m.no(2, hand.clone(), Error::Busy);
    assert_eq!(m.sys.node(2).rebuilds, before);
    assert!(!m.sys.node(2).handing.contains_key(&IOMMU_ROLE));
    let end = m.sys.now() + 300 * MS;
    while !m.sys.node(2).rebuilds.is_empty() {
        assert!(
            m.sys.now() < end && pump(&mut m, end),
            "the rebuild never ends"
        );
    }
    let outcome = m.call(2, hand);
    assert!(
        outcome == WAITS || matches!(outcome, Ok(Output::Epoch(_))),
        "{outcome:?}"
    );
}

/// Node 0 of two cores beside node 1 of one; node 0 took a free chunk as
/// the block of slot `BLOCK`, and a second thread runs on its core 1. The
/// node, the second thread and the chunk.
fn block_taken() -> (N, ObjId, u64) {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    k.place_first(0);
    let other = k.thread_on(300, 1);
    k.interrupts();
    let shift = k.m.geometry().chunk_shift();
    let unclaimed = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let free = (0..1u64 << (k.m.machine.phys_bits - shift))
        .find(|&c| k.m.ownership(c) == unclaimed)
        .expect("a free chunk");
    k.ok(Syscall::MemoryTake {
        control: s(CONTROL),
        kind: dodos_abi::MEMORY_TAKE_RUN,
        place: free << shift,
        size: u64::from(shift),
        destination: s(BLOCK),
    });
    (k, other, free)
}

const CONTROL: u32 = dodos_abi::START_SLOT_MEMORY_CONTROL;
const BLOCK: u32 = 220;

fn give_block() -> Syscall {
    Syscall::MemoryGive {
        control: s(CONTROL),
        untyped: s(BLOCK),
        node: 1,
    }
}

// D-159 (1), gap 49 (The machine table, releasing and giving): "A give or
// a destruction of a block is `busy` while a request of the node already
// names a give or a destruction of one of its chunks". Core 0 destroys
// the block: its request asks core 1 and no word is written yet. A
// healthy kernel never asks its gate twice (`memory_give` of the dying
// block is `gone`, by the order of checks); a faulty one that calls its
// gate for a give of the chunk is refused. Once core 1 has answered, the
// destruction's completion writes the word *free* with the same owner.
#[test]
fn a_chunk_whose_destruction_is_in_flight_is_not_given_by_the_gate() {
    let (mut k, other, chunk) = block_taken();
    let taken = k.m.ownership(chunk);
    assert_eq!(k.call(k.t, Syscall::CapDelete(s(BLOCK))), Err(Error::Again));
    assert!(k.m.cores[0].request.is_some(), "no request in flight");
    assert_eq!(k.m.ownership(chunk), taken);
    assert_eq!(k.call(other, give_block()), Err(Error::Gone));
    assert!(!k.m.gate_give_chunk(chunk, 1), "given under a destruction");
    assert_eq!(k.m.ownership(chunk), taken);
    k.interrupts();
    k.ok(Syscall::CapDelete(s(BLOCK)));
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Node(k.m.node));
    assert_eq!(k.m.ownership(chunk), free.raw());
}

// D-159 (1), gap 49: the same under a give in flight, and the give's
// completion writes the word for the node it named.
#[test]
fn a_chunk_whose_give_is_in_flight_is_not_given_again_by_the_gate() {
    let (mut k, _, chunk) = block_taken();
    let taken = k.m.ownership(chunk);
    assert_eq!(k.call(k.t, give_block()), WAITS);
    assert!(k.m.cores[0].request.is_some(), "no request in flight");
    assert!(!k.m.gate_give_chunk(chunk, 2), "given twice");
    assert_eq!(k.m.ownership(chunk), taken);
    k.interrupts();
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Node(1));
    assert_eq!(k.m.ownership(chunk), free.raw());
}
