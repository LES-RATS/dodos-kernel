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

mod debug;
mod features;
mod machine;
mod ring;
mod start;

pub mod aarch64;
pub mod riscv64;
pub mod x86_64;

pub use debug::{
    Build, CONSOLE_STEP_BYTES, CONSOLE_TICK_NS, CORE_CRASH_RECORD_BYTES, CRASH_AREA_MAGIC,
    CRASH_AREA_VERSION, CRASH_HEADER_BYTES, CRASH_RECORD_KERNEL_ENTRIES, CRASH_RECORD_USER_ENTRIES,
    ConsoleKind, ConsoleMessage, CoreCrashRecord, CrashAreaHeader, CrashReason, DEBUG_AREA_BYTES,
    DEBUG_AREA_MAGIC, DEBUG_AREA_VERSION, DEBUG_HEADER_BYTES, DEBUG_WRITE_MAX, DebugAreaHeader,
    STUB_STOP_NS, StopRecord, TRACE_BURST, TRACE_BURST_NS, TRACE_ENTRY_BYTES, TRACE_FLAG_CONTINUED,
    TRACE_KERNEL_ENTRIES, TRACE_KERNEL_OFFSET, TRACE_SLOT_MASK, TRACE_SLOT_SHIFT, TRACE_TEXT_BYTES,
    TRACE_USER_ENTRIES, TRACE_USER_OFFSET, TraceCode, TraceEntry, trace_index,
};
pub use features::{
    FEATURE_INSTRUCTION_FIRST, FEATURE_USER_INTERRUPTS, FeatureList, FeatureMask, Kind,
    kind_covers, thread_bytes,
};
pub use machine::{
    BarrierView, BootFloors, BootMembership, BootNode, BootNodes, CHECK_MUL, CORE_ANSWER_NS,
    CORE_ANSWER_RANGE_NS, CORE_EVENT_FEEDBACK, CORE_MARK, CORE_NO_SLOT, CORE_UNUSABLE,
    CORES_PER_KERNEL_MAX, CoreState, CoreWord, DEVICE_TABLE_SPARE, DRIVER_NONE, DeviceEntry,
    Evidence, FENCE_CLASS_COOPERATIVE, FENCE_CLASS_FORCED, FENCE_CLASS_SIGNALLED, FENCE_NS,
    FENCE_RANGE_NS, FENCE_RECORD_BYTES, FENCE_RECORD_CORES, FenceRecord, Floors, GRANT_ENTRY_BYTES,
    GRANTS, GateWord, Governs, IDENTIFIERS_PER_SEGMENT, IDLE_ENTRY_WORDS, IDLE_FLAG_LOSES_CONTEXT,
    IDLE_METHOD_HALT, IDLE_METHOD_MWAIT, IDLE_METHOD_PSCI, IDLE_METHOD_SBI, IDLE_STATES_MAX,
    IoPortRange, Leaf, MAP_COUNT_MAX, Member, MemberState, ModuleFrame, ModuleFrames, NMI_GRACE_NS,
    NODE_IMAGE_BOOT, NODE_IMAGE_REPLACEMENT, NODE_START_FRESH, NODE_START_RESUME,
    NODE_STATUS_CLASS_SHIFT, NODE_STATUS_CORES_MASK, NODE_STATUS_UNUSABLE, NODE_STATUS_WORDS,
    NODE_STOP_KILL, NODE_STOP_RETIRE, NODE_STOP_SEAL, NODE_WATCH_SLOTS, NODES_MAX,
    OBSERVATION_LIFE, OBSERVATIONS_MAX, OWNER_RESERVE_BASE, OWNER_UNCLAIMED, OwnState, Owner,
    Ownership, PageClass, RECOVERY_BANDWIDTH, RECOVERY_PERIOD_NS, RESERVES_MAX, RESERVES_MIN,
    ROLE_EPOCH_MASK, ROLE_FAILED, ROLE_NOBODY, ROLE_PLAIN_PLATFORM, ROLES_PLAIN_MAX, RemoteRef,
    RoleKind, RoleState, RoleWord, SEAL_NS, SEAL_RANGE_NS, SETTLE_NS, SLOT_BY_KIND_SHIFT,
    SLOT_DEAD, SLOT_DEPTH_SHIFT, SLOT_DYING, SLOT_EMPTY, SLOT_FRAME_ASKED_MASK, SLOT_FRAME_DEVICE,
    SLOT_FRAME_FIRST_SHIFT, SLOT_FRAME_MEMORY_SHIFT, SLOT_FRAME_PAGE_SHIFT, SLOT_FRAME_PAGES_SHIFT,
    SLOT_FULL, SLOT_IRQ_CORE_SHIFT, SLOT_KIND_SHIFT, SLOT_RECORD_BINDING, SLOT_RECORD_EMPTY,
    SLOT_RECORD_MAPPING, SLOT_RECORD_PARTIAL, SLOT_RECORD_RIGHTS_SHIFT, SLOT_RECORD_UNMAPPING,
    SLOT_REMOTE, SLOT_REMOTE_GRANT_SHIFT, SLOT_REMOTE_SERIAL_SHIFT, SLOT_REMOTE_USE_SHIFT,
    SLOT_RIGHTS_SHIFT, SLOT_ROOT, SLOT_STATE_MASK, SLOT_UNTYPED_BLOCK, SLOT_UNTYPED_CLEAN,
    SLOT_UNTYPED_DEVICE, SLOT_UNTYPED_ENTRY, SLOT_UNTYPED_ENTRY_SHIFT, SLOT_UNTYPED_LOG2_SHIFT,
    STALE_NS, STALE_RANGE_NS, START_NS, START_RETRY_NS, STUCK_NS, SWEEP_NS, SWEEP_RANGE_NS,
    Transition, USE_ENTRY_BYTES, barrier, best_class, clear_step, clear_steps, floors,
    ioport_check, member_transition, module_frames, ports_meet, successor,
};
pub use ring::{
    CREDIT_NONE, CREDIT_SERIAL_MASK, NodeMessage, PENDING_STEP, REPORT_END, REQUESTS_PER_PEER,
    RING_ANSWER_RESERVE, RING_ENTRY_BYTES, RING_HEADER_BYTES, RING_KERNEL_ENTRIES,
    RING_MESSAGE_ENTRIES_MAX, RING_USER_ENTRIES, RING_USER_STEP, Ring, RingHeader, SEND_GRANTS_MAX,
    SENDS_PER_PEER, SENDS_RESERVED_FOR_MANAGE, UNIT_REQUESTS, UnitOperation, UnitResult,
    message_entries, ring_fits, ring_free, ring_resync,
};
pub use start::{
    START_CORE_BOOTED, START_CORE_FENCE_SHIFT, START_FREE_SLOTS, START_IRQ_DIRECTED_EOI,
    START_IRQ_REMAPPING, START_IRQ_X2APIC, START_MACHINE_LAYOUT, START_MAGIC,
    START_MEMORY_CONTROL_FLOW, START_MEMORY_GATE_NO_WINDOW, START_MEMORY_MAP_THROUGH_ROLE,
    START_MEMORY_MONITOR, START_MEMORY_UNFENCED_ACCEPTED, START_MODULE_EXEC,
    START_MODULE_FIRST_SERVICE, START_MODULE_MAPPED, START_MODULE_READ, START_MODULE_WRITE,
    START_NODE_FRESH, START_NODE_RESUMED, START_PAGE_MAX_PAGES, START_RANGE_DEVICE,
    START_RANGE_FIRMWARE, START_RANGE_KEPT_BY_STAGE, START_RANGE_UNUSABLE, START_RANGE_USABLE,
    START_SLOT_ASPACE, START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_FIRST, START_SLOT_IOPORTS,
    START_SLOT_IRQ_CONTROL, START_SLOT_MACHINE_FIXED, START_SLOT_MACHINE_LIVING,
    START_SLOT_MAILBOX, START_SLOT_MEMORY_CONTROL, START_SLOT_MESSAGE_PAGE,
    START_SLOT_MESSAGE_PAGE_MAP, START_SLOT_SCHED, START_SLOT_START_PAGE, START_SLOT_TASK,
    START_SLOT_THREAD, START_STAGE_DEVICE_TREE, START_STAGE_LIMINE, START_STAGE_OTHER,
    START_STAGE_UBIOS, START_STAGE_UEFI, START_TABLE_ACPI_ROOT, START_TABLE_DEVICE_TREE,
    START_TABLE_SMBIOS, START_TABLE_UBIOS, START_UNTYPED_DEVICE, START_VERSION, StartBootTimes,
    StartCore, StartCrash, StartDebug, StartEntry, StartFeedback, StartFirmwareTable,
    StartFramebuffer, StartHeader, StartImage, StartInterrupts, StartIoApic, StartIommu, StartKept,
    StartKind, StartKindRecord, StartMachine, StartMemory, StartMemoryRange, StartModule,
    StartNode, StartProcess, StartRole, StartShare, StartUntyped,
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
/// Slots added by one `cspace_grow`: one segment of 8 KB, a byte size that
/// does not change with the page (Values of the machine).
pub const CSPACE_SEGMENT_SLOTS: u32 = 128;
/// Units of work of one call of a removal before it returns `Again`: a
/// capability removed, a thread woken, or one link of a destroyed object
/// undone (The derivation tree, provisional).
pub const REMOVAL_STEP: usize = 32;
/// Slots of dying capability spaces examined by one call of a removal
/// before it returns `Again` (The derivation tree, provisional).
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
/// addresses): a segment's address and its sequence word, which serves the
/// lock-free slot read of a node of several cores and, in variant L2, the
/// segment's lock (D-129).
pub const CSPACE_DIRECTORY_BYTES: usize = 16;

/// Bytes that one call of a creation clears, at most (rules of
/// `untyped_retype`, the clean mark). A byte size, not a number of pages.
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
/// Bytes of a `sched` (Object sizes: the parameters in force and pending,
/// the links of the queues and of the change list, the chain, the count).
pub const SCHED_BYTES: usize = 256;
/// Bytes of the fixed part of a `thread`; the vector state area is added
/// and the sum rounded up to a power of two (Object sizes, Unlike cores).
pub const THREAD_FIXED_BYTES: u64 = 1024;
/// Pages of an `aspace` and of an `iospace`: the root table and a header
/// (Address spaces and their tables).
pub const ASPACE_PAGES: u64 = 2;
pub const IOSPACE_PAGES: u64 = 2;
/// Entries of one table page, at most: the number of pages of a frame and
/// the fields of a mapping record have 12 bits (Values of the machine).
pub const TABLE_ENTRIES_MAX: u32 = 4096;
/// Levels of the tables of an `aspace`, at least (Values of the machine).
pub const LEVELS_MIN: u32 = 2;
/// Devices that one `iospace` serves, at most.
pub const IOSPACE_MAX_DEVICES: usize = 16;
/// Nanoseconds within which an IOMMU unit confirms an invalidation.
pub const IOMMU_WAIT_NS: u64 = 1_000_000;
/// Bytes of `entropy_read`, at most (Entropy).
pub const ENTROPY_MAX_BYTES: u64 = 64;

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
    /// A right is missing; or the call asks for more than a capability
    /// allows (rights that are not a subset, another badge); or a state
    /// check that guards a rule of the gate refuses it (the class of a page
    /// a mapping targets, the owner of a device identifier). Order of checks.
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
    /// A machine object: one node slot (called `core` before D-128).
    Node = 14,
    Debug = 15,
    /// Reserved, not built.
    Vcpu = 16,
    IrqControl = 17,
    /// A machine object: one IOMMU unit.
    Iommu = 18,
    /// The authority to take, give and split the blocks of one node.
    MemoryControl = 19,
    /// A machine object: one duty with one holder at a time.
    Role = 20,
}

impl ObjectKind {
    /// The kinds, by number: the entry `ObjectSizes` covers them all.
    pub const ALL: [Self; 21] = [
        Self::Untyped,
        Self::CSpace,
        Self::ASpace,
        Self::Frame,
        Self::Thread,
        Self::Sched,
        Self::Endpoint,
        Self::Reply,
        Self::Notify,
        Self::Timer,
        Self::Irq,
        Self::IoPorts,
        Self::IoSpace,
        Self::Task,
        Self::Node,
        Self::Debug,
        Self::Vcpu,
        Self::IrqControl,
        Self::Iommu,
        Self::MemoryControl,
        Self::Role,
    ];
}

/// A page size, by its index (Values of the machine): 0 the page, 1 the
/// first large page, 2 the second, 3 the third. Its bytes are a value of the
/// machine, read from the entry `Memory` of the start-up page
/// ([`Geometry::bytes`]).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageSize {
    Page = 0,
    Large1 = 1,
    Large2 = 2,
    Large3 = 3,
}

impl PageSize {
    pub const ALL: [Self; 4] = [Self::Page, Self::Large1, Self::Large2, Self::Large3];

    /// `None` for a value that names no page size.
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Page),
            1 => Some(Self::Large1),
            2 => Some(Self::Large2),
            3 => Some(Self::Large3),
            _ => None,
        }
    }
}

/// The values of the machine that fix the shape of memory (Values of the
/// machine), as the entry `Memory` of the start-up page publishes them.
/// The default is all zero, which [`Geometry::valid`] refuses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    /// The log2 of each page size, by index; 0 where the processor has none.
    pub page_shift: [u8; 4],
    /// *E*, the entries of one table page.
    pub entries: u32,
    /// *L*, the levels of the tables of an `aspace`.
    pub levels: u32,
    /// The first address beyond the user range.
    pub user_end: u64,
}

impl Geometry {
    /// The bytes of a page size, `None` where the processor has none.
    pub const fn bytes(&self, size: PageSize) -> Option<u64> {
        match self.page_shift[size as usize] {
            0 => None,
            shift => Some(1 << shift),
        }
    }

    /// *P*, the bytes of a page.
    pub const fn page(&self) -> u64 {
        1 << self.page_shift[0]
    }

    /// The log2 of the chunk: the first large page (Chunks).
    pub const fn chunk_shift(&self) -> u32 {
        self.page_shift[1] as u32
    }

    pub const fn chunk(&self) -> u64 {
        1 << self.chunk_shift()
    }

    /// The bit set of the supported page sizes, bit `i` for index `i`.
    pub const fn sizes(&self) -> u32 {
        let mut bits = 0;
        let mut i = 0;
        while i < 4 {
            if self.page_shift[i] != 0 {
                bits |= 1 << i;
            }
            i += 1;
        }
        bits
    }

    /// The bits of a virtual address the tables translate: the page and
    /// log2 *E* per level.
    pub const fn address_bits(&self) -> u32 {
        self.page_shift[0] as u32 + self.levels * self.entries.trailing_zeros()
    }

    /// The bytes one entry of the root table maps: the reserved slot is the
    /// range of one such entry (Address spaces and their tables).
    pub const fn root_entry(&self) -> u64 {
        1 << (self.page_shift[0] as u32 + (self.levels - 1) * self.entries.trailing_zeros())
    }

    /// Whether an address is canonical: its bits from the top bit the
    /// tables translate (`address_bits` − 1) to bit 63 are all equal
    /// (x86-64: bits 47-63 on four levels). It depends on the address bits
    /// of the geometry, never on the user range end (DeepSeek F1 of the
    /// seventh review).
    pub const fn canonical(&self, address: u64) -> bool {
        let top = address >> (self.address_bits() - 1);
        top == 0 || top == u64::MAX >> (self.address_bits() - 1)
    }

    /// Whether the values are ones the kernel starts with (Values of the
    /// machine, Start-up state): a page of at least 4 KB; *E* the entries
    /// of one table page of 8-byte entries, at most 4,096 (so a 64 KB
    /// granule, 8,192 entries, is refused); *L* from 2 to 8; page sizes one
    /// per level, each the size one entry of the next level up maps
    /// (log2 *E* above the one before), with none after a missing one, a
    /// chunk (the second size) required, and none larger than *E* chunks,
    /// so that one leaf fits one call of the gate (G3); a page size of
    /// index *i* is a leaf of level *i* + 1, so *i* < *L*; a user range end
    /// that is a multiple of the chunk and leaves, below the half of the
    /// address bits the tables translate, the whole root entry of the
    /// reserved slot.
    pub const fn valid(&self) -> bool {
        let s = self.page_shift;
        if s[0] < 12 || s[0] > 32 || self.entries == 0 || !self.entries.is_power_of_two() {
            return false;
        }
        let per_level = self.entries.trailing_zeros() as u8;
        let mut sizes = s[1] == s[0] + per_level && self.levels > 1;
        let mut i = 2;
        while i < 4 {
            // Present only after a present size, one level up, below the
            // level of the root, and at most E chunks (index 2 at most).
            if s[i] != 0 && (s[i - 1] == 0 || s[i] != s[i - 1] + per_level || i > 2) {
                sizes = false;
            }
            if s[i] != 0 && i as u32 >= self.levels {
                sizes = false;
            }
            i += 1;
        }
        sizes
            && self.entries as u64 == 1 << (s[0] - 3)
            && self.entries <= TABLE_ENTRIES_MAX
            && self.levels >= LEVELS_MIN
            && self.levels <= 8
            && self.address_bits() <= 64
            && self.user_end != 0
            && self.user_end.is_multiple_of(self.chunk())
            && self.user_end <= (1 << (self.address_bits() - 1)) - self.root_entry()
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
/// address is a multiple of the page size (`page` bytes, a power of two);
/// the first page is in the frame; the page count is at least 1 and does
/// not run past the frame; and the range lies below `limit`, computed
/// without overflow. `Invalid` otherwise, in that order. A frame has at most
/// *E* pages, so an accepted range holds at most *E* pages.
pub const fn mapping_range(
    address: u64,
    first: u64,
    count: u64,
    frame_pages: u64,
    page: u64,
    limit: u64,
) -> Result<(), Error> {
    if !address.is_multiple_of(page) || first >= frame_pages {
        return Err(Error::Invalid);
    }
    if count == 0 || count > frame_pages - first {
        return Err(Error::Invalid);
    }
    match count.checked_mul(page) {
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
    NodeStart,
    NodeStop,
    NodeStatus,
    DebugWrite,
    TraceEmit,
    /// Call reference, one core, Threads: ends the wait of a thread with
    /// `cancelled`. It belongs to the group of the threads; it is last here
    /// so that the calls that were there before keep their place.
    ThreadCancel,
    IrqClaim,
    SchedRead,
    MemoryTake,
    MemoryGive,
    MemorySplit,
    NodeIdle,
    NodeWatch,
    /// Between nodes: the stamped send into another node's mailbox, at the
    /// place of `core_place`, removed by D-128.
    NodeSend,
    RoleClaim,
    RoleHand,
    ThreadSetFeatures,
    /// Between nodes: the doorbell of a grant, at the place of
    /// `thread_move`, removed by D-128.
    GrantSignal,
    SchedCount,
    IoportIn,
    IoportOut,
    IoportsIssue,
    EntropyRead,
    NodePerf,
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
    /// A breakpoint or watchpoint of the thread's debug registers; word 3
    /// holds the bits of those that hit, word 4 the status (x86-64: DR6).
    HardwareBreakpoint = 9,
}

impl FaultKind {
    /// `None` for a label that names no kind.
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Page),
            2 => Some(Self::Undefined),
            3 => Some(Self::Arithmetic),
            4 => Some(Self::Protection),
            5 => Some(Self::Debug),
            6 => Some(Self::BadCall),
            7 => Some(Self::NoAddressSpace),
            8 => Some(Self::NoContext),
            9 => Some(Self::HardwareBreakpoint),
            _ => None,
        }
    }
}

/// What an exception taken from a user thread is (Faults), as the tables of
/// each architecture classify it (`aarch64::ec_use`, `riscv64::cause_use`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionUse {
    /// A fault of this kind, delivered on the thread's fault endpoint.
    Fault(FaultKind),
    /// A system call: a call if it comes from the kernel-supplied code page,
    /// a fault *bad call* otherwise.
    Call,
    /// An access to the state of feature `bit` of the architecture's list
    /// (Unlike cores): an undefined instruction, with word 4 = 1 + `bit`,
    /// when the thread's mask lacks the feature; otherwise the kernel's own
    /// (it loads the state and the thread resumes).
    Feature(u32),
    /// The kernel's own: the thread resumes, as for the vector state on
    /// x86-64 (#NM).
    Own,
    /// No exception of the thread: a bug of the kernel (it accuses itself,
    /// evidence self) or a hardware error, handled by the recovery of the
    /// node.
    NotThread,
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
/// Period and budget of the scheduling context of the first thread: 63/64
/// of the core, the recovery reserve holding the other 1/64 (Start-up
/// state).
pub const START_SCHED_PERIOD_NS: u64 = 10_000_000;
pub const START_SCHED_BUDGET_NS: u64 = 9_843_750;
/// The bits of the state word of `sched_read`.
pub const SCHED_STATE_CONFIGURED: u64 = 1 << 0;
pub const SCHED_STATE_THROTTLED: u64 = 1 << 1;
pub const SCHED_STATE_HOME: u64 = 1 << 2;
pub const SCHED_STATE_LENT: u64 = 1 << 3;
/// Counting is on (`sched_count`).
pub const SCHED_STATE_COUNTED: u64 = 1 << 4;
/// Bits 8-15: 1 + the last hardware class, or 0 (Unlike cores).
pub const SCHED_STATE_HW_CLASS_SHIFT: u32 = 8;
pub const SCHED_STATE_HW_CLASS_MASK: u64 = 0xFF << 8;
/// The modes of `sched_count`.
pub const SCHED_COUNT_OFF: u64 = 0;
pub const SCHED_COUNT_ON: u64 = 1;

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
    /// A core event (Unlike cores): bits 8-15 the event, every other bit 0.
    pub const KIND_CORE_EVENT: u64 = 3;
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

    /// A core event of the node's core (Unlike cores).
    pub const fn core_event(event: u8) -> Self {
        Self((event as u64) << 8 | Self::KIND_CORE_EVENT)
    }

    /// `Invalid` for a kind other than 1, 2 or 3, a reserved bit set, bits
    /// 8-9 set for an MSI, or any bit but the event set for a core event.
    pub const fn from_raw(raw: u64) -> Result<Self, Error> {
        let reserved = raw >> 10 & 0x3f_ffff != 0;
        let valid = match raw & 0xff {
            Self::KIND_MSI => !reserved && raw & (Self::LEVEL | Self::ACTIVE_LOW) == 0,
            Self::KIND_LINE => !reserved,
            Self::KIND_CORE_EVENT => raw >> 16 == 0,
            _ => false,
        };
        if !valid {
            return Err(Error::Invalid);
        }
        Ok(Self(raw))
    }

    pub const fn is_core_event(self) -> bool {
        self.0 & 0xff == Self::KIND_CORE_EVENT
    }

    /// The event of a core event.
    pub const fn event(self) -> u8 {
        (self.0 >> 8) as u8
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

    /// A level line; MSI messages and core events are edge sources.
    pub const fn level(self) -> bool {
        self.is_line() && self.0 & Self::LEVEL != 0
    }

    pub const fn active_low(self) -> bool {
        self.is_line() && self.0 & Self::ACTIVE_LOW != 0
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

    /// Whether `value` is one of `list`, by a loop Kani unwinds (a slice's
    /// `contains` goes through `memchr` for bytes).
    fn listed<const N: usize>(list: &[u64; N], value: u64) -> bool {
        let mut i = 0;
        while i < N {
            if list[i] == value {
                return true;
            }
            i += 1;
        }
        false
    }

    #[kani::proof]
    #[kani::unwind(50)]
    fn an_arm64_class_is_classified_exactly_when_the_architecture_defines_it() {
        let ec: u8 = kani::any::<u8>() & 0x3F;
        let syndrome = (ec as u64) << crate::aarch64::ESR_EC_SHIFT;
        let classes = crate::aarch64::EXCEPTION_CLASSES.map(u64::from);
        // Status code 0 of an abort is an address size fault: classified.
        let defined = listed(&classes, ec as u64);
        assert!(crate::aarch64::exception_use(syndrome).is_some() == defined);
    }

    #[kani::proof]
    #[kani::unwind(21)]
    fn a_riscv_cause_is_classified_exactly_when_the_specification_defines_it() {
        let scause: u64 = kani::any();
        let defined = listed(&crate::riscv64::CAUSES, scause);
        assert!(crate::riscv64::exception_use(scause).is_some() == defined);
    }

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
    fn accepted_mapping_lies_below_the_limit_and_holds_at_most_e_pages() {
        let (address, first, count, pages, limit): (u64, u64, u64, u64, u64) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        // The page sizes of the two geometries of the tests and their
        // first large pages.
        let shift = match kani::any::<u8>() % 4 {
            0 => 12,
            1 => 14,
            2 => 21,
            _ => 25,
        };
        let entries: u64 = kani::any();
        kani::assume(entries >= 2 && entries <= TABLE_ENTRIES_MAX as u64);
        kani::assume(pages >= 1 && pages <= entries);
        let page = 1u64 << shift;
        if mapping_range(address, first, count, pages, page, limit).is_ok() {
            assert!(count >= 1 && count <= entries && first + count <= pages);
            assert!(address % page == 0);
            let end = address as u128 + count as u128 * page as u128;
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
        let event: u8 = kani::any();
        let core = IrqSource::core_event(event);
        assert!(IrqSource::from_raw(core.raw()) == Ok(core));
        assert!(core.is_core_event() && core.event() == event && !core.level());
    }

    #[kani::proof]
    fn irq_source_accepts_only_its_three_kinds() {
        let raw: u64 = kani::any();
        if let Ok(source) = IrqSource::from_raw(raw) {
            let kind = raw & 0xff;
            assert!(kind >= 1 && kind <= 3 && source.raw() == raw);
            assert!(kind != 3 || raw >> 16 == 0);
            assert!(kind == 3 || raw >> 10 & 0x3f_ffff == 0);
        }
    }
}

#[cfg(test)]
mod tests;
