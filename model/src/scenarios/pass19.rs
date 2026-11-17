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

// G9 (D-137): the classes of an iospace by level, its root's count of
// devices, and the attach word of each device identifier (findings 12 of
// the tenth model review, B.3.1 and B.3.2 of Hydrogen's verification).

use super::pass17::{hold_identifier, holder_requester_third, iospace_root, judged, kill};
use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attached, three};
use crate::rings::UnitOp;
use dodos_abi::{AttachState, AttachWord, PageClass};

#[test]
fn the_holder_refuses_a_root_pinned_by_the_requester_unless_its_class_is_an_iospace_root() {
    let (mut m, holder, requester, _) = holder_requester_third();
    hold_identifier(&mut m, requester, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let root = iospace_root(&mut m, requester, 0x10);
    let attach = UnitOp::Attach {
        device: 0x10,
        domain: own,
        root,
    };
    let class = |m: &mut S, class: Option<PageClass>| {
        m.sys.edit_table(|t| {
            let page = root / t.geometry.page();
            std::sync::Arc::make_mut(&mut t.classes).insert(page, class.expect("a class"));
        });
    };
    // A lower table named as root: the unit would read its leaves as table
    // pointers into pages the node can write (B.3.1).
    class(&mut m, PageClass::iospace_table(1));
    assert!(judged(&mut m, holder, requester, 0, attach));
    // A data page of the requester, the attach word pinned with it.
    class(&mut m, Some(PageClass::DATA));
    assert!(judged(&mut m, holder, requester, 0, attach));
    // A root with no device pinned to it.
    class(&mut m, PageClass::iospace_root(0));
    assert!(judged(&mut m, holder, requester, 0, attach));
    class(&mut m, PageClass::iospace_root(1));
    assert!(!judged(&mut m, holder, requester, 0, attach));
    // Served: the holder's gate moved the word to attached.
    let word = m.sys.node(holder).table.attach_word(0x10);
    assert_eq!(word.state(), Some(AttachState::Attached));
}

#[test]
fn the_holder_refuses_an_attach_that_the_requesters_gate_did_not_pin() {
    let (mut m, holder, requester, _) = holder_requester_third();
    hold_identifier(&mut m, requester, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    // A root of the requester's class, but pinned for another device: the
    // requester's kernel forged the operands in its ring (B.3.2).
    let root = iospace_root(&mut m, requester, 0x11);
    let forged = UnitOp::Attach {
        device: 0x10,
        domain: own,
        root,
    };
    assert!(judged(&mut m, holder, requester, 0, forged));
    assert_eq!(m.sys.canonical().attach_word(0x10), AttachWord::NONE);
}

#[test]
fn an_attach_pins_its_root_and_a_detach_unpins_it_only_at_its_answer() {
    let mut m = three();
    attached(&mut m, 0);
    let key = DEVICE as u32;
    let root = {
        let node = m.sys.node(0);
        let iospace = node.object_at(m.t[0], s(IOSPACE)).unwrap();
        node.aspaces[&iospace].root.unwrap()
    };
    let page = root / m.sys.canonical().geometry.page();
    let word = m.sys.canonical().attach_word(key);
    assert_eq!((word.state(), word.root()), (Some(AttachState::Attached), root));
    assert_eq!(m.sys.canonical().classes[&page], PageClass::iospace_root(1).unwrap());
    // The owner's gate never writes 0 from attached.
    let role = IOMMU_ROLE;
    m.sys.node_mut(0).detach_done(role, DEVICE as u16);
    assert_eq!(m.sys.canonical().attach_word(key), word);
    // A detach: answered done once the holder has written detached; the
    // owner then writes 0, the identifier word free and the count 0.
    let detach = super::units::attach(&m, 0, 0, DEVICE);
    let detach = match detach {
        Syscall::IoSpaceAttach { iommu, device, .. } => Syscall::IoSpaceAttach {
            iommu,
            iospace: SlotAddr::NONE,
            device,
        },
        other => other,
    };
    assert_eq!(m.call(0, detach), WAITS);
    assert_ne!(m.sys.canonical().attach_word(key), AttachWord::NONE);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    assert_eq!(m.sys.canonical().attach_word(key), AttachWord::NONE);
    assert_eq!(m.sys.canonical().classes[&page], PageClass::iospace_root(0).unwrap());
    assert!(!m.sys.canonical().identifiers.contains_key(&key));
}

#[test]
fn a_rebuild_writes_the_entry_of_an_attached_device_whose_holder_died_before_writing_it() {
    let mut m = three();
    attached(&mut m, 0);
    // The holder moved the word to attached and died before its context
    // entry was written: the successor writes it from node 0's report.
    m.sys.edit_table(|t| {
        t.units.entry(IOMMU_ROLE).or_default().entries.remove(&(DEVICE as u16));
    });
    m.ok(0, kill(1));
    m.run(30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    let unit = &m.sys.canonical().units[&IOMMU_ROLE];
    assert!(unit.entries.contains_key(&(DEVICE as u16)));
}

#[test]
fn a_rebuild_writes_no_entry_for_a_reported_device_whose_attach_word_is_0() {
    let mut m = three();
    attached(&mut m, 0);
    // A report that names a device no gate pinned: no context entry.
    m.sys.edit_table(|t| {
        let root = t.attach_word(DEVICE as u32).root();
        t.attach.remove(&(DEVICE as u32));
        let page = root / t.geometry.page();
        let class = PageClass::iospace_root(0).expect("0");
        std::sync::Arc::make_mut(&mut t.classes).insert(page, class);
        t.units.entry(IOMMU_ROLE).or_default().entries.remove(&(DEVICE as u16));
    });
    m.ok(0, kill(1));
    m.run(30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    let unit = &m.sys.canonical().units[&IOMMU_ROLE];
    assert!(!unit.entries.contains_key(&(DEVICE as u16)));
}

#[test]
fn r6_clears_the_attach_word_of_a_device_of_a_dead_generation() {
    let mut m = three();
    attached(&mut m, 0);
    m.ok(2, kill(0));
    m.run(30 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    assert_eq!(m.sys.canonical().attach_word(DEVICE as u32), AttachWord::NONE);
}

#[test]
fn a_root_whose_detach_the_unit_never_confirms_never_turns_back_into_data() {
    let mut m = three();
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let page = root / m.sys.canonical().geometry.page();
    m.sys.edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    // The destruction detaches the device: a request the unit never
    // confirms; the holder answers failed after IOMMU_WAIT_NS.
    assert_eq!(m.call(0, Syscall::CapDelete(s(IOSPACE))), Err(Error::Again));
    m.run(30 * MS);
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        outcome = m.call(0, Syscall::CapDelete(s(IOSPACE)));
        if outcome != Err(Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    // The attach word stays attached, and the root keeps its class with
    // its count above 0: no gate turns it back into data.
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(AttachState::Attached));
    assert_eq!(m.sys.canonical().classes.get(&page), PageClass::iospace_root(1).as_ref());
}
