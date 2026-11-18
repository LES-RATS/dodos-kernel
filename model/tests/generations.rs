//! Words held by a generation that no longer exists (review 7: Argon 1.1,
//! Opus S1, S2, X1). Rule 3 of taking judges the generation of a word
//! *taken*: once its owner has started again, a word taken by the earlier
//! generation is free for every live node, as the reclaim barrier says
//! ("After dead, every ownership word and device entry owned by X is free
//! for any live node"). A word *free* with a node as owner stays that
//! node's while it lives.
//!
//! Every test runs on `System`, which delivers the messages of one ring in
//! the order they were sent, and checks every invariant after every event.

use dodos_abi::ObjectKind::IoSpace;
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const MS: u64 = 1_000_000;
const UT: SlotAddr = SlotAddr::own(25);
const CONTROL: SlotAddr = SlotAddr::own(START_SLOT_MEMORY_CONTROL);
const IOMMU: SlotAddr = SlotAddr::own(18);
/// A device identifier no kernel uses.
const DEV: u64 = 0x0010;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn iospace(destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind: IoSpace,
        size: 0,
        page: 0,
        memory: 0,
        destination: s(destination),
    }
}

fn attach(iospace: SlotAddr, device: u64) -> Syscall {
    Syscall::IoSpaceAttach {
        iommu: IOMMU,
        iospace,
        device,
    }
}

fn take(kind: u64, place: u64, size: u64, destination: u32) -> Syscall {
    Syscall::MemoryTake {
        control: CONTROL,
        kind,
        place,
        size,
        destination: s(destination),
    }
}

struct S {
    sys: System,
    t: Vec<ObjId>,
}

impl S {
    fn new() -> Self {
        let (sys, t) = System::boot(&Machine::example()).unwrap();
        Self { sys, t }
    }

    #[track_caller]
    fn call(&mut self, node: u16, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    /// A call that succeeds, at once or after the role holder answers.
    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        let outcome = self.call(node, call);
        assert!(outcome == DONE || outcome == WAITS, "{outcome:?}");
        self.run(MS);
        if outcome == WAITS {
            let woken = self.sys.node(node).woken(self.t[node as usize]);
            assert_eq!(woken, Some(&DONE));
        }
    }

    #[track_caller]
    fn run(&mut self, ns: u64) {
        let to = self.sys.now() + ns;
        while self.sys.now() < to {
            let at = (self.sys.now() + MS / 10).min(to);
            self.sys.run(at, MS / 10);
            assert_eq!(self.sys.check(), Ok(()));
        }
    }

    /// Node 0 kills node 1; the machine runs until node 1 is dead.
    #[track_caller]
    fn kill_1(&mut self) {
        let stop = Syscall::NodeStop {
            node: s(17),
            mode: NODE_STOP_KILL,
            core: 0,
        };
        assert_eq!(self.call(0, stop), DONE);
        self.run(10 * MS);
        let member = self.sys.member(1);
        assert_eq!(
            (member.state(), member.generation()),
            (MemberState::Dead, 1)
        );
    }

    /// Node 0 starts node 1 fresh: generation 2.
    #[track_caller]
    fn restart_1(&mut self) {
        let start = Syscall::NodeStart {
            node: s(17),
            mode: NODE_START_FRESH,
            image: 0,
            module: 0,
            window: 0,
            mask: 1 << 1,
        };
        assert_eq!(self.call(0, start), DONE);
        self.run(MS);
        let member = self.sys.member(1);
        assert_eq!(
            (member.state(), member.generation()),
            (MemberState::Live, 2)
        );
        self.t[1] = self.sys.node(1).object_at_start(START_SLOT_THREAD);
        self.run(MS);
    }

    /// A run of one chunk well past the floors and the reserves.
    fn free_run(&self) -> (u64, u64) {
        let geometry = self.sys.node(0).geometry();
        let chunk = geometry.chunk();
        let ram = Machine::example_on(geometry).memory[5].base;
        (ram + 64 * chunk, u64::from(chunk.trailing_zeros()))
    }

    /// The entry of the device table of the range at 2 MB.
    fn device_index(&self) -> u64 {
        let node = self.sys.node(0);
        (0..node.device_count())
            .find(|&i| node.device_entry(i).base() == Machine::DEVICES)
            .unwrap() as u64
    }
}

/// Argon 1.1: node 1 (generation 1) attaches a device and dies; node 0
/// starts it fresh before any survivor takes the device. The identifier
/// word still reads taken by generation 1, which is over: node 0 attaches
/// the device, and detaches it.
#[test]
fn after_a_fresh_start_of_its_owner_an_identifier_word_of_the_dead_generation_is_free() {
    let mut m = S::new();
    m.ok(1, iospace(100));
    m.ok(1, attach(s(100), DEV));
    m.kill_1();
    m.restart_1();
    m.ok(0, iospace(100));
    m.ok(0, attach(s(100), DEV));
    // Node 1's new generation is refused it now: node 0 holds it.
    m.ok(1, iospace(100));
    assert_eq!(m.call(1, attach(s(100), DEV)), Err(Error::Denied));
    m.ok(0, attach(SlotAddr::NONE, DEV));
    m.ok(1, attach(s(100), DEV));
}

/// The same while the owner is dead (rule 3, as before).
#[test]
fn while_its_owner_is_dead_another_node_takes_its_identifier_word() {
    let mut m = S::new();
    m.ok(1, iospace(100));
    m.ok(1, attach(s(100), DEV));
    m.kill_1();
    m.ok(0, iospace(100));
    m.ok(0, attach(s(100), DEV));
}

/// Opus S1 and S2: a chunk and a device entry taken by generation 1 of
/// node 1 still read taken after its fresh start (R6 clears the map counts
/// and page classes, not the words); node 0 takes both.
#[test]
fn after_a_fresh_start_of_its_owner_its_chunk_and_device_entry_are_free_for_others() {
    let mut m = S::new();
    let (place, size) = m.free_run();
    let index = m.device_index();
    let chunk = place >> m.sys.node(0).geometry().chunk_shift();
    m.ok(1, take(0, place, size, 101));
    m.ok(1, take(1, index, 0, 102));
    m.kill_1();
    m.restart_1();
    let word = Ownership::from_raw(m.sys.node(0).ownership(chunk)).unwrap();
    assert_eq!(
        (word.state(), word.owner(), word.generation()),
        (OwnState::Taken, Owner::Node(1), 1)
    );
    m.ok(0, take(0, place, size, 101));
    m.ok(0, take(1, index, 0, 102));
    let word = Ownership::from_raw(m.sys.node(0).ownership(chunk)).unwrap();
    assert_eq!((word.owner(), word.generation()), (Owner::Node(0), 1));
}

/// What rule 3 keeps: a run given to a live node is free with that node
/// as owner, and no other node takes it, before or after a fresh start of
/// the giver.
#[test]
fn a_run_given_to_a_live_node_stays_busy_to_every_other_node() {
    let mut m = S::new();
    let (place, size) = m.free_run();
    m.ok(1, take(0, place, size, 101));
    let give = Syscall::MemoryGive {
        control: CONTROL,
        untyped: s(101),
        node: 0,
    };
    m.ok(1, give);
    // Before the giver's fresh start (the final review's F2 Opus 26: only
    // after was tried), and after.
    assert_eq!(m.call(1, take(0, place, size, 102)), Err(Error::Busy));
    m.kill_1();
    m.restart_1();
    assert_eq!(m.call(1, take(0, place, size, 101)), Err(Error::Busy));
    m.ok(0, take(0, place, size, 101));
}

/// Opus X1: a legal give and a legal destruction of a block by node 0
/// pass every invariant on node 1 too: a release is judged against its
/// writer and its generation, not against the node that checks.
#[test]
fn a_give_or_a_destruction_by_one_node_passes_the_checks_of_every_node() {
    let mut m = S::new();
    let (place, size) = m.free_run();
    m.ok(0, take(0, place, size, 101));
    let give = Syscall::MemoryGive {
        control: CONTROL,
        untyped: s(101),
        node: 1,
    };
    m.ok(0, give);
    m.ok(1, take(0, place, size, 101));
    let shift = m.sys.node(0).geometry().chunk_shift();
    m.ok(0, take(0, place + (2 << shift), size, 102));
    m.ok(0, Syscall::CapDelete(s(102)));
}
