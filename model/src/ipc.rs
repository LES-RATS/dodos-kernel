//! IPC, transfer of capabilities and notifications.

use crate::caps::Placing;
use crate::remote::Request;
use crate::{
    CapId, Delivery, Message, Model, ObjId, Outcome, Output, Parcel, Place, Range, ReplyState,
    Slot, Transfer, Wait, Window, live,
};
use dodos_abi::{
    Error, Header, Marks, ObjectKind, REG_WORDS, Received, Rights, SlotAddr, TREE_DEPTH,
};

/// What `wait` and `poll` do when the word is zero.
#[derive(Clone, Copy)]
pub(crate) enum IfZero {
    Wait,
    WouldWait,
}

/// A capability of the list of a message, once its slot is checked.
#[derive(Clone, Copy)]
enum Listed {
    Move(CapId),
    /// With the rights of the capability to derive.
    Derive(CapId, Rights),
}

impl Listed {
    fn cap(self) -> CapId {
        match self {
            Listed::Move(cap) | Listed::Derive(cap, _) => cap,
        }
    }
}

impl Message {
    /// Order of checks, step 2: the header is read right after the
    /// capability the call acts on. Beyond the limits (Messages: 64 words, 4
    /// capabilities), with a flag bit other than `add_marks` set by the
    /// sender, with an added-marks word that has a reserved bit, or that
    /// needs a message page the thread does not have (D-110), it is
    /// `invalid` before any other capability argument is looked at.
    pub(crate) fn check_header(&self, page: bool) -> Result<(), Error> {
        self.check(page)
    }

    fn check(&self, page: bool) -> Result<(), Error> {
        let header = Header::new(self.label, self.words.len(), self.caps.len(), self.flags)?;
        let add_marks = header.flags() == Header::FLAG_ADD_MARKS;
        if header.flags() & !Header::FLAG_ADD_MARKS != 0 {
            return Err(Error::Invalid);
        }
        if add_marks {
            Marks::from_bits(self.marks)?;
        }
        // Bindings: a header needs a message page for more words than the
        // registers hold, a capability, or the flag `add_marks`.
        if !page && (self.words.len() > REG_WORDS || !self.caps.is_empty() || add_marks) {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// IPC: `send`, and `grant` if the message lists capabilities, on an
    /// endpoint as on a reply object. Order of checks, step 2: judged first,
    /// with the number of capabilities that the header states.
    fn rights(&self) -> Rights {
        if self.caps.is_empty() {
            Rights::SEND
        } else {
            Rights::SEND.union(Rights::GRANT)
        }
    }
}

/// Notifications: what `recv` returns for a pending word of the bound
/// notify. The badge field holds the word.
fn notification(word: u64) -> Delivery {
    // Notifications: the flag `notification`, label 0, no words and no
    // capabilities; the badge field holds the word, the marks are 0 and the
    // task identifier is 0.
    let header = Header::new(0, 0, 0, Header::FLAG_NOTIFICATION).expect("an empty message");
    Delivery {
        received: received(header, word),
        words: Vec::new(),
    }
}

/// What the kernel sets on a received message: the stamp comes from the
/// delivery; a notification carries marks 0 and task 0.
fn received(header: Header, badge: u64) -> Received {
    Received {
        header,
        badge,
        marks: Marks::NONE,
        task: 0,
    }
}

impl Parcel {
    fn new(sender: ObjId, message: Message, badge: u64) -> Self {
        let added = message.flags & Header::FLAG_ADD_MARKS != 0;
        Self {
            label: message.label,
            words: message.words,
            badge,
            sender,
            caps: message.caps,
            marks: if added { message.marks } else { 0 },
        }
    }
}

impl Model {
    pub(crate) fn call(
        &mut self,
        thread: ObjId,
        endpoint: SlotAddr,
        message: Message,
        range: Range,
    ) -> Outcome {
        let held = self.held(thread, endpoint, ObjectKind::Endpoint, message.rights())?;
        let (endpoint, badge) = (held.object, held.badge);
        self.call_to(thread, endpoint, badge, message, range)
    }

    /// `call` on an endpoint already judged: the endpoint, or the mailbox
    /// of this node (badge 0).
    pub(crate) fn call_to(
        &mut self,
        thread: ObjId,
        endpoint: ObjId,
        badge: u64,
        message: Message,
        range: Range,
    ) -> Outcome {
        message.check(self.has_page(thread))?;
        self.listed(thread, &message.caps)?;
        let window = self.window(thread, range)?;
        let parcel = Parcel::new(thread, message, badge);
        self.enter(thread, endpoint, parcel, window);
        Ok(Output::Waits)
    }

    /// IPC, the state part of `call`, also the delivery of a fault: the
    /// message is delivered to a thread that waits to receive, or the caller
    /// joins the queue of the endpoint.
    pub(crate) fn enter(&mut self, thread: ObjId, endpoint: ObjId, parcel: Parcel, window: Window) {
        match self.receiver(endpoint) {
            // IPC: a thread waits to receive.
            Some((server, reply, into)) => {
                self.unlink(server);
                let delivery = self.serve(thread, parcel, window, (server, reply, into));
                self.wake(server, Ok(Output::Message(delivery)));
            }
            // IPC: the caller joins the queue of the endpoint.
            None => {
                live(&mut self.endpoints, endpoint).queue.push_back(thread);
                live(&mut self.threads, thread).wait = Wait::Call {
                    endpoint,
                    parcel,
                    window,
                };
                self.observe();
            }
        }
    }

    pub(crate) fn recv(
        &mut self,
        thread: ObjId,
        endpoint: SlotAddr,
        reply: SlotAddr,
        range: Range,
    ) -> Outcome {
        let endpoint = self.object(thread, endpoint, ObjectKind::Endpoint, Rights::RECV)?;
        let reply = self.object(thread, reply, ObjectKind::Reply, Rights::RECV)?;
        let window = self.window(thread, range)?;
        // IPC: the reply object must be idle, `busy` before anything is
        // received: a pending notification stays pending.
        if self.replies[&reply] != ReplyState::Idle {
            return Err(Error::Busy);
        }
        self.receive(thread, endpoint, reply, window)
    }

    pub(crate) fn reply_recv(
        &mut self,
        thread: ObjId,
        reply: SlotAddr,
        message: Message,
        endpoint: SlotAddr,
        range: Range,
    ) -> Outcome {
        // IPC: `send` on the reply object, then as `recv` with it.
        let rights = message.rights().union(Rights::RECV);
        let reply = self.object(thread, reply, ObjectKind::Reply, rights)?;
        message.check(self.has_page(thread))?;
        self.listed(thread, &message.caps)?;
        let endpoint = self.object(thread, endpoint, ObjectKind::Endpoint, Rights::RECV)?;
        let window = self.window(thread, range)?;
        // IPC: the reply object may here be armed or idle; attached to
        // another thread is `busy`.
        if let ReplyState::Attached(_) = self.replies[&reply] {
            return Err(Error::Busy);
        }
        // IPC: if the reply object is not armed the reply is dropped
        // without an error. Transfer of capabilities: the server is not
        // told that the capabilities of its reply were dropped.
        self.answer(reply, Parcel::new(thread, message, 0));
        self.receive(thread, endpoint, reply, window)
    }

    pub(crate) fn send(&mut self, thread: ObjId, target: SlotAddr, message: Message) -> Outcome {
        let cap = self.cap(thread, target, Rights::READ)?;
        // IPC: `send` takes an endpoint or a reply object.
        let kind = match self.objects[&self.caps[&cap].object].kind {
            ObjectKind::Reply => ObjectKind::Reply,
            _ => ObjectKind::Endpoint,
        };
        let held = self.is(cap, kind, message.rights())?;
        let (target, badge) = (held.object, held.badge);
        self.send_to(thread, target, kind, badge, message)
    }

    /// `send` on an object already judged: an endpoint (the mailbox of
    /// this node too) or a reply object.
    pub(crate) fn send_to(
        &mut self,
        thread: ObjId,
        target: ObjId,
        kind: ObjectKind,
        badge: u64,
        message: Message,
    ) -> Outcome {
        // IPC: a reply carries badge 0, the badge of every reply capability.
        message.check(self.has_page(thread))?;
        self.listed(thread, &message.caps)?;
        let parcel = Parcel::new(thread, message, badge);
        let flags = if kind == ObjectKind::Reply {
            self.answer(target, parcel)
        } else {
            // The reply object of the receiver becomes idle: no caller
            // waits for an answer. Delivered only if the first receiver has
            // an own context (D-110).
            let own = |server: &ObjId| self.threads[server].home.is_some();
            let receiver = self.receiver(target).filter(|(server, ..)| own(server));
            receiver.map(|(server, _, into)| {
                self.unlink(server);
                let (delivery, flags) = self.deliver(parcel, server, into, 0);
                self.wake(server, Ok(Output::Message(delivery)));
                flags
            })
        };
        // IPC: `send` never queues. Transfer of capabilities: it returns
        // the flag at once.
        Ok(Output::Sent {
            flags: flags.ok_or(Error::WouldWait)?,
        })
    }

    pub(crate) fn signal(&mut self, thread: ObjId, notify: SlotAddr) -> Outcome {
        let held = self.held(thread, notify, ObjectKind::Notify, Rights::SEND)?;
        let (id, badge) = (held.object, held.badge);
        // Notifications: the bits of the badge, or bit 0 without a badge.
        live(&mut self.notifies, id).word |= if badge == 0 { 1 } else { badge };
        self.hand_over(id);
        Ok(Output::Done)
    }

    /// Notifications: gives the word of a notify, if it is not zero, to the
    /// first thread that waits on the notify, or else to the first thread
    /// that waits to receive on the bound endpoint, and clears it.
    pub(crate) fn hand_over(&mut self, id: ObjId) {
        let notify = &self.notifies[&id];
        let (word, bound) = (notify.word, notify.endpoint);
        if word == 0 {
            return;
        }
        if let Some(&waiter) = notify.waiters.front() {
            self.unlink(waiter);
            self.wake(waiter, Ok(Output::Word(word)));
        } else if let Some((server, ..)) = bound
            .and_then(|bound| self.receiver(bound))
            .filter(|(server, ..)| self.threads[server].home.is_some())
        {
            // That thread's reply object becomes idle.
            self.unlink(server);
            self.wake(server, Ok(Output::Message(notification(word))));
        } else {
            return;
        }
        live(&mut self.notifies, id).word = 0;
    }

    /// `wait` and `poll`.
    pub(crate) fn wait(&mut self, thread: ObjId, notify: SlotAddr, if_zero: IfZero) -> Outcome {
        let id = self.object(thread, notify, ObjectKind::Notify, Rights::RECV)?;
        // Notifications: returns the word and clears it.
        let word = std::mem::take(&mut live(&mut self.notifies, id).word);
        if word != 0 {
            return Ok(Output::Word(word));
        }
        if let IfZero::WouldWait = if_zero {
            return Err(Error::WouldWait);
        }
        live(&mut self.notifies, id).waiters.push_back(thread);
        live(&mut self.threads, thread).wait = Wait::Notify(id);
        self.observe();
        Ok(Output::Waits)
    }

    pub(crate) fn notify_bind(
        &mut self,
        thread: ObjId,
        notify: SlotAddr,
        endpoint: SlotAddr,
    ) -> Outcome {
        let notify = self.object(thread, notify, ObjectKind::Notify, Rights::RECV)?;
        let endpoint = match endpoint.is_none() {
            true => None,
            false => Some(self.object(thread, endpoint, ObjectKind::Endpoint, Rights::MANAGE)?),
        };
        let bound = self.notifies[&notify].endpoint;
        match endpoint {
            // Notifications: endpoint 0 removes the binding, and succeeds
            // if there is none.
            None => {
                if let Some(bound) = bound {
                    live(&mut self.endpoints, bound).notify = None;
                }
            }
            // Notifications: one notify per endpoint, one endpoint per
            // notify; `busy` otherwise, also for the pair that is already
            // bound.
            Some(endpoint) => {
                if bound.is_some() || self.endpoints[&endpoint].notify.is_some() {
                    return Err(Error::Busy);
                }
                live(&mut self.endpoints, endpoint).notify = Some(notify);
            }
        }
        live(&mut self.notifies, notify).endpoint = endpoint;
        // Notifications: a pending word is delivered at once to a thread
        // that already waits to receive.
        self.hand_over(notify);
        Ok(Output::Done)
    }

    /// The list of a message as the node of its sender judges it: each
    /// capability, and the rights of a derivation (`None` for a move).
    pub(crate) fn listed_for(
        &self,
        sender: ObjId,
        transfers: &[Transfer],
    ) -> Result<Vec<(CapId, Option<Rights>)>, Error> {
        let listed = self.listed(sender, transfers)?;
        Ok(listed
            .into_iter()
            .map(|listed| match listed {
                Listed::Move(cap) => (cap, None),
                Listed::Derive(cap, rights) => (cap, Some(rights)),
            })
            .collect())
    }

    /// The receive range of a call, judged on the caller's node.
    pub(crate) fn window_of(&self, thread: ObjId, range: Range) -> Result<Window, Error> {
        self.window(thread, range)
    }

    /// Transfer of capabilities: the checks of the list of a message, made
    /// when the call is made and again at delivery, against what the slots
    /// of the sender hold then.
    fn listed(&self, sender: ObjId, transfers: &[Transfer]) -> Result<Vec<Listed>, Error> {
        let mut caps: Vec<Listed> = Vec::new();
        for transfer in transfers {
            let listed = match *transfer {
                Transfer::Move(slot) => Listed::Move(self.cap(sender, slot, Rights::WRITE)?),
                Transfer::Derive(slot, rights) => {
                    let cap = self.cap(sender, slot, Rights::READ)?;
                    let held = self.caps[&cap].rights;
                    // The last of the three checks of the slot: `derive`.
                    if !held.contains(Rights::DERIVE) {
                        return Err(Error::Denied);
                    }
                    // Capabilities and objects: the rights word is a plain
                    // argument, judged with its slot: a bit that names no
                    // right is `invalid`, before the subset.
                    let rights = Rights::from_bits(rights).ok_or(Error::Invalid)?;
                    if !held.contains(rights) {
                        return Err(Error::Denied);
                    }
                    Listed::Derive(cap, rights)
                }
            };
            // Transfer of capabilities: a slot listed twice is `invalid`;
            // this, then the depth of a capability to derive, are checked
            // with the slot, after its three checks and before the next
            // listed slot.
            let twice = caps.iter().any(|earlier| earlier.cap() == listed.cap());
            let too_deep = match listed {
                Listed::Move(_) => false,
                Listed::Derive(cap, _) => self.depth(cap) >= TREE_DEPTH,
            };
            if twice || too_deep {
                return Err(Error::Invalid);
            }
            caps.push(listed);
        }
        Ok(caps)
    }

    /// IPC, the state part of `recv` and `reply_recv`, in order: a pending
    /// notification of the bound notify, the first caller in the queue,
    /// otherwise the thread waits with the reply object attached.
    fn receive(&mut self, thread: ObjId, endpoint: ObjId, reply: ObjId, window: Window) -> Outcome {
        // In order: the lends of the thread and of the reply object are
        // released; a pending notification only for a thread with an own
        // context (D-110).
        self.release_for_receive(thread, reply);
        let own = self.threads[&thread].home.is_some();
        if let Some(notify) = self.endpoints[&endpoint].notify.filter(|_| own) {
            let word = std::mem::take(&mut live(&mut self.notifies, notify).word);
            if word != 0 {
                return Ok(Output::Message(notification(word)));
            }
        }
        // Faults: a thread without an own context that finds a fault *no
        // context* first in the queue does not receive it: the faulting
        // thread leaves the queue and is suspended with its fault recorded,
        // and the receive returns `again`, having received nothing.
        if !own
            && let Some(&first) = self.endpoints[&endpoint].queue.front()
            && let Wait::Call { parcel, .. } = &self.threads[&first].wait
            && self.threads[&first].faulting
            && parcel.label == dodos_abi::FaultKind::NoContext as u32
        {
            let words: [u64; 5] = parcel.words[..].try_into().expect("five words");
            self.unlink(first);
            let first_thread = live(&mut self.threads, first);
            (first_thread.faulting, first_thread.raised) = (false, None);
            self.record(first, dodos_abi::FaultKind::NoContext as u32, words);
            return Err(Error::Again);
        }
        if let Some((caller, parcel, theirs)) = self.take_caller(endpoint) {
            let delivery = self.serve(caller, parcel, theirs, (thread, reply, window));
            // A delivery whose capabilities another node grants waits for
            // that node.
            if let Some(id) = self.transfer.take() {
                self.transfer_wait(id, thread, delivery);
                return Ok(Output::Waits);
            }
            return Ok(Output::Message(delivery));
        }
        live(&mut self.endpoints, endpoint).queue.push_back(thread);
        *live(&mut self.replies, reply) = ReplyState::Attached(thread);
        live(&mut self.threads, thread).wait = Wait::Recv {
            endpoint,
            reply,
            window,
        };
        self.observe();
        Ok(Output::Waits)
    }

    /// IPC: the message of a caller reaches a server. The reply object of
    /// the server is armed with the caller, which waits for the reply.
    fn serve(
        &mut self,
        caller: ObjId,
        parcel: Parcel,
        window: Window,
        (server, reply, into): (ObjId, ObjId, Window),
    ) -> Delivery {
        // Faults: a fault *no context* reaches only a receiver with an own
        // context; the event log keeps what each delivery found.
        if self.threads[&caller].faulting && parcel.label == dodos_abi::FaultKind::NoContext as u32
        {
            let own = self.threads[&server].home.is_some();
            self.event.no_context_delivered.push(own);
        }
        // Transfer of capabilities: the reply will tell the caller if its
        // capabilities were dropped.
        let (delivery, flags) = self.deliver(parcel, server, into, 0);
        *live(&mut self.replies, reply) = ReplyState::Armed {
            caller,
            window,
            flags,
        };
        if let Some(id) = self.transfer {
            self.transfer_reply(id, reply);
        }
        // The budget follows the call: the delivery lends the caller's
        // context to a receiver that has none of its own (D-110).
        self.lend(caller, reply, server);
        live(&mut self.threads, caller).wait = Wait::Reply(reply);
        self.observe();
        delivery
    }

    /// IPC: answers the caller that a reply object is armed with and makes
    /// the object idle. `None` if it is not armed; otherwise the flags for
    /// the thread that answers.
    fn answer(&mut self, reply: ObjId, parcel: Parcel) -> Option<u32> {
        let ReplyState::Armed {
            caller,
            window,
            flags,
        } = self.replies[&reply]
        else {
            return None;
        };
        self.unlink(caller);
        // Transfer of capabilities: the flag in the header of a reply means
        // that the capabilities of the request or of the reply were dropped.
        let (delivery, dropped) = self.deliver(parcel, caller, window, flags);
        self.wake(caller, Ok(Output::Message(delivery)));
        Some(dropped)
    }

    /// Bindings: whether a thread has a message page.
    fn has_page(&self, thread: ObjId) -> bool {
        self.threads[&thread].page.is_some()
    }

    /// Transfer of capabilities, checked again at delivery: all or nothing.
    /// Returns what the receiver gets, and `FLAG_CAPS_DROPPED` or 0.
    fn deliver(
        &mut self,
        mut parcel: Parcel,
        receiver: ObjId,
        window: Window,
        flags: u32,
    ) -> (Delivery, u32) {
        // Transfer of capabilities: if the sender's message page was unbound
        // while it waited, the list is lost, and the capabilities are
        // dropped; the words that were in the page are delivered as 0
        // (D-110).
        let lost = !self.has_page(parcel.sender);
        if lost {
            parcel
                .words
                .iter_mut()
                .skip(REG_WORDS)
                .for_each(|word| *word = 0);
        }
        let index = |i: usize| window.first as usize + i;
        let empty = |index: usize| {
            // Removal ends: a range in a dying or destroyed space takes
            // nothing.
            let space = self
                .cspaces
                .get(&window.cspace)
                .filter(|_| !self.dying(window.cspace));
            space.and_then(|space| space.slots.get(index)) == Some(&Slot::Empty)
        };
        // The same checks as when the call was made, and the first slots of
        // the receiver's range are empty, one per capability.
        // The derivation tree: a capability moved or derived by IPC into a
        // space that cannot hold it is refused at delivery, which drops the
        // capabilities.
        let fits = |listed: &Listed| {
            let placing = match listed {
                Listed::Move(_) => Placing::Moved,
                Listed::Derive(..) => Placing::Derived,
            };
            self.fits(window.cspace, listed.cap(), placing).is_ok()
        };
        let listed = self
            .listed(parcel.sender, &parcel.caps)
            .ok()
            .filter(|caps| caps.is_empty() || !lost)
            .filter(|caps| {
                caps.len() <= window.count as usize
                    && (0..caps.len()).all(|i| empty(index(i)))
                    && caps.iter().all(fits)
            });
        for (i, &listed) in listed.iter().flatten().enumerate() {
            let place = Place {
                space: window.cspace,
                index: index(i) as u32,
            };
            match listed {
                Listed::Move(cap) => self.place(cap, place),
                // A derived capability keeps its badge.
                Listed::Derive(cap, rights) => {
                    self.derive(cap, rights, self.caps[&cap].badge, place);
                }
            }
        }
        // If not, the words are still delivered, with the flag and a count
        // of 0.
        let (caps, dropped) = match listed {
            Some(caps) => (caps.len(), 0),
            None => (0, Header::FLAG_CAPS_DROPPED),
        };
        let header = Header::new(parcel.label, parcel.words.len(), caps, flags | dropped);
        // Bindings: a delivery to a thread without a message page writes the
        // register words only; the header keeps the number of words sent.
        if !self.has_page(receiver) {
            parcel.words.truncate(REG_WORDS);
        }
        // Tasks and marks: the stamp, taken at delivery: the task of the
        // sender and its effective marks with the marks it added. The
        // receiver takes them unless it is attached to a mark-aware task;
        // a sender that transferred a capability takes the receiver's marks
        // as they were, unless it is attached to one.
        let sender = parcel.sender;
        // A proxy stands for a thread of another node: the stamp is that
        // thread's, taken when the request left its node.
        let (task, marks) = match self.threads[&sender].stamp {
            Some(stamp) => stamp,
            None => (
                self.threads[&sender]
                    .task
                    .map_or(0, |task| self.tasks[&task].id.raw()),
                self.marks(sender).expect("a live sender").bits(),
            ),
        };
        let stamp = marks | parcel.marks;
        let before = self.marks(receiver).expect("a live receiver").bits();
        if !self.aware(receiver) {
            self.mark(receiver, stamp);
        }
        let proxy = self.threads[&sender].stamp.is_some();
        if caps > 0 && !proxy && !self.aware(sender) {
            self.mark(sender, before);
        }
        let mut received = received(
            header.expect("checked when the call was made"),
            parcel.badge,
        );
        (received.marks, received.task) = (Marks::from_bits(stamp).expect("known marks"), task);
        let delivery = Delivery {
            received,
            words: parcel.words,
        };
        // Grants: the capabilities of a proxy's message are those of the
        // thread it stands for; the receiver waits while that node grants
        // them.
        if self.threads[&sender].foreign.is_some() {
            let id = self.defer_transfer(receiver, sender, delivery.clone(), window);
            (self.transfer, self.last_transfer) = (Some(id), Some(id));
        }
        (delivery, dropped)
    }

    /// The first thread in the queue of an endpoint, if it waits to
    /// receive: the thread, its reply object, its receive range.
    pub(crate) fn receiver(&self, endpoint: ObjId) -> Option<(ObjId, ObjId, Window)> {
        let &thread = self.endpoints[&endpoint].queue.front()?;
        match self.threads[&thread].wait {
            Wait::Recv { reply, window, .. } => Some((thread, reply, window)),
            _ => None,
        }
    }

    /// The first thread in the queue of an endpoint, taken out of it, if it
    /// waits to be received: the thread, its message, its receive range.
    fn take_caller(&mut self, endpoint: ObjId) -> Option<(ObjId, Parcel, Window)> {
        let &thread = self.endpoints[&endpoint].queue.front()?;
        let Wait::Call { parcel, window, .. } = &self.threads[&thread].wait else {
            return None;
        };
        let found = (thread, parcel.clone(), *window);
        self.unlink(thread);
        Some(found)
    }

    /// Takes a thread out of what it waits on. A reply object attached to
    /// the thread or armed with it becomes idle.
    ///
    /// IPC: the queues are doubly linked through the thread objects, so in a
    /// kernel this is one step. The scan of the queue here is a property of
    /// the model, whose queues are vectors.
    pub(crate) fn unlink(&mut self, thread: ObjId) {
        let wait = std::mem::take(&mut live(&mut self.threads, thread).wait);
        let other = |&queued: &ObjId| queued != thread;
        match wait {
            Wait::None => {}
            Wait::Call { endpoint, .. } => live(&mut self.endpoints, endpoint).queue.retain(other),
            Wait::Reply(reply) => *live(&mut self.replies, reply) = ReplyState::Idle,
            Wait::Recv {
                endpoint, reply, ..
            } => {
                live(&mut self.endpoints, endpoint).queue.retain(other);
                *live(&mut self.replies, reply) = ReplyState::Idle;
            }
            Wait::Notify(notify) => live(&mut self.notifies, notify).waiters.retain(other),
            // A wait for another node: the request is forgotten, and a wait
            // that a proxy holds there is ended there.
            Wait::Node(id) => match self.requests.get_mut(&id) {
                // A move goes on without the thread that left: its answer
                // still ends the other's wait.
                Some(Request::Move { caller, source, .. }) => {
                    for waiting in [caller, source] {
                        if *waiting == Some(thread) {
                            *waiting = None;
                        }
                    }
                }
                _ => {
                    self.cancel_remote(id);
                    self.requests.remove(&id);
                }
            },
        }
        self.observe();
    }
}
