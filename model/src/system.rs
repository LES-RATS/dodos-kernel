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
use dodos_abi::{Member, MemberState};
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
        let state = model.table.member(node).state();
        self.cores[core] == CoreState::Running
            && !matches!(state, MemberState::Sealed | MemberState::Dead)
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
    pub fn deliver(&mut self, index: usize) {
        let to = self.in_flight[index].to;
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
                    // The handler sees its member word in fencing and stops
                    // in a loop; on the bootstrap core it parks it.
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
            HwAction::ParkSelf { node } => {
                let core = self.table().placed[node as usize];
                self.cores[core] = CoreState::Parked;
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
    /// gate clears the map counts of its tables and their page classes.
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
            let tables = model.table_pages();
            let core = model.table.placed[node];
            self.write_beacon(|t| {
                for (count, own) in t.maps.iter_mut().zip(&leaves) {
                    *count -= own;
                }
                for page in &tables {
                    t.classes.remove(page);
                }
                t.reports[node].clear();
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
    /// 73. a remote object names a grant of another node, never a slot.
    pub fn check(&self) -> Check {
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
        let mut leaves = vec![0u64; table.maps.len()];
        for node in self.nodes.iter().flatten() {
            for (sum, own) in leaves.iter_mut().zip(node.chunk_leaves()) {
                *sum += own;
            }
        }
        ensure(leaves == table.maps, "58: map count of the machine")?;
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
                    let held = uses.values().any(|u| {
                        u.granter == granter as u16 && u.grant == grant && u.serial == entry.serial
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
        Ok(())
    }

    /// The bodies in flight, for a test to find the one it wants.
    pub fn find(&self, wanted: impl Fn(&Body) -> bool) -> Option<usize> {
        self.in_flight.iter().position(|p| wanted(&p.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{GrantEntry, UseEntry};
    use crate::{Message, Range, Transfer, live};
    use dodos_abi::{ObjectKind, PageSize, Rights, SlotAddr};

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

    #[test]
    fn a_map_count_above_the_leaves_of_every_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0]
            .as_mut()
            .unwrap()
            .table
            .maps
            .iter()
            .position(|&m| m > 0)
            .unwrap();
        tables(&mut sys, |t| t.maps[chunk] += 1);
        assert_eq!(sys.check(), Err("58: map count of the machine".into()));
    }

    #[test]
    fn a_map_count_below_the_leaves_of_one_node_is_reported() {
        let (mut sys, _) = granted();
        let chunk = sys.nodes[0]
            .as_mut()
            .unwrap()
            .table
            .maps
            .iter()
            .position(|&m| m > 0)
            .unwrap();
        tables(&mut sys, |t| t.maps[chunk] = 0);
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
}
