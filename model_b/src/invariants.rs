//! The invariant checker over the whole state. Run by the property test
//! after every step; any violation is reported as a string.
//!
//! Invariants checked:
//!
//! 1. Slot/capability bijection: every `Full` slot names a capability that
//!    exists, every capability sits in exactly one slot, and its `loc`
//!    field names that slot. `Dead` and `Empty` slots hold no capability.
//! 2. No capability points at a destroyed object.
//! 3. Derivation tree well formed: parent/child links are symmetric, the
//!    tree is acyclic, and roots have no parent.
//! 4. Rights attenuation: a derived capability's rights are a subset of its
//!    parent's rights.
//! 5. No thread is in two queues, and every blocked thread is in exactly the
//!    queue its state names (endpoint receivers, endpoint message queue,
//!    notify waiters); a runnable thread is in none.
//! 6. A thread blocked in `call` appears in its endpoint's queue with a
//!    reply object that exists, is a reply, and names that thread as caller.
//! 7. Every thread object exists, and every thread's bound cspace (if any)
//!    exists and is a cspace.
//! 8. An endpoint's bound notify, if any, exists and is a notify; a notify's
//!    waiters are exactly the threads blocked on it.
//! 9. Queued messages are well formed: `header.words() == words.len()` and
//!    `header.caps() == transfers.len()`.
//! 10. Untyped memory accounting: `size == init_size - (base - init_base)`,
//!     and the range never goes below zero.
//! 11. No thread is blocked on `send` (the contract says `send` never
//!     blocks); the state exists but must stay unused.

use crate::state::*;
use std::collections::{BTreeMap, BTreeSet};

/// Check every invariant; returns the list of violations (empty when the
/// state is well formed).
pub fn check_invariants(st: &State) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();

    // Where each capability sits.
    let mut cap_slot: BTreeMap<CapId, (ObjId, u32)> = BTreeMap::new();
    for (oid, o) in &st.objects {
        if let Object::CSpace { slots } = o {
            for (i, s) in slots.iter().enumerate() {
                if let SlotState::Full(id) = s {
                    if !st.caps.contains_key(id) {
                        v.push(format!("I1: slot {oid}/{} names missing cap {id}", i));
                    } else if cap_slot.insert(*id, (*oid, i as u32)).is_some() {
                        v.push(format!("I1: cap {id} sits in two slots"));
                    }
                }
            }
        }
    }
    for (id, cap) in &st.caps {
        match cap_slot.get(id) {
            None => v.push(format!("I1: cap {id} is in no slot")),
            Some(loc) => {
                if *loc != cap.loc {
                    v.push(format!("I1: cap {id} loc {:?} != slot {:?}", cap.loc, loc));
                }
            }
        }
        if !st.objects.contains_key(&cap.obj) {
            v.push(format!("I2: cap {id} points at missing object {}", cap.obj));
        }
    }

    // Derivation tree: symmetry, acyclicity, rights attenuation.
    for (id, cap) in &st.caps {
        if let Some(p) = cap.parent {
            match st.caps.get(&p) {
                None => v.push(format!("I3: cap {id} has missing parent {p}")),
                Some(pc) => {
                    if !pc.children.contains(id) {
                        v.push(format!("I3: parent {p} of {id} does not list it as child"));
                    }
                    if !pc.rights.contains(cap.rights) {
                        v.push(format!(
                            "I4: cap {id} rights {:?} not a subset of parent {p} {:?}",
                            cap.rights, pc.rights
                        ));
                    }
                }
            }
        }
        for ch in &cap.children {
            match st.caps.get(ch) {
                None => v.push(format!("I3: cap {id} has missing child {ch}")),
                Some(c) => {
                    if c.parent != Some(*id) {
                        v.push(format!("I3: child {ch} of {id} has parent {:?}", c.parent));
                    }
                }
            }
        }
        // Acyclicity: walk up the parent chain.
        let mut seen = BTreeSet::new();
        let mut cur = Some(*id);
        while let Some(c) = cur {
            if !seen.insert(c) {
                v.push(format!("I3: cycle in derivation tree at cap {c}"));
                break;
            }
            cur = st.caps.get(&c).and_then(|x| x.parent);
        }
    }

    // Threads and queues.
    let mut queue_count: BTreeMap<ThreadId, usize> = BTreeMap::new();
    for (oid, o) in &st.objects {
        match o {
            Object::Endpoint(e) => {
                if let Some(n) = e.bound_notify
                    && !matches!(st.objects.get(&n), Some(Object::Notify(_)))
                {
                    v.push(format!("I8: endpoint {oid} bound to non-notify {n}"));
                }
                for t in &e.receivers {
                    *queue_count.entry(*t).or_insert(0) += 1;
                }
                for inc in &e.queue {
                    // A queued `send` leaves its sender runnable; only a
                    // queued `call` has its sender blocked (on reply).
                    if matches!(
                        st.threads.get(&inc.sender).map(|t| &t.state),
                        Some(ThreadState::BlockedReply { .. })
                    ) {
                        *queue_count.entry(inc.sender).or_insert(0) += 1;
                    }
                    if inc.header.words() != inc.words.len()
                        || inc.header.caps() != inc.transfers.len()
                    {
                        v.push(format!(
                            "I9: queued message on {oid} has inconsistent header"
                        ));
                    }
                    if let Some(r) = inc.reply {
                        match st.objects.get(&r) {
                            Some(Object::Reply(rr)) => {
                                if rr.caller != inc.sender {
                                    v.push(format!(
                                        "I6: queued call on {oid} reply {r} names caller {} not sender {}",
                                        rr.caller, inc.sender
                                    ));
                                }
                            }
                            _ => v.push(format!(
                                "I6: queued message on {oid} names missing reply {r}"
                            )),
                        }
                    }
                }
            }
            Object::Notify(n) => {
                for t in &n.waiters {
                    *queue_count.entry(*t).or_insert(0) += 1;
                }
            }
            Object::Untyped {
                base,
                size,
                init_base,
                init_size,
                ..
            } if (*size > *init_size
                || *base < *init_base
                || *size != init_size - (base - init_base)) =>
            {
                v.push(format!(
                        "I10: untyped {oid} accounting broken (base {base}, size {size}, init {init_base}/{init_size})"
                    ));
            }
            _ => {}
        }
    }

    for (tid, t) in &st.threads {
        if !matches!(st.objects.get(&t.obj), Some(Object::Thread)) {
            v.push(format!("I7: thread {tid} has no thread object"));
        }
        if let Some(cs) = t.cspace
            && !matches!(st.objects.get(&cs), Some(Object::CSpace { .. }))
        {
            v.push(format!("I7: thread {tid} bound to missing cspace {cs}"));
        }
        match &t.state {
            ThreadState::Runnable => {}
            ThreadState::BlockedSend { .. } => v.push(format!(
                "I11: thread {tid} blocked on send (contract: send never blocks)"
            )),
            ThreadState::BlockedRecv { endpoint, .. } => {
                match st.objects.get(endpoint) {
                    Some(Object::Endpoint(e)) => {
                        if !e.receivers.contains(tid) {
                            v.push(format!("I5: thread {tid} blocked-recv on {endpoint} but not in its receivers"));
                        }
                    }
                    _ => v.push(format!(
                        "I5: thread {tid} blocked-recv on missing endpoint {endpoint}"
                    )),
                }
            }
            ThreadState::BlockedWait { notify } => match st.objects.get(notify) {
                Some(Object::Notify(n)) => {
                    if !n.waiters.contains(tid) {
                        v.push(format!(
                            "I8: thread {tid} blocked-wait on {notify} but not in its waiters"
                        ));
                    }
                }
                _ => v.push(format!(
                    "I8: thread {tid} blocked-wait on missing notify {notify}"
                )),
            },
            ThreadState::BlockedReply {
                endpoint, reply, ..
            } => {
                // The call's message was either queued on the endpoint or
                // already delivered to a receiver; in both cases the reply
                // object must exist and name this thread.
                match st.objects.get(endpoint) {
                    Some(Object::Endpoint(e)) => {
                        for i in e.queue.iter().filter(|i| i.sender == *tid) {
                            if i.reply != Some(*reply) {
                                v.push(format!(
                                    "I6: thread {tid} queued on {endpoint} with a different reply"
                                ));
                            }
                        }
                    }
                    _ => v.push(format!(
                        "I6: thread {tid} blocked-reply on missing endpoint {endpoint}"
                    )),
                }
                match st.objects.get(reply) {
                    Some(Object::Reply(r)) => {
                        if r.caller != *tid {
                            v.push(format!(
                                "I6: reply {reply} caller {} != thread {tid}",
                                r.caller
                            ));
                        }
                    }
                    _ => v.push(format!("I6: thread {tid} blocked on missing reply {reply}")),
                }
            }
        }
    }

    // No thread in two queues.
    for (tid, n) in &queue_count {
        if *n > 1 {
            v.push(format!("I5: thread {tid} is in {n} queues"));
        }
        match st.threads.get(tid) {
            Some(t) => {
                if matches!(t.state, ThreadState::Runnable) {
                    v.push(format!("I5: runnable thread {tid} is still in a queue"));
                }
            }
            None => v.push(format!("I5: queue names dead thread {tid}")),
        }
    }
    // Every queued/blocked thread must exist: covered above. Every `gone`
    // completion is fine.

    // A thread blocked on receive or on a notify must be in exactly that
    // queue (a thread blocked on reply may be in none: its message was
    // already delivered to a receiver).
    for (tid, t) in &st.threads {
        let needs_queue = matches!(
            t.state,
            ThreadState::BlockedRecv { .. } | ThreadState::BlockedWait { .. }
        );
        if needs_queue && !queue_count.contains_key(tid) {
            v.push(format!("I5: blocked thread {tid} is in no queue"));
        }
    }

    v
}
