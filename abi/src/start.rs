//! The start-up page and the fixed slots of the first process (Start-up
//! state). Every record is `repr(C)`, little-endian, and its size is asserted
//! in the tests.

/// Fixed slots of the start-up space (Start-up state, Slots). Slot 0 of
/// every capability space is never used; the start-up capabilities that
/// depend on the machine begin at `START_SLOT_FIRST`.
pub const START_SLOT_CSPACE: u32 = 1;
pub const START_SLOT_ASPACE: u32 = 2;
pub const START_SLOT_THREAD: u32 = 3;
pub const START_SLOT_SCHED: u32 = 4;
pub const START_SLOT_DEBUG: u32 = 5;
pub const START_SLOT_IRQ_CONTROL: u32 = 6;
/// The `ioports` of the node on x86-64; empty on other architectures.
pub const START_SLOT_IOPORTS: u32 = 7;
pub const START_SLOT_TASK: u32 = 8;
pub const START_SLOT_START_PAGE: u32 = 9;
/// The frame of the message page of the first thread, which holds its
/// binding.
pub const START_SLOT_MESSAGE_PAGE: u32 = 10;
/// A copy of `START_SLOT_MESSAGE_PAGE`, which holds the mapping of the
/// message page.
pub const START_SLOT_MESSAGE_PAGE_MAP: u32 = 11;
pub const START_SLOT_MEMORY_CONTROL: u32 = 12;
/// The kept frames of the living and of the fixed part of the machine
/// table, `read` and `derive` only.
pub const START_SLOT_MACHINE_LIVING: u32 = 13;
pub const START_SLOT_MACHINE_FIXED: u32 = 14;
/// The mailbox endpoint of the node, with `recv`.
pub const START_SLOT_MAILBOX: u32 = 15;
/// The `core` roots, then the `iommu` roots, the `role` roots, the feedback
/// frames, the floor untypeds, the copied segments, the kept frames of the
/// modules and the crash frames.
pub const START_SLOT_FIRST: u32 = 16;
/// The first process always has at least this many empty slots.
pub const START_FREE_SLOTS: u32 = 4096;

/// Version 3: the entries of the neutral hand-over (D-125).
pub const START_VERSION: u32 = 3;
pub const START_MAGIC: u64 = u64::from_le_bytes(*b"DodOSst\0");
/// Pages of the start-up page, at most.
pub const START_PAGE_MAX_PAGES: u64 = 63;

/// Header of the start-up page given to the first process.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartHeader {
    pub magic: u64,
    pub version: u32,
    pub entries: u32,
}

/// One entry of the start-up page; `offset` and `len` are in bytes from the
/// start of the page.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartEntry {
    pub kind: u32,
    pub reserved: u32,
    pub offset: u64,
    pub len: u64,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    ObjectSizes = 0,
    Untyped = 1,
    Cores = 2,
    FirmwareTables = 3,
    MemoryRanges = 4,
    Framebuffer = 5,
    EntropySeed = 6,
    BootTimes = 7,
    CrashRecord = 8,
    Process = 9,
    Modules = 10,
    Node = 11,
    Floors = 12,
    Interrupts = 13,
    Kept = 14,
    Memory = 15,
    Iommu = 16,
    Machine = 17,
    Roles = 18,
    Reserves = 19,
    Image = 20,
    Kinds = 21,
    Features = 22,
    Feedback = 23,
    Debug = 24,
}

/// Flags of `StartUntyped`: bit 0 device; other bits 0.
pub const START_UNTYPED_DEVICE: u8 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartUntyped {
    pub base: u64,
    pub slot: u32,
    pub log2_size: u8,
    pub flags: u8,
    pub node: u16,
}

/// Flags of `StartCore`: bit 0 the core firmware booted, bits 1-2 its fence
/// class.
pub const START_CORE_BOOTED: u16 = 1 << 0;
pub const START_CORE_FENCE_SHIFT: u32 = 1;

/// One physical core, sorted by hardware identifier, so a core's position
/// is its core number. The fields the contract lists take 28 bytes; the
/// record is 32, so a reserved word ends it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartCore {
    pub hw_id: u64,
    pub slot: u32,
    pub kind: u16,
    pub flags: u16,
    pub threads: u8,
    pub perf_highest: u8,
    pub perf_efficient: u8,
    pub efficiency_class: u8,
    pub package: u32,
    pub reserved: u32,
    pub reserved_tail: u32,
}

/// The previous crash area: flags bit 0 its stop record is valid, bit 1 at
/// least one node record is valid.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartCrash {
    pub base: u64,
    pub len: u64,
    pub sequence: u64,
    pub slot: u32,
    pub flags: u32,
}

/// Flags of `StartModule`.
pub const START_MODULE_MAPPED: u32 = 1 << 0;
pub const START_MODULE_READ: u32 = 1 << 1;
pub const START_MODULE_WRITE: u32 = 1 << 2;
pub const START_MODULE_EXEC: u32 = 1 << 3;
pub const START_MODULE_FIRST_SERVICE: u32 = 1 << 4;

/// A module: its run of slots, its address and length, its flags and its
/// name in the name area that follows the records.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartModule {
    pub first_slot: u32,
    pub slots: u32,
    pub vaddr: u64,
    pub len: u64,
    pub flags: u32,
    pub name_offset: u32,
    pub name_len: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartProcess {
    pub entry: u64,
    pub message_page: u64,
    pub start_page: u64,
    pub start_page_len: u64,
    pub slots: u32,
    pub first_empty: u32,
}

/// Flags of `StartNode`.
pub const START_NODE_FRESH: u32 = 1 << 0;
pub const START_NODE_RESUMED: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartNode {
    pub index: u32,
    pub nodes: u32,
    pub generation: u32,
    pub flags: u32,
}

/// A range of a floor (entry `Floors`) or of a reserve (entry `Reserves`,
/// whose node field holds the reserve's index).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartShare {
    pub node: u32,
    pub reserved: u32,
    pub base: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartKept {
    pub base: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartIoApic {
    pub first_gsi: u32,
    pub inputs: u32,
}

/// Flags of `StartInterrupts`.
pub const START_IRQ_REMAPPING: u32 = 1 << 0;
pub const START_IRQ_X2APIC: u32 = 1 << 1;
pub const START_IRQ_DIRECTED_EOI: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartInterrupts {
    pub first_vector: u32,
    pub last_vector: u32,
    pub flags: u32,
    pub ioapics: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartFramebuffer {
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub format: u32,
}

/// The counter at the events of the start, and the first stage that built
/// the hand-over block (D-125).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartBootTimes {
    pub frequency: u64,
    pub stage_entry: u64,
    pub firmware_end: u64,
    pub kernel: u64,
    pub first_thread: u64,
    /// `START_STAGE_*`.
    pub stage: u32,
    pub stage_version: u32,
}

/// The kind of first stage (D-125).
pub const START_STAGE_UEFI: u32 = 0;
pub const START_STAGE_DEVICE_TREE: u32 = 1;
pub const START_STAGE_UBIOS: u32 = 2;
pub const START_STAGE_LIMINE: u32 = 3;
pub const START_STAGE_OTHER: u32 = 4;

/// One firmware table of the hand-over block (`FirmwareTables`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartFirmwareTable {
    /// `START_TABLE_*`.
    pub kind: u32,
    pub reserved: u32,
    pub address: u64,
    pub len: u64,
}

pub const START_TABLE_ACPI_ROOT: u32 = 1;
pub const START_TABLE_DEVICE_TREE: u32 = 2;
pub const START_TABLE_UBIOS: u32 = 3;
pub const START_TABLE_SMBIOS: u32 = 4;

/// One memory range of the hand-over block, with a DodOS type
/// (`MemoryRanges`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartMemoryRange {
    pub base: u64,
    pub len: u64,
    /// `START_RANGE_*`.
    pub kind: u32,
    pub reserved: u32,
}

pub const START_RANGE_USABLE: u32 = 0;
pub const START_RANGE_KEPT_BY_STAGE: u32 = 1;
pub const START_RANGE_FIRMWARE: u32 = 2;
pub const START_RANGE_UNUSABLE: u32 = 3;
pub const START_RANGE_DEVICE: u32 = 4;

/// Flags of `StartMemory`.
pub const START_MEMORY_MAP_THROUGH_ROLE: u32 = 1 << 0;
pub const START_MEMORY_GATE_NO_WINDOW: u32 = 1 << 1;
pub const START_MEMORY_CONTROL_FLOW: u32 = 1 << 2;
pub const START_MEMORY_MONITOR: u32 = 1 << 3;
/// The image accepts a core of fence class 0 beside other nodes (The fence).
pub const START_MEMORY_UNFENCED_ACCEPTED: u32 = 1 << 4;

/// The values of the machine.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartMemory {
    pub user_end: u64,
    pub code_page: u64,
    pub clock_page: u64,
    /// The log2 of each page size, by index; 0 where the processor has none.
    pub page_shift: [u8; 4],
    pub table_entries: u32,
    pub levels: u32,
    pub iospace_width: u32,
    /// Bit `i` for page size index `i`.
    pub iospace_page_sizes: u32,
    pub flags: u32,
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartIommu {
    pub slot: u32,
    pub segment: u32,
    pub domains: u32,
    pub role: u32,
}

/// Where the living part lies in the frame of slot 13, and its sizes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartMachine {
    pub ownership: u32,
    pub devices: u32,
    pub gate_words: u32,
    pub page_classes: u32,
    pub members: u32,
    pub roles: u32,
    pub grants: u32,
    pub uses: u32,
    pub chunks: u64,
    pub device_capacity: u32,
    pub role_count: u32,
    pub grants_per_node: u32,
    pub layout: u32,
    /// The offset of the identifier words of the devices.
    pub identifiers: u32,
    /// The offset of the leaf counts of the device entries (G4), one u64
    /// per entry of the device table.
    pub device_leaves: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartRole {
    /// 0 unit, 1 reserve, 2 plain (`RoleKind`).
    pub kind: u8,
    /// `Governs`.
    pub governs: u8,
    pub object: u16,
    pub slot: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartImage {
    pub base: u64,
    pub len: u64,
    pub sha256: [u8; 32],
}

/// One kind of core, by kind number.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartKindRecord {
    pub features: u64,
    pub baseline: u64,
    pub id: [u64; 3],
    pub cores: u32,
    pub vector_bytes: u32,
    pub baseline_bytes: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartFeedback {
    pub package: u32,
    pub slot: u32,
    pub bytes: u32,
    pub reserved: u32,
}

/// The debugging values of the node; the ports refused to it follow, as
/// pairs of u16. The fields the contract lists take 52 bytes; the record
/// is 56, so a reserved word ends it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartDebug {
    pub build: u32,
    pub console_kind: u32,
    pub console_address: u64,
    pub crash_base: u64,
    pub crash_len: u64,
    pub slot: u32,
    pub area_bytes: u32,
    pub kernel_entries: u16,
    pub user_entries: u16,
    pub entry_bytes: u16,
    pub breakpoints: u16,
    pub watchpoints: u16,
    pub reserved: u16,
    pub reserved_tail: u32,
}
