//! The machine table (Memory and devices between nodes): its fixed part,
//! written once at machine start-up, and its living part, written only
//! through the write gate of a node. One table serves every node of a
//! machine; a node that runs alone holds it.

use dodos_abi::{
    CoreState, CoreWord, DeviceEntry, GateWord, Geometry, Governs, Member, MemberState, NodeGen,
    Owner, Ownership, PageClass, RoleKind, RoleState, RoleWord,
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

/// The state of a grant entry (Grants); a free entry is not listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantState {
    Live,
    Revoking,
    /// Marked by step R3 of a reclaim, its grantee fencing: the entry stays
    /// used and its frames pinned until the grantee's generation is over.
    Pinned,
}

/// What an observation records (Detection): a core's heartbeat, or the
/// consumer index of one of the two rings from the observing node to
/// another (`user` for the user ring).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Watched {
    Heartbeat { core: usize, heartbeat: u64 },
    Ring { user: bool, consumed: u64 },
}

/// An observation of a node, written by the gate only, stamped with the
/// clock, of the generation it was taken for: it is evidence against that
/// generation only (Cores, Detection: "a core whose word names X's
/// generation"; pass 25: an observation of a generation that ended accused
/// the next, whose heartbeat, a count of its kernel's events, read the
/// value observed again).
///
/// It is stamped with the member word the gate read of the node observed,
/// and holds only while that word still reads the same state, generation
/// and resume bit; none is taken of a sealed node (D-146: an observation
/// taken while the node was sealed counted against it once it resumed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) node: u16,
    pub(crate) generation: u32,
    pub(crate) state: MemberState,
    pub(crate) resume: bool,
    pub(crate) watched: Watched,
    pub(crate) at: u64,
}

impl Observation {
    /// Whether the member word `member` still reads what the observation
    /// was stamped with.
    pub(crate) fn holds(&self, member: Member) -> bool {
        self.generation == member.generation()
            && self.state == member.state()
            && self.resume == member.resuming()
    }
}

/// A node beacon, written only by the kernel of that node, on any of its
/// cores.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NodeBeacon {
    /// The code of a failure the node found in itself, with its generation
    /// and the number of the core that found it.
    pub(crate) panic: Option<(u64, u32, u32)>,
    /// The instant of its seal.
    pub(crate) sealed_at: Option<u64>,
    /// For each node, the generation whose reclaim this node finished.
    pub(crate) acked: Vec<u32>,
    /// For each node, set before the first translation to its memory.
    pub(crate) maps_of: Vec<bool>,
    pub(crate) observations: Vec<Observation>,
    /// The consumer index of the kernel ring and of the user ring from each
    /// other node.
    pub(crate) consumed: Vec<(u64, u64)>,
}

/// A core beacon, one page per physical core, written only by that core
/// (its `parked` by the park stub and the fence signal's handler).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CoreBeacon {
    /// Written at the exit of every event, and the instant of the next
    /// timer when idle: equal in any two states, as the time counters are.
    pub(crate) heartbeat: crate::Ephemeral<u64>,
    pub(crate) idle_until: crate::Ephemeral<u64>,
    /// Set while the core is in VMX root operation (x86-64).
    pub(crate) vmx: bool,
    /// The park request the park stub or the fence handler last read.
    pub(crate) parked: u64,
    /// The start request the park stub took when it left for the entry
    /// stub, 0 when it parks: what the starter waits for (Beacons, the core
    /// beacon, gap 8).
    pub(crate) started: u64,
    /// The node slot and generation it last ran, for readers.
    pub(crate) ran: (u16, u32),
    /// Its idle, slack, kernel and recovery time, as last published.
    pub(crate) counters: crate::Ephemeral<[u64; 4]>,
}

/// One message in a ring, as the model carries it: the header the sender
/// wrote and the words after it, or bytes the receiver cannot read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RingEntry {
    /// The first entry of a message; the next `length − 1` entries are its
    /// continuation.
    Message(crate::rings::Message),
    Continued,
    /// A raw header, as a faulty or dying sender may leave it (a test's
    /// input): the receiver checks it like any other.
    Raw(dodos_abi::RingHeader),
}

/// One ring: its producer index, in its header, written by the sender only;
/// its entries. The consumer index is in the receiver's node beacon.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ring {
    pub(crate) produced: u64,
    pub(crate) entries: Vec<Option<RingEntry>>,
}

impl Ring {
    pub(crate) fn new(entries: u64) -> Self {
        Self {
            produced: 0,
            entries: vec![None; entries as usize],
        }
    }

    /// The entry at index `at` of the ring (a 64-bit count).
    pub(crate) fn at(&self, at: u64) -> Option<&RingEntry> {
        self.entries[(at % self.entries.len() as u64) as usize].as_ref()
    }

    pub(crate) fn set(&mut self, at: u64, entry: RingEntry) {
        let len = self.entries.len() as u64;
        self.entries[(at % len) as usize] = Some(entry);
    }
}

/// The two rings of an ordered pair of node slots, kept memory.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RingPair {
    pub(crate) kernel: Ring,
    pub(crate) user: Ring,
}

/// The state of an IOMMU unit as its hardware holds it: the context entry
/// of each device (by identifier: the node, its generation and the root of
/// the iospace), its remapping entries (by handle), the operations
/// submitted and not yet confirmed (each with its submission instant and
/// whether the unit will confirm it), and the node whose gate maps the
/// unit's registers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct UnitState {
    pub(crate) entries: BTreeMap<u16, (u16, u32)>,
    /// The domain number of each context entry, by device.
    pub(crate) domains: BTreeMap<u16, u16>,
    pub(crate) remap: BTreeMap<u32, (u16, u32)>,
    pub(crate) queue: Vec<(u64, u64)>,
    pub(crate) mapped: BTreeSet<u16>,
    /// An IOAPIC: the redirection entry of each line claimed, by GSI.
    pub(crate) lines: BTreeMap<u32, (u16, u32)>,
    /// The lines whose entry a rebuild wrote masked, from the mask state
    /// of their report (D-139).
    pub(crate) masked: BTreeSet<u32>,
    /// A test's input: the unit never confirms again.
    pub(crate) broken: bool,
    /// The serial of the next submission.
    pub(crate) next: u64,
}

/// A claimed entry of a core's interrupt table, as the kept memory holds
/// it: who claimed it, and the unit roles that hold its remapping entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct IrqTableEntry {
    pub(crate) node: u16,
    pub(crate) generation: u32,
    pub(crate) units: Vec<usize>,
}

/// The fence of a node, as every node reads it: the instant of its first
/// action, of the confirmed stop, and of the last stop sent to each of its
/// cores: the fence record of the node slot in the living part, written by
/// the driver's gate (The fence, the fence record, gap 6; its layout is
/// `dodos_abi::FenceRecord`): the generation it concerns, written anew by
/// the first fence action of a generation, and the last stops indexed by
/// core number mod 64 (D-134).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FenceRecord {
    pub(crate) generation: u32,
    pub(crate) started: Option<u64>,
    pub(crate) confirmed: Option<u64>,
    /// By `dodos_abi::FenceRecord::stop_index`.
    pub(crate) last_stop: BTreeMap<usize, u64>,
}

impl FenceRecord {
    /// The instant of the first fence action of `generation`, if the record
    /// concerns it.
    pub(crate) fn begun(&self, generation: u32) -> Option<u64> {
        self.started.filter(|_| self.generation == generation)
    }

    /// The instant of the last stop sent to a core, 0 for none.
    pub(crate) fn last_stop(&self, core: usize) -> u64 {
        let index = dodos_abi::FenceRecord::stop_index(core);
        self.last_stop.get(&index).copied().unwrap_or(0)
    }

    /// A stop sent to a core now.
    pub(crate) fn stopped(&mut self, core: usize, now: u64) {
        self.last_stop
            .insert(dodos_abi::FenceRecord::stop_index(core), now);
    }
}

/// RISC-V: a level source of an APLIC domain in MSI mode (the RISC-V AIA
/// 1.0, §4.7, §4.9): whether its input is high; its pending bit, set by a
/// low-to-high transition of the input or by a write of `setipnum` while
/// the input is high, cleared while the input is low and when an MSI is
/// sent; its enable bit (`setienum`, `clrienum`); the MSIs sent and not yet
/// arrived. "An MSI is sent for a specific source only when the source's
/// corresponding pending and enable bits are both one" (§4.9).
///
/// `active`: its `sourcecfg` names a mode; an inactive source (`sourcecfg`
/// 0) ignores its input and has neither pending nor enable bit (§4.5.2).
/// A claim makes it active; a mask by operation 5 and a rebuild that finds
/// it claimed by nobody make it inactive (D-147).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Aplic {
    pub(crate) high: bool,
    pub(crate) pending: bool,
    pub(crate) enabled: bool,
    pub(crate) sent: u32,
    pub(crate) active: bool,
}

impl Aplic {
    /// `sourcecfg` 0: the source inactive, its bits cleared.
    pub(crate) fn deactivate(&mut self) {
        (self.active, self.pending, self.enabled) = (false, false, false);
    }

    /// The APLIC sends an MSI as soon as the source is pending and enabled.
    pub(crate) fn forward(&mut self) {
        if self.active && self.pending && self.enabled {
            self.pending = false;
            self.sent += 1;
        }
    }

    /// The device drives the input (a test's input).
    #[cfg(test)]
    pub(crate) fn input(&mut self, high: bool) {
        self.pending = self.active && high && (self.pending || !self.high);
        self.high = high;
        self.forward();
    }
}

/// What the anchor of a grant says, in the 64 bytes of its slot inside the
/// grant entry, which every node reads: the frame (its address, page size,
/// pages and memory type), its rights and its depth. A remote frame
/// capability takes all of them from here, never from the `Send` (Grants,
/// a remote frame capability, D-141).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub(crate) frame: crate::Frame,
    pub(crate) rights: dodos_abi::Rights,
    pub(crate) depth: usize,
}

impl Anchor {
    /// An anchor of one page of ordinary memory at depth 2, for the tests
    /// that write a grant entry by hand.
    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self {
            frame: crate::Frame {
                page: dodos_abi::PageSize::Page,
                pages: 1,
                memory: dodos_abi::MemoryType::WriteBack,
                base: 0,
                kept: false,
            },
            rights: dodos_abi::Rights::READ,
            depth: 2,
        }
    }
}

/// A grant entry of the granting node: its anchor, the grantee and its
/// generation, the serial, the state, the use index and the bit *answered*
/// written from the `SendAnswer`, and the doorbell (a notify of the
/// granting node and the bits it sets). The anchor's slot is also in the
/// granting node's derivation list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GrantEntry {
    pub(crate) anchor: Anchor,
    pub(crate) grantee: u16,
    pub(crate) generation: u32,
    pub(crate) serial: u32,
    pub(crate) state: GrantState,
    pub(crate) use_index: Option<u16>,
    pub(crate) answered: bool,
    pub(crate) doorbell: Option<(crate::ObjId, u64)>,
}

/// A use entry of the holding node: the granting node, the grant, the
/// serial, the granting node's generation, the root remote capability on
/// this node (its object; none once it is removed while a failed unit kept
/// the count above 0, D-142), the leaves of its tables under the grant
/// (G6), and the flags *release pending* and *signal pending*.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UseEntry {
    pub(crate) granter: u16,
    pub(crate) grant: u16,
    pub(crate) serial: u32,
    pub(crate) generation: u32,
    pub(crate) root: Option<crate::ObjId>,
    pub(crate) leaves: u64,
    pub(crate) release_pending: bool,
    pub(crate) signal_pending: bool,
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
    /// The line word of a claimable line, by GSI (Interrupts, D-135).
    Line(u32),
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
    /// Whether the writer's member word let its gate write (starting,
    /// live or sealing), read before the write.
    pub(crate) writes: bool,
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
    /// Of each floor, its table floor: the chunks whose gate words carry
    /// the table bit from machine start-up (D-154).
    pub(crate) table_floors: Vec<Vec<(u64, u64)>>,
    /// The ranges of each reserve.
    pub(crate) reserves: Vec<Vec<(u64, u64)>>,
    pub(crate) role_records: Vec<RoleRecord>,
    /// The global read-only set: the kept ranges, the kept image, the crash
    /// areas and the feedback tables (G, class 3).
    pub(crate) kept: Vec<(u64, u64)>,
    /// The chunks whose word is *none*, fixed at machine start-up (8.2,
    /// invariant 5).
    pub(crate) none: Arc<BTreeSet<u64>>,
    /// The fence class of each physical core, by core number, whether
    /// firmware booted it, and its kind's state features and vector
    /// length (what `node_start` compares).
    pub(crate) fence: Vec<u8>,
    pub(crate) booted: Vec<bool>,
    pub(crate) state_kinds: Vec<(u32, u32)>,
    /// The cores per node the kernel image supports (entry `Node`).
    pub(crate) cores_supported: u32,
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
    /// The grant count of each entry of the device table, bits 32-44 of
    /// its leaf-count word in the living part, kept apart here (D-148).
    pub(crate) device_grants: Arc<Vec<u16>>,
    /// The identifier words of the devices, by segment << 16 | identifier,
    /// as raw ownership words; one not listed is free and unclaimed.
    pub(crate) identifiers: BTreeMap<u32, u64>,
    /// In a development image, the release word of the debugger stub and
    /// the stopped time, the total of the times spent stopped in the stub
    /// (Two builds, and the debugger stub, D-137).
    pub(crate) release: bool,
    pub(crate) stopped_time: u64,
    /// The attach words, by segment << 16 | identifier, as raw words; one
    /// not listed is 0 (IOMMU, D-137).
    pub(crate) attach: BTreeMap<u32, u64>,
    /// The line words, by GSI, as raw ownership words; one not listed is
    /// free and unclaimed (Interrupts, D-135).
    pub(crate) lines: BTreeMap<u32, u64>,
    /// RISC-V: the level sources of the APLIC domains, by GSI (hardware).
    pub(crate) aplic: BTreeMap<u32, Aplic>,
    pub(crate) members: Vec<Member>,
    pub(crate) roles: Vec<RoleWord>,
    /// The grant table and the use table of each node, by index, and the
    /// serial of each free grant entry that was used.
    pub(crate) grants: Vec<BTreeMap<u16, GrantEntry>>,
    pub(crate) uses: Vec<BTreeMap<u16, UseEntry>>,
    pub(crate) serials: Vec<BTreeMap<u16, u32>>,
    /// Cores: the beacons of the node slots and of the physical cores, the
    /// core word, park request and start request of each physical core (a
    /// start request names a node and its generation as `NodeGen`, or is
    /// 0), and the fences.
    pub(crate) beacons: Vec<NodeBeacon>,
    pub(crate) core_beacons: Vec<CoreBeacon>,
    pub(crate) core_words: Vec<u64>,
    pub(crate) park_requests: Vec<u64>,
    /// Not the living part: what the machine answers about a core that is
    /// off (ARM64 `AFFINITY_INFO` OFF, RISC-V STOPPED or the stop
    /// extension), as the park request it answers, with no start since.
    pub(crate) stopped: Vec<u64>,
    pub(crate) start_requests: Vec<u64>,
    pub(crate) fences: Vec<FenceRecord>,
    /// The rings of each ordered pair of node slots, [from][to].
    pub(crate) rings: Arc<Vec<Vec<RingPair>>>,
    /// The times of `BootMembership`.
    pub(crate) stale_ns: u64,
    pub(crate) stuck_ns: u64,
    pub(crate) sweep_ns: u64,
    pub(crate) seal_ns: u64,
    pub(crate) core_answer_ns: u64,
    /// Debugging: the crash area of this boot, the previous one (never
    /// written), and the values the ports hold.
    pub(crate) crash: Arc<crate::debug::CrashArea>,
    pub(crate) previous: Option<crate::debug::CrashArea>,
    pub(crate) ports: BTreeMap<u16, u8>,
    /// The slot of a replacement kernel image holds an image, and the node
    /// slots last resumed with it.
    pub(crate) replacement: bool,
    pub(crate) resume_replacement: Vec<bool>,
    /// The units of the unit roles, as their hardware holds them, by role.
    pub(crate) units: BTreeMap<usize, UnitState>,
    /// The interrupt tables of the physical cores, kept memory (rule 3):
    /// each claimed entry by its handle (the core's number × 192 + the
    /// vector's index), with the node and generation that claimed it and
    /// the unit roles that hold its remapping entry (Interrupts).
    pub(crate) irq_tables: BTreeMap<u16, IrqTableEntry>,
    /// The retired word of each physical core, a bit per vector of its
    /// table, as handles: set at a removal's answer *failed* and by R6,
    /// cleared only when the machine restarts (Interrupts, the irq, D-140).
    pub(crate) retired: BTreeSet<u16>,
    /// The writes of ownership and identifier words since the last check of
    /// the invariants, and whether that check has run since (the next event
    /// then starts a new journal).
    pub(crate) writes: crate::Ephemeral<Vec<WordWrite>>,
    pub(crate) checked: crate::Ephemeral<std::cell::Cell<bool>>,
    /// The member and role words written in the current event, with their
    /// writer: (node, before, after) and (role, writer, before, after).
    pub(crate) member_writes: crate::Ephemeral<Vec<(u16, u16, Member, Member)>>,
    pub(crate) role_writes: crate::Ephemeral<Vec<(usize, u16, RoleWord, RoleWord)>>,
    /// Whether the current event made a gate call that writes no word of
    /// the machine table: an entry of a translation table, or a gate word
    /// (Gate over page tables).
    pub(crate) gated: crate::Ephemeral<bool>,
    /// The steps of the fence in the current event, in the order they were
    /// made (The fence: "the driver raises p's park request, then marks p's
    /// word, then stops p").
    pub(crate) fence_trace: crate::Ephemeral<Vec<FenceStep>>,
}

/// One step of the fence on a physical core, as the trace records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceStep {
    Raised(usize),
    Marked(usize),
    Stopped(usize),
}

impl Table {
    pub(crate) fn member(&self, node: u16) -> Member {
        self.members[node as usize]
    }

    pub(crate) fn generation(&self, node: u16) -> u32 {
        self.member(node).generation()
    }

    /// The core word of a physical core, `None` if it fails its check.
    pub(crate) fn core_word(&self, core: usize) -> Option<CoreWord> {
        CoreWord::from_raw(*self.core_words.get(core)?)
    }

    /// Whether an ownership word reads *taken* by a node with a
    /// generation: with the node's own code, or with the code of a reserve
    /// whose role the node holds, which a chunk of a reserve keeps when it
    /// is taken (rule 4 of taking: "the word then keeps the owner code of
    /// the reserve, with the state *taken* and X's generation"; R6 reads
    /// it so too). A reserve role does not move while a word of its
    /// reserve is taken by a node that is not dead (Roles), so the code
    /// and the generation name one node. Every judgement of "taken by it
    /// with its generation" reads the word here: G3 (2), R4's audit, the
    /// checks of a unit request.
    pub(crate) fn taken_by(&self, raw: u64, node: u16, generation: u32) -> bool {
        dodos_abi::Ownership::from_raw(raw).is_some_and(|w| {
            w.state() == dodos_abi::OwnState::Taken
                && w.generation() == generation
                && match w.owner() {
                    dodos_abi::Owner::Node(owner) => owner == node,
                    dodos_abi::Owner::Reserve(j) => self.holds_reserve(node, j),
                    dodos_abi::Owner::Unclaimed => false,
                }
        })
    }

    /// Whether a page of ordinary memory is held by a node with a
    /// generation: its chunk taken by it, or in its floor (G3 (2)).
    pub(crate) fn holds_page(&self, at: u64, node: u16, generation: u32) -> bool {
        let chunk = at / self.geometry.chunk();
        let word = self.owners.get(chunk as usize);
        let taken = word.is_some_and(|&raw| self.taken_by(raw, node, generation));
        let floors = self.floors.get(node as usize).into_iter().flatten();
        taken || floors.clone().any(|&(start, end)| start <= at && at < end)
    }

    /// The physical cores whose words name a node with a generation (owned
    /// or joining).
    pub(crate) fn cores_of(&self, node: u16, generation: u32) -> Vec<usize> {
        (0..self.core_words.len())
            .filter(|&c| self.core_word(c).is_some_and(|w| w.names(node, generation)))
            .collect()
    }

    /// Whether a core word naming a node with a generation reads joining:
    /// the node's starting → live waits until none does (The states of a
    /// node, D-158).
    pub(crate) fn joining(&self, node: u16, generation: u32) -> bool {
        self.cores_of(node, generation).into_iter().any(|c| {
            self.core_word(c)
                .is_some_and(|w| w.state() == CoreState::Joining)
        })
    }

    /// The gate writes a core word by compare-and-swap, on a row of the
    /// table of its transitions (Node slots and core words): never the
    /// reserved state *leaving*.
    pub(crate) fn write_core(&mut self, core: usize, before: u64, after: CoreWord) -> bool {
        let row = CoreWord::from_raw(before).and_then(|w| dodos_abi::core_transition(w, after));
        if row.is_none() || self.core_words[core] != before {
            return false;
        }
        self.core_words[core] = after.raw();
        true
    }

    /// A start request naming a node of a generation: `NodeGen` with bit 63
    /// set, so that 0 names none.
    pub(crate) fn start_request(node: u16, generation: u32) -> u64 {
        1 << 63 | NodeGen::new(node, generation).raw()
    }

    /// The node a start request names, if any.
    pub(crate) fn started_node(raw: u64) -> Option<(u16, u32)> {
        (raw >> 63 == 1).then_some(((raw >> 32) as u16, raw as u32))
    }

    /// Whether a node's member word lets it write: starting, live or
    /// sealing.
    pub(crate) fn writes(&self, node: u16) -> bool {
        matches!(
            self.member(node).state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing
        )
    }

    /// The gate writes a member word. The model runs one event at a time,
    /// so no other writer can change the word between its read and this
    /// write; the caller reads the word it replaces (ver-B O20).
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
        let word = word.with_map_count(count);
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
        self.gated.0 = true;
    }

    /// The chunks a frame touches, and the device entry it lies in.
    pub(crate) fn frame_touches(&self, frame: &crate::Frame) -> (Vec<u64>, Option<usize>) {
        let bytes = frame.pages << self.geometry.page_shift[frame.page as usize];
        let end = frame.base + bytes.max(1) - 1;
        let device = self.devices.iter().position(|e| {
            e.is_used() && e.base() <= frame.base && frame.base < e.base() + (1 << e.log2())
        });
        // A frame of the global read-only set is counted nowhere: an anchor
        // over it raises no count, and no gate word of a chunk that no node
        // owns is written after machine start-up (G4, D-151).
        let kept = frame.kept || self.kept.iter().any(|&(s, e)| s <= frame.base && end < e);
        match device {
            Some(index) => (Vec::new(), Some(index)),
            None if kept => (Vec::new(), None),
            None => {
                let shift = self.geometry.chunk_shift();
                let chunks = (frame.base >> shift..=end >> shift)
                    .filter(|&c| (c as usize) < self.gate_words.len())
                    .collect();
                (chunks, None)
            }
        }
    }

    /// The owner's gate raises (`delta` 1) or lowers (-1) the grant count
    /// of every chunk, or of the device entry, a grant entry's anchor
    /// touches, at its write and at its freeing (D-148).
    pub(crate) fn add_grants(&mut self, frame: &crate::Frame, delta: i32) {
        let (chunks, device) = self.frame_touches(frame);
        for chunk in chunks {
            let word = self.gate_word(chunk);
            let count = (i32::from(word.grants()) + delta).max(0) as u16;
            let word = word.with_grants(count).expect("a grant count");
            Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
        }
        if let Some(index) = device {
            let counts = Arc::make_mut(&mut self.device_grants);
            counts[index] = (i32::from(counts[index]) + delta).max(0) as u16;
        }
        self.gated.0 = true;
    }

    /// The grant count of a chunk.
    pub(crate) fn grant_count(&self, chunk: u64) -> u16 {
        self.gate_words
            .get(chunk as usize)
            .and_then(|&raw| GateWord::from_raw(raw))
            .map_or(0, GateWord::grants)
    }

    /// The map count of a chunk written, its other fields kept (a test's
    /// input).
    #[cfg(test)]
    pub(crate) fn set_map_count(&mut self, chunk: u64, count: u32) {
        let word = self.gate_word(chunk).with_map_count(count);
        self.write_gate_word(chunk, word);
    }

    /// The gate writes the whole gate word of a chunk (G4, D-154): at
    /// `memory_take` and at a fresh start, its counts 0 and its table bit
    /// as the kind of the take or the floor says; its page classes are
    /// written data first, so the word counts no page.
    pub(crate) fn write_gate_word(&mut self, chunk: u64, word: GateWord) {
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
        self.gated.0 = true;
    }

    /// R6 (D-154): the map count and the table bit of a chunk the dead
    /// owner took, or of its floor, are cleared with its page classes; its
    /// grant count goes with the owner's grant table.
    pub(crate) fn clear_gate_word(&mut self, chunk: u64) {
        let word = self.gate_word(chunk);
        let cleared = GateWord::ORDINARY
            .with_not_data(word.not_data())
            .and_then(|w| w.with_grants(word.grants()))
            .expect("the fields of a gate word");
        self.write_gate_word(chunk, cleared);
    }

    /// Whether a chunk is a table chunk: bit 58 of its gate word (D-154).
    pub(crate) fn table_chunk(&self, chunk: u64) -> bool {
        self.gate_words
            .get(chunk as usize)
            .and_then(|&raw| GateWord::from_raw(raw))
            .is_some_and(GateWord::is_table)
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
        let word = word.with_not_data(not_data).expect("at most E pages");
        Arc::make_mut(&mut self.gate_words)[chunk as usize] = word.raw();
        self.gated.0 = true;
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

    /// Whether a node holds a reserve role, any of them.
    pub(crate) fn holds_a_reserve(&self, node: u16) -> bool {
        self.role_records.iter().any(|record| {
            record.kind == RoleKind::Reserve && self.holds_reserve(node, record.object)
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
            writes: self.member(writer).writes(),
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

    /// Step R6: a write by the node that clears a dead generation's word,
    /// to its cleared value only, which the gate allows to a node that is
    /// not the owner.
    pub(crate) fn write_owner_cleared(
        &mut self,
        writer: u16,
        chunk: u64,
        before: u64,
        after: Ownership,
    ) {
        if !self.member(writer).writes() || self.owners[chunk as usize] != before {
            return;
        }
        let write = self.journal(writer, Word::Chunk(chunk), before, after.raw());
        Arc::make_mut(&mut self.owners)[chunk as usize] = after.raw();
        self.writes.push(write);
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
        // Every write through a gate first reads the writer's member word
        // (ver-B O21).
        if !self.member(writer).writes() {
            return;
        }
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

    /// The attach word of a device identifier, 0 if it is not listed.
    pub(crate) fn attach_word(&self, key: u32) -> dodos_abi::AttachWord {
        let raw = self.attach.get(&key).copied().unwrap_or(0);
        dodos_abi::AttachWord::from_raw(raw).unwrap_or(dodos_abi::AttachWord::NONE)
    }

    /// A compare-and-swap of an attach word through a gate: written only if
    /// it reads `before` (IOMMU, D-137). Returns whether it was written.
    pub(crate) fn swap_attach(
        &mut self,
        key: u32,
        before: dodos_abi::AttachWord,
        after: dodos_abi::AttachWord,
    ) -> bool {
        if self.attach_word(key) != before {
            return false;
        }
        match after == dodos_abi::AttachWord::NONE {
            true => self.attach.remove(&key),
            false => self.attach.insert(key, after.raw()),
        };
        true
    }

    /// The devices whose attach word names `root`: the *n* of the root's
    /// class (G2, D-137).
    pub(crate) fn attached_to(&self, root: u64) -> u8 {
        let words = self
            .attach
            .values()
            .filter_map(|&raw| dodos_abi::AttachWord::from_raw(raw));
        words
            .filter(|w| w.state().is_some() && w.root() == root)
            .count() as u8
    }

    /// The line word of a GSI, free and unclaimed if it is not listed.
    pub(crate) fn line_word(&self, gsi: u32) -> Ownership {
        let raw = self.lines.get(&gsi).copied();
        let raw = raw.unwrap_or(Ownership::free(Owner::Unclaimed).raw());
        Ownership::from_raw(raw).unwrap_or(Ownership::taken(Owner::Unclaimed, u32::MAX))
    }

    /// The write gate on a line word: *taken* by the writer's generation,
    /// or `None` for *free*, unclaimed; also R6's clearing of a word that a
    /// generation that is over took, to its cleared value.
    pub(crate) fn write_line(&mut self, writer: u16, gsi: u32, after: Option<Ownership>) {
        if !self.member(writer).writes() {
            return;
        }
        let unclaimed = Ownership::free(Owner::Unclaimed).raw();
        let before = self.lines.get(&gsi).copied().unwrap_or(unclaimed);
        let after_raw = after.map_or(unclaimed, Ownership::raw);
        let write = self.journal(writer, Word::Line(gsi), before, after_raw);
        match after {
            Some(word) => self.lines.insert(gsi, word.raw()),
            None => self.lines.remove(&gsi),
        };
        self.writes.push(write);
    }
}
