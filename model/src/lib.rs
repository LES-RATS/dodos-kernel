//! Executable reference model of the DodOS kernel, for one core.
//!
//! The contract is `architecture/kernel-interface.md` in the documentation
//! repository. A comment that names a section names a section of its "Call
//! reference". The kernel is tested against this model call by call:
//! [`Model::apply`] applies one call made by one thread, and
//! [`Model::check`] checks every invariant of the state.
//!
//! Not modelled: the contents of memory, and the hardware (translations are
//! a map; the invalidations of the TLB and of the IOMMU are done before a
//! call returns).
//!
//! `CONTRACT-GAP` marks a behaviour that the contract does not fix.

#![forbid(unsafe_code)]

mod caps;
mod check;
mod ipc;
mod threads;

pub use check::Census;

use dodos_abi::{
    CSPACE_SEGMENT_SLOTS, Call, Error, MAX_CAPS, MemoryType, ObjectKind, PageSize, Received,
    Rights, START_SLOT_ASPACE, START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_FIRST,
    START_SLOT_MESSAGE_PAGE, START_SLOT_SCHED, START_SLOT_THREAD, SlotAddr,
};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;

/// Bytes of the ordinary untyped, in slot `START_SLOT_FIRST`.
pub const BOOT_UNTYPED_BYTES: u64 = 4 << 30;
/// Bytes of the device untyped, in slot `START_SLOT_FIRST + 1`.
pub const BOOT_DEVICE_BYTES: u64 = 1 << 20;

/// Identity of a kernel object. It is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjId(u32);

/// Identity of a capability. It is never reused and only grows, so a
/// capability is younger than its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CapId(u32);

/// One capability listed in a message (Transfer of capabilities).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transfer {
    Move(SlotAddr),
    /// With the rights word as the sender gives it: a bit that names no
    /// right is `invalid`.
    Derive(SlotAddr, u32),
}

/// A message as its sender gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub label: u32,
    /// The flag bits of the header. Order of checks: a sender may set only
    /// `Header::FLAG_ADD_MARKS`.
    pub flags: u32,
    pub words: Vec<u64>,
    pub caps: Vec<Transfer>,
    /// The added-marks word of the message page, read only when the flag
    /// `add_marks` is set (Tasks and marks).
    pub marks: u64,
}

/// The receive range: where capabilities that arrive are placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub first: SlotAddr,
    pub count: u32,
}

impl Range {
    pub const NONE: Self = Self {
        first: SlotAddr::NONE,
        count: 0,
    };
}

/// A message as its receiver gets it. The capabilities are in the first
/// `received.header.caps()` slots of its receive range. Bindings: a
/// receiver without a message page gets the register words only, and the
/// header keeps the number of words sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub received: Received,
    pub words: Vec<u64>,
}

/// One system call with its arguments, in the order of the contract's tables.
#[derive(Clone, Debug, PartialEq)]
pub enum Syscall {
    CapDerive {
        source: SlotAddr,
        destination: SlotAddr,
        /// The rights word as the caller gives it: a bit that names no right
        /// is `invalid`.
        rights: u32,
        badge: u64,
    },
    CapMove {
        source: SlotAddr,
        destination: SlotAddr,
    },
    CapDelete(SlotAddr),
    CapRevoke(SlotAddr),
    CapInspect(SlotAddr),
    /// `size` is the size argument; for a frame it is the number of pages,
    /// of the page size `page` and the memory type `memory`, raw words that
    /// no other kind reads.
    UntypedRetype {
        untyped: SlotAddr,
        kind: ObjectKind,
        size: u64,
        page: u64,
        memory: u64,
        destination: SlotAddr,
    },
    CSpaceGrow {
        cspace: SlotAddr,
        untyped: SlotAddr,
    },
    ThreadConfigure {
        thread: SlotAddr,
        cspace: SlotAddr,
        aspace: SlotAddr,
        fault: SlotAddr,
        page: SlotAddr,
    },
    ThreadResume(SlotAddr),
    ThreadSuspend(SlotAddr),
    /// The thread, and a wait number.
    ThreadCancel(SlotAddr, u64),
    /// `wait`, here and below, is the wait number of the call; 0 is none.
    Call {
        endpoint: SlotAddr,
        message: Message,
        range: Range,
        wait: u64,
    },
    Recv {
        endpoint: SlotAddr,
        reply: SlotAddr,
        range: Range,
        wait: u64,
    },
    ReplyRecv {
        reply: SlotAddr,
        message: Message,
        endpoint: SlotAddr,
        range: Range,
        wait: u64,
    },
    Send {
        target: SlotAddr,
        message: Message,
    },
    Signal(SlotAddr),
    /// The notify, and the wait number.
    Wait(SlotAddr, u64),
    Poll(SlotAddr),
    NotifyBind {
        notify: SlotAddr,
        endpoint: SlotAddr,
    },
    /// A call that this model does not make: it returns `Unsupported`. A
    /// call that has a variant above is refused here with a panic.
    Other(Call),
}

/// The results of a call that did not fail.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// The call has no result.
    Done,
    /// `cap_inspect`.
    Inspect {
        kind: ObjectKind,
        rights: Rights,
        badge: u64,
    },
    /// `cspace_grow`: the new number of slots.
    Slots(u32),
    /// `wait`, `poll`: the word of the notify.
    Word(u64),
    /// `send`: 0, or `Header::FLAG_CAPS_DROPPED`.
    Sent { flags: u32 },
    /// `recv`, `reply_recv`: a message or a notification. `call`: the reply.
    Message(Delivery),
    /// The thread waits. The outcome of its call comes later, in
    /// [`Model::woken`].
    Waits,
}

/// The error code and the results of a call. `Err(Error::Ok)` never occurs.
pub type Outcome = Result<Output, Error>;

/// A place for a capability: a capability space and an index in it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    space: ObjId,
    index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Empty,
    Full(CapId),
    Dead,
}

/// Mapping records: what a frame capability holds besides its object.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Record {
    #[default]
    Empty,
    /// Bindings: the message page of this thread.
    Binding(ObjId),
}

#[derive(Clone, Debug, PartialEq)]
struct Cap {
    object: ObjId,
    rights: Rights,
    badge: u64,
    parent: Option<CapId>,
    /// In the order of derivation, the most recent last.
    children: Vec<CapId>,
    place: Place,
    /// Removal ends: set by the first step of `cap_delete` on the capability,
    /// and on a root capability by the first step of a removal that reaches
    /// it.
    dying: bool,
    /// Empty but on a frame capability.
    record: Record,
}

/// What every object has. The state of an object that has one is in the map
/// of its kind.
#[derive(Clone, Debug, PartialEq)]
struct Object {
    kind: ObjectKind,
    /// The capability the object lives as long as.
    root: CapId,
    /// The untyped its memory came from; `None` for a start-up object.
    from: Option<ObjId>,
}

#[derive(Clone, Debug, PartialEq)]
struct Untyped {
    bytes: u64,
    device: bool,
    watermark: u64,
    /// Rules of `untyped_retype`: the memory from the watermark to the
    /// clean mark is known to be zero. Device memory has none: it stays 0.
    clean: u64,
    /// Objects, segments of capability spaces and tables of address spaces
    /// taken from it and not yet destroyed.
    live: u32,
}

/// Frames: fixed by `untyped_retype` and never changed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    page: PageSize,
    pages: u64,
    memory: MemoryType,
}

#[derive(Clone, Debug, PartialEq)]
struct CSpace {
    max: u32,
    slots: Vec<Slot>,
    /// The derivation tree: `Some` once the space is dying.
    sweep: Option<Sweep>,
    /// Bindings: the threads bound to the space, the most recent last.
    threads: Vec<ObjId>,
}

/// The derivation tree: what a dying capability space keeps.
#[derive(Clone, Debug, PartialEq)]
struct Sweep {
    /// The index where the sweep stands: the slots before it were examined
    /// and are not examined again.
    next: usize,
    /// The capabilities the space held when it began to die. Removal ends:
    /// a dying space only shrinks, so invariant 23 refuses any other.
    held: Vec<CapId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ASpace {
    /// A thread and its address space: the threads bound to it, the most
    /// recent last.
    threads: Vec<ObjId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Thread {
    /// Bindings, each a link both ways: the capability space, the address
    /// space, the fault endpoint with the badge of the capability given to
    /// `thread_configure`, and the frame capability whose record binds the
    /// message page.
    cspace: Option<ObjId>,
    aspace: Option<ObjId>,
    fault: Option<(ObjId, u64)>,
    page: Option<CapId>,
    suspended: bool,
    wait: Wait,
    /// Threads: the wait number of the call the thread waits in; 0 if it
    /// gave none, or does not wait.
    number: u64,
    /// Threads: the word "cancelled up to", 0 when the thread is created.
    cancelled_up_to: u64,
    /// Outcome of the call the thread waited in, until its next call.
    outcome: Option<Outcome>,
}

/// IPC: the states of a thread. `None` is ready.
#[derive(Clone, Debug, Default, PartialEq)]
enum Wait {
    #[default]
    None,
    /// Waiting to be received, in the queue of an endpoint.
    Call {
        endpoint: ObjId,
        parcel: Parcel,
        window: Window,
    },
    /// Waiting for a reply, through the reply object armed with the thread.
    Reply(ObjId),
    /// Waiting to receive, in the queue of an endpoint.
    Recv {
        endpoint: ObjId,
        reply: ObjId,
        window: Window,
    },
    /// Waiting on a notify, in its queue.
    Notify(ObjId),
}

/// A message on its way. Transfer of capabilities: the kernel remembers the
/// slots that the sender listed, not what they held.
#[derive(Clone, Debug, PartialEq)]
struct Parcel {
    label: u32,
    words: Vec<u64>,
    badge: u64,
    sender: ObjId,
    caps: Vec<Transfer>,
}

/// A receive range, resolved when the call was made.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    cspace: ObjId,
    first: u32,
    count: u32,
}

/// An endpoint queues callers or receivers, never both.
#[derive(Clone, Debug, Default, PartialEq)]
struct Endpoint {
    queue: VecDeque<ObjId>,
    notify: Option<ObjId>,
    /// Bindings: the threads whose fault endpoint it is, the most recent
    /// last.
    threads: Vec<ObjId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReplyState {
    Idle,
    /// To a thread that waits to receive.
    Attached(ObjId),
    /// With a caller, its receive range, and the flags its reply will carry.
    Armed {
        caller: ObjId,
        window: Window,
        flags: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Notify {
    word: u64,
    waiters: VecDeque<ObjId>,
    endpoint: Option<ObjId>,
}

/// The whole state of the kernel of one core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    next_id: u32,
    caps: BTreeMap<CapId, Cap>,
    objects: BTreeMap<ObjId, Object>,
    untypeds: BTreeMap<ObjId, Untyped>,
    frames: BTreeMap<ObjId, Frame>,
    cspaces: BTreeMap<ObjId, CSpace>,
    aspaces: BTreeMap<ObjId, ASpace>,
    threads: BTreeMap<ObjId, Thread>,
    endpoints: BTreeMap<ObjId, Endpoint>,
    replies: BTreeMap<ObjId, ReplyState>,
    notifies: BTreeMap<ObjId, Notify>,
    /// The derivation tree: the longest walk so far to the next capability
    /// to remove, in capabilities. The contract says: shorter than 192.
    longest_walk: usize,
    /// Large pages: the page sizes the processor supports, bit `i` for
    /// `PageSize` `i`.
    page_sizes: u32,
}

/// Whether a call has a variant of its own in `Syscall`.
fn modelled(call: Call) -> bool {
    use Call::{
        CSpaceGrow, CapDelete, CapDerive, CapInspect, CapMove, CapRevoke, NotifyBind, Poll, Recv,
        ReplyRecv, Send, Signal, ThreadCancel, ThreadConfigure, ThreadResume, ThreadSuspend,
        UntypedRetype, Wait,
    };
    matches!(
        call,
        CapDerive
            | CapMove
            | CapDelete
            | CapRevoke
            | CapInspect
            | UntypedRetype
            | CSpaceGrow
            | ThreadConfigure
            | ThreadResume
            | ThreadSuspend
            | ThreadCancel
            | Call::Call
            | Recv
            | ReplyRecv
            | Send
            | Signal
            | Wait
            | Poll
            | NotifyBind
    )
}

/// The entry `id` of one of the maps of the state, to change it. Every
/// identity the state holds names a live entry (`Model::check`).
fn live<K: Ord + Debug, T>(map: &mut BTreeMap<K, T>, id: K) -> &mut T {
    map.get_mut(&id).expect("names a live entry")
}

impl Model {
    /// The state at start-up, and the first thread, which can run (Start-up
    /// information).
    ///
    /// CONTRACT-GAP (Start-up information): that section is only proposed.
    /// It also gives the first process a `core` capability per core, the
    /// control capabilities for interrupts, I/O ports and IOMMU domains, and
    /// the frames of the boot image; and it leaves the size of the start-up
    /// space and the untyped ranges to the boot image. The model gives none
    /// of the first, one segment, and the two untypeds below. Its capability
    /// space has one segment and holds itself, an `aspace`, the thread, a
    /// `sched` and `debug` in their fixed slots, then one ordinary untyped
    /// and one device untyped.
    pub fn boot() -> (Self, ObjId) {
        let mut model = Self {
            page_sizes: 0b111,
            ..Self::default()
        };
        let space = ObjId(model.next_id);
        let mut add = |kind, size, index, rights| {
            let cap = model.create(kind, size, None, rights, Place { space, index });
            model.caps[&cap].object
        };
        let all = Rights::ALL;
        let segment = CSPACE_SEGMENT_SLOTS.into();
        add(ObjectKind::CSpace, segment, START_SLOT_CSPACE, all);
        let aspace = add(ObjectKind::ASpace, 0, START_SLOT_ASPACE, all);
        let thread = add(ObjectKind::Thread, 0, START_SLOT_THREAD, all);
        add(ObjectKind::Sched, 0, START_SLOT_SCHED, all);
        add(ObjectKind::Debug, 0, START_SLOT_DEBUG, all);
        let ordinary = add(
            ObjectKind::Untyped,
            BOOT_UNTYPED_BYTES,
            START_SLOT_FIRST,
            all,
        );
        // Rules of `untyped_retype`: the start-up capabilities for device
        // memory have no `exec`.
        let no_exec = all.without(Rights::EXEC);
        let device = add(
            ObjectKind::Untyped,
            BOOT_DEVICE_BYTES,
            START_SLOT_FIRST + 1,
            no_exec,
        );
        live(&mut model.untypeds, ordinary).clean = BOOT_UNTYPED_BYTES;
        live(&mut model.untypeds, device).device = true;
        // Start-up state: the frame of the message page of the first thread,
        // which holds its binding.
        let page = model.create(
            ObjectKind::Frame,
            1,
            None,
            all,
            Place {
                space,
                index: START_SLOT_MESSAGE_PAGE,
            },
        );
        let frame = Frame {
            page: PageSize::Size4K,
            pages: 1,
            memory: MemoryType::WriteBack,
        };
        model.frames.insert(model.caps[&page].object, frame);
        model.bind_space(thread, Some(space));
        model.bind_aspace(thread, Some(aspace));
        model.bind_page(thread, Some(page));
        live(&mut model.threads, thread).suspended = false;
        (model, thread)
    }

    /// The threads that can run: not suspended and not waiting. A call can
    /// be applied for any of them.
    pub fn runnable(&self) -> Vec<ObjId> {
        let ready = |thread: &&Thread| !thread.suspended && thread.wait == Wait::None;
        let ids = self.threads.iter().filter(|(_, thread)| ready(thread));
        ids.map(|(&id, _)| id).collect()
    }

    /// The outcome of the call in which `thread` waited, once it is over and
    /// until the thread makes its next call.
    pub fn woken(&self, thread: ObjId) -> Option<&Outcome> {
        self.threads.get(&thread)?.outcome.as_ref()
    }

    /// The derivation tree: the longest walk so far to the next capability
    /// to remove, in capabilities; at most `dodos_abi::LONGEST_WALK`. The
    /// kernel can be measured against it.
    pub fn longest_walk(&self) -> usize {
        self.longest_walk
    }

    /// The object that a slot address names for `thread`, if any: a way to
    /// find the identity of a thread that a call created.
    pub fn object_at(&self, thread: ObjId, slot: SlotAddr) -> Option<ObjId> {
        self.threads.get(&thread)?;
        let cap = self.cap(thread, slot, Rights::READ).ok()?;
        Some(self.caps[&cap].object)
    }

    /// Applies one call made by `thread`, which must be able to run.
    pub fn apply(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        assert!(self.runnable().contains(&thread), "{thread:?} cannot run");
        live(&mut self.threads, thread).outcome = None;
        let number = match call {
            Syscall::Call { wait, .. }
            | Syscall::Recv { wait, .. }
            | Syscall::ReplyRecv { wait, .. }
            | Syscall::Wait(_, wait) => wait,
            _ => 0,
        };
        let outcome = self.dispatch(thread, call)?;
        // Threads: a call that is about to wait with a wait number that is
        // not 0 and not greater than the word of the thread returns
        // `cancelled` instead, with the effects of a wait ended at once: a
        // `call` whose message was delivered has sent it, and the reply
        // object it armed is idle again. A call that ends without waiting
        // does not look at the word.
        if outcome == Output::Waits {
            let waiting = live(&mut self.threads, thread);
            if number != 0 && number <= waiting.cancelled_up_to {
                self.unlink(thread);
                return Err(Error::Cancelled);
            }
            waiting.number = number;
        }
        Ok(outcome)
    }

    fn dispatch(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        match call {
            Syscall::CapDerive {
                source,
                destination,
                rights,
                badge,
            } => self.cap_derive(thread, source, destination, rights, badge),
            Syscall::CapMove {
                source,
                destination,
            } => self.cap_move(thread, source, destination),
            Syscall::CapDelete(slot) => self.cap_delete(thread, slot),
            Syscall::CapRevoke(slot) => self.cap_revoke(thread, slot),
            Syscall::CapInspect(slot) => self.cap_inspect(thread, slot),
            Syscall::UntypedRetype {
                untyped,
                kind,
                size,
                page,
                memory,
                destination,
            } => {
                let size = caps::SizeArgument { size, page, memory };
                self.untyped_retype(thread, untyped, kind, size, destination)
            }
            Syscall::CSpaceGrow { cspace, untyped } => self.cspace_grow(thread, cspace, untyped),
            Syscall::ThreadConfigure {
                thread: target,
                cspace,
                aspace,
                fault,
                page,
            } => {
                let bindings = threads::Bindings {
                    cspace,
                    aspace,
                    fault,
                    page,
                };
                self.thread_configure(thread, target, bindings)
            }
            Syscall::ThreadResume(target) => self.thread_resume(thread, target),
            Syscall::ThreadSuspend(target) => self.thread_suspend(thread, target),
            Syscall::ThreadCancel(target, number) => self.thread_cancel(thread, target, number),
            Syscall::Call {
                endpoint,
                message,
                range,
                wait: _,
            } => self.call(thread, endpoint, message, range),
            Syscall::Recv {
                endpoint,
                reply,
                range,
                wait: _,
            } => self.recv(thread, endpoint, reply, range),
            Syscall::ReplyRecv {
                reply,
                message,
                endpoint,
                range,
                wait: _,
            } => self.reply_recv(thread, reply, message, endpoint, range),
            Syscall::Send { target, message } => self.send(thread, target, message),
            Syscall::Signal(notify) => self.signal(thread, notify),
            Syscall::Wait(notify, _) => self.wait(thread, notify, ipc::IfZero::Wait),
            Syscall::Poll(notify) => self.wait(thread, notify, ipc::IfZero::WouldWait),
            Syscall::NotifyBind { notify, endpoint } => self.notify_bind(thread, notify, endpoint),
            // The calls not yet made by this model, and those of the groups
            // not yet specified (cores, debugging), return `unsupported` as
            // soon as the capability space of the caller is found, before
            // any argument is looked at. Removal ends: a thread whose
            // capability space is dying or destroyed gets `gone` from these
            // calls as from every other.
            Syscall::Other(call) => {
                assert!(!modelled(call), "{call:?} has its own variant of `Syscall`");
                self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
                Err(Error::Unsupported)
            }
        }
    }

    // Order of checks. Each call checks its capability arguments with the
    // functions below, in the order of its table (steps 1 and 2), then its
    // plain arguments (step 3), then the state (step 4).

    /// Slots and addresses: the capability space that an address is in.
    /// Through a `cspace` capability, `right` is the right needed: `READ` to
    /// look at a slot, `WRITE` to change it.
    fn addressed_space(
        &self,
        thread: ObjId,
        addr: SlotAddr,
        right: Rights,
    ) -> Result<ObjId, Error> {
        let own = self.bound_space(thread)?;
        // Order of checks: a thread whose capability space is dying gets
        // `gone` from every call except `cap_delete` on its own slots.
        if self.dying(own) {
            return Err(Error::Gone);
        }
        self.through(own, addr, right)
    }

    /// Order of checks: a thread bound to no capability space gets `gone`
    /// from every call, `cap_delete` too. The derivation tree: a thread
    /// whose capability space is destroyed is bound to none (D-110).
    fn bound_space(&self, thread: ObjId) -> Result<ObjId, Error> {
        self.threads[&thread].cspace.ok_or(Error::Gone)
    }

    /// Slots and addresses: `own`, or the capability space that `via` names
    /// in it.
    fn through(&self, own: ObjId, addr: SlotAddr, right: Rights) -> Result<ObjId, Error> {
        if addr.via() == 0 {
            return Ok(own);
        }
        let via = self.full(own, addr.via())?;
        Ok(self.is(via, ObjectKind::CSpace, right)?.object)
    }

    /// Order of checks, steps 1 and 2, the slot: `invalid` if empty or out
    /// of range, `gone` if dead. Removal ends: `gone` if the capability is
    /// dying, or its object is.
    fn full(&self, cspace: ObjId, index: u32) -> Result<CapId, Error> {
        match self.cspaces[&cspace].slots.get(index as usize) {
            Some(&Slot::Full(cap)) => {
                let held = &self.caps[&cap];
                let dying = held.dying || self.dying(held.object);
                if dying { Err(Error::Gone) } else { Ok(cap) }
            }
            Some(Slot::Dead) => Err(Error::Gone),
            Some(Slot::Empty) | None => Err(Error::Invalid),
        }
    }

    /// Removal ends: an object is dying from the first step of a removal
    /// that reaches its root capability. An object that does not exist, or
    /// has no root capability, is not: `check` reports it.
    fn dying(&self, object: ObjId) -> bool {
        let root = self.objects.get(&object).map(|object| object.root);
        root.is_some_and(|root| self.caps.get(&root).is_some_and(|root| root.dying))
    }

    /// Order of checks, steps 1 and 2: the slot of a capability argument.
    fn cap(&self, thread: ObjId, addr: SlotAddr, right: Rights) -> Result<CapId, Error> {
        self.full(self.addressed_space(thread, addr, right)?, addr.index())
    }

    /// Order of checks, steps 1 and 2: the kind (`invalid`), then the rights
    /// (`denied`).
    fn is(&self, cap: CapId, kind: ObjectKind, rights: Rights) -> Result<&Cap, Error> {
        let held = &self.caps[&cap];
        if self.objects[&held.object].kind != kind {
            return Err(Error::Invalid);
        }
        if !held.rights.contains(rights) {
            return Err(Error::Denied);
        }
        Ok(held)
    }

    /// Order of checks, steps 1 and 2: the three checks of a capability
    /// argument, in order. `need` is the rights that the call needs.
    fn held(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<&Cap, Error> {
        self.is(self.cap(thread, at, Rights::READ)?, kind, need)
    }

    /// As `held`, for a call that needs only the object.
    fn object(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<ObjId, Error> {
        Ok(self.held(thread, at, kind, need)?.object)
    }

    /// As `object`, for an argument that may be 0.
    fn optional(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<Option<ObjId>, Error> {
        match at.is_none() {
            true => Ok(None),
            false => Ok(Some(self.object(thread, at, kind, need)?)),
        }
    }

    /// Order of checks, step 2: a slot in which the call places a
    /// capability. Slots and addresses: slot 0 or beyond the table is
    /// `invalid`.
    fn destination(&self, thread: ObjId, addr: SlotAddr) -> Result<Place, Error> {
        let space = self.addressed_space(thread, addr, Rights::WRITE)?;
        if addr.index() == 0 || addr.index() as usize >= self.cspaces[&space].slots.len() {
            return Err(Error::Invalid);
        }
        Ok(Place {
            space,
            index: addr.index(),
        })
    }

    /// Order of checks, step 4: `no_slot` if the destination is not empty.
    fn vacant(&self, Place { space, index }: Place) -> Result<(), Error> {
        match self.cspaces[&space].slots[index as usize] {
            Slot::Empty => Ok(()),
            Slot::Full(_) | Slot::Dead => Err(Error::NoSlot),
        }
    }

    /// Order of checks, step 2, and Transfer of capabilities: the receive
    /// range, resolved when the call is made. Whether its slots are empty
    /// is checked at delivery.
    fn window(&self, thread: ObjId, mut range: Range) -> Result<Window, Error> {
        // Transfer of capabilities: an empty range has no address.
        if range.count == 0 {
            range.first = SlotAddr::NONE;
        }
        let (first, count) = (range.first.index(), range.count);
        let cspace = self.addressed_space(thread, range.first, Rights::WRITE)?;
        // Transfer of capabilities: slot 0 or beyond the table is `invalid`.
        let end = u64::from(first) + u64::from(count);
        let beyond = first == 0 || end > self.cspaces[&cspace].slots.len() as u64;
        // Order of checks, step 3: a receive range has at most 4 slots.
        if count != 0 && beyond || count as usize > MAX_CAPS {
            return Err(Error::Invalid);
        }
        Ok(Window {
            cspace,
            first,
            count,
        })
    }

    /// Ends the wait of a thread that `unlink` took out of what it waited
    /// on: `outcome` is the outcome of the call it waited in. The wait
    /// number goes with the wait. A caller that a server receives is taken
    /// out of the queue and goes on waiting, for the reply, with the same
    /// number: that is not the end of its wait.
    fn wake(&mut self, thread: ObjId, outcome: Outcome) {
        let woken = live(&mut self.threads, thread);
        (woken.outcome, woken.number) = (Some(outcome), 0);
    }
}
