//! Memory and devices between nodes: the two nodes of `Machine::example`
//! as one `System`, whose messages between kernels arrive in the order each
//! test chooses. Every event is followed by every invariant of every node
//! and those between nodes.

use dodos_abi::ObjectKind::{Endpoint, Frame, Notify, Reply, Thread};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
/// The untyped of one chunk of each node's start-up space.
const UT: SlotAddr = SlotAddr::own(25);
const MAILBOX: SlotAddr = SlotAddr::own(START_SLOT_MAILBOX);
const ASPACE: SlotAddr = SlotAddr::own(START_SLOT_ASPACE);
/// The `core` capability of node `n`.
const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}
/// The slots the tests use, past the start-up capabilities.
const REPLY: u32 = 100;
const OBJ: u32 = 101;
const GOT: u32 = 110;
/// An address of the user range where the first process maps nothing.
const AT: u64 = 1 << 30;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn msg(label: u32, caps: Vec<Transfer>) -> Message {
    Message {
        label,
        flags: 0,
        words: vec![label.into()],
        caps,
        marks: 0,
    }
}

fn range(first: u32, count: u32) -> Range {
    Range {
        first: s(first),
        count,
    }
}

fn retype(kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

fn recv(window: Range) -> Syscall {
    Syscall::Recv {
        endpoint: MAILBOX,
        reply: s(REPLY),
        range: window,
        wait: 0,
    }
}

/// A receive on the mailbox with the reply object in `reply`, made first,
/// into one slot at `first`.
fn recv_into(reply: u32, first: u32) -> Syscall {
    Syscall::Recv {
        endpoint: MAILBOX,
        reply: s(reply),
        range: range(first, 1),
        wait: 0,
    }
}

fn map(frame: u32, count: u64, rights: Rights) -> Syscall {
    Syscall::ASpaceMap(Map {
        space: ASPACE,
        frame: s(frame),
        untyped: SlotAddr::NONE,
        address: AT,
        first: 0,
        count,
        rights: rights.bits(),
    })
}

/// The message in an outcome, as (label, capabilities, flags).
#[track_caller]
fn got(outcome: Option<&Outcome>) -> (u32, usize, u32) {
    let Some(Ok(Output::Message(delivery))) = outcome else {
        panic!("not a message: {outcome:?}");
    };
    let header = delivery.received.header;
    (header.label(), header.caps(), header.flags())
}

/// The machine, its first thread on each node, and the reply object of
/// each node in `REPLY`.
struct S {
    sys: System,
    t: Vec<ObjId>,
}

impl S {
    fn new() -> Self {
        let (sys, t) = System::boot(&Machine::example()).unwrap();
        let mut m = Self { sys, t };
        for node in 0..2 {
            m.ok(node, retype(Reply, 0, REPLY));
            // The first thread keeps half of its core, so that the threads
            // `spawn` makes have contexts of their own.
            m.ok(
                node,
                Syscall::SchedConfigure {
                    sched: SlotAddr::own(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 5_000_000,
                    period: 10_000_000,
                    class: 1,
                },
            );
        }
        m
    }

    #[track_caller]
    fn on(&mut self, node: u16, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        assert_eq!(self.on(node, call), DONE);
    }

    #[track_caller]
    fn deliver(&mut self, index: usize) {
        self.sys.deliver(index);
        assert_eq!(self.sys.check(), Ok(()));
    }

    /// Delivers the messages in flight, oldest first, until none is.
    #[track_caller]
    fn settle(&mut self) {
        for _ in 0..64 {
            if self.sys.pending().is_empty() {
                return;
            }
            self.deliver(0);
        }
        panic!("messages without end");
    }

    #[track_caller]
    fn deliver_where(&mut self, wanted: impl Fn(&Body) -> bool) {
        let index = self.sys.find(wanted).expect("a message in flight");
        self.deliver(index);
    }

    /// A second thread on `node`, in `slot`, in the spaces of the first,
    /// with a message page and a context of its own on the node's core.
    fn spawn(&mut self, node: u16, slot: u32) -> ObjId {
        let spare = 4000 - 2 * (slot - 200);
        self.ok(node, retype(Thread, 0, slot));
        self.ok(node, retype(Frame, 1, spare));
        self.ok(
            node,
            Syscall::ThreadConfigure {
                thread: s(slot),
                cspace: SlotAddr::own(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(spare),
            },
        );
        self.ok(node, retype(ObjectKind::Sched, 0, spare - 1));
        self.ok(
            node,
            Syscall::SchedConfigure {
                sched: s(spare - 1),
                core: core(node.into()),
                budget: 10_000,
                period: 10_000_000,
                class: 1,
            },
        );
        self.ok(
            node,
            Syscall::SchedBind {
                sched: s(spare - 1),
                thread: s(slot),
            },
        );
        self.ok(node, Syscall::ThreadResume(s(slot)));
        self.sys
            .node(node)
            .object_at(self.t[node as usize], s(slot))
            .unwrap()
    }

    #[track_caller]
    fn by(&mut self, node: u16, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, thread, call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    /// Node 0 makes an object of `kind` in `OBJ` and gives node 1 a
    /// derived capability for it, in `GOT`.
    fn granted(&mut self, kind: ObjectKind) {
        let size = if kind == Frame { 1 } else { 0 };
        self.ok(0, retype(kind, size, OBJ));
        let all = Rights::ALL.without(Rights::EXEC).bits();
        assert_eq!(self.transfer(vec![Transfer::Derive(s(OBJ), all)]).1, 1);
    }

    fn woken(&self, node: u16) -> Option<&Outcome> {
        self.sys.node(node).woken(self.t[node as usize])
    }

    /// Node 1 waits on its mailbox; node 0 sends it a message that lists
    /// `caps`; every message settles. Returns node 1's delivery.
    fn transfer(&mut self, caps: Vec<Transfer>) -> (u32, usize, u32) {
        assert_eq!(self.on(1, recv(range(GOT, 4))), WAITS);
        let send = Syscall::Send {
            target: core(1),
            message: msg(7, caps),
        };
        assert_eq!(self.on(0, send), WAITS);
        self.settle();
        got(self.woken(1))
    }
}

// The mailbox of a node.

#[test]
fn send_on_a_core_capability_reaches_the_mailbox_of_that_node() {
    let mut m = S::new();
    assert_eq!(m.on(1, recv(Range::NONE)), WAITS);
    let send = Syscall::Send {
        target: core(1),
        message: msg(5, vec![]),
    };
    // The sender waits for the other node until its request is answered.
    assert_eq!(m.on(0, send), WAITS);
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            from: 0,
            to: 1,
            body: Body::Request { .. },
            ..
        }]
    ));
    m.deliver(0);
    assert_eq!(got(m.woken(1)), (5, 0, 0));
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            from: 1,
            to: 0,
            body: Body::Answer { .. },
            ..
        }]
    ));
    m.deliver(0);
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

#[test]
fn call_on_a_core_capability_waits_for_the_reply_of_the_other_node() {
    let mut m = S::new();
    assert_eq!(m.on(1, recv(Range::NONE)), WAITS);
    let call = Syscall::Call {
        endpoint: core(1),
        message: msg(5, vec![]),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.on(0, call), WAITS);
    m.settle();
    assert_eq!(got(m.woken(1)), (5, 0, 0));
    // The reply goes back through the proxy that made the call.
    let reply = Syscall::ReplyRecv {
        reply: s(REPLY),
        message: msg(6, vec![]),
        endpoint: MAILBOX,
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.on(1, reply), WAITS);
    m.settle();
    assert_eq!(got(m.woken(0)), (6, 0, 0));
}

#[test]
fn send_on_the_own_core_capability_reaches_the_own_mailbox_at_once() {
    let mut m = S::new();
    let send = Syscall::Send {
        target: core(0),
        message: msg(5, vec![]),
    };
    // No receiver: the mailbox is an endpoint, and the send fails as it
    // would there, with no message between kernels.
    assert_eq!(m.on(0, send), Err(Error::WouldWait));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn mailbox_needs_send_and_grant_for_capabilities() {
    let mut m = S::new();
    m.ok(
        0,
        Syscall::CapDerive {
            source: core(1),
            destination: s(OBJ),
            rights: Rights::SEND.bits(),
            badge: 0,
        },
    );
    m.ok(0, retype(Notify, 0, OBJ + 1));
    let listed = Syscall::Send {
        target: s(OBJ),
        message: msg(5, vec![Transfer::Move(s(OBJ + 1))]),
    };
    assert_eq!(m.on(0, listed), Err(Error::Denied));
    m.ok(
        0,
        Syscall::CapDerive {
            source: core(1),
            destination: s(OBJ + 2),
            rights: Rights::READ.bits(),
            badge: 0,
        },
    );
    let plain = Syscall::Send {
        target: s(OBJ + 2),
        message: msg(5, vec![]),
    };
    assert_eq!(m.on(0, plain), Err(Error::Denied));
    assert!(m.sys.pending().is_empty());
}

// Grants.

#[test]
fn a_derived_capability_crosses_by_a_grant_whose_anchor_is_its_child() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    assert_eq!(m.transfer(vec![derive]), (7, 1, 0));
    // The source stays; the holder has a remote capability in its range.
    let source = m.sys.node(0).object_at(m.t[0], s(OBJ));
    assert!(source.is_some());
    assert!(m.sys.node(1).object_at(m.t[1], s(GOT)).is_some());
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

#[test]
fn a_moved_capability_leaves_its_slot_for_the_anchor() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    assert_eq!(m.transfer(vec![Transfer::Move(s(OBJ))]), (7, 1, 0));
    assert_eq!(m.sys.node(0).object_at(m.t[0], s(OBJ)), None);
}

#[test]
fn a_transfer_with_no_room_on_the_receiver_drops_the_capabilities_and_frees_the_grants() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    // The receiver's range is full when the grants arrive.
    m.ok(1, retype(Notify, 0, GOT));
    let dropped = Header::FLAG_CAPS_DROPPED;
    assert_eq!(m.transfer(vec![Transfer::Move(s(OBJ))]), (7, 0, dropped));
    // The moved capability is back in its slot, and no grant remains.
    assert!(m.sys.node(0).object_at(m.t[0], s(OBJ)).is_some());
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: dropped })));
}

// Calls on a remote object.

#[test]
fn signal_on_a_remote_notify_is_made_by_a_proxy_of_the_granting_node() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), WAITS);
    m.settle();
    assert_eq!(m.woken(1), Some(&DONE));
    // The signal is pending on node 0's object.
    assert!(matches!(
        m.on(0, Syscall::Poll(s(OBJ))),
        Ok(Output::Word(1))
    ));
}

#[test]
fn a_wait_on_a_remote_notify_is_held_by_the_proxy_until_a_signal() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    assert_eq!(m.on(1, Syscall::Wait(s(GOT), 0)), WAITS);
    m.settle();
    assert_eq!(m.woken(1), None);
    m.ok(0, Syscall::Signal(s(OBJ)));
    m.settle();
    assert!(matches!(m.woken(1), Some(Ok(Output::Word(1)))));
}

#[test]
fn thread_cancel_of_a_thread_waiting_on_a_remote_object_ends_the_proxys_wait() {
    let mut m = S::new();
    m.granted(Notify);
    let a = m.spawn(1, 200);
    assert_eq!(m.by(1, a, Syscall::Wait(s(GOT), 0)), WAITS);
    m.settle();
    // The cancel succeeds at once and is sent to node 0, whose proxy's
    // wait ends with `cancelled`, which comes back as the call's result.
    m.ok(1, Syscall::ThreadCancel(s(200), 0));
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            body: Body::Cancel { .. },
            ..
        }]
    ));
    m.settle();
    assert_eq!(m.sys.node(1).woken(a), Some(&Err(Error::Cancelled)));
    // The proxy is free: a signal stays pending on node 0's object.
    m.ok(0, Syscall::Signal(s(OBJ)));
    assert!(matches!(
        m.on(0, Syscall::Poll(s(OBJ))),
        Ok(Output::Word(1))
    ));
}

#[test]
fn thread_cancel_does_not_end_a_wait_for_another_node_on_no_object() {
    let mut m = S::new();
    m.granted(Notify);
    let a = m.spawn(1, 200);
    assert_eq!(m.by(1, a, Syscall::Signal(s(GOT))), WAITS);
    // The signal is not a wait on an object: the cancel ends nothing and
    // sends nothing; the request goes on.
    let before = m.sys.pending().len();
    m.ok(1, Syscall::ThreadCancel(s(200), 0));
    assert_eq!(m.sys.pending().len(), before);
    m.settle();
    assert_eq!(m.sys.node(1).woken(a), Some(&DONE));
}

#[test]
fn destroying_a_thread_whose_wait_a_proxy_holds_ends_that_wait_on_the_other_node() {
    let mut m = S::new();
    m.granted(Notify);
    let a = m.spawn(1, 200);
    assert_eq!(m.by(1, a, Syscall::Wait(s(GOT), 0)), WAITS);
    m.settle();
    m.ok(1, Syscall::CapDelete(s(200)));
    assert!(m.sys.find(|b| matches!(b, Body::Cancel { .. })).is_some());
    // The answer finds no thread and is dropped; the proxy is free again.
    m.settle();
    m.ok(0, Syscall::Signal(s(OBJ)));
    assert!(matches!(
        m.on(0, Syscall::Poll(s(OBJ))),
        Ok(Output::Word(1))
    ));
}

#[test]
fn sixteen_proxies_per_peer_and_further_requests_wait_first_in_first_out() {
    let mut m = S::new();
    m.granted(Notify);
    let threads: Vec<ObjId> = (0..PROXIES_PER_PEER as u32 + 2)
        .map(|i| m.spawn(1, 200 + i))
        .collect();
    for &t in &threads {
        assert_eq!(m.by(1, t, Syscall::Wait(s(GOT), 0)), WAITS);
    }
    // Sixteen requests are in flight; the seventeenth and eighteenth wait
    // on node 1.
    assert_eq!(m.sys.pending().len(), PROXIES_PER_PEER);
    m.settle();
    // One signal wakes the first proxy; its answer frees a proxy and the
    // first held request goes out, the seventeenth.
    m.ok(0, Syscall::Signal(s(OBJ)));
    m.deliver_where(|b| matches!(b, Body::Answer { .. }));
    assert!(matches!(
        m.sys.node(1).woken(threads[0]),
        Some(Ok(Output::Word(1)))
    ));
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            body: Body::Request { .. },
            ..
        }]
    ));
    m.settle();
    for (i, &t) in threads.iter().enumerate().skip(1) {
        assert_eq!(m.sys.node(1).woken(t), None, "thread {i}");
    }
    // Every later signal wakes the next in order, the held ones last, in
    // the order they were made.
    for &t in &threads[1..] {
        m.ok(0, Syscall::Signal(s(OBJ)));
        m.settle();
        assert!(matches!(m.sys.node(1).woken(t), Some(Ok(Output::Word(1)))));
    }
}

#[test]
fn recv_on_a_remote_endpoint_needs_a_reply_object_of_that_node() {
    let mut m = S::new();
    m.ok(0, retype(Endpoint, 0, OBJ));
    m.ok(0, retype(Reply, 0, OBJ + 1));
    let all = Rights::ALL.without(Rights::EXEC).bits();
    let caps = vec![
        Transfer::Derive(s(OBJ), all),
        Transfer::Derive(s(OBJ + 1), all),
    ];
    assert_eq!(m.transfer(caps).1, 2);
    let a = m.spawn(1, 200);
    let mine = Syscall::Recv {
        endpoint: s(GOT),
        reply: s(REPLY),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.by(1, a, mine), Err(Error::Invalid));
    assert!(m.sys.pending().is_empty());
    let theirs = Syscall::Recv {
        endpoint: s(GOT),
        reply: s(GOT + 1),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.by(1, a, theirs), WAITS);
    m.settle();
    // A caller on node 0 reaches the proxy, which receives for thread a.
    let b = m.spawn(0, 200);
    let call = Syscall::Call {
        endpoint: s(OBJ),
        message: msg(9, vec![]),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.by(0, b, call), WAITS);
    m.settle();
    assert_eq!(got(m.sys.node(1).woken(a)), (9, 0, 0));
}

#[test]
fn cap_inspect_of_a_remote_capability_reads_the_slot_without_a_message() {
    let mut m = S::new();
    m.granted(Notify);
    let inspected = m.on(1, Syscall::CapInspect(s(GOT)));
    assert!(matches!(
        inspected,
        Ok(Output::Inspect { kind: Notify, .. })
    ));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn a_capability_argument_of_the_callers_own_node_is_invalid() {
    let mut m = S::new();
    m.granted(Notify);
    m.ok(1, retype(Endpoint, 0, OBJ));
    let bind = Syscall::NotifyBind {
        notify: s(GOT),
        endpoint: s(OBJ),
    };
    assert_eq!(m.on(1, bind), Err(Error::Invalid));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn a_remote_capability_derived_on_its_holder_is_under_the_same_grant() {
    let mut m = S::new();
    m.granted(Notify);
    m.ok(
        1,
        Syscall::CapDerive {
            source: s(GOT),
            destination: s(OBJ),
            rights: Rights::SEND.bits(),
            badge: 0,
        },
    );
    assert_eq!(m.on(1, Syscall::Signal(s(OBJ))), WAITS);
    m.settle();
    // Revoking on node 0 makes both invalid.
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert_eq!(m.on(1, Syscall::Signal(s(OBJ))), Err(Error::Gone));
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Gone));
}

#[test]
fn a_use_of_a_revoked_remote_capability_is_gone_and_its_slot_dead() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    // No mapping under the grant: the revocation frees it at once.
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Gone));
    // The slot is dead: a second use is `gone` again. The removed remote
    // object releases a grant that is already freed, which node 0 ignores.
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Gone));
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            body: Body::Release { .. },
            ..
        }]
    ));
    m.settle();
}

// Memory under a grant.

#[test]
fn a_granted_frame_maps_on_the_holder_and_counts_in_its_use_entry() {
    let mut m = S::new();
    m.ok(0, retype(Frame, 1, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    m.ok(1, map(GOT, 1, Rights::READ.union(Rights::WRITE)));
    assert!(m.sys.node(1).translated(m.t[1], AT).is_some());
}

#[test]
fn revoking_a_mapped_grant_waits_for_the_holder_then_returns_again() {
    let mut m = S::new();
    m.ok(0, retype(Frame, 1, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    m.ok(1, map(GOT, 1, Rights::READ.union(Rights::WRITE)));
    assert_eq!(m.on(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    m.deliver_where(|b| matches!(b, Body::Revoke { .. }));
    // The holder removed its root remote capability and its mapping.
    assert_eq!(m.sys.node(1).translated(m.t[1], AT), None);
    m.deliver_where(|b| matches!(b, Body::RevokeStep { done: true, .. }));
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Invalid));
}

#[test]
fn release_by_the_holder_frees_the_grant_and_its_anchor() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let derive = Transfer::Derive(s(OBJ), Rights::ALL.without(Rights::EXEC).bits());
    m.transfer(vec![derive]);
    m.ok(1, Syscall::CapDelete(s(GOT)));
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            body: Body::Release { .. },
            ..
        }]
    ));
    m.settle();
    // A second transfer takes the same grant again, with the next serial.
    m.transfer(vec![derive]);
}

// A remote capability going back to its node.

#[test]
fn a_remote_capability_derived_back_to_its_node_is_an_ordinary_child_of_the_anchor() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    let all = Rights::ALL.without(Rights::EXEC).bits();
    m.transfer(vec![Transfer::Derive(s(OBJ), all)]);
    // Node 0 waits on its own mailbox; node 1 sends the remote capability
    // back, derived with fewer rights.
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(
            8,
            vec![Transfer::Derive(
                s(GOT),
                Rights::SEND.union(Rights::READ).bits(),
            )],
        ),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.settle();
    assert_eq!(got(m.woken(0)), (8, 1, 0));
    // The same object as the source, with no grant: a signal through it
    // is local, and node 1 keeps its remote capability.
    assert_eq!(m.on(0, Syscall::Signal(s(GOT))), DONE);
    assert!(m.sys.pending().is_empty());
    assert!(matches!(
        m.on(0, Syscall::Poll(s(OBJ))),
        Ok(Output::Word(1))
    ));
    assert!(m.sys.node(1).object_at(m.t[1], s(GOT)).is_some());
    // Revoking the source removes the child and the anchor.
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert_eq!(m.sys.node(0).object_at(m.t[0], s(GOT)), None);
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Gone));
}

#[test]
fn a_remote_capability_moved_back_to_its_node_takes_the_anchors_place_and_frees_the_grant() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    m.transfer(vec![Transfer::Move(s(OBJ))]);
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(8, vec![Transfer::Move(s(GOT))]),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.deliver_where(|b| matches!(b, Body::Request { .. }));
    m.deliver_where(|b| matches!(b, Body::TransferAsk { .. }));
    m.deliver_where(|b| matches!(b, Body::TransferGrant { .. }));
    assert_eq!(got(m.woken(0)), (8, 1, 0));
    // The root capability is in the range again; node 1 removes its remote
    // capability when it learns it, and its release is ignored.
    assert!(m.sys.find(|b| matches!(b, Body::Returned { .. })).is_some());
    m.settle();
    assert_eq!(m.sys.node(1).object_at(m.t[1], s(GOT)), None);
    assert_eq!(m.on(0, Syscall::Signal(s(GOT))), DONE);
    // The grant is free again: a new transfer takes it.
    m.transfer(vec![Transfer::Move(s(GOT))]);
}

#[test]
fn a_move_back_of_a_remote_capability_with_a_descendant_drops_the_capabilities() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, OBJ));
    m.transfer(vec![Transfer::Move(s(OBJ))]);
    // A child of the remote capability on node 1, under the same grant.
    m.ok(
        1,
        Syscall::CapDerive {
            source: s(GOT),
            destination: s(OBJ),
            rights: Rights::SEND.bits(),
            badge: 0,
        },
    );
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(8, vec![Transfer::Move(s(GOT))]),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.settle();
    assert_eq!(got(m.woken(0)), (8, 0, Header::FLAG_CAPS_DROPPED));
    // The child signals node 0's object through the same grant.
    assert_eq!(m.on(1, Syscall::Signal(s(OBJ))), WAITS);
    m.settle();
    assert_eq!(m.woken(1), Some(&DONE));
}

#[test]
fn a_release_leaves_an_anchor_with_a_descendant_held_by_nobody() {
    let mut m = S::new();
    m.granted(Notify);
    // Node 1 sends the remote capability back derived: a child of the
    // anchor on node 0.
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(8, vec![Transfer::Derive(s(GOT), Rights::SEND.bits())]),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.settle();
    m.ok(1, Syscall::CapDelete(s(GOT)));
    m.settle();
    // The anchor stays with its grant: the next transfer takes grant 1.
    assert_eq!(m.on(1, recv(range(GOT, 4))), WAITS);
    let send = Syscall::Send {
        target: core(1),
        message: msg(7, vec![Transfer::Derive(s(OBJ), Rights::SEND.bits())]),
    };
    assert_eq!(m.on(0, send), WAITS);
    m.deliver_where(|b| matches!(b, Body::Request { .. }));
    m.deliver_where(|b| matches!(b, Body::TransferAsk { .. }));
    let granted = m.sys.pending().iter().find_map(|p| match &p.body {
        Body::TransferGrant {
            grants: Some(grants),
            ..
        } => Some(grants.clone()),
        _ => None,
    });
    assert!(matches!(
        granted.as_deref(),
        Some([Crossing::Grant(Granted { grant: 1, .. })])
    ));
    // The child still signals node 0's object.
    m.settle();
    assert_eq!(m.on(0, Syscall::Signal(s(GOT))), DONE);
}

#[test]
fn removing_a_mapped_remote_frame_on_the_holder_unmaps_it_and_releases_the_grant() {
    let mut m = S::new();
    m.granted(Frame);
    m.ok(1, map(GOT, 1, Rights::READ));
    m.ok(1, Syscall::CapDelete(s(GOT)));
    assert_eq!(m.sys.node(1).translated(m.t[1], AT), None);
    assert!(matches!(
        m.sys.pending(),
        [Packet {
            body: Body::Release {
                serial: 0,
                grant: 0
            },
            ..
        }]
    ));
    m.settle();
    // The grant is free: revoking the source needs no message.
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn mapping_an_invalid_remote_frame_is_gone_and_its_slot_dead() {
    let mut m = S::new();
    m.granted(Frame);
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert_eq!(m.on(1, map(GOT, 1, Rights::READ)), Err(Error::Gone));
    assert_eq!(m.on(1, Syscall::CapInspect(s(GOT))), Err(Error::Gone));
}

#[test]
fn a_move_of_a_root_capability_leaves_node_0_with_only_the_anchor() {
    let mut m = S::new();
    m.ok(0, retype(Frame, 1, OBJ));
    m.transfer(vec![Transfer::Move(s(OBJ))]);
    // The root is the anchor: node 1 maps it, and holds it as the root.
    m.ok(1, map(GOT, 1, Rights::READ.union(Rights::WRITE)));
    assert!(m.sys.node(1).translated(m.t[1], AT).is_some());
}

#[test]
fn thread_cancel_of_a_request_waiting_for_a_free_proxy_takes_it_out_of_the_list() {
    let mut m = S::new();
    m.granted(Notify);
    let threads: Vec<ObjId> = (0..PROXIES_PER_PEER as u32 + 2)
        .map(|i| m.spawn(1, 200 + i))
        .collect();
    for &t in &threads {
        assert_eq!(m.by(1, t, Syscall::Wait(s(GOT), 0)), WAITS);
    }
    m.settle();
    // The seventeenth is cancelled at once, with nothing sent; destroying
    // the eighteenth takes it out too.
    m.ok(
        1,
        Syscall::ThreadCancel(s(200 + PROXIES_PER_PEER as u32), 0),
    );
    assert_eq!(
        m.sys.node(1).woken(threads[PROXIES_PER_PEER]),
        Some(&Err(Error::Cancelled))
    );
    m.ok(1, Syscall::CapDelete(s(201 + PROXIES_PER_PEER as u32)));
    assert!(m.sys.pending().is_empty());
    // A freed proxy sends no held request any more.
    m.ok(0, Syscall::Signal(s(OBJ)));
    m.settle();
    assert!(m.sys.pending().is_empty());
    assert!(matches!(
        m.sys.node(1).woken(threads[0]),
        Some(Ok(Output::Word(1)))
    ));
    // The other fifteen proxies wake their threads; no proxy waits for the
    // two that left, so a further signal stays pending.
    for &t in &threads[1..PROXIES_PER_PEER] {
        m.ok(0, Syscall::Signal(s(OBJ)));
        m.settle();
        assert!(matches!(m.sys.node(1).woken(t), Some(Ok(Output::Word(1)))));
    }
    m.ok(0, Syscall::Signal(s(OBJ)));
    assert!(matches!(
        m.on(0, Syscall::Poll(s(OBJ))),
        Ok(Output::Word(1))
    ));
}

#[test]
fn a_revoking_grant_makes_its_remote_capabilities_invalid_at_once() {
    let mut m = S::new();
    m.granted(Frame);
    m.ok(1, map(GOT, 1, Rights::READ));
    assert_eq!(m.on(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    // Before the holder hears of it, a use of the remote capability is
    // `gone`, and its slot dead; the mapping goes with it.
    assert_eq!(m.on(1, Syscall::CapInspect(s(GOT))), Err(Error::Gone));
    assert_eq!(m.sys.node(1).translated(m.t[1], AT), None);
    m.settle();
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn derive_and_move_from_an_invalid_remote_capability_are_gone() {
    for moved in [false, true] {
        let mut m = S::new();
        m.granted(Notify);
        m.ok(0, Syscall::CapRevoke(s(OBJ)));
        let call = match moved {
            false => Syscall::CapDerive {
                source: s(GOT),
                destination: s(OBJ),
                rights: Rights::SEND.bits(),
                badge: 0,
            },
            true => Syscall::CapMove {
                source: s(GOT),
                destination: s(OBJ),
            },
        };
        assert_eq!(m.on(1, call), Err(Error::Gone));
        // The slot is dead: `cap_delete` empties it.
        assert_eq!(m.on(1, Syscall::CapInspect(s(GOT))), Err(Error::Gone));
        m.ok(1, Syscall::CapDelete(s(GOT)));
        assert_eq!(m.on(1, Syscall::CapInspect(s(GOT))), Err(Error::Invalid));
    }
}

#[test]
fn a_reused_grant_has_the_next_serial_so_an_old_remote_capability_stays_invalid() {
    let mut m = S::new();
    m.granted(Notify);
    // The grant is freed; the remote capability in `GOT` is invalid but
    // its slot is not yet dead.
    m.ok(0, Syscall::CapRevoke(s(OBJ)));
    // A new grant takes entry 0 again, into the next slots of node 1.
    assert_eq!(m.on(1, recv(range(GOT + 1, 4))), WAITS);
    let send = Syscall::Send {
        target: core(1),
        message: msg(7, vec![Transfer::Derive(s(OBJ), Rights::SEND.bits())]),
    };
    assert_eq!(m.on(0, send), WAITS);
    m.settle();
    assert_eq!(got(m.woken(1)), (7, 1, 0));
    // The old capability names serial 0, the entry serial 1.
    assert_eq!(m.on(1, Syscall::Signal(s(GOT))), Err(Error::Gone));
    assert_eq!(m.on(1, Syscall::Signal(s(GOT + 1))), WAITS);
    m.settle();
    assert_eq!(m.woken(1), Some(&DONE));
}

#[test]
fn a_cspace_root_moved_to_a_range_in_a_space_as_deep_is_dropped() {
    let mut m = S::new();
    // Node 0 moves the root of a cspace made from its untyped (depth 2);
    // node 1 receives in a cspace made the same way, whose root is as deep.
    m.ok(0, retype(ObjectKind::CSpace, 128, OBJ));
    m.ok(1, retype(ObjectKind::CSpace, 128, OBJ));
    let grown = m.on(
        1,
        Syscall::CSpaceGrow {
            cspace: s(OBJ),
            untyped: UT,
        },
    );
    assert_eq!(grown, Ok(Output::Slots(128)));
    let window = Range {
        first: SlotAddr::through(OBJ, 8),
        count: 1,
    };
    assert_eq!(m.on(1, recv(window)), WAITS);
    let send = Syscall::Send {
        target: core(1),
        message: msg(7, vec![Transfer::Move(s(OBJ))]),
    };
    assert_eq!(m.on(0, send), WAITS);
    m.settle();
    let dropped = Header::FLAG_CAPS_DROPPED;
    assert_eq!(got(m.woken(1)), (7, 0, dropped));
    // The root is back in its slot on node 0. Into the start-up space,
    // whose root is at depth 1, the same move is accepted.
    assert!(m.sys.node(0).object_at(m.t[0], s(OBJ)).is_some());
    assert_eq!(m.transfer(vec![Transfer::Move(s(OBJ))]), (7, 1, 0));
}

#[test]
fn a_revocation_whose_holder_needs_several_steps_returns_again_after_each() {
    let mut m = S::new();
    m.granted(Frame);
    // Forty copies of the remote frame on node 1, each mapped: removing
    // the root remote capability takes more than one step of 32 units.
    let page = m.sys.node(1).geometry().page();
    for i in 0..40u32 {
        m.ok(
            1,
            Syscall::CapDerive {
                source: s(GOT),
                destination: s(300 + i),
                rights: Rights::READ.bits(),
                badge: 0,
            },
        );
        let at = Syscall::ASpaceMap(Map {
            space: ASPACE,
            frame: s(300 + i),
            untyped: SlotAddr::NONE,
            address: AT + u64::from(i) * page,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        });
        m.ok(1, at);
    }
    assert_eq!(m.on(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    m.deliver_where(|b| matches!(b, Body::Revoke { .. }));
    // The first step does not finish: the grant stays, revoking.
    m.deliver_where(|b| matches!(b, Body::RevokeStep { done: false, .. }));
    assert_eq!(m.woken(0), Some(&Err(Error::Again)));
    assert!(
        m.sys.node(1).translated(m.t[1], AT + 39 * page).is_some()
            || m.sys.node(1).translated(m.t[1], AT).is_some()
    );
    let mut rounds = 0;
    while m.on(0, Syscall::CapRevoke(s(OBJ))) == WAITS {
        m.settle();
        assert_eq!(m.woken(0), Some(&Err(Error::Again)));
        rounds += 1;
        assert!(rounds < 8);
    }
    assert!(rounds >= 1);
    for i in 0..40u64 {
        assert_eq!(m.sys.node(1).translated(m.t[1], AT + i * page), None);
    }
}

#[test]
fn a_stale_remote_capability_sent_home_is_dropped() {
    let mut m = S::new();
    m.granted(Notify);
    let b = m.spawn(0, 200);
    let c = m.spawn(1, 200);
    m.ok(1, retype(Reply, 0, REPLY + 1));
    // Node 1 sends its remote capability home, moved; node 0 receives.
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(8, vec![Transfer::Move(s(GOT))]),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.deliver_where(|b| matches!(b, Body::Request { .. }));
    m.deliver_where(|b| matches!(b, Body::TransferAsk { .. }));
    // Before the crossing arrives, node 0 revokes the grant and grants the
    // object again: the entry is live, with the next serial.
    assert_eq!(m.by(0, b, Syscall::CapRevoke(s(OBJ))), DONE);
    assert_eq!(m.by(1, c, recv_into(REPLY + 1, GOT + 4)), WAITS);
    let send = Syscall::Send {
        target: core(1),
        message: msg(9, vec![Transfer::Derive(s(OBJ), Rights::SEND.bits())]),
    };
    assert_eq!(m.by(0, b, send), WAITS);
    for _ in 0..4 {
        m.deliver_where(|body| !matches!(body, Body::TransferGrant { grants: Some(g), .. } if matches!(g[..], [Crossing::Home { .. }])));
    }
    assert_eq!(got(m.sys.node(1).woken(c)), (9, 1, 0));
    m.settle();
    // The crossing of serial 0 is refused.
    assert_eq!(got(m.woken(0)), (8, 0, Header::FLAG_CAPS_DROPPED));
}

#[test]
fn a_task_made_on_node_1_carries_node_1_in_its_identifier() {
    let mut m = S::new();
    m.ok(1, retype(ObjectKind::Task, 0, OBJ));
    let Ok(Output::Task { id, .. }) = m.on(1, Syscall::TaskRead(s(OBJ))) else {
        panic!("a task");
    };
    assert_eq!(TaskId::from_raw(id).node(), 1);
}

#[test]
fn a_remote_capability_derived_home_while_its_grant_is_revoking_is_dropped() {
    let mut m = S::new();
    m.granted(Frame);
    m.ok(1, map(GOT, 1, Rights::READ));
    let b = m.spawn(0, 200);
    // Node 1 sends a copy of its mapped remote frame home.
    assert_eq!(m.on(0, recv(range(GOT, 4))), WAITS);
    let back = Syscall::Send {
        target: core(0),
        message: msg(8, vec![Transfer::Derive(s(GOT), Rights::READ.bits())]),
    };
    assert_eq!(m.on(1, back), WAITS);
    m.deliver_where(|b| matches!(b, Body::Request { .. }));
    m.deliver_where(|b| matches!(b, Body::TransferAsk { .. }));
    // Node 0 revokes before the crossing arrives: the grant is revoking,
    // with the same serial, while the holder still maps it.
    assert_eq!(m.by(0, b, Syscall::CapRevoke(s(OBJ))), WAITS);
    m.deliver_where(|b| matches!(b, Body::TransferGrant { .. }));
    assert_eq!(got(m.woken(0)), (8, 0, Header::FLAG_CAPS_DROPPED));
    m.settle();
}
