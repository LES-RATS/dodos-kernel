//! Debugging (call reference): the debug area of a physical core, the
//! trace rings and their entries, the crash records, the console.

/// Bytes of the debug area of a physical core (D-129): seventeen blocks of
/// 4,096 bytes, a byte size that does not change with the page.
pub const DEBUG_AREA_BYTES: usize = 69_632;
/// The header of a debug area, then the core's crash record.
pub const DEBUG_HEADER_BYTES: usize = 64;
pub const CORE_CRASH_RECORD_BYTES: usize = 4_032;
/// The header of a crash area, before the debug area of core 0.
pub const CRASH_HEADER_BYTES: usize = 4_096;
/// The trace rings: entries of 64 bytes, 256 in the kernel ring at byte
/// 4,096 of the area, 768 in the user ring at byte 20,480.
pub const TRACE_ENTRY_BYTES: usize = 64;
pub const TRACE_KERNEL_ENTRIES: u64 = 256;
pub const TRACE_USER_ENTRIES: u64 = 768;
pub const TRACE_KERNEL_OFFSET: usize = 4_096;
pub const TRACE_USER_OFFSET: usize = 20_480;
/// Bytes of text in one entry (words 5 to 7).
pub const TRACE_TEXT_BYTES: u64 = 24;
/// Bytes of `debug_write`, at most.
pub const DEBUG_WRITE_MAX: u64 = 512;
/// The rate limit of the kernel ring: entries of one code per window.
pub const TRACE_BURST: u32 = 16;
pub const TRACE_BURST_NS: u64 = 1_000_000;
/// The pacing of the console: bytes per due event, and its period.
pub const CONSOLE_STEP_BYTES: usize = 16;
pub const CONSOLE_TICK_NS: u64 = 1_000_000;
/// A core that does not stop for the stub within this is shown as running.
pub const STUB_STOP_NS: u64 = 10_000_000;
/// Entries of each ring copied into a core record.
pub const CRASH_RECORD_KERNEL_ENTRIES: usize = 32;
pub const CRASH_RECORD_USER_ENTRIES: usize = 16;
/// Magic numbers of a debug area and of a crash area.
pub const DEBUG_AREA_MAGIC: u32 = u32::from_le_bytes(*b"DoDb");
pub const CRASH_AREA_MAGIC: u64 = u64::from_le_bytes(*b"DodOScr\0");
/// Version 2 since D-129: the headers hold a core number and the node slot
/// the core last ran.
pub const DEBUG_AREA_VERSION: u16 = 2;
pub const CRASH_AREA_VERSION: u32 = 2;

/// The codes of the trace rings: below 16 the kernel's, 16 and 17 the
/// user ring's.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceCode {
    Text = 1,
    DmaFault = 2,
    Suppressed = 3,
    UnitTimeout = 4,
    FaultRecorded = 5,
    Failure = 6,
    HardwareError = 7,
    StrayVector = 8,
    Transition = 9,
    BadWord = 10,
    /// A message between kernels refused: its sender may not send it.
    Refused = 11,
    UserText = 16,
    UserEmit = 17,
}

impl TraceCode {
    /// Whether the code belongs to the user ring.
    pub const fn user(self) -> bool {
        self as u16 >= 16
    }
}

/// Word 2, bit 16: the text goes on in the entry with the next sequence.
pub const TRACE_FLAG_CONTINUED: u64 = 1 << 16;
/// Word 2, bits 17-31: the node slot of the node the core ran.
pub const TRACE_SLOT_SHIFT: u32 = 17;
pub const TRACE_SLOT_MASK: u64 = 0x7fff;

/// The entry of sequence `seq` (at least 1) sits at index (seq − 1) mod
/// `entries` of its ring.
pub const fn trace_index(seq: u64, entries: u64) -> u64 {
    (seq - 1) % entries
}

/// One entry of a trace ring: 8 words.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceEntry {
    /// The sequence, at least 1; 0 while the entry is being written.
    pub seq: u64,
    pub instant: u64,
    /// The code (0-15), the flag *continued* (16), the node slot of the
    /// node the core ran (17-31), that node's generation (32-63).
    pub word2: u64,
    pub task: u64,
    pub marks: u64,
    pub payload: [u64; 3],
}

impl TraceEntry {
    pub const fn code(&self) -> u16 {
        self.word2 as u16
    }

    pub const fn continued(&self) -> bool {
        self.word2 & TRACE_FLAG_CONTINUED != 0
    }

    pub const fn generation(&self) -> u32 {
        (self.word2 >> 32) as u32
    }

    pub const fn slot(&self) -> u16 {
        (self.word2 >> TRACE_SLOT_SHIFT & TRACE_SLOT_MASK) as u16
    }

    /// Word 2 of an entry: its code, the flag *continued*, the node slot
    /// and the generation.
    pub const fn word2_of(code: u16, continued: bool, slot: u16, generation: u32) -> u64 {
        let flag = if continued { TRACE_FLAG_CONTINUED } else { 0 };
        code as u64
            | flag
            | (slot as u64 & TRACE_SLOT_MASK) << TRACE_SLOT_SHIFT
            | (generation as u64) << 32
    }
}

/// The header of a debug area: magic, version, the core number, the
/// generation and node slot of the node the core last ran, the next
/// sequences of the two rings, the suppressed entries. CONTRACT-GAP 2: the
/// fields the contract lists, with its "16 reserved bytes", take 56 bytes,
/// and the area's header is 64; 24 reserved bytes end it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugAreaHeader {
    pub magic: u32,
    pub version: u16,
    pub core: u16,
    pub generation: u32,
    pub slot: u32,
    pub next_kernel: u64,
    pub next_user: u64,
    pub suppressed: u64,
    pub reserved_tail: [u64; 3],
}

/// The reasons of a crash record.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashReason {
    Assertion = 1,
    KernelException = 2,
    DoubleFault = 3,
    HardwareError = 4,
    Fenced = 5,
    UnitTimeout = 6,
    Containment = 7,
    StartRefused = 8,
    /// Another core of the node did not answer within `CORE_ANSWER_NS`.
    CoreSilent = 9,
}

/// The core record, at offset 64 of the core's debug area (4,032 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreCrashRecord {
    /// The boot identifier of the crash area once complete; 0 while written.
    pub commit: u64,
    pub generation: u32,
    pub reason: u32,
    pub instant: u64,
    pub member: u64,
    pub detail: [u64; 32],
    pub text: [u8; 128],
    pub kernel: [TraceEntry; CRASH_RECORD_KERNEL_ENTRIES],
    pub user: [TraceEntry; CRASH_RECORD_USER_ENTRIES],
    pub reserved: [u8; 544],
}

/// The stop record of a crash area header.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopRecord {
    pub commit: u64,
    pub core: u32,
    pub reason: u32,
    pub instant: u64,
    pub text: [u8; 128],
}

/// The header of a crash area (4,096 bytes): the machine record.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrashAreaHeader {
    pub magic: u64,
    pub version: u32,
    /// The number of physical cores.
    pub cores: u32,
    pub boot_id: u64,
    pub sequence: u64,
    pub area_bytes: u32,
    pub build: u32,
    pub reserved: u64,
    pub stop: StopRecord,
    pub reserved_tail: [u8; 3896],
}

/// The two kinds of kernel build.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Production = 0,
    Development = 1,
}

/// The console devices.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    None = 0,
    Uart16550Port = 1,
    Uart16550Mmio = 2,
    Pl011 = 3,
    SbiDbcn = 4,
}

/// The numbered messages of the console.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleMessage {
    KernelStarts = 1,
    NodeLive = 2,
    NodeAccused = 3,
    NodeDead = 4,
    NodeFailed = 5,
    NodeUnfenced = 6,
    RoleMoved = 7,
    StartRefused = 8,
    MachineStops = 9,
    IommuFaults = 10,
    UnitTimeout = 11,
    HardwareErrors = 12,
    FaultRecorded = 13,
    Suppressed = 14,
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// For the two rings; a sequence below 2^32, more than any boot
    /// writes into one ring.
    #[kani::proof]
    fn the_index_of_an_entry_is_its_sequence_minus_one_mod_n() {
        let seq = kani::any::<u32>() as u64;
        let entries = if kani::any() {
            TRACE_KERNEL_ENTRIES
        } else {
            TRACE_USER_ENTRIES
        };
        kani::assume(seq >= 1);
        let index = trace_index(seq, entries);
        assert!(index < entries);
        if seq < u64::MAX {
            assert!(trace_index(seq + 1, entries) == (index + 1) % entries);
        }
        if seq > entries {
            assert!(trace_index(seq - entries, entries) == index);
        }
    }
}
