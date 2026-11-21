//! The scenarios of pass 38: what the review of passes 35 and 36 found
//! unguarded. Each test is named after the sentence it guards.

use super::pass17::seal;
use super::*;
use crate::system::Event;
use dodos_abi::Evidence;

// `node_stop` mode seal (D-146): the write of sealed is the node's last
// gate call; after it the node fences, reclaims and accuses nothing (seed
// 867 of pass 28: a node that had just written sealed accused another in
// the same step). The interleaving the seed met by chance, built event by
// event: node 1, sealing, accuses node 3 (evidence self) and drives its
// fence; node 1's seal reaches its last step; node 3's core confirms its
// stop; then, just before that last step, node 2 finds a failure in
// itself. In the step that writes sealed node 1 must neither go on with
// node 3's fence (write it fenced) nor accuse node 2.
#[test]
fn a_node_that_writes_sealed_fences_and_accuses_nothing_in_the_same_step() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    let step = Event::Timer { node: 1, core: 0 };
    m.ok(0, seal(1));
    m.fail(3);
    assert!(m.sys.events().contains(&step));
    m.sys.fire(step);
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.state(1), MemberState::Sealing);
    assert_eq!(m.state(3), MemberState::Fencing);
    assert_eq!(m.sys.member(3).driver(), 1, "node 1 drives node 3's fence");
    let parking = m.step_until(|m| m.sys.canonical().beacons[1].sealed_at.is_some());
    assert!(parking && m.state(1) == MemberState::Sealing);
    // Node 3's core confirms its stop: the fence's next round, on its
    // driver, would write fenced.
    while let Some(&action) = m
        .sys
        .events()
        .iter()
        .find(|e| matches!(e, Event::Action(_)))
    {
        m.sys.fire(action);
        assert_eq!(m.sys.check(), Ok(()));
    }
    m.fail(2);
    assert_eq!(m.sys.node(1).evidence(2), Some(Evidence::SelfReported));
    assert!(m.sys.events().contains(&step), "node 1's last step is due");
    m.sys.fire(step);
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.state(1), MemberState::Sealed);
    assert_eq!(
        (m.state(3), m.sys.member(3).driver()),
        (MemberState::Fencing, 1),
        "a node that wrote sealed went on with a fence"
    );
    assert_eq!(m.state(2), MemberState::Live, "node 2 was accused");
}

// What runs next, the change list (invariant 60; the review of passes 35
// and 36, finding 4): a context is listed once per list, and an event whose
// choice is made again lists it again on the next list, whether it enters
// again there or stays where it was. Here A entered once, before B, and is
// listed again after B without entering again: its place is its first.
#[test]
fn a_context_listed_again_without_entering_again_is_judged_at_its_first_place() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.place_first(0);
    let second = k.thread_on(300, 0);
    let context = |k: &N, thread| k.m.threads[&thread].home.expect("a context");
    let (a, b) = (context(&k, k.t), context(&k, second));
    let from = k.m.entered;
    k.m.entered += 2;
    for (id, at) in [(a, from), (b, from + 1)] {
        let queued = crate::live(&mut k.m.scheds, id).queue.as_mut();
        queued.expect("in a queue").entered = at;
    }
    k.m.event.entered_from = from;
    k.m.event.changes = vec![a, b, a];
    assert_eq!(k.m.check(), Ok(()));
    // No place of each, in the order of entry, follows the other's.
    let reported = Err("60: order of entry and the change list");
    k.m.event.changes = vec![b, a];
    assert_eq!(k.m.check(), reported);
    k.m.event.changes = vec![b, b, a];
    assert_eq!(k.m.check(), reported);
    // An entered context the lists do not name.
    k.m.event.changes = vec![a];
    assert_eq!(k.m.check(), reported);
}

// D-163 (1), the states of a node, starting → live: "Every core whose word
// names the node reads *owned* without the mark", at a fresh start as at a
// resume (the review of passes 35 and 36, finding 6: only the resume was
// tested). Node 1 of two cores starts afresh; the first of its cores to
// write itself owned has its word marked (only a faulty kernel writes it
// there, with the park request it needs); the other core enters and writes
// itself owned; the node stays starting, and is accused with stale after
// `START_NS`.
#[test]
fn a_node_started_afresh_with_a_marked_core_word_stays_starting_and_is_accused_stale() {
    use super::pass17::{fresh, kill};
    use dodos_abi::{CoreState, CoreWord, START_NS, SWEEP_NS};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    let word = |m: &S, core: usize| CoreWord::from_raw(m.sys.core_word(core)).unwrap();
    let first = m.t[0];
    m.supervisor(0, first);
    m.ok(0, kill(1));
    m.run(30 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, fresh(1, 0b110));
    assert_eq!(m.state(1), MemberState::Starting);
    let owned = |m: &S, core| word(m, core).state() == CoreState::Owned;
    assert!(m.step_until(|m| owned(m, 1) || owned(m, 2)));
    let (marked, other) = if owned(&m, 1) { (1, 2) } else { (2, 1) };
    assert_eq!(word(&m, other).state(), CoreState::Joining);
    let mark = word(&m, marked).marked();
    m.sys.edit_table(|t| {
        t.core_words[marked] = mark.raw();
        t.park_requests[marked] += 1;
    });
    assert_eq!(m.sys.check(), Ok(()));
    m.run(3 * MS);
    assert_eq!(word(&m, other).state(), CoreState::Owned);
    assert_eq!(word(&m, marked), mark, "left as it is");
    assert_eq!(m.state(1), MemberState::Starting);
    m.run(START_NS + SWEEP_NS + 250 * MS);
    assert_eq!(m.sys.member(1).evidence(), Evidence::Stale);
}

// Grants, release by the holder (D-166, gap 59): a pinned entry is judged
// again by its count "while *Y* is starting, live, sealing or sealed; once
// *Y* is accused (fencing, fenced or unfenced) R3 of Reclaim governs the
// entry, whoever pinned it: a removal that reaches its anchor returns
// `again` and the entry is freed only when *Y*'s generation is over,
// whatever the count reads". Node 1's faulty `Release` pinned node 0's
// grant, then node 1 lowered its count to 0; node 1 is then accused. The
// removal that reaches the anchor returns `again`; the entry stays pinned.
#[test]
fn a_removal_that_reaches_a_pinned_grant_of_an_accused_grantee_whose_count_reads_0_is_again() {
    use super::pass22::OBJ;
    use super::pass37::{count, pinned_with_its_revoke_in_flight, revoke_in_flight, state};
    use crate::rings::Body;
    use crate::table::GrantState;
    let (mut m, grant, second) = pinned_with_its_revoke_in_flight();
    let credit = revoke_in_flight(&m).expect("in flight");
    let step = super::pass22::forged(
        &m,
        dodos_abi::NodeMessage::RevokeStep,
        1,
        0,
        Some(credit),
        Body::RevokeStep { removed: false },
    );
    m.sys.inject(1, 0, step);
    assert!(m.step_until(|m| revoke_in_flight(m).is_none()));
    let serial = m.sys.canonical().grants[0][&grant].serial;
    m.sys.undue_release(0, grant, serial);
    super::pass37::unmapped(&mut m);
    assert_eq!(state(&m, grant), Some(GrantState::Pinned));
    assert_eq!(count(&m), 0);
    let third = m.t[2];
    m.supervisor(2, third);
    m.ok(2, super::pass17::kill(1));
    assert_eq!(m.state(1), MemberState::Fencing);
    assert_eq!(
        m.call_by(0, second, Syscall::CapRevoke(s(OBJ))),
        Err(Error::Again)
    );
    assert_eq!(state(&m, grant), Some(GrantState::Pinned));
    assert_eq!(count(&m), 0, "the count read 0");
}

// G13, reload: a node started fresh "writes its node beacon anew (no
// failure, no acknowledgement, no bit of `maps_of`)" and zeroes the gate
// part of its node block, the observations among it. Seed 632 of the long
// run of pass 38: node 3's generation that died had acknowledged node 2,
// still fenced; node 3 started afresh, and its new reclaim of node 2 found
// that acknowledgement already written (ACKED, R5 before R4). Here node 1
// maps node 0's grant (`maps_of` bit 0) and acknowledges node 3's death,
// then dies and starts afresh.
#[test]
fn a_node_started_afresh_writes_its_node_beacon_anew() {
    use super::pass17::{fresh, kill};
    use super::pass22::{granted_on, map_got};
    let mut m = granted_on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    m.settle();
    assert_eq!(m.call(1, map_got(1 << 30)), DONE);
    let supervisor = m.t[2];
    m.supervisor(2, supervisor);
    let g3 = m.sys.member(3).generation();
    m.ok(2, kill(3));
    m.run(30 * MS);
    assert_eq!(m.state(3), MemberState::Dead);
    let beacon = &m.sys.canonical().beacons[1];
    assert_eq!(beacon.acked[3], g3, "node 1 acknowledged node 3");
    assert!(beacon.maps_of[0], "node 1 maps node 0's memory");
    assert!(!beacon.observations.is_empty());
    m.ok(2, kill(1));
    m.run(30 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(2, fresh(1, 0b10));
    let g1 = m.sys.member(1).generation();
    let built = |m: &S| m.sys.has(1) && m.sys.node(1).generation_built() == g1;
    assert!(m.step_until(built));
    let beacon = &m.sys.canonical().beacons[1];
    assert_eq!(beacon.acked, vec![0; 4], "no acknowledgement");
    assert_eq!(beacon.maps_of, vec![false; 4], "no bit of maps_of");
    assert!(beacon.observations.is_empty(), "the observations zeroed");
    assert!(beacon.panic.is_none());
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Live);
}

// The debugger, the trace (code 1, the text of a node's start) and the
// clock: a kernel started at run time stamps its entries from the
// machine's clock, its start's own entry among them. Found by a focused
// run of pass 38's long run: node 4, started afresh 17 times, kept its
// debug area across its generations (its header right), and every
// generation's start entry read instant 0, 17 entries of one code in one
// window (rule 91).
#[test]
fn the_start_entry_of_a_node_started_at_run_time_has_the_instant_of_its_start() {
    use super::pass17::{fresh, kill};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let first = m.t[0];
    m.supervisor(0, first);
    m.ok(0, kill(2));
    m.run(30 * MS);
    m.ok(0, fresh(2, 0b100));
    let g2 = m.sys.member(2).generation();
    let built = |m: &S| m.sys.has(2) && m.sys.node(2).generation_built() == g2;
    assert!(m.step_until(built));
    let now = m.sys.now();
    let area = &m.sys.canonical().crash.areas[2];
    let start = area
        .kernel
        .iter()
        .find(|e| e.seq != 0 && e.code() == 1 && e.word2 >> 32 == u64::from(g2))
        .expect("the start entry of the new generation");
    assert_eq!(start.instant, now);
}

// Roles, the reports are pulled: "a node that seals during the rebuild
// answers the asks that reached it before it sealed and is then treated as
// sealed". Found by a focused run of pass 38's long run (seed 574): node 4,
// of two cores, sealing, received node 2's `ReportAsk` while its second
// core parked; its last step wrote sealed and left the ask unread; node 2
// passed it over as sealed, kept the credit for the late report, then
// sealed and handed the role; at node 4's resume the ask no longer came
// from the role's holder, the receive check refused it, and node 2's
// credit stayed in use with nothing in flight (CREDIT). Here node 2 of two
// cores seals; once its second core is parking, the unit role's holder
// asks it for its report: it answers before it writes sealed.
#[test]
fn a_sealing_node_answers_a_report_ask_that_reached_it_while_its_cores_park() {
    use super::pass17::seal;
    use super::units::IOMMU_ROLE;
    use crate::rings::Body;
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2, 3]]));
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let sealing = 2;
    assert_ne!(holder, sealing, "the holder is not the sealing node");
    let other = if holder == 0 { 1 } else { 0 };
    m.ok(other, seal(sealing));
    // Core 3 confirms its park: node 2's next reserve step is its last.
    let parked =
        |m: &S| m.sys.canonical().beacons[2].sealed_at.is_some() && m.sys.node(2).park_confirmed(3);
    assert!(m.step_until(parked) && m.state(sealing) == MemberState::Sealing);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let ask = Body::ReportAsk {
        role: IOMMU_ROLE as u16,
        epoch,
        key: 0,
    };
    let message = super::pass22::forged(
        &m,
        dodos_abi::NodeMessage::ReportAsk,
        holder,
        sealing,
        Some((0, 1)),
        ask,
    );
    m.sys.inject(holder, sealing, message);
    let unread = |m: &S| {
        let t = m.sys.canonical();
        t.rings[holder as usize][2].kernel.produced != t.beacons[2].consumed[holder as usize].0
    };
    assert!(unread(&m));
    let step = crate::system::Event::Timer { node: 2, core: 0 };
    let due = m.step_until(|m| m.sys.events().contains(&step));
    assert!(due && m.state(sealing) == MemberState::Sealing && unread(&m));
    m.sys.fire(step);
    assert_eq!(m.sys.check(), Ok(()));
    assert!(m.step_until(|m| m.state(sealing) == MemberState::Sealed));
    assert!(!unread(&m), "the ask was left unread under the seal");
}

// Address spaces and their tables (D-137, D-139): the tables a change
// unhooks are listed in its request, at most 2 × (L − 1), and join the free
// list only once its invalidation is complete; under a failed unit it never
// is, and they never do (IOMMU). Seed 89 of pass 38's long run: node 2
// mapped and unmapped its frame in its iospace again and again behind the failed
// unit; the tables of the first change, kept, were filed under the frame
// capability, and the second change's tables joined them, so rule 59 read
// one change unhooking more than its request lists.
#[test]
fn the_tables_a_failed_unit_keeps_are_not_counted_with_the_next_change_of_the_record() {
    use super::pass24::{GOT, Y, granted, io_map};
    use super::units::IOMMU_ROLE;
    let mut g = granted(false);
    for round in 0..3 {
        let map = io_map(&g.m, GOT);
        g.m.ok(Y, map);
        if round == 0 {
            g.m.sys.break_unit(IOMMU_ROLE);
        }
        // Behind a unit known failed the unmap is `busy`, its record
        // emptied with its counts kept (IOMMU).
        let unmapped = g.m.call(Y, Syscall::IoSpaceUnmap(s(GOT)));
        let expected = if round == 0 { WAITS } else { Err(Error::Busy) };
        assert_eq!(unmapped, expected, "round {round}");
        g.m.run(2 * dodos_abi::IOMMU_WAIT_NS + 60 * MS);
        assert!(g.m.sys.role(IOMMU_ROLE).failed());
        assert_eq!(g.m.sys.check(), Ok(()));
    }
}
