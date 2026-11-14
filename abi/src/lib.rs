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
/// Slots added by one `cspace_grow`: one 4 KB segment.
pub const CSPACE_SEGMENT_SLOTS: u32 = 128;
/// Capabilities removed or threads woken by one call of `cap_delete` or
/// `cap_revoke` before it returns `Again`.
pub const REMOVAL_STEP: usize = 32;
/// Slots of dying capability spaces examined by one call of `cap_delete` or
/// `cap_revoke` before it returns `Again` (The derivation tree, provisional).
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
/// Bytes of an `endpoint`, of a `reply`, of a `notify`, of a `timer` and of
/// an `irq` (Object sizes).
pub const ENDPOINT_BYTES: usize = 64;
pub const REPLY_BYTES: usize = 64;
pub const NOTIFY_BYTES: usize = 64;
pub const TIMER_BYTES: usize = 64;
pub const IRQ_BYTES: usize = 64;
/// Bytes of a `task` (Object sizes).
pub const TASK_BYTES: usize = 256;
/// Bytes of a `sched` (Object sizes).
pub const SCHED_BYTES: usize = 128;
/// Bytes of a `thread` in the one-core reference, without the vector state
/// area (Capabilities and objects, rules of `untyped_retype`).
pub const THREAD_BYTES: usize = 1024;
/// Bytes of the root table of an `aspace` and of an `iospace` (Object
/// sizes).
pub const ASPACE_BYTES: usize = 4096;
pub const IOSPACE_BYTES: usize = 4096;

/// Fixed slots of the first process. Slot 0 of every capability space is
/// never used; the other start-up capabilities begin at `START_SLOT_FIRST`.
pub const START_SLOT_CSPACE: u32 = 1;
pub const START_SLOT_ASPACE: u32 = 2;
pub const START_SLOT_THREAD: u32 = 3;
pub const START_SLOT_SCHED: u32 = 4;
pub const START_SLOT_DEBUG: u32 = 5;
pub const START_SLOT_FIRST: u32 = 16;

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
    /// Admission refused: the scheduling budgets exceed the period.
    Budget = 8,
    /// A non-blocking call found nothing.
    WouldWait = 9,
    /// Not available on this architecture or kernel version.
    Unsupported = 10,
    /// The wait was ended by `thread_cancel` (Call reference, one core:
    /// Errors, and Threads).
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
    /// Call reference, one core, Threads: ends the wait of a thread with
    /// `cancelled`. It belongs to the group of the threads; it is last here
    /// so that the calls that were there before keep their place.
    ThreadCancel,
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
    /// the receiver's side; a header given by a sender must also have no
    /// flag set (`flags() == 0`), which the caller of this function checks.
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
    fn derive_never_adds_a_right() {
        let (have, wanted): (u32, u32) = (kani::any(), kani::any());
        if let (Some(have), Some(wanted)) = (Rights::from_bits(have), Rights::from_bits(wanted)) {
            if let Ok(got) = have.derive(wanted) {
                assert!(got == wanted && have.contains(got) && have.contains(Rights::DERIVE));
            }
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
        assert_eq!(Error::Cancelled as u64, 11);
    }

    #[test]
    fn thread_cancel_comes_after_the_calls_that_were_there_before() {
        assert_eq!(Call::TraceEmit as u32, 44);
        assert_eq!(Call::ThreadCancel as u32, 45);
    }

    #[test]
    fn calls_keep_their_place() {
        use self::Call as C;
        use C::*;
        let calls = [
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
            C::Call,
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
            ThreadCancel,
        ];
        for (number, call) in calls.into_iter().enumerate() {
            assert_eq!(call as usize, number, "{call:?}");
        }
    }

    #[test]
    fn object_kinds_and_page_sizes_have_their_numbers() {
        use ObjectKind::*;
        let kinds = [
            Untyped, CSpace, ASpace, Frame, Thread, Sched, Endpoint, Reply, Notify, Timer, Irq,
            IoPorts, IoSpace, Task, Core, Debug,
        ];
        for (number, kind) in kinds.into_iter().enumerate() {
            assert_eq!(kind as usize, number, "{kind:?}");
        }
        let pages = [PageSize::Size4K, PageSize::Size2M, PageSize::Size1G];
        for (number, page) in pages.into_iter().enumerate() {
            assert_eq!(page as usize, number, "{page:?}");
            assert_eq!(page.bytes(), 1 << (12 + 9 * number));
        }
    }

    #[test]
    fn header_flags_have_their_bits() {
        assert_eq!(Header::FLAG_CAPS_DROPPED, 1);
        assert_eq!(Header::FLAG_NOTIFICATION, 2);
        assert_eq!(Header::FLAG_BITS, 22);
        let top = Header::new(0, 0, 0, (1 << 22) - 1).unwrap();
        assert_eq!(top.flags(), (1 << 22) - 1);
        assert_eq!(top.raw() >> 63, 1);
    }

    #[test]
    fn start_up_page_has_its_magic_and_kinds() {
        assert_eq!(START_MAGIC, 0x0074_7353_4f64_6f44);
        use StartKind::*;
        let kinds = [
            ObjectSizes,
            Untyped,
            Cores,
            AcpiRoot,
            UefiMemoryMap,
            Framebuffer,
            EntropySeed,
            BootTimes,
            CrashRecord,
        ];
        for (number, kind) in kinds.into_iter().enumerate() {
            assert_eq!(kind as usize, number, "{kind:?}");
        }
    }

    #[test]
    fn fields_of_the_shared_layouts_have_their_offsets() {
        use core::mem::offset_of;
        let received = [
            offset_of!(Received, header),
            offset_of!(Received, badge),
            offset_of!(Received, marks),
            offset_of!(Received, task),
        ];
        assert_eq!(received, [0, 8, 16, 24]);
        let header = [
            offset_of!(StartHeader, magic),
            offset_of!(StartHeader, version),
            offset_of!(StartHeader, entries),
        ];
        assert_eq!(header, [0, 8, 12]);
        let entry = [
            offset_of!(StartEntry, kind),
            offset_of!(StartEntry, reserved),
            offset_of!(StartEntry, offset),
            offset_of!(StartEntry, len),
        ];
        assert_eq!(entry, [0, 4, 8, 16]);
        let aligns = [
            align_of::<Received>(),
            align_of::<StartHeader>(),
            align_of::<StartEntry>(),
        ];
        assert_eq!(aligns, [8, 8, 8]);
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
    fn without_removes_only_the_named_rights() {
        let rw = Rights::READ.union(Rights::WRITE);
        assert_eq!(rw.without(Rights::WRITE), Rights::READ);
        assert_eq!(rw.without(Rights::EXEC), rw);
    }

    #[test]
    fn marks_reject_reserved_bits() {
        assert_eq!(
            Marks::from_bits(0b11),
            Ok(Marks::PRIVATE.union(Marks::UNTRUSTED))
        );
        assert_eq!(Marks::from_bits(1 << 2), Err(Error::Invalid));
    }

    #[test]
    fn slot_address_splits_into_via_and_index() {
        let a = SlotAddr::through(7, 9);
        assert_eq!((a.via(), a.index()), (7, 9));
        assert_eq!(SlotAddr::own(9).via(), 0);
        assert!(SlotAddr::NONE.is_none());
        assert!(!SlotAddr::own(START_SLOT_CSPACE).is_none());
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
