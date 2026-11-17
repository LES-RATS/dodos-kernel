//! The scenarios of pass 19 (D-137): the tenth review of the model
//! (`Docs/reviews/2026-11-17-model-review-10-deepseek.md`) judged finding by
//! finding, and what the eleventh review of the contract and its
//! verification changed in the contract, each a test named after the
//! sentence it guards and shown failing once under a mutation of the code
//! it guards.

use super::*;
use crate::locks::{LockId, Step};

// Finding 4: invariant 43 at a resume (`core_entered`).

#[test]
fn a_core_that_answers_again_replenishes_its_contexts_whose_instant_has_passed() {
    let machine = one_node(Arch::X86_64, 1);
    let mut k = N::on(&machine);
    k.place_first(0);
    let context = k.m.threads[&k.t].home.expect("a home context");
    let now = k.m.now;
    k.m.threads.get_mut(&k.t).unwrap().suspended = true;
    {
        let sched = k.m.scheds.get_mut(&context).unwrap();
        sched.remaining = 0;
        sched.throttled = Some((now, 1));
        let entered = sched.queue.map(|q| q.entered).unwrap_or(0);
        sched.queue = Some(crate::Queued {
            queue: crate::Queue::Sleeping,
            key: now,
            entered,
        });
    }
    k.m.cores[0].silent = true;
    // It holds while the core is silent: the context keeps its state.
    assert_eq!(k.m.check(), Ok(()));
    // What a resume does to the core: it answers again, and the context
    // whose replenishment instant has passed is replenished at once.
    k.m.core_entered(0);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.scheds[&context].throttled, None);
    assert!(k.m.scheds[&context].remaining > 0);
}

// Finding 5: any other result of a call sets the count of `again` to 0.

/// One call of the first thread whose try fails and whose second round
/// finds the receiver cancelled by core 1: an `again` of tries.
fn again_of_tries(k: &mut N, third: ObjId, call: &Syscall, round: u64) {
    let lock = super::pass18::first_try(k, k.t, call);
    let cancel = Syscall::ThreadCancel(s(300), 7 + round);
    k.m.hold(1, &[lock], Some((third, cancel)))
        .expect("a second core");
    assert_eq!(k.call(k.t, call.clone()), Err(Error::Again));
    let receiver = k.m.object_at(k.t, s(300)).unwrap();
    let recv = Syscall::Recv {
        endpoint: s(310),
        reply: s(311),
        range: crate::Range {
            first: s(380),
            count: 1,
        },
        wait: 8 + round,
    };
    assert_eq!(k.call(receiver, recv), WAITS);
}

#[test]
fn a_result_other_than_the_again_of_tries_sets_the_count_to_0() {
    let (mut k, _, third, call) = super::pass18::l2_delivery();
    again_of_tries(&mut k, third, &call, 0);
    again_of_tries(&mut k, third, &call, 1);
    assert_eq!(k.m.threads[&k.t].again_tries, 2);
    // A call of the same thread that succeeds, with no other core in an
    // event: the count goes back to 0.
    assert!(k.call(k.t, Syscall::CapInspect(s(310))).is_ok());
    assert_eq!(k.m.threads[&k.t].again_tries, 0);
    // So the next `again` of tries is the first of a new run, not an
    // escalation.
    again_of_tries(&mut k, third, &call, 2);
    assert_eq!(k.m.threads[&k.t].again_tries, 1);
    assert!(!k.m.locks.steps.contains(&Step::Quiet(true)));
}

// Finding 6: the fields of a waiting thread and its attached reply object
// are guarded by the endpoint's lock; a call that acts on the thread by its
// own capability locks the thread too.

#[test]
fn under_l2_a_delivery_wakes_the_receiver_under_the_endpoint_lock_and_takes_neither_its_lock_nor_its_reply_objects()
 {
    let (mut k, receiver, _, call) = super::pass18::l2_delivery();
    assert_eq!(k.call(k.t, call), WAITS);
    assert!(k.m.woken(receiver).is_some());
    let steps = &k.m.locks.steps;
    let locked = |id: LockId| {
        steps
            .iter()
            .any(|s| matches!(s, Step::Lock(l) | Step::Try(l) if *l == id))
    };
    assert!(locked(LockId::Endpoint(
        k.m.object_at(k.t, s(310)).unwrap()
    )));
    assert!(!locked(LockId::Thread(receiver)), "{steps:?}");
}

#[test]
fn under_l2_thread_cancel_locks_the_waiting_thread_and_the_endpoint_it_waits_on() {
    let (mut k, receiver, _, _) = super::pass18::l2_delivery();
    k.ok(Syscall::ThreadCancel(s(300), 0));
    assert_eq!(k.m.woken(receiver), Some(&Err(Error::Cancelled)));
    let steps = &k.m.locks.steps;
    let endpoint = LockId::Endpoint(k.m.object_at(k.t, s(310)).unwrap());
    let thread = steps
        .iter()
        .position(|s| *s == Step::Lock(LockId::Thread(receiver)));
    let waited = steps.iter().position(|s| *s == Step::Lock(endpoint));
    assert!(thread.is_some() && waited > thread, "{steps:?}");
}
