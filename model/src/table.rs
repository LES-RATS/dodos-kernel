//! The machine table (Memory and devices between nodes): its fixed part,
//! written once at machine start-up, and its living part, written only
//! through the write gate of a node. One table serves every node of a
//! machine; a node that runs alone holds it.

use dodos_abi::{
    DeviceEntry, GateWord, Geometry, Governs, Member, MemberState, Owner, Ownership, PageClass,
    RoleKind, RoleState, RoleWord,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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

/// Which word of the living part a write names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Word {
    /// The ownership word of a chunk.
    Chunk(u64),
    /// The ownership word of an entry of the device table.
    Entry(usize),
    /// The identifier word of a device, by segment << 16 | identifier.
    Identifier(u32),
}

/// One write of the living part, kept until the next check of the
/// invariants so that they can judge who wrote what (8.2, invariant 52),
/// with what the writer's gate read when it wrote: its own generation, the
/// member word of the word's owner when a node owns it, whether it held the
/// role of the word's reserve, and the member word of the holder that role
/// names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WordWrite {
    pub(crate) word: Word,
    pub(crate) writer: u16,
    pub(crate) generation: u32,
    pub(crate) owner: Option<Member>,
    pub(crate) holds: bool,
    pub(crate) holder: Option<Member>,
    pub(crate) before: u64,
    pub(crate) after: u64,
}

/// The machine table as one node reads it. Every node of a `System` keeps
/// its copy, equal to the others after each event; the large parts are
/// shared between the copies and copied only when one is written (they are
/// one table, so the sharing is real ownership).
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
    pub(crate) none: Arc<BTreeSet<u64>>,
    /// The fence class of each physical core, by core number, and whether
    /// firmware booted it.
    pub(crate) fence: Vec<u8>,
    pub(crate) booted: Vec<bool>,
    // The living part.
    /// The ownership words, one per chunk, as raw words: a stray write is a
    /// word that fails its check.
    pub(crate) owners: Arc<Vec<u64>>,
    /// The gate word of each chunk (G4, `GateWord` of `abi/`), raw: the map
    /// count in bits 0-31, the number of the chunk's pages not of class
    /// *data* in bits 32-44.
    pub(crate) gate_words: Arc<Vec<u64>>,
    /// The page classes, by page number, of the pages of ordinary memory
    /// not of class *data*: a page table of level k, or an iospace table.
    /// The other classes are not stored: *device* and *global read-only*
    /// follow from where a page lies, and the gate's own pages are kept.
    pub(crate) classes: Arc<BTreeMap<u64, PageClass>>,
    pub(crate) devices: Arc<Vec<DeviceEntry>>,
    /// The leaf count of each entry of the device table (G4): the leaves of
    /// its owner's tables, CPU and IOMMU, that target its pages.
    pub(crate) device_leaves: Arc<Vec<u64>>,
    /// The identifier words of the devices, by segment << 16 | identifier,
    /// as raw ownership words; one not listed is free and unclaimed.
    pub(crate) identifiers: BTreeMap<u32, u64>,
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
    pub(crate) crash: Arc<crate::debug::CrashArea>,
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
    /// The writes of ownership and identifier words since the last check of
    /// the invariants, and whether that check has run since (the next event
    /// then starts a new journal).
    pub(crate) writes: crate::Ephemeral<Vec<WordWrite>>,
    pub(crate) checked: crate::Ephemeral<std::cell::Cell<bool>>,
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

    /// The gate word of a chunk (G4).
    pub(crate) fn gate_word(&self, chunk: u64) -> GateWord {
        GateWord::from_raw(self.gate_words[chunk as usize]).expect("a gate word")
    }

    /// The map count of a chunk, 0 for a chunk beyond the table.
    pub(crate) fn map_count(&self, chunk: u64) -> u64 {
        match self.gate_words.get(chunk as usize) {
            Some(&raw) => GateWord::from_raw(raw)
                .expect("a gate word")
                .map_count()
                .into(),
            None => 0,
        }
    }

    /// The gate adds `delta` leaves to the map count of a chunk.
    pub(crate) fn add_leaves(&mut self, chunk: u64, delta: i64) {
        let Some(&raw) = self.gate_words.get(chunk as usize) else {
            return;
        };
        let word = GateWord::from_raw(raw).expect("a gate word");
        let count = u64::from(word.map_count())
            .checked_add_signed(delta)
            .expect("a map count never goes below 0");
        let count = u32::try_from(count).expect("a map count fits 32 bits");
        let word = GateWord::new(count, word.not_data()).expect("a gate word");
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
    }

    /// The gate writes the map count of a chunk (G13: 0 at a take; R6: 0).
    pub(crate) fn set_map_count(&mut self, chunk: u64, count: u32) {
        let word = self.gate_word(chunk);
        let word = GateWord::new(count, word.not_data()).expect("a gate word");
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
    }

    /// The gate writes the class of a page of ordinary memory, by page
    /// number (`None` for *data*), and the count of its chunk's pages not
    /// of class *data* in the chunk's gate word (G2).
    pub(crate) fn set_class(&mut self, page: u64, class: Option<PageClass>) {
        let before = match class {
            Some(class) => Arc::make_mut(&mut self.classes).insert(page, class),
            None => Arc::make_mut(&mut self.classes).remove(&page),
        };
        let delta = i32::from(class.is_some()) - i32::from(before.is_some());
        let chunk = (page * self.geometry.page()) >> self.geometry.chunk_shift();
        if delta == 0 || chunk as usize >= self.gate_words.len() {
            return;
        }
        let word = self.gate_word(chunk);
        let not_data = u16::try_from(i32::from(word.not_data()) + delta).expect("pages of a chunk");
        let word = GateWord::new(word.map_count(), not_data).expect("at most E pages");
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
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
        // The member word of a node owner. A code beyond the nodes of the
        // machine is refused, as a live owner of the word's generation.
        let owner = match word.owner() {
            Owner::Node(owner) if owner < self.nodes => self.member(owner),
            _ => Member::new(
                MemberState::Live,
                dodos_abi::Evidence::None,
                dodos_abi::DRIVER_NONE,
                word.generation(),
            ),
        };
        let holds = match word.owner() {
            Owner::Reserve(j) => self.holds_reserve(node, j),
            _ => false,
        };
        word.may_take(node, self.generation(node), owner, holds)
    }

    /// The record of a write of `word` by `writer`, with what its gate read.
    pub(crate) fn journal(&self, writer: u16, word: Word, before: u64, after: u64) -> WordWrite {
        let read = Ownership::from_raw(before);
        let owner = read.and_then(|w| match w.owner() {
            Owner::Node(o) if o < self.nodes => Some(self.member(o)),
            _ => None,
        });
        let reserve = read.and_then(|w| match w.owner() {
            Owner::Reserve(j) => Some(j),
            _ => None,
        });
        let holder = reserve.and_then(|j| {
            self.role_records
                .iter()
                .zip(&self.roles)
                .find(|(record, _)| record.kind == RoleKind::Reserve && record.object == j)
                .filter(|(_, word)| word.state() != RoleState::Vacant)
                .and_then(|(_, word)| self.members.get(word.holder() as usize).copied())
        });
        WordWrite {
            word,
            writer,
            generation: self.generation(writer),
            owner,
            holds: reserve.is_some_and(|j| self.holds_reserve(writer, j)),
            holder,
            before,
            after,
        }
    }

    /// Starts the journal of an event: the writes of earlier events stay
    /// until a check of the invariants has judged them.
    pub(crate) fn begin_journal(&mut self) {
        if self.checked.get() {
            self.writes.clear();
            self.checked.set(false);
        }
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
        let write = self.journal(writer, Word::Chunk(chunk), before, after.raw());
        Arc::make_mut(&mut self.owners)[chunk as usize] = after.raw();
        self.writes.push(write);
        true
    }

    /// The write gate on the ownership word of a device entry.
    pub(crate) fn write_entry(&mut self, writer: u16, index: usize, entry: DeviceEntry) -> bool {
        if !self.member(writer).writes() {
            return false;
        }
        let before = self.devices[index].owner.raw();
        let write = self.journal(writer, Word::Entry(index), before, entry.owner.raw());
        Arc::make_mut(&mut self.devices)[index] = entry;
        self.writes.push(write);
        true
    }

    /// The write gate on the identifier word of a device: *taken* by the
    /// writer's generation, or `None` for *free*, unclaimed (a word not
    /// listed).
    pub(crate) fn write_identifier(&mut self, writer: u16, key: u32, after: Option<Ownership>) {
        let unclaimed = Ownership::free(Owner::Unclaimed).raw();
        let before = self.identifiers.get(&key).copied().unwrap_or(unclaimed);
        let after_raw = after.map_or(unclaimed, Ownership::raw);
        let write = self.journal(writer, Word::Identifier(key), before, after_raw);
        match after {
            Some(word) => self.identifiers.insert(key, word.raw()),
            None => self.identifiers.remove(&key),
        };
        self.writes.push(write);
    }
}
