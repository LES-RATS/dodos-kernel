//! Capabilities and objects: the derivation tree, removal, object creation.

use crate::{
    Access, CSpace, Cap, CapId, Endpoint, Model, Notify, ObjId, Object, Outcome, Output, Place,
    ReplyState, Slot, Sweep, Thread, Untyped, live,
};
use dodos_abi::{
    ASPACE_BYTES, CSPACE_DIRECTORY_BYTES, CSPACE_MAX_SLOTS, CSPACE_SEGMENT_SLOTS, ENDPOINT_BYTES,
    Error, LONGEST_WALK, NOTIFY_BYTES, ObjectKind, PageSize, REMOVAL_SLOTS, REMOVAL_STEP,
    REPLY_BYTES, Rights, SCHED_BYTES, SLOT_BYTES, SlotAddr, THREAD_BYTES, TREE_DEPTH,
};

/// Slots and addresses: one segment of a capability space.
const SEGMENT_BYTES: u64 = CSPACE_SEGMENT_SLOTS as u64 * SLOT_BYTES as u64;
/// Rules of `untyped_retype`: the smallest untyped, and the greatest
/// alignment of a kernel object.
const PAGE: u64 = PageSize::Size4K.bytes();

/// How a capability comes to a slot, for `Model::fits`.
#[derive(Clone, Copy)]
pub(crate) enum Placing {
    Moved,
    /// A new capability derived from the one named.
    Derived,
    /// The root capability of a new object, made from the untyped named.
    Retyped(ObjectKind),
}

/// What the sweep of a dying capability space found, for `Model::swept`.
enum Swept {
    /// A capability still in a slot: it is removed next.
    Holds(CapId),
    /// No slot is left to examine, or the capability is no root of a space.
    Empty,
    /// The slots that the call may still examine ran out.
    OutOfBudget,
}

/// What `cap_delete` and `cap_revoke` remove.
#[derive(Clone, Copy, PartialEq)]
enum Removal {
    Descendants,
    DescendantsThenSelf,
}

/// Capabilities and objects, with the table of Object sizes: the bytes and
/// the alignment of a new object, from the size argument of `untyped_retype`.
fn footprint(kind: ObjectKind, size: u64, page: PageSize) -> Result<(u64, u64), Error> {
    let segments = size / u64::from(CSPACE_SEGMENT_SLOTS);
    let (bytes, valid) = match kind {
        ObjectKind::Untyped => (size, size.is_power_of_two() && size >= PAGE),
        // Capabilities and objects: a size that does not fit in 64 bits is
        // `invalid`.
        ObjectKind::Frame => (
            size.checked_mul(page.bytes()).ok_or(Error::Invalid)?,
            size > 0,
        ),
        // Slots and addresses: a non-zero multiple of 128, at most 2^20, and
        // a directory rounded up to a power of two.
        ObjectKind::CSpace => (
            (segments * CSPACE_DIRECTORY_BYTES as u64).next_power_of_two(),
            size > 0
                && size.is_multiple_of(CSPACE_SEGMENT_SLOTS.into())
                && size <= CSPACE_MAX_SLOTS.into(),
        ),
        ObjectKind::ASpace => (ASPACE_BYTES as u64, size == 0),
        // Rules of `untyped_retype`: a thread in the one-core reference is
        // 1 KB.
        ObjectKind::Thread => (THREAD_BYTES as u64, size == 0),
        ObjectKind::Sched => (SCHED_BYTES as u64, size == 0),
        ObjectKind::Endpoint => (ENDPOINT_BYTES as u64, size == 0),
        ObjectKind::Reply => (REPLY_BYTES as u64, size == 0),
        ObjectKind::Notify => (NOTIFY_BYTES as u64, size == 0),
        // Order of checks, step 3: a kind that this version does not make,
        // the kinds of the groups not yet specified, is `unsupported`, after
        // the rule on device memory and before the size argument.
        _ => return Err(Error::Unsupported),
    };
    // Rules of `untyped_retype`: aligned on the size of a child untyped, on
    // the page size for a frame, and for the other objects on their size,
    // or on 4 KB if they are larger.
    let align = match kind {
        ObjectKind::Untyped => bytes,
        ObjectKind::Frame => page.bytes(),
        _ => bytes.min(PAGE),
    };
    valid.then_some((bytes, align)).ok_or(Error::Invalid)
}

impl Model {
    pub(crate) fn cap_derive(
        &mut self,
        thread: ObjId,
        source: SlotAddr,
        destination: SlotAddr,
        rights: Rights,
        badge: u64,
    ) -> Outcome {
        let parent = self.cap(thread, source, Access::Read)?;
        let source = &self.caps[&parent];
        let (object, held, kept) = (source.object, source.rights, source.badge);
        // Capabilities and objects: `derive` on the source.
        if !held.contains(Rights::DERIVE) {
            return Err(Error::Denied);
        }
        let place = self.destination(thread, destination)?;
        // Capabilities and objects: a rights word with a bit that names no
        // right is `invalid`, judged before the subset. The model takes
        // `Rights`, which has no such bit: whoever decodes the raw word
        // answers `invalid` before calling the model.
        // The new rights are a subset of the source's, checked with the
        // plain arguments. Order of checks, step
        // 3: in the order of the table, the rights before the badge.
        if !held.contains(rights) {
            return Err(Error::Denied);
        }
        // Capabilities and objects: a badge only on an endpoint or a notify.
        let kind = self.objects[&object].kind;
        if badge != 0 && kind != ObjectKind::Endpoint && kind != ObjectKind::Notify {
            return Err(Error::Invalid);
        }
        // Capabilities and objects: another badge is `denied`, checked with
        // the plain arguments; the same one is accepted.
        if badge != 0 && kept != 0 && badge != kept {
            return Err(Error::Denied);
        }
        // Order of checks, step 3: the depth checks come last.
        self.fits(place.0, parent, Placing::Derived)?;
        self.vacant(place)?;
        let badge = if kept != 0 { kept } else { badge };
        self.derive(parent, rights, badge, place);
        Ok(Output::Done)
    }

    pub(crate) fn cap_move(
        &mut self,
        thread: ObjId,
        source: SlotAddr,
        destination: SlotAddr,
    ) -> Outcome {
        let cap = self.cap(thread, source, Access::Write)?;
        let place = self.destination(thread, destination)?;
        self.fits(place.0, cap, Placing::Moved)?;
        self.vacant(place)?;
        self.place(cap, place);
        Ok(Output::Done)
    }

    pub(crate) fn cap_delete(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        // Removal ends: the one call left to a thread whose capability space
        // is dying, on its own slots only.
        let own = self.own(thread)?;
        if self.dying(own) && slot.via() != 0 {
            return Err(Error::Gone);
        }
        let space = self.through(own, slot, Access::Write)?;
        match self.cspaces[&space].slots.get(slot.index() as usize) {
            // Slots and addresses: `cap_delete` on a dead slot makes it
            // empty.
            Some(Slot::Dead) => {
                *self.slot((space, slot.index())) = Slot::Empty;
                Ok(Output::Done)
            }
            // Removal ends: from the first step the capability is dying,
            // and only `cap_delete` can still use it.
            Some(&Slot::Full(cap)) => {
                live(&mut self.caps, cap).dying = true;
                self.remove(cap, Removal::DescendantsThenSelf)
            }
            Some(Slot::Empty) | None => Err(Error::Invalid),
        }
    }

    pub(crate) fn cap_revoke(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let cap = self.cap(thread, slot, Access::Write)?;
        self.remove(cap, Removal::Descendants)
    }

    pub(crate) fn cap_inspect(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let cap = &self.caps[&self.cap(thread, slot, Access::Read)?];
        Ok(Output::Inspect {
            kind: self.objects[&cap.object].kind,
            rights: cap.rights,
            badge: cap.badge,
        })
    }

    pub(crate) fn untyped_retype(
        &mut self,
        thread: ObjId,
        untyped: SlotAddr,
        kind: ObjectKind,
        size: u64,
        page: PageSize,
        destination: SlotAddr,
    ) -> Outcome {
        let parent = self.cap(thread, untyped, Access::Read)?;
        let &Cap { object, rights, .. } = self.is(parent, ObjectKind::Untyped, Rights::WRITE)?;
        let place = self.destination(thread, destination)?;
        // Order of checks, step 3: the kind is judged before the size
        // argument. Capabilities and objects: device memory can only become
        // a frame or a child untyped.
        let device = self.untypeds[&object].device;
        if device && kind != ObjectKind::Frame && kind != ObjectKind::Untyped {
            return Err(Error::Invalid);
        }
        let (bytes, align) = footprint(kind, size, page)?;
        self.fits(place.0, parent, Placing::Retyped(kind))?;
        self.vacant(place)?;
        self.take(object, bytes, align)?;
        // Rules of `untyped_retype`: every right, but `exec` only if the
        // untyped capability has it, and never on device memory: the
        // start-up capabilities for device memory have no `exec`.
        let no_exec = Rights::EXEC.without(rights);
        self.create(
            kind,
            size,
            Some(parent),
            Rights::ALL.without(no_exec),
            place,
        );
        Ok(Output::Done)
    }

    pub(crate) fn cspace_grow(
        &mut self,
        thread: ObjId,
        cspace: SlotAddr,
        untyped: SlotAddr,
    ) -> Outcome {
        let space = self.object(thread, cspace, ObjectKind::CSpace, Rights::MANAGE)?;
        let untyped = self.object(thread, untyped, ObjectKind::Untyped, Rights::WRITE)?;
        // Slots and addresses: only from the untyped it was created from;
        // any capability for that untyped object is accepted. Errors:
        // `no_slot` also means that the space is at its maximum.
        // The derivation tree: the start-up space, created from no untyped,
        // never grows.
        if self.objects[&space].from != Some(untyped) {
            return Err(Error::Invalid);
        }
        let CSpace { max, slots, .. } = &self.cspaces[&space];
        if slots.len() >= *max as usize {
            return Err(Error::NoSlot);
        }
        // Rules of `untyped_retype`: a segment is 4 KB, aligned on its size.
        self.take(untyped, SEGMENT_BYTES, SEGMENT_BYTES)?;
        let space = live(&mut self.cspaces, space);
        let slots = space.slots.len() + CSPACE_SEGMENT_SLOTS as usize;
        space.slots.resize(slots, Slot::Empty);
        Ok(Output::Slots(slots as u32))
    }

    /// The derivation tree: a start-up capability is at depth 1.
    pub(crate) fn depth(&self, cap: CapId) -> usize {
        std::iter::successors(Some(cap), |cap| self.caps[cap].parent).count()
    }

    /// The derivation tree, checked with the plain arguments each time a
    /// capability is placed in a slot of `space`: the tree is at most 64
    /// deep, and a capability space holds only what is deeper than itself.
    /// The root capability of another space must be deeper than the root
    /// capability of `space`, and an untyped capability not less deep.
    pub(crate) fn fits(&self, space: ObjId, source: CapId, placing: Placing) -> Result<(), Error> {
        let own = self.objects[&space].root;
        let kind = self.objects[&self.caps[&source].object].kind;
        // A space may hold its own root capability; a copy is not a root.
        let moved_root = self.rooted(source).is_some() && source != own;
        let (kind, root, depth) = match placing {
            Placing::Moved => (kind, moved_root, self.depth(source)),
            Placing::Derived => (kind, false, self.depth(source) + 1),
            Placing::Retyped(kind) => (kind, true, self.depth(source) + 1),
        };
        let deep_enough = match kind {
            ObjectKind::Untyped => depth >= self.depth(own),
            ObjectKind::CSpace if root => depth > self.depth(own),
            _ => true,
        };
        (deep_enough && depth <= TREE_DEPTH)
            .then_some(())
            .ok_or(Error::Invalid)
    }

    /// Rules of `untyped_retype`: memory is taken at a watermark that only
    /// moves up, aligned.
    fn take(&mut self, untyped: ObjId, bytes: u64, align: u64) -> Result<(), Error> {
        let untyped = live(&mut self.untypeds, untyped);
        let start = untyped.watermark.checked_next_multiple_of(align);
        let end = start.and_then(|start| start.checked_add(bytes));
        untyped.watermark = end
            .filter(|&end| end <= untyped.bytes)
            .ok_or(Error::NoMemory)?;
        untyped.live += 1;
        Ok(())
    }

    /// Rules of `untyped_retype`: when the last object made from the untyped
    /// is destroyed the watermark returns to the start; this is the only
    /// moment the memory becomes usable again.
    fn release(&mut self, untyped: ObjId) {
        let untyped = live(&mut self.untypeds, untyped);
        untyped.live -= 1;
        if untyped.live == 0 {
            untyped.watermark = 0;
        }
    }

    /// A new object and its root capability, in an empty slot. The parent
    /// is the untyped capability the memory comes from; a capability given
    /// at start-up has none.
    pub(crate) fn create(
        &mut self,
        kind: ObjectKind,
        size: u64,
        parent: Option<CapId>,
        rights: Rights,
        place: Place,
    ) -> CapId {
        let id = ObjId(self.next_id);
        self.next_id += 1;
        let from = parent.map(|parent| self.caps[&parent].object);
        match kind {
            ObjectKind::Untyped => {
                let untyped = Untyped {
                    bytes: size,
                    device: from.is_some_and(|from| self.untypeds[&from].device),
                    watermark: 0,
                    live: 0,
                };
                self.untypeds.insert(id, untyped);
            }
            ObjectKind::CSpace => {
                // Slots and addresses: created with a maximum and no slot.
                // The one given at start-up has all its slots.
                let slots = if parent.is_none() { size as usize } else { 0 };
                let space = CSpace {
                    max: size as u32,
                    slots: vec![Slot::Empty; slots],
                    sweep: None,
                };
                self.cspaces.insert(id, space);
            }
            ObjectKind::Thread => {
                // Threads: a new thread is suspended and bound to nothing.
                let thread = Thread {
                    suspended: true,
                    ..Thread::default()
                };
                self.threads.insert(id, thread);
            }
            ObjectKind::Endpoint => {
                self.endpoints.insert(id, Endpoint::default());
            }
            ObjectKind::Reply => {
                self.replies.insert(id, ReplyState::Idle);
            }
            ObjectKind::Notify => {
                self.notifies.insert(id, Notify::default());
            }
            _ => {}
        }
        // The derivation tree: the capability that `untyped_retype` returns,
        // or the one given at start-up, is the root capability.
        let root = self.new_cap(id, rights, 0, parent, place);
        self.objects.insert(id, Object { kind, root, from });
        root
    }

    fn new_cap(
        &mut self,
        object: ObjId,
        rights: Rights,
        badge: u64,
        parent: Option<CapId>,
        place: Place,
    ) -> CapId {
        let id = CapId(self.next_id);
        self.next_id += 1;
        let cap = Cap {
            object,
            rights,
            badge,
            parent,
            children: Vec::new(),
            place,
            dying: false,
        };
        self.caps.insert(id, cap);
        if let Some(parent) = parent {
            live(&mut self.caps, parent).children.push(id);
        }
        *self.slot(place) = Slot::Full(id);
        id
    }

    /// The derivation tree: a new capability for the same object, child of
    /// `parent`, in an empty slot.
    pub(crate) fn derive(&mut self, parent: CapId, rights: Rights, badge: u64, place: Place) {
        self.new_cap(
            self.caps[&parent].object,
            rights,
            badge,
            Some(parent),
            place,
        );
    }

    /// The derivation tree: puts a capability in an empty slot and empties
    /// the slot it was in. Nothing else about it changes.
    pub(crate) fn place(&mut self, cap: CapId, place: Place) {
        let old = std::mem::replace(&mut live(&mut self.caps, cap).place, place);
        *self.slot(old) = Slot::Empty;
        *self.slot(place) = Slot::Full(cap);
    }

    fn slot(&mut self, (space, index): Place) -> &mut Slot {
        &mut live(&mut self.cspaces, space).slots[index as usize]
    }

    /// The derivation tree: removal in bounded steps. Per call, at most 32
    /// capabilities are removed or threads woken, and at most 128 slots of
    /// dying capability spaces are examined; `again` while work remains.
    /// `cap_revoke` on the root capability of a space starts below it, and
    /// so does not sweep the space.
    fn remove(&mut self, target: CapId, removal: Removal) -> Outcome {
        let (mut steps, mut slots) = (REMOVAL_STEP, REMOVAL_SLOTS);
        loop {
            let start = match removal {
                Removal::DescendantsThenSelf => target,
                Removal::Descendants => match self.caps[&target].children.last() {
                    Some(&child) => child,
                    None => return Ok(Output::Done),
                },
            };
            if steps == 0 {
                return Err(Error::Again);
            }
            steps -= 1;
            let next = self.next_removed(start, &mut slots).ok_or(Error::Again)?;
            if let Some(waiter) = self.waiter(next) {
                // The derivation tree: destroying an object wakes every
                // thread that waits on it, with `gone`. IPC: threads that
                // wait on the same object are served first in, first out.
                self.unlink(waiter);
                self.wake(waiter, Err(Error::Gone));
                continue;
            }
            // Slots and addresses: `cap_delete` acts on the slot it names,
            // which becomes empty; a capability removed by someone else
            // leaves a dead slot.
            let left = if next == target {
                Slot::Empty
            } else {
                Slot::Dead
            };
            self.remove_cap(next, left);
            if next == target {
                return Ok(Output::Done);
            }
        }
    }

    /// The derivation tree, the fixed order: the most recently derived child
    /// first, a capability only after all its descendants, and the root
    /// capability of a `cspace` only after the capabilities in it, in index
    /// order. `None` when the slots that the call may still examine run out
    /// before the next capability to remove is found.
    ///
    /// Removal ends: an object is dying from the first step of a removal
    /// that reaches its root capability, even when that step only removes a
    /// copy; and the step that marks a reply object wakes, with `gone`, the
    /// thread attached to it or the caller it is armed with. That wake is
    /// not counted in the 32 of a step.
    fn next_removed(&mut self, mut cap: CapId, slots: &mut usize) -> Option<CapId> {
        for walk in 1.. {
            // The derivation tree: the walk is shorter than 192. Twice the
            // longest is a walk that does not end: a fault of the model, not
            // a state to report.
            assert!(
                walk <= 2 * LONGEST_WALK,
                "the walk of a removal does not end"
            );
            self.longest_walk = self.longest_walk.max(walk);
            if let Some(object) = self.rooted(cap) {
                live(&mut self.caps, cap).dying = true;
                if let Some(space) = self.cspaces.get_mut(&object) {
                    // The derivation tree: the sweep of a space starts at
                    // slot 1.
                    space.sweep.get_or_insert_with(|| {
                        let held = space.slots.iter().filter_map(|slot| match *slot {
                            Slot::Full(held) => Some(held),
                            Slot::Empty | Slot::Dead => None,
                        });
                        Sweep {
                            next: 1,
                            held: held.collect(),
                        }
                    });
                }
                let waiter = match self.replies.get(&object) {
                    Some(&ReplyState::Attached(thread)) => Some(thread),
                    Some(&ReplyState::Armed { caller, .. }) => Some(caller),
                    Some(ReplyState::Idle) | None => None,
                };
                if let Some(waiter) = waiter {
                    self.unlink(waiter);
                    self.wake(waiter, Err(Error::Gone));
                }
            }
            match self.caps[&cap].children.last() {
                Some(&child) => cap = child,
                None => match self.swept(cap, slots) {
                    Swept::Holds(held) => cap = held,
                    Swept::Empty => break,
                    Swept::OutOfBudget => return None,
                },
            }
        }
        Some(cap)
    }

    /// The first capability left in the capability space that `cap` is the
    /// root capability of, if `cap` is one.
    ///
    /// The derivation tree: the dying space keeps the index where the sweep
    /// stands, and a slot examined once is not examined again. A space may
    /// hold its own root capability, which is removed last.
    fn swept(&mut self, cap: CapId, slots: &mut usize) -> Swept {
        let space = self
            .rooted(cap)
            .and_then(|space| self.cspaces.get_mut(&space));
        let Some(space) = space else {
            return Swept::Empty;
        };
        let sweep = space.sweep.as_mut().expect("the walk marked the space");
        // The derivation tree: a slot counts as examined when the sweep
        // moves past it, so a full slot is not counted while its capability
        // is being removed. A step may end on the slot budget alone.
        while let Some(&slot) = space.slots.get(sweep.next) {
            match slot {
                Slot::Full(held) if held != cap => return Swept::Holds(held),
                Slot::Full(_) | Slot::Empty | Slot::Dead => {
                    let Some(left) = slots.checked_sub(1) else {
                        return Swept::OutOfBudget;
                    };
                    *slots = left;
                    sweep.next += 1;
                }
            }
        }
        Swept::Empty
    }

    /// The object that `cap` is the root capability of, if it is one.
    fn rooted(&self, cap: CapId) -> Option<ObjId> {
        let object = self.caps[&cap].object;
        (self.objects[&object].root == cap).then_some(object)
    }

    /// The first thread in a queue of the object that `cap` is the root
    /// capability of.
    fn waiter(&self, cap: CapId) -> Option<ObjId> {
        let object = self.rooted(cap)?;
        match self.objects[&object].kind {
            ObjectKind::Endpoint => self.endpoints[&object].queue.front().copied(),
            ObjectKind::Notify => self.notifies[&object].waiters.front().copied(),
            _ => None,
        }
    }

    /// Removes a capability that has no descendant, and leaves `left` in
    /// its slot. The derivation tree: the object is destroyed with its root
    /// capability.
    fn remove_cap(&mut self, id: CapId, left: Slot) {
        let cap = self.caps.remove(&id).expect("names a live entry");
        if let Some(parent) = cap.parent {
            live(&mut self.caps, parent)
                .children
                .retain(|&child| child != id);
        }
        *self.slot(cap.place) = left;
        if self.objects[&cap.object].root == id {
            self.destroy(cap.object);
        }
    }

    /// Destroys an object on which no thread waits. Its memory goes back to
    /// the untyped it came from.
    fn destroy(&mut self, id: ObjId) {
        let object = self.objects.remove(&id).expect("names a live entry");
        match object.kind {
            ObjectKind::Untyped => {
                self.untypeds.remove(&id);
            }
            ObjectKind::CSpace => {
                // Slots and addresses: its segments die with it. The one
                // given at start-up took its segment from no untyped.
                let space = self.cspaces.remove(&id).expect("names a live entry");
                let segments = space.slots.len() / CSPACE_SEGMENT_SLOTS as usize;
                for untyped in object.from.iter().flat_map(|from| [*from].repeat(segments)) {
                    self.release(untyped);
                }
            }
            ObjectKind::Thread => {
                // Threads: a waiting thread leaves its queue when it is
                // destroyed. IPC: when a caller disappears the reply object
                // becomes idle.
                self.unlink(id);
                self.threads.remove(&id);
            }
            ObjectKind::Endpoint => {
                let endpoint = self.endpoints.remove(&id).expect("names a live entry");
                if let Some(notify) = endpoint.notify {
                    live(&mut self.notifies, notify).endpoint = None;
                }
            }
            ObjectKind::Reply => {
                self.replies.remove(&id);
            }
            ObjectKind::Notify => {
                let notify = self.notifies.remove(&id).expect("names a live entry");
                if let Some(endpoint) = notify.endpoint {
                    live(&mut self.endpoints, endpoint).notify = None;
                }
            }
            _ => {}
        }
        if let Some(untyped) = object.from {
            self.release(untyped);
        }
    }
}
