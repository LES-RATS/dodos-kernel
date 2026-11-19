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
        let mut m = Machine::new(2, 2, 8);
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
    let mut m = Machine::new(2, 2, 8);
    let mut rng = Rng(seed + 1);
    for step in 0..400 {
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
    for seed in 0..200u64 {
        let mut m = Machine::new(2, 2, 8);
        let mut rng = Rng(seed + 1);
        for _ in 0..400 {
            let op = m.random_op(&mut rng);
            if m.step(&op).is_ok() {
                let name = format!("{op:?}");
                *done
                    .entry(name.split(' ').next().unwrap().to_string())
                    .or_default() += 1;
            }
            asked += m
                .gate
                .nodes
                .iter()
                .flat_map(|n| &n.requests)
                .filter(|r| matches!(r, Request::InUse { asked, .. } if !asked.is_empty()))
                .count();
        }
    }
    eprintln!("{done:#?}\nsteps with a request asking a core: {asked}");
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
        Op::RetypeRoot { n: 0, page: 2 },
        Op::Bind {
            n: 0,
            tag: 1,
            root: 2,
        },
        Op::TakeTable { n: 0, page: 3 },
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
        Op::RetypeRoot { n: 0, page: 4 },
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
        Op::RetypeRoot { n: 0, page: 5 },
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
        Op::RetypeRoot { n: 0, page: 2 },
        Op::Bind {
            n: 0,
            tag: 1,
            root: 2,
        },
        Op::TakeTable { n: 0, page: 3 },
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
        broken.1, "G: a translation reaches memory its node does not hold",
        "{broken:?}"
    );
    let mut m = run(OldRules::default(), &steps).expect("the tables");
    m.step(&Op::Ack { n: 0, p: 1 })
        .expect("the idle core's acknowledgement");
    m.check().expect("after the give");
    assert_eq!(m.gate.owners[4], Owner::Free { node: Some(1) });
}
