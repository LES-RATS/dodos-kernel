//! The machine table and what several nodes share (Memory and devices
//! between nodes, The gate over page tables, Cores): the capability slot,
//! the ownership words, the device table, the member and role words, the
//! numbers of grants, proxies, nodes and roles, the times of membership,
//! the calls of the cores, I/O ports, and the pure rules of the gate.

use crate::{CLEAR_STEP_BYTES, Error, Geometry, PageSize};

// The capability slot: eight words of 64 bits.

/// Word 0, bits 0-1: the state of a slot.
pub const SLOT_STATE_MASK: u64 = 0b11;
pub const SLOT_EMPTY: u64 = 0;
pub const SLOT_FULL: u64 = 1;
pub const SLOT_DEAD: u64 = 2;
/// Full, naming a grant of another node.
pub const SLOT_REMOTE: u64 = 3;
/// Word 0, bit 2: under `cap_delete`; bit 3: the root of its object.
pub const SLOT_DYING: u64 = 1 << 2;
pub const SLOT_ROOT: u64 = 1 << 3;
/// Word 0: the kind in bits 4-9, the rights in 10-17, the depth in 18-24,
/// and from bit 25 the bits that depend on the kind.
pub const SLOT_KIND_SHIFT: u32 = 4;
pub const SLOT_RIGHTS_SHIFT: u32 = 10;
pub const SLOT_DEPTH_SHIFT: u32 = 18;
pub const SLOT_BY_KIND_SHIFT: u32 = 25;
/// A frame, in word 0: the page size index (25-26), the memory type
/// (27-28), device memory (29), the number of pages − 1 (30-41), the first
/// page of the record (42-53).
pub const SLOT_FRAME_PAGE_SHIFT: u32 = 25;
pub const SLOT_FRAME_MEMORY_SHIFT: u32 = 27;
pub const SLOT_FRAME_DEVICE: u64 = 1 << 29;
pub const SLOT_FRAME_PAGES_SHIFT: u32 = 30;
pub const SLOT_FRAME_FIRST_SHIFT: u32 = 42;
/// A frame, word 2: the kind of the record (bits 0-1: 0 empty, 1 mapping,
/// 2 message page binding, 3 unmapping) and the rights of a mapping (2-4).
pub const SLOT_RECORD_EMPTY: u64 = 0;
pub const SLOT_RECORD_MAPPING: u64 = 1;
pub const SLOT_RECORD_BINDING: u64 = 2;
/// Record kind 3, *unmapping*: a mapping whose entries are cleared and
/// whose invalidation on other cores of the node, or by an IOMMU unit, is
/// not complete (D-131).
pub const SLOT_RECORD_UNMAPPING: u64 = 3;
/// An `irq`, word 1: the number of its core in bits 16-31, the index of its
/// entry in that core's interrupt table in bits 0-15 (D-129).
pub const SLOT_IRQ_CORE_SHIFT: u32 = 16;
pub const SLOT_RECORD_RIGHTS_SHIFT: u32 = 2;
/// An untyped, in word 0: the log2 of its size (25-30), device memory (31),
/// a clean mark (32), a block (33), the block is a device entry (34), the
/// index of that entry (35-50).
pub const SLOT_UNTYPED_LOG2_SHIFT: u32 = 25;
pub const SLOT_UNTYPED_DEVICE: u64 = 1 << 31;
pub const SLOT_UNTYPED_CLEAN: u64 = 1 << 32;
pub const SLOT_UNTYPED_BLOCK: u64 = 1 << 33;
pub const SLOT_UNTYPED_ENTRY: u64 = 1 << 34;
pub const SLOT_UNTYPED_ENTRY_SHIFT: u32 = 35;
/// An untyped's root slot, word 6: 0, or the kernel epoch at which its
/// watermark became ready to return, raised by the destruction of its last
/// object; it returns once a barrier covers that epoch (A node of several
/// cores, D-135, D-138).
pub const SLOT_UNTYPED_READY_WORD: usize = 6;
/// A remote capability, word 1: the granting node (0-9), the grant index
/// (10-21), the serial (22-51), the index of its use entry (52-63).
pub const SLOT_REMOTE_GRANT_SHIFT: u32 = 10;
pub const SLOT_REMOTE_SERIAL_SHIFT: u32 = 22;
pub const SLOT_REMOTE_USE_SHIFT: u32 = 52;

/// Word 1 of a remote capability.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteRef(u64);

impl RemoteRef {
    /// The node is below 1,024, the grant and the use entry below 4,096,
    /// the serial below 2^30.
    pub const fn new(node: u16, grant: u16, serial: u32, use_entry: u16) -> Self {
        assert!((node as usize) < NODES_MAX && (grant as usize) < GRANTS);
        assert!(serial < 1 << 30 && (use_entry as usize) < GRANTS);
        Self(
            node as u64
                | (grant as u64) << SLOT_REMOTE_GRANT_SHIFT
                | (serial as u64) << SLOT_REMOTE_SERIAL_SHIFT
                | (use_entry as u64) << SLOT_REMOTE_USE_SHIFT,
        )
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn node(self) -> u16 {
        (self.0 & 0x3ff) as u16
    }

    pub const fn grant(self) -> u16 {
        (self.0 >> SLOT_REMOTE_GRANT_SHIFT & 0xfff) as u16
    }

    pub const fn serial(self) -> u32 {
        (self.0 >> SLOT_REMOTE_SERIAL_SHIFT & 0x3fff_ffff) as u32
    }

    pub const fn use_entry(self) -> u16 {
        (self.0 >> SLOT_REMOTE_USE_SHIFT) as u16
    }
}

// Numbers of the multi-node part (provisional).

/// Nodes, at most: kept memory is O(n²) through `acked`.
pub const NODES_MAX: usize = 1024;
/// Entries of the grant table and of the use table of each node.
pub const GRANTS: usize = 4096;
/// The largest map count of a chunk: the count is 32 bits of its gate word
/// (G4); a mapping that would pass it is `busy`.
pub const MAP_COUNT_MAX: u64 = u32::MAX as u64;
/// Identifier words of each PCI segment an IOMMU unit serves: one per bus,
/// device and function (IOMMU).
pub const IDENTIFIERS_PER_SEGMENT: usize = 65_536;
/// Line words at most: one ownership word per claimable line of the machine
/// (x86-64: per GSI an IOAPIC serves), in the living part; a line beyond the
/// last word is `unsupported` for `irq_claim` (Interrupts, D-135).
pub const IRQ_LINES_MAX: usize = 4_096;
/// Bit 5 of word 2 of a frame slot: the mapping its record holds is partial,
/// made in steps (G3).
pub const SLOT_RECORD_PARTIAL: u64 = 1 << 5;
/// Word 1 of a frame slot whose record is a partial mapping, bits 0-11: the
/// page count the call asked for, − 1 (G3: the call goes on only with the
/// same count). The frame's address is page-aligned, so these bits are
/// otherwise 0.
pub const SLOT_FRAME_ASKED_MASK: u64 = 0xfff;

/// The class of a page (The gate over page tables, G2), one byte per page
/// of ordinary memory: 0 data, 1 gate, 2 global read-only, 3 device, 16 + k
/// a page table of level k, 32 + k an iospace table of level k, 48 + n an
/// iospace root with n devices attached (D-137; 4, *iospace table*, is no
/// longer a class).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageClass(u8);

/// What a leaf asks of the page it targets (G3 (4)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub write: bool,
    pub exec: bool,
    /// The leaf carries a device memory type (uncached or write-combining).
    pub device_type: bool,
}

impl PageClass {
    pub const DATA: Self = Self(0);
    pub const GATE: Self = Self(1);
    pub const GLOBAL_READ_ONLY: Self = Self(2);
    pub const DEVICE: Self = Self(3);

    /// A page table of level `level`, 1 (its entries are pages) to 8.
    pub const fn table(level: u8) -> Option<Self> {
        match level {
            1..=8 => Some(Self(16 + level)),
            _ => None,
        }
    }

    /// An iospace table of level `level`, 1 (its entries are pages) to 8,
    /// below its root (D-137).
    pub const fn iospace_table(level: u8) -> Option<Self> {
        match level {
            1..=8 => Some(Self(32 + level)),
            _ => None,
        }
    }

    /// The root of an iospace with `devices` devices pinned or attached to
    /// it, 0 to 16 (D-137).
    pub const fn iospace_root(devices: u8) -> Option<Self> {
        match devices {
            0..=16 => Some(Self(48 + devices)),
            _ => None,
        }
    }

    /// The level of an iospace table, `None` for any other class.
    pub const fn iospace_level(self) -> Option<u8> {
        match self.0 {
            33..=40 => Some(self.0 - 32),
            _ => None,
        }
    }

    /// The devices pinned or attached to an iospace root, `None` for any
    /// other class.
    pub const fn root_devices(self) -> Option<u8> {
        match self.0 {
            48..=64 => Some(self.0 - 48),
            _ => None,
        }
    }

    /// `None` for a byte the page classes may not hold (G2): the stored
    /// classes are *data*, *page table of level k*, *iospace table of level
    /// k* and *iospace root with n devices attached*; a *gate*, *global
    /// read-only* or *device* page lies in no chunk, and its class follows
    /// from where it lies.
    pub const fn stored(raw: u8) -> Option<Self> {
        match raw {
            0 | 17..=24 | 33..=40 | 48..=64 => Some(Self(raw)),
            _ => None,
        }
    }

    /// `None` for a byte that names no class.
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0..=3 | 17..=24 | 33..=40 | 48..=64 => Some(Self(raw)),
            _ => None,
        }
    }

    pub const fn raw(self) -> u8 {
        self.0
    }

    /// The permission rules of G3 (3) and (4): a leaf may target data,
    /// device and global read-only pages only; a data page never with a
    /// device memory type nor both writable and executable; a global
    /// read-only page never writable; a device page never executable and
    /// always with a device memory type. A table or gate page is never the
    /// target of a leaf (outside the gate's own view).
    pub const fn permits(self, leaf: Leaf) -> bool {
        match self.0 {
            0 => !leaf.device_type && !(leaf.write && leaf.exec),
            2 => !leaf.write && !leaf.device_type,
            3 => !leaf.exec && leaf.device_type,
            _ => false,
        }
    }
}

/// The state of an attach word (IOMMU, D-137).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachState {
    /// The owner's gate pinned the root: an attach may be served.
    Pinned = 1,
    /// The holder's gate wrote a context entry that names the root.
    Attached = 2,
    /// The holder's gate cleared the entry and its invalidation is done.
    Detached = 3,
}

/// The attach word of a device identifier, beside its identifier word in
/// the living part (IOMMU, D-137): 0, or the state in bits 0-1 and the
/// physical address of the iospace's root, aligned on a page, in the other
/// bits. Changed only by compare-and-swap through a gate.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachWord(u64);

impl AttachWord {
    pub const NONE: Self = Self(0);
    /// The address bits that a page of the smallest size leaves 0.
    const LOW: u64 = 0xfff;

    /// `None` for a root of 0 or not aligned on the smallest page.
    pub const fn new(state: AttachState, root: u64) -> Option<Self> {
        if root == 0 || root & Self::LOW != 0 {
            return None;
        }
        Some(Self(root | state as u64))
    }

    /// `None` for a word that fails its check: a state 0 with an address,
    /// a state with no address, or a bit set between the state and the
    /// address.
    pub const fn from_raw(raw: u64) -> Option<Self> {
        let root = raw & !Self::LOW;
        match (raw & 0b11, root, raw & Self::LOW & !0b11) {
            (0, 0, 0) => Some(Self::NONE),
            (1..=3, 1.., 0) => Some(Self(raw)),
            _ => None,
        }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn state(self) -> Option<AttachState> {
        match self.0 & 0b11 {
            1 => Some(AttachState::Pinned),
            2 => Some(AttachState::Attached),
            3 => Some(AttachState::Detached),
            _ => None,
        }
    }

    pub const fn root(self) -> u64 {
        self.0 & !Self::LOW
    }
}

/// The gate word of a chunk (G4): the map count in bits 0-31, the number of
/// pages of the chunk not of class data in bits 32-44, the other bits 0.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateWord(u64);

impl GateWord {
    pub const fn new(map_count: u32, not_data: u16) -> Option<Self> {
        if not_data > 1 << 12 {
            return None;
        }
        Some(Self(map_count as u64 | (not_data as u64) << 32))
    }

    /// `None` for a word with a bit above 44 set, or more pages not of
    /// class data than a chunk can have (*E* ≤ 4,096).
    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw >> 45 != 0 || (raw >> 32) as u16 > 1 << 12 {
            return None;
        }
        Some(Self(raw))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn map_count(self) -> u32 {
        self.0 as u32
    }

    pub const fn not_data(self) -> u16 {
        (self.0 >> 32) as u16
    }
}
pub const GRANT_ENTRY_BYTES: usize = 128;
pub const USE_ENTRY_BYTES: usize = 32;
/// Unused entries of the device table, for splits.
pub const DEVICE_TABLE_SPARE: usize = 8192;
/// Reserves, from 2 to 8 (1 on a machine with one usable core), and plain
/// roles, at most 64 in all.
pub const RESERVES_MIN: u32 = 2;
pub const RESERVES_MAX: u32 = 8;
pub const ROLES_PLAIN_MAX: u32 = 64;
/// The watch slots of a node, the entries of an idle table, the open
/// observations of a node.
pub const NODE_WATCH_SLOTS: u64 = 4;
pub const IDLE_STATES_MAX: usize = 15;
pub const OBSERVATIONS_MAX: usize = 8;
/// Words of one entry of an idle table.
pub const IDLE_ENTRY_WORDS: usize = 4;

// The times of membership (Detection, The fence, Numbers): the default and
// the range the kernel accepts from `BootMembership`.
pub const STALE_NS: u64 = 100_000_000;
pub const STUCK_NS: u64 = 100_000_000;
pub const STALE_RANGE_NS: (u64, u64) = (10_000_000, 1_000_000_000);
pub const SWEEP_NS: u64 = 1_000_000_000;
pub const SWEEP_RANGE_NS: (u64, u64) = (100_000_000, 60_000_000_000);
pub const NMI_GRACE_NS: u64 = 1_000_000;
/// 20 ms since D-129 (it was 10 ms, less than the SDM's example of INIT and
/// SIPI alone), within 10 ms to 1 s.
pub const FENCE_NS: u64 = 20_000_000;
pub const FENCE_RANGE_NS: (u64, u64) = (10_000_000, 1_000_000_000);
pub const SETTLE_NS: u64 = 1_000_000;
pub const START_NS: u64 = 1_000_000_000;
/// The period at which the node that starts a core sends its wake again
/// until the park stub's `started` equals its start request (Node slots and
/// core words, the start request).
pub const START_RETRY_NS: u64 = 1_000_000;

/// The cores of a node a fence record holds the last stop of: a node's
/// cores lie in one window of 64 core numbers.
pub const FENCE_RECORD_CORES: usize = 64;
/// Bytes of one fence record.
pub const FENCE_RECORD_BYTES: usize = 544;

/// The fence record of a node slot, in the living part, written by the gate
/// of the node's fence driver (The fence: the fence record): the generation
/// it concerns, the instant of the first fence action, the instant the fence
/// was confirmed, and for each core of the node, at the index of its core
/// number modulo 64 (`stop_index`), the instant of the last stop sent to
/// that core: a node's cores lie in one window of 64 core numbers, so the
/// index never changes while R6 frees other words, as a position would
/// (D-134). An instant of 0 means none; the instants are the gate's clock.
/// The delays of a take-over (Roles) and of R6 are measured from them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceRecord {
    pub generation: u32,
    pub reserved: u32,
    pub first_action: u64,
    pub confirmed: u64,
    pub reserved2: u64,
    pub last_stop: [u64; FENCE_RECORD_CORES],
}

impl FenceRecord {
    /// The index in `last_stop` of the last stop sent to a core: its core
    /// number modulo 64 (The fence, the fence record, D-134).
    pub const fn stop_index(core: usize) -> usize {
        core % FENCE_RECORD_CORES
    }
}
/// The time a node may stay sealed before evidence seal accuses it.
pub const SEAL_NS: u64 = 1_000_000_000;
pub const SEAL_RANGE_NS: (u64, u64) = (10_000_000, 60_000_000_000);
/// The time a core of a node may take to release a lock of the node or to
/// acknowledge a request of another core of its node; beyond it the node
/// accuses itself (A node of several cores).
pub const CORE_ANSWER_NS: u64 = 10_000_000;
pub const CORE_ANSWER_RANGE_NS: (u64, u64) = (1_000_000, 100_000_000);
/// Cores of one node at most: the bitmaps of a node's cores are 64 bits.
pub const CORES_PER_KERNEL_MAX: usize = 64;
/// Observations expire after this many times `STALE_NS`.
pub const OBSERVATION_LIFE: u64 = 4;
/// The recovery reserve: 1/64 of a core in the units of admission, period
/// 10 ms.
pub const RECOVERY_BANDWIDTH: u64 = 1 << 26;
pub const RECOVERY_PERIOD_NS: u64 = 10_000_000;

// The calls of the nodes.
pub const NODE_START_FRESH: u64 = 0;
pub const NODE_START_RESUME: u64 = 1;
/// Kernel images of `node_start`: the boot kernel and the replacement slot.
pub const NODE_IMAGE_BOOT: u64 = 0;
pub const NODE_IMAGE_REPLACEMENT: u64 = 1;
pub const NODE_STOP_KILL: u64 = 0;
pub const NODE_STOP_SEAL: u64 = 1;
pub const NODE_STOP_RETIRE: u64 = 2;
/// Word 1 of `node_status`: the number of cores whose words name the node
/// (bits 0-15), its fence class (bits 16-17), and bit 18 set if the core
/// asked is unusable.
pub const NODE_STATUS_CORES_MASK: u64 = 0xFFFF;
pub const NODE_STATUS_CLASS_SHIFT: u32 = 16;
pub const NODE_STATUS_UNUSABLE: u64 = 1 << 18;
/// Words of the result of `node_status`.
pub const NODE_STATUS_WORDS: usize = 7;
/// Methods and flags of an idle table (`node_idle`).
pub const IDLE_METHOD_HALT: u64 = 0;
pub const IDLE_METHOD_MWAIT: u64 = 1;
pub const IDLE_METHOD_PSCI: u64 = 2;
pub const IDLE_METHOD_SBI: u64 = 3;
pub const IDLE_FLAG_LOSES_CONTEXT: u64 = 1;
/// Fence classes of a core (The fence).
pub const FENCE_CLASS_COOPERATIVE: u8 = 0;
pub const FENCE_CLASS_SIGNALLED: u8 = 1;
pub const FENCE_CLASS_FORCED: u8 = 2;
/// The core event of a change of the feedback table or of the performance
/// capabilities (Unlike cores).
pub const CORE_EVENT_FEEDBACK: u8 = 1;

// Chunks, floors and the ownership words.

/// The multiplier of the check field.
pub const CHECK_MUL: u64 = 0x9E37_79B9_7F4A_7C15;
/// The owner of a word that nobody holds.
pub const OWNER_UNCLAIMED: u16 = 0xFFFF;
/// The owner of a word of reserve `j` is `OWNER_RESERVE_BASE + j`.
pub const OWNER_RESERVE_BASE: u16 = 0xFFF0;

/// The state of an ownership word.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnState {
    /// Not ordinary memory, kept, or in a floor; never changes.
    None = 0,
    /// No block covers the chunk.
    Free = 1,
    /// A block of that generation of that owner covers it.
    Taken = 2,
}

/// The owner field of an ownership word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Node(u16),
    Unclaimed,
    Reserve(u16),
}

impl Owner {
    pub const fn code(self) -> u16 {
        match self {
            Self::Node(node) => node,
            Self::Unclaimed => OWNER_UNCLAIMED,
            Self::Reserve(j) => OWNER_RESERVE_BASE + j,
        }
    }

    /// `None` for a code that names no node below `NODES_MAX`, no reserve
    /// and not the unclaimed owner.
    pub const fn from_code(code: u16) -> Option<Self> {
        if code == OWNER_UNCLAIMED {
            Some(Self::Unclaimed)
        } else if code >= OWNER_RESERVE_BASE && code < OWNER_RESERVE_BASE + RESERVES_MAX as u16 {
            Some(Self::Reserve(code - OWNER_RESERVE_BASE))
        } else if (code as usize) < NODES_MAX {
            Some(Self::Node(code))
        } else {
            None
        }
    }
}

/// The ownership word of a chunk, or of a device entry: state (0-1), check
/// (2-15), owner (16-31), generation (32-63).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ownership(u64);

impl Ownership {
    /// The top 14 bits of v × `CHECK_MUL`, with v = state | owner << 2 |
    /// generation << 18.
    const fn check(state: u64, owner: u16, generation: u32) -> u64 {
        let v = state | (owner as u64) << 2 | (generation as u64) << 18;
        v.wrapping_mul(CHECK_MUL) >> 50
    }

    const fn make(state: OwnState, owner: u16, generation: u32) -> Self {
        let state = state as u64;
        Self(
            state
                | Self::check(state, owner, generation) << 2
                | (owner as u64) << 16
                | (generation as u64) << 32,
        )
    }

    /// The word 0: a chunk that is not ordinary memory, kept, or in a floor.
    pub const fn none() -> Self {
        Self(0)
    }

    /// No block covers the chunk; its generation is 0.
    pub const fn free(owner: Owner) -> Self {
        Self::make(OwnState::Free, owner.code(), 0)
    }

    /// A block of that generation of that owner covers the chunk.
    pub const fn taken(owner: Owner, generation: u32) -> Self {
        Self::make(OwnState::Taken, owner.code(), generation)
    }

    /// `None` for a word that fails its check: state 3, a check field that
    /// does not match, an owner code that names nothing, a word *none*
    /// other than 0, or a free word with a generation.
    pub const fn from_raw(raw: u64) -> Option<Self> {
        let state = raw & 0b11;
        let owner = (raw >> 16) as u16;
        let generation = (raw >> 32) as u32;
        let well_formed = match state {
            0 => raw == 0,
            1 => generation == 0 && Owner::from_code(owner).is_some(),
            2 => Owner::from_code(owner).is_some(),
            _ => false,
        };
        if !well_formed || raw >> 2 & 0x3fff != Self::check(state, owner, generation) {
            return None;
        }
        Some(Self(raw))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn state(self) -> OwnState {
        match self.0 & 0b11 {
            0 => OwnState::None,
            1 => OwnState::Free,
            _ => OwnState::Taken,
        }
    }

    pub const fn owner(self) -> Owner {
        match Owner::from_code((self.0 >> 16) as u16) {
            Some(owner) => owner,
            None => Owner::Unclaimed,
        }
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Who may take a word: whether node `node`, of current generation
    /// `generation`, may write *taken* into this word. `owner` is the member
    /// word of the owner, when the owner is a node (ignored otherwise);
    /// `holds_reserve` whether `node` holds the role of the reserve, when the
    /// owner is one. Rules 1 to 4 of taking; any other owner is refused, and
    /// so is a word *none*. Rule 3: the owner is dead, or the word is
    /// *taken* by a generation of its owner that is over (the owner's
    /// member word names another generation): a node gets a new generation
    /// only after it was dead, past the barrier and R6. A *free* word with a
    /// node as owner is memory released or given to that node, and stays
    /// that node's while it is not dead.
    pub const fn may_take(
        self,
        node: u16,
        generation: u32,
        owner: Member,
        holds_reserve: bool,
    ) -> bool {
        let state = self.state();
        if matches!(state, OwnState::None) {
            return false;
        }
        match self.owner() {
            Owner::Node(o) if o == node => {
                matches!(state, OwnState::Free) || self.generation() != generation
            }
            Owner::Node(_) => {
                matches!(owner.state(), MemberState::Dead)
                    || matches!(state, OwnState::Taken) && owner.generation() != self.generation()
            }
            Owner::Unclaimed => true,
            Owner::Reserve(_) => holds_reserve,
        }
    }
}

/// One entry of the device table, 16 bytes: the base (aligned on the size,
/// at least one page) with the log2 of the size in bits 0-5 and bit 6 when
/// the entry is used; then an ownership word.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub range: u64,
    pub owner: Ownership,
}

impl DeviceEntry {
    pub const USED: u64 = 1 << 6;

    /// A used entry of `1 << log2` bytes at `base`, unclaimed.
    pub const fn used(base: u64, log2: u32) -> Self {
        assert!(log2 < 64 && base & ((1 << log2) - 1) == 0);
        Self {
            range: base | log2 as u64 | Self::USED,
            owner: Ownership::free(Owner::Unclaimed),
        }
    }

    /// An unused entry, unclaimed.
    pub const fn unused() -> Self {
        Self {
            range: 0,
            owner: Ownership::free(Owner::Unclaimed),
        }
    }

    pub const fn is_used(&self) -> bool {
        self.range & Self::USED != 0
    }

    pub const fn log2(&self) -> u32 {
        (self.range & 0x3f) as u32
    }

    pub const fn base(&self) -> u64 {
        self.range & !0x7f
    }
}

// The states of a node.

/// The state of a node, bits 0-3 of its member word.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberState {
    Dead = 0,
    Starting = 1,
    Live = 2,
    Sealing = 3,
    Sealed = 4,
    Fencing = 5,
    Fenced = 6,
    Unfenced = 7,
}

/// The evidence of the last accusation, bits 4-6 of a member word.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    None = 0,
    SelfReported = 1,
    Stale = 2,
    Stuck = 3,
    Kill = 4,
    Retire = 5,
    /// The node is sealed, and the instant of its seal is older than
    /// `SEAL_NS` (D-129, in place of the evidence host).
    Seal = 6,
}

/// The driver field of a member word with no driver.
pub const DRIVER_NONE: u16 = 0xFFFF;

/// The member word of a node: state (0-3), evidence (4-6), the node's fence
/// class (7-8, the lowest class of its cores, written when it starts), zero
/// (9-15), driver (16-31), generation (32-63).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member(u64);

impl Member {
    pub const fn new(state: MemberState, evidence: Evidence, driver: u16, generation: u32) -> Self {
        Self(
            state as u64
                | (evidence as u64) << 4
                | (driver as u64) << 16
                | (generation as u64) << 32,
        )
    }

    /// Dead, generation 0: never started.
    pub const fn never() -> Self {
        Self::new(MemberState::Dead, Evidence::None, DRIVER_NONE, 0)
    }

    /// The same word with the fence class `class` (0 to 2) in bits 7-8.
    pub const fn with_class(self, class: u8) -> Self {
        Self(self.0 & !(0b11 << 7) | ((class & 0b11) as u64) << 7)
    }

    /// `None` for a state 8 to 15, an evidence 7, a class 3, or a bit 9-15
    /// set: "a word with 3 fails its check, as a state 8 to 15 does" (The
    /// states of a node, the member word, gap 1).
    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw & 0xf >= 8 || raw >> 4 & 0x7 == 7 || raw >> 7 & 0b11 == 3 || raw >> 9 & 0x7f != 0 {
            return None;
        }
        Some(Self(raw))
    }

    /// The node's fence class, bits 7-8.
    pub const fn class(self) -> u8 {
        (self.0 >> 7 & 0b11) as u8
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn state(self) -> MemberState {
        match self.0 & 0xf {
            0 => MemberState::Dead,
            1 => MemberState::Starting,
            2 => MemberState::Live,
            3 => MemberState::Sealing,
            4 => MemberState::Sealed,
            5 => MemberState::Fencing,
            6 => MemberState::Fenced,
            _ => MemberState::Unfenced,
        }
    }

    pub const fn evidence(self) -> Evidence {
        match self.0 >> 4 & 0x7 {
            0 => Evidence::None,
            1 => Evidence::SelfReported,
            2 => Evidence::Stale,
            3 => Evidence::Stuck,
            4 => Evidence::Kill,
            5 => Evidence::Retire,
            _ => Evidence::Seal,
        }
    }

    pub const fn driver(self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Starting, live or sealing: a node whose own gate still writes.
    pub const fn writes(self) -> bool {
        matches!(
            self.state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing
        )
    }
}

/// The rows of the table of transitions of a member word (The states of a
/// node). The gate checks the condition of each row besides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    /// dead (g) → starting (g + 1), by `node_start` mode fresh, with the
    /// class of the cores named.
    Start,
    /// sealed (g) → starting (g), by `node_start` mode resume.
    Resume,
    /// starting (g) → live (g), by the node itself.
    Up,
    /// live (g) → sealing (g), by `node_stop` mode seal.
    Seal,
    /// sealing (g) → sealed (g), by the node itself.
    Sealed,
    /// sealing (g) → live (g), by the node itself when no other node is
    /// live (D-131).
    Abandon,
    /// starting, live, sealing, sealed (g) → fencing (g), with evidence.
    Accuse,
    /// fencing or unfenced (g), driver D → the same state, driver E.
    Redrive,
    /// fencing (g) → fenced (g), by the driver.
    Fence,
    /// fencing (g) → unfenced (g), by the driver.
    Unfence,
    /// unfenced (g) → fenced (g).
    LateFence,
    /// fenced (g) → dead (g), when the reclaim barrier holds.
    Reclaim,
}

/// The gate's check of a transition: the row of the table that changes
/// `from` into `to`, `None` if none does. Evidence and driver must be what
/// the row writes: an accusation names evidence other than none and a
/// driver; every other row keeps both, except a start, which clears them,
/// and a change of driver, which keeps the evidence. The fence class is
/// written by a fresh start only, and every other row keeps it: a node's
/// cores do not change while it lives.
pub const fn member_transition(from: Member, to: Member) -> Option<Transition> {
    use MemberState::{Dead, Fenced, Fencing, Live, Sealed, Sealing, Starting, Unfenced};
    let (g, h) = (from.generation(), to.generation());
    let same = g == h && from.class() == to.class();
    let kept = from.evidence() as u8 == to.evidence() as u8 && from.driver() == to.driver();
    let cleared = matches!(to.evidence(), Evidence::None) && to.driver() == DRIVER_NONE;
    let accused = !matches!(to.evidence(), Evidence::None) && to.driver() != DRIVER_NONE;
    match (from.state(), to.state()) {
        (Dead, Starting) if g.wrapping_add(1) == h && cleared => Some(Transition::Start),
        (Sealed, Starting) if same && cleared => Some(Transition::Resume),
        (Starting, Live) if same && kept => Some(Transition::Up),
        (Live, Sealing) if same && kept => Some(Transition::Seal),
        (Sealing, Sealed) if same && kept => Some(Transition::Sealed),
        (Sealing, Live) if same && kept => Some(Transition::Abandon),
        (Starting | Live | Sealing | Sealed, Fencing) if same && accused => {
            Some(Transition::Accuse)
        }
        (Fencing, Fencing) | (Unfenced, Unfenced)
            if same
                && from.evidence() as u8 == to.evidence() as u8
                && to.driver() != DRIVER_NONE
                && to.driver() != from.driver() =>
        {
            Some(Transition::Redrive)
        }
        (Fencing, Fenced) if same && kept => Some(Transition::Fence),
        (Fencing, Unfenced) if same && kept => Some(Transition::Unfence),
        (Unfenced, Fenced) if same && kept => Some(Transition::LateFence),
        (Fenced, Dead) if same && kept => Some(Transition::Reclaim),
        _ => None,
    }
}

/// The successor of a unit role whose holder is no longer live (Roles): the
/// first node after `holder` in cyclic index order among the live nodes
/// whose fence class, in their member word, is the highest found among live
/// nodes. `None` when no node is live.
pub fn successor(members: &[Member], holder: usize) -> Option<usize> {
    let n = members.len();
    let live = |i: usize| matches!(members[i].state(), MemberState::Live);
    let best = (0..n)
        .filter(|&i| live(i))
        .map(|i| members[i].class())
        .max()?;
    (1..=n)
        .map(|step| (holder + step) % n)
        .find(|&i| live(i) && members[i].class() == best)
}

/// The best fence class among live nodes, read from their member words;
/// `None` when no node is live.
pub fn best_class(members: &[Member]) -> Option<u8> {
    let live = members
        .iter()
        .filter(|m| matches!(m.state(), MemberState::Live));
    live.map(|m| m.class()).max()
}

/// What the reclaim barrier reads of one node *Y* ≠ *X* (The reclaim
/// barrier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarrierView {
    pub member: Member,
    /// `acked[X]` in *Y*'s beacon.
    pub acked: u32,
    /// `maps_of` bit *X* in *Y*'s beacon.
    pub maps: bool,
    /// For *Y* fenced: every node that is starting, live or sealing has
    /// acknowledged *Y*.
    pub acknowledged_by_all: bool,
}

/// The reclaim barrier: whether *X*, of generation `generation`, may go from
/// fenced to dead, given the view of every other node and whether no unit
/// role is held by *X* or is handing to or from it.
pub fn barrier(generation: u32, others: &[BarrierView], unit_roles_clear: bool) -> bool {
    unit_roles_clear
        && others.iter().all(|y| match y.member.state() {
            MemberState::Starting | MemberState::Live | MemberState::Sealing => {
                y.acked == generation
            }
            MemberState::Sealed | MemberState::Fencing | MemberState::Unfenced => !y.maps,
            MemberState::Fenced => !y.maps || y.acknowledged_by_all,
            MemberState::Dead => true,
        })
}

// Node slots and core words.

/// The state of a core word, bits 0-1.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreState {
    /// No node holds the core; it executes no kernel.
    Free = 0,
    /// The core runs the node the word names.
    Owned = 1,
    /// Reserved for nodes that give cores at run time: no rule of the
    /// contract writes it, and the gate refuses to.
    Leaving = 2,
    /// The node the word names is starting on the core.
    Joining = 3,
}

/// Bit 2 of a core word: the driver of the fence of the node the word
/// names has asked the core to stop.
pub const CORE_MARK: u64 = 1 << 2;
/// Bit 3 of a core word: the core is not started again before the machine
/// restarts.
pub const CORE_UNUSABLE: u64 = 1 << 3;
/// The node slot field of a core word that names no node.
pub const CORE_NO_SLOT: u16 = 0xFFFF;

/// The core word of a physical core (Node slots and core words): state
/// (0-1), the fence mark (2), unusable (3), the check (4-15: the top 12
/// bits of the product by `CHECK_MUL` of the word with its check bits 0),
/// the node slot (16-31, 0xFFFF for none), the node's generation (32-63, 0
/// when free).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreWord(u64);

impl CoreWord {
    const CHECK: u64 = 0xfff << 4;

    /// The word without its check bits: what the check is computed from.
    const fn base(raw: u64) -> u64 {
        raw & !Self::CHECK
    }

    const fn checked(v: u64) -> Self {
        let v = Self::base(v);
        Self(v | (v.wrapping_mul(CHECK_MUL) >> 52) << 4)
    }

    const fn make(state: CoreState, slot: u16, generation: u32, flags: u64) -> Self {
        Self::checked(
            state as u64
                | flags & (CORE_MARK | CORE_UNUSABLE)
                | (slot as u64) << 16
                | (generation as u64) << 32,
        )
    }

    /// Free: no slot, generation 0, the flag unusable as given.
    pub const fn free(unusable: bool) -> Self {
        let flags = if unusable { CORE_UNUSABLE } else { 0 };
        Self::make(CoreState::Free, CORE_NO_SLOT, 0, flags)
    }

    /// Joining (N, g): node slot N of generation g is starting on the core.
    pub const fn joining(slot: u16, generation: u32) -> Self {
        Self::make(CoreState::Joining, slot, generation, 0)
    }

    /// Owned (N, g): the core runs node slot N of generation g.
    pub const fn owned(slot: u16, generation: u32) -> Self {
        Self::make(CoreState::Owned, slot, generation, 0)
    }

    /// The same word with the fence mark.
    pub const fn marked(self) -> Self {
        Self::checked(self.0 | CORE_MARK)
    }

    /// The same word with the flag unusable.
    pub const fn unusable(self) -> Self {
        Self::checked(self.0 | CORE_UNUSABLE)
    }

    /// The word, or `None` if its check fails: such a word counts as held
    /// by another live node (the reader traces it, code 10).
    pub const fn from_raw(raw: u64) -> Option<Self> {
        let word = Self::checked(raw);
        if word.0 == raw { Some(word) } else { None }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn state(self) -> CoreState {
        match self.0 & 0b11 {
            0 => CoreState::Free,
            1 => CoreState::Owned,
            2 => CoreState::Leaving,
            _ => CoreState::Joining,
        }
    }

    pub const fn is_marked(self) -> bool {
        self.0 & CORE_MARK != 0
    }

    pub const fn is_unusable(self) -> bool {
        self.0 & CORE_UNUSABLE != 0
    }

    pub const fn slot(self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Whether the word names node slot `slot` with generation `generation`
    /// (owned or joining).
    pub const fn names(self, slot: u16, generation: u32) -> bool {
        matches!(self.state(), CoreState::Owned | CoreState::Joining)
            && self.slot() == slot
            && self.generation() == generation
    }
}

// Roles.

/// The state of a role word, bits 0-1.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleState {
    Vacant = 0,
    Held = 1,
    Handing = 2,
}

/// The kinds of role.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleKind {
    Unit = 0,
    Reserve = 1,
    Plain = 2,
}

/// What a role governs (entry `Roles`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Governs {
    IommuUnit = 0,
    IoApic = 1,
    GicDistributor = 2,
    GicIts = 3,
    Console = 4,
    Reserve = 5,
    Plain = 6,
    /// The APLIC of a RISC-V interrupt domain.
    Aplic = 7,
}

/// Plain role 0: the node of its holder alone may use the PCI configuration
/// ports and the reset port.
pub const ROLE_PLAIN_PLATFORM: u16 = 0;
/// The holder and target fields with nobody.
pub const ROLE_NOBODY: u16 = 0xFFFF;
/// The epoch of a role word: 29 bits, compared for equality only.
pub const ROLE_EPOCH_MASK: u32 = (1 << 29) - 1;
/// Bit 63 of a role word: the unit failed.
pub const ROLE_FAILED: u64 = 1 << 63;

/// The role word: state (0-1), holder (2-17; for handing, the node that
/// hands), target (18-33; handing only, otherwise 0xFFFF), epoch (34-62),
/// and bit 63, for a unit role, *failed*: the unit did not confirm within
/// `IOMMU_WAIT_NS` (D-131).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleWord(u64);

impl RoleWord {
    const fn make(state: RoleState, holder: u16, target: u16, epoch: u32) -> Self {
        Self(
            state as u64
                | (holder as u64) << 2
                | (target as u64) << 18
                | ((epoch & ROLE_EPOCH_MASK) as u64) << 34,
        )
    }

    /// The same word with the bit *failed*, which its holder's gate writes
    /// once and which stays until the machine restarts.
    pub const fn failed_now(self) -> Self {
        Self(self.0 | ROLE_FAILED)
    }

    /// Bit 63: the unit failed.
    pub const fn failed(self) -> bool {
        self.0 & ROLE_FAILED != 0
    }

    /// The same word with the fields given, keeping the bit *failed*.
    pub const fn keeping_failed(self, next: Self) -> Self {
        Self(next.0 | self.0 & ROLE_FAILED)
    }

    pub const fn vacant(epoch: u32) -> Self {
        Self::make(RoleState::Vacant, ROLE_NOBODY, ROLE_NOBODY, epoch)
    }

    pub const fn held(holder: u16, epoch: u32) -> Self {
        Self::make(RoleState::Held, holder, ROLE_NOBODY, epoch)
    }

    pub const fn handing(from: u16, to: u16, epoch: u32) -> Self {
        Self::make(RoleState::Handing, from, to, epoch)
    }

    /// `None` for state 3, a vacant word that names a holder or a target,
    /// or a held word that names a target.
    pub const fn from_raw(raw: u64) -> Option<Self> {
        let word = Self(raw);
        let ok = match raw & 0b11 {
            0 => word.holder() == ROLE_NOBODY && word.target() == ROLE_NOBODY,
            1 => word.target() == ROLE_NOBODY,
            2 => true,
            _ => false,
        };
        if ok { Some(word) } else { None }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn state(self) -> RoleState {
        match self.0 & 0b11 {
            0 => RoleState::Vacant,
            1 => RoleState::Held,
            _ => RoleState::Handing,
        }
    }

    pub const fn holder(self) -> u16 {
        (self.0 >> 2) as u16
    }

    pub const fn target(self) -> u16 {
        (self.0 >> 18) as u16
    }

    /// 29 bits; one more at every change, modulo 2^29.
    pub const fn epoch(self) -> u32 {
        (self.0 >> 34) as u32 & ROLE_EPOCH_MASK
    }
}

// Start-up: floors and the kept frames of a module.

/// The record `BootFloors` of the boot image (32 bytes); the names of the
/// plain roles follow it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootFloors {
    pub floor_node: u64,
    pub reserve_bytes: u64,
    pub reserve_per_gib: u64,
    pub reserves: u32,
    pub plain_roles: u32,
}

/// The record `BootMembership` of the boot image (24 bytes): the times of
/// Detection in milliseconds, then `SEAL_NS` and `CORE_ANSWER_NS` in
/// milliseconds (D-129).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootMembership {
    pub stale_ms: u32,
    pub stuck_ms: u32,
    pub sweep_ms: u32,
    pub seal_ms: u32,
    pub core_answer_ms: u32,
    pub reserved: u32,
}

/// The record `BootNodes` of the boot image (8 bytes), followed by one
/// `BootNode` per node the machine start-up starts: the number of node
/// slots and the number of nodes of the initial layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootNodes {
    pub slots: u32,
    pub nodes: u32,
}

/// One node of the initial layout (16 bytes): its cores as a window and a
/// mask, the cores whose number is 64 × window + i for each bit i set.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootNode {
    pub window: u64,
    pub mask: u64,
}

/// The sizes of the floors and the reserves (Start-up state, rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Floors {
    /// The floor of every node, a whole number of chunks.
    pub floor: u64,
    /// Each reserve, a whole number of chunks.
    pub reserve: u64,
    /// n floors and k reserves.
    pub total: u64,
}

/// The floors and reserves of a machine of `ordinary` bytes of ordinary
/// memory and `n` nodes: `floor_node` rounded up to a chunk, and
/// `reserve_bytes + reserve_per_gib × O` rounded up to a chunk, with O the
/// ordinary memory in whole GiB. `None` when a sum overflows or the total
/// does not fit in the ordinary memory. `chunk` is a power of two.
pub const fn floors(ordinary: u64, n: u64, boot: BootFloors, chunk: u64) -> Option<Floors> {
    let Some(floor) = round_up(boot.floor_node, chunk) else {
        return None;
    };
    let Some(per) = boot.reserve_per_gib.checked_mul(ordinary >> 30) else {
        return None;
    };
    let Some(bytes) = boot.reserve_bytes.checked_add(per) else {
        return None;
    };
    let Some(reserve) = round_up(bytes, chunk) else {
        return None;
    };
    let Some(floors) = floor.checked_mul(n) else {
        return None;
    };
    let Some(reserves) = reserve.checked_mul(boot.reserves as u64) else {
        return None;
    };
    match floors.checked_add(reserves) {
        Some(total) if total <= ordinary => Some(Floors {
            floor,
            reserve,
            total,
        }),
        _ => None,
    }
}

/// `bytes` rounded up to a multiple of `unit`, a power of two; `None` on
/// overflow.
const fn round_up(bytes: u64, unit: u64) -> Option<u64> {
    match bytes.checked_add(unit - 1) {
        Some(sum) => Some(sum & !(unit - 1)),
        None => None,
    }
}

/// One kept frame of a module: its page size, its number of pages, and its
/// offset from the start of the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleFrame {
    pub size: PageSize,
    pub pages: u64,
    pub offset: u64,
}

/// The kept frames of a module (Start-up state, rule 5), in order: at each
/// position the largest supported page size on which the physical address
/// (and, for a segment, the virtual address) is aligned and that is not
/// longer than what remains rounded up to a page; pages of that size while
/// they fit, at most *E*, ending early at the first position aligned on a
/// larger size that fits. The last frame ends at the length rounded up to a
/// page. `virt` is `None` for a file that is not mapped.
#[derive(Clone, Copy, Debug)]
pub struct ModuleFrames {
    phys: u64,
    virt: Option<u64>,
    offset: u64,
    end: u64,
    geometry: Geometry,
}

/// The kept frames of a module of `len` bytes at `phys`.
pub const fn module_frames(
    phys: u64,
    virt: Option<u64>,
    len: u64,
    geometry: Geometry,
) -> ModuleFrames {
    let page = geometry.page();
    ModuleFrames {
        phys,
        virt,
        offset: 0,
        end: len.div_ceil(page) * page,
        geometry,
    }
}

impl ModuleFrames {
    /// The largest supported size that suits position `offset` with `left`
    /// bytes left, and that is above `above` if given.
    fn size_at(&self, offset: u64, left: u64) -> PageSize {
        let mut best = PageSize::Page;
        for size in PageSize::ALL {
            let Some(bytes) = self.geometry.bytes(size) else {
                continue;
            };
            let aligned = (self.phys + offset).is_multiple_of(bytes)
                && self
                    .virt
                    .is_none_or(|virt| (virt + offset).is_multiple_of(bytes));
            if aligned && bytes <= left {
                best = size;
            }
        }
        best
    }
}

impl Iterator for ModuleFrames {
    type Item = ModuleFrame;

    fn next(&mut self) -> Option<ModuleFrame> {
        if self.offset >= self.end {
            return None;
        }
        let start = self.offset;
        let size = self.size_at(start, self.end - start);
        let bytes = self.geometry.bytes(size).unwrap_or(self.geometry.page());
        let mut pages = 0;
        let mut at = start;
        let entries = u64::from(self.geometry.entries);
        while pages < entries && at + bytes <= self.end {
            if pages > 0 && self.size_at(at, self.end - at) > size {
                break;
            }
            pages += 1;
            at += bytes;
        }
        self.offset = at;
        Some(ModuleFrame {
            size,
            pages,
            offset: start,
        })
    }
}

/// The clean mark after one step of a creation (rules of `untyped_retype`):
/// at most `CLEAR_STEP_BYTES` from the clean mark, stopping at `end`.
pub const fn clear_step(clean: u64, end: u64) -> u64 {
    if clean >= end {
        return clean;
    }
    let reached = clean.saturating_add(CLEAR_STEP_BYTES);
    if reached < end { reached } else { end }
}

/// The calls a creation takes to clear from `clean` to `end`: one per step.
pub const fn clear_steps(clean: u64, end: u64) -> u64 {
    if clean >= end {
        1
    } else {
        (end - clean).div_ceil(CLEAR_STEP_BYTES)
    }
}

// I/O ports.

/// The range of an `ioports` capability: first in bits 0-15, last in 16-31,
/// the other bits 0. `cap_inspect` returns it as the badge.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoPortRange(u64);

impl IoPortRange {
    /// Every port: the range of a root.
    pub const ALL: Self = Self(0xFFFF << 16);

    /// `None` unless first ≤ last ≤ 65,535.
    pub const fn new(first: u64, last: u64) -> Option<Self> {
        if first <= last && last <= 0xFFFF {
            Some(Self(first | last << 16))
        } else {
            None
        }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn first(self) -> u64 {
        self.0 & 0xFFFF
    }

    pub const fn last(self) -> u64 {
        self.0 >> 16 & 0xFFFF
    }

    /// first ≤ port and port + width − 1 ≤ last, computed without overflow;
    /// false for a width of 0.
    pub const fn contains(self, port: u64, width: u64) -> bool {
        width != 0
            && port >= self.first()
            && match port.checked_add(width - 1) {
                Some(end) => end <= self.last(),
                None => false,
            }
    }

    /// Whether `other` lies inside this range.
    pub const fn covers(self, other: Self) -> bool {
        self.first() <= other.first() && other.last() <= self.last()
    }
}

/// The plain arguments of `ioport_in` and `ioport_out`, in order, each
/// `Invalid`: the width is 1, 2 or 4; the access lies in the range; for
/// `ioport_out`, the value is below 2^(8 × width).
pub const fn ioport_check(
    range: IoPortRange,
    port: u64,
    width: u64,
    value: Option<u64>,
) -> Result<(), Error> {
    if !matches!(width, 1 | 2 | 4) || !range.contains(port, width) {
        return Err(Error::Invalid);
    }
    match value {
        Some(value) if value >> (8 * width) != 0 => Err(Error::Invalid),
        _ => Ok(()),
    }
}

/// Whether two ranges of ports, inclusive, meet.
pub const fn ports_meet(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

#[cfg(kani)]
mod attach_word_proofs {
    use super::{AttachState, AttachWord};

    /// An attach word reads back what its constructor writes, for every
    /// state and every root it accepts (D-137).
    #[kani::proof]
    fn an_attach_word_reads_back_what_new_writes() {
        let state = match kani::any::<u8>() % 3 {
            0 => AttachState::Pinned,
            1 => AttachState::Attached,
            _ => AttachState::Detached,
        };
        let root: u64 = kani::any();
        if let Some(word) = AttachWord::new(state, root) {
            assert_eq!(AttachWord::from_raw(word.raw()), Some(word));
            assert_eq!(word.state(), Some(state));
            assert_eq!(word.root(), root);
        }
    }
}

#[cfg(kani)]
mod page_class_proofs {
    use super::{Leaf, PageClass};

    /// G3 (4), for every class byte and every leaf: a table or gate page is
    /// never the target of a leaf; no leaf of an accepted class is both
    /// writable and executable; a global read-only page is never writable;
    /// a device page is never executable and always of a device type.
    #[kani::proof]
    fn a_class_refuses_what_g3_refuses() {
        let raw: u8 = kani::any();
        let leaf = Leaf {
            write: kani::any(),
            exec: kani::any(),
            device_type: kani::any(),
        };
        let Some(class) = PageClass::from_raw(raw) else {
            return;
        };
        if class.permits(leaf) {
            assert!(!(leaf.write && leaf.exec));
            assert!(
                class == PageClass::DATA
                    || class == PageClass::GLOBAL_READ_ONLY
                    || class == PageClass::DEVICE
            );
            assert!(class != PageClass::GLOBAL_READ_ONLY || !leaf.write);
            assert!(class != PageClass::DEVICE || (!leaf.exec && leaf.device_type));
            assert!(class != PageClass::DATA || !leaf.device_type);
        }
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn any_owner() -> Owner {
        match kani::any::<u8>() % 3 {
            0 => Owner::Node(kani::any::<u16>() % NODES_MAX as u16),
            1 => Owner::Unclaimed,
            _ => Owner::Reserve(kani::any::<u16>() % RESERVES_MAX as u16),
        }
    }

    /// A word with the fields given and no check computed: the check is a
    /// product of 64 bits, which the solver bit-blasts (more than 2 minutes
    /// even with a generation of 8 bits). The check itself is tested over
    /// random and extreme words in `tests.rs`.
    fn unchecked(state: OwnState, owner: Owner, generation: u32) -> Ownership {
        Ownership(state as u64 | (owner.code() as u64) << 16 | (generation as u64) << 32)
    }

    #[kani::proof]
    fn ownership_fields_read_back() {
        let (owner, generation) = (any_owner(), kani::any::<u32>());
        let state = if kani::any() {
            OwnState::Free
        } else {
            OwnState::Taken
        };
        let word = unchecked(state, owner, generation);
        assert!(word.owner() == owner && word.generation() == generation);
        assert!(word.state() as u8 == state as u8);
        assert!(Owner::from_code(owner.code()) == Some(owner));
    }

    #[kani::proof]
    fn ownership_refuses_state_3_and_none_is_0() {
        let raw: u64 = kani::any();
        assert!(Ownership::from_raw(raw | 0b11).is_none());
        assert!(Ownership::none().raw() == 0);
        assert!(Ownership::from_raw(0) == Some(Ownership::none()));
        // A word of state 0 other than 0 is refused before any check.
        if raw & 0b11 == 0 && raw != 0 {
            assert!(Ownership::from_raw(raw).is_none());
        }
    }

    #[kani::proof]
    fn may_take_accepts_exactly_rules_1_to_4() {
        let word = match kani::any::<u8>() % 3 {
            0 => Ownership::none(),
            1 => unchecked(OwnState::Free, any_owner(), 0),
            _ => unchecked(OwnState::Taken, any_owner(), kani::any()),
        };
        let (node, generation): (u16, u32) = (kani::any(), kani::any());
        kani::assume((node as usize) < NODES_MAX);
        let Some(owner) = Member::from_raw(kani::any()) else {
            return;
        };
        let holds: bool = kani::any();
        let accepted = word.may_take(node, generation, owner, holds);
        let free = matches!(word.state(), OwnState::Free);
        let taken = matches!(word.state(), OwnState::Taken);
        let dead = matches!(owner.state(), MemberState::Dead);
        let over = taken && owner.generation() != word.generation();
        let expected = match word.owner() {
            _ if !free && !taken => false,
            Owner::Node(o) if o == node => free || word.generation() != generation,
            Owner::Node(_) => dead || over,
            Owner::Unclaimed => true,
            Owner::Reserve(_) => holds,
        };
        assert!(accepted == expected);
    }

    #[kani::proof]
    fn a_stored_class_is_a_class_and_never_gate_global_or_device() {
        let raw: u8 = kani::any();
        if let Some(class) = PageClass::stored(raw) {
            assert!(PageClass::from_raw(raw) == Some(class));
            assert!(raw != 1 && raw != 2 && raw != 3);
        }
    }

    #[kani::proof]
    fn member_and_role_words_read_back() {
        let raw: u64 = kani::any();
        if let Some(member) = Member::from_raw(raw) {
            let again = Member::new(
                member.state(),
                member.evidence(),
                member.driver(),
                member.generation(),
            )
            .with_class(member.class());
            assert!(again == member);
            assert!(member.class() < 3);
        }
        if let Some(role) = RoleWord::from_raw(raw) {
            let again = match role.state() {
                RoleState::Vacant => RoleWord::vacant(role.epoch()),
                RoleState::Held => RoleWord::held(role.holder(), role.epoch()),
                RoleState::Handing => RoleWord::handing(role.holder(), role.target(), role.epoch()),
            };
            assert!(role.keeping_failed(again) == role);
            assert!(again.failed_now().failed() && !again.failed());
        }
        let (holder, epoch): (u16, u32) = (kani::any(), kani::any());
        kani::assume(epoch <= ROLE_EPOCH_MASK);
        let held = RoleWord::held(holder, epoch);
        assert!(RoleWord::from_raw(held.raw()) == Some(held));
        assert!(held.holder() == holder && held.epoch() == epoch);
    }

    #[kani::proof]
    fn the_transition_check_accepts_exactly_the_rows() {
        let (a, b): (u64, u64) = (kani::any(), kani::any());
        let (Some(from), Some(to)) = (Member::from_raw(a), Member::from_raw(b)) else {
            return;
        };
        use MemberState::*;
        let same = from.generation() == to.generation();
        match member_transition(from, to) {
            Some(Transition::Start) => {
                assert!(from.state() == Dead && to.state() == Starting);
                assert!(to.generation() == from.generation().wrapping_add(1));
            }
            Some(Transition::Resume) => {
                assert!(from.state() == Sealed && to.state() == Starting && same)
            }
            Some(Transition::Up) => assert!(from.state() == Starting && to.state() == Live && same),
            Some(Transition::Seal) => {
                assert!(from.state() == Live && to.state() == Sealing && same)
            }
            Some(Transition::Sealed) => {
                assert!(from.state() == Sealing && to.state() == Sealed && same)
            }
            Some(Transition::Accuse) => {
                assert!(from.writes() || from.state() == Sealed);
                assert!(to.state() == Fencing && same && to.evidence() != Evidence::None);
            }
            Some(Transition::Abandon) => {
                assert!(from.state() == Sealing && to.state() == Live && same)
            }
            Some(Transition::Redrive) => {
                assert!(matches!(from.state(), Fencing | Unfenced) && to.state() == from.state());
                assert!(from.driver() != to.driver() && same);
            }
            Some(Transition::Fence) => {
                assert!(from.state() == Fencing && to.state() == Fenced && same)
            }
            Some(Transition::Unfence) => {
                assert!(from.state() == Fencing && to.state() == Unfenced && same)
            }
            Some(Transition::LateFence) => {
                assert!(from.state() == Unfenced && to.state() == Fenced && same)
            }
            Some(Transition::Reclaim) => {
                assert!(from.state() == Fenced && to.state() == Dead && same)
            }
            None => {}
        }
        if matches!(from.state(), Fenced | Unfenced | Dead)
            && matches!(to.state(), Live | Sealing | Sealed)
        {
            assert!(member_transition(from, to).is_none());
        }
    }

    fn any_member() -> Member {
        let state = match kani::any::<u8>() % 8 {
            0 => MemberState::Dead,
            1 => MemberState::Starting,
            2 => MemberState::Live,
            3 => MemberState::Sealing,
            4 => MemberState::Sealed,
            5 => MemberState::Fencing,
            6 => MemberState::Fenced,
            _ => MemberState::Unfenced,
        };
        Member::new(state, Evidence::None, DRIVER_NONE, kani::any::<u8>() as u32)
            .with_class(kani::any::<u8>() % 3)
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn successor_is_a_live_node_of_the_best_class() {
        let members = [any_member(), any_member(), any_member()];
        let holder = kani::any::<usize>() % 3;
        let live = |i: usize| members[i].state() == MemberState::Live;
        match successor(&members, holder) {
            Some(s) => {
                assert!(s < 3 && live(s));
                assert!(best_class(&members) == Some(members[s].class()));
                for i in 0..3 {
                    assert!(!live(i) || members[i].class() <= members[s].class());
                }
            }
            None => assert!(!live(0) && !live(1) && !live(2)),
        }
    }

    /// A core word of a node at the full widths of its fields (a slot of 16
    /// bits, a generation of 32, the mark, the flag unusable): one of the
    /// two constructors that name a node, marked or not, unusable or not.
    /// Returns the word, its fields and the two flags chosen.
    fn any_named_word() -> (CoreWord, u16, u32, bool, bool) {
        let (slot, generation): (u16, u32) = (kani::any(), kani::any());
        let (mark, unusable): (bool, bool) = (kani::any(), kani::any());
        let word = match kani::any() {
            true => CoreWord::joining(slot, generation),
            false => CoreWord::owned(slot, generation),
        };
        let word = if mark { word.marked() } else { word };
        let word = if unusable { word.unusable() } else { word };
        (word, slot, generation, mark, unusable)
    }

    /// A core word read back, at the full widths of `CoreWord` (the eighth
    /// model review found the proof narrowed to 4 bits of slot and of
    /// generation). `from_raw` accepts a word when the check it recomputes
    /// from the word's base (the word without its check bits) equals the
    /// check stored; the constructors store the check of the base they
    /// build. So a word reads back if (1) its fields are those given and
    /// (2) the base `from_raw` recovers is the base the constructor hashed:
    /// the check, a pure function, then gives the same value on both sides.
    /// Proved at the full widths, without the product of 64 bits in the
    /// cone of either property, which is what kept the end-to-end proof
    /// from ending: (1) here, (2) in `core_words_keep_their_base`, and the
    /// refusal of a flipped check bit in `core_words_with_a_check_bit_flipped_are_refused`.
    /// `core_words_read_back_end_to_end` runs the real product, end to end,
    /// with a slot and a generation of 4 bits.
    #[kani::proof]
    fn core_words_read_back() {
        let (word, slot, generation, mark, unusable) = any_named_word();
        assert!(word.slot() == slot && word.generation() == generation);
        assert!(word.names(slot, generation));
        assert!(word.state() == CoreState::Joining || word.state() == CoreState::Owned);
        assert!(word.is_marked() == mark && word.is_unusable() == unusable);
        let flag: bool = kani::any();
        let free = CoreWord::free(flag);
        assert!(free.state() == CoreState::Free && free.slot() == CORE_NO_SLOT);
        assert!(free.generation() == 0 && !free.is_marked());
        assert!(free.is_unusable() == flag);
    }

    /// (2): the base `from_raw` recovers from a word a constructor wrote is
    /// the base the constructor built from its fields, at the full widths.
    #[kani::proof]
    fn core_words_keep_their_base() {
        let (word, slot, generation, mark, unusable) = any_named_word();
        // The base from the inputs chosen, not from the word itself.
        let state = word.state() as u64;
        let built = state
            | if mark { CORE_MARK } else { 0 }
            | if unusable { CORE_UNUSABLE } else { 0 }
            | (slot as u64) << 16
            | (generation as u64) << 32;
        assert!(CoreWord::base(word.raw()) == built);
        assert!(CoreWord::base(word.raw()) & CoreWord::CHECK == 0);
    }

    /// A word with one check bit flipped has the same base, so the check
    /// `from_raw` recomputes is the stored one, which the flipped word no
    /// longer holds: it is refused. At the full widths.
    #[kani::proof]
    fn core_words_with_a_check_bit_flipped_are_refused() {
        let (word, _, _, _, _) = any_named_word();
        let bit: u32 = kani::any();
        kani::assume(bit >= 4 && bit < 16);
        let flipped = word.raw() ^ 1 << bit;
        assert!(CoreWord::base(flipped) == CoreWord::base(word.raw()));
        assert!(flipped & CoreWord::CHECK != word.raw() & CoreWord::CHECK);
    }

    /// The real check end to end, with a slot and a generation of 4 bits
    /// each (the solver's time grows with the bits of the product).
    #[kani::proof]
    fn core_words_read_back_end_to_end() {
        let (slot, generation): (u8, u8) = (kani::any(), kani::any());
        let (slot, generation) = (u16::from(slot & 0xF), u32::from(generation & 0xF));
        let word = match kani::any::<u8>() % 3 {
            0 => CoreWord::free(kani::any()),
            1 => CoreWord::joining(slot, generation),
            _ => CoreWord::owned(slot, generation),
        };
        let word = if kani::any() { word.marked() } else { word };
        assert!(CoreWord::from_raw(word.raw()) == Some(word));
        let bit: u32 = kani::any();
        kani::assume(bit >= 4 && bit < 16);
        assert!(CoreWord::from_raw(word.raw() ^ 1 << bit).is_none());
    }

    /// The word of a fence record that holds the last stop sent to a core:
    /// by its core number modulo 64, always inside the record, and two
    /// cores of one window of 64 core numbers never share one (D-134).
    #[kani::proof]
    fn fence_record_stop_index_is_inside_and_one_per_core_of_a_window() {
        let (a, b): (usize, usize) = (kani::any(), kani::any());
        assert!(FenceRecord::stop_index(a) < FENCE_RECORD_CORES);
        if a / FENCE_RECORD_CORES == b / FENCE_RECORD_CORES && a != b {
            assert!(FenceRecord::stop_index(a) != FenceRecord::stop_index(b));
        }
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn the_barrier_is_monotone() {
        let generation: u32 = kani::any();
        let mut before = [BarrierView {
            member: any_member(),
            acked: kani::any(),
            maps: kani::any(),
            acknowledged_by_all: kani::any(),
        }; 2];
        before[1] = BarrierView {
            member: any_member(),
            acked: kani::any(),
            maps: kani::any(),
            acknowledged_by_all: kani::any(),
        };
        let mut after = before;
        for y in after.iter_mut() {
            if kani::any() {
                y.acked = generation;
            }
            if kani::any() {
                y.maps = false;
            }
            if kani::any() {
                y.acknowledged_by_all = true;
            }
        }
        let clear: bool = kani::any();
        if barrier(generation, &before, clear) {
            assert!(barrier(generation, &after, clear));
        }
    }

    #[kani::proof]
    fn floors_are_whole_chunks_without_overflow() {
        let boot = BootFloors {
            floor_node: kani::any(),
            reserve_bytes: kani::any(),
            reserve_per_gib: kani::any(),
            reserves: kani::any(),
            plain_roles: 0,
        };
        let (ordinary, n): (u64, u64) = (kani::any(), kani::any());
        let chunk = if kani::any() { 1u64 << 21 } else { 1u64 << 25 };
        if let Some(f) = floors(ordinary, n, boot, chunk) {
            assert!(f.floor % chunk == 0 && f.reserve % chunk == 0);
            assert!(f.floor >= boot.floor_node && f.reserve >= boot.reserve_bytes);
            let total = f.floor as u128 * n as u128 + f.reserve as u128 * boot.reserves as u128;
            assert!(total == f.total as u128 && f.total <= ordinary);
        }
    }

    #[kani::proof]
    #[kani::unwind(10)]
    fn module_frames_cover_the_module_in_order() {
        let geometry = Geometry {
            page_shift: [12, 14, 16, 0],
            entries: 4,
            levels: 2,
            user_end: 1 << 20,
        };
        let (phys, len): (u64, u64) = (kani::any(), kani::any());
        kani::assume(phys % 4096 == 0 && phys < 1 << 20 && len >= 1 && len <= 8 * 4096);
        let mapped: bool = kani::any();
        let virt: u64 = kani::any();
        kani::assume(virt % 4096 == 0 && virt < 1 << 20);
        let virt = if mapped { Some(virt) } else { None };
        let end = len.div_ceil(4096) * 4096;
        let mut at = 0;
        let mut frames = module_frames(phys, virt, len, geometry);
        for _ in 0..8 {
            let Some(frame) = frames.next() else { break };
            let bytes = geometry.bytes(frame.size).unwrap();
            assert!(frame.offset == at && frame.pages >= 1 && frame.pages <= 4);
            assert!((phys + at) % bytes == 0);
            assert!(virt.is_none_or(|v| (v + at) % bytes == 0));
            at += frame.pages * bytes;
            assert!(at <= end);
        }
        assert!(frames.next().is_none() && at == end);
    }

    #[kani::proof]
    fn clearing_takes_between_one_and_two_calls_per_64_kb() {
        let (watermark, clean, start, shift): (u64, u64, u64, u32) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(shift >= 3 && shift <= 40);
        let bytes = 1u64 << shift;
        kani::assume(start & (bytes - 1) == 0 && start >= watermark && start - watermark < bytes);
        kani::assume(start < 1 << 50 && watermark <= clean && clean <= start);
        let end = start + bytes;
        let step = clear_step(clean, end);
        assert!(step > clean && step - clean <= CLEAR_STEP_BYTES && step <= end);
        let steps = clear_steps(clean, end);
        assert!(steps >= bytes.div_ceil(CLEAR_STEP_BYTES));
        assert!(steps <= (2 * bytes).div_ceil(CLEAR_STEP_BYTES));
    }

    #[kani::proof]
    fn port_checks_follow_the_contract() {
        let (first, last, port, width, value): (u64, u64, u64, u64, u64) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        let Some(range) = IoPortRange::new(first, last) else {
            assert!(first > last || last > 0xFFFF);
            return;
        };
        assert!(range.first() == first && range.last() == last);
        let out: bool = kani::any();
        let value = if out { Some(value) } else { None };
        if ioport_check(range, port, width, value).is_ok() {
            assert!(width == 1 || width == 2 || width == 4);
            assert!(first <= port && port + width - 1 <= last);
            assert!(value.is_none_or(|v| v < 1 << (8 * width)));
        } else {
            // In u128, so that a port near 2^64 does not overflow here.
            let end = port as u128 + width as u128;
            let fits = matches!(width, 1 | 2 | 4) && first <= port && end - 1 <= last as u128;
            assert!(!fits || value.is_some_and(|v| v >> (8 * width) != 0));
        }
    }
}
