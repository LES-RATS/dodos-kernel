//! Threads: faults and the register calls, one test per rule, on the
//! start-up space of `Machine::example`.

use dodos_abi::ObjectKind::{Endpoint, Frame, Reply, Thread};
use dodos_abi::x86_64::{REG_RAX, REG_RDI, REG_RFLAGS, REG_RIP, REG_RSP, REGS_WORDS};
use dodos_abi::*;
use dodos_model::*;

const UT: u32 = 20;
const EP: u32 = 30;
const RP: u32 = 31;
const DONE: Outcome = Ok(Output::Done);
const ENTRY: u64 = 0x20_0000;

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

struct K {
    m: Model,
    t: ObjId,
}

impl K {
    /// The first thread, with an endpoint and a reply object.
    fn new() -> Self {
        let (m, t) = Model::boot(&Machine::example()).unwrap();
        let mut k = Self { m, t };
        k.make(Endpoint, EP);
        k.make(Reply, RP);
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

    /// A thread in `slot`, bound to the start-up spaces, with a message page
    /// in `slot + 1` if `page`, and the capability in `fault` as its fault
    /// endpoint; suspended.
    fn thread(&mut self, slot: u32, fault: u32, page: bool) -> ObjId {
        self.make(Thread, slot);
        if page {
            self.make(Frame, slot + 1);
        }
        let optional = |used, index| if used { s(index) } else { SlotAddr::NONE };
        self.ok(Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: optional(fault != 0, fault),
            page: optional(page, slot + 1),
        });
        self.m.object_at(self.t, s(slot)).unwrap()
    }

    /// As `thread`, and running.
    fn running(&mut self, slot: u32, fault: u32) -> ObjId {
        let thread = self.thread(slot, fault, true);
        self.ok(Syscall::ThreadResume(s(slot)));
        thread
    }

    fn fault(&mut self, thread: ObjId, kind: FaultKind) {
        self.m.fault(thread, kind, [14, ACCESS_WRITE, 0xdead_0000]);
        assert_eq!(self.m.check(), Ok(()));
    }

    fn can_run(&self, thread: ObjId) -> bool {
        self.m.runnable().contains(&thread)
    }

    fn regs(&mut self, slot: u32) -> Vec<u64> {
        match self.go(Syscall::ThreadReadRegs(s(slot))) {
            Ok(Output::Regs(block)) => block,
            other => panic!("{other:?}"),
        }
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

/// An answer through the reply object, with a label and words.
fn answer(label: u32, words: Vec<u64>) -> Syscall {
    Syscall::Send {
        target: s(RP),
        message: Message {
            label,
            words,
            ..Message::default()
        },
    }
}

/// The fault message the first thread receives: label, words, badge.
fn received(outcome: Outcome) -> (u32, Vec<u64>, u64, usize, u32) {
    let Ok(Output::Message(delivery)) = outcome else {
        panic!("not a message: {outcome:?}");
    };
    let header = delivery.received.header;
    (
        header.label(),
        delivery.words,
        delivery.received.badge,
        header.caps(),
        header.flags(),
    )
}

#[test]
fn fault_without_a_fault_endpoint_suspends_the_thread_and_is_dropped() {
    let mut k = K::new();
    let thread = k.running(40, 0);
    k.fault(thread, FaultKind::Page);
    assert!(!k.can_run(thread));
    // Its registers are unchanged: resumed, it runs the same instruction.
    assert_eq!(k.regs(40)[REG_RIP], 0);
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(thread));
    assert_eq!(k.m.census().faulting, 0);
}

#[test]
fn fault_on_a_dying_fault_endpoint_suspends_the_thread() {
    let mut k = K::new();
    let thread = k.running(40, EP);
    // 33 copies of the endpoint: its removal takes two calls.
    for slot in 50..83 {
        k.ok(Syscall::CapDerive {
            source: s(EP),
            destination: s(slot),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.make(Endpoint, 49);
    assert_eq!(k.go(Syscall::CapDelete(s(EP))), Err(Error::Again));
    k.fault(thread, FaultKind::Undefined);
    assert!(!k.can_run(thread));
    assert_eq!(k.m.census().faulting, 0);
}

#[test]
fn fault_is_a_call_on_the_fault_endpoint_with_five_words_and_its_badge() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: s(EP),
        destination: s(32),
        rights: Rights::SEND.bits(),
        badge: 0x77,
    });
    let thread = k.running(40, 32);
    k.ok(Syscall::ThreadSuspend(s(40)));
    let mut block = k.regs(40);
    (block[REG_RIP], block[REG_RSP]) = (0x40_1000, 0x7000);
    k.ok(Syscall::ThreadWriteRegs {
        thread: s(40),
        block,
    });
    k.ok(Syscall::ThreadResume(s(40)));
    // No receiver yet: the faulting thread joins the queue.
    k.fault(thread, FaultKind::Page);
    assert!(!k.can_run(thread));
    assert_eq!(k.m.census().callers, 1);
    let words = vec![0x40_1000, 0x7000, 14, ACCESS_WRITE, 0xdead_0000];
    assert_eq!(received(k.go(recv())), (1, words, 0x77, 0, 0));
    assert_eq!(k.m.census().served, 1);
}

#[test]
fn fault_is_delivered_at_once_to_a_handler_that_waits_to_receive() {
    let mut k = K::new();
    let handler = k.running(44, 0);
    let thread = k.running(40, EP);
    assert_eq!(k.on(handler, recv()), Ok(Output::Waits));
    k.fault(thread, FaultKind::BadCall);
    let (label, words, ..) = received(k.m.woken(handler).cloned().unwrap());
    assert_eq!(
        (label, words.len()),
        (FaultKind::BadCall as u32, FAULT_WORDS)
    );
}

#[test]
fn fault_reaches_a_handler_without_a_message_page_in_its_registers() {
    let mut k = K::new();
    let handler = k.thread(44, 0, false);
    k.ok(Syscall::ThreadResume(s(44)));
    let thread = k.running(40, EP);
    assert_eq!(k.on(handler, recv()), Ok(Output::Waits));
    k.fault(thread, FaultKind::Arithmetic);
    let (_, words, ..) = received(k.m.woken(handler).cloned().unwrap());
    assert_eq!(words.len(), FAULT_WORDS);
}

#[test]
fn fault_is_sent_when_the_capability_space_of_the_thread_is_dying() {
    let mut k = K::new();
    // A space of its own, with 40 capabilities to sweep.
    k.ok(Syscall::UntypedRetype {
        untyped: s(UT),
        kind: ObjectKind::CSpace,
        size: 128,
        page: 0,
        memory: 0,
        destination: s(60),
    });
    k.go(Syscall::CSpaceGrow {
        cspace: s(60),
        untyped: s(UT),
    })
    .unwrap();
    for index in 1..41 {
        k.ok(Syscall::CapDerive {
            source: s(EP),
            destination: SlotAddr::through(60, index),
            rights: Rights::ALL.bits(),
            badge: 0,
        });
    }
    k.make(Thread, 61);
    k.ok(Syscall::ThreadConfigure {
        thread: s(61),
        cspace: s(60),
        aspace: s(START_SLOT_ASPACE),
        fault: s(EP),
        page: SlotAddr::NONE,
    });
    k.ok(Syscall::ThreadResume(s(61)));
    let thread = k.m.object_at(k.t, s(61)).unwrap();
    assert_eq!(k.go(Syscall::CapDelete(s(60))), Err(Error::Again));
    k.fault(thread, FaultKind::Protection);
    assert_eq!(k.m.census().faulting, 1);
    assert_eq!(received(k.go(recv())).0, FaultKind::Protection as u32);
}

/// A thread in slot 40 that faulted on `EP`, and whose fault the first
/// thread received.
fn faulted(k: &mut K) -> ObjId {
    let thread = k.running(40, EP);
    k.fault(thread, FaultKind::Page);
    received(k.go(recv()));
    thread
}

#[test]
fn answer_with_label_0_resumes_the_thread_where_it_faulted() {
    let mut k = K::new();
    let thread = faulted(&mut k);
    assert_eq!(k.go(answer(0, vec![9, 9])), Ok(Output::Sent { flags: 0 }));
    assert!(k.can_run(thread));
    assert_eq!(k.m.woken(thread), None);
    k.ok(Syscall::ThreadSuspend(s(40)));
    assert_eq!(k.regs(40)[REG_RIP], 0);
}

#[test]
fn answer_with_label_1_and_two_words_redirects_the_thread() {
    let mut k = K::new();
    let thread = faulted(&mut k);
    k.go(answer(1, vec![0x40_2000, 0x8000, 5])).unwrap();
    assert!(k.can_run(thread));
    k.ok(Syscall::ThreadSuspend(s(40)));
    let block = k.regs(40);
    assert_eq!((block[REG_RIP], block[REG_RSP]), (0x40_2000, 0x8000));
}

#[test]
fn answer_with_another_label_or_a_redirect_that_cannot_be_leaves_it_suspended() {
    for (label, words) in [
        (2, vec![0x40_2000, 0x8000]),
        (1, vec![0x40_2000]),
        (1, vec![1 << 47, 0x8000]),
        (1, vec![0x40_2000, 1 << 63]),
    ] {
        let mut k = K::new();
        let thread = faulted(&mut k);
        k.go(answer(label, words)).unwrap();
        assert!(!k.can_run(thread));
        // Its registers are as at the fault.
        let block = k.regs(40);
        assert_eq!((block[REG_RIP], block[REG_RSP]), (0, 0));
    }
}

#[test]
fn capabilities_in_the_answer_are_dropped() {
    let mut k = K::new();
    faulted(&mut k);
    let with_caps = Syscall::Send {
        target: s(RP),
        message: Message {
            caps: vec![Transfer::Derive(s(EP), Rights::ALL.bits())],
            ..Message::default()
        },
    };
    let dropped = Header::FLAG_CAPS_DROPPED;
    assert_eq!(k.go(with_caps), Ok(Output::Sent { flags: dropped }));
}

#[test]
fn thread_suspended_while_it_waits_gets_the_effect_and_runs_when_resumed() {
    let mut k = K::new();
    let thread = faulted(&mut k);
    k.ok(Syscall::ThreadSuspend(s(40)));
    k.go(answer(1, vec![0x40_3000, 0x9000])).unwrap();
    assert!(!k.can_run(thread));
    assert_eq!(k.regs(40)[REG_RIP], 0x40_3000);
    k.ok(Syscall::ThreadResume(s(40)));
    assert!(k.can_run(thread));
}

#[test]
fn fault_wait_that_ends_without_an_answer_leaves_the_thread_suspended() {
    // The reply object destroyed.
    let mut k = K::new();
    let thread = faulted(&mut k);
    k.ok(Syscall::CapDelete(s(RP)));
    assert!(!k.can_run(thread));
    assert_eq!(k.m.census().faulting, 0);
    // The endpoint destroyed while the thread is queued.
    let mut k = K::new();
    let thread = k.running(40, EP);
    k.fault(thread, FaultKind::Debug);
    k.ok(Syscall::CapDelete(s(EP)));
    assert!(!k.can_run(thread));
    assert_eq!(k.m.woken(thread), None);
}

#[test]
fn thread_cancel_ends_a_fault_wait_only_with_wait_number_0() {
    let mut k = K::new();
    let thread = faulted(&mut k);
    let cancel = |number| Syscall::ThreadCancel(s(40), number);
    k.ok(cancel(5));
    assert_eq!(k.m.census().faulting, 1);
    k.ok(cancel(0));
    assert_eq!(k.m.census().faulting, 0);
    assert!(!k.can_run(thread));
    assert_eq!(k.m.woken(thread), None);
    // The late answer is dropped.
    assert_eq!(k.go(answer(0, vec![])), Err(Error::WouldWait));
}

#[test]
fn thread_that_waits_on_a_fault_is_busy_for_thread_configure() {
    let mut k = K::new();
    faulted(&mut k);
    k.ok(Syscall::ThreadSuspend(s(40)));
    let configure = Syscall::ThreadConfigure {
        thread: s(40),
        cspace: s(START_SLOT_CSPACE),
        aspace: SlotAddr::NONE,
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    };
    assert_eq!(k.go(configure), Err(Error::Busy));
}

// Register calls.

#[test]
fn register_block_of_a_new_thread_is_zero_but_the_flags() {
    let mut k = K::new();
    k.thread(40, 0, false);
    let mut zero = vec![0; REGS_WORDS];
    zero[REG_RFLAGS] = 0x202;
    assert_eq!(k.regs(40), zero);
}

#[test]
fn first_thread_starts_at_the_entry_point_with_the_start_up_page() {
    let mut k = K::new();
    let other = k.running(40, 0);
    let first = START_SLOT_THREAD;
    assert_eq!(k.on(other, Syscall::ThreadSuspend(s(first))), DONE);
    let Ok(Output::Regs(block)) = k.on(other, Syscall::ThreadReadRegs(s(first))) else {
        panic!("no registers");
    };
    assert_eq!(block[REG_RIP], ENTRY);
    assert_eq!(block[REG_RDI], x86_64::START_PAGE_ADDR);
    assert_eq!(block[REG_RFLAGS], 0x202);
    assert_eq!(block[REG_RSP], 0);
}

#[test]
fn thread_read_regs_needs_read_and_a_suspended_thread() {
    let mut k = K::new();
    k.thread(40, 0, false);
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(42),
        rights: Rights::WRITE.bits(),
        badge: 0,
    });
    assert_eq!(k.go(Syscall::ThreadReadRegs(s(42))), Err(Error::Denied));
    k.ok(Syscall::ThreadResume(s(40)));
    assert_eq!(k.go(Syscall::ThreadReadRegs(s(40))), Err(Error::Busy));
    // A thread never reads its own registers.
    let own = Syscall::ThreadReadRegs(s(START_SLOT_THREAD));
    assert_eq!(k.go(own), Err(Error::Busy));
}

#[test]
fn waiting_thread_can_be_read_but_not_written() {
    let mut k = K::new();
    let thread = k.running(40, 0);
    assert_eq!(k.on(thread, recv()), Ok(Output::Waits));
    k.ok(Syscall::ThreadSuspend(s(40)));
    let block = k.regs(40);
    let write = Syscall::ThreadWriteRegs {
        thread: s(40),
        block,
    };
    assert_eq!(k.go(write), Err(Error::Busy));
}

#[test]
fn register_calls_from_a_thread_without_a_message_page_are_invalid_after_the_thread() {
    let mut k = K::new();
    let caller = k.thread(44, 0, false);
    k.ok(Syscall::ThreadResume(s(44)));
    k.thread(40, 0, false);
    let read = |slot| Syscall::ThreadReadRegs(s(slot));
    assert_eq!(k.on(caller, read(40)), Err(Error::Invalid));
    assert_eq!(k.on(caller, read(EP)), Err(Error::Invalid));
    let write = Syscall::ThreadWriteRegs {
        thread: s(40),
        block: vec![0; 3],
    };
    assert_eq!(k.on(caller, write), Err(Error::Invalid));
    // The thread is judged first: an empty slot is `invalid` too, a dead
    // one `gone`.
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(42),
        rights: Rights::ALL.bits(),
        badge: 0,
    });
    k.ok(Syscall::CapRevoke(s(40)));
    assert_eq!(k.on(caller, read(42)), Err(Error::Gone));
}

#[test]
fn thread_write_regs_checks_the_size_then_the_canonical_words_then_the_state() {
    let mut k = K::new();
    k.thread(40, 0, false);
    let write = |block: Vec<u64>| Syscall::ThreadWriteRegs {
        thread: s(40),
        block,
    };
    let good = k.regs(40);
    let mut long = good.clone();
    long.push(0);
    assert_eq!(k.go(write(long)), Err(Error::Invalid));
    assert_eq!(k.go(write(good[..19].to_vec())), Err(Error::Invalid));
    for word in [REG_RIP, REG_RSP, 18, 19] {
        let mut wrong = good.clone();
        wrong[word] = 0x8000_0000_0000;
        assert_eq!(k.go(write(wrong)), Err(Error::Invalid), "word {word}");
    }
    // Other words may hold anything.
    let mut free = good.clone();
    free[REG_RAX] = u64::MAX;
    k.ok(write(free.clone()));
    // The size before the state.
    k.ok(Syscall::ThreadResume(s(40)));
    assert_eq!(k.go(write(good[..3].to_vec())), Err(Error::Invalid));
    assert_eq!(k.go(write(free)), Err(Error::Busy));
}

#[test]
fn thread_write_regs_needs_write_and_forces_the_fixed_bits_of_the_flags() {
    let mut k = K::new();
    k.thread(40, 0, false);
    k.ok(Syscall::CapDerive {
        source: s(40),
        destination: s(42),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let mut block = k.regs(40);
    block[REG_RFLAGS] = u64::MAX;
    let denied = Syscall::ThreadWriteRegs {
        thread: s(42),
        block: block.clone(),
    };
    assert_eq!(k.go(denied), Err(Error::Denied));
    k.ok(Syscall::ThreadWriteRegs {
        thread: s(40),
        block,
    });
    assert_eq!(k.regs(40)[REG_RFLAGS], 0x0024_0FD7);
}
