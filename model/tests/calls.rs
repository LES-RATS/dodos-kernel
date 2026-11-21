//! One test per behaviour of the call reference. Every call goes through
//! `K::on`, which checks the invariants after it.

mod common;

use dodos_abi::ObjectKind::{ASpace, CSpace, Endpoint, Frame, Notify, Reply, Thread, Untyped};
use dodos_abi::*;
use dodos_model::*;

const SPACE: SlotAddr = SlotAddr::own(START_SLOT_CSPACE);
const UT: SlotAddr = s(START_SLOT_FIRST);
const DEV: SlotAddr = s(START_SLOT_FIRST + 1);
/// The table untyped of the table floor, at depth 1 like `UT`: a root that
/// no capability comes from (its pieces are tables, not capabilities), so
/// one with no descendant.
const START_UT: SlotAddr = SlotAddr::own(common::TABLES);
/// The `core` capability of the boot core.
const CORE: SlotAddr = SlotAddr::own(16);
/// The first slot beyond the table of the start-up space of the machine of
/// the tests.
const BEYOND: SlotAddr = SlotAddr::own(4224);
const DROPPED: u32 = Header::FLAG_CAPS_DROPPED;
/// Every right but `exec`: the rights of a root made at start-up from no
/// untyped, and of a machine object.
const NOT_EXEC: Rights = Rights::ALL.without(Rights::EXEC);
// Slots that the tests use for the same thing.
const EP: u32 = 20;
const RP: u32 = 21;
const NT: u32 = 22;

/// The slot numbers of the tests, on the start-up space of
/// `Machine::example`: 16 is its untyped of one chunk (slot 25) and 17 the
/// block of its device range (taken into slot 64), and the slots from 18
/// are 64 further, after the start-up capabilities and the device block.
/// The fixed slots below 16 are themselves.
const fn shifted(index: u32) -> u32 {
    match index {
        0..16 => index,
        16 => common::UNTYPED,
        17 => common::DEVICE,
        _ => index + 64,
    }
}

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(shifted(index))
}

/// A slot of the caller's own space, for a thread whose space is not the
/// first one: its index as it is.
const fn o(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

/// A slot of another capability space, through the `cspace` capability in
/// a slot of the first one.
fn via(cspace: u32, index: u32) -> SlotAddr {
    SlotAddr::through(shifted(cspace), index)
}

fn retype(untyped: SlotAddr, kind: ObjectKind, size: u64, destination: SlotAddr) -> Syscall {
    Syscall::UntypedRetype {
        untyped,
        kind,
        size: common::sized(kind, size),
        page: PageSize::Page as u64,
        // Frames: write-back on ordinary memory, uncached on device memory.
        memory: (untyped == DEV) as u64,
        destination,
    }
}

fn frame(
    untyped: SlotAddr,
    page: PageSize,
    pages: u64,
    memory: MemoryType,
    destination: SlotAddr,
) -> Syscall {
    Syscall::UntypedRetype {
        untyped,
        kind: Frame,
        size: pages,
        page: page as u64,
        memory: memory as u64,
        destination,
    }
}

fn derive(source: SlotAddr, destination: SlotAddr, rights: Rights, badge: u64) -> Syscall {
    Syscall::CapDerive {
        source,
        destination,
        rights: rights.bits(),
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
        page: SlotAddr::NONE,
    }
}

/// The same configuration, with a message page.
fn with_page(mut configure: Syscall, slot: u32) -> Syscall {
    if let Syscall::ThreadConfigure { page, .. } = &mut configure {
        *page = s(slot);
    }
    configure
}

/// A message whose only word is its label.
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

fn call(endpoint: u32, message: Message, range: Range) -> Syscall {
    Syscall::Call {
        endpoint: s(endpoint),
        message,
        range,
        wait: 0,
    }
}

fn recv(endpoint: u32, reply: u32, range: Range) -> Syscall {
    Syscall::Recv {
        endpoint: s(endpoint),
        reply: s(reply),
        range,
        wait: 0,
    }
}

fn reply_recv(reply: u32, message: Message, endpoint: u32) -> Syscall {
    Syscall::ReplyRecv {
        reply: s(reply),
        message,
        endpoint: s(endpoint),
        range: Range::NONE,
        wait: 0,
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
    /// The next slot, counting down, where `spawn` places a message page.
    spare: u32,
}

impl K {
    /// The first thread's context keeps half of the core, so that the
    /// threads that `spawn` makes have contexts of their own.
    fn new() -> Self {
        let (mut m, t) = Model::boot(&Machine::example()).unwrap();
        common::take_device(&mut m, t);
        let mut k = Self { m, t, spare: 4000 };
        k.ok(Syscall::SchedConfigure {
            sched: SlotAddr::own(START_SLOT_SCHED),
            node: CORE,
            core: 0,
            budget: 5_000_000,
            period: 10_000_000,
            class: 1,
        });
        k
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
        // A creation clears its memory 64 KB per call, from the clean mark
        // to the end of the new object, padding included: it takes exactly
        // one call per step of clearing (rules of `untyped_retype`; Argon b4
        // of the seventh review). A child untyped and device memory are not
        // cleared, and a table untyped has no clean mark, the gate clearing
        // what it takes within the call (D-154; pass 35: the first test to
        // make an `its_table` through here found the count wrong): one call.
        if let Syscall::UntypedRetype { untyped, kind, .. } = call {
            let marks = self.m.untyped_marks(self.t, untyped);
            let table = self.m.untyped_is_table(self.t, untyped) == Some(true);
            // The invariants are checked once at the end: the journal of
            // writes is kept until that check.
            let mut calls = 0;
            loop {
                calls += 1;
                match self.m.apply(self.t, call.clone()) {
                    DONE => break,
                    Err(Error::Again) if calls < 40_000 => {}
                    other => panic!("{call:?}: {other:?}"),
                }
            }
            assert_eq!(self.m.check(), Ok(()));
            let (_, clean, device) = marks.expect("an untyped");
            let (end, _, _) = self.m.untyped_marks(self.t, untyped).expect("an untyped");
            let steps = match device || table || kind == Untyped {
                true => 1,
                false => clear_steps(clean, end),
            };
            assert_eq!(calls, steps, "{call:?}");
            return;
        }
        assert_eq!(self.go(call), DONE);
    }

    /// The first thread makes a call that fails.
    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    /// The first thread repeats a removal while it returns `again`, at most
    /// `calls` times in all, and this returns how many calls it took.
    #[track_caller]
    fn finish(&mut self, removal: Syscall, calls: u32) -> u32 {
        for made in 1..=calls {
            match self.go(removal.clone()) {
                DONE => return made,
                Err(Error::Again) => {}
                other => panic!("{removal:?}: {other:?}"),
            }
        }
        panic!("{removal:?} does not end in {calls} calls");
    }

    /// The first thread repeats a creation while it returns `again`, as it
    /// clears memory 64 KB per call, at most 40,000 times. The invariants
    /// are checked once, at the end.
    #[track_caller]
    fn made(&mut self, creation: Syscall) {
        self.made_in(creation, 40_000);
    }

    /// As `made`, within `calls` calls.
    fn made_in(&mut self, creation: Syscall, calls: u64) {
        for _ in 0..calls {
            match self.m.apply(self.t, creation.clone()) {
                DONE => return assert_eq!(self.m.check(), Ok(())),
                Err(Error::Again) => {}
                other => panic!("{creation:?}: {other:?}"),
            }
        }
        panic!("{creation:?} does not end");
    }

    /// A new object from the ordinary untyped, in a slot of the first thread.
    fn make(&mut self, kind: ObjectKind, slot: u32) {
        let size = match kind {
            Untyped => self.m.geometry().page(),
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
    /// A scheduling context of 10 µs every 10 ms, bound to the thread in
    /// `slot` as its own.
    fn schedule(&mut self, slot: u32) {
        let sched = self.spare;
        self.spare -= 1;
        self.make(ObjectKind::Sched, sched);
        self.ok(Syscall::SchedConfigure {
            sched: s(sched),
            node: CORE,
            core: 0,
            budget: 10_000,
            period: 10_000_000,
            class: 1,
        });
        self.ok(Syscall::SchedBind {
            sched: s(sched),
            thread: s(slot),
        });
    }

    /// A frame for a message page, in a slot that the tests leave free.
    fn page(&mut self) -> u32 {
        let page = self.spare;
        self.spare -= 1;
        self.make(Frame, page);
        page
    }

    /// It has a message page and a scheduling context of its own, and runs
    /// in the address space of the first thread.
    fn spawn(&mut self, slot: u32) -> ObjId {
        self.make(Thread, slot);
        let page = self.page();
        let aspace = SlotAddr::own(START_SLOT_ASPACE);
        self.ok(with_page(
            configure(slot, SPACE, aspace, SlotAddr::NONE),
            page,
        ));
        self.schedule(slot);
        self.ok(Syscall::ThreadResume(s(slot)));
        self.m.object_at(self.t, s(slot)).unwrap()
    }

    /// Values of the machine: the bytes of a page size.
    fn bytes(&self, size: PageSize) -> u64 {
        self.m.geometry().bytes(size).unwrap()
    }

    /// A block of `1 << log2` bytes of ordinary memory in `slot`.
    fn run(&mut self, log2: u32, slot: u32) {
        common::take_run(&mut self.m, self.t, log2, shifted(slot));
        assert_eq!(self.m.check(), Ok(()));
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
    assert_eq!(K::new().m, K::new().m);
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
    // The sweep reaches the thread itself in slot 3 within the first call.
    k.no(Syscall::CapDelete(SPACE), Error::Again);
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
    assert_eq!(k.inspect(BEYOND), Err(Error::Invalid));
    k.no(retype(UT, Endpoint, 0, BEYOND), Error::Invalid);
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
    assert_eq!(k.m.object_at(k.t, via(31, 5)), None);
    assert!(k.m.object_at(k.t, via(30, 5)).is_some());
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
    // `derive` is not `write`.
    k.weak(30, 32, Rights::READ.union(Rights::DERIVE));
    k.no(Syscall::CapDelete(via(32, 5)), Error::Denied);
    k.no(Syscall::CapRevoke(via(32, 5)), Error::Denied);
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
    k.no(grow_from(30, s(31)), Error::Invalid);
    k.no(grow_from(30, DEV), Error::Invalid);
    // Any capability for that untyped will do.
    let copy = k.weak(START_SLOT_FIRST, 32, Rights::WRITE);
    assert_eq!(k.go(grow_from(30, s(copy))), Ok(Output::Slots(128)));
}

#[test]
fn cspace_grow_takes_a_cspace_and_an_untyped() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.no(grow_from(30, s(30)), Error::Invalid);
    k.no(grow(START_SLOT_FIRST), Error::Invalid);
}

#[test]
fn capability_space_given_at_start_up_never_grows() {
    K::new().no(grow(START_SLOT_CSPACE), Error::Invalid);
}

#[test]
fn cspace_grow_is_no_memory_when_the_untyped_is_exhausted() {
    let mut k = K::new();
    // An untyped of 16 KB, or a page: its directory at 0, then one segment
    // of 8 KB per grow until the untyped is full.
    let bytes = k.m.geometry().page().max(16384);
    let segments = bytes / 8192;
    k.ok(retype(UT, Untyped, bytes, s(31)));
    k.ok(retype(s(31), CSpace, 128 * segments, s(30)));
    let from_child = grow_from(30, s(31));
    for grown in 1..segments {
        let slots = 128 * grown as u32;
        assert_eq!(k.go(from_child.clone()), Ok(Output::Slots(slots)));
    }
    k.no(from_child, Error::NoMemory);
}

#[test]
fn segments_go_back_to_the_untyped_when_the_cspace_is_destroyed() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    let bytes = 2 * page.max(8192);
    k.ok(retype(UT, Untyped, bytes, s(31)));
    k.ok(retype(s(31), CSpace, 128, s(30)));
    let from_child = grow_from(30, s(31));
    k.go(from_child).unwrap();
    // The directory at 0, the segment at 8 KB, directories of 8 KB to the
    // end.
    for slot in 0..(bytes - 16384) / 8192 {
        k.ok(retype(s(31), CSpace, 512 * 128, s(1000 + slot as u32)));
    }
    k.no(retype(s(31), Endpoint, 0, s(40)), Error::NoMemory);
    for slot in 0..(bytes - 16384) / 8192 {
        k.ok(Syscall::CapDelete(s(1000 + slot as u32)));
    }
    k.ok(Syscall::CapDelete(s(30)));
    // The watermark is back at zero: the whole untyped can be taken again.
    k.ok(retype(s(31), Untyped, bytes, s(40)));
}

/// Every right but `right` and `exec`: the start-up capabilities of the
/// tests have no `exec` (the final review's D3 and H3).
fn caps_without(right: Rights) -> Rights {
    NOT_EXEC.without(right)
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
    k.no(Syscall::Wait(s(no_right), 0), Error::Invalid);
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
    k.no(retype(UT, Untyped, 1 << 40, UT), Error::NoSlot);
    k.no(retype(UT, Untyped, 1 << 40, s(30)), Error::NoMemory);
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
    k.no(derive(s(EP), BEYOND, Rights::ALL, 0), Error::Invalid);
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
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, page, s(30)));
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
    assert_eq!(k.go(Syscall::Wait(s(NT), 0)), Ok(Output::Word(1)));
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
    k.no(Syscall::Wait(s(notify), 0), Error::Gone);
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
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
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
        assert_eq!(k.on(thread, Syscall::Wait(s(NT), 0)), WAITS);
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
    let page = k.page();
    k.ok(with_page(
        configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE),
        page,
    ));
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
    assert_eq!(k.on(inside, Syscall::CapInspect(o(20))), Err(Error::Gone));
    assert_eq!(k.on(inside, Syscall::Signal(o(20))), Err(Error::Gone));
    assert_eq!(
        k.on(inside, derive(o(20), o(30), Rights::ALL, 0)),
        Err(Error::Gone)
    );
    assert_eq!(k.on(inside, Syscall::Yield), Err(Error::Gone));
    // `cap_delete` works, on a full slot and on a dead one.
    assert_eq!(k.on(inside, Syscall::CapDelete(o(20))), DONE);
    assert_eq!(k.on(inside, Syscall::CapDelete(s(1))), DONE);
    assert_eq!(k.kind(60), Err(Error::Gone));
}

#[test]
fn thread_whose_capability_space_is_dying_deletes_its_own_slots_only() {
    let mut k = K::with_ipc();
    let inside = space_with_a_thread(&mut k);
    // In its slot 25, a capability for the first space, which is not dying.
    k.ok(derive(SPACE, via(30, 25), NOT_EXEC, 0));
    assert_eq!(
        k.on(
            inside,
            Syscall::CapDelete(SlotAddr::through(25, shifted(NT)))
        ),
        DONE
    );
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(
        k.on(
            inside,
            Syscall::CapDelete(SlotAddr::through(25, shifted(EP)))
        ),
        Err(Error::Gone)
    );
    assert_eq!(k.kind(EP), Ok(Endpoint));
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
    let sent = k.go(send(
        EP,
        msg(5, vec![Transfer::Derive(s(NT), Rights::ALL.bits())]),
    ));
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
    assert_eq!(k.on(other, Syscall::Yield), Err(Error::Gone));
    assert_eq!(k.on(other, Syscall::CapDelete(s(1))), Err(Error::Gone));
}

#[test]
fn revoking_the_capability_a_thread_used_does_not_disturb_its_wait() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    k.weak(NT, 30, Rights::RECV);
    assert_eq!(k.on(waiter, Syscall::Wait(s(30), 0)), WAITS);
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

/// A capability space at depth 3 in slot 34, made from the child untyped in
/// slot 33 (depth 2), with one segment.
fn deeper_space(k: &mut K) {
    k.ok(retype(UT, Untyped, 1 << 16, s(33)));
    k.ok(retype(s(33), CSpace, 128, s(34)));
    k.go(grow_from(34, s(33))).unwrap();
}

#[test]
fn untyped_capability_is_placed_only_in_a_space_that_is_not_deeper() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    // The space is at depth 2, the start-up untypeds at depth 1; a block,
    // a child of the memory_control, is at depth 2.
    k.no(mv(UT, via(30, 5)), Error::Invalid);
    k.ok(mv(DEV, via(30, 4)));
    // A child and a copy are at depth 2.
    k.make(Untyped, 31);
    k.weak(START_SLOT_FIRST, 32, Rights::ALL);
    k.ok(mv(s(31), via(30, 5)));
    k.ok(mv(s(32), via(30, 6)));
}

#[test]
fn untyped_capability_is_derived_or_created_only_in_a_space_that_is_not_deeper() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    deeper_space(&mut k);
    let page = k.m.geometry().page();
    k.ok(derive(UT, via(30, 5), Rights::ALL, 0));
    k.ok(retype(UT, Untyped, page, via(30, 6)));
    k.no(derive(UT, via(34, 5), Rights::ALL, 0), Error::Invalid);
    k.no(retype(UT, Untyped, page, via(34, 5)), Error::Invalid);
    k.ok(retype(s(33), Untyped, page, via(34, 5)));
    k.ok(retype(UT, Notify, 0, via(34, 6)));
}

#[test]
fn capability_of_another_kind_moves_into_any_capability_space() {
    let mut k = K::with_ipc();
    k.weak(START_SLOT_CSPACE, 31, NOT_EXEC);
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    // All three are older than the space; the second is not a root.
    k.ok(mv(s(START_SLOT_DEBUG), via(30, 5)));
    k.ok(mv(s(31), via(30, 6)));
    k.ok(mv(s(EP), via(30, 7)));
}

#[test]
fn root_capability_of_a_space_is_placed_only_in_a_space_less_deep() {
    let mut k = K::new();
    for slot in [30, 31] {
        k.make(CSpace, slot);
        k.go(grow(slot)).unwrap();
        k.weak(slot, slot + 5, Rights::ALL);
    }
    deeper_space(&mut k);
    k.weak(34, 39, Rights::ALL);
    // Two spaces at the same depth hold neither root.
    k.no(mv(s(30), via(36, 1)), Error::Invalid);
    k.no(mv(s(31), via(35, 1)), Error::Invalid);
    k.no(mv(s(30), via(39, 1)), Error::Invalid);
    k.ok(mv(s(34), via(35, 1)));
    // Deleting the space destroys the deeper one it holds. Two tables to
    // sweep: two calls.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(39), Err(Error::Gone));
}

#[test]
fn copy_of_a_space_capability_is_derived_into_any_space() {
    let mut k = K::new();
    for slot in [30, 31] {
        k.make(CSpace, slot);
        k.go(grow(slot)).unwrap();
    }
    k.ok(derive(s(30), via(31, 5), Rights::ALL, 0));
    k.ok(derive(SPACE, via(31, 6), NOT_EXEC, 0));
}

#[test]
fn space_is_created_only_in_a_space_less_deep() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.make(Untyped, 31);
    k.no(retype(UT, CSpace, 128, via(30, 5)), Error::Invalid);
    k.ok(retype(s(31), CSpace, 128, via(30, 5)));
}

#[test]
fn spaces_nested_at_the_same_depth_are_refused() {
    // The chain of the review: any number of spaces at depth 2, each one
    // holding the root capability of the next. The first link is refused.
    let mut k = K::new();
    for slot in 30..40 {
        k.make(CSpace, slot);
        k.go(grow(slot)).unwrap();
        k.weak(slot, slot + 20, Rights::ALL);
    }
    for slot in 31..40 {
        k.no(mv(s(slot), via(slot + 19, 1)), Error::Invalid);
    }
}

#[test]
fn spaces_nest_no_deeper_than_the_tree() {
    let mut k = K::new();
    // Slot 20: a space for the capabilities that are no longer needed here.
    k.ok(retype(UT, CSpace, 256, s(20)));
    k.go(grow(20)).unwrap();
    k.go(grow(20)).unwrap();
    // Slot 30: a copy of the untyped, one level deeper at each turn.
    // Slot 32, then 31: the innermost space, which gets the next one.
    k.weak(START_SLOT_FIRST, 30, Rights::ALL);
    k.ok(retype(s(30), CSpace, 128, s(32)));
    k.go(grow(32)).unwrap();
    let (mut innermost, mut spaces) = (32, 1);
    for turn in 0.. {
        k.ok(derive(s(30), s(33), Rights::ALL, 0));
        k.ok(mv(s(30), via(20, 1 + 2 * turn)));
        k.ok(mv(s(33), s(30)));
        if k.go(retype(s(30), CSpace, 128, via(innermost, 1))) != DONE {
            break;
        }
        spaces += 1;
        let grown = Syscall::CSpaceGrow {
            cspace: via(innermost, 1),
            untyped: UT,
        };
        k.go(grown).unwrap();
        // The root capability at depth 64 cannot be copied.
        if k.go(derive(via(innermost, 1), s(34), Rights::ALL, 0)) != DONE {
            break;
        }
        if innermost == 31 {
            k.ok(mv(s(31), via(20, 2 + 2 * turn)));
        }
        k.ok(mv(s(34), s(31)));
        innermost = 31;
    }
    // Root capabilities at depths 3 to 64.
    assert_eq!(spaces, 62);
    k.finish(Syscall::CapDelete(s(32)), 1000);
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn three_layers_of_spaces_create_spaces_and_hand_memory_down() {
    let mut k = K::new();
    // The first thread makes a space from a child untyped, and hands it a
    // child of that untyped.
    // The sizes of the reference machine, scaled to pages larger than 4 KB.
    let scale = k.m.geometry().page() / 4096;
    k.ok(retype(UT, Untyped, scale << 20, s(30)));
    k.ok(retype(s(30), CSpace, 128, s(31)));
    k.go(grow_from(31, s(30))).unwrap();
    k.ok(retype(s(30), Untyped, scale << 18, via(31, 10)));
    k.ok(retype(s(30), Thread, 0, s(32)));
    k.ok(configure(32, s(31), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(32)));
    let mut inside = k.m.object_at(k.t, s(32)).unwrap();
    // Twice, the thread of a space does the same from the untyped it got,
    // in slot 10 of its space.
    for size in [scale << 16, scale << 14] {
        assert_eq!(k.on(inside, retype(s(10), CSpace, 128, s(11))), DONE);
        assert_eq!(k.on(inside, grow_from(11, s(10))), Ok(Output::Slots(128)));
        assert_eq!(
            k.on(inside, retype(s(10), Untyped, size, via(11, 10))),
            DONE
        );
        assert_eq!(k.on(inside, retype(s(10), Thread, 0, s(12))), DONE);
        let none = SlotAddr::NONE;
        assert_eq!(k.on(inside, configure(12, s(11), none, none)), DONE);
        assert_eq!(k.on(inside, Syscall::ThreadResume(s(12))), DONE);
        inside = k.m.object_at(inside, s(12)).unwrap();
    }
    assert_eq!(k.on(inside, retype(s(10), Endpoint, 0, s(13))), DONE);
    // The first thread kept the parent, and takes everything back.
    k.finish(Syscall::CapRevoke(s(30)), 100);
    assert_eq!(k.m.runnable(), [k.t]);
    k.ok(retype(s(30), Untyped, scale << 20, s(40)));
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
fn capability_moved_by_ipc_into_a_space_that_cannot_hold_it_is_dropped() {
    let mut k = K::with_ipc();
    k.make(CSpace, 31);
    k.go(grow(31)).unwrap();
    // The table untyped, a root with no descendant: an untyped root that
    // has one does not move at all (D-131).
    caller_with(&mut k, vec![Transfer::Move(START_UT)], Range::NONE);
    let range = Range {
        first: via(31, 5),
        count: 1,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 0, DROPPED));
    assert_eq!(k.inspect(START_UT).unwrap().0, Untyped);
}

#[test]
fn capability_derived_by_ipc_into_a_space_that_cannot_hold_it_is_dropped() {
    let mut k = K::with_ipc();
    deeper_space(&mut k);
    caller_with(
        &mut k,
        vec![Transfer::Derive(UT, Rights::ALL.bits())],
        Range::NONE,
    );
    let range = Range {
        first: via(34, 5),
        count: 1,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 0, DROPPED));
}

#[test]
fn capability_derived_by_ipc_lands_in_a_space_that_is_not_deeper() {
    let mut k = K::with_ipc();
    k.make(CSpace, 31);
    k.go(grow(31)).unwrap();
    caller_with(
        &mut k,
        vec![Transfer::Derive(UT, Rights::ALL.bits())],
        Range::NONE,
    );
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
    let deep = msg(1, vec![Transfer::Derive(s(91), Rights::NONE.bits())]);
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
fn child_untyped_is_a_power_of_two_of_at_least_one_page() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    for size in [0, page / 2, page + 1, 3 * page] {
        k.no(retype(UT, Untyped, size, s(30)), Error::Invalid);
    }
    k.ok(retype(UT, Untyped, page, s(30)));
    k.ok(retype(UT, Untyped, 1 << 20, s(31)));
}

#[test]
fn frame_takes_a_page_size_and_a_number_of_pages() {
    let mut k = K::new();
    let chunk = k.m.geometry().chunk_shift();
    let entries = u64::from(k.m.geometry().entries);
    k.run(chunk + 1, 30);
    k.no(retype(s(30), Frame, 0, s(31)), Error::Invalid);
    let wb = MemoryType::WriteBack;
    let large = |size, destination| frame(s(30), PageSize::Large1, size, wb, destination);
    k.made(large(2, s(31)));
    k.no(retype(s(30), Frame, 1, s(32)), Error::NoMemory);
    k.no(large(entries, s(32)), Error::NoMemory);
    // Frames: at most E pages (D-110).
    k.no(large(entries + 1, s(32)), Error::Invalid);
    k.no(large(u64::MAX, s(32)), Error::Invalid);
}

#[test]
fn frame_size_argument_is_judged_page_size_then_memory_type_then_pages() {
    let mut k = K::new();
    let raw = |untyped, page, memory, size| Syscall::UntypedRetype {
        untyped,
        kind: Frame,
        size,
        page,
        memory,
        destination: s(30),
    };
    // The page size value first, then whether the processor has it, then
    // the memory type, then the pages.
    k.no(raw(UT, 4, 1, 0), Error::Invalid);
    k.no(raw(UT, 3, 1, 0), Error::Unsupported);
    k.no(raw(UT, 0, 1, 1), Error::Invalid);
    k.no(raw(UT, 0, 3, 1), Error::Invalid);
    k.no(raw(UT, 0, 0, 0), Error::Invalid);
    let entries = u64::from(k.m.geometry().entries);
    k.no(raw(UT, 0, 0, entries + 1), Error::Invalid);
    k.made(raw(UT, 0, 0, entries));
    k.ok(Syscall::CapDelete(s(30)));
    // Device memory is uncached or write-combining, never write-back.
    k.no(raw(DEV, 0, 0, 1), Error::Invalid);
    k.ok(raw(DEV, 0, 2, 1));
    k.ok(Syscall::CapDelete(s(30)));
    k.ok(raw(DEV, 0, 1, 1));
}

#[test]
fn cspace_maximum_is_a_whole_number_of_segments_up_to_2_to_the_20() {
    let mut k = K::new();
    for size in [0, 1, 127, 129, (1 << 20) + 128, 1 << 32] {
        k.no(retype(UT, CSpace, size, s(30)), Error::Invalid);
    }
    k.ok(retype(UT, CSpace, 384, s(30)));
    k.made(retype(UT, CSpace, 1 << 20, s(31)));
}

#[test]
fn size_argument_is_zero_for_the_other_kinds() {
    // A thread's size argument is its feature mask (tests/cores.rs); a
    // timer and a task take 0 too (the final review's H2). An aspace and an
    // iospace take the slot address of a table untyped (D-154): 0, or the
    // slot of an ordinary untyped, names none (the audit of lot 8b, F7:
    // every case here went through `sized`, which writes the table
    // untyped's slot for 0).
    let mut k = K::new();
    use ObjectKind::{IoSpace, Task, Timer};
    for kind in [
        ASpace,
        ObjectKind::Sched,
        Endpoint,
        Reply,
        Notify,
        Timer,
        Task,
        IoSpace,
    ] {
        for size in [1, 7, 4096, u64::MAX] {
            k.no(retype(UT, kind, size, s(30)), Error::Invalid);
        }
        if matches!(kind, ASpace | IoSpace) {
            for size in [0, UT.raw()] {
                let raw = Syscall::UntypedRetype {
                    untyped: UT,
                    kind,
                    size,
                    page: PageSize::Page as u64,
                    memory: 0,
                    destination: s(30),
                };
                k.no(raw, Error::Invalid);
            }
        }
        k.ok(retype(UT, kind, 0, s(30)));
        assert_eq!(k.kind(30), Ok(kind));
        k.ok(Syscall::CapDelete(s(30)));
    }
}

/// An `its_table` comes from a table untyped only, as 2^k pages with k at
/// most 4, and a table untyped makes only child table untypeds and
/// `its_table`s (D-154; the audit of lot 8b, F11: only the random test
/// reached these).
#[test]
fn an_its_table_comes_from_a_table_untyped_only_with_at_most_sixteen_pages() {
    let mut k = K::new();
    let its = ObjectKind::ItsTable;
    k.no(retype(UT, its, 0, s(30)), Error::Invalid);
    k.no(retype(DEV, its, 0, s(30)), Error::Invalid);
    let beyond = u64::from(ITS_TABLE_K_MAX) + 1;
    k.no(retype(START_UT, its, beyond, s(30)), Error::Invalid);
    for kind in [Frame, Endpoint, Notify, Thread] {
        let size = if kind == Frame { 1 } else { 0 };
        k.no(retype(START_UT, kind, size, s(30)), Error::Invalid);
    }
    k.ok(retype(START_UT, its, u64::from(ITS_TABLE_K_MAX), s(30)));
    assert_eq!(k.kind(30), Ok(its));
    k.ok(Syscall::CapDelete(s(30)));
}

#[test]
fn the_reserved_kind_is_unsupported() {
    let mut k = K::new();
    k.no(retype(UT, ObjectKind::Vcpu, 0, s(30)), Error::Unsupported);
}

#[test]
fn kinds_never_made_from_an_untyped_are_invalid() {
    let mut k = K::new();
    use ObjectKind::{Debug, IoPorts, Iommu, Irq, IrqControl, MemoryControl, Node, Role};
    for kind in [
        Irq,
        IrqControl,
        Iommu,
        MemoryControl,
        Node,
        Role,
        Debug,
        IoPorts,
    ] {
        k.no(retype(UT, kind, 0, s(30)), Error::Invalid);
    }
}

#[test]
fn timers_tasks_and_iospaces_are_made_from_an_untyped() {
    let mut k = K::new();
    use ObjectKind::{IoSpace, Task, Timer};
    for (kind, slot) in [(Timer, 30), (Task, 31), (IoSpace, 32)] {
        k.ok(retype(UT, kind, 0, s(slot)));
        assert_eq!(k.kind(slot), Ok(kind));
    }
}

#[test]
fn device_memory_becomes_only_a_frame_or_a_child_untyped() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.no(retype(DEV, Endpoint, 0, s(30)), Error::Invalid);
    k.no(retype(DEV, CSpace, 128, s(30)), Error::Invalid);
    k.ok(retype(DEV, Frame, 1, s(30)));
    k.ok(retype(DEV, Untyped, page, s(31)));
    k.no(retype(s(31), Endpoint, 0, s(32)), Error::Invalid);
    k.ok(frame(s(31), PageSize::Page, 1, MemoryType::Uncached, s(32)));
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
    let page = k.m.geometry().page();
    k.ok(retype(DEV, Frame, 1, s(30)));
    k.ok(retype(DEV, Untyped, page, s(31)));
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
    let page = k.m.geometry().page();
    // Two headers of address spaces, one page each (D-154), fill it.
    k.ok(retype(UT, Untyped, 2 * page, s(30)));
    k.ok(retype(s(30), ASpace, 0, s(31)));
    k.ok(retype(s(30), ASpace, 0, s(33)));
    k.no(retype(s(30), Endpoint, 0, s(32)), Error::NoMemory);
}

#[test]
fn memory_is_aligned_on_the_size_of_the_object() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, 4 * page, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    // The header of an address space, one page (D-154), is aligned on a
    // page: at one page, which leaves room for a directory of a page at
    // two and another header at three, and nothing after it.
    k.ok(retype(s(30), ASpace, 0, s(32)));
    k.ok(retype(s(30), CSpace, page / 16 * 128, s(34)));
    k.ok(retype(s(30), ASpace, 0, s(35)));
    k.no(retype(s(30), Endpoint, 0, s(33)), Error::NoMemory);
}

#[test]
fn child_untyped_is_aligned_on_its_size() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, 16 * page, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(retype(s(30), Untyped, 8 * page, s(32)));
    k.no(retype(s(30), Untyped, page, s(33)), Error::NoMemory);
}

#[test]
fn the_header_of_an_address_space_is_aligned_on_a_page() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, 4 * page, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    // The header of an address space is one page since D-154 (its root
    // table is a page of the table untyped): after an endpoint the first
    // lies at one page, not after the endpoint's bytes, so three fill the
    // untyped to its end and nothing fits after them, not even an endpoint.
    k.ok(retype(s(30), ASpace, 0, s(32)));
    k.ok(retype(s(30), ASpace, 0, s(33)));
    k.ok(retype(s(30), ASpace, 0, s(34)));
    k.no(retype(s(30), ASpace, 0, s(35)), Error::NoMemory);
    k.no(retype(s(30), Endpoint, 0, s(36)), Error::NoMemory);
}

#[test]
fn cspace_directory_is_rounded_up_to_a_power_of_two() {
    let mut k = K::new();
    // An untyped of a page, at least 8 KB, filled by directories of 257
    // segments: 4,112 bytes each (16 bytes a segment since D-129), rounded
    // up to 8 KB.
    let bytes = k.m.geometry().page().max(8192);
    k.ok(retype(UT, Untyped, bytes, s(30)));
    for slot in 0..bytes / 8192 {
        k.ok(retype(s(30), CSpace, 257 * 128, s(31 + slot as u32)));
    }
    k.no(retype(s(30), Endpoint, 0, s(29)), Error::NoMemory);
}

#[test]
fn frame_is_aligned_on_its_page_size() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, 4 * page, s(30)));
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.ok(retype(s(30), Frame, 3, s(32)));
    k.no(retype(s(30), Endpoint, 0, s(33)), Error::NoMemory);
}

#[test]
fn watermark_returns_to_zero_only_when_the_last_object_is_destroyed() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, 2 * page, s(30)));
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
    let inspected = Output::Inspect {
        kind: ASpace,
        rights: Rights::ALL,
        badge: 0,
    };
    assert_eq!(k.on(other, Syscall::CapInspect(s(31))), Ok(inspected));
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
    assert_eq!(k.on(first, Syscall::Wait(s(NT), 0)), WAITS);
    assert_eq!(k.on(second, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(first), Some(Ok(Output::Word(1))));
    assert_eq!(k.woken(second), None);
    assert!(!k.can_run(first));
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(first));
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
    let many = msg(
        1,
        vec![Transfer::Derive(s(NT), Rights::NONE.bits()); MAX_CAPS + 1],
    );
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
    let (first, second) = (k.spawn(40), k.spawn(41));
    // Slot 30 is empty: no capability space to go through.
    let empty = Range {
        first: via(30, 40),
        count: 0,
    };
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), empty)), WAITS);
    assert_eq!(got(k.go(recv(EP, RP, empty))), (1, 0, 0, 0));
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), empty)), WAITS);
    let answer = Syscall::ReplyRecv {
        reply: s(RP),
        message: msg(8, vec![]),
        endpoint: s(EP),
        range: empty,
        wait: 0,
    };
    assert_eq!(got(k.go(answer)), (2, 0, 0, 0));
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
    caller_with(
        &mut k,
        vec![Transfer::Derive(s(30), wanted.bits())],
        Range::NONE,
    );
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
        (
            Transfer::Derive(s(no_derive), Rights::NONE.bits()),
            Error::Denied,
        ),
        (
            Transfer::Derive(s(weak), Rights::SEND.bits()),
            Error::Denied,
        ),
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
    let derived = msg(
        1,
        vec![Transfer::Derive(via(read_only, NT), Rights::NONE.bits())],
    );
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
fn receive_range_with_slot_0_or_a_slot_beyond_the_table_is_invalid() {
    let mut k = K::with_ipc();
    let beyond = |first, count| Range {
        first: SlotAddr::own(first),
        count,
    };
    for range in [range(0, 1), beyond(4224, 1), beyond(4222, 3)] {
        k.no(recv(EP, RP, range), Error::Invalid);
        k.no(call(EP, msg(1, vec![]), range), Error::Invalid);
    }
    k.no(send(EP, msg(1, vec![])), Error::WouldWait);
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, range(126, 2))), WAITS);
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
    let same = SlotAddr::through(START_SLOT_CSPACE, shifted(NT));
    let twice = msg(
        1,
        vec![
            Transfer::Move(s(NT)),
            Transfer::Derive(same, Rights::NONE.bits()),
        ],
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
        vec![Transfer::Derive(s(30), Rights::SEND.bits())],
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
    assert_eq!(k.go(Syscall::Wait(s(NT), 0)), Ok(Output::Word(1)));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn wait_waits_when_the_word_is_zero_and_poll_would_wait() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Poll(s(NT))), Err(Error::WouldWait));
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    assert!(!k.can_run(waiter));
}

#[test]
fn signal_wakes_one_thread_with_the_whole_word_and_clears_it() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.ok(derive(s(NT), s(30), Rights::SEND, 0b110));
    assert_eq!(k.on(first, Syscall::Wait(s(NT), 0)), WAITS);
    assert_eq!(k.on(second, Syscall::Wait(s(NT), 0)), WAITS);
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
    k.no(Syscall::Wait(s(no_recv), 0), Error::Denied);
    k.no(Syscall::Poll(s(no_recv)), Error::Denied);
}

#[test]
fn signal_wait_and_poll_take_a_notify_only() {
    let mut k = K::with_ipc();
    k.no(Syscall::Signal(s(EP)), Error::Invalid);
    k.no(Syscall::Wait(s(EP), 0), Error::Invalid);
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
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
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
    // Also for the pair that is already bound.
    k.no(bind(NT, s(EP)), Error::Busy);
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
    assert_eq!(k.go(recv(EP, RP, Range::NONE)), WAITS);
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
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    let _ = k.on(waiter, Syscall::Poll(s(NT)));
}

#[test]
fn outcome_of_a_wait_is_kept_until_the_next_call_of_the_thread() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
    assert_eq!(k.on(waiter, Syscall::Poll(s(NT))), Err(Error::WouldWait));
    assert_eq!(k.woken(waiter), None);
}

// Behaviours that the reviews of the model found untested.

#[test]
fn cap_derive_checks_the_destination_before_the_rights_asked_for() {
    let mut k = K::with_ipc();
    let weak = k.weak(EP, 30, Rights::SEND.union(Rights::DERIVE));
    k.no(derive(s(weak), s(0), Rights::ALL, 0), Error::Invalid);
    k.no(derive(s(weak), s(31), Rights::ALL, 0), Error::Denied);
    // Without `derive`, the source fails first.
    let no_derive = k.weak(EP, 31, Rights::SEND);
    k.no(derive(s(no_derive), s(0), Rights::NONE, 0), Error::Denied);
}

#[test]
fn header_limits_are_checked_before_the_listed_slots() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    let many = msg(1, vec![Transfer::Move(s(30)); MAX_CAPS + 1]);
    k.no(send(EP, many.clone()), Error::Invalid);
    k.no(call(EP, many.clone(), Range::NONE), Error::Invalid);
    k.no(reply_recv(RP, many, EP), Error::Invalid);
    k.no(send(EP, msg(1, vec![Transfer::Move(s(30))])), Error::Gone);
}

#[test]
fn arguments_of_call_are_checked_in_the_order_of_the_table() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    let no_send = k.weak(EP, 31, caps_without(Rights::SEND));
    let read_only = k.weak(START_SLOT_CSPACE, 32, Rights::READ);
    let dead = || msg(1, vec![Transfer::Move(s(30))]);
    let denied = Range {
        first: via(read_only, 40),
        count: 5,
    };
    // The endpoint, then the listed slots, then the range, then its count.
    k.no(call(no_send, dead(), denied), Error::Denied);
    k.no(call(EP, dead(), denied), Error::Gone);
    k.no(call(EP, msg(1, vec![]), denied), Error::Denied);
    k.no(call(EP, msg(1, vec![]), range(40, 5)), Error::Invalid);
}

#[test]
fn listed_slot_is_checked_fully_before_the_next_one() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    let twice = vec![
        Transfer::Move(s(NT)),
        Transfer::Move(s(NT)),
        Transfer::Move(s(30)),
    ];
    k.no(send(EP, msg(1, twice)), Error::Invalid);
    let dead_first = vec![
        Transfer::Move(s(30)),
        Transfer::Move(s(NT)),
        Transfer::Move(s(NT)),
    ];
    k.no(send(EP, msg(1, dead_first)), Error::Gone);
}

#[test]
fn capability_arguments_are_checked_in_the_order_of_the_table() {
    let mut k = K::with_ipc();
    k.make(Thread, 30);
    k.make(ASpace, 31);
    k.make(CSpace, 32);
    let aspace = k.weak(31, 40, caps_without(Rights::MANAGE));
    let cspace = k.weak(32, 41, caps_without(Rights::MANAGE));
    let endpoint = k.weak(EP, 42, caps_without(Rights::RECV));
    let none = SlotAddr::NONE;
    // `thread_configure`: the cspace, the aspace, the fault endpoint.
    k.no(configure(30, s(NT), s(aspace), none), Error::Invalid);
    k.no(configure(30, s(cspace), s(NT), none), Error::Denied);
    k.no(configure(30, SPACE, s(aspace), s(NT)), Error::Denied);
    k.no(configure(30, SPACE, s(NT), s(aspace)), Error::Invalid);
    // `recv`, `notify_bind` and `cspace_grow`: the first, then the second.
    k.no(recv(endpoint, NT, Range::NONE), Error::Denied);
    k.no(recv(NT, endpoint, Range::NONE), Error::Invalid);
    k.no(bind(EP, s(cspace)), Error::Invalid);
    k.no(grow_from(cspace, s(NT)), Error::Denied);
    k.no(grow_from(NT, s(cspace)), Error::Invalid);
}

#[test]
fn thread_configure_is_busy_on_a_thread_that_waits() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.no(
        configure(40, SPACE, SlotAddr::NONE, SlotAddr::NONE),
        Error::Busy,
    );
    k.ok(Syscall::Signal(s(NT)));
    k.ok(configure(40, SPACE, SlotAddr::NONE, SlotAddr::NONE));
}

#[test]
fn thread_configure_binds_a_suspended_thread_to_another_capability_space() {
    let mut k = K::new();
    let other = k.spawn(40);
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 5)));
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(configure(40, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(40)));
    assert_eq!(k.on(other, Syscall::Signal(s(5))), DONE);
}

#[test]
fn thread_resume_is_busy_when_the_capability_space_of_the_thread_is_destroyed() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.make(Thread, 31);
    k.ok(configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::CapDelete(s(30)));
    k.no(Syscall::ThreadResume(s(31)), Error::Busy);
    k.ok(configure(31, SPACE, SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(31)));
}

#[test]
fn cap_delete_and_cap_revoke_need_no_right_on_the_capability() {
    let mut k = K::with_ipc();
    let no_right = k.weak(EP, 30, Rights::NONE);
    // Nothing was derived from it: there is nothing to revoke.
    k.ok(Syscall::CapRevoke(s(no_right)));
    k.ok(Syscall::CapDelete(s(no_right)));
    assert_eq!(k.kind(no_right), Err(Error::Invalid));
}

#[test]
fn capability_on_the_path_of_a_removal_keeps_working_until_it_is_removed() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    for slot in 40..80 {
        k.weak(30, slot, Rights::ALL);
    }
    k.no(Syscall::CapRevoke(s(EP)), Error::Again);
    assert_eq!(k.kind(30), Ok(Endpoint));
    k.no(send(30, msg(1, vec![])), Error::WouldWait);
    // The accepted limit: it can still be derived from.
    k.ok(derive(s(30), s(100), Rights::ALL, 0));
    k.ok(Syscall::CapRevoke(s(EP)));
    assert_eq!(k.kind(100), Err(Error::Gone));
}

#[test]
fn cap_revoke_on_the_root_capability_of_a_space_does_not_sweep_the_space() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 5)));
    k.weak(30, 31, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(30)));
    assert_eq!(k.kind(31), Err(Error::Gone));
    assert_eq!(k.inspect(via(30, 5)).unwrap().0, Notify);
}

#[test]
fn wake_of_the_step_that_marks_a_reply_object_is_not_counted() {
    let mut k = K::with_ipc();
    let server = k.spawn(70);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    // 31 copies and the capability: 32 removals, one call.
    for slot in 30..61 {
        k.weak(RP, slot, Rights::ALL);
    }
    k.ok(Syscall::CapDelete(s(RP)));
    assert_eq!(k.woken(server), Some(Err(Error::Gone)));
}

#[test]
fn reply_recv_transfers_capabilities_both_ways_and_is_not_told_of_a_drop() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Notify, 30);
    k.make(Notify, 31);
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), range(50, 1))), WAITS);
    let moves = msg(2, vec![Transfer::Move(s(30))]);
    assert_eq!(k.on(second, call(EP, moves, Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    let answer = |label, slot| Syscall::ReplyRecv {
        reply: s(RP),
        message: msg(label, vec![Transfer::Move(s(slot))]),
        endpoint: s(EP),
        range: range(52, 1),
        wait: 0,
    };
    // The reply takes slot 31 to the first caller; slot 30 arrives.
    assert_eq!(got(k.go(answer(8, 31))), (2, 0, 1, 0));
    assert_eq!(got(k.woken(first).unwrap()), (8, 0, 1, 0));
    assert_eq!((k.kind(50), k.kind(52)), (Ok(Notify), Ok(Notify)));
    // The second caller gave no range: the server only sees that it waits.
    assert_eq!(k.go(answer(9, 50)), WAITS);
    assert_eq!(got(k.woken(second).unwrap()), (9, 0, 0, DROPPED));
    let inspected = Output::Inspect {
        kind: Notify,
        rights: Rights::ALL,
        badge: 0,
    };
    assert_eq!(k.on(second, Syscall::CapInspect(s(50))), Ok(inspected));
}

#[test]
fn reply_header_has_the_dropped_flag_of_the_request_with_the_capabilities_of_the_reply() {
    let mut k = K::with_ipc();
    k.make(Notify, 30);
    let caller = caller_with(&mut k, vec![Transfer::Move(s(30))], range(50, 1));
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (1, 0, 0, DROPPED));
    k.go(send(RP, msg(9, vec![Transfer::Move(s(NT))]))).unwrap();
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 1, DROPPED));
}

#[test]
fn send_on_an_idle_reply_object_would_wait_and_moves_nothing() {
    let mut k = K::with_ipc();
    k.no(
        send(RP, msg(9, vec![Transfer::Move(s(NT))])),
        Error::WouldWait,
    );
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn large_frame_is_aligned_on_its_page_size() {
    let mut k = K::new();
    let chunk = k.m.geometry().chunk_shift();
    k.run(chunk + 1, 30);
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    let large = Syscall::UntypedRetype {
        untyped: s(30),
        kind: Frame,
        size: 1,
        page: PageSize::Large1 as u64,
        memory: 0,
        destination: s(32),
    };
    k.made(large);
    k.no(retype(s(30), Frame, 1, s(33)), Error::NoMemory);
}

#[test]
fn binding_delivers_nothing_to_a_caller_in_the_queue() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.woken(caller), None);
    assert_eq!(
        got(k.go(recv(EP, RP, Range::NONE))).3,
        Header::FLAG_NOTIFICATION
    );
}

// The budget of a removal, and the second reviews of the model.

/// A capability space in slot 30 with `segments` segments, and how many
/// calls of `cap_delete` destroy it.
fn calls_to_delete_a_space(k: &mut K, segments: u64) -> u32 {
    k.ok(retype(UT, CSpace, 1 << 20, s(30)));
    for _ in 0..segments {
        k.go(grow(30)).unwrap();
    }
    k.ok(retype(UT, Notify, 0, via(30, segments as u32 * 128 - 1)));
    k.finish(Syscall::CapDelete(s(30)), 1000)
}

#[test]
fn removal_examines_at_most_128_slots_of_dying_spaces_per_call() {
    // 1,279 slots to examine after slot 0, one of them full: ten calls.
    assert_eq!(calls_to_delete_a_space(&mut K::new(), 10), 10);
    // 255 slots: 128, then 127.
    assert_eq!(calls_to_delete_a_space(&mut K::new(), 2), 2);
    // 16,383 slots: 128 calls, where 129 slots per call would take 127.
    assert_eq!(calls_to_delete_a_space(&mut K::new(), 128), 128);
}

#[test]
fn segments_that_were_never_added_are_not_examined() {
    // A maximum of 2^20 slots, one segment: 127 slots to examine.
    assert_eq!(calls_to_delete_a_space(&mut K::new(), 1), 1);
}

#[test]
fn slot_examined_once_is_not_examined_again() {
    let mut k = K::new();
    k.ok(retype(UT, CSpace, 256, s(30)));
    k.go(grow(30)).unwrap();
    k.go(grow(30)).unwrap();
    // Slots 200 to 239 hold a notify each: 199 empty slots before them.
    for index in 200..240 {
        k.ok(retype(UT, Notify, 0, via(30, index)));
    }
    // 128 slots; then 71 slots and 32 notifies; then 8 notifies, the rest
    // of the table and the space.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.ok(Syscall::CapDelete(s(30)));
}

#[test]
fn header_with_a_flag_set_by_the_sender_is_invalid() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    let mut flagged = msg(1, vec![]);
    flagged.flags = Header::FLAG_CAPS_DROPPED;
    k.no(send(EP, flagged.clone()), Error::Invalid);
    k.no(call(EP, flagged.clone(), Range::NONE), Error::Invalid);
    k.make(Reply, 30);
    k.no(reply_recv(30, flagged, EP), Error::Invalid);
    assert_eq!(k.woken(server), None);
}

#[test]
fn header_is_read_right_after_the_capability_the_call_acts_on() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    let no_send = k.weak(RP, 31, caps_without(Rights::SEND));
    let mut long = msg(1, vec![Transfer::Move(s(30))]);
    long.words = vec![0; MAX_WORDS + 1];
    // After the reply object, before the listed slot and the endpoint,
    // which are both dead.
    k.no(reply_recv(no_send, long.clone(), 30), Error::Denied);
    k.no(reply_recv(RP, long.clone(), 30), Error::Invalid);
    k.no(
        reply_recv(RP, msg(1, vec![Transfer::Move(s(30))]), EP),
        Error::Gone,
    );
    k.no(reply_recv(RP, msg(1, vec![]), 30), Error::Gone);
    // The same in `call` and `send`.
    k.no(call(EP, long.clone(), range(0, 1)), Error::Invalid);
    k.no(send(EP, long), Error::Invalid);
}

#[test]
fn arguments_of_reply_recv_are_checked_in_the_order_of_the_table() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.make(Reply, 30);
    assert_eq!(k.on(server, recv(EP, 30, Range::NONE)), WAITS);
    let read_only = k.weak(START_SLOT_CSPACE, 31, Rights::READ);
    let no_recv = k.weak(EP, 32, caps_without(Rights::RECV));
    let with = |endpoint, first, count| Syscall::ReplyRecv {
        reply: s(30),
        message: msg(1, vec![]),
        endpoint: s(endpoint),
        range: Range { first, count },
        wait: 0,
    };
    k.weak(NT, 33, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    // The endpoint (here a dead slot, then one without `recv`), the range,
    // the number of its slots, then the state: the reply object is attached
    // to another thread.
    k.no(with(33, via(read_only, 40), 5), Error::Gone);
    k.no(with(no_recv, s(40), 5), Error::Denied);
    k.no(with(EP, via(read_only, 40), 5), Error::Denied);
    k.no(with(EP, s(40), 5), Error::Invalid);
    k.no(with(EP, s(40), 1), Error::Busy);
}

#[test]
fn state_is_checked_after_every_argument() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    // `recv`: the range before the armed reply object.
    k.no(recv(EP, RP, range(40, 5)), Error::Invalid);
    k.no(recv(EP, RP, range(40, 1)), Error::Busy);
    // `thread_configure`: every capability before the thread that runs.
    k.no(configure(40, SPACE, SlotAddr::NONE, s(NT)), Error::Invalid);
    // `notify_bind`: the endpoint before the binding.
    k.ok(bind(NT, s(EP)));
    let no_manage = k.weak(EP, 30, caps_without(Rights::MANAGE));
    k.no(bind(NT, s(no_manage)), Error::Denied);
}

#[test]
fn cspace_grow_checks_the_untyped_then_the_maximum_then_the_memory() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    k.ok(retype(UT, Untyped, page.max(16384), s(31)));
    k.ok(retype(s(31), CSpace, 128, s(30)));
    k.go(grow_from(30, s(31))).unwrap();
    // At its maximum, and its untyped is exhausted.
    k.no(grow_from(30, UT), Error::Invalid);
    k.no(grow_from(30, s(31)), Error::NoSlot);
    // A space whose untyped runs out before its maximum.
    k.ok(retype(UT, Untyped, page, s(33)));
    k.ok(retype(s(33), CSpace, 2 * 128 * page.div_ceil(8192), s(32)));
    let segments = (page - 8192.min(page)) / 8192;
    for _ in 0..segments {
        k.go(grow_from(32, s(33))).unwrap();
    }
    k.no(grow_from(32, s(33)), Error::NoMemory);
}

#[test]
fn segment_is_aligned_on_8_kb() {
    let mut k = K::new();
    let page = k.m.geometry().page();
    let bytes = 4 * page.max(8192);
    k.ok(retype(UT, Untyped, bytes, s(31)));
    // The directory at 0, the segment of 8 KB at 8 KB, then directories of
    // 8 KB up to the end: nothing is left.
    k.ok(retype(s(31), CSpace, 128, s(30)));
    k.go(grow_from(30, s(31))).unwrap();
    for slot in 0..(bytes - 16384) / 8192 {
        k.ok(retype(s(31), CSpace, 512 * 128, s(1000 + slot as u32)));
    }
    k.no(retype(s(31), Endpoint, 0, s(34)), Error::NoMemory);
}

#[test]
fn untyped_retype_judges_the_kind_before_the_size() {
    let mut k = K::new();
    // Device memory: `invalid` for every kind but a frame and an untyped.
    k.no(retype(DEV, ObjectKind::IoPorts, 0, s(30)), Error::Invalid);
    k.no(retype(DEV, ObjectKind::Irq, 0, s(30)), Error::Invalid);
    k.no(retype(DEV, Endpoint, 1, s(30)), Error::Invalid);
    // A kind this version does not make, then the size.
    k.no(retype(UT, ObjectKind::Vcpu, 5, s(30)), Error::Unsupported);
    k.no(retype(UT, ObjectKind::IoPorts, 5, s(30)), Error::Invalid);
    k.no(retype(UT, ObjectKind::Irq, 5, s(30)), Error::Invalid);
    k.no(retype(UT, Endpoint, 5, s(30)), Error::Invalid);
    // All of it after the destination.
    k.no(retype(UT, ObjectKind::IoPorts, 5, s(0)), Error::Invalid);
}

#[test]
fn refused_derivation_or_creation_is_invalid_before_no_slot() {
    let mut k = K::new();
    deeper_space(&mut k);
    k.ok(retype(UT, Notify, 0, via(34, 5)));
    k.no(derive(UT, via(34, 5), Rights::ALL, 0), Error::Invalid);
    k.no(retype(UT, Untyped, 4096, via(34, 5)), Error::Invalid);
    k.no(retype(UT, Notify, 0, via(34, 5)), Error::NoSlot);
}

#[test]
fn plain_arguments_of_cap_derive_are_judged_in_the_order_of_the_table() {
    let mut k = K::with_ipc();
    // The rights before the badge: an untyped takes no badge.
    let weak = k.weak(START_SLOT_FIRST, 29, Rights::WRITE.union(Rights::DERIVE));
    k.no(derive(s(weak), s(100), Rights::ALL, 7), Error::Denied);
    k.no(derive(s(weak), s(100), Rights::WRITE, 7), Error::Invalid);
    // The badge before the depth: slot 91 is at depth 64, with badge 7.
    k.ok(derive(s(EP), s(30), Rights::ALL, 7));
    for slot in 31..92 {
        k.weak(slot - 1, slot, Rights::ALL);
    }
    k.no(derive(s(91), s(100), Rights::ALL, 8), Error::Denied);
    k.no(derive(s(91), s(100), Rights::ALL, 7), Error::Invalid);
}

#[test]
fn reply_lands_in_the_range_of_a_call_that_was_delivered_at_once() {
    let mut k = K::with_ipc();
    let (server, caller) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), range(50, 1))), WAITS);
    let answer = send(RP, msg(9, vec![Transfer::Move(s(NT))]));
    assert_eq!(k.on(server, answer), Ok(Output::Sent { flags: 0 }));
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 1, 0));
    assert_eq!(k.kind(50), Ok(Notify));
}

#[test]
fn thread_bound_to_a_dying_space_can_be_resumed() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    k.ok(Syscall::ThreadSuspend(s(31)));
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.ok(Syscall::ThreadResume(s(31)));
    assert!(k.can_run(inside));
}

#[test]
fn reply_recv_answers_and_returns_a_pending_notification_before_a_caller() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    let notification = got(k.go(reply_recv(RP, msg(8, vec![]), EP)));
    assert_eq!(notification, (0, 1, 0, Header::FLAG_NOTIFICATION));
    assert_eq!(got(k.woken(first).unwrap()), (8, 0, 0, 0));
    assert_eq!(k.woken(second), None);
    // The reply object is idle again.
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
}

#[test]
fn message_without_capabilities_reaches_a_dying_space_without_the_dropped_flag() {
    let mut k = K::with_ipc();
    space_with_a_thread(&mut k);
    let server = k.spawn(70);
    let range = Range {
        first: via(30, 25),
        count: 1,
    };
    assert_eq!(k.on(server, recv(EP, RP, range)), WAITS);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(
        k.go(send(EP, msg(5, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(server).unwrap()), (5, 0, 0, 0));
}

#[test]
fn capabilities_of_a_caller_whose_space_began_to_die_are_dropped_at_delivery() {
    let mut k = K::with_ipc();
    let inside = space_with_a_thread(&mut k);
    k.ok(derive(s(EP), via(30, 25), Rights::ALL, 0));
    let derived = msg(1, vec![Transfer::Derive(s(1), Rights::NONE.bits())]);
    let called = Syscall::Call {
        endpoint: o(25),
        message: derived,
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.on(inside, called), WAITS);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(got(k.go(recv(EP, RP, range(100, 1)))), (1, 0, 0, DROPPED));
}

#[test]
fn root_capability_of_a_space_moved_by_ipc_follows_the_depth_rule() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(70), k.spawn(71));
    for slot in [30, 31] {
        k.make(CSpace, slot);
        k.go(grow(slot)).unwrap();
    }
    deeper_space(&mut k);
    k.weak(31, 36, Rights::ALL);
    k.weak(34, 39, Rights::ALL);
    k.make(Reply, 80);
    let into = |space, index| Range {
        first: via(space, index),
        count: 1,
    };
    // The same depth: refused. Deeper: accepted.
    let same = msg(1, vec![Transfer::Move(s(30))]);
    assert_eq!(k.on(first, call(EP, same, Range::NONE)), WAITS);
    assert_eq!(got(k.go(recv(EP, RP, into(36, 5)))), (1, 0, 0, DROPPED));
    let deeper = msg(2, vec![Transfer::Move(s(34))]);
    assert_eq!(k.on(second, call(EP, deeper, Range::NONE)), WAITS);
    assert_eq!(got(k.go(recv(EP, 80, into(36, 5)))), (2, 0, 1, 0));
    assert_eq!(k.kind(34), Err(Error::Invalid));
    // A copy of a space capability goes anywhere, a deeper space too.
    let copy = msg(3, vec![Transfer::Derive(SPACE, Rights::NONE.bits())]);
    k.make(Endpoint, 81);
    k.make(Reply, 82);
    let third = k.spawn(72);
    assert_eq!(k.on(third, call(81, copy, Range::NONE)), WAITS);
    assert_eq!(got(k.go(recv(81, 82, into(39, 5)))), (3, 0, 1, 0));
}

#[test]
fn frame_of_the_second_large_page_is_aligned_on_its_size() {
    let mut k = K::new();
    // A block of one page of the second large page (the largest run).
    let log2 = k.bytes(PageSize::Large2).trailing_zeros();
    k.run(log2, 29);
    let huge = |size, slot| Syscall::UntypedRetype {
        untyped: s(29),
        kind: Frame,
        size,
        page: PageSize::Large2 as u64,
        memory: 0,
        destination: s(slot),
    };
    k.no(huge(0, 30), Error::Invalid);
    k.no(huge(1 << 34, 30), Error::Invalid);
    // A page fills the block, from dirty memory: one call per 64 KB.
    let calls = k.bytes(PageSize::Large2) / CLEAR_STEP_BYTES;
    k.made_in(huge(1, 31), calls);
    k.no(retype(s(29), Endpoint, 0, s(33)), Error::NoMemory);
    // An endpoint at 0 leaves no room for one.
    k.ok(Syscall::CapDelete(s(31)));
    k.ok(retype(s(29), Endpoint, 0, s(30)));
    k.no(huge(1, 31), Error::NoMemory);
}

#[test]
fn cap_move_from_a_dead_slot_is_gone() {
    let mut k = K::with_ipc();
    k.weak(EP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.no(mv(s(30), s(31)), Error::Gone);
}

#[test]
fn suspended_thread_that_waits_to_receive_still_gets_a_notification() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(got(k.woken(server).unwrap()).3, Header::FLAG_NOTIFICATION);
    assert!(!k.can_run(server));
}

#[test]
fn destroying_an_object_wakes_its_waiters_first_in_first_out() {
    let mut k = K::with_ipc();
    let threads: Vec<ObjId> = (40..73).map(|slot| k.spawn(slot)).collect();
    for &thread in &threads {
        assert_eq!(k.on(thread, Syscall::Wait(s(NT), 0)), WAITS);
    }
    // 32 wakes in the first call: all but the last to come.
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    assert_eq!(k.woken(threads[31]), Some(Err(Error::Gone)));
    assert_eq!(k.woken(threads[32]), None);
}

#[test]
fn longest_walk_that_the_depth_rule_allows_is_within_the_bound() {
    let mut k = K::new();
    // The start-up space holds the first untyped before anything else: the
    // capabilities of its slots 2 to 19 move to slots from 300.
    for from in (2..12).chain(16..20) {
        k.ok(mv(SlotAddr::own(from), SlotAddr::own(300 + from)));
    }
    // A space at each depth from 2 to 64, its root capability in slot 38 +
    // depth here. Each holds, in its slot 2, a copy of the untyped at its
    // own depth; the root capability of the next space is the most recent
    // child of that copy.
    k.weak(START_SLOT_FIRST, 33, Rights::ALL);
    k.ok(retype(UT, CSpace, 128, s(40)));
    k.go(grow(40)).unwrap();
    k.ok(mv(s(33), via(40, 2)));
    for space in 41..=102 {
        k.ok(derive(via(space - 1, 2), s(33), Rights::ALL, 0));
        k.ok(retype(via(space - 1, 2), CSpace, 128, s(space)));
        k.go(grow(space)).unwrap();
        k.ok(mv(s(33), via(space, 2)));
    }
    // The deepest space holds, in its slot 1, a capability at depth 1 and
    // then a copy of it at each depth down to 64.
    k.ok(mv(SlotAddr::own(300 + START_SLOT_DEBUG), via(102, 1)));
    k.ok(derive(via(102, 1), via(102, 3), NOT_EXEC, 0));
    for index in 3..65 {
        k.ok(derive(via(102, index), via(102, index + 1), NOT_EXEC, 0));
    }
    k.no(
        derive(via(102, 65), via(102, 66), NOT_EXEC, 0),
        Error::Invalid,
    );
    // The walk: the root capability of the start-up space, the untyped it
    // holds, 63 root capabilities, 62 copies of the untyped, then 64
    // ordinary capabilities. The contract says: shorter than 192.
    k.no(Syscall::CapDelete(SPACE), Error::Again);
    assert_eq!(k.m.longest_walk(), 1 + 1 + 63 + 62 + 64);
    assert_eq!(LONGEST_WALK, 191);
}

// The third reviews of the model: the rules that no test defended.

/// A message that lists `count` capabilities, moved and derived in turn,
/// from the notifies that this makes in slots 30 and after.
fn listing(k: &mut K, count: u32) -> Vec<Transfer> {
    let listed = |slot: u32| match slot % 2 {
        0 => Transfer::Move(s(slot)),
        _ => Transfer::Derive(s(slot), Rights::SEND.bits()),
    };
    (30..30 + count).for_each(|slot| k.make(Notify, slot));
    (30..30 + count).map(listed).collect()
}

#[test]
fn capabilities_of_one_message_each_land_in_their_own_slot() {
    for count in 2..=4 {
        let mut k = K::with_ipc();
        let caps = listing(&mut k, count);
        caller_with(&mut k, caps, Range::NONE);
        let delivered = got(k.go(recv(EP, RP, range(50, 4))));
        assert_eq!(delivered, (1, 0, count as usize, 0));
        for slot in 0..4 {
            let rights = [Rights::ALL, Rights::SEND][slot as usize % 2];
            let landed = Ok((Notify, rights, 0));
            let expected = if slot < count {
                landed
            } else {
                Err(Error::Invalid)
            };
            assert_eq!(k.inspect(s(50 + slot)), expected);
            // What was moved left its slot; what was derived stayed.
            let stays = slot % 2 == 1 && slot < count;
            assert_eq!(k.kind(30 + slot).is_ok(), stays, "slot {slot} of {count}");
        }
    }
}

#[test]
fn depth_rule_is_judged_on_each_capability_of_a_message() {
    let mut k = K::with_ipc();
    k.make(CSpace, 31);
    k.go(grow(31)).unwrap();
    // The notify can go anywhere; the table untyped is less deep than the
    // space (and has no descendant: one that has does not move, D-131).
    let caps = vec![
        Transfer::Derive(s(NT), Rights::ALL.bits()),
        Transfer::Move(START_UT),
    ];
    caller_with(&mut k, caps, Range::NONE);
    let range = Range {
        first: via(31, 5),
        count: 2,
    };
    assert_eq!(got(k.go(recv(EP, RP, range))), (1, 0, 0, DROPPED));
    assert_eq!(k.inspect(via(31, 5)), Err(Error::Invalid));
}

#[test]
fn reply_of_reply_recv_carries_badge_0() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.ok(derive(s(EP), s(30), Rights::ALL, 42));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(k.go(reply_recv(RP, msg(8, vec![]), 30)), WAITS);
    assert_eq!(got(k.woken(caller).unwrap()), (8, 0, 0, 0));
}

#[test]
fn reply_recv_answers_before_it_receives() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Notify, 30);
    k.make(Notify, 60);
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), range(50, 1))), WAITS);
    let moves = msg(2, vec![Transfer::Move(s(30))]);
    assert_eq!(k.on(second, call(EP, moves, Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    // The reply moves the capability out of slot 60, which then receives
    // the capability of the next request.
    let answer = Syscall::ReplyRecv {
        reply: s(RP),
        message: msg(8, vec![Transfer::Move(s(60))]),
        endpoint: s(EP),
        range: range(60, 1),
        wait: 0,
    };
    assert_eq!(got(k.go(answer)), (2, 0, 1, 0));
    assert_eq!(got(k.woken(first).unwrap()), (8, 0, 1, 0));
}

#[test]
fn reply_recv_checks_the_listed_slots_before_the_endpoint() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    let no_recv = k.weak(EP, 31, caps_without(Rights::RECV));
    let dead = msg(1, vec![Transfer::Move(s(30))]);
    k.no(reply_recv(RP, dead, no_recv), Error::Gone);
    k.no(reply_recv(RP, msg(1, vec![]), no_recv), Error::Denied);
}

#[test]
fn reply_recv_with_an_idle_reply_object_moves_no_capability() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    let moves = msg(8, vec![Transfer::Move(s(NT))]);
    assert_eq!(k.on(server, reply_recv(RP, moves, EP)), WAITS);
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn cap_delete_through_a_cspace_capability_leaves_the_slot_empty() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 5)));
    k.ok(Syscall::CapDelete(via(30, 5)));
    assert_eq!(k.inspect(via(30, 5)), Err(Error::Invalid));
    k.ok(retype(UT, Notify, 0, via(30, 5)));
}

#[test]
fn cap_delete_on_a_dead_slot_through_a_cspace_capability_needs_write() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(derive(s(EP), via(30, 5), Rights::ALL, 0));
    k.ok(Syscall::CapRevoke(s(EP)));
    let read_only = k.weak(30, 31, Rights::READ);
    k.no(Syscall::CapDelete(via(read_only, 5)), Error::Denied);
    assert_eq!(k.inspect(via(30, 5)), Err(Error::Gone));
    k.ok(Syscall::CapDelete(via(30, 5)));
    assert_eq!(k.inspect(via(30, 5)), Err(Error::Invalid));
}

#[test]
fn thread_in_a_dying_space_repeats_a_cap_delete_that_returned_again() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    for index in 60..93 {
        k.ok(derive(via(30, 20), via(30, index), Rights::ALL, 0));
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    // 33 copies and one in the first space: more than one call.
    assert_eq!(k.on(inside, Syscall::CapDelete(o(20))), Err(Error::Again));
    assert_eq!(k.on(inside, Syscall::CapDelete(o(20))), DONE);
}

#[test]
fn slot_of_the_root_capability_of_a_dying_space_counts_as_examined() {
    let mut k = K::new();
    k.ok(retype(UT, CSpace, 256, s(30)));
    k.go(grow(30)).unwrap();
    k.go(grow(30)).unwrap();
    // Slots 1 to 128, the root's slot 5 among them, are the 128 the first
    // call examines; a capability in slot 129 would still be found and
    // removed without examining a slot, so the notify is in slot 130 (the
    // final review's D5).
    k.ok(retype(UT, Notify, 0, via(30, 130)));
    k.ok(derive(via(30, 130), s(40), Rights::ALL, 0));
    k.weak(30, 31, Rights::ALL);
    k.ok(mv(s(30), via(31, 5)));
    k.no(Syscall::CapDelete(via(31, 5)), Error::Again);
    // The first call examined 128 slots, its own among them, and stopped
    // before the notify.
    assert_eq!(k.kind(40), Ok(Notify));
}

#[test]
fn sweep_goes_on_past_the_root_capability_of_the_space() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Notify, 0, via(30, 9)));
    k.ok(derive(via(30, 9), s(40), Rights::ALL, 0));
    k.weak(30, 31, Rights::ALL);
    k.ok(mv(s(30), via(31, 5)));
    k.ok(Syscall::CapDelete(via(31, 5)));
    assert_eq!(k.kind(40), Err(Error::Gone));
    assert_eq!(k.kind(31), Err(Error::Gone));
}

#[test]
fn rights_of_a_capability_to_derive_are_judged_before_its_depth() {
    let mut k = K::with_ipc();
    for slot in 30..91 {
        k.weak(
            slot - if slot == 30 { 30 - NT } else { 1 },
            slot,
            Rights::ALL,
        );
    }
    // Slot 91: at depth 64, without every right.
    let deep = k.weak(90, 91, Rights::SEND.union(Rights::DERIVE));
    let derived = |rights: Rights| msg(1, vec![Transfer::Derive(s(deep), rights.bits())]);
    k.no(send(EP, derived(Rights::ALL)), Error::Denied);
    k.no(send(EP, derived(Rights::SEND)), Error::Invalid);
}

/// Object sizes: a thread of mask 0 on the reference machine is 1 KB +
/// 832 bytes of x87, SSE and AVX state, rounded up to 2 KB.
#[test]
fn thread_takes_2_kb() {
    let mut k = K::new();
    k.make(Untyped, 30);
    let fit = (k.m.geometry().page() / 2048) as u32;
    for slot in 0..fit {
        k.ok(retype(s(30), Thread, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), Thread, 0, s(999)), Error::NoMemory);
}

#[test]
fn thread_is_aligned_on_2_kb() {
    let mut k = K::new();
    k.make(Untyped, 30);
    k.ok(retype(s(30), Endpoint, 0, s(40)));
    let fit = (k.m.geometry().page() / 2048) as u32 - 1;
    for slot in 0..fit {
        k.ok(retype(s(30), Thread, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), Endpoint, 0, s(42)), Error::NoMemory);
}

#[test]
fn sched_takes_256_bytes() {
    let mut k = K::new();
    k.make(Untyped, 30);
    let fit = (k.m.geometry().page() / 256) as u32;
    for slot in 0..fit {
        k.ok(retype(s(30), ObjectKind::Sched, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), ObjectKind::Sched, 0, s(999)), Error::NoMemory);
}

#[test]
fn endpoint_reply_and_notify_take_64_bytes() {
    let mut k = K::new();
    k.make(Untyped, 30);
    let fit = (k.m.geometry().page() / 64) as u32;
    for slot in 0..fit {
        let kind = [Endpoint, Reply, Notify][slot as usize % 3];
        k.ok(retype(s(30), kind, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), Endpoint, 0, s(999)), Error::NoMemory);
}

#[test]
fn directory_of_one_segment_takes_16_bytes() {
    let mut k = K::new();
    k.make(Untyped, 30);
    // Four directories in the first 64 bytes (16 bytes each since D-129: a
    // segment's address and its sequence word), then endpoints.
    for slot in 40..44 {
        k.ok(retype(s(30), CSpace, 128, s(slot)));
    }
    let fit = (k.m.geometry().page() / 64) as u32 - 1;
    for slot in 0..fit {
        k.ok(retype(s(30), Endpoint, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), Endpoint, 0, s(999)), Error::NoMemory);
}

#[test]
fn directory_is_aligned_on_its_size() {
    let mut k = K::new();
    k.make(Untyped, 30);
    // 16 bytes at 0, then directories of half a page from half a page:
    // nothing is left.
    let half = k.m.geometry().page() / 2;
    k.ok(retype(s(30), CSpace, 128, s(40)));
    k.ok(retype(s(30), CSpace, half / 16 * 128, s(41)));
    k.no(retype(s(30), Endpoint, 0, s(42)), Error::NoMemory);
}

#[test]
fn send_on_an_attached_reply_object_would_wait() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
    assert_eq!(k.woken(server), None);
}

#[test]
fn send_checks_the_listed_slots_before_the_state() {
    let mut k = K::with_ipc();
    k.no(
        send(RP, msg(9, vec![Transfer::Move(s(30))])),
        Error::Invalid,
    );
    k.no(
        send(EP, msg(9, vec![Transfer::Move(s(30))])),
        Error::Invalid,
    );
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
}

#[test]
fn recv_with_a_reply_object_that_is_not_idle_leaves_a_pending_notification() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.ok(Syscall::Signal(s(NT)));
    // The reply object is armed: `busy`, and the word stays.
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn grant_is_judged_before_the_limits_of_the_header() {
    let mut k = K::with_ipc();
    let endpoint = k.weak(EP, 30, caps_without(Rights::GRANT));
    let reply = k.weak(RP, 31, caps_without(Rights::GRANT));
    let many = msg(1, vec![Transfer::Move(s(NT)); MAX_CAPS + 1]);
    k.no(send(endpoint, many.clone()), Error::Denied);
    k.no(call(endpoint, many.clone(), Range::NONE), Error::Denied);
    k.no(reply_recv(reply, many.clone(), EP), Error::Denied);
    // With `grant`, the header is beyond the limits.
    k.no(send(EP, many), Error::Invalid);
    // A flag set by the sender, and one capability listed.
    let mut flagged = msg(1, vec![Transfer::Move(s(NT))]);
    flagged.flags = Header::FLAG_NOTIFICATION;
    k.no(send(endpoint, flagged.clone()), Error::Denied);
    k.no(send(EP, flagged), Error::Invalid);
}

#[test]
fn destroying_an_endpoint_wakes_its_callers_32_per_call() {
    let mut k = K::with_ipc();
    let callers: Vec<ObjId> = (40..73).map(|slot| k.spawn(slot)).collect();
    for &caller in &callers {
        assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    }
    k.no(Syscall::CapDelete(s(EP)), Error::Again);
    assert_eq!(k.woken(callers[31]), Some(Err(Error::Gone)));
    assert_eq!(k.woken(callers[32]), None);
    k.ok(Syscall::CapDelete(s(EP)));
    assert_eq!(k.woken(callers[32]), Some(Err(Error::Gone)));
}

#[test]
fn child_untyped_has_exec_only_if_the_untyped_capability_has_it() {
    let mut k = K::new();
    let no_exec = k.weak(START_SLOT_FIRST, 30, caps_without(Rights::EXEC));
    let bytes = 4 * k.m.geometry().page();
    k.ok(retype(s(no_exec), Untyped, bytes, s(31)));
    assert_eq!(
        k.inspect(s(31)),
        Ok((Untyped, caps_without(Rights::EXEC), 0))
    );
    // And so nothing made from the child has it.
    k.ok(retype(s(31), Frame, 1, s(32)));
    assert_eq!(k.inspect(s(32)), Ok((Frame, caps_without(Rights::EXEC), 0)));
    k.ok(retype(UT, Untyped, bytes, s(33)));
    assert_eq!(k.inspect(s(33)), Ok((Untyped, Rights::ALL, 0)));
}

#[test]
fn kind_that_is_not_made_is_unsupported_before_the_depths_and_the_state() {
    let mut k = K::new();
    for slot in 30..93 {
        let source = if slot == 30 {
            START_SLOT_FIRST
        } else {
            slot - 1
        };
        k.weak(source, slot, Rights::ALL);
    }
    // From an untyped capability at depth 64, into a slot that is full.
    k.no(
        retype(s(92), ObjectKind::Vcpu, 0, s(30)),
        Error::Unsupported,
    );
    k.no(retype(s(92), Endpoint, 0, s(30)), Error::Invalid);
    k.no(retype(UT, Endpoint, 0, s(30)), Error::NoSlot);
}

#[test]
fn thread_configure_checks_the_thread_first() {
    let mut k = K::with_ipc();
    k.make(Thread, 30);
    let no_manage = k.weak(30, 31, caps_without(Rights::MANAGE));
    k.no(
        configure(no_manage, s(NT), SlotAddr::NONE, SlotAddr::NONE),
        Error::Denied,
    );
    k.no(
        configure(NT, s(NT), SlotAddr::NONE, SlotAddr::NONE),
        Error::Invalid,
    );
    k.no(
        configure(30, s(NT), SlotAddr::NONE, SlotAddr::NONE),
        Error::Invalid,
    );
}

#[test]
fn suspended_caller_keeps_its_place_in_the_queue_of_an_endpoint() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Reply, 30);
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))), (1, 0, 0, 0));
    // It is answered while suspended, and runs only once resumed.
    assert_eq!(
        k.go(send(RP, msg(9, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(first).unwrap()), (9, 0, 0, 0));
    assert!(!k.can_run(first));
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(first));
}

#[test]
fn call_is_delivered_to_a_suspended_receiver_which_does_not_run() {
    let mut k = K::with_ipc();
    let (server, caller) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.woken(server).unwrap()), (1, 0, 0, 0));
    assert!(!k.can_run(server));
    // Its reply object is armed with the caller.
    assert_eq!(
        k.go(send(RP, msg(9, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, 0));
}

/// A capability space in slot 30 with one segment, destroyed while the
/// thread returned waits to receive with a range in it.
fn receiver_into_a_destroyed_space(k: &mut K) -> ObjId {
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    let server = k.spawn(70);
    let range = Range {
        first: via(30, 5),
        count: 1,
    };
    assert_eq!(k.on(server, recv(EP, RP, range)), WAITS);
    k.ok(Syscall::CapDelete(s(30)));
    server
}

#[test]
fn delivery_into_a_destroyed_capability_space_drops_the_capabilities() {
    let mut k = K::with_ipc();
    let server = receiver_into_a_destroyed_space(&mut k);
    let sent = k.go(send(
        EP,
        msg(5, vec![Transfer::Derive(s(NT), Rights::ALL.bits())]),
    ));
    assert_eq!(sent, Ok(Output::Sent { flags: DROPPED }));
    assert_eq!(got(k.woken(server).unwrap()), (5, 0, 0, DROPPED));
}

#[test]
fn message_without_capabilities_reaches_a_destroyed_space_without_the_dropped_flag() {
    let mut k = K::with_ipc();
    let server = receiver_into_a_destroyed_space(&mut k);
    assert_eq!(
        k.go(send(EP, msg(5, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(server).unwrap()), (5, 0, 0, 0));
}

#[test]
fn reply_into_a_destroyed_capability_space_drops_the_capabilities() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    let caller = k.spawn(70);
    let range = Range {
        first: via(30, 5),
        count: 1,
    };
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), range)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.ok(Syscall::CapDelete(s(30)));
    let sent = k.go(send(RP, msg(9, vec![Transfer::Move(s(NT))])));
    assert_eq!(sent, Ok(Output::Sent { flags: DROPPED }));
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, DROPPED));
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn caller_is_woken_with_gone_when_a_sweep_destroys_its_reply_object() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Reply, 0, via(30, 5)));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    let served = Syscall::Recv {
        endpoint: s(EP),
        reply: via(30, 5),
        range: Range::NONE,
        wait: 0,
    };
    got(k.go(served));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.woken(caller), Some(Err(Error::Gone)));
}

#[test]
fn slot_through_a_capability_for_a_dying_space_is_gone() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(derive(s(NT), via(30, 5), Rights::ALL, 0));
    for slot in 40..74 {
        k.weak(30, slot, Rights::ALL);
    }
    // 34 copies of the capability for the space: the oldest two are left.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.kind(41), Err(Error::Gone));
    assert_eq!(k.inspect(via(40, 5)), Err(Error::Gone));
    k.no(Syscall::Signal(via(40, 5)), Error::Gone);
    k.no(retype(UT, Notify, 0, via(40, 6)), Error::Gone);
    k.no(Syscall::CapDelete(via(40, 5)), Error::Gone);
    // A dying space does not grow.
    k.no(grow(40), Error::Gone);
}

#[test]
fn removal_does_not_end_while_another_thread_derives_33_capabilities_per_step() {
    // The limit that the contract accepts, for `cap_revoke` and for
    // `cap_delete`: what is below and not yet reached still works. Slot 30
    // is a copy of the endpoint capability, slot 31 a copy of that copy.
    // The test stops after 20 rounds; the removal would never end.
    for removal in [Syscall::CapRevoke(s(EP)), Syscall::CapDelete(s(30))] {
        let mut k = K::with_ipc();
        let other = k.spawn(29);
        k.weak(EP, 30, Rights::ALL);
        k.weak(30, 31, Rights::ALL);
        for round in 0..20 {
            for slot in 40..73 {
                let copy = derive(s(31), s(slot), Rights::ALL, 0);
                assert_eq!(k.on(other, copy), DONE, "round {round}");
            }
            // The 32 most recent copies go, one is left; the other thread
            // empties the slots and starts again.
            k.no(removal.clone(), Error::Again);
            assert_eq!(k.kind(40), Ok(Endpoint));
            for slot in 40..73 {
                assert_eq!(k.on(other, Syscall::CapDelete(s(slot))), DONE);
            }
        }
        // Once the other thread stops, the removal ends.
        k.ok(removal);
        assert_eq!(k.kind(31), Err(Error::Gone));
    }
}

#[test]
fn one_call_wakes_a_thread_of_a_reply_object_at_each_of_its_32_steps() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    let servers: Vec<ObjId> = (40..73).map(|slot| k.spawn(slot)).collect();
    // 33 reply objects in the space, each attached to a thread that waits.
    for (index, &server) in (1..).zip(&servers) {
        k.ok(retype(UT, Reply, 0, via(30, index)));
        let waits = Syscall::Recv {
            endpoint: s(EP),
            reply: via(30, index),
            range: Range::NONE,
            wait: 0,
        };
        assert_eq!(k.on(server, waits), WAITS);
    }
    // 32 reply objects removed, and 32 threads woken on top of that.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.woken(servers[31]), Some(Err(Error::Gone)));
    assert_eq!(k.woken(servers[32]), None);
}

// The fourth reviews of the model.

#[test]
fn numbers_of_the_contract_are_those_of_the_interface() {
    assert_eq!((MAX_WORDS, MAX_CAPS, REG_WORDS), (64, 4, 6));
    assert_eq!((TREE_DEPTH, LONGEST_WALK), (64, 191));
    assert_eq!((REMOVAL_STEP, REMOVAL_SLOTS), (32, 128));
    assert_eq!((CSPACE_SEGMENT_SLOTS, CSPACE_MAX_SLOTS), (128, 1 << 20));
    assert_eq!((SLOT_BYTES, CSPACE_DIRECTORY_BYTES), (64, 16));
    assert_eq!((ENDPOINT_BYTES, REPLY_BYTES, NOTIFY_BYTES), (64, 64, 64));
    assert_eq!(
        (SCHED_BYTES, THREAD_FIXED_BYTES, x86_64::ASPACE_BYTES),
        (256, 1024, 8192)
    );
    assert_eq!((TIMER_BYTES, IRQ_BYTES, TASK_BYTES), (64, 64, 256));
    assert_eq!(
        (x86_64::IOSPACE_BYTES, x86_64::UNTYPED_MIN_BYTES),
        (8192, 4096)
    );
    let fixed = [
        START_SLOT_CSPACE,
        START_SLOT_ASPACE,
        START_SLOT_THREAD,
        START_SLOT_SCHED,
        START_SLOT_DEBUG,
        START_SLOT_FIRST,
    ];
    assert_eq!(fixed, [1, 2, 3, 4, 5, 16]);
}

#[test]
fn numbers_of_the_errors_are_those_of_the_interface() {
    let errors = [
        Error::Ok,
        Error::Gone,
        Error::Again,
        Error::Denied,
        Error::Invalid,
        Error::NoMemory,
        Error::NoSlot,
        Error::Busy,
        Error::Budget,
        Error::WouldWait,
        Error::Unsupported,
        Error::Cancelled,
    ];
    for (number, error) in errors.into_iter().enumerate() {
        assert_eq!(error as usize, number);
    }
}

#[test]
fn bits_of_the_rights_are_those_of_the_interface() {
    let rights = [
        Rights::READ,
        Rights::WRITE,
        Rights::EXEC,
        Rights::SEND,
        Rights::RECV,
        Rights::GRANT,
        Rights::DERIVE,
        Rights::MANAGE,
    ];
    for (bit, right) in rights.into_iter().enumerate() {
        assert_eq!(right.bits(), 1 << bit);
    }
    assert_eq!(Rights::ALL.bits(), 0xff);
}

#[test]
fn start_up_untypeds_of_the_machine_of_the_tests_have_their_sizes() {
    let mut k = K::new();
    // The second untyped of the floor of node 0, one chunk, and the device
    // range of 2 MB.
    let (chunk, page) = (k.m.geometry().chunk(), k.m.geometry().page());
    k.ok(retype(UT, Untyped, chunk, s(30)));
    k.no(retype(UT, Untyped, page, s(31)), Error::NoMemory);
    k.ok(retype(DEV, Untyped, 2 << 20, s(32)));
    k.no(retype(DEV, Untyped, page, s(33)), Error::NoMemory);
}

#[test]
fn poll_returns_the_word_and_clears_it() {
    let mut k = K::with_ipc();
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn receivers_are_served_first_in_first_out() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.make(Reply, 30);
    assert_eq!(k.on(first, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.on(second, recv(EP, 30, Range::NONE)), WAITS);
    k.go(send(EP, msg(5, vec![]))).unwrap();
    assert_eq!(got(k.woken(first).unwrap()), (5, 0, 0, 0));
    assert_eq!(k.woken(second), None);
    k.go(send(EP, msg(6, vec![]))).unwrap();
    assert_eq!(got(k.woken(second).unwrap()), (6, 0, 0, 0));
}

#[test]
fn two_slots_for_one_object_are_listed_in_one_message() {
    let mut k = K::with_ipc();
    k.weak(NT, 30, Rights::ALL);
    let caps = vec![
        Transfer::Move(s(NT)),
        Transfer::Derive(s(30), Rights::SEND.bits()),
    ];
    caller_with(&mut k, caps, Range::NONE);
    assert_eq!(got(k.go(recv(EP, RP, range(50, 2)))), (1, 0, 2, 0));
    assert_eq!(k.inspect(s(51)), Ok((Notify, Rights::SEND, 0)));
}

#[test]
fn slot_listed_twice_is_invalid_wherever_it_is_in_the_list() {
    let mut k = K::with_ipc();
    k.make(Notify, 30);
    let copy = |slot| Transfer::Derive(s(slot), Rights::NONE.bits());
    k.no(
        send(EP, msg(1, vec![copy(NT), copy(30), copy(NT)])),
        Error::Invalid,
    );
    k.no(send(EP, msg(1, vec![copy(NT), copy(30)])), Error::WouldWait);
}

#[test]
fn depth_of_a_capability_to_derive_is_judged_before_the_next_listed_slot() {
    let mut k = K::with_ipc();
    for slot in 30..92 {
        k.weak(if slot == 30 { NT } else { slot - 1 }, slot, Rights::ALL);
    }
    k.weak(RP, 100, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(RP)));
    // Slot 91 is at depth 64, slot 100 is dead.
    let deep = Transfer::Derive(s(91), Rights::NONE.bits());
    let dead = Transfer::Move(s(100));
    k.no(send(EP, msg(1, vec![deep, dead])), Error::Invalid);
    k.no(send(EP, msg(1, vec![dead, deep])), Error::Gone);
}

#[test]
fn recv_checks_the_reply_object_before_the_range() {
    let mut k = K::with_ipc();
    let read_only = k.weak(START_SLOT_CSPACE, 31, Rights::READ);
    k.weak(RP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(RP)));
    let denied = Range {
        first: via(read_only, 40),
        count: 1,
    };
    k.no(recv(EP, 30, denied), Error::Gone);
    k.no(recv(EP, RP, denied), Error::Denied);
}

#[test]
fn notify_bind_with_endpoint_0_needs_recv_on_the_notify_only() {
    let mut k = K::with_ipc();
    let recv_only = k.weak(NT, 30, Rights::RECV);
    k.ok(bind(NT, s(EP)));
    k.ok(bind(recv_only, SlotAddr::NONE));
    k.ok(bind(NT, s(EP)));
}

#[test]
fn removing_a_binding_leaves_the_word_pending() {
    let mut k = K::with_ipc();
    k.ok(bind(NT, s(EP)));
    k.ok(Syscall::Signal(s(NT)));
    k.ok(bind(NT, SlotAddr::NONE));
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn cap_delete_on_an_empty_slot_of_an_own_dying_space_is_invalid() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    // Slots of the dying space itself (`o`): 100 is an ordinary empty
    // slot, 128 the first past its table, 0 the slot never used (the final
    // review's H1: `s` named slots past the table, and only slot 0 inside
    // it).
    assert_eq!(
        k.on(inside, Syscall::CapDelete(o(100))),
        Err(Error::Invalid)
    );
    assert_eq!(
        k.on(inside, Syscall::CapDelete(o(128))),
        Err(Error::Invalid)
    );
    assert_eq!(k.on(inside, Syscall::CapDelete(o(0))), Err(Error::Invalid));
}

#[test]
fn sched_is_aligned_on_256_bytes() {
    let mut k = K::new();
    k.make(Untyped, 30);
    // An endpoint at 0, then scheduling contexts from 256: nothing left.
    k.ok(retype(s(30), Endpoint, 0, s(40)));
    let fit = (k.m.geometry().page() / 256) as u32 - 1;
    for slot in 0..fit {
        k.ok(retype(s(30), ObjectKind::Sched, 0, s(1000 + slot)));
    }
    k.no(retype(s(30), Endpoint, 0, s(999)), Error::NoMemory);
}

#[test]
fn thread_suspend_succeeds_on_a_thread_bound_to_no_capability_space() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.ok(Syscall::ThreadSuspend(s(30)));
    k.no(Syscall::ThreadResume(s(30)), Error::Busy);
}

#[test]
fn thread_resume_is_busy_for_a_thread_that_runs_in_a_destroyed_space() {
    let mut k = K::new();
    k.make(CSpace, 30);
    k.make(Thread, 31);
    k.ok(configure(31, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(31)));
    k.ok(Syscall::CapDelete(s(30)));
    // Already in the state asked for, and `busy` comes first.
    let other = k.m.object_at(k.t, s(31)).unwrap();
    assert!(k.can_run(other));
    k.no(Syscall::ThreadResume(s(31)), Error::Busy);
}

#[test]
fn signal_on_a_notify_bound_to_a_dying_endpoint_still_wakes_its_receiver() {
    let mut k = K::with_ipc();
    let server = k.spawn(70);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    dying(&mut k, EP, 30);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(got(k.woken(server).unwrap()).3, Header::FLAG_NOTIFICATION);
}

#[test]
fn binding_to_a_dying_endpoint_is_removed_and_the_word_goes_elsewhere() {
    let mut k = K::with_ipc();
    let server = k.spawn(70);
    k.make(Endpoint, 80);
    k.make(Reply, 81);
    k.ok(bind(NT, s(EP)));
    dying(&mut k, EP, 30);
    k.ok(Syscall::Signal(s(NT)));
    k.ok(bind(NT, SlotAddr::NONE));
    assert_eq!(k.on(server, recv(80, 81, Range::NONE)), WAITS);
    k.ok(bind(NT, s(80)));
    assert_eq!(
        got(k.woken(server).unwrap()),
        (0, 1, 0, Header::FLAG_NOTIFICATION)
    );
}

#[test]
fn thread_whose_capability_is_being_deleted_still_receives_and_arms_its_reply_object() {
    let mut k = K::with_ipc();
    let (server, caller) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    for slot in 50..83 {
        k.weak(40, slot, Rights::ALL);
    }
    k.no(Syscall::CapDelete(s(40)), Error::Again);
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.woken(server).unwrap()), (1, 0, 0, 0));
    // The thread is destroyed; its caller waits on, and is answered through
    // the reply object.
    k.ok(Syscall::CapDelete(s(40)));
    assert!(!k.can_run(server) && k.woken(caller).is_none());
    assert_eq!(
        k.go(send(RP, msg(9, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(caller).unwrap()), (9, 0, 0, 0));
}

#[test]
fn copy_of_a_dying_object_is_deleted_and_not_revoked() {
    let mut k = K::with_ipc();
    let copy = dying(&mut k, EP, 30);
    k.no(Syscall::CapRevoke(s(copy)), Error::Gone);
    k.ok(Syscall::CapDelete(s(copy)));
    assert_eq!(k.kind(copy), Err(Error::Invalid));
}

#[test]
fn device_memory_refuses_a_kind_that_is_not_made_as_invalid_whatever_the_size() {
    let mut k = K::new();
    k.no(retype(DEV, ObjectKind::Timer, 5, s(30)), Error::Invalid);
    k.no(retype(DEV, ObjectKind::Timer, 0, s(30)), Error::Invalid);
}

#[test]
fn reply_recv_with_an_attached_reply_object_leaves_a_pending_notification() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.make(Endpoint, 30);
    assert_eq!(k.on(server, recv(30, RP, Range::NONE)), WAITS);
    k.ok(bind(NT, s(EP)));
    k.ok(Syscall::Signal(s(NT)));
    k.no(reply_recv(RP, msg(1, vec![]), EP), Error::Busy);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn thread_configure_is_busy_in_every_wait() {
    let mut k = K::with_ipc();
    let (caller, server) = (k.spawn(40), k.spawn(41));
    k.make(Endpoint, 30);
    k.make(Reply, 31);
    let none = SlotAddr::NONE;
    for slot in [40, 41] {
        k.ok(Syscall::ThreadSuspend(s(slot)));
        k.ok(Syscall::ThreadResume(s(slot)));
    }
    // Waiting to be received, waiting to receive, then waiting for a reply.
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(server, recv(30, 31, Range::NONE)), WAITS);
    for slot in [40, 41] {
        k.ok(Syscall::ThreadSuspend(s(slot)));
        k.no(configure(slot, SPACE, none, none), Error::Busy);
    }
    got(k.go(recv(EP, RP, Range::NONE)));
    k.no(configure(40, SPACE, none, none), Error::Busy);
    // Answered while suspended, it waits no more.
    k.go(send(RP, msg(9, vec![]))).unwrap();
    k.ok(configure(40, SPACE, none, none));
}

#[test]
fn thread_that_waits_on_the_notify_is_served_before_a_receiver_of_the_endpoint() {
    let mut k = K::with_ipc();
    let (server, waiter) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    // A thread waits on the notify: its word is zero, binding delivers
    // nothing, and the next signal is for that thread.
    k.ok(bind(NT, s(EP)));
    assert_eq!((k.woken(waiter), k.woken(server)), (None, None));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
    assert_eq!(k.woken(server), None);
}

#[test]
fn revoke_of_a_capability_crosses_the_sweep_of_a_space_that_holds_its_copies() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    for index in 1..=40 {
        k.ok(derive(s(EP), via(30, index), Rights::ALL, 0));
    }
    // The sweep removes 32 copies; the revocation removes the other 8,
    // ahead of the sweep; the sweep then finds only dead slots.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.ok(Syscall::CapRevoke(s(EP)));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(EP), Ok(Endpoint));
}

#[test]
fn two_sweeps_and_the_removal_of_the_capability_they_hold_copies_of_all_end() {
    let mut k = K::with_ipc();
    for space in [30, 31] {
        k.make(CSpace, space);
        k.go(grow(space)).unwrap();
        for index in 1..=40 {
            k.ok(derive(s(EP), via(space, index), Rights::ALL, 0));
        }
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.no(Syscall::CapDelete(s(31)), Error::Again);
    k.ok(Syscall::CapDelete(s(EP)));
    k.ok(Syscall::CapDelete(s(31)));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(
        (k.kind(30), k.kind(31), k.kind(EP)),
        (
            Err(Error::Invalid),
            Err(Error::Invalid),
            Err(Error::Invalid)
        )
    );
}

#[test]
fn removal_begun_inside_a_dying_space_is_finished_by_the_sweep() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    for index in 60..93 {
        k.ok(derive(via(30, 20), via(30, index), Rights::ALL, 0));
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    // The thread begins to delete the notify of slot 20, ahead of the sweep.
    assert_eq!(k.on(inside, Syscall::CapDelete(o(20))), Err(Error::Again));
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.kind(60), Err(Error::Gone));
    assert_eq!(k.on(inside, Syscall::CapDelete(o(20))), Err(Error::Gone));
}

#[test]
fn supervisor_kills_a_process_by_deleting_its_space_then_its_untyped() {
    let mut k = K::with_ipc();
    // The process: its untyped in slot 30, its space in slot 31 with two
    // segments, its thread in slot 32. In its space: memory of its own in
    // slot 16, an endpoint of its supervisor in slot 3.
    k.ok(retype(UT, Untyped, 1 << 20, s(30)));
    k.ok(retype(s(30), CSpace, 256, s(31)));
    k.go(grow_from(31, s(30))).unwrap();
    k.go(grow_from(31, s(30))).unwrap();
    k.ok(retype(s(30), Untyped, 1 << 16, via(31, 16)));
    k.ok(derive(s(EP), via(31, 3), Rights::ALL, 0));
    k.ok(retype(s(30), Thread, 0, s(32)));
    k.ok(retype(s(30), Frame, 1, s(33)));
    k.ok(with_page(
        configure(32, s(31), SlotAddr::NONE, SlotAddr::NONE),
        33,
    ));
    k.ok(Syscall::ThreadResume(s(32)));
    let process = k.m.object_at(k.t, s(32)).unwrap();
    // It fills its space with 200 copies, and goes on deriving and sending.
    for slot in 20..220 {
        assert_eq!(k.on(process, derive(o(16), o(slot), Rights::ALL, 0)), DONE);
    }
    let copy = derive(o(16), o(230), Rights::ALL, 0);
    let sent = send(3, msg(1, vec![Transfer::Derive(o(16), Rights::ALL.bits())]));
    assert_eq!(k.on(process, sent.clone()), Err(Error::WouldWait));
    // From the first step of the removal of its space it gets `gone`.
    let calls = (1..=100)
        .find(|_| {
            if k.go(Syscall::CapDelete(s(31))) == DONE {
                return true;
            }
            assert_eq!(k.on(process, copy.clone()), Err(Error::Gone));
            assert_eq!(k.on(process, sent.clone()), Err(Error::Gone));
            false
        })
        .expect("the removal ends");
    // 203 capabilities at 32 per call are seven calls; the 255 slots of the
    // table, at 128 per call, end one call early and make it eight.
    assert_eq!(calls, 8);
    assert_eq!(k.on(process, copy), Err(Error::Gone));
    // Then its untyped, and with it its thread.
    k.ok(Syscall::CapDelete(s(30)));
    assert_eq!(k.m.runnable(), [k.t]);
    assert_eq!(k.kind(30), Err(Error::Invalid));
}

// What the mutation check of the last pass found without a test.

#[test]
fn capability_at_depth_64_is_moved_by_ipc() {
    let mut k = K::with_ipc();
    for slot in 30..92 {
        k.weak(if slot == 30 { NT } else { slot - 1 }, slot, Rights::ALL);
    }
    // Slot 91 is at depth 64: it cannot be derived, and it can be moved.
    let deep = |transfer| send(EP, msg(1, vec![transfer]));
    k.no(
        deep(Transfer::Derive(s(91), Rights::NONE.bits())),
        Error::Invalid,
    );
    k.no(deep(Transfer::Move(s(91))), Error::WouldWait);
}

#[test]
fn notify_bind_checks_the_notify_before_the_endpoint() {
    let mut k = K::with_ipc();
    let no_manage = k.weak(EP, 30, caps_without(Rights::MANAGE));
    k.no(bind(EP, s(no_manage)), Error::Invalid);
    k.no(bind(NT, s(no_manage)), Error::Denied);
}

#[test]
fn rights_of_a_listed_slot_are_judged_before_whether_it_is_listed_twice() {
    let mut k = K::with_ipc();
    let weak = k.weak(NT, 30, Rights::SEND.union(Rights::DERIVE));
    let twice = |rights: Rights| {
        let first = Transfer::Derive(s(weak), Rights::NONE.bits());
        send(
            EP,
            msg(1, vec![first, Transfer::Derive(s(weak), rights.bits())]),
        )
    };
    k.no(twice(Rights::ALL), Error::Denied);
    k.no(twice(Rights::SEND), Error::Invalid);
}

#[test]
fn cap_move_checks_the_source_before_the_destination() {
    let mut k = K::with_ipc();
    k.weak(RP, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(RP)));
    // Slot 30 is dead; slot 0 is no destination.
    k.no(mv(s(30), s(0)), Error::Gone);
    k.no(mv(s(RP), s(0)), Error::Invalid);
}

#[test]
fn untyped_retype_checks_the_untyped_before_the_destination() {
    let mut k = K::new();
    k.make(Untyped, 31);
    k.weak(31, 30, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(31)));
    k.no(retype(s(30), Endpoint, 0, s(0)), Error::Gone);
    k.no(retype(s(31), Endpoint, 0, s(0)), Error::Invalid);
}

// `thread_cancel`.

const CANCELLED: Option<Outcome> = Some(Err(Error::Cancelled));

fn cancel(thread: u32) -> Syscall {
    Syscall::ThreadCancel(s(thread), 0)
}

#[test]
fn thread_cancel_takes_a_queued_caller_out_of_the_queue() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(first, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(second, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    k.ok(cancel(40));
    assert_eq!(k.woken(first), CANCELLED);
    assert!(k.can_run(first));
    assert_eq!(k.woken(second), None);
    assert_eq!(got(k.go(recv(EP, RP, Range::NONE))).0, 2);
}

#[test]
fn send_on_the_reply_object_of_a_cancelled_caller_is_would_wait() {
    let mut k = K::with_ipc();
    let caller = caller_with(&mut k, vec![], Range::NONE);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.ok(cancel(40));
    assert_eq!(k.woken(caller), CANCELLED);
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
    assert_eq!(k.woken(caller), CANCELLED);
}

#[test]
fn reply_recv_on_the_reply_object_of_a_cancelled_caller_drops_the_reply() {
    let mut k = K::with_ipc();
    let server = k.spawn(41);
    let caller = caller_with(&mut k, vec![], Range::NONE);
    got(k.on(server, recv(EP, RP, Range::NONE)));
    k.ok(cancel(40));
    assert_eq!(k.on(server, reply_recv(RP, msg(9, vec![]), EP)), WAITS);
    assert_eq!(k.woken(caller), CANCELLED);
}

#[test]
fn thread_cancel_of_a_receiver_makes_its_reply_object_idle() {
    let mut k = K::with_ipc();
    let (server, other) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
    k.ok(cancel(40));
    assert_eq!(k.woken(server), CANCELLED);
    // No thread waits to receive, and the reply object can be used again.
    k.no(send(EP, msg(1, vec![])), Error::WouldWait);
    assert_eq!(k.on(other, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn thread_cancel_of_a_waiter_on_a_notify_leaves_the_word_and_the_binding() {
    let mut k = K::with_ipc();
    k.ok(bind(NT, s(EP)));
    let (first, second) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(first, Syscall::Wait(s(NT), 0)), WAITS);
    assert_eq!(k.on(second, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(cancel(40));
    assert_eq!(k.woken(first), CANCELLED);
    // The word of the notify is still 0.
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    k.no(bind(NT, s(EP)), Error::Busy);
    // The next signal goes to the waiter that is left, then stays pending.
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(second), Some(Ok(Output::Word(1))));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn thread_cancel_of_a_caller_moves_none_of_the_capabilities_it_listed() {
    let mut k = K::with_ipc();
    k.make(Notify, 30);
    let listed = vec![
        Transfer::Move(s(NT)),
        Transfer::Derive(s(30), Rights::ALL.bits()),
    ];
    let caller = caller_with(&mut k, listed, range(50, 2));
    let before = k.m.census();
    k.ok(cancel(40));
    assert_eq!(k.woken(caller), CANCELLED);
    assert_eq!(k.kind(NT), Ok(Notify));
    assert_eq!(k.kind(50), Err(Error::Invalid));
    // No copy of slot 30 was made: nothing is left to revoke.
    k.ok(Syscall::CapRevoke(s(30)));
    assert_eq!(before.callers, 1);
    assert_eq!(k.m.census().callers, 0);
}

#[test]
fn thread_cancel_of_a_thread_that_does_not_wait_changes_nothing_and_is_not_remembered() {
    let mut k = K::with_ipc();
    let other = k.spawn(40);
    let before = k.m.clone();
    k.ok(cancel(40));
    assert_eq!(k.m, before);
    assert_eq!(k.on(other, Syscall::Wait(s(NT), 0)), WAITS);
    assert_eq!(k.woken(other), None);
    assert!(!k.can_run(other));
}

#[test]
fn suspended_thread_that_is_cancelled_stays_suspended_and_finds_cancelled() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(cancel(40));
    assert!(!k.can_run(waiter));
    assert_eq!(k.woken(waiter), CANCELLED);
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(waiter));
    assert_eq!(k.woken(waiter), CANCELLED);
}

#[test]
fn thread_cancel_needs_manage_on_a_thread() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(30);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    let no_manage = k.weak(30, 40, caps_without(Rights::MANAGE));
    k.no(cancel(no_manage), Error::Denied);
    k.no(cancel(EP), Error::Invalid);
    k.no(cancel(41), Error::Invalid);
    assert_eq!(k.woken(waiter), None);
}

#[test]
fn thread_that_cancels_itself_does_not_wait_and_nothing_changes() {
    let mut k = K::new();
    let before = k.m.clone();
    k.ok(cancel(START_SLOT_THREAD));
    assert_eq!(k.m, before);
}

#[test]
fn thread_cancel_by_a_thread_whose_capability_space_is_dying_is_gone() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.on(inside, cancel(20)), Err(Error::Gone));
}

#[test]
fn thread_cancel_leaves_the_outcome_of_a_wait_that_is_already_over() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    k.ok(cancel(40));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
}

#[test]
fn outcome_of_a_cancelled_wait_is_cleared_by_the_next_call_of_the_thread() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), WAITS);
    k.ok(cancel(40));
    assert_eq!(k.on(waiter, Syscall::Poll(s(NT))), Err(Error::WouldWait));
    assert_eq!(k.woken(waiter), None);
}

/// The intended use: a limit on a wait is built outside the kernel.
#[test]
fn client_cancelled_in_a_call_calls_again_and_the_late_answer_is_dropped() {
    let mut k = K::with_ipc();
    let (client, server) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(client, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.on(server, recv(EP, RP, Range::NONE))).0, 1);
    // The server does not answer; a second thread cancels the client.
    k.ok(cancel(40));
    assert_eq!(k.woken(client), CANCELLED);
    assert_eq!(k.on(client, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    // The late answer to the first call is dropped: the client still waits
    // to be received, and the server receives its second call.
    let late = k.on(server, reply_recv(RP, msg(9, vec![]), EP));
    assert_eq!(got(late).0, 2);
    assert_eq!(k.woken(client), None);
    assert_eq!(
        k.on(server, send(RP, msg(8, vec![]))),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(got(k.woken(client).unwrap()).0, 8);
}

// Wait numbers (D-109).

fn cancel_up_to(thread: u32, number: u64) -> Syscall {
    Syscall::ThreadCancel(s(thread), number)
}

fn wait_on(notify: u32, number: u64) -> Syscall {
    Syscall::Wait(s(notify), number)
}

/// A call that can wait, with a wait number.
fn numbered(mut call: Syscall, number: u64) -> Syscall {
    match &mut call {
        Syscall::Call { wait, .. }
        | Syscall::Recv { wait, .. }
        | Syscall::ReplyRecv { wait, .. }
        | Syscall::Wait(_, wait) => *wait = number,
        other => panic!("{other:?} does not wait"),
    }
    call
}

#[test]
fn wait_whose_number_is_already_cancelled_returns_cancelled_at_once() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    // The cancel comes before the wait: it is remembered.
    k.ok(cancel_up_to(40, 5));
    for number in [1, 5] {
        assert_eq!(k.on(waiter, wait_on(NT, number)), Err(Error::Cancelled));
        assert!(k.can_run(waiter));
    }
    assert_eq!(k.on(waiter, wait_on(NT, 6)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
}

#[test]
fn cancel_that_comes_after_the_wait_is_over_ends_nothing_later() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, wait_on(NT, 1)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    k.ok(cancel_up_to(40, 1));
    assert_eq!(k.woken(waiter), Some(Ok(Output::Word(1))));
    // The next wait has a greater number.
    assert_eq!(k.on(waiter, wait_on(NT, 2)), WAITS);
    assert_eq!(k.woken(waiter), None);
    assert!(!k.can_run(waiter));
}

#[test]
fn stale_smaller_number_does_not_end_a_wait_with_a_greater_one() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, wait_on(NT, 7)), WAITS);
    k.ok(cancel_up_to(40, 6));
    assert_eq!(k.woken(waiter), None);
    assert!(!k.can_run(waiter));
    // A number equal to the wait's ends it.
    k.ok(cancel_up_to(40, 7));
    assert_eq!(k.woken(waiter), CANCELLED);
    // A smaller number does not lower the word.
    k.ok(cancel_up_to(40, 2));
    assert_eq!(k.on(waiter, wait_on(NT, 7)), Err(Error::Cancelled));
    assert_eq!(k.on(waiter, wait_on(NT, 8)), WAITS);
}

#[test]
fn wait_number_0_is_never_cancelled_by_number_and_cancel_number_0_ends_any_wait() {
    let mut k = K::with_ipc();
    let (first, second) = (k.spawn(40), k.spawn(41));
    k.ok(cancel_up_to(40, 9));
    // A wait without a number waits whatever the word, and a number does
    // not end it.
    assert_eq!(k.on(first, wait_on(NT, 0)), WAITS);
    k.ok(cancel_up_to(40, 9));
    k.ok(cancel_up_to(40, 10));
    assert_eq!(k.woken(first), None);
    k.ok(cancel_up_to(40, 0));
    assert_eq!(k.woken(first), CANCELLED);
    // Number 0 ends a wait that has a number, and remembers nothing.
    assert_eq!(k.on(second, wait_on(NT, 5)), WAITS);
    k.ok(cancel_up_to(41, 0));
    assert_eq!(k.woken(second), CANCELLED);
    assert_eq!(k.on(second, wait_on(NT, 1)), WAITS);
}

#[test]
fn call_that_ends_without_waiting_does_not_look_at_the_word() {
    let mut k = K::with_ipc();
    let (server, caller) = (k.spawn(40), k.spawn(41));
    k.ok(cancel_up_to(40, 5));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.on(server, wait_on(NT, 5)), Ok(Output::Word(1)));
    assert_eq!(k.on(caller, call(EP, msg(3, vec![]), Range::NONE)), WAITS);
    let received = k.on(server, numbered(recv(EP, RP, Range::NONE), 5));
    assert_eq!(got(received).0, 3);
    // Not even a call that fails.
    k.no(numbered(recv(EP, RP, Range::NONE), 5), Error::Busy);
}

#[test]
fn call_whose_number_is_cancelled_does_not_join_the_queue() {
    let mut k = K::with_ipc();
    let (client, server) = (k.spawn(40), k.spawn(41));
    k.ok(cancel_up_to(40, 1));
    let queued = numbered(call(EP, msg(1, vec![]), Range::NONE), 1);
    assert_eq!(k.on(client, queued), Err(Error::Cancelled));
    assert_eq!(k.m.census().callers, 0);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn call_delivered_at_once_whose_number_is_cancelled_has_sent_its_message() {
    let mut k = K::with_ipc();
    let (client, server) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, range(50, 1))), WAITS);
    k.ok(cancel_up_to(40, 1));
    let sent = msg(1, vec![Transfer::Move(s(NT))]);
    let delivered = numbered(call(EP, sent, Range::NONE), 1);
    assert_eq!(k.on(client, delivered), Err(Error::Cancelled));
    assert_eq!(got(k.woken(server).unwrap()), (1, 0, 1, 0));
    assert_eq!(k.kind(50), Ok(Notify));
    // The reply object it armed is idle again.
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn recv_whose_number_is_cancelled_leaves_its_reply_object_idle() {
    let mut k = K::with_ipc();
    let (server, other) = (k.spawn(40), k.spawn(41));
    k.ok(cancel_up_to(40, 2));
    let received = numbered(recv(EP, RP, Range::NONE), 2);
    assert_eq!(k.on(server, received), Err(Error::Cancelled));
    k.no(send(EP, msg(1, vec![])), Error::WouldWait);
    assert_eq!(k.on(other, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn reply_recv_whose_receive_is_cancelled_at_once_has_delivered_its_reply() {
    let mut k = K::with_ipc();
    let (client, server) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(client, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.on(server, recv(EP, RP, Range::NONE))).0, 1);
    k.ok(cancel_up_to(41, 4));
    let answer = numbered(reply_recv(RP, msg(8, vec![]), EP), 4);
    assert_eq!(k.on(server, answer), Err(Error::Cancelled));
    assert_eq!(got(k.woken(client).unwrap()).0, 8);
    // It does not wait to receive, and its reply object is idle.
    k.no(send(EP, msg(2, vec![])), Error::WouldWait);
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
}

#[test]
fn thread_cancel_with_a_number_ends_each_of_the_four_waits() {
    let mut k = K::with_ipc();
    let threads: Vec<ObjId> = (40..44).map(|slot| k.spawn(slot)).collect();
    k.make(Endpoint, 30);
    k.make(Reply, 31);
    let message = || msg(1, vec![]);
    let waits = [
        numbered(call(30, message(), Range::NONE), 3),
        numbered(call(EP, message(), Range::NONE), 3),
        numbered(recv(EP, 31, Range::NONE), 3),
        wait_on(NT, 3),
    ];
    for (&thread, wait) in threads.iter().zip(waits) {
        assert_eq!(k.on(thread, wait), WAITS);
        // The second, once received, waits for the reply with its number.
        if thread == threads[1] {
            assert_eq!(got(k.go(recv(EP, RP, Range::NONE))).0, 1);
        }
    }
    let census = k.m.census();
    let waiting = [census.callers, census.served, census.receivers];
    assert_eq!((waiting, census.waiters), ([1, 1, 1], 1));
    for slot in 40..44 {
        k.ok(cancel_up_to(slot, 2));
    }
    assert!(threads.iter().all(|&thread| k.woken(thread).is_none()));
    for slot in 40..44 {
        k.ok(cancel_up_to(slot, 3));
    }
    assert!(threads.iter().all(|&thread| k.woken(thread) == CANCELLED));
}

#[test]
fn suspended_thread_whose_number_is_cancelled_stays_suspended() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    assert_eq!(k.on(waiter, wait_on(NT, 2)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.ok(cancel_up_to(40, 2));
    assert!(!k.can_run(waiter));
    assert_eq!(k.woken(waiter), CANCELLED);
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(waiter));
}

// The fifth reviews of the model.

#[test]
fn suspended_thread_cancelled_in_each_ipc_wait_stays_suspended() {
    let mut k = K::with_ipc();
    let (queued, served, server) = (k.spawn(40), k.spawn(41), k.spawn(42));
    k.make(Endpoint, 30);
    k.make(Reply, 31);
    assert_eq!(k.on(queued, call(30, msg(1, vec![]), Range::NONE)), WAITS);
    assert_eq!(k.on(served, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    assert_eq!(k.on(server, recv(EP, 31, Range::NONE)), WAITS);
    for slot in 40..43 {
        k.ok(Syscall::ThreadSuspend(s(slot)));
        k.ok(cancel(slot));
    }
    for thread in [queued, served, server] {
        assert!(!k.can_run(thread));
        assert_eq!(k.woken(thread), CANCELLED);
    }
    k.no(send(RP, msg(9, vec![])), Error::WouldWait);
    k.no(send(EP, msg(9, vec![])), Error::WouldWait);
}

#[test]
fn thread_cancel_leaves_a_message_or_gone_that_already_ended_the_wait() {
    let mut k = K::with_ipc();
    let (caller, waiter) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(caller, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    got(k.go(recv(EP, RP, Range::NONE)));
    k.go(send(RP, msg(9, vec![]))).unwrap();
    k.make(Notify, 30);
    assert_eq!(k.on(waiter, wait_on(30, 0)), WAITS);
    k.ok(Syscall::CapDelete(s(30)));
    k.ok(cancel(40));
    k.ok(cancel_up_to(41, 1));
    assert_eq!(got(k.woken(caller).unwrap()).0, 9);
    assert_eq!(k.woken(waiter), Some(Err(Error::Gone)));
}

#[test]
fn thread_cancel_ends_the_wait_of_a_thread_bound_to_a_dying_or_destroyed_space() {
    let mut k = K::with_ipc();
    space_with_a_thread(&mut k);
    k.make(Thread, 32);
    k.ok(configure(32, s(30), SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(32)));
    let threads = [31, 32].map(|slot| k.m.object_at(k.t, s(slot)).unwrap());
    // Both wait on the notify of the first thread, through a copy in slot
    // 21 of their space, which the sweep reaches after 40 capabilities.
    k.ok(derive(s(NT), via(30, 21), Rights::ALL, 0));
    for thread in threads {
        assert_eq!(k.on(thread, Syscall::Wait(o(21), 0)), WAITS);
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.ok(cancel(31));
    assert_eq!(k.woken(threads[0]), CANCELLED);
    k.finish(Syscall::CapDelete(s(30)), 4);
    assert_eq!(k.woken(threads[1]), None);
    k.ok(cancel_up_to(32, 0));
    assert_eq!(k.woken(threads[1]), CANCELLED);
    // A thread bound to no space.
    k.make(Thread, 33);
    k.ok(cancel(33));
    k.ok(cancel_up_to(33, 4));
}

#[test]
fn thread_cancel_by_a_thread_whose_capability_space_is_destroyed_is_gone() {
    let mut k = K::new();
    let inside = space_with_a_thread(&mut k);
    k.finish(Syscall::CapDelete(s(30)), 4);
    assert_eq!(k.on(inside, cancel(20)), Err(Error::Gone));
}

#[test]
fn thread_cancel_on_a_dying_thread_or_through_a_dying_copy_is_gone() {
    let mut k = K::with_ipc();
    // A thread whose root capability is under `cap_delete`.
    k.make(Thread, 30);
    let copy = dying(&mut k, 30, 40);
    k.no(cancel(copy), Error::Gone);
    k.no(cancel(30), Error::Gone);
    // A copy under `cap_delete` of a thread that lives.
    let waiter = k.spawn(31);
    assert_eq!(k.on(waiter, wait_on(NT, 0)), WAITS);
    k.weak(31, 32, Rights::ALL);
    dying(&mut k, 32, 80);
    k.no(cancel(32), Error::Gone);
    assert_eq!(k.woken(waiter), None);
    k.ok(cancel(31));
    assert_eq!(k.woken(waiter), CANCELLED);
}

#[test]
fn thread_cancel_takes_a_waiter_out_of_the_queue_of_a_dying_notify() {
    let mut k = K::with_ipc();
    let threads: Vec<ObjId> = (40..74).map(|slot| k.spawn(slot)).collect();
    for &thread in &threads {
        assert_eq!(k.on(thread, wait_on(NT, 0)), WAITS);
    }
    // 32 are woken; two wait on the dying notify.
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    k.ok(cancel(72));
    assert_eq!(k.woken(threads[32]), CANCELLED);
    k.ok(Syscall::CapDelete(s(NT)));
    assert_eq!(k.woken(threads[32]), CANCELLED);
    assert_eq!(k.woken(threads[33]), Some(Err(Error::Gone)));
}

#[test]
fn thread_configure_is_busy_until_the_wait_is_cancelled_and_keeps_the_outcome() {
    let mut k = K::with_ipc();
    let waiter = k.spawn(40);
    let none = SlotAddr::NONE;
    assert_eq!(k.on(waiter, wait_on(NT, 0)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.no(configure(40, SPACE, none, none), Error::Busy);
    k.ok(cancel(40));
    k.ok(configure(40, SPACE, none, none));
    assert_eq!(k.woken(waiter), CANCELLED);
}

#[test]
fn reply_recv_to_a_cancelled_caller_moves_none_of_the_capabilities_of_the_reply() {
    let mut k = K::with_ipc();
    let server = k.spawn(41);
    caller_with(&mut k, vec![], Range::NONE);
    got(k.on(server, recv(EP, RP, Range::NONE)));
    k.ok(cancel(40));
    let answer = reply_recv(RP, msg(9, vec![Transfer::Move(s(NT))]), EP);
    assert_eq!(k.on(server, answer), WAITS);
    assert_eq!(k.kind(NT), Ok(Notify));
}

#[test]
fn signal_after_the_receiver_of_a_bound_endpoint_is_cancelled_stays_pending() {
    let mut k = K::with_ipc();
    let server = k.spawn(40);
    k.ok(bind(NT, s(EP)));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    k.ok(cancel(40));
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.woken(server), CANCELLED);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn wake_of_a_reply_object_does_not_count_as_an_examined_slot() {
    let mut k = K::with_ipc();
    k.ok(retype(UT, CSpace, 256, s(30)));
    k.go(grow(30)).unwrap();
    k.go(grow(30)).unwrap();
    // A reply object in slot 127 with a receiver, a notify in slot 129.
    k.ok(retype(UT, Reply, 0, via(30, 127)));
    let server = k.spawn(70);
    let waits = Syscall::Recv {
        endpoint: s(EP),
        reply: via(30, 127),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.on(server, waits), WAITS);
    k.ok(retype(UT, Notify, 0, via(30, 129)));
    k.weak(30, 31, Rights::ALL);
    k.ok(derive(via(30, 129), s(50), Rights::ALL, 0));
    // Slots 1 to 128 are the 128 the call examines: the notify is found
    // without examining a slot, and removed in the same call.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    assert_eq!(k.woken(server), Some(Err(Error::Gone)));
    assert_eq!(k.kind(50), Err(Error::Gone));
    k.ok(Syscall::CapDelete(s(30)));
}

#[test]
fn late_answer_through_a_reply_object_received_on_again_reaches_the_next_call() {
    let mut k = K::with_ipc();
    let (client, server) = (k.spawn(40), k.spawn(41));
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.on(client, call(EP, msg(1, vec![]), Range::NONE)), WAITS);
    k.ok(cancel(40));
    // The server gives the reply object to `recv` again before it answers.
    assert_eq!(k.on(server, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.on(client, call(EP, msg(2, vec![]), Range::NONE)), WAITS);
    assert_eq!(got(k.woken(server).unwrap()).0, 2);
    // Its answer to the first request goes to the second call.
    let late = k.on(server, send(RP, msg(9, vec![])));
    assert_eq!(late, Ok(Output::Sent { flags: 0 }));
    assert_eq!(got(k.woken(client).unwrap()).0, 9);
}

/// Removal ends: "a call makes at most 32 walks, so it wakes at most 64
/// threads". A walk that marks a reply object ends at that object or at a
/// copy of it, never at an object with a queue: here one call wakes the
/// receiver of the reply object, then 31 callers, 32 threads in all.
#[test]
fn one_call_of_a_removal_wakes_a_reply_object_s_thread_and_then_31_callers() {
    let mut k = K::with_ipc();
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    k.ok(retype(UT, Reply, 0, via(30, 1)));
    k.ok(retype(UT, Endpoint, 0, via(30, 2)));
    let server = k.spawn(39);
    let waits = Syscall::Recv {
        endpoint: s(EP),
        reply: via(30, 1),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.on(server, waits), WAITS);
    let callers: Vec<ObjId> = (40..80).map(|slot| k.spawn(slot)).collect();
    for &caller in &callers {
        let queued = Syscall::Call {
            endpoint: via(30, 2),
            message: msg(1, vec![]),
            range: Range::NONE,
            wait: 0,
        };
        assert_eq!(k.on(caller, queued), WAITS);
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    let woken = callers.iter().filter(|&&caller| k.woken(caller).is_some());
    assert_eq!(k.woken(server), Some(Err(Error::Gone)));
    assert_eq!(woken.count(), 31);
}

#[test]
fn recv_with_a_busy_reply_object_leaves_a_pending_notification() {
    let mut k = K::with_ipc();
    let (server, caller) = (k.spawn(40), k.spawn(41));
    k.make(Endpoint, 30);
    k.ok(bind(NT, s(EP)));
    // Attached to a thread that waits to receive, then armed with a caller.
    assert_eq!(k.on(server, recv(30, RP, Range::NONE)), WAITS);
    k.ok(Syscall::Signal(s(NT)));
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
    assert_eq!(k.on(caller, call(30, msg(1, vec![]), Range::NONE)), WAITS);
    k.no(recv(EP, RP, Range::NONE), Error::Busy);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

#[test]
fn notification_has_label_0_no_words_no_marks_and_task_0() {
    let mut k = K::with_ipc();
    k.ok(bind(NT, s(EP)));
    k.ok(Syscall::Signal(s(NT)));
    let header = Header::new(0, 0, 0, Header::FLAG_NOTIFICATION).unwrap();
    let received = Received {
        header,
        badge: 1,
        marks: Marks::NONE,
        task: 0,
    };
    let words = Vec::new();
    let delivery = Delivery { received, words };
    assert_eq!(
        k.go(recv(EP, RP, Range::NONE)),
        Ok(Output::Message(delivery))
    );
}

#[test]
fn rights_word_with_a_bit_that_names_no_right_is_invalid_after_its_slot() {
    let mut k = K::with_ipc();
    let unknown = Rights::ALL.bits() | 1 << 8;
    let raw = |source, destination, rights| Syscall::CapDerive {
        source,
        destination,
        rights,
        badge: 0,
    };
    k.no(raw(s(EP), s(30), unknown), Error::Invalid);
    k.no(raw(s(EP), s(30), 1 << 31), Error::Invalid);
    // A source without `derive` is `denied` whatever the word.
    let no_derive = k.weak(EP, 31, caps_without(Rights::DERIVE));
    k.no(raw(s(no_derive), s(30), unknown), Error::Denied);
    // The destination comes first: through a cspace capability without
    // `write`.
    let read_only = k.weak(START_SLOT_CSPACE, 32, Rights::READ);
    k.no(raw(s(EP), via(read_only, 30), unknown), Error::Denied);
    // The bit before the subset, and before the state.
    let weak = k.weak(EP, 33, Rights::SEND.union(Rights::DERIVE));
    k.no(raw(s(weak), s(30), unknown), Error::Invalid);
    k.no(
        raw(s(weak), s(30), Rights::SEND.bits() | 1 << 8),
        Error::Invalid,
    );
    k.no(raw(s(EP), s(NT), unknown), Error::Invalid);
    k.ok(raw(s(EP), s(30), Rights::ALL.bits()));
}

#[test]
fn rights_word_of_a_listed_slot_is_judged_with_its_slot() {
    let mut k = K::with_ipc();
    let unknown = Rights::ALL.bits() | 1 << 8;
    let no_derive = k.weak(NT, 30, caps_without(Rights::DERIVE));
    k.weak(RP, 31, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(RP)));
    let weak = k.weak(NT, 32, Rights::SEND.union(Rights::DERIVE));
    let listing = |caps| send(EP, msg(1, caps));
    let (bad, dead) = (Transfer::Derive(s(NT), unknown), Transfer::Move(s(31)));
    let no_derive = Transfer::Derive(s(no_derive), unknown);
    k.no(listing(vec![no_derive]), Error::Denied);
    k.no(listing(vec![bad]), Error::Invalid);
    k.no(listing(vec![bad, dead]), Error::Invalid);
    k.no(listing(vec![dead, bad]), Error::Gone);
    k.no(
        listing(vec![Transfer::Derive(s(weak), unknown)]),
        Error::Invalid,
    );
    let whole = Transfer::Derive(s(NT), Rights::ALL.bits());
    k.no(listing(vec![whole]), Error::WouldWait);
}

// The D-110 changes to the groups of the decided subsections.

fn configured(k: &mut K, thread: u32, page: SlotAddr) -> Outcome {
    let none = SlotAddr::NONE;
    let mut call = configure(thread, SPACE, none, none);
    if let Syscall::ThreadConfigure { page: given, .. } = &mut call {
        *given = page;
    }
    k.go(call)
}

/// A thread in `slot` of the first space, bound to it, with the frame in
/// `page` as its message page if `page` is not 0, and running.
fn runs_with_page(k: &mut K, slot: u32, page: u32) -> ObjId {
    k.make(Thread, slot);
    let page = if page == 0 { SlotAddr::NONE } else { s(page) };
    assert_eq!(configured(k, slot, page), DONE);
    k.schedule(slot);
    k.ok(Syscall::ThreadResume(s(slot)));
    k.m.object_at(k.t, s(slot)).unwrap()
}

/// A message of `words` words, 1 to `words`.
fn long(words: u64) -> Message {
    Message {
        label: 1,
        words: (1..=words).collect(),
        ..Message::default()
    }
}

#[test]
fn message_page_is_a_frame_of_one_page_of_ordinary_memory() {
    let mut k = K::new();
    k.make(Thread, 30);
    let wb = MemoryType::WriteBack;
    k.ok(frame(UT, PageSize::Page, 2, wb, s(31)));
    k.run(k.m.geometry().chunk_shift(), 29);
    k.made(frame(s(29), PageSize::Large1, 1, wb, s(32)));
    k.ok(frame(DEV, PageSize::Page, 1, MemoryType::Uncached, s(33)));
    k.make(Endpoint, 34);
    for slot in [31, 32, 33, 34] {
        assert_eq!(
            configured(&mut k, 30, s(slot)),
            Err(Error::Invalid),
            "{slot}"
        );
    }
    k.make(Frame, 35);
    assert_eq!(configured(&mut k, 30, s(35)), DONE);
}

#[test]
fn message_page_needs_read_and_write_judged_after_its_size() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.make(Frame, 31);
    let read = k.weak(31, 32, Rights::READ);
    let write = k.weak(31, 33, Rights::WRITE);
    assert_eq!(configured(&mut k, 30, s(read)), Err(Error::Denied));
    assert_eq!(configured(&mut k, 30, s(write)), Err(Error::Denied));
    // A frame of two pages without the rights: its size first.
    k.ok(frame(UT, PageSize::Page, 2, MemoryType::WriteBack, s(34)));
    let small = k.weak(34, 35, Rights::READ);
    assert_eq!(configured(&mut k, 30, s(small)), Err(Error::Invalid));
}

#[test]
fn message_page_bound_to_another_thread_is_busy_and_to_the_same_one_accepted() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.make(Thread, 31);
    k.make(Frame, 32);
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    assert_eq!(configured(&mut k, 31, s(32)), Err(Error::Busy));
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    // A copy of the bound capability has an empty record.
    let copy = k.weak(32, 33, Rights::ALL);
    assert_eq!(configured(&mut k, 31, s(copy)), DONE);
}

#[test]
fn thread_that_runs_is_busy_before_its_message_page_is_looked_at() {
    let mut k = K::new();
    k.make(Frame, 32);
    k.make(Thread, 30);
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    runs_with_page(&mut k, 31, 0);
    // The frame is bound to thread 30, and thread 31 runs: `busy` either
    // way; a frame of the wrong size is `invalid` first.
    assert_eq!(configured(&mut k, 31, s(32)), Err(Error::Busy));
    k.ok(frame(UT, PageSize::Page, 2, MemoryType::WriteBack, s(33)));
    assert_eq!(configured(&mut k, 31, s(33)), Err(Error::Invalid));
}

#[test]
fn message_page_binding_moves_with_its_capability() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.make(Thread, 31);
    k.make(Frame, 32);
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    k.ok(mv(s(32), s(40)));
    assert_eq!(configured(&mut k, 31, s(40)), Err(Error::Busy));
    assert_eq!(configured(&mut k, 30, s(40)), DONE);
}

#[test]
fn page_0_unbinds_the_message_page_and_another_page_frees_the_old_one() {
    let mut k = K::new();
    k.make(Thread, 30);
    k.make(Thread, 31);
    k.make(Frame, 32);
    k.make(Frame, 33);
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    assert_eq!(configured(&mut k, 30, s(33)), DONE);
    assert_eq!(configured(&mut k, 31, s(32)), DONE);
    assert_eq!(configured(&mut k, 31, s(33)), Err(Error::Busy));
    assert_eq!(configured(&mut k, 30, SlotAddr::NONE), DONE);
    assert_eq!(configured(&mut k, 31, s(33)), DONE);
}

#[test]
fn removing_the_frame_capability_unbinds_the_message_page_in_the_same_step() {
    let mut k = K::with_ipc();
    k.make(Frame, 31);
    let copy = k.weak(31, 32, Rights::ALL);
    let thread = runs_with_page(&mut k, 30, copy);
    assert_eq!(k.on(thread, send(EP, long(7))), Err(Error::WouldWait));
    // Revoking the parent removes the copy that binds the page.
    k.ok(Syscall::CapRevoke(s(31)));
    assert_eq!(k.on(thread, send(EP, long(7))), Err(Error::Invalid));
    assert_eq!(k.on(thread, send(EP, long(6))), Err(Error::WouldWait));
}

#[test]
fn add_marks_is_the_only_flag_a_sender_may_set() {
    let mut k = K::with_ipc();
    let marked = |flags| Message {
        flags,
        marks: Marks::UNTRUSTED.bits(),
        ..msg(1, vec![])
    };
    let add = Header::FLAG_ADD_MARKS;
    k.no(send(EP, marked(add)), Error::WouldWait);
    for flag in [DROPPED, Header::FLAG_NOTIFICATION, 1 << 3] {
        k.no(send(EP, marked(add | flag)), Error::Invalid);
    }
}

#[test]
fn added_marks_word_with_a_reserved_bit_is_invalid_only_with_the_flag() {
    let mut k = K::with_ipc();
    let word = |flags| Message {
        flags,
        marks: 1 << 2,
        ..msg(1, vec![])
    };
    k.no(send(EP, word(Header::FLAG_ADD_MARKS)), Error::Invalid);
    k.no(send(EP, word(0)), Error::WouldWait);
}

#[test]
fn header_that_needs_a_message_page_is_invalid_for_a_thread_without_one() {
    let mut k = K::with_ipc();
    let thread = runs_with_page(&mut k, 30, 0);
    assert_eq!(k.on(thread, send(EP, long(6))), Err(Error::WouldWait));
    assert_eq!(k.on(thread, send(EP, long(7))), Err(Error::Invalid));
    let one = msg(1, vec![Transfer::Derive(s(NT), Rights::ALL.bits())]);
    assert_eq!(k.on(thread, send(EP, one)), Err(Error::Invalid));
    let marked = Message {
        flags: Header::FLAG_ADD_MARKS,
        ..msg(1, vec![])
    };
    assert_eq!(k.on(thread, send(EP, marked)), Err(Error::Invalid));
}

#[test]
fn need_of_a_message_page_is_judged_with_the_header_before_the_listed_slots() {
    let mut k = K::with_ipc();
    k.weak(NT, 31, Rights::ALL);
    k.ok(Syscall::CapRevoke(s(NT)));
    let thread = runs_with_page(&mut k, 30, 0);
    let dead = msg(1, vec![Transfer::Move(s(31))]);
    assert_eq!(
        k.on(thread, call(EP, dead.clone(), range(0, 9))),
        Err(Error::Invalid)
    );
    // With a page, the dead slot is judged next.
    k.ok(Syscall::ThreadSuspend(s(30)));
    k.make(Frame, 32);
    assert_eq!(configured(&mut k, 30, s(32)), DONE);
    k.ok(Syscall::ThreadResume(s(30)));
    assert_eq!(k.on(thread, call(EP, dead, range(0, 9))), Err(Error::Gone));
}

#[test]
fn receiver_without_a_message_page_gets_the_register_words_only() {
    let mut k = K::with_ipc();
    let receiver = runs_with_page(&mut k, 30, 0);
    assert_eq!(k.on(receiver, recv(EP, RP, Range::NONE)), WAITS);
    assert_eq!(k.go(send(EP, long(10))), Ok(Output::Sent { flags: 0 }));
    let Some(Ok(Output::Message(delivery))) = k.woken(receiver) else {
        panic!("not a message");
    };
    assert_eq!(delivery.words, [1, 2, 3, 4, 5, 6]);
    assert_eq!(delivery.received.header.words(), 10);
}

#[test]
fn caller_whose_message_page_is_unbound_while_it_waits_loses_its_list_and_page_words() {
    let mut k = K::with_ipc();
    k.make(Frame, 31);
    let caller = runs_with_page(&mut k, 30, 31);
    let message = Message {
        caps: vec![Transfer::Derive(s(NT), Rights::ALL.bits())],
        ..long(8)
    };
    assert_eq!(k.on(caller, call(EP, message, Range::NONE)), WAITS);
    k.ok(Syscall::CapDelete(s(31)));
    let Ok(Output::Message(delivery)) = k.go(recv(EP, RP, range(100, 1))) else {
        panic!("not a message");
    };
    assert_eq!(delivery.words, [1, 2, 3, 4, 5, 6, 0, 0]);
    let header = delivery.received.header;
    assert_eq!((header.caps(), header.flags()), (0, DROPPED));
    assert_eq!(k.kind(100), Err(Error::Invalid));
}

/// A child untyped of 1 MB in `slot`, whose memory is dirty: an object was
/// made from it and destroyed.
fn dirty(k: &mut K, slot: u32) {
    k.ok(retype(UT, Untyped, 1 << 20, s(slot)));
    k.ok(retype(s(slot), Endpoint, 0, s(slot + 1)));
    k.ok(Syscall::CapDelete(s(slot + 1)));
}

/// The pages of a number of kilobytes, on the geometry of the run.
fn kb_pages(k: &K) -> impl Fn(u64) -> u64 + use<> {
    let page = k.m.geometry().page();
    move |kb| (kb << 10) / page
}

fn pages(untyped: u32, count: u64, slot: u32) -> Syscall {
    let wb = MemoryType::WriteBack;
    frame(s(untyped), PageSize::Page, count, wb, s(slot))
}

#[test]
fn creation_from_dirty_memory_clears_64_kb_per_call_and_makes_nothing_until_the_end() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    // 128 KB: two calls.
    k.no(pages(30, kb(128), 32), Error::Again);
    assert_eq!(k.kind(32), Err(Error::Invalid));
    k.ok(pages(30, kb(128), 32));
    // 192 KB: three.
    k.no(pages(30, kb(192), 33), Error::Again);
    k.no(pages(30, kb(192), 33), Error::Again);
    k.ok(pages(30, kb(192), 33));
    // An endpoint after it: one call.
    k.ok(retype(s(30), Endpoint, 0, s(34)));
}

#[test]
fn work_of_a_clearing_step_does_not_depend_on_the_arguments() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    k.no(pages(30, kb(128), 32), Error::Again);
    // The first 64 KB are clear: an endpoint is made at once, at the
    // watermark, and the frame then starts at a page and ends 128 KB
    // later: two more calls.
    k.ok(retype(s(30), Endpoint, 0, s(33)));
    k.no(pages(30, kb(128), 32), Error::Again);
    k.ok(pages(30, kb(128), 32));
}

#[test]
fn every_check_comes_before_the_clearing() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    k.no(pages(30, kb(128), 30), Error::NoSlot);
    let entries = u64::from(k.m.geometry().entries);
    let too_many = (2 << 20) / k.m.geometry().page() + 1;
    k.no(pages(30, too_many.min(entries), 32), Error::NoMemory);
    // Nothing was cleared: 128 KB still take two calls.
    k.no(pages(30, kb(128), 32), Error::Again);
    k.ok(pages(30, kb(128), 32));
}

#[test]
fn child_untyped_is_not_cleared_and_keeps_what_its_parent_had_cleared() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    // The parent clears its first 64 KB, then gives its first 128 KB to a
    // child at once.
    k.no(pages(30, kb(128), 32), Error::Again);
    k.ok(retype(s(30), Untyped, 1 << 17, s(33)));
    // The child knows its first 64 KB are clear: 128 KB in one call.
    k.ok(pages(33, kb(128), 34));
    // The parent's clean mark moved with its watermark: the next 64 KB in
    // one call.
    k.ok(pages(30, kb(64), 35));
}

#[test]
fn memory_is_dirty_again_once_the_watermark_returns_to_the_start() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    k.ok(retype(UT, Untyped, 1 << 20, s(30)));
    k.made(pages(30, kb(128), 32));
    k.ok(Syscall::CapDelete(s(32)));
    k.no(pages(30, kb(128), 32), Error::Again);
    k.ok(pages(30, kb(128), 32));
}

#[test]
fn device_memory_is_never_cleared() {
    let mut k = K::new();
    let uncached = MemoryType::Uncached;
    // Half of the 2 MB device range, in one call.
    let pages = (1 << 20) / k.m.geometry().page();
    k.ok(frame(DEV, PageSize::Page, pages, uncached, s(30)));
    k.ok(Syscall::CapDelete(s(30)));
    k.ok(frame(DEV, PageSize::Page, pages, uncached, s(30)));
}

#[test]
fn cspace_grow_clears_its_segment_within_the_call() {
    let mut k = K::new();
    dirty(&mut k, 30);
    k.ok(retype(s(30), CSpace, 256, s(32)));
    for slots in [128, 256] {
        assert_eq!(k.go(grow_from(32, s(30))), Ok(Output::Slots(slots)));
    }
}

/// An empty capability space in slot 30, and `count` suspended threads in
/// the slots from 40 bound to it, the first one first.
fn space_with_threads(k: &mut K, count: u32) {
    k.make(CSpace, 30);
    k.go(grow(30)).unwrap();
    for slot in 40..40 + count {
        k.make(Thread, slot);
        k.ok(configure(slot, s(30), SlotAddr::NONE, SlotAddr::NONE));
    }
}

#[test]
fn destroying_a_cspace_unbinds_its_threads_one_unit_each_most_recent_first() {
    let mut k = K::new();
    space_with_threads(&mut k, 33);
    // 32 units: the threads of slots 72 down to 41.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.no(Syscall::ThreadResume(s(72)), Error::Busy);
    k.no(Syscall::ThreadResume(s(41)), Error::Busy);
    // The first thread is still bound, to a dying space: it can be resumed.
    k.ok(Syscall::ThreadResume(s(40)));
    k.ok(Syscall::CapDelete(s(30)));
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.no(Syscall::ThreadResume(s(40)), Error::Busy);
}

#[test]
fn thread_whose_space_is_destroyed_is_bound_again_by_thread_configure() {
    let mut k = K::new();
    space_with_threads(&mut k, 1);
    k.ok(Syscall::CapDelete(s(30)));
    k.no(Syscall::ThreadResume(s(40)), Error::Busy);
    k.ok(configure(40, SPACE, SlotAddr::NONE, SlotAddr::NONE));
    k.ok(Syscall::ThreadResume(s(40)));
    let thread = k.m.object_at(k.t, s(40)).unwrap();
    assert_eq!(
        k.on(thread, Syscall::CapInspect(s(40))),
        Ok(Output::Inspect {
            kind: Thread,
            rights: Rights::ALL,
            badge: 0,
        })
    );
}

#[test]
fn destroying_an_endpoint_unbinds_the_threads_whose_fault_endpoint_it_is() {
    let mut k = K::with_ipc();
    for slot in 40..73 {
        k.make(Thread, slot);
        k.ok(configure(slot, SPACE, SlotAddr::NONE, s(EP)));
    }
    k.no(Syscall::CapDelete(s(EP)), Error::Again);
    k.ok(Syscall::CapDelete(s(EP)));
}

#[test]
fn destroying_an_aspace_unbinds_its_threads() {
    let mut k = K::new();
    k.make(ASpace, 31);
    for slot in 40..73 {
        k.make(Thread, slot);
        k.ok(configure(slot, SPACE, s(31), SlotAddr::NONE));
    }
    k.no(Syscall::CapDelete(s(31)), Error::Again);
    k.ok(Syscall::CapDelete(s(31)));
}

#[test]
fn removal_of_a_frame_capability_and_its_binding_is_one_unit() {
    let mut k = K::new();
    k.make(Frame, 30);
    // 31 copies, the last one bound: 32 units with the root.
    for slot in 40..71 {
        k.weak(30, slot, Rights::ALL);
    }
    k.make(Thread, 31);
    assert_eq!(configured(&mut k, 31, s(70)), DONE);
    k.ok(Syscall::CapDelete(s(30)));
    k.make(Frame, 32);
    assert_eq!(configured(&mut k, 31, s(32)), DONE);
}

#[test]
fn child_untyped_of_dirty_memory_is_made_in_one_call() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    k.ok(retype(s(30), Untyped, 1 << 19, s(32)));
    // Its own memory is dirty: 128 KB from it take two calls.
    k.no(pages(32, kb(128), 33), Error::Again);
    k.ok(pages(32, kb(128), 33));
}

// The settled problems of pass 11: the clean mark.

#[test]
fn step_that_reaches_the_end_stops_there() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    // 96 KB: two steps, the second stops at 96 KB.
    k.no(pages(30, kb(96), 32), Error::Again);
    k.ok(pages(30, kb(96), 32));
    // The next 96 KB start from there: two steps again. Had the step
    // cleared a whole 64 KB, to 128 KB, one would have been enough.
    k.no(pages(30, kb(96), 33), Error::Again);
    k.ok(pages(30, kb(96), 33));
}

#[test]
fn padding_is_cleared_with_the_object() {
    let mut k = K::new();
    let kb = kb_pages(&k);
    dirty(&mut k, 30);
    // An endpoint at 0, then a frame of 64 KB aligned on a page: the
    // padding before it and the frame are more than 64 KB, so two steps.
    k.ok(retype(s(30), Endpoint, 0, s(31)));
    k.no(pages(30, kb(64), 32), Error::Again);
    k.ok(pages(30, kb(64), 32));
}

// Lot 3, F-01 and lot 4, L4-07 of pass 32's audit. An untyped root that
// has a descendant does not move: its copies name its slot (The capability
// slot, D-131). `cap_move` of it is `busy`, whether the descendant is an
// object made from it or a copy of it.
#[test]
fn cap_move_of_an_untyped_root_that_has_a_descendant_is_busy() {
    let mut k = K::new();
    k.make(Notify, 31);
    k.no(mv(UT, s(40)), Error::Busy);
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 1 << 16, s(33)));
    k.ok(derive(s(33), s(31), Rights::ALL, 0));
    k.no(mv(s(33), s(40)), Error::Busy);
    // A copy is no root: it moves.
    k.ok(mv(s(31), s(41)));
}

// A fresh untyped, the usual thing a creator hands on, has no descendant
// and moves freely; once its only descendant is deleted it moves again.
#[test]
fn an_untyped_root_with_no_descendant_moves() {
    let mut k = K::new();
    k.ok(retype(UT, Untyped, 1 << 16, s(33)));
    k.ok(mv(s(33), s(34)));
    k.ok(derive(s(34), s(31), Rights::ALL, 0));
    k.no(mv(s(34), s(35)), Error::Busy);
    k.ok(Syscall::CapDelete(s(31)));
    k.ok(mv(s(34), s(35)));
}

// A *move* in a message of the root capability of an untyped that has a
// descendant is `busy`, as for `cap_move`, checked when the call is made:
// an error, and nothing is sent (Transfer of capabilities, D-131).
#[test]
fn a_move_by_ipc_of_an_untyped_root_that_has_a_descendant_is_busy() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    let moved = msg(1, vec![Transfer::Move(UT)]);
    assert_eq!(k.on(caller, call(EP, moved, Range::NONE)), Err(Error::Busy));
    assert_eq!(
        k.on(caller, send(EP, msg(1, vec![Transfer::Move(UT)]))),
        Err(Error::Busy)
    );
    assert_eq!(k.inspect(UT).unwrap().0, Untyped);
}

// The same check is made again at delivery, against what the sender's
// slots hold then: an untyped root that got a descendant while its caller
// waited is not moved, and the capabilities are dropped.
#[test]
fn an_untyped_root_that_got_a_descendant_before_delivery_is_dropped() {
    let mut k = K::with_ipc();
    k.ok(retype(UT, Untyped, 1 << 16, s(33)));
    caller_with(&mut k, vec![Transfer::Move(s(33))], Range::NONE);
    k.ok(derive(s(33), s(31), Rights::ALL, 0));
    assert_eq!(got(k.go(recv(EP, RP, range(50, 2)))), (1, 0, 0, DROPPED));
    assert_eq!(k.inspect(s(33)).unwrap().0, Untyped);
    assert_eq!(k.kind(50), Err(Error::Invalid));
}

// Pass 34, R-1 of the review of pass 32. The `busy` of a *move* of an
// untyped root that has a descendant is a state check (Order of checks,
// step 4): `call` judges its receive range before it, a plain argument
// (step 3), so a range of more than 4 slots is `invalid`.
#[test]
fn a_call_judges_its_receive_range_before_the_busy_of_a_move() {
    let mut k = K::with_ipc();
    let caller = k.spawn(40);
    let moved = msg(1, vec![Transfer::Move(UT)]);
    assert_eq!(
        k.on(caller, call(EP, moved, range(50, 5))),
        Err(Error::Invalid)
    );
}

// Pass 34, R-1: `reply_recv` judges its endpoint (a capability argument,
// step 2) before the `busy` of a *move* (step 4): an empty endpoint slot
// is `invalid`.
#[test]
fn a_reply_recv_judges_its_endpoint_before_the_busy_of_a_move() {
    let mut k = K::with_ipc();
    let moved = msg(1, vec![Transfer::Move(UT)]);
    k.no(reply_recv(RP, moved, 77), Error::Invalid);
}

// Finding 4 of the review of passes 33 and 34: `reply_recv` judges the
// `busy` of a *move* of an untyped root that has a descendant (Order of
// checks, step 4), as `call` and `send` do.
#[test]
fn a_reply_recv_that_moves_an_untyped_root_with_a_descendant_is_busy() {
    let mut k = K::with_ipc();
    let moved = msg(1, vec![Transfer::Move(UT)]);
    k.no(reply_recv(RP, moved, EP), Error::Busy);
    assert_eq!(k.inspect(UT).unwrap().0, Untyped);
}
