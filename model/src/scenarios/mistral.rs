//! The mutations of the eighth model review by Mistral
//! (`Docs/reviews/2026-11-16-model-review-8-mistral.md`, F1 to F14) that
//! left every test green: one test each that fails under the mutation and
//! names the sentence of the contract it guards. F1 is caught by the
//! invariant of the fence's order inside the event (`check_fence_order`)
//! and the scenario `kernel_shape_*_request_before_mark`; F15 is in
//! `checker.rs`.

use super::pass17::{fresh, kill};
use super::*;
use crate::cores::Flush;
use crate::table::{GrantState, Observation, UseEntry, Watched};
use dodos_abi::{CoreState, CoreWord, SETTLE_NS};

// F2: "and it cancels every start request aimed at X" (The fence).

#[test]
fn f2_the_first_round_of_a_fence_cancels_the_start_requests_aimed_at_the_node() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 4, &[&[0], &[1, 2], &[3]]));
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // Node 1 starts again on cores 1 and 2, and is accused before core 2,
    // hung, took its request: the fence's first round cancels it (core 1
    // may have taken its own; the final review's E13).
    m.sys.hang(2, false);
    m.ok(0, fresh(1, 0b110));
    for _ in 0..1000 {
        if m.sys.canonical().start_requests[2] != 0 {
            break;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    }
    assert_ne!(m.sys.canonical().start_requests[2], 0);
    m.ok(0, kill(1));
    for _ in 0..1000 {
        let generation = m.sys.member(1).generation();
        if m.sys.canonical().fences[1].begun(generation).is_some() {
            break;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    }
    assert_eq!(m.state(1), MemberState::Fencing);
    assert_eq!(m.sys.canonical().start_requests[2], 0);
}

// F3: a core word becomes free only when "the core is confirmed stopped,
// and SETTLE_NS have passed since the last stop sent to it" (Node slots and
// core words, the transitions).

/// One event of a fixed order, or time moved on by 10 µs when none can
/// happen now.
fn tick(sys: &mut crate::system::System) {
    match sys.events().first() {
        Some(&event) => sys.fire(event),
        None => sys.move_clock(sys.now() + 10_000),
    }
}

#[test]
fn f3_r6_keeps_a_core_word_until_settle_ns_after_the_last_stop_even_when_confirmed() {
    let mut m = S::two();
    m.ok(0, kill(1));
    let free = |sys: &crate::system::System| {
        CoreWord::from_raw(sys.core_word(1)).is_some_and(|w| w.state() == CoreState::Free)
    };
    // A copy of the machine finds the tick at which R6 frees the core word.
    let mut probe = m.sys.clone();
    let mut ticks = 0;
    while !free(&probe) {
        tick(&mut probe);
        ticks += 1;
        assert!(ticks < 100_000);
    }
    // The machine goes to the tick before it; a stop was sent to the core
    // just then (a stop repeated by a driver that took over): the core is
    // confirmed, but R6 waits SETTLE_NS before it frees the word.
    for _ in 1..ticks {
        tick(&mut m.sys);
    }
    assert!(!free(&m.sys));
    let now = m.sys.now();
    m.sys.edit_table(|t| {
        t.fences[1].stopped(1, now);
    });
    tick(&mut m.sys);
    m.sys.settle(100_000);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(!free(&m.sys), "freed before SETTLE_NS after the last stop");
    m.run(SETTLE_NS + 20 * MS);
    assert!(free(&m.sys));
}

// F4: the barrier holds only when "no context entry of any IOMMU unit
// names X's generation" (The reclaim barrier).

#[test]
fn f4_the_barrier_does_not_hold_while_a_context_entry_names_the_generation() {
    let mut m = S::two();
    m.ok(0, kill(1));
    for _ in 0..2000 {
        if m.state(1) == MemberState::Fenced && m.sys.node(0).barrier_holds(1) {
            break;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    }
    assert!(m.sys.node(0).barrier_holds(1));
    let g = m.sys.member(1).generation();
    // A context entry of the unit still names the dead node's generation
    // (a holder that acknowledged without clearing it, or a state the
    // explorations do not reach): the barrier must not hold.
    m.sys.edit_table(|t| {
        t.units.entry(0).or_default().entries.insert(0x10, (1, g));
    });
    assert!(!m.sys.node(0).barrier_holds(1));
}

// F5: stale evidence when "now − t ≥ STALE_NS" (Detection), at the
// boundary.

#[test]
fn f5_a_heartbeat_unchanged_for_exactly_stale_ns_is_stale_evidence() {
    let mut m = S::two();
    m.run(MS);
    let heartbeat = m.sys.canonical().core_beacons[1].heartbeat.0;
    let at = m.sys.now();
    let stale = m.sys.canonical().stale_ns;
    m.sys.edit_table(|t| {
        t.core_beacons[1].idle_until.0 = 0;
        t.beacons[0].observations.push(Observation {
            node: 1,
            watched: Watched::Heartbeat { core: 1, heartbeat },
            at,
        });
    });
    m.sys.move_clock(at + stale - 1);
    assert_eq!(m.sys.node(0).evidence(1), None);
    m.sys.move_clock(at + stale);
    assert_eq!(m.sys.node(0).evidence(1), Some(dodos_abi::Evidence::Stale));
}

// F6: a remote frame capability is valid only if "the granting node's
// member word names the generation recorded in the use entry" (Grants).

#[test]
fn f6_a_remote_capability_whose_use_entry_names_another_generation_is_invalid() {
    let mut m = S::two();
    let g = m.sys.member(0).generation();
    let grantee_generation = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        t.grants[0].insert(
            3,
            crate::table::GrantEntry {
                anchor: crate::table::Anchor::test(),
                state: GrantState::Live,
                serial: 9,
                grantee: 1,
                generation: grantee_generation,
                use_index: Some(5),
                answered: true,
                doorbell: None,
            },
        );
        t.uses[1].insert(
            5,
            UseEntry {
                granter: 0,
                grant: 3,
                serial: 9,
                generation: g + 2,
                root: ObjId(0),
                leaves: 0,
                release_pending: false,
                signal_pending: false,
            },
        );
    });
    let remote = crate::grants::Remote {
        node: 0,
        grant: 3,
        serial: 9,
        use_entry: 5,
        depth: 1,
        rights: Rights::READ,
    };
    assert!(!m.sys.node(1).valid(&remote));
    m.sys
        .edit_table(|t| t.uses[1].get_mut(&5).unwrap().generation = g);
    assert!(m.sys.node(1).valid(&remote));
}

// F7: "failed empties the record with its counts kept" (Devices and shared
// hardware, unit requests).

#[test]
fn f7_an_unmap_whose_unit_failed_empties_the_record_and_keeps_the_map_count() {
    let mut m = S::two();
    // Node 0 maps a frame of its own in an iospace that serves a device
    // behind the unit node 1 holds (the IOMMU role is node 1's; the final
    // review's E12): the attach and the unmap cross as unit requests.
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, 150);
    m.ok(0, iospace);
    let frame = m.retype(0, ObjectKind::Frame, 1, 151);
    m.ok(0, frame);
    let iommu = s(slots_of(m.sys.node(0), m.t[0], ObjectKind::Iommu)[0]);
    let attach = Syscall::IoSpaceAttach {
        iommu,
        iospace: s(150),
        device: 0x10,
    };
    assert_eq!(m.sys.role(0).holder(), 1);
    assert_eq!(m.call(0, attach), WAITS);
    m.run(20 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    let untyped = untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(
        0,
        Syscall::IoSpaceMap(crate::Map {
            space: s(150),
            frame: s(151),
            untyped: s(untyped),
            address: 1 << 20,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    let object = m.sys.node(0).object_at(m.t[0], s(151)).unwrap();
    let chunk = m.sys.node(0).frames[&object].base >> m.sys.node(0).geometry().chunk_shift();
    let count = m.sys.node(0).map_count(chunk);
    assert!(count > 0);
    // The unit stops confirming: the unmap ends `busy`, its record is
    // emptied, and the count of its leaf is never lowered.
    m.sys.break_unit(0);
    assert_eq!(m.call(0, Syscall::IoSpaceUnmap(s(151))), WAITS);
    m.run(dodos_abi::IOMMU_WAIT_NS + 30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    assert_eq!(m.sys.node(0).map_count(chunk), count);
    // The record is emptied, as the name says (held by the test, not only
    // by invariant 36).
    let node = m.sys.node(0);
    let held = node.caps.values().find(|c| c.object == object);
    assert_eq!(held.map(|c| c.record), Some(crate::Record::Empty));
}

// F8: at step 4 "the gate refuses if Y is no longer live; X then maps the
// unit again, serves it again and stays the holder, and the call returns
// busy" (Roles, planned hand-over).

#[test]
fn f8_a_hand_over_whose_target_died_before_step_4_returns_busy_and_writes_no_handing() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let role = 0;
    let holder = m.sys.role(role).holder();
    // The target is of the best class (node 0 holds the bootstrap core, of
    // class 1, and could not be a target).
    let target = (1..3).find(|&n| n != holder).unwrap();
    let third = (0..3).find(|&n| n != holder && n != target).unwrap();
    let slot = s(slots_of(m.sys.node(holder), m.t[holder as usize], ObjectKind::Role)[role]);
    let hand = Syscall::RoleHand {
        role: slot,
        node: node_cap(target),
    };
    // The holder's unit does not drain at once, so the hand-over waits
    // before step 4; the target is accused meanwhile.
    m.sys.edit_table(|t| {
        let unit = t.units.entry(role).or_default();
        unit.broken = true;
        unit.queue.push((unit.next, 0));
    });
    assert_eq!(m.call(holder, hand), WAITS);
    m.ok(third, kill(target));
    m.sys
        .edit_table(|t| t.units.entry(role).or_default().broken = false);
    m.run(30 * MS);
    assert_eq!(m.woken(holder), Some(&Err(Error::Busy)));
    let word = m.sys.role(role);
    assert_eq!(word.state(), dodos_abi::RoleState::Held);
    assert_eq!(word.holder(), holder);
}

// F9: the header of an aspace "holds the set of the node's cores that have
// loaded its tag since it was last invalidated on them" (TLB).

#[test]
fn f9_a_whole_tag_invalidated_on_a_core_takes_the_core_out_of_the_set() {
    let mut k = N::on(&one_node(crate::Arch::X86_64, 2));
    let space = k.m.threads[&k.t].aspace.unwrap();
    k.m.aspaces.get_mut(&space).unwrap().cores |= 0b10;
    k.m.invalidate_on(1, &Flush::Tag(space));
    assert_eq!(k.m.aspaces[&space].cores & 0b10, 0);
}

// F10: "the unmapping of a run from the kernel's own space asks every
// other core of the node, idle ones included" (TLB), from a core that is
// not core 0.

#[test]
fn f10_a_give_from_a_core_other_than_the_first_asks_every_other_core() {
    let mut k = N::on(&machine(crate::Arch::X86_64, 4, &[&[0, 1, 2], &[3]]));
    k.place_first(0);
    let other = k.thread_on(300, 1);
    k.interrupts();
    let shift = k.m.geometry().chunk_shift();
    let free = (0..1u64 << (k.m.machine.phys_bits - shift))
        .find(|&c| {
            k.m.ownership(c) == dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw()
        })
        .expect("a free chunk");
    let take = Syscall::MemoryTake {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: free << shift,
        size: u64::from(shift),
        destination: s(220),
    };
    assert_eq!(k.call(other, take), DONE);
    let give = Syscall::MemoryGive {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        untyped: s(220),
        node: 1,
    };
    assert_eq!(k.call(other, give), WAITS);
    let request = k.m.cores[1].request.clone().expect("core 1's request");
    assert_eq!(request.asked, 0b101);
}

// F11: "REQUESTS_PER_PEER (16) request credits" (What crosses between
// nodes, credits).

#[test]
fn f11_a_node_has_sixteen_request_credits_toward_each_peer() {
    let mut peer = crate::rings::Peer::new();
    for index in 0..dodos_abi::REQUESTS_PER_PEER {
        let free = peer.free(false).expect("a request credit");
        assert_eq!(free as usize, index);
        peer.credits[index] = Some(crate::rings::Credit {
            kind: dodos_abi::NodeMessage::Revoke,
            serial: 1,
            completes: crate::rings::Completes::Revoke {
                grant: index as u16,
            },
        });
    }
    assert_eq!(peer.free(false), None);
}

// F12: pending signals "are sent when room comes, in the order of the use
// table, at most 32 per event" (What crosses between nodes).

#[test]
fn f12_at_most_32_pending_signals_are_sent_in_one_event() {
    let mut m = S::two();
    let g = m.sys.member(0).generation();
    let grantee_generation = m.sys.member(1).generation();
    m.sys.edit_table(|t| {
        for grant in 0..33u16 {
            t.grants[0].insert(
                grant,
                crate::table::GrantEntry {
                    anchor: crate::table::Anchor::test(),
                    state: GrantState::Live,
                    serial: 1,
                    grantee: 1,
                    generation: grantee_generation,
                    use_index: Some(grant),
                    answered: true,
                    doorbell: None,
                },
            );
            t.uses[1].insert(
                grant,
                UseEntry {
                    granter: 0,
                    grant,
                    serial: 1,
                    generation: g,
                    root: ObjId(0),
                    leaves: 0,
                    release_pending: false,
                    signal_pending: true,
                },
            );
        }
    });
    let before = m.sys.canonical().rings[1][0].user.produced;
    m.sys.node_mut(1).send_pending();
    assert_eq!(m.sys.check(), Ok(()));
    let written = m.sys.node(1).table.rings[1][0].user.produced - before;
    assert_eq!(written, dodos_abi::PENDING_STEP as u64);
}

// F13: observations are "at most 8 open per node" (Detection).

#[test]
fn f13_a_node_opens_at_most_eight_observations() {
    let cores: Vec<usize> = (1..10).collect();
    let mut m = S::on(&machine(crate::Arch::X86_64, 10, &[&[0], &cores]));
    let mut most = 0;
    for _ in 0..12 {
        m.run_by(100 * MS, 10 * MS);
        most = most.max(m.sys.canonical().beacons[0].observations.len());
    }
    assert_eq!(most, dodos_abi::OBSERVATIONS_MAX);
}

// F14: a user ring is served "at most 16 entries per ring per interrupt"
// (What crosses between nodes, processing).

#[test]
fn f14_one_interrupt_serves_at_most_sixteen_entries_of_a_user_ring() {
    let mut m = S::two();
    for _ in 0..17 {
        let body = crate::rings::Body::Signal {
            grant: 0,
            serial: 0,
        };
        let message = crate::rings::Message {
            header: dodos_abi::RingHeader::new(
                dodos_abi::NodeMessage::Signal,
                1,
                (1, m.sys.member(1).generation()),
                m.sys.member(0).generation(),
                None,
            ),
            body,
        };
        m.sys.inject(1, 0, message);
    }
    let before = m.sys.canonical().beacons[0].consumed[1].1;
    m.sys.fire(crate::system::Event::Ipi { node: 0, core: 0 });
    let after = m.sys.canonical().beacons[0].consumed[1].1;
    assert_eq!(after - before, dodos_abi::RING_USER_STEP);
}
