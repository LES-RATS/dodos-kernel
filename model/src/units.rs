//! Devices and shared hardware (D-131): unit requests. A unit request is
//! held by the requester, in its unit request table, until a holder of the
//! unit's role answers it done or *failed*; the holder keeps it in its unit
//! service table only while the unit works on it. The unit itself is the
//! machine's (`table::UnitState`): an operation submitted to it applies at
//! once and is confirmed at a later check, unless the unit is broken; a
//! unit that has not confirmed `IOMMU_WAIT_NS` after a submission has
//! failed (IOMMU).

use crate::rings::{Body, Completes, UnitOp};
use crate::{CapId, Model, ObjId, Output, Wait, live};
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
    /// A step for which no thread waits and no record is left: a device
    /// detached by the destruction of its iospace.
    Step,
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
    /// For a claim that writes a remapping entry, the PCI segment whose
    /// units are asked one after another: at a done or a *failed*, the
    /// entry is used again for the next unit of that segment (Devices and
    /// shared hardware, several remapping units, gap 12, D-134). The cursor
    /// is the entry's role and this segment.
    pub(crate) segment: Option<u16>,
    /// Whether a unit of the sequence was found failed: the call then ends
    /// `busy` after the last one.
    pub(crate) failed: bool,
    /// For the claim of a remapped line, its redirection entry, asked of
    /// the holder of the IOAPIC's role after the last unit (D-134).
    pub(crate) then: Option<(usize, UnitOp)>,
    /// Answered *elsewhere* at `epoch`: sent again only once the role word
    /// reads another epoch (Roles, D-141).
    pub(crate) elsewhere: bool,
}

/// What follows the first unit of a claim: the segment whose later units
/// it asks, and the redirection entry of a line after the last (gap 12).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Chain {
    pub(crate) segment: Option<u16>,
    pub(crate) then: Option<(usize, UnitOp)>,
}

/// An entry of the unit service table: a request of another node that the
/// holder serves, submitted to the unit (or kept while the unit is
/// rebuilt).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Served {
    pub(crate) role: usize,
    pub(crate) op: UnitOp,
    pub(crate) serial: u32,
    /// The requester's entry and the serial of its record's state word,
    /// which the holder's gate answers (D-151, D-160).
    pub(crate) record: (usize, u64),
    /// The submission and its instant; `None` while kept for a rebuild.
    pub(crate) submitted: Option<(u64, u64)>,
}

/// A test's input: how a faulty node answers a `ReportAsk` (D-169).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FaultyReport {
    /// Normal kernel behaviour.
    #[default]
    Normal,
    /// Consumes the `ReportAsk` and never answers.
    Silent,
    /// Answers with strictly rising keys and items that fail their kind check.
    BadItems,
}

/// Why a rebuild ended a node's report early (D-141, D-169).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RebuildCut {
    /// No `Report` arrived within `STUCK_NS` of the ask or of reaching the
    /// node while no request credit toward it was free.
    Deadline,
    /// A `Report` carried an item that failed the check of its kind.
    BadItem,
    /// A `Report` failed the structural checks (count > 28, keys not rising,
    /// or `next` not advancing).
    Malformed,
}

/// Identifies one `ReportAsk` handled by a peer (D-169): the peer and its
/// generation, the asking holder and its generation, and the request credit
/// `(index, serial)` that carried the ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReportAskId {
    pub(crate) peer: u16,
    pub(crate) peer_gen: u32,
    pub(crate) holder: u16,
    pub(crate) holder_gen: u32,
    pub(crate) index: u8,
    pub(crate) serial: u32,
}

/// One rebuild cut recorded by a holder (D-141, D-169): the role, the peer
/// and its generation, the reason, and the request credit `(index, serial)`
/// of the `ReportAsk` the cut waited on or answered (if any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RebuildCutRecord {
    pub(crate) role: usize,
    pub(crate) peer: u16,
    pub(crate) generation: u32,
    pub(crate) reason: RebuildCut,
    pub(crate) credit: Option<(u8, u32)>,
}

/// The cursor of a rebuild of a unit role this node took over (Roles):
/// the node it asks and the key from which it asks, whether an ask is in
/// flight, when the wait on this node began, and the items reported so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Rebuild {
    pub(crate) node: u16,
    pub(crate) key: u64,
    /// The credit `(index, serial)` of the `ReportAsk` the cursor currently
    /// waits on, if one is in flight (D-167).
    pub(crate) asked: Option<(u8, u32)>,
    /// The instant on the clock less the stopped time when the current ask
    /// was sent, or when the cursor reached `node` while no request credit
    /// toward it was free (D-169): if no `Report` arrives within `STUCK_NS`,
    /// `node`'s report is ended and the cursor moves on.
    pub(crate) asked_at: Option<u64>,
    pub(crate) items: Vec<(u16, crate::rings::Item)>,
    /// The nodes the rebuild passed over because they were sealed: their
    /// entries are kept without a report, even if they resume before the
    /// rebuild ends (the final review's verification of lot C, H11).
    pub(crate) sealed: std::collections::BTreeSet<u16>,
    /// The `(peer, generation)` pairs whose report this rebuild cut early
    /// (D-141, D-169), so lines cleared at completion are recorded in
    /// `UnitState::cut_lines` only for those cuts.
    pub(crate) cut: std::collections::BTreeSet<(u16, u32)>,
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

    /// The unit role of the line controller (IOAPIC, GIC distributor or
    /// APLIC) that serves a global system interrupt.
    pub(crate) fn ioapic_role(&self, gsi: u32) -> Option<usize> {
        let index = self
            .machine
            .ioapics
            .iter()
            .position(|io| io.first_gsi <= gsi && gsi < io.first_gsi + io.inputs)?;
        self.table.role_records.iter().position(|r| {
            r.kind == dodos_abi::RoleKind::Unit
                && crate::table::governs_lines(r.governs)
                && usize::from(r.object) == index
        })
    }

    /// Whether the unit of a role has failed (bit 63 of its role word).
    pub(crate) fn unit_failed(&self, role: usize) -> bool {
        self.table.roles[role].failed()
    }

    /// Whether the role word of any IOMMU unit reads *failed* (Reclaim,
    /// R6; D-154, F14).
    pub(crate) fn an_iommu_unit_failed(&self) -> bool {
        self.table
            .role_records
            .iter()
            .enumerate()
            .any(|(role, record)| {
                record.governs == dodos_abi::Governs::IommuUnit && self.unit_failed(role)
            })
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
        self.unit_request_with(role, op, waiter, record, None, Chain::default())
            .map(|_| ())
    }

    /// A unit request whose done gives the waiting call these result words,
    /// and the later units and redirection entry of a claim (`irq_claim`),
    /// all written in the entry before it is first sent: a unit this node
    /// holds is served at once, and a failed one is passed over at once.
    pub(crate) fn unit_request_with(
        &mut self,
        role: usize,
        op: UnitOp,
        waiter: Waiter,
        record: Option<CapId>,
        words: Option<(u64, u64)>,
        chain: Chain,
    ) -> Result<usize, Error> {
        let free = self.unit_requests.iter().position(Option::is_none);
        let Some(index) = free else {
            let _ = self.unit_requests_full();
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
            segment: chain.segment,
            failed: false,
            then: chain.then,
            elsewhere: false,
        });
        self.table.open_unit_record(self.node, index, role, op);
        if let Waiter::Thread(Some(thread)) = waiter {
            live(&mut self.threads, thread).wait = Wait::Unit(index);
            self.wait_mark(crate::locks::WaitKind::Unit);
            self.observe();
        }
        self.unit_send_queued();
        Ok(index)
    }

    /// The IOMMU units that serve a PCI segment, by role, in the order of
    /// the units.
    pub(crate) fn segment_roles(&self, segment: u16) -> Vec<usize> {
        let units = self.machine.iommus.iter().enumerate();
        let serving = units.filter(|(_, unit)| unit.segment == segment);
        serving.map(|(unit, _)| self.unit_role(unit)).collect()
    }

    /// At the done of a unit of a claim, the next unit of its segment: "it
    /// waits for the answer of one before it sends the next, so the thread
    /// holds one wait and one entry of the unit request table at a time"
    /// (Devices and shared hardware, gap 12). Returns whether the entry was
    /// used again.
    fn next_unit(&mut self, index: usize, entry: UnitEntry, failed: bool) -> bool {
        let failed = entry.failed || failed;
        let next = entry.segment.and_then(|segment| {
            let roles = self.segment_roles(segment);
            let after = roles.iter().position(|&r| r == entry.role).map(|i| i + 1);
            after.and_then(|i| roles.get(i)).copied()
        });
        // After the last unit, the redirection entry of a remapped line.
        let (role, op, segment, then) = match (next, entry.then) {
            (Some(role), then) => (role, entry.op, entry.segment, then),
            (None, Some((role, op))) => (role, op, None, None),
            (None, None) => return false,
        };
        self.unit_order += 1;
        self.unit_requests[index] = Some(UnitEntry {
            role,
            op,
            epoch: self.table.roles[role].epoch(),
            stage: Stage::Queued,
            order: self.unit_order,
            segment,
            failed,
            then,
            elsewhere: false,
            ..entry
        });
        self.table.open_unit_record(self.node, index, role, op);
        self.unit_send_queued();
        true
    }

    /// The domain numbers of node slot `slot` on IOMMU unit `unit`: 0 is
    /// never used and the last is the kernel's own, and slot *s* owns 1 +
    /// *s* × *d* to (*s* + 1) × *d*, with *d* = ⌊(*D* − 2) / *n*⌋ (Devices
    /// and shared hardware, domain numbers, D-134).
    pub(crate) fn domain_range(&self, unit: usize, slot: u16) -> std::ops::RangeInclusive<u16> {
        let total = self.machine.iommus[unit].domains;
        let per = total.saturating_sub(2) / u32::from(self.table.nodes.max(1));
        let first = 1 + u32::from(slot) * per;
        // Empty when the unit has no number for each slot (the start-up
        // refuses such a layout).
        (first as u16)..=((first + per - 1) as u16)
    }

    /// What the holder's gate checks of a request's operands before it
    /// writes the unit (the table of operands, D-134): a device whose
    /// identifier word the requester holds and a domain number of its
    /// range; a context entry of its own; a handle, or an APIC ID, of a
    /// core whose word names it; the masking of a line it claimed.
    pub(crate) fn operands_allowed(&self, role: usize, op: UnitOp, from: (u16, u32)) -> bool {
        let (node, generation) = from;
        let names = |core: usize| {
            self.table
                .core_word(core)
                .is_some_and(|w| w.names(node, generation))
        };
        let handle_core = |handle: u16| usize::from(handle) / dodos_abi::x86_64::IRQ_TABLE_ENTRIES;
        let record = self.table.role_records[role];
        let unit =
            (record.governs == dodos_abi::Governs::IommuUnit).then_some(usize::from(record.object));
        let in_range =
            |domain: u16| unit.is_some_and(|u| self.domain_range(u, node).contains(&domain));
        let held = |device: u16| {
            unit.is_some_and(|u| {
                let key = u32::from(self.machine.iommus[u].segment) << 16 | u32::from(device);
                let raw = self.table.identifiers.get(&key).copied();
                raw.and_then(dodos_abi::Ownership::from_raw)
                    .is_some_and(|w| {
                        w.state() == dodos_abi::OwnState::Taken
                            && w.owner() == dodos_abi::Owner::Node(node)
                            && w.generation() == generation
                    })
            })
        };
        let state = self.table.units.get(&role);
        // The holder serves an operation only on a unit of its kind:
        // operations 1 to 4 on an IOMMU unit, 5 on a line controller (D-141).
        let iommu_op = !matches!(op, UnitOp::Redirection { .. });
        let kind_fits = match record.governs {
            dodos_abi::Governs::IommuUnit => iommu_op,
            governs => crate::table::governs_lines(governs) && !iommu_op,
        };
        if !kind_fits {
            return false;
        }
        match op {
            UnitOp::Attach {
                device,
                domain,
                root,
            } => {
                // G9: the attach word moves from *pinned* to *attached* with
                // that root, or reads *attached* with it; the root is an
                // iospace root with a device pinned, held by the node that
                // holds the device (D-137).
                let page = root / self.table.geometry.page();
                let class = self.table.classes.get(&page).copied();
                let word = self
                    .attach_key(role, device)
                    .map(|k| self.table.attach_word(k));
                let pinned = word.is_some_and(|w| {
                    w.root() == root
                        && matches!(
                            w.state(),
                            Some(dodos_abi::AttachState::Pinned | dodos_abi::AttachState::Attached)
                        )
                });
                held(device)
                    && in_range(domain)
                    && pinned
                    && class
                        .and_then(dodos_abi::PageClass::root_devices)
                        .is_some_and(|n| n >= 1)
                    && self.table.holds_page(root, node, generation)
            }
            // The identifier word names the requester with its generation,
            // as for an attach; the context entry names that domain number,
            // or none is left and the attach word already reads *detached*
            // with that root: a detach served again (D-140).
            UnitOp::Detach {
                device,
                domain,
                root,
            } => {
                let entry = state.and_then(|u| u.entries.get(&device).map(|&o| (o, u)));
                held(device)
                    && match entry {
                        Some((owner, u)) => {
                            owner == from && u.domains.get(&device).is_none_or(|&d| d == domain)
                        }
                        None => self.attach_key(role, device).is_some_and(|k| {
                            Some(self.table.attach_word(k))
                                == dodos_abi::AttachWord::new(
                                    dodos_abi::AttachState::Detached,
                                    root,
                                )
                        }),
                    }
            }
            UnitOp::Invalidate { domain } => domain.is_some_and(in_range),
            UnitOp::InterruptCache { handle } => names(handle_core(handle)),
            // Masked: operand 2 is 0, and the line's word is free or names
            // the requester (D-135).
            UnitOp::Redirection {
                claimed: false,
                gsi,
                target,
                ..
            } => {
                // With the generation the word holds, as for a claim: a word
                // a generation that is over took is not the requester's
                // (D-141; the final review's verification of lot C, H16).
                let word = self.table.line_word(gsi);
                let free = word.state() == dodos_abi::OwnState::Free;
                let names =
                    word.owner() == dodos_abi::Owner::Node(node) && word.generation() == generation;
                target == crate::rings::Target::Masked && (free || names)
            }
            // Claimed: *release* is 0 (bit 35 is masked only), the line's word
            // names the requester with its generation (D-135), and the target
            // a core whose word names it.
            UnitOp::Redirection { release: true, .. } => false,
            UnitOp::Redirection { gsi, .. }
                if self.table.line_word(gsi)
                    != dodos_abi::Ownership::taken(dodos_abi::Owner::Node(node), generation) =>
            {
                false
            }
            UnitOp::Redirection { target, .. } => match target {
                crate::rings::Target::Remapped { handle, .. } => names(handle_core(handle)),
                crate::rings::Target::Direct { apic, .. } => self
                    .machine
                    .core_list()
                    .iter()
                    .position(|c| c.hw_id == u64::from(apic))
                    .is_some_and(names),
                crate::rings::Target::Masked => false,
            },
        }
    }

    /// The entries of the unit request table, served oldest first: an entry
    /// is sent when the role word reads held by a live node and a request
    /// credit toward that node is free, with the epoch the word then holds;
    /// an entry for a role this node holds is submitted to the unit with no
    /// message.
    pub(crate) fn unit_send_queued(&mut self) {
        if self.seal == Some(crate::nodes::SealStep::Parking) {
            return;
        }
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
            // A requester answered *elsewhere* keeps the request queued
            // while the word reads the epoch it sent, and sends it once
            // the word does (Roles, D-141; pass 25: it sent it again at
            // once to the node that hands, at every step, for ever). Every
            // end of a hand-over changes the epoch, an abandoned one too
            // (D-144).
            if entry.elsewhere && epoch == entry.epoch {
                continue;
            }
            if holder == self.node {
                if !self.serves_role(entry.role) {
                    continue;
                }
                if self.unit_failed(entry.role) {
                    self.unit_complete(index, UnitResult::Failed);
                    continue;
                }
                let submitted = self.submit(entry.role, entry.op);
                let at = self.membership_now();
                let e = self.unit_requests[index].as_mut().expect("queued");
                (e.epoch, e.stage) = (epoch, Stage::Local { submitted, at });
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
                entry: index as u8,
                serial: self.table.unit_records[usize::from(self.node)][index].serial,
            };
            let completes = Completes::Unit { entry: index };
            if let Ok(Some((credit, _))) = self.write_message(holder, body, Some(completes), None) {
                let e = self.unit_requests[index].as_mut().expect("queued");
                (e.epoch, e.stage) = (epoch, Stage::Sent { to: holder, credit });
                e.elsewhere = false;
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
        // The instant of a submission is on the clock of membership, as
        // every unit wait (Two builds, and the debugger stub; the final
        // review's verification of lot C, O13).
        let now = self.membership_now();
        let word = self.table.roles[role];
        self.submitted.0.push((role, word, op));
        // The holder's gate writes the attach word *attached* before the
        // context entry (D-137).
        if let UnitOp::Attach { device, root, .. } = op
            && let Some(key) = self.attach_key(role, device)
        {
            let attached = dodos_abi::AttachWord::new(dodos_abi::AttachState::Attached, root);
            let pinned = dodos_abi::AttachWord::new(dodos_abi::AttachState::Pinned, root);
            if let (Some(pinned), Some(attached)) = (pinned, attached) {
                self.table.swap_attach(key, pinned, attached);
            }
        }
        let unit = self.table.units.entry(role).or_default();
        unit.next += 1;
        let serial = unit.next;
        match op {
            UnitOp::Attach { device, domain, .. } => {
                unit.entries.insert(device, requester);
                unit.domains.insert(device, domain);
            }
            UnitOp::Detach { device, .. } => {
                unit.entries.remove(&device);
                unit.domains.remove(&device);
            }
            UnitOp::Redirection { gsi, claimed, .. } => {
                // RISC-V: the APLIC source's `sourcecfg` and target written
                // by the domain's holder: a claim activates the source, a
                // mask is `sourcecfg` 0 (D-147).
                if self.machine.arch == crate::Arch::Riscv64 {
                    let line = self.table.aplic.entry(gsi).or_default();
                    match claimed {
                        true => line.active = true,
                        false => line.deactivate(),
                    }
                }
                unit.cut_lines.retain(|&(l, _, _)| l != gsi);
                match claimed {
                    true => {
                        unit.masked.remove(&gsi);
                        unit.lines.insert(gsi, requester)
                    }
                    false => {
                        unit.masked.remove(&gsi);
                        unit.lines.remove(&gsi)
                    }
                };
            }
            UnitOp::Invalidate { .. } | UnitOp::InterruptCache { .. } => {}
        }
        // A redirection entry, written through an index and a data
        // register, is not confirmed by the unit: answered at once.
        if !matches!(op, UnitOp::Redirection { .. }) {
            unit.queue.push((serial, now));
        }
        // A masked operation 5 with *release* (bit 35) writes the line word
        // *free*, unclaimed, through the holder's gate in the same call when
        // it reads taken by the requester (D-150).
        if let UnitOp::Redirection {
            gsi,
            claimed: false,
            release: true,
            ..
        } = op
            && self.table.line_word(gsi)
                == dodos_abi::Ownership::taken(dodos_abi::Owner::Node(requester.0), requester.1)
        {
            let read = crate::table::LineFree::Release {
                requester: requester.0,
                generation: requester.1,
            };
            self.table.write_line_free(self.node, gsi, read);
        }
        serial
    }

    /// Whether the unit has confirmed a submission. A unit that works
    /// confirms everything submitted up to the check, the same instant
    /// included: the queue holds only what `submit_for` wrote to the unit,
    /// and the contract bounds the unit's wait from above only
    /// (`IOMMU_WAIT_NS`), so a fast unit is a legal one (O20). A broken one
    /// confirms nothing.
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
        (role, epoch): (u16, u32),
        (entry, record): (u8, u64),
    ) {
        let Some((index, serial)) = credit else {
            return;
        };
        // The holder's gate reads the request record, never the ring, and
        // answers only a record whose state word reads the serial the
        // message names with *open*: a message that names another serial
        // changes nothing (Devices and shared hardware, the request record;
        // D-151, D-160). The operation and its operands are the record's;
        // the message's operation is a copy for the trace. The kernel
        // judges the role word of the message's role and epoch; the gate
        // compares that role with the record's word 0 when it serves the
        // message (D-162, below).
        let entry = usize::from(entry);
        let Some(opened) = self.table.open_record(from, entry, record) else {
            return;
        };
        let Some(op) = opened.op else {
            return;
        };
        let role = role as usize;
        let word = self.table.roles.get(role).copied();
        let held = word.is_some_and(|w| {
            w.state() == RoleState::Held && w.holder() == self.node && w.epoch() == epoch
        });
        let answer = |model: &mut Self, result| {
            model.answer_unit(from, credit, (entry, record), result);
        };
        // The holder of a failed unit answers *failed*, during a hand-over
        // too: the answer is final, no requester is turned away (D-145).
        if held && self.unit_failed(role) {
            return answer(self, UnitResult::Failed);
        }
        if !held || self.handing.contains_key(&role) {
            return answer(self, UnitResult::Elsewhere);
        }
        // The holder's gate refuses operands that name what the requester
        // does not hold: nothing is written, the refusal is traced (code
        // 11), and the request is answered done (D-134). A record whose
        // word 0 names another role than the message fails its checks as
        // well (What crosses between nodes, row 4; D-162).
        let requester = (from, self.table.generation(from));
        if opened.role != role || !self.operands_allowed(role, op, requester) {
            // (*s*, *open*) → (*s*, *refused*): the checks fail (Gate
            // words, the request record).
            let refused = crate::table::RecordState::Refused;
            self.table.answer_unit_record(from, entry, record, refused);
            self.refused.0 += 1;
            self.trace(TraceCode::Refused, None, [u64::from(from), 4, role as u64]);
            return answer(self, UnitResult::Done);
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
                Some((serial, self.membership_now()))
            }
        };
        self.unit_service.insert(
            (from, index),
            Served {
                role,
                op,
                serial,
                record: (entry, record),
                submitted,
            },
        );
    }

    /// A `UnitRequest` of `from` received by this holder, for the tests:
    /// the requester's gate first opens a free record of its table that
    /// names `op` (D-151), which the message then names (D-160).
    #[cfg(test)]
    pub(crate) fn unit_requested_with_record(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        (role, epoch, op): (u16, u32, UnitOp),
    ) {
        let records = &self.table.unit_records[usize::from(from)];
        let open = |r: &&crate::table::UnitRecord| r.state == crate::table::RecordState::Open;
        let entry = records
            .iter()
            .position(|r| !open(&r))
            .expect("a free entry");
        let serial = self
            .table
            .open_unit_record(from, entry, usize::from(role), op);
        self.unit_requested(from, credit, (role, epoch), (entry as u8, serial));
    }

    /// The holder answers a request of `peer`: its gate answers the record
    /// from (*s*, *open*) at a done or a *failed* (Gate words, the request
    /// record), then its kernel writes the `UnitAnswer`; *elsewhere* leaves
    /// the record *open*, for the node that serves it.
    fn answer_unit(
        &mut self,
        peer: u16,
        credit: Option<(u8, u32)>,
        (entry, serial): (usize, u64),
        result: UnitResult,
    ) {
        if let Some(state) = record_state(result) {
            self.table.answer_unit_record(peer, entry, serial, state);
        }
        let _ = self.write_message(peer, Body::UnitAnswer { result }, None, credit);
    }

    /// A `UnitAnswer` received by the requester, applied in the event that
    /// consumes it whether or not a thread still waits: done completes the
    /// entry, *failed* ends it with its counts kept, *elsewhere* queues it
    /// again, sent once the role word reads held by a live node with
    /// another epoch than the one it was sent with.
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
                    e.elsewhere = true;
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
        // The record no longer reads *open*: answered by the holder's gate,
        // this node's own for a role it holds (the model keeps the answer
        // in the message for another holder; D-151).
        let serial = self.table.unit_records[usize::from(self.node)][index].serial;
        if let Some(state) = record_state(result) {
            self.table
                .answer_unit_record(self.node, index, serial, state);
        }
        // A claim goes on to its next unit, or to its line's redirection
        // entry, at a done or a *failed* alike (gap 12, D-134).
        let failed = result == UnitResult::Failed;
        if result != UnitResult::Elsewhere && self.next_unit(index, entry, failed) {
            return;
        }
        let result = match entry.failed {
            true => UnitResult::Failed,
            false => result,
        };
        // The removal of an irq whose invalidation of the interrupt entry
        // cache a unit answered *failed*: its handle and vector are not
        // reused while the unit is failed (D-139).
        if let UnitOp::InterruptCache { handle } = entry.op
            && result == UnitResult::Failed
            && self
                .irqs
                .values()
                .any(|irq| irq.removing && self.handle_of(irq) == handle)
        {
            self.table.retired.insert(handle);
        }
        // The attach word against every event (D-139): an attach or a
        // detach is judged by the word at its answer; one that did not
        // attach or detach ends its call with `busy`.
        let result = match entry.op {
            UnitOp::Attach { device, root, .. }
                if !self.attach_answered(entry.role, device, root, result) =>
            {
                UnitResult::Failed
            }
            UnitOp::Detach { device, .. }
                if !self.detach_answered(entry.role, device, result, entry.waiter) =>
            {
                UnitResult::Failed
            }
            _ => result,
        };
        let record = match entry.waiter {
            Waiter::Record(cap) => Some(cap),
            Waiter::Thread(_) | Waiter::Step => entry.record,
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
                self.detach_confirmed(entry.role, entry.op);
                self.unit_complete(index, UnitResult::Done);
                any = true;
            } else if self.unit_overdue(at) {
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
                self.detach_confirmed(entry.role, entry.op);
                UnitResult::Done
            } else if self.unit_overdue(at) {
                self.fail_unit(entry.role);
                continue;
            } else {
                continue;
            };
            self.unit_service.remove(&(peer, credit));
            self.answer_unit(peer, Some((credit, entry.serial)), entry.record, result);
            any = true;
        }
        let before: Vec<Option<UnitEntry>> = self.unit_requests.clone();
        self.unit_send_queued();
        any || before != self.unit_requests
    }

    /// The holder's gate, once the invalidation that follows the clearing of
    /// a detached device's context entry is confirmed: the attach word
    /// *attached* becomes *detached* (D-137).
    pub(crate) fn detach_confirmed(&mut self, role: usize, op: UnitOp) {
        let UnitOp::Detach { device, .. } = op else {
            return;
        };
        let Some(key) = self.attach_key(role, device) else {
            return;
        };
        let word = self.table.attach_word(key);
        if word.state() == Some(dodos_abi::AttachState::Attached) {
            let detached =
                dodos_abi::AttachWord::new(dodos_abi::AttachState::Detached, word.root());
            self.table.swap_attach(key, word, detached.expect("a root"));
        }
    }

    /// Whether a submission of `at` is past `IOMMU_WAIT_NS` on the clock of
    /// membership; never while the debugger stub's release word is raised,
    /// when every wait suspends (Two builds, and the debugger stub, D-135;
    /// the final review's verification of lot C, O13).
    pub(crate) fn unit_overdue(&self, at: u64) -> bool {
        !self.table.release && at + IOMMU_WAIT_NS <= self.membership_now()
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
        // The submissions of the units this node holds, R1's among them:
        // their failure is judged by time too (O13).
        let held = self
            .table
            .units
            .iter()
            .filter(|&(&role, _)| {
                let word = self.table.roles[role];
                word.state() == dodos_abi::RoleState::Held && word.holder() == self.node
            })
            .flat_map(|(_, unit)| unit.queue.iter().map(|&(_, at)| at));
        own.chain(served)
            .chain(held)
            .min()
            .map(|at| at + IOMMU_WAIT_NS)
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
            let credit = Some((credit, entry.serial));
            self.answer_unit(peer, credit, entry.record, UnitResult::Failed);
        }
    }

    /// R0 for a node `peer` that held a role: the entries sent to it are
    /// queued again and their credits freed (the credits are freed by R0
    /// itself); the entries of its own requests this node serves are
    /// dropped, with no answer.
    pub(crate) fn unit_r0(&mut self, peer: u16) {
        for index in 0..UNIT_REQUESTS {
            let Some(entry) = self.unit_requests[index].as_mut() else {
                continue;
            };
            if !matches!(entry.stage, Stage::Sent { to, .. } if to == peer) {
                continue;
            }
            // A record the holder's gate answered before it died ends on
            // its state word, which the requester's gate reads (Devices and
            // shared hardware, the request record, D-151): sent again, no
            // holder would answer a record no longer *open*.
            let state = self.table.unit_records[usize::from(self.node)][index].state;
            match state {
                crate::table::RecordState::Done | crate::table::RecordState::Refused => {
                    self.unit_complete(index, UnitResult::Done);
                }
                crate::table::RecordState::Failed => self.unit_complete(index, UnitResult::Failed),
                _ => entry.stage = Stage::Queued,
            }
        }
        self.unit_service.retain(|&(from, _), _| from != peer);
    }
}

/// The state the holder's gate writes in a request record's state word for
/// an answer (Gate words, the request record): none for *elsewhere*, which
/// leaves the record *open* for the node that serves it. A refusal of the
/// checks writes *refused*, answered done to the kernel (D-134).
fn record_state(result: UnitResult) -> Option<crate::table::RecordState> {
    match result {
        UnitResult::Done => Some(crate::table::RecordState::Done),
        UnitResult::Failed => Some(crate::table::RecordState::Failed),
        UnitResult::Elsewhere => None,
    }
}
