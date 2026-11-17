//! Pass 23: the rules of the contract that the final review found held by
//! no test, or only by a test's name, each with a scenario that fails when
//! the rule is removed.

use super::*;

const FRAME: u32 = 150;

/// Node 1 of cores 1 and 2, its first thread on core 1 and a thread on core
/// 2 that has the space loaded; a frame mapped by node 1; then core 2
/// stops answering and the unmap starts a request that asks it.
fn request_toward_a_stopped_core() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.ok(1, place_first(1, 1));
    let _ = m.thread_on(1, 300, 2);
    m.settle();
    let frame = m.retype(1, ObjectKind::Frame, 1, FRAME);
    m.ok(1, frame);
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(FRAME),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(1, map);
    m.run(MS);
    m.sys.hang(2, true);
    assert_eq!(m.call(1, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    let asked = m.sys.node(1).cores.iter().any(|c| c.request.is_some());
    assert!(asked, "the unmap waits for core 2");
    m
}

fn accused(m: &S, node: u16) -> bool {
    m.sys.canonical().beacons[node as usize].panic.is_some() || m.state(node) != MemberState::Live
}

/// E23 of the final review (merge-stopping): "a lock wait or a request not
/// complete, longer than `CORE_ANSWER_NS` … is a failure of the node", and
/// "both sides of every comparison are read from the same clock" (The
/// stopped time, D-137, D-139): a request in flight across a debugger stop
/// longer than `CORE_ANSWER_NS` is not late when the node runs again, and
/// becomes late `CORE_ANSWER_NS` after its start on the clock less the
/// stopped time. Judged by the due event of the requesting core.
#[test]
fn a_request_across_a_long_stop_is_late_only_on_the_clock_less_the_stopped_time() {
    let answer = dodos_abi::CORE_ANSWER_NS;
    let mut m = request_toward_a_stopped_core();
    let started = m.sys.node(1).membership_now();
    m.sys.stub_stop(2 * answer);
    assert_eq!(m.sys.check(), Ok(()));
    // The requesting core takes a timer interrupt at the release (a timer
    // whose instant the stop spanned falls due then): its request is past
    // `CORE_ANSWER_NS` on the hardware clock, not on membership's.
    m.sys.fire(crate::system::Event::Timer { node: 1, core: 0 });
    assert_eq!(m.sys.check(), Ok(()));
    assert!(!accused(&m, 1), "a stop is not the core's silence");
    m.run(answer / 2);
    assert!(!accused(&m, 1), "a stop is not the core's silence");
    assert!(m.sys.node(1).membership_now() < started + answer);
    // The rest of the bound passes with core 2 still silent: late now.
    m.run(answer);
    assert!(accused(&m, 1), "a request older than CORE_ANSWER_NS");
}

/// The other half: without a stop, the request is late exactly at
/// `CORE_ANSWER_NS`, not before (a node that accuses itself too early is
/// refused by no invariant: `check_requests` accepts any state after a
/// self-accusation).
#[test]
fn a_request_with_no_stop_is_late_at_core_answer_ns_and_not_before() {
    let answer = dodos_abi::CORE_ANSWER_NS;
    let mut m = request_toward_a_stopped_core();
    m.run(answer - 2 * 100_000);
    assert!(!accused(&m, 1));
    m.run(4 * 100_000);
    assert!(accused(&m, 1));
}

/// The watch of a node's own cores on the same clock: a core whose
/// heartbeat stops is a failure of the node `STALE_NS` after its last beat
/// on the clock less the stopped time, and no node accuses itself while the
/// release word is raised (The debugger stub, D-135, D-137). A stop of
/// twice `STALE_NS` with core 2 hung leaves node 1 live; the bound then
/// passes and node 1 accuses itself.
#[test]
fn a_core_silent_across_a_long_stop_is_stale_only_on_the_clock_less_the_stopped_time() {
    let stale = dodos_abi::STALE_NS;
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.ok(1, place_first(1, 1));
    let _ = m.thread_on(1, 300, 2);
    m.settle();
    // A beat of core 1's watch has seen core 2's heartbeat before the stop.
    m.run(stale / 2 + 10 * MS);
    assert!(m.sys.node(1).cores[1].watched.0.1 != 0, "core 2 watched");
    m.sys.hang(2, true);
    // Core 1's watch has seen core 2's last heartbeat, on the clock of
    // membership, before the stop.
    m.run(10 * MS);
    let last = m.sys.node(1).cores[1].watched.0;
    m.sys.stub_stop(2 * stale);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(
        !accused(&m, 1),
        "no accusation while the release word is raised"
    );
    assert_eq!(m.sys.node(1).cores[1].watched.0, last);
    m.run(stale / 2);
    assert!(!accused(&m, 1));
    m.run(stale);
    assert!(accused(&m, 1), "core 2 silent for STALE_NS");
}

/// E34 of the final review: "keep the entries that match …, clear every
/// other entry no report claims" (Roles, the rebuild of a unit). A context
/// entry in node 0's name for a device node 0 does not report (node 0
/// live, its real device beside it) is cleared by the new holder's
/// rebuild; the reported one stays. No test held the clearing.
#[test]
fn the_rebuild_clears_an_entry_of_a_live_node_that_its_report_does_not_claim() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let generation = m.sys.member(0).generation();
    let stray = 0x77;
    m.sys.edit_table(|t| {
        let unit = t.units.entry(IOMMU_ROLE).or_default();
        unit.entries.insert(stray, (0, generation));
    });
    assert_eq!(m.sys.check(), Ok(()));
    m.ok(2, pass17::kill(1));
    m.run(30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(m.sys.node(2).rebuilds.is_empty(), "the rebuild ended");
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(entries.contains_key(&(DEVICE as u16)), "reported: kept");
    assert!(
        !entries.contains_key(&stray),
        "claimed by no report: cleared"
    );
}

/// Entropy: "a random generator per physical core …, seeded from the
/// node's seed of the start-up page and the core number, always", and the
/// node's seed is "its own" for "each node, and each fresh start" (entry
/// `EntropySeed`). The generators of two cores of a node, of two nodes, and
/// of one node before and after a fresh start differ; the same machine
/// seeds them alike, and another seed otherwise (the final review's
/// verification of lot F2, M8: every generator started at 0, and a boot
/// that seeded them otherwise compared equal).
#[test]
fn each_core_generator_is_seeded_from_its_nodes_seed_and_its_number() {
    let two_cores = N::on(&one_node(Arch::X86_64, 2));
    let (a, b) = (two_cores.m.cores[0].entropy, two_cores.m.cores[1].entropy);
    assert_ne!(a, b, "two cores of a node");
    assert_eq!(N::on(&one_node(Arch::X86_64, 2)).m.cores[0].entropy, a);
    let mut other = one_node(Arch::X86_64, 2);
    other.boot_seed ^= 1;
    assert_ne!(N::on(&other).m.cores[0].entropy, a, "another machine seed");
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let first = m.sys.node(1).cores[0].entropy;
    assert_ne!(m.sys.node(0).cores[0].entropy, first, "two nodes");
    m.ok(0, pass17::kill(1));
    m.run(30 * MS);
    m.ok(0, pass17::fresh(1, 0b10));
    m.run(30 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_ne!(m.sys.node(1).cores[0].entropy, first, "a fresh start");
}
