//! Unlike cores: feature masks and the size of a thread,
//! `thread_set_features`, what a core enables, and instructions retired on
//! demand. (`thread_move` was withdrawn by D-128.)

use dodos_abi::ObjectKind::{Frame, Notify, Sched, Thread};
use dodos_abi::x86_64::{AMX, AVX, AVX512};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const UT: SlotAddr = SlotAddr::own(25);
const SCHED: SlotAddr = SlotAddr::own(START_SLOT_SCHED);

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
        node: core(0),
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
            node: core(0),
            core: 0,
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
            node: core(0),
            core: 0,
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
    let (list, kind) = (machine.features(), machine.kind_of(0));
    assert_eq!(thread_bytes(FeatureMask::NONE, list, &kind), Some(2048));
    assert_eq!(
        thread_bytes(FeatureMask::from_raw(1 << AVX), list, &kind),
        Some(2048)
    );
    let amx = FeatureMask::from_raw(1 << AMX);
    assert_eq!(thread_bytes(amx, list, &kind), Some(16384));
    // The untyped gives a thread of mask AMX 16 KB, one of mask 0 2 KB, each
    // aligned on its size or on a page if larger (`untyped_retype`): AMX at
    // 0, mask 0 at 16 KB, AMX at 20 KB, ending at 36 KB (the final review's
    // F2 Opus 8: the identities compared always rose, and the comment
    // placed the third at 32 KB).
    let mut k = K::new();
    let watermark = |k: &K| k.m.untyped_marks(k.t, UT).unwrap().0;
    k.ok(retype(Thread, 1 << AMX, 100));
    let first = watermark(&k) - (16 << 10);
    k.ok(retype(Thread, 0, 101));
    assert_eq!(watermark(&k), first + (16 << 10) + 2048);
    k.ok(retype(Thread, 1 << AMX, 102));
    assert_eq!(watermark(&k), first + (36 << 10));
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
        node: core(1),
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

/// `sched_count` turning counting on for a context already counted keeps
/// the class its stop recorded, and its count (the final review's F2 Opus
/// 9: on a machine with no classifier the class was 0 either way).
#[test]
fn counting_turned_on_again_keeps_the_class_and_the_count() {
    let mut machine = Machine::example();
    machine.classifier = Some(3);
    let mut k = K::on(&machine);
    let a = k.spawn(100, 0);
    k.ok(count(s(102), 1));
    k.ok(Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    assert_eq!(k.m.running(), Some(a));
    assert_eq!(
        k.m.apply(a, Syscall::ThreadResume(s(START_SLOT_THREAD))),
        DONE
    );
    assert_eq!(k.m.apply(a, Syscall::ThreadSuspend(s(100))), DONE);
    assert_eq!(k.m.check(), Ok(()));
    let (_, state, count_before) = read(&mut k.m, k.t, s(102));
    assert_ne!(state & SCHED_STATE_HW_CLASS_MASK, 0);
    k.ok(count(s(102), SCHED_COUNT_ON));
    let (_, again, count_after) = read(&mut k.m, k.t, s(102));
    assert_eq!(
        again & SCHED_STATE_HW_CLASS_MASK,
        state & SCHED_STATE_HW_CLASS_MASK
    );
    assert_eq!(count_after, count_before);
}

/// Argon F1: on a machine where the other places are phantom nodes, the
/// only node that runs cannot stop itself (a phantom node could neither
/// drive its fence nor take a role), so the node keeps writing and a take
/// that follows works.
#[test]
fn the_only_running_node_cannot_stop_itself_beside_phantom_nodes() {
    for mode in [NODE_STOP_KILL, NODE_STOP_RETIRE, NODE_STOP_SEAL] {
        let machine = Machine::example();
        let (mut m, t) = Model::boot(&machine).unwrap();
        // The node holds a reserve role, which a kill and a retire ask of
        // their caller (D-150): the stops below are judged on the nodes
        // that would stay live.
        let claim = Syscall::RoleClaim(SlotAddr::own(START_SLOT_FIRST + 5));
        assert!(matches!(m.apply(t, claim), Ok(Output::Epoch(_))));
        let stop = Syscall::NodeStop {
            node: SlotAddr::own(START_SLOT_FIRST),
            mode,
            core: 0,
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
        let other = Syscall::NodeStop {
            node: SlotAddr::own(START_SLOT_FIRST + 1),
            mode: NODE_STOP_KILL,
            core: 0,
        };
        assert_eq!(m.apply(t, other), Ok(Output::Done));
    }
}

/// Argon F3: the order of the checks of thread_set_features and sched_count,
/// and sched_count on a counted context (mutants 20, 21, 24, 26; mutant 31,
/// `thread_move`, went with the call, D-128).
#[test]
fn unlike_cores_rules_that_no_test_pinned() {
    // #20: thread_set_features checks target capability before mask: a
    // capability without `write` is `denied` though the mask is invalid
    // too (an empty slot and a bad mask are both `invalid`, which shows no
    // order: the final review's F2 Opus 10).
    let mut k = K::new();
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
            node: s(START_SLOT_CSPACE),
            mode: SCHED_COUNT_ON,
        },
        Error::Denied,
    );

    // #26: sched_count(SCHED_COUNT_ON) on an already counted context keeps
    // its count (its class: the test below)
    k.ok(count(SCHED, SCHED_COUNT_ON));
    k.m.advance(k.m.now() + 100);
    assert_eq!(k.m.check(), Ok(()));
    let (_, _, before) = read(&mut k.m, k.t, SCHED);
    assert!(before >= 100);
    k.ok(count(SCHED, SCHED_COUNT_ON));
    let (_, _, after) = read(&mut k.m, k.t, SCHED);
    assert_eq!(after, before);
}
