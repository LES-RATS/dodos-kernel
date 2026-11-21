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
    RING_KERNEL_ENTRIES, RING_USER_ENTRIES, RING_USER_STEP, Ring, RingHeader, SENDS_PER_PEER,
    TraceCode, UnitResult, message_entries, ring_fits, ring_resync,
};

/// The operation of a unit request, idempotent (Devices and shared
/// hardware).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOp {
    /// The context entry of a device points at the tables of an iospace of
    /// the requesting node's generation, whose root table is at `root`,
    /// under the domain number of that iospace on the unit.
    Attach { device: u16, domain: u16, root: u64 },
    /// The context entry of a device, under that domain number, is cleared;
    /// `root` is the root table the requester's header entry names (operand
    /// 2 since D-140).
    Detach { device: u16, domain: u16, root: u64 },
    /// The unit's translation cache is invalidated (after an unmap or the
    /// unhooking of a table in an iospace that serves devices): for one
    /// domain number, or the whole unit (`None`, the holder's own: R1, a
    /// rebuild).
    Invalidate { domain: Option<u16> },
    /// The interrupt entry cache is invalidated for one remapping handle,
    /// after its entry is written (index-selective, mask 0).
    InterruptCache { handle: u16 },
    /// A line's redirection entry in an IOAPIC: claimed by the requester
    /// (with its trigger, its polarity and its target), or masked.
    Redirection {
        gsi: u32,
        claimed: bool,
        level: bool,
        low: bool,
        target: Target,
    },
}

/// Where a line's redirection entry points: a remapping handle and the
/// vector of its remapping entry; without remapping a vector and an APIC
/// ID; nothing for a masked entry (Devices and shared hardware, the
/// operands of each operation, D-134).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Remapped { handle: u16, vector: u8 },
    Direct { vector: u8, apic: u32 },
    Masked,
}

/// One item of a `Report`: a device the asked node attached through the
/// unit (its identifier and the root its words name), a remapping handle
/// it claimed, or a line with the operands of its operation 5 claimed (its
/// trigger, its polarity and its target: D-137, the new holder rewrites
/// the line's entry from them) and its mask state, bit 32 of the first
/// word: unmasked only while a level line's irq is armed, or an edge
/// line's irq is not dying (D-139).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {
    Device(u16, u64),
    Handle(u32),
    Line(u32, bool, bool, Target, bool),
}

impl Item {
    /// The key of an item in the fixed order of a report: its kind in bits
    /// 61-63 (0 a device, 1 a remapping handle, 2 a line; the ITS kinds 3
    /// and 4 are not modelled), its identifier below, so keys compare as
    /// numbers in that order (the words of a `Report`, D-141).
    pub(crate) fn key(self) -> u64 {
        use dodos_abi::{REPORT_KIND_DEVICE, REPORT_KIND_HANDLE, REPORT_KIND_LINE};
        let (kind, id) = match self {
            Item::Device(d, _) => (REPORT_KIND_DEVICE, u64::from(d)),
            Item::Handle(h) => (REPORT_KIND_HANDLE, u64::from(h)),
            Item::Line(l, ..) => (REPORT_KIND_LINE, u64::from(l)),
        };
        kind << dodos_abi::REPORT_KIND_SHIFT | id
    }
}

/// The items of one `Report` at most, so that it fits eleven entries.
pub(crate) const REPORT_ITEMS: usize = 28;

/// What a message carries after its header: the words of the contract's
/// table by kind (What crosses between nodes, gap 3), which
/// `dodos_abi::NodeMessage` documents.
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
    /// The role, the epoch and the operation (a copy for the holder's
    /// trace), the entry of the requester's unit request table and the
    /// serial of its state word; no operand (D-160).
    UnitRequest {
        role: u16,
        epoch: u32,
        op: UnitOp,
        entry: u8,
        serial: u64,
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
            // The number of items, the next key, then three words each
            // (D-141).
            Body::Report { items, .. } => 2 + 3 * items.len(),
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
    /// A `Send`: its grant entries, each with the serial the send wrote
    /// it with, the thread that waits for the answer (none once
    /// destroyed), and whether the call's `node` capability had `manage`.
    Send {
        grants: Vec<(u16, u32)>,
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
    pub(crate) fn free(&self, send: bool) -> Option<u8> {
        let range = match send {
            false => 0..REQUESTS_PER_PEER,
            true => REQUESTS_PER_PEER..REQUESTS_PER_PEER + SENDS_PER_PEER,
        };
        range
            .into_iter()
            .find(|&i| self.credits[i].is_none())
            .map(|i| i as u8)
    }

    /// The sends in flight.
    pub(crate) fn sends(&self) -> usize {
        self.credits[REQUESTS_PER_PEER..].iter().flatten().count()
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
    /// The receiver's member word reads starting by a fresh start: nothing
    /// but an answer is written toward it until it reads live (D-144).
    Starting,
}

impl Model {
    /// The ring pair `from` → `to`.
    pub(crate) fn ring_of(&self, from: u16, to: u16, user: bool) -> &crate::table::Ring {
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
        // No node writes into a ring toward a node whose member word reads
        // starting with the resume bit clear, until it reads live: a fresh
        // start sets its consumer indices to the producers', which would
        // drop what was written meanwhile. A message for it waits in its
        // sender, as one for a sealed node does; an answer to a message the
        // starting generation sent is written, since it sends only after
        // its reset. A resuming node kept its indices and is written to as
        // a live node (What crosses between nodes, a fresh start, D-144: a
        // rebuild's `ReportAsk` was skipped, and its credit never answered).
        let member = self.table.member(to);
        if member.state() == MemberState::Starting && !member.resuming() && !kind.is_answer() {
            return Err(Full::Starting);
        }
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
    /// one function and before anything changes, on a message read from the
    /// ring `from` → this node. `None` passes; `Some` names what it named,
    /// for the trace (code 11).
    pub(crate) fn refused(&self, from: u16, ring: Ring, message: &Message) -> Option<u64> {
        let header = &message.header;
        let sender = header.sender;
        // The sender the header names is the node whose ring the message
        // was read from: the gate maps that ring writable in that node's
        // kernel alone, so the ring says who wrote it (D-141).
        if sender != from || sender >= self.table.nodes {
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
        // The kind is one the ring carries: the kernel ring the kernels'
        // own kinds, the user ring kinds 11 and 12 (Two rings per ordered
        // pair of nodes). A kernel message read from the user ring is no
        // kind that ring knows.
        if message.body.kind().ring() != ring {
            return Some(0);
        }
        let generation = header.sender_generation;
        let me = self.node as usize;
        // A request names a credit of its kind's range (a `UnitRequest`
        // one whose entry of the unit service table is free), and its answer
        // fits in the free entries of the kernel ring toward its sender
        // (D-141).
        let kind = message.body.kind();
        let answer = NodeMessage::ALL
            .into_iter()
            .find(|k| k.answers() == Some(kind));
        if let Some(answer) = answer {
            let send = kind == NodeMessage::Send;
            let range = match send {
                false => 0..REQUESTS_PER_PEER,
                true => REQUESTS_PER_PEER..REQUESTS_PER_PEER + SENDS_PER_PEER,
            };
            let Some((index, _)) = header
                .credit()
                .filter(|(i, _)| range.contains(&(*i as usize)))
            else {
                return Some(u64::from(header.credit));
            };
            if matches!(message.body, Body::UnitRequest { .. })
                && self.unit_service.contains_key(&(sender, index))
            {
                return Some(u64::from(header.credit));
            }
            let most = match answer {
                NodeMessage::Report => dodos_abi::RING_MESSAGE_ENTRIES_MAX as u64,
                NodeMessage::SendAnswer => message_entries(1 + dodos_abi::SEND_GRANTS_MAX) as u64,
                _ => message_entries(2) as u64,
            };
            if !self.ring_room(sender, answer, most) {
                return Some(u64::from(header.credit));
            }
        }
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
            // A `ReportAsk` passes whatever its role, its epoch and its
            // sender; one whose role, epoch or holder is stale is answered
            // with an empty `Report` (*end*) so its credit is freed (D-167).
            Body::ReportAsk { .. } => None,
            // A `UnitRequest` passes whatever its epoch.
            Body::UnitRequest { .. } => None,
            // A `Send`'s stamp carries 0 (a sender attached to no task) or
            // a task identifier of the sender's node with a counter that is
            // not 0 (D-134).
            Body::Send {
                header: inner,
                stamp,
                grants,
                words,
            } => {
                let task = dodos_abi::TaskId::from_raw(stamp.0);
                if stamp.0 != 0 && (task.counter() == 0 || task.node() != sender) {
                    return Some(stamp.0);
                }
                // Its marks have no reserved bit, and its header is one a
                // local `send` accepts with no flag, with as many
                // capabilities as the grants it names (D-141).
                let local = Header::new(inner.label(), words.len(), grants.len(), 0);
                if dodos_abi::Marks::from_bits(stamp.1).is_err()
                    || local.ok() != Some(*inner)
                    || grants.len() > dodos_abi::SEND_GRANTS_MAX
                {
                    return Some(stamp.1);
                }
                // Each grant it names is an entry of the sender's grant
                // table, live, to this node with its current generation,
                // with that serial, not answered, with the rights stated,
                // and an anchor of depth 1 to 64; none is named twice.
                let mine = self.table.generation(self.node);
                let table = &self.table.grants[sender as usize];
                for (i, granted) in grants.iter().enumerate() {
                    let good = table.get(&granted.grant).is_some_and(|e| {
                        e.state == crate::table::GrantState::Live
                            && e.grantee == self.node
                            && e.generation == mine
                            && e.serial == granted.serial
                            && !e.answered
                            && e.anchor.rights == granted.rights
                            && (1..=dodos_abi::TREE_DEPTH).contains(&e.anchor.depth)
                    });
                    let twice = grants[..i].iter().any(|g| g.grant == granted.grant);
                    if !good || twice {
                        return Some(u64::from(granted.grant));
                    }
                }
                None
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
                if !in_use {
                    return Some(u64::from(header.credit));
                }
                // A `SendAnswer` gives one word per grant of the send it
                // answers, none of them a use index if nothing was
                // delivered (D-141).
                if let Body::SendAnswer {
                    delivered, uses, ..
                } = &message.body
                {
                    let (index, _) = header.credit().expect("in use");
                    let sent = match &self.peers[sender as usize].credits[index as usize] {
                        Some(Credit {
                            completes: Completes::Send { grants, .. },
                            ..
                        }) => grants.len(),
                        _ => usize::MAX,
                    };
                    if uses.len() != sent || (!delivered && uses.iter().any(Option::is_some)) {
                        return Some(u64::from(header.credit));
                    }
                }
                None
            }
        }
    }

    /// Consumes at most `step` messages of one ring into this node, each
    /// under the check; returns how many entries remain.
    pub(crate) fn consume(&mut self, from: u16, user: bool, step: u64) -> u64 {
        let size = if user {
            RING_USER_ENTRIES
        } else {
            RING_KERNEL_ENTRIES
        };
        let ring = match user {
            true => Ring::User,
            false => Ring::Kernel,
        };
        let me = self.node;
        let mut served = 0;
        while served < step {
            let produced = self.ring_of(from, me, user).produced;
            let mut consumed = self.consumed(from, me, user);
            // A producer index behind the consumer, or more than the
            // ring's entries ahead of it, moves the consumer (D-131, D-141;
            // `ring_resync`, in that order, D-160).
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
            let available = produced.wrapping_sub(consumed);
            let entry = self.ring_of(from, me, user).at(consumed).cloned();
            let (length, message) = match entry {
                Some(RingEntry::Message(m)) if m.header.readable(available) => {
                    (u64::from(m.header.length), Some(m))
                }
                // A header it cannot read is consumed as one entry.
                _ => (1, None),
            };
            // A message that would carry the step past its last entry waits
            // for the next interrupt: "at most 16 entries per ring per
            // interrupt" (Processing). The first message is always served,
            // so each interrupt makes progress.
            if served > 0 && served + length > step {
                self.set_consumed(from, user, consumed);
                break;
            }
            self.set_consumed(from, user, consumed.wrapping_add(length));
            // The step counts entries, as the contract counts them, not
            // messages (D-141).
            served += length;
            match message {
                Some(message) => match self.refused(from, ring, &message) {
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
    /// Whether the node reads its rings: once ready, and during a resume
    /// from the end of R0 on, while R2 to R5 go on (`resume_marks` is set
    /// then; What crosses between nodes, "At a resume", D-142).
    pub(crate) fn consumes(&self) -> bool {
        self.table.member(self.node).writes() && (self.ready || self.resume_marks.is_some())
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
    /// every message waiting, each a unit of the step. Once a sealing node
    /// has parked its other cores, each ring stops at a message whose step
    /// would open a request of a core or a unit request, leaving that
    /// message and what follows it in that ring until the resume (D-167).
    /// Returns whether one was consumed.
    pub(crate) fn serve_kernel_rings(&mut self, units: &mut usize) -> bool {
        if !self.consumes() {
            return false;
        }
        let mut any = false;
        let me = self.node;
        for from in (0..self.table.nodes).filter(|&f| f != me) {
            while *units > 0 && self.has_answerable_kernel_message(from) {
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

    /// Whether the kernel ring `from` → this node holds a message the node
    /// may still answer now (`node_stop` mode 1, D-167).
    pub(crate) fn has_answerable_kernel_message(&self, from: u16) -> bool {
        self.has_kernel_message(from) && !self.seal_waits_ring(from)
    }

    /// Whether the next unread message in the kernel ring `from` → this node
    /// would open a request of a core or a unit request, and so must wait
    /// (with what follows it in that ring) until the resume once the sealing
    /// node has parked its other cores (`node_stop` mode 1, D-167).
    fn seal_waits_ring(&self, from: u16) -> bool {
        if self.seal != Some(crate::nodes::SealStep::Parking) {
            return false;
        }
        let me = self.node;
        let ring = self.ring_of(from, me, false);
        let produced = ring.produced;
        let consumed = ring_resync(
            produced,
            self.consumed(from, me, false),
            RING_KERNEL_ENTRIES,
        );
        if produced == consumed {
            return false;
        }
        let available = produced.wrapping_sub(consumed);
        let Some(RingEntry::Message(message)) = ring.at(consumed) else {
            return false;
        };
        if !message.header.readable(available) {
            return false;
        }
        if self.refused(from, Ring::Kernel, message).is_some() {
            return false;
        }
        self.message_opens_request(message)
    }

    /// Whether a valid kernel message's step would open a request of a core
    /// or a unit request (a `Revoke` of a mapped frame).
    fn message_opens_request(&self, message: &Message) -> bool {
        let Body::Revoke { use_index, .. } = message.body else {
            return false;
        };
        let me = self.node as usize;
        let Some(&entry) = self.table.uses[me].get(&use_index) else {
            return false;
        };
        if entry.release_pending {
            return false;
        }
        let Some(root_obj) = entry.root.filter(|r| self.objects.contains_key(r)) else {
            return false;
        };
        let root_cap = self.objects[&root_obj].root;
        self.revoke_step_opens_request(root_cap)
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
            Body::UnitRequest {
                role,
                epoch,
                entry,
                serial,
                ..
            } => self.unit_requested(from, credit, (role, epoch), (entry, serial)),
            Body::UnitAnswer { result } => self.unit_answered(from, credit, result),
            Body::Handed { role, .. } => {
                self.handed_applied.0 += 1;
                self.handed(role as usize);
            }
            Body::ReportAsk { role, epoch, key } => {
                self.report_asked(from, credit, role, epoch, key)
            }
            Body::Report { next, items } => self.report_received(from, credit, next, items),
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
            // The granter's generation still running: starting, live,
            // sealing or sealed. A message held for it is sent once its word
            // no longer reads starting with the resume bit clear; a sealed
            // granter reads its ring at its resume (D-147: a release held for
            // a node that started fresh and sealed before the holder's next
            // step was dropped).
            let current = self.table.generation(entry.granter) == entry.generation
                && matches!(
                    self.table.member(entry.granter).state(),
                    MemberState::Starting
                        | MemberState::Live
                        | MemberState::Sealing
                        | MemberState::Sealed
                );
            if entry.release_pending {
                if !current {
                    self.free_use(index);
                    continue;
                }
                let body = Body::Release {
                    grant: entry.grant,
                    serial: entry.serial,
                };
                if self.write_message(entry.granter, body, None, None).is_ok() {
                    self.free_use(index);
                    sent += 1;
                }
                continue;
            }
            // A pending signal to a generation that is over died with it:
            // the flag is cleared, as R0 drops it (the final review's
            // verification of lot C, H6: it kept the reserve busy for ever).
            let over = self.table.generation(entry.granter) != entry.generation
                || !matches!(
                    self.table.member(entry.granter).state(),
                    MemberState::Starting
                        | MemberState::Live
                        | MemberState::Sealing
                        | MemberState::Sealed
                );
            if entry.signal_pending && over {
                if let Some(used) = self.table.uses[me].get_mut(&index) {
                    used.signal_pending = false;
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

    /// Whether, with no more than `PENDING_STEP` messages held, one is held
    /// toward a current granter that can be written to and has room: what
    /// `send_pending` sends (D-146).
    pub(crate) fn held_sendable(&self) -> bool {
        let me = self.node as usize;
        let pending: Vec<_> = self.table.uses[me]
            .values()
            .filter(|u| u.release_pending || u.signal_pending)
            .collect();
        pending.len() <= PENDING_STEP
            && pending.into_iter().any(|used| {
                let member = self.table.member(used.granter);
                let writable = member.generation() == used.generation
                    && (matches!(
                        member.state(),
                        MemberState::Live | MemberState::Sealing | MemberState::Sealed
                    ) || member.state() == MemberState::Starting && member.resuming());
                let kind = match used.release_pending {
                    true => NodeMessage::Release,
                    false => NodeMessage::Signal,
                };
                writable && self.ring_room(used.granter, kind, 1)
            })
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
    /// before (What crosses between nodes, G13); nothing but an answer was
    /// written toward it while it was starting (D-144).
    pub(crate) fn skip_rings(&mut self) {
        for from in 0..self.table.nodes {
            for user in [false, true] {
                let produced = self.ring_of(from, self.node, user).produced;
                self.set_consumed(from, user, produced);
            }
        }
    }
}
