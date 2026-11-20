//! The scenarios of pass 35: decisions D-161 and D-162 brought into the
//! model, and the tests the audits of lot 8 found weak. Each test is named
//! after the sentence it guards.

use super::pass17::kill;
use super::units::{PLATFORM, role_slot};
use super::*;
use crate::rings::UnitOp;
use crate::table::RecordState;
use dodos_abi::{CoreState, CoreWord, FENCE_NS, SETTLE_NS};

// The calls, `node_start`, effect of mode 0 (D-161): the core words first,
// each from *free*, the member word last, from dead (g); on a failed
// compare-and-swap the call writes back the words it wrote.

/// Four nodes, node 1 on cores 1 and 2, killed and dead: its two cores
/// free for a fresh start.
fn node_1_dead_on_two_cores() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1, 2], &[3]]));
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(FENCE_NS + 10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    for core in [1, 2] {
        let word = CoreWord::from_raw(m.sys.core_word(core)).unwrap();
        assert_eq!(word.state(), CoreState::Free);
    }
    m
}

#[test]
fn node_start_writes_back_the_core_words_it_wrote_when_a_core_is_taken_by_another_start() {
    let mut m = node_1_dead_on_two_cores();
    let member = m.sys.member(1);
    // Another `node_start` of node 1 took core 2 between this kernel's
    // read of the words and its gate call.
    let other = CoreWord::joining(1, member.generation() + 1).raw();
    m.sys.edit_table(|t| t.core_words[2] = other);
    let model = m.sys.node_mut(0);
    assert_eq!(model.start_words(1, member, 2, &[1, 2]), Err(Error::Busy));
    // Core 1, written first, is free again; core 2 is the other call's;
    // the member word is as it was read.
    assert_eq!(m.sys.core_word(1), CoreWord::free(false).raw());
    assert_eq!(m.sys.core_word(2), other);
    assert_eq!(m.sys.member(1), member);
}

#[test]
fn node_start_writes_the_member_word_last_and_undoes_the_core_words_when_it_fails() {
    let mut m = node_1_dead_on_two_cores();
    let member = m.sys.member(1);
    // Another call already wrote the member word: the compare-and-swap
    // from dead (g), the last of the call, fails.
    let moved = dodos_abi::Member::new(
        MemberState::Starting,
        dodos_abi::Evidence::None,
        dodos_abi::DRIVER_NONE,
        member.generation() + 1,
    );
    m.sys.edit_table(|t| t.members[1] = moved);
    let model = m.sys.node_mut(0);
    assert_eq!(model.start_words(1, member, 2, &[1, 2]), Err(Error::Busy));
    for core in [1, 2] {
        assert_eq!(m.sys.core_word(core), CoreWord::free(false).raw());
    }
    assert_eq!(m.sys.member(1), moved);
}

// Roles, `role_claim` (D-161): a plain role is `busy` while its holder is
// fencing, and moves from an unfenced holder only once `FENCE_NS` +
// `SETTLE_NS` have passed since the first fence action.

#[test]
fn a_plain_role_is_busy_while_its_holder_fences_and_unfenced_until_fence_and_settle_have_passed() {
    let mut m = S::two();
    let platform = role_slot(&m, 1, PLATFORM);
    assert!(m.call(1, Syscall::RoleClaim(platform)).is_ok());
    // Node 1 hangs in VMX root operation without its handler: the fence
    // cannot confirm it, it is unfenced after `FENCE_NS`.
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.supervisor(0, m.t[0]);
    let begun = m.sys.now();
    m.ok(0, kill(1));
    let claim = Syscall::RoleClaim(role_slot(&m, 0, PLATFORM));
    assert_eq!(m.state(1), MemberState::Fencing);
    m.no(0, claim.clone(), Error::Busy);
    m.run(FENCE_NS + SETTLE_NS / 2);
    assert_eq!(m.state(1), MemberState::Unfenced);
    assert!(m.sys.now() < begun + FENCE_NS + SETTLE_NS);
    m.no(0, claim.clone(), Error::Busy);
    m.run(SETTLE_NS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    let epoch = m.sys.role(PLATFORM).epoch();
    assert_eq!(m.call(0, claim), Ok(Output::Epoch(epoch + 1)));
    assert_eq!(m.sys.role(PLATFORM).holder(), 0);
}

// What crosses between nodes, row 4 (D-162): a `UnitRequest` whose role
// differs from the role in word 0 of its request record is answered
// *refused* by the holder's gate.

/// The holder of `role` receives a `UnitRequest` of `requester` naming
/// `role` with its current epoch, for a record whose word 0 names
/// `recorded`; returns the state the holder's gate left the record in.
fn requested_under_record_role(
    m: &mut S,
    requester: u16,
    (role, recorded): (usize, usize),
    op: UnitOp,
) -> RecordState {
    let holder = m.sys.role(role).holder();
    let epoch = m.sys.role(role).epoch();
    let model = m.sys.node_mut(holder);
    let entry = 0;
    let serial = model.table.open_unit_record(requester, entry, recorded, op);
    model.unit_requested(requester, Some((0, 1)), (role as u16, epoch), (0, serial));
    assert_eq!(m.sys.check(), Ok(()));
    m.sys.node(holder).table.unit_records[usize::from(requester)][entry].state
}

#[test]
fn a_unit_request_whose_role_is_not_its_records_is_refused() {
    use super::units::DEVICE;
    let (mut m, root) = super::pass21::attached_on_the_first_of_three_units();
    // Node 0 detaches its own device through the first unit, where it is
    // attached: accepted when the roles agree.
    let (role, other) = (0, 1);
    let holder = m.sys.role(role).holder();
    let unit = &m.sys.node(holder).table.units[&role];
    let detach = UnitOp::Detach {
        device: DEVICE as u16,
        domain: unit.domains[&(DEVICE as u16)],
        root,
    };
    let refused = m.sys.node(holder).refused.0;
    let unit = m.sys.node(holder).table.units.get(&role).cloned();
    let state = requested_under_record_role(&mut m, 0, (role, other), detach);
    assert_eq!(state, RecordState::Refused);
    assert_eq!(m.sys.node(holder).refused.0, refused + 1);
    assert_eq!(m.sys.node(holder).table.units.get(&role).cloned(), unit);
    let state = requested_under_record_role(&mut m, 0, (role, role), detach);
    assert_ne!(state, RecordState::Refused);
    assert_eq!(m.sys.node(holder).refused.0, refused + 1);
}
