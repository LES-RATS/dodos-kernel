//! Grants: frames between nodes (D-120, D-128, D-129, D-131). A slot never
//! names a slot of another node; the only thing of one node another node
//! can hold is a frame, through a grant that the granting node's gate
//! writes in its grant table. This file holds both sides: the granting
//! node (anchors, grant entries, `node_send`, revocation, the answers it
//! consumes) and the holder (remote frame capabilities, use entries, the
//! delivery of a `Send`, a `Revoke`'s step, the release, `grant_signal`).

use crate::rings::{Body, Completes};
use crate::table::{Anchor, GrantEntry, GrantState, UseEntry};
use crate::{
    Cap, CapId, Delivery, Frame, Message, Model, ObjId, Object, Outcome, Output, Place, Record,
    Slot, Transfer, Wait, live,
};
use dodos_abi::{
    Error, GRANTS, Header, MemberState, ObjectKind, Rights, SEND_GRANTS_MAX, SlotAddr,
};

/// A remote object on its holder: the grant its capabilities name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Remote {
    pub(crate) node: u16,
    pub(crate) grant: u16,
    pub(crate) serial: u32,
    pub(crate) use_entry: u16,
    /// The depth of the anchor: the root remote capability has it.
    pub(crate) depth: usize,
    /// The rights of the anchor: the root remote capability has them
    /// (invariant 3; ver2-D 4).
    pub(crate) rights: Rights,
}

/// A grant as a `Send` names it: the entry, its serial and the rights the
/// message states, which must be the anchor's. The frame, the rights and
/// the depth of the remote capability are read from the anchor in the
/// granting node's grant entry, never from the message (Grants, a remote
/// frame capability, D-141).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Granted {
    pub grant: u16,
    pub serial: u32,
    pub rights: Rights,
}

/// The index, in the kernel space of a node, of the anchor of grant `g`.
pub(crate) fn anchor_index(grant: u16) -> u32 {
    1 + u32::from(grant)
}

/// The outcome of a `node_send` from its answer.
fn sent(delivered: bool, dropped: bool) -> Outcome {
    match (delivered, dropped) {
        (true, false) => Ok(Output::Sent { flags: 0 }),
        (true, true) => Ok(Output::Sent {
            flags: Header::FLAG_CAPS_DROPPED,
        }),
        (false, _) => Err(Error::WouldWait),
    }
}

impl Model {
    /// The kernel space of a node: the anchors of its grants, one slot per
    /// entry of its grant table, made at start-up from no untyped.
    pub(crate) fn build_kernel_space(&mut self) {
        let slots = (anchor_index(GRANTS as u16) as u64 + 1)
            .next_multiple_of(u64::from(dodos_abi::CSPACE_SEGMENT_SLOTS));
        let space = ObjId(self.next_id);
        let root_place = Place {
            space,
            index: slots as u32 - 1,
        };
        let all = Rights::ALL.without(Rights::EXEC);
        self.create(ObjectKind::CSpace, slots, None, all, root_place);
        self.kernel_space = Some(space);
    }

    /// Whether a remote object is valid: its grant entry is live with its
    /// serial, and the granting node's member word names the generation
    /// recorded in the use entry "in a state starting, live, sealing or
    /// sealed" (Grants, a remote frame capability, gap 10): a generation
    /// that is over (accused, dead or started again) makes it invalid.
    pub(crate) fn valid(&self, remote: &Remote) -> bool {
        let entry = self.table.grants[remote.node as usize].get(&remote.grant);
        let used = self.table.uses[self.node as usize].get(&remote.use_entry);
        let member = self.table.member(remote.node);
        let current = matches!(
            member.state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing | MemberState::Sealed
        );
        entry.is_some_and(|e| e.state == GrantState::Live && e.serial == remote.serial)
            && used
                .is_some_and(|u| u.serial == remote.serial && member.generation() == u.generation)
            && current
    }

    /// The anchor of a grant, if its slot holds one.
    pub(crate) fn anchor(&self, grant: u16) -> Option<CapId> {
        let space = self.kernel_space?;
        match self.cspaces[&space].slots.get(anchor_index(grant) as usize) {
            Some(&Slot::Full(cap)) => Some(cap),
            _ => None,
        }
    }

    /// The grant whose anchor `cap` is, if it is one.
    pub(crate) fn grant_of(&self, cap: CapId) -> Option<u16> {
        let place = self.caps[&cap].place;
        let index = place.index.checked_sub(1)?;
        (Some(place.space) == self.kernel_space && (index as usize) < GRANTS)
            .then_some(index as u16)
    }

    /// The gate frees a grant entry; its serial grows, and it leaves the
    /// list of doorbells of its notify in one step.
    pub(crate) fn free_grant(&mut self, grant: u16) {
        let node = self.node as usize;
        if let Some(entry) = self.table.grants[node].remove(&grant) {
            self.table.add_grants(&entry.anchor.frame, -1);
            self.table.serials[node].insert(grant, next_serial(entry.serial));
            if let Some((notify, _)) = entry.doorbell
                && let Some(notify) = self.notifies.get_mut(&notify)
            {
                notify.doorbells.retain(|&g| g != grant);
            }
        }
    }

    /// Frees a grant and removes its anchor if the anchor has no
    /// descendant; otherwise the anchor stays, held by nobody, until an
    /// ancestor's removal reaches it.
    fn free_with_anchor(&mut self, grant: u16) {
        let anchor = self.anchor(grant);
        self.free_grant(grant);
        if let Some(anchor) = anchor
            && self.caps[&anchor].children.is_empty()
        {
            self.remove_leaf(anchor);
        }
    }

    /// Whether the generation a grant entry names for its grantee is over:
    /// the grantee is dead, or its member word names another generation.
    pub(crate) fn grantee_over(&self, entry: &GrantEntry) -> bool {
        let member = self.table.member(entry.grantee);
        member.state() == MemberState::Dead || member.generation() != entry.generation
    }

    /// `node_send` (Calls between nodes): the stamped send into another
    /// node's mailbox, which may grant the frames its message lists.
    pub(crate) fn node_send(
        &mut self,
        thread: ObjId,
        node: SlotAddr,
        message: Message,
        doorbell: SlotAddr,
    ) -> Outcome {
        // The node: slot, kind, `send` (and `grant` if the header lists
        // frames, judged with the capability the call acts on, as for every
        // message: Order of checks, step 2; "`send`, and `grant` when the
        // header lists frames", `node_send`, gap 9). `invalid` if it names the
        // caller's own node; `gone` if that node is not live.
        let need = match message.caps.is_empty() {
            true => Rights::SEND,
            false => Rights::SEND.union(Rights::GRANT),
        };
        let held = self.held(thread, node, ObjectKind::Node, need)?;
        let manage = held.rights.contains(Rights::MANAGE);
        let target = self.nodes[&held.object];
        if target == self.node {
            return Err(Error::Invalid);
        }
        if self.table.member(target).state() != MemberState::Live {
            return Err(Error::Gone);
        }
        // The doorbell, if not 0: slot, kind notify, `send`; a notify of the
        // caller's node.
        let bell = match doorbell.is_none() {
            true => None,
            false => {
                let held = self.held(thread, doorbell, ObjectKind::Notify, Rights::SEND)?;
                let (object, badge) = (held.object, held.badge);
                Some((object, if badge == 0 { 1 } else { badge }))
            }
        };
        // The header and the message as for `send`.
        message.check_header(self.has_page(thread))?;
        // The list: frames of the caller's own node, each derived with its
        // rights; a move, another kind or a remote frame is `invalid`.
        let mut listed: Vec<(CapId, Rights)> = Vec::new();
        for transfer in &message.caps {
            let Transfer::Derive(slot, rights) = *transfer else {
                let Transfer::Move(slot) = *transfer else {
                    unreachable!()
                };
                self.cap(thread, slot, Rights::READ)?;
                return Err(Error::Invalid);
            };
            let cap = self.cap(thread, slot, Rights::READ)?;
            let source = self.is(cap, ObjectKind::Frame, Rights::DERIVE)?;
            if self.remotes.contains_key(&source.object) {
                return Err(Error::Invalid);
            }
            let rights = Rights::from_bits(rights).ok_or(Error::Invalid)?;
            if !source.rights.contains(rights) {
                return Err(Error::Denied);
            }
            if listed.iter().any(|&(c, _)| c == cap) {
                return Err(Error::Invalid);
            }
            // A capability at depth 64 cannot be derived: its anchor would
            // be deeper than the tree allows (D-141).
            if self.depth(cap) >= dodos_abi::TREE_DEPTH {
                return Err(Error::Invalid);
            }
            listed.push((cap, rights));
        }
        // The gate writes an anchor only over memory its node holds (G3
        // (2), D-141): a refusal here is the gate's, before any entry.
        if !listed
            .iter()
            .all(|&(cap, _)| self.holds_frame(self.frames[&self.caps[&cap].object]))
        {
            return Err(Error::Busy);
        }
        // The state: the sends in flight to the node (two kept for a
        // capability with `manage`), the room in the user ring, the free
        // entries of the grant table.
        let (in_flight, _) = self.peers[target as usize].sends();
        let limit = match manage {
            true => dodos_abi::SENDS_PER_PEER,
            false => dodos_abi::SENDS_PER_PEER - dodos_abi::SENDS_RESERVED_FOR_MANAGE,
        };
        if in_flight >= limit {
            return Err(Error::Busy);
        }
        let header = Header::new(message.label, message.words.len(), listed.len(), 0)?;
        let placeholder = Granted {
            grant: 0,
            serial: 0,
            rights: Rights::NONE,
        };
        let probe = Body::Send {
            header,
            stamp: (0, 0),
            grants: vec![placeholder; listed.len()],
            words: message.words.clone(),
        };
        if !self.ring_room(target, dodos_abi::NodeMessage::Send, probe.entries()) {
            return Err(Error::Busy);
        }
        let me = self.node as usize;
        // An entry is free only once its anchor is gone too: an anchor
        // held by nobody keeps its entry used (Grants, release by the
        // holder), and the anchor a `RevokeStep` leaves stays until the
        // removal step's next call removes it (Revocation).
        let free: Vec<u16> = (0..GRANTS as u16)
            .filter(|&g| !self.table.grants[me].contains_key(&g) && self.anchor(g).is_none())
            .take(SEND_GRANTS_MAX)
            .collect();
        if free.len() < listed.len() {
            return Err(Error::Busy);
        }
        // Effect: the gate writes one grant entry per frame, then the
        // message goes into the user ring with the caller's stamp, and the
        // thread waits for the `SendAnswer`.
        let grants: Vec<Granted> = listed
            .iter()
            .zip(&free)
            .map(|(&(cap, rights), &grant)| self.make_grant(cap, rights, grant, target, bell))
            .collect();
        let (task, marks) = self.stamp_of(thread);
        let added = match message.flags & Header::FLAG_ADD_MARKS {
            0 => 0,
            _ => message.marks,
        };
        let body = Body::Send {
            header,
            stamp: (task, marks | added),
            grants: grants.clone(),
            words: message.words,
        };
        let completes = Completes::Send {
            grants: grants.iter().map(|g| (g.grant, g.serial)).collect(),
            thread: Some(thread),
            manage,
        };
        let credit = self
            .write_message(target, body, Some(completes), None)
            .ok()
            .flatten()
            .expect("the ring and the credits were judged");
        live(&mut self.threads, thread).wait = Wait::Node {
            peer: target,
            credit: credit.0,
        };
        self.wait_mark(crate::locks::WaitKind::Node);
        self.observe();
        Ok(Output::Waits)
    }

    /// The task identifier and effective marks of a thread.
    pub(crate) fn stamp_of(&self, thread: ObjId) -> (u64, u64) {
        let task = self.threads[&thread]
            .task
            .map_or(0, |t| self.tasks[&t].id.raw());
        (task, self.marks(thread).map_or(0, |m| m.bits()))
    }

    /// The gate writes a grant entry: the anchor is a new child of the
    /// frame capability with the rights asked and badge 0, the grantee the
    /// target's current generation, the doorbell the notify named.
    fn make_grant(
        &mut self,
        cap: CapId,
        rights: Rights,
        grant: u16,
        grantee: u16,
        doorbell: Option<(ObjId, u64)>,
    ) -> Granted {
        let space = self.kernel_space.expect("a kernel space");
        let place = Place {
            space,
            index: anchor_index(grant),
        };
        self.derive(cap, rights, 0, place);
        let anchor = *self.caps[&cap].children.last().expect("the anchor");
        let node = self.node as usize;
        let serial = self.table.serials[node].get(&grant).copied().unwrap_or(0);
        let generation = self.table.generation(grantee);
        let anchor_says = Anchor {
            frame: self.frames[&self.caps[&cap].object],
            rights,
            depth: self.depth(anchor),
        };
        self.table.add_grants(&anchor_says.frame, 1);
        self.table.grants[node].insert(
            grant,
            GrantEntry {
                anchor: anchor_says,
                grantee,
                generation,
                serial,
                state: GrantState::Live,
                use_index: None,
                answered: false,
                doorbell,
            },
        );
        if let Some((notify, _)) = doorbell {
            live(&mut self.notifies, notify).doorbells.push(grant);
        }
        Granted {
            grant,
            serial,
            rights,
        }
    }

    /// Whether this node holds every chunk of a frame, as the gate reads it
    /// before it writes an anchor (G3 (2)): a chunk taken by this node with
    /// its generation (with its own code, or a reserve's whose role it
    /// holds), its ordinary floor, a device entry it owns, or the global
    /// read-only set (a kept frame); and no chunk of it is a table chunk.
    pub(crate) fn holds_frame(&self, frame: Frame) -> bool {
        let me = self.node;
        let generation = self.table.generation(me);
        match self.leaf_target(frame) {
            crate::memory::Target::Nothing => frame.kept,
            crate::memory::Target::Entry(index) => {
                let word = self.table.devices[index].owner.raw();
                self.table.taken_by(word, me, generation)
            }
            crate::memory::Target::Chunks => {
                let chunk = self.machine.geometry.chunk();
                let end = frame.base + frame.pages * self.bytes(frame.page);
                // A table chunk, of the table floor or taken as kind 2, is
                // never granted (G6, D-156).
                (frame.base / chunk..end.div_ceil(chunk)).all(|c| {
                    !self.table.table_chunk(c) && self.table.holds_page(c * chunk, me, generation)
                })
            }
        }
    }

    /// The count of the holder's use entry under a grant of this node, read
    /// as a revocation reads it: counted only if the entry at the use index
    /// the grant entry records names this node, the grant, its serial and
    /// this node's generation, and 0 otherwise (Grants, D-141). The holder's
    /// gate counts a leaf only in that entry, and frees it only at 0.
    pub(crate) fn holder_count(&self, grant: u16, entry: &GrantEntry) -> u64 {
        let generation = self.table.generation(self.node);
        entry
            .use_index
            .and_then(|u| self.table.uses[entry.grantee as usize].get(&u))
            .filter(|u| {
                u.granter == self.node
                    && u.grant == grant
                    && u.serial == entry.serial
                    && u.generation == generation
            })
            .map_or(0, |u| u.leaves)
    }

    /// A `Send` received: delivered to the first thread that waits to
    /// receive on the mailbox, if it has an own context, with the remote
    /// frame capabilities of its grants (all or none); answered at once.
    pub(crate) fn send_received(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        header: Header,
        stamp: (u64, u64),
        grants: Vec<Granted>,
        words: Vec<u64>,
    ) {
        let mailbox = self.mailbox.expect("a mailbox");
        let own = |model: &Self, server: &ObjId| model.threads[server].home.is_some();
        let receiver = self
            .receiver(mailbox)
            .filter(|(server, ..)| own(self, server));
        let Some((server, _, window)) = receiver else {
            let body = Body::SendAnswer {
                delivered: false,
                dropped: false,
                uses: vec![None; grants.len()],
            };
            let _ = self.write_message(from, body, None, credit);
            return;
        };
        // The receiver's range takes the remote frame capabilities, all or
        // none: with a slot missing or the use table too full, none.
        let me = self.node as usize;
        let room = {
            let space = self
                .cspaces
                .get(&window.cspace)
                .filter(|_| !self.dying(window.cspace));
            grants.len() <= window.count as usize
                && (0..grants.len()).all(|i| {
                    space.and_then(|s| s.slots.get(window.first as usize + i)) == Some(&Slot::Empty)
                })
        };
        let uses_free = (0..GRANTS as u16)
            .filter(|u| !self.table.uses[me].contains_key(u))
            .count();
        let accepted = room && uses_free >= grants.len();
        let mut uses = vec![None; grants.len()];
        if accepted {
            for (i, granted) in grants.iter().enumerate() {
                let place = Place {
                    space: window.cspace,
                    index: window.first + i as u32,
                };
                uses[i] = Some(self.place_remote(from, *granted, place));
            }
        }
        let dropped = !grants.is_empty() && !accepted;
        let caps = if accepted { grants.len() } else { 0 };
        let flags = if dropped {
            Header::FLAG_CAPS_DROPPED
        } else {
            0
        };
        let header = Header::new(header.label(), words.len(), caps, flags).expect("within limits");
        let mut words = words;
        if !self.has_page(server) {
            words.truncate(dodos_abi::REG_WORDS);
        }
        // The stamp of the sender, as its kernel states it; badge 0.
        if !self.aware(server) {
            self.mark(server, stamp.1);
        }
        let mut received = crate::ipc::received(header, 0);
        received.marks = dodos_abi::Marks::from_bits(stamp.1).unwrap_or(dodos_abi::Marks::NONE);
        received.task = stamp.0;
        self.unlink(server);
        self.wake(server, Ok(Output::Message(Delivery { received, words })));
        let body = Body::SendAnswer {
            delivered: true,
            dropped,
            uses,
        };
        let _ = self.write_message(from, body, None, credit);
    }

    /// A remote capability in a slot: a local object that names the grant,
    /// and its root capability with the anchor's rights and depth; the
    /// gate writes the use entry. Returns the use index.
    fn place_remote(&mut self, granter: u16, granted: Granted, place: Place) -> u16 {
        let me = self.node as usize;
        let use_entry = (0..GRANTS as u16)
            .find(|u| !self.table.uses[me].contains_key(u))
            .expect("checked");
        let id = ObjId(self.next_id);
        self.next_id += 1;
        let cap = CapId(self.next_id);
        self.next_id += 1;
        let generation = self.table.generation(granter);
        self.table.uses[me].insert(
            use_entry,
            UseEntry {
                granter,
                grant: granted.grant,
                serial: granted.serial,
                generation,
                root: Some(id),
                leaves: 0,
                release_pending: false,
                signal_pending: false,
            },
        );
        let anchor = self.table.grants[granter as usize][&granted.grant].anchor;
        self.caps.insert(
            cap,
            Cap {
                object: id,
                rights: anchor.rights,
                badge: 0,
                parent: None,
                children: Vec::new(),
                place,
                dying: false,
                record: Record::Empty,
            },
        );
        self.objects.insert(
            id,
            Object {
                kind: ObjectKind::Frame,
                root: cap,
                from: None,
                boot: false,
            },
        );
        live(&mut self.cspaces, place.space).slots[place.index as usize] = Slot::Full(cap);
        self.frames.insert(id, anchor.frame);
        let remote = Remote {
            node: granter,
            grant: granted.grant,
            serial: granted.serial,
            use_entry,
            depth: anchor.depth,
            rights: anchor.rights,
        };
        self.remotes.insert(id, remote);
        use_entry
    }

    /// The `SendAnswer`, applied in the event that consumes it whether or
    /// not the sending thread still waits (D-131): each use index recorded
    /// in its grant entry, marked *answered*; the grants of the frames not
    /// delivered freed; the credit freed; then the thread woken, if one
    /// waits.
    pub(crate) fn send_answered(
        &mut self,
        from: u16,
        credit: Option<(u8, u32)>,
        delivered: bool,
        dropped: bool,
        uses: Vec<Option<u16>>,
    ) {
        let Some((index, _)) = credit else { return };
        let Some(taken) = self.take_credit(from, index) else {
            return;
        };
        let Completes::Send { grants, thread, .. } = taken.completes else {
            return;
        };
        let me = self.node as usize;
        for (i, (grant, serial)) in grants.into_iter().enumerate() {
            // The answer acts on the entry its send wrote and on no other:
            // the gate writes the use index and *answered* once per serial,
            // by a compare-and-swap from unanswered (G6). An entry released
            // before its answer and written anew for another send has
            // another serial, and may name another node: the answer then
            // finds nothing of its own to write or to free.
            let Some(entry) = self.table.grants[me].get_mut(&grant) else {
                continue;
            };
            if entry.serial != serial || entry.grantee != from || entry.answered {
                continue;
            }
            match uses.get(i).copied().flatten() {
                Some(use_index) => (entry.use_index, entry.answered) = (Some(use_index), true),
                None => self.free_with_anchor(grant),
            }
        }
        if let Some(thread) = thread
            && self.threads.get(&thread).is_some_and(|t| {
                t.wait
                    == Wait::Node {
                        peer: from,
                        credit: index,
                    }
            })
        {
            self.end_bounded(thread);
            self.wake(thread, sent(delivered, dropped));
        }
    }

    /// The thread of a wait for another node that is destroyed or whose
    /// wait otherwise ends: the answer, when it comes, finds no thread to
    /// wake.
    pub(crate) fn forget_sender(&mut self, thread: ObjId, peer: u16, credit: u8) {
        if let Some(Some(credit)) = self.peers[peer as usize].credits.get_mut(credit as usize)
            && let Completes::Send {
                thread: waiting, ..
            } = &mut credit.completes
            && *waiting == Some(thread)
        {
            *waiting = None;
        }
        if self.remover == Some(thread) {
            self.remover = None;
        }
    }

    /// `grant_signal`: rings the doorbell of a grant through its remote
    /// frame capability. No state error: the `Signal` goes into the user
    /// ring to the granting node, or the use entry's *signal pending* is
    /// set.
    pub(crate) fn grant_signal(&mut self, thread: ObjId, frame: SlotAddr) -> Outcome {
        let cap = self.cap(thread, frame, Rights::READ)?;
        let held = self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        let Some(remote) = self.remotes.get(&held.object).copied() else {
            return Err(Error::Invalid);
        };
        if !held.rights.contains(Rights::SEND) {
            return Err(Error::Denied);
        }
        let body = Body::Signal {
            grant: remote.grant,
            serial: remote.serial,
        };
        // A signal to a granting node whose generation is over is dropped:
        // it would only be dropped again (H6).
        let me = self.node as usize;
        if self.valid(&remote)
            && self.write_message(remote.node, body, None, None).is_err()
            && let Some(entry) = self.table.uses[me].get_mut(&remote.use_entry)
        {
            entry.signal_pending = true;
        }
        Ok(Output::Done)
    }

    /// A `Signal` received: if the grant entry is live and has a doorbell,
    /// its bits are set in its notify, as by `signal`; a dying notify gets
    /// nothing; a grant without a doorbell, revoking or pinned, nothing.
    pub(crate) fn signal_received(&mut self, grant: u16) {
        let me = self.node as usize;
        let Some(entry) = self.table.grants[me].get(&grant).copied() else {
            return;
        };
        if entry.state != GrantState::Live {
            return;
        }
        if let Some((notify, bits)) = entry.doorbell
            && self.notifies.contains_key(&notify)
            && !self.dying(notify)
        {
            live(&mut self.notifies, notify).word |= bits;
            self.hand_over(notify);
        }
    }

    /// Destroying a notify unbinds its doorbells, one unit each, after its
    /// timers, its irqs and its watches. Says whether there was one.
    pub(crate) fn undoorbell_one(&mut self, notify: ObjId) -> bool {
        let Some(grant) = self.notifies[&notify].doorbells.last().copied() else {
            return false;
        };
        live(&mut self.notifies, notify).doorbells.pop();
        let me = self.node as usize;
        if let Some(entry) = self.table.grants[me].get_mut(&grant) {
            entry.doorbell = None;
        }
        true
    }

    /// Revocation, one unit, when a removal on this node reaches an anchor
    /// (Grants). Returns whether the anchor may be removed now; `Err` with
    /// the outcome of the call when the calling thread waits.
    pub(crate) fn revoke_anchor(
        &mut self,
        anchor: CapId,
        caller: Option<ObjId>,
    ) -> Result<bool, Outcome> {
        let Some(grant) = self.grant_of(anchor) else {
            return Ok(true);
        };
        let me = self.node as usize;
        let Some(entry) = self.table.grants[me].get(&grant).copied() else {
            return Ok(true);
        };
        // A generation of the grantee that is over: the grant died with it.
        if self.grantee_over(&entry) {
            self.free_grant(grant);
            return Ok(true);
        }
        // A pinned grant, or one whose grantee is accused: `again`, with no
        // message and no wait, until the grantee's generation is over.
        let accused = matches!(
            self.table.member(entry.grantee).state(),
            MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
        );
        if entry.state == GrantState::Pinned || accused {
            return Ok(false);
        }
        // Not answered yet: `again`, with no message.
        if !entry.answered {
            return Ok(false);
        }
        // The grant becomes revoking, so its remote capabilities are invalid
        // and the grantee's gate refuses new mappings under it; then the
        // count of the grantee's use entry at the use index is read, counted
        // only if that entry names this node, this grant, its serial and
        // this node's generation.
        let grantee = entry.grantee;
        if let Some(e) = self.table.grants[me].get_mut(&grant) {
            e.state = GrantState::Revoking;
        }
        let count = self.holder_count(grant, &entry);
        if count == 0 {
            self.free_grant(grant);
            return Ok(true);
        }
        // A `Revoke` already in flight for this grant: `again`, with no
        // second message and no wait, until its answer.
        let in_flight = self.peers[grantee as usize]
            .credits
            .iter()
            .flatten()
            .any(|c| matches!(c.completes, Completes::Revoke { grant: g } if g == grant));
        let Some(thread) = caller.filter(|_| !in_flight) else {
            return Ok(false);
        };
        let body = Body::Revoke {
            grant,
            serial: entry.serial,
            use_index: entry.use_index.expect("answered"),
        };
        // No request credit free, or no room in the ring: `again`, with no
        // message and no wait.
        let Ok(Some((index, _))) =
            self.write_message(grantee, body, Some(Completes::Revoke { grant }), None)
        else {
            return Ok(false);
        };
        live(&mut self.threads, thread).wait = Wait::Node {
            peer: grantee,
            credit: index,
        };
        self.wait_mark(crate::locks::WaitKind::Node);
        self.observe();
        Err(Ok(Output::Waits))
    }

    /// The holder frees a use entry only at a count of 0 (G6, D-142): an
    /// entry whose count a failed unit kept stays, with no root, until the
    /// machine restarts. The gate counts no new leaf in it (no remote
    /// capability names it any more) and a `Send` never writes it for
    /// another grant (it is not free). A pending release is dropped.
    pub(crate) fn free_use(&mut self, index: u16) {
        let uses = &mut self.table.uses[self.node as usize];
        match uses.get_mut(&index) {
            Some(entry) if entry.leaves == 0 => {
                uses.remove(&index);
            }
            Some(entry) => entry.release_pending = false,
            None => {}
        }
    }

    /// A `Revoke` received by the holder: one removal step of at most 32
    /// units on its root remote capability of the grant; answered at once
    /// with `RevokeStep`. A record whose invalidation needs other cores or
    /// a unit is left *unmapping* and ends the step; the root is not
    /// removed while a record under it is unmapping. A `Revoke` that finds
    /// the root already removed (its release pending, or no root) answers
    /// removed, drops a pending release and frees the use entry if its
    /// count is 0 (D-142).
    pub(crate) fn revoke_received(
        &mut self,
        from: u16,
        (_grant, _serial, use_index): (u16, u32, u16),
        credit: Option<(u8, u32)>,
    ) {
        let me = self.node as usize;
        let entry = self.table.uses[me][&use_index];
        let root = entry.root.filter(|r| self.objects.contains_key(r));
        let removed = match (entry.release_pending, root) {
            (true, _) | (_, None) => {
                self.free_use(use_index);
                true
            }
            (false, Some(root)) => {
                let root = self.objects[&root].root;
                live(&mut self.caps, root).dying = true;
                self.revoking = true;
                let step = self.remove_root(root);
                self.revoking = false;
                step == Ok(Output::Done)
            }
        };
        let _ = self.write_message(from, Body::RevokeStep { removed }, None, credit);
    }

    /// The `RevokeStep`, applied in the event that consumes it whether or
    /// not the thread still waits: a step that reports the root removed
    /// frees the grant entry, or pins it while the holder's use entry still
    /// counts a leaf; any other makes the removal step return `again`, and
    /// its next call sends `Revoke` again.
    pub(crate) fn revoke_answered(&mut self, from: u16, credit: Option<(u8, u32)>, removed: bool) {
        let Some((index, _)) = credit else { return };
        let Some(taken) = self.take_credit(from, index) else {
            return;
        };
        let Completes::Revoke { grant } = taken.completes else {
            return;
        };
        // The root reported removed frees the entry only if the holder's
        // use entry counts no leaf (D-141); otherwise a failed unit kept the
        // count, the grant is pinned until the machine restarts and the
        // step traced (D-142): the removal step then returns `again` with
        // no message.
        if removed {
            let me = self.node as usize;
            match self.table.grants[me].get(&grant).copied() {
                Some(entry) if self.holder_count(grant, &entry) == 0 => self.free_grant(grant),
                Some(_) => {
                    self.pin(grant);
                    self.refuse_trace(from, dodos_abi::NodeMessage::RevokeStep, grant);
                }
                None => {}
            }
        }
        self.wake_remover(from, index);
    }

    /// The thread that waits for the answer of a revocation step goes on:
    /// its removal call returns `again`.
    fn wake_remover(&mut self, peer: u16, credit: u8) {
        let waiting = self
            .threads
            .iter()
            .find(|(_, t)| t.wait == Wait::Node { peer, credit })
            .map(|(&id, _)| id);
        if let Some(thread) = waiting {
            self.end_bounded(thread);
            self.wake(thread, Err(Error::Again));
        }
    }

    /// Release by the holder: its root remote capability of a grant is
    /// removed (by a removal other than one done for a `Revoke`). It sends
    /// the granting node a `Release` and frees its use entry once the
    /// message is in the ring, or keeps the entry with *release pending*
    /// while the ring has no room beyond the reserve. A use entry of a
    /// generation of the granting node that is over sends nothing. Every
    /// path frees the use entry only at a count of 0 (D-142).
    pub(crate) fn remote_destroyed(&mut self, object: ObjId) {
        let Some(remote) = self.remotes.remove(&object) else {
            return;
        };
        let me = self.node as usize;
        let Some(entry) = self.table.uses[me].get_mut(&remote.use_entry) else {
            return;
        };
        entry.root = None;
        let entry = *entry;
        let current = self.table.writes(remote.node)
            || self.table.member(remote.node).state() == MemberState::Sealed;
        let current = current && self.table.generation(remote.node) == entry.generation;
        // A removal done for a `Revoke` sends no `Release`: its last
        // `RevokeStep` stands for it.
        if self.revoking || !current {
            self.free_use(remote.use_entry);
            return;
        }
        let body = Body::Release {
            grant: remote.grant,
            serial: remote.serial,
        };
        match self.write_message(remote.node, body, None, None) {
            Ok(_) => self.free_use(remote.use_entry),
            Err(_) => {
                if let Some(entry) = self.table.uses[me].get_mut(&remote.use_entry) {
                    entry.release_pending = true;
                }
            }
        }
    }

    /// A `Release` received by the granting node: it frees the entry if
    /// its anchor has no descendant, otherwise the anchor stays, held by
    /// nobody. On a *revoking* entry it ends the revocation as a
    /// `RevokeStep` reporting the root removed would: the entry is freed,
    /// a `Revoke` of it still in flight stops counting as in flight (its
    /// credit freed, its thread woken with `again`), and the removal step
    /// finds the entry freed at its next call; or, at a count above 0, the
    /// entry is pinned and the credit stays in use until its answer
    /// (D-142).
    pub(crate) fn release_received(&mut self, from: u16, grant: u16, _serial: u32) {
        let me = self.node as usize;
        let Some(entry) = self.table.grants[me].get(&grant).copied() else {
            return;
        };
        // A `Release` frees the entry only if the holder's use entry counts
        // no leaf under it; otherwise the entry stays and the message is
        // traced (Grants, release by the holder, D-141), and a *revoking*
        // entry is pinned: only a failed unit leaves the root removed with
        // the count above 0 (D-142).
        if self.holder_count(grant, &entry) != 0 {
            if entry.state == GrantState::Revoking {
                self.pin(grant);
            }
            self.refuse_trace(from, dodos_abi::NodeMessage::Release, grant);
            return;
        }
        if entry.state == GrantState::Revoking {
            self.free_grant(grant);
            let credit = self.peers[from as usize].credits.iter().position(|c| {
                c.as_ref().is_some_and(
                    |c| matches!(c.completes, Completes::Revoke { grant: g } if g == grant),
                )
            });
            if let Some(index) = credit {
                self.peers[from as usize].credits[index] = None;
                self.wake_remover(from, index as u8);
            }
            return;
        }
        // The entry is freed; its anchor is removed if it has no
        // descendant, and otherwise stays, held by nobody.
        self.free_with_anchor(grant);
    }

    /// A grant whose holder removed its root while a failed unit kept its
    /// use entry's count: pinned until the machine restarts (D-142).
    fn pin(&mut self, grant: u16) {
        if let Some(entry) = self.table.grants[self.node as usize].get_mut(&grant) {
            entry.state = GrantState::Pinned;
        }
    }

    /// A message the check passed but whose effect a word written by
    /// another node's gate refuses: it changes nothing, and is traced as a
    /// refusal (code 11).
    pub(crate) fn refuse_trace(&mut self, from: u16, kind: dodos_abi::NodeMessage, named: u16) {
        self.refused.0 += 1;
        let who = u64::from(from) | u64::from(self.table.generation(from)) << 32;
        self.trace(
            dodos_abi::TraceCode::Refused,
            None,
            [who, kind as u64, u64::from(named)],
        );
    }
}

/// The serial of a grant index once its entry is freed: one more, on the 30
/// bits a `Revoke`, `Release` or `Signal` carries (O8).
pub(crate) fn next_serial(serial: u32) -> u32 {
    (serial + 1) & 0x3FFF_FFFF
}
