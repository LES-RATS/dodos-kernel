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

/// Message words carried in registers on x86-64. A message that fits here and
/// carries no capability takes the fast path.
pub const REG_WORDS: usize = 6;
/// Message words in all, registers plus the per-thread message page.
pub const MAX_WORDS: usize = 64;
/// Capabilities per message.
pub const MAX_CAPS: usize = 4;
/// Bytes of one capability slot.
pub const SLOT_BYTES: usize = 32;

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
    /// No free capability slot.
    NoSlot = 6,
    /// The object is in a state that forbids the call.
    Busy = 7,
    /// Admission refused: the scheduling budgets exceed the period.
    Budget = 8,
    /// A non-blocking call found nothing.
    WouldWait = 9,
    /// Not available on this architecture or kernel version.
    Unsupported = 10,
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

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The rights of a capability derived from `self` with `wanted`:
    /// `Denied` unless `self` may be derived and `wanted` adds nothing.
    pub const fn derive(self, wanted: Self) -> Result<Self, Error> {
        if self.contains(Self::DERIVE) && self.contains(wanted) {
            Ok(wanted)
        } else {
            Err(Error::Denied)
        }
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
}

/// The system calls, by group. Numbering is not part of the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    CapDerive,
    CapMove,
    CapDelete,
    CapRevoke,
    CapInspect,
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
    TaskCreate,
    TaskAttach,
    TaskMark,
    TaskRevoke,
    TaskRead,
    CoreStart,
    CoreStop,
    CoreStatus,
    DebugWrite,
    TraceEmit,
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

    /// Checks a raw word received from an untrusted caller.
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

    /// A message that fits in registers and carries no capability.
    pub const fn is_fast(self) -> bool {
        self.words() <= REG_WORDS && self.caps() == 0
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
    /// Marks of the sender's task.
    pub marks: Marks,
    /// Identifier of the sender's task, for the log.
    pub task: u64,
}

/// Header of the start-up page given to the first process.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartHeader {
    pub magic: u64,
    pub version: u32,
    pub entries: u32,
}

pub const START_MAGIC: u64 = u64::from_le_bytes(*b"DodOSst\0");

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(Error::Ok as u64, 0);
        assert_eq!(Error::Gone as u64, 1);
        assert_eq!(Error::Again as u64, 2);
        assert_eq!(Error::Unsupported as u64, 10);
    }

    #[test]
    fn derive_only_removes_rights() {
        let parent = Rights::READ.union(Rights::WRITE).union(Rights::DERIVE);
        assert_eq!(parent.derive(Rights::READ), Ok(Rights::READ));
        assert_eq!(parent.derive(parent), Ok(parent));
        assert_eq!(
            parent.derive(Rights::READ.union(Rights::EXEC)),
            Err(Error::Denied)
        );
        // Without DERIVE nothing can be derived, not even fewer rights.
        assert_eq!(Rights::READ.derive(Rights::READ), Err(Error::Denied));
        // A derived capability without DERIVE ends the chain.
        assert_eq!(Rights::READ.derive(Rights::NONE), Err(Error::Denied));
    }

    #[test]
    fn rights_reject_unknown_bits() {
        assert_eq!(Rights::from_bits(0xff), Some(Rights::ALL));
        assert_eq!(Rights::from_bits(0x100), None);
    }

    #[test]
    fn marks_only_add() {
        let m = Marks::NONE.union(Marks::UNTRUSTED);
        assert!(m.union(Marks::PRIVATE).contains(m));
        assert!(!m.contains(Marks::PRIVATE));
    }

    #[test]
    fn header_round_trips_at_the_limits() {
        let h = Header::new(u32::MAX, MAX_WORDS, MAX_CAPS, Header::FLAG_CAPS_DROPPED).unwrap();
        assert_eq!(
            (h.label(), h.words(), h.caps(), h.flags()),
            (u32::MAX, MAX_WORDS, MAX_CAPS, Header::FLAG_CAPS_DROPPED)
        );
        assert_eq!(Header::from_raw(h.raw()), Ok(h));
    }

    #[test]
    fn header_rejects_oversized_messages() {
        assert_eq!(Header::new(0, MAX_WORDS + 1, 0, 0), Err(Error::Invalid));
        assert_eq!(Header::new(0, 0, MAX_CAPS + 1, 0), Err(Error::Invalid));
        assert_eq!(Header::new(0, 0, 0, 1 << 22), Err(Error::Invalid));
        // Raw words from user space: 65 words, then 5 capabilities.
        assert_eq!(Header::from_raw(65 << 32), Err(Error::Invalid));
        assert_eq!(Header::from_raw(5 << 39), Err(Error::Invalid));
    }

    #[test]
    fn fast_path_is_registers_without_capabilities() {
        assert!(Header::new(1, REG_WORDS, 0, 0).unwrap().is_fast());
        assert!(!Header::new(1, REG_WORDS + 1, 0, 0).unwrap().is_fast());
        assert!(!Header::new(1, 0, 1, 0).unwrap().is_fast());
    }

    #[test]
    fn layouts_are_fixed() {
        assert_eq!(size_of::<Received>(), 32);
        assert_eq!(size_of::<StartHeader>(), 16);
        assert_eq!(size_of::<StartEntry>(), 24);
        assert_eq!(PageSize::Size2M.bytes(), 2 * 1024 * 1024);
    }
}
