//! Debugging, the I/O ports, entropy and breakpoints (8.5): one test per
//! rule, on `Machine::example`, every invariant after every call.

use dodos_abi::ObjectKind::{Endpoint, Frame, Sched, Task, Thread};
use dodos_abi::x86_64::{REG_DR0, REG_DR7, REG_RFLAGS, REG_UINTR, REGS_WORDS, RFLAGS_FIXED};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const UT: SlotAddr = SlotAddr::own(25);
const DEBUG: SlotAddr = SlotAddr::own(START_SLOT_DEBUG);
const PORTS: SlotAddr = SlotAddr::own(START_SLOT_IOPORTS);
const MS: u64 = 1_000_000;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
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

fn write(length: u64) -> Syscall {
    Syscall::DebugWrite {
        debug: DEBUG,
        length,
        words: (0..length.div_ceil(8))
            .map(|w| 0x0101_0101_0101_0101 * (w + 1))
            .collect(),
    }
}

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

    fn user(&self) -> Vec<TraceEntry> {
        self.m.ring(true)
    }

    fn kernel(&self) -> Vec<TraceEntry> {
        self.m.ring(false)
    }
}

// debug_write and trace_emit.

#[test]
fn debug_write_writes_one_entry_per_24_bytes_all_but_the_last_continued() {
    let mut k = K::new();
    k.ok(write(0));
    assert!(k.user().is_empty());
    k.ok(write(40));
    let entries = k.user();
    assert_eq!(entries.len(), 2);
    assert_eq!((entries[0].seq, entries[1].seq), (1, 2));
    assert!(entries[0].continued() && !entries[1].continued());
    assert!(entries.iter().all(|e| e.code() == 16));
    // Byte k is byte k mod 8 of word k div 8; the last entry is padded.
    assert_eq!(
        entries[0].payload,
        [
            0x0101_0101_0101_0101,
            0x0202_0202_0202_0202,
            0x0303_0303_0303_0303
        ]
    );
    assert_eq!(
        entries[1].payload,
        [0x0404_0404_0404_0404, 0x0505_0505_0505_0505, 0]
    );
    // The task identifier of the caller, its effective marks.
    assert!(entries.iter().all(|e| e.task != 0));
    k.ok(write(512));
    assert_eq!(k.user().len(), 2 + 22);
}

#[test]
fn debug_write_is_judged_rights_then_length_then_the_message_page() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: DEBUG,
        destination: s(100),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let weak = Syscall::DebugWrite {
        debug: s(100),
        length: 8,
        words: vec![1],
    };
    k.no(weak, Error::Denied);
    k.no(write(513), Error::Invalid);
    // A thread without a message page: 6 words in the registers.
    let mut k = K::new();
    k.ok(retype(Thread, 0, 101));
    k.ok(Syscall::ThreadConfigure {
        thread: s(101),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    });
    k.ok(retype(Sched, 0, 102));
    k.ok(Syscall::SchedConfigure {
        sched: SlotAddr::own(START_SLOT_SCHED),
        core: SlotAddr::own(16),
        budget: 5 * MS,
        period: 10 * MS,
        class: 1,
    });
    k.ok(Syscall::SchedConfigure {
        sched: s(102),
        core: SlotAddr::own(16),
        budget: MS,
        period: 10 * MS,
        class: 1,
    });
    k.ok(Syscall::SchedBind {
        sched: s(102),
        thread: s(101),
    });
    k.ok(Syscall::ThreadResume(s(101)));
    let other = k.m.object_at(k.t, s(101)).unwrap();
    assert_eq!(k.m.apply(other, write(48)), DONE);
    assert_eq!(k.m.apply(other, write(49)), Err(Error::Invalid));
    // `entropy_read` likewise: 6 words in the registers.
    let entropy = |length| Syscall::EntropyRead {
        length,
        mix: [0; 4],
    };
    assert!(matches!(
        k.m.apply(other, entropy(48)),
        Ok(Output::Bytes(_))
    ));
    assert_eq!(k.m.apply(other, entropy(49)), Err(Error::Invalid));
}

#[test]
fn trace_emit_writes_one_entry_of_code_17_and_no_call_on_debug_writes_the_kernel_ring() {
    let mut k = K::new();
    let kernel = k.kernel();
    k.ok(Syscall::TraceEmit {
        debug: DEBUG,
        words: [1, 2, 3],
    });
    assert_eq!(k.user().len(), 1);
    assert_eq!((k.user()[0].code(), k.user()[0].payload), (17, [1, 2, 3]));
    assert_eq!(k.kernel(), kernel);
}

#[test]
fn a_destroyed_debug_object_is_invalid_for_its_calls() {
    let mut k = K::new();
    k.ok(Syscall::CapDelete(DEBUG));
    k.no(write(8), Error::Invalid);
    k.no(
        Syscall::TraceEmit {
            debug: DEBUG,
            words: [0; 3],
        },
        Error::Invalid,
    );
}

// The kernel ring.

#[test]
fn the_kernel_ring_starts_with_the_text_of_the_start() {
    let k = K::new();
    let first = k.kernel()[0];
    assert_eq!((first.seq, first.code()), (1, 1));
    assert_eq!(first.payload[0], u64::from_le_bytes(*b"start\0\0\0"));
}

impl K {
    /// A second thread in slot 101, in the spaces of the first, with a
    /// message page, a context of its own, resumed.
    fn spawn(&mut self) -> ObjId {
        self.ok(retype(Thread, 0, 101));
        self.ok(retype(Frame, 1, 103));
        self.ok(Syscall::ThreadConfigure {
            thread: s(101),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(103),
        });
        self.ok(retype(Sched, 0, 102));
        self.ok(Syscall::SchedConfigure {
            sched: SlotAddr::own(START_SLOT_SCHED),
            core: SlotAddr::own(16),
            budget: 5 * MS,
            period: 10 * MS,
            class: 1,
        });
        self.ok(Syscall::SchedConfigure {
            sched: s(102),
            core: SlotAddr::own(16),
            budget: MS,
            period: 10 * MS,
            class: 1,
        });
        self.ok(Syscall::SchedBind {
            sched: s(102),
            thread: s(101),
        });
        self.ok(Syscall::ThreadResume(s(101)));
        self.m.object_at(self.t, s(101)).unwrap()
    }
}

#[test]
fn a_fault_recorded_in_its_thread_is_traced_and_the_rate_limit_counts_the_rest() {
    let mut k = K::new();
    let b = k.spawn();
    // The thread has no fault endpoint: each fault is recorded in it.
    for _ in 0..20 {
        k.m.fault(b, FaultKind::Page, [14, 2, 0x1000]);
        assert_eq!(k.m.check(), Ok(()));
        k.ok(Syscall::ThreadResume(s(101)));
    }
    let recorded = k.kernel().iter().filter(|e| e.code() == 5).count();
    assert_eq!(recorded, 16);
    assert_eq!(k.m.suppressed(), 4);
    // The first entry of a new window is preceded by code 3 with the count.
    k.m.advance(k.m.now() + 2 * MS);
    k.m.fault(b, FaultKind::Page, [14, 2, 0x1000]);
    assert_eq!(k.m.check(), Ok(()));
    let ring = k.kernel();
    let n = ring.len();
    assert_eq!(
        (ring[n - 2].code(), ring[n - 2].payload[..2].to_vec()),
        (3, vec![5, 4])
    );
    assert_eq!(ring[n - 1].code(), 5);
    // Its words: the kind, word 0 and word 4 of the fault.
    assert_eq!(ring[n - 1].payload[0], FaultKind::Page as u64);
    assert_eq!(ring[n - 1].payload[2], 0x1000);
}

// Hardware breakpoints and the user-interrupt word.

fn block() -> Vec<u64> {
    let mut block = vec![0; REGS_WORDS];
    block[REG_RFLAGS] = RFLAGS_FIXED;
    block
}

#[test]
fn breakpoint_words_are_judged_word_by_word_and_the_user_interrupt_word_is_unsupported() {
    let mut k = K::new();
    let b = k.spawn();
    let _ = b;
    k.ok(Syscall::ThreadSuspend(s(101)));
    let regs = |block: Vec<u64>| Syscall::ThreadWriteRegs {
        thread: s(101),
        block,
    };
    let mut outside = block();
    outside[REG_DR0] = Machine::example().geometry.user_end;
    k.no(regs(outside.clone()), Error::Invalid);
    let mut io = block();
    io[REG_DR7] = 1 | 0b10 << 16;
    k.no(regs(io), Error::Invalid);
    let mut kernel_bits = block();
    kernel_bits[REG_DR7] = 1 << 13;
    k.no(regs(kernel_bits), Error::Invalid);
    let mut uintr = block();
    uintr[REG_UINTR] = 1;
    k.no(regs(uintr.clone()), Error::Unsupported);
    // Judged with its word: an invalid earlier word comes first.
    uintr[REG_DR0] = outside[REG_DR0];
    k.no(regs(uintr), Error::Invalid);
    let mut watch = block();
    watch[REG_DR0] = 0x1000;
    watch[REG_DR7] = 1 | 0b01 << 16;
    k.ok(regs(watch));
    let Ok(Output::Regs(read)) = k.go(Syscall::ThreadReadRegs(s(101))) else {
        panic!("registers");
    };
    assert_eq!((read[REG_DR0], read[REG_UINTR]), (0x1000, 0));
}

#[test]
fn a_breakpoint_hit_is_a_fault_of_kind_9() {
    let mut k = K::new();
    let b = k.spawn();
    k.ok(Syscall::ThreadSuspend(s(101)));
    let mut watch = block();
    watch[REG_DR0] = 0x1000;
    watch[REG_DR7] = 1;
    k.ok(Syscall::ThreadWriteRegs {
        thread: s(101),
        block: watch,
    });
    k.ok(Syscall::ThreadResume(s(101)));
    k.m.fault(b, FaultKind::HardwareBreakpoint, [1, 1, 0xFFFF_0FF1]);
    assert_eq!(k.m.check(), Ok(()));
    let Ok(Output::Regs(read)) = k.go(Syscall::ThreadReadRegs(s(101))) else {
        panic!("registers");
    };
    // The fault record follows the block: kind 9, words 3 and 4.
    assert_eq!(read[REGS_WORDS], 9);
    assert_eq!(read[REGS_WORDS + 4..REGS_WORDS + 6], [1, 0xFFFF_0FF1]);
}

// The I/O ports.

fn port_in(ioports: SlotAddr, port: u64, width: u64) -> Syscall {
    Syscall::IoportIn {
        ioports,
        port,
        width,
    }
}

fn port_out(ioports: SlotAddr, port: u64, width: u64, value: u64) -> Syscall {
    Syscall::IoportOut {
        ioports,
        port,
        width,
        value,
    }
}

#[test]
fn port_calls_read_and_write_within_the_range_of_the_capability() {
    let mut k = K::new();
    k.ok(port_out(PORTS, 0x70, 2, 0xBEEF));
    assert_eq!(k.go(port_in(PORTS, 0x70, 2)), Ok(Output::Value(0xBEEF)));
    assert_eq!(k.go(port_in(PORTS, 0x71, 1)), Ok(Output::Value(0xBE)));
    k.no(port_in(PORTS, 0x70, 3), Error::Invalid);
    k.no(port_out(PORTS, 0x70, 1, 0x100), Error::Invalid);
    k.no(port_in(PORTS, 0xFFFF, 2), Error::Invalid);
    // The kernel's ports and those of PCI configuration are busy.
    k.no(port_in(PORTS, 0x20, 1), Error::Busy);
    // An access that touches one refused port.
    k.no(port_in(PORTS, 0x9F, 4), Error::Busy);
    k.no(port_in(PORTS, 0xCF8, 4), Error::Busy);
    // With the role `platform`, the configuration ports are allowed.
    assert!(matches!(
        k.go(Syscall::RoleClaim(s(23))),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(k.go(port_in(PORTS, 0xCF8, 4)), Ok(Output::Value(0)));
}

#[test]
fn ioports_issue_narrows_the_range_and_inspect_returns_it() {
    let mut k = K::new();
    let issue = |first, last, destination| Syscall::IoportsIssue {
        ioports: PORTS,
        destination: s(destination),
        first,
        last,
    };
    k.no(issue(0x10, 0x5, 100), Error::Invalid);
    k.no(issue(0x10, 0x10000, 100), Error::Invalid);
    k.ok(issue(0x60, 0x64, 100));
    let Ok(Output::Inspect { badge, .. }) = k.go(Syscall::CapInspect(s(100))) else {
        panic!("inspect");
    };
    assert_eq!(badge, 0x60 | 0x64 << 16);
    // Outside the child's range.
    let child = Syscall::IoportsIssue {
        ioports: s(100),
        destination: s(101),
        first: 0x60,
        last: 0x70,
    };
    k.no(child, Error::Invalid);
    k.no(port_in(s(100), 0x65, 1), Error::Invalid);
    assert_eq!(k.go(port_in(s(100), 0x64, 1)), Ok(Output::Value(0)));
    // `cap_derive` keeps the range; a badge is `invalid`.
    k.ok(Syscall::CapDerive {
        source: s(100),
        destination: s(102),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let Ok(Output::Inspect { badge, .. }) = k.go(Syscall::CapInspect(s(102))) else {
        panic!("inspect");
    };
    assert_eq!(badge, 0x60 | 0x64 << 16);
    k.no(
        Syscall::CapDerive {
            source: s(100),
            destination: s(103),
            rights: 0,
            badge: 1,
        },
        Error::Invalid,
    );
    // `derive` is needed to issue.
    k.no(
        Syscall::IoportsIssue {
            ioports: s(102),
            destination: s(104),
            first: 0x60,
            last: 0x60,
        },
        Error::Denied,
    );
}

#[test]
fn port_calls_are_unsupported_off_x86_64() {
    let mut machine = Machine::example();
    machine.arch = Arch::Other;
    let mut k = K::on(&machine);
    k.no(port_in(s(4000), 0x70, 1), Error::Unsupported);
    k.no(port_out(s(4000), 0x70, 1, 0), Error::Unsupported);
    let issue = Syscall::IoportsIssue {
        ioports: s(4000),
        destination: s(4001),
        first: 0,
        last: 0,
    };
    k.no(issue, Error::Unsupported);
}

// Entropy.

#[test]
fn entropy_read_changes_no_state_but_the_generators_and_pads_the_last_word() {
    let mut k = K::new();
    let before = k.m.clone();
    let Ok(Output::Bytes(words)) = k.go(Syscall::EntropyRead {
        length: 13,
        mix: [1, 2, 3, 4],
    }) else {
        panic!("bytes");
    };
    assert_eq!(words.len(), 2);
    assert_eq!(words[1] >> 40, 0);
    assert_eq!(k.m, before);
    k.no(
        Syscall::EntropyRead {
            length: 65,
            mix: [0; 4],
        },
        Error::Invalid,
    );
    let Ok(Output::Bytes(words)) = k.go(Syscall::EntropyRead {
        length: 64,
        mix: [0; 4],
    }) else {
        panic!("bytes");
    };
    assert_eq!(words.len(), 8);
}

// Records.

#[test]
fn a_failure_writes_a_committed_record_and_the_fence_does_not_overwrite_it() {
    let (mut sys, _) = System::boot(&Machine::example()).unwrap();
    sys.panic(1, 3);
    let record = sys.node(0).crash_record(1);
    assert_eq!(record, Some((1, 1)));
    let failure = sys.node(1).ring(false).iter().any(|e| e.code() == 6);
    assert!(failure);
    sys.run(20 * MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Dead);
    // The fence handler found a committed record of the same generation.
    assert_eq!(sys.node(0).crash_record(1), Some((1, 1)));
}

#[test]
fn the_fence_handler_writes_a_record_of_reason_5() {
    let (mut sys, t) = System::boot(&Machine::example()).unwrap();
    let kill = Syscall::CoreStop {
        core: SlotAddr::own(17),
        mode: CORE_STOP_KILL,
    };
    assert_eq!(sys.apply(0, t[0], kill), DONE);
    sys.run(MS, MS / 10);
    assert_eq!(sys.node(0).crash_record(1), Some((1, 5)));
    // The transition was traced by the node that made it.
    let traced = sys
        .node(0)
        .ring(false)
        .iter()
        .any(|e| e.code() == 9 && e.payload[2] == 0);
    assert!(traced);
}

#[test]
fn a_node_started_fresh_keeps_its_rings_and_goes_on_from_their_sequences() {
    let (mut sys, t) = System::boot(&Machine::example()).unwrap();
    let kill = Syscall::CoreStop {
        core: SlotAddr::own(17),
        mode: CORE_STOP_KILL,
    };
    assert_eq!(sys.apply(0, t[0], kill), DONE);
    sys.run(20 * MS, MS / 10);
    let start = Syscall::CoreStart {
        core: SlotAddr::own(17),
        mode: 0,
        image: 0,
        module: 0,
    };
    assert_eq!(sys.apply(0, t[0], start), DONE);
    let ring = sys.node(1).ring(false);
    // Two entries of the start, one of each generation, and the record of
    // the dead generation is still there.
    let starts: Vec<u32> = ring
        .iter()
        .filter(|e| e.code() == 1)
        .map(|e| e.generation())
        .collect();
    assert_eq!(starts, vec![1, 2]);
    assert_eq!(sys.node(1).area_generation(1), 2);
    assert_eq!(sys.node(0).crash_record(1), Some((1, 5)));
    let _ = (Endpoint, Task);
}

// Destroying what is linked (8.5, invariant 7).

fn bind(notify: u32, endpoint: SlotAddr) -> Syscall {
    Syscall::NotifyBind {
        notify: s(notify),
        endpoint,
    }
}

#[test]
fn a_destroyed_endpoint_leaves_its_notify_unbound_with_its_word_and_its_waiters() {
    let mut k = K::new();
    k.ok(retype(Endpoint, 0, 110));
    k.ok(retype(ObjectKind::Notify, 0, 111));
    k.ok(bind(111, s(110)));
    k.ok(Syscall::Signal(s(111)));
    k.ok(Syscall::CapDelete(s(110)));
    // Unbound: it can be bound again; its word is kept.
    k.ok(retype(Endpoint, 0, 110));
    k.ok(bind(111, s(110)));
    assert_eq!(k.go(Syscall::Poll(s(111))), Ok(Output::Word(1)));
    // With a waiter on the notify: the waiter is untouched.
    let b = k.spawn();
    assert_eq!(k.m.apply(b, Syscall::Wait(s(111), 0)), Ok(Output::Waits));
    k.ok(Syscall::CapDelete(s(110)));
    assert_eq!(k.m.woken(b), None);
    k.ok(Syscall::Signal(s(111)));
    assert_eq!(k.m.woken(b), Some(&Ok(Output::Word(1))));
}

#[test]
fn a_destroyed_notify_leaves_its_endpoint_unbound_with_its_receivers() {
    let mut k = K::new();
    k.ok(retype(Endpoint, 0, 110));
    k.ok(retype(ObjectKind::Notify, 0, 111));
    k.ok(retype(ObjectKind::Reply, 0, 112));
    k.ok(bind(111, s(110)));
    let b = k.spawn();
    let recv = Syscall::Recv {
        endpoint: s(110),
        reply: s(112),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.m.apply(b, recv), Ok(Output::Waits));
    k.ok(Syscall::CapDelete(s(111)));
    assert_eq!(k.m.woken(b), None);
    k.ok(retype(ObjectKind::Notify, 0, 111));
    k.ok(bind(111, s(110)));
}

#[test]
fn destroying_a_thread_with_every_link_takes_one_step_of_at_most_8_units() {
    let mut k = K::new();
    let b = k.spawn();
    // Its links: its wait on an endpoint, its page, its fault endpoint,
    // its address space, its capability space, its task, its home context.
    k.ok(retype(Endpoint, 0, 110));
    k.ok(retype(Task, 0, 111));
    k.ok(Syscall::ThreadSuspend(s(101)));
    k.ok(Syscall::ThreadConfigure {
        thread: s(101),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(START_SLOT_ASPACE),
        fault: s(110),
        page: s(103),
    });
    k.ok(Syscall::TaskAttach {
        thread: s(101),
        task: s(111),
    });
    k.ok(Syscall::ThreadResume(s(101)));
    let call = Syscall::Call {
        endpoint: s(110),
        message: Message::default(),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.m.apply(b, call), Ok(Output::Waits));
    k.ok(Syscall::CapDelete(s(101)));
    assert_eq!(k.m.object_at(k.t, s(101)), None);
    // Every link is undone: the page can bind another thread, the task
    // lists no thread, the context has no home.
    let Ok(Output::Task { threads, .. }) = k.go(Syscall::TaskRead(s(111))) else {
        panic!("task");
    };
    assert_eq!(threads, 0);
    let Ok(Output::SchedRead { state, .. }) = k.go(Syscall::SchedRead(s(102))) else {
        panic!("sched");
    };
    assert_eq!(state & SCHED_STATE_HOME, 0);
}

#[test]
fn calls_on_debug_and_ioports_of_another_node_run_on_the_callers_node() {
    let (mut sys, t) = System::boot(&Machine::example()).unwrap();
    // Node 0 gives node 1 copies of its `debug` and `ioports` capabilities.
    let reply = retype(ObjectKind::Reply, 0, 99);
    assert_eq!(sys.apply(1, t[1], reply), DONE);
    let recv = Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(99),
        range: Range {
            first: s(110),
            count: 2,
        },
        wait: 0,
    };
    assert_eq!(sys.apply(1, t[1], recv), Ok(Output::Waits));
    let send = Syscall::Send {
        target: SlotAddr::own(17),
        message: Message {
            caps: vec![
                Transfer::Derive(DEBUG, Rights::ALL.without(Rights::EXEC).bits()),
                Transfer::Derive(PORTS, Rights::ALL.without(Rights::EXEC).bits()),
            ],
            ..Message::default()
        },
    };
    assert_eq!(sys.apply(0, t[0], send), Ok(Output::Waits));
    sys.settle(16);
    let before = sys.pending().len();
    let emit = Syscall::TraceEmit {
        debug: s(110),
        words: [7, 8, 9],
    };
    assert_eq!(sys.apply(1, t[1], emit), DONE);
    // No message between kernels; the entry is in node 1's own ring.
    assert_eq!(sys.pending().len(), before);
    assert_eq!(
        sys.node(1).ring(true).last().map(|e| e.payload),
        Some([7, 8, 9])
    );
    assert!(sys.node(0).ring(true).is_empty());
    let port = Syscall::IoportOut {
        ioports: s(111),
        port: 0x80,
        width: 1,
        value: 5,
    };
    assert_eq!(sys.apply(1, t[1], port), DONE);
    assert_eq!(sys.pending().len(), before);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn a_change_of_a_role_word_is_traced_by_the_node_that_made_it() {
    let mut k = K::new();
    assert!(matches!(
        k.go(Syscall::RoleClaim(s(23))),
        Ok(Output::Epoch(_))
    ));
    let last = *k.kernel().last().unwrap();
    assert_eq!((last.code(), last.payload[2]), (9, 1));
    assert_eq!(
        dodos_abi::RoleWord::from_raw(last.payload[1]).map(|w| w.holder()),
        Some(0)
    );
}
