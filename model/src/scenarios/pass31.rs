//! Pass 31, D-154 and D-155. D-154: dedicated table chunks replace D-153's
//! split and merge of the kernel's large leaf. A table untyped
//! (`memory_take` kind 2, or cut from a node's table floor) has no clean
//! mark and no `exec`, and makes only a child table untyped or an
//! `its_table`; an `aspace` and an `iospace` take a one-page header from an
//! ordinary untyped and their root table and lower tables from the table
//! untyped their size argument names; the chunks of a table untyped carry
//! the table bit, are mapped read-only by the kernel, and are the target of
//! no leaf (G3) and no grant's anchor (G6). D-155: `DR7.GD` refused.

use super::pass17::{fresh, kill, resume, seal};
use super::pass22::take;
use super::*;
use crate::Map;
use dodos_abi::{
    GateWord, MEMORY_TAKE_RUN, MEMORY_TAKE_TABLES, MemberState, Owner, Ownership, PageClass,
};

const BLOCK: u32 = 200;
const TABLES: u32 = 201;
const SPACE: u32 = 210;

/// The first chunk no node holds and no reserve names, from `after` on.
fn free_chunk(model: &Model, after: u64) -> u64 {
    let free = Ownership::free(Owner::Unclaimed).raw();
    (after..)
        .find(|&chunk| model.ownership(chunk) == free)
        .expect("a free chunk")
}

/// A node alone with a table block of one chunk in `TABLES` and an
/// ordinary block of one chunk in `BLOCK`; their chunks.
fn with_blocks() -> (N, u64, u64) {
    // Two node slots of one core each: node 0 runs beside a phantom.
    let mut k = N::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let shift = u64::from(k.m.geometry().chunk_shift());
    let tables = free_chunk(&k.m, 0);
    k.ok(take(MEMORY_TAKE_TABLES, tables << shift, shift, TABLES));
    let block = free_chunk(&k.m, 0);
    k.ok(take(MEMORY_TAKE_RUN, block << shift, shift, BLOCK));
    (k, tables, block)
}

fn word(k: &N, chunk: u64) -> GateWord {
    GateWord::from_raw(k.m.gate_word(chunk)).expect("a gate word")
}

/// An `aspace` in `slot`: its header from `BLOCK`, its root from `tables`.
fn aspace(tables: u32, slot: u32) -> Syscall {
    retype_from(BLOCK, ObjectKind::ASpace, s(tables).raw(), slot)
}

fn map(space: u32, frame: u32, untyped: u32, address: u64) -> Syscall {
    Syscall::ASpaceMap(Map {
        space: s(space),
        frame: s(frame),
        untyped: s(untyped),
        address,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    })
}

// `memory_take` kind 2: the run mapped read-only with large entries, the
// whole gate word written with the table bit and its counts 0; a table
// untyped, with no clean mark and no `exec` whatever the memory_control
// has. Kind 0 writes the whole word 0.
#[test]
fn memory_take_kind_2_makes_a_table_untyped_over_table_chunks() {
    let (mut k, tables, block) = with_blocks();
    assert_eq!(word(&k, tables), GateWord::TABLE);
    assert_eq!(word(&k, block), GateWord::ORDINARY);
    assert!(k.m.table_chunk(tables) && k.m.kernel_maps_read_only(tables));
    assert!(!k.m.table_chunk(block) && !k.m.kernel_maps_read_only(block));
    assert_eq!(k.m.untyped_is_table(k.t, s(TABLES)), Some(true));
    assert_eq!(k.m.untyped_is_table(k.t, s(BLOCK)), Some(false));
    // No clean mark: watermark and clean mark at the start.
    assert_eq!(k.m.untyped_marks(k.t, s(TABLES)), Some((0, 0, false)));
    let rights = |k: &mut N, slot| match k.call(k.t, Syscall::CapInspect(s(slot))) {
        Ok(Output::Inspect { rights, .. }) => rights,
        other => panic!("{other:?}"),
    };
    assert_eq!(rights(&mut k, TABLES), Rights::ALL.without(Rights::EXEC));
    assert_eq!(rights(&mut k, BLOCK), Rights::ALL);
    // The kind is 0, 1 or 2.
    let shift = u64::from(k.m.geometry().chunk_shift());
    let free = free_chunk(&k.m, 0) << shift;
    k.no(take(3, free, shift, 202), Error::Invalid);
    // A table chunk is taken once: `busy` for this generation.
    k.no(
        take(MEMORY_TAKE_RUN, tables << shift, shift, 202),
        Error::Busy,
    );
}

// A table untyped makes only a child table untyped or an `its_table`; an
// `its_table` is `invalid` from an ordinary or a device untyped. The gate
// clears its pages in the call (no `again`) and turns them *ITS table*.
#[test]
fn a_table_untyped_makes_only_a_child_table_untyped_or_an_its_table() {
    let (mut k, tables, _) = with_blocks();
    let page = k.m.geometry().page();
    for (kind, size) in [
        (ObjectKind::Frame, 1),
        (ObjectKind::Endpoint, 0),
        (ObjectKind::CSpace, 128),
        (ObjectKind::Thread, 0),
        (ObjectKind::ASpace, s(TABLES).raw()),
        (ObjectKind::IoSpace, s(TABLES).raw()),
    ] {
        k.no(retype_from(TABLES, kind, size, 220), Error::Invalid);
    }
    // The reserved kind is `unsupported` after the rule on table untypeds.
    k.no(
        retype_from(TABLES, ObjectKind::Vcpu, 0, 220),
        Error::Invalid,
    );
    k.ok(retype_from(TABLES, ObjectKind::Untyped, 4 * page, 220));
    assert_eq!(k.m.untyped_is_table(k.t, s(220)), Some(true));
    // From the child too; k from 0 to 4, above `invalid`.
    for (slot, k_arg) in (221..).zip(0..=4u64) {
        let from = if k_arg <= 1 { 220 } else { TABLES };
        assert_eq!(
            k.call(k.t, retype_from(from, ObjectKind::ItsTable, k_arg, slot)),
            DONE,
            "k {k_arg}"
        );
    }
    k.no(
        retype_from(TABLES, ObjectKind::ItsTable, 5, 230),
        Error::Invalid,
    );
    k.no(
        retype_from(BLOCK, ObjectKind::ItsTable, 0, 230),
        Error::Invalid,
    );
    k.no(k.retype(ObjectKind::ItsTable, 0, 230), Error::Invalid);
    // 1 + 2 + 4 + 8 + 16 pages of class *ITS table*, counted in the word.
    assert_eq!(word(&k, tables).not_data(), 31);
    let base = tables << k.m.geometry().chunk_shift();
    assert_eq!(k.m.page_class(base), PageClass::its_table(false));
    // Destroyed, its pages turn back into *data*.
    for slot in 221..226 {
        k.ok(Syscall::CapDelete(s(slot)));
    }
    assert_eq!(word(&k, tables), GateWord::TABLE);
}

// An `aspace`: one page of header from the ordinary untyped, one page of
// root table from the table untyped the size argument names, lower tables
// from that table untyped at a map; all given back at its destruction.
#[test]
fn an_aspace_takes_its_header_and_its_tables_from_two_untypeds() {
    let (mut k, tables, block) = with_blocks();
    let page = k.m.geometry().page();
    let levels = u64::from(k.m.geometry().levels);
    k.ok(aspace(TABLES, SPACE));
    let mark = |k: &N, slot| k.m.untyped_marks(k.t, s(slot)).expect("an untyped").0;
    assert_eq!((mark(&k, BLOCK), mark(&k, TABLES)), (page, page));
    let shift = k.m.geometry().chunk_shift();
    let root = k.m.root_table(k.t, s(SPACE)).expect("a root");
    assert_eq!(root, tables << shift);
    assert_eq!(
        k.m.page_class(root),
        PageClass::table(levels as u8).unwrap()
    );
    assert_eq!(word(&k, tables).not_data(), 1);
    assert_eq!(word(&k, block).not_data(), 0);
    // A map takes L − 1 tables from the table untyped, none elsewhere.
    k.ok(retype_from(BLOCK, ObjectKind::Frame, 1, 211));
    let at = 1 << 30;
    // The ordinary untyped the header came from is not its table untyped.
    k.no(map(SPACE, 211, BLOCK, at), Error::Invalid);
    k.ok(map(SPACE, 211, TABLES, at));
    assert_eq!(mark(&k, TABLES), levels * page);
    assert_eq!(word(&k, tables).not_data(), levels as u16);
    assert_eq!(word(&k, tables).map_count(), 0);
    assert_eq!(word(&k, block).map_count(), 1);
    // Destroyed: the header back to the ordinary untyped, the root and the
    // tables to the table untyped, whose pages are *data* again.
    k.ok(Syscall::CapDelete(s(211)));
    k.ok(Syscall::CapDelete(s(SPACE)));
    k.interrupts();
    assert_eq!(word(&k, tables), GateWord::TABLE);
    k.ok(aspace(TABLES, SPACE));
    assert_eq!(k.m.root_table(k.t, s(SPACE)), Some(root));
}

// The size argument of an `aspace` is judged as a capability argument:
// its slot, its kind, that it is a table untyped, `write`.
#[test]
fn the_size_argument_of_a_space_is_a_table_untyped_with_write() {
    let (mut k, _, _) = with_blocks();
    k.ok(retype_from(BLOCK, ObjectKind::Endpoint, 0, 211));
    k.ok(Syscall::CapDerive {
        source: s(TABLES),
        destination: s(212),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let before = k.m.untyped_marks(k.t, s(BLOCK));
    for kind in [ObjectKind::ASpace, ObjectKind::IoSpace] {
        // No slot, an empty slot, an endpoint, an ordinary untyped.
        for size in [0, s(213).raw(), s(211).raw(), s(BLOCK).raw()] {
            k.no(retype_from(BLOCK, kind, size, SPACE), Error::Invalid);
        }
        // A table untyped without `write`.
        k.no(retype_from(BLOCK, kind, s(212).raw(), SPACE), Error::Denied);
    }
    // Nothing was taken by a refused call.
    assert_eq!(k.m.untyped_marks(k.t, s(BLOCK)), before);
    k.ok(aspace(TABLES, SPACE));
}

// A root table that would end beyond the table untyped is `no_memory`
// with nothing changed: the header is not taken from the ordinary untyped.
#[test]
fn a_root_beyond_the_table_untyped_is_no_memory_with_nothing_taken() {
    let (mut k, _, _) = with_blocks();
    let page = k.m.geometry().page();
    k.ok(retype_from(TABLES, ObjectKind::Untyped, page, 220));
    k.ok(aspace(220, SPACE));
    let before = k.m.untyped_marks(k.t, s(BLOCK));
    k.no(aspace(220, SPACE + 1), Error::NoMemory);
    assert_eq!(k.m.untyped_marks(k.t, s(BLOCK)), before);
    // A map that needs a table finds none in it either.
    k.ok(retype_from(BLOCK, ObjectKind::Frame, 1, 211));
    k.no(map(SPACE, 211, 220, 1 << 30), Error::NoMemory);
}

// A table untyped whose pieces belong to a surviving space waits for it:
// `again` at its removal, `busy` at a give of its block.
#[test]
fn a_table_untyped_waits_for_the_spaces_whose_tables_it_holds() {
    let (mut k, _, _) = with_blocks();
    let page = k.m.geometry().page();
    k.ok(retype_from(TABLES, ObjectKind::Untyped, 4 * page, 220));
    k.ok(aspace(220, SPACE));
    for _ in 0..3 {
        k.no(Syscall::CapDelete(s(220)), Error::Again);
    }
    let give = Syscall::MemoryGive {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        untyped: s(TABLES),
        node: 1,
    };
    k.no(give.clone(), Error::Busy);
    k.ok(Syscall::CapDelete(s(SPACE)));
    k.interrupts();
    // The child was dying: its removal ends now, and the block is given.
    let mut outcome = k.call(k.t, Syscall::CapDelete(s(220)));
    for _ in 0..8 {
        if outcome != Err(Error::Again) {
            break;
        }
        k.interrupts();
        outcome = k.call(k.t, Syscall::CapDelete(s(220)));
    }
    assert_eq!(outcome, DONE);
    let mut outcome = k.call(k.t, give.clone());
    for _ in 0..8 {
        if !matches!(outcome, Err(Error::Again | Error::Busy)) {
            break;
        }
        k.interrupts();
        outcome = k.call(k.t, give.clone());
    }
    assert!(
        matches!(outcome, Ok(Output::Done | Output::Waits)),
        "{outcome:?}"
    );
}

// A faulty kernel's own gate: no leaf and no anchor toward a table chunk
// (G3, G6), no table in an ordinary chunk (G5's row from *data*), no give
// of a table chunk that holds a table (G5).
#[test]
fn a_faulty_kernels_gate_keeps_tables_in_table_chunks() {
    let (mut k, tables, block) = with_blocks();
    let geometry = k.m.geometry();
    let pages = geometry.chunk() / geometry.page();
    let (in_tables, in_block) = (tables * pages + 3, block * pages + 3);
    let class = PageClass::table(1).expect("a class");
    assert!(!k.m.gate_leaf_allowed(in_tables) && !k.m.gate_anchor_allowed(in_tables));
    assert!(k.m.gate_leaf_allowed(in_block) && k.m.gate_anchor_allowed(in_block));
    assert!(!k.m.gate_retype_page(in_block, class));
    // Its table floor refuses a leaf too, and its ordinary floor a table.
    let floor = k.m.table_floor(0)[0].0 / geometry.page();
    assert!(!k.m.gate_leaf_allowed(floor + pages - 1));
    assert!(!k.m.gate_retype_page(floor - 1, class));
    assert_eq!(k.m.check(), Ok(()));
    // A table chunk that holds a root is not given.
    k.ok(aspace(TABLES, SPACE));
    assert!(!k.m.gate_give_chunk(tables, 1));
    assert_eq!(k.m.check(), Ok(()));
    // A *data* page of a table chunk the node holds does become a table.
    assert!(k.m.gate_retype_page(in_tables, class));
    assert_eq!(k.m.page_class(in_tables * geometry.page()), class);
}

// The node's table floor at start-up: at least one chunk with the table
// bit, mapped read-only, holding the start-up aspace's root and tables;
// its first untyped is a table untyped whose watermark stands after them.
#[test]
fn the_start_up_aspace_lies_in_the_table_floor() {
    let k = N::on(&one_node(Arch::X86_64, 1));
    let geometry = k.m.geometry();
    let (page, shift) = (geometry.page(), geometry.chunk_shift());
    let floor = k.m.table_floor(0);
    assert!(!floor.is_empty());
    let (start, end) = floor[0];
    for chunk in start >> shift..end >> shift {
        assert!(k.m.table_chunk(chunk) && k.m.kernel_maps_read_only(chunk));
        assert_eq!(word(&k, chunk).map_count(), 0);
    }
    let root =
        k.m.root_table(k.t, s(dodos_abi::START_SLOT_ASPACE))
            .expect("the start-up root");
    assert_eq!(root, start);
    let levels = geometry.levels as u8;
    assert_eq!(k.m.page_class(root), PageClass::table(levels).unwrap());
    // The root, the tables its mappings need and the reserve of free
    // tables, and nothing else that is not *data*.
    let tables = k.tables();
    let (watermark, clean, _) = k.m.untyped_marks(k.t, s(tables)).expect("its untyped");
    assert_eq!(clean, 0);
    assert_eq!(
        u64::from(word(&k, start >> shift).not_data()),
        watermark / page
    );
    assert!(watermark / page > k.m.machine.reserve_tables);
    // Every chunk that holds a page that is not *data* is a table chunk.
    for chunk in 0..k.m.table.gate_words.len() as u64 {
        if k.m.gate_word(chunk) >> 32 & 0x1FFF != 0 {
            assert!(k.m.table_chunk(chunk), "chunk {chunk}");
        }
    }
}

// A table floor too small for the start-up aspace's root and tables, or
// none, is a refusal of the start-up.
#[test]
fn a_boot_image_without_a_table_floor_is_refused() {
    let mut machine = Machine::example();
    machine.floors.floor_tables = 0;
    assert_eq!(Model::boot(&machine).err(), Some(crate::Refusal::Floors));
    // One chunk is the least; two chunks are two table chunks.
    machine.floors.floor_tables = machine.geometry.chunk() + 1;
    let (model, _) = Model::boot(&machine).expect("two chunks");
    let chunk = machine.geometry.chunk();
    let size: u64 = model.table_floor(0).iter().map(|(s, e)| e - s).sum();
    assert_eq!(size, 2 * chunk);
    assert_eq!(model.check(), Ok(()));
    // A reserve of free tables the floor cannot hold.
    let mut machine = Machine::example();
    machine.reserve_tables = machine.geometry.chunk() / machine.geometry.page();
    assert_eq!(Model::boot(&machine).err(), Some(crate::Refusal::Floors));
}

/// Two nodes; node 1 holds a table block with an `aspace` whose root lies
/// in it. The block's chunk.
fn second_node_with_tables() -> (S, u64) {
    let mut m = S::two();
    let shift = u64::from(m.sys.canonical().geometry.chunk_shift());
    let chunk = free_chunk(m.sys.node(1), 0);
    m.ok(1, take(MEMORY_TAKE_TABLES, chunk << shift, shift, TABLES));
    let ut = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(
        1,
        retype_from(ut, ObjectKind::ASpace, s(TABLES).raw(), SPACE),
    );
    assert!(m.sys.canonical().table_chunk(chunk));
    assert_eq!(m.sys.canonical().gate_word(chunk).not_data(), 1);
    (m, chunk)
}

/// The chunks of node 1's table floor.
fn table_floor_chunks(m: &S) -> Vec<u64> {
    let table = m.sys.canonical();
    let (chunk, shift) = (table.geometry.chunk(), table.geometry.chunk_shift());
    table.table_floors[1]
        .iter()
        .flat_map(|&(s, e)| (s..e).step_by(chunk as usize))
        .map(|at| at >> shift)
        .collect()
}

// R6 clears the whole gate word of the dead node's chunks and floors, the
// table bit with the counts; a fresh start (G13) writes its table floor's
// words with the table bit again and builds its start-up aspace there.
#[test]
fn r6_clears_the_table_bit_and_a_fresh_start_writes_its_table_floor() {
    let (mut m, chunk) = second_node_with_tables();
    let floor = table_floor_chunks(&m);
    assert!(floor.iter().all(|&c| m.sys.canonical().table_chunk(c)));
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    let table = m.sys.canonical();
    for &c in floor.iter().chain([&chunk]) {
        assert_eq!(table.gate_word(c), GateWord::ORDINARY, "chunk {c}");
    }
    let pages = table.geometry.chunk() / table.geometry.page();
    assert!(
        table
            .classes
            .range(chunk * pages..(chunk + 1) * pages)
            .next()
            .is_none()
    );
    // Node 0 takes the dead generation's chunk as an ordinary one.
    let shift = u64::from(table.geometry.chunk_shift());
    m.ok(0, take(MEMORY_TAKE_RUN, chunk << shift, shift, BLOCK));
    assert!(!m.sys.canonical().table_chunk(chunk));
    m.ok(0, fresh(1, 0b10));
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    let table = m.sys.canonical();
    for &c in &floor {
        let word = table.gate_word(c);
        assert!(word.is_table() && word.not_data() > 0, "chunk {c}");
        assert!(m.sys.node(1).kernel_maps_read_only(c));
    }
    assert_eq!(m.sys.check(), Ok(()));
}

// A seal and a resume leave a node's table chunks as they are: their bit,
// their tables, their read-only leaf; the resumed node maps with them.
#[test]
fn a_sealed_node_keeps_its_table_chunks_through_its_resume() {
    let (mut m, chunk) = second_node_with_tables();
    let before = (m.sys.canonical().gate_word(chunk), table_floor_chunks(&m));
    m.ok(0, seal(1));
    m.run(2 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    assert_eq!(m.sys.canonical().gate_word(chunk), before.0);
    assert!(before.1.iter().all(|&c| m.sys.canonical().table_chunk(c)));
    m.ok(0, resume(1));
    m.run(3 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.canonical().gate_word(chunk), before.0);
    assert!(m.sys.node(1).kernel_maps_read_only(chunk));
    let ut = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(1, retype_from(ut, ObjectKind::Frame, 1, 211));
    m.ok(1, map(SPACE, 211, TABLES, 1 << 30));
    let word = m.sys.canonical().gate_word(chunk);
    assert!(word.is_table() && word.map_count() == 0 && word.not_data() > 1);
}

// The checker's rules of D-154, each fed a state that breaks it alone.

#[track_caller]
fn reported(k: &N, message: &str) {
    assert_eq!(k.m.check(), Err(message));
}

#[test]
fn a_table_chunk_that_counts_a_grant_is_reported() {
    let (mut k, tables, _) = with_blocks();
    let word = word(&k, tables).with_grants(1).unwrap();
    k.m.table.write_gate_word(tables, word);
    reported(&k, "TABLES: a gate word no gate writes");
}

#[test]
fn a_table_chunk_the_kernel_maps_writable_is_reported() {
    let (mut k, tables, _) = with_blocks();
    k.m.kernel_read_only.remove(&tables);
    reported(&k, "TABLES: the kernel's view of a table chunk");
    // And an ordinary chunk mapped read-only.
    let (mut k, _, block) = with_blocks();
    k.m.kernel_read_only.insert(block);
    reported(&k, "TABLES: the kernel's view of a table chunk");
}

#[test]
fn exec_on_a_table_untyped_is_reported() {
    let (mut k, _, _) = with_blocks();
    let object = k.m.object_at(k.t, s(k.tables())).expect("the untyped");
    let root = k.m.objects[&object].root;
    crate::live(&mut k.m.caps, root).rights = Rights::ALL;
    reported(&k, "TABLES: exec on a table untyped or on what it made");
}

#[test]
fn a_root_table_outside_its_table_untyped_is_reported() {
    let (mut k, _, _) = with_blocks();
    k.ok(aspace(TABLES, SPACE));
    k.ok(aspace(TABLES, SPACE + 1));
    // The space says its tables come from the table floor's untyped.
    let space = k.m.object_at(k.t, s(SPACE)).expect("the space");
    let (block, floor) = (
        k.m.object_at(k.t, s(TABLES)).expect("the block"),
        k.m.object_at(k.t, s(k.tables())).expect("the floor's"),
    );
    crate::live(&mut k.m.aspaces, space).table_untyped = Some(floor);
    crate::live(&mut k.m.untypeds, block).live -= 1;
    crate::live(&mut k.m.untypeds, floor).live += 1;
    reported(&k, "TABLES: a table of a space outside its table untyped");
}

#[test]
fn an_endpoint_made_from_a_table_untyped_is_reported() {
    let (mut k, _, _) = with_blocks();
    k.ok(retype_from(BLOCK, ObjectKind::Endpoint, 0, 211));
    let endpoint = k.m.object_at(k.t, s(211)).expect("the endpoint");
    let (block, tables) = (
        k.m.object_at(k.t, s(BLOCK)).expect("the block"),
        k.m.object_at(k.t, s(TABLES)).expect("the table block"),
    );
    // As if the table untyped had made it.
    let root = k.m.objects[&endpoint].root;
    let parent = k.m.objects[&tables].root;
    let old = k.m.caps[&root].parent.expect("its parent");
    crate::live(&mut k.m.caps, old)
        .children
        .retain(|&c| c != root);
    crate::live(&mut k.m.caps, parent).children.push(root);
    crate::live(&mut k.m.caps, root).parent = Some(parent);
    crate::live(&mut k.m.caps, root).rights = Rights::ALL.without(Rights::EXEC);
    crate::live(&mut k.m.objects, endpoint).from = Some(tables);
    crate::live(&mut k.m.untypeds, block).live -= 1;
    crate::live(&mut k.m.untypeds, tables).live += 1;
    let untyped = crate::live(&mut k.m.untypeds, block);
    (untyped.watermark, untyped.clean) = (0, 0);
    untyped.zero.clear();
    crate::live(&mut k.m.untypeds, tables).watermark = 4096;
    reported(&k, "TABLES: a kind a table untyped does not make");
}

// D-155: `DR7.GD = 1` (bit 13, general detect) is `invalid` in a register
// write, as an I/O condition is, with an otherwise valid control word.
#[test]
fn a_register_write_with_general_detect_is_invalid() {
    use dodos_abi::x86_64::{DR7_GD, REG_DR0, REG_DR7, REG_RFLAGS, REGS_WORDS, RFLAGS_FIXED};
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.ok(k.retype(ObjectKind::Thread, 0, 211));
    let write = |dr7: u64| {
        let mut block = vec![0; REGS_WORDS];
        block[REG_RFLAGS] = RFLAGS_FIXED;
        block[REG_DR0] = 0x1000;
        block[REG_DR7] = dr7;
        Syscall::ThreadWriteRegs {
            thread: s(211),
            block,
        }
    };
    let watch = 1 | 0b01 << 16 | 0b11 << 18;
    k.ok(write(watch));
    k.no(write(watch | DR7_GD), Error::Invalid);
    k.no(write(DR7_GD), Error::Invalid);
    k.no(write(watch | 0b10 << 20), Error::Invalid);
}

// D-154, F14: once the role word of any IOMMU unit reads *failed*, R6
// retires every handle of the dead node's cores, present or not, so no
// vector of those cores is claimed again before the machine restarts.
#[test]
fn r6_retires_every_handle_of_a_dead_nodes_cores_once_a_unit_failed() {
    use super::units::IOMMU_ROLE;
    let handles = |m: &S, core: u64| {
        crate::irq::handles_of_core(core)
            .filter(|h| m.sys.canonical().retired.contains(h))
            .count()
    };
    // No unit failed: a death retires nothing.
    let mut healthy = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    healthy.ok(0, kill(1));
    healthy.run(10 * MS);
    assert_eq!(healthy.state(1), MemberState::Dead);
    assert_eq!(handles(&healthy, 1), 0);
    // The IOMMU unit fails first: node 1 claimed no vector, and all 192
    // handles of its core are retired; no other core's.
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, SPACE);
    m.ok(0, iospace);
    m.sys.break_unit(IOMMU_ROLE);
    let call = super::units::attach(&m, 0, SPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert!(m.sys.role(IOMMU_ROLE).failed());
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert_eq!(handles(&m, 1), dodos_abi::x86_64::IRQ_TABLE_ENTRIES);
    assert_eq!(handles(&m, 0) + handles(&m, 2), 0);
    assert_eq!(m.sys.check(), Ok(()));
}

// D-156, the review of pass 31.

const INNER: u32 = 230;

/// A slot of the capability space in `INNER`.
fn via(index: u32) -> SlotAddr {
    SlotAddr::through(INNER, index)
}

fn retype_at(untyped: SlotAddr, kind: ObjectKind, size: u64, destination: SlotAddr) -> Syscall {
    Syscall::UntypedRetype {
        untyped,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination,
    }
}

/// `with_blocks`, and a capability space of 128 slots in `INNER` whose
/// slot 5 holds the root capability of a child of the table block; the
/// space.
fn with_inner_tables() -> (N, u64, ObjId) {
    let (mut k, tables, _) = with_blocks();
    let page = k.m.geometry().page();
    k.ok(retype_from(BLOCK, ObjectKind::CSpace, 128, INNER));
    let grow = Syscall::CSpaceGrow {
        cspace: s(INNER),
        untyped: s(BLOCK),
    };
    assert_eq!(k.call(k.t, grow), Ok(Output::Slots(128)));
    let child = retype_at(s(TABLES), ObjectKind::Untyped, 4 * page, via(5));
    k.ok(child);
    let inner = k.m.object_at(k.t, s(INNER)).expect("a cspace");
    (k, tables, inner)
}

/// Deletes the capability in `slot` in at most 16 calls.
fn deleted(k: &mut N, slot: u32) -> Outcome {
    let mut outcome = Err(Error::Again);
    for _ in 0..16 {
        outcome = k.call(k.t, Syscall::CapDelete(s(slot)));
        if outcome != Err(Error::Again) {
            break;
        }
        k.interrupts();
    }
    outcome
}

// The bounded sweep in two passes: a dying `cspace` holds the root
// capability of a table untyped in a slot below the `aspace` that names it.
// Pass 1 defers the slot and removes the higher ones, the `aspace` among
// them; pass 2 removes the table untyped (The derivation tree, D-156).
#[test]
fn a_dying_cspace_removes_a_space_above_the_table_untyped_it_names() {
    let (mut k, tables, inner) = with_inner_tables();
    k.ok(retype_at(
        s(BLOCK),
        ObjectKind::ASpace,
        via(5).raw(),
        via(9),
    ));
    // More capabilities than one call removes, above the deferred slot.
    for index in 10..50 {
        k.ok(retype_at(s(BLOCK), ObjectKind::Endpoint, 0, via(index)));
    }
    k.no(Syscall::CapDelete(s(INNER)), Error::Again);
    // The call ended in pass 1, past the deferred slot, which is still
    // full: the checker accepts it there (invariant 27).
    let space = &k.m.cspaces[&inner];
    let sweep = space.sweep.as_ref().expect("a dying space");
    assert_eq!((sweep.first_deferred, sweep.pass2), (Some(5), false));
    assert!(sweep.next > 9 && matches!(space.slots[5], crate::Slot::Full(_)));
    assert_eq!(deleted(&mut k, INNER), DONE);
    // Everything went back: the table block's watermark returned.
    k.interrupts();
    k.ok(aspace(TABLES, SPACE));
    let shift = k.m.geometry().chunk_shift();
    assert_eq!(k.m.root_table(k.t, s(SPACE)), Some(tables << shift));
}

// A space outside the dying `cspace` names the table untyped: pass 1 defers
// its slot and removes the higher ones; pass 2 defers nothing and is
// `again` on that slot until the space outside is destroyed.
#[test]
fn a_dying_cspace_waits_in_pass_2_for_a_space_outside_it() {
    let (mut k, _, inner) = with_inner_tables();
    k.ok(retype_at(
        s(BLOCK),
        ObjectKind::ASpace,
        via(5).raw(),
        s(SPACE),
    ));
    for index in [9, 10] {
        k.ok(retype_at(s(BLOCK), ObjectKind::Endpoint, 0, via(index)));
    }
    for _ in 0..3 {
        k.no(Syscall::CapDelete(s(INNER)), Error::Again);
        let space = &k.m.cspaces[&inner];
        let sweep = space.sweep.as_ref().expect("a dying space");
        assert_eq!(
            (sweep.next, sweep.first_deferred, sweep.pass2),
            (5, None, true)
        );
        let full = |index: usize| matches!(space.slots[index], crate::Slot::Full(_));
        assert!(full(5) && !full(9) && !full(10));
    }
    k.ok(Syscall::CapDelete(s(SPACE)));
    k.interrupts();
    assert_eq!(deleted(&mut k, INNER), DONE);
}

// A child table untyped destroyed with pieces lost behind a failed unit
// stays a lost piece of its parent: each ancestor is deletable, and no
// ancestor's watermark returns over the pages the unit keeps (Destruction
// of an address space, D-137, D-156).
#[test]
fn a_table_untyped_destroyed_with_lost_pages_holds_the_watermarks_above_it() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    let floor = super::table_slot(m.sys.node(0), m.t[0]);
    let page = m.sys.node(0).geometry().page();
    // The table floor, then 160, 161 and 162, each a child of the last.
    for (from, slot, pages) in [(floor, 160, 16), (160, 161, 8), (161, 162, 4)] {
        m.ok(
            0,
            retype_from(from, ObjectKind::Untyped, pages * page, slot),
        );
    }
    m.ok(
        0,
        retype_from(ut, ObjectKind::IoSpace, s(162).raw(), IOSPACE),
    );
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    let root = m
        .sys
        .node(0)
        .root_table(m.t[0], s(IOSPACE))
        .expect("a root");
    m.sys.break_unit(IOMMU_ROLE);
    let deleted = |m: &mut S, slot: u32| {
        let mut outcome = Err(Error::Again);
        for _ in 0..8 {
            outcome = m.call(0, Syscall::CapDelete(s(slot)));
            if outcome != Err(Error::Again) {
                break;
            }
            m.run(10 * MS);
        }
        outcome
    };
    let mark = |m: &S, slot: u32| {
        let node = m.sys.node(0);
        node.untyped_marks(m.t[0], s(slot)).expect("an untyped").0
    };
    let lost = |m: &S, slot: u32| {
        let node = m.sys.node(0);
        node.untypeds[&node.object_at(m.t[0], s(slot)).expect("an untyped")].lost
    };
    // After a barrier, a retype from the untyped in `slot` lands above its
    // watermark, which did not return: not over the root the unit keeps.
    let held = |m: &mut S, slot: u32| {
        m.run(30 * MS);
        let before = mark(m, slot);
        assert!(before > 0);
        m.ok(0, retype_from(slot, ObjectKind::Untyped, page, 170));
        assert_eq!(mark(m, slot), before + page, "slot {slot}");
        m.ok(0, Syscall::CapDelete(s(170)));
        m.run(30 * MS);
        assert_eq!(mark(m, slot), before + page, "slot {slot}");
    };
    // The iospace is destroyed with its root attached: lost to 162.
    assert_eq!(deleted(&mut m, IOSPACE), DONE);
    assert_eq!(lost(&m, 162), 1);
    assert_ne!(m.sys.node(0).page_class(root), PageClass::DATA);
    let floor_lost = lost(&m, floor);
    for (slot, parent) in [(162, 161), (161, 160), (160, floor)] {
        // Deletable: its lost pieces are waited for by no one.
        assert_eq!(deleted(&mut m, slot), DONE, "slot {slot}");
        held(&mut m, parent);
    }
    assert_eq!(lost(&m, floor), floor_lost + 1);
    assert_ne!(m.sys.node(0).page_class(root), PageClass::DATA);
    assert_eq!(m.sys.check(), Ok(()));
}

// G6 in the kernel's own reading before a grant: a frame over a table
// chunk the node holds, here a *data* page of its table floor that no
// healthy retype makes a frame of, is `busy` at `node_send` (D-156).
#[test]
fn node_send_of_a_frame_over_a_table_chunk_is_busy() {
    let (mut k, _, _) = with_blocks();
    k.ok(retype_from(BLOCK, ObjectKind::Frame, 1, 211));
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message {
            caps: vec![crate::Transfer::Derive(s(211), Rights::READ.bits())],
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    // Over its ordinary block, the gate's reading refuses nothing.
    assert_ne!(k.m.clone().apply(k.t, send.clone()), Err(Error::Busy));
    let frame = k.m.object_at(k.t, s(211)).expect("a frame");
    let (_, end) = k.m.table_floor(0)[0];
    crate::live(&mut k.m.frames, frame).base = end - k.m.geometry().page();
    assert_eq!(k.m.apply(k.t, send), Err(Error::Busy));
}
