//! Calls on an object of another node, and grants (Memory and devices
//! between nodes): what one node does with the messages between kernels.
//! The machine that carries them, and lets a test choose the order in which
//! they arrive, is `System`.

use crate::table::{GrantEntry, GrantState, UseEntry};
use crate::{
    Cap, CapId, Delivery, Frame, Model, ObjId, Object, Outcome, Output, Place, Record, Slot,
    Syscall, Transfer, Wait, Window, live,
};
use dodos_abi::{
    Error, GRANTS, Header, MemberState, ObjectKind, PROXIES_PER_PEER, Rights, SlotAddr,
};

/// One message between kernels.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub from: u16,
    pub to: u16,
    pub body: Body,
}

/// What a message between kernels carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// A call on an object of the receiving node, its capability arguments
    /// rewritten to the anchors of their grants, made by a thread with
    /// this stamp, word "cancelled up to" and wait number; `caps` are the
    /// capabilities its message lists, slots of the calling node.
    Request {
        id: u64,
        call: Syscall,
        stamp: (u64, u64),
        cancelled_up_to: u64,
        number: u64,
        caps: Vec<Transfer>,
    },
    /// The outcome of a request.
    Answer { id: u64, outcome: Outcome },
    /// `thread_cancel` of a thread whose wait a proxy holds.
    Cancel { id: u64 },
    /// At a delivery: the receiving node asks the node of the sender to
    /// grant the capabilities its request listed.
    TransferAsk { id: u64, request: u64 },
    /// The grants, or `None` if the capabilities are dropped.
    TransferGrant {
        id: u64,
        grants: Option<Vec<Crossing>>,
    },
    /// The granting node asks the holder to remove its root remote
    /// capability of a revoking grant, one step per message.
    Revoke { id: u64, grant: u16, serial: u32 },
    /// The holder's answer after each step.
    RevokeStep { id: u64, done: bool },
    /// The holder removed its root remote capability of a grant.
    Release { grant: u16, serial: u32 },
    /// The receiving node refused the grants of a transfer at delivery: no
    /// capability moves.
    Refused { grants: Vec<(u16, u32)> },
    /// The receiving node took back the anchors of these grants, moved
    /// home by a transfer: the holder removes its remote capability.
    Returned { grants: Vec<(u16, u32)> },
}

/// One capability of a transfer as the receiving node gets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Crossing {
    /// A new grant of the sending node.
    Grant(Granted),
    /// A remote capability of the receiving node, going back to it: a
    /// derivation (rights and badge) makes an ordinary child of the anchor,
    /// a move moves the anchor's contents into the slot and frees the grant.
    Home {
        grant: u16,
        serial: u32,
        derived: Option<(Rights, u64)>,
    },
}

/// A grant as the holder learns it: the entry, and what the anchor is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Granted {
    pub grant: u16,
    pub serial: u32,
    pub generation: u32,
    pub kind: ObjectKind,
    pub rights: Rights,
    pub badge: u64,
    pub depth: usize,
    /// The anchor is the root capability of its object (a move of a root).
    pub root: bool,
    pub(crate) frame: Option<Frame>,
}

/// A remote object on its holder: the grant its capabilities name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Remote {
    pub(crate) node: u16,
    pub(crate) grant: u16,
    pub(crate) serial: u32,
    pub(crate) use_entry: u16,
    /// The depth of the anchor: the root remote capability has it.
    pub(crate) depth: usize,
    pub(crate) root: bool,
}

/// What a node that sent a request waits for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Request {
    /// The answer to a call of `thread`, on an object of node `peer`, and
    /// whether the call waits on that object (a proxy holds it).
    Call {
        thread: ObjId,
        peer: u16,
        caps: Vec<Transfer>,
        /// The call waits on an object there (`call`, `recv`, `wait`).
        waits: bool,
    },
    /// The grants of the capabilities of a delivery to `receiver`.
    Transfer {
        receiver: ObjId,
        delivery: Delivery,
        window: Window,
        /// A proxy whose `send` waits for the transfer to answer.
        answer: Option<(u16, u64)>,
        /// The reply object armed with a proxy, whose flags wait for it.
        reply: Option<ObjId>,
    },
    /// The removal step of `thread` waits for the holder of `grant`.
    Revoke { thread: ObjId, grant: u16 },
}

/// The index, in the kernel space of a node, of the anchor of grant `g`,
/// of proxy `i` for peer `p`, and of the mailbox capability.
pub(crate) fn anchor_index(grant: u16) -> u32 {
    1 + u32::from(grant)
}

pub(crate) fn mailbox_index() -> u32 {
    1 + GRANTS as u32
}

pub(crate) fn proxy_index(peer: u16, i: usize) -> u32 {
    2 + GRANTS as u32 + u32::from(peer) * PROXIES_PER_PEER as u32 + i as u32
}

/// The capability arguments of a call, in the order of its table: the
/// first is the capability the call acts on.
pub(crate) fn capability_arguments(call: &Syscall) -> Vec<SlotAddr> {
    use Syscall as S;
    match call {
        S::Call { endpoint, .. } => vec![*endpoint],
        S::Recv {
            endpoint, reply, ..
        } => vec![*endpoint, *reply],
        S::Send { target, .. } => vec![*target],
        S::Signal(notify) | S::Poll(notify) | S::Wait(notify, _) => vec![*notify],
        S::NotifyBind { notify, endpoint } => vec![*notify, *endpoint],
        S::ThreadResume(t) | S::ThreadSuspend(t) | S::ThreadReadRegs(t) => vec![*t],
        S::ThreadCancel(t, _) => vec![*t],
        S::ThreadWriteRegs { thread, .. } => vec![*thread],
        S::SchedRead(sched) | S::SchedUnbind(sched) => vec![*sched],
        S::TimerCancel(timer) => vec![*timer],
        S::TimerSet { timer, notify, .. } => vec![*timer, *notify],
        S::IrqAck(irq) => vec![*irq],
        S::IrqBind { irq, notify } => vec![*irq, *notify],
        S::TaskRead(task) | S::TaskRevoke(task) => vec![*task],
        S::TaskMark { task, .. } => vec![*task],
        S::TaskAttach { thread, task } => vec![*thread, *task],
        S::SchedBind { sched, thread } => vec![*sched, *thread],
        _ => Vec::new(),
    }
}

/// The same call with its capability arguments replaced, in order.
fn rewritten(call: &Syscall, mut slots: impl Iterator<Item = SlotAddr>) -> Syscall {
    use Syscall as S;
    let mut next = || slots.next().expect("one slot per capability argument");
    let mut call = call.clone();
    match &mut call {
        S::Call { endpoint, .. } => *endpoint = next(),
        S::Recv {
            endpoint, reply, ..
        } => (*endpoint, *reply) = (next(), next()),
        S::Send { target, .. } => *target = next(),
        S::Signal(notify) | S::Poll(notify) | S::Wait(notify, _) => *notify = next(),
        S::NotifyBind { notify, endpoint } => (*notify, *endpoint) = (next(), next()),
        S::ThreadResume(t) | S::ThreadSuspend(t) | S::ThreadReadRegs(t) => *t = next(),
        S::ThreadCancel(t, _) => *t = next(),
        S::ThreadWriteRegs { thread, .. } => *thread = next(),
        S::SchedRead(sched) | S::SchedUnbind(sched) => *sched = next(),
        S::TimerCancel(timer) => *timer = next(),
        S::TimerSet { timer, notify, .. } => (*timer, *notify) = (next(), next()),
        S::IrqAck(irq) => *irq = next(),
        S::IrqBind { irq, notify } => (*irq, *notify) = (next(), next()),
        S::TaskRead(task) | S::TaskRevoke(task) => *task = next(),
        S::TaskMark { task, .. } => *task = next(),
        S::TaskAttach { thread, task } => (*thread, *task) = (next(), next()),
        S::SchedBind { sched, thread } => (*sched, *thread) = (next(), next()),
        _ => {}
    }
    call
}

impl Model {
    /// Messages between kernels: what this node sends.
    pub(crate) fn send_packet(&mut self, to: u16, body: Body) {
        self.outbox.push(Packet {
            from: self.node,
            to,
            body,
        });
    }

    fn new_request(&mut self) -> u64 {
        self.next_request += 1;
        u64::from(self.node) << 48 | self.next_request
    }

    /// Whether a remote object is valid: its grant entry is live with its
    /// serial and the granting node is live with the generation recorded
    /// in the use entry.
    pub(crate) fn valid(&self, remote: &Remote) -> bool {
        let entry = self.table.grants[remote.node as usize].get(&remote.grant);
        let used = self.table.uses[self.node as usize].get(&remote.use_entry);
        let member = self.table.member(remote.node);
        entry.is_some_and(|e| e.state == GrantState::Live && e.serial == remote.serial)
            && used
                .is_some_and(|u| u.serial == remote.serial && member.generation() == u.generation)
            && member.state() == MemberState::Live
    }

    /// Calls on an object of another node: if the capability a call acts on
    /// is remote, or is a `core` capability with `send` naming another
    /// node's mailbox, the call becomes a message. `None` if it is not.
    pub(crate) fn forwarded(&mut self, thread: ObjId, call: &Syscall) -> Option<Outcome> {
        let slots = capability_arguments(call);
        let &target = slots.first()?;
        // The thread's own state first, then the slot of the target.
        let Ok(cap) = self.cap(thread, target, Rights::READ) else {
            return None;
        };
        let object = self.caps[&cap].object;
        if self.objects[&object].kind == ObjectKind::Core
            && matches!(call, Syscall::Call { .. } | Syscall::Send { .. })
        {
            return Some(self.mailbox(thread, call, cap));
        }
        let remote = *self.remotes.get(&object)?;
        // A use of an invalid remote capability is `gone`, judged with the
        // slot, and the slot becomes dead.
        if !self.valid(&remote) {
            self.remove_leaf_dead(cap);
            return Some(Err(Error::Gone));
        }
        Some(self.forward(thread, call, &slots, remote.node))
    }

    /// The mailbox of a node (Machine objects): `call` and `send` on a
    /// `core` capability with `send` go to the mailbox of that node's
    /// current generation; `gone` if the node is not live.
    fn mailbox(&mut self, thread: ObjId, call: &Syscall, cap: CapId) -> Outcome {
        let rights = match call {
            Syscall::Call { message, .. } | Syscall::Send { message, .. } => {
                message_rights(message)
            }
            _ => Rights::SEND,
        };
        if !self.caps[&cap].rights.contains(rights) {
            return Err(Error::Denied);
        }
        let node = self.cores[&self.caps[&cap].object] as u16;
        if self.table.member(node).state() != MemberState::Live {
            return Err(Error::Gone);
        }
        if node == self.node {
            let mailbox = self.mailbox.expect("a mailbox");
            return match call.clone() {
                Syscall::Call { message, range, .. } => {
                    self.call_to(thread, mailbox, 0, message, range)
                }
                Syscall::Send { message, .. } => {
                    self.send_to(thread, mailbox, ObjectKind::Endpoint, 0, message)
                }
                _ => unreachable!("a call or a send"),
            };
        }
        let call = rewritten(call, [SlotAddr::own(mailbox_index())].into_iter());
        self.request(thread, node, call)
    }

    /// The caller's kernel judges the slot of every other capability
    /// argument; one that names no object of the target's node is
    /// `invalid` (a call is between two nodes, never three).
    fn forward(&mut self, thread: ObjId, call: &Syscall, slots: &[SlotAddr], node: u16) -> Outcome {
        let mut anchors = Vec::new();
        for &slot in slots {
            if slot.is_none() {
                anchors.push(SlotAddr::NONE);
                continue;
            }
            let cap = self.cap(thread, slot, Rights::READ)?;
            let remote = self.remotes.get(&self.caps[&cap].object).copied();
            match remote {
                Some(remote) if remote.node == node && self.valid(&remote) => {
                    anchors.push(SlotAddr::own(anchor_index(remote.grant)));
                }
                Some(remote) if remote.node == node => {
                    self.remove_leaf_dead(cap);
                    return Err(Error::Gone);
                }
                _ => return Err(Error::Invalid),
            }
        }
        let call = rewritten(call, anchors.into_iter());
        self.request(thread, node, call)
    }

    /// The message of a request: its listed capabilities are judged now,
    /// on this node, and transferred at delivery by grants; the thread
    /// waits in the state *waiting for another node*.
    fn request(&mut self, thread: ObjId, node: u16, mut call: Syscall) -> Outcome {
        let caps = match &mut call {
            Syscall::Call { message, range, .. } => {
                message.check_header(self.threads[&thread].page.is_some())?;
                self.listed_for(thread, &message.caps)?;
                // The receive range is this node's: an answer's capabilities
                // are not carried back.
                self.window_of(thread, *range)?;
                std::mem::take(&mut message.caps)
            }
            Syscall::Send { message, .. } => {
                message.check_header(self.threads[&thread].page.is_some())?;
                self.listed_for(thread, &message.caps)?;
                std::mem::take(&mut message.caps)
            }
            _ => Vec::new(),
        };
        let id = self.new_request();
        let number = match call {
            Syscall::Call { wait, .. } | Syscall::Recv { wait, .. } | Syscall::Wait(_, wait) => {
                wait
            }
            _ => 0,
        };
        let stamp = self.stamp_of(thread);
        let waits = matches!(
            call,
            Syscall::Call { .. } | Syscall::Recv { .. } | Syscall::Wait(..)
        );
        let body = Body::Request {
            id,
            call,
            stamp,
            cancelled_up_to: self.threads[&thread].cancelled_up_to,
            number,
            caps: caps.clone(),
        };
        self.requests.insert(
            id,
            Request::Call {
                thread,
                peer: node,
                caps,
                waits,
            },
        );
        live(&mut self.threads, thread).wait = Wait::Node(id);
        self.observe();
        // A request that finds no free proxy waits on this node, first in
        // first out.
        let outstanding = self.outstanding.entry(node).or_default();
        if *outstanding < PROXIES_PER_PEER {
            *outstanding += 1;
            self.send_packet(node, body);
        } else {
            self.held_requests.entry(node).or_default().push_back(body);
        }
        Ok(Output::Waits)
    }

    /// The stamp a delivery takes: the task identifier and the effective
    /// marks of the thread.
    fn stamp_of(&self, thread: ObjId) -> (u64, u64) {
        let task = self.threads[&thread]
            .task
            .map_or(0, |t| self.tasks[&t].id.raw());
        (task, self.marks(thread).map_or(0, |m| m.bits()))
    }

    /// A message between kernels arrives: an event of this node.
    pub(crate) fn receive_packet(&mut self, packet: Packet) {
        *self.last_event = None;
        let entry = self.begin();
        let from = packet.from;
        match packet.body {
            Body::Request {
                id,
                call,
                stamp,
                cancelled_up_to,
                number,
                caps,
            } => self.serve_request(from, id, call, (stamp, cancelled_up_to, number), caps),
            Body::Answer { id, outcome } => self.answered_request(from, id, outcome),
            Body::Cancel { id } => self.cancel_proxy(from, id),
            Body::TransferAsk { id, request } => self.grant_transfer(from, id, request),
            Body::TransferGrant { id, grants } => self.place_transfer(from, id, grants),
            Body::Revoke { id, grant, serial } => self.revoke_step(from, id, grant, serial),
            Body::RevokeStep { id, done } => self.revoked_step(id, done),
            Body::Release { grant, serial } => self.released(grant, serial),
            Body::Refused { grants } => self.refused(grants),
            Body::Returned { grants } => self.returned(from, grants),
        }
        self.end(entry);
    }

    /// A proxy of node `from` makes the call, with the stamp and the wait
    /// number of the thread it stands for.
    fn serve_request(
        &mut self,
        from: u16,
        id: u64,
        call: Syscall,
        (stamp, cancelled_up_to, number): ((u64, u64), u64, u64),
        caps: Vec<Transfer>,
    ) {
        let proxy = (0..PROXIES_PER_PEER)
            .map(|i| self.proxy_at(from, i))
            .find(|p| self.proxies.get(p) == Some(&None))
            .expect("the calling node counts its requests");
        live(&mut self.proxies, proxy).replace((from, id));
        let standing = live(&mut self.threads, proxy);
        standing.stamp = Some(stamp);
        standing.cancelled_up_to = cancelled_up_to;
        standing.foreign = (!caps.is_empty()).then_some((from, id, caps.len()));
        self.last_transfer = None;
        let outcome = self.dispatch(proxy, call);
        let outcome = match outcome {
            Ok(Output::Waits) if number != 0 && number <= cancelled_up_to => {
                self.unlink(proxy);
                Err(Error::Cancelled)
            }
            Ok(Output::Waits) => {
                live(&mut self.threads, proxy).number = number;
                return;
            }
            other => other,
        };
        // A `send` whose capabilities wait for their grants answers later.
        let transfer = self.last_transfer.take();
        match (transfer, &outcome) {
            (Some(transfer), Ok(Output::Sent { .. })) => {
                if let Some(Request::Transfer { answer, .. }) = self.requests.get_mut(&transfer) {
                    *answer = Some((from, id));
                }
            }
            _ => self.answer_proxy(proxy, outcome),
        }
    }

    /// The proxy of peer `peer` at position `i`.
    fn proxy_at(&self, peer: u16, i: usize) -> ObjId {
        let space = self.kernel_space.expect("a kernel space");
        let index = proxy_index(peer, i) as usize;
        match self.cspaces[&space].slots[index] {
            Slot::Full(cap) => self.caps[&cap].object,
            _ => panic!("proxy {peer}/{i}"),
        }
    }

    /// The call a proxy stands in is over: its outcome goes back, and the
    /// proxy is free again.
    pub(crate) fn answer_proxy(&mut self, proxy: ObjId, outcome: Outcome) {
        let Some(Some((peer, id))) = self.proxies.get(&proxy).copied() else {
            return;
        };
        self.proxies.insert(proxy, None);
        let standing = live(&mut self.threads, proxy);
        (standing.stamp, standing.foreign, standing.outcome) = (None, None, None);
        (standing.number, standing.cancelled_up_to) = (0, 0);
        self.send_packet(peer, Body::Answer { id, outcome });
    }

    /// The answer to a request of this node.
    fn answered_request(&mut self, from: u16, id: u64, outcome: Outcome) {
        if let Some(count) = self.outstanding.get_mut(&from) {
            *count = count.saturating_sub(1);
        }
        if let Some(body) = self
            .held_requests
            .get_mut(&from)
            .and_then(|q| q.pop_front())
        {
            *self.outstanding.entry(from).or_default() += 1;
            self.send_packet(from, body);
        }
        let Some(Request::Call { thread, .. }) = self.requests.remove(&id) else {
            return;
        };
        if self
            .threads
            .get(&thread)
            .is_some_and(|t| t.wait == Wait::Node(id))
        {
            self.unlink(thread);
            live(&mut self.threads, thread).answered = true;
            self.wake(thread, outcome);
        }
    }

    /// `thread_cancel` of a wait held by a proxy: the proxy's wait ends,
    /// and the call returns `cancelled`.
    fn cancel_proxy(&mut self, from: u16, id: u64) {
        let proxy = self
            .proxies
            .iter()
            .find(|(_, held)| **held == Some((from, id)))
            .map(|(&p, _)| p);
        if let Some(proxy) = proxy.filter(|p| self.threads[p].wait != Wait::None) {
            self.unlink(proxy);
            self.wake(proxy, Err(Error::Cancelled));
        }
    }

    /// Ends a wait on an object of the other node: one a proxy holds is
    /// ended there; one still in the list of requests that found no free
    /// proxy leaves it, and returns `Some(true)`. `None` if the thread
    /// waits for the other node on no object: nothing ends.
    pub(crate) fn cancel_remote(&mut self, id: u64) -> Option<bool> {
        let Some(&Request::Call { peer, waits, .. }) = self.requests.get(&id) else {
            return None;
        };
        let held = self.held_requests.get_mut(&peer);
        let position = held.as_ref().and_then(|q| {
            q.iter()
                .position(|b| matches!(b, Body::Request { id: i, .. } if *i == id))
        });
        if let (Some(queue), Some(position)) = (held, position) {
            queue.remove(position);
            self.requests.remove(&id);
            return Some(true);
        }
        if !waits {
            return None;
        }
        self.send_packet(peer, Body::Cancel { id });
        Some(false)
    }

    // Grants.

    /// At the delivery of a proxy's message whose request listed
    /// capabilities: the receiver waits for another node while the
    /// sender's node grants them.
    pub(crate) fn defer_transfer(
        &mut self,
        receiver: ObjId,
        sender: ObjId,
        delivery: Delivery,
        window: Window,
    ) -> u64 {
        let (peer, request, _) = self.threads[&sender].foreign.expect("a foreign message");
        let id = self.new_request();
        self.requests.insert(
            id,
            Request::Transfer {
                receiver,
                delivery,
                window,
                answer: None,
                reply: None,
            },
        );
        self.send_packet(peer, Body::TransferAsk { id, request });
        id
    }

    /// The receiver of a delivery whose capabilities another node grants
    /// waits for that node, with the delivery it will get.
    pub(crate) fn transfer_wait(&mut self, id: u64, receiver: ObjId, delivery: Delivery) {
        if let Some(Request::Transfer {
            receiver: waiting,
            delivery: kept,
            ..
        }) = self.requests.get_mut(&id)
        {
            (*waiting, *kept) = (receiver, delivery);
        }
        live(&mut self.threads, receiver).wait = Wait::Node(id);
        self.observe();
    }

    /// The reply object armed by a delivery whose capabilities another node
    /// grants: its flags wait for the grants.
    pub(crate) fn transfer_reply(&mut self, id: u64, armed: ObjId) {
        if let Some(Request::Transfer { reply, .. }) = self.requests.get_mut(&id) {
            *reply = Some(armed);
        }
    }

    /// The sender's node grants the capabilities its request listed, as
    /// they are in its slots now: one grant entry each, whose anchor is a
    /// child of the source (a derivation) or takes its place (a move).
    fn grant_transfer(&mut self, from: u16, id: u64, request: u64) {
        let Some(Request::Call { thread, caps, .. }) = self.requests.get(&request).cloned() else {
            return self.send_packet(from, Body::TransferGrant { id, grants: None });
        };
        let listed = self
            .listed_for(thread, &caps)
            .ok()
            .filter(|_| self.threads[&thread].page.is_some());
        // A remote capability of the receiving node goes home with no
        // grant; a move home only of a root remote capability with no
        // descendant and no mapping. One of a third node is dropped.
        let home = |model: &Self, cap: CapId| model.remotes.get(&model.caps[&cap].object).copied();
        let allowed = listed.as_ref().is_some_and(|listed| {
            listed.iter().all(|&(cap, derived)| match home(self, cap) {
                None => true,
                Some(remote) if remote.node != from => false,
                Some(_) if derived.is_some() => true,
                Some(remote) => {
                    let held = &self.caps[&cap];
                    remote.root
                        && held.parent.is_none()
                        && held.children.is_empty()
                        && held.record == Record::Empty
                }
            })
        });
        let needed = listed
            .iter()
            .flatten()
            .filter(|&&(cap, _)| home(self, cap).is_none())
            .count();
        let mut free = (0..GRANTS as u16)
            .filter(|g| !self.table.grants[self.node as usize].contains_key(g))
            .take(needed)
            .collect::<Vec<_>>()
            .into_iter();
        let grants = listed
            .filter(|_| allowed && free.len() == needed)
            .map(|listed| {
                listed
                    .into_iter()
                    .map(|(cap, derived)| match home(self, cap) {
                        Some(remote) => Crossing::Home {
                            grant: remote.grant,
                            serial: remote.serial,
                            derived: derived.map(|rights| (rights, self.caps[&cap].badge)),
                        },
                        None => {
                            let grant = free.next().expect("counted");
                            Crossing::Grant(self.make_grant((cap, derived), grant, from))
                        }
                    })
                    .collect()
            });
        self.send_packet(from, Body::TransferGrant { id, grants });
    }

    /// The gate writes a grant entry, and its anchor takes its place in the
    /// derivation list.
    fn make_grant(&mut self, listed: (CapId, Option<Rights>), grant: u16, grantee: u16) -> Granted {
        let space = self.kernel_space.expect("a kernel space");
        let place = Place {
            space,
            index: anchor_index(grant),
        };
        let (cap, derived) = listed;
        let anchor = match derived {
            Some(rights) => {
                self.derive(cap, rights, self.caps[&cap].badge, place);
                *self.caps[&cap].children.last().expect("the anchor")
            }
            None => {
                let from = self.caps[&cap].place;
                self.place(cap, place);
                self.moved_from.insert(grant, from);
                cap
            }
        };
        let serial = self.table.serials[self.node as usize]
            .get(&grant)
            .copied()
            .unwrap_or(0);
        let generation = self.table.generation(grantee);
        self.table.grants[self.node as usize].insert(
            grant,
            GrantEntry {
                grantee,
                generation,
                serial,
                state: GrantState::Live,
            },
        );
        let held = &self.caps[&anchor];
        let object = held.object;
        Granted {
            grant,
            serial,
            generation: self.table.generation(self.node),
            kind: self.objects[&object].kind,
            rights: held.rights,
            badge: held.badge,
            depth: self.depth(anchor),
            root: self.objects[&object].root == anchor,
            frame: self.frames.get(&object).copied(),
        }
    }

    /// The grants arrive: the receiver's range is judged again, the gate
    /// writes the use entries and the remote capabilities, and the
    /// receiver gets its message. Refused, the grants are released.
    fn place_transfer(&mut self, from: u16, id: u64, grants: Option<Vec<Crossing>>) {
        let Some(Request::Transfer {
            receiver,
            mut delivery,
            window,
            answer,
            reply,
        }) = self.requests.remove(&id)
        else {
            return;
        };
        let room = |model: &Self, n: usize| {
            let space = model
                .cspaces
                .get(&window.cspace)
                .filter(|_| !model.dying(window.cspace));
            n <= window.count as usize
                && (0..n).all(|i| {
                    space.and_then(|s| s.slots.get(window.first as usize + i)) == Some(&Slot::Empty)
                })
        };
        let root_depth = self.objects.get(&window.cspace).map(|o| self.depth(o.root));
        // What each capability would be in the range: kind, depth, and
        // whether it is the root of its object; `None` for a remote
        // capability going home whose grant is no longer live.
        let landing = |model: &Self, crossing: &Crossing| match *crossing {
            Crossing::Grant(g) => Some((g.kind, g.depth, g.root)),
            Crossing::Home {
                grant,
                serial,
                derived,
            } => {
                let entry = model.table.grants[model.node as usize].get(&grant)?;
                let live = entry.state == GrantState::Live
                    && entry.serial == serial
                    && entry.grantee == from;
                let anchor = model.anchor(grant).filter(|_| live)?;
                let object = model.caps[&anchor].object;
                let kind = model.objects[&object].kind;
                let depth = model.depth(anchor) + usize::from(derived.is_some());
                Some((
                    kind,
                    depth,
                    derived.is_none() && model.objects[&object].root == anchor,
                ))
            }
        };
        let fits = |landed: Option<(ObjectKind, usize, bool)>| match (landed, root_depth) {
            (Some((ObjectKind::Untyped, depth, _)), Some(d)) => depth >= d,
            (Some((ObjectKind::CSpace, depth, true)), Some(d)) => depth > d,
            (Some(_), Some(_)) => true,
            _ => false,
        };
        let uses_free = (0..GRANTS as u16)
            .filter(|u| !self.table.uses[self.node as usize].contains_key(u))
            .count();
        let new_grants = |grants: &[Crossing]| {
            grants
                .iter()
                .filter(|c| matches!(c, Crossing::Grant(_)))
                .count()
        };
        let accepted = grants.clone().filter(|grants| {
            room(self, grants.len())
                && grants.iter().all(|c| fits(landing(self, c)))
                && uses_free >= new_grants(grants)
        });
        let caps = match accepted {
            Some(grants) => {
                let mut returned = Vec::new();
                for (i, crossing) in grants.iter().enumerate() {
                    let place = Place {
                        space: window.cspace,
                        index: window.first + i as u32,
                    };
                    match *crossing {
                        Crossing::Grant(granted) => self.place_remote(from, granted, place),
                        Crossing::Home {
                            grant,
                            serial,
                            derived,
                        } => {
                            let anchor = self.anchor(grant).expect("judged");
                            match derived {
                                Some((rights, badge)) => self.derive(anchor, rights, badge, place),
                                None => {
                                    self.place(anchor, place);
                                    self.free_grant(grant);
                                    returned.push((grant, serial));
                                }
                            }
                        }
                    }
                }
                if !returned.is_empty() {
                    self.send_packet(from, Body::Returned { grants: returned });
                }
                grants.len()
            }
            None => {
                let grants: Vec<(u16, u32)> = grants
                    .into_iter()
                    .flatten()
                    .filter_map(|c| match c {
                        Crossing::Grant(g) => Some((g.grant, g.serial)),
                        Crossing::Home { .. } => None,
                    })
                    .collect();
                if !grants.is_empty() {
                    self.send_packet(from, Body::Refused { grants });
                }
                0
            }
        };
        let dropped = if caps == 0 {
            Header::FLAG_CAPS_DROPPED
        } else {
            0
        };
        let header = delivery.received.header;
        let flags = header.flags() | dropped;
        delivery.received.header =
            Header::new(header.label(), header.words(), caps, flags).expect("within the limits");
        if let Some(reply) = reply
            && let Some(crate::ReplyState::Armed { flags, .. }) = self.replies.get_mut(&reply)
        {
            *flags |= dropped;
        }
        if self
            .threads
            .get(&receiver)
            .is_some_and(|t| t.wait == Wait::Node(id))
        {
            self.unlink(receiver);
            self.wake(receiver, Ok(Output::Message(delivery)));
        }
        if let Some((peer, request)) = answer {
            let proxy = self
                .proxies
                .iter()
                .find(|(_, h)| **h == Some((peer, request)))
                .map(|(&p, _)| p);
            if let Some(proxy) = proxy {
                self.answer_proxy(proxy, Ok(Output::Sent { flags: dropped }));
            }
        }
    }

    /// A remote capability in a slot: a local object that names the grant,
    /// and its root capability with the anchor's kind, rights and depth.
    fn place_remote(&mut self, granter: u16, granted: Granted, place: Place) {
        let use_entry = (0..GRANTS as u16)
            .find(|u| !self.table.uses[self.node as usize].contains_key(u))
            .expect("checked");
        self.table.uses[self.node as usize].insert(
            use_entry,
            UseEntry {
                granter,
                grant: granted.grant,
                serial: granted.serial,
                generation: granted.generation,
                leaves: 0,
            },
        );
        let id = ObjId(self.next_id);
        self.next_id += 1;
        let cap = CapId(self.next_id);
        self.next_id += 1;
        self.caps.insert(
            cap,
            Cap {
                object: id,
                rights: granted.rights,
                badge: granted.badge,
                parent: None,
                children: Vec::new(),
                place,
                dying: false,
                record: Record::Empty,
            },
        );
        self.objects.insert(
            id,
            Object {
                kind: granted.kind,
                root: cap,
                from: None,
                boot: false,
            },
        );
        *self.slot_mut(place) = Slot::Full(cap);
        if let Some(frame) = granted.frame {
            self.frames.insert(id, frame);
        }
        let remote = Remote {
            node: granter,
            grant: granted.grant,
            serial: granted.serial,
            use_entry,
            depth: granted.depth,
            root: granted.root,
        };
        self.remotes.insert(id, remote);
    }

    /// Release by the holder: when its root remote capability of a grant
    /// is removed, the holder frees its use entry and tells the granting
    /// node, which frees the entry if the anchor has no descendant.
    pub(crate) fn remote_destroyed(&mut self, object: ObjId) {
        let Some(remote) = self.remotes.remove(&object) else {
            return;
        };
        self.table.uses[self.node as usize].remove(&remote.use_entry);
        let (grant, serial) = (remote.grant, remote.serial);
        self.send_packet(remote.node, Body::Release { grant, serial });
    }

    /// The granting node at a release: the entry is freed if its anchor has
    /// no descendant; otherwise the anchor stays, held by nobody.
    fn released(&mut self, grant: u16, serial: u32) {
        let entries = &self.table.grants[self.node as usize];
        if !entries.get(&grant).is_some_and(|e| e.serial == serial) {
            return;
        }
        let Some(anchor) = self.anchor(grant) else {
            return;
        };
        if self.caps[&anchor].children.is_empty() {
            self.free_grant(grant);
            self.remove_leaf(anchor);
        }
    }

    /// A transfer refused at delivery: each grant is freed; a derived
    /// anchor is removed, and a moved capability goes back to its slot.
    fn refused(&mut self, grants: Vec<(u16, u32)>) {
        for (grant, serial) in grants {
            let entries = &self.table.grants[self.node as usize];
            if !entries.get(&grant).is_some_and(|e| e.serial == serial) {
                continue;
            }
            let Some(anchor) = self.anchor(grant) else {
                continue;
            };
            let moved_from = self.moved_from.remove(&grant);
            self.free_grant(grant);
            match moved_from {
                Some(from)
                    if self
                        .cspaces
                        .get(&from.space)
                        .and_then(|s| s.slots.get(from.index as usize))
                        == Some(&Slot::Empty) =>
                {
                    self.place(anchor, from)
                }
                Some(_) => {}
                None => self.remove_leaf(anchor),
            }
        }
    }

    /// The granting node took back the anchors of these grants: the holder
    /// removes its root remote capability of each, if it is still a leaf
    /// with no mapping. Its release names a freed grant, which is ignored.
    fn returned(&mut self, from: u16, grants: Vec<(u16, u32)>) {
        for (grant, serial) in grants {
            let root = self
                .remotes
                .iter()
                .find(|(_, r)| r.node == from && r.grant == grant && r.serial == serial)
                .map(|(&object, _)| self.objects[&object].root);
            if let Some(root) = root {
                let held = &self.caps[&root];
                if held.children.is_empty() && held.record == Record::Empty {
                    self.remove_leaf(root);
                }
            }
        }
    }

    /// The anchor of a grant, if its slot holds one.
    pub(crate) fn anchor(&self, grant: u16) -> Option<CapId> {
        let space = self.kernel_space?;
        match self.cspaces[&space].slots.get(anchor_index(grant) as usize) {
            Some(&Slot::Full(cap)) => Some(cap),
            _ => None,
        }
    }

    /// The grant whose anchor `cap` is, if it is one.
    pub(crate) fn grant_of(&self, cap: CapId) -> Option<u16> {
        let place = self.caps[&cap].place;
        let index = place.index.checked_sub(1)?;
        (Some(place.space) == self.kernel_space && (index as usize) < GRANTS)
            .then_some(index as u16)
    }

    /// The gate frees a grant entry; its serial grows.
    pub(crate) fn free_grant(&mut self, grant: u16) {
        let node = self.node as usize;
        self.moved_from.remove(&grant);
        if let Some(entry) = self.table.grants[node].remove(&grant) {
            self.table.serials[node].insert(grant, (entry.serial + 1) & 0x3FFF_FFFF);
        }
    }

    /// Revocation, one unit: when a removal reaches an anchor, the grant
    /// becomes revoking; with no mapping under it on the holder, the entry
    /// is freed at once; otherwise the holder is asked to remove its root
    /// remote capability, and the calling thread waits. Returns whether
    /// the anchor may be removed now.
    pub(crate) fn revoke_anchor(
        &mut self,
        anchor: CapId,
        caller: Option<ObjId>,
    ) -> Result<bool, Outcome> {
        let Some(grant) = self.grant_of(anchor) else {
            return Ok(true);
        };
        let node = self.node as usize;
        let Some(entry) = self.table.grants[node].get_mut(&grant) else {
            return Ok(true);
        };
        entry.state = GrantState::Revoking;
        let (grantee, serial) = (entry.grantee, entry.serial);
        let mapped = self.table.uses[grantee as usize].values().any(|u| {
            u.granter == self.node && u.grant == grant && u.serial == serial && u.leaves > 0
        });
        let gone = self.table.member(grantee).state() == MemberState::Dead
            || !self.table.uses[grantee as usize]
                .values()
                .any(|u| u.granter == self.node && u.grant == grant && u.serial == serial);
        if !mapped || gone {
            self.free_grant(grant);
            return Ok(true);
        }
        let Some(thread) = caller else {
            return Ok(false);
        };
        let id = self.new_request();
        self.requests.insert(id, Request::Revoke { thread, grant });
        self.send_packet(grantee, Body::Revoke { id, grant, serial });
        live(&mut self.threads, thread).wait = Wait::Node(id);
        self.observe();
        Err(Ok(Output::Waits))
    }

    /// The holder removes its root remote capability of a revoking grant,
    /// one step of at most 32 units, and answers.
    fn revoke_step(&mut self, from: u16, id: u64, grant: u16, serial: u32) {
        let root = self
            .remotes
            .iter()
            .find(|(_, r)| r.node == from && r.grant == grant && r.serial == serial)
            .map(|(&object, _)| self.objects[&object].root);
        let done = match root {
            None => true,
            Some(root) => {
                live(&mut self.caps, root).dying = true;
                self.remove_root(root) == Ok(Output::Done)
            }
        };
        self.send_packet(from, Body::RevokeStep { id, done });
    }

    /// The granting node at each answer of the holder: the removal step
    /// returns `again`; once the root is removed, the entry is freed.
    fn revoked_step(&mut self, id: u64, done: bool) {
        let Some(Request::Revoke { thread, grant }) = self.requests.remove(&id) else {
            return;
        };
        if done {
            self.free_grant(grant);
        }
        if self
            .threads
            .get(&thread)
            .is_some_and(|t| t.wait == Wait::Node(id))
        {
            self.unlink(thread);
            live(&mut self.threads, thread).answered = true;
            self.wake(thread, Err(Error::Again));
        }
    }

    /// The kernel space of a node: the anchors of its grants, the
    /// capability for its mailbox, and its proxies for every other node,
    /// made at start-up from no untyped.
    pub(crate) fn build_kernel_space(&mut self, mailbox: ObjId) {
        let nodes = self.table.nodes;
        let slots = proxy_index(nodes, 0) as u64 + 1;
        let slots = slots.next_multiple_of(u64::from(dodos_abi::CSPACE_SEGMENT_SLOTS));
        let space = ObjId(self.next_id);
        let root_place = Place {
            space,
            index: slots as u32 - 1,
        };
        let all = Rights::ALL.without(Rights::EXEC);
        self.create(ObjectKind::CSpace, slots, None, all, root_place);
        self.kernel_space = Some(space);
        let mailbox_root = self.objects[&mailbox].root;
        let place = Place {
            space,
            index: mailbox_index(),
        };
        self.derive(mailbox_root, Rights::SEND.union(Rights::GRANT), 0, place);
        let own = self.node;
        for peer in (0..nodes).filter(|&peer| peer != own) {
            for i in 0..PROXIES_PER_PEER {
                let place = Place {
                    space,
                    index: proxy_index(peer, i),
                };
                let cap = self.create(ObjectKind::Thread, 0, None, all, place);
                let proxy = self.caps[&cap].object;
                self.bind_space(proxy, Some(space));
                self.proxies.insert(proxy, None);
            }
        }
    }

    /// A slot of this node, to change it.
    fn slot_mut(&mut self, Place { space, index }: Place) -> &mut Slot {
        &mut live(&mut self.cspaces, space).slots[index as usize]
    }
}

/// `send`, and `grant` if the message lists capabilities.
fn message_rights(message: &crate::Message) -> Rights {
    match message.caps.is_empty() {
        true => Rights::SEND,
        false => Rights::SEND.union(Rights::GRANT),
    }
}
