//! Random call sequences: every invariant holds after every call, a seed
//! always gives the same state, and the generator reaches the calls, the
//! errors and the deep states that the coverage tests below name.
//!
//! The generator mixes two things. Single calls with random arguments, biased
//! towards slots that hold what the call needs. And scripts: a thread begins
//! something that takes many calls (a chain of copies down to depth 64, 33
//! copies of a capability and its removal, a process with its own capability
//! space, a nest of spaces and its removal) and finishes it over the next
//! steps, while the other threads go on making calls in between.

mod common;

use dodos_abi::*;
use dodos_model::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::OnceLock;

const SEEDS: u64 = 24;
const STEPS: usize = 5000;

/// The steps of a run: more where a page is larger than 16 KB, since each
/// creation then clears its memory in more calls and a run of `STEPS` makes
/// fewer objects (the coverage below is measured on the smaller pages).
/// `DODOS_PROPERTY_STEPS` sets another number (0 runs the directed cases
/// alone, which is how the floors below are shown above them).
fn steps() -> usize {
    if let Ok(steps) = std::env::var("DODOS_PROPERTY_STEPS") {
        return steps.parse().expect("DODOS_PROPERTY_STEPS is a number");
    }
    match page() > 16 << 10 {
        true => 2 * STEPS,
        false => STEPS,
    }
}

/// xorshift64* (Marsaglia, "Xorshift RNGs", 2003; multiplier from Vigna).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

// The bias towards valid arguments: the generator keeps each kind of object
// in its own class of slots, in every capability space. Slot `34 + 9 * n + c`
// is of class `c`; class 7 is left for moves and receive ranges. Every space
// that a script makes is laid out as the start-up space: a capability for the
// space itself in slot 1, an untyped in slot 107 (`MEMORY_SLOT`).
const UNTYPED: u32 = 0;
const CSPACE: u32 = 1;
const THREAD: u32 = 2;
const ENDPOINT: u32 = 3;
const REPLY: u32 = 4;
const NOTIFY: u32 = 5;
const OTHER: u32 = 6;
const SPARE: u32 = 7;
const SPACE: u32 = 8;
/// The classes of slots.
const CLASSES: u64 = 9;

/// The slots from here on are left to the scripts.
const SCRIPT_FIRST: u32 = 110;
/// The slots of a capability space that a script makes: four segments, so
/// that the removal of one has steps that only move the sweep.
const CHILD_SLOTS: u32 = 4 * CSPACE_SEGMENT_SLOTS;
/// The slots of the start-up space that the scripts use.
const HOME_SLOTS: u32 = 4 * CSPACE_SEGMENT_SLOTS;
/// The untyped of one chunk of the floor of `Machine::example`, at depth 1,
/// and the start-up untyped before it.
const UNTYPED_SLOT: u32 = 25;
/// The blocks the scripts take objects from: an ordinary run of 32 chunks
/// and the device range, at depth 2.
const MEMORY_SLOT: u32 = 107;
const DEVICE_SLOT: u32 = 108;
/// The `core` capabilities of `Machine::example`: the boot core's, then
/// the other's.
const CORE_SLOT: u32 = 16;
/// The `iommu` capability of `Machine::example`.
const IOMMU_SLOT: u32 = 18;
/// The links of a chain of copies that starts at depth 2 and ends at 64.
const CHAIN_LINKS: usize = TREE_DEPTH - 2;

/// Values of the machine of the run: a page, a first large page, and the
/// bytes a root entry covers.
fn geometry() -> Geometry {
    static GEOMETRY: std::sync::OnceLock<Geometry> = std::sync::OnceLock::new();
    *GEOMETRY.get_or_init(|| Machine::example().geometry)
}

/// The base of the ordinary memory of the machine past its floors' start.
fn ram() -> u64 {
    Machine::example_on(geometry()).memory[5].base
}

fn page() -> u64 {
    geometry().page()
}

/// The index of the device entry of the 2 MB device range: the entries
/// before it hold the parts of usable memory left out of the chunks.
fn device_index() -> u64 {
    static INDEX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *INDEX.get_or_init(|| {
        let (m, _) = Model::boot(&Machine::example()).unwrap();
        (0..m.device_count())
            .find(|&i| m.device_entry(i).base() == Machine::DEVICES)
            .unwrap() as u64
    })
}

fn large() -> u64 {
    geometry().chunk()
}

fn root_span() -> u64 {
    let geometry = geometry();
    geometry.page() << ((geometry.levels - 1) * geometry.entries.trailing_zeros())
}

const HERE: SlotAddr = SlotAddr::own(START_SLOT_CSPACE);
const MEMORY: SlotAddr = SlotAddr::own(MEMORY_SLOT);

const KINDS: [(ObjectKind, u32, u64); 12] = [
    (ObjectKind::Untyped, UNTYPED, 0),
    (ObjectKind::CSpace, CSPACE, 256),
    (ObjectKind::Thread, THREAD, 0),
    (ObjectKind::Endpoint, ENDPOINT, 0),
    (ObjectKind::Reply, REPLY, 0),
    (ObjectKind::Notify, NOTIFY, 0),
    (ObjectKind::Frame, OTHER, 3),
    (ObjectKind::ASpace, SPACE, 0),
    (ObjectKind::Sched, OTHER, 0),
    (ObjectKind::Timer, OTHER, 0),
    (ObjectKind::Task, OTHER, 0),
    (ObjectKind::IoSpace, SPACE, 0),
];
/// The kinds that this version does not make, and those never made from an
/// untyped.
const NOT_MADE: [ObjectKind; 9] = [
    ObjectKind::IoPorts,
    ObjectKind::Node,
    ObjectKind::Debug,
    ObjectKind::Vcpu,
    ObjectKind::Irq,
    ObjectKind::IrqControl,
    ObjectKind::Iommu,
    ObjectKind::MemoryControl,
    ObjectKind::Role,
];
const PAGES: [PageSize; 5] = [
    PageSize::Page,
    PageSize::Page,
    PageSize::Page,
    PageSize::Large1,
    PageSize::Large2,
];

/// The kinds whose dying the seeds measure, as `Census::dying` names them:
/// every kind made from an untyped, the irq, the role and the
/// memory_control (the final review's H8 left out timer, irq, iospace and
/// task; the role and the memory_control were never measured). A `node`,
/// an `iommu`, `irq_control`, `ioports` and the debug capability are of the
/// start-up and never die in a run of one node.
const MADE: [ObjectKind; 15] = [
    ObjectKind::Untyped,
    ObjectKind::CSpace,
    ObjectKind::ASpace,
    ObjectKind::Frame,
    ObjectKind::Thread,
    ObjectKind::Sched,
    ObjectKind::Endpoint,
    ObjectKind::Reply,
    ObjectKind::Notify,
    ObjectKind::Timer,
    ObjectKind::Irq,
    ObjectKind::IoSpace,
    ObjectKind::Task,
    ObjectKind::MemoryControl,
    ObjectKind::Role,
];

/// The calls, numbered as `name_of` numbers them.
const NAMES: [&str; 64] = [
    "cap_derive",
    "cap_move",
    "cap_delete",
    "cap_revoke",
    "cap_inspect",
    "untyped_retype",
    "cspace_grow",
    "thread_configure",
    "thread_resume",
    "thread_suspend",
    "call",
    "recv",
    "reply_recv",
    "send",
    "signal",
    "wait",
    "poll",
    "notify_bind",
    "debug_write",
    "thread_cancel",
    "aspace_map",
    "iospace_map",
    "aspace_unmap",
    "iospace_unmap",
    "aspace_protect",
    "iospace_attach",
    "thread_read_regs",
    "thread_write_regs",
    "sched_configure",
    "sched_bind",
    "sched_unbind",
    "sched_set_cores",
    "sched_read",
    "yield",
    "timer_set",
    "timer_cancel",
    "irq_claim",
    "irq_bind",
    "irq_ack",
    "task_exempt",
    "task_attach",
    "task_mark",
    "task_revoke",
    "task_read",
    "thread_set_features",
    "sched_count",
    "node_send",
    "memory_take",
    "memory_give",
    "memory_split",
    "node_start",
    "node_stop",
    "node_status",
    "node_idle",
    "node_watch",
    "grant_signal",
    "node_perf",
    "role_claim",
    "role_hand",
    "trace_emit",
    "ioport_in",
    "ioport_out",
    "ioports_issue",
    "entropy_read",
];
/// The calls that never succeed on one node beside phantom nodes:
/// `grant_signal` needs a remote frame, which only another node that runs
/// grants, and `node_start` a dead node (both in `model/src/scenarios/`).
const NEVER_ON_ONE_NODE: [&str; 2] = ["grant_signal", "node_start"];
/// `call`, `recv`, `reply_recv` and `wait`.
const BLOCKING: [usize; 4] = [10, 11, 12, 15];

fn name_of(call: &Syscall) -> usize {
    match call {
        Syscall::CapDerive { .. } => 0,
        Syscall::CapMove { .. } => 1,
        Syscall::CapDelete(_) => 2,
        Syscall::CapRevoke(_) => 3,
        Syscall::CapInspect(_) => 4,
        Syscall::UntypedRetype { .. } => 5,
        Syscall::CSpaceGrow { .. } => 6,
        Syscall::ThreadConfigure { .. } => 7,
        Syscall::ThreadResume(_) => 8,
        Syscall::ThreadSuspend(_) => 9,
        Syscall::Call { .. } => 10,
        Syscall::Recv { .. } => 11,
        Syscall::ReplyRecv { .. } => 12,
        Syscall::Send { .. } => 13,
        Syscall::Signal(_) => 14,
        Syscall::Wait(..) => 15,
        Syscall::Poll(_) => 16,
        Syscall::NotifyBind { .. } => 17,
        Syscall::DebugWrite { .. } => 18,
        Syscall::TraceEmit { .. } => 59,
        Syscall::IoportIn { .. } => 60,
        Syscall::IoportOut { .. } => 61,
        Syscall::IoportsIssue { .. } => 62,
        Syscall::EntropyRead { .. } => 63,
        Syscall::ThreadCancel(..) => 19,
        Syscall::ASpaceMap(_) => 20,
        Syscall::IoSpaceMap(_) => 21,
        Syscall::ASpaceUnmap(_) => 22,
        Syscall::IoSpaceUnmap(_) => 23,
        Syscall::ASpaceProtect { .. } => 24,
        Syscall::IoSpaceAttach { .. } => 25,
        Syscall::ThreadReadRegs(_) => 26,
        Syscall::ThreadWriteRegs { .. } => 27,
        Syscall::SchedConfigure { .. } => 28,
        Syscall::SchedBind { .. } => 29,
        Syscall::SchedUnbind(_) => 30,
        Syscall::SchedSetCores { .. } => 31,
        Syscall::SchedRead(_) => 32,
        Syscall::Yield => 33,
        Syscall::TimerSet { .. } => 34,
        Syscall::TimerCancel(_) => 35,
        Syscall::IrqClaim { .. } => 36,
        Syscall::IrqBind { .. } => 37,
        Syscall::IrqAck(_) => 38,
        Syscall::TaskExempt { .. } => 39,
        Syscall::TaskAttach { .. } => 40,
        Syscall::TaskMark { .. } => 41,
        Syscall::TaskRevoke(_) => 42,
        Syscall::TaskRead(_) => 43,
        Syscall::ThreadSetFeatures { .. } => 44,
        Syscall::SchedCount { .. } => 45,
        Syscall::NodeSend { .. } => 46,
        Syscall::MemoryTake { .. } => 47,
        Syscall::MemoryGive { .. } => 48,
        Syscall::MemorySplit { .. } => 49,
        Syscall::NodeStart { .. } => 50,
        Syscall::NodeStop { .. } => 51,
        Syscall::NodeStatus { .. } => 52,
        Syscall::NodeIdle { .. } => 53,
        Syscall::NodeWatch { .. } => 54,
        Syscall::GrantSignal(_) => 55,
        Syscall::NodePerf { .. } => 56,
        Syscall::RoleClaim(_) => 57,
        Syscall::RoleHand { .. } => 58,
    }
}

fn own(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn derive(source: SlotAddr, destination: SlotAddr) -> Syscall {
    Syscall::CapDerive {
        source,
        destination,
        rights: Rights::ALL.bits(),
        badge: 0,
    }
}

fn retype(kind: ObjectKind, size: u64, destination: SlotAddr) -> Syscall {
    Syscall::UntypedRetype {
        untyped: MEMORY,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination,
    }
}

/// A chain of copies in one capability space: the slot of its first link,
/// the slot of its deepest, and the number of its links.
#[derive(Clone, Copy)]
struct Chain {
    first: u32,
    last: u32,
    links: usize,
}

/// What the generator remembers from one call to the next.
#[derive(Default)]
struct Plan {
    /// The calls that a thread still has to make to finish what it began.
    scripts: BTreeMap<ObjId, VecDeque<Syscall>>,
    /// The chain of copies that grows in each capability space.
    chains: BTreeMap<ObjId, Chain>,
    /// The slots of each capability space that a script filled, to empty
    /// them once what they held is removed.
    used: BTreeMap<ObjId, BTreeSet<u32>>,
    /// In each capability space, the slot of an endpoint on which a thread
    /// began to wait for a message of four capabilities.
    listening: BTreeMap<ObjId, u32>,
}

enum Wanted {
    Full,
    Free,
}

/// The generator of the calls of one thread. It looks at the state only to
/// prefer a slot that is full, or one that is not, and to see what a
/// capability is before it builds on it.
struct Generator<'a> {
    rng: &'a mut Rng,
    model: &'a Model,
    thread: ObjId,
    /// The capability space of the thread, by the capability in its slot 1.
    space: Option<ObjId>,
    /// The start-up space.
    home: ObjId,
}

impl Generator<'_> {
    fn full(&self, slot: SlotAddr) -> bool {
        self.model.object_at(self.thread, slot).is_some()
    }

    /// The kind and the rights of the capability in a slot, asked of a copy
    /// of the state.
    fn inspect(&self, slot: SlotAddr) -> Option<(ObjectKind, Rights)> {
        match self
            .model
            .clone()
            .apply(self.thread, Syscall::CapInspect(slot))
        {
            Ok(Output::Inspect { kind, rights, .. }) => Some((kind, rights)),
            _ => None,
        }
    }

    /// Whether a slot holds a capability with every right.
    fn whole(&self, slot: SlotAddr) -> bool {
        self.inspect(slot)
            .is_some_and(|(_, rights)| rights.contains(Rights::ALL.without(Rights::EXEC)))
    }

    fn class_slot(&mut self, class: u32) -> u32 {
        34 + CLASSES as u32 * self.rng.below(8) as u32 + class
    }

    /// A slot of a class, in the caller's capability space or through
    /// another, and now and then any address at all.
    fn slot(&mut self, class: u32) -> SlotAddr {
        let index = self.class_slot(class);
        match self.rng.below(100) {
            0 => SlotAddr::from_raw(self.rng.next()),
            1 => own(self.rng.below(140) as u32),
            2..12 => SlotAddr::through(self.class_slot(CSPACE), index),
            _ => own(index),
        }
    }

    /// A slot of a class, after a few tries for one that holds a capability
    /// (`Wanted::Full`) or for one that does not.
    fn find(&mut self, class: u32, wanted: Wanted) -> SlotAddr {
        for _ in 0..6 {
            let slot = self.slot(class);
            if self.full(slot) == matches!(wanted, Wanted::Full) {
                return slot;
            }
        }
        self.slot(class)
    }

    /// A slot of a class in the caller's own space, full or free as wanted,
    /// if a few tries find one.
    fn find_own(&mut self, class: u32, wanted: Wanted) -> Option<u32> {
        let wanted = matches!(wanted, Wanted::Full);
        let tries: Vec<u32> = (0..8).map(|_| self.class_slot(class)).collect();
        tries
            .into_iter()
            .find(|&index| self.full(own(index)) == wanted)
    }

    /// A capability of a class: often one of those in the fixed slots.
    fn held(&mut self, class: u32) -> SlotAddr {
        let fixed = match class {
            UNTYPED => MEMORY_SLOT + self.rng.chance(10) as u32,
            CSPACE => START_SLOT_CSPACE,
            _ => return self.find(class, Wanted::Full),
        };
        if self.rng.chance(40) {
            own(fixed)
        } else {
            self.find(class, Wanted::Full)
        }
    }

    fn free(&mut self, class: u32) -> SlotAddr {
        self.find(class, Wanted::Free)
    }

    /// A rights word: mostly every right, seldom a bit that names none.
    fn rights(&mut self) -> u32 {
        match self.rng.below(100) {
            0..75 => Rights::ALL.bits(),
            75..98 => self.rng.below(256) as u32,
            _ => self.rng.below(1024) as u32,
        }
    }

    /// A wait number: mostly none, else a small one, so that the numbers
    /// of the waits and those of `thread_cancel` meet.
    fn number(&mut self) -> u64 {
        if self.rng.chance(50) {
            0
        } else {
            1 + self.rng.below(8)
        }
    }

    /// The untyped to take memory from: mostly the one in slot 107, so that a
    /// capability space is often grown from the untyped it was created from.
    fn memory(&mut self) -> SlotAddr {
        if self.rng.chance(60) {
            MEMORY
        } else {
            self.held(UNTYPED)
        }
    }

    /// A slot of a class in another capability space, or in the caller's
    /// own through its capability for it.
    fn inside(&mut self, class: u32) -> SlotAddr {
        let via = if self.rng.chance(10) {
            START_SLOT_CSPACE
        } else {
            self.find(CSPACE, Wanted::Full).index()
        };
        SlotAddr::through(via, self.class_slot(class))
    }

    /// The source of a move that the depths can refuse: an untyped
    /// capability or a cspace capability, here or in another space. Seldom
    /// one in a fixed slot: once moved, the rest of the run finds little to
    /// do.
    fn bound(&mut self) -> SlotAddr {
        let class = if self.rng.chance(50) { UNTYPED } else { CSPACE };
        match self.rng.below(100) {
            0..30 => self.inside(class),
            30..33 => self.held(class),
            _ => self.find(class, Wanted::Full),
        }
    }

    /// Four capabilities of four slots of the caller's space, to derive.
    fn four(&mut self) -> Vec<Transfer> {
        let classes = [ENDPOINT, REPLY, NOTIFY, OTHER, THREAD];
        let mut slots: Vec<u32> = Vec::new();
        for _ in 0..12 {
            let class = self.rng.pick(&classes);
            let found = self
                .find_own(class, Wanted::Full)
                .filter(|slot| !slots.contains(slot));
            slots.extend(found);
        }
        slots.truncate(MAX_CAPS);
        let copy = |slot: u32| Transfer::Derive(own(slot), Rights::NONE.bits());
        slots.into_iter().map(copy).collect()
    }

    /// Four free slots in a row in the caller's space, among those left to
    /// the scripts: a receive range for four capabilities.
    fn four_free(&mut self) -> Option<SlotAddr> {
        let table = if self.space == Some(self.home) {
            HOME_SLOTS
        } else {
            CHILD_SLOTS
        };
        let free = |first: &u32| (*first..first + 4).all(|index| !self.full(own(index)));
        (SCRIPT_FIRST..table - 4).rev().find(free).map(own)
    }

    fn message(&mut self) -> Message {
        let mut caps = Vec::new();
        if self.rng.chance(12) {
            caps = self.four();
        }
        while caps.is_empty() && self.rng.chance(30) {
            let class = self.rng.below(8) as u32;
            caps.push(match self.rng.below(100) {
                0..25 => Transfer::Move(self.bound()),
                25..55 => Transfer::Move(self.held(class)),
                _ => Transfer::Derive(self.held(class), self.rights()),
            });
            while caps.len() < MAX_CAPS + 1 && self.rng.chance(30) {
                caps.push(Transfer::Derive(self.held(class), self.rights()));
            }
        }
        // Mostly within the registers; up to the limit; seldom beyond it.
        let words = match self.rng.below(100) {
            0 => MAX_WORDS as u64 + 1,
            1..20 => 4 + self.rng.below(MAX_WORDS as u64 - 3),
            _ => self.rng.below(4),
        };
        Message {
            label: self.rng.next() as u32,
            flags: (self.rng.below(200) == 0) as u32,
            marks: 0,
            words: (0..words).collect(),
            caps,
        }
    }

    fn range(&mut self) -> Range {
        if let Some(first) = self.four_free().filter(|_| self.rng.chance(25)) {
            return Range { first, count: 4 };
        }
        let first = if self.rng.chance(30) {
            self.inside(SPARE)
        } else {
            self.free(SPARE)
        };
        Range {
            first,
            count: self.rng.below(6) as u32,
        }
    }

    /// A duration: mostly between the smallest budget and a millisecond,
    /// seldom 0 or below the smallest budget.
    fn duration(&mut self) -> u64 {
        match self.rng.below(20) {
            0 => 0,
            1 => self.rng.below(SCHED_MIN_BUDGET_NS),
            _ => SCHED_MIN_BUDGET_NS + self.rng.below(1_000_000),
        }
    }

    /// A feature mask: mostly mask 0 or a baseline feature; the large
    /// optional feature, which needs a larger area; and masks that the
    /// node's kind refuses.
    fn mask(&mut self) -> u64 {
        match self.rng.below(10) {
            0..3 => 0,
            3 => 1 << x86_64::AVX,
            4..6 => 1 << x86_64::AMX,
            6 => 1 << x86_64::AVX512,
            7 => 1 << FEATURE_USER_INTERRUPTS,
            8 => 1 << 40,
            _ => self.rng.next() & 0xFF,
        }
    }

    /// A `role` capability of the start-up space: the unit roles, the
    /// reserves and the plain roles of `Machine::example`.
    fn role(&mut self) -> SlotAddr {
        if self.rng.chance(95) {
            own(common::ROLES + self.rng.below(5) as u32)
        } else {
            self.held(OTHER)
        }
    }

    /// A call of the Cores group on the `core` capability of this node or
    /// of the phantom node. Seldom a stop: it ends what the phantom node
    /// can show.
    fn core_call(&mut self) -> Syscall {
        let node = match self.rng.below(10) {
            0..6 => own(CORE_SLOT),
            6..9 => own(CORE_SLOT + 1),
            _ => self.held(OTHER),
        };
        let core = self.rng.below(3);
        match self.rng.below(20) {
            0 => Syscall::NodeStart {
                node,
                mode: self.rng.below(3),
                image: self.rng.below(3),
                module: self.rng.below(3),
                window: self.rng.below(2),
                mask: self.rng.below(4),
            },
            1 if self.rng.chance(10) => Syscall::NodeStop {
                node,
                mode: self.rng.below(4),
                core: self.rng.below(2),
            },
            1 | 2 => Syscall::NodeStatus { node, core },
            3..6 => Syscall::NodeIdle {
                node,
                core,
                words: match self.rng.below(4) {
                    0 => Vec::new(),
                    1 => vec![100, 200, 0, 0, 300, 400, 1, 0],
                    2 => vec![100, 200, 0, 0, 50, 400, 1, 0],
                    _ => vec![self.rng.below(5), 0, self.rng.below(4), self.rng.below(3)],
                },
            },
            6..12 => Syscall::NodeWatch {
                node,
                index: self.rng.below(5),
                notify: if self.rng.chance(30) {
                    SlotAddr::NONE
                } else {
                    self.held(NOTIFY)
                },
            },
            12..15 => Syscall::GrantSignal(self.held(OTHER)),
            _ => {
                let lowest = self.rng.below(300);
                Syscall::NodePerf {
                    node,
                    core,
                    lowest,
                    highest: lowest + self.rng.below(100),
                    preference: self.rng.below(300),
                }
            }
        }
    }

    /// A call of debugging, of the I/O ports, or `entropy_read`.
    fn debug_call(&mut self) -> Syscall {
        let debug = if self.rng.chance(90) {
            own(START_SLOT_DEBUG)
        } else {
            self.held(OTHER)
        };
        let ports = if self.rng.chance(90) {
            own(START_SLOT_IOPORTS)
        } else {
            self.held(OTHER)
        };
        let port = match self.rng.below(10) {
            0 => 0x20,
            1 => 0xCF8,
            _ => 0x60 + self.rng.below(16),
        };
        let width = self.rng.pick(&[1, 2, 4, 3]);
        match self.rng.below(12) {
            0..3 => {
                let length = match self.rng.below(10) {
                    0 => 0,
                    1 => 513,
                    2 => 100,
                    _ => self.rng.below(48),
                };
                Syscall::DebugWrite {
                    debug,
                    length,
                    words: (0..length.div_ceil(8)).collect(),
                }
            }
            3 | 4 => Syscall::TraceEmit {
                debug,
                words: [self.rng.next(), 1, 2],
            },
            5 => Syscall::IoportIn {
                ioports: ports,
                port,
                width,
            },
            6 => Syscall::IoportOut {
                ioports: ports,
                port,
                width,
                value: self.rng.below(1 << 9),
            },
            7 | 8 => {
                let first = self.rng.below(0x100);
                Syscall::IoportsIssue {
                    ioports: ports,
                    destination: self.free(OTHER),
                    first,
                    last: first + self.rng.below(0x40),
                }
            }
            _ => Syscall::EntropyRead {
                length: self.rng.below(70),
                mix: [self.rng.next(), 0, 0, 0],
            },
        }
    }

    /// The memory_control of the node, mostly.
    fn control(&mut self) -> SlotAddr {
        if self.rng.chance(95) {
            own(START_SLOT_MEMORY_CONTROL)
        } else {
            self.held(OTHER)
        }
    }

    /// The rights word of a mapping: mostly a valid one.
    fn mapping_rights(&mut self) -> u32 {
        let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
        match self.rng.below(10) {
            0..4 => r.union(w).bits(),
            4..6 => r.bits(),
            6..8 => r.union(x).bits(),
            8 => w.bits(),
            _ => self.rng.below(16) as u32,
        }
    }

    /// The arguments of a mapping: a space and a frame of their classes,
    /// mostly the untyped in slot 107, and a range near the bottom of the
    /// user range, so that mappings meet.
    fn map(&mut self) -> Map {
        let size = if self.rng.chance(90) { page() } else { large() };
        Map {
            space: self.held(SPACE),
            frame: self.held(OTHER),
            untyped: match self.rng.below(10) {
                0..7 => MEMORY,
                7 => SlotAddr::NONE,
                _ => self.held(UNTYPED),
            },
            address: (1 << 30) + size * self.rng.below(16) + (self.rng.chance(2) as u64) * page(),
            first: self.rng.chance(15) as u64,
            count: 1 + self.rng.below(3),
            rights: self.mapping_rights(),
        }
    }

    /// One call with random arguments.
    fn call(&mut self) -> Syscall {
        let class = self.rng.below(CLASSES) as u32;
        let near = if self.rng.chance(80) { class } else { SPARE };
        let answered = if self.rng.chance(50) { ENDPOINT } else { REPLY };
        let none = |rng: &mut Rng| rng.chance(60);
        // One call in seven is among those that the depths rule on.
        // Values 74 to 79 and 37 are the notify binds and the debug calls
        // (the final review's H11: they had roll 37 alone).
        let roll = if self.rng.chance(14) {
            29 + self.rng.below(8)
        } else {
            self.rng.below(80)
        };
        match roll {
            0..3 => Syscall::CapDerive {
                source: self.held(class),
                destination: self.free(near),
                rights: self.rights(),
                badge: match self.rng.below(10) {
                    0..7 => 0,
                    7 | 8 => self.rng.below(8),
                    _ => 1 << self.rng.below(64),
                },
            },
            3 => Syscall::CapMove {
                source: self.find(class, Wanted::Full),
                destination: self.free(near),
            },
            // Rarely a capability in a fixed slot: that ends most of the run.
            // Now and then a start-up capability, the floor untypeds
            // included.
            4 if self.rng.below(300) == 0 => Syscall::CapDelete(own(self.rng.below(26) as u32)),
            4 => Syscall::CapDelete(self.slot(class)),
            5 if self.rng.chance(2) => Syscall::CapRevoke(self.held(UNTYPED)),
            5 => Syscall::CapRevoke(self.find(class, Wanted::Full)),
            6 => Syscall::CapInspect(self.held(class)),
            7..12 => {
                let (mut kind, class, size) = self.rng.pick(&KINDS);
                // A child untyped of 16 pages, whatever the page.
                let size = match kind {
                    ObjectKind::Untyped => 16 * page(),
                    _ => size,
                };
                if self.rng.chance(3) {
                    kind = self.rng.pick(&NOT_MADE);
                }
                Syscall::UntypedRetype {
                    untyped: self.memory(),
                    kind,
                    // A thread with the large optional feature, whose
                    // area fits either mask.
                    size: if kind == ObjectKind::Thread && self.rng.chance(30) {
                        1 << x86_64::AMX
                    } else if self.rng.chance(95) {
                        size
                    } else {
                        self.rng.below(300)
                    },
                    page: self.rng.pick(&PAGES) as u64,
                    memory: self.rng.below(100).saturating_sub(97),
                    destination: self.free(class),
                }
            }
            12 | 13 => Syscall::CSpaceGrow {
                cspace: self.find(CSPACE, Wanted::Full),
                untyped: self.memory(),
            },
            14 => Syscall::ThreadConfigure {
                thread: self.held(THREAD),
                cspace: self.held(CSPACE),
                aspace: if none(self.rng) {
                    SlotAddr::NONE
                } else {
                    self.held(OTHER)
                },
                fault: if none(self.rng) {
                    SlotAddr::NONE
                } else {
                    self.held(ENDPOINT)
                },
                page: if none(self.rng) {
                    SlotAddr::NONE
                } else {
                    self.held(OTHER)
                },
            },
            15 | 16 => Syscall::ThreadResume(self.held(THREAD)),
            17 if self.rng.chance(30) => Syscall::ThreadSuspend(self.held(THREAD)),
            17 if self.rng.chance(60) => Syscall::ThreadCancel(self.held(THREAD), self.number()),
            17..20 => Syscall::Call {
                endpoint: self.held(ENDPOINT),
                message: self.message(),
                range: self.range(),
                wait: self.number(),
            },
            20 | 21 => Syscall::Recv {
                endpoint: self.held(ENDPOINT),
                reply: self.held(REPLY),
                range: self.range(),
                wait: self.number(),
            },
            22 | 23 => Syscall::ReplyRecv {
                reply: self.held(REPLY),
                message: self.message(),
                endpoint: self.held(ENDPOINT),
                range: self.range(),
                wait: self.number(),
            },
            24 | 25 => Syscall::Send {
                target: self.held(answered),
                message: self.message(),
            },
            26 => Syscall::Signal(self.held(NOTIFY)),
            27 => Syscall::Wait(self.held(NOTIFY), self.number()),
            28 => Syscall::Poll(self.held(NOTIFY)),
            // A start-up untyped, the least deep, into another space.
            29 if self.rng.chance(40) => Syscall::CapMove {
                source: own(UNTYPED_SLOT - self.rng.chance(30) as u32),
                destination: {
                    let via = self.find(CSPACE, Wanted::Full).index();
                    SlotAddr::through(via, self.class_slot(UNTYPED))
                },
            },
            // The moves that the depths can refuse, both ways.
            29..32 => Syscall::CapMove {
                source: self.bound(),
                destination: if self.rng.chance(30) {
                    self.free(near)
                } else {
                    self.inside(near)
                },
            },
            // The same by IPC: a message that moves one, and a receiver whose
            // range is in another capability space.
            32..35 => {
                let message = Message {
                    caps: vec![Transfer::Move(self.bound())],
                    ..Message::default()
                };
                match self.rng.below(3) {
                    0 => Syscall::Send {
                        target: self.held(ENDPOINT),
                        message,
                    },
                    1 => Syscall::Call {
                        endpoint: self.held(ENDPOINT),
                        message,
                        range: self.range(),
                        wait: self.number(),
                    },
                    _ => Syscall::ReplyRecv {
                        reply: self.held(REPLY),
                        message,
                        endpoint: self.held(ENDPOINT),
                        range: self.range(),
                        wait: self.number(),
                    },
                }
            }
            35 | 36 => Syscall::Recv {
                endpoint: self.held(ENDPOINT),
                reply: self.held(REPLY),
                range: Range {
                    first: self.inside(SPARE),
                    count: 1 + self.rng.below(4) as u32,
                },
                wait: self.number(),
            },
            38 | 39 => Syscall::ThreadCancel(self.held(THREAD), self.number()),
            40 | 41 => Syscall::ASpaceMap(self.map()),
            42 => Syscall::IoSpaceMap(self.map()),
            43 if self.rng.chance(50) => Syscall::ASpaceUnmap(self.held(OTHER)),
            43 => Syscall::IoSpaceUnmap(self.held(OTHER)),
            44 => Syscall::ASpaceProtect {
                aspace: self.held(SPACE),
                frame: self.held(OTHER),
                rights: self.mapping_rights(),
            },
            47 if self.rng.chance(50) => Syscall::ThreadReadRegs(self.held(THREAD)),
            47 => Syscall::ThreadWriteRegs {
                thread: self.held(THREAD),
                block: {
                    let mut block = vec![self.rng.below(1 << 47); x86_64::REGS_WORDS];
                    // A breakpoint control that enables nothing, and the
                    // user-interrupt word 0, mostly.
                    block[x86_64::REG_DR7] = 0;
                    block[x86_64::REG_UINTR] = 0;
                    match self.rng.below(14) {
                        0 => block.push(0),
                        1 => block[x86_64::REG_RIP] = 1 << 47,
                        2 => block[x86_64::REG_DR7] = 1 << 13,
                        3 => block[x86_64::REG_DR0] = 1 << 63,
                        4 => block[x86_64::REG_UINTR] = 1,
                        // A breakpoint enabled: an execution breakpoint
                        // or a write watchpoint of one byte, which a
                        // fault of kind 9 can then hit (ver-D M1).
                        5..7 => block[x86_64::REG_DR7] = 0x1 | 0b01 << 16,
                        7..10 => block[x86_64::REG_DR7] = 0x1,
                        _ => {}
                    }
                    block
                },
            },
            48 => Syscall::SchedConfigure {
                sched: self.held(OTHER),
                node: own(CORE_SLOT + self.rng.chance(5) as u32),
                core: 0,
                budget: self.duration(),
                period: self.duration() * 4,
                class: self.rng.below(4),
            },
            49 => Syscall::SchedBind {
                sched: self.held(OTHER),
                thread: self.held(THREAD),
            },
            50 => Syscall::SchedUnbind(self.held(OTHER)),
            51 => Syscall::SchedSetCores {
                sched: self.held(OTHER),
                node: own(CORE_SLOT),
                window: self.rng.chance(5) as u64,
                mask: self.rng.below(4),
            },
            52 => Syscall::SchedRead(self.held(OTHER)),
            53 => Syscall::Yield,
            54 => Syscall::TimerSet {
                timer: self.held(OTHER),
                notify: self.held(NOTIFY),
                deadline: self.model.now() + self.duration() * self.rng.below(3),
                tolerance: self.duration() * self.rng.below(2),
            },
            55 => Syscall::TimerCancel(self.held(OTHER)),
            56 => Syscall::IrqClaim {
                control: if self.rng.chance(90) {
                    own(START_SLOT_IRQ_CONTROL)
                } else {
                    self.held(OTHER)
                },
                source: match self.rng.below(10) {
                    0..5 => IrqSource::msi(0, self.rng.below(4) as u8, 0, 0).raw(),
                    5..9 => {
                        IrqSource::line(self.rng.below(26) as u32, self.rng.chance(50), false).raw()
                    }
                    _ => self.rng.next(),
                },
                core: 0,
                vector: if self.rng.chance(95) {
                    48 + self.rng.below(12)
                } else {
                    self.rng.below(300)
                },
                destination: self.free(OTHER),
            },
            57 => Syscall::IrqBind {
                irq: self.held(OTHER),
                notify: if self.rng.chance(25) {
                    SlotAddr::NONE
                } else {
                    self.held(NOTIFY)
                },
            },
            58 => Syscall::IrqAck(self.held(OTHER)),
            59 => Syscall::TaskExempt {
                task: self.held(OTHER),
                aware: if self.rng.chance(50) {
                    own(START_SLOT_TASK)
                } else {
                    self.held(OTHER)
                },
            },
            60 => Syscall::TaskAttach {
                thread: self.held(THREAD),
                task: match self.rng.below(10) {
                    0..2 => SlotAddr::NONE,
                    2 => own(START_SLOT_TASK),
                    _ => self.held(OTHER),
                },
            },
            61 => Syscall::TaskMark {
                task: self.held(OTHER),
                marks: self.rng.below(5),
            },
            62 if self.rng.chance(20) => Syscall::TaskRevoke(self.held(OTHER)),
            62 | 63 => Syscall::TaskRead(if self.rng.chance(20) {
                own(START_SLOT_TASK)
            } else {
                self.held(OTHER)
            }),
            64 => Syscall::ThreadSetFeatures {
                thread: self.held(THREAD),
                mask: self.mask(),
            },
            65 => Syscall::SchedCount {
                sched: self.held(OTHER),
                node: own(CORE_SLOT + self.rng.chance(5) as u32),
                mode: self.rng.below(100).saturating_sub(95).min(2) + self.rng.chance(50) as u64,
            },
            66 => {
                let control = self.control();
                let (kind, place, size) = match self.rng.below(10) {
                    0..7 => {
                        let shift = geometry().chunk_shift() as u64;
                        let log2 = shift + self.rng.below(3);
                        let chunks = 4098 >> (log2 - shift);
                        let place = ram() + (self.rng.below(chunks) << log2);
                        (0, place, log2)
                    }
                    7..9 => (1, device_index() + self.rng.below(4), 0),
                    _ => (self.rng.below(3), self.rng.next() >> 20, self.rng.below(40)),
                };
                Syscall::MemoryTake {
                    control,
                    kind,
                    place,
                    size,
                    destination: self.free(UNTYPED),
                }
            }
            // Seldom the blocks of the scripts: given, they end most of a
            // run.
            67 => Syscall::MemoryGive {
                control: self.control(),
                untyped: if self.rng.below(50) == 0 {
                    self.held(UNTYPED)
                } else {
                    self.find(UNTYPED, Wanted::Full)
                },
                node: match self.rng.below(10) {
                    0..8 => 1,
                    8 => 0,
                    _ => 2,
                },
            },
            68 => Syscall::MemorySplit {
                control: self.control(),
                untyped: if self.rng.chance(10) {
                    own(DEVICE_SLOT)
                } else {
                    self.find(UNTYPED, Wanted::Full)
                },
            },
            // Seldom to the phantom node, whose mailbox no kernel serves: the
            // caller waits for an answer that never comes.
            69 => Syscall::NodeSend {
                node: own(CORE_SLOT + self.rng.chance(3) as u32),
                message: Message::default(),
                doorbell: if self.rng.chance(50) {
                    SlotAddr::NONE
                } else {
                    self.held(NOTIFY)
                },
            },
            70 => self.core_call(),
            71 => self.core_call(),
            72 => Syscall::RoleClaim(self.role()),
            73 => Syscall::RoleHand {
                role: self.role(),
                node: match self.rng.below(10) {
                    0 => SlotAddr::NONE,
                    1..6 => own(CORE_SLOT),
                    _ => own(CORE_SLOT + 1),
                },
            },
            45 | 46 => Syscall::IoSpaceAttach {
                iommu: if self.rng.chance(90) {
                    own(IOMMU_SLOT)
                } else {
                    self.held(OTHER)
                },
                iospace: if self.rng.chance(30) {
                    SlotAddr::NONE
                } else {
                    self.held(SPACE)
                },
                device: match self.rng.below(100) {
                    0..90 => 0x10 + self.rng.below(20),
                    90..96 => 0xf8,
                    _ => self.rng.below(1 << 17),
                },
            },
            37 | 74..80 if self.rng.chance(80) => Syscall::NotifyBind {
                notify: self.held(NOTIFY),
                endpoint: if self.rng.chance(20) {
                    SlotAddr::NONE
                } else {
                    self.held(ENDPOINT)
                },
            },
            _ => self.debug_call(),
        }
    }

    // The scripts. Each returns the calls that the thread makes over the
    // next steps, or none when its capability space does not lend itself.

    /// Free slots of the caller's own space among those left to the scripts.
    fn spare(&mut self, plan: &mut Plan, wanted: usize) -> Vec<u32> {
        let Some(space) = self.space else {
            return Vec::new();
        };
        let table = if space == self.home {
            HOME_SLOTS
        } else {
            CHILD_SLOTS
        };
        let free = (SCRIPT_FIRST..table).filter(|&index| !self.full(own(index)));
        let slots: Vec<u32> = free.take(wanted).collect();
        plan.used.entry(space).or_default().extend(&slots);
        slots
    }

    /// Lengthens the chain of copies of this space, down to depth 64, or
    /// begins one from a capability of any class; and removes a chain that
    /// is complete, link by link from its first.
    fn chain(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let Some(space) = self.space else {
            return Vec::new();
        };
        let grown = plan
            .chains
            .get(&space)
            .copied()
            .filter(|chain| self.full(own(chain.last)));
        if let Some(chain) = grown.filter(|chain| chain.links >= CHAIN_LINKS) {
            plan.chains.remove(&space);
            return vec![Syscall::CapDelete(own(chain.first)); chain.links / REMOVAL_STEP + 2];
        }
        let class = self.rng.below(7) as u32;
        let begun = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)));
        let (mut from, first, links) = match (grown, begun) {
            (Some(chain), _) => (chain.last, Some(chain.first), chain.links),
            (None, Some(index)) => (index, None, 0),
            (None, None) => return Vec::new(),
        };
        let wanted = (CHAIN_LINKS - links).min(20 + self.rng.below(40) as usize);
        let slots = self.spare(plan, wanted);
        let (Some(&start), Some(&last)) = (slots.first(), slots.last()) else {
            return Vec::new();
        };
        let chain = Chain {
            first: first.unwrap_or(start),
            last,
            links: links + slots.len(),
        };
        plan.chains.insert(space, chain);
        let link = |&to: &u32| derive(own(std::mem::replace(&mut from, to)), own(to));
        slots.iter().map(link).collect()
    }

    /// 33 copies or more of a capability of any class, then its removal by
    /// `cap_delete` or `cap_revoke`, which takes several calls, then the
    /// slots of the copies emptied.
    fn fan(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let classes = [
            UNTYPED, CSPACE, THREAD, THREAD, ENDPOINT, ENDPOINT, REPLY, REPLY,
        ];
        let class = if self.rng.chance(50) {
            self.rng.pick(&classes)
        } else {
            self.rng.pick(&[NOTIFY, OTHER])
        };
        let Some(source) = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)))
        else {
            return Vec::new();
        };
        let wanted = REMOVAL_STEP + 1 + self.rng.below(6) as usize;
        let slots = self.spare(plan, wanted);
        if slots.len() < wanted {
            return Vec::new();
        }
        let removal = if self.rng.chance(60) {
            Syscall::CapDelete(own(source))
        } else {
            Syscall::CapRevoke(own(source))
        };
        let copies = slots.iter().map(|&to| derive(own(source), own(to)));
        let emptied = slots.iter().map(|&slot| Syscall::CapDelete(own(slot)));
        copies.chain(vec![removal; 3]).chain(emptied).collect()
    }

    /// A process: a capability space made from the untyped in slot 107, laid
    /// out as this one, with endpoints, reply objects and notifies of its
    /// own and copies of some of this space's, and a thread that runs in it.
    fn spawn(&mut self) -> Vec<Syscall> {
        let untyped = self
            .inspect(MEMORY)
            .is_some_and(|(kind, _)| kind == ObjectKind::Untyped);
        let classes = (
            self.find_own(CSPACE, Wanted::Free),
            self.find_own(THREAD, Wanted::Free),
        );
        let (Some(space), Some(thread)) = classes else {
            return Vec::new();
        };
        if !untyped || !self.whole(MEMORY) || self.model.runnable().len() > 14 {
            return Vec::new();
        }
        let inside = |index: u32| SlotAddr::through(space, index);
        let grown = Syscall::CSpaceGrow {
            cspace: own(space),
            untyped: MEMORY,
        };
        let mut calls = vec![retype(ObjectKind::CSpace, CHILD_SLOTS.into(), own(space))];
        calls.extend(vec![grown; (CHILD_SLOTS / CSPACE_SEGMENT_SLOTS) as usize]);
        calls.push(derive(MEMORY, inside(MEMORY_SLOT)));
        calls.push(derive(own(space), inside(START_SLOT_CSPACE)));
        // The address space and the core capability of this space, so that
        // the threads of the new one can run and have contexts.
        calls.push(derive(own(START_SLOT_ASPACE), inside(START_SLOT_ASPACE)));
        calls.push(derive(own(CORE_SLOT), inside(CORE_SLOT)));
        for (kind, class) in [
            (ObjectKind::Endpoint, ENDPOINT),
            (ObjectKind::Reply, REPLY),
            (ObjectKind::Notify, NOTIFY),
            (ObjectKind::Frame, OTHER),
        ] {
            let size = (kind == ObjectKind::Frame) as u64;
            calls.push(retype(kind, size, inside(34 + class)));
            calls.push(retype(kind, size, inside(43 + class)));
            // A capability of this space, shared with the new one.
            if let Some(shared) = self.find_own(class, Wanted::Full) {
                calls.push(derive(own(shared), inside(52 + class)));
            }
        }
        // Two threads in the new space, so that one can serve the other.
        // The capability of the second goes into the new space.
        // Each has one of the two frames of the new space as its message
        // page.
        // Each has a context of its own, now and then none: a passive one.
        // The first one's is in this space, and outlives the new one, as
        // the first thread does; the second one's is in the new space, and
        // dies with it. Now and then a thread has no address space, and
        // faults when it would run.
        // Without a free slot here, the first one's context goes in slot
        // 105 of the new space, which nothing else fills (the final
        // review's H10: slot 107 holds the untyped, and the retype failed).
        let kept = self.find_own(OTHER, Wanted::Free).map_or(inside(105), own);
        let threads = [
            (own(thread), 34, kept),
            (inside(34 + THREAD), 43, inside(106)),
        ];
        for (thread, page, sched) in threads {
            calls.push(retype(ObjectKind::Thread, 0, thread));
            calls.push(Syscall::ThreadConfigure {
                thread,
                cspace: own(space),
                aspace: if self.rng.chance(10) {
                    SlotAddr::NONE
                } else {
                    own(START_SLOT_ASPACE)
                },
                fault: inside(34 + ENDPOINT),
                page: inside(page + OTHER),
            });
            if self.rng.chance(80) {
                calls.push(retype(ObjectKind::Sched, 0, sched));
                calls.push(Syscall::SchedConfigure {
                    sched,
                    node: own(CORE_SLOT),
                    core: 0,
                    budget: SCHED_MIN_BUDGET_NS,
                    period: 10_000_000,
                    class: self.rng.below(3),
                });
                calls.push(Syscall::SchedBind { sched, thread });
            }
            calls.push(Syscall::ThreadResume(thread));
        }
        calls
    }

    /// A nest of spaces, one at each depth, each holding in its slot 2 a
    /// copy of the untyped at its own depth, whose most recent child is the
    /// root capability of the next space; in the deepest, a chain of copies
    /// of an ordinary capability. Then its removal from the first space,
    /// whose walk is as long as the nest is deep, and the slots emptied.
    fn nest(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        if !self.whole(MEMORY) {
            return Vec::new();
        }
        let wanted = 24 + self.rng.below(22) as usize;
        let slots = self.spare(plan, wanted);
        let [held, roots @ ..] = slots.as_slice() else {
            return Vec::new();
        };
        if roots.len() < 12 {
            return Vec::new();
        }
        let mut calls = Vec::new();
        let mut untyped = MEMORY;
        for &root in roots {
            calls.push(derive(untyped, own(*held)));
            calls.push(Syscall::UntypedRetype {
                untyped,
                kind: ObjectKind::CSpace,
                size: CSPACE_SEGMENT_SLOTS.into(),
                page: 0,
                memory: 0,
                destination: own(root),
            });
            calls.push(Syscall::CSpaceGrow {
                cspace: own(root),
                untyped: MEMORY,
            });
            untyped = SlotAddr::through(root, 2);
            calls.push(Syscall::CapMove {
                source: own(*held),
                destination: untyped,
            });
        }
        let deepest = |index: u32| SlotAddr::through(roots[roots.len() - 1], index);
        let class = self.rng.pick(&[ENDPOINT, NOTIFY, REPLY]);
        if let Some(ordinary) = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)))
        {
            calls.push(derive(own(ordinary), deepest(1)));
            calls.push(derive(deepest(1), deepest(3)));
            calls.extend((3..40).map(|index| derive(deepest(index), deepest(index + 1))));
        }
        // One call for each space to sweep, and a few for the chain.
        calls.extend(vec![Syscall::CapDelete(own(roots[0])); roots.len() + 6]);
        calls.extend(slots.iter().map(|&slot| Syscall::CapDelete(own(slot))));
        calls
    }

    /// Empties the slots that scripts filled and that are no longer full.
    fn tidy(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let Some(used) = self.space.and_then(|space| plan.used.get_mut(&space)) else {
            return Vec::new();
        };
        let stale: Vec<u32> = used
            .iter()
            .copied()
            .filter(|&slot| !self.full(own(slot)))
            .take(40)
            .collect();
        used.retain(|slot| !stale.contains(slot));
        stale
            .into_iter()
            .map(|slot| Syscall::CapDelete(own(slot)))
            .collect()
    }

    /// Waits to receive, with a range of four free slots, on an endpoint of
    /// this space; `speak` is its other half.
    fn listen(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let slots = (
            self.find_own(ENDPOINT, Wanted::Full),
            self.find_own(REPLY, Wanted::Full),
        );
        let (Some(endpoint), Some(reply), Some(first), Some(space)) =
            (slots.0, slots.1, self.four_free(), self.space)
        else {
            return Vec::new();
        };
        plan.listening.insert(space, endpoint);
        // Now and then the range is in a space this one holds, as deep as a
        // space whose root capability `speak` may move there: the depths
        // refuse that move (8.6; the final review's H6: only the directed
        // case reached a refused send).
        let range = match self.rng.chance(40) {
            true => Range {
                first: self.inside(SPARE),
                count: 1,
            },
            false => Range { first, count: 4 },
        };
        vec![Syscall::Recv {
            endpoint: own(endpoint),
            reply: own(reply),
            range,
            wait: 0,
        }]
    }

    /// A message of four capabilities for the thread that `listen` made
    /// wait in this capability space, if there is one.
    fn speak(&mut self, plan: &mut Plan) -> Option<Syscall> {
        let endpoint = plan.listening.remove(&self.space?)?;
        let caps = match self.rng.chance(40) {
            true => vec![Transfer::Move(self.bound())],
            false => self.four(),
        };
        let message = Message {
            caps,
            ..Message::default()
        };
        let range = self.range();
        Some(if self.rng.chance(50) {
            Syscall::Send {
                target: own(endpoint),
                message,
            }
        } else {
            Syscall::Call {
                endpoint: own(endpoint),
                message,
                range,
                wait: 0,
            }
        })
    }

    /// The end of a process: the capability space in a slot of this one is
    /// deleted, whatever runs in it.
    fn kill(&mut self) -> Vec<Syscall> {
        let space = self.find_own(CSPACE, Wanted::Full);
        space.map_or(Vec::new(), |space| vec![Syscall::CapDelete(own(space)); 10])
    }

    /// An address space and an iospace from the untyped in slot 107, frames
    /// mapped into both, devices attached, rights changed; then the frames,
    /// the spaces or the mappings removed, in a random order.
    fn spaces(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        if !self.whole(MEMORY) {
            return Vec::new();
        }
        // The slots are taken at once: `spare` sees only what is full now.
        let slots = self.spare(plan, 8 + 33);
        let [aspace, iospace, frames @ ..] = slots.as_slice() else {
            return Vec::new();
        };
        if frames.len() < 6 + 33 {
            return Vec::new();
        }
        let (frames, copies) = frames.split_at(6);
        let mut calls = vec![
            retype(ObjectKind::ASpace, 0, own(*aspace)),
            retype(ObjectKind::IoSpace, 0, own(*iospace)),
        ];
        calls.extend(
            frames
                .iter()
                .map(|&frame| retype(ObjectKind::Frame, 2, own(frame))),
        );
        let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
        let base = (1 << 30) + self.rng.below(4) * root_span();
        for (n, &frame) in frames.iter().enumerate() {
            let far = if self.rng.chance(30) { 1 << 30 } else { 0 };
            let rights = self.rng.pick(&[r, r.union(w), r.union(x)]);
            calls.push(Syscall::ASpaceMap(Map {
                space: own(*aspace),
                frame: own(frame),
                untyped: MEMORY,
                address: base + far + 2 * page() * n as u64,
                first: 0,
                count: 1 + self.rng.below(2),
                rights: rights.bits(),
            }));
        }
        calls.push(Syscall::ASpaceProtect {
            aspace: own(*aspace),
            frame: own(frames[0]),
            rights: r.bits(),
        });
        // Now and then 33 copies of a frame, each mapped, so that the
        // destruction of the aspace takes more than one call.
        if self.rng.chance(30) {
            for (n, &copy) in copies.iter().enumerate() {
                calls.push(derive(own(frames[4]), own(copy)));
                calls.push(Syscall::ASpaceMap(Map {
                    space: own(*aspace),
                    frame: own(copy),
                    untyped: MEMORY,
                    address: (1 << 31) + page() * n as u64,
                    first: 0,
                    count: 1,
                    rights: r.bits(),
                }));
            }
            calls.extend(vec![Syscall::CapDelete(own(*aspace)); 3]);
        }
        for n in 0..1 + self.rng.below(3) {
            calls.push(Syscall::IoSpaceAttach {
                iommu: own(IOMMU_SLOT),
                iospace: own(*iospace),
                device: 0x20 + n,
            });
        }
        // A copy of a mapped frame, mapped into the iospace.
        let copy = frames[5];
        calls.push(Syscall::CapDelete(own(copy)));
        calls.push(derive(own(frames[1]), own(copy)));
        calls.push(Syscall::IoSpaceMap(Map {
            space: own(*iospace),
            frame: own(copy),
            untyped: MEMORY,
            address: 0x1_0000,
            first: 0,
            count: 2,
            rights: w.bits(),
        }));
        let mut ends = vec![
            vec![Syscall::CapDelete(own(*aspace)); 2],
            vec![Syscall::CapDelete(own(*iospace)); 2],
            vec![Syscall::CapRevoke(own(frames[1])); 2],
            vec![Syscall::ASpaceUnmap(own(frames[2]))],
            vec![Syscall::IoSpaceUnmap(own(copy))],
            frames
                .iter()
                .map(|&frame| Syscall::CapDelete(own(frame)))
                .collect(),
        ];
        while !ends.is_empty() {
            let at = self.rng.below(ends.len() as u64) as usize;
            calls.extend(ends.swap_remove(at));
        }
        calls
    }

    /// An object of a kind that the other scripts seldom leave dying, with
    /// 33 copies of its capability, then the removal of its root, which
    /// takes several calls, then the copies emptied: a timer, a task, an
    /// iospace from the untyped in slot 107; in the start-up space a role
    /// (the final review's H8; the memory_control is removed late in the
    /// run, in `run`).
    fn crowd(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let home = self.space == Some(self.home);
        let choice = self.rng.below(10);
        let slots = self.spare(plan, REMOVAL_STEP + 2);
        let [first, copies @ ..] = slots.as_slice() else {
            return Vec::new();
        };
        if copies.len() <= REMOVAL_STEP {
            return Vec::new();
        }
        let mut calls = Vec::new();
        let original = match choice {
            1..3 if home => own(common::ROLES + self.rng.below(5) as u32),
            _ => {
                if !self.whole(MEMORY) {
                    return Vec::new();
                }
                let kinds = [ObjectKind::Timer, ObjectKind::Task, ObjectKind::IoSpace];
                calls.push(retype(self.rng.pick(&kinds), 0, own(*first)));
                own(*first)
            }
        };
        // Copies without `exec`, which a role capability does not have.
        let copy = |to: &u32| Syscall::CapDerive {
            source: original,
            destination: own(*to),
            rights: Rights::ALL.without(Rights::EXEC).bits(),
            badge: 0,
        };
        calls.extend(copies.iter().map(copy));
        calls.extend(vec![Syscall::CapDelete(original); 3]);
        calls.extend(copies.iter().map(|&slot| Syscall::CapDelete(own(slot))));
        calls
    }

    /// A thread of this space that runs, suspended, given a breakpoint, and
    /// resumed: a thread that a fault of kind 9 can then hit (ver-D M1).
    fn watch(&mut self) -> Vec<Syscall> {
        // A thread that runs: one with no context never faults.
        let runs = self.model.runnable();
        let tries: Vec<u32> = (0..8).map(|_| self.class_slot(THREAD)).collect();
        let found = tries.into_iter().find(|&slot| {
            let object = self.model.object_at(self.thread, own(slot));
            object.is_some_and(|t| t != self.thread && runs.contains(&t))
        });
        let Some(slot) = found else {
            return Vec::new();
        };
        let mut block = vec![0; x86_64::REGS_WORDS];
        block[x86_64::REG_RFLAGS] = 0x202;
        block[x86_64::REG_DR0] = 0x1000;
        block[x86_64::REG_DR7] = 0x1;
        let thread = own(slot);
        vec![
            Syscall::ThreadSuspend(thread),
            Syscall::ThreadWriteRegs { thread, block },
            Syscall::ThreadResume(thread),
        ]
    }

    fn script(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        match self.rng.below(100) {
            0..18 => self.spawn(),
            18..44 => self.chain(plan),
            44..66 => self.fan(plan),
            66..74 => self.nest(plan),
            74..78 => self.kill(),
            78..86 => self.listen(plan),
            86..94 => self.spaces(plan),
            94..97 => self.crowd(plan),
            97..99 => self.watch(),
            _ => self.tidy(plan),
        }
    }
}

/// What a run reached.
#[derive(Debug, Default, PartialEq)]
struct Reached {
    /// Calls that did not fail, by their place in `NAMES`.
    ok: Vec<u32>,
    /// Calls that returned each error, by its code.
    errors: [u32; 12],
    /// Times nothing of use could run, and the run went on from the
    /// start-up state.
    restarts: u32,
    /// Steps at which several threads could run.
    several_threads: u32,
    /// Calls made by threads whose capability space is not the start-up
    /// space, and those of them that did not fail.
    outside: u32,
    outside_ok: u32,
    /// Calls that a script made, and those of them that did not fail.
    scripted: u32,
    scripted_ok: u32,
    /// Messages received with capabilities, with the flag `caps_dropped`,
    /// and with the flag `notification`; each counted once, when received.
    with_caps: u32,
    dropped: u32,
    notifications: u32,
    /// Threads woken with `gone`, each counted once.
    woken_gone: u32,
    /// The deepest capability, and the longest walk of a removal.
    deepest: usize,
    longest_walk: usize,
    /// Steps after which an object of each kind was dying, by kind.
    dying: [u32; ObjectKind::ALL.len()],
    /// Steps after which a capability under `cap_delete` was dying while its
    /// object was not.
    dying_copies: u32,
    /// Steps after which a thread waited to be received, waited for a reply
    /// through an armed reply object, waited to receive, waited on a notify;
    /// and after which a notify was bound.
    callers: u32,
    served: u32,
    receivers: u32,
    waiters: u32,
    bound: u32,
    /// `cap_move` of an untyped capability (first) or of a cspace
    /// capability (second) through a cspace capability: those accepted, and
    /// those refused only because of the depths.
    moved: [u32; 2],
    refused: [u32; 2],
    /// `send` that lists such a move and found a receiver: those delivered
    /// with their capabilities, and those dropped only because of the
    /// depths.
    sent: u32,
    sent_refused: u32,
    /// `call` and `reply_recv` that list such a move and did not fail.
    called: u32,
    answered: u32,
    /// Messages received with four capabilities.
    with_four: u32,
    /// Calls made by a thread whose capability space is destroyed.
    destroyed: u32,
    /// `thread_cancel` that ended a wait: of a queued caller, of a caller
    /// that waits for the reply, of a receiver, of a waiter on a notify.
    cancelled: [u32; 4],
    /// `thread_cancel` with a wait number other than 0 that ended a wait.
    cancelled_by_number: u32,
    /// `call`, `recv`, `reply_recv` and `wait` that returned `cancelled` at
    /// once: their wait number was already cancelled.
    cancelled_at_once: [u32; 4],
    /// `untyped_retype` that returned `unsupported`: a kind not made.
    retype_unsupported: u32,
    /// `cap_delete` and `cap_revoke` that returned `again`, and those of
    /// them that removed no capability: the step ended on the slot budget
    /// alone.
    again: u32,
    swept_only: u32,
    /// Seeds in which a random removal took more than one call (the
    /// directed case's removal does not count).
    seeds_walked: u32,
    /// Steps after which a frame capability held a mapping, and a device
    /// was attached; removals that undid a mapping, and that detached a
    /// device.
    mapped: u32,
    attached: u32,
    /// Faults that found no handler, and faults delivered; steps after
    /// which a thread waited on a fault.
    faults: [u32; 2],
    faulting: u32,
    /// Breakpoint hits given (faults of kind 9), and steps after which a
    /// thread waited on one.
    breakpoints: u32,
    breakpoint_waits: u32,
    /// Steps after which: a context was throttled, a context was lent, a
    /// timer was armed, a thread was marked as having lost its context,
    /// the core was idle, the running context ran on slack, a thread waited
    /// on a fault *no address space*, on a fault *no context*.
    throttled: u32,
    lent: u32,
    armed: u32,
    lost: u32,
    idle: u32,
    on_slack: u32,
    no_aspace: u32,
    no_context: u32,
    /// Times the clock moved, and those in which a timer fired.
    advanced: u32,
    fired: u32,
    /// Interrupts that arrived; threads whose effective marks grew; steps
    /// after which an irq was claimed, one was fired, a task was revoked.
    interrupts: u32,
    marked: u32,
    claimed: u32,
    irq_fired: u32,
    revoked: u32,
    unmapped_by_removal: u32,
    detached_by_removal: u32,
}

impl Reached {
    fn note(&mut self, outcome: &Outcome) {
        self.woken_gone += (*outcome == Err(Error::Gone)) as u32;
        if let Ok(Output::Message(delivery)) = outcome {
            let header = delivery.received.header;
            self.with_caps += (header.caps() > 0) as u32;
            self.with_four += (header.caps() == MAX_CAPS) as u32;
            self.dropped += (header.flags() & Header::FLAG_CAPS_DROPPED != 0) as u32;
            self.notifications += (header.flags() & Header::FLAG_NOTIFICATION != 0) as u32;
        }
    }

    fn count(&mut self, model: &Model) {
        let census = model.census();
        self.deepest = self.deepest.max(census.deepest);
        self.longest_walk = self.longest_walk.max(model.longest_walk());
        // One step counts once per kind: `Census::dying` is in the order of
        // the objects, so equal kinds are made neighbours first (the final
        // review's D2 = H7).
        let mut kinds = census.dying;
        kinds.sort_by_key(|kind| *kind as u8);
        kinds.dedup();
        kinds
            .iter()
            .for_each(|&kind| self.dying[kind as usize] += 1);
        self.dying_copies += (census.dying_copies > 0) as u32;
        self.callers += (census.callers > 0) as u32;
        self.served += (census.served > 0) as u32;
        self.receivers += (census.receivers > 0) as u32;
        self.waiters += (census.waiters > 0) as u32;
        self.bound += (census.bound > 0) as u32;
        self.mapped += (census.mappings > 0) as u32;
        self.faulting += (census.faulting > 0) as u32;
        self.throttled += (census.throttled > 0) as u32;
        self.lent += (census.lends > 0) as u32;
        self.armed += (census.armed > 0) as u32;
        self.lost += (census.lost > 0) as u32;
        self.idle += census.idle as u32;
        self.on_slack += census.on_slack as u32;
        self.no_aspace += (census.fault_kinds[FaultKind::NoAddressSpace as usize] > 0) as u32;
        self.no_context += (census.fault_kinds[FaultKind::NoContext as usize] > 0) as u32;
        self.breakpoint_waits +=
            (census.fault_kinds[FaultKind::HardwareBreakpoint as usize] > 0) as u32;
        self.claimed += (census.irqs > 0) as u32;
        self.irq_fired += (census.fired > 0) as u32;
        self.revoked += (census.revoked > 0) as u32;
        self.attached += (census.devices > 0) as u32;
    }
}

/// What `at_risk` found about a call before it was made.
struct Risk {
    /// 0 for an untyped capability, 1 for a cspace capability.
    kind: usize,
    /// The call succeeds with a capability that every space can hold in
    /// its place: a copy of the thread capability in its start-up slot.
    fits_derived: bool,
}

/// Whether a call moves an untyped or a cspace capability, by `cap_move`
/// through a cspace capability or by `send`. It asks copies of the state.
fn at_risk(model: &Model, thread: ObjId, call: &Syscall) -> Option<Risk> {
    let on_copy = |call: Syscall| model.clone().apply(thread, call);
    let kind = |slot: SlotAddr| match on_copy(Syscall::CapInspect(slot)) {
        Ok(Output::Inspect {
            kind: ObjectKind::Untyped,
            ..
        }) => Some(0),
        Ok(Output::Inspect {
            kind: ObjectKind::CSpace,
            ..
        }) => Some(1),
        _ => None,
    };
    let ordinary = own(START_SLOT_THREAD);
    match call {
        &Syscall::CapMove {
            source,
            destination,
        } if source.via() | destination.via() != 0 => {
            let derived = Syscall::CapDerive {
                source: ordinary,
                destination,
                rights: 0,
                badge: 0,
            };
            Some(Risk {
                kind: kind(source)?,
                fits_derived: on_copy(derived).is_ok(),
            })
        }
        Syscall::Send { target, message } => {
            let mut derived = message.clone();
            let mut found = None;
            for transfer in &mut derived.caps {
                if let Transfer::Move(slot) = *transfer {
                    found = found.or(kind(slot));
                    *transfer = Transfer::Derive(ordinary, Rights::NONE.bits());
                }
            }
            let derived = Syscall::Send {
                target: *target,
                message: derived,
            };
            let fits_derived = on_copy(derived) == Ok(Output::Sent { flags: 0 });
            Some(Risk {
                kind: found?,
                fits_derived,
            })
        }
        Syscall::Call { message, .. } | Syscall::ReplyRecv { message, .. } => {
            let moved = |transfer: &Transfer| match *transfer {
                Transfer::Move(slot) => kind(slot),
                Transfer::Derive(..) => None,
            };
            Some(Risk {
                kind: message.caps.iter().find_map(moved)?,
                fits_derived: false,
            })
        }
        _ => None,
    }
}

/// The start-up state, in which the first thread's context keeps half of the
/// core, so that other contexts can be admitted.
fn start() -> (Model, ObjId) {
    let (mut model, first) = Model::boot(&Machine::example()).unwrap();
    let half = Syscall::SchedConfigure {
        sched: own(START_SLOT_SCHED),
        node: own(CORE_SLOT),
        core: 0,
        budget: 5_000_000,
        period: 10_000_000,
        class: 1,
    };
    assert_eq!(model.apply(first, half), Ok(Output::Done));
    let chunk = model.geometry().chunk_shift();
    common::take_run(&mut model, first, chunk + 5, MEMORY_SLOT);
    common::take_device(&mut model, first);
    let to_place = Syscall::CapMove {
        source: own(common::DEVICE),
        destination: own(DEVICE_SLOT),
    };
    assert_eq!(model.apply(first, to_place), Ok(Output::Done));
    (model, first)
}

/// Before the random calls: a child untyped of the memory with 40
/// endpoints in it, then its removal, which takes two calls, so that an
/// untyped is dying across a call; then the slots are emptied. It uses
/// slots that the scripts never use. It is asserted here and counts in no
/// floor: the floors measure the random calls (the final review's H6).
fn crowd(model: &mut Model, first: ObjId) {
    let slots: Vec<u32> = (4000..4041).collect();
    let mut calls = vec![retype(ObjectKind::Untyped, 1 << 16, own(slots[0]))];
    calls.extend(slots[1..].iter().map(|&slot| Syscall::UntypedRetype {
        untyped: own(slots[0]),
        kind: ObjectKind::Endpoint,
        size: 0,
        page: 0,
        memory: 0,
        destination: own(slot),
    }));
    let removal = calls.len();
    calls.extend(vec![Syscall::CapDelete(own(slots[0])); 2]);
    calls.extend(slots[1..].iter().map(|&slot| Syscall::CapDelete(own(slot))));
    for (index, call) in calls.into_iter().enumerate() {
        let outcome = model.apply(first, call);
        assert!(matches!(outcome, Ok(_) | Err(Error::Again)), "{outcome:?}");
        assert_eq!(model.check(), Ok(()));
        if index == removal {
            assert_eq!(outcome, Err(Error::Again));
            assert!(model.census().dying.contains(&ObjectKind::Untyped));
        }
    }
}

/// The directed case of a `send` whose listed move the depth rule refuses
/// at delivery (8.6): a receiver waits with its range in a capability
/// space whose root is as deep as the root of the space being moved, so
/// the capability is dropped and `send` returns the flag `caps_dropped`;
/// then the same move into the start-up space, whose root is less deep,
/// is accepted. Then every object of the case is removed. It uses slots
/// that the scripts never use, and counts in no floor.
fn refused_send(model: &mut Model, first: ObjId) {
    let s = |index: u32| own(4100 + index);
    let make = |kind: ObjectKind, size: u64, slot: SlotAddr| Syscall::UntypedRetype {
        untyped: MEMORY,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: slot,
    };
    let setup = [
        make(ObjectKind::Endpoint, 0, s(0)),
        make(ObjectKind::Reply, 0, s(1)),
        make(ObjectKind::Thread, 0, s(2)),
        make(ObjectKind::Frame, 1, s(3)),
        make(ObjectKind::Sched, 0, s(4)),
        make(ObjectKind::CSpace, 128, s(5)),
        Syscall::CSpaceGrow {
            cspace: s(5),
            untyped: MEMORY,
        },
        make(ObjectKind::CSpace, 128, s(6)),
        Syscall::ThreadConfigure {
            thread: s(2),
            cspace: HERE,
            aspace: own(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: s(3),
        },
        Syscall::SchedConfigure {
            sched: s(4),
            node: own(CORE_SLOT),
            core: 0,
            budget: 100_000,
            period: 10_000_000,
            class: 1,
        },
        Syscall::SchedBind {
            sched: s(4),
            thread: s(2),
        },
        Syscall::ThreadResume(s(2)),
    ];
    let apply = |model: &mut Model, thread: ObjId, call: Syscall| {
        let outcome = model.apply(thread, call);
        assert_eq!(model.check(), Ok(()));
        outcome
    };
    for call in setup {
        assert!(apply(model, first, call).is_ok());
    }
    let receiver = model.object_at(first, s(2)).expect("the receiver");
    let send = Syscall::Send {
        target: s(0),
        message: Message {
            caps: vec![Transfer::Move(s(6))],
            ..Message::default()
        },
    };
    for (range, flags) in [
        (SlotAddr::through(4105, 1), Header::FLAG_CAPS_DROPPED),
        (s(7), 0),
    ] {
        let recv = Syscall::Recv {
            endpoint: s(0),
            reply: s(1),
            range: Range {
                first: range,
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(apply(model, receiver, recv), Ok(Output::Waits));
        assert_eq!(
            apply(model, first, send.clone()),
            Ok(Output::Sent { flags })
        );
    }
    for index in (0..8).rev() {
        for _ in 0..16 {
            if apply(model, first, Syscall::CapDelete(s(index))) != Err(Error::Again) {
                break;
            }
        }
    }
}

/// The directed case of a thread whose capability space is destroyed while
/// it can run: it gets `gone` from `cap_delete` as from every call. Then
/// the thread is removed. It uses slots that the scripts never use, and
/// counts in no floor.
fn destroyed_space(model: &mut Model, first: ObjId) {
    let s = |index: u32| own(4120 + index);
    let make = |kind: ObjectKind, size: u64, slot: SlotAddr| Syscall::UntypedRetype {
        untyped: MEMORY,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: slot,
    };
    let setup = [
        make(ObjectKind::CSpace, 128, s(0)),
        make(ObjectKind::Thread, 0, s(1)),
        make(ObjectKind::Sched, 0, s(2)),
        Syscall::ThreadConfigure {
            thread: s(1),
            cspace: s(0),
            aspace: own(START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: SlotAddr::NONE,
        },
        Syscall::SchedConfigure {
            sched: s(2),
            node: own(CORE_SLOT),
            core: 0,
            budget: 100_000,
            period: 10_000_000,
            class: 1,
        },
        Syscall::SchedBind {
            sched: s(2),
            thread: s(1),
        },
        Syscall::ThreadResume(s(1)),
    ];
    for call in setup {
        assert!(model.apply(first, call).is_ok());
        assert_eq!(model.check(), Ok(()));
    }
    let orphan = model.object_at(first, s(1)).expect("the thread");
    for _ in 0..16 {
        if model.apply(first, Syscall::CapDelete(s(0))) != Err(Error::Again) {
            break;
        }
        assert_eq!(model.check(), Ok(()));
    }
    assert_eq!(
        model.apply(orphan, Syscall::CapDelete(HERE)),
        Err(Error::Gone)
    );
    assert_eq!(model.check(), Ok(()));
    for index in [1, 2] {
        for _ in 0..16 {
            if model.apply(first, Syscall::CapDelete(s(index))) != Err(Error::Again) {
                break;
            }
        }
        assert_eq!(model.check(), Ok(()));
    }
}

fn run(seed: u64, reached: &mut Reached) -> Model {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (mut model, first) = start();
    let home = model.object_at(first, HERE).expect("the start-up space");
    let mut plan = Plan::default();
    // Every thread that could run at some step: those that can be woken.
    let mut known: BTreeSet<ObjId> = BTreeSet::new();
    let mut walked = false;
    crowd(&mut model, first);
    refused_send(&mut model, first);
    destroyed_space(&mut model, first);
    // The directed cases alone: what they leave in the counters that take a
    // maximum (the deepest capability, the longest walk).
    if steps() == 0 {
        reached.count(&model);
    }
    let mut control_due = false;
    for step in 0..steps() {
        if step == steps() - steps() / 10 {
            control_due = rng.chance(50);
        }
        let threads = model.runnable();
        known.extend(&threads);
        // A thread is of use if its capability space holds a capability for
        // itself in slot 1, as the start-up space and those of the scripts
        // do: one that is bound to an empty or a dying space only gets
        // errors.
        let space_of = |thread: &ObjId| model.object_at(*thread, HERE);
        let of_use: Vec<ObjId> = threads
            .iter()
            .copied()
            .filter(|thread| space_of(thread).is_some())
            .collect();
        if of_use.is_empty() {
            // Nothing of use can run: go on from the start-up state.
            reached.restarts += 1;
            (model, _) = start();
            (plan, known) = (Plan::default(), BTreeSet::new());
            continue;
        }
        reached.several_threads += (threads.len() > 1) as u32;
        // A thread in the middle of a script is picked more often.
        let scripted: Vec<ObjId> = of_use
            .iter()
            .copied()
            .filter(|thread| plan.scripts.contains_key(thread))
            .collect();
        let thread = match rng.below(100) {
            0..50 if !scripted.is_empty() => rng.pick(&scripted),
            0..92 => rng.pick(&of_use),
            _ => rng.pick(&threads),
        };
        let space = space_of(&thread);
        // Now and then time passes.
        if rng.chance(4) {
            let to = model.now() + rng.below(3_000_000);
            let armed = model.census().armed;
            model.advance(to);
            reached.advanced += 1;
            reached.fired += (model.census().armed < armed) as u32;
            if let Err(broken) = model.check() {
                panic!("seed {seed}, step {step}, advance to {to}: {broken}");
            }
            reached.count(&model);
            continue;
        }
        // Now and then an interrupt arrives.
        if rng.chance(1) {
            model.interrupt(48 + rng.below(12) as u8);
            if let Err(broken) = model.check() {
                panic!("seed {seed}, step {step}, interrupt: {broken}");
            }
            reached.interrupts += 1;
            reached.count(&model);
            continue;
        }
        // Now and then a thread that runs with a breakpoint enabled hits
        // it: a fault of kind 9 (ver-D M1).
        let watched = threads
            .iter()
            .copied()
            .filter(|t| model.breakpoint_enabled(*t))
            .find(|t| plan.scripts.get(t).is_none_or(VecDeque::is_empty));
        if let Some(watched) = watched.filter(|_| rng.chance(2)) {
            let before = model.census().faulting;
            let hit = [1 << rng.below(4), 0xFFFF_0FF0 | 1, 0];
            model.fault(watched, FaultKind::HardwareBreakpoint, hit);
            if let Err(broken) = model.check() {
                panic!("seed {seed}, step {step}, {watched:?} hits a breakpoint: {broken}");
            }
            reached.breakpoints += 1;
            reached.faults[(model.census().faulting > before) as usize] += 1;
            reached.count(&model);
            continue;
        }
        // Now and then the thread faults instead of making a call.
        if rng.below(200) == 0 && !plan.scripts.contains_key(&thread) {
            let kinds = [
                FaultKind::Page,
                FaultKind::Undefined,
                FaultKind::Arithmetic,
                FaultKind::Protection,
                FaultKind::Debug,
                FaultKind::BadCall,
            ];
            let kind = rng.pick(&kinds);
            let before = model.census().faulting;
            model.fault(thread, kind, [rng.below(32), rng.below(16), rng.next()]);
            if let Err(broken) = model.check() {
                panic!("seed {seed}, step {step}, {thread:?} faults: {broken}");
            }
            let after = model.census().faulting;
            reached.faults[(after > before) as usize] += 1;
            reached.count(&model);
            continue;
        }
        let mut generator = Generator {
            rng: &mut rng,
            model: &model,
            thread,
            space,
            home,
        };
        // The next call of the script of the thread; or the first call of a
        // new script; or one call with random arguments.
        let mut next = None;
        let begins = if space == Some(home) { 3 } else { 12 };
        if let Some(script) = plan.scripts.get_mut(&thread) {
            if generator.rng.chance(92) {
                next = script.pop_front();
            }
        } else if of_use.len() > 1 && generator.rng.chance(30) {
            next = generator.speak(&mut plan);
        } else if generator.rng.chance(begins) {
            let mut script: VecDeque<Syscall> = generator.script(&mut plan).into();
            next = script.pop_front();
            plan.scripts.insert(thread, script);
        }
        if plan.scripts.get(&thread).is_some_and(VecDeque::is_empty) {
            plan.scripts.remove(&thread);
        }
        // From the last tenth of the run, in half of the seeds, the next
        // thread of the start-up space with no script removes the
        // memory_control, under which every block is: it dies over several
        // calls (H8), and little is left to do after.
        if control_due && space == Some(home) && next.is_none() {
            control_due = false;
            let control = own(START_SLOT_MEMORY_CONTROL);
            let mut script: VecDeque<Syscall> = vec![Syscall::CapDelete(control); 3].into();
            next = script.pop_front();
            plan.scripts.insert(thread, script);
        }
        let from_script = next.is_some();
        let call = next.unwrap_or_else(|| {
            let mut call = generator.call();
            // The last thread of use seldom makes a call that waits.
            while of_use.len() == 1
                && BLOCKING.contains(&name_of(&call))
                && generator.rng.chance(98)
            {
                call = generator.call();
            }
            call
        });
        let name = name_of(&call);
        let risk = at_risk(&model, thread, &call);
        let moves = matches!(call, Syscall::CapMove { .. });
        // Destroyed, and not dying or empty: `cap_delete` is `gone` too.
        let gone = |call: Syscall| model.clone().apply(thread, call) == Err(Error::Gone);
        let destroyed = space.is_none() && gone(Syscall::CapDelete(HERE));
        let shown = format!("{thread:?} {call:?}");
        let had_outcome: BTreeSet<ObjId> = known
            .iter()
            .copied()
            .filter(|thread| model.woken(*thread).is_some())
            .collect();

        let waits = |model: &Model| {
            let census = model.census();
            [
                census.callers,
                census.served,
                census.receivers,
                census.waiters,
            ]
        };
        let waited = waits(&model);
        let before = model.census();
        let capabilities = before.capabilities;
        let numbered = matches!(call, Syscall::ThreadCancel(_, number) if number != 0);
        let marks: Vec<(ObjId, Marks)> = known
            .iter()
            .filter_map(|&thread| Some((thread, model.marks(thread)?)))
            .collect();
        let outcome = model.apply(thread, call);
        // A call that waits for a unit ends at the unit's confirmation, at
        // the next step of the recovery reserve of node 0, which holds every
        // unit role: what is due at this instant runs.
        if outcome == Ok(Output::Waits) {
            model.settle();
        }
        // Tasks and marks: the effective marks of a thread never lose a bit
        // while the thread lives.
        for (thread, before) in marks {
            if let Some(after) = model.marks(thread) {
                assert!(
                    after.contains(before),
                    "seed {seed}, step {step}: {thread:?} lost marks"
                );
                reached.marked += (after != before) as u32;
            }
        }
        if name == 19 {
            let after = waits(&model);
            for (kind, (before, after)) in waited.into_iter().zip(after).enumerate() {
                reached.cancelled[kind] += (after < before) as u32;
            }
            reached.cancelled_by_number += (numbered && after != waited) as u32;
        }
        if outcome == Err(Error::Cancelled) {
            let blocking = BLOCKING.iter().position(|&blocking| blocking == name);
            reached.cancelled_at_once[blocking.expect("a call that waits")] += 1;
        }
        reached.retype_unsupported += (name == 5 && outcome == Err(Error::Unsupported)) as u32;
        if name == 2 || name == 3 {
            let after = model.census();
            reached.unmapped_by_removal += (after.mappings < before.mappings) as u32;
            // A removal detaches a device by a unit request: the device
            // leaves the header at the answer, so a step that starts a
            // detach counts as well as one that consumes its answer (D-139).
            let detached = after.devices < before.devices || after.detaching > before.detaching;
            reached.detached_by_removal += detached as u32;
        }
        if (name == 2 || name == 3) && outcome == Err(Error::Again) {
            walked = true;
            reached.again += 1;
            reached.swept_only += (model.census().capabilities == capabilities) as u32;
        }
        if let Err(broken) = model.check() {
            panic!("seed {seed}, step {step}, {shown}: {broken}");
        }

        match (risk, &outcome) {
            (Some(risk), Ok(Output::Done)) => reached.moved[risk.kind] += 1,
            (Some(risk), Err(Error::Invalid)) if moves && risk.fits_derived => {
                reached.refused[risk.kind] += 1
            }
            (Some(_), Ok(Output::Sent { flags: 0 })) => reached.sent += 1,
            (Some(risk), Ok(Output::Sent { .. })) if risk.fits_derived => reached.sent_refused += 1,
            (Some(_), Ok(_)) if name == 10 => reached.called += 1,
            (Some(_), Ok(_)) if name == 12 => reached.answered += 1,
            _ => {}
        }
        reached.destroyed += destroyed as u32;
        match &outcome {
            Ok(_) => {
                reached.ok.resize(NAMES.len(), 0);
                reached.ok[name] += 1;
            }
            Err(error) => reached.errors[*error as usize] += 1,
        }
        reached.outside += (space != Some(home)) as u32;
        reached.outside_ok += (space != Some(home) && outcome.is_ok()) as u32;
        reached.scripted += from_script as u32;
        reached.scripted_ok += (from_script && outcome.is_ok()) as u32;
        // What the call returned, and what it woke: each outcome once.
        if outcome != Err(Error::Gone) {
            reached.note(&outcome);
        }
        let woken = known.iter().filter(|thread| !had_outcome.contains(thread));
        woken
            .filter_map(|&thread| model.woken(thread))
            .for_each(|outcome| reached.note(outcome));
        reached.count(&model);
    }
    // Directed, after the random calls: the node that runs stops the
    // phantom node, the one stop that can succeed beside it (a stop of
    // itself is `busy`, Cores).
    let holder = model
        .runnable()
        .into_iter()
        .find(|&t| model.object_at(t, own(CORE_SLOT + 1)).is_some());
    if let Some(thread) = holder
        && model.apply(thread, stop_call()).is_ok()
    {
        reached.ok.resize(NAMES.len(), 0);
        reached.ok[name_of(&stop_call())] += 1;
    }
    assert_eq!(model.check(), Ok(()));
    reached.seeds_walked += walked as u32;
    model
}

/// The directed stop of the phantom node, to name it.
fn stop_call() -> Syscall {
    Syscall::NodeStop {
        node: own(CORE_SLOT + 1),
        mode: NODE_STOP_KILL,
        core: 0,
    }
}

/// Every seed, run once for the tests below. A run panics if an invariant
/// fails.
fn all_seeds() -> &'static Reached {
    &seeds().0
}

impl Reached {
    /// What another run reached, added: counts summed, the deepest and the
    /// longest walk kept.
    fn add(&mut self, other: &Reached) {
        if self.ok.len() < other.ok.len() {
            self.ok.resize(other.ok.len(), 0);
        }
        self.restarts += other.restarts;
        self.several_threads += other.several_threads;
        self.outside += other.outside;
        self.outside_ok += other.outside_ok;
        self.scripted += other.scripted;
        self.scripted_ok += other.scripted_ok;
        self.with_caps += other.with_caps;
        self.dropped += other.dropped;
        self.notifications += other.notifications;
        self.woken_gone += other.woken_gone;
        self.dying_copies += other.dying_copies;
        self.callers += other.callers;
        self.served += other.served;
        self.receivers += other.receivers;
        self.waiters += other.waiters;
        self.bound += other.bound;
        self.sent += other.sent;
        self.sent_refused += other.sent_refused;
        self.called += other.called;
        self.answered += other.answered;
        self.with_four += other.with_four;
        self.destroyed += other.destroyed;
        self.cancelled_by_number += other.cancelled_by_number;
        self.retype_unsupported += other.retype_unsupported;
        self.again += other.again;
        self.swept_only += other.swept_only;
        self.seeds_walked += other.seeds_walked;
        self.mapped += other.mapped;
        self.attached += other.attached;
        self.faulting += other.faulting;
        self.breakpoints += other.breakpoints;
        self.breakpoint_waits += other.breakpoint_waits;
        self.throttled += other.throttled;
        self.lent += other.lent;
        self.armed += other.armed;
        self.lost += other.lost;
        self.idle += other.idle;
        self.on_slack += other.on_slack;
        self.no_aspace += other.no_aspace;
        self.no_context += other.no_context;
        self.advanced += other.advanced;
        self.fired += other.fired;
        self.interrupts += other.interrupts;
        self.marked += other.marked;
        self.claimed += other.claimed;
        self.irq_fired += other.irq_fired;
        self.revoked += other.revoked;
        self.unmapped_by_removal += other.unmapped_by_removal;
        self.detached_by_removal += other.detached_by_removal;
        for (sum, n) in self.ok.iter_mut().zip(&other.ok) {
            *sum += n;
        }
        for (sum, n) in self.errors.iter_mut().zip(&other.errors) {
            *sum += n;
        }
        for (sum, n) in self.dying.iter_mut().zip(&other.dying) {
            *sum += n;
        }
        for (sum, n) in self.moved.iter_mut().zip(&other.moved) {
            *sum += n;
        }
        for (sum, n) in self.refused.iter_mut().zip(&other.refused) {
            *sum += n;
        }
        for (sum, n) in self.cancelled.iter_mut().zip(&other.cancelled) {
            *sum += n;
        }
        for (sum, n) in self
            .cancelled_at_once
            .iter_mut()
            .zip(&other.cancelled_at_once)
        {
            *sum += n;
        }
        for (sum, n) in self.faults.iter_mut().zip(&other.faults) {
            *sum += n;
        }
        self.deepest = self.deepest.max(other.deepest);
        self.longest_walk = self.longest_walk.max(other.longest_walk);
    }
}

/// The runs of every seed, once: what they reached, and the final state of
/// each, which the tests of determinism compare without running again.
fn seeds() -> &'static (Reached, std::sync::Mutex<Vec<Model>>) {
    static SEEDS_RUN: OnceLock<(Reached, std::sync::Mutex<Vec<Model>>)> = OnceLock::new();
    SEEDS_RUN.get_or_init(|| {
        // The seeds run in parallel, each on its own; what they reached is
        // added in the order of the seeds.
        let runs: Vec<(Reached, Model)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..SEEDS)
                .map(|seed| {
                    scope.spawn(move || {
                        let mut reached = Reached::default();
                        let state = run(seed, &mut reached);
                        (reached, state)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut reached = Reached::default();
        let mut states = Vec::new();
        for (one, state) in runs {
            reached.add(&one);
            states.push(state);
        }
        println!("{reached:#?}");
        (reached, std::sync::Mutex::new(states))
    })
}

#[test]
fn invariants_hold_after_every_call_of_random_sequences() {
    let reached = all_seeds();
    // "other" stands for the calls that are not modelled: it never succeeds.
    for (name, &ok) in NAMES.iter().zip(&reached.ok) {
        assert_eq!(ok == 0, NEVER_ON_ONE_NODE.contains(name), "{name}");
    }
    // Every error code is returned.
    for (code, &count) in reached.errors.iter().enumerate().skip(1) {
        assert!(count > 0, "error code {code}");
    }
    assert!(reached.several_threads as usize > STEPS);
}

// The coverage of the 24 seeds. Each floor is three quarters of the count
// measured on 17 November 2026 (pass 23), rounded up: the smallest of the
// three runs of `verify` (pages of 4, 16 and 32 KB), over the 24 seeds, in
// the comments. A change of the generator that loses a quarter of a kind of
// state fails here, not only one that loses all of it (the eighth model
// review found floors halved and floors of 1 that guarded only against 0).
// The directed cases count in no floor (the final review's H6: three floors
// were below what the directed cases gave alone): with
// `DODOS_PROPERTY_STEPS=0` the counters hold 0 but the deepest capability
// (3), the longest walk (2) and the steps with a mapping (24), each far
// under its floor.

/// Asserts that each count is at least its floor.
#[track_caller]
fn at_least<const N: usize>(counts: [u32; N], floors: [u32; N]) {
    let low = counts
        .iter()
        .zip(&floors)
        .any(|(count, floor)| count < floor);
    assert!(!low, "{counts:?} under {floors:?}");
}

#[test]
fn deepest_capability_and_longest_walk_are_reached() {
    let reached = all_seeds();
    // Measured: depth 64, a walk of 126 capabilities.
    assert_eq!(reached.deepest, TREE_DEPTH);
    assert!(reached.longest_walk >= 95, "{}", reached.longest_walk);
    assert!(reached.longest_walk <= LONGEST_WALK);
    // A random removal takes more than one call in every seed.
    assert_eq!(reached.seeds_walked as u64, SEEDS);
}

#[test]
fn objects_of_every_kind_are_seen_dying() {
    let reached = all_seeds();
    // Measured, in steps (the smallest of the three runs): untyped 2,862,
    // cspace 10,715, aspace 23, frame 8,548, thread 19, sched 19, endpoint
    // 16, reply 19, notify 41, timer 20, irq 4, iospace 10,982, task 20,
    // memory_control 976 (32 KB; 2,383 on the others), role 5. Pass 23 made
    // the walk leave a timer, a task, an iospace and a role dying (`crowd`)
    // and remove the memory_control late in half of the seeds (H8); a copy
    // under `cap_delete` 359.
    let dying = MADE.map(|kind| reached.dying[kind as usize]);
    at_least(
        dying,
        [
            2147, 8037, 18, 6411, 15, 15, 12, 15, 31, 15, 3, 8237, 15, 732, 4,
        ],
    );
    at_least([reached.dying_copies], [270]);
}

#[test]
fn threads_outside_the_start_up_space_mostly_succeed() {
    let reached = all_seeds();
    // Measured: 57,952 calls, 38,842 of them successful.
    at_least([reached.outside], [43464]);
    assert!(
        2 * reached.outside_ok > reached.outside,
        "{}",
        reached.outside_ok
    );
}

#[test]
fn waits_of_every_sort_and_their_ends_are_reached() {
    let reached = all_seeds();
    // Measured, in steps: callers queued 48,534, reply objects armed
    // 13,666, receivers 41,641, waiters on a notify 16,807, a notify bound
    // 64,527.
    let waits = [
        reached.callers,
        reached.served,
        reached.receivers,
        reached.waiters,
        reached.bound,
    ];
    at_least(waits, [36401, 10250, 31231, 12606, 48396]);
    // Measured: 17 messages with capabilities, 5 with the capabilities
    // dropped, 21 notifications, 8 threads woken with `gone`.
    let ends = [
        reached.with_caps,
        reached.dropped,
        reached.notifications,
        reached.woken_gone,
    ];
    at_least(ends, [13, 4, 16, 6]);
    // Measured: 5 messages received with four capabilities, and 10 calls by
    // a thread whose capability space is destroyed (the directed case
    // apart).
    at_least([reached.with_four, reached.destroyed], [4, 8]);
}

/// The invariant "removal ends" is checked after every call above. It says
/// something only if the moves that could break it are tried: here they are
/// counted, accepted and refused, for both kinds and both ways of moving.
#[test]
fn moves_that_the_depths_rule_on_are_accepted_and_refused() {
    let reached = all_seeds();
    // Measured: untyped 3,857 accepted and 68 refused, cspace 84 and 13;
    // by `send` 4 delivered and 1 refused at random (the directed case
    // apart: the final review's H6 found the 24 refused all directed).
    at_least(reached.moved, [2893, 63]);
    at_least(reached.refused, [51, 10]);
    at_least([reached.sent, reached.sent_refused], [3, 1]);
    // The same moves listed by `call` and by `reply_recv`: 23 and 8.
    at_least([reached.called, reached.answered], [18, 6]);
}

#[test]
fn waits_of_every_sort_are_cancelled() {
    let reached = all_seeds();
    // Measured: 39 queued callers, 4 callers that wait for the reply, 24
    // receivers, 11 waiters on a notify; 15 ends by a wait number.
    at_least(reached.cancelled, [30, 3, 18, 9]);
    at_least([reached.cancelled_by_number], [12]);
    // Measured: `call` 11, `recv` 10, `reply_recv` 4 and `wait` 5 returned
    // `cancelled` at once.
    at_least(reached.cancelled_at_once, [9, 8, 3, 4]);
}

#[test]
fn removal_steps_that_only_move_the_sweep_are_reached() {
    let reached = all_seeds();
    // Measured: 210 `again` of a removal removed nothing.
    at_least([reached.swept_only], [158]);
}

#[test]
fn untyped_retype_of_a_kind_not_made_is_reached() {
    let reached = all_seeds();
    // Measured: 9 calls returned `unsupported` for the one kind this
    // version does not make, the reserved `vcpu` (the other kinds never
    // made from an untyped are `invalid` since 16 November 2026).
    at_least([reached.retype_unsupported], [7]);
}

#[test]
fn faults_are_raised_delivered_and_waited_on() {
    let reached = all_seeds();
    // Measured: 139 faults found no handler and 58 were delivered; a thread
    // waited on a fault after 24,561 steps.
    at_least(reached.faults, [105, 44]);
    at_least([reached.faulting], [18421]);
    // A breakpoint hit, a fault of kind 9, given 2 times and waited on
    // after 924 steps (ver-D M1: the walk gave kinds 1 to 6 only).
    at_least([reached.breakpoints, reached.breakpoint_waits], [2, 693]);
}

#[test]
fn scheduling_states_are_reached() {
    let reached = all_seeds();
    // Measured, in steps: a context throttled 86,597, lent 1,955, a timer
    // armed 275, a thread marked as having lost its context 9,051, the
    // core idle 13,001, running on slack 70,147, a fault no address space
    // queued 7,665, a fault no context queued 143.
    let states = [
        reached.throttled,
        reached.lent,
        reached.armed,
        reached.lost,
        reached.idle,
        reached.on_slack,
        reached.no_aspace,
        reached.no_context,
    ];
    at_least(states, [64948, 1467, 207, 6789, 9751, 52611, 5749, 108]);
    // Measured: the clock moved 4,730 times, and a timer fired in 11.
    at_least([reached.advanced, reached.fired], [3548, 9]);
}

#[test]
fn interrupts_and_marks_are_reached() {
    let reached = all_seeds();
    // Measured: 1,165 interrupts arrived, the marks of a thread grew 4
    // times; after 76,208 steps an irq was claimed, after 1,348 one was
    // fired, after 3,990 a task was revoked.
    let counts = [
        reached.interrupts,
        reached.marked,
        reached.claimed,
        reached.irq_fired,
        reached.revoked,
    ];
    at_least(counts, [874, 3, 57156, 1011, 2993]);
}

#[test]
fn mappings_and_devices_are_reached_and_undone_by_removals() {
    let reached = all_seeds();
    // Measured: a mapping held after 119,814 steps, a device attached
    // after 26,616; 360 removals undid a mapping and 43 started or ended a
    // detach (D-139: a device leaves the header at its detach's answer).
    let counts = [
        reached.mapped,
        reached.attached,
        reached.unmapped_by_removal,
        reached.detached_by_removal,
    ];
    at_least(counts, [89861, 19962, 270, 33]);
}

#[test]
fn same_seed_gives_the_same_state() {
    // Seeds 0 and 1 run again and end in the state of their first run;
    // seed 0 twice, reaching the same things.
    let states = seeds().1.lock().unwrap();
    let (mut first, mut second) = (Reached::default(), Reached::default());
    assert_eq!(run(0, &mut first), states[0]);
    assert_eq!(run(0, &mut second), states[0]);
    assert_eq!(first, second);
    assert_eq!(run(1, &mut Reached::default()), states[1]);
}

#[test]
fn different_seeds_give_different_states() {
    let states = seeds().1.lock().unwrap();
    assert_ne!(states[1], states[2]);
}

/// A memory_control dying under a removal of more than one step is counted
/// by its kind: the counter of dying kinds has a place for every kind
/// (`dying` was 19 places for 21 kinds; the final review's verification of
/// lot F1, D1).
#[test]
fn a_dying_memory_control_is_counted() {
    let (mut model, first) = start();
    let endpoints = (4000..4040).map(|slot| Syscall::UntypedRetype {
        untyped: MEMORY,
        kind: ObjectKind::Endpoint,
        size: 0,
        page: 0,
        memory: 0,
        destination: own(slot),
    });
    for call in endpoints {
        assert_eq!(model.apply(first, call), Ok(Output::Done));
    }
    let delete = model.apply(first, Syscall::CapDelete(own(START_SLOT_MEMORY_CONTROL)));
    assert_eq!(delete, Err(Error::Again));
    let mut reached = Reached::default();
    reached.count(&model);
    assert_eq!(reached.dying[ObjectKind::MemoryControl as usize], 1);
}

/// The long run, outside `verify`: `DODOS_PROPERTY_SEEDS` seeds (2,000 if
/// unset) from `DODOS_PROPERTY_FIRST` (the first seed after the 24 of
/// `verify` if unset), as many at once as the machine has cores, every
/// invariant checked after every call; the seeds that fail are named. Run
/// with
/// `cargo test --release -p dodos-model --test property many_seeds --
/// --ignored`.
#[test]
#[ignore]
fn many_seeds() {
    let count: u64 = std::env::var("DODOS_PROPERTY_SEEDS")
        .map(|s| s.parse().expect("DODOS_PROPERTY_SEEDS is a number"))
        .unwrap_or(2000);
    let first: u64 = std::env::var("DODOS_PROPERTY_FIRST")
        .map(|s| s.parse().expect("DODOS_PROPERTY_FIRST is a number"))
        .unwrap_or(SEEDS);
    let width = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let next = std::sync::atomic::AtomicU64::new(first);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..width {
            scope.spawn(|| {
                loop {
                    let seed = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if seed >= first + count {
                        break;
                    }
                    let one = std::panic::catch_unwind(|| {
                        let mut reached = Reached::default();
                        run(seed, &mut reached);
                    });
                    if one.is_err() {
                        failed.lock().unwrap().push(seed);
                    }
                }
            });
        }
    });
    let mut failed = failed.into_inner().unwrap();
    failed.sort();
    assert!(failed.is_empty(), "failing seeds: {failed:?}");
}

/// One step counts each kind dying once, whatever the order of the objects
/// (`Vec::dedup` alone removes neighbours only: the final review's D2 =
/// H7). Two untypeds dying, an endpoint between them in the order of the
/// objects.
#[test]
fn a_kind_dying_twice_in_one_step_is_counted_once() {
    let (mut model, first) = start();
    let mut ok = |call: Syscall| assert!(model.apply(first, call).is_ok());
    let child = |untyped: u32, kind: ObjectKind, slot: u32| Syscall::UntypedRetype {
        untyped: own(untyped),
        kind,
        size: 0,
        page: 0,
        memory: 0,
        destination: own(slot),
    };
    ok(retype(ObjectKind::Untyped, 1 << 16, own(4000)));
    ok(child(4000, ObjectKind::Endpoint, 4001));
    for slot in 4002..4036 {
        ok(derive(own(4001), own(slot)));
    }
    ok(retype(ObjectKind::Untyped, 1 << 16, own(4100)));
    for slot in 4101..4135 {
        ok(child(4100, ObjectKind::Endpoint, slot));
    }
    for untyped in [4000, 4100] {
        let delete = model.apply(first, Syscall::CapDelete(own(untyped)));
        assert_eq!(delete, Err(Error::Again));
    }
    let dying = model.census().dying;
    assert_eq!(
        dying.iter().filter(|&&k| k == ObjectKind::Untyped).count(),
        2
    );
    let mut reached = Reached::default();
    reached.count(&model);
    assert_eq!(reached.dying[ObjectKind::Untyped as usize], 1);
}
