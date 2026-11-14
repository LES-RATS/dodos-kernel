//! Capabilities and objects: the derivation tree, removal, object creation.

use crate::{
    Access, CSpace, Cap, CapId, Endpoint, Model, Notify, ObjId, Object, Outcome, Output, Place,
    ReplyState, Slot, Thread, Untyped, Wait, live,
};
use dodos_abi::{
    CSPACE_SEGMENT_SLOTS, Error, ObjectKind, PageSize, REMOVAL_STEP, Rights, SLOT_BYTES, SlotAddr,
    TREE_DEPTH,
};
use std::collections::BTreeSet;

/// Slots and addresses: one segment of a capability space.
const SEGMENT_BYTES: u64 = CSPACE_SEGMENT_SLOTS as u64 * SLOT_BYTES as u64;
/// Slots and addresses: the directory of a capability space, per segment.
const DIRECTORY_BYTES: u64 = 8;
/// Rules of `untyped_retype`: the greatest alignment of an object.
const PAGE: u64 = 4096;

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
        ObjectKind::Frame => (size.saturating_mul(page.bytes()), size > 0),
        // Slots and addresses: a non-zero multiple of 128, and a directory
        // rounded up to a power of two. An index has 32 bits.
        ObjectKind::CSpace => (
            (segments * DIRECTORY_BYTES).next_power_of_two(),
            size > 0 && size.is_multiple_of(CSPACE_SEGMENT_SLOTS.into()) && size <= u32::MAX.into(),
        ),
        ObjectKind::ASpace => (PAGE, size == 0),
        ObjectKind::Thread => (1024, size == 0),
        ObjectKind::Sched => (128, size == 0),
        ObjectKind::Endpoint | ObjectKind::Reply | ObjectKind::Notify => (64, size == 0),
        // CONTRACT-GAP (Capabilities and objects): the contract does not
        // list the kinds that `untyped_retype` makes. The kinds outside the
        // one-core model are `unsupported`.
        _ => return Err(Error::Unsupported),
    };
    // Rules of `untyped_retype`: aligned on the size of the object, or on
    // 4 KB if the object is larger, and on the page size for a frame.
    let align = if kind == ObjectKind::Frame {
        page.bytes()
    } else {
        bytes.min(PAGE)
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
        // Capabilities and objects: another badge is `denied`, checked with
        // the plain arguments; the same one is accepted.
        if badge != 0 && kept != 0 && badge != kept {
            return Err(Error::Denied);
        }
        self.shallow(parent)?;
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
        self.movable(cap, place.0)?;
        self.vacant(place)?;
        self.place(cap, place);
        Ok(Output::Done)
    }

    pub(crate) fn cap_delete(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let space = self.space(thread, slot, Access::Write)?;
        match self.cspaces[&space].slots.get(slot.index() as usize) {
            // Slots and addresses: `cap_delete` on a dead slot makes it
            // empty.
            Some(Slot::Dead) => {
                *self.slot((space, slot.index())) = Slot::Empty;
                Ok(Output::Done)
            }
            // The derivation tree: from the first step the capability is
            // dying, and only `cap_delete` can still use it.
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
        let (bytes, align) = footprint(kind, size, page)?;
        // Capabilities and objects: device memory can only become a frame
        // or a child untyped.
        let device = self.untypeds[&object].device;
        if device && kind != ObjectKind::Frame && kind != ObjectKind::Untyped {
            return Err(Error::Invalid);
        }
        self.shallow(parent)?;
        self.vacant(place)?;
        self.take(object, bytes, align)?;
        // Rules of `untyped_retype`: every right, but `exec` only if the
        // untyped capability has it. Never on device memory: the start-up
        // capability for device memory has no `exec`.
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
        // Slots and addresses: only from the untyped it was created from.
        // CONTRACT-GAP (Slots and addresses): the capability space given at
        // start-up was created from no untyped. It never grows.
        if self.objects[&space].from != Some(untyped) {
            return Err(Error::Invalid);
        }
        let CSpace { max, slots } = &self.cspaces[&space];
        if slots.len() >= *max as usize {
            return Err(Error::NoSlot);
        }
        self.take(untyped, SEGMENT_BYTES, SEGMENT_BYTES)?;
        let space = live(&mut self.cspaces, space);
        let slots = space.slots.len() + CSPACE_SEGMENT_SLOTS as usize;
        space.slots.resize(slots, Slot::Empty);
        Ok(Output::Slots(slots as u32))
    }

    /// The derivation tree: the tree is at most 64 deep. `invalid` if a
    /// child of `parent` would be deeper.
    pub(crate) fn shallow(&self, parent: CapId) -> Result<(), Error> {
        // CONTRACT-GAP (Order of checks): the contract does not place this
        // `invalid`, nor the one of `movable`. Both are checked with the
        // plain arguments. A capability given at start-up is at depth 1.
        let above = |cap: &CapId| self.caps[cap].parent;
        if std::iter::successors(Some(parent), above).count() >= TREE_DEPTH {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// The derivation tree: a capability cannot be moved into a capability
    /// space that descends from it (`invalid`).
    pub(crate) fn movable(&self, cap: CapId, space: ObjId) -> Result<(), Error> {
        // CONTRACT-GAP (The derivation tree): the rule of the contract does
        // not stop every removal that would never end. A capability space
        // that descends from nothing of `cap` can still wait for it: the
        // root capability of space A in space B and that of B in A, or `cap`
        // in a space whose root capability is held in a space that descends
        // from `cap`. Here the move is `invalid` whenever the root
        // capability of the destination must be removed before `cap`: `cap`
        // is reached from it by going, any number of times, from a
        // capability to its parent or to the root capability of the space
        // that holds it. A capability space may hold its own root
        // capability, as the one given at start-up does.
        let mut later = vec![self.objects[&space].root];
        let mut seen = BTreeSet::new();
        while let Some(next) = later.pop() {
            if !seen.insert(next) {
                continue;
            }
            let holder = self.objects[&self.caps[&next].place.0].root;
            let own = (holder != next).then_some(holder);
            for after in self.caps[&next].parent.into_iter().chain(own) {
                if after == cap {
                    return Err(Error::Invalid);
                }
                later.push(after);
            }
        }
        Ok(())
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

    /// Rules of `untyped_retype`: the watermark returns to zero when the
    /// last object taken from the untyped is destroyed.
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
    fn next_removed(&self, mut cap: CapId) -> CapId {
        loop {
            let child = self.caps[&cap].children.last().copied();
            match child.or_else(|| self.swept(cap)) {
                Some(next) => cap = next,
                None => return cap,
            }
        }
    }

    /// The first capability to remove from the capability space that `cap`
    /// is the root capability of, if there is one.
    fn swept(&self, cap: CapId) -> Option<CapId> {
        let space = self.cspaces.get(&self.rooted(cap)?)?;
        // CONTRACT-GAP (The derivation tree): the contract does not say
        // what "every capability in it" is for a capability space that
        // holds its own root capability, as the one given at start-up does.
        // That capability is not removed first: it goes last, as the root
        // capability.
        space.slots.iter().find_map(|slot| match *slot {
            Slot::Full(held) if held != cap => Some(held),
            _ => None,
        })
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
