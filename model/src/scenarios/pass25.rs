//! Pass 25: the tables of the IOMMU units kept at start-up (IOMMU, Start-up
//! state, C7 of D-142): 1 MB and 4 KB per VT-d unit, among the kept ranges,
//! out of the ordinary memory, and a machine with no room for them refused.

use super::*;
use crate::boot::{IommuUnit, Refusal, RegionKind};
use dodos_abi::x86_64::VTD_UNIT_TABLE_BYTES;

/// The kept ranges of the machine table that are a unit's tables.
fn unit_tables(machine: &Machine) -> Vec<(u64, u64)> {
    let (model, _) = Model::boot(machine).expect("the machine boots");
    let bytes = VTD_UNIT_TABLE_BYTES.next_multiple_of(machine.geometry.page());
    let image = machine.image_base;
    let kept = model.table.kept.iter().copied();
    kept.filter(|&(s, e)| e - s == bytes && s != image)
        .collect()
}

/// The example with `units` VT-d units, no usable memory below 4 GB, and
/// `chunks` chunks of usable memory from 4 GB, the top two holding the
/// crash areas.
fn small(units: usize, chunks: u64) -> Machine {
    let mut machine = Machine::example();
    let chunk = machine.geometry.chunk();
    machine.memory[1].kind = RegionKind::Unusable;
    let ram = machine.memory[5].base;
    let end = machine.memory[6].base + machine.memory[6].len;
    machine.memory[5].len = chunks * chunk;
    machine.memory[6].base = ram + chunks * chunk;
    machine.memory[6].len = end - machine.memory[6].base;
    machine.crash = (ram + (chunks - 2) * chunk, Some(ram + (chunks - 1) * chunk));
    machine.iommus = vec![
        IommuUnit {
            segment: 0,
            domains: 256,
        };
        units
    ];
    machine
}

#[test]
fn the_tables_of_each_vtd_unit_are_kept_at_start_up_out_of_the_ordinary_memory() {
    let machine = Machine::example();
    let tables = unit_tables(&machine);
    assert_eq!(tables.len(), machine.iommus.len());
    let (model, _) = Model::boot(&machine).expect("the machine boots");
    let (chunk, page) = (machine.geometry.chunk(), machine.geometry.page());
    let none = dodos_abi::Ownership::none().raw();
    for &(start, end) in &tables {
        assert!(start.is_multiple_of(page));
        // In usable memory, clear of every other kept range.
        assert!(
            machine.memory.iter().any(|r| {
                r.kind == RegionKind::Usable && r.base <= start && end <= r.base + r.len
            })
        );
        let overlaps = |&(s, e): &(u64, u64)| s < end && start < e;
        let kept = model.table.kept.iter().filter(|r| overlaps(r)).count();
        assert_eq!(kept, 1);
        // In no chunk, no floor and no device entry.
        assert!((start / chunk..end.div_ceil(chunk)).all(|c| model.ownership(c) == none));
        assert!(!model.table.floors.iter().flatten().any(overlaps));
        let used = model.table.devices.iter().filter(|d| d.is_used());
        assert!(
            !used
                .map(|d| (d.base(), d.base() + (1 << d.log2())))
                .any(|r| overlaps(&r))
        );
    }
    // The example has room for them in its usable memory below 4 GB, which
    // holds no chunk: its ordinary memory has as many chunks as with no
    // unit.
    assert!(tables.iter().all(|&(_, end)| end <= 4 << 20));
    let words = 1u64 << (machine.phys_bits - machine.geometry.chunk_shift());
    let chunks = |model: &Model| (0..words).filter(|&c| model.ownership(c) != none).count();
    let mut no_unit = machine.clone();
    no_unit.iommus.clear();
    let (other, _) = Model::boot(&no_unit).expect("the machine boots");
    assert_eq!(chunks(&model), chunks(&other));
}

#[test]
fn every_vtd_unit_has_tables_of_its_own() {
    let mut machine = Machine::example();
    machine.iommus.push(IommuUnit {
        segment: 1,
        domains: 256,
    });
    let tables = unit_tables(&machine);
    assert_eq!(tables.len(), 2);
    assert!(tables[0].1 <= tables[1].0 || tables[1].1 <= tables[0].0);
}

#[test]
fn a_machine_with_no_room_for_the_tables_of_its_units_is_refused() {
    // The fewest chunks with which the machine boots with no unit.
    let fewest = (3..64)
        .find(|&chunks| Model::boot(&small(0, chunks)).is_ok())
        .expect("a machine that boots");
    // A unit's tables take a chunk out of the ordinary memory there: the
    // floors no longer fit, refusal 4 (Start-up state: "the floors, the
    // reserves and the kept memory do not fit in the ordinary memory").
    assert_eq!(Model::boot(&small(1, fewest)).err(), Some(Refusal::Floors));
    let one_more = small(1, fewest + 1);
    assert_eq!(unit_tables(&one_more).len(), 1);
}

#[test]
fn arm64_and_riscv_keep_no_vtd_tables() {
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut machine = Machine::example();
        machine.arch = arch;
        assert!(unit_tables(&machine).is_empty(), "{arch:?}");
    }
}
