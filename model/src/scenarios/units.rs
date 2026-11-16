//! Devices and shared hardware, and Roles (D-129, D-131): unit requests
//! held by the requester until a holder answers done or *failed*, the
//! holder's unit service table, the epoch and *elsewhere*, the failed unit,
//! the planned hand-over in its five steps, the take-over after a death
//! with its rebuild by pulled reports, R1 at every change of holder, and
//! the plain and reserve roles.

use super::*;
use crate::Wait;
use crate::units::Stage;
use dodos_abi::{
    Evidence, FENCE_NS, IOMMU_WAIT_NS, NODE_STOP_KILL, NODE_STOP_SEAL, RoleState, SETTLE_NS,
};

/// The roles of the reference machine, by index: the IOMMU unit, the
/// IOAPIC, two reserves, the plain role `platform`.
const IOMMU_ROLE: usize = 0;
const IOAPIC_ROLE: usize = 1;
const RESERVE: usize = 2;
const PLATFORM: usize = 4;
const IOSPACE: u32 = 150;
const DEVICE: u64 = 0x10;

/// The slot of role `index` in a start-up space.
fn role_slot(m: &S, node: u16, index: usize) -> SlotAddr {
    let slots = slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Role);
    s(slots[index])
}

/// The slot of the IOMMU unit in a start-up space.
fn iommu_slot(m: &S, node: u16) -> SlotAddr {
    s(slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Iommu)[0])
}

fn attach(m: &S, node: u16, iospace: u32, device: u64) -> Syscall {
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

fn kill(n: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode: NODE_STOP_KILL,
        core: 0,
    }
}

/// Three nodes of one core each, all of fence class 2 but node 0.
fn three() -> S {
    S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]))
}

/// Node `node` makes an iospace and attaches the device through the
/// unit's holder; every message settles.
fn attached(m: &mut S, node: u16) {
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
    // Node 1 holds the units; node 0 asks; node 1 hands the IOMMU role to
    // node 2 meanwhile: the request carries the old epoch.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    let hand = Syscall::RoleHand {
        role: role_slot(&m, 1, IOMMU_ROLE),
        node: node_cap(2),
    };
    let outcome = m.call(1, hand);
    assert!(outcome == WAITS || matches!(outcome, Ok(Output::Epoch(_))));
    m.run(40 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_ne!(m.sys.role(IOMMU_ROLE).epoch(), epoch);
    // The request ended done, served by whichever holder had the role
    // with the epoch it carried.
    assert_eq!(m.woken(0), Some(&DONE));
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
    assert!(
        m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
}

#[test]
fn a_holder_answers_elsewhere_to_a_request_of_another_epoch_and_serves_one_of_its_epoch() {
    let mut k = N::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 0 holds the IOMMU role (Model::boot); node 1 asks with an old
    // epoch, on its request credit 0.
    let epoch = k.m.table.roles[IOMMU_ROLE].epoch();
    let op = crate::rings::UnitOp::Attach {
        device: DEVICE as u16,
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
    k.m.unit_requested(1, Some((0, 1)), (IOMMU_ROLE as u16, epoch + 7, op));
    assert_eq!(answer(&k), Some(dodos_abi::UnitResult::Elsewhere));
    assert!(
        k.m.table
            .units
            .get(&IOMMU_ROLE)
            .is_none_or(|u| u.entries.is_empty())
    );
    k.m.unit_requested(1, Some((1, 1)), (IOMMU_ROLE as u16, epoch, op));
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
    // A later request is answered failed at once.
    let again = attach(&m, 0, IOSPACE, DEVICE + 1);
    let outcome = m.call(0, again);
    if outcome == WAITS {
        m.run(30 * MS);
        assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    } else {
        assert_eq!(outcome, Err(Error::Busy));
    }
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

/// The operations submitted to the unit of a role so far.
fn submissions(m: &S, role: usize) -> u64 {
    m.sys.node(0).table.units.get(&role).map_or(0, |u| u.next)
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
    // Node 1 hangs without its handler on a class-2 core: it is fenced by
    // INIT, and dead once the barrier holds.
    m.sys.hang(1, true);
    m.ok(0, kill(1));
    m.settle();
    let plain = role_slot(&m, 0, PLATFORM);
    let reserve0 = role_slot(&m, 0, RESERVE);
    // Fencing: the plain role is claimed; the reserve role is busy.
    assert!(m.call(0, Syscall::RoleClaim(plain)).is_ok());
    if m.state(1) != MemberState::Dead {
        m.no(0, Syscall::RoleClaim(reserve0), Error::Busy);
    }
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
    // Not held by the caller's node.
    let theirs = role_slot(&m, 0, IOMMU_ROLE);
    m.no(
        0,
        Syscall::RoleHand {
            role: theirs,
            node: node_cap(0),
        },
        Error::Busy,
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
    // word is handing.
    if outcome == WAITS {
        let mut handing = false;
        for _ in 0..100 {
            m.run(100_000);
            let word = m.sys.role(IOMMU_ROLE);
            handing |= word.state() == RoleState::Handing;
            if word.state() == RoleState::Held && word.holder() == 2 {
                break;
            }
        }
        assert!(handing || m.sys.role(IOMMU_ROLE).holder() == 2);
    }
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
    let _ = m.call(
        1,
        Syscall::RoleHand {
            role: unit,
            node: node_cap(2),
        },
    );
    for _ in 0..200 {
        if m.sys.role(IOMMU_ROLE).state() == RoleState::Handing {
            break;
        }
        m.run(100_000);
    }
    assert_eq!(m.sys.role(IOMMU_ROLE).state(), RoleState::Handing);
    // The take-over is judged on the target, never on the node that hands.
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
    m.ok(0, kill(1));
    let mut first = None;
    for _ in 0..100 {
        m.run(100_000);
        let word = m.sys.role(IOMMU_ROLE);
        if word.holder() != 1 {
            first = Some(m.sys.now());
            break;
        }
    }
    assert!(first.is_some());
    // The successor is the next live node of the best class: node 2.
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 2);
}

#[test]
fn an_unfenced_holder_keeps_its_unit_roles_for_fence_ns_and_settle_ns() {
    let mut m = three();
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
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
