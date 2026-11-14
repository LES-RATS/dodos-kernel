use std::collections::BTreeSet;

use dodos_abi::{Error, ObjectKind, Rights};

use crate::state::{
    CapEntry, EdgeKind, EndpointObject, KernelObject, KernelState, ReplyState, Slot, ThreadState,
};
use crate::types::{
    CallOutcome, CallResult, CallValue, CapId, CapInfo, ObjectId, SlotIndex, ThreadId, Wakeup,
};

/// Looks up a live capability in a specific `CSpace` and verifies `required` rights.
pub(crate) fn lookup_cap_in_cspace(
    state: &KernelState,
    cspace_id: ObjectId,
    slot: SlotIndex,
    required: Rights,
) -> Result<(CapId, CapEntry), Error> {
    let Some(KernelObject::CSpace(cspace)) = state.objects.get(&cspace_id) else {
        return Err(Error::Gone);
    };
    let Some(&slot_state) = cspace.slots.get(slot) else {
        return Err(Error::Invalid);
    };
    let cap_id = match slot_state {
        Slot::Empty => return Err(Error::Invalid),
        Slot::Revoked => return Err(Error::Gone),
        Slot::Live(id) => id,
    };
    let Some(entry) = state.caps.get(&cap_id) else {
        return Err(Error::Gone);
    };
    if !entry.rights.contains(required) {
        return Err(Error::Denied);
    }
    Ok((cap_id, entry.clone()))
}

/// Resolves a `CSpace` capability slot in the caller's `CSpace`.
pub(crate) fn lookup_cspace_cap(
    state: &KernelState,
    caller_cspace: ObjectId,
    cspace_slot: SlotIndex,
    required: Rights,
) -> Result<ObjectId, Error> {
    let (_, entry) = lookup_cap_in_cspace(state, caller_cspace, cspace_slot, required)?;
    match state.objects.get(&entry.object) {
        Some(KernelObject::CSpace(_)) => Ok(entry.object),
        Some(_) => Err(Error::Invalid),
        None => Err(Error::Gone),
    }
}

/// Checks that `dst_slot` in `dst_cspace` is in bounds and `Slot::Empty`.
pub(crate) fn check_dst_slot_empty(
    state: &KernelState,
    dst_cspace: ObjectId,
    dst_slot: SlotIndex,
) -> Result<(), Error> {
    let Some(KernelObject::CSpace(cspace)) = state.objects.get(&dst_cspace) else {
        return Err(Error::Gone);
    };
    match cspace.slots.get(dst_slot) {
        None => Err(Error::Invalid),
        Some(Slot::Empty) => Ok(()),
        Some(Slot::Live(_)) | Some(Slot::Revoked) => Err(Error::NoSlot),
    }
}

/// Counts live capabilities pointing to `object_id`.
pub(crate) fn cap_count_for_object(state: &KernelState, object_id: ObjectId) -> usize {
    state
        .caps
        .values()
        .filter(|entry| entry.object == object_id)
        .count()
}

pub(crate) struct NewCap {
    pub dst_cspace: ObjectId,
    pub dst_slot: SlotIndex,
    pub object: ObjectId,
    pub rights: Rights,
    pub badge: u64,
    pub parent: Option<CapId>,
    pub edge: EdgeKind,
}

/// Inserts a new `CapEntry` into `state.caps`, `dst_cspace`, and its parent's `children`.
pub(crate) fn insert_cap(state: &mut KernelState, new_cap: NewCap) -> CapId {
    let cap_id = state.alloc_cap_id();
    if let Some(KernelObject::CSpace(cs)) = state.objects.get_mut(&new_cap.dst_cspace) {
        cs.slots[new_cap.dst_slot] = Slot::Live(cap_id);
    }
    state.caps.insert(
        cap_id,
        CapEntry {
            location: (new_cap.dst_cspace, new_cap.dst_slot),
            object: new_cap.object,
            rights: new_cap.rights,
            badge: new_cap.badge,
            parent: new_cap.parent,
            edge: new_cap.edge,
            children: Vec::new(),
        },
    );
    if let Some(parent_id) = new_cap.parent
        && let Some(parent_entry) = state.caps.get_mut(&parent_id)
    {
        parent_entry.children.push(cap_id);
    }
    cap_id
}

/// Moves an existing `CapId` from `(src_cspace, src_slot)` to `(dst_cspace, dst_slot)`.
pub(crate) fn move_cap_location(
    state: &mut KernelState,
    cap_id: CapId,
    dst_cspace: ObjectId,
    dst_slot: SlotIndex,
) {
    let Some(entry) = state.caps.get_mut(&cap_id) else {
        return;
    };
    let (old_cspace, old_slot) = entry.location;
    entry.location = (dst_cspace, dst_slot);
    if let Some(KernelObject::CSpace(cs)) = state.objects.get_mut(&old_cspace) {
        cs.slots[old_slot] = Slot::Empty;
    }
    if let Some(KernelObject::CSpace(cs)) = state.objects.get_mut(&dst_cspace) {
        cs.slots[dst_slot] = Slot::Live(cap_id);
    }
}

/// Implements `cap_derive`.
pub(crate) fn cap_derive(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    src: (SlotIndex, SlotIndex),
    dst: (SlotIndex, SlotIndex),
    wanted_rights: Rights,
    requested_badge: u64,
) -> CallOutcome {
    let (src_cspace_slot, src_slot) = src;
    let (dst_cspace_slot, dst_slot) = dst;
    // CONTRACT-GAP: Required rights on the containing CSpaces are not fixed by
    // `kernel-interface.md`; we require `READ` on `src_cspace` and `WRITE` on `dst_cspace`.
    let src_cspace = match lookup_cspace_cap(state, caller_cspace, src_cspace_slot, Rights::READ) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let dst_cspace = match lookup_cspace_cap(state, caller_cspace, dst_cspace_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let (src_cap_id, src_entry) =
        match lookup_cap_in_cspace(state, src_cspace, src_slot, Rights::DERIVE) {
            Ok(v) => v,
            Err(e) => return CallOutcome::err(e),
        };
    let new_rights = match src_entry.rights.derive(wanted_rights) {
        Ok(r) => r,
        Err(e) => return CallOutcome::err(e),
    };

    // CONTRACT-GAP: `kernel-interface.md` does not state which object kinds may
    // carry a badge or whether a badged capability can be re-badged. We allow
    // setting a non-zero badge only on `Endpoint` and `Notify`, and forbid
    // changing or stripping an already non-zero badge (`Error::Denied`).
    let obj_kind = match state.objects.get(&src_entry.object) {
        Some(obj) => obj.kind(),
        None => return CallOutcome::err(Error::Gone),
    };
    let new_badge = if src_entry.badge != 0 {
        if requested_badge != 0 && requested_badge != src_entry.badge {
            return CallOutcome::err(Error::Denied);
        }
        src_entry.badge
    } else if requested_badge != 0 {
        match obj_kind {
            ObjectKind::Endpoint | ObjectKind::Notify => requested_badge,
            _ => return CallOutcome::err(Error::Invalid),
        }
    } else {
        0
    };

    if let Err(e) = check_dst_slot_empty(state, dst_cspace, dst_slot) {
        return CallOutcome::err(e);
    }

    insert_cap(
        state,
        NewCap {
            dst_cspace,
            dst_slot,
            object: src_entry.object,
            rights: new_rights,
            badge: new_badge,
            parent: Some(src_cap_id),
            edge: EdgeKind::Derived,
        },
    );

    CallOutcome::completed(CallResult::ok())
}

/// Implements `cap_move`.
pub(crate) fn cap_move(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    src_cspace_slot: SlotIndex,
    src_slot: SlotIndex,
    dst_cspace_slot: SlotIndex,
    dst_slot: SlotIndex,
) -> CallOutcome {
    // CONTRACT-GAP: `kernel-interface.md` does not specify CSpace rights for
    // `cap_move`; we require `READ | WRITE` on `src_cspace` and `WRITE` on `dst_cspace`.
    let src_cspace = match lookup_cspace_cap(
        state,
        caller_cspace,
        src_cspace_slot,
        Rights::READ.union(Rights::WRITE),
    ) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let dst_cspace = match lookup_cspace_cap(state, caller_cspace, dst_cspace_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let (src_cap_id, _) = match lookup_cap_in_cspace(state, src_cspace, src_slot, Rights::NONE) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    if let Err(e) = check_dst_slot_empty(state, dst_cspace, dst_slot) {
        return CallOutcome::err(e);
    }
    move_cap_location(state, src_cap_id, dst_cspace, dst_slot);
    CallOutcome::completed(CallResult::ok())
}

/// Implements `cap_inspect`.
pub(crate) fn cap_inspect(
    state: &KernelState,
    caller_cspace: ObjectId,
    cspace_slot: SlotIndex,
    slot: SlotIndex,
) -> CallOutcome {
    let cspace = match lookup_cspace_cap(state, caller_cspace, cspace_slot, Rights::READ) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let (_, entry) = match lookup_cap_in_cspace(state, cspace, slot, Rights::NONE) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let Some(obj) = state.objects.get(&entry.object) else {
        return CallOutcome::err(Error::Gone);
    };
    CallOutcome::completed(CallResult::ok_with(CallValue::Inspect(CapInfo {
        kind: obj.kind(),
        rights: entry.rights,
        badge: entry.badge,
        is_root: entry.parent.is_none(),
        child_count: entry.children.len(),
    })))
}

/// Implements `cap_delete`.
pub(crate) fn cap_delete(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    cspace_slot: SlotIndex,
    slot: SlotIndex,
) -> CallOutcome {
    let cspace = match lookup_cspace_cap(state, caller_cspace, cspace_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let slot_state = {
        let Some(KernelObject::CSpace(cs)) = state.objects.get(&cspace) else {
            return CallOutcome::err(Error::Gone);
        };
        let Some(&s) = cs.slots.get(slot) else {
            return CallOutcome::err(Error::Invalid);
        };
        s
    };

    let cap_id = match slot_state {
        Slot::Empty => return CallOutcome::err(Error::Invalid),
        // CONTRACT-GAP: Deleting a `Revoked` tombstone clears it back to `Empty`
        // so capability slots are not permanently leaked after revocation.
        Slot::Revoked => {
            if let Some(KernelObject::CSpace(cs)) = state.objects.get_mut(&cspace) {
                cs.slots[slot] = Slot::Empty;
            }
            return CallOutcome::completed(CallResult::ok());
        }
        Slot::Live(id) => id,
    };

    let mut woken = Vec::new();
    let budget = state.config.revoke_step_budget.max(1);
    let (finished, _) = bounded_delete_cap(state, cap_id, false, budget, &mut woken, None);

    CallOutcome {
        status: CallStatus::Completed(CallResult::err(if finished {
            Error::Ok
        } else {
            Error::Again
        })),
        woken,
    }
}

use crate::types::CallStatus;

/// Implements `cap_revoke`.
pub(crate) fn cap_revoke(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    cspace_slot: SlotIndex,
    slot: SlotIndex,
) -> CallOutcome {
    let cspace = match lookup_cspace_cap(state, caller_cspace, cspace_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let (target_cap_id, _) = match lookup_cap_in_cspace(state, cspace, slot, Rights::NONE) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };

    let mut woken = Vec::new();
    let budget = state.config.revoke_step_budget.max(1);
    let mut steps_used = 0;

    while steps_used < budget {
        let Some(leaf_id) = find_revocation_leaf(state, target_cap_id) else {
            return CallOutcome {
                status: CallStatus::Completed(CallResult::ok()),
                woken,
            };
        };
        let (_, used) = bounded_delete_cap(
            state,
            leaf_id,
            true,
            budget - steps_used,
            &mut woken,
            Some(target_cap_id),
        );
        steps_used += used.max(1);

        if !state.caps.contains_key(&target_cap_id) {
            return CallOutcome {
                status: CallStatus::Completed(CallResult::err(Error::Gone)),
                woken,
            };
        }
    }

    let done = state
        .caps
        .get(&target_cap_id)
        .is_some_and(|e| e.children.is_empty());
    CallOutcome {
        status: CallStatus::Completed(CallResult::err(if done { Error::Ok } else { Error::Again })),
        woken,
    }
}

/// Finds a leaf descendant in the CDT subtree rooted at `root_cap_id`.
/// Returns `None` if `root_cap_id` has no children or no longer exists.
fn find_revocation_leaf(state: &KernelState, root_cap_id: CapId) -> Option<CapId> {
    let first_child = state.caps.get(&root_cap_id)?.children.first().copied()?;
    let mut cur = first_child;
    while let Some(next) = state
        .caps
        .get(&cur)
        .and_then(|e| e.children.first().copied())
    {
        cur = next;
    }
    Some(cur)
}

/// Deletes `root_cap_id` in at most `budget` steps (`budget >= 1`).
///
/// If `root_cap_id` is the last capability to a non-empty `CSpace`, its slots are
/// cleared incrementally so every intermediate `Error::Again` return preserves
/// all invariants. Mutual cycles of last-cap `CSpace`s are detected via `stack`
/// and torn down atomically once all non-cycle slots in the component are empty.
fn bounded_delete_cap(
    state: &mut KernelState,
    root_cap_id: CapId,
    mark_revoked: bool,
    budget: usize,
    woken: &mut Vec<Wakeup>,
    protected_cap: Option<CapId>,
) -> (bool, usize) {
    let mut steps_used = 0;
    let mut stack: Vec<(CapId, ObjectId)> = Vec::new();
    let mut deferred_cycle_caps: BTreeSet<CapId> = BTreeSet::new();

    let Some(root_obj) = state.caps.get(&root_cap_id).map(|e| e.object) else {
        return (true, 0);
    };
    stack.push((root_cap_id, root_obj));

    while steps_used < budget {
        let Some(&(top_cap_id, top_obj_id)) = stack.last() else {
            return (true, steps_used);
        };

        let is_last_cap_cspace = matches!(
            state.objects.get(&top_obj_id),
            Some(KernelObject::CSpace(_))
        ) && cap_count_for_object(state, top_obj_id) == 1;

        if !is_last_cap_cspace {
            stack.pop();
            unlink_and_destroy_if_last(state, top_cap_id, mark_revoked, woken);
            steps_used += 1;
            if stack.is_empty() {
                return (true, steps_used);
            }
            continue;
        }

        // Collect stack cap IDs and CSpace object IDs currently on the teardown stack.
        let stack_caps: BTreeSet<CapId> = stack.iter().map(|&(c, _)| c).collect();
        let stack_objs: BTreeSet<ObjectId> = stack.iter().map(|&(_, o)| o).collect();

        // Find the next live slot inside `top_obj_id` that is not on `stack` and
        // not deferred as a mutual CSpace cycle edge.
        let candidate = find_clearable_slot(
            state,
            top_obj_id,
            &stack_caps,
            &deferred_cycle_caps,
            protected_cap,
        );

        match candidate {
            Some((child_cap_id, child_obj_id)) => {
                let child_is_last_cspace = matches!(
                    state.objects.get(&child_obj_id),
                    Some(KernelObject::CSpace(_))
                ) && cap_count_for_object(state, child_obj_id) == 1;

                if child_is_last_cspace && !stack_objs.contains(&child_obj_id) {
                    stack.push((child_cap_id, child_obj_id));
                } else {
                    unlink_and_destroy_if_last(state, child_cap_id, mark_revoked, woken);
                    steps_used += 1;
                }
            }
            None => {
                // No non-stack / non-deferred live slots remain in `top_obj_id`.
                let contains_stack_cap = cspace_contains_any_cap(state, top_obj_id, &stack_caps);
                if contains_stack_cap && stack.len() > 1 {
                    // `top_obj_id` holds a cap belonging to an ancestor CSpace on `stack`
                    // (a mutual CSpace last-cap cycle). Defer tearing down this edge until
                    // the ancestor has cleared all its other slots.
                    deferred_cycle_caps.insert(top_cap_id);
                    stack.pop();
                } else {
                    // Tear down `top_obj_id` plus any deferred cycle CSpaces attached to it.
                    teardown_empty_and_cycle_cspaces(
                        state,
                        top_cap_id,
                        &deferred_cycle_caps,
                        mark_revoked,
                        woken,
                    );
                    stack.pop();
                    steps_used += 1;
                    if stack.is_empty() {
                        return (true, steps_used);
                    }
                }
            }
        }
    }

    (stack.is_empty(), steps_used)
}

fn find_clearable_slot(
    state: &KernelState,
    cspace_id: ObjectId,
    stack_caps: &BTreeSet<CapId>,
    deferred_cycle_caps: &BTreeSet<CapId>,
    protected_cap: Option<CapId>,
) -> Option<(CapId, ObjectId)> {
    let Some(KernelObject::CSpace(cs)) = state.objects.get(&cspace_id) else {
        return None;
    };
    let mut deferred_protected = None;
    for &slot in &cs.slots {
        if let Slot::Live(cap_id) = slot {
            if stack_caps.contains(&cap_id) || deferred_cycle_caps.contains(&cap_id) {
                continue;
            }
            if Some(cap_id) == protected_cap {
                if let Some(entry) = state.caps.get(&cap_id) {
                    deferred_protected = Some((cap_id, entry.object));
                }
                continue;
            }
            if let Some(entry) = state.caps.get(&cap_id) {
                return Some((cap_id, entry.object));
            }
        }
    }
    deferred_protected
}

fn cspace_contains_any_cap(
    state: &KernelState,
    cspace_id: ObjectId,
    caps: &BTreeSet<CapId>,
) -> bool {
    let Some(KernelObject::CSpace(cs)) = state.objects.get(&cspace_id) else {
        return false;
    };
    cs.slots.iter().any(|slot| match slot {
        Slot::Live(id) => caps.contains(id),
        _ => false,
    })
}

/// Tears down `top_cap_id` (whose CSpace has no non-cycle slots left) along with any
/// `deferred_cycle_caps` reachable inside its cycle component.
fn teardown_empty_and_cycle_cspaces(
    state: &mut KernelState,
    top_cap_id: CapId,
    deferred_cycle_caps: &BTreeSet<CapId>,
    mark_revoked: bool,
    woken: &mut Vec<Wakeup>,
) {
    // Gather all CSpaces in the deferred cycle component starting from `top_cap_id`.
    let mut to_remove_caps = vec![top_cap_id];
    let mut idx = 0;
    while idx < to_remove_caps.len() {
        let cap_id = to_remove_caps[idx];
        idx += 1;
        if let Some(obj_id) = state.caps.get(&cap_id).map(|e| e.object)
            && let Some(KernelObject::CSpace(cs)) = state.objects.get(&obj_id)
        {
            for &slot in &cs.slots {
                if let Slot::Live(child_cap) = slot
                    && deferred_cycle_caps.contains(&child_cap)
                    && !to_remove_caps.contains(&child_cap)
                {
                    to_remove_caps.push(child_cap);
                }
            }
        }
    }

    // Remove all capabilities in the cycle component first (clearing slots & reparenting CDT),
    // then destroy all their CSpace objects so no slot lookup touches a removed CSpace.
    let mut objs_to_destroy = Vec::new();
    for &cap_id in &to_remove_caps {
        if let Some(obj_id) = unlink_cap_only(state, cap_id, mark_revoked) {
            objs_to_destroy.push(obj_id);
        }
    }
    for obj_id in objs_to_destroy {
        if cap_count_for_object(state, obj_id) == 0 {
            destroy_object(state, obj_id, woken);
        }
    }
}

/// Removes `cap_id` from its slot and CDT, reparenting any CDT children, and returns
/// the target `ObjectId` if `cap_id` existed.
fn unlink_cap_only(state: &mut KernelState, cap_id: CapId, mark_revoked: bool) -> Option<ObjectId> {
    let entry = state.caps.remove(&cap_id)?;
    let (cspace_id, slot_idx) = entry.location;
    if let Some(KernelObject::CSpace(cs)) = state.objects.get_mut(&cspace_id)
        && slot_idx < cs.slots.len()
    {
        cs.slots[slot_idx] = if mark_revoked {
            Slot::Revoked
        } else {
            Slot::Empty
        };
    }

    // Detach from parent's children list.
    if let Some(parent_id) = entry.parent
        && let Some(parent_entry) = state.caps.get_mut(&parent_id)
    {
        parent_entry.children.retain(|&c| c != cap_id);
    }

    // Reparent children to `entry.parent` (or promote to Root).
    for &child_id in &entry.children {
        if let Some(child_entry) = state.caps.get_mut(&child_id) {
            child_entry.parent = entry.parent;
            if entry.parent.is_none() {
                child_entry.edge = EdgeKind::Root;
            } else if entry.edge == EdgeKind::Retyped {
                child_entry.edge = EdgeKind::Retyped;
            }
        }
        if let Some(parent_id) = entry.parent
            && let Some(parent_entry) = state.caps.get_mut(&parent_id)
        {
            parent_entry.children.push(child_id);
        }
    }

    Some(entry.object)
}

/// Unlinks `cap_id` from the CDT and destroys the target object if it was the last cap.
fn unlink_and_destroy_if_last(
    state: &mut KernelState,
    cap_id: CapId,
    mark_revoked: bool,
    woken: &mut Vec<Wakeup>,
) {
    let Some(obj_id) = unlink_cap_only(state, cap_id, mark_revoked) else {
        return;
    };
    if cap_count_for_object(state, obj_id) == 0 {
        destroy_object(state, obj_id, woken);
    }
}

/// Destroys a kernel object whose last capability has gone, cleaning up all
/// queues, thread bindings, and untyped backing counts.
pub(crate) fn destroy_object(state: &mut KernelState, obj_id: ObjectId, woken: &mut Vec<Wakeup>) {
    // Untyped stays alive as backing storage if retyped objects still reference it.
    if let Some(KernelObject::Untyped(u)) = state.objects.get(&obj_id)
        && u.backed_count > 0
    {
        return;
    }

    let Some(obj) = state.objects.remove(&obj_id) else {
        return;
    };

    let parent_untyped = obj.parent_untyped();

    match obj {
        KernelObject::Untyped(_) | KernelObject::Frame(_) => {}
        KernelObject::ASpace(_) => {
            for obj in state.objects.values_mut() {
                if let KernelObject::Thread(t) = obj
                    && t.aspace == Some(obj_id)
                {
                    t.aspace = None;
                }
            }
        }
        KernelObject::CSpace(_) => {
            let mut affected_threads = Vec::new();
            for (&id, obj) in &state.objects {
                if let KernelObject::Thread(t) = obj
                    && t.cspace == Some(obj_id)
                {
                    affected_threads.push(ThreadId(id));
                }
            }
            for tid in affected_threads {
                cancel_thread_ipc_wait(state, tid);
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&tid.0) {
                    t.cspace = None;
                    t.state = ThreadState::Suspended;
                }
            }
        }
        KernelObject::Endpoint(EndpointObject {
            send_queue,
            recv_queue,
            ..
        }) => {
            for tid in send_queue.into_iter().chain(recv_queue) {
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&tid.0) {
                    t.state = ThreadState::Runnable;
                    woken.push(Wakeup {
                        thread: tid,
                        result: CallResult::err(Error::Gone),
                    });
                }
            }
            for obj in state.objects.values_mut() {
                if let KernelObject::Thread(t) = obj
                    && t.fault_ep == Some(obj_id)
                {
                    t.fault_ep = None;
                }
            }
        }
        KernelObject::Notify(ntfn) => {
            for tid in ntfn.wait_queue {
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&tid.0) {
                    t.state = ThreadState::Runnable;
                    woken.push(Wakeup {
                        thread: tid,
                        result: CallResult::err(Error::Gone),
                    });
                }
            }
        }
        KernelObject::Reply(reply) => match reply.state {
            ReplyState::Unarmed | ReplyState::CallerGone => {}
            ReplyState::WaitingRecv(server_tid) => {
                let ep_opt = match state.objects.get(&server_tid.0) {
                    Some(KernelObject::Thread(t)) => match t.state {
                        ThreadState::BlockedOnReceive {
                            source: crate::state::RecvSource::Endpoint(ep_id),
                            ..
                        } => Some(ep_id),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(ep_id) = ep_opt
                    && let Some(KernelObject::Endpoint(ep)) = state.objects.get_mut(&ep_id)
                {
                    ep.recv_queue.retain(|&id| id != server_tid);
                }
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&server_tid.0) {
                    t.state = ThreadState::Runnable;
                    woken.push(Wakeup {
                        thread: server_tid,
                        result: CallResult::err(Error::Gone),
                    });
                }
            }
            ReplyState::Armed { caller, .. } => {
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&caller.0) {
                    t.state = ThreadState::Runnable;
                    woken.push(Wakeup {
                        thread: caller,
                        result: CallResult::err(Error::Gone),
                    });
                }
            }
        },
        KernelObject::Thread(t) => {
            cancel_thread_ipc_state(state, ThreadId(obj_id), &t.state);
        }
    }

    if let Some(parent_id) = parent_untyped {
        decrement_untyped_backing(state, parent_id, woken);
    }
}

/// Detaches `tid` from whichever IPC queue or reply object it is currently waiting on.
pub(crate) fn cancel_thread_ipc_wait(state: &mut KernelState, tid: ThreadId) {
    let Some(KernelObject::Thread(t)) = state.objects.get(&tid.0) else {
        return;
    };
    let old_state = t.state.clone();
    cancel_thread_ipc_state(state, tid, &old_state);
}

fn cancel_thread_ipc_state(state: &mut KernelState, tid: ThreadId, thread_state: &ThreadState) {
    match thread_state {
        ThreadState::Suspended | ThreadState::Runnable => {}
        ThreadState::BlockedOnSend { endpoint, .. } => {
            if let Some(KernelObject::Endpoint(ep)) = state.objects.get_mut(endpoint) {
                ep.send_queue.retain(|&id| id != tid);
            }
        }
        ThreadState::BlockedOnReceive {
            source, reply_obj, ..
        } => {
            match source {
                crate::state::RecvSource::Endpoint(ep_id) => {
                    if let Some(KernelObject::Endpoint(ep)) = state.objects.get_mut(ep_id) {
                        ep.recv_queue.retain(|&id| id != tid);
                    }
                }
                crate::state::RecvSource::Notify(ntfn_id) => {
                    if let Some(KernelObject::Notify(ntfn)) = state.objects.get_mut(ntfn_id) {
                        ntfn.wait_queue.retain(|&id| id != tid);
                    }
                }
            }
            if let Some(r_id) = reply_obj
                && let Some(KernelObject::Reply(r)) = state.objects.get_mut(r_id)
                && r.state == ReplyState::WaitingRecv(tid)
            {
                r.state = ReplyState::Unarmed;
            }
        }
        ThreadState::BlockedOnReply { reply_obj } => {
            if let Some(KernelObject::Reply(r)) = state.objects.get_mut(reply_obj)
                && matches!(r.state, ReplyState::Armed { caller, .. } if caller == tid)
            {
                r.state = ReplyState::CallerGone;
            }
        }
    }
}

/// Decrements `backed_count` on `untyped_id`, resetting its watermark if empty and
/// destroying it if it also has zero capabilities.
fn decrement_untyped_backing(
    state: &mut KernelState,
    mut untyped_id: ObjectId,
    woken: &mut Vec<Wakeup>,
) {
    loop {
        let (should_destroy, next_parent) = match state.objects.get_mut(&untyped_id) {
            Some(KernelObject::Untyped(u)) => {
                u.backed_count = u.backed_count.saturating_sub(1);
                if u.backed_count == 0 {
                    u.watermark = 0;
                    (true, u.parent_untyped)
                } else {
                    (false, None)
                }
            }
            _ => return,
        };
        if !should_destroy || cap_count_for_object(state, untyped_id) > 0 {
            return;
        }
        state.objects.remove(&untyped_id);
        match next_parent {
            Some(p) => untyped_id = p,
            None => return,
        }
        let _ = &mut *woken;
    }
}
