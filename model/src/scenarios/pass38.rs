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
