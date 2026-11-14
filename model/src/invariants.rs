use std::collections::{BTreeMap, BTreeSet};

use dodos_abi::{ObjectKind, Rights};

use crate::state::{
    EdgeKind, KernelObject, KernelState, RecvSource, ReplyState, Slot, ThreadState,
};
use crate::types::{ObjectId, ThreadId};

/// Violation detected by [`KernelState::check_invariants`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvariantError(pub String);

impl std::fmt::Display for InvariantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvariantError {}

fn fail(msg: impl Into<String>) -> Result<(), InvariantError> {
    Err(InvariantError(msg.into()))
}

/// Checks every global invariant of `KernelState`.
///
/// Checked invariants:
/// 1. **Monotonic IDs**: `next_object_id` is strictly greater than every key in
///    `objects`, and `next_cap_id` is strictly greater than every key in `caps`.
/// 2. **Slot/CDT bijection**:
///    - Every `Slot::Live(cap_id)` at `(cspace_id, slot_idx)` exists in `caps`
///      with `entry.location == (cspace_id, slot_idx)`.
///    - Every `CapEntry` in `caps` points to a live `CSpace` at `entry.location.0`
///      whose slot at `entry.location.1` equals `Slot::Live(cap_id)`.
/// 3. **Derivation tree (CDT) well-formedness**:
///    - Every `parent` link points to a live `CapEntry` whose `children` list
///      contains the child `CapId` exactly once.
///    - Every `child` listed in `children` exists in `caps`, appears without
///      duplicates, and has `child.parent == Some(parent_id)`.
///    - The `parent` chain is acyclic (every node reaches a root in bounded steps).
///    - `parent.is_none()` iff `edge == EdgeKind::Root`.
/// 4. **Rights attenuation & badge monotonicity**:
///    - For every `EdgeKind::Derived` child: `child.object == parent.object`,
///      `parent.rights.contains(Rights::DERIVE)`, and `parent.rights.contains(child.rights)`.
///    - If a `Derived` parent carries a non-zero badge, `child.badge == parent.badge`.
///    - Any non-zero badge exists only on `Endpoint` or `Notify` capabilities.
///    - For every `EdgeKind::Retyped` child: `parent.object` is a live `Untyped`
///      object and `child.object` has `parent_untyped == Some(parent.object)`.
/// 5. **Object lifecycle & no dangling references**:
///    - Every `CapEntry.object` refers to a live object in `objects`.
///    - Every live object in `objects` has at least one capability in `caps`,
///      except an `Untyped` whose capabilities were deleted while `backed_count > 0`.
///    - Each `Untyped` has `watermark <= size`, `backed_count` matching the exact
///      number of live objects backed by it, and `watermark == 0` when `backed_count == 0`.
///    - Device `Untyped` only backs `Untyped` or `Frame` objects (`is_device` matches).
///    - Every `CSpace` has `>= 1` slot, every `Untyped` has `size >= 1`, and
///      every `Frame` has `count >= 1`.
/// 6. **Thread, queue, and reply consistency**:
///    - Every `ThreadId` in any `Endpoint.send_queue`, `Endpoint.recv_queue`,
///      `Notify.wait_queue`, or `ReplyState::Armed` / `ReplyState::WaitingRecv`
///      refers to a live `Thread`.
///    - No `ThreadId` appears twice in one queue or in two different queues/replies.
///    - Each thread's `ThreadState` matches the exact queue or reply object it sits in.
///    - Every `Runnable` thread has a live `cspace`.
///    - Every `thread.cspace`, `thread.aspace`, and `thread.fault_ep` refers to
///      a live `CSpace`, `ASpace`, or `Endpoint` respectively.
/// 7. **IPC & notification invariants**:
///    - No `Endpoint` simultaneously has non-empty `send_queue` and `recv_queue`.
///    - No `Notify` simultaneously has `bits != 0` and non-empty `wait_queue`.
///    - Every `PendingMessage` has `words.len() == header.words()` and `caps.len() == header.caps()`.
pub fn check_invariants(state: &KernelState) -> Result<(), InvariantError> {
    // 1. Monotonic IDs.
    if let Some(&max_obj) = state.objects.keys().next_back()
        && state.next_object_id <= max_obj.0
    {
        return fail(format!(
            "next_object_id {} <= max live object {}",
            state.next_object_id, max_obj.0
        ));
    }
    if let Some(&max_cap) = state.caps.keys().next_back()
        && state.next_cap_id <= max_cap.0
    {
        return fail(format!(
            "next_cap_id {} <= max live cap {}",
            state.next_cap_id, max_cap.0
        ));
    }

    // 2. Slot <-> CapEntry bijection.
    let mut seen_slot_caps = BTreeSet::new();
    for (&obj_id, obj) in &state.objects {
        if let KernelObject::CSpace(cs) = obj {
            if cs.slots.is_empty() {
                return fail(format!("CSpace {:?} has 0 slots", obj_id));
            }
            for (idx, &slot) in cs.slots.iter().enumerate() {
                if let Slot::Live(cap_id) = slot {
                    if !seen_slot_caps.insert(cap_id) {
                        return fail(format!("CapId {:?} appears in two slots", cap_id));
                    }
                    let Some(entry) = state.caps.get(&cap_id) else {
                        return fail(format!(
                            "slot ({:?}, {}) references missing CapId {:?}",
                            obj_id, idx, cap_id
                        ));
                    };
                    if entry.location != (obj_id, idx) {
                        return fail(format!(
                            "CapId {:?} location {:?} != slot ({:?}, {})",
                            cap_id, entry.location, obj_id, idx
                        ));
                    }
                }
            }
        }
    }
    for (&cap_id, entry) in &state.caps {
        let (cs_id, idx) = entry.location;
        match state.objects.get(&cs_id) {
            Some(KernelObject::CSpace(cs)) => match cs.slots.get(idx) {
                Some(&Slot::Live(slot_cap)) if slot_cap == cap_id => {}
                other => {
                    return fail(format!(
                        "CapId {:?} claims ({:?}, {}) but slot is {:?}",
                        cap_id, cs_id, idx, other
                    ));
                }
            },
            _ => {
                return fail(format!("CapId {:?} sits in non-CSpace {:?}", cap_id, cs_id));
            }
        }
    }

    // 3 & 4. CDT structure, acyclicity, rights attenuation, badges, retype edges.
    let mut cap_counts_by_obj: BTreeMap<ObjectId, usize> = BTreeMap::new();
    for (&cap_id, entry) in &state.caps {
        let Some(target_obj) = state.objects.get(&entry.object) else {
            return fail(format!(
                "CapId {:?} points to destroyed object {:?}",
                cap_id, entry.object
            ));
        };
        *cap_counts_by_obj.entry(entry.object).or_insert(0) += 1;

        if entry.badge != 0
            && !matches!(target_obj.kind(), ObjectKind::Endpoint | ObjectKind::Notify)
        {
            return fail(format!(
                "CapId {:?} has non-zero badge on {:?}",
                cap_id,
                target_obj.kind()
            ));
        }

        // Check unique children and reverse parent links.
        let mut child_set = BTreeSet::new();
        for &child_id in &entry.children {
            if !child_set.insert(child_id) {
                return fail(format!(
                    "CapId {:?} lists duplicate child {:?}",
                    cap_id, child_id
                ));
            }
            let Some(child_entry) = state.caps.get(&child_id) else {
                return fail(format!(
                    "CapId {:?} lists missing child {:?}",
                    cap_id, child_id
                ));
            };
            if child_entry.parent != Some(cap_id) {
                return fail(format!(
                    "child {:?} parent {:?} != {:?}",
                    child_id, child_entry.parent, cap_id
                ));
            }
        }

        match (entry.parent, entry.edge) {
            (None, EdgeKind::Root) => {}
            (None, _) => {
                return fail(format!(
                    "CapId {:?} has parent None but edge {:?}",
                    cap_id, entry.edge
                ));
            }
            (Some(_), EdgeKind::Root) => {
                return fail(format!("CapId {:?} has parent Some but edge Root", cap_id));
            }
            (Some(parent_id), EdgeKind::Derived) => {
                let Some(parent_entry) = state.caps.get(&parent_id) else {
                    return fail(format!(
                        "CapId {:?} references missing parent {:?}",
                        cap_id, parent_id
                    ));
                };
                if !parent_entry.children.contains(&cap_id) {
                    return fail(format!(
                        "parent {:?} children list missing {:?}",
                        parent_id, cap_id
                    ));
                }
                if parent_entry.object != entry.object {
                    return fail(format!(
                        "Derived CapId {:?} object {:?} != parent object {:?}",
                        cap_id, entry.object, parent_entry.object
                    ));
                }
                if !parent_entry.rights.contains(Rights::DERIVE) {
                    return fail(format!(
                        "Derived CapId {:?} parent {:?} lacks DERIVE right",
                        cap_id, parent_id
                    ));
                }
                if !parent_entry.rights.contains(entry.rights) {
                    return fail(format!(
                        "Derived CapId {:?} rights {:?} exceed parent {:?}",
                        cap_id, entry.rights, parent_entry.rights
                    ));
                }
                if parent_entry.badge != 0 && entry.badge != parent_entry.badge {
                    return fail(format!(
                        "Derived CapId {:?} changed badge {} != parent {}",
                        cap_id, entry.badge, parent_entry.badge
                    ));
                }
            }
            (Some(parent_id), EdgeKind::Retyped) => {
                let Some(parent_entry) = state.caps.get(&parent_id) else {
                    return fail(format!(
                        "CapId {:?} references missing parent {:?}",
                        cap_id, parent_id
                    ));
                };
                if !parent_entry.children.contains(&cap_id) {
                    return fail(format!(
                        "parent {:?} children list missing {:?}",
                        parent_id, cap_id
                    ));
                }
                if !matches!(
                    state.objects.get(&parent_entry.object),
                    Some(KernelObject::Untyped(_))
                ) {
                    return fail(format!(
                        "Retyped CapId {:?} parent object {:?} is not Untyped",
                        cap_id, parent_entry.object
                    ));
                }
                let mut cur_u = target_obj.parent_untyped();
                let mut found_ancestor = false;
                let mut u_steps = 0usize;
                while let Some(uid) = cur_u {
                    if uid == parent_entry.object {
                        found_ancestor = true;
                        break;
                    }
                    u_steps += 1;
                    if u_steps > state.objects.len() {
                        break;
                    }
                    cur_u = state.objects.get(&uid).and_then(|o| o.parent_untyped());
                }
                if !found_ancestor {
                    return fail(format!(
                        "Retyped CapId {:?} target {:?} is not backed by Untyped {:?}",
                        cap_id, entry.object, parent_entry.object
                    ));
                }
            }
        }

        // Acyclicity check.
        let mut steps = 0usize;
        let mut cur = entry.parent;
        while let Some(p) = cur {
            steps += 1;
            if steps > state.caps.len() {
                return fail(format!("CDT cycle detected involving {:?}", cap_id));
            }
            cur = state.caps.get(&p).and_then(|e| e.parent);
        }
    }

    // 5. Object lifecycle & Untyped backing counts.
    let mut actual_backed_counts: BTreeMap<ObjectId, usize> = BTreeMap::new();
    for (&obj_id, obj) in &state.objects {
        if let Some(parent_u) = obj.parent_untyped() {
            let Some(KernelObject::Untyped(u)) = state.objects.get(&parent_u) else {
                return fail(format!(
                    "object {:?} references missing parent_untyped {:?}",
                    obj_id, parent_u
                ));
            };
            if u.is_device {
                match obj {
                    KernelObject::Untyped(cu) if cu.is_device => {}
                    KernelObject::Frame(f) if f.is_device => {}
                    _ => {
                        return fail(format!(
                            "device Untyped {:?} backs non-frame/non-device object {:?}",
                            parent_u, obj_id
                        ));
                    }
                }
            }
            *actual_backed_counts.entry(parent_u).or_insert(0) += 1;
        }

        match obj {
            KernelObject::Untyped(u) => {
                if u.size == 0 {
                    return fail(format!("Untyped {:?} has size 0", obj_id));
                }
                if u.watermark > u.size {
                    return fail(format!(
                        "Untyped {:?} watermark {} > size {}",
                        obj_id, u.watermark, u.size
                    ));
                }
                if u.backed_count == 0 && u.watermark != 0 {
                    return fail(format!(
                        "Untyped {:?} has backed_count 0 but watermark {}",
                        obj_id, u.watermark
                    ));
                }
                let caps = cap_counts_by_obj.get(&obj_id).copied().unwrap_or(0);
                if caps == 0 && u.backed_count == 0 {
                    return fail(format!(
                        "Untyped {:?} has 0 capabilities and 0 backed objects",
                        obj_id
                    ));
                }
            }
            KernelObject::Frame(f) => {
                if f.count == 0 {
                    return fail(format!("Frame {:?} has count 0", obj_id));
                }
                if !cap_counts_by_obj.contains_key(&obj_id) {
                    return fail(format!("Frame {:?} has 0 capabilities", obj_id));
                }
            }
            _ => {
                if !cap_counts_by_obj.contains_key(&obj_id) {
                    return fail(format!("object {:?} has 0 capabilities", obj_id));
                }
            }
        }
    }
    for (&obj_id, obj) in &state.objects {
        if let KernelObject::Untyped(u) = obj {
            let actual = actual_backed_counts.get(&obj_id).copied().unwrap_or(0);
            if u.backed_count != actual {
                return fail(format!(
                    "Untyped {:?} backed_count {} != actual {}",
                    obj_id, u.backed_count, actual
                ));
            }
        }
    }

    // 6 & 7. Threads, queues, replies, endpoints, notifications.
    let mut queued_threads: BTreeMap<ThreadId, String> = BTreeMap::new();
    let mut record_waiter = |tid: ThreadId, place: String| -> Result<(), InvariantError> {
        if !matches!(state.objects.get(&tid.0), Some(KernelObject::Thread(_))) {
            return fail(format!("{} references non-thread {:?}", place, tid));
        }
        if let Some(prev) = queued_threads.insert(tid, place.clone()) {
            return fail(format!(
                "thread {:?} present in both {} and {}",
                tid, prev, place
            ));
        }
        Ok(())
    };

    for (&obj_id, obj) in &state.objects {
        match obj {
            KernelObject::Endpoint(ep) => {
                if !ep.send_queue.is_empty() && !ep.recv_queue.is_empty() {
                    return fail(format!(
                        "Endpoint {:?} has both non-empty send_queue and recv_queue",
                        obj_id
                    ));
                }
                for &tid in &ep.send_queue {
                    record_waiter(tid, format!("Endpoint({:?}).send_queue", obj_id))?;
                    match state.objects.get(&tid.0) {
                        Some(KernelObject::Thread(t))
                            if matches!(
                                t.state,
                                ThreadState::BlockedOnSend { endpoint, .. } if endpoint == obj_id
                            ) => {}
                        _ => {
                            return fail(format!(
                                "thread {:?} in Endpoint({:?}).send_queue has mismatched state",
                                tid, obj_id
                            ));
                        }
                    }
                }
                for &tid in &ep.recv_queue {
                    record_waiter(tid, format!("Endpoint({:?}).recv_queue", obj_id))?;
                    match state.objects.get(&tid.0) {
                        Some(KernelObject::Thread(t))
                            if matches!(
                                t.state,
                                ThreadState::BlockedOnReceive {
                                    source: RecvSource::Endpoint(e),
                                    ..
                                } if e == obj_id
                            ) => {}
                        _ => {
                            return fail(format!(
                                "thread {:?} in Endpoint({:?}).recv_queue has mismatched state",
                                tid, obj_id
                            ));
                        }
                    }
                }
            }
            KernelObject::Notify(ntfn) => {
                if ntfn.bits != 0 && !ntfn.wait_queue.is_empty() {
                    return fail(format!(
                        "Notify {:?} has bits {:#x} and non-empty wait_queue",
                        obj_id, ntfn.bits
                    ));
                }
                for &tid in &ntfn.wait_queue {
                    record_waiter(tid, format!("Notify({:?}).wait_queue", obj_id))?;
                    match state.objects.get(&tid.0) {
                        Some(KernelObject::Thread(t))
                            if matches!(
                                t.state,
                                ThreadState::BlockedOnReceive {
                                    source: RecvSource::Notify(n),
                                    ..
                                } if n == obj_id
                            ) => {}
                        _ => {
                            return fail(format!(
                                "thread {:?} in Notify({:?}).wait_queue has mismatched state",
                                tid, obj_id
                            ));
                        }
                    }
                }
            }
            KernelObject::Reply(reply) => match reply.state {
                ReplyState::Unarmed | ReplyState::CallerGone => {}
                ReplyState::WaitingRecv(tid) => match state.objects.get(&tid.0) {
                    Some(KernelObject::Thread(t))
                        if matches!(
                            t.state,
                            ThreadState::BlockedOnReceive {
                                reply_obj: Some(r),
                                ..
                            } if r == obj_id
                        ) => {}
                    _ => {
                        return fail(format!(
                            "Reply({:?}) WaitingRecv({:?}) mismatches thread state",
                            obj_id, tid
                        ));
                    }
                },
                ReplyState::Armed { caller, .. } => {
                    record_waiter(caller, format!("Reply({:?}).Armed", obj_id))?;
                    match state.objects.get(&caller.0) {
                        Some(KernelObject::Thread(t))
                            if matches!(
                                t.state,
                                ThreadState::BlockedOnReply { reply_obj } if reply_obj == obj_id
                            ) => {}
                        _ => {
                            return fail(format!(
                                "Reply({:?}) Armed({:?}) mismatches thread state",
                                obj_id, caller
                            ));
                        }
                    }
                }
            },
            KernelObject::Thread(t) => {
                let tid = ThreadId(obj_id);
                if let Some(cs_id) = t.cspace
                    && !matches!(state.objects.get(&cs_id), Some(KernelObject::CSpace(_)))
                {
                    return fail(format!(
                        "Thread {:?} references non-CSpace {:?}",
                        tid, cs_id
                    ));
                }
                if let Some(as_id) = t.aspace
                    && !matches!(state.objects.get(&as_id), Some(KernelObject::ASpace(_)))
                {
                    return fail(format!(
                        "Thread {:?} references non-ASpace {:?}",
                        tid, as_id
                    ));
                }
                if let Some(ep_id) = t.fault_ep
                    && !matches!(state.objects.get(&ep_id), Some(KernelObject::Endpoint(_)))
                {
                    return fail(format!(
                        "Thread {:?} references non-Endpoint fault_ep {:?}",
                        tid, ep_id
                    ));
                }

                match &t.state {
                    ThreadState::Suspended => {}
                    ThreadState::Runnable => {
                        if t.cspace.is_none() {
                            return fail(format!("Runnable thread {:?} has no CSpace", tid));
                        }
                    }
                    ThreadState::BlockedOnSend { endpoint, msg } => {
                        if msg.words.len() != msg.header.words()
                            || msg.caps.len() != msg.header.caps()
                        {
                            return fail(format!(
                                "BlockedOnSend thread {:?} has malformed PendingMessage",
                                tid
                            ));
                        }
                        match state.objects.get(endpoint) {
                            Some(KernelObject::Endpoint(ep)) if ep.send_queue.contains(&tid) => {}
                            _ => {
                                return fail(format!(
                                    "BlockedOnSend thread {:?} missing from Endpoint({:?}).send_queue",
                                    tid, endpoint
                                ));
                            }
                        }
                    }
                    ThreadState::BlockedOnReceive {
                        source, reply_obj, ..
                    } => match source {
                        RecvSource::Endpoint(ep_id) => {
                            match state.objects.get(ep_id) {
                                Some(KernelObject::Endpoint(ep))
                                    if ep.recv_queue.contains(&tid) => {}
                                _ => {
                                    return fail(format!(
                                        "BlockedOnReceive thread {:?} missing from Endpoint({:?}).recv_queue",
                                        tid, ep_id
                                    ));
                                }
                            }
                            let Some(r_id) = reply_obj else {
                                return fail(format!(
                                    "BlockedOnReceive on Endpoint({:?}) has no reply_obj",
                                    ep_id
                                ));
                            };
                            match state.objects.get(r_id) {
                                Some(KernelObject::Reply(r))
                                    if r.state == ReplyState::WaitingRecv(tid) => {}
                                _ => {
                                    return fail(format!(
                                        "BlockedOnReceive thread {:?} reply_obj {:?} not WaitingRecv",
                                        tid, r_id
                                    ));
                                }
                            }
                        }
                        RecvSource::Notify(ntfn_id) => {
                            if reply_obj.is_some() {
                                return fail(format!(
                                    "BlockedOnReceive on Notify({:?}) has reply_obj",
                                    ntfn_id
                                ));
                            }
                            match state.objects.get(ntfn_id) {
                                Some(KernelObject::Notify(n)) if n.wait_queue.contains(&tid) => {}
                                _ => {
                                    return fail(format!(
                                        "BlockedOnReceive thread {:?} missing from Notify({:?}).wait_queue",
                                        tid, ntfn_id
                                    ));
                                }
                            }
                        }
                    },
                    ThreadState::BlockedOnReply { reply_obj } => {
                        match state.objects.get(reply_obj) {
                            Some(KernelObject::Reply(r)) if matches!(r.state, ReplyState::Armed { caller, .. } if caller == tid) =>
                                {}
                            _ => {
                                return fail(format!(
                                    "BlockedOnReply thread {:?} not armed on Reply({:?})",
                                    tid, reply_obj
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
