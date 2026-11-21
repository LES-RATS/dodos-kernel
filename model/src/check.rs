//! The invariants of the state.

use crate::{CapId, Model, ObjId, Output, Place, Record, ReplyState, Slot, Thread, Wait, Window};
use dodos_abi::{
    CSPACE_MAX_SLOTS, CSPACE_SEGMENT_SLOTS, Error, FAULT_WORDS, FaultKind, Header, LONGEST_WALK,
    MAX_CAPS, MAX_WORDS, MemoryType, ObjectKind, REG_WORDS, Rights, TREE_DEPTH,
};
use std::collections::BTreeMap;
use std::mem::discriminant;

/// `Err` names the invariant that fails, by its number in `Model::check`.
type Check = Result<(), &'static str>;

fn ensure(holds: bool, invariant: &'static str) -> Check {
    if holds { Ok(()) } else { Err(invariant) }
}

/// What a state holds, for a test or a harness to measure what its sequences
/// of calls reach. It is no part of the behaviour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Census {
    /// Capabilities, in every slot.
    pub capabilities: usize,
    /// The depth of the deepest capability.
    pub deepest: usize,
    /// The kind of each object that is dying.
    pub dying: Vec<ObjectKind>,
    /// Capabilities under `cap_delete` whose object is not dying.
    pub dying_copies: usize,
    /// Threads waiting to be received, in the queue of an endpoint.
    pub callers: usize,
    /// Threads waiting for a reply, each through an armed reply object.
    pub served: usize,
    /// Threads waiting to receive, each with an attached reply object.
    pub receivers: usize,
    /// Threads waiting on a notify.
    pub waiters: usize,
    /// Notifies bound to an endpoint.
    pub bound: usize,
    /// Frame capabilities whose record holds a mapping.
    pub mappings: usize,
    /// Devices attached to an iospace.
    pub devices: usize,
    /// Devices whose detach is in flight (D-139).
    pub detaching: usize,
    /// Threads that wait on a fault, and of these, by the kind of their
    /// fault while it is queued, indexed by the kind's label (1 to 9;
    /// invariant 40 bounds it, entry 0 stays 0).
    pub faulting: usize,
    pub fault_kinds: [usize; 10],
    /// Contexts throttled, lends, armed timers, threads marked as having
    /// lost their context.
    pub throttled: usize,
    pub lends: usize,
    pub armed: usize,
    pub lost: usize,
    /// Whether the core is idle, and whether the running context runs on
    /// slack.
    pub idle: bool,
    pub on_slack: bool,
    /// Claimed irqs, fired ones, revoked tasks.
    pub irqs: usize,
    pub fired: usize,
    pub revoked: usize,
}

impl Model {
    /// The depth of every capability, from the parents down: a capability
    /// without a parent is at depth 1. The invariants do not use
    /// `Model::depth`, which the calls use: a fault in one does not hide in
    /// the other. A parent is older than its child (invariant 1), so it
    /// comes first; one that is missing counts for nothing here and is
    /// reported by invariant 1.
    fn depths(&self) -> BTreeMap<CapId, usize> {
        let mut depths = BTreeMap::new();
        for (&id, cap) in &self.caps {
            let above = cap.parent.and_then(|parent| depths.get(&parent));
            // A root remote capability has the depth of its anchor.
            let top = match self.remotes.get(&cap.object) {
                Some(remote) if cap.parent.is_none() => remote.depth,
                _ => 1,
            };
            let depth = above.map_or(top, |above| above + 1);
            depths.insert(id, depth);
        }
        depths
    }

    /// Whether `object` is an `its_table` whose pages read pinned (The ITS
    /// word: bits 6-7 of the class 1 from the first LPI's claim).
    fn its_pinned(&self, object: ObjId) -> bool {
        self.its_tables.get(&object).is_some_and(|&(first, _)| {
            self.table.classes.get(&first) == Some(&dodos_abi::PageClass::its_table(true))
        })
    }

    /// Whether the root capability of an object is marked dying, read from
    /// the capability itself and not through `Model::dying`.
    fn marked(&self, object: ObjId) -> bool {
        let root = self.objects.get(&object).map(|object| object.root);
        let root = root.and_then(|root| self.caps.get(&root));
        root.is_some_and(|root| root.dying)
    }

    /// Counts what the state holds.
    pub fn census(&self) -> Census {
        let dying = self.objects.iter().filter(|(id, _)| self.marked(**id));
        let copy = |cap: &&crate::Cap| cap.dying && !self.marked(cap.object);
        let waits = |wanted: fn(&Wait) -> bool| {
            self.threads
                .values()
                .filter(|thread| wanted(&thread.wait))
                .count()
        };
        Census {
            capabilities: self.caps.len(),
            deepest: self.depths().into_values().max().unwrap_or(0),
            dying: dying.map(|(_, object)| object.kind).collect(),
            dying_copies: self.caps.values().filter(copy).count(),
            callers: waits(|wait| matches!(wait, Wait::Call { .. })),
            served: waits(|wait| matches!(wait, Wait::Reply(_))),
            receivers: waits(|wait| matches!(wait, Wait::Recv { .. })),
            waiters: waits(|wait| matches!(wait, Wait::Notify(_))),
            bound: self
                .notifies
                .values()
                .filter(|notify| notify.endpoint.is_some())
                .count(),
            mappings: self
                .caps
                .values()
                .filter(|cap| matches!(cap.record, Record::Mapping(_)))
                .count(),
            devices: self.iospaces.values().map(Vec::len).sum(),
            detaching: self.iommus.values().map(|u| u.detaching.len()).sum(),
            faulting: self.threads.values().filter(|t| t.faulting).count(),
            fault_kinds: {
                let mut kinds = [0; 10];
                for thread in self.threads.values().filter(|t| t.faulting) {
                    if let Wait::Call { parcel, .. } = &thread.wait
                        && let Some(kind) = kinds.get_mut(parcel.label as usize)
                    {
                        *kind += 1;
                    }
                }
                kinds
            },
            throttled: self
                .scheds
                .values()
                .filter(|s| s.throttled.is_some())
                .count(),
            lends: self.lends.len(),
            armed: self.timers.values().flatten().count(),
            lost: self.threads.values().filter(|t| t.lost).count(),
            idle: self.cores[0].running.is_none(),
            on_slack: self.cores[0].running.is_some_and(|(_, ready)| !ready),
            irqs: self.irqs.len(),
            fired: self
                .irqs
                .values()
                .filter(|i| i.state == crate::IrqState::Fired)
                .count(),
            revoked: self.tasks.values().filter(|t| t.revoked).count(),
        }
    }

    /// A block whose words a removal step has written free (on x86-64 and
    /// RISC-V in a node of several cores, at the completion of the request
    /// of its run, which forgets the block) and that the next step of the
    /// removal destroys: its root, dying, is still a child of the
    /// memory_control capability it was taken through (found by pass 20).
    pub(crate) fn written_block(&self, id: ObjId) -> bool {
        let Some(untyped) = self.untypeds.get(&id) else {
            return false;
        };
        let Some(root) = self.objects.get(&id).and_then(|o| self.caps.get(&o.root)) else {
            return false;
        };
        let parent = root.parent.and_then(|parent| self.caps.get(&parent));
        let control = parent.and_then(|parent| self.objects.get(&parent.object));
        untyped.block.is_none()
            && root.dying
            && control.is_some_and(|c| c.kind == ObjectKind::MemoryControl)
    }

    /// Checks every invariant of the state and names the first that fails.
    ///
    /// The derivation tree:
    /// 1. a capability's parent exists, is older than it (so the tree has no
    ///    cycle) and lists it once among its children;
    /// 2. a capability's children exist, name it as their parent, and are in
    ///    the order of derivation;
    /// 3. a capability derived from another has a subset of its rights, and
    ///    its badge if it has one; a root capability has every right, with
    ///    `exec` if and only if the capability it was made from has it (a
    ///    frame of the start-up never has it, an irq always), and no badge,
    ///    but a kept frame, which has the rights of its row; only an endpoint
    ///    or a notify capability has a badge;
    /// 4. a capability is the root capability of its object, or was derived
    ///    from a capability for the same object;
    /// 20. the tree is at most `TREE_DEPTH` deep;
    /// 21. removal ends: no capability has to be removed before itself, when
    ///     a capability goes before its parent and before the root capability
    ///     of the capability space that holds it, unless it is that one;
    /// 24. the walk to the next capability to remove was always shorter than
    ///     192 capabilities: at most `LONGEST_WALK`;
    /// 28. a capability space holds only what is deeper than itself: an
    ///     untyped or `memory_control` capability it holds is not less deep
    ///     than its root capability (D-141), and the root capability of
    ///     another space is deeper.
    ///
    /// Slots:
    /// 5. slot 0 of every capability space is empty;
    /// 6. every capability is in a slot: the slot it names is full and names
    ///    it, and every full slot names a live capability that names it;
    /// 7. a capability space has a whole number of segments, at most its
    ///    maximum, which is a whole number of segments, at least one and at
    ///    most 2^20 slots; the start-up space, taken from no untyped, has
    ///    all its slots;
    /// 27. a capability space keeps a sweep if and only if it is dying; the
    ///     sweep stands at slot 1 or after, within the table, and no slot
    ///     before it is full, but for the root capability of the space and,
    ///     in pass 1, for the root capability of a table untyped in a slot
    ///     from the first deferred one on; the first deferred slot is
    ///     before the sweep in pass 1, and none is kept in pass 2 (D-156).
    ///
    /// Objects:
    /// 8. every capability names a live object;
    /// 9. an object exists if and only if its root capability does: its root
    ///    capability is live and names it (with 4 and 8);
    /// 10. an object has state of its kind, and every state is that of an
    ///     object of its kind;
    /// 11. an object taken from an untyped names a live untyped, and its root
    ///     capability is a child of a capability for that untyped; an irq was
    ///     taken from none and its root capability is a child of an
    ///     irq_control capability; a block was taken from none and its root
    ///     capability is a child of a memory_control capability; another
    ///     start-up object was taken from none and its root capability has
    ///     no parent;
    /// 25. device memory holds only frames and child untypeds, which are
    ///     device memory too, and no capability for a device untyped has
    ///     `exec` (so, with 3, none for what is made from it either);
    /// 26. every identity was given out before the next one to give; an
    ///     object is younger than the untyped it was taken from and older
    ///     than its root capability.
    ///
    /// Waiting:
    /// 12. every thread in the queue of an endpoint waits on that endpoint,
    ///     and callers and receivers never queue together;
    /// 13. every thread in the queue of a notify waits on that notify, and
    ///     the word of a notify with a waiter is zero;
    /// 14. a thread is in at most one queue, once, and it is in the queue of
    ///     what it waits on;
    /// 15. a reply object's state agrees with the thread it names: attached
    ///     to a thread if and only if that thread waits to receive with it,
    ///     armed with a thread if and only if that thread waits for a reply
    ///     through it;
    /// 16. a thread that waits has no outcome yet, unless it waits on a
    ///     fault that the kernel raised before it ran again (the outcome of
    ///     its last call is in its registers);
    /// 30. the outcome of a wait is `gone`, `cancelled`, a word that is not
    ///     zero, or a message with as many words as its header states, or
    ///     with the register words only when it states more;
    /// 31. a thread has a wait number other than 0 only while it waits, and
    ///     that number is greater than its word "cancelled up to";
    /// 17. a notify names its endpoint if and only if the endpoint names it;
    /// 22. no thread waits with a dying reply object, to receive or for an
    ///     answer: a reply object whose root capability is dying is idle;
    /// 23. a dying capability space only shrinks, so no thread in it or
    ///     outside it was the cause of a new capability there: every
    ///     capability it holds, it held when it began to die;
    /// 40. a thread that waits on a fault waits as a caller, to be received
    ///     or for the answer, and a fault message that waits has a label of
    ///     a fault kind, five words and no capability;
    /// 29. a message that waits was sent by the thread that waits with it
    ///     and is within the limits of the header; a receive range that
    ///     waits has at most 4 slots, none of them slot 0 or beyond the
    ///     table; the flags that an armed reply object keeps are the flag
    ///     `caps_dropped` or none.
    ///
    /// Bindings:
    /// 32. a thread bound to a capability space, an address space or a
    ///     fault endpoint is listed once by that live object, and an object
    ///     lists only threads bound to it;
    /// 33. the message page of a thread is a live capability for a frame of
    ///     one 4 KB page of ordinary memory whose record names the thread,
    ///     and a record that names a thread is that thread's message page.
    ///
    /// Memory:
    /// 18. the watermark of an untyped is within it, is zero if and only if
    ///     nothing taken from it is live, and is a multiple of 8 bytes, the
    ///     smallest size of an object; the clean mark of ordinary memory is
    ///     between the watermark and the end, and device memory has none;
    /// 19. the live count of an untyped is the number of live objects,
    ///     cspace segments and tables of address spaces taken from it;
    /// 65. between two steps of a creation, the memory from the watermark to
    ///     the clean mark, padding included, was cleared by the steps and
    ///     taken by no object since;
    /// 34. a frame has 1 to *E* pages, ordinary memory is write-back and
    ///     device memory is not, and only a frame capability has a record.
    pub fn check(&self) -> Check {
        self.check_tree()?;
        self.check_removal()?;
        self.check_slots()?;
        self.check_objects()?;
        self.check_waits()?;
        self.check_memory()?;
        self.check_spaces()?;
        self.check_bindings()?;
        self.check_sched()?;
        self.check_tasks()?;
        self.check_irqs()?;
        self.check_table()?;
        self.check_identifiers()?;
        self.check_lines()?;
        self.check_reclaim()?;
        self.check_cores()?;
        self.check_node()?;
        self.check_debug()?;
        self.check_events()
    }

    /// 88. a device is attached to an iospace of this node, or is being
    ///     detached (its attach word not 0, D-137), exactly while its
    ///     identifier word reads taken by this node's generation (IOMMU: a
    ///     device identifier has an owner); an attach word that is not 0
    ///     sits beside an identifier word taken by some generation, and
    ///     names a root of class 40 + n (D-154) with n at least the words
    ///     that name it (D-137).
    fn check_identifiers(&self) -> Check {
        let generation = self.table.generation(self.node);
        let taken =
            dodos_abi::Ownership::taken(dodos_abi::Owner::Node(self.node), generation).raw();
        let mut attached = std::collections::BTreeSet::new();
        for iommu in self.iommus.values() {
            let segment = u32::from(self.machine.iommus[iommu.unit].segment);
            for &device in iommu.attached.keys() {
                attached.insert(segment << 16 | u32::from(device));
            }
        }
        let held: std::collections::BTreeSet<u32> = self
            .table
            .identifiers
            .iter()
            .filter(|&(_, &raw)| raw == taken)
            .map(|(&key, _)| key)
            .collect();
        // The words that name each root, against its class (audit lot 7:
        // only n > 0 was judged).
        let mut naming: std::collections::BTreeMap<u64, u8> = std::collections::BTreeMap::new();
        let page = self.table.geometry.page();
        for &raw in self.table.attach.values() {
            if let Some(word) = dodos_abi::AttachWord::from_raw(raw) {
                let count = naming.entry(word.root() / page).or_default();
                *count = count.saturating_add(1);
            }
        }
        for (&key, &raw) in &self.table.attach {
            let word = dodos_abi::AttachWord::from_raw(raw);
            let named = self.table.identifiers.get(&key).copied();
            let owned = named
                .and_then(dodos_abi::Ownership::from_raw)
                .is_some_and(|w| w.state() == dodos_abi::OwnState::Taken);
            let root = word.map_or(0, |w| w.root()) / page;
            let class = self.table.classes.get(&root).copied();
            let counted = class.and_then(dodos_abi::PageClass::root_devices);
            if word.is_none_or(|w| w.state().is_none() || !w.root().is_multiple_of(page))
                || !owned
                || counted.is_none_or(|n| n == 0)
            {
                return Err("88: an attach word with no identifier word or root beside it");
            }
            let names = naming.get(&root).copied().unwrap_or(0);
            ensure(
                counted.is_some_and(|n| n >= names),
                "88: a root whose class counts fewer devices than the words that name it",
            )?;
            if named == Some(taken) {
                attached.insert(key);
            }
        }
        match held == attached {
            true => Ok(()),
            false => Err("88: identifier words and attached devices"),
        }
    }

    /// 96. a line is claimed by an irq of this node exactly while its line
    ///     word reads taken by this node's generation (Interrupts, the
    ///     owner word of a line, D-135).
    fn check_lines(&self) -> Check {
        // The table of line words has at most `IRQ_LINES_MAX` words, kept
        // for the life of the machine (Interrupts, D-135; finding 9 of the
        // tenth model review).
        if self
            .table
            .lines
            .keys()
            .any(|&gsi| gsi as usize >= dodos_abi::IRQ_LINES_MAX)
        {
            return Err("96: a line word beyond the last");
        }
        let generation = self.table.generation(self.node);
        let taken =
            dodos_abi::Ownership::taken(dodos_abi::Owner::Node(self.node), generation).raw();
        let claimed: std::collections::BTreeSet<u32> = self
            .irqs
            .values()
            .filter(|irq| irq.source.is_line())
            .map(|irq| irq.source.id())
            .collect();
        let held: std::collections::BTreeSet<u32> = self
            .table
            .lines
            .iter()
            .filter(|&(_, &raw)| raw == taken)
            .map(|(&gsi, _)| gsi)
            .collect();
        match held == claimed {
            true => Ok(()),
            false => Err("96: line words and claimed lines"),
        }
    }

    /// R0. Reclaim runs once for each generation of each node (What a
    /// death does: R0 to R6; finding 8 of the tenth model review). Its own
    /// check, not a part of the interrupts' (ver-D D9).
    fn check_reclaim(&self) -> Check {
        ensure(
            self.r0_runs.values().all(|&runs| runs <= 1),
            "R0: run twice for one generation",
        )?;
        // What R0 leaves (Reclaim, R0; audit lot 7): once it ran for a
        // generation that is accused or dead, no thread waits for an answer
        // of it, no unit request stays sent to it, and none of its requests
        // is served.
        for (&x, reclaim) in &self.reclaims {
            let member = self.table.members.get(usize::from(x));
            let isolated = reclaim.isolated
                && member.is_some_and(|m| {
                    m.generation() == reclaim.generation
                        && matches!(
                            m.state(),
                            dodos_abi::MemberState::Fencing
                                | dodos_abi::MemberState::Fenced
                                | dodos_abi::MemberState::Unfenced
                                | dodos_abi::MemberState::Dead
                        )
                });
            if !isolated {
                continue;
            }
            let waits = self
                .threads
                .values()
                .any(|t| matches!(t.wait, Wait::Node { peer, .. } if peer == x));
            ensure(!waits, "R0: a wait for an answer of a node it isolated")?;
            let sent = self
                .unit_requests
                .iter()
                .flatten()
                .any(|e| matches!(e.stage, crate::units::Stage::Sent { to, .. } if to == x));
            ensure(!sent, "R0: a unit request still sent to a node it isolated")?;
            let served = self.unit_service.keys().any(|&(from, _)| from == x);
            ensure(!served, "R0: a request of a node it isolated still served")?;
        }
        Ok(())
    }

    fn check_tree(&self) -> Check {
        let depths = self.depths();
        for (&id, cap) in &self.caps {
            let object = self
                .objects
                .get(&cap.object)
                .ok_or("8: object of a capability")?;
            let mut derived = false;
            if let Some(parent) = cap.parent {
                let above = self.caps.get(&parent).ok_or("1: parent exists")?;
                ensure(parent < id, "1: parent is older")?;
                let listed = above.children.iter().filter(|&&child| child == id).count();
                ensure(listed == 1, "1: parent lists its child once")?;
                derived = above.object == cap.object;
                let subset = above.rights.contains(cap.rights);
                ensure(!derived || subset, "3: rights of a derivation")?;
                // An `ioports` capability carries a range in that word
                // (8.5, invariant 6 here numbered 93): inside its parent's.
                let ports = object.kind == ObjectKind::IoPorts;
                let kept = above.badge == 0 || above.badge == cap.badge;
                ensure(!derived || ports || kept, "3: badge of a derivation")?;
                let (outer, inner) = (above.badge, cap.badge);
                let inside = (outer & 0xFFFF) <= (inner & 0xFFFF)
                    && (inner >> 16 & 0xFFFF) <= (outer >> 16 & 0xFFFF)
                    && (inner & 0xFFFF) <= (inner >> 16 & 0xFFFF)
                    && inner >> 32 == 0;
                ensure(
                    !derived || !ports || inside,
                    "93: ioports range outside its parent's",
                )?;
            }
            let named = |child: &CapId| self.caps.get(child).is_some_and(|c| c.parent == Some(id));
            ensure(cap.children.iter().all(named), "2: children name it")?;
            ensure(cap.children.is_sorted(), "2: order of derivation")?;
            ensure(object.root == id || derived, "4: root or derivation")?;
            // A kept frame has the rights of its row: `read` and `derive`,
            // and `exec` for one of the kept image.
            let kept = self.frames.get(&cap.object).is_some_and(|frame| frame.kept);
            let read_derive = Rights::READ.union(Rights::DERIVE);
            let row = cap.rights == read_derive || cap.rights == read_derive.union(Rights::EXEC);
            let every = cap.rights.contains(Rights::ALL.without(Rights::EXEC));
            // A root remote capability has the rights of its anchor.
            let rights = match self.remotes.get(&cap.object) {
                Some(remote) => cap.rights == remote.rights,
                None if kept => row,
                None => every,
            };
            // The root of an `ioports` object holds every port.
            let root_badge = match object.kind {
                ObjectKind::IoPorts => 0xFFFF << 16,
                _ => 0,
            };
            ensure(
                derived || rights && cap.badge == root_badge,
                "3: root capability",
            )?;
            let exec = |cap: &CapId| self.caps[cap].rights.contains(Rights::EXEC);
            let made = cap.parent.filter(|_| !derived);
            // Device memory and a table untyped never carry `exec` (D-154).
            let device = self
                .untypeds
                .get(&cap.object)
                .is_some_and(|u| u.device || u.table);
            // The frames of the start-up, children of the start-up untyped,
            // have no `exec`; an irq's root has every right; device memory
            // is judged by invariant 25.
            let irq = self.irqs.contains_key(&cap.object);
            let boot = object.boot && !exec(&id);
            let same = irq || boot || made.is_none_or(|from| exec(&from) == exec(&id) || device);
            ensure(same, "3: exec of a root capability")?;
            // The word of an `ioports` capability is its range, not a badge.
            let badged = matches!(
                object.kind,
                ObjectKind::Endpoint | ObjectKind::Notify | ObjectKind::IoPorts
            );
            ensure(badged || cap.badge == 0, "3: badge on another kind")?;
            let device = |id: &ObjId| self.untypeds.get(id).is_some_and(|untyped| untyped.device);
            let on_device = device(&cap.object);
            let exec = cap.rights.contains(Rights::EXEC);
            ensure(!(on_device && exec), "25: exec on device memory")?;
            // A table untyped, and what is made from one, has no `exec`
            // (rules of `untyped_retype`, D-154).
            let table = |id: &ObjId| self.untypeds.get(id).is_some_and(|untyped| untyped.table);
            let from_tables = object.from.is_some_and(|from| table(&from));
            ensure(
                !((table(&cap.object) || from_tables) && exec),
                "TABLES: exec on a table untyped or on what it made",
            )?;
            ensure(id.0.max(cap.object.0) < self.next_id, "26: identity")?;
            let Place { space, index } = cap.place;
            let slots = self.cspaces.get(&space).map(|space| &space.slots);
            let slot = slots.and_then(|slots| slots.get(index as usize));
            ensure(slot == Some(&Slot::Full(id)), "6: slot of a capability")?;
            ensure(depths[&id] <= TREE_DEPTH, "20: depth of the tree")?;
        }
        Ok(())
    }

    /// Invariant 21. A capability is removed after its children, and the
    /// root capability of a capability space after what the space holds:
    /// `after(a)` gives the capabilities that go after `a`, its parent and
    /// the root of the space it is in. A removal ends if and only if these
    /// edges make no cycle. So this takes away, again and again, a
    /// capability that nothing left has to go before; what cannot be taken
    /// away is in a cycle.
    fn check_removal(&self) -> Check {
        let after = |id: &CapId| {
            let cap = &self.caps[id];
            let holder = self.objects.get(&cap.place.space).map(|space| space.root);
            let holder = holder.filter(|holder| holder != id);
            cap.parent.into_iter().chain(holder)
        };
        let mut before: BTreeMap<CapId, usize> = BTreeMap::new();
        for later in self.caps.keys().flat_map(after) {
            *before.entry(later).or_default() += 1;
        }
        let first = |id: &&CapId| !before.contains_key(id);
        let mut free: Vec<CapId> = self.caps.keys().filter(first).copied().collect();
        let mut left = self.caps.len();
        while let Some(id) = free.pop() {
            left -= 1;
            for later in after(&id).filter(|later| self.caps.contains_key(later)) {
                let count = before.entry(later).or_default();
                *count -= 1;
                if *count == 0 {
                    free.push(later);
                }
            }
        }
        ensure(left == 0, "21: removal that never ends")?;
        ensure(self.longest_walk <= LONGEST_WALK, "24: walk of a removal")
    }

    fn check_slots(&self) -> Check {
        let depths = self.depths();
        // A capability that is missing is reported by invariant 6 or 9.
        let depth = |cap: CapId| depths.get(&cap).copied().unwrap_or(0);
        for (&id, space) in &self.cspaces {
            let first = space.slots.first();
            ensure(first.is_none_or(|slot| *slot == Slot::Empty), "5: slot 0")?;
            let object = self.objects.get(&id).ok_or("10: state without an object")?;
            let (root, sweep) = (object.root, space.sweep.as_ref());
            for (index, slot) in space.slots.iter().enumerate() {
                let Slot::Full(cap) = *slot else { continue };
                let held = self.caps.get(&cap).ok_or("6: capability of a full slot")?;
                let here = held.place
                    == Place {
                        space: id,
                        index: index as u32,
                    };
                ensure(here, "6: capability of a full slot")?;
                let of = self.objects.get(&held.object);
                let other_root = of.is_some_and(|of| of.root == cap) && cap != root;
                let deep_enough = match of.map(|of| of.kind) {
                    Some(ObjectKind::Untyped | ObjectKind::MemoryControl) => {
                        depth(cap) >= depth(root)
                    }
                    Some(ObjectKind::CSpace) if other_root => depth(cap) > depth(root),
                    _ => true,
                };
                ensure(deep_enough, "28: depth of what a space holds")?;
                let swept = sweep.is_some_and(|sweep| index < sweep.next);
                // Pass 1 moves past the root capability of a table untyped
                // or of an `its_table` pinned when the sweep passed it
                // (D-156, D-157 (3); audit lot 7: an `its_table` was not
                // read); it may be unpinned since, the spaces or LPIs that
                // pinned it gone.
                let table_root = of.is_some_and(|of| of.root == cap)
                    && (self.untypeds.get(&held.object).is_some_and(|u| u.table)
                        || self.its_tables.contains_key(&held.object));
                let deferred = table_root
                    && sweep.is_some_and(|sweep| {
                        !sweep.pass2 && sweep.first_deferred.is_some_and(|first| first <= index)
                    });
                ensure(
                    !swept || cap == root || deferred,
                    "27: full slot behind the sweep",
                )?;
                let kept = sweep.is_none_or(|sweep| sweep.held.contains(&cap));
                ensure(kept, "23: new capability in a dying space")?;
            }
            ensure(
                self.marked(id) == sweep.is_some(),
                "27: sweep of a dying space",
            )?;
            let slots = space.slots.len();
            let within = |sweep: &crate::Sweep| (1..=slots.max(1)).contains(&sweep.next);
            ensure(sweep.is_none_or(within), "27: index of the sweep")?;
            let first = |sweep: &crate::Sweep| match sweep.first_deferred {
                Some(first) => !sweep.pass2 && (1..sweep.next).contains(&first),
                None => true,
            };
            ensure(
                sweep.is_none_or(first),
                "27: first deferred slot of the sweep",
            )?;
            let whole = slots.is_multiple_of(CSPACE_SEGMENT_SLOTS as usize);
            ensure(whole && slots <= space.max as usize, "7: segments")?;
            let max = space.max != 0 && space.max.is_multiple_of(CSPACE_SEGMENT_SLOTS);
            ensure(max && space.max <= CSPACE_MAX_SLOTS, "7: maximum")?;
            let grows = object.from.is_some() || slots == space.max as usize;
            ensure(grows, "7: size of the start-up space")?;
        }
        Ok(())
    }

    fn check_objects(&self) -> Check {
        for (&id, object) in &self.objects {
            let root = self
                .caps
                .get(&object.root)
                .ok_or("9: root capability exists")?;
            ensure(root.object == id, "9: root capability names its object")?;
            ensure(
                id.0 < object.root.0,
                "26: root capability older than its object",
            )?;
            // A remote object holds no state of its kind on its holder, but
            // a remote frame, its frame.
            let remote = self.remotes.contains_key(&id);
            let has_state = match object.kind {
                _ if remote => object.kind != ObjectKind::Frame || self.frames.contains_key(&id),
                ObjectKind::Untyped => self.untypeds.contains_key(&id),
                ObjectKind::CSpace => self.cspaces.contains_key(&id),
                ObjectKind::Thread => self.threads.contains_key(&id),
                ObjectKind::Endpoint => self.endpoints.contains_key(&id),
                ObjectKind::Frame => self.frames.contains_key(&id),
                ObjectKind::ASpace => self.aspaces.contains_key(&id),
                ObjectKind::IoSpace => {
                    self.aspaces.contains_key(&id) && self.iospaces.contains_key(&id)
                }
                ObjectKind::Iommu => self.iommus.contains_key(&id),
                ObjectKind::Task => self.tasks.contains_key(&id),
                ObjectKind::Irq => self.irqs.contains_key(&id),
                ObjectKind::Sched => self.scheds.contains_key(&id),
                ObjectKind::Timer => self.timers.contains_key(&id),
                ObjectKind::Reply => self.replies.contains_key(&id),
                ObjectKind::Notify => self.notifies.contains_key(&id),
                _ => true,
            };
            ensure(has_state, "10: state of an object")?;
            if let Some(from) = object.from {
                ensure(self.untypeds.contains_key(&from), "11: untyped is live")?;
                ensure(from < id, "26: object older than its untyped")?;
                let parent = root.parent.and_then(|parent| self.caps.get(&parent));
                let child = parent.is_some_and(|parent| parent.object == from);
                ensure(child, "11: root capability is a child of the untyped's")?;
                let plain = matches!(object.kind, ObjectKind::Frame | ObjectKind::Untyped);
                let device = self.untypeds[&from].device;
                let inherits = self.untypeds.get(&id).is_none_or(|child| child.device);
                ensure(!device || plain && inherits, "25: kind on device memory")?;
            } else if object.kind == ObjectKind::Irq {
                // Interrupts: the root capability of an irq is a child of
                // the irq_control capability used to claim it.
                let parent = root.parent.and_then(|parent| self.caps.get(&parent));
                let control = parent.and_then(|parent| self.objects.get(&parent.object));
                let claimed = control.is_some_and(|c| c.kind == ObjectKind::IrqControl);
                ensure(claimed, "11: parent of an irq")?;
            } else if self.untypeds.get(&id).is_some_and(|u| u.block.is_some())
                || self.written_block(id)
            {
                // `memory_control`: a block is a child of the memory_control
                // capability it was taken through.
                let parent = root.parent.and_then(|parent| self.caps.get(&parent));
                let control = parent.and_then(|parent| self.objects.get(&parent.object));
                let taken = control.is_some_and(|c| c.kind == ObjectKind::MemoryControl);
                ensure(taken, "11: parent of a block")?;
            } else {
                ensure(root.parent.is_none(), "11: parent of a start-up capability")?;
            }
        }
        let is = |kind: ObjectKind| {
            move |id: &ObjId| {
                self.objects
                    .get(id)
                    .is_some_and(|object| object.kind == kind)
            }
        };
        let of_its_kind = self.untypeds.keys().all(is(ObjectKind::Untyped))
            && self.cspaces.keys().all(is(ObjectKind::CSpace))
            && self.threads.keys().all(is(ObjectKind::Thread))
            && self.frames.keys().all(is(ObjectKind::Frame))
            && self
                .aspaces
                .keys()
                .all(|id| is(ObjectKind::ASpace)(id) || is(ObjectKind::IoSpace)(id))
            && self.iospaces.keys().all(is(ObjectKind::IoSpace))
            // A machine object outlives its root on this node: the node keeps
            // its own list of its devices on the unit (D-131).
            && self
                .iommus
                .keys()
                .all(|id| !self.objects.contains_key(id) || is(ObjectKind::Iommu)(id))
            && self.tasks.keys().all(is(ObjectKind::Task))
            && self.irqs.keys().all(is(ObjectKind::Irq))
            && self.scheds.keys().all(is(ObjectKind::Sched))
            && self.timers.keys().all(is(ObjectKind::Timer))
            && self.endpoints.keys().all(is(ObjectKind::Endpoint))
            && self.replies.keys().all(is(ObjectKind::Reply))
            && self.notifies.keys().all(is(ObjectKind::Notify));
        ensure(of_its_kind, "10: state without an object")
    }

    fn check_waits(&self) -> Check {
        let wait = |thread: &ObjId| self.threads.get(thread).map(|thread| &thread.wait);
        let mut queued: Vec<ObjId> = Vec::new();
        for (&id, endpoint) in &self.endpoints {
            let here = |thread| match wait(thread) {
                Some(Wait::Call { endpoint, .. } | Wait::Recv { endpoint, .. }) => *endpoint == id,
                _ => false,
            };
            ensure(endpoint.queue.iter().all(here), "12: queue of an endpoint")?;
            let role = |thread| wait(thread).map(discriminant);
            let alike = endpoint
                .queue
                .iter()
                .all(|thread| role(thread) == role(&endpoint.queue[0]));
            ensure(alike, "12: callers and receivers together")?;
            queued.extend(&endpoint.queue);
            let bound = |notify| {
                self.notifies
                    .get(notify)
                    .is_some_and(|n| n.endpoint == Some(id))
            };
            ensure(
                endpoint.notify.iter().all(bound),
                "17: notify of an endpoint",
            )?;
        }
        for (&id, notify) in &self.notifies {
            let here = |thread| wait(thread) == Some(&Wait::Notify(id));
            ensure(notify.waiters.iter().all(here), "13: queue of a notify")?;
            let zero = notify.waiters.is_empty() || notify.word == 0;
            ensure(zero, "13: word of a notify with a waiter")?;
            queued.extend(&notify.waiters);
            let bound = |ep| {
                self.endpoints
                    .get(ep)
                    .is_some_and(|ep| ep.notify == Some(id))
            };
            ensure(
                notify.endpoint.iter().all(bound),
                "17: endpoint of a notify",
            )?;
        }
        let entries = queued.len();
        queued.sort();
        queued.dedup();
        ensure(queued.len() == entries, "14: a thread queued twice")?;
        let in_queue = |thread: &&Thread| {
            !matches!(thread.wait, Wait::None | Wait::Reply(_)) && !thread.bounded()
        };
        let waiting = self.threads.values().filter(in_queue).count();
        ensure(waiting == entries, "14: a waiting thread not queued")?;
        // A thread waiting for a unit is the waiter of its entry of the
        // unit request table, and an entry names as waiter only a thread
        // that waits for it: a wait that ended otherwise, or a thread
        // destroyed, leaves no name (ver2-AD D8).
        for (&id, thread) in &self.threads {
            if let Wait::Unit(index) = thread.wait {
                let entry = self.unit_requests.get(index).and_then(Option::as_ref);
                let named =
                    entry.is_some_and(|e| e.waiter == crate::units::Waiter::Thread(Some(id)));
                ensure(named, "14: a wait on no unit request")?;
            }
        }
        // The same for a hand-over's caller (Roles, `role_hand`, steps 1
        // to 4; audit lot 7: `Wait::Hand` was judged by no invariant).
        for (&id, thread) in &self.threads {
            if let Wait::Hand(role) = thread.wait {
                let named = self
                    .handing
                    .get(&role)
                    .is_some_and(|h| h.caller == Some(id));
                ensure(named, "14: a wait on no hand-over")?;
            }
        }
        for (&role, hand) in &self.handing {
            if let Some(thread) = hand.caller {
                ensure(
                    wait(&thread) == Some(&Wait::Hand(role)),
                    "14: a hand-over names a thread that does not wait for it",
                )?;
            }
        }
        for (index, entry) in self.unit_requests.iter().enumerate() {
            if let Some(crate::units::Waiter::Thread(Some(thread))) =
                entry.as_ref().map(|e| &e.waiter)
            {
                let waits = wait(thread) == Some(&Wait::Unit(index));
                ensure(
                    waits,
                    "14: a unit request names a thread that does not wait for it",
                )?;
            }
        }

        // A range in a destroyed space is no longer within any table.
        let within = |window: &Window| {
            let slots = self
                .cspaces
                .get(&window.cspace)
                .map(|space| space.slots.len());
            let end = window.first as usize + window.count as usize;
            let in_table = slots.is_none_or(|slots| window.first != 0 && end <= slots);
            window.count as usize <= MAX_CAPS && (window.count == 0 || in_table)
        };
        for (&id, state) in &self.replies {
            let idle = *state == ReplyState::Idle || !self.marked(id);
            ensure(idle, "22: thread of a dying reply object")?;
            if let ReplyState::Armed { window, flags, .. } = state {
                let known = flags & !Header::FLAG_CAPS_DROPPED == 0;
                ensure(
                    known && within(window),
                    "29: range or flags of a reply object",
                )?;
            }
            let agrees = match *state {
                ReplyState::Idle => true,
                ReplyState::Attached(thread) => {
                    matches!(wait(&thread), Some(Wait::Recv { reply, .. }) if *reply == id)
                }
                ReplyState::Armed { caller, .. } => wait(&caller) == Some(&Wait::Reply(id)),
            };
            ensure(agrees, "15: thread of a reply object")?;
        }
        for (&id, thread) in &self.threads {
            let state = |reply| self.replies.get(reply).copied();
            let agrees = match &thread.wait {
                Wait::Recv { reply, .. } => state(reply) == Some(ReplyState::Attached(id)),
                Wait::Reply(reply) => {
                    matches!(state(reply), Some(ReplyState::Armed { caller, .. }) if caller == id)
                }
                _ => true,
            };
            ensure(agrees, "15: reply object of a thread")?;
            // 16 (8.6, its strongest form): a thread that waits has no
            // outcome, unless it waits as a caller on a fault of kind 7 or 8
            // raised after its last call ended and before it ran again;
            // word 0 of that fault is the address after that call, the
            // thread's instruction pointer, which nothing has moved since.
            let ready = thread.wait == Wait::None;
            let raised = thread.faulting
                && thread.raised.is_some_and(|(kind, word)| {
                    let kernel = kind == FaultKind::NoAddressSpace as u32
                        || kind == FaultKind::NoContext as u32;
                    kernel && word == thread.regs[dodos_abi::x86_64::REG_RIP]
                });
            let waited = ready || thread.outcome.is_none() || raised;
            ensure(waited, "16: outcome while waiting")?;
            let outcome = match &thread.outcome {
                _ if thread.bounded() || thread.answered => true,
                None | Some(Err(Error::Gone | Error::Cancelled)) => true,
                Some(Ok(Output::Word(word))) => *word != 0,
                Some(Ok(Output::Message(delivery))) => {
                    let (got, sent) = (delivery.words.len(), delivery.received.header.words());
                    got == sent || got == REG_WORDS && sent > REG_WORDS
                }
                Some(_) => false,
            };
            ensure(outcome, "30: outcome of a wait")?;
            let number = match thread.number {
                0 => true,
                number => !ready && number > thread.cancelled_up_to,
            };
            ensure(number, "31: wait number of a thread")?;
            let limits = match &thread.wait {
                Wait::Call { parcel, window, .. } => {
                    let header = parcel.words.len() <= MAX_WORDS && parcel.caps.len() <= MAX_CAPS;
                    parcel.sender == id && header && within(window)
                }
                Wait::Recv { window, .. } => within(window),
                _ => true,
            };
            ensure(limits, "29: message or range of a waiting thread")?;
            let fault = match &thread.wait {
                Wait::Call { parcel, .. } => {
                    // Every kind of the table, the hardware breakpoint (9)
                    // included (ver-D M1).
                    let kind = FaultKind::from_raw(parcel.label).is_some();
                    kind && parcel.words.len() == FAULT_WORDS && parcel.caps.is_empty()
                }
                Wait::Reply(_) => true,
                _ => false,
            };
            ensure(!thread.faulting || fault, "40: fault wait")?;
        }
        Ok(())
    }

    fn check_memory(&self) -> Check {
        for (&id, untyped) in &self.untypeds {
            // An object counts once, and once more for each segment of a
            // capability space.
            let segments = |object: &ObjId| match self.cspaces.get(object) {
                Some(space) => space.slots.len() / CSPACE_SEGMENT_SLOTS as usize,
                None => 0,
            };
            let taken = self
                .objects
                .iter()
                .filter(|(_, object)| object.from == Some(id));
            let live: usize = taken.map(|(object, _)| 1 + segments(object)).sum();
            // The root table and the lower tables of a space are live
            // pieces of its table untyped; its header of its ordinary
            // untyped (Address spaces and their tables, D-154).
            let tables: usize = self
                .aspaces
                .values()
                .filter(|space| space.table_untyped == Some(id))
                .map(|space| 1 + space.tables as usize)
                .sum();
            let live = live + tables;
            ensure(
                self.startup_lost
                    .is_none_or(|(first, pieces)| first != id || untyped.lost >= pieces),
                "19: the start-up aspace's pieces returned to their untyped",
            )?;
            ensure(
                untyped.live as usize == live + untyped.lost as usize,
                "19: live count",
            )?;
            ensure(untyped.watermark <= untyped.bytes, "18: watermark within")?;
            ensure(
                untyped.watermark.is_multiple_of(8),
                "18: watermark on 8 bytes",
            )?;
            // A watermark ready to return waits, above 0, with nothing live.
            let zero = match untyped.ready.is_some() {
                true => untyped.watermark > 0,
                false => (untyped.watermark == 0) == (untyped.live == 0),
            };
            ensure(zero, "18: watermark is zero when nothing is live")?;
            ensure(
                untyped.ready.is_none() || untyped.live == 0,
                "18: a watermark ready to return under a live object",
            )?;
            // Device memory and a table untyped have no clean mark (D-154).
            let clean = match untyped.device || untyped.table {
                true => untyped.clean == 0,
                false => (untyped.watermark..=untyped.bytes).contains(&untyped.clean),
            };
            ensure(clean, "18: clean mark")?;
            // From the watermark to the clean mark, padding included, every
            // byte was cleared and taken by no object since.
            let mut at = untyped.watermark;
            let mut cleared = untyped.zero.clone();
            cleared.sort();
            for (start, end) in cleared {
                if start <= at && end > at {
                    at = end;
                }
            }
            ensure(
                untyped.device || at >= untyped.clean,
                "65: clean mark over memory not cleared",
            )?;
        }
        for (&id, frame) in &self.frames {
            let pages = (1..=u64::from(self.machine.geometry.entries)).contains(&frame.pages);
            let from = self.objects.get(&id).and_then(|object| object.from);
            let device =
                from.is_some_and(|from| self.untypeds.get(&from).is_some_and(|u| u.device));
            let write_back = frame.memory == MemoryType::WriteBack;
            // A remote frame is judged on its own node.
            let remote = self.remotes.contains_key(&id);
            ensure(pages && (remote || device != write_back), "34: frame")?;
        }
        for cap in self.caps.values() {
            let frame = self.frames.contains_key(&cap.object);
            ensure(
                frame || cap.record == Record::Empty,
                "34: record of another kind",
            )?;
        }
        Ok(())
    }

    fn check_bindings(&self) -> Check {
        for (&id, thread) in &self.threads {
            let once = |list: Option<&Vec<ObjId>>| {
                list.is_some_and(|list| list.iter().filter(|&&t| t == id).count() == 1)
            };
            let space = thread
                .cspace
                .map(|s| self.cspaces.get(&s).map(|s| &s.threads));
            ensure(space.is_none_or(once), "32: capability space of a thread")?;
            let aspace = thread
                .aspace
                .map(|a| self.aspaces.get(&a).map(|a| &a.threads));
            ensure(aspace.is_none_or(once), "32: address space of a thread")?;
            let fault = thread
                .fault
                .map(|(e, _)| self.endpoints.get(&e).map(|e| &e.threads));
            ensure(fault.is_none_or(once), "32: fault endpoint of a thread")?;
            let page = thread.page.map(|page| {
                let cap = self.caps.get(&page);
                let frame = cap.and_then(|cap| self.frames.get(&cap.object));
                let one = frame.is_some_and(|f| f.pages == 1 && f.page as u8 == 0);
                one && frame.is_some_and(|f| f.memory == MemoryType::WriteBack)
                    && cap.is_some_and(|cap| cap.record == Record::Binding(id))
            });
            ensure(page.is_none_or(|page| page), "33: message page of a thread")?;
        }
        let bound = |thread: &ObjId, wanted: fn(&Thread) -> Option<ObjId>, object: ObjId| {
            self.threads
                .get(thread)
                .is_some_and(|t| wanted(t) == Some(object))
        };
        for (&id, space) in &self.cspaces {
            let all = space.threads.iter().all(|t| bound(t, |t| t.cspace, id));
            ensure(all, "32: thread listed by a capability space")?;
        }
        for (&id, aspace) in &self.aspaces {
            let all = aspace.threads.iter().all(|t| bound(t, |t| t.aspace, id));
            ensure(all, "32: thread listed by an address space")?;
        }
        for (&id, endpoint) in &self.endpoints {
            let all = endpoint
                .threads
                .iter()
                .all(|t| bound(t, |t| t.fault.map(|f| f.0), id));
            ensure(all, "32: thread listed by a fault endpoint")?;
        }
        for (&id, cap) in &self.caps {
            if let Record::Binding(thread) = cap.record {
                let page = self.threads.get(&thread).and_then(|t| t.page);
                ensure(page == Some(id), "33: record of a message page")?;
            }
        }
        Ok(())
    }
}

mod cores;
mod debug;
mod machine;
mod memory;
mod node;
mod sched;
mod table;
mod tasks;
#[cfg(test)]
mod tests;
