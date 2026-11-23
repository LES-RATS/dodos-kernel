//! Pass 24, D-142: a frame another node granted, mapped in an iospace whose
//! IOMMU unit fails. The holder frees a use entry only at a count of 0; a
//! root removed while the failed unit kept the count leaves the entry with
//! no root, and the granting node pins the grant (C1); a survivor leaves R2
//! undone while such an entry names a dead granter's generation (C2).
//! States are reached by user calls and by the unit's failure
//! (`System::break_unit`).

use super::pass22::take;
use super::units::{IOMMU_ROLE, IOSPACE, attached};
use super::*;
use crate::table::GrantState;
use crate::{Map, Message, Range, Record, Transfer};
use dodos_abi::START_SLOT_MAILBOX;

const REPLY: u32 = 100;
const OBJ: u32 = 101;
const OTHER: u32 = 102;
pub(super) const GOT: u32 = 110;
const GOT_OTHER: u32 = 120;
const BLOCK: u32 = 160;
/// The granting node, the holder, and the third node, which holds the
/// IOMMU unit's role.
pub(super) const X: u16 = 0;
pub(super) const Y: u16 = 2;
pub(super) const THIRD: u16 = 1;

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

pub(super) fn io_map(m: &S, frame: u32) -> Syscall {
    Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(frame),
        untyped: s(super::table_slot(m.sys.node(Y), m.t[Y as usize])),
        address: 0x1000_0000 + u64::from(frame) * 0x10_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    })
}

/// Node `X` grants node `Y` a frame of a chunk it took (slot `BLOCK`),
/// received in slot `slot`; `Y` has an iospace with a device attached.
pub(super) struct Granted {
    pub(super) m: S,
    pub(super) chunk: u64,
    pub(super) frame: ObjId,
    pub(super) grant: u16,
    pub(super) use_index: u16,
}

pub(super) fn granted(caching: bool) -> Granted {
    granted_by(X, caching)
}

/// As `granted`, the frame granted by node `x`.
pub(super) fn granted_by(x: u16, caching: bool) -> Granted {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.caching_mode = caching;
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
    m.ok(x, take(0, chunk << shift, u64::from(shift), BLOCK));
    m.ok(x, retype_from(BLOCK, ObjectKind::Frame, 1, OBJ));
    let frame = m
        .sys
        .node(x)
        .object_at(m.t[x as usize], s(OBJ))
        .expect("the frame");
    for node in [x, Y] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    assert_eq!(m.call(Y, recv(GOT)), WAITS);
    assert_eq!(m.call(x, send(OBJ)), WAITS);
    m.settle();
    let (&grant, entry) = m.sys.canonical().grants[x as usize]
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

/// A call repeated, time passing between, until it is done.
#[track_caller]
pub(super) fn until_done(m: &mut S, node: u16, call: Syscall) {
    for _ in 0..20 {
        match m.call(node, call.clone()) {
            DONE => return,
            WAITS => m.run(30 * MS),
            // A removal's step: time passes; any other answer fails the
            // test (the audit of lot 8a, D-8a F20: every error was retried).
            Err(Error::Again) => m.run(10 * MS),
            other => panic!("{call:?}: {other:?}"),
        }
    }
    panic!("{call:?} never done");
}

/// The holder's use entry: kept with no root, its count above 0.
#[track_caller]
pub(super) fn kept_without_root(g: &Granted) {
    let used = g.m.sys.canonical().uses[Y as usize]
        .get(&g.use_index)
        .copied();
    let used = used.expect("the use entry is kept");
    assert_eq!((used.root, used.leaves), (None, 1));
}

/// The removal on the granting node: once the grant is pinned, `again`
/// with no message, for ever; the grant never freed, the frame never
/// destroyed, the chunk never given or retyped.
#[track_caller]
fn pinned_for_ever(g: &mut Granted) {
    let m = &mut g.m;
    for _ in 0..4 {
        if m.sys.canonical().grants[X as usize][&g.grant].state == GrantState::Pinned {
            break;
        }
        if m.call(X, Syscall::CapDelete(s(OBJ))) == WAITS {
            m.run(30 * MS);
        }
    }
    let entry = m.sys.canonical().grants[X as usize][&g.grant];
    assert_eq!(entry.state, GrantState::Pinned);
    let rings = |m: &S| {
        let x = m.sys.node(X);
        (
            x.ring_of(X, Y, false).produced,
            x.ring_of(X, Y, true).produced,
        )
    };
    let before = rings(m);
    for _ in 0..3 {
        m.no(X, Syscall::CapDelete(s(OBJ)), Error::Again);
        m.run(100 * MS);
    }
    assert_eq!(rings(m), before, "a message for a pinned grant");
    assert!(m.sys.canonical().grants[X as usize].contains_key(&g.grant));
    assert!(m.sys.node(X).objects.contains_key(&g.frame));
    let give = Syscall::MemoryGive {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        untyped: s(BLOCK),
        node: u64::from(THIRD),
    };
    m.no(X, give, Error::Busy);
    let taken = m.sys.canonical().owners[g.chunk as usize];
    let word = dodos_abi::Ownership::from_raw(taken).expect("a word");
    assert_eq!(word.owner(), dodos_abi::Owner::Node(X));
    assert_eq!(word.state(), dodos_abi::OwnState::Taken);
    kept_without_root(g);
}

// (a) The unit fails, then the holder removes its root itself: its
// `Release` finds a count above 0 and is traced, the entry stays live; the
// revocation then ends pinned.
#[test]
fn a_unit_failed_under_a_remote_frame_pins_the_grant_at_the_holders_removal() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    let refused = g.m.sys.node(X).refused.0;
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    kept_without_root(&g);
    assert_eq!(g.m.sys.node(X).refused.0, refused + 1);
    let entry = g.m.sys.canonical().grants[X as usize][&g.grant];
    assert_eq!(entry.state, GrantState::Live);
    pinned_for_ever(&mut g);
}

// (b) The unit fails while the holder's removal step for a `Revoke` waits
// on it: the record is emptied with its count kept, the next `Revoke`
// removes the root, and its `RevokeStep` pins the grant.
#[test]
fn a_unit_failed_during_a_revocation_pins_the_grant() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    assert_eq!(g.m.call(X, Syscall::CapDelete(s(OBJ))), WAITS);
    let unmapping = g.m.step_until(|m| {
        m.sys
            .node(Y)
            .caps
            .values()
            .any(|c| matches!(c.record, Record::Unmapping(_)))
    });
    assert!(unmapping, "the holder's step never asked the unit");
    g.m.sys.break_unit(IOMMU_ROLE);
    g.m.run(30 * MS);
    pinned_for_ever(&mut g);
}

// (c) A `Release` crossing a `Revoke`: the holder removes its root (the
// unit failed, the record emptied with its count kept) while the `Revoke`
// is in its ring; the `Release` pins the revoking entry and the credit of
// the `Revoke` stays in use until its answer, which comes.
#[test]
fn a_release_crossing_a_revoke_at_a_count_above_0_pins_and_keeps_the_credit() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    assert_eq!(g.m.call(Y, Syscall::IoSpaceUnmap(s(GOT))), WAITS);
    g.m.run(30 * MS);
    assert_eq!(g.m.woken(Y), Some(&Err(Error::Busy)));
    assert_eq!(g.m.call(X, Syscall::CapDelete(s(OBJ))), WAITS);
    g.m.ok(Y, Syscall::CapDelete(s(GOT)));
    let revoking = |m: &S| {
        m.sys.node(X).peers[Y as usize].credits.iter().flatten().any(
        |c| matches!(c.completes, crate::rings::Completes::Revoke { grant } if grant == g.grant),
    )
    };
    // The `Release` pins the entry before the `RevokeStep` comes: the
    // credit is still in use when the entry is first pinned.
    let grant = g.grant;
    let pinned = |m: &S| m.sys.canonical().grants[X as usize][&grant].state == GrantState::Pinned;
    assert!(g.m.step_until(pinned));
    assert!(revoking(&g.m), "the credit of the Revoke was freed");
    g.m.settle();
    assert!(!revoking(&g.m), "the Revoke was never answered");
    pinned_for_ever(&mut g);
}

// (d) The unit fails before the map: `iospace_map`, which must ask a unit
// that caches absent entries, is answered *failed* and is `busy` with the
// mapping kept and counted; the revocation then ends pinned.
#[test]
fn a_unit_failed_before_the_map_pins_the_grant() {
    let mut g = granted(true);
    g.m.sys.break_unit(IOMMU_ROLE);
    let map = io_map(&g.m, GOT);
    assert_eq!(g.m.call(Y, map), WAITS);
    g.m.run(30 * MS);
    assert_eq!(g.m.woken(Y), Some(&Err(Error::Busy)));
    pinned_for_ever(&mut g);
}

// A use entry kept with no root takes no new leaf and is not written for
// another grant: the next `Send` takes another use index (the gate takes
// the first free one), and a map under it counts in its own entry.
#[test]
fn a_use_entry_kept_without_a_root_is_never_written_for_another_grant() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    kept_without_root(&g);
    g.m.ok(X, retype_from(BLOCK, ObjectKind::Frame, 1, OTHER));
    assert_eq!(g.m.call(Y, recv(GOT_OTHER)), WAITS);
    assert_eq!(g.m.call(X, send(OTHER)), WAITS);
    g.m.settle();
    let uses = &g.m.sys.canonical().uses[Y as usize];
    assert_eq!(uses.len(), 2);
    let other = *uses
        .keys()
        .find(|&&u| u != g.use_index)
        .expect("a new entry");
    let aspace = Syscall::ASpaceMap(Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(GOT_OTHER),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    g.m.ok(Y, aspace);
    assert_eq!(g.m.sys.canonical().uses[Y as usize][&other].leaves, 1);
    kept_without_root(&g);
}

// The gate refuses (`gone`) a new leaf under a use entry whose root is
// removed (Grants, D-142); no call reaches it with a remote capability
// left, so the state is written as a faulty kernel of the holder would.
#[test]
fn the_gate_refuses_a_leaf_under_a_use_entry_with_no_root() {
    let mut g = granted(false);
    let index = g.use_index;
    g.m.sys.edit_table(move |t| {
        t.uses[Y as usize]
            .get_mut(&index)
            .expect("the use entry")
            .root = None;
    });
    let map = io_map(&g.m, GOT);
    g.m.no(Y, map, Error::Gone);
    assert_eq!(g.m.sys.canonical().uses[Y as usize][&index].leaves, 0);
}

/// The granting node killed by the third node once the holder's unit failed (or
/// not): time for the fence, the reclaim and the barrier.
pub(super) fn granter_killed(fail: bool) -> Granted {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    if fail {
        g.m.sys.break_unit(IOMMU_ROLE);
        until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
        g.m.settle();
        kept_without_root(&g);
    }
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(500 * MS);
    g
}

// C2: the granting node dies after the failure: the holder leaves R2
// undone and never acknowledges, so the dead node's barrier never holds
// and its chunks stay taken.
#[test]
fn a_granter_killed_under_a_count_a_failed_unit_kept_is_never_dead() {
    let g = granter_killed(true);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    let generation = g.m.sys.member(X).generation();
    assert_ne!(
        g.m.sys.canonical().beacons[Y as usize].acked[X as usize],
        generation
    );
    let word = dodos_abi::Ownership::from_raw(g.m.sys.canonical().owners[g.chunk as usize]);
    let word = word.expect("a word");
    assert_eq!(word.state(), dodos_abi::OwnState::Taken);
    assert_eq!(word.generation(), generation);
    kept_without_root(&g);
}

// The control case: the same death with the unit healthy; the barrier
// holds.
#[test]
fn a_granter_killed_with_its_holders_unit_healthy_is_dead() {
    let g = granter_killed(false);
    assert_eq!(g.m.state(X), MemberState::Dead);
}

/// The holder sealed, the granting node killed (its core hung first when
/// `unfenced`), then the holder resumed by the third node; `fail` breaks
/// the unit and removes the holder's root first, so that its use entry
/// keeps a count with no root.
fn resumed_over_a_dead_granter(fail: bool, unfenced: bool) -> Granted {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    if fail {
        g.m.sys.break_unit(IOMMU_ROLE);
        until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
        g.m.settle();
    }
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    if unfenced {
        // In VMX root operation, where INIT is blocked (D-149).
        g.m.sys.set_vmx(0, true);
        g.m.sys.hang(0, false);
    }
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(dodos_abi::FENCE_NS + MS);
    let accused = if unfenced {
        MemberState::Unfenced
    } else {
        MemberState::Fenced
    };
    assert_eq!(g.m.state(X), accused);
    g.m.ok(THIRD, super::pass17::resume(Y));
    g
}

/// The holder runs again: live, and its first thread's call done; the
/// granting node is never dead, its chunk stays taken by its generation,
/// no acknowledgement of it is written, and the kept use entry stays.
#[track_caller]
fn live_and_the_granter_frozen(g: &mut Granted, accused: MemberState) {
    g.m.run(200 * MS);
    assert_eq!(g.m.state(Y), MemberState::Live);
    let notify = g.m.retype(Y, ObjectKind::Notify, 0, 130);
    g.m.ok(Y, notify);
    g.m.run(500 * MS);
    assert_eq!(g.m.state(X), accused);
    let generation = g.m.sys.member(X).generation();
    assert_ne!(
        g.m.sys.canonical().beacons[Y as usize].acked[X as usize],
        generation
    );
    let word = dodos_abi::Ownership::from_raw(g.m.sys.canonical().owners[g.chunk as usize]);
    let word = word.expect("a word");
    assert_eq!(word.state(), dodos_abi::OwnState::Taken);
    assert_eq!(word.generation(), generation);
    kept_without_root(g);
}

// D-142 at a resume: the holder sealed with a count the failed unit kept,
// the granting node killed and fenced; the resume does not wait for the R2
// that count leaves undone: the holder runs its threads, and the granting
// node's barrier never holds (Grants, "Generations").
#[test]
fn a_holder_resumed_under_a_count_a_failed_unit_kept_runs_again_beside_a_fenced_granter() {
    let mut g = resumed_over_a_dead_granter(true, false);
    live_and_the_granter_frozen(&mut g, MemberState::Fenced);
}

// The same with the granting node unfenced.
#[test]
fn a_holder_resumed_under_a_count_a_failed_unit_kept_runs_again_beside_an_unfenced_granter() {
    let mut g = resumed_over_a_dead_granter(true, true);
    live_and_the_granter_frozen(&mut g, MemberState::Unfenced);
}

/// Whether the holder has a record unmapping.
pub(super) fn unmapping(m: &S) -> bool {
    m.sys
        .node(Y)
        .caps
        .values()
        .any(|c| matches!(c.record, Record::Unmapping(_)))
}

// The control case: the unit healthy, the holder's mapping is undone at the
// resume by a request to the third node's unit; while the record is
// unmapping the resume waits, it reads the unit's answer on its kernel
// ring while R2 goes on (What crosses between nodes, "At a resume",
// D-142), the record completes, the resume ends, and the granting node's
// barrier holds: it is dead and its chunk can be taken by another node.
#[test]
fn a_resume_reads_the_answer_of_another_nodes_unit_during_r2_and_ends() {
    let mut g = resumed_over_a_dead_granter(false, false);
    assert!(g.m.step_until(unmapping), "the resume never asked the unit");
    assert_eq!(g.m.state(Y), MemberState::Starting);
    g.m.run(300 * MS);
    assert!(!unmapping(&g.m));
    assert_eq!(g.m.state(Y), MemberState::Live);
    assert!(g.m.sys.canonical().uses[Y as usize].is_empty());
    assert_eq!(g.m.state(X), MemberState::Dead);
    let shift = g.m.sys.canonical().geometry.chunk_shift();
    g.m.ok(THIRD, take(0, g.chunk << shift, u64::from(shift), 170));
}

// The same, with the third node's unit failing while the request is
// pending: the answer is *failed*, the record is emptied with its count
// kept, the holder becomes live, and the granting node never dies.
#[test]
fn a_resume_whose_unit_request_is_answered_failed_runs_again_and_the_granter_never_dies() {
    let mut g = resumed_over_a_dead_granter(false, false);
    assert!(g.m.step_until(unmapping), "the resume never asked the unit");
    g.m.sys.break_unit(IOMMU_ROLE);
    live_and_the_granter_frozen(&mut g, MemberState::Fenced);
}

// A message that waited from a generation that is over is refused when the
// resume reads it during R2: node X's `Revoke`, written while the holder was
// sealed, is read while the holder's record is still unmapping, after X was
// killed.
#[test]
fn a_message_of_an_ended_generation_read_during_the_resumes_r2_is_refused() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.ok(THIRD, super::pass17::seal(Y));
    g.m.run(50 * MS);
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    assert_eq!(g.m.call(X, Syscall::CapRevoke(s(OBJ))), WAITS);
    g.m.run(MS);
    let waiting = |m: &S| {
        let ring = &m.sys.canonical().rings[X as usize][Y as usize].kernel;
        ring.produced != m.sys.canonical().beacons[Y as usize].consumed[X as usize].0
    };
    assert!(
        waiting(&g.m),
        "the Revoke waits in the sealed holder's ring"
    );
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(dodos_abi::FENCE_NS + MS);
    assert_eq!(g.m.state(X), MemberState::Fenced);
    let refused = g.m.sys.node(Y).refused.0;
    g.m.ok(THIRD, super::pass17::resume(Y));
    let read = g.m.step_until(|m| !waiting(m));
    assert!(read && unmapping(&g.m), "not read during R2");
    assert_eq!(g.m.state(Y), MemberState::Starting);
    assert_eq!(g.m.sys.node(Y).refused.0, refused + 1);
}

// An answer for a thread of the resuming node, read during R2, is applied
// (the thread's wait ends) but the thread runs only once the resume ends.
#[test]
fn an_answer_read_during_the_resumes_r2_wakes_its_thread_which_runs_only_after_the_resume() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    let send = Syscall::NodeSend {
        node: node_cap(THIRD),
        message: Message {
            label: 9,
            flags: 0,
            words: vec![9],
            caps: vec![],
            marks: 0,
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(g.m.call(Y, send), WAITS);
    g.m.ok(THIRD, super::pass17::seal(Y));
    // The third node takes no interrupt until the holder is sealed, so the
    // `Send` waits in its user ring and its answer comes after the seal.
    for _ in 0..100_000 {
        if g.m.state(Y) == MemberState::Sealed {
            break;
        }
        let events = g.m.sys.events();
        let next = events
            .iter()
            .copied()
            .find(|e| !matches!(*e, crate::system::Event::Ipi { node, .. } if node == THIRD));
        match next {
            Some(event) => {
                g.m.sys.fire(event);
                assert_eq!(g.m.sys.check(), Ok(()));
            }
            None => {
                let next = g.m.sys.now() + 100_000;
                g.m.sys.move_clock(next);
            }
        }
    }
    assert_eq!(g.m.state(Y), MemberState::Sealed);
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(dodos_abi::FENCE_NS + MS);
    let thread = g.m.t[Y as usize];
    let waits = |m: &S| {
        matches!(
            m.sys.node(Y).threads[&m.t[Y as usize]].wait,
            crate::Wait::Node { .. }
        )
    };
    assert!(waits(&g.m), "the answer was read while sealed");
    g.m.ok(THIRD, super::pass17::resume(Y));
    let woken = g.m.step_until(|m| !waits(m));
    assert!(woken && unmapping(&g.m), "not read during R2");
    assert_eq!(g.m.woken(Y), Some(&Err(Error::WouldWait)));
    assert_eq!(g.m.state(Y), MemberState::Starting);
    // Event by event until the resume ends: the thread never runs before.
    let ended = g.m.step_until(|m| {
        let live = m.state(Y) == MemberState::Live;
        assert!(
            live || m.sys.node(Y).running() != Some(thread),
            "ran during the resume"
        );
        live
    });
    assert!(ended, "the resume never ended");
    let notify = g.m.retype(Y, ObjectKind::Notify, 0, 130);
    g.m.ok(Y, notify);
}

// The checker's rule for C2: the granting generation of a leaf a running
// holder still counts is never over. The state is the one above, with the
// granting node's member word written dead as a faulty barrier would.
#[test]
fn a_granter_dead_under_a_leaf_a_running_holder_counts_is_reported() {
    let mut g = granter_killed(true);
    let member = g.m.sys.member(X);
    let dead = dodos_abi::Member::new(
        MemberState::Dead,
        member.evidence(),
        member.driver(),
        member.generation(),
    )
    .with_class(member.class());
    g.m.sys.edit_table(move |t| t.write_member(THIRD, X, dead));
    assert_eq!(
        g.m.sys.check(),
        Err("G6: a granting generation over while a running holder counts a leaf".into())
    );
}

/// The state of (a) with the holder's `Release` left pending, as a kernel
/// ring to the granting node with no room beyond its reserve leaves it
/// (Grants, release by the holder).
fn kept_with_release_pending() -> Granted {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    let index = g.use_index;
    g.m.sys.edit_table(move |t| {
        t.uses[Y as usize]
            .get_mut(&index)
            .expect("kept")
            .release_pending = true;
    });
    g
}

// The pending `Release` goes when room comes, and the entry stays: it is
// freed only at a count of 0 (D-142).
#[test]
fn a_pending_release_sent_at_a_count_above_0_keeps_the_use_entry() {
    let mut g = kept_with_release_pending();
    let before = written_to_x(&g);
    g.m.run(10 * MS);
    kept_without_root(&g);
    assert!(!g.m.sys.canonical().uses[Y as usize][&g.use_index].release_pending);
    // Sent: one message more on Y's kernel ring toward X (the audit of lot
    // 8a, D-8a F21: sent and dropped ended alike).
    assert_eq!(written_to_x(&g), before + 1);
}

/// The entries Y wrote on its kernel ring toward X.
fn written_to_x(g: &Granted) -> u64 {
    g.m.sys.canonical().rings[Y as usize][X as usize]
        .kernel
        .produced
}

// R0 drops the pending release of a granting generation that is over, and
// the entry stays at a count above 0 (Grants, "X dies", D-142).
#[test]
fn r0_drops_a_pending_release_at_a_count_above_0_and_keeps_the_use_entry() {
    let mut g = kept_with_release_pending();
    let before = written_to_x(&g);
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(10 * MS);
    kept_without_root(&g);
    // Dropped: nothing written toward X.
    assert_eq!(written_to_x(&g), before);
    assert!(!g.m.sys.canonical().uses[Y as usize][&g.use_index].release_pending);
}

// The granting node is killed while its `Revoke` waits in the holder's
// ring: the holder's R0 drops it unanswered, and the dead node's credit is
// not judged (found by the exploration of D-142, seed 1).
#[test]
fn a_revoke_in_flight_when_the_granting_node_is_killed_leaves_a_credit_no_check_judges() {
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    assert_eq!(g.m.call(X, Syscall::CapRevoke(s(OBJ))), WAITS);
    // The `Revoke` holds a credit of X toward Y (the audit of lot 8a,
    // D-8a F06: nothing showed it).
    let credits = |g: &Granted| {
        g.m.sys.node(X).peers[Y as usize]
            .credits
            .iter()
            .flatten()
            .count()
    };
    assert!(credits(&g) > 0, "no credit in use");
    g.m.supervisor(THIRD, g.m.t[THIRD as usize]);
    g.m.ok(THIRD, super::pass17::kill(X));
    g.m.run(50 * MS);
    assert_ne!(g.m.state(X), MemberState::Live);
    // Y's R0 dropped the `Revoke` unanswered, and the check, run after
    // every step, never judged the dead node's credit.
    assert_eq!(g.m.sys.check(), Ok(()));
}
