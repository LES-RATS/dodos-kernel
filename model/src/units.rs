//! Devices and shared hardware (D-131): unit requests. A unit request is
//! held by the requester, in its unit request table, until a holder of the
//! unit's role answers it done or *failed*; the holder keeps it in its unit
//! service table only while the unit works on it. The unit itself is the
//! machine's (`table::UnitState`): an operation submitted to it applies at
//! once and is confirmed at a later check, unless the unit is broken; a
//! unit that has not confirmed `IOMMU_WAIT_NS` after a submission has
//! failed (IOMMU).

use crate::rings::{Body, Completes, UnitOp};
use crate::{CapId, Model, ObjId, Output, Wait, locked};
use dodos_abi::{
    Error, IOMMU_WAIT_NS, MemberState, RoleState, TraceCode, UNIT_REQUESTS, UnitResult,
};

/// Where an entry of the unit request table stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stage {
    /// Waiting for a holder to send it to.
    Queued,
    /// In the ring to the holder, with the request credit it uses.
    Sent { to: u16, credit: u8 },
    /// Served by this node, which holds the role: submitted to the unit.
    Local { submitted: u64, at: u64 },
}

/// What the end of a unit request completes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Waiter {
    /// The thread of a call, in *waiting for a unit* (none once it is
    /// destroyed or its wait ended otherwise).
    Thread(Option<ObjId>),
    /// The record left *unmapping* by a removal step, a step of a `Revoke`
    /// or a step of R2, for which no thread waits.
    Record(CapId),
}

/// An entry of the unit request table: the role, the operation, the epoch
/// it was last sent with, where it stands, and what it completes. The
/// record of an unmapping whose call waits is in `record`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnitEntry {
    pub(crate) role: usize,
    pub(crate) op: UnitOp,
    pub(crate) epoch: u32,
    pub(crate) stage: Stage,
    pub(crate) waiter: Waiter,
    pub(crate) record: Option<CapId>,
    /// The order of the entry, for "oldest first".
    pub(crate) order: u64,
    /// The result a done gives the waiting call, if not none: the message
    /// words of `irq_claim`.
    pub(crate) words: Option<(u64, u64)>,
}

/// An entry of the unit service table: a request of another node that the
/// holder serves, submitted to the unit (or kept while the unit is
/// rebuilt).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Served {
    pub(crate) role: usize,
    pub(crate) op: UnitOp,
    pub(crate) serial: u32,
    /// The submission and its instant; `None` while kept for a rebuild.
    pub(crate) submitted: Option<(u64, u64)>,
}

/// The cursor of a rebuild of a unit role this node took over (Roles):
/// the node it asks and the key from which it asks, whether an ask is in
/// flight, and the items reported so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Rebuild {
    pub(crate) node: u16,
    pub(crate) key: u64,
    pub(crate) asked: bool,
    pub(crate) items: Vec<(u16, crate::rings::Item)>,
    /// The drain and the final invalidation of the whole unit: its
    /// submission, once made.
    pub(crate) closing: Option<(u64, u64)>,
}

impl Model {
    /// The unit role that governs an IOMMU unit, by the unit's index.
    pub(crate) fn unit_role(&self, unit: usize) -> usize {
        self.table
            .role_records
            .iter()
            .position(|r| {
                r.kind == dodos_abi::RoleKind::Unit
                    && r.governs == dodos_abi::Governs::IommuUnit
                    && usize::from(r.object) == unit
            })
            .expect("a unit role per IOMMU unit")
    }

    /// The unit role of the IOAPIC that serves a global system interrupt.
    pub(crate) fn ioapic_role(&self, gsi: u32) -> Option<usize> {
        let index = self
            .machine
            .ioapics
            .iter()
            .position(|io| io.first_gsi <= gsi && gsi < io.first_gsi + io.inputs)?;
        self.table.role_records.iter().position(|r| {
            r.kind == dodos_abi::RoleKind::Unit
                && r.governs == dodos_abi::Governs::IoApic
                && usize::from(r.object) == index
        })
    }

    /// Whether the unit of a role has failed (bit 63 of its role word).
    pub(crate) fn unit_failed(&self, role: usize) -> bool {
        self.table.roles[role].failed()
    }

    /// Makes an entry of the unit request table for a call or a step. A
    /// call that finds no free entry returns `again` with nothing changed;
    /// a step returns `again` in the same way.
    pub(crate) fn unit_request(
        &mut self,
        role: usize,
        op: UnitOp,
        waiter: Waiter,
        record: Option<CapId>,
    ) -> Result<(), Error> {
        self.unit_request_with(role, op, waiter, record, None)
    }

    /// A unit request whose done gives the waiting call these result words
    /// (`irq_claim`).
    pub(crate) fn unit_request_with(
        &mut self,
        role: usize,
        op: UnitOp,
        waiter: Waiter,
        record: Option<CapId>,
        words: Option<(u64, u64)>,
    ) -> Result<(), Error> {
        let free = self.unit_requests.iter().position(Option::is_none);
        let Some(index) = free else {
            return Err(Error::Again);
        };
        self.unit_order += 1;
        self.unit_requests[index] = Some(UnitEntry {
            role,
            op,
            epoch: self.table.roles[role].epoch(),
            stage: Stage::Queued,
            waiter,
            record,
            order: self.unit_order,
            words,
        });
        if let Waiter::Thread(Some(thread)) = waiter {
            locked(&mut self.threads, &self.locks, thread).wait = Wait::Unit(index);
            self.wait_mark(crate::locks::WaitKind::Unit);
            self.observe();
        }
        self.unit_send_queued();
        Ok(())
    }

    /// The entries of the unit request table, served oldest first: an entry
    /// is sent when the role word reads held by a live node and a request
    /// credit toward that node is free, with the epoch the word then holds;
    /// an entry for a role this node holds is submitted to the unit with no
    /// message.
    pub(crate) fn unit_send_queued(&mut self) {
        let mut queued: Vec<(u64, usize)> = self
            .unit_requests
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.filter(|e| e.stage == Stage::Queued).map(|e| (e.order, i)))
            .collect();
        queued.sort();
        for (_, index) in queued {
            let entry = self.unit_requests[index].expect("queued");
            let word = self.table.roles[entry.role];
            if word.state() != RoleState::Held {
                continue;
            }
            let holder = word.holder();
            if self.table.member(holder).state() != MemberState::Live
                && self.table.member(holder).state() != MemberState::Sealing
            {
                continue;
            }
            let epoch = word.epoch();
            if holder == self.node {
                if !self.serves_role(entry.role) {
                    continue;
                }
                if self.unit_failed(entry.role) {
                    self.unit_complete(index, UnitResult::Failed);
                    continue;
                }
                let submitted = self.submit(entry.role, entry.op);
                let e = self.unit_requests[index].as_mut().expect("queued");
                (e.epoch, e.stage) = (
                    epoch,
                    Stage::Local {
                        submitted,
                        at: self.now,
                    },
                );
                // An operation the unit does not confirm is done at once.
                if matches!(entry.op, UnitOp::Redirection { .. }) {
                    self.unit_complete(index, UnitResult::Done);
                }
                continue;
            }
            let body = Body::UnitRequest {
                role: entry.role as u16,
                epoch,
                op: entry.op,
            };
            let completes = Completes::Unit { entry: index };
            if let Ok(Some((credit, _))) = self.write_message(holder, body, Some(completes), None) {
                let e = self.unit_requests[index].as_mut().expect("queued");
                (e.epoch, e.stage) = (epoch, Stage::Sent { to: holder, credit });
            }
        }
    }

    /// Whether this node serves a role it holds: not while it hands it
    /// over (from step 1), nor while it rebuilds it.
    fn serves_role(&self, role: usize) -> bool {
        !self.handing.contains_key(&role) && !self.rebuilds.contains_key(&role)
    }

    /// The machine's unit: an operation submitted applies at once, and is
    /// confirmed at a later check. Returns the submission's serial.
    pub(crate) fn submit(&mut self, role: usize, op: UnitOp) -> u64 {
        let requester = (self.node, self.table.generation(self.node));
        self.submit_for(role, op, requester)
    }

    pub(crate) fn submit_for(&mut self, role: usize, op: UnitOp, requester: (u16, u32)) -> u64 {
        let now = self.now;
        let unit = self.table.units.entry(role).or_default();
        unit.next += 1;
        let serial = unit.next;
        match op {
            UnitOp::Attach { device } => {
                unit.entries.insert(device, requester);
            }
            UnitOp::Detach { device } => {
                unit.entries.remove(&device);
            }
            UnitOp::Redirection { gsi, claimed } => {
                match claimed {
                    true => unit.lines.insert(gsi, requester),
                    false => unit.lines.remove(&gsi),
                };
            }
            UnitOp::Invalidate | UnitOp::InterruptCache => {}
        }
        // A redirection entry, written through an index and a data
        // register, is not confirmed by the unit: answered at once.
        if !matches!(op, UnitOp::Redirection { .. }) {
            unit.queue.push((serial, now));
        }
        serial
    }

    /// Whether the unit has confirmed a submission. A unit that works
    /// confirms what was submitted before the current instant's check; a
    /// broken one confirms nothing.
    pub(crate) fn confirmed(&mut self, role: usize, serial: u64) -> bool {
        let unit = self.table.units.entry(role).or_default();
        if !unit.broken {
            unit.queue.clear();
        }
        !unit.queue.iter().any(|&(s, _)| s == serial)
    }

    /// A `UnitRequest` received by a holder: judged by the role word. Served
    /// if the word reads held by this node with the request's epoch and
    /// this node has not begun to hand the role over; answered *failed* at
    /// once if the unit has failed; kept while the unit is rebuilt;
    /// answered *elsewhere* otherwise.
    pub(crate) fn unit_requested(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        (role, epoch, op): (u16, u32, UnitOp),
    ) {
        let Some((index, serial)) = credit else {
            return;
        };
        let role = role as usize;
        let word = self.table.roles.get(role).copied();
        let held = word.is_some_and(|w| {
            w.state() == RoleState::Held && w.holder() == self.node && w.epoch() == epoch
        });
        let answer = |model: &mut Self, result| {
            let _ = model.write_message(from, Body::UnitAnswer { result }, None, credit);
        };
        if !held || self.handing.contains_key(&role) {
            return answer(self, UnitResult::Elsewhere);
        }
        if self.unit_failed(role) {
            return answer(self, UnitResult::Failed);
        }
        let submitted = match self.rebuilds.contains_key(&role) {
            true => None,
            false => {
                let requester = (from, self.table.generation(from));
                let serial = self.submit_for(role, op, requester);
                // An operation the unit does not confirm is answered at once.
                if matches!(op, UnitOp::Redirection { .. }) {
                    return answer(self, UnitResult::Done);
                }
                Some((serial, self.now))
            }
        };
        self.unit_service.insert(
            (from, index),
            Served {
                role,
                op,
                serial,
                submitted,
            },
        );
    }

    /// A `UnitAnswer` received by the requester, applied in the event that
    /// consumes it whether or not a thread still waits: done completes the
    /// entry, *failed* ends it with its counts kept, *elsewhere* queues it
    /// again, sent at once if the role word now reads held by a live node.
    pub(crate) fn unit_answered(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        result: UnitResult,
    ) {
        let Some((index, _)) = credit else { return };
        let Some(taken) = self.take_credit(from, index) else {
            return;
        };
        let Completes::Unit { entry } = taken.completes else {
            return;
        };
        match result {
            UnitResult::Elsewhere => {
                if let Some(e) = self.unit_requests[entry].as_mut() {
                    e.stage = Stage::Queued;
                }
                self.unit_send_queued();
            }
            done_or_failed => self.unit_complete(entry, done_or_failed),
        }
    }

    /// The end of an entry: done lowers the counts of the record's leaves
    /// and empties it, or ends the waiting call with its result; *failed*
    /// empties the record with its counts kept, or ends the call with
    /// `busy`.
    fn unit_complete(&mut self, index: usize, result: UnitResult) {
        let Some(entry) = self.unit_requests[index].take() else {
            return;
        };
        let record = match entry.waiter {
            Waiter::Record(cap) => Some(cap),
            Waiter::Thread(_) => entry.record,
        };
        if let Some(cap) = record {
            match result {
                UnitResult::Done => self.finish_unmapping(cap),
                _ => self.freeze_unmapping(cap),
            }
        }
        if let Waiter::Thread(Some(thread)) = entry.waiter
            && self
                .threads
                .get(&thread)
                .is_some_and(|t| t.wait == Wait::Unit(index))
        {
            self.end_bounded(thread);
            let outcome = match (result, entry.words) {
                (UnitResult::Done, Some((address, data))) => Ok(Output::Msi { address, data }),
                (UnitResult::Done, None) => Ok(Output::Done),
                _ => Err(Error::Busy),
            };
            self.wake(thread, outcome);
        }
    }

    /// The thread of a wait for a unit that is destroyed: the confirmation,
    /// when it comes, finds no thread to wake.
    pub(crate) fn forget_unit_waiter(&mut self, thread: ObjId, entry: usize) {
        if let Some(e) = self.unit_requests.get_mut(entry).and_then(Option::as_mut)
            && e.waiter == Waiter::Thread(Some(thread))
        {
            e.waiter = Waiter::Thread(None);
        }
    }

    /// At each step of the recovery reserve: the holder checks for the
    /// confirmation of what it submitted, answers done at the confirmation
    /// and *failed* at `IOMMU_WAIT_NS`; the requester completes its own
    /// entries in the same way and sends what is queued. Returns whether it
    /// did anything.
    pub(crate) fn unit_step(&mut self) -> bool {
        let mut any = false;
        // Its own entries, served with no message.
        for index in 0..UNIT_REQUESTS {
            let Some(entry) = self.unit_requests[index] else {
                continue;
            };
            let Stage::Local { submitted, at } = entry.stage else {
                continue;
            };
            if self.confirmed(entry.role, submitted) {
                self.unit_complete(index, UnitResult::Done);
                any = true;
            } else if at + IOMMU_WAIT_NS <= self.now {
                self.fail_unit(entry.role);
                any = true;
            }
        }
        // The requests of other nodes it serves.
        let served: Vec<((u16, u8), Served)> =
            self.unit_service.iter().map(|(&k, &v)| (k, v)).collect();
        for ((peer, credit), entry) in served {
            let Some((serial, at)) = entry.submitted else {
                continue;
            };
            let result = if self.confirmed(entry.role, serial) {
                UnitResult::Done
            } else if at + IOMMU_WAIT_NS <= self.now {
                self.fail_unit(entry.role);
                continue;
            } else {
                continue;
            };
            self.unit_service.remove(&(peer, credit));
            let answer = Body::UnitAnswer { result };
            let _ = self.write_message(peer, answer, None, Some((credit, entry.serial)));
            any = true;
        }
        let before: Vec<Option<UnitEntry>> = self.unit_requests.clone();
        self.unit_send_queued();
        any || before != self.unit_requests
    }

    /// The earliest instant a unit wait ends by time: `IOMMU_WAIT_NS` after
    /// the oldest submission not yet confirmed.
    pub(crate) fn unit_deadline(&self) -> Option<u64> {
        let own = self
            .unit_requests
            .iter()
            .flatten()
            .filter_map(|e| match e.stage {
                Stage::Local { at, .. } => Some(at),
                _ => None,
            });
        let served = self
            .unit_service
            .values()
            .filter_map(|s| s.submitted.map(|(_, at)| at));
        own.chain(served).min().map(|at| at + IOMMU_WAIT_NS)
    }

    /// Whether a step of the unit requests could make progress now.
    pub(crate) fn unit_work(&self) -> bool {
        let pending = |role: usize, serial: u64| {
            self.table
                .units
                .get(&role)
                .is_some_and(|u| !u.broken && u.queue.iter().any(|&(s, _)| s == serial))
        };
        let own = self.unit_requests.iter().flatten().any(|e| match e.stage {
            Stage::Local { submitted, .. } => pending(e.role, submitted),
            Stage::Queued => {
                let word = self.table.roles[e.role];
                word.state() == RoleState::Held
                    && matches!(
                        self.table.member(word.holder()).state(),
                        MemberState::Live | MemberState::Sealing
                    )
            }
            Stage::Sent { .. } => false,
        });
        let served = self.unit_service.values().any(|s| {
            s.submitted
                .is_some_and(|(serial, _)| pending(s.role, serial))
        });
        own || served
    }

    /// A unit that has not confirmed within `IOMMU_WAIT_NS` has failed: it
    /// is the faulty party, never a node. The holder writes the *failed*
    /// bit in the role word through its gate, writes code 4 in its kernel
    /// ring, and answers *failed* to every request it holds for the unit
    /// and to every later one.
    pub(crate) fn fail_unit(&mut self, role: usize) {
        if !self.unit_failed(role) {
            let word = self.table.roles[role];
            self.table.write_role(self.node, role, word.failed_now());
            let unit = u64::from(self.table.role_records[role].object);
            self.trace(TraceCode::UnitTimeout, None, [unit, 0, 0]);
        }
        for index in 0..UNIT_REQUESTS {
            if self.unit_requests[index]
                .is_some_and(|e| e.role == role && matches!(e.stage, Stage::Local { .. }))
            {
                self.unit_complete(index, UnitResult::Failed);
            }
        }
        let held: Vec<(u16, u8)> = self
            .unit_service
            .iter()
            .filter(|(_, s)| s.role == role)
            .map(|(&k, _)| k)
            .collect();
        for (peer, credit) in held {
            let entry = self.unit_service.remove(&(peer, credit)).expect("listed");
            let answer = Body::UnitAnswer {
                result: UnitResult::Failed,
            };
            let _ = self.write_message(peer, answer, None, Some((credit, entry.serial)));
        }
    }

    /// R0 for a node `peer` that held a role: the entries sent to it are
    /// queued again and their credits freed (the credits are freed by R0
    /// itself); the entries of its own requests this node serves are
    /// dropped, with no answer.
    pub(crate) fn unit_r0(&mut self, peer: u16) {
        for entry in self.unit_requests.iter_mut().flatten() {
            if matches!(entry.stage, Stage::Sent { to, .. } if to == peer) {
                entry.stage = Stage::Queued;
            }
        }
        self.unit_service.retain(|&(from, _), _| from != peer);
    }
}
