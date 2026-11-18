//! Pass 28: the idle states that lose a core's context, and the rules of
//! D-143 that depend on them: on ARM64 the gate masks the PE and reads the
//! park request before `CPU_SUSPEND` and reads it again at every way out of
//! a suspend, the fence signalling once per round of 11.2 ms; on RISC-V the
//! same read at every return from `sbi_hart_suspend` (`node_idle`; The
//! fence, ARM64 and RISC-V).

use super::pass17::{fresh, kill, seal};
use super::*;
use crate::system::{Event, Hardware};
use dodos_abi::{IDLE_FLAG_LOSES_CONTEXT, IDLE_METHOD_PSCI, IDLE_METHOD_SBI};

/// The exit latency of the state that loses its context in the tests.
const LATENCY: u64 = 200_000;

/// Node 0 of core 0, node 1 of cores 1 and 2 on `arch`; core 2 has an idle
/// table whose one state loses its context, and nothing to run.
fn sleeper(arch: Arch) -> S {
    let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
    let method = match arch {
        Arch::Aarch64 => IDLE_METHOD_PSCI,
        _ => IDLE_METHOD_SBI,
    };
    let idle = Syscall::NodeIdle {
        node: node_cap(1),
        core: 2,
        words: vec![LATENCY, 100_000, method, IDLE_FLAG_LOSES_CONTEXT],
    };
    m.ok(1, idle);
    m
}

fn asleep(m: &S, core: usize) -> bool {
    matches!(m.sys.hardware(core), Hardware::Asleep { wake: None, .. })
}

/// Time until `node` is no longer fencing, checked every 100 µs; the
/// instant it left fencing.
fn fence_ends(m: &mut S, node: u16) -> u64 {
    let start = m.sys.now();
    while m.state(node) == MemberState::Fencing && m.sys.now() < start + 100 * MS {
        m.run(100_000);
    }
    m.sys.now() - start
}

// A core asleep in a state that loses its context when its node is fenced:
// the first signal wakes it, firmware masks the PE at the wake and the
// signal is lost there, and the gate's read of the park request at the wake
// parks it; the second round's read confirms it at 11.2 ms. Without the
// read, the core, with nothing to run, goes back to sleep at each signal
// and is never confirmed (`node_idle`, The fence, ARM64, D-143).
#[test]
fn a_core_asleep_when_its_node_is_fenced_is_parked_by_the_read_at_its_wake() {
    let mut m = sleeper(Arch::Aarch64);
    assert!(m.step_until(|m| asleep(m, 2)), "core 2 never sleeps");
    // Node 1's first core hangs, its handler still able to run: no code of
    // node 1 parks core 2 for it, which, woken with nothing to run, would
    // go back at once to its deepest state.
    m.hang(1, true);
    m.ok(0, kill(1));
    let took = fence_ends(&mut m, 1);
    assert_eq!(m.state(1), MemberState::Fenced);
    assert!(took <= dodos_abi::FENCE_NS, "{took}");
    assert_eq!(m.sys.hardware(2), Hardware::Off);
}

// The window that remains: the first signal lands after the gate's read of
// the park request and before the power-down, SDEI masked, and is lost; the
// core sleeps, the second round's signal wakes it at 11.2 ms, the read at
// the wake parks it, and the third round's read confirms it at 22.4 ms,
// under `FENCE_NS` (The fence, ARM64, the bound, D-143). The core's thread
// waits for a timer due just after that wake: without the read at the wake
// the core would run it, park only at the third round's signal, and be
// confirmed at the fourth round's read, past `FENCE_NS`.
#[test]
fn a_signal_lost_on_the_way_to_sleep_is_made_good_by_the_next_round_and_the_read_at_the_wake() {
    let mut m = sleeper(Arch::Aarch64);
    m.ok(1, super::place_first(1, 1));
    let other = m.thread_on(1, 300, 2);
    let timer = m.retype(1, ObjectKind::Timer, 0, 310);
    m.ok(1, timer);
    let notify = m.retype(1, ObjectKind::Notify, 0, 311);
    m.ok(1, notify);
    let set = Syscall::TimerSet {
        timer: s(310),
        notify: s(311),
        deadline: m.sys.now() + 11_300_000,
        tolerance: 0,
    };
    assert!(m.call_by(1, other, set).is_ok());
    assert_eq!(m.call_by(1, other, Syscall::Wait(s(311), 0)), WAITS);
    let suspending = |m: &S| matches!(m.sys.hardware(2), Hardware::Suspending { .. });
    assert!(m.step_until(suspending), "core 2 never suspends");
    m.hang(1, true);
    m.ok(0, kill(1));
    // Events until the fence's stop of core 2 is in flight, its power-down
    // held back; then the stop, which lands in the window.
    let stop = |m: &S| {
        m.sys
            .actions()
            .iter()
            .position(|&(_, a)| a == crate::nodes::HwAction::Stop { core: 2 })
    };
    for _ in 0..10_000 {
        if stop(&m).is_some() {
            break;
        }
        let events = m.sys.events();
        match events
            .into_iter()
            .find(|&e| e != Event::PowerDown { core: 2 })
        {
            Some(event) => m.sys.fire(event),
            None => {
                let next = m.sys.now() + 10_000;
                m.sys.move_clock(next);
            }
        }
        assert_eq!(m.sys.check(), Ok(()));
    }
    let index = stop(&m).expect("a stop of core 2");
    m.sys.fire(Event::Action(index));
    assert!(suspending(&m), "the signal was not lost");
    let took = fence_ends(&mut m, 1);
    assert_eq!(m.state(1), MemberState::Fenced);
    assert!(
        took >= 2 * 11_200_000,
        "confirmed before the third round: {took}"
    );
    assert!(took <= dodos_abi::FENCE_NS, "{took}");
}

// A park request raised before the core suspends: the gate's read before
// `CPU_SUSPEND` finds the word marked and runs the park routine with no
// unmask, rather than sleeping (`node_idle`, D-143, D-146).
#[test]
fn the_gate_reads_the_park_request_before_a_suspend_that_loses_the_context() {
    let mut m = sleeper(Arch::Aarch64);
    let ready = |m: &S| m.sys.events().contains(&Event::Sleep { core: 2 });
    assert!(m.step_until(ready));
    // The park request of core 2 raised and its word marked, as a fence
    // does, before the core's next step (the fence's stop not yet sent).
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.park_requests[2] += 1;
        t.core_words[2] = dodos_abi::CoreWord::owned(1, g).marked().raw();
    });
    m.sys.fire(Event::Sleep { core: 2 });
    assert_eq!(m.sys.hardware(2), Hardware::Parking);
    let table = m.sys.canonical();
    assert_eq!(table.core_beacons[2].parked, table.park_requests[2]);
}

// A seal does not mark the words of its node's cores, so the read at the
// return from `sbi_hart_suspend` finds nothing (D-146): node 1 raises the
// park request of its sleeping hart and sends its interrupt, which wakes
// it; its kernel, awake, parks it, and the seal ends.
#[test]
fn a_riscv_hart_asleep_when_its_node_seals_is_woken_and_parked_by_its_kernel() {
    let mut m = sleeper(Arch::Riscv64);
    assert!(m.step_until(|m| asleep(m, 2)), "hart 2 never sleeps");
    m.ok(0, seal(1));
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    assert_eq!(m.sys.hardware(2), Hardware::Off);
}

// D-146: the gate's read of the park request means the word's fence mark,
// not "the request differs from `parked`". Node 1 is fenced: RISC-V stops
// its harts by `sbi_hart_stop`, and on ARM64, node 1 sealed first, core 2
// is already off when the fence raises its request, so neither writes
// `parked`. Node 1
// starts again on core 2 alone, healthy, and its first thread waits for a
// timer while the core sleeps in a state that loses its context: the read
// at the wake finds no mark and the core runs the thread. Read as "request
// differs from `parked`", the core would park itself at that wake (or at
// its first suspend on ARM64) and the healthy node would lose its only core.
fn reused_after_a_fence(arch: Arch) {
    let mut m = sleeper(arch);
    if arch == Arch::Aarch64 {
        // Node 1 seals first: its cores park themselves, off before the
        // fence raises their requests.
        m.ok(0, seal(1));
        m.run(20 * MS);
        assert_eq!(m.state(1), MemberState::Sealed);
        assert_eq!(m.sys.hardware(2), Hardware::Off);
    }
    m.ok(0, kill(1));
    let start = m.sys.now();
    while m.state(1) != MemberState::Dead && m.sys.now() < start + 100 * MS {
        m.run(100_000);
    }
    assert_eq!(m.state(1), MemberState::Dead);
    let table = m.sys.canonical();
    assert_ne!(
        table.park_requests[2], table.core_beacons[2].parked,
        "the fence left core 2's request unanswered in `parked`"
    );
    m.ok(0, fresh(1, 1 << 2));
    let start = m.sys.now();
    while m.state(1) != MemberState::Live && m.sys.now() < start + 100 * MS {
        m.run(100_000);
    }
    assert_eq!(m.state(1), MemberState::Live);
    m.restarted(1);
    let method = match arch {
        Arch::Aarch64 => IDLE_METHOD_PSCI,
        _ => IDLE_METHOD_SBI,
    };
    let idle = Syscall::NodeIdle {
        node: node_cap(1),
        core: 2,
        words: vec![LATENCY, 100_000, method, IDLE_FLAG_LOSES_CONTEXT],
    };
    m.ok(1, idle);
    let timer = m.retype(1, ObjectKind::Timer, 0, 310);
    m.ok(1, timer);
    let notify = m.retype(1, ObjectKind::Notify, 0, 311);
    m.ok(1, notify);
    let set = Syscall::TimerSet {
        timer: s(310),
        notify: s(311),
        deadline: m.sys.now() + 2 * MS,
        tolerance: 0,
    };
    assert!(m.call(1, set).is_ok());
    assert_eq!(m.call(1, Syscall::Wait(s(311), 0)), WAITS);
    assert!(m.step_until(|m| asleep(m, 2)), "core 2 never sleeps");
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_ne!(m.sys.hardware(2), Hardware::Off);
    assert!(
        m.woken(1).is_some_and(|o| o.is_ok()),
        "the thread never woke"
    );
}

#[test]
fn a_hart_stopped_by_the_fence_and_reused_does_not_park_itself_at_its_wake() {
    reused_after_a_fence(Arch::Riscv64);
}

#[test]
fn a_core_already_off_at_a_fence_and_reused_does_not_park_itself_at_its_wake() {
    reused_after_a_fence(Arch::Aarch64);
}

// The DR6 rules of D-143, x86-64: a #DB at CPL0 on the core that runs the
// first thread of a node, whose control word enables breakpoint 0.

const B0: u64 = 1;
const BD: u64 = 1 << 13;
const BS: u64 = 1 << 14;
const BT: u64 = 1 << 15;
const RESET: u64 = 0xFFFF_0FF0;

fn debugged() -> N {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let regs = &mut k.m.threads.get_mut(&k.t).unwrap().regs;
    regs[dodos_abi::x86_64::REG_DR7] = 1;
    k
}

fn accused(k: &N) -> bool {
    k.m.table.beacons[usize::from(k.m.node)].panic.is_some()
}

/// The fault of kind 9 recorded in the first thread (it has no fault
/// endpoint): words 3 and 4.
fn breakpoint(k: &N) -> Option<(u64, u64)> {
    k.m.threads[&k.t]
        .recorded
        .filter(|&(kind, _)| kind == dodos_abi::FaultKind::HardwareBreakpoint as u32)
        .map(|(_, words)| (words[3], words[4]))
}

// A single step that ends on the system-call entry is ignored, and the
// handler loads DR6's reset value: the next #DB, a breakpoint of the thread
// hit elsewhere, reads only its own cause and is the thread's fault, word 4
// the value read. Without the reset BS would still read set, and the
// second #DB would be a bug of the kernel (D-143, SDM §20.2.3).
#[test]
fn the_reset_of_dr6_keeps_a_single_step_ignored_at_the_entry_from_making_the_next_db_a_bug() {
    let mut k = debugged();
    k.m.kernel_debug(k.t, BS, true);
    assert_eq!(k.m.check(), Ok(()));
    assert!(!accused(&k));
    assert_eq!(
        breakpoint(&k),
        None,
        "a single step at the entry is ignored"
    );
    k.m.kernel_debug(k.t, B0, false);
    assert_eq!(k.m.check(), Ok(()));
    assert!(!accused(&k), "the second #DB read a single step left over");
    assert_eq!(breakpoint(&k), Some((B0, RESET | B0)));
}

// Each cause judged alone: BS at the entry and an enabled breakpoint
// together are the thread's, the fault of kind 9 delivered and the single
// step ignored (rule (1) of D-142 as written called it a bug).
#[test]
fn a_single_step_at_the_entry_with_a_breakpoint_of_the_thread_is_the_threads_fault() {
    let mut k = debugged();
    k.m.kernel_debug(k.t, BS | B0, true);
    assert_eq!(k.m.check(), Ok(()));
    assert!(!accused(&k));
    assert_eq!(breakpoint(&k), Some((B0, RESET | BS | B0)));
}

// BS anywhere but the entry, BD or BT at CPL0 is a bug of the kernel, a
// breakpoint of the thread with it or not; a breakpoint the thread does not
// enable names no cause of the thread's.
#[test]
fn a_single_step_elsewhere_a_register_access_or_a_task_switch_at_cpl0_is_a_bug_of_the_kernel() {
    for causes in [BS, BS | B0, BD, BT | B0, BD | BS] {
        let mut k = debugged();
        k.m.kernel_debug(k.t, causes, false);
        assert!(accused(&k), "{causes:#x}");
        assert_eq!(breakpoint(&k), None);
        assert_eq!(k.m.cores[0].dr6, 0, "{causes:#x}");
    }
    let mut k = debugged();
    k.m.kernel_debug(k.t, 1 << 1, false);
    assert!(!accused(&k));
    assert_eq!(breakpoint(&k), None, "breakpoint 1 is not enabled");
}

// At CPL3 too the handler loads DR6's reset value: a hardware breakpoint of
// the thread, then a single step that ends on the entry, which is ignored.
#[test]
fn the_handler_resets_dr6_at_cpl3_as_well() {
    let mut k = debugged();
    k.m.fault(k.t, dodos_abi::FaultKind::HardwareBreakpoint, [1, B0, B0]);
    assert_eq!(breakpoint(&k), Some((B0, RESET | B0)));
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.cores[0].dr6, 0);
}

// The APLIC rule of D-143, RISC-V: a level line of GSI 5 on the APLIC
// domain a machine of one node holds, in MSI mode.

const GSI: u32 = 5;

fn claimed(level: bool) -> S {
    let mut m = S::on(&one_node(Arch::Riscv64, 1));
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(GSI, level, false).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, 0, claim, 30 * MS);
    let notify = m.retype(0, ObjectKind::Notify, 0, 211);
    m.ok(0, notify);
    m
}

pub(super) fn bind(m: &mut S) {
    let bind = Syscall::IrqBind {
        irq: s(210),
        notify: s(211),
    };
    m.ok(0, bind);
}

fn state(m: &S) -> crate::IrqState {
    m.sys.node(0).irqs.values().next().expect("the irq").state
}

fn forward(m: &mut S) -> bool {
    let forwarded = m.sys.node_mut(0).aplic_forward(GSI);
    assert_eq!(m.sys.check(), Ok(()));
    forwarded
}

fn input(m: &mut S, high: bool) {
    m.sys.node_mut(0).line_input(GSI, high);
}

// The driver acknowledges while its device still asserts the line (a second
// event came while it served the first): `irq_ack` writes `setipnum`, the
// source is pending again and forwarded, and the irq fires. Without the
// write the APLIC never forwards it again, the line high and the irq armed
// for ever (the AIA, §4.7, §4.9.2).
#[test]
fn the_acknowledgement_of_a_level_line_still_high_signals_it_again() {
    let mut m = claimed(true);
    bind(&mut m);
    input(&mut m, true);
    assert!(forward(&mut m));
    assert_eq!(state(&m), crate::IrqState::Fired);
    assert!(!forward(&mut m), "no second MSI while the input stays high");
    m.ok(0, Syscall::IrqAck(s(210)));
    assert!(forward(&mut m), "the source is pending again");
    assert_eq!(state(&m), crate::IrqState::Fired);
    // Served: the line low, the acknowledgement arms the irq with nothing
    // pending.
    input(&mut m, false);
    m.ok(0, Syscall::IrqAck(s(210)));
    assert_eq!(state(&m), crate::IrqState::Armed);
    assert!(!forward(&mut m));
}

// A line raised before its irq is bound: the APLIC forwards it to an idle
// irq, which ignores a level arrival; the bind that arms the irq writes
// `setipnum`, and the line is signalled.
#[test]
fn a_bind_that_arms_a_level_line_already_high_signals_it() {
    let mut m = claimed(true);
    input(&mut m, true);
    assert!(forward(&mut m));
    assert_eq!(state(&m), crate::IrqState::Idle);
    bind(&mut m);
    assert!(forward(&mut m));
    assert_eq!(state(&m), crate::IrqState::Fired);
}

// An edge line gets no write: its acknowledgement leaves the APLIC as it
// is.
#[test]
fn an_edge_line_gets_no_setipnum_write() {
    let mut m = claimed(false);
    bind(&mut m);
    m.sys.node_mut(0).interrupt_on(0, 48);
    m.ok(0, Syscall::IrqAck(s(210)));
    assert!(m.sys.node(0).aplic.is_empty());
}
