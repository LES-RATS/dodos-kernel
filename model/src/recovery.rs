//! Cores (8.4): the calls on `core` capabilities, and what each node does
//! in its recovery reserve: observe the nodes it needs, accuse with
//! evidence, drive the fence of the node it accused, reclaim (R0 to R5),
//! move a dead node across the barrier, and tell its watchers. The
//! hardware that the fence acts on is in `System`; a node asks it through
//! `hw_out`.
//!
//! Recovery work takes no time in the model: a step happens when `System`
//! runs the node's recovery at an instant, and the reserve's bandwidth is
//! only admitted.

use crate::remote::Request;
use crate::table::{FenceRecord, GrantState, Observation, Watched};
use crate::{Model, ObjId, Outcome, Output, SlotAddr, Wait, live};
use dodos_abi::{
    CORE_IMAGE_BOOT, CORE_IMAGE_REPLACEMENT, CORE_START_FRESH, CORE_START_RESUME,
    CORE_STATUS_FENCE_SHIFT, CORE_STATUS_PARKED, CORE_STATUS_UNUSABLE, CORE_STOP_KILL,
    CORE_STOP_RETIRE, CORE_STOP_SEAL, CORE_WATCH_SLOTS, DRIVER_NONE, Error, Evidence, FENCE_NS,
    IDLE_ENTRY_WORDS, IDLE_FLAG_LOSES_CONTEXT, IDLE_METHOD_HALT, IDLE_METHOD_MWAIT,
    IDLE_METHOD_PSCI, IDLE_METHOD_SBI, IDLE_STATES_MAX, Member, MemberState, NMI_GRACE_NS,
    OBSERVATION_LIFE, OBSERVATIONS_MAX, ObjectKind, Rights, RoleKind, RoleState, SETTLE_NS,
    kind_covers,
};

/// What a node asks of the hardware: the steps of a fence on a physical
/// core with the fence epoch, the start of a node, or the park of its own
/// core at the end of the event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HwAction {
    Nmi {
        core: usize,
        epoch: u64,
    },
    Init {
        core: usize,
    },
    Sipi {
        core: usize,
        epoch: u64,
    },
    /// ARM64 and RISC-V: the signal the dying kernel cannot mask, whose
    /// handler stops its core; class 2 stops it whatever it runs.
    Signal {
        core: usize,
        epoch: u64,
    },
    Start {
        node: u16,
        mode: u64,
    },
    ParkSelf {
        node: u16,
    },
}

/// The fence a node drives: its epoch, the instant of its first action and
/// of each step it took.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fence {
    pub(crate) generation: u32,
    pub(crate) epoch: u64,
    pub(crate) begun: u64,
    pub(crate) init: Option<u64>,
    /// INIT was sent while `vmx` was clear.
    pub(crate) init_clear: bool,
    pub(crate) last_try: u64,
}

/// How far this node is in the reclaim of a node's generation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Reclaim {
    pub(crate) generation: u32,
    /// R0 and R1 done.
    pub(crate) isolated: bool,
    /// R2 to R4 done; R5 written.
    pub(crate) cleared: bool,
    pub(crate) acked: bool,
}

impl Model {
    /// The node a `core` capability names, with `right`.
    fn core_node(&self, thread: ObjId, core: SlotAddr, right: Rights) -> Result<u16, Error> {
        let object = self.object(thread, core, ObjectKind::Core, right)?;
        Ok(self.cores[&object] as u16)
    }

    fn live_nodes(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.table.nodes).filter(|&n| self.table.member(n).state() == MemberState::Live)
    }

    /// Several nodes on one core (D-108): the event of this node runs on
    /// its core, whose kernel loads its block and keeps the blocks of the
    /// other nodes placed there saved, their seals valid.
    pub(crate) fn load_block(&mut self) {
        let me = self.node;
        let core = self.table.placed[me as usize];
        self.table.core_beacons[core].running = me;
        for z in 0..self.table.nodes {
            if self.table.placed[z as usize] == core && !self.table.dead(z) {
                self.table.beacons[z as usize].save_seal = z != me;
            }
        }
    }

    /// Whether `node` is not the node its core runs and its block is saved:
    /// it cannot execute, so its stop needs no stop of the core.
    pub(crate) fn parked_block(&self, node: u16) -> bool {
        let core = self.table.placed[node as usize];
        self.table.beacons[node as usize].save_seal && self.table.core_beacons[core].running != node
    }

    /// `core_start`.
    pub(crate) fn core_start(
        &mut self,
        thread: ObjId,
        core: SlotAddr,
        mode: u64,
        image: u64,
        module: u64,
    ) -> Outcome {
        let node = self.core_node(thread, core, Rights::MANAGE)?;
        let modules = self.machine.segments.len() as u64;
        let module_ok = match mode {
            CORE_START_FRESH => module < modules,
            CORE_START_RESUME => module == 0,
            _ => return Err(Error::Invalid),
        };
        if !matches!(image, CORE_IMAGE_BOOT | CORE_IMAGE_REPLACEMENT) || !module_ok {
            return Err(Error::Invalid);
        }
        let member = self.table.member(node);
        let core_of = self.table.placed[node as usize];
        match mode {
            CORE_START_FRESH => {
                if member.state() != MemberState::Dead {
                    return Err(Error::Busy);
                }
                let runs = (0..self.table.nodes).any(|z| {
                    z != node
                        && self.table.placed[z as usize] == core_of
                        && !matches!(
                            self.table.member(z).state(),
                            MemberState::Dead | MemberState::Sealed
                        )
                });
                if self.table.unusable[core_of] || runs {
                    return Err(Error::Busy);
                }
            }
            _ => {
                let parked =
                    self.table.core_beacons[core_of].running != node || self.parked_core(core_of);
                if member.state() != MemberState::Sealed || !parked {
                    return Err(Error::Busy);
                }
            }
        }
        if image == CORE_IMAGE_REPLACEMENT && !self.table.replacement {
            return Err(Error::Busy);
        }
        // The gate's condition for a fresh start: no role word names it.
        let generation = match mode {
            CORE_START_FRESH => {
                let named = self.table.roles.iter().any(|w| {
                    w.state() != RoleState::Vacant && (w.holder() == node || w.target() == node)
                });
                if named {
                    return Err(Error::Busy);
                }
                member.generation().wrapping_add(1)
            }
            _ => member.generation(),
        };
        let starting = Member::new(
            MemberState::Starting,
            Evidence::None,
            DRIVER_NONE,
            generation,
        );
        self.table.write_member(self.node, node, starting);
        self.hw_out.push(crate::recovery::HwAction::Start {
            node,
            mode: mode | image << 8,
        });
        self.observe_watched();
        Ok(Output::Done)
    }

    /// Whether a physical core is confirmed parked: its beacon says so, or
    /// it runs no node that writes.
    fn parked_core(&self, core: usize) -> bool {
        let running = self.table.core_beacons[core].running;
        running == crate::table::NO_NODE || !self.table.writes(running)
    }

    /// `core_stop`.
    pub(crate) fn core_stop(&mut self, thread: ObjId, core: SlotAddr, mode: u64) -> Outcome {
        let node = self.core_node(thread, core, Rights::MANAGE)?;
        if !matches!(mode, CORE_STOP_KILL | CORE_STOP_SEAL | CORE_STOP_RETIRE) {
            return Err(Error::Invalid);
        }
        let state = self.table.member(node).state();
        let refused = match mode {
            CORE_STOP_SEAL => state != MemberState::Live,
            _ => matches!(
                state,
                MemberState::Dead
                    | MemberState::Fencing
                    | MemberState::Fenced
                    | MemberState::Unfenced
            ),
        };
        if refused {
            return Err(Error::Busy);
        }
        // The last node: no other node would be live afterwards. The check
        // counts nodes: one placed on the stopped node's core stays live
        // unless that core must be stopped; a phantom node runs nothing and
        // does not count.
        let core_of = self.table.placed[node as usize];
        let stops_core = mode != CORE_STOP_SEAL && !self.parked_block(node);
        let stays = |n: u16| {
            n != node
                && (self.shared || n == self.node)
                && !(stops_core && self.table.placed[n as usize] == core_of)
        };
        if !self.live_nodes().any(stays) {
            return Err(Error::Busy);
        }
        match mode {
            CORE_STOP_SEAL => {
                let member = self.table.member(node);
                let sealing = Member::new(
                    MemberState::Sealing,
                    member.evidence(),
                    member.driver(),
                    member.generation(),
                );
                self.table.write_member(self.node, node, sealing);
            }
            _ => {
                let evidence = if mode == CORE_STOP_KILL {
                    Evidence::Kill
                } else {
                    Evidence::Retire
                };
                if mode == CORE_STOP_RETIRE {
                    let core_of = self.table.placed[node as usize];
                    self.table.unusable[core_of] = true;
                }
                self.accuse(node, evidence);
            }
        }
        // Stopping its own node: it parks at the end of the event, and
        // another node drives the fence.
        if node == self.node && mode != CORE_STOP_SEAL {
            self.fences.remove(&node);
            self.hw_out.push(HwAction::ParkSelf { node });
        }
        self.observe_watched();
        Ok(Output::Done)
    }

    /// `core_status`: no state error.
    pub(crate) fn core_status(&mut self, thread: ObjId, core: SlotAddr) -> Outcome {
        let node = self.core_node(thread, core, Rights::READ)?;
        let placed = self.table.placed[node as usize];
        let parked = placed != node as usize;
        let flags = (u64::from(parked) * CORE_STATUS_PARKED)
            | (u64::from(self.table.unusable[placed]) * CORE_STATUS_UNUSABLE)
            | u64::from(self.table.fence[placed]) << CORE_STATUS_FENCE_SHIFT;
        let counters = self.table.beacons[node as usize].counters.0;
        let perf = if self.machine.perf_request {
            self.machine.perf_caps
        } else {
            0
        };
        Ok(Output::Status([
            self.table.member(node).raw(),
            placed as u64 | flags,
            counters[0],
            counters[1],
            counters[2],
            counters[3],
            perf,
        ]))
    }

    /// `core_idle`: the table travels as the message words; on another node
    /// the call is carried there (rule 7).
    pub(crate) fn core_idle(&mut self, thread: ObjId, core: SlotAddr, words: &[u64]) -> Outcome {
        let node = self.core_node(thread, core, Rights::WRITE)?;
        let table = idle_table(words, self.machine.arch)?;
        if node == self.node {
            self.idle_table = table;
            return Ok(Output::Done);
        }
        self.carried(thread, node, crate::remote::Carried::Idle(table))
    }

    /// `core_perf`: on another node the call is carried there.
    pub(crate) fn core_perf(
        &mut self,
        thread: ObjId,
        core: SlotAddr,
        request: (u64, u64, u64),
    ) -> Outcome {
        let node = self.core_node(thread, core, Rights::WRITE)?;
        if !self.machine.perf_request {
            return Err(Error::Unsupported);
        }
        let (lowest, highest, preference) = request;
        if lowest > 0xFF || highest > 0xFF || preference > 0xFF || lowest > highest {
            return Err(Error::Invalid);
        }
        if node == self.node {
            self.perf = Some(request);
            return Ok(Output::Done);
        }
        self.carried(thread, node, crate::remote::Carried::Perf(request))
    }

    /// `core_watch`.
    pub(crate) fn core_watch(
        &mut self,
        thread: ObjId,
        core: SlotAddr,
        index: u64,
        notify: SlotAddr,
    ) -> Outcome {
        let node = self.core_node(thread, core, Rights::READ)?;
        if node != self.node {
            return Err(Error::Invalid);
        }
        let notify = match notify.is_none() {
            true => None,
            false => {
                let cap = self.cap(thread, notify, Rights::SEND)?;
                let held = self.is(cap, ObjectKind::Notify, Rights::SEND)?;
                let (object, badge) = (held.object, held.badge);
                if self.remotes.contains_key(&object) {
                    return Err(Error::Invalid);
                }
                Some((object, if badge == 0 { 1 } else { badge }))
            }
        };
        if index >= CORE_WATCH_SLOTS {
            return Err(Error::Invalid);
        }
        let slot = &mut self.watchers[index as usize];
        if notify.is_some() && slot.is_some() {
            return Err(Error::Busy);
        }
        *slot = notify;
        Ok(Output::Done)
    }

    /// `core_place`.
    pub(crate) fn core_place(&mut self, thread: ObjId, core: SlotAddr, physical: u64) -> Outcome {
        let node = self.core_node(thread, core, Rights::MANAGE)?;
        let cores = self.table.fence.len() as u64;
        if physical >= cores {
            return Err(Error::Invalid);
        }
        let physical = physical as usize;
        if !matches!(
            self.table.member(node).state(),
            MemberState::Live | MemberState::Sealed
        ) {
            return Err(Error::Busy);
        }
        if self.table.unusable[physical] {
            return Err(Error::Busy);
        }
        let hosted: Vec<u16> = (0..self.table.nodes)
            .filter(|&z| z != node && self.table.placed[z as usize] == physical)
            .collect();
        let not_live = hosted.iter().any(|&z| {
            !matches!(
                self.table.member(z).state(),
                MemberState::Live | MemberState::Sealed | MemberState::Dead
            )
        });
        if not_live {
            return Err(Error::Busy);
        }
        let host_kind = self.machine.kinds[usize::from(self.machine.core_list()[physical].kind)];
        if !kind_covers(&host_kind, &self.machine.own_kind(node)) {
            return Err(Error::Busy);
        }
        // Admission of the core: the sums of the nodes it would run.
        let sums: u64 = hosted
            .iter()
            .filter(|&&z| !self.table.dead(z))
            .map(|&z| self.table.admitted[z as usize])
            .sum::<u64>()
            + self.table.admitted[node as usize];
        if sums > dodos_abi::ADMISSION_WHOLE_CORE {
            return Err(Error::Budget);
        }
        self.table.placed[node as usize] = physical;
        Ok(Output::Done)
    }

    // Membership.

    /// The gate accuses a node with evidence, this node its driver.
    pub(crate) fn accuse(&mut self, node: u16, evidence: Evidence) {
        let member = self.table.member(node);
        let fencing = Member::new(
            MemberState::Fencing,
            evidence,
            self.node,
            member.generation(),
        );
        self.table.write_member(self.node, node, fencing);
        self.table.fences[node as usize] = FenceRecord::default();
        if node != self.node {
            self.begin_fence(node);
        }
    }

    /// Whether evidence holds against a node now, as the gate checks it.
    fn evidence(&self, node: u16) -> Option<Evidence> {
        let member = self.table.member(node);
        let beacon = &self.table.beacons[node as usize];
        if beacon.panic.is_some_and(|(_, g)| g == member.generation()) {
            return Some(Evidence::SelfReported);
        }
        let core = self.table.placed[node as usize];
        let beacon = self.table.core_beacons[core];
        let now = self.now;
        let observed = self.table.beacons[self.node as usize].observations.iter();
        for observation in observed.filter(|o| o.node == node) {
            match observation.watched {
                Watched::Heartbeat { core: c, heartbeat } if c == core => {
                    let stale = beacon.heartbeat.0 == heartbeat
                        && now >= observation.at + self.table.stale_ns
                        && now > beacon.idle_until.0.saturating_add(self.table.stale_ns);
                    // A node that does not beat within `START_NS` of its start.
                    let starting = member.state() == MemberState::Starting
                        && beacon.heartbeat.0 == heartbeat
                        && now >= observation.at + dodos_abi::START_NS;
                    if stale || starting {
                        return Some(Evidence::Stale);
                    }
                }
                Watched::Ring { consumed } => {
                    let ring = self.table.rings[self.node as usize][node as usize];
                    if ring.1 == consumed
                        && ring.0 > consumed
                        && now >= observation.at + self.table.stuck_ns
                    {
                        return Some(Evidence::Stuck);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The nodes this node needs: those its threads wait for, and its two
    /// successors among the nodes that are not dead (the sweep).
    fn needed(&self) -> Vec<u16> {
        let mut needed: Vec<u16> = self
            .requests
            .values()
            .filter_map(|request| match request {
                Request::Call { peer, .. } => Some(*peer),
                _ => None,
            })
            .collect();
        if self.now >= self.next_sweep {
            let n = self.table.nodes;
            let successors = (1..n)
                .map(|step| (self.node + step) % n)
                .filter(|&z| self.table.writes(z))
                .take(2);
            needed.extend(successors);
        }
        needed.sort();
        needed.dedup();
        needed
    }

    /// Observations: the gate opens one per needed node, at most 8, each
    /// expiring after 4 × `self.table.stale_ns`.
    fn observe_needed(&mut self) {
        let now = self.now;
        let me = self.node as usize;
        let life = OBSERVATION_LIFE * self.table.stale_ns;
        self.table.beacons[me]
            .observations
            .retain(|o| now < o.at + life);
        let swept = now >= self.next_sweep;
        // A sealed node does not beat: it is never observed (only kill,
        // retire and host accuse it).
        for node in self.needed() {
            if !self.table.writes(node) {
                continue;
            }
            let core = self.table.placed[node as usize];
            let heartbeat = self.table.core_beacons[core].heartbeat.0;
            let ring = self.table.rings[me][node as usize];
            let mut wanted = vec![Watched::Heartbeat { core, heartbeat }];
            if ring.0 > ring.1 {
                wanted.push(Watched::Ring { consumed: ring.1 });
            }
            for watched in wanted {
                let observations = &mut self.table.beacons[me].observations;
                let open = observations.iter().any(|o| {
                    o.node == node
                        && match (o.watched, watched) {
                            (
                                Watched::Heartbeat {
                                    heartbeat: h,
                                    core: c,
                                },
                                Watched::Heartbeat { .. },
                            ) => c == core && h == heartbeat,
                            (Watched::Ring { consumed: a }, Watched::Ring { consumed: b }) => {
                                a == b
                            }
                            _ => false,
                        }
                });
                if !open && observations.len() < OBSERVATIONS_MAX {
                    observations.retain(|o| {
                        o.node != node
                            || std::mem::discriminant(&o.watched)
                                != std::mem::discriminant(&watched)
                    });
                    observations.push(Observation {
                        node,
                        watched,
                        at: now,
                    });
                }
            }
        }
        if swept {
            self.next_sweep = now + self.table.sweep_ns;
            // A successor idle with no armed timer gets a ping, which becomes
            // stuck evidence if it is not consumed.
            let n = self.table.nodes;
            let successors: Vec<u16> = (1..n)
                .map(|step| (self.node + step) % n)
                .filter(|&z| self.table.writes(z))
                .take(2)
                .collect();
            for z in successors {
                let core = self.table.placed[z as usize];
                if self.table.core_beacons[core].idle_until.0 == u64::MAX && self.table.writes(z) {
                    self.send_packet(z, crate::remote::Body::Ping);
                }
            }
        }
    }

    /// One step of this node's recovery reserve: at most one unit of work.
    /// Says whether there was one.
    pub(crate) fn recovery_step(&mut self) -> bool {
        *self.last_event = None;
        let entry = self.begin();
        let done = self.recovery_unit();
        self.observe_watched();
        self.end(entry);
        done
    }

    fn recovery_unit(&mut self) -> bool {
        let me = self.node;
        if !self.table.writes(me) {
            return false;
        }
        if self.sealing_unit() {
            return true;
        }
        self.observe_needed();
        let n = self.table.nodes;
        // Accusations, with evidence the gate checks.
        for node in (0..n).filter(|&z| z != me) {
            let state = self.table.member(node).state();
            let accusable = matches!(
                state,
                MemberState::Starting | MemberState::Live | MemberState::Sealing
            );
            if !accusable {
                continue;
            }
            let Some(evidence) = self.evidence(node) else {
                continue;
            };
            let others = self.live_nodes().any(|z| z != node);
            if evidence == Evidence::SelfReported || others {
                self.accuse(node, evidence);
                return true;
            }
        }
        // Fences: drive those of this node, take those whose driver is gone.
        for node in (0..n).filter(|&z| z != me) {
            let member = self.table.member(node);
            if !matches!(member.state(), MemberState::Fencing | MemberState::Unfenced) {
                self.fences.remove(&node);
                continue;
            }
            let driver = member.driver();
            let driver_gone = driver == DRIVER_NONE || !self.table.writes(driver);
            // A fencing or unfenced node whose driver is gone is driven
            // again by this node.
            if driver != me && driver_gone {
                let redriven =
                    Member::new(member.state(), member.evidence(), me, member.generation());
                self.table.write_member(me, node, redriven);
                self.begin_fence(node);
                return true;
            }
            if self.fence_unit(node) {
                return true;
            }
        }
        // Reclaim, then the barrier.
        for node in (0..n).filter(|&z| z != me) {
            if self.reclaim_unit(node) {
                return true;
            }
        }
        for node in (0..n).filter(|&z| z != me) {
            if self.barrier_unit(node) {
                return true;
            }
        }
        if self.unpin() {
            return true;
        }
        self.roles_unit()
    }

    /// The first action of a fence: a new epoch, the first step.
    fn begin_fence(&mut self, node: u16) {
        self.table.fence_epoch += 1;
        let epoch = self.table.fence_epoch;
        let record = &mut self.table.fences[node as usize];
        if record.started.is_none() {
            record.started = Some(self.now);
        }
        let generation = self.table.generation(node);
        let fence = Fence {
            generation,
            epoch,
            begun: self.now,
            last_try: self.now,
            ..Fence::default()
        };
        self.fences.insert(node, fence);
        // A node whose block is saved on a core that runs another node is
        // stopped without a stop of that core (Several nodes on one core).
        if self.parked_block(node) {
            return;
        }
        let core = self.table.placed[node as usize];
        match self.machine.arch {
            crate::boot::Arch::X86_64 => self.hw_out.push(HwAction::Nmi { core, epoch }),
            crate::boot::Arch::Other => self.hw_out.push(HwAction::Signal { core, epoch }),
        }
    }

    /// One step of a fence this node drives: the next action due, the
    /// confirmation, or the time-out to unfenced.
    fn fence_unit(&mut self, node: u16) -> bool {
        let Some(mut fence) = self.fences.get(&node).copied() else {
            return false;
        };
        let member = self.table.member(node);
        if member.generation() != fence.generation {
            self.fences.remove(&node);
            return false;
        }
        let core = self.table.placed[node as usize];
        let beacon = self.table.core_beacons[core];
        let now = self.now;
        let x86 = self.machine.arch == crate::boot::Arch::X86_64;
        let bsp = self.table.booted[core];
        let confirmed = beacon.parked == fence.epoch
            || (x86
                && !bsp
                && fence
                    .init
                    .is_some_and(|at| fence.init_clear && now >= at + SETTLE_NS))
            || (!x86 && self.table.off[core]);
        if self.parked_block(node) {
            self.fenced(node, false);
            self.fences.remove(&node);
            return true;
        }
        if confirmed {
            self.fenced(node, true);
            self.fences.remove(&node);
            return true;
        }
        // INIT and SIPI after the grace of the NMI, on a core other than the
        // bootstrap core.
        if x86 && !bsp && fence.init.is_none() && now >= fence.begun + NMI_GRACE_NS {
            fence.init = Some(now);
            fence.init_clear = !beacon.vmx;
            self.hw_out.push(HwAction::Init { core });
            self.hw_out.push(HwAction::Sipi {
                core,
                epoch: fence.epoch,
            });
            self.hw_out.push(HwAction::Sipi {
                core,
                epoch: fence.epoch,
            });
            self.fences.insert(node, fence);
            return true;
        }
        let started = self.table.fences[node as usize]
            .started
            .unwrap_or(fence.begun);
        if member.state() == MemberState::Fencing
            && member.driver() == self.node
            && now >= started + FENCE_NS
        {
            let unfenced = Member::new(
                MemberState::Unfenced,
                member.evidence(),
                member.driver(),
                member.generation(),
            );
            self.table.write_member(self.node, node, unfenced);
            return true;
        }
        // An unfenced node: the driver tries again every `self.table.sweep_ns`.
        if member.state() == MemberState::Unfenced && now >= fence.last_try + self.table.sweep_ns {
            self.begin_fence(node);
            return true;
        }
        false
    }

    /// The stop of `node` is confirmed: it is fenced. If its core was
    /// stopped, every other node placed on that core that is fencing or
    /// unfenced becomes fenced with its own evidence and driver; one that is
    /// starting or live becomes sealed if its save seal is valid and no unit
    /// role names it, and is otherwise accused with evidence host and fenced
    /// too.
    fn fenced(&mut self, node: u16, stopped: bool) {
        let member = self.table.member(node);
        let fenced = Member::new(
            MemberState::Fenced,
            member.evidence(),
            member.driver(),
            member.generation(),
        );
        self.table.write_member(self.node, node, fenced);
        self.table.fences[node as usize].confirmed = Some(self.now);
        let core = self.table.placed[node as usize];
        let hosted: Vec<u16> = (0..self.table.nodes)
            .filter(|&z| stopped && z != node && self.table.placed[z as usize] == core)
            .collect();
        for z in hosted {
            let state = self.table.member(z).state();
            if matches!(state, MemberState::Dead | MemberState::Fenced) {
                continue;
            }
            let sealed = self.table.beacons[z as usize].save_seal;
            if state == MemberState::Sealed && sealed {
                continue;
            }
            let g = self.table.generation(z);
            let member = self.table.member(z);
            if matches!(state, MemberState::Fencing | MemberState::Unfenced) {
                // The same stop confirms its own fence.
                let fenced =
                    Member::new(MemberState::Fenced, member.evidence(), member.driver(), g);
                self.table.write_member(self.node, z, fenced);
                self.table.fences[z as usize].confirmed = Some(self.now);
                self.fences.remove(&z);
                continue;
            }
            let named = self
                .table
                .roles
                .iter()
                .zip(&self.table.role_records)
                .any(|(w, r)| {
                    r.kind == RoleKind::Unit
                        && w.state() != RoleState::Vacant
                        && (w.holder() == z || (w.state() == RoleState::Handing && w.target() == z))
                });
            if self.table.beacons[z as usize].save_seal && !named {
                let sealed = Member::new(MemberState::Sealed, Evidence::None, DRIVER_NONE, g);
                self.table.write_member(self.node, z, sealed);
            } else {
                let host = Member::new(MemberState::Fencing, Evidence::Host, self.node, g);
                self.table.write_member(self.node, z, host);
                let fenced = Member::new(MemberState::Fenced, Evidence::Host, self.node, g);
                self.table.write_member(self.node, z, fenced);
                self.table.fences[z as usize] = FenceRecord {
                    started: Some(self.now),
                    confirmed: Some(self.now),
                };
            }
        }
    }

    /// R0 to R5 for one node, one unit.
    fn reclaim_unit(&mut self, node: u16) -> bool {
        let member = self.table.member(node);
        let state = member.state();
        let g = member.generation();
        if !matches!(
            state,
            MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
        ) {
            if state != MemberState::Dead {
                self.reclaims.remove(&node);
            }
            return false;
        }
        let mut reclaim = self
            .reclaims
            .get(&node)
            .copied()
            .filter(|r| r.generation == g)
            .unwrap_or(Reclaim {
                generation: g,
                ..Reclaim::default()
            });
        if !reclaim.isolated {
            self.isolate(node);
            reclaim.isolated = true;
            self.reclaims.insert(node, reclaim);
            return true;
        }
        if state == MemberState::Fencing {
            return false;
        }
        if !reclaim.cleared {
            // R2: one root remote capability of the node, with its
            // descendants and mappings, one step.
            let root = self
                .remotes
                .iter()
                .find(|(_, r)| r.node == node)
                .map(|(&o, _)| self.objects[&o].root);
            if let Some(root) = root {
                live(&mut self.caps, root).dying = true;
                let _ = self.remove_root(root);
                return true;
            }
            // R3: the grants made to the node's generation are pinned.
            let me = self.node as usize;
            let pinned = self.table.grants[me]
                .iter()
                .find(|(_, e)| {
                    e.grantee == node && e.generation == g && e.state != GrantState::Pinned
                })
                .map(|(&grant, _)| grant);
            if let Some(grant) = pinned {
                live(&mut self.table.grants[me], grant).state = GrantState::Pinned;
                return true;
            }
            // R4: the audit is invariants 54 and 58 of this node, which
            // `check` judges after every event.
            reclaim.cleared = true;
            self.reclaims.insert(node, reclaim);
            return true;
        }
        if state == MemberState::Fenced && !reclaim.acked {
            // R5: its answer, a write to its own beacon.
            let beacon = &mut self.table.beacons[self.node as usize];
            beacon.maps_of[node as usize] = false;
            beacon.acked[node as usize] = g;
            reclaim.acked = true;
            self.reclaims.insert(node, reclaim);
            return true;
        }
        false
    }

    /// R0 and R1: nothing new from the node; the waits of this node's
    /// threads on it end with `gone`; its proxies here stop standing for
    /// its threads; the unit roles this node holds block its devices.
    fn isolate(&mut self, node: u16) {
        // Every wait on the node ends, whatever the request: the waiting
        // thread gets `gone`, and nothing the request would have changed has
        // changed here.
        let ended: Vec<u64> = self
            .requests
            .iter()
            .filter(|&(&id, request)| match request {
                Request::Call { peer, .. }
                | Request::Carry { peer, .. }
                | Request::Transfer { peer, .. }
                | Request::Move { peer, .. } => *peer == node,
                Request::Revoke { .. } => self.revoked_grantee(id) == Some(node),
                Request::Unit { .. } => false,
            })
            .map(|(&id, _)| id)
            .collect();
        for id in ended {
            // Each waiting thread, and whether its call returns `gone` (the
            // thread of a move only leaves its wait).
            let waiting: Vec<(ObjId, bool)> = match self.requests.remove(&id) {
                Some(Request::Call { thread, .. }) | Some(Request::Carry { thread, .. }) => {
                    vec![(thread, true)]
                }
                Some(Request::Revoke { thread, .. }) => {
                    thread.map(|t| (t, true)).into_iter().collect()
                }
                Some(Request::Transfer {
                    receiver, reply, ..
                }) => {
                    // The reply object armed for the node's caller is idle.
                    if let Some(reply) = reply
                        && self.replies.contains_key(&reply)
                    {
                        *live(&mut self.replies, reply) = crate::ReplyState::Idle;
                    }
                    receiver.map(|t| (t, true)).into_iter().collect()
                }
                // The thread and its context are as before the call.
                Some(Request::Move { caller, source, .. }) => {
                    let caller = caller.map(|t| (t, true));
                    caller
                        .into_iter()
                        .chain(source.map(|t| (t, false)))
                        .collect()
                }
                _ => Vec::new(),
            };
            for (thread, caller) in waiting {
                if self
                    .threads
                    .get(&thread)
                    .is_some_and(|t| t.wait == Wait::Node(id))
                {
                    live(&mut self.threads, thread).wait = Wait::None;
                    if caller {
                        live(&mut self.threads, thread).answered = true;
                        self.wake(thread, Err(Error::Gone));
                    }
                }
            }
        }
        self.held_requests.remove(&node);
        self.outstanding.remove(&node);
        let proxies: Vec<ObjId> = self
            .proxies
            .iter()
            .filter(|(_, held)| held.is_some_and(|(peer, _)| peer == node))
            .map(|(&p, _)| p)
            .collect();
        for proxy in proxies {
            if self.threads[&proxy].wait != Wait::None {
                self.unlink(proxy);
            }
            self.proxies.insert(proxy, None);
            let standing = live(&mut self.threads, proxy);
            (standing.stamp, standing.foreign, standing.outcome) = (None, None, None);
            (standing.number, standing.cancelled_up_to) = (0, 0);
        }
        self.block_devices(node);
    }

    /// The grantee of the grant a revocation waits for.
    fn revoked_grantee(&self, id: u64) -> Option<u16> {
        let Some(Request::Revoke { grant, .. }) = self.requests.get(&id) else {
            return None;
        };
        self.table.grants[self.node as usize]
            .get(grant)
            .map(|e| e.grantee)
    }

    /// The barrier, by any live node: the node goes from fenced to dead; in
    /// the same step its reserve roles become vacant and the plain roles
    /// still naming it are vacated.
    fn barrier_unit(&mut self, node: u16) -> bool {
        let member = self.table.member(node);
        if member.state() != MemberState::Fenced
            || self.table.member(self.node).state() != MemberState::Live
        {
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
        let units_clear =
            self.table
                .role_records
                .iter()
                .zip(&self.table.roles)
                .all(|(record, word)| {
                    // Every unit role held, not handing, by a node that is
                    // live or sealing.
                    record.kind != RoleKind::Unit
                        || (word.state() == RoleState::Held
                            && word.holder() != node
                            && matches!(
                                self.table.member(word.holder()).state(),
                                MemberState::Live | MemberState::Sealing
                            ))
                });
        // The reclaim barrier: no context entry of any IOMMU unit names the
        // node's generation; when the holder of a unit is itself fenced,
        // nobody blocks them (R1) until its successor takes the unit.
        let blocked = self.table.units.values().all(|unit| {
            unit.entries
                .values()
                .all(|&(n, generation)| n != node || generation != g)
        });
        if !blocked || !dodos_abi::barrier(g, &views, units_clear) {
            return false;
        }
        // R6: the chunks of the reserves whose role the node holds, taken
        // with its generation, become free with the reserve's code, so the
        // reserve can be claimed again (the model clears all at once).
        self.free_reserves_of(node, g);
        let dead = Member::new(MemberState::Dead, member.evidence(), member.driver(), g);
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
        true
    }

    /// R6 for the reserves a dying node holds: each word of such a reserve
    /// taken with its generation is written free with the reserve's code.
    fn free_reserves_of(&mut self, node: u16, generation: u32) {
        let shift = self.table.geometry.chunk_shift();
        let held: Vec<u16> = (0..self.table.roles.len())
            .filter(|&r| {
                let word = self.table.roles[r];
                self.table.role_records[r].kind == RoleKind::Reserve
                    && word.state() == RoleState::Held
                    && word.holder() == node
            })
            .map(|r| self.table.role_records[r].object)
            .collect();
        for j in held {
            let owner = dodos_abi::Owner::Reserve(j);
            let taken = dodos_abi::Ownership::taken(owner, generation).raw();
            let ranges = self.table.reserves[j as usize].clone();
            for chunk in ranges.iter().flat_map(|&(a, b)| a >> shift..b >> shift) {
                if self.table.owners[chunk as usize] == taken {
                    let free = dodos_abi::Ownership::free(owner);
                    assert!(self.table.write_owner(self.node, chunk, taken, free));
                }
            }
        }
    }

    /// Whether the generation a grant entry was made to is over: its
    /// grantee is dead, or names another generation (R3).
    pub(crate) fn grantee_over(&self, entry: &crate::table::GrantEntry) -> bool {
        self.table.dead(entry.grantee) || self.table.generation(entry.grantee) != entry.generation
    }

    /// The frames pinned by R3 for a generation now over, even if the
    /// grantee started again meanwhile: the entries are freed and their
    /// anchors removed, one unit.
    fn unpin(&mut self) -> bool {
        let me = self.node as usize;
        let pinned = self.table.grants[me]
            .iter()
            .find(|(_, e)| e.state == GrantState::Pinned && self.grantee_over(e))
            .map(|(&grant, _)| grant);
        let Some(grant) = pinned else {
            return false;
        };
        let anchor = self.anchor(grant);
        self.free_grant(grant);
        if let Some(anchor) = anchor
            && self.caps[&anchor].children.is_empty()
        {
            self.remove_leaf(anchor);
        }
        true
    }

    /// Watchers: after every change of a member word or a role word that
    /// this node processes, each bound notify is signalled.
    pub(crate) fn observe_watched(&mut self) {
        let seen = (self.table.members.clone(), self.table.roles.clone());
        if self.seen == seen {
            return;
        }
        self.seen = seen;
        let watchers: Vec<(ObjId, u64)> = self.watchers.iter().flatten().copied().collect();
        for (notify, bits) in watchers {
            if self.notifies.contains_key(&notify) && !self.dying(notify) {
                self.signal_bits_pub(notify, bits);
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

    /// Idle and slack time, published in the node beacon at each event.
    pub(crate) fn publish_counters(&mut self) {
        let me = self.node as usize;
        if let Some(beacon) = self.table.beacons.get_mut(me) {
            beacon.counters.0 = [self.idle_ns.0, self.slack_ns.0, 0, 0];
        }
        if let Some(admitted) = self.table.admitted.get_mut(me) {
            *admitted = self.admitted;
        }
    }

    /// The instant the core's timer is set to next, if any.
    pub(crate) fn next_instant(&self) -> Option<u64> {
        self.hardware()
    }

    /// The page numbers of the table pages of this node's spaces.
    pub(crate) fn table_pages(&self) -> Vec<u64> {
        let page = self.table.geometry.page();
        self.aspaces
            .values()
            .flat_map(|space| space.table_pages().map(|at| at / page))
            .collect()
    }

    /// The end of a node's start-up, as an event of that node: it writes
    /// live; a resumed node whose sealed block the image cannot read
    /// accuses itself instead and parks.
    pub(crate) fn started(&mut self, readable: bool) {
        *self.last_event = None;
        let entry = self.begin();
        let me = self.node;
        let member = self.table.member(me);
        if readable {
            // A node that was sealed while a granting node's generation ended
            // drops its remote capabilities of that generation first (R2),
            // with no release.
            self.drop_stale_remotes();
            let live = Member::new(
                MemberState::Live,
                member.evidence(),
                member.driver(),
                member.generation(),
            );
            self.table.write_member(me, me, live);
        } else {
            self.table.beacons[me as usize].panic = Some((1, member.generation()));
            self.hw_out.push(HwAction::ParkSelf { node: me });
        }
        self.next_sweep = self.now + self.table.sweep_ns;
        self.observe_watched();
        self.end(entry);
    }

    /// Removes every remote capability whose use entry records a
    /// generation of its granting node that is over.
    fn drop_stale_remotes(&mut self) {
        loop {
            let stale = self.remotes.iter().find(|(_, r)| {
                let used = self.table.uses[self.node as usize].get(&r.use_entry);
                !self.table.writes(r.node)
                    || used.is_none_or(|u| u.generation != self.table.generation(r.node))
            });
            let Some(root) = stale.map(|(&o, _)| self.objects[&o].root) else {
                return;
            };
            live(&mut self.caps, root).dying = true;
            while self.remove_root(root) == Err(Error::Again) {}
        }
    }
}

/// The idle table of `core_idle`, judged entry by entry.
fn idle_table(words: &[u64], arch: crate::boot::Arch) -> Result<Vec<[u64; 4]>, Error> {
    if !words.len().is_multiple_of(IDLE_ENTRY_WORDS)
        || words.len() > IDLE_STATES_MAX * IDLE_ENTRY_WORDS
    {
        return Err(Error::Invalid);
    }
    let methods: &[u64] = match arch {
        crate::boot::Arch::X86_64 => &[IDLE_METHOD_HALT, IDLE_METHOD_MWAIT],
        crate::boot::Arch::Other => &[IDLE_METHOD_HALT, IDLE_METHOD_PSCI, IDLE_METHOD_SBI],
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
