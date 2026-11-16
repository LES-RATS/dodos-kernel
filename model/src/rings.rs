//! What crosses between nodes (D-128, D-129, D-131): the messages of the
//! two rings of each ordered pair of node slots, the request and send
//! credits of a node toward each peer, the write of a message with the
//! room each kind needs, and the one check on every message received.
//!
//! The rings are in the machine table (kept memory): the producer index in
//! the ring's header, written by the sender only; the consumer index in the
//! receiver's node beacon, written by the receiver only. The kernel ring is
//! served in the receiver's recovery reserve, the user ring at the
//! interrupt the sender sends when it goes from empty to not empty, at most
//! `RING_USER_STEP` entries per interrupt.

use crate::table::{RingEntry, Watched};
use crate::{Model, ObjId};
use dodos_abi::{
    Header, MemberState, NodeMessage, PENDING_STEP, REQUESTS_PER_PEER, RING_ANSWER_RESERVE,
    RING_KERNEL_ENTRIES, RING_USER_ENTRIES, RING_USER_STEP, RingHeader, SENDS_PER_PEER, TraceCode,
    UnitResult, message_entries, ring_fits, ring_resync,
};

/// The operation of a unit request, idempotent (Devices and shared
/// hardware).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOp {
    /// The context entry of a device points at the tables of an iospace of
    /// the requesting node's generation.
    Attach { device: u16 },
    /// The context entry of a device is cleared.
    Detach { device: u16 },
    /// The unit's translation cache is invalidated (after an unmap or the
    /// unhooking of a table in an iospace that serves devices).
    Invalidate,
    /// The interrupt entry cache is invalidated after a remapping entry.
    InterruptCache,
    /// A line's redirection entry in an IOAPIC: claimed by the requester,
    /// or masked.
    Redirection { gsi: u32, claimed: bool },
}

/// One item of a `Report`: a device the asked node attached through the
/// unit (its identifier), a remapping handle it claimed, or a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {
    Device(u16),
    Handle(u32),
    Line(u32),
}

impl Item {
    /// The key of an item in the fixed order of a report: the devices by
    /// identifier, then the handles, then the lines.
    pub(crate) fn key(self) -> u64 {
        match self {
            Item::Device(d) => u64::from(d),
            Item::Handle(h) => 1 << 32 | u64::from(h),
            Item::Line(l) => 2 << 32 | u64::from(l),
        }
    }
}

/// The items of one `Report` at most, so that it fits eleven entries.
pub(crate) const REPORT_ITEMS: usize = 28;

/// What a message carries after its header (CONTRACT-GAP 3: the words are
/// those `dodos_abi::NodeMessage` documents).
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Revoke {
        grant: u16,
        serial: u32,
        use_index: u16,
    },
    RevokeStep {
        removed: bool,
    },
    Release {
        grant: u16,
        serial: u32,
    },
    UnitRequest {
        role: u16,
        epoch: u32,
        op: UnitOp,
    },
    UnitAnswer {
        result: UnitResult,
    },
    Handed {
        role: u16,
        epoch: u32,
    },
    ReportAsk {
        role: u16,
        epoch: u32,
        key: u64,
    },
    Report {
        next: u64,
        items: Vec<Item>,
    },
    Ping,
    SendAnswer {
        delivered: bool,
        dropped: bool,
        uses: Vec<Option<u16>>,
    },
    Send {
        header: Header,
        stamp: (u64, u64),
        grants: Vec<crate::grants::Granted>,
        words: Vec<u64>,
    },
    Signal {
        grant: u16,
        serial: u32,
    },
}

impl Body {
    pub fn kind(&self) -> NodeMessage {
        match self {
            Body::Revoke { .. } => NodeMessage::Revoke,
            Body::RevokeStep { .. } => NodeMessage::RevokeStep,
            Body::Release { .. } => NodeMessage::Release,
            Body::UnitRequest { .. } => NodeMessage::UnitRequest,
            Body::UnitAnswer { .. } => NodeMessage::UnitAnswer,
            Body::Handed { .. } => NodeMessage::Handed,
            Body::ReportAsk { .. } => NodeMessage::ReportAsk,
            Body::Report { .. } => NodeMessage::Report,
            Body::Ping => NodeMessage::Ping,
            Body::SendAnswer { .. } => NodeMessage::SendAnswer,
            Body::Send { .. } => NodeMessage::Send,
            Body::Signal { .. } => NodeMessage::Signal,
        }
    }

    /// The words after the header.
    fn words(&self) -> usize {
        match self {
            Body::Revoke { .. } | Body::RevokeStep { .. } | Body::Release { .. } => 1,
            Body::Signal { .. } | Body::Handed { .. } => 1,
            Body::UnitRequest { .. } => 3,
            Body::UnitAnswer { .. } | Body::ReportAsk { .. } => 2,
            Body::Report { items, .. } => 1 + 3 * items.len(),
            Body::Ping => 0,
            Body::SendAnswer { uses, .. } => 1 + uses.len(),
            Body::Send { grants, words, .. } => 3 + grants.len() + words.len(),
        }
    }

    /// The entries the message takes in its ring.
    pub(crate) fn entries(&self) -> u64 {
        message_entries(self.words()) as u64
    }
}

/// One message as its first entry holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub header: RingHeader,
    pub body: Body,
}

/// What the answer to a request completes, recorded with its credit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Completes {
    /// A `Revoke`: the grant.
    Revoke { grant: u16 },
    /// A `UnitRequest`: its entry of the unit request table.
    Unit { entry: usize },
    /// A `ReportAsk`: the unit role being rebuilt.
    Report { role: usize },
    /// A `Send`: its grant entries, the thread that waits for the answer
    /// (none once destroyed), and whether the call's `node` capability had
    /// `manage`.
    Send {
        grants: Vec<u16>,
        thread: Option<ObjId>,
        manage: bool,
    },
}

/// A credit in use: from the moment its message is written into the ring
/// until the sender has consumed its answer, or the peer's generation ends.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Credit {
    pub(crate) kind: NodeMessage,
    pub(crate) serial: u32,
    pub(crate) completes: Completes,
}

/// A node's credits toward one peer: `REQUESTS_PER_PEER` request credits
/// (indices 0 to 15 of the header's credit field) and `SENDS_PER_PEER` send
/// credits (16 to 23), and the serial of the next use.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Peer {
    pub(crate) credits: Vec<Option<Credit>>,
    pub(crate) serial: u32,
}

impl Peer {
    pub(crate) fn new() -> Self {
        Self {
            credits: vec![None; REQUESTS_PER_PEER + SENDS_PER_PEER],
            serial: 0,
        }
    }

    /// A free credit of the kind: a request credit, or a send credit.
    fn free(&self, send: bool) -> Option<u8> {
        let range = match send {
            false => 0..REQUESTS_PER_PEER,
            true => REQUESTS_PER_PEER..REQUESTS_PER_PEER + SENDS_PER_PEER,
        };
        range
            .into_iter()
            .find(|&i| self.credits[i].is_none())
            .map(|i| i as u8)
    }

    /// The sends in flight, and those through a capability without
    /// `manage`.
    pub(crate) fn sends(&self) -> (usize, usize) {
        let sends = self.credits[REQUESTS_PER_PEER..].iter().flatten();
        let all: Vec<&Credit> = sends.collect();
        let plain = all
            .iter()
            .filter(|c| matches!(c.completes, Completes::Send { manage: false, .. }))
            .count();
        (all.len(), plain)
    }
}

/// Why a write found no room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Full {
    /// The ring has no room for the message (beyond the reserve of answers
    /// for a kernel message that is not an answer).
    Ring,
    /// No credit of the kind is free toward the peer.
    Credit,
}

impl Model {
    /// The ring pair `from` → `to`.
    fn ring_of(&self, from: u16, to: u16, user: bool) -> &crate::table::Ring {
        let pair = &self.table.rings[from as usize][to as usize];
        if user { &pair.user } else { &pair.kernel }
    }

    /// The consumer index of a ring into `to`, in `to`'s node beacon.
    pub(crate) fn consumed(&self, from: u16, to: u16, user: bool) -> u64 {
        let pair = self.table.beacons[to as usize].consumed[from as usize];
        if user { pair.1 } else { pair.0 }
    }

    /// Whether a message of `kind` taking `entries` entries fits in the ring
    /// to `to`: a kernel message that is not an answer leaves the reserve
    /// of answers free; an answer always has room.
    pub(crate) fn ring_room(&self, to: u16, kind: NodeMessage, entries: u64) -> bool {
        let user = kind.ring() == dodos_abi::Ring::User;
        let (size, reserve) = match (user, kind.is_answer()) {
            (true, _) => (RING_USER_ENTRIES, 0),
            (false, true) => (RING_KERNEL_ENTRIES, 0),
            (false, false) => (RING_KERNEL_ENTRIES, RING_ANSWER_RESERVE),
        };
        let produced = self.ring_of(self.node, to, user).produced;
        ring_fits(
            produced,
            self.consumed(self.node, to, user),
            size,
            entries,
            reserve,
        )
    }

    /// Writes a message into the ring to `to`, with plain stores (not gate
    /// calls: the receiver checks every message). A request or a send takes
    /// a credit, which records what its answer completes. The receiver is
    /// sent the ring's interrupt when the ring goes from empty to not empty.
    pub(crate) fn write_message(
        &mut self,
        to: u16,
        body: Body,
        completes: Option<Completes>,
        answered: Option<(u8, u32)>,
    ) -> Result<Option<(u8, u32)>, Full> {
        let kind = body.kind();
        let entries = body.entries();
        if !self.ring_room(to, kind, entries) {
            return Err(Full::Ring);
        }
        let user = kind.ring() == dodos_abi::Ring::User;
        let credit = match completes {
            Some(completes) => {
                let peer = &mut self.peers[to as usize];
                let index = peer.free(kind == NodeMessage::Send).ok_or(Full::Credit)?;
                peer.serial = (peer.serial + 1) & dodos_abi::CREDIT_SERIAL_MASK;
                let serial = peer.serial;
                peer.credits[index as usize] = Some(Credit {
                    kind,
                    serial,
                    completes,
                });
                Some((index, serial))
            }
            None => answered,
        };
        let me = (self.node, self.table.generation(self.node));
        let header = RingHeader::new(kind, entries as u8, me, self.table.generation(to), credit);
        let was_empty =
            self.ring_of(self.node, to, user).produced == self.consumed(self.node, to, user);
        let pair =
            &mut std::sync::Arc::make_mut(&mut self.table.rings)[self.node as usize][to as usize];
        let ring = if user {
            &mut pair.user
        } else {
            &mut pair.kernel
        };
        let at = ring.produced;
        ring.set(at, RingEntry::Message(Message { header, body }));
        for i in 1..entries {
            ring.set(at + i, RingEntry::Continued);
        }
        ring.produced += entries;
        if was_empty {
            self.hw_out.push(crate::nodes::HwAction::Ring { node: to });
        }
        Ok(credit)
    }

    /// The credit of this node toward `peer` at `index`, if in use with
    /// `serial` and for `kind`.
    fn credit_in_use(&self, peer: u16, index: u8, serial: u32, kind: NodeMessage) -> bool {
        let credit = self.peers[peer as usize].credits.get(index as usize);
        credit.is_some_and(|c| {
            c.as_ref()
                .is_some_and(|c| c.serial == serial && c.kind == kind)
        })
    }

    /// Frees a credit and returns what its answer completes.
    pub(crate) fn take_credit(&mut self, peer: u16, index: u8) -> Option<Credit> {
        self.peers[peer as usize].credits[index as usize].take()
    }

    /// The one check on every message received (D-124, D-129, D-131), in
    /// one function and before anything changes. `None` passes; `Some`
    /// names what it named, for the trace (code 11).
    pub(crate) fn refused(&self, message: &Message) -> Option<u64> {
        let header = &message.header;
        let sender = header.sender;
        if sender >= self.table.nodes {
            return Some(0);
        }
        // The sender's member word is starting, live, sealing or sealed
        // with that generation: what a node wrote before its seal is
        // answered (D-131).
        let member = self.table.member(sender);
        let sending = matches!(
            member.state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing | MemberState::Sealed
        );
        if !sending || member.generation() != header.sender_generation {
            return Some(0);
        }
        // The receiver generation is this node's current one.
        if header.receiver_generation != self.table.generation(self.node) {
            return Some(0);
        }
        if message.body.kind() as u8 != header.kind {
            return Some(0);
        }
        let generation = header.sender_generation;
        let me = self.node as usize;
        // By kind.
        match &message.body {
            // A message that names a grant matches the entry it names: the
            // use entry naming the sender as granting node with that
            // generation and serial (on the holder).
            Body::Revoke {
                grant,
                serial,
                use_index,
            } => {
                let entry = self.table.uses[me].get(use_index);
                let matches = entry.is_some_and(|u| {
                    u.granter == sender
                        && u.grant == *grant
                        && u.serial == *serial
                        && u.generation == generation
                });
                (!matches).then_some(u64::from(*grant))
            }
            // The grant entry naming the sender as grantee with the
            // generation it records and that serial (on the granting node).
            Body::Release { grant, serial } | Body::Signal { grant, serial } => {
                let entry = self.table.grants[me].get(grant);
                let matches = entry.is_some_and(|g| {
                    g.grantee == sender && g.generation == generation && g.serial == *serial
                });
                (!matches).then_some(u64::from(*grant))
            }
            // A `Handed` names the role's current epoch, comes from the node
            // the role word names as holder, and names this node as target.
            Body::Handed { role, epoch } => {
                let word = self.table.roles.get(*role as usize);
                let matches = word.is_some_and(|w| {
                    w.state() == dodos_abi::RoleState::Handing
                        && w.epoch() == *epoch
                        && w.holder() == sender
                        && w.target() == self.node
                });
                (!matches).then_some(u64::from(*role))
            }
            // A `ReportAsk` names the role's current epoch and comes from
            // the node the role word names as holder.
            Body::ReportAsk { role, epoch, .. } => {
                let word = self.table.roles.get(*role as usize);
                let matches = word.is_some_and(|w| {
                    w.state() == dodos_abi::RoleState::Held
                        && w.epoch() == *epoch
                        && w.holder() == sender
                });
                (!matches).then_some(u64::from(*role))
            }
            // A `UnitRequest` passes whatever its epoch.
            Body::UnitRequest { .. } | Body::Ping => None,
            // A `Send`'s stamp carries a task identifier of the sender's
            // node.
            Body::Send { stamp, .. } => {
                let task = dodos_abi::TaskId::from_raw(stamp.0);
                (task.node() != sender).then_some(stamp.0)
            }
            // An answer names a credit of this node toward the sender that
            // is in use, with that serial and the kind of request it
            // answers.
            Body::RevokeStep { .. }
            | Body::UnitAnswer { .. }
            | Body::Report { .. }
            | Body::SendAnswer { .. } => {
                let kind = message.body.kind().answers().expect("an answer");
                let in_use = header
                    .credit()
                    .is_some_and(|(index, serial)| self.credit_in_use(sender, index, serial, kind));
                (!in_use).then_some(u64::from(header.credit))
            }
        }
    }

    /// Consumes at most `step` messages of one ring into this node, each
    /// under the check; returns how many entries remain.
    fn consume(&mut self, from: u16, user: bool, step: u64) -> u64 {
        let size = if user {
            RING_USER_ENTRIES
        } else {
            RING_KERNEL_ENTRIES
        };
        let me = self.node;
        let mut served = 0;
        while served < step {
            let produced = self.ring_of(from, me, user).produced;
            let mut consumed = self.consumed(from, me, user);
            // A producer index more than the ring's entries ahead first
            // moves the consumer index to the producer minus the entries.
            let start = ring_resync(produced, consumed, size);
            if start != consumed {
                self.refused.0 += 1;
                self.trace(TraceCode::Refused, None, [u64::from(from), 0, 0]);
                consumed = start;
            }
            if produced == consumed {
                self.set_consumed(from, user, consumed);
                return 0;
            }
            let available = produced - consumed;
            let entry = self.ring_of(from, me, user).at(consumed).cloned();
            let (length, message) = match entry {
                Some(RingEntry::Message(m)) if m.header.readable(available) => {
                    (u64::from(m.header.length), Some(m))
                }
                // A header it cannot read is consumed as one entry.
                _ => (1, None),
            };
            self.set_consumed(from, user, consumed + length);
            served += 1;
            match message {
                Some(message) => match self.refused(&message) {
                    None => self.receive_message(message),
                    Some(named) => {
                        self.refused.0 += 1;
                        let who =
                            u64::from(from) | u64::from(message.header.sender_generation) << 32;
                        self.trace(
                            TraceCode::Refused,
                            None,
                            [who, u64::from(message.header.kind), named],
                        );
                    }
                },
                None => {
                    self.refused.0 += 1;
                    self.trace(TraceCode::Refused, None, [u64::from(from), 0, 0]);
                }
            }
        }
        let produced = self.ring_of(from, me, user).produced;
        produced.wrapping_sub(self.consumed(from, me, user))
    }

    fn set_consumed(&mut self, from: u16, user: bool, at: u64) {
        let pair = &mut self.table.beacons[self.node as usize].consumed[from as usize];
        if user {
            pair.1 = at;
        } else {
            pair.0 = at;
        }
    }

    /// Whether this node consumes from its rings: a node whose own gate
    /// writes, past the steps of its start or resume. A sealed node sends
    /// and consumes nothing.
    pub(crate) fn consumes(&self) -> bool {
        self.table.member(self.node).writes() && self.ready
    }

    /// The user rings into this node, at the interrupt that signals them:
    /// at most 16 entries per ring; if entries remain, the core sends
    /// itself the interrupt again.
    pub(crate) fn serve_user_rings(&mut self) {
        if !self.consumes() {
            return;
        }
        let mut left = false;
        for from in 0..self.table.nodes {
            if from != self.node && self.consume(from, true, RING_USER_STEP) > 0 {
                left = true;
            }
        }
        if left {
            let at = *self.at;
            self.cores[at].ipis.insert(crate::Ipi::Ring);
        }
    }

    /// The kernel rings into this node, in a step of the recovery reserve:
    /// every message waiting, each a unit of the step. Returns whether one
    /// was consumed.
    pub(crate) fn serve_kernel_rings(&mut self, units: &mut usize) -> bool {
        if !self.consumes() {
            return false;
        }
        let mut any = false;
        let me = self.node;
        for from in (0..self.table.nodes).filter(|&f| f != me) {
            while *units > 0 && self.has_kernel_message(from) {
                self.consume(from, false, 1);
                *units -= 1;
                any = true;
            }
        }
        any
    }

    pub(crate) fn has_kernel_message(&self, from: u16) -> bool {
        self.ring_of(from, self.node, false).produced != self.consumed(from, self.node, false)
    }

    /// Whether any ring into this node holds something.
    pub(crate) fn rings_waiting(&self) -> bool {
        (0..self.table.nodes)
            .filter(|&f| f != self.node)
            .any(|from| {
                [false, true].into_iter().any(|user| {
                    self.ring_of(from, self.node, user).produced
                        != self.consumed(from, self.node, user)
                })
            })
    }

    /// A message that passed the check.
    fn receive_message(&mut self, message: Message) {
        let sender = self.table.member(message.header.sender);
        self.applied.0.push((message.header, sender));
        let from = message.header.sender;
        let credit = message.header.credit();
        match message.body {
            Body::Revoke {
                grant,
                serial,
                use_index,
            } => self.revoke_received(from, (grant, serial, use_index), credit),
            Body::RevokeStep { removed } => self.revoke_answered(from, credit, removed),
            Body::Release { grant, serial } => self.release_received(from, grant, serial),
            Body::UnitRequest { role, epoch, op } => {
                self.unit_requested(from, credit, (role, epoch, op))
            }
            Body::UnitAnswer { result } => self.unit_answered(from, credit, result),
            Body::Handed { role, .. } => self.handed(role as usize),
            Body::ReportAsk { role, epoch, key } => {
                self.report_asked(from, credit, role, epoch, key)
            }
            Body::Report { next, items } => self.report_received(from, credit, next, items),
            Body::Ping => {}
            Body::SendAnswer {
                delivered,
                dropped,
                uses,
            } => self.send_answered(from, credit, delivered, dropped, uses),
            Body::Send {
                header,
                stamp,
                grants,
                words,
            } => self.send_received(from, credit, header, stamp, grants, words),
            Body::Signal { grant, .. } => self.signal_received(grant),
        }
    }

    /// The other kernel messages wait in state that already exists until
    /// there is room beyond the reserve: the releases and signals pending
    /// in the use entries, sent in the order of the use table, at most 32
    /// per event.
    pub(crate) fn send_pending(&mut self) {
        let me = self.node as usize;
        let mut sent = 0;
        let uses: Vec<u16> = self.table.uses[me].keys().copied().collect();
        for index in uses {
            if sent >= PENDING_STEP {
                return;
            }
            let Some(entry) = self.table.uses[me].get(&index).copied() else {
                continue;
            };
            let current = self.table.writes(entry.granter)
                && self.table.generation(entry.granter) == entry.generation;
            if entry.release_pending {
                if !current {
                    self.table.uses[me].remove(&index);
                    continue;
                }
                let body = Body::Release {
                    grant: entry.grant,
                    serial: entry.serial,
                };
                if self.write_message(entry.granter, body, None, None).is_ok() {
                    self.table.uses[me].remove(&index);
                    sent += 1;
                }
                continue;
            }
            if entry.signal_pending && current {
                let body = Body::Signal {
                    grant: entry.grant,
                    serial: entry.serial,
                };
                if self.write_message(entry.granter, body, None, None).is_ok() {
                    if let Some(used) = self.table.uses[me].get_mut(&index) {
                        used.signal_pending = false;
                    }
                    sent += 1;
                }
            }
        }
    }

    /// The observation of a ring from this node to `to` that is not empty,
    /// for stuck evidence.
    pub(crate) fn ring_watch(&self, to: u16) -> Option<Watched> {
        [false, true].into_iter().find_map(|user| {
            let produced = self.ring_of(self.node, to, user).produced;
            let consumed = self.consumed(self.node, to, user);
            (produced != consumed).then_some(Watched::Ring { user, consumed })
        })
    }

    /// The consumer index of a ring from this node to `to`.
    pub(crate) fn ring_consumed(&self, to: u16, user: bool) -> u64 {
        self.consumed(self.node, to, user)
    }

    /// The rings into this node at a fresh start: every consumer index is
    /// set to its ring's producer index, so the node consumes nothing sent
    /// before (What crosses between nodes, G13).
    pub(crate) fn skip_rings(&mut self) {
        for from in 0..self.table.nodes {
            for user in [false, true] {
                let produced = self.ring_of(from, self.node, user).produced;
                self.set_consumed(from, user, produced);
            }
        }
    }
}
