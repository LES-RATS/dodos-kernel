use dodos_abi::{Call, Error, Header, ObjectKind, PageSize, REG_WORDS, Received, Rights};

/// Slot index within a capability space (`CSpace`).
pub type SlotIndex = usize;

/// Fixed slots populated in the initial root thread's `CSpace`.
// CONTRACT-GAP: `kernel-interface.md` (Start-up information) states the initial
// process receives its capabilities in fixed slots without assigning numbers;
// we assign slots 0..=4 deterministically.
pub const ROOT_CSPACE_SLOT: SlotIndex = 0;
pub const ROOT_THREAD_SLOT: SlotIndex = 1;
pub const ROOT_UNTYPED_SLOT: SlotIndex = 2;
pub const ROOT_DEVICE_UNTYPED_SLOT: SlotIndex = 3;
pub const ROOT_ASPACE_SLOT: SlotIndex = 4;

/// Unique monotonic identifier of a kernel object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub u64);

/// Unique identifier of a thread (wraps the `ObjectId` of the `Thread` object).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub ObjectId);

/// Unique monotonic identifier of a capability node in the derivation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapId(pub u64);

/// Configuration parameters for bootstrapping the one-core kernel model.
// CONTRACT-GAP: `kernel-interface.md` (Rules, System calls) specifies bounded
// steps returning `again` and fixed boot capabilities without numerical limits;
// we parameterise the initial capacities and bounded step count here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelConfig {
    pub init_cspace_slots: usize,
    pub init_untyped_bytes: u64,
    pub init_device_untyped_bytes: Option<u64>,
    pub revoke_step_budget: usize,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            init_cspace_slots: 64,
            init_untyped_bytes: 1024 * 1024,
            init_device_untyped_bytes: Some(64 * 1024),
            revoke_step_budget: 8,
        }
    }
}

/// Specification of an object to create via `untyped_retype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetypeSpec {
    Untyped {
        bytes: u64,
    },
    CSpace {
        slots: usize,
    },
    ASpace,
    Frame {
        page_size: PageSize,
        count: usize,
    },
    Thread,
    Endpoint,
    Reply,
    Notify,
    /// Out-of-scope object kinds (`Sched`, `Timer`, `Irq`, `IoPorts`, `IoSpace`, `Task`, `Core`, `Debug`).
    Unsupported(ObjectKind),
}

impl RetypeSpec {
    pub const fn object_kind(self) -> ObjectKind {
        match self {
            Self::Untyped { .. } => ObjectKind::Untyped,
            Self::CSpace { .. } => ObjectKind::CSpace,
            Self::ASpace => ObjectKind::ASpace,
            Self::Frame { .. } => ObjectKind::Frame,
            Self::Thread => ObjectKind::Thread,
            Self::Endpoint => ObjectKind::Endpoint,
            Self::Reply => ObjectKind::Reply,
            Self::Notify => ObjectKind::Notify,
            Self::Unsupported(kind) => kind,
        }
    }
}

/// Capability transfer mode for one entry in an IPC message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapTransfer {
    Move { slot: SlotIndex },
    Derive { slot: SlotIndex, rights: Rights },
}

/// A range of empty slots `[start .. start + count]` declared by a receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SlotRange {
    pub start: SlotIndex,
    pub count: usize,
}

impl SlotRange {
    pub const fn empty() -> Self {
        Self { start: 0, count: 0 }
    }

    pub const fn new(start: SlotIndex, count: usize) -> Self {
        Self { start, count }
    }
}

/// An IPC message supplied by a caller or sender.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub header: Header,
    pub words: Vec<u64>,
    pub caps: Vec<CapTransfer>,
}

impl Message {
    pub fn new(
        label: u32,
        words: Vec<u64>,
        caps: Vec<CapTransfer>,
        flags: u32,
    ) -> Result<Self, Error> {
        let header = Header::new(label, words.len(), caps.len(), flags)?;
        Ok(Self {
            header,
            words,
            caps,
        })
    }

    pub fn empty(label: u32) -> Self {
        Self {
            header: Header::new(label, 0, 0, 0).expect("empty header is valid"),
            words: Vec::new(),
            caps: Vec::new(),
        }
    }
}

/// Minimal thread register set modelled for `thread_read_regs` / `thread_write_regs`.
// CONTRACT-GAP: `kernel-interface.md` (Objects, System calls) does not define
// the layout of thread registers; we model a minimal portable register record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadRegs {
    pub pc: u64,
    pub sp: u64,
    pub args: [u64; REG_WORDS],
}

/// Capability metadata returned by `cap_inspect`.
// CONTRACT-GAP: `kernel-interface.md` (System calls) lists `cap_inspect`
// without defining its output fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapInfo {
    pub kind: ObjectKind,
    pub rights: Rights,
    pub badge: u64,
    pub is_root: bool,
    pub child_count: usize,
}

/// Typed representation of a kernel system call in the one-core model.
// CONTRACT-GAP: `kernel-interface.md` (System calls) and `dodos-abi` name
// the calls without defining argument structs or register layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysCall {
    CapDerive {
        src_cspace: SlotIndex,
        src_slot: SlotIndex,
        dst_cspace: SlotIndex,
        dst_slot: SlotIndex,
        rights: Rights,
        badge: u64,
    },
    CapMove {
        src_cspace: SlotIndex,
        src_slot: SlotIndex,
        dst_cspace: SlotIndex,
        dst_slot: SlotIndex,
    },
    CapDelete {
        cspace: SlotIndex,
        slot: SlotIndex,
    },
    CapRevoke {
        cspace: SlotIndex,
        slot: SlotIndex,
    },
    CapInspect {
        cspace: SlotIndex,
        slot: SlotIndex,
    },
    UntypedRetype {
        untyped: SlotIndex,
        spec: RetypeSpec,
        dst_cspace: SlotIndex,
        dst_slot: SlotIndex,
    },
    Call {
        endpoint: SlotIndex,
        msg: Message,
        reply_recv_slots: SlotRange,
    },
    ReplyRecv {
        reply: SlotIndex,
        reply_msg: Message,
        endpoint: SlotIndex,
        recv_slots: SlotRange,
    },
    Recv {
        endpoint: SlotIndex,
        reply: SlotIndex,
        recv_slots: SlotRange,
    },
    Send {
        target: SlotIndex,
        msg: Message,
    },
    Signal {
        notify: SlotIndex,
    },
    Wait {
        target: SlotIndex,
    },
    Poll {
        target: SlotIndex,
    },
    ThreadConfigure {
        thread: SlotIndex,
        cspace: SlotIndex,
        aspace: Option<SlotIndex>,
        fault_ep: Option<SlotIndex>,
    },
    ThreadReadRegs {
        thread: SlotIndex,
    },
    ThreadWriteRegs {
        thread: SlotIndex,
        regs: ThreadRegs,
    },
    ThreadResume {
        thread: SlotIndex,
    },
    ThreadSuspend {
        thread: SlotIndex,
    },
    Yield,
    /// Out-of-scope system calls (`ASpace*`, `Sched*`, `Timer*`, `Irq*`, `IoSpace*`, `Task*`, `Core*`, `Debug*`).
    Unsupported(Call),
}

impl SysCall {
    pub const fn kind(&self) -> Call {
        match self {
            Self::CapDerive { .. } => Call::CapDerive,
            Self::CapMove { .. } => Call::CapMove,
            Self::CapDelete { .. } => Call::CapDelete,
            Self::CapRevoke { .. } => Call::CapRevoke,
            Self::CapInspect { .. } => Call::CapInspect,
            Self::UntypedRetype { .. } => Call::UntypedRetype,
            Self::Call { .. } => Call::Call,
            Self::ReplyRecv { .. } => Call::ReplyRecv,
            Self::Recv { .. } => Call::Recv,
            Self::Send { .. } => Call::Send,
            Self::Signal { .. } => Call::Signal,
            Self::Wait { .. } => Call::Wait,
            Self::Poll { .. } => Call::Poll,
            Self::ThreadConfigure { .. } => Call::ThreadConfigure,
            Self::ThreadReadRegs { .. } => Call::ThreadReadRegs,
            Self::ThreadWriteRegs { .. } => Call::ThreadWriteRegs,
            Self::ThreadResume { .. } => Call::ThreadResume,
            Self::ThreadSuspend { .. } => Call::ThreadSuspend,
            Self::Yield => Call::Yield,
            Self::Unsupported(call) => *call,
        }
    }
}

/// Payload returned by a completed call or wake-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallValue {
    None,
    Inspect(CapInfo),
    Received {
        info: Received,
        words: Vec<u64>,
        caps_transferred: usize,
    },
    Reply {
        header: Header,
        words: Vec<u64>,
        caps_transferred: usize,
    },
    NotifyBits(u64),
    Regs(ThreadRegs),
}

/// Result delivered to a thread when a call finishes or wakes up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallResult {
    pub error: Error,
    pub value: CallValue,
}

impl CallResult {
    pub const fn ok() -> Self {
        Self {
            error: Error::Ok,
            value: CallValue::None,
        }
    }

    pub const fn err(error: Error) -> Self {
        Self {
            error,
            value: CallValue::None,
        }
    }

    pub const fn ok_with(value: CallValue) -> Self {
        Self {
            error: Error::Ok,
            value,
        }
    }
}

/// Immediate disposition of the calling thread after `KernelState::apply`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallStatus {
    Completed(CallResult),
    Blocked,
}

/// A thread woken to `Runnable` as a side effect of another thread's call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wakeup {
    pub thread: ThreadId,
    pub result: CallResult,
}

/// Complete observable outcome of one `KernelState::apply` step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOutcome {
    pub status: CallStatus,
    pub woken: Vec<Wakeup>,
}

impl CallOutcome {
    pub(crate) fn completed(result: CallResult) -> Self {
        Self {
            status: CallStatus::Completed(result),
            woken: Vec::new(),
        }
    }

    pub(crate) fn err(error: Error) -> Self {
        Self::completed(CallResult::err(error))
    }

    pub(crate) fn blocked(woken: Vec<Wakeup>) -> Self {
        Self {
            status: CallStatus::Blocked,
            woken,
        }
    }
}
