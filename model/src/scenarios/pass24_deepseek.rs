//! DeepSeek's verification of pass 24 (D-142, `reviews/2026-11-18-pass24-verify-deepseek.md`
//! in the documentation repository): the tests it wrote against the claims
//! of the pass, each named after the behaviour it holds, and the two rules
//! it found held by no test (the first map before the answer, C4; a block
//! under a live grant, C11). States are reached by user calls and by the
//! unit's failure (`System::break_unit`).

use super::pass22::take;
use super::units::{IOMMU_ROLE, IOSPACE, attached};
use super::*;
use crate::table::GrantState;
use crate::{Message, Range, Record, Transfer};
use dodos_abi::START_SLOT_MAILBOX;

const REPLY: u32 = 100;
const OBJ: u32 = 101;
const OTHER: u32 = 102;
const GOT: u32 = 110;
const BLOCK: u32 = 160;
/// The granting node, the unit's holder, and the holder of the frame.
const X: u16 = 0;
const THIRD: u16 = 1;
const Y: u16 = 2;

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

fn send(slot: u32) -> Syscall {
    let rights = Rights::READ.union(Rights::WRITE).union(Rights::DERIVE);
    Syscall::NodeSend {
        node: node_cap(Y),
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

fn io_map(m: &S, frame: u32) -> Syscall {
    Syscall::IoSpaceMap(crate::Map {
        space: s(IOSPACE),
        frame: s(frame),
        untyped: s(untyped_slot(m.sys.node(Y), m.t[Y as usize])),
        address: 0x1000_0000 + u64::from(frame) * 0x10_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    })
}

struct Granted {
    m: S,
    chunk: u64,
    frame: ObjId,
    grant: u16,
    use_index: u16,
}

/// Node `X` grants node `Y` a frame of a chunk it took; `Y`'s iospace has a
/// device of `Y` behind the unit of `THIRD`.
fn granted() -> Granted {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.caching_mode = false;
    let mut m = S::on(&machine);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), THIRD);
    attached(&mut m, Y);
    let shift = m.sys.canonical().geometry.chunk_shift();
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let chunk = m
        .sys
        .canonical()
        .owners
        .iter()
        .position(|&w| w == free)
        .expect("a free chunk") as u64;
    m.ok(X, take(0, chunk << shift, u64::from(shift), BLOCK));
    m.ok(X, retype_from(BLOCK, ObjectKind::Frame, 1, OBJ));
    let frame = m.sys.node(X).object_at(m.t[0], s(OBJ)).expect("the frame");
    for node in [X, Y] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    assert_eq!(m.call(Y, recv(GOT)), WAITS);
    assert_eq!(m.call(X, send(OBJ)), WAITS);
    m.settle();
    let (&grant, entry) = m.sys.canonical().grants[X as usize]
        .iter()
        .next()
        .expect("the grant");
    let use_index = entry.use_index.expect("answered");
    Granted {
        m,
        chunk,
        frame,
        grant,
        use_index,
    }
}

#[track_caller]
fn until_done(m: &mut S, node: u16, call: Syscall) {
    for _ in 0..20 {
        match m.call(node, call.clone()) {
            DONE => return,
            WAITS => m.run(30 * MS),
            _ => m.run(10 * MS),
        }
    }
    panic!("{call:?} never done");
}

fn chunk_shift(m: &S) -> u64 {
    u64::from(m.sys.canonical().geometry.chunk_shift())
}

/// The chunk `X` granted is still taken by `X`'s generation, and no live
/// node may take it.
#[track_caller]
fn chunk_frozen(m: &mut S, chunk: u64) {
    let shift = chunk_shift(m);
    let word =
        dodos_abi::Ownership::from_raw(m.sys.canonical().owners[chunk as usize]).expect("a word");
    assert_eq!(word.state(), dodos_abi::OwnState::Taken);
    assert_eq!(word.owner(), dodos_abi::Owner::Node(X));
    assert_eq!(word.generation(), m.sys.member(X).generation());
    m.no(THIRD, take(0, chunk << shift, shift, 200), Error::Busy);
}

fn uses(m: &S) -> usize {
    m.sys.canonical().uses[Y as usize].len()
}

fn unmapping(m: &S) -> bool {
    m.sys
        .node(Y)
        .caps
        .values()
        .any(|c| matches!(c.record, Record::Unmapping(_)))
}

/// The state of pass 24 (a): the unit fails under a remote frame the holder
/// mapped, and the holder removes its root, so its use entry is kept with no
/// root and a count of 1, and the granting node's grant is pinned.
fn pinned_by_a_failed_unit() -> Granted {
    let mut g = granted();
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    let used = g.m.sys.canonical().uses[Y as usize][&g.use_index];
    assert_eq!((used.root, used.leaves), (None, 1));
    for _ in 0..4 {
        if g.m.sys.canonical().grants[X as usize][&g.grant].state == GrantState::Pinned {
            break;
        }
        if g.m.call(X, Syscall::CapDelete(s(OBJ))) == WAITS {
            g.m.run(30 * MS);
        }
    }
    assert_eq!(
        g.m.sys.canonical().grants[X as usize][&g.grant].state,
        GrantState::Pinned
    );
    g
}

// The holder of the frame is killed under a count the failed unit kept. It
// never dies, because the unit that failed is the one its device sits
// behind, so its barrier never holds (IOMMU, R1: "for such a node the unit
// cannot confirm that the device is blocked"). Its use entry counts the
// leaf, the granting node stays fenced, and no chunk may be taken.
#[test]
fn a_holder_killed_under_a_count_a_failed_unit_kept_never_dies() {
    let mut g = pinned_by_a_failed_unit();
    let generation = g.m.sys.member(Y).generation();
    g.m.ok(THIRD, super::pass17::kill(Y));
    g.m.run(1_000 * MS);
    assert_ne!(g.m.state(Y), MemberState::Dead, "the holder died");
    assert_eq!(uses(&g.m), 1, "the use entry went with the holder");
    let used = g.m.sys.canonical().uses[Y as usize][&g.use_index];
    assert_eq!((used.root, used.leaves), (None, 1));
    assert_ne!(
        g.m.sys.canonical().beacons[THIRD as usize].acked[Y as usize],
        generation
    );
    let chunk = g.chunk;
    let used = g.use_index;
    chunk_frozen(&mut g.m, chunk);
    assert_eq!(g.m.sys.canonical().uses[Y as usize][&used].leaves, 1);
    // The granting node is alive all along, and its `Revoke`/`Release`
    // never frees the entry while the count stands.
    assert_eq!(g.m.state(X), MemberState::Live);
}

// The slot of the holder cannot be
// started again while it is not dead, so its use table cannot be cleared
// under the granting node's generation.
#[test]
fn a_fresh_start_of_a_holder_under_a_kept_count_is_busy() {
    let mut g = pinned_by_a_failed_unit();
    g.m.ok(THIRD, super::pass17::kill(Y));
    g.m.run(500 * MS);
    let call = super::pass17::fresh(Y, 1);
    g.m.no(THIRD, call, Error::Busy);
    assert_eq!(uses(&g.m), 1);
}

// The holder is sealed and never resumes (Reclaim, R2, D-142). A sealed node acknowledges nothing, so the dead
// granting node's barrier never holds and its memory stays its own for as
// long as the machine runs.
#[test]
fn a_sealed_holder_under_a_kept_count_keeps_the_killed_granter_fenced() {
    let mut g = pinned_by_a_failed_unit();
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(1_500 * MS);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    let generation = g.m.sys.member(X).generation();
    assert_ne!(
        g.m.sys.canonical().beacons[Y as usize].acked[X as usize],
        generation
    );
    let chunk = g.chunk;
    chunk_frozen(&mut g.m, chunk);
    assert_eq!(uses(&g.m), 1);
}

// The holder is sealed, its use entry keeps a count, the granting node is
// killed, and the holder is resumed (Grants, "Generations", D-142): the
// resumed holder is live and the
// dead node's memory is still not handed out, for a second of machine time
// and a fresh unit of work on the resumed node.
#[test]
fn a_resumed_holder_under_a_kept_count_runs_and_the_killed_granters_chunk_stays_frozen() {
    let mut g = pinned_by_a_failed_unit();
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(dodos_abi::FENCE_NS + MS);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    g.m.ok(THIRD, super::pass17::resume(Y));
    g.m.run(300 * MS);
    assert_eq!(g.m.state(Y), MemberState::Live);
    // It runs threads again, which was the fault D-142 corrects.
    let notify = g.m.retype(Y, ObjectKind::Notify, 0, 130);
    g.m.ok(Y, notify);
    g.m.run(1_000 * MS);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    let generation = g.m.sys.member(X).generation();
    assert_ne!(
        g.m.sys.canonical().beacons[Y as usize].acked[X as usize],
        generation
    );
    let chunk = g.chunk;
    chunk_frozen(&mut g.m, chunk);
    assert_eq!(uses(&g.m), 1);
}

// A message about a grant from a generation that ended during the seal
// (What crosses between nodes, "At a resume", D-142): a `Send` of the granting node waits in the user ring of
// the sealed holder; it is read during the resume's R2 and refused, and
// writes no use entry and no remote capability.
#[test]
fn a_send_of_a_generation_that_ended_during_the_seal_is_refused_during_the_resume() {
    let mut g = granted();
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.ok(X, retype_from(BLOCK, ObjectKind::Frame, 1, OTHER));
    // A second frame of `X`, in the holder's user ring while it is still
    // live; the seal is asked before any interrupt serves that ring.
    assert_eq!(g.m.call(X, send(OTHER)), WAITS);
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    let waiting = |m: &S| {
        let ring = &m.sys.canonical().rings[X as usize][Y as usize].user;
        ring.produced != m.sys.canonical().beacons[Y as usize].consumed[X as usize].1
    };
    assert!(waiting(&g.m), "the Send waits in the sealed holder's ring");
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(dodos_abi::FENCE_NS + MS);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    let refused = g.m.sys.node(Y).refused.0;
    g.m.ok(THIRD, super::pass17::resume(Y));
    let read = g.m.step_until(|m| !waiting(m));
    assert!(read && unmapping(&g.m), "the Send was not read during R2");
    assert_eq!(g.m.sys.node(Y).refused.0, refused + 1, "not refused");
    assert_eq!(g.m.state(Y), MemberState::Starting);
    g.m.run(300 * MS);
    assert_eq!(g.m.state(Y), MemberState::Live);
    assert_eq!(uses(&g.m), 0, "a use entry for the refused Send");
}

// A release and a revocation at a count the failed unit kept, in the order
// `pass24.rs` does not test (Grants, D-142): the holder releases its root first, while the grant is still live and its
// count above 0 (the `Release` is traced and leaves the entry as it is);
// the granting node's later removal then revokes, the holder answers that
// the root is removed, and the grant is pinned -- the frame and the chunk
// stay the granting node's for ever.
#[test]
fn a_release_at_a_count_above_0_before_the_revoke_leaves_the_grant_live_then_pinned() {
    let mut g = granted();
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    let refused = g.m.sys.node(X).refused.0;
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    // The entry stays live at a count above 0: the Release is traced.
    let entry = g.m.sys.canonical().grants[X as usize][&g.grant];
    assert_eq!(entry.state, GrantState::Live);
    assert_eq!(g.m.sys.node(X).refused.0, refused + 1, "no trace");
    assert_eq!(
        g.m.sys.canonical().grants[X as usize][&g.grant].state,
        GrantState::Live
    );
    assert_eq!(uses(&g.m), 1);
    // Now the granting node reaches the anchor: revoking, then pinned.
    for _ in 0..6 {
        if g.m.sys.canonical().grants[X as usize][&g.grant].state == GrantState::Pinned {
            break;
        }
        if g.m.call(X, Syscall::CapDelete(s(OBJ))) == WAITS {
            g.m.run(30 * MS);
        }
    }
    assert_eq!(
        g.m.sys.canonical().grants[X as usize][&g.grant].state,
        GrantState::Pinned
    );
    let chunk = g.chunk;
    chunk_frozen(&mut g.m, chunk);
    assert!(g.m.sys.node(X).objects.contains_key(&g.frame));
}

// Two grants under one failed unit (Grants, D-142): the holder maps a
// second grant of a second chunk of the same granting node, the unit fails, and both removals run. Both
// use entries are kept, both grants pinned, both chunks frozen.
#[test]
fn two_grants_under_one_failed_unit_are_both_pinned() {
    let mut g = granted();
    let shift = chunk_shift(&g.m);
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let chunk2 =
        g.m.sys
            .canonical()
            .owners
            .iter()
            .position(|&w| w == free)
            .expect("a free chunk") as u64;
    g.m.ok(X, take(0, chunk2 << shift, shift, BLOCK + 1));
    g.m.ok(X, retype_from(BLOCK + 1, ObjectKind::Frame, 1, OTHER));
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    assert_eq!(g.m.call(Y, recv(120)), WAITS);
    assert_eq!(g.m.call(X, send(OTHER)), WAITS);
    g.m.settle();
    let map = io_map(&g.m, 120);
    g.m.ok(Y, map);
    assert_eq!(
        g.m.sys.canonical().uses[Y as usize]
            .values()
            .filter(|u| u.leaves == 1)
            .count(),
        2
    );
    let second = *g.m.sys.canonical().grants[X as usize]
        .keys()
        .find(|&&k| k != g.grant)
        .expect("a second grant");
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    until_done(&mut g.m, Y, Syscall::CapDelete(s(120)));
    g.m.settle();
    assert_eq!(uses(&g.m), 2);
    // Both kept with no root and their count.
    for used in g.m.sys.canonical().uses[Y as usize].values() {
        assert_eq!((used.root, used.leaves), (None, 1));
    }
    for _ in 0..6 {
        let both = g.m.sys.canonical().grants[X as usize]
            .values()
            .all(|e| e.state == GrantState::Pinned);
        if both {
            break;
        }
        if g.m.call(X, Syscall::CapDelete(s(OBJ))) == WAITS {
            g.m.run(30 * MS);
        }
        if g.m.call(X, Syscall::CapDelete(s(OTHER))) == WAITS {
            g.m.run(30 * MS);
        }
    }
    let grants = g.m.sys.canonical().grants[X as usize].clone();
    assert_eq!(
        (grants[&g.grant].state, grants[&second].state),
        (GrantState::Pinned, GrantState::Pinned)
    );
    let chunk = (g.chunk, chunk2);
    chunk_frozen(&mut g.m, chunk.0);
    let word = dodos_abi::Ownership::from_raw(g.m.sys.canonical().owners[chunk.1 as usize])
        .expect("a word");
    assert_eq!(word.owner(), dodos_abi::Owner::Node(X));
    assert_eq!(word.state(), dodos_abi::OwnState::Taken);
}

// The holder of the frame is killed as well as the granting node. The
// holder is fenced and never dies (its own device sat behind the failed unit), so it
// neither acknowledges nor is waited for: the second protection is the
// `maps_of` bit it set when it mapped, which a fenced node's `acked` cannot
// stand in for (The reclaim barrier). The granting node's memory must stay
// frozen.
#[test]
fn killing_the_holder_and_the_granter_under_a_kept_count_leaves_the_granter_frozen() {
    let mut g = pinned_by_a_failed_unit();
    assert!(
        g.m.sys.canonical().beacons[Y as usize].maps_of[X as usize],
        "the holder never mapped under the grant"
    );
    g.m.ok(THIRD, super::pass17::kill(Y));
    g.m.run(500 * MS);
    assert_ne!(g.m.state(Y), MemberState::Dead);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(2_000 * MS);
    assert_ne!(g.m.state(X), MemberState::Dead, "the granter died");
    assert_eq!(g.m.state(X), MemberState::Fenced);
    assert!(
        g.m.sys.canonical().beacons[Y as usize].maps_of[X as usize],
        "the maps bit of the fenced holder was cleared"
    );
    let chunk = g.chunk;
    chunk_frozen(&mut g.m, chunk);
    let used = g.use_index;
    let generation = g.m.sys.member(X).generation();
    assert_ne!(
        g.m.sys.canonical().beacons[Y as usize].acked[X as usize],
        generation
    );
    assert_eq!(g.m.sys.canonical().uses[Y as usize][&used].leaves, 1);
}

// A `Revoke` of a live granting node left in the ring of the sealed holder
// is served at the resume, as on a live node (What crosses between nodes,
// "At a resume", D-142), and the granting node's step ends pinned. Here
// the resume's R2 to R5 end in the event that drains the ring, so the
// answer is written before the node is live or once it is: the test holds
// the pin and that the waiting message is not lost, not the instant.
#[test]
fn a_revoke_left_in_a_sealed_holders_ring_is_served_at_its_resume_and_pins_the_grant() {
    let mut g = granted();
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    assert_eq!(g.m.call(X, Syscall::CapDelete(s(OBJ))), WAITS);
    g.m.run(MS);
    let waiting = |m: &S| {
        let ring = &m.sys.canonical().rings[X as usize][Y as usize].kernel;
        ring.produced != m.sys.canonical().beacons[Y as usize].consumed[X as usize].0
    };
    assert!(
        waiting(&g.m),
        "the Revoke waits in the sealed holder's ring"
    );
    g.m.ok(THIRD, super::pass17::resume(Y));
    let read = g.m.step_until(|m| !waiting(m));
    assert!(read, "the Revoke was never read in the resume");
    assert!(
        matches!(g.m.state(Y), MemberState::Starting | MemberState::Live),
        "the Revoke was not read during the resume: {:?}",
        g.m.state(Y)
    );
    g.m.run(300 * MS);
    assert_eq!(g.m.state(Y), MemberState::Live);
    assert_eq!(
        g.m.sys.canonical().grants[X as usize][&g.grant].state,
        GrantState::Pinned
    );
    let chunk = g.chunk;
    chunk_frozen(&mut g.m, chunk);
}

/// `granted()` without the settle: the granting node has not consumed the
/// `SendAnswer` yet, so the grant entry is not *answered* and no use index
/// is recorded (Grants, the count says what the holder translates).
fn granted_unanswered() -> (S, u16) {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.caching_mode = false;
    let mut m = S::on(&machine);
    attached(&mut m, Y);
    let shift = m.sys.canonical().geometry.chunk_shift();
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let chunk = m
        .sys
        .canonical()
        .owners
        .iter()
        .position(|&w| w == free)
        .expect("a free chunk") as u64;
    m.ok(X, take(0, chunk << shift, u64::from(shift), BLOCK));
    m.ok(X, retype_from(BLOCK, ObjectKind::Frame, 1, OBJ));
    let reply = m.retype(X, ObjectKind::Reply, 0, REPLY);
    m.ok(X, reply);
    let reply = m.retype(Y, ObjectKind::Reply, 0, REPLY);
    m.ok(Y, reply);
    assert_eq!(m.call(Y, recv(GOT)), WAITS);
    assert_eq!(m.call(X, send(OBJ)), WAITS);
    let (&grant, entry) = m.sys.canonical().grants[X as usize]
        .iter()
        .next()
        .expect("the grant");
    assert!(!entry.answered, "the answer was consumed");
    assert_eq!(entry.use_index, None);
    (m, grant)
}

// The first map of a frame just received waits until its sender has
// consumed the answer: `again` while the granting node is live and while
// it is sealed, and `gone` once it is accused (Grants, the count says what
// the holder translates, D-141, C4 of D-142).
#[test]
fn a_first_map_before_the_answer_is_again_while_the_granter_is_live_or_sealed_and_gone_once_accused()
 {
    let (mut m, grant) = granted_unanswered();
    // Delivered to the holder, the answer not yet consumed by the live
    // granting node.
    let delivered = m.step_until(|m| uses(m) == 1);
    let entry = m.sys.canonical().grants[X as usize][&grant];
    assert!(delivered && !entry.answered && m.state(X) == MemberState::Live);
    m.no(Y, io_map(&m, GOT), Error::Again);
    // Sealed before the answer comes: the delivery happens during the seal.
    let (mut m, grant) = granted_unanswered();
    m.ok(THIRD, super::pass17::seal(X));
    m.run(50 * MS);
    assert_eq!(m.state(X), MemberState::Sealed);
    let entry = m.sys.canonical().grants[X as usize][&grant];
    assert!(!entry.answered, "the sealed granter consumed the answer");
    m.no(Y, io_map(&m, GOT), Error::Again);
    let leaves = |m: &S| {
        m.sys.canonical().uses[Y as usize]
            .values()
            .map(|u| u.leaves)
            .sum::<u64>()
    };
    assert_eq!(leaves(&m), 0, "a leaf was counted");
    // Accused: the remote capability is no longer valid, so the map is
    // `gone` at once, before the holder's R2 removes the root.
    m.ok(THIRD, super::pass17::kill(X));
    assert_ne!(m.state(X), MemberState::Live);
    m.no(Y, io_map(&m, GOT), Error::Gone);
    assert_eq!(leaves(&m), 0);
}

// A block under a live grant is never given or split (G6, C11 of D-142:
// the anchor is a descendant of the block's capability). `memory_split`
// takes only the block of a device entry of more than one page, so the
// grant is of a device frame; the holder maps it, and the block is `busy`
// for both calls while the grant is live and while it is revoked, and
// splits once the revocation ends.
#[test]
fn a_block_under_a_live_grant_is_busy_for_a_give_and_a_split() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let page = m.sys.canonical().geometry.page().trailing_zeros();
    let index = m
        .sys
        .canonical()
        .devices
        .iter()
        .position(|e| e.is_used() && e.log2() > page)
        .expect("an entry to split");
    m.ok(X, take(1, index as u64, 0, BLOCK));
    let frame = Syscall::UntypedRetype {
        untyped: s(BLOCK),
        kind: ObjectKind::Frame,
        size: 1,
        page: 0,
        memory: dodos_abi::MemoryType::Uncached as u64,
        destination: s(OBJ),
    };
    m.ok(X, frame);
    for node in [X, Y] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    assert_eq!(m.call(Y, recv(GOT)), WAITS);
    assert_eq!(m.call(X, send(OBJ)), WAITS);
    m.settle();
    let control = s(dodos_abi::START_SLOT_MEMORY_CONTROL);
    let give = Syscall::MemoryGive {
        control,
        untyped: s(BLOCK),
        node: u64::from(THIRD),
    };
    let split = Syscall::MemorySplit {
        control,
        untyped: s(BLOCK),
    };
    // The grant alone, no leaf counted: the anchor is the descendant.
    assert_eq!(m.sys.canonical().device_leaves[index], 0);
    m.no(X, give.clone(), Error::Busy);
    m.no(X, split.clone(), Error::Busy);
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(GOT),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(Y, map);
    // The frame's removal reaches the anchor: the grant is revoking, its
    // `Revoke` in flight, and the block is still `busy` for another thread.
    m.ok(X, super::place_first(X, 0));
    let other = m.thread_on(X, 300, 0);
    assert_eq!(m.call(X, Syscall::CapDelete(s(OBJ))), WAITS);
    assert_eq!(
        m.sys.canonical().grants[X as usize]
            .values()
            .next()
            .map(|e| e.state),
        Some(GrantState::Revoking)
    );
    assert_eq!(m.call_by(X, other, split.clone()), Err(Error::Busy));
    m.run(30 * MS);
    until_done(&mut m, X, Syscall::CapDelete(s(OBJ)));
    assert!(m.sys.canonical().grants[X as usize].is_empty());
    assert!(matches!(m.call(X, split), Ok(Output::Index(_))));
}
