//! What the test files share: the slots of the start-up space of
//! `Machine::example`, the same on the three geometries of the tests.
//!
//! The start-up space holds the fixed slots 1 to 15, then from slot 16 the
//! two `core` roots (16, 17), the `iommu` root (18), the five `role` roots
//! (19 to 23), the two untypeds of the floor (24, the start-up untyped, and
//! 25, of one chunk), the copied writable segment (26), the kept frames of
//! the three modules (27 to 29) and of the two crash areas (30, 31).

#![allow(dead_code)]

use dodos_abi::{START_SLOT_MEMORY_CONTROL, SlotAddr};
use dodos_model::{Model, ObjId, Output, Syscall};

/// The untyped of one chunk that the tests take objects from.
pub const UNTYPED: u32 = 25;
/// Where the tests place the block of the device range of the machine.
pub const DEVICE: u32 = 64;
/// The first role root, and the start-up untyped.
pub const ROLES: u32 = 19;
pub const START_UNTYPED: u32 = 24;

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
        _ => index + 64,
    }
}

/// Takes the device entry of the machine's 2 MB device range at 2 MB as a
/// block in `DEVICE` (the parts of usable memory below it left out of the
/// chunks are device entries too).
pub fn take_device(model: &mut Model, thread: ObjId) {
    let index = (0..model.device_count())
        .find(|&i| model.device_entry(i).base() == dodos_model::Machine::DEVICES)
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
    let geometry = model.geometry();
    let chunk = geometry.chunk();
    let machine = dodos_model::Machine::example_on(geometry);
    let ram = machine.memory[5].base;
    // Floors of 2 × 3 chunks and 2 reserves of 1 chunk: 8 chunks.
    let free = ram + 8 * chunk;
    let place = free.next_multiple_of(1 << log2);
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place,
        size: u64::from(log2),
        destination: SlotAddr::own(slot),
    };
    assert_eq!(model.apply(thread, take), Ok(Output::Done));
}
