//! Roles (D-115, D-129, D-131): duties with one holder at a time, recorded
//! in the role words. A unit role moves by a planned hand-over (`role_hand`,
//! or a seal) or, after a death, by its successor's take-over, which
//! rebuilds the unit from the reports it pulls from the other nodes; a
//! reserve or plain role moves by one compare-and-swap.

use crate::rings::{Body, Completes, Item, REPORT_ITEMS};
use crate::units::Rebuild;
use crate::{Model, ObjId, Outcome, Output, Wait, live};
use dodos_abi::{
    Error, FENCE_NS, MemberState, OwnState, Owner, Ownership, REPORT_END, Rights, RoleKind,
    RoleState, RoleWord, SETTLE_NS, SlotAddr,
};

/// A planned hand-over of a unit role this node holds, steps 1 to 4: the
/// target, the instant of step 1, the thread of `role_hand` that waits
/// for the drain (none for a seal), and whether the unit had failed at
/// step 1 (a seal then hands it without step 2, D-145).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Handover {
    pub(crate) target: u16,
    pub(crate) started: u64,
    pub(crate) caller: Option<ObjId>,
    pub(crate) failed: bool,
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
            let now = self.membership_now();
            let blocking = names
                .iter()
                .filter(|&&n| (n as usize) < self.table.members.len())
                .any(|&n| {
                    let member = self.table.member(n);
                    // Starting, live, sealing, sealed or fencing, for every
                    // role (D-161: fencing for a plain role too).
                    let alive = matches!(
                        member.state(),
                        MemberState::Starting
                            | MemberState::Live
                            | MemberState::Sealing
                            | MemberState::Sealed
                            | MemberState::Fencing
                    );
                    let fenced =
                        matches!(member.state(), MemberState::Fenced | MemberState::Unfenced);
                    // A plain role moves from an unfenced holder only once
                    // `FENCE_NS` + `SETTLE_NS` have passed since the first
                    // fence action (Roles, kinds of role, D-161).
                    let unsettled = member.state() == MemberState::Unfenced
                        && self.table.fences[n as usize]
                            .begun(member.generation())
                            .is_none_or(|begun| now < begun + FENCE_NS + SETTLE_NS);
                    alive || (fenced && (record.kind == RoleKind::Reserve || unsettled))
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
        // A role already in a hand-over, or rebuilding, is on its way out
        // or not yet served: `busy`, as `hand_all_units` judges it (the
        // final review's verification of lot C, O18 and H17: a second call
        // orphaned the first, which never returned; Roles, the planned
        // hand-over, D-159: "never while X rebuilds u: `role_hand` is then
        // `busy`").
        if self.handing.contains_key(&role) || self.rebuilds.contains_key(&role) {
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
                // "or it is a phantom node (it runs no kernel that could take
                // step 5, as `node_stop` already holds)" (The calls,
                // `role_hand`, gap 13).
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
                        started: self.membership_now(),
                        caller: Some(thread),
                        failed: false,
                    },
                );
                live(&mut self.threads, thread).wait = Wait::Hand(role);
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

    /// The successor of a unit role whose holder is `holder`: the first real
    /// live node after `holder` in cyclic index order whose fence class is the
    /// highest among real live nodes. Phantom nodes hold no core and run no
    /// kernel, so they never take a unit role (Roles, Phantom nodes).
    fn unit_successor(&self, holder: u16) -> Option<u16> {
        let n = self.table.nodes;
        let live = |z: u16| self.table.member(z).state() == MemberState::Live && self.real(z);
        let best = (0..n)
            .filter(|&z| live(z))
            .map(|z| self.table.member(z).class())
            .max()?;
        (1..=n)
            .map(|step| (holder + step) % n)
            .find(|&z| live(z) && self.table.member(z).class() == best)
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
            // A unit this node is rebuilding is not handed yet: the seal
            // waits for the rebuild, which ends in bounded time, as
            // `role_hand` is `busy` for it (Roles, D-159: "a sealing node
            // begins the hand-over of u when its rebuild has ended"; a
            // hand-over begun under a rebuild dropped the rebuild before
            // its closing invalidation and left its `ReportAsk` credits in
            // use for ever).
            if self.rebuilds.contains_key(&role) {
                continue;
            }
            // A failed unit's role is handed too, without step 2 (D-145).
            if word.state() == RoleState::Held && !self.handing.contains_key(&role) {
                let Some(successor) = self.unit_successor(me) else {
                    continue;
                };
                self.handing.insert(
                    role,
                    Handover {
                        target: successor,
                        started: self.membership_now(),
                        caller: None,
                        failed: self.unit_failed(role),
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
        // A request kept during a rebuild has asked the unit nothing: only
        // a submitted one is begun (H17).
        let begun =
            self.unit_service
                .values()
                .any(|s| s.role == role && s.submitted.is_some())
                || self.unit_requests.iter().flatten().any(|e| {
                    e.role == role && matches!(e.stage, crate::units::Stage::Local { .. })
                });
        // Step 2, but on a unit failed at step 1, handed by a seal with no
        // drain and no wait: it never drains, and every request it held was
        // answered *failed* (D-145). A unit that fails during the drain
        // ends the hand-over below, at `IOMMU_WAIT_NS` (D-144), and a seal
        // hands it again at its next step, without the drain.
        let drained = hand.failed
            || !begun && {
                let serial = self.table.units.get(&role).map_or(0, |u| u.next);
                self.confirmed(role, serial)
            };
        if !drained {
            if self.unit_overdue(hand.started) {
                self.fail_unit(role);
                self.abandon(role);
                finish(self, Err(Error::Busy));
                return true;
            }
            return false;
        }
        // Step 3: unmap the unit's registers through the gate.
        if let Some(unit) = self.table.units.get_mut(&role) {
            unit.mapped.remove(&self.node);
        }
        // Step 4: the gate refuses if the target is no longer live (or is a
        // phantom); then this node maps the unit again and stays the holder.
        if self.table.member(hand.target).state() != MemberState::Live || !self.real(hand.target) {
            if let Some(unit) = self.table.units.get_mut(&role) {
                unit.mapped.insert(self.node);
            }
            self.abandon(role);
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

    /// A hand-over that ends without step 4 writes held (this node, a new
    /// epoch) in one gate write before its call returns, so that every
    /// requester it answered *elsewhere* sends its request again (Roles,
    /// D-144: the word kept its epoch, and such a request stayed queued for
    /// ever). The new epoch is the next one: no handing word was written
    /// with it, and a later change of holder writes the one after.
    fn abandon(&mut self, role: usize) {
        let word = self.table.roles[role];
        let held = word.keeping_failed(RoleWord::held(self.node, word.epoch().wrapping_add(1)));
        self.table.write_role(self.node, role, held);
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
        // The instants of the fence record of the generation judged.
        let record = &self.table.fences[judged as usize];
        let same = record.generation == member.generation();
        let at = match member.state() {
            MemberState::Fenced | MemberState::Dead if same => record.confirmed? + SETTLE_NS,
            MemberState::Fenced | MemberState::Dead => return None,
            MemberState::Unfenced => record.begun(member.generation())? + FENCE_NS + SETTLE_NS,
            _ => return None,
        };
        let successor = self.unit_successor(judged)?;
        (successor == self.node).then_some(at)
    }

    /// Step 5 of a hand-over to this node, or a take-over after a death.
    fn take_step(&mut self, role: usize) -> bool {
        let word = self.table.roles[role];
        let me = self.node;
        if word.state() == RoleState::Handing && word.target() == me {
            // Step 5: map the unit, check the queue, write held (e + 2); a
            // failed unit's queue is not checked (it is not here: the model
            // has no queue check at step 5), the word keeps bit 63, and every
            // request is answered *failed* as on any failed unit (D-145).
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
        if self.membership_now() < at {
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
        // A unit whose bit 63 is set already is not rebuilt: no drain, no
        // closing invalidation; every request, kept or sent again at R0, is
        // answered *failed* at once (D-145).
        match word.failed() {
            true => {
                self.fail_unit(role);
                self.unit_send_queued();
            }
            false => {
                self.rebuilds.insert(role, Rebuild::default());
            }
        }
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
            // A dead node too (Roles, R1 at every change of holder: "fencing,
            // fenced, unfenced or dead"): its entries on the unit are
            // cleared, with no acknowledgement to withdraw (H12).
            if member.state() == MemberState::Dead {
                let g = member.generation();
                let unit = self.table.units.entry(role).or_default();
                unit.entries.retain(|_, &mut (n, e)| !(n == node && e == g));
                unit.remap.retain(|_, &mut (n, e)| !(n == node && e == g));
                unit.lines.retain(|_, &mut (n, e)| !(n == node && e == g));
                continue;
            }
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
        if let Some((serial, at)) = rebuild.closing {
            // A unit that does not confirm within `IOMMU_WAIT_NS` has
            // failed: the rebuild ends, and the requests it kept are
            // answered *failed* (IOMMU, D-131; pass 25: it waited for the
            // confirmation for ever, and the requests with it).
            if !self.confirmed(role, serial) && !self.unit_failed(role) {
                if !self.unit_overdue(at) {
                    return false;
                }
                self.fail_unit(role);
            }
            self.rebuilds.remove(&role);
            match self.unit_failed(role) {
                true => {
                    self.fail_unit(role);
                    self.unit_send_queued();
                }
                false => self.serve_kept(role),
            }
            return true;
        }
        let mut progressed = false;
        if let Some((index, serial)) = rebuild.asked {
            // A node accused during the rebuild is skipped (including if it
            // was already restarted afresh after R0 wiped our credit toward
            // its old generation); one that sealed before it answered is then
            // treated as sealed, as one whose turn finds it sealed (Roles: "a
            // node that seals during the rebuild [...] is then treated as
            // sealed", D-167): its entries are kept even if it resumes before
            // the rebuild ends.
            let live_ask = self.credit_in_use(
                rebuild.node,
                index,
                serial,
                dodos_abi::NodeMessage::ReportAsk,
            );
            if !self.table.writes(rebuild.node) || !live_ask {
                if self.table.member(rebuild.node).state() == MemberState::Sealed {
                    rebuild.sealed.insert(rebuild.node);
                }
                rebuild.asked = None;
                rebuild.asked_at = None;
                rebuild.node += 1;
                rebuild.key = 0;
                progressed = true;
            } else if rebuild.asked_at.is_some_and(|at| {
                !self.table.release && self.membership_now() >= at + self.table.stuck_ns
            }) {
                // D-169: no `Report` within `STUCK_NS` of the ask ends that
                // node's report as a malformed one (trace code 11, claims
                // cleared), and the cursor moves on; any late `Report` is
                // dropped and its credit freed in `report_received`.
                let z = rebuild.node;
                self.refuse_trace(z, dodos_abi::NodeMessage::Report, role as u16);
                self.rebuild_cuts.0.push((
                    role,
                    z,
                    self.table.generation(z),
                    crate::units::RebuildCut::Deadline,
                ));
                rebuild.asked = None;
                rebuild.asked_at = None;
                rebuild.node = z + 1;
                rebuild.key = 0;
                progressed = true;
            } else {
                return false;
            }
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
                rebuild.asked_at = None;
                progressed = true;
                continue;
            }
            if !self.table.writes(z) {
                if self.table.member(z).state() == MemberState::Sealed {
                    rebuild.sealed.insert(z);
                }
                rebuild.node += 1;
                rebuild.asked_at = None;
                progressed = true;
                continue;
            }
            // A node starting afresh is not asked until its word leaves
            // `Starting` (D-144, D-147); `START_NS` bounds its start, not
            // the rebuild's `STUCK_NS` ask deadline.
            let member = self.table.member(z);
            if member.state() == MemberState::Starting && !member.resuming() {
                rebuild.asked_at = None;
                self.rebuilds.insert(role, rebuild);
                return progressed;
            }
            // At most one `ReportAsk` in flight to each node, whatever the
            // number of units rebuilt (Roles): an ask a rebuild abandoned,
            // when the node it asked was accused, sealed or cut at
            // `STUCK_NS`, is in flight until its answer is consumed (pass
            // 26, D-169).
            let busy = self.peers[z as usize]
                .credits
                .iter()
                .flatten()
                .any(|c| c.kind == dodos_abi::NodeMessage::ReportAsk);
            let now = self.membership_now();
            if busy {
                let since = *rebuild.asked_at.get_or_insert(now);
                if !self.table.release && now >= since + self.table.stuck_ns {
                    self.refuse_trace(z, dodos_abi::NodeMessage::Report, role as u16);
                    self.rebuild_cuts.0.push((
                        role,
                        z,
                        self.table.generation(z),
                        crate::units::RebuildCut::Deadline,
                    ));
                    rebuild.asked = None;
                    rebuild.asked_at = None;
                    rebuild.node = z + 1;
                    rebuild.key = 0;
                    progressed = true;
                    continue;
                }
                self.rebuilds.insert(role, rebuild);
                return progressed;
            }
            let body = Body::ReportAsk {
                role: role as u16,
                epoch: word.epoch(),
                key: rebuild.key,
            };
            let asked = self.write_message(z, body, Some(Completes::Report { role }), None);
            match asked {
                Ok(credit) => {
                    rebuild.asked = credit;
                    rebuild.asked_at = Some(now);
                    self.rebuilds.insert(role, rebuild);
                    return true;
                }
                Err(crate::rings::Full::Credit | crate::rings::Full::Ring) => {
                    // D-169: waiting because no request credit (or ring
                    // room) toward `z` is free also expires at `STUCK_NS`
                    // from when the cursor reached `z`.
                    let since = *rebuild.asked_at.get_or_insert(now);
                    if !self.table.release && now >= since + self.table.stuck_ns {
                        self.refuse_trace(z, dodos_abi::NodeMessage::Report, role as u16);
                        self.rebuild_cuts.0.push((
                            role,
                            z,
                            self.table.generation(z),
                            crate::units::RebuildCut::Deadline,
                        ));
                        rebuild.asked = None;
                        rebuild.asked_at = None;
                        rebuild.node = z + 1;
                        rebuild.key = 0;
                        progressed = true;
                        continue;
                    }
                    self.rebuilds.insert(role, rebuild);
                    // An ask that found no room or no credit is no progress,
                    // so a step does not spend its units on it (the final
                    // review's verification of lot C, H14).
                    return progressed;
                }
                Err(_) => {
                    self.rebuilds.insert(role, rebuild);
                    return progressed;
                }
            }
        }
        // Every report is in: keep the entries that match, rewrite those
        // that differ, keep without a report the entries of a sealed node,
        // clear every other entry no report claims; then invalidate the
        // whole unit.
        let reported: Vec<(u16, Item)> = rebuild.items.clone();
        let generation = |model: &Self, z: u16| model.table.generation(z);
        // The nodes passed over as sealed, and those sealed now (a node that
        // sealed after its turn reported before it sealed).
        let sealed: Vec<u16> = (0..self.table.nodes)
            .filter(|&z| {
                rebuild.sealed.contains(&z) || self.table.member(z).state() == MemberState::Sealed
            })
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
                .any(|&(z, i)| z == node && matches!(i, Item::Device(d, _) if d == device));
            if !claimed && !sealed.contains(&node) {
                unit.entries.remove(&device);
            }
        }
        // The remapping entries no report claims are cleared too (O17).
        let remaps: Vec<(u32, (u16, u32))> = unit.remap.iter().map(|(&h, &e)| (h, e)).collect();
        for (handle, (node, _)) in remaps {
            let claimed = reported
                .iter()
                .any(|&(z, i)| z == node && i == Item::Handle(handle));
            if !claimed && !sealed.contains(&node) {
                unit.remap.remove(&handle);
            }
        }
        let lines: Vec<(u32, (u16, u32))> = unit.lines.iter().map(|(&l, &e)| (l, e)).collect();
        for (line, (node, _)) in lines {
            let claimed = reported
                .iter()
                .any(|&(z, i)| z == node && matches!(i, Item::Line(l, ..) if l == line));
            if !claimed && !sealed.contains(&node) {
                unit.lines.remove(&line);
            }
        }
        // RISC-V: the sources of the domain nobody claims made inactive,
        // `sourcecfg` 0 (D-147).
        if self.machine.arch == crate::Arch::Riscv64 {
            let kept: Vec<u32> = self.table.units[&role].lines.keys().copied().collect();
            let reported_lines: Vec<u32> = reported
                .iter()
                .filter_map(|&(_, i)| match i {
                    Item::Line(l, ..) => Some(l),
                    _ => None,
                })
                .collect();
            let unclaimed: Vec<u32> = self
                .table
                .aplic
                .keys()
                .copied()
                .filter(|&gsi| self.ioapic_role(gsi) == Some(role))
                .filter(|gsi| !kept.contains(gsi) && !reported_lines.contains(gsi))
                .collect();
            for gsi in unclaimed {
                self.table.aplic.get_mut(&gsi).expect("listed").deactivate();
            }
        }
        for &(z, item) in &reported {
            let g = generation(self, z);
            match item {
                // A context entry only for a device whose attach word reads
                // *attached* with the root the report names (D-137) and
                // whose identifier word names the reporter with its
                // generation (D-141); an item that fails is traced and
                // claims nothing.
                Item::Device(device, root) => {
                    let unit_entry = self.rebuilt_attach(role, device, root, (z, g));
                    let unit = self.table.units.entry(role).or_default();
                    match unit_entry {
                        true => {
                            unit.entries.insert(device, (z, g));
                        }
                        // A claimed entry whose report fails the check is
                        // cleared, not kept with its old root (O19).
                        false => {
                            if unit.entries.get(&device).is_some_and(|&(n, _)| n == z) {
                                unit.entries.remove(&device);
                            }
                        }
                    }
                }
                // A line's entry rewritten from the operands of its
                // operation 5 claimed, checked as at that request (D-137),
                // masked unless its report says it is unmasked (D-139).
                Item::Line(gsi, level, low, target, unmasked) => {
                    let op = crate::rings::UnitOp::Redirection {
                        gsi,
                        claimed: true,
                        level,
                        low,
                        target,
                    };
                    if self.operands_allowed(role, op, (z, g)) {
                        let riscv = self.machine.arch == crate::Arch::Riscv64;
                        let unit = self.table.units.entry(role).or_default();
                        unit.lines.insert(gsi, (z, g));
                        // RISC-V masks a source by its enable, below.
                        match unmasked || riscv {
                            true => unit.masked.remove(&gsi),
                            false => unit.masked.insert(gsi),
                        };
                        // RISC-V: the source's mode and target rewritten,
                        // then for a level source `setipnum`, then
                        // `setienum`, whatever the report's enable says (a
                        // report older than the claimer's last write would
                        // undo it); an edge source's enable from the report
                        // (D-147).
                        if riscv {
                            let line = self.table.aplic.entry(gsi).or_default();
                            line.active = true;
                            match level {
                                true => {
                                    line.pending |= line.high;
                                    line.enabled = true;
                                }
                                false => line.enabled = unmasked,
                            }
                            line.forward();
                        }
                    }
                }
                // A handle in the range of a core whose word names the
                // reporter, checked as at its request (operation 4).
                Item::Handle(handle) => {
                    let op = crate::rings::UnitOp::InterruptCache {
                        handle: handle as u16,
                    };
                    if self.operands_allowed(role, op, (z, g)) {
                        let unit = self.table.units.entry(role).or_default();
                        unit.remap.insert(handle, (z, g));
                    }
                }
            }
        }
        let serial = self.submit(role, crate::rings::UnitOp::Invalidate { domain: None });
        rebuild.closing = Some((serial, self.membership_now()));
        self.rebuilds.insert(role, rebuild);
        true
    }

    /// A rebuild's check of a reported device without side effects: its
    /// attach word reads *attached* with the root (D-137), and its
    /// identifier word names the reporter with its generation, the check of
    /// operation 1 (D-141, D-169).
    pub(crate) fn rebuilt_attach_ok(
        &self,
        role: usize,
        device: u16,
        root: u64,
        (reporter, generation): (u16, u32),
    ) -> bool {
        let Some(key) = self.attach_key(role, device) else {
            return false;
        };
        let word = self.table.attach_word(key);
        let owned = self.table.identifiers.get(&key).is_some_and(|&raw| {
            dodos_abi::Ownership::from_raw(raw).is_some_and(|w| {
                w.state() == dodos_abi::OwnState::Taken
                    && w.owner() == dodos_abi::Owner::Node(reporter)
                    && w.generation() == generation
            })
        });
        owned && word.root() == root && word.state() == Some(dodos_abi::AttachState::Attached)
    }

    /// A rebuild's check of a reported device: its attach word reads
    /// *attached* with the root (D-137), and its identifier word names the
    /// reporter with its generation, the check of operation 1 (D-141: a
    /// faulty node could report another node's device under a domain number
    /// of its own range). A device whose attach is in flight is in no header
    /// and no report; its kept attach is served once the rebuild ends (the
    /// attach word against every event, D-139).
    pub(crate) fn rebuilt_attach(
        &mut self,
        role: usize,
        device: u16,
        root: u64,
        reporter: (u16, u32),
    ) -> bool {
        let good = self.rebuilt_attach_ok(role, device, root, reporter);
        if !good {
            self.refuse_trace(reporter.0, dodos_abi::NodeMessage::Report, device);
        }
        good
    }

    /// A rebuild's check of one reported item against the words of its kind
    /// (Roles, take-over after a death and *The reports are pulled*, D-169):
    /// operation 1 for a context entry, the check of its request for a
    /// handle or a line.
    pub(crate) fn item_allowed(&self, role: usize, item: Item, reporter: (u16, u32)) -> bool {
        match item {
            Item::Device(device, root) => self.rebuilt_attach_ok(role, device, root, reporter),
            Item::Handle(handle) => u16::try_from(handle).is_ok_and(|handle| {
                self.operands_allowed(
                    role,
                    crate::rings::UnitOp::InterruptCache { handle },
                    reporter,
                )
            }),
            Item::Line(gsi, level, low, target, _) => {
                self.ioapic_role(gsi) == Some(role)
                    && self.operands_allowed(
                        role,
                        crate::rings::UnitOp::Redirection {
                            gsi,
                            claimed: true,
                            level,
                            low,
                            target,
                        },
                        reporter,
                    )
            }
        }
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
            let at = self.membership_now();
            if let Some(entry) = self.unit_service.get_mut(&(peer, credit)) {
                entry.submitted = Some((serial, at));
            }
        }
        self.unit_send_queued();
    }

    /// This node's items on a unit, in the fixed order of a report: its
    /// devices attached through the unit, from its own list, by identifier;
    /// then the lines it claimed, by global system interrupt.
    pub(crate) fn items(&self, role: usize) -> Vec<Item> {
        let record = self.table.role_records[role];
        let mut items: Vec<Item> = match record.governs {
            // Its devices, then the remapping handles it claimed whose entry
            // the unit holds (the final review's verification of lot C,
            // O17: no handle was ever reported).
            dodos_abi::Governs::IommuUnit => self
                .iommus
                .values()
                .filter(|u| u.unit == usize::from(record.object))
                .flat_map(|u| {
                    u.attached.iter().map(|(&d, io)| {
                        let root = self.aspaces.get(io).and_then(|a| a.root).unwrap_or(0);
                        Item::Device(d, root)
                    })
                })
                .chain(self.irqs.values().filter_map(|irq| {
                    let handle = self.handle_of(irq);
                    let entry = self.table.irq_tables.get(&handle)?;
                    entry
                        .units
                        .contains(&role)
                        .then_some(Item::Handle(u32::from(handle)))
                }))
                .collect(),
            dodos_abi::Governs::IoApic => self
                .irqs
                .values()
                .filter(|irq| {
                    irq.source.is_line() && self.ioapic_role(irq.source.id()) == Some(role)
                })
                .map(|irq| {
                    let target = self.line_target(irq.core, irq.vector);
                    // The mask state: an edge line is unmasked while its irq
                    // is not dying; a level line only while its irq is armed
                    // on x86-64 without a directed end of interrupt, and
                    // elsewhere while it is not dying, its claimer never
                    // unmasking it (Interrupts; D-139, D-147).
                    // RISC-V: the enable of the APLIC source as it reads,
                    // which a rebuild ignores for a level source (D-147).
                    let level_masked = match self.machine.arch {
                        crate::Arch::Riscv64 => !self
                            .table
                            .aplic
                            .get(&irq.source.id())
                            .is_some_and(|line| line.enabled),
                        _ => self.masks_at_arrival() && irq.state != crate::IrqState::Armed,
                    };
                    let unmasked = !irq.removing && !(irq.source.level() && level_masked);
                    Item::Line(
                        irq.source.id(),
                        irq.source.level(),
                        irq.source.active_low(),
                        target,
                        unmasked,
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        items.sort();
        items
    }

    /// A `ReportAsk`: answered at once with one `Report`, its items from
    /// the key asked, as many as fit, and the key of the next item or end;
    /// an ask whose role, epoch or holder is stale is answered with no item
    /// and next key *end* so its credit is freed (D-167). The asked node
    /// keeps nothing.
    pub(crate) fn report_asked(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        role: u16,
        epoch: u32,
        key: u64,
    ) {
        if self.faulty_report == crate::units::FaultyReport::Silent {
            self.faulty_report_used = true;
            if let Some((index, serial)) = credit {
                self.swallowed_asks
                    .insert((from, self.table.generation(from), index, serial));
            }
            return;
        }
        let current = self.table.roles.get(role as usize).is_some_and(|w| {
            self.table.role_records[role as usize].kind == RoleKind::Unit
                && w.state() == RoleState::Held
                && w.epoch() == epoch
                && w.holder() == from
        });
        if !current {
            let body = Body::Report {
                next: REPORT_END,
                items: Vec::new(),
            };
            let _ = self.write_message(from, body, None, credit);
            return;
        }
        if self.faulty_report == crate::units::FaultyReport::BadItems {
            self.faulty_report_used = true;
            let bad: Vec<Item> = match self.table.role_records[role as usize].governs {
                dodos_abi::Governs::IommuUnit => vec![
                    Item::Device(0xfffe, 0),
                    Item::Device(0xffff, 0),
                    Item::Handle(0xffff_fffe),
                    Item::Handle(0xffff_ffff),
                ],
                dodos_abi::Governs::IoApic => vec![
                    Item::Line(
                        0xffff_fffe,
                        false,
                        false,
                        crate::rings::Target::Masked,
                        false,
                    ),
                    Item::Line(
                        0xffff_ffff,
                        false,
                        false,
                        crate::rings::Target::Masked,
                        false,
                    ),
                ],
                _ => Vec::new(),
            };
            let sent: Vec<Item> = bad.into_iter().filter(|i| i.key() >= key).take(2).collect();
            let next = sent
                .last()
                .map_or(REPORT_END, |i| i.key().saturating_add(1));
            let body = Body::Report { next, items: sent };
            let _ = self.write_message(from, body, None, credit);
            return;
        }
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
    /// ask from the next key, or the next node at the end. Taken only for
    /// the ask the cursor currently waits on; any other frees its credit
    /// and is dropped (D-167).
    pub(crate) fn report_received(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        next: u64,
        items: Vec<Item>,
    ) {
        let Some((index, serial)) = credit else {
            return;
        };
        let Some(taken) = self.take_credit(from, index) else {
            return;
        };
        let Completes::Report { role } = taken.completes else {
            return;
        };
        // A report for any ask other than the one the cursor currently waits
        // on (e.g. a node passed over as accused, sealed, or cut at
        // `STUCK_NS`, or an ask from an earlier epoch) frees its credit and
        // moves nothing (pass 26, D-167, D-169).
        let Some(rebuild) = self
            .rebuilds
            .get(&role)
            .filter(|r| r.asked == Some((index, serial)) && r.node == from)
        else {
            return;
        };
        // The holder takes a `Report` only if it holds at most 28 items
        // whose keys rise strictly from the key asked, a next key (unless
        // end) above the last of them, and every item passes the check of
        // its kind (D-141, D-169); otherwise it traces code 11, refuses the
        // whole `Report`, and treats that node's report as ended.
        let asked = rebuild.key;
        let mut last = None;
        let rising = items.iter().all(|item| {
            let key = item.key();
            let above = last.map_or(key >= asked, |l| key > l);
            last = Some(key);
            above
        });
        // CONTRACT-GAP 63: Roles (*The reports are pulled*, D-169) states
        // that a rebuild asks each node at most `ceil(items / 28) + 1` times,
        // "a faulty node included, since every item taken is an entry its
        // words give it", yet the take condition requires only `count <= 28`,
        // keys rising strictly from `asked`, `next` (unless *end*) above the
        // last item, and every item passing its kind check. Does a non-final
        // `Report` (`next != REPORT_END`) have to carry 28 items (or at least
        // 1 item)? As written, a faulty node can answer with 0 items and
        // `next = asked + 1` (unbounded asks; also exercised by
        // `verify13::a_report_asked_from_a_key_takes_the_item_of_that_key`),
        // or with 1 valid item per `Report` (`items + 1` asks instead of
        // `ceil(items / 28) + 1`). Reading closest to the text: keep the
        // stated take condition (`next == REPORT_END || last.map_or(next >
        // asked, |l| next > l)`) without adding an unstated `items.len() ==
        // REPORT_ITEMS` check.
        let advances = next == REPORT_END || last.map_or(next > asked, |l| next > l);
        let reporter = (from, self.table.generation(from));
        let structural = items.len() <= REPORT_ITEMS && rising && advances;
        let allowed = items
            .iter()
            .all(|&item| self.item_allowed(role, item, reporter));
        let good = structural && allowed;
        if !good {
            self.refuse_trace(from, dodos_abi::NodeMessage::Report, role as u16);
            let reason = match structural {
                false => crate::units::RebuildCut::Malformed,
                true => crate::units::RebuildCut::BadItem,
            };
            self.rebuild_cuts.0.push((role, from, reporter.1, reason));
        }
        let rebuild = self.rebuilds.get_mut(&role).expect("a rebuild");
        if good {
            rebuild.items.extend(items.into_iter().map(|i| (from, i)));
        }
        rebuild.asked = None;
        rebuild.asked_at = None;
        match next {
            key if good && key != REPORT_END => rebuild.key = key,
            _ => {
                rebuild.node = from + 1;
                rebuild.key = 0;
            }
        }
    }
}
