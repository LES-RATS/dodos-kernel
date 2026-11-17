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

/// Another core of this node inside a kernel event, across this node's
/// events, as a test scripts it (`Model::hold`, D-137): its entry word,
/// the locks it has taken so far, in the order it took them (its trace,
/// checked by the same rules as every event's), and the call its event
/// makes when it ends, if any. A try of a lock it holds fails; a wait for
/// a lock it holds, or an escalation that reads its entry word below the
/// new epoch, lets its event end, with its effect. No other core holds a
/// lock it holds: two cores never hold one lock.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Holder {
    /// Its core, and its entry word: the kernel epoch it read when it
    /// entered the event (D-135).
    pub(crate) core: usize,
    pub(crate) entered: u64,
    /// Its steps so far: its locks, waited for or tried, in its order.
    pub(crate) steps: Vec<Step>,
    pub(crate) then: Option<(ObjId, crate::Syscall)>,
}

impl Holder {
    /// The locks it holds.
    pub(crate) fn held(&self) -> Vec<LockId> {
        self.steps
            .iter()
            .filter_map(|s| match s {
                Step::Lock(l) | Step::Try(l) => Some(*l),
                _ => None,
            })
            .collect()
    }
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
    /// The node's quiet word taken (`true`) or released (`false`) by a call
    /// that escalates after two `again` of the rule of tries (D-135).
    Quiet(bool),
    /// The kernel epoch raised to this value, by an atomic add: at an
    /// escalation, or at the destruction of the last object of an untyped
    /// (D-135, D-137).
    Epoch(u64),
    /// The watermark of this untyped became ready to return, at the epoch
    /// raised just before (word 6 of its root slot).
    Ready(ObjId),
    /// An escalation read the entry word of another core of the node, and
    /// waited for that core's event to end if the word was not 0 and below
    /// the new epoch (D-135).
    Entry {
        core: usize,
        waited: bool,
    },
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
    /// The threads the event acts on by their own capability, which it
    /// locks by their own word even while they wait (D-135, A node of
    /// several cores: `thread_cancel`, `thread_suspend`, `task_revoke`).
    pub(crate) own: RefCell<Vec<ObjId>>,
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

/// The bound *k* of a call under L2, the most locks it holds at once in
/// its second round, the gate lock counted (A node of several cores, "The
/// bound k", counted again by D-137 from pieces each tied to a rule):
/// placing a capability takes 3 segments, moving one 7 (4 segments, and
/// for a full record 2 neighbours in its space's list and the space or the
/// bound thread), removing one with its record 8; four capabilities of a
/// message moved take 26. The marks of a delivery take no lock (D-137).
pub(crate) fn bound(call: &crate::Syscall) -> usize {
    use crate::Syscall as S;
    match call {
        // 32 units of at most 8, 64 woken threads of 2, the segments of
        // 128 examined slots, the gate.
        S::CapDelete(_) | S::CapRevoke(_) => 32 * 8 + 64 * 2 + 2 + 1,
        // The task, 32 detached threads of 6, the gate (the first unit,
        // 1 + 63 + 63 + 1 = 128, is smaller).
        S::TaskRevoke(_) => 1 + 32 * 6 + 1,
        // The endpoint, the receiver's reply object, the lent context and
        // the top of its chain, the 26, two cores and the gate.
        S::Call { .. } => 33,
        // The endpoint or the reply object, the 26, a core and the gate.
        S::Send { .. } => 29,
        // The endpoint and the reply object, two lends released of 4, the
        // bound notify, the 26, a lend and its chain's top, the core.
        S::Recv { .. } => 40,
        // The answer (29) and the receive (38), taken before either writes.
        S::ReplyRecv { .. } => 67,
        // The thread, three lists of bound threads of 6, two message pages.
        S::ThreadConfigure { .. } => 21,
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
        self.locks.own.borrow_mut().clear();
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
                let before = self.before.take();
                if let Some(before) = &before {
                    found.extend(before.changed(self));
                }
                // The fields of a thread that waits, and the reply object
                // attached to a thread that waits to receive, are guarded
                // by the lock of the object waited on, not by their own
                // (D-135; finding 6 of the tenth model review).
                let mut guarded: Vec<LockId> = Vec::new();
                for lock in found {
                    let lock = self.guard_of(lock, before.as_deref());
                    if !guarded.contains(&lock) {
                        guarded.push(lock);
                    }
                }
                let mut found = guarded;
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

    /// The lock that guards what `lock` names: the object a thread waits on
    /// (before the event or after it) for a waiting thread's fields, unless
    /// the event acts on that thread by its own capability; the endpoint a
    /// thread waits to receive on, for the reply object attached to it.
    fn guard_of(&self, lock: LockId, before: Option<&Snapshot>) -> LockId {
        let waited = |wait: &crate::Wait| match *wait {
            crate::Wait::Call { endpoint, .. } | crate::Wait::Recv { endpoint, .. } => {
                Some(LockId::Endpoint(endpoint))
            }
            crate::Wait::Reply(reply) => Some(LockId::Reply(reply)),
            crate::Wait::Notify(notify) => Some(LockId::Notify(notify)),
            _ => None,
        };
        let waits_on = |thread: ObjId| {
            let old = before.and_then(|b| b.threads.get(&thread)).map(|t| &t.wait);
            let new = self.threads.get(&thread).map(|t| &t.wait);
            old.and_then(waited).or_else(|| new.and_then(waited))
        };
        match lock {
            LockId::Thread(t) if !self.locks.own.borrow().contains(&t) => {
                waits_on(t).unwrap_or(lock)
            }
            LockId::Reply(r) => {
                let attached = |state: Option<&crate::ReplyState>| match state {
                    Some(crate::ReplyState::Attached(t)) => Some(*t),
                    _ => None,
                };
                let old = attached(before.and_then(|b| b.replies.get(&r)));
                let thread = old.or_else(|| attached(self.replies.get(&r)));
                match thread.and_then(waits_on) {
                    Some(endpoint @ LockId::Endpoint(_)) => endpoint,
                    _ => lock,
                }
            }
            _ => lock,
        }
    }

    /// The event acts on `thread` by its own capability: it holds the
    /// thread's own lock even while the thread waits.
    pub(crate) fn touch_own(&self, thread: ObjId) {
        self.locks.own.borrow_mut().push(thread);
        self.touch(LockId::Thread(thread));
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

    /// Another core of the node enters a kernel event and takes, in the
    /// order of L2, the locks of `prefix`, then those of its call (as the
    /// call takes them with no lock held elsewhere), which it makes when
    /// its event ends (D-137: a scripted holder is a second core). Refused
    /// if the core is not one of the node's, is already in an event, or is
    /// not the core of the call's thread; if a lock is held by another
    /// core; or if the holder's trace breaks the order.
    #[cfg(test)]
    pub(crate) fn hold(
        &mut self,
        core: usize,
        prefix: &[LockId],
        then: Option<(ObjId, crate::Syscall)>,
    ) -> Result<(), &'static str> {
        if core >= self.width() || self.contended.contains_key(&core) {
            return Err("a core is in one kernel event at a time");
        }
        let mut found: Vec<LockId> = prefix.to_vec();
        if let Some((thread, call)) = &then {
            if self.core_of(*thread) != core {
                return Err("the holder's call runs on the holder's core");
            }
            found.extend(self.dry_found(&Contended::Call(*thread, call.clone())));
        }
        let mut steps = Vec::new();
        let mut held: Vec<LockId> = Vec::new();
        for lock in found {
            if held.contains(&lock) {
                continue;
            }
            if self.contended.values().any(|h| h.held().contains(&lock)) {
                return Err("two cores never hold one lock");
            }
            steps.push(Step::Pay(Cost::Lock));
            match held.iter().all(|&h| h < lock) {
                true => steps.push(Step::Lock(lock)),
                false => steps.push(Step::Try(lock)),
            }
            held.push(lock);
        }
        order(&steps, None, None)?;
        let entered = self.kernel_epoch;
        self.contended.insert(
            core,
            Holder {
                core,
                entered,
                steps,
                then,
            },
        );
        Ok(())
    }

    /// A try of a lock fails while another core holds it.
    fn try_fails(&self, lock: LockId) -> bool {
        self.contended.values().any(|h| h.held().contains(&lock))
    }

    /// The event of the core that holds `lock` ends, with its effect: a wait
    /// for the lock is a wait for that event. Returns whether it changed
    /// the node.
    fn let_go(&mut self, lock: LockId) -> bool {
        let core = self
            .contended
            .values()
            .find(|h| h.held().contains(&lock))
            .map(|h| h.core);
        core.is_some_and(|core| self.end_holder(core))
    }

    /// The event another core is in ends, with its effect, which takes no
    /// lock but those it holds (they were found by the same run). Returns
    /// whether it changed the node.
    fn end_holder(&mut self, core: usize) -> bool {
        let Some(holder) = self.contended.remove(&core) else {
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
                // The holder's event ends before this core holds the lock.
                let changed = self.let_go(lock);
                steps.push(Step::Lock(lock));
                held.push(lock);
                if changed {
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
            // Under L1 a core in an event holds no lock of an object: the
            // event runs, and the other core stays in its event.
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
            // It takes the quiet word, raises the epoch by an atomic add,
            // and reads the entry word of every other core of the node,
            // waiting for the event of each core whose word is not 0 and
            // below the new epoch (D-135, D-137).
            steps.push(Step::Quiet(true));
            self.kernel_epoch += 1;
            steps.push(Step::Epoch(self.kernel_epoch));
            let at = self.core_of(thread);
            for core in (0..self.width()).filter(|&c| c != at) {
                let entry = self.contended.get(&core).map_or(0, |h| h.entered);
                let waited = entry != 0 && entry < self.kernel_epoch;
                steps.push(Step::Entry { core, waited });
                if waited {
                    self.end_holder(core);
                }
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

/// The order of L2 on one core's steps (A node of several cores, D-134,
/// D-135, D-137): a wait is for a lock after every lock held; a lock that
/// comes before one held is tried; a failed try releases every lock before
/// anything else is taken; at most two rounds; at most `bound` locks held
/// at once; the quiet word is taken with no lock held and no try under
/// it, and, given the node's width and the event's core, it is followed by
/// the raise of the epoch and the read of the entry word of every other
/// core of the node before the first lock, and the effect of the call
/// under it comes after a lock (findings 2 and 13 of the tenth model
/// review). Judged in the order the steps were taken, never sorted.
pub(crate) fn order(
    steps: &[Step],
    bound: Option<usize>,
    escalation: Option<(usize, usize)>,
) -> Result<(), &'static str> {
    let ensure = |ok: bool, what: &'static str| if ok { Ok(()) } else { Err(what) };
    let mut held: Vec<LockId> = Vec::new();
    let (mut failed, mut releasing, mut quiet, mut most) = (0, false, false, 0);
    let mut locked_under_quiet = false;
    for (index, step) in steps.iter().enumerate() {
        let takes = matches!(
            step,
            Step::Lock(_) | Step::Try(_) | Step::TryFailed(_) | Step::Quiet(true)
        );
        if takes {
            ensure(
                !releasing || held.is_empty(),
                "L2: a lock taken after a failed try before every lock held was released",
            )?;
            releasing = false;
        }
        match *step {
            Step::Lock(lock) => {
                ensure(
                    held.iter().all(|&h| h < lock),
                    "L2: a lock waited for out of the order fixed by kind",
                )?;
                held.push(lock);
                locked_under_quiet |= quiet;
            }
            Step::Try(lock) | Step::TryFailed(lock) => {
                ensure(
                    held.iter().any(|&h| lock < h) && !held.contains(&lock),
                    "L2: a lock tried that is not before a lock held",
                )?;
                ensure(!quiet, "L2: a try under the quiet word")?;
                match *step {
                    Step::Try(_) => held.push(lock),
                    _ => (failed, releasing) = (failed + 1, true),
                }
            }
            Step::Unlock(lock) => held.retain(|&h| h != lock),
            Step::Quiet(true) => {
                ensure(
                    held.is_empty(),
                    "L2: the quiet word taken while a lock is held",
                )?;
                quiet = true;
                locked_under_quiet = false;
                if let Some((width, core)) = escalation {
                    let raised = matches!(steps.get(index + 1), Some(Step::Epoch(_)));
                    ensure(
                        raised,
                        "L2: an escalation that does not raise the kernel epoch",
                    )?;
                    let read: Vec<usize> = steps[index + 2..]
                        .iter()
                        .take_while(|s| matches!(s, Step::Entry { .. }))
                        .filter_map(|s| match s {
                            Step::Entry { core, .. } => Some(*core),
                            _ => None,
                        })
                        .collect();
                    let others: Vec<usize> = (0..width).filter(|&c| c != core).collect();
                    ensure(
                        read == others,
                        "L2: an escalation that does not read the entry word of every other core",
                    )?;
                }
            }
            Step::Quiet(false) => quiet = false,
            Step::Wait(_)
            | Step::Pay(Cost::Interrupt | Cost::Broadcast | Cost::IdleWord)
            | Step::Pay(Cost::Request | Cost::LoadedSet) => ensure(
                !quiet || locked_under_quiet,
                "L2: the effect of an escalated call before its locks",
            )?,
            _ => {}
        }
        most = most.max(held.len());
    }
    ensure(failed <= 2, "L2: more than two rounds of locks in one call")?;
    ensure(!quiet, "L2: the quiet word held past the end of an event")?;
    if let Some(bound) = bound {
        ensure(most <= bound, "L2: more than k locks in one call")?;
    }
    Ok(())
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
