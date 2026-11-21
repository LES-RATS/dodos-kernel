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
}

/// Node *X*'s member word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Member {
    Live,
    Fenced,
    Dead,
}

/// Where a core of *X* is: in its program (which call, at which word
/// access), done, or stopped by the fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Pc {
    /// The attach, at access `k`.
    Attach(u8),
    /// The release, at access `k`.
    Release(u8),
    Done,
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
    /// *X*'s cores, its gate lock (the core holding it), its member word,
    /// and R1's walk for it.
    pub(crate) cores: [Pc; 2],
    pub(crate) lock: Option<usize>,
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
            member: Member::Live,
            r1_done: false,
        }
    }

    fn runs(&self, c: usize) -> bool {
        self.member == Member::Live && matches!(self.cores[c], Pc::Attach(_) | Pc::Release(_))
    }

    pub(crate) fn steps(&self, v: Variant) -> Vec<Step> {
        let mut out = Vec::new();
        for c in 0..2 {
            if !self.runs(c) {
                continue;
            }
            // A call's first access takes the lock; a core finding it held
            // by the other spins (no step).
            let at_entry = matches!(self.cores[c], Pc::Attach(0) | Pc::Release(0));
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
        if self.member == Member::Live {
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
            Pc::Attach(0) | Pc::Release(0) => {
                if v.lock {
                    self.lock = Some(c);
                }
                self.cores[c] = match self.cores[c] {
                    Pc::Attach(_) => Pc::Attach(1),
                    _ => Pc::Release(1),
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
            Step::Fence => {
                self.member = Member::Fenced;
                for pc in self.cores.iter_mut() {
                    if let Pc::Attach(k) | Pc::Release(k) = *pc {
                        *pc = Pc::Stopped(k, matches!(*pc, Pc::Attach(_)));
                    }
                }
            }
            Step::R1 => {
                // R1 for *X*: each attach word of *X*'s generation reading
                // *attached* has its context entry cleared, the unit's cache
                // invalidated, the word written *detached* (here 0 at R6).
                for d in 0..2 {
                    if self.ids[d] && self.attach[d] == Attach::Attached {
                        self.context[d] = false;
                        self.cache[d] = false;
                        self.attach[d] = Attach::Zero;
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
            && self.member == Member::Live
            && self.gate_record.map(|(_, w)| w) != self.words
        {
            return Err("RECORD: an open record's words are not its opener's");
        }
        let live_calls = self.cores.iter().all(|pc| !matches!(pc, Pc::Stopped(..)));
        if live_calls && self.member == Member::Live {
            let counted = self.attach.iter().filter(|&&a| a != Attach::Zero).count() as u8;
            let mid = self.cores.iter().any(|pc| matches!(pc, Pc::Attach(3..=6)));
            if !mid && counted != self.root_count {
                return Err("COUNT: an iospace root's count is not its attach words");
            }
        }
        if self.member == Member::Dead
            && (self.context.iter().any(|&c| c) || self.cache.iter().any(|&c| c))
        {
            return Err("R1: a dead node's device still translated");
        }
        let inside = (0..2)
            .filter(|&c| matches!(self.cores[c], Pc::Attach(1..) | Pc::Release(1..)))
            .count();
        if v.lock && self.member == Member::Live && inside > 1 {
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
    /// A state from which nothing moves, with a core of a live *X* still in
    /// its program.
    pub(crate) wedge: Option<Vec<Step>>,
}

pub(crate) fn explore(v: Variant) -> Findings {
    let mut seen = BTreeSet::new();
    let mut findings = Findings::default();
    let mut stack = vec![(State::new(), Vec::new())];
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
        let pending = state.member == Member::Live
            && state
                .cores
                .iter()
                .any(|pc| matches!(pc, Pc::Attach(_) | Pc::Release(_)));
        if moving.is_empty() && pending && findings.wedge.is_none() {
            findings.wedge = Some(path.clone());
        }
        for step in steps {
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

    // D-168 (3): the seal's park call acquires the gate lock, writes
    // `seal_parked`, and releases the gate lock before stopping the core
    // without returning. If the lock were left held, the resumed node's first
    // gate call would spin forever on the lock.
    #[test]
    fn seal_park_releases_the_gate_lock_before_stopping_the_core() {
        let run = |release_on_park: bool| -> Findings {
            // Core 0 entered the seal's park call (taking the gate lock),
            // wrote `seal_parked`, and parked; if `release_on_park` is true
            // (D-168), it released the lock before stopping. The node then
            // resumed and core 1 runs its first gate call (core 0 already
            // done).
            let mut start = State::new();
            start.lock = if release_on_park { None } else { Some(0) };
            start.cores = [Pc::Done, Pc::Attach(0)];
            let v = Variant { lock: true };
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
                let pending = state.member == Member::Live
                    && state
                        .cores
                        .iter()
                        .any(|pc| matches!(pc, Pc::Attach(_) | Pc::Release(_)));
                if moving.is_empty() && pending && findings.wedge.is_none() {
                    findings.wedge = Some(path.clone());
                }
                for step in steps {
                    let mut next = state.clone();
                    next.take(step, v);
                    let mut p = path.clone();
                    p.push(step);
                    stack.push((next, p));
                }
            }
            findings
        };

        let ok = run(true);
        assert_eq!(ok.broken, None, "{ok:?}");
        assert_eq!(ok.wedge, None, "{ok:?}");

        let wedged = run(false);
        assert!(
            wedged.wedge.is_some(),
            "leaving the gate lock held in park must wedge the resumed node's first gate call"
        );
    }
}
