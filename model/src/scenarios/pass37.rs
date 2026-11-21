//! The scenarios of pass 37: decision D-165 brought into the model, and
//! the minimal traces of the two seeds the long combination run of pass 35
//! left open. Each test is named after the sentence it guards.

use super::pass22::{OBJ, forged, granted, map_got, the_grant};
use super::*;
use crate::rings::Body;
use crate::table::GrantState;
use dodos_abi::NodeMessage;

// Grants, release by the holder (D-165, gap 57): "A *pinned* entry is
// judged again whenever a `Release` or a `RevokeStep` that passes the
// receive check names it, and whenever a removal step on *X* reaches its
// anchor: the count is read in the same way, and at a count of 0 the entry
// is freed as a *revoking* one is, a `Revoke` of it still in flight stops
// counting as in flight, its credit is freed and the thread that waits for
// its answer is woken; at a count above 0 it stays *pinned*." "A late
// answer to the `Revoke` names a credit serial that is over and changes
// nothing."

/// Node 0 revokes the grant node 1 maps (thread 0 waits for the answer);
/// node 1's faulty kernel writes a `Release` before it reads the `Revoke`,
/// its leaf still counted, and node 0 pins the entry. Node 1 has not read
/// the `Revoke` yet. Returns the grant and a second thread of node 0.
fn pinned_with_its_revoke_in_flight() -> (S, u16, ObjId) {
    let mut m = granted(true);
    m.ok(0, super::place_first(0, 0));
    let second = m.thread_on(0, 300, 0);
    assert_eq!(m.call(1, map_got(1 << 30)), DONE);
    let (grant, serial) = the_grant(&m);
    assert_eq!(m.call(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    let release = forged(
        &m,
        NodeMessage::Release,
        1,
        0,
        None,
        Body::Release { grant, serial },
    );
    m.sys.inject(1, 0, release);
    assert!(m.step_until(|m| state(m, grant) == Some(GrantState::Pinned)));
    assert!(revoke_in_flight(&m).is_some(), "the Revoke unanswered");
    (m, grant, second)
}

fn state(m: &S, grant: u16) -> Option<GrantState> {
    m.sys.canonical().grants[0].get(&grant).map(|e| e.state)
}

/// The credit (index, serial) of node 0's `Revoke` to node 1 still in use.
fn revoke_in_flight(m: &S) -> Option<(u8, u32)> {
    let credits = &m.sys.node(0).peers[1].credits;
    credits.iter().enumerate().find_map(|(i, c)| {
        c.as_ref()
            .filter(|c| c.kind == NodeMessage::Revoke)
            .map(|c| (i as u8, c.serial))
    })
}

/// The count of node 1's use entry of the grant.
fn count(m: &S) -> u64 {
    m.sys.canonical().uses[1].values().map(|u| u.leaves).sum()
}

/// Node 1's faulty kernel has its gate clear its leaf under the grant
/// (its remote capability is invalid once the grant is revoking, so no call
/// does it), keeping its root and its use entry: the count, a gate word,
/// falls to 0 without a `Release`.
fn unmapped(m: &mut S) {
    let node = m.sys.node_mut(1);
    let mapped = node.caps.iter().find(|(_, c)| {
        node.remotes.contains_key(&c.object) && matches!(c.record, crate::Record::Mapping(_))
    });
    let cap = *mapped.expect("node 1's leaf").0;
    let unmapped = node.begin_unmap(cap, None);
    assert_eq!(unmapped, Ok(crate::memory::Unmapped::Done));
    let table = node.table.clone();
    m.sys.edit_table(|t| *t = table);
    assert_eq!(count(m), 0);
    assert_eq!(m.sys.canonical().uses[1].len(), 1, "the use entry kept");
}

/// The late answer of the `Revoke`: node 1 reads it, removes its root and
/// answers removed; node 0's receive check refuses the answer, whose credit
/// is over, and nothing else changes.
fn late_answer_changes_nothing(m: &mut S, grant: u16) {
    let refused = m.sys.node(0).refused.0;
    let grants = m.sys.canonical().grants[0].clone();
    m.run(30 * MS);
    assert!(m.sys.canonical().uses[1].is_empty(), "the Revoke ran");
    assert_eq!(m.sys.node(0).refused.0, refused + 1, "its answer refused");
    assert_eq!(m.sys.canonical().grants[0], grants);
    assert_eq!(state(m, grant), None);
}

#[test]
fn a_revoke_step_not_removed_frees_a_pinned_grant_at_a_count_of_0() {
    let (mut m, grant, _) = pinned_with_its_revoke_in_flight();
    unmapped(&mut m);
    // Node 1's faulty kernel answers the `Revoke` before it reads it, with
    // its credit, reporting its root not removed: the answer passes the
    // receive check and names the pinned entry.
    let credit = revoke_in_flight(&m).expect("in flight");
    let step = forged(
        &m,
        NodeMessage::RevokeStep,
        1,
        0,
        Some(credit),
        Body::RevokeStep { removed: false },
    );
    m.sys.inject(1, 0, step);
    assert!(m.step_until(|m| state(m, grant).is_none()));
    assert_eq!(revoke_in_flight(&m), None);
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
    late_answer_changes_nothing(&mut m, grant);
}

#[test]
fn a_revoke_step_not_removed_leaves_a_pinned_grant_pinned_at_a_count_above_0() {
    let (mut m, grant, _) = pinned_with_its_revoke_in_flight();
    let credit = revoke_in_flight(&m).expect("in flight");
    let step = forged(
        &m,
        NodeMessage::RevokeStep,
        1,
        0,
        Some(credit),
        Body::RevokeStep { removed: false },
    );
    m.sys.inject(1, 0, step);
    assert!(m.step_until(|m| revoke_in_flight(m).is_none()));
    assert_eq!(state(&m, grant), Some(GrantState::Pinned));
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
}

#[test]
fn a_removal_step_that_reaches_a_pinned_anchor_frees_it_and_its_revokes_credit_at_a_count_of_0() {
    let (mut m, grant, second) = pinned_with_its_revoke_in_flight();
    unmapped(&mut m);
    // A second thread of node 0 removes the frame's children: its removal
    // reaches the pinned anchor while the first thread waits.
    assert_eq!(m.call_by(0, second, Syscall::CapRevoke(s(OBJ))), DONE);
    assert_eq!(state(&m, grant), None);
    assert_eq!(revoke_in_flight(&m), None);
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
    assert_eq!(m.sys.check(), Ok(()));
    late_answer_changes_nothing(&mut m, grant);
}

#[test]
fn a_removal_step_that_reaches_a_pinned_anchor_at_a_count_above_0_leaves_it_pinned() {
    let (mut m, grant, second) = pinned_with_its_revoke_in_flight();
    assert_eq!(
        m.call_by(0, second, Syscall::CapRevoke(s(OBJ))),
        Err(Error::Again)
    );
    assert_eq!(state(&m, grant), Some(GrantState::Pinned));
    assert!(revoke_in_flight(&m).is_some());
    assert_eq!(m.sys.check(), Ok(()));
}

// D-165: "Nothing else reads a pinned entry, so one whose holder sends
// nothing more stays until an ancestor's removal or the end of the holder's
// generation." Found by the seed 2 of `verify` once the table cycle's
// slots moved: faulty node 2 lowered the count of a grant it had pinned,
// with no `Revoke` in flight, and the checker's R3 reported the pinned
// entry of a running grantee. Only a holder that wrote an undue `Release`
// lowers that count (a correct one's count under a pinned entry is a
// failed unit's), so R3 passes over it for such a grantee and still
// reports it for a correct one
// (`checker::a_pinned_grant_of_a_grantee_that_runs_is_reported`). The next
// removal step that reaches the anchor frees it.
#[test]
fn a_pinned_grant_whose_faulty_holder_lowers_its_count_and_sends_nothing_stays_until_a_removal_reaches_it()
 {
    let (mut m, grant, second) = pinned_with_its_revoke_in_flight();
    let credit = revoke_in_flight(&m).expect("in flight");
    let step = forged(
        &m,
        NodeMessage::RevokeStep,
        1,
        0,
        Some(credit),
        Body::RevokeStep { removed: false },
    );
    m.sys.inject(1, 0, step);
    assert!(m.step_until(|m| revoke_in_flight(m).is_none()));
    m.sys.undue_release(1);
    unmapped(&mut m);
    assert_eq!(state(&m, grant), Some(GrantState::Pinned));
    assert_eq!(m.sys.check(), Ok(()));
    m.run(30 * MS);
    assert_eq!(
        state(&m, grant),
        Some(GrantState::Pinned),
        "nothing reads it"
    );
    assert_eq!(m.call_by(0, second, Syscall::CapRevoke(s(OBJ))), DONE);
    assert_eq!(state(&m, grant), None);
}

// `node_send`, Checks (D-165, gap 58): "Then the state, in order: `gone`
// if the node is not live ... it is the first check of the state, and no
// other state is read for a node that is not live; `busy` if a frame
// listed lies over memory the caller's node does not hold ...; `busy` if
// `SENDS_PER_PEER` sends ... are in flight to it, ..., or if the message
// does not fit in the user ring, or if the caller's grant table has fewer
// free entries than the frames listed." Each case below holds two faults:
// node 1 not live and one of the later `busy`s; `gone` wins. (`gone`
// against the gate's `busy` over a frame is
// `pass36::node_send_judges_gone_before_the_busy_of_a_frame_its_gate_refuses`;
// the gate's `busy` against the other `busy`s is the same error with
// nothing written, so no call tells their order apart.)

const SENT: u32 = 211;

/// Node 0 of two, a frame of its own in `SENT`, and its `node_send` to
/// node 1 listing it (or nothing).
fn sender(frames: bool) -> (N, Syscall) {
    let mut k = N::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let frame = k.retype(ObjectKind::Frame, 1, SENT);
    k.ok(frame);
    let caps = match frames {
        true => vec![crate::Transfer::Derive(s(SENT), Rights::READ.bits())],
        false => Vec::new(),
    };
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message {
            caps,
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    (k, send)
}

/// The call is `busy` while node 1 is live and `gone` once it is not.
fn gone_before_busy(mut k: N, send: Syscall) {
    assert_eq!(k.m.clone().apply(k.t, send.clone()), Err(Error::Busy));
    let member = k.m.table.member(1);
    k.m.table.members[1] = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::Stale,
        dodos_abi::DRIVER_NONE,
        member.generation(),
    );
    assert_eq!(k.m.apply(k.t, send), Err(Error::Gone));
}

#[test]
fn node_send_judges_gone_before_the_busy_of_its_sends_in_flight() {
    let (mut k, send) = sender(false);
    let credits = &mut k.m.peers[1].credits;
    for credit in &mut credits[dodos_abi::REQUESTS_PER_PEER..] {
        *credit = Some(crate::rings::Credit {
            kind: NodeMessage::Send,
            serial: 0,
            completes: crate::rings::Completes::Send {
                grants: Vec::new(),
                thread: None,
                manage: true,
            },
        });
    }
    gone_before_busy(k, send);
}

#[test]
fn node_send_judges_gone_before_the_busy_of_a_full_user_ring() {
    let (mut k, send) = sender(false);
    let rings = std::sync::Arc::make_mut(&mut k.m.table.rings);
    rings[0][1].user.produced += dodos_abi::RING_USER_ENTRIES;
    gone_before_busy(k, send);
}

#[test]
fn node_send_judges_gone_before_the_busy_of_a_full_grant_table() {
    let (mut k, send) = sender(true);
    let frame = k.m.object_at(k.t, s(SENT)).expect("the frame");
    let entry = crate::table::GrantEntry {
        anchor: crate::table::Anchor {
            frame: k.m.frames[&frame],
            rights: Rights::READ,
            depth: 2,
        },
        grantee: 1,
        generation: k.m.table.member(1).generation(),
        serial: 0,
        state: GrantState::Live,
        use_index: None,
        answered: false,
        doorbell: None,
    };
    k.m.table.grants[0] = (0..dodos_abi::GRANTS as u16).map(|g| (g, entry)).collect();
    gone_before_busy(k, send);
}
