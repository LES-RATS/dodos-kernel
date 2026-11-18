//! The invariants of a node of several cores (A node of several cores; TLB,
//! one core and several; Locking), each named after the sentence of the
//! contract it checks.

use super::{Check, ensure};
use crate::cores::{Completes, Flush};
use crate::locks::{LockId, Step};
use crate::{Model, Record, Wait};

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
        for &(role, word, op) in self.submitted.iter() {
            ensure(
                word.state() == dodos_abi::RoleState::Held && word.holder() == self.node,
                "SERVED: a unit operation submitted by a node the role word does not name as holder",
            )?;
            // CONTRACT-GAP 17 (pass 25): R1 is not a request, and the
            // contract does not say whether the node that hands a unit
            // role runs it during steps 1 to 3 for a node accused then
            // (Roles: R1 at every change of holder names the target at
            // step 5 and a take-over only). Its whole-unit invalidation is
            // not judged here.
            let r1 = matches!(op, crate::rings::UnitOp::Invalidate { domain: None });
            ensure(
                r1 || !self.handing.contains_key(&role),
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
                                Flush::Kernel(_) | Flush::Nothing => false,
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
            // The state word reads *in use* exactly while the request is.
            let in_use = core.request_state == crate::cores::RequestState::InUse;
            ensure(
                in_use == core.request.is_some(),
                "TLB: the state word of a request",
            )?;
            let Some(request) = &core.request else {
                continue;
            };
            // On ARM64 the request serves only the barrier before a
            // watermark returns, which invalidates nothing (D-138).
            let barrier = matches!(request.completes, Completes::Barrier(_));
            ensure(
                barrier == (request.flush == Flush::Nothing),
                "TLB: a barrier that invalidates, or an invalidation that is a barrier",
            )?;
            ensure(
                self.width() > 1 && (self.machine.arch != crate::Arch::Aarch64 || barrier),
                "TLB: a request on a node of one core or on ARM64",
            )?;
            if let Completes::Barrier(epoch) = request.completes {
                ensure(
                    epoch <= self.kernel_epoch,
                    "epoch: a barrier beyond the kernel epoch",
                )?;
            }
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
            // A cancelled wait forgets its waiter in the same step
            // (`forget_waiter`), so a waiter named is a live thread that
            // waits for this request (ver-D D10).
            let waits = request.waiter.is_none_or(|thread| {
                self.threads
                    .get(&thread)
                    .is_some_and(|t| t.wait == Wait::Cores(requester))
            });
            ensure(waits, "TLB: the thread of a request waits for another")?;
            let panicked = self.table.beacons[self.node as usize].panic.is_some();
            ensure(
                panicked
                    || self.membership_now() < request.started + self.table.core_answer_ns
                    || core.silent,
                "CORE_ANSWER_NS: a request older than the bound on a node that did not accuse itself",
            )?;
        }
        for (position, core) in self.cores.iter().enumerate() {
            for &requester in &core.pending {
                let asks = self
                    .cores
                    .get(requester)
                    .and_then(|r| r.request.as_ref())
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
    /// cost of several cores on a node of one core; every lock is the node
    /// lock, held from the entry of the event to its exit, or the gate lock
    /// inside it (D-138); "no lock is held across a wait for another core
    /// or for a unit": no event spins inside its hold.
    fn check_locks(&self) -> Check {
        let steps = &self.locks.steps;
        if self.width() == 1 {
            let paid = steps
                .iter()
                .any(|s| matches!(s, Step::Pay(_) | Step::Lock(_) | Step::Unlock(_)));
            ensure(
                !paid,
                "N = 1: a node of one core paid for a rule of several cores",
            )?;
        }
        ensure(
            !steps.iter().any(|s| matches!(s, Step::Spin(_))),
            "no lock is held across a wait for another core or for a unit",
        )?;
        self.check_epoch()?;
        // Each lock is released only while held, and the gate lock before
        // the node lock (the final review's E3: an unmatched unlock and a
        // gate lock never released passed).
        let (mut node, mut gate) = (false, false);
        for step in steps {
            match step {
                Step::Lock(LockId::Node) => {
                    ensure(!node, "the node lock taken twice in one event")?;
                    node = true;
                }
                Step::Unlock(LockId::Node) => {
                    ensure(
                        node && !gate,
                        "the node lock released while not held or under the gate lock",
                    )?;
                    node = false;
                }
                Step::Lock(LockId::Gate) => {
                    ensure(node, "the gate lock taken outside the node lock")?;
                    ensure(!gate, "the gate lock taken twice")?;
                    gate = true;
                }
                Step::Unlock(LockId::Gate) => {
                    ensure(gate, "the gate lock released while not held")?;
                    gate = false;
                }
                _ => {}
            }
        }
        ensure(!node, "the node lock held past the exit of an event")
    }

    /// The kernel epoch (A node of several cores, D-135, D-138): it rises
    /// by one at the last destruction of an untyped's object, followed by
    /// the watermark ready to return at that epoch, and nowhere else; the
    /// last raise of the event is the epoch; word 6 of an untyped's root
    /// slot and the barrier epoch never pass it.
    fn check_epoch(&self) -> Check {
        let steps = &self.locks.steps;
        let mut last: Option<u64> = None;
        for (index, step) in steps.iter().enumerate() {
            let Step::Epoch(epoch) = *step else {
                continue;
            };
            ensure(
                last.is_none_or(|l| epoch == l + 1),
                "epoch: raised by more than one at a time",
            )?;
            last = Some(epoch);
            ensure(
                matches!(steps.get(index + 1), Some(Step::Ready(_))),
                "epoch: raised other than at the last destruction of an untyped",
            )?;
        }
        ensure(
            last.is_none_or(|l| l == self.kernel_epoch),
            "epoch: the node's epoch is not its last raise",
        )?;
        ensure(
            self.barrier_epoch <= self.kernel_epoch,
            "epoch: the barrier epoch beyond the kernel epoch",
        )?;
        for untyped in self.untypeds.values() {
            ensure(
                untyped
                    .ready
                    .is_none_or(|e| e >= 1 && e <= self.kernel_epoch && untyped.live == 0),
                "epoch: word 6 of an untyped's root slot is not the epoch its watermark became ready at",
            )?;
        }
        Ok(())
    }

    /// What crosses between nodes, credits: "a credit is in use from the
    /// moment its message is written into the ring until the sender has
    /// consumed its answer, or until the peer's generation ends"; a node
    /// has at most `SENDS_PER_PEER` sends in flight to a peer, each in a
    /// send credit and no request in one; a thread
    /// waiting for another node names a credit in use toward it.
    fn check_credits(&self) -> Check {
        for (peer, credits) in self.peers.iter().enumerate() {
            ensure(
                credits.credits.len() == dodos_abi::REQUESTS_PER_PEER + dodos_abi::SENDS_PER_PEER,
                "credits: the credits of a peer",
            )?;
            // At most `SENDS_PER_PEER` sends in flight follows from the
            // length above once a send sits only in a send credit (16 to
            // 23) and a request never does: the count itself could not fail
            // (ver-D D4), the place can.
            let placed = credits.credits.iter().enumerate().all(|(index, credit)| {
                credit.as_ref().is_none_or(|c| {
                    matches!(c.completes, crate::rings::Completes::Send { .. })
                        == (index >= dodos_abi::REQUESTS_PER_PEER)
                })
            });
            ensure(placed, "credits: a send outside the send credits")?;
            if peer == self.node as usize {
                ensure(
                    credits.credits.iter().all(Option::is_none),
                    "credits: a credit toward the node itself",
                )?;
            }
        }
        for thread in self.threads.values() {
            if let Wait::Node { peer, credit } = thread.wait {
                let in_use = self
                    .peers
                    .get(peer as usize)
                    .and_then(|p| p.credits.get(credit as usize))
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
