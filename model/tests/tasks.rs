//! Tasks and marks: one test per rule, on the start-up state of
//! `Machine::example` (the start-up task in slot 8, mark-aware, with the
//! first thread).

mod common;

use dodos_abi::ObjectKind::{Endpoint, Frame, Reply, Sched, Task, Thread};
use dodos_abi::*;
use dodos_model::*;

const UT: u32 = 20;
const START: u32 = START_SLOT_TASK;
const EP: u32 = 30;
const RP: u32 = 31;
const DONE: Outcome = Ok(Output::Done);
const U: u64 = 2;
const P: u64 = 1;

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(common::slot(index))
}

struct K {
    m: Model,
    t: ObjId,
    spare: u32,
}

impl K {
    /// With an endpoint and a reply object, and the first thread's context
    /// at half of the core.
    fn new() -> Self {
        let (m, t) = Model::boot(&Machine::example()).unwrap();
        let mut k = Self { m, t, spare: 3000 };
        k.make(Endpoint, EP);
        k.make(Reply, RP);
        k.ok(Syscall::SchedConfigure {
            sched: s(START_SLOT_SCHED),
            node: s(16),
            core: 0,
            budget: 5_000_000,
            period: 10_000_000,
            class: 1,
        });
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

    fn make(&mut self, kind: ObjectKind, slot: u32) {
        self.ok(Syscall::UntypedRetype {
            untyped: s(UT),
            kind,
            size: (kind == Frame) as u64,
            page: 0,
            memory: 0,
            destination: s(slot),
        });
    }

    fn spare(&mut self) -> u32 {
        self.spare += 1;
        self.spare
    }

    /// A running thread in `slot` with a message page and a context of its
    /// own, attached to the task in `task` if it is not 0.
    fn thread(&mut self, slot: u32, task: u32) -> ObjId {
        self.make(Thread, slot);
        let (page, sched) = (self.spare(), self.spare());
        self.make(Frame, page);
        self.make(Sched, sched);
        self.ok(Syscall::SchedConfigure {
            sched: s(sched),
            node: s(16),
            core: 0,
            budget: 10_000,
            period: 10_000_000,
            class: 1,
        });
        self.ok(Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: s(EP),
            page: s(page),
        });
        self.ok(Syscall::SchedBind {
            sched: s(sched),
            thread: s(slot),
        });
        if task != 0 {
            self.ok(attach(slot, task));
        }
        self.ok(Syscall::ThreadResume(s(slot)));
        self.m.object_at(self.t, s(slot)).unwrap()
    }

    /// `task_read`: identifier, marks, flags, threads.
    fn read(&mut self, slot: u32) -> (u64, u64, u64, u64) {
        match self.go(Syscall::TaskRead(s(slot))) {
            Ok(Output::Task {
                id,
                marks,
                flags,
                threads,
            }) => (id, marks, flags, threads),
            other => panic!("{other:?}"),
        }
    }

    fn marks(&self, thread: ObjId) -> u64 {
        self.m.marks(thread).unwrap().bits()
    }
}

fn attach(thread: u32, task: u32) -> Syscall {
    Syscall::TaskAttach {
        thread: s(thread),
        task: if task == 0 { SlotAddr::NONE } else { s(task) },
    }
}

fn mark(task: u32, marks: u64) -> Syscall {
    Syscall::TaskMark {
        task: s(task),
        marks,
    }
}

fn recv() -> Syscall {
    Syscall::Recv {
        endpoint: s(EP),
        reply: s(RP),
        range: Range::NONE,
        wait: 0,
    }
}

fn send(caps: Vec<Transfer>, flags: u32, marks: u64) -> Syscall {
    Syscall::Send {
        target: s(EP),
        message: Message {
            flags,
            marks,
            caps,
            ..Message::default()
        },
    }
}

/// The stamp of the message a thread received: marks and task.
fn stamp(k: &K, thread: ObjId) -> (u64, u64) {
    let Some(Ok(Output::Message(delivery))) = k.m.woken(thread) else {
        panic!("no message");
    };
    (delivery.received.marks.bits(), delivery.received.task)
}

#[test]
fn start_up_task_is_identifier_1_mark_aware_with_the_first_thread() {
    let mut k = K::new();
    assert_eq!(k.read(START), (1, 0, TaskFlags::AWARE, 1));
}

#[test]
fn new_task_has_the_next_identifier_and_nothing_else() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Task, 41);
    assert_eq!(k.read(40), (2, 0, 0, 0));
    assert_eq!(k.read(41).0, TaskId::new(0, 3).raw());
}

#[test]
fn attaching_moves_the_effective_marks_into_the_new_task() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Task, 41);
    k.ok(mark(40, U));
    let thread = k.thread(50, 40);
    assert_eq!(k.marks(thread), U);
    k.ok(Syscall::TaskExempt {
        task: s(41),
        aware: s(START),
    });
    // Even into a mark-aware task.
    k.ok(attach(50, 41));
    assert_eq!(k.read(41), (3, U, TaskFlags::AWARE, 1));
    assert_eq!(k.read(40).3, 0);
    // Attached again to the same task: nothing changes.
    k.ok(attach(50, 41));
    // Detached: its own word keeps the task's marks.
    k.ok(mark(41, P));
    k.ok(attach(50, 0));
    assert_eq!(k.marks(thread), U | P);
    k.ok(attach(50, 0));
}

#[test]
fn task_attach_needs_manage_on_the_thread_and_on_the_task() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Thread, 50);
    for (from, to) in [(50, 51), (40, 41)] {
        k.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(to),
            rights: Rights::READ.bits(),
            badge: 0,
        });
    }
    k.no(attach(51, 40), Error::Denied);
    k.no(attach(50, 41), Error::Denied);
    k.no(attach(50, EP), Error::Invalid);
}

#[test]
fn task_mark_needs_write_and_refuses_a_reserved_bit() {
    let mut k = K::new();
    k.make(Task, 40);
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(41),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(mark(41, U), Error::Denied);
    k.no(mark(40, 1 << 2), Error::Invalid);
    k.ok(mark(40, 0));
    k.ok(mark(40, U));
    k.ok(mark(40, P));
    assert_eq!(k.read(40).1, U | P);
}

#[test]
fn task_exempt_needs_a_mark_aware_task_judged_as_a_right() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Task, 41);
    let exempt = |task, aware| Syscall::TaskExempt {
        task: s(task),
        aware: s(aware),
    };
    k.no(exempt(40, 41), Error::Denied);
    k.no(exempt(40, EP), Error::Invalid);
    k.ok(exempt(40, START));
    k.ok(exempt(41, 40));
    k.ok(exempt(41, 40));
    assert_eq!(k.read(41).2, TaskFlags::AWARE);
}

#[test]
fn task_revoke_stops_its_threads_32_per_call() {
    let mut k = K::new();
    k.make(Task, 40);
    let mut threads = Vec::new();
    for slot in (100..133).map(|n| n * 2) {
        threads.push(k.thread(slot, 40));
    }
    k.ok(mark(40, U));
    // A waiting thread: its wait ends with `gone`.
    assert_eq!(k.on(threads[32], recv()), Ok(Output::Waits));
    // The first call writes the flag (and marks the threads other cores
    // run): its whole step, it detaches nothing (D-137).
    k.no(Syscall::TaskRevoke(s(40)), Error::Again);
    assert_eq!(k.m.woken(threads[32]), None);
    assert_eq!(k.read(40), (2, U, TaskFlags::REVOKED, 33));
    k.no(Syscall::TaskRevoke(s(40)), Error::Again);
    assert_eq!(k.m.woken(threads[32]), Some(&Err(Error::Gone)));
    assert_eq!(k.read(40), (2, U, TaskFlags::REVOKED, 1));
    k.ok(Syscall::TaskRevoke(s(40)));
    assert_eq!(k.read(40).3, 0);
    k.ok(Syscall::TaskRevoke(s(40)));
    for thread in threads {
        assert!(!k.m.runnable().contains(&thread));
        assert_eq!(k.marks(thread), U);
    }
}

#[test]
fn revoked_task_is_gone_for_the_calls_that_name_it_but_read_revoke_and_cap_calls() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Thread, 50);
    k.ok(Syscall::TaskRevoke(s(40)));
    k.no(attach(50, 40), Error::Gone);
    k.no(mark(40, U), Error::Gone);
    let exempt = Syscall::TaskExempt {
        task: s(40),
        aware: s(START),
    };
    k.no(exempt, Error::Gone);
    let aware = Syscall::TaskExempt {
        task: s(START),
        aware: s(40),
    };
    k.no(aware, Error::Gone);
    assert_eq!(k.read(40).2, TaskFlags::REVOKED);
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(41),
        rights: Rights::ALL.bits(),
        badge: 0,
    });
    k.ok(Syscall::CapDelete(s(41)));
}

#[test]
fn thread_of_a_revoked_task_gets_gone_from_every_call_and_never_runs() {
    let mut k = K::new();
    k.make(Task, 40);
    let threads: Vec<ObjId> = (0..33).map(|n| k.thread(200 + 2 * n, 40)).collect();
    k.no(Syscall::TaskRevoke(s(40)), Error::Again);
    // The first call detaches nothing (D-137); the first thread attached
    // is still attached, and gets `gone` (the final review's F2 Opus 37).
    let early = threads[0];
    assert_eq!(k.on(early, Syscall::CapDelete(s(EP))), Err(Error::Gone));
    assert_eq!(k.on(early, Syscall::Yield), Err(Error::Gone));
    assert_ne!(k.m.running(), Some(early));
    // A thread attached to a revoked task cannot be moved to another.
    k.make(Task, 41);
    k.no(attach(200, 41), Error::Busy);
}

#[test]
fn delivery_stamps_the_sender_s_task_and_marks_with_the_added_marks() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Task, 41);
    k.ok(mark(40, U));
    let receiver = k.thread(50, 41);
    let sender = k.thread(52, 40);
    assert_eq!(k.on(receiver, recv()), Ok(Output::Waits));
    let added = send(vec![], Header::FLAG_ADD_MARKS, P);
    assert_eq!(k.on(sender, added), Ok(Output::Sent { flags: 0 }));
    assert_eq!(stamp(&k, receiver), (U | P, TaskId::new(0, 2).raw()));
    // The receiver takes the marks; the sender's own are unchanged.
    assert_eq!(k.marks(receiver), U | P);
    assert_eq!(k.marks(sender), U);
}

#[test]
fn mark_aware_receiver_takes_no_marks_and_a_thread_without_a_task_keeps_them() {
    let mut k = K::new();
    k.make(Task, 40);
    k.ok(mark(40, U));
    let sender = k.thread(52, 40);
    // The first thread is in the mark-aware start-up task.
    let handler = k.t;
    assert_eq!(k.on(handler, recv()), Ok(Output::Waits));
    k.on(sender, send(vec![], 0, 0)).unwrap();
    assert_eq!(stamp(&k, handler), (U, TaskId::new(0, 2).raw()));
    assert_eq!(k.marks(handler), 0);
    // A thread attached to no task keeps marks in its own word.
    let plain = k.thread(54, 0);
    assert_eq!(k.on(plain, recv()), Ok(Output::Waits));
    k.on(sender, send(vec![], 0, 0)).unwrap();
    assert_eq!(k.marks(plain), U);
    assert_eq!(stamp(&k, plain).1, TaskId::new(0, 2).raw());
}

#[test]
fn sender_of_a_capability_takes_the_receiver_s_former_marks() {
    let mut k = K::new();
    k.make(Task, 40);
    k.make(Task, 41);
    k.ok(mark(41, U));
    let sender = k.thread(52, 40);
    let receiver = k.thread(50, 41);
    // A range of one slot for the capability.
    let ranged = Syscall::Recv {
        endpoint: s(EP),
        reply: s(RP),
        range: Range {
            first: s(60),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(k.on(receiver, ranged.clone()), Ok(Output::Waits));
    let derived = vec![Transfer::Derive(s(EP), Rights::ALL.bits())];
    assert_eq!(
        k.on(sender, send(derived.clone(), 0, 0)),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(k.marks(sender), U);
    // A transfer that is dropped carries nothing back.
    let fresh = k.thread(54, 0);
    let receiver2 = k.thread(56, 41);
    assert_eq!(k.on(receiver2, recv()), Ok(Output::Waits));
    let dropped = Header::FLAG_CAPS_DROPPED;
    assert_eq!(
        k.on(fresh, send(derived, 0, 0)),
        Ok(Output::Sent { flags: dropped })
    );
    assert_eq!(k.marks(fresh), 0);
}

#[test]
fn fault_message_carries_the_marks_of_the_faulting_thread() {
    let mut k = K::new();
    k.make(Task, 40);
    k.ok(mark(40, U));
    let thread = k.thread(52, 40);
    // The first thread, mark-aware, handles the faults on EP.
    assert_eq!(k.go(recv()), Ok(Output::Waits));
    // An address no space of the node translates: a fault on a translated
    // address with the access asked is spurious (D-129).
    k.m.fault(thread, FaultKind::Page, [14, 2, 0x10_0000_0000]);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(stamp(&k, k.t), (U, TaskId::new(0, 2).raw()));
}

#[test]
fn destroying_a_task_detaches_its_threads_with_its_marks() {
    let mut k = K::new();
    k.make(Task, 40);
    k.ok(mark(40, U));
    let thread = k.thread(50, 40);
    k.ok(Syscall::CapDelete(s(40)));
    assert_eq!(k.marks(thread), U);
    assert!(k.m.runnable().contains(&thread));
}

#[test]
fn destroying_a_revoked_task_stops_the_threads_still_attached() {
    let mut k = K::new();
    k.make(Task, 41);
    let threads: Vec<ObjId> = (0..33).map(|n| k.thread(200 + 2 * n, 41)).collect();
    k.no(Syscall::TaskRevoke(s(41)), Error::Again);
    // 33 threads still attached: two steps of the removal.
    k.no(Syscall::CapDelete(s(41)), Error::Again);
    k.ok(Syscall::CapDelete(s(41)));
    assert!(
        threads
            .iter()
            .all(|thread| !k.m.runnable().contains(thread))
    );
}

#[test]
fn destroyed_thread_leaves_its_task() {
    let mut k = K::new();
    k.make(Task, 42);
    k.thread(54, 42);
    assert_eq!(k.read(42).3, 1);
    k.ok(Syscall::CapDelete(s(54)));
    assert_eq!(k.read(42).3, 0);
}

#[test]
fn added_marks_word_is_read_only_with_the_flag() {
    let mut k = K::new();
    let receiver = k.thread(50, 0);
    let sender = k.thread(52, 0);
    assert_eq!(k.on(receiver, recv()), Ok(Output::Waits));
    assert_eq!(
        k.on(sender, send(vec![], 0, U)),
        Ok(Output::Sent { flags: 0 })
    );
    assert_eq!(stamp(&k, receiver).0, 0);
}

#[test]
fn mark_aware_sender_of_a_capability_takes_no_marks() {
    let mut k = K::new();
    k.make(Task, 41);
    k.ok(mark(41, U));
    let receiver = k.thread(50, 41);
    let ranged = Syscall::Recv {
        endpoint: s(EP),
        reply: s(RP),
        range: Range {
            first: s(60),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(k.on(receiver, ranged), Ok(Output::Waits));
    // The first thread is in the mark-aware start-up task.
    let derived = vec![Transfer::Derive(s(EP), Rights::ALL.bits())];
    assert_eq!(k.go(send(derived, 0, 0)), Ok(Output::Sent { flags: 0 }));
    assert_eq!(k.marks(k.t), 0);
}

#[test]
fn revoked_thread_that_the_choice_would_take_is_suspended() {
    let mut k = K::new();
    k.make(Task, 40);
    // The first thread attached (its context in slot 3002) is given a
    // context of 1 ms period: the nearest deadline.
    let early = k.thread(300, 40);
    for n in 0..32 {
        k.thread(310 + 2 * n, 40);
    }
    k.make(Sched, 302);
    k.ok(Syscall::SchedConfigure {
        sched: s(302),
        node: s(16),
        core: 0,
        budget: 10_000,
        period: 1_000_000,
        class: 1,
    });
    k.ok(Syscall::SchedUnbind(s(3002)));
    k.ok(Syscall::SchedBind {
        sched: s(302),
        thread: s(300),
    });
    assert_eq!(k.m.running(), Some(early));
    // The first call detaches nothing (D-137); the early one stays attached
    // to the revoked task, and is suspended when the choice takes it (the
    // final review's F2 Opus 37).
    k.no(Syscall::TaskRevoke(s(40)), Error::Again);
    assert!(!k.m.runnable().contains(&early));
    assert_ne!(k.m.running(), Some(early));
}

#[test]
fn revoked_task_is_gone_before_its_kind_and_rights_are_judged() {
    let mut k = K::new();
    k.make(Task, 40);
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(41),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(mark(41, U), Error::Denied);
    k.ok(Syscall::TaskRevoke(s(40)));
    k.no(mark(41, U), Error::Gone);
}
