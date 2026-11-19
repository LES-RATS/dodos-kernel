//! The gate model's tests: the exploration by faulty kernels, and the
//! scenarios of the attacks and defects that led to D-150 to D-152.

use super::faulty::{Op, Rng};
use super::*;

/// Seeds of the exploration: every node may be faulty; the checker after
/// every step.
#[test]
fn faulty_kernels_keep_g_on_the_cpu_side() {
    let mut done = 0;
    for seed in 0..200u64 {
        let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
        let mut rng = Rng(seed + 1);
        for step in 0..400 {
            let op = m.random_op(&mut rng);
            let outcome = m.step(&op);
            done += usize::from(outcome.is_ok());
            if let Err(rule) = m.check() {
                panic!("seed {seed} step {step}: {op:?} -> {outcome:?}: {rule}");
            }
        }
    }
    assert!(done > 200 * 40, "only {done} calls performed");
}

/// Replays one seed of the exploration with its steps printed (a test's
/// tool: `DODOS_GATE_SEED`).
#[test]
fn replay_a_gate_seed() {
    let Some(seed) = std::env::var("DODOS_GATE_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let steps = std::env::var("DODOS_GATE_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
    let mut rng = Rng(seed + 1);
    for step in 0..steps {
        let op = m.random_op(&mut rng);
        let outcome = m.step(&op);
        if outcome.is_ok() {
            eprintln!("{step}: {op:?}");
        }
        if let Err(rule) = m.check() {
            eprintln!("{step}: {rule}\n{:#?}\n{:#?}", m.gate.nodes, m.tlb);
            return;
        }
    }
}

/// What the exploration performs, by operation (`DODOS_COUNTS`).
#[test]
fn gate_exploration_coverage() {
    if std::env::var("DODOS_COUNTS").is_err() {
        return;
    }
    let mut done: BTreeMap<String, usize> = BTreeMap::new();
    let mut asked = 0;
    let mut splits = 0;
    for seed in 0..200u64 {
        let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
        let mut rng = Rng(seed + 1);
        for _ in 0..400 {
            let op = m.random_op(&mut rng);
            if m.step(&op).is_ok() {
                let name = format!("{op:?}");
                *done
                    .entry(name.split(' ').next().unwrap().to_string())
                    .or_default() += 1;
            }
            splits += m
                .layout
                .iter()
                .flat_map(|l| m.gate.tables[&l.kernel_table])
                .filter(|e| matches!(e, Entry::Table(_)))
                .count();
            asked += m
                .gate
                .nodes
                .iter()
                .flat_map(|n| &n.requests)
                .filter(|r| matches!(r, Request::InUse { asked, .. } if !asked.is_empty()))
                .count();
        }
    }
    eprintln!(
        "{done:#?}\nsteps with a request asking a core: {asked}\nsteps times split chunks: {splits}"
    );
}

/// Runs `steps` on a machine with `old` rules; the first rule the checker
/// reports, if any.
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

/// Node 0's user space: root page 2 bound to tag 1, table page 3 hooked at
/// entry 0, a writable leaf at (0, 0) toward page 16 (chunk 4, which node
/// 0 took); core 1 has loaded it and cached the leaf.
fn space() -> Vec<Op> {
    vec![
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
        Op::Map {
            n: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
            page: 16,
            write: true,
        },
        Op::Load {
            n: 0,
            p: 1,
            root: 2,
            tag: 1,
        },
    ]
}

// (iv) of the defects of D-150 and D-151: a root written dying while
// another core has it loaded. The core is asked, invalidates, and, its
// loaded root still the dying one, refills under the tag; it loads another
// root; the teardown lowers the counts; the chunk is given away; the tag is
// bound again with an empty set and loaded: the stale leaf reaches memory
// node 1 owns. Under the tables (D-152) the asked core loads the kernel's
// root before it invalidates, and nothing is refilled.
#[test]
fn a_dying_root_loaded_on_another_core_leaves_no_stale_translation() {
    let mut steps = space();
    steps.extend([
        Op::RetypeRoot {
            n: 0,
            p: 0,
            page: 4,
            split: None,
        },
        Op::Bind {
            n: 0,
            tag: 2,
            root: 4,
        },
        Op::Destroy {
            n: 0,
            p: 0,
            root: 2,
        },
        Op::Ack { n: 0, p: 1 },
        Op::Load {
            n: 0,
            p: 1,
            root: 4,
            tag: 2,
        },
        Op::Teardown { n: 0, root: 2 },
        Op::Give {
            n: 0,
            p: 0,
            chunk: 4,
            to: 1,
        },
        Op::Ack { n: 0, p: 1 },
        Op::RetypeRoot {
            n: 0,
            p: 0,
            page: 5,
            split: None,
        },
        Op::Bind {
            n: 0,
            tag: 1,
            root: 5,
        },
        Op::Load {
            n: 0,
            p: 1,
            root: 5,
            tag: 1,
        },
    ]);
    let old = OldRules {
        dying_without_kernel_load: true,
        ..OldRules::default()
    };
    let broken = run(old, &steps).expect_err("the old rule broke nothing");
    assert!(broken.1.starts_with("G:"), "{broken:?}");
    run(OldRules::default(), &steps).expect("the tables");
}

// (v): a root loaded under two tags; an unmap asks one tag's set, the
// core that loaded the root under the other keeps the leaf, the count is
// lowered and the chunk given away. Under the tables a root is bound to
// one tag (bind needs an *unbound* root).
#[test]
fn a_root_under_two_tags_is_refused() {
    let mut steps = space();
    steps.truncate(6);
    let attack = [
        Op::Bind {
            n: 0,
            tag: 2,
            root: 2,
        },
        Op::Load {
            n: 0,
            p: 1,
            root: 2,
            tag: 2,
        },
        Op::Unmap {
            n: 0,
            p: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
        },
        Op::Give {
            n: 0,
            p: 0,
            chunk: 4,
            to: 1,
        },
        Op::Ack { n: 0, p: 1 },
    ];
    steps.extend(attack.iter().cloned());
    let old = OldRules {
        root_under_two_tags: true,
        ..OldRules::default()
    };
    let broken = run(old, &steps).expect_err("the old rule broke nothing");
    assert!(
        broken.1.starts_with("G:") || broken.1.starts_with("TAG:"),
        "{broken:?}"
    );
    let mut m = run(OldRules::default(), &space()[..6]).expect("the space");
    assert_eq!(m.step(&attack[0]), Err(Refused::Cas), "a second tag bound");
}

// (vi): a fresh start that leaves a core's invalidation request in use by
// the dead generation: every unmap of the new generation is refused for
// ever. Under the tables the fresh start zeroes the gate part.
#[test]
fn a_fresh_start_leaves_no_request_of_the_dead_generation() {
    let mut steps = space();
    steps.extend([
        Op::Unmap {
            n: 0,
            p: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
        },
        Op::Death { n: 0 },
        Op::Fresh { n: 0 },
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
        Op::Map {
            n: 0,
            root: 2,
            tag: 1,
            va: (0, 0),
            page: 16,
            write: true,
        },
    ]);
    let unmap = Op::Unmap {
        n: 0,
        p: 0,
        root: 2,
        tag: 1,
        va: (0, 0),
    };
    let old = OldRules {
        fresh_keeps_gate_part: true,
        ..OldRules::default()
    };
    // The old rule: the checker finds the dead generation's request, or
    // the unmap is refused for ever.
    if let Ok(mut m) = run(old, &steps) {
        assert!(
            m.step(&unmap).is_err(),
            "the dead generation's request let the unmap through"
        );
    }
    let mut m = run(OldRules::default(), &steps).expect("the tables");
    assert_eq!(m.step(&unmap), Ok(()));
}

// (ix): the kernel-space unmapping of a give that skips an idle core: the
// idle core runs on the kernel's root, cached the run's kernel leaf, and
// keeps it once the chunk is node 1's. Under the tables the unmapping asks
// every core, idle ones included.
#[test]
fn a_give_invalidates_the_kernel_space_on_idle_cores() {
    let steps = vec![
        Op::Take { n: 0, chunk: 4 },
        Op::Idle { n: 0, p: 1 },
        Op::Give {
            n: 0,
            p: 0,
            chunk: 4,
            to: 1,
        },
    ];
    let old = OldRules {
        kernel_run_skips_idle: true,
        ..OldRules::default()
    };
    let broken = run(old, &steps).expect_err("the old rule broke nothing");
    assert_eq!(
        broken.1, "G: the kernel's own view reaches memory its node does not hold",
        "{broken:?}"
    );
    let mut m = run(OldRules::default(), &steps).expect("the tables");
    m.step(&Op::Ack { n: 0, p: 1 })
        .expect("the idle core's acknowledgement");
    m.check().expect("after the give");
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(1) });
}

/// Node 1's iospace root again, its device attached, as its new
/// generation's start-up does (scaffolding of the scenarios).
fn reattach(m: &mut Machine, n: u16) {
    m.attach_through_unit(n, n);
}

/// Steps one by one, the checker after each; the first rule broken.
fn steps(m: &mut Machine, ops: &[Op]) -> Result<(), (usize, String)> {
    for (i, op) in ops.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check().map_err(|rule| (i, rule))?;
    }
    Ok(())
}

/// (i) up to the late answer: node 1 unmaps a leaf of its iospace; node
/// 0, the holder, accepts, submits, the unit completes; node 1 dies and
/// starts afresh; its new generation maps the same leaf and unmaps it in
/// the same entry.
fn late_answer(old: OldRules) -> Machine {
    let mut m = Machine::new(2, 2, 8).with_units();
    m.old = old;
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 15,
                e: 0,
                page: 12,
            },
            Op::IoUnmap {
                n: 1,
                root: 15,
                entry: 0,
                e: 0,
            },
            Op::Accept {
                h: 0,
                slot: 1,
                entry: 0,
            },
            Op::Submit { h: 0 },
            Op::UnitCompletes { id: 2 },
            Op::Death { n: 1 },
            Op::Fresh { n: 1 },
        ],
    )
    .expect("the first generation");
    reattach(&mut m, 1);
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 15,
                e: 0,
                page: 12,
            },
            Op::IoUnmap {
                n: 1,
                root: 15,
                entry: 0,
                e: 0,
            },
        ],
    )
    .expect("the new generation");
    m
}

// (i) of the defects of D-150 and D-151: a done word of a dead generation
// read after a fresh start that reuses entry and serial. With the serial
// reset at the fresh start, the holder's late answer to the dead
// generation's record matches the new one; the requester lowers the count
// while the unit still caches the new leaf. Under the tables the serial is
// never reset, the fresh start voids the open record, and the late answer
// fails its compare-and-swap.
#[test]
fn a_late_answer_to_a_dead_generation_completes_no_request_of_the_next() {
    let mut m = late_answer(OldRules {
        serial_reset_at_fresh: true,
        ..OldRules::default()
    });
    m.step(&Op::Answer { h: 0, service: 0 })
        .expect("the late answer, matching");
    m.step(&Op::Release { n: 1, entry: 0 })
        .expect("the release on it");
    assert_eq!(
        m.check(),
        Err("G: a device's stale translation whose count the gate lowered".to_string())
    );
    let mut m = late_answer(OldRules::default());
    assert_eq!(m.step(&Op::Answer { h: 0, service: 0 }), Err(Refused::Cas));
    assert!(
        m.step(&Op::Release { n: 1, entry: 0 }).is_err(),
        "released on no answer"
    );
    m.check().expect("the tables");
}

// (ii): a holder's gate reading words 0 to 2 torn against a requester
// restarting the entry. The small machine's records name one operation,
// the invalidation of the requester's domain, so torn words change
// nothing it can observe; what closes the defect under the tables is that
// the requester writes words 0 to 2 only while the record is not open,
// which the gate checks.
#[test]
fn a_requester_cannot_rewrite_an_open_record() {
    let mut m = Machine::new(2, 2, 8).with_units();
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 15,
                e: 0,
                page: 12,
            },
            Op::IoMap {
                n: 1,
                root: 15,
                e: 1,
                page: 13,
            },
            Op::IoUnmap {
                n: 1,
                root: 15,
                entry: 0,
                e: 0,
            },
        ],
    )
    .expect("the steps");
    assert!(
        m.step(&Op::IoUnmap {
            n: 1,
            root: 15,
            entry: 0,
            e: 1
        })
        .is_err(),
        "an open record rewritten"
    );
    let mut m = Machine::new(2, 2, 8).with_units();
    m.old = OldRules {
        words_while_open: true,
        ..OldRules::default()
    };
    steps(
        &mut m,
        &[
            Op::IoMap {
                n: 1,
                root: 15,
                e: 0,
                page: 12,
            },
            Op::IoMap {
                n: 1,
                root: 15,
                e: 1,
                page: 13,
            },
            Op::IoUnmap {
                n: 1,
                root: 15,
                entry: 0,
                e: 0,
            },
            Op::IoUnmap {
                n: 1,
                root: 15,
                entry: 0,
                e: 1,
            },
        ],
    )
    .expect("the old rule lets the record be rewritten");
}

// (iii): no one able to write the value the holder's compare-and-swap
// expects, so no healthy request completes. Under the tables the
// requester writes (s + 1, *open*) and the holder answers from (s, *open*).
#[test]
fn a_healthy_request_completes() {
    let run = |old: OldRules| {
        let mut m = Machine::new(2, 2, 8).with_units();
        m.old = old;
        steps(
            &mut m,
            &[
                Op::IoMap {
                    n: 1,
                    root: 15,
                    e: 0,
                    page: 12,
                },
                Op::IoUnmap {
                    n: 1,
                    root: 15,
                    entry: 0,
                    e: 0,
                },
                Op::Accept {
                    h: 0,
                    slot: 1,
                    entry: 0,
                },
                Op::Submit { h: 0 },
                Op::UnitCompletes { id: 2 },
            ],
        )
        .expect("the steps");
        let answered = m.step(&Op::Answer { h: 0, service: 0 });
        (answered, m)
    };
    let (answered, _) = run(OldRules {
        holder_expects_void: true,
        ..OldRules::default()
    });
    assert_eq!(
        answered,
        Err(Refused::Cas),
        "the old compare-and-swap matched"
    );
    let (answered, mut m) = run(OldRules::default());
    assert_eq!(answered, Ok(()));
    m.step(&Op::Release { n: 1, entry: 0 })
        .expect("the release");
    m.check().expect("the tables");
    assert_eq!(m.gate.map_counts[3], 0, "the count of page 12 lowered");
}

// (vii) of the defects of D-150 and D-151: a stale drop bit across an irq
// removal. Node 1's level SPI arrives (its drop bit set) and is removed;
// node 0 claims the line, which arrives for it; node 1's acknowledgement,
// on its stale bit, deactivates node 0's active line. Under the tables the
// claim clears the bit and the deactivation needs the line word to name the
// node.
#[test]
fn a_stale_drop_bit_deactivates_no_line_of_another_node() {
    let ops = [
        Op::ClaimSpi { n: 1, spi: 0 },
        Op::SpiArrives { spi: 0 },
        Op::RemoveSpi { n: 1, spi: 0 },
        Op::ClaimSpi { n: 0, spi: 0 },
    ];
    let mut m = Machine::new(2, 2, 8);
    m.old = OldRules {
        drop_bit_per_intid: true,
        ..OldRules::default()
    };
    steps(&mut m, &ops).expect("the steps");
    m.step(&Op::AckSpi { n: 1, spi: 0 })
        .expect("the old rule's deactivation");
    assert_eq!(
        m.check(),
        Err("DROP: a node deactivated a line it does not hold".to_string())
    );
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &ops).expect("the steps");
    assert!(
        m.step(&Op::AckSpi { n: 1, spi: 0 }).is_err(),
        "a stale bit deactivated a line"
    );
}

// (viii): a seal whose park requests are raised by a later call, which a
// faulty kernel skips: its other core never parks, yet its `seal_parked`
// equals its unraised request and the seal ends sealed with the core
// running. Under the tables the call that writes sealing raises them.
#[test]
fn a_seal_raises_every_park_request_of_its_node() {
    let ops = [Op::Sealing { n: 1 }, Op::SealedCall { n: 1, p: 0 }];
    let mut m = Machine::new(2, 2, 8);
    m.make_faulty_for_test(1);
    m.old = OldRules {
        park_requests_by_later_call: true,
        ..OldRules::default()
    };
    m.step(&ops[0]).expect("sealing");
    m.step(&ops[1]).expect("the old rule's last call");
    assert_eq!(
        m.check(),
        Err("SEAL: a sealed node's core still runs".to_string())
    );
    let mut m = Machine::new(2, 2, 8);
    m.step(&ops[0]).expect("sealing");
    assert!(
        m.step(&ops[1]).is_err(),
        "sealed with a core that never parked"
    );
    m.check().expect("the tables");
}

/// The long run of the exploration (`DODOS_GATE_SEEDS`, `DODOS_GATE_STEPS`).
#[test]
#[ignore = "long: run with --release"]
fn faulty_kernels_keep_g_long_run() {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .map_or(default, |n| n.parse().expect("a number"))
    };
    let (seeds, length) = (
        number("DODOS_GATE_SEEDS", 5000),
        number("DODOS_GATE_STEPS", 800),
    );
    for seed in 0..seeds {
        let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
        let mut rng = Rng(seed + 1);
        for step in 0..length {
            let op = m.random_op(&mut rng);
            let outcome = m.step(&op);
            if let Err(rule) = m.check() {
                panic!("seed {seed} step {step}: {op:?} -> {outcome:?}: {rule}");
            }
        }
    }
}

// The audit table's first row (D-149, R6's proof), driven by a faulty
// node: the gate model's barrier holds the moment a node dies, the worst a
// faulty node's early `acked` can do. Node 0 takes chunk 4 and grants node
// 1 its page 16, which node 1 maps; node 0 dies. On the barrier alone R6
// freed node 0's grant and memory while node 1 still translated the page;
// under the table node 0 stays fenced while node 1's use entry counts the
// leaf.
#[test]
fn a_dead_granters_memory_is_not_reused_while_a_grantee_translates_it() {
    let ops = [
        Op::Take { n: 0, chunk: 4 },
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
        Op::RetypeRoot {
            n: 1,
            p: 0,
            page: 10,
            split: None,
        },
        Op::Bind {
            n: 1,
            tag: 1,
            root: 10,
        },
        Op::TakeTable {
            n: 1,
            p: 0,
            page: 11,
            split: None,
        },
        Op::Hook {
            n: 1,
            root: 10,
            e0: 0,
            child: 11,
        },
        Op::MapRemote {
            n: 1,
            root: 10,
            tag: 1,
            va: (0, 0),
            index: 0,
        },
        Op::Load {
            n: 1,
            p: 0,
            root: 10,
            tag: 1,
        },
        Op::Death { n: 0 },
    ];
    let mut m = Machine::new(2, 2, 8);
    m.old = OldRules {
        r6_frees_on_acked: true,
        ..OldRules::default()
    };
    let broken = steps(&mut m, &ops).expect_err("the old rule broke nothing");
    // The count of node 0's chunk below the leaf node 1 still has, or the
    // leaf itself toward memory node 0 no longer holds.
    assert!(
        broken.1.starts_with("G:") || broken.1.starts_with("COUNT:"),
        "{broken:?}"
    );
    let mut m = Machine::new(2, 2, 8);
    steps(&mut m, &ops).expect("the table");
    assert_eq!(
        m.gate.members[0].state,
        NodeState::Fenced,
        "node 0 dead under a counted leaf"
    );
}
