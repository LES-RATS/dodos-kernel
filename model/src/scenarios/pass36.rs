//! The scenarios of pass 36: the review of passes 33 and 34, decisions
//! D-163 and D-164 brought into the model. Each test is named after the
//! sentence it guards.

use super::pass17::kill;
use super::pass32::{FRAME, REPLY, recv, send};
use super::*;
use dodos_abi::FENCE_NS;

// What a death does, R3 (finding 1 of the review of passes 33 and 34): a
// pinned grant is kept while "its grantee is fenced or unfenced". A
// grantee that ends unfenced (its x86-64 core in VMX root operation takes
// no INIT) is not running: its granter's pinned entry is no fault, and
// R0's isolation of it holds as for a fenced one.
#[test]
fn a_grant_pinned_for_a_grantee_left_unfenced_is_no_fault() {
    let mut m = S::two();
    m.supervisor(0, m.t[0]);
    let frame = m.retype(0, ObjectKind::Frame, 1, FRAME);
    m.ok(0, frame);
    let reply = m.retype(1, ObjectKind::Reply, 0, REPLY);
    m.ok(1, reply);
    assert_eq!(m.call(1, recv()), WAITS);
    assert_eq!(m.call(0, send(1, FRAME, Rights::READ)), WAITS);
    m.settle();
    let entries = m.sys.node(0).grant_entries();
    assert_eq!(entries.len(), 1, "the grant to node 1");
    m.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, kill(1));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    let pinned = m.sys.node(0).grant_entries();
    assert_eq!(pinned.len(), 1);
    assert_eq!(
        pinned[0].1.3, 2,
        "the grant is pinned for the unfenced node"
    );
    assert!(m.sys.node(0).reclaims[&1].isolated, "R0 ran for node 1");
    assert_eq!(m.sys.check(), Ok(()));
}

// Reclaim, R0, and the request record (finding 3 of the review of passes
// 33 and 34): a request whose holder's gate answered it before the holder
// died ends on its record's state word, whatever the answer: *done* and
// *refused* end the call done (a refusal of the checks is answered done,
// D-134), *failed* ends it as a failed unit does, `busy`. Queued again, no
// holder would answer a record no longer *open*.

/// Node 0 attaches through the holder of the IOMMU unit, whose gate
/// answers the record *done* (only the holder's events fire) and which
/// then dies: how the waiting call ends. `answer` stands for the answer the
/// holder's gate wrote in the record's state word (*refused* for operands
/// that fail its checks, *failed* for a unit that did not confirm), written
/// over its *done* before it dies: a failed unit's own timer ends node 0's
/// call through the role word before any death, so the state is written
/// here rather than reached.
fn answered_then_the_holder_dies(answer: crate::table::RecordState) -> Option<Outcome> {
    use super::pass32::drive;
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    use crate::table::RecordState;
    let mut m = three();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let killer = (1..3).find(|&n| n != holder).expect("a third node");
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    let index = m.sys.node(0).unit_requests.iter().position(Option::is_some);
    let index = index.expect("the attach's entry");
    let answered = move |m: &S| m.sys.canonical().unit_records[0][index].state == RecordState::Done;
    assert!(drive(&mut m, &[holder], answered), "never answered");
    m.sys
        .edit_table(|t| t.unit_records[0][index].state = answer);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(m.woken(0).is_none(), "node 0 has not read the answer");
    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(holder));
    m.run(100 * MS);
    m.woken(0).cloned()
}

#[test]
fn a_request_its_dead_holder_refused_ends_done_on_its_record() {
    let woken = answered_then_the_holder_dies(crate::table::RecordState::Refused);
    assert_eq!(woken, Some(DONE));
}

#[test]
fn a_request_its_dead_holder_answered_failed_ends_busy_on_its_record() {
    let woken = answered_then_the_holder_dies(crate::table::RecordState::Failed);
    assert_eq!(woken, Some(Err(Error::Busy)));
}

// Gate words, the request record (finding 6 of the review of passes 33 and
// 34): the holder's gate answers only a record whose state word reads the
// serial the answer names with *open*. A late answer, after the requester
// opened the entry again with the next serial, or a second answer of the
// same serial, leaves the word as it is.
#[test]
fn the_holder_answers_a_request_record_only_from_its_serial_with_open() {
    use crate::rings::UnitOp;
    use crate::table::RecordState;
    let mut m = S::two();
    let op = UnitOp::Invalidate { domain: Some(1) };
    let state = |m: &S| m.sys.canonical().unit_records[0][3];
    let first = m.sys.edit_table(|t| t.open_unit_record(0, 3, 0, op));
    m.sys
        .edit_table(|t| t.answer_unit_record(0, 3, first, RecordState::Done));
    assert_eq!(state(&m).state, RecordState::Done);
    // A second answer of the same serial: the record is no longer open.
    m.sys
        .edit_table(|t| t.answer_unit_record(0, 3, first, RecordState::Failed));
    assert_eq!(state(&m).state, RecordState::Done);
    // The entry opened again; the late answer names the old serial.
    let second = m.sys.edit_table(|t| t.open_unit_record(0, 3, 0, op));
    assert_eq!(second, first + 1);
    m.sys
        .edit_table(|t| t.answer_unit_record(0, 3, first, RecordState::Failed));
    assert_eq!(
        (state(&m).serial, state(&m).state),
        (second, RecordState::Open)
    );
}

// D-163 (1), the states of a node, starting → live: "Every core whose word
// names the node reads *owned* without the mark: none reads *joining* and
// none is marked". Node 1 of two cores is sealed; the word of core 2 is
// marked under the seal (only a faulty kernel writes it there). The resume
// leaves that word as it is; core 1 enters and writes itself owned; the
// node stays starting, and is accused with stale after `START_NS` (finding
// 10 of the review of passes 33 and 34: it went live).
#[test]
fn a_resumed_node_with_a_marked_core_word_stays_starting_and_is_accused_stale() {
    use super::pass17::{resume, seal};
    use dodos_abi::{CoreState, CoreWord, Evidence, START_NS, SWEEP_NS};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    let word = |m: &S, core: usize| CoreWord::from_raw(m.sys.core_word(core)).unwrap();
    m.ok(0, seal(1));
    m.run(3 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let marked = word(&m, 2).marked();
    m.sys.edit_table(|t| t.core_words[2] = marked.raw());
    assert_eq!(m.sys.check(), Ok(()));
    m.ok(0, resume(1));
    assert_eq!(m.state(1), MemberState::Starting);
    assert_eq!(word(&m, 1).state(), CoreState::Joining);
    assert_eq!(word(&m, 2), marked, "left as it is");
    m.run(3 * MS);
    assert_eq!(word(&m, 1).state(), CoreState::Owned);
    assert_eq!(m.state(1), MemberState::Starting);
    m.run(START_NS + SWEEP_NS + 250 * MS);
    assert_eq!(m.sys.member(1).evidence(), Evidence::Stale);
}

// D-164 and D-165 (gap 58): `gone` for a node that is not live is the
// first check of the state: a frame over a table chunk, which the gate's
// reading refuses `busy` (G3 (2), D-156), sent to a node that is not live
// is `gone`; sent to a live node, `busy`.
#[test]
fn node_send_judges_gone_before_the_busy_of_a_frame_its_gate_refuses() {
    use super::pass31::{BLOCK, with_blocks};
    let (mut k, _, _) = with_blocks();
    k.ok(retype_from(BLOCK, ObjectKind::Frame, 1, 211));
    let frame = k.m.object_at(k.t, s(211)).expect("a frame");
    let (_, end) = k.m.table_floor(0)[0];
    crate::live(&mut k.m.frames, frame).base = end - k.m.geometry().page();
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message {
            caps: vec![crate::Transfer::Derive(s(211), Rights::READ.bits())],
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(k.m.clone().apply(k.t, send.clone()), Err(Error::Busy));
    let member = k.m.table.member(1);
    let fencing = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::Stale,
        dodos_abi::DRIVER_NONE,
        member.generation(),
    );
    k.m.table.members[1] = fencing;
    assert_eq!(k.m.apply(k.t, send), Err(Error::Gone));
}

// The long property run of pass 36 (seed 2011, also on b20f664): an event
// whose change list is observed twice (a `reply_recv` that moves a
// capability: its reply's part, then its receive's) lists a context
// twice, [A, B, A], and A's entry in its queue is the later one, after
// B's. Invariant 60 (the contexts that entered a queue in the last event
// entered in the order of the change list) read A at its first place and
// reported the state; its place is its last.
#[test]
fn a_context_listed_twice_in_one_event_is_judged_at_its_last_place() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.place_first(0);
    let second = k.thread_on(300, 0);
    let context = |k: &N, thread| k.m.threads[&thread].home.expect("a context");
    let entered = |k: &N, id| k.m.scheds[&id].queue.expect("in a queue").entered;
    let (mut a, mut b) = (context(&k, k.t), context(&k, second));
    // B entered before A, as now: the queues keep their order.
    if entered(&k, a) < entered(&k, b) {
        (a, b) = (b, a);
    }
    let from = k.m.entered;
    k.m.entered += 2;
    for (id, at) in [(b, from), (a, from + 1)] {
        let queued = crate::live(&mut k.m.scheds, id).queue.as_mut();
        queued.expect("in a queue").entered = at;
    }
    k.m.event.entered_from = from;
    k.m.event.changes = vec![a, b, a];
    assert_eq!(k.m.check(), Ok(()));
    // Listed once, before B, and entered after it: reported.
    k.m.event.changes = vec![a, b];
    assert_eq!(k.m.check(), Err("60: order of entry and the change list"));
}
