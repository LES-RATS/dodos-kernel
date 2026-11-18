//! Pass 27, D-145: gap 21 of pass 26 decided. A seal hands a failed unit's
//! role without step 2 (no drain, no wait), answering *failed* at step 1;
//! the handing and held words keep bit 63, and the target maps the unit,
//! runs R1 and answers *failed*. A unit that fails during a seal's drain
//! ends that hand-over (D-144) and is handed at the seal's next step. A
//! take-over of a failed unit's role is not a rebuild.

use super::pass17::{kill, seal};
use super::units::{IOMMU_ROLE, IOSPACE, attach};
use super::*;
use dodos_abi::RoleState;

/// Four nodes; node 1 holds the IOMMU role; node 0 has an iospace.
fn four() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    m
}

/// The unit stops confirming, and node 0's attach makes it fail.
fn failed(m: &mut S) {
    m.sys.break_unit(IOMMU_ROLE);
    let call = attach(m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert!(m.sys.role(IOMMU_ROLE).failed());
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
}

/// No node rebuilds the IOMMU role.
fn no_rebuild(m: &S) -> bool {
    (0..4u16).all(|n| !m.sys.has(n) || m.sys.node(n).rebuilds.is_empty())
}

// Gap 21, the combined exploration's seed 278 of pass 26: the seal of the
// holder of a failed unit's role never ended, its hand-over abandoned and
// begun again every `IOMMU_WAIT_NS`, the epoch climbing. The seal now hands
// it with no drain: the seal ends, the epoch stops at e + 2, and a call on
// the unit is `busy` (Roles, D-145).
#[test]
fn a_seal_hands_a_failed_units_role_without_the_drain() {
    let mut m = four();
    failed(&mut m);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    m.ok(2, seal(1));
    m.run(50 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!(word.state(), RoleState::Held);
    assert!(word.failed() && word.holder() != 1);
    assert_eq!(word.epoch(), epoch + 2);
    m.run(10 * dodos_abi::IOMMU_WAIT_NS);
    assert_eq!(
        m.sys.role(IOMMU_ROLE).epoch(),
        epoch + 2,
        "the epoch climbs"
    );
    // A call on the failed unit is `busy` (IOMMU).
    let call = attach(&m, 0, IOSPACE, 0x12);
    assert_eq!(m.call(0, call), Err(Error::Busy));
}

// A unit that fails during a seal's own drain: the hand-over ends by D-144,
// held (X, e + 1) with bit 63, and the seal's next step hands the role
// without step 2: handing (e + 2), then held by the target (e + 3), both
// with bit 63 (Roles, D-145).
#[test]
fn a_unit_that_fails_during_a_seals_drain_is_handed_at_the_seals_next_step() {
    let mut m = four();
    m.sys.break_unit(IOMMU_ROLE);
    // A request node 1 has begun holds the drain.
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    assert!(!m.sys.role(IOMMU_ROLE).failed());
    m.ok(2, seal(1));
    // The abandoned hand-over and the next one fall in one step of the
    // seal: the epoch tells them (e + 1, then e + 2 and e + 3).
    m.run(50 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let word = m.sys.role(IOMMU_ROLE);
    assert!(word.failed() && word.state() == RoleState::Held && word.holder() != 1);
    assert_eq!(word.epoch(), epoch + 3);
}

/// Events until `until` holds, `no_rebuild` checked after each.
fn watch(m: &mut S, until: impl Fn(&S) -> bool) {
    for _ in 0..200_000 {
        assert!(no_rebuild(m), "a failed unit is rebuilt");
        if until(m) {
            return;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.advance(m.sys.now() + 10_000),
        }
        assert_eq!(m.sys.check(), Ok(()));
    }
    panic!("never");
}

// A take-over of a failed unit's role is not a rebuild: the successor of a
// dead holder writes held (S, e + 1) with bit 63, runs R1 and acknowledges
// the old holder's reclaim at once (it had no device behind the unit),
// with no drain, no reports and no closing invalidation (Roles, D-145).
#[test]
fn a_take_over_of_a_failed_units_role_is_not_a_rebuild() {
    let mut m = four();
    failed(&mut m);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let generation = m.sys.member(1).generation();
    let submitted = |m: &S| m.sys.canonical().units[&IOMMU_ROLE].next;
    let before = submitted(&m);
    m.ok(2, kill(1));
    watch(&mut m, |m| m.sys.role(IOMMU_ROLE).holder() != 1);
    // One submission, R1's for the old holder: no closing invalidation of
    // a rebuild (a rebuild with every node answering at once ends within
    // the take-over's event, so its mark is that invalidation).
    assert_eq!(submitted(&m), before + 1);
    let word = m.sys.role(IOMMU_ROLE);
    let successor = word.holder();
    assert!(word.failed() && word.state() == RoleState::Held);
    assert_eq!(word.epoch(), epoch + 1);
    watch(&mut m, |m| m.state(1) == MemberState::Dead);
    assert_eq!(
        m.sys.canonical().beacons[successor as usize].acked[1],
        generation
    );
}

// The target of a seal's hand-over of a failed unit dies after step 4: the
// take-over of a handing word with bit 63, judged on the target, is the
// take-over of a failed unit, with no rebuild (Roles, D-145).
#[test]
fn a_take_over_of_a_handing_word_with_bit_63_is_not_a_rebuild() {
    let mut m = four();
    failed(&mut m);
    m.ok(3, seal(1));
    let handing = |m: &S| m.sys.role(IOMMU_ROLE).state() == RoleState::Handing;
    watch(&mut m, handing);
    let word = m.sys.role(IOMMU_ROLE);
    assert!(word.failed());
    let target = word.target();
    let killer = (0..4u16)
        .find(|&n| n != 1 && n != target && n != 0)
        .expect("a killer");
    let submitted = |m: &S| m.sys.canonical().units[&IOMMU_ROLE].next;
    let before = submitted(&m);
    m.ok(killer, kill(target));
    watch(&mut m, |m| {
        let word = m.sys.role(IOMMU_ROLE);
        word.state() == RoleState::Held && word.holder() != 1 && word.holder() != target
    });
    assert!(m.sys.role(IOMMU_ROLE).failed());
    // R1's for the dead target only: no closing invalidation of a rebuild.
    assert_eq!(submitted(&m), before + 1);
    m.run(50 * MS);
    assert!(no_rebuild(&m));
    assert_eq!(m.state(1), MemberState::Sealed);
}

// At step 1 the holder of a failed unit answers *failed* to a request, not
// *elsewhere*: the answer is final (Roles, D-145).
#[test]
fn a_request_to_a_failed_unit_during_its_hand_over_is_answered_failed() {
    use crate::rings::{Body, UnitOp};
    use dodos_abi::UnitResult;
    let mut m = four();
    failed(&mut m);
    m.sys.node_mut(1).handing.insert(
        IOMMU_ROLE,
        crate::roles::Handover {
            target: 2,
            started: 0,
            caller: None,
            failed: true,
        },
    );
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let before = m.sys.canonical().rings[1][0].kernel.produced;
    let op = UnitOp::Invalidate { domain: Some(1) };
    m.sys
        .node_mut(1)
        .unit_requested(0, Some((0, 1)), (IOMMU_ROLE as u16, epoch, op));
    let ring = &m.sys.node(1).table.rings[1][0].kernel;
    assert!(ring.produced > before);
    let answer = ring.at(ring.produced - 1);
    assert!(
        matches!(
            answer,
            Some(crate::table::RingEntry::Message(crate::rings::Message {
                body: Body::UnitAnswer {
                    result: UnitResult::Failed
                },
                ..
            }))
        ),
        "{answer:?}"
    );
}
