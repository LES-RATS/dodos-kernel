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

// D-158 (The tag table, untagged mode). Node 0 of an untagged machine:
// space A is root 2 with table 3 and a writable leaf at (0, 0) toward page
// 16; space B is root 20 with table 21 (chunk 5, taken for tables) and a
// writable leaf at (0, 1) toward page 17. Every bind names tag 0.

const A: Page = 2;
const B: Page = 20;

fn bind0(root: Page) -> Op {
    Op::Bind { n: 0, tag: 0, root }
}

fn load0(p: usize, root: Page) -> Op {
    Op::Load {
        n: 0,
        p,
        root,
        tag: 0,
    }
}

fn space(root: Page, table: Page, va: (usize, usize), page: Page) -> [Op; 5] {
    [
        Op::RetypeRoot { n: 0, page: root },
        bind0(root),
        Op::TakeTable { n: 0, page: table },
        Op::Hook {
            n: 0,
            root,
            e0: 0,
            child: table,
        },
        Op::Map {
            n: 0,
            root,
            tag: 0,
            va,
            page,
            write: true,
        },
    ]
}

/// The untagged machine of `width` tag bits with spaces A and B.
fn untagged(width: u32, old: OldRules) -> Machine {
    let mut m = Machine::new(2, 2, 8).untagged(width);
    m.old = old;
    let take_tables = Op::Take {
        n: 0,
        chunk: 5,
        kind: Take::Tables,
    };
    steps(&mut m, &[TAKE_4, take_tables]);
    steps(&mut m, &space(A, 3, (0, 0), 16));
    steps(&mut m, &space(B, 21, (0, 1), 17));
    m
}

/// The leaves core `core` holds, by virtual address.
fn cached(m: &Machine, core: usize) -> Vec<((usize, usize), Page)> {
    let leaves = m.tlb[core].leaves.iter();
    leaves.map(|(&(_, va), &(page, _))| (va, page)).collect()
}

// "When the width read at start is below 2, the node runs untagged, a fact
// of the machine written at start and never changed: the gate reserves no
// ASID [...] the table holds entry 0 alone, the kernel's root". With 2
// bits the rules are the tagged ones: the kernel's entry, the user tags,
// the gate's tag after them.
#[test]
fn the_tag_table_is_sized_from_the_width_read_at_start() {
    for width in [0, 1] {
        let m = Machine::new(2, 2, 8).untagged(width);
        assert!(m.is_untagged() && m.riscv, "width {width}");
        assert_eq!((m.tags(), m.gate_tag()), (1, None), "width {width}");
        for (n, node) in m.gate.nodes.iter().enumerate() {
            assert_eq!(node.tags.len(), 1);
            assert_eq!(node.tags[0].root, Some(m.layout[n].kernel_root));
        }
        m.check().expect("the untagged machine at start");
    }
    let m = Machine::new(2, 2, 8);
    assert!(!m.is_untagged());
    assert_eq!((m.tag_width, m.tags(), m.gate_tag()), (2, 3, Some(3)));
    assert_eq!(m.gate.nodes[0].tags.len(), 4);
}

// "`bind(0, R)` is the only bind: it takes R from *unbound* to *live* on
// the condition that R's byte reads *unbound*, and writes no entry. A bind
// that names another tag is `invalid`." Two roots are live at once, which
// D-157's one entry "shared by every root" could not hold. On a tagged
// machine tag 0 stays the kernel's.
#[test]
fn an_untagged_bind_names_tag_0_and_writes_no_entry() {
    let mut m = Machine::new(2, 2, 8).untagged(1);
    let table = m.gate.nodes[0].tags.clone();
    steps(&mut m, &[Op::RetypeRoot { n: 0, page: A }]);
    let other = Op::Bind {
        n: 0,
        tag: 1,
        root: A,
    };
    assert_eq!(m.step(&other), Err(Refused::Condition));
    steps(&mut m, &[bind0(A)]);
    assert_eq!(m.gate.classes[A as usize], Class::Root(RootState::Live));
    assert_eq!(m.step(&bind0(A)), Err(Refused::Cas), "bound twice");
    steps(&mut m, &[Op::RetypeRoot { n: 0, page: 3 }, bind0(3)]);
    assert_eq!(m.gate.classes[3], Class::Root(RootState::Live));
    assert_eq!(m.gate.nodes[0].tags, table, "an entry written");
    let mut tagged = Machine::new(2, 2, 8);
    steps(&mut tagged, &[Op::RetypeRoot { n: 0, page: A }]);
    assert_eq!(tagged.step(&bind0(A)), Err(Refused::Condition));
}

// "The root load of (R, 0) checks that R's byte reads *live* and that the
// node owns R [...]. It writes the core's loaded root first, then
// invalidates the core's whole TLB, then loads the root [...]. A core
// therefore holds translations of the root it has loaded and of no other,
// and the owes bit is never set." Core 1 loads A, then B, then goes idle
// on the kernel's root: each time it holds the loaded root's leaves alone.
#[test]
fn an_untagged_root_load_invalidates_the_cores_whole_tlb() {
    let mut m = untagged(1, OldRules::default());
    steps(&mut m, &[load0(1, A)]);
    assert_eq!(m.gate.nodes[0].loaded[1], (A, 0));
    assert_eq!(cached(&m, 1), [((0, 0), 16)]);
    steps(&mut m, &[load0(1, B)]);
    assert_eq!(cached(&m, 1), [((0, 1), 17)]);
    steps(&mut m, &[Op::Idle { n: 0, p: 1 }]);
    assert_eq!(cached(&m, 1), []);
    assert!(m.gate.nodes[0].idle.iter().all(|word| !word.owes));
    // A live root of another node (node 1's page 8), a page that is not a live
    // root (page 16), and a non-zero tag.
    steps(
        &mut m,
        &[
            Op::RetypeRoot { n: 1, page: 8 },
            Op::Bind {
                n: 1,
                root: 8,
                tag: 0,
            },
        ],
    );
    assert_eq!(m.gate.classes[8], Class::Root(RootState::Live));
    assert_eq!(m.step(&load0(1, 8)), Err(Refused::Condition));
    assert_eq!(m.step(&load0(1, 16)), Err(Refused::Condition));
    let tagged = Op::Load {
        n: 0,
        p: 1,
        root: A,
        tag: 1,
    };
    assert_eq!(m.step(&tagged), Err(Refused::Condition));
}

// Without the invalidation at each load (a machine without tags run by
// the tagged rules), core 1 keeps A's leaf while it has B loaded: the
// clearing call of A's leaf asks the cores whose loaded root is A, so not
// core 1, lowers the count at once, and the chunk is given away under a
// translation core 1 still holds.
#[test]
fn an_untagged_load_without_its_invalidation_leaves_a_translation_of_another_root() {
    let old = OldRules {
        untagged_load_keeps_the_tlb: true,
        ..OldRules::default()
    };
    let mut m = untagged(1, old);
    m.step(&load0(1, A)).expect("A loaded");
    m.check().expect("A's leaves");
    m.step(&load0(1, B)).expect("B loaded");
    assert_eq!(cached(&m, 1), [((0, 0), 16), ((0, 1), 17)]);
    assert_eq!(
        m.check(),
        Err("G: a stale translation whose count the gate lowered".into())
    );
}

// "The asked set of a clearing call for a user root (an unmap, a protect)
// is every other core of the node whose loaded root is the root, read
// after the call cleared its entries". Core 1 has A loaded: core 0's unmap
// of A's leaf asks it, and the leaf's count stays until it acknowledges.
// With B loaded, or idle, core 1 is not asked and the count is lowered in
// the call; no owes bit is set.
#[test]
fn an_untagged_clearing_call_asks_the_cores_whose_loaded_root_is_the_root() {
    let unmap = Op::Unmap {
        n: 0,
        p: 0,
        root: A,
        tag: 0,
        va: (0, 0),
    };
    let mut m = untagged(1, OldRules::default());
    steps(&mut m, &[load0(1, A), unmap.clone()]);
    let Request::InUse { asked, .. } = &m.gate.nodes[0].requests[0] else {
        panic!("the request completed with core 1 holding the leaf");
    };
    assert_eq!(asked, &BTreeSet::from([1]));
    assert_eq!(
        (m.gate.map_counts[4], cached(&m, 1)),
        (2, vec![((0, 0), 16)])
    );
    // "Every *other* core of the node": the caller, its loaded root the
    // root too, is not asked (finding 7 of the review of passes 33 and 34).
    let mut both = untagged(1, OldRules::default());
    steps(&mut both, &[load0(0, A), load0(1, A), unmap.clone()]);
    let Request::InUse { asked, .. } = &both.gate.nodes[0].requests[0] else {
        panic!("the request completed with core 1 holding the leaf");
    };
    assert_eq!(asked, &BTreeSet::from([1]));
    steps(&mut m, &[Op::Ack { n: 0, p: 1 }]);
    assert_eq!((m.gate.map_counts[4], cached(&m, 1)), (1, vec![]));
    for other in [load0(1, B), Op::Idle { n: 0, p: 1 }] {
        let mut m = untagged(1, OldRules::default());
        steps(&mut m, &[load0(1, A), other, unmap.clone()]);
        assert_eq!(m.gate.nodes[0].requests[0], Request::Free);
        assert_eq!(m.gate.map_counts[4], 1);
        assert!(m.gate.nodes[0].idle.iter().all(|word| !word.owes));
    }
    // "A call that clears entries checks that R's byte reads *live* or
    // *dying* and that the node owns R": another tag, another node's root.
    let mut m = untagged(1, OldRules::default());
    let wrong_tag = Op::Unmap {
        n: 0,
        p: 0,
        root: A,
        tag: 1,
        va: (0, 0),
    };
    let wrong_node = Op::Unmap {
        n: 1,
        p: 0,
        root: A,
        tag: 0,
        va: (0, 0),
    };
    assert_eq!(m.step(&wrong_tag), Err(Refused::Condition));
    assert_eq!(m.step(&wrong_node), Err(Refused::Condition));
}

// "`unbind(0, R)` takes R from *live* to *unbound* at once, with an empty
// asked set, on the condition that no core's loaded root is R; otherwise
// it is `busy`."
#[test]
fn an_untagged_unbind_is_at_once_and_busy_while_a_core_has_the_root_loaded() {
    let unbind = Op::UnbindRoot {
        n: 0,
        p: 0,
        root: A,
    };
    let mut m = untagged(1, OldRules::default());
    steps(&mut m, &[load0(1, A)]);
    assert_eq!(m.step(&unbind), Err(Refused::Condition));
    assert_eq!(m.gate.classes[A as usize], Class::Root(RootState::Live));
    steps(&mut m, &[Op::Idle { n: 0, p: 1 }, unbind.clone()]);
    assert_eq!(m.gate.classes[A as usize], Class::Root(RootState::Unbound));
    assert_eq!(m.gate.nodes[0].requests[0], Request::Free, "a core asked");
    assert_eq!(m.step(&load0(1, A)), Err(Refused::Condition));
    steps(&mut m, &[bind0(A), load0(1, A)]);
    // The tagged call names a tag the table does not hold; on a tagged
    // machine the untagged call is refused.
    let tagged = Op::Unbind { n: 0, p: 0, tag: 0 };
    assert_eq!(m.step(&tagged), Err(Refused::Condition));
    let mut tagged = Machine::new(2, 2, 8);
    steps(
        &mut tagged,
        &[
            Op::RetypeRoot { n: 0, page: A },
            Op::Bind {
                n: 0,
                tag: 1,
                root: A,
            },
        ],
    );
    assert_eq!(tagged.step(&unbind), Err(Refused::Condition));
}

// "For a *dying* root it is the same set, the caller included when it has
// the root loaded"; "the last teardown call writes no entry". Both cores
// have A loaded when core 0 destroys it: core 0 loads the kernel's root
// and invalidates in the call, core 1 is asked and does the same at its
// acknowledgement, and only then is A torn down.
#[test]
fn an_untagged_dying_root_asks_every_core_that_has_it_loaded() {
    let mut m = untagged(1, OldRules::default());
    let table = m.gate.nodes[0].tags.clone();
    let kernel = m.layout[0].kernel_root;
    let destroy = Op::Destroy {
        n: 0,
        p: 0,
        root: A,
    };
    steps(&mut m, &[load0(0, A), load0(1, A), destroy]);
    assert_eq!(m.gate.classes[A as usize], Class::Root(RootState::Dying));
    assert_eq!(m.gate.nodes[0].loaded, [(kernel, 0), (A, 0)]);
    assert_eq!(cached(&m, 0), []);
    let Request::InUse { asked, .. } = &m.gate.nodes[0].requests[0] else {
        panic!("the request completed with core 1 on the dying root");
    };
    assert_eq!(asked, &BTreeSet::from([1]));
    let teardown = Op::Teardown { n: 0, root: A };
    assert_eq!(m.step(&teardown), Err(Refused::Condition));
    steps(&mut m, &[Op::Ack { n: 0, p: 1 }]);
    assert_eq!(m.gate.nodes[0].loaded[1], (kernel, 0));
    assert_eq!(cached(&m, 1), []);
    steps(&mut m, &[teardown]);
    assert_eq!(m.gate.classes[A as usize], Class::Data);
    assert_eq!(m.gate.map_counts[4], 1, "B's leaf alone");
    assert_eq!(m.gate.nodes[0].tags, table, "an entry written");
}

// The exploration by faulty kernels on machines without tags, of 0 bits
// and of 1 bit: the checker after every step, its tag invariants read for
// the untagged mode.
#[test]
fn faulty_kernels_keep_g_on_untagged_machines() {
    use super::faulty::Rng;
    let (mut done, mut loads, mut asked) = (0, 0, 0);
    // Loads of another root by a core that has translations cached, which
    // the load's invalidation is for; and calls that name another node's
    // root, which `tag_holds` refuses (finding 7 of the review of passes 33
    // and 34: the exploration reached neither).
    let (mut switches, mut foreign) = (0, 0);
    for seed in 0..200u64 {
        let width = (seed % 2) as u32;
        let mut m = Machine::new(2, 2, 8)
            .untagged(width)
            .with_units()
            .with_spaces();
        let mut rng = Rng(seed + 1);
        for step in 0..400 {
            let op = m.random_op(&mut rng);
            let switch = match op {
                Op::Load { n, p, root, .. } if p < m.layout[usize::from(n)].cores.len() => {
                    let core = m.core(n, p);
                    m.gate.nodes[usize::from(n)].loaded[p].0 != root
                        && !m.tlb[core].leaves.is_empty()
                }
                _ => false,
            };
            let named = match op {
                Op::Map { n, root, .. } | Op::Unmap { n, root, .. } | Op::Load { n, root, .. } => {
                    let live =
                        m.gate.classes.get(root as usize) == Some(&Class::Root(RootState::Live));
                    live && !m.holds(n, root)
                }
                _ => false,
            };
            let outcome = m.step(&op);
            done += usize::from(outcome.is_ok());
            switches += usize::from(switch && outcome.is_ok());
            foreign += usize::from(named);
            assert!(!named || outcome.is_err(), "{op:?} on another node's root");
            loads += usize::from(outcome.is_ok() && matches!(op, Op::Load { .. }));
            let requests = m.gate.nodes.iter().flat_map(|n| &n.requests);
            asked += requests
                .filter(|r| matches!(r, Request::InUse { asked, .. } if !asked.is_empty()))
                .count();
            if let Err(rule) = m.check() {
                panic!("seed {seed} step {step}: {op:?} -> {outcome:?}: {rule}");
            }
        }
    }
    assert!(done > 200 * 40, "only {done} calls performed");
    assert!(loads > 200 && asked > 200, "{loads} loads, {asked} asked");
    assert!(
        switches > 0 && foreign > 0,
        "{switches} switches, {foreign} foreign"
    );
}

// Pass 40 (v2-D #4): sealing on an untagged machine loads the kernel root via
// load_kernel_root, invalidating each core's TLB so no stale user translations
// remain cached after seal.
#[test]
fn an_untagged_seal_invalidates_each_cores_tlb() {
    let mut m = untagged(0, OldRules::default());
    steps(&mut m, &[load0(1, A)]);
    assert_eq!(cached(&m, 1), [((0, 0), 16)]);
    steps(&mut m, &[Op::Seal { n: 0 }]);
    assert_eq!(m.gate.members[0].state, NodeState::Sealed);
    assert_eq!(cached(&m, 0), []);
    assert_eq!(cached(&m, 1), []);
}
