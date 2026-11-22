//! Two cores racing inside gate calls (Gate words, the gate lock; G12): the
//! calls whose words another node reads, split into their word accesses,
//! every interleaving enumerated. Node *X* has two cores; each runs a
//! program of gate calls on one request entry: an attach of its own device,
//! then the release of the answer. The holder *H* (one core) accepts,
//! completes and answers. *X*'s fence may stop either core at any word
//! access, leaving its call half done and its gate lock held; then R1 (by
//! *H*) and R6. The gate lock, as the contract states it: a queue lock of
//! the node, taken by the gate at every entry that writes, released at its
//! exit, held for one call and never across a wait for another core.
//!
//! The x86-64 entry and exit sequence of a gate call is a draft under
//! review (research/drafts/) and is not modelled.

use std::collections::BTreeSet;

/// A request record's operation, as words 0 to 2 hold it: the attach of a
/// device.
type Words = u8;

/// The state of a request record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Record {
    Void,
    Open,
    Done,
}

/// The attach word's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Attach {
    Zero,
    Pinned,
    Attached,
    Detached,
}

/// Node *X*'s member word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Member {
    Live,
    Sealing,
    Fenced,
    Dead,
}

/// Where a core of *X* is: in its program (which call, at which word
/// access), done, parked by the seal's own park call without returning, or
/// stopped by the fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Pc {
    /// The attach, at access `k`.
    Attach(u8),
    /// The release, at access `k`.
    Release(u8),
    /// The seal's park call, at access `k` (D-168 (3)).
    SealPark(u8),
    Done,
    /// Stopped by the seal's park call without returning (D-168 (3)).
    Parked,
    Stopped(u8, bool),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct State {
    /// Per device: its identifier word taken by *X*, and its attach word.
    pub(crate) ids: [bool; 2],
    pub(crate) attach: [Attach; 2],
    /// The iospace root's count of devices, and the domain word bound.
    pub(crate) root_count: u8,
    pub(crate) domain: bool,
    /// The one request entry both cores use: words, serial, state.
    pub(crate) words: Option<Words>,
    pub(crate) serial: u8,
    pub(crate) record: Record,
    /// *X*'s gate record of the entry: the serial and the words its opener
    /// wrote.
    pub(crate) gate_record: Option<(u8, Words)>,
    /// The unit: context entries per device, and its cache of them.
    pub(crate) context: [bool; 2],
    pub(crate) cache: [bool; 2],
    /// *H*'s service of the entry (serial, words), completed or not.
    pub(crate) service: Option<(u8, Words, bool)>,
    /// *X*'s cores, its gate lock (the core holding it), its `seal_parked`
    /// words (one per core), its member word, and R1's walk for it.
    pub(crate) cores: [Pc; 2],
    pub(crate) lock: Option<usize>,
    pub(crate) seal_parked: [bool; 2],
    pub(crate) member: Member,
    pub(crate) r1_done: bool,
}

/// The variants: the contract's lock, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Variant {
    pub(crate) lock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Core `c` of *X* makes its next word access.
    X(usize),
    Accept,
    Complete,
    Answer,
    /// *X* finishes sealing once core 0 wrote `seal_parked` and parked and
    /// core 1 finished its in-flight call, then resumes so core 1 makes a
    /// gate call (D-168 (3)).
    Resume,
    Fence,
    R1,
    R6,
    /// The unit caches the context entries (hardware).
    Fill,
}

impl State {
    pub(crate) fn new() -> Self {
        Self {
            ids: [false; 2],
            attach: [Attach::Zero; 2],
            root_count: 0,
            domain: false,
            words: None,
            serial: 0,
            record: Record::Void,
            gate_record: None,
            context: [false; 2],
            cache: [false; 2],
            service: None,
            cores: [Pc::Attach(0), Pc::Attach(0)],
            lock: None,
            seal_parked: [false; 2],
            member: Member::Live,
            r1_done: false,
        }
    }

    /// Node *X* is sealing: core 0 executes the seal's park call while core 1
    /// runs `other` (either racing with core 0 on a gate call during the seal,
    /// or already done and waiting for the resume); once core 0 has written
    /// `seal_parked` and stopped and core 1 is done, `Step::Resume` resumes
    /// *X* and core 1 makes a gate call (D-168 (3)).
    pub(crate) fn sealing(other: Pc) -> Self {
        let mut s = Self::new();
        s.member = Member::Sealing;
        s.cores = [Pc::SealPark(0), other];
        s
    }

    fn runs(&self, c: usize) -> bool {
        match self.member {
            Member::Live => matches!(self.cores[c], Pc::Attach(_) | Pc::Release(_)),
            Member::Sealing => {
                matches!(
                    self.cores[c],
                    Pc::Attach(_) | Pc::Release(_) | Pc::SealPark(_)
                )
            }
            Member::Fenced | Member::Dead => false,
        }
    }

    pub(crate) fn steps(&self, v: Variant) -> Vec<Step> {
        let mut out = Vec::new();
        for c in 0..2 {
            if !self.runs(c) {
                continue;
            }
            // A call's first access takes the lock; a core finding it held
            // by the other spins (no step).
            let at_entry = matches!(
                self.cores[c],
                Pc::Attach(0) | Pc::Release(0) | Pc::SealPark(0)
            );
            let blocked = v.lock && at_entry && self.lock.is_some_and(|h| h != c);
            if !blocked {
                out.push(Step::X(c));
            }
        }
        if self.record == Record::Open && self.service.is_none() {
            out.push(Step::Accept);
        }
        if self.service.is_some_and(|(_, _, done)| !done) {
            out.push(Step::Complete);
        }
        if self.service.is_some_and(|(_, _, done)| done) {
            out.push(Step::Answer);
        }
        if self.member == Member::Sealing
            && self.seal_parked[0]
            && self.cores[0] == Pc::Parked
            && self.cores[1] == Pc::Done
            && self.record != Record::Open
            && self.service.is_none()
        {
            out.push(Step::Resume);
        }
        if matches!(self.member, Member::Live | Member::Sealing) {
            out.push(Step::Fence);
        }
        if self.member == Member::Fenced && !self.r1_done {
            out.push(Step::R1);
        }
        if self.member == Member::Fenced && self.r1_done {
            out.push(Step::R6);
        }
        if self.context != self.cache && self.context.iter().zip(self.cache).any(|(&c, k)| c && !k)
        {
            out.push(Step::Fill);
        }
        out
    }

    /// Core `c`'s next word access. Each arm is one access of the living
    /// part (a compare-and-swap against what the gate read) or of the gate
    /// part; a check that fails ends the call (releasing the lock).
    fn x(&mut self, c: usize, v: Variant) {
        let device = c;
        let end = |s: &mut State, next: Pc| {
            if s.lock == Some(c) {
                s.lock = None;
            }
            s.cores[c] = next;
        };
        match self.cores[c] {
            Pc::Attach(0) | Pc::Release(0) | Pc::SealPark(0) => {
                if v.lock {
                    self.lock = Some(c);
                }
                self.cores[c] = match self.cores[c] {
                    Pc::Attach(_) => Pc::Attach(1),
                    Pc::Release(_) => Pc::Release(1),
                    _ => Pc::SealPark(1),
                };
            }
            // The attach: the checks (attach word 0; the entry neither open
            // nor held by a gate record), then the identifier word, the
            // attach word, the root's count, the domain word, words 0 to 2,
            // the state word.
            Pc::Attach(1) => {
                let free = self.record != Record::Open && self.gate_record.is_none();
                if self.attach[device] != Attach::Zero || !free {
                    end(self, Pc::Done);
                } else {
                    self.ids[device] = true;
                    self.cores[c] = Pc::Attach(2);
                }
            }
            Pc::Attach(2) => {
                self.attach[device] = Attach::Pinned;
                self.cores[c] = Pc::Attach(3);
            }
            Pc::Attach(3) => {
                self.root_count += 1;
                self.cores[c] = Pc::Attach(4);
            }
            Pc::Attach(4) => {
                self.domain = true;
                self.cores[c] = Pc::Attach(5);
            }
            Pc::Attach(5) => {
                self.words = Some(device as Words);
                self.cores[c] = Pc::Attach(6);
            }
            Pc::Attach(6) => {
                // (s, not open) → (s + 1, open), the gate record written.
                if self.record == Record::Open {
                    end(self, Pc::Done);
                } else {
                    self.serial += 1;
                    self.record = Record::Open;
                    self.gate_record = Some((self.serial, device as Words));
                    end(self, Pc::Release(0));
                }
            }
            // The release, on the state word at the gate record's serial:
            // the attach word stays *attached* (done), the gate record freed.
            Pc::Release(1) => {
                let answered = self.record == Record::Done
                    && self.gate_record.is_some_and(|(s, _)| s == self.serial);
                if answered {
                    self.gate_record = None;
                    end(self, Pc::Done);
                } else {
                    // Not yet: the call ends, the program retries.
                    end(self, Pc::Release(0));
                }
            }
            // The seal's park call (Gate words, "The park request, `parked`,
            // `seal_parked` and the start request", D-168 (3)): while sealing,
            // write `seal_parked`, release the gate lock, and stop the core
            // without returning.
            Pc::SealPark(1) => {
                if self.member == Member::Sealing {
                    self.seal_parked[c] = true;
                }
                end(self, Pc::Parked);
            }
            _ => {}
        }
    }

    pub(crate) fn take(&mut self, step: Step, v: Variant) {
        match step {
            Step::X(c) => self.x(c, v),
            Step::Accept => {
                // The holder reads the state word, then words 0 to 2, then
                // checks (the identifier word names *X*, a live requester,
                // the attach word *pinned*) and moves the attach word, then
                // writes the context entry.
                let Some(d) = self.words else {
                    return;
                };
                let d = usize::from(d);
                let checks =
                    self.ids[d] && self.member == Member::Live && self.attach[d] == Attach::Pinned;
                if checks {
                    self.attach[d] = Attach::Attached;
                    self.context[d] = true;
                }
                self.service = Some((self.serial, d as Words, false));
            }
            Step::Complete => {
                if let Some(s) = self.service.as_mut() {
                    s.2 = true;
                }
            }
            Step::Answer => {
                let Some((serial, _, _)) = self.service.take() else {
                    return;
                };
                if self.record == Record::Open && self.serial == serial {
                    self.record = Record::Done;
                }
            }
            Step::Resume => {
                // The node sealed once core 0 wrote `seal_parked` and parked,
                // and now resumes: core 1 makes a gate call on the resumed
                // node while core 0 stays done.
                self.member = Member::Live;
                self.seal_parked = [false; 2];
                self.cores = [Pc::Done, Pc::Attach(0)];
            }
            Step::Fence => {
                self.member = Member::Fenced;
                for pc in self.cores.iter_mut() {
                    match *pc {
                        Pc::Attach(k) => *pc = Pc::Stopped(k, true),
                        Pc::Release(k) | Pc::SealPark(k) => *pc = Pc::Stopped(k, false),
                        Pc::Done | Pc::Parked | Pc::Stopped(_, _) => {}
                    }
                }
            }
            Step::R1 => {
                // R1 for *X*: each attach word of *X*'s generation reading
                // *attached* has its context entry cleared, the unit's cache
                // invalidated, and the word written *detached* (leaving
                // `root_count` unchanged until R6 clears both).
                for d in 0..2 {
                    if self.ids[d] && self.attach[d] == Attach::Attached {
                        self.context[d] = false;
                        self.cache[d] = false;
                        self.attach[d] = Attach::Detached;
                    }
                }
                self.r1_done = true;
            }
            Step::R6 => {
                for d in 0..2 {
                    if self.ids[d] {
                        self.attach[d] = Attach::Zero;
                        self.ids[d] = false;
                    }
                }
                self.domain = false;
                self.root_count = 0;
                self.member = Member::Dead;
            }
            Step::Fill => {
                for d in 0..2 {
                    self.cache[d] |= self.context[d];
                }
            }
        }
    }

    /// The invariants after every access: (G) for the device (a context or
    /// a cached context only for an attach word *attached* with its
    /// identifier taken and the domain word bound); the record (an open
    /// record's words are its opener's, so the holder serves what the
    /// requester asked); the root's count of devices is its attach words;
    /// a dead node leaves nothing at the unit; the lock held by one core.
    pub(crate) fn check(&self, v: Variant) -> Result<(), &'static str> {
        for d in 0..2 {
            let held = self.attach[d] == Attach::Attached && self.ids[d] && self.domain;
            if (self.context[d] || self.cache[d]) && !held {
                return Err("G: the unit translates for a device whose words do not hold it");
            }
        }
        if self.record == Record::Open
            && matches!(self.member, Member::Live | Member::Sealing)
            && self.gate_record.map(|(_, w)| w) != self.words
        {
            return Err("RECORD: an open record's words are not its opener's");
        }
        // Between `Attach(2)` (attach word written *pinned*) and `Attach(3)`
        // (root's count raised) the count lags by one; a fence may stop a core
        // there, and R6 clears both at *dead*. Outside that window — across
        // live, sealing, fenced (R1 writes *detached*, keeping the count) and
        // dead — the root's count must equal the non-zero attach words.
        let mid_live = self.cores.iter().any(|pc| matches!(pc, Pc::Attach(3)));
        let mid_stopped = self.member == Member::Fenced
            && self
                .cores
                .iter()
                .any(|pc| matches!(pc, Pc::Stopped(3, true)));
        if !mid_live && !mid_stopped {
            let counted = self.attach.iter().filter(|&&a| a != Attach::Zero).count() as u8;
            if counted != self.root_count {
                return Err("COUNT: an iospace root's count is not its attach words");
            }
        }
        if self.member == Member::Dead
            && (self.context.iter().any(|&c| c) || self.cache.iter().any(|&c| c))
        {
            return Err("R1: a dead node's device still translated");
        }
        let inside = (0..2)
            .filter(|&c| {
                matches!(
                    self.cores[c],
                    Pc::Attach(1..) | Pc::Release(1..) | Pc::SealPark(1..)
                )
            })
            .count();
        if v.lock && matches!(self.member, Member::Live | Member::Sealing) && inside > 1 {
            return Err("LOCK: two cores of a node inside gate calls at once");
        }
        Ok(())
    }
}

/// What an exploration found.
#[derive(Debug, Default)]
pub(crate) struct Findings {
    pub(crate) states: usize,
    pub(crate) broken: Option<(&'static str, Vec<Step>)>,
    /// A state from which nothing moves, with a core of a live or sealing *X*
    /// still in its program (or waiting to resume).
    pub(crate) wedge: Option<Vec<Step>>,
    /// Whether `Step::Resume` was reached during the exploration.
    pub(crate) resumed: bool,
}

pub(crate) fn explore(v: Variant) -> Findings {
    explore_from(State::new(), v)
}

pub(crate) fn explore_from(start: State, v: Variant) -> Findings {
    let mut seen = BTreeSet::new();
    let mut findings = Findings::default();
    let mut stack = vec![(start, Vec::new())];
    while let Some((state, path)) = stack.pop() {
        if !seen.insert(state.clone()) {
            continue;
        }
        findings.states += 1;
        if let Err(rule) = state.check(v)
            && findings.broken.is_none()
        {
            findings.broken = Some((rule, path.clone()));
        }
        let steps = state.steps(v);
        let moving: Vec<Step> = steps
            .iter()
            .copied()
            .filter(|s| !matches!(s, Step::Fence | Step::Fill))
            .collect();
        let pending = match state.member {
            Member::Live => state
                .cores
                .iter()
                .any(|pc| matches!(pc, Pc::Attach(_) | Pc::Release(_))),
            Member::Sealing => {
                state
                    .cores
                    .iter()
                    .any(|pc| matches!(pc, Pc::Attach(_) | Pc::Release(_) | Pc::SealPark(_)))
                    || (state.seal_parked[0] && state.cores[1] == Pc::Done)
            }
            Member::Fenced | Member::Dead => false,
        };
        if moving.is_empty() && pending && findings.wedge.is_none() {
            findings.wedge = Some(path.clone());
        }
        for step in steps {
            if step == Step::Resume {
                findings.resumed = true;
            }
            let mut next = state.clone();
            next.take(step, v);
            let mut p = path.clone();
            p.push(step);
            stack.push((next, p));
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    // The gate lock as the contract states it: every interleaving of the two
    // cores' calls, the holder's and the fence's holds the invariants and
    // never wedges; a core stopped inside its call leaves its lock held,
    // which no other node reads, and R1 and R6 clear what it half wrote.
    #[test]
    fn the_gate_lock_keeps_two_racing_cores_apart() {
        let f = explore(Variant { lock: true });
        assert_eq!(f.broken, None, "{f:?}");
        assert_eq!(f.wedge, None, "{f:?}");
        assert!(f.states > 300, "{} states", f.states);
    }

    // Without the lock, the two cores open the one entry at once: one core's
    // words, the other's state word and gate record, so the holder serves an
    // attach the opener did not ask (pass 29's defect (ii), torn words, made
    // observable by two devices).
    #[test]
    fn without_the_lock_two_cores_tear_a_record() {
        let f = explore(Variant { lock: false });
        let (rule, path) = f.broken.expect("nothing broke without the lock");
        assert_eq!(
            rule, "RECORD: an open record's words are not its opener's",
            "{path:?}"
        );
    }

    // D-168 (3): while the node is sealing, core 0's seal park call takes the
    // gate lock at its first access (blocked while core 1 holds it), writes
    // `seal_parked`, releases the gate lock, and stops without returning; the
    // node then resumes and core 1 makes a gate call. Every interleaving ends
    // without a wedge and without a broken rule.
    #[test]
    fn seal_park_releases_the_gate_lock_before_stopping_the_core() {
        let v = Variant { lock: true };

        // 1. Core 0 parks for seal, then the node resumes and core 1 makes its
        // first gate call (attaching and releasing its device). Removing the
        // lock release from `Pc::SealPark(1)` wedges core 1 right after
        // `Step::Resume`.
        let quiet = explore_from(State::sealing(Pc::Done), v);
        assert_eq!(quiet.broken, None, "{quiet:?}");
        assert_eq!(quiet.wedge, None, "{quiet:?}");
        assert!(quiet.resumed, "never reached Step::Resume");
        assert!(quiet.states > 20, "{} states", quiet.states);

        // 2. Core 0's `SealPark` races with core 1's in-flight gate call while
        // sealing (testing mutual exclusion on the gate lock in both orders),
        // then resumes and core 1 makes a post-resume gate call.
        let racing = explore_from(State::sealing(Pc::Attach(0)), v);
        assert_eq!(racing.broken, None, "{racing:?}");
        assert_eq!(racing.wedge, None, "{racing:?}");
        assert!(racing.resumed, "never reached Step::Resume while racing");
        assert!(racing.states > 100, "{} states", racing.states);
    }

    // Pass 40 (v1-D #2): `Step::R1` writes *detached* (keeping `root_count`
    // matched until R6) and `COUNT` is checked while fenced too.
    #[test]
    fn r1_writes_detached_and_count_is_checked_while_fenced() {
        let mut s = State::new();
        s.ids[0] = true;
        s.attach[0] = Attach::Attached;
        s.root_count = 1;
        s.domain = true;
        s.context[0] = true;
        s.cores = [Pc::Done, Pc::Done];
        let v = Variant { lock: true };

        s.take(Step::Fence, v);
        s.take(Step::R1, v);
        assert_eq!(s.attach[0], Attach::Detached);
        assert_eq!(s.root_count, 1);
        assert_eq!(s.check(v), Ok(()));

        // Corrupting the attach word to Zero while fenced (the old R1 bug)
        // must trip COUNT.
        let mut broken = s.clone();
        broken.attach[0] = Attach::Zero;
        assert_eq!(
            broken.check(v),
            Err("COUNT: an iospace root's count is not its attach words")
        );

        s.take(Step::R6, v);
        assert_eq!(s.attach[0], Attach::Zero);
        assert_eq!(s.root_count, 0);
        assert_eq!(s.check(v), Ok(()));
    }
}
