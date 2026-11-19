//! D-153, gap 29: the kernel's own view of a chunk, its split, the return to
//! writable and the merge (Gate words, kept kernel tables), executed with a
//! core faulting during the break-before-make, a page leaving *data* while
//! a request on its chunk is in flight, the split table's class, and a
//! stale writable entry on another core when a class byte changes. Each
//! scenario of a CONTRACT-GAP breaks under the letter of D-153 (an
//! `OldRules` switch) and holds under the reading the model takes.

use super::faulty::Op;
use super::*;

fn steps(m: &mut Machine, ops: &[Op]) -> Result<(), (usize, String)> {
    for (i, op) in ops.iter().enumerate() {
        m.step(op)
            .unwrap_or_else(|e| panic!("step {i}: {op:?} refused: {e:?}"));
        m.check().map_err(|rule| (i, rule))?;
    }
    Ok(())
}

/// Node 0 (cores 0 and 1) with chunk 4 (pages 16 to 19) taken, mapped by
/// its large leaf, and its user space on its floor: root 2 bound to tag 1,
/// table 3 hooked at entry 0, loaded on core 1.
fn machine(old: OldRules) -> Machine {
    let mut m = Machine::new(2, 2, 8);
    m.old = old;
    steps(
        &mut m,
        &[
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
            Op::Load {
                n: 0,
                p: 1,
                root: 2,
                tag: 1,
            },
        ],
    )
    .expect("the machine");
    m
}

fn table(page: Page, p: usize, split: Option<Page>) -> Op {
    Op::TakeTable {
        n: 0,
        p,
        page,
        split,
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

// The split, the return to writable and the merge, as the rows say. Core
// 1 holds the large leaf; core 0 splits the chunk to make page 16 a table,
// with page 17 as the split table. During the break-before-make core 0
// faults on the chunk (it waits and retries); core 1's stale large leaf
// still writes page 16, which is still *data*; at the completion the byte
// changes only once core 1 invalidated, so no stale writable entry reaches
// the table. Page 16 back to *data* is writable again; the chunk's count
// falls to the split table's own and the merge restores the large leaf.
#[test]
fn a_chunk_is_split_made_writable_again_and_merged() {
    let mut m = machine(OldRules::default());
    steps(&mut m, &[store(1, 16), table(16, 0, Some(17))]).expect("the split asked");
    assert_eq!(
        m.kernel_entry(0, 4).1,
        Entry::Absent,
        "the large leaf broken"
    );
    assert_eq!(
        m.step(&store(0, 18)),
        Err(Refused::Condition),
        "core 0 faults during the break-before-make"
    );
    steps(&mut m, &[store(1, 16), Op::Ack { n: 0, p: 1 }]).expect("the split");
    assert_eq!(m.kernel_entry(0, 4).1, Entry::Table(17));
    assert_eq!(
        m.gate.classes[17],
        Class::Table {
            level: 1,
            hook: Hook::Hooked
        },
        "the split table's own class: an aspace table of level 1"
    );
    assert_eq!(m.step(&store(1, 16)), Err(Refused::Condition), "read-only");
    assert_eq!(
        m.step(&store(0, 17)),
        Err(Refused::Condition),
        "the table itself"
    );
    steps(&mut m, &[store(0, 18), Op::TableFree { n: 0, page: 16 }]).expect("back to data");
    steps(&mut m, &[store(0, 16)]).expect("writable again");
    steps(
        &mut m,
        &[
            Op::Merge {
                n: 0,
                p: 0,
                chunk: 4,
            },
            Op::Ack { n: 0, p: 1 },
            store(1, 17),
        ],
    )
    .expect("the merge");
    assert_eq!(m.kernel_entry(0, 4).1, Entry::Large { chunk: 4 });
    assert_eq!(m.gate.classes[17], Class::Data);
}

// CONTRACT-GAP 40 (the page class byte, every row from *data*; kept kernel
// tables, the split): the row's conditions are read in the call, the byte
// written at the completion of the kernel-space request, and nothing holds
// them meanwhile. Core 0 asks page 16 to become a table; before core 1
// acknowledges, the node maps page 16 writable in its user space; the
// completion makes it a table under a writable user leaf.
#[test]
fn no_count_of_a_chunk_rises_while_its_pages_leave_data() {
    let map = Op::Map {
        n: 0,
        root: 2,
        tag: 1,
        va: (0, 0),
        page: 16,
        write: true,
    };
    let mut m = machine(OldRules {
        kernel_pages_unguarded: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut m,
        &[table(16, 0, Some(17)), map.clone(), Op::Ack { n: 0, p: 1 }],
    );
    assert_eq!(
        broken,
        Err((
            2,
            "G: a writable translation reaches a page that is not data".to_string()
        ))
    );
    let mut m = machine(OldRules::default());
    steps(&mut m, &[table(16, 0, Some(17))]).expect("asked");
    assert_eq!(m.step(&map), Err(Refused::Condition));
}

/// Chunk 4 split (page 17 the split table) with no other page left out of
/// *data*: ready for its merge.
fn split_alone(old: OldRules) -> Machine {
    let mut m = machine(old);
    steps(
        &mut m,
        &[
            table(16, 0, Some(17)),
            Op::Ack { n: 0, p: 1 },
            Op::TableFree { n: 0, page: 16 },
        ],
    )
    .expect("split, the page back to data");
    m
}

// CONTRACT-GAP 41 (kept kernel tables, the rows of the split and the
// merge): nothing says what a page leaving *data* does while a split or a
// merge of its chunk is in flight. Read as the rows go, the chunk has no
// large leaf and no hooked split table then, so nothing in the kernel's
// view needs changing and the byte changes at once: core 1, which the
// merge's request has not yet made invalidate, still holds the split
// table's writable leaf of the page, now a table (and the merge's
// completion would then write the large leaf, writable, over it).
#[test]
fn no_page_leaves_data_while_its_chunk_is_merged() {
    let merge = Op::Merge {
        n: 0,
        p: 0,
        chunk: 4,
    };
    let mut m = split_alone(OldRules {
        chunk_requests_unserialized: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut m,
        &[merge.clone(), table(18, 1, None), Op::Ack { n: 0, p: 1 }],
    );
    assert_eq!(
        broken,
        Err((
            1,
            "G: the kernel's own view writes a page that is not data".to_string()
        ))
    );
    let mut m = split_alone(OldRules::default());
    steps(&mut m, &[merge]).expect("the merge asked");
    assert_eq!(m.step(&table(18, 1, None)), Err(Refused::Condition));
    assert_eq!(m.step(&table(18, 1, Some(19))), Err(Refused::Condition));
}

// CONTRACT-GAP 42 (the ownership words, row *taken* → *free*; kept kernel
// tables, `memory_give`): the give reads the counts; while the chunk's split
// is in flight they read 0, the split table counted only at the completion.
// Core 0 splits chunk 4, core 1 gives it; the split's completion hooks a
// table of a chunk being given, whose completion leaves the kernel's view
// on memory node 1 now holds.
#[test]
fn no_give_of_a_chunk_whose_split_is_in_flight() {
    let give = Op::Give {
        n: 0,
        p: 1,
        chunk: 4,
        to: 1,
    };
    let mut m = machine(OldRules {
        give_during_split: true,
        ..OldRules::default()
    });
    let broken = steps(
        &mut m,
        &[
            table(16, 0, Some(17)),
            give.clone(),
            Op::Ack { n: 0, p: 1 },
            Op::Ack { n: 0, p: 0 },
        ],
    );
    assert_eq!(
        broken.map_err(|(_, rule)| rule),
        Err("G: the kernel's own view reaches memory its node does not hold".to_string())
    );
    let mut m = machine(OldRules::default());
    steps(&mut m, &[table(16, 0, Some(17))]).expect("asked");
    assert_eq!(m.step(&give), Err(Refused::Condition));
}

// CONTRACT-GAP 43 (G3 (3); kept kernel tables, the split table's class): G3
// forbids writing an entry toward a kernel table, not walking through one;
// a split table is a hooked table of class 16 + 1 below the kernel table,
// so a clearing call whose walk crosses the root's shared entry reaches
// it. Page 18 of the split chunk mapped in the user space; a clearing call
// through the kernel half clears page 18's kernel leaf and records it as a
// cleared leaf the chunk's map count holds, which it never did: the count
// is already below what it must hold, and the completion lowers it below
// the user leaf (G5's test is then wrong for the chunk).
#[test]
fn a_walk_never_crosses_a_shared_entry() {
    let ops = |m: &mut Machine| {
        steps(
            m,
            &[
                table(16, 0, Some(17)),
                Op::Ack { n: 0, p: 1 },
                Op::Map {
                    n: 0,
                    root: 2,
                    tag: 1,
                    va: (0, 1),
                    page: 18,
                    write: true,
                },
            ],
        )
    };
    let attack = Op::UnmapKernelHalf {
        n: 0,
        p: 0,
        root: 2,
        tag: 1,
        slot: 0,
        index: 2,
    };
    let mut m = machine(OldRules {
        walks_through_shared: true,
        ..OldRules::default()
    });
    ops(&mut m).expect("the split and the user leaf");
    let broken = steps(&mut m, std::slice::from_ref(&attack));
    assert_eq!(
        broken,
        Err((
            0,
            "COUNT: a map count below the leaves it holds".to_string()
        ))
    );
    let mut m = machine(OldRules::default());
    ops(&mut m).expect("the split and the user leaf");
    assert_eq!(m.step(&attack), Err(Refused::Condition));
}

// CONTRACT-GAP 44 (kept kernel tables, the split row: "a *loose* level-1
// table (class 16 + 1) drawn from a *data* page of the same chunk that the
// kernel names"): the row gives the page no condition. It is *loose* and
// drawn from a *data* page at once, though only the split can make a page
// of a chunk under its large leaf leave *data*; nothing says it differs
// from the pages the call turns from *data*, nor that no anchor covers it.
// The model reads: a *data* page of the chunk, none of the pages leaving,
// held, unmapped, under no anchor, turned into the split table at the
// completion.
#[test]
fn the_split_table_is_another_data_page_of_the_chunk() {
    let mut m = machine(OldRules::default());
    for split in [None, Some(16), Some(20), Some(3)] {
        assert_eq!(
            m.step(&table(16, 0, split)),
            Err(Refused::Condition),
            "{split:?}"
        );
    }
    steps(&mut m, &[table(16, 0, Some(19)), Op::Ack { n: 0, p: 1 }]).expect("the split");
}
