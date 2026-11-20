//! Pass 33: the gate rows that D-158 and D-159 add or change. Each
//! scenario is a sequence of gate calls a faulty kernel can make; each
//! fails without the rule it names.

use super::faulty::Op;
use super::*;

/// Runs `ops` with the checker after each; every one must be performed.
fn steps(m: &mut Machine, ops: &[Op]) {
    for (i, op) in ops.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check()
            .unwrap_or_else(|rule| panic!("step {i}: {op:?}: {rule}"));
    }
}

const TAKE_4: Op = Op::Take {
    n: 0,
    chunk: 4,
    kind: Take::Ordinary,
};

fn give(p: usize) -> Op {
    Op::Give {
        n: 0,
        p,
        chunk: 4,
        to: 1,
    }
}

fn destroy_block(p: usize) -> Op {
    Op::DestroyBlock { n: 0, p, chunk: 4 }
}

// D-159 (1), gap 49 (The machine table, releasing and giving; Gate words,
// the ownership words): "Destroying a block writes *free* with the same
// owner into its words", at the completion of the request that unmapped
// its run on every core.
#[test]
fn a_blocks_destruction_frees_its_chunk_for_its_node_at_the_completion() {
    let mut m = Machine::new(2, 2, 8);
    let taken = Owner::Taken {
        node: 0,
        generation: 1,
    };
    steps(&mut m, &[TAKE_4, destroy_block(0)]);
    assert_eq!(m.gate.owners[4], taken, "written before core 1 answered");
    steps(&mut m, &[Op::Ack { n: 0, p: 1 }]);
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(0) });
}

// D-159 (1), gap 49: "A give or a destruction of a block is `busy` while a
// request of the node already names a give or a destruction of one of its
// chunks". The destruction side: under a destruction in flight neither a
// give nor a second destruction is performed, and under a give in flight
// no destruction is.
#[test]
fn a_chunk_whose_destruction_or_give_is_in_flight_is_neither_given_nor_destroyed() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[TAKE_4, destroy_block(0)]);
    assert_eq!(m.step(&give(1)), Err(Refused::Condition));
    assert_eq!(m.step(&destroy_block(1)), Err(Refused::Condition));
    steps(&mut m, &[Op::Ack { n: 0, p: 1 }]);
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(0) });
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[TAKE_4, give(0)]);
    assert_eq!(m.step(&destroy_block(1)), Err(Refused::Condition));
    steps(&mut m, &[Op::Ack { n: 0, p: 1 }]);
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(1) });
}

// D-159 (1), gap 49: the compare-and-swap of the completion alone does not
// hold the destruction either. With the second request let through (the
// reading before D-153's gap 30), node 0 destroys chunk 4 on one core and
// gives it on the other; the destruction completes, node 0 takes the chunk
// again, and the give's late completion finds the word it expects and
// frees a run mapped anew.
#[test]
fn a_give_beside_a_destruction_would_free_a_chunk_its_node_took_again() {
    let aba = [Op::Ack { n: 0, p: 1 }, TAKE_4, Op::Ack { n: 0, p: 0 }];
    let mut old = Machine::new(2, 2, 8);
    old.old.counts_free_during_give = true;
    steps(&mut old, &[TAKE_4, destroy_block(0), give(1)]);
    for op in &aba {
        old.step(op).expect("performed under the old reading");
    }
    assert_eq!(
        old.check(),
        Err("G: the kernel's own view reaches memory its node does not hold".into())
    );
}

/// Node 0's root page 2, made and never bound.
const ROOT: Page = 2;

// D-159 (Gate words, the page class byte, the row aspace root *unbound* →
// *dying*): "owner's, the first step of the destruction; nothing to ask:
// no tag is bound to it and no core loaded it". A root never bound is
// destroyed: no request is opened, the teardown then writes it *data*.
#[test]
fn an_aspace_root_never_bound_is_destroyed() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[Op::RetypeRoot { n: 0, page: ROOT }]);
    assert_eq!(
        m.gate.classes[ROOT as usize],
        Class::Root(RootState::Unbound)
    );
    steps(
        &mut m,
        &[Op::Destroy {
            n: 0,
            p: 0,
            root: ROOT,
        }],
    );
    assert_eq!(m.gate.classes[ROOT as usize], Class::Root(RootState::Dying));
    assert_eq!(m.gate.nodes[0].requests[0], Request::Free, "nothing to ask");
    // A dying root is never bound.
    let bind = Op::Bind {
        n: 0,
        tag: 1,
        root: ROOT,
    };
    assert_eq!(m.step(&bind), Err(Refused::Cas));
    steps(&mut m, &[Op::Teardown { n: 0, root: ROOT }]);
    assert_eq!(m.gate.classes[ROOT as usize], Class::Data);
    assert_eq!(m.gate.not_data[0], 0);
}

// The same row for a root that was bound, mapped and unbound again: its
// unbind invalidated its whole tag on every core that had loaded it, so
// nothing is left to ask; the teardown clears its leaf and lowers its
// count at once.
#[test]
fn an_aspace_root_unbound_with_its_tables_is_destroyed_and_torn_down() {
    let mut m = Machine::new(2, 2, 8);
    let space = [
        TAKE_4,
        Op::RetypeRoot { n: 0, page: ROOT },
        Op::Bind {
            n: 0,
            tag: 1,
            root: ROOT,
        },
        Op::TakeTable { n: 0, page: 3 },
        Op::Hook {
            n: 0,
            root: ROOT,
            e0: 0,
            child: 3,
        },
        Op::Map {
            n: 0,
            root: ROOT,
            tag: 1,
            va: (0, 0),
            page: 16,
            write: true,
        },
        Op::Load {
            n: 0,
            p: 1,
            root: ROOT,
            tag: 1,
        },
        Op::Idle { n: 0, p: 1 },
        Op::Unbind { n: 0, p: 0, tag: 1 },
        Op::Ack { n: 0, p: 1 },
    ];
    steps(&mut m, &space);
    assert_eq!(
        m.gate.classes[ROOT as usize],
        Class::Root(RootState::Unbound)
    );
    assert_eq!(m.gate.map_counts[4], 1);
    steps(
        &mut m,
        &[
            Op::Destroy {
                n: 0,
                p: 0,
                root: ROOT,
            },
            Op::Teardown { n: 0, root: ROOT },
        ],
    );
    assert_eq!(m.gate.classes[ROOT as usize], Class::Data);
    assert_eq!(m.gate.classes[3], Class::Data);
    assert_eq!(m.gate.map_counts[4], 0);
}
