//! Scheduling and time: one test per rule, on the start-up state of
//! `Machine::example`. Its first thread runs on the context in slot 4
//! (63/64 of the core every 10 ms, class 1, the recovery reserve holding
//! the other 1/64); the boot core's capability is in slot 16,
//! the other core's in slot 17.

mod common;

use dodos_abi::ObjectKind::{Endpoint, Frame, Notify, Reply, Sched, Thread, Timer};
use dodos_abi::*;
use dodos_model::*;

const UT: u32 = 20;
const CORE: u32 = 16;
const OTHER_CORE: u32 = 17;
const FIRST: u32 = START_SLOT_SCHED;
const MS: u64 = 1_000_000;
const US: u64 = 1_000;
const EP: u32 = 30;
const RP: u32 = 31;
const NT: u32 = 32;
const DONE: Outcome = Ok(Output::Done);
const ALL: u64 = SCHED_STATE_CONFIGURED | SCHED_STATE_HOME;

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(common::slot(index))
}

struct K {
    m: Model,
    t: ObjId,
}

impl K {
    fn new() -> Self {
        let (m, t) = Model::boot(&Machine::example()).unwrap();
        let mut k = Self { m, t };
        k.make(Endpoint, EP);
        k.make(Reply, RP);
        k.make(Notify, NT);
        k
    }

    /// The first thread's context lowered to `budget` every 10 ms.
    fn lowered(budget: u64) -> Self {
        let mut k = Self::new();
        k.ok(configure(FIRST, budget, 10 * MS, 1));
        k
    }

    fn on(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        let outcome = self.m.apply(thread, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    fn go(&mut self, call: Syscall) -> Outcome {
        self.on(self.t, call)
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        assert_eq!(self.go(call), DONE);
    }

    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    fn advance(&mut self, by: u64) {
        let to = self.m.now() + by;
        self.m.advance(to);
        assert_eq!(self.m.check(), Ok(()));
    }

    fn make(&mut self, kind: ObjectKind, slot: u32) {
        let call = Syscall::UntypedRetype {
            untyped: s(UT),
            kind,
            size: (kind == Frame) as u64,
            page: 0,
            memory: 0,
            destination: s(slot),
        };
        self.ok(call);
    }

    /// A context in `slot`, configured with `budget` every `period`.
    fn context(&mut self, slot: u32, budget: u64, period: u64, class: u64) {
        self.make(Sched, slot);
        self.ok(configure(slot, budget, period, class));
    }

    /// A running thread in `slot`, bound to the start-up spaces, with a
    /// message page in `slot + 1` and the context in `context` as its own
    /// if it is not 0; its fault endpoint is `EP`.
    fn thread(&mut self, slot: u32, context: u32) -> ObjId {
        self.make(Thread, slot);
        self.make(Frame, slot + 1);
        self.ok(Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: s(EP),
            page: s(slot + 1),
        });
        if context != 0 {
            self.ok(bind(context, slot));
        }
        self.ok(Syscall::ThreadResume(s(slot)));
        self.m.object_at(self.t, s(slot)).unwrap()
    }

    /// `sched_read`: consumed, remaining, deadline, state.
    fn read(&mut self, slot: u32) -> (u64, i64, u64, u64) {
        self.read_by(self.t, slot)
    }

    /// `sched_read` by another thread than the first.
    fn read_by(&mut self, thread: ObjId, slot: u32) -> (u64, i64, u64, u64) {
        match self.on(thread, Syscall::SchedRead(s(slot))) {
            Ok(Output::SchedRead {
                consumed,
                remaining,
                deadline,
                state,
                ..
            }) => (consumed, remaining, deadline, state),
            other => panic!("{other:?}"),
        }
    }

    fn state(&mut self, slot: u32) -> u64 {
        self.read(slot).3
    }

    fn can_run(&self, thread: ObjId) -> bool {
        self.m.runnable().contains(&thread)
    }
}

fn configure(slot: u32, budget: u64, period: u64, class: u64) -> Syscall {
    Syscall::SchedConfigure {
        sched: s(slot),
        node: s(CORE),
        core: 0,
        budget,
        period,
        class,
    }
}

fn bind(sched: u32, thread: u32) -> Syscall {
    Syscall::SchedBind {
        sched: s(sched),
        thread: s(thread),
    }
}

fn recv(endpoint: u32, reply: u32) -> Syscall {
    Syscall::Recv {
        endpoint: s(endpoint),
        reply: s(reply),
        range: Range::NONE,
        wait: 0,
    }
}

fn call(endpoint: u32) -> Syscall {
    Syscall::Call {
        endpoint: s(endpoint),
        message: Message::default(),
        range: Range::NONE,
        wait: 0,
    }
}

fn answer(reply: u32) -> Syscall {
    Syscall::Send {
        target: s(reply),
        message: Message::default(),
    }
}

fn timer_set(timer: u32, notify: u32, deadline: u64, tolerance: u64) -> Syscall {
    Syscall::TimerSet {
        timer: s(timer),
        notify: s(notify),
        deadline,
        tolerance,
    }
}

// Clock, and the start-up context.

#[test]
fn first_thread_runs_on_a_context_of_63_64_of_the_core() {
    let mut k = K::new();
    assert_eq!(k.m.running(), Some(k.t));
    let budget = START_SCHED_BUDGET_NS;
    assert_eq!(k.read(FIRST), (0, budget as i64, 10 * MS, ALL));
    // With the recovery reserve it takes the whole core: nothing else is
    // admitted.
    k.make(Sched, 40);
    k.no(configure(40, 10 * US, 10 * MS, 1), Error::Budget);
}

#[test]
fn time_is_charged_to_the_running_context_at_the_next_event() {
    let mut k = K::new();
    k.advance(3 * MS);
    // The clock moved; nothing was charged yet: the charge comes with the
    // next event, which `sched_read` is.
    let (consumed, remaining, ..) = k.read(FIRST);
    let left = START_SCHED_BUDGET_NS - 3 * MS;
    assert_eq!((consumed, remaining), (3 * MS, left as i64));
}

// The calls of the scheduling contexts.

#[test]
fn sched_configure_needs_manage_on_the_sched_and_write_on_the_core() {
    let mut k = K::lowered(5 * MS);
    k.make(Sched, 40);
    for (from, to, rights) in [(40, 41, Rights::WRITE), (CORE, 42, Rights::READ)] {
        k.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(to),
            rights: rights.bits(),
            badge: 0,
        });
    }
    let weak = |sched, core| Syscall::SchedConfigure {
        sched: s(sched),
        node: s(core),
        core: 0,
        budget: 10 * US,
        period: 10 * MS,
        class: 1,
    };
    k.no(weak(41, CORE), Error::Denied);
    k.no(weak(40, 42), Error::Denied);
    k.no(weak(40, EP), Error::Invalid);
    // Another core's capability, judged right after its three checks:
    // before arguments that are wrong too.
    let other = Syscall::SchedConfigure {
        sched: s(40),
        node: s(OTHER_CORE),
        core: 0,
        budget: 1,
        period: 0,
        class: 9,
    };
    k.no(other, Error::Invalid);
    k.ok(weak(40, CORE));
}

#[test]
fn budget_and_period_are_both_0_or_within_the_limits_and_class_is_0_to_2() {
    let mut k = K::lowered(5 * MS);
    k.make(Sched, 40);
    for (budget, period, class) in [
        (0, 10 * MS, 1),
        (10 * US, 0, 1),
        (10 * US - 1, 10 * MS, 1),
        (2 * MS, MS, 1),
        (10 * US, (1 << 36) + 1, 1),
        (10 * US, 10 * MS, 3),
    ] {
        k.no(configure(40, budget, period, class), Error::Invalid);
    }
    k.ok(configure(40, 10 * US, 1 << 36, 2));
    k.ok(configure(40, 0, 0, 0));
}

#[test]
fn admission_refuses_more_than_one_whole_core_and_changes_nothing() {
    let mut k = K::lowered(5 * MS);
    k.make(Sched, 40);
    k.make(Sched, 41);
    // Half of the core, 31/64 here, and the recovery reserve's 1/64.
    k.no(configure(40, 5 * MS, 10 * MS, 1), Error::Budget);
    k.ok(configure(40, 4_843_750, 10 * MS, 1));
    k.no(configure(41, 10 * US, 10 * MS, 1), Error::Budget);
    assert_eq!(k.state(41), 0);
    // Removing a configuration never fails, and frees its share at once.
    k.ok(configure(40, 0, 0, 1));
    k.ok(configure(41, 10 * US, 10 * MS, 1));
    // The bandwidth is rounded up: 31/64 of the core, 10,000 ns every
    // 20,001 ns and 858,950 ns every 2^35 ns sum to 0.064 of a unit (2^-32
    // of a core) below 63/64, so they fit exactly beside the reserve, and
    // not once each is rounded up (the final review's F2 Opus 33: three
    // thirds and the reserve did not fit whichever way it rounded).
    k.ok(configure(FIRST, 0, 0, 1));
    k.ok(configure(41, 0, 0, 1));
    k.ok(configure(40, 4_843_750, 10 * MS, 1));
    k.ok(configure(41, 10 * US, 20_001, 1));
    k.make(Sched, 42);
    k.no(configure(42, 858_950, 1 << 35, 1), Error::Budget);
}

#[test]
fn new_configuration_sets_r_to_b_and_d_to_now_plus_t() {
    let mut k = K::lowered(5 * MS);
    k.advance(MS);
    k.make(Sched, 40);
    k.ok(configure(40, 2 * MS, 8 * MS, 0));
    let (_, remaining, deadline, state) = k.read(40);
    assert_eq!((remaining, deadline), (2 * MS as i64, 9 * MS));
    assert_eq!(state, SCHED_STATE_CONFIGURED);
}

#[test]
fn reconfiguration_lowers_r_keeps_d_and_uses_b_and_t_from_the_next_replenishment() {
    let mut k = K::new();
    k.ok(configure(FIRST, 4 * MS, 10 * MS, 1));
    // r became min(63/64 of 10 ms, 4 ms); d stays 10 ms.
    assert_eq!(k.read(FIRST).1, 4 * MS as i64);
    assert_eq!(k.read(FIRST).2, 10 * MS);
    // Exhausted at 4 ms, replenished at 10 ms with the new budget.
    k.advance(4 * MS);
    assert_ne!(k.state(FIRST) & SCHED_STATE_THROTTLED, 0);
    k.advance(6 * MS);
    let (_, remaining, deadline, state) = k.read(FIRST);
    assert_eq!((remaining, deadline), (4 * MS as i64, 20 * MS));
    assert_eq!(state & SCHED_STATE_THROTTLED, 0);
}

#[test]
fn budget_0_makes_the_context_not_configured_and_its_holder_does_not_run() {
    let mut k = K::new();
    k.ok(configure(FIRST, 0, 0, 1));
    assert_eq!(k.read(FIRST), (0, 0, 0, SCHED_STATE_HOME));
    assert_eq!(k.m.running(), None);
}

#[test]
fn sched_bind_is_busy_for_a_context_with_a_home_or_a_thread_with_a_context() {
    let mut k = K::lowered(5 * MS);
    k.context(40, 10 * US, 10 * MS, 1);
    k.context(41, 10 * US, 10 * MS, 1);
    k.thread(50, 40);
    k.make(Thread, 52);
    k.no(bind(40, 52), Error::Busy);
    k.no(bind(41, 50), Error::Busy);
    // A suspended thread bound to no space can be bound.
    k.ok(bind(41, 52));
    assert_eq!(k.state(41), ALL);
}

#[test]
fn sched_bind_is_busy_for_a_thread_with_a_lent_context() {
    let mut k = K::lowered(5 * MS);
    let server = k.thread(50, 0);
    assert_eq!(k.on(server, recv(EP, RP)), Ok(Output::Waits));
    k.go(call(EP)).unwrap();
    assert_eq!(k.m.running(), Some(server));
    let make = Syscall::UntypedRetype {
        untyped: s(UT),
        kind: Sched,
        size: 0,
        page: 0,
        memory: 0,
        destination: s(40),
    };
    assert_eq!(k.on(server, make), DONE);
    assert_eq!(k.on(server, bind(40, 50)), Err(Error::Busy));
}

#[test]
fn sched_unbind_leaves_the_thread_without_a_context_and_unmarked() {
    let mut k = K::lowered(5 * MS);
    k.context(40, 10 * US, 10 * MS, 1);
    let thread = k.thread(50, 40);
    k.ok(Syscall::SchedUnbind(s(40)));
    k.ok(Syscall::SchedUnbind(s(40)));
    assert_eq!(k.state(40), SCHED_STATE_CONFIGURED);
    // It raises nothing: it stays runnable, and simply does not run.
    assert!(k.can_run(thread));
    assert_eq!(k.m.census().faulting, 0);
}

#[test]
fn sched_set_cores_takes_a_window_and_a_mask_that_hold_the_core_of_the_kernel() {
    let mut k = K::new();
    let set = |core, window, mask| Syscall::SchedSetCores {
        sched: s(FIRST),
        node: s(core),
        window,
        mask,
    };
    k.no(set(CORE, 1024, 1), Error::Invalid);
    k.no(set(CORE, 0, 0), Error::Invalid);
    k.no(set(CORE, 1, 1), Error::Invalid);
    k.no(set(CORE, 0, 2), Error::Invalid);
    k.no(set(OTHER_CORE, 0, 3), Error::Invalid);
    k.ok(set(CORE, 0, 3));
}

#[test]
fn sched_read_needs_read() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: s(FIRST),
        destination: s(40),
        rights: Rights::WRITE.bits(),
        badge: 0,
    });
    k.no(Syscall::SchedRead(s(40)), Error::Denied);
}

// What runs next.

#[test]
fn nearest_deadline_runs_and_an_equal_one_does_not_preempt() {
    let mut k = K::lowered(5 * MS);
    k.context(40, 10 * US, 5 * MS, 1);
    let early = k.thread(50, 40);
    // Deadline 5 ms before the first thread's 10 ms.
    assert_eq!(k.m.running(), Some(early));
    k.context(41, 10 * US, 5 * MS, 1);
    k.thread(52, 41);
    assert_eq!(k.m.running(), Some(early));
}

#[test]
fn exhausted_context_waits_for_its_replenishment_and_another_runs() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 4 * MS, 1);
    let thread = k.thread(50, 40);
    assert_eq!(k.m.running(), Some(thread));
    k.advance(MS);
    // Exhausted at 1 ms: throttled until its deadline, 4 ms; the first
    // thread runs from the ready queue.
    assert_eq!(k.m.running(), Some(k.t));
    let (consumed, remaining, deadline, state) = k.read(40);
    assert_eq!((consumed, remaining, deadline), (MS, 0, 4 * MS));
    assert_ne!(state & SCHED_STATE_THROTTLED, 0);
    k.advance(3 * MS);
    let (_, remaining, deadline, state) = k.read(40);
    assert_eq!(
        (remaining, deadline, state & SCHED_STATE_THROTTLED),
        (MS as i64, 8 * MS, 0)
    );
    assert_eq!(k.m.running(), Some(thread));
}

/// "Destroying a context …: the first step marks it dying, takes it out of
/// every queue of the scheduler": a throttled context whose destruction
/// takes several calls keeps its replenishment instant, which passes with
/// nothing to run it, and the state is valid (seed 329 of pass 23's long
/// run reached it and the check refused it).
#[test]
fn a_throttled_context_that_dies_over_several_calls_is_never_replenished() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 4 * MS, 1);
    let thread = k.thread(50, 40);
    k.advance(MS);
    assert_ne!(k.state(40) & SCHED_STATE_THROTTLED, 0);
    // 33 copies: its removal takes two calls, and the first marks it dying.
    for slot in 60..93 {
        k.ok(Syscall::CapDerive {
            source: s(40),
            destination: s(slot),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.no(Syscall::CapDelete(s(40)), Error::Again);
    assert!(!k.can_run(thread));
    // Its replenishment instant, 4 ms, passes: nothing replenishes it.
    k.advance(5 * MS);
    assert!(!k.can_run(thread));
    k.ok(Syscall::CapDelete(s(40)));
}

#[test]
fn throttled_context_runs_on_slack_when_no_context_has_budget() {
    let mut k = K::lowered(MS);
    k.advance(MS);
    // The first context is exhausted at 1 ms, alone: it runs on slack,
    // charged in consumed, not in its budget.
    assert_eq!(k.m.running(), Some(k.t));
    k.advance(2 * MS);
    let (consumed, remaining, ..) = k.read(FIRST);
    assert_eq!((consumed, remaining), (3 * MS, 0));
}

#[test]
fn slack_goes_to_the_smallest_class_first() {
    let mut k = K::lowered(MS);
    k.context(40, 10 * US, 10 * MS, 0);
    let frame = k.thread(50, 40);
    k.advance(MS + 10 * US);
    // Both exhausted: the frame class gets the slack.
    assert_eq!(k.m.running(), Some(frame));
}

#[test]
fn context_that_wakes_keeps_its_budget_only_within_its_bandwidth() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 10 * MS, 1);
    let thread = k.thread(50, 40);
    // It waits at once, then wakes 9 ms later: r × T = 1 × 10 is more than
    // (d − now) × B = 1 × 1, so r and d start afresh.
    assert_eq!(k.on(thread, Syscall::Wait(s(NT), 0)), Ok(Output::Waits));
    k.advance(9 * MS);
    k.ok(Syscall::Signal(s(NT)));
    let (_, remaining, deadline, _) = k.read(40);
    assert_eq!((remaining, deadline), (MS as i64, 19 * MS));
}

#[test]
fn yield_gives_up_the_budget_and_runs_again_on_an_idle_core() {
    let mut k = K::new();
    k.advance(2 * MS);
    k.ok(Syscall::Yield);
    let (consumed, remaining, deadline, state) = k.read(FIRST);
    assert_eq!((consumed, remaining, deadline), (2 * MS, 0, 10 * MS));
    assert_ne!(state & SCHED_STATE_THROTTLED, 0);
    // Alone on the core, it runs again at once, on slack.
    assert_eq!(k.m.running(), Some(k.t));
    assert!(k.m.census().on_slack);
}

#[test]
fn yield_of_a_throttled_context_goes_behind_the_others_of_its_instant() {
    let mut k = K::lowered(MS);
    k.context(40, 10 * US, 10 * MS, 1);
    let other = k.thread(50, 40);
    k.advance(MS + 10 * US);
    // Both throttled to 10 ms, class 1: the first throttled entered first.
    // The first thread's context was throttled first (the audit of lot
    // 8b, F12: neither was named, and none running passed).
    assert_eq!(k.m.running(), Some(k.t));
    assert_eq!(k.on(k.t, Syscall::Yield), DONE);
    assert_eq!(k.m.running(), Some(other));
}

#[test]
fn yield_from_a_thread_whose_space_is_dying_is_gone() {
    let mut k = K::lowered(5 * MS);
    k.ok(Syscall::UntypedRetype {
        untyped: s(UT),
        kind: ObjectKind::CSpace,
        size: 128,
        page: 0,
        memory: 0,
        destination: s(40),
    });
    k.go(Syscall::CSpaceGrow {
        cspace: s(40),
        untyped: s(UT),
    })
    .unwrap();
    for index in 1..40 {
        k.ok(Syscall::CapDerive {
            source: s(EP),
            destination: SlotAddr::through(common::slot(40), index),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.make(Thread, 41);
    k.ok(Syscall::ThreadConfigure {
        thread: s(41),
        cspace: s(40),
        aspace: s(START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    });
    k.ok(Syscall::ThreadResume(s(41)));
    let inside = k.m.object_at(k.t, s(41)).unwrap();
    k.no(Syscall::CapDelete(s(40)), Error::Again);
    assert_eq!(k.on(inside, Syscall::Yield), Err(Error::Gone));
}

// The budget follows the call.

#[test]
fn call_lends_the_caller_s_context_to_a_passive_receiver() {
    let mut k = K::lowered(5 * MS);
    let server = k.thread(50, 0);
    assert_eq!(k.on(server, recv(EP, RP)), Ok(Output::Waits));
    k.go(call(EP)).unwrap();
    assert_eq!(k.m.running(), Some(server));
    assert_ne!(k.read_by(server, FIRST).3 & SCHED_STATE_LENT, 0);
    // The time the server runs is charged to the client's context.
    k.advance(MS);
    assert_eq!(k.read_by(server, FIRST).0, MS);
}

#[test]
fn answer_keeps_the_lend_and_the_home_runs_first() {
    let mut k = K::lowered(5 * MS);
    let server = k.thread(50, 0);
    k.on(server, recv(EP, RP)).unwrap();
    k.go(call(EP)).unwrap();
    assert_eq!(k.on(server, answer(RP)), Ok(Output::Sent { flags: 0 }));
    // The client is runnable again: its home runs; the lend stays until the
    // server receives again.
    assert_eq!(k.m.running(), Some(k.t));
    assert_ne!(k.state(FIRST) & SCHED_STATE_LENT, 0);
    assert_eq!(k.on(server, recv(EP, RP)), Ok(Output::Waits));
    assert_eq!(k.state(FIRST) & SCHED_STATE_LENT, 0);
}

#[test]
fn receiver_with_an_own_context_gets_no_lend() {
    let mut k = K::lowered(5 * MS);
    k.context(40, 10 * US, 10 * MS, 1);
    let server = k.thread(50, 40);
    k.on(server, recv(EP, RP)).unwrap();
    k.go(call(EP)).unwrap();
    assert_eq!(k.read_by(server, FIRST).3 & SCHED_STATE_LENT, 0);
}

#[test]
fn passive_receiver_of_a_call_without_a_lend_is_marked_and_faults_no_context() {
    let mut k = K::lowered(5 * MS);
    // A client without a context, and a passive server whose fault
    // endpoint is EP; the first thread receives the fault.
    let server = k.thread(50, 0);
    let client = k.thread(52, 0);
    k.make(Endpoint, 33);
    k.on(
        server,
        Syscall::Recv {
            endpoint: s(33),
            reply: s(RP),
            range: Range::NONE,
            wait: 0,
        },
    )
    .unwrap();
    assert_eq!(k.on(client, call(33)), Ok(Output::Waits));
    assert!(!k.can_run(server));
    k.make(Reply, 34);
    let Ok(Output::Message(fault)) = k.go(recv(EP, 34)) else {
        panic!("no fault");
    };
    let header = fault.received.header;
    assert_eq!(header.label(), FaultKind::NoContext as u32);
    assert_eq!(fault.words[2..], [0, 0, 0]);
}

#[test]
fn passive_thread_gets_no_notification_and_no_send() {
    let mut k = K::lowered(5 * MS);
    let server = k.thread(50, 0);
    k.ok(Syscall::NotifyBind {
        notify: s(NT),
        endpoint: s(EP),
    });
    k.ok(Syscall::Signal(s(NT)));
    // The pending word stays pending for a passive receiver.
    assert_eq!(k.on(server, recv(EP, RP)), Ok(Output::Waits));
    // A send finds it, but it has no context: `would_wait`.
    let sent = Syscall::Send {
        target: s(EP),
        message: Message::default(),
    };
    k.no(sent, Error::WouldWait);
    k.ok(Syscall::Signal(s(NT)));
    assert_eq!(k.m.woken(server), None);
}

#[test]
fn destroying_the_reply_object_of_a_lend_marks_its_receiver() {
    let mut k = K::lowered(5 * MS);
    let server = k.thread(50, 0);
    k.on(server, recv(EP, RP)).unwrap();
    k.go(call(EP)).unwrap();
    k.on(server, answer(RP)).unwrap();
    k.ok(Syscall::CapDelete(s(RP)));
    // The server, runnable and without a context, raised the fault.
    assert!(!k.can_run(server));
    assert_eq!(k.m.census().fault_kinds[FaultKind::NoContext as usize], 1);
}

#[test]
fn destroying_a_context_marks_its_home_and_frees_its_share() {
    // Half of the core but the recovery reserve's 1/64.
    let half = 4_843_750;
    let mut k = K::lowered(5 * MS);
    k.context(40, half, 10 * MS, 1);
    let thread = k.thread(50, 40);
    k.ok(Syscall::CapDelete(s(40)));
    assert!(!k.can_run(thread));
    assert_eq!(k.m.census().fault_kinds[FaultKind::NoContext as usize], 1);
    k.context(41, half, 10 * MS, 1);
}

#[test]
fn binding_a_context_removes_the_mark() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 10 * MS, 1);
    k.thread(50, 40);
    k.ok(Syscall::ThreadSuspend(s(50)));
    k.ok(Syscall::CapDelete(s(40)));
    k.context(41, MS, 10 * MS, 1);
    k.ok(bind(41, 50));
    k.ok(Syscall::ThreadResume(s(50)));
    assert_eq!(k.m.census().faulting, 0);
}

#[test]
fn thread_without_an_address_space_faults_when_it_would_run() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 5 * MS, 1);
    k.make(Thread, 50);
    k.ok(Syscall::ThreadConfigure {
        thread: s(50),
        cspace: s(START_SLOT_CSPACE),
        aspace: SlotAddr::NONE,
        fault: s(EP),
        page: SlotAddr::NONE,
    });
    k.ok(bind(40, 50));
    let thread = k.m.object_at(k.t, s(50)).unwrap();
    k.ok(Syscall::ThreadResume(s(50)));
    assert!(!k.can_run(thread));
    assert_eq!(
        k.m.census().fault_kinds[FaultKind::NoAddressSpace as usize],
        1
    );
    assert_eq!(k.m.running(), Some(k.t));
}

// Timers.

#[test]
fn timer_fires_at_its_deadline_and_signals_its_notify() {
    let mut k = K::new();
    k.make(Timer, 40);
    assert_eq!(k.go(timer_set(40, NT, 2 * MS, 0)), Ok(Output::WasArmed(0)));
    k.advance(2 * MS - 1);
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    k.advance(1);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
    assert_eq!(k.go(Syscall::TimerCancel(s(40))), Ok(Output::WasArmed(0)));
}

#[test]
fn timer_fires_at_the_instant_with_the_most_trailing_zeros_of_its_window() {
    let mut k = K::new();
    k.make(Timer, 40);
    // Between 1,000 and 1,023 ns: at 1,008.
    k.ok_armed(timer_set(40, NT, 1000, 23), 0);
    k.advance(1007);
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    k.advance(1);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
}

impl K {
    #[track_caller]
    fn ok_armed(&mut self, call: Syscall, was: u64) {
        assert_eq!(self.go(call), Ok(Output::WasArmed(was)));
    }
}

#[test]
fn timer_set_again_replaces_the_arming_and_cancel_says_whether_it_was_armed() {
    let mut k = K::new();
    k.make(Timer, 40);
    k.ok_armed(timer_set(40, NT, 5 * MS, 0), 0);
    k.ok_armed(timer_set(40, NT, 7 * MS, 0), 1);
    k.advance(6 * MS);
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
    k.ok_armed(Syscall::TimerCancel(s(40)), 1);
    k.ok_armed(Syscall::TimerCancel(s(40)), 0);
    k.advance(2 * MS);
    k.no(Syscall::Poll(s(NT)), Error::WouldWait);
}

#[test]
fn timer_whose_instant_is_not_after_now_fires_in_the_call() {
    let mut k = K::new();
    k.make(Timer, 40);
    k.advance(MS);
    k.ok_armed(timer_set(40, NT, MS, 0), 0);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(1)));
    k.ok_armed(Syscall::TimerCancel(s(40)), 0);
}

#[test]
fn timer_signals_the_bits_of_the_badge_given() {
    let mut k = K::new();
    k.make(Timer, 40);
    k.ok(Syscall::CapDerive {
        source: s(NT),
        destination: s(41),
        rights: Rights::ALL.bits(),
        badge: 0b1010,
    });
    k.ok_armed(timer_set(40, 41, MS, 0), 0);
    k.advance(MS);
    assert_eq!(k.go(Syscall::Poll(s(NT))), Ok(Output::Word(0b1010)));
}

#[test]
fn timer_set_checks_its_rights_and_a_deadline_below_2_to_the_63() {
    let mut k = K::new();
    k.make(Timer, 40);
    for (from, to, rights) in [(40, 41, Rights::READ), (NT, 42, Rights::RECV)] {
        k.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(to),
            rights: rights.bits(),
            badge: 0,
        });
    }
    k.no(timer_set(41, NT, MS, 0), Error::Denied);
    k.no(timer_set(40, 42, MS, 0), Error::Denied);
    k.no(timer_set(40, NT, 1 << 63, 0), Error::Invalid);
    k.no(Syscall::TimerCancel(s(41)), Error::Denied);
    k.ok_armed(timer_set(40, NT, (1 << 63) - 1, u64::MAX), 0);
}

#[test]
fn destroying_a_notify_disarms_its_timers() {
    let mut k = K::new();
    k.make(Timer, 40);
    k.ok_armed(timer_set(40, NT, MS, 0), 0);
    k.ok(Syscall::CapDelete(s(NT)));
    k.ok_armed(Syscall::TimerCancel(s(40)), 0);
}

#[test]
fn twenty_timers_due_at_one_instant_all_fire() {
    let mut k = K::new();
    for n in 0..20 {
        k.make(Timer, 40 + n);
        k.make(Notify, 70 + n);
        k.ok_armed(timer_set(40 + n, 70 + n, MS, 0), 0);
    }
    k.advance(MS);
    for n in 0..20 {
        assert_eq!(k.go(Syscall::Poll(s(70 + n))), Ok(Output::Word(1)));
    }
}

#[test]
fn firing_timer_wakes_a_waiter() {
    let mut k = K::lowered(5 * MS);
    k.context(50, MS, 10 * MS, 1);
    let waiter = k.thread(51, 50);
    k.make(Timer, 40);
    k.ok_armed(timer_set(40, NT, MS, 0), 0);
    assert_eq!(k.on(waiter, Syscall::Wait(s(NT), 0)), Ok(Output::Waits));
    k.advance(MS);
    assert_eq!(k.m.woken(waiter), Some(&Ok(Output::Word(1))));
}

#[test]
fn wake_up_rule_judges_with_the_parameters_in_force_and_resets_with_the_new_ones() {
    // Half of the core but the recovery reserve's 1/64.
    let mut k = K::lowered(4_843_750);
    k.context(40, 2 * MS, 10 * MS, 1);
    let thread = k.thread(50, 40);
    assert_eq!(k.on(thread, Syscall::Wait(s(NT), 0)), Ok(Output::Waits));
    // New parameters, 1 ms every 2 ms: r becomes 1 ms, d stays 10 ms.
    k.ok(configure(40, MS, 2 * MS, 1));
    k.advance(7 * MS);
    k.ok(Syscall::Signal(s(NT)));
    // With the parameters in force, 1 × 10 > 3 × 2: r and d start afresh,
    // with the new ones: d = 7 + 2 ms.
    let (_, remaining, deadline, _) = k.read(40);
    assert_eq!((remaining, deadline), (MS as i64, 9 * MS));
}

#[test]
fn timer_whose_notify_is_dying_is_disarmed_without_a_signal() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 10 * MS, 1);
    let receiver = k.thread(50, 40);
    k.ok(Syscall::NotifyBind {
        notify: s(NT),
        endpoint: s(EP),
    });
    assert_eq!(k.on(receiver, recv(EP, RP)), Ok(Output::Waits));
    for slot in 60..93 {
        k.ok(Syscall::CapDerive {
            source: s(NT),
            destination: s(slot),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.make(Timer, 41);
    k.ok_armed(timer_set(41, NT, MS, 0), 0);
    k.no(Syscall::CapDelete(s(NT)), Error::Again);
    k.advance(MS);
    assert_eq!(k.m.woken(receiver), None);
    k.ok(Syscall::CapDelete(s(NT)));
    k.ok_armed(Syscall::TimerCancel(s(41)), 0);
}

// The settled problems of pass 11: faults *no context*, and the order of an
// event.

/// A passive thread in `slot`: bound to the start-up spaces with a message
/// page and the fault endpoint `fault`, resumed, with no context.
fn passive(k: &mut K, slot: u32, fault: u32) -> ObjId {
    k.make(Thread, slot);
    k.make(Frame, slot + 1);
    k.ok(Syscall::ThreadConfigure {
        thread: s(slot),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(START_SLOT_ASPACE),
        fault: s(fault),
        page: s(slot + 1),
    });
    k.ok(Syscall::ThreadResume(s(slot)));
    k.m.object_at(k.t, s(slot)).unwrap()
}

/// The fault record that `thread_read_regs` returns after the block.
fn recorded(k: &mut K, slot: u32) -> Vec<u64> {
    match k.go(Syscall::ThreadReadRegs(s(slot))) {
        Ok(Output::Regs(block)) => block[x86_64::REGS_WORDS..].to_vec(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn fault_no_context_is_not_delivered_to_a_passive_receiver() {
    let mut k = K::lowered(5 * MS);
    let handler = passive(&mut k, 50, EP);
    assert_eq!(k.on(handler, recv(EP, RP)), Ok(Output::Waits));
    k.context(40, MS, 10 * MS, 1);
    let victim = k.thread(60, 40);
    // The context of the victim dies: it raises the fault, which the first
    // receiver, without an own context, does not get.
    k.ok(Syscall::CapDelete(s(40)));
    assert!(!k.can_run(victim));
    assert_eq!(k.m.woken(handler), None);
    assert_eq!(k.m.census().receivers, 1);
    assert_eq!(recorded(&mut k, 60)[0], FaultKind::NoContext as u64);
    // No second fault: the handler did not lose anything.
    assert_eq!(k.m.census().faulting, 0);
}

#[test]
fn passive_receiver_that_finds_a_fault_no_context_gets_again() {
    let mut k = K::lowered(5 * MS);
    k.context(40, MS, 10 * MS, 1);
    let victim = k.thread(60, 40);
    k.ok(Syscall::CapDelete(s(40)));
    // Nobody waited: the fault is queued, as any call.
    assert_eq!(k.m.census().fault_kinds[FaultKind::NoContext as usize], 1);
    let handler = passive(&mut k, 50, EP);
    assert_eq!(k.on(handler, recv(EP, RP)), Err(Error::Again));
    // The faulting thread left the queue, suspended with its fault
    // recorded; the reply object stayed idle.
    assert_eq!(k.m.census().fault_kinds[FaultKind::NoContext as usize], 0);
    assert!(!k.can_run(victim));
    assert_eq!(recorded(&mut k, 60)[0], FaultKind::NoContext as u64);
    assert_eq!(k.on(handler, recv(EP, RP)), Ok(Output::Waits));
}

#[test]
fn faults_no_context_are_raised_in_the_order_of_the_no_context_list() {
    let mut k = K::lowered(5 * MS);
    // Two passive servers, A then B, whose fault endpoint capabilities
    // carry the badges 0xA and 0xB.
    for (slot, badge) in [(74, 0xA), (75, 0xB)] {
        k.ok(Syscall::CapDerive {
            source: s(EP),
            destination: s(slot),
            rights: Rights::SEND.bits(),
            badge,
        });
    }
    for slot in [70, 71] {
        k.make(Endpoint, slot);
    }
    for slot in [72, 73, 76] {
        k.make(Reply, slot);
    }
    let a = passive(&mut k, 50, 74);
    let b = passive(&mut k, 52, 75);
    assert_eq!(k.on(a, recv(70, 72)), Ok(Output::Waits));
    assert_eq!(k.on(b, recv(71, 73)), Ok(Output::Waits));
    // A client lends its context to A, which answers and keeps the lend,
    // then to B, on top.
    k.context(40, MS, 10 * MS, 1);
    let client = k.thread(45, 40);
    assert_eq!(k.on(client, call(70)), Ok(Output::Waits));
    k.on(a, answer(72)).unwrap();
    assert_eq!(k.on(client, call(71)), Ok(Output::Waits));
    // The context dies: its lends are cleared from the top, B then A, and
    // each is marked while it runs: B's fault comes first.
    k.ok(Syscall::CapDelete(s(40)));
    let badges: Vec<u64> = [RP, 76]
        .into_iter()
        .map(|reply| match k.go(recv(EP, reply)) {
            Ok(Output::Message(delivery)) => delivery.received.badge,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(badges, [0xB, 0xA]);
}

#[test]
fn contexts_enter_their_queues_in_the_order_of_the_change_list() {
    let mut k = K::lowered(5 * MS);
    k.make(Endpoint, 70);
    k.make(Reply, 72);
    k.make(Reply, 73);
    // Two contexts made in order, with the same deadline.
    k.make(Sched, 40);
    k.make(Sched, 41);
    k.ok(configure(40, 100 * US, MS, 1));
    k.ok(configure(41, 100 * US, MS, 1));
    let first = k.thread(50, 40);
    let second = k.thread(52, 41);
    // The second waits first; destroying the endpoint wakes it first, and
    // its context enters the ready queue first.
    assert_eq!(k.on(second, recv(70, 72)), Ok(Output::Waits));
    assert_eq!(k.on(first, recv(70, 73)), Ok(Output::Waits));
    k.ok(Syscall::CapDelete(s(70)));
    assert_eq!(k.m.running(), Some(second));
}
