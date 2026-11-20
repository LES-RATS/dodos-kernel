//! The scenarios of pass 20 (D-138): the lock per object left the contract
//! of version 1, and what stays is true under one node lock: the watermark
//! of an untyped returns after a barrier, marks are plain words written
//! under the node lock, the lock-free signal takes no lock only when it
//! wakes no one, and the bounds of one node lock. Each test is named after
//! the sentence it guards.

use super::*;
use crate::cores::{Completes, Flush};
use crate::locks::{Cost, LockId, Measured, Step};

/// A node of `cores` cores on `arch`; an untyped of 64 KB at slot 350, a
/// context made from it at 351 and destroyed: its watermark is ready to
/// return. Returns the node and the untyped.
pub(super) fn ready_untyped(arch: Arch, cores: usize) -> (N, ObjId) {
    let mut k = N::on(&one_node(arch, cores));
    k.ok(k.retype(ObjectKind::Untyped, 1 << 16, 350));
    k.ok(retype_from(350, ObjectKind::Sched, 0, 351));
    let untyped = k.m.object_at(k.t, s(350)).unwrap();
    assert!(k.m.untypeds[&untyped].watermark > 0);
    k.ok(Syscall::CapDelete(s(351)));
    (k, untyped)
}

// The memory of a destroyed object is not reused while a core may still
// reach it without the lock (A node of several cores, D-138).

#[test]
fn a_watermark_ready_to_return_returns_only_after_a_barrier_asks_every_other_core() {
    let (mut k, untyped) = ready_untyped(Arch::X86_64, 3);
    let epoch = k.m.kernel_epoch;
    assert_eq!(k.m.untypeds[&untyped].ready, Some(epoch));
    // The first retype starts the barrier and returns `again`, nothing
    // changed: every other core is asked, idle ones included.
    let again = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, again.clone()), Err(Error::Again));
    let request = k.m.cores[0].request.clone().expect("the barrier");
    assert_eq!(request.flush, Flush::Nothing);
    assert_eq!(request.completes, Completes::Barrier(epoch));
    assert_eq!(request.asked, 0b110);
    assert!(
        k.m.cores[2].idle.mark,
        "core 2 is idle and asked all the same"
    );
    assert!(k.m.untypeds[&untyped].ready.is_some());
    assert_eq!(k.m.object_at(k.t, s(352)), None);
    // While core 1 has not answered, the watermark stays.
    k.m.take_interrupts(2);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.call(k.t, again.clone()), Err(Error::Again));
    assert!(k.m.barrier_epoch < epoch);
    // Every asked core answered: the barrier covers the epoch, and the
    // watermark returns at the next retype.
    k.interrupts();
    assert_eq!(k.m.barrier_epoch, epoch);
    assert_eq!(k.call(k.t, again), DONE);
    assert_eq!(k.m.untypeds[&untyped].ready, None);
    assert_eq!(
        k.m.untypeds[&untyped].watermark,
        dodos_abi::NOTIFY_BYTES as u64
    );
}

#[test]
fn the_barrier_uses_the_request_on_arm64_too() {
    let (mut k, untyped) = ready_untyped(Arch::Aarch64, 2);
    let again = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, again.clone()), Err(Error::Again));
    assert!(matches!(
        k.m.cores[0].request,
        Some(ref r) if r.completes == Completes::Barrier(k.m.kernel_epoch)
    ));
    assert!(k.m.untypeds[&untyped].ready.is_some());
    k.interrupts();
    assert_eq!(k.call(k.t, again), DONE);
}

#[test]
fn on_a_node_of_one_core_the_watermark_returns_at_once() {
    let (mut k, untyped) = ready_untyped(Arch::X86_64, 1);
    assert_eq!(k.m.untypeds[&untyped].ready, None);
    assert_eq!(k.m.untypeds[&untyped].watermark, 0);
    k.ok(retype_from(350, ObjectKind::Notify, 0, 352));
    assert!(k.m.cores[0].request.is_none());
}

#[test]
fn a_barrier_in_flight_that_covers_the_epoch_is_not_started_again() {
    let (mut k, _) = ready_untyped(Arch::X86_64, 2);
    let again = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, again.clone()), Err(Error::Again));
    let started = k.m.cores[0].request.clone().unwrap().started;
    let ipis = k.m.pending_ipis().len();
    assert_eq!(k.call(k.t, again), Err(Error::Again));
    assert_eq!(k.m.cores[0].request.clone().unwrap().started, started);
    assert_eq!(k.m.pending_ipis().len(), ipis);
}

#[test]
fn a_give_of_a_block_whose_watermark_is_ready_to_return_waits_for_the_barrier() {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let shift = k.m.geometry().chunk_shift();
    let free = (0..1u64 << (k.m.machine.phys_bits - shift))
        .find(|&c| {
            k.m.ownership(c) == dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw()
        })
        .expect("a free chunk");
    let control = s(dodos_abi::START_SLOT_MEMORY_CONTROL);
    k.ok(Syscall::MemoryTake {
        control,
        kind: 0,
        place: free << shift,
        size: u64::from(shift),
        destination: s(220),
    });
    let block = k.m.object_at(k.t, s(220)).unwrap();
    k.ok(retype_from(220, ObjectKind::Notify, 0, 221));
    k.ok(Syscall::CapDelete(s(221)));
    assert!(k.m.untypeds[&block].ready.is_some());
    let give = Syscall::MemoryGive {
        control,
        untyped: s(220),
        node: 1,
    };
    // The barrier first: `again`, and the block is still this node's.
    let taken = k.m.ownership(free);
    assert_eq!(k.call(k.t, give.clone()), Err(Error::Again));
    assert_eq!(k.m.ownership(free), taken);
    assert!(k.m.object_at(k.t, s(220)).is_some());
    k.interrupts();
    // Then the give, which waits for the invalidation of the run.
    assert_eq!(k.call(k.t, give), WAITS);
    k.interrupts();
    assert_ne!(k.m.ownership(free), taken);
}

#[test]
fn a_barrier_epoch_beyond_the_kernel_epoch_is_reported() {
    let (mut k, _) = ready_untyped(Arch::X86_64, 2);
    k.m.barrier_epoch = k.m.kernel_epoch + 1;
    assert_eq!(
        k.m.check(),
        Err("epoch: the barrier epoch beyond the kernel epoch")
    );
}

#[test]
fn a_raise_of_the_kernel_epoch_other_than_at_a_last_destruction_is_reported() {
    let (mut k, untyped) = ready_untyped(Arch::X86_64, 2);
    // The event the trace is of began at the epoch before the raise.
    k.m.locks.entry_epoch = Some(k.m.kernel_epoch);
    k.m.kernel_epoch += 1;
    let epoch = k.m.kernel_epoch;
    k.m.locks.steps = vec![Step::Epoch(epoch)];
    assert_eq!(
        k.m.check(),
        Err("epoch: raised other than at the last destruction of an untyped")
    );
    k.m.locks.steps = vec![Step::Epoch(epoch), Step::Ready(untyped)];
    k.m.untypeds.get_mut(&untyped).unwrap().ready = Some(epoch);
    assert_eq!(k.m.check(), Ok(()));
}

// Marks are read and written under the node lock (Tasks and marks, D-138).

#[test]
fn a_delivery_writes_its_marks_into_the_receivers_task_alone() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let receiver = k.thread_on(300, 1);
    k.ok(k.retype(ObjectKind::Task, 0, 390));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(390),
    });
    let task = k.m.object_at(k.t, s(390)).unwrap();
    k.ok(k.retype(ObjectKind::Endpoint, 0, 310));
    k.ok(k.retype(ObjectKind::Reply, 0, 311));
    let recv = Syscall::Recv {
        endpoint: s(310),
        reply: s(311),
        range: crate::Range::NONE,
        wait: 0,
    };
    assert_eq!(k.call(receiver, recv), WAITS);
    let untrusted = dodos_abi::Marks::UNTRUSTED.bits();
    k.ok(Syscall::TaskMark {
        task: s(dodos_abi::START_SLOT_TASK),
        marks: untrusted,
    });
    let call = Syscall::Call {
        endpoint: s(310),
        message: crate::Message::default(),
        range: crate::Range::NONE,
        wait: 0,
    };
    assert_eq!(k.call(k.t, call), WAITS);
    // The receiver's effective marks are its task's: written there, and
    // its own word, which no rule reads while it is attached, is left.
    assert_eq!(k.m.tasks[&task].marks & untrusted, untrusted);
    assert_eq!(k.m.threads[&receiver].marks, 0);
    assert_eq!(k.m.marks(receiver).unwrap().bits() & untrusted, untrusted);
    // Detaching sets its own word to the marks of the task it leaves (the
    // receiver detaches itself: the caller waits for its answer).
    let detach = Syscall::TaskAttach {
        thread: s(300),
        task: SlotAddr::NONE,
    };
    assert_eq!(k.call(receiver, detach), DONE);
    assert_eq!(k.m.threads[&receiver].marks, k.m.tasks[&task].marks);
    assert_eq!(k.m.marks(receiver).unwrap().bits() & untrusted, untrusted);
}

// Locking: the lock-free signal (A node of several cores, D-138).

#[test]
fn a_signal_takes_the_node_lock_only_when_it_wakes_a_waiter() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let waiter = k.thread_on(300, 1);
    k.ok(k.retype(ObjectKind::Notify, 0, 360));
    let lock = |k: &N| k.m.locks.steps.contains(&Step::Lock(LockId::Node));
    // No one waits: one atomic OR, no lock.
    k.ok(Syscall::Signal(s(360)));
    assert!(!lock(&k), "{:?}", k.m.locks.steps);
    assert!(!k.m.locks.steps.contains(&Step::Pay(Cost::Lock)));
    // A thread takes the bits, then waits; the next signal wakes it and
    // takes the lock.
    let wait = Syscall::Wait(s(360), 0);
    assert!(matches!(k.call(waiter, wait.clone()), Ok(Output::Word(_))));
    assert_eq!(k.call(waiter, wait), WAITS);
    k.ok(Syscall::Signal(s(360)));
    assert!(lock(&k), "{:?}", k.m.locks.steps);
}

// The bounds of one node lock (A node of several cores, D-138; finding 7
// of the twelfth contract review).

#[test]
fn a_request_is_bounded_by_two_events_and_a_delivery() {
    let m = Measured {
        cores: 4,
        node_hold: 100,
        delivery: 50,
    };
    // E = N × H.
    assert_eq!(m.event_with_waits(), 400);
    // The asked core's event, the delivery, the interrupt's own event.
    assert_eq!(m.healthy_wait(), 2 * 400 + 50);
}

#[test]
fn the_start_up_refuses_a_core_answer_ns_that_leaves_out_the_event_of_the_interrupt() {
    let mut machine = one_node(Arch::X86_64, 4);
    machine.cores_supported = 4;
    assert!(Model::boot(&machine).is_ok());
    // CORE_ANSWER_NS is 10 ms. With H = 1.5 ms and D = 0.5 ms the earlier
    // bound, N × H + D = 6.5 ms, passes; 2 × N × H + D = 12.5 ms does not.
    machine.hold_ns = 1_500_000;
    machine.delivery_ns = 500_000;
    assert_eq!(
        Model::boot(&machine).err(),
        Some(crate::boot::Refusal::Floors)
    );
    // H = 1 ms: 2 × 4 ms + 0.5 ms = 8.5 ms, below 10 ms; STALE_NS (100 ms)
    // stays above CORE_ANSWER_NS + E.
    machine.hold_ns = 1_000_000;
    assert!(Model::boot(&machine).is_ok());
}

#[test]
fn the_destruction_of_a_block_whose_watermark_is_ready_to_return_waits_for_the_barrier() {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let shift = k.m.geometry().chunk_shift();
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
    let block = k.m.object_at(k.t, s(220)).unwrap();
    k.ok(retype_from(220, ObjectKind::Notify, 0, 221));
    k.ok(Syscall::CapDelete(s(221)));
    // The step that would destroy the block starts the barrier and returns
    // `again`: the run is still mapped in the kernel's own space.
    assert_eq!(k.call(k.t, Syscall::CapDelete(s(220))), Err(Error::Again));
    assert!(matches!(
        k.m.cores[0].request,
        Some(ref r) if matches!(r.completes, Completes::Barrier(_))
    ));
    assert!(k.m.kernel_runs.contains(&free));
    assert!(k.m.untypeds.contains_key(&block));
    // The step went no further than the barrier: the block is not taken
    // yet (the twelfth model review, finding 11).
    assert!(k.m.untypeds[&block].block.is_some());
    k.interrupts();
    // Then the destruction goes on: its own request for the run, then the
    // words written at its completion.
    let mut steps = 0;
    while k.call(k.t, Syscall::CapDelete(s(220))) == Err(Error::Again) {
        k.interrupts();
        steps += 1;
        assert!(steps < 8);
    }
    assert!(!k.m.kernel_runs.contains(&free));
}

// Found by pass 20, a fault of the model older than it: the removal of a
// block of a run on x86-64 in a node of several cores writes its words at
// the completion of its request, which forgets the block, and the checks
// then took it for an untyped of the start-up.

#[test]
fn the_removal_of_a_run_block_on_two_cores_writes_its_words_at_the_completion() {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let shift = k.m.geometry().chunk_shift();
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
    let taken = k.m.ownership(free);
    assert_eq!(k.call(k.t, Syscall::CapDelete(s(220))), Err(Error::Again));
    k.interrupts();
    assert_ne!(k.m.ownership(free), taken);
    assert_eq!(k.call(k.t, Syscall::CapDelete(s(220))), DONE);
    assert_eq!(k.m.object_at(k.t, s(220)), None);
}

#[test]
fn a_retype_on_another_core_while_a_barrier_covers_the_epoch_starts_none() {
    let (mut k, _) = ready_untyped(Arch::X86_64, 2);
    let other = k.thread_on(300, 1);
    let again = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, again.clone()), Err(Error::Again));
    assert!(k.m.cores[0].request.is_some());
    // Core 1's request is free, but the barrier of core 0 covers the
    // epoch: no second barrier.
    assert_eq!(k.call(other, again), Err(Error::Again));
    assert!(k.m.cores[1].request.is_none());
}

#[test]
fn a_thread_detached_by_the_destruction_of_its_task_keeps_its_marks() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let thread = k.thread_on(300, 1);
    k.ok(k.retype(ObjectKind::Task, 0, 390));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(390),
    });
    let untrusted = dodos_abi::Marks::UNTRUSTED.bits();
    k.ok(Syscall::TaskMark {
        task: s(390),
        marks: untrusted,
    });
    assert_eq!(k.m.threads[&thread].marks, 0);
    let deleted = (0..8)
        .map(|_| k.call(k.t, Syscall::CapDelete(s(390))))
        .find(|outcome| *outcome != Err(Error::Again));
    assert_eq!(deleted, Some(DONE));
    assert_eq!(k.m.threads[&thread].task, None);
    assert_eq!(k.m.threads[&thread].marks & untrusted, untrusted);
}

// Part 2: the twelfth contract review (D-139).

/// Node 0 of three makes an iospace and asks the attach of the device of
/// node 1's unit, not yet consumed; returns the machine and the root.
pub(super) fn attach_in_flight() -> (S, u64) {
    use super::units::{DEVICE, IOSPACE, attach, three};
    let mut m = three();
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    let node = m.sys.node(0);
    let space = node.object_at(m.t[0], s(IOSPACE)).unwrap();
    let root = node.aspaces[&space].root.unwrap();
    (m, root)
}

#[test]
fn an_attach_enters_the_header_only_at_its_answer_done() {
    use super::units::DEVICE;
    let (mut m, root) = attach_in_flight();
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(dodos_abi::AttachState::Pinned));
    assert_eq!(m.sys.node(0).census().devices, 0);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    assert_eq!(m.sys.node(0).census().devices, 1);
    assert_eq!(m.sys.canonical().attach_word(DEVICE as u32).root(), root);
}

#[test]
fn an_attach_answered_failed_on_a_pinned_word_writes_it_0_and_frees_the_identifier() {
    use super::units::{DEVICE, IOMMU_ROLE};
    let (mut m, root) = attach_in_flight();
    let page = root / m.sys.canonical().geometry.page();
    assert_eq!(
        m.sys.canonical().classes[&page],
        dodos_abi::PageClass::iospace_root(1).unwrap()
    );
    let node = m.sys.node_mut(0);
    let entered = node.attach_answered(
        IOMMU_ROLE,
        DEVICE as u16,
        root,
        dodos_abi::UnitResult::Failed,
    );
    assert!(!entered);
    assert_eq!(
        m.sys.canonical().attach_word(DEVICE as u32),
        dodos_abi::AttachWord::NONE
    );
    assert_eq!(
        m.sys.canonical().classes[&page],
        dodos_abi::PageClass::iospace_root(0).unwrap()
    );
    assert_eq!(m.sys.node(0).census().devices, 0);
}

#[test]
fn an_attach_the_holder_served_and_the_unit_did_not_confirm_stays_attached_out_of_the_header() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    let mut m = three();
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(dodos_abi::AttachState::Attached));
    assert_eq!(m.sys.node(0).census().devices, 0);
}

#[test]
fn one_request_at_a_time_per_device_and_an_attached_device_attached_again_succeeds() {
    use super::units::{DEVICE, IOSPACE, attach, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    // Already attached to this iospace, nothing in flight: success.
    let again = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, again.clone()), DONE);
    // A detach in flight: a second detach and an attach are `busy`.
    let detach = super::pass17::detach_of(again.clone());
    assert_eq!(m.call(0, detach.clone()), WAITS);
    assert_eq!(m.sys.node(0).census().detaching, 1);
    assert_eq!(m.sys.node(0).census().devices, 1);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    assert_eq!(m.sys.node(0).census().devices, 0);
    assert_eq!(
        m.sys.canonical().attach_word(DEVICE as u32),
        dodos_abi::AttachWord::NONE
    );
}

#[test]
fn a_detach_while_an_attach_is_in_flight_is_busy_and_the_destruction_waits_for_it() {
    use super::units::{DEVICE, IOSPACE, attach, three};
    let mut m = three();
    m.ok(0, super::place_first(0, 0));
    let other = m.thread_on(0, 300, 0);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call.clone()), WAITS);
    let detach = super::pass17::detach_of(call);
    // The attach word is pinned: a detach is `busy`, and the destruction
    // of the iospace waits for the attach's answer.
    assert_eq!(m.call_by(0, other, detach), Err(Error::Busy));
    assert_eq!(
        m.call_by(0, other, Syscall::CapDelete(s(IOSPACE))),
        Err(Error::Again)
    );
    m.run(30 * MS);
    // The answer brought the device into the header; the destruction then
    // detaches it and ends.
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        outcome = m.call_by(0, other, Syscall::CapDelete(s(IOSPACE)));
        if outcome != Err(Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    assert_eq!(
        m.sys.canonical().attach_word(DEVICE as u32),
        dodos_abi::AttachWord::NONE
    );
}

#[test]
fn a_report_carries_a_lines_mask_and_the_new_holder_writes_an_idle_line_masked() {
    let (mut m, _, _, _) = super::pass17::holder_requester_third();
    let role = m.sys.node(0).ioapic_role(5).unwrap();
    let holder = m.sys.role(role).holder();
    let a = (0..3).find(|&n| n != holder).unwrap();
    let killer = (0..3).find(|&n| n != holder && n != a).unwrap();
    // A level line, its irq idle: masked at the controller.
    let line = dodos_abi::IrqSource::line(5, true, false).raw();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: line,
        core: u64::from(a),
        vector: 48,
        destination: s(210),
    };
    // The claim is left in flight on purpose: the holder dies under it.
    let outcome = m.call(a, claim);
    // (The audit of lot 8a, D-8a F36: `DONE` was accepted too.)
    assert_eq!(outcome, WAITS);
    m.run(2 * MS);
    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, super::pass17::kill(holder));
    m.run(30 * MS);
    assert_ne!(m.sys.role(role).holder(), holder);
    // D-147: with a directed end of interrupt no node masks a level line
    // at its arrival, and the report reads it unmasked while its irq is
    // not dying, idle included: the new holder writes it unmasked, which
    // its claimer, not the holder, could never undo otherwise.
    let unit = &m.sys.canonical().units[&role];
    assert!(unit.lines.contains_key(&5));
    assert!(
        !unit.masked.contains(&5),
        "an idle level line written masked"
    );
}

#[test]
fn a_vector_whose_invalidation_a_failed_unit_did_not_confirm_is_not_claimed_again() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, 0, claim, 30 * MS);
    assert!(m.sys.node(0).object_at(m.t[0], s(210)).is_some());
    // The holder of the unit is another node: the removal is a request.
    assert_ne!(m.sys.role(IOMMU_ROLE).holder(), 0);
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        outcome = m.call(0, Syscall::CapDelete(s(210)));
        if outcome != Err(Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    assert!(!m.sys.canonical().retired.is_empty());
    let again = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(211),
    };
    assert_eq!(m.call(0, again), Err(Error::Busy));
}

#[test]
fn the_removal_of_an_msi_with_remapping_is_a_request_that_clears_its_entry() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, 0, claim, 30 * MS);
    let remapped = |m: &S| !m.sys.canonical().units[&IOMMU_ROLE].remap.is_empty();
    assert!(remapped(&m));
    assert_eq!(m.call(0, Syscall::CapDelete(s(210))), Err(Error::Again));
    assert!(!remapped(&m));
    m.run(30 * MS);
    assert_eq!(m.call(0, Syscall::CapDelete(s(210))), DONE);
    assert!(m.sys.canonical().retired.is_empty());
    let again = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(211),
    };
    let outcome = m.call(0, again);
    // The MSI waits for its unit requests (the audit of lot 8a, D-8a F36:
    // an immediate answer was accepted too).
    assert_eq!(outcome, WAITS);
}

#[test]
fn idle_until_is_written_on_the_clock_less_the_stopped_time() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let other = k.thread_on(300, 1);
    k.ok(k.retype(ObjectKind::Timer, 0, 310));
    k.ok(k.retype(ObjectKind::Notify, 0, 311));
    k.m.table.stopped_time = 7 * MS;
    let deadline = k.m.now + 50 * MS;
    let set = Syscall::TimerSet {
        timer: s(310),
        notify: s(311),
        deadline,
        tolerance: 0,
    };
    assert!(k.call(other, set).is_ok());
    assert_eq!(k.call(other, Syscall::Wait(s(311), 0)), WAITS);
    let number = k.m.cores[1].number as usize;
    let idle_until = k.m.table.core_beacons[number].idle_until.0;
    assert_ne!(idle_until, u64::MAX);
    assert_eq!(idle_until, k.m.hardware(1).unwrap() - 7 * MS);
}

/// The start of a request is read on the clock less the stopped time
/// (D-139). How a request is judged across a stop is pass 23's
/// `a_request_across_a_long_stop_is_late_only_on_the_clock_less_the_stopped_time`
/// (the final review's E23: this test, named after the judgement, never ran
/// it).
#[test]
fn a_request_reads_its_start_on_the_clock_less_the_stopped_time() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(1);
    let _ = k.thread_on(300, 1);
    // A stop already behind the node.
    k.m.table.stopped_time = 50 * MS;
    let later = k.m.now + 100 * MS;
    k.m.set_now(later);
    // A request of the core of the last event, started now and not
    // acknowledged.
    k.m.start_barrier();
    let at = *k.m.at;
    let request = k.m.cores[at].request.clone().expect("a request");
    assert_eq!(request.started, k.m.membership_now());
    assert_eq!(request.started, later - 50 * MS);
}

#[test]
fn a_second_detach_while_one_is_in_flight_is_busy() {
    use super::units::{DEVICE, IOSPACE, attach, attached, three};
    let mut m = three();
    m.ok(0, super::place_first(0, 0));
    let other = m.thread_on(0, 300, 0);
    attached(&mut m, 0);
    let detach = super::pass17::detach_of(attach(&m, 0, IOSPACE, DEVICE));
    assert_eq!(m.call(0, detach.clone()), WAITS);
    assert_eq!(m.call_by(0, other, detach), Err(Error::Busy));
    let again = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call_by(0, other, again), Err(Error::Busy));
}

#[test]
fn more_tables_unhooked_by_one_change_than_its_request_lists_are_reported() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.ok(k.retype(ObjectKind::Frame, 1, 350));
    let space = k.m.threads[&k.t].aspace.unwrap();
    let cap = *k.m.caps.keys().next().unwrap();
    let levels = k.m.levels(space) as usize;
    let aspace = k.m.aspaces.get_mut(&space).unwrap();
    for i in 0..(2 * (levels - 1) + 1) as u64 {
        aspace.unhooked.push((cap, i));
    }
    assert_eq!(
        k.m.check(),
        Err("59: more tables unhooked by one change than its request lists")
    );
}

#[test]
fn a_detach_answered_failed_takes_the_device_out_of_the_header_and_leaves_the_word_attached() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    let detach = super::pass17::detach_of(attach(&m, 0, IOSPACE, DEVICE));
    assert_eq!(m.call(0, detach), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    assert_eq!(m.sys.node(0).census().devices, 0);
    assert_eq!(m.sys.node(0).census().detaching, 0);
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(dodos_abi::AttachState::Attached));
}

// Mistral's eleventh model review: its surviving mutations killed (findings
// 4, 5 and 6; 1, 2 and 3 left with L2).

#[test]
fn the_holders_gate_writes_detached_only_from_attached() {
    use super::units::{DEVICE, IOMMU_ROLE};
    let (mut m, _) = attach_in_flight();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let before = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(before.state(), Some(dodos_abi::AttachState::Pinned));
    let op = crate::rings::UnitOp::Detach {
        device: DEVICE as u16,
        domain: 0,
        root: before.root(),
    };
    m.sys.node_mut(holder).detach_confirmed(IOMMU_ROLE, op);
    assert_eq!(m.sys.node(holder).table.attach_word(DEVICE as u32), before);
}

#[test]
fn no_gate_records_an_observation_while_the_release_word_is_raised() {
    let mut m = S::two();
    m.sys.hang(1, false);
    m.sys.edit_table(|t| t.release = true);
    let (sweep, stale) = (m.sys.canonical().sweep_ns, m.sys.canonical().stale_ns);
    m.run(sweep + stale + 10 * MS);
    assert!(m.sys.canonical().beacons[0].observations.is_empty());
    assert_eq!(m.state(1), MemberState::Live);
    m.sys.edit_table(|t| t.release = false);
    m.run(sweep + 2 * stale + 10 * MS);
    assert_ne!(m.state(1), MemberState::Live);
}

#[test]
fn a_table_unhooked_by_a_record_whose_invalidation_is_in_flight_stays_off_the_free_list() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let page = k.m.geometry().page();
    let entries = u64::from(k.m.geometry().entries);
    // An aspace made from an untyped: its tables come from it and go to
    // its free list (the start-up space's tables are the kernel's).
    k.ok(k.retype(ObjectKind::ASpace, 0, 360));
    let space = k.m.object_at(k.t, s(360)).unwrap();
    let mut caps = Vec::new();
    for (slot, at) in [
        (350u32, 4 * page * entries * entries),
        (351, 8 * page * entries * entries),
    ] {
        k.ok(k.retype(ObjectKind::Frame, 1, slot));
        k.ok(Syscall::ASpaceMap(crate::Map {
            space: s(360),
            frame: s(slot),
            untyped: s(k.tables()),
            address: at,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }));
        caps.push(k.m.cap(k.t, s(slot), Rights::NONE).unwrap());
    }
    // Both records cleared, as by two changes whose invalidations are in
    // flight (on x86-64 in a node of several cores); then the first one's
    // completes.
    k.m.clear_mapping(caps[0]);
    k.m.clear_mapping(caps[1]);
    let of = |k: &N, cap| {
        k.m.aspaces[&space]
            .unhooked
            .iter()
            .filter(|&&(by, _)| by == cap)
            .count()
    };
    let second = of(&k, caps[1]);
    assert!(of(&k, caps[0]) > 0 && second > 0);
    k.m.finish_unmapping(caps[0]);
    assert_eq!(of(&k, caps[0]), 0);
    assert_eq!(
        of(&k, caps[1]),
        second,
        "the second change's tables stay unhooked"
    );
}
