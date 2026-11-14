//! Executable reference model of the DodOS kernel, for one core.
//!
//! The contract is `architecture/kernel-interface.md` in the documentation
//! repository. A comment that names a section names a section of its "Call
//! reference, one core". The kernel is tested against this model call by
//! call: [`Model::apply`] applies one call made by one thread, and
//! [`Model::check`] checks every invariant of the state.
//!
//! Not modelled: what address spaces, frames and scheduling contexts do once
//! created, the contents of memory, tasks and marks, registers, and the
//! scheduling policy ([`Model::runnable`] lists the threads that can run; the
//! caller picks one).
//!
//! `CONTRACT-GAP` marks a behaviour that the contract does not fix.

#![forbid(unsafe_code)]

mod caps;
mod check;
mod ipc;

use dodos_abi::{
    CSPACE_SEGMENT_SLOTS, Call, Error, MAX_CAPS, ObjectKind, PageSize, Received, Rights,
    START_SLOT_ASPACE, START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_FIRST, START_SLOT_SCHED,
    START_SLOT_THREAD, SlotAddr,
};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;

/// Bytes of the ordinary untyped, in slot `START_SLOT_FIRST`.
pub const BOOT_UNTYPED_BYTES: u64 = 64 << 20;
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
    Derive(SlotAddr, Rights),
}

/// A message as its sender gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub label: u32,
    /// The flag bits of the header. Order of checks: the kernel sets them,
    /// and a sender that sets one gets `invalid`.
    pub flags: u32,
    pub words: Vec<u64>,
    pub caps: Vec<Transfer>,
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
/// `received.header.caps()` slots of its receive range.
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
        rights: Rights,
        badge: u64,
    },
    CapMove {
        source: SlotAddr,
        destination: SlotAddr,
    },
    CapDelete(SlotAddr),
    CapRevoke(SlotAddr),
    CapInspect(SlotAddr),
    /// `size` is the size argument; for a frame it is the number of pages of
    /// size `page`, which no other kind reads.
    UntypedRetype {
        untyped: SlotAddr,
        kind: ObjectKind,
        size: u64,
        page: PageSize,
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
    },
    ThreadResume(SlotAddr),
    ThreadSuspend(SlotAddr),
    Call {
        endpoint: SlotAddr,
        message: Message,
        range: Range,
    },
    Recv {
        endpoint: SlotAddr,
        reply: SlotAddr,
        range: Range,
    },
    ReplyRecv {
        reply: SlotAddr,
        message: Message,
        endpoint: SlotAddr,
        range: Range,
    },
    Send {
        target: SlotAddr,
        message: Message,
    },
    Signal(SlotAddr),
    Wait(SlotAddr),
    Poll(SlotAddr),
    NotifyBind {
        notify: SlotAddr,
        endpoint: SlotAddr,
    },
    /// A call outside the one-core model: it returns `Unsupported`.
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

/// What a check gives when it passes.
type Checked<T> = Result<T, Error>;

/// A place for a capability: a capability space and an index in it.
type Place = (ObjId, u32);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Empty,
    Full(CapId),
    Dead,
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
    /// Objects, and segments of capability spaces, taken from it and not
    /// yet destroyed.
    live: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct CSpace {
    max: u32,
    slots: Vec<Slot>,
    /// The derivation tree: `Some` once the space is dying.
    sweep: Option<Sweep>,
}

/// The derivation tree: what a dying capability space keeps.
#[derive(Clone, Debug, PartialEq)]
struct Sweep {
    /// The index where the sweep stands: the slots before it were examined
    /// and are not examined again.
    next: usize,
    /// The capabilities it held when it began to die. Removal ends: a dying
    /// space only shrinks, so it never holds another.
    held: Vec<CapId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Thread {
    /// `None` until `thread_configure`. A capability space that no longer
    /// exists is one that was destroyed.
    cspace: Option<ObjId>,
    suspended: bool,
    wait: Wait,
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

/// The right needed to use a slot through a `cspace` capability.
#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

/// The whole state of the kernel of one core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    next_id: u32,
    caps: BTreeMap<CapId, Cap>,
    objects: BTreeMap<ObjId, Object>,
    untypeds: BTreeMap<ObjId, Untyped>,
    cspaces: BTreeMap<ObjId, CSpace>,
    threads: BTreeMap<ObjId, Thread>,
    endpoints: BTreeMap<ObjId, Endpoint>,
    replies: BTreeMap<ObjId, ReplyState>,
    notifies: BTreeMap<ObjId, Notify>,
    /// The derivation tree: the longest walk so far to the next capability
    /// to remove, in capabilities. The contract bounds it by 192.
    longest_walk: usize,
}

/// The derivation tree: the bound of the contract on the walk to the next
/// capability to remove, 128 restricted capabilities then 64 ordinary ones.
pub const LONGEST_WALK: usize = 3 * dodos_abi::TREE_DEPTH;

/// The entry `id` of one of the maps of the state, to change it. Every
/// identity the state holds names a live entry (`Model::check`).
fn live<K: Ord + Debug, T>(map: &mut BTreeMap<K, T>, id: K) -> &mut T {
    map.get_mut(&id).expect("names a live entry")
}

impl Model {
    /// The state at start-up, and the first thread, which can run (Start-up
    /// information). Its capability space has one segment and holds itself,
    /// an `aspace`, the thread, a `sched` and `debug` in their fixed slots,
    /// then one ordinary untyped and one device untyped.
    pub fn boot() -> (Self, ObjId) {
        let mut model = Self::default();
        let space = ObjId(model.next_id);
        let mut add = |kind, size, index, rights| {
            let cap = model.create(kind, size, None, rights, (space, index));
            model.caps[&cap].object
        };
        let (all, segment) = (Rights::ALL, CSPACE_SEGMENT_SLOTS);
        add(ObjectKind::CSpace, segment.into(), START_SLOT_CSPACE, all);
        add(ObjectKind::ASpace, 0, START_SLOT_ASPACE, all);
        let thread = add(ObjectKind::Thread, 0, START_SLOT_THREAD, all);
        add(ObjectKind::Sched, 0, START_SLOT_SCHED, all);
        add(ObjectKind::Debug, 0, START_SLOT_DEBUG, all);
        add(
            ObjectKind::Untyped,
            BOOT_UNTYPED_BYTES,
            START_SLOT_FIRST,
            all,
        );
        // CONTRACT-GAP (Start-up information): the contract gives the first
        // process every untyped, and says "never `exec` on device memory"
        // of new capabilities only. The start-up capability for device
        // memory has no `exec` either: invariant 25 rests on it.
        let no_exec = all.without(Rights::EXEC);
        let device = add(
            ObjectKind::Untyped,
            BOOT_DEVICE_BYTES,
            START_SLOT_FIRST + 1,
            no_exec,
        );
        live(&mut model.untypeds, device).device = true;
        let first = live(&mut model.threads, thread);
        (first.cspace, first.suspended) = (Some(space), false);
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
    /// to remove, in capabilities; at most `LONGEST_WALK`. The kernel can be
    /// measured against it.
    pub fn longest_walk(&self) -> usize {
        self.longest_walk
    }

    /// The object that a slot address names for `thread`, if any: a way to
    /// find the identity of a thread that a call created.
    pub fn object_at(&self, thread: ObjId, slot: SlotAddr) -> Option<ObjId> {
        self.threads.get(&thread)?;
        let cap = self.cap(thread, slot, Access::Read).ok()?;
        Some(self.caps[&cap].object)
    }

    /// Applies one call made by `thread`, which must be able to run.
    pub fn apply(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        assert!(self.runnable().contains(&thread), "{thread:?} cannot run");
        live(&mut self.threads, thread).outcome = None;
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
                destination,
            } => self.untyped_retype(thread, untyped, kind, size, page, destination),
            Syscall::CSpaceGrow { cspace, untyped } => self.cspace_grow(thread, cspace, untyped),
            Syscall::ThreadConfigure {
                thread: target,
                cspace,
                aspace,
                fault,
            } => self.thread_configure(thread, target, cspace, aspace, fault),
            Syscall::ThreadResume(target) => self.thread_resume(thread, target),
            Syscall::ThreadSuspend(target) => self.thread_suspend(thread, target),
            Syscall::Call {
                endpoint,
                message,
                range,
            } => self.call(thread, endpoint, message, range),
            Syscall::Recv {
                endpoint,
                reply,
                range,
            } => self.recv(thread, endpoint, reply, range),
            Syscall::ReplyRecv {
                reply,
                message,
                endpoint,
                range,
            } => self.reply_recv(thread, reply, message, endpoint, range),
            Syscall::Send { target, message } => self.send(thread, target, message),
            Syscall::Signal(notify) => self.signal(thread, notify),
            Syscall::Wait(notify) => self.wait(thread, notify, ipc::IfZero::Wait),
            Syscall::Poll(notify) => self.wait(thread, notify, ipc::IfZero::WouldWait),
            Syscall::NotifyBind { notify, endpoint } => self.notify_bind(thread, notify, endpoint),
            // CONTRACT-GAP (Threads): `thread_read_regs` and
            // `thread_write_regs` have a row in the table of the one-core
            // calls, with rights and a `busy` rule, and a sentence that
            // puts the register block outside the one-core model. Here they
            // are `unsupported`, as every call of another group.
            Syscall::Other(_) => {
                self.space(thread, SlotAddr::NONE, Access::Read)?;
                Err(Error::Unsupported)
            }
        }
    }

    // Order of checks. Each call checks its capability arguments with the
    // functions below, in the order of its table (steps 1 and 2), then its
    // plain arguments (step 3), then the state (step 4).

    /// Slots and addresses: the capability space that an address is in.
    /// Through a `cspace` capability, `access` says which right is needed.
    fn space(&self, thread: ObjId, addr: SlotAddr, access: Access) -> Result<ObjId, Error> {
        let own = self.own(thread)?;
        // Removal ends: a thread whose capability space is dying gets `gone`
        // from every call except `cap_delete`.
        if self.dying(own) {
            return Err(Error::Gone);
        }
        self.through(own, addr, access)
    }

    /// The derivation tree: once its capability space is destroyed, a
    /// thread gets `gone` from every call, `cap_delete` too.
    fn own(&self, thread: ObjId) -> Result<ObjId, Error> {
        let bound = self.threads[&thread].cspace;
        bound
            .filter(|own| self.cspaces.contains_key(own))
            .ok_or(Error::Gone)
    }

    /// Slots and addresses: `own`, or the capability space that `via` names
    /// in it.
    fn through(&self, own: ObjId, addr: SlotAddr, access: Access) -> Result<ObjId, Error> {
        if addr.via() == 0 {
            return Ok(own);
        }
        let right = match access {
            Access::Read => Rights::READ,
            Access::Write => Rights::WRITE,
        };
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
    /// that reaches its root capability.
    fn dying(&self, object: ObjId) -> bool {
        self.caps[&self.objects[&object].root].dying
    }

    /// Order of checks, steps 1 and 2: the slot of a capability argument.
    fn cap(&self, thread: ObjId, addr: SlotAddr, access: Access) -> Result<CapId, Error> {
        self.full(self.space(thread, addr, access)?, addr.index())
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
    fn held(&self, thread: ObjId, at: SlotAddr, kind: ObjectKind, need: Rights) -> Checked<&Cap> {
        self.is(self.cap(thread, at, Access::Read)?, kind, need)
    }

    /// As `held`, for a call that needs only the object.
    fn object(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Checked<ObjId> {
        Ok(self.held(thread, at, kind, need)?.object)
    }

    /// Order of checks, step 2: a slot in which the call places a
    /// capability. Slots and addresses: slot 0 or beyond the table is
    /// `invalid`.
    fn destination(&self, thread: ObjId, addr: SlotAddr) -> Result<Place, Error> {
        let space = self.space(thread, addr, Access::Write)?;
        if addr.index() == 0 || addr.index() as usize >= self.cspaces[&space].slots.len() {
            return Err(Error::Invalid);
        }
        Ok((space, addr.index()))
    }

    /// Order of checks, step 4: `no_slot` if the destination is not empty.
    fn vacant(&self, (space, index): Place) -> Result<(), Error> {
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
        let cspace = self.space(thread, range.first, Access::Write)?;
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
    /// on: `outcome` is the outcome of the call it waited in.
    fn wake(&mut self, thread: ObjId, outcome: Outcome) {
        live(&mut self.threads, thread).outcome = Some(outcome);
    }

    // Threads.

    fn thread_configure(
        &mut self,
        thread: ObjId,
        target: SlotAddr,
        cspace: SlotAddr,
        aspace: SlotAddr,
        fault: SlotAddr,
    ) -> Outcome {
        let manage = Rights::MANAGE;
        let target = self.object(thread, target, ObjectKind::Thread, manage)?;
        let space = self.object(thread, cspace, ObjectKind::CSpace, manage)?;
        // CONTRACT-GAP (Threads): the contract does not say what becomes of
        // a thread when its address space or its fault endpoint is
        // destroyed. Both are checked here and not kept: nothing in the
        // one-core model reads them.
        // Threads: both may be 0.
        if !aspace.is_none() {
            self.object(thread, aspace, ObjectKind::ASpace, manage)?;
        }
        if !fault.is_none() {
            self.object(thread, fault, ObjectKind::Endpoint, Rights::SEND)?;
        }
        let target = live(&mut self.threads, target);
        // Threads: the thread must be suspended and must not wait.
        if !target.suspended || target.wait != Wait::None {
            return Err(Error::Busy);
        }
        target.cspace = Some(space);
        Ok(Output::Done)
    }

    /// Threads: resuming a thread that is not suspended succeeds; `busy` if
    /// the thread is bound to no capability space, or to one that is
    /// destroyed.
    fn thread_resume(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        self.own(target).or(Err(Error::Busy))?;
        live(&mut self.threads, target).suspended = false;
        Ok(Output::Done)
    }

    /// Threads: suspending a suspended thread succeeds, and suspending a
    /// thread that waits leaves its wait as it is.
    fn thread_suspend(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        live(&mut self.threads, target).suspended = true;
        Ok(Output::Done)
    }
}
