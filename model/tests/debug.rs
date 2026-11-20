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
/// An address no space of the node translates: a page fault on a translated
/// address with the access asked is spurious and sends nothing (D-129).
const UNMAPPED: u64 = 0x10_0000_0000;

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

/// Rights before the length: a weak capability with a length too long is
/// `denied`; a length beyond the registers is refused without a message
/// page. (Length before the message page is not visible: both refusals are
/// `invalid`; the final review's F2 Opus 14.)
#[test]
fn debug_write_judges_rights_before_length_and_the_registers_without_a_page() {
    let mut k = K::new();
    k.ok(Syscall::CapDerive {
        source: DEBUG,
        destination: s(100),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let weak = Syscall::DebugWrite {
        debug: s(100),
        length: 513,
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
        node: SlotAddr::own(16),
        core: 0,
        budget: 5 * MS,
        period: 10 * MS,
        class: 1,
    });
    k.ok(Syscall::SchedConfigure {
        sched: s(102),
        node: SlotAddr::own(16),
        core: 0,
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
    // Each call of the second thread checked after it (the audit of lot
    // 8b, D-8b F22: these four were not).
    let mut by_other = |call| {
        let outcome = k.m.apply(other, call);
        assert_eq!(k.m.check(), Ok(()));
        outcome
    };
    assert_eq!(by_other(write(48)), DONE);
    assert_eq!(by_other(write(49)), Err(Error::Invalid));
    // `entropy_read` likewise: 6 words in the registers.
    let entropy = |length| Syscall::EntropyRead {
        length,
        mix: [0; 4],
    };
    assert!(matches!(by_other(entropy(48)), Ok(Output::Bytes(_))));
    assert_eq!(by_other(entropy(49)), Err(Error::Invalid));
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
            node: SlotAddr::own(16),
            core: 0,
            budget: 5 * MS,
            period: 10 * MS,
            class: 1,
        });
        self.ok(Syscall::SchedConfigure {
            sched: s(102),
            node: SlotAddr::own(16),
            core: 0,
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
        k.m.fault(b, FaultKind::Page, [14, 2, UNMAPPED]);
        assert_eq!(k.m.check(), Ok(()));
        k.ok(Syscall::ThreadResume(s(101)));
    }
    let recorded = k.kernel().iter().filter(|e| e.code() == 5).count();
    assert_eq!(recorded, 16);
    assert_eq!(k.m.suppressed(), 4);
    // The first entry of a new window is preceded by code 3 with the count.
    k.m.advance(k.m.now() + 2 * MS);
    k.m.fault(b, FaultKind::Page, [14, 2, UNMAPPED]);
    assert_eq!(k.m.check(), Ok(()));
    let ring = k.kernel();
    let n = ring.len();
    assert_eq!(
        (ring[n - 2].code(), ring[n - 2].payload[..2].to_vec()),
        (3, vec![5, 4])
    );
    assert_eq!(ring[n - 1].code(), 5);
    // Its words: the kind with the exception number in its high half,
    // word 0 and word 4 of the fault.
    assert_eq!(ring[n - 1].payload[0], 14 << 32 | FaultKind::Page as u64);
    assert_eq!(ring[n - 1].payload[2], UNMAPPED);
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
    k.spawn();
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

/// x86-64, the gate's `#DB` stub (D-143, D-155, D-156 F4), as the model
/// holds it: the stub merges the causes of DR6 into the core's deferred
/// word and loads DR6's reset value; the kernel consumes the word whole.
/// A breakpoint of the thread hit on the kernel's entry of a fault is the
/// thread's kind 9, kept behind that fault and judged against the control
/// word at the `#DB`: a debugger that clears that word before kind 9 is
/// raised does not take it back. The next `#DB` reads only its own cause.
/// (The audit of lot 8b, F5: no test of `tests/` went through the stub;
/// the model has no `deferred_events` word, and no bit of it.)
#[test]
fn a_db_at_the_entry_defers_dr6_and_its_kind_9_is_judged_at_the_db() {
    const RESET: u64 = 0xFFFF_0FF0;
    const B0: u64 = 1;
    const BS: u64 = 1 << 14;
    let mut k = K::new();
    let mut watch = block();
    watch[REG_DR0] = 0x1000;
    watch[REG_DR7] = 1;
    let b = k.spawn();
    k.ok(Syscall::ThreadSuspend(s(101)));
    let write = |block| Syscall::ThreadWriteRegs {
        thread: s(101),
        block,
    };
    k.ok(write(watch.clone()));
    k.ok(Syscall::ThreadResume(s(101)));
    // A `MOV SS` before an undefined instruction: the `#DB` on the
    // kernel's entry of that fault, which goes first and suspends the
    // thread (it has no fault endpoint); kind 9 is kept behind it.
    let undefined = FaultKind::Undefined;
    assert!(k.m.fault_with_debug(b, B0, undefined, [6, 0, 0]));
    assert_eq!(k.m.check(), Ok(()));
    // The debugger clears the control word, then resumes the thread.
    watch[REG_DR7] = 0;
    k.ok(write(watch.clone()));
    k.ok(Syscall::ThreadResume(s(101)));
    let Ok(Output::Regs(read)) = k.go(Syscall::ThreadReadRegs(s(101))) else {
        panic!("registers");
    };
    // Raised all the same: word 3 the hit judged at the `#DB`, word 4 the
    // value the stub read.
    assert_eq!(read[REGS_WORDS], 9);
    assert_eq!(read[REGS_WORDS + 4..REGS_WORDS + 6], [B0, RESET | B0]);
    // Breakpoint 0 enabled again; the next `#DB`, a single step on the
    // system-call entry, reads only its own cause (the stub reset DR6):
    // ignored, and no kind 9 from a B0 left over.
    watch[REG_DR7] = 1;
    k.ok(write(watch));
    k.ok(Syscall::ThreadResume(s(101)));
    assert_eq!(k.m.call_with_debug(b, BS, Syscall::Yield), Some(DONE));
    assert_eq!(k.m.check(), Ok(()));
    k.ok(Syscall::ThreadSuspend(s(101)));
    let Ok(Output::Regs(read)) = k.go(Syscall::ThreadReadRegs(s(101))) else {
        panic!("registers");
    };
    assert_eq!(read[REGS_WORDS], 0, "a fault recorded");
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
    machine.arch = Arch::Aarch64;
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

/// The words it returns: ⌈length / 8⌉, the last one padded with zeros. (It
/// changes no state but the generator's: `debug.rs`'s unit test, which can
/// read the generator.)
#[test]
fn entropy_read_fills_its_words_and_pads_the_last_one() {
    let mut k = K::new();
    let Ok(Output::Bytes(words)) = k.go(Syscall::EntropyRead {
        length: 13,
        mix: [1, 2, 3, 4],
    }) else {
        panic!("bytes");
    };
    assert_eq!(words.len(), 2);
    assert_eq!(words[1] >> 40, 0);
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
    // Node 1 finds a failure in itself on its core (core number 1).
    sys.fail(1, 0);
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
    let kill = Syscall::NodeStop {
        node: SlotAddr::own(17),
        mode: NODE_STOP_KILL,
        core: 0,
    };
    // A kill comes from a node that holds a reserve role (D-150).
    let claim = sys.apply(0, t[0], Syscall::RoleClaim(SlotAddr::own(21)));
    assert!(matches!(claim, Ok(Output::Epoch(_))));
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
    let kill = Syscall::NodeStop {
        node: SlotAddr::own(17),
        mode: NODE_STOP_KILL,
        core: 0,
    };
    // A kill comes from a node that holds a reserve role (D-150).
    let claim = sys.apply(0, t[0], Syscall::RoleClaim(SlotAddr::own(21)));
    assert!(matches!(claim, Ok(Output::Epoch(_))));
    assert_eq!(sys.apply(0, t[0], kill), DONE);
    sys.run(20 * MS, MS / 10);
    let start = Syscall::NodeStart {
        node: SlotAddr::own(17),
        mode: 0,
        image: 0,
        module: 0,
        window: 0,
        mask: 1 << 1,
    };
    assert_eq!(sys.apply(0, t[0], start), DONE);
    sys.run(sys.now() + MS, MS / 10);
    let ring = sys.node(1).ring(false);
    // Two entries of the start, one of each generation, and the record of
    // the dead generation is still there.
    let starts: Vec<u32> = ring
        .iter()
        .filter(|e| e.code() == 1)
        .map(|e| e.generation())
        .collect();
    assert_eq!(starts, vec![1, 2]);
    assert_eq!(sys.node(1).area_generation(1), (2, 1));
    assert_eq!(sys.node(0).crash_record(1), Some((1, 5)));
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

/// A thread with every link is destroyed in one call (`done`), each link
/// undone; one step holds up to `REMOVAL_STEP` units, which the test does
/// not count (the final review's F2 Opus 11: the name said 8).
#[test]
fn destroying_a_thread_with_every_link_finishes_in_one_step() {
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
    // The page binds another thread.
    k.ok(retype(Thread, 0, 112));
    k.ok(Syscall::ThreadConfigure {
        thread: s(112),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: s(103),
    });
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

/// Argon F3: the window of the rate limit at its boundary, the order of the
/// checks of the debug, port and entropy calls, the refused console port, and
/// the words entropy_read mixes (mutants 54, 59, 63, 66, 68, 80-87, 96-99, 107).
#[test]
fn debug_port_and_entropy_rules_that_no_test_pinned() {
    use dodos_abi::x86_64::REG_DR3;

    let mut k = K::new();

    // #54: rate limit window rolls over at exactly at + TRACE_BURST_NS
    let b = k.spawn();
    let t0 = k.m.now();
    for _ in 0..17 {
        k.m.fault(b, FaultKind::Page, [14, 2, UNMAPPED]);
        k.ok(Syscall::ThreadResume(s(101)));
    }
    assert_eq!(k.m.suppressed(), 1);
    k.m.advance(t0 + TRACE_BURST_NS);
    k.m.fault(b, FaultKind::Page, [14, 2, UNMAPPED]);
    let ring = k.kernel();
    assert_eq!(ring[ring.len() - 2].code(), 3);

    // #59: debug_write checks debug capability before length > DEBUG_WRITE_MAX
    k.ok(Syscall::CapDerive {
        source: DEBUG,
        destination: s(200),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(
        Syscall::DebugWrite {
            debug: s(200),
            length: DEBUG_WRITE_MAX + 1,
            words: vec![],
        },
        Error::Denied,
    );

    // #63: trace_emit requires WRITE on debug capability
    k.no(
        Syscall::TraceEmit {
            debug: s(200),
            words: [0; 3],
        },
        Error::Denied,
    );

    // #66, #68, #107: thread_write_regs checks REG_DR3 < user_end, and checks words (Invalid / Unsupported) before Busy
    let mut bad_dr3 = block();
    bad_dr3[REG_DR3] = Machine::example().geometry.user_end;
    k.no(
        Syscall::ThreadWriteRegs {
            thread: s(START_SLOT_THREAD),
            block: bad_dr3,
        },
        Error::Invalid,
    );
    let mut bad_uintr = block();
    bad_uintr[REG_UINTR] = 1;
    k.no(
        Syscall::ThreadWriteRegs {
            thread: s(START_SLOT_THREAD),
            block: bad_uintr,
        },
        Error::Unsupported,
    );

    // #80, #81, #82: ioport_out requires WRITE; ioport checks Invalid before Busy (ports_refused); console_port is refused
    k.ok(Syscall::CapDerive {
        source: PORTS,
        destination: s(201),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(port_out(s(201), 0x80, 1, 0), Error::Denied);
    k.no(port_in(PORTS, 0x20, 3), Error::Invalid);
    let mut mach_uart = Machine::example();
    mach_uart.console_port = Some(0x3F8);
    let mut ku = K::on(&mach_uart);
    ku.no(port_in(PORTS, 0x3F8, 1), Error::Busy);
    ku.no(port_in(PORTS, 0x3FF, 1), Error::Busy);
    assert_eq!(ku.go(port_in(PORTS, 0x400, 1)), Ok(Output::Value(0)));

    // #84, #85, #87: ioports_issue requires DERIVE (even with WRITE), checks destination before range, and checks fits before vacant
    k.ok(Syscall::CapDerive {
        source: PORTS,
        destination: s(202),
        rights: Rights::READ.union(Rights::WRITE).bits(),
        badge: 0,
    });
    k.no(
        Syscall::IoportsIssue {
            ioports: s(202),
            destination: s(203),
            first: 0x80,
            last: 0x80,
        },
        Error::Denied,
    );
    k.ok(Syscall::CapDerive {
        source: s(START_SLOT_CSPACE),
        destination: s(204),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    k.no(
        Syscall::IoportsIssue {
            ioports: PORTS,
            destination: SlotAddr::through(204, 0),
            first: 10,
            last: 5,
        },
        Error::Denied,
    );
    let mut cur = PORTS;
    for slot in 300..363 {
        k.ok(Syscall::CapDerive {
            source: cur,
            destination: s(slot),
            rights: Rights::READ.union(Rights::DERIVE).bits(),
            badge: 0,
        });
        cur = s(slot);
    }
    k.no(
        Syscall::IoportsIssue {
            ioports: cur,
            destination: s(START_SLOT_CSPACE),
            first: 0x80,
            last: 0x80,
        },
        Error::Invalid,
    );

    // #96, #97, #99: entropy_read mixes all 4 words (one word set in each
    // of four fresh nodes gives four different results: the final review's
    // F2 Opus 12 found only the fourth varied) and keeps all 8 * left bits
    // in the last word.
    let read = |mix: [u64; 4]| {
        let mut k = K::new();
        let Ok(Output::Bytes(words)) = k.go(Syscall::EntropyRead { length: 13, mix }) else {
            panic!("bytes");
        };
        words
    };
    let results = [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]].map(read);
    for i in 0..4 {
        for j in i + 1..4 {
            assert_ne!(results[i], results[j], "words {i} and {j}");
        }
    }
    assert!(results.iter().any(|w| w[1] >> 32 != 0));
}
