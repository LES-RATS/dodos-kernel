//! Scenarios of the new shape of nodes (D-128, D-129, D-131), inside the
//! crate so that they can read what a node keeps: its cores, its rings, its
//! locks and its requests. Each test names the sentence of the contract it
//! is about. The harness drives a whole machine (`System`) or one node
//! alone (`Model`), and every step is followed by the whole check.

mod checker;
mod cores;
mod crossing;
mod explore;
mod membership;
mod mistral;
mod pass17;
mod pass18;
mod pass19;
mod reviews;
mod units;

use crate::boot::{Core, Machine};
use crate::system::{Rng, System};
use crate::{Arch, Model, ObjId, Outcome, Output, Syscall};
use dodos_abi::{
    Error, MemberState, ObjectKind, PageSize, Rights, START_SLOT_FIRST, START_SLOT_SCHED, SlotAddr,
};

pub(crate) const DONE: Outcome = Ok(Output::Done);
pub(crate) const WAITS: Outcome = Ok(Output::Waits);
pub(crate) const MS: u64 = 1_000_000;

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
            let window = cores[0] as u64 / 64;
            let mask = cores.iter().fold(0, |m, &c| m | 1 << (c % 64));
            (window, mask)
        })
        .collect();
    machine.boot_nodes = Some((nodes.len() as u16, layout));
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
            (model.objects[&object].kind == ObjectKind::Untyped).then_some((untyped.bytes, i))
        })
        .max()
        .map(|(_, i)| i)
        .expect("an untyped in the start-up space")
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

/// A node alone: its model, its first thread, its largest untyped.
pub(crate) struct N {
    pub(crate) m: Model,
    pub(crate) t: ObjId,
    pub(crate) ut: u32,
}

impl N {
    pub(crate) fn on(machine: &Machine) -> Self {
        let (mut m, t) = Model::boot(machine).expect("the machine boots");
        // The interrupts between the node's cores are taken when a test
        // says so, so that a state between two events can be seen.
        m.manual = true;
        let ut = untyped_slot(&m, t);
        let n = Self { m, t, ut };
        assert_eq!(n.m.check(), Ok(()));
        n
    }

    #[track_caller]
    pub(crate) fn call(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.m.apply(thread, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    #[track_caller]
    pub(crate) fn ok(&mut self, call: Syscall) {
        let mut outcome = self.call(self.t, call.clone());
        while matches!(call, Syscall::UntypedRetype { .. }) && outcome == Err(Error::Again) {
            outcome = self.call(self.t, call.clone());
        }
        assert_eq!(outcome, DONE, "{call:?}");
    }

    #[track_caller]
    pub(crate) fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.call(self.t, call), Err(error));
    }

    pub(crate) fn retype(&self, kind: ObjectKind, size: u64, destination: u32) -> Syscall {
        retype_from(self.ut, kind, size, destination)
    }

    /// Every core takes its pending interrupts, in core order, until none
    /// is pending.
    #[track_caller]
    pub(crate) fn interrupts(&mut self) {
        while let Some(&(core, _)) = self.m.pending_ipis().first() {
            self.m.take_interrupts(core);
            assert_eq!(self.m.check(), Ok(()));
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

/// A whole machine: its system and the first thread of each node.
pub(crate) struct S {
    pub(crate) sys: System,
    pub(crate) t: Vec<ObjId>,
}

impl S {
    pub(crate) fn on(machine: &Machine) -> Self {
        let (sys, t) = System::boot(machine).expect("the machine boots");
        let s = Self { sys, t };
        assert_eq!(s.sys.check(), Ok(()));
        s
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
        let outcome = self.sys.apply(node, thread, call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    #[track_caller]
    pub(crate) fn ok(&mut self, node: u16, call: Syscall) {
        let mut outcome = self.call(node, call.clone());
        while matches!(call, Syscall::UntypedRetype { .. }) && outcome == Err(Error::Again) {
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
        retype_from(untyped_slot(model, self.t[node as usize]), kind, size, dest)
    }

    #[track_caller]
    pub(crate) fn settle(&mut self) {
        self.sys.settle(100_000);
        assert_eq!(self.sys.check(), Ok(()));
    }

    /// Time passes `ns`, checked every 100 µs.
    #[track_caller]
    pub(crate) fn run(&mut self, ns: u64) {
        self.run_by(ns, 100_000);
    }

    /// Time passes `ns`, checked every `step`.
    #[track_caller]
    pub(crate) fn run_by(&mut self, ns: u64, step: u64) {
        let to = self.sys.now() + ns;
        while self.sys.now() < to && !self.sys.stopped {
            let at = (self.sys.now() + step).min(to);
            self.sys.advance(at);
            assert_eq!(self.sys.check(), Ok(()));
        }
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
