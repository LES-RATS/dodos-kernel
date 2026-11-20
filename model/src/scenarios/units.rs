//! Devices and shared hardware, and Roles (D-129, D-131): unit requests
//! held by the requester until a holder answers done or *failed*, the
//! holder's unit service table, the epoch and *elsewhere*, the failed unit,
//! the planned hand-over in its five steps, the take-over after a death
//! with its rebuild by pulled reports, R1 at every change of holder, and
//! the plain and reserve roles.

use super::pass17::{kill, submissions};
use super::*;
use crate::Wait;
use crate::units::Stage;
use dodos_abi::{Evidence, FENCE_NS, IOMMU_WAIT_NS, NODE_STOP_SEAL, RoleState, SETTLE_NS};

/// The roles of the reference machine, by index: the IOMMU unit, the
/// IOAPIC, two reserves, the plain role `platform`.
pub(super) const IOMMU_ROLE: usize = 0;
pub(super) const IOAPIC_ROLE: usize = 1;
pub(super) const RESERVE: usize = 2;
pub(super) const PLATFORM: usize = 4;
pub(super) const IOSPACE: u32 = 150;
pub(super) const DEVICE: u64 = 0x10;

/// The slot of role `index` in a start-up space.
pub(super) fn role_slot(m: &S, node: u16, index: usize) -> SlotAddr {
    let slots = slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Role);
    s(slots[index])
}

/// The slot of the IOMMU unit in a start-up space.
fn iommu_slot(m: &S, node: u16) -> SlotAddr {
    s(slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Iommu)[0])
}

pub(super) fn attach(m: &S, node: u16, iospace: u32, device: u64) -> Syscall {
    Syscall::IoSpaceAttach {
        iommu: iommu_slot(m, node),
        iospace: if iospace == 0 {
            SlotAddr::NONE
        } else {
            s(iospace)
        },
        device,
    }
}

/// Three nodes of one core each, all of fence class 2 but node 0.
pub(super) fn three() -> S {
    S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]))
}

/// Node `node` makes an iospace and attaches the device through the
/// unit's holder; every message settles.
pub(super) fn attached(m: &mut S, node: u16) {
    let iospace = m.retype(node, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(node, iospace);
    let call = attach(m, node, IOSPACE, DEVICE);
    let outcome = m.call(node, call);
    if outcome == WAITS {
        m.run(30 * MS);
        assert_eq!(m.woken(node), Some(&DONE));
    } else {
        assert_eq!(outcome, DONE);
    }
}

// Unit requests.

#[test]
fn an_attach_on_a_node_without_the_unit_role_is_a_request_to_its_holder() {
    let mut m = S::two();
    // Node 1 holds the unit roles (class 2); node 0 attaches.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    assert!(matches!(m.sys.node(0).threads[&m.t[0]].wait, Wait::Unit(_)));
    let entry = m.sys.node(0).unit_requests.iter().flatten().next().cloned();
    let entry = entry.expect("an entry of the unit request table");
    assert_eq!(entry.role, IOMMU_ROLE);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    // The entry ended at the answer; the unit's context entry names the
    // iospace of node 0.
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
    let unit = &m.sys.canonical().units[&IOMMU_ROLE];
    assert!(unit.entries.contains_key(&(DEVICE as u16)));
}

#[test]
fn a_node_that_holds_the_role_serves_its_own_request_with_no_message() {
    let mut m = S::two();
    attached(&mut m, 1);
    assert!(!m.sys.node(1).rings_waiting());
    assert!(
        m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
}

#[test]
fn a_request_answered_elsewhere_is_sent_again_with_the_epoch_of_the_new_holder() {
    let mut m = three();
    // Node 1 holds the units and begins to hand the IOMMU role to node 2;
    // node 0 asks meanwhile, with the epoch it reads: node 1 answers
    // *elsewhere*, and the request goes again once the word reads held by
    // node 2 with a new epoch.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let hand = Syscall::RoleHand {
        role: role_slot(&m, 1, IOMMU_ROLE),
        node: node_cap(2),
    };
    assert_eq!(m.call(1, hand), WAITS);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    let entry = |m: &S| m.sys.node(0).unit_requests.iter().flatten().next().cloned();
    assert_eq!(
        entry(&m).map(|e| (e.epoch, e.elsewhere)),
        Some((epoch, false))
    );
    // Answered *elsewhere* by node 1, which hands the role (the audit of
    // lot 8a, D-8a F07: the request was served before the hand-over, and
    // no epoch was read).
    assert!(m.step_until(|m| entry(m).is_some_and(|e| e.elsewhere)));
    assert_eq!(entry(&m).map(|e| e.epoch), Some(epoch));
    // Sent again with the epoch of the new holder.
    assert!(m.step_until(|m| entry(m).is_none_or(|e| !e.elsewhere)));
    let sent = entry(&m).expect("the request still waits");
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_eq!(sent.epoch, m.sys.role(IOMMU_ROLE).epoch());
    assert_ne!(sent.epoch, epoch);
    m.run(40 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_ne!(m.sys.role(IOMMU_ROLE).epoch(), epoch);
    assert_eq!(m.woken(0), Some(&DONE));
}

#[test]
fn a_holder_answers_elsewhere_to_a_request_of_another_epoch_and_serves_one_of_its_epoch() {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 0 holds the IOMMU role (Model::boot); node 1 asks with an old
    // epoch, on its request credit 0.
    let epoch = k.m.table.roles[IOMMU_ROLE].epoch();
    // Node 1 holds the device's identifier word, and asks with a domain
    // number of its own range (the holder's gate checks both, D-134).
    let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(1), k.m.table.generation(1));
    k.m.table.identifiers.insert(DEVICE as u32, taken.raw());
    // The root names an iospace root in node 1's floor, to which node 1's
    // gate pinned the device (G9, the attach word, D-137).
    let root = k.m.table.floors[1][0].0;
    let page = root / k.m.table.geometry.page();
    let pinned = dodos_abi::AttachWord::new(dodos_abi::AttachState::Pinned, root);
    k.m.table
        .attach
        .insert(DEVICE as u32, pinned.expect("a root").raw());
    std::sync::Arc::make_mut(&mut k.m.table.classes)
        .insert(page, dodos_abi::PageClass::iospace_root(1).expect("1"));
    let op = crate::rings::UnitOp::Attach {
        device: DEVICE as u16,
        domain: *k.m.domain_range(0, 1).start(),
        root,
    };
    let answer = |k: &N| {
        let ring = &k.m.table.rings[0][1].kernel;
        (0..ring.produced).rev().find_map(|at| match ring.at(at) {
            Some(crate::table::RingEntry::Message(m)) => match m.body {
                crate::rings::Body::UnitAnswer { result } => Some(result),
                _ => None,
            },
            _ => None,
        })
    };
    k.m.unit_requested_with_record(1, Some((0, 1)), (IOMMU_ROLE as u16, epoch + 7, op));
    assert_eq!(answer(&k), Some(dodos_abi::UnitResult::Elsewhere));
    assert!(
        k.m.table
            .units
            .get(&IOMMU_ROLE)
            .is_none_or(|u| u.entries.is_empty())
    );
    k.m.unit_requested_with_record(1, Some((1, 1)), (IOMMU_ROLE as u16, epoch, op));
    // Served: submitted to the unit, answered done at its confirmation (the
    // final review's E35).
    assert!(k.m.unit_service.contains_key(&(1, 1)));
    k.m.unit_step();
    assert_eq!(answer(&k), Some(dodos_abi::UnitResult::Done));
    assert!(
        k.m.table.units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
}

#[test]
fn the_requester_keeps_its_request_through_the_death_of_the_holder_and_the_next_holder_serves_it() {
    let mut m = three();
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    // The holder hangs: the request is sent and never answered.
    m.sys.hang(1, true);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(MS);
    let sent = m
        .sys
        .node(0)
        .unit_requests
        .iter()
        .flatten()
        .any(|e| matches!(e.stage, Stage::Sent { .. }));
    assert!(sent);
    m.supervisor(2, m.t[2]);
    m.ok(2, kill(1));
    m.run(40 * MS);
    // R0 queued it again; the successor took the role over and served it.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_eq!(m.woken(0), Some(&DONE));
}

#[test]
fn a_unit_that_does_not_confirm_within_iommu_wait_ns_has_failed() {
    let mut m = S::two();
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    m.sys.break_unit(IOMMU_ROLE);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(IOMMU_WAIT_NS + 30 * MS);
    // The waiting call ends with `busy`; the holder's gate wrote the
    // failed bit, kept until the machine restarts; the unit's failure is
    // not a failure of the node.
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    assert!(m.sys.role(IOMMU_ROLE).failed());
    assert_eq!(m.state(1), MemberState::Live);
    // A later request is answered failed at once: the role word's failed
    // bit ends the call before anything is sent (the final review's E35: a
    // wait of 30 ms was allowed).
    let again = attach(&m, 0, IOSPACE, DEVICE + 1);
    assert_eq!(m.call(0, again), Err(Error::Busy));
}

/// Three nodes of one core each on a machine with two IOMMU units on PCI
/// segment 0: the unit roles go to nodes 1 and 2, so node 0's requests
/// travel to both.
fn two_units() -> S {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    let first = machine.iommus[0];
    machine.iommus.push(first);
    S::on(&machine)
}

/// The claim of an MSI of segment 0 on node 0's core.
fn claim_msi() -> Syscall {
    Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    }
}

#[test]
fn the_claim_of_an_msi_asks_each_remapping_unit_of_its_segment_one_after_another() {
    let mut m = two_units();
    let roles = m.sys.node(0).segment_roles(0);
    assert_eq!(roles.len(), 2);
    let before = (submissions(&m, roles[0]), submissions(&m, roles[1]));
    assert_eq!(m.call(0, claim_msi()), WAITS);
    // One wait and one entry of the unit request table, for the first unit
    // only: the second is not asked before the first has answered.
    let entries: Vec<usize> = m
        .sys
        .node(0)
        .unit_requests
        .iter()
        .flatten()
        .map(|e| e.role)
        .collect();
    assert_eq!(entries, vec![roles[0]]);
    assert_eq!(submissions(&m, roles[1]), before.1);
    m.run(30 * MS);
    // Both units invalidated their interrupt entry cache, and the call
    // ends with its message words once the last has answered.
    assert!(matches!(m.woken(0), Some(Ok(crate::Output::Msi { .. }))));
    assert_eq!(submissions(&m, roles[0]), before.0 + 1);
    assert_eq!(submissions(&m, roles[1]), before.1 + 1);
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
}

#[test]
fn a_claim_whose_second_remapping_unit_has_failed_ends_busy_and_the_first_keeps_its_invalidation() {
    let mut m = two_units();
    let roles = m.sys.node(0).segment_roles(0);
    let before = submissions(&m, roles[0]);
    m.sys.break_unit(roles[1]);
    assert_eq!(m.call(0, claim_msi()), WAITS);
    m.run(IOMMU_WAIT_NS + 30 * MS);
    // The second unit did not confirm: it has failed, and the call ends
    // with `busy`; the first unit's invalidation stays done, and its unit
    // has not failed.
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    assert!(m.sys.role(roles[1]).failed());
    assert!(!m.sys.role(roles[0]).failed());
    assert_eq!(submissions(&m, roles[0]), before + 1);
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
}

// Roles: plain and reserve.

#[test]
fn role_claim_takes_a_vacant_plain_or_reserve_role_and_refuses_a_unit_role() {
    let mut m = S::two();
    let unit = role_slot(&m, 0, IOMMU_ROLE);
    m.no(0, Syscall::RoleClaim(unit), Error::Invalid);
    let platform = role_slot(&m, 0, PLATFORM);
    let held = m.call(0, Syscall::RoleClaim(platform));
    assert!(matches!(held, Ok(Output::Epoch(_))), "{held:?}");
    assert_eq!(m.sys.role(PLATFORM).holder(), 0);
    // Claimed again by its holder: success with no change.
    let epoch = m.sys.role(PLATFORM).epoch();
    assert!(m.call(0, Syscall::RoleClaim(platform)).is_ok());
    assert_eq!(m.sys.role(PLATFORM).epoch(), epoch);
    // Held by a live node: busy for the other.
    let other = role_slot(&m, 1, PLATFORM);
    m.no(1, Syscall::RoleClaim(other), Error::Busy);
    let reserve = role_slot(&m, 1, RESERVE);
    assert!(m.call(1, Syscall::RoleClaim(reserve)).is_ok());
}

#[test]
fn role_hand_moves_a_plain_role_or_vacates_it() {
    let mut m = S::two();
    let platform = role_slot(&m, 0, PLATFORM);
    assert!(m.call(0, Syscall::RoleClaim(platform)).is_ok());
    let hand = |node| Syscall::RoleHand {
        role: platform,
        node,
    };
    assert!(m.call(0, hand(node_cap(1))).is_ok());
    assert_eq!(m.sys.role(PLATFORM).holder(), 1);
    // Not held by the caller's node: busy.
    m.no(0, hand(node_cap(1)), Error::Busy);
    let back = Syscall::RoleHand {
        role: role_slot(&m, 1, PLATFORM),
        node: SlotAddr::NONE,
    };
    assert!(m.call(1, back).is_ok());
    assert_eq!(m.sys.role(PLATFORM).state(), RoleState::Vacant);
}

#[test]
fn a_plain_role_of_a_fenced_holder_can_be_claimed_a_reserve_role_only_once_it_is_dead() {
    let mut m = three();
    let platform = role_slot(&m, 1, PLATFORM);
    let reserve = role_slot(&m, 1, RESERVE);
    assert!(m.call(1, Syscall::RoleClaim(platform)).is_ok());
    assert!(m.call(1, Syscall::RoleClaim(reserve)).is_ok());
    // Node 1 hangs with its handler on a class-2 core: the fence's NMI parks
    // it, and it is dead once the barrier holds (the final review's M2: the
    // comment described a hang without the handler).
    m.sys.hang(1, true);
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    let plain = role_slot(&m, 0, PLATFORM);
    let reserve0 = role_slot(&m, 0, RESERVE);
    // Fencing: both are busy (D-161: fencing for a plain role too).
    assert_eq!(m.state(1), MemberState::Fencing);
    m.no(0, Syscall::RoleClaim(plain), Error::Busy);
    m.no(0, Syscall::RoleClaim(reserve0), Error::Busy);
    // Fenced, not yet dead: the plain role is claimed; the reserve role is
    // busy (the audit's F11: this was asserted only if the node happened
    // not to be dead yet).
    assert!(m.step_until(|m| m.state(1) == MemberState::Fenced));
    assert!(m.call(0, Syscall::RoleClaim(plain)).is_ok());
    m.no(0, Syscall::RoleClaim(reserve0), Error::Busy);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert!(m.call(0, Syscall::RoleClaim(reserve0)).is_ok());
}

// Unit roles: the planned hand-over.

#[test]
fn role_hand_of_a_unit_role_is_judged_in_order() {
    let mut m = S::two();
    let unit = role_slot(&m, 1, IOMMU_ROLE);
    // Node 0 as target is invalid for a unit role.
    m.no(
        1,
        Syscall::RoleHand {
            role: unit,
            node: SlotAddr::NONE,
        },
        Error::Invalid,
    );
    // Not held by the caller's node, its target the caller's node: the
    // first state check, `busy`, before the success with no change.
    let theirs = role_slot(&m, 0, IOMMU_ROLE);
    m.no(
        0,
        Syscall::RoleHand {
            role: theirs,
            node: node_cap(0),
        },
        Error::Busy,
    );
    // Node 0 and not held by the caller's node: the plain check first (the
    // audit of lot 8a, F10: no case had two faults).
    m.no(
        0,
        Syscall::RoleHand {
            role: theirs,
            node: SlotAddr::NONE,
        },
        Error::Invalid,
    );
    // The target is the caller's node: success with no change.
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    assert!(
        m.call(
            1,
            Syscall::RoleHand {
                role: unit,
                node: node_cap(1),
            }
        )
        .is_ok()
    );
    assert_eq!(m.sys.role(IOMMU_ROLE).epoch(), epoch);
    // A target of a lesser fence class: busy.
    m.no(
        1,
        Syscall::RoleHand {
            role: unit,
            node: node_cap(0),
        },
        Error::Busy,
    );
}

#[test]
fn a_unit_role_is_not_handed_to_a_node_with_a_core_in_vmx_root_operation() {
    let mut m = three();
    m.sys.set_vmx(2, true);
    let unit = role_slot(&m, 1, IOMMU_ROLE);
    m.no(
        1,
        Syscall::RoleHand {
            role: unit,
            node: node_cap(2),
        },
        Error::Busy,
    );
}

#[test]
fn a_unit_role_is_handed_in_steps_handing_then_held_by_the_target_two_epochs_later() {
    let mut m = three();
    let unit = role_slot(&m, 1, IOMMU_ROLE);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let outcome = m.call(
        1,
        Syscall::RoleHand {
            role: unit,
            node: node_cap(2),
        },
    );
    // The caller waits while the queue drains; the call returns once the
    // word is handing (step 4), which the target turns into held (step 5).
    // Seen event by event: sampled every 100 µs, handing was never seen
    // (the final review's E33).
    // (The audit of lot 8a, D-8a F36: `DONE` was accepted too.)
    assert_eq!(outcome, WAITS);
    assert!(m.step_until(|m| m.sys.role(IOMMU_ROLE).state() == RoleState::Handing));
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!(
        (word.holder(), word.target(), word.epoch()),
        (1, 2, epoch + 1)
    );
    m.run(30 * MS);
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!((word.state(), word.holder()), (RoleState::Held, 2));
    assert_eq!(word.epoch(), epoch + 2);
}

#[test]
fn a_handing_word_whose_target_dies_is_taken_over_by_the_targets_successor() {
    let mut m = three();
    let unit = role_slot(&m, 1, IOMMU_ROLE);
    // The target hangs: it never completes step 5.
    m.sys.hang(2, true);
    // The caller waits while the queue drains (the audit of lot 8a, D-8a
    // F18: the outcome was dropped).
    let outcome = m.call(
        1,
        Syscall::RoleHand {
            role: unit,
            node: node_cap(2),
        },
    );
    assert_eq!(outcome, WAITS);
    for _ in 0..200 {
        if m.sys.role(IOMMU_ROLE).state() == RoleState::Handing {
            break;
        }
        m.run(100_000);
    }
    assert_eq!(m.sys.role(IOMMU_ROLE).state(), RoleState::Handing);
    // The take-over is judged on the target, never on the node that hands.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(2));
    m.run(FENCE_NS + 10 * SETTLE_NS);
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!(word.state(), RoleState::Held);
    assert_ne!(word.holder(), 2);
}

// Unit roles: the take-over after a death.

#[test]
fn a_unit_role_is_taken_over_settle_ns_after_the_stop_is_confirmed() {
    let mut m = three();
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    // Event by event, so that the instant the holder changes is exact: the
    // stop confirmed, then `SETTLE_NS` (the final review's E32: only
    // `is_some` was asserted, and the delay was held by no test).
    assert!(m.step_until(|m| m.sys.role(IOMMU_ROLE).holder() != 1));
    let confirmed = m.sys.canonical().fences[1]
        .confirmed
        .expect("the stop confirmed");
    assert_eq!(m.sys.now(), confirmed + SETTLE_NS);
    // The successor is the next live node of the best class: node 2.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 2);
}

#[test]
fn an_unfenced_holder_keeps_its_unit_roles_for_fence_ns_and_settle_ns() {
    let mut m = three();
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(FENCE_NS - MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    m.run(2 * MS + 2 * SETTLE_NS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
}

#[test]
fn the_rebuild_of_a_unit_keeps_the_entries_of_live_nodes_and_clears_those_of_no_report() {
    let mut m = three();
    attached(&mut m, 0);
    // The holder dies; node 2 takes over and rebuilds from the reports.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(
        m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
    assert!(m.sys.node(2).rebuilds.is_empty());
}

#[test]
fn the_rebuild_keeps_without_a_report_the_entries_of_a_sealed_node() {
    let mut m = three();
    attached(&mut m, 0);
    m.ok(
        2,
        Syscall::NodeStop {
            node: node_cap(0),
            mode: NODE_STOP_SEAL,
            core: 0,
        },
    );
    m.run(5 * MS);
    assert_eq!(m.state(0), MemberState::Sealed);
    m.supervisor(2, m.t[2]);
    m.ok(2, kill(1));
    m.run(30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(
        m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
}

#[test]
fn a_node_that_dies_with_an_attached_device_leaves_no_context_entry_once_dead() {
    let mut m = three();
    attached(&mut m, 0);
    m.supervisor(2, m.t[2]);
    m.ok(2, kill(0));
    m.run(30 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
}

#[test]
fn a_sealed_node_holds_no_unit_role() {
    let mut m = three();
    m.ok(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: NODE_STOP_SEAL,
            core: 0,
        },
    );
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    for role in [IOMMU_ROLE, IOAPIC_ROLE] {
        assert_ne!(m.sys.role(role).holder(), 1);
    }
    assert_eq!(m.sys.member(1).evidence(), Evidence::None);
}
