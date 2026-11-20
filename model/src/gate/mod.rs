//! Pass 29: the gate executed from its tables (D-152, "Gate words").
//!
//! A small machine whose state is the gate-written words of the contract,
//! each a type with exactly the states of its table, and whose operations
//! are the transitions of those tables: one function per row, with the
//! row's writer, its condition (reading the words the row names) and its
//! compare-and-swap. The global events are the columns of the event tables
//! (`events.rs`). The hardware is adversarial: after every step each core
//! caches every translation reachable under the tag it has loaded (the
//! TLB and the paging-structure caches), and keeps what it cached until a
//! gate call invalidates it.
//!
//! Kernel-written and gate-written state are separate types: what a node's
//! kernel may write is `Kernel` (its data pages, its beacon words); the
//! gate's words are the fields of `Gate` and of `NodeGate`, which only the
//! functions of this module change. The faulty-kernel actor (`faulty.rs`)
//! calls any gate operation with any arguments in any order, and writes
//! any kernel-writable word; the checker (`check.rs`) holds the
//! invariants of (G) after every step.
//!
//! A cell of a table that cannot be executed, or that the checker breaks,
//! is marked `CONTRACT-GAP` and numbered (26 to 39 settled by D-153; 40 to
//! 46, found in D-153's split and merge of the kernel's large leaf, closed
//! by D-154, which replaces that design by dedicated table chunks; 49, a
//! second give of a chunk whose give is in flight, found by pass 32's
//! audit, settled by D-159 as the model read it, `cpu.rs`). The earlier
//! readings stay as `OldRules` switches, so that each scenario shows its
//! attack failing.

pub(crate) mod check;
pub(crate) mod cpu;
pub(crate) mod devices;
pub(crate) mod events;
pub(crate) mod faulty;
pub(crate) mod grants;
pub(crate) mod parks;
pub(crate) mod ports;
#[cfg(test)]
pub(crate) mod race;
pub(crate) mod suspend;
pub(crate) mod unit;

use std::collections::{BTreeMap, BTreeSet};

/// The geometry of the small machine: entries per table, levels (the root
/// at `LEVELS`), pages per chunk.
pub(crate) const ENTRIES: usize = 4;
pub(crate) const LEVELS: u8 = 2;
pub(crate) const PAGES_PER_CHUNK: u64 = 4;
/// The tags of the small machine's architecture; one, the last, is the
/// gate's own, bound to no kernel or user root, and the tag table holds
/// the others (G7, D-155: `GATE_PCID` on x86-64, a reserved ASID on ARM64
/// and RISC-V). Entry 0 is the kernel's own.
pub(crate) const ARCH_TAGS: usize = 5;
pub(crate) const TAGS: usize = ARCH_TAGS - 1;
pub(crate) const GATE_TAG: usize = TAGS;

/// A page number.
pub(crate) type Page = u64;

/// Why a gate call refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The condition of the row does not hold.
    Condition,
    /// The compare-and-swap found another value.
    Cas,
}

pub(crate) type Gated<T = ()> = Result<T, Refused>;

pub(crate) fn require(cond: bool) -> Gated {
    if cond {
        Ok(())
    } else {
        Err(Refused::Condition)
    }
}

/// The state of a node (its member word), as far as the gate reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeState {
    Dead,
    Starting { resume: bool },
    Live,
    Sealing,
    Sealed,
    Fencing,
    Fenced,
}

impl NodeState {
    /// The gate of a node in this state writes (G13: a fencing, fenced,
    /// unfenced, dead or sealed node's gate refuses every write).
    pub(crate) fn writes(self) -> bool {
        matches!(
            self,
            NodeState::Starting { .. } | NodeState::Live | NodeState::Sealing
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub(crate) state: NodeState,
    pub(crate) generation: u32,
}

/// An ownership word (chunks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Free {
        node: Option<u16>,
    },
    Taken {
        node: u16,
        generation: u32,
    },
    /// A chunk of a node's floor: never taken, never given.
    Floor {
        node: u16,
    },
}

/// The hook state of a table below a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hook {
    Loose,
    Hooked,
    Unhooking,
}

/// The state of an aspace root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootState {
    Unbound,
    Live,
    Dying,
}

/// The page class byte (bits 0-5 the class, bits 6-7 the hook state):
/// exactly the valid bytes of its table; any other byte fails the check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Data,
    /// An aspace table of level `level` below the root, with its hook state.
    Table {
        level: u8,
        hook: Hook,
    },
    Root(RootState),
    /// A kept kernel table (the kernel's own root and table): no class
    /// byte in the contract, its class following from where it lies; one
    /// state, *fixed*: never hooked to, never loose, never data.
    Kernel,
    /// An iospace root 40 + `devices` (0 to 16), *live* or *dying* (the
    /// small machine's iospaces have one level: its entries are leaves).
    IoRoot {
        devices: u8,
        dying: bool,
    },
    /// A page of an `its_table` object (class 8), unpinned or pinned (bits
    /// 6-7 0 or 1); no entries the gate reads.
    ItsTable {
        pinned: bool,
    },
}

impl Class {
    /// Whether the page holds entries the gate (or a walk) reads: every
    /// class but *data* and *ITS table*.
    pub(crate) fn has_entries(self) -> bool {
        !matches!(self, Class::Data | Class::ItsTable { .. })
    }
}

/// An entry of a table (gate-written: a table page is mapped writable by
/// no kernel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Absent,
    /// A table one level down.
    Table(Page),
    /// A leaf: a page and whether it is writable.
    Leaf {
        page: Page,
        write: bool,
    },
    /// A shared entry toward the node's kept kernel table.
    Shared(Page),
    /// The kernel's large leaf of a chunk, in its kernel table: every page
    /// of the chunk, writable for an ordinary chunk and read-only for a
    /// table chunk, never rewritten between take and give (Gate words, kept
    /// kernel tables; D-154).
    Large {
        chunk: u64,
        write: bool,
    },
}

/// The tag table's entry: the root the tag is bound to and the set of the
/// node's cores (positions) that loaded it since it was last invalidated
/// on them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TagEntry {
    pub(crate) root: Option<Page>,
    pub(crate) set: BTreeSet<usize>,
}

/// What a core's invalidation request invalidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Invalidates {
    /// A range under a tag: the leaves cleared (virtual addresses).
    Range {
        tag: usize,
        root: Page,
        va: Vec<(usize, usize)>,
    },
    /// A whole tag (an unbind, or a dying root).
    Tag { tag: usize, root: Page },
    /// A run of the kernel's own space (a give or a destruction): the
    /// chunk, and the node it goes to.
    KernelRun { chunk: u64, to: Option<u16> },
    /// Nothing: a barrier.
    Barrier,
}

/// How a chunk is taken (`memory_take`): kind 0, an ordinary chunk, or
/// kind 2, a table chunk (D-154).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Take {
    Ordinary,
    Tables,
}

/// What pages leaving *data* become, in the call that turns them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Table,
    Root,
    IoRoot,
    Its { first: Page, k: u8 },
}

/// The count a cleared leaf is in until its invalidation is complete (Gate
/// words, a core's invalidation request: "the count they are in").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Counted {
    /// The map count of the leaf's chunk: a leaf toward the node's own
    /// memory (G4).
    Map,
    /// The node's use entry of this index: a leaf toward a granted page,
    /// which the granter's map count never holds (G6).
    Use(usize),
}

/// The gate's record of the clearing: the run of leaves cleared (their
/// pages and the count each is in) and the tables unhooked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Clearing {
    pub(crate) leaves: Vec<(Page, Counted)>,
    pub(crate) unhooked: Vec<Page>,
    /// A dying root torn down once the request completes.
    pub(crate) dying: Option<Page>,
}

/// A core's invalidation request (gate part).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Free,
    InUse {
        what: Invalidates,
        asked: BTreeSet<usize>,
        acked: BTreeSet<usize>,
        record: Clearing,
    },
}

/// The idle word of a core: the idle mark and the owes bit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IdleWord {
    pub(crate) idle: bool,
    pub(crate) owes: bool,
}

/// The gate part of a node block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeGate {
    /// Per core position: the loaded root and tag.
    pub(crate) loaded: Vec<(Page, usize)>,
    pub(crate) tags: Vec<TagEntry>,
    pub(crate) requests: Vec<Request>,
    pub(crate) pending: Vec<BTreeSet<usize>>,
    pub(crate) idle: Vec<IdleWord>,
}

/// One node of the small machine: its cores (physical numbers), its kept
/// kernel root (tag 0) and kernel table, kept memory outside the chunks,
/// and its floor: one chunk of table floor (`floor_tables`, the table bit
/// set), then one chunk of ordinary floor (`floor_node`) (D-154).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeLayout {
    pub(crate) cores: Vec<usize>,
    pub(crate) kernel_root: Page,
    pub(crate) kernel_table: Page,
    pub(crate) floor: (Page, Page),
    pub(crate) table_floor: (Page, Page),
}

/// What a core's hardware caches under a tag: leaves and tables reached
/// by a walk (the paging-structure caches), by virtual address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tlb {
    /// (tag, va) → (page, writable): cached leaves.
    pub(crate) leaves: BTreeMap<(usize, (usize, usize)), (Page, bool)>,
    /// (tag, e0) → the level-1 table cached for it.
    pub(crate) tables: BTreeMap<(usize, usize), Page>,
    /// The kernel's own space (global, whatever the tag): the large leaves
    /// cached, kernel-table slot → (chunk, writable).
    pub(crate) large: BTreeMap<usize, (u64, bool)>,
}

/// The gate-written state of the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gate {
    pub(crate) members: Vec<Member>,
    pub(crate) owners: Vec<Owner>,
    /// Per page of ordinary memory: its class byte.
    pub(crate) classes: Vec<Class>,
    /// Per chunk: map count, pages not data, grant count and table bit
    /// (`GateWord`; bit 58, D-154).
    pub(crate) map_counts: Vec<u64>,
    pub(crate) not_data: Vec<u64>,
    pub(crate) grant_counts: Vec<u64>,
    pub(crate) table_bits: Vec<bool>,
    /// The entries of every table page, by page (a table's contents are
    /// written only by the gate).
    pub(crate) tables: BTreeMap<Page, [Entry; ENTRIES]>,
    /// The first word of each *loose* table: 0 or the next *loose* table of
    /// the node's free list (written only by the gate, `table_link`).
    pub(crate) links: BTreeMap<Page, Page>,
    pub(crate) nodes: Vec<NodeGate>,
}

/// The kernel-written state: what a faulty kernel may write at will.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kernel {
    /// The contents a kernel wrote into data pages it maps writable: an
    /// arbitrary value, which a stale walk would read as entries.
    pub(crate) data: BTreeMap<Page, u64>,
    /// For the checker: the stores the kernel made through its own view
    /// into a page that was not *data*.
    pub(crate) wrote_tables: Vec<(u16, Page)>,
}

/// The rules of an earlier text, each switched on to show that its attack
/// breaks the checker (the scenarios that must fail against the old rule);
/// all off is the tables of D-152.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OldRules {
    /// D-151: an asked core invalidates a dying root's tag without loading
    /// the kernel's root first.
    pub(crate) dying_without_kernel_load: bool,
    /// The kernel-space unmapping and the barrier asking only the cores
    /// that are not idle.
    pub(crate) kernel_run_skips_idle: bool,
    /// A fresh start that leaves the gate part as the dead generation left
    /// it.
    pub(crate) fresh_keeps_gate_part: bool,
    /// A root bound under a second tag.
    pub(crate) root_under_two_tags: bool,
    /// A fresh start that resets its request records' serials.
    pub(crate) serial_reset_at_fresh: bool,
    /// A requester that rewrites an *open* record (restarting the entry).
    pub(crate) words_while_open: bool,
    /// A holder whose compare-and-swap expects a value nobody writes.
    pub(crate) holder_expects_void: bool,
    /// Drop bits keyed on the INTID, never reset when the line word moves.
    pub(crate) drop_bit_per_intid: bool,
    /// A seal whose park requests a later call raises, which a faulty
    /// kernel may skip.
    pub(crate) park_requests_by_later_call: bool,
    /// R6 freeing a dead granter's grant entries on the barrier alone
    /// (`acked`, which a faulty node may write early), not on the use
    /// counts (D-149).
    pub(crate) r6_frees_on_acked: bool,
    /// The clearing call leaving its own core's caches alone (the reading
    /// before D-153, gap 26).
    pub(crate) no_own_invalidation: bool,
    /// An unbind asking only the cores of the tag's set that are not idle
    /// (the reading before D-153, gap 27).
    pub(crate) unbind_skips_idle: bool,
    /// A page of any chunk the node holds leaving *data*, the table bit
    /// and the chunk's map count unread (the reading before D-153's gap 28,
    /// which D-154 settles by the table bit).
    pub(crate) tables_in_ordinary_chunks: bool,
    /// G3 without D-154: a leaf written toward a *data* page of a table
    /// chunk.
    pub(crate) leaves_into_table_chunks: bool,
    /// G6 without D-154: a grant's anchor over a page of a table chunk.
    pub(crate) anchors_over_table_chunks: bool,
    /// `memory_take` kind 2 mapping the run writable in the kernel's own
    /// space, as kind 0 does (without D-154's read-only large leaf).
    pub(crate) table_chunks_mapped_writable: bool,
    /// G7 without D-155: the tag table sized from the architecture's whole
    /// tag width, the gate's own tag bound to a root like any other.
    pub(crate) gate_tag_bound: bool,
    /// G10 without D-155: the gate writing the ICR with any vector the
    /// kernel names (an NMI's, an exception's, a claimable one).
    pub(crate) any_vector_in_icr: bool,
    /// RISC-V without D-154's M3: the unhooking of a table invalidated by
    /// address (`SFENCE.VMA` with `rs1` not `x0`), which drops leaf entries
    /// only and leaves the cached non-leaf entry.
    pub(crate) riscv_unhook_by_address: bool,
    /// A chunk's counts left free between a give's call and its completion
    /// (the reading before D-153, gap 30).
    pub(crate) counts_free_during_give: bool,
    /// R1's invalidation of the interrupt entry cache covering only the
    /// handles the walk found present (the reading before D-153, gap 32).
    pub(crate) r1_present_handles_only: bool,
    /// R6 on a failed unit retiring only the present entries toward the
    /// node's cores (the reading before D-153, gap 33).
    pub(crate) r6_retires_present_only: bool,
    /// The identifier word freed with the attach word alone, its ITS word
    /// unread (the reading before D-153, gap 34).
    pub(crate) identifier_freed_by_attach_word: bool,
    /// The holder serving an ITS unmapping whatever its ITS word reads
    /// (the reading before D-153, gap 35).
    pub(crate) its_unmap_unordered: bool,
    /// The holder serving a fencing or fenced requester's records
    /// (the reading before D-153, gap 36).
    pub(crate) serve_fenced_requesters: bool,
    /// An attach or a detach asked while the node's own record of the
    /// device is open (the reading before D-153, gap 38).
    pub(crate) concurrent_device_requests: bool,
}

/// The small machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Machine {
    /// The earlier rules switched on (none: the tables).
    pub(crate) old: OldRules,
    pub(crate) layout: Vec<NodeLayout>,
    pub(crate) gate: Gate,
    pub(crate) kernel: Kernel,
    /// Per physical core: its hardware caches.
    pub(crate) tlb: Vec<Tlb>,
    /// The nodes whose kernel is faulty.
    pub(crate) faulty: BTreeSet<u16>,
    /// The unit side's words.
    pub(crate) unit: unit::UnitGate,
    /// The unit's cache: (domain, entry) → page.
    pub(crate) iotlb: BTreeMap<(u16, usize), Page>,
    /// The interrupt and park side's words.
    pub(crate) parks: parks::ParkGate,
    /// Grant and use entries.
    pub(crate) grants: grants::GrantGate,
    /// The device side's words and the unit's other caches.
    pub(crate) dev: devices::DeviceGate,
    /// Device and port entries (x86-64 ports).
    pub(crate) ports: ports::PortGate,
    /// The hardware is RISC-V: an invalidation by address drops only the
    /// leaf entries cached for it, never a cached non-leaf entry ("the
    /// fence also invalidates all address-translation cache entries that
    /// contain leaf page table entries" for the address when `rs1` is not
    /// `x0`; the Privileged Specification, `SFENCE.VMA`).
    pub(crate) riscv: bool,
    /// The ICRs the gates composed: the sending node, the target core's
    /// number and the vector (x86-64, G10).
    pub(crate) icr: Vec<(u16, usize, u8)>,
    /// Memory reused since a translation could reach it: a page given
    /// away or turned into a table (for the checker).
    pub(crate) chunks: u64,
}

impl Machine {
    /// `nodes` nodes of `cores` cores each; `chunks` chunks of ordinary
    /// memory, the first two of each node its floor: a chunk of table
    /// floor, whose gate word has the table bit set, then a chunk of
    /// ordinary floor (D-154). Each node's kept kernel root and kernel
    /// table are kept memory, in no chunk: two pages past the chunks.
    pub(crate) fn new(nodes: u16, cores: usize, chunks: u64) -> Self {
        let mut layout = Vec::new();
        let pages = chunks * PAGES_PER_CHUNK;
        let mut classes = vec![Class::Data; (pages + 2 * u64::from(nodes)) as usize];
        let mut owners = vec![Owner::Free { node: None }; chunks as usize];
        let mut table_bits = vec![false; chunks as usize];
        let mut tables = BTreeMap::new();
        for n in 0..nodes {
            let first = u64::from(n) * 2 * PAGES_PER_CHUNK;
            let floor = (first, first + 2 * PAGES_PER_CHUNK);
            let table_floor = (first, first + PAGES_PER_CHUNK);
            table_bits[(u64::from(n) * 2) as usize] = true;
            let kernel_root = pages + 2 * u64::from(n);
            let kernel_table = kernel_root + 1;
            // The kept kernel root and table have no class byte in the
            // contract (kept memory): their class follows from where they
            // lie, never on a free list and never torn down.
            classes[kernel_root as usize] = Class::Kernel;
            classes[kernel_table as usize] = Class::Kernel;
            let mut root = [Entry::Absent; ENTRIES];
            root[ENTRIES - 1] = Entry::Shared(kernel_table);
            tables.insert(kernel_root, root);
            tables.insert(kernel_table, [Entry::Absent; ENTRIES]);
            for c in 0..2 {
                owners[(u64::from(n) * 2 + c) as usize] = Owner::Floor { node: n };
            }
            layout.push(NodeLayout {
                cores: (0..cores).map(|p| usize::from(n) * cores + p).collect(),
                kernel_root,
                kernel_table,
                floor,
                table_floor,
            });
        }
        let nodes_gate = layout
            .iter()
            .map(|l| {
                let mut tags = vec![TagEntry::default(); ARCH_TAGS];
                tags[0].root = Some(l.kernel_root);
                NodeGate {
                    loaded: vec![(l.kernel_root, 0); l.cores.len()],
                    tags,
                    requests: vec![Request::Free; l.cores.len()],
                    pending: vec![BTreeSet::new(); l.cores.len()],
                    idle: vec![IdleWord::default(); l.cores.len()],
                }
            })
            .collect();
        let mut not_data = vec![0; chunks as usize];
        for (page, class) in classes.iter().enumerate().take(pages as usize) {
            if *class != Class::Data {
                not_data[page / PAGES_PER_CHUNK as usize] += 1;
            }
        }
        Self {
            old: OldRules::default(),
            layout,
            gate: Gate {
                members: (0..nodes)
                    .map(|_| Member {
                        state: NodeState::Live,
                        generation: 1,
                    })
                    .collect(),
                owners,
                classes,
                map_counts: vec![0; chunks as usize],
                not_data,
                grant_counts: vec![0; chunks as usize],
                table_bits,
                tables,
                links: BTreeMap::new(),
                nodes: nodes_gate,
            },
            kernel: Kernel::default(),
            tlb: vec![Tlb::default(); usize::from(nodes) * cores],
            faulty: BTreeSet::new(),
            unit: unit::UnitGate::new(nodes),
            iotlb: BTreeMap::new(),
            parks: parks::ParkGate::new(nodes, cores),
            grants: grants::GrantGate::new(nodes),
            dev: devices::DeviceGate::new(nodes),
            ports: ports::PortGate::new(),
            riscv: false,
            icr: Vec::new(),
            chunks,
        }
    }

    /// Node `n`'s kernel faulty (it skips what it likes; the gate model's
    /// operations are open to every node anyway).
    pub(crate) fn make_faulty_for_test(&mut self, n: u16) {
        self.faulty.insert(n);
    }

    pub(crate) fn chunk_of(page: Page) -> u64 {
        page / PAGES_PER_CHUNK
    }

    /// Whether node `n`'s generation holds page `page`: its chunk taken by
    /// it with its generation, or its floor.
    pub(crate) fn holds(&self, n: u16, page: Page) -> bool {
        let g = self.gate.members[usize::from(n)].generation;
        let (start, end) = self.layout[usize::from(n)].floor;
        let taken = matches!(
            self.gate.owners.get(Self::chunk_of(page) as usize),
            Some(Owner::Taken { node, generation }) if *node == n && *generation == g
        );
        taken || (start <= page && page < end)
    }

    /// Whether a live, revoking or pinned grant to node `n` covers `page`.
    pub(crate) fn granted_to(&self, n: u16, page: Page) -> bool {
        self.grants
            .grants
            .iter()
            .flatten()
            .flatten()
            .any(|g| g.grantee == n && g.page == page)
    }

    /// The physical core at position `p` of node `n`.
    pub(crate) fn core(&self, n: u16, p: usize) -> usize {
        self.layout[usize::from(n)].cores[p]
    }

    /// The node and position of a physical core.
    pub(crate) fn position(&self, core: usize) -> (u16, usize) {
        for (n, l) in self.layout.iter().enumerate() {
            if let Some(p) = l.cores.iter().position(|&c| c == core) {
                return (n as u16, p);
            }
        }
        panic!("core {core} in no node")
    }

    /// The hardware: every core caches what its loaded root and tag reach
    /// now (the most it may cache), keeping what it cached before.
    pub(crate) fn fill(&mut self) {
        for core in 0..self.tlb.len() {
            let (n, p) = self.position(core);
            // A core of a node that runs nothing (stopped, parked) caches
            // nothing.
            if !self.gate.members[usize::from(n)].state.writes() {
                continue;
            }
            let (root, tag) = self.gate.nodes[usize::from(n)].loaded[p];
            let Some(entries) = self.gate.tables.get(&root).copied() else {
                continue;
            };
            for (e0, entry) in entries.iter().enumerate() {
                let table = match *entry {
                    Entry::Table(t) | Entry::Shared(t) => t,
                    _ => continue,
                };
                self.tlb[core].tables.insert((tag, e0), table);
            }
            // The leaves through every cached table of the tag, read as the
            // table reads now (a stale cached table is walked too).
            let cached: Vec<(usize, Page)> = self.tlb[core]
                .tables
                .iter()
                .filter(|((t, _), _)| *t == tag)
                .map(|(&(_, e0), &page)| (e0, page))
                .collect();
            for (e0, table) in cached {
                if let Some(entries) = self.gate.tables.get(&table).copied() {
                    for (e1, entry) in entries.iter().enumerate() {
                        if let Entry::Leaf { page, write } = *entry {
                            self.tlb[core].leaves.insert((tag, (e0, e1)), (page, write));
                        }
                    }
                }
            }
            // The kernel's own space, through the kernel table: the large
            // leaf of each chunk it maps, with its permission.
            let kernel = self.layout[usize::from(n)].kernel_table;
            let entries = self.gate.tables[&kernel];
            for (slot, entry) in entries.iter().enumerate() {
                if let Entry::Large { chunk, write } = *entry {
                    self.tlb[core].large.insert(slot, (chunk, write));
                }
            }
        }
    }

    /// The hardware invalidates on `core`: a range of a tag, or a whole
    /// tag, or every tag.
    pub(crate) fn invalidate(&mut self, core: usize, what: Flush) {
        let riscv = self.riscv;
        let tlb = &mut self.tlb[core];
        match what {
            Flush::Range { tag, ref va } => {
                tlb.leaves
                    .retain(|&(t, v), _| !(t == tag && va.contains(&v)));
                // A range invalidation drops the paging-structure caches of
                // the tag too (x86-64 INVLPG, ARM64 TLBI VA): the tables
                // cached for those addresses. Not on RISC-V, where
                // `SFENCE.VMA` by address drops leaf entries only (D-154,
                // M3).
                if !riscv {
                    tlb.tables
                        .retain(|&(t, e0), _| !(t == tag && va.iter().any(|v| v.0 == e0)));
                }
            }
            Flush::Tag(tag) => {
                tlb.leaves.retain(|&(t, _), _| t != tag);
                tlb.tables.retain(|&(t, _), _| t != tag);
            }
            Flush::All => {
                tlb.leaves.clear();
                tlb.tables.clear();
                tlb.large.clear();
            }
            // The kernel's own space: what any tag cached through the
            // shared entry (global translations), nothing of the user half.
            Flush::KernelSpace => {
                let shared = ENTRIES - 1;
                tlb.leaves.retain(|&(_, (e0, _)), _| e0 != shared);
                tlb.tables.retain(|&(_, e0), _| e0 != shared);
                tlb.large.clear();
            }
        }
    }
}

/// What the hardware invalidates on one core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Flush {
    Range { tag: usize, va: Vec<(usize, usize)> },
    Tag(usize),
    All,
    KernelSpace,
}

#[cfg(test)]
mod devices_tests;
#[cfg(test)]
mod pass32_tests;
#[cfg(test)]
mod pass33_tests;
#[cfg(test)]
mod settled_tests;
#[cfg(test)]
mod suspend_tests;
#[cfg(test)]
mod table_chunk_tests;
#[cfg(test)]
mod tests;
