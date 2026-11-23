//! Memory and devices: one test per rule of the contract, on the start-up
//! space of `Machine::example` (its untypeds in slots 19 to 21, its IOMMU
//! unit in slot 18).

mod common;

use dodos_abi::ObjectKind::{ASpace, Endpoint, Frame, IoSpace, Thread, Untyped};
use dodos_abi::*;
use dodos_model::*;

const IMAGE: u32 = 19;
const UT: u32 = 20;
/// The node's table-floor untyped, which the spaces' tables come from
/// (D-154).
const TB: u32 = common::TABLES_INDEX;
const DEV: u32 = 21;
const IOMMU: u32 = 18;
const START: u32 = START_SLOT_ASPACE;
const MB: u64 = 1 << 20;
/// An address of the user range where the first process maps nothing.
const AT: u64 = 1 << 30;
const R: Rights = Rights::READ;
const W: Rights = Rights::WRITE;
const X: Rights = Rights::EXEC;
const DONE: Outcome = Ok(Output::Done);

fn s(index: u32) -> SlotAddr {
    SlotAddr::own(common::slot(index))
}

fn rw() -> Rights {
    R.union(W)
}

struct K {
    m: Model,
    t: ObjId,
}

impl K {
    fn new() -> Self {
        Self::on(&Machine::example())
    }

    fn on(machine: &Machine) -> Self {
        let (mut m, t) = Model::boot(machine).unwrap();
        common::take_device(&mut m, t);
        Self { m, t }
    }

    fn go(&mut self, call: Syscall) -> Outcome {
        // A call that waits for a unit (an attach, an unmap of an iospace
        // that serves devices) ends at the unit's confirmation, which node 0,
        // the holder of every unit role, checks at its next step.
        let outcome = self.m.apply_settled(self.t, call);
        assert_eq!(self.m.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, call: Syscall) {
        assert_eq!(self.go(call), DONE);
    }

    #[track_caller]
    fn no(&mut self, call: Syscall, error: Error) {
        assert_eq!(self.go(call), Err(error));
    }

    /// Repeats a call while it returns `again`; at most `calls` times.
    #[track_caller]
    fn done(&mut self, call: Syscall, calls: u32) -> u32 {
        for made in 1..=calls {
            match self.go(call.clone()) {
                DONE => return made,
                Err(Error::Again) => {}
                other => panic!("{call:?}: {other:?}"),
            }
        }
        panic!("{call:?} does not end in {calls} calls");
    }

    fn retype(&mut self, untyped: u32, kind: ObjectKind, size: u64, slot: u32) {
        let call = Syscall::UntypedRetype {
            untyped: s(untyped),
            kind,
            size: common::sized(kind, size),
            page: 0,
            memory: 0,
            destination: s(slot),
        };
        self.done(call, 2);
    }

    /// A frame of `pages` pages of `page` size from `untyped`.
    fn frame(&mut self, untyped: u32, page: PageSize, pages: u64, memory: MemoryType, slot: u32) {
        let call = Syscall::UntypedRetype {
            untyped: s(untyped),
            kind: Frame,
            size: pages,
            page: page as u64,
            memory: memory as u64,
            destination: s(slot),
        };
        self.done(call, 40_000);
    }

    fn small(&mut self, pages: u64, slot: u32) {
        self.frame(UT, PageSize::Page, pages, MemoryType::WriteBack, slot);
    }

    fn derive(&mut self, from: u32, to: u32, rights: Rights) {
        self.ok(Syscall::CapDerive {
            source: s(from),
            destination: s(to),
            rights: rights.bits(),
            badge: 0,
        });
    }

    fn at(&self, address: u64) -> Option<Rights> {
        self.m.translated(self.t, address)
    }

    /// Values of the machine: a page, a first large page, the bytes that a
    /// table of level `k` covers (P × E^k), and the levels.
    fn page(&self) -> u64 {
        self.m.geometry().page()
    }

    fn large(&self) -> u64 {
        self.m.geometry().chunk()
    }

    fn span(&self, level: u32) -> u64 {
        let geometry = self.m.geometry();
        geometry.page() << (level * geometry.entries.trailing_zeros())
    }

    fn levels(&self) -> u32 {
        self.m.geometry().levels
    }

    /// A block of `chunks` chunks (a power of two) in `slot`.
    fn run(&mut self, chunks: u64, slot: u32) {
        let log2 = self.m.geometry().chunk_shift() + chunks.trailing_zeros();
        common::take_run(&mut self.m, self.t, log2, common::slot(slot));
        assert_eq!(self.m.check(), Ok(()));
    }
}

fn map(
    space: u32,
    frame: u32,
    untyped: u32,
    address: u64,
    first: u64,
    count: u64,
    rights: Rights,
) -> Syscall {
    Syscall::ASpaceMap(Map {
        space: s(space),
        frame: s(frame),
        untyped: s(untyped),
        address,
        first,
        count,
        rights: rights.bits(),
    })
}

fn iomap(space: u32, frame: u32, address: u64, count: u64, rights: Rights) -> Syscall {
    Syscall::IoSpaceMap(Map {
        space: s(space),
        frame: s(frame),
        untyped: s(TB),
        address,
        first: 0,
        count,
        rights: rights.bits(),
    })
}

fn protect(space: u32, frame: u32, rights: Rights) -> Syscall {
    Syscall::ASpaceProtect {
        aspace: s(space),
        frame: s(frame),
        rights: rights.bits(),
    }
}

fn attach(iospace: u32, device: u64) -> Syscall {
    Syscall::IoSpaceAttach {
        iommu: s(IOMMU),
        iospace: s(iospace),
        device,
    }
}

/// An address space in slot 30 made from the ordinary untyped, and a
/// suspended thread bound to it, by which `translated` reads it.
fn with_aspace() -> (K, ObjId) {
    let mut k = K::new();
    k.retype(UT, ASpace, 0, 30);
    k.retype(UT, Thread, 0, 31);
    k.ok(Syscall::ThreadConfigure {
        thread: s(31),
        cspace: s(START_SLOT_CSPACE),
        aspace: s(30),
        fault: SlotAddr::NONE,
        page: SlotAddr::NONE,
    });
    let viewer = k.m.object_at(k.t, s(31)).unwrap();
    (k, viewer)
}

fn seen(k: &K, viewer: ObjId, address: u64) -> Option<Rights> {
    k.m.translated(viewer, address)
}

// Calls on address spaces: aspace_map.

#[test]
fn aspace_map_maps_pages_first_to_first_plus_count_minus_1_from_the_address() {
    let (mut k, v) = with_aspace();
    k.small(4, 40);
    k.ok(map(30, 40, TB, AT, 1, 2, rw()));
    assert_eq!(seen(&k, v, AT - 1), None);
    assert_eq!(seen(&k, v, AT), Some(rw()));
    let page = k.page();
    assert_eq!(seen(&k, v, AT + 2 * page - 1), Some(rw()));
    assert_eq!(seen(&k, v, AT + 2 * page), None);
}

#[test]
fn aspace_map_needs_write_on_the_aspace_and_a_live_aspace() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.derive(30, 32, R);
    k.no(map(32, 40, 0, AT, 0, 1, R), Error::Denied);
    k.no(map(40, 40, 0, AT, 0, 1, R), Error::Invalid);
    k.no(map(33, 40, 0, AT, 0, 1, R), Error::Invalid);
}

#[test]
fn aspace_map_checks_the_frame_slot_and_kind_without_its_rights() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.derive(40, 41, Rights::NONE);
    k.retype(UT, Endpoint, 0, 42);
    // A frame of the wrong kind is `invalid`, with an untyped that is not
    // the aspace's as well (both `invalid`: no order shows; the final
    // review's F2 Opus 32).
    k.no(map(30, 42, IMAGE, AT, 0, 1, R), Error::Invalid);
    // A frame without rights passes its slot check; the rights asked for
    // are judged with the plain arguments, after the untyped.
    k.no(map(30, 41, IMAGE, AT, 0, 1, R), Error::Invalid);
    k.no(map(30, 41, TB, AT, 0, 1, R), Error::Denied);
}

#[test]
fn untyped_of_aspace_map_is_its_own_or_0() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.small(1, 41);
    // A copy of the table untyped named when the aspace was made is
    // accepted (D-154).
    k.derive(TB, 32, Rights::ALL.without(X));
    k.ok(map(30, 40, 32, AT, 0, 1, R));
    k.derive(TB, 33, R);
    let page = k.page();
    k.no(map(30, 41, 33, AT + page, 0, 1, R), Error::Denied);
    // The ordinary untyped its header came from is not its table untyped.
    k.no(map(30, 41, UT, AT + page, 0, 1, R), Error::Invalid);
    k.no(map(30, 41, IMAGE, AT + page, 0, 1, R), Error::Invalid);
    // Refused with bad plain arguments as well (all `invalid`: no order
    // shows).
    k.no(map(30, 41, IMAGE, AT + 1, 0, 9, X), Error::Invalid);
}

#[test]
fn start_up_aspace_takes_no_untyped_and_uses_its_reserve() {
    let mut k = K::new();
    k.small(1, 40);
    k.no(map(START, 40, TB, AT, 0, 1, R), Error::Invalid);
    // The reserve of L tables: a new region of a root entry needs L - 1.
    let (root, levels) = (k.span(k.levels() - 1), k.levels());
    k.ok(map(START, 40, 0, root, 0, 1, R));
    assert_eq!(k.at(root), Some(R));
    // Another region needs L - 1 more: no memory, and nothing is taken.
    k.small(1, 41);
    k.no(map(START, 41, 0, 2 * root, 0, 1, R), Error::NoMemory);
    // A new last table in the first region: one table, from the reserve.
    k.ok(map(START, 41, 0, root + k.span(1), 0, 1, R));
    assert!(levels >= 3);
}

/// Each plain argument of `aspace_map` is refused (all `invalid`, so no
/// order among them shows); the range before the rights does show (the
/// final review's F2 Opus 32).
#[test]
fn each_plain_argument_of_aspace_map_is_refused_and_the_range_before_the_rights() {
    let (mut k, _) = with_aspace();
    k.run(2, 35);
    k.frame(35, PageSize::Large1, 2, MemoryType::WriteBack, 40);
    let page = k.large();
    // The address is a multiple of the frame's page size.
    k.no(map(30, 40, TB, AT + k.page(), 0, 1, R), Error::Invalid);
    // The first page is in the frame.
    k.no(map(30, 40, TB, AT, 2, 1, R), Error::Invalid);
    // The page count is at least 1 and stays in the frame.
    k.no(map(30, 40, TB, AT, 0, 0, R), Error::Invalid);
    k.no(map(30, 40, TB, AT, 1, 2, R), Error::Invalid);
    // The range lies inside the user range.
    let end = k.m.geometry().user_end;
    k.no(map(30, 40, TB, end - page, 0, 2, R), Error::Invalid);
    k.ok(map(30, 40, TB, end - 2 * page, 0, 2, R));
    // The range before the rights: a bad range with rights the frame has
    // not.
    k.ok(Syscall::ASpaceUnmap(s(40)));
    k.derive(40, 41, R);
    k.no(map(30, 41, TB, AT + 1, 0, 1, rw()), Error::Invalid);
}

#[test]
fn rights_of_a_mapping_are_read_and_write_or_execute_within_the_frame() {
    let (mut k, v) = with_aspace();
    k.small(1, 40);
    for word in [
        W,
        W.union(X),
        R.union(W).union(X),
        Rights::SEND,
        Rights::NONE,
    ] {
        k.no(map(30, 40, TB, AT, 0, 1, word), Error::Invalid);
    }
    k.derive(40, 41, R.union(X));
    k.no(map(30, 41, TB, AT, 0, 1, rw()), Error::Denied);
    k.ok(map(30, 41, TB, AT, 0, 1, R.union(X)));
    assert_eq!(seen(&k, v, AT), Some(R.union(X)));
}

#[test]
fn plain_arguments_come_before_the_state() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    k.no(map(30, 40, TB, AT + 1, 0, 1, R), Error::Invalid);
    k.no(map(30, 40, TB, AT, 0, 1, R), Error::Busy);
}

#[test]
fn full_record_is_busy_even_for_this_aspace() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    let page = k.page();
    k.no(map(30, 40, TB, AT + page, 0, 1, R), Error::Busy);
    // The binding of a message page is a full record too.
    k.no(
        map(30, START_SLOT_MESSAGE_PAGE, TB, AT + page, 0, 1, R),
        Error::Busy,
    );
}

#[test]
fn translated_address_of_any_page_size_is_busy() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.small(1, 41);
    k.run(2, 35);
    k.frame(35, PageSize::Large1, 1, MemoryType::WriteBack, 42);
    k.frame(35, PageSize::Large1, 1, MemoryType::WriteBack, 43);
    let (page, large) = (k.page(), k.large());
    k.ok(map(30, 40, TB, AT + 5 * page, 0, 1, R));
    // A large page over a page.
    k.no(map(30, 42, TB, AT, 0, 1, R), Error::Busy);
    k.ok(map(30, 42, TB, AT + large, 0, 1, R));
    // A page inside a large page, and the same page twice.
    k.no(map(30, 41, TB, AT + large + 3 * page, 0, 1, R), Error::Busy);
    k.no(map(30, 43, TB, AT + large, 0, 1, R), Error::Busy);
}

/// Tables come from the aspace's untyped, then, unmapped, from its free
/// list; once the list is empty they come from the untyped again (the
/// final review's F2 Opus 28: the name had the order reversed and the
/// third map was missing).
#[test]
fn tables_come_from_the_untyped_then_from_the_free_list_then_from_the_untyped() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.small(1, 41);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    // Unmapped: its L − 1 tables go to the free list.
    k.ok(Syscall::ASpaceUnmap(s(40)));
    // Untyped 0 supplies none, but the free list does.
    let root = k.span(k.levels() - 1);
    k.ok(map(30, 41, 0, 3 * root, 0, 1, R));
    k.no(map(30, 40, 0, AT, 0, 1, R), Error::NoMemory);
    // The list is empty: the untyped supplies them again.
    k.ok(map(30, 40, TB, AT, 0, 1, R));
}

#[test]
fn tables_beyond_what_the_untyped_holds_are_no_memory_and_nothing_is_taken() {
    let mut k = K::new();
    // A child table untyped of 8 pages: the aspace's root takes 1, child
    // untypeds of a page take all but L − 2: one table short (D-154).
    let (page, short) = (k.page(), u64::from(k.levels()) - 2);
    k.retype(TB, Untyped, 8 * page, 32);
    k.retype(UT, ASpace, s(32).raw(), 30);
    for slot in 0..8 - 1 - short {
        k.retype(32, Untyped, page, 50 + slot as u32);
    }
    k.small(1, 40);
    k.no(map(30, 40, 32, AT, 0, 1, R), Error::NoMemory);
    // Nothing was taken: a child of the L − 2 pages left still fits.
    k.retype(32, Untyped, short * page, 33);
}

#[test]
fn tables_count_as_live_pieces_until_the_aspace_is_destroyed() {
    let mut k = K::new();
    let page = k.page();
    k.retype(TB, Untyped, 8 * page, 32);
    k.retype(UT, ASpace, s(32).raw(), 30);
    k.small(1, 40);
    k.ok(map(30, 40, 32, AT, 0, 1, R));
    k.ok(Syscall::ASpaceUnmap(s(40)));
    // The root and L − 1 tables on its free list, pieces of the table
    // untyped (D-154): the rest is left, but not the whole untyped.
    k.retype(32, Untyped, page, 33);
    k.ok(Syscall::CapDelete(s(33)));
    k.no(
        Syscall::UntypedRetype {
            untyped: s(32),
            kind: Untyped,
            size: 8 * page,
            page: 0,
            memory: 0,
            destination: s(33),
        },
        Error::NoMemory,
    );
    // The aspace destroyed, the whole untyped is free again.
    k.ok(Syscall::CapDelete(s(30)));
    k.retype(32, Untyped, 8 * page, 33);
}

#[test]
fn large_pages_map_with_the_frame_s_page_size() {
    let (mut k, v) = with_aspace();
    k.run(1, 35);
    k.frame(35, PageSize::Large1, 1, MemoryType::WriteBack, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    let large = k.large();
    assert_eq!(seen(&k, v, AT + large - 1), Some(R));
    assert_eq!(seen(&k, v, AT + large), None);
}

#[test]
fn device_frame_maps_into_an_aspace_without_exec() {
    let (mut k, v) = with_aspace();
    k.frame(DEV, PageSize::Page, 1, MemoryType::Uncached, 40);
    k.no(map(30, 40, TB, AT, 0, 1, R.union(X)), Error::Denied);
    k.ok(map(30, 40, TB, AT, 0, 1, rw()));
    assert_eq!(seen(&k, v, AT), Some(rw()));
}

// aspace_unmap and aspace_protect.

#[test]
fn aspace_unmap_needs_no_right_and_succeeds_on_an_empty_record() {
    let (mut k, v) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    k.derive(40, 41, Rights::NONE);
    k.ok(Syscall::ASpaceUnmap(s(41)));
    assert_eq!(seen(&k, v, AT), Some(R));
    k.ok(Syscall::CapMove {
        source: s(40),
        destination: s(42),
    });
    k.ok(Syscall::ASpaceUnmap(s(42)));
    assert_eq!(seen(&k, v, AT), None);
    k.ok(Syscall::ASpaceUnmap(s(42)));
}

#[test]
fn aspace_unmap_is_busy_on_an_iospace_mapping_or_a_binding() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.small(1, 40);
    k.ok(iomap(30, 40, 0, 1, rw()));
    k.no(Syscall::ASpaceUnmap(s(40)), Error::Busy);
    k.no(
        Syscall::ASpaceUnmap(s(START_SLOT_MESSAGE_PAGE)),
        Error::Busy,
    );
    k.no(
        Syscall::IoSpaceUnmap(s(START_SLOT_MESSAGE_PAGE_MAP)),
        Error::Busy,
    );
    k.ok(Syscall::IoSpaceUnmap(s(40)));
}

#[test]
fn aspace_unmap_checks_the_frame_slot_and_kind() {
    let mut k = K::new();
    k.retype(UT, Endpoint, 0, 40);
    k.no(Syscall::ASpaceUnmap(s(40)), Error::Invalid);
    k.no(Syscall::ASpaceUnmap(s(41)), Error::Invalid);
}

#[test]
fn record_that_names_a_dying_aspace_is_unmapped_as_usual() {
    let (mut k, _) = with_aspace();
    // 33 frames mapped: destroying the aspace takes two calls.
    for n in 0..33 {
        k.small(1, 40 + n);
        k.ok(map(30, 40 + n, TB, AT + k.page() * u64::from(n), 0, 1, R));
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    // The most recently mapped records are emptied first: the first one is
    // still full, and is unmapped by the call.
    k.ok(Syscall::ASpaceUnmap(s(40)));
    k.no(map(30, 40, TB, AT, 0, 1, R), Error::Gone);
    k.ok(Syscall::CapDelete(s(30)));
}

#[test]
fn destroying_an_aspace_empties_its_records_most_recent_first_then_unbinds_its_threads() {
    let (mut k, _) = with_aspace();
    for n in 0..32 {
        k.small(1, 40 + n);
        k.ok(map(30, 40 + n, TB, AT + k.page() * u64::from(n), 0, 1, R));
    }
    // 32 records, one thread, the root: the first call empties the records
    // of slots 71 down to 40, the second unbinds the thread.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.retype(UT, ASpace, 0, 32);
    k.ok(map(32, 40, TB, AT, 0, 1, R));
    k.ok(Syscall::CapDelete(s(30)));
}

#[test]
fn aspace_protect_replaces_the_rights_of_the_whole_mapping() {
    let (mut k, v) = with_aspace();
    k.small(2, 40);
    k.ok(map(30, 40, TB, AT, 0, 2, rw()));
    k.ok(protect(30, 40, R));
    assert_eq!(seen(&k, v, AT + k.page()), Some(R));
    // It can also add rights, and the same rights change nothing.
    k.ok(protect(30, 40, R.union(X)));
    k.ok(protect(30, 40, R.union(X)));
    assert_eq!(seen(&k, v, AT), Some(R.union(X)));
}

#[test]
fn aspace_protect_checks_the_aspace_the_frame_the_rights_then_the_record() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.small(1, 41);
    k.retype(UT, ASpace, 0, 32);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    k.derive(30, 33, R);
    k.no(protect(33, 40, R), Error::Denied);
    k.no(protect(30, 32, R), Error::Invalid);
    k.no(protect(30, 40, W), Error::Invalid);
    // A record in another space, empty, or a binding.
    k.no(protect(32, 40, R), Error::Busy);
    k.no(protect(30, 41, R), Error::Busy);
    k.no(protect(30, START_SLOT_MESSAGE_PAGE, R), Error::Busy);
    // The rights before the record.
    k.derive(41, 42, R);
    k.no(protect(30, 42, rw()), Error::Denied);
}

// Mapping records.

#[test]
fn derived_frame_capability_has_an_empty_record_and_maps_the_same_pages_again() {
    let (mut k, v) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, rw()));
    k.derive(40, 41, R.union(X));
    k.ok(map(30, 41, TB, AT + MB, 0, 1, R.union(X)));
    assert_eq!(seen(&k, v, AT), Some(rw()));
    assert_eq!(seen(&k, v, AT + MB), Some(R.union(X)));
}

#[test]
fn removing_a_frame_capability_undoes_its_mapping_and_revoke_keeps_its_own() {
    let (mut k, v) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    k.derive(40, 41, Rights::ALL);
    k.ok(map(30, 41, TB, AT + MB, 0, 1, R));
    k.ok(Syscall::CapRevoke(s(40)));
    assert_eq!(seen(&k, v, AT + MB), None);
    assert_eq!(seen(&k, v, AT), Some(R));
    k.ok(Syscall::CapDelete(s(40)));
    assert_eq!(seen(&k, v, AT), None);
}

#[test]
fn cap_inspect_says_nothing_of_the_record() {
    let (mut k, _) = with_aspace();
    k.small(1, 40);
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    let inspected = Output::Inspect {
        kind: Frame,
        rights: Rights::ALL,
        badge: 0,
    };
    assert_eq!(k.go(Syscall::CapInspect(s(40))), Ok(inspected));
}

// IOMMU.

#[test]
fn iospace_map_judges_the_page_size_first_then_the_width_and_the_rights() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.run(1, 35);
    k.frame(35, PageSize::Large1, 1, MemoryType::WriteBack, 40);
    // The units do not support large pages: `unsupported`, before an
    // address that is not even aligned.
    k.no(iomap(30, 40, 1, 1, R), Error::Unsupported);
    k.small(1, 41);
    k.no(iomap(30, 41, 1 << 48, 1, R), Error::Invalid);
    for rights in [Rights::NONE, X, R.union(X)] {
        k.no(iomap(30, 41, 0, 1, rights), Error::Invalid);
    }
    k.derive(41, 42, R);
    k.no(iomap(30, 42, 0, 1, W), Error::Denied);
    // Write only, and device memory, are accepted.
    k.ok(iomap(30, 41, (1 << 48) - k.page(), 1, W));
    k.frame(DEV, PageSize::Page, 1, MemoryType::Uncached, 43);
    k.ok(iomap(30, 43, 0, 1, rw()));
}

#[test]
fn iospace_of_39_bits_has_three_levels_of_tables() {
    let mut machine = Machine::example();
    machine.io_width = 39;
    let mut k = K::on(&machine);
    // Four pages of a table untyped: the root takes one, and two tables
    // suffice.
    k.retype(TB, Untyped, 4 * k.page(), 32);
    k.retype(UT, IoSpace, s(32).raw(), 30);
    k.small(1, 40);
    k.ok(Syscall::IoSpaceMap(Map {
        space: s(30),
        frame: s(40),
        untyped: s(32),
        address: 0,
        first: 0,
        count: 1,
        rights: R.bits(),
    }));
    k.no(iomap(30, 40, 1 << 39, 1, R), Error::Invalid);
}

#[test]
fn iospace_attach_judges_the_state_in_the_order_of_the_contract() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.retype(UT, IoSpace, 0, 31);
    // Higher bits than 16 are `invalid`; a device the kernel uses `busy`.
    k.no(attach(30, 1 << 16), Error::Invalid);
    k.no(attach(30, 0xf8), Error::Busy);
    k.ok(attach(30, 0x10));
    // Attached to another iospace; to this one again.
    k.no(attach(31, 0x10), Error::Busy);
    k.ok(attach(30, 0x10));
    // 16 devices at most.
    for device in 0x11..0x20 {
        k.ok(attach(30, device));
    }
    k.no(attach(30, 0x20), Error::Busy);
    // Detaching frees the place; detaching what is not attached succeeds.
    k.ok(attach(0, 0x10));
    k.ok(attach(0, 0x10));
    k.ok(attach(30, 0x20));
}

#[test]
fn iospace_attach_needs_manage_on_the_iommu_and_on_the_iospace() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.derive(IOMMU, 32, R);
    k.derive(30, 33, R);
    let weak = |iommu, iospace| Syscall::IoSpaceAttach {
        iommu: s(iommu),
        iospace: s(iospace),
        device: 0x10,
    };
    k.no(weak(32, 30), Error::Denied);
    k.no(weak(IOMMU, 33), Error::Denied);
    k.no(weak(30, 30), Error::Invalid);
}

#[test]
fn unit_without_a_free_domain_number_is_busy() {
    // The fewest domain numbers a unit has, 16 (VT-d's ND = 0; the start-up
    // refuses fewer, D-141), shared out by node slot: number 0 is never used
    // and the last is the kernel's own (Devices and shared hardware, D-134).
    let mut machine = Machine::example();
    machine.iommus[0].domains = 16;
    let per = (16 - 2) / machine.nodes() as u32;
    let mut k = K::on(&machine);
    for i in 0..=per {
        k.retype(UT, IoSpace, 0, 30 + i);
    }
    for i in 0..per {
        k.ok(attach(30 + i, 0x10 + u64::from(i)));
    }
    // A second device of the same iospace needs no new number.
    k.ok(attach(30, 0x40));
    k.no(attach(30 + per, 0x41), Error::Busy);
    // The number is freed with the last device.
    k.ok(attach(0, 0x10));
    k.ok(attach(0, 0x40));
    k.ok(attach(30 + per, 0x41));
}

#[test]
fn iospace_serves_devices_of_one_unit() {
    let mut machine = Machine::example();
    machine.iommus.push(IommuUnit {
        segment: 1,
        domains: 16,
    });
    let mut k = K::on(&machine);
    // The second unit's capability is in slot 19, then the roles (one more
    // unit role), then the untypeds: the start-up one in slot 26, the one
    // of a chunk in 27 and the table floor's in 28 (D-154); this test
    // takes the first (the final review's F2 Opus 29).
    k.ok(Syscall::UntypedRetype {
        untyped: SlotAddr::own(26),
        kind: IoSpace,
        size: SlotAddr::own(28).raw(),
        page: 0,
        memory: 0,
        destination: s(30),
    });
    k.ok(attach(30, 0x10));
    let other = Syscall::IoSpaceAttach {
        iommu: SlotAddr::own(IOMMU + 1),
        iospace: s(30),
        device: 0x10,
    };
    k.no(other, Error::Busy);
}

#[test]
fn destroying_an_iospace_detaches_its_devices_then_empties_its_records() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.retype(UT, IoSpace, 0, 31);
    for device in 0x10..0x20 {
        k.ok(attach(30, device));
    }
    for n in 0..17 {
        k.small(1, 40 + n);
        k.ok(iomap(30, 40 + n, k.page() * u64::from(n), 1, R));
    }
    // 16 devices and 16 records in the first call; the device of slot 0x10
    // was attached first and is detached in it.
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    // The device is being detached: its attach word is not 0 until the
    // detach's answer (D-137).
    k.no(attach(31, 0x10), Error::Busy);
    k.m.settle();
    k.ok(attach(31, 0x10));
    // The first record is still full.
    k.no(iomap(31, 40, 0, 1, R), Error::Busy);
    k.ok(Syscall::CapDelete(s(30)));
    k.ok(iomap(31, 40, 0, 1, R));
}

#[test]
fn dying_iospace_is_gone_for_map_and_attach_and_its_devices_detach() {
    let mut k = K::new();
    k.retype(UT, IoSpace, 0, 30);
    k.ok(attach(30, 0x10));
    for n in 0..40 {
        k.small(1, 40 + n);
        k.ok(iomap(30, 40 + n, k.page() * u64::from(n), 1, R));
    }
    k.no(Syscall::CapDelete(s(30)), Error::Again);
    k.no(iomap(30, 40, 0, 1, R), Error::Gone);
    k.no(attach(30, 0x11), Error::Gone);
    // Its detach is in flight: one request at a time per device (D-139).
    k.no(attach(0, 0x10), Error::Busy);
    k.m.settle();
    k.ok(attach(0, 0x10));
}

/// D-131 (the eighth review's N5): the `iommu` object, a machine object,
/// keeps no list; removing its root removes its descendants and destroys
/// nothing, and the node keeps its own devices attached.
#[test]
fn removing_the_iommu_root_destroys_nothing_and_keeps_the_devices() {
    let mut k = K::new();
    for slot in 30..33 {
        k.retype(UT, IoSpace, 0, slot);
    }
    for n in 0..40u64 {
        k.ok(attach(30 + (n / 16) as u32, 0x10 + n));
    }
    k.ok(Syscall::CapDelete(s(IOMMU)));
    assert_eq!(k.m.census().devices, 40);
    // Destroying an iospace still detaches its devices: its root goes once
    // their detaches are answered (D-137).
    // A device leaves the header at its detach's answer (D-139).
    k.no(Syscall::CapDelete(s(32)), Error::Again);
    assert_eq!(k.m.census().devices, 40);
    k.m.settle();
    assert_eq!(k.m.census().devices, 32);
    k.ok(Syscall::CapDelete(s(32)));
}

#[test]
fn iommu_with_no_device_attached_is_destroyed_in_one_step() {
    let mut k = K::new();
    k.ok(Syscall::CapDelete(s(IOMMU)));
    assert_eq!(k.go(Syscall::CapInspect(s(IOMMU))), Err(Error::Invalid));
}

/// Argon F3 (mutants 129, 130, 131): on the 16 KB geometry the tables of a
/// space follow its page and its E, and an iospace of 47 bits has three
/// levels of 2,048 entries.
#[test]
fn tables_follow_the_page_and_e_of_the_sixteen_kb_geometry() {
    // Every call followed by the whole check, as through `K::go` (the
    // final review's F2 Opus 31).
    fn checked(m: &mut Model, t: ObjId, call: Syscall) -> Outcome {
        let outcome = m.apply(t, call);
        assert_eq!(m.check(), Ok(()));
        outcome
    }
    // #129, #130, #131: tables() and io_levels() use geometry.page_shift[0] and geometry.entries on 16 KB geometry
    let mut mach = Machine::example_on(SIXTEEN_KB);
    mach.io_width = 47;
    let (mut m, t) = Model::boot(&mach).unwrap();
    let page = m.geometry().page();
    let chunk = m.geometry().chunk();
    let ut = SlotAddr::own(25);
    // The table floor's untyped (D-154).
    let tables = SlotAddr::own(common::TABLES);
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::UntypedRetype {
                untyped: ut,
                kind: ObjectKind::ASpace,
                size: tables.raw(),
                page: 0,
                memory: 0,
                destination: SlotAddr::own(100),
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::UntypedRetype {
                untyped: ut,
                kind: ObjectKind::Frame,
                size: 2,
                page: PageSize::Page as u64,
                memory: 0,
                destination: SlotAddr::own(101),
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::CapDerive {
                source: SlotAddr::own(101),
                destination: SlotAddr::own(102),
                rights: Rights::READ.bits(),
                badge: 0,
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::ASpaceMap(Map {
                space: SlotAddr::own(100),
                frame: SlotAddr::own(101),
                untyped: tables,
                address: chunk,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            }),
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::ASpaceMap(Map {
                space: SlotAddr::own(100),
                frame: SlotAddr::own(102),
                untyped: SlotAddr::NONE,
                address: chunk + (1 << 23),
                first: 1,
                count: 1,
                rights: Rights::READ.bits(),
            }),
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::CapDerive {
                source: SlotAddr::own(101),
                destination: SlotAddr::own(104),
                rights: Rights::READ.bits(),
                badge: 0,
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::UntypedRetype {
                untyped: tables,
                kind: ObjectKind::Untyped,
                size: 4 * page,
                page: 0,
                memory: 0,
                destination: SlotAddr::own(105),
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::UntypedRetype {
                untyped: ut,
                kind: ObjectKind::IoSpace,
                size: SlotAddr::own(105).raw(),
                page: 0,
                memory: 0,
                destination: SlotAddr::own(106),
            },
        ),
        Ok(Output::Done)
    );
    assert_eq!(
        checked(
            &mut m,
            t,
            Syscall::IoSpaceMap(Map {
                space: SlotAddr::own(106),
                frame: SlotAddr::own(104),
                untyped: SlotAddr::own(105),
                address: chunk,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            }),
        ),
        Ok(Output::Done)
    );
}

/// Argon section 2.3 of the seventh review: survivors #066, #067, #069,
/// #070, #072 and #083 (the continuation of a partial mapping, the budget of
/// a gate call, the bound of a protect).
#[test]
fn r7_aspace_map_partial_and_gate_chunks_boundaries() {
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut k = K::on(&machine);
    k.retype(UT, ASpace, 0, 30);
    k.retype(UT, ASpace, 0, 31);
    k.small(2, 40);
    // #072: mapping 1 page (cost 1 == gate_chunks 1) completes in 1 call (Done)
    k.ok(map(30, 40, TB, AT, 0, 1, R));
    // #066: completed mapping (partial == false, count == 1 < 2) cannot be continued as count: 2
    k.no(map(30, 40, TB, AT, 0, 2, R), Error::Busy);
    // #083: aspace_protect on 1 chunk (touched == 1 == gate_chunks) succeeds
    k.ok(protect(30, 40, rw()));
    k.ok(Syscall::ASpaceUnmap(s(40)));

    // A device frame of 3 pages, 2 of them mapped (cost = 1 per page, so
    // count: 2 takes 2 steps with gate_chunks = 1)
    k.frame(DEV, PageSize::Page, 3, MemoryType::Uncached, 41);
    assert_eq!(k.go(map(30, 41, TB, AT, 0, 2, R)), Err(Error::Again));
    // #067: continuing on another aspace (31) returns Busy
    k.no(map(31, 41, TB, AT, 0, 2, R), Error::Busy);
    // #069: continuing with another first page (1) returns Busy
    k.no(map(30, 41, TB, AT, 1, 2, R), Error::Busy);
    // #070: continuing with different rights (rw()) returns Busy
    k.no(map(30, 41, TB, AT, 0, 2, rw()), Error::Busy);
    // Matching continuation finishes with Done
    k.ok(map(30, 41, TB, AT, 0, 2, R));
    // #072: with gate_chunks = 2, mapping 2 device pages (spent + c == 2 == budget at step 1) finishes in 1 call (Done)!
    let mut m2 = Machine::example();
    m2.gate_chunks = 2;
    let mut k2 = K::on(&m2);
    k2.retype(UT, ASpace, 0, 30);
    k2.frame(DEV, PageSize::Page, 2, MemoryType::Uncached, 41);
    k2.ok(map(30, 41, TB, AT, 0, 2, R));
}
