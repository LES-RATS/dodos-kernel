//! Locking inside a node of several cores (A node of several cores): the
//! two variants over one layout of objects, the locks each event takes, the
//! waits it enters, and what a node of one core must never pay for.
//!
//! The model runs every event of a node alone, as variant L1 does; what it
//! records is what an implementation of the variant would take:
//!
//! - **L1**: the node lock from the entry of the event to its exit, the
//!   gate lock inside it for each gate call; the two lock-free paths take
//!   no lock (a `signal` with no waiter to wake).
//! - **L2**: a lock per object the event changes, by the kind's lock word
//!   (endpoint, notify, reply, timer, thread, `sched`, task, aspace,
//!   iospace), the sequence word of a `cspace` segment for a slot written
//!   (and for the state of an `untyped`, in its root's slot), the lock word
//!   of a core for its queues and its interrupt table, and the gate lock
//!   last. The event's locks are collected where it touches objects
//!   (`live`), taken in the order of `LockId` before the event changes
//!   anything, and released at its exit.
//!
//! CONTRACT-GAP 5: the contract fixes neither the order by kind nor the
//! bound *k* of each call. The order is that of `LockId`'s variants; the
//! bound of a call is `bound`.
//!
//! The checks (`Model::check_locks`): a node of one core takes no lock and
//! pays no cost of several cores; under L1 every lock is the node lock or
//! the gate lock inside it; under L2 the locks are taken in order, the gate
//! lock last, at most *k*, and every object the event changed was locked
//! (compared with the state before the event); under both, no event spins
//! for a unit or another node.

use crate::ObjId;
use std::cell::RefCell;

/// The two variants of the lock discipline (open point of the contract).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locking {
    /// One node lock held for one kernel event, the gate lock inside it.
    #[default]
    L1,
    /// A lock per object, taken in one order by kind, the gate lock last.
    L2,
}

/// A lock of a node, in the order of variant L2: a call takes its locks in
/// this order, the gate lock last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockId {
    /// L1: the node lock.
    Node,
    /// The sequence word of a segment of a `cspace`: its space and the
    /// index of its first slot / 128.
    Segment(ObjId, u32),
    Task(ObjId),
    Thread(ObjId),
    Endpoint(ObjId),
    Reply(ObjId),
    Notify(ObjId),
    Timer(ObjId),
    Sched(ObjId),
    ASpace(ObjId),
    IoSpace(ObjId),
    /// The lock word of a core's part: its queues and its interrupt table.
    Core(usize),
    /// The gate lock, the last of every order.
    Gate,
}

/// The kinds of wait a thread enters at the end of an event, and the spin
/// an event must never make inside a hold (a wait for a unit or for another
/// node in the middle of an event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    /// Waiting for its node's cores.
    Cores,
    /// Waiting for a unit.
    Unit,
    /// Waiting for another node.
    Node,
}

/// What only a node of several cores pays for: none of it may appear in
/// an event of a node of one core (the principle of D-117).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// An interrupt to another core of the node.
    Interrupt,
    /// A broadcast invalidation (ARM64) in place of a local one.
    Broadcast,
    /// A compare-and-swap on an idle word.
    IdleWord,
    /// An invalidation request written.
    Request,
    /// A write of the set of cores that loaded a tag.
    LoadedSet,
    /// A lock instruction.
    Lock,
}

/// One step of an event, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Lock(LockId),
    Unlock(LockId),
    /// A thread entered a wait at the end of the event.
    Wait(WaitKind),
    /// The event waited inside its hold (never, in a correct kernel).
    Spin(WaitKind),
    Pay(Cost),
}

/// The steps of the current or last event, and the objects it touched.
#[derive(Clone, Debug, Default)]
pub(crate) struct Trace {
    pub(crate) steps: Vec<Step>,
    pub(crate) touched: RefCell<Vec<LockId>>,
    /// The event is a lock-free path of L1 (a `signal` that wakes no one).
    pub(crate) lock_free: bool,
    /// The bound *k* of the event's call, if it is a call.
    pub(crate) bound: Option<usize>,
}

impl Trace {
    /// Records that the event touched (changed or decided on) `id`.
    pub(crate) fn touch(&self, id: LockId) {
        let mut touched = self.touched.borrow_mut();
        if !touched.contains(&id) {
            touched.push(id);
        }
    }
}

/// A kind of state that has a lock word under L2, for `live`.
pub(crate) trait Locked {
    fn lock(id: ObjId) -> Option<LockId>;
}

macro_rules! locked {
    ($t:ty, $v:ident) => {
        impl Locked for $t {
            fn lock(id: ObjId) -> Option<LockId> {
                Some(LockId::$v(id))
            }
        }
    };
}

locked!(crate::Thread, Thread);
locked!(crate::Endpoint, Endpoint);
locked!(crate::ReplyState, Reply);
locked!(crate::Notify, Notify);
locked!(Option<crate::Armed>, Timer);
locked!(crate::Sched, Sched);
locked!(crate::Task, Task);
locked!(crate::ASpace, ASpace);

/// The bound *k* of a call under L2: the locks its rules can make it take,
/// the gate lock counted (CONTRACT-GAP 5). A step of a removal undoes at
/// most 32 links and wakes at most 64 threads, each with its object, its
/// context and its core; a step of `task_revoke` stops 32 threads.
pub(crate) fn bound(call: &crate::Syscall) -> usize {
    use crate::Syscall as S;
    match call {
        S::CapDelete(_) | S::CapRevoke(_) => 4 * (32 + 64) + 8,
        S::TaskRevoke(_) => 4 * 32 + 8,
        S::Call { .. } | S::ReplyRecv { .. } | S::Send { .. } | S::NodeSend { .. } => 16,
        _ => 12,
    }
}

impl crate::Model {
    /// Records a cost of several cores.
    pub(crate) fn pay(&mut self, cost: Cost) {
        self.locks.steps.push(Step::Pay(cost));
    }

    /// Records a wait the event makes a thread enter.
    pub(crate) fn wait_mark(&mut self, kind: WaitKind) {
        self.locks.steps.push(Step::Wait(kind));
    }

    /// Records that the event touched a lock.
    pub(crate) fn touch(&self, id: LockId) {
        self.locks.touch(id);
    }

    /// The lock of the segment that holds slot `index` of a space.
    pub(crate) fn touch_slot(&self, space: ObjId, index: u32) {
        self.touch(LockId::Segment(
            space,
            index / dodos_abi::CSPACE_SEGMENT_SLOTS,
        ));
    }

    /// The state of an untyped is guarded by the lock of the segment that
    /// holds its root capability's slot (D-131).
    pub(crate) fn touch_untyped(&self, untyped: ObjId) {
        let Some(object) = self.objects.get(&untyped) else {
            return;
        };
        let place = self.caps[&object.root].place;
        self.touch_slot(place.space, place.index);
    }

    /// An entry of a core's interrupt table, and a core's queues, are
    /// guarded by that core's lock word.
    pub(crate) fn touch_core(&self, core: usize) {
        self.touch(LockId::Core(core));
    }

    /// The start of an event: its trace is empty.
    pub(crate) fn begin_locks(&mut self) {
        self.locks.steps.clear();
        self.locks.touched.borrow_mut().clear();
        self.locks.lock_free = false;
        self.locks.bound = None;
        self.stalled = None;
        if self.locking_watched() {
            *self.before = Some(Box::new(crate::locks::Snapshot::of(self)));
        }
    }

    /// Whether the event's objects are compared with the state before it:
    /// variant L2 on a node of several cores.
    fn locking_watched(&self) -> bool {
        self.width() > 1 && self.machine.locking == Locking::L2
    }

    /// The end of an event: the locks it took are recorded in the order of
    /// its variant (a node of one core takes none: no lock instruction).
    pub(crate) fn end_locks(&mut self) {
        if self.width() == 1 {
            self.locks.touched.borrow_mut().clear();
            return;
        }
        let mut steps = Vec::new();
        let gate = !self.table.writes.is_empty()
            || !self.table.member_writes.is_empty()
            || !self.table.role_writes.is_empty();
        match self.machine.locking {
            Locking::L1 => {
                let woke = self.locks.steps.iter().any(|s| matches!(s, Step::Wait(_)));
                let lock_free = self.locks.lock_free && !woke && self.signal_woke_nobody();
                if !lock_free {
                    steps.push(Step::Pay(Cost::Lock));
                    steps.push(Step::Lock(LockId::Node));
                    if gate {
                        steps.push(Step::Lock(LockId::Gate));
                        steps.push(Step::Unlock(LockId::Gate));
                    }
                }
                steps.append(&mut self.locks.steps);
                if !lock_free {
                    steps.push(Step::Unlock(LockId::Node));
                }
            }
            Locking::L2 => {
                let mut touched: Vec<LockId> = self.locks.touched.borrow().clone();
                if let Some(before) = self.before.take() {
                    touched.extend(before.changed(self));
                }
                if gate {
                    touched.push(LockId::Gate);
                }
                touched.sort();
                touched.dedup();
                for &lock in &touched {
                    steps.push(Step::Pay(Cost::Lock));
                    steps.push(Step::Lock(lock));
                }
                steps.append(&mut self.locks.steps);
                for &lock in touched.iter().rev() {
                    steps.push(Step::Unlock(lock));
                }
            }
        }
        self.locks.steps = steps;
    }

    /// The lock-free signal of L1 woke no one: only the notify's word
    /// changed (one atomic OR).
    fn signal_woke_nobody(&self) -> bool {
        !self
            .locks
            .touched
            .borrow()
            .iter()
            .any(|l| matches!(l, LockId::Thread(_) | LockId::Core(_)))
    }
}

/// The state of the objects that have a lock under variant L2, before an
/// event, to find those the event changed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    threads: std::collections::BTreeMap<ObjId, crate::Thread>,
    endpoints: std::collections::BTreeMap<ObjId, crate::Endpoint>,
    replies: std::collections::BTreeMap<ObjId, crate::ReplyState>,
    notifies: std::collections::BTreeMap<ObjId, crate::Notify>,
    timers: std::collections::BTreeMap<ObjId, Option<crate::Armed>>,
    scheds: std::collections::BTreeMap<ObjId, crate::Sched>,
    tasks: std::collections::BTreeMap<ObjId, crate::Task>,
    aspaces: std::collections::BTreeMap<ObjId, (usize, usize, usize)>,
    slots: std::collections::BTreeMap<ObjId, Vec<crate::Slot>>,
    untypeds: std::collections::BTreeMap<ObjId, (u64, u64)>,
    irqs: std::collections::BTreeMap<ObjId, crate::Irq>,
}

impl Snapshot {
    pub(crate) fn of(model: &crate::Model) -> Self {
        Self {
            threads: model.threads.clone(),
            endpoints: model.endpoints.clone(),
            replies: model.replies.clone(),
            notifies: model.notifies.clone(),
            timers: model.timers.clone(),
            scheds: model.scheds.clone(),
            tasks: model.tasks.clone(),
            aspaces: model
                .aspaces
                .iter()
                .map(|(&id, a)| (id, (a.translations.len(), a.records.len(), a.threads.len())))
                .collect(),
            slots: model
                .cspaces
                .iter()
                .map(|(&id, c)| (id, c.slots.clone()))
                .collect(),
            untypeds: model
                .untypeds
                .iter()
                .map(|(&id, u)| (id, (u.watermark, u.clean)))
                .collect(),
            irqs: model.irqs.clone(),
        }
    }

    /// The locks of the objects the event changed: each object whose state
    /// differs, the segment of each slot written, the segment of an
    /// untyped's root slot, the core of an irq or of a context whose queue
    /// changed.
    pub(crate) fn changed(&self, model: &crate::Model) -> Vec<LockId> {
        fn diff<T: PartialEq>(
            before: &std::collections::BTreeMap<ObjId, T>,
            after: &std::collections::BTreeMap<ObjId, T>,
        ) -> Vec<ObjId> {
            let keys = before.keys().chain(after.keys());
            let mut ids: Vec<ObjId> = keys
                .filter(|k| before.get(k) != after.get(k))
                .copied()
                .collect();
            ids.dedup();
            ids
        }
        let mut locks = Vec::new();
        locks.extend(
            diff(&self.threads, &model.threads)
                .into_iter()
                .map(LockId::Thread),
        );
        locks.extend(
            diff(&self.endpoints, &model.endpoints)
                .into_iter()
                .map(LockId::Endpoint),
        );
        locks.extend(
            diff(&self.replies, &model.replies)
                .into_iter()
                .map(LockId::Reply),
        );
        locks.extend(
            diff(&self.notifies, &model.notifies)
                .into_iter()
                .map(LockId::Notify),
        );
        locks.extend(
            diff(&self.timers, &model.timers)
                .into_iter()
                .map(LockId::Timer),
        );
        locks.extend(
            diff(&self.tasks, &model.tasks)
                .into_iter()
                .map(LockId::Task),
        );
        for id in diff(&self.scheds, &model.scheds) {
            locks.push(LockId::Sched(id));
            let before = self.scheds.get(&id);
            let after = model.scheds.get(&id);
            let queue = before.map(|s| (s.queue, s.core)) != after.map(|s| (s.queue, s.core));
            if queue {
                locks.extend(
                    before
                        .into_iter()
                        .chain(after)
                        .map(|s| LockId::Core(s.core)),
                );
            }
        }
        let shapes: std::collections::BTreeMap<ObjId, (usize, usize, usize)> = model
            .aspaces
            .iter()
            .map(|(&id, a)| (id, (a.translations.len(), a.records.len(), a.threads.len())))
            .collect();
        for id in diff(&self.aspaces, &shapes) {
            let io = model.iospaces.contains_key(&id);
            locks.push(if io {
                LockId::IoSpace(id)
            } else {
                LockId::ASpace(id)
            });
        }
        for (space, slots) in &model.cspaces {
            let before = self.slots.get(space);
            for (index, slot) in slots.slots.iter().enumerate() {
                if before.and_then(|b| b.get(index)) != Some(slot) {
                    let segment = index as u32 / dodos_abi::CSPACE_SEGMENT_SLOTS;
                    locks.push(LockId::Segment(*space, segment));
                }
            }
        }
        let untypeds: std::collections::BTreeMap<ObjId, (u64, u64)> = model
            .untypeds
            .iter()
            .map(|(&id, u)| (id, (u.watermark, u.clean)))
            .collect();
        for id in diff(&self.untypeds, &untypeds) {
            if let Some(object) = model.objects.get(&id)
                && let Some(cap) = model.caps.get(&object.root)
            {
                let segment = cap.place.index / dodos_abi::CSPACE_SEGMENT_SLOTS;
                locks.push(LockId::Segment(cap.place.space, segment));
            }
        }
        for id in diff(&self.irqs, &model.irqs) {
            let core = model.irqs.get(&id).or(self.irqs.get(&id)).map(|i| i.core);
            locks.extend(core.map(LockId::Core));
        }
        locks
    }
}
