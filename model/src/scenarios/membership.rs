//! Cores (D-128, D-129): the calls of the group, detection with evidence,
//! the fence of every core of a node on the hardware of `System`, reclaim
//! and the barrier, fresh start, seal and resume for a replacement, and
//! the park stub's lost wake-up with the starter's retry every millisecond.
//! On the reference machine node 0 holds the bootstrap core (fence class 1)
//! and node 1 a core of class 2, which holds the unit roles.

use super::*;
use crate::nodes::HwAction;
use crate::system::Hardware;
use dodos_abi::{
    CoreState, CoreWord, CrashReason, Evidence, FENCE_NS, IDLE_FLAG_LOSES_CONTEXT,
    IDLE_METHOD_HALT, IDLE_METHOD_MWAIT, IDLE_METHOD_PSCI, NODE_START_FRESH, NODE_START_RESUME,
    NODE_STATUS_CLASS_SHIFT, NODE_STATUS_CORES_MASK, NODE_STATUS_UNUSABLE, NODE_STOP_KILL,
    NODE_STOP_RETIRE, NODE_STOP_SEAL, RoleKind, RoleState, SEAL_NS, STALE_NS, START_NS,
    START_SLOT_THREAD, STUCK_NS, SWEEP_NS,
};

fn stop(n: u16, mode: u64) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode,
        core: 0,
    }
}

/// A fresh start of node `n` on the cores of `mask` (window 0).
fn start_on(n: u16, mask: u64) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_FRESH,
        image: 0,
        module: 0,
        window: 0,
        mask,
    }
}

fn resume(n: u16, image: u64) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_RESUME,
        image,
        module: 0,
        window: 0,
        mask: 0,
    }
}

fn status(s: &mut S, from: u16, n: u16, core: u64) -> [u64; 7] {
    let call = Syscall::NodeStatus {
        node: node_cap(n),
        core,
    };
    match s.call(from, call) {
        Ok(Output::Status(words)) => words,
        other => panic!("{other:?}"),
    }
}

/// The unit roles of the machine, by index.
fn unit_roles(s: &S) -> Vec<usize> {
    let table = s.sys.canonical();
    (0..table.roles.len())
        .filter(|&r| table.role_records[r].kind == RoleKind::Unit)
        .collect()
}

// The calls of the group.

#[test]
fn node_status_reads_the_member_word_the_cores_and_the_counters() {
    let mut m = S::two();
    let words = status(&mut m, 0, 1, 1);
    let member = dodos_abi::Member::from_raw(words[0]).unwrap();
    assert_eq!(
        (member.state(), member.generation()),
        (MemberState::Live, 1)
    );
    // One core names node 1, of fence class 2, usable.
    assert_eq!(words[1] & NODE_STATUS_CORES_MASK, 1);
    assert_eq!(words[1] >> NODE_STATUS_CLASS_SHIFT & 3, 2);
    assert_eq!(words[1] & NODE_STATUS_UNUSABLE, 0);
    assert_eq!(words[6], Machine::example().perf_caps);
    // The time words of a core whose word does not name the node are 0.
    let words = status(&mut m, 0, 1, 0);
    assert_eq!(words[2..6], [0; 4]);
    // A core number beyond the core list is invalid; `read` is needed.
    m.no(
        0,
        Syscall::NodeStatus {
            node: node_cap(1),
            core: 2,
        },
        Error::Invalid,
    );
    m.ok(
        0,
        Syscall::CapDerive {
            source: node_cap(1),
            destination: s(100),
            rights: Rights::WRITE.bits(),
            badge: 0,
        },
    );
    m.no(
        0,
        Syscall::NodeStatus {
            node: s(100),
            core: 0,
        },
        Error::Denied,
    );
}

#[test]
fn node_status_counts_every_core_of_a_node_of_several_cores() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2, 3]]));
    let words = status(&mut m, 0, 1, 2);
    assert_eq!(words[1] & NODE_STATUS_CORES_MASK, 3);
}

#[test]
fn node_watch_names_the_callers_node_and_is_signalled_at_each_change() {
    let mut m = S::two();
    let notify = m.retype(0, ObjectKind::Notify, 0, 100);
    m.ok(0, notify);
    let watch = |n, index, notify| Syscall::NodeWatch {
        node: node_cap(n),
        index,
        notify,
    };
    m.no(0, watch(1, 0, s(100)), Error::Invalid);
    m.no(0, watch(0, 4, s(100)), Error::Invalid);
    m.ok(0, watch(0, 0, s(100)));
    m.no(0, watch(0, 0, s(100)), Error::Busy);
    // Unbinding succeeds on an unbound index.
    m.ok(0, watch(0, 3, SlotAddr::NONE));
    // A change of a member word: the notify is signalled.
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.settle();
    assert_eq!(m.call(0, Syscall::Poll(s(100))), Ok(Output::Word(1)));
    m.run(5 * MS);
    assert_eq!(m.call(0, Syscall::Poll(s(100))), Ok(Output::Word(1)));
}

#[test]
fn node_idle_judges_its_table_entry_by_entry_for_a_core_of_the_callers_node() {
    let mut m = S::two();
    let idle = |n, core, words: Vec<u64>| Syscall::NodeIdle {
        node: node_cap(n),
        core,
        words,
    };
    m.ok(0, idle(0, 0, vec![]));
    m.ok(
        0,
        idle(
            0,
            0,
            vec![
                10,
                20,
                IDLE_METHOD_HALT,
                0,
                30,
                40,
                IDLE_METHOD_MWAIT | 0x20 << 32,
                IDLE_FLAG_LOSES_CONTEXT,
            ],
        ),
    );
    for words in [
        vec![1, 2, 0],
        vec![1 << 63, 0, 0, 0],
        vec![10, 0, 0, 0, 10, 0, 0, 0],
        vec![10, 0, IDLE_METHOD_PSCI, 0],
        vec![10, 0, 0, 2],
        vec![0; 64],
    ] {
        m.no(0, idle(0, 0, words), Error::Invalid);
    }
    // Another node, or a core that is not one of the node's, is invalid.
    m.no(0, idle(1, 1, vec![]), Error::Invalid);
    m.no(0, idle(0, 1, vec![]), Error::Invalid);
}

#[test]
fn node_perf_is_judged_and_unsupported_without_the_register() {
    let mut m = S::two();
    let perf = |n, core, lowest, highest| Syscall::NodePerf {
        node: node_cap(n),
        core,
        lowest,
        highest,
        preference: 0x80,
    };
    m.ok(0, perf(0, 0, 10, 40));
    m.no(0, perf(0, 0, 41, 40), Error::Invalid);
    m.no(0, perf(0, 0, 10, 256), Error::Invalid);
    m.no(0, perf(1, 1, 10, 40), Error::Invalid);
    m.no(0, perf(0, 1, 10, 40), Error::Invalid);
    let mut machine = Machine::example();
    machine.perf_request = false;
    let mut m = S::on(&machine);
    m.no(0, perf(0, 0, 41, 40), Error::Unsupported);
}

#[test]
fn node_stop_is_judged_and_never_leaves_no_live_node() {
    let mut m = S::two();
    m.no(0, stop(1, 3), Error::Unsupported);
    m.no(0, stop(1, 4), Error::Invalid);
    // Modes kill and seal take core 0; retire a core whose word names the
    // node.
    m.no(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_KILL,
            core: 1,
        },
        Error::Invalid,
    );
    m.no(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_RETIRE,
            core: 0,
        },
        Error::Invalid,
    );
    m.ok(0, stop(1, NODE_STOP_KILL));
    // Already fencing: busy for every mode.
    m.no(0, stop(1, NODE_STOP_KILL), Error::Busy);
    m.no(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_RETIRE,
            core: 1,
        },
        Error::Busy,
    );
    m.no(0, stop(1, NODE_STOP_SEAL), Error::Busy);
    // Node 0 is now the last live node: it cannot stop itself.
    m.no(0, stop(0, NODE_STOP_KILL), Error::Busy);
    m.no(0, stop(0, NODE_STOP_SEAL), Error::Busy);
}

#[test]
fn node_start_is_judged_in_order() {
    let mut m = S::two();
    let call = |mode, image, module, window, mask| Syscall::NodeStart {
        node: node_cap(1),
        mode,
        image,
        module,
        window,
        mask,
    };
    m.no(0, call(2, 0, 0, 0, 2), Error::Invalid);
    m.no(0, call(0, 2, 0, 0, 2), Error::Invalid);
    m.no(0, call(0, 0, 9, 0, 2), Error::Invalid);
    m.no(0, call(0, 0, 0, 1024, 2), Error::Invalid);
    m.no(0, call(0, 0, 0, 0, 0), Error::Invalid);
    m.no(0, call(1, 0, 1, 0, 0), Error::Invalid);
    m.no(0, call(1, 0, 0, 0, 2), Error::Invalid);
    // A live node: busy for both modes.
    m.no(0, call(0, 0, 0, 0, 2), Error::Busy);
    m.no(0, call(1, 0, 0, 0, 0), Error::Busy);
    // More cores than the image supports: unsupported, after the plain
    // arguments and before the state.
    let mut machine = Machine::example();
    machine.cores_supported = 1;
    let mut m = S::on(&machine);
    m.no(0, call(0, 0, 0, 0, 3), Error::Unsupported);
}

// Stop, fence, reclaim, start.

#[test]
fn a_killed_node_is_fenced_reclaimed_and_dead_then_started_fresh() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_KILL));
    let member = m.sys.member(1);
    assert_eq!(
        (member.state(), member.evidence(), member.driver()),
        (MemberState::Fencing, Evidence::Kill, 0)
    );
    // The driver raises the park request, then marks the word, then stops
    // the core.
    m.settle();
    let word = CoreWord::from_raw(m.sys.core_word(1)).unwrap();
    assert!(word.is_marked());
    assert_ne!(m.sys.park_request(1), 0);
    m.run(2 * MS);
    assert!(matches!(m.sys.hardware(1), Hardware::Stub { .. }));
    // Its unit roles go to node 0 after SETTLE_NS, then the barrier holds.
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert!(!m.sys.has(1));
    for role in unit_roles(&m) {
        assert_eq!(m.sys.role(role).holder(), 0);
    }
    // The core word is free again after R6.
    let word = CoreWord::from_raw(m.sys.core_word(1)).unwrap();
    assert_eq!(word.state(), CoreState::Free);
    // A fresh start: generation 2, its own start-up state, live once its
    // core reads owned.
    m.ok(0, start_on(1, 0b10));
    assert_eq!(m.sys.member(1).generation(), 2);
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    m.restarted(1);
    let notify = m.retype(1, ObjectKind::Notify, 0, 100);
    m.ok(1, notify);
}

#[test]
fn a_fresh_start_on_several_cores_writes_joining_then_owned_then_live() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, start_on(1, 0b110));
    // One gate call: starting (g + 1), each word joining.
    assert_eq!(m.state(1), MemberState::Starting);
    for core in [1, 2] {
        let word = CoreWord::from_raw(m.sys.core_word(core)).unwrap();
        assert_eq!(word.state(), CoreState::Joining);
    }
    m.run(3 * MS);
    for core in [1, 2] {
        let word = CoreWord::from_raw(m.sys.core_word(core)).unwrap();
        assert_eq!(word.state(), CoreState::Owned);
    }
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.node(1).width(), 2);
}

#[test]
fn a_fresh_start_on_a_core_that_is_not_free_is_busy_and_changes_nothing() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(10 * MS);
    let before = m.sys.member(1);
    // Core 2 belongs to node 2.
    m.no(0, start_on(1, 0b110), Error::Busy);
    assert_eq!(m.sys.member(1), before);
    assert_eq!(
        CoreWord::from_raw(m.sys.core_word(1)).unwrap().state(),
        CoreState::Free
    );
}

#[test]
fn a_fresh_start_on_cores_of_different_state_features_is_busy() {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    let mut lesser = machine.kinds[0];
    lesser.features &= !(1 << dodos_abi::x86_64::AMX);
    machine.kinds.push(lesser);
    machine.cores[2].kind = 1;
    let mut m = S::on(&machine);
    for n in [1, 2] {
        m.ok(0, stop(n, NODE_STOP_KILL));
    }
    m.run(10 * MS);
    m.no(0, start_on(1, 0b110), Error::Busy);
}

#[test]
fn a_node_that_reports_a_failure_is_accused_with_evidence_self() {
    let mut m = S::two();
    m.sys.fail(1, 0);
    m.run(MS / 10);
    assert_eq!(m.sys.member(1).evidence(), Evidence::SelfReported);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn a_hung_node_is_accused_stale_by_the_sweep() {
    let mut m = S::two();
    m.sys.hang(1, false);
    // The sweep observes it at its first look, and the heartbeat does not
    // move for STALE_NS.
    m.run(SWEEP_NS + STALE_NS + 10 * MS);
    let member = m.sys.member(1);
    assert!(
        matches!(member.state(), MemberState::Fenced | MemberState::Dead),
        "{member:?}"
    );
    assert_eq!(member.evidence(), Evidence::Stale);
}

#[test]
fn a_node_that_does_not_consume_a_message_waiting_in_its_ring_is_accused_stuck() {
    let mut m = S::two();
    // Node 1 is idle and hangs; its next armed timer (its sweep) is a
    // second away, so it is not stale before then.
    m.ok(1, Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    m.sys.hang(1, false);
    // Node 0 sends it a message: the ring is not empty, the interrupt is
    // sent, and the consumer index does not move for STUCK_NS.
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message::default(),
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(0, send), WAITS);
    m.run(STUCK_NS + 10 * MS);
    let member = m.sys.member(1);
    assert_eq!(member.evidence(), Evidence::Stuck, "{member:?}");
    // The sender's wait ends with `gone` at R0.
    assert_eq!(m.woken(0), Some(&Err(Error::Gone)));
}

#[test]
fn nodes_that_beat_are_never_accused() {
    let mut m = S::two();
    m.run(2 * SWEEP_NS + 10 * MS);
    assert_eq!(m.state(0), MemberState::Live);
    assert_eq!(m.state(1), MemberState::Live);
}

#[test]
fn a_hung_bootstrap_core_that_cannot_run_its_handler_is_unfenced() {
    // D-149: INIT reaches the bootstrap core whose cleared BSP flag the
    // image declares honoured; one hung in VMX root operation, where INIT
    // is blocked, cannot be confirmed while it is.
    let mut m = S::two();
    m.sys.set_vmx(0, true);
    m.sys.hang(0, false);
    m.ok(1, stop(0, NODE_STOP_KILL));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(0), MemberState::Unfenced);
    // There is no confirmation by time: it never becomes dead while its
    // stop is not confirmed.
    m.run(SWEEP_NS);
    assert_eq!(m.state(0), MemberState::Unfenced);
}

#[test]
fn a_core_in_vmx_root_operation_that_cannot_run_its_handler_is_unfenced_then_fenced_when_it_leaves_it()
 {
    let mut m = S::two();
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    // Its unit roles are taken over FENCE_NS + SETTLE_NS after the first
    // fence action.
    m.run(2 * MS);
    for role in unit_roles(&m) {
        assert_eq!(m.sys.role(role).holder(), 0);
    }
    // It leaves VMX root operation: the next try confirms the stop.
    m.sys.set_vmx(1, false);
    m.run(SWEEP_NS + 5 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn a_retired_core_is_unusable_for_a_fresh_start() {
    let mut m = S::two();
    m.ok(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_RETIRE,
            core: 1,
        },
    );
    assert_eq!(m.sys.member(1).evidence(), Evidence::Retire);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.no(0, start_on(1, 0b10), Error::Busy);
    let words = status(&mut m, 0, 1, 1);
    assert_ne!(words[1] & NODE_STATUS_UNUSABLE, 0);
}

#[test]
fn a_node_that_stops_itself_parks_and_another_drives_its_fence() {
    let mut m = S::two();
    m.ok(1, stop(1, NODE_STOP_KILL));
    m.settle();
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert_eq!(m.sys.member(1).driver(), 0);
}

#[test]
fn every_core_of_a_killed_node_of_several_cores_is_stopped_before_it_is_dead() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2, 3]]));
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.settle();
    for core in 1..4 {
        assert!(
            CoreWord::from_raw(m.sys.core_word(core))
                .unwrap()
                .is_marked()
        );
        assert_ne!(m.sys.park_request(core), 0);
    }
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    for core in 1..4 {
        assert!(matches!(m.sys.hardware(core), Hardware::Stub { .. }));
        assert_eq!(
            CoreWord::from_raw(m.sys.core_word(core)).unwrap().state(),
            CoreState::Free
        );
    }
}

#[test]
fn the_fence_of_each_architecture_confirms_every_core() {
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
        m.ok(0, stop(1, NODE_STOP_KILL));
        // ARM64 confirms at the second round's read, 11.2 ms (D-143).
        m.run(dodos_abi::FENCE_NS);
        assert_eq!(m.state(1), MemberState::Dead, "{arch:?}");
        for core in [1, 2] {
            assert_eq!(m.sys.hardware(core), Hardware::Off);
        }
    }
}

#[test]
fn an_arm64_fence_is_confirmed_by_off_after_the_request_never_by_the_handlers_own_write() {
    let mut m = S::on(&machine(Arch::Aarch64, 3, &[&[0], &[1, 2]]));
    m.ok(0, stop(1, NODE_STOP_KILL));
    // Every event but `CPU_OFF`: the handlers write `parked`, the cores do
    // not reach OFF, and the fence is not confirmed.
    for _ in 0..2_000 {
        let events: Vec<_> = m
            .sys
            .events()
            .into_iter()
            .filter(|e| !matches!(e, crate::system::Event::CpuOff { .. }))
            .collect();
        match events.first() {
            Some(&event) => {
                m.sys.fire(event);
                assert_eq!(m.sys.check(), Ok(()));
            }
            None => {
                let next = m.sys.now() + 100_000;
                m.sys.move_clock(next);
            }
        }
    }
    for core in [1, 2] {
        assert_eq!(m.sys.hardware(core), Hardware::Parking);
        assert_eq!(
            m.sys.canonical().core_beacons[core].parked,
            m.sys.park_request(core)
        );
    }
    assert!(matches!(
        m.state(1),
        MemberState::Fencing | MemberState::Unfenced
    ));
    // `CPU_OFF` completes: OFF confirms, and the node is reclaimed.
    m.run(SWEEP_NS + 10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn a_fence_whose_driver_dies_is_driven_again_by_another_node() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Core 2 cannot be stopped at once: it hangs without its handler, so
    // the fence of node 2 lasts.
    m.sys.hang(2, false);
    m.sys.set_vmx(2, true);
    m.ok(1, stop(2, NODE_STOP_KILL));
    assert_eq!(m.sys.member(2).driver(), 1);
    // The driver dies.
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(5 * MS);
    assert_eq!(m.sys.member(2).driver(), 0);
    m.sys.set_vmx(2, false);
    m.run(SWEEP_NS + 10 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
}

// Seal and resume: the replacement of a kernel.

#[test]
fn a_sealed_node_hands_its_unit_roles_and_resumes_with_its_generation() {
    let mut m = S::two();
    let notify = m.retype(1, ObjectKind::Notify, 0, 100);
    m.ok(1, notify);
    m.ok(0, stop(1, NODE_STOP_SEAL));
    assert_eq!(m.state(1), MemberState::Sealing);
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // Its word stays owned, its core parked.
    assert_eq!(
        CoreWord::from_raw(m.sys.core_word(1)).unwrap().state(),
        CoreState::Owned
    );
    assert!(matches!(m.sys.hardware(1), Hardware::Stub { .. }));
    for role in unit_roles(&m) {
        assert_eq!(
            (m.sys.role(role).state(), m.sys.role(role).holder()),
            (RoleState::Held, 0)
        );
    }
    // A resume: starting with the same generation, then live with its
    // objects as they were.
    m.ok(0, resume(1, 0));
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.member(1).generation(), 1);
    // The notify made before the seal is still there, unsignalled.
    assert_eq!(m.call(1, Syscall::Poll(s(100))), Err(Error::WouldWait));
}

#[test]
fn a_seal_writes_sealed_only_after_the_last_park_request_and_resumes_on_the_same_cores() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2, 3]]));
    // A thread of node 1 runs on its core 2.
    m.ok(1, super::place_first(1, 1));
    let worker = m.thread_on(1, 300, 2);
    m.settle();
    assert_eq!(m.sys.node(1).cores[1].thread, Some(worker));
    m.ok(0, stop(1, NODE_STOP_SEAL));
    let mut sealed_with_running = false;
    for _ in 0..200 {
        m.run(20_000);
        if m.state(1) == MemberState::Sealed {
            sealed_with_running |= (1..4).any(|c| m.sys.hardware(c) == Hardware::Running);
            break;
        }
    }
    assert_eq!(m.state(1), MemberState::Sealed);
    assert!(!sealed_with_running);
    m.ok(0, resume(1, 0));
    m.run(3 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    for core in 1..4 {
        assert_eq!(m.sys.hardware(core), Hardware::Running);
    }
    // Its threads run again where their contexts are placed.
    assert_eq!(m.sys.node(1).cores[1].thread, Some(worker));
}

#[test]
fn a_resume_with_an_image_that_cannot_read_the_block_accuses_itself() {
    let mut machine = Machine::example();
    machine.replacement = true;
    machine.replacement_reads_sealed = false;
    let mut m = S::on(&machine);
    m.ok(0, stop(1, NODE_STOP_SEAL));
    m.run(2 * MS);
    m.ok(0, resume(1, 1));
    m.run(2 * MS);
    assert_eq!(m.sys.member(1).evidence(), Evidence::SelfReported);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn the_replacement_image_needs_a_full_slot() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_SEAL));
    m.run(2 * MS);
    m.no(0, resume(1, 1), Error::Busy);
    m.ok(0, resume(1, 0));
}

#[test]
fn a_node_sealed_longer_than_seal_ns_is_accused_with_evidence_seal() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_SEAL));
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // The sweep never observes a sealed node for stale evidence.
    m.run(SEAL_NS - 10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    m.run(SWEEP_NS + 20 * MS);
    let member = m.sys.member(1);
    assert_eq!(member.evidence(), Evidence::Seal);
    assert!(matches!(
        member.state(),
        MemberState::Fenced | MemberState::Dead
    ));
}

#[test]
fn a_sealing_node_returns_to_live_if_no_other_node_is_live() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 0 asks node 1 to seal, then node 2 kills node 0 and node 2
    // fails: node 1 is left alone and does not seal.
    m.ok(0, stop(1, NODE_STOP_SEAL));
    // The seal began (the final review's E10: only the end was asserted).
    assert_eq!(m.state(1), MemberState::Sealing);
    m.ok(2, stop(0, NODE_STOP_KILL));
    m.sys.fail(2, 0);
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Live);
}

#[test]
fn a_node_dying_while_starting_is_accused_stale_after_start_ns() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(10 * MS);
    // The core hangs before it reaches its entry: the node never beats.
    m.sys.hang(1, true);
    m.ok(0, start_on(1, 0b10));
    assert_eq!(m.state(1), MemberState::Starting);
    m.run(START_NS + SWEEP_NS + 20 * MS);
    let member = m.sys.member(1);
    assert_eq!(member.evidence(), Evidence::Stale);
    assert_eq!(member.state(), MemberState::Dead);
    // Its next fresh start has generation g + 2.
    m.ok(0, start_on(1, 0b10));
    assert_eq!(m.sys.member(1).generation(), 3);
}

// The park stub's lost wake-up (16 November 2026).

#[test]
fn a_wake_lost_in_the_park_stubs_window_is_sent_again_every_millisecond_until_the_stub_confirms() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // Core 1 is in the park stub, between its read of the start request
    // and its halt: the NMI of the start reaches it there and starts
    // nothing.
    m.sys.set_window(1);
    m.ok(0, start_on(1, 0b10));
    m.settle();
    assert_eq!(m.state(1), MemberState::Starting);
    assert_eq!(m.sys.canonical().core_beacons[1].started, 0);
    // The stub halts; a millisecond later the starter sends the wake again,
    // and the stub confirms.
    m.sys.fire(crate::system::Event::StubHalt { core: 1 });
    m.run(2 * MS);
    assert_ne!(m.sys.canonical().core_beacons[1].started, 0);
    assert_eq!(m.state(1), MemberState::Live);
    let wakes = m
        .sys
        .sent
        .iter()
        .filter(|a| matches!(a, (_, 0, HwAction::Start { core: 1 })))
        .count();
    assert!(wakes >= 2, "{wakes}");
}

#[test]
fn a_crash_record_of_reason_fenced_is_written_by_the_handler_of_a_kernel_that_runs() {
    let mut m = S::two();
    m.ok(0, stop(1, NODE_STOP_KILL));
    m.run(2 * MS);
    let record = m.sys.canonical().crash.areas[1].record;
    assert_eq!(record.map(|r| r.reason), Some(CrashReason::Fenced as u32));
}
