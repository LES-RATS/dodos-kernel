//! What crosses between nodes (D-128, D-129, D-131): `node_send` and its
//! `SendAnswer`, `grant_signal` and the doorbell, the revocation and the
//! release of a grant, the one check on every message received, the two
//! rings per ordered pair with their credits and their reserve of answers,
//! and what ends with a generation (R0), a fresh start and a resume.

use super::*;
use crate::rings::Body;
use crate::table::GrantState;
use crate::{Message, Range, Transfer, Wait};
use dodos_abi::{
    NODE_START_FRESH, NODE_START_RESUME, NODE_STOP_KILL, NODE_STOP_SEAL, RingHeader,
    SENDS_PER_PEER, START_SLOT_MAILBOX,
};

const REPLY: u32 = 100;
const OBJ: u32 = 101;
const GOT: u32 = 110;
const BELL: u32 = 120;
const AT: u64 = 1 << 30;

fn msg(label: u32, caps: Vec<Transfer>) -> Message {
    Message {
        label,
        flags: 0,
        words: vec![label.into()],
        caps,
        marks: 0,
    }
}

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

fn send(to: u16, caps: Vec<Transfer>, doorbell: SlotAddr) -> Syscall {
    Syscall::NodeSend {
        node: node_cap(to),
        message: msg(7, caps),
        doorbell,
    }
}

fn frame_rights() -> u32 {
    Rights::READ
        .union(Rights::WRITE)
        .union(Rights::SEND)
        .union(Rights::DERIVE)
        .bits()
}

/// Two nodes; node 1 has a reply object to receive with; node 0 has a frame
/// at `OBJ`.
fn two() -> S {
    let mut m = S::two();
    let reply = m.retype(1, ObjectKind::Reply, 0, REPLY);
    m.ok(1, reply);
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    m
}

/// Node 1 waits on its mailbox; node 0 grants it the frame `OBJ` with a
/// doorbell (or none); every message settles; the grant's index.
fn granted(m: &mut S, doorbell: SlotAddr) -> u16 {
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    let call = send(1, vec![Transfer::Derive(s(OBJ), frame_rights())], doorbell);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    let entries = m.sys.node(0).grant_entries();
    assert_eq!(entries.len(), 1);
    entries[0].0
}

fn map_got(m: &mut S) {
    m.ok(
        1,
        Syscall::ASpaceMap(crate::Map {
            space: s(dodos_abi::START_SLOT_ASPACE),
            frame: s(GOT),
            untyped: SlotAddr::NONE,
            address: AT,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
}

// `node_send`.

#[test]
fn node_send_delivers_to_the_first_waiting_receiver_with_the_remote_frames() {
    let mut m = two();
    let grant = granted(&mut m, SlotAddr::NONE);
    // The receiver got the message with its stamp and the remote frame.
    assert!(matches!(m.woken(1), Some(Ok(Output::Message(_)))));
    let frame = m.sys.node(1).object_at(m.t[1], s(GOT)).unwrap();
    assert!(m.sys.node(1).remotes.contains_key(&frame));
    // The sender recorded the use index and marked the entry answered.
    let entry = m.sys.canonical().grants[0][&grant];
    assert!(entry.answered && entry.use_index.is_some());
    assert_eq!(entry.state, GrantState::Live);
    // Its credit is free again.
    assert_eq!(m.sys.node(0).peers[1].sends(), (0, 0));
}

#[test]
fn node_send_with_no_waiting_receiver_is_would_wait_and_frees_its_grants() {
    let mut m = two();
    let call = send(
        1,
        vec![Transfer::Derive(s(OBJ), frame_rights())],
        SlotAddr::NONE,
    );
    assert_eq!(m.call(0, call), WAITS);
    assert_eq!(m.sys.node(0).grant_entries().len(), 1);
    m.settle();
    assert_eq!(m.woken(0), Some(&Err(Error::WouldWait)));
    assert!(m.sys.node(0).grant_entries().is_empty());
    // Nothing waits in the ring for a user thread: it was consumed.
    assert!(!m.sys.node(1).rings_waiting());
}

#[test]
fn node_send_is_judged_in_order() {
    let mut m = two();
    // The caller's own node: invalid.
    m.no(0, send(0, vec![], SlotAddr::NONE), Error::Invalid);
    // A move, or a capability that is not a frame: invalid.
    m.no(
        0,
        send(1, vec![Transfer::Move(s(OBJ))], SlotAddr::NONE),
        Error::Invalid,
    );
    let notify = m.retype(0, ObjectKind::Notify, 0, BELL);
    m.ok(0, notify);
    m.no(
        0,
        send(
            1,
            vec![Transfer::Derive(s(BELL), frame_rights())],
            SlotAddr::NONE,
        ),
        Error::Invalid,
    );
    // The doorbell must be a notify.
    m.no(0, send(1, vec![], s(OBJ)), Error::Invalid);
    // `grant` on the node when the list is not empty.
    m.ok(
        0,
        Syscall::CapDerive {
            source: node_cap(1),
            destination: s(130),
            rights: Rights::SEND.bits(),
            badge: 0,
        },
    );
    let derive = Syscall::NodeSend {
        node: s(130),
        message: msg(7, vec![Transfer::Derive(s(OBJ), frame_rights())]),
        doorbell: SlotAddr::NONE,
    };
    m.no(0, derive, Error::Denied);
    // A node that is not live: gone.
    m.ok(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_KILL,
            core: 0,
        },
    );
    m.no(0, send(1, vec![], SlotAddr::NONE), Error::Gone);
}

#[test]
fn node_send_is_busy_beyond_the_sends_in_flight_to_a_peer_two_kept_for_manage() {
    let mut m = two();
    // Node 1 hangs: no answer comes back, the credits stay in use.
    m.sys.hang(1, true);
    m.ok(0, super::place_first(0, 0));
    let mut senders = vec![m.t[0]];
    for i in 1..=SENDS_PER_PEER as u32 {
        senders.push(m.thread_on(0, 300 + 4 * i, 0));
    }
    // Without `manage`: six sends at most.
    m.ok(
        0,
        Syscall::CapDerive {
            source: node_cap(1),
            destination: s(130),
            rights: Rights::SEND.union(Rights::GRANT).bits(),
            badge: 0,
        },
    );
    let plain = |m: &mut S, thread| {
        let call = Syscall::NodeSend {
            node: s(130),
            message: msg(7, vec![]),
            doorbell: SlotAddr::NONE,
        };
        m.call_by(0, thread, call)
    };
    let limit = SENDS_PER_PEER - dodos_abi::SENDS_RESERVED_FOR_MANAGE;
    for &thread in &senders[..limit] {
        assert_eq!(plain(&mut m, thread), WAITS);
    }
    assert_eq!(plain(&mut m, senders[limit]), Err(Error::Busy));
    // With `manage`: up to eight.
    for &thread in &senders[limit..SENDS_PER_PEER] {
        assert_eq!(m.call_by(0, thread, send(1, vec![], SlotAddr::NONE)), WAITS);
    }
    let last = senders[SENDS_PER_PEER];
    assert_eq!(
        m.call_by(0, last, send(1, vec![], SlotAddr::NONE)),
        Err(Error::Busy)
    );
}

#[test]
fn a_send_answer_is_applied_whether_or_not_the_sending_thread_still_waits() {
    let mut m = two();
    m.ok(0, super::place_first(0, 0));
    let sender = m.thread_on(0, 300, 0);
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    let call = send(
        1,
        vec![Transfer::Derive(s(OBJ), frame_rights())],
        SlotAddr::NONE,
    );
    assert_eq!(m.call_by(0, sender, call), WAITS);
    // The sending thread is destroyed before the answer comes.
    let mut outcome = m.call(0, Syscall::CapDelete(s(300)));
    while outcome == Err(Error::Again) {
        outcome = m.call(0, Syscall::CapDelete(s(300)));
    }
    assert_eq!(outcome, DONE);
    m.settle();
    // The answer still recorded the use index and freed the credit.
    let entries = m.sys.canonical().grants[0].clone();
    assert_eq!(entries.len(), 1);
    assert!(
        entries
            .values()
            .all(|e| e.answered && e.use_index.is_some())
    );
    assert_eq!(m.sys.node(0).peers[1].sends(), (0, 0));
}

#[test]
fn a_frame_delivered_to_a_full_range_is_dropped_with_the_flag_caps_dropped() {
    let mut m = two();
    // The receiver's range has no slot.
    let recv = Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(REPLY),
        range: Range {
            first: SlotAddr::NONE,
            count: 0,
        },
        wait: 0,
    };
    assert_eq!(m.call(1, recv), WAITS);
    let call = send(
        1,
        vec![Transfer::Derive(s(OBJ), frame_rights())],
        SlotAddr::NONE,
    );
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    assert_eq!(
        m.woken(0),
        Some(&Ok(Output::Sent {
            flags: dodos_abi::Header::FLAG_CAPS_DROPPED
        }))
    );
    assert!(m.sys.node(0).grant_entries().is_empty());
}

// `grant_signal`.

#[test]
fn grant_signal_rings_the_doorbell_of_a_live_grant() {
    let mut m = two();
    let notify = m.retype(0, ObjectKind::Notify, 0, BELL);
    m.ok(0, notify);
    granted(&mut m, s(BELL));
    m.ok(1, Syscall::GrantSignal(s(GOT)));
    m.settle();
    assert!(matches!(m.call(0, Syscall::Poll(s(BELL))), Ok(Output::Word(w)) if w != 0));
}

#[test]
fn grant_signal_is_invalid_on_a_frame_that_is_not_remote_and_needs_send() {
    let mut m = two();
    granted(&mut m, SlotAddr::NONE);
    m.no(0, Syscall::GrantSignal(s(OBJ)), Error::Invalid);
    m.ok(
        1,
        Syscall::CapDerive {
            source: s(GOT),
            destination: s(GOT + 1),
            rights: Rights::READ.bits(),
            badge: 0,
        },
    );
    m.no(1, Syscall::GrantSignal(s(GOT + 1)), Error::Denied);
    // A grant without a doorbell: the signal changes nothing, and the call
    // never waits.
    m.ok(1, Syscall::GrantSignal(s(GOT)));
    m.settle();
}

// Revocation and release.

#[test]
fn removing_the_anchor_revokes_the_grant_and_the_holder_removes_its_remote_frame() {
    let mut m = two();
    let grant = granted(&mut m, SlotAddr::NONE);
    map_got(&mut m);
    // The granting node removes what is below its frame: the anchor is
    // reached, the grant becomes revoking and a `Revoke` is sent; the
    // thread waits for the `RevokeStep`.
    assert_eq!(m.call(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    assert_eq!(
        m.sys.canonical().grants[0][&grant].state,
        GrantState::Revoking
    );
    // A revoking grant makes its remote capabilities invalid at once.
    let holder = m.sys.node(1);
    let remote = holder
        .remotes
        .values()
        .next()
        .copied()
        .expect("a remote frame");
    assert!(!holder.valid(&remote));
    m.run(20 * MS);
    // The holder removed its root remote capability with its mapping, and
    // answered: the grant is freed.
    assert_eq!(m.sys.node(1).translated(m.t[1], AT), None);
    assert!(m.sys.node(0).grant_entries().is_empty());
    let woken = m.woken(0).cloned();
    assert!(
        matches!(woken, Some(Ok(Output::Done)) | Some(Err(Error::Again))),
        "{woken:?}"
    );
}

#[test]
fn a_release_by_the_holder_frees_the_grant() {
    let mut m = two();
    granted(&mut m, SlotAddr::NONE);
    m.ok(1, Syscall::CapDelete(s(GOT)));
    m.settle();
    assert!(m.sys.node(0).grant_entries().is_empty());
    assert!(m.sys.canonical().uses[1].is_empty());
}

#[test]
fn a_release_that_crosses_a_revoke_ends_the_revocation() {
    let mut m = two();
    let grant = granted(&mut m, SlotAddr::NONE);
    map_got(&mut m);
    // Both at once: the granter revokes, the holder releases; neither has
    // seen the other's message.
    assert_eq!(m.call(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    m.ok(1, Syscall::CapDelete(s(GOT)));
    m.run(20 * MS);
    assert!(!m.sys.canonical().grants[0].contains_key(&grant));
    assert!(m.sys.canonical().uses[1].is_empty());
    assert!(m.woken(0).is_some());
}

// The one check on every message received.

fn refused_count(m: &S, node: u16) -> u64 {
    m.sys.node(node).refused.0
}

fn message(
    kind: dodos_abi::NodeMessage,
    sender: (u16, u32),
    receiver: u32,
    credit: Option<(u8, u32)>,
    body: Body,
) -> crate::rings::Message {
    crate::rings::Message {
        header: RingHeader::new(kind, body.entries() as u8, sender, receiver, credit),
        body,
    }
}

#[test]
fn a_message_whose_sender_generation_is_not_current_is_refused() {
    let mut m = two();
    let before = refused_count(&m, 1);
    let ping = message(dodos_abi::NodeMessage::Ping, (0, 9), 1, None, Body::Ping);
    m.sys.inject(0, 1, ping);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 1), before + 1);
    // The same message with the sender's generation passes.
    let ping = message(dodos_abi::NodeMessage::Ping, (0, 1), 1, None, Body::Ping);
    m.sys.inject(0, 1, ping);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 1), before + 1);
}

#[test]
fn a_message_to_an_earlier_generation_of_the_receiver_is_refused() {
    let mut m = two();
    let before = refused_count(&m, 1);
    let ping = message(dodos_abi::NodeMessage::Ping, (0, 1), 0, None, Body::Ping);
    m.sys.inject(0, 1, ping);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 1), before + 1);
}

#[test]
fn a_message_whose_kind_is_not_that_of_its_body_is_refused() {
    let mut m = two();
    let before = refused_count(&m, 1);
    let mut ping = message(dodos_abi::NodeMessage::Ping, (0, 1), 1, None, Body::Ping);
    ping.header.kind = dodos_abi::NodeMessage::Handed as u8;
    m.sys.inject(0, 1, ping);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 1), before + 1);
}

#[test]
fn an_answer_that_names_no_credit_in_use_is_refused_and_changes_nothing() {
    let mut m = two();
    let before = refused_count(&m, 0);
    let answer = message(
        dodos_abi::NodeMessage::SendAnswer,
        (1, 1),
        1,
        Some((16, 3)),
        Body::SendAnswer {
            delivered: true,
            dropped: false,
            uses: vec![],
        },
    );
    m.sys.inject(1, 0, answer);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 0), before + 1);
}

#[test]
fn a_release_or_a_signal_that_names_a_grant_entry_of_another_grantee_or_serial_is_refused() {
    let mut m = two();
    let notify = m.retype(0, ObjectKind::Notify, 0, BELL);
    m.ok(0, notify);
    let grant = granted(&mut m, s(BELL));
    let serial = m.sys.canonical().grants[0][&grant].serial;
    let before = refused_count(&m, 0);
    let signal = |serial| {
        message(
            dodos_abi::NodeMessage::Signal,
            (1, 1),
            1,
            None,
            Body::Signal { grant, serial },
        )
    };
    m.sys.inject(1, 0, signal(serial + 1));
    m.settle();
    assert_eq!(refused_count(&m, 0), before + 1);
    assert_eq!(m.call(0, Syscall::Poll(s(BELL))), Err(Error::WouldWait));
    let release = message(
        dodos_abi::NodeMessage::Release,
        (1, 1),
        1,
        None,
        Body::Release {
            grant,
            serial: serial + 1,
        },
    );
    m.sys.inject(1, 0, release);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 0), before + 2);
    assert!(m.sys.canonical().grants[0].contains_key(&grant));
}

#[test]
fn a_handed_that_does_not_name_the_current_epoch_and_this_node_as_target_is_refused() {
    let mut m = two();
    let before = refused_count(&m, 0);
    let handed = message(
        dodos_abi::NodeMessage::Handed,
        (1, 1),
        1,
        None,
        Body::Handed { role: 0, epoch: 77 },
    );
    m.sys.inject(1, 0, handed);
    m.run(20 * MS);
    assert_eq!(refused_count(&m, 0), before + 1);
}

#[test]
fn a_send_whose_stamp_names_a_task_of_another_node_is_refused() {
    let mut m = two();
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    let before = refused_count(&m, 1);
    let forged = message(
        dodos_abi::NodeMessage::Send,
        (0, 1),
        1,
        Some((dodos_abi::REQUESTS_PER_PEER as u8, 1)),
        Body::Send {
            header: dodos_abi::Header::new(7, 1, 0, 0).unwrap(),
            stamp: (dodos_abi::TaskId::new(1, 5).raw(), 0),
            grants: vec![],
            words: vec![7],
        },
    );
    m.sys.inject(0, 1, forged);
    m.settle();
    assert_eq!(refused_count(&m, 1), before + 1);
    assert_eq!(m.woken(1), None);
}

#[test]
fn an_unreadable_header_is_consumed_as_one_entry() {
    let mut m = two();
    let header = RingHeader {
        kind: 200,
        length: 9,
        sender: 0,
        sender_generation: 1,
        receiver_generation: 1,
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    m.sys.inject_raw(0, 1, true, header, 1);
    m.settle();
    let consumed = m.sys.canonical().beacons[1].consumed[0].1;
    let produced = m.sys.canonical().rings[0][1].user.produced;
    assert_eq!(consumed, produced);
    // The ring goes on: a later send is delivered.
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    assert_eq!(m.call(0, send(1, vec![], SlotAddr::NONE)), WAITS);
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

#[test]
fn a_producer_index_too_far_ahead_is_read_as_a_failure_of_the_sender_and_resynchronised() {
    let mut m = two();
    let header = RingHeader {
        kind: dodos_abi::NodeMessage::Ping as u8,
        length: 0,
        sender: 0,
        sender_generation: 1,
        receiver_generation: 1,
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    m.sys
        .inject_raw(0, 1, false, header, 10 * dodos_abi::RING_KERNEL_ENTRIES);
    m.settle();
    let consumed = m.sys.canonical().beacons[1].consumed[0].0;
    let produced = m.sys.canonical().rings[0][1].kernel.produced;
    assert_eq!(consumed, produced);
}

// What ends with a generation, a fresh start and a resume.

#[test]
fn r0_ends_with_gone_the_wait_for_an_answer_of_a_node_accused_first() {
    let mut m = two();
    m.ok(0, super::place_first(0, 0));
    let killer = m.thread_on(0, 300, 0);
    m.sys.hang(1, true);
    assert_eq!(m.call(0, send(1, vec![], SlotAddr::NONE)), WAITS);
    assert!(matches!(
        m.sys.node(0).threads[&m.t[0]].wait,
        Wait::Node { peer: 1, .. }
    ));
    let stop = Syscall::NodeStop {
        node: node_cap(1),
        mode: NODE_STOP_KILL,
        core: 0,
    };
    assert_eq!(m.call_by(0, killer, stop), DONE);
    m.settle();
    assert_eq!(m.woken(0), Some(&Err(Error::Gone)));
    // R0 freed the credit toward node 1.
    assert_eq!(m.sys.node(0).peers[1].sends(), (0, 0));
}

#[test]
fn a_fresh_start_consumes_nothing_sent_to_its_earlier_generation() {
    let mut m = two();
    let stop = Syscall::NodeStop {
        node: node_cap(1),
        mode: NODE_STOP_KILL,
        core: 0,
    };
    m.ok(0, stop);
    m.run(10 * MS);
    // A raw entry left in the ring before the start.
    let header = RingHeader {
        kind: dodos_abi::NodeMessage::Ping as u8,
        length: 0,
        sender: 0,
        sender_generation: 1,
        receiver_generation: 1,
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    m.sys.inject_raw(0, 1, false, header, 1);
    m.ok(
        0,
        Syscall::NodeStart {
            node: node_cap(1),
            mode: NODE_START_FRESH,
            image: 0,
            module: 0,
            window: 0,
            mask: 0b10,
        },
    );
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    // The consumer index was set to the producer index before any thread
    // ran: nothing was refused, nothing consumed.
    assert_eq!(refused_count(&m, 1), 0);
    let consumed = m.sys.canonical().beacons[1].consumed[0].0;
    let produced = m.sys.canonical().rings[0][1].kernel.produced;
    assert_eq!(consumed, produced);
}

#[test]
fn a_resume_keeps_its_consumer_indices_and_consumes_the_answers_that_waited() {
    let mut m = two();
    m.ok(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_SEAL,
            core: 0,
        },
    );
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // A send to a sealed node is gone (nothing is sent to it); a message
    // written before the seal waits.
    m.no(0, send(1, vec![], SlotAddr::NONE), Error::Gone);
    let before = m.sys.canonical().beacons[1].consumed.clone();
    m.ok(
        0,
        Syscall::NodeStart {
            node: node_cap(1),
            mode: NODE_START_RESUME,
            image: 0,
            module: 0,
            window: 0,
            mask: 0,
        },
    );
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.canonical().beacons[1].consumed, before);
}

#[test]
fn the_kernel_ring_keeps_its_reserve_of_answers_free_of_requests() {
    let m = two();
    let model = m.sys.node(0);
    // A request of the kernel ring fits only beyond the reserve; an answer
    // always fits.
    let entries = dodos_abi::RING_KERNEL_ENTRIES;
    let reserve = dodos_abi::RING_ANSWER_RESERVE;
    assert!(model.ring_room(1, dodos_abi::NodeMessage::Revoke, entries - reserve));
    assert!(!model.ring_room(1, dodos_abi::NodeMessage::Revoke, entries - reserve + 1));
    assert!(model.ring_room(1, dodos_abi::NodeMessage::RevokeStep, entries));
}
