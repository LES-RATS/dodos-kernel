//! IPC, transfer of capabilities and notifications.

use crate::caps::Placing;
use crate::{
    CapId, Delivery, Message, Model, ObjId, Outcome, Output, Parcel, Range, ReplyState, Slot,
    Transfer, Wait, Window, live,
};
use dodos_abi::{Error, Header, Marks, ObjectKind, Received, Rights, SlotAddr, TREE_DEPTH};

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
    /// capabilities), or with a flag bit set by the sender, it is `invalid`
    /// before any other capability argument is looked at.
    fn check(&self) -> Result<(), Error> {
        let header = Header::new(self.label, self.words.len(), self.caps.len(), self.flags)?;
        if header.flags() != 0 {
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
    // CONTRACT-GAP (Notifications): the contract gives the flag and the
    // badge field of this header, not the rest. The label is 0 and there
    // are no words.
    let header = Header::new(0, 0, 0, Header::FLAG_NOTIFICATION).expect("an empty message");
    Delivery {
        received: received(header, word),
        words: Vec::new(),
    }
}

/// What the kernel sets on a received message.
fn received(header: Header, badge: u64) -> Received {
    // Messages: the receiver gets the marks and the identifier of the
    // sender's task. Tasks are not modelled: the marks are none and the
    // task is 0, and the oracle says nothing on what a kernel stamps.
    Received {
        header,
        badge,
        marks: Marks::NONE,
        task: 0,
    }
}

impl Parcel {
    fn new(sender: ObjId, message: Message, badge: u64) -> Self {
        Self {
            label: message.label,
            words: message.words,
            badge,
            sender,
            caps: message.caps,
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
        message.check()?;
        self.listed(thread, &message.caps)?;
        let window = self.window(thread, range)?;
        let receiver = self.receiver(endpoint);
        let parcel = Parcel::new(thread, message, badge);
        match receiver {
            // IPC: a thread waits to receive.
            Some((server, reply, into)) => {
                self.unlink(server);
                let delivery = self.serve(thread, parcel, window, reply, into);
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
            }
        }
        Ok(Output::Waits)
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
        message.check()?;
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
        // IPC: a reply carries badge 0, the badge of every reply capability.
        message.check()?;
        self.listed(thread, &message.caps)?;
        let parcel = Parcel::new(thread, message, badge);
        let flags = if kind == ObjectKind::Reply {
            self.answer(target, parcel)
        } else {
            // The reply object of the receiver becomes idle: no caller
            // waits for an answer.
            self.receiver(target).map(|(server, _, into)| {
                self.unlink(server);
                let (delivery, flags) = self.deliver(parcel, into, 0);
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
    fn hand_over(&mut self, id: ObjId) {
        let notify = &self.notifies[&id];
        let (word, bound) = (notify.word, notify.endpoint);
        if word == 0 {
            return;
        }
        if let Some(&waiter) = notify.waiters.front() {
            self.unlink(waiter);
            self.wake(waiter, Ok(Output::Word(word)));
        } else if let Some((server, ..)) = bound.and_then(|bound| self.receiver(bound)) {
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
                    Listed::Derive(cap, self.caps[&cap].rights.derive(rights)?)
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
        if let Some(notify) = self.endpoints[&endpoint].notify {
            let word = std::mem::take(&mut live(&mut self.notifies, notify).word);
            if word != 0 {
                return Ok(Output::Message(notification(word)));
            }
        }
        if let Some((caller, parcel, theirs)) = self.take_caller(endpoint) {
            let delivery = self.serve(caller, parcel, theirs, reply, window);
            return Ok(Output::Message(delivery));
        }
        live(&mut self.endpoints, endpoint).queue.push_back(thread);
        *live(&mut self.replies, reply) = ReplyState::Attached(thread);
        live(&mut self.threads, thread).wait = Wait::Recv {
            endpoint,
            reply,
            window,
        };
        Ok(Output::Waits)
    }

    /// IPC: the message of a caller reaches a server. The reply object of
    /// the server is armed with the caller, which waits for the reply.
    fn serve(
        &mut self,
        caller: ObjId,
        parcel: Parcel,
        window: Window,
        reply: ObjId,
        into: Window,
    ) -> Delivery {
        // Transfer of capabilities: the reply will tell the caller if its
        // capabilities were dropped.
        let (delivery, flags) = self.deliver(parcel, into, 0);
        *live(&mut self.replies, reply) = ReplyState::Armed {
            caller,
            window,
            flags,
        };
        live(&mut self.threads, caller).wait = Wait::Reply(reply);
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
        let (delivery, dropped) = self.deliver(parcel, window, flags);
        self.wake(caller, Ok(Output::Message(delivery)));
        Some(dropped)
    }

    /// Transfer of capabilities, checked again at delivery: all or nothing.
    /// Returns what the receiver gets, and `FLAG_CAPS_DROPPED` or 0.
    fn deliver(&mut self, parcel: Parcel, window: Window, flags: u32) -> (Delivery, u32) {
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
            .filter(|caps| {
                caps.len() <= window.count as usize
                    && (0..caps.len()).all(|i| empty(index(i)))
                    && caps.iter().all(fits)
            });
        for (i, &listed) in listed.iter().flatten().enumerate() {
            let place = (window.cspace, index(i) as u32);
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
        let delivery = Delivery {
            received: received(
                header.expect("checked when the call was made"),
                parcel.badge,
            ),
            words: parcel.words,
        };
        (delivery, dropped)
    }

    /// The first thread in the queue of an endpoint, if it waits to
    /// receive: the thread, its reply object, its receive range.
    fn receiver(&self, endpoint: ObjId) -> Option<(ObjId, ObjId, Window)> {
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

    /// Takes a thread out of what it waits on and returns what that was. A
    /// reply object attached to the thread or armed with it becomes idle.
    ///
    /// IPC: the queues are doubly linked through the thread objects, so in a
    /// kernel this is one step. The scan of the queue here is a property of
    /// the model, whose queues are vectors.
    pub(crate) fn unlink(&mut self, thread: ObjId) -> Wait {
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
        }
        wait
    }
}
