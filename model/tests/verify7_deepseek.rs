//! Independent verification tests for Review 7 (DeepSeek) and sibling
//! findings, written by Hydrogen (`2026-11-16-model-review-7-verify-
//! deepseek-by-hydrogen.md`), taken in by pass 14.
//!
//! The tests of F1, F2, F5, F6, F10 and N1, N2, N4, N5 failed on `2afc901`
//! (run again by pass 14 before taking them in) and pass since pass 14; those
//! of F4, F9 and N3 assert what the contract asks and passed before. Two
//! corrections: F1 wrote back the 32 words that `thread_read_regs` returns
//! (the block and the fault record), `invalid` whatever the rule; F5 reads
//! the whole gate word through `gate_word`.
//!
//! The file is the record of that review: each comment below states the
//! finding as it was, against the model and the contract of 16 November
//! 2026, and the line numbers it cites are those of that contract (they
//! have moved since). The reference test of each rule, which later passes
//! keep (the final review's F2 Opus 39 and 40): F1, `faults.rs`'s
//! redirect below the half of the address bits; F2 and N2,
//! `gate_attack.rs`'s `a_partial_mapping_goes_on_only_with_the_page_count_
//! it_started_with`; F5, `gate_classes.rs` (the class of a page); F6 and
//! N4, `gate_classes.rs` (device leaves and the global read-only set);
//! N5, the `abi` Kani proof of `Geometry::valid`.

mod common;

use dodos_abi::ObjectKind::{ASpace, Endpoint, Frame, IoSpace, Task, Thread};
use dodos_abi::*;
use dodos_model::*;

const UNTYPED: SlotAddr = SlotAddr::own(common::UNTYPED);
/// The node's table-floor untyped (D-154).
const TABLES: SlotAddr = SlotAddr::own(common::TABLES);
const CONTROL: SlotAddr = SlotAddr::own(START_SLOT_MEMORY_CONTROL);
const LIVING: SlotAddr = SlotAddr::own(START_SLOT_MACHINE_LIVING);
const IOMMU: SlotAddr = SlotAddr::own(18);
const VADDR: u64 = 1 << 30;

const fn slot(index: u32) -> SlotAddr {
    SlotAddr::own(common::slot(index))
}

fn retype_with(
    untyped: SlotAddr,
    kind: ObjectKind,
    size: u64,
    page: PageSize,
    memory: MemoryType,
    dst: u32,
) -> Syscall {
    Syscall::UntypedRetype {
        untyped,
        kind,
        size: common::sized(kind, size),
        page: page as u64,
        memory: memory as u64,
        destination: slot(dst),
    }
}

fn retype_small(kind: ObjectKind, size: u64, dst: u32) -> Syscall {
    retype_with(
        UNTYPED,
        kind,
        size,
        PageSize::Page,
        MemoryType::WriteBack,
        dst,
    )
}

fn aspace_map(
    space: SlotAddr,
    frame: SlotAddr,
    untyped: SlotAddr,
    at: u64,
    count: u64,
    rights: Rights,
) -> Syscall {
    Syscall::ASpaceMap(Map {
        space,
        frame,
        untyped,
        address: at,
        first: 0,
        count,
        rights: rights.bits(),
    })
}

fn iospace_map(
    space: SlotAddr,
    frame: SlotAddr,
    untyped: SlotAddr,
    at: u64,
    count: u64,
    rights: Rights,
) -> Syscall {
    Syscall::IoSpaceMap(Map {
        space,
        frame,
        untyped,
        address: at,
        first: 0,
        count,
        rights: rights.bits(),
    })
}

struct NodeHarness {
    m: Model,
    t: ObjId,
}

impl NodeHarness {
    fn on(machine: &Machine) -> Self {
        let (m, t) = Model::boot(machine).unwrap();
        Self { m, t }
    }

    fn new() -> Self {
        Self::on(&Machine::example())
    }

    #[track_caller]
    fn call(&mut self, call: Syscall) -> Outcome {
        let out = self.m.apply(self.t, call);
        assert_eq!(self.m.check(), Ok(()));
        out
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        for _ in 0..40_000 {
            match self.call(call.clone()) {
                Ok(Output::Done) => return,
                Err(Error::Again) => {}
                other => panic!("expected Done/Again, got {other:?}"),
            }
        }
        panic!("exceeded step limit");
    }

    #[track_caller]
    fn err(&mut self, call: Syscall, expected: Error) {
        assert_eq!(self.call(call), Err(expected));
    }

    fn make_aspace(&mut self, aspace_idx: u32, thread_idx: u32, table_ut: SlotAddr) -> ObjId {
        // Its header from the ordinary untyped, its root table from the
        // table untyped the size argument names (D-154).
        self.ok(retype_with(
            UNTYPED,
            ASpace,
            table_ut.raw(),
            PageSize::Page,
            MemoryType::WriteBack,
            aspace_idx,
        ));
        self.ok(retype_small(Thread, 0, thread_idx));
        self.ok(Syscall::ThreadConfigure {
            thread: slot(thread_idx),
            cspace: SlotAddr::own(START_SLOT_CSPACE),
            aspace: slot(aspace_idx),
            fault: SlotAddr::NONE,
            page: SlotAddr::NONE,
        });
        self.m.object_at(self.t, slot(thread_idx)).unwrap()
    }
}

struct TwoNodeHarness {
    sys: System,
    t: Vec<ObjId>,
}

impl TwoNodeHarness {
    fn new() -> Self {
        let (sys, t) = System::boot(&Machine::example()).unwrap();
        Self { sys, t }
    }

    #[track_caller]
    fn call(&mut self, node: u16, call: Syscall) -> Outcome {
        let out = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        out
    }

    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        for _ in 0..40_000 {
            match self.call(node, call.clone()) {
                Ok(Output::Done) => return,
                // A call that waits for a unit (node 1 holds the unit role of
                // the machine) ends at the unit's confirmation.
                Ok(Output::Waits) => {
                    self.settle();
                    let woken = self.sys.node(node).woken(self.t[node as usize]);
                    assert_eq!(woken, Some(&Ok(Output::Done)));
                    return;
                }
                Err(Error::Again) => {}
                other => panic!("node {node} expected Done/Again, got {other:?}"),
            }
        }
        panic!("exceeded step limit on node {node}");
    }

    #[track_caller]
    fn err(&mut self, node: u16, call: Syscall, expected: Error) {
        assert_eq!(self.call(node, call), Err(expected));
    }

    #[track_caller]
    fn settle(&mut self) {
        self.sys.settle(100_000);
        assert_eq!(self.sys.check(), Ok(()));
    }
}

// ---------------------------------------------------------------------------
// F1: Canonical-address check in `abi::Geometry::canonical` and `ThreadWriteRegs`.
// Contract line 423: "the words the architecture requires to be canonical addresses
// must be canonical".
// ---------------------------------------------------------------------------

#[test]
fn f1_canonical_follows_address_bits_in_abi_and_thread_write_regs() {
    let base = Machine::example().geometry;
    let bits = base.address_bits();
    let lower_user_end = 1u64 << (bits - 2);
    let low_geom = Geometry {
        user_end: lower_user_end,
        ..base
    };
    assert!(low_geom.valid());

    // Highest canonical lower-half address of this address width, and lowest
    // canonical upper-half address of this address width.
    let high_lower = (1u64 << (bits - 1)) - 0x1000;
    let low_upper = u64::MAX << (bits - 1);

    assert!(
        low_geom.canonical(high_lower),
        "F1: lower-half address {high_lower:#x} within {bits}-bit width must be canonical even when user_end={lower_user_end:#x}"
    );
    assert!(
        low_geom.canonical(low_upper),
        "F1: upper-half address {low_upper:#x} within {bits}-bit width must be canonical even when user_end={lower_user_end:#x}"
    );

    // Same failure observed through a kernel syscall (`thread_write_regs`):
    let machine = Machine::example_on(low_geom);
    let mut k = NodeHarness::on(&machine);
    k.ok(retype_small(Thread, 0, 30));
    let Ok(Output::Regs(mut regs)) = k.call(Syscall::ThreadReadRegs(slot(30))) else {
        panic!("expected ThreadReadRegs output");
    };
    // The result is the register block, then the six words of the fault
    // record (Register calls): the block alone is written back. (Hydrogen's
    // test wrote the 32 words back, which is `invalid` whatever the rule of
    // canonical addresses.)
    regs.truncate(dodos_abi::x86_64::REGS_WORDS);
    regs[x86_64::REG_RIP] = high_lower;
    k.ok(Syscall::ThreadWriteRegs {
        thread: slot(30),
        block: regs,
    });
}

// ---------------------------------------------------------------------------
// F2: Partial mapping resumed with a different page count.
// Contract lines 282 & 887: resumption identity omits `count`, allowing a
// 4-page partial mapping to finish as a 3-page mapping with `Ok(Done)`.
// ---------------------------------------------------------------------------

#[test]
fn f2_partial_aspace_map_rejects_different_page_count() {
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut k = NodeHarness::on(&machine);
    let viewer = k.make_aspace(30, 31, TABLES);
    k.ok(retype_small(Frame, 4, 32));

    k.err(
        aspace_map(slot(30), slot(32), TABLES, VADDR, 4, Rights::READ),
        Error::Again,
    );
    let page = k.m.geometry().page();
    assert!(k.m.translated(viewer, VADDR).is_some());
    assert!(k.m.translated(viewer, VADDR + page).is_none());

    // Resuming with a smaller count equal to the done count (1) returns Busy;
    // the finding was that count = 3 (smaller than the original 4, larger
    // than the done 1) succeeded with Ok(Done): it is `busy` too.
    let mut retry_short = Err(Error::Again);
    for _ in 0..3 {
        retry_short = k.call(aspace_map(
            slot(30),
            slot(32),
            TABLES,
            VADDR,
            3,
            Rights::READ,
        ));
        if retry_short != Err(Error::Again) {
            break;
        }
    }
    assert_eq!(
        retry_short,
        Err(Error::Busy),
        "F2: a 4-page partial aspace_map resumed with count = 3"
    );
}

// ---------------------------------------------------------------------------
// F4: `iospace_attach` checks identifier owner (`denied`, line 342) BEFORE
// four `busy` checks (lines 343-347), whereas Order of Checks line 139 puts
// `busy` before `denied`.
// ---------------------------------------------------------------------------

#[test]
fn f4_iospace_attach_state_denied_precedes_trailing_busy_checks() {
    let mut sys = TwoNodeHarness::new();
    sys.ok(0, retype_small(IoSpace, 0, 30));
    // Node 0 claims device identifier (0, 1) (Node 1 holds the unit role).
    assert_eq!(
        sys.call(
            0,
            Syscall::IoSpaceAttach {
                iommu: IOMMU,
                iospace: slot(30),
                device: 1,
            },
        ),
        Ok(Output::Waits)
    );
    sys.settle();
    assert_eq!(sys.sys.node(0).woken(sys.t[0]), Some(&Ok(Output::Done)));

    // Node 1 fills an iospace with 16 devices (2..=17) so any further attach
    // hits the `IOSPACE_MAX_DEVICES` (`busy`, line 346) state condition too.
    sys.ok(1, retype_small(IoSpace, 0, 30));
    for dev in 2..=17u64 {
        sys.ok(
            1,
            Syscall::IoSpaceAttach {
                iommu: IOMMU,
                iospace: slot(30),
                device: dev,
            },
        );
    }

    // Both `denied` (device 1 owned by node 0) and `busy` (16 devices) hold.
    // The model follows line 342 before line 346 (`Denied`), not line 139 (`Busy`).
    sys.err(
        1,
        Syscall::IoSpaceAttach {
            iommu: IOMMU,
            iospace: slot(30),
            device: 1,
        },
        Error::Denied,
    );
}

// ---------------------------------------------------------------------------
// F5: Gate word bits 32-44 (`not_data`, G4 line 888) remain 0 after placing a
// page table into a chunk.
// ---------------------------------------------------------------------------

#[test]
fn f5_gate_word_bits_32_to_44_count_non_data_pages() {
    let mut k = NodeHarness::new();
    let chunk_shift = k.m.geometry().chunk_shift();
    let chunk_size = k.m.geometry().chunk();
    let machine = Machine::example_on(k.m.geometry());
    let base = (machine.memory[5].base + 10 * chunk_size).next_multiple_of(1 << chunk_shift);
    let chunk = base >> chunk_shift;

    common::take_tables(&mut k.m, k.t, chunk_shift, common::slot(40));
    // An ASpace whose table untyped is slot(40), a table block (D-154),
    // turns pages of `chunk` into table pages (non-data per G2): its root
    // and the tables of its mapping.
    k.make_aspace(30, 31, slot(40));
    k.ok(retype_small(Frame, 1, 32));
    k.ok(aspace_map(
        slot(30),
        slot(32),
        slot(40),
        VADDR,
        1,
        Rights::READ,
    ));

    // The whole gate word (`map_count` gives bits 0-31 only since pass 14).
    let raw = k.m.gate_word(chunk);
    let word = GateWord::from_raw(raw).expect("valid GateWord");
    assert!(
        word.not_data() > 0,
        "F5: chunk {chunk} holds table pages, so GateWord::not_data() (bits 32-44) must be > 0, got raw {raw:#x}"
    );
}

// ---------------------------------------------------------------------------
// F6: the finding was that device leaves and global read-only (kept)
// leaves were counted in `table.maps[chunk]`, though G2 (line 886) says
// both lie in no chunk.
// ---------------------------------------------------------------------------

#[test]
fn f6_device_and_kept_leaves_do_not_increment_chunk_gate_words() {
    let mut k = NodeHarness::new();
    common::take_device(&mut k.m, k.t);
    let chunk_shift = k.m.geometry().chunk_shift();
    let _viewer = k.make_aspace(30, 31, TABLES);

    // 1. Device leaf:
    k.ok(Syscall::UntypedRetype {
        untyped: SlotAddr::own(common::DEVICE),
        kind: Frame,
        size: 1,
        page: PageSize::Page as u64,
        memory: MemoryType::Uncached as u64,
        destination: slot(32),
    });
    let (_, _, dev_base) = k.m.frame_at(k.t, slot(32)).unwrap();
    let dev_chunk = dev_base >> chunk_shift;
    let dev_before = k.m.map_count(dev_chunk);
    k.ok(aspace_map(
        slot(30),
        slot(32),
        TABLES,
        VADDR,
        1,
        Rights::READ,
    ));
    let dev_after = k.m.map_count(dev_chunk);

    // 2. Global read-only (kept) leaf:
    let (_, _, kept_base) = k.m.frame_at(k.t, LIVING).unwrap();
    let kept_chunk = kept_base >> chunk_shift;
    let kept_before = k.m.map_count(kept_chunk);
    let page = k.m.geometry().page();
    k.ok(aspace_map(
        slot(30),
        LIVING,
        TABLES,
        VADDR + page,
        1,
        Rights::READ,
    ));
    let kept_after = k.m.map_count(kept_chunk);
    assert!(
        dev_after == dev_before && kept_after == kept_before,
        "F6: device chunk {dev_chunk} changed {dev_before}->{dev_after} and kept chunk {kept_chunk} changed {kept_before}->{kept_after}; both lie in no chunk"
    );
}

// ---------------------------------------------------------------------------
// F9: Completed multi-step mapping has `partial == false` and hits the 5th
// clause of `aspace_protect` ("touches more than E chunks, which only a
// mapping made in steps does", line 292). DeepSeek claimed this is dead.
// ---------------------------------------------------------------------------

#[test]
fn f9_completed_stepped_mapping_is_not_partial_and_hits_protect_fifth_clause() {
    // Part A (all 3 geometries with gate_chunks = 1):
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut k = NodeHarness::on(&machine);
    k.make_aspace(30, 31, TABLES);
    k.ok(retype_small(Frame, 2, 32));
    k.err(
        aspace_map(slot(30), slot(32), TABLES, VADDR, 2, Rights::READ),
        Error::Again,
    );
    k.ok(aspace_map(
        slot(30),
        slot(32),
        TABLES,
        VADDR,
        2,
        Rights::READ,
    ));
    // Now `partial == false`: protect cannot hit the 3rd ("partial mapping")
    // clause, yet still returns `Busy` via the 5th ("more than E chunks") clause.
    k.err(
        Syscall::ASpaceProtect {
            aspace: slot(30),
            frame: slot(32),
            rights: Rights::READ.bits(),
        },
        Error::Busy,
    );

    // Part B (unmodified `gate_chunks == E`): `memory_take` forbids runs > E * chunk,
    // so a 2 * E-chunk run cannot be taken when largest_page <= E * chunk.
    let default_m = Machine::example();
    let large2 = default_m
        .geometry
        .bytes(PageSize::Large2)
        .expect("three sizes");
    // On 16 and 32 KB pages the run of 2 × E chunks lies beyond the memory
    // of `Machine::example`, so its place would be refused too: the case
    // runs on 4 KB pages, and the others say so (the final review's F2 Opus
    // 20).
    if large2 > 1 << 30 {
        assert!(
            default_m.geometry.page() > 4096,
            "skipped only on larger pages"
        );
    } else {
        let mut k = NodeHarness::on(&default_m);
        let chunk = k.m.geometry().chunk();
        let ram = default_m.memory[5].base;
        let place = (ram + 10 * chunk).next_multiple_of(2 * large2);
        k.err(
            Syscall::MemoryTake {
                control: CONTROL,
                kind: 0,
                place,
                size: u64::from((2 * large2).trailing_zeros()),
                destination: slot(40),
            },
            Error::Invalid,
        );
    }
}

// ---------------------------------------------------------------------------
// F10: `Geometry::valid` accepts `largest_page > chunk * E`, breaking G3's
// "One call of the gate checks at most E chunks" given `step >= 1`.
// ---------------------------------------------------------------------------

#[test]
fn f10_geometry_valid_refuses_page_larger_than_e_chunks() {
    let four_level_512 = Geometry {
        page_shift: [12, 21, 30, 39],
        entries: 512,
        levels: 4,
        user_end: 1 << 46,
    };
    assert!(
        !four_level_512.valid(),
        "F10: 512 GiB Large3 leaf spans 2^18 > E=512 chunks, violating G3's one-call bound; Geometry::valid must reject it"
    );
}

// ---------------------------------------------------------------------------
// New sibling finding N1: `Geometry::valid` allows `user_end == 1 << (bits - 1)`,
// leaving no lower-half canonical entry for the reserved slot (line 258).
// ---------------------------------------------------------------------------

#[test]
fn n1_geometry_valid_leaves_room_for_reserved_slot() {
    let base = Machine::example().geometry;
    let bits = base.address_bits();
    let full_lower = Geometry {
        user_end: 1u64 << (bits - 1),
        ..base
    };
    assert!(
        !full_lower.valid(),
        "N1: user_end = 1 << {} fills the entire lower canonical half and leaves no lower-half entry for the reserved slot",
        bits - 1
    );
}

// ---------------------------------------------------------------------------
// New sibling finding N2: `iospace_map` shares F2's partial-count bug.
// ---------------------------------------------------------------------------

#[test]
fn n2_partial_iospace_map_rejects_different_page_count() {
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut k = NodeHarness::on(&machine);
    k.ok(retype_small(IoSpace, 0, 30));
    k.ok(retype_small(Frame, 4, 31));

    k.err(
        iospace_map(slot(30), slot(31), TABLES, VADDR, 4, Rights::READ),
        Error::Again,
    );
    let mut retry_short = Err(Error::Again);
    for _ in 0..3 {
        retry_short = k.call(iospace_map(
            slot(30),
            slot(31),
            TABLES,
            VADDR,
            3,
            Rights::READ,
        ));
        if retry_short != Err(Error::Again) {
            break;
        }
    }
    assert_eq!(
        retry_short,
        Err(Error::Busy),
        "N2: a 4-page partial iospace_map resumed with count = 3"
    );
}

// ---------------------------------------------------------------------------
// New sibling finding N3: More `denied`s without a missing right (beyond F3's
// two), falsifying line 284's `"the only denied judged with the state"`, plus
// `iospace_attach` putting `busy` BEFORE `denied` as well as AFTER `denied`.
// ---------------------------------------------------------------------------

#[test]
fn n3_other_state_denied_calls_and_iospace_attach_busy_before_denied() {
    let mut sys = TwoNodeHarness::new();
    // 1. `cap_derive` re-badge returns `Denied` (line 147), not missing rights:
    sys.ok(0, retype_small(Endpoint, 0, 30));
    sys.ok(
        0,
        Syscall::CapDerive {
            source: slot(30),
            destination: slot(31),
            rights: Rights::ALL.bits(),
            badge: 7,
        },
    );
    sys.err(
        0,
        Syscall::CapDerive {
            source: slot(31),
            destination: slot(32),
            rights: Rights::ALL.bits(),
            badge: 8,
        },
        Error::Denied,
    );

    // 2. `task_exempt` returns `Denied` when the aware task is not mark-aware (line 929):
    sys.ok(0, retype_small(Task, 0, 33));
    sys.ok(0, retype_small(Task, 0, 34));
    sys.err(
        0,
        Syscall::TaskExempt {
            task: slot(33),
            aware: slot(34),
        },
        Error::Denied,
    );

    // 3. `iospace_attach(..., 0)` on another node's attached device returns `Denied` (line 349):
    sys.ok(0, retype_small(IoSpace, 0, 35));
    assert_eq!(
        sys.call(
            0,
            Syscall::IoSpaceAttach {
                iommu: IOMMU,
                iospace: slot(35),
                device: 1,
            },
        ),
        Ok(Output::Waits)
    );
    sys.settle();
    sys.err(
        1,
        Syscall::IoSpaceAttach {
            iommu: IOMMU,
            iospace: SlotAddr::NONE,
            device: 1,
        },
        Error::Denied,
    );

    // 4. `iospace_attach` puts `kernel_devices` (`busy`, line 341) BEFORE
    //    `denied` (line 342) and four `busy` checks AFTER `denied` (343-347):
    sys.ok(1, retype_small(IoSpace, 0, 35));
    sys.err(
        1,
        Syscall::IoSpaceAttach {
            iommu: IOMMU,
            iospace: slot(35),
            device: 0x00f8,
        },
        Error::Busy,
    );
}

// ---------------------------------------------------------------------------
// New sibling finding N4: `iospace_map` of a device leaf and multi-node boot
// kept-frame mappings wrote the gate word of `*none*` chunk 0 from both
// nodes, breaking G4's single-writer rule ("so each count has one writer").
// ---------------------------------------------------------------------------

#[test]
fn n4_iospace_device_leaf_and_two_node_kept_mappings_write_none_chunk_gate_word() {
    let mut sys = TwoNodeHarness::new();
    let chunk_shift = sys.sys.node(0).geometry().chunk_shift();

    // 1. `iospace_map` of a device frame writes `table.maps[dev_chunk]`:
    let dev_delta = {
        let index = (0..sys.sys.node(0).device_count())
            .find(|&i| sys.sys.node(0).device_entry(i).base() == Machine::DEVICES)
            .unwrap();
        sys.ok(
            0,
            Syscall::MemoryTake {
                control: CONTROL,
                kind: 1,
                place: index as u64,
                size: 0,
                destination: slot(30),
            },
        );
        sys.ok(0, retype_small(IoSpace, 0, 31));
        sys.ok(
            0,
            Syscall::UntypedRetype {
                untyped: slot(30),
                kind: Frame,
                size: 1,
                page: PageSize::Page as u64,
                memory: MemoryType::Uncached as u64,
                destination: slot(32),
            },
        );
        let (_, _, dev_base) = sys.sys.node(0).frame_at(sys.t[0], slot(32)).unwrap();
        let dev_chunk = dev_base >> chunk_shift;
        let before = sys.sys.node(0).map_count(dev_chunk);
        sys.ok(
            0,
            iospace_map(slot(31), slot(32), TABLES, VADDR, 1, Rights::READ),
        );
        (before, sys.sys.node(0).map_count(dev_chunk))
    };

    // 2. Both Node 0 and Node 1 mapping their kept `START_SLOT_MACHINE_LIVING`
    //    frame write the gate word of the same `*none*` chunk (chunk 0):
    let (_, _, living_base) = sys.sys.node(0).frame_at(sys.t[0], LIVING).unwrap();
    let kept_chunk = living_base >> chunk_shift;
    assert_eq!(
        Ownership::from_raw(sys.sys.node(0).ownership(kept_chunk))
            .unwrap()
            .state(),
        OwnState::None,
        "chunk {kept_chunk} holding living table has ownership state None"
    );
    let page = sys.sys.node(0).geometry().page();
    let mut kept_deltas = Vec::new();
    for node in 0..2u16 {
        let before = sys.sys.node(node).map_count(kept_chunk);
        sys.ok(node, retype_small(ASpace, 0, 40));
        sys.ok(
            node,
            aspace_map(slot(40), LIVING, TABLES, VADDR + 2 * page, 1, Rights::READ),
        );
        kept_deltas.push((node, before, sys.sys.node(node).map_count(kept_chunk)));
    }
    assert!(
        dev_delta.0 == dev_delta.1 && kept_deltas.iter().all(|&(_, b, a)| b == a),
        "N4: iospace device chunk changed {}->{}, and both nodes wrote kept *none* chunk {kept_chunk}: {kept_deltas:?}",
        dev_delta.0,
        dev_delta.1
    );
}

// ---------------------------------------------------------------------------
// New sibling finding N5: `Geometry::valid` allows unaligned `page_shift`
// steps, letting `Model::boot` run with `page_shift[1] != page_shift[0] + log2(E)`,
// breaking `tables()` level arithmetic and producing `Large2 > E * chunk`.
// ---------------------------------------------------------------------------

#[test]
fn n5_geometry_valid_forbids_unaligned_page_shifts_and_oversized_leaf() {
    let weird = Geometry {
        page_shift: [12, 18, 28, 0], // 4 KiB, 256 KiB (not 4 KiB * 512!), 256 MiB (1024 chunks > E=512)
        entries: 512,
        levels: 3,
        user_end: 1 << 38,
    };
    // (A block that booted the model on this geometry ran only when the
    // assertion below was about to fail: deleted, the final review's F2
    // Opus 38.)
    assert!(
        !weird.valid(),
        "N5: unaligned page_shift [12, 18, 28, 0] with E=512 boots Model, breaks table-level math for Large1, and has Large2 = 1024 > E chunks"
    );
}
