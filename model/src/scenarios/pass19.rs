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
    assert_eq!(
        (word.state(), word.root()),
        (Some(AttachState::Attached), root)
    );
    assert_eq!(
        m.sys.canonical().classes[&page],
        PageClass::iospace_root(1).unwrap()
    );
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
    assert_eq!(
        m.sys.canonical().classes[&page],
        PageClass::iospace_root(0).unwrap()
    );
    assert!(!m.sys.canonical().identifiers.contains_key(&key));
}

#[test]
fn a_rebuild_writes_the_entry_of_an_attached_device_whose_holder_died_before_writing_it() {
    let mut m = three();
    attached(&mut m, 0);
    // The holder moved the word to attached and died before its context
    // entry was written: the successor writes it from node 0's report.
    m.sys.edit_table(|t| {
        t.units
            .entry(IOMMU_ROLE)
            .or_default()
            .entries
            .remove(&(DEVICE as u16));
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
        t.units
            .entry(IOMMU_ROLE)
            .or_default()
            .entries
            .remove(&(DEVICE as u16));
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
    assert_eq!(
        m.sys.canonical().attach_word(DEVICE as u32),
        AttachWord::NONE
    );
}

#[test]
fn a_root_whose_detach_the_unit_never_confirms_never_turns_back_into_data() {
    let mut m = three();
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let page = root / m.sys.canonical().geometry.page();
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
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
    assert_eq!(
        m.sys.canonical().classes.get(&page),
        PageClass::iospace_root(1).as_ref()
    );
}

// Finding 9: the bound of the line words, at exactly `IRQ_LINES_MAX`.

#[test]
fn a_line_at_irq_lines_max_is_unsupported_and_the_last_word_is_claimable() {
    let max = dodos_abi::IRQ_LINES_MAX as u32;
    // An IOAPIC that serves GSIs up to IRQ_LINES_MAX + 3: the firmware lists
    // more lines than the table of line words holds.
    let mut machine = one_node(Arch::X86_64, 1);
    machine.ioapics.push(crate::boot::IoApic {
        first_gsi: max - 4,
        inputs: 8,
        segment: 0,
    });
    let mut k = N::on(&machine);
    let claim = |gsi: u32, slot: u32, vector: u64| Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(gsi, false, false).raw(),
        core: 0,
        vector,
        destination: s(slot),
    };
    // The word at IRQ_LINES_MAX does not exist: `unsupported`.
    assert_eq!(k.call(k.t, claim(max, 400, 48)), Err(Error::Unsupported));
    assert!(!k.m.table.lines.contains_key(&max));
    // The last word is claimable.
    assert!(k.call(k.t, claim(max - 1, 401, 49)).is_ok());
    assert!(k.m.table.lines.contains_key(&(max - 1)));
    // A table with a word at IRQ_LINES_MAX is reported.
    let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(0), k.m.table.generation(0));
    k.m.table.lines.insert(max, taken.raw());
    assert_eq!(k.m.check(), Err("96: a line word beyond the last"));
}

// Finding 8: R0 once for one generation.

#[test]
fn r0_run_twice_for_one_generation_is_reported() {
    let machine = one_node(Arch::X86_64, 1);
    let mut k = N::on(&machine);
    k.m.r0_runs.insert((1, 3), 1);
    assert_eq!(k.m.check(), Ok(()));
    k.m.r0_runs.insert((1, 3), 2);
    assert_eq!(k.m.check(), Err("R0: run twice for one generation"));
}

// Finding 16: the completing core writes done into the requester's part,
// whichever core the requester is.

#[test]
fn the_core_that_completes_a_request_of_core_1_writes_core_1s_state_word_done() {
    use super::cores::{FRAME, two_threads};
    use crate::cores::RequestState;
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    // Core 0 caches the translation too; core 1's thread unmaps.
    k.m.at.0 = 0;
    k.m.cache_here();
    let before = k.m.cores[0].request_state;
    assert_eq!(k.call(other, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    assert_eq!(k.m.cores[1].request_state, RequestState::InUse);
    // Core 0's acknowledgement completes core 1's request.
    k.m.take_interrupts(0);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.cores[1].request_state, RequestState::Done);
    assert_eq!(k.m.cores[1].request, None);
    assert_eq!(k.m.cores[0].request_state, before);
}

// Finding 17: a late wake of a running core starts nothing, also when its
// word carries the fence mark (a stop would park it there).

#[test]
fn a_late_start_wake_of_a_running_marked_core_still_starts_and_stops_nothing() {
    use crate::system::Hardware;
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
        m.sys.edit_table(|t| {
            let word = dodos_abi::CoreWord::from_raw(t.core_words[1]).expect("a word");
            t.core_words[1] = word.marked().raw();
        });
        let parked = m.sys.canonical().core_beacons[1].parked;
        // `CPU_ON` or `sbi_hart_start` is no stop: the core runs on.
        m.sys.late_wake(1);
        assert_eq!(m.sys.hardware(1), Hardware::Running, "{arch:?}");
        assert_eq!(m.sys.canonical().core_beacons[1].parked, parked, "{arch:?}");
    }
}

// D-136: only the machine start-up fills the replacement slot.

#[test]
fn a_resume_with_kernel_image_1_and_an_empty_slot_is_busy_and_no_call_fills_the_slot() {
    use super::pass17::seal;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    assert!(!m.sys.canonical().replacement);
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let resume = Syscall::NodeStart {
        node: node_cap(1),
        mode: dodos_abi::NODE_START_RESUME,
        image: dodos_abi::NODE_IMAGE_REPLACEMENT,
        module: 0,
        window: 0,
        mask: 0,
    };
    assert_eq!(m.call(0, resume), Err(Error::Busy));
    assert!(!m.sys.canonical().replacement);
    // A slot written after the start-up is reported.
    m.sys.edit_table(|t| t.replacement = true);
    assert_eq!(
        m.sys.check(),
        Err("D-136: the replacement slot written after the machine start-up".into())
    );
}

// Finding 11: a unit role of an unfenced holder is taken over FENCE_NS +
// SETTLE_NS after the first fence action, not before.

#[test]
fn an_unfenced_holders_unit_role_is_taken_over_at_fence_ns_and_settle_ns_after_the_first_action() {
    let mut m = three();
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, kill(1));
    m.run(MS);
    let generation = m.sys.member(1).generation();
    let begun = m.sys.canonical().fences[1]
        .begun(generation)
        .expect("begun");
    let due = begun + dodos_abi::FENCE_NS + dodos_abi::SETTLE_NS;
    m.sys.advance(due - dodos_abi::SETTLE_NS / 2);
    assert_eq!(m.state(1), MemberState::Unfenced);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    m.sys.advance(due + dodos_abi::SETTLE_NS / 2);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
}

// Finding 14: the start of a core that never takes its wake ends at
// START_NS after it began: the node that starts it sends no wake after it
// (Node slots and core words, the start request; the accusation of the
// node itself comes from an observation of the gate, Detection).

#[test]
fn a_start_whose_core_never_takes_its_wake_ends_at_start_ns() {
    use super::pass17::fresh;
    let mut m = S::two();
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.sys.hang(1, true);
    m.ok(0, fresh(1, 0b10));
    let since = m.sys.node(0).starts[&1].since;
    m.sys.advance(since + dodos_abi::START_NS - MS / 2);
    assert!(m.sys.node(0).starts.contains_key(&1));
    m.sys.advance(since + dodos_abi::START_NS + MS / 4);
    assert!(!m.sys.node(0).starts.contains_key(&1));
}

// Finding 15: a node that resumes beside an unfenced peer does not become
// live while it still holds a remote capability of that peer (R2 "is done
// when no such record remains").

#[test]
fn a_resume_beside_an_unfenced_peer_waits_until_r2_removed_every_remote_capability_of_it() {
    use super::pass17::{resume, seal};
    use crate::{Message, Range, Transfer};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 2 grants node 1 a frame, which node 1 copies 40 times: R2's
    // removal of its root takes two steps of 32 units.
    let frame = m.retype(2, ObjectKind::Frame, 1, 101);
    m.ok(2, frame);
    let reply = m.retype(1, ObjectKind::Reply, 0, 100);
    m.ok(1, reply);
    let recv = Syscall::Recv {
        endpoint: s(dodos_abi::START_SLOT_MAILBOX),
        reply: s(100),
        range: Range {
            first: s(110),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(m.call(1, recv), WAITS);
    let rights = dodos_abi::Rights::READ
        .union(dodos_abi::Rights::DERIVE)
        .bits();
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: Message {
            label: 7,
            words: vec![7],
            caps: vec![Transfer::Derive(s(101), rights)],
            ..Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(2, send), WAITS);
    m.settle();
    for n in 0..40 {
        m.ok(
            1,
            Syscall::CapDerive {
                source: s(110),
                destination: s(600 + n),
                rights,
                badge: 0,
            },
        );
    }
    let held = |m: &S| {
        let node = m.sys.node(1);
        let remote = |object: &ObjId| node.remotes.get(object).is_some_and(|r| r.node == 2);
        node.caps.values().filter(|c| remote(&c.object)).count()
    };
    assert_eq!(held(&m), 41);
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    m.sys.set_vmx(2, true);
    m.sys.hang(2, false);
    m.ok(0, kill(2));
    m.run(dodos_abi::FENCE_NS + 10 * MS);
    assert_eq!(m.state(2), MemberState::Unfenced);
    m.ok(0, resume(1));
    let mut live = false;
    for _ in 0..100_000 {
        if m.sys.settle(1) == 0 {
            let now = m.sys.now();
            m.sys.advance(now + 50_000);
        }
        if m.state(1) == MemberState::Live {
            live = true;
            break;
        }
    }
    assert!(live);
    assert_eq!(held(&m), 0);
}
