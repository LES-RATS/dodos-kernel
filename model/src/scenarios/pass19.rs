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

// The store-load order of the entry word and the quiet word (D-137): the
// four orders of two cores' accesses.

#[test]
fn with_the_ordering_operations_no_interleaving_lets_a_core_enter_while_an_escalation_runs_alone() {
    let ordered = crate::locks::entry_orders(true);
    // Six interleavings of a's two accesses with b's two.
    assert_eq!(ordered.len(), 6);
    assert!(!ordered.contains(&(true, true)), "{ordered:?}");
    // Without them, the store-buffer outcome is reachable: a reads the word
    // free while b reads a's entry word 0.
    let unordered = crate::locks::entry_orders(false);
    assert!(unordered.contains(&(true, true)), "{unordered:?}");
}

// A core that waits for the quiet word keeps its interrupts masked (D-137).

#[test]
fn an_arrival_on_a_core_that_waits_for_the_quiet_word_stays_pending_until_the_word_is_released() {
    let (mut k, _, _, _) = super::pass18::l2_delivery();
    k.ok(k.retype(ObjectKind::Notify, 0, 330));
    let msi = dodos_abi::IrqSource::msi(0, 3, 1, 0).raw();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: msi,
        core: 0,
        vector: 48,
        destination: s(340),
    };
    assert!(k.call(k.t, claim).is_ok());
    k.ok(Syscall::IrqBind {
        irq: s(340),
        notify: s(330),
    });
    let notify = k.m.object_at(k.t, s(330)).unwrap();
    // Core 1's escalated call holds the word: core 0's arrival is kept at
    // its controller, none of its steps done.
    k.m.quiet = Some(1);
    k.m.interrupt_on(0, 48);
    assert_eq!(k.m.notifies[&notify].word, 0);
    assert_eq!(k.m.masked.get(&0), Some(&vec![48]));
    assert_eq!(k.m.check(), Ok(()));
    // Released: core 0 takes it, and the notify is signalled.
    k.m.release_quiet();
    assert!(k.m.masked.is_empty());
    assert_ne!(k.m.notifies[&notify].word, 0);
    // An arrival kept with the word released is reported.
    k.m.masked.insert(2, vec![48]);
    assert_eq!(
        k.m.check(),
        Err("quiet word: an arrival kept pending with the word released")
    );
}

// The closed bound of an escalated call and the healthy wait that
// CORE_ANSWER_NS judges (D-137).

#[test]
fn the_bound_of_an_escalated_call_is_closed_and_core_answer_ns_covers_a_request_under_each_variant()
{
    use crate::locks::{Locking, Measured};
    let m = Measured {
        cores: 4,
        node_hold: 100,
        object_hold: 10,
        event: 1_000,
        one_core: 500,
        delivery: 50,
    };
    let e2 = 1_000 + 2 * Measured::K_MAX * 3 * 10;
    assert_eq!(m.event_with_waits(Locking::L2), e2);
    assert_eq!(m.event_with_waits(Locking::L1), 1_000 + 3 * 100);
    // own + E + (N − 1) × B1max.
    assert_eq!(m.escalated(700), 700 + e2 + 3 * 500);
    // L1: N × H + D; L2: 2 × E + (N − 1) × B1max + D.
    assert_eq!(m.healthy_wait(Locking::L1), 4 * 100 + 50);
    assert_eq!(m.healthy_wait(Locking::L2), 2 * e2 + 3 * 500 + 50);
    assert_eq!(Measured::K_MAX, 387);
}

#[test]
fn the_start_up_refuses_a_core_answer_ns_below_the_healthy_wait_of_l2_that_n_minus_1_times_h_passes()
 {
    let mut machine = one_node(Arch::X86_64, 4);
    machine.locking = crate::Locking::L2;
    machine.cores_supported = 4;
    assert!(Model::boot(&machine).is_ok());
    // h = 10 µs: E = E1 + 2 × 387 × 3 × 10 µs, about 23 ms, so a request's
    // wait passes 10 ms, though (N − 1) × H is 30 µs.
    machine.object_hold_ns = 10_000;
    assert_eq!(
        Model::boot(&machine).err(),
        Some(crate::boot::Refusal::Floors)
    );
}

// Marks only gain bits: ORed into the own word always, and into the task
// (D-137).

#[test]
fn a_delivery_ors_its_marks_into_the_receivers_own_word_and_its_task_and_takes_no_task_lock() {
    let (mut k, receiver, _, call) = super::pass18::l2_delivery();
    k.ok(k.retype(ObjectKind::Task, 0, 390));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(390),
    });
    let task = k.m.object_at(k.t, s(390)).unwrap();
    let untrusted = dodos_abi::Marks::UNTRUSTED.bits();
    // The caller's task is marked.
    k.ok(Syscall::TaskMark {
        task: s(dodos_abi::START_SLOT_TASK),
        marks: untrusted,
    });
    assert_eq!(k.call(k.t, call), WAITS);
    assert_eq!(k.m.tasks[&task].marks & untrusted, untrusted);
    assert_eq!(k.m.threads[&receiver].marks & untrusted, untrusted);
    let steps = &k.m.locks.steps;
    assert!(
        !steps
            .iter()
            .any(|s| matches!(s, Step::Lock(LockId::Task(_)) | Step::Try(LockId::Task(_)))),
        "{steps:?}"
    );
}

// The bound k counted again (D-137).

#[test]
fn the_bound_k_of_each_call_is_the_contracts() {
    use crate::locks::bound;
    let range = crate::Range::NONE;
    let call = Syscall::Call {
        endpoint: s(1),
        message: crate::Message::default(),
        range,
        wait: 0,
    };
    let send = Syscall::Send {
        target: s(1),
        message: crate::Message::default(),
    };
    let recv = Syscall::Recv {
        endpoint: s(1),
        reply: s(2),
        range,
        wait: 0,
    };
    let reply_recv = Syscall::ReplyRecv {
        reply: s(2),
        message: crate::Message::default(),
        endpoint: s(1),
        range,
        wait: 0,
    };
    let configure = Syscall::ThreadConfigure {
        thread: s(1),
        cspace: s(2),
        aspace: s(3),
        fault: s(4),
        page: s(5),
    };
    assert_eq!(
        [call, send, recv, reply_recv, configure].map(|c| bound(&c)),
        [33, 29, 40, 67, 21]
    );
    assert_eq!(bound(&Syscall::CapDelete(s(1))), 387);
    assert_eq!(bound(&Syscall::TaskRevoke(s(1))), 194);
    assert_eq!(bound(&Syscall::Yield), 12);
}

// The debugger's stopped time (D-137).

#[test]
fn a_node_sealed_before_a_long_stop_in_the_stub_is_not_accused_with_seal_at_the_release() {
    use super::pass17::seal;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // A stop of 30 SEAL_NS: no node accuses another while it lasts, and at
    // the release the seal is only as old as the time that was not stopped.
    m.sys.stub_stop(30 * m.sys.canonical().seal_ns);
    assert_eq!(m.state(1), MemberState::Sealed);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // The clock of membership goes on: SEAL_NS later, it is accused.
    m.run(m.sys.canonical().seal_ns + m.sys.canonical().sweep_ns);
    assert_ne!(m.state(1), MemberState::Sealed);
    assert_eq!(m.sys.member(1).evidence(), dodos_abi::Evidence::Seal);
}

// The sweep reads the seal of its sealed successors (D-137).

#[test]
fn a_sealed_node_that_nobody_needs_is_accused_with_seal_within_sweep_ns_and_seal_ns() {
    use super::pass17::seal;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    let sealed = m.sys.now();
    let bound = sealed + m.sys.canonical().seal_ns + m.sys.canonical().sweep_ns;
    m.sys.advance(bound);
    assert_ne!(m.state(1), MemberState::Sealed);
    assert_eq!(m.sys.member(1).evidence(), dodos_abi::Evidence::Seal);
}

// An unhooked table joins its free list only at the completion of the
// invalidation that covers its unhooking (Address spaces and their tables,
// D-137).

#[test]
fn a_table_unhooked_by_an_unmap_joins_the_free_list_only_at_the_completion_of_its_request() {
    let mut m = three();
    attached(&mut m, 0);
    let frame = m.retype(0, ObjectKind::Frame, 1, 160);
    m.ok(0, frame);
    let untyped = super::untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(
        0,
        Syscall::IoSpaceMap(crate::Map {
            space: s(IOSPACE),
            frame: s(160),
            untyped: s(untyped),
            address: 1 << 20,
            first: 0,
            count: 1,
            rights: dodos_abi::Rights::READ.bits(),
        }),
    );
    let space = m.sys.node(0).object_at(m.t[0], s(IOSPACE)).unwrap();
    let free = m.sys.node(0).aspaces[&space].free.len();
    // The unmap waits for the unit: the device may still hold a cached
    // entry of the old parent, so the tables it unhooked wait too.
    assert_eq!(m.call(0, Syscall::IoSpaceUnmap(s(160))), WAITS);
    let waiting: Vec<u64> = m.sys.node(0).aspaces[&space]
        .unhooked
        .iter()
        .map(|&(_, at)| at)
        .collect();
    assert!(!waiting.is_empty());
    assert_eq!(m.sys.node(0).aspaces[&space].free.len(), free);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    let aspace = &m.sys.node(0).aspaces[&space];
    assert!(aspace.unhooked.is_empty());
    assert!(waiting.iter().all(|at| aspace.free.contains(at)));
}

// The removal of a line's irq waits for a claim still asking on that line
// (Interrupts, the irq, D-137).

#[test]
fn the_removal_of_a_lines_irq_waits_while_an_entry_still_asks_for_that_line() {
    let (mut m, holder, a, _) = holder_requester_third();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, false, false).raw(),
        core: u64::from(a),
        vector: 48,
        destination: s(210),
    };
    let claimed = m.call(a, claim);
    if claimed == WAITS {
        m.settle();
    }
    let _ = holder;
    let thread = m.t[a as usize];
    let node = m.sys.node_mut(a);
    let irq = node.object_at(thread, s(210)).expect("the irq");
    // An entry still asks the line's redirection entry.
    let index = node.unit_requests.iter().position(Option::is_none).unwrap();
    node.unit_requests[index] = Some(crate::units::UnitEntry {
        role: node.ioapic_role(5).unwrap(),
        op: UnitOp::Redirection {
            gsi: 5,
            claimed: true,
            level: false,
            low: false,
            target: crate::rings::Target::Masked,
        },
        epoch: 0,
        stage: crate::units::Stage::Queued,
        waiter: crate::units::Waiter::Thread(None),
        record: None,
        order: 0,
        words: None,
        segment: None,
        failed: false,
        then: None,
    });
    assert!(matches!(node.irq_removal(irq), crate::caps::Undone::Wait));
    assert!(!node.irqs[&irq].removing);
}

// The delivery of a fault is an event, not a call: under contention it
// follows the path of an arrival and never returns `again` (D-137).

#[test]
fn under_l2_a_fault_waits_for_the_core_that_holds_its_endpoint_and_is_delivered() {
    let (mut k, receiver, _, _) = super::pass18::l2_delivery();
    // A thread on core 1 whose fault endpoint is the endpoint the receiver
    // waits on.
    let mut calls = super::thread_calls(k.ut, k.m.node, 500, 1);
    if let Syscall::ThreadConfigure { fault, .. } = &mut calls[3] {
        *fault = s(310);
    }
    for call in calls {
        k.ok(call);
    }
    let faulter = k.m.object_at(k.t, s(500)).unwrap();
    k.interrupts();
    // Core 2 is in an event that holds the endpoint's lock.
    let endpoint = k.m.object_at(k.t, s(310)).unwrap();
    k.m.hold(2, &[LockId::Endpoint(endpoint)], None)
        .expect("a third core");
    k.m.fault(faulter, dodos_abi::FaultKind::Undefined, [6, 0, 0]);
    assert_eq!(k.m.check(), Ok(()));
    // It waited for core 2's event, then delivered the fault as a call; no
    // `again`, no escalation.
    assert!(k.m.contended.is_empty());
    assert!(matches!(k.m.woken(receiver), Some(Ok(Output::Message(_)))));
    assert!(!k.m.locks.steps.contains(&Step::Quiet(true)));
}

// `recv` and `reply_recv` make every try before the release of their
// lends (D-137).

#[test]
fn under_l2_a_recv_whose_try_fails_returns_again_with_its_lends_still_held() {
    let mut machine = one_node(Arch::X86_64, 3);
    machine.locking = crate::Locking::L2;
    let mut k = N::on(&machine);
    k.place_first(0);
    let caller = k.thread_on(320, 1);
    k.ok(k.retype(ObjectKind::Endpoint, 0, 310));
    k.ok(k.retype(ObjectKind::Reply, 0, 311));
    k.ok(k.retype(ObjectKind::Notify, 0, 360));
    // A passive server: no context of its own.
    k.ok(k.retype(ObjectKind::Frame, 1, 342));
    k.ok(k.retype(ObjectKind::Thread, 0, 340));
    k.ok(Syscall::ThreadConfigure {
        thread: s(340),
        cspace: SlotAddr::own(dodos_abi::START_SLOT_CSPACE),
        aspace: s(dodos_abi::START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: s(342),
    });
    k.ok(Syscall::ThreadResume(s(340)));
    let server = k.m.object_at(k.t, s(340)).unwrap();
    let recv = |range| Syscall::Recv {
        endpoint: s(310),
        reply: s(311),
        range,
        wait: 0,
    };
    assert_eq!(k.call(server, recv(crate::Range::NONE)), WAITS);
    // The first thread calls: the server runs on its lent context.
    let call = |caps| Syscall::Call {
        endpoint: s(310),
        message: crate::Message {
            caps,
            ..crate::Message::default()
        },
        range: crate::Range::NONE,
        wait: 0,
    };
    assert_eq!(k.call(k.t, call(vec![])), WAITS);
    // It answers through the reply object and keeps the lend until it
    // receives again.
    let answer = Syscall::Send {
        target: s(311),
        message: crate::Message::default(),
    };
    assert_eq!(k.call(server, answer), Ok(Output::Sent { flags: 0 }));
    let lends = k.m.lends.clone();
    assert!(!lends.is_empty());
    // A second caller queues, moving a capability.
    assert_eq!(
        k.call(caller, call(vec![crate::Transfer::Move(s(360))])),
        WAITS
    );
    // The server receives: it would release its lend, then take the
    // queued caller, whose capability's segment it tries. Core 1 holds the
    // context the lend returns to, core 2 that segment: the first try fails,
    // the second round waits for core 1's event and its try of the segment
    // fails again, so the call returns `again`, having released no lend.
    let again = recv(crate::Range {
        first: s(380),
        count: 1,
    });
    let lock = super::pass18::first_try(&k, server, &again);
    assert!(matches!(lock, LockId::Sched(_)), "{lock:?}");
    k.m.hold(1, &[lock], None).expect("a second core");
    let segment = LockId::Segment(
        k.m.object_at(k.t, s(dodos_abi::START_SLOT_CSPACE)).unwrap(),
        2,
    );
    k.m.hold(2, &[segment], None).expect("a third core");
    assert_eq!(k.call(server, again), Err(Error::Again));
    assert_eq!(k.m.lends, lends);
}

// The `Report` of a line carries the operands of its operation 5 claimed;
// a new holder rewrites the line's entry from them (D-137).

#[test]
fn a_new_holder_of_an_ioapic_rewrites_a_reported_lines_entry_from_its_operands() {
    let (mut m, _, _, _) = holder_requester_third();
    let role = m.sys.node(0).ioapic_role(5).unwrap();
    let holder = m.sys.role(role).holder();
    let a = (0..3).find(|&n| n != holder).unwrap();
    let killer = (0..3).find(|&n| n != holder && n != a).unwrap();
    assert!(super::pass18::claim_line(&mut m, a, 5, 210).is_ok());
    // The holder dies with the entry lost: the successor rebuilds it from
    // a's report.
    m.sys.edit_table(|t| {
        t.units.entry(role).or_default().lines.remove(&5);
    });
    m.ok(killer, kill(holder));
    m.run(30 * MS);
    assert_ne!(m.sys.role(role).holder(), holder);
    let generation = m.sys.member(a).generation();
    let unit = &m.sys.canonical().units[&role];
    assert_eq!(unit.lines.get(&5), Some(&(a, generation)));
}

// Two cores never hold one lock (D-137: a scripted holder is a second
// core).

#[test]
fn a_second_core_never_takes_a_lock_another_core_holds() {
    let (mut k, _, _, _) = super::pass18::l2_delivery();
    let lock = LockId::Core(0);
    k.m.hold(1, &[lock], None).expect("a second core");
    assert_eq!(
        k.m.hold(2, &[lock], None),
        Err("two cores never hold one lock")
    );
    assert_eq!(
        k.m.hold(1, &[LockId::Core(2)], None),
        Err("a core is in one kernel event at a time")
    );
    // A holder whose trace breaks the order is reported.
    k.m.contended.get_mut(&1).unwrap().steps.extend([
        Step::Pay(crate::locks::Cost::Lock),
        Step::Lock(LockId::Thread(k.t)),
    ]);
    assert_eq!(
        k.m.check(),
        Err("L2: a lock waited for out of the order fixed by kind")
    );
}

// The verification of the tenth model review, missed fault 2: R5 is not
// written while R2 still has remote capabilities to remove.

#[test]
fn r5_is_written_only_once_r2_removed_every_remote_capability_of_the_fenced_node() {
    use crate::{Message, Range, Transfer};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let reply = m.retype(1, ObjectKind::Reply, 0, 100);
    m.ok(1, reply);
    let rights = dodos_abi::Rights::READ
        .union(dodos_abi::Rights::DERIVE)
        .bits();
    // Nine sends of four frames from node 2 to node 1: 36 grants, more
    // remote capabilities than the 32 units of one step.
    for round in 0..9u32 {
        let first = 200 + 4 * round;
        for n in 0..4 {
            let frame = m.retype(2, ObjectKind::Frame, 1, first + n);
            m.ok(2, frame);
        }
        let recv = Syscall::Recv {
            endpoint: s(dodos_abi::START_SLOT_MAILBOX),
            reply: s(100),
            range: Range {
                first: s(400 + 4 * round),
                count: 4,
            },
            wait: 0,
        };
        assert_eq!(m.call(1, recv), WAITS);
        let send = Syscall::NodeSend {
            node: node_cap(1),
            message: Message {
                label: 7,
                words: vec![7],
                caps: (0..4)
                    .map(|n| Transfer::Derive(s(first + n), rights))
                    .collect(),
                ..Message::default()
            },
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call(2, send), WAITS);
        m.settle();
    }
    let held = |m: &S| {
        m.sys
            .node(1)
            .remotes
            .values()
            .filter(|r| r.node == 2)
            .count()
    };
    assert_eq!(held(&m), 36);
    m.ok(0, kill(2));
    let generation = m.sys.member(2).generation();
    for _ in 0..200_000 {
        if m.sys.settle(1) == 0 {
            let now = m.sys.now();
            m.sys.advance(now + 50_000);
        }
        if m.sys.canonical().beacons[1].acked[2] == generation {
            break;
        }
    }
    assert_eq!(m.sys.canonical().beacons[1].acked[2], generation);
    assert_eq!(held(&m), 0);
}

// The verification of the tenth model review, missed fault 3: a starting
// node that never beats is accused with stale once START_NS has passed
// since its observation, and not at STALE_NS (D-137).

#[test]
fn a_start_on_a_core_that_never_beats_is_accused_after_start_ns_and_not_before() {
    use super::pass17::fresh;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1]]));
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // Core 2 was never used: it never takes its wake.
    m.sys.hang(2, true);
    m.ok(0, fresh(1, 0b100));
    let since = m.sys.now();
    m.sys.advance(since + dodos_abi::START_NS - MS);
    assert_eq!(m.state(1), MemberState::Starting);
    let sweep = m.sys.canonical().sweep_ns;
    m.sys.advance(since + dodos_abi::START_NS + 2 * sweep);
    assert_ne!(m.state(1), MemberState::Starting);
    assert_eq!(m.sys.member(1).evidence(), dodos_abi::Evidence::Stale);
}
