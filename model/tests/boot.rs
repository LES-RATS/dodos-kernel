//! Start-up state: the machine table and the start-up space the kernel
//! builds from the machine and the boot image, one test per rule, on the
//! geometry of the run (both geometries give the same slots).

use dodos_abi::ObjectKind::{
    ASpace, CSpace, Core, Debug, Endpoint, Frame, IoPorts, Iommu, IrqControl, MemoryControl, Role,
    Sched, Task, Thread, Untyped,
};
use dodos_abi::*;
use dodos_model::*;

fn own(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn boot(machine: &Machine) -> (Model, ObjId) {
    let booted = Model::boot(machine).unwrap();
    assert_eq!(booted.0.check(), Ok(()));
    booted
}

fn inspect(m: &mut Model, t: ObjId, slot: u32) -> Result<(ObjectKind, Rights), Error> {
    match m.apply(t, Syscall::CapInspect(own(slot)))? {
        Output::Inspect { kind, rights, .. } => Ok((kind, rights)),
        other => panic!("{other:?}"),
    }
}

fn kind(m: &mut Model, t: ObjId, slot: u32) -> Result<ObjectKind, Error> {
    Ok(inspect(m, t, slot)?.0)
}

fn retype(untyped: u32, kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: own(untyped),
        kind,
        size,
        page: 0,
        memory: 0,
        destination: own(destination),
    }
}

fn derive(source: u32, destination: u32, rights: Rights) -> Syscall {
    Syscall::CapDerive {
        source: own(source),
        destination: own(destination),
        rights: rights.bits(),
        badge: 0,
    }
}

fn take(kind: u64, place: u64, size: u64, destination: u32) -> Syscall {
    Syscall::MemoryTake {
        control: own(START_SLOT_MEMORY_CONTROL),
        kind,
        place,
        size,
        destination: own(destination),
    }
}

/// The bytes of the untyped in `slot`: the largest child untyped it can
/// make, made and deleted again in a free slot.
fn bytes(m: &mut Model, t: ObjId, slot: u32) -> u64 {
    let free = 4000;
    let page = m.geometry().page().trailing_zeros();
    for log in (page..=40).rev() {
        if m.apply(t, retype(slot, Untyped, 1 << log, free)).is_ok() {
            m.apply(t, Syscall::CapDelete(own(free))).unwrap();
            return 1 << log;
        }
    }
    0
}

/// The machine of the tests, changed.
fn with(change: impl FnOnce(&mut Machine)) -> Machine {
    let mut machine = Machine::example();
    change(&mut machine);
    machine
}

const NOT_EXEC: Rights = Rights::ALL.without(Rights::EXEC);
const READ_DERIVE: Rights = Rights::READ.union(Rights::DERIVE);

#[test]
fn start_up_space_holds_the_slots_of_the_table_in_their_order() {
    let (mut m, t) = boot(&Machine::example());
    let fixed = [
        CSpace,
        ASpace,
        Thread,
        Sched,
        Debug,
        IrqControl,
        IoPorts,
        Task,
        Frame,
        Frame,
        Frame,
        MemoryControl,
        Frame,
        Frame,
        Endpoint,
    ];
    for (slot, wanted) in (1..).zip(fixed) {
        assert_eq!(kind(&mut m, t, slot), Ok(wanted), "slot {slot}");
    }
    assert_eq!(kind(&mut m, t, 0), Err(Error::Invalid));
    // From 16: the two cores, the IOMMU unit, the five roles (the IOMMU
    // unit, the IOAPIC, two reserves, `platform`), the two untypeds of the
    // floor, the copied writable segment, the kept frames of the three
    // modules, the two crash areas; then empty slots.
    let machine = [
        Core, Core, Iommu, Role, Role, Role, Role, Role, Untyped, Untyped, Frame, Frame, Frame,
        Frame, Frame, Frame,
    ];
    for (slot, wanted) in (16..).zip(machine) {
        assert_eq!(kind(&mut m, t, slot), Ok(wanted), "slot {slot}");
    }
    assert_eq!(kind(&mut m, t, 32), Err(Error::Invalid));
    assert_eq!(m.object_at(t, own(START_SLOT_THREAD)), Some(t));
}

#[test]
fn start_up_space_has_4096_empty_slots_or_more_in_whole_segments() {
    let (mut m, t) = boot(&Machine::example());
    // 32 slots used and 4,096 free: 4,128, so 33 segments of 128.
    assert_eq!(m.apply(t, derive(25, 4223, Rights::ALL)), Ok(Output::Done));
    assert_eq!(
        m.apply(t, derive(25, 4224, Rights::ALL)),
        Err(Error::Invalid)
    );
    // It never grows.
    let grow = Syscall::CSpaceGrow {
        cspace: own(START_SLOT_CSPACE),
        untyped: own(25),
    };
    assert_eq!(m.apply(t, grow), Err(Error::Invalid));
}

#[test]
fn start_up_capabilities_have_the_rights_of_their_kind_and_row() {
    let (mut m, t) = boot(&Machine::example());
    // Roots made from no untyped, and machine objects: every right but
    // `exec`; so do the frames of the start-up untyped and the copy.
    for slot in (1..12).chain([15, 26]).chain(16..24) {
        assert_eq!(inspect(&mut m, t, slot).unwrap().1, NOT_EXEC, "slot {slot}");
    }
    // The memory_control and the floor untypeds have `exec` too.
    for slot in [12, 24, 25] {
        assert_eq!(
            inspect(&mut m, t, slot).unwrap().1,
            Rights::ALL,
            "slot {slot}"
        );
    }
    // The kept frames of the kept image: `read`, `exec` and `derive`; of the
    // machine table and the crash areas: `read` and `derive`.
    for slot in 27..30 {
        let rights = READ_DERIVE.union(Rights::EXEC);
        assert_eq!(inspect(&mut m, t, slot).unwrap().1, rights, "slot {slot}");
    }
    for slot in [13, 14, 30, 31] {
        assert_eq!(
            inspect(&mut m, t, slot).unwrap().1,
            READ_DERIVE,
            "slot {slot}"
        );
    }
}

/// Derives copies from `slot` into the slots from 4000, and returns how many
/// were accepted before the depth of 64 refused one.
fn copies_to_the_bottom(m: &mut Model, t: ObjId, slot: u32) -> u32 {
    let mut from = slot;
    for made in 0..70 {
        let to = 4000 + made;
        if m.apply(t, derive(from, to, READ_DERIVE)) != Ok(Output::Done) {
            return made;
        }
        from = to;
    }
    panic!("no limit");
}

#[test]
fn start_up_capabilities_are_at_depth_1_the_frames_at_2_and_the_copy_at_3() {
    let depths = [
        (24, 1),
        (START_SLOT_TASK, 1),
        (27, 1),
        (13, 1),
        (26, 2),
        (10, 2),
        (11, 3),
    ];
    for (slot, depth) in depths {
        let (mut m, t) = boot(&Machine::example());
        let below = copies_to_the_bottom(&mut m, t, slot);
        assert_eq!(below, TREE_DEPTH as u32 - depth, "slot {slot}");
    }
}

#[test]
fn untypeds_are_the_floor_cut_into_naturally_aligned_blocks() {
    let (mut m, t) = boot(&Machine::example());
    // A floor of three chunks from a chunk aligned on four: two chunks, then
    // one.
    let chunk = m.geometry().chunk();
    assert_eq!(bytes(&mut m, t, 25), chunk);
    // The start-up untyped holds the frames of the start-up: a child of one
    // chunk still fits after them, aligned.
    let child = retype(24, Untyped, chunk, 40);
    assert_eq!(m.apply(t, child), Ok(Output::Done));
    let full = retype(24, Untyped, chunk, 41);
    assert_eq!(m.apply(t, full), Err(Error::NoMemory));
}

#[test]
fn start_up_untyped_is_taken_after_the_message_page() {
    let (mut m, t) = boot(&Machine::example());
    // The copied segment, the start-up page and the message page are its
    // children, in that order, from its start.
    let base = |m: &Model, slot| m.frame_at(t, own(slot)).unwrap().2;
    let (copied, start, message) = (base(&m, 26), base(&m, 9), base(&m, 10));
    let page = m.geometry().page();
    let start_pages = m.frame_at(t, own(9)).unwrap().1;
    let copied_pages = m.frame_at(t, own(26)).unwrap().1;
    assert_eq!(start, copied + copied_pages * page);
    assert_eq!(message, start + start_pages * page);
    assert_eq!(copied, Machine::example().memory[4].base);
    // An endpoint goes right after the message page.
    let endpoint = retype(24, Endpoint, 0, 40);
    assert_eq!(m.apply(t, endpoint), Ok(Output::Done));
}

#[test]
fn ordinary_untypeds_are_dirty_from_their_start() {
    let (mut m, t) = boot(&Machine::example());
    let frame = Syscall::UntypedRetype {
        untyped: own(25),
        kind: Frame,
        size: 32,
        page: 0,
        memory: 0,
        destination: own(40),
    };
    // 32 pages at 64 KB per call.
    let calls = (32 * m.geometry().page()).div_ceil(CLEAR_STEP_BYTES);
    for _ in 1..calls {
        assert_eq!(m.apply(t, frame.clone()), Err(Error::Again));
    }
    assert_eq!(m.apply(t, frame), Ok(Output::Done));
}

#[test]
fn address_space_maps_the_segments_the_message_page_and_the_start_up_page() {
    let (m, t) = boot(&Machine::example());
    let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
    let geometry = m.geometry();
    let (chunk, page) = (geometry.chunk(), geometry.page());
    let mapped = |address| m.translated(t, address);
    let segments = Machine::example().segments;
    let (code, data) = (&segments[0], &segments[1]);
    assert_eq!(code.vaddr, chunk);
    assert_eq!(mapped(code.vaddr), Some(r.union(x)));
    assert_eq!(mapped(code.vaddr + code.len - 1), Some(r.union(x)));
    assert_eq!(mapped(data.vaddr), Some(r.union(w)));
    assert_eq!(mapped(data.vaddr + data.len - 1), Some(r.union(w)));
    assert_eq!(mapped(data.vaddr + data.len), None);
    assert_eq!(mapped(page), Some(r.union(w)));
    assert_eq!(mapped(2 * page), Some(r));
    let start_pages = m.frame_at(t, own(START_SLOT_START_PAGE)).unwrap().1;
    assert_eq!(mapped((2 + start_pages) * page), None);
    assert_eq!(mapped(0), None);
}

#[test]
fn segments_that_are_not_writable_are_mapped_from_the_kept_image() {
    let (m, t) = boot(&Machine::example());
    // The executable segment is the first module: its kept frame in slot 27
    // holds the mapping; the writable one is mapped from its copy.
    let image = Machine::example().image_base;
    assert_eq!(m.frame_at(t, own(27)).unwrap().2, image);
    let copied = m.frame_at(t, own(26)).unwrap().2;
    assert!(copied >= Machine::example().memory[4].base);
    let (mut m, t) = (m, t);
    let unmap = Syscall::ASpaceUnmap(own(27));
    assert_eq!(m.apply(t, unmap), Ok(Output::Done));
    assert_eq!(m.translated(t, m.geometry().chunk()), None);
}

#[test]
fn kept_frames_cover_each_module_in_consecutive_slots_by_the_largest_sizes() {
    // A file of one large page and three pages, placed on a large page.
    let machine = with(|m| {
        let (chunk, page) = (m.geometry.chunk(), m.geometry.page());
        m.files[0].len = chunk + 3 * page - 5;
        m.image_base = 8 << 20;
        m.kept.push((8 << 20, 0));
        m.kept.pop();
    });
    let chunk = machine.geometry.chunk();
    let image = (8u64 << 20).max(chunk).next_multiple_of(chunk);
    let machine = Machine {
        image_base: image,
        ..machine
    };
    let (m, t) = boot(&machine);
    // The two segments, then the file: one large page, then three pages.
    assert_eq!(m.frame_at(t, own(29)).unwrap().0, PageSize::Large1);
    assert_eq!(m.frame_at(t, own(29)).unwrap().1, 1);
    let (size, pages, base) = m.frame_at(t, own(30)).unwrap();
    assert_eq!((size, pages), (PageSize::Page, 3));
    assert_eq!(base, m.frame_at(t, own(29)).unwrap().2 + chunk);
    // Then the crash areas.
    assert_eq!(m.frame_at(t, own(31)).unwrap().0, PageSize::Large1);
}

#[test]
fn no_module_is_refused_for_its_size() {
    let machine = with(|m| {
        let entries = u64::from(m.geometry.entries);
        m.files[0].len = (entries + 1) * m.geometry.page();
        m.image_base = 64 << 20;
    });
    let chunk = machine.geometry.chunk();
    let machine = Machine {
        image_base: (64u64 << 20).next_multiple_of(chunk),
        ..machine
    };
    let (m, t) = boot(&machine);
    let pages = |slot| m.frame_at(t, own(slot)).map(|f| f.1);
    assert!(pages(29).is_some() && pages(30).is_some());
}

#[test]
fn message_page_frame_holds_the_binding_and_its_copy_the_mapping() {
    let (mut m, t) = boot(&Machine::example());
    m.apply(t, retype(25, Thread, 0, 40)).unwrap();
    for page in [10, 11] {
        let configure = Syscall::ThreadConfigure {
            thread: own(40),
            cspace: own(START_SLOT_CSPACE),
            aspace: SlotAddr::NONE,
            fault: SlotAddr::NONE,
            page: own(page),
        };
        assert_eq!(m.apply(t, configure), Err(Error::Busy), "slot {page}");
    }
    // Deleting the copy undoes the mapping; the binding stays.
    m.apply(t, Syscall::CapDelete(own(11))).unwrap();
    assert_eq!(m.translated(t, m.geometry().page()), None);
    let long = Message {
        words: vec![0; 7],
        ..Message::default()
    };
    m.apply(t, retype(25, Endpoint, 0, 41)).unwrap();
    let sent = Syscall::Send {
        target: own(41),
        message: long,
    };
    assert_eq!(m.apply(t, sent), Err(Error::WouldWait));
}

#[test]
fn first_thread_runs_on_its_context_of_63_64_of_the_core() {
    // Its registers are read in the tests of the register calls.
    let (mut m, t) = boot(&Machine::example());
    assert_eq!(m.running(), Some(t));
    let read = m
        .apply(t, Syscall::SchedRead(own(START_SLOT_SCHED)))
        .unwrap();
    let Output::SchedRead { remaining, .. } = read else {
        panic!("{read:?}");
    };
    assert_eq!(remaining, START_SCHED_BUDGET_NS as i64);
}

#[test]
fn boot_is_deterministic() {
    assert_eq!(boot(&Machine::example()), boot(&Machine::example()));
}

// The machine table.

#[test]
fn member_words_say_live_with_generation_1() {
    let (m, _) = boot(&Machine::example());
    for node in 0..2 {
        let word = m.member(node);
        assert_eq!((word.state(), word.generation()), (MemberState::Live, 1));
    }
}

#[test]
fn unit_roles_go_to_the_nodes_of_the_best_fence_class() {
    // Core 1 is of fence class 2, the boot core of class 1: both unit roles
    // are held by node 1; reserve and plain roles are vacant.
    let (sys, _) = System::boot(&Machine::example()).unwrap();
    for unit in 0..2 {
        assert_eq!(sys.role(unit).state(), RoleState::Held);
        assert_eq!(sys.role(unit).holder(), 1);
    }
    for role in 2..5 {
        assert_eq!(sys.role(role).state(), RoleState::Vacant);
    }
    // With equal classes, unit role j is held by node j mod n.
    let even = with(|m| m.cores[1].fence = 1);
    let (sys, _) = System::boot(&even).unwrap();
    assert_eq!((sys.role(0).holder(), sys.role(1).holder()), (0, 1));
}

#[test]
fn node_0_alone_holds_every_unit_role() {
    let (m, _) = boot(&Machine::example());
    for unit in 0..2 {
        assert_eq!(
            (m.role(unit).state(), m.role(unit).holder()),
            (RoleState::Held, 0)
        );
    }
}

#[test]
fn ownership_words_are_none_in_floors_reserves_name_their_reserve() {
    let (m, _) = boot(&Machine::example());
    let geometry = m.geometry();
    let chunk = geometry.chunk_shift();
    let ram = Machine::example().memory[4].base >> chunk;
    // Floors of three chunks for nodes 0 and 1: none.
    for c in ram..ram + 6 {
        assert_eq!(m.ownership(c), 0, "chunk {c}");
    }
    // Then the two reserves of one chunk each, then free and unclaimed.
    assert_eq!(
        m.ownership(ram + 6),
        Ownership::free(Owner::Reserve(0)).raw()
    );
    assert_eq!(
        m.ownership(ram + 7),
        Ownership::free(Owner::Reserve(1)).raw()
    );
    assert_eq!(
        m.ownership(ram + 8),
        Ownership::free(Owner::Unclaimed).raw()
    );
    // The crash areas at the top are kept: none.
    let top = (Machine::example().memory[4].base + Machine::example().memory[4].len) >> chunk;
    assert_eq!(m.ownership(top - 1), 0);
    assert_eq!(
        m.ownership(top - 3),
        Ownership::free(Owner::Unclaimed).raw()
    );
}

#[test]
fn device_memory_is_every_address_that_is_not_ram_kept_or_unusable() {
    // A reserved range from 3 MB to 7 MB, the rest of the low memory
    // unusable: entries of 1, 2 and 1 MB, then unused ones.
    let machine = with(|m| {
        let region = |base, len, kind| Region { base, len, kind };
        let mb = 1 << 20;
        let low = m.memory[3].base + m.memory[3].len;
        m.memory[2] = region(2 * mb, mb, RegionKind::Unusable);
        m.memory[3] = region(3 * mb, 4 * mb, RegionKind::Reserved);
        m.memory
            .insert(4, region(7 * mb, low - 7 * mb, RegionKind::Unusable));
    });
    let (m, _) = boot(&machine);
    // The entries below 2 MB hold the part of the usable second megabyte
    // left out of the chunks (Argon F2); then the device ranges.
    let used = m.device_count() - DEVICE_TABLE_SPARE;
    let low = (0..used)
        .filter(|&i| m.device_entry(i).base() < 2 << 20)
        .count();
    assert!((0..low).all(|i| m.device_entry(i).base() >= 1 << 20));
    let sizes: Vec<u32> = (low..used).map(|i| m.device_entry(i).log2()).collect();
    assert_eq!(sizes, [20, 21, 20]);
    assert!((0..used).all(|i| m.device_entry(i).is_used()));
    assert!(!m.device_entry(used).is_used());
    assert!(!m.device_entry(used + DEVICE_TABLE_SPARE - 1).is_used());
    // No device untyped is given at start-up.
    let (mut m, t) = boot(&Machine::example());
    assert_eq!(m.apply(t, take(1, 0, 0, 40)), Ok(Output::Done));
    let device = inspect(&mut m, t, 40).unwrap();
    assert_eq!(device, (Untyped, NOT_EXEC));
}

// The refusals.

#[test]
fn kernel_refuses_a_segment_below_the_first_large_page_or_beyond_the_user_range() {
    let geometry = Machine::example().geometry;
    let (chunk, page, end) = (geometry.chunk(), geometry.page(), geometry.user_end);
    let low = with(|m| m.segments[0].vaddr = chunk - page);
    assert_eq!(Model::boot(&low), Err(Refusal::SegmentPlace));
    // The second segment ending at the end of the user range is accepted,
    // a page beyond it refused.
    let len = Machine::example().segments[1].len;
    let top = with(|m| m.segments[1].vaddr = end - len);
    assert!(Model::boot(&top).is_ok());
    let high = with(|m| m.segments[1].vaddr = end - len + page);
    assert_eq!(Model::boot(&high), Err(Refusal::SegmentPlace));
}

#[test]
fn kernel_refuses_a_segment_that_is_writable_and_executable() {
    let both =
        with(|m| m.segments[1].rights = Rights::READ.union(Rights::WRITE).union(Rights::EXEC));
    assert_eq!(Model::boot(&both), Err(Refusal::WriteAndExec));
}

#[test]
fn kernel_refuses_a_start_up_page_of_more_than_63_pages() {
    let page = Machine::example().geometry.page() as u32;
    let long = with(|m| m.files[0].name = 63 * page);
    assert_eq!(Model::boot(&long), Err(Refusal::StartPage));
    let fits = with(|m| m.files[0].name = 61 * page);
    let (m, t) = boot(&fits);
    // 62 pages: mapped from 2 × P.
    let at = |n: u64| 2 * page as u64 + n * page as u64;
    assert!(m.translated(t, at(61)).is_some());
    assert!(m.translated(t, at(62)).is_none());
}

#[test]
fn kernel_refuses_segments_that_overlap_or_are_not_on_a_page() {
    let geometry = Machine::example().geometry;
    let (chunk, page) = (geometry.chunk(), geometry.page());
    let overlap = with(|m| m.segments[1].vaddr = chunk + 0x1_0000 - page);
    assert_eq!(Model::boot(&overlap), Err(Refusal::SegmentLayout));
    let unaligned = with(|m| m.segments[1].vaddr = chunk + 0x1_0000 + page / 2);
    assert_eq!(Model::boot(&unaligned), Err(Refusal::SegmentLayout));
}

#[test]
fn kernel_refuses_floors_that_do_not_fit_and_reserves_out_of_range() {
    let huge = with(|m| m.floors.floor_node = 1 << 40);
    assert_eq!(Model::boot(&huge), Err(Refusal::Floors));
    for k in [1, 9] {
        let reserves = with(|m| m.floors.reserves = k);
        assert_eq!(Model::boot(&reserves), Err(Refusal::Floors), "k = {k}");
    }
    // One core: one reserve is accepted.
    let single = with(|m| {
        m.cores.truncate(1);
        m.floors.reserves = 1;
    });
    assert!(Model::boot(&single).is_ok());
    let plain = with(|m| m.floors.plain_roles = ROLES_PLAIN_MAX);
    assert_eq!(Model::boot(&plain), Err(Refusal::Floors));
}

#[test]
fn kernel_refuses_times_of_membership_out_of_their_ranges() {
    for (stale, stuck, sweep) in [
        (9, 100, 1000),
        (100, 1001, 1000),
        (100, 100, 99),
        (100, 100, 60_001),
    ] {
        let times = with(|m| {
            m.membership.stale_ms = stale;
            m.membership.stuck_ms = stuck;
            m.membership.sweep_ms = sweep;
        });
        assert_eq!(
            Model::boot(&times),
            Err(Refusal::Floors),
            "{stale} {stuck} {sweep}"
        );
    }
    let edges = with(|m| {
        m.membership.stale_ms = 10;
        m.membership.stuck_ms = 1000;
        m.membership.sweep_ms = 60_000;
    });
    assert!(Model::boot(&edges).is_ok());
}

#[test]
fn kernel_refuses_machine_values_out_of_range() {
    let wide = with(|m| m.geometry.entries = 8192);
    assert_eq!(Model::boot(&wide), Err(Refusal::MachineValues));
    let many = with(|m| {
        let core = m.cores[1];
        m.cores = (0..=NODES_MAX as u64)
            .map(|hw_id| dodos_model::Core { hw_id, ..core })
            .collect();
    });
    assert_eq!(Model::boot(&many), Err(Refusal::MachineValues));
}

#[test]
fn ports_exist_only_on_x86_64() {
    let other = with(|m| m.arch = Arch::Other);
    let (mut m, t) = boot(&other);
    assert_eq!(kind(&mut m, t, START_SLOT_IOPORTS), Err(Error::Invalid));
}

/// Argon F2: the parts of usable memory left out of the chunks are device
/// memory, so no usable page is dropped: every page of a usable region is
/// in a chunk (its ownership word is not none, or it lies in a floor or a
/// reserve), kept, or under a used entry of the device table.
#[test]
fn no_usable_page_is_left_out_of_both_the_chunks_and_the_device_table() {
    let machine = Machine::example();
    let (m, _) = Model::boot(&machine).unwrap();
    let geometry = m.geometry();
    let page = geometry.page();
    let entries: Vec<_> = (0..m.device_count())
        .map(|i| m.device_entry(i))
        .filter(|e| e.is_used())
        .map(|e| (e.base(), e.base() + (1 << e.log2())))
        .collect();
    let device = |at: u64| entries.iter().any(|&(s, e)| s <= at && at < e);
    let mut fragments = 0;
    for region in machine
        .memory
        .iter()
        .filter(|r| r.kind == RegionKind::Usable)
    {
        let (start, end) = (
            region.base.next_multiple_of(page),
            (region.base + region.len) / page * page,
        );
        for at in (start..end).step_by(page as usize) {
            if !m.chunked_or_kept(at) {
                assert!(device(at), "usable page {at:#x} dropped");
                fragments += 1;
            }
        }
    }
    assert!(fragments > 0, "the example machine has a fragment");
}

/// Mistral F7: the generator is seeded from the start-up page's seed always,
/// and from the processor's source where the kernel can read one; with
/// neither the kernel does not start.
#[test]
fn kernel_refuses_a_machine_with_no_seed_and_no_processor_source() {
    let none = with(|m| {
        m.boot_seed = 0;
        m.processor_entropy = false;
    });
    assert_eq!(Model::boot(&none).err(), Some(Refusal::NoEntropy));
    let seed_only = with(|m| m.processor_entropy = false);
    assert!(Model::boot(&seed_only).is_ok());
    let source_only = with(|m| m.boot_seed = 0);
    assert!(Model::boot(&source_only).is_ok());
}

/// Mistral F1, F2: a machine with a core that no other core can stop (fence
/// class 0) runs one node, unless the image declares the hardening monitor
/// (every core then of class 2) or accepts an unfenced core.
#[test]
fn a_machine_with_a_core_of_fence_class_0_runs_one_node_unless_declared() {
    let mut machine = Machine::example();
    machine.cores[1].fence = 0;
    assert_eq!(machine.nodes(), 1);
    let (sys, _) = System::boot(&machine).unwrap();
    assert!(sys.has(0));
    let mut monitored = machine.clone();
    monitored.monitor = true;
    assert_eq!(monitored.nodes(), 2);
    let (m, _) = Model::boot(&monitored).unwrap();
    assert!(m.core_list_fences().iter().all(|&c| c == 2));
    let mut accepted = machine.clone();
    accepted.accept_unfenced = true;
    assert_eq!(accepted.nodes(), 2);
    assert!(System::boot(&accepted).is_ok());
}

/// DODOS_PAGE selects the geometry of the machine of the tests (Argon F3,
/// mutant 119).
#[test]
fn the_geometry_of_the_tests_is_the_one_dodos_page_names() {
    let page = Machine::example().geometry.page();
    match std::env::var("DODOS_PAGE").as_deref() {
        Ok("16k") => assert_eq!(page, 16 << 10),
        Ok("32k") => assert_eq!(page, 32 << 10),
        _ => assert_eq!(page, 4 << 10),
    }
}

/// Argon F3: the boot test of a start-up page of exactly 63 pages (mutant 122).
#[test]
fn a_start_up_page_of_exactly_63_pages_is_accepted() {
    // #119: when DODOS_PAGE=16k, Machine::example() uses 16 KB pages
    if std::env::var("DODOS_PAGE").as_deref() == Ok("16k") {
        assert_eq!(Machine::example().geometry.page(), 16_384);
    }
    // #122: start-up page of exactly 63 pages (START_PAGE_MAX_PAGES) is accepted
    let page = Machine::example().geometry.page() as u32;
    let fits63 = with(|m| m.files[0].name = 62 * page);
    let (m, t) = boot(&fits63);
    let at = |n: u64| 2 * page as u64 + n * page as u64;
    assert!(m.translated(t, at(62)).is_some());
    assert!(m.translated(t, at(63)).is_none());
}
