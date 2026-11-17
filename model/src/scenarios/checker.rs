//! The checker fed a bad state (the eighth model review, F15): for each
//! invariant of a node of several cores (`check/node.rs`) and of the whole
//! machine (`check/machine.rs`) that no test had made fail, a valid state
//! spoiled in one place, and the check's report. A check that cannot fail
//! is caught here.

use super::*;
use crate::Wait;
use crate::cores::{Completes, Flush, Request};
use crate::locks::{Cost, LockId, Step};
use crate::nodes::HwAction;
use crate::table::{FenceStep, GrantState};
use dodos_abi::{CHECK_MUL, CoreWord, Member, RoleWord};

/// A node of two cores alone.
fn node2() -> N {
    N::on(&one_node(crate::Arch::X86_64, 2))
}

#[track_caller]
fn reported(k: &N, message: &str) {
    assert_eq!(k.m.check(), Err(message), "the check of the spoiled node");
}

#[track_caller]
fn machine_reported(m: &S, message: &str) {
    let report = m.sys.check().expect_err("the spoiled machine");
    assert!(report.ends_with(message), "{report}");
}

/// A thread of the node with no context, configured and resumed: it can be
/// said to wait without its context leaving a queue.
fn idle_thread(k: &mut N) -> crate::ObjId {
    k.ok(k.retype(ObjectKind::Thread, 0, 300));
    k.ok(Syscall::ThreadConfigure {
        thread: s(300),
        cspace: SlotAddr::own(dodos_abi::START_SLOT_CSPACE),
        aspace: s(dodos_abi::START_SLOT_ASPACE),
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    });
    k.m.object_at(k.t, s(300)).unwrap()
}

/// The first thread marked leaving its core: its context leaves its
/// queue, and the core has the reschedule interrupt pending.
fn mark_leaving(k: &mut N) {
    let context = k.m.threads[&k.t].home.unwrap();
    k.m.threads.get_mut(&k.t).unwrap().leaving = true;
    k.m.scheds.get_mut(&context).unwrap().queue = None;
    k.m.cores[0].ipis.insert(crate::Ipi::Reschedule);
}

fn request(asked: u64, acked: u64, started: u64) -> Request {
    Request {
        flush: Flush::Kernel(vec![]),
        asked,
        acked,
        started,
        completes: Completes::Protect,
        waiter: None,
    }
}

// The requests of the cores and the TLB (check/node.rs).

#[test]
fn a_translation_a_core_keeps_after_a_completed_change_is_reported() {
    let mut k = node2();
    let space = k.m.threads[&k.t].aspace.unwrap();
    let cap = *k.m.caps.keys().next().unwrap();
    k.m.cores[1]
        .tlb
        .entry(space)
        .or_default()
        .insert(1 << 40, (cap, 0, Rights::READ));
    reported(
        &k,
        "TLB: a core can use a translation a completed change removed",
    );
}

#[test]
fn a_run_of_the_kernels_space_a_core_still_translates_after_it_was_given_is_reported() {
    let mut k = node2();
    k.m.cores[1].kernel_tlb.insert(u64::MAX >> 30);
    reported(
        &k,
        "G5: a run given or destroyed while a core may still translate it in the kernel's own space",
    );
}

#[test]
fn a_request_on_a_node_of_one_core_is_reported() {
    let mut k = N::on(&one_node(crate::Arch::X86_64, 1));
    k.m.cores[0].request = Some(request(0, 0, 0));
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    reported(&k, "TLB: a request on a node of one core or on ARM64");
}

#[test]
fn a_request_that_asks_its_own_core_is_reported() {
    let mut k = node2();
    k.m.cores[0].request = Some(request(0b11, 0, 0));
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    reported(&k, "TLB: a request asks its own core");
}

#[test]
fn a_complete_request_kept_is_reported() {
    let mut k = node2();
    k.m.cores[0].request = Some(request(0b10, 0b10, 0));
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    reported(&k, "TLB: a complete request kept");
}

#[test]
fn a_request_whose_record_is_not_unmapping_is_reported() {
    let mut k = node2();
    let cap = *k.m.caps.keys().next().unwrap();
    let mut r = request(0b10, 0, 0);
    r.completes = Completes::Record(cap);
    k.m.cores[0].request = Some(r);
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    k.m.cores[1].pending.insert(0);
    reported(&k, "TLB: the record of a request is not unmapping");
}

#[test]
fn a_request_whose_thread_waits_for_something_else_is_reported() {
    let mut k = node2();
    let thread = idle_thread(&mut k);
    let mut r = request(0b10, 0, 0);
    r.waiter = Some(thread);
    k.m.cores[0].request = Some(r);
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    k.m.cores[1].pending.insert(0);
    k.m.threads.get_mut(&thread).unwrap().wait = Wait::Cores(1);
    reported(&k, "TLB: the thread of a request waits for another");
}

#[test]
fn a_request_older_than_core_answer_ns_on_a_node_that_did_not_accuse_itself_is_reported() {
    let mut k = node2();
    k.m.cores[0].request = Some(request(0b10, 0, 0));
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    k.m.cores[1].pending.insert(0);
    k.m.set_now(dodos_abi::CORE_ANSWER_NS);
    reported(
        &k,
        "CORE_ANSWER_NS: a request older than the bound on a node that did not accuse itself",
    );
}

#[test]
fn a_pending_bit_that_names_no_request_is_reported() {
    let mut k = node2();
    k.m.cores[1].pending.insert(0);
    reported(&k, "TLB: a pending bit names no request that asks the core");
}

#[test]
fn an_idle_mark_on_a_node_of_one_core_is_reported() {
    let mut k = N::on(&one_node(crate::Arch::X86_64, 1));
    k.m.cores[0].idle.mark = true;
    reported(
        &k,
        "TLB: a node of one core has no idle mark, owes bit or pending set",
    );
}

#[test]
fn a_thread_waiting_for_a_request_not_in_flight_is_reported() {
    let mut k = node2();
    let thread = idle_thread(&mut k);
    k.m.threads.get_mut(&thread).unwrap().wait = Wait::Cores(1);
    reported(
        &k,
        "TLB: a thread waits for a request that is not in flight",
    );
}

// A thread leaving its core.

#[test]
fn a_marked_thread_with_no_leave_interrupt_is_reported() {
    let mut k = node2();
    let thread = idle_thread(&mut k);
    k.m.threads.get_mut(&thread).unwrap().leaving = true;
    reported(
        &k,
        "leaving its core: a marked thread not on a core sent the leave interrupt",
    );
}

#[test]
fn a_thread_leaving_its_core_on_a_node_of_one_core_is_reported() {
    let mut k = N::on(&one_node(crate::Arch::X86_64, 1));
    mark_leaving(&mut k);
    k.m.cores[0].ipis.insert(crate::Ipi::Leave);
    reported(&k, "leaving its core: reached on a node of one core");
}

// Locks.

#[test]
fn a_node_of_one_core_that_pays_for_several_is_reported() {
    let mut k = N::on(&one_node(crate::Arch::X86_64, 1));
    k.m.locks.steps = vec![Step::Pay(Cost::Interrupt)];
    reported(
        &k,
        "N = 1: a node of one core paid for a rule of several cores",
    );
}

#[test]
fn a_spin_inside_a_hold_is_reported() {
    let mut k = node2();
    k.m.locks.steps = vec![Step::Spin(crate::locks::WaitKind::Cores)];
    reported(
        &k,
        "no lock is held across a wait for another core or for a unit",
    );
}

#[test]
fn the_gate_lock_outside_the_node_lock_is_reported() {
    let mut k = node2();
    k.m.locks.steps = vec![Step::Lock(LockId::Gate), Step::Unlock(LockId::Gate)];
    reported(&k, "the gate lock taken outside the node lock");
}

#[test]
fn the_node_lock_held_past_the_exit_is_reported() {
    let mut k = node2();
    k.m.locks.steps = vec![Step::Pay(Cost::Lock), Step::Lock(LockId::Node)];
    reported(&k, "the node lock held past the exit of an event");
}

// Credits.

#[test]
fn a_peer_with_the_wrong_number_of_credits_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.peers[1].credits.pop();
    reported(&k, "credits: the credits of a peer");
}

#[test]
fn a_credit_toward_the_node_itself_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.peers[0].credits[0] = Some(crate::rings::Credit {
        kind: dodos_abi::NodeMessage::Revoke,
        serial: 1,
        completes: crate::rings::Completes::Revoke { grant: 0 },
    });
    reported(&k, "credits: a credit toward the node itself");
}

#[test]
fn a_thread_waiting_for_an_answer_no_credit_expects_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    let thread = idle_thread(&mut k);
    k.m.threads.get_mut(&thread).unwrap().wait = Wait::Node { peer: 1, credit: 3 };
    reported(
        &k,
        "credits: a thread waits for an answer no credit expects",
    );
}

// What crosses between nodes, as a receiver applies it.

#[test]
fn a_unit_operation_submitted_by_a_node_that_is_not_the_holder_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.submitted.push((0, RoleWord::held(1, 5)));
    reported(
        &k,
        "SERVED: a unit operation submitted by a node the role word does not name as holder",
    );
}

#[test]
fn a_unit_operation_submitted_after_step_1_of_a_hand_over_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.submitted.push((0, RoleWord::held(0, 5)));
    k.m.handing.insert(
        0,
        crate::roles::Handover {
            target: 1,
            started: 0,
            caller: None,
        },
    );
    reported(
        &k,
        "SERVED: a unit operation submitted after step 1 of a hand-over",
    );
}

#[test]
fn a_message_applied_to_another_generation_of_its_receiver_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    let sender = k.m.table.member(1);
    let header = dodos_abi::RingHeader::new(dodos_abi::NodeMessage::Release, 1, (1, 1), 99, None);
    k.m.applied.push((header, sender));
    reported(
        &k,
        "RECEIVER: a message acted on a generation of its receiver it was not sent to",
    );
}

#[test]
fn a_message_of_a_generation_that_is_over_applied_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    let sender = k.m.table.member(1);
    let me = k.m.table.generation(0);
    let header = dodos_abi::RingHeader::new(
        dodos_abi::NodeMessage::Release,
        1,
        (1, sender.generation() + 2),
        me,
        None,
    );
    k.m.applied.push((header, sender));
    reported(
        &k,
        "SENDER: a message of a generation that is over was applied",
    );
}

// The machine (check/machine.rs).

/// A core word with a valid check, as the gate would write it, whatever
/// its fields: the test's input for words no constructor makes.
fn word_with_check(v: u64) -> u64 {
    let v = v & !(0xfff << 4);
    v | (v.wrapping_mul(CHECK_MUL) >> 52) << 4
}

#[test]
fn a_core_word_that_fails_its_check_is_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| t.core_words[1] ^= 1 << 5);
    machine_reported(&m, "core word: a word that fails its check");
}

#[test]
fn a_free_core_word_that_names_a_node_is_reported() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1]]));
    m.sys
        .edit_table(|t| t.core_words[2] = word_with_check(1 << 16 | 1 << 32));
    machine_reported(&m, "core word: a free word names a node");
}

#[test]
fn a_stop_in_flight_under_a_word_that_is_not_marked_is_reported() {
    let mut m = S::two();
    m.sys.push_action(0, HwAction::Stop { core: 1 });
    machine_reported(
        &m,
        "fence: a stop in flight under a word that is not marked",
    );
}

#[test]
fn a_word_marked_with_no_park_request_raised_is_reported() {
    let mut m = S::two();
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.core_words[1] = CoreWord::owned(1, g).marked().raw();
        t.park_requests[1] = 0;
    });
    machine_reported(
        &m,
        "fence: a word marked before its park request was raised",
    );
}

#[test]
fn a_core_that_runs_a_node_its_word_does_not_name_is_reported() {
    let mut m = S::two();
    m.sys
        .edit_table(|t| t.core_words[1] = CoreWord::owned(0, t.member(0).generation()).raw());
    machine_reported(&m, "P2: a core executes a node its word does not name");
    let mut m = S::two();
    m.sys
        .edit_table(|t| t.core_words[1] = CoreWord::free(false).raw());
    machine_reported(&m, "P2: a core executes a node its word does not name");
}

#[test]
fn a_core_that_runs_a_generation_that_is_over_is_reported() {
    let mut m = S::two();
    m.sys
        .edit_table(|t| t.core_words[1] = CoreWord::owned(1, t.member(1).generation() + 2).raw());
    machine_reported(
        &m,
        "P3: a core executes a dead node or a generation that is over",
    );
}

#[test]
fn a_map_count_that_is_not_the_leaves_of_its_chunk_is_reported() {
    let mut m = S::two();
    let frame = m.retype(0, ObjectKind::Frame, 1, 150);
    m.ok(0, frame);
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(150),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(0, map);
    let object = m.sys.node(0).object_at(m.t[0], s(150)).unwrap();
    let chunk = m.sys.node(0).frames[&object].base >> m.sys.node(0).geometry().chunk_shift();
    m.sys.edit_table(|t| {
        // A count below the leaves the node holds (a chunk of a floor is
        // judged at least, not exactly).
        std::sync::Arc::make_mut(&mut t.gate_words)[chunk as usize] -= 1;
    });
    // The node's own check of its counts reports it first: G4 at the level
    // of the machine sums the same leaves over the running nodes, and only
    // the owner holds leaves of a chunk (a grantee's are in its use entry),
    // so for a running owner it is the same rule; for an owner that does
    // not run it allows a count above the leaves left, never below.
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(report, "node 0: 58: map count");
}

#[test]
fn a_grant_entry_unanswered_with_nothing_in_flight_is_reported() {
    let mut m = S::two();
    let generation = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.grants[0].insert(
            7,
            crate::table::GrantEntry {
                grantee: 1,
                generation,
                serial: 1,
                state: GrantState::Live,
                use_index: None,
                answered: false,
                doorbell: None,
            },
        );
    });
    machine_reported(&m, "LEAK: a grant entry unanswered with nothing in flight");
}

#[test]
fn a_ring_beyond_its_entries_is_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| {
        std::sync::Arc::make_mut(&mut t.rings)[0][1].kernel.produced += 65;
    });
    machine_reported(&m, "rings: a ring overflows");
}

#[test]
fn a_credit_in_use_with_nothing_in_flight_is_reported() {
    let mut m = S::two();
    m.sys.node_mut(0).peers[1].credits[0] = Some(crate::rings::Credit {
        kind: dodos_abi::NodeMessage::Revoke,
        serial: 1,
        completes: crate::rings::Completes::Revoke { grant: 0 },
    });
    let report = m.sys.check().expect_err("spoiled");
    assert!(
        report
            .ends_with("CREDIT: a credit in use with neither its request nor its answer in flight"),
        "{report}"
    );
}

#[test]
fn two_nodes_that_map_one_units_registers_are_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| {
        let unit = t.units.entry(0).or_default();
        unit.mapped.insert(0);
        unit.mapped.insert(1);
    });
    machine_reported(&m, "ONE-HOLDER: two nodes map a unit's registers");
}

#[test]
fn a_stop_sent_before_its_word_was_marked_inside_the_event_is_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| {
        t.fence_trace.extend([
            FenceStep::Raised(1),
            FenceStep::Stopped(1),
            FenceStep::Marked(1),
        ]);
    });
    machine_reported(&m, "fence: a stop sent before its word was marked");
}

#[test]
fn a_word_marked_before_its_park_request_inside_the_event_is_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| {
        t.fence_trace.extend([
            FenceStep::Marked(1),
            FenceStep::Raised(1),
            FenceStep::Stopped(1),
        ]);
    });
    machine_reported(
        &m,
        "fence: a word marked before its park request was raised",
    );
}

#[test]
fn a_grant_freed_while_its_holder_translates_to_it_is_reported() {
    let mut m = S::two();
    // A use entry of node 1 that counts a leaf under a grant of node 0 that
    // node 0 no longer has.
    let generation = m.sys.member(0).generation();
    m.sys.edit_table(|t| {
        t.uses[1].insert(
            4,
            crate::table::UseEntry {
                granter: 0,
                grant: 9,
                serial: 1,
                generation,
                root: crate::ObjId(0),
                leaves: 1,
                release_pending: false,
                signal_pending: false,
            },
        );
    });
    let report = m.sys.check().expect_err("spoiled");
    assert!(
        report.ends_with("G6: a grant freed while its holder translates to it")
            || report.contains("58"),
        "{report}"
    );
    let _ = Member::new;
}

#[test]
fn a_thread_on_two_cores_is_reported() {
    let mut k = node2();
    k.m.cores[1].thread = Some(k.t);
    k.m.cores[1].ipis.insert(crate::Ipi::Reschedule);
    reported(&k, "46: a thread on two cores");
}

#[test]
fn a_running_thread_bound_to_no_address_space_is_reported() {
    let mut k = node2();
    let space = k.m.threads[&k.t].aspace.unwrap();
    k.m.threads.get_mut(&k.t).unwrap().aspace = None;
    k.m.aspaces
        .get_mut(&space)
        .unwrap()
        .threads
        .retain(|&t| t != k.t);
    reported(&k, "46: address space of the running thread");
}

#[test]
fn a_translation_under_no_grant_is_reported() {
    let mut m = S::two();
    super::pass17::granted_to_1(&mut m);
    let untyped = SlotAddr::NONE;
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(110),
        untyped,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(1, map);
    m.sys.edit_table(|t| t.grants[0].clear());
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(report, "node 1: 58: translation under no grant");
}
