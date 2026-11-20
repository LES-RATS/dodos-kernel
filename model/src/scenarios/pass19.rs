//! The scenarios of pass 19 (D-137): the tenth review of the model
//! (`Docs/reviews/2026-11-17-model-review-10-deepseek.md`) judged finding by
//! finding, and what the eleventh review of the contract and its
//! verification changed in the contract, each a test named after the
//! sentence it guards and shown failing once under a mutation of the code
//! it guards.

use super::*;

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
        sched.throttled = Some((now, dodos_abi::sched_debt(0, 1)));
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
    m.sys.node_mut(0).detach_answered(
        role,
        DEVICE as u16,
        dodos_abi::UnitResult::Done,
        crate::units::Waiter::Thread(None),
    );
    assert_eq!(m.sys.canonical().attach_word(key), word);
    // A detach: answered done once the holder has written detached; the
    // owner then writes 0, the identifier word free and the count 0.
    let detach = super::units::attach(&m, 0, 0, DEVICE);
    let detach = super::pass17::detach_of(detach);
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
    m.supervisor(0, m.t[0]);
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
    m.supervisor(0, m.t[0]);
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
    m.supervisor(2, m.t[2]);
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
    m.supervisor(0, m.t[0]);
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
    m.supervisor(0, m.t[0]);
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
    m.supervisor(0, m.t[0]);
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
    let untyped = super::table_slot(m.sys.node(0), m.t[0]);
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
    let (mut m, _, a, _) = holder_requester_third();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, false, false).raw(),
        core: u64::from(a),
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, a, claim, 30 * MS);
    let thread = m.t[a as usize];
    assert_eq!(m.sys.check(), Ok(()));
    let node = m.sys.node_mut(a);
    let irq = node.object_at(thread, s(210)).expect("the irq");
    // An entry still asks the line's redirection entry: the claim of a
    // remapped line queues it after its last unit (its `then`, gap 12).
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
        elsewhere: false,
        then: None,
    });
    assert_eq!(node.check(), Ok(()), "the entry is one the check accepts");
    assert!(matches!(node.irq_removal(irq), crate::caps::Undone::Wait));
    assert!(!node.irqs[&irq].removing);
    assert_eq!(m.sys.check(), Ok(()));
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
    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(holder));
    m.run(30 * MS);
    assert_ne!(m.sys.role(role).holder(), holder);
    let generation = m.sys.member(a).generation();
    let unit = &m.sys.canonical().units[&role];
    assert_eq!(unit.lines.get(&5), Some(&(a, generation)));
}

// The verification of the tenth model review, missed fault 2: R5 is not
// written while R2 still has remote capabilities to remove.

/// Node 2 grants node 0 `4 × rounds` frames, then is killed; node 0's
/// recovery runs until it acknowledges node 2. Node 0 holds no unit role
/// then (node 1 holds the IOMMU's, node 2 the IOAPIC's), so its R1 is done
/// at once and only R2 and R4 can hold its R5 back. Returns the number of
/// node 0's run blocks taken, the remote capabilities of node 2 it still held at the step
/// that acknowledged, and whether its audit was done then.
fn acknowledged_after_grants(rounds: u32, blocks: u32) -> (usize, usize, bool) {
    use crate::{Message, Range, Transfer};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let reply = m.retype(0, ObjectKind::Reply, 0, 100);
    m.ok(0, reply);
    // Node 0 takes `blocks` runs of one chunk of unclaimed memory: R4
    // audits each.
    let shift = m.sys.canonical().geometry.chunk_shift();
    let free: Vec<u64> = m
        .sys
        .canonical()
        .owners
        .iter()
        .enumerate()
        .filter(|&(_, &raw)| {
            dodos_abi::Ownership::from_raw(raw).is_some_and(|w| {
                w.state() == dodos_abi::OwnState::Free && w.owner() == dodos_abi::Owner::Unclaimed
            })
        })
        .map(|(chunk, _)| (chunk as u64) << shift)
        .take(blocks as usize)
        .collect();
    for (n, place) in free.into_iter().enumerate() {
        let take = Syscall::MemoryTake {
            control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
            kind: 0,
            place,
            size: u64::from(shift),
            destination: s(700 + n as u32),
        };
        m.ok(0, take);
    }
    let rights = dodos_abi::Rights::READ
        .union(dodos_abi::Rights::DERIVE)
        .bits();
    for round in 0..rounds {
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
        assert_eq!(m.call(0, recv), WAITS);
        let send = Syscall::NodeSend {
            node: node_cap(0),
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
            .node(0)
            .remotes
            .values()
            .filter(|r| r.node == 2)
            .count()
    };
    assert_eq!(held(&m), 4 * rounds as usize);
    let runs = m
        .sys
        .node(0)
        .untypeds
        .values()
        .filter(|u| u.block == Some(crate::Block::Run))
        .count();
    m.supervisor(1, m.t[1]);
    m.ok(1, kill(2));
    let generation = m.sys.member(2).generation();
    // Event by event until node 2 is fenced, then node 0's reserve step by
    // step, as its own reserve would take them (the model otherwise runs
    // every step a node can make at one instant in one event).
    for _ in 0..200_000 {
        if m.state(2) == MemberState::Fenced {
            break;
        }
        if m.sys.settle(1) == 0 {
            let now = m.sys.now();
            m.sys.advance(now + 50_000);
        }
    }
    assert_eq!(m.state(2), MemberState::Fenced);
    for _ in 0..64 {
        if m.sys.node(0).table.beacons[0].acked[2] == generation {
            break;
        }
        m.sys.node_mut(0).reserve_step();
    }
    assert_eq!(m.sys.node(0).table.beacons[0].acked[2], generation);
    let reclaim = m.sys.node(0).reclaims.get(&2).cloned();
    let audited = reclaim.is_some_and(|r| r.audit.is_some_and(|a| a.is_empty()));
    (runs, held(&m), audited)
}

#[test]
fn r5_is_written_only_once_r2_removed_every_remote_capability_of_the_fenced_node() {
    // 72 grants and no block: R2 takes three steps of 32 units, which
    // outlast R1, and nothing is left for R4 to stop R5.
    let (blocks, held, audited) = acknowledged_after_grants(18, 0);
    assert_eq!((blocks, held, audited), (0, 0, true));
}

#[test]
fn r5_is_written_only_once_r4_audited_every_block() {
    // 64 grants take R2 two whole steps, and the second leaves no unit for
    // R4's three blocks: R5 waits for the next step.
    let (blocks, held, audited) = acknowledged_after_grants(16, 3);
    assert_eq!((blocks, held, audited), (3, 0, true));
}

// The verification of the tenth model review, missed fault 3: a starting
// node that never beats is accused with stale once START_NS has passed
// since its observation, and not at STALE_NS (D-137).

#[test]
fn a_start_on_a_core_that_never_beats_is_accused_after_start_ns_and_not_before() {
    use super::pass17::fresh;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1]]));
    m.supervisor(0, m.t[0]);
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

#[test]
fn a_start_on_a_reused_core_with_an_old_idle_until_is_not_accused_before_start_ns() {
    use super::pass17::fresh;
    // A sweep every 100 ms observes the starting node early.
    let mut two = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    two.membership.sweep_ms = 100;
    let mut m = S::on(&two);
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // Core 1 ran node 1 before: its `idle_until` is an old instant, so the
    // test of STALE_NS would hold at once.
    m.sys.hang(1, true);
    m.ok(0, fresh(1, 0b10));
    let since = m.sys.now();
    m.sys.advance(since + dodos_abi::START_NS - MS);
    assert_eq!(m.state(1), MemberState::Starting);
}
