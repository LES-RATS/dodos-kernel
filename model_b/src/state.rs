//! State of the modelled kernel, one core.
//!
//! Everything the model knows lives in [`State`]: kernel objects, the
//! capability table of every cspace, the derivation tree, and the threads
//! with their wait queues. The state is a plain value: `Clone`, `Debug`,
//! `PartialEq`, so a differential test can diff two states.
//!
//! Ideas taken from seL4 (cited, not copied): the derivation tree and
//! revocation walk, untyped memory as the only source of object memory, the
//! flat cspace, badges on endpoint capabilities, and (from seL4 MCS) the
//! one-shot reply object created by `call`.

use dodos_abi::{Error, Marks, ObjectKind, PageSize, Received, Rights};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Identifier of a kernel object.
pub type ObjId = u64;
/// Identifier of a thread. A thread is also an object: the `ThreadId` is the
/// object id of its `Object::Thread`.
pub type ThreadId = u64;
/// Identifier of a derivation-tree node. Nodes are identified by `CapId`,
/// not by slot, because a capability can move between slots and cspaces
/// without leaving the tree.
pub type CapId = u64;
/// Index of a slot in a cspace.
pub type SlotIdx = u32;

/// Bytes of one capability slot (contract, "Object sizes").
pub const SLOT_BYTES: u64 = dodos_abi::SLOT_BYTES as u64;
/// Size of a thread object in bytes (contract: 1 KB).
pub const THREAD_BYTES: u64 = 1024;
/// Size of an endpoint, notify or reply object in bytes (contract: 64 bytes).
pub const SMALL_BYTES: u64 = 64;
/// Nodes deleted per `cap_revoke` call. The contract says revocation runs in
/// bounded steps and returns `again` but does not fix the bound.
// CONTRACT-GAP: the step bound of `cap_revoke` is unspecified ("Object sizes"
// and rule 4 say only "bounded steps"). Chosen: 8 nodes per call.
pub const REVOKE_STEP: usize = 8;

/// Slots of the boot cspace created by [`State::new`].
pub const BOOT_CSPACE_SLOTS: u32 = 128;
/// Bytes of ordinary untyped memory given to the boot process.
pub const BOOT_MEMORY_BYTES: u64 = 64 * 1024 * 1024;
/// Bytes of device untyped memory given to the boot process.
pub const BOOT_DEVICE_BYTES: u64 = 1024 * 1024;
/// Physical address where the ordinary untyped range starts.
pub const BOOT_MEMORY_BASE: u64 = 0x1000_0000;
/// Physical address where the device untyped range starts.
pub const BOOT_DEVICE_BASE: u64 = 0xF000_0000;

/// The boot thread (the fourth object created by [`State::new`]).
pub const BOOT_THREAD: ThreadId = 3;

/// Boot slots: 0 = own cspace, 1 = untyped, 2 = device untyped, 3 = own thread.
pub const BOOT_SLOT_CSPACE: u32 = 0;
pub const BOOT_SLOT_UNTYPED: u32 = 1;
pub const BOOT_SLOT_DEVICE: u32 = 2;
pub const BOOT_SLOT_THREAD: u32 = 3;

/// One slot of a cspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotState {
    Empty,
    /// Tombstone: the slot held a capability that was revoked. A call on a
    /// dead slot returns `gone` (contract rule 2), not `invalid`.
    Dead,
    Full(CapId),
}

/// A node of the derivation tree. The tree links a capability to the
/// capabilities derived from it, across cspaces; revocation walks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cap {
    pub id: CapId,
    pub obj: ObjId,
    pub rights: Rights,
    pub badge: u64,
    pub parent: Option<CapId>,
    pub children: Vec<CapId>,
    /// Set on a whole subtree at the start of `cap_revoke`: every call on a
    /// revoking capability returns `gone` immediately (contract rule 2).
    pub revoking: bool,
    /// Where the node currently sits: (cspace object, slot).
    pub loc: (ObjId, SlotIdx),
}

/// How a capability listed in a message is transferred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferMode {
    /// The capability leaves the sender's slot; the node keeps its place in
    /// the derivation tree.
    Move,
    /// The sender keeps its capability; the receiver gets a derived one, a
    /// child of the sender's node, so the sender can still revoke it.
    Derive(Rights),
}

/// A capability listed in a message, resolved at send time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTransfer {
    /// Slot of the sender's cspace that held the capability at send time.
    pub from: (ObjId, SlotIdx),
    pub cap: CapId,
    pub mode: TransferMode,
}

/// A message waiting in an endpoint queue, or handed to a receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub sender: ThreadId,
    /// Badge of the endpoint capability the sender used.
    pub badge: u64,
    pub header: dodos_abi::Header,
    pub words: Vec<u64>,
    pub transfers: Vec<ResolvedTransfer>,
    /// Set for messages sent with `call`: the reply object created for them.
    pub reply: Option<ObjId>,
}

/// An endpoint: a synchronous rendezvous point (contract, "Objects").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// FIFO of messages that found no waiting receiver.
    // CONTRACT-GAP: the contract calls the endpoint a "synchronous
    // rendezvous point" but does not fix a queue depth for messages that
    // arrive with no receiver. Chosen: an unbounded FIFO (a model has no
    // kernel heap to bound it); `send` never blocks either way.
    pub queue: VecDeque<Incoming>,
    /// Threads blocked in `recv` / `reply_recv` on this endpoint, FIFO.
    pub receivers: VecDeque<ThreadId>,
    /// Notify signalled when a message is queued with no receiver waiting.
    // CONTRACT-GAP: the contract names "an endpoint with a bound notify"
    // but no call binds one. Chosen: the binding is given as an auxiliary
    // slot at `untyped_retype` time when the endpoint is created.
    pub bound_notify: Option<ObjId>,
}

/// A reply object: a one-shot right to answer one call (seL4 MCS idea).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The thread blocked in `call` that this reply answers.
    pub caller: ThreadId,
    /// Set when the call's capability transfer was dropped for lack of
    /// slots; the caller is told through the flag in the reply's header.
    pub caps_dropped: bool,
}

/// A notification: a word of signal bits, asynchronous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notify {
    pub word: u64,
    /// Threads blocked in `wait` on this notify.
    pub waiters: Vec<ThreadId>,
}

/// Kernel objects. Only the kinds the one-core model needs; the other kinds
/// of the contract are out of scope and `untyped_retype` refuses them with
/// `unsupported`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Object {
    /// A range of physical memory, not yet typed. The only source of memory
    /// for other objects. `size` is what remains; retyping carves from the
    /// front.
    Untyped {
        base: u64,
        size: u64,
        init_base: u64,
        init_size: u64,
        device: bool,
    },
    CSpace {
        slots: Vec<SlotState>,
    },
    /// A run of physical pages of one size. The contract says a frame has no
    /// object of its own and the capability slot holds the mapping record;
    /// mappings (aspace) are out of scope, so only the page size is kept.
    Frame {
        page_size: PageSize,
    },
    /// Marker for a thread; the runtime state lives in `State::threads`.
    Thread,
    Endpoint(Endpoint),
    Reply(Reply),
    Notify(Notify),
}

impl Object {
    pub fn kind(&self) -> ObjectKind {
        match self {
            Object::Untyped { .. } => ObjectKind::Untyped,
            Object::CSpace { .. } => ObjectKind::CSpace,
            Object::Frame { .. } => ObjectKind::Frame,
            Object::Thread => ObjectKind::Thread,
            Object::Endpoint(_) => ObjectKind::Endpoint,
            Object::Reply(_) => ObjectKind::Reply,
            Object::Notify(_) => ObjectKind::Notify,
        }
    }
}

/// What a thread is doing. The model has no scheduler: the test picks any
/// runnable thread and applies its next call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    /// Never entered: the contract says `send` never blocks.
    // CONTRACT-GAP: the contract says "send never blocks", so no call ever
    // leaves a thread blocked on send; the variant exists so the state
    // machine is explicit and the invariant checker can reject it.
    BlockedSend {
        endpoint: ObjId,
    },
    /// Blocked in `recv` / `reply_recv` (or `wait` on an endpoint) on an
    /// endpoint. `range` is the receiver's declared slot range for incoming
    /// capabilities; `reply_recv` asks for the reply object of a call.
    BlockedRecv {
        endpoint: ObjId,
        range: (u32, u32),
        reply_recv: bool,
    },
    /// Blocked in `wait` on a notify (possibly the notify bound to an
    /// endpoint the thread waited on).
    BlockedWait {
        notify: ObjId,
    },
    /// Blocked in `call`: waiting for the reply. `range` is the declared
    /// slot range for capabilities carried by the reply.
    BlockedReply {
        endpoint: ObjId,
        reply: ObjId,
        range: (u32, u32),
    },
}

impl ThreadState {
    /// Does this blocked state reference the object (used when destroying it)?
    pub fn references(&self, obj: ObjId) -> bool {
        match self {
            ThreadState::Runnable => false,
            ThreadState::BlockedSend { endpoint } => *endpoint == obj,
            ThreadState::BlockedRecv { endpoint, .. } => *endpoint == obj,
            ThreadState::BlockedWait { notify } => *notify == obj,
            ThreadState::BlockedReply {
                endpoint, reply, ..
            } => *endpoint == obj || *reply == obj,
        }
    }
}

/// Completion of a call that blocked a thread earlier. It is returned by the
/// next `apply` on that thread, together with the result of the new call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbox {
    /// A message was delivered to the thread.
    Message {
        received: Received,
        words: Vec<u64>,
        /// Slot of the reply capability, when the message came from a `call`
        /// and the receiver asked for the reply (`reply_recv`).
        reply_slot: Option<u32>,
    },
    /// A `wait` completed: the accumulated signal bits.
    Notification(u64),
    /// The blocked call completed with an error, e.g. `gone` when the object
    /// it waited on was destroyed.
    Error(Error),
}

/// A thread. `task` and `marks` are the minimum the IPC stamp needs: task
/// objects are out of scope, so marks are always `NONE` and the task id is
/// assigned at thread creation.
// CONTRACT-GAP: tasks and marks are out of scope, but the contract forces
// every received message to carry the sender's marks and task id. Chosen:
// each thread has a fixed task id (assigned in creation order) and marks
// `NONE`; there is no way to add marks without task objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub obj: ObjId,
    /// The cspace the thread works in; `None` once that cspace was destroyed.
    pub cspace: Option<ObjId>,
    pub task: u64,
    pub marks: Marks,
    pub state: ThreadState,
    pub inbox: Option<Inbox>,
}

/// The whole modelled kernel, one core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub objects: BTreeMap<ObjId, Object>,
    pub caps: BTreeMap<CapId, Cap>,
    pub threads: BTreeMap<ThreadId, Thread>,
    next_obj: ObjId,
    next_cap: CapId,
    next_task: u64,
}

impl State {
    // CONTRACT-GAP: "Start-up information" says the first process gets
    // everything in fixed slots, but fixes neither the slot numbers nor the
    // amount of memory. Chosen: the four boot slots and sizes documented
    // above, one ordinary and one device untyped range.
    /// The boot state, fixed and deterministic: one cspace of
    /// [`BOOT_CSPACE_SLOTS`] slots, one ordinary untyped range and one device
    /// untyped range, and one runnable boot thread holding a full-rights
    /// capability to each in the boot slots (contract, "Start-up
    /// information": the first process gets everything).
    pub fn new() -> Self {
        let mut st = State {
            objects: BTreeMap::new(),
            caps: BTreeMap::new(),
            threads: BTreeMap::new(),
            next_obj: 0,
            next_cap: 0,
            next_task: 0,
        };
        let cspace = st.fresh_obj();
        st.objects.insert(
            cspace,
            Object::CSpace {
                slots: vec![SlotState::Empty; BOOT_CSPACE_SLOTS as usize],
            },
        );
        let mem = st.fresh_obj();
        st.objects.insert(
            mem,
            Object::Untyped {
                base: BOOT_MEMORY_BASE,
                size: BOOT_MEMORY_BYTES,
                init_base: BOOT_MEMORY_BASE,
                init_size: BOOT_MEMORY_BYTES,
                device: false,
            },
        );
        let dev = st.fresh_obj();
        st.objects.insert(
            dev,
            Object::Untyped {
                base: BOOT_DEVICE_BASE,
                size: BOOT_DEVICE_BYTES,
                init_base: BOOT_DEVICE_BASE,
                init_size: BOOT_DEVICE_BYTES,
                device: true,
            },
        );
        let thr = st.fresh_obj();
        st.objects.insert(thr, Object::Thread);
        let task = st.next_task;
        st.next_task += 1;
        st.threads.insert(
            thr,
            Thread {
                obj: thr,
                cspace: Some(cspace),
                task,
                marks: Marks::NONE,
                state: ThreadState::Runnable,
                inbox: None,
            },
        );
        st.put_cap(cspace, BOOT_SLOT_CSPACE, None, cspace, Rights::ALL, 0);
        st.put_cap(cspace, BOOT_SLOT_UNTYPED, None, mem, Rights::ALL, 0);
        st.put_cap(cspace, BOOT_SLOT_DEVICE, None, dev, Rights::ALL, 0);
        st.put_cap(cspace, BOOT_SLOT_THREAD, None, thr, Rights::ALL, 0);
        st
    }

    /// The threads the test may pick next: exactly the runnable ones.
    pub fn runnable(&self) -> Vec<ThreadId> {
        self.threads
            .iter()
            .filter(|(_, t)| matches!(t.state, ThreadState::Runnable))
            .map(|(id, _)| *id)
            .collect()
    }

    /// The cspace a thread works in, if it is alive.
    pub fn thread_cspace(&self, caller: ThreadId) -> Option<ObjId> {
        self.threads.get(&caller)?.cspace
    }

    /// Number of slots of a cspace.
    pub fn cspace_len(&self, cspace: ObjId) -> Option<usize> {
        match self.objects.get(&cspace) {
            Some(Object::CSpace { slots }) => Some(slots.len()),
            _ => None,
        }
    }

    pub(crate) fn fresh_obj(&mut self) -> ObjId {
        let id = self.next_obj;
        self.next_obj += 1;
        id
    }

    pub(crate) fn take_task(&mut self) -> u64 {
        let t = self.next_task;
        self.next_task += 1;
        t
    }

    pub(crate) fn fresh_cap(&mut self) -> CapId {
        let id = self.next_cap;
        self.next_cap += 1;
        id
    }

    /// Insert a new capability node into a slot and link it into the tree.
    pub(crate) fn put_cap(
        &mut self,
        cspace: ObjId,
        slot: SlotIdx,
        parent: Option<CapId>,
        obj: ObjId,
        rights: Rights,
        badge: u64,
    ) -> CapId {
        let id = self.fresh_cap();
        if let Some(p) = parent
            && let Some(pc) = self.caps.get_mut(&p)
        {
            pc.children.push(id);
        }
        self.caps.insert(
            id,
            Cap {
                id,
                obj,
                rights,
                badge,
                parent,
                children: Vec::new(),
                revoking: false,
                loc: (cspace, slot),
            },
        );
        if let Some(Object::CSpace { slots }) = self.objects.get_mut(&cspace) {
            slots[slot as usize] = SlotState::Full(id);
        }
        id
    }

    /// The cspace a slot number belongs to: the caller's own, or the one
    /// named by `args.dst_cspace_slot` (a path relative to a capability the
    /// caller holds, as in seL4's destination cspace argument).
    pub(crate) fn dst_cspace(
        &self,
        caller: ThreadId,
        args: &crate::calls::Args,
    ) -> Result<ObjId, Error> {
        match args.dst_cspace_slot {
            None => self.caller_cspace(caller),
            Some(s) => {
                let cap = self.resolve(caller, s)?;
                if !matches!(self.objects.get(&cap.obj), Some(Object::CSpace { .. })) {
                    return Err(Error::Invalid);
                }
                Ok(cap.obj)
            }
        }
    }

    pub(crate) fn caller_cspace(&self, caller: ThreadId) -> Result<ObjId, Error> {
        let t = self.threads.get(&caller).ok_or(Error::Gone)?;
        let cs = t.cspace.ok_or(Error::Gone)?;
        if !matches!(self.objects.get(&cs), Some(Object::CSpace { .. })) {
            return Err(Error::Gone);
        }
        Ok(cs)
    }

    /// Resolve a slot of the caller's cspace to its capability, without the
    /// revocation check (used by `cap_revoke`, which must be able to
    /// continue an ongoing revocation).
    pub(crate) fn resolve_raw(&self, caller: ThreadId, slot: u32) -> Result<Cap, Error> {
        let cs = self.caller_cspace(caller)?;
        let s = self
            .objects
            .get(&cs)
            .and_then(|o| match o {
                Object::CSpace { slots } => slots.get(slot as usize),
                _ => None,
            })
            .cloned()
            .ok_or(Error::Invalid)?;
        match s {
            SlotState::Empty => Err(Error::Invalid),
            SlotState::Dead => Err(Error::Gone),
            SlotState::Full(id) => Ok(self.caps.get(&id).cloned().ok_or(Error::Gone)?),
        }
    }

    /// Resolve a slot of the caller's cspace to its capability. A capability
    /// being revoked is already `gone` (contract rule 2).
    pub(crate) fn resolve(&self, caller: ThreadId, slot: u32) -> Result<Cap, Error> {
        let cap = self.resolve_raw(caller, slot)?;
        if cap.revoking {
            return Err(Error::Gone);
        }
        Ok(cap)
    }

    /// How many capabilities anywhere point at this object.
    pub(crate) fn cap_count(&self, obj: ObjId) -> usize {
        self.caps.values().filter(|c| c.obj == obj).count()
    }

    /// First empty slot of a cspace.
    pub(crate) fn lowest_free_slot(&self, cspace: ObjId) -> Option<u32> {
        match self.objects.get(&cspace) {
            Some(Object::CSpace { slots }) => slots
                .iter()
                .position(|s| matches!(s, SlotState::Empty))
                .map(|i| i as u32),
            _ => None,
        }
    }

    /// Move a capability node to another slot (possibly of another cspace);
    /// the tree is untouched.
    pub(crate) fn move_cap_to(&mut self, id: CapId, cspace: ObjId, slot: SlotIdx) {
        if let Some(cap) = self.caps.get(&id) {
            let (old_cs, old_slot) = cap.loc;
            if let Some(Object::CSpace { slots }) = self.objects.get_mut(&old_cs)
                && let Some(s) = slots.get_mut(old_slot as usize)
                && *s == SlotState::Full(id)
            {
                *s = SlotState::Empty;
            }
        }
        if let Some(Object::CSpace { slots }) = self.objects.get_mut(&cspace) {
            slots[slot as usize] = SlotState::Full(id);
        }
        if let Some(cap) = self.caps.get_mut(&id) {
            cap.loc = (cspace, slot);
        }
    }

    /// Remove one node of the derivation tree: the slot becomes a tombstone
    /// (`Dead` when the capability was revoked or consumed, `Empty` when it
    /// was deleted by its holder), children are re-attached to the node's
    /// parent (so they stay revocable through the ancestors), and the object
    /// is destroyed when its last capability goes.
    // CONTRACT-GAP: the contract does not say what happens to the derivation
    // tree when an intermediate capability is deleted (not revoked). Chosen:
    // children are re-parented to the deleted node's parent, so revoking an
    // ancestor still removes everything derived from it.
    pub(crate) fn remove_cap_node(&mut self, id: CapId, dead: bool) {
        let Some(cap) = self.caps.remove(&id) else {
            return;
        };
        if let Some(p) = cap.parent
            && let Some(pc) = self.caps.get_mut(&p)
        {
            pc.children.retain(|c| *c != id);
        }
        for ch in &cap.children {
            if let Some(c) = self.caps.get_mut(ch) {
                c.parent = cap.parent;
            }
            if let Some(p) = cap.parent
                && let Some(pc) = self.caps.get_mut(&p)
                && !pc.children.contains(ch)
            {
                pc.children.push(*ch);
            }
        }
        let (cs, slot) = cap.loc;
        if let Some(Object::CSpace { slots }) = self.objects.get_mut(&cs)
            && let Some(s) = slots.get_mut(slot as usize)
            && *s == SlotState::Full(id)
        {
            *s = if dead {
                SlotState::Dead
            } else {
                SlotState::Empty
            };
        }
        if self.cap_count(cap.obj) == 0 {
            self.destroy_object(cap.obj);
        }
    }

    /// Destroy an object: remove it, wake every thread blocked on it with
    /// `gone`, unbind threads whose cspace it was, and clean the queues.
    pub(crate) fn destroy_object(&mut self, obj: ObjId) {
        let Some(object) = self.objects.remove(&obj) else {
            return;
        };
        if matches!(object, Object::Thread) {
            self.threads.remove(&obj);
        }
        if matches!(object, Object::CSpace { .. }) {
            for t in self.threads.values_mut() {
                if t.cspace == Some(obj) {
                    t.cspace = None;
                }
            }
            // The slots are gone with the cspace: so are the capabilities
            // they held (their objects may be destroyed in turn).
            let stored: Vec<CapId> = self
                .caps
                .values()
                .filter(|c| c.loc.0 == obj)
                .map(|c| c.id)
                .collect();
            for id in stored {
                self.remove_cap_node(id, true);
            }
        }
        let mut woken = Vec::new();
        for (tid, t) in &self.threads {
            if t.state.references(obj) {
                woken.push(*tid);
            }
        }
        for tid in woken {
            if let Some(t) = self.threads.get_mut(&tid) {
                t.state = ThreadState::Runnable;
                t.inbox = Some(Inbox::Error(Error::Gone));
            }
        }
        let live_objs: BTreeSet<ObjId> = self.objects.keys().copied().collect();
        let live_threads: BTreeSet<ThreadId> = self.threads.keys().copied().collect();
        for (oid, o) in self.objects.iter_mut() {
            match o {
                Object::Endpoint(ep) => {
                    if ep.bound_notify == Some(obj) {
                        ep.bound_notify = None;
                    }
                    ep.receivers.retain(|tid| {
                        live_threads.contains(tid)
                            && matches!(&self.threads[tid].state, ThreadState::BlockedRecv { endpoint, .. } if *endpoint == *oid)
                    });
                    ep.queue.retain(|inc| {
                        live_threads.contains(&inc.sender)
                            && inc.reply.is_none_or(|r| live_objs.contains(&r))
                    });
                }
                Object::Notify(n) => {
                    n.waiters.retain(|tid| {
                        live_threads.contains(tid)
                            && matches!(&self.threads[tid].state, ThreadState::BlockedWait { notify } if *notify == *oid)
                    });
                }
                _ => {}
            }
        }
    }

    /// The subtree rooted at a node, as (depth, node) pairs. Deterministic:
    /// children are visited in insertion order.
    pub(crate) fn subtree(&self, root: CapId) -> Vec<(usize, CapId)> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, root)];
        while let Some((d, id)) = stack.pop() {
            if let Some(cap) = self.caps.get(&id) {
                out.push((d, id));
                for ch in cap.children.iter().rev() {
                    stack.push((d + 1, *ch));
                }
            }
        }
        out
    }
}

/// The boot state is the natural default.
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
