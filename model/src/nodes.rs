//! Cores (D-115 to D-117, D-128, D-129, D-131), what one node's kernel does:
//! the calls `node_start`, `node_stop`, `node_status`, `node_idle`,
//! `node_watch` and `node_perf`; the membership of nodes (evidence, the
//! sweep, accusation); the fence of every core of a node it drives; the
//! reclaim of a node, R0 to R6, and the barrier; the seal and the resume of
//! its own node; the starts it drives; and the recovery reserve in whose
//! steps all of it runs.
//!
//! What a node asks of the machine (stops, starts, parks, interrupts to
//! other nodes) is a `HwAction`, which `System` carries out.

use crate::table::FenceStep;
use crate::table::{FenceRecord, Observation, Watched};
use crate::{Model, ObjId, Outcome, Output, Wait, live, locked};
use dodos_abi::{
    CoreState, CoreWord, DRIVER_NONE, Error, Evidence, FENCE_NS, IDLE_ENTRY_WORDS,
    IDLE_FLAG_LOSES_CONTEXT, IDLE_METHOD_HALT, IDLE_METHOD_MWAIT, IDLE_METHOD_PSCI,
    IDLE_METHOD_SBI, IDLE_STATES_MAX, Member, MemberState, NODE_START_FRESH, NODE_START_RESUME,
    NODE_STOP_KILL, NODE_STOP_RETIRE, NODE_STOP_SEAL, NODE_WATCH_SLOTS, OBSERVATION_LIFE,
    OBSERVATIONS_MAX, ObjectKind, RECOVERY_PERIOD_NS, REMOVAL_STEP, Rights, RoleKind, RoleState,
    SETTLE_NS, START_NS, START_RETRY_NS, SlotAddr,
};

/// What a node asks of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwAction {
    /// The fence stops a physical core by the means of its architecture
    /// (x86-64: NMI, INIT unless it is the bootstrap core, SIPI twice to
    /// the park stub; ARM64: SDEI event 0; RISC-V: the stop extension).
    Stop { core: usize },
    /// A parked core is started: its start request is written; x86-64
    /// sends an NMI to the park stub, ARM64 calls `CPU_ON`, RISC-V
    /// `sbi_hart_start`.
    Start { core: usize },
    /// A core of this node parks itself (a seal, a failure, its own stop).
    Park { core: usize },
    /// The interrupt of a ring to another node, at its first core.
    Ring { node: u16 },
    /// An interrupt to a core of another node: the notice of a failure, a
    /// sweep's wake.
    Interrupt { node: u16 },
    /// Containment is violated: the machine stops with a stop record.
    StopMachine,
}

/// A fence this node drives: the generation, and the instant of its last
/// round of stops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fence {
    pub(crate) generation: u32,
    pub(crate) last: u64,
}

/// The phases of step R6, a cursor over the words of the living part.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Clear {
    pub(crate) phase: u8,
    pub(crate) index: usize,
}

/// This node's reclaim of another node's generation.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reclaim {
    pub(crate) generation: u32,
    /// R0 done (at the accusation).
    pub(crate) isolated: bool,
    /// R1 submitted to each unit this node holds, by role, and done.
    pub(crate) r1: std::collections::BTreeMap<usize, Option<u64>>,
    pub(crate) r3: bool,
    /// R4: the blocks still to audit, once begun.
    pub(crate) audit: Option<Vec<ObjId>>,
    pub(crate) acked: bool,
    /// R6: the cursor of its walk.
    pub(crate) clear: Option<Clear>,
}

/// The seal of this node (`node_stop` mode seal), its steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SealStep {
    /// Handing its unit roles over, answering its kernel rings, completing
    /// the requests of its cores and waiting for its unit requests sent.
    Quiescing,
    /// Its other cores asked to park; the last core writes sealed once
    /// each is confirmed.
    Parking,
}

/// A start this node drives: the node and generation named, the start
/// request written, the instant of the last wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Starting {
    pub(crate) node: u16,
    pub(crate) generation: u32,
    pub(crate) request: u64,
    pub(crate) last: u64,
    pub(crate) since: u64,
}

/// The fence repeats its stops every millisecond for a core not confirmed.
pub(crate) const FENCE_RETRY_NS: u64 = 1_000_000;

impl Model {
    /// The `node` capability of a call: its node slot.
    fn node_of(&self, thread: ObjId, node: SlotAddr, right: Rights) -> Result<u16, Error> {
        let node = self.object(thread, node, ObjectKind::Node, right)?;
        Ok(self.nodes[&node])
    }

    /// The live nodes other than this one; phantoms count (they hold no
    /// core) only where the contract does not exclude them.
    fn others_live(&self, except: u16, phantoms: bool) -> bool {
        (0..self.table.nodes).any(|z| {
            z != except
                && self.table.member(z).state() == MemberState::Live
                && (phantoms || !self.table.cores_of(z, self.table.generation(z)).is_empty())
        })
    }

    // The calls.

    /// `node_start`: mode fresh on a set of free cores, or mode resume of a
    /// sealed node on its own cores.
    pub(crate) fn node_start(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        (mode, image, module): (u64, u64, u64),
        (window, mask): (u64, u64),
    ) -> Outcome {
        let target = self.node_of(thread, node, Rights::MANAGE)?;
        // Plain arguments.
        if mode > NODE_START_RESUME || image > dodos_abi::NODE_IMAGE_REPLACEMENT {
            return Err(Error::Invalid);
        }
        let cores: Vec<usize> = match mode {
            NODE_START_FRESH => {
                let first_service = self.first_service_module(module);
                if !first_service || window >= dodos_abi::CORE_WINDOWS || mask == 0 {
                    return Err(Error::Invalid);
                }
                let cores: Vec<usize> = (0..64)
                    .filter(|i| mask >> i & 1 == 1)
                    .map(|i| (64 * window + i) as usize)
                    .collect();
                if cores.len() > self.table.cores_supported as usize {
                    return Err(Error::Unsupported);
                }
                cores
            }
            _ => {
                if module != 0 || window != 0 || mask != 0 {
                    return Err(Error::Invalid);
                }
                Vec::new()
            }
        };
        let member = self.table.member(target);
        let generation = member.generation();
        match mode {
            NODE_START_FRESH => {
                if member.state() != MemberState::Dead {
                    return Err(Error::Busy);
                }
                let count = self.table.core_words.len();
                let free = |c: &usize| {
                    *c < count
                        && self
                            .table
                            .core_word(*c)
                            .is_some_and(|w| w.state() == CoreState::Free && !w.is_unusable())
                };
                let alike = cores.iter().all(|&c| {
                    c < count && self.table.state_kinds[c] == self.table.state_kinds[cores[0]]
                });
                if !cores.iter().all(free) || !alike {
                    return Err(Error::Busy);
                }
            }
            _ => {
                let parked = self
                    .table
                    .cores_of(target, generation)
                    .iter()
                    .all(|&c| self.park_confirmed(c));
                if member.state() != MemberState::Sealed || !parked {
                    return Err(Error::Busy);
                }
            }
        }
        if image == dodos_abi::NODE_IMAGE_REPLACEMENT && !self.table.replacement {
            return Err(Error::Busy);
        }
        // Effect. Fresh: in one gate call, the member word becomes starting
        // (g + 1) with the class of the cores named, and each of their
        // words joining; a compare-and-swap that fails puts the words back
        // and the call is `busy`. Resume: starting (g).
        let (next, cores) = match mode {
            NODE_START_FRESH => {
                let next = generation.wrapping_add(1);
                let class = cores
                    .iter()
                    .map(|&c| self.table.fence[c])
                    .min()
                    .unwrap_or(0);
                let mut written = Vec::new();
                for &core in &cores {
                    let before = self.table.core_words[core];
                    if !self
                        .table
                        .write_core(core, before, CoreWord::joining(target, next))
                    {
                        for (c, b) in written {
                            self.table.core_words[c] = b;
                        }
                        return Err(Error::Busy);
                    }
                    written.push((core, before));
                }
                let starting =
                    Member::new(MemberState::Starting, Evidence::None, DRIVER_NONE, next)
                        .with_class(class);
                self.table.write_member(self.node, target, starting);
                self.table.beacons[target as usize].panic = None;
                (next, cores)
            }
            _ => {
                let starting = Member::new(
                    MemberState::Starting,
                    Evidence::None,
                    DRIVER_NONE,
                    generation,
                )
                .with_class(member.class());
                self.table.write_member(self.node, target, starting);
                (generation, self.table.cores_of(target, generation))
            }
        };
        // The caller writes each core's start request and starts it, in its
        // recovery reserve.
        let request = crate::table::Table::start_request(target, next);
        for core in cores {
            self.starts.insert(
                core,
                Starting {
                    node: target,
                    generation: next,
                    request,
                    last: 0,
                    since: self.now,
                },
            );
        }
        self.start_image = (image == dodos_abi::NODE_IMAGE_REPLACEMENT, module);
        if mode == NODE_START_RESUME {
            self.table.resume_replacement[target as usize] = self.start_image.0;
        }
        Ok(Output::Done)
    }

    /// Whether a module of the kept image is flagged as a first service:
    /// only the segments of the boot image's first service are.
    fn first_service_module(&self, module: u64) -> bool {
        (module as usize) < self.machine.segments.len()
    }

    /// `node_stop`: mode kill, seal or retire.
    pub(crate) fn node_stop(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        mode: u64,
        core: u64,
    ) -> Outcome {
        let target = self.node_of(thread, node, Rights::MANAGE)?;
        let member = self.table.member(target);
        let generation = member.generation();
        if mode > NODE_STOP_RETIRE {
            return Err(Error::Invalid);
        }
        match mode {
            NODE_STOP_RETIRE => {
                let named = (core as usize) < self.table.core_words.len()
                    && self
                        .table
                        .core_word(core as usize)
                        .is_some_and(|w| w.names(target, generation));
                if !named {
                    return Err(Error::Invalid);
                }
            }
            _ if core != 0 => return Err(Error::Invalid),
            _ => {}
        }
        let state = member.state();
        match mode {
            NODE_STOP_SEAL if state != MemberState::Live => return Err(Error::Busy),
            NODE_STOP_KILL | NODE_STOP_RETIRE
                if matches!(
                    state,
                    MemberState::Dead
                        | MemberState::Fencing
                        | MemberState::Fenced
                        | MemberState::Unfenced
                ) =>
            {
                return Err(Error::Busy);
            }
            _ => {}
        }
        // No other node would be live afterwards (a phantom does not
        // count).
        if !self.others_live(target, false) {
            return Err(Error::Busy);
        }
        match mode {
            NODE_STOP_SEAL => {
                let sealing = Member::new(
                    MemberState::Sealing,
                    member.evidence(),
                    member.driver(),
                    generation,
                )
                .with_class(member.class());
                self.table.write_member(self.node, target, sealing);
                if target != self.node {
                    self.hw_out.push(HwAction::Interrupt { node: target });
                }
            }
            _ => {
                if mode == NODE_STOP_RETIRE {
                    let before = self.table.core_words[core as usize];
                    let word = CoreWord::from_raw(before).expect("checked").unusable();
                    self.table.write_core(core as usize, before, word);
                }
                let evidence = match mode {
                    NODE_STOP_KILL => Evidence::Kill,
                    _ => Evidence::Retire,
                };
                self.accuse(target, evidence);
                if target == self.node {
                    // Every core of the node parks itself at the end of its
                    // current event; another live node drives the fence.
                    self.park_all();
                }
            }
        }
        Ok(Output::Done)
    }

    /// `node_status`: shared memory only, no message.
    pub(crate) fn node_status(&mut self, thread: ObjId, node: SlotAddr, core: u64) -> Outcome {
        let target = self.node_of(thread, node, Rights::READ)?;
        let count = self.table.core_words.len();
        if core as usize >= count {
            return Err(Error::Invalid);
        }
        let member = self.table.member(target);
        let mine = self.table.cores_of(target, member.generation());
        let word = self.table.core_word(core as usize);
        let named = word.is_some_and(|w| w.names(target, member.generation()));
        let unusable = word.is_some_and(|w| w.is_unusable());
        let word1 = mine.len() as u64
            | u64::from(member.class()) << dodos_abi::NODE_STATUS_CLASS_SHIFT
            | if unusable {
                dodos_abi::NODE_STATUS_UNUSABLE
            } else {
                0
            };
        let counters = match named {
            true => self.table.core_beacons[core as usize].counters.0,
            false => [0; 4],
        };
        let perf = match self.machine.perf_request {
            true => self.machine.perf_caps,
            false => 0,
        };
        Ok(Output::Status([
            member.raw(),
            word1,
            counters[0],
            counters[1],
            counters[2],
            counters[3],
            perf,
        ]))
    }

    /// The node of `node_idle` and `node_perf`: the caller's own, and the
    /// core one of its cores (`invalid`, judged with the plain arguments,
    /// first).
    fn own_core(&self, thread: ObjId, node: SlotAddr, core: u64) -> Result<usize, Error> {
        let target = self.node_of(thread, node, Rights::WRITE)?;
        match target == self.node {
            true => self.position(core).ok_or(Error::Invalid),
            false => Err(Error::Invalid),
        }
    }

    /// `node_idle`: the idle table of one core of the caller's node.
    pub(crate) fn node_idle(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        core: u64,
        words: &[u64],
    ) -> Outcome {
        let position = self.own_core(thread, node, core)?;
        let table = idle_table(words, self.machine.arch)?;
        self.touch_core(position);
        self.cores[position].idle_table = table;
        Ok(Output::Done)
    }

    /// `node_perf`: the performance request of one core of the caller's
    /// node.
    pub(crate) fn node_perf(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        core: u64,
        request: (u64, u64, u64),
    ) -> Outcome {
        let target = self.node_of(thread, node, Rights::WRITE)?;
        if !self.machine.perf_request {
            return Err(Error::Unsupported);
        }
        if target != self.node {
            return Err(Error::Invalid);
        }
        let position = self.position(core).ok_or(Error::Invalid)?;
        let (lowest, highest, preference) = request;
        if lowest > 0xFF || highest > 0xFF || preference > 0xFF || lowest > highest {
            return Err(Error::Invalid);
        }
        self.touch_core(position);
        self.cores[position].perf = Some(request);
        Ok(Output::Done)
    }

    /// `node_watch`.
    pub(crate) fn node_watch(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        index: u64,
        notify: SlotAddr,
    ) -> Outcome {
        let target = self.node_of(thread, node, Rights::READ)?;
        if target != self.node {
            return Err(Error::Invalid);
        }
        let notify = match notify.is_none() {
            true => None,
            false => {
                let held = self.held(thread, notify, ObjectKind::Notify, Rights::SEND)?;
                let (object, badge) = (held.object, held.badge);
                Some((object, if badge == 0 { 1 } else { badge }))
            }
        };
        if index >= NODE_WATCH_SLOTS {
            return Err(Error::Invalid);
        }
        let slot = &mut self.watchers[index as usize];
        if notify.is_some() && slot.is_some() {
            return Err(Error::Busy);
        }
        *slot = notify;
        Ok(Output::Done)
    }

    // Membership.

    /// The gate accuses a node with evidence, this node its driver. Its
    /// fence begins at the next step of this node's recovery reserve.
    pub(crate) fn accuse(&mut self, node: u16, evidence: Evidence) {
        let member = self.table.member(node);
        let fencing = Member::new(
            MemberState::Fencing,
            evidence,
            self.node,
            member.generation(),
        )
        .with_class(member.class());
        self.table.write_member(self.node, node, fencing);
        if node != self.node {
            self.fences.insert(
                node,
                Fence {
                    generation: member.generation(),
                    last: 0,
                },
            );
        }
    }

    /// A failure the node found in itself: the core writes its crash record
    /// and `panic` in the node beacon, sends an interrupt to a core of the
    /// next live node, and the node's cores park themselves.
    pub(crate) fn accuse_self(&mut self, reason: u32) {
        let me = self.node;
        let generation = self.table.generation(me);
        if self.table.beacons[me as usize]
            .panic
            .is_some_and(|(_, g, _)| g == generation)
        {
            return;
        }
        self.fail(reason);
        let number = self.here().number;
        self.table.beacons[me as usize].panic = Some((u64::from(reason), generation, number));
        let next = (1..self.table.nodes)
            .map(|step| (me + step) % self.table.nodes)
            .find(|&z| self.table.member(z).state() == MemberState::Live);
        if let Some(next) = next {
            self.hw_out.push(HwAction::Interrupt { node: next });
        }
        self.park_all();
    }

    /// Every core of the node parks itself.
    pub(crate) fn park_all(&mut self) {
        for position in 0..self.width() {
            let core = self.cores[position].number as usize;
            self.hw_out.push(HwAction::Park { core });
            self.cores[position].silent = true;
        }
    }

    /// The members and roles this node has not yet processed: watchers are
    /// signalled after every change of a member word or a role word; a node
    /// seen fencing gets R0 at once.
    pub(crate) fn observe_watched(&mut self) {
        let seen = (self.table.members.clone(), self.table.roles.clone());
        if self.seen == seen {
            return;
        }
        self.seen = seen;
        let watchers: Vec<(ObjId, u64)> = self.watchers.iter().flatten().copied().collect();
        for (notify, bits) in watchers {
            if self.notifies.contains_key(&notify) && !self.dying(notify) {
                locked(&mut self.notifies, &self.locks, notify).word |= bits;
                self.hand_over(notify);
            }
        }
        if !self.table.member(self.node).writes() {
            return;
        }
        let me = self.node;
        for node in (0..self.table.nodes).filter(|&z| z != me) {
            let member = self.table.member(node);
            if matches!(
                member.state(),
                MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
            ) {
                self.r0(node, member.generation());
            }
        }
    }

    /// Destroying a notify unbinds it from the watch slots, one unit each.
    pub(crate) fn unwatch_one(&mut self, notify: ObjId) -> bool {
        match self
            .watchers
            .iter_mut()
            .find(|w| w.is_some_and(|(n, _)| n == notify))
        {
            Some(slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    }

    /// The idle, slack and kernel time of the core, published in its core
    /// beacon at each event.
    pub(crate) fn publish_counters(&mut self) {
        let at = *self.at;
        let number = self.cores[at].number as usize;
        let part = &self.cores[at];
        let counters = [part.idle_ns.0, part.slack_ns.0, 0, 0];
        self.table.core_beacons[number].counters.0 = counters;
    }

    /// Whether a core is confirmed parked by its current park request: on
    /// x86-64 `parked` equals the request; on ARM64 and RISC-V the
    /// machine's answer (`AFFINITY_INFO` OFF, STOPPED or the stop
    /// extension's) came after the request with no start since.
    pub(crate) fn park_confirmed(&self, core: usize) -> bool {
        let request = self.table.park_requests[core];
        // x86-64: `parked` equals the request (the handler or the stub
        // wrote it). ARM64 and RISC-V: the firmware's answer, OFF or
        // STOPPED after the request (`AFFINITY_INFO`, the stop extension),
        // never the handler's own write, which comes before `CPU_OFF`.
        match self.machine.arch {
            crate::Arch::X86_64 => self.table.core_beacons[core].parked == request,
            _ => self.table.stopped[core] == request,
        }
    }

    /// The evidence that holds against a node now, as the gate checks it.
    pub(crate) fn evidence(&self, node: u16) -> Option<Evidence> {
        let member = self.table.member(node);
        let g = member.generation();
        let beacon = &self.table.beacons[node as usize];
        if beacon
            .panic
            .is_some_and(|(_, generation, _)| generation == g)
        {
            return Some(Evidence::SelfReported);
        }
        if member.state() == MemberState::Sealed {
            let sealed = beacon.sealed_at.unwrap_or(self.now);
            return (self.now >= sealed + self.table.seal_ns).then_some(Evidence::Seal);
        }
        let now = self.now;
        let me = self.node as usize;
        for observation in self.table.beacons[me]
            .observations
            .iter()
            .filter(|o| o.node == node)
        {
            match observation.watched {
                Watched::Heartbeat { core, heartbeat } => {
                    let named = self.table.core_word(core).is_some_and(|w| w.names(node, g));
                    let beacon = self.table.core_beacons[core];
                    let stale = named
                        && beacon.heartbeat.0 == heartbeat
                        && now >= observation.at + self.table.stale_ns
                        && now - self.table.stale_ns > beacon.idle_until.0;
                    let starting = member.state() == MemberState::Starting
                        && beacon.heartbeat.0 == heartbeat
                        && now >= observation.at + START_NS;
                    if stale || starting {
                        return Some(Evidence::Stale);
                    }
                }
                Watched::Ring { user, consumed } => {
                    if self.ring_consumed(node, user) == consumed
                        && self.ring_watch(node).is_some()
                        && now >= observation.at + self.table.stuck_ns
                    {
                        return Some(Evidence::Stuck);
                    }
                }
            }
        }
        None
    }

    /// The nodes this node needs: those its threads wait for, those it has
    /// messages waiting for, and at the sweep its two successors among the
    /// nodes that are starting, live or sealing.
    fn needed(&self) -> Vec<u16> {
        let mut needed: Vec<u16> = self
            .threads
            .values()
            .filter_map(|t| match t.wait {
                Wait::Node { peer, .. } => Some(peer),
                _ => None,
            })
            .collect();
        for entry in self.unit_requests.iter().flatten() {
            let word = self.table.roles[entry.role];
            if word.state() == RoleState::Held && word.holder() != self.node {
                needed.push(word.holder());
            }
        }
        needed.extend(
            (0..self.table.nodes).filter(|&z| z != self.node && self.ring_watch(z).is_some()),
        );
        if self.now >= self.next_sweep {
            needed.extend(self.successors());
        }
        needed.sort();
        needed.dedup();
        needed
    }

    /// The two successors of this node among the nodes that are starting,
    /// live or sealing, in cyclic index order.
    fn successors(&self) -> Vec<u16> {
        let n = self.table.nodes;
        (1..n)
            .map(|step| (self.node + step) % n)
            .filter(|&z| self.table.writes(z) && self.real(z))
            .take(2)
            .collect()
    }

    /// Whether a node holds cores (a phantom node of `Model::boot` holds
    /// none, beats nothing and is never observed).
    pub(crate) fn real(&self, node: u16) -> bool {
        !self
            .table
            .cores_of(node, self.table.generation(node))
            .is_empty()
    }

    /// Observations: the gate opens one per needed node and core, and per
    /// ring that holds a message, at most 8, each expiring after 4 ×
    /// `STALE_NS`. A sealed node does not beat: it is never observed.
    fn observe_needed(&mut self) -> bool {
        let now = self.now;
        let me = self.node as usize;
        let life = OBSERVATION_LIFE * self.table.stale_ns;
        let before = self.table.beacons[me].observations.len();
        self.table.beacons[me]
            .observations
            .retain(|o| now < o.at + life);
        let mut changed = before != self.table.beacons[me].observations.len();
        let swept = now >= self.next_sweep;
        for node in self.needed() {
            if !self.table.writes(node) || !self.real(node) {
                continue;
            }
            let g = self.table.generation(node);
            let mut wanted: Vec<Watched> = self
                .table
                .cores_of(node, g)
                .into_iter()
                .map(|core| Watched::Heartbeat {
                    core,
                    heartbeat: self.table.core_beacons[core].heartbeat.0,
                })
                .collect();
            wanted.extend(self.ring_watch(node));
            for watched in wanted {
                let observations = &mut self.table.beacons[me].observations;
                if observations
                    .iter()
                    .any(|o| o.node == node && o.watched == watched)
                {
                    continue;
                }
                let same_kind = |o: &Observation| {
                    o.node == node
                        && match (o.watched, watched) {
                            (
                                Watched::Heartbeat { core: a, .. },
                                Watched::Heartbeat { core: b, .. },
                            ) => a == b,
                            (Watched::Ring { user: a, .. }, Watched::Ring { user: b, .. }) => {
                                a == b
                            }
                            _ => false,
                        }
                };
                observations.retain(|o| !same_kind(o));
                if observations.len() < OBSERVATIONS_MAX {
                    observations.push(Observation {
                        node,
                        watched,
                        at: now,
                    });
                    changed = true;
                }
            }
        }
        if swept {
            self.next_sweep = now + self.table.sweep_ns;
            // A successor that is idle with no armed timer is sent an
            // interrupt and a ping, which becomes stuck evidence if it is
            // not consumed.
            for z in self.successors() {
                let g = self.table.generation(z);
                let idle = self
                    .table
                    .cores_of(z, g)
                    .iter()
                    .all(|&c| self.table.core_beacons[c].idle_until.0 == u64::MAX);
                if idle {
                    let _ = self.write_message(z, crate::rings::Body::Ping, None, None);
                    self.hw_out.push(HwAction::Interrupt { node: z });
                }
            }
            changed = true;
        }
        changed
    }

    /// Accusations, with evidence the gate checks: at least one other node
    /// live or sealing, except for evidence self.
    fn accuse_with_evidence(&mut self) -> bool {
        for node in (0..self.table.nodes).filter(|&z| z != self.node) {
            let state = self.table.member(node).state();
            let accusable = matches!(
                state,
                MemberState::Starting
                    | MemberState::Live
                    | MemberState::Sealing
                    | MemberState::Sealed
            );
            if !accusable {
                continue;
            }
            let Some(evidence) = self.evidence(node) else {
                continue;
            };
            let others = (0..self.table.nodes).any(|z| {
                z != node
                    && matches!(
                        self.table.member(z).state(),
                        MemberState::Live | MemberState::Sealing
                    )
            });
            if evidence == Evidence::SelfReported || others {
                self.accuse(node, evidence);
                return true;
            }
        }
        false
    }

    // The fence.

    /// One step of the fences this node drives, and the fences it takes
    /// over from a driver that is no longer starting, live or sealing.
    fn fence_step(&mut self) -> bool {
        let me = self.node;
        let mut any = false;
        for node in (0..self.table.nodes).filter(|&z| z != me) {
            let member = self.table.member(node);
            if !matches!(member.state(), MemberState::Fencing | MemberState::Unfenced) {
                self.fences.remove(&node);
                continue;
            }
            let driver = member.driver();
            if driver != me && (driver == DRIVER_NONE || !self.table.writes(driver)) {
                let redriven =
                    Member::new(member.state(), member.evidence(), me, member.generation())
                        .with_class(member.class());
                self.table.write_member(me, node, redriven);
                self.fences.insert(
                    node,
                    Fence {
                        generation: member.generation(),
                        last: 0,
                    },
                );
                any = true;
                continue;
            }
            if driver == me && self.fence_unit(node) {
                any = true;
            }
        }
        any
    }

    /// A round of the fence of `node`: for each core whose word names its
    /// generation, the park request raised, then the word marked, then the
    /// stop sent; the start requests aimed at it cancelled. The node is
    /// fenced when every core is confirmed; unfenced after `FENCE_NS`
    /// without every confirmation; an unfenced node is tried again every
    /// `SWEEP_NS`.
    fn fence_unit(&mut self, node: u16) -> bool {
        let Some(fence) = self.fences.get(&node).copied() else {
            return false;
        };
        let member = self.table.member(node);
        let g = fence.generation;
        if member.generation() != g {
            self.fences.remove(&node);
            return false;
        }
        let now = self.now;
        let cores = self.table.cores_of(node, g);
        let unconfirmed: Vec<usize> = cores
            .iter()
            .copied()
            .filter(|&c| {
                !(self.table.core_word(c).is_some_and(|w| w.is_marked()) && self.park_confirmed(c))
            })
            .collect();
        let started = self.table.fences[node as usize].begun(g);
        if unconfirmed.is_empty() && started.is_some() {
            for core in 0..self.table.start_requests.len() {
                if crate::table::Table::started_node(self.table.start_requests[core])
                    == Some((node, g))
                {
                    self.table.start_requests[core] = 0;
                }
            }
            let fenced = Member::new(MemberState::Fenced, member.evidence(), member.driver(), g)
                .with_class(member.class());
            self.table.write_member(self.node, node, fenced);
            self.table.fences[node as usize].confirmed = Some(now);
            self.fences.remove(&node);
            return true;
        }
        let period = match member.state() {
            MemberState::Unfenced => self.table.sweep_ns,
            _ => crate::nodes::FENCE_RETRY_NS,
        };
        if started.is_some() && now < fence.last + period {
            if member.state() == MemberState::Fencing
                && started.is_some_and(|s| now >= s + FENCE_NS)
            {
                let unfenced =
                    Member::new(MemberState::Unfenced, member.evidence(), member.driver(), g)
                        .with_class(member.class());
                self.table.write_member(self.node, node, unfenced);
                return true;
            }
            return false;
        }
        // The first fence action of a generation writes the record anew:
        // that generation, its instant, every other word 0 (gap 6).
        if started.is_none() {
            self.table.fences[node as usize] = FenceRecord {
                generation: g,
                started: Some(now),
                ..FenceRecord::default()
            };
        }
        for core in unconfirmed {
            let word = self.table.core_word(core).expect("names the node");
            if !word.is_marked() {
                // The park request first, then the mark.
                self.table.park_requests[core] += 1;
                self.table.fence_trace.push(FenceStep::Raised(core));
                let before = self.table.core_words[core];
                self.table.write_core(core, before, word.marked());
                self.table.fence_trace.push(FenceStep::Marked(core));
            }
            self.hw_out.push(HwAction::Stop { core });
            self.table.fence_trace.push(FenceStep::Stopped(core));
            self.table.fences[node as usize].stopped(core, now);
        }
        for core in 0..self.table.start_requests.len() {
            if crate::table::Table::started_node(self.table.start_requests[core]) == Some((node, g))
            {
                self.table.start_requests[core] = 0;
            }
        }
        if let Some(f) = self.fences.get_mut(&node) {
            f.last = now;
        }
        true
    }

    // Reclaim.

    /// R0, at once when this node sees `node` fencing: it refuses every new
    /// message from it (the check), ends with `gone` every wait of its
    /// threads for an answer of it, queues again the unit requests sent to
    /// it, drops the requests of it that it serves, drops the pending
    /// signals and releases naming its grants, and frees every credit in
    /// use toward it.
    pub(crate) fn r0(&mut self, node: u16, generation: u32) {
        let reclaim = self.reclaims.entry(node).or_default();
        if reclaim.generation == generation && reclaim.isolated {
            return;
        }
        *reclaim = crate::nodes::Reclaim {
            generation,
            isolated: true,
            ..Default::default()
        };
        let waiting: Vec<ObjId> = self
            .threads
            .iter()
            .filter(|(_, t)| matches!(t.wait, Wait::Node { peer, .. } if peer == node))
            .map(|(&id, _)| id)
            .collect();
        for thread in waiting {
            self.end_bounded(thread);
            self.wake(thread, Err(Error::Gone));
        }
        self.unit_r0(node);
        self.peers[node as usize] = crate::rings::Peer::new();
        let me = self.node as usize;
        let drop: Vec<u16> = self.table.uses[me]
            .iter()
            .filter(|(_, u)| u.granter == node && u.generation == generation)
            .map(|(&i, _)| i)
            .collect();
        for index in drop {
            let entry = self.table.uses[me].get_mut(&index).expect("listed");
            entry.signal_pending = false;
            if entry.release_pending {
                self.table.uses[me].remove(&index);
            }
        }
        self.unit_send_queued();
        self.r1(node, generation);
    }

    /// R1, for each unit role this node holds: the context entries, the
    /// remapping entries and the lines of `node` are cleared, and the unit
    /// invalidated; R1 is done when the unit confirms (never on a unit that
    /// has failed).
    pub(crate) fn r1(&mut self, node: u16, generation: u32) {
        let held: Vec<usize> = (0..self.table.roles.len())
            .filter(|&r| {
                let word = self.table.roles[r];
                self.table.role_records[r].kind == RoleKind::Unit
                    && word.state() == RoleState::Held
                    && word.holder() == self.node
            })
            .collect();
        for role in held {
            if self
                .reclaims
                .get(&node)
                .is_some_and(|r| r.r1.contains_key(&role))
            {
                continue;
            }
            let unit = self.table.units.entry(role).or_default();
            unit.entries
                .retain(|_, &mut (n, g)| !(n == node && g == generation));
            unit.remap
                .retain(|_, &mut (n, g)| !(n == node && g == generation));
            unit.lines
                .retain(|_, &mut (n, g)| !(n == node && g == generation));
            let serial = self.submit(role, crate::rings::UnitOp::Invalidate { domain: None });
            if let Some(reclaim) = self.reclaims.get_mut(&node) {
                reclaim.r1.insert(role, Some(serial));
            }
        }
    }

    /// Whether R1 is done for `node` on every unit role this node holds.
    fn r1_done(&mut self, node: u16) -> bool {
        let generation = self.table.generation(node);
        self.r1(node, generation);
        let roles: Vec<(usize, Option<u64>)> = self
            .reclaims
            .get(&node)
            .map(|r| r.r1.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default();
        let mut done = true;
        for (role, serial) in roles {
            let word = self.table.roles[role];
            if word.state() != RoleState::Held || word.holder() != self.node {
                continue;
            }
            match serial {
                None => {}
                Some(serial) if !self.unit_failed(role) && self.confirmed(role, serial) => {
                    self.reclaims
                        .get_mut(&node)
                        .expect("a reclaim")
                        .r1
                        .insert(role, None);
                }
                Some(_) => done = false,
            }
        }
        done
    }

    /// R2 to R5 for one node, in steps of the reserve: at most `units`
    /// units. Returns whether one was done.
    fn reclaim_step(&mut self, node: u16, units: &mut usize) -> bool {
        let member = self.table.member(node);
        let state = member.state();
        let g = member.generation();
        if !matches!(
            state,
            MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
        ) {
            if state != MemberState::Dead
                || self.reclaims.get(&node).is_some_and(|r| r.generation != g)
            {
                self.reclaims.remove(&node);
            }
            return false;
        }
        self.r0(node, g);
        if state == MemberState::Fencing {
            return false;
        }
        let mut any = false;
        // R2: every translation this node holds to memory of the node:
        // its root remote capabilities of the node's grants, one removal
        // step each; done when no record under them remains unmapping.
        while *units > 0 {
            let root = self
                .remotes
                .iter()
                .find(|(_, r)| r.node == node)
                .map(|(&o, _)| self.objects[&o].root);
            let Some(root) = root else { break };
            live(&mut self.caps, root).dying = true;
            *units -= 1;
            any = true;
            if self.remove_root(root) != Ok(Output::Done) {
                return true;
            }
        }
        // R3: the grants made to its generation are pinned, and the records
        // of its use of them dropped.
        let me = self.node as usize;
        if !self.reclaims[&node].r3 {
            for entry in self.table.grants[me].values_mut() {
                if entry.grantee == node && entry.generation == g {
                    entry.state = crate::table::GrantState::Pinned;
                }
            }
            self.reclaims.get_mut(&node).expect("a reclaim").r3 = true;
            any = true;
        }
        // R4: the audit of the ownership words of this node's live blocks,
        // one block per unit, under the gate lock.
        if self.reclaims[&node].audit.is_none() {
            let blocks: Vec<ObjId> = self
                .untypeds
                .iter()
                .filter(|(_, u)| u.block == Some(crate::Block::Run))
                .map(|(&id, _)| id)
                .collect();
            self.reclaims.get_mut(&node).expect("a reclaim").audit = Some(blocks);
        }
        while *units > 0 {
            let next = self
                .reclaims
                .get_mut(&node)
                .expect("a reclaim")
                .audit
                .as_mut()
                .expect("begun")
                .pop();
            let Some(block) = next else { break };
            *units -= 1;
            any = true;
            if self
                .untypeds
                .get(&block)
                .is_some_and(|u| u.block == Some(crate::Block::Run))
                && !self.audit_block(block)
            {
                self.containment_violated();
                return true;
            }
        }
        // R5: its answer, a write to its own page; not before R1 is done on
        // every unit this node holds, and only for a fenced node (an
        // unfenced one keeps its memory frozen).
        if !self.reclaims[&node].acked && state == MemberState::Fenced && self.r1_done(node) {
            let beacon = &mut self.table.beacons[me];
            beacon.maps_of[node as usize] = false;
            beacon.acked[node as usize] = g;
            self.reclaims.get_mut(&node).expect("a reclaim").acked = true;
            any = true;
        }
        any
    }

    /// R4 for one block: every ownership word it covers still reads taken
    /// by this node with its generation.
    fn audit_block(&self, block: ObjId) -> bool {
        let untyped = &self.untypeds[&block];
        let shift = self.table.geometry.chunk_shift();
        let me = dodos_abi::Owner::Node(self.node);
        let generation = self.table.generation(self.node);
        (untyped.base >> shift..(untyped.base + untyped.bytes) >> shift).all(|c| {
            dodos_abi::Ownership::from_raw(self.table.owners[c as usize]).is_some_and(|w| {
                w.state() == dodos_abi::OwnState::Taken
                    && (w.owner() == me || matches!(w.owner(), dodos_abi::Owner::Reserve(_)))
                    && w.generation() == generation
            })
        })
    }

    /// A violation of containment: the machine stops with a stop record.
    pub(crate) fn containment_violated(&mut self) {
        let number = self.here().number as u16;
        let crash = std::sync::Arc::make_mut(&mut self.table.crash);
        crash.stop = Some((
            crash.boot_id,
            number,
            dodos_abi::CrashReason::Containment as u32,
        ));
        self.hw_out.push(HwAction::StopMachine);
    }

    /// The barrier: whether `node` (fenced) may go from fenced to dead.
    pub(crate) fn barrier_holds(&self, node: u16) -> bool {
        let member = self.table.member(node);
        if member.state() != MemberState::Fenced {
            return false;
        }
        let g = member.generation();
        let views: Vec<dodos_abi::BarrierView> = (0..self.table.nodes)
            .filter(|&y| y != node)
            .map(|y| {
                let beacon = &self.table.beacons[y as usize];
                let acknowledged_by_all = (0..self.table.nodes)
                    .filter(|&z| z != y && self.table.writes(z))
                    .all(|z| {
                        self.table.beacons[z as usize].acked[y as usize] == self.table.generation(y)
                    });
                dodos_abi::BarrierView {
                    member: self.table.member(y),
                    acked: beacon.acked[node as usize],
                    maps: beacon.maps_of[node as usize],
                    acknowledged_by_all,
                }
            })
            .collect();
        // Every unit role held, not handing, by a node that is live or
        // sealing, and that holder has acknowledged the node.
        let units_clear =
            self.table
                .role_records
                .iter()
                .zip(&self.table.roles)
                .all(|(record, word)| {
                    record.kind != RoleKind::Unit
                        || (word.state() == RoleState::Held
                            && word.holder() != node
                            && matches!(
                                self.table.member(word.holder()).state(),
                                MemberState::Live | MemberState::Sealing
                            )
                            && (!self.real(word.holder())
                                || self.table.beacons[word.holder() as usize].acked[node as usize]
                                    == g))
                });
        // No context entry of any IOMMU unit names the node's generation.
        let blocked = self.table.units.values().all(|unit| {
            unit.entries
                .values()
                .all(|&(n, generation)| n != node || generation != g)
        });
        blocked && dodos_abi::barrier(g, &views, units_clear)
    }

    /// R6, by a live node that sees the barrier hold: what the node's gate
    /// wrote in the living part is cleared, one unit per gate call, then
    /// dead is written. The walk waits while the barrier does not hold.
    fn clear_step(&mut self, node: u16, units: &mut usize) -> bool {
        if self.table.member(self.node).state() != MemberState::Live || !self.barrier_holds(node) {
            return false;
        }
        let g = self.table.generation(node);
        let mut cursor = self
            .reclaims
            .get(&node)
            .and_then(|r| r.clear)
            .unwrap_or_default();
        let mut any = false;
        let shift = self.table.geometry.chunk_shift();
        while *units > 0 {
            let before = any;
            *units -= 1;
            any = true;
            match cursor.phase {
                // Chunks taken by its generation, and its floor: map counts
                // 0, page classes data; a reserve's chunk of a role it holds
                // becomes free with the reserve's code.
                0 => {
                    if cursor.index >= self.table.owners.len() {
                        cursor = Clear { phase: 1, index: 0 };
                        continue;
                    }
                    let chunk = cursor.index as u64;
                    cursor.index += 1;
                    let at = chunk << shift;
                    let floor = self.table.floors[node as usize]
                        .iter()
                        .any(|&(s, e)| s <= at && at < e);
                    let raw = self.table.owners[chunk as usize];
                    let word = dodos_abi::Ownership::from_raw(raw);
                    let taken = word.is_some_and(|w| {
                        w.state() == dodos_abi::OwnState::Taken
                            && w.generation() == g
                            && (w.owner() == dodos_abi::Owner::Node(node)
                                || matches!(w.owner(), dodos_abi::Owner::Reserve(j) if self.table.holds_reserve(node, j)))
                    });
                    if taken || floor {
                        self.table.set_map_count(chunk, 0);
                        let pages = self.table.geometry.chunk() / self.table.geometry.page();
                        let first = at / self.table.geometry.page();
                        for page in first..first + pages {
                            self.table.set_class(page, None);
                        }
                    }
                    if let Some(w) = word.filter(|_| taken)
                        && let dodos_abi::Owner::Reserve(j) = w.owner()
                    {
                        let free = dodos_abi::Ownership::free(dodos_abi::Owner::Reserve(j));
                        self.table.write_owner_cleared(self.node, chunk, raw, free);
                    }
                }
                // Device entries taken by its generation: leaf count 0.
                1 => {
                    if cursor.index >= self.table.devices.len() {
                        cursor = Clear { phase: 2, index: 0 };
                        continue;
                    }
                    let index = cursor.index;
                    cursor.index += 1;
                    let owner = self.table.devices[index].owner;
                    if owner.state() == dodos_abi::OwnState::Taken
                        && owner.owner() == dodos_abi::Owner::Node(node)
                        && owner.generation() == g
                    {
                        std::sync::Arc::make_mut(&mut self.table.device_leaves)[index] = 0;
                    }
                }
                // Its grant entries: free, the serial grows.
                2 => {
                    let next = self.table.grants[node as usize].keys().next().copied();
                    match next {
                        Some(grant) => {
                            let entry = self.table.grants[node as usize]
                                .remove(&grant)
                                .expect("listed");
                            self.table.serials[node as usize]
                                .insert(grant, (entry.serial + 1) & 0x3FFF_FFFF);
                        }
                        None => cursor = Clear { phase: 3, index: 0 },
                    }
                }
                // Its use entries: free; then the line words its generation
                // took: free, unclaimed (Interrupts, D-135).
                3 => {
                    let next = self.table.uses[node as usize].keys().next().copied();
                    let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(node), g).raw();
                    let line = self.table.lines.iter().find(|&(_, &raw)| raw == taken);
                    match (next, line.map(|(&gsi, _)| gsi)) {
                        (Some(index), _) => {
                            self.table.uses[node as usize].remove(&index);
                        }
                        (None, Some(gsi)) => self.table.write_line(self.node, gsi, None),
                        (None, None) => cursor = Clear { phase: 4, index: 0 },
                    }
                }
                // Every core word that names its generation: free, keeping
                // the flag unusable, once its core is confirmed stopped and
                // `SETTLE_NS` have passed since the last stop sent to it.
                4 => {
                    let left = self.table.cores_of(node, g);
                    let Some(&core) = left.first() else {
                        cursor = Clear { phase: 5, index: 0 };
                        continue;
                    };
                    let record = &self.table.fences[node as usize];
                    let last = match record.generation == g {
                        true => record.last_stop(core),
                        false => 0,
                    };
                    if !self.park_confirmed(core) || self.now < last + SETTLE_NS {
                        self.reclaims.entry(node).or_default().clear = Some(cursor);
                        *units += 1;
                        return before;
                    }
                    let before = self.table.core_words[core];
                    let unusable = CoreWord::from_raw(before).is_some_and(|w| w.is_unusable());
                    self.table
                        .write_core(core, before, CoreWord::free(unusable));
                    self.table.start_requests[core] = 0;
                }
                // Then dead; the reserve roles it held become vacant and
                // the plain roles naming it are vacated.
                _ => {
                    let member = self.table.member(node);
                    let dead =
                        Member::new(MemberState::Dead, member.evidence(), member.driver(), g)
                            .with_class(member.class());
                    self.table.write_member(self.node, node, dead);
                    for role in 0..self.table.roles.len() {
                        let record = self.table.role_records[role];
                        let word = self.table.roles[role];
                        let names = word.state() != RoleState::Vacant
                            && (word.holder() == node || word.target() == node);
                        if names && record.kind != RoleKind::Unit {
                            let vacant = dodos_abi::RoleWord::vacant(word.epoch().wrapping_add(1));
                            self.table.write_role(self.node, role, vacant);
                        }
                    }
                    self.reclaims.remove(&node);
                    return true;
                }
            }
        }
        self.reclaims.entry(node).or_default().clear = Some(cursor);
        any
    }

    /// The frames pinned by R3 for a generation now over: the entries are
    /// freed and their anchors removed, one unit each.
    fn unpin(&mut self, units: &mut usize) -> bool {
        let me = self.node as usize;
        let mut any = false;
        while *units > 0 {
            let pinned = self.table.grants[me]
                .iter()
                .find(|(_, e)| e.state == crate::table::GrantState::Pinned && self.grantee_over(e))
                .map(|(&grant, _)| grant);
            let Some(grant) = pinned else { break };
            *units -= 1;
            any = true;
            let anchor = self.anchor(grant);
            self.free_grant(grant);
            if let Some(anchor) = anchor
                && self.caps[&anchor].children.is_empty()
            {
                self.remove_leaf(anchor);
            }
        }
        any
    }

    // The seal, the resume, the starts.

    /// The steps of a seal of this node (`node_stop` mode seal): it hands
    /// each unit role it holds over, answers its kernel rings, completes the
    /// requests of its cores and waits for its unit requests already sent;
    /// then saves and seals its block, writes the instant of its seal,
    /// raises the park request of each other core and parks them, and the
    /// last core, once each is confirmed, raises its own park request and
    /// writes sealed in one gate call. A sealing node whose panic is set
    /// does not write sealed; one that finds no other node live returns to
    /// live.
    fn seal_step(&mut self, units: &mut usize) -> bool {
        let me = self.node;
        let member = self.table.member(me);
        if member.state() != MemberState::Sealing {
            self.seal = None;
            return false;
        }
        if !self.others_live(me, true) {
            let live = Member::new(
                MemberState::Live,
                member.evidence(),
                member.driver(),
                member.generation(),
            )
            .with_class(member.class());
            self.table.write_member(me, me, live);
            self.seal = None;
            return true;
        }
        if self.table.beacons[me as usize].panic.is_some() {
            return false;
        }
        match self.seal.unwrap_or(SealStep::Quiescing) {
            SealStep::Quiescing => {
                self.seal = Some(SealStep::Quiescing);
                let handed = self.hand_all_units();
                let quiet = !self.cores.iter().any(|c| c.request.is_some())
                    && !self.unit_requests.iter().flatten().any(|e| {
                        matches!(
                            e.stage,
                            crate::units::Stage::Sent { .. } | crate::units::Stage::Local { .. }
                        )
                    })
                    && !(0..self.table.nodes).any(|z| z != me && self.has_kernel_message(z));
                *units = units.saturating_sub(1);
                if handed && quiet {
                    self.table.beacons[me as usize].sealed_at = Some(self.now);
                    for position in 1..self.width() {
                        let core = self.cores[position].number as usize;
                        self.table.park_requests[core] += 1;
                        self.hw_out.push(HwAction::Park { core });
                        self.cores[position].silent = true;
                    }
                    self.seal = Some(SealStep::Parking);
                    return true;
                }
                handed && quiet
            }
            SealStep::Parking => {
                let others =
                    (1..self.width()).all(|p| self.park_confirmed(self.cores[p].number as usize));
                let named = self
                    .table
                    .role_records
                    .iter()
                    .zip(&self.table.roles)
                    .any(|(r, w)| {
                        r.kind == RoleKind::Unit
                            && w.state() != RoleState::Vacant
                            && (w.holder() == me
                                || (w.state() == RoleState::Handing && w.target() == me))
                    });
                if !others || named {
                    return false;
                }
                let core = self.cores[0].number as usize;
                self.table.park_requests[core] += 1;
                let sealed = Member::new(
                    MemberState::Sealed,
                    member.evidence(),
                    member.driver(),
                    member.generation(),
                )
                .with_class(member.class());
                self.table.write_member(me, me, sealed);
                self.sealed_view = self.table.members.clone();
                self.hw_out.push(HwAction::Park { core });
                self.cores[0].silent = true;
                self.seal = None;
                true
            }
        }
    }

    /// The steps of a resume, before any thread runs: R0, R2 and R3 for
    /// every node whose generation ended while this node was sealed, R4 and
    /// R5 for those still fenced or unfenced; then the rings that waited
    /// are consumed under the check, and the node becomes live.
    fn resume_step(&mut self, units: &mut usize) -> bool {
        if self.ready || !self.resuming {
            return false;
        }
        let me = self.node;
        let member = self.table.member(me);
        if member.state() != MemberState::Starting {
            return false;
        }
        let mut any = false;
        for node in (0..self.table.nodes).filter(|&z| z != me) {
            let state = self.table.member(node).state();
            if matches!(
                state,
                MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
            ) {
                any |= self.reclaim_step(node, units);
                if state != MemberState::Fencing
                    && !self.reclaims.get(&node).is_some_and(|r| r.acked)
                {
                    return any;
                }
            }
            // A generation that was not over when this node sealed and is
            // over now (dead, or started again: the member word names
            // another generation) gets R0: its waits end with `gone`, its
            // credits are freed; then its remote capabilities go, with no
            // release, and the grants made to it are freed at once (R2,
            // R3; Grants, "Generations").
            if let Some(old) = self.ended_while_sealed(node) {
                self.r0(node, old);
                any |= self.drop_ended(node, old, units);
            }
        }
        if self.cores.iter().all(|c| !c.silent) {
            self.ready = true;
            self.resuming = false;
            self.sealed_view.clear();
            let live = Member::new(
                MemberState::Live,
                member.evidence(),
                member.driver(),
                member.generation(),
            )
            .with_class(member.class());
            self.table.write_member(me, me, live);
            any = true;
        }
        any
    }

    /// The generation of `node` that this node saw when it sealed, if it
    /// was not over then and is over now.
    pub(crate) fn ended_while_sealed(&self, node: u16) -> Option<u32> {
        let old = *self.sealed_view.get(node as usize)?;
        let now = self.table.member(node);
        let was = old.state() != MemberState::Dead && old.generation() != 0;
        let over = now.state() == MemberState::Dead || now.generation() != old.generation();
        (was && over).then_some(old.generation())
    }

    /// The remote capabilities of a generation of `node` that is over are
    /// removed, with no release; the grants made to that generation, live,
    /// revoking or pinned, are freed (R2, R3).
    fn drop_ended(&mut self, node: u16, generation: u32, units: &mut usize) -> bool {
        let me = self.node as usize;
        for entry in self.table.grants[me].values_mut() {
            if entry.grantee == node && entry.generation == generation {
                entry.state = crate::table::GrantState::Pinned;
            }
        }
        let mut any = false;
        while *units > 0 {
            let me = self.node as usize;
            let stale = self.remotes.iter().find(|(_, r)| {
                let used = self.table.uses[me].get(&r.use_entry);
                r.node == node
                    && used.is_none_or(|u| {
                        u.generation != self.table.generation(node) || !self.table.writes(node)
                    })
            });
            let Some(root) = stale.map(|(&o, _)| self.objects[&o].root) else {
                break;
            };
            *units -= 1;
            any = true;
            live(&mut self.caps, root).dying = true;
            if self.remove_root(root) != Ok(Output::Done) {
                return true;
            }
        }
        any | self.unpin(units)
    }

    /// The starts this node drives: each core's start request written, and
    /// its wake sent, then sent again every millisecond until the park
    /// stub confirms it (the stub's `started` in its core beacon equals the
    /// request), for at most `START_NS`.
    fn start_step(&mut self) -> bool {
        let now = self.now;
        let mut any = false;
        let cores: Vec<usize> = self.starts.keys().copied().collect();
        for core in cores {
            let start = self.starts[&core];
            let member = self.table.member(start.node);
            let current =
                member.state() == MemberState::Starting && member.generation() == start.generation;
            let confirmed = self.table.core_beacons[core].started == start.request;
            if !current || confirmed || now >= start.since + START_NS {
                self.starts.remove(&core);
                any = true;
                continue;
            }
            if start.last != 0 && now < start.last + START_RETRY_NS {
                continue;
            }
            self.table.start_requests[core] = start.request;
            self.hw_out.push(HwAction::Start { core });
            self.starts.get_mut(&core).expect("listed").last = now.max(1);
            any = true;
        }
        any
    }

    // The recovery reserve.

    /// The instant of the next step of the recovery reserve: now while it
    /// has work that can progress (and its last step at this instant did),
    /// otherwise the next instant at which time makes some work possible.
    pub(crate) fn reserve_due(&self) -> Option<u64> {
        if !self.table.member(self.node).writes()
            && !self
                .table
                .member(self.node)
                .state()
                .eq(&MemberState::Starting)
        {
            return None;
        }
        if self.stalled != Some(self.now) && self.reserve_work() {
            return Some(self.now);
        }
        let now = self.now;
        let mut instants: Vec<u64> = Vec::new();
        if self.others_exist() {
            instants.push(self.next_sweep);
        }
        for fence in self.fences.values() {
            instants.push(fence.last + FENCE_RETRY_NS);
        }
        for start in self.starts.values() {
            instants.push(start.last + START_RETRY_NS);
        }
        instants.extend(self.unit_deadline());
        let me = self.node as usize;
        for o in &self.table.beacons[me].observations {
            instants.push(o.at + self.table.stale_ns.min(self.table.stuck_ns));
        }
        for node in 0..self.table.nodes {
            let record = &self.table.fences[node as usize];
            if let Some(confirmed) = record.confirmed {
                instants.push(confirmed + SETTLE_NS);
            }
            if let Some(started) = record.started {
                instants.push(started + FENCE_NS + SETTLE_NS);
                instants.push(started + FENCE_NS);
            }
            for &last in record.last_stop.values() {
                instants.push(last + SETTLE_NS);
            }
            if let Some(sealed) = self.table.beacons[node as usize].sealed_at {
                instants.push(sealed + self.table.seal_ns);
            }
        }
        for hand in self.handing.values() {
            instants.push(hand.started + dodos_abi::IOMMU_WAIT_NS);
        }
        // The reserve is a context of period 10 ms: its next step comes at
        // the latest one period after its last one, an instant that does
        // not move with the clock.
        let period = (self.steps_at.0 + RECOVERY_PERIOD_NS).max(now);
        instants
            .into_iter()
            .filter(|&at| at > now)
            .min()
            .map(|at| at.min(period))
    }

    /// Whether another node slot exists that runs (the sweep has someone to
    /// observe).
    fn others_exist(&self) -> bool {
        (0..self.table.nodes).any(|z| z != self.node && self.real(z))
    }

    /// Whether some work of the reserve could progress now.
    fn reserve_work(&self) -> bool {
        let words_changed = self.seen != (self.table.members.clone(), self.table.roles.clone());
        let recovering = (0..self.table.nodes).any(|z| {
            z != self.node
                && matches!(
                    self.table.member(z).state(),
                    MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
                )
        });
        let pinned = self.table.grants[self.node as usize]
            .values()
            .any(|e| e.state == crate::table::GrantState::Pinned && self.grantee_over(e));
        let pending = self.table.uses[self.node as usize]
            .values()
            .any(|u| u.release_pending || u.signal_pending);
        let sweep = self.now >= self.next_sweep && self.others_exist();
        words_changed
            || sweep
            || recovering
            || pinned
            || pending
            || self.rings_waiting()
            || self.unit_work()
            || !self.handing.is_empty()
            || !self.rebuilds.is_empty()
            || self.roles_work()
            || self.seal.is_some()
            || self.table.member(self.node).state() == MemberState::Sealing
            || !self.ready
            || !self.starts.is_empty()
    }

    /// One step of the recovery reserve, on the node's first core: at most
    /// 32 units of work, in this order: membership, the seal or the resume,
    /// the starts it drives, the fences, the reclaims and R6, the roles,
    /// the units, the kernel rings, the pending releases and signals, the
    /// observations and accusations.
    pub(crate) fn reserve_step(&mut self) {
        let mut units = REMOVAL_STEP;
        let seen = self.seen.clone();
        let actions = self.hw_out.len();
        let consumed = self.table.beacons[self.node as usize].consumed.clone();
        let mut progressed = false;
        self.observe_watched();
        let me = self.node;
        let writes = self.table.member(me).writes();
        progressed |= self.seal_step(&mut units);
        progressed |= self.resume_step(&mut units);
        if writes {
            progressed |= self.start_step();
            progressed |= self.fence_step();
            for node in (0..self.table.nodes).filter(|&z| z != me) {
                if units == 0 {
                    break;
                }
                progressed |= self.reclaim_step(node, &mut units);
                progressed |= self.clear_step(node, &mut units);
            }
            progressed |= self.unpin(&mut units);
            // The kernel rings before the roles: a `Handed` that came is
            // applied before the target reads the handing word (both take
            // step 5, Roles).
            progressed |= self.unit_step();
            progressed |= self.serve_kernel_rings(&mut units);
            progressed |= self.roles_step(&mut units);
            self.send_pending();
            progressed |= self.observe_needed();
            progressed |= self.accuse_with_evidence();
        }
        self.observe_watched();
        progressed |= seen != self.seen
            || actions != self.hw_out.len()
            || consumed != self.table.beacons[self.node as usize].consumed;
        // A step that makes progress is followed by another at the same
        // instant; the work of a node is bounded, so the steps at one
        // instant are too (a model's fault otherwise).
        self.steps_now.0 = match self.steps_at.0 == self.now {
            true => self.steps_now.0 + 1,
            false => 1,
        };
        self.steps_at.0 = self.now;
        assert!(
            self.steps_now.0 < 100_000,
            "the reserve makes progress for ever"
        );
        self.stalled = match progressed {
            true => None,
            false => Some(self.now),
        };
    }

    /// The machine actions the last events asked for, taken by `System`.
    pub fn take_hw(&mut self) -> Vec<HwAction> {
        std::mem::take(&mut self.hw_out)
    }
}

/// The idle table of `node_idle`, judged entry by entry.
fn idle_table(words: &[u64], arch: crate::boot::Arch) -> Result<Vec<[u64; 4]>, Error> {
    if !words.len().is_multiple_of(IDLE_ENTRY_WORDS)
        || words.len() > IDLE_STATES_MAX * IDLE_ENTRY_WORDS
    {
        return Err(Error::Invalid);
    }
    let methods: &[u64] = match arch {
        crate::boot::Arch::X86_64 => &[IDLE_METHOD_HALT, IDLE_METHOD_MWAIT],
        crate::boot::Arch::Aarch64 => &[IDLE_METHOD_HALT, IDLE_METHOD_PSCI],
        crate::boot::Arch::Riscv64 => &[IDLE_METHOD_HALT, IDLE_METHOD_SBI],
    };
    let mut table = Vec::new();
    let mut last = None;
    for entry in words.chunks(IDLE_ENTRY_WORDS) {
        let [latency, residency, method, flags] = [entry[0], entry[1], entry[2], entry[3]];
        let bounded = latency < 1 << 63 && residency < 1 << 63;
        let increasing = last.is_none_or(|l| latency > l);
        let known = methods.contains(&(method & 0xFFFF_FFFF));
        if !bounded || !increasing || !known || flags & !IDLE_FLAG_LOSES_CONTEXT != 0 {
            return Err(Error::Invalid);
        }
        last = Some(latency);
        table.push([latency, residency, method, flags]);
    }
    Ok(table)
}
