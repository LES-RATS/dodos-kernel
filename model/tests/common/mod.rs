//! What the test files share: the slots of the start-up space of
//! `Machine::example`, the same on the three geometries of the tests.
//!
//! The start-up space holds the fixed slots 1 to 15, then from slot 16 the
//! two `core` roots (16, 17), the `iommu` root (18), the five `role` roots
//! (19 to 23), the two untypeds of the ordinary floor (24, the start-up
//! untyped, and 25, of one chunk), the table untyped of the table floor
//! (26, D-154), the copied writable segment (27), the kept frames of the
//! three modules (28 to 30) and of the two crash areas (31, 32).

#![allow(dead_code)]

use dodos_abi::{ObjectKind, START_SLOT_MEMORY_CONTROL, SlotAddr};
use dodos_model::{Model, ObjId, Output, Syscall};

/// The untyped of one chunk that the tests take objects from.
pub const UNTYPED: u32 = 25;
/// The table untyped of the node's table floor: the root table and the
/// lower tables of the spaces the tests make come from it (D-154).
pub const TABLES: u32 = 26;

/// The logical number of the table-floor untyped for `slot`.
pub const TABLES_INDEX: u32 = 1000;

/// The size argument of a retype as the tests write it: an `aspace` or an
/// `iospace` asked with 0 names the table-floor untyped, the slot address
/// the contract asks for there (D-154); every other size is itself.
pub const fn sized(kind: ObjectKind, size: u64) -> u64 {
    match kind {
        ObjectKind::ASpace | ObjectKind::IoSpace if size == 0 => SlotAddr::own(TABLES).raw(),
        _ => size,
    }
}

/// Where the tests place the block of the device range of the machine.
pub const DEVICE: u32 = 64;
/// The first role root, and the start-up untyped.
pub const ROLES: u32 = 19;
pub const START_UNTYPED: u32 = 24;

/// The claims of the two reserve roles (the role roots 21 and 22), in
/// order: a node that stops another with mode kill or retire holds one
/// (`node_stop`, D-150), so a test makes these calls first and goes on
/// once one succeeded.
pub fn reserve_claims() -> [Syscall; 2] {
    [ROLES + 2, ROLES + 3].map(|role| Syscall::RoleClaim(SlotAddr::own(role)))
}

/// The slot a test names by a logical number: the fixed slots and the
/// first three machine roots are themselves, 19 is the start-up untyped
/// (an untyped that no test object comes from), 20 the untyped, 21 the
/// device block, and every number from 22 lies past the start-up
/// capabilities and the device block, 64 further.
pub const fn slot(index: u32) -> u32 {
    match index {
        0..=18 => index,
        19 => START_UNTYPED,
        20 => UNTYPED,
        21 => DEVICE,
        TABLES_INDEX => TABLES,
        _ => index + 64,
    }
}

/// Takes the device entry of the machine's 2 MB device range at 2 MB as a
/// block in `DEVICE` (the parts of usable memory below it left out of the
/// chunks are device entries too).
pub fn take_device(model: &mut Model, thread: ObjId) {
    let index = (0..model.device_count())
        .find(|&i| model.device_entry(i).base() == Some(dodos_model::Machine::DEVICES))
        .expect("the 2 MB device range");
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(START_SLOT_MEMORY_CONTROL),
        kind: 1,
        place: index as u64,
        size: 0,
        destination: SlotAddr::own(DEVICE),
    };
    assert_eq!(model.apply(thread, take), Ok(Output::Done));
}

/// Takes a run of `1 << log2` bytes of unclaimed ordinary memory, the first
/// such run from the base of the machine's ordinary memory past its floors
/// and reserves, as a block in `slot`.
pub fn take_run(model: &mut Model, thread: ObjId, log2: u32, slot: u32) {
    take_kind(model, thread, dodos_abi::MEMORY_TAKE_RUN, log2, slot);
}

/// The same run taken for tables: a table untyped, `memory_take` kind 2
/// (D-154).
pub fn take_tables(model: &mut Model, thread: ObjId, log2: u32, slot: u32) {
    take_kind(model, thread, dodos_abi::MEMORY_TAKE_TABLES, log2, slot);
}

fn take_kind(model: &mut Model, thread: ObjId, kind: u64, log2: u32, slot: u32) {
    let geometry = model.geometry();
    let chunk = geometry.chunk();
    let machine = dodos_model::Machine::example_on(geometry);
    let ram = machine.memory[5].base;
    // Floors of 2 × (3 + 1) chunks (the table floor, D-154) and 2 reserves
    // of 1 chunk: 10 chunks.
    let free = ram + 10 * chunk;
    let place = free.next_multiple_of(1 << log2);
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(START_SLOT_MEMORY_CONTROL),
        kind,
        place,
        size: u64::from(log2),
        destination: SlotAddr::own(slot),
    };
    assert_eq!(model.apply(thread, take), Ok(Output::Done));
}
