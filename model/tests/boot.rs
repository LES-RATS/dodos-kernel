//! Start-up state: the state the kernel builds from the machine and the boot
//! image, one test per rule.

use dodos_abi::ObjectKind::{
    ASpace, CSpace, Core, Debug, Frame, IoPorts, Iommu, IrqControl, Sched, Task, Thread, Untyped,
};
use dodos_abi::*;
use dodos_model::*;

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

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

fn derive(source: u32, destination: u32) -> Syscall {
    Syscall::CapDerive {
        source: own(source),
        destination: own(destination),
        rights: Rights::ALL.bits(),
        badge: 0,
    }
}

/// The bytes of the untyped in `slot`: the largest child untyped it can make,
/// made and deleted again in a free slot.
fn bytes(m: &mut Model, t: ObjId, slot: u32) -> u64 {
    let free = 4000;
    for log in (12..=40).rev() {
        if m.apply(t, retype(slot, Untyped, 1 << log, free)).is_ok() {
            m.apply(t, Syscall::CapDelete(own(free))).unwrap();
            return 1 << log;
        }
    }
    0
}

#[test]
fn start_up_space_holds_the_slots_of_the_table_in_their_order() {
    let (mut m, t) = boot(&Machine::example());
    let fixed = [
        CSpace, ASpace, Thread, Sched, Debug, IrqControl, IoPorts, Task, Frame, Frame, Frame,
    ];
    for (slot, wanted) in (1..).zip(fixed) {
        assert_eq!(kind(&mut m, t, slot), Ok(wanted), "slot {slot}");
    }
    for slot in [0, 12, 13, 14, 15] {
        assert_eq!(kind(&mut m, t, slot), Err(Error::Invalid), "slot {slot}");
    }
    // From 16: the cores, the IOMMU unit, the image untyped, the ordinary
    // untyped, the device untyped, the three modules; then empty slots.
    let machine = [
        Core, Core, Iommu, Untyped, Untyped, Untyped, Frame, Frame, Frame,
    ];
    for (slot, wanted) in (16..).zip(machine) {
        assert_eq!(kind(&mut m, t, slot), Ok(wanted), "slot {slot}");
    }
    assert_eq!(kind(&mut m, t, 25), Err(Error::Invalid));
    assert_eq!(m.object_at(t, own(START_SLOT_THREAD)), Some(t));
}

#[test]
fn start_up_space_has_4096_empty_slots_or_more_in_whole_segments() {
    let (mut m, t) = boot(&Machine::example());
    // 25 slots used and 4,096 free: 4,121, so 33 segments of 128.
    assert_eq!(m.apply(t, derive(20, 4223)), Ok(Output::Done));
    assert_eq!(m.apply(t, derive(20, 4224)), Err(Error::Invalid));
    // It never grows.
    let grow = Syscall::CSpaceGrow {
        cspace: own(START_SLOT_CSPACE),
        untyped: own(20),
    };
    assert_eq!(m.apply(t, grow), Err(Error::Invalid));
}

#[test]
fn start_up_capabilities_have_every_right_device_memory_without_exec() {
    let (mut m, t) = boot(&Machine::example());
    for slot in (1..12).chain(16..21).chain(22..25) {
        assert_eq!(
            inspect(&mut m, t, slot).unwrap().1,
            Rights::ALL,
            "slot {slot}"
        );
    }
    let device = inspect(&mut m, t, 21).unwrap().1;
    assert_eq!(device, Rights::ALL.without(Rights::EXEC));
}

/// Derives copies from `slot` into the slots from 4000, and returns how many
/// were accepted before the depth of 64 refused one.
fn copies_to_the_bottom(m: &mut Model, t: ObjId, slot: u32) -> u32 {
    let mut from = slot;
    for made in 0..70 {
        let to = 4000 + made;
        if m.apply(t, derive(from, to)) != Ok(Output::Done) {
            return made;
        }
        from = to;
    }
    panic!("no limit");
}

#[test]
fn start_up_capabilities_are_at_depth_1_the_frames_at_2_and_the_copy_at_3() {
    for (slot, depth) in [(20, 1), (START_SLOT_TASK, 1), (22, 2), (10, 2), (11, 3)] {
        let (mut m, t) = boot(&Machine::example());
        let below = copies_to_the_bottom(&mut m, t, slot);
        assert_eq!(below, TREE_DEPTH as u32 - depth, "slot {slot}");
    }
}

#[test]
fn untypeds_follow_the_shares_and_the_blocks_of_the_rules() {
    let (mut m, t) = boot(&Machine::example());
    // The image region of 512 KB, the share of node 0 (4 GB of the 8), and
    // the device range of 2 MB.
    assert_eq!(bytes(&mut m, t, 20), 4 * GB);
    assert_eq!(bytes(&mut m, t, 21), 2 * MB);
}

#[test]
fn image_untyped_is_taken_after_the_message_page() {
    let (mut m, t) = boot(&Machine::example());
    // The modules take 112 KB, the start-up page and the message page 8 KB:
    // 120 KB of 512 KB. An endpoint at 120 KB, a child of 256 KB at 256 KB,
    // and then nothing.
    let endpoint = retype(19, ObjectKind::Endpoint, 0, 31);
    assert_eq!(m.apply(t, endpoint), Ok(Output::Done));
    let child = retype(19, Untyped, 1 << 18, 30);
    assert_eq!(m.apply(t, child), Ok(Output::Done));
    let full = retype(19, Untyped, 4096, 32);
    assert_eq!(m.apply(t, full), Err(Error::NoMemory));
}

#[test]
fn image_memory_is_usable_again_once_every_frame_of_the_boot_image_is_deleted() {
    let (mut m, t) = boot(&Machine::example());
    let whole = retype(19, Untyped, 1 << 19, 30);
    assert_eq!(m.apply(t, whole.clone()), Err(Error::NoMemory));
    // The first thread loses its message page with slot 10; it still runs.
    for slot in [22, 23, 24, 9, 11, 10] {
        assert_eq!(m.apply(t, Syscall::CapDelete(own(slot))), Ok(Output::Done));
    }
    assert_eq!(m.apply(t, whole), Ok(Output::Done));
}

#[test]
fn ordinary_untypeds_are_dirty_from_their_start() {
    let (mut m, t) = boot(&Machine::example());
    let frame = Syscall::UntypedRetype {
        untyped: own(20),
        kind: Frame,
        size: 32,
        page: 0,
        memory: 0,
        destination: own(30),
    };
    assert_eq!(m.apply(t, frame.clone()), Err(Error::Again));
    assert_eq!(m.apply(t, frame), Ok(Output::Done));
}

#[test]
fn address_space_maps_the_segments_the_message_page_and_the_start_up_page() {
    let (m, t) = boot(&Machine::example());
    let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
    let mapped = |address| m.translated(t, address);
    assert_eq!(mapped(0x20_0000), Some(r.union(x)));
    assert_eq!(mapped(0x20_ffff), Some(r.union(x)));
    assert_eq!(mapped(0x21_0000), Some(r.union(w)));
    assert_eq!(mapped(0x21_3fff), Some(r.union(w)));
    assert_eq!(mapped(0x21_4000), None);
    assert_eq!(mapped(x86_64::START_MESSAGE_PAGE_ADDR), Some(r.union(w)));
    assert_eq!(mapped(x86_64::START_PAGE_ADDR), Some(r));
    // One page of start-up page, and nothing else.
    assert_eq!(mapped(x86_64::START_PAGE_ADDR + 0x1000), None);
    assert_eq!(mapped(0), None);
}

#[test]
fn message_page_frame_holds_the_binding_and_its_copy_the_mapping() {
    let (mut m, t) = boot(&Machine::example());
    m.apply(t, retype(20, Thread, 0, 30)).unwrap();
    for page in [10, 11] {
        let configure = Syscall::ThreadConfigure {
            thread: own(30),
            cspace: own(START_SLOT_CSPACE),
            aspace: SlotAddr::NONE,
            fault: SlotAddr::NONE,
            page: own(page),
        };
        assert_eq!(m.apply(t, configure), Err(Error::Busy), "slot {page}");
    }
    // Deleting the copy undoes the mapping; the binding stays.
    m.apply(t, Syscall::CapDelete(own(11))).unwrap();
    assert_eq!(m.translated(t, x86_64::START_MESSAGE_PAGE_ADDR), None);
    let long = Message {
        words: vec![0; 7],
        ..Message::default()
    };
    m.apply(t, retype(20, ObjectKind::Endpoint, 0, 31)).unwrap();
    let sent = Syscall::Send {
        target: own(31),
        message: long,
    };
    assert_eq!(m.apply(t, sent), Err(Error::WouldWait));
}

#[test]
fn boot_is_deterministic() {
    assert_eq!(boot(&Machine::example()), boot(&Machine::example()));
}

/// The machine of the tests, changed.
fn with(change: impl FnOnce(&mut Machine)) -> Machine {
    let mut machine = Machine::example();
    change(&mut machine);
    machine
}

#[test]
fn shares_split_ordinary_memory_by_ascending_address_in_2_mb_units() {
    // Three cores: each node gets floor(8 GB / 3 / 2 MB) × 2 MB.
    let three = with(|m| {
        m.cores.push(dodos_model::Core {
            apic_id: 1,
            class: 0,
        })
    });
    let (mut m, t) = boot(&three);
    let share = 8 * GB / 3 / (2 * MB) * (2 * MB);
    // The share from 4 GB, in naturally aligned blocks: 2 GB first.
    assert_eq!(bytes(&mut m, t, 21), 2 * GB);
    let mut total = 0;
    let mut slot = 21;
    while kind(&mut m, t, slot) == Ok(Untyped) && total < share {
        total += bytes(&mut m, t, slot);
        slot += 1;
    }
    assert_eq!(total, share);
    // Then the device untyped: nothing of the share is left over.
    assert_eq!(bytes(&mut m, t, slot), 2 * MB);
    assert!(!inspect(&mut m, t, slot).unwrap().1.contains(Rights::EXEC));
    // A single core takes all the ordinary memory: two blocks of 4 GB.
    let one = with(|m| m.cores.truncate(1));
    let (mut m, t) = boot(&one);
    assert_eq!(bytes(&mut m, t, 19), 4 * GB);
    assert_eq!(bytes(&mut m, t, 20), 4 * GB);
    assert_eq!(bytes(&mut m, t, 21), 2 * MB);
}

#[test]
fn device_memory_is_every_address_that_is_not_ram_kept_image_or_unusable() {
    // A reserved range from 3 MB to 7 MB, the rest of the first 4 GB
    // unusable: blocks of 1, 2 and 1 MB.
    let machine = with(|m| {
        let region = |base, len, kind| Region { base, len, kind };
        m.memory[2] = region(2 * MB, MB, RegionKind::Unusable);
        m.memory[3] = region(3 * MB, 4 * MB, RegionKind::Reserved);
        m.memory
            .insert(4, region(7 * MB, 4 * GB - 7 * MB, RegionKind::Unusable));
    });
    let (mut m, t) = boot(&machine);
    let sizes: Vec<u64> = (21..24).map(|slot| bytes(&mut m, t, slot)).collect();
    assert_eq!(sizes, [MB, 2 * MB, MB]);
    for slot in 21..24 {
        assert!(!inspect(&mut m, t, slot).unwrap().1.contains(Rights::EXEC));
    }
    assert_eq!(kind(&mut m, t, 24), Ok(Frame));
}

#[test]
fn kernel_refuses_a_segment_below_2_mb_or_beyond_the_user_range() {
    let low = with(|m| m.segments[0].vaddr = 0x1f_f000);
    assert_eq!(Model::boot(&low), Err(Refusal::SegmentPlace));
    // The second segment is 16 KB: ending at the end of the user range is
    // accepted, one page beyond it refused.
    let top = with(|m| m.segments[1].vaddr = (255 << 39) - 0x4000);
    assert!(Model::boot(&top).is_ok());
    let high = with(|m| m.segments[1].vaddr = (255 << 39) - 0x3000);
    assert_eq!(Model::boot(&high), Err(Refusal::SegmentPlace));
}

#[test]
fn single_node_takes_the_ordinary_memory_that_is_not_a_multiple_of_2_mb() {
    let odd = with(|m| {
        m.cores.truncate(1);
        m.memory[4].len += 4096;
        m.memory[5].base += 4096;
        m.memory[5].len -= 4096;
    });
    let (mut m, t) = boot(&odd);
    // 4 GB, 4 GB, then the last page, then the device untyped.
    assert_eq!(bytes(&mut m, t, 21), 4096);
    assert_eq!(bytes(&mut m, t, 22), 2 * MB);
}

#[test]
fn kernel_refuses_a_segment_that_is_writable_and_executable() {
    let both =
        with(|m| m.segments[1].rights = Rights::READ.union(Rights::WRITE).union(Rights::EXEC));
    assert_eq!(Model::boot(&both), Err(Refusal::WriteAndExec));
}

#[test]
fn kernel_refuses_a_start_up_page_of_more_than_63_pages() {
    let long = with(|m| m.files[0].name = 63 * 4096);
    assert_eq!(Model::boot(&long), Err(Refusal::StartPage));
    let fits = with(|m| m.files[0].name = 61 * 4096);
    let (m, t) = boot(&fits);
    // 62 pages: mapped from 0x2000.
    assert!(
        m.translated(t, x86_64::START_PAGE_ADDR + 61 * 4096)
            .is_some()
    );
    assert!(
        m.translated(t, x86_64::START_PAGE_ADDR + 62 * 4096)
            .is_none()
    );
}

#[test]
fn kernel_refuses_a_module_larger_than_one_frame_and_segments_that_overlap() {
    let large = with(|m| m.files[0].len = 2 * MB + 1);
    assert_eq!(Model::boot(&large), Err(Refusal::ModuleSize));
    let overlap = with(|m| m.segments[1].vaddr = 0x20_f000);
    assert_eq!(Model::boot(&overlap), Err(Refusal::SegmentLayout));
    let unaligned = with(|m| m.segments[1].vaddr = 0x21_0800);
    assert_eq!(Model::boot(&unaligned), Err(Refusal::SegmentLayout));
}
