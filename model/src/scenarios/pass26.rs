//! Pass 26, D-144: the four gaps of pass 25 decided. Gap 20: nothing but an
//! answer is written toward a node starting afresh until it is live (its
//! fresh start sets its consumer indices to the producers'); a resuming
//! node, whose member word carries the resume bit, is written to as a live
//! node.

use super::pass17::{fresh, kill, resume, seal};
use super::units::IOMMU_ROLE;
use super::*;
use crate::system::Hardware;

/// Four nodes of one core each; node 3 dead, its core parked.
fn four_with_3_dead() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    m.ok(0, kill(3));
    m.run(100 * MS);
    assert_eq!(m.state(3), MemberState::Dead);
    assert!(matches!(m.sys.hardware(3), Hardware::Stub { .. }));
    m
}

/// The node that rebuilds the IOMMU role, and its rebuild's cursor.
fn rebuilding(m: &S) -> Option<(u16, crate::units::Rebuild)> {
    (0..4u16).filter(|&n| m.sys.has(n)).find_map(|n| {
        m.sys
            .node(n)
            .rebuilds
            .get(&IOMMU_ROLE)
            .map(|r| (n, r.clone()))
    })
}

/// The holder of the IOMMU role killed; at its fence node 3 is started
/// afresh, then events until the holder's successor rebuilds the role
/// (with a unit that works and every node answering at once, a rebuild
/// ends within one event: node 3 must be starting when it begins).
fn take_over(m: &mut S, killer: Option<u16>) -> u16 {
    let holder = m.sys.role(IOMMU_ROLE).holder();
    assert!(holder != 0 && holder != 3);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    m.ok(0, kill(holder));
    while m.state(holder) != MemberState::Fenced {
        m.run(10_000);
    }
    start_3(m, killer);
    let taken = |m: &S| rebuilding(m).is_some() || m.sys.role(IOMMU_ROLE).epoch() != epoch;
    assert!(
        (0..5).any(|_| m.step_until(taken)) && rebuilding(m).is_some(),
        "no rebuild"
    );
    holder
}

/// Node 3 started afresh by node 0, its first wake lost, so that its start
/// lasts a retry of the starter.
fn start_3(m: &mut S, killer: Option<u16>) {
    m.sys.set_window(3);
    m.ok(0, fresh(3, 1 << 3));
    assert_eq!(m.state(3), MemberState::Starting);
    assert!(!m.sys.member(3).resuming());
    // The starter killed at once by `killer`, before its retry: nobody
    // wakes node 3 again.
    if let Some(killer) = killer {
        m.ok(killer, kill(0));
    }
}

// The combined exploration's seed 7 of pass 25: a rebuild asks every node
// that is starting for its report, and its `ReportAsk` to a node starting
// afresh, written before the node built its state, was skipped by the
// node's reset of its consumer indices; its credit was never answered and
// the rebuild waited for ever. It now waits in its sender until the node
// is live, and is served then (What crosses between nodes, a fresh start,
// D-144). The take-over and node 3's entry fall in one event here, so the
// wait itself is not seen from outside; the check after every event is.
#[test]
fn a_rebuilds_report_ask_to_a_node_starting_afresh_waits_and_is_served_once_it_is_live() {
    let mut m = four_with_3_dead();
    take_over(&mut m, None);
    // The check after each event holds CREDIT: no `ReportAsk` was written
    // to node 3 while it was starting, and none was skipped by its reset.
    m.run(50 * MS);
    assert_eq!(m.state(3), MemberState::Live);
    assert!(rebuilding(&m).is_none(), "the rebuild never ended");
}

// A node started afresh that never becomes live (its starter dies before
// its wake is taken) is accused at `START_NS`; a rebuild that waited for it
// goes on without it.
#[test]
fn a_rebuild_goes_on_without_a_node_whose_fresh_start_never_ends() {
    let mut m = four_with_3_dead();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let other = (1..3u16).find(|&n| n != holder).expect("a third node");
    take_over(&mut m, Some(other));
    m.run(dodos_abi::START_NS / 2);
    assert_eq!(m.state(3), MemberState::Starting);
    assert!(
        rebuilding(&m).is_some_and(|(_, r)| r.node == 3),
        "the rebuild did not wait"
    );
    m.run(dodos_abi::START_NS + dodos_abi::SWEEP_NS);
    assert_ne!(m.state(3), MemberState::Starting);
    assert!(rebuilding(&m).is_none(), "the rebuild never went on");
}

// A resume under a rebuild: node 2, sealed over a frame of node 0 mapped
// in its iospace behind the IOMMU unit, resumes after node 0's death; its
// R2 asks the unit's holder to invalidate, the holder dies, and its
// successor's rebuild asks node 2 for its report. Were the `ReportAsk` to
// wait for node 2 to be live, it would wait for R2, which waits for the
// rebuild: the resume bit makes node 2 written to as a live node, so both
// end (D-144).
#[test]
fn a_resume_under_a_rebuild_reports_and_both_end() {
    use super::units::{IOSPACE, attached};
    let mut m = S::on(&machine(Arch::X86_64, 5, &[&[0], &[1], &[2], &[3], &[4]]));
    let holder = m.sys.role(IOMMU_ROLE).holder();
    assert_eq!(holder, 1);
    attached(&mut m, 2);
    // Node 0 grants node 2 a frame, which node 2 maps in its iospace.
    let reply = m.retype(2, ObjectKind::Reply, 0, 100);
    m.ok(2, reply);
    let frame = m.retype(0, ObjectKind::Frame, 1, 101);
    m.ok(0, frame);
    let recv = Syscall::Recv {
        endpoint: s(dodos_abi::START_SLOT_MAILBOX),
        reply: s(100),
        range: crate::Range {
            first: s(110),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(m.call(2, recv), WAITS);
    let send = Syscall::NodeSend {
        node: node_cap(2),
        message: crate::Message {
            caps: vec![crate::Transfer::Derive(s(101), Rights::READ.bits())],
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(0, send), WAITS);
    m.settle();
    let ut = untyped_slot(m.sys.node(2), m.t[2]);
    let map = Syscall::IoSpaceMap(crate::Map {
        space: s(IOSPACE),
        frame: s(110),
        untyped: s(ut),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(2, map);
    // Node 2 sealed, node 0 killed, then the unit's holder killed.
    m.ok(3, seal(2));
    m.run(50 * MS);
    assert_eq!(m.state(2), MemberState::Sealed);
    m.ok(3, kill(0));
    m.run(dodos_abi::FENCE_NS + MS);
    m.ok(3, kill(holder));
    while m.state(holder) != MemberState::Fenced {
        m.run(10_000);
    }
    // At the holder's fence, node 2 resumes: its member word carries the
    // resume bit.
    m.ok(3, resume(2));
    assert!(m.sys.member(2).resuming());
    let rebuilds = |m: &S| (0..5u16).any(|n| m.sys.has(n) && !m.sys.node(n).rebuilds.is_empty());
    m.run(500 * MS);
    assert_eq!(m.state(2), MemberState::Live, "the resume never ended");
    assert!(!m.sys.member(2).resuming());
    assert!(!rebuilds(&m), "the rebuild never ended");
}

// Gap 19, the combined exploration's seed 257 of pass 25: a message waits
// in node 0's ring to node 3, which is starting afresh and consumes no ring
// before it is ready; its start never ends (its starter dies). Evidence
// stuck against a starting node waits `START_NS`, as stale does: node 3 is
// not accused at `STUCK_NS`, and is accused once its start has had its
// time (D-137, D-144).
#[test]
fn a_starting_node_is_not_accused_stuck_before_start_ns() {
    let mut m = four_with_3_dead();
    // An entry left in the ring from node 0 while node 3 was dead.
    let header = dodos_abi::RingHeader {
        kind: dodos_abi::NodeMessage::Release as u8,
        length: 0,
        sender: 0,
        sender_generation: 1,
        receiver_generation: 1,
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    m.sys.inject_raw(0, 3, false, header, 1);
    // Node 1 starts node 3, its wake lost; node 2 kills node 1 at once.
    m.sys.set_window(3);
    m.ok(1, fresh(3, 1 << 3));
    m.ok(2, kill(1));
    let stuck = m.sys.canonical().stuck_ns;
    m.run(stuck + 100 * MS);
    assert_eq!(m.state(3), MemberState::Starting, "{:?}", m.sys.member(3));
    m.run(dodos_abi::START_NS + dodos_abi::SWEEP_NS);
    assert_ne!(m.state(3), MemberState::Starting);
}

// Gap 18, the other end of a hand-over without step 4: the target dies
// during the drain, the gate refuses step 4, the node that hands maps the
// unit again and stays the holder, with a new epoch (D-144).
#[test]
fn a_hand_over_refused_at_step_4_writes_held_with_a_new_epoch() {
    use super::units::{IOSPACE, attach};
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let set_broken = |m: &mut S, broken: bool| {
        for n in 0..4 {
            if m.sys.has(n) {
                let unit = m.sys.node_mut(n).table.units.entry(IOMMU_ROLE);
                unit.or_default().broken = broken;
            }
        }
    };
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    // A request node 1 has begun holds the drain.
    set_broken(&mut m, true);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(2),
    };
    assert_eq!(m.call(1, hand), WAITS);
    m.ok(3, kill(2));
    set_broken(&mut m, false);
    m.run(20 * MS);
    assert_eq!(m.woken(1), Some(&Err(Error::Busy)));
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!((word.holder(), word.epoch()), (1, epoch + 1));
    assert!(!word.failed());
}

// Gap 17, the combined exploration's seed 7 of pass 25: the node that
// hands a unit role is the holder until step 4. A node accused during
// steps 1 to 3 gets its R1 from the node that hands: the drain waits for
// R1's invalidation, step 4 comes only once it is confirmed, and `acked`
// of the accused node is not written by the node that hands before
// (Roles, D-144).
#[test]
fn the_node_that_hands_a_unit_role_runs_r1_for_a_node_accused_during_steps_1_to_3() {
    use super::units::{IOSPACE, attach, attached};
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    attached(&mut m, 2);
    let set_broken = |m: &mut S, broken: bool| {
        for n in 0..4 {
            if m.sys.has(n) {
                let unit = m.sys.node_mut(n).table.units.entry(IOMMU_ROLE);
                unit.or_default().broken = broken;
            }
        }
    };
    // A request node 1 has begun holds the drain.
    set_broken(&mut m, true);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(3),
    };
    assert_eq!(m.call(1, hand), WAITS);
    // Node 2, whose device sits behind the unit, is killed.
    let generation = m.sys.member(2).generation();
    m.ok(3, kill(2));
    let r1 = |m: &S| {
        m.sys
            .node(1)
            .reclaims
            .get(&2)
            .and_then(|r| r.r1.get(&IOMMU_ROLE).copied())
    };
    assert!(
        m.step_until(|m| matches!(r1(m), Some(Some(_)))),
        "no R1 by the node that hands"
    );
    assert!(m.sys.node(1).handing.contains_key(&IOMMU_ROLE));
    m.run(5 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).state(), dodos_abi::RoleState::Held);
    assert_eq!(
        m.sys.role(IOMMU_ROLE).holder(),
        1,
        "step 4 before R1 was confirmed"
    );
    assert_ne!(m.sys.canonical().beacons[1].acked[2], generation);
    // The unit confirms: R1 is done, then the hand-over ends.
    set_broken(&mut m, false);
    m.run(20 * MS);
    assert_eq!(r1(&m), Some(None));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 3);
}

// Gap 20, the one exception: an answer to a message the starting
// generation itself sent is written while it is starting, since it sends
// only after its reset. In the model no fresh start sends before it is live
// (its first thread runs once the node is live), so the message is the test's
// input: a `Send` written as node 3's new generation, once its first core
// has built its state and while its second core's wake is lost.
#[test]
fn an_answer_to_a_message_of_a_node_starting_afresh_is_written_before_it_is_live() {
    use crate::rings::Body;
    let mut m = S::on(&machine(Arch::X86_64, 5, &[&[0], &[1], &[2], &[3, 4]]));
    m.ok(0, kill(3));
    m.run(100 * MS);
    assert_eq!(m.state(3), MemberState::Dead);
    let old = m.sys.member(3).generation();
    m.sys.set_window(4);
    m.ok(0, fresh(3, 1 << 3 | 1 << 4));
    let built = |m: &S| m.sys.has(3) && m.sys.node(3).generation_built() == old + 1;
    assert!(m.step_until(built));
    assert_eq!(m.state(3), MemberState::Starting);
    let body = Body::Send {
        header: dodos_abi::Header::new(7, 1, 0, 0).expect("a header"),
        stamp: (0, 0),
        grants: vec![],
        words: vec![7],
    };
    let credit = Some((dodos_abi::REQUESTS_PER_PEER as u8, 1));
    let send = super::pass22::forged(&m, dodos_abi::NodeMessage::Send, 3, 0, credit, body);
    m.sys.inject(3, 0, send);
    let written = |m: &S| {
        let pair = &m.sys.canonical().rings[0][3];
        pair.kernel.produced + pair.user.produced
    };
    let before = written(&m);
    let answered = |m: &S| written(m) > before || m.state(3) != MemberState::Starting;
    assert!(m.step_until(answered));
    assert_eq!(m.state(3), MemberState::Starting, "node 3 was live first");
    assert!(written(&m) > before, "the answer was not written");
}
// CONTRACT-GAP 21, the combined exploration's seed 278: a sealing node
// hands each unit role it holds to its successor, and a sealed node holds
// none (Cores, the seal; Roles); a failed unit cannot be handed, since its
// queue never drains (step 2) and `role_hand` is `busy` on it. The seal of
// the holder of a failed unit's role never ends: the hand-over is
// abandoned at `IOMMU_WAIT_NS` and starts again, each time with a new epoch
// (D-144), and a request to the unit is answered *elsewhere* at each.
// The test holds the reading the model implements until the contract
// settles it.
#[test]
fn gap_21_the_seal_of_the_holder_of_a_failed_units_role_never_ends() {
    use super::units::{IOSPACE, attach, three};
    let mut m = three();
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    m.sys.break_unit(IOMMU_ROLE);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert!(m.sys.role(IOMMU_ROLE).failed());
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    m.ok(2, seal(1));
    m.run(dodos_abi::SEAL_NS);
    assert_eq!(m.state(1), MemberState::Sealing);
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!(word.holder(), 1);
    assert!(
        word.epoch() > epoch + 10,
        "the hand-over did not start again"
    );
}
