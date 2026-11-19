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
//! is marked `CONTRACT-GAP` and numbered (from 26).

pub(crate) mod check;
pub(crate) mod cpu;
pub(crate) mod events;
pub(crate) mod faulty;

use std::collections::{BTreeMap, BTreeSet};

/// The geometry of the small machine: entries per table, levels (the root
/// at `LEVELS`), pages per chunk.
pub(crate) const ENTRIES: usize = 4;
pub(crate) const LEVELS: u8 = 2;
pub(crate) const PAGES_PER_CHUNK: u64 = 4;
/// Tags per node: entry 0 the kernel's own.
pub(crate) const TAGS: usize = 4;

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
    Free { node: Option<u16> },
    Taken { node: u16, generation: u32 },
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

/// The gate's record of the clearing: the run of leaves cleared (their
/// pages, in the chunks whose map counts hold them) and the tables
/// unhooked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Clearing {
    pub(crate) leaves: Vec<Page>,
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
/// kernel root (tag 0) and kernel table, and its floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeLayout {
    pub(crate) cores: Vec<usize>,
    pub(crate) kernel_root: Page,
    pub(crate) kernel_table: Page,
    pub(crate) floor: (Page, Page),
}

/// What a core's hardware caches under a tag: leaves and tables reached
/// by a walk (the paging-structure caches), by virtual address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tlb {
    /// (tag, va) → (page, writable): cached leaves.
    pub(crate) leaves: BTreeMap<(usize, (usize, usize)), (Page, bool)>,
    /// (tag, e0) → the level-1 table cached for it.
    pub(crate) tables: BTreeMap<(usize, usize), Page>,
}

/// The gate-written state of the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gate {
    pub(crate) members: Vec<Member>,
    pub(crate) owners: Vec<Owner>,
    /// Per page of ordinary memory: its class byte.
    pub(crate) classes: Vec<Class>,
    /// Per chunk: map count, pages not data, grant count (`GateWord`).
    pub(crate) map_counts: Vec<u64>,
    pub(crate) not_data: Vec<u64>,
    pub(crate) grant_counts: Vec<u64>,
    /// The entries of every table page, by page (a table's contents are
    /// written only by the gate).
    pub(crate) tables: BTreeMap<Page, [Entry; ENTRIES]>,
    pub(crate) nodes: Vec<NodeGate>,
}

/// The kernel-written state: what a faulty kernel may write at will.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kernel {
    /// The contents a kernel wrote into data pages it maps writable: an
    /// arbitrary value, which a stale walk would read as entries.
    pub(crate) data: BTreeMap<Page, u64>,
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
    /// Memory reused since a translation could reach it: a page given
    /// away or turned into a table (for the checker).
    pub(crate) chunks: u64,
}

impl Machine {
    /// `nodes` nodes of `cores` cores each; `chunks` chunks of ordinary
    /// memory, the first two of each node its floor (kept root and kernel
    /// table in the floor's first chunk, the rest of the floor data).
    pub(crate) fn new(nodes: u16, cores: usize, chunks: u64) -> Self {
        let mut layout = Vec::new();
        let pages = chunks * PAGES_PER_CHUNK;
        let mut classes = vec![Class::Data; pages as usize];
        let mut owners = vec![Owner::Free { node: None }; chunks as usize];
        let mut tables = BTreeMap::new();
        for n in 0..nodes {
            let first = u64::from(n) * 2 * PAGES_PER_CHUNK;
            let floor = (first, first + 2 * PAGES_PER_CHUNK);
            let kernel_root = first;
            let kernel_table = first + 1;
            // The kept kernel root and table have no class byte in the
            // contract (kept memory); the small machine keeps them in the
            // floor's first page pair, of class Root(Live) and a hooked
            // table, never on a free list and never torn down.
            classes[kernel_root as usize] = Class::Kernel;
            classes[kernel_table as usize] = Class::Kernel;
            let mut root = [Entry::Absent; ENTRIES];
            root[ENTRIES - 1] = Entry::Shared(kernel_table);
            tables.insert(kernel_root, root);
            tables.insert(kernel_table, [Entry::Absent; ENTRIES]);
            for c in 0..2 {
                owners[(u64::from(n) * 2 + c) as usize] = Owner::Taken {
                    node: n,
                    generation: 1,
                };
            }
            layout.push(NodeLayout {
                cores: (0..cores).map(|p| usize::from(n) * cores + p).collect(),
                kernel_root,
                kernel_table,
                floor,
            });
        }
        let nodes_gate = layout
            .iter()
            .map(|l| {
                let mut tags = vec![TagEntry::default(); TAGS];
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
        for (page, class) in classes.iter().enumerate() {
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
                tables,
                nodes: nodes_gate,
            },
            kernel: Kernel::default(),
            tlb: vec![Tlb::default(); usize::from(nodes) * cores],
            faulty: BTreeSet::new(),
            chunks,
        }
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
        }
    }

    /// The hardware invalidates on `core`: a range of a tag, or a whole
    /// tag, or every tag.
    pub(crate) fn invalidate(&mut self, core: usize, what: Flush) {
        let tlb = &mut self.tlb[core];
        match what {
            Flush::Range { tag, ref va } => {
                tlb.leaves
                    .retain(|&(t, v), _| !(t == tag && va.contains(&v)));
                // A range invalidation drops the paging-structure caches of
                // the tag too (x86-64 INVLPG, ARM64 TLBI VA): the tables
                // cached for those addresses.
                tlb.tables
                    .retain(|&(t, e0), _| !(t == tag && va.iter().any(|v| v.0 == e0)));
            }
            Flush::Tag(tag) => {
                tlb.leaves.retain(|&(t, _), _| t != tag);
                tlb.tables.retain(|&(t, _), _| t != tag);
            }
            Flush::All => {
                tlb.leaves.clear();
                tlb.tables.clear();
            }
            // The kernel's own space: what any tag cached through the
            // shared entry (global translations), nothing of the user half.
            Flush::KernelSpace => {
                let shared = ENTRIES - 1;
                tlb.leaves.retain(|&(_, (e0, _)), _| e0 != shared);
                tlb.tables.retain(|&(_, e0), _| e0 != shared);
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
mod tests;
