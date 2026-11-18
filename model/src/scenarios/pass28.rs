//! Pass 28: the idle states that lose a core's context, and the rules of
//! D-143 that depend on them: on ARM64 the gate masks the PE and reads the
//! park request before `CPU_SUSPEND` and reads it again at every way out of
//! a suspend, the fence signalling once per round of 11.2 ms; on RISC-V the
//! same read at every return from `sbi_hart_suspend` (`node_idle`; The
//! fence, ARM64 and RISC-V).

use super::pass17::{fresh, kill, resume, seal};
use super::*;
use crate::system::{Event, Hardware};
use dodos_abi::{IDLE_FLAG_LOSES_CONTEXT, IDLE_METHOD_PSCI, IDLE_METHOD_SBI};

/// The exit latency of the state that loses its context in the tests.
const LATENCY: u64 = 200_000;

/// The method word of a state that loses the context: PSCI with the
/// StateType bit of the original format, or an SBI non-retentive type.
fn lossy(arch: Arch) -> u64 {
    match arch {
        Arch::Aarch64 => IDLE_METHOD_PSCI | 1 << 16 << 32,
        _ => IDLE_METHOD_SBI | 0x8000_0000 << 32,
    }
}

/// Node 0 of core 0, node 1 of cores 1 and 2 on `arch`; core 2 has an idle
/// table whose one state loses its context, and nothing to run.
fn sleeper(arch: Arch) -> S {
    let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
    let method = lossy(arch);
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
    survives_its_next_suspend(&mut m, arch, true);
}

/// Node 1 gives core 2 its idle state that loses the context; a thread on
/// core 2 (its first thread when `first`, a new one otherwise) waits for a
/// timer 2 ms ahead, so core 2 sleeps and its timer wakes it: the core runs
/// the thread and node 1 stays live.
fn survives_its_next_suspend(m: &mut S, arch: Arch, first: bool) {
    let method = lossy(arch);
    let idle = Syscall::NodeIdle {
        node: node_cap(1),
        core: 2,
        words: vec![LATENCY, 100_000, method, IDLE_FLAG_LOSES_CONTEXT],
    };
    m.ok(1, idle);
    let thread = match first {
        true => m.t[1],
        false => m.thread_on(1, 300, 2),
    };
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
    assert!(m.call_by(1, thread, set).is_ok());
    assert_eq!(m.call_by(1, thread, Syscall::Wait(s(311), 0)), WAITS);
    assert!(m.step_until(|m| asleep(m, 2)), "core 2 never sleeps");
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_ne!(m.sys.hardware(2), Hardware::Off);
    let woken = m.sys.node(1).woken(thread);
    assert!(woken.is_some_and(|o| o.is_ok()), "the thread never woke");
}

#[test]
fn a_hart_stopped_by_the_fence_and_reused_does_not_park_itself_at_its_wake() {
    reused_after_a_fence(Arch::Riscv64);
}

#[test]
fn a_core_already_off_at_a_fence_and_reused_does_not_park_itself_at_its_wake() {
    reused_after_a_fence(Arch::Aarch64);
}

// D-146, (a): a seal parks node 1's cores through its own kernel, which
// writes no `parked` on ARM64 or RISC-V (`CPU_OFF`, `sbi_hart_stop`), so
// after the resume each core's request differs from `parked`; the words
// carry no mark, and core 2 survives its next suspend.
fn resumed_after_a_seal(arch: Arch) {
    let mut m = sleeper(arch);
    m.ok(0, seal(1));
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let table = m.sys.canonical();
    assert_ne!(table.park_requests[2], table.core_beacons[2].parked);
    m.ok(0, resume(1));
    let start = m.sys.now();
    while m.state(1) != MemberState::Live && m.sys.now() < start + 100 * MS {
        m.run(100_000);
    }
    assert_eq!(m.state(1), MemberState::Live);
    survives_its_next_suspend(&mut m, arch, false);
}

#[test]
fn a_core_resumed_after_a_seal_survives_its_next_suspend_on_arm64() {
    resumed_after_a_seal(Arch::Aarch64);
}

#[test]
fn a_hart_resumed_after_a_seal_survives_its_next_suspend_on_riscv() {
    resumed_after_a_seal(Arch::Riscv64);
}

/// Core 2 of node 1 on its way to sleep, node 1's other core hung and node
/// 1 killed; events until the fence's stop of core 2 is in flight, its
/// power-down held back; then the stop, lost in the window.
fn signal_lost_on_the_way_to_sleep(m: &mut S) {
    let suspending = |m: &S| matches!(m.sys.hardware(2), Hardware::Suspending { .. });
    assert!(m.step_until(suspending), "core 2 never suspends");
    m.hang(1, true);
    m.ok(0, kill(1));
    let stop = |m: &S| {
        m.sys
            .actions()
            .iter()
            .position(|&(_, a)| a == crate::nodes::HwAction::Stop { core: 2 })
    };
    for _ in 0..10_000 {
        if stop(m).is_some() {
            break;
        }
        let held = |e: &Event| {
            matches!(
                e,
                Event::PowerDown { core: 2 } | Event::ShallowReturn { core: 2 }
            )
        };
        match m.sys.events().into_iter().find(|e| !held(e)) {
            Some(event) => m.sys.fire(event),
            None => {
                let next = m.sys.now() + 10_000;
                m.sys.move_clock(next);
            }
        }
        assert_eq!(m.sys.check(), Ok(()));
    }
    let index = stop(m).expect("a stop of core 2");
    m.sys.fire(Event::Action(index));
    assert!(suspending(m), "the signal was not lost");
}

// D-146: `CPU_SUSPEND` returns without losing the context (refused, or a
// shallower state) after the fence's signal was lost on the way: the gate
// unmasks and reads the word, finds the mark and parks the core, rather
// than returning to the node with its word marked and nothing in flight.
#[test]
fn a_suspend_that_returns_without_losing_the_context_reads_the_mark() {
    let mut m = sleeper(Arch::Aarch64);
    signal_lost_on_the_way_to_sleep(&mut m);
    m.sys.fire(Event::ShallowReturn { core: 2 });
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.sys.hardware(2), Hardware::Parking);
}

// D-146, the RISC-V cooperative fence (hart 2 of fence class 0, no stop
// extension): its interrupt, lost on the way into a non-retentive suspend,
// is sent again at the next round of 11.2 ms; it wakes the hart, the read
// at the return stops it, and the third round's `sbi_hart_get_status`
// confirms it, within `FENCE_NS`. Sent once, the hart would sleep through
// the fence.
#[test]
fn the_riscv_cooperative_fence_sends_its_interrupt_again_each_round() {
    let mut machine = machine(Arch::Riscv64, 3, &[&[0], &[1, 2]]);
    machine.cores[2].fence = 0;
    machine.accept_unfenced = true;
    let mut m = S::on(&machine);
    let idle = Syscall::NodeIdle {
        node: node_cap(1),
        core: 2,
        words: vec![
            LATENCY,
            100_000,
            lossy(Arch::Riscv64),
            IDLE_FLAG_LOSES_CONTEXT,
        ],
    };
    m.ok(1, idle);
    signal_lost_on_the_way_to_sleep(&mut m);
    let took = fence_ends(&mut m, 1);
    assert!(matches!(
        m.state(1),
        MemberState::Fenced | MemberState::Dead
    ));
    assert!(
        took >= 11_200_000,
        "confirmed before the second round: {took}"
    );
    assert!(took <= dodos_abi::FENCE_NS, "{took}");
    assert_eq!(m.sys.hardware(2), Hardware::Off);
}

// D-146: whether a state loses the context is read from its parameter,
// and `node_idle` refuses a method 2 or 3 entry whose flag disagrees: ARM64
// the StateType bit, 16 in the original format and 30 in the extended one
// the machine start-up read; RISC-V bit 31 of the suspend type.
#[test]
fn node_idle_refuses_a_suspend_state_whose_flag_disagrees_with_its_parameter() {
    let idle = |method: u64, flag: u64| Syscall::NodeIdle {
        node: node_cap(0),
        core: 0,
        words: vec![LATENCY, 100_000, method, flag],
    };
    let psci = |bits: u64| IDLE_METHOD_PSCI | bits << 32;
    let sbi = |bits: u64| IDLE_METHOD_SBI | bits << 32;
    let lose = IDLE_FLAG_LOSES_CONTEXT;
    let mut k = N::on(&one_node(Arch::Aarch64, 1));
    k.ok(idle(psci(1 << 16), lose));
    k.ok(idle(psci(0), 0));
    k.no(idle(psci(1 << 16), 0), Error::Invalid);
    k.no(idle(psci(0), lose), Error::Invalid);
    // Bit 30 says nothing in the original format.
    k.no(idle(psci(1 << 30), lose), Error::Invalid);
    let mut machine = one_node(Arch::Aarch64, 1);
    machine.psci_extended = true;
    let mut k = N::on(&machine);
    k.ok(idle(psci(1 << 30), lose));
    k.no(idle(psci(1 << 16), lose), Error::Invalid);
    let mut k = N::on(&one_node(Arch::Riscv64, 1));
    k.ok(idle(sbi(0x8000_0000), lose));
    k.ok(idle(sbi(0), 0));
    k.no(idle(sbi(0x8000_0000), 0), Error::Invalid);
    k.no(idle(sbi(0), lose), Error::Invalid);
}

// The gate reads the parameter: a retentive SBI state, flag 0, is entered
// as one that keeps the context, so no read at its return is due.
#[test]
fn a_state_that_keeps_the_context_by_its_parameter_is_no_lossy_sleep() {
    let mut m = S::on(&machine(Arch::Riscv64, 3, &[&[0], &[1, 2]]));
    let idle = Syscall::NodeIdle {
        node: node_cap(1),
        core: 2,
        words: vec![LATENCY, 100_000, IDLE_METHOD_SBI, 0],
    };
    m.ok(1, idle);
    m.run(5 * MS);
    assert!(!m.sys.events().contains(&Event::Sleep { core: 2 }));
    assert_eq!(m.sys.hardware(2), Hardware::Running);
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

// The APLIC rules of D-143 and D-146, RISC-V: a level line of GSI 5 of an
// APLIC domain in MSI mode, claimed by node `n` of a machine of `nodes`
// nodes of one core each.

const GSI: u32 = 5;

fn claimed_on(nodes: usize, n: u16, level: bool) -> S {
    let groups: Vec<Vec<usize>> = (0..nodes).map(|c| vec![c]).collect();
    let groups: Vec<&[usize]> = groups.iter().map(Vec::as_slice).collect();
    let mut m = S::on(&machine(Arch::Riscv64, nodes, &groups));
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(GSI, level, false).raw(),
        core: u64::from(n),
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, n, claim, 30 * MS);
    let notify = m.retype(n, ObjectKind::Notify, 0, 211);
    m.ok(n, notify);
    m
}

fn claimed(level: bool) -> S {
    claimed_on(1, 0, level)
}

pub(super) fn bind_on(m: &mut S, n: u16) {
    let bind = Syscall::IrqBind {
        irq: s(210),
        notify: s(211),
    };
    m.ok(n, bind);
}

pub(super) fn bind(m: &mut S) {
    bind_on(m, 0);
}

fn state_on(m: &S, n: u16) -> crate::IrqState {
    m.sys.node(n).irqs.values().next().expect("the irq").state
}

fn state(m: &S) -> crate::IrqState {
    state_on(m, 0)
}

/// The MSIs of the line on their way, delivered one by one: how many.
fn deliver(m: &mut S) -> u32 {
    let mut arrivals = 0;
    while m.sys.aplic_deliver(GSI) {
        assert_eq!(m.sys.check(), Ok(()));
        arrivals += 1;
    }
    arrivals
}

fn input(m: &mut S, high: bool) {
    m.sys.line_input(GSI, high);
    assert_eq!(m.sys.check(), Ok(()));
}

fn ack(m: &mut S, n: u16) {
    m.ok(n, Syscall::IrqAck(s(210)));
}

// The driver acknowledges while its device still asserts the line (a second
// event came while it served the first): `irq_ack` writes `setipnum`, then
// `setienum`; the source is pending again and enabled, an MSI is sent, and
// the irq fires. Without the write the APLIC never sends it again, the line
// high and the irq armed for ever (the AIA, §4.7, §4.9.2).
#[test]
fn the_acknowledgement_of_a_level_line_still_high_signals_it_again() {
    let mut m = claimed(true);
    bind(&mut m);
    input(&mut m, true);
    assert_eq!(deliver(&mut m), 1);
    assert_eq!(state(&m), crate::IrqState::Fired);
    assert_eq!(
        deliver(&mut m),
        0,
        "no second MSI while the input stays high"
    );
    ack(&mut m, 0);
    assert_eq!(deliver(&mut m), 1, "the source is pending again");
    assert_eq!(state(&m), crate::IrqState::Fired);
    // Served: the line low, the acknowledgement arms the irq with nothing
    // pending.
    input(&mut m, false);
    ack(&mut m, 0);
    assert_eq!(state(&m), crate::IrqState::Armed);
    assert_eq!(deliver(&mut m), 0);
}

// A line raised before its irq is bound: its source is not enabled, so
// nothing is sent; the bind that arms the irq writes `setipnum` and
// `setienum`, and the line is signalled once.
#[test]
fn a_bind_that_arms_a_level_line_already_high_signals_it() {
    let mut m = claimed(true);
    input(&mut m, true);
    assert_eq!(deliver(&mut m), 0);
    assert_eq!(state(&m), crate::IrqState::Idle);
    bind(&mut m);
    assert_eq!(deliver(&mut m), 1);
    assert_eq!(state(&m), crate::IrqState::Fired);
}

// An edge line gets no write.
#[test]
fn an_edge_line_gets_no_setipnum_write() {
    let mut m = claimed(false);
    bind(&mut m);
    m.sys.node_mut(0).interrupt_on(0, 48);
    ack(&mut m, 0);
    assert!(m.sys.canonical().aplic.is_empty());
}

// D-146, the order: the line falls and rises while its source is masked
// (the irq fired, not yet acknowledged); the rise sets the pending bit. The
// acknowledgement writes `setipnum`, which finds it set, then `setienum`,
// which sends one MSI. Enabled first, the source would send for the old
// pending bit, and `setipnum` would set it again: two arrivals.
#[test]
fn a_level_line_that_fell_and_rose_while_masked_arrives_once() {
    let mut m = claimed(true);
    bind(&mut m);
    input(&mut m, true);
    assert_eq!(deliver(&mut m), 1);
    assert!(
        !m.sys.canonical().aplic[&GSI].enabled,
        "masked at its arrival"
    );
    input(&mut m, false);
    input(&mut m, true);
    ack(&mut m, 0);
    assert_eq!(m.sys.canonical().aplic[&GSI].sent, 1);
    assert_eq!(deliver(&mut m), 1);
    assert_eq!(state(&m), crate::IrqState::Fired);
}

// D-146: a level line claimed on a node that does not hold the APLIC
// domain's role; its arrival masks it through the claiming node's gate,
// with no request to the holder, and its acknowledgement signals it again.
// The holder then hands the role on by a seal: the line works as before,
// its writes never going through the holder.
#[test]
fn a_level_line_on_a_node_without_the_aplic_role_works_across_a_move_of_the_role() {
    let probe = claimed_on(3, 0, true);
    let role = probe
        .sys
        .node(0)
        .ioapic_role(GSI)
        .expect("an APLIC serves GSI 5");
    let holder = probe.sys.role(role).holder();
    let n = (0..3).find(|&n| n != holder).unwrap();
    let mut m = claimed_on(3, n, true);
    assert_ne!(m.sys.role(role).holder(), n);
    bind_on(&mut m, n);
    input(&mut m, true);
    let asked = |m: &S| m.sys.node(n).unit_requests.iter().flatten().count();
    let before = asked(&m);
    assert_eq!(deliver(&mut m), 1);
    assert_eq!(state_on(&m, n), crate::IrqState::Fired);
    assert_eq!(asked(&m), before, "the arrival asks the holder nothing");
    let other = (0..3).find(|&z| z != n && z != holder).unwrap();
    m.ok(other, seal(holder));
    m.run(20 * MS);
    assert_eq!(m.state(holder), MemberState::Sealed);
    assert_ne!(
        m.sys.role(role).holder(),
        holder,
        "the seal handed the role"
    );
    ack(&mut m, n);
    assert_eq!(deliver(&mut m), 1);
    assert_eq!(state_on(&m, n), crate::IrqState::Fired);
}

// G9 on RISC-V: the gate writes the three registers only with the number of
// a level source whose line word the node holds and that a core of it
// claimed; another node's line is refused and the APLIC is unchanged.
#[test]
fn the_gate_refuses_an_aplic_write_for_a_line_the_node_does_not_hold() {
    use crate::irq::AplicReg;
    let mut m = claimed_on(2, 0, true);
    bind_on(&mut m, 0);
    let before = m.sys.canonical().aplic.clone();
    for register in [AplicReg::SetIp, AplicReg::SetIe, AplicReg::ClrIe] {
        assert!(!m.sys.node_mut(1).aplic_write(GSI, register));
        assert!(!m.sys.node_mut(0).aplic_write(GSI + 1, register));
    }
    assert_eq!(m.sys.node(1).table.aplic, before);
    assert_eq!(m.sys.node(0).table.aplic, before);
}
