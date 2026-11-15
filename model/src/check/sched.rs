//! The invariants of scheduling and time.

use super::{Check, ensure};
use crate::{Model, ObjId, Queue, Wait};
use dodos_abi::{ADMISSION_WHOLE_CORE, SCHED_MAX_PERIOD_NS, SCHED_MIN_BUDGET_NS, bandwidth};
use std::collections::BTreeSet;

impl Model {
    /// Scheduling and time:
    /// 41. a context is in the queue it belongs at, by its key: the ready
    ///     queue by deadline if it is eligible with budget, the slack queue
    ///     of its class if it is eligible and throttled, the sleeping queue
    ///     if it is throttled and not eligible, none otherwise; eligible
    ///     meaning configured, not dying, and a runnable holder (the home if
    ///     runnable, else the receiver of the top lend); the orders of entry
    ///     were given out once each;
    /// 42. the admitted sum is the sum of the bandwidths of the configured
    ///     contexts that are not dying, at most one whole core;
    /// 43. a configured context has 10 µs ≤ budget ≤ period ≤ 2^36 ns (and
    ///     so do the parameters it waits to use), class 0 to 2, budget left
    ///     if and only if it is not throttled, and a replenishment instant
    ///     after now; one that is not configured has nothing left, no
    ///     deadline and is not throttled;
    /// 44. a home and its context name each other; a lend names a live
    ///     reply object, a live context whose chain holds it once, and a
    ///     live receiver without an own context, which receives no other
    ///     lend; a chain holds only its context's lends;
    /// 45. a thread marked as having lost its context has none, and does
    ///     not run (it raised its fault at the end of the event);
    /// 46. the running context is the choice: the first of the ready queue,
    ///     else of the slack queues by class; its holder has an address
    ///     space that is not dying;
    /// 47. an armed timer is listed once by its live notify and fires after
    ///     now; a notify lists only the armed timers that name it.
    pub(super) fn check_sched(&self) -> Check {
        let runs = |thread: &ObjId| {
            let thread = &self.threads[thread];
            !thread.suspended && thread.wait == Wait::None
        };
        let mut entered = BTreeSet::new();
        let mut sum = 0u64;
        for (&id, sched) in &self.scheds {
            let home = sched
                .home
                .filter(|home| self.threads.contains_key(home) && runs(home));
            let top = sched.chain.last().and_then(|reply| self.lends.get(reply));
            let holder = home.or(top.map(|&(_, receiver)| receiver));
            let dying = self.marked(id);
            let eligible = sched.configured && !dying && holder.is_some_and(|h| runs(&h));
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
            if sched.configured && !dying {
                let (budget, period) = sched.pending.unwrap_or((sched.budget, sched.period));
                sum += bandwidth(budget, period);
            }
            let valid = |(budget, period): (u64, u64)| {
                SCHED_MIN_BUDGET_NS <= budget && budget <= period && period <= SCHED_MAX_PERIOD_NS
            };
            let state = match sched.configured {
                true => {
                    let throttled = sched.throttled.is_some_and(|(at, _)| at > self.now);
                    valid((sched.budget, sched.period))
                        && sched.pending.is_none_or(valid)
                        && sched.class <= 2
                        && (sched.remaining > 0) != sched.throttled.is_some()
                        && (sched.throttled.is_none() || throttled)
                }
                false => sched.remaining == 0 && sched.deadline == 0 && sched.throttled.is_none(),
            };
            ensure(state, "43: parameters and state of a context")?;
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
        ensure(
            sum == self.admitted && sum <= ADMISSION_WHOLE_CORE,
            "42: admitted sum",
        )?;
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

    fn check_choice(&self) -> Check {
        let first = |queue: Queue| {
            let queued = self.scheds.iter().filter_map(|(&id, sched)| {
                let q = sched.queue.filter(|q| q.queue == queue)?;
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
        ensure(self.running == chosen, "46: running context")?;
        let Some((context, _)) = chosen else {
            return Ok(());
        };
        let sched = &self.scheds[&context];
        let home = sched.home.filter(|home| {
            let thread = &self.threads[home];
            !thread.suspended && thread.wait == Wait::None
        });
        let holder = home.or(sched.chain.last().map(|reply| self.lends[reply].1));
        let aspace = holder.and_then(|holder| self.threads[&holder].aspace);
        ensure(
            aspace.is_some_and(|a| !self.marked(a)),
            "46: address space of the running thread",
        )
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
            ensure(
                listed == Some(1) && armed.instant > self.now,
                "47: armed timer",
            )?;
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
