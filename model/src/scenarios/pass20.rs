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
fn ready_untyped(arch: Arch, cores: usize) -> (N, ObjId) {
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
    while k.call(k.t, Syscall::CapDelete(s(390))) == Err(Error::Again) {}
    assert_eq!(k.m.threads[&thread].task, None);
    assert_eq!(k.m.threads[&thread].marks & untrusted, untrusted);
}
