//! Unlike cores: feature masks and the size of a thread,
//! `thread_set_features`, what a core enables, instructions retired on
//! demand, and `thread_move` between the two nodes of `Machine::example`.

use dodos_abi::ObjectKind::{Frame, Notify, Reply, Sched, Task, Thread};
use dodos_abi::x86_64::{AMX, AVX, AVX512, REG_RAX, REG_RFLAGS, RFLAGS_FIXED};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const UT: SlotAddr = SlotAddr::own(25);
const MAILBOX: SlotAddr = SlotAddr::own(START_SLOT_MAILBOX);
const SCHED: SlotAddr = SlotAddr::own(START_SLOT_SCHED);
const NOT_EXEC: Rights = Rights::ALL.without(Rights::EXEC);

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

fn retype(kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

fn features(thread: u32, mask: u64) -> Syscall {
    Syscall::ThreadSetFeatures {
        thread: s(thread),
        mask,
    }
}

fn count(sched: SlotAddr, mode: u64) -> Syscall {
    Syscall::SchedCount {
        sched,
        core: core(0),
        mode,
    }
}

/// `sched_read`: consumed, state, instructions.
fn read(model: &mut Model, thread: ObjId, sched: SlotAddr) -> (u64, u64, u64) {
    match model.apply(thread, Syscall::SchedRead(sched)) {
        Ok(Output::SchedRead {
            consumed,
            state,
            instructions,
            ..
        }) => (consumed, state, instructions),
        other => panic!("{other:?}"),
    }
}

/// One node, its first thread, and every invariant after every call.
struct K {
    m: Model,
    t: ObjId,
}

impl K {
    fn new() -> Self {
        Self::on(&Machine::example())
    }

    fn on(machine: &Machine) -> Self {
        let (m, t) = Model::boot(machine).unwrap();
        Self { m, t }
    }

    #[track_caller]
    fn go(&mut self, call: Syscall) -> Outcome {
        let outcome = self.m.apply(self.t, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        assert_eq!(self.go(call), DONE);
    }

    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    /// A thread of mask `mask` in `slot`, in the spaces of the first
    /// thread, with a page, a context of its own of 1 ms in 10, resumed.
    fn spawn(&mut self, slot: u32, mask: u64) -> ObjId {
        self.ok(Syscall::SchedConfigure {
            sched: SCHED,
            core: core(0),
            budget: 5_000_000,
            period: 10_000_000,
            class: 1,
        });
        self.ok(retype(Thread, mask, slot));
        self.ok(retype(Frame, 1, slot + 1));
        self.ok(Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(slot + 1),
        });
        self.ok(retype(Sched, 0, slot + 2));
        self.ok(Syscall::SchedConfigure {
            sched: s(slot + 2),
            core: core(0),
            budget: 1_000_000,
            period: 10_000_000,
            class: 1,
        });
        self.ok(Syscall::SchedBind {
            sched: s(slot + 2),
            thread: s(slot),
        });
        self.ok(Syscall::ThreadResume(s(slot)));
        self.m.object_at(self.t, s(slot)).unwrap()
    }
}

// The feature mask of a thread.

#[test]
fn thread_mask_is_judged_invalid_then_unsupported() {
    let mut k = K::new();
    // Invalid: a bit 48-63, a bit that names no listed state feature, a
    // length without a feature that has one (x86-64 has none).
    for mask in [1 << 48, 1 << 63, 1 << 7, 1 << 30, 16 << 32] {
        k.no(retype(Thread, mask, 100), Error::Invalid);
    }
    // Invalid comes first: a length with bit 31.
    k.no(retype(Thread, 1 << 31 | 16 << 32, 100), Error::Invalid);
    // Unsupported: bit 31, a feature the node's kind does not have.
    for mask in [
        1 << FEATURE_USER_INTERRUPTS,
        1 << AVX512,
        1 << AVX512 | 1 << AMX,
    ] {
        k.no(retype(Thread, mask, 100), Error::Unsupported);
    }
    // A baseline feature is accepted and has no effect; the large optional
    // feature is accepted.
    k.ok(retype(Thread, 1 << AVX, 100));
    k.ok(retype(Thread, 1 << AMX, 101));
}

#[test]
fn thread_size_is_the_power_of_two_above_its_area() {
    // The kind of `Machine::example`: 1 KB + 832 bytes of baseline, and
    // 8 KB for AMX.
    let machine = Machine::example();
    let (list, kind) = (machine.features(), machine.own_kind(0));
    assert_eq!(thread_bytes(FeatureMask::NONE, list, &kind), Some(2048));
    assert_eq!(
        thread_bytes(FeatureMask::from_raw(1 << AVX), list, &kind),
        Some(2048)
    );
    let amx = FeatureMask::from_raw(1 << AMX);
    assert_eq!(thread_bytes(amx, list, &kind), Some(16384));
    // The untyped gives a thread of mask AMX 16 KB: two of them and one of
    // mask 0 leave the next 2 KB object at 32 KB + 2 KB.
    let mut k = K::new();
    k.ok(retype(Thread, 1 << AMX, 100));
    k.ok(retype(Thread, 0, 101));
    k.ok(retype(Thread, 1 << AMX, 102));
    let at = |k: &K, slot: u32| k.m.object_at(k.t, s(slot)).unwrap();
    assert!(at(&k, 100) < at(&k, 101) && at(&k, 101) < at(&k, 102));
}

#[test]
fn thread_set_features_needs_manage_and_judges_the_mask_first() {
    let mut k = K::new();
    k.ok(retype(Thread, 1 << AMX, 100));
    k.ok(Syscall::CapDerive {
        source: s(100),
        destination: s(101),
        rights: Rights::READ.union(Rights::WRITE).bits(),
        badge: 0,
    });
    k.no(features(101, 0), Error::Denied);
    k.no(features(100, 1 << 48), Error::Invalid);
    k.no(features(100, 1 << AVX512), Error::Unsupported);
    k.no(features(102, 0), Error::Invalid);
    k.ok(features(100, 0));
    k.ok(features(100, 1 << AMX | 1 << AVX));
}

#[test]
fn thread_set_features_is_busy_unless_suspended_and_not_waiting() {
    let mut k = K::new();
    let a = k.spawn(100, 1 << AMX);
    // Not suspended.
    k.no(features(100, 0), Error::Busy);
    // Suspended while it waits on a notify: still busy.
    k.ok(retype(Notify, 0, 110));
    assert_eq!(k.m.apply(a, Syscall::Wait(s(110), 0)), WAITS);
    k.ok(Syscall::ThreadSuspend(s(100)));
    k.no(features(100, 0), Error::Busy);
    k.ok(Syscall::Signal(s(110)));
    k.ok(features(100, 0));
}

#[test]
fn thread_set_features_never_needs_more_than_the_area() {
    let mut k = K::new();
    k.ok(retype(Thread, 0, 100));
    // The state checks come before: the mask needs more than 2 KB.
    k.no(features(100, 1 << AMX), Error::NoMemory);
    k.ok(features(100, 1 << AVX));
    k.ok(retype(Thread, 1 << AMX, 101));
    k.ok(features(101, 0));
    k.ok(features(101, 1 << AMX));
}

#[test]
fn a_core_enables_the_baseline_and_the_optional_features_of_the_running_thread() {
    let mut k = K::new();
    let a = k.spawn(100, 1 << AMX);
    let b = k.spawn(110, 0);
    let baseline = 1 << AVX;
    // Each thread runs in turn: the first gives up its core.
    k.ok(Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    let mut seen = Vec::new();
    for _ in 0..4 {
        let running = k.m.running().unwrap();
        seen.push((running, k.m.enabled()));
        assert_eq!(k.m.apply(running, Syscall::Yield), DONE);
        assert_eq!(k.m.check(), Ok(()));
    }
    assert!(seen.contains(&(a, baseline | 1 << AMX)));
    assert!(seen.contains(&(b, baseline)));
}

// Instructions retired, on demand.

#[test]
fn sched_count_needs_manage_a_core_of_its_node_and_a_mode() {
    let mut k = K::new();
    k.ok(retype(Sched, 0, 100));
    k.ok(Syscall::CapDerive {
        source: s(100),
        destination: s(101),
        rights: Rights::READ.union(Rights::WRITE).bits(),
        badge: 0,
    });
    k.no(count(s(101), 1), Error::Denied);
    let other = Syscall::SchedCount {
        sched: s(100),
        core: core(1),
        mode: 1,
    };
    k.no(other, Error::Invalid);
    k.no(count(s(100), 2), Error::Invalid);
    // Turning it off when it is off, and on twice, change nothing.
    k.ok(count(s(100), 0));
    k.ok(count(s(100), 1));
    k.ok(count(s(100), 1));
    let (_, state, instructions) = read(&mut k.m, k.t, s(100));
    assert_eq!(state & SCHED_STATE_COUNTED, SCHED_STATE_COUNTED);
    assert_eq!(instructions, 0);
}

#[test]
fn a_counted_context_counts_while_it_runs_and_off_resets_the_count() {
    let mut k = K::new();
    k.ok(count(SCHED, 1));
    let (consumed, _, before) = read(&mut k.m, k.t, SCHED);
    k.m.advance(k.m.now() + 300_000);
    assert_eq!(k.m.check(), Ok(()));
    let (later, state, after) = read(&mut k.m, k.t, SCHED);
    // One instruction per nanosecond on the model's cores, charged with the
    // time.
    assert_eq!(after - before, later - consumed);
    assert!(after > before);
    assert_eq!(state & SCHED_STATE_HW_CLASS_MASK, 0);
    k.ok(count(SCHED, 0));
    let (_, state, instructions) = read(&mut k.m, k.t, SCHED);
    assert_eq!((state & SCHED_STATE_COUNTED, instructions), (0, 0));
}

#[test]
fn an_uncounted_context_reads_zero_instructions() {
    let mut k = K::new();
    k.m.advance(k.m.now() + 300_000);
    let (consumed, _, instructions) = read(&mut k.m, k.t, SCHED);
    assert!(consumed > 0);
    assert_eq!(instructions, 0);
}

#[test]
fn without_a_counter_the_count_reads_zero() {
    let mut machine = Machine::example();
    machine.counter = false;
    let mut k = K::on(&machine);
    k.ok(count(SCHED, 1));
    k.m.advance(k.m.now() + 300_000);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(read(&mut k.m, k.t, SCHED).2, 0);
}

#[test]
fn the_class_is_read_when_a_counted_context_stops_running() {
    let mut machine = Machine::example();
    machine.classifier = Some(3);
    let mut k = K::on(&machine);
    let a = k.spawn(100, 0);
    k.ok(count(s(102), 1));
    // Thread a runs and stops: its context records 1 + the class.
    k.ok(Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    assert_eq!(k.m.running(), Some(a));
    assert_eq!(
        k.m.apply(a, Syscall::ThreadResume(s(START_SLOT_THREAD))),
        DONE
    );
    assert_eq!(k.m.apply(a, Syscall::ThreadSuspend(s(100))), DONE);
    assert_eq!(k.m.check(), Ok(()));
    let (_, state, _) = read(&mut k.m, k.t, s(102));
    assert_eq!(
        state & SCHED_STATE_HW_CLASS_MASK,
        4 << SCHED_STATE_HW_CLASS_SHIFT
    );
    // Turning counting off clears the class.
    k.ok(count(s(102), 0));
    assert_eq!(read(&mut k.m, k.t, s(102)).1 & SCHED_STATE_HW_CLASS_MASK, 0);
}

// thread_move between the two nodes.

/// The two nodes; node 0's thread in `A` with its own context in `A + 2`,
/// suspended; node 1's thread and sched made for it, granted to node 0 in
/// `D` and `D + 1`.
struct S {
    sys: System,
    t: Vec<ObjId>,
}

const A: u32 = 100;
const D: u32 = 120;

impl S {
    fn new() -> Self {
        Self::with(0)
    }

    fn with(mask: u64) -> Self {
        let (sys, t) = System::boot(&Machine::example()).unwrap();
        let mut m = Self { sys, t };
        for node in 0..2 {
            m.ok(node, retype(Reply, 0, 99));
            m.ok(
                node,
                Syscall::SchedConfigure {
                    sched: SCHED,
                    core: core(node.into()),
                    budget: 5_000_000,
                    period: 10_000_000,
                    class: 1,
                },
            );
        }
        // Node 0: the thread, its page and its context, which may run on
        // both cores.
        m.ok(0, retype(Thread, mask, A));
        m.ok(0, retype(Frame, 1, A + 1));
        m.ok(
            0,
            Syscall::ThreadConfigure {
                thread: s(A),
                cspace: s(START_SLOT_CSPACE),
                aspace: s(START_SLOT_ASPACE),
                fault: SlotAddr::NONE,
                page: s(A + 1),
            },
        );
        m.ok(0, retype(Sched, 0, A + 2));
        m.ok(
            0,
            Syscall::SchedConfigure {
                sched: s(A + 2),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 2,
            },
        );
        m.ok(
            0,
            Syscall::SchedSetCores {
                sched: s(A + 2),
                core: core(0),
                window: 0,
                mask: 0b11,
            },
        );
        m.ok(
            0,
            Syscall::SchedBind {
                sched: s(A + 2),
                thread: s(A),
            },
        );
        let mut block = vec![0; x86_64::REGS_WORDS];
        block[REG_RFLAGS] = RFLAGS_FIXED;
        block[REG_RAX] = 42;
        m.ok(
            0,
            Syscall::ThreadWriteRegs {
                thread: s(A),
                block,
            },
        );
        // Node 1: the destination thread and sched, granted to node 0.
        m.ok(1, retype(Thread, mask, D));
        m.ok(1, retype(Sched, 0, D + 1));
        assert_eq!(m.on(0, recv(D, 2)), WAITS);
        let send = Syscall::Send {
            target: core(0),
            message: Message {
                label: 1,
                flags: 0,
                words: vec![1],
                caps: vec![
                    Transfer::Derive(s(D), NOT_EXEC.bits()),
                    Transfer::Derive(s(D + 1), NOT_EXEC.bits()),
                ],
                marks: 0,
            },
        };
        assert_eq!(m.on(1, send), WAITS);
        m.settle();
        m
    }

    #[track_caller]
    fn on(&mut self, node: u16, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        assert_eq!(self.on(node, call), DONE);
    }

    #[track_caller]
    fn settle(&mut self) {
        for _ in 0..32 {
            if self.sys.pending().is_empty() {
                return;
            }
            self.sys.deliver(0);
            assert_eq!(self.sys.check(), Ok(()));
        }
        panic!("messages without end");
    }

    /// `thread_move` of `A` into the granted `D`, with these parameters.
    fn moving(&mut self, parameters: (u64, u64, u64)) -> Outcome {
        let call = Syscall::ThreadMove {
            thread: s(A),
            destination: s(D),
            sched: s(D + 1),
            core: core(1),
            budget: parameters.0,
            period: parameters.1,
            class: parameters.2,
        };
        let outcome = self.on(0, call);
        if outcome != WAITS {
            return outcome;
        }
        self.settle();
        self.sys
            .node(0)
            .woken(self.t[0])
            .cloned()
            .expect("answered")
    }

    /// The destination thread bound to node 1's spaces and a page, so that
    /// it can be resumed.
    fn bind_destination(&mut self) {
        self.ok(1, retype(Frame, 1, D + 3));
        self.ok(
            1,
            Syscall::ThreadConfigure {
                thread: s(D),
                cspace: s(START_SLOT_CSPACE),
                aspace: s(START_SLOT_ASPACE),
                fault: SlotAddr::NONE,
                page: s(D + 3),
            },
        );
    }

    /// A second thread on node 0, in slot 140, that runs.
    fn helper(&mut self) -> ObjId {
        self.ok(0, retype(Thread, 0, 140));
        self.ok(0, retype(Frame, 1, 141));
        self.ok(
            0,
            Syscall::ThreadConfigure {
                thread: s(140),
                cspace: s(START_SLOT_CSPACE),
                aspace: s(START_SLOT_ASPACE),
                fault: SlotAddr::NONE,
                page: s(141),
            },
        );
        self.ok(0, retype(Sched, 0, 142));
        self.ok(
            0,
            Syscall::SchedConfigure {
                sched: s(142),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            },
        );
        self.ok(
            0,
            Syscall::SchedBind {
                sched: s(142),
                thread: s(140),
            },
        );
        self.ok(0, Syscall::ThreadResume(s(140)));
        self.sys.node(0).object_at(self.t[0], s(140)).unwrap()
    }

    fn read(&mut self, node: u16, sched: u32) -> (u64, u64) {
        let thread = self.t[node as usize];
        let (consumed, state, _) = {
            let mut model = self.sys.node(node).clone();
            read(&mut model, thread, s(sched))
        };
        (consumed, state)
    }
}

fn recv(first: u32, count: u32) -> Syscall {
    Syscall::Recv {
        endpoint: MAILBOX,
        reply: s(99),
        range: Range {
            first: s(first),
            count,
        },
        wait: 0,
    }
}

#[test]
fn thread_move_hands_the_state_and_the_context_to_the_destination() {
    let mut m = S::new();
    assert_eq!(m.moving((0, 0, 0)), DONE);
    // The destination thread has the registers; its new context is
    // configured with the parameters and class of the old one, bound to it.
    let dest = m.sys.node(1).object_at(m.t[1], s(D)).unwrap();
    let mut node1 = m.sys.node(1).clone();
    let Ok(Output::Regs(block)) = node1.apply(m.t[1], Syscall::ThreadReadRegs(s(D))) else {
        panic!("registers");
    };
    assert_eq!(block[REG_RAX], 42);
    let (_, state) = m.read(1, D + 1);
    assert_eq!(
        state & (SCHED_STATE_CONFIGURED | SCHED_STATE_HOME),
        SCHED_STATE_CONFIGURED | SCHED_STATE_HOME
    );
    // It stays suspended; the source keeps its bindings and stays
    // suspended, with no context, and a block of a new thread.
    assert!(!m.sys.node(1).runnable().contains(&dest));
    let (_, state) = m.read(0, A + 2);
    assert_eq!(state & (SCHED_STATE_CONFIGURED | SCHED_STATE_HOME), 0);
    let mut node0 = m.sys.node(0).clone();
    let Ok(Output::Regs(block)) = node0.apply(m.t[0], Syscall::ThreadReadRegs(s(A))) else {
        panic!("registers");
    };
    assert_eq!(block[REG_RAX], 0);
    // Bound and resumed on node 1, the destination runs.
    m.bind_destination();
    m.ok(1, Syscall::ThreadResume(s(D)));
    assert!(m.sys.node(1).runnable().contains(&dest));
}

#[test]
fn thread_move_with_parameters_configures_the_destination_with_them() {
    let mut m = S::new();
    assert_eq!(m.moving((2_000_000, 8_000_000, 1)), DONE);
    let mut node1 = m.sys.node(1).clone();
    let Ok(Output::SchedRead { remaining, .. }) = node1.apply(m.t[1], Syscall::SchedRead(s(D + 1)))
    else {
        panic!("a context");
    };
    // r = min(r, B): the old context had at most 1 ms left.
    assert!(remaining <= 1_000_000);
}

#[test]
fn thread_move_carries_the_marks_into_the_destination() {
    let mut m = S::new();
    m.ok(0, retype(Task, 0, A + 3));
    m.ok(
        0,
        Syscall::TaskAttach {
            thread: s(A),
            task: s(A + 3),
        },
    );
    m.ok(
        0,
        Syscall::TaskMark {
            task: s(A + 3),
            marks: 1,
        },
    );
    assert_eq!(m.moving((0, 0, 0)), DONE);
    let dest = m.sys.node(1).object_at(m.t[1], s(D)).unwrap();
    assert_eq!(m.sys.node(1).marks(dest).unwrap().bits() & 1, 1);
}

#[test]
fn thread_move_checks_its_capabilities_in_order() {
    let mut m = S::new();
    let call = |destination, sched, core| Syscall::ThreadMove {
        thread: s(A),
        destination,
        sched,
        core,
        budget: 0,
        period: 0,
        class: 0,
    };
    // A destination thread of the own node: `invalid`, after its checks.
    assert_eq!(
        m.on(0, call(s(START_SLOT_THREAD), s(D + 1), core(1))),
        Err(Error::Invalid)
    );
    assert_eq!(
        m.on(0, call(s(D + 1), s(D + 1), core(1))),
        Err(Error::Invalid)
    );
    // A sched of another node than the destination thread's.
    assert_eq!(m.on(0, call(s(D), s(A + 2), core(1))), Err(Error::Invalid));
    // A core that does not name the destination node.
    assert_eq!(m.on(0, call(s(D), s(D + 1), core(0))), Err(Error::Invalid));
    // Plain arguments: not all 0 and not those of `sched_configure`.
    for parameters in [
        (1, 0, 0),
        (1_000_000, 500_000, 0),
        (1_000_000, 2_000_000, 3),
    ] {
        assert_eq!(m.moving(parameters), Err(Error::Invalid));
    }
    assert!(m.sys.pending().is_empty());
}

#[test]
fn thread_move_is_busy_unless_the_source_is_suspended_with_a_movable_context() {
    let mut m = S::new();
    m.ok(0, Syscall::ThreadResume(s(A)));
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    m.ok(0, Syscall::ThreadSuspend(s(A)));
    // Its context not configured.
    let off = Syscall::SchedConfigure {
        sched: s(A + 2),
        core: core(0),
        budget: 0,
        period: 0,
        class: 0,
    };
    m.ok(0, off);
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    // No own context.
    m.ok(0, Syscall::SchedUnbind(s(A + 2)));
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    assert!(m.sys.pending().is_empty());
}

#[test]
fn thread_move_is_busy_when_the_destination_is_not_ready_for_it() {
    // The destination thread resumed.
    let mut m = S::new();
    m.bind_destination();
    m.ok(1, Syscall::ThreadResume(s(D)));
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    // The destination thread with a context of its own.
    let mut m = S::new();
    m.ok(1, retype(Sched, 0, D + 2));
    m.ok(
        1,
        Syscall::SchedBind {
            sched: s(D + 2),
            thread: s(D),
        },
    );
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    // The destination sched configured, or with a home.
    let mut m = S::new();
    m.ok(
        1,
        Syscall::SchedConfigure {
            sched: s(D + 1),
            core: core(1),
            budget: 1_000_000,
            period: 10_000_000,
            class: 1,
        },
    );
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    let mut m = S::new();
    m.ok(1, retype(Thread, 0, D + 2));
    m.ok(
        1,
        Syscall::SchedBind {
            sched: s(D + 1),
            thread: s(D + 2),
        },
    );
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
}

#[test]
fn thread_move_is_busy_for_another_mask_or_a_core_outside_the_cores_set() {
    // Both threads made with AMX; the destination's mask set back to 0.
    let mut m = S::with(1 << AMX);
    m.ok(1, features(D, 0));
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
    m.ok(1, features(D, 1 << AMX));
    assert_eq!(m.moving((0, 0, 0)), DONE);
    // A context whose cores set holds only node 0's core.
    let mut m = S::new();
    m.ok(
        0,
        Syscall::SchedSetCores {
            sched: s(A + 2),
            core: core(0),
            window: 0,
            mask: 0b01,
        },
    );
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
}

#[test]
fn thread_move_fails_with_budget_when_the_destination_cannot_admit_it() {
    let mut m = S::new();
    // Node 1 keeps 5 ms in 10; the moved context asks for 6.
    assert_eq!(m.moving((6_000_000, 10_000_000, 1)), Err(Error::Budget));
    // Nothing moved: the source keeps its context.
    let (_, state) = m.read(0, A + 2);
    assert_eq!(state & SCHED_STATE_CONFIGURED, SCHED_STATE_CONFIGURED);
    assert_eq!(m.moving((0, 0, 0)), DONE);
}

#[test]
fn thread_cancel_does_not_end_the_wait_of_a_move() {
    let mut m = S::new();
    let call = Syscall::ThreadMove {
        thread: s(A),
        destination: s(D),
        sched: s(D + 1),
        core: core(1),
        budget: 0,
        period: 0,
        class: 0,
    };
    let b = m.helper();
    assert_eq!(m.on(0, call), WAITS);
    // The thread waits for the other node too; a cancel ends neither.
    for target in [A, START_SLOT_THREAD] {
        let cancel = Syscall::ThreadCancel(s(target), 0);
        assert_eq!(m.sys.apply(0, b, cancel), DONE);
        assert_eq!(m.sys.check(), Ok(()));
    }
    let before = m.sys.pending().len();
    assert_eq!(before, 1);
    m.settle();
    assert_eq!(m.sys.node(0).woken(m.t[0]), Some(&DONE));
}

#[test]
fn the_thread_of_a_move_in_flight_is_destroyed_only_after_the_answer() {
    let mut m = S::new();
    let b = m.helper();
    let call = Syscall::ThreadMove {
        thread: s(A),
        destination: s(D),
        sched: s(D + 1),
        core: core(1),
        budget: 0,
        period: 0,
        class: 0,
    };
    assert_eq!(m.on(0, call), WAITS);
    // The thread is frozen until the answer: its removal changes nothing.
    for _ in 0..3 {
        assert_eq!(
            m.sys.apply(0, b, Syscall::CapDelete(s(A))),
            Err(Error::Again)
        );
        assert_eq!(m.sys.check(), Ok(()));
    }
    m.settle();
    assert_eq!(m.sys.node(0).woken(m.t[0]), Some(&DONE));
    let mut steps = 0;
    while m.sys.apply(0, b, Syscall::CapDelete(s(A))) != DONE {
        assert_eq!(m.sys.check(), Ok(()));
        steps += 1;
        assert!(steps < 16);
    }
    assert_eq!(m.sys.check(), Ok(()));
    // The context the thread left is no longer configured.
    let (_, state) = m.read(0, A + 2);
    assert_eq!(state & SCHED_STATE_CONFIGURED, 0);
}

#[test]
fn thread_move_refuses_a_sched_of_a_third_node() {
    let mut machine = Machine::example();
    let mut third = machine.cores[1];
    third.hw_id = 4;
    machine.cores.push(third);
    let (mut sys, t) = System::boot(&machine).unwrap();
    let on = |sys: &mut System, node: u16, call: Syscall| {
        let outcome = sys.apply(node, t[node as usize], call);
        assert_eq!(sys.check(), Ok(()));
        outcome
    };
    for node in 0..3 {
        assert_eq!(on(&mut sys, node, retype(Reply, 0, 99)), DONE);
    }
    assert_eq!(on(&mut sys, 0, retype(Thread, 0, A)), DONE);
    assert_eq!(on(&mut sys, 1, retype(Thread, 0, D)), DONE);
    assert_eq!(on(&mut sys, 2, retype(Sched, 0, D)), DONE);
    // Node 0 receives the thread of node 1, then the sched of node 2.
    for (from, slot) in [(1u16, D), (2, D + 1)] {
        assert_eq!(on(&mut sys, 0, recv(slot, 1)), WAITS);
        let send = Syscall::Send {
            target: core(0),
            message: Message {
                label: 1,
                flags: 0,
                words: vec![1],
                caps: vec![Transfer::Derive(s(D), NOT_EXEC.bits())],
                marks: 0,
            },
        };
        assert_eq!(on(&mut sys, from, send), WAITS);
        sys.settle(32);
        assert_eq!(sys.check(), Ok(()));
    }
    let call = Syscall::ThreadMove {
        thread: s(A),
        destination: s(D),
        sched: s(D + 1),
        core: core(1),
        budget: 0,
        period: 0,
        class: 0,
    };
    assert_eq!(on(&mut sys, 0, call), Err(Error::Invalid));
}

/// Argon F1: on a machine where the other places are phantom nodes, the
/// only node that runs cannot stop itself (a phantom node could neither
/// drive its fence nor take a role), so the node keeps writing and a take
/// that follows works.
#[test]
fn the_only_running_node_cannot_stop_itself_beside_phantom_nodes() {
    for mode in [CORE_STOP_KILL, CORE_STOP_RETIRE, CORE_STOP_SEAL] {
        let machine = Machine::example();
        let (mut m, t) = Model::boot(&machine).unwrap();
        let stop = Syscall::CoreStop {
            core: SlotAddr::own(START_SLOT_FIRST),
            mode,
        };
        assert_eq!(m.apply(t, stop), Err(Error::Busy));
        assert_eq!(m.member(0).state(), MemberState::Live);
        let shift = m.geometry().chunk_shift();
        let free = (0..1u64 << (machine.phys_bits - shift))
            .find(|&c| m.ownership(c) == Ownership::free(Owner::Unclaimed).raw())
            .unwrap();
        let take = Syscall::MemoryTake {
            control: SlotAddr::own(START_SLOT_MEMORY_CONTROL),
            kind: 0,
            place: free << shift,
            size: u64::from(shift),
            destination: SlotAddr::own(120),
        };
        assert_eq!(m.apply(t, take), Ok(Output::Done));
        assert_eq!(m.check(), Ok(()));
        // A phantom node can still be stopped by the node that runs.
        let other = Syscall::CoreStop {
            core: SlotAddr::own(START_SLOT_FIRST + 1),
            mode: CORE_STOP_KILL,
        };
        assert_eq!(m.apply(t, other), Ok(Output::Done));
    }
}

/// Argon F3: the order of the checks of thread_set_features and sched_count,
/// sched_count on a counted context, and thread_move with a context still
/// lent (mutants 20, 21, 24, 26, 31).
#[test]
fn unlike_cores_rules_that_no_test_pinned() {
    // #20: thread_set_features checks target capability before mask
    let mut k = K::new();
    k.no(features(999, 1u64 << 63), Error::Invalid);
    k.ok(Syscall::CapDerive {
        source: s(START_SLOT_THREAD),
        destination: s(200),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(features(200, 1u64 << 63), Error::Denied);

    // #21: thread_set_features checks Busy (running/waiting) before NoMemory (needed > area)
    k.no(features(START_SLOT_THREAD, 1 << AMX), Error::Busy);

    // #24: sched_count checks sched capability before core capability
    k.ok(Syscall::CapDerive {
        source: SCHED,
        destination: s(201),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(
        Syscall::SchedCount {
            sched: s(201),
            core: s(START_SLOT_CSPACE),
            mode: SCHED_COUNT_ON,
        },
        Error::Denied,
    );

    // #26: sched_count(SCHED_COUNT_ON) on an already counted context preserves count and hw_class
    k.ok(count(SCHED, SCHED_COUNT_ON));
    k.m.advance(k.m.now() + 100);
    assert_eq!(k.m.check(), Ok(()));
    let (_, _, before) = read(&mut k.m, k.t, SCHED);
    assert!(before >= 100);
    k.ok(count(SCHED, SCHED_COUNT_ON));
    let (_, _, after) = read(&mut k.m, k.t, SCHED);
    assert_eq!(after, before);

    // #31: thread_move rejects a suspended source thread whose context is still lent out
    let mut m = S::new();
    m.ok(0, retype(ObjectKind::Endpoint, 0, 150));
    m.ok(0, retype(Thread, 0, 151));
    m.ok(0, retype(Frame, 1, 152));
    m.ok(
        0,
        Syscall::ThreadConfigure {
            thread: s(151),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(152),
        },
    );
    m.ok(0, Syscall::ThreadResume(s(151)));
    let passive = m.sys.node(0).object_at(m.t[0], s(151)).unwrap();
    let recv_ep = Syscall::Recv {
        endpoint: s(150),
        reply: s(99),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.sys.apply(0, passive, recv_ep), WAITS);
    m.ok(0, Syscall::ThreadResume(s(A)));
    let src = m.sys.node(0).object_at(m.t[0], s(A)).unwrap();
    let call_ep = Syscall::Call {
        endpoint: s(150),
        message: Message::default(),
        range: Range::NONE,
        wait: 1,
    };
    assert_eq!(m.sys.apply(0, src, call_ep), WAITS);
    m.ok(0, Syscall::ThreadSuspend(s(A)));
    m.ok(0, Syscall::ThreadCancel(s(A), 1));
    assert_eq!(m.moving((0, 0, 0)), Err(Error::Busy));
}
