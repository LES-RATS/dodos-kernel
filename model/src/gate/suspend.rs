//! EXPERIMENTAL, not in the conformance set: the draft of whole-machine
//! suspend and resume (Gate words; "Whole-machine suspend and resume
//! (draft, not normative until D-153)"), executed so that its questions
//! can be answered from what the model shows. `node_stop` mode 3 is
//! `unsupported` in the normative text; nothing here is reached from the
//! gate model's normative operations.
//!
//! A small machine: nodes of cores, IOMMU units each held by a node, the
//! unit request records open between them, the suspend word, and the
//! cores' states. Every interleaving of the actors' steps is enumerated
//! (depth-first, with the states seen), under each variant of the open
//! rules; a node may hang at any step. Two findings are recorded per
//! variant: an isolation break (a core of a node runs while a unit that
//! has not failed is not yet enabled again) and a wedge (a reachable state
//! in which nobody can move, the word neither 0 nor *asleep*).

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Word {
    Zero,
    Suspending,
    Asleep,
    Resuming,
}

/// The member word of a node, as far as the draft reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Member {
    Live,
    /// Accused (by its own evidence, the one accusation the draft keeps
    /// while the word is not 0): fencing.
    Fencing,
}

/// The node of a core that belongs to no node (x86-64: a class-0 core left
/// out of every node, parked since the start, Q10).
pub(crate) const FREE: usize = usize::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CoreState {
    Running,
    Parked,
    Hung,
    /// Started again at the resume, not yet through its entry stub.
    Starting,
}

/// The variants of the draft's open rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Variant {
    /// Q3: a holder parks only once no open record names a unit it holds,
    /// and every node only once its own records are answered (the draft's
    /// text); `false`: a core parks at once.
    pub(crate) serve_before_park: bool,
    /// Q4: who may abandon at the park bound: only *S*'s gate, or any core
    /// still running.
    pub(crate) abandon_by_any_running: bool,
    /// Q4: a watchdog: the first core of the lowest other live node stays
    /// awake until *S*'s last call, which it then makes for *S* if *S* is
    /// hung (its gate writes *resuming*).
    pub(crate) watchdog: bool,
    /// Q5: the firmware may refuse or return from the sleep call.
    pub(crate) firmware_may_return: bool,
    /// Fixed point 1 removed: the resume starts the cores before it enables
    /// the units (D-151's text).
    pub(crate) start_before_units: bool,
    /// Q4: a parked core is woken at the park bound (an alarm armed by its
    /// park), and may then abandon.
    pub(crate) alarm_wakes_parked: bool,
    /// Q7: the entry stub returns a started core to its node only if the
    /// member word reads live with the generation its start request names;
    /// `false`: on the resume bit of the request alone.
    pub(crate) stub_checks_member: bool,
    /// Q10: the resume path starts a core of no node as it starts a node's.
    pub(crate) start_free_cores: bool,
    /// A node other than *S* may accuse itself at any step while the word
    /// is not 0 (evidence self).
    pub(crate) self_accusation: bool,
    /// Q4: only the watchdog core (the first core of the lowest node after
    /// *S*) parks keeping a wake, an alarm at `SUSPEND_PARK_NS` (ARM64
    /// `CPU_SUSPEND`, RISC-V `sbi_hart_suspend`); every other core is off
    /// (`CPU_OFF`, `sbi_hart_stop`), with no alarm.
    pub(crate) alarm_on_watchdog_only: bool,
    /// Q5 (x86-64): the PM1 write may seem not to sleep (the core reaches
    /// the instruction after it, past a bound), the core goes to
    /// *resuming*, and the platform may still enter the sleep later, during
    /// the resume.
    pub(crate) pm1_late_sleep: bool,
    /// Q5: the waking vector accepts the word *resuming* as well as
    /// *asleep* (the machine slept after the PM1 path gave up).
    pub(crate) wake_accepts_resuming: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct State {
    pub(crate) word: Word,
    /// Per core: its node and state.
    pub(crate) cores: Vec<(usize, CoreState)>,
    /// Per unit: its holder, whether it failed, whether enabled.
    pub(crate) units: Vec<(usize, bool, bool)>,
    /// Open records: (requester node, unit).
    pub(crate) open: BTreeSet<(usize, usize)>,
    /// The park bound passed (time, the one input the model abstracts).
    pub(crate) bound: bool,
    /// The core firmware resumes on.
    pub(crate) resume_core: Option<usize>,
    /// Per node: its member word and generation.
    pub(crate) members: Vec<(Member, u32)>,
    /// Per core: the start request the resume path wrote (node,
    /// generation, the resume bit implied).
    pub(crate) requests: Vec<Option<(usize, u32)>>,
    /// Q5: a PM1 write that did not seem to sleep, until the word is 0.
    pub(crate) pm1_pending: bool,
    /// Q5: the machine asleep after a late sleep, the word *resuming*: no
    /// actor runs until a wake.
    pub(crate) late_asleep: bool,
}

pub(crate) const S: usize = 0;

impl State {
    /// `nodes` nodes of `cores` cores; `units` units, unit *u* held by node
    /// *u* mod *nodes*; `open` records.
    pub(crate) fn new(nodes: usize, cores: usize, units: usize, open: &[(usize, usize)]) -> Self {
        Self {
            word: Word::Zero,
            cores: (0..nodes * cores)
                .map(|c| (c / cores, CoreState::Running))
                .collect(),
            units: (0..units).map(|u| (u % nodes, false, true)).collect(),
            open: open.iter().copied().collect(),
            bound: false,
            resume_core: None,
            members: vec![(Member::Live, 1); nodes],
            requests: vec![None; nodes * cores],
            pm1_pending: false,
            late_asleep: false,
        }
    }

    /// The same machine with one more core, of no node, parked (Q10).
    pub(crate) fn with_free_core(mut self) -> Self {
        self.cores.push((FREE, CoreState::Parked));
        self.requests.push(None);
        self
    }

    pub(crate) fn live(&self, n: usize) -> bool {
        n != FREE && self.members[n].0 == Member::Live
    }

    fn running(&self, n: usize) -> bool {
        self.cores
            .iter()
            .any(|&(m, s)| m == n && s == CoreState::Running)
    }

    fn node_hung(&self, n: usize) -> bool {
        self.cores
            .iter()
            .any(|&(m, s)| m == n && s == CoreState::Hung)
    }

    fn nodes(&self) -> usize {
        self.members.len()
    }

    /// The first core of the watchdog node: the lowest live node after *S*.
    pub(super) fn watchdog_core(&self) -> Option<usize> {
        let node = (1..self.nodes()).find(|&n| self.live(n))?;
        self.cores.iter().position(|&(n, _)| n == node)
    }
}

/// One step of an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// *S*'s gate, `node_stop` mode 3: 0 → *suspending*; every *owned*
    /// core's park request raised (fixed point 2).
    Mode3,
    /// A holder answers an open record of a unit it holds (or a failed
    /// unit's records answered *failed* at once).
    Serve { requester: usize, unit: usize },
    /// A core parks.
    Park { core: usize },
    /// A core hangs (the input).
    Hang { core: usize },
    /// The park bound passes (time).
    Bound,
    /// *suspending* → *resuming*, by `core`'s gate.
    Abandon { core: usize },
    /// *S*'s last core: *suspending* → *asleep*, every other owned core
    /// confirmed.
    Sleep { core: usize },
    /// The firmware returns from the sleep call: → *resuming*.
    FirmwareReturns,
    /// The wake: *asleep* → *resuming* on `core`, translation off.
    Wake { core: usize },
    /// The resume path enables unit `unit` (fixed point 1).
    Enable { unit: usize },
    /// The resume path starts core `core`.
    Start { core: usize },
    /// A started core passes its entry stub.
    Enter { core: usize },
    /// *resuming* → 0: every started core entered (fixed point 5).
    End,
    /// A parked core woken by its alarm at the park bound.
    Alarm { core: usize },
    /// A node accuses itself (evidence self): fencing, its running cores
    /// stopped.
    SelfAccuse { node: usize },
    /// Q5: the platform enters the sleep after the PM1 path gave up: every
    /// core and unit loses its state, the word unchanged (*resuming*).
    LateSleep,
}

impl State {
    /// The steps that can be taken now under `v`.
    pub(crate) fn steps(&self, v: Variant) -> Vec<Step> {
        let mut out = Vec::new();
        let s_runs = self.running(S);
        match self.word {
            Word::Zero => {
                // Every member word dead or live (a fencing node makes mode
                // 3 `busy`).
                if s_runs && self.members.iter().all(|m| m.0 == Member::Live) {
                    out.push(Step::Mode3);
                }
            }
            Word::Suspending => {
                for &(r, u) in &self.open {
                    let (holder, failed, _) = self.units[u];
                    if failed || self.running(holder) {
                        out.push(Step::Serve {
                            requester: r,
                            unit: u,
                        });
                    }
                }
                for (core, &(n, state)) in self.cores.iter().enumerate() {
                    if state != CoreState::Running {
                        continue;
                    }
                    let holds_open = self.open.iter().any(|&(_, u)| self.units[u].0 == n);
                    let own_open = self.open.iter().any(|&(r, _)| r == n);
                    let may_park = !v.serve_before_park || !(holds_open || own_open);
                    let last_of_s =
                        n == S
                            && self.cores.iter().enumerate().all(|(c, &(m, st))| {
                                c == core || m != S || st == CoreState::Parked
                            });
                    let watchdog = v.watchdog && Some(core) == self.watchdog_core();
                    if may_park && !last_of_s && !watchdog {
                        out.push(Step::Park { core });
                    }
                    if self.bound && (n == S || v.abandon_by_any_running) {
                        out.push(Step::Abandon { core });
                    }
                    let others_parked = self
                        .cores
                        .iter()
                        .enumerate()
                        .all(|(c, &(_, st))| c == core || st == CoreState::Parked);
                    if last_of_s && others_parked && !holds_open && !own_open {
                        out.push(Step::Sleep { core });
                    }
                    // The watchdog parks last, just before S's last call;
                    // at the bound with S hung it abandons for S.
                    if watchdog {
                        let rest_parked = self.cores.iter().enumerate().all(|(c, &(m, st))| {
                            c == core
                                || st == CoreState::Parked
                                || (m == S && st == CoreState::Running)
                        });
                        if rest_parked && may_park && s_runs {
                            out.push(Step::Park { core });
                        }
                        if self.bound && self.node_hung(S) {
                            out.push(Step::Abandon { core });
                        }
                    }
                }
                if !self.bound {
                    out.push(Step::Bound);
                }
                if self.bound && (v.alarm_wakes_parked || v.alarm_on_watchdog_only) {
                    let watchdog = self.watchdog_core();
                    for (core, &(n, state)) in self.cores.iter().enumerate() {
                        let keeps = !v.alarm_on_watchdog_only || Some(core) == watchdog;
                        if state == CoreState::Parked && n != FREE && keeps {
                            out.push(Step::Alarm { core });
                        }
                    }
                }
            }
            Word::Asleep => {
                if v.firmware_may_return && self.resume_core.is_none() {
                    out.push(Step::FirmwareReturns);
                }
                for core in 0..self.cores.len() {
                    out.push(Step::Wake { core });
                }
            }
            Word::Resuming => {
                // Q5: the platform slept late, after the PM1 path gave up;
                // then nothing runs until a wake, which the draft's waking
                // vector accepts only from *asleep*.
                if self.late_asleep {
                    if v.wake_accepts_resuming {
                        for core in 0..self.cores.len() {
                            out.push(Step::Wake { core });
                        }
                    }
                    return out;
                }
                if v.pm1_late_sleep
                    && self.pm1_pending
                    && self.cores.iter().any(|&(_, st)| st == CoreState::Running)
                {
                    out.push(Step::LateSleep);
                }
                let units_on = self.units.iter().all(|&(_, failed, on)| failed || on);
                for (u, &(_, failed, on)) in self.units.iter().enumerate() {
                    if !failed && !on {
                        out.push(Step::Enable { unit: u });
                    }
                }
                for (core, &(n, state)) in self.cores.iter().enumerate() {
                    let startable = self.live(n) || n == FREE && v.start_free_cores;
                    if state == CoreState::Parked
                        && (units_on || v.start_before_units)
                        && startable
                        && self.requests[core].is_none()
                    {
                        out.push(Step::Start { core });
                    }
                }
                // Back: every core of a live node entered (a node accused
                // meanwhile is not waited for: fixed point 5).
                let back = self.cores.iter().all(|&(n, st)| {
                    !self.live(n) || st != CoreState::Parked && st != CoreState::Starting
                });
                if back {
                    out.push(Step::End);
                }
            }
        }
        // A started core runs its entry stub whatever the word reads.
        for (core, &(_, state)) in self.cores.iter().enumerate() {
            if state == CoreState::Starting {
                out.push(Step::Enter { core });
            }
        }
        if v.self_accusation && self.word != Word::Zero {
            for (node, member) in self.members.iter().enumerate().skip(1) {
                if member.0 == Member::Live {
                    out.push(Step::SelfAccuse { node });
                }
            }
        }
        // A core may hang at any step (one hang per run: the inputs of a
        // fault).
        if !self.cores.iter().any(|&(_, st)| st == CoreState::Hung) {
            for (core, &(_, state)) in self.cores.iter().enumerate() {
                if state == CoreState::Running {
                    out.push(Step::Hang { core });
                }
            }
        }
        out
    }

    pub(crate) fn take(&mut self, step: Step, v: Variant) {
        match step {
            Step::Mode3 => self.word = Word::Suspending,
            Step::Serve { requester, unit } => {
                self.open.remove(&(requester, unit));
            }
            Step::Park { core } => self.cores[core].1 = CoreState::Parked,
            Step::Hang { core } => self.cores[core].1 = CoreState::Hung,
            Step::Bound => self.bound = true,
            Step::Abandon { .. } => {
                self.word = Word::Resuming;
                self.resume_core = None;
            }
            Step::FirmwareReturns => {
                self.word = Word::Resuming;
                self.resume_core = None;
                // The firmware's return, or a PM1 write past its bound.
                self.pm1_pending = true;
            }
            Step::LateSleep => {
                for core in self.cores.iter_mut() {
                    if matches!(core.1, CoreState::Running | CoreState::Starting) {
                        core.1 = CoreState::Parked;
                    }
                }
                for unit in &mut self.units {
                    unit.2 = false;
                }
                self.requests.iter_mut().for_each(|r| *r = None);
                self.pm1_pending = false;
                self.late_asleep = true;
            }
            Step::Sleep { core } => {
                self.cores[core].1 = CoreState::Parked;
                self.word = Word::Asleep;
                // Across the sleep the units lose their state.
                for unit in &mut self.units {
                    unit.2 = false;
                }
            }
            Step::Wake { core } => {
                self.word = Word::Resuming;
                self.late_asleep = false;
                self.resume_core = Some(core);
            }
            Step::Enable { unit } => self.units[unit].2 = true,
            Step::Start { core } => {
                let node = self.cores[core].0;
                self.cores[core].1 = CoreState::Starting;
                let generation = self.members.get(node).map_or(0, |m| m.1);
                self.requests[core] = Some((node, generation));
            }
            Step::Enter { core } => {
                // The entry stub: back to its node only on a member word
                // that reads live with the generation the request names;
                // otherwise it parks again.
                let node = self.cores[core].0;
                let returns = match (node, self.requests[core]) {
                    (FREE, _) => true,
                    (_, Some((n, g))) => {
                        !v.stub_checks_member || self.members[n] == (Member::Live, g)
                    }
                    (_, None) => false,
                };
                self.cores[core].1 = match returns {
                    true => CoreState::Running,
                    false => CoreState::Parked,
                };
            }
            Step::Alarm { core } => self.cores[core].1 = CoreState::Running,
            Step::SelfAccuse { node } => {
                self.members[node].0 = Member::Fencing;
                for core in self.cores.iter_mut() {
                    if core.0 == node && core.1 == CoreState::Running {
                        core.1 = CoreState::Parked;
                    }
                }
            }
            Step::End => {
                self.word = Word::Zero;
                self.bound = false;
                self.resume_core = None;
                self.pm1_pending = false;
                self.requests.iter_mut().for_each(|r| *r = None);
            }
        }
    }

    /// Isolation (fixed point 1): no core of a node runs while a unit that
    /// has not failed is disabled.
    pub(crate) fn isolated(&self) -> bool {
        let units_off = self.units.iter().any(|&(_, failed, on)| !failed && !on);
        !(units_off && self.cores.iter().any(|&(_, st)| st == CoreState::Running))
    }
}

/// What an exploration of a variant found.
#[derive(Debug, Default)]
pub(crate) struct Findings {
    pub(crate) states: usize,
    pub(crate) isolation: Option<Vec<Step>>,
    pub(crate) wedge: Option<(Vec<Step>, State)>,
    /// A wedge reached with no hang (a healthy machine wedged).
    pub(crate) healthy_wedge: Option<Vec<Step>>,
    /// Healthy *suspending* states (no hang) from which no healthy path
    /// reaches the sleep: a suspend that can only be abandoned.
    pub(crate) healthy_no_sleep: usize,
    pub(crate) slept: bool,
    /// The first of those states, and the path to it.
    pub(crate) no_sleep_example: Option<(Vec<Step>, State)>,
    /// Q6: a core runs while the word reads *asleep*.
    pub(crate) asleep_runner: Option<Vec<Step>>,
    /// Q10: a core of no node runs.
    pub(crate) free_runs: Option<Vec<Step>>,
    /// Q10: a resume on a core of no node reached the end (0).
    pub(crate) ended_after_free_wake: bool,
    /// Q7: a started core returned into a node that is not live.
    pub(crate) entered_not_live: Option<Vec<Step>>,
}

/// Whether a path without a hang and without the bound reaches the sleep.
fn can_sleep(start: &State, v: Variant) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start.clone()];
    while let Some(state) = stack.pop() {
        if state.word == Word::Asleep {
            return true;
        }
        if !seen.insert(state.clone()) || state.word != Word::Suspending {
            continue;
        }
        for step in state.steps(v) {
            if matches!(step, Step::Hang { .. } | Step::Bound | Step::Abandon { .. }) {
                continue;
            }
            let mut next = state.clone();
            next.take(step, v);
            stack.push(next);
        }
    }
    false
}

/// Every interleaving from `start` under `v`.
pub(crate) fn explore(start: &State, v: Variant) -> Findings {
    let mut seen = BTreeSet::new();
    let mut findings = Findings::default();
    let mut stack = vec![(start.clone(), Vec::new())];
    while let Some((state, path)) = stack.pop() {
        if !seen.insert(state.clone()) {
            continue;
        }
        findings.states += 1;
        if !state.isolated() && findings.isolation.is_none() {
            findings.isolation = Some(path.clone());
        }
        findings.slept |= state.word == Word::Asleep;
        // Q6: while the word reads *asleep* no core runs, so nothing but
        // the firmware's resume vector can reach the resume path.
        let runs = |st: CoreState| matches!(st, CoreState::Running | CoreState::Hung);
        if state.word == Word::Asleep
            && state.cores.iter().any(|&(_, st)| runs(st))
            && findings.asleep_runner.is_none()
        {
            findings.asleep_runner = Some(path.clone());
        }
        // Q10: a core of no node never runs a node's code.
        if state
            .cores
            .iter()
            .any(|&(n, st)| n == FREE && st == CoreState::Running)
            && findings.free_runs.is_none()
        {
            findings.free_runs = Some(path.clone());
        }
        // Q7: no started core returns into a node that is not live.
        if state
            .cores
            .iter()
            .any(|&(n, st)| n != FREE && st == CoreState::Running && !state.live(n))
            && findings.entered_not_live.is_none()
        {
            findings.entered_not_live = Some(path.clone());
        }
        let healthy = !state.cores.iter().any(|&(_, st)| st == CoreState::Hung);
        if healthy && state.word == Word::Suspending && !can_sleep(&state, v) {
            findings.healthy_no_sleep += 1;
            findings
                .no_sleep_example
                .get_or_insert_with(|| (path.clone(), state.clone()));
        }
        let steps = state.steps(v);
        let moving: Vec<Step> = steps
            .iter()
            .copied()
            .filter(|s| !matches!(s, Step::Hang { .. }))
            .collect();
        let stuck = moving.is_empty() && !matches!(state.word, Word::Zero | Word::Asleep);
        if stuck {
            let hung = state.cores.iter().any(|&(_, st)| st == CoreState::Hung);
            if !hung && findings.healthy_wedge.is_none() {
                findings.healthy_wedge = Some(path.clone());
            }
            if findings.wedge.is_none() {
                findings.wedge = Some((path.clone(), state.clone()));
            }
        }
        for step in steps {
            let mut next = state.clone();
            next.take(step, v);
            let mut p = path.clone();
            p.push(step);
            if step == Step::End && state.resume_core.is_some_and(|c| state.cores[c].0 == FREE) {
                findings.ended_after_free_wake = true;
            }
            stack.push((next, p));
        }
    }
    findings
}
