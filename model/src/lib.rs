//! Executable reference model of the DodOS kernel, for one core.
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
//! written); the hardware (translations are a map; the invalidations of the
//! TLB and of the IOMMU are done before a call returns; masking, the end of
//! interrupt and the entropy pool are not seen); the kernel's own time,
//! taken as zero; more than one node (the waiting-for-the-core-group state
//! is never entered); and the groups not specified (cores, debugging,
//! `ioports`), whose calls return `unsupported`.
//!
//! `CONTRACT-GAP` marks a behaviour that the contract does not fix.

#![forbid(unsafe_code)]

mod blocks;
mod boot;
mod caps;
mod check;
mod faults;
mod ipc;
mod irq;
mod memory;
mod moves;
mod remote;
mod sched;
mod system;
mod table;
mod tasks;
mod threads;

pub use boot::{
    Arch, Core, File, IoApic, IommuUnit, Machine, Refusal, Region, RegionKind, SIXTEEN_KB, Segment,
};
pub use check::Census;
pub use remote::{Body, Crossing, Granted, Packet};
pub use system::System;

use dodos_abi::{
    Call, Error, Geometry, IrqSource, MAX_CAPS, MemoryType, ObjectKind, PageSize, Received, Rights,
    SlotAddr, TaskId,
};
use std::collections::{BTreeMap, VecDeque};
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
    /// The context, the `core` capability of its node, and the mode.
    SchedCount {
        sched: SlotAddr,
        core: SlotAddr,
        mode: u64,
    },
    /// `thread_move`: the thread, the destination thread and sched (remote
    /// capabilities of the destination node), its `core` capability, and
    /// the parameters (all 0: the context's own).
    ThreadMove {
        thread: SlotAddr,
        destination: SlotAddr,
        sched: SlotAddr,
        core: SlotAddr,
        budget: u64,
        period: u64,
        class: u64,
    },
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
    SchedConfigure {
        sched: SlotAddr,
        core: SlotAddr,
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
        core: SlotAddr,
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
    /// The irq_control, the source word, the vector and the destination.
    IrqClaim {
        control: SlotAddr,
        source: u64,
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
    /// The memory_control, the kind (0 a run, 1 a device entry), the place
    /// (a physical base or an entry's index), the size (the log2 of the
    /// run's bytes, or 0), and the destination.
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
    /// A call that this model does not make: it returns `Unsupported`. A
    /// call that has a variant above is refused here with a panic.
    Other(Call),
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
    /// `send`: 0, or `Header::FLAG_CAPS_DROPPED`.
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Mapping {
    space: ObjId,
    address: u64,
    first: u64,
    count: u64,
    rights: Rights,
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
    /// and are not examined again.
    next: usize,
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
    /// The gate over page tables (G2): the physical pages of its root table
    /// and of the lower tables taken from its untyped, of class *table*. The
    /// start-up aspace, made from kept memory, has none here.
    table_pages: Vec<u64>,
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
    /// Faults: a fault that was not delivered, recorded in the thread: its
    /// kind and its five words, until the thread is resumed.
    recorded: Option<(u32, [u64; 5])>,
    /// Scheduling contexts: its own context, if it is that context's home.
    home: Option<ObjId>,
    /// The budget follows the call: marked as having lost its context.
    lost: bool,
    /// Tasks and marks: the task it is attached to, and its own marks word.
    task: Option<ObjId>,
    marks: u64,
    /// A proxy: the stamp of the thread of another node it stands for, and
    /// the node, the request and the number of capabilities its message
    /// lists, which are transferred by grants at delivery.
    stamp: Option<(u64, u64)>,
    foreign: Option<(u16, u64, usize)>,
    /// Unlike cores: the feature mask, and the bytes of the thread as it
    /// was made, which fix its save area.
    mask: u64,
    area: u64,
    /// The outcome is the answer of another node to a forwarded call: any
    /// outcome that call has.
    answered: bool,
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
    /// Waiting for another node: the request this node sent (Calls on an
    /// object of another node).
    Node(u64),
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
}

/// Interrupts: one entry of the interrupt table of the node.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Irq {
    vector: u8,
    source: IrqSource,
    state: IrqState,
    /// An arrival of an edge source that was not signalled.
    pending: bool,
    /// The notify it is bound to, and the bits it sets.
    notify: Option<(ObjId, u64)>,
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
    /// Signed nanoseconds left in the current period.
    remaining: i64,
    deadline: u64,
    /// While throttled: the replenishment instant, and the number of
    /// budgets it brings.
    throttled: Option<(u64, u64)>,
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
}

/// Bookkeeping of the current or last event: what the invariants read to
/// judge an event. Two states that differ only in it are the same state,
/// so it is equal to any other.
#[derive(Clone, Debug, Default)]
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
    /// The destination threads of `thread_move` placed in the event, with
    /// the effective marks of their source.
    moved: Vec<(ObjId, u64)>,
}

/// The whole state of the kernel of one core.
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
    /// The `core` objects, with their node index.
    cores: BTreeMap<ObjId, u32>,
    /// The `role` objects, with the index of their role.
    roles: BTreeMap<ObjId, usize>,
    /// The index of this node, and the machine table it shares with the
    /// other nodes.
    node: u16,
    table: table::Table,
    /// The write gate: the chunks the node maps in its kernel space.
    kernel_runs: std::collections::BTreeSet<u64>,
    /// IOMMU: the devices each `iospace` serves, the most recently attached
    /// last, each by its `iommu` and its identifier.
    iospaces: BTreeMap<ObjId, Vec<(ObjId, u16)>>,
    iommus: BTreeMap<ObjId, Iommu>,
    scheds: BTreeMap<ObjId, Sched>,
    /// Timers: `None` while idle.
    timers: BTreeMap<ObjId, Option<Armed>>,
    /// The budget follows the call: the lends, by the reply object that
    /// records each, with its context and its receiver.
    lends: BTreeMap<ObjId, (ObjId, ObjId)>,
    /// Clock: the instant of the clock, which only `Model::advance` moves,
    /// and the instant of the previous event.
    now: u64,
    previous: u64,
    /// What runs next: the context that runs, and whether from the ready
    /// queue (not from a slack queue).
    running: Option<(ObjId, bool)>,
    /// Admission: the sum of the bandwidths of the configured contexts that
    /// are not dying.
    admitted: u64,
    /// What runs next: the order of entry into the queues.
    entered: u64,
    /// Order inside an event: what each context was at the start of the
    /// event (eligible, remaining, throttled, queue and key), the change
    /// list of the node, and its no-context list.
    snapshot: Ephemeral<BTreeMap<ObjId, sched::Snap>>,
    change_list: Ephemeral<Vec<ObjId>>,
    no_context_list: Ephemeral<Vec<ObjId>>,
    /// Bounds: the contexts the last event put in a new queue position,
    /// and what that event was.
    last_event: Ephemeral<Option<(sched::EventKind, usize)>>,
    /// What the last event did, for the invariants that judge an event.
    event: Ephemeral<EventLog>,
    /// Calls on an object of another node: the messages the last event
    /// sent, the requests this node waits on, the remote objects it holds,
    /// its kernel space (anchors and proxies) and its proxies with the
    /// request each stands in, and its requests to each peer, sent and held.
    outbox: Vec<remote::Packet>,
    requests: BTreeMap<u64, remote::Request>,
    next_request: u64,
    remotes: BTreeMap<ObjId, remote::Remote>,
    kernel_space: Option<ObjId>,
    mailbox: Option<ObjId>,
    /// The thread whose call is the removal in progress, which waits when an
    /// anchor's grant needs its holder (Grants).
    remover: Option<ObjId>,
    /// A transfer between nodes that the current delivery began, and the
    /// last one an event began.
    transfer: Option<u64>,
    last_transfer: Option<u64>,
    proxies: BTreeMap<ObjId, Option<(u16, u64)>>,
    /// Remote capabilities found invalid during the call, whose slots
    /// become dead when it ends.
    stale: Ephemeral<std::cell::RefCell<Vec<CapId>>>,
    /// What a core enables for a thread: the state features enabled on the
    /// core, as the last switch wrote them.
    enabled: u32,
    /// Other models of the machine share its table (`System`): the map
    /// counts hold their leaves too.
    shared: bool,
    /// The slots that the capabilities moved into anchors came from.
    moved_from: BTreeMap<u16, Place>,
    outstanding: BTreeMap<u16, usize>,
    held_requests: BTreeMap<u16, VecDeque<remote::Body>>,
    /// Interrupts: the claimed entries of the interrupt table.
    irqs: BTreeMap<ObjId, Irq>,
    tasks: BTreeMap<ObjId, Task>,
    /// Tasks and marks: the counter of the identifiers of the node.
    task_counter: u64,
    /// What the start-up state was built from.
    machine: Machine,
}

/// Whether a call is one that `Syscall::Other` stands for: the calls of the
/// groups not yet specified (cores, debugging), and those this model does
/// not make yet.
fn other(call: Call) -> bool {
    use Call::{
        CoreIdle, CorePerf, CorePlace, CoreStart, CoreStatus, CoreStop, CoreWatch, DebugWrite,
        EntropyRead, IoportIn, IoportOut, IoportsIssue, RoleClaim, RoleHand, TraceEmit,
    };
    matches!(
        call,
        CoreStart
            | CoreStop
            | CoreStatus
            | DebugWrite
            | TraceEmit
            | CoreIdle
            | CoreWatch
            | CorePlace
            | RoleClaim
            | RoleHand
            | IoportIn
            | IoportOut
            | IoportsIssue
            | EntropyRead
            | CorePerf
    )
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

    /// The threads that can run: not suspended and not waiting. A call can
    /// be applied for any of them.
    pub fn runnable(&self) -> Vec<ObjId> {
        let ready = |thread: &&Thread| !thread.suspended && thread.wait == Wait::None;
        let ids = self.threads.iter().filter(|(_, thread)| ready(thread));
        ids.map(|(&id, _)| id).collect()
    }

    /// Unlike cores: the state features the core enables, as the last
    /// switch wrote them.
    pub fn enabled(&self) -> u32 {
        self.enabled
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
        self.table.owners[chunk as usize] = raw;
    }

    /// The raw ownership word of a chunk.
    pub fn ownership(&self, chunk: u64) -> u64 {
        self.table.owners[chunk as usize]
    }

    /// The map count of a chunk (G4).
    pub fn map_count(&self, chunk: u64) -> u64 {
        self.table.maps[chunk as usize]
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
    pub fn apply(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        assert!(self.runnable().contains(&thread), "{thread:?} cannot run");
        let kind = match call {
            Syscall::CapDelete(_) | Syscall::CapRevoke(_) => sched::EventKind::Removal,
            Syscall::TaskRevoke(_) => sched::EventKind::TaskRevoke,
            _ => sched::EventKind::Call,
        };
        *self.last_event = Some((kind, 0));
        let entry = self.begin();
        let outcome = self.made(thread, call);
        self.end(entry);
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
    /// call become dead.
    pub(crate) fn bury_stale(&mut self) {
        let stale = std::mem::take(&mut *self.stale.0.borrow_mut());
        for cap in stale {
            if self.caps.contains_key(&cap) {
                self.remove_leaf_dead(cap);
            }
        }
    }

    fn dispatch_call(&mut self, thread: ObjId, call: Syscall) -> Outcome {
        if let Some(outcome) = self.forwarded(thread, &call) {
            return outcome;
        }
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
            Syscall::SchedCount { sched, core, mode } => {
                self.sched_count(thread, sched, core, mode)
            }
            Syscall::ThreadMove {
                thread: source,
                destination,
                sched,
                core,
                budget,
                period,
                class,
            } => self.thread_move(
                thread,
                source,
                (destination, sched, core),
                (budget, period, class),
            ),
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
                self.sched_configure(thread, sched, core, parameters)
            }
            Syscall::SchedBind {
                sched,
                thread: target,
            } => self.sched_bind(thread, sched, target),
            Syscall::SchedUnbind(sched) => self.sched_unbind(thread, sched),
            Syscall::SchedSetCores {
                sched,
                core,
                window,
                mask,
            } => self.sched_set_cores(thread, sched, core, (window, mask)),
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
                vector,
                destination,
            } => self.irq_claim(thread, control, (source, vector), destination),
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
            // The calls not yet made by this model, and those of the groups
            // not yet specified (cores, debugging), return `unsupported` as
            // soon as the capability space of the caller is found, before
            // any argument is looked at. Removal ends: a thread whose
            // capability space is dying or destroyed gets `gone` from these
            // calls as from every other.
            Syscall::Other(call) => {
                assert!(other(call), "{call:?} has its own variant of `Syscall`");
                self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
                Err(Error::Unsupported)
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
    /// (`denied`).
    fn is(&self, cap: CapId, kind: ObjectKind, rights: Rights) -> Result<&Cap, Error> {
        let held = &self.caps[&cap];
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
        // A delivery whose capabilities another node grants: the receiver
        // waits for that node instead.
        if let Some(id) = self.transfer.take() {
            if let Ok(Output::Message(delivery)) = outcome {
                return self.transfer_wait(id, thread, delivery);
            }
            self.transfer = Some(id);
        }
        let woken = live(&mut self.threads, thread);
        woken.number = 0;
        if std::mem::take(&mut woken.faulting) {
            return self.answered(thread, outcome);
        }
        // A proxy's call is over: its outcome goes back to its node.
        if woken.stamp.is_some() {
            return self.answer_proxy(thread, outcome);
        }
        woken.outcome = Some(outcome);
        self.observe();
    }
}
