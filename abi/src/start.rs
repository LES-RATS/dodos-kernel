//! The start-up page and the fixed slots of the first process (Start-up
//! state). Every record is `repr(C)`, little-endian, and its size is asserted
//! in the tests.

/// Fixed slots of the first process. Slot 0 of every capability space is
/// never used; slots 12 to 15 are empty; the start-up capabilities that
/// depend on the machine begin at `START_SLOT_FIRST`.
pub const START_SLOT_CSPACE: u32 = 1;
pub const START_SLOT_ASPACE: u32 = 2;
pub const START_SLOT_THREAD: u32 = 3;
pub const START_SLOT_SCHED: u32 = 4;
pub const START_SLOT_DEBUG: u32 = 5;
pub const START_SLOT_IRQ_CONTROL: u32 = 6;
pub const START_SLOT_IOPORTS: u32 = 7;
pub const START_SLOT_TASK: u32 = 8;
pub const START_SLOT_START_PAGE: u32 = 9;
/// The frame of the message page of the first thread, which holds its
/// binding.
pub const START_SLOT_MESSAGE_PAGE: u32 = 10;
/// A copy of `START_SLOT_MESSAGE_PAGE`, which holds the mapping of the
/// message page.
pub const START_SLOT_MESSAGE_PAGE_MAP: u32 = 11;
pub const START_SLOT_FIRST: u32 = 16;
/// The first process always has at least this many empty slots.
pub const START_FREE_SLOTS: u32 = 4096;

pub const START_VERSION: u32 = 1;
pub const START_MAGIC: u64 = u64::from_le_bytes(*b"DodOSst\0");
/// Pages of the start-up page, at most.
pub const START_PAGE_MAX_PAGES: u64 = 63;
/// Bytes after the modules of the boot image that hold the start-up page and
/// the message page.
pub const START_RESERVE_BYTES: u64 = 256 * 1024;
pub const START_MESSAGE_PAGE_ADDR: u64 = 0x1000;
pub const START_PAGE_ADDR: u64 = 0x2000;
/// No segment of the first service lies below this address.
pub const FIRST_SERVICE_MIN_ADDR: u64 = 0x20_0000;
/// The shares of memory of the nodes are multiples of this.
pub const SHARE_ALIGN: u64 = 2 * 1024 * 1024;

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
    AcpiRoot = 3,
    UefiMemoryMap = 4,
    Framebuffer = 5,
    EntropySeed = 6,
    BootTimes = 7,
    CrashRecord = 8,
    Process = 9,
    Modules = 10,
    Node = 11,
    Shares = 12,
    Interrupts = 13,
    Kept = 14,
    Memory = 15,
    Iommu = 16,
}

/// Flags of `StartUntyped`.
pub const START_UNTYPED_DEVICE: u8 = 1 << 0;
pub const START_UNTYPED_IMAGE: u8 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartUntyped {
    pub base: u64,
    pub slot: u32,
    pub log2_size: u8,
    pub flags: u8,
    pub node: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartCore {
    pub apic_id: u32,
    pub node: u32,
    pub class: u32,
    /// Bit 0: the boot core.
    pub flags: u32,
    pub slot: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartModule {
    pub slot: u32,
    /// Bit 0 mapped, bit 1 read, bit 2 write, bit 3 exec.
    pub flags: u32,
    pub vaddr: u64,
    pub len: u64,
    pub name_offset: u32,
    pub name_len: u32,
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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartNode {
    pub index: u32,
    pub nodes: u32,
    pub generation: u32,
    pub boot: u32,
}

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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartInterrupts {
    pub first_vector: u32,
    pub last_vector: u32,
    /// Bit 0 interrupt remapping on, bit 1 x2APIC.
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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartBootTimes {
    pub frequency: u64,
    pub loader: u64,
    pub exit_boot_services: u64,
    pub kernel: u64,
    pub first_thread: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartMemory {
    pub user_end: u64,
    pub code_page: u64,
    pub clock_page: u64,
    /// Bit `i` for `PageSize` `i`.
    pub page_sizes: u32,
    pub iospace_width: u32,
    pub iospace_page_sizes: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartIommu {
    pub slot: u32,
    pub segment: u32,
    pub domains: u32,
    pub reserved: u32,
}
