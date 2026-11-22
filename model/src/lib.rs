//! Executable reference model of the DodOS kernel: the kernel of one node
//! of *N* cores ([`Model`]) and the machine of several nodes (`System`).
//!
//! The contract is `architecture/kernel-interface.md` in the documentation
//! repository. A comment that names a section names a section of its "Call
//! reference". The kernel is tested against this model call by call:
//! [`Model::apply`] applies one call made by one thread, and
//! [`Model::check`] checks every invariant of the state.
//!
//! The clock, faults of user code and interrupts are inputs too:
//! [`Model::advance`], [`Model::fault`], [`Model::interrupt`]. Each call,
//! fault or interrupt is an event that ends with the choice of what runs
//! next, which [`Model::running`] gives; `apply` accepts any runnable
//! thread, so a harness applies the calls of `running`.
//!
//! Not modelled: the contents of memory (the start-up page is sized, not
//! written); the hardware below what the contract names (translations are
//! a map, the TLB a set of cached translations per core; masking, the end
//! of interrupt and the entropy pool are not seen); the kernel's own time,
//! taken as zero; real concurrency (every event runs alone, as the node
//! lock makes them). `model/README.md`, "What it does not model", has the
//! whole list.
//!
//! `CONTRACT-GAP` marks a behaviour that the contract does not fix; the
//! contract settled the sixteen of pass 15 (pass 16, D-133), and each such
//! branch names the contract's sentence, "gap *n*". The four of pass 25,
//! 17 to 20, are settled by D-144, and gap 21 of pass 26 by D-145.

#![forbid(unsafe_code)]

mod blocks;
mod boot;
mod caps;
mod check;
mod cores;
mod debug;
mod faults;
#[cfg(test)]
mod gate;
mod grants;
mod ipc;
mod irq;
mod locks;
mod memory;
mod nodes;
mod rings;
mod roles;
#[cfg(test)]
mod scenarios;
mod sched;
mod system;
mod table;
mod tasks;
mod threads;
mod units;

pub use boot::{
    Arch, Core, File, IoApic, IommuUnit, Machine, Refusal, Region, RegionKind, SIXTEEN_KB, Segment,
    THIRTY_TWO_KB,
};
pub use check::Census;
pub use cores::Ipi;
pub use locks::{LockId, Step};
pub use nodes::HwAction;
pub use rings::Message as RingMessage;
pub use system::{Hardware, System};

use dodos_abi::{
    Error, Geometry, IrqSource, MAX_CAPS, MemoryType, ObjectKind, PageSize, Received, Rights,
    SlotAddr, TaskId,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Debug;

/// Identity of a kernel object. It is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjId(u32);

/// Identity of a capability. It is never reused and only grows, so a
/// capability is younger than its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CapId(u32);

/// One capability listed in a message (Transfer of capabilities).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transfer {
    Move(SlotAddr),
    /// With the rights word as the sender gives it: a bit that names no
    /// right is `invalid`.
    Derive(SlotAddr, u32),
}

/// A message as its sender gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub label: u32,
    /// The flag bits of the header. Order of checks: a sender may set only
    /// `Header::FLAG_ADD_MARKS`.
    pub flags: u32,
    pub words: Vec<u64>,
    pub caps: Vec<Transfer>,
    /// The added-marks word of the message page, read only when the flag
    /// `add_marks` is set (Tasks and marks).
    pub marks: u64,
}

/// The receive range: where capabilities that arrive are placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub first: SlotAddr,
    pub count: u32,
}

impl Range {
    pub const NONE: Self = Self {
        first: SlotAddr::NONE,
        count: 0,
    };
}

/// A message as its receiver gets it. The capabilities are in the first
/// `received.header.caps()` slots of its receive range. Bindings: a
/// receiver without a message page gets the register words only, and the
/// header keeps the number of words sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub received: Received,
    pub words: Vec<u64>,
}

/// One system call with its arguments, in the order of the contract's tables.
#[derive(Clone, Debug, PartialEq)]
pub enum Syscall {
    CapDerive {
        source: SlotAddr,
        destination: SlotAddr,
        /// The rights word as the caller gives it: a bit that names no right
        /// is `invalid`.
        rights: u32,
        badge: u64,
    },
    CapMove {
        source: SlotAddr,
        destination: SlotAddr,
    },
    CapDelete(SlotAddr),
    CapRevoke(SlotAddr),
    CapInspect(SlotAddr),
    /// `size` is the size argument; for a frame it is the number of pages,
    /// of the page size `page` and the memory type `memory`, raw words that
    /// no other kind reads.
    UntypedRetype {
        untyped: SlotAddr,
        kind: ObjectKind,
        size: u64,
        page: u64,
        memory: u64,
        destination: SlotAddr,
    },
    CSpaceGrow {
        cspace: SlotAddr,
        untyped: SlotAddr,
    },
    ThreadConfigure {
        thread: SlotAddr,
        cspace: SlotAddr,
        aspace: SlotAddr,
        fault: SlotAddr,
        page: SlotAddr,
    },
    ThreadResume(SlotAddr),
    ThreadReadRegs(SlotAddr),
    /// The thread, and the register block as the message words of the call.
    ThreadWriteRegs {
        thread: SlotAddr,
        block: Vec<u64>,
    },
    ThreadSuspend(SlotAddr),
    /// The thread, and a wait number.
    ThreadCancel(SlotAddr, u64),
    /// Unlike cores: the thread and its new feature mask.
    ThreadSetFeatures {
        thread: SlotAddr,
        mask: u64,
    },
    /// The context, the `node` capability of its node, and the mode.
    SchedCount {
        sched: SlotAddr,
        node: SlotAddr,
        mode: u64,
    },
    /// Debugging: the text travels as the message words.
    DebugWrite {
        debug: SlotAddr,
        length: u64,
        words: Vec<u64>,
    },
    TraceEmit {
        debug: SlotAddr,
        words: [u64; 3],
    },
    /// I/O ports.
    IoportIn {
        ioports: SlotAddr,
        port: u64,
        width: u64,
    },
    IoportOut {
        ioports: SlotAddr,
        port: u64,
        width: u64,
        value: u64,
    },
    IoportsIssue {
        ioports: SlotAddr,
        destination: SlotAddr,
        first: u64,
        last: u64,
    },
    /// Entropy: no capability.
    EntropyRead {
        length: u64,
        mix: [u64; 4],
    },
    /// Nodes (Cores, The calls): the `node` capability and the arguments of
    /// each call. `node_start`: the mode, the kernel image, the module, and
    /// the cores window and mask.
    NodeStart {
        node: SlotAddr,
        mode: u64,
        image: u64,
        module: u64,
        window: u64,
        mask: u64,
    },
    /// The mode, and for mode retire the core number to mark unusable.
    NodeStop {
        node: SlotAddr,
        mode: u64,
        core: u64,
    },
    /// A core number.
    NodeStatus {
        node: SlotAddr,
        core: u64,
    },
    /// A core number; the table travels as the message words.
    NodeIdle {
        node: SlotAddr,
        core: u64,
        words: Vec<u64>,
    },
    NodeWatch {
        node: SlotAddr,
        index: u64,
        notify: SlotAddr,
    },
    NodePerf {
        node: SlotAddr,
        core: u64,
        lowest: u64,
        highest: u64,
        preference: u64,
    },
    RoleClaim(SlotAddr),
    /// The role and the `node` capability of the target; none vacates.
    RoleHand {
        role: SlotAddr,
        node: SlotAddr,
    },
    /// Calls between nodes: the stamped send into another node's mailbox,
    /// with a doorbell (a notify of the caller's node, or none) for the
    /// grants of the frames its message lists.
    NodeSend {
        node: SlotAddr,
        message: Message,
        doorbell: SlotAddr,
    },
    /// The doorbell of a grant, through its remote frame capability.
    GrantSignal(SlotAddr),
    /// `wait`, here and below, is the wait number of the call; 0 is none.
    Call {
        endpoint: SlotAddr,
        message: Message,
        range: Range,
        wait: u64,
    },
    Recv {
        endpoint: SlotAddr,
        reply: SlotAddr,
        range: Range,
        wait: u64,
    },
    ReplyRecv {
        reply: SlotAddr,
        message: Message,
        endpoint: SlotAddr,
        range: Range,
        wait: u64,
    },
    Send {
        target: SlotAddr,
        message: Message,
    },
    Signal(SlotAddr),
    /// The notify, and the wait number.
    Wait(SlotAddr, u64),
    Poll(SlotAddr),
    NotifyBind {
        notify: SlotAddr,
        endpoint: SlotAddr,
    },
    /// `aspace_map`, and `iospace_map` with an `iospace` and a device
    /// address. `rights` is the word as the caller gives it.
    ASpaceMap(Map),
    IoSpaceMap(Map),
    ASpaceUnmap(SlotAddr),
    IoSpaceUnmap(SlotAddr),
    ASpaceProtect {
        aspace: SlotAddr,
        frame: SlotAddr,
        rights: u32,
    },
    /// The iommu, the iospace (0 to detach), and the device identifier.
    IoSpaceAttach {
        iommu: SlotAddr,
        iospace: SlotAddr,
        device: u64,
    },
    /// The context, the `node` capability of its node, the core number to
    /// place it on, and its parameters.
    SchedConfigure {
        sched: SlotAddr,
        node: SlotAddr,
        core: u64,
        budget: u64,
        period: u64,
        class: u64,
    },
    SchedBind {
        sched: SlotAddr,
        thread: SlotAddr,
    },
    SchedUnbind(SlotAddr),
    SchedSetCores {
        sched: SlotAddr,
        node: SlotAddr,
        window: u64,
        mask: u64,
    },
    SchedRead(SlotAddr),
    Yield,
    TimerSet {
        timer: SlotAddr,
        notify: SlotAddr,
        deadline: u64,
        tolerance: u64,
    },
    TimerCancel(SlotAddr),
    /// The irq_control, the source word, the core number, the vector and
    /// the destination.
    IrqClaim {
        control: SlotAddr,
        source: u64,
        core: u64,
        vector: u64,
        destination: SlotAddr,
    },
    IrqBind {
        irq: SlotAddr,
        notify: SlotAddr,
    },
    IrqAck(SlotAddr),
    TaskExempt {
        task: SlotAddr,
        aware: SlotAddr,
    },
    TaskAttach {
        thread: SlotAddr,
        task: SlotAddr,
    },
    TaskMark {
        task: SlotAddr,
        marks: u64,
    },
    TaskRevoke(SlotAddr),
    TaskRead(SlotAddr),
    /// The memory_control, the kind (0 an ordinary run, 1 a device entry,
    /// 2 a table run, D-154), the place (a physical base or an entry's
    /// index), the size (the log2 of the run's bytes for kinds 0 and 2, or
    /// 0 for kind 1), and the destination.
    MemoryTake {
        control: SlotAddr,
        kind: u64,
        place: u64,
        size: u64,
        destination: SlotAddr,
    },
    /// The memory_control, the block, and the node it is given to.
    MemoryGive {
        control: SlotAddr,
        untyped: SlotAddr,
        node: u64,
    },
    MemorySplit {
        control: SlotAddr,
        untyped: SlotAddr,
    },
}

/// The arguments of `aspace_map` and `iospace_map`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Map {
    pub space: SlotAddr,
    pub frame: SlotAddr,
    /// 0, or the untyped the space was created from.
    pub untyped: SlotAddr,
    pub address: u64,
    pub first: u64,
    pub count: u64,
    pub rights: u32,
}

/// The results of a call that did not fail.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// The call has no result.
    Done,
    /// `cap_inspect`.
    Inspect {
        kind: ObjectKind,
        rights: Rights,
        badge: u64,
    },
    /// `cspace_grow`: the new number of slots.
    Slots(u32),
    /// `wait`, `poll`: the word of the notify.
    Word(u64),
    /// `thread_read_regs`: the register block.
    Regs(Vec<u64>),
    /// `sched_read`.
    SchedRead {
        consumed: u64,
        remaining: i64,
        deadline: u64,
        state: u64,
        /// The instructions retired while counted.
        instructions: u64,
    },
    /// `node_status`: the seven words.
    Status([u64; 7]),
    /// `ioport_in`: the value read.
    Value(u64),
    /// `entropy_read`: the bytes, in words.
    Bytes(Vec<u64>),
    /// `role_claim`, `role_hand`: the epoch.
    Epoch(u32),
    /// `timer_set`, `timer_cancel`: 1 if the timer was armed, 0 otherwise.
    WasArmed(u64),
    /// `irq_claim`: the address and the data of an MSI.
    Msi { address: u64, data: u64 },
    /// `task_read`.
    Task {
        id: u64,
        marks: u64,
        flags: u64,
        threads: u64,
    },
    /// `send`, and `node_send` once delivered: 0, or
    /// `Header::FLAG_CAPS_DROPPED`.
    Sent { flags: u32 },
    /// `recv`, `reply_recv`: a message or a notification. `call`: the reply.
    Message(Delivery),
    /// `memory_split`: the index of the new entry.
    Index(u64),
    /// The thread waits. The outcome of its call comes later, in
    /// [`Model::woken`].
    Waits,
}

/// The error code and the results of a call. `Err(Error::Ok)` never occurs.
pub type Outcome = Result<Output, Error>;

/// A place for a capability: a capability space and an index in it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    space: ObjId,
    index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Empty,
    Full(CapId),
    Dead,
}

/// Mapping records: what a frame capability holds besides its object.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Record {
    #[default]
    Empty,
    /// A mapping: the pages first to first + count − 1 of the frame, at
    /// consecutive addresses from `address`, in an `aspace` or an `iospace`.
    Mapping(Mapping),
    /// Bindings: the message page of this thread.
    Binding(ObjId),
    /// A mapping whose entries are cleared and whose invalidation on other
    /// cores of the node, or by an IOMMU unit, is not complete (D-131): its
    /// leaves still count.
    Unmapping(Mapping),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Mapping {
    space: ObjId,
    address: u64,
    first: u64,
    count: u64,
    rights: Rights,
    /// Made in steps and not finished (G3): `count` pages written so far.
    partial: bool,
    /// The page count the call that made it asked for: a partial mapping
    /// goes on only by the same call, page count included (G3; held in the
    /// low 12 bits of word 1 of the slot, which a frame's page-aligned
    /// address leaves 0).
    asked: u64,
}

/// One translation of a space: a page of the frame of the capability whose
/// record made it, of the frame's page size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Translation {
    cap: CapId,
    page: u64,
    size: PageSize,
    rights: Rights,
}

#[derive(Clone, Debug, PartialEq)]
struct Cap {
    object: ObjId,
    rights: Rights,
    badge: u64,
    parent: Option<CapId>,
    /// In the order of derivation, the most recent last.
    children: Vec<CapId>,
    place: Place,
    /// Removal ends: set by the first step of `cap_delete` on the capability,
    /// and on a root capability by the first step of a removal that reaches
    /// it.
    dying: bool,
    /// Empty but on a frame capability.
    record: Record,
}

/// What every object has. The state of an object that has one is in the map
/// of its kind.
#[derive(Clone, Debug, PartialEq)]
struct Object {
    kind: ObjectKind,
    /// The capability the object lives as long as.
    root: CapId,
    /// The untyped its memory came from; `None` for a start-up object.
    from: Option<ObjId>,
    /// Made at start-up (Start-up state): the frames of the start-up
    /// untyped have no `exec`, against the rule of `untyped_retype`.
    boot: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Untyped {
    /// The physical address of its first byte.
    base: u64,
    /// A block made by `memory_take`: of a run of chunks, or of an entry of
    /// the device table.
    block: Option<Block>,
    bytes: u64,
    device: bool,
    /// A **table untyped** (D-154): made by `memory_take` kind 2, cut from
    /// a table floor, or a child of one. It has no clean mark and no
    /// `exec`, and makes only a child table untyped or an `its_table`; the
    /// root table and the lower tables of a space come from it.
    table: bool,
    watermark: u64,
    /// Rules of `untyped_retype`: the memory from the watermark to the
    /// clean mark is known to be zero. Device memory has none: it stays 0.
    clean: u64,
    /// The ranges, as offsets, that the clearing steps cleared and no
    /// object took since: what the clean mark claims to be zero is in them.
    zero: Vec<(u64, u64)>,
    /// Objects, segments of capability spaces and tables of address spaces
    /// taken from it and not yet destroyed.
    live: u32,
    /// Of those, the pages of iospaces destroyed while a device of a failed
    /// unit may still reach them (the root's count of devices above 0):
    /// lost to the node until the machine restarts, they stay live pieces,
    /// so the watermark never returns over them (IOMMU, D-139, D-141).
    lost: u32,
    /// On several cores: the kernel epoch at which its watermark became
    /// ready to return, at the destruction of its last object; it returns
    /// at the first `untyped_retype` once a complete barrier covers that
    /// epoch (word 6 of its root's slot, D-135, D-138).
    ready: Option<u64>,
}

/// What a block covers (`memory_take`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Block {
    Run,
    Entry(usize),
}

/// Frames: fixed by `untyped_retype` and never changed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    page: PageSize,
    pages: u64,
    memory: MemoryType,
    /// The physical address of its first page.
    base: u64,
    /// A kept frame: made at start-up over kept memory, from no untyped.
    kept: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CSpace {
    max: u32,
    slots: Vec<Slot>,
    /// The derivation tree: `Some` once the space is dying.
    sweep: Option<Sweep>,
    /// Bindings: the threads bound to the space, the most recent last.
    threads: Vec<ObjId>,
}

/// The derivation tree: what a dying capability space keeps.
#[derive(Clone, Debug, PartialEq)]
struct Sweep {
    /// The index where the sweep stands: the slots before it were examined
    /// and, within the pass, are not examined again.
    next: usize,
    /// The first slot pass 1 deferred, the contract's 0 as `None`: it holds
    /// the root capability of a table untyped that spaces still name, and
    /// the higher slots are removed before it (D-156).
    first_deferred: Option<usize>,
    /// Whether the sweep is in its second pass, which defers nothing.
    pass2: bool,
    /// The capabilities the space held when it began to die. Removal ends:
    /// a dying space only shrinks, so invariant 23 refuses any other.
    held: Vec<CapId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ASpace {
    /// A thread and its address space: the threads bound to it, the most
    /// recent last.
    threads: Vec<ObjId>,
    /// Mapping records: every translation in the space, by the address of
    /// its page.
    translations: BTreeMap<u64, Translation>,
    /// The capabilities whose records name the space, the most recently
    /// mapped last.
    records: Vec<CapId>,
    /// Address spaces and their tables: the lower tables taken, hooked or on
    /// the free list. The hooked ones follow from the translations.
    tables: u64,
    /// The gate over page tables (G2), for a space made from an untyped (the
    /// start-up aspace, made from kept memory, has none here): the physical
    /// page of its root table, of class *page table of level L* (an
    /// iospace's, *iospace table*); each hooked lower table, by its level
    /// and the address bits above what it covers, of the class of its level;
    /// and the free list, most recently freed last, whose pages keep the
    /// class they had.
    root: Option<u64>,
    /// The table untyped named when the space was created, which its root
    /// table and its lower tables are live pieces of (D-154); `None` for
    /// the start-up aspace, whose tables lie in the table floor.
    table_untyped: Option<ObjId>,
    hooked: BTreeMap<(u32, u64), u64>,
    free: Vec<u64>,
    /// The tables unhooked by the clearing of a record, by its capability
    /// while its invalidation is open, which join the free list only at the
    /// completion of that invalidation (Address spaces and their tables,
    /// D-137); or by a frozen change once a failed unit left that
    /// invalidation undone: they never join it (IOMMU), each frozen change
    /// keeps its own bound of at most 2 × (L − 1) tables, and the record's
    /// next change unhooks its own (pass 38, seed 89 of the long run).
    unhooked: Vec<(UnhookedBy, u64)>,
    /// Number of changes whose unhooked tables a failed unit left frozen.
    frozen_changes: u64,
    /// A node of several cores: the set of the node's cores that loaded its
    /// tag since it was last invalidated on them, by position (TLB).
    cores: u64,
    /// Destruction: the first step's invalidation of the whole tag is
    /// complete on every core.
    flushed: bool,
    /// An iospace: the IOMMU units whose devices it served.
    units: std::collections::BTreeSet<usize>,
}

/// The change that unhooked a table from an address space or iospace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnhookedBy {
    /// An open unmapping of this capability's record.
    Cap(CapId),
    /// A change whose invalidation a failed unit left undone, numbered in
    /// the order the space froze its changes.
    Frozen(u64),
}

impl ASpace {
    /// The physical pages of its table pages: root, hooked and free.
    fn table_pages(&self) -> impl Iterator<Item = u64> + '_ {
        self.root
            .iter()
            .copied()
            .chain(self.hooked.values().copied())
            .chain(self.free.iter().copied())
            .chain(self.unhooked.iter().map(|&(_, page)| page))
    }
}

/// IOMMU: one unit, made by the kernel at start.
#[derive(Clone, Debug, Default, PartialEq)]
struct Iommu {
    /// Its index among the units of the machine.
    unit: usize,
    /// The devices attached through it, and the iospace of each.
    attached: BTreeMap<u16, ObjId>,
    /// The same devices, most recently attached last: the list that the
    /// device entries of the iospaces' headers link.
    order: Vec<u16>,
    /// The devices whose entry in their iospace's header is marked
    /// *detaching*: a detach of it is in flight (IOMMU, the attach word
    /// against every event, D-139).
    detaching: BTreeSet<u16>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Thread {
    /// Bindings, each a link both ways: the capability space, the address
    /// space, the fault endpoint with the badge of the capability given to
    /// `thread_configure`, and the frame capability whose record binds the
    /// message page.
    cspace: Option<ObjId>,
    aspace: Option<ObjId>,
    fault: Option<(ObjId, u64)>,
    page: Option<CapId>,
    suspended: bool,
    wait: Wait,
    /// Threads: the wait number of the call the thread waits in; 0 if it
    /// gave none, or does not wait.
    number: u64,
    /// Threads: the word "cancelled up to", 0 when the thread is created.
    cancelled_up_to: u64,
    /// Outcome of the call the thread waited in, until its next call.
    outcome: Option<Outcome>,
    /// Register calls: the register block, as the thread will see it when
    /// it resumes.
    regs: Vec<u64>,
    /// Faults: the thread waits on a fault, as a caller of its fault
    /// endpoint; the words of the fault are kept with its message.
    faulting: bool,
    /// The kind and word 0 of the fault it waits on as a caller.
    raised: Option<(u32, u64)>,
    /// Faults: a fault that was not delivered, recorded in the thread: its
    /// kind and its five words, until the thread is resumed.
    recorded: Option<(u32, [u64; 5])>,
    /// x86-64: a fault of kind 9 from a #DB at CPL0, kept until the thread
    /// would next return to user mode, in one word: bits 0-31 the DR6 value
    /// read, bits 32-35 the enabled breakpoints that hit, judged against
    /// the control word at the #DB (D-146, D-147).
    debug_kept: Option<u64>,
    /// Scheduling contexts: its own context, if it is that context's home.
    home: Option<ObjId>,
    /// The budget follows the call: marked as having lost its context.
    lost: bool,
    /// Tasks and marks: the task it is attached to, and its own marks word.
    task: Option<ObjId>,
    marks: u64,
    /// Unlike cores: the feature mask, and the bytes of the thread as it
    /// was made, which fix its save area.
    mask: u64,
    area: u64,
    /// A node of several cores: the thread is marked *leaving its core*;
    /// the core that runs it saves its registers and clears the mark.
    leaving: bool,
    /// The outcome is the end of a bounded wait (for another node, a unit,
    /// its node's cores, a hand-over): any outcome its call has.
    answered: bool,
}

impl Thread {
    /// A bounded wait that holds no queue of an object: for another node,
    /// for a unit, for its node's cores, or for a hand-over's drain.
    pub(crate) fn bounded(&self) -> bool {
        matches!(
            self.wait,
            Wait::Node { .. } | Wait::Unit(_) | Wait::Cores(_) | Wait::Hand(_)
        )
    }
}

/// IPC: the states of a thread. `None` is ready.
#[derive(Clone, Debug, Default, PartialEq)]
enum Wait {
    #[default]
    None,
    /// Waiting to be received, in the queue of an endpoint.
    Call {
        endpoint: ObjId,
        parcel: Parcel,
        window: Window,
    },
    /// Waiting for a reply, through the reply object armed with the thread.
    Reply(ObjId),
    /// Waiting to receive, in the queue of an endpoint.
    Recv {
        endpoint: ObjId,
        reply: ObjId,
        window: Window,
    },
    /// Waiting on a notify, in its queue.
    Notify(ObjId),
    /// Waiting for another node (IPC): the answer of a `node_send` or of a
    /// revocation step, by the peer and the credit of the request.
    Node { peer: u16, credit: u8 },
    /// Waiting for a unit: the confirmation of the entry of the unit
    /// request table.
    Unit(usize),
    /// Waiting for its node's cores: the completion of the invalidation
    /// request of the core at this position.
    Cores(usize),
    /// Waiting for a unit: the drain of the queue of the unit of this role,
    /// for a planned hand-over (`role_hand`).
    Hand(usize),
}

/// A message on its way. Transfer of capabilities: the kernel remembers the
/// slots that the sender listed, not what they held.
#[derive(Clone, Debug, PartialEq)]
struct Parcel {
    label: u32,
    words: Vec<u64>,
    badge: u64,
    sender: ObjId,
    caps: Vec<Transfer>,
    /// Tasks and marks: the added-marks word, read when the call was made;
    /// 0 without the flag `add_marks`.
    marks: u64,
}

/// A receive range, resolved when the call was made.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    cspace: ObjId,
    first: u32,
    count: u32,
}

/// An endpoint queues callers or receivers, never both.
#[derive(Clone, Debug, Default, PartialEq)]
struct Endpoint {
    queue: VecDeque<ObjId>,
    notify: Option<ObjId>,
    /// Bindings: the threads whose fault endpoint it is, the most recent
    /// last.
    threads: Vec<ObjId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReplyState {
    Idle,
    /// To a thread that waits to receive.
    Attached(ObjId),
    /// With a caller, its receive range, and the flags its reply will carry.
    Armed {
        caller: ObjId,
        window: Window,
        flags: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Notify {
    word: u64,
    waiters: VecDeque<ObjId>,
    endpoint: Option<ObjId>,
    /// Timers: the armed timers that signal it, the most recently armed
    /// last.
    timers: Vec<ObjId>,
    /// Interrupts: the irqs bound to it, the most recently bound last.
    irqs: Vec<ObjId>,
    /// Grants: the grant entries whose doorbell it is, the most recently
    /// made last.
    doorbells: Vec<u16>,
}

/// Interrupts: one entry of the interrupt table of a physical core of the
/// node.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Irq {
    /// The position of its core in the node.
    core: usize,
    vector: u8,
    source: IrqSource,
    state: IrqState,
    /// An arrival of an edge source that was not signalled.
    pending: bool,
    /// The notify it is bound to, and the bits it sets.
    notify: Option<(ObjId, u64)>,
    /// Its removal asked the holder of its line's controller to mask the
    /// line (Interrupts, the irq, D-137).
    removing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IrqState {
    /// Bound to no notify.
    Idle,
    /// Bound, and the next arrival is signalled.
    Armed,
    /// Bound and signalled, and no further signal until `irq_ack`.
    Fired,
}

/// Tasks and marks: a task.
#[derive(Clone, Debug, PartialEq)]
struct Task {
    id: TaskId,
    marks: u64,
    aware: bool,
    revoked: bool,
    /// The threads attached to it, the most recently attached last.
    threads: Vec<ObjId>,
}

/// Scheduling contexts: the parameters and the state of a `sched`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Sched {
    configured: bool,
    /// The budget and the period in force.
    budget: u64,
    period: u64,
    /// Those `sched_configure` gave, used from the next replenishment.
    pending: Option<(u64, u64)>,
    class: u64,
    /// The cores where the context may be placed: a window and a mask.
    cores: (u64, u64),
    /// A node of several cores: the position of the core of its node where
    /// it is placed, admitted and queued.
    core: usize,
    /// Moved by `sched_configure` while its holder ran on its old core: not
    /// eligible, whatever its holder, until that core has left it (the
    /// leave interrupt clears it; D-141).
    moving: bool,
    /// Signed nanoseconds left in the current period.
    remaining: i64,
    deadline: u64,
    /// While throttled: the replenishment instant, and the budgets it
    /// brings (bounded by `SCHED_MAX_DEBT`, D-159).
    throttled: Option<(u64, dodos_abi::SchedDebt)>,
    consumed: u64,
    home: Option<ObjId>,
    /// The budget follows the call: the reply objects that carry its lends,
    /// in the order they were made; the last is the top.
    chain: Vec<ObjId>,
    /// What runs next: its place in a queue, by key and order of entry.
    queue: Option<Queued>,
    /// Destroying a context: its first step is done.
    retired: bool,
    /// Instructions retired, on demand: counting on, the count, and the
    /// hardware class read when it last stopped running (1 + the class, or
    /// 0).
    counted: bool,
    count: u64,
    hw_class: u64,
}

/// What runs next: the queues of the scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Queue {
    Ready,
    /// One per class.
    Slack(u64),
    Sleeping,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Queued {
    queue: Queue,
    key: u64,
    entered: u64,
}

/// Timers: an armed timer, with its firing instant, its notify and the bits
/// it sets, and its order of arming.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Armed {
    instant: u64,
    notify: ObjId,
    bits: u64,
    entered: u64,
    /// The position of the core whose timer queue holds it: the core that
    /// ran `timer_set`.
    core: usize,
}

/// Bookkeeping of the current or last event: what the invariants read to
/// judge an event. Two states that differ only in it are the same state,
/// so it is equal to any other.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ephemeral<T>(pub(crate) T);

impl<T> PartialEq for Ephemeral<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> std::ops::Deref for Ephemeral<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Ephemeral<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// What an event did (Order inside an event, Faults, Exhaustion and
/// replenishment), kept until the next event so that the invariants can
/// judge it.
#[derive(Clone, Debug, Default, PartialEq)]
struct EventLog {
    /// The order of entry of the first context of the event.
    entered_from: u64,
    /// The change list, in its order, over the rounds of the choice.
    changes: Vec<ObjId>,
    /// The parameters in force of each context at the start of the event,
    /// and the contexts whose parameters were set by a replenishment, a
    /// reset of the wake-up rule or a configuration.
    params: BTreeMap<ObjId, (u64, u64)>,
    resets: Vec<ObjId>,
    /// The faults raised, by thread and kind, and for each fault *no
    /// context* that reached a receiver, whether it had an own context.
    raised: Vec<(ObjId, u32)>,
    no_context_delivered: Vec<bool>,
    /// The consumed time and the instruction count of each context at the
    /// start of the event.
    counts: BTreeMap<ObjId, (u64, u64, bool)>,
    /// Debugging: kernel entries written in the event; the entries of a
    /// `debug_write` or `trace_emit` (first sequence, count, stamp); the
    /// ports a port call touched.
    kernel_entries: u32,
    user_entries: Option<(u64, u64, (u64, u64))>,
    ports: Option<(u64, u64)>,
    /// RISC-V: the APLIC sources whose `setipnum`, `setienum` or `clrienum`
    /// the event wrote (G9, D-146).
    aplic: Vec<u32>,
    /// Whether `Model::due` ran in this event, and whether due events remained
    /// after the `TIME_EVENT_STEP` (16) cap (Due events and the hardware timer).
    ran_due: bool,
    due_remaining: bool,
    /// The state of this node's member word at each accusation the event
    /// wrote (D-146: only a live or sealing node writes fencing).
    accusers: Vec<dodos_abi::MemberState>,
    /// A step of the recovery reserve in the event left a held message it
    /// could have sent (judged at the step: the granter's word and room).
    held_after_step: bool,
    /// The member words read by `observe_needed` in this event, if it ran
    /// (D-147: a closed observation frees its slot at each step that takes
    /// observations).
    observed_members: Option<Vec<dodos_abi::Member>>,
    /// RISC-V: an arrival of a level line left its APLIC source enabled.
    arrival_enabled: bool,
}

/// The whole state of the kernel of one node: its shared state, and the
/// part of each of its physical cores (A node of several cores).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    next_id: u32,
    caps: BTreeMap<CapId, Cap>,
    objects: BTreeMap<ObjId, Object>,
    untypeds: BTreeMap<ObjId, Untyped>,
    frames: BTreeMap<ObjId, Frame>,
    cspaces: BTreeMap<ObjId, CSpace>,
    aspaces: BTreeMap<ObjId, ASpace>,
    threads: BTreeMap<ObjId, Thread>,
    endpoints: BTreeMap<ObjId, Endpoint>,
    replies: BTreeMap<ObjId, ReplyState>,
    notifies: BTreeMap<ObjId, Notify>,
    /// The derivation tree: the longest walk so far to the next capability
    /// to remove, in capabilities. The contract says: shorter than 192.
    longest_walk: usize,
    /// The `node` objects, with their node slot.
    nodes: BTreeMap<ObjId, u16>,
    /// The `role` objects, with the index of their role.
    roles: BTreeMap<ObjId, usize>,
    /// The slot of this node, and the machine table it shares with the
    /// other nodes.
    node: u16,
    table: table::Table,
    /// The write gate: the chunks the node maps in its kernel space.
    kernel_runs: std::collections::BTreeSet<u64>,
    /// Of the chunks the kernel's own space maps, its table floor included:
    /// those the gate mapped read-only, as one large leaf never rewritten
    /// between take and give (kept kernel tables, D-154).
    kernel_read_only: std::collections::BTreeSet<u64>,
    /// The `its_table` objects: the first page and the number of pages of
    /// each, of class *ITS table* (D-152 complement, D-154).
    its_tables: BTreeMap<ObjId, (u64, u64)>,
    /// IOMMU: the devices each `iospace` serves, the most recently attached
    /// last, each by its `iommu` and its identifier: the node's own list of
    /// its devices on each unit (D-131).
    iospaces: BTreeMap<ObjId, Vec<(ObjId, u16)>>,
    /// The domain number of each iospace that serves devices, on its unit,
    /// taken from this node slot's range at its first device and freed with
    /// its last (Devices and shared hardware, D-134).
    domains: BTreeMap<ObjId, u16>,
    /// The unit an iospace keeps for a device that left its header with its
    /// attach word *attached* (an answer *failed*, or a detach a holder
    /// refused at a step of a destruction): its unmaps are invalidated
    /// through that unit as for a device in the header, and its domain
    /// number stays taken (the attach word's table, D-141).
    kept: BTreeMap<ObjId, ObjId>,
    /// The domain numbers of destroyed iospaces whose root still counts a
    /// device: lost to the node until the machine restarts, by unit.
    lost_domains: BTreeSet<(usize, u16)>,
    iommus: BTreeMap<ObjId, Iommu>,
    scheds: BTreeMap<ObjId, Sched>,
    /// Timers: `None` while idle.
    timers: BTreeMap<ObjId, Option<Armed>>,
    /// The budget follows the call: the lends, by the reply object that
    /// records each, with its context and its receiver.
    lends: BTreeMap<ObjId, (ObjId, ObjId)>,
    /// Clock: the instant of the clock, which only `Model::advance` moves.
    now: u64,
    /// The part of each physical core of the node, by its position.
    cores: Vec<cores::CorePart>,
    /// The position of the core the current event runs on.
    at: Ephemeral<usize>,
    /// What runs next: the order of entry into the queues.
    entered: u64,
    /// Order inside an event: what each context was at the start of the
    /// event (eligible, remaining, throttled, queue and key), the change
    /// list, and the no-context list.
    snapshot: Ephemeral<BTreeMap<ObjId, sched::Snap>>,
    change_list: Ephemeral<Vec<ObjId>>,
    no_context_list: Ephemeral<Vec<ObjId>>,
    /// Bounds: the contexts the last event put in a new queue position,
    /// and what that event was.
    last_event: Ephemeral<Option<(sched::EventKind, usize)>>,
    /// What the last event did, for the invariants that judge an event.
    event: Ephemeral<EventLog>,
    /// The locks the last event took and the waits it entered, for the
    /// checks of the node lock (A node of several cores).
    locks: Ephemeral<locks::Trace>,
    /// The kernel epoch: one word of the node block, raised at the last
    /// destruction of an untyped's object (D-135, D-138).
    pub(crate) kernel_epoch: u64,
    /// A test's input: how long an event of this node waits for the node
    /// lock before it holds it. The model runs every event alone, so the
    /// wait is a time and not an interleaving; what reads the clock under
    /// the lock reads it this much later than the event's entry.
    pub(crate) lock_wait: u64,
    /// A test's input: the acknowledgements (node, generation) this node's
    /// faulty kernel wrote before its R4 for that node ended
    /// (`System::early_ack`). ACKED does not judge R4's order for them and
    /// judges every other rule; once the audit ends there is nothing left
    /// to pass over.
    pub(crate) early_acks: BTreeSet<(u16, u32)>,
    /// The barrier epoch: the largest kernel epoch a complete barrier
    /// covers; a watermark ready at an epoch it covers returns (A node of
    /// several cores, D-138).
    pub(crate) barrier_epoch: u64,
    /// The pending interrupts of the cores are delivered by the caller
    /// (`System`), which interleaves them, rather than at the end of each
    /// event.
    manual: bool,
    /// Grants: the remote objects this node holds, its kernel space (the
    /// anchors of its grants), its mailbox.
    remotes: BTreeMap<ObjId, grants::Remote>,
    kernel_space: Option<ObjId>,
    mailbox: Option<ObjId>,
    /// The thread whose call is the removal in progress, which waits when an
    /// anchor's grant needs its holder (Grants).
    remover: Option<ObjId>,
    /// Remote capabilities found invalid during the call, whose slots
    /// become dead when it ends.
    stale: Ephemeral<std::cell::RefCell<Vec<CapId>>>,
    /// What crosses between nodes: this node's credits toward each node
    /// slot, the messages the last event wrote into its rings (the rings
    /// themselves are in the machine table), its unit request table, its
    /// unit service table, and the cursor of each rebuild it runs.
    peers: Vec<rings::Peer>,
    unit_requests: Vec<Option<units::UnitEntry>>,
    unit_service: BTreeMap<(u16, u8), units::Served>,
    rebuilds: BTreeMap<usize, units::Rebuild>,
    /// A test's input: faulty behaviour on `ReportAsk` (D-169).
    pub(crate) faulty_report: units::FaultyReport,
    /// A test's input: `(from, from_generation, credit_index, credit_serial)`
    /// of `ReportAsk` messages this node's faulty kernel consumed without
    /// answering (`FaultyReport::Silent`). Only those credits are exempted
    /// from CREDIT in `System::check`, so every other credit and peer stays
    /// judged.
    pub(crate) swallowed_asks: BTreeSet<(u16, u32, u8, u32)>,
    /// Rebuild reports ended early by D-141/D-169.
    pub(crate) rebuild_cuts: Ephemeral<Vec<units::RebuildCutRecord>>,
    /// Cores: the fences this node drives, its reclaim of each node, what
    /// it asks of the machine (stops, starts, interrupts to other nodes),
    /// its watch slots, the member and role words it last processed, the
    /// instant of its next sweep.
    fences: BTreeMap<u16, nodes::Fence>,
    reclaims: BTreeMap<u16, nodes::Reclaim>,
    hw_out: Vec<nodes::HwAction>,
    watchers: [Option<(ObjId, u64)>; 4],
    seen: (Vec<dodos_abi::Member>, Vec<dodos_abi::RoleWord>),
    next_sweep: u64,
    /// Cursor of the sweep over sealed nodes: index of the last sealed node
    /// read by the previous sweep, starting at this node's own index (D-168).
    pub(crate) seal_cursor: u16,
    /// The seal of this node, its step; whether the node consumes and
    /// runs threads (false while it resumes); the starts it drives, and the
    /// image and module of the last `node_start`; the planned hand-overs it
    /// runs; the order of its unit requests; whether the current removal
    /// step is done for a `Revoke`; the instant its recovery reserve last
    /// made no progress; the generation it was built for.
    seal: Option<nodes::SealStep>,
    ready: bool,
    resuming: bool,
    /// The membership-clock instant of the node's first core entry in the
    /// current fresh start (D-171).
    pub(crate) start_entered: Option<u64>,
    /// The member words as this node last saw them when it wrote sealed:
    /// at its resume, a generation that ended since gets R0, R2 and R3.
    sealed_view: Vec<dodos_abi::Member>,
    /// The generations that ended while this node was sealed whose R0 its
    /// resume has done, once each.
    ended_done: BTreeSet<(u16, u32)>,
    /// The producer index of each ring into this node (kernel, user) when
    /// its resume finished its reclaims: what waited in its rings, which it
    /// consumes before it writes live.
    resume_marks: Option<Vec<(u64, u64)>>,
    /// The runs of R0 for each generation of each node, which the check
    /// holds to at most one (`node_start` mode resume: "R0 once for that
    /// generation"; finding 8 of the tenth model review).
    r0_runs: BTreeMap<(u16, u32), u32>,
    /// The first table floor untyped and the pieces the start-up aspace
    /// took from it for good (D-157 (2)), which the check holds it to.
    startup_lost: Option<(ObjId, u32)>,
    starts: BTreeMap<usize, nodes::Starting>,
    handing: BTreeMap<usize, roles::Handover>,
    unit_order: u64,
    revoking: bool,
    stalled: Option<u64>,
    /// The messages refused by the one check, or unreadable, since start:
    /// each is also an entry of code 11 in the trace, which may be rate
    /// limited.
    refused: Ephemeral<u64>,
    /// What the explorations count: the `Handed`s this node applied.
    handed_applied: Ephemeral<u64>,
    /// One more at each translation added to a space or run added to the
    /// kernel's own space: a core caches again only what changed since its
    /// last event (the model's bookkeeping, not the kernel's).
    added: Ephemeral<u64>,
    /// What the current event did that the checks of crossing judge: each
    /// submission to a unit with the role word it read and its operation,
    /// and each message applied with its header.
    submitted: Ephemeral<Vec<(usize, dodos_abi::RoleWord, crate::rings::UnitOp)>>,
    applied: Ephemeral<Vec<(dodos_abi::RingHeader, dodos_abi::Member)>>,
    steps_at: Ephemeral<u64>,
    steps_now: Ephemeral<u64>,
    #[cfg(test)]
    pub(crate) bound_hits: Ephemeral<crate::scenarios::bounds::BoundHits>,
    built: u32,
    /// IOMMU: counts of leaves whose invalidation a failed unit never
    /// confirmed, which are never lowered (IOMMU): by chunk, and by entry
    /// of the device table, and by use entry for a frame another node
    /// granted (D-142).
    frozen: BTreeMap<u64, u64>,
    frozen_entries: BTreeMap<usize, u64>,
    frozen_uses: BTreeMap<u16, u64>,
    /// The start-up space of the node.
    start_space: Option<ObjId>,
    /// Other models of the machine share its table (`System`): the map
    /// counts hold their leaves too.
    shared: bool,
    /// Interrupts: the claimed entries of the interrupt tables of the
    /// node's cores.
    irqs: BTreeMap<ObjId, Irq>,
    tasks: BTreeMap<ObjId, Task>,
    /// Tasks and marks: the counter of the identifiers of the node.
    task_counter: u64,
    /// What the start-up state was built from.
    machine: Machine,
}

/// The entry `id` of one of the maps of the state, to change it. Every
/// identity the state holds names a live entry (`Model::check`).
fn live<K: Ord + Debug, T>(map: &mut BTreeMap<K, T>, id: K) -> &mut T {
    map.get_mut(&id).expect("names a live entry")
}

impl Model {
    /// Values of the machine: the values that fix the shape of memory.
    pub fn geometry(&self) -> Geometry {
        self.machine.geometry
    }

    /// Values of the machine: the bytes of a page size the processor
    /// supports. Every frame has one, so the model never asks for another.
    pub(crate) fn bytes(&self, size: PageSize) -> u64 {
        self.machine
            .geometry
            .bytes(size)
            .expect("a supported page size")
    }

    /// *P*, the bytes of a page.
    pub(crate) fn page(&self) -> u64 {
        self.machine.geometry.page()
    }

    /// The threads that can run: not suspended, not waiting, and not marked
    /// *leaving its core*. A call can be applied for any of them.
    /// "A core takes its pending interrupts at each entry into its kernel
    /// before it carries out a call, so a marked thread that enters the
    /// kernel leaves its core there" (A node of several cores, gap 4).
    pub fn runnable(&self) -> Vec<ObjId> {
        let ready = |thread: &&Thread| Self::runnable_thread(thread);
        let ids = self.threads.iter().filter(|(_, thread)| ready(thread));
        ids.map(|(&id, _)| id).collect()
    }

    /// The object in a slot of the start-up space (the space of the
    /// node's first thread), to find the first thread after a start.
    pub fn object_at_start(&self, index: u32) -> ObjId {
        let space = self.start_space.expect("a start-up space");
        match self.cspaces[&space].slots[index as usize] {
            Slot::Full(cap) => self.caps[&cap].object,
            _ => panic!("slot {index} of the start-up space is empty"),
        }
    }

    /// Unlike cores: the state features the node's first core enables, as
    /// the last switch wrote them.
    pub fn enabled(&self) -> u32 {
        self.cores[0].enabled
    }

    /// The outcome of the call in which `thread` waited, once it is over and
    /// until the thread makes its next call.
    pub fn woken(&self, thread: ObjId) -> Option<&Outcome> {
        self.threads.get(&thread)?.outcome.as_ref()
    }

    /// The derivation tree: the longest walk so far to the next capability
    /// to remove, in capabilities; at most `dodos_abi::LONGEST_WALK`. The
    /// kernel can be measured against it.
    pub fn longest_walk(&self) -> usize {
        self.longest_walk
    }

    /// The object that a slot address names for `thread`, if any: a way to
    /// find the identity of a thread that a call created.
    pub fn object_at(&self, thread: ObjId, slot: SlotAddr) -> Option<ObjId> {
        self.threads.get(&thread)?;
        let cap = self.cap(thread, slot, Rights::READ).ok()?;
        Some(self.caps[&cap].object)
    }

    /// The machine table, as user space reads it through the kept frame of
    /// slot 13: the member word of a node.
    pub fn member(&self, node: u16) -> dodos_abi::Member {
        self.table.members[node as usize]
    }

    /// The role word of role `index`, in the order of the entry `Roles`.
    pub fn role(&self, index: usize) -> dodos_abi::RoleWord {
        self.table.roles[index]
    }

    /// A stray write into the living part, which no gate made: the input a
    /// test uses to show that a word that fails its check is `busy`.
    pub fn stray_write(&mut self, chunk: u64, raw: u64) {
        std::sync::Arc::make_mut(&mut self.table.owners)[chunk as usize] = raw;
    }

    /// The raw ownership word of a chunk.
    pub fn ownership(&self, chunk: u64) -> u64 {
        self.table.owners[chunk as usize]
    }

    /// The number of chunks the ownership table names.
    pub fn chunk_count(&self) -> u64 {
        self.table.owners.len() as u64
    }

    /// The watermark and the clean mark of an untyped, as offsets from its
    /// first byte, and whether it is device memory (rules of
    /// `untyped_retype`).
    pub fn untyped_marks(&self, thread: ObjId, slot: SlotAddr) -> Option<(u64, u64, bool)> {
        let object = self.object_at(thread, slot)?;
        let untyped = self.untypeds.get(&object)?;
        Some((untyped.watermark, untyped.clean, untyped.device))
    }

    /// Whether the untyped in `slot` is a table untyped (D-154: bit 51 of
    /// word 0 of its slot, bit 1 of its `StartUntyped` flags).
    pub fn untyped_is_table(&self, thread: ObjId, slot: SlotAddr) -> Option<bool> {
        let object = self.object_at(thread, slot)?;
        Some(self.untypeds.get(&object)?.table)
    }

    /// Whether a chunk is a table chunk: bit 58 of its gate word (D-154).
    pub fn table_chunk(&self, chunk: u64) -> bool {
        self.table.table_chunk(chunk)
    }

    /// Whether the kernel's own space maps a chunk read-only (D-154).
    pub fn kernel_maps_read_only(&self, chunk: u64) -> bool {
        self.kernel_read_only.contains(&chunk)
    }

    /// The table floor of a node slot, as ranges of chunks (D-154).
    pub fn table_floor(&self, node: u16) -> Vec<(u64, u64)> {
        self.table.table_floors[node as usize].clone()
    }

    /// The physical address of the root table of the space in `slot`.
    pub fn root_table(&self, thread: ObjId, slot: SlotAddr) -> Option<u64> {
        self.aspaces.get(&self.object_at(thread, slot)?)?.root
    }

    /// The map count of a chunk (G4): bits 0-31 of its gate word.
    pub fn map_count(&self, chunk: u64) -> u64 {
        self.table.map_count(chunk)
    }

    /// The gate word of a chunk (G4), raw: the map count, and the number of
    /// its pages not of class *data* in bits 32-44.
    pub fn gate_word(&self, chunk: u64) -> u64 {
        self.table.gate_word(chunk).raw()
    }

    /// Whether a thread's breakpoint control word enables a breakpoint, so
    /// that a hit, a fault of kind 9, can be given to `Model::fault`.
    pub fn breakpoint_enabled(&self, thread: ObjId) -> bool {
        self.threads
            .get(&thread)
            .is_some_and(|t| t.regs[dodos_abi::x86_64::REG_DR7] & 0xFF != 0)
    }

    /// The leaf count of an entry of the device table (G4).
    pub fn device_leaves(&self, index: usize) -> u64 {
        self.table.device_leaves[index]
    }

    /// The class of a page of ordinary memory, by its physical address, as
    /// the page classes hold it (G2): `PageClass::DATA` if not stored.
    pub fn page_class(&self, at: u64) -> dodos_abi::PageClass {
        let page = at / self.page();
        let class = self.table.classes.get(&page).copied();
        class.unwrap_or(dodos_abi::PageClass::DATA)
    }

    /// Every used entry of this node's grant table, by index, as
    /// `grant_entry` gives it.
    pub fn grant_entries(&self) -> Vec<(u16, (u16, u32, u32, u8))> {
        let used = self.table.grants[self.node as usize].keys();
        used.filter_map(|&g| Some((g, self.grant_entry(g)?)))
            .collect()
    }

    /// An entry of this node's grant table: the grantee, its generation,
    /// the serial, and the state (0 live, 1 revoking, 2 pinned).
    pub fn grant_entry(&self, grant: u16) -> Option<(u16, u32, u32, u8)> {
        let entry = self.table.grants[self.node as usize].get(&grant)?;
        let state = match entry.state {
            table::GrantState::Live => 0,
            table::GrantState::Revoking => 1,
            table::GrantState::Pinned => 2,
        };
        Some((entry.grantee, entry.generation, entry.serial, state))
    }

    /// Whether an address lies in a chunk of ordinary memory (a floor's
    /// included) or in memory the kernel keeps.
    pub fn chunked_or_kept(&self, at: u64) -> bool {
        let shift = self.table.geometry.chunk_shift();
        let inside = |ranges: &[(u64, u64)]| ranges.iter().any(|&(s, e)| s <= at && at < e);
        // An address past the ownership words is in no chunk (ver-B O5).
        let none = dodos_abi::Ownership::none().raw();
        self.table
            .owners
            .get((at >> shift) as usize)
            .is_some_and(|&w| w != none)
            || self.table.floors.iter().any(|f| inside(f))
            || inside(&self.table.kept)
    }

    /// The fence class of each physical core, by core number.
    pub fn core_list_fences(&self) -> Vec<u8> {
        self.table.fence.clone()
    }

    /// The capacity of the device table.
    pub fn device_count(&self) -> usize {
        self.table.devices.len()
    }

    /// An entry of the device table.
    pub fn device_entry(&self, index: usize) -> dodos_abi::DeviceEntry {
        self.table.devices[index]
    }

    /// The page size, the number of pages and the physical base of the frame
    /// that a slot address names for `thread`, as the slot records them.
    pub fn frame_at(&self, thread: ObjId, slot: SlotAddr) -> Option<(PageSize, u64, u64)> {
        let frame = self.frames.get(&self.object_at(thread, slot)?)?;
        Some((frame.page, frame.pages, frame.base))
    }

    /// The rights of the translation that covers `address` in the address
    /// space of `thread`, if any.
    pub fn translated(&self, thread: ObjId, address: u64) -> Option<Rights> {
        let aspace = &self.aspaces[&self.threads.get(&thread)?.aspace?];
        let (&at, translation) = aspace.translations.range(..=address).next_back()?;
        (address - at < self.bytes(translation.size)).then_some(translation.rights)
    }

    /// Applies one call made by `thread`, which must be able to run.
    ///
    /// The call is one event of the kernel (What runs next): the time since
    /// the previous event is charged first, and the choice of what runs
    /// next is made last.
    ///
    /// The call is an event of the core the thread's events run on: the
    /// core that runs it, else the core of its context.
    pub fn apply(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        assert!(self.runnable().contains(&thread), "{thread:?} cannot run");
        let kind = match call {
            Syscall::CapDelete(_) | Syscall::CapRevoke(_) => sched::EventKind::Removal,
            Syscall::TaskRevoke(_) => sched::EventKind::TaskRevoke,
            _ => sched::EventKind::Call,
        };
        *self.last_event = Some((kind, 0));
        let core = self.core_of(thread);
        assert!(!self.cores[core].silent, "core {core} does not answer");
        let entry = self.begin_on(core);
        self.locks.lock_free = matches!(call, Syscall::Signal(_));
        let outcome = self.made(thread, call);
        self.end(entry);
        self.auto_deliver();
        outcome
    }

    fn made(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        let made = live(&mut self.threads, thread);
        (made.outcome, made.answered) = (None, false);
        let number = match call {
            Syscall::Call { wait, .. }
            | Syscall::Recv { wait, .. }
            | Syscall::ReplyRecv { wait, .. }
            | Syscall::Wait(_, wait) => wait,
            _ => 0,
        };
        let outcome = self.dispatch(thread, call)?;
        // Threads: a call that is about to wait with a wait number that is
        // not 0 and not greater than the word of the thread returns
        // `cancelled` instead, with the effects of a wait ended at once: a
        // `call` whose message was delivered has sent it, and the reply
        // object it armed is idle again. A call that ends without waiting
        // does not look at the word.
        if outcome == Output::Waits {
            let waiting = live(&mut self.threads, thread);
            if number != 0 && number <= waiting.cancelled_up_to {
                self.unlink(thread);
                return Err(Error::Cancelled);
            }
            waiting.number = number;
        }
        Ok(outcome)
    }

    /// One call, after which the slots of the invalid remote capabilities
    /// it used are dead.
    fn dispatch(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        self.stale.0.borrow_mut().clear();
        let outcome = self.dispatch_call(thread, call);
        self.bury_stale();
        outcome
    }

    /// The slots of the invalid remote capabilities found since the last
    /// call become dead, "unless its record holds a mapping: then the slot
    /// stays until a removal reaches it (the step of a `Revoke`, R2, or a
    /// removal by its holder), which undoes the mapping with its
    /// invalidations" (Grants, a remote frame capability, gap 15).
    pub(crate) fn bury_stale(&mut self) {
        let stale = std::mem::take(&mut *self.stale.0.borrow_mut());
        for cap in stale {
            let mapped = self
                .caps
                .get(&cap)
                .is_some_and(|c| matches!(c.record, Record::Mapping(_) | Record::Unmapping(_)));
            if self.caps.contains_key(&cap) && !mapped {
                self.remove_leaf_dead(cap);
            }
        }
    }

    fn dispatch_call(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        match call {
            Syscall::CapDerive {
                source,
                destination,
                rights,
                badge,
            } => self.cap_derive(thread, source, destination, rights, badge),
            Syscall::CapMove {
                source,
                destination,
            } => self.cap_move(thread, source, destination),
            Syscall::CapDelete(slot) => self.cap_delete(thread, slot),
            Syscall::CapRevoke(slot) => self.cap_revoke(thread, slot),
            Syscall::CapInspect(slot) => self.cap_inspect(thread, slot),
            Syscall::UntypedRetype {
                untyped,
                kind,
                size,
                page,
                memory,
                destination,
            } => {
                let size = caps::SizeArgument { size, page, memory };
                self.untyped_retype(thread, untyped, kind, size, destination)
            }
            Syscall::CSpaceGrow { cspace, untyped } => self.cspace_grow(thread, cspace, untyped),
            Syscall::ThreadConfigure {
                thread: target,
                cspace,
                aspace,
                fault,
                page,
            } => {
                let bindings = threads::Bindings {
                    cspace,
                    aspace,
                    fault,
                    page,
                };
                self.thread_configure(thread, target, bindings)
            }
            Syscall::ThreadResume(target) => self.thread_resume(thread, target),
            Syscall::ThreadReadRegs(target) => self.thread_read_regs(thread, target),
            Syscall::ThreadWriteRegs {
                thread: target,
                block,
            } => self.thread_write_regs(thread, target, block),
            Syscall::ThreadSuspend(target) => self.thread_suspend(thread, target),
            Syscall::ThreadCancel(target, number) => self.thread_cancel(thread, target, number),
            Syscall::ThreadSetFeatures {
                thread: target,
                mask,
            } => self.thread_set_features(thread, target, mask),
            Syscall::SchedCount { sched, node, mode } => {
                self.sched_count(thread, sched, node, mode)
            }
            Syscall::NodeStart {
                node,
                mode,
                image,
                module,
                window,
                mask,
            } => self.node_start(thread, node, (mode, image, module), (window, mask)),
            Syscall::NodeStop { node, mode, core } => self.node_stop(thread, node, mode, core),
            Syscall::NodeStatus { node, core } => self.node_status(thread, node, core),
            Syscall::NodeIdle { node, core, words } => self.node_idle(thread, node, core, &words),
            Syscall::NodeWatch {
                node,
                index,
                notify,
            } => self.node_watch(thread, node, index, notify),
            Syscall::NodePerf {
                node,
                core,
                lowest,
                highest,
                preference,
            } => self.node_perf(thread, node, core, (lowest, highest, preference)),
            Syscall::NodeSend {
                node,
                message,
                doorbell,
            } => self.node_send(thread, node, message, doorbell),
            Syscall::GrantSignal(frame) => self.grant_signal(thread, frame),
            Syscall::RoleClaim(role) => self.role_claim(thread, role),
            Syscall::DebugWrite {
                debug,
                length,
                words,
            } => self.debug_write(thread, debug, length, &words),
            Syscall::TraceEmit { debug, words } => self.trace_emit(thread, debug, words),
            Syscall::IoportIn {
                ioports,
                port,
                width,
            } => self.ioport(thread, ioports, port, width, None),
            Syscall::IoportOut {
                ioports,
                port,
                width,
                value,
            } => self.ioport(thread, ioports, port, width, Some(value)),
            Syscall::IoportsIssue {
                ioports,
                destination,
                first,
                last,
            } => self.ioports_issue(thread, ioports, destination, first, last),
            Syscall::EntropyRead { length, mix } => self.entropy_read(thread, length, mix),
            Syscall::RoleHand { role, node } => self.role_hand(thread, role, node),
            Syscall::Call {
                endpoint,
                message,
                range,
                wait: _,
            } => self.call(thread, endpoint, message, range),
            Syscall::Recv {
                endpoint,
                reply,
                range,
                wait: _,
            } => self.recv(thread, endpoint, reply, range),
            Syscall::ReplyRecv {
                reply,
                message,
                endpoint,
                range,
                wait: _,
            } => self.reply_recv(thread, reply, message, endpoint, range),
            Syscall::Send { target, message } => self.send(thread, target, message),
            Syscall::Signal(notify) => self.signal(thread, notify),
            Syscall::Wait(notify, _) => self.wait(thread, notify, ipc::IfZero::Wait),
            Syscall::Poll(notify) => self.wait(thread, notify, ipc::IfZero::WouldWait),
            Syscall::NotifyBind { notify, endpoint } => self.notify_bind(thread, notify, endpoint),
            Syscall::ASpaceMap(map) => self.aspace_map(thread, map, ObjectKind::ASpace),
            Syscall::IoSpaceMap(map) => self.aspace_map(thread, map, ObjectKind::IoSpace),
            Syscall::ASpaceUnmap(frame) => self.unmap(thread, frame, ObjectKind::ASpace),
            Syscall::IoSpaceUnmap(frame) => self.unmap(thread, frame, ObjectKind::IoSpace),
            Syscall::ASpaceProtect {
                aspace,
                frame,
                rights,
            } => self.aspace_protect(thread, aspace, frame, rights),
            Syscall::IoSpaceAttach {
                iommu,
                iospace,
                device,
            } => self.iospace_attach(thread, iommu, iospace, device),
            Syscall::SchedConfigure {
                sched,
                node,
                core,
                budget,
                period,
                class,
            } => {
                let parameters = sched::Parameters {
                    budget,
                    period,
                    class,
                };
                self.sched_configure(thread, sched, node, core, parameters)
            }
            Syscall::SchedBind {
                sched,
                thread: target,
            } => self.sched_bind(thread, sched, target),
            Syscall::SchedUnbind(sched) => self.sched_unbind(thread, sched),
            Syscall::SchedSetCores {
                sched,
                node,
                window,
                mask,
            } => self.sched_set_cores(thread, sched, node, (window, mask)),
            Syscall::SchedRead(sched) => self.sched_read(thread, sched),
            Syscall::Yield => self.yield_now(thread),
            Syscall::TimerSet {
                timer,
                notify,
                deadline,
                tolerance,
            } => self.timer_set(thread, timer, notify, (deadline, tolerance)),
            Syscall::TimerCancel(timer) => self.timer_cancel(thread, timer),
            Syscall::IrqClaim {
                control,
                source,
                core,
                vector,
                destination,
            } => self.irq_claim(thread, control, (source, core, vector), destination),
            Syscall::IrqBind { irq, notify } => self.irq_bind(thread, irq, notify),
            Syscall::IrqAck(irq) => self.irq_ack(thread, irq),
            Syscall::TaskExempt { task, aware } => self.task_exempt(thread, task, aware),
            Syscall::TaskAttach {
                thread: target,
                task,
            } => self.task_attach(thread, target, task),
            Syscall::TaskMark { task, marks } => self.task_mark(thread, task, marks),
            Syscall::TaskRevoke(task) => self.task_revoke(thread, task),
            Syscall::TaskRead(task) => self.task_read(thread, task),
            Syscall::MemoryTake {
                control,
                kind,
                place,
                size,
                destination,
            } => self.memory_take(thread, control, (kind, place, size), destination),
            Syscall::MemoryGive {
                control,
                untyped,
                node,
            } => self.memory_give(thread, control, untyped, node),
            Syscall::MemorySplit { control, untyped } => {
                self.memory_split(thread, control, untyped)
            }
        }
    }

    // Order of checks. Each call checks its capability arguments with the
    // functions below, in the order of its table (steps 1 and 2), then its
    // plain arguments (step 3), then the state (step 4).

    /// Slots and addresses: the capability space that an address is in.
    /// Through a `cspace` capability, `right` is the right needed: `READ` to
    /// look at a slot, `WRITE` to change it.
    fn addressed_space(
        &self,
        thread: ObjId,
        addr: SlotAddr,
        right: Rights,
    ) -> Result<ObjId, Error> {
        let own = self.bound_space(thread)?;
        // Order of checks: a thread whose capability space is dying gets
        // `gone` from every call except `cap_delete` on its own slots.
        if self.dying(own) {
            return Err(Error::Gone);
        }
        self.through(own, addr, right)
    }

    /// Order of checks: a thread bound to no capability space, or attached
    /// to a revoked task, gets `gone` from every call, `cap_delete` too. The
    /// derivation tree: a thread whose capability space is destroyed is
    /// bound to none (D-110).
    ///
    /// A thread of a revoked task cannot even delete its own slots (Tasks
    /// and marks, Order of checks).
    fn bound_space(&self, thread: ObjId) -> Result<ObjId, Error> {
        let bound = &self.threads[&thread];
        if bound.task.is_some_and(|task| self.tasks[&task].revoked) {
            return Err(Error::Gone);
        }
        bound.cspace.ok_or(Error::Gone)
    }

    /// Slots and addresses: `own`, or the capability space that `via` names
    /// in it.
    fn through(&self, own: ObjId, addr: SlotAddr, right: Rights) -> Result<ObjId, Error> {
        if addr.via() == 0 {
            return Ok(own);
        }
        let via = self.full(own, addr.via())?;
        Ok(self.is(via, ObjectKind::CSpace, right)?.object)
    }

    /// Order of checks, steps 1 and 2, the slot: `invalid` if empty or out
    /// of range, `gone` if dead. Removal ends: `gone` if the capability is
    /// dying, or its object is.
    fn full(&self, cspace: ObjId, index: u32) -> Result<CapId, Error> {
        match self.cspaces[&cspace].slots.get(index as usize) {
            Some(&Slot::Full(cap)) => {
                let held = &self.caps[&cap];
                let dying = held.dying || self.dying(held.object);
                // Grants: a use of an invalid remote capability is `gone`,
                // judged with the slot, and the slot becomes dead at the end
                // of the call.
                let invalid = self
                    .remotes
                    .get(&held.object)
                    .is_some_and(|r| !self.valid(r));
                if invalid {
                    self.stale.0.borrow_mut().push(cap);
                }
                if dying || invalid {
                    Err(Error::Gone)
                } else {
                    Ok(cap)
                }
            }
            Some(Slot::Dead) => Err(Error::Gone),
            Some(Slot::Empty) | None => Err(Error::Invalid),
        }
    }

    /// Removal ends: an object is dying from the first step of a removal
    /// that reaches its root capability. An object that does not exist, or
    /// has no root capability, is not: `check` reports it.
    fn dying(&self, object: ObjId) -> bool {
        let root = self.objects.get(&object).map(|object| object.root);
        root.is_some_and(|root| self.caps.get(&root).is_some_and(|root| root.dying))
    }

    /// Order of checks, steps 1 and 2: the slot of a capability argument.
    fn cap(&self, thread: ObjId, addr: SlotAddr, right: Rights) -> Result<CapId, Error> {
        self.full(self.addressed_space(thread, addr, right)?, addr.index())
    }

    /// Order of checks, steps 1 and 2: the kind (`invalid`), then the rights
    /// (`denied`). A revoked task named where another kind is asked is
    /// `gone` before its kind is judged (Tasks and marks, "A revoked task";
    /// pass 35: it was `invalid`); where a task is asked, `task` judges it,
    /// and `task_read` and `task_revoke` are exempt.
    fn is(&self, cap: CapId, kind: ObjectKind, rights: Rights) -> Result<&Cap, Error> {
        let held = &self.caps[&cap];
        let revoked = self
            .tasks
            .get(&held.object)
            .is_some_and(|task| task.revoked);
        if revoked && kind != ObjectKind::Task {
            return Err(Error::Gone);
        }
        if self.objects[&held.object].kind != kind {
            return Err(Error::Invalid);
        }
        if !held.rights.contains(rights) {
            return Err(Error::Denied);
        }
        Ok(held)
    }

    /// Order of checks, steps 1 and 2: the three checks of a capability
    /// argument, in order. `need` is the rights that the call needs.
    fn held(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<&Cap, Error> {
        self.is(self.cap(thread, at, Rights::READ)?, kind, need)
    }

    /// As `held`, for a call that needs only the object.
    fn object(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<ObjId, Error> {
        Ok(self.held(thread, at, kind, need)?.object)
    }

    /// As `object`, for an argument that may be 0.
    fn optional(
        &self,
        thread: ObjId,
        at: SlotAddr,
        kind: ObjectKind,
        need: Rights,
    ) -> Result<Option<ObjId>, Error> {
        match at.is_none() {
            true => Ok(None),
            false => Ok(Some(self.object(thread, at, kind, need)?)),
        }
    }

    /// Order of checks, step 2: a slot in which the call places a
    /// capability. Slots and addresses: slot 0 or beyond the table is
    /// `invalid`.
    fn destination(&self, thread: ObjId, addr: SlotAddr) -> Result<Place, Error> {
        let space = self.addressed_space(thread, addr, Rights::WRITE)?;
        if addr.index() == 0 || addr.index() as usize >= self.cspaces[&space].slots.len() {
            return Err(Error::Invalid);
        }
        Ok(Place {
            space,
            index: addr.index(),
        })
    }

    /// Order of checks, step 4: `no_slot` if the destination is not empty.
    fn vacant(&self, Place { space, index }: Place) -> Result<(), Error> {
        match self.cspaces[&space].slots[index as usize] {
            Slot::Empty => Ok(()),
            Slot::Full(_) | Slot::Dead => Err(Error::NoSlot),
        }
    }

    /// Order of checks, step 2, and Transfer of capabilities: the receive
    /// range, resolved when the call is made. Whether its slots are empty
    /// is checked at delivery.
    fn window(&self, thread: ObjId, mut range: Range) -> Result<Window, Error> {
        // Transfer of capabilities: an empty range has no address.
        if range.count == 0 {
            range.first = SlotAddr::NONE;
        }
        let (first, count) = (range.first.index(), range.count);
        let cspace = self.addressed_space(thread, range.first, Rights::WRITE)?;
        // Transfer of capabilities: slot 0 or beyond the table is `invalid`.
        let end = u64::from(first) + u64::from(count);
        let beyond = first == 0 || end > self.cspaces[&cspace].slots.len() as u64;
        // Order of checks, step 3: a receive range has at most 4 slots.
        if count != 0 && beyond || count as usize > MAX_CAPS {
            return Err(Error::Invalid);
        }
        Ok(Window {
            cspace,
            first,
            count,
        })
    }

    /// Ends a bounded wait (one that holds no queue): its outcome may be
    /// any outcome of its call.
    pub(crate) fn end_bounded(&mut self, thread: ObjId) {
        let ended = live(&mut self.threads, thread);
        (ended.wait, ended.answered) = (Wait::None, true);
    }

    /// Ends the wait of a thread that `unlink` took out of what it waited
    /// on: `outcome` is the outcome of the call it waited in. The wait
    /// number goes with the wait. A caller that a server receives is taken
    /// out of the queue and goes on waiting, for the reply, with the same
    /// number: that is not the end of its wait.
    ///
    /// Faults: a thread that waits on a fault has no call to return to: an
    /// answer has its effect on the thread, and a wait that ends without an
    /// answer leaves it suspended with the fault dropped.
    fn wake(&mut self, thread: ObjId, outcome: Outcome) {
        self.locks.woke = true;
        let woken = live(&mut self.threads, thread);
        woken.number = 0;
        if std::mem::take(&mut woken.faulting) {
            woken.raised = None;
            return self.answered(thread, outcome);
        }
        woken.outcome = Some(outcome);
        self.observe();
    }

    /// Checks a message header and records exact `MAX_WORDS` / `MAX_CAPS`
    /// refusal hits in test builds.
    pub(crate) fn check_message_header(
        &mut self,
        thread: ObjId,
        message: &Message,
    ) -> Result<(), Error> {
        let res = message.check_header(self.has_page(thread));
        #[cfg(test)]
        if res == Err(Error::Invalid) {
            if message.words.len() > dodos_abi::MAX_WORDS {
                self.bound_hits.0.msg_words_over_64_refused = true;
            }
            if message.caps.len() > dodos_abi::MAX_CAPS {
                self.bound_hits.0.send_grants_over_4_refused = true;
            }
        }
        res
    }

    /// Checks whether the unit request table is full, recording the exact
    /// 65th-request refusal hit (and recovery overlap) in test builds.
    pub(crate) fn unit_requests_full(&mut self) -> bool {
        let full = self.unit_requests.iter().all(Option::is_some);
        #[cfg(test)]
        if full {
            self.bound_hits.0.unit_requests_65th_refused = true;
            let in_rec = self.recovery_in_progress(None)
                || self.unit_requests.iter().flatten().any(|e| {
                    let word = self.table.roles[e.role];
                    self.recovery_in_progress(Some(word.holder()))
                        || (word.state() == dodos_abi::RoleState::Handing
                            && self.recovery_in_progress(Some(word.target())))
                });
            if in_rec {
                self.bound_hits.0.unit_requests_65th_in_recovery = true;
            }
        }
        full
    }

    /// Whether a death, fence, seal, fresh start, or rebuild involving this
    /// node (or `peer`, when named) is in progress right now.
    #[cfg(test)]
    pub(crate) fn recovery_in_progress(&self, peer: Option<u16>) -> bool {
        let node_recovering = |z: u16| {
            let m = self.table.member(z);
            let st = m.state();
            matches!(
                st,
                dodos_abi::MemberState::Fencing
                    | dodos_abi::MemberState::Fenced
                    | dodos_abi::MemberState::Unfenced
                    | dodos_abi::MemberState::Sealing
                    | dodos_abi::MemberState::Sealed
                    | dodos_abi::MemberState::Starting
            ) || self.reclaims.contains_key(&z)
                || self.fences.contains_key(&z)
                || self.rebuilds.values().any(|r| r.node == z)
                || self.starts.values().any(|s| s.node == z)
                || self.table.roles.iter().any(|w| {
                    w.state() == dodos_abi::RoleState::Handing
                        && (w.holder() == z || w.target() == z)
                })
        };
        if node_recovering(self.node)
            || self.seal.is_some()
            || !self.rebuilds.is_empty()
            || !self.reclaims.is_empty()
            || !self.fences.is_empty()
            || !self.starts.is_empty()
            || !self.handing.is_empty()
        {
            return true;
        }
        peer.is_some_and(node_recovering)
    }
}
