//! The gate words, the page classes and the leaf counts of device entries
//! (review 7: DeepSeek F5, F6, F7, Hydrogen N4, Opus S3). A table page has
//! the class of its level (an iospace's, *iospace table*), and each chunk's
//! gate word counts its pages not of class *data*; a leaf of device memory
//! counts in its device entry and a leaf of the global read-only set in no
//! count, so no node writes the gate word of a chunk nobody owns.

mod common;

use dodos_abi::ObjectKind::{ASpace, Frame, IoSpace};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const BLOCK: u32 = 141;
const SPACE: u32 = 150;
const AT: u64 = 1 << 30;
const LIVING: SlotAddr = SlotAddr::own(START_SLOT_MACHINE_LIVING);

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

/// A call that succeeds, in as many steps of clearing as it needs.
#[track_caller]
fn ok(m: &mut Model, t: ObjId, call: Syscall) {
    let mut outcome = Err(Error::Again);
    for _ in 0..4096 {
        outcome = m.apply(t, call.clone());
        if outcome != Err(Error::Again) {
            break;
        }
    }
    assert_eq!(outcome, DONE);
    assert_eq!(m.check(), Ok(()));
}

fn retype(untyped: u32, kind: ObjectKind, page: PageSize, count: u64, at: u32) -> Syscall {
    let memory = match untyped {
        common::DEVICE => MemoryType::Uncached,
        _ => MemoryType::WriteBack,
    };
    Syscall::UntypedRetype {
        untyped: s(untyped),
        kind,
        size: count,
        page: page as u64,
        memory: memory as u64,
        destination: s(at),
    }
}

fn map(kind: ObjectKind, frame: SlotAddr, untyped: u32, address: u64, count: u64) -> Syscall {
    let map = Map {
        space: s(SPACE),
        frame,
        untyped: s(untyped),
        address,
        first: 0,
        count,
        rights: Rights::READ.bits(),
    };
    match kind {
        ASpace => Syscall::ASpaceMap(map),
        _ => Syscall::IoSpaceMap(map),
    }
}

/// A node with a block of one chunk in `BLOCK`, and a space of `kind` made
/// from it in `SPACE`: its root table is the block's first page.
fn with_space(kind: ObjectKind) -> (Model, ObjId, u64) {
    let (mut m, t) = Model::boot(&Machine::example()).unwrap();
    let log2 = m.geometry().chunk_shift();
    common::take_run(&mut m, t, log2, BLOCK);
    let base = m.frame_at(t, s(BLOCK)).map(|f| f.2);
    assert_eq!(base, None, "a block is no frame");
    ok(&mut m, t, retype(BLOCK, kind, PageSize::Page, 0, SPACE));
    let block = block_base(&m);
    (m, t, block)
}

/// The physical base of the block: the run `take_run` takes.
fn block_base(m: &Model) -> u64 {
    let geometry = m.geometry();
    let ram = Machine::example_on(geometry).memory[5].base;
    (ram + 8 * geometry.chunk()).next_multiple_of(geometry.chunk())
}

#[test]
fn table_pages_have_the_class_of_their_level_and_count_in_the_gate_word() {
    let (mut m, t, base) = with_space(ASpace);
    let geometry = m.geometry();
    let (page, levels) = (geometry.page(), geometry.levels);
    let chunk = base >> geometry.chunk_shift();
    // The root is a page table of level L; the gate word counts it.
    assert_eq!(m.page_class(base), PageClass::table(levels as u8).unwrap());
    assert_eq!(
        GateWord::from_raw(m.gate_word(chunk)).unwrap().not_data(),
        1
    );
    // A page of a frame mapped in it needs a table of each level below the
    // root, taken from the block after the space's two pages.
    ok(
        &mut m,
        t,
        retype(common::UNTYPED, Frame, PageSize::Page, 1, 160),
    );
    ok(&mut m, t, map(ASpace, s(160), BLOCK, AT, 1));
    let tables: Vec<PageClass> = (0..levels as u64 - 1)
        .map(|n| m.page_class(base + (2 + n) * page))
        .collect();
    let mut wanted: Vec<PageClass> = (1..levels as u8)
        .map(|level| PageClass::table(level).unwrap())
        .collect();
    let mut found = tables.clone();
    found.sort_by_key(|c| c.raw());
    wanted.sort_by_key(|c| c.raw());
    assert_eq!(found, wanted);
    let word = GateWord::from_raw(m.gate_word(chunk)).unwrap();
    assert_eq!(u32::from(word.not_data()), levels);
    // Unmapped, the tables go to the free list and keep their class.
    ok(&mut m, t, Syscall::ASpaceUnmap(s(160)));
    let again: Vec<PageClass> = (0..levels as u64 - 1)
        .map(|n| m.page_class(base + (2 + n) * page))
        .collect();
    assert_eq!(again, tables);
    // Destroyed, the space gives every table page back as data.
    ok(&mut m, t, Syscall::CapDelete(s(SPACE)));
    for n in 0..levels as u64 + 1 {
        assert_eq!(m.page_class(base + n * page), PageClass::DATA);
    }
    assert_eq!(
        GateWord::from_raw(m.gate_word(chunk)).unwrap().not_data(),
        0
    );
}

#[test]
fn a_table_taken_again_from_the_free_list_takes_the_class_of_its_new_level() {
    let (mut m, t, base) = with_space(ASpace);
    let geometry = m.geometry();
    let (page, levels) = (geometry.page(), geometry.levels);
    // A page, then a first large page elsewhere: the second needs no table
    // of level 1, so one free table is hooked at level 2 or above.
    ok(
        &mut m,
        t,
        retype(common::UNTYPED, Frame, PageSize::Page, 1, 160),
    );
    ok(&mut m, t, map(ASpace, s(160), BLOCK, AT, 1));
    ok(&mut m, t, Syscall::ASpaceUnmap(s(160)));
    let class_of = |m: &Model| -> Vec<u8> {
        (0..levels as u64 - 1)
            .map(|n| m.page_class(base + (2 + n) * page).raw())
            .collect()
    };
    let before = class_of(&m);
    let log2 = geometry.chunk_shift();
    let take = Syscall::MemoryTake {
        control: s(START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: base + 4 * geometry.chunk(),
        size: u64::from(log2),
        destination: s(142),
    };
    ok(&mut m, t, take);
    ok(&mut m, t, retype(142, Frame, PageSize::Large1, 1, 161));
    let far = AT + (1 << (log2 + geometry.entries.trailing_zeros()));
    ok(&mut m, t, map(ASpace, s(161), BLOCK, far, 1));
    let classes = class_of(&m);
    // What only the second map shows: a page of the free list that held a
    // table of level 1 holds one of level 2 now (the final review's F2
    // Opus 23: the classes of levels 2 to L − 1 were there before it).
    let (one, two) = (
        PageClass::table(1).unwrap().raw(),
        PageClass::table(2).unwrap().raw(),
    );
    let moved = before
        .iter()
        .zip(&classes)
        .any(|(&b, &a)| b == one && a == two);
    assert!(moved, "{before:?} then {classes:?}");
    assert_eq!(m.check(), Ok(()));
}

#[test]
fn an_iospace_root_is_of_class_root_with_0_devices_and_its_tables_of_their_level() {
    let (mut m, t, base) = with_space(IoSpace);
    let page = m.geometry().page();
    // D-137: the root counts its devices; each lower table has the class
    // of its level.
    assert_eq!(Some(m.page_class(base)), PageClass::iospace_root(0));
    ok(
        &mut m,
        t,
        retype(common::UNTYPED, Frame, PageSize::Page, 1, 160),
    );
    ok(&mut m, t, map(IoSpace, s(160), BLOCK, AT, 1));
    let level = m.page_class(base + 2 * page).iospace_level();
    assert!(level.is_some_and(|l| l >= 1), "{level:?}");
    let chunk = base >> m.geometry().chunk_shift();
    assert!(GateWord::from_raw(m.gate_word(chunk)).unwrap().not_data() >= 2);
}

#[test]
fn a_device_leaf_counts_in_its_device_entry_and_in_no_chunk() {
    for kind in [ASpace, IoSpace] {
        let (mut m, t, _) = with_space(kind);
        common::take_device(&mut m, t);
        let index = (0..m.device_count())
            .find(|&i| m.device_entry(i).base() == Machine::DEVICES)
            .unwrap();
        let counts = |m: &Model| -> Vec<u64> { (0..64).map(|c| m.map_count(c)).collect() };
        ok(
            &mut m,
            t,
            retype(common::DEVICE, Frame, PageSize::Page, 1, 160),
        );
        let before = counts(&m);
        ok(&mut m, t, map(kind, s(160), BLOCK, AT, 1));
        assert_eq!(counts(&m), before, "{kind:?}");
        assert_eq!(m.device_leaves(index), 1);
        let unmap = match kind {
            IoSpace => Syscall::IoSpaceUnmap(s(160)),
            _ => Syscall::ASpaceUnmap(s(160)),
        };
        ok(&mut m, t, unmap);
        assert_eq!(m.device_leaves(index), 0);
    }
}

#[test]
fn a_leaf_of_the_global_read_only_set_counts_nowhere() {
    let (mut m, t, _) = with_space(ASpace);
    let living = m.frame_at(t, LIVING).unwrap().2;
    let chunk = living >> m.geometry().chunk_shift();
    // At start-up already, nothing is counted in the chunk of the kept
    // memory, whose word is none.
    assert_eq!(m.ownership(chunk), Ownership::none().raw());
    assert_eq!(m.map_count(chunk), 0);
    let derive = Syscall::CapDerive {
        source: LIVING,
        destination: s(160),
        rights: Rights::READ.bits(),
        badge: 0,
    };
    ok(&mut m, t, derive);
    ok(&mut m, t, map(ASpace, s(160), BLOCK, AT, 1));
    assert_eq!(m.map_count(chunk), 0);
    assert!((0..m.device_count()).all(|i| m.device_leaves(i) == 0));
}

/// Hydrogen N4: two nodes that map their kept frames write no shared word,
/// so one node's leaves never make another node's mapping `busy`, even
/// with the smallest maximum of the map count.
#[test]
fn one_nodes_kept_leaves_never_make_another_nodes_mapping_busy() {
    let machine = Machine {
        map_count_max: 1,
        ..Machine::example()
    };
    let (mut sys, t) = System::boot(&machine).unwrap();
    for node in 0..2u16 {
        let retype = retype(25, ASpace, PageSize::Page, 0, SPACE);
        assert_eq!(sys.apply(node, t[node as usize], retype), DONE);
    }
    for (node, n) in [(0u16, 0u64), (0, 1), (0, 2), (1, 0), (1, 1)] {
        let at = 170 + n as u32;
        let derive = Syscall::CapDerive {
            source: LIVING,
            destination: s(at),
            rights: Rights::READ.bits(),
            badge: 0,
        };
        assert_eq!(sys.apply(node, t[node as usize], derive), DONE);
        let page = sys.node(node).geometry().page();
        let call = map(ASpace, s(at), 25, AT + n * page, 1);
        assert_eq!(sys.apply(node, t[node as usize], call), DONE);
        assert_eq!(sys.check(), Ok(()));
    }
}
