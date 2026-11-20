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
use dodos_abi::{CHECK_MUL, CoreWord, RoleWord};

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
pub(super) fn idle_thread(k: &mut N) -> crate::ObjId {
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
    let op = crate::rings::UnitOp::Invalidate { domain: Some(0) };
    k.m.submitted.push((0, RoleWord::held(1, 5), op));
    reported(
        &k,
        "SERVED: a unit operation submitted by a node the role word does not name as holder",
    );
}

#[test]
fn a_member_word_with_the_resume_bit_in_another_state_than_starting_is_reported() {
    let mut m = S::two();
    let live = m.sys.member(1);
    assert_eq!(live.state(), MemberState::Live);
    m.sys.edit_table(|t| t.members[1] = live.with_resume());
    assert_eq!(
        m.sys.check(),
        Err("member word: a word that fails its check".to_string())
    );
}

/// "starting (g) → live (g): Every core whose word names the node reads
/// *owned*: none reads *joining*" (The states of a node, D-158), and no row
/// writes a word joining under a node that is live, sealing or sealed.
#[test]
fn a_core_word_joining_under_a_live_node_is_reported() {
    let mut m = S::two();
    let word = dodos_abi::CoreWord::from_raw(m.sys.core_word(1)).expect("a word");
    assert_eq!(word.state(), dodos_abi::CoreState::Owned);
    let joining = dodos_abi::CoreWord::joining(word.slot(), word.generation());
    m.sys.edit_table(|t| t.core_words[1] = joining.raw());
    assert_eq!(
        m.sys.check(),
        Err("core word: joining under a node that is live, sealing or sealed".to_string())
    );
}

/// R1's invalidation during a hand-over: judged by whether the node that
/// hands still maps the unit (D-144).
#[test]
fn r1_submitted_during_a_hand_over_is_reported_once_the_unit_is_unmapped() {
    let spoiled = |mapped: bool| {
        let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
        let op = crate::rings::UnitOp::Invalidate { domain: None };
        k.m.submitted.push((0, RoleWord::held(0, 5), op));
        k.m.handing.insert(
            0,
            crate::roles::Handover {
                target: 1,
                started: 0,
                caller: None,
                failed: false,
            },
        );
        let unit = k.m.table.units.entry(0).or_default();
        match mapped {
            true => unit.mapped.insert(0),
            false => unit.mapped.remove(&0),
        };
        k
    };
    assert_eq!(spoiled(true).m.check(), Ok(()));
    reported(
        &spoiled(false),
        "SERVED: a unit operation submitted after step 1 of a hand-over",
    );
}

#[test]
fn a_hand_over_of_a_failed_unit_that_outlives_its_step_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.handing.insert(
        0,
        crate::roles::Handover {
            target: 1,
            started: 0,
            caller: None,
            failed: true,
        },
    );
    reported(&k, "HANDED: a hand-over of a failed unit waits for a drain");
}

#[test]
fn a_unit_operation_submitted_after_step_1_of_a_hand_over_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    let op = crate::rings::UnitOp::Invalidate { domain: Some(0) };
    k.m.submitted.push((0, RoleWord::held(0, 5), op));
    k.m.handing.insert(
        0,
        crate::roles::Handover {
            target: 1,
            started: 0,
            caller: None,
            failed: false,
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
                anchor: crate::table::Anchor::test(),
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

// G6 is guarded by pass 18's `a_grant_freed_while_an_unfenced_holder_
// translates_to_it_is_reported_by_g6`: on a running holder node invariant
// 58 names the fault first (ver2-D 7 found the test here vacuous).

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
    k.m.cores[0].loaded = None;
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

// The checks of a node that no test made fail (the final review's
// verifications of lot D, D4 and D10, and of lot AD, D1, D2 and D8): one
// spoiled state each, each red when its check is removed.

/// A request of core 0 asking core 1, in use.
fn in_use(k: &mut N, r: Request) {
    k.m.cores[0].request = Some(r);
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    k.m.cores[1].pending.insert(0);
}

#[test]
fn a_state_word_in_use_with_no_request_is_reported() {
    let mut k = node2();
    k.m.cores[0].request_state = crate::cores::RequestState::InUse;
    reported(&k, "TLB: the state word of a request");
}

#[test]
fn a_barrier_that_invalidates_is_reported() {
    let mut k = node2();
    let mut r = request(0b10, 0, 0);
    r.completes = Completes::Barrier(k.m.kernel_epoch);
    in_use(&mut k, r);
    reported(
        &k,
        "TLB: a barrier that invalidates, or an invalidation that is a barrier",
    );
}

#[test]
fn a_barrier_beyond_the_kernel_epoch_is_reported() {
    let mut k = node2();
    let mut r = request(0b10, 0, 0);
    r.flush = Flush::Nothing;
    r.completes = Completes::Barrier(k.m.kernel_epoch + 1);
    in_use(&mut k, r);
    reported(&k, "epoch: a barrier beyond the kernel epoch");
}

#[test]
fn a_request_whose_thread_waits_for_nothing_is_reported() {
    let mut k = node2();
    let thread = idle_thread(&mut k);
    let mut r = request(0b10, 0, 0);
    r.waiter = Some(thread);
    in_use(&mut k, r);
    reported(&k, "TLB: the thread of a request waits for another");
}

#[test]
fn a_request_whose_thread_is_gone_is_reported() {
    let mut k = node2();
    let mut r = request(0b10, 0, 0);
    r.waiter = Some(crate::ObjId(u32::MAX - 7));
    in_use(&mut k, r);
    reported(&k, "TLB: the thread of a request waits for another");
}

#[test]
fn an_epoch_raised_by_two_is_reported() {
    let (mut k, untyped) = super::pass20::ready_untyped(crate::Arch::X86_64, 2);
    let e = k.m.kernel_epoch;
    k.m.kernel_epoch = e + 3;
    k.m.locks.steps = vec![
        Step::Epoch(e + 1),
        Step::Ready(untyped),
        Step::Epoch(e + 3),
        Step::Ready(untyped),
    ];
    k.m.untypeds.get_mut(&untyped).unwrap().ready = Some(e + 3);
    reported(&k, "epoch: raised by more than one at a time");
}

#[test]
fn an_epoch_that_is_not_the_last_raise_is_reported() {
    let (mut k, untyped) = super::pass20::ready_untyped(crate::Arch::X86_64, 2);
    let e = k.m.kernel_epoch;
    k.m.kernel_epoch = e + 2;
    k.m.locks.steps = vec![Step::Epoch(e + 1), Step::Ready(untyped)];
    k.m.untypeds.get_mut(&untyped).unwrap().ready = Some(e + 1);
    reported(&k, "epoch: the node's epoch is not its last raise");
}

#[test]
fn a_word_6_beyond_the_kernel_epoch_is_reported() {
    let (mut k, untyped) = super::pass20::ready_untyped(crate::Arch::X86_64, 2);
    let e = k.m.kernel_epoch;
    k.m.untypeds.get_mut(&untyped).unwrap().ready = Some(e + 1);
    reported(
        &k,
        "epoch: word 6 of an untyped's root slot is not the epoch its watermark became ready at",
    );
}

#[test]
fn a_ready_watermark_under_a_live_object_is_reported() {
    let mut k = node2();
    let e = k.m.kernel_epoch.max(1);
    k.m.kernel_epoch = e;
    let untyped =
        k.m.untypeds
            .iter()
            .find(|(_, u)| u.live > 0)
            .map(|(&id, _)| id)
            .unwrap();
    k.m.untypeds.get_mut(&untyped).unwrap().ready = Some(e);
    reported(&k, "18: a watermark ready to return under a live object");
}

#[test]
fn a_send_in_a_request_credit_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.peers[1].credits[0] = Some(crate::rings::Credit {
        kind: dodos_abi::NodeMessage::Send,
        serial: 1,
        completes: crate::rings::Completes::Send {
            grants: vec![],
            thread: None,
            manage: true,
        },
    });
    reported(&k, "credits: a send outside the send credits");
}

#[test]
fn a_request_in_a_send_credit_is_reported() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    k.m.peers[1].credits[dodos_abi::REQUESTS_PER_PEER] = Some(crate::rings::Credit {
        kind: dodos_abi::NodeMessage::Revoke,
        serial: 1,
        completes: crate::rings::Completes::Revoke { grant: 0 },
    });
    reported(&k, "credits: a send outside the send credits");
}

#[test]
fn a_core_running_a_thread_that_does_not_hold_its_context_is_reported() {
    let mut k = node2();
    let other = idle_thread(&mut k);
    k.m.cores[0].thread = Some(other);
    reported(&k, "46: thread of the running context");
}

#[test]
fn a_core_that_loaded_another_space_is_reported() {
    let mut k = node2();
    k.ok(k.retype(ObjectKind::ASpace, 0, 301));
    let other = k.m.object_at(k.t, s(301)).unwrap();
    k.m.cores[0].loaded = Some(other);
    reported(&k, "G7: root a core loaded");
}

/// G6 by the holder's state the contract allows: a record left
/// *unmapping*, its leaf still counted in the use entry, under a grant the
/// granting node freed. The holder's own check accepts it (lot AD, D7);
/// G6 alone names it.
#[test]
fn a_grant_freed_while_its_holder_still_counts_an_unmapping_leaf_is_reported_by_g6() {
    let mut m = S::two();
    super::pass17::granted_to_1(&mut m);
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(110),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(1, map);
    let holder = m.t[1];
    let node = m.sys.node_mut(1);
    let frame = node.object_at(holder, s(110)).unwrap();
    let cap = *node
        .caps
        .iter()
        .find(|(_, c)| c.object == frame && matches!(c.record, crate::Record::Mapping(_)))
        .unwrap()
        .0;
    let crate::Record::Mapping(mapping) = node.caps[&cap].record else {
        unreachable!()
    };
    crate::live(&mut node.caps, cap).record = crate::Record::Unmapping(mapping);
    for space in node.aspaces.values_mut() {
        space.translations.retain(|_, t| t.cap != cap);
    }
    for core in &mut node.cores {
        for cached in core.tlb.values_mut() {
            cached.retain(|_, held| held.0 != cap);
        }
    }
    assert_eq!(m.sys.node(1).check(), Ok(()), "the holder accepts it");
    m.sys.edit_table(|t| t.grants[0].clear());
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(
        report,
        "G6: a grant freed while its holder translates to it"
    );
}

// A value of the state used as an index is judged before it is used: the
// checker names the invariant, it never panics (ver-D, Mistral's note on
// `check_cores`; the sweep of pass 23).

#[test]
fn a_core_word_that_names_a_slot_past_the_table_is_reported() {
    let mut m = S::two();
    m.sys
        .edit_table(|t| t.core_words[1] = CoreWord::owned(40, 1).raw());
    machine_reported(&m, "core word: names a slot past the table");
}

#[test]
fn a_use_entry_that_names_a_granter_past_the_table_is_reported() {
    let mut m = S::two();
    m.sys.edit_table(|t| {
        t.uses[1].insert(
            5,
            crate::table::UseEntry {
                granter: 40,
                grant: 3,
                serial: 1,
                generation: 1,
                root: Some(crate::ObjId(0)),
                leaves: 0,
                release_pending: false,
                signal_pending: false,
            },
        );
    });
    let report = m.sys.check().expect_err("spoiled");
    assert!(report.ends_with("past the table"), "{report}");
}

#[test]
fn a_pending_bit_that_names_a_core_past_the_node_is_reported() {
    let mut k = node2();
    k.m.cores[1].pending.insert(7);
    reported(&k, "TLB: a pending bit names no request that asks the core");
}

#[test]
fn a_thread_waiting_for_a_peer_past_the_table_is_reported() {
    let mut k = node2();
    let thread = idle_thread(&mut k);
    k.m.threads.get_mut(&thread).unwrap().wait = Wait::Node {
        peer: 40,
        credit: 3,
    };
    reported(
        &k,
        "credits: a thread waits for an answer no credit expects",
    );
}

// Two rules of the machine that the sweep of pass 23 found held by no
// test: each switched off, the whole suite stayed green.

#[test]
fn a_core_word_that_reads_leaving_is_reported() {
    let mut m = S::two();
    let g = m.sys.member(1).generation();
    // State 2, *leaving*, which no rule writes (reserved for nodes that
    // give cores at run time).
    m.sys.edit_table(|t| {
        t.core_words[1] = word_with_check(2 | 1 << 16 | u64::from(g) << 32);
    });
    machine_reported(&m, "core word: leaving is written");
}

#[test]
fn a_park_request_that_goes_back_is_reported() {
    let mut m = S::two();
    m.ok(0, super::pass17::kill(1));
    m.run(10 * MS);
    assert_ne!(m.sys.park_request(1), 0, "the fence raised it");
    m.sys.edit_table(|t| t.park_requests[1] = 0);
    machine_reported(&m, "fence: a park request went back");
}

// D-143: a core of ARM64 or RISC-V that runs with its park request raised,
// its word marked (D-146), and neither the fence's stop nor its node's park
// in flight: no read before a suspend or at a wake parked it.
#[test]
fn a_core_that_runs_with_its_park_request_raised_and_nothing_in_flight_is_reported() {
    let mut m = S::on(&machine(crate::Arch::Aarch64, 2, &[&[0], &[1]]));
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.park_requests[1] += 1;
        t.core_words[1] = CoreWord::owned(1, g).marked().raw();
    });
    machine_reported(
        &m,
        "PARKED: a core runs with its park request raised and no stop or park in flight",
    );
}

// A core on its way to a sleep that loses its context, or asleep, while
// its node still has a thread on it.
#[test]
fn a_core_asleep_that_runs_a_thread_is_reported() {
    let mut m = S::on(&machine(crate::Arch::Riscv64, 2, &[&[0], &[1]]));
    m.ok(1, super::place_first(1, 1));
    m.sys.set_hardware(
        1,
        crate::system::Hardware::Asleep {
            latency: 1,
            wake: None,
        },
    );
    machine_reported(&m, "SLEEP: a core asleep runs a thread of its node");
}

// D-143: a core whose DR6 still holds a cause after an event: the #DB
// handler did not load its reset value.
#[test]
fn a_dr6_left_with_a_cause_is_reported() {
    let mut k = node2();
    k.m.cores[1].dr6 = 1 << 14;
    reported(
        &k,
        "DR6: a core's DR6 is not at its reset value after an event",
    );
}

/// A level line of GSI 5 claimed and bound by the one node of a RISC-V
/// machine.
fn riscv_level_line() -> S {
    let mut m = S::on(&one_node(crate::Arch::Riscv64, 1));
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, true, false).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, 0, claim, 30 * MS);
    let notify = m.retype(0, ObjectKind::Notify, 0, 211);
    m.ok(0, notify);
    super::pass28::bind(&mut m);
    m
}

// D-143, RISC-V: an armed level line whose input is high and with no MSI on
// its way, as an acknowledgement without the `setipnum` write leaves it.
#[test]
fn an_armed_level_line_high_and_no_msi_on_its_way_is_reported() {
    let mut m = riscv_level_line();
    m.sys.line_input(5, true);
    m.sys.edit_table(|t| t.aplic.get_mut(&5).unwrap().sent = 0);
    machine_reported(
        &m,
        "SETIPNUM: an armed level line high and no MSI on its way",
    );
}

// D-146: an arrival of a level line that left its source enabled at its
// APLIC.
#[test]
fn an_arrival_that_leaves_its_level_line_enabled_is_reported() {
    let mut m = riscv_level_line();
    m.sys.node_mut(0).event.arrival_enabled = true;
    machine_reported(
        &m,
        "CLRIENUM: an arrival left its level line enabled at its APLIC",
    );
}

// D-146: two MSIs of one level source on their way, as `setienum` written
// before `setipnum` sends for a line that fell and rose while masked.
#[test]
fn two_msis_of_one_level_source_on_their_way_are_reported() {
    let mut m = riscv_level_line();
    m.sys.line_input(5, true);
    m.sys.edit_table(|t| t.aplic.get_mut(&5).unwrap().sent = 2);
    machine_reported(&m, "APLIC: two MSIs of one level source on their way");
}

// G9, D-146: an event that wrote an APLIC register for a line the node does
// not hold.
#[test]
fn an_aplic_write_for_a_line_the_node_does_not_hold_is_reported() {
    let mut m = riscv_level_line();
    m.sys.node_mut(0).event.aplic.push(6);
    machine_reported(
        &m,
        "G9: an APLIC register written for a line the node does not hold",
    );
}

// ARM64 and the RISC-V cooperative fence (D-143, D-146): a fenced node's
// core not yet confirmed whose last stop is more than two rounds old, as a
// fence that signals once leaves it.
#[test]
fn an_unconfirmed_core_two_rounds_without_a_stop_is_reported() {
    let mut m = S::on(&machine(crate::Arch::Aarch64, 2, &[&[0], &[1]]));
    m.run(30 * MS);
    m.ok(0, super::pass17::kill(1));
    let sent = |m: &S| m.sys.canonical().fences[1].last_stop(1) != 0;
    assert!(m.step_until(sent));
    m.sys.edit_table(|t| t.fences[1].last_stop.clear());
    machine_reported(
        &m,
        "FENCE ROUND: an unconfirmed core of a fenced node went two rounds without a stop",
    );
}

// D-146: a suspend state in an idle table whose flag says it keeps the
// context while its parameter says it loses it.
#[test]
fn an_idle_state_whose_flag_disagrees_with_its_parameter_is_reported() {
    let mut k = N::on(&one_node(crate::Arch::Riscv64, 1));
    k.m.cores[0].idle_table = vec![[10, 0, dodos_abi::IDLE_METHOD_SBI | 0x8000_0000 << 32, 0]];
    reported(
        &k,
        "IDLE: a suspend state whose flag disagrees with its parameter",
    );
}

// D-146: `acked` written for a node that is not fenced (R5 alone writes
// it, and only for a fenced node).
#[test]
fn an_acknowledgement_of_a_node_that_is_not_fenced_is_reported() {
    let mut m = S::two();
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| t.beacons[0].acked[1] = g);
    let report = m.sys.check().expect_err("the spoiled machine");
    assert!(
        report.ends_with("ACKED: an acknowledgement of a node that is not fenced"),
        "{report}"
    );
}

// D-146: an accusation written while the accuser's own node is starting.
#[test]
fn an_accusation_by_a_node_that_is_not_live_is_reported() {
    let mut k = node2();
    k.m.event.accusers.push(dodos_abi::MemberState::Starting);
    reported(&k, "ACCUSE: a node that is not live or sealing accused");
}

// D-146: a unit role held by a node whose member word reads starting.
#[test]
fn a_unit_role_held_by_a_starting_node_is_reported() {
    let mut m = S::two();
    let holder = m.sys.role(super::units::IOMMU_ROLE).holder();
    let g = m.sys.member(holder).generation();
    m.sys.edit_table(|t| {
        let member = t.member(holder);
        let starting =
            dodos_abi::Member::new(MemberState::Starting, member.evidence(), member.driver(), g);
        t.members[holder as usize] = starting;
    });
    machine_reported(&m, "UNIT ROLE: a starting node holds a unit role");
}

// D-146: a thread that can run and still keeps the kind 9 of a #DB at CPL0
// after an event.
#[test]
fn a_runnable_thread_that_keeps_a_breakpoint_fault_is_reported() {
    let mut k = node2();
    let thread = k.t;
    k.m.threads.get_mut(&thread).unwrap().debug_kept = Some(1 << 32 | 0xFFFF_0FF1);
    reported(
        &k,
        "KIND 9: a runnable thread keeps a breakpoint fault after an event",
    );
}

// D-146: an observation stamped with a sealed member word.
#[test]
fn an_observation_of_a_sealed_node_is_reported() {
    let mut m = S::two();
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.beacons[0].observations.push(crate::table::Observation {
            node: 1,
            generation: g,
            state: MemberState::Sealed,
            resume: false,
            watched: crate::table::Watched::Heartbeat {
                core: 1,
                heartbeat: 0,
            },
            at: 0,
        });
    });
    machine_reported(&m, "OBSERVATION: an observation taken of a sealed node");
}

// D-146: a step of the reserve that left a `Release` it could have sent.
#[test]
fn a_message_held_after_a_step_that_could_send_it_is_reported() {
    let mut k = node2();
    k.m.event.held_after_step = true;
    reported(
        &k,
        "HELD: a message held after a step of the reserve that could send it",
    );
}

// D-147: a step that took observations kept one that no longer holds.
#[test]
fn a_closed_observation_kept_after_a_step_is_reported() {
    let mut k = node2();
    k.m.event.closed_kept = true;
    reported(
        &k,
        "OBSERVATION: a closed observation kept its slot after a step",
    );
}

// D-147: with a directed end of interrupt, a claimed, live level line left
// masked at its IOAPIC, which its claimer could never unmask.
#[test]
fn a_claimed_live_level_line_masked_with_a_directed_end_of_interrupt_is_reported() {
    let (mut m, _, role) = super::pass28::line_bound_on(|holder, _, _| (holder + 1) % 3);
    m.sys.edit_table(|t| {
        t.units.entry(role).or_default().masked.insert(5);
    });
    machine_reported(
        &m,
        "LINE MASK: a claimed, live level line masked at its controller",
    );
}

// G15, D-149: an x86-64 core whose `parked` answers its park request under
// a marked word while it still runs: a confirmation its own kernel made.
#[test]
fn an_x86_core_confirmed_while_it_runs_is_reported() {
    let mut m = S::two();
    let g = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.park_requests[1] += 1;
        t.core_words[1] = CoreWord::owned(1, g).marked().raw();
        t.core_beacons[1].parked = t.park_requests[1];
    });
    m.sys.push_action(0, HwAction::Stop { core: 1 });
    machine_reported(&m, "G15: an x86-64 core confirmed while it runs");
}

// D-147: `parked` written on ARM64 with no fence mark.
#[test]
fn an_arm64_parked_written_with_no_fence_mark_is_reported() {
    let mut m = S::on(&machine(crate::Arch::Aarch64, 2, &[&[0], &[1]]));
    m.sys.parked_unfenced = true;
    machine_reported(&m, "PARKED: an ARM64 parked written with no fence mark");
}

// D-149: a core whose park its firmware refused, counted confirmed.
#[test]
fn a_refused_core_counted_confirmed_is_reported() {
    let mut m = S::on(&machine(crate::Arch::Aarch64, 2, &[&[0], &[1]]));
    m.sys.set_hardware(1, crate::system::Hardware::Refused);
    m.sys.edit_table(|t| t.stopped[1] = t.park_requests[1]);
    machine_reported(
        &m,
        "REFUSED: a core whose park was refused counted confirmed",
    );
}

// D-149: a core running a node with an in-service bit left from its last.
#[test]
fn a_core_running_with_an_in_service_bit_of_its_last_node_is_reported() {
    let mut m = S::two();
    m.sys.in_service[1] = true;
    machine_reported(
        &m,
        "ISR: a core runs a node with an in-service bit from its last one",
    );
}

// D-149: a dead generation whose device's attach word still reads
// attached (R6 cleared it before R1's gate wrote it detached). The nodes'
// own states are not judged here (the spoiled words are no kernel's).
#[test]
fn a_dead_generations_device_still_attached_is_reported() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let g = m.sys.member(1).generation();
    m.ok(0, super::pass17::kill(1));
    m.run(100 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    for n in 0..3 {
        m.sys.make_faulty(n);
    }
    let key = 0x10;
    m.sys.edit_table(|t| {
        let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(1), g).raw();
        t.identifiers.insert(key, taken);
        let word = dodos_abi::AttachWord::new(dodos_abi::AttachState::Attached, 0x1000).unwrap();
        t.attach.insert(key, word.raw());
    });
    machine_reported(&m, "R6: a dead generation's device still attached");
}

// D-148: a gate word whose grant count is not the grants over its chunk.
#[test]
fn a_grant_count_that_is_not_the_grants_over_its_chunk_is_reported() {
    let g = super::pass24::granted(false);
    let mut m = g.m;
    m.sys.edit_table(|t| {
        let word = t.gate_word(g.chunk).with_grants(0).unwrap();
        std::sync::Arc::make_mut(&mut t.gate_words)[g.chunk as usize] = word.raw();
    });
    machine_reported(
        &m,
        "G4: a grant count that is not the grants over its chunk",
    );
}

// D-148: a page under a live grant's anchor of a class other than data.
#[test]
fn a_page_under_a_grants_anchor_not_of_class_data_is_reported() {
    let g = super::pass24::granted(false);
    let mut m = g.m;
    m.sys.make_faulty(super::pass24::X);
    let base = m.sys.node(super::pass24::X).frame_base(g.frame);
    m.sys.edit_table(|t| {
        let page = base / t.geometry.page();
        t.set_class(page, Some(dodos_abi::PageClass::table(1).unwrap()));
    });
    // D-154: a page that is not *data* in an ordinary chunk is refused
    // first by the gate word's own rule, on every node that reads the
    // word; G5's rule stays behind it.
    machine_reported(&m, "TABLES: a gate word no gate writes");
}

// D-148: granted memory given to another node while its granter is live.
#[test]
fn granted_memory_owned_by_another_node_is_reported() {
    let g = super::pass24::granted(false);
    let mut m = g.m;
    m.sys.make_faulty(super::pass24::X);
    m.sys.edit_table(|t| {
        let free = dodos_abi::Ownership::free(dodos_abi::Owner::Node(1)).raw();
        std::sync::Arc::make_mut(&mut t.owners)[g.chunk as usize] = free;
    });
    machine_reported(&m, "G5: granted memory owned by another than its granter");
}

// D-148: a level line claimed beside other nodes on x86-64 without a
// directed end of interrupt (the claim made where it was directed, the
// machine then read without it).
#[test]
fn a_level_line_without_a_directed_end_of_interrupt_beside_other_nodes_is_reported() {
    let (mut m, n, _) = super::pass28::line_bound_on(|holder, _, _| holder);
    for z in 0..3 {
        m.sys.node_mut(z).machine.directed_eoi = false;
    }
    let report = m.sys.check().expect_err("the spoiled machine");
    assert_eq!(
        report,
        format!(
            "node {n}: LEVEL: a level line with no directed end of interrupt beside other nodes"
        )
    );
}
