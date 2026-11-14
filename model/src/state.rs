use std::collections::{BTreeMap, VecDeque};

use dodos_abi::{Header, ObjectKind, PageSize, Rights};

use crate::types::{
    CapId, CapTransfer, KernelConfig, Message, ObjectId, ROOT_ASPACE_SLOT, ROOT_CSPACE_SLOT,
    ROOT_DEVICE_UNTYPED_SLOT, ROOT_THREAD_SLOT, ROOT_UNTYPED_SLOT, SlotIndex, SlotRange, ThreadId,
    ThreadRegs,
};

/// State of a single capability slot in a flat `CSpace`.
// CONTRACT-GAP: `kernel-interface.md` (Rules, Errors) distinguishes `gone`
// (revoked or deleted object) from `invalid` (bad slot) without specifying
// how a flat slot table preserves the distinction; we track `Revoked` as a
// tombstone distinct from `Empty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Empty,
    Revoked,
    Live(CapId),
}

/// Relationship between a capability and its parent in the derivation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Boot root capability (no parent).
    Root,
    /// Created via `cap_derive` or IPC `CapTransfer::Derive` (`rights <= parent.rights`).
    Derived,
    /// Initial capability created by `untyped_retype` under an `Untyped` capability.
    // CONTRACT-GAP: `kernel-interface.md` does not explicitly state how
    // `untyped_retype` links new objects into the parent `Untyped` capability's
    // derivation tree; following seL4, retyped objects sit as `Retyped`
    // children so `cap_revoke` on the `Untyped` revokes everything allocated
    // from it, without restricting object rights to the `Untyped`'s rights.
    Retyped,
}

/// One node in the global capability derivation tree (CDT).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapEntry {
    pub location: (ObjectId, SlotIndex),
    pub object: ObjectId,
    pub rights: Rights,
    pub badge: u64,
    pub parent: Option<CapId>,
    pub edge: EdgeKind,
    pub children: Vec<CapId>,
}

/// An untyped physical memory range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntypedObject {
    pub size: u64,
    pub watermark: u64,
    pub is_device: bool,
    pub backed_count: usize,
    pub parent_untyped: Option<ObjectId>,
}

/// A flat capability space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CSpaceObject {
    pub slots: Vec<Slot>,
    pub parent_untyped: Option<ObjectId>,
}

/// Minimal address space stub (only creation/binding needed on one core).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ASpaceObject {
    pub parent_untyped: Option<ObjectId>,
}

/// Minimal physical frame run stub (only retype accounting needed on one core).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameObject {
    pub page_size: PageSize,
    pub count: usize,
    pub is_device: bool,
    pub parent_untyped: Option<ObjectId>,
}

/// Synchronous IPC rendezvous endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointObject {
    pub send_queue: VecDeque<ThreadId>,
    pub recv_queue: VecDeque<ThreadId>,
    pub parent_untyped: Option<ObjectId>,
}

/// Asynchronous notification bitmask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyObject {
    pub bits: u64,
    pub wait_queue: VecDeque<ThreadId>,
    pub parent_untyped: Option<ObjectId>,
}

/// One-shot reply capability target state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyState {
    Unarmed,
    /// Bound to a receiver currently blocked waiting on an endpoint.
    WaitingRecv(ThreadId),
    Armed {
        caller: ThreadId,
        request_caps_dropped: bool,
        reply_recv_slots: SlotRange,
    },
    /// The caller was destroyed or its wait was cancelled before a reply arrived.
    CallerGone,
}

/// One-shot reply object (64 bytes in `kernel-interface.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyObject {
    pub state: ReplyState,
    pub parent_untyped: Option<ObjectId>,
}

/// Message buffered inside a thread while it is `BlockedOnSend`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMessage {
    pub header: Header,
    pub words: Vec<u64>,
    pub caps: Vec<CapTransfer>,
    pub badge: u64,
    pub reply_recv_slots: SlotRange,
}

impl PendingMessage {
    pub fn from_message(msg: &Message, badge: u64, reply_recv_slots: SlotRange) -> Self {
        Self {
            header: msg.header,
            words: msg.words.clone(),
            caps: msg.caps.clone(),
            badge,
            reply_recv_slots,
        }
    }
}

/// Source object a `BlockedOnReceive` thread is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvSource {
    Endpoint(ObjectId),
    Notify(ObjectId),
}

/// Execution / IPC state of a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Suspended,
    Runnable,
    BlockedOnSend {
        endpoint: ObjectId,
        msg: PendingMessage,
    },
    BlockedOnReceive {
        source: RecvSource,
        reply_obj: Option<ObjectId>,
        recv_slots: SlotRange,
    },
    BlockedOnReply {
        reply_obj: ObjectId,
    },
}

/// Kernel thread object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadObject {
    pub state: ThreadState,
    pub cspace: Option<ObjectId>,
    pub aspace: Option<ObjectId>,
    pub fault_ep: Option<ObjectId>,
    pub regs: ThreadRegs,
    pub parent_untyped: Option<ObjectId>,
}

/// Any live kernel object in the one-core model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelObject {
    Untyped(UntypedObject),
    CSpace(CSpaceObject),
    ASpace(ASpaceObject),
    Frame(FrameObject),
    Thread(ThreadObject),
    Endpoint(EndpointObject),
    Notify(NotifyObject),
    Reply(ReplyObject),
}

impl KernelObject {
    pub const fn kind(&self) -> ObjectKind {
        match self {
            Self::Untyped(_) => ObjectKind::Untyped,
            Self::CSpace(_) => ObjectKind::CSpace,
            Self::ASpace(_) => ObjectKind::ASpace,
            Self::Frame(_) => ObjectKind::Frame,
            Self::Thread(_) => ObjectKind::Thread,
            Self::Endpoint(_) => ObjectKind::Endpoint,
            Self::Notify(_) => ObjectKind::Notify,
            Self::Reply(_) => ObjectKind::Reply,
        }
    }

    pub const fn parent_untyped(&self) -> Option<ObjectId> {
        match self {
            Self::Untyped(o) => o.parent_untyped,
            Self::CSpace(o) => o.parent_untyped,
            Self::ASpace(o) => o.parent_untyped,
            Self::Frame(o) => o.parent_untyped,
            Self::Thread(o) => o.parent_untyped,
            Self::Endpoint(o) => o.parent_untyped,
            Self::Notify(o) => o.parent_untyped,
            Self::Reply(o) => o.parent_untyped,
        }
    }
}

/// Complete deterministic state of the one-core kernel reference model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelState {
    pub config: KernelConfig,
    pub objects: BTreeMap<ObjectId, KernelObject>,
    pub caps: BTreeMap<CapId, CapEntry>,
    pub next_object_id: u64,
    pub next_cap_id: u64,
}

impl KernelState {
    /// Creates a bootstrapped one-core kernel state with the initial root thread
    /// and its start-up capabilities.
    pub fn new(config: KernelConfig) -> Self {
        let mut state = Self {
            config,
            objects: BTreeMap::new(),
            caps: BTreeMap::new(),
            next_object_id: 1,
            next_cap_id: 1,
        };

        let cspace_slots = config.init_cspace_slots.max(8);
        let cspace_id = state.alloc_object_id();
        let thread_id = state.alloc_object_id();
        let untyped_id = state.alloc_object_id();
        let aspace_id = state.alloc_object_id();

        let mut slots = vec![Slot::Empty; cspace_slots];

        let cspace_cap = state.alloc_cap_id();
        slots[ROOT_CSPACE_SLOT] = Slot::Live(cspace_cap);
        state.caps.insert(
            cspace_cap,
            CapEntry {
                location: (cspace_id, ROOT_CSPACE_SLOT),
                object: cspace_id,
                rights: Rights::ALL,
                badge: 0,
                parent: None,
                edge: EdgeKind::Root,
                children: Vec::new(),
            },
        );

        let thread_cap = state.alloc_cap_id();
        slots[ROOT_THREAD_SLOT] = Slot::Live(thread_cap);
        state.caps.insert(
            thread_cap,
            CapEntry {
                location: (cspace_id, ROOT_THREAD_SLOT),
                object: thread_id,
                rights: Rights::ALL,
                badge: 0,
                parent: None,
                edge: EdgeKind::Root,
                children: Vec::new(),
            },
        );

        let untyped_cap = state.alloc_cap_id();
        slots[ROOT_UNTYPED_SLOT] = Slot::Live(untyped_cap);
        state.caps.insert(
            untyped_cap,
            CapEntry {
                location: (cspace_id, ROOT_UNTYPED_SLOT),
                object: untyped_id,
                rights: Rights::ALL,
                badge: 0,
                parent: None,
                edge: EdgeKind::Root,
                children: Vec::new(),
            },
        );

        if let Some(dev_bytes) = config.init_device_untyped_bytes.filter(|&b| b > 0) {
            let dev_id = state.alloc_object_id();
            state.objects.insert(
                dev_id,
                KernelObject::Untyped(UntypedObject {
                    size: dev_bytes,
                    watermark: 0,
                    is_device: true,
                    backed_count: 0,
                    parent_untyped: None,
                }),
            );
            let dev_cap = state.alloc_cap_id();
            slots[ROOT_DEVICE_UNTYPED_SLOT] = Slot::Live(dev_cap);
            state.caps.insert(
                dev_cap,
                CapEntry {
                    location: (cspace_id, ROOT_DEVICE_UNTYPED_SLOT),
                    object: dev_id,
                    rights: Rights::ALL,
                    badge: 0,
                    parent: None,
                    edge: EdgeKind::Root,
                    children: Vec::new(),
                },
            );
        }

        let aspace_cap = state.alloc_cap_id();
        slots[ROOT_ASPACE_SLOT] = Slot::Live(aspace_cap);
        state.caps.insert(
            aspace_cap,
            CapEntry {
                location: (cspace_id, ROOT_ASPACE_SLOT),
                object: aspace_id,
                rights: Rights::ALL,
                badge: 0,
                parent: None,
                edge: EdgeKind::Root,
                children: Vec::new(),
            },
        );

        state.objects.insert(
            cspace_id,
            KernelObject::CSpace(CSpaceObject {
                slots,
                parent_untyped: None,
            }),
        );
        state.objects.insert(
            untyped_id,
            KernelObject::Untyped(UntypedObject {
                size: config.init_untyped_bytes.max(1),
                watermark: 0,
                is_device: false,
                backed_count: 0,
                parent_untyped: None,
            }),
        );
        state.objects.insert(
            aspace_id,
            KernelObject::ASpace(ASpaceObject {
                parent_untyped: None,
            }),
        );
        state.objects.insert(
            thread_id,
            KernelObject::Thread(ThreadObject {
                state: ThreadState::Runnable,
                cspace: Some(cspace_id),
                aspace: Some(aspace_id),
                fault_ep: None,
                regs: ThreadRegs::default(),
                parent_untyped: None,
            }),
        );

        state
    }

    /// Returns the `ThreadId` of the initial root thread (if still live).
    pub fn root_thread(&self) -> Option<ThreadId> {
        self.objects.iter().find_map(|(&id, obj)| match obj {
            KernelObject::Thread(_) if id.0 == 2 => Some(ThreadId(id)),
            _ => None,
        })
    }

    /// Lists all currently `Runnable` threads in deterministic `ThreadId` order.
    pub fn runnable_threads(&self) -> Vec<ThreadId> {
        self.objects
            .iter()
            .filter_map(|(&id, obj)| match obj {
                KernelObject::Thread(t) if t.state == ThreadState::Runnable => Some(ThreadId(id)),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn alloc_object_id(&mut self) -> ObjectId {
        let id = ObjectId(self.next_object_id);
        self.next_object_id += 1;
        id
    }

    pub(crate) fn alloc_cap_id(&mut self) -> CapId {
        let id = CapId(self.next_cap_id);
        self.next_cap_id += 1;
        id
    }
}
