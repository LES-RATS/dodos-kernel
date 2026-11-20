//! A node of several cores (D-128, D-129, D-131): the part of each physical
//! core of the node, the kernel's interrupts between the cores of a node
//! (reschedule, leave, invalidation), the invalidation request of each core
//! with its pending set and its idle word, and the translations each core
//! may still hold (TLB, one core and several).
//!
//! A node of one core takes none of these paths: no interrupt, no request,
//! no idle word, no set of cores in a tag's header; every event that would
//! pay for one on such a node is refused by the checks of `locks.rs`.
//!
//! The translations a core may hold are modelled as a cache per tag (the
//! address space whose root it loaded): at the end of every event of a core
//! that runs on the root of an address space, every translation of that
//! space enters the cache, with the rights it has; a local invalidation
//! takes entries out. A cached entry that its space no longer holds, or
//! holds with fewer rights, is *stale*; the checks allow it only where the
//! contract does (`Model::check_cores`).

use crate::locks::Cost;
use crate::{CapId, Ephemeral, Model, ObjId, Output, Wait, live};
use dodos_abi::{Error, Rights};
use std::collections::{BTreeMap, BTreeSet};

/// The kernel's interrupts to a core of its own node (G10: ordinary
/// vectors), and the one interrupt of a ring that another node sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ipi {
    /// A context placed on the core became eligible and should take it.
    Reschedule,
    /// A thread the core runs is marked *leaving its core*.
    Leave,
    /// A request of another core of the node is pending on it.
    Invalidate,
    /// A user ring into the node went from empty to not empty, or entries
    /// remain after a step of 16 (What crosses between nodes).
    Ring,
}

impl Ipi {
    /// x86-64: the vector of the interrupt, one of the ordinary
    /// inter-kernel vectors `IKV_FIRST..=IKV_LAST`, which are the only
    /// ones the gate composes an ICR for (G10, D-155).
    pub fn vector(self) -> u8 {
        let index = match self {
            Ipi::Reschedule => 0,
            Ipi::Leave => 1,
            Ipi::Invalidate => 2,
            Ipi::Ring => 3,
        };
        dodos_abi::x86_64::IKV_FIRST + index
    }
}

/// The idle word of a core: the idle mark and the owes bit, changed only by
/// compare-and-swap (D-131).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IdleWord {
    pub(crate) mark: bool,
    pub(crate) owes: bool,
}

/// What a request invalidates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Flush {
    /// The translations of these addresses under the tag of a space.
    Range { space: ObjId, addresses: Vec<u64> },
    /// Every translation under the tag of a space.
    Tag(ObjId),
    /// The translations of these chunks in the kernel's own space.
    Kernel(Vec<u64>),
    /// Nothing: the barrier before a watermark returns (D-138).
    Nothing,
}

/// What the completion of a request completes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Completes {
    /// The record of one frame capability, *unmapping* meanwhile: its
    /// counts are lowered and it is emptied.
    Record(CapId),
    /// A protect that reduced rights.
    Protect,
    /// The first step of the destruction of an aspace.
    Destroy(ObjId),
    /// The unmapping of a run from the kernel's own space: the words of the
    /// run are written at the completion (`memory_give`, or the destruction
    /// of a block).
    Run(crate::blocks::FreedRun),
    /// A barrier: the barrier epoch becomes at least this kernel epoch.
    Barrier(u64),
}

/// The state word of a core's invalidation request: *in use* written by
/// its starting core, *done* written by the core whose acknowledgement
/// completes it, by compare-and-swap; a core starts a new request only
/// once its previous one reads *done* (A node of several cores, D-135).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RequestState {
    /// Never used since the node started.
    #[default]
    None,
    InUse,
    Done,
}

/// The invalidation request of a core (TLB, x86-64 and RISC-V): only that
/// core starts it, for one change at a time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Request {
    pub(crate) flush: Flush,
    /// The positions asked and those that have acknowledged.
    pub(crate) asked: u64,
    pub(crate) acked: u64,
    pub(crate) started: u64,
    pub(crate) completes: Completes,
    /// The thread that waits for it, in *waiting for its node's cores*.
    pub(crate) waiter: Option<ObjId>,
}

/// The translations a core may hold of one space: by the address of the
/// page, the capability and page whose record made it, and the rights it
/// was cached with.
pub(crate) type Cached = BTreeMap<u64, (CapId, u64, Rights)>;

/// The part of one physical core in the node block, and what is kept apart
/// for it (its debug area is in the crash area of the machine table).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CorePart {
    /// Its core number in the machine's core list.
    pub(crate) number: u32,
    /// What runs next: the context it runs, whether from the ready queue,
    /// and the thread it runs on it (the holder at the choice).
    pub(crate) running: Option<(ObjId, bool)>,
    pub(crate) thread: Option<ObjId>,
    /// Clock: the instant of its previous event.
    pub(crate) previous: u64,
    /// Admission: the sum of the bandwidths admitted on it.
    pub(crate) admitted: u64,
    /// Idle: its idle table, and its performance request.
    pub(crate) idle_table: Vec<[u64; 4]>,
    pub(crate) perf: Option<(u64, u64, u64)>,
    pub(crate) idle: IdleWord,
    /// Its invalidation request, and the positions whose request is pending
    /// on it.
    pub(crate) request: Option<Request>,
    /// The state word of its request.
    pub(crate) request_state: RequestState,
    pub(crate) pending: BTreeSet<usize>,
    /// The interrupts sent to it and not yet taken.
    pub(crate) ipis: BTreeSet<Ipi>,
    /// The root it has loaded: an address space, or the kernel's own.
    pub(crate) loaded: Option<ObjId>,
    /// The translations it may hold: of each space by its tag, and of the
    /// kernel's own space by chunk.
    pub(crate) tlb: BTreeMap<ObjId, Cached>,
    pub(crate) kernel_tlb: BTreeSet<u64>,
    /// A core that stopped answering (a test's input): it takes no event.
    pub(crate) silent: bool,
    /// The heartbeat of this core as the node's other cores last saw it
    /// move, and the instant they saw it (A node of several cores).
    pub(crate) watched: Ephemeral<(u64, u64)>,
    /// The count of additions and the root at its last caching.
    pub(crate) cached: Ephemeral<(u64, Option<ObjId>)>,
    /// Idle, slack and kernel time, published in its core beacon.
    pub(crate) idle_ns: Ephemeral<u64>,
    pub(crate) slack_ns: Ephemeral<u64>,
    /// What a core enables for a thread, as the last switch wrote it.
    pub(crate) enabled: u32,
    /// Debugging: the rate limit of each code of its kernel ring, the
    /// entries it suppressed and reported, and its random generator, whose
    /// state decides every later read and is compared like the rest (the
    /// final review's F2 M8: as bookkeeping, a boot that seeded it
    /// otherwise compared equal).
    pub(crate) limits: [crate::debug::Limit; 18],
    /// x86-64: the causes DR6 holds above its reset value, set by the
    /// processor at a #DB and never cleared by it for BD, BS and BT; the
    /// handler loads the reset value, so it reads 0 after every event
    /// (Debugging, D-143).
    pub(crate) dr6: u64,
    /// While the core sleeps, the exit latency of its state (`System`
    /// writes it): its timer is taken at its wake, up to that latency late.
    pub(crate) asleep: u64,
    pub(crate) reported: u64,
    pub(crate) entropy: u64,
}

/// What `start_request` did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Started {
    /// The invalidation is complete: on one core, on ARM64 (the broadcast),
    /// or when no other core had to be asked.
    Complete,
    /// The request is written and the cores asked; its completion comes at
    /// the last acknowledgement.
    Asked,
    /// The request of the calling core is in use: nothing changed.
    InUse,
}

impl Model {
    /// The number of cores of the node.
    pub(crate) fn width(&self) -> usize {
        self.cores.len()
    }

    /// The part of the core the current event runs on.
    pub(crate) fn here(&self) -> &CorePart {
        &self.cores[*self.at]
    }

    pub(crate) fn here_mut(&mut self) -> &mut CorePart {
        let at = *self.at;
        &mut self.cores[at]
    }

    /// The position of a core of this node by its core number.
    pub(crate) fn position(&self, number: u64) -> Option<usize> {
        self.cores
            .iter()
            .position(|c| u64::from(c.number) == number)
    }

    /// The positions of the node's cores as a bitmap.
    fn all_cores(&self) -> u64 {
        match self.width() {
            64 => u64::MAX,
            n => (1 << n) - 1,
        }
    }

    /// The core a thread's events run on: the core that runs it, else the
    /// core of its context, else the node's first core.
    pub(crate) fn core_of(&self, thread: ObjId) -> usize {
        if let Some(j) = self.cores.iter().position(|c| c.thread == Some(thread)) {
            return j;
        }
        match self.context_of(thread) {
            Some(context) => self.scheds[&context].core,
            None => 0,
        }
    }

    /// The core that runs `thread` now, if one does.
    pub(crate) fn running_core(&self, thread: ObjId) -> Option<usize> {
        self.cores.iter().position(|c| c.thread == Some(thread))
    }

    /// The kernel sends core `to` one of its interrupts through the gate
    /// (G10). Never on a node of one core, never to the calling core.
    pub(crate) fn post(&mut self, to: usize, ipi: Ipi) {
        debug_assert!(to != *self.at || ipi == Ipi::Ring);
        self.pay(Cost::Interrupt);
        self.cores[to].ipis.insert(ipi);
    }

    /// A thread that runs on another core of the node must leave it: the
    /// thread is marked *leaving its core*, and its core is sent the leave
    /// interrupt. Returns whether it was running there.
    pub(crate) fn mark_leaving(&mut self, thread: ObjId) -> bool {
        let Some(core) = self.running_core(thread) else {
            return false;
        };
        if core == *self.at {
            return false;
        }
        if !self.threads[&thread].leaving {
            live(&mut self.threads, thread).leaving = true;
            self.post(core, Ipi::Leave);
            self.observe();
        }
        true
    }

    /// The core on which an event runs stops running a thread marked
    /// *leaving its core*: it has saved the thread's registers into the
    /// thread object, and clears the mark, which makes its context
    /// eligible again where it is now placed (What runs next).
    pub(crate) fn leave(&mut self) {
        let at = *self.at;
        let Some(thread) = self.cores[at].thread else {
            return;
        };
        if self.threads.get(&thread).is_some_and(|t| t.leaving) {
            live(&mut self.threads, thread).leaving = false;
            // The context it ran on, moved meanwhile, may run on its new
            // core from now on (D-141).
            if let Some((context, _)) = self.cores[at].running
                && let Some(sched) = self.scheds.get_mut(&context)
                && sched.moving
            {
                sched.moving = false;
                if !self.change_list.contains(&context) {
                    self.change_list.push(context);
                }
            }
            self.cores[at].thread = None;
            self.cores[at].running = None;
            self.observe();
        }
    }

    /// Whether a change to the translations of `space` needs the cores of
    /// the node (x86-64 and RISC-V, a node of several cores, another core
    /// in the space's set of cores that loaded its tag); the kernel's own
    /// space (`None`) asks every other core, idle ones included.
    fn others_hold(&self, space: Option<ObjId>) -> bool {
        if self.width() == 1 || self.machine.arch == crate::Arch::Aarch64 {
            return false;
        }
        let me = 1u64 << *self.at;
        match space {
            Some(space) => self.aspaces.get(&space).is_some_and(|a| a.cores & !me != 0),
            None => true,
        }
    }

    /// A change that removes or reduces translations: the invalidation of
    /// `flush` on every core that may hold them (TLB). The calling core
    /// invalidates on itself in the call. On a node of one core, and on
    /// ARM64 (a broadcast completed by `DSB ISH`), the invalidation is
    /// complete in the call. Otherwise the calling core's request is
    /// written, each core asked gets its pending bit and the invalidation
    /// interrupt, and an idle core is not asked but owes the invalidation
    /// (its owes bit set by a compare-and-swap that expects its mark).
    pub(crate) fn start_request(
        &mut self,
        flush: Flush,
        completes: Completes,
        waiter: Option<ObjId>,
    ) -> Started {
        let at = *self.at;
        let space = match &flush {
            Flush::Range { space, .. } => Some(*space),
            Flush::Tag(space) => Some(*space),
            Flush::Kernel(_) | Flush::Nothing => None,
        };
        if !self.others_hold(space) {
            match self.machine.arch {
                crate::Arch::Aarch64 if self.width() > 1 => {
                    self.pay(Cost::Broadcast);
                    for core in 0..self.width() {
                        self.invalidate_on(core, &flush);
                    }
                }
                _ => self.invalidate_on(at, &flush),
            }
            return Started::Complete;
        }
        if self.cores[at].request_state == RequestState::InUse {
            return Started::InUse;
        }
        self.invalidate_on(at, &flush);
        let me = 1u64 << at;
        let candidates = match space {
            Some(space) => self.aspaces[&space].cores & !me,
            // The kernel's own space: every other core, idle ones included
            // (D-131): an idle core runs on the kernel's root.
            None => self.all_cores() & !me,
        };
        let mut asked = 0;
        for core in (0..self.width()).filter(|&c| candidates & 1 << c != 0) {
            // A requester sets its pending bit and then reads the idle
            // word; a core going idle sets its mark and then reads its
            // pending set: each by a compare-and-swap, so one sees the other.
            self.pay(Cost::IdleWord);
            if space.is_some() && self.cores[core].idle.mark {
                self.cores[core].idle.owes = true;
                continue;
            }
            asked |= 1 << core;
            self.cores[core].pending.insert(at);
            self.post(core, Ipi::Invalidate);
        }
        self.pay(Cost::Request);
        let request = Request {
            flush,
            asked,
            acked: 0,
            started: self.locked_now(),
            completes,
            waiter,
        };
        self.cores[at].request = Some(request);
        self.cores[at].request_state = RequestState::InUse;
        if asked == 0 {
            // A request that asks no core completes in the call that starts
            // it (every other core of the set is idle and owes it).
            self.complete(at);
            return Started::Complete;
        }
        if let Some(thread) = waiter {
            live(&mut self.threads, thread).wait = Wait::Cores(at);
            self.wait_mark(crate::locks::WaitKind::Cores);
            self.observe();
        }
        Started::Asked
    }

    /// The local invalidation of `flush` on core `core`: its cached
    /// translations go, and a whole tag clears the core's bit in the set of
    /// cores that loaded it.
    pub(crate) fn invalidate_on(&mut self, core: usize, flush: &Flush) {
        if *flush == Flush::Nothing {
            return;
        }
        let part = &mut self.cores[core];
        // What is still mapped is cached again at the core's next event.
        part.cached.0 = (u64::MAX, None);
        match flush {
            Flush::Range { space, addresses } => {
                if let Some(cached) = part.tlb.get_mut(space) {
                    for at in addresses {
                        cached.remove(at);
                    }
                }
            }
            Flush::Tag(space) => {
                part.tlb.remove(space);
                if let Some(aspace) = self.aspaces.get_mut(space) {
                    aspace.cores &= !(1 << core);
                }
            }
            Flush::Kernel(chunks) => {
                for chunk in chunks {
                    part.kernel_tlb.remove(chunk);
                }
            }
            Flush::Nothing => {}
        }
    }

    /// The barrier before a watermark returns (A node of several cores,
    /// D-138): the request of the calling core with nothing to invalidate
    /// and the kernel epoch of the moment, asking every other core of the
    /// node, idle ones included, on every architecture (on ARM64 it is the
    /// request's only use). Each asked core answers at the interrupt, which
    /// it takes at the end of its current event; the completion raises the
    /// barrier epoch. Nothing if the request of the core is in use.
    pub(crate) fn start_barrier(&mut self) {
        let at = *self.at;
        if self.width() == 1 || self.cores[at].request_state == RequestState::InUse {
            return;
        }
        let asked = self.all_cores() & !(1u64 << at);
        for core in (0..self.width()).filter(|&c| asked & 1 << c != 0) {
            self.cores[core].pending.insert(at);
            self.post(core, Ipi::Invalidate);
        }
        self.pay(Cost::Request);
        self.cores[at].request = Some(Request {
            flush: Flush::Nothing,
            asked,
            acked: 0,
            started: self.locked_now(),
            completes: Completes::Barrier(self.kernel_epoch),
            waiter: None,
        });
        self.cores[at].request_state = RequestState::InUse;
    }

    /// At the invalidation interrupt, a core does on itself what each
    /// request pending on it asks, clears that request's bit in its pending
    /// set, and sets its own position in the request's acknowledged set by
    /// an atomic OR; the core whose acknowledgement completes the asked set
    /// completes the request in the same event.
    pub(crate) fn answer_pending(&mut self) {
        let at = *self.at;
        let pending: Vec<usize> = std::mem::take(&mut self.cores[at].pending)
            .into_iter()
            .collect();
        for requester in pending {
            let Some(flush) = self.cores[requester]
                .request
                .as_ref()
                .map(|r| r.flush.clone())
            else {
                continue;
            };
            self.invalidate_on(at, &flush);
            let request = self.cores[requester].request.as_mut().expect("a request");
            request.acked |= 1 << at;
            if request.acked == request.asked {
                self.complete(requester);
            }
        }
    }

    /// The completion of the request of core `requester`: through the gate
    /// the counts of the leaves it cleared are lowered (G4, G6), the record
    /// is emptied, the destruction step or the give may go on, the waiting
    /// thread becomes runnable with its result, and the request is freed.
    fn complete(&mut self, requester: usize) {
        // The completing core writes the state word *done*; the rest of the
        // request is written by its own core only (D-135).
        let request = self.cores[requester].request.take().expect("a request");
        self.cores[requester].request_state = RequestState::Done;
        match request.completes {
            Completes::Record(cap) => self.finish_unmapping(cap),
            Completes::Protect => {}
            Completes::Destroy(space) => {
                if let Some(aspace) = self.aspaces.get_mut(&space) {
                    aspace.flushed = true;
                }
            }
            Completes::Run(freed) => self.write_freed(freed),
            Completes::Barrier(epoch) => self.barrier_epoch = self.barrier_epoch.max(epoch),
        }
        if let Some(thread) = request.waiter
            && self
                .threads
                .get(&thread)
                .is_some_and(|t| t.wait == Wait::Cores(requester))
        {
            self.end_bounded(thread);
            self.wake(thread, Ok(Output::Done));
        }
    }

    /// The thread of a request that is destroyed: the request completes
    /// without it (its effect on records and counts is applied as if the
    /// thread still waited).
    pub(crate) fn forget_waiter(&mut self, thread: ObjId) {
        for core in &mut self.cores {
            if let Some(request) = core.request.as_mut()
                && request.waiter == Some(thread)
            {
                request.waiter = None;
            }
        }
    }

    /// Before it goes idle a core loads the kernel's own root, sets its
    /// idle mark, and then reads its pending set and answers what is
    /// pending. Not on a node of one core.
    pub(crate) fn go_idle(&mut self) {
        let at = *self.at;
        self.cores[at].loaded = None;
        if self.width() == 1 {
            return;
        }
        if !self.cores[at].idle.mark {
            self.pay(Cost::IdleWord);
            self.cores[at].idle.mark = true;
        }
        if !self.cores[at].pending.is_empty() {
            self.answer_pending();
        }
    }

    /// A core that wakes clears both bits of its idle word by one
    /// compare-and-swap and, if the owes bit was set, invalidates every
    /// translation under the tags of its node before it loads a root of
    /// the node again.
    pub(crate) fn wake_core(&mut self) {
        let at = *self.at;
        if self.width() == 1 || !self.cores[at].idle.mark {
            return;
        }
        self.pay(Cost::IdleWord);
        let owed = self.cores[at].idle.owes;
        self.cores[at].idle = IdleWord::default();
        if owed {
            let tags: Vec<ObjId> = self.cores[at].tlb.keys().copied().collect();
            for space in tags {
                self.invalidate_on(at, &Flush::Tag(space));
            }
        }
    }

    /// The root a core loads for the thread it runs (G7), through the gate:
    /// its address space, which enters the space's set of cores that loaded
    /// its tag; and what the core caches of it. An idle core runs on the
    /// kernel's root.
    pub(crate) fn load_root(&mut self) {
        let at = *self.at;
        let space = self.cores[at]
            .thread
            .and_then(|t| self.threads[&t].aspace)
            .filter(|&a| !self.dying(a));
        self.cores[at].loaded = space;
        if let Some(space) = space {
            let bit = 1u64 << at;
            if self.aspaces[&space].cores & bit == 0 && self.width() > 1 {
                self.pay(Cost::LoadedSet);
                live(&mut self.aspaces, space).cores |= bit;
            }
        }
        self.cache_here();
    }

    /// What the core may cache from now on: every translation of the space
    /// whose root it runs on, and the kernel's own space.
    pub(crate) fn cache_here(&mut self) {
        let at = *self.at;
        let now = (self.added.0, self.cores[at].loaded);
        if self.cores[at].cached.0 == now {
            return;
        }
        self.cores[at].cached.0 = now;
        if let Some(space) = self.cores[at].loaded {
            let entries: Cached = self.aspaces[&space]
                .translations
                .iter()
                .map(|(&a, t)| (a, (t.cap, t.page, t.rights)))
                .collect();
            let cached = self.cores[at].tlb.entry(space).or_default();
            for (address, entry) in entries {
                cached.insert(address, entry);
            }
        }
        let runs: Vec<u64> = self.kernel_runs.iter().copied().collect();
        self.cores[at].kernel_tlb.extend(runs);
    }

    /// A spurious fault (D-129): a page fault on an address that the node's
    /// own tables translate with the access asked is resolved on the
    /// faulting core without a message: the kernel invalidates that address
    /// there and resumes the thread.
    pub(crate) fn spurious(&mut self, thread: ObjId, address: u64, access: u64) -> bool {
        let Some(space) = self.threads[&thread].aspace else {
            return false;
        };
        let Some(rights) = self.translated_in(space, address) else {
            return false;
        };
        let needs = [
            (dodos_abi::ACCESS_READ, Rights::READ),
            (dodos_abi::ACCESS_WRITE, Rights::WRITE),
            (dodos_abi::ACCESS_EXEC, Rights::EXEC),
        ];
        let allowed = needs
            .iter()
            .all(|&(bit, right)| access & bit == 0 || rights.contains(right));
        if !allowed {
            return false;
        }
        // The base of the translation that covers the address, which is
        // the key the core's TLB holds it by: a large page is invalidated
        // whole (the final review's verification of lot C, H5).
        let (&base, _) = self.aspaces[&space]
            .translations
            .range(..=address)
            .next_back()
            .expect("translated");
        let core = self.core_of(thread);
        let flush = Flush::Range {
            space,
            addresses: vec![base],
        };
        self.invalidate_on(core, &flush);
        true
    }

    /// The rights with which a space translates an address, if it does.
    pub(crate) fn translated_in(&self, space: ObjId, address: u64) -> Option<Rights> {
        let (&at, t) = self.aspaces[&space]
            .translations
            .range(..=address)
            .next_back()?;
        (address < at + self.bytes(t.size)).then_some(t.rights)
    }

    /// A call or a step that needs the request of its core while it is in
    /// use returns `again` with nothing changed.
    pub(crate) fn request_free(&self) -> Result<(), Error> {
        let needs = self.width() > 1 && self.machine.arch != crate::Arch::Aarch64;
        match needs && self.here().request_state == RequestState::InUse {
            true => Err(Error::Again),
            false => Ok(()),
        }
    }

    /// The interrupts pending on a core, taken in one event of that core:
    /// it answers the requests pending on it, stops running a thread
    /// marked *leaving its core*, serves the user rings into the node that
    /// signalled it, and chooses again. Returns whether it took any.
    pub fn take_interrupts(&mut self, core: usize) -> bool {
        if self.cores[core].ipis.is_empty() || self.cores[core].silent {
            return false;
        }
        *self.last_event = None;
        let entry = self.begin_on(core);
        let ipis = std::mem::take(&mut self.cores[core].ipis);
        if ipis.contains(&Ipi::Invalidate) {
            self.answer_pending();
        }
        if ipis.contains(&Ipi::Ring) {
            self.serve_user_rings();
        }
        self.end(entry);
        true
    }

    /// A node of several cores watches its own cores at their events: "if
    /// ... its heartbeat goes stale while it is not idle, the node accuses
    /// itself (evidence self)". A core whose heartbeat has not moved for
    /// `STALE_NS` and that is not idle (now − `STALE_NS` past its
    /// `idle_until`, as for stale evidence) is a failure of the node. Not
    /// before the node is ready: its cores enter one by one.
    /// "Each core of the node compares, at each of its timer events, the
    /// heartbeats of the node's other cores with what it last saw" (A node
    /// of several cores, gap 16).
    pub(crate) fn watch_own_cores(&mut self) {
        // No accusation, of itself either, while the release word of the
        // stub is raised (The debugger stub, D-135, D-137).
        if self.width() == 1 || !self.ready || self.table.release {
            return;
        }
        // Both sides of each comparison on the clock of membership (D-139).
        let (now, stale, at) = (self.membership_now(), self.table.stale_ns, *self.at);
        let mut silent = false;
        for position in (0..self.width()).filter(|&p| p != at) {
            let number = self.cores[position].number as usize;
            let beacon = &self.table.core_beacons[number];
            let (heartbeat, idle_until) = (beacon.heartbeat.0, beacon.idle_until.0);
            let watched = &mut self.cores[position].watched.0;
            if watched.0 != heartbeat || watched.1 == 0 {
                *watched = (heartbeat, now.max(1));
                continue;
            }
            silent |= now >= watched.1 + stale && now - stale > idle_until;
        }
        if silent {
            self.accuse_self(dodos_abi::CrashReason::CoreSilent as u32);
        }
    }

    /// The interrupts pending on the node's cores, as `System` sees them.
    pub fn pending_ipis(&self) -> Vec<(usize, Ipi)> {
        let pending = self
            .cores
            .iter()
            .enumerate()
            .flat_map(|(core, part)| part.ipis.iter().map(move |&ipi| (core, ipi)));
        pending.collect()
    }

    /// Unless the caller interleaves them (`System`), the interrupts an
    /// event sent are taken at once, each core in position order, until
    /// none is pending.
    pub(crate) fn auto_deliver(&mut self) {
        if self.manual {
            return;
        }
        // Each delivery takes at least one interrupt and sends only those
        // its own event needs, which a node of N cores bounds.
        for _ in 0..64 * (self.width() + 1) {
            let Some(core) = (0..self.width())
                .find(|&c| !self.cores[c].ipis.is_empty() && !self.cores[c].silent)
            else {
                return;
            };
            self.take_interrupts(core);
        }
    }

    /// A core of the node stops answering (a test's input): it takes no
    /// event, no interrupt and no timer from now on.
    pub fn silence(&mut self, core: usize) {
        self.cores[core].silent = true;
    }

    /// The generation the node's state was built for.
    pub(crate) fn generation_built(&self) -> u32 {
        self.built
    }

    /// Whether the core at `position` takes no event (parked, hung, or not
    /// yet entered).
    pub fn is_silent(&self, position: usize) -> bool {
        self.cores[position].silent
    }

    /// Whether the core at `position` has a context or a thread to run and
    /// no interrupt pending to wake it.
    pub(crate) fn runs_unwoken(&self, position: usize) -> bool {
        let part = &self.cores[position];
        (part.running.is_some() || part.thread.is_some()) && part.ipis.is_empty()
    }

    /// Whether the node is ready: its state built, its first thread able to
    /// run (before it is live on a fresh start).
    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    /// The physical core number of the core at `position`.
    pub(crate) fn core_number(&self, position: usize) -> usize {
        self.cores[position].number as usize
    }

    /// The next instant the core at `position` must act: its hardware
    /// timer.
    pub(crate) fn timer_of(&self, position: usize) -> Option<u64> {
        self.hardware(position)
    }

    /// The exit latency of the idle state that loses its context which the
    /// core at `position` enters now, if any: the core is idle (it runs
    /// nothing, no interrupt waits, nothing is due), and the deepest state
    /// of its idle table whose exit latency plus minimum residency is
    /// shorter than the time until its next instant, the deepest with none
    /// armed, loses its context (Idle; `node_idle`, flags bit 0). Until a
    /// table is given the core halts, which loses nothing.
    pub(crate) fn sleep_state(&self, position: usize) -> Option<u64> {
        let part = &self.cores[position];
        if part.silent
            || !self.ready
            || part.running.is_some()
            || part.thread.is_some()
            || !part.ipis.is_empty()
            || self.due_now(position)
        {
            return None;
        }
        let until = self
            .hardware(position)
            .map(|at| at.saturating_sub(self.now));
        let entry = part
            .idle_table
            .iter()
            .rev()
            .find(|e| until.is_none_or(|u| e[0].saturating_add(e[1]) < u))?;
        // The gate decides from the parameter, never from the flag (D-146).
        crate::nodes::loses_context(entry[2], &self.machine).then_some(entry[0])
    }

    /// Whether a core's timer is due at the current instant.
    pub(crate) fn due_now(&self, core: usize) -> bool {
        self.hardware(core).is_some_and(|at| at <= self.now)
    }

    /// The machine's clock, as `System` moves it.
    pub(crate) fn set_now(&mut self, now: u64) {
        self.now = self.now.max(now);
    }

    /// A failure the node finds in itself on its core at `position` (a
    /// test's input): an event of that core in which it accuses itself.
    pub fn fail_on(&mut self, position: usize) {
        *self.last_event = None;
        let entry = self.begin_on(position);
        self.accuse_self(dodos_abi::CrashReason::Assertion as u32);
        self.end(entry);
    }

    /// An interrupt from another node or from the machine, taken by the
    /// first core of the node that answers.
    pub(crate) fn receive_interrupt(&mut self, ipi: Ipi) {
        self.stalled = None;
        if let Some(core) = (0..self.width()).find(|&c| !self.cores[c].silent) {
            self.cores[core].ipis.insert(ipi);
        }
    }

    /// A fresh start, before any thread runs: the consumer index of every
    /// ring into the node is set to its producer index, and its cores take
    /// no event until each has entered.
    pub(crate) fn begin_fresh(&mut self) {
        self.skip_rings();
        self.ready = false;
        for core in &mut self.cores {
            core.silent = true;
        }
    }

    /// A core enters the node by the entry stub: it invalidates its whole
    /// TLB, clears its interrupt table and, at a fresh start, writes its
    /// word owned through the node's gate, joining since its start or its
    /// resume (D-158); after a fresh start the node becomes live when no
    /// core word naming it reads joining. At a resume the core at position
    /// 0 runs the steps of the resume, which end with live.
    pub(crate) fn core_entered(&mut self, number: usize) {
        let Some(position) = self.position(number as u64) else {
            return;
        };
        let part = &mut self.cores[position];
        part.silent = false;
        part.tlb.clear();
        part.kernel_tlb.clear();
        part.cached.0 = (u64::MAX, None);
        part.loaded = None;
        part.running = None;
        part.thread = None;
        part.ipis.clear();
        // The header of the core's debug area counts the entries
        // suppressed since machine start (Debugging, the trace rings): a
        // kernel that starts on the core counts those of the earlier
        // kernels as reported already (pass 25: invariant 91 failed after
        // a fresh start).
        if !self.resuming {
            self.cores[position].reported = self.table.crash.areas[number].suppressed;
        }
        let generation = self.table.generation(self.node);
        let before = self.table.core_words[number];
        let joining = CoreWordOf::from_raw(before)
            .is_some_and(|w| w.state() == dodos_abi::CoreState::Joining);
        if joining {
            self.table
                .write_core(number, before, CoreWordOf::owned(self.node, generation));
        }
        self.enter_idle(position);
        // The core answers again: a context of it throttled while it was
        // silent, whose replenishment instant has passed, is replenished at
        // once ("If that instant is not after now, it is replenished at
        // once"; finding 4 of the tenth model review, finding 15 of the
        // ninth: invariant 43 failed at a resume).
        let now = self.now;
        let late: Vec<(ObjId, dodos_abi::SchedDebt)> = self
            .scheds
            .iter()
            .filter(|(_, sched)| sched.core == position)
            .filter_map(|(&id, sched)| {
                let (instant, debt) = sched.throttled?;
                (instant <= now).then_some((id, debt))
            })
            .collect();
        if !late.is_empty() {
            let entry = self.begin_on(position);
            for (context, debt) in late {
                self.replenish(context, debt);
            }
            self.end(entry);
        }
        let fresh = !self.resuming;
        // A fresh start: the core at position 0 built the start-up state, and
        // the first thread runs from then on, on the cores that entered,
        // before the node is live (Cores, `node_start`: "the first thread
        // runs the chosen module; the node becomes live when every core
        // reads owned"; pass 28: it ran only once the node was live). At no
        // event of its cores: each core whose choice is not what it runs
        // takes a reschedule, at which it chooses (pass 25, invariant 46).
        if fresh && !self.ready && position == 0 {
            self.ready = true;
            let entered: Vec<usize> = (0..self.width())
                .filter(|&p| !self.cores[p].silent)
                .collect();
            for p in entered {
                if self.pick(p).map(|w| w.0) != self.cores[p].running.map(|r| r.0) {
                    self.cores[p].ipis.insert(Ipi::Reschedule);
                }
            }
        }
        let member = self.table.member(self.node);
        let owned = !self.table.joining(self.node, member.generation());
        if owned && fresh && member.state() == dodos_abi::MemberState::Starting {
            let live = dodos_abi::Member::new(
                dodos_abi::MemberState::Live,
                member.evidence(),
                member.driver(),
                member.generation(),
            )
            .with_class(member.class());
            self.table.write_member(self.node, self.node, live);
        }
        if !fresh && position == 0 {
            self.stalled = None;
        }
    }

    /// A core that enters with nothing to run goes idle: it loads the
    /// kernel's own root and sets its idle mark (not on a node of one
    /// core, which has none).
    pub(crate) fn enter_idle(&mut self, position: usize) {
        let several = self.width() > 1;
        let part = &mut self.cores[position];
        part.loaded = None;
        if several && part.thread.is_none() {
            part.idle.mark = true;
        }
    }

    /// A resume: the node's cores start from their park, and no thread runs
    /// before the steps of the resume are done. A sealed block the image
    /// cannot read makes the node accuse itself.
    pub(crate) fn begin_resume(&mut self, readable: bool) {
        self.ready = false;
        self.resuming = true;
        self.seal = None;
        if !readable {
            *self.at = 0;
            self.accuse_self(dodos_abi::CrashReason::StartRefused as u32);
        }
    }
}

use dodos_abi::CoreWord as CoreWordOf;
