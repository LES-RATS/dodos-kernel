//! The scenarios with which the reviews broke the text, each a test: the
//! findings confirmed in the verifications of the eighth review
//! (`Docs/reviews/2026-11-16-contract-review-8-verify-*.md`) and in those of
//! the kernel-shape proposals (`2026-11-16-verify-kernel-shape-*.md`).
//!
//! Where another module already holds the scenario, the table says which
//! test; the others are here. Findings about wording, or about mechanisms
//! the decided shape removed (the moves of cores between kernels, the flush
//! epoch), have no test: the table says so.
//!
//! | Finding | Test |
//! |---|---|
//! | Hydrogen 1: a `UnitRequest` of an old or handing epoch is dropped | `units::a_request_answered_elsewhere_is_sent_again_with_the_epoch_of_the_new_holder` |
//! | Hydrogen 2: a resume drops the answers that waited | `crossing::a_resume_keeps_its_consumer_indices_and_consumes_the_answers_that_waited`, `hydrogen_2_*` here |
//! | Hydrogen 3: a `Release` crossing a `Revoke` | `crossing::a_release_that_crosses_a_revoke_ends_the_revocation`, `explore::crossing_*` |
//! | Hydrogen 4: a thread destroyed while it waits for a `SendAnswer` | `crossing::a_send_answer_is_applied_whether_or_not_the_sending_thread_still_waits`, `explore::waiter_*` |
//! | Hydrogen 5: the reports of a rebuild (cursor per unit, sealed and starting nodes, the end) | `units::the_rebuild_of_a_unit_keeps_the_entries_of_live_nodes_and_clears_those_of_no_report`, `units::the_rebuild_keeps_without_a_report_the_entries_of_a_sealed_node` |
//! | Hydrogen 6: a faulty peer's ring (unreadable header, producer too far ahead) | `crossing::an_unreadable_header_is_consumed_as_one_entry`, `crossing::a_producer_index_too_far_ahead_*` |
//! | Hydrogen 7, Opus N3: the take-over of a handing word judged on its target | `units::a_handing_word_whose_target_dies_is_taken_over_by_the_targets_successor`, `hydrogen_n3_*` here |
//! | Hydrogen 8: sealed before the last park request; `panic`; the two-node seal | `membership::a_seal_writes_sealed_only_after_the_last_park_request_*`, `membership::a_sealing_node_returns_to_live_*`, `hydrogen_8_*` here |
//! | Hydrogen 9, Opus N4: the park stub writes `parked` at every wake; the bootstrap core | `hydrogen_9_*` here |
//! | Hydrogen 10: ARM64, a core already off confirms the new park request | `hydrogen_10_*` here |
//! | Hydrogen 11: `idle_until` with no armed timer; a busy core beats | `hydrogen_11_*` here, `membership::nodes_that_beat_are_never_accused` |
//! | Hydrogen 12: `node_watch` on the own node; sends kept for `manage` | `membership::node_watch_*`, `crossing::node_send_is_busy_beyond_the_sends_in_flight_*` |
//! | Opus N1: the reserve of answers never overflows | invariant RESERVE, `crossing::the_kernel_ring_keeps_its_reserve_of_answers_free_of_requests` |
//! | Opus N2: a removal done for a `Revoke` sends no `Release` | `hydrogen_n2_*` here |
//! | Opus N5: a `Revoke` step that needs a unit's invalidation | `hydrogen_n5_*` here |
//! | DeepSeek 1: an idle core and the invalidation | `cores::an_idle_core_is_not_asked_and_owes_the_invalidation_at_its_wake` |
//! | DeepSeek 2: a marked thread is not runnable | `cores::sched_configure_that_moves_a_running_context_marks_*` |
//! | DeepSeek 3: the record *unmapping* is busy | `deepseek_3_*` here |
//! | DeepSeek 4: an interrupt entry and an untyped under L2 | `deepseek_4_*` here |
//! | DeepSeek 5: ARM64 `CnP` | wording of the architecture: not modelled |
//! | DeepSeek 6: no unit wait inside a hold; a unit's failure is not the node's | invariant "no lock is held across a wait", `units::a_unit_that_does_not_confirm_within_iommu_wait_ns_has_failed` |
//! | DeepSeek 8: a second suspend | `cores::thread_suspend_of_a_thread_on_another_core_returns_again_while_it_is_marked` |
//! | DeepSeek 9: the audit of the ownership words (R4) | `tests/blocks.rs` (the stray write that stops the machine) |
//! | DeepSeek 10: the gate lock last and counted in *k* | `cores::under_l2_a_call_takes_its_locks_in_the_order_of_their_kind_the_gate_lock_last` |
//! | DeepSeek 11: `send` for `grant_signal` | `crossing::grant_signal_is_invalid_on_a_frame_that_is_not_remote_and_needs_send` |
//! | DeepSeek 12 to 14 | wording |
//! | DeepSeek 15: `busy` for a full grant table | `deepseek_15_*` here |
//! | Round 2, DeepSeek 4, first round's Test 4: the park request before the mark, the mark before the stop | invariants "fence: ..." of `System::check`, `kernel_shape_*_request_before_mark` here |
//! | Round 2, DeepSeek 5: a take writes the taker's current generation | `membership::a_fresh_start_on_several_cores_writes_joining_then_owned_then_live` |
//! | Round 2, DeepSeek 6 (`r6_inflight`): R6 waits `SETTLE_NS` after the last stop | `kernel_shape_r6_*` here |
//! | Round 2, DeepSeek 7, first round's Test 2: the class rule on the taker's cores | `kernel_shape_*_class_*` here |
//! | Round 2, DeepSeek 8: the bootstrap core is lost when its kernel dies | `hydrogen_9_*` here (started again by its NMI) |
//! | Round 2, Hydrogen 1: `CoreWord` ABA on *leaving* | invariant "core word: leaving is written" |
//! | Round 2, Hydrogen 2: a park request that wraps | invariant "fence: a park request went back" |
//! | Round 2, Hydrogen 3: a resume omits R0 | `kernel_shape_a_resume_runs_r0_*` here |
//! | Round 2, Hydrogen 4: one ring shared by kernel and user messages | `kernel_shape_kernel_messages_*` here |
//! | Round 2, Hydrogen 5: the rebuild forgets the floors | `units::the_rebuild_keeps_without_a_report_the_entries_of_a_sealed_node` |
//! | Round 2, Hydrogen 6: write skew of role words | the role words change only by compare-and-swap through the gate (`table::write_role`), checked by ONE-HOLDER |
//! | First round, `fence_owned`: the fence covers joining cores | `kernel_shape_the_fence_stops_the_joining_cores_*` here |
//! | Ninth review 1: a late start wake, the start request's layout, a fence NMI in the entry stub | `pass17::a_start_wake_that_reaches_a_core_already_running_changes_nothing`, `pass17::a_fence_nmi_that_lands_in_the_entry_stub_gives_no_false_confirmation` |
//! | Ninth review 2: the fence record by core number | `abi` `a_fence_record_indexes_its_last_stops_by_core_number_modulo_64`, `mistral::f3_*` (R6 by the core's own last stop) |
//! | Ninth review 3: `cap_delete` of an invalid remote capability with a mapping | `crossing::cap_delete_of_an_invalid_remote_capability_with_a_mapping_undoes_the_mapping` |
//! | Ninth review 4: the unit sequence of a claim | `pass17::a_claim_passes_a_failed_unit_*`, `pass17::a_claim_whose_waiting_thread_is_destroyed_*`, `pass17::a_claim_of_a_remapped_line_*` |
//! | Ninth review 5: domain numbers and the operands of a unit request | `pass17::the_holder_refuses_*`, `pass17::each_node_slot_takes_its_domain_numbers_*`, `pass17::the_start_up_refuses_a_unit_*` |
//! | Ninth review 6: the order of L2 | `pass17::under_l2_a_call_records_its_locks_*`, `cores::the_check_refuses_a_lock_order_the_contract_forbids` |
//! | Ninth review 7: one node on a machine of class 0 | `pass17::one_node_on_a_machine_of_class_0_*` |
//! | Eighth model review, DeepSeek 1 to 3 and Hydrogen's table row 17 | `pass17::a_thread_attached_to_no_task_*`, `pass17::a_send_whose_stamp_*`, `pass17::reserve_keeps_room_*`, invariant "fence: a stop sent before its word was marked", `pass17::a_resume_after_a_peer_died_*` |
//! | Eighth model review, Mistral F1 to F15 | `mistral.rs` (one test per mutation), `checker.rs` and `check/tests.rs` (the checker fed a bad state) |

use super::*;
use crate::Record;
use crate::system::Hardware;
use dodos_abi::{
    CoreState, CoreWord, CrashReason, Evidence, NODE_START_FRESH, NODE_START_RESUME,
    NODE_STOP_KILL, NODE_STOP_SEAL, RoleState, SETTLE_NS, START_SLOT_MAILBOX,
};

fn kill(n: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode: NODE_STOP_KILL,
        core: 0,
    }
}

fn seal(n: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode: NODE_STOP_SEAL,
        core: 0,
    }
}

fn resume(n: u16) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_RESUME,
        image: 0,
        module: 0,
        window: 0,
        mask: 0,
    }
}

fn fresh(n: u16, mask: u64) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_FRESH,
        image: 0,
        module: 0,
        window: 0,
        mask,
    }
}

fn recv(first: u32) -> Syscall {
    Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(100),
        range: crate::Range {
            first: s(first),
            count: 1,
        },
        wait: 0,
    }
}

fn grant(caps: Vec<crate::Transfer>) -> Syscall {
    Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message {
            caps,
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    }
}

/// Node 0 grants node 1 its frame at slot 101; node 1 holds it at 110.
fn granted(m: &mut S) {
    let reply = m.retype(1, ObjectKind::Reply, 0, 100);
    m.ok(1, reply);
    let frame = m.retype(0, ObjectKind::Frame, 1, 101);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv(110)), WAITS);
    let rights = Rights::READ.union(Rights::WRITE).bits();
    assert_eq!(
        m.call(0, grant(vec![crate::Transfer::Derive(s(101), rights)])),
        WAITS
    );
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

// The eighth review, Hydrogen and its verifications.

#[test]
fn hydrogen_2_a_thread_that_waits_for_an_answer_while_its_node_seals_gets_it_at_the_resume() {
    let mut m = S::two();
    // Node 1 sends to node 0, whose mailbox has no receiver: the answer is
    // `would_wait`. Before node 0 serves it, node 1 is asked to seal.
    let send = Syscall::NodeSend {
        node: node_cap(0),
        message: crate::Message::default(),
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(1, send), WAITS);
    m.ok(0, seal(1));
    m.run(5 * MS);
    if m.state(1) == MemberState::Sealed {
        m.ok(0, resume(1));
        m.run(5 * MS);
    }
    assert_eq!(m.state(1), MemberState::Live);
    // The answer was consumed, before the seal or at the resume: the wait
    // ended with it, never with `gone`.
    assert_eq!(m.woken(1), Some(&Err(Error::WouldWait)));
}

#[test]
fn hydrogen_8_a_sealing_node_whose_panic_is_set_does_not_write_sealed_and_is_accused_self() {
    let mut m = S::two();
    m.ok(0, seal(1));
    assert_eq!(m.state(1), MemberState::Sealing);
    m.sys.fail(1, 0);
    m.run(10 * MS);
    let member = m.sys.member(1);
    assert_eq!(member.evidence(), Evidence::SelfReported);
    assert_ne!(member.state(), MemberState::Sealed);
}

#[test]
fn hydrogen_9_a_bootstrap_core_that_parked_itself_is_confirmed_at_the_fence_nmi_and_started_again()
{
    let mut m = S::two();
    // Node 0, on the bootstrap core, stops itself: its core parks in the
    // stub before the fence; the fence's NMI is answered by the stub.
    m.ok(0, kill(0));
    m.run(10 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    assert!(matches!(m.sys.hardware(0), Hardware::Stub { .. }));
    // The bootstrap core is started again by an NMI to the stub, never by
    // INIT: the core is not lost for the rest of the boot.
    m.ok(1, fresh(0, 0b1));
    m.run(3 * MS);
    assert_eq!(m.state(0), MemberState::Live);
    assert_eq!(m.sys.hardware(0), Hardware::Running);
}

#[test]
fn hydrogen_10_an_arm64_core_already_off_confirms_the_new_park_request() {
    let mut m = S::on(&machine(Arch::Aarch64, 3, &[&[0], &[1, 2]]));
    m.ok(0, seal(1));
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // A sealed node accused with kill: its parked cores are confirmed at
    // once by their answer to the new park request.
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn hydrogen_11_a_core_idle_with_no_armed_timer_is_never_stale() {
    let mut k = N::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    // Node 1 is a phantom here: give it core 1, owned, idle with no armed
    // timer, observed long ago with the heartbeat it still has.
    let generation = k.m.table.generation(1);
    k.m.table.core_words[1] = CoreWord::owned(1, generation).raw();
    k.m.table.core_beacons[1].idle_until.0 = u64::MAX;
    k.m.table.beacons[0]
        .observations
        .push(crate::table::Observation {
            node: 1,
            watched: crate::table::Watched::Heartbeat {
                core: 1,
                heartbeat: k.m.table.core_beacons[1].heartbeat.0,
            },
            at: 0,
        });
    k.m.now = 50 * dodos_abi::STALE_NS;
    assert_eq!(k.m.evidence(1), None);
    // Busy, the same core is stale.
    k.m.table.core_beacons[1].idle_until.0 = 0;
    assert_eq!(k.m.evidence(1), Some(Evidence::Stale));
}

#[test]
fn hydrogen_n2_a_removal_done_for_a_revoke_sends_no_release() {
    let mut m = S::two();
    granted(&mut m);
    m.ok(
        1,
        Syscall::ASpaceMap(crate::Map {
            space: s(dodos_abi::START_SLOT_ASPACE),
            frame: s(110),
            untyped: SlotAddr::NONE,
            address: 1 << 30,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    assert_eq!(m.call(0, Syscall::CapRevoke(s(101))), WAITS);
    m.run(30 * MS);
    assert!(m.sys.node(0).grant_entries().is_empty());
    // A `Release` after the last `RevokeStep` would name a freed grant and
    // be refused: none was sent.
    assert_eq!(m.sys.node(0).refused.0, 0);
    assert!(m.sys.canonical().uses[1].is_empty());
}

#[test]
fn hydrogen_n3_the_node_that_hands_dies_after_step_4_and_the_target_completes_step_5_alone() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let role = s(slots_of(m.sys.node(1), m.t[1], ObjectKind::Role)[0]);
    let _ = m.call(
        1,
        Syscall::RoleHand {
            role,
            node: node_cap(2),
        },
    );
    for _ in 0..200 {
        if m.sys.role(0).state() == RoleState::Handing {
            break;
        }
        m.run(100_000);
    }
    if m.sys.role(0).state() == RoleState::Handing {
        m.ok(0, kill(1));
    }
    m.run(30 * MS);
    // Only the target took the role; ONE-HOLDER held at every step.
    let word = m.sys.role(0);
    assert_eq!((word.state(), word.holder()), (RoleState::Held, 2));
}

#[test]
fn hydrogen_n5_a_revoke_step_that_needs_a_units_invalidation_completes_its_record_by_a_unit_request()
 {
    let mut m = S::two();
    granted(&mut m);
    // Node 1 (which holds the IOMMU role) maps the remote frame in an
    // iospace that serves a device.
    let iospace = m.retype(1, ObjectKind::IoSpace, 0, 150);
    m.ok(1, iospace);
    let iommu = s(slots_of(m.sys.node(1), m.t[1], ObjectKind::Iommu)[0]);
    let attach = Syscall::IoSpaceAttach {
        iommu,
        iospace: s(150),
        device: 0x10,
    };
    let outcome = m.call(1, attach);
    if outcome == WAITS {
        m.run(20 * MS);
    }
    let untyped = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(
        1,
        Syscall::IoSpaceMap(crate::Map {
            space: s(150),
            frame: s(110),
            untyped: s(untyped),
            address: 1 << 20,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    // The granter revokes: the holder's step clears the record, which is
    // left unmapping, and completed by a unit request no thread waits for.
    assert_eq!(m.call(0, Syscall::CapRevoke(s(101))), WAITS);
    let mut unmapping = false;
    for _ in 0..20 {
        // Event by event, so that the state between the step and the
        // unit's confirmation is seen.
        for _ in 0..200 {
            if m.sys.events().is_empty() {
                m.run(MS);
            } else {
                let event = m.sys.events()[0];
                m.sys.fire(event);
                assert_eq!(m.sys.check(), Ok(()));
            }
            unmapping |= m
                .sys
                .node(1)
                .caps
                .values()
                .any(|c| matches!(c.record, Record::Unmapping(_)));
        }
        if m.sys.node(0).grant_entries().is_empty() {
            break;
        }
        // The step returned `again`: the caller repeats, and the next
        // `Revoke` goes on once the record is complete.
        let _ = m.call(0, Syscall::CapRevoke(s(101)));
    }
    assert!(unmapping, "the step left the record unmapping");
    m.run(30 * MS);
    assert!(m.sys.node(0).grant_entries().is_empty());
    assert!(
        m.sys
            .node(1)
            .caps
            .values()
            .all(|c| !matches!(c.record, Record::Unmapping(_)))
    );
    assert!(m.sys.node(1).unit_requests.iter().all(Option::is_none));
}

// The eighth review, DeepSeek and its verification.

#[test]
fn deepseek_3_a_record_unmapping_is_busy_for_unmap_and_protect() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let other = k.thread_on(300, 1);
    k.interrupts();
    k.ok(k.retype(ObjectKind::Frame, 1, 200));
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(200),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.union(Rights::WRITE).bits(),
    });
    k.ok(map);
    // Core 1 has the space loaded: the unmap on core 0 waits for it.
    assert_eq!(k.call(k.t, Syscall::ASpaceUnmap(s(200))), WAITS);
    // The other thread, on core 1, finds the record unmapping: busy, for an
    // unmap and for a protect.
    assert_eq!(
        k.call(other, Syscall::ASpaceUnmap(s(200))),
        Err(Error::Busy)
    );
    let protect = Syscall::ASpaceProtect {
        aspace: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(200),
        rights: Rights::READ.bits(),
    };
    assert_eq!(k.call(other, protect), Err(Error::Busy));
}

#[test]
fn deepseek_15_node_send_is_busy_when_the_grant_table_has_fewer_free_entries_than_frames() {
    let mut k = N::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    k.ok(k.retype(ObjectKind::Frame, 1, 200));
    let taken = crate::table::GrantEntry {
        grantee: 1,
        generation: 1,
        serial: 1,
        state: crate::table::GrantState::Live,
        use_index: None,
        answered: true,
        doorbell: None,
    };
    // Every grant entry but none is used (a state the checks would refuse,
    // for its anchors are missing: the call alone is judged).
    let me = k.m.node as usize;
    for index in 0..dodos_abi::GRANTS as u16 {
        k.m.table.grants[me].insert(index, taken);
    }
    let send = grant(vec![crate::Transfer::Derive(s(200), Rights::READ.bits())]);
    assert_eq!(k.m.apply(k.t, send), Err(Error::Busy));
}

// The verifications of the kernel-shape proposals.

#[test]
fn kernel_shape_the_fence_raises_the_park_request_before_it_marks_the_word_request_before_mark() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2, 3]]));
    let before: Vec<u64> = (1..4).map(|c| m.sys.park_request(c)).collect();
    m.ok(0, kill(1));
    // Every step checks the invariants "fence: a word marked before its
    // park request was raised" and "a stop in flight under a word that is
    // not marked".
    let mut rng = crate::system::Rng(7);
    m.shuffle(&mut rng, 50);
    for core in 1..4 {
        if CoreWord::from_raw(m.sys.core_word(core))
            .unwrap()
            .is_marked()
        {
            assert!(m.sys.park_request(core) > before[core - 1]);
        }
    }
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn kernel_shape_r6_frees_a_core_word_only_settle_ns_after_the_last_stop_sent_to_it() {
    let mut m = S::two();
    m.ok(0, kill(1));
    let mut freed = None;
    for _ in 0..200 {
        m.run(50_000);
        let word = CoreWord::from_raw(m.sys.core_word(1)).unwrap();
        if word.state() == CoreState::Free {
            freed = Some(m.sys.now());
            break;
        }
    }
    let freed = freed.expect("the word is freed");
    let last_stop = m
        .sys
        .sent
        .iter()
        .filter(|(_, _, a)| *a == crate::nodes::HwAction::Stop { core: 1 })
        .map(|(at, _, _)| *at)
        .max()
        .expect("a stop");
    assert!(freed >= last_stop + SETTLE_NS, "{freed} {last_stop}");
}

#[test]
fn kernel_shape_a_unit_role_is_not_handed_to_a_node_with_a_core_of_a_lesser_class() {
    // Node 1 covers cores 1 and 2, core 2 of fence class 1: the node is of
    // class 1. Node 2, of class 2, holds the units.
    let mut machine = machine(Arch::X86_64, 4, &[&[0], &[1, 2], &[3]]);
    machine.cores[2].fence = 1;
    let mut m = S::on(&machine);
    assert_eq!(m.sys.member(1).class(), 1);
    assert_eq!(m.sys.role(0).holder(), 2);
    let role = s(slots_of(m.sys.node(2), m.t[2], ObjectKind::Role)[0]);
    m.no(
        2,
        Syscall::RoleHand {
            role,
            node: node_cap(1),
        },
        Error::Busy,
    );
}

#[test]
fn kernel_shape_the_fence_stops_the_joining_cores_of_a_starting_node() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2], &[3]]));
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, fresh(1, 0b110));
    // Accused while its words read joining, before any core entered.
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    for core in [1, 2] {
        assert_eq!(
            CoreWord::from_raw(m.sys.core_word(core)).unwrap().state(),
            CoreState::Free
        );
        assert_ne!(m.sys.hardware(core), Hardware::Running);
    }
}

#[test]
fn kernel_shape_kernel_messages_are_not_blocked_by_a_full_user_ring() {
    let mut k = N::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let pair = &mut std::sync::Arc::make_mut(&mut k.m.table.rings)[0][1];
    pair.user.produced = dodos_abi::RING_USER_ENTRIES;
    assert!(!k.m.ring_room(1, dodos_abi::NodeMessage::Send, 1));
    assert!(k.m.ring_room(1, dodos_abi::NodeMessage::Revoke, 1));
    assert!(k.m.ring_room(1, dodos_abi::NodeMessage::UnitRequest, 1));
}

#[test]
fn kernel_shape_a_resume_runs_r0_for_a_generation_that_ended_while_the_node_was_sealed() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    granted(&mut m);
    assert_eq!(m.sys.canonical().uses[1].len(), 1);
    m.ok(2, seal(1));
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // The granter's generation ends while the holder is sealed.
    m.ok(2, kill(0));
    m.run(10 * MS);
    m.ok(2, resume(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    // The resume dropped the remote frame of the ended generation, with no
    // release.
    assert!(m.sys.node(1).remotes.is_empty());
    assert!(m.sys.canonical().uses[1].is_empty());
    assert_eq!(m.sys.member(0).state(), MemberState::Dead);
    let _ = CrashReason::Fenced;
}
