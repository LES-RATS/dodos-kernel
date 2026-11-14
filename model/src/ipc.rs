use std::collections::BTreeSet;

use dodos_abi::{Error, Header, MAX_CAPS, Marks, Received, Rights};

use crate::caps::{NewCap, insert_cap, lookup_cap_in_cspace, move_cap_location};
use crate::state::{
    EdgeKind, KernelObject, KernelState, PendingMessage, RecvSource, ReplyState, Slot, ThreadState,
};
use crate::types::{
    CallOutcome, CallResult, CallStatus, CallValue, CapTransfer, Message, ObjectId, SlotIndex,
    SlotRange, ThreadId, Wakeup,
};

fn validate_message(
    state: &KernelState,
    sender_cspace: ObjectId,
    msg: &Message,
) -> Result<(), Error> {
    if msg.words.len() != msg.header.words() || msg.caps.len() != msg.header.caps() {
        return Err(Error::Invalid);
    }
    if msg.caps.len() > MAX_CAPS {
        return Err(Error::Invalid);
    }

    // CONTRACT-GAP: `kernel-interface.md` does not specify whether listing the
    // same slot twice with `Move` in one message is allowed; we forbid moving
    // a slot that appears more than once in `msg.caps` (`Error::Invalid`).
    let mut moved_slots = BTreeSet::new();
    let mut seen_slots = BTreeSet::new();
    for transfer in &msg.caps {
        match *transfer {
            CapTransfer::Move { slot } => {
                if !seen_slots.insert(slot) {
                    return Err(Error::Invalid);
                }
                moved_slots.insert(slot);
                lookup_cap_in_cspace(state, sender_cspace, slot, Rights::NONE)?;
            }
            CapTransfer::Derive { slot, rights } => {
                if moved_slots.contains(&slot) {
                    return Err(Error::Invalid);
                }
                seen_slots.insert(slot);
                let (_, entry) = lookup_cap_in_cspace(state, sender_cspace, slot, Rights::DERIVE)?;
                entry.rights.derive(rights)?;
            }
        }
    }
    Ok(())
}

fn validate_recv_slots(
    state: &KernelState,
    cspace_id: ObjectId,
    recv_slots: SlotRange,
) -> Result<(), Error> {
    if recv_slots.count == 0 {
        return Ok(());
    }
    let Some(end) = recv_slots.start.checked_add(recv_slots.count) else {
        return Err(Error::Invalid);
    };
    let Some(KernelObject::CSpace(cs)) = state.objects.get(&cspace_id) else {
        return Err(Error::Gone);
    };
    if end > cs.slots.len() {
        return Err(Error::Invalid);
    }
    for idx in recv_slots.start..end {
        if cs.slots[idx] != Slot::Empty {
            return Err(Error::NoSlot);
        }
    }
    Ok(())
}

fn strip_kernel_flags(header: Header) -> Header {
    // CONTRACT-GAP: `kernel-interface.md` does not specify whether a sender
    // may set `FLAG_CAPS_DROPPED`; we clear kernel-owned `FLAG_CAPS_DROPPED`
    // on entry and set it only when capabilities are dropped on transfer.
    let flags = header.flags() & !Header::FLAG_CAPS_DROPPED;
    Header::new(header.label(), header.words(), header.caps(), flags)
        .expect("stripped header preserves valid word and cap counts")
}

fn build_delivered_header(raw_header: Header, caps_dropped: bool) -> Header {
    let base = strip_kernel_flags(raw_header);
    if caps_dropped {
        Header::new(
            base.label(),
            base.words(),
            0,
            base.flags() | Header::FLAG_CAPS_DROPPED,
        )
        .expect("caps_dropped header is valid")
    } else {
        base
    }
}

/// Transfers `caps` from `sender_cspace` into `recv_cspace`'s `recv_slots` all-or-nothing.
///
/// Returns:
/// - `Ok(false)` if all capabilities transferred cleanly.
/// - `Ok(true)` if the receiver lacked enough empty slots (`caps_dropped`, nothing moved).
/// - `Err(())` if a sender capability became invalid/revoked while waiting in `BlockedOnSend`.
fn transfer_caps_at_rendezvous(
    state: &mut KernelState,
    sender_cspace: ObjectId,
    caps: &[CapTransfer],
    recv_cspace: ObjectId,
    recv_slots: SlotRange,
) -> Result<bool, ()> {
    if caps.is_empty() {
        return Ok(false);
    }

    // Re-validate all sender capabilities (relevant when sender was queued).
    let mut moved_slots = BTreeSet::new();
    let mut seen_slots = BTreeSet::new();
    for transfer in caps {
        match *transfer {
            CapTransfer::Move { slot } => {
                if !seen_slots.insert(slot) {
                    return Err(());
                }
                moved_slots.insert(slot);
                lookup_cap_in_cspace(state, sender_cspace, slot, Rights::NONE).map_err(|_| ())?;
            }
            CapTransfer::Derive { slot, rights } => {
                if moved_slots.contains(&slot) {
                    return Err(());
                }
                seen_slots.insert(slot);
                let (_, entry) = lookup_cap_in_cspace(state, sender_cspace, slot, Rights::DERIVE)
                    .map_err(|_| ())?;
                entry.rights.derive(rights).map_err(|_| ())?;
            }
        }
    }

    // Collect empty destination slots in `recv_slots`.
    let dst_slots: Vec<SlotIndex> = {
        let Some(KernelObject::CSpace(cs)) = state.objects.get(&recv_cspace) else {
            return Ok(true);
        };
        let Some(end) = recv_slots.start.checked_add(recv_slots.count) else {
            return Ok(true);
        };
        if end > cs.slots.len() {
            return Ok(true);
        }
        (recv_slots.start..end)
            .filter(|&idx| cs.slots[idx] == Slot::Empty)
            .take(caps.len())
            .collect()
    };

    if dst_slots.len() < caps.len() {
        return Ok(true);
    }

    for (transfer, dst_slot) in caps.iter().zip(dst_slots) {
        match *transfer {
            CapTransfer::Move { slot } => {
                let (cap_id, _) =
                    lookup_cap_in_cspace(state, sender_cspace, slot, Rights::NONE).unwrap();
                move_cap_location(state, cap_id, recv_cspace, dst_slot);
            }
            CapTransfer::Derive { slot, rights } => {
                let (src_cap_id, src_entry) =
                    lookup_cap_in_cspace(state, sender_cspace, slot, Rights::DERIVE).unwrap();
                insert_cap(
                    state,
                    NewCap {
                        dst_cspace: recv_cspace,
                        dst_slot,
                        object: src_entry.object,
                        rights,
                        badge: src_entry.badge,
                        parent: Some(src_cap_id),
                        edge: EdgeKind::Derived,
                    },
                );
            }
        }
    }

    Ok(false)
}

fn deliver_reply_to_caller(
    state: &mut KernelState,
    caller_tid: ThreadId,
    reply_msg: &Message,
    sender_cspace: ObjectId,
    request_caps_dropped: bool,
    caller_recv_slots: SlotRange,
) -> Wakeup {
    let caller_cspace = match state.objects.get(&caller_tid.0) {
        Some(KernelObject::Thread(t)) => t.cspace,
        _ => None,
    };

    let reply_caps_dropped = match caller_cspace {
        Some(dst_cs) => transfer_caps_at_rendezvous(
            state,
            sender_cspace,
            &reply_msg.caps,
            dst_cs,
            caller_recv_slots,
        )
        .unwrap_or(true),
        None => !reply_msg.caps.is_empty(),
    };

    let any_dropped = reply_caps_dropped || request_caps_dropped;
    let delivered_header = build_delivered_header(reply_msg.header, any_dropped);
    let caps_transferred = if reply_caps_dropped {
        0
    } else {
        reply_msg.caps.len()
    };

    if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&caller_tid.0) {
        t.state = ThreadState::Runnable;
    }

    Wakeup {
        thread: caller_tid,
        result: CallResult::ok_with(CallValue::Reply {
            header: delivered_header,
            words: reply_msg.words.clone(),
            caps_transferred,
        }),
    }
}

/// Implements `call`.
pub(crate) fn call(
    state: &mut KernelState,
    caller_tid: ThreadId,
    caller_cspace: ObjectId,
    ep_slot: SlotIndex,
    msg: Message,
    reply_recv_slots: SlotRange,
) -> CallOutcome {
    let needed = if msg.caps.is_empty() {
        Rights::SEND
    } else {
        Rights::SEND.union(Rights::GRANT)
    };
    let (_, ep_entry) = match lookup_cap_in_cspace(state, caller_cspace, ep_slot, needed) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let ep_id = ep_entry.object;
    if !matches!(state.objects.get(&ep_id), Some(KernelObject::Endpoint(_))) {
        return CallOutcome::err(Error::Invalid);
    }

    if let Err(e) = validate_message(state, caller_cspace, &msg) {
        return CallOutcome::err(e);
    }
    if let Err(e) = validate_recv_slots(state, caller_cspace, reply_recv_slots) {
        return CallOutcome::err(e);
    }

    let waiting_receiver = match state.objects.get_mut(&ep_id) {
        Some(KernelObject::Endpoint(ep)) => ep.recv_queue.pop_front(),
        _ => None,
    };

    if let Some(server_tid) = waiting_receiver {
        let (server_cspace, reply_opt, server_recv_slots) = match state.objects.get(&server_tid.0) {
            Some(KernelObject::Thread(t)) => match &t.state {
                ThreadState::BlockedOnReceive {
                    reply_obj,
                    recv_slots,
                    ..
                } => (t.cspace, *reply_obj, *recv_slots),
                _ => unreachable!("queued receiver must be BlockedOnReceive"),
            },
            _ => unreachable!("queued receiver must exist"),
        };

        let reply_id = reply_opt.expect("endpoint receiver must bind a reply object");
        let caps_dropped = match server_cspace {
            Some(dst_cs) => transfer_caps_at_rendezvous(
                state,
                caller_cspace,
                &msg.caps,
                dst_cs,
                server_recv_slots,
            )
            .expect("caller message was validated immediately prior"),
            None => !msg.caps.is_empty(),
        };

        // Arm the server's reply object and block caller on reply.
        if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
            r.state = ReplyState::Armed {
                caller: caller_tid,
                request_caps_dropped: caps_dropped,
                reply_recv_slots,
            };
        }
        if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&caller_tid.0) {
            t.state = ThreadState::BlockedOnReply {
                reply_obj: reply_id,
            };
        }

        // Wake the server with the received message.
        if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&server_tid.0) {
            t.state = ThreadState::Runnable;
        }
        let delivered_header = build_delivered_header(msg.header, caps_dropped);
        let caps_transferred = if caps_dropped { 0 } else { msg.caps.len() };
        // CONTRACT-GAP: Tasks and provenance marks are out of scope in the
        // one-core model; `Received.marks` is `Marks::NONE` and `task` is `0`.
        let received = CallValue::Received {
            info: Received {
                header: delivered_header,
                badge: ep_entry.badge,
                marks: Marks::NONE,
                task: 0,
            },
            words: msg.words,
            caps_transferred,
        };

        CallOutcome::blocked(vec![Wakeup {
            thread: server_tid,
            result: CallResult::ok_with(received),
        }])
    } else {
        // No receiver waiting: block caller on send queue.
        let pending = PendingMessage::from_message(&msg, ep_entry.badge, reply_recv_slots);
        if let Some(KernelObject::Endpoint(ep)) = state.objects.get_mut(&ep_id) {
            ep.send_queue.push_back(caller_tid);
        }
        if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&caller_tid.0) {
            t.state = ThreadState::BlockedOnSend {
                endpoint: ep_id,
                msg: pending,
            };
        }
        CallOutcome::blocked(Vec::new())
    }
}

/// Implements `recv`.
pub(crate) fn recv(
    state: &mut KernelState,
    caller_tid: ThreadId,
    caller_cspace: ObjectId,
    ep_slot: SlotIndex,
    reply_slot: SlotIndex,
    recv_slots: SlotRange,
) -> CallOutcome {
    let (_, ep_entry) = match lookup_cap_in_cspace(state, caller_cspace, ep_slot, Rights::RECV) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let ep_id = ep_entry.object;
    if !matches!(state.objects.get(&ep_id), Some(KernelObject::Endpoint(_))) {
        return CallOutcome::err(Error::Invalid);
    }
    if let Err(e) = validate_recv_slots(state, caller_cspace, recv_slots) {
        return CallOutcome::err(e);
    }

    // CONTRACT-GAP: `kernel-interface.md` lists `reply` as a 64-byte object
    // without detailing how `recv` binds it. Following seL4 MCS, `recv` takes
    // an unarmed `Reply` capability (`Busy` if already bound or armed).
    let (_, r_entry) = match lookup_cap_in_cspace(state, caller_cspace, reply_slot, Rights::SEND) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let reply_id = match state.objects.get(&r_entry.object) {
        Some(KernelObject::Reply(r)) if r.state == ReplyState::Unarmed => r_entry.object,
        Some(KernelObject::Reply(_)) => return CallOutcome::err(Error::Busy),
        Some(_) => return CallOutcome::err(Error::Invalid),
        None => return CallOutcome::err(Error::Gone),
    };

    recv_common(
        state,
        caller_tid,
        caller_cspace,
        ep_id,
        reply_id,
        recv_slots,
        Vec::new(),
    )
}

/// Implements `reply_recv`.
pub(crate) fn reply_recv(
    state: &mut KernelState,
    caller_tid: ThreadId,
    caller_cspace: ObjectId,
    reply_slot: SlotIndex,
    reply_msg: Message,
    ep_slot: SlotIndex,
    recv_slots: SlotRange,
) -> CallOutcome {
    let reply_needed = if reply_msg.caps.is_empty() {
        Rights::SEND
    } else {
        Rights::SEND.union(Rights::GRANT)
    };
    let (_, reply_entry) =
        match lookup_cap_in_cspace(state, caller_cspace, reply_slot, reply_needed) {
            Ok(v) => v,
            Err(e) => return CallOutcome::err(e),
        };
    let reply_id = reply_entry.object;
    let reply_state = match state.objects.get(&reply_id) {
        Some(KernelObject::Reply(r)) => r.state,
        Some(_) => return CallOutcome::err(Error::Invalid),
        None => return CallOutcome::err(Error::Gone),
    };

    if let Err(e) = validate_message(state, caller_cspace, &reply_msg) {
        return CallOutcome::err(e);
    }

    let (_, ep_entry) = match lookup_cap_in_cspace(state, caller_cspace, ep_slot, Rights::RECV) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let ep_id = ep_entry.object;
    if !matches!(state.objects.get(&ep_id), Some(KernelObject::Endpoint(_))) {
        return CallOutcome::err(Error::Invalid);
    }
    if let Err(e) = validate_recv_slots(state, caller_cspace, recv_slots) {
        return CallOutcome::err(e);
    }

    let mut woken = Vec::new();
    match reply_state {
        // CONTRACT-GAP: Like seL4 MCS `ReplyRecv`, if `Reply` is `Unarmed`
        // (e.g. previous message was a one-way `send`), `reply_recv` skips the
        // reply step and enters `recv`; if `CallerGone`, it disarms and returns `Gone`.
        ReplyState::Unarmed => {}
        ReplyState::WaitingRecv(_) => return CallOutcome::err(Error::Busy),
        ReplyState::CallerGone => {
            if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
                r.state = ReplyState::Unarmed;
            }
            return CallOutcome::err(Error::Gone);
        }
        ReplyState::Armed {
            caller,
            request_caps_dropped,
            reply_recv_slots,
        } => {
            if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
                r.state = ReplyState::Unarmed;
            }
            let w = deliver_reply_to_caller(
                state,
                caller,
                &reply_msg,
                caller_cspace,
                request_caps_dropped,
                reply_recv_slots,
            );
            woken.push(w);
        }
    }

    recv_common(
        state,
        caller_tid,
        caller_cspace,
        ep_id,
        reply_id,
        recv_slots,
        woken,
    )
}

fn recv_common(
    state: &mut KernelState,
    receiver_tid: ThreadId,
    receiver_cspace: ObjectId,
    ep_id: ObjectId,
    reply_id: ObjectId,
    recv_slots: SlotRange,
    mut woken: Vec<Wakeup>,
) -> CallOutcome {
    // Check queued senders on `ep_id`.
    loop {
        let next_sender = match state.objects.get_mut(&ep_id) {
            Some(KernelObject::Endpoint(ep)) => ep.send_queue.pop_front(),
            _ => None,
        };
        let Some(sender_tid) = next_sender else {
            break;
        };

        let (sender_cspace, pending) = match state.objects.get(&sender_tid.0) {
            Some(KernelObject::Thread(t)) => match &t.state {
                ThreadState::BlockedOnSend { msg, .. } => (t.cspace, msg.clone()),
                _ => unreachable!("queued sender must be BlockedOnSend"),
            },
            _ => unreachable!("queued sender must exist"),
        };

        let transfer_res = match sender_cspace {
            Some(src_cs) => transfer_caps_at_rendezvous(
                state,
                src_cs,
                &pending.caps,
                receiver_cspace,
                recv_slots,
            ),
            None => Err(()),
        };

        let caps_dropped = match transfer_res {
            Ok(dropped) => dropped,
            Err(()) => {
                // Sender's capability was revoked while blocked; wake sender with `Gone`.
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&sender_tid.0) {
                    t.state = ThreadState::Runnable;
                }
                woken.push(Wakeup {
                    thread: sender_tid,
                    result: CallResult::err(Error::Gone),
                });
                continue;
            }
        };

        // Arm `reply_id` and transition sender to `BlockedOnReply`.
        if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
            r.state = ReplyState::Armed {
                caller: sender_tid,
                request_caps_dropped: caps_dropped,
                reply_recv_slots: pending.reply_recv_slots,
            };
        }
        if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&sender_tid.0) {
            t.state = ThreadState::BlockedOnReply {
                reply_obj: reply_id,
            };
        }

        let delivered_header = build_delivered_header(pending.header, caps_dropped);
        let caps_transferred = if caps_dropped { 0 } else { pending.caps.len() };
        return CallOutcome {
            status: CallStatus::Completed(CallResult::ok_with(CallValue::Received {
                info: Received {
                    header: delivered_header,
                    badge: pending.badge,
                    marks: Marks::NONE,
                    task: 0,
                },
                words: pending.words,
                caps_transferred,
            })),
            woken,
        };
    }

    // No valid sender waiting: block receiver on `ep_id`.
    if let Some(KernelObject::Endpoint(ep)) = state.objects.get_mut(&ep_id) {
        ep.recv_queue.push_back(receiver_tid);
    }
    if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
        r.state = ReplyState::WaitingRecv(receiver_tid);
    }
    if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&receiver_tid.0) {
        t.state = ThreadState::BlockedOnReceive {
            source: RecvSource::Endpoint(ep_id),
            reply_obj: Some(reply_id),
            recv_slots,
        };
    }

    CallOutcome::blocked(woken)
}

/// Implements non-blocking `send` on an `Endpoint` or a `Reply` capability.
pub(crate) fn send(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    target_slot: SlotIndex,
    msg: Message,
) -> CallOutcome {
    let needed = if msg.caps.is_empty() {
        Rights::SEND
    } else {
        Rights::SEND.union(Rights::GRANT)
    };
    let (_, entry) = match lookup_cap_in_cspace(state, caller_cspace, target_slot, needed) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    if let Err(e) = validate_message(state, caller_cspace, &msg) {
        return CallOutcome::err(e);
    }

    match state.objects.get(&entry.object) {
        Some(KernelObject::Endpoint(_)) => {
            let ep_id = entry.object;
            let waiting_receiver = match state.objects.get_mut(&ep_id) {
                Some(KernelObject::Endpoint(ep)) => ep.recv_queue.pop_front(),
                _ => None,
            };
            // CONTRACT-GAP: `kernel-interface.md` states `send` never blocks
            // without naming the error when no receiver is waiting; we return `WouldWait`.
            let Some(server_tid) = waiting_receiver else {
                return CallOutcome::err(Error::WouldWait);
            };

            let (server_cspace, reply_opt, server_recv_slots) =
                match state.objects.get(&server_tid.0) {
                    Some(KernelObject::Thread(t)) => match &t.state {
                        ThreadState::BlockedOnReceive {
                            reply_obj,
                            recv_slots,
                            ..
                        } => (t.cspace, *reply_obj, *recv_slots),
                        _ => unreachable!("queued receiver must be BlockedOnReceive"),
                    },
                    _ => unreachable!("queued receiver must exist"),
                };

            // Disarm any `WaitingRecv` reply object since this is a one-way `send`.
            if let Some(r_id) = reply_opt
                && let Some(KernelObject::Reply(r)) = state.objects.get_mut(&r_id)
            {
                r.state = ReplyState::Unarmed;
            }

            let caps_dropped = match server_cspace {
                Some(dst_cs) => transfer_caps_at_rendezvous(
                    state,
                    caller_cspace,
                    &msg.caps,
                    dst_cs,
                    server_recv_slots,
                )
                .expect("sender message was validated immediately prior"),
                None => !msg.caps.is_empty(),
            };

            if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&server_tid.0) {
                t.state = ThreadState::Runnable;
            }
            let delivered_header = build_delivered_header(msg.header, caps_dropped);
            let caps_transferred = if caps_dropped { 0 } else { msg.caps.len() };
            CallOutcome {
                status: CallStatus::Completed(CallResult::ok()),
                woken: vec![Wakeup {
                    thread: server_tid,
                    result: CallResult::ok_with(CallValue::Received {
                        info: Received {
                            header: delivered_header,
                            badge: entry.badge,
                            marks: Marks::NONE,
                            task: 0,
                        },
                        words: msg.words,
                        caps_transferred,
                    }),
                }],
            }
        }
        // CONTRACT-GAP: `kernel-interface.md` defines `reply` as an object and
        // lists `reply_recv` and `send` without a standalone `reply` call; we
        // support one-way reply via `send` on a `Reply` capability.
        Some(KernelObject::Reply(r)) => {
            let reply_id = entry.object;
            match r.state {
                ReplyState::Unarmed => CallOutcome::err(Error::Invalid),
                ReplyState::WaitingRecv(_) => CallOutcome::err(Error::Busy),
                ReplyState::CallerGone => {
                    if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
                        r.state = ReplyState::Unarmed;
                    }
                    CallOutcome::err(Error::Gone)
                }
                ReplyState::Armed {
                    caller,
                    request_caps_dropped,
                    reply_recv_slots,
                } => {
                    if let Some(KernelObject::Reply(r)) = state.objects.get_mut(&reply_id) {
                        r.state = ReplyState::Unarmed;
                    }
                    let wakeup = deliver_reply_to_caller(
                        state,
                        caller,
                        &msg,
                        caller_cspace,
                        request_caps_dropped,
                        reply_recv_slots,
                    );
                    CallOutcome {
                        status: CallStatus::Completed(CallResult::ok()),
                        woken: vec![wakeup],
                    }
                }
            }
        }
        Some(_) => CallOutcome::err(Error::Invalid),
        None => CallOutcome::err(Error::Gone),
    }
}

/// Implements `signal` on a `Notify` capability.
pub(crate) fn signal(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    notify_slot: SlotIndex,
) -> CallOutcome {
    let (_, entry) = match lookup_cap_in_cspace(state, caller_cspace, notify_slot, Rights::SEND) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };
    let ntfn_id = entry.object;
    if !matches!(state.objects.get(&ntfn_id), Some(KernelObject::Notify(_))) {
        return CallOutcome::err(Error::Invalid);
    }

    // CONTRACT-GAP: `kernel-interface.md` does not specify whether `signal`
    // takes a payload word or ORs the capability's badge; following seL4 and
    // the model brief, `signal` ORs `entry.badge` into `notify.bits`, waking a
    // waiter when `bits != 0`.
    let woken_tid_and_bits = match state.objects.get_mut(&ntfn_id) {
        Some(KernelObject::Notify(ntfn)) => {
            ntfn.bits |= entry.badge;
            if ntfn.bits != 0 {
                if let Some(tid) = ntfn.wait_queue.pop_front() {
                    let delivered = ntfn.bits;
                    ntfn.bits = 0;
                    Some((tid, delivered))
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };

    let mut woken = Vec::new();
    if let Some((tid, bits)) = woken_tid_and_bits {
        if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&tid.0) {
            t.state = ThreadState::Runnable;
        }
        woken.push(Wakeup {
            thread: tid,
            result: CallResult::ok_with(CallValue::NotifyBits(bits)),
        });
    }

    CallOutcome {
        status: CallStatus::Completed(CallResult::ok()),
        woken,
    }
}

/// Implements `wait` on a `Notify` capability.
pub(crate) fn wait(
    state: &mut KernelState,
    caller_tid: ThreadId,
    caller_cspace: ObjectId,
    target_slot: SlotIndex,
) -> CallOutcome {
    let (_, entry) = match lookup_cap_in_cspace(state, caller_cspace, target_slot, Rights::RECV) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };

    match state.objects.get_mut(&entry.object) {
        Some(KernelObject::Notify(ntfn)) => {
            if ntfn.bits != 0 {
                let bits = ntfn.bits;
                ntfn.bits = 0;
                CallOutcome::completed(CallResult::ok_with(CallValue::NotifyBits(bits)))
            } else {
                ntfn.wait_queue.push_back(caller_tid);
                let ntfn_id = entry.object;
                if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&caller_tid.0) {
                    t.state = ThreadState::BlockedOnReceive {
                        source: RecvSource::Notify(ntfn_id),
                        reply_obj: None,
                        recv_slots: SlotRange::empty(),
                    };
                }
                CallOutcome::blocked(Vec::new())
            }
        }
        // CONTRACT-GAP: `kernel-interface.md` mentions waiting on an endpoint
        // with a bound `notify`, but provides no system call to bind a `notify`
        // to an `endpoint` or `thread`; waiting on an `Endpoint` returns `Invalid`.
        Some(_) => CallOutcome::err(Error::Invalid),
        None => CallOutcome::err(Error::Gone),
    }
}

/// Implements non-blocking `poll` on a `Notify` capability.
pub(crate) fn poll(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    target_slot: SlotIndex,
) -> CallOutcome {
    let (_, entry) = match lookup_cap_in_cspace(state, caller_cspace, target_slot, Rights::RECV) {
        Ok(v) => v,
        Err(e) => return CallOutcome::err(e),
    };

    match state.objects.get_mut(&entry.object) {
        Some(KernelObject::Notify(ntfn)) => {
            if ntfn.bits != 0 {
                let bits = ntfn.bits;
                ntfn.bits = 0;
                CallOutcome::completed(CallResult::ok_with(CallValue::NotifyBits(bits)))
            } else {
                CallOutcome::err(Error::WouldWait)
            }
        }
        Some(_) => CallOutcome::err(Error::Invalid),
        None => CallOutcome::err(Error::Gone),
    }
}
