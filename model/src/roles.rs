//! Roles (D-115, D-129, D-131): duties with one holder at a time, recorded
//! in the role words. A unit role moves by a planned hand-over (`role_hand`,
//! or a seal) or, after a death, by its successor's take-over, which
//! rebuilds the unit from the reports it pulls from the other nodes; a
//! reserve or plain role moves by one compare-and-swap.

use crate::rings::{Body, Completes, Item, REPORT_ITEMS};
use crate::units::Rebuild;
use crate::{Model, ObjId, Outcome, Output, Wait, locked};
use dodos_abi::{
    Error, FENCE_NS, IOMMU_WAIT_NS, MemberState, OwnState, Owner, Ownership, REPORT_END, Rights,
    RoleKind, RoleState, RoleWord, SETTLE_NS, SlotAddr,
};

/// A planned hand-over of a unit role this node holds, steps 1 to 4: the
/// target, the instant of step 1, and the thread of `role_hand` that waits
/// for the drain (none for a seal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Handover {
    pub(crate) target: u16,
    pub(crate) started: u64,
    pub(crate) caller: Option<ObjId>,
}

impl Model {
    /// The role object of a call, its index.
    fn role_of(&self, thread: ObjId, role: SlotAddr) -> Result<usize, Error> {
        let object = self.object(thread, role, dodos_abi::ObjectKind::Role, Rights::MANAGE)?;
        Ok(self.roles[&object])
    }

    /// Whether a word of reserve `j` is taken by a node that is not dead (a
    /// word with the reserve's code was taken by the node the role word
    /// names, with the generation the word holds).
    fn reserve_taken(&self, role: usize) -> bool {
        let j = self.table.role_records[role].object;
        let holder = self.table.roles[role].holder();
        let shift = self.table.geometry.chunk_shift();
        self.table.reserves[j as usize].iter().any(|&(s, e)| {
            (s >> shift..e >> shift).any(|c| {
                Ownership::from_raw(self.table.owners[c as usize]).is_some_and(|w| {
                    w.state() == OwnState::Taken
                        && w.owner() == Owner::Reserve(j)
                        && (holder as usize) < self.table.members.len()
                        && self.table.member(holder).state() != MemberState::Dead
                        && self.table.generation(holder) == w.generation()
                })
            })
        })
    }

    /// `role_claim`.
    pub(crate) fn role_claim(&mut self, thread: ObjId, role: SlotAddr) -> Outcome {
        let role = self.role_of(thread, role)?;
        let record = self.table.role_records[role];
        if record.kind == RoleKind::Unit {
            return Err(Error::Invalid);
        }
        let word = self.table.roles[role];
        if word.state() == RoleState::Held && word.holder() == self.node {
            return Ok(Output::Epoch(word.epoch()));
        }
        if word.state() != RoleState::Vacant {
            let names = [word.holder(), word.target()];
            let blocking = names
                .iter()
                .filter(|&&n| (n as usize) < self.table.members.len())
                .any(|&n| {
                    let state = self.table.member(n).state();
                    let alive = matches!(
                        state,
                        MemberState::Starting
                            | MemberState::Live
                            | MemberState::Sealing
                            | MemberState::Sealed
                    );
                    let fenced = matches!(
                        state,
                        MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
                    );
                    alive || (record.kind == RoleKind::Reserve && fenced)
                });
            if blocking {
                return Err(Error::Busy);
            }
        }
        if record.kind == RoleKind::Reserve && self.reserve_taken(role) {
            return Err(Error::Busy);
        }
        let epoch = word.epoch().wrapping_add(1);
        self.table
            .write_role(self.node, role, RoleWord::held(self.node, epoch));
        Ok(Output::Epoch(epoch & dodos_abi::ROLE_EPOCH_MASK))
    }

    /// `role_hand`.
    pub(crate) fn role_hand(&mut self, thread: ObjId, role: SlotAddr, node: SlotAddr) -> Outcome {
        let role = self.role_of(thread, role)?;
        let target = match node.is_none() {
            true => None,
            false => {
                let object =
                    self.object(thread, node, dodos_abi::ObjectKind::Node, Rights::NONE)?;
                Some(self.nodes[&object])
            }
        };
        let record = self.table.role_records[role];
        if record.kind == RoleKind::Unit && target.is_none() {
            return Err(Error::Invalid);
        }
        let word = self.table.roles[role];
        if word.state() != RoleState::Held || word.holder() != self.node {
            return Err(Error::Busy);
        }
        if target == Some(self.node) {
            return Ok(Output::Epoch(word.epoch()));
        }
        if let Some(target) = target
            && self.table.member(target).state() != MemberState::Live
        {
            return Err(Error::Busy);
        }
        match record.kind {
            RoleKind::Unit => {
                let target = target.expect("judged");
                let best = dodos_abi::best_class(&self.table.members).unwrap_or(0);
                if self.table.member(target).class() < best || self.vmx(target) {
                    return Err(Error::Busy);
                }
                // CONTRACT-GAP 13: a phantom node of a model of one node
                // runs nothing and could take no role; as for `node_stop`, it
                // does not count as a live node, and a hand-over to it is
                // `busy`.
                if !self.real(target) {
                    return Err(Error::Busy);
                }
                if self.unit_failed(role) {
                    return Err(Error::Busy);
                }
                // Steps 1 to 4: the caller waits in *waiting for a unit*
                // while the queue drains, checked at later steps.
                self.handing.insert(
                    role,
                    Handover {
                        target,
                        started: self.now,
                        caller: Some(thread),
                    },
                );
                locked(&mut self.threads, &self.locks, thread).wait = Wait::Hand(role);
                self.wait_mark(crate::locks::WaitKind::Unit);
                self.observe();
                Ok(Output::Waits)
            }
            _ => {
                if record.kind == RoleKind::Reserve && self.reserve_taken(role) {
                    return Err(Error::Busy);
                }
                let epoch = word.epoch().wrapping_add(1);
                let next = match target {
                    Some(target) => RoleWord::held(target, epoch),
                    None => RoleWord::vacant(epoch),
                };
                self.table.write_role(self.node, role, next);
                Ok(Output::Epoch(epoch & dodos_abi::ROLE_EPOCH_MASK))
            }
        }
    }

    /// Whether a core of a node is in VMX root operation (no core of a
    /// node that holds a unit role enters it).
    fn vmx(&self, node: u16) -> bool {
        let g = self.table.generation(node);
        self.table
            .cores_of(node, g)
            .iter()
            .any(|&c| self.table.core_beacons[c].vmx)
    }

    /// A seal hands every unit role this node holds to its successor;
    /// returns whether none is held or handing to it any more.
    pub(crate) fn hand_all_units(&mut self) -> bool {
        let me = self.node;
        let mut done = true;
        for role in 0..self.table.roles.len() {
            if self.table.role_records[role].kind != RoleKind::Unit {
                continue;
            }
            let word = self.table.roles[role];
            let mine = word.holder() == me && word.state() != RoleState::Vacant;
            let target = word.state() == RoleState::Handing && word.target() == me;
            if !mine && !target {
                continue;
            }
            done = false;
            if word.state() == RoleState::Held && !self.handing.contains_key(&role) {
                let Some(successor) = dodos_abi::successor(&self.table.members, me as usize) else {
                    continue;
                };
                self.handing.insert(
                    role,
                    Handover {
                        target: successor as u16,
                        started: self.now,
                        caller: None,
                    },
                );
            }
        }
        done
    }

    /// One step of the roles: the hand-overs this node runs (steps 2 to 4),
    /// step 5 of a handing word naming this node as target, the take-overs
    /// it is the successor for, and the rebuilds it runs.
    pub(crate) fn roles_step(&mut self, units: &mut usize) -> bool {
        let mut any = false;
        let roles: Vec<usize> = self.handing.keys().copied().collect();
        for role in roles {
            any |= self.hand_step(role);
        }
        for role in 0..self.table.roles.len() {
            if self.table.role_records[role].kind != RoleKind::Unit {
                continue;
            }
            any |= self.take_step(role);
        }
        let rebuilt: Vec<usize> = self.rebuilds.keys().copied().collect();
        for role in rebuilt {
            if *units == 0 {
                break;
            }
            *units -= 1;
            any |= self.rebuild_step(role);
        }
        any
    }

    /// Whether the roles have work that could progress now.
    pub(crate) fn roles_work(&self) -> bool {
        (0..self.table.roles.len()).any(|role| {
            let record = self.table.role_records[role];
            let word = self.table.roles[role];
            record.kind == RoleKind::Unit
                && ((word.state() == RoleState::Handing && word.target() == self.node)
                    || self.take_over_due(role).is_some())
        })
    }

    /// Steps 2 to 4 of a planned hand-over: the unit's queue drained (at
    /// most `IOMMU_WAIT_NS`: a unit that does not drain has failed), its
    /// registers unmapped, then handing written and `Handed` sent; if the
    /// target is no longer live, this node maps the unit again and stays
    /// the holder.
    fn hand_step(&mut self, role: usize) -> bool {
        let hand = self.handing[&role];
        let finish = |model: &mut Self, outcome: Outcome| {
            model.handing.remove(&role);
            if let Some(thread) = hand.caller
                && model
                    .threads
                    .get(&thread)
                    .is_some_and(|t| t.wait == Wait::Hand(role))
            {
                model.end_bounded(thread);
                model.wake(thread, outcome);
            }
        };
        let word = self.table.roles[role];
        if word.state() != RoleState::Held || word.holder() != self.node {
            finish(self, Err(Error::Busy));
            return true;
        }
        let begun =
            self.unit_service.values().any(|s| s.role == role)
                || self.unit_requests.iter().flatten().any(|e| {
                    e.role == role && matches!(e.stage, crate::units::Stage::Local { .. })
                });
        let drained = !begun && {
            let serial = self.table.units.get(&role).map_or(0, |u| u.next);
            self.confirmed(role, serial)
        };
        if !drained {
            if self.now >= hand.started + IOMMU_WAIT_NS {
                self.fail_unit(role);
                finish(self, Err(Error::Busy));
                return true;
            }
            return false;
        }
        // Step 3: unmap the unit's registers through the gate.
        if let Some(unit) = self.table.units.get_mut(&role) {
            unit.mapped.remove(&self.node);
        }
        // Step 4: the gate refuses if the target is no longer live; then
        // this node maps the unit again and stays the holder.
        if self.table.member(hand.target).state() != MemberState::Live {
            if let Some(unit) = self.table.units.get_mut(&role) {
                unit.mapped.insert(self.node);
            }
            finish(self, Err(Error::Busy));
            return true;
        }
        let epoch = word.epoch().wrapping_add(1);
        let handing = word.keeping_failed(RoleWord::handing(self.node, hand.target, epoch));
        self.table.write_role(self.node, role, handing);
        let body = Body::Handed {
            role: role as u16,
            epoch: handing.epoch(),
        };
        let _ = self.write_message(hand.target, body, None, None);
        finish(self, Ok(Output::Epoch(handing.epoch())));
        true
    }

    /// A `Handed` received: step 5, as at every read of a handing word that
    /// names this node.
    pub(crate) fn handed(&mut self, role: usize) {
        self.take_step(role);
    }

    /// The instant from which this node may take over a unit role, if it is
    /// the successor: for a held word, the holder fenced or dead and
    /// `SETTLE_NS` since its stop was confirmed, or unfenced and the sum of
    /// `FENCE_NS` and `SETTLE_NS` since the first fence action; for a
    /// handing word, the
    /// same test applied to the target, never to the node that hands, and
    /// the successor computed from the target.
    fn take_over_due(&self, role: usize) -> Option<u64> {
        let word = self.table.roles[role];
        let judged = match word.state() {
            RoleState::Held => word.holder(),
            RoleState::Handing => word.target(),
            RoleState::Vacant => return None,
        };
        if judged == self.node {
            return None;
        }
        let member = self.table.member(judged);
        let record = &self.table.fences[judged as usize];
        let at = match member.state() {
            MemberState::Fenced | MemberState::Dead => record.confirmed.unwrap_or(0) + SETTLE_NS,
            MemberState::Unfenced => record.started? + FENCE_NS + SETTLE_NS,
            _ => return None,
        };
        let successor = dodos_abi::successor(&self.table.members, judged as usize)?;
        (successor == self.node as usize).then_some(at)
    }

    /// Step 5 of a hand-over to this node, or a take-over after a death.
    fn take_step(&mut self, role: usize) -> bool {
        let word = self.table.roles[role];
        let me = self.node;
        if word.state() == RoleState::Handing && word.target() == me {
            // Step 5: map the unit, check the queue, write held (e + 2).
            if let Some(unit) = self.table.units.get_mut(&role) {
                unit.mapped.insert(me);
            }
            let held = word.keeping_failed(RoleWord::held(me, word.epoch().wrapping_add(1)));
            self.table.write_role(me, role, held);
            self.after_take(role);
            return true;
        }
        let Some(at) = self.take_over_due(role) else {
            return false;
        };
        if self.now < at {
            return false;
        }
        // Take-over: held (S, e + 1), the unit mapped, then rebuilt from the
        // reports of the nodes, without reading the old holder's memory.
        let held = word.keeping_failed(RoleWord::held(me, word.epoch().wrapping_add(1)));
        self.table.write_role(me, role, held);
        if let Some(unit) = self.table.units.get_mut(&role) {
            unit.mapped.clear();
            unit.mapped.insert(me);
        }
        self.after_take(role);
        self.rebuilds.insert(role, Rebuild::default());
        true
    }

    /// R1 at every change of holder: the node that takes a unit role runs
    /// R1 on it for every node that is fencing, fenced, unfenced or dead
    /// before it serves any other request, and writes `acked` back to 0
    /// for each such node it had acknowledged, until that R1 is done.
    fn after_take(&mut self, role: usize) {
        let me = self.node as usize;
        let mine = self.node;
        for node in (0..self.table.nodes).filter(|&z| z != mine) {
            let member = self.table.member(node);
            if !matches!(
                member.state(),
                MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
            ) {
                continue;
            }
            let g = member.generation();
            if self.table.beacons[me].acked[node as usize] == g {
                self.table.beacons[me].acked[node as usize] = 0;
            }
            let reclaim = self.reclaims.entry(node).or_default();
            if reclaim.generation != g {
                *reclaim = crate::nodes::Reclaim {
                    generation: g,
                    ..Default::default()
                };
            }
            reclaim.acked = false;
            reclaim.r1.remove(&role);
            self.r1(node, g);
        }
    }

    /// One step of a rebuild: the drain, then the reports pulled node by
    /// node in index order (at most one `ReportAsk` in flight to each
    /// node), then the comparison, the invalidation of the whole unit, and
    /// the requests kept during the rebuild.
    fn rebuild_step(&mut self, role: usize) -> bool {
        let Some(mut rebuild) = self.rebuilds.get(&role).cloned() else {
            return false;
        };
        let word = self.table.roles[role];
        if word.state() != RoleState::Held || word.holder() != self.node {
            self.rebuilds.remove(&role);
            return true;
        }
        if let Some((serial, _)) = rebuild.closing {
            if !self.confirmed(role, serial) {
                return false;
            }
            self.rebuilds.remove(&role);
            self.serve_kept(role);
            return true;
        }
        if rebuild.asked {
            // A node accused during the rebuild is skipped.
            if self.table.writes(rebuild.node) {
                return false;
            }
            rebuild.asked = false;
            rebuild.node += 1;
            rebuild.key = 0;
        }
        // The next node to ask: starting, live or sealing; a sealed node is
        // not asked (its entries are kept by the memory's owner); this node
        // reports its own items with no message.
        while rebuild.node < self.table.nodes {
            let z = rebuild.node;
            if z == self.node {
                let items = self.items(role);
                rebuild.items.extend(items.into_iter().map(|i| (z, i)));
                rebuild.node += 1;
                continue;
            }
            if !self.table.writes(z) {
                rebuild.node += 1;
                continue;
            }
            let busy = self
                .rebuilds
                .iter()
                .any(|(&r, b)| r != role && b.asked && b.node == z);
            if busy {
                self.rebuilds.insert(role, rebuild);
                return false;
            }
            let body = Body::ReportAsk {
                role: role as u16,
                epoch: word.epoch(),
                key: rebuild.key,
            };
            let asked = self.write_message(z, body, Some(Completes::Report { role }), None);
            rebuild.asked = asked.is_ok();
            self.rebuilds.insert(role, rebuild);
            return true;
        }
        // Every report is in: keep the entries that match, rewrite those
        // that differ, keep without a report the entries of a sealed node,
        // clear every other entry no report claims; then invalidate the
        // whole unit.
        let reported: Vec<(u16, Item)> = rebuild.items.clone();
        let generation = |model: &Self, z: u16| model.table.generation(z);
        let sealed: Vec<u16> = (0..self.table.nodes)
            .filter(|&z| self.table.member(z).state() == MemberState::Sealed)
            .collect();
        let entries: Vec<(u16, (u16, u32))> = self
            .table
            .units
            .get(&role)
            .map(|u| u.entries.iter().map(|(&d, &e)| (d, e)).collect())
            .unwrap_or_default();
        let unit = self.table.units.entry(role).or_default();
        for (device, (node, _)) in entries {
            let claimed = reported
                .iter()
                .any(|&(z, i)| z == node && i == Item::Device(device));
            if !claimed && !sealed.contains(&node) {
                unit.entries.remove(&device);
            }
        }
        let lines: Vec<(u32, (u16, u32))> = unit.lines.iter().map(|(&l, &e)| (l, e)).collect();
        for (line, (node, _)) in lines {
            let claimed = reported
                .iter()
                .any(|&(z, i)| z == node && i == Item::Line(line));
            if !claimed && !sealed.contains(&node) {
                unit.lines.remove(&line);
            }
        }
        for &(z, item) in &reported {
            if let Item::Device(device) = item {
                let g = generation(self, z);
                self.table
                    .units
                    .entry(role)
                    .or_default()
                    .entries
                    .insert(device, (z, g));
            }
        }
        let serial = self.submit(role, crate::rings::UnitOp::Invalidate);
        rebuild.closing = Some((serial, self.now));
        self.rebuilds.insert(role, rebuild);
        true
    }

    /// The requests kept during a rebuild are served, first.
    fn serve_kept(&mut self, role: usize) {
        let kept: Vec<((u16, u8), crate::units::Served)> = self
            .unit_service
            .iter()
            .filter(|(_, s)| s.role == role && s.submitted.is_none())
            .map(|(&k, &v)| (k, v))
            .collect();
        for ((peer, credit), served) in kept {
            let requester = (peer, self.table.generation(peer));
            let serial = self.submit_for(role, served.op, requester);
            if let Some(entry) = self.unit_service.get_mut(&(peer, credit)) {
                entry.submitted = Some((serial, self.now));
            }
        }
        self.unit_send_queued();
    }

    /// This node's items on a unit, in the fixed order of a report: its
    /// devices attached through the unit, from its own list, by identifier;
    /// then the lines it claimed, by global system interrupt.
    fn items(&self, role: usize) -> Vec<Item> {
        let record = self.table.role_records[role];
        let mut items: Vec<Item> = match record.governs {
            dodos_abi::Governs::IommuUnit => self
                .iommus
                .values()
                .filter(|u| u.unit == usize::from(record.object))
                .flat_map(|u| u.attached.keys().map(|&d| Item::Device(d)))
                .collect(),
            dodos_abi::Governs::IoApic => self
                .irqs
                .values()
                .filter(|irq| {
                    irq.source.is_line() && self.ioapic_role(irq.source.id()) == Some(role)
                })
                .map(|irq| Item::Line(irq.source.id()))
                .collect(),
            _ => Vec::new(),
        };
        items.sort();
        items
    }

    /// A `ReportAsk`: answered at once with one `Report`, its items from
    /// the key asked, as many as fit, and the key of the next item or end.
    /// The asked node keeps nothing.
    pub(crate) fn report_asked(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        role: u16,
        _epoch: u32,
        key: u64,
    ) {
        let items: Vec<Item> = self
            .items(role as usize)
            .into_iter()
            .filter(|i| i.key() >= key)
            .collect();
        let (sent, rest) = items.split_at(items.len().min(REPORT_ITEMS));
        let next = rest.first().map_or(REPORT_END, |i| i.key());
        let body = Body::Report {
            next,
            items: sent.to_vec(),
        };
        let _ = self.write_message(from, body, None, credit);
    }

    /// A `Report` received by the new holder: its items kept, and the next
    /// ask from the next key, or the next node at the end.
    pub(crate) fn report_received(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        next: u64,
        items: Vec<Item>,
    ) {
        let Some((index, _)) = credit else { return };
        let Some(taken) = self.take_credit(from, index) else {
            return;
        };
        let Completes::Report { role } = taken.completes else {
            return;
        };
        let Some(rebuild) = self.rebuilds.get_mut(&role) else {
            return;
        };
        rebuild.items.extend(items.into_iter().map(|i| (from, i)));
        rebuild.asked = false;
        match next {
            REPORT_END => {
                rebuild.node = from + 1;
                rebuild.key = 0;
            }
            key => rebuild.key = key,
        }
    }
}
