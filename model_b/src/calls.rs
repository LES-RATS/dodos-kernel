//! The entry point of the model: [`State::apply`], one call made by one
//! thread, and the result. Out-of-scope calls of the contract return
//! `unsupported`.

use crate::state::*;
use dodos_abi::{Call, Error, Header, MAX_CAPS, MAX_WORDS, ObjectKind, PageSize, Received, Rights};
use std::collections::VecDeque;

/// Arguments of a call. One flat struct: which field matters depends on the
/// call, as in registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// Generic target slot (in the caller's cspace).
    pub slot: u32,
    /// Generic destination slot.
    pub dst_slot: u32,
    /// Wanted rights (`cap_derive`, derive-transfer).
    pub rights: Rights,
    /// Badge (`cap_derive`, `signal`).
    pub badge: u64,
    /// Object kind (`untyped_retype`).
    pub kind: ObjectKind,
    /// Size argument: bytes for `untyped`, slot count for `cspace`, ignored
    /// otherwise (`untyped_retype`).
    pub size: u64,
    /// Page size (`untyped_retype` of a frame).
    pub page_size: PageSize,
    /// Auxiliary slot: cspace of a new thread, notify bound to a new
    /// endpoint (`untyped_retype`).
    pub aux_slot: Option<u32>,
    /// Destination cspace for `cap_move` / `cap_derive`: a slot in the
    /// caller's cspace holding a cspace capability; `None` is the caller's
    /// own cspace.
    pub dst_cspace_slot: Option<u32>,
    /// Message header (`send`, `call`).
    pub header: Header,
    /// Message words (`send`, `call`).
    pub words: Vec<u64>,
    /// Capability transfer list (`send`, `call`).
    pub transfers: Vec<Transfer>,
    /// Receiver's declared slot range for incoming capabilities (`recv`,
    /// `reply_recv`), or for the reply's capabilities (`call`).
    pub recv_base: u32,
    pub recv_count: u32,
}

/// One entry of the message's capability transfer list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub slot: u32,
    pub mode: TransferMode,
}

impl Args {
    pub fn new() -> Self {
        Args {
            slot: 0,
            dst_slot: 0,
            rights: Rights::NONE,
            badge: 0,
            kind: ObjectKind::Untyped,
            size: 0,
            page_size: PageSize::Size4K,
            aux_slot: None,
            dst_cspace_slot: None,
            header: Header::new(0, 0, 0, 0).expect("zero header is valid"),
            words: Vec::new(),
            transfers: Vec::new(),
            recv_base: 0,
            recv_count: 0,
        }
    }
}

impl Default for Args {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Error code of the call itself (`ok` on success, also when the call
    /// blocked).
    pub error: Error,
    /// Returned words.
    pub values: Vec<u64>,
    /// True when the call left the thread blocked (the call completes later;
    /// see `inbox` on the thread's next `apply`).
    pub blocked: bool,
    /// A message (or error, or notification) delivered to the thread while it
    /// was blocked, completing its earlier blocked call. Returned together
    /// with the result of the new call, like a message landing in the IPC
    /// buffer before the thread's next syscall.
    pub inbox: Option<Inbox>,
}

impl Outcome {
    fn empty() -> Self {
        Outcome {
            error: Error::Ok,
            values: Vec::new(),
            blocked: false,
            inbox: None,
        }
    }

    fn err(e: Error) -> Self {
        Outcome {
            error: e,
            ..Self::empty()
        }
    }
}

/// Set a flag bit in a message header, keeping it valid.
fn header_with_flag(h: Header, flag: u32) -> Header {
    match Header::new(h.label(), h.words(), h.caps(), h.flags() | flag) {
        Ok(x) => x,
        Err(_) => h,
    }
}

impl State {
    /// Apply one call made by `caller`. This is the only entry point of the
    /// model: the test picks a runnable thread (see [`State::runnable`]), builds
    /// the call, and reads back the [`Outcome`].
    ///
    /// A call on a thread that does not exist, or whose cspace was destroyed,
    /// returns `gone` (contract rule 2). A call from a thread that is itself
    /// blocked returns `busy`: the harness only picks runnable threads.
    /// CONTRACT-GAP: the order in which a call reports several errors at once
    /// (bad slot, wrong type, missing right, ...) is not in the contract.
    /// Chosen: resolve the slot first (`gone`/`invalid`), then the object
    /// type (`invalid`), then the rights (`denied`), then the arguments.
    pub fn apply(&mut self, caller: ThreadId, call: Call, args: &Args) -> Outcome {
        let Some(thread) = self.threads.get(&caller) else {
            return Outcome::err(Error::Gone);
        };
        if thread.cspace.is_none() {
            return Outcome::err(Error::Gone);
        }
        if !matches!(thread.state, ThreadState::Runnable) {
            return Outcome::err(Error::Busy);
        }
        let inbox = self.threads.get_mut(&caller).and_then(|t| t.inbox.take());
        let mut out = match call {
            Call::CapDerive => self.cap_derive(caller, args),
            Call::CapMove => self.cap_move(caller, args),
            Call::CapDelete => self.cap_delete(caller, args),
            Call::CapRevoke => self.cap_revoke(caller, args),
            Call::CapInspect => self.cap_inspect(caller, args),
            Call::UntypedRetype => self.untyped_retype(caller, args),
            Call::Send => self.send(caller, args),
            Call::Recv => self.recv(caller, args, false),
            Call::ReplyRecv => self.recv(caller, args, true),
            Call::Call => self.call(caller, args),
            Call::Signal => self.signal(caller, args),
            Call::Wait => self.wait(caller, args),
            Call::Poll => self.poll(caller, args),
            // Everything else of the contract is out of scope for the
            // one-core model (aspace, sched, timer, irq, io, task, core,
            // debug, thread register access, yield, ...).
            _ => Outcome::err(Error::Unsupported),
        };
        if out.inbox.is_none() {
            out.inbox = inbox;
        }
        out
    }

    // ------------------------------------------------------------------
    // Capabilities
    // ------------------------------------------------------------------

    /// `cap_derive`: copy a capability with fewer rights, optionally with a
    /// badge. The new node is a child of the source in the derivation tree.
    fn cap_derive(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let src = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // Rights::derive (abi) enforces: source needs `derive`, wanted adds nothing.
        let wanted = match src.rights.derive(args.rights) {
            Ok(r) => r,
            Err(e) => return Outcome::err(e),
        };
        let dst_cs = match self.dst_cspace(caller, args) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // CONTRACT-GAP: whether setting or clearing a badge needs a right
        // (`manage`) is not in the contract. Chosen: any derive may set any
        // badge, including 0 (clearing), as a mint in seL4 may.
        if !self.dst_slot_free(dst_cs, args.dst_slot) {
            return Outcome::err(Error::Invalid);
        }
        let id = self.put_cap(
            dst_cs,
            args.dst_slot,
            Some(src.id),
            src.obj,
            wanted,
            args.badge,
        );
        let mut out = Outcome::empty();
        out.values = vec![id];
        out
    }

    /// `cap_move`: move a capability to another (empty) slot, possibly of
    /// another cspace named by a capability. The derivation tree is
    /// untouched.
    fn cap_move(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let src = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        let dst_cs = match self.dst_cspace(caller, args) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // CONTRACT-GAP: moving a capability onto its own slot. Chosen:
        // a no-op returning `ok`.
        if dst_cs == src.loc.0 && args.dst_slot == src.loc.1 {
            return Outcome::empty();
        }
        if !self.dst_slot_free(dst_cs, args.dst_slot) {
            return Outcome::err(Error::Invalid);
        }
        // CONTRACT-GAP: moving a capability into a cspace named by a
        // capability needs no right on that cspace in this model (the caller
        // holds a capability to it). Chosen: no right required.
        self.move_cap_to(src.id, dst_cs, args.dst_slot);
        Outcome::empty()
    }

    /// `cap_delete`: remove one capability. Children are re-parented (see
    /// `remove_cap_node`); the object is destroyed when its last capability
    /// goes.
    // CONTRACT-GAP: neither `cap_delete` nor `cap_revoke` is given a required
    /// right in the contract. Chosen: the holder needs none (as in seL4).
    // CONTRACT-GAP: rule 4 says deletion of a large object also runs in
    /// bounded steps with `again`; chosen: only `cap_revoke` is stepped,
    /// `cap_delete` removes its one node in a single step.
    fn cap_delete(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        self.remove_cap_node(cap.id, false);
        Outcome::empty()
    }

    /// `cap_revoke`: mark the whole subtree `gone` at once, then delete its
    /// nodes in bounded steps of [`REVOKE_STEP`], children before parents.
    /// Returns `again` while nodes remain, `ok` when the root is deleted.
    fn cap_revoke(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve_raw(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        if !cap.revoking {
            for (_, id) in self.subtree(cap.id) {
                if let Some(c) = self.caps.get_mut(&id) {
                    c.revoking = true;
                }
            }
        }
        // Children first: sort by depth, deepest first, then by id.
        let mut nodes = self.subtree(cap.id);
        nodes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut removed_root = false;
        for (_, id) in nodes.into_iter().take(REVOKE_STEP) {
            if id == cap.id {
                removed_root = true;
            }
            self.remove_cap_node(id, true);
        }
        let mut out = Outcome::empty();
        out.error = if removed_root {
            Error::Ok
        } else {
            Error::Again
        };
        out
    }

    /// `cap_inspect`: read kind, rights, badge and object of a capability.
    fn cap_inspect(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // CONTRACT-GAP: the contract does not say which right `cap_inspect`
        // needs. Chosen: none; holding the capability is enough.
        let kind = self
            .objects
            .get(&cap.obj)
            .map(|o| o.kind())
            .unwrap_or(ObjectKind::Untyped);
        let mut out = Outcome::empty();
        out.values = vec![kind as u64, cap.rights.bits() as u64, cap.badge, cap.obj];
        out
    }

    /// Is the destination slot inside its cspace and empty?
    fn dst_slot_free(&self, cspace: ObjId, slot: u32) -> bool {
        match self.objects.get(&cspace) {
            Some(Object::CSpace { slots }) => {
                matches!(slots.get(slot as usize), Some(SlotState::Empty))
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // Memory: untyped_retype
    // ------------------------------------------------------------------

    /// `untyped_retype`: carve an object out of an untyped capability. The new
    /// capability is a child of the untyped capability in the derivation
    /// tree, so revoking the untyped revokes everything made from it (as in
    /// seL4).
    fn untyped_retype(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // CONTRACT-GAP: the contract does not say from which end of an
        // untyped range retype carves, or at which alignment. Chosen: from
        // the front, no alignment requirement.
        let (device, mem) = match self.objects.get(&cap.obj) {
            Some(Object::Untyped {
                base, size, device, ..
            }) => (*device, (*base, *size)),
            _ => return Outcome::err(Error::Invalid), // wrong object type
        };
        // CONTRACT-GAP: which right `untyped_retype` needs on the untyped
        // capability is not in the contract. Chosen: `write` ("change an
        // object's state").
        if !cap.rights.contains(Rights::WRITE) {
            return Outcome::err(Error::Denied);
        }
        // Object sizes (contract, "Object sizes"). Retyping to any other
        // kind of the contract is out of scope for the one-core model.
        let bytes = match args.kind {
            ObjectKind::Untyped => {
                if args.size == 0 {
                    return Outcome::err(Error::Invalid);
                }
                args.size
            }
            ObjectKind::CSpace => {
                if args.size == 0 || args.size > u32::MAX as u64 {
                    return Outcome::err(Error::Invalid);
                }
                args.size * SLOT_BYTES
            }
            ObjectKind::Frame => args.page_size.bytes(),
            ObjectKind::Thread => THREAD_BYTES,
            ObjectKind::Endpoint | ObjectKind::Notify => SMALL_BYTES,
            _ => return Outcome::err(Error::Unsupported),
        };
        // Device memory can only become a frame (that is mapped), never a
        // kernel object (contract, "Objects").
        if device && !matches!(args.kind, ObjectKind::Frame | ObjectKind::Untyped) {
            return Outcome::err(Error::Invalid);
        }
        let (base, size) = mem;
        if bytes > size {
            return Outcome::err(Error::NoMemory);
        }
        // Destination: always a slot of the caller's own cspace.
        // CONTRACT-GAP: the contract says a call names its target by a slot
        // in the caller's cspace; chosen: the new capability always lands in
        // the caller's cspace (other cspaces are filled by IPC transfer or by
        // `cap_move` with a destination cspace).
        if args.dst_cspace_slot.is_some() {
            return Outcome::err(Error::Invalid);
        }
        let cs = match self.caller_cspace(caller) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // Growing a cspace: destination holds a cspace capability.
        let grow_cspace = if args.kind == ObjectKind::CSpace {
            match self.resolve_raw(caller, args.dst_slot) {
                Ok(dcap) => match self.objects.get(&dcap.obj) {
                    Some(Object::CSpace { .. }) => {
                        // CONTRACT-GAP: which right growing a cspace needs is
                        // not in the contract. Chosen: `manage` on the cspace
                        // capability ("configure the object").
                        if !dcap.rights.contains(Rights::MANAGE) {
                            return Outcome::err(Error::Denied);
                        }
                        if dcap.revoking {
                            return Outcome::err(Error::Gone);
                        }
                        Some(dcap.obj)
                    }
                    _ => return Outcome::err(Error::Invalid),
                },
                Err(Error::Invalid) => None, // empty slot: create a new cspace
                Err(e) => return Outcome::err(e),
            }
        } else {
            None
        };
        if grow_cspace.is_none() && !self.dst_slot_free(cs, args.dst_slot) {
            return Outcome::err(Error::Invalid);
        }
        // Carve the memory from the front of the untyped range.
        if let Some(Object::Untyped {
            base: b, size: s, ..
        }) = self.objects.get_mut(&cap.obj)
        {
            *b += bytes;
            *s -= bytes;
        }
        let mut out = Outcome::empty();
        match args.kind {
            ObjectKind::Untyped => {
                let obj = self.fresh_obj();
                self.objects.insert(
                    obj,
                    Object::Untyped {
                        base,
                        size: bytes,
                        init_base: base,
                        init_size: bytes,
                        device,
                    },
                );
                self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                out.values = vec![obj];
            }
            ObjectKind::CSpace => match grow_cspace {
                Some(csobj) => {
                    if let Some(Object::CSpace { slots }) = self.objects.get_mut(&csobj) {
                        slots.resize(slots.len() + args.size as usize, SlotState::Empty);
                    }
                    out.values = vec![csobj];
                }
                None => {
                    let obj = self.fresh_obj();
                    self.objects.insert(
                        obj,
                        Object::CSpace {
                            slots: vec![SlotState::Empty; args.size as usize],
                        },
                    );
                    self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                    out.values = vec![obj];
                }
            },
            ObjectKind::Frame => {
                let obj = self.fresh_obj();
                self.objects.insert(
                    obj,
                    Object::Frame {
                        page_size: args.page_size,
                    },
                );
                self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                out.values = vec![obj];
            }
            ObjectKind::Thread => {
                let obj = self.fresh_obj();
                self.objects.insert(obj, Object::Thread);
                // The new thread's cspace: the auxiliary slot names a cspace
                // capability, or the caller's own cspace by default.
                let tcs = match args.aux_slot {
                    Some(s) => {
                        let acap = match self.resolve(caller, s) {
                            Ok(c) => c,
                            Err(e) => return Outcome::err(e),
                        };
                        match self.objects.get(&acap.obj) {
                            Some(Object::CSpace { .. }) => acap.obj,
                            _ => return Outcome::err(Error::Invalid),
                        }
                    }
                    None => cs,
                };
                let task = self.take_task();
                self.threads.insert(
                    obj,
                    Thread {
                        obj,
                        cspace: Some(tcs),
                        task,
                        marks: dodos_abi::Marks::NONE,
                        state: ThreadState::Runnable,
                        inbox: None,
                    },
                );
                self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                out.values = vec![obj];
            }
            ObjectKind::Endpoint => {
                let obj = self.fresh_obj();
                // Optional bound notify, given as an auxiliary slot.
                let bound = match args.aux_slot {
                    Some(s) => {
                        let acap = match self.resolve(caller, s) {
                            Ok(c) => c,
                            Err(e) => return Outcome::err(e),
                        };
                        match self.objects.get(&acap.obj) {
                            Some(Object::Notify(_)) => Some(acap.obj),
                            _ => return Outcome::err(Error::Invalid),
                        }
                    }
                    None => None,
                };
                self.objects.insert(
                    obj,
                    Object::Endpoint(Endpoint {
                        queue: VecDeque::new(),
                        receivers: VecDeque::new(),
                        bound_notify: bound,
                    }),
                );
                self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                out.values = vec![obj];
            }
            ObjectKind::Notify => {
                let obj = self.fresh_obj();
                self.objects.insert(
                    obj,
                    Object::Notify(Notify {
                        word: 0,
                        waiters: Vec::new(),
                    }),
                );
                self.put_cap(cs, args.dst_slot, Some(cap.id), obj, Rights::ALL, 0);
                out.values = vec![obj];
            }
            _ => return Outcome::err(Error::Unsupported),
        }
        out
    }

    // ------------------------------------------------------------------
    // IPC
    // ------------------------------------------------------------------

    /// Validate a message and resolve its transfer list. All-or-nothing on
    /// the sender side: a bad slot or an impossible derivation fails the
    /// whole send.
    fn build_message(
        &self,
        caller: ThreadId,
        args: &Args,
    ) -> Result<(Header, Vec<u64>, Vec<ResolvedTransfer>), Error> {
        let h = Header::from_raw(args.header.raw())?;
        if h.words() != args.words.len() || h.caps() != args.transfers.len() {
            return Err(Error::Invalid);
        }
        if args.words.len() > MAX_WORDS || args.transfers.len() > MAX_CAPS {
            return Err(Error::Invalid);
        }
        let cs = self.caller_cspace(caller)?;
        let mut resolved = Vec::new();
        for t in &args.transfers {
            let cap = self.resolve(caller, t.slot)?;
            let mode = match t.mode {
                TransferMode::Move => TransferMode::Move,
                TransferMode::Derive(r) => {
                    // Same rule as cap_derive: source needs `derive`, wanted
                    // adds nothing.
                    let _ = cap.rights.derive(r)?;
                    TransferMode::Derive(r)
                }
            };
            resolved.push(ResolvedTransfer {
                from: (cs, t.slot),
                cap: cap.id,
                mode,
            });
        }
        Ok((h, args.words.clone(), resolved))
    }

    /// `send`: never blocks. On an endpoint, deliver to a waiting receiver or
    /// queue the message (and signal the bound notify). On a reply object,
    /// answer the one blocked call and consume the reply.
    fn send(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        match self.objects.get(&cap.obj) {
            Some(Object::Endpoint(_)) => {}
            Some(Object::Reply(_)) => return self.send_reply(caller, cap, args),
            _ => return Outcome::err(Error::Invalid),
        }
        if !cap.rights.contains(Rights::SEND) {
            return Outcome::err(Error::Denied);
        }
        // CONTRACT-GAP: the contract says the sender "needs the grant right
        // on the endpoint" to list capabilities. Chosen: `grant` is required
        // exactly when the message carries at least one capability.
        if !args.transfers.is_empty() && !cap.rights.contains(Rights::GRANT) {
            return Outcome::err(Error::Denied);
        }
        let (header, words, transfers) = match self.build_message(caller, args) {
            Ok(m) => m,
            Err(e) => return Outcome::err(e),
        };
        let ep = cap.obj;
        let badge = cap.badge;
        let inc = Incoming {
            sender: caller,
            badge,
            header,
            words,
            transfers,
            reply: None,
        };
        self.route_to_endpoint(ep, inc)
    }

    /// Hand a message to the first waiting receiver, or queue it in the
    /// endpoint and signal its bound notify.
    fn route_to_endpoint(&mut self, ep: ObjId, inc: Incoming) -> Outcome {
        let badge = inc.badge;
        let recv = match self.objects.get_mut(&ep) {
            Some(Object::Endpoint(e)) => e.receivers.pop_front(),
            _ => None,
        };
        match recv {
            Some(recv) => {
                let (range, want_reply) = match &self.threads[&recv].state {
                    ThreadState::BlockedRecv {
                        range, reply_recv, ..
                    } => (*range, *reply_recv),
                    _ => ((0, 0), false),
                };
                self.deliver(recv, inc, range, want_reply);
            }
            None => {
                let bound = match self.objects.get_mut(&ep) {
                    Some(Object::Endpoint(e)) => {
                        e.queue.push_back(inc);
                        e.bound_notify
                    }
                    _ => None,
                };
                if let Some(n) = bound {
                    // CONTRACT-GAP: what bits a queued message sets in the
                    // bound notify is not in the contract. Chosen: the
                    // sender's badge, as a `signal` with that badge.
                    self.do_signal(n, badge);
                }
            }
        }
        Outcome::empty()
    }

    /// Deliver a message to a thread: transfer the capabilities all or
    /// nothing into its declared range, move the reply capability if asked,
    /// and complete the thread's blocked call.
    fn deliver(
        &mut self,
        recv: ThreadId,
        inc: Incoming,
        range: (u32, u32),
        want_reply: bool,
    ) -> bool {
        let dropped = self.transfer_caps(recv, &inc.transfers, range.0, range.1);
        let header = if dropped {
            header_with_flag(inc.header, Header::FLAG_CAPS_DROPPED)
        } else {
            inc.header
        };
        // Record the drop on the reply object, so the caller is told in the
        // reply (contract, "Capability transfer").
        if let Some(robj) = inc.reply
            && dropped
            && let Some(Object::Reply(r)) = self.objects.get_mut(&robj)
        {
            r.caps_dropped = true;
        }
        // Move the reply capability to the receiver if it asked for it and
        // has a free slot; otherwise the reply stays with the caller.
        let mut reply_slot = None;
        if want_reply && let Some(robj) = inc.reply {
            let rcs = self.thread_cspace(recv);
            if let Some(c) = rcs
                && let Some(free) = self.lowest_free_slot(c)
            {
                let cap_id = self
                    .caps
                    .iter()
                    .find(|(_, c2)| c2.obj == robj)
                    .map(|(id, _)| *id);
                if let Some(cid) = cap_id {
                    self.move_cap_to(cid, c, free);
                    reply_slot = Some(free);
                }
            }
        }
        let (marks, task) = match self.threads.get(&inc.sender) {
            Some(t) => (t.marks, t.task),
            None => (dodos_abi::Marks::NONE, 0),
        };
        let received = Received {
            header,
            badge: inc.badge,
            marks,
            task,
        };
        if let Some(t) = self.threads.get_mut(&recv) {
            t.inbox = Some(Inbox::Message {
                received,
                words: inc.words.clone(),
                reply_slot,
            });
            t.state = ThreadState::Runnable;
        }
        dropped
    }

    /// Transfer capabilities into the receiver's declared range, all or
    /// nothing. Returns true when everything was dropped (`caps_dropped`).
    fn transfer_caps(
        &mut self,
        recv: ThreadId,
        transfers: &[ResolvedTransfer],
        base: u32,
        count: u32,
    ) -> bool {
        if transfers.is_empty() {
            return false;
        }
        let Some(recv_cs) = self.thread_cspace(recv) else {
            return true;
        };
        let Some(len) = self.cspace_len(recv_cs) else {
            return true;
        };
        // Too few slots, or a range outside the cspace: drop everything.
        if count < transfers.len() as u32 || base as usize + count as usize > len {
            return true;
        }
        // Any occupied slot in the range: drop everything.
        let occupied = (base..base + count).any(|s| {
            matches!(
                self.objects.get(&recv_cs).and_then(|o| match o {
                    Object::CSpace { slots } => slots.get(s as usize),
                    _ => None,
                }),
                Some(SlotState::Full(_))
            )
        });
        if occupied {
            return true;
        }
        // All-or-nothing: every move must still be possible at delivery time.
        // CONTRACT-GAP: the contract does not say what happens when a listed
        // capability was moved away or revoked between send and receive.
        // Chosen: the whole transfer is dropped (`caps_dropped`).
        for tr in transfers {
            let ok = match self.caps.get(&tr.cap) {
                Some(c) => {
                    !c.revoking
                        && match self.objects.get(&tr.from.0) {
                            Some(Object::CSpace { slots }) => {
                                matches!(slots.get(tr.from.1 as usize), Some(SlotState::Full(id)) if *id == tr.cap)
                            }
                            _ => false,
                        }
                }
                None => false,
            };
            if !ok {
                return true;
            }
        }
        for (i, tr) in transfers.iter().enumerate() {
            let dst = base + i as u32;
            match tr.mode {
                TransferMode::Move => self.move_cap_to(tr.cap, recv_cs, dst),
                TransferMode::Derive(r) => {
                    let obj = self.caps[&tr.cap].obj;
                    // CONTRACT-GAP: the badge of a derived-and-transferred
                    // capability is not in the contract. Chosen: 0.
                    self.put_cap(recv_cs, dst, Some(tr.cap), obj, r, 0);
                }
            }
        }
        false
    }

    /// `send` on a reply object: answer the one blocked call. The reply is
    /// one-shot: the reply capability is consumed and its slot becomes dead.
    fn send_reply(&mut self, caller: ThreadId, cap: Cap, args: &Args) -> Outcome {
        if !cap.rights.contains(Rights::SEND) {
            return Outcome::err(Error::Denied);
        }
        // CONTRACT-GAP: the rights of a reply capability are not in the
        // contract. Chosen: `send` only, so a reply message carries no
        // capabilities (a transfer list is refused with `denied`).
        if !args.transfers.is_empty() {
            return Outcome::err(Error::Denied);
        }
        let (header, words, _) = match self.build_message(caller, args) {
            Ok(m) => m,
            Err(e) => return Outcome::err(e),
        };
        let robj = cap.obj;
        // The caller must still be blocked on this reply.
        let (blocked_caller, caps_dropped) = match self.objects.get(&robj) {
            Some(Object::Reply(r)) => match self.threads.get(&r.caller) {
                Some(t) => match &t.state {
                    ThreadState::BlockedReply { reply, .. } if *reply == robj => {
                        (r.caller, r.caps_dropped)
                    }
                    _ => return Outcome::err(Error::Busy),
                },
                None => return Outcome::err(Error::Busy),
            },
            _ => return Outcome::err(Error::Invalid),
        };
        // The caller is told about dropped capabilities through the flag in
        // the reply's header (contract, "Capability transfer").
        let header = if caps_dropped {
            header_with_flag(header, Header::FLAG_CAPS_DROPPED)
        } else {
            header
        };
        let (marks, task) = (self.threads[&caller].marks, self.threads[&caller].task);
        let received = Received {
            header,
            badge: cap.badge,
            marks,
            task,
        };
        if let Some(t) = self.threads.get_mut(&blocked_caller) {
            t.inbox = Some(Inbox::Message {
                received,
                words,
                reply_slot: None,
            });
            t.state = ThreadState::Runnable;
        }
        // One-shot: consume the reply capability and destroy the object; the
        // slot becomes a dead tombstone, later calls on it are `gone`.
        self.remove_cap_node(cap.id, true);
        Outcome::empty()
    }

    /// `call`: send a message and block until the reply. A reply object and
    /// its capability are created in the caller's cspace; the caller is told
    /// the slot as a returned value.
    fn call(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        if !matches!(self.objects.get(&cap.obj), Some(Object::Endpoint(_))) {
            return Outcome::err(Error::Invalid);
        }
        if !cap.rights.contains(Rights::SEND) {
            return Outcome::err(Error::Denied);
        }
        if !args.transfers.is_empty() && !cap.rights.contains(Rights::GRANT) {
            return Outcome::err(Error::Denied);
        }
        let (header, words, transfers) = match self.build_message(caller, args) {
            Ok(m) => m,
            Err(e) => return Outcome::err(e),
        };
        let cs = match self.caller_cspace(caller) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        let len = self.cspace_len(cs).unwrap_or(0);
        // The declared range for capabilities carried by the reply.
        if args.recv_base as usize + args.recv_count as usize > len {
            return Outcome::err(Error::Invalid);
        }
        // CONTRACT-GAP: where the reply capability lives is not in the
        // contract. Chosen (seL4 MCS idea): the lowest free slot of the
        // caller's cspace, reported as a returned value; `no_slot` when the
        // cspace is full.
        let Some(reply_slot) = self.lowest_free_slot(cs) else {
            return Outcome::err(Error::NoSlot);
        };
        let ep = cap.obj;
        let robj = self.fresh_obj();
        self.objects.insert(
            robj,
            Object::Reply(Reply {
                caller,
                caps_dropped: false,
            }),
        );
        // CONTRACT-GAP: the place of the reply capability in the derivation
        // tree. Chosen: a child of the endpoint capability used, so
        // revoking the endpoint kills pending replies.
        self.put_cap(cs, reply_slot, Some(cap.id), robj, Rights::SEND, 0);
        let inc = Incoming {
            sender: caller,
            badge: cap.badge,
            header,
            words,
            transfers,
            reply: Some(robj),
        };
        self.route_to_endpoint(ep, inc);
        if let Some(t) = self.threads.get_mut(&caller) {
            t.state = ThreadState::BlockedReply {
                endpoint: ep,
                reply: robj,
                range: (args.recv_base, args.recv_count),
            };
        }
        let mut out = Outcome::empty();
        out.blocked = true;
        out.values = vec![reply_slot as u64];
        out
    }

    /// `recv` / `reply_recv`: receive on an endpoint. Takes a queued message
    /// if there is one, otherwise blocks the thread as a receiver.
    fn recv(&mut self, caller: ThreadId, args: &Args, want_reply: bool) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        if !matches!(self.objects.get(&cap.obj), Some(Object::Endpoint(_))) {
            return Outcome::err(Error::Invalid);
        }
        if !cap.rights.contains(Rights::RECV) {
            return Outcome::err(Error::Denied);
        }
        let cs = match self.caller_cspace(caller) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        let len = self.cspace_len(cs).unwrap_or(0);
        if args.recv_base as usize + args.recv_count as usize > len {
            return Outcome::err(Error::Invalid);
        }
        let ep = cap.obj;
        let inc = match self.objects.get_mut(&ep) {
            Some(Object::Endpoint(e)) => e.queue.pop_front(),
            _ => None,
        };
        let mut out = Outcome::empty();
        match inc {
            Some(inc) => {
                // CONTRACT-GAP: the contract does not say what a plain
                // `recv` (not `reply_recv`) does with the reply right of a
                // received `call`. Chosen: it stays with the caller, which
                // remains blocked until the reply is used or destroyed.
                self.deliver(caller, inc, (args.recv_base, args.recv_count), want_reply);
                out.inbox = self.threads.get_mut(&caller).and_then(|t| t.inbox.take());
            }
            None => {
                if let Some(t) = self.threads.get_mut(&caller) {
                    t.state = ThreadState::BlockedRecv {
                        endpoint: ep,
                        range: (args.recv_base, args.recv_count),
                        reply_recv: want_reply,
                    };
                }
                if let Some(Object::Endpoint(e)) = self.objects.get_mut(&ep) {
                    e.receivers.push_back(caller);
                }
                out.blocked = true;
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Notifications
    // ------------------------------------------------------------------

    /// `signal`: set bits in a notify word and wake the waiters.
    fn signal(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        if !matches!(self.objects.get(&cap.obj), Some(Object::Notify(_))) {
            return Outcome::err(Error::Invalid);
        }
        if !cap.rights.contains(Rights::SEND) {
            return Outcome::err(Error::Denied);
        }
        let n = cap.obj;
        let bits = self.do_signal(n, args.badge);
        let mut out = Outcome::empty();
        out.values = vec![bits];
        out
    }

    /// OR `badge` into the notify word and wake all waiters with the whole
    /// word, which is then cleared. Returns the word as it was after the OR.
    // CONTRACT-GAP: wake-one or wake-all, and clear-on-read, are not in the
    // contract ("bits merge"). Chosen: wake all current waiters, each gets
    // the full word, the word is cleared; with no waiter it accumulates.
    fn do_signal(&mut self, n: ObjId, badge: u64) -> u64 {
        let mut bits = badge;
        let mut waiters = Vec::new();
        if let Some(Object::Notify(notify)) = self.objects.get_mut(&n) {
            notify.word |= badge;
            bits = notify.word;
            if !notify.waiters.is_empty() {
                waiters = std::mem::take(&mut notify.waiters);
                notify.word = 0; // the waiters consumed the word
            }
        }
        for tid in waiters {
            if let Some(t) = self.threads.get_mut(&tid) {
                t.state = ThreadState::Runnable;
                t.inbox = Some(Inbox::Notification(bits));
            }
        }
        bits
    }

    /// `wait`: on a notify, or on an endpoint with a bound notify. Returns
    /// immediately when bits are pending, otherwise blocks.
    fn wait(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        let notify = match self.objects.get(&cap.obj) {
            Some(Object::Notify(_)) => {
                if !cap.rights.contains(Rights::RECV) {
                    return Outcome::err(Error::Denied);
                }
                cap.obj
            }
            Some(Object::Endpoint(e)) => {
                if !cap.rights.contains(Rights::RECV) {
                    return Outcome::err(Error::Denied);
                }
                // CONTRACT-GAP: what exactly `wait` on an endpoint with a
                // bound notify waits for is ambiguous. Chosen: it waits on
                // the bound notify (an endpoint without one is `invalid`).
                match e.bound_notify {
                    Some(n) => n,
                    None => return Outcome::err(Error::Invalid),
                }
            }
            _ => return Outcome::err(Error::Invalid),
        };
        let mut out = Outcome::empty();
        let pending = match self.objects.get_mut(&notify) {
            Some(Object::Notify(n)) => {
                if n.word != 0 {
                    let w = n.word;
                    n.word = 0;
                    Some(w)
                } else {
                    None
                }
            }
            _ => None,
        };
        match pending {
            Some(w) => {
                out.values = vec![w];
            }
            None => {
                if let Some(t) = self.threads.get_mut(&caller) {
                    t.state = ThreadState::BlockedWait { notify };
                }
                if let Some(Object::Notify(n)) = self.objects.get_mut(&notify) {
                    n.waiters.push(caller);
                }
                out.blocked = true;
            }
        }
        out
    }

    /// `poll`: non-blocking read of a notify word.
    fn poll(&mut self, caller: ThreadId, args: &Args) -> Outcome {
        let cap = match self.resolve(caller, args.slot) {
            Ok(c) => c,
            Err(e) => return Outcome::err(e),
        };
        // CONTRACT-GAP: `poll` on an endpoint is not in the contract.
        // Chosen: notifications only, an endpoint is `invalid`.
        if !matches!(self.objects.get(&cap.obj), Some(Object::Notify(_))) {
            return Outcome::err(Error::Invalid);
        }
        if !cap.rights.contains(Rights::RECV) {
            return Outcome::err(Error::Denied);
        }
        let n = cap.obj;
        let mut out = Outcome::empty();
        match self.objects.get_mut(&n) {
            Some(Object::Notify(notify)) => {
                if notify.word != 0 {
                    out.values = vec![notify.word];
                    notify.word = 0;
                } else {
                    out.error = Error::WouldWait;
                }
            }
            _ => out.error = Error::Invalid,
        }
        out
    }
}
