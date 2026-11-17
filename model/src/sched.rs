//! Scheduling and time: the clock, scheduling contexts and their calls,
//! what runs next, the lends of the budget across `call`, and timers.

use crate::{Armed, Model, ObjId, Outcome, Output, Queue, Queued, Sched, Thread, live};
use dodos_abi::RECOVERY_BANDWIDTH;
use dodos_abi::{
    ADMISSION_WHOLE_CORE, CORE_WINDOWS, Error, FaultKind, ObjectKind, Rights, SCHED_COUNT_OFF,
    SCHED_COUNT_ON, SCHED_MAX_PERIOD_NS, SCHED_MIN_BUDGET_NS, SCHED_STATE_CONFIGURED,
    SCHED_STATE_COUNTED, SCHED_STATE_HOME, SCHED_STATE_HW_CLASS_SHIFT, SCHED_STATE_LENT,
    SCHED_STATE_THROTTLED, SlotAddr, TIME_EVENT_STEP, TIME_LIMIT, bandwidth, firing_instant,
};

/// Due events and the hardware timer: one due event, in the order in which
/// a timer interrupt processes them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Due {
    /// A replenishment, ranked by its queue: the slack queues by class,
    /// then the sleeping queue.
    Replenish(u64, ObjId),
    Fire(ObjId),
}

impl Sched {
    /// Admission: the bandwidth of a context, from the parameters last
    /// given, which count in the sum from the call that gave them; 0 when
    /// it is not configured.
    pub(crate) fn bandwidth(&self) -> u64 {
        let (budget, period) = self.pending.unwrap_or((self.budget, self.period));
        if self.configured {
            bandwidth(budget, period)
        } else {
            0
        }
    }
}

/// What a context is at the start of an event, for the change list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Snap {
    eligible: bool,
    remaining: i64,
    throttled: Option<(u64, u64)>,
    wanted: Option<(Queue, u64)>,
}

/// Bounds: what an event was, for the number of contexts it may put in a
/// new queue position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EventKind {
    /// A step of a removal: at most 96.
    Removal,
    /// A step of `task_revoke`: at most 32.
    TaskRevoke,
    /// Any other call: at most 4.
    Call,
}

/// The arguments of `sched_configure` after the capabilities.
pub(crate) struct Parameters {
    pub(crate) budget: u64,
    pub(crate) period: u64,
    pub(crate) class: u64,
}

impl Model {
    /// Clock: the instant of the clock, in nanoseconds.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// What runs next: the thread that runs on the node's first core, the
    /// holder of the context that its last choice took; `None` when that
    /// core is idle.
    pub fn running(&self) -> Option<ObjId> {
        self.running_on(0)
    }

    /// The thread that runs on the core at `position` of the node.
    pub fn running_on(&self, position: usize) -> Option<ObjId> {
        self.cores.get(position)?.thread
    }

    /// The number of cores of the node.
    pub fn core_count(&self) -> usize {
        self.cores.len()
    }

    /// Clock: time passes up to `to`. The reference model processes every
    /// due event at its instant: a timer interrupt is an event, on its
    /// core, at each instant a core's hardware timer is set to, up to `to`;
    /// at one instant, the cores in position order.
    pub fn advance(&mut self, to: u64) {
        assert!(to >= self.now && to < TIME_LIMIT, "the clock is monotonic");
        while let Some((instant, core)) = self.next_timer().filter(|&(at, _)| at <= to) {
            self.now = instant.max(self.now);
            self.timer_interrupt(core);
        }
        self.now = to;
    }

    /// What is due at the current instant runs: the steps of the recovery
    /// reserve that can progress, the timers due now.
    pub fn settle(&mut self) {
        self.advance(self.now);
    }

    /// A call, then what is due at its instant: the outcome of the call,
    /// or, if it waited and its wait ended meanwhile, the outcome it was
    /// woken with.
    pub fn apply_settled(&mut self, thread: ObjId, call: crate::Syscall) -> Outcome {
        let outcome = self.apply(thread, call);
        if outcome != Ok(Output::Waits) {
            return outcome;
        }
        self.settle();
        match self.threads.get(&thread) {
            Some(t) if t.wait == crate::Wait::None => {
                t.outcome.clone().unwrap_or(Ok(Output::Waits))
            }
            _ => Ok(Output::Waits),
        }
    }

    /// The earliest instant a core's hardware timer is set to, and the core.
    pub(crate) fn next_timer(&self) -> Option<(u64, usize)> {
        (0..self.width())
            .filter(|&core| !self.cores[core].silent)
            .filter_map(|core| Some((self.hardware(core)?, core)))
            .min()
    }

    /// A timer interrupt on a core: an event that processes its due events.
    pub(crate) fn timer_interrupt(&mut self, core: usize) {
        *self.last_event = None;
        let entry = self.begin_on(core);
        self.due();
        self.end(entry);
        self.auto_deliver();
    }

    /// The start of an event on a core: the event runs there.
    pub(crate) fn begin_on(&mut self, core: usize) -> Option<(ObjId, bool)> {
        *self.at = core;
        self.begin()
    }

    /// Order inside an event, 1: the time since the previous event of the
    /// core is charged to the context that was running there: to its
    /// consumed time, and to its budget if it ran from the ready queue. The
    /// kernel's own time is taken as zero. Returns the context running at
    /// the entry.
    pub(crate) fn begin(&mut self) -> Option<(ObjId, bool)> {
        self.table.begin_journal();
        self.table.member_writes.clear();
        self.table.fence_trace.clear();
        self.table.role_writes.clear();
        self.table.gated.0 = false;
        self.submitted.0.clear();
        self.applied.0.clear();
        self.begin_locks();
        let at = *self.at;
        // A core that wakes from idle clears its idle word, and its beacon's
        // `idle_until` is 0 again at its first event after idle (D-131).
        if self.cores[at].running.is_none() {
            self.wake_core();
            let number = self.cores[at].number as usize;
            self.table.core_beacons[number].idle_until.0 = 0;
        }
        self.change_list.clear();
        self.no_context_list.clear();
        *self.snapshot = self.scheds.keys().map(|&id| (id, self.snap(id))).collect();
        let params = self
            .scheds
            .iter()
            .map(|(&id, s)| (id, (s.budget, s.period)));
        *self.event = crate::EventLog {
            entered_from: self.entered,
            params: params.collect(),
            ..crate::EventLog::default()
        };
        let elapsed = self.now - self.cores[at].previous;
        self.cores[at].previous = self.now;
        self.event.counts = self
            .scheds
            .iter()
            .map(|(&id, s)| (id, (s.consumed, s.count, s.counted)))
            .collect();
        // The time counters of `node_status`: idle when nothing ran, slack
        // when a context ran from its slack queue.
        match self.cores[at].running {
            None => self.cores[at].idle_ns.0 += elapsed,
            Some((_, false)) => self.cores[at].slack_ns.0 += elapsed,
            Some(_) => {}
        }
        let (context, ready) = self.cores[at].running?;
        let counter = self.machine.counter;
        let running = self.scheds.get_mut(&context)?;
        running.consumed = running.consumed.saturating_add(elapsed);
        // Instructions retired: charged at the same events as the time,
        // one per nanosecond on the model's cores.
        if running.counted && counter {
            running.count = running.count.saturating_add(elapsed);
        }
        if ready {
            running.remaining = running.remaining.saturating_sub_unsigned(elapsed);
        }
        self.observe();
        Some((context, ready))
    }

    /// What a context is, for the change list.
    fn snap(&self, context: ObjId) -> Snap {
        let sched = &self.scheds[&context];
        Snap {
            eligible: self.eligible(context),
            remaining: sched.remaining,
            throttled: sched.throttled,
            wanted: self.wanted(context),
        }
    }

    /// Order inside an event: a context is appended to the change list the
    /// first time in the event that its eligibility, its budget or its key
    /// changes; a thread to the no-context list when it is marked while
    /// runnable or becomes runnable while marked. Called after each change
    /// of the state of a thread or a context.
    pub(crate) fn observe(&mut self) {
        let changed: Vec<ObjId> = self
            .scheds
            .keys()
            .copied()
            .filter(|id| !self.change_list.contains(id))
            .filter(|&id| self.snapshot.get(&id) != Some(&self.snap(id)))
            .collect();
        self.change_list.extend(changed);
        let marked: Vec<ObjId> = self
            .threads
            .iter()
            .filter(|(id, t)| {
                t.lost && Self::runnable_thread(t) && !self.no_context_list.contains(id)
            })
            .map(|(&id, _)| id)
            .collect();
        self.no_context_list.extend(marked);
    }

    /// Order inside an event, 4 to 7: the exhaustion of the context that
    /// ran from the ready queue, the faults *no context*, every context in
    /// its queue, then the choice.
    pub(crate) fn end(&mut self, entry: Option<(ObjId, bool)>) {
        self.trace_transitions();
        // A thread marked *leaving its core* that this core runs: the core
        // saves its registers and stops running it.
        self.leave();
        self.choose(entry);
        let at = *self.at;
        // A core that goes idle answers its pending set first; a completed
        // request can wake its waiter onto this core, so the choice is made
        // again and the core that finds a thread leaves idle (D-141).
        if self.cores[at].thread.is_none() && !self.cores[at].pending.is_empty() {
            self.go_idle();
            self.choose(None);
            if self.cores[at].thread.is_some() {
                self.wake_core();
            }
        }
        self.publish_counters();
        // Instructions retired: when a counted context stops running, the
        // kernel reads the processor's class of the code it ran.
        let now = self.cores[at].running.map(|(context, _)| context);
        if let Some((stopped, _)) = entry
            && now != Some(stopped)
            && let Some(sched) = self.scheds.get_mut(&stopped)
            && sched.counted
        {
            sched.hw_class = self
                .machine
                .classifier
                .map_or(0, |class| 1 + u64::from(class));
        }
        // What a core enables for a thread: the baseline of the node and the
        // optional features of the thread's mask, written at a switch.
        if let Some(holder) = self.cores[at].thread {
            self.cores[at].enabled = self.enabled_for(holder);
        }
        // The root the core runs on (G7), and an idle core's idle word.
        match self.cores[at].thread {
            Some(_) => self.load_root(),
            None => {
                self.go_idle();
                self.cache_here();
            }
        }
        // Wake-ups across cores: a context that should take another core
        // makes this core send that core the reschedule interrupt.
        self.reschedule_others();
        self.beat();
        self.end_locks();
    }

    /// The core beacon at the exit of every event of a core: its heartbeat,
    /// and while it goes idle the instant of its next armed timer (the
    /// largest value if none), so that it need not beat while idle.
    fn beat(&mut self) {
        let at = *self.at;
        self.cores[at].events.0 += 1;
        let number = self.cores[at].number as usize;
        let (node, generation) = (self.node, self.table.generation(self.node));
        // `idle_until` is written on the clock it is compared on: the
        // monotonic instant of the next timer less the stopped time when
        // the core goes idle (Two builds, and the debugger stub, D-139).
        let idle_until = match self.cores[at].running {
            Some(_) => 0,
            None => self
                .hardware(at)
                .map_or(u64::MAX, |t| t.saturating_sub(self.table.stopped_time)),
        };
        let beacon = &mut self.table.core_beacons[number];
        beacon.heartbeat.0 = self.cores[at].events.0;
        beacon.idle_until.0 = idle_until;
        beacon.ran = (node, generation);
    }

    /// After an event on one core, every other core whose choice the event
    /// changed is sent the reschedule interrupt: it is idle and a context
    /// placed on it became eligible, or a context placed on it should
    /// preempt the one it runs, or what it runs is no longer what it would
    /// choose. Never on a node of one core.
    fn reschedule_others(&mut self) {
        let at = *self.at;
        for core in (0..self.width()).filter(|&c| c != at) {
            let wanted = self.pick(core);
            let runs = self.cores[core].running;
            let holder_moved = runs.is_some_and(|(context, _)| {
                !self.scheds.contains_key(&context)
                    || self.holder(context) != self.cores[core].thread
            });
            let differs = wanted.map(|w| w.0) != runs.map(|r| r.0);
            if (differs || holder_moved) && !self.cores[core].ipis.contains(&crate::Ipi::Reschedule)
            {
                self.post(core, crate::Ipi::Reschedule);
            }
        }
    }

    /// The state features enabled for a thread on its node's cores.
    pub(crate) fn enabled_for(&self, thread: ObjId) -> u32 {
        let kind = self.machine.kind_of(self.cores[0].number as usize);
        let list = self.machine.features();
        let optional = self.threads[&thread].mask as u32
            & list.state
            & !(1 << dodos_abi::FEATURE_USER_INTERRUPTS);
        kind.baseline(list) | optional
    }

    fn choose(&mut self, entry: Option<(ObjId, bool)>) {
        if let Some((context, true)) = entry {
            let exhausted = self
                .scheds
                .get(&context)
                .is_some_and(|s| s.configured && s.throttled.is_none() && s.remaining <= 0);
            if exhausted {
                self.exhaust(context);
            }
        }
        // The choice: a holder that would run without an address space, or
        // with a dying one, raises the fault *no address space* instead, and
        // the choice is made again; so does a holder of a revoked task,
        // after it is suspended. Each round takes one thread out of what can
        // run, so the loop ends.
        let mut entered = 0;
        let at = *self.at;
        for _ in 0..=self.threads.len() {
            self.no_context();
            entered += self.requeue();
            let picked = self.pick(at);
            self.cores[at].running = picked;
            self.cores[at].thread = picked.and_then(|(context, _)| self.holder(context));
            let Some(holder) = self.cores[at].thread else {
                *self.last_event = self.last_event.map(|(kind, n)| (kind, n + entered));
                return;
            };
            // Tasks and marks: a thread of a revoked task never runs again;
            // when the choice would take it, it is suspended and the choice
            // is made again.
            let task = self.threads[&holder].task;
            if task.is_some_and(|task| self.tasks[&task].revoked) {
                live(&mut self.threads, holder).suspended = true;
                self.observe();
                continue;
            }
            let aspace = self.threads[&holder].aspace;
            if aspace.is_some_and(|aspace| !self.dying(aspace)) {
                *self.last_event = self.last_event.map(|(kind, n)| (kind, n + entered));
                return;
            }
            self.raise(holder, FaultKind::NoAddressSpace, [0; 3]);
        }
        unreachable!("every round of the choice takes a thread out");
    }

    /// The budget follows the call: at the end of the event in which a
    /// thread marked as having lost its context is runnable, it raises the
    /// fault *no context*, which lends nothing.
    ///
    /// The faults are raised in the order of the no-context list; a fault
    /// *no context* goes one level only, so no passive handler faults in
    /// turn and the work of one event stays constant.
    fn no_context(&mut self) {
        self.observe();
        let mut next = 0;
        while let Some(&thread) = self.no_context_list.get(next) {
            next += 1;
            // A thread that is no longer marked and runnable raises nothing.
            let raised = self.threads.get(&thread);
            if !raised.is_some_and(|t| t.lost && Self::runnable_thread(t)) {
                continue;
            }
            // The fault leaves the thread waiting or suspended; the mark
            // stays until a context is bound or lent to it.
            self.raise(thread, FaultKind::NoContext, [0; 3]);
            self.observe();
        }
        self.no_context_list.clear();
    }

    /// Scheduling contexts: a thread's context, its own or the one lent to
    /// it.
    pub(crate) fn context_of(&self, thread: ObjId) -> Option<ObjId> {
        let lent = self
            .lends
            .values()
            .find(|(_, receiver)| *receiver == thread);
        self.threads[&thread]
            .home
            .or(lent.map(|(context, _)| *context))
    }

    /// A thread is runnable when it is not suspended, does not wait, and is
    /// not marked *leaving its core* (D-131).
    pub(crate) fn runnable_thread(thread: &Thread) -> bool {
        !thread.suspended && thread.wait == crate::Wait::None && !thread.leaving
    }

    /// The budget follows the call: the holder of a context is its home, if
    /// the home is runnable; otherwise the receiver of the top lend.
    pub(crate) fn holder(&self, context: ObjId) -> Option<ObjId> {
        let sched = &self.scheds[&context];
        let home = sched
            .home
            .filter(|home| Self::runnable_thread(&self.threads[home]));
        home.or_else(|| Some(self.lends[sched.chain.last()?].1))
    }

    /// Scheduling contexts: a context is eligible when it is configured,
    /// not dying, and its holder is runnable.
    fn eligible(&self, context: ObjId) -> bool {
        let holder = self.holder(context);
        self.scheds[&context].configured
            && !self.scheds[&context].moving
            && !self.dying(context)
            && holder.is_some_and(|holder| Self::runnable_thread(&self.threads[&holder]))
    }

    /// What runs next: the queue and the key a context belongs at.
    fn wanted(&self, context: ObjId) -> Option<(Queue, u64)> {
        let sched = &self.scheds[&context];
        if !sched.configured || self.dying(context) {
            return None;
        }
        match (self.eligible(context), sched.throttled) {
            (true, None) => Some((Queue::Ready, sched.deadline)),
            (true, Some((instant, _))) => Some((Queue::Slack(sched.class), instant)),
            (false, Some((instant, _))) => Some((Queue::Sleeping, instant)),
            (false, None) => None,
        }
    }

    /// Order inside an event, 6: every context whose eligibility or budget
    /// changed is put in its queue, in the order of the change list; one
    /// that became eligible with budget gets the wake-up rule first. A
    /// context that changes queue, or whose key changes, enters again,
    /// after every context with the same key. Returns how many contexts
    /// entered a new queue position.
    fn requeue(&mut self) -> usize {
        self.observe();
        let mut entered = 0;
        let mut next = 0;
        while let Some(&context) = self.change_list.get(next) {
            next += 1;
            if !self.scheds.contains_key(&context) {
                continue;
            }
            let was = self.scheds[&context].queue.map(|q| q.queue);
            let was_eligible = matches!(was, Some(Queue::Ready | Queue::Slack(_)));
            if !was_eligible && self.eligible(context) && self.scheds[&context].throttled.is_none()
            {
                self.wake_up(context);
            }
            let wanted = self.wanted(context);
            let sched = live(&mut self.scheds, context);
            let same = sched.queue.map(|q| (q.queue, q.key)) == wanted;
            if !same {
                sched.queue = wanted.map(|(queue, key)| Queued {
                    queue,
                    key,
                    entered: self.entered,
                });
                self.entered += 1;
                entered += usize::from(wanted.is_some());
            }
        }
        // Every context out of its place was on the list: a change that no
        // observation saw is a fault of the model.
        let missed = self
            .scheds
            .keys()
            .find(|&&id| self.scheds[&id].queue.map(|q| (q.queue, q.key)) != self.wanted(id));
        assert!(missed.is_none(), "a change of {missed:?} was not observed");
        self.event.changes.append(&mut self.change_list);
        *self.snapshot = self.scheds.keys().map(|&id| (id, self.snap(id))).collect();
        entered
    }

    /// The wake-up rule: a context with budget that becomes eligible keeps
    /// r and d if r × T ≤ (d − now) × B and d is after now; otherwise
    /// r = B and d = now + T.
    ///
    /// Pending parameters come into force at the next replenishment or at
    /// the next reset of the wake-up rule, whichever comes first.
    fn wake_up(&mut self, context: ObjId) {
        let now = self.now;
        let sched = live(&mut self.scheds, context);
        let (b, t) = (u128::from(sched.budget), u128::from(sched.period));
        let r = sched.remaining.max(0) as u128;
        let keeps = sched.deadline > now && r * t <= u128::from(sched.deadline - now) * b;
        if !keeps {
            if let Some((budget, period)) = sched.pending.take() {
                (sched.budget, sched.period) = (budget, period);
            }
            sched.remaining = sched.budget as i64;
            sched.deadline = now + sched.period;
            self.event.resets.push(context);
        }
    }

    /// Exhaustion: with n = 1 + ⌊−r / B⌋, the replenishment instant is
    /// d + (n − 1) × T; not after now, the context is replenished at once.
    fn exhaust(&mut self, context: ObjId) {
        let sched = live(&mut self.scheds, context);
        let budget = sched.budget.max(1);
        let n = 1 + sched.remaining.unsigned_abs() / budget;
        let instant = sched.deadline + (n - 1) * sched.period;
        if instant <= self.now {
            self.replenish(context, n);
        } else {
            sched.throttled = Some((instant, n));
        }
        self.observe();
    }

    /// Replenishment: r += n × B, d += n × T, and if d is not after now,
    /// d = now + T.
    ///
    /// If r is still not positive, the context is exhausted again at once,
    /// in the same event; its next instant is after now, so this ends.
    pub(crate) fn replenish(&mut self, context: ObjId, n: u64) {
        let now = self.now;
        self.event.resets.push(context);
        let sched = live(&mut self.scheds, context);
        if let Some((budget, period)) = sched.pending.take() {
            (sched.budget, sched.period) = (budget, period);
        }
        sched.remaining = sched.remaining.saturating_add((n * sched.budget) as i64);
        sched.deadline += n * sched.period;
        if sched.deadline <= now {
            sched.deadline = now + sched.period;
        }
        sched.throttled = None;
        if sched.remaining <= 0 {
            self.exhaust(context);
        }
        self.observe();
    }

    /// The choice on a core: the first context of its ready queue;
    /// otherwise the first of its slack queue of the smallest class number;
    /// otherwise nothing. Between equal keys, the context that entered
    /// first.
    pub(crate) fn pick(&self, core: usize) -> Option<(ObjId, bool)> {
        let first = |queue: Queue| {
            let queued = self.scheds.iter().filter_map(|(&id, sched)| {
                let place = sched.queue.filter(|place| place.queue == queue)?;
                (sched.core == core).then_some(((place.key, place.entered), id))
            });
            queued.min().map(|(_, id)| id)
        };
        if let Some(context) = first(Queue::Ready) {
            return Some((context, true));
        }
        (0..3)
            .find_map(|class| first(Queue::Slack(class)))
            .map(|context| (context, false))
    }

    /// The due event of the core's invalidation request, on the hardware
    /// clock: `CORE_ANSWER_NS` after its start, which is read on the clock
    /// less the stopped time, so the stops already counted are added back
    /// (The stopped time, D-137). While the release word of the stub is
    /// raised the due event suspends: none is armed, and the first event
    /// after the release arms it again on the new stopped time (The
    /// debugger stub, D-135).
    pub(crate) fn request_due(&self, core: usize) -> Option<u64> {
        let request = self.cores[core].request.as_ref();
        let request = request.filter(|_| !self.table.release);
        request.map(|r| r.started + self.table.core_answer_ns + self.table.stopped_time)
    }

    /// Due events and the hardware timer of a core: after the choice, the
    /// earliest of the instant the running context's budget ends, the
    /// first replenishment instant of its throttled queues, the first
    /// firing instant of its timer queue, the due event of its invalidation
    /// request (`CORE_ANSWER_NS` after its start), on the node's first core
    /// the due step of its recovery reserve (`reserve_due`), and, while the
    /// core runs a thread, its previous event + `STALE_NS` / 2, an event
    /// that only beats (D-131). An idle core with nothing armed sets no
    /// timer.
    pub(crate) fn hardware(&self, core: usize) -> Option<u64> {
        let part = &self.cores[core];
        let budget = part.running.and_then(|(context, ready)| {
            let sched = self.scheds.get(&context).filter(|_| ready)?;
            Some(part.previous + sched.remaining.max(0) as u64)
        });
        let replenished = self
            .scheds
            .values()
            .filter(|s| s.core == core)
            .filter_map(|s| s.queue.and(s.throttled));
        let fired = self
            .timers
            .values()
            .flatten()
            .filter(|armed| armed.core == core)
            .map(|armed| armed.instant);
        let answer = self.request_due(core);
        let beat = part
            .thread
            .map(|_| part.previous + self.table.stale_ns / 2)
            .filter(|_| self.table.stale_ns > 0);
        let reserve = (core == 0).then(|| self.reserve_due()).flatten();
        let instants = replenished
            .map(|(instant, _)| instant)
            .chain(fired)
            .chain(budget)
            .chain(answer)
            .chain(beat)
            .chain(reserve);
        instants.min()
    }

    /// Order inside an event, 2: a timer interrupt processes at most 16 due
    /// events, the earliest instant first; at equal instants replenishments
    /// before timers, the slack queues by class, then the sleeping queue,
    /// then the timers; inside one queue, the order of the queue.
    fn due(&mut self) {
        let now = self.now;
        let at = *self.at;
        // The due event of the core's invalidation request: a request not
        // complete `CORE_ANSWER_NS` after its start is a failure of the
        // node, which accuses itself (A node of several cores). While the
        // release word of the stub is raised, no node accuses itself: the
        // due event suspends until it is lowered (The debugger stub, D-135,
        // D-137; found by pass 23's scenario of E23).
        let late = !self.table.release
            && self.cores[at]
                .request
                .as_ref()
                .is_some_and(|r| r.started + self.table.core_answer_ns <= self.membership_now());
        if late {
            self.accuse_self(dodos_abi::CrashReason::CoreSilent as u32);
        }
        self.watch_own_cores();
        // The recovery reserve runs on the node's first core.
        if at == 0 {
            self.reserve_step();
        }
        let mut due: Vec<((u64, u64, u64), Due)> = Vec::new();
        for (&id, sched) in &self.scheds {
            if sched.core != at {
                continue;
            }
            let (Some(place), Some((instant, _))) = (sched.queue, sched.throttled) else {
                continue;
            };
            let rank = match place.queue {
                Queue::Slack(class) => class,
                _ => 3,
            };
            if instant <= now {
                due.push(((instant, rank, place.entered), Due::Replenish(rank, id)));
            }
        }
        for (&id, armed) in &self.timers {
            if let Some(armed) = armed.filter(|armed| armed.instant <= now && armed.core == at) {
                due.push(((armed.instant, 4, armed.entered), Due::Fire(id)));
            }
        }
        due.sort();
        for (_, event) in due.into_iter().take(TIME_EVENT_STEP) {
            match event {
                Due::Replenish(_, context) => {
                    let n = self.scheds[&context].throttled.expect("throttled").1;
                    self.replenish(context, n);
                }
                Due::Fire(timer) => self.fire(timer),
            }
        }
    }

    /// Start-up state: the context of the first thread is configured with
    /// its budget and period, class 1, and bound to it. The recovery
    /// reserve (Cores) is admitted in the node's sum from start-up.
    pub(crate) fn first_context(
        &mut self,
        context: ObjId,
        thread: ObjId,
        budget: u64,
        period: u64,
    ) {
        let sched = live(&mut self.scheds, context);
        (sched.configured, sched.budget, sched.period, sched.class) = (true, budget, period, 1);
        (sched.remaining, sched.deadline) = (budget as i64, self.now + period);
        sched.home = Some(thread);
        // Placed on the node's first core, with the cores set of all its
        // cores; the recovery reserve is admitted on that core.
        sched.core = 0;
        let set = self.cores_set();
        live(&mut self.scheds, context).cores = set;
        live(&mut self.threads, thread).home = Some(context);
        self.cores[0].admitted = bandwidth(budget, period) + RECOVERY_BANDWIDTH;
        *self.at = 0;
        self.end(None);
    }

    /// The cores set of a new context: every core of its node, as a window
    /// and a mask. "The cores a node starts on lie in one window of 64 core
    /// numbers" (The calls, `node_start`, gap 7), which `check_window`
    /// checks, so one window holds them all.
    pub(crate) fn cores_set(&self) -> (u64, u64) {
        let window = u64::from(self.cores[0].number) / 64;
        let mask = self
            .cores
            .iter()
            .fold(0, |mask, c| mask | 1 << (c.number % 64));
        (window, mask)
    }

    /// Whether a core number lies in a context's cores set.
    fn in_set(set: (u64, u64), number: u64) -> bool {
        set.0 == number / 64 && set.1 & 1 << (number % 64) != 0
    }

    // The calls of the scheduling contexts.

    /// The `node` capability of a call on a context: `write`, and it must
    /// name the node that holds the context (`invalid`, judged right after
    /// its three checks).
    fn own_node(&self, thread: ObjId, node: SlotAddr) -> Result<(), Error> {
        let node = self.object(thread, node, ObjectKind::Node, Rights::WRITE)?;
        match self.nodes[&node] == self.node {
            true => Ok(()),
            false => Err(Error::Invalid),
        }
    }

    /// `sched_configure`: the parameters, and the core of its node where
    /// the context is placed (D-129). A core other than the one where it is
    /// placed moves it there, admitted on the new core; its holder, if it
    /// runs on another core, is marked *leaving its core*, and runs on the
    /// new core once the mark is clear (no wait, D-131).
    pub(crate) fn sched_configure(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        node: SlotAddr,
        core: u64,
        parameters: Parameters,
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_node(thread, node)?;
        let Parameters {
            budget,
            period,
            class,
        } = parameters;
        // The core number must be a core of this node and in the context's
        // cores set.
        let target = self.position(core).ok_or(Error::Invalid)?;
        if !Self::in_set(self.scheds[&context].cores, core) {
            return Err(Error::Invalid);
        }
        let off = budget == 0 && period == 0;
        let on = SCHED_MIN_BUDGET_NS <= budget && budget <= period && period <= SCHED_MAX_PERIOD_NS;
        if !(off || on) || class > 2 {
            return Err(Error::Invalid);
        }
        // Admission on the core named: its sum with the new bandwidth in
        // place of the old one is at most one whole core; a context that
        // moves leaves the sum of its old core in the same call. Removing
        // a configuration never fails.
        let placed = self.scheds[&context].core;
        let before = self.scheds[&context].bandwidth();
        let kept = if placed == target { before } else { 0 };
        let sum = self.cores[target].admitted - kept + bandwidth(budget, period);
        if sum > ADMISSION_WHOLE_CORE {
            return Err(Error::Budget);
        }
        self.cores[placed].admitted -= before;
        self.cores[target].admitted += bandwidth(budget, period);
        if placed != target {
            // A move of a context whose holder runs on its old core marks
            // the context moving with that thread's leave (D-141).
            let moving = self
                .holder(context)
                .is_some_and(|holder| self.mark_leaving(holder));
            let sched = live(&mut self.scheds, context);
            sched.moving |= moving;
            sched.core = target;
            sched.queue = None;
            if !self.change_list.contains(&context) {
                self.change_list.push(context);
            }
        }
        let now = self.now;
        // A configuration from or to the state not configured sets the
        // parameters in force; on a configured context they are pending.
        if off || !self.scheds[&context].configured {
            self.event.resets.push(context);
        }
        let sched = live(&mut self.scheds, context);
        if off {
            // "With budget 0: the context is not configured, r = 0, d = 0,
            // consumed = 0" (the final review's verification of lot B,
            // finding 9).
            sched.configured = false;
            sched.consumed = 0;
            (sched.remaining, sched.deadline, sched.throttled) = (0, 0, None);
            (sched.budget, sched.period, sched.pending) = (0, 0, None);
        } else if !sched.configured {
            sched.configured = true;
            (sched.budget, sched.period, sched.pending) = (budget, period, None);
            (sched.remaining, sched.deadline) = (budget as i64, now + period);
        } else {
            sched.remaining = sched.remaining.min(budget as i64);
            sched.pending = Some((budget, period));
        }
        sched.class = class;
        self.observe();
        Ok(Output::Done)
    }

    pub(crate) fn sched_bind(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        target: SlotAddr,
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        // `busy` if the context has a home, if the thread has an own
        // context, or if a context is lent to it.
        let lent = self.lends.values().any(|(_, receiver)| *receiver == target);
        if self.scheds[&context].home.is_some() || self.threads[&target].home.is_some() || lent {
            return Err(Error::Busy);
        }
        live(&mut self.scheds, context).home = Some(target);
        let bound = live(&mut self.threads, target);
        (bound.home, bound.lost) = (Some(context), false);
        self.observe();
        Ok(Output::Done)
    }

    /// `sched_unbind`: the home becomes none and has no context, without
    /// being marked; the lends go on.
    pub(crate) fn sched_unbind(&mut self, thread: ObjId, sched: SlotAddr) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        if let Some(home) = live(&mut self.scheds, context).home.take() {
            live(&mut self.threads, home).home = None;
        }
        self.observe();
        Ok(Output::Done)
    }

    /// `sched_set_cores`: the cores 64 × window + i, for each bit i of the
    /// mask, by core number; the set must contain the core where the
    /// context is placed.
    pub(crate) fn sched_set_cores(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        node: SlotAddr,
        (window, mask): (u64, u64),
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_node(thread, node)?;
        let placed = u64::from(self.cores[self.scheds[&context].core].number);
        let holds = Self::in_set((window, mask), placed);
        if window >= CORE_WINDOWS || mask == 0 || !holds {
            return Err(Error::Invalid);
        }
        live(&mut self.scheds, context).cores = (window, mask);
        Ok(Output::Done)
    }

    /// `sched_read`: consumed, remaining, deadline and the state bits; a
    /// context that is not configured reads 0, 0, 0 and its state bits.
    ///
    /// Consumed counts since the context was last configured from the
    /// state not configured, and reads 0 while it is not configured.
    pub(crate) fn sched_read(&mut self, thread: ObjId, sched: SlotAddr) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::READ)?;
        let sched = &self.scheds[&context];
        let bit = |set: bool, bit: u64| if set { bit } else { 0 };
        let state = bit(sched.configured, SCHED_STATE_CONFIGURED)
            | bit(sched.throttled.is_some(), SCHED_STATE_THROTTLED)
            | bit(sched.home.is_some(), SCHED_STATE_HOME)
            | bit(!sched.chain.is_empty(), SCHED_STATE_LENT)
            | bit(sched.counted, SCHED_STATE_COUNTED)
            | sched.hw_class << SCHED_STATE_HW_CLASS_SHIFT;
        let read = match sched.configured {
            true => (sched.consumed, sched.remaining, sched.deadline),
            false => (0, 0, 0),
        };
        Ok(Output::SchedRead {
            consumed: read.0,
            remaining: read.1,
            deadline: read.2,
            state,
            instructions: sched.count,
        })
    }

    /// `sched_count`: counting of the instructions a context retires, on
    /// or off. Off sets the count and the class to 0. No state error.
    pub(crate) fn sched_count(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        node: SlotAddr,
        mode: u64,
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_node(thread, node)?;
        let on = match mode {
            SCHED_COUNT_OFF => false,
            SCHED_COUNT_ON => true,
            _ => return Err(Error::Invalid),
        };
        let sched = live(&mut self.scheds, context);
        if !on {
            (sched.count, sched.hw_class) = (0, 0);
        }
        sched.counted = on;
        Ok(Output::Done)
    }

    /// `yield`: the context the caller runs on gives up its remaining
    /// budget, and is exhausted at once (n = 1, instant d); one already
    /// throttled enters its slack queue again, behind the contexts with the
    /// same instant.
    ///
    /// "A thread that has no context never runs, so it never makes this
    /// call" (What runs next, `yield`); the model lets any runnable thread
    /// call, and then `yield` changes nothing.
    pub(crate) fn yield_now(&mut self, thread: ObjId) -> Outcome {
        self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
        let Some(context) = self.context_of(thread) else {
            return Ok(Output::Done);
        };
        let sched = live(&mut self.scheds, context);
        if !sched.configured {
            return Ok(Output::Done);
        }
        match sched.throttled {
            None => {
                sched.remaining = 0;
                self.exhaust(context);
            }
            // It enters its slack queue again: a change of its place.
            Some(_) => {
                sched.queue = None;
                if !self.change_list.contains(&context) {
                    self.change_list.push(context);
                }
            }
        }
        self.observe();
        Ok(Output::Done)
    }

    // The budget follows the call.

    /// A lend is made when a call is delivered to a receiver that has no
    /// own context, if the caller's context exists and is not dying; it is
    /// recorded in the reply object the delivery arms. A passive receiver
    /// that receives a call that brings no lend is marked.
    pub(crate) fn lend(&mut self, caller: ObjId, reply: ObjId, receiver: ObjId) {
        if self.threads[&receiver].home.is_some() {
            return;
        }
        let context = self
            .context_of(caller)
            .filter(|&context| !self.dying(context));
        let Some(context) = context else {
            live(&mut self.threads, receiver).lost = true;
            self.observe();
            return;
        };
        self.lends.insert(reply, (context, receiver));
        live(&mut self.scheds, context).chain.push(reply);
        live(&mut self.threads, receiver).lost = false;
        self.observe();
    }

    /// A lend is released by the receive of its receiver, without a mark,
    /// or by anything else (its reply object given to another receiver,
    /// destroyed, its context destroyed), which marks the receiver.
    pub(crate) fn release_lend(&mut self, reply: ObjId, by: Option<ObjId>) {
        let Some((context, receiver)) = self.lends.remove(&reply) else {
            return;
        };
        if let Some(sched) = self.scheds.get_mut(&context) {
            sched.chain.retain(|&other| other != reply);
        }
        if by != Some(receiver)
            && let Some(receiver) = self.threads.get_mut(&receiver)
        {
            receiver.lost = true;
        }
        self.observe();
    }

    /// The lends of a thread that begins to receive, and of the reply
    /// object it gives, are released.
    pub(crate) fn release_for_receive(&mut self, thread: ObjId, reply: ObjId) {
        let own: Vec<ObjId> = self
            .lends
            .iter()
            .filter(|(_, (_, receiver))| *receiver == thread)
            .map(|(&reply, _)| reply)
            .collect();
        for lent in own.into_iter().chain([reply]) {
            self.release_lend(lent, Some(thread));
        }
    }

    /// Destroying a context, the first step: it is taken out of every queue
    /// and its bandwidth out of the sum, and its home has no context.
    ///
    /// The home is marked when the context of which it is the home becomes
    /// dying, at the first step of its removal.
    pub(crate) fn sched_dying(&mut self, context: ObjId) {
        let sched = live(&mut self.scheds, context);
        (sched.queue, sched.retired) = (None, true);
        let (core, bandwidth) = (sched.core, sched.bandwidth());
        self.cores[core].admitted -= bandwidth;
        let sched = live(&mut self.scheds, context);
        if let Some(home) = sched.home.take() {
            let thread = live(&mut self.threads, home);
            (thread.home, thread.lost) = (None, true);
        }
        self.observe();
    }

    // Timers.

    pub(crate) fn timer_set(
        &mut self,
        thread: ObjId,
        timer: SlotAddr,
        notify: SlotAddr,
        (deadline, tolerance): (u64, u64),
    ) -> Outcome {
        let timer = self.object(thread, timer, ObjectKind::Timer, Rights::WRITE)?;
        let held = self.held(thread, notify, ObjectKind::Notify, Rights::SEND)?;
        let (notify, badge) = (held.object, held.badge);
        if deadline >= TIME_LIMIT {
            return Err(Error::Invalid);
        }
        let was = self.disarm(timer);
        let instant = firing_instant(deadline, tolerance);
        let bits = if badge == 0 { 1 } else { badge };
        // Armed timers are kept in the queue of the core that ran
        // `timer_set`.
        let armed = Armed {
            instant,
            notify,
            bits,
            entered: self.entered,
            core: *self.at,
        };
        self.entered += 1;
        *live(&mut self.timers, timer) = Some(armed);
        live(&mut self.notifies, notify).timers.push(timer);
        if instant <= self.now {
            self.fire(timer);
        }
        Ok(Output::WasArmed(was))
    }

    pub(crate) fn timer_cancel(&mut self, thread: ObjId, timer: SlotAddr) -> Outcome {
        let timer = self.object(thread, timer, ObjectKind::Timer, Rights::WRITE)?;
        Ok(Output::WasArmed(self.disarm(timer)))
    }

    /// `timer_set` and `timer_cancel`: an armed timer whose firing instant
    /// is not after now fires first; then the arming, if any, is cancelled.
    /// Returns 1 if the timer was still armed.
    fn disarm(&mut self, timer: ObjId) -> u64 {
        if self.timers[&timer].is_some_and(|armed| armed.instant <= self.now) {
            self.fire(timer);
        }
        let Some(armed) = live(&mut self.timers, timer).take() else {
            return 0;
        };
        live(&mut self.notifies, armed.notify)
            .timers
            .retain(|&t| t != timer);
        1
    }

    /// Firing does what `signal` does with the capability given to
    /// `timer_set`, and the timer becomes idle; a timer whose notify is
    /// dying is disarmed without a signal.
    pub(crate) fn fire(&mut self, timer: ObjId) {
        let armed = live(&mut self.timers, timer).take().expect("armed");
        live(&mut self.notifies, armed.notify)
            .timers
            .retain(|&t| t != timer);
        if !self.dying(armed.notify) {
            live(&mut self.notifies, armed.notify).word |= armed.bits;
            self.hand_over(armed.notify);
        }
    }

    /// Destroying an armed timer takes it out of the queue and of the list
    /// of its notify, in one step.
    pub(crate) fn timer_destroyed(&mut self, timer: ObjId) {
        if let Some(Some(armed)) = self.timers.remove(&timer) {
            live(&mut self.notifies, armed.notify)
                .timers
                .retain(|&t| t != timer);
        }
    }

    /// Destroying a context: the lends are cleared from the top of the
    /// chain down, one unit each; a thread that loses its context this way
    /// is marked. Says whether there was one.
    pub(crate) fn clear_lend(&mut self, context: ObjId) -> bool {
        let Some(&top) = self.scheds[&context].chain.last() else {
            return false;
        };
        self.release_lend(top, None);
        true
    }

    /// Destroying a notify disarms its armed timers, most recently armed
    /// first, one unit each. Says whether there was one.
    pub(crate) fn disarm_one(&mut self, notify: ObjId) -> bool {
        let Some(&timer) = self.notifies[&notify].timers.last() else {
            return false;
        };
        *live(&mut self.timers, timer) = None;
        live(&mut self.notifies, notify).timers.pop();
        true
    }

    /// Destroying a context, or a thread: what is left of it is unlinked.
    pub(crate) fn sched_destroyed(&mut self, context: ObjId) {
        self.scheds.remove(&context);
        for core in &mut self.cores {
            if core.running.is_some_and(|(running, _)| running == context) {
                core.running = None;
            }
        }
    }

    /// Destroying a thread: its home context has no home (its lends go on),
    /// and its own lend is taken out of the chain in one step.
    pub(crate) fn thread_unscheduled(&mut self, thread: ObjId) {
        if let Some(home) = self.threads[&thread].home {
            live(&mut self.scheds, home).home = None;
        }
        let own: Vec<ObjId> = self
            .lends
            .iter()
            .filter(|(_, (_, receiver))| *receiver == thread)
            .map(|(&reply, _)| reply)
            .collect();
        for reply in own {
            self.release_lend(reply, Some(thread));
        }
    }
}
