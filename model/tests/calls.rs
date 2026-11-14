//! One test per behaviour of the call reference. Every call goes through
//! `K::on`, which checks the invariants after it.

use dodos_abi::ObjectKind::{ASpace, CSpace, Endpoint, Frame, Notify, Reply, Thread, Untyped};
use dodos_abi::*;
use dodos_model::*;

const SPACE: SlotAddr = SlotAddr::own(START_SLOT_CSPACE);
const UT: SlotAddr = SlotAddr::own(START_SLOT_FIRST);
const DEV: SlotAddr = SlotAddr::own(START_SLOT_FIRST + 1);
const DROPPED: u32 = Header::FLAG_CAPS_DROPPED;
// Slots that the tests use for the same thing.
const EP: u32 = 20;
const RP: u32 = 21;
const NT: u32 = 22;

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn via(cspace: u32, index: u32) -> SlotAddr {
    SlotAddr::through(cspace, index)
}

fn retype(untyped: SlotAddr, kind: ObjectKind, size: u64, destination: SlotAddr) -> Syscall {
    Syscall::UntypedRetype {
        untyped,
        kind,
        size,
        page: PageSize::Size4K,
        destination,
    }
}

fn derive(source: SlotAddr, destination: SlotAddr, rights: Rights, badge: u64) -> Syscall {
    Syscall::CapDerive {
        source,
        destination,
        rights,
        badge,
    }
}

fn mv(source: SlotAddr, destination: SlotAddr) -> Syscall {
    Syscall::CapMove {
        source,
        destination,
    }
}

fn grow(cspace: u32) -> Syscall {
    grow_from(cspace, UT)
}

fn grow_from(cspace: u32, untyped: SlotAddr) -> Syscall {
    Syscall::CSpaceGrow {
        cspace: s(cspace),
        untyped,
    }
}

fn configure(thread: u32, cspace: SlotAddr, aspace: SlotAddr, fault: SlotAddr) -> Syscall {
    Syscall::ThreadConfigure {
        thread: s(thread),
        cspace,
        aspace,
        fault,
    }
}

/// A message whose only word is its label.
fn msg(label: u32, caps: Vec<Transfer>) -> Message {
    Message {
        label,
        words: vec![label.into()],
        caps,
    }
}

fn range(first: u32, count: u32) -> Range {
    Range {
        first: s(first),
        count,
    }
}

fn call(endpoint: u32, message: Message, range: Range) -> Syscall {
    Syscall::Call {
        endpoint: s(endpoint),
        message,
        range,
    }
}

fn recv(endpoint: u32, reply: u32, range: Range) -> Syscall {
    Syscall::Recv {
        endpoint: s(endpoint),
        reply: s(reply),
        range,
    }
}

fn reply_recv(reply: u32, message: Message, endpoint: u32) -> Syscall {
    Syscall::ReplyRecv {
        reply: s(reply),
        message,
        endpoint: s(endpoint),
        range: Range::NONE,
    }
}

fn send(target: u32, message: Message) -> Syscall {
    Syscall::Send {
        target: s(target),
        message,
    }
}

fn bind(notify: u32, endpoint: SlotAddr) -> Syscall {
    Syscall::NotifyBind {
        notify: s(notify),
        endpoint,
    }
}

/// The message in an outcome, as (label, badge, capabilities, flags).
fn got(outcome: Outcome) -> (u32, u64, usize, u32) {
    let Ok(Output::Message(delivery)) = outcome else {
        panic!("not a message: {outcome:?}");
    };
    let header = delivery.received.header;
    assert_eq!(
        delivery.words,
        [u64::from(header.label())][..header.words()]
    );
    (
        header.label(),
        delivery.received.badge,
        header.caps(),
        header.flags(),
    )
}

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);

/// The model, and its first thread.
struct K {
    m: Model,
    t: ObjId,
}

impl K {
    fn new() -> Self {
        let (m, t) = Model::boot();
        Self { m, t }
    }

    fn on(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.m.apply(thread, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    fn go(&mut self, call: Syscall) -> Outcome {
        self.on(self.t, call)
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        assert_eq!(self.go(call), DONE);
    }

    /// The first thread makes a call that fails.
    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    /// A new object from the ordinary untyped, in a slot of the first thread.
    fn make(&mut self, kind: ObjectKind, slot: u32) {
        let size = match kind {
            Untyped => 4096,
            CSpace => 128,
            Frame => 1,
            _ => 0,
        };
        self.ok(retype(UT, kind, size, s(slot)));
    }

    /// An endpoint, a reply object and a notify in `EP`, `RP` and `NT`.
    fn with_ipc() -> Self {
        let mut k = Self::new();
        k.make(Endpoint, EP);
        k.make(Reply, RP);
        k.make(Notify, NT);
        k
    }

    /// A new thread that runs in the capability space of the first thread.
    fn spawn(&mut self, slot: u32) -> ObjId {
        self.make(Thread, slot);
        self.ok(configure(slot, SPACE, SlotAddr::NONE, SlotAddr::NONE));
        self.ok(Syscall::ThreadResume(s(slot)));
        self.m.object_at(self.t, s(slot)).unwrap()
    }

    /// A copy of a capability with the given rights only.
    fn weak(&mut self, source: u32, destination: u32, rights: Rights) -> u32 {
        self.ok(derive(s(source), s(destination), rights, 0));
        destination
    }

    fn inspect(&mut self, slot: SlotAddr) -> Result<(ObjectKind, Rights, u64), Error> {
        match self.go(Syscall::CapInspect(slot))? {
            Output::Inspect {
                kind,
                rights,
                badge,
            } => Ok((kind, rights, badge)),
            other => panic!("not an inspection: {other:?}"),
        }
    }

    fn kind(&mut self, slot: u32) -> Result<ObjectKind, Error> {
        Ok(self.inspect(s(slot))?.0)
    }

    fn woken(&self, thread: ObjId) -> Option<Outcome> {
        self.m.woken(thread).cloned()
    }

    fn can_run(&self, thread: ObjId) -> bool {
        self.m.runnable().contains(&thread)
    }
}

// Start-up.

#[test]
fn boot_state_is_deterministic_and_holds_the_start_up_slots() {
    let mut k = K::new();
    assert_eq!(Model::boot(), Model::boot());
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.runnable(), [k.t]);
    assert_eq!(k.m.object_at(k.t, s(START_SLOT_THREAD)), Some(k.t));
    assert_eq!(k.kind(START_SLOT_CSPACE), Ok(CSpace));
    assert_eq!(k.kind(START_SLOT_ASPACE), Ok(ASpace));
    assert_eq!(k.kind(START_SLOT_THREAD), Ok(Thread));
    assert_eq!(k.kind(START_SLOT_SCHED), Ok(ObjectKind::Sched));
    assert_eq!(k.kind(START_SLOT_DEBUG), Ok(ObjectKind::Debug));
    assert_eq!(k.inspect(UT), Ok((Untyped, Rights::ALL, 0)));
    let (kind, rights, _) = k.inspect(DEV).unwrap();
    assert!(kind == Untyped && !rights.contains(Rights::EXEC));
    assert_eq!(k.kind(START_SLOT_FIRST + 2), Err(Error::Invalid));
}

#[test]
fn deleting_the_first_capability_space_destroys_everything_in_it() {
    let mut k = K::new();
    assert_eq!(k.go(Syscall::CapDelete(SPACE)), DONE);
    assert!(k.m.runnable().is_empty());
}

// Slots and addresses.

#[test]
fn slot_zero_is_never_used() {
    let mut k = K::new();
    assert_eq!(k.kind(0), Err(Error::Invalid));
    k.no(retype(UT, Endpoint, 0, s(0)), Error::Invalid);
    k.no(mv(UT, s(0)), Error::Invalid);
}

#[test]
fn empty_or_out_of_range_slot_is_invalid() {
    let mut k = K::new();
    assert_eq!(k.kind(30), Err(Error::Invalid));
    assert_eq!(k.kind(128), Err(Error::Invalid));
    k.no(retype(UT, Endpoint, 0, s(128)), Error::Invalid);
}

#[test]
fn slot_of_a_revoked_capability_is_dead_and_using_it_is_gone() {
    let mut k = K::new();
    k.make(Endpoint, EP);
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(30), Err(Error::Gone));
    k.no(Syscall::Signal(s(30)), Error::Gone);
}

#[test]
fn dead_slot_is_not_free() {
    let mut k = K::new();
    k.make(Endpoint, EP);
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.no(mv(s(EP), s(30)), Error::NoSlot);
    k.no(retype(UT, Endpoint, 0, s(30)), Error::NoSlot);
}

#[test]
fn cap_delete_on_a_dead_slot_makes_it_empty() {
    let mut k = K::new();
    k.make(Endpoint, EP);
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(30), Err(Error::Invalid));
    k.ok(mv(s(EP), s(30)));
}

#[test]
fn cap_delete_on_an_empty_slot_is_invalid() {
    assert_eq!(K::new().go(Syscall::CapDelete(s(30))), Err(Error::Invalid));
}

#[test]
fn via_reaches_a_slot_of_another_capability_space() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Endpoint, 0, via(30, 5)));
    assert_eq!(k.inspect(via(30, 5)).unwrap().0, Endpoint);
    assert_eq!(k.kind(5), Ok(ObjectKind::Debug));
}

#[test]
fn via_must_name_a_live_cspace_capability() {
    let mut k = K::new();
    assert_eq!(k.inspect(via(START_SLOT_FIRST, 1)), Err(Error::Invalid));
    assert_eq!(k.inspect(via(30, 1)), Err(Error::Invalid));
    k.make(CSpace, 30);
    k.weak(30, 31, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(30)));
    assert_eq!(k.inspect(via(31, 1)), Err(Error::Gone));
}

#[test]
fn reading_a_slot_through_a_cspace_capability_needs_read() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Endpoint, 0, via(30, 5)));
    k.weak(30, 31, Rights::WRITE);
    assert_eq!(k.inspect(via(31, 5)), Err(Error::Denied));
    k.no(derive(via(31, 5), s(40), Rights::ALL, 0), Error::Denied);
    k.no(Syscall::Signal(via(31, 5)), Error::Denied);
    k.ok(mv(via(31, 5), s(40)));
}

#[test]
fn changing_a_slot_through_a_cspace_capability_needs_write() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Endpoint, 0, via(30, 5)));
    k.weak(30, 31, Rights::READ);
    k.no(retype(UT, Endpoint, 0, via(31, 6)), Error::Denied);
    k.no(derive(UT, via(31, 6), Rights::ALL, 0), Error::Denied);
    k.no(mv(UT, via(31, 6)), Error::Denied);
    k.no(mv(via(31, 5), s(40)), Error::Denied);
    k.no(Syscall::CapDelete(via(31, 5)), Error::Denied);
    k.no(Syscall::CapRevoke(via(31, 5)), Error::Denied);
    assert_eq!(k.inspect(via(31, 5)).unwrap().0, Endpoint);
}

#[test]
fn thread_needs_no_right_and_no_capability_for_its_own_capability_space() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 3)));
    k.make(Thread, 31);
    k.ok(configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(31)));
    let other = k.m.object_at(k.t, s(31)).unwrap();
    assert_eq!(k.on(other, mv(s(3), s(4))), DONE);
    assert_eq!(k.on(other, Syscall::Signal(s(4))), DONE);
}

#[test]
fn cspace_is_created_with_no_slot_and_grows_by_one_segment() {
    let mut k = K::new();
    k.ok(retype(UT, CSpace, 256, s(30)));
    assert_eq!(k.inspect(via(30, 1)), Err(Error::Invalid));
    k.no(retype(UT, Endpoint, 0, via(30, 1)), Error::Invalid);
    assert_eq!(k.go(grow(30)), Ok(Output::Slots(128)));
    k.ok(retype(UT, Endpoint, 0, via(30, 127)));
    k.no(retype(UT, Endpoint, 0, via(30, 128)), Error::Invalid);
}

#[test]
fn growing_a_cspace_does_not_change_slot_indexes() {
    let mut k = K::new();
    k.ok(retype(UT, CSpace, 256, s(30)));
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Endpoint, 0, via(30, 5)));
    assert_eq!(k.go(grow(30)), Ok(Output::Slots(256)));
    assert_eq!(k.inspect(via(30, 5)).unwrap().0, Endpoint);
    k.ok(retype(UT, Notify, 0, via(30, 255)));
}

#[test]
fn cspace_grow_is_no_slot_at_the_maximum() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.no(grow(30), Error::NoSlot);
}

#[test]
fn cspace_grow_needs_manage_on_the_cspace_and_write_on_the_untyped() {
    let mut k = K::new();
    k.make(CSpace, 30);
    let no_manage = k.weak(30, 31, caps_without(Rights::MANAGE));
    k.no(grow(no_manage), Error::Denied);
    let read_only = k.weak(START_SLOT_FIRST, 32, Rights::READ);
    let call = grow_from(30, s(read_only));
    k.no(call, Error::Denied);
}

#[test]
fn cspace_grow_takes_only_the_untyped_the_cspace_was_created_from() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.make(Untyped, 31);
    let from = |untyped| grow_from(30, untyped);
    k.no(from(s(31)), Error::Invalid);
    k.no(from(DEV), Error::Invalid);
    k.no(from(s(30)), Error::Invalid);
    k.no(grow(START_SLOT_FIRST), Error::Invalid);
    // Any capability for that untyped will do.
    let copy = k.weak(START_SLOT_FIRST, 32, Rights::WRITE);
    assert_eq!(k.go(from(s(copy))), Ok(Output::Slots(128)));
}

#[test]
fn capability_space_given_at_start_up_never_grows() {
    K::new().no(grow(START_SLOT_CSPACE), Error::Invalid);
}

#[test]
fn cspace_grow_is_no_memory_when_the_untyped_is_exhausted() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 8192, s(31)));
    k.ok(retype(s(31), CSpace, 384, s(30)));
    let from_child = grow_from(30, s(31));
    assert_eq!(k.go(from_child.clone()), Ok(Output::Slots(128)));
    k.no(from_child, Error::NoMemory);
}

#[test]
fn segments_go_back_to_the_untyped_when_the_cspace_is_destroyed() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 8192, s(31)));
    k.ok(retype(s(31), CSpace, 128, s(30)));
    let from_child = grow_from(30, s(31));
    k.go(from_child).unwrap();
    k.no(retype(s(31), Endpoint, 0, s(40)), Error::NoMemory);
    k.ok(Syscall::CapDelete(s(30)));
    // The watermark is back at zero: the whole untyped can be taken again.
    k.ok(retype(s(31), ASpace, 0, s(40)));
    k.ok(retype(s(31), ASpace, 0, s(41)));
}

/// Every right but one.
fn caps_without(right: Rights) -> Rights {
    Rights::from_bits(Rights::ALL.bits() & !right.bits()).unwrap()
}

// Order of checks.

#[test]
fn first_capability_is_checked_before_the_others() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.weak(START_SLOT_CSPACE, 31, Rights::READ);
    k.no(derive(s(30), via(31, 40), Rights::ALL, 0), Error::Gone);
    k.no(derive(s(EP), via(31, 40), Rights::ALL, 0), Error::Denied);
}

#[test]
fn kind_is_checked_before_rights() {
    let mut k = K::with_ipc();
    let no_right = k.weak(EP, 30, Rights::NONE);
    k.no(Syscall::Signal(s(no_right)), Error::Invalid);
    k.no(Syscall::Wait(s(no_right)), Error::Invalid);
}

#[test]
fn capability_arguments_are_checked_before_plain_arguments() {
    let mut k = K::new();
    k.weak(START_SLOT_CSPACE, 31, Rights::READ);
    k.no(derive(UT, via(31, 40), Rights::ALL, 7), Error::Denied);
    k.no(retype(UT, Endpoint, 1, via(31, 40)), Error::Denied);
}

#[test]
fn plain_arguments_are_checked_before_the_state() {
    let mut k = K::new();
    k.no(retype(UT, Endpoint, 1, UT), Error::Invalid);
    k.no(derive(UT, UT, Rights::ALL, 7), Error::Invalid);
}

#[test]
fn no_slot_is_reported_before_no_memory() {
    let mut k = K::new();
    k.no(retype(UT, Frame, 1 << 40, UT), Error::NoSlot);
    k.no(retype(UT, Frame, 1 << 40, s(30)), Error::NoMemory);
}

// The derivation tree.

#[test]
fn cap_derive_makes_a_capability_with_the_rights_asked_for() {
    let mut k = K::with_ipc();
    k.ok(derive(s(EP), s(30), Rights::SEND.union(Rights::DERIVE), 0));
    assert_eq!(
        k.inspect(s(30)),
        Ok((Endpoint, Rights::SEND.union(Rights::DERIVE), 0))
    );
    assert_eq!(k.inspect(s(EP)), Ok((Endpoint, Rights::ALL, 0)));
}

#[test]
fn cap_derive_needs_derive_on_the_source() {
    let mut k = K::with_ipc();
    let no_derive = k.weak(EP, 30, caps_without(Rights::DERIVE));
    k.no(derive(s(no_derive), s(31), Rights::NONE, 0), Error::Denied);
}

#[test]
fn cap_derive_cannot_add_rights() {
    let mut k = K::with_ipc();
    let weak = k.weak(EP, 30, Rights::SEND.union(Rights::DERIVE));
    k.no(derive(s(weak), s(31), Rights::RECV, 0), Error::Denied);
}

#[test]
fn cap_derive_takes_a_full_source_and_an_empty_destination() {
    let mut k = K::with_ipc();
    k.no(derive(s(30), s(31), Rights::ALL, 0), Error::Invalid);
    k.no(derive(s(EP), s(NT), Rights::ALL, 0), Error::NoSlot);
    k.no(derive(s(EP), s(128), Rights::ALL, 0), Error::Invalid);
}

#[test]
fn badge_is_accepted_only_on_an_endpoint_or_a_notify() {
    let mut k = K::with_ipc();
    k.ok(derive(s(EP), s(30), Rights::ALL, 7));
    k.ok(derive(s(NT), s(31), Rights::ALL, 8));
    assert_eq!(k.inspect(s(30)).unwrap().2, 7);
    assert_eq!(k.inspect(s(31)).unwrap().2, 8);
    k.no(derive(s(RP), s(32), Rights::ALL, 7), Error::Invalid);
    k.no(derive(UT, s(32), Rights::ALL, 7), Error::Invalid);
}

#[test]
fn badged_capability_keeps_its_badge_and_refuses_another() {
    let mut k = K::with_ipc();
    k.ok(derive(s(EP), s(30), Rights::ALL, 7));
    k.no(derive(s(30), s(31), Rights::ALL, 8), Error::Denied);
    k.ok(derive(s(30), s(31), Rights::ALL, 0));
    k.ok(derive(s(30), s(32), Rights::ALL, 7));
    assert_eq!(k.inspect(s(31)).unwrap().2, 7);
    assert_eq!(k.inspect(s(32)).unwrap().2, 7);
}

#[test]
fn cap_move_keeps_rights_badge_and_place_in_the_tree() {
    let mut k = K::with_ipc();
    k.ok(derive(s(EP), s(30), Rights::SEND, 7));
    k.ok(mv(s(30), s(31)));
    assert_eq!(k.kind(30), Err(Error::Invalid));
    assert_eq!(k.inspect(s(31)), Ok((Endpoint, Rights::SEND, 7)));
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn cap_move_needs_no_right_on_the_capability() {
    let mut k = K::with_ipc();
    let no_right = k.weak(EP, 30, Rights::NONE);
    k.ok(mv(s(no_right), s(31)));
}

#[test]
fn cap_move_takes_a_full_source_and_an_empty_destination() {
    let mut k = K::with_ipc();
    k.no(mv(s(30), s(31)), Error::Invalid);
    k.no(mv(s(EP), s(NT)), Error::NoSlot);
    k.no(mv(s(EP), s(EP)), Error::NoSlot);
}

#[test]
fn cap_inspect_needs_no_right_on_the_capability() {
    let mut k = K::with_ipc();
    let no_right = k.weak(EP, 30, Rights::NONE);
    assert_eq!(k.inspect(s(no_right)), Ok((Endpoint, Rights::NONE, 0)));
}

#[test]
fn cap_revoke_removes_the_descendants_and_keeps_the_capability() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    k.weak(30, 31, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(30), Err(Error::Gone));
    assert_eq!(k.kind(31), Err(Error::Gone));
    assert_eq!(k.kind(EP), Ok(Endpoint));
}

#[test]
fn cap_revoke_takes_a_live_capability() {
    let mut k = K::with_ipc();
    k.no(Syscall::CapRevoke(s(30)), Error::Invalid);
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.no(Syscall::CapRevoke(s(30)), Error::Gone);
}

#[test]
fn cap_delete_removes_the_descendants_instead_of_handing_them_up() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    k.weak(30, 31, Rights::ALL);
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(30), Err(Error::Invalid));
    assert_eq!(k.kind(31), Err(Error::Gone));
    assert_eq!(k.kind(EP), Ok(Endpoint));
}

#[test]
fn object_lives_as_long_as_its_root_capability_wherever_it_is_moved() {
    let mut k = K::new();
    k.make(Untyped, 30);
    k.ok(retype(s(30), ASpace, 0, s(31)));
    k.weak(31, 32, Rights::ALL);
    k.ok(mv(s(31), s(33)));
    k.ok(Syscall::CapDelete(s(32)));
    k.no(retype(s(30), ASpace, 0, s(34)), Error::NoMemory);
    k.weak(33, 32, Rights::ALL);
    k.ok(Syscall::CapDelete(s(33)));
    assert_eq!(k.kind(32), Err(Error::Gone));
    k.ok(retype(s(30), ASpace, 0, s(34)));
}

#[test]
fn cap_revoke_removes_32_capabilities_then_returns_again() {
    let mut k = K::with_ipc();
    for slot in 30..70 {
        k.weak(EP, slot, Rights::ALL);
    }
    k.no(Syscall::CapRevoke(s(EP)), Error::Again);
    // The most recently derived child goes first.
    assert_eq!(k.kind(38), Err(Error::Gone));
    assert_eq!(k.kind(37), Ok(Endpoint));
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(30), Err(Error::Gone));
}

#[test]
fn cap_revoke_of_32_descendants_is_one_call() {
    let mut k = K::with_ipc();
    for slot in 30..62 {
        k.weak(EP, slot, Rights::ALL);
    }
    k.ok(Syscall::CapRevoke(s(EP)));
}

#[test]
fn cap_delete_returns_again_while_descendants_remain() {
    let mut k = K::with_ipc();
    for slot in 30..62 {
        k.weak(EP, slot, Rights::ALL);
    }
    k.no(Syscall::CapDelete(s(EP)), Error::Again);
    assert_eq!(k.kind(30), Err(Error::Gone));
    k.ok(Syscall::CapDelete(s(EP)));
    assert_eq!(k.kind(EP), Err(Error::Invalid));
}

#[test]
fn dying_capability_is_gone_for_every_call_but_cap_delete() {
    let mut k = K::with_ipc();
    for slot in 30..62 {
        k.weak(NT, slot, Rights::ALL);
    }
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    assert_eq!(k.kind(NT), Err(Error::Gone));
    k.no(Syscall::CapRevoke(s(NT)), Error::Gone);
    k.no(Syscall::Signal(s(NT)), Error::Gone);
    k.no(mv(s(NT), s(70)), Error::Gone);
    k.no(send(EP, msg(1, vec![Transfer::Move(s(NT))])), Error::Gone);
    k.ok(Syscall::CapDelete(s(NT)));
}

#[test]
fn copy_under_cap_delete_is_dying_and_its_object_is_not() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    for slot in 31..64 {
        k.weak(30, slot, Rights::ALL);
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.kind(30), Err(Error::Gone));
    k.ok(Syscall::Signal(s(31)));
    assert_eq!(k.go(Syscall::Wait(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn capability_is_not_dying_after_cap_revoke_returns_again() {
    let mut k = K::with_ipc();
    for slot in 30..63 {
        k.weak(NT, slot, Rights::ALL);
    }
    k.no(Syscall::CapRevoke(s(NT)), Error::Again);
    assert_eq!(k.kind(NT), Ok(Notify));
}

/// Gives a capability 33 copies, then calls `cap_delete` on it once: it is
/// dying, and its oldest copy, in the slot returned, is still there.
fn dying(k: &mut K, root: u32, first_copy: u32) -> u32 {
    for slot in first_copy..first_copy + 33 {
        k.weak(root, slot, Rights::ALL);
    }
    k.no(Syscall::CapDelete(s(root)), Error::Again);
    first_copy
}

#[test]
fn every_call_on_a_dying_object_is_gone() {
    let mut k = K::with_ipc();
    let notify = dying(&mut k, NT, 30);
    k.no(Syscall::Wait(s(notify)), Error::Gone);
    k.no(Syscall::Poll(s(notify)), Error::Gone);
    k.no(Syscall::Signal(s(notify)), Error::Gone);
    assert_eq!(k.kind(notify), Err(Error::Gone));
    let endpoint = dying(&mut k, EP, 70);
    k.no(call(endpoint, msg(1, vec![]), Range::NONE), Error::Gone);
    k.no(recv(endpoint, RP, Range::NONE), Error::Gone);
    k.no(send(endpoint, msg(1, vec![])), Error::Gone);
    // `cap_delete` still takes the slot.
    k.ok(Syscall::CapDelete(s(notify)));
    assert_eq!(k.kind(notify), Err(Error::Invalid));
}

#[test]
fn object_is_dying_as_soon_as_a_removal_reaches_its_root_capability() {
    let mut k = K::new();
    k.make(Untyped, 30);
    k.ok(retype(s(30), Notify, 0, s(31)));
    for slot in 40..73 {
        k.weak(31, slot, Rights::ALL);
    }
    // The first step removes copies of the notify capability only.
    k.no(Syscall::CapRevoke(s(30)), Error::Again);
    assert_eq!(k.kind(40), Err(Error::Gone));
    assert_eq!(k.kind(31), Err(Error::Gone));
    assert_eq!(k.kind(30), Ok(Untyped));
}

#[test]
fn recv_with_a_dying_reply_object_is_gone_and_the_caller_stays_queued() {
    let mut k = K::with_ipc();
    let caller = k.spawn(70);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    let copy = dying(&mut k, RP, 30);
    k.no(recv(EP, copy, Range::NONE), Error::Gone);
    k.no(reply_recv(copy, msg(1, vec![]), EP), Error::Gone);
    assert_eq!(k.woken(caller), None);
    k.make(Reply, 80);
    assert_eq!(got(k.go(recv(EP, 80, Range::NONE))), (1, 0, 0, 0));
}

#[test]
fn marking_a_reply_object_as_dying_wakes_the_receiver_attached_to_it() {
    let mut k = K::with_ipc();
    let server = k.spawn(70);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    dying(&mut k, RP, 30);
    assert_eq!(k.woken(server), Some(Err(Error::Gone)));
    // So a call meets no receiver, and joins the queue.
    assert_eq!(k.on(server, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.woken(server), None);
}

#[test]
fn marking_a_reply_object_as_dying_wakes_the_caller_it_is_armed_with() {
    let mut k = K::with_ipc();
    let caller = k.spawn(70);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    dying(&mut k, RP, 30);
    assert_eq!(k.woken(caller), Some(Err(Error::Gone)));
}

#[test]
fn capability_is_removed_after_its_descendants_the_most_recent_first() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    for slot in 31..64 {
        k.weak(30, slot, Rights::ALL);
    }
    k.no(Syscall::CapRevoke(s(EP)), Error::Again);
    assert_eq!(k.kind(32), Err(Error::Gone));
    assert_eq!(k.kind(31), Ok(Endpoint));
    assert_eq!(k.kind(30), Ok(Endpoint));
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(30), Err(Error::Gone));
}

#[test]
fn destroying_an_object_wakes_the_threads_that_wait_on_it_with_gone() {
    let mut k = K::with_ipc();
    let (caller, receiver, waiter) = (k.spawn(40), k.spawn(41), k.spawn(42));
    k.make(Endpoint, 30);
    k.make(Reply, 31);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(receiver, recv(30, 31, Range::NONE)), WAITS);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT))), WAITS);
    for slot in [EP, 30, NT] {
        k.ok(Syscall::CapDelete(s(slot)));
    }
    for thread in [caller, receiver, waiter] {
        assert_eq!(k.woken(thread), Some(Err(Error::Gone)));
        assert!(k.can_run(thread));
    }
    // The reply object of the receiver is idle again.
    k.make(Endpoint, 32);
    assert_eq!(k.on(receiver, recv(32, 31, Range::NONE)), WAITS);
}

#[test]
fn destroying_a_reply_object_wakes_its_caller_and_its_receiver_with_gone() {
    let mut k = K::with_ipc();
    let (caller, receiver) = (k.spawn(40), k.spawn(41));
    k.make(Reply, 30);
    k.make(Endpoint, 31);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(k.on(receiver, recv(31, 30, Range::NONE)), WAITS);
    k.ok(Syscall::CapDelete(s(RP)));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.woken(caller), Some(Err(Error::Gone)));
    assert_eq!(k.woken(receiver), Some(Err(Error::Gone)));
    k.no(send(31, msg(2, vec![])), Error::WouldWait);
}

#[test]
fn waking_a_thread_counts_as_one_step_of_removal() {
    let mut k = K::with_ipc();
    let threads: Vec<ObjId> = (40..72).map(|slot| k.spawn(slot)).collect();
    for &thread in &threads {
        assert_eq!(k.on(thread, Syscall::Wait(s(NT))), WAITS);
    }
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    assert!(
        threads
            .iter()
            .all(|&thread| k.woken(thread) == Some(Err(Error::Gone)))
    );
    k.ok(Syscall::CapDelete(s(NT)));
}

/// A capability space in slot 30 that holds 20 notifies, each with a copy
/// in slots 41 to 60 of the first space, and a thread that runs in it.
fn space_with_a_thread(k: &mut K) -> ObjId {
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    for index in 1..=20 {
        k.ok(retype(UT, Notify, 0, via(30, index)));
        k.ok(derive(via(30, index), s(40 + index), Rights::ALL, 0));
    }
    k.make(Thread, 31);
    k.ok(configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(31)));
    k.m.object_at(k.t, s(31)).unwrap()
}

#[test]
fn destroying_a_cspace_removes_its_capabilities_in_index_order() {
    let mut k = K::new();
    space_with_a_thread(&mut k);
    // 32 steps: the copy, then the notify, of the first 16 slots.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.kind(56), Err(Error::Gone));
    assert_eq!(k.kind(57), Ok(Notify));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(57), Err(Error::Gone));
    assert_eq!(k.kind(30), Err(Error::Invalid));
}

#[test]
fn thread_whose_capability_space_is_dying_gets_gone_from_every_call_but_cap_delete() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.on(inside, Syscall::CapInspect(s(20))), Err(Error::Gone));
    assert_eq!(k.on(inside, Syscall::Signal(s(20))), Err(Error::Gone));
    assert_eq!(
        k.on(inside, derive(s(20), s(30), Rights::ALL, 0)),
        Err(Error::Gone)
    );
    assert_eq!(k.on(inside, Syscall::Other(Call::Yield)), Err(Error::Gone));
    // `cap_delete` works, on a full slot and on a dead one.
    assert_eq!(k.on(inside, Syscall::CapDelete(s(20))), DONE);
    assert_eq!(k.on(inside, Syscall::CapDelete(s(1))), DONE);
    assert_eq!(k.kind(60), Err(Error::Gone));
}

#[test]
fn delivery_into_a_dying_capability_space_drops_the_capabilities() {
    let mut k = K::with_ipc();
    space_with_a_thread(&mut k);
    let server = k.spawn(70);
    let range = Range {
        first: via(30, 25),
        count: 1,
    };
    assert_eq!(k.on(server, recv(EP, RP, range)), WAITS);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    let sent = k.go(send(EP, msg(5, vec![Transfer::Derive(s(NT), Rights::ALL)])));
    assert_eq!(sent, Ok(Output::Sent { flags: DROPPED }));
    assert_eq!(got(k.woken(server).unwrap()), (5, 0, 0, DROPPED));
}

#[test]
fn destroying_a_cspace_destroys_the_objects_whose_root_it_holds() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 5)));
    k.ok(derive(via(30, 5), s(31), Rights::ALL, 0));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn thread_whose_capability_space_is_destroyed_gets_gone_from_every_call() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.make(Thread, 31);
    k.ok(configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(31)));
    let other = k.m.object_at(k.t, s(31)).unwrap();
    k.ok(Syscall::CapDelete(s(30)));
    assert!(k.can_run(other));
    assert_eq!(k.on(other, Syscall::CapInspect(s(1))), Err(Error::Gone));
    assert_eq!(k.on(other, Syscall::Other(Call::Yield)), Err(Error::Gone));
    assert_eq!(k.on(other, Syscall::CapDelete(s(1))), Err(Error::Gone));
}

#[test]
fn revoking_the_capability_a_thread_used_does_not_disturb_its_wait() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    k.weak(NT, 30, Rights::RECV);
    assert_eq!(k.on(waiter, Syscall::Wait(s(30))), WAITS);
    k.ok(Syscall::CapRevoke(s(NT)));
    assert_eq!(k.kind(30), Err(Error::Gone));
    assert!(!k.can_run(waiter) && k.woken(waiter).is_none());
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
}

#[test]
fn cspace_that_holds_its_own_root_capability_can_be_deleted() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.weak(30, 31, Rights::ALL);
    k.ok(mv(s(30), via(31, 1)));
    k.ok(Syscall::CapDelete(via(31, 1)));
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn untyped_capability_moves_only_into_a_capability_space_older_than_itself() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.no(mv(UT, via(30, 5)), Error::Invalid);
    k.no(mv(DEV, via(30, 5)), Error::Invalid);
    // An untyped capability made after the space, a child or a copy, can
    // go there.
    k.make(Untyped, 31);
    k.weak(START_SLOT_FIRST, 32, Rights::ALL);
    k.ok(mv(s(31), via(30, 5)));
    k.ok(mv(s(32), via(30, 6)));
}

#[test]
fn capability_of_another_kind_moves_into_any_capability_space() {
    let mut k = K::with_ipc();
    k.weak(START_SLOT_CSPACE, 31, Rights::ALL);
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    // All three are older than the space; the second is not a root.
    k.ok(mv(s(START_SLOT_DEBUG), via(30, 5)));
    k.ok(mv(s(31), via(30, 6)));
    k.ok(mv(s(EP), via(30, 7)));
}

#[test]
fn root_capability_of_a_cspace_moves_only_into_a_capability_space_older_than_itself() {
    let mut k = K::new();
    for slot in [30, 31] {
        k.make(CSpace, slot);
        k.go(grow(slot)).unwrap();
        k.weak(slot, slot + 2, Rights::ALL);
    }
    k.no(mv(s(30), via(33, 1)), Error::Invalid);
    k.ok(mv(s(31), via(32, 1)));
    // Deleting the older space destroys the younger one it holds.
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(33), Err(Error::Gone));
}

#[test]
fn refused_move_is_invalid_before_no_slot() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.make(Notify, 31);
    k.ok(mv(s(31), via(30, 5)));
    k.no(mv(UT, via(30, 5)), Error::Invalid);
}

#[test]
fn capability_moved_by_ipc_into_a_capability_space_younger_than_itself_is_dropped() {
    let mut k = K::with_ipc();
    k.make(Untyped, 30);
    k.make(CSpace, 31);
    k.go(grow(31)).unwrap();
    caller_with(&mut k, vec![Transfer::Move(s(30))], Range::NONE);
    let range = Range {
        first: via(31, 5),
        count: 1,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(30), Ok(Untyped));
}

#[test]
fn capability_derived_by_ipc_lands_in_any_capability_space() {
    let mut k = K::with_ipc();
    k.make(CSpace, 31);
    k.go(grow(31)).unwrap();
    caller_with(&mut k, vec![Transfer::Derive(UT, Rights::ALL)], Range::NONE);
    let range = Range {
        first: via(31, 5),
        count: 1,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 1, 0));
    assert_eq!(k.inspect(via(31, 5)).unwrap().0, Untyped);
}

#[test]
fn derivation_deeper_than_64_is_invalid() {
    let mut k = K::with_ipc();
    // The untyped is at depth 1, the endpoint at depth 2.
    for slot in 30..92 {
        k.weak(
            slot - if slot == 30 { 30 - EP } else { 1 },
            slot,
            Rights::ALL,
        );
    }
    k.no(derive(s(91), s(92), Rights::ALL, 0), Error::Invalid);
    let deep = msg(1, vec![Transfer::Derive(s(91), Rights::NONE)]);
    k.no(send(EP, deep), Error::Invalid);
    k.ok(mv(s(91), s(92)));
}

#[test]
fn creation_deeper_than_64_is_invalid() {
    let mut k = K::new();
    for slot in 30..93 {
        let source = if slot == 30 {
            START_SLOT_FIRST
        } else {
            slot - 1
        };
        k.weak(source, slot, Rights::ALL);
    }
    k.no(retype(s(92), Endpoint, 0, s(100)), Error::Invalid);
    k.ok(retype(s(91), Endpoint, 0, s(100)));
    k.no(derive(s(100), s(101), Rights::ALL, 0), Error::Invalid);
}

// Capabilities and objects: untyped_retype.

#[test]
fn untyped_retype_needs_write_on_an_untyped() {
    let mut k = K::with_ipc();
    let read_only = k.weak(START_SLOT_FIRST, 30, Rights::READ);
    k.no(retype(s(read_only), Endpoint, 0, s(31)), Error::Denied);
    k.no(retype(s(EP), Endpoint, 0, s(31)), Error::Invalid);
    k.no(retype(s(31), Endpoint, 0, s(32)), Error::Invalid);
}

#[test]
fn untyped_retype_needs_an_empty_destination() {
    let mut k = K::with_ipc();
    k.no(retype(UT, Endpoint, 0, s(EP)), Error::NoSlot);
}

#[test]
fn child_untyped_is_a_power_of_two_of_at_least_4_kb() {
    let mut k = K::new();
    for size in [0, 2048, 4097, 12288] {
        k.no(retype(UT, Untyped, size, s(30)), Error::Invalid);
    }
    k.ok(retype(UT, Untyped, 4096, s(30)));
    k.ok(retype(UT, Untyped, 1 << 20, s(31)));
}

#[test]
fn frame_takes_a_page_size_and_a_number_of_pages() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 1 << 22, s(30)));
    k.no(retype(s(30), Frame, 0, s(31)), Error::Invalid);
    let large = |size, destination| Syscall::UntypedRetype {
        untyped: s(30),
        kind: Frame,
        size,
        page: PageSize::Size2M,
        destination,
    };
    k.ok(large(2, s(31)));
    k.no(retype(s(30), Frame, 1, s(32)), Error::NoMemory);
    k.no(large(u64::MAX, s(32)), Error::NoMemory);
}

#[test]
fn cspace_maximum_is_a_whole_number_of_segments() {
    let mut k = K::new();
    for size in [0, 1, 127, 129, 1 << 32] {
        k.no(retype(UT, CSpace, size, s(30)), Error::Invalid);
    }
    k.ok(retype(UT, CSpace, 384, s(30)));
}

#[test]
fn size_argument_is_zero_for_the_other_kinds() {
    let mut k = K::new();
    for kind in [ASpace, Thread, ObjectKind::Sched, Endpoint, Reply, Notify] {
        k.no(retype(UT, kind, 1, s(30)), Error::Invalid);
        k.ok(retype(UT, kind, 0, s(30)));
        assert_eq!(k.kind(30), Ok(kind));
        k.ok(Syscall::CapDelete(s(30)));
    }
}

#[test]
fn kinds_outside_the_one_core_model_are_unsupported() {
    let mut k = K::new();
    for kind in [
        ObjectKind::Timer,
        ObjectKind::Task,
        ObjectKind::Irq,
        ObjectKind::Debug,
    ] {
        k.no(retype(UT, kind, 0, s(30)), Error::Unsupported);
    }
}

#[test]
fn device_memory_becomes_only_a_frame_or_a_child_untyped() {
    let mut k = K::new();
    k.no(retype(DEV, Endpoint, 0, s(30)), Error::Invalid);
    k.no(retype(DEV, CSpace, 128, s(30)), Error::Invalid);
    k.ok(retype(DEV, Frame, 1, s(30)));
    k.ok(retype(DEV, Untyped, 4096, s(31)));
    k.no(retype(s(31), Endpoint, 0, s(32)), Error::Invalid);
    k.ok(retype(s(31), Frame, 1, s(32)));
}

#[test]
fn new_capability_has_every_right() {
    let mut k = K::new();
    k.make(Frame, 30);
    assert_eq!(k.inspect(s(30)), Ok((Frame, Rights::ALL, 0)));
    // Whatever the rights of the untyped capability, `exec` aside.
    let write_only = k.weak(START_SLOT_FIRST, 31, Rights::WRITE);
    k.ok(retype(s(write_only), Endpoint, 0, s(32)));
    assert_eq!(
        k.inspect(s(32)),
        Ok((Endpoint, caps_without(Rights::EXEC), 0))
    );
}

#[test]
fn new_capability_has_exec_only_if_the_untyped_capability_has_it() {
    let mut k = K::new();
    let no_exec = k.weak(START_SLOT_FIRST, 30, caps_without(Rights::EXEC));
    k.ok(retype(s(no_exec), Frame, 1, s(31)));
    assert_eq!(k.inspect(s(31)), Ok((Frame, caps_without(Rights::EXEC), 0)));
}

#[test]
fn new_capability_never_has_exec_on_device_memory() {
    let mut k = K::new();
    k.ok(retype(DEV, Frame, 1, s(30)));
    k.ok(retype(DEV, Untyped, 4096, s(31)));
    assert_eq!(k.inspect(s(30)), Ok((Frame, caps_without(Rights::EXEC), 0)));
    assert_eq!(
        k.inspect(s(31)),
        Ok((Untyped, caps_without(Rights::EXEC), 0))
    );
}

#[test]
fn new_capability_is_a_child_of_the_untyped_capability() {
    let mut k = K::new();
    k.weak(START_SLOT_FIRST, 30, Rights::ALL);
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(Syscall::CapRevoke(s(30)));
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn untyped_retype_is_no_memory_when_the_untyped_is_exhausted() {
    let mut k = K::new();
    k.make(Untyped, 30);
    k.ok(retype(s(30), ASpace, 0, s(31)));
    k.no(retype(s(30), Endpoint, 0, s(32)), Error::NoMemory);
}

#[test]
fn memory_is_aligned_on_the_size_of_the_object() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 8192, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(retype(s(30), ASpace, 0, s(32)));
    k.no(retype(s(30), Endpoint, 0, s(33)), Error::NoMemory);
}

#[test]
fn child_untyped_is_aligned_on_its_size() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 1 << 16, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(retype(s(30), Untyped, 1 << 15, s(32)));
    k.no(retype(s(30), Untyped, 4096, s(33)), Error::NoMemory);
}

#[test]
fn kernel_object_larger_than_4_kb_is_aligned_on_4_kb() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 1 << 14, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    // A directory of 8 KB, at 4 KB: the last 4 KB are left.
    k.ok(retype(s(30), CSpace, 513 * 128, s(32)));
    k.ok(retype(s(30), ASpace, 0, s(33)));
}

#[test]
fn cspace_directory_is_rounded_up_to_a_power_of_two() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 8192, s(30)));
    // 513 segments: 4,104 bytes of directory, rounded up to 8 KB.
    k.ok(retype(s(30), CSpace, 513 * 128, s(31)));
    k.no(retype(s(30), Endpoint, 0, s(32)), Error::NoMemory);
}

#[test]
fn frame_is_aligned_on_its_page_size() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 16384, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(retype(s(30), Frame, 3, s(32)));
    k.no(retype(s(30), Endpoint, 0, s(33)), Error::NoMemory);
}

#[test]
fn watermark_returns_to_zero_only_when_the_last_object_is_destroyed() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 8192, s(30)));
    k.ok(retype(s(30), ASpace, 0, s(31)));
    k.ok(retype(s(30), ASpace, 0, s(32)));
    k.ok(Syscall::CapDelete(s(31)));
    k.no(retype(s(30), ASpace, 0, s(33)), Error::NoMemory);
    k.ok(Syscall::CapDelete(s(32)));
    k.ok(retype(s(30), ASpace, 0, s(33)));
}

// Threads.

#[test]
fn new_thread_is_suspended() {
    let mut k = K::new();
    k.make(Thread, 30);
    assert_eq!(k.m.runnable(), [k.t]);
}

#[test]
fn thread_resume_is_busy_when_the_thread_is_bound_to_no_capability_space() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.no(Syscall::ThreadResume(s(30)), Error::Busy);
    k.ok(configure(30, SPACE, SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(30)));
}

#[test]
fn thread_configure_binds_the_capability_space() {
    let mut k = K::new();
    k.make(ASpace, 31);
    k.make(Endpoint, 32);
    k.make(Thread, 30);
    k.ok(configure(30, SPACE, s(31), s(32)));
    k.ok(Syscall::ThreadResume(s(30)));
    let other = k.m.object_at(k.t, s(30)).unwrap();
    assert_eq!(k.on(other, Syscall::CapInspect(s(31))).map(drop), Ok(()));
}

#[test]
fn thread_configure_needs_manage_on_the_thread_the_cspace_and_the_aspace() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.make(ASpace, 31);
    let none = SlotAddr::NONE;
    let thread = k.weak(30, 40, caps_without(Rights::MANAGE));
    let cspace = k.weak(START_SLOT_CSPACE, 41, caps_without(Rights::MANAGE));
    let aspace = k.weak(31, 42, caps_without(Rights::MANAGE));
    k.no(configure(thread, SPACE, none, none), Error::Denied);
    k.no(configure(30, s(cspace), none, none), Error::Denied);
    k.no(configure(30, SPACE, s(aspace), none), Error::Denied);
}

#[test]
fn thread_configure_needs_send_on_the_fault_endpoint() {
    let mut k = K::with_ipc();
    k.make(Thread, 30);
    let no_send = k.weak(EP, 40, caps_without(Rights::SEND));
    k.no(
        configure(30, SPACE, SlotAddr::NONE, s(no_send)),
        Error::Denied,
    );
    k.ok(configure(30, SPACE, SlotAddr::NONE, s(EP)));
}

#[test]
fn thread_configure_checks_the_kind_of_each_capability() {
    let mut k = K::with_ipc();
    k.make(Thread, 30);
    let none = SlotAddr::NONE;
    k.no(configure(EP, SPACE, none, none), Error::Invalid);
    k.no(configure(30, s(EP), none, none), Error::Invalid);
    k.no(configure(30, none, none, none), Error::Invalid);
    k.no(configure(30, SPACE, s(EP), none), Error::Invalid);
    k.no(configure(30, SPACE, none, s(NT)), Error::Invalid);
}

#[test]
fn thread_configure_is_busy_unless_the_thread_is_suspended() {
    let mut k = K::new();
    let _ = k.spawn(30);
    let none = SlotAddr::NONE;
    k.no(configure(30, SPACE, none, none), Error::Busy);
    k.no(configure(START_SLOT_THREAD, SPACE, none, none), Error::Busy);
    k.ok(Syscall::ThreadSuspend(s(30)));
    k.ok(configure(30, SPACE, none, none));
}

#[test]
fn thread_resume_and_suspend_succeed_in_the_state_asked_for() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.ok(configure(30, SPACE, SlotAddr::NONE, SlotAddr::NONE));
    let other = k.m.object_at(k.t, s(30)).unwrap();
    k.ok(Syscall::ThreadSuspend(s(30)));
    assert!(!k.can_run(other));
    k.ok(Syscall::ThreadResume(s(30)));
    k.ok(Syscall::ThreadResume(s(30)));
    assert!(k.can_run(other));
    k.ok(Syscall::ThreadSuspend(s(30)));
    assert!(!k.can_run(other));
}

#[test]
fn thread_resume_and_suspend_need_manage_on_a_thread() {
    let mut k = K::with_ipc();
    k.make(Thread, 30);
    let no_manage = k.weak(30, 40, caps_without(Rights::MANAGE));
    k.no(Syscall::ThreadResume(s(no_manage)), Error::Denied);
    k.no(Syscall::ThreadSuspend(s(no_manage)), Error::Denied);
    k.no(Syscall::ThreadResume(s(EP)), Error::Invalid);
    k.no(Syscall::ThreadSuspend(s(EP)), Error::Invalid);
}

#[test]
fn thread_that_suspends_itself_stops_running() {
    let mut k = K::new();
    k.ok(Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    assert!(k.m.runnable().is_empty());
}

#[test]
fn suspended_thread_keeps_its_place_in_its_queue_and_still_gets_its_message() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(first, Syscall::Wait(s(NT))), WAITS);
    assert_eq!(k.on(second, Syscall::Wait(s(NT))), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(first), Some(Ok(Output::Word(1))));
    assert_eq!(k.woken(second), None);
    assert!(!k.can_run(first));
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(first));
}

#[test]
fn calls_outside_the_one_core_model_are_unsupported() {
    let mut k = K::new();
    for call in [
        Call::ThreadReadRegs,
        Call::ThreadWriteRegs,
        Call::Yield,
        Call::ASpaceMap,
    ] {
        k.no(Syscall::Other(call), Error::Unsupported);
    }
}

// IPC.

#[test]
fn call_joins_the_queue_and_recv_takes_the_first_caller() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Reply, 30);
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    assert!(!k.can_run(first) && !k.can_run(second));
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (1, 0, 0, 0));
    assert_eq!(got(k.go(recv(EP, 30, Range::NONE))), (2, 0, 0, 0));
}

#[test]
fn call_is_delivered_at_once_to_a_thread_that_waits_to_receive() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.go(call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.woken(server).unwrap()), (1, 0, 0, 0));
    assert!(k.can_run(server) && !k.can_run(k.t));
}

#[test]
fn receiver_gets_the_badge_of_the_capability_the_sender_used() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.ok(derive(s(EP), s(30), Rights::SEND, 42));
    assert_eq!(k.on(caller, call(30, msg(1, vec![]), Range::NONE)), WAITS);
    let Ok(Output::Message(delivery)) = k.go(recv(EP, RP, Range::NONE)) else {
        panic!("not a message");
    };
    assert_eq!(delivery.received.badge, 42);
    assert_eq!(delivery.received.marks, Marks::NONE);
    assert_eq!(delivery.received.task, 0);
    assert_eq!(delivery.words, [1]);
}

#[test]
fn send_on_a_reply_object_answers_the_caller_with_badge_0() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.ok(derive(s(EP), s(30), Rights::SEND, 42));
    k.ok(derive(s(RP), s(31), Rights::SEND, 0));
    assert_eq!(k.on(caller, call(30, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(
        k.go(send(31, msg(9, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, 0));
    assert!(k.can_run(caller));
    // The reply object is idle again.
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
}

#[test]
fn call_needs_send_on_an_endpoint() {
    let mut k = K::with_ipc();
    let no_send = k.weak(EP, 30, caps_without(Rights::SEND));
    k.no(call(no_send, msg(1, vec![]), Range::NONE), Error::Denied);
    k.no(call(NT, msg(1, vec![]), Range::NONE), Error::Invalid);
    k.no(call(30 + 1, msg(1, vec![]), Range::NONE), Error::Invalid);
}

#[test]
fn message_has_at_most_64_words_and_4_capabilities() {
    let mut k = K::with_ipc();
    let mut long = msg(1, vec![]);
    long.words = vec![0; MAX_WORDS + 1];
    k.no(call(EP, long.clone(), Range::NONE), Error::Invalid);
    k.no(send(EP, long.clone()), Error::Invalid);
    k.no(reply_recv(RP, long, EP), Error::Invalid);
    let many = msg(1, vec![Transfer::Derive(s(NT), Rights::NONE); MAX_CAPS + 1]);
    k.no(call(EP, many.clone(), Range::NONE), Error::Invalid);
    k.no(send(EP, many), Error::Invalid);
    long = msg(1, vec![]);
    long.words = vec![1; MAX_WORDS];
    k.no(send(EP, long), Error::WouldWait);
}

#[test]
fn receive_range_has_at_most_4_slots() {
    let mut k = K::with_ipc();
    k.no(call(EP, msg(1, vec![]), range(40, 5)), Error::Invalid);
    k.no(recv(EP, RP, range(40, 5)), Error::Invalid);
}

#[test]
fn receive_range_through_a_cspace_capability_needs_write() {
    let mut k = K::with_ipc();
    let read_only = k.weak(START_SLOT_CSPACE, 30, Rights::READ);
    let range = Range {
        first: via(read_only, 40),
        count: 1,
    };
    k.no(recv(EP, RP, range), Error::Denied);
    k.no(call(EP, msg(1, vec![]), range), Error::Denied);
}

#[test]
fn address_of_an_empty_receive_range_is_not_looked_at() {
    let mut k = K::with_ipc();
    caller_with(&mut k, vec![], Range::NONE);
    let empty = Range {
        first: via(30, 40),
        count: 0,
    };
    assert_eq!(got(k.go(recv(EP, RP, empty))), (1, 0, 0, 0));
}

#[test]
fn recv_needs_recv_on_the_endpoint_and_on_the_reply_object() {
    let mut k = K::with_ipc();
    let endpoint = k.weak(EP, 30, caps_without(Rights::RECV));
    let reply = k.weak(RP, 31, caps_without(Rights::RECV));
    k.no(recv(endpoint, RP, Range::NONE), Error::Denied);
    k.no(recv(EP, reply, Range::NONE), Error::Denied);
    k.no(recv(NT, RP, Range::NONE), Error::Invalid);
    k.no(recv(EP, EP, Range::NONE), Error::Invalid);
}

#[test]
fn recv_is_busy_when_the_reply_object_is_armed() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
}

#[test]
fn recv_is_busy_when_the_reply_object_is_attached() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
    k.no(reply_recv(RP, msg(1, vec![]), EP), Error::Busy);
}

#[test]
fn reply_recv_answers_the_caller_then_receives() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(got(k.go(reply_recv(RP, msg(8, vec![]), EP))), (2, 0, 0, 0));
    assert_eq!(got(k.woken(first).unwrap()), (8, 0, 0, 0));
    assert_eq!(k.go(reply_recv(RP, msg(9, vec![]), EP)), WAITS);
    assert_eq!(got(k.woken(second).unwrap()), (9, 0, 0, 0));
}

#[test]
fn reply_recv_drops_the_reply_without_an_error_when_the_object_is_not_armed() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.go(reply_recv(RP, msg(8, vec![]), EP))), (1, 0, 0, 0));
    assert_eq!(k.woken(caller), None);
}

#[test]
fn reply_recv_needs_send_and_recv_on_the_reply_object_and_recv_on_the_endpoint() {
    let mut k = K::with_ipc();
    let no_send = k.weak(RP, 30, caps_without(Rights::SEND));
    let no_recv = k.weak(RP, 31, caps_without(Rights::RECV));
    let endpoint = k.weak(EP, 32, caps_without(Rights::RECV));
    k.no(reply_recv(no_send, msg(1, vec![]), EP), Error::Denied);
    k.no(reply_recv(no_recv, msg(1, vec![]), EP), Error::Denied);
    k.no(reply_recv(RP, msg(1, vec![]), endpoint), Error::Denied);
    k.no(reply_recv(EP, msg(1, vec![]), EP), Error::Invalid);
    k.no(reply_recv(RP, msg(1, vec![]), RP), Error::Invalid);
}

#[test]
fn send_on_an_endpoint_is_delivered_to_a_thread_that_waits_to_receive() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.ok(derive(s(EP), s(30), Rights::SEND, 42));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(
        k.go(send(30, msg(5, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(server).unwrap()), (5, 42, 0, 0));
    // The reply object of the receiver is idle: nobody waits for an answer.
    k.no(send(RP, msg(6, vec![])), Error::WouldWait);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn send_never_queues() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.no(send(EP, msg(5, vec![])), Error::WouldWait);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn send_does_not_reach_a_caller_in_the_queue() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    k.no(send(EP, msg(5, vec![])), Error::WouldWait);
    assert_eq!(k.woken(caller), None);
}

#[test]
fn send_needs_send_on_an_endpoint_or_a_reply_object() {
    let mut k = K::with_ipc();
    let endpoint = k.weak(EP, 30, caps_without(Rights::SEND));
    let reply = k.weak(RP, 31, caps_without(Rights::SEND));
    k.no(send(endpoint, msg(1, vec![])), Error::Denied);
    k.no(send(reply, msg(1, vec![])), Error::Denied);
    k.no(send(NT, msg(1, vec![])), Error::Invalid);
    k.no(send(32, msg(1, vec![])), Error::Invalid);
}

#[test]
fn reply_object_becomes_idle_when_its_caller_disappears() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.ok(Syscall::CapDelete(s(40)));
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
    let server = k.spawn(41);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn caller_is_not_woken_when_the_server_thread_alone_is_destroyed() {
    let mut k = K::with_ipc();
    let (caller, server) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.on(server, recv(EP, RP, Range::NONE)));
    k.ok(Syscall::CapDelete(s(41)));
    assert!(!k.can_run(caller) && k.woken(caller).is_none());
    // Another thread can still answer through the reply object.
    assert_eq!(
        k.go(send(RP, msg(9, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, 0));
}

#[test]
fn destroyed_thread_leaves_its_queue() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    k.ok(Syscall::CapDelete(s(40)));
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (2, 0, 0, 0));
}

// Transfer of capabilities.

/// A caller in the queue of `EP` whose message lists `caps`.
fn caller_with(k: &mut K, caps: Vec<Transfer>, range: Range) -> ObjId {
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, caps), range)), WAITS);
    caller
}

#[test]
fn moved_capability_leaves_the_sender_for_the_first_slot_of_the_range() {
    let mut k = K::with_ipc();
    caller_with(&mut k, vec![Transfer::Move(s(NT))], Range::NONE);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 2)))), (1, 0, 1, 0));
    assert_eq!(k.kind(NT), Err(Error::Invalid));
    assert_eq!(k.inspect(s(50)), Ok((Notify, Rights::ALL, 0)));
    assert_eq!(k.kind(51), Err(Error::Invalid));
}

#[test]
fn derived_capability_is_a_child_of_the_sender_s_and_keeps_its_badge() {
    let mut k = K::with_ipc();
    k.ok(derive(s(NT), s(30), Rights::ALL, 7));
    let wanted = Rights::SEND.union(Rights::DERIVE);
    caller_with(&mut k, vec![Transfer::Derive(s(30), wanted)], Range::NONE);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 1)))), (1, 0, 1, 0));
    assert_eq!(k.inspect(s(50)), Ok((Notify, wanted, 7)));
    assert_eq!(k.kind(30), Ok(Notify));
    k.ok(Syscall::CapRevoke(s(30)));
    assert_eq!(k.kind(50), Err(Error::Gone));
}

#[test]
fn capabilities_land_in_a_range_of_another_capability_space() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    caller_with(&mut k, vec![Transfer::Move(s(NT))], Range::NONE);
    let range = Range {
        first: via(30, 9),
        count: 1,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 1, 0));
    assert_eq!(k.inspect(via(30, 9)).unwrap().0, Notify);
}

#[test]
fn message_with_capabilities_needs_grant() {
    let mut k = K::with_ipc();
    let endpoint = k.weak(EP, 30, caps_without(Rights::GRANT));
    let reply = k.weak(RP, 31, caps_without(Rights::GRANT));
    let caps = || msg(1, vec![Transfer::Move(s(NT))]);
    k.no(call(endpoint, caps(), Range::NONE), Error::Denied);
    k.no(send(endpoint, caps()), Error::Denied);
    k.no(send(reply, caps()), Error::Denied);
    k.no(reply_recv(reply, caps(), EP), Error::Denied);
    k.no(send(endpoint, msg(1, vec![])), Error::WouldWait);
}

#[test]
fn listed_slots_are_checked_when_the_call_is_made_and_nothing_is_sent() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, range(50, 1))), WAITS);
    let no_derive = k.weak(NT, 30, caps_without(Rights::DERIVE));
    let weak = k.weak(NT, 31, Rights::DERIVE);
    k.weak(31, 32, Rights::NONE);
    k.ok(Syscall::CapRevoke(s(31)));
    for (transfer, error) in [
        (Transfer::Move(s(33)), Error::Invalid),
        (Transfer::Move(s(32)), Error::Gone),
        (Transfer::Derive(s(no_derive), Rights::NONE), Error::Denied),
        (Transfer::Derive(s(weak), Rights::SEND), Error::Denied),
    ] {
        assert_eq!(k.go(send(EP, msg(1, vec![transfer]))), Err(error));
        assert_eq!(
            k.go(call(EP, msg(1, vec![transfer]), Range::NONE)),
            Err(error)
        );
        assert_eq!(k.woken(server), None);
    }
}

#[test]
fn moving_a_capability_through_a_cspace_capability_needs_write() {
    let mut k = K::with_ipc();
    let read_only = k.weak(START_SLOT_CSPACE, 30, Rights::READ);
    let moved = msg(1, vec![Transfer::Move(via(read_only, NT))]);
    let derived = msg(1, vec![Transfer::Derive(via(read_only, NT), Rights::NONE)]);
    k.no(send(EP, moved), Error::Denied);
    k.no(send(EP, derived), Error::WouldWait);
}

#[test]
fn empty_range_refuses_every_capability_and_the_words_still_arrive() {
    let mut k = K::with_ipc();
    let caller = caller_with(&mut k, vec![Transfer::Move(s(NT))], Range::NONE);
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(NT), Ok(Notify));
    // The caller is told in the header of the reply.
    k.go(send(RP, msg(9, vec![]))).unwrap();
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, DROPPED));
}

#[test]
fn send_returns_the_dropped_flag_at_once() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    let sent = k.go(send(EP, msg(5, vec![Transfer::Move(s(NT))])));
    assert_eq!(sent, Ok(Output::Sent { flags: DROPPED }));
    assert_eq!(got(k.woken(server).unwrap()), (5, 0, 0, DROPPED));
}

#[test]
fn transfer_is_all_or_nothing() {
    let mut k = K::with_ipc();
    k.make(Notify, 30);
    let caps = vec![Transfer::Move(s(NT)), Transfer::Move(s(30))];
    caller_with(&mut k, caps, Range::NONE);
    // Slot 19 is empty, slot 20 is not.
    assert_eq!(got(k.go(recv(EP, RP, range(19, 2)))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(19), Err(Error::Invalid));
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn transfer_is_dropped_when_the_range_is_shorter_than_the_list() {
    let mut k = K::with_ipc();
    k.make(Notify, 30);
    let caps = vec![Transfer::Move(s(NT)), Transfer::Move(s(30))];
    caller_with(&mut k, caps, Range::NONE);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 1)))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(50), Err(Error::Invalid));
}

#[test]
fn transfer_is_dropped_when_the_range_is_slot_0_or_out_of_the_cspace() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Reply, 30);
    let caps = || msg(1, vec![Transfer::Derive(s(NT), Rights::NONE)]);
    assert_eq!(k.on(first, call(EP, caps(), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, caps(), Range::NONE)), WAITS);
    assert_eq!(got(k.go(recv(EP, RP, range(0, 1)))), (1, 0, 0, DROPPED));
    assert_eq!(got(k.go(recv(EP, 30, range(128, 1)))), (1, 0, 0, DROPPED));
}

#[test]
fn transfer_is_dropped_when_a_listed_capability_no_longer_exists_at_delivery() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    caller_with(&mut k, vec![Transfer::Move(s(30))], Range::NONE);
    k.ok(Syscall::CapRevoke(s(NT)));
    assert_eq!(got(k.go(recv(EP, RP, range(50, 1)))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(50), Err(Error::Invalid));
}

#[test]
fn reply_carries_capabilities_into_the_range_given_with_call() {
    let mut k = K::with_ipc();
    let caller = caller_with(&mut k, vec![], range(50, 1));
    got(k.go(recv(EP, RP, Range::NONE)));
    let sent = k.go(send(RP, msg(9, vec![Transfer::Move(s(NT))])));
    assert_eq!(sent, Ok(Output::Sent { flags: 0 }));
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 1, 0));
    assert_eq!(k.kind(50), Ok(Notify));
}

#[test]
fn reply_whose_capabilities_are_dropped_tells_both_sides() {
    let mut k = K::with_ipc();
    let caller = caller_with(&mut k, vec![], Range::NONE);
    got(k.go(recv(EP, RP, Range::NONE)));
    let sent = k.go(send(RP, msg(9, vec![Transfer::Move(s(NT))])));
    assert_eq!(sent, Ok(Output::Sent { flags: DROPPED }));
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, DROPPED));
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn slot_listed_twice_is_invalid() {
    let mut k = K::with_ipc();
    let same = via(START_SLOT_CSPACE, NT);
    let twice = msg(
        1,
        vec![Transfer::Move(s(NT)), Transfer::Derive(same, Rights::NONE)],
    );
    k.no(send(EP, twice.clone()), Error::Invalid);
    k.no(call(EP, twice.clone(), Range::NONE), Error::Invalid);
    k.no(reply_recv(RP, twice, EP), Error::Invalid);
}

#[test]
fn delivery_transfers_what_the_listed_slot_holds_then() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    caller_with(&mut k, vec![Transfer::Move(s(30))], Range::NONE);
    k.ok(Syscall::CapDelete(s(30)));
    k.weak(RP, 30, Rights::ALL);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 1)))), (1, 0, 1, 0));
    assert_eq!(k.kind(50), Ok(Reply));
}

#[test]
fn transfer_is_dropped_when_the_listed_slot_fails_a_check_at_delivery() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    caller_with(
        &mut k,
        vec![Transfer::Derive(s(30), Rights::SEND)],
        Range::NONE,
    );
    k.ok(Syscall::CapDelete(s(30)));
    k.weak(NT, 30, Rights::SEND);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 1)))), (1, 0, 0, DROPPED));
    assert_eq!(k.kind(50), Err(Error::Invalid));
}

// Notifications.

#[test]
fn signal_sets_bit_0_or_the_bits_of_the_badge() {
    let mut k = K::with_ipc();
    k.ok(derive(s(NT), s(30), Rights::SEND, 0b110));
    k.ok(Syscall::Signal(s(NT)));
    k.ok(Syscall::Signal(s(30)));
    k.ok(Syscall::Signal(s(30)));
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(0b111)));
}

#[test]
fn wait_returns_the_word_and_clears_it() {
    let mut k = K::with_ipc();
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.go(Syscall::Wait(s(NT))), Ok(Output::Word(1)));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn wait_waits_when_the_word_is_zero_and_poll_would_wait() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Poll(s(NT))), Err(Error::WouldWait));
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT))), WAITS);
    assert!(!k.can_run(waiter));
}

#[test]
fn signal_wakes_one_thread_with_the_whole_word_and_clears_it() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.ok(derive(s(NT), s(30), Rights::SEND, 0b110));
    assert_eq!(k.on(first, Syscall::Wait(s(NT))), WAITS);
    assert_eq!(k.on(second, Syscall::Wait(s(NT))), WAITS);
    k.ok(Syscall::Signal(s(30)));
    assert_eq!(k.woken(first), Some(Ok(Output::Word(0b110))));
    assert_eq!(k.woken(second), None);
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn signal_needs_send_and_wait_and_poll_need_recv() {
    let mut k = K::with_ipc();
    let no_send = k.weak(NT, 30, caps_without(Rights::SEND));
    let no_recv = k.weak(NT, 31, caps_without(Rights::RECV));
    k.no(Syscall::Signal(s(no_send)), Error::Denied);
    k.no(Syscall::Wait(s(no_recv)), Error::Denied);
    k.no(Syscall::Poll(s(no_recv)), Error::Denied);
}

#[test]
fn signal_wait_and_poll_take_a_notify_only() {
    let mut k = K::with_ipc();
    k.no(Syscall::Signal(s(EP)), Error::Invalid);
    k.no(Syscall::Wait(s(EP)), Error::Invalid);
    k.no(Syscall::Poll(s(EP)), Error::Invalid);
}

#[test]
fn recv_returns_a_pending_notification_of_the_bound_notify_before_any_caller() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.ok(bind(NT, s(EP)));
    k.ok(derive(s(NT), s(30), Rights::SEND, 0b100));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(30)));
    k.ok(Syscall::Signal(s(NT)));
    let notification = got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(notification, (0, 0b101, 0, Header::FLAG_NOTIFICATION));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (1, 0, 0, 0));
}

#[test]
fn reply_recv_returns_a_pending_notification_of_the_bound_notify() {
    let mut k = K::with_ipc();
    k.ok(bind(NT, s(EP)));
    k.ok(Syscall::Signal(s(NT)));
    let notification = got(k.go(reply_recv(RP, msg(9, vec![]), EP)));
    assert_eq!(notification, (0, 1, 0, Header::FLAG_NOTIFICATION));
}

#[test]
fn signal_on_a_bound_notify_wakes_a_thread_that_waits_to_receive() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    let notification = got(k.woken(server).unwrap());
    assert_eq!(notification, (0, 1, 0, Header::FLAG_NOTIFICATION));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    // Its reply object is idle again.
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn signal_on_a_bound_notify_prefers_a_thread_that_waits_on_the_notify() {
    let mut k = K::with_ipc();
    let (server, waiter) = (k.spawn(40), k.spawn(41));
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT))), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
    assert_eq!(k.woken(server), None);
}

#[test]
fn signal_on_an_unbound_notify_leaves_receivers_alone() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(server), None);
}

#[test]
fn notify_bind_needs_recv_on_the_notify_and_manage_on_the_endpoint() {
    let mut k = K::with_ipc();
    let notify = k.weak(NT, 30, caps_without(Rights::RECV));
    let endpoint = k.weak(EP, 31, caps_without(Rights::MANAGE));
    k.no(bind(notify, s(EP)), Error::Denied);
    k.no(bind(NT, s(endpoint)), Error::Denied);
    k.no(bind(EP, s(EP)), Error::Invalid);
    k.no(bind(NT, s(NT)), Error::Invalid);
}

#[test]
fn notify_bind_is_busy_when_either_side_is_already_bound() {
    let mut k = K::with_ipc();
    k.make(Endpoint, 30);
    k.make(Notify, 31);
    k.ok(bind(NT, s(EP)));
    k.no(bind(NT, s(30)), Error::Busy);
    k.no(bind(31, s(EP)), Error::Busy);
    k.ok(bind(31, s(30)));
}

#[test]
fn notify_bind_with_endpoint_0_removes_the_binding() {
    let mut k = K::with_ipc();
    k.make(Notify, 31);
    k.ok(bind(NT, SlotAddr::NONE));
    k.ok(bind(NT, s(EP)));
    k.ok(bind(NT, SlotAddr::NONE));
    k.ok(bind(31, s(EP)));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.go(recv(EP, RP, Range::NONE)).map(drop), Ok(()));
    assert!(!k.can_run(k.t));
}

#[test]
fn binding_ends_when_either_object_is_destroyed() {
    let mut k = K::with_ipc();
    k.make(Endpoint, 30);
    k.make(Notify, 31);
    k.ok(bind(NT, s(EP)));
    k.ok(Syscall::CapDelete(s(EP)));
    k.ok(bind(NT, s(30)));
    k.ok(Syscall::CapDelete(s(NT)));
    k.ok(bind(31, s(30)));
}

#[test]
fn binding_delivers_a_pending_word_at_once_to_a_thread_that_waits_to_receive() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(server), None);
    k.ok(bind(NT, s(EP)));
    let notification = got(k.woken(server).unwrap());
    assert_eq!(notification, (0, 1, 0, Header::FLAG_NOTIFICATION));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn binding_delivers_nothing_when_the_word_is_zero() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.woken(server), None);
}

// The model itself.

#[test]
#[should_panic(expected = "cannot run")]
fn apply_refuses_a_thread_that_cannot_run() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT))), WAITS);
    let _ = k.on(waiter, Syscall::Poll(s(NT)));
}

#[test]
fn outcome_of_a_wait_is_kept_until_the_next_call_of_the_thread() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT))), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
    assert_eq!(k.on(waiter, Syscall::Poll(s(NT))), Err(Error::WouldWait));
    assert_eq!(k.woken(waiter), None);
}
