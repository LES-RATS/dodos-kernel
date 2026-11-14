//! Types of the DodOS kernel interface.
//!
//! Source of truth: `architecture/kernel-interface.md` in the documentation
//! repository. This crate is the contract in code: the executable model, the
//! kernel and user space all build against it. Every value is provisional
//! until the one-core model is accepted.
//!
//! Call numbers are deliberately absent: they are private to the
//! kernel-supplied code page and can change between kernel versions.

#![no_std]
#![forbid(unsafe_code)]

mod start;
pub mod x86_64;

pub use start::{
    FIRST_SERVICE_MIN_ADDR, SHARE_ALIGN, START_FREE_SLOTS, START_MAGIC, START_MESSAGE_PAGE_ADDR,
    START_PAGE_ADDR, START_PAGE_MAX_PAGES, START_RESERVE_BYTES, START_SLOT_ASPACE,
    START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_FIRST, START_SLOT_IOPORTS,
    START_SLOT_IRQ_CONTROL, START_SLOT_MESSAGE_PAGE, START_SLOT_MESSAGE_PAGE_MAP, START_SLOT_SCHED,
    START_SLOT_START_PAGE, START_SLOT_TASK, START_SLOT_THREAD, START_UNTYPED_DEVICE,
    START_UNTYPED_IMAGE, START_VERSION, StartBootTimes, StartCore, StartEntry, StartFramebuffer,
    StartHeader, StartInterrupts, StartIoApic, StartIommu, StartKept, StartKind, StartMemory,
    StartModule, StartNode, StartProcess, StartShare, StartUntyped,
};

/// Message words carried in registers on x86-64. A message that fits here,
/// carries no capability and has no flag takes the fast path.
pub const REG_WORDS: usize = 6;
/// Message words in all, registers plus the per-thread message page.
pub const MAX_WORDS: usize = 64;
/// Capabilities per message.
pub const MAX_CAPS: usize = 4;
/// Bytes of one capability slot (Object sizes).
pub const SLOT_BYTES: usize = 64;
/// Slots added by one `cspace_grow`: one segment of 8 KB.
pub const CSPACE_SEGMENT_SLOTS: u32 = 128;
/// Units of work of one call of a removal before it returns `Again`: a
/// capability removed, a thread woken, or one link of a destroyed object
/// undone (The derivation tree, provisional).
pub const REMOVAL_STEP: usize = 32;
/// Slots of dying capability spaces, or identifiers of a destroyed `iommu`,
/// examined by one call of a removal before it returns `Again` (The
/// derivation tree, provisional).
pub const REMOVAL_SLOTS: usize = 128;

/// Greatest depth of the derivation tree.
pub const TREE_DEPTH: usize = 64;
/// Greatest length, in capabilities, of the walk that finds the next
/// capability to remove (The derivation tree, the "Why" of the depth rule).
/// On untyped capabilities and roots of spaces the depth never shrinks and
/// stays equal at most once per level: two per level. Then on ordinary
/// capabilities, one per level. The untyped at the last level cannot be on
/// such a walk: nothing can be derived from it, so the walk would end there.
pub const LONGEST_WALK: usize = 2 * TREE_DEPTH - 1 + TREE_DEPTH;

/// Greatest maximum number of slots of a capability space (Slots and
/// addresses, provisional).
pub const CSPACE_MAX_SLOTS: u32 = 1 << 20;
/// Bytes of the directory of a capability space for each segment of its
/// maximum, before the directory is rounded up to a power of two (Slots and
/// addresses).
pub const CSPACE_DIRECTORY_BYTES: usize = 8;

/// Bytes of the smallest `untyped` (Capabilities and objects,
/// `untyped_retype`: a power of two, at least 4 KB).
pub const UNTYPED_MIN_BYTES: u64 = 4096;
/// Bytes that one call of a creation clears, at most (rules of
/// `untyped_retype`, the clean mark).
pub const CLEAR_STEP_BYTES: u64 = 65_536;
/// Bytes of an `endpoint`, of a `reply`, of a `notify` and of a `timer`
/// (Object sizes).
pub const ENDPOINT_BYTES: usize = 64;
pub const REPLY_BYTES: usize = 64;
pub const NOTIFY_BYTES: usize = 64;
pub const TIMER_BYTES: usize = 64;
/// Bytes of one entry of the interrupt table.
pub const IRQ_BYTES: usize = 64;
/// Bytes of a `task` (Object sizes).
pub const TASK_BYTES: usize = 256;
/// Bytes of a `sched` (Object sizes).
pub const SCHED_BYTES: usize = 128;
/// Bytes of a `thread` in the one-core reference, without the vector state
/// area (Capabilities and objects, rules of `untyped_retype`).
pub const THREAD_BYTES: usize = 1024;
/// Bytes of an `aspace` and of an `iospace`: the root table and a header
/// (Object sizes).
pub const ASPACE_BYTES: usize = 8192;
pub const IOSPACE_BYTES: usize = 8192;
/// Bytes of one lower table of an address space, and its entries.
pub const TABLE_BYTES: usize = 4096;
pub const TABLE_ENTRIES: usize = 512;
/// Pages of one frame, at most.
pub const FRAME_MAX_PAGES: u64 = 512;
/// Devices that one `iospace` serves, at most.
pub const IOSPACE_MAX_DEVICES: usize = 16;
/// Nanoseconds within which an IOMMU unit confirms an invalidation.
pub const IOMMU_WAIT_NS: u64 = 1_000_000;

/// Address of a capability slot: an index, reached directly in the caller's
/// own capability space (`via` = 0) or through one `cspace` capability held
/// at index `via` of the caller's own space. Address 0 is "no capability".
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotAddr(u64);

impl SlotAddr {
    pub const NONE: Self = Self(0);

    pub const fn own(index: u32) -> Self {
        Self(index as u64)
    }

    pub const fn through(via: u32, index: u32) -> Self {
        Self((via as u64) << 32 | index as u64)
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    pub const fn via(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// Result of a system call. The set is fixed; every caller handles `Gone` and
/// `Again`.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Ok = 0,
    /// The capability was revoked or the object deleted.
    Gone = 1,
    /// A bounded step was done; repeat the call.
    Again = 2,
    /// A right is missing.
    Denied = 3,
    /// Bad slot, wrong object type or bad argument.
    Invalid = 4,
    /// The supplied untyped memory is exhausted.
    NoMemory = 5,
    /// The destination slot is not empty, or the capability space is at its
    /// maximum.
    NoSlot = 6,
    /// The object is in a state that forbids the call.
    Busy = 7,
    /// Admission refused: the bandwidths admitted on the core would exceed
    /// one whole core.
    Budget = 8,
    /// A non-blocking call found nothing.
    WouldWait = 9,
    /// Not available on this architecture or kernel version.
    Unsupported = 10,
    /// The wait was ended by `thread_cancel`, or was not begun because its
    /// wait number was already cancelled.
    Cancelled = 11,
}

/// Rights carried by a capability. Deriving can only remove rights.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(u32);

impl Rights {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    /// Map a frame executable. Only the compile service receives it.
    pub const EXEC: Self = Self(1 << 2);
    pub const SEND: Self = Self(1 << 3);
    pub const RECV: Self = Self(1 << 4);
    /// Transfer capabilities through an endpoint.
    pub const GRANT: Self = Self(1 << 5);
    pub const DERIVE: Self = Self(1 << 6);
    pub const MANAGE: Self = Self(1 << 7);
    pub const ALL: Self = Self(0xff);

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// `None` if `bits` names a right that does not exist.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// These rights without those of `other`.
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Provenance marks of a task. Marks only ever add.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marks(u64);

impl Marks {
    pub const NONE: Self = Self(0);
    pub const PRIVATE: Self = Self(1 << 0);
    pub const UNTRUSTED: Self = Self(1 << 1);
    const KNOWN: u64 = 0b11;

    /// `Invalid` if a reserved bit is set.
    pub const fn from_bits(bits: u64) -> Result<Self, Error> {
        if bits & !Self::KNOWN != 0 {
            return Err(Error::Invalid);
        }
        Ok(Self(bits))
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Kernel objects.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Untyped = 0,
    CSpace = 1,
    ASpace = 2,
    Frame = 3,
    Thread = 4,
    Sched = 5,
    Endpoint = 6,
    Reply = 7,
    Notify = 8,
    Timer = 9,
    Irq = 10,
    IoPorts = 11,
    IoSpace = 12,
    Task = 13,
    Core = 14,
    Debug = 15,
    /// Reserved, not built.
    Vcpu = 16,
    IrqControl = 17,
    Iommu = 18,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    Size4K = 0,
    Size2M = 1,
    Size1G = 2,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Size4K => 1 << 12,
            Self::Size2M => 1 << 21,
            Self::Size1G => 1 << 30,
        }
    }

    /// `None` for a value that names no page size.
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Size4K),
            1 => Some(Self::Size2M),
            2 => Some(Self::Size1G),
            _ => None,
        }
    }
}

/// The memory type of a frame (Frames).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    /// The only type of ordinary memory.
    WriteBack = 0,
    Uncached = 1,
    WriteCombining = 2,
}

impl MemoryType {
    /// `Invalid` for a value that names no type, or a type that does not
    /// suit the kind of memory.
    pub const fn for_memory(raw: u64, device: bool) -> Result<Self, Error> {
        match (raw, device) {
            (0, false) => Ok(Self::WriteBack),
            (1, true) => Ok(Self::Uncached),
            (2, true) => Ok(Self::WriteCombining),
            _ => Err(Error::Invalid),
        }
    }
}

/// Calls on address spaces: the plain arguments that place a mapping. The
/// address is a multiple of the page size; the first page is in the frame;
/// the page count is at least 1 and does not run past the frame; and the
/// range lies below `limit`, computed without overflow. `Invalid` otherwise,
/// in that order.
pub const fn mapping_range(
    address: u64,
    first: u64,
    count: u64,
    frame_pages: u64,
    page: PageSize,
    limit: u64,
) -> Result<(), Error> {
    let size = page.bytes();
    if !address.is_multiple_of(size) || first >= frame_pages {
        return Err(Error::Invalid);
    }
    if count == 0 || count > frame_pages - first {
        return Err(Error::Invalid);
    }
    match count.checked_mul(size) {
        Some(bytes) if address <= limit && bytes <= limit - address => Ok(()),
        _ => Err(Error::Invalid),
    }
}

/// Calls on address spaces: the rights word of a mapping in an `aspace`. A
/// bit other than `read`, `write` and `exec`, a word without `read`, or with
/// both `write` and `exec`, is `Invalid`.
pub const fn mapping_rights(word: u32) -> Result<Rights, Error> {
    let known = Rights::READ.0 | Rights::WRITE.0 | Rights::EXEC.0;
    let both = Rights::WRITE.0 | Rights::EXEC.0;
    if word & !known != 0 || word & Rights::READ.0 == 0 || word & both == both {
        return Err(Error::Invalid);
    }
    Ok(Rights(word))
}

/// IOMMU: the rights word of a mapping in an `iospace`, a non-empty subset
/// of `read` and `write`; `Invalid` otherwise, `exec` included.
pub const fn io_mapping_rights(word: u32) -> Result<Rights, Error> {
    let known = Rights::READ.0 | Rights::WRITE.0;
    if word == 0 || word & !known != 0 {
        return Err(Error::Invalid);
    }
    Ok(Rights(word))
}

/// The system calls, by group. Numbering is not part of the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    CapDerive,
    CapMove,
    CapDelete,
    CapRevoke,
    CapInspect,
    CSpaceGrow,
    UntypedRetype,
    ASpaceMap,
    ASpaceUnmap,
    ASpaceProtect,
    Call,
    ReplyRecv,
    Recv,
    Send,
    Signal,
    Wait,
    Poll,
    NotifyBind,
    ThreadConfigure,
    ThreadReadRegs,
    ThreadWriteRegs,
    ThreadResume,
    ThreadSuspend,
    Yield,
    SchedConfigure,
    SchedBind,
    SchedUnbind,
    SchedSetCores,
    TimerSet,
    TimerCancel,
    IrqBind,
    IrqAck,
    IoSpaceMap,
    IoSpaceUnmap,
    IoSpaceAttach,
    TaskExempt,
    TaskAttach,
    TaskMark,
    TaskRevoke,
    TaskRead,
    CoreStart,
    CoreStop,
    CoreStatus,
    DebugWrite,
    TraceEmit,
    /// Call reference, one core, Threads: ends the wait of a thread with
    /// `cancelled`. It belongs to the group of the threads; it is last here
    /// so that the calls that were there before keep their place.
    ThreadCancel,
    IrqClaim,
    SchedRead,
}

/// Header word of a message: label (bits 0-31, opaque to the kernel), number
/// of words (32-38), number of capabilities (39-41), flags (42-63).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header(u64);

impl Header {
    /// Set by the kernel on the receiver's side when a capability transfer
    /// was dropped for lack of slots.
    pub const FLAG_CAPS_DROPPED: u32 = 1 << 0;
    /// Set by the kernel when `recv` returns a notification of the bound
    /// `notify` instead of a message; the badge field then holds the word.
    pub const FLAG_NOTIFICATION: u32 = 1 << 1;
    /// Set by the sender, the only flag it may set: the message page holds
    /// marks added to the stamp of this message (Tasks and marks).
    pub const FLAG_ADD_MARKS: u32 = 1 << 2;
    const FLAG_BITS: u32 = 22;

    /// `Invalid` if the message is larger than the interface allows.
    pub const fn new(label: u32, words: usize, caps: usize, flags: u32) -> Result<Self, Error> {
        if words > MAX_WORDS || caps > MAX_CAPS || flags >> Self::FLAG_BITS != 0 {
            return Err(Error::Invalid);
        }
        Ok(Self(
            label as u64 | (words as u64) << 32 | (caps as u64) << 39 | (flags as u64) << 42,
        ))
    }

    /// Checks the limits of a raw word: the number of words and of
    /// capabilities. It accepts every flag, since the kernel sets flags on
    /// the receiver's side; a header given by a sender may only have
    /// `FLAG_ADD_MARKS`, which the caller of this function checks.
    pub const fn from_raw(raw: u64) -> Result<Self, Error> {
        let h = Self(raw);
        if h.words() > MAX_WORDS || h.caps() > MAX_CAPS {
            return Err(Error::Invalid);
        }
        Ok(h)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn label(self) -> u32 {
        self.0 as u32
    }

    pub const fn words(self) -> usize {
        (self.0 >> 32 & 0x7f) as usize
    }

    pub const fn caps(self) -> usize {
        (self.0 >> 39 & 0x7) as usize
    }

    pub const fn flags(self) -> u32 {
        (self.0 >> 42) as u32
    }

    /// A message that fits in registers, carries no capability and has no
    /// flag.
    pub const fn is_fast(self) -> bool {
        self.words() <= REG_WORDS && self.caps() == 0 && self.flags() == 0
    }
}

/// What the kernel sets on every received message. The sender cannot forge
/// any of it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    pub header: Header,
    /// Badge of the endpoint capability the sender used.
    pub badge: u64,
    /// The effective marks of the sender, and the marks it added.
    pub marks: Marks,
    /// Identifier of the sender's task, for the log.
    pub task: u64,
}

/// The kinds of fault, which are the labels of the fault message (Faults).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    Page = 1,
    Undefined = 2,
    Arithmetic = 3,
    Protection = 4,
    Debug = 5,
    BadCall = 6,
    NoAddressSpace = 7,
    NoContext = 8,
}

/// Words of a fault message.
pub const FAULT_WORDS: usize = 5;
/// Labels of the answer to a fault: resume at word 0 of the fault, or with
/// the instruction and stack pointers of words 0 and 1 of the answer. Any
/// other label leaves the thread suspended.
pub const FAULT_REPLY_RESUME: u32 = 0;
pub const FAULT_REPLY_REDIRECT: u32 = 1;
/// The access bits of word 3 of a page fault.
pub const ACCESS_READ: u64 = 1 << 0;
pub const ACCESS_WRITE: u64 = 1 << 1;
pub const ACCESS_EXEC: u64 = 1 << 2;
pub const ACCESS_PRESENT: u64 = 1 << 3;

/// Every instant and every duration given to a call is below this (Clock).
pub const TIME_LIMIT: u64 = 1 << 63;
pub const SCHED_MIN_BUDGET_NS: u64 = 10_000;
pub const SCHED_MAX_PERIOD_NS: u64 = 1 << 36;
pub const SCHED_CLASS_FRAME: u64 = 0;
pub const SCHED_CLASS_INTERACTIVE: u64 = 1;
pub const SCHED_CLASS_BACKGROUND: u64 = 2;
/// The sum of the bandwidths of one core, at most (Admission).
pub const ADMISSION_WHOLE_CORE: u64 = 1 << 32;
/// The windows of 64 cores that `sched_set_cores` accepts.
pub const CORE_WINDOWS: u64 = 1024;
pub const CORE_WINDOW_BITS: u32 = 64;
/// Due events processed by one timer interrupt, at most.
pub const TIME_EVENT_STEP: usize = 16;
/// Budget and period of the scheduling context of the first thread.
pub const START_SCHED_PERIOD_NS: u64 = 10_000_000;
/// The bits of the state word of `sched_read`.
pub const SCHED_STATE_CONFIGURED: u64 = 1 << 0;
pub const SCHED_STATE_THROTTLED: u64 = 1 << 1;
pub const SCHED_STATE_HOME: u64 = 1 << 2;
pub const SCHED_STATE_LENT: u64 = 1 << 3;

/// Admission: the bandwidth of a context, ⌈budget × 2^32 / period⌉, and 0
/// for a period of 0 (a context that is not configured). The period is
/// below 2^48 (a valid one is at most 2^36), and the budget at most the
/// period.
///
/// It is a long division by 64-bit words, 16 bits at a time after the
/// whole part, rather than one division of 128 bits, which Kani cannot
/// prove in reasonable time.
pub const fn bandwidth(budget: u64, period: u64) -> u64 {
    if period == 0 {
        return 0;
    }
    assert!(period < 1 << 48 && budget <= period);
    let whole = budget / period;
    let high = (budget % period) << 16;
    let low = (high % period) << 16;
    let quotient = whole << 32 | (high / period) << 16 | (low / period);
    if low.is_multiple_of(period) {
        quotient
    } else {
        quotient + 1
    }
}

/// Timers: the firing instant of a timer. With `x` the smaller of deadline +
/// tolerance and 2^63 − 1, it is the number between the deadline and `x`
/// that has the most trailing zero bits, and 0 for deadline 0.
pub const fn firing_instant(deadline: u64, tolerance: u64) -> u64 {
    if deadline == 0 {
        return 0;
    }
    let sum = deadline.saturating_add(tolerance);
    let x = if sum < TIME_LIMIT - 1 {
        sum
    } else {
        TIME_LIMIT - 1
    };
    let differ = (deadline - 1) ^ x;
    // The index of the highest bit in which deadline − 1 and x differ;
    // they differ, since x ≥ deadline.
    let m = 63 - differ.leading_zeros();
    x >> m << m
}

/// The clock page that the kernel writes and every address space maps read
/// only (Clock). A reader reads `seq`, the parameters, the counter, then
/// `seq` again, and starts over if it was odd or changed.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockPage {
    pub version: u32,
    /// Odd while the kernel writes.
    pub seq: u32,
    pub shift: u32,
    /// Reserved, 0.
    pub flags: u32,
    pub mult: u64,
    pub counter_base: u64,
    pub ns_base: u64,
}

impl ClockPage {
    /// Nanoseconds at a counter value: `ns_base + ((counter − counter_base)
    /// × mult >> shift)`, the product taken in 128 bits.
    pub const fn nanoseconds(&self, counter: u64) -> u64 {
        let elapsed = counter.wrapping_sub(self.counter_base) as u128;
        let scaled = (elapsed * self.mult as u128) >> self.shift;
        self.ns_base.wrapping_add(scaled as u64)
    }
}

/// The source word of `irq_claim` on x86-64 (Interrupts). Bits 0-7 are the
/// kind: 1 an MSI or MSI-X message, 2 a line.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqSource(u64);

impl IrqSource {
    pub const KIND_MSI: u64 = 1;
    pub const KIND_LINE: u64 = 2;
    const LEVEL: u64 = 1 << 8;
    const ACTIVE_LOW: u64 = 1 << 9;

    /// An MSI or MSI-X message of the function named by its requester ID.
    /// The device is below 32 and the function below 8.
    pub const fn msi(segment: u16, bus: u8, device: u8, function: u8) -> Self {
        assert!(device < 32 && function < 8);
        let id =
            (segment as u64) << 16 | (bus as u64) << 8 | (device as u64) << 3 | function as u64;
        Self(id << 32 | Self::KIND_MSI)
    }

    /// A line of an IOAPIC, by its global system interrupt.
    pub const fn line(gsi: u32, level: bool, active_low: bool) -> Self {
        let mut raw = (gsi as u64) << 32 | Self::KIND_LINE;
        if level {
            raw |= Self::LEVEL;
        }
        if active_low {
            raw |= Self::ACTIVE_LOW;
        }
        Self(raw)
    }

    /// `Invalid` for a kind other than 1 or 2, a reserved bit set, or bits
    /// 8-9 set for an MSI.
    pub const fn from_raw(raw: u64) -> Result<Self, Error> {
        let reserved = raw >> 10 & 0x3f_ffff != 0;
        let valid = match raw & 0xff {
            Self::KIND_MSI => raw & (Self::LEVEL | Self::ACTIVE_LOW) == 0,
            Self::KIND_LINE => true,
            _ => false,
        };
        if reserved || !valid {
            return Err(Error::Invalid);
        }
        Ok(Self(raw))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_line(self) -> bool {
        self.0 & 0xff == Self::KIND_LINE
    }

    /// The requester ID of an MSI, or the GSI of a line.
    pub const fn id(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn segment(self) -> u16 {
        (self.0 >> 48) as u16
    }

    pub const fn bus(self) -> u8 {
        (self.0 >> 40) as u8
    }

    pub const fn device(self) -> u8 {
        (self.0 >> 35) as u8 & 0x1f
    }

    pub const fn function(self) -> u8 {
        (self.0 >> 32) as u8 & 0x7
    }

    pub const fn level(self) -> bool {
        self.0 & Self::LEVEL != 0
    }

    pub const fn active_low(self) -> bool {
        self.0 & Self::ACTIVE_LOW != 0
    }
}

/// The flags of `task_read`.
pub struct TaskFlags;

impl TaskFlags {
    pub const AWARE: u64 = 1;
    pub const REVOKED: u64 = 2;
}

/// A task identifier: the node index in bits 48-63, a counter of that node
/// in bits 0-47. Identifier 0 means "no task".
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId(u64);

impl TaskId {
    pub const NONE: Self = Self(0);

    /// The counter is below 2^48.
    pub const fn new(node: u16, counter: u64) -> Self {
        assert!(counter < 1 << 48);
        Self((node as u64) << 48 | counter)
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn node(self) -> u16 {
        (self.0 >> 48) as u16
    }

    pub const fn counter(self) -> u64 {
        self.0 & ((1 << 48) - 1)
    }
}

/// A node and its generation, carried by a reference to an object of
/// another node: generation in bits 0-31, node in bits 32-47, bits 48-63
/// zero.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeGen(u64);

impl NodeGen {
    pub const fn new(node: u16, generation: u32) -> Self {
        Self((node as u64) << 32 | generation as u64)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn node(self) -> u16 {
        (self.0 >> 32) as u16
    }

    pub const fn generation(self) -> u32 {
        self.0 as u32
    }
}

/// Proofs checked by Kani for every input, not for chosen examples.
#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    fn header_new_refuses_or_reads_back() {
        let (label, words, caps, flags): (u32, usize, usize, u32) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        match Header::new(label, words, caps, flags) {
            Ok(h) => {
                assert!(h.label() == label && h.words() == words);
                assert!(h.caps() == caps && h.flags() == flags);
                assert!(Header::from_raw(h.raw()) == Ok(h));
            }
            Err(_) => assert!(words > MAX_WORDS || caps > MAX_CAPS || flags >> 22 != 0),
        }
    }

    #[kani::proof]
    fn header_from_raw_never_exceeds_the_limits() {
        let raw: u64 = kani::any();
        if let Ok(h) = Header::from_raw(raw) {
            assert!(h.words() <= MAX_WORDS && h.caps() <= MAX_CAPS && h.raw() == raw);
        }
    }

    #[kani::proof]
    fn slot_address_reads_back() {
        let (via, index): (u32, u32) = (kani::any(), kani::any());
        let a = SlotAddr::through(via, index);
        assert!(a.via() == via && a.index() == index);
        assert!(SlotAddr::from_raw(a.raw()) == a);
        assert!(a.is_none() == (via == 0 && index == 0));
    }

    #[kani::proof]
    fn accepted_mapping_lies_below_the_limit_and_holds_at_most_512_pages() {
        let (address, first, count, pages, limit): (u64, u64, u64, u64, u64) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        let page = match kani::any::<u8>() % 3 {
            0 => PageSize::Size4K,
            1 => PageSize::Size2M,
            _ => PageSize::Size1G,
        };
        kani::assume(pages >= 1 && pages <= FRAME_MAX_PAGES);
        if mapping_range(address, first, count, pages, page, limit).is_ok() {
            assert!(count >= 1 && count <= FRAME_MAX_PAGES && first + count <= pages);
            assert!(address % page.bytes() == 0);
            let end = address as u128 + count as u128 * page.bytes() as u128;
            assert!(end <= limit as u128);
        }
    }

    #[kani::proof]
    fn firing_instant_is_in_its_window_with_the_most_trailing_zeros() {
        let (deadline, tolerance, other): (u64, u64, u64) = (kani::any(), kani::any(), kani::any());
        kani::assume(deadline < TIME_LIMIT);
        let f = firing_instant(deadline, tolerance);
        if deadline == 0 {
            assert!(f == 0);
            return;
        }
        let x = core::cmp::min(deadline.saturating_add(tolerance), TIME_LIMIT - 1);
        assert!(deadline <= f && f <= x);
        if deadline <= other && other <= x {
            assert!(other.trailing_zeros() <= f.trailing_zeros());
        }
    }

    #[kani::proof]
    fn bandwidth_is_at_most_a_whole_core_when_the_budget_fits_the_period() {
        let (budget, period): (u64, u64) = (kani::any(), kani::any());
        kani::assume(budget <= period && period <= SCHED_MAX_PERIOD_NS);
        let u = bandwidth(budget, period);
        assert!(u <= ADMISSION_WHOLE_CORE);
        assert!((budget == 0) == (u == 0));
    }

    #[kani::proof]
    fn irq_source_reads_back_what_the_constructors_write() {
        let (segment, bus, device, function): (u16, u8, u8, u8) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(device < 32 && function < 8);
        let msi = IrqSource::msi(segment, bus, device, function);
        assert!(IrqSource::from_raw(msi.raw()) == Ok(msi));
        assert!(!msi.is_line() && msi.segment() == segment && msi.bus() == bus);
        assert!(msi.device() == device && msi.function() == function);
        let (gsi, level, low): (u32, bool, bool) = (kani::any(), kani::any(), kani::any());
        let line = IrqSource::line(gsi, level, low);
        assert!(IrqSource::from_raw(line.raw()) == Ok(line));
        assert!(line.is_line() && line.id() == gsi);
        assert!(line.level() == level && line.active_low() == low);
    }
}

#[cfg(test)]
mod tests;
