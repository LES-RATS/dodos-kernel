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

    /// What runs next: the thread that runs, the holder of the context
    /// that the last choice took; `None` when the core is idle.
    pub fn running(&self) -> Option<ObjId> {
        let (context, _) = self.running?;
        self.holder(context)
    }

    /// Clock: time passes up to `to`. The reference model processes every
    /// due event at its instant: a timer interrupt is an event at each
    /// instant the hardware timer is set to, up to `to`.
    pub fn advance(&mut self, to: u64) {
        assert!(to >= self.now && to < TIME_LIMIT, "the clock is monotonic");
        while let Some(instant) = self.hardware().filter(|&instant| instant <= to) {
            self.now = instant.max(self.now);
            *self.last_event = None;
            let entry = self.begin();
            self.due();
            self.end(entry);
        }
        self.now = to;
    }

    /// Order inside an event, 1: the time since the previous event is
    /// charged to the context that was running: to its consumed time, and
    /// to its budget if it ran from the ready queue. The kernel's own time
    /// is taken as zero. Returns the context running at the entry.
    pub(crate) fn begin(&mut self) -> Option<(ObjId, bool)> {
        self.table.writes.clear();
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
        let elapsed = self.now - self.previous;
        self.previous = self.now;
        self.event.counts = self
            .scheds
            .iter()
            .map(|(&id, s)| (id, (s.consumed, s.count, s.counted)))
            .collect();
        let (context, ready) = self.running?;
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
        self.choose(entry);
        // Instructions retired: when a counted context stops running, the
        // kernel reads the processor's class of the code it ran.
        let now = self.running.map(|(context, _)| context);
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
        if let Some(holder) = self.running() {
            self.enabled = self.enabled_for(holder);
        }
    }

    /// The state features enabled for a thread on its node's cores.
    pub(crate) fn enabled_for(&self, thread: ObjId) -> u32 {
        let kind = self.machine.own_kind(self.node);
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
        for _ in 0..=self.threads.len() {
            self.no_context();
            entered += self.requeue();
            self.running = self.pick();
            let Some(holder) = self.running() else {
                *self.last_event = self.last_event.map(|(kind, _)| (kind, entered));
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
                *self.last_event = self.last_event.map(|(kind, _)| (kind, entered));
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

    fn runnable_thread(thread: &Thread) -> bool {
        !thread.suspended && thread.wait == crate::Wait::None
    }

    /// The budget follows the call: the holder of a context is its home, if
    /// the home is runnable; otherwise the receiver of the top lend.
    fn holder(&self, context: ObjId) -> Option<ObjId> {
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
    fn replenish(&mut self, context: ObjId, n: u64) {
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

    /// The choice: the first context of the ready queue; otherwise the
    /// first of the slack queue of the smallest class number; otherwise
    /// nothing. Between equal keys, the context that entered first.
    fn pick(&self) -> Option<(ObjId, bool)> {
        let first = |queue: Queue| {
            let queued = self.scheds.iter().filter_map(|(&id, sched)| {
                let place = sched.queue.filter(|place| place.queue == queue)?;
                Some(((place.key, place.entered), id))
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

    /// Due events and the hardware timer: after the choice, the earliest of
    /// the instant the running context's budget ends, the first
    /// replenishment instant and the first firing instant.
    fn hardware(&self) -> Option<u64> {
        let budget = self.running.and_then(|(context, ready)| {
            let sched = self.scheds.get(&context).filter(|_| ready)?;
            Some(self.now + sched.remaining.max(0) as u64)
        });
        let replenished = self
            .scheds
            .values()
            .filter_map(|s| s.queue.and(s.throttled));
        let fired = self.timers.values().flatten().map(|armed| armed.instant);
        let instants = replenished
            .map(|(instant, _)| instant)
            .chain(fired)
            .chain(budget);
        instants.min()
    }

    /// Order inside an event, 2: a timer interrupt processes at most 16 due
    /// events, the earliest instant first; at equal instants replenishments
    /// before timers, the slack queues by class, then the sleeping queue,
    /// then the timers; inside one queue, the order of the queue.
    fn due(&mut self) {
        let now = self.now;
        let mut due: Vec<((u64, u64, u64), Due)> = Vec::new();
        for (&id, sched) in &self.scheds {
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
            if let Some(armed) = armed.filter(|armed| armed.instant <= now) {
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
        live(&mut self.threads, thread).home = Some(context);
        self.admitted = bandwidth(budget, period) + RECOVERY_BANDWIDTH;
        self.end(None);
    }

    // The calls of the scheduling contexts.

    /// The `core` capability of a call: `write`, and the core of the kernel
    /// that holds the context (`invalid`, judged right after its three
    /// checks). One node: the boot core, node 0.
    fn own_core(&self, thread: ObjId, core: SlotAddr) -> Result<(), Error> {
        let core = self.object(thread, core, ObjectKind::Core, Rights::WRITE)?;
        match self.cores[&core] == u32::from(self.node) {
            true => Ok(()),
            false => Err(Error::Invalid),
        }
    }

    pub(crate) fn sched_configure(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        core: SlotAddr,
        parameters: Parameters,
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_core(thread, core)?;
        let Parameters {
            budget,
            period,
            class,
        } = parameters;
        let off = budget == 0 && period == 0;
        let on = SCHED_MIN_BUDGET_NS <= budget && budget <= period && period <= SCHED_MAX_PERIOD_NS;
        if !(off || on) || class > 2 {
            return Err(Error::Invalid);
        }
        // Admission: the sum with the new bandwidth in place of the old one
        // is at most one whole core; removing a configuration never fails.
        let before = self.scheds[&context].bandwidth();
        let sum = self.admitted - before + bandwidth(budget, period);
        if sum > ADMISSION_WHOLE_CORE {
            return Err(Error::Budget);
        }
        self.admitted = sum;
        let now = self.now;
        // A configuration from or to the state not configured sets the
        // parameters in force; on a configured context they are pending.
        if off || !self.scheds[&context].configured {
            self.event.resets.push(context);
        }
        let sched = live(&mut self.scheds, context);
        if off {
            sched.configured = false;
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
    /// mask; the set must hold the own core of the node.
    ///
    /// A core is numbered by its place in the core list of the start-up
    /// page (Values of the machine); the own core of node i is core i.
    pub(crate) fn sched_set_cores(
        &mut self,
        thread: ObjId,
        sched: SlotAddr,
        core: SlotAddr,
        (window, mask): (u64, u64),
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_core(thread, core)?;
        let own = u64::from(self.node);
        let holds = window == own / 64 && mask & 1 << (own % 64) != 0;
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
        core: SlotAddr,
        mode: u64,
    ) -> Outcome {
        let context = self.object(thread, sched, ObjectKind::Sched, Rights::MANAGE)?;
        self.own_core(thread, core)?;
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
    /// CONTRACT-GAP (`yield`): a caller with no context does not run in a
    /// kernel; the model lets any runnable thread call, and then `yield`
    /// changes nothing.
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
        self.admitted -= sched.bandwidth();
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
        let armed = Armed {
            instant,
            notify,
            bits,
            entered: self.entered,
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
        if self.running.is_some_and(|(running, _)| running == context) {
            self.running = None;
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
