//! Scenarios of the new shape of nodes (D-128, D-129, D-131), inside the
//! crate so that they can read what a node keeps: its cores, its rings, its
//! locks and its requests. Each test names the sentence of the contract it
//! is about. The harness drives a whole machine (`System`) or one node
//! alone (`Model`), and every step is followed by the whole check.

mod bounds;
mod checker;
mod combinations;
mod cores;
mod crossing;
mod explore;
mod membership;
mod mistral;
mod pass17;
mod pass18;
mod pass19;
mod pass20;
mod pass21;
mod pass22;
mod pass23;
mod pass24;
mod pass24_deepseek;
mod pass25;
mod pass26;
mod pass27;
mod pass28;
mod pass31;
mod pass32;
mod pass32_node;
mod pass33;
mod pass35;
mod pass36;
mod pass37;
mod pass38;
mod pass39;
mod pass40;
mod pass41;
mod pass42;
mod pass43;
mod reviews;
mod units;
mod verify13;

use crate::boot::{Core, Machine};
use crate::system::{Rng, System};
use crate::{Arch, Model, ObjId, Outcome, Output, Syscall};
use dodos_abi::{
    Error, MemberState, ObjectKind, PageSize, Rights, START_SLOT_FIRST, START_SLOT_SCHED, SlotAddr,
};

pub(crate) const DONE: Outcome = Ok(Output::Done);
pub(crate) const WAITS: Outcome = Ok(Output::Waits);
pub(crate) const MS: u64 = 1_000_000;

/// The most calls `ok` makes of one `untyped_retype` that returns `again`
/// (each clears a bounded part): a call still `again` after them fails the
/// test instead of hanging it (the audits of lot 8a, F14 and F09).
const RETYPE_STEPS: u32 = 40_000;

pub(crate) const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

/// The node capability of node `n` in a start-up space.
pub(crate) const fn node_cap(n: u16) -> SlotAddr {
    SlotAddr::own(START_SLOT_FIRST + n as u32)
}

/// A machine of `cores` cores of one kind on `arch`, laid out as `nodes`
/// (each the core numbers of one node, in position order); the first core
/// is the one firmware booted, of fence class 1, the others of class 2.
pub(crate) fn machine(arch: Arch, cores: usize, nodes: &[&[usize]]) -> Machine {
    let mut machine = Machine::example();
    machine.arch = arch;
    machine.cores = (0..cores)
        .map(|i| Core {
            hw_id: 2 * i as u64,
            kind: 0,
            fence: if i == 0 { 1 } else { 2 },
            booted: i == 0,
            package: 0,
        })
        .collect();
    let layout = nodes
        .iter()
        .map(|cores| {
            assert!(!cores.is_empty(), "a node must have at least one core");
            let window = cores[0] as u64 / 64;
            assert!(
                cores.iter().all(|&c| c as u64 / 64 == window),
                "a node's cores must lie in one 64-core window: {cores:?}"
            );
            let mask = cores.iter().fold(0, |m, &c| m | 1 << (c % 64));
            (window, mask)
        })
        .collect();
    machine.boot_nodes = Some((nodes.len() as u32, layout));
    // From 31 cores a crash area takes more than a chunk: the two areas
    // end where the example's end, apart by their length (Start-up state).
    let (chunk, len) = (machine.geometry.chunk(), machine.crash_len());
    if len > chunk {
        let end = machine.crash.0 + 2 * chunk;
        machine.crash = (end - 2 * len, Some(end - len));
    }
    machine
}

/// One node of `n` cores alone on the machine.
pub(crate) fn one_node(arch: Arch, n: usize) -> Machine {
    let cores: Vec<usize> = (0..n).collect();
    machine(arch, n, &[&cores])
}

/// The slot of the largest untyped of a start-up space.
pub(crate) fn untyped_slot(model: &Model, thread: ObjId) -> u32 {
    (START_SLOT_FIRST..START_SLOT_FIRST + 256)
        .filter_map(|i| {
            let object = model.object_at(thread, s(i))?;
            let untyped = model.untypeds.get(&object)?;
            let ordinary = model.objects[&object].kind == ObjectKind::Untyped && !untyped.table;
            ordinary.then_some((untyped.bytes, i))
        })
        .max()
        .map(|(_, i)| i)
        .expect("an untyped in the start-up space")
}

/// The slot of the first table untyped of a start-up space: the one cut
/// from the node's table floor (D-154).
pub(crate) fn table_slot(model: &Model, thread: ObjId) -> u32 {
    (START_SLOT_FIRST..START_SLOT_FIRST + 256)
        .find(|&i| model.untyped_is_table(thread, s(i)) == Some(true))
        .expect("a table untyped in the start-up space")
}

/// The size argument of a retype in the tests: for an `aspace` or an
/// `iospace` asked with 0, the slot of the node's table-floor untyped,
/// which the contract asks for there (D-154).
pub(crate) fn sized(model: &Model, thread: ObjId, kind: ObjectKind, size: u64) -> u64 {
    match kind {
        ObjectKind::ASpace | ObjectKind::IoSpace if size == 0 => s(table_slot(model, thread)).raw(),
        _ => size,
    }
}

/// The slots of a start-up space that hold objects of `kind`, in order.
pub(crate) fn slots_of(model: &Model, thread: ObjId, kind: ObjectKind) -> Vec<u32> {
    (START_SLOT_FIRST..START_SLOT_FIRST + 256)
        .filter(|&i| {
            model
                .object_at(thread, s(i))
                .is_some_and(|o| model.objects[&o].kind == kind)
        })
        .collect()
}

pub(crate) fn retype_from(untyped: u32, kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: s(untyped),
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

/// Whether a call is a `node_stop` of mode kill or retire, which the gate
/// accepts only from a node that holds a reserve role (D-150).
pub(super) fn stops_with_evidence(call: &Syscall) -> bool {
    let stop = |mode| mode == dodos_abi::NODE_STOP_KILL || mode == dodos_abi::NODE_STOP_RETIRE;
    matches!(call, Syscall::NodeStop { mode, .. } if stop(*mode))
}

/// The node of `model` claims a reserve role through `apply`, the first
/// one a claim succeeds on, unless it holds one already.
fn claim_a_reserve(model: &Model, thread: ObjId, mut apply: impl FnMut(Syscall) -> Outcome) {
    if model.table.holds_a_reserve(model.node) {
        return;
    }
    let slots = slots_of(model, thread, ObjectKind::Role);
    let reserves = model.table.role_records.iter().zip(slots);
    for (_, slot) in reserves.filter(|(r, _)| r.kind == dodos_abi::RoleKind::Reserve) {
        if matches!(apply(Syscall::RoleClaim(s(slot))), Ok(Output::Epoch(_))) {
            return;
        }
    }
}

/// A node alone: its model, its first thread, its largest untyped.
pub(crate) struct N {
    pub(crate) m: Model,
    pub(crate) t: ObjId,
    pub(crate) ut: u32,
    pub(crate) bounds: bounds::Bounds,
    pub(crate) _active: bounds::Active,
}

impl Drop for N {
    fn drop(&mut self) {
        self.bounds.sample_model(&self.m);
        bounds::record_scenario(&self.bounds);
    }
}

impl N {
    pub(crate) fn on(machine: &Machine) -> Self {
        let (mut m, t) = Model::boot(machine).expect("the machine boots");
        // The interrupts between the node's cores are taken when a test
        // says so, so that a state between two events can be seen.
        m.manual = true;
        let ut = untyped_slot(&m, t);
        let mut bounds = bounds::Bounds::default();
        bounds.sample_model(&m);
        let n = Self {
            m,
            t,
            ut,
            bounds,
            _active: bounds::Active::enter(),
        };
        assert_eq!(n.m.check(), Ok(()));
        n
    }

    #[track_caller]
    pub(crate) fn call(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        let meta = bounds::CallMeta::of(&call);
        let outcome = self.m.apply(thread, call);
        assert_eq!(self.m.check(), Ok(()));
        self.bounds.sample_call(&self.m, meta, &outcome);
        self.bounds.sample_model(&self.m);
        outcome
    }

    #[track_caller]
    pub(crate) fn ok(&mut self, call: Syscall) {
        let mut outcome = self.call(self.t, call.clone());
        let mut steps = 0;
        while matches!(call, Syscall::UntypedRetype { .. }) && outcome == Err(Error::Again) {
            steps += 1;
            assert!(steps < RETYPE_STEPS, "{call:?} never ends");
            outcome = self.call(self.t, call.clone());
        }
        assert_eq!(outcome, DONE, "{call:?}");
    }

    #[track_caller]
    pub(crate) fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.call(self.t, call), Err(error));
    }

    pub(crate) fn retype(&self, kind: ObjectKind, size: u64, destination: u32) -> Syscall {
        let size = sized(&self.m, self.t, kind, size);
        retype_from(self.ut, kind, size, destination)
    }

    /// The slot of the node's table-floor untyped (D-154).
    pub(crate) fn tables(&self) -> u32 {
        table_slot(&self.m, self.t)
    }

    /// Every core takes its pending interrupts, in core order, until none
    /// is pending.
    #[track_caller]
    pub(crate) fn interrupts(&mut self) {
        while let Some(&(core, _)) = self.m.pending_ipis().first() {
            self.m.take_interrupts(core);
            assert_eq!(self.m.check(), Ok(()));
            self.bounds.sample_model(&self.m);
        }
    }

    /// A new thread `slot` of the start-up space, bound to a new context
    /// `slot + 1` placed on core number `core` of the node, resumed; with a
    /// frame for its area at `slot + 2`. Returns the thread.
    #[track_caller]
    pub(crate) fn thread_on(&mut self, slot: u32, core: u64) -> ObjId {
        for call in thread_calls(self.ut, self.m.node, slot, core) {
            self.ok(call);
        }
        self.m.object_at(self.t, s(slot)).expect("the thread")
    }

    /// The first thread's own context, placed on core number `core`, takes
    /// half of it: room for another context.
    #[track_caller]
    pub(crate) fn place_first(&mut self, core: u64) {
        self.ok(place_first(self.m.node, core));
    }
}

/// The calls that make thread `slot` with its context at `slot + 1` on
/// core number `core` and its message page at `slot + 2`, and resume it.
pub(crate) fn thread_calls(ut: u32, node: u16, slot: u32, core: u64) -> Vec<Syscall> {
    vec![
        retype_from(ut, ObjectKind::Frame, 1, slot + 2),
        retype_from(ut, ObjectKind::Thread, 0, slot),
        retype_from(ut, ObjectKind::Sched, 0, slot + 1),
        Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: SlotAddr::own(dodos_abi::START_SLOT_CSPACE),
            aspace: s(dodos_abi::START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(slot + 2),
        },
        Syscall::SchedConfigure {
            sched: s(slot + 1),
            node: node_cap(node),
            core,
            budget: 200_000,
            period: 10_000_000,
            class: 1,
        },
        Syscall::SchedBind {
            sched: s(slot + 1),
            thread: s(slot),
        },
        Syscall::ThreadResume(s(slot)),
    ]
}

pub(crate) fn place_first(node: u16, core: u64) -> Syscall {
    Syscall::SchedConfigure {
        sched: s(START_SLOT_SCHED),
        node: node_cap(node),
        core,
        budget: 5_000_000,
        period: 10_000_000,
        class: 1,
    }
}

/// A whole machine: its system, the first thread of each node, and the
/// harm a test did to its nodes.
pub(crate) struct S {
    pub(crate) sys: System,
    pub(crate) t: Vec<ObjId>,
    pub(crate) harm: Harm,
    pub(crate) bounds: bounds::Bounds,
    scan: bounds::RingScan,
    _active: bounds::Active,
}

impl Drop for S {
    fn drop(&mut self) {
        self.sample_bounds();
        bounds::record_scenario(&self.bounds);
    }
}

/// The nodes a test harmed, each with the generation harmed: killed or a
/// core retired by a call that succeeded, a core hung, a failure found;
/// and the nodes sealed. An exploration's liveness judges an accusation
/// by them (`Harm::accepts`).
#[derive(Default)]
pub(crate) struct Harm {
    pub(crate) harmed: std::collections::BTreeSet<(u16, u32)>,
    pub(crate) sealed: std::collections::BTreeSet<(u16, u32)>,
    /// Each start or resume that succeeded: the node and generation
    /// started, and its starter with its generation.
    pub(crate) starts: std::collections::BTreeMap<(u16, u32), (u16, u32)>,
}

impl Harm {
    /// Whether the accusation of `node` in `member` is one a harm explains:
    /// the node harmed in that generation, or sealed and accused with
    /// evidence seal.
    pub(crate) fn accepts(&self, node: u16, member: dodos_abi::Member) -> bool {
        let at = (node, member.generation());
        // A start or a resume whose starter was harmed or sealed may never
        // end: nobody wakes its cores again (a sealing node drives no
        // start), and it is accused stale or stuck at `START_NS` (D-137,
        // D-144).
        let orphan = self
            .starts
            .get(&at)
            .is_some_and(|starter| self.harmed.contains(starter) || self.sealed.contains(starter));
        self.harmed.contains(&at)
            || self.sealed.contains(&at) && member.evidence() == dodos_abi::Evidence::Seal
            || orphan
                && matches!(
                    member.evidence(),
                    dodos_abi::Evidence::Stale | dodos_abi::Evidence::Stuck
                )
    }
}

impl S {
    pub(crate) fn on(machine: &Machine) -> Self {
        let (sys, t) = System::boot(machine).expect("the machine boots");
        let mut s = Self {
            sys,
            t,
            harm: Harm::default(),
            bounds: bounds::Bounds::default(),
            scan: bounds::RingScan::default(),
            _active: bounds::Active::enter(),
        };
        assert_eq!(s.sys.check(), Ok(()));
        s.sample_bounds();
        s
    }

    pub(crate) fn sample_bounds(&mut self) {
        self.bounds.sample_system(&self.sys, &mut self.scan);
    }

    /// The reference machine: two nodes of one core each.
    pub(crate) fn two() -> Self {
        Self::on(&Machine::example())
    }

    #[track_caller]
    pub(crate) fn call(&mut self, node: u16, call: Syscall) -> Outcome {
        let thread = self.t[node as usize];
        self.call_by(node, thread, call)
    }

    #[track_caller]
    pub(crate) fn call_by(&mut self, node: u16, thread: ObjId, call: Syscall) -> Outcome {
        let meta = bounds::CallMeta::of(&call);
        // A stop of a node that succeeds is a harm (kill, retire) or a seal.
        let stop = match call {
            Syscall::NodeStop {
                node: target, mode, ..
            } => target
                .index()
                .checked_sub(START_SLOT_FIRST)
                .map(|n| (n as u16, mode)),
            _ => None,
        };
        let generation = stop.map(|(n, _)| self.sys.member(n).generation());
        let started = match call {
            Syscall::NodeStart { node: target, .. } => target
                .index()
                .checked_sub(START_SLOT_FIRST)
                .map(|n| n as u16),
            _ => None,
        };
        let starter = (node, self.sys.member(node).generation());
        let outcome = self.sys.apply(node, thread, call);
        assert_eq!(self.sys.check(), Ok(()));
        if self.sys.has(node) {
            self.bounds.sample_call(self.sys.node(node), meta, &outcome);
        }
        self.sample_bounds();
        if let (Some(target), DONE) = (started, outcome.clone()) {
            let started = (target, self.sys.member(target).generation());
            self.harm.starts.insert(started, starter);
        }
        if let (Some((target, mode)), Some(generation), DONE) = (stop, generation, outcome.clone())
        {
            match mode {
                dodos_abi::NODE_STOP_SEAL => self.harm.sealed.insert((target, generation)),
                _ => self.harm.harmed.insert((target, generation)),
            };
        }
        outcome
    }

    /// Node `node` holds a reserve role from here on, if one can be
    /// claimed by its thread `thread`: what a node that stops another with
    /// mode kill or retire must hold (`node_stop`, D-150), claimed as the
    /// supervisor that stops a node would. Each test says it before its
    /// stop: `call` claims nothing, so that it and `System::apply` behave
    /// alike (pass 34, R-6 of the review of pass 32).
    pub(crate) fn supervisor(&mut self, node: u16, thread: ObjId) {
        let model = self.sys.node(node).clone();
        claim_a_reserve(&model, thread, |call| self.sys.apply(node, thread, call));
        assert_eq!(self.sys.check(), Ok(()));
        self.sample_bounds();
    }

    /// A core hangs (`System::hang`), a harm of the node it runs.
    #[track_caller]
    pub(crate) fn hang(&mut self, core: usize, nmi: bool) {
        let word = dodos_abi::CoreWord::from_raw(self.sys.core_word(core));
        if let Some(word) = word.filter(|w| w.state() == dodos_abi::CoreState::Owned) {
            self.harm.harmed.insert((word.slot(), word.generation()));
        }
        self.sys.hang(core, nmi);
        assert_eq!(self.sys.check(), Ok(()));
        self.sample_bounds();
    }

    /// A node finds a failure in itself (`System::fail`), a harm.
    #[track_caller]
    pub(crate) fn fail(&mut self, node: u16) {
        let generation = self.sys.member(node).generation();
        self.harm.harmed.insert((node, generation));
        self.sys.fail(node, 0);
        assert_eq!(self.sys.check(), Ok(()));
        self.sample_bounds();
    }

    #[track_caller]
    pub(crate) fn ok(&mut self, node: u16, call: Syscall) {
        let mut outcome = self.call(node, call.clone());
        let mut steps = 0;
        while matches!(call, Syscall::UntypedRetype { .. }) && outcome == Err(Error::Again) {
            steps += 1;
            assert!(steps < RETYPE_STEPS, "{call:?} never ends");
            outcome = self.call(node, call.clone());
        }
        assert_eq!(outcome, DONE, "{call:?}");
    }

    #[track_caller]
    pub(crate) fn no(&mut self, node: u16, call: Syscall, error: Error) {
        assert_eq!(self.call(node, call), Err(error));
    }

    /// A thread of node `node` on its core number `core` (see
    /// `thread_calls`), after the first context made room on core 0.
    #[track_caller]
    pub(crate) fn thread_on(&mut self, node: u16, slot: u32, core: u64) -> ObjId {
        let ut = untyped_slot(self.sys.node(node), self.t[node as usize]);
        for call in thread_calls(ut, node, slot, core) {
            self.ok(node, call);
        }
        let first = self.t[node as usize];
        self.sys
            .node(node)
            .object_at(first, s(slot))
            .expect("the thread")
    }

    pub(crate) fn retype(&self, node: u16, kind: ObjectKind, size: u64, dest: u32) -> Syscall {
        let model = self.sys.node(node);
        let first = self.t[node as usize];
        let size = sized(model, first, kind, size);
        retype_from(untyped_slot(model, first), kind, size, dest)
    }

    #[track_caller]
    pub(crate) fn settle(&mut self) {
        self.sys.settle(100_000);
        assert_eq!(self.sys.check(), Ok(()));
        self.sample_bounds();
    }

    /// Time passes `ns`, checked every 100 µs.
    #[track_caller]
    pub(crate) fn run(&mut self, ns: u64) {
        self.run_by(ns, 100_000);
    }

    /// Time passes `ns`, checked every `step`.
    #[track_caller]
    pub(crate) fn run_by(&mut self, ns: u64, step: u64) {
        assert!(step > 0, "run_by step must be positive");
        let to = self.sys.now() + ns;
        while self.sys.now() < to && !self.sys.stopped {
            let at = (self.sys.now() + step).min(to);
            self.sys.advance(at);
            assert_eq!(self.sys.check(), Ok(()));
            self.sample_bounds();
        }
    }

    /// Events one at a time, the earliest first, each followed by the check,
    /// until `until` holds (at most 10,000 events or steps of 10 µs): for a
    /// state that lasts less than one step of `run`.
    #[track_caller]
    pub(crate) fn step_until(&mut self, until: impl Fn(&Self) -> bool) -> bool {
        for _ in 0..10_000 {
            if until(self) {
                return true;
            }
            match self.sys.events().first() {
                Some(&event) => self.sys.fire(event),
                None => {
                    let at = self.sys.now() + 10_000;
                    self.sys.advance(at);
                }
            }
            assert_eq!(self.sys.check(), Ok(()));
            self.sample_bounds();
        }
        until(self)
    }

    /// Events chosen by the seeded generator, `count` of them at most,
    /// each followed by the check.
    #[track_caller]
    pub(crate) fn shuffle(&mut self, rng: &mut Rng, count: usize) {
        for _ in 0..count {
            let events = self.sys.events();
            if events.is_empty() || self.sys.stopped {
                break;
            }
            let event = events[rng.below(events.len())];
            if std::env::var("DODOS_SEED").is_ok() {
                eprintln!("{} event {event:?}", self.sys.now());
            }
            self.sys.fire(event);
            assert_eq!(self.sys.check(), Ok(()), "after {event:?}");
            self.sample_bounds();
        }
    }

    pub(crate) fn state(&self, node: u16) -> MemberState {
        self.sys.member(node).state()
    }

    pub(crate) fn woken(&self, node: u16) -> Option<&Outcome> {
        self.sys.node(node).woken(self.t[node as usize])
    }

    /// After a node starts again: its new first thread.
    pub(crate) fn restarted(&mut self, node: u16) {
        self.t[node as usize] = self
            .sys
            .node(node)
            .object_at_start(dodos_abi::START_SLOT_THREAD);
    }
}

/// Prints the bounds accumulated across all scenario tests when `DODOS_BOUNDS`
/// is set.
#[test]
fn zzz_scenario_bounds_summary() {
    if std::env::var("DODOS_BOUNDS").is_ok() {
        bounds::wait_for_active();
        bounds::scenario_snapshot().print("scenario tests (lib)");
    }
}
