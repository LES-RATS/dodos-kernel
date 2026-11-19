//! D-154: dedicated table chunks, which replace D-153's split and merge of
//! the kernel's large leaf (pass 30's gaps 40 to 46 leave with it), and
//! D-155's reserved gate tag. Each rule is shown twice: its attack breaks
//! the checker under the earlier reading (an `OldRules` switch), and the
//! gate refuses it under the tables.

use super::faulty::{Op, Rng};
use super::*;

fn steps(m: &mut Machine, ops: &[Op]) -> Result<(), (usize, String)> {
    for (i, op) in ops.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check().map_err(|rule| (i, rule))?;
    }
    Ok(())
}

fn take(chunk: u64, kind: Take) -> Op {
    Op::Take { n: 0, chunk, kind }
}

/// Node 0 (cores 0 and 1) with its user space in its table floor (root 2
/// bound to tag 1, table 3 hooked at entry 0, loaded on core 1), chunk 4
/// (pages 16 to 19) taken as a table chunk and chunk 5 (pages 20 to 23) as
/// an ordinary one.
fn machine(old: OldRules) -> Machine {
    let mut m = Machine::new(2, 2, 8);
    m.old = old;
    let ops = [
        take(4, Take::Tables),
        take(5, Take::Ordinary),
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
        Op::Load {
            n: 0,
            p: 1,
            root: 2,
            tag: 1,
        },
    ];
    steps(&mut m, &ops).expect("the machine");
    m
}

fn map(page: Page, write: bool) -> Op {
    Op::Map {
        n: 0,
        root: 2,
        tag: 1,
        va: (0, 0),
        page,
        write,
    }
}

fn store(p: usize, page: Page) -> Op {
    Op::KernelAccess {
        n: 0,
        p,
        page,
        value: Some(7),
    }
}

fn load(p: usize, page: Page) -> Op {
    Op::KernelAccess {
        n: 0,
        p,
        page,
        value: None,
    }
}

// `memory_take` kind 2 and the rows of the page class byte from *data*: the
// whole gate word written with the table bit, the run mapped read-only by
// one large leaf; a page of it becomes a root, a table, an ITS table in the
// call, with no request of a core and no change of the kernel's view.
#[test]
fn a_page_of_a_table_chunk_becomes_a_table_with_no_request() {
    let mut m = machine(OldRules::default());
    assert!(m.gate.table_bits[4] && !m.gate.table_bits[5]);
    let large = |m: &Machine, chunk: u64| m.kernel_entry(0, chunk).1;
    assert_eq!(
        large(&m, 4),
        Entry::Large {
            chunk: 4,
            write: false
        }
    );
    assert_eq!(
        large(&m, 5),
        Entry::Large {
            chunk: 5,
            write: true
        }
    );
    // The kernel reads a table chunk and never writes it; it writes an
    // ordinary one.
    for p in 0..2 {
        assert_eq!(m.step(&load(p, 16)), Ok(()));
        assert_eq!(m.step(&store(p, 16)), Err(Refused::Condition));
        assert_eq!(m.step(&store(p, 20)), Ok(()));
    }
    let before = (m.gate.nodes[0].clone(), large(&m, 4));
    let ops = [
        Op::RetypeRoot { n: 0, page: 16 },
        Op::TakeTable { n: 0, page: 17 },
        Op::ItsRetype {
            n: 0,
            first: 18,
            k: 1,
        },
    ];
    steps(&mut m, &ops).expect("the rows from data");
    // No request, no pending bit, no idle word and no kernel-table entry
    // changed: the other core was asked nothing.
    assert_eq!((m.gate.nodes[0].clone(), large(&m, 4)), before);
    assert_eq!(m.gate.not_data[4], 4);
    assert_eq!((m.gate.map_counts[4], m.gate.grant_counts[4]), (0, 0));
    for p in 0..2 {
        assert_eq!(m.step(&store(p, 16)), Err(Refused::Condition));
    }
    m.check().expect("the tables");
}

// The row from *data* asks for the table bit: a page of an ordinary chunk,
// which the kernel maps writable and a leaf may target, never becomes a
// table, a root, an iospace root or an ITS table.
#[test]
fn no_page_of_an_ordinary_chunk_leaves_data() {
    let mut m = machine(OldRules::default());
    let attacks = [
        Op::TakeTable { n: 0, page: 20 },
        Op::RetypeRoot { n: 0, page: 20 },
        Op::IoRoot { n: 0, page: 20 },
        Op::ItsRetype {
            n: 0,
            first: 20,
            k: 0,
        },
        // A page of its ordinary floor too.
        Op::TakeTable { n: 0, page: 4 },
    ];
    for attack in &attacks {
        assert_eq!(m.step(attack), Err(Refused::Condition), "{attack:?}");
        m.check().expect("refused");
    }
    let mut old = machine(OldRules {
        tables_in_ordinary_chunks: true,
        ..OldRules::default()
    });
    let broken = steps(&mut old, &[Op::TakeTable { n: 0, page: 20 }]);
    assert_eq!(
        broken,
        Err((
            0,
            "TABLE: a page that is not data in an ordinary chunk".into()
        ))
    );
}

// G3 (3): no leaf targets a table chunk, not even a *data* page of one
// read-only: the next call may turn it into a table with no count read.
#[test]
fn no_leaf_targets_a_table_chunk() {
    let mut m = machine(OldRules::default());
    for write in [false, true] {
        assert_eq!(m.step(&map(17, write)), Err(Refused::Condition));
    }
    assert_eq!(m.step(&map(20, true)), Ok(()));
    m.check().expect("a leaf toward an ordinary chunk");
    let old = OldRules {
        leaves_into_table_chunks: true,
        ..OldRules::default()
    };
    // The core that loaded the space caches the leaf.
    let broken = steps(&mut machine(old), &[map(17, true)]);
    assert_eq!(
        broken,
        Err((0, "G: a translation reaches a table chunk".into()))
    );
    // With no core on the space, the table chunk still counts a leaf, which
    // the rows from *data* never read.
    let mut idle = machine(old);
    let kernel = idle.layout[0].kernel_root;
    idle.gate.nodes[0].loaded[1] = (kernel, 0);
    idle.invalidate(1, Flush::All);
    let broken = steps(&mut idle, &[map(17, false)]);
    assert_eq!(
        broken,
        Err((0, "TABLE: a table chunk counts a leaf".into()))
    );
}

// G3 (3) on the device side: no leaf of an iospace targets a table chunk.
#[test]
fn no_device_leaf_targets_a_table_chunk() {
    let io_map = |page| Op::IoMap {
        n: 0,
        root: 1,
        e: 0,
        page,
    };
    let mut m = Machine::new(2, 2, 8).with_units();
    steps(&mut m, &[take(4, Take::Tables)]).expect("a table chunk");
    assert_eq!(m.step(&io_map(17)), Err(Refused::Condition));
    let mut old = Machine::new(2, 2, 8).with_units();
    old.old.leaves_into_table_chunks = true;
    let broken = steps(&mut old, &[take(4, Take::Tables), io_map(17)]);
    assert_eq!(
        broken,
        Err((1, "G: a device's translation reaches a table chunk".into()))
    );
}

// G6: a grant's anchor lies only over memory whose chunk has the table bit
// clear, so a table chunk's grant count is 0 and no grant table is walked.
#[test]
fn no_grant_anchor_covers_a_table_chunk() {
    let grant = |page| Op::Grant {
        n: 0,
        slot: 0,
        page,
        to: 1,
    };
    let mut m = machine(OldRules::default());
    assert_eq!(m.step(&grant(17)), Err(Refused::Condition));
    // A page of its table floor is refused as well.
    assert_eq!(m.step(&grant(0)), Err(Refused::Condition));
    assert_eq!(m.step(&grant(20)), Ok(()));
    m.check().expect("an anchor over an ordinary chunk");
    let old = OldRules {
        anchors_over_table_chunks: true,
        ..OldRules::default()
    };
    let broken = steps(&mut machine(old), &[grant(17)]);
    assert_eq!(
        broken,
        Err((0, "TABLE: a table chunk counts a grant's anchor".into()))
    );
}

// Kept kernel tables: a table chunk is mapped read-only from take to give.
// Mapped writable as an ordinary chunk is, the kernel stores into a root.
#[test]
fn the_kernel_never_writes_a_table_chunk() {
    let old = OldRules {
        table_chunks_mapped_writable: true,
        ..OldRules::default()
    };
    let mut m = Machine::new(2, 2, 8);
    m.old = old;
    let broken = steps(&mut m, &[take(4, Take::Tables)]);
    assert_eq!(
        broken,
        Err((
            0,
            "KERNEL: a large leaf's permission is not its chunk's table bit".into()
        ))
    );
    // What the permission guards: the store reaches the root.
    assert_eq!(m.step(&Op::RetypeRoot { n: 0, page: 16 }), Ok(()));
    assert_eq!(m.step(&store(1, 16)), Ok(()));
    assert_eq!(m.kernel.wrote_tables, vec![(0, 16)]);
}

// The give of a table chunk: refused while it holds a page that is not
// *data* (G5), then as for any chunk; the node that takes it writes the
// whole word anew, and its kind 0 clears the table bit.
#[test]
fn a_table_chunk_is_given_once_it_holds_no_table() {
    let mut m = machine(OldRules::default());
    let give = Op::Give {
        n: 0,
        p: 0,
        chunk: 4,
        to: 1,
    };
    steps(&mut m, &[Op::TakeTable { n: 0, page: 17 }]).expect("a table");
    assert_eq!(m.step(&give), Err(Refused::Condition));
    let ops = [
        Op::TableFree { n: 0, page: 17 },
        give,
        Op::Ack { n: 0, p: 1 },
        Op::Take {
            n: 1,
            chunk: 4,
            kind: Take::Ordinary,
        },
    ];
    steps(&mut m, &ops).expect("the give and the take");
    assert!(!m.gate.table_bits[4]);
    assert_eq!(
        m.kernel_entry(1, 4).1,
        Entry::Large {
            chunk: 4,
            write: true
        }
    );
    // Node 1 holds an ordinary chunk: no table in it.
    let table = Op::TakeTable { n: 1, page: 17 };
    assert_eq!(m.step(&table), Err(Refused::Condition));
}

// A node's floor at start-up: its table floor has the table bit and holds
// its start-up space and its iospace root; its ordinary floor holds none.
#[test]
fn the_start_up_tables_lie_in_the_table_floor() {
    let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
    m.fill();
    m.check().expect("the start-up state");
    assert_eq!(
        m.gate.table_bits,
        [true, false, true, false, false, false, false, false]
    );
    for (page, class) in m.gate.classes.iter().enumerate() {
        let chunk = page as u64 / PAGES_PER_CHUNK;
        let kept = chunk >= m.chunks;
        assert!(
            *class == Class::Data || kept || m.gate.table_bits[chunk as usize],
            "page {page}: {class:?}"
        );
    }
    // The table floor is read-only to its kernel, the ordinary floor
    // writable.
    assert_eq!(m.step(&load(0, 2)), Ok(()));
    assert_eq!(m.step(&store(0, 2)), Err(Refused::Condition));
    assert_eq!(m.step(&store(0, 0)), Err(Refused::Condition));
    assert_eq!(m.step(&store(0, 5)), Ok(()));
    m.check().expect("the floor");
}

// R6 clears the whole gate word of the dead node's chunks and floors, its
// table bit with its counts; a fresh start (G13) writes its floors' words
// anew: the table bit for its table floor, 0 for its ordinary floor.
#[test]
fn r6_clears_the_table_bit_and_a_fresh_start_writes_its_floors() {
    let mut m = machine(OldRules::default());
    steps(&mut m, &[Op::RetypeRoot { n: 0, page: 16 }]).expect("a root");
    m.death(0).expect("the death of node 0");
    m.fill();
    m.check().expect("after R6");
    assert_eq!(m.gate.members[0].state, NodeState::Dead);
    // Chunks 0 and 1 its floor, 4 and 5 taken: every word whole 0.
    for chunk in [0, 1, 4, 5] {
        assert!(!m.gate.table_bits[chunk], "chunk {chunk}");
        assert_eq!(m.gate.not_data[chunk], 0, "chunk {chunk}");
    }
    assert!(
        (0..8)
            .chain(16..24)
            .all(|p| m.gate.classes[p] == Class::Data)
    );
    // Node 1 takes what the dead generation held, as an ordinary chunk.
    let again = Op::Take {
        n: 1,
        chunk: 4,
        kind: Take::Ordinary,
    };
    steps(&mut m, &[again, Op::Fresh { n: 0 }]).expect("the fresh start");
    assert!(m.gate.table_bits[0] && !m.gate.table_bits[1]);
    assert!(!m.gate.table_bits[4]);
    // The fresh generation makes its tables in its table floor again.
    let ops = [
        Op::RetypeRoot { n: 0, page: 2 },
        Op::TakeTable { n: 0, page: 4 },
    ];
    assert_eq!(m.step(&ops[0]), Ok(()));
    assert_eq!(m.step(&ops[1]), Err(Refused::Condition));
    m.check().expect("the fresh generation");
}

// A seal and a resume leave a table chunk as it is: its bit, its tables,
// its read-only leaf; the resumed node goes on making tables in it.
#[test]
fn a_sealed_node_keeps_its_table_chunks_through_its_resume() {
    let mut m = machine(OldRules::default());
    steps(&mut m, &[Op::TakeTable { n: 0, page: 17 }]).expect("a table");
    let before = (m.gate.table_bits.clone(), m.gate.classes.clone());
    steps(&mut m, &[Op::Seal { n: 0 }]).expect("the seal");
    assert_eq!(m.gate.members[0].state, NodeState::Sealed);
    // A sealed node's gate refuses every write.
    let table = Op::TakeTable { n: 0, page: 18 };
    assert_eq!(m.step(&table), Err(Refused::Condition));
    assert_eq!((m.gate.table_bits.clone(), m.gate.classes.clone()), before);
    steps(&mut m, &[Op::Resume { n: 0 }]).expect("the resume");
    assert_eq!((m.gate.table_bits.clone(), m.gate.classes.clone()), before);
    assert_eq!(
        m.kernel_entry(0, 4).1,
        Entry::Large {
            chunk: 4,
            write: false
        }
    );
    steps(&mut m, &[table]).expect("a table after the resume");
    assert_eq!(m.step(&store(0, 18)), Err(Refused::Condition));
}

// G7 (D-155): one tag is the gate's own; it is bound to no root and loaded
// for none, and the tag table holds the architecture's other tags.
#[test]
fn the_gates_own_tag_is_bound_to_no_root() {
    assert_eq!((ARCH_TAGS, TAGS, GATE_TAG), (5, 4, 4));
    // As `abi/` sizes it: the tags of the width less the gate's, which is
    // the tag after them (`GATE_PCID` on x86-64).
    assert_eq!(dodos_abi::usable_tags(12), 4095);
    assert_eq!(
        dodos_abi::gate_tag(12),
        Some(u32::from(dodos_abi::x86_64::GATE_PCID))
    );
    let bind = |tag| Op::Bind {
        n: 0,
        tag,
        root: 16,
    };
    let load = |tag| Op::Load {
        n: 0,
        p: 0,
        root: 16,
        tag,
    };
    let mut m = machine(OldRules::default());
    steps(&mut m, &[Op::RetypeRoot { n: 0, page: 16 }]).expect("a root");
    assert_eq!(m.step(&bind(GATE_TAG)), Err(Refused::Condition));
    assert_eq!(m.step(&load(GATE_TAG)), Err(Refused::Condition));
    // Every other tag of the table binds it, the last one included.
    steps(&mut m, &[bind(TAGS - 1), load(TAGS - 1)]).expect("a usable tag");
    assert_eq!(m.gate.nodes[0].tags[GATE_TAG], TagEntry::default());
    let mut old = machine(OldRules {
        gate_tag_bound: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut old,
        &[Op::RetypeRoot { n: 0, page: 16 }, bind(GATE_TAG)],
    );
    assert_eq!(
        broken,
        Err((
            1,
            "TAG: the gate's own tag bound to a root or loaded for one".into()
        ))
    );
}

// G10 (D-155): the gate composes the ICR only for an ordinary inter-kernel
// vector, `IKV_FIRST..=IKV_LAST`, toward a core named by its number; an
// NMI's vector, an exception's or a claimable one toward another node's
// core is refused.
#[test]
fn the_gate_sends_only_inter_kernel_vectors_to_other_cores() {
    use dodos_abi::x86_64::{IKV_FIRST, IKV_LAST};
    let ipi = |target, vector| Op::Ipi {
        n: 0,
        target,
        vector,
    };
    let mut m = machine(OldRules::default());
    // To a core of its node and to one of node 1 (cores 2 and 3).
    for vector in IKV_FIRST..=IKV_LAST {
        steps(&mut m, &[ipi(1, vector), ipi(2, vector)]).expect("an ordinary vector");
    }
    for vector in [2, 14, 18, 48, 239, IKV_FIRST - 1, IKV_LAST + 1, 255] {
        assert_eq!(m.step(&ipi(2, vector)), Err(Refused::Condition), "{vector}");
    }
    // No core of that number.
    assert_eq!(m.step(&ipi(4, IKV_FIRST)), Err(Refused::Condition));
    // The kernel's own interrupts are four of them.
    let own = [
        crate::Ipi::Reschedule,
        crate::Ipi::Leave,
        crate::Ipi::Invalidate,
        crate::Ipi::Ring,
    ]
    .map(crate::Ipi::vector);
    assert_eq!(own, [IKV_FIRST, IKV_FIRST + 1, IKV_FIRST + 2, IKV_LAST]);
    let mut old = machine(OldRules {
        any_vector_in_icr: true,
        ..OldRules::default()
    });
    assert_eq!(
        steps(&mut old, &[ipi(2, 2)]),
        Err((
            0,
            "G10: an ICR composed with a vector that is no inter-kernel vector".into()
        ))
    );
}

// D-154, F10: the ITS word goes *pinned* → 0 at a *failed* or *refused*
// answer to an ITS mapping, as at an unmapping's answer: the `its_table`
// is unpinned and can be destroyed. The identifier word stays taken, its
// attach word still *attached* (F9).
#[test]
fn a_failed_its_mapping_unpins_its_table() {
    use devices::Bound;
    let mut m = Machine::new(2, 2, 8).with_units();
    let ops = [
        Op::ItsRetype {
            n: 1,
            first: 8,
            k: 0,
        },
        Op::ItsClaim {
            n: 1,
            device: 1,
            first: 8,
            event: 0,
            entry: 1,
        },
    ];
    steps(&mut m, &ops).expect("the claim asked");
    assert_eq!(m.dev.its[1], (Bound::Pinned, 8, 0));
    assert_eq!(m.gate.classes[8], Class::ItsTable { pinned: true });
    let destroy = Op::ItsDestroy { n: 1, first: 8 };
    assert_eq!(m.step(&destroy), Err(Refused::Condition));
    steps(&mut m, &[Op::UnitFails, Op::Release { n: 1, entry: 1 }]).expect("answered failed");
    assert_eq!(m.dev.its[1], (Bound::Zero, 0, 0));
    assert_eq!(m.gate.classes[8], Class::ItsTable { pinned: false });
    assert!(m.dev.ids[1].is_some(), "its attach word is not 0");
    steps(&mut m, &[destroy]).expect("the its_table destroyed");
    assert_eq!(m.gate.classes[8], Class::Data);
}

// D-154, M3: on RISC-V an invalidation by address drops only leaf entries,
// so a change that unhooks a table invalidates the whole ASID on the
// caller and on every asked core. By address, core 1 keeps the non-leaf
// entry toward table 3 and walks it once it is *data* again.
#[test]
fn riscv_unhooks_a_table_with_a_whole_asid_fence() {
    let ops = [
        Op::Unhook {
            n: 0,
            p: 0,
            root: 2,
            tag: 1,
            e0: 0,
        },
        Op::Ack { n: 0, p: 1 },
        Op::TableFree { n: 0, page: 3 },
    ];
    let run = |old| {
        let mut m = machine(old);
        m.riscv = true;
        steps(&mut m, &ops)
    };
    assert_eq!(
        run(OldRules {
            riscv_unhook_by_address: true,
            ..OldRules::default()
        }),
        Err((
            2,
            "G: a core walks a cached table that is no table of its node".into()
        ))
    );
    run(OldRules::default()).expect("the whole-ASID fence");
    // Where an invalidation by address drops the cached tables too (x86-64,
    // ARM64), the range is enough.
    steps(&mut machine(OldRules::default()), &ops).expect("by address");
}

/// The first rule the faulty exploration breaks under `old`, over 200
/// seeds of 400 steps, if any.
fn explored(old: OldRules) -> Option<String> {
    for seed in 0..200u64 {
        let mut m = Machine::new(2, 2, 8).with_units().with_spaces();
        m.old = old;
        let mut rng = Rng(seed + 1);
        for _ in 0..400 {
            let op = m.random_op(&mut rng);
            let _ = m.step(&op);
            if let Err(rule) = m.check() {
                return Some(rule);
            }
        }
    }
    None
}

// The faulty kernels of the exploration try each of these attacks: under
// each earlier reading some seed breaks the checker, and none does under
// the tables (`faulty_kernels_keep_g_on_the_cpu_side`).
#[test]
fn the_faulty_exploration_finds_each_attack_of_pass_31() {
    let olds = [
        OldRules {
            tables_in_ordinary_chunks: true,
            ..OldRules::default()
        },
        OldRules {
            leaves_into_table_chunks: true,
            ..OldRules::default()
        },
        OldRules {
            anchors_over_table_chunks: true,
            ..OldRules::default()
        },
        OldRules {
            table_chunks_mapped_writable: true,
            ..OldRules::default()
        },
        OldRules {
            gate_tag_bound: true,
            ..OldRules::default()
        },
        OldRules {
            any_vector_in_icr: true,
            ..OldRules::default()
        },
    ];
    for old in olds {
        assert!(explored(old).is_some(), "{old:?} broke nothing");
    }
    assert_eq!(explored(OldRules::default()), None);
}
