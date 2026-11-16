//! Argon section 2.3 of the seventh review: the parts of its test
//! `r7_system_unit_survivor_tests` that pin rules in the scope of pass 14
//! (the checker's invariants 52, 56, 57 and 88; the recovery and roles
//! inside one node: the redrive of a fence, the barrier while a unit role
//! is handing, a reserve taken, the hand-over of a sealed holder's unit,
//! the observation of sealed nodes). Its assertion that was red on clean
//! code (a two-node machine asked for observations of nodes 2 and 3) runs
//! on four nodes here. The parts that pin the protocols between nodes
//! being redesigned (misdirected messages, moves, transfers, pinned grants,
//! stale remote capabilities, nodes co-hosted on one core) are not taken.

use super::*;
use crate::remote::Request;
use crate::table::Word;
use crate::{Model, Output};
use dodos_abi::*;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

/// The example machine with `n` cores, the times of membership short.
fn machine(n: usize) -> Machine {
    let mut machine = Machine::example();
    while machine.cores.len() < n {
        let mut extra = machine.cores[1];
        extra.hw_id = 2 * machine.cores.len() as u64;
        machine.cores.push(extra);
    }
    machine.membership.stale_ms = 10;
    machine.membership.stuck_ms = 10;
    machine.membership.sweep_ms = 100;
    machine
}

/// Changes the machine table of every node the same way.
fn tables(sys: &mut System, change: impl Fn(&mut crate::table::Table)) {
    for node in sys.nodes.iter_mut().flatten() {
        change(&mut node.table);
    }
}

/// #048, #050, #089: the identifier word is keyed by segment << 16, and
/// invariant 88 refuses a held word with no attached device, and the
/// reverse.
#[test]
fn r7_identifier_words_are_keyed_by_segment_and_judged_by_88() {
    let mut machine = Machine::example();
    machine.iommus[0].segment = 1;
    let (mut m, t) = Model::boot(&machine).unwrap();
    let iospace = Syscall::UntypedRetype {
        untyped: s(25),
        kind: ObjectKind::IoSpace,
        size: 0,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(100),
    };
    assert_eq!(m.apply(t, iospace), Ok(Output::Done));
    let attach = Syscall::IoSpaceAttach {
        iommu: s(18),
        iospace: s(100),
        device: 0x0100,
    };
    assert_eq!(m.apply(t, attach), Ok(Output::Done));
    assert_eq!(m.check(), Ok(()));
    assert!(m.table.identifiers.contains_key(&0x0001_0100));
    let taken = Ownership::taken(Owner::Node(0), m.table.generation(0)).raw();
    m.table.identifiers.insert(0x0001_0200, taken);
    assert_eq!(m.check(), Err("88: identifier words and attached devices"));
    m.table.identifiers.remove(&0x0001_0200);
    m.table.identifiers.remove(&0x0001_0100);
    assert_eq!(m.check(), Err("88: identifier words and attached devices"));
}

/// #051 to #054: R6 frees a reserve word taken by the holder its role
/// names, fenced or dead, with that generation; any other release of a
/// reserve word by a node that does not hold its role is refused (52).
#[test]
fn r7_r6_releases_of_reserve_words_are_judged_by_52() {
    let (m, _) = Model::boot(&Machine::example()).unwrap();
    let reserve = Owner::Reserve(0);
    let role = m
        .table
        .role_records
        .iter()
        .position(|r| r.kind == RoleKind::Reserve && r.object == 0)
        .unwrap();
    let release = |members: Member, held: bool, before: Ownership, after: Ownership| {
        let mut m = m.clone();
        m.table.members[1] = members;
        if held {
            m.table.roles[role] = RoleWord::held(1, 1);
        }
        let write = m
            .table
            .journal(0, Word::Chunk(0), before.raw(), after.raw());
        m.table.writes.push(write);
        m.check()
    };
    let dead = Member::new(MemberState::Dead, Evidence::Stale, 0, 1);
    let taken = Ownership::taken(reserve, 1);
    let free = Ownership::free(reserve);
    assert_eq!(release(dead, true, taken, free), Ok(()));
    let broken = Err("52: write of an ownership word");
    // #051: the word was not taken.
    assert_eq!(release(dead, true, free, free), broken);
    // #052: freed with another reserve's code.
    let other = Ownership::free(Owner::Reserve(1));
    assert_eq!(release(dead, true, taken, other), broken);
    // #053: no role names a holder of the reserve.
    assert_eq!(release(dead, false, taken, free), broken);
    // #054: the holder's generation is not the word's.
    let later = Member::new(MemberState::Dead, Evidence::Stale, 0, 2);
    assert_eq!(release(later, true, taken, free), broken);
}

/// #056, #057, #058: invariants 56 and 57.
#[test]
fn r7_invariants_56_and_57_refuse_a_word_changed_from_none_and_a_kernel_run_not_held() {
    let (m, _) = Model::boot(&Machine::example()).unwrap();
    let none_chunk = *m.table.none.iter().next().unwrap();
    let free_chunk = (0..m.table.owners.len() as u64)
        .find(|c| !m.table.none.contains(c) && !m.kernel_runs.contains(c))
        .unwrap();
    let mut m56 = m.clone();
    let owners = std::sync::Arc::make_mut(&mut m56.table.owners);
    owners[free_chunk as usize] = Ownership::taken(Owner::Node(0), 1).raw();
    m56.kernel_runs.insert(none_chunk);
    assert_eq!(m56.check(), Err("57: kernel space and ownership"));
    let mut m57 = m.clone();
    std::sync::Arc::make_mut(&mut m57.table.owners)[free_chunk as usize] = 0;
    assert_eq!(m57.check(), Err("56: a word changed from or to none"));
    let mut m58 = m.clone();
    let owners = std::sync::Arc::make_mut(&mut m58.table.owners);
    owners[none_chunk as usize] = Ownership::taken(Owner::Node(1), 1).raw();
    owners[free_chunk as usize] = 0;
    assert_eq!(m58.check(), Err("56: a word changed from or to none"));
}

/// #108: a fence redriven by a new driver keeps an unfenced node unfenced.
#[test]
fn r7_a_redriven_fence_keeps_an_unfenced_node_unfenced() {
    let (mut sys, _) = System::boot(&machine(3)).unwrap();
    let unfenced = Member::new(MemberState::Unfenced, Evidence::Stale, 1, 1);
    let fencing = Member::new(MemberState::Fencing, Evidence::Kill, 0, 1);
    tables(&mut sys, |t| {
        t.members[2] = unfenced;
        t.members[1] = fencing;
    });
    sys.nodes[0].as_mut().unwrap().recovery_step();
    let member = sys.nodes[0].as_ref().unwrap().table.member(2);
    assert_eq!(
        (member.state(), member.driver()),
        (MemberState::Unfenced, 0)
    );
}

/// #121: the barrier of a fenced node does not hold while a unit role is
/// handing.
#[test]
fn r7_the_barrier_waits_while_a_unit_role_is_handing() {
    let (mut sys, _) = System::boot(&machine(3)).unwrap();
    let fenced = Member::new(MemberState::Fenced, Evidence::Kill, 0, 1);
    tables(&mut sys, |t| {
        t.members[1] = fenced;
        t.beacons[0].acked[1] = 1;
        t.beacons[2].acked[1] = 1;
        t.beacons[0].maps_of[1] = false;
        t.beacons[2].maps_of[1] = false;
        t.roles[0] = RoleWord::handing(0, 2, 1);
    });
    for _ in 0..8 {
        sys.nodes[0].as_mut().unwrap().recovery_step();
    }
    let member = sys.nodes[0].as_ref().unwrap().table.member(1);
    assert_eq!(member.state(), MemberState::Fenced);
}

/// #152, #153: a reserve role cannot be claimed while a word of its
/// reserve is taken by the holder the role names, with the generation it
/// holds; once that generation is over, it can.
#[test]
fn r7_a_reserve_role_is_busy_while_its_reserve_is_taken_by_its_holder() {
    let (mut sys, t) = System::boot(&Machine::example()).unwrap();
    let role = sys
        .node(0)
        .table
        .role_records
        .iter()
        .position(|r| r.kind == RoleKind::Reserve)
        .unwrap();
    let reserve = sys.node(0).table.role_records[role].object;
    let shift = sys.node(0).table.geometry.chunk_shift();
    let chunk = sys.node(0).table.reserves[reserve as usize][0].0 >> shift;
    let slot = s(19 + role as u32);
    let taken = Ownership::taken(Owner::Reserve(reserve), 1).raw();
    tables(&mut sys, |t| {
        std::sync::Arc::make_mut(&mut t.owners)[chunk as usize] = taken;
        t.roles[role] = RoleWord::held(0, 2);
    });
    assert_eq!(
        sys.apply(1, t[1], Syscall::RoleClaim(slot)),
        Err(Error::Busy)
    );
    // The holder hands the role on only when no word of the reserve is
    // taken; a word taken by a generation of the holder that is over is not
    // (#153; a state only a test builds: the barrier vacates the reserve
    // roles of a node that dies).
    let hand = Syscall::RoleHand {
        role: slot,
        core: core(1),
    };
    assert_eq!(sys.apply(0, t[0], hand.clone()), Err(Error::Busy));
    let later = Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, 5);
    tables(&mut sys, |t| t.members[0] = later);
    assert!(matches!(sys.apply(0, t[0], hand), Ok(Output::Epoch(_))));
    tables(&mut sys, |t| {
        t.roles[role] = RoleWord::held(0, 4);
        t.members[0] = Member::new(MemberState::Dead, Evidence::Kill, 1, 5);
    });
    assert!(matches!(
        sys.apply(1, t[1], Syscall::RoleClaim(slot)),
        Ok(Output::Epoch(_))
    ));
}

/// #156: the node that takes a unit role keeps the context entries of a
/// sealed node.
#[test]
fn r7_a_unit_taken_over_keeps_the_entries_of_a_sealed_node() {
    let (mut sys, t) = System::boot(&machine(3)).unwrap();
    let iospace = Syscall::UntypedRetype {
        untyped: s(26),
        kind: ObjectKind::IoSpace,
        size: 0,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(100),
    };
    assert_eq!(sys.apply(0, t[0], iospace), Ok(Output::Done));
    let attach = Syscall::IoSpaceAttach {
        iommu: s(19),
        iospace: s(100),
        device: 0x0100,
    };
    assert_eq!(sys.apply(0, t[0], attach), Ok(Output::Waits));
    sys.settle(32);
    let seal = Syscall::CoreStop {
        core: core(0),
        mode: CORE_STOP_SEAL,
    };
    assert_eq!(sys.apply(1, t[1], seal), Ok(Output::Done));
    sys.run(sys.now() + 2_000_000, 100_000);
    assert_eq!(sys.member(0).state(), MemberState::Sealed);
    let hand = Syscall::RoleHand {
        role: s(20),
        core: core(2),
    };
    assert!(matches!(sys.apply(1, t[1], hand), Ok(Output::Epoch(_))));
    sys.run(sys.now() + 1_000_000, 100_000);
    assert_eq!(sys.role(0).holder(), 2);
    let entry = sys.node(2).table.units[&0].entries.get(&0x0100).copied();
    assert_eq!(entry, Some((0, 1)));
    assert_eq!(sys.check(), Ok(()));
}

/// #105, #106: a sealed node is never observed, by the sweep or by a
/// waiting request; the sweep observes the two next nodes that write.
/// Argon's assertion ran on two nodes, where nodes 2 and 3 do not exist:
/// four nodes here.
#[test]
fn r7_a_sealed_node_is_never_observed_and_the_sweep_skips_it() {
    let (mut sys, t) = System::boot(&machine(4)).unwrap();
    let seal = Syscall::CoreStop {
        core: core(1),
        mode: CORE_STOP_SEAL,
    };
    assert_eq!(sys.apply(0, t[0], seal), Ok(Output::Done));
    sys.run(sys.now() + 2_000_000, 100_000);
    assert_eq!(sys.member(1).state(), MemberState::Sealed);
    let n0 = sys.nodes[0].as_mut().unwrap();
    n0.table.beacons[0].observations.clear();
    n0.now = n0.next_sweep + 1_000;
    let call = Request::Call {
        peer: 1,
        thread: t[0],
        caps: vec![],
        waits: true,
    };
    n0.requests.insert(888, call);
    n0.recovery_step();
    let observed: Vec<u16> = n0.table.beacons[0]
        .observations
        .iter()
        .map(|o| o.node)
        .collect();
    assert!(!observed.contains(&1), "{observed:?}");
    assert!(
        observed.contains(&2) && observed.contains(&3),
        "{observed:?}"
    );
}
