//! A node of several cores (D-128, D-129, D-131): placement, wake-ups
//! across cores, the mark *leaving its core*, the waits for a node's cores
//! (TLB, one core and several), the idle word, `CORE_ANSWER_NS`, and the
//! two locking variants over one layout.

use super::*;
use crate::cores::{Flush, Ipi};
use crate::locks::{Cost, LockId, Step};
use crate::{Locking, Record, Wait};
use dodos_abi::{CrashReason, START_SLOT_ASPACE};

const FRAME: u32 = 200;
const AT: u64 = 1 << 30;

fn map(frame: u32, address: u64, rights: Rights) -> Syscall {
    Syscall::ASpaceMap(crate::Map {
        space: s(START_SLOT_ASPACE),
        frame: s(frame),
        untyped: SlotAddr::NONE,
        address,
        first: 0,
        count: 1,
        rights: rights.bits(),
    })
}

fn rw() -> Rights {
    Rights::READ.union(Rights::WRITE)
}

/// A node of `n` cores with a second thread placed on core number 1,
/// running there, and a frame mapped in the start-up space that both
/// threads share, which core 1 has used (its translation is cached).
fn two_threads(arch: Arch, n: usize) -> (N, ObjId) {
    let mut k = N::on(&one_node(arch, n));
    // The first context leaves room on core 0 for another.
    k.place_first(0);
    let other = k.thread_on(300, 1);
    k.interrupts();
    assert_eq!(k.m.cores[1].thread, Some(other));
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    k.ok(map(FRAME, AT, rw()));
    // Core 1 touches the page: its TLB caches the translation.
    let space = k.m.threads[&other].aspace.expect("an aspace");
    k.m.at.0 = 1;
    k.m.cache_here();
    k.m.at.0 = 0;
    assert!(
        k.m.cores[1]
            .tlb
            .get(&space)
            .is_some_and(|c| c.contains_key(&AT))
    );
    (k, other)
}

// What is per physical core, and placement inside a node.

#[test]
fn a_node_whose_cores_lie_in_two_windows_is_reported() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.cores_set(), (0, 0b11));
    // Core numbers 0 and 64: no window and mask can name both.
    k.m.cores[1].number = 64;
    assert_eq!(
        k.m.check(),
        Err("WINDOW: a node whose cores lie in two windows of 64 core numbers")
    );
}

#[test]
fn a_node_of_n_cores_has_one_part_per_core_in_the_order_of_core_numbers() {
    for n in [1, 2, 4] {
        let k = N::on(&one_node(Arch::X86_64, n));
        assert_eq!(k.m.width(), n);
        let numbers: Vec<u32> = k.m.cores.iter().map(|c| c.number).collect();
        assert_eq!(numbers, (0..n as u32).collect::<Vec<_>>());
        // Every core word names the node with its generation.
        for core in 0..n {
            let word = dodos_abi::CoreWord::from_raw(k.m.table.core_words[core]).unwrap();
            assert!(word.names(0, k.m.generation_built()));
        }
    }
}

#[test]
fn sched_configure_places_a_context_on_a_core_of_its_node_and_admits_it_there() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    let before = k.m.cores[1].admitted;
    let thread = k.thread_on(300, 1);
    let context = k.m.context_of(thread).unwrap();
    assert_eq!(k.m.scheds[&context].core, 1);
    assert!(k.m.cores[1].admitted > before);
    // A core number outside the node is invalid.
    k.no(
        Syscall::SchedConfigure {
            sched: s(301),
            node: node_cap(0),
            core: 2,
            budget: 1_000_000,
            period: 10_000_000,
            class: 1,
        },
        Error::Invalid,
    );
}

#[test]
fn a_wake_up_across_cores_sends_one_reschedule_interrupt_and_none_on_one_core() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    let thread = k.thread_on(300, 1);
    // The resume on core 0 made a context of core 1 eligible: core 1, idle,
    // is sent the reschedule interrupt, once.
    assert_eq!(k.m.cores[1].ipis, [Ipi::Reschedule].into());
    assert!(k.m.cores[0].ipis.is_empty());
    k.interrupts();
    assert_eq!(k.m.cores[1].thread, Some(thread));
    // On a node of one core no interrupt is ever sent.
    let mut one = N::on(&one_node(Arch::X86_64, 1));
    one.place_first(0);
    one.thread_on(300, 0);
    assert!(one.m.pending_ipis().is_empty());
}

// A thread that runs on another core of its node.

#[test]
fn sched_configure_that_moves_a_running_context_marks_its_thread_leaving_and_waits_for_nothing() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.ok(Syscall::SchedConfigure {
        sched: s(301),
        node: node_cap(0),
        core: 0,
        budget: 1_000_000,
        period: 10_000_000,
        class: 1,
    });
    assert!(k.m.threads[&other].leaving);
    assert!(k.m.cores[1].ipis.contains(&Ipi::Leave));
    // While the mark is set no core runs it: it is not runnable.
    assert!(!Model::runnable_thread(&k.m.threads[&other]));
    // Core 1 takes the leave interrupt: it saves the registers, stops
    // running the thread and clears the mark.
    k.m.take_interrupts(1);
    assert_eq!(k.m.check(), Ok(()));
    assert!(!k.m.threads[&other].leaving);
    assert_ne!(k.m.cores[1].thread, Some(other));
    assert!(Model::runnable_thread(&k.m.threads[&other]));
}

#[test]
fn thread_suspend_of_a_thread_on_another_core_returns_again_while_it_is_marked() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.no(Syscall::ThreadSuspend(s(300)), Error::Again);
    assert!(k.m.threads[&other].leaving && k.m.threads[&other].suspended);
    // The eighth review: a second suspend, before the core left the
    // thread, returns `again` too, never success on a running thread.
    k.no(Syscall::ThreadSuspend(s(300)), Error::Again);
    k.m.take_interrupts(1);
    assert_eq!(k.m.cores[1].thread, None);
    k.ok(Syscall::ThreadSuspend(s(300)));
}

#[test]
fn the_registers_and_the_configuration_of_a_marked_thread_are_busy() {
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    k.no(Syscall::ThreadSuspend(s(300)), Error::Again);
    k.no(Syscall::ThreadReadRegs(s(300)), Error::Busy);
    let block = vec![0; dodos_abi::x86_64::REGS_WORDS];
    k.no(
        Syscall::ThreadWriteRegs {
            thread: s(300),
            block: block.clone(),
        },
        Error::Busy,
    );
    k.no(
        Syscall::ThreadConfigure {
            thread: s(300),
            cspace: SlotAddr::own(dodos_abi::START_SLOT_CSPACE),
            aspace: s(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(302),
        },
        Error::Busy,
    );
    k.m.take_interrupts(1);
    assert!(matches!(
        k.call(k.t, Syscall::ThreadReadRegs(s(300))),
        Ok(Output::Regs(_))
    ));
}

#[test]
fn task_revoke_returns_again_while_a_thread_of_the_task_is_marked() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Task, 0, 320));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(320),
    });
    k.no(Syscall::TaskRevoke(s(320)), Error::Again);
    assert!(k.m.threads[&other].leaving);
    // Every call returns `again` while the mark is set; the marked thread
    // is not detached.
    k.no(Syscall::TaskRevoke(s(320)), Error::Again);
    assert!(k.m.threads[&other].task.is_some());
    k.m.take_interrupts(1);
    k.ok(Syscall::TaskRevoke(s(320)));
    assert!(k.m.threads[&other].task.is_none() && k.m.threads[&other].suspended);
}

#[test]
fn a_removal_step_that_reaches_a_thread_running_on_another_core_returns_again() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.no(Syscall::CapDelete(s(300)), Error::Again);
    assert!(k.m.threads[&other].leaving);
    k.no(Syscall::CapDelete(s(300)), Error::Again);
    k.m.take_interrupts(1);
    // The removal goes on once the thread has left.
    let mut outcome = k.call(k.t, Syscall::CapDelete(s(300)));
    for _ in 0..8 {
        if outcome != Err(Error::Again) {
            break;
        }
        outcome = k.call(k.t, Syscall::CapDelete(s(300)));
    }
    assert_eq!(outcome, DONE);
    assert!(!k.m.threads.contains_key(&other));
}

#[test]
fn thread_cancel_never_waits_for_another_core() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.ok(Syscall::ThreadCancel(s(300), 5));
    assert!(!k.m.threads[&other].leaving);
    assert_eq!(k.m.threads[&other].cancelled_up_to, 5);
}

// TLB, one core and several.

#[test]
fn an_unmap_on_two_cores_leaves_the_record_unmapping_until_the_other_core_acknowledges() {
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    let cap = k.m.cap(k.t, s(FRAME), Rights::NONE).unwrap();
    let chunk = k.m.frames[&k.m.caps[&cap].object].base / k.m.geometry().chunk();
    let count = k.m.map_count(chunk);
    assert_eq!(k.call(k.t, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    assert!(matches!(k.m.caps[&cap].record, Record::Unmapping(_)));
    assert_eq!(k.m.threads[&k.t].wait, Wait::Cores(0));
    // The counts are lowered only at the completion.
    assert_eq!(k.m.map_count(chunk), count);
    let request = k.m.cores[0].request.clone().unwrap();
    assert_eq!(request.asked, 0b10);
    assert!(matches!(request.flush, Flush::Range { .. }));
    k.m.take_interrupts(1);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.cores[0].request, None);
    assert_eq!(k.m.caps[&cap].record, Record::Empty);
    assert_eq!(k.m.map_count(chunk), count - 1);
    assert_eq!(k.m.woken(k.t), Some(&DONE));
}

#[test]
fn an_unmap_on_arm64_invalidates_by_broadcast_in_one_step() {
    let (mut k, _) = two_threads(Arch::Aarch64, 2);
    let cap = k.m.cap(k.t, s(FRAME), Rights::NONE).unwrap();
    k.ok(Syscall::ASpaceUnmap(s(FRAME)));
    assert_eq!(k.m.caps[&cap].record, Record::Empty);
    assert!(k.m.cores.iter().all(|c| c.request.is_none()));
    assert!(k.m.cores[1].tlb.values().all(|c| !c.contains_key(&AT)));
}

#[test]
fn an_unmap_on_one_core_completes_in_its_call() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    k.ok(map(FRAME, AT, rw()));
    k.ok(Syscall::ASpaceUnmap(s(FRAME)));
    let cap = k.m.cap(k.t, s(FRAME), Rights::NONE).unwrap();
    assert_eq!(k.m.caps[&cap].record, Record::Empty);
}

#[test]
fn an_unmap_while_the_cores_request_is_in_use_is_again_and_changes_nothing() {
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME + 1));
    k.ok(map(FRAME + 1, AT + (1 << 21), rw()));
    // The request of core 0 is in use by the first thread's unmap; a
    // thread of core 0 that unmaps meanwhile gets `again`.
    let third = k.thread_on(310, 0);
    assert_eq!(k.call(k.t, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    let cap = k.m.cap(k.t, s(FRAME + 1), Rights::NONE).unwrap();
    let record = k.m.caps[&cap].record;
    assert_eq!(
        k.call(third, Syscall::ASpaceUnmap(s(FRAME + 1))),
        Err(Error::Again)
    );
    assert_eq!(k.m.caps[&cap].record, record);
}

#[test]
fn an_idle_core_is_not_asked_and_owes_the_invalidation_at_its_wake() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    let other = k.thread_on(300, 1);
    k.interrupts();
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    k.ok(map(FRAME, AT, rw()));
    let space = k.m.threads[&other].aspace.unwrap();
    k.m.at.0 = 1;
    k.m.cache_here();
    k.m.at.0 = 0;
    // The other thread stops: core 1 goes idle with the translation cached.
    k.no(Syscall::ThreadSuspend(s(300)), Error::Again);
    k.m.take_interrupts(1);
    assert!(k.m.cores[1].idle.mark);
    // The unmap does not ask the idle core: it sets its owes bit, and the
    // call completes at once.
    k.ok(Syscall::ASpaceUnmap(s(FRAME)));
    assert!(k.m.cores[1].idle.owes);
    assert!(k.m.cores[1].tlb[&space].contains_key(&AT));
    // At its wake the core clears both bits and invalidates every tag of
    // the node before it loads a root.
    k.ok(Syscall::ThreadResume(s(300)));
    k.interrupts();
    assert_eq!(k.m.cores[1].idle, crate::cores::IdleWord::default());
    assert!(
        !k.m.cores[1]
            .tlb
            .get(&space)
            .is_some_and(|c| c.contains_key(&AT))
    );
}

#[test]
fn a_protect_that_reduces_rights_waits_for_the_other_cores() {
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    let protect = Syscall::ASpaceProtect {
        aspace: s(START_SLOT_ASPACE),
        frame: s(FRAME),
        rights: Rights::READ.bits(),
    };
    assert_eq!(k.call(k.t, protect), WAITS);
    assert!(matches!(
        k.m.cores[0].request.as_ref().map(|r| &r.completes),
        Some(crate::cores::Completes::Protect)
    ));
    k.m.take_interrupts(1);
    assert_eq!(k.m.woken(k.t), Some(&DONE));
}

#[test]
fn a_give_invalidates_the_kernels_own_space_on_every_core_idle_ones_included() {
    // Node 0 of two cores beside a phantom node 1.
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let geometry = k.m.geometry();
    let shift = geometry.chunk_shift();
    let free = (0..1u64 << (k.m.machine.phys_bits - shift))
        .find(|&c| {
            k.m.ownership(c) == dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw()
        })
        .expect("a free chunk");
    k.ok(Syscall::MemoryTake {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: free << shift,
        size: u64::from(shift),
        destination: s(220),
    });
    assert!(k.m.kernel_runs.contains(&free));
    // Core 1 is idle, and translated the run in the kernel's own space
    // before it went idle.
    assert!(k.m.cores[1].idle.mark);
    k.m.cores[1].kernel_tlb.insert(free);
    let give = Syscall::MemoryGive {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        untyped: s(220),
        node: 1,
    };
    assert_eq!(k.call(k.t, give), WAITS);
    // The idle core is asked too, and no word is written before it
    // acknowledges.
    let request = k.m.cores[0].request.clone().expect("a request");
    assert_eq!(request.asked, 0b10);
    assert!(matches!(request.flush, Flush::Kernel(ref chunks) if chunks.contains(&free)));
    let taken = k.m.ownership(free);
    k.m.take_interrupts(1);
    assert_eq!(k.m.check(), Ok(()));
    assert!(!k.m.cores[1].kernel_tlb.contains(&free));
    assert_ne!(k.m.ownership(free), taken);
    assert_eq!(k.m.woken(k.t), Some(&DONE));
}

// A core of the node that stops answering while the others run.

#[test]
fn a_request_not_acknowledged_within_core_answer_ns_makes_the_node_accuse_itself() {
    // Node 0 of cores 0 and 1, node 1 of core 2.
    let mut sys = S::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    sys.ok(0, super::place_first(0, 0));
    let other = sys.thread_on(0, 300, 1);
    sys.settle();
    assert_eq!(sys.sys.node(0).cores[1].thread, Some(other));
    let frame = sys.retype(0, ObjectKind::Frame, 1, FRAME);
    sys.ok(0, frame);
    sys.ok(0, map(FRAME, AT, rw()));
    // Core 1 stops answering; node 0 unmaps: its request asks core 1,
    // which loaded the space's tag, and is never acknowledged.
    sys.sys.hang(1, true);
    assert_eq!(sys.call(0, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    sys.run(9 * MS);
    assert_eq!(sys.state(0), MemberState::Live);
    sys.run(2 * MS);
    // The core that started the request sees it at the due event it armed:
    // the node accuses itself, and the accusing core's crash record gives
    // reason 9.
    assert_ne!(sys.state(0), MemberState::Live);
    assert_eq!(
        sys.sys.member(0).evidence(),
        dodos_abi::Evidence::SelfReported
    );
    let record = sys.sys.canonical().crash.areas[0].record;
    assert_eq!(
        record.map(|r| r.reason),
        Some(CrashReason::CoreSilent as u32)
    );
}

#[test]
fn a_core_whose_heartbeat_goes_stale_while_it_is_not_idle_makes_its_node_accuse_itself() {
    // Node 0 of cores 0 and 1, each running a thread; node 1 of core 2.
    let mut sys = S::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    sys.ok(0, super::place_first(0, 0));
    let other = sys.thread_on(0, 300, 1);
    sys.settle();
    assert_eq!(sys.sys.node(0).cores[1].thread, Some(other));
    // Core 1 stops answering while it runs: no request is in flight, but
    // its heartbeat no longer moves. Core 0 sees it at its own events.
    sys.sys.hang(1, true);
    sys.run_by(dodos_abi::STALE_NS / 2, MS);
    assert_eq!(sys.state(0), MemberState::Live);
    sys.run_by(dodos_abi::STALE_NS / 2 + 60 * MS, MS);
    let member = sys.sys.member(0);
    assert_eq!(member.evidence(), dodos_abi::Evidence::SelfReported);
    let record = sys.sys.canonical().crash.areas[0].record;
    assert_eq!(
        record.map(|r| r.reason),
        Some(CrashReason::CoreSilent as u32)
    );
}

#[test]
fn a_core_that_runs_one_thread_with_no_other_event_beats_every_half_stale_ns() {
    // Node 0 of cores 0 and 1; a thread on core 1 on a context of a long
    // budget: no timer of its own for seconds.
    let mut sys = S::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    sys.ok(0, super::place_first(0, 0));
    let other = sys.thread_on(0, 300, 1);
    sys.ok(
        0,
        Syscall::SchedConfigure {
            sched: s(301),
            node: node_cap(0),
            core: 1,
            budget: 5_000_000_000,
            period: 10_000_000_000,
            class: 1,
        },
    );
    sys.settle();
    assert_eq!(sys.sys.node(0).cores[1].thread, Some(other));
    let before = sys.sys.canonical().core_beacons[1].heartbeat.0;
    sys.run_by(3 * dodos_abi::STALE_NS, MS);
    // Its heartbeat moved at least every STALE_NS / 2: nobody accused it.
    assert!(sys.sys.canonical().core_beacons[1].heartbeat.0 >= before + 5);
    assert_eq!(sys.state(0), MemberState::Live);
}

// Locking: two variants with one layout.

#[test]
fn a_node_of_one_core_takes_no_lock_and_pays_no_cost_of_several_cores() {
    for locking in [Locking::L1, Locking::L2] {
        let mut machine = one_node(Arch::X86_64, 1);
        machine.locking = locking;
        let mut k = N::on(&machine);
        k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
        k.ok(map(FRAME, AT, rw()));
        k.ok(Syscall::ASpaceUnmap(s(FRAME)));
        assert!(
            k.m.locks
                .steps
                .iter()
                .all(|s| !matches!(s, Step::Lock(_) | Step::Unlock(_) | Step::Pay(_)))
        );
    }
}

#[test]
fn under_l1_every_event_holds_the_node_lock_from_entry_to_exit_and_the_gate_lock_inside() {
    let mut machine = one_node(Arch::X86_64, 2);
    machine.locking = Locking::L1;
    let mut k = N::on(&machine);
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    k.ok(map(FRAME, AT, rw()));
    let steps: Vec<&Step> =
        k.m.locks
            .steps
            .iter()
            .filter(|s| matches!(s, Step::Lock(_) | Step::Unlock(_)))
            .collect();
    assert_eq!(steps.first(), Some(&&Step::Lock(LockId::Node)));
    assert_eq!(steps.last(), Some(&&Step::Unlock(LockId::Node)));
    assert!(steps.contains(&&Step::Lock(LockId::Gate)));
}

#[test]
fn under_l2_a_call_takes_its_locks_in_the_order_of_their_kind_the_gate_lock_last() {
    let mut machine = one_node(Arch::X86_64, 2);
    machine.locking = Locking::L2;
    let mut k = N::on(&machine);
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    k.ok(map(FRAME, AT, rw()));
    let locks: Vec<LockId> =
        k.m.locks
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Lock(l) => Some(*l),
                _ => None,
            })
            .collect();
    assert!(!locks.is_empty());
    assert!(locks.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(locks.last(), Some(&LockId::Gate));
    let paid =
        k.m.locks
            .steps
            .iter()
            .filter(|s| **s == Step::Pay(Cost::Lock))
            .count();
    assert_eq!(paid, locks.len());
}

#[test]
fn the_check_refuses_a_lock_order_the_contract_forbids() {
    let mut machine = one_node(Arch::X86_64, 2);
    machine.locking = Locking::L2;
    let mut k = N::on(&machine);
    k.ok(k.retype(ObjectKind::Frame, 1, FRAME));
    // The gate lock before an object lock.
    k.m.locks.steps = vec![
        Step::Pay(Cost::Lock),
        Step::Lock(LockId::Gate),
        Step::Pay(Cost::Lock),
        Step::Lock(LockId::Core(0)),
    ];
    assert!(k.m.check().is_err());
    // Object locks out of the order of their kind, the gate lock not taken.
    let thread = k.t;
    k.m.locks.steps = vec![
        Step::Pay(Cost::Lock),
        Step::Lock(LockId::Core(0)),
        Step::Pay(Cost::Lock),
        Step::Lock(LockId::Thread(thread)),
    ];
    assert!(k.m.check().is_err());
    // A spin for another core or a unit inside a hold.
    k.m.locks.steps = vec![Step::Spin(crate::locks::WaitKind::Unit)];
    assert!(k.m.check().is_err());
    // A node of one core that pays for a rule of several cores.
    let mut one = N::on(&one_node(Arch::X86_64, 1));
    one.m.locks.steps = vec![Step::Pay(Cost::Interrupt)];
    assert!(one.m.check().is_err());
}
