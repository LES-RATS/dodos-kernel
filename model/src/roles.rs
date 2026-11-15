//! Roles (8.4): `role_claim` and `role_hand`, the planned hand-over of a
//! unit role, its take-over after a death with a rebuild from the reports
//! of the live nodes, the requests to the holder of an IOMMU unit's role,
//! and the seal of a node.
//!
//! The context entries of each IOMMU unit are in `Table::units`, as the
//! hardware holds them; the record of what each node attached is in that
//! node's `iommus`, which is its report. Only IOMMU units have modelled
//! state; the other unit roles move by the same rules with nothing to
//! rebuild.

use crate::recovery::HwAction;
use crate::remote::{Body, Request};
use crate::{Model, ObjId, Outcome, Output, SlotAddr, Wait, live};
use dodos_abi::{
    Error, FENCE_NS, Governs, MemberState, ObjectKind, Ownership, Rights, RoleKind, RoleState,
    RoleWord, SETTLE_NS, successor,
};

/// What a node asks of the holder of a unit's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOp {
    /// The context entry of a device names the requesting node.
    Attach(u16),
    /// The context entry of a device is cleared.
    Detach(u16),
    /// The invalidation after an unmap in an iospace that serves devices.
    Invalidate,
}

impl Model {
    /// The role a `role` capability names, with `manage`.
    fn role_index(&self, thread: ObjId, role: SlotAddr) -> Result<usize, Error> {
        let object = self.object(thread, role, ObjectKind::Role, Rights::MANAGE)?;
        Ok(self.roles[&object])
    }

    /// Whether a word of reserve `j` is taken by a node that is not dead.
    fn reserve_taken(&self, j: u16) -> bool {
        let shift = self.table.geometry.chunk_shift();
        self.table.reserves[j as usize].iter().any(|&(start, end)| {
            (start >> shift..end >> shift).any(|chunk| {
                let word = Ownership::from_raw(self.table.owners[chunk as usize]);
                word.is_some_and(|w| match (w.state(), w.owner()) {
                    (dodos_abi::OwnState::Taken, dodos_abi::Owner::Node(n)) => !self.table.dead(n),
                    (dodos_abi::OwnState::Taken, dodos_abi::Owner::Reserve(_)) => true,
                    _ => false,
                })
            })
        })
    }

    /// `role_claim`.
    pub(crate) fn role_claim(&mut self, thread: ObjId, role: SlotAddr) -> Outcome {
        let index = self.role_index(thread, role)?;
        let record = self.table.role_records[index];
        if record.kind == RoleKind::Unit {
            return Err(Error::Invalid);
        }
        let word = self.table.roles[index];
        let me = self.node;
        if word.state() == RoleState::Held && word.holder() == me {
            return Ok(Output::Epoch(word.epoch()));
        }
        if word.state() != RoleState::Vacant {
            let blocks = |n: u16| match n {
                dodos_abi::ROLE_NOBODY => false,
                n => match self.table.member(n).state() {
                    MemberState::Starting
                    | MemberState::Live
                    | MemberState::Sealing
                    | MemberState::Sealed => true,
                    MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced => {
                        record.kind == RoleKind::Reserve
                    }
                    MemberState::Dead => false,
                },
            };
            let target = if word.state() == RoleState::Handing {
                word.target()
            } else {
                dodos_abi::ROLE_NOBODY
            };
            if blocks(word.holder()) || blocks(target) {
                return Err(Error::Busy);
            }
        }
        if record.kind == RoleKind::Reserve && self.reserve_taken(record.object) {
            return Err(Error::Busy);
        }
        let epoch = word.epoch().wrapping_add(1) & 0x3FFF_FFFF;
        self.table.write_role(me, index, RoleWord::held(me, epoch));
        self.observe_watched();
        Ok(Output::Epoch(epoch))
    }

    /// `role_hand`: the core names the target node and needs no right; 0
    /// vacates (not for a unit role).
    pub(crate) fn role_hand(&mut self, thread: ObjId, role: SlotAddr, core: SlotAddr) -> Outcome {
        let index = self.role_index(thread, role)?;
        let record = self.table.role_records[index];
        let target = match core.is_none() {
            true => None,
            false => Some(self.core_target(thread, core)?),
        };
        if record.kind == RoleKind::Unit && target.is_none() {
            return Err(Error::Invalid);
        }
        let word = self.table.roles[index];
        let me = self.node;
        if word.state() != RoleState::Held || word.holder() != me {
            return Err(Error::Busy);
        }
        if target == Some(me) {
            return Ok(Output::Epoch(word.epoch()));
        }
        if let Some(target) = target {
            if self.table.member(target).state() != MemberState::Live {
                return Err(Error::Busy);
            }
            if record.kind == RoleKind::Unit {
                let best = (0..self.table.nodes)
                    .filter(|&n| self.table.member(n).state() == MemberState::Live)
                    .map(|n| self.table.fence_of(n))
                    .max()
                    .unwrap_or(0);
                let core = self.table.placed[target as usize];
                if self.table.fence_of(target) < best || self.table.core_beacons[core].vmx {
                    return Err(Error::Busy);
                }
            }
        }
        if record.kind == RoleKind::Reserve && self.reserve_taken(record.object) {
            return Err(Error::Busy);
        }
        let epoch = word.epoch().wrapping_add(1) & 0x3FFF_FFFF;
        let new = match (record.kind, target) {
            (RoleKind::Unit, Some(target)) => {
                // Steps 1 to 4: no new request is accepted, the queue is
                // drained, the unit unmapped, and the word written handing,
                // the last act of this node on it.
                self.unmap_unit(index);
                self.send_packet(target, Body::Handed { role: index as u16 });
                RoleWord::handing(me, target, epoch)
            }
            (_, Some(target)) => RoleWord::held(target, epoch),
            (_, None) => RoleWord::vacant(epoch),
        };
        self.table.write_role(me, index, new);
        self.observe_watched();
        Ok(Output::Epoch(epoch))
    }

    /// The node a `core` capability names, with no right needed.
    fn core_target(&self, thread: ObjId, core: SlotAddr) -> Result<u16, Error> {
        let object = self.object(thread, core, ObjectKind::Core, Rights::NONE)?;
        Ok(self.cores[&object] as u16)
    }

    fn unmap_unit(&mut self, role: usize) {
        if let Some(unit) = self.table.units.get_mut(&role)
            && unit.mapped == Some(self.node)
        {
            unit.mapped = None;
        }
    }

    fn map_unit(&mut self, role: usize) {
        let me = self.node;
        self.table.units.entry(role).or_default().mapped = Some(me);
    }

    /// The unit role of an IOMMU unit.
    pub(crate) fn unit_role(&self, unit: usize) -> Option<usize> {
        self.table.role_records.iter().position(|r| {
            r.kind == RoleKind::Unit && r.governs == Governs::IommuUnit && r.object as usize == unit
        })
    }

    /// A change of a unit's context entries: made at once by its holder,
    /// otherwise asked of the holder, with the epoch this node read; the
    /// caller, if any, waits for the answer.
    pub(crate) fn unit_request(
        &mut self,
        thread: Option<ObjId>,
        unit: usize,
        op: UnitOp,
    ) -> Outcome {
        let Some(role) = self.unit_role(unit) else {
            return Ok(Output::Done);
        };
        let word = self.table.roles[role];
        if word.state() == RoleState::Held && word.holder() == self.node {
            self.apply_unit(role, self.node, op);
            return Ok(Output::Done);
        }
        let id = self.new_request();
        self.requests.insert(
            id,
            Request::Unit {
                thread,
                role,
                op,
                epoch: word.epoch(),
                sent_to: None,
            },
        );
        self.send_unit_request(id);
        match thread {
            Some(thread) => {
                live(&mut self.threads, thread).wait = Wait::Node(id);
                self.observe();
                Ok(Output::Waits)
            }
            None => Ok(Output::Done),
        }
    }

    /// Sends a request to the holder the role word names, if it is live;
    /// otherwise it waits for one.
    pub(crate) fn send_unit_request(&mut self, id: u64) {
        let Some(&Request::Unit { role, op, .. }) = self.requests.get(&id) else {
            return;
        };
        let word = self.table.roles[role];
        let holder = word.holder();
        let usable = word.state() == RoleState::Held
            && self.table.member(holder).state() == MemberState::Live;
        if !usable {
            return;
        }
        if holder == self.node {
            self.apply_unit(role, self.node, op);
            return self.answered_unit(id, true);
        }
        if let Some(Request::Unit { epoch, sent_to, .. }) = self.requests.get_mut(&id) {
            (*epoch, *sent_to) = (word.epoch(), Some(holder));
        }
        let epoch = word.epoch();
        self.send_packet(
            holder,
            Body::UnitRequest {
                id,
                role: role as u16,
                op,
                epoch,
            },
        );
    }

    /// The holder: a request with another epoch is refused; otherwise the
    /// entries change.
    pub(crate) fn serve_unit(&mut self, from: u16, id: u64, role: usize, op: UnitOp, epoch: u32) {
        let word = self.table.roles[role];
        let mine =
            word.state() == RoleState::Held && word.holder() == self.node && word.epoch() == epoch;
        if mine {
            self.apply_unit(role, from, op);
        }
        self.send_packet(from, Body::UnitAnswer { id, done: mine });
    }

    fn apply_unit(&mut self, role: usize, node: u16, op: UnitOp) {
        let generation = self.table.generation(node);
        let unit = self.table.units.entry(role).or_default();
        match op {
            UnitOp::Attach(device) => {
                unit.entries.insert(device, (node, generation));
            }
            UnitOp::Detach(device) => {
                if unit.entries.get(&device).is_some_and(|&(n, _)| n == node) {
                    unit.entries.remove(&device);
                }
            }
            UnitOp::Invalidate => {}
        }
    }

    /// The answer of a holder: done, the thread waiting goes on; refused,
    /// the request goes to the holder the role word names now.
    pub(crate) fn answered_unit(&mut self, id: u64, done: bool) {
        if !done {
            return self.send_unit_request(id);
        }
        let Some(Request::Unit { thread, .. }) = self.requests.remove(&id) else {
            return;
        };
        if let Some(thread) = thread.filter(|t| {
            self.threads
                .get(t)
                .is_some_and(|t| t.wait == Wait::Node(id))
        }) {
            live(&mut self.threads, thread).wait = Wait::None;
            live(&mut self.threads, thread).answered = true;
            self.wake(thread, Ok(Output::Done));
        }
    }

    /// R1: the unit roles this node holds block the devices of `node`.
    pub(crate) fn block_devices(&mut self, node: u16) {
        let me = self.node;
        let held: Vec<usize> = (0..self.table.roles.len())
            .filter(|&r| {
                let word = self.table.roles[r];
                self.table.role_records[r].kind == RoleKind::Unit
                    && word.state() == RoleState::Held
                    && word.holder() == me
            })
            .collect();
        for role in held {
            if let Some(unit) = self.table.units.get_mut(&role) {
                unit.entries.retain(|_, &mut (n, _)| n != node);
            }
        }
    }

    /// One unit of the roles: step 5 of a hand-over to this node, a
    /// take-over after the delay, or a request waiting for a live holder.
    pub(crate) fn roles_unit(&mut self) -> bool {
        let me = self.node;
        if self.table.member(me).state() != MemberState::Live {
            return false;
        }
        for role in 0..self.table.roles.len() {
            let record = self.table.role_records[role];
            if record.kind != RoleKind::Unit {
                continue;
            }
            let word = self.table.roles[role];
            // Step 5: the target maps the unit, checks its queue, and
            // writes held.
            if word.state() == RoleState::Handing && word.target() == me {
                self.map_unit(role);
                let epoch = word.epoch().wrapping_add(1) & 0x3FFF_FFFF;
                self.table.write_role(me, role, RoleWord::held(me, epoch));
                return true;
            }
            // Take-over: the node the role waits for is no longer live.
            // After step 4 the role waits for the target (step 5); if the
            // target dies, its successor takes over with a rebuild.
            let waited = match word.state() {
                RoleState::Handing => word.target(),
                _ => word.holder(),
            };
            let state = self.table.member(waited).state();
            if matches!(
                state,
                MemberState::Live | MemberState::Starting | MemberState::Sealing
            ) {
                continue;
            }
            let classes: Vec<u8> = (0..self.table.nodes)
                .map(|n| self.table.fence_of(n))
                .collect();
            if successor(&self.table.members, &classes, waited as usize) != Some(me as usize) {
                continue;
            }
            let fence = self.table.fences[waited as usize];
            let now = self.now;
            let due = match state {
                MemberState::Fenced | MemberState::Dead => {
                    fence.confirmed.is_none_or(|at| now >= at + SETTLE_NS)
                }
                MemberState::Unfenced => fence
                    .started
                    .is_some_and(|at| now >= at + FENCE_NS + SETTLE_NS),
                // A sealed holder of a unit role, or one still fencing: no
                // take-over without a stop.
                _ => false,
            };
            if !due {
                continue;
            }
            let epoch = word.epoch().wrapping_add(1) & 0x3FFF_FFFF;
            self.table.write_role(me, role, RoleWord::held(me, epoch));
            self.map_unit(role);
            self.rebuild(role);
            return true;
        }
        // Requests that wait for a live holder.
        let waiting: Vec<u64> = self
            .requests
            .iter()
            .filter_map(|(&id, r)| match r {
                Request::Unit { role, sent_to, .. } => {
                    let word = self.table.roles[*role];
                    let moved = word.state() == RoleState::Held
                        && Some(word.holder()) != *sent_to
                        && self.table.member(word.holder()).state() == MemberState::Live;
                    moved.then_some(id)
                }
                _ => None,
            })
            .collect();
        if let Some(&id) = waiting.first() {
            self.send_unit_request(id);
            return true;
        }
        false
    }

    /// The rebuild of an IOMMU unit by its new holder, from the reports of
    /// the live nodes: the entries they claim are kept or rewritten, the
    /// others cleared.
    fn rebuild(&mut self, role: usize) {
        let reports = std::mem::take(&mut self.table.reports);
        let unit_index = self.table.role_records[role].object as usize;
        let mut entries = std::collections::BTreeMap::new();
        for (node, claimed) in reports.iter().enumerate() {
            let node = node as u16;
            if !self.table.writes(node) {
                continue;
            }
            let generation = self.table.generation(node);
            for &(unit, device) in claimed {
                if unit == unit_index {
                    entries.insert(device, (node, generation));
                }
            }
        }
        self.table.reports = reports;
        let unit = self.table.units.entry(role).or_default();
        unit.entries = entries;
    }

    /// The seal of a node, one unit per step: each unit role it holds goes
    /// to its successor; the requests its proxies serve end with `gone`;
    /// then its block is saved and sealed, it writes sealed and parks.
    pub(crate) fn sealing_unit(&mut self) -> bool {
        let me = self.node;
        if self.table.member(me).state() != MemberState::Sealing {
            return false;
        }
        let classes: Vec<u8> = (0..self.table.nodes)
            .map(|n| self.table.fence_of(n))
            .collect();
        for role in 0..self.table.roles.len() {
            let word = self.table.roles[role];
            let unit = self.table.role_records[role].kind == RoleKind::Unit;
            if unit && word.state() == RoleState::Held && word.holder() == me {
                let Some(target) = successor(&self.table.members, &classes, me as usize) else {
                    return false;
                };
                let target = target as u16;
                self.unmap_unit(role);
                self.send_packet(target, Body::Handed { role: role as u16 });
                let epoch = word.epoch().wrapping_add(1) & 0x3FFF_FFFF;
                self.table
                    .write_role(me, role, RoleWord::handing(me, target, epoch));
                return true;
            }
        }
        let serving: Vec<ObjId> = self
            .proxies
            .iter()
            .filter(|(_, h)| h.is_some())
            .map(|(&p, _)| p)
            .collect();
        if let Some(&proxy) = serving.first() {
            if self.threads[&proxy].wait != Wait::None {
                self.unlink(proxy);
            }
            self.answer_proxy(proxy, Err(Error::Gone));
            return true;
        }
        // No role word may name it as the holder of a unit role, nor as the
        // target of a handing.
        let named = self
            .table
            .roles
            .iter()
            .zip(&self.table.role_records)
            .any(|(w, r)| {
                r.kind == RoleKind::Unit
                    && ((w.state() == RoleState::Held && w.holder() == me)
                        || (w.state() == RoleState::Handing && w.target() == me))
            });
        if named {
            return false;
        }
        self.table.beacons[me as usize].save_seal = true;
        let member = self.table.member(me);
        let sealed = dodos_abi::Member::new(
            MemberState::Sealed,
            member.evidence(),
            member.driver(),
            member.generation(),
        );
        self.table.write_member(me, me, sealed);
        self.hw_out.push(HwAction::ParkSelf { node: me });
        true
    }
}
