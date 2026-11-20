//! The gate over page tables attacked from outside the gate.

mod common;

use dodos_abi::ObjectKind::{Frame, IoSpace, Reply};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const MS: u64 = 1_000_000;
const UT: SlotAddr = SlotAddr::own(25);
const MAILBOX: SlotAddr = SlotAddr::own(START_SLOT_MAILBOX);
const ASPACE: SlotAddr = SlotAddr::own(START_SLOT_ASPACE);
const CONTROL: SlotAddr = SlotAddr::own(START_SLOT_MEMORY_CONTROL);
const LIVING: SlotAddr = SlotAddr::own(START_SLOT_MACHINE_LIVING);
const IOMMU: SlotAddr = SlotAddr::own(18);
const REPLY: u32 = 100;
const OBJ: u32 = 101;
const GOT: u32 = 110;
const AT: u64 = 1 << 30;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

fn retype(kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind,
        size: common::sized(kind, size),
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

fn map_at(space: SlotAddr, frame: u32, address: u64, count: u64, rights: Rights) -> Syscall {
    Syscall::ASpaceMap(Map {
        space,
        frame: s(frame),
        untyped: SlotAddr::NONE,
        address,
        first: 0,
        count,
        rights: rights.bits(),
    })
}

fn map(frame: u32, count: u64, rights: Rights) -> Syscall {
    map_at(ASPACE, frame, AT, count, rights)
}

fn msg(label: u32, caps: Vec<Transfer>) -> Message {
    Message {
        label,
        flags: 0,
        words: vec![label.into()],
        caps,
        marks: 0,
    }
}

fn range(first: u32, count: u32) -> Range {
    Range {
        first: s(first),
        count,
    }
}

fn recv(first: u32) -> Syscall {
    Syscall::Recv {
        endpoint: MAILBOX,
        reply: s(REPLY),
        range: range(first, 4),
        wait: 0,
    }
}

fn stop(n: u32, mode: u64) -> Syscall {
    Syscall::NodeStop {
        node: core(n),
        mode,
        core: 0,
    }
}

/// A fresh start of node `n` on core `n`, the core it held.
fn start(n: u32, mode: u64) -> Syscall {
    Syscall::NodeStart {
        node: core(n),
        mode,
        image: 0,
        module: 0,
        window: 0,
        mask: 1 << n,
    }
}

/// A run of ordinary memory that no floor and no reserve covers: the base
/// of the machine's ordinary memory past its eight floor and reserve
/// chunks, aligned on the size of the run.
fn free_run(geometry: Geometry, log2: u32) -> u64 {
    let machine = Machine::example_on(geometry);
    let ram = machine.memory[5].base;
    let free = ram + 10 * geometry.chunk();
    free.next_multiple_of(1 << log2)
}

struct S {
    sys: System,
    t: Vec<ObjId>,
}

impl S {
    fn new() -> Self {
        Self::on(&Machine::example())
    }

    fn on(machine: &Machine) -> Self {
        let (sys, t) = System::boot(machine).unwrap();
        let mut m = Self { sys, t };
        for node in 0..2 {
            m.ok(node, retype(Reply, 0, REPLY));
            m.ok(
                node,
                Syscall::SchedConfigure {
                    sched: SlotAddr::own(START_SLOT_SCHED),
                    node: core(node.into()),
                    // Node n runs on core n.
                    core: node.into(),
                    budget: 5_000_000,
                    period: 10_000_000,
                    class: 1,
                },
            );
        }
        m
    }

    #[track_caller]
    fn on_node(&mut self, node: u16, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        // A creation clears its memory 64 KB per call: with large pages it
        // may take several.
        let mut outcome = self.on_node(node, call.clone());
        while matches!(call, Syscall::UntypedRetype { .. }) && outcome == Err(Error::Again) {
            outcome = self.on_node(node, call.clone());
        }
        assert_eq!(outcome, DONE);
    }

    #[track_caller]
    fn no(&mut self, node: u16, call: Syscall, error: Error) {
        assert_eq!(self.on_node(node, call), Err(error));
    }

    /// What can happen at this instant happens: interrupts, steps of the
    /// recovery reserves, the messages of the rings.
    #[track_caller]
    fn settle(&mut self) {
        self.sys.settle(100_000);
        assert_eq!(self.sys.check(), Ok(()));
    }

    #[track_caller]
    fn run(&mut self, ns: u64) {
        let to = self.sys.now() + ns;
        while self.sys.now() < to {
            let at = (self.sys.now() + 100_000).min(to);
            self.sys.advance(at);
            assert_eq!(self.sys.check(), Ok(()));
        }
    }

    fn state(&self, node: u16) -> MemberState {
        self.sys.member(node).state()
    }

    fn woken(&self, node: u16) -> Option<&Outcome> {
        self.sys.node(node).woken(self.t[node as usize])
    }

    /// Node 1 waits on its mailbox; node 0 grants it the frames `caps`
    /// lists by `node_send`; every message settles.
    #[track_caller]
    fn transfer(&mut self, caps: Vec<Transfer>) {
        assert_eq!(self.on_node(1, recv(GOT)), WAITS);
        let send = Syscall::NodeSend {
            node: core(1),
            message: msg(7, caps),
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(self.on_node(0, send), WAITS);
        self.settle();
        assert_eq!(self.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    }

    /// The page size, the number of pages and the physical base of a frame.
    fn frame(&self, node: u16, slot: u32) -> (PageSize, u64, u64) {
        self.sys
            .node(node)
            .frame_at(self.t[node as usize], s(slot))
            .expect("a frame")
    }

    fn geometry(&self, node: u16) -> Geometry {
        self.sys.node(node).geometry()
    }
}

fn all() -> u32 {
    Rights::ALL.without(Rights::EXEC).bits()
}

// 1. A grant whose granting node dies: R2 unmaps it.

#[test]
fn the_holder_of_a_grant_loses_its_mapping_when_the_granter_is_accused() {
    let mut m = S::new();
    m.ok(0, retype(Frame, 1, OBJ));
    // Node 0 maps its frame too: the granted frame's own chunk counts a
    // leaf of node 0 (the final review's F2 Opus 19: the test read a chunk
    // nothing ever mapped).
    m.ok(0, map(OBJ, 1, Rights::READ));
    let granted = m.frame(0, OBJ).2 / m.geometry(0).chunk();
    assert_eq!(m.sys.node(0).map_count(granted), 1);
    m.transfer(vec![Transfer::Derive(s(OBJ), all())]);
    m.ok(1, map(GOT, 1, Rights::READ.union(Rights::WRITE)));
    assert_eq!(
        m.sys.node(1).translated(m.t[1], AT),
        Some(Rights::READ.union(Rights::WRITE))
    );
    // Node 1, which holds a reserve role for it (D-150), kills node 0;
    // the machine runs to the end of the reclaim.
    let [claim, _] = common::reserve_claims();
    assert!(matches!(m.on_node(1, claim), Ok(Output::Epoch(_))));
    m.ok(1, stop(0, NODE_STOP_KILL));
    m.run(5 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    // R2 removed the holder's root remote capability and its mapping.
    assert_eq!(m.sys.node(1).translated(m.t[1], AT), None);
    m.no(1, Syscall::CapInspect(s(GOT)), Error::Invalid);
    // And the next generation of node 0 finds the chunk of the granted
    // frame without a trace of the leaves of the dead generation: R6
    // cleared its map count.
    m.ok(1, start(0, NODE_START_FRESH));
    // Node 1 starts core 0 in its recovery reserve; the core passes the
    // entry stub and the node becomes live.
    m.run(MS);
    assert_eq!(m.state(0), MemberState::Live);
    let thread = m.sys.node(0).object_at_start(START_SLOT_THREAD);
    m.t[0] = thread;
    let geometry = m.geometry(0);
    let log2 = geometry.chunk_shift() as u64;
    let place = free_run(geometry, log2 as u32);
    m.ok(
        0,
        Syscall::MemoryTake {
            control: CONTROL,
            kind: 0,
            place,
            size: log2,
            destination: s(200),
        },
    );
    assert_eq!(m.sys.node(0).map_count(place / geometry.chunk()), 0);
    assert_eq!(m.sys.node(0).map_count(granted), 0);
}

// 2. A block is not given while a piece of it lives, so the gate's own
//    check of the map count (G5) is never reached on this path.

#[test]
fn a_block_whose_grantee_maps_it_is_not_given() {
    let mut m = S::new();
    let geometry = m.geometry(0);
    let log2 = geometry.chunk_shift();
    let place = free_run(geometry, log2);
    m.ok(
        0,
        Syscall::MemoryTake {
            control: CONTROL,
            kind: 0,
            place,
            size: u64::from(log2),
            destination: s(210),
        },
    );
    m.ok(
        0,
        Syscall::UntypedRetype {
            untyped: s(210),
            kind: Frame,
            size: 1,
            page: PageSize::Page as u64,
            memory: 0,
            destination: s(211),
        },
    );
    m.transfer(vec![Transfer::Derive(s(211), all())]);
    m.ok(1, map(GOT, 1, Rights::READ.union(Rights::WRITE)));
    // The granter's map count of the chunk is 0: the holder's leaf counts in
    // its use entry (G4, G6).
    assert_eq!(m.sys.node(0).map_count(place / geometry.chunk()), 0);
    // The give is `busy`; the map count read above is 0, so the gate's
    // check of it cannot be the one refusing (which rule of the pieces does
    // the return value does not say: the final review's F2 Opus 22).
    m.no(
        0,
        Syscall::MemoryGive {
            control: CONTROL,
            untyped: s(210),
            node: 1,
        },
        Error::Busy,
    );
}

// 3. The map count of G4 and its limit.

#[test]
fn a_map_count_that_would_pass_its_limit_is_busy() {
    let mut machine = Machine::example();
    machine.map_count_max = 1;
    let mut m = S::on(&machine);
    m.ok(0, retype(Frame, 1, 200));
    m.ok(0, retype(Frame, 1, 201));
    let (_, _, first) = m.frame(0, 200);
    let (_, _, second) = m.frame(0, 201);
    let chunk = m.geometry(0).chunk();
    assert_eq!(first / chunk, second / chunk, "one chunk");
    let count = m.sys.node(0).map_count(first / chunk);
    m.ok(0, map(200, 1, Rights::READ));
    assert_eq!(m.sys.node(0).map_count(first / chunk), count + 1);
    // At another address, so that the busy is the count's (the same address
    // would be busy as already translated).
    m.no(0, map_at(ASPACE, 201, 2 * AT, 1, Rights::READ), Error::Busy);
    assert_eq!(m.sys.node(0).map_count(first / chunk), count + 1);
}

// 4. A device given to a node that does not own it, through the IOMMU.

#[test]
fn a_node_cannot_attach_or_detach_a_device_whose_identifier_another_node_holds() {
    let mut m = S::new();
    let attach = |iospace| Syscall::IoSpaceAttach {
        iommu: IOMMU,
        iospace,
        device: 4,
    };
    // Node 0 attaches device 4: its identifier word is node 0's.
    m.ok(0, retype(IoSpace, 0, 221));
    assert_eq!(m.on_node(0, attach(s(221))), WAITS);
    m.settle();
    assert_eq!(m.woken(0), Some(&DONE));
    // Node 1 can neither point it at its own tables nor detach it.
    m.ok(1, retype(IoSpace, 0, 221));
    m.no(1, attach(s(221)), Error::Denied);
    m.no(1, attach(SlotAddr::NONE), Error::Denied);
    assert_eq!(m.sys.check(), Ok(()));
    // Detached by node 0, the identifier is free: node 1 attaches it.
    assert_eq!(m.on_node(0, attach(SlotAddr::NONE)), WAITS);
    m.settle();
    // Node 1 holds the unit role: its attach waits for its own unit.
    assert_eq!(m.on_node(1, attach(s(221))), WAITS);
    m.settle();
    assert_eq!(m.woken(1), Some(&DONE));
    assert_eq!(m.sys.check(), Ok(()));
}

// The death of a node with an attached device, its context entry cleared by
// R1: `scenarios::units::a_node_that_dies_with_an_attached_device_leaves_no_
// context_entry_once_dead`, which reads the unit's entries (this file's
// weaker twin, which asserted only the death, was deleted: the final
// review's F2 Opus 21).

// 5. A page of the machine table is read only, and a kept frame is never
//    writable.

#[test]
fn the_kept_frame_of_the_living_part_of_the_machine_table_is_never_writable() {
    let mut m = S::new();
    // Slot 13 holds the living part, `read` and `derive`.
    m.no(
        0,
        Syscall::CapDerive {
            source: LIVING,
            destination: s(230),
            rights: Rights::READ.union(Rights::WRITE).bits(),
            badge: 0,
        },
        Error::Denied,
    );
    m.ok(
        0,
        Syscall::CapDerive {
            source: LIVING,
            destination: s(231),
            rights: Rights::READ.bits(),
            badge: 0,
        },
    );
    m.no(
        0,
        map(231, 1, Rights::READ.union(Rights::WRITE)),
        Error::Denied,
    );
    let (_, pages, _) = m.frame(0, 231);
    m.ok(0, map(231, pages, Rights::READ));
    // Every leaf of the machine table is read only.
    let page = m.geometry(0).page();
    for at in 0..pages {
        assert_eq!(
            m.sys.node(0).translated(m.t[0], AT + at * page),
            Some(Rights::READ)
        );
    }
}

// 6. A frame of the largest page size: G4 counts the leaf in every chunk.

#[test]
fn a_frame_of_one_gigabyte_counts_in_every_chunk_it_covers() {
    let mut m = S::new();
    let geometry = m.geometry(0);
    let Some(bytes) = geometry.bytes(PageSize::Large2) else {
        return;
    };
    let chunk = geometry.chunk();
    let chunks = bytes / chunk;
    let size = bytes.trailing_zeros();
    let place = free_run(geometry, size);
    // The largest page of 16 and 32 KB pages is 64 GB and 512 GB: its
    // creation clears it in 2^20 calls and more, so the case runs on 4 KB
    // pages, and the other geometries say so (the final review's F2 Opus
    // 20: they returned in silence).
    if bytes > 1 << 30 {
        assert!(geometry.page() > 4096, "skipped only on larger pages");
        return;
    }
    assert!(
        place / chunk + chunks <= 4098 + 8,
        "the run lies in the memory"
    );
    m.ok(
        0,
        Syscall::MemoryTake {
            control: CONTROL,
            kind: 0,
            place,
            size: u64::from(size),
            destination: s(240),
        },
    );
    let mut calls = 0u64;
    loop {
        let outcome = m.on_node(
            0,
            Syscall::UntypedRetype {
                untyped: s(240),
                kind: Frame,
                size: 1,
                page: PageSize::Large2 as u64,
                memory: 0,
                destination: s(241),
            },
        );
        calls += 1;
        match outcome {
            DONE => break,
            Err(Error::Again) => {}
            other => panic!("{other:?}"),
        }
    }
    // The clean mark of a dirty block: one step per 64 KB of the frame.
    assert_eq!(calls, bytes / CLEAR_STEP_BYTES);
    let (page, pages, base) = m.frame(0, 241);
    assert_eq!((page, pages), (PageSize::Large2, 1));
    let address = geometry.user_end - bytes;
    m.ok(0, map_at(ASPACE, 241, address, 1, Rights::READ));
    for chunk in base / chunk..(base + bytes) / chunk {
        assert_eq!(m.sys.node(0).map_count(chunk), 1, "chunk {chunk}");
    }
    m.ok(0, Syscall::ASpaceUnmap(s(241)));
    for chunk in base / chunk..(base + bytes) / chunk {
        assert_eq!(m.sys.node(0).map_count(chunk), 0, "chunk {chunk}");
    }
}

// DeepSeek F1: the bound of a gate call and the steps of a mapping.

#[test]
fn a_mapping_beyond_the_budget_of_a_gate_call_is_written_in_steps() {
    // One chunk checked per call: each page of the frame is a step.
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut m = S::on(&machine);
    let page = m.geometry(0).page();
    m.ok(0, retype(Frame, 3, 200));
    let translated = |m: &S, n: u64| m.sys.node(0).translated(m.t[0], AT + n * page);
    m.no(0, map(200, 3, Rights::READ), Error::Again);
    assert!(translated(&m, 0).is_some() && translated(&m, 1).is_none());
    // The record is partial: another mapping through it, or a protect, is
    // `busy`.
    m.no(0, map_at(ASPACE, 200, 2 * AT, 3, Rights::READ), Error::Busy);
    let protect = Syscall::ASpaceProtect {
        aspace: ASPACE,
        frame: s(200),
        rights: Rights::READ.bits(),
    };
    m.no(0, protect.clone(), Error::Busy);
    // The same call goes on, and the last step ends it.
    m.no(0, map(200, 3, Rights::READ), Error::Again);
    m.ok(0, map(200, 3, Rights::READ));
    assert!((0..3).all(|n| translated(&m, n).is_some()));
    m.no(0, protect, Error::Busy);
    // A partial mapping is undone by an unmap like any other.
    m.ok(0, retype(Frame, 3, 201));
    m.no(
        0,
        map_at(ASPACE, 201, 2 * AT, 3, Rights::READ),
        Error::Again,
    );
    m.ok(0, Syscall::ASpaceUnmap(s(201)));
    assert!(m.sys.node(0).translated(m.t[0], 2 * AT).is_none());
    assert_eq!(m.sys.check(), Ok(()));
}

/// DeepSeek F2 and Hydrogen N2 of the seventh review: a partial mapping
/// goes on only by the same call, page count included, for `aspace_map`
/// and `iospace_map` alike; another count is `busy`, and the record keeps
/// the count it started with.
#[test]
fn a_partial_mapping_goes_on_only_with_the_page_count_it_started_with() {
    let mut machine = Machine::example();
    machine.gate_chunks = 1;
    let mut m = S::on(&machine);
    let page = m.geometry(0).page();
    m.ok(0, retype(IoSpace, 0, 210));
    for frame in [200, 211] {
        m.ok(0, retype(Frame, 4, frame));
    }
    let io_map = |count| {
        Syscall::IoSpaceMap(Map {
            space: s(210),
            frame: s(211),
            untyped: s(common::TABLES),
            address: AT,
            first: 0,
            count,
            rights: Rights::READ.bits(),
        })
    };
    let aspace_map = |count| map(200, count, Rights::READ);
    for call in [aspace_map as fn(u64) -> Syscall, io_map] {
        m.no(0, call(4), Error::Again);
        for other in [1, 2, 3] {
            m.no(0, call(other), Error::Busy);
        }
        // The same call goes on, and ends with the four pages it asked.
        let mut outcome = Err(Error::Again);
        for _ in 0..8 {
            outcome = m.on_node(0, call(4));
            if outcome != Err(Error::Again) {
                break;
            }
        }
        assert_eq!(outcome, DONE);
        m.no(0, call(4), Error::Busy);
    }
    for n in 0..4 {
        assert!(m.sys.node(0).translated(m.t[0], AT + n * page).is_some());
    }
}

#[test]
fn a_protect_within_the_budget_of_a_gate_call_succeeds() {
    let mut m = S::new();
    m.ok(0, retype(Frame, 3, 200));
    m.ok(0, map(200, 3, Rights::READ));
    let protect = Syscall::ASpaceProtect {
        aspace: ASPACE,
        frame: s(200),
        rights: Rights::READ.union(Rights::WRITE).bits(),
    };
    m.ok(0, protect);
}
