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
//!   last. The event's locks are recorded in the order it finds them,
//!   where it touches objects (`live`), not sorted: a lock that comes after
//!   every lock the event holds is waited for; a lock that comes before one
//!   it holds is one it found by reading an object it holds, which the
//!   contract has it *try* ("The order of L2 and its bound", D-134).
//!   Every event still runs alone, but a test can script a lock that
//!   another core holds across events (`Model::contended`, a `Holder`):
//!   a try of it fails, and the event then makes the contract's rounds
//!   (`contend`): it releases everything, waits for the locks it knows in
//!   the order, reads again, and a call returns `again` with nothing
//!   changed if a link changed or a try of its second round failed; a
//!   `signal` and an arrival wait in the order and never return `again`;
//!   a third such `again` in a row escalates to the quiet word (D-135).
//!   The model locks a woken thread and its reply object by their own
//!   words (tried, earlier in the order), where the contract guards them
//!   by the lock of the object waited on: an over-approximation.
//!
//! The order by kind is the contract's: that of `LockId`'s variants,
//! objects of one kind by identity as by address. The bound *k* of a call
//! is `bound`, the most locks it holds at once.
//!
//! The checks (`Model::check_locks`): a node of one core takes no lock and
//! pays no cost of several cores; under L1 every lock is the node lock or
//! the gate lock inside it; under L2 a wait is only for a lock that comes
//! after every lock held, a try only for one that comes before a lock
//! held, the gate lock last, at most *k* held at once, every object the
//! event changed was locked (compared with the state before the event), a
//! failed try followed by the release of every lock before any other is
//! taken, at most two rounds, and the quiet word taken with no lock held
//! and no try under it; under both, no event spins for a unit or another
//! node.

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

/// A lock of this node that another core holds across this node's events,
/// as a test scripts it: the tries of other cores it makes fail before its
/// event ends, and what that event does when it ends (a call of a thread
/// of the node, on the holder's core), if anything. A wait for the lock
/// lets the holder's event end: a lock is held for one bounded event.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Holder {
    pub(crate) tries: u32,
    pub(crate) then: Option<(ObjId, crate::Syscall)>,
    /// The holder's core, and its entry word: the kernel epoch it read
    /// when it entered the event in which it holds the lock (D-135).
    pub(crate) core: usize,
    pub(crate) entered: u64,
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
    /// A lock waited for: in the order by kind, after every lock held.
    Lock(LockId),
    /// A lock found by reading an object the event holds, earlier in the
    /// order than a lock held: tried, never waited for (D-134).
    Try(LockId),
    /// A try that failed: another core held the lock. The call releases
    /// every lock it holds, having changed nothing, before it takes any
    /// other (D-134).
    TryFailed(LockId),
    Unlock(LockId),
    /// The node's quiet word taken (`true`) or cleared (`false`) by a call
    /// that escalates after two `again` of the rule of tries (D-135).
    Quiet(bool),
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
/// the gate lock counted (gap 5). A step of a removal undoes at
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
    pub(crate) fn locking_watched(&self) -> bool {
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
            || !self.table.role_writes.is_empty()
            || *self.table.gated;
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
                // In the order the event found them: those it touched, then
                // the objects it changed without touching them, then the
                // gate lock.
                let mut found: Vec<LockId> = self.locks.touched.borrow().clone();
                if let Some(before) = self.before.take() {
                    for lock in before.changed(self) {
                        if !found.contains(&lock) {
                            found.push(lock);
                        }
                    }
                }
                found.retain(|&l| l != LockId::Gate);
                if gate {
                    found.push(LockId::Gate);
                }
                let mut held: Option<LockId> = None;
                for &lock in &found {
                    steps.push(Step::Pay(Cost::Lock));
                    match held {
                        Some(top) if lock < top => steps.push(Step::Try(lock)),
                        _ => {
                            steps.push(Step::Lock(lock));
                            held = Some(lock);
                        }
                    }
                }
                steps.append(&mut self.locks.steps);
                for &lock in found.iter().rev() {
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

/// How a round of locks ended.
enum Round {
    /// Every lock taken: these, in the order taken.
    Taken(Vec<LockId>),
    /// A try failed: the locks held then, and the index of the failed lock
    /// in the order of the round.
    Failed(Vec<LockId>, usize),
    /// A wait reached a lock another core holds, whose event changes what
    /// the call reads: the round is begun again once it has ended.
    Changed(Vec<LockId>),
}

/// Locking under contention (variant L2, A node of several cores, D-134,
/// D-135). The model runs one event at a time; a lock another core holds
/// across events (`Model::contended`) makes a try fail. A call then makes
/// the rounds of the contract: the locks it finds, in the order it finds
/// them, waits in the order by kind and tries for the others; at a failed
/// try it releases everything, takes the locks it knows in the order,
/// reads again what it follows and returns `again` with nothing changed if
/// a link changed or a try of its second round failed. A `signal` never
/// returns `again`: its second round waits for every lock. After two such
/// `again` in a row, a call escalates to the quiet word: the other cores
/// leave their events and the call runs alone.
impl crate::Model {
    /// The locks an event would take, in the order it finds them: the event
    /// run on a copy of the node with no lock held elsewhere.
    fn dry_found(&self, event: &Contended) -> Vec<LockId> {
        let mut dry = self.clone();
        dry.contended.clear();
        let _ = dry.run_once(event);
        dry.locks
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Lock(l) | Step::Try(l) => Some(*l),
                _ => None,
            })
            .collect()
    }

    /// The event, as it runs with no lock held elsewhere.
    fn run_once(&mut self, event: &Contended) -> crate::Outcome {
        match event {
            Contended::Call(thread, call) => self.apply_once(*thread, call.clone()),
            Contended::Arrival(position, vector) => {
                self.interrupt_once(*position, *vector);
                Ok(crate::Output::Done)
            }
        }
    }

    /// A try of a lock that another core holds fails while that holder
    /// still makes tries fail.
    fn try_fails(&mut self, lock: LockId) -> bool {
        match self.contended.get_mut(&lock) {
            Some(holder) if holder.tries > 0 => {
                holder.tries -= 1;
                true
            }
            _ => false,
        }
    }

    /// The event of the core that holds `lock` ends, with its effect: a wait
    /// for the lock is a wait for that event. Returns whether it changed
    /// the node.
    fn let_go(&mut self, lock: LockId) -> bool {
        let Some(holder) = self.contended.remove(&lock) else {
            return false;
        };
        let Some((thread, call)) = holder.then else {
            return false;
        };
        let _ = self.apply_once(thread, call);
        true
    }

    /// One round: the locks of `found` not yet held, in its order.
    fn round(&mut self, found: &[LockId], mut held: Vec<LockId>, steps: &mut Vec<Step>) -> Round {
        for (index, &lock) in found.iter().enumerate() {
            if held.contains(&lock) {
                continue;
            }
            steps.push(Step::Pay(Cost::Lock));
            if held.iter().all(|&h| h < lock) {
                steps.push(Step::Lock(lock));
                held.push(lock);
                if self.let_go(lock) {
                    return Round::Changed(held);
                }
            } else if self.try_fails(lock) {
                steps.push(Step::TryFailed(lock));
                return Round::Failed(held, index);
            } else {
                steps.push(Step::Try(lock));
                held.push(lock);
            }
        }
        Round::Taken(held)
    }

    /// A call under contention.
    pub(crate) fn apply_contended(
        &mut self,
        thread: ObjId,
        call: crate::Syscall,
    ) -> crate::Outcome {
        self.contend(Contended::Call(thread, call))
    }

    /// An arrival under contention: an event that is not a call never
    /// returns `again`.
    pub(crate) fn interrupt_contended(&mut self, position: usize, vector: u8) {
        let _ = self.contend(Contended::Arrival(position, vector));
    }

    /// An event under contention, by the rule of tries.
    fn contend(&mut self, event: Contended) -> crate::Outcome {
        if !self.locking_watched() {
            self.contended.clear();
            return self.run_once(&event);
        }
        let mut steps = Vec::new();
        let first = self.dry_found(&event);
        let (held, failed) = match self.round(&first, Vec::new(), &mut steps) {
            Round::Taken(held) => return self.finish(&event, steps, &held),
            // The holder's event came first: begin again, nothing changed.
            Round::Changed(held) => {
                release(&held, &mut steps);
                return self.contend(event);
            }
            Round::Failed(held, index) => (held, index),
        };
        release(&held, &mut steps);
        // The second round: the locks it now knows, in the order, waiting.
        let mut known: Vec<LockId> = first[..=failed].to_vec();
        for &lock in &known {
            self.let_go(lock);
        }
        let second = self.dry_found(&event);
        if event.never_again() {
            // A `signal`, or an event that is not a call, follows the path
            // of an arrival: it waits for every lock of its second round,
            // in the order, and never returns `again` (D-135).
            known.extend(second.iter().copied());
            known.sort();
            known.dedup();
            for &lock in &known {
                steps.push(Step::Pay(Cost::Lock));
                steps.push(Step::Lock(lock));
                self.let_go(lock);
            }
            return self.finish(&event, steps, &known);
        }
        known.sort();
        known.dedup();
        for &lock in &known {
            steps.push(Step::Pay(Cost::Lock));
            steps.push(Step::Lock(lock));
        }
        // It checks that what it read still leads to the same locks.
        let changed = first[..=failed].iter().any(|l| !second.contains(l));
        let held = match changed {
            true => known,
            false => match self.round(&second, known, &mut steps) {
                Round::Taken(held) => return self.finish(&event, steps, &held),
                Round::Failed(held, _) | Round::Changed(held) => held,
            },
        };
        release(&held, &mut steps);
        self.again_of_tries(event, steps)
    }

    /// The event runs with the locks the rounds took; the steps it records
    /// are the rounds', then its own other steps, then the releases.
    fn finish(
        &mut self,
        event: &Contended,
        mut steps: Vec<Step>,
        held: &[LockId],
    ) -> crate::Outcome {
        let outcome = self.run_once(event);
        self.record_rounds(&mut steps, held);
        if let Contended::Call(thread, _) = event
            && let Some(t) = self.threads.get_mut(thread)
        {
            t.again_tries = 0;
        }
        outcome
    }

    /// Replaces the lock steps the event synthesised with the rounds it
    /// made, keeping its other steps.
    fn record_rounds(&mut self, steps: &mut Vec<Step>, held: &[LockId]) {
        let others = self.locks.steps.iter().copied().filter(|s| {
            !matches!(
                s,
                Step::Lock(_) | Step::Try(_) | Step::Unlock(_) | Step::Pay(Cost::Lock)
            )
        });
        steps.extend(others);
        release(held, steps);
        self.locks.steps = std::mem::take(steps);
    }

    /// A call that the rule of tries makes return `again`, with nothing
    /// changed; or, if its thread already got two such results in a row,
    /// the escalation: it takes the node's quiet word, the other cores
    /// leave their events (every lock they held is let go), and it runs
    /// alone, waiting for its locks in the order (D-135).
    fn again_of_tries(&mut self, event: Contended, mut steps: Vec<Step>) -> crate::Outcome {
        let Contended::Call(thread, _) = event else {
            unreachable!("only a call returns again");
        };
        let tries = self.threads.get(&thread).map_or(0, |t| t.again_tries);
        if tries >= 2 {
            steps.push(Step::Quiet(true));
            self.kernel_epoch += 1;
            let held: Vec<LockId> = self.contended.keys().copied().collect();
            for lock in held {
                self.let_go(lock);
            }
            let mut alone = self.dry_found(&event);
            alone.sort();
            alone.dedup();
            for &lock in &alone {
                steps.push(Step::Pay(Cost::Lock));
                steps.push(Step::Lock(lock));
            }
            let outcome = self.run_once(&event);
            self.record_rounds(&mut steps, &alone);
            self.locks.steps.push(Step::Quiet(false));
            if let Some(t) = self.threads.get_mut(&thread) {
                t.again_tries = 0;
            }
            return outcome;
        }
        // An event that changes nothing: the call's entry and exit.
        let core = self.core_of(thread);
        let entry = self.begin_on(core);
        self.end(entry);
        self.locks.steps = steps;
        if let Some(t) = self.threads.get_mut(&thread) {
            t.again_tries = tries + 1;
        }
        Err(dodos_abi::Error::Again)
    }
}

/// An event that may find locks held by another core.
enum Contended {
    Call(ObjId, crate::Syscall),
    /// An interrupt arrival: the core's position and the vector.
    Arrival(usize, u8),
}

impl Contended {
    /// A `signal`, and an event that is not a call, never return `again`.
    fn never_again(&self) -> bool {
        !matches!(self, Self::Call(_, call) if !matches!(call, crate::Syscall::Signal(_)))
    }
}

/// The releases of the locks held, in the reverse order.
fn release(held: &[LockId], steps: &mut Vec<Step>) {
    steps.extend(held.iter().rev().map(|&l| Step::Unlock(l)));
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
