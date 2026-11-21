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
