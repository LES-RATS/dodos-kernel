//! The invariants of scheduling and time.

use super::{Check, ensure};
use crate::{Model, ObjId, Queue};
use dodos_abi::{ADMISSION_WHOLE_CORE, SCHED_MAX_PERIOD_NS, SCHED_MIN_BUDGET_NS, bandwidth};
use std::collections::BTreeSet;

impl Model {
    /// Scheduling and time:
    /// 41. a context is in the queue it belongs at, by its key: the ready
    ///     queue by deadline if it is eligible with budget, the slack queue
    ///     of its class if it is eligible and throttled, the sleeping queue
    ///     if it is throttled and not eligible, none otherwise; eligible
    ///     meaning configured, not dying, not moving to another core, and a
    ///     runnable holder (the home if
    ///     runnable, else the receiver of the top lend); the orders of entry
    ///     were given out once each;
    /// 42. the admitted sum of each core of the node is the sum of the
    ///     bandwidths of the configured contexts placed on it that are not
    ///     dying, and of the recovery reserve on the node's first core, at
    ///     most one whole core; a context is placed on a core of the node;
    /// 43. a configured context has 10 µs ≤ budget ≤ period ≤ 2^36 ns (and
    ///     so do the parameters it waits to use), class 0 to 2, budget left
    ///     if and only if it is not throttled, and a replenishment instant
    ///     after now unless it is dying or its core is silent; one that is
    ///     not configured has nothing left, no deadline and is not
    ///     throttled;
    /// 44. a home and its context name each other; a lend names a live
    ///     reply object, a live context whose chain holds it once, and a
    ///     live receiver without an own context, which receives no other
    ///     lend; a chain holds only its context's lends;
    /// 45. a thread marked as having lost its context has none, and does
    ///     not run (it raised its fault at the end of the event);
    /// 46. the running context of each core is its choice: the first of its
    ///     ready queue, else of its slack queues by class, unless the core
    ///     has the reschedule interrupt pending; the thread it runs, while
    ///     not marked *leaving its core*, has an address space that is not
    ///     dying; no thread runs on two cores;
    /// 47. an armed timer is listed once by its live notify and fires after
    ///     now; a notify lists only the armed timers that name it.
    pub(super) fn check_sched(&self) -> Check {
        // A lend to a thread that does not exist is named by 44 (or an
        // earlier rule), never a panic of the check.
        let runs = |thread: &ObjId| self.threads.get(thread).is_some_and(Self::runnable_thread);
        let mut entered = BTreeSet::new();
        let mut sums = vec![0u64; self.width()];
        for (&id, sched) in &self.scheds {
            let home = sched
                .home
                .filter(|home| self.threads.contains_key(home) && runs(home));
            let top = sched.chain.last().and_then(|reply| self.lends.get(reply));
            let holder = home.or(top.map(|&(_, receiver)| receiver));
            let dying = self.marked(id);
            let eligible =
                sched.configured && !sched.moving && !dying && holder.is_some_and(|h| runs(&h));
            let wanted = match (sched.configured && !dying, eligible, sched.throttled) {
                (false, ..) => None,
                (true, true, None) => Some((Queue::Ready, sched.deadline)),
                (true, true, Some((at, _))) => Some((Queue::Slack(sched.class), at)),
                (true, false, Some((at, _))) => Some((Queue::Sleeping, at)),
                (true, false, None) => None,
            };
            let place = sched.queue.map(|q| (q.queue, q.key));
            ensure(place == wanted, "41: queue of a context")?;
            if let Some(queued) = sched.queue {
                let fresh = queued.entered < self.entered && entered.insert(queued.entered);
                ensure(fresh, "41: order of entry")?;
            }
            ensure(sched.core < self.width(), "42: core of a context")?;
            let valid = |(budget, period): (u64, u64)| {
                SCHED_MIN_BUDGET_NS <= budget && budget <= period && period <= SCHED_MAX_PERIOD_NS
            };
            let state = match sched.configured {
                true => {
                    // A context placed on a core that stopped answering
                    // keeps its state: that core takes no timer interrupt
                    // (until its node accuses itself). So does a dying
                    // context: the first step of its destruction takes it
                    // out of every queue of the scheduler (Destroying a
                    // context), so its replenishment never runs (found by
                    // seed 329 of pass 23's long run).
                    let silent = self.cores.get(sched.core).is_some_and(|c| c.silent);
                    // A replenishment on a sleeping core is taken at its
                    // wake, up to the state's exit latency late (Idle).
                    let late = self.cores.get(sched.core).map_or(0, |c| c.asleep);
                    let due =
                        |at: u64| at > self.now || late > 0 && at.saturating_add(late) >= self.now;
                    let throttled =
                        silent || dying || sched.throttled.is_some_and(|(at, _)| due(at));
                    valid((sched.budget, sched.period))
                        && sched.pending.is_none_or(valid)
                        && sched.class <= 2
                        && (sched.remaining > 0) != sched.throttled.is_some()
                        && (sched.throttled.is_none() || throttled)
                }
                false => sched.remaining == 0 && sched.deadline == 0 && sched.throttled.is_none(),
            };
            ensure(state, "43: parameters and state of a context")?;
            // Summed only once 43 holds: `bandwidth` asserts its arguments
            // (audit lot 7: the check panicked on the state it reports).
            if sched.configured && !dying {
                let (budget, period) = sched.pending.unwrap_or((sched.budget, sched.period));
                sums[sched.core] += bandwidth(budget, period);
            }
            let homed = sched
                .home
                .is_none_or(|home| self.threads.get(&home).is_some_and(|t| t.home == Some(id)));
            ensure(homed, "44: home of a context")?;
            let chained = sched.chain.iter().all(|reply| {
                self.lends
                    .get(reply)
                    .is_some_and(|&(context, _)| context == id)
            });
            let once = sched.chain.iter().collect::<BTreeSet<_>>().len() == sched.chain.len();
            ensure(chained && once, "44: chain of a context")?;
        }
        // The recovery reserve is admitted in the sum of the first core
        // from start-up.
        sums[0] += dodos_abi::RECOVERY_BANDWIDTH;
        for (core, &sum) in sums.iter().enumerate() {
            ensure(
                sum == self.cores[core].admitted && sum <= ADMISSION_WHOLE_CORE,
                "42: admitted sum",
            )?;
        }
        let mut receivers = BTreeSet::new();
        for (reply, &(context, receiver)) in &self.lends {
            let live = self.replies.contains_key(reply)
                && self
                    .scheds
                    .get(&context)
                    .is_some_and(|s| s.chain.contains(reply))
                && self
                    .threads
                    .get(&receiver)
                    .is_some_and(|t| t.home.is_none());
            ensure(live && receivers.insert(receiver), "44: lend")?;
        }
        for (&id, thread) in &self.threads {
            let homed = thread
                .home
                .is_none_or(|home| self.scheds.get(&home).is_some_and(|s| s.home == Some(id)));
            ensure(homed, "44: home of a thread")?;
            let none = thread.home.is_none() && !receivers.contains(&id);
            ensure(
                !thread.lost || none && !runs(&id),
                "45: thread that lost its context",
            )?;
        }
        self.check_choice()?;
        self.check_timers()
    }

    /// 46 (continued). The thread a core runs is the holder of its running
    /// context, and the root it loaded is that thread's address space
    /// unless it is dying (G7), each unless a reschedule is pending.
    fn check_choice(&self) -> Check {
        let mut running = BTreeSet::new();
        for (position, core) in self.cores.iter().enumerate() {
            // A silent core takes no event; during a start or a resume no
            // thread runs before the node is ready.
            if core.silent || !self.ready {
                continue;
            }
            let first = |queue: Queue| {
                let queued = self.scheds.iter().filter_map(|(&id, sched)| {
                    let q = sched
                        .queue
                        .filter(|q| q.queue == queue && sched.core == position)?;
                    Some(((q.key, q.entered), id))
                });
                queued.min().map(|(_, id)| id)
            };
            let chosen = match first(Queue::Ready) {
                Some(id) => Some((id, true)),
                None => (0..3)
                    .find_map(|class| first(Queue::Slack(class)))
                    .map(|id| (id, false)),
            };
            let pending = core.ipis.contains(&crate::Ipi::Reschedule);
            ensure(core.running == chosen || pending, "46: running context")?;
            // The thread a core runs holds the context it charges: its home
            // if the home can run, else the receiver of the top lend
            // ("What runs next"; ver2-AD D1). Read here, not through
            // `Model::holder`.
            let holder = core.running.and_then(|(context, _)| {
                let sched = self.scheds.get(&context)?;
                let home = sched
                    .home
                    .filter(|home| self.threads.get(home).is_some_and(Self::runnable_thread));
                home.or_else(|| Some(self.lends.get(sched.chain.last()?)?.1))
            });
            ensure(
                pending || core.thread == holder,
                "46: thread of the running context",
            )?;
            // The root a core loaded is that of the thread it runs, not
            // dying (G7, `load_root`; ver2-AD D2).
            let space = core
                .thread
                .and_then(|t| self.threads.get(&t)?.aspace)
                .filter(|&a| !self.marked(a));
            ensure(pending || core.loaded == space, "G7: root a core loaded")?;
            if let Some(thread) = core.thread {
                ensure(running.insert(thread), "46: a thread on two cores")?;
                let marked = self.threads.get(&thread).is_none_or(|t| t.leaving);
                let aspace = self.threads.get(&thread).and_then(|t| t.aspace);
                ensure(
                    pending || marked || aspace.is_some_and(|a| !self.marked(a)),
                    "46: address space of the running thread",
                )?;
            }
        }
        Ok(())
    }

    fn check_timers(&self) -> Check {
        for (&id, armed) in &self.timers {
            let Some(armed) = armed else {
                continue;
            };
            let listed = self
                .notifies
                .get(&armed.notify)
                .map(|notify| notify.timers.iter().filter(|&&timer| timer == id).count());
            // A timer of a sleeping core is taken at its wake, up to the
            // state's exit latency after its instant (Idle).
            let late = self.cores.get(armed.core).map_or(0, |c| c.asleep);
            let ahead = armed.instant > self.now
                || late > 0 && armed.instant.saturating_add(late) >= self.now;
            ensure(listed == Some(1) && ahead, "47: armed timer")?;
        }
        for (&id, notify) in &self.notifies {
            let armed = notify.timers.iter().all(|timer| {
                self.timers
                    .get(timer)
                    .is_some_and(|armed| armed.is_some_and(|armed| armed.notify == id))
            });
            ensure(armed, "47: timers of a notify")?;
        }
        Ok(())
    }
}

impl Model {
    /// The settled problems of pass 11 (8.1):
    /// 60. the contexts that entered a queue in the last event entered in
    ///     the order of the change list, and only those of the list;
    /// 61. the parameters in force of a context changed in the last event
    ///     only by a replenishment, a reset of the wake-up rule, or a
    ///     configuration from or to the state not configured;
    /// 62. a step of a removal put at most 96 contexts in a new queue
    ///     position, a step of `task_revoke` at most 32, any other call at
    ///     most 4;
    /// 63. a thread has a recorded fault only while it is suspended;
    /// 64. a fault *no context* reached only receivers with an own context,
    ///     and no thread raised two faults in one event.
    pub(super) fn check_events(&self) -> Check {
        let event = &self.event;
        let mut entered: Vec<(u64, ObjId)> = self
            .scheds
            .iter()
            .filter_map(|(&id, sched)| sched.queue.map(|q| (q.entered, id)))
            .filter(|&(at, _)| at >= event.entered_from)
            .collect();
        entered.sort();
        let positions: Option<Vec<usize>> = entered
            .iter()
            .map(|(_, id)| event.changes.iter().position(|c| c == id))
            .collect();
        let ordered = positions.is_some_and(|p| p.is_sorted());
        ensure(ordered, "60: order of entry and the change list")?;
        for (id, sched) in &self.scheds {
            let before = event.params.get(id).copied().unwrap_or((0, 0));
            let changed = before != (sched.budget, sched.period);
            let allowed = !changed || event.resets.contains(id) || !event.params.contains_key(id);
            ensure(allowed, "61: parameters in force changed")?;
        }
        if let Some((kind, count)) = *self.last_event {
            let bound = match kind {
                crate::sched::EventKind::Removal => 96,
                crate::sched::EventKind::TaskRevoke => 32,
                crate::sched::EventKind::Call => 4,
            };
            ensure(count <= bound, "62: contexts put in a new queue position")?;
        }
        for thread in self.threads.values() {
            ensure(
                thread.recorded.is_none() || thread.suspended,
                "63: recorded fault of a thread that is not suspended",
            )?;
        }
        let own = event.no_context_delivered.iter().all(|&own| own);
        ensure(own, "64: fault no context delivered to a passive receiver")?;
        let mut raisers: Vec<ObjId> = event.raised.iter().map(|&(t, _)| t).collect();
        raisers.sort();
        let once = raisers.windows(2).all(|pair| pair[0] != pair[1]);
        ensure(once, "64: a second fault in one event")
    }
}
