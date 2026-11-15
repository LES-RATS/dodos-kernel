//! The machine table (Memory and devices between nodes): its fixed part,
//! written once at machine start-up, and its living part, written only
//! through the write gate of a node. One table serves every node of a
//! machine; a node that runs alone holds it.

use dodos_abi::{
    DeviceEntry, Geometry, Governs, Member, MemberState, Owner, Ownership, RoleKind, RoleState,
    RoleWord,
};
use std::collections::{BTreeMap, BTreeSet};

/// The fixed record of a role: its kind, what it governs, and the index of
/// that object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoleRecord {
    pub(crate) kind: RoleKind,
    pub(crate) governs: Governs,
    pub(crate) object: u16,
}

/// The state of a grant entry (Grants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantState {
    Live,
    Revoking,
    /// Freed by step R3 of a reclaim, its grantee fenced: the frames stay
    /// pinned until the grantee is dead (model choice: the entry stays
    /// used, so that its anchor keeps its place).
    Pinned,
}

/// What an observation records (Detection): a core's heartbeat, or the
/// consumer index of a ring from the observing node to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Watched {
    Heartbeat { core: usize, heartbeat: u64 },
    Ring { consumed: u64 },
}

/// An observation of a node, written by the gate only, stamped with the
/// clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) node: u16,
    pub(crate) watched: Watched,
    pub(crate) at: u64,
}

/// A node beacon, written only by the kernel that runs that node.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NodeBeacon {
    /// The code of a failure the node found in itself, with its
    /// generation.
    pub(crate) panic: Option<(u64, u32)>,
    pub(crate) save_seal: bool,
    /// For each node, the generation whose reclaim this node finished.
    pub(crate) acked: Vec<u32>,
    /// For each node, set before the first translation to its memory.
    pub(crate) maps_of: Vec<bool>,
    pub(crate) observations: Vec<Observation>,
    /// The idle, slack, kernel and recovery time, as last published:
    /// bookkeeping of time, equal in any two states.
    pub(crate) counters: crate::Ephemeral<[u64; 4]>,
}

/// A core beacon, written only by that core.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CoreBeacon {
    /// Written at every event, and the instant of the next timer when idle:
    /// equal in any two states, as the time counters are.
    pub(crate) heartbeat: crate::Ephemeral<u64>,
    pub(crate) idle_until: crate::Ephemeral<u64>,
    /// The node whose block it runs.
    pub(crate) running: u16,
    pub(crate) vmx: bool,
    /// The fence epoch written by the park stub.
    pub(crate) parked: u64,
}

/// The node field of a core beacon while the core switches between parked
/// nodes, or runs none.
pub(crate) const NO_NODE: u16 = 0xFFFF;

/// The state of an IOMMU unit as its hardware holds it: the context entry
/// of each device, naming the node (and generation) whose iospace it uses,
/// and the node whose gate maps the unit's registers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct UnitState {
    pub(crate) entries: BTreeMap<u16, (u16, u32)>,
    pub(crate) mapped: Option<u16>,
}

/// The fence of a node, as every node reads it: the instant of its first
/// action and of the confirmed stop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FenceRecord {
    pub(crate) started: Option<u64>,
    pub(crate) confirmed: Option<u64>,
}

/// A grant entry of the granting node: the grantee and its generation,
/// the serial, the state. Its anchor is a slot of the granting node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GrantEntry {
    pub(crate) grantee: u16,
    pub(crate) generation: u32,
    pub(crate) serial: u32,
    pub(crate) state: GrantState,
}

/// A use entry of the holding node: the granting node, the grant, the
/// serial, the granting node's generation, and the leaves of its tables
/// under the grant (G6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UseEntry {
    pub(crate) granter: u16,
    pub(crate) grant: u16,
    pub(crate) serial: u32,
    pub(crate) generation: u32,
    pub(crate) leaves: u64,
}

/// The class of a page (The gate over page tables, G2). A page of memory a
/// node holds that is not listed is of class *data*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PageClass {
    /// A table page of an `aspace` or an `iospace`, its root included.
    Table,
}

/// One write of the living part, kept until the end of the event so that
/// the invariants can judge who wrote what (8.2, invariant 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WordWrite {
    pub(crate) chunk: Option<u64>,
    pub(crate) entry: Option<usize>,
    pub(crate) writer: u16,
    pub(crate) before: u64,
    pub(crate) after: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Table {
    // The fixed part.
    pub(crate) geometry: Geometry,
    pub(crate) nodes: u16,
    /// The floor of each node, as ranges [start, end).
    pub(crate) floors: Vec<Vec<(u64, u64)>>,
    /// The ranges of each reserve.
    pub(crate) reserves: Vec<Vec<(u64, u64)>>,
    pub(crate) role_records: Vec<RoleRecord>,
    /// The global read-only set: the kept ranges, the kept image, the crash
    /// areas and the feedback tables (G, class 3).
    pub(crate) kept: Vec<(u64, u64)>,
    /// The chunks whose word is *none*, fixed at machine start-up (8.2,
    /// invariant 5).
    pub(crate) none: BTreeSet<u64>,
    /// The fence class of each physical core, by core number, and whether
    /// firmware booted it.
    pub(crate) fence: Vec<u8>,
    pub(crate) booted: Vec<bool>,
    // The living part.
    /// The ownership words, one per chunk, as raw words: a stray write is a
    /// word that fails its check.
    pub(crate) owners: Vec<u64>,
    /// The map count of each chunk (G4).
    pub(crate) maps: Vec<u64>,
    /// The page classes, by page number, of the pages not of class *data*.
    pub(crate) classes: BTreeMap<u64, PageClass>,
    pub(crate) devices: Vec<DeviceEntry>,
    pub(crate) members: Vec<Member>,
    pub(crate) roles: Vec<RoleWord>,
    /// The grant table and the use table of each node, by index, and the
    /// serial of each free grant entry that was used.
    pub(crate) grants: Vec<BTreeMap<u16, GrantEntry>>,
    pub(crate) uses: Vec<BTreeMap<u16, UseEntry>>,
    pub(crate) serials: Vec<BTreeMap<u16, u32>>,
    /// Cores: the beacons, the physical core each node is placed on, the
    /// cores marked unusable, the fences, the last fence epoch, and the
    /// rings between nodes as (produced, consumed).
    pub(crate) beacons: Vec<NodeBeacon>,
    pub(crate) core_beacons: Vec<CoreBeacon>,
    pub(crate) placed: Vec<usize>,
    pub(crate) unusable: Vec<bool>,
    pub(crate) fences: Vec<FenceRecord>,
    pub(crate) fence_epoch: u64,
    pub(crate) rings: Vec<Vec<(u64, u64)>>,
    /// `STALE_NS`, `STUCK_NS` and `SWEEP_NS`, as the boot image states them
    /// in `BootMembership`.
    pub(crate) stale_ns: u64,
    pub(crate) stuck_ns: u64,
    pub(crate) sweep_ns: u64,
    /// Debugging: the crash area of this boot, the previous one (never
    /// written), and the values the ports hold.
    pub(crate) crash: crate::debug::CrashArea,
    pub(crate) previous: Option<crate::debug::CrashArea>,
    pub(crate) ports: BTreeMap<u16, u8>,
    /// The slot of a replacement kernel image holds an image.
    pub(crate) replacement: bool,
    /// The IOMMU units by their role, and each node's report of what it
    /// attached: (unit, device).
    pub(crate) units: BTreeMap<usize, UnitState>,
    pub(crate) reports: Vec<Vec<(usize, u16)>>,
    /// The admitted bandwidth of each node, as it publishes it, and the
    /// physical cores the hardware reports off (ARM64 `AFFINITY_INFO`).
    pub(crate) admitted: Vec<u64>,
    pub(crate) off: Vec<bool>,
    /// The writes of the current event.
    pub(crate) writes: crate::Ephemeral<Vec<WordWrite>>,
    /// The member and role words written in the current event, with their
    /// writer: (node, before, after) and (role, writer, before, after).
    pub(crate) member_writes: crate::Ephemeral<Vec<(u16, u16, Member, Member)>>,
    pub(crate) role_writes: crate::Ephemeral<Vec<(usize, u16, RoleWord, RoleWord)>>,
}

impl Table {
    pub(crate) fn member(&self, node: u16) -> Member {
        self.members[node as usize]
    }

    pub(crate) fn generation(&self, node: u16) -> u32 {
        self.member(node).generation()
    }

    /// The fence class of the core a node is placed on.
    pub(crate) fn fence_of(&self, node: u16) -> u8 {
        self.fence[self.placed[node as usize]]
    }

    /// Whether a node's member word lets it write: starting, live or
    /// sealing.
    pub(crate) fn writes(&self, node: u16) -> bool {
        matches!(
            self.member(node).state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing
        )
    }

    /// The gate writes a member word, by compare-and-swap.
    pub(crate) fn write_member(&mut self, writer: u16, node: u16, to: Member) {
        let before = self.members[node as usize];
        self.members[node as usize] = to;
        self.member_writes.push((node, writer, before, to));
    }

    /// The gate writes a role word.
    pub(crate) fn write_role(&mut self, writer: u16, role: usize, to: RoleWord) {
        let before = self.roles[role];
        self.roles[role] = to;
        self.role_writes.push((role, writer, before, to));
    }

    /// Whether a node's member word is dead.
    pub(crate) fn dead(&self, node: u16) -> bool {
        self.member(node).state() == MemberState::Dead
    }

    /// Whether `node` holds the role of reserve `j`.
    pub(crate) fn holds_reserve(&self, node: u16, j: u16) -> bool {
        self.role_records
            .iter()
            .zip(&self.roles)
            .any(|(record, word)| {
                record.kind == RoleKind::Reserve
                    && record.object == j
                    && word.state() == RoleState::Held
                    && word.holder() == node
            })
    }

    /// Who may take a word (rules 1 to 4 of taking), for `node`.
    pub(crate) fn may_take(&self, word: Ownership, node: u16) -> bool {
        let dead = match word.owner() {
            Owner::Node(owner) => owner < self.nodes && self.dead(owner),
            _ => false,
        };
        let holds = match word.owner() {
            Owner::Reserve(j) => self.holds_reserve(node, j),
            _ => false,
        };
        word.may_take(node, self.generation(node), dead, holds)
    }

    /// The write gate: writes an ownership word by compare-and-swap, after
    /// reading the writer's own member word. Returns false, writing
    /// nothing, if the writer's gate refuses or the word changed.
    pub(crate) fn write_owner(
        &mut self,
        writer: u16,
        chunk: u64,
        before: u64,
        after: Ownership,
    ) -> bool {
        if !self.member(writer).writes() || self.owners[chunk as usize] != before {
            return false;
        }
        self.owners[chunk as usize] = after.raw();
        self.writes.push(WordWrite {
            chunk: Some(chunk),
            entry: None,
            writer,
            before,
            after: after.raw(),
        });
        true
    }

    /// The write gate on the ownership word of a device entry.
    pub(crate) fn write_entry(&mut self, writer: u16, index: usize, entry: DeviceEntry) -> bool {
        if !self.member(writer).writes() {
            return false;
        }
        let before = self.devices[index].owner.raw();
        self.devices[index] = entry;
        self.writes.push(WordWrite {
            chunk: None,
            entry: Some(index),
            writer,
            before,
            after: entry.owner.raw(),
        });
        true
    }
}
