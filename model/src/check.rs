//! The invariants of the state.

use crate::{CapId, Model, ObjId, Object, ReplyState, Slot, Thread, Wait};
use dodos_abi::{CSPACE_SEGMENT_SLOTS, ObjectKind};
use std::mem::discriminant;

/// `Err` names the invariant that fails, by its number in `Model::check`.
type Check = Result<(), &'static str>;

fn ensure(holds: bool, invariant: &'static str) -> Check {
    if holds { Ok(()) } else { Err(invariant) }
}

impl Model {
    /// Checks every invariant of the state and names the first that fails.
    ///
    /// The derivation tree:
    /// 1. a capability's parent exists, is older than it (so the tree has no
    ///    cycle) and lists it once among its children;
    /// 2. a capability's children exist, name it as their parent, and are in
    ///    the order of derivation;
    /// 3. a capability derived from another has a subset of its rights, and
    ///    its badge if it has one (a root capability is a child of an untyped
    ///    capability and has every right, whatever the rights of that one);
    /// 4. a capability is the root capability of its object, or was derived
    ///    from a capability for the same object.
    ///
    /// Slots:
    /// 5. slot 0 of every capability space is empty;
    /// 6. every full slot names a live capability that names that slot, and
    ///    every capability that names a slot is in it;
    /// 7. a capability space has a whole number of segments, at most its
    ///    maximum.
    ///
    /// Objects:
    /// 8. every capability names a live object;
    /// 9. an object exists if and only if its root capability does: its root
    ///    capability is live and names it (with 4 and 8);
    /// 10. an object has state of its kind, and no state is left without its
    ///     object;
    /// 11. an object taken from an untyped names a live untyped, and its root
    ///     capability is a child of a capability for that untyped.
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
    /// 16. a thread that waits has no outcome yet;
    /// 17. a notify names its endpoint if and only if the endpoint names it.
    ///
    /// Memory:
    /// 18. the watermark of an untyped is within it, and is zero if and only
    ///     if nothing taken from it is live;
    /// 19. the live count of an untyped is the number of live objects and
    ///     cspace segments taken from it.
    pub fn check(&self) -> Check {
        self.check_tree()?;
        self.check_slots()?;
        self.check_objects()?;
        self.check_waits()?;
        self.check_memory()
    }

    fn check_tree(&self) -> Check {
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
                let kept = above.badge == 0 || above.badge == cap.badge;
                ensure(!derived || kept, "3: badge of a derivation")?;
            }
            let named = |child: &CapId| self.caps.get(child).is_some_and(|c| c.parent == Some(id));
            ensure(cap.children.iter().all(named), "2: children name it")?;
            ensure(cap.children.is_sorted(), "2: order of derivation")?;
            ensure(object.root == id || derived, "4: root or derivation")?;
            if let Some((space, index)) = cap.place {
                let slots = self.cspaces.get(&space).map(|space| &space.slots);
                let slot = slots.and_then(|slots| slots.get(index as usize));
                ensure(slot == Some(&Slot::Full(id)), "6: slot of a capability")?;
            }
        }
        Ok(())
    }

    fn check_slots(&self) -> Check {
        for (&id, space) in &self.cspaces {
            let first = space.slots.first();
            ensure(first.is_none_or(|slot| *slot == Slot::Empty), "5: slot 0")?;
            for (index, slot) in space.slots.iter().enumerate() {
                if let Slot::Full(cap) = slot {
                    let place = self.caps.get(cap).and_then(|cap| cap.place);
                    let here = place == Some((id, index as u32));
                    ensure(here, "6: capability of a full slot")?;
                }
            }
            let slots = space.slots.len();
            let whole = slots.is_multiple_of(CSPACE_SEGMENT_SLOTS as usize);
            ensure(whole && slots <= space.max as usize, "7: segments")?;
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
            let has_state = match object.kind {
                ObjectKind::Untyped => self.untypeds.contains_key(&id),
                ObjectKind::CSpace => self.cspaces.contains_key(&id),
                ObjectKind::Thread => self.threads.contains_key(&id),
                ObjectKind::Endpoint => self.endpoints.contains_key(&id),
                ObjectKind::Reply => self.replies.contains_key(&id),
                ObjectKind::Notify => self.notifies.contains_key(&id),
                _ => true,
            };
            ensure(has_state, "10: state of an object")?;
            if let Some(from) = object.from {
                ensure(self.untypeds.contains_key(&from), "11: untyped is live")?;
                let parent = root.parent.and_then(|parent| self.caps.get(&parent));
                let child = parent.is_some_and(|parent| parent.object == from);
                ensure(child, "11: root capability is a child of the untyped's")?;
            }
        }
        let untypeds = self.untypeds.keys().chain(self.cspaces.keys());
        let waited = self.endpoints.keys().chain(self.replies.keys());
        let states = untypeds
            .chain(waited)
            .chain(self.threads.keys())
            .chain(self.notifies.keys());
        let mut owned = states.map(|id| self.objects.contains_key(id));
        ensure(owned.all(|owned| owned), "10: state without an object")
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
        let in_queue = |thread: &&Thread| !matches!(thread.wait, Wait::None | Wait::Reply(_));
        let waiting = self.threads.values().filter(in_queue).count();
        ensure(waiting == entries, "14: a waiting thread not queued")?;

        for (&id, state) in &self.replies {
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
            let ready = thread.wait == Wait::None;
            ensure(
                ready || thread.outcome.is_none(),
                "16: outcome while waiting",
            )?;
        }
        Ok(())
    }

    fn check_memory(&self) -> Check {
        for (&id, untyped) in &self.untypeds {
            let taken = |from: &&Object| from.from == Some(id);
            let segments = self.cspaces.values().flat_map(|space| &space.segments);
            let live = self.objects.values().filter(taken).count()
                + segments.filter(|&&from| from == id).count();
            ensure(untyped.live as usize == live, "19: live count")?;
            ensure(untyped.watermark <= untyped.bytes, "18: watermark within")?;
            let zero = (untyped.watermark == 0) == (untyped.live == 0);
            ensure(zero, "18: watermark is zero when nothing is live")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
