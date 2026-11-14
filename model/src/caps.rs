//! Capabilities and objects: the derivation tree, removal, object creation.

use crate::{
    Access, CSpace, Cap, CapId, Endpoint, Model, Notify, ObjId, Object, Outcome, Output, Place,
    ReplyState, Slot, Thread, Untyped, Wait, live,
};
use dodos_abi::{
    CSPACE_SEGMENT_SLOTS, Error, ObjectKind, PageSize, REMOVAL_STEP, Rights, SLOT_BYTES, SlotAddr,
};

/// Every right but `exec`.
pub(crate) const NO_EXEC: Rights = Rights::READ
    .union(Rights::WRITE)
    .union(Rights::SEND)
    .union(Rights::RECV)
    .union(Rights::GRANT)
    .union(Rights::DERIVE)
    .union(Rights::MANAGE);

/// Slots and addresses: one segment of a capability space.
const SEGMENT_BYTES: u64 = CSPACE_SEGMENT_SLOTS as u64 * SLOT_BYTES as u64;
/// Slots and addresses: the directory of a capability space, per segment.
const DIRECTORY_BYTES: u64 = 8;

/// What `cap_delete` and `cap_revoke` remove.
#[derive(Clone, Copy, PartialEq)]
enum Removal {
    Descendants,
    DescendantsThenSelf,
}

/// Capabilities and objects, with the table of Object sizes: the bytes and
/// the alignment of a new object, from the size argument of `untyped_retype`.
fn footprint(kind: ObjectKind, size: u64, page: PageSize) -> Result<(u64, u64), Error> {
    let directory = size / u64::from(CSPACE_SEGMENT_SLOTS) * DIRECTORY_BYTES;
    let (bytes, align, valid) = match kind {
        ObjectKind::Untyped => (size, size, size.is_power_of_two() && size >= 4096),
        ObjectKind::Frame => (size.saturating_mul(page.bytes()), page.bytes(), size > 0),
        // CONTRACT-GAP (Slots and addresses): the contract does not say
        // which maximums a `cspace` accepts. A maximum is a whole number of
        // segments, at least one, and fits the 32 bits of an index.
        ObjectKind::CSpace => (
            directory,
            directory,
            size > 0 && size.is_multiple_of(CSPACE_SEGMENT_SLOTS.into()) && size <= u32::MAX.into(),
        ),
        ObjectKind::ASpace => (4096, 4096, size == 0),
        ObjectKind::Thread => (1024, 1024, size == 0),
        ObjectKind::Sched => (128, 128, size == 0),
        ObjectKind::Endpoint | ObjectKind::Reply | ObjectKind::Notify => (64, 64, size == 0),
        // CONTRACT-GAP (Capabilities and objects): the contract does not
        // list the kinds that `untyped_retype` makes. The kinds outside the
        // one-core model are `unsupported`.
        _ => return Err(Error::Unsupported),
    };
    if valid {
        Ok((bytes, align))
    } else {
        Err(Error::Invalid)
    }
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
        let (object, kept) = (source.object, source.badge);
        // Capabilities and objects: `derive` on the source, and the new
        // rights are a subset of the source's.
        let rights = source.rights.derive(rights)?;
        let place = self.destination(thread, destination)?;
        // Capabilities and objects: a badge only on an endpoint or a notify.
        let kind = self.objects[&object].kind;
        if badge != 0 && kind != ObjectKind::Endpoint && kind != ObjectKind::Notify {
            return Err(Error::Invalid);
        }
        // CONTRACT-GAP (Order of checks): the contract does not place the
        // `denied` of a second badge. It is checked with the plain
        // arguments, and asking again for the badge already there is
        // accepted.
        if badge != 0 && kept != 0 && badge != kept {
            return Err(Error::Denied);
        }
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
        self.vacant(place)?;
        self.place(cap, place);
        Ok(Output::Done)
    }

    pub(crate) fn cap_delete(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let space = self.space(thread, slot, Access::Write)?;
        // Slots and addresses: `cap_delete` on a dead slot makes it empty.
        if self.cspaces[&space].slots.get(slot.index() as usize) == Some(&Slot::Dead) {
            *self.slot((space, slot.index())) = Slot::Empty;
            return Ok(Output::Done);
        }
        let cap = self.full(space, slot.index())?;
        self.remove(cap, Removal::DescendantsThenSelf)
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
        let (bytes, align) = footprint(kind, size, page)?;
        // Capabilities and objects: device memory can only become a frame
        // or a child untyped.
        let device = self.untypeds[&object].device;
        if device && kind != ObjectKind::Frame && kind != ObjectKind::Untyped {
            return Err(Error::Invalid);
        }
        self.vacant(place)?;
        self.take(object, bytes, align)?;
        // Rules of `untyped_retype`: every right, but `exec` only if the
        // untyped capability has it. Never on device memory: the start-up
        // capability for device memory has no `exec`.
        let exec = rights.contains(Rights::EXEC);
        let rights = if exec { Rights::ALL } else { NO_EXEC };
        let cap = self.create(kind, size, Some(parent), rights);
        self.place(cap, place);
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
        // Capabilities and objects: device memory never holds kernel data.
        if self.untypeds[&untyped].device {
            return Err(Error::Invalid);
        }
        let CSpace { max, slots, .. } = &self.cspaces[&space];
        if slots.len() >= *max as usize {
            return Err(Error::NoSlot);
        }
        // CONTRACT-GAP (Slots and addresses): the contract does not say what
        // ties a segment to the untyped it came from. Here the segment
        // counts as an object taken from the untyped until the capability
        // space is destroyed, and destroying the untyped first changes
        // nothing for the capability space.
        self.take(untyped, SEGMENT_BYTES, SEGMENT_BYTES)?;
        let space = live(&mut self.cspaces, space);
        let slots = space.slots.len() + CSPACE_SEGMENT_SLOTS as usize;
        space.slots.resize(slots, Slot::Empty);
        space.segments.push(untyped);
        Ok(Output::Slots(slots as u32))
    }

    /// Rules of `untyped_retype`: memory is taken at a watermark that only
    /// moves up, aligned on the size of the object.
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

    /// Rules of `untyped_retype`: the watermark returns to zero when the
    /// last object taken from the untyped is destroyed.
    fn release(&mut self, untyped: ObjId) {
        if let Some(untyped) = self.untypeds.get_mut(&untyped) {
            untyped.live -= 1;
            if untyped.live == 0 {
                untyped.watermark = 0;
            }
        }
    }

    /// A new object and its root capability, not yet in a slot. The parent
    /// is the untyped capability the memory comes from.
    pub(crate) fn create(
        &mut self,
        kind: ObjectKind,
        size: u64,
        parent: Option<CapId>,
        rights: Rights,
    ) -> CapId {
        let id = ObjId(self.next_id);
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
                let space = CSpace {
                    max: size as u32,
                    slots: Vec::new(),
                    segments: Vec::new(),
                };
                self.cspaces.insert(id, space);
            }
            ObjectKind::Thread => {
                // Threads: a new thread is suspended and bound to nothing.
                let thread = Thread {
                    cspace: None,
                    suspended: true,
                    wait: Wait::None,
                    outcome: None,
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
        self.next_id += 1;
        let root = self.new_cap(id, rights, 0, parent);
        // CONTRACT-GAP (The derivation tree): the contract defines the root
        // capability only for `untyped_retype`. A start-up capability is the
        // root capability of its object too.
        self.objects.insert(id, Object { kind, root, from });
        root
    }

    fn new_cap(
        &mut self,
        object: ObjId,
        rights: Rights,
        badge: u64,
        parent: Option<CapId>,
    ) -> CapId {
        let id = CapId(self.next_id);
        self.next_id += 1;
        let cap = Cap {
            object,
            rights,
            badge,
            parent,
            children: Vec::new(),
            place: None,
        };
        self.caps.insert(id, cap);
        if let Some(parent) = parent {
            live(&mut self.caps, parent).children.push(id);
        }
        id
    }

    /// The derivation tree: a new capability for the same object, child of
    /// `parent`, in an empty slot.
    pub(crate) fn derive(&mut self, parent: CapId, rights: Rights, badge: u64, place: Place) {
        let cap = self.new_cap(self.caps[&parent].object, rights, badge, Some(parent));
        self.place(cap, place);
    }

    /// The derivation tree: puts a capability in an empty slot and empties
    /// the slot it was in. Nothing else about it changes.
    pub(crate) fn place(&mut self, cap: CapId, place: Place) {
        if let Some(old) = live(&mut self.caps, cap).place.replace(place) {
            *self.slot(old) = Slot::Empty;
        }
        *self.slot(place) = Slot::Full(cap);
    }

    fn slot(&mut self, (space, index): Place) -> &mut Slot {
        &mut live(&mut self.cspaces, space).slots[index as usize]
    }

    /// The derivation tree: removal in bounded steps. Each capability
    /// removed and each thread woken is one step.
    fn remove(&mut self, target: CapId, removal: Removal) -> Outcome {
        let mut steps = REMOVAL_STEP;
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
            let next = self.next_removed(start);
            if let Some(waiter) = self.waiter(next) {
                // The derivation tree: destroying an object wakes every
                // thread that waits on it, with `gone`.
                self.unlink(waiter);
                self.wake(waiter, Err(Error::Gone));
                continue;
            }
            // Slots and addresses: the slot named by `cap_delete` becomes
            // empty; a capability removed by someone else leaves a dead slot.
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
    /// order.
    fn next_removed(&self, start: CapId) -> CapId {
        let mut path = vec![start];
        loop {
            let cap = path[path.len() - 1];
            let child = self.caps[&cap].children.last().copied();
            match child.or_else(|| self.swept(cap, &path)) {
                Some(next) => path.push(next),
                None => return cap,
            }
        }
    }

    /// The first capability to remove from the capability space that `cap`
    /// is the root capability of, if there is one.
    fn swept(&self, cap: CapId, path: &[CapId]) -> Option<CapId> {
        let space = self.cspaces.get(&self.rooted(cap)?)?;
        // CONTRACT-GAP (The derivation tree): a capability space can hold
        // its own root capability, or an ancestor of it, or the root
        // capability of a capability space that holds this one's: removing
        // "every capability in it" first would never end. A capability that
        // is on the path of the removal under way, or an ancestor of one, is
        // skipped. If it is still there when the capability space is
        // destroyed it stays in the tree without a slot: it can no longer be
        // named, and it is removed with the capability it was derived from.
        let on_path = |held: CapId| path.iter().any(|&below| self.is_ancestor(held, below));
        space.slots.iter().find_map(|slot| match *slot {
            Slot::Full(held) if !on_path(held) => Some(held),
            _ => None,
        })
    }

    /// Whether `cap` is `of` or one of its ancestors.
    fn is_ancestor(&self, cap: CapId, of: CapId) -> bool {
        std::iter::successors(Some(of), |below| self.caps[below].parent).any(|above| above == cap)
    }

    /// The object that `cap` is the root capability of, if it is one.
    fn rooted(&self, cap: CapId) -> Option<ObjId> {
        let object = self.caps[&cap].object;
        (self.objects[&object].root == cap).then_some(object)
    }

    /// The first thread that waits on the object that `cap` is the root
    /// capability of.
    fn waiter(&self, cap: CapId) -> Option<ObjId> {
        let object = self.rooted(cap)?;
        match self.objects[&object].kind {
            ObjectKind::Endpoint => self.endpoints[&object].queue.front().copied(),
            ObjectKind::Notify => self.notifies[&object].waiters.front().copied(),
            ObjectKind::Reply => match self.replies[&object] {
                ReplyState::Idle => None,
                // CONTRACT-GAP (The derivation tree): the contract does not
                // say what destroying an attached reply object does. The
                // thread that waits to receive with it is woken with `gone`.
                ReplyState::Attached(thread) => Some(thread),
                ReplyState::Armed { caller, .. } => Some(caller),
            },
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
        if let Some(place) = cap.place {
            *self.slot(place) = left;
        }
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
                let space = self.cspaces.remove(&id).expect("names a live entry");
                for slot in space.slots {
                    // Only the capabilities that `swept` skips are left.
                    if let Slot::Full(cap) = slot {
                        live(&mut self.caps, cap).place = None;
                    }
                }
                for untyped in space.segments {
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
