//! The invariants of a node of several cores (A node of several cores; TLB,
//! one core and several; Locking), each named after the sentence of the
//! contract it checks.

use super::{Check, ensure};
use crate::cores::{Completes, Flush};
use crate::locks::{Cost, LockId, Step};
use crate::{Locking, Model, Record, Wait};

impl Model {
    pub(super) fn check_node(&self) -> Check {
        self.check_tlb()?;
        self.check_requests()?;
        self.check_marks()?;
        self.check_locks()?;
        self.check_credits()?;
        self.check_crossing()?;
        self.check_window()
    }

    /// The calls, `node_start`: "The cores a node starts on lie in one
    /// window of 64 core numbers" (gap 7), so a context's cores set, a
    /// window and a mask, holds every core of its node.
    fn check_window(&self) -> Check {
        let window = self.cores[0].number / 64;
        ensure(
            self.cores.iter().all(|c| c.number / 64 == window),
            "WINDOW: a node whose cores lie in two windows of 64 core numbers",
        )
    }

    /// Roles: "the holder serves a request only if the role word reads
    /// held by its own node with that epoch and it has not begun a
    /// hand-over" (SERVED); What crosses between nodes: no message acts on
    /// a later generation of its receiver (RECEIVER), and none of a
    /// generation that is over is applied, a signal included (SENDER).
    fn check_crossing(&self) -> Check {
        for &(role, word) in self.submitted.iter() {
            ensure(
                word.state() == dodos_abi::RoleState::Held && word.holder() == self.node,
                "SERVED: a unit operation submitted by a node the role word does not name as holder",
            )?;
            ensure(
                !self.handing.contains_key(&role),
                "SERVED: a unit operation submitted after step 1 of a hand-over",
            )?;
        }
        for (header, sender) in self.applied.iter() {
            ensure(
                header.receiver_generation == self.table.generation(self.node),
                "RECEIVER: a message acted on a generation of its receiver it was not sent to",
            )?;
            // The sender's member word as the receiver read it when it
            // applied the message.
            let current = sender.generation() == header.sender_generation
                && matches!(
                    sender.state(),
                    dodos_abi::MemberState::Starting
                        | dodos_abi::MemberState::Live
                        | dodos_abi::MemberState::Sealing
                        | dodos_abi::MemberState::Sealed
                );
            ensure(
                current,
                "SENDER: a message of a generation that is over was applied",
            )?;
        }
        Ok(())
    }

    /// TLB: "when a call returns success, no thread on any core of the
    /// node can use a translation that the call removed or reduced": a
    /// translation a core may still hold that its space no longer holds, or
    /// holds with fewer rights, is allowed only while a request not yet
    /// complete asks that core for it, or on an idle core whose owes bit is
    /// set ("a translation a core kept from before it went idle is never
    /// used"), or under the tag of a space being destroyed until the request
    /// of its first step completes; a core that runs on the root of a space
    /// holds no such translation of it outside a request asking it. The
    /// kernel's own space: a run no longer mapped there may be held by a
    /// core only while a request asks that core for it.
    fn check_tlb(&self) -> Check {
        for (position, core) in self.cores.iter().enumerate() {
            let asked = |space: crate::ObjId, address: Option<u64>| {
                self.cores
                    .iter()
                    .filter_map(|c| c.request.as_ref())
                    .any(|r| {
                        r.asked & 1 << position != 0
                            && r.acked & 1 << position == 0
                            && match &r.flush {
                                Flush::Tag(s) => *s == space,
                                Flush::Range {
                                    space: s,
                                    addresses,
                                } => *s == space && address.is_none_or(|a| addresses.contains(&a)),
                                Flush::Kernel(_) => false,
                            }
                    })
            };
            for (&space, cached) in &core.tlb {
                for (&address, &(cap, page, rights)) in cached {
                    let now = self
                        .aspaces
                        .get(&space)
                        .and_then(|a| a.translations.get(&address))
                        .is_some_and(|t| {
                            t.cap == cap && t.page == page && t.rights.contains(rights)
                        });
                    if now {
                        continue;
                    }
                    let usable = core.loaded == Some(space) && !core.silent;
                    let owed = !usable && core.idle.mark && core.idle.owes;
                    ensure(
                        asked(space, Some(address)) || owed || core.silent,
                        "TLB: a core can use a translation a completed change removed",
                    )?;
                }
            }
            for chunk in &core.kernel_tlb {
                if self.kernel_runs.contains(chunk) {
                    continue;
                }
                let asked = self
                    .cores
                    .iter()
                    .filter_map(|c| c.request.as_ref())
                    .any(|r| {
                        r.asked & 1 << position != 0
                            && r.acked & 1 << position == 0
                            && matches!(&r.flush, Flush::Kernel(chunks) if chunks.contains(chunk))
                    });
                let mine = self.table.owners.get(*chunk as usize).is_some_and(|&raw| {
                    dodos_abi::Ownership::from_raw(raw).is_some_and(|w| {
                        w.state() == dodos_abi::OwnState::Taken
                            && w.owner() == dodos_abi::Owner::Node(self.node)
                    })
                });
                ensure(
                    asked || core.silent || mine,
                    "G5: a run given or destroyed while a core may still translate it in the kernel's own space",
                )?;
            }
        }
        Ok(())
    }

    /// TLB, the request of a core: a node of one core has none; a request
    /// asks only other cores of the node and only while not complete; each
    /// position in a core's pending set names a request that asks it and
    /// that it has not acknowledged; a record it completes is *unmapping*;
    /// its waiting thread waits for it; "a core that does not acknowledge a
    /// request within `CORE_ANSWER_NS` of its start is a failure of the
    /// node", so no request older than that remains on a node that has not
    /// accused itself.
    fn check_requests(&self) -> Check {
        for (requester, core) in self.cores.iter().enumerate() {
            let Some(request) = &core.request else {
                continue;
            };
            ensure(
                self.width() > 1 && self.machine.arch != crate::Arch::Aarch64,
                "TLB: a request on a node of one core or on ARM64",
            )?;
            ensure(
                request.asked & 1 << requester == 0 && request.acked & !request.asked == 0,
                "TLB: a request asks its own core",
            )?;
            ensure(
                request.acked != request.asked,
                "TLB: a complete request kept",
            )?;
            if let Completes::Record(cap) = request.completes {
                let unmapping = self
                    .caps
                    .get(&cap)
                    .is_some_and(|c| matches!(c.record, Record::Unmapping(_)));
                ensure(unmapping, "TLB: the record of a request is not unmapping")?;
            }
            if let Some(thread) = request.waiter
                && let Some(t) = self.threads.get(&thread)
            {
                let waits = t.wait == Wait::Cores(requester) || t.wait == Wait::None;
                ensure(waits, "TLB: the thread of a request waits for another")?;
            }
            let panicked = self.table.beacons[self.node as usize].panic.is_some();
            ensure(
                panicked || self.now < request.started + self.table.core_answer_ns || core.silent,
                "CORE_ANSWER_NS: a request older than the bound on a node that did not accuse itself",
            )?;
        }
        for (position, core) in self.cores.iter().enumerate() {
            for &requester in &core.pending {
                let asks = self.cores[requester]
                    .request
                    .as_ref()
                    .is_some_and(|r| r.asked & 1 << position != 0 && r.acked & 1 << position == 0);
                ensure(
                    asks,
                    "TLB: a pending bit names no request that asks the core",
                )?;
            }
            if self.width() == 1 {
                ensure(
                    core.idle == crate::cores::IdleWord::default() && core.pending.is_empty(),
                    "TLB: a node of one core has no idle mark, owes bit or pending set",
                )?;
            }
        }
        for thread in self.threads.values() {
            if let Wait::Cores(requester) = thread.wait {
                ensure(
                    self.cores
                        .get(requester)
                        .is_some_and(|c| c.request.is_some()),
                    "TLB: a thread waits for a request that is not in flight",
                )?;
            }
        }
        Ok(())
    }

    /// A thread that runs on another core of its node: "while the mark is
    /// set, no core runs the thread" except the core it is leaving, which
    /// has the leave interrupt pending; a marked thread is not runnable.
    fn check_marks(&self) -> Check {
        for (&id, thread) in &self.threads {
            if !thread.leaving {
                continue;
            }
            let core = self.cores.iter().find(|c| c.thread == Some(id));
            let leaving = core.is_some_and(|c| c.ipis.contains(&crate::Ipi::Leave) || c.silent);
            ensure(
                leaving,
                "leaving its core: a marked thread not on a core sent the leave interrupt",
            )?;
            ensure(
                self.width() > 1,
                "leaving its core: reached on a node of one core",
            )?;
        }
        Ok(())
    }

    /// Locking: "a kernel built for N = 1 ... no lock instruction", and no
    /// cost of several cores on a node of one core; under L1 every lock is
    /// the node lock, held from the entry of the event to its exit, or the
    /// gate lock inside it; under L2 the locks are taken in the order of
    /// their kind, the gate lock last, at most k per call; under both "no
    /// lock is held across a wait for another core or for a unit": no event
    /// spins inside its hold.
    fn check_locks(&self) -> Check {
        let steps = &self.locks.steps;
        if self.width() == 1 {
            let paid = steps.iter().any(|s| {
                matches!(
                    s,
                    Step::Pay(_) | Step::Lock(_) | Step::Try(_) | Step::Unlock(_)
                )
            });
            ensure(
                !paid,
                "N = 1: a node of one core paid for a rule of several cores",
            )?;
        }
        ensure(
            !steps.iter().any(|s| matches!(s, Step::Spin(_))),
            "no lock is held across a wait for another core or for a unit",
        )?;
        let locks: Vec<LockId> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Lock(l) | Step::Try(l) => Some(*l),
                _ => None,
            })
            .collect();
        match self.machine.locking {
            Locking::L1 => {
                let mut node = false;
                for step in steps {
                    match step {
                        Step::Lock(LockId::Node) => node = true,
                        Step::Unlock(LockId::Node) => node = false,
                        Step::Lock(LockId::Gate) => {
                            ensure(node, "L1: the gate lock taken outside the node lock")?
                        }
                        Step::Lock(_) | Step::Try(_) => {
                            return Err("L1: a lock other than the node lock and the gate lock");
                        }
                        _ => {}
                    }
                }
                ensure(!node, "L1: the node lock held past the exit of an event")?;
            }
            Locking::L2 => {
                // "A core waits for a lock only in the order by kind": a
                // wait is for a lock after every lock held; a lock that
                // comes before one held is tried (D-134). Judged in the
                // order the event took them, never sorted.
                let mut held: Vec<LockId> = Vec::new();
                for step in steps {
                    match *step {
                        Step::Lock(lock) => {
                            ensure(
                                held.iter().all(|&h| h < lock),
                                "L2: a lock waited for out of the order fixed by kind",
                            )?;
                            held.push(lock);
                        }
                        Step::Try(lock) => {
                            ensure(
                                held.iter().any(|&h| lock < h) && !held.contains(&lock),
                                "L2: a lock tried that is not before a lock held",
                            )?;
                            held.push(lock);
                        }
                        Step::Unlock(lock) => held.retain(|&h| h != lock),
                        _ => {}
                    }
                }
                if let Some(&last) = locks.last() {
                    ensure(
                        !locks.contains(&LockId::Gate) || last == LockId::Gate,
                        "L2: the gate lock is not the last of the order",
                    )?;
                }
                if let Some(bound) = self.locks.bound {
                    ensure(locks.len() <= bound, "L2: more than k locks in one call")?;
                }
                let paid = steps
                    .iter()
                    .filter(|s| matches!(s, Step::Pay(Cost::Lock)))
                    .count();
                ensure(
                    paid == locks.len(),
                    "L2: a lock taken without its instruction",
                )?;
            }
        }
        Ok(())
    }

    /// What crosses between nodes, credits: "a credit is in use from the
    /// moment its message is written into the ring until the sender has
    /// consumed its answer, or until the peer's generation ends"; a node
    /// has at most `SENDS_PER_PEER` sends in flight to a peer; a thread
    /// waiting for another node names a credit in use toward it.
    fn check_credits(&self) -> Check {
        for (peer, credits) in self.peers.iter().enumerate() {
            ensure(
                credits.credits.len() == dodos_abi::REQUESTS_PER_PEER + dodos_abi::SENDS_PER_PEER,
                "credits: the credits of a peer",
            )?;
            let (sends, _) = credits.sends();
            ensure(
                sends <= dodos_abi::SENDS_PER_PEER,
                "credits: more sends than SENDS_PER_PEER",
            )?;
            if peer == self.node as usize {
                ensure(
                    credits.credits.iter().all(Option::is_none),
                    "credits: a credit toward the node itself",
                )?;
            }
        }
        for thread in self.threads.values() {
            if let Wait::Node { peer, credit } = thread.wait {
                let in_use = self.peers[peer as usize]
                    .credits
                    .get(credit as usize)
                    .is_some_and(Option::is_some);
                ensure(
                    in_use,
                    "credits: a thread waits for an answer no credit expects",
                )?;
            }
        }
        Ok(())
    }
}
