//! Locking inside a node of several cores (A node of several cores, D-138):
//! one node lock held for one kernel event, the gate lock inside it, the
//! waits an event makes a thread enter, and what a node of one core must
//! never pay for.
//!
//! The model runs every event of a node alone, as the node lock makes the
//! events of a node run one at a time; what it records is what a kernel
//! takes: the node lock from the entry of the event (after its lock-free
//! part) to its exit, and the gate lock inside it for each gate call. A
//! `signal` that wakes no one takes no lock (the lock-free signal).
//!
//! The lock per object of the earlier text (variant L2: the order by kind,
//! the tries, the quiet word, the bound *k*) left the contract of version 1
//! (D-138); its text is `research/drafts/locking-l2.md` of the
//! documentation repository, and its model is in the history of this
//! branch (commit `04503d5`).
//!
//! The checks (`Model::check_locks`): a node of one core takes no lock and
//! pays no cost of several cores; every lock is the node lock or the gate
//! lock inside it; no event spins for a unit or another node.

/// A lock of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockId {
    /// The node lock.
    Node,
    /// The gate lock, taken inside the node lock.
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
    /// A request of a core written (an invalidation, or the barrier before
    /// a watermark returns).
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
    /// The kernel epoch raised to this value, at the destruction of the
    /// last object of an untyped (D-135, D-138).
    Epoch(u64),
    /// The watermark of this untyped became ready to return, at the epoch
    /// raised just before (word 6 of its root slot).
    Ready(crate::ObjId),
    /// A thread entered a wait at the end of the event.
    Wait(WaitKind),
    /// The event waited inside its hold (never, in a correct kernel).
    Spin(WaitKind),
    Pay(Cost),
}

/// The steps of the current or last event.
#[derive(Clone, Debug, Default)]
pub(crate) struct Trace {
    pub(crate) steps: Vec<Step>,
    /// The event is a `signal`, whose lock-free path takes no lock if it
    /// wakes no one.
    pub(crate) lock_free: bool,
    /// The event made a waiting thread runnable.
    pub(crate) woke: bool,
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

    /// The start of an event: its trace is empty.
    pub(crate) fn begin_locks(&mut self) {
        self.locks.steps.clear();
        self.locks.lock_free = false;
        self.locks.woke = false;
        self.stalled = None;
    }

    /// The end of an event: the locks it took are recorded around its
    /// steps (a node of one core takes none: no lock instruction).
    pub(crate) fn end_locks(&mut self) {
        if self.width() == 1 {
            return;
        }
        let gate = !self.table.writes.is_empty()
            || !self.table.member_writes.is_empty()
            || !self.table.role_writes.is_empty()
            || *self.table.gated;
        let waits = self.locks.steps.iter().any(|s| matches!(s, Step::Wait(_)));
        let lock_free = self.locks.lock_free && !self.locks.woke && !waits && !gate;
        let mut steps = Vec::new();
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
        self.locks.steps = steps;
    }
}

/// The bounds of D-042 under one node lock (A node of several cores,
/// D-137, D-138): the longest event of a core, its wait for the lock
/// included, and the longest healthy wait that `CORE_ANSWER_NS` judges.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Measured {
    /// *N*, the cores of the largest node the image supports.
    pub(crate) cores: u64,
    /// *H*, the longest hold of the node lock by an event.
    pub(crate) node_hold: u64,
    /// *D*, the delivery of one interrupt.
    pub(crate) delivery: u64,
}

impl Measured {
    /// *E* = *N* × *H*: the longest kernel event of a core, its wait for
    /// the node lock included.
    pub(crate) fn event_with_waits(&self) -> u64 {
        self.cores.saturating_mul(self.node_hold)
    }

    /// The longest healthy wait `CORE_ANSWER_NS` judges, a request: the
    /// asked core's current event, the delivery, then the event of the
    /// interrupt itself, 2 × E + D (D-138: the interrupt's own event was
    /// left out, finding 7 of the twelfth contract review).
    pub(crate) fn healthy_wait(&self) -> u64 {
        self.event_with_waits()
            .saturating_mul(2)
            .saturating_add(self.delivery)
    }
}
