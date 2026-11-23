//! `memory_control` and its calls: blocks taken from the ownership words
//! and the device table, given, split and destroyed (Memory and devices
//! between nodes), on the geometry of the run.

use dodos_abi::ObjectKind::{Endpoint, Frame, Untyped};
use dodos_abi::*;
use dodos_model::*;

const CONTROL: u32 = START_SLOT_MEMORY_CONTROL;
const DONE: Outcome = Ok(Output::Done);

fn own(index: u32) -> SlotAddr {
    SlotAddr::own(index)
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
        let (m, t) = Model::boot(machine).unwrap();
        Self { m, t }
    }

    fn go(&mut self, call: Syscall) -> Outcome {
        let outcome = self.m.apply(self.t, call);
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

    fn chunk(&self) -> u64 {
        self.m.geometry().chunk()
    }

    fn shift(&self) -> u64 {
        u64::from(self.m.geometry().chunk_shift())
    }

    /// The first chunk of ordinary memory past the floors and reserves.
    fn free(&self) -> u64 {
        Machine::example().memory[5].base + 10 * self.chunk()
    }

    /// The index of the device entry of the 2 MB device range at 2 MB.
    fn dev(&self) -> u64 {
        (0..self.m.device_count())
            .find(|&i| self.m.device_entry(i).base() == Some(Machine::DEVICES))
            .expect("the device range") as u64
    }
}

fn take(kind: u64, place: u64, size: u64, destination: u32) -> Syscall {
    take_through(CONTROL, kind, place, size, destination)
}

fn take_through(control: u32, kind: u64, place: u64, size: u64, destination: u32) -> Syscall {
    Syscall::MemoryTake {
        control: own(control),
        kind,
        place,
        size,
        destination: own(destination),
    }
}

fn give(untyped: u32, node: u64) -> Syscall {
    Syscall::MemoryGive {
        control: own(CONTROL),
        untyped: own(untyped),
        node,
    }
}

fn split(untyped: u32) -> Syscall {
    Syscall::MemorySplit {
        control: own(CONTROL),
        untyped: own(untyped),
    }
}

fn retype(untyped: u32, kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: own(untyped),
        kind,
        size,
        page: 0,
        memory: (kind == Frame && matches!(untyped, 41 | 42)) as u64,
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

fn inspect(k: &mut K, slot: u32) -> (ObjectKind, Rights) {
    match k.go(Syscall::CapInspect(own(slot))) {
        Ok(Output::Inspect { kind, rights, .. }) => (kind, rights),
        other => panic!("{other:?}"),
    }
}

// memory_take.

#[test]
fn memory_take_makes_a_block_of_a_run_taken_by_this_node() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(0, free, shift + 1, 40));
    assert_eq!(inspect(&mut k, 40), (Untyped, Rights::ALL));
    for chunk in [free >> shift, (free >> shift) + 1] {
        let word = Ownership::from_raw(k.m.ownership(chunk)).unwrap();
        assert_eq!(word, Ownership::taken(Owner::Node(0), 1));
    }
    // Its memory is dirty: a frame of 128 KB takes the steps of 64 KB.
    let pages = (128 << 10) / k.m.geometry().page();
    let frame = Syscall::UntypedRetype {
        untyped: own(40),
        kind: Frame,
        size: pages,
        page: 0,
        memory: 0,
        destination: own(41),
    };
    k.no(frame.clone(), Error::Again);
    k.ok(frame);
}

#[test]
fn memory_take_needs_manage_on_a_memory_control() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(derive(CONTROL, 40, Rights::ALL.without(Rights::MANAGE)));
    k.no(take_through(40, 0, free, shift, 41), Error::Denied);
    k.no(
        take_through(START_SLOT_DEBUG, 0, free, shift, 41),
        Error::Invalid,
    );
    k.no(take_through(39, 0, free, shift, 41), Error::Invalid);
    // The memory_control before the destination.
    k.no(take_through(40, 0, free, shift, 0), Error::Denied);
    k.no(take(0, free, shift, 0), Error::Invalid);
}

/// Each plain argument of `memory_take` is refused, all with `invalid`, so
/// no order among them shows; the plain arguments before the state, which
/// does show, is the last pair (the final review's F2 Opus 1).
#[test]
fn each_plain_argument_of_memory_take_is_refused_and_before_the_state() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    let entries = u64::from(k.m.geometry().entries.trailing_zeros());
    // The kind, before the size and the place: kind 3 names none (kind 2
    // takes a table run since D-154; the audit of lot 8b, F6: it stood
    // here as the bad kind, refused for its place).
    k.no(take(3, free, shift, 40), Error::Invalid);
    // Kind 0: the size from the chunk to the chunk times E.
    k.no(take(0, free, shift - 1, 40), Error::Invalid);
    k.no(take(0, free, shift + entries + 1, 40), Error::Invalid);
    // The place is a multiple of the run, the run is in the table.
    k.no(take(0, free + k.chunk(), shift + 1, 40), Error::Invalid);
    let words = 1u64 << Machine::example().phys_bits;
    k.no(take(0, words, shift, 40), Error::Invalid);
    k.no(take(0, u64::MAX << shift, shift, 40), Error::Invalid);
    // No word of the run is none: a floor, the low memory, a crash area.
    let floor = Machine::example().memory[5].base;
    k.no(take(0, floor, shift, 40), Error::Invalid);
    k.no(take(0, 0, shift, 40), Error::Invalid);
    // Kind 1: the size is 0, the place below the capacity.
    k.no(take(1, 0, 1, 40), Error::Invalid);
    let capacity = k.m.device_count() as u64;
    k.no(take(1, capacity, 0, 40), Error::Invalid);
    // An unused entry is a state error, not a plain one.
    k.no(take(1, capacity - 1, 0, 40), Error::Busy);
    // Kind 2 at a place and size kind 0 accepts is no plain fault.
    k.ok(take(2, free, shift, 40));
}

/// The depth rule refuses a take through a memory_control at depth 64 and
/// accepts one at 63. Both a bad kind (3) and the depth give `invalid`, so
/// the order the contract states between them cannot be seen from the
/// result (the final review's F2 Opus 1: the test was named after that
/// order).
#[test]
fn a_take_through_a_memory_control_at_depth_64_is_refused_and_at_63_accepted() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    // A copy of the memory_control at depth 64.
    let mut from = CONTROL;
    for slot in 100..163 {
        k.ok(derive(from, slot, Rights::ALL));
        from = slot;
    }
    k.no(take_through(162, 3, free, shift, 40), Error::Invalid);
    k.no(take_through(162, 0, free, shift, 40), Error::Invalid);
    k.ok(take_through(161, 0, free, shift, 40));
}

#[test]
fn state_of_memory_take_is_judged_in_order() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    let chunk = free >> shift;
    // Taken by this node's current generation.
    k.ok(take(0, free, shift, 40));
    k.no(take(0, free, shift, 41), Error::Busy);
    // A word that fails its check counts as owned by another live node.
    let next = free + k.chunk();
    k.m.stray_write(chunk + 1, Ownership::free(Owner::Unclaimed).raw() ^ 1 << 40);
    k.no(take(0, next, shift, 41), Error::Busy);
    k.m.stray_write(chunk + 1, Ownership::free(Owner::Unclaimed).raw());
    // A reserve whose role this node does not hold.
    // Past the two node slots' floors of four chunks (D-154).
    let reserve = Machine::example().memory[5].base + 8 * k.chunk();
    k.no(take(0, reserve, shift, 41), Error::Busy);
    // The state before the destination.
    k.no(take(0, reserve, shift, 40), Error::Busy);
    k.no(take(0, next, shift, 40), Error::NoSlot);
    // The device entry: an unused one, then a taken one.
    k.ok(take(1, k.dev(), 0, 41));
    k.no(take(1, k.dev(), 0, 42), Error::Busy);
}

#[test]
fn a_word_given_to_another_live_node_is_refused() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(0, free, shift, 40));
    k.ok(give(40, 1));
    let word = Ownership::from_raw(k.m.ownership(free >> shift)).unwrap();
    assert_eq!(word, Ownership::free(Owner::Node(1)));
    k.no(take(0, free, shift, 40), Error::Busy);
}

#[test]
fn block_rights_have_exec_only_for_ordinary_memory_through_exec() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(1, k.dev(), 0, 41));
    assert_eq!(
        inspect(&mut k, 41),
        (Untyped, Rights::ALL.without(Rights::EXEC))
    );
    k.ok(derive(CONTROL, 42, Rights::ALL.without(Rights::EXEC)));
    k.ok(take_through(42, 0, free, shift, 43));
    assert_eq!(
        inspect(&mut k, 43),
        (Untyped, Rights::ALL.without(Rights::EXEC))
    );
    // A device block is device memory: no endpoint, a frame in one call.
    k.no(retype(41, Endpoint, 0, 44), Error::Invalid);
    k.ok(retype(41, Frame, 4, 44));
}

// memory_give.

#[test]
fn memory_give_destroys_the_block_and_frees_its_words_for_the_node() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(0, free, shift + 1, 40));
    k.ok(give(40, 1));
    assert_eq!(k.go(Syscall::CapInspect(own(40))), Err(Error::Invalid));
    for chunk in [free >> shift, (free >> shift) + 1] {
        let word = Ownership::from_raw(k.m.ownership(chunk)).unwrap();
        assert_eq!(word, Ownership::free(Owner::Node(1)));
    }
    // A device entry is given the same way.
    k.ok(take(1, k.dev(), 0, 41));
    k.ok(give(41, 1));
    assert_eq!(
        k.m.device_entry(k.dev() as usize).owner,
        Ownership::free(Owner::Node(1))
    );
}

#[test]
fn checks_of_memory_give_come_in_order() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(0, free, shift, 40));
    // An untyped that is no block, and a copy of a block, are `invalid`.
    k.no(give(25, 1), Error::Invalid);
    k.ok(derive(40, 41, Rights::ALL));
    k.no(give(41, 1), Error::Invalid);
    k.ok(derive(40, 42, Rights::READ));
    k.no(give(42, 1), Error::Denied);
    // The node: below n, and not this node.
    k.no(give(40, 2), Error::Invalid);
    k.no(give(40, 0), Error::Invalid);
    // A descendant, then a live piece.
    k.no(give(40, 1), Error::Busy);
    k.ok(Syscall::CapDelete(own(41)));
    k.ok(Syscall::CapDelete(own(42)));
    k.ok(retype(40, Endpoint, 0, 43));
    k.no(give(40, 1), Error::Busy);
    k.ok(Syscall::CapDelete(own(43)));
    k.ok(give(40, 1));
}

#[test]
fn destroying_a_block_frees_its_words_with_the_same_owner() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(take(0, free, shift, 40));
    k.ok(Syscall::CapDelete(own(40)));
    let word = Ownership::from_raw(k.m.ownership(free >> shift)).unwrap();
    assert_eq!(word, Ownership::free(Owner::Node(0)));
    // Its own free word is taken again (rule 1).
    k.ok(take(0, free, shift, 40));
}

#[test]
fn revoking_the_memory_control_destroys_the_blocks_taken_through_it() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    k.ok(derive(CONTROL, 40, Rights::ALL));
    k.ok(take_through(40, 0, free, shift, 41));
    k.ok(take_through(40, 1, k.dev(), 0, 42));
    k.ok(Syscall::CapRevoke(own(40)));
    assert_eq!(k.go(Syscall::CapInspect(own(41))), Err(Error::Gone));
    let word = Ownership::from_raw(k.m.ownership(free >> shift)).unwrap();
    assert_eq!(word, Ownership::free(Owner::Node(0)));
    assert_eq!(
        k.m.device_entry(k.dev() as usize).owner,
        Ownership::free(Owner::Node(0))
    );
}

// memory_split.

#[test]
fn memory_split_cuts_a_device_entry_into_two_halves() {
    let mut k = K::new();
    k.ok(take(1, k.dev(), 0, 40));
    let entry = k.m.device_entry(k.dev() as usize);
    // The lowest unused entry that is unclaimed gets the upper half.
    let used = (k.m.device_count() - DEVICE_TABLE_SPARE) as u64;
    assert_eq!(k.go(split(40)), Ok(Output::Index(used)));
    assert_eq!(k.go(Syscall::CapInspect(own(40))), Err(Error::Invalid));
    let (lower, upper) = (
        k.m.device_entry(k.dev() as usize),
        k.m.device_entry(used as usize),
    );
    assert_eq!(
        (lower.base(), lower.log2()),
        (entry.base(), entry.log2() - 1)
    );
    let half = 1 << (entry.log2() - 1);
    assert_eq!(
        (upper.base(), upper.log2()),
        (entry.base().map(|base| base + half), entry.log2() - 1)
    );
    assert!(upper.is_used());
    for e in [lower, upper] {
        assert_eq!(e.owner, Ownership::free(Owner::Node(0)));
    }
    // Both halves are this node's to take.
    k.ok(take(1, used, 0, 41));
    k.ok(take(1, k.dev(), 0, 42));
}

#[test]
fn checks_of_memory_split_come_in_order() {
    let mut k = K::new();
    let (free, shift) = (k.free(), k.shift());
    // A block of ordinary memory, and a device block of one page.
    k.ok(take(0, free, shift, 40));
    k.no(split(40), Error::Invalid);
    k.ok(take(1, k.dev(), 0, 41));
    let page = k.m.geometry().page().trailing_zeros();
    let halvings = k.m.device_entry(k.dev() as usize).log2() - page;
    let used = (k.m.device_count() - DEVICE_TABLE_SPARE) as u64;
    for _ in 0..halvings {
        let Ok(Output::Index(_)) = k.go(split(41)) else {
            panic!("a split");
        };
        k.ok(take(1, k.dev(), 0, 41));
    }
    assert_eq!(k.m.device_entry(k.dev() as usize).log2(), page);
    k.no(split(41), Error::Invalid);
    // A descendant, then a live piece.
    k.ok(take(1, used, 0, 42));
    k.ok(derive(42, 43, Rights::READ));
    k.no(split(42), Error::Busy);
    k.ok(Syscall::CapDelete(own(43)));
    k.ok(retype(42, Frame, 1, 44));
    k.no(split(42), Error::Busy);
}

#[test]
fn memory_split_is_busy_when_no_entry_is_unused_and_unclaimed() {
    // A device range of 2^33 bytes: 8,192 splits use every spare entry.
    let machine = {
        let mut m = Machine::example();
        let top = 1u64 << m.phys_bits;
        m.memory.push(Region {
            base: top,
            len: 1 << 33,
            kind: RegionKind::Reserved,
        });
        m.memory.push(Region {
            base: top + (1 << 33),
            len: top - (1 << 33),
            kind: RegionKind::Unusable,
        });
        m.phys_bits += 1;
        m
    };
    let mut k = K::on(&machine);
    let used = k.m.device_count() - DEVICE_TABLE_SPARE;
    let big = (0..used)
        .find(|&i| k.m.device_entry(i).log2() == 33)
        .unwrap() as u64;
    let mut work = vec![big];
    let mut splits = 0;
    // The invariants are checked after every split (Argon b2 of the seventh
    // review: a check every 512 splits let a wrong write between two of them
    // pass).
    while splits < DEVICE_TABLE_SPARE {
        let entry = work.pop().expect("halves left to split");
        assert_eq!(k.m.apply(k.t, take(1, entry, 0, 40)), Ok(Output::Done));
        let Ok(Output::Index(upper)) = k.m.apply(k.t, split(40)) else {
            panic!("split {splits}");
        };
        assert_eq!(k.m.check(), Ok(()));
        work.insert(0, entry);
        work.insert(0, upper);
        splits += 1;
    }
    let entry = work.pop().unwrap();
    k.ok(take(1, entry, 0, 40));
    k.no(split(40), Error::Busy);
}
