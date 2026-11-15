//! `thread_move` (Unlike cores): the whole state of a thread goes into a
//! thread made beforehand on another node, through a message between
//! kernels. The node of the thread judges what it holds and sends the
//! state; the destination node judges its own objects, admits the context
//! and answers; at the answer the source gives up its context.

use crate::remote::{Body, Request};
use crate::{Model, ObjId, Outcome, Output, Wait, live};
use dodos_abi::{
    ADMISSION_WHOLE_CORE, Error, FeatureMask, ObjectKind, Rights, SCHED_MAX_PERIOD_NS,
    SCHED_MIN_BUDGET_NS, SlotAddr, bandwidth,
};

/// What crosses: the thread's state and its context's.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    pub(crate) regs: Vec<u64>,
    pub(crate) cancelled_up_to: u64,
    /// The thread's effective marks.
    pub(crate) marks: u64,
    pub(crate) mask: u64,
    /// The parameters in force, the pending ones, and the class.
    pub(crate) budget: u64,
    pub(crate) period: u64,
    pub(crate) pending: Option<(u64, u64)>,
    pub(crate) class: u64,
    pub(crate) cores: (u64, u64),
    pub(crate) remaining: i64,
    pub(crate) deadline: u64,
    pub(crate) throttled: Option<(u64, u64)>,
    pub(crate) consumed: u64,
    pub(crate) counted: bool,
}

impl Model {
    /// `thread_move`: the checks of the source node, in the order of the
    /// table of checks, then the state of the thread and of its context;
    /// the caller and the thread wait for the destination node.
    pub(crate) fn thread_move(
        &mut self,
        caller: ObjId,
        source: SlotAddr,
        (destination, sched, core): (SlotAddr, SlotAddr, SlotAddr),
        (budget, period, class): (u64, u64, u64),
    ) -> Outcome {
        let source = self.object(caller, source, ObjectKind::Thread, Rights::MANAGE)?;
        let target = self.remote_object(caller, destination, ObjectKind::Thread)?;
        let Some(&node_remote) = self.remotes.get(&target) else {
            return Err(Error::Invalid);
        };
        let node = node_remote.node;
        let context_cap = self.remote_object(caller, sched, ObjectKind::Sched)?;
        let sched_remote = self.remotes.get(&context_cap).copied();
        let Some(sched_remote) = sched_remote.filter(|r| r.node == node) else {
            return Err(Error::Invalid);
        };
        let core = self.object(caller, core, ObjectKind::Core, Rights::WRITE)?;
        if self.cores[&core] != u32::from(node) {
            return Err(Error::Invalid);
        }
        let keep = budget == 0 && period == 0 && class == 0;
        let on = SCHED_MIN_BUDGET_NS <= budget && budget <= period && period <= SCHED_MAX_PERIOD_NS;
        if !keep && (!on || class > 2) {
            return Err(Error::Invalid);
        }
        let moved = &self.threads[&source];
        if !moved.suspended || moved.wait != Wait::None {
            return Err(Error::Busy);
        }
        let Some(context) = moved.home else {
            return Err(Error::Busy);
        };
        let held = &self.scheds[&context];
        if !held.configured || !held.chain.is_empty() {
            return Err(Error::Busy);
        }
        let (budget, period, pending, class) = match keep {
            true => (held.budget, held.period, held.pending, held.class),
            false => (budget, period, None, class),
        };
        let state = Moved {
            regs: moved.regs.clone(),
            cancelled_up_to: moved.cancelled_up_to,
            marks: self.marks(source).map_or(0, |m| m.bits()),
            mask: moved.mask,
            budget,
            period,
            pending,
            class,
            cores: held.cores,
            remaining: held.remaining.min(budget as i64),
            deadline: held.deadline,
            throttled: held.throttled,
            consumed: held.consumed,
            counted: held.counted,
        };
        let id = self.new_request();
        self.requests.insert(
            id,
            Request::Move {
                caller: Some(caller),
                source: Some(source),
                context,
            },
        );
        let body = Body::Move {
            id,
            thread: node_remote.grant,
            sched: sched_remote.grant,
            state: Box::new(state),
        };
        self.send_packet(node, body);
        // The caller waits for the other node; so does the thread, whose
        // state is in transit (model choice: nothing changes it meanwhile).
        live(&mut self.threads, caller).wait = Wait::Node(id);
        live(&mut self.threads, source).wait = Wait::Node(id);
        self.observe();
        Ok(Output::Waits)
    }

    /// A capability argument judged by the caller's kernel: slot (a remote
    /// one that is invalid is `gone`), kind and `manage`.
    fn remote_object(
        &mut self,
        caller: ObjId,
        slot: SlotAddr,
        kind: ObjectKind,
    ) -> Result<ObjId, Error> {
        self.object(caller, slot, kind, Rights::MANAGE)
    }

    /// The destination node: the checks on its objects, in order, then the
    /// admission; the state goes into the thread and the context.
    pub(crate) fn receive_move(
        &mut self,
        from: u16,
        id: u64,
        (thread, sched): (u16, u16),
        state: Moved,
    ) {
        let outcome = self.place_move(thread, sched, state);
        self.send_packet(from, Body::Moved { id, outcome });
    }

    fn place_move(&mut self, thread: u16, sched: u16, state: Moved) -> Outcome {
        let anchored = |model: &Self, grant| model.anchor(grant).map(|cap| model.caps[&cap].object);
        let (Some(target), Some(context)) = (anchored(self, thread), anchored(self, sched)) else {
            return Err(Error::Gone);
        };
        let lent = self.lends.values().any(|(_, receiver)| *receiver == target);
        let dest = &self.threads[&target];
        if !dest.suspended
            || dest.wait != Wait::None
            || dest.home.is_some()
            || lent
            || dest.mask != state.mask
        {
            return Err(Error::Busy);
        }
        let kind = self.machine.own_kind(self.node);
        if !FeatureMask::from_raw(state.mask).fits(&kind) {
            return Err(Error::Busy);
        }
        let own = u64::from(self.node);
        let (window, mask) = state.cores;
        if window != own / 64 || mask & 1 << (own % 64) == 0 {
            return Err(Error::Busy);
        }
        let held = &self.scheds[&context];
        if held.configured || held.home.is_some() {
            return Err(Error::Busy);
        }
        let (budget, period) = state.pending.unwrap_or((state.budget, state.period));
        let sum = self.admitted + bandwidth(budget, period);
        if sum > ADMISSION_WHOLE_CORE {
            return Err(Error::Budget);
        }
        self.admitted = sum;
        self.event.resets.push(context);
        let placed = live(&mut self.scheds, context);
        placed.configured = true;
        (placed.budget, placed.period, placed.pending) =
            (state.budget, state.period, state.pending);
        (placed.class, placed.cores) = (state.class, state.cores);
        (placed.remaining, placed.deadline, placed.throttled) =
            (state.remaining, state.deadline, state.throttled);
        (placed.consumed, placed.counted) = (state.consumed, state.counted);
        placed.home = Some(target);
        let dest = live(&mut self.threads, target);
        dest.regs = state.regs;
        dest.cancelled_up_to = state.cancelled_up_to;
        (dest.home, dest.lost) = (Some(context), false);
        // No mark is lost: the thread's own word, and its task's marks as
        // `task_attach` adds them.
        dest.marks |= state.marks;
        if let Some(task) = dest.task {
            live(&mut self.tasks, task).marks |= state.marks;
        }
        self.event.moved.push((target, state.marks));
        self.observe();
        Ok(Output::Done)
    }

    /// The answer of the destination node: on success the thread gives up
    /// its context, which is unbound and not configured, and its registers
    /// are those of a new thread; the caller gets the outcome.
    pub(crate) fn moved(&mut self, id: u64, outcome: Outcome) {
        let Some(Request::Move {
            caller,
            source,
            context,
        }) = self.requests.remove(&id)
        else {
            return;
        };
        if let Some(source) = source.filter(|s| {
            self.threads
                .get(s)
                .is_some_and(|t| t.wait == Wait::Node(id))
        }) {
            live(&mut self.threads, source).wait = Wait::None;
            if outcome.is_ok() {
                let fresh = live(&mut self.threads, source);
                fresh.regs = vec![0; dodos_abi::x86_64::REGS_WORDS];
                fresh.regs[dodos_abi::x86_64::REG_RFLAGS] = dodos_abi::x86_64::RFLAGS_FIXED;
                if fresh.home == Some(context) {
                    fresh.home = None;
                }
            }
        }
        if outcome.is_ok() && self.scheds.contains_key(&context) {
            let before = self.scheds[&context].bandwidth();
            self.admitted -= before;
            self.event.resets.push(context);
            let gone = live(&mut self.scheds, context);
            if gone.home == source {
                gone.home = None;
            }
            gone.configured = false;
            (gone.remaining, gone.deadline, gone.throttled) = (0, 0, None);
            (gone.budget, gone.period, gone.pending) = (0, 0, None);
        }
        if let Some(caller) = caller.filter(|c| {
            self.threads
                .get(c)
                .is_some_and(|t| t.wait == Wait::Node(id))
        }) {
            live(&mut self.threads, caller).wait = Wait::None;
            live(&mut self.threads, caller).answered = true;
            self.wake(caller, outcome);
        }
        self.observe();
    }
}
