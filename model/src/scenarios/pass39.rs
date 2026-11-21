//! Pass 39: decision D-167 (a sealing node leaves request-opening messages and
//! what follows them in that ring for its resume; a stale `ReportAsk` is
//! answered with an empty `Report` (*end*) so its credit is freed), and the
//! directed tests for the findings of the review of passes 37 and 38.

use super::pass17::{kill, resume, seal};
use super::pass22::take;
use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attached, role_slot};
use super::*;
use crate::rings::Body;
use crate::system::Event;
use crate::{Map, Message, Range, Transfer};
use dodos_abi::{
    CORE_ANSWER_NS, IOMMU_WAIT_NS, NodeMessage, REPORT_END, RoleState, START_SLOT_ASPACE,
    START_SLOT_MAILBOX,
};

const REPLY: u32 = 100;
const BLOCK: u32 = 160;
const FRAME: u32 = 161;
const GOT: u32 = 170;
const OTHER_THREAD: u32 = 180;

fn recv(first: u32) -> Syscall {
    Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(REPLY),
        range: Range {
            first: s(first),
            count: 4,
        },
        wait: 0,
    }
}

fn send_frame(to: u16, slot: u32) -> Syscall {
    let rights = Rights::READ.union(Rights::WRITE).union(Rights::DERIVE);
    Syscall::NodeSend {
        node: node_cap(to),
        message: Message {
            label: 7,
            flags: 0,
            words: vec![7],
            caps: vec![Transfer::Derive(s(slot), rights.bits())],
            marks: 0,
        },
        doorbell: SlotAddr::NONE,
    }
}

fn grant_frame(m: &mut S, from: u16, to: u16) {
    let shift = m.sys.canonical().geometry.chunk_shift();
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let chunk = m
        .sys
        .canonical()
        .owners
        .iter()
        .position(|&w| w == free)
        .expect("a free chunk") as u64;
    m.ok(from, take(0, chunk << shift, u64::from(shift), BLOCK));
    m.ok(from, retype_from(BLOCK, ObjectKind::Frame, 1, FRAME));
    for node in [from, to] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    assert_eq!(m.call(to, recv(GOT)), WAITS);
    assert_eq!(m.call(from, send_frame(to, FRAME)), WAITS);
    m.settle();
    assert!(!m.sys.canonical().grants[from as usize].is_empty());
}

fn unread_kernel(m: &S, from: u16, to: u16) -> bool {
    let t = m.sys.canonical();
    t.rings[from as usize][to as usize].kernel.produced
        != t.beacons[to as usize].consumed[from as usize].0
}

fn io_map_on(m: &S, node: u16, frame: u32) -> Syscall {
    Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(frame),
        untyped: s(super::table_slot(m.sys.node(node), m.t[node as usize])),
        address: 0x1000_0000 + u64::from(frame) * 0x10_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    })
}

/// Completes `cap_delete(FRAME)` on `node`, stepping the machine between
/// retries until the grant entry is gone.
fn finish_revoke(m: &mut S, node: u16) {
    for _ in 0..40 {
        if m.sys.canonical().grants[node as usize].is_empty() {
            return;
        }
        let thread = m.t[node as usize];
        if m.sys.node(node).runnable().contains(&thread) {
            let outcome = m.call(node, Syscall::CapDelete(s(FRAME)));
            if outcome == DONE {
                return;
            }
        }
        m.run(20 * MS);
    }
    assert!(
        m.sys.canonical().grants[node as usize].is_empty(),
        "grant was not freed"
    );
}

// D-167 (1), core request case: a `Revoke` of a frame mapped in an `aspace`
// loaded on another core reaches a sealing node of several cores while its
// cores park. The ring service stops at the `Revoke`, the node seals with it
// unread and no request open, and after its resume it completes the `Revoke`
// and stays alive.
#[test]
fn a_revoke_of_a_mapped_frame_waits_during_seal_parking_core_request() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    // Second thread on node 0 so it can call `resume` while node 0's first
    // thread waits on `CapDelete`'s `Revoke`.
    m.ok(0, place_first(0, 0));
    let resumer = m.thread_on(0, OTHER_THREAD, 0);
    // Node 1 places a second thread on its second core (position 1, core 2)
    // in its start-up aspace so core 1 loads that aspace.
    let other = m.thread_on(1, OTHER_THREAD, 2);
    m.settle();
    assert_eq!(m.sys.node(1).cores[1].thread, Some(other));

    // Node 0 grants a frame to node 1, which maps it in the shared aspace.
    grant_frame(&mut m, 0, 1);
    m.ok(
        1,
        Syscall::ASpaceMap(Map {
            space: s(START_SLOT_ASPACE),
            frame: s(GOT),
            untyped: SlotAddr::NONE,
            address: 1 << 30,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );

    // Node 0 seals node 1; stop once core 2 is parking (and confirmed).
    m.ok(0, seal(1));
    let parking = |m: &S| {
        m.state(1) == MemberState::Sealing
            && m.sys.canonical().beacons[1].sealed_at.is_some()
            && m.sys.node(1).park_confirmed(2)
    };
    assert!(m.step_until(parking));

    // Node 0 starts revoking the mapped frame: `Revoke` enters ring 0 → 1.
    assert_eq!(m.call(0, Syscall::CapDelete(s(FRAME))), WAITS);
    assert!(unread_kernel(&m, 0, 1));

    // Node 1 writes sealed with the `Revoke` still unread and no request open.
    assert!(m.step_until(|m| m.state(1) == MemberState::Sealed));
    assert!(
        unread_kernel(&m, 0, 1),
        "the Revoke must wait in the ring until resume"
    );
    assert!(
        m.sys.node(1).cores.iter().all(|c| c.request.is_none()),
        "no core request may be open when sealed"
    );
    assert_eq!(m.sys.check(), Ok(()));

    // Wait past `CORE_ANSWER_NS` before resuming: an open request would accuse
    // the node at resume.
    m.run(CORE_ANSWER_NS + 20 * MS);
    assert_eq!(m.sys.apply(0, resumer, resume(1)), DONE);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(m.step_until(|m| m.state(1) == MemberState::Live));
    finish_revoke(&mut m, 0);
    assert!(!unread_kernel(&m, 0, 1));
    m.run(2 * CORE_ANSWER_NS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.check(), Ok(()));
}

// D-167 (1), unit request case: a `Revoke` of a frame mapped in an `iospace`
// serving a device reaches a sealing node of several cores while its cores
// park. The ring service stops at the `Revoke`, the node seals with it unread
// and no unit request open, and after its resume it completes the `Revoke`
// and stays alive.
#[test]
fn a_revoke_of_a_mapped_frame_waits_during_seal_parking_unit_request() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2, 3]]));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    m.ok(0, place_first(0, 0));
    let resumer = m.thread_on(0, OTHER_THREAD, 0);
    m.settle();
    attached(&mut m, 2);
    grant_frame(&mut m, 0, 2);
    m.ok(2, io_map_on(&m, 2, GOT));

    m.ok(0, seal(2));
    let parking = |m: &S| {
        m.state(2) == MemberState::Sealing
            && m.sys.canonical().beacons[2].sealed_at.is_some()
            && m.sys.node(2).park_confirmed(3)
    };
    assert!(m.step_until(parking));

    assert_eq!(m.call(0, Syscall::CapDelete(s(FRAME))), WAITS);
    assert!(unread_kernel(&m, 0, 2));

    assert!(m.step_until(|m| m.state(2) == MemberState::Sealed));
    assert!(
        unread_kernel(&m, 0, 2),
        "the Revoke must wait in the ring until resume"
    );
    assert!(
        m.sys.node(2).unit_requests.iter().flatten().all(|e| {
            !matches!(
                e.stage,
                crate::units::Stage::Sent { .. } | crate::units::Stage::Local { .. }
            )
        }),
        "no unit request may be open when sealed"
    );
    assert_eq!(m.sys.check(), Ok(()));

    m.run(IOMMU_WAIT_NS + 20 * MS);
    assert_eq!(m.sys.apply(0, resumer, resume(2)), DONE);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(m.step_until(|m| m.state(2) == MemberState::Live));
    finish_revoke(&mut m, 0);
    assert!(!unread_kernel(&m, 0, 2));
    m.run(2 * IOMMU_WAIT_NS);
    assert_eq!(m.state(2), MemberState::Live);
    assert_eq!(m.sys.check(), Ok(()));
}

// D-167 (2): a `ReportAsk` waits behind a `Revoke` of a mapped frame on a
// sealing node whose cores park. The sealing node writes sealed leaving both
// unread; the rebuilding holder sees the member word read sealed, stops
// waiting for its `Report`, and finishes the rebuild keeping the sealed node's
// entries; the role's holder then changes before the resume. At the resume the
// stale `ReportAsk` is answered with an empty `Report` (*end*), and the
// asker's credit is freed.
#[test]
fn a_report_ask_behind_a_revoke_on_a_sealing_node_is_answered_empty_at_resume() {
    let mut m = S::on(&machine(
        Arch::X86_64,
        6,
        &[&[0], &[1], &[2], &[3], &[4, 5]],
    ));
    let old_holder = m.sys.role(IOMMU_ROLE).holder();
    assert_eq!(old_holder, 1);
    let driver = 0;
    let asker = 2;
    let next_holder = 3;
    let sealing = 4;

    // Node 4 attaches a device on IOMMU_ROLE and maps a frame granted by node 2
    // into that iospace.
    attached(&mut m, sealing);
    grant_frame(&mut m, asker, sealing);
    m.ok(sealing, io_map_on(&m, sealing, GOT));

    // Create a second thread on node 2 so it can hand IOMMU_ROLE while node 2's
    // first thread waits on `CapDelete`'s `Revoke` to sealed node 4.
    m.ok(asker, place_first(asker, 2));
    let hander = m.thread_on(asker, OTHER_THREAD, 2);
    m.settle();

    // Hand IOAPIC_ROLE away from node 1 if it holds it, so node 1 holds only
    // IOMMU_ROLE when killed and IOMMU_ROLE never contends on the
    // one-ReportAsk-per-peer limit.
    if m.sys.role(super::units::IOAPIC_ROLE).holder() == old_holder {
        let hand_ioapic = Syscall::RoleHand {
            role: role_slot(&m, old_holder, super::units::IOAPIC_ROLE),
            node: node_cap(next_holder),
        };
        assert_eq!(m.call(old_holder, hand_ioapic), WAITS);
        assert!(m.step_until(|m| {
            let w = m.sys.role(super::units::IOAPIC_ROLE);
            w.state() == RoleState::Held && w.holder() == next_holder
        }));
    }

    // Kill node 1 and wait until its fence is confirmed. Node 2 (node 1's
    // successor) will take over IOMMU_ROLE at `confirmed + SETTLE_NS`.
    m.supervisor(driver, m.t[driver as usize]);
    m.ok(driver, kill(old_holder));
    assert!(m.step_until(|m| m.sys.canonical().fences[old_holder as usize].confirmed.is_some()));

    // Seal node 4 and stop once it enters `SealStep::Parking` (core 5 is
    // parking, not yet confirmed).
    m.ok(driver, seal(sealing));
    assert!(m.step_until(|m| {
        m.state(sealing) == MemberState::Sealing
            && m.sys.canonical().beacons[sealing as usize].sealed_at.is_some()
    }));
    assert!(!m.sys.node(sealing).park_confirmed(5));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), old_holder);

    // Node 2 starts revoking its granted frame: `Revoke` is now at the head of
    // ring 2 → 4 while node 4 is in `SealStep::Parking`.
    assert_eq!(m.call(asker, Syscall::CapDelete(s(FRAME))), WAITS);
    assert!(unread_kernel(&m, asker, sealing));

    // Step all events (including node 4's timers) EXCEPT core 5's `CpuOff`
    // park confirmation, so node 4 remains in `SealStep::Parking` while node 2
    // takes over IOMMU_ROLE and sends `ReportAsk` behind the `Revoke`.
    let has_ask_credit = |m: &S| {
        m.sys.node(asker).peers[sealing as usize]
            .credits
            .iter()
            .flatten()
            .any(|c| c.kind == NodeMessage::ReportAsk)
    };
    let ready = |m: &S| {
        m.sys.role(IOMMU_ROLE).holder() == asker
            && m.sys
                .node(asker)
                .rebuilds
                .get(&IOMMU_ROLE)
                .is_some_and(|r| r.node == sealing && r.asked.is_some())
    };
    let mut rounds = 0;
    while !ready(&m) {
        let is_core5_park = |m: &S, e: Event| {
            matches!(
                e,
                Event::Action(i)
                    if matches!(
                        m.sys.actions().get(i),
                        Some((_, crate::HwAction::Park { core: 5 }))
                    )
            )
        };
        let mut fired_any = 0;
        while !ready(&m) && fired_any < 64 {
            let Some(ev) = m
                .sys
                .events()
                .into_iter()
                .find(|&e| !is_core5_park(&m, e))
            else {
                break;
            };
            m.sys.fire(ev);
            fired_any += 1;
        }
        assert_eq!(m.sys.check(), Ok(()));
        if !ready(&m) {
            let cap = m.sys.now() + 100_000;
            let step_to = (0..5u16)
                .filter(|&z| m.sys.has(z))
                .flat_map(|z| {
                    let n = m.sys.node(z);
                    (0..n.core_count()).filter_map(move |p| n.timer_of(p))
                })
                .filter(|&t| t > m.sys.now())
                .fold(cap, u64::min);
            m.sys.move_clock(step_to);
        }
        rounds += 1;
        assert!(rounds < 500, "rebuild did not send ReportAsk to node 4");
    }
    assert_eq!(m.state(sealing), MemberState::Sealing);
    assert!(has_ask_credit(&m));

    // Node 4 seals leaving both `Revoke` and `ReportAsk` unread in ring 2 → 4.
    assert!(m.step_until(|m| m.state(sealing) == MemberState::Sealed));
    assert!(unread_kernel(&m, asker, sealing));

    // Node 2 sees node 4 reads sealed, stops waiting for its `Report`, and
    // finishes the rebuild keeping node 4's attached device entry.
    assert!(m.step_until(|m| m.sys.node(asker).rebuilds.is_empty()));
    assert!(
        m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16)),
        "rebuild must keep the sealed node's device entry"
    );
    assert!(
        has_ask_credit(&m),
        "the ReportAsk credit stays in flight until resume"
    );

    // Change the role's holder from node 2 to node 3 before node 4 resumes.
    let hand = Syscall::RoleHand {
        role: role_slot(&m, asker, IOMMU_ROLE),
        node: node_cap(next_holder),
    };
    assert_eq!(m.sys.apply(asker, hander, hand), WAITS);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(m.step_until(|m| {
        let w = m.sys.role(IOMMU_ROLE);
        w.state() == RoleState::Held && w.holder() == next_holder
    }));
    assert!(has_ask_credit(&m));

    // Resume node 4. At resume it consumes the `Revoke` and the stale
    // `ReportAsk`, answering the stale `ReportAsk` with an empty `Report`
    // (*end*) so node 2's credit is freed.
    let refused_before = m.sys.node(sealing).refused.0;
    m.ok(driver, resume(sealing));
    let mut saw_empty_report = false;
    let mut steps = 0;
    while has_ask_credit(&m) || m.state(sealing) != MemberState::Live {
        let t = m.sys.canonical();
        let ring = &t.rings[sealing as usize][asker as usize].kernel;
        let consumed = t.beacons[asker as usize].consumed[sealing as usize].0;
        let mut at = consumed;
        while at < ring.produced {
            if let Some(crate::table::RingEntry::Message(msg)) = ring.at(at)
                && let Body::Report { next, items } = &msg.body
            {
                assert_eq!(*next, REPORT_END, "stale ReportAsk must be answered *end*");
                assert!(items.is_empty(), "stale ReportAsk must be answered empty");
                saw_empty_report = true;
            }
            at += 1;
        }
        let events = m.sys.events();
        if let Some(&event) = events.first() {
            m.sys.fire(event);
            assert_eq!(m.sys.check(), Ok(()));
        } else {
            m.sys.advance(m.sys.now() + 100_000);
            assert_eq!(m.sys.check(), Ok(()));
        }
        steps += 1;
        assert!(steps < 500, "resume did not answer ReportAsk and free credit");
    }
    assert!(
        saw_empty_report,
        "expected an empty Report (*end*) on the ring"
    );
    assert!(!has_ask_credit(&m), "asker's ReportAsk credit must be free");
    assert_eq!(
        m.sys.node(sealing).refused.0,
        refused_before,
        "stale ReportAsk must be answered empty, not refused"
    );
    finish_revoke(&mut m, asker);
    assert_eq!(m.state(sealing), MemberState::Live);
    assert_eq!(m.sys.check(), Ok(()));
}
