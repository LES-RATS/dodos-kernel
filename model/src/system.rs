//! A machine of several nodes: one model per node, one machine table they
//! share, the messages between kernels in flight, which arrive in the
//! order a test chooses (Memory and devices between nodes), and the
//! physical cores (Cores): their states, which the fence acts on. Every
//! event of a node runs alone, so the states "waiting for another node"
//! are entered and left as on a machine whose cores interleave in that
//! order.

use crate::boot::{Layout, Machine};
use crate::recovery::HwAction;
use crate::remote::{Body, Packet};
use crate::table::GrantState;
use crate::{Model, ObjId, Outcome, Refusal, Syscall};
use dodos_abi::{Member, MemberState, RoleKind, RoleState};
use std::collections::BTreeSet;

/// The state of a physical core, as the hardware has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreState {
    /// It runs the kernel of the node placed on it.
    Running,
    /// It executes nothing useful: hung, or in a loop with interrupts
    /// masked; `nmi` says whether its kernel can still run its NMI handler.
    Hung { nmi: bool },
    /// In the loop of the NMI handler, interrupts masked.
    Looping,
    /// INIT received, waiting for a SIPI.
    InitPending,
    /// Halted: by the park stub (with the fence epoch in its beacon), by
    /// its own kernel, or never started.
    Parked,
    /// Off (ARM64 `CPU_OFF`, RISC-V stopped).
    Off,
}

/// The machine: its nodes, the messages in flight, its physical cores.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    /// The model of each node: `None` once it is dead, until a fresh start.
    nodes: Vec<Option<Model>>,
    in_flight: Vec<Packet>,
    cores: Vec<CoreState>,
    machine: Machine,
    layout: Layout,
    /// The common clock.
    now: u64,
}

/// `Err` names the invariant that fails.
type Check = Result<(), String>;

impl System {
    /// Every node started from its floor and the kept image, with the
    /// machine table of machine start-up; the first thread of each node.
    pub fn boot(machine: &Machine) -> Result<(Self, Vec<ObjId>), Refusal> {
        let layout = machine.layout()?;
        // The nodes start one after another, each on the table the one
        // before left; then all read the last.
        let mut table = machine.table(&layout);
        let mut nodes: Vec<Option<Model>> = Vec::new();
        let mut firsts = Vec::new();
        for node in 0..machine.nodes() as u16 {
            let (mut model, first) = Model::start(machine, &layout, table, node)?;
            model.shared = true;
            table = model.table.clone();
            nodes.push(Some(model));
            firsts.push(first);
        }
        for model in nodes.iter_mut().flatten() {
            model.table = table.clone();
        }
        let cores = vec![CoreState::Running; machine.nodes()];
        let mut system = Self {
            nodes,
            in_flight: Vec::new(),
            cores,
            machine: machine.clone(),
            layout,
            now: 0,
        };
        for node in 0..machine.nodes() as u16 {
            system.after(node);
        }
        Ok((system, firsts))
    }

    /// The model of a node, to read it.
    pub fn node(&self, node: u16) -> &Model {
        self.nodes[node as usize]
            .as_ref()
            .expect("a node with a kernel")
    }

    /// Whether a node has a kernel: not dead.
    pub fn has(&self, node: u16) -> bool {
        self.nodes[node as usize].is_some()
    }

    fn table(&self) -> &crate::table::Table {
        &self.nodes.iter().flatten().next().expect("a node").table
    }

    /// The member word of a node.
    pub fn member(&self, node: u16) -> Member {
        self.table().member(node)
    }

    /// The role word of role `index`.
    pub fn role(&self, index: usize) -> dodos_abi::RoleWord {
        self.table().roles[index]
    }

    /// The number of roles of the machine.
    pub fn node_count_roles(&self) -> usize {
        self.table().roles.len()
    }

    /// The state of a physical core.
    pub fn core(&self, core: usize) -> CoreState {
        self.cores[core]
    }

    /// The common clock.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Whether a node runs: it has a kernel and the core it is placed on
    /// runs.
    pub fn runs(&self, node: u16) -> bool {
        let Some(model) = &self.nodes[node as usize] else {
            return false;
        };
        let core = model.table.placed[node as usize];
        let member = model.table.member(node);
        // A stopped node whose block is saved on a core that runs another
        // node is never loaded again.
        let unloaded = !member.writes() && model.parked_block(node);
        self.cores[core] == CoreState::Running
            && !matches!(member.state(), MemberState::Sealed | MemberState::Dead)
            && !unloaded
    }

    fn model(&mut self, node: u16) -> &mut Model {
        self.nodes[node as usize]
            .as_mut()
            .expect("a node with a kernel")
    }

    /// The messages in flight, in the order they were sent.
    pub fn pending(&self) -> &[Packet] {
        &self.in_flight
    }

    /// One call made by a thread of a node, which runs.
    pub fn apply(&mut self, node: u16, thread: ObjId, call: Syscall) -> Outcome {
        assert!(self.runs(node), "node {node} does not run");
        let outcome = self.model(node).apply(thread, call);
        self.after(node);
        outcome
    }

    /// The message in flight at `index` arrives at its node, which runs.
    /// The order of messages: one ring per ordered pair of nodes, so a
    /// message never overtakes an older one with the same sender and
    /// receiver; messages of different rings arrive in any order.
    pub fn deliver(&mut self, index: usize) {
        let (from, to) = (self.in_flight[index].from, self.in_flight[index].to);
        assert!(
            !self.in_flight[..index]
                .iter()
                .any(|p| p.from == from && p.to == to),
            "ring order: an older message from {from} to {to} is still in flight"
        );
        assert!(self.runs(to), "node {to} does not run");
        let packet = self.in_flight.remove(index);
        let from = packet.from as usize;
        self.model(to).table.rings[from][to as usize].1 += 1;
        self.model(to).receive_packet(packet);
        self.after(to);
    }

    /// Delivers the messages in flight to nodes that run, oldest first, as
    /// long as some are, at most `bound` of them; returns how many. Those
    /// to a node without a kernel are dropped.
    pub fn settle(&mut self, bound: usize) -> usize {
        let mut delivered = 0;
        while delivered < bound {
            self.in_flight.retain({
                let nodes = &self.nodes;
                move |p| nodes[p.to as usize].is_some()
            });
            let Some(index) = self.in_flight.iter().position(|p| self.runs(p.to)) else {
                break;
            };
            self.deliver(index);
            delivered += 1;
        }
        delivered
    }

    /// Time passes on every node that runs, up to `to`.
    pub fn advance(&mut self, to: u64) {
        assert!(to >= self.now, "the clock is monotonic");
        self.now = to;
        for node in 0..self.nodes.len() as u16 {
            if self.runs(node) {
                self.model(node).advance(to);
                self.after(node);
            }
        }
    }

    /// The recovery reserve of every node that runs, until none has work,
    /// at most `bound` units; returns how many.
    pub fn recover(&mut self, bound: usize) -> usize {
        let mut units = 0;
        loop {
            let mut any = false;
            for node in 0..self.nodes.len() as u16 {
                if units >= bound {
                    return units;
                }
                if self.runs(node) {
                    let now = self.now;
                    let model = self.model(node);
                    if model.now() < now {
                        model.advance(now);
                    }
                    let done = self.model(node).recovery_step();
                    self.after(node);
                    if done {
                        units += 1;
                        any = true;
                    }
                }
            }
            if !any {
                return units;
            }
        }
    }

    /// The machine runs up to `to` in steps of `step`: time, the recovery
    /// reserves, and the messages, at each step.
    pub fn run(&mut self, to: u64, step: u64) {
        let mut at = self.now;
        while at < to {
            at = (at + step).min(to);
            self.advance(at);
            for _ in 0..64 {
                let work = self.recover(1024) + self.settle(1024);
                if work == 0 {
                    break;
                }
            }
        }
    }

    // What tests do to the hardware.

    /// The core of a node hangs; its kernel can or cannot still run its
    /// NMI handler.
    pub fn hang(&mut self, node: u16, nmi: bool) {
        let core = self.table().placed[node as usize];
        self.cores[core] = CoreState::Hung { nmi };
    }

    /// A node finds a failure in itself: it writes its panic beacon, sends
    /// an interrupt to the next live node, and parks itself.
    pub fn panic(&mut self, node: u16, code: u64) {
        let generation = self.table().generation(node);
        let model = self.model(node);
        model.table.beacons[node as usize].panic = Some((code, generation));
        // Its record and an entry of code 6, before it parks (reason 1, an
        // assertion of the kernel).
        model.failed(1);
        let core = model.table.placed[node as usize];
        self.after(node);
        self.cores[core] = CoreState::Hung { nmi: true };
    }

    /// A core enters or leaves VMX root operation.
    pub fn set_vmx(&mut self, core: usize, on: bool) {
        for model in self.nodes.iter_mut().flatten() {
            model.table.core_beacons[core].vmx = on;
        }
    }

    /// A firmware stall stops every core for `duration`: no event, no
    /// heartbeat.
    pub fn stall(&mut self, duration: u64) {
        self.now += duration;
    }

    /// After an event of `node`: its messages are in flight, its requests
    /// to the hardware are made, its core beacon is written, and every node
    /// reads the machine table as that node left it.
    fn after(&mut self, node: u16) {
        let model = self.nodes[node as usize]
            .as_mut()
            .expect("a node with a kernel");
        for packet in &model.outbox {
            model.table.rings[packet.from as usize][packet.to as usize].0 += 1;
        }
        self.in_flight.append(&mut model.outbox);
        let core = model.table.placed[node as usize];
        let idle = model.running().is_none();
        let until = if idle {
            model.next_instant().unwrap_or(u64::MAX)
        } else {
            0
        };
        let beacon = &mut model.table.core_beacons[core];
        (beacon.heartbeat.0, beacon.running, beacon.idle_until.0) = (model.events.0, node, until);
        let actions = std::mem::take(&mut model.hw_out);
        let table = model.table.clone();
        for other in self.nodes.iter_mut().flatten() {
            other.table = table.clone();
        }
        for action in actions {
            self.hardware(action);
        }
        self.drop_dead();
    }

    /// The hardware acts.
    fn hardware(&mut self, action: HwAction) {
        let booted = |s: &Self, core: usize| s.table().booted[core];
        match action {
            HwAction::Nmi { core, epoch } => match self.cores[core] {
                CoreState::Running | CoreState::Hung { nmi: true } => {
                    // The handler sees its member word in fencing, writes
                    // its record (reason 5) unless a committed record of the
                    // same generation is there, and stops in a loop; on the
                    // bootstrap core it parks it.
                    let now = self.now;
                    let running = self.table().core_beacons[core].running;
                    if (running as usize) < self.nodes.len() {
                        let member = self.table().member(running);
                        let record = crate::debug::Record {
                            commit: 0,
                            generation: member.generation(),
                            reason: 5,
                            instant: now,
                            member: member.raw(),
                        };
                        self.write_beacon(|t| {
                            std::sync::Arc::make_mut(&mut t.crash).write_record(running, record)
                        });
                    }
                    if booted(self, core) {
                        self.cores[core] = CoreState::Parked;
                        self.write_beacon(|t| t.core_beacons[core].parked = epoch);
                    } else {
                        self.cores[core] = CoreState::Looping;
                        self.write_beacon(|t| t.core_beacons[core].vmx = false);
                    }
                }
                _ => {}
            },
            HwAction::Init { core } => {
                let vmx = self.table().core_beacons[core].vmx;
                if !vmx && self.cores[core] != CoreState::Off {
                    self.cores[core] = CoreState::InitPending;
                }
            }
            HwAction::Sipi { core, epoch } => {
                if self.cores[core] == CoreState::InitPending {
                    self.cores[core] = CoreState::Parked;
                    self.write_beacon(|t| t.core_beacons[core].parked = epoch);
                }
            }
            HwAction::Signal { core, epoch } => {
                let class = self.table().fence[core];
                let handler = matches!(
                    self.cores[core],
                    CoreState::Running | CoreState::Hung { nmi: true }
                );
                match class {
                    dodos_abi::FENCE_CLASS_FORCED => {
                        self.cores[core] = CoreState::Off;
                        self.write_beacon(|t| t.off[core] = true);
                    }
                    dodos_abi::FENCE_CLASS_SIGNALLED if handler => {
                        self.cores[core] = CoreState::Off;
                        self.write_beacon(|t| t.off[core] = true);
                    }
                    dodos_abi::FENCE_CLASS_COOPERATIVE if handler => {
                        self.cores[core] = CoreState::Parked;
                        self.write_beacon(|t| t.core_beacons[core].parked = epoch);
                    }
                    _ => {}
                }
            }
            // The core is parked only when no node that is starting, live or
            // sealing remains placed on it; otherwise it saves the node's
            // block and goes on running the others.
            HwAction::ParkSelf { node } => {
                let core = self.table().placed[node as usize];
                let other = (0..self.nodes.len() as u16).find(|&z| {
                    z != node && self.table().placed[z as usize] == core && self.table().writes(z)
                });
                match other {
                    None => self.cores[core] = CoreState::Parked,
                    Some(other) => self.write_beacon(|t| {
                        t.beacons[node as usize].save_seal = true;
                        t.core_beacons[core].running = other;
                    }),
                }
            }
            HwAction::Start { node, mode } => self.start_node(node, mode),
        }
    }

    /// Writes what the hardware writes into the shared memory every node
    /// reads.
    fn write_beacon(&mut self, write: impl Fn(&mut crate::table::Table)) {
        for model in self.nodes.iter_mut().flatten() {
            write(&mut model.table);
        }
    }

    /// `core_start`: a fresh start builds the node's start-up state from
    /// its floor and the kept image; a resume loads its sealed block. The
    /// node then writes live.
    fn start_node(&mut self, node: u16, mode: u64) {
        let table = self.table().clone();
        let core = table.placed[node as usize];
        let resume = mode & 0xFF == dodos_abi::CORE_START_RESUME;
        let replacement = mode >> 8 == dodos_abi::CORE_IMAGE_REPLACEMENT;
        if resume {
            let model = self.model(node);
            model.table = table;
            model.table.beacons[node as usize].save_seal = false;
        } else {
            // A fresh start writes the node's beacon anew: nothing of the
            // generation that is over stays in it (its failure, what it
            // acknowledged, whom it mapped, what it observed).
            let mut table = table;
            let nodes = table.nodes as usize;
            table.beacons[node as usize] = crate::table::NodeBeacon {
                acked: vec![0; nodes],
                maps_of: vec![false; nodes],
                ..Default::default()
            };
            let (mut model, _) =
                Model::start(&self.machine, &self.layout, table, node).expect("started once");
            model.shared = true;
            self.nodes[node as usize] = Some(model);
        }
        self.cores[core] = CoreState::Running;
        let now = self.now;
        let readable = !resume || !replacement || self.machine.replacement_reads_sealed;
        let model = self.model(node);
        if model.now() < now {
            model.advance(now);
        }
        model.started(readable);
        self.after(node);
    }

    /// A node that became dead has no kernel: its model is dropped, and the
    /// gate clears the map counts of its tables, their page classes, and
    /// its grant and use tables (R6): every entry becomes free and its
    /// serial grows, so no name of this generation is used again.
    fn drop_dead(&mut self) {
        for node in 0..self.nodes.len() {
            let dead = self.nodes[node]
                .as_ref()
                .is_some_and(|m| m.table.member(node as u16).state() == MemberState::Dead);
            if !dead {
                continue;
            }
            let model = self.nodes[node].take().expect("checked");
            let leaves = model.chunk_leaves();
            let entry_leaves = model.entry_leaves();
            let tables = model.table_pages();
            let core = model.table.placed[node];
            self.write_beacon(|t| {
                for (chunk, &own) in leaves.iter().enumerate() {
                    t.add_leaves(chunk as u64, -(own as i64));
                }
                for (count, own) in std::sync::Arc::make_mut(&mut t.device_leaves)
                    .iter_mut()
                    .zip(&entry_leaves)
                {
                    *count -= own;
                }
                for &page in &tables {
                    t.set_class(page, None);
                }
                t.reports[node].clear();
                for (grant, entry) in std::mem::take(&mut t.grants[node]) {
                    t.serials[node].insert(grant, (entry.serial + 1) & 0x3FFF_FFFF);
                }
                t.uses[node].clear();
            });
            if matches!(
                self.cores[core],
                CoreState::Looping | CoreState::Hung { .. } | CoreState::InitPending
            ) {
                self.cores[core] = CoreState::Parked;
            }
        }
    }

    /// Every invariant of every node, and those between nodes. That no
    /// chunk is covered by blocks of two nodes (8.2, invariant 2) follows
    /// from 53 and 54 on every node, which read the same words; that the
    /// memory under a revoked grant returns to an untyped only after the
    /// holder has no mapping under it (8.2, invariant 8) follows from 58 on
    /// the holder, which counts every leaf of granted memory in its use
    /// entry and requires the grant entry to be there.
    /// 58. every map count equals the leaves of every node that target its
    ///     chunk;
    /// 71. a grant entry is live or revoking only while its anchor is in the
    ///     granting node's kernel space, and only its grantee holds a
    ///     remote object of a live grant with its serial;
    /// 72. a use entry names a grant of another node, and is the use entry
    ///     of exactly one remote object of its node;
    /// 73. a remote object names a grant of another node, never a slot;
    /// and those of cores and roles (8.4, safety):
    /// 80. no chunk is covered by blocks of two nodes (judged first: on
    ///     each node, 54 would report it otherwise);
    /// 81. a context entry of a unit names a node that is not dead, with
    ///     its current generation (no write of a device reaches memory
    ///     taken after a death);
    /// 82. a unit's registers are mapped only by the holder its role word
    ///     names, and by nobody while the word is handing;
    /// 83. a unit role is never vacant and never held by a sealed or dead
    ///     node; a role word changes without its holder's write only when
    ///     the holder may lose it: fenced, unfenced or dead (reserve: dead),
    ///     or vacant, or by the target of a hand-over;
    /// 84. a node becomes dead only when its core is stopped and every
    ///     node that is starting, live or sealing has acknowledged it or
    ///     maps nothing of it;
    /// 85. every member word written follows a row of the table of
    ///     transitions: a start only from dead (generation + 1) or sealed
    ///     (the same);
    /// 86. one node at least is live;
    /// 87 (audit: every word under a live block of a live node reads taken
    ///     by it with its generation) is 54 on each node.
    pub fn check(&self) -> Check {
        self.check_owners()?;
        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(node) = node {
                node.check()
                    .map_err(|broken| format!("node {index}: {broken}"))?;
            }
        }
        let ensure = |holds: bool, invariant: &str| match holds {
            true => Ok(()),
            false => Err(invariant.to_string()),
        };
        let table = self.table();
        let mut leaves = vec![0u64; table.gate_words.len()];
        let mut entries = vec![0u64; table.device_leaves.len()];
        for node in self.nodes.iter().flatten() {
            for (sum, own) in leaves.iter_mut().zip(node.chunk_leaves()) {
                *sum += own;
            }
            for (sum, own) in entries.iter_mut().zip(node.entry_leaves()) {
                *sum += own;
            }
        }
        let counts = (0..leaves.len() as u64).map(|c| table.map_count(c));
        ensure(
            leaves.iter().copied().eq(counts),
            "58: map count of the machine",
        )?;
        ensure(
            entries == *table.device_leaves,
            "58: leaf count of a device entry of the machine",
        )?;
        let tables: BTreeSet<u64> = self
            .nodes
            .iter()
            .flatten()
            .flat_map(|n| n.table_pages())
            .collect();
        let classes: BTreeSet<u64> = table.classes.keys().copied().collect();
        ensure(tables == classes, "59: page classes of the machine")?;
        for (granter, entries) in table.grants.iter().enumerate() {
            let Some(node) = &self.nodes[granter] else {
                ensure(entries.is_empty(), "71: grant of a dead node")?;
                continue;
            };
            for (&grant, entry) in entries {
                ensure(node.anchor(grant).is_some(), "71: grant without its anchor")?;
                // A remote object of this grant, with its serial, is held by
                // its grantee only.
                for (holder, uses) in table.uses.iter().enumerate() {
                    // A use entry of an earlier generation of the granting
                    // node names nothing of this one.
                    let generation = table.generation(granter as u16);
                    let held = uses.values().any(|u| {
                        u.granter == granter as u16
                            && u.grant == grant
                            && u.serial == entry.serial
                            && u.generation == generation
                    });
                    let live = entry.state == GrantState::Live;
                    ensure(
                        !held || !live || holder == entry.grantee as usize,
                        "71: remote object held by another node",
                    )?;
                }
            }
        }
        for (holder, uses) in table.uses.iter().enumerate() {
            let Some(node) = &self.nodes[holder] else {
                continue;
            };
            let named: BTreeSet<u16> = node.remotes.values().map(|r| r.use_entry).collect();
            let entries: BTreeSet<u16> = uses.keys().copied().collect();
            ensure(named == entries, "72: use entries and remote objects")?;
            for used in uses.values() {
                ensure(
                    used.granter as usize != holder,
                    "72: use of a grant of its own node",
                )?;
            }
            for remote in node.remotes.values() {
                ensure(
                    remote.node as usize != holder,
                    "73: remote object of its own node",
                )?;
            }
        }
        self.check_cores()
    }

    /// 80: blocks of two nodes on one chunk.
    fn check_owners(&self) -> Check {
        let mut covered: BTreeSet<u64> = BTreeSet::new();
        for node in self.nodes.iter().flatten() {
            let shift = node.table.geometry.chunk_shift();
            let own: BTreeSet<u64> = node
                .untypeds
                .values()
                .filter(|u| u.block == Some(crate::Block::Run))
                .flat_map(|u| u.base >> shift..(u.base + u.bytes) >> shift)
                .collect();
            if !covered.is_disjoint(&own) {
                return Err("80: a chunk under blocks of two nodes".into());
            }
            covered.extend(own);
        }
        Ok(())
    }

    /// 81 to 86.
    fn check_cores(&self) -> Check {
        let ensure = |holds: bool, invariant: &str| match holds {
            true => Ok(()),
            false => Err(invariant.to_string()),
        };
        let table = self.table();
        for unit in table.units.values() {
            for &(node, generation) in unit.entries.values() {
                let member = table.member(node);
                let current =
                    member.state() != MemberState::Dead && member.generation() == generation;
                ensure(current, "81: context entry of a dead generation")?;
            }
        }
        for (role, unit) in &table.units {
            let word = table.roles[*role];
            let mapped = match unit.mapped {
                None => true,
                Some(node) => word.state() == RoleState::Held && word.holder() == node,
            };
            ensure(
                mapped,
                "82: unit mapped by a node its role word does not name",
            )?;
        }
        for (record, word) in table.role_records.iter().zip(&table.roles) {
            if record.kind != RoleKind::Unit {
                continue;
            }
            ensure(word.state() != RoleState::Vacant, "83: a unit role vacant")?;
            if word.state() == RoleState::Held {
                let state = table.member(word.holder()).state();
                let bad = matches!(state, MemberState::Sealed | MemberState::Dead);
                ensure(!bad, "83: a unit role held by a sealed or dead node")?;
            }
        }
        for &(role, writer, before, _) in table.role_writes.iter() {
            let kind = table.role_records[role].kind;
            let holder = before.holder();
            let own = before.state() == RoleState::Vacant
                || holder == writer
                || (before.state() == RoleState::Handing && before.target() == writer);
            let lost = |state: MemberState| match kind {
                RoleKind::Reserve => state == MemberState::Dead,
                _ => matches!(
                    state,
                    MemberState::Fenced | MemberState::Unfenced | MemberState::Dead
                ),
            };
            let target_lost =
                before.state() == RoleState::Handing && lost(table.member(before.target()).state());
            ensure(
                own || lost(table.member(holder).state()) || target_lost,
                "83: a role taken from its holder",
            )?;
        }
        for &(node, _, from, to) in table.member_writes.iter() {
            ensure(
                dodos_abi::member_transition(from, to).is_some(),
                "85: a member word written off the table of transitions",
            )?;
            if to.state() == MemberState::Dead && from.state() != MemberState::Dead {
                let core = table.placed[node as usize];
                let others_here = (0..table.nodes)
                    .any(|z| z != node && table.placed[z as usize] == core && self.runs(z));
                let stopped =
                    matches!(self.cores[core], CoreState::Parked | CoreState::Off) || others_here;
                ensure(stopped, "84: dead while its core is not stopped")?;
                let acknowledged = (0..table.nodes)
                    .filter(|&y| y != node && table.writes(y))
                    .all(|y| {
                        let beacon = &table.beacons[y as usize];
                        beacon.acked[node as usize] == to.generation()
                            || !beacon.maps_of[node as usize]
                    });
                ensure(acknowledged, "84: dead before every node acknowledged it")?;
            }
        }
        let live = (0..table.nodes).any(|n| table.member(n).state() == MemberState::Live);
        ensure(live, "86: no live node")
    }

    /// The bodies in flight, for a test to find the one it wants.
    pub fn find(&self, wanted: impl Fn(&Body) -> bool) -> Option<usize> {
        self.in_flight.iter().position(|p| wanted(&p.body))
    }
}

#[cfg(test)]
#[path = "units_tests.rs"]
mod units_tests;

#[cfg(test)]
#[path = "survivors_tests.rs"]
mod survivors_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{GrantEntry, UseEntry};
    use crate::{Message, Range, Transfer, live};
    use dodos_abi::{DRIVER_NONE, Evidence, ObjectKind, PageSize, Rights, SlotAddr};

    const OBJ: u32 = 101;
    const GOT: u32 = 110;

    /// Two nodes; node 0 has granted node 1 a notify, whose remote
    /// capability is in slot `GOT` of node 1.
    fn granted() -> (System, Vec<ObjId>) {
        let (mut sys, t) = System::boot(&Machine::example()).unwrap();
        let retype = |kind, destination| Syscall::UntypedRetype {
            untyped: SlotAddr::own(25),
            kind,
            size: 0,
            page: PageSize::Page as u64,
            memory: 0,
            destination: SlotAddr::own(destination),
        };
        for node in 0..2 {
            assert_eq!(
                sys.apply(node, t[node as usize], retype(ObjectKind::Reply, 100)),
                Ok(crate::Output::Done)
            );
        }
        assert_eq!(
            sys.apply(0, t[0], retype(ObjectKind::Notify, OBJ)),
            Ok(crate::Output::Done)
        );
        let recv = Syscall::Recv {
            endpoint: SlotAddr::own(dodos_abi::START_SLOT_MAILBOX),
            reply: SlotAddr::own(100),
            range: Range {
                first: SlotAddr::own(GOT),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(sys.apply(1, t[1], recv), Ok(crate::Output::Waits));
        let all = Rights::ALL.without(Rights::EXEC).bits();
        let send = Syscall::Send {
            target: SlotAddr::own(17),
            message: Message {
                label: 1,
                flags: 0,
                words: vec![1],
                caps: vec![Transfer::Derive(SlotAddr::own(OBJ), all)],
                marks: 0,
            },
        };
        assert_eq!(sys.apply(0, t[0], send), Ok(crate::Output::Waits));
        assert_eq!(sys.settle(16), 4);
        assert_eq!(sys.check(), Ok(()));
        assert_eq!(sys.nodes[1].as_mut().unwrap().remotes.len(), 1);
        (sys, t)
    }

    /// A fresh start leaves nothing of the generation that is over in the
    /// node's beacon: no failure, no acknowledgement, no dependency, no
    /// observation (pass 14, words of a generation that no longer exists).
    #[test]
    fn a_fresh_start_writes_the_node_beacon_anew() {
        let (mut sys, t) = System::boot(&Machine::example()).unwrap();
        sys.panic(1, 7);
        let mut at = 0;
        while sys.member(1).state() != MemberState::Dead {
            at += 100_000;
            sys.run(at, 100_000);
            assert!(at < 50_000_000, "node 1 dies");
        }
        assert!(sys.table().beacons[1].panic.is_some());
        tables(&mut sys, |table| {
            table.beacons[1].acked[0] = 9;
            table.beacons[1].maps_of[0] = true;
        });
        let start = Syscall::CoreStart {
            core: SlotAddr::own(17),
            mode: dodos_abi::CORE_START_FRESH,
            image: 0,
            module: 0,
        };
        assert_eq!(sys.apply(0, t[0], start), Ok(crate::Output::Done));
        let beacon = &sys.table().beacons[1];
        assert_eq!(beacon.panic, None);
        assert_eq!((beacon.acked[0], beacon.maps_of[0]), (0, false));
        assert!(beacon.observations.is_empty());
        assert_eq!(sys.check(), Ok(()));
    }

    /// Changes the machine table of every node the same way.
    fn tables(sys: &mut System, change: impl Fn(&mut crate::table::Table)) {
        for node in sys.nodes.iter_mut().flatten() {
            change(&mut node.table);
        }
    }

    /// Node 1 waits on its mailbox for one capability; node 0 sends it a
    /// derived capability for its notify. Returns the flags of the message
    /// node 1 gets.
    fn second_transfer(sys: &mut System, t: &[ObjId]) -> u32 {
        let recv = Syscall::Recv {
            endpoint: SlotAddr::own(dodos_abi::START_SLOT_MAILBOX),
            reply: SlotAddr::own(100),
            range: Range {
                first: SlotAddr::own(GOT + 1),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(sys.apply(1, t[1], recv), Ok(crate::Output::Waits));
        let send = Syscall::Send {
            target: SlotAddr::own(17),
            message: Message {
                label: 2,
                flags: 0,
                words: vec![2],
                caps: vec![Transfer::Derive(SlotAddr::own(OBJ), Rights::SEND.bits())],
                marks: 0,
            },
        };
        assert_eq!(sys.apply(0, t[0], send), Ok(crate::Output::Waits));
        sys.settle(16);
        match sys.nodes[1].as_mut().unwrap().woken(t[1]) {
            Some(Ok(crate::Output::Message(delivery))) => delivery.received.header.flags(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_full_grant_table_drops_the_capabilities_of_a_transfer() {
        let (mut sys, t) = granted();
        // Every other entry of node 0's grant table is taken.
        let entry = GrantEntry {
            grantee: 1,
            generation: 0,
            serial: 0,
            state: GrantState::Live,
        };
        tables(&mut sys, |t| {
            for grant in 1..dodos_abi::GRANTS as u16 {
                t.grants[0].insert(grant, entry);
            }
        });
        assert_eq!(
            second_transfer(&mut sys, &t),
            dodos_abi::Header::FLAG_CAPS_DROPPED
        );
    }

    #[test]
    fn a_full_use_table_on_the_receiver_drops_the_capabilities_and_frees_the_grant() {
        let (mut sys, t) = granted();
        let used = UseEntry {
            granter: 0,
            grant: 9,
            serial: 0,
            generation: 0,
            leaves: 0,
        };
        tables(&mut sys, |t| {
            for entry in 1..dodos_abi::GRANTS as u16 {
                t.uses[1].insert(entry, used);
            }
        });
        assert_eq!(
            second_transfer(&mut sys, &t),
            dodos_abi::Header::FLAG_CAPS_DROPPED
        );
        // The grant made for it was freed at the refusal: only grant 0 is
        // left on node 0.
        assert_eq!(sys.nodes[0].as_mut().unwrap().table.grants[0].len(), 1);
    }

    /// A message that names a grant, sent by a node that is not the grantee
    /// with the generation the entry records, is refused and traced (Who
    /// may send what); the right sender's is acted on.
    #[test]
    fn a_release_from_a_node_that_is_not_the_grantee_is_refused_and_traced() {
        let (mut sys, _) = granted();
        let packet = |generation| Packet {
            from: 1,
            generation,
            to: 0,
            body: Body::Release {
                grant: 0,
                serial: 0,
            },
        };
        // The entry records another generation of node 1 than the one that
        // sends: the release is refused.
        tables(&mut sys, |t| live(&mut t.grants[0], 0).generation = 7);
        sys.in_flight.push(packet(1));
        sys.settle(1);
        assert!(sys.node(0).grant_entry(0).is_some());
        let traced = sys.node(0).ring(false);
        let entry = traced.last().expect("an entry");
        assert_eq!(entry.code(), dodos_abi::TraceCode::Refused as u16);
        assert_eq!(entry.payload[0], 1 | 1 << 32);
        // From the grantee with its generation, the release frees the entry
        // once its anchor has no descendant.
        tables(&mut sys, |t| live(&mut t.grants[0], 0).generation = 1);
        sys.in_flight.push(packet(1));
        sys.settle(1);
        assert_eq!(sys.node(0).grant_entry(0), None);
    }

    /// A sealed node is never observed, and the gate refuses evidence other
    /// than kill, retire and host against it, even an old observation.
    #[test]
    fn a_sealed_node_is_never_observed_nor_accused_with_stale_evidence() {
        let mut machine = Machine::example();
        machine.membership.stale_ms = 10;
        machine.membership.sweep_ms = 100;
        let (mut sys, t) = System::boot(&machine).unwrap();
        let seal = Syscall::CoreStop {
            core: SlotAddr::own(17),
            mode: dodos_abi::CORE_STOP_SEAL,
        };
        assert_eq!(sys.apply(0, t[0], seal), Ok(crate::Output::Done));
        sys.run(2_000_000, 100_000);
        assert_eq!(sys.member(1).state(), MemberState::Sealed);
        sys.run(300_000_000, 1_000_000);
        let observed = |sys: &System| {
            sys.table().beacons[0]
                .observations
                .iter()
                .any(|o| o.node == 1)
        };
        assert!(!observed(&sys));
        // An observation of its heartbeat, as if made before the seal.
        let core = sys.table().placed[1];
        let heartbeat = sys.table().core_beacons[core].heartbeat.0;
        let now = sys.now();
        tables(&mut sys, |t| {
            t.beacons[0].observations.push(crate::table::Observation {
                node: 1,
                watched: crate::table::Watched::Heartbeat { core, heartbeat },
                at: now,
            })
        });
        sys.run(now + 30_000_000, 1_000_000);
        assert_eq!(sys.member(1).state(), MemberState::Sealed);
    }

    /// A use entry of an earlier generation of the granting node names no
    /// grant of the later one (invariant 71 judges the generation).
    #[test]
    fn a_use_entry_of_an_ended_generation_does_not_count_as_holding_a_live_grant() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| live(&mut t.grants[0], 0).grantee = 0);
        assert_eq!(
            sys.check(),
            Err("71: remote object held by another node".into())
        );
        tables(&mut sys, |t| {
            for used in t.uses[1].values_mut() {
                used.generation = 9;
            }
        });
        assert_eq!(sys.check(), Ok(()));
    }

    #[test]
    fn a_map_count_above_the_leaves_of_every_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0]
            .as_mut()
            .unwrap()
            .table
            .gate_words
            .iter()
            .position(|&m| m as u32 > 0)
            .unwrap() as u64;
        tables(&mut sys, |t| t.add_leaves(chunk, 1));
        assert_eq!(sys.check(), Err("58: map count of the machine".into()));
    }

    #[test]
    fn a_map_count_below_the_leaves_of_one_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0]
            .as_mut()
            .unwrap()
            .table
            .gate_words
            .iter()
            .position(|&m| m as u32 > 0)
            .unwrap() as u64;
        tables(&mut sys, |t| t.set_map_count(chunk, 0));
        assert_eq!(sys.check(), Err("node 0: 58: map count".into()));
    }

    #[test]
    fn a_grant_entry_without_its_anchor_is_reported() {
        let (mut sys, _) = granted();
        let entry = GrantEntry {
            grantee: 1,
            generation: 0,
            serial: 0,
            state: GrantState::Live,
        };
        tables(&mut sys, |t| {
            t.grants[0].insert(9, entry);
        });
        assert_eq!(sys.check(), Err("71: grant without its anchor".into()));
    }

    #[test]
    fn a_remote_object_of_a_live_grant_on_another_node_than_its_grantee_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| live(&mut t.grants[0], 0).grantee = 0);
        assert_eq!(
            sys.check(),
            Err("71: remote object held by another node".into())
        );
    }

    #[test]
    fn a_use_entry_of_no_remote_object_is_reported() {
        let (mut sys, _) = granted();
        let used = UseEntry {
            granter: 0,
            grant: 0,
            serial: 0,
            generation: 0,
            leaves: 0,
        };
        tables(&mut sys, |t| {
            t.uses[1].insert(7, used);
        });
        assert_eq!(
            sys.check(),
            Err("72: use entries and remote objects".into())
        );
    }

    #[test]
    fn a_use_entry_of_a_grant_of_its_own_node_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| {
            for used in t.uses[1].values_mut() {
                used.granter = 1;
            }
        });
        assert_eq!(
            sys.check(),
            Err("72: use of a grant of its own node".into())
        );
    }

    #[test]
    fn a_remote_object_of_its_own_node_is_reported() {
        let (mut sys, _) = granted();
        for remote in sys.nodes[1].as_mut().unwrap().remotes.values_mut() {
            remote.node = 1;
        }
        assert_eq!(sys.check(), Err("73: remote object of its own node".into()));
    }

    // Cores and roles.

    fn live_member(g: u32) -> Member {
        Member::new(MemberState::Live, Evidence::None, DRIVER_NONE, g)
    }

    #[test]
    fn blocks_of_two_nodes_on_a_chunk_are_reported() {
        let (mut sys, t) = granted();
        let ram = Machine::example().memory[4].base;
        let shift = sys.table().geometry.chunk_shift();
        let take = Syscall::MemoryTake {
            control: SlotAddr::own(dodos_abi::START_SLOT_MEMORY_CONTROL),
            kind: 0,
            place: ram + (8 << shift),
            size: u64::from(shift),
            destination: SlotAddr::own(120),
        };
        assert_eq!(sys.apply(0, t[0], take), Ok(crate::Output::Done));
        let block = sys.nodes[0]
            .as_ref()
            .unwrap()
            .untypeds
            .values()
            .find(|u| u.block == Some(crate::Block::Run))
            .unwrap()
            .clone();
        sys.nodes[1]
            .as_mut()
            .unwrap()
            .untypeds
            .insert(crate::ObjId(u32::MAX), block);
        assert_eq!(
            sys.check(),
            Err("80: a chunk under blocks of two nodes".into())
        );
    }

    #[test]
    fn a_context_entry_of_a_dead_generation_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| {
            let unit = t.units.values_mut().next().unwrap();
            unit.entries.insert(5, (1, 0));
        });
        assert_eq!(
            sys.check(),
            Err("81: context entry of a dead generation".into())
        );
    }

    #[test]
    fn a_unit_mapped_by_another_node_than_its_holder_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| {
            t.units.values_mut().next().unwrap().mapped = Some(0)
        });
        assert_eq!(
            sys.check(),
            Err("82: unit mapped by a node its role word does not name".into())
        );
    }

    #[test]
    fn a_vacant_unit_role_or_one_held_by_a_sealed_node_is_reported() {
        let (mut sys, _) = granted();
        tables(&mut sys, |t| {
            t.roles[1] = dodos_abi::RoleWord::vacant(2);
        });
        assert_eq!(sys.check(), Err("83: a unit role vacant".into()));
        let (mut sys, _) = granted();
        let sealed = Member::new(MemberState::Sealed, Evidence::None, DRIVER_NONE, 1);
        tables(&mut sys, |t| t.members[1] = sealed);
        assert_eq!(
            sys.check(),
            Err("83: a unit role held by a sealed or dead node".into())
        );
    }

    #[test]
    fn a_role_taken_from_a_live_holder_is_reported() {
        let (mut sys, _) = granted();
        let (before, after) = (
            dodos_abi::RoleWord::held(1, 1),
            dodos_abi::RoleWord::held(0, 2),
        );
        tables(&mut sys, |t| {
            t.roles[2] = after;
            t.role_writes.push((2, 0, before, after));
        });
        assert_eq!(sys.check(), Err("83: a role taken from its holder".into()));
    }

    #[test]
    fn a_node_dead_before_its_stop_or_its_acknowledgements_is_reported() {
        let (mut sys, _) = granted();
        let fenced = Member::new(MemberState::Fenced, Evidence::Kill, 0, 1);
        let dead = Member::new(MemberState::Dead, Evidence::Kill, 0, 1);
        tables(&mut sys, |t| t.member_writes.push((1, 0, fenced, dead)));
        assert_eq!(
            sys.check(),
            Err("84: dead while its core is not stopped".into())
        );
        sys.cores[1] = CoreState::Parked;
        tables(&mut sys, |t| t.beacons[0].maps_of[1] = true);
        assert_eq!(
            sys.check(),
            Err("84: dead before every node acknowledged it".into())
        );
    }

    #[test]
    fn a_member_word_written_off_the_table_is_reported() {
        let (mut sys, _) = granted();
        let starting = Member::new(MemberState::Starting, Evidence::None, DRIVER_NONE, 1);
        tables(&mut sys, |t| {
            t.member_writes.push((1, 0, live_member(1), starting))
        });
        assert_eq!(
            sys.check(),
            Err("85: a member word written off the table of transitions".into())
        );
    }

    #[test]
    fn a_machine_with_no_live_node_is_reported() {
        let (mut sys, _) = granted();
        let fencing = Member::new(MemberState::Fencing, Evidence::Kill, 0, 1);
        tables(&mut sys, |t| t.members = vec![fencing; 2]);
        assert_eq!(sys.check(), Err("86: no live node".into()));
    }
}
