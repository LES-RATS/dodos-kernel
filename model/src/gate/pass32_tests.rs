//! Pass 32: the gate defects that the full audit of the model found and
//! that an independent re-judge confirmed (lot 6: C-1 to C-5, M-1, M-3 to
//! M-5). Each scenario is a sequence of gate calls a faulty kernel can
//! make; each fails without the rule it names.

use super::faulty::Op;
use super::grants::GrantState;
use super::unit::RecordState;
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

const NODE_1_TAKEN: Owner = Owner::Taken {
    node: 1,
    generation: 1,
};

// Lot 6, C-1: two cores of a node give the same chunk. The second give is
// refused while the first one's request names the chunk (Gate words, the
// ownership words and G5: the run unmapped and that unmapping invalidated
// on every core before any word is written), so one request at most ever
// completes a give of a chunk.
#[test]
fn a_chunk_whose_give_is_in_flight_is_not_given_again() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[TAKE_4, give(0)]);
    assert_eq!(m.step(&give(1)), Err(Refused::Condition));
    steps(
        &mut m,
        &[
            Op::Ack { n: 0, p: 1 },
            Op::Take {
                n: 1,
                chunk: 4,
                kind: Take::Ordinary,
            },
            Op::Ack { n: 0, p: 0 },
        ],
    );
    assert_eq!(m.gate.owners[4], NODE_1_TAKEN);
}

// Lot 6, C-1: the word of a give is written by a compare-and-swap from
// *taken* by the giving generation (Gate words, the ownership words, row
// *taken* → *free*). With the second give let through (the reading before
// D-153's gap 30, which read no request), its late completion finds the
// word taken by node 1 and writes nothing.
#[test]
fn a_gives_completion_writes_its_word_by_compare_and_swap() {
    let mut m = Machine::new(2, 2, 8);
    m.old.counts_free_during_give = true;
    steps(
        &mut m,
        &[
            TAKE_4,
            give(0),
            give(1),
            Op::Ack { n: 0, p: 1 },
            Op::Take {
                n: 1,
                chunk: 4,
                kind: Take::Ordinary,
            },
            Op::Ack { n: 0, p: 0 },
        ],
    );
    assert_eq!(m.gate.owners[4], NODE_1_TAKEN);
}

// Lot 6, C-1: the compare-and-swap alone does not hold G5. Node 0 gives
// chunk 4 twice to node 1, which dies; the first completion frees the
// word, node 0 takes the chunk again (rule 3 of taking) and the second
// completion finds the word it expects, *taken* by node 0's generation,
// over a run mapped anew. The refusal of the second give is what stops it.
#[test]
fn a_second_give_would_free_a_chunk_its_node_took_again() {
    let aba = [
        Op::Death { n: 1 },
        Op::Ack { n: 0, p: 1 },
        TAKE_4,
        Op::Ack { n: 0, p: 0 },
    ];
    let mut old = Machine::new(2, 2, 8);
    old.old.counts_free_during_give = true;
    steps(&mut old, &[TAKE_4, give(0), give(1)]);
    for op in &aba {
        old.step(op).expect("performed under the old reading");
    }
    assert_eq!(
        old.check(),
        Err("G: the kernel's own view reaches memory its node does not hold".into())
    );
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[TAKE_4, give(0)]);
    assert_eq!(m.step(&give(1)), Err(Refused::Condition));
    for op in &aba {
        let _ = m.step(op);
        m.check().expect("the rule");
    }
    assert_eq!(
        m.gate.owners[4],
        Owner::Taken {
            node: 0,
            generation: 1
        }
    );
}

// Lot 6, C-2: a word *free* with a node as owner is that node's while it
// is not dead (Chunks, floors and the ownership words, who may take a
// word, rules 1 and 3): neither a third node nor the giver takes it.
#[test]
fn a_chunk_given_to_a_live_node_is_taken_by_no_other() {
    let mut m = Machine::new(3, 1, 12);
    let take = |n| Op::Take {
        n,
        chunk: 8,
        kind: Take::Ordinary,
    };
    let give = Op::Give {
        n: 0,
        p: 0,
        chunk: 8,
        to: 1,
    };
    steps(&mut m, &[take(0), give]);
    assert_eq!(m.gate.owners[8], Owner::Free { node: Some(1) });
    assert_eq!(m.step(&take(2)), Err(Refused::Condition));
    assert_eq!(m.step(&take(0)), Err(Refused::Condition));
    steps(&mut m, &[take(1)]);
}

// Rule 3 of taking: memory given to a node that is dead is free for any
// live node.
#[test]
fn a_chunk_given_to_a_dead_node_is_free_for_anyone() {
    let mut m = Machine::new(3, 1, 12);
    steps(
        &mut m,
        &[
            Op::Take {
                n: 0,
                chunk: 8,
                kind: Take::Ordinary,
            },
            Op::Give {
                n: 0,
                p: 0,
                chunk: 8,
                to: 1,
            },
            Op::Death { n: 1 },
            Op::Take {
                n: 2,
                chunk: 8,
                kind: Take::Ordinary,
            },
        ],
    );
}

/// Node 0 takes chunk 4 and grants its page 16 to node 1, which answers
/// with its use entry 0 and maps the page at (0, 0) of a space of its own
/// (root 10 under tag 1, table 11), loaded on its core `p`.
fn grantee_maps(p: usize) -> Vec<Op> {
    vec![
        TAKE_4,
        Op::Grant {
            n: 0,
            slot: 0,
            page: 16,
            to: 1,
        },
        Op::Deliver {
            to: 1,
            index: 0,
            granter: 0,
            slot: 0,
        },
        Op::RetypeRoot { n: 1, page: 10 },
        Op::Bind {
            n: 1,
            tag: 1,
            root: 10,
        },
        Op::TakeTable { n: 1, page: 11 },
        Op::Hook {
            n: 1,
            root: 10,
            e0: 0,
            child: 11,
        },
        MAP_REMOTE,
        Op::Load {
            n: 1,
            p,
            root: 10,
            tag: 1,
        },
    ]
}

const MAP_REMOTE: Op = Op::MapRemote {
    n: 1,
    root: 10,
    tag: 1,
    va: (0, 0),
    index: 0,
};

const UNMAP_REMOTE: Op = Op::Unmap {
    n: 1,
    p: 0,
    root: 10,
    tag: 1,
    va: (0, 0),
};

fn use_count(m: &Machine, index: usize) -> u64 {
    m.grants.uses[1][index].expect("the use entry").count
}

// Lot 6, C-3: a grantee's leaf is counted in its use entry and nowhere
// else: the map count of the granter's chunk has one writer, the owner's
// gate (G4, G6). The grant count holds the chunk while the grant lives.
#[test]
fn a_grantees_leaf_raises_no_count_of_the_granters_chunk() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &grantee_maps(0));
    assert_eq!((m.gate.map_counts[4], use_count(&m, 0)), (0, 1));
    assert_eq!(m.step(&give(0)), Err(Refused::Condition));
    assert_eq!(
        m.step(&Op::FreeGrant { n: 0, slot: 0 }),
        Err(Refused::Condition)
    );
    steps(&mut m, &[UNMAP_REMOTE]);
    assert_eq!((m.gate.map_counts[4], use_count(&m, 0)), (0, 0));
    steps(&mut m, &[Op::FreeGrant { n: 0, slot: 0 }, give(0)]);
}

// Lot 6, C-3: a grantee that dies with a leaf under the grant leaves the
// granter's chunk free to give (G6: the entry is freed when the grantee's
// generation is dead and reclaimed).
#[test]
fn a_dead_grantee_holds_nothing_of_the_granters_chunk() {
    let mut m = Machine::new(2, 2, 8);
    let mut ops = grantee_maps(0);
    ops.push(Op::Death { n: 1 });
    steps(&mut m, &ops);
    assert_eq!(m.gate.members[1].state, NodeState::Dead);
    assert_eq!((m.gate.map_counts[4], m.gate.grant_counts[4]), (0, 0));
    steps(&mut m, &[give(0), Op::Ack { n: 0, p: 1 }]);
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(1) });
}

// Lot 6, C-4: the record of a clearing names the count its leaf is in
// (Gate words, a core's invalidation request: "the count they are in").
// Node 1 maps one granted page under two grants, in two spaces; core 1
// caches the leaf of the first. The clearing of the second completes at
// once and lowers the second use entry, not the first, whose leaf core 1
// still translates: the granter cannot free the first grant yet.
#[test]
fn a_cleared_remote_leaf_lowers_the_use_entry_it_is_counted_in() {
    let mut m = Machine::new(2, 2, 8);
    let space = |root, tag, table, index| {
        [
            Op::RetypeRoot { n: 1, page: root },
            Op::Bind { n: 1, tag, root },
            Op::TakeTable { n: 1, page: table },
            Op::Hook {
                n: 1,
                root,
                e0: 0,
                child: table,
            },
            Op::MapRemote {
                n: 1,
                root,
                tag,
                va: (0, 0),
                index,
            },
        ]
    };
    let grant = |slot| {
        [
            Op::Grant {
                n: 0,
                slot,
                page: 16,
                to: 1,
            },
            Op::Deliver {
                to: 1,
                index: slot,
                granter: 0,
                slot,
            },
        ]
    };
    steps(&mut m, &[TAKE_4]);
    steps(&mut m, &grant(0));
    steps(&mut m, &grant(1));
    steps(&mut m, &space(8, 1, 10, 0));
    steps(&mut m, &space(9, 2, 11, 1));
    let unmap = |p, root, tag| Op::Unmap {
        n: 1,
        p,
        root,
        tag,
        va: (0, 0),
    };
    steps(
        &mut m,
        &[
            Op::Load {
                n: 1,
                p: 1,
                root: 8,
                tag: 1,
            },
            unmap(0, 8, 1),
            unmap(1, 9, 2),
        ],
    );
    assert_ne!(m.gate.nodes[1].requests[0], Request::Free);
    assert_eq!((use_count(&m, 0), use_count(&m, 1)), (1, 0));
    assert_eq!(
        m.step(&Op::FreeGrant { n: 0, slot: 0 }),
        Err(Refused::Condition),
        "a grant freed while a core still translates under it"
    );
    steps(
        &mut m,
        &[
            Op::FreeGrant { n: 0, slot: 1 },
            Op::Ack { n: 1, p: 1 },
            Op::FreeGrant { n: 0, slot: 0 },
        ],
    );
    assert!(m.grants.remote.is_empty());
}

// Lot 6, C-4: a leaf mapped again at an address whose clearing is still
// in flight is one more leaf of the use entry; the completion lowers the
// count of the cleared one and leaves the new one counted.
#[test]
fn a_remote_leaf_mapped_again_while_its_clearing_is_in_flight_stays_counted() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &grantee_maps(1));
    steps(&mut m, &[UNMAP_REMOTE, MAP_REMOTE]);
    assert_eq!(use_count(&m, 0), 2);
    steps(&mut m, &[Op::Ack { n: 1, p: 1 }]);
    assert_eq!(use_count(&m, 0), 1);
    steps(&mut m, &[UNMAP_REMOTE, Op::Ack { n: 1, p: 1 }]);
    assert_eq!(use_count(&m, 0), 0);
    assert!(m.grants.remote.is_empty());
    steps(&mut m, &[Op::FreeGrant { n: 0, slot: 0 }]);
}

// Lot 6, C-5: a dead node's request is never read again (Gate words, a
// core's invalidation request, cell Death). Node 0 dies with its give of
// chunk 4 in flight; node 1 takes the chunk and maps and grants in it.
#[test]
fn a_dead_nodes_give_in_flight_does_not_hold_the_chunks_next_owner() {
    let mut m = Machine::new(2, 2, 8);
    steps(
        &mut m,
        &[
            TAKE_4,
            give(0),
            Op::Death { n: 0 },
            Op::Take {
                n: 1,
                chunk: 4,
                kind: Take::Ordinary,
            },
        ],
    );
    assert_ne!(m.gate.nodes[0].requests[0], Request::Free);
    steps(
        &mut m,
        &[
            Op::RetypeRoot { n: 1, page: 8 },
            Op::Bind {
                n: 1,
                tag: 1,
                root: 8,
            },
            Op::TakeTable { n: 1, page: 11 },
            Op::Hook {
                n: 1,
                root: 8,
                e0: 0,
                child: 11,
            },
            Op::Map {
                n: 1,
                root: 8,
                tag: 1,
                va: (0, 0),
                page: 16,
                write: true,
            },
            Op::Grant {
                n: 1,
                slot: 0,
                page: 17,
                to: 0,
            },
        ],
    );
}

// Lot 6, M-1: a fenced node's gate refuses every write (G13; the grant
// entry's cell Fence: "X's gate refuses"), the freeing of a grant among
// them; R6 frees its entries.
#[test]
fn a_fenced_granter_frees_no_grant() {
    let mut m = Machine::new(2, 2, 8);
    let grant = Op::Grant {
        n: 0,
        slot: 0,
        page: 16,
        to: 1,
    };
    steps(&mut m, &[TAKE_4, grant, Op::Fence { n: 0 }]);
    assert_eq!(
        m.step(&Op::FreeGrant { n: 0, slot: 0 }),
        Err(Refused::Condition)
    );
    assert_eq!(
        m.grants.grants[0][0].map(|g| g.state),
        Some(GrantState::Live)
    );
    m.recover(0);
    steps(&mut m, &[Op::R6 { n: 0 }]);
    assert_eq!(m.grants.grants[0][0], None);
}

// Lot 6, M-3: a node is sealed only once it holds no unit role (the role
// word's cell Seal: "unit roles handed over") and none of its records is
// *open* (the request record's cell Seal). The calls of a seal made one
// by one, without the hand-over, leave the holder sealing.
#[test]
fn a_node_is_not_sealed_while_it_holds_a_unit_role_or_an_open_record() {
    let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
    assert_eq!(m.unit.holder, 0);
    let sealed = |n| Op::SealedCall { n, p: 1 };
    steps(&mut m, &[Op::Sealing { n: 0 }, Op::ParkCall { n: 0, p: 0 }]);
    assert_eq!(m.step(&sealed(0)), Err(Refused::Condition));
    assert_eq!(m.gate.members[0].state, NodeState::Sealing);

    let mut m = Machine::new(2, 2, 8).with_units();
    let root = m.ioroot_page(1);
    steps(
        &mut m,
        &[
            Op::IoAttach {
                n: 1,
                device: 2,
                root,
                entry: 0,
            },
            Op::Sealing { n: 1 },
            Op::ParkCall { n: 1, p: 0 },
        ],
    );
    assert_eq!(m.unit.records[1][0].state, RecordState::Open);
    assert_eq!(m.step(&sealed(1)), Err(Refused::Condition));
    assert_eq!(m.gate.members[1].state, NodeState::Sealing);
}

// The whole seal refuses before any write while a record of the node is
// *open*: the node stays live.
#[test]
fn a_seal_waits_for_the_answers_to_its_open_records() {
    let mut m = Machine::new(2, 2, 8).with_units();
    let root = m.ioroot_page(1);
    let attach = Op::IoAttach {
        n: 1,
        device: 2,
        root,
        entry: 0,
    };
    steps(&mut m, &[attach]);
    assert_eq!(m.step(&Op::Seal { n: 1 }), Err(Refused::Condition));
    assert_eq!(m.gate.members[1].state, NodeState::Live);
    m.check().expect("nothing written");
}

// Lot 6, M-4: the holder's gate writes an *open* record *failed* whenever
// its role word reads *failed*, whether its call sets the bit or finds it
// set (Gate words, the request record, row (s, open) → (s, failed)). A
// record opened after the unit failed is answered, released, and its
// handle retired.
#[test]
fn a_record_opened_after_the_unit_failed_is_answered_failed() {
    let mut m = Machine::new(2, 2, 8).with_units();
    let claim = Op::ClaimMsi {
        n: 1,
        core: 2,
        v: 0,
    };
    let remove = Op::RemoveMsi {
        n: 1,
        core: 2,
        v: 0,
        entry: 0,
    };
    steps(&mut m, &[claim.clone(), Op::UnitFails, remove]);
    assert_eq!(m.unit.records[1][0].state, RecordState::Open);
    let accept = Op::Accept {
        h: 0,
        slot: 1,
        entry: 0,
    };
    steps(&mut m, &[accept]);
    assert_eq!(m.unit.records[1][0].state, RecordState::Failed);
    steps(&mut m, &[Op::Release { n: 1, entry: 0 }]);
    assert!(m.dev.retired.contains(&(2, 0)));
    assert_eq!(m.step(&claim), Err(Refused::Condition));
}

// Lot 6, M-5: the gate refuses an argument out of its range, whatever the
// kernel passes: a tag beyond the tag table, a core position the node
// does not have, a node that does not exist, a run that ends beyond the
// address space.
#[test]
fn an_argument_out_of_its_range_is_refused() {
    let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
    steps(&mut m, &[TAKE_4]);
    let (root, va) = (2, (0, 0));
    let wild = [
        Op::Map {
            n: 0,
            root,
            tag: 7,
            va,
            page: 16,
            write: false,
        },
        Op::Unmap {
            n: 0,
            p: 0,
            root,
            tag: 7,
            va,
        },
        Op::Unmap {
            n: 0,
            p: 9,
            root,
            tag: 1,
            va,
        },
        Op::Unhook {
            n: 0,
            p: 0,
            root,
            tag: 7,
            e0: 0,
        },
        Op::Unhook {
            n: 0,
            p: 9,
            root,
            tag: 1,
            e0: 0,
        },
        Op::MapRemote {
            n: 0,
            root,
            tag: 7,
            va,
            index: 0,
        },
        Op::Bind { n: 0, tag: 7, root },
        Op::Load {
            n: 0,
            p: 9,
            root,
            tag: 1,
        },
        Op::Load {
            n: 0,
            p: 0,
            root,
            tag: 7,
        },
        Op::Idle { n: 0, p: 9 },
        Op::Ack { n: 0, p: 9 },
        Op::Unbind { n: 0, p: 9, tag: 1 },
        Op::Unbind { n: 0, p: 0, tag: 7 },
        Op::Destroy { n: 0, p: 9, root },
        Op::Barrier { n: 0, p: 9 },
        Op::Give {
            n: 0,
            p: 9,
            chunk: 4,
            to: 1,
        },
        Op::Give {
            n: 0,
            p: 0,
            chunk: 4,
            to: 9,
        },
        Op::Give {
            n: 0,
            p: 0,
            chunk: 99,
            to: 1,
        },
        Op::Take {
            n: 0,
            chunk: 99,
            kind: Take::Ordinary,
        },
        Op::ItsRetype {
            n: 0,
            first: u64::MAX,
            k: 1,
        },
        Op::ItsRetype {
            n: 0,
            first: 0,
            k: 64,
        },
        Op::KernelAccess {
            n: 0,
            p: 9,
            page: 16,
            value: None,
        },
    ];
    for op in &wild {
        assert_eq!(m.step(op), Err(Refused::Condition), "{op:?}");
        m.check().expect("nothing written");
    }
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &[Op::Sealing { n: 0 }]);
    for op in [Op::ParkCall { n: 0, p: 9 }, Op::SealedCall { n: 0, p: 9 }] {
        assert_eq!(m.step(&op), Err(Refused::Condition), "{op:?}");
    }
}

// Pass 34, R-7 of the review of pass 32 (G6: "use count ± leaves: the
// grantee's, its maps and the completions"). A fenced grantee's gate
// writes nothing, so its use count still equals its leaves; only a dead
// node's entries are skipped (R6 frees them while its requests stay).
#[test]
fn a_fenced_grantees_use_count_that_is_not_its_leaves_is_reported() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &grantee_maps(0));
    for state in [NodeState::Fencing, NodeState::Fenced] {
        let mut m = m.clone();
        m.gate.members[1].state = state;
        assert_eq!(m.check(), Ok(()));
        m.grants.uses[1][0].as_mut().expect("the use entry").count = 2;
        assert_eq!(
            m.check(),
            Err("USE: a use count that is not the leaves under its grant".into())
        );
    }
}

// Pass 34, R-7: a grantee's record whose leaf the walk of the tables did
// not count (its table page not of class table) is reported, not a
// panic of the checker on the subtraction. The whole check reports the
// class first (CLASS), so the rule of the counts is called alone.
#[test]
fn a_grantees_leaf_in_no_table_is_reported() {
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &grantee_maps(0));
    m.gate.classes[11] = Class::Data;
    assert!(m.check().is_err());
    assert_eq!(
        m.check_counts(),
        Err("USE: a grantee's leaf in no table walked".into())
    );
}

// Pass 34, R-8 of the review of pass 32 (the grant entry's cell Fence:
// "X's gate refuses"; cell Death: "as grantee, the granters pin (R3) and
// free at its generation's end"). The grantee's R6 frees the grants of a
// granter whose gate writes; a fenced granter's entry is left to its own
// R6, which frees it.
#[test]
fn a_grantees_r6_leaves_a_fenced_granters_grant_to_its_own_r6() {
    // Three nodes, so that node 2 holds the unit through both deaths; the
    // first chunk past the floors is 6.
    let mut m = Machine::new(3, 2, 8);
    let mut ops = grantee_maps(0);
    ops[0] = Op::Take {
        n: 0,
        chunk: 6,
        kind: Take::Ordinary,
    };
    ops[1] = Op::Grant {
        n: 0,
        slot: 0,
        page: 6 * PAGES_PER_CHUNK,
        to: 1,
    };
    steps(&mut m, &ops);
    steps(&mut m, &[Op::Fence { n: 0 }, Op::Death { n: 1 }]);
    assert_eq!(m.gate.members[1].state, NodeState::Dead);
    assert!(m.grants.grants[0][0].is_some(), "a fenced gate wrote");
    m.recover(0);
    steps(&mut m, &[Op::R6 { n: 0 }]);
    assert_eq!(m.gate.members[0].state, NodeState::Dead);
    assert_eq!(m.grants.grants[0][0], None);
}

// Pass 36, finding 11 of the review of passes 33 and 34 (G13: a sealed
// node's gate refuses every write; Grants, Generations: "A node that was
// sealed while X's generation ended ... when it resumes ... runs R0, R2 and
// R3 ... (R3 frees at once the grants it made to a generation already
// over)"). The grantee's R6 leaves a sealed granter's grant entry as it
// is; the granter's resume frees it.
#[test]
fn a_grantees_r6_leaves_a_sealed_granters_grant_to_its_resume() {
    let mut m = Machine::new(3, 2, 8);
    let mut ops = grantee_maps(0);
    ops[0] = Op::Take {
        n: 0,
        chunk: 6,
        kind: Take::Ordinary,
    };
    ops[1] = Op::Grant {
        n: 0,
        slot: 0,
        page: 6 * PAGES_PER_CHUNK,
        to: 1,
    };
    steps(&mut m, &ops);
    steps(&mut m, &[Op::Seal { n: 0 }]);
    assert_eq!(m.gate.members[0].state, NodeState::Sealed);
    steps(&mut m, &[Op::Death { n: 1 }]);
    assert_eq!(m.gate.members[1].state, NodeState::Dead);
    assert!(m.grants.grants[0][0].is_some(), "a sealed gate wrote");
    steps(&mut m, &[Op::Resume { n: 0 }]);
    assert_eq!(m.gate.members[0].state, NodeState::Live);
    assert_eq!(m.grants.grants[0][0], None, "R3 at the resume");
}
