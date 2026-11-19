//! The scenarios of pass 29's gaps 26 to 28 and 30, settled by D-153: each
//! breaks under the reading before D-153 (an `OldRules` switch) and holds
//! under the rule. The exploration seeds that found them (0, 8, 75, 151)
//! no longer replay the same steps since the generator changed, so these
//! scenarios are the regression tests D-153 asks for.

use super::faulty::Op;
use super::*;

fn run(old: OldRules, steps: &[Op]) -> Result<Machine, (usize, String)> {
    let mut m = Machine::new(2, 2, 8);
    m.old = old;
    for (i, op) in steps.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check().map_err(|rule| (i, rule))?;
    }
    Ok(m)
}

/// Node 0's user space on its floor (root 2 bound to tag 1, table 3 hooked
/// at entry 0), chunk 4 taken, core 1 running on the space; with `leaf`, a
/// writable leaf at (0, 0) toward page 16.
fn space(leaf: bool) -> Vec<Op> {
    let mut ops = vec![
        Op::Take { n: 0, chunk: 4 },
        Op::RetypeRoot {
            n: 0,
            p: 0,
            page: 2,
            split: None,
        },
        Op::Bind {
            n: 0,
            tag: 1,
            root: 2,
        },
        Op::TakeTable {
            n: 0,
            p: 0,
            page: 3,
            split: None,
        },
        Op::Hook {
            n: 0,
            root: 2,
            e0: 0,
            child: 3,
        },
    ];
    if leaf {
        ops.push(Op::Map {
            n: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
            page: 16,
            write: true,
        });
    }
    ops.push(Op::Load {
        n: 0,
        p: 1,
        root: 2,
        tag: 1,
    });
    ops
}

// D-153, gap 26: the clearing call invalidates on its own core. Core 0,
// which has the space loaded and the leaf cached, unmaps it; core 1
// acknowledges; the count is lowered while core 0 still caches the leaf.
#[test]
fn the_clearing_call_invalidates_its_own_core() {
    let mut steps = space(true);
    steps.extend([
        Op::Load {
            n: 0,
            p: 0,
            root: 2,
            tag: 1,
        },
        Op::Unmap {
            n: 0,
            p: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
        },
        Op::Ack { n: 0, p: 1 },
    ]);
    let broken = run(
        OldRules {
            no_own_invalidation: true,
            ..OldRules::default()
        },
        &steps,
    )
    .expect_err("the old reading broke nothing");
    assert_eq!(
        broken,
        (
            9,
            "G: a stale translation whose count the gate lowered".to_string()
        )
    );
    run(OldRules::default(), &steps).expect("the rule");
}

// D-153, gap 27: an unbind asks the tag's whole set, idle cores included.
// Core 1, in the tag's set, goes idle; an unbind that skips it completes
// its request with core 1 still in the set, which nothing then empties:
// the tag is never freed.
#[test]
fn an_unbind_asks_the_idle_cores_of_its_set() {
    let mut steps = space(false);
    steps.extend([Op::Idle { n: 0, p: 1 }, Op::Unbind { n: 0, p: 0, tag: 1 }]);
    let old = run(
        OldRules {
            unbind_skips_idle: true,
            ..OldRules::default()
        },
        &steps,
    )
    .expect("the old reading");
    assert_eq!(old.gate.nodes[0].requests[0], Request::Free);
    assert!(old.gate.nodes[0].pending.iter().all(BTreeSet::is_empty));
    assert_eq!(
        old.gate.nodes[0].tags[1].root,
        Some(2),
        "the old reading freed the tag"
    );
    let mut m = run(OldRules::default(), &steps).expect("the rule");
    m.step(&Op::Ack { n: 0, p: 1 })
        .expect("the idle core's ack");
    m.check().expect("the rule");
    assert_eq!(m.gate.nodes[0].tags[1].root, None, "the tag freed");
}

// D-153, gap 28: every row from *data* requires the chunk's map count 0.
// Page 16, mapped writable in node 0's space, becomes a table.
#[test]
fn a_mapped_page_never_becomes_a_table() {
    let mut steps = space(true);
    steps.extend([
        Op::TakeTable {
            n: 0,
            p: 0,
            page: 16,
            split: Some(17),
        },
        Op::Ack { n: 0, p: 1 },
    ]);
    let broken = run(
        OldRules {
            class_change_ignores_map_count: true,
            ..OldRules::default()
        },
        &steps,
    )
    .expect_err("the old reading broke nothing");
    assert_eq!(
        broken.1,
        "G: a writable translation reaches a page that is not data"
    );
    let mut m = run(OldRules::default(), &space(true)).expect("the space");
    assert_eq!(
        m.step(&Op::TakeTable {
            n: 0,
            p: 0,
            page: 16,
            split: Some(17),
        }),
        Err(Refused::Condition)
    );
}

// D-153, gap 30: while a request names a give of a chunk, a write that
// would raise one of its counts is refused. Node 0 gives chunk 4 (core 1
// asked), maps page 16 meanwhile, and the give completes with the leaf in
// place.
#[test]
fn no_count_rises_while_a_give_is_in_flight() {
    let mut steps = space(false);
    steps.extend([Op::Give {
        n: 0,
        p: 0,
        chunk: 4,
        to: 1,
    }]);
    let map = Op::Map {
        n: 0,
        root: 2,
        tag: 1,
        va: (0, 0),
        page: 16,
        write: true,
    };
    let mut old_steps = steps.clone();
    old_steps.extend([map.clone(), Op::Ack { n: 0, p: 1 }]);
    let broken = run(
        OldRules {
            counts_free_during_give: true,
            ..OldRules::default()
        },
        &old_steps,
    )
    .expect_err("the old reading broke nothing");
    assert_eq!(
        broken.1,
        "G: a translation reaches memory its node does not hold"
    );
    let mut m = run(OldRules::default(), &steps).expect("the give asked");
    assert_eq!(m.step(&map), Err(Refused::Condition));
}
