//! The scenarios of pass 35: decisions D-161 and D-162 brought into the
//! model, and the tests the audits of lot 8 found weak. Each test is named
//! after the sentence it guards.

use super::pass17::kill;
use super::units::{PLATFORM, role_slot};
use super::*;
use crate::rings::UnitOp;
use crate::table::RecordState;
use dodos_abi::{CoreState, CoreWord, FENCE_NS, MEMORY_TAKE_TABLES, SETTLE_NS};

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

// The derivation tree, the bounded sweep in two passes, across nested
// dying `cspace`s (D-157 (3), D-161 (6)): an inner `cspace` at slot 3 of an
// outer dying `cspace` holds a table untyped at its slot 5, which an
// `aspace` at slot 9 of the outer `cspace` names. The inner `cspace`'s
// pass 2 treats that `aspace` as outside it and returns `again`, and the
// outer sweep waits at slot 3 for the inner one: the deletion of the
// outer `cspace` stays `again`. The supervisor destroys the pinning object
// by deleting the ordinary untyped its header came from; the outer
// deletion then ends. (The audits of lot 8a, F7, and lot 8b, F3: no
// nested layout was built.)
#[test]
fn a_space_in_an_outer_dying_cspace_pins_a_table_untyped_of_an_inner_one() {
    use super::pass31::{BLOCK, INNER, TABLES, deleted, retype_at, via, with_blocks};
    const NESTED: u32 = 240;
    const DEEPER: u32 = 241;
    const DEEPER_TABLES: u32 = 242;
    const HEADERS: u32 = 250;
    let (mut k, _, _) = with_blocks();
    let page = k.m.geometry().page();
    let cspace = |k: &mut N, untyped: u32, slot: u32| {
        k.ok(retype_from(untyped, ObjectKind::CSpace, 128, slot));
        let grow = Syscall::CSpaceGrow {
            cspace: s(slot),
            untyped: s(untyped),
        };
        assert_eq!(k.call(k.t, grow), Ok(Output::Slots(128)));
    };
    cspace(&mut k, BLOCK, INNER);
    // The inner `cspace` one level deeper than the outer one, so that the
    // outer one may hold its root, and its table untyped as deep as it.
    k.ok(retype_from(BLOCK, ObjectKind::Untyped, 16 * page, DEEPER));
    cspace(&mut k, DEEPER, NESTED);
    k.ok(retype_at(
        s(TABLES),
        ObjectKind::Untyped,
        8 * page,
        s(DEEPER_TABLES),
    ));
    let nested = |index| SlotAddr::through(NESTED, index);
    let table = ObjectKind::Untyped;
    k.ok(retype_at(s(DEEPER_TABLES), table, 4 * page, nested(5)));
    // The pinning `aspace`: its header from its own ordinary untyped.
    k.ok(retype_from(BLOCK, ObjectKind::Untyped, 4 * page, HEADERS));
    k.ok(retype_at(
        s(HEADERS),
        ObjectKind::ASpace,
        nested(5).raw(),
        via(9),
    ));
    // The inner `cspace` moves to slot 3 of the outer one.
    let outer = k.m.object_at(k.t, s(INNER)).expect("the outer cspace");
    let inner = k.m.object_at(k.t, s(NESTED)).expect("the inner cspace");
    let moved = Syscall::CapMove {
        source: s(NESTED),
        destination: via(3),
    };
    k.ok(moved);
    for _ in 0..8 {
        k.no(Syscall::CapDelete(s(INNER)), Error::Again);
        k.interrupts();
    }
    // The outer sweep waits at slot 3; the inner one is in pass 2 at slot
    // 5, its table untyped pinned by the `aspace` at slot 9 of the outer.
    let sweep = |k: &N, space| {
        let sweep = k.m.cspaces[&space].sweep.as_ref().expect("dying");
        (sweep.next, sweep.pass2)
    };
    assert_eq!(sweep(&k, outer).0, 3);
    assert_eq!(sweep(&k, inner), (5, true));
    assert!(matches!(k.m.cspaces[&outer].slots[9], crate::Slot::Full(_)));
    // The way out D-161 (6) names: the header's untyped deleted first.
    assert_eq!(deleted(&mut k, HEADERS), DONE);
    k.interrupts();
    assert_eq!(deleted(&mut k, INNER), DONE);
    assert_eq!(k.m.check(), Ok(()));
}

// D-157 (2), gap 48: the start-up aspace's root and tables, drawn from the
// first table-floor untyped, stay permanent live pieces of it for the life
// of the node, even once that aspace is destroyed: the untyped's watermark
// never returns, and a new space takes its root above them. (The audits
// of lot 8a, F7, and lot 8b, F9: only the start-up state was asserted.)
#[test]
fn the_table_floor_untyped_keeps_the_start_up_aspaces_tables_after_its_destruction() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let tables = k.tables();
    let untyped = k.m.object_at(k.t, s(tables)).expect("the table untyped");
    let before = k.m.untypeds[&untyped].watermark;
    // A second thread, in an `aspace` whose root comes from a table block
    // of its own, destroys the start-up one: nothing else is made from the
    // table-floor untyped.
    let shift = k.m.geometry().chunk_shift();
    let chunk = super::pass31::free_chunk(&k.m, 0);
    let block = super::pass22::take(MEMORY_TAKE_TABLES, chunk << shift, shift.into(), 355);
    k.ok(block);
    k.ok(retype_from(k.ut, ObjectKind::ASpace, s(355).raw(), 360));
    assert_eq!(k.m.root_table(k.t, s(360)), Some(chunk << shift));
    k.place_first(0);
    let mut calls = thread_calls(k.ut, 0, 300, 0);
    if let Syscall::ThreadConfigure { aspace, .. } = &mut calls[3] {
        *aspace = s(360);
    }
    for call in calls {
        k.ok(call);
    }
    let other = k.m.object_at(k.t, s(300)).expect("the thread");
    let mut outcome = Err(Error::Again);
    for _ in 0..64 {
        let delete = Syscall::CapDelete(s(dodos_abi::START_SLOT_ASPACE));
        outcome = k.call(other, delete);
        if outcome != Err(Error::Again) {
            break;
        }
        k.interrupts();
    }
    assert_eq!(outcome, DONE);
    assert_eq!(k.m.object_at(k.t, s(dodos_abi::START_SLOT_ASPACE)), None);
    // The watermark stays where the start-up left it, and is not even
    // ready to return.
    assert_eq!(k.m.untypeds[&untyped].ready, None);
    assert_eq!(k.m.untypeds[&untyped].watermark, before);
    assert_eq!(k.m.check(), Ok(()));
}

// D-156 F5: `untyped_retype` of an `aspace` applies the barrier rule of a
// watermark that returns to the named table untyped too, before
// `no_memory`. On a node of three cores, an `aspace`'s root came from a
// child table untyped and the `aspace` is destroyed: that untyped's
// watermark is ready to return. The next `aspace` named on it, its header
// from an ordinary untyped with room, starts the barrier and is `again`
// with nothing made; once every other core answered, it is made, its root
// at the start of the table untyped. (The audits of lot 8a, F8, and lot
// 8b, F10: only ordinary untypeds met the barrier.)
#[test]
fn an_aspace_waits_for_the_barrier_of_its_table_untypeds_returning_watermark() {
    let mut k = N::on(&one_node(Arch::X86_64, 3));
    let page = k.m.geometry().page();
    let shift = k.m.geometry().chunk_shift();
    let chunk = super::pass31::free_chunk(&k.m, 0);
    let block = super::pass22::take(MEMORY_TAKE_TABLES, chunk << shift, shift.into(), 355);
    k.ok(block);
    let child = Syscall::UntypedRetype {
        untyped: s(355),
        kind: ObjectKind::Untyped,
        size: 4 * page,
        page: 0,
        memory: 0,
        destination: s(356),
    };
    k.ok(child);
    let aspace = retype_from(k.ut, ObjectKind::ASpace, s(356).raw(), 360);
    k.ok(aspace.clone());
    k.ok(Syscall::CapDelete(s(360)));
    let tables = k.m.object_at(k.t, s(356)).expect("the child");
    assert!(k.m.untypeds[&tables].ready.is_some());
    // Only the table untyped waits: the ordinary one holds other objects.
    let primary = k.m.object_at(k.t, s(k.ut)).expect("the untyped");
    assert_eq!(k.m.untypeds[&primary].ready, None);
    assert_eq!(k.call(k.t, aspace.clone()), Err(Error::Again));
    let request = k.m.cores[0].request.clone().expect("the barrier");
    assert!(matches!(
        request.completes,
        crate::cores::Completes::Barrier(_)
    ));
    assert_eq!(k.m.object_at(k.t, s(360)), None);
    k.interrupts();
    k.ok(aspace);
    assert_eq!(k.m.untypeds[&tables].ready, None);
    assert_eq!(k.m.root_table(k.t, s(360)), Some(chunk << shift));
}
