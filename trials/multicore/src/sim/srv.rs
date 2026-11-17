//! Workload srv: a passive server, which runs on the budget of its caller,
//! shared by a background client that makes long requests and a foreground
//! client that makes short, latency-sensitive ones (README, "Fourth series").
//!
//! The endpoint lives on the node of server thread 0; in candidate 1 a server
//! thread on another node hears from it by message. A budget travels with
//! the call: a grant is what the caller has left in its current period, and
//! the reply hands back what is left, so no node reads another node's state.

use super::{CALL, Core, DISPATCH, FREE, Held, LEND, LINES, Machine, Mode, Policy, REPLY, Role};
use crate::platform;
use crate::rt::{Duration, Instant};
use crate::sync::{MAX_CORES, Msg, busy};
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

/// A foreground request, and a background one.
const SHORT_NS: u64 = 5_000;
const LONG_NS: u64 = 50_000;
/// Every client's budget period, and its budget in each.
const PERIOD_NS: u64 = 250_000;
const FG_BUDGET_NS: u64 = 100_000;
const BG_BUDGET_NS: u64 = 120_000;
/// The foreground client calls every half to 1.5 times this, at random.
const FG_EVERY_NS: u64 = 250_000;
/// User time of the background client between two calls.
const BG_GAP_NS: u64 = 2_000;
/// A server thread works in chunks, and looks for a lent context and at its
/// budget between them.
const CHUNK_NS: u64 = 1_000;
const WINDOW_MASK: u64 = 0xFF_FFFF;

/// A caller's budget left in one of its periods, carried by its call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Grant {
    caller: usize,
    window: u64,
    rem: u64,
}

impl Grant {
    /// A grant that the first refresh fills.
    fn fresh(caller: usize) -> Grant {
        Grant {
            caller,
            window: u64::MAX,
            rem: 0,
        }
    }

    /// One word: caller, period (24 bits), budget left (32 bits). Never 0,
    /// because clients are not on core 0.
    fn pack(self) -> u64 {
        self.caller as u64 | (self.window & WINDOW_MASK) << 8 | self.rem << 32
    }

    fn unpack(w: u64) -> Grant {
        Grant {
            caller: (w & 0xFF) as usize,
            window: (w >> 8) & WINDOW_MASK,
            rem: w >> 32,
        }
    }
}

impl Machine {
    /// The current period of `caller`; the clients' periods are a third of
    /// a period apart.
    fn window(&self, caller: usize) -> u64 {
        let phase = if caller == self.threads {
            PERIOD_NS / 3
        } else {
            0
        };
        ((self.t0.elapsed().as_nanos() as u64 + phase) / PERIOD_NS) & WINDOW_MASK
    }

    /// A new period gives the whole budget back.
    fn refresh(&self, g: &mut Grant) {
        let w = self.window(g.caller);
        if w != g.window {
            g.window = w;
            g.rem = if g.caller == self.threads {
                FG_BUDGET_NS
            } else {
                BG_BUDGET_NS
            };
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Serving {
    /// The class of the context the thread runs on: 1 foreground.
    class: u8,
    /// The queued caller whose context is lent to the thread.
    lender: Option<usize>,
}

/// The endpoint: free server threads, callers in order, and what each
/// thread serves. Kernel state of the endpoint's node, under its lock.
pub struct Ep {
    fg: usize,
    free: u64,
    queue: VecDeque<Grant>,
    serving: Vec<Serving>,
}

/// What the endpoint asks of a server thread.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Dispatch(usize, Grant),
    Lend(usize, Grant),
}

/// What a server thread does after a reply.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Serve(Grant),
    /// Serve the caller whose context it was lent (now or on its way).
    Lender,
    Idle,
}

impl Ep {
    pub fn new(threads: usize) -> Ep {
        Ep {
            fg: threads,
            free: (1 << threads) - 1,
            queue: VecDeque::with_capacity(MAX_CORES),
            serving: vec![Serving::default(); threads],
        }
    }

    fn class(&self, caller: usize) -> u8 {
        u8::from(caller == self.fg)
    }

    fn call(&mut self, g: Grant, policy: Policy) -> Option<Action> {
        let class = self.class(g.caller);
        if self.free != 0 {
            let j = self.free.trailing_zeros() as usize;
            self.free &= !(1 << j);
            self.serving[j] = Serving {
                class,
                lender: None,
            };
            return Some(Action::Dispatch(j, g));
        }
        self.queue.push_back(g);
        if policy != Policy::Lend {
            return None;
        }
        let j = self
            .serving
            .iter()
            .position(|s| s.lender.is_none() && s.class < class)?;
        self.serving[j] = Serving {
            class,
            lender: Some(g.caller),
        };
        Some(Action::Lend(j, g))
    }

    /// Thread `j` has replied. One that ran on a lent context serves its
    /// lender next; no other thread takes a caller whose context is lent,
    /// which would then run in two places.
    fn finish(&mut self, j: usize) -> Next {
        if let Some(l) = self.serving[j].lender.take() {
            self.queue.retain(|g| g.caller != l);
            return Next::Lender;
        }
        let lent = |c| self.serving.iter().any(|s| s.lender == Some(c));
        if let Some(i) = self.queue.iter().position(|g| !lent(g.caller)) {
            let g = self.queue.remove(i).expect("position is in the queue");
            self.serving[j] = Serving {
                class: self.class(g.caller),
                lender: None,
            };
            return Next::Serve(g);
        }
        self.free |= 1 << j;
        Next::Idle
    }

    /// After a run: no caller left waiting, every thread free.
    pub fn check_idle(&self) {
        assert!(self.queue.is_empty(), "srv: a caller was never served");
        assert_eq!(self.free.count_ones() as usize, self.serving.len());
    }
}

impl Core<'_> {
    /// `f` on the endpoint, under its lock, with half a kernel operation on
    /// its lines. `Held::Yes` in a message handler: the node lock is held
    /// (node lock, then endpoint lock). Nothing is sent under the endpoint
    /// lock: `f` returns what to send, and `act` sends it after.
    fn with_ep<R>(&mut self, held: Held, f: impl FnOnce(&mut Ep) -> R) -> R {
        let m = self.m;
        let _g = match held {
            Held::Yes => self.fine().then(|| m.obj_locks[0].lock()),
            Held::No => Some(self.obj_lock(0)),
        };
        self.obj_work(0, LINES / 2, m.p.work_ns / 2);
        let mut ep = m.ep.try_lock().expect("two cores in the endpoint");
        f(&mut ep)
    }

    fn act(&mut self, a: Option<Action>, held: Held) {
        match a {
            None => {}
            Some(Action::Dispatch(j, g)) => self.to(DISPATCH, j, g, held),
            Some(Action::Lend(j, g)) => self.to(LEND, j, g, held),
        }
    }

    /// Sends a request, a lent context or a reply to core `j`: shared
    /// memory on this node, a message to another.
    fn to(&mut self, kind: u64, j: usize, g: Grant, held: Held) {
        let m = self.m;
        let node = m.node_of(j);
        if node == self.node {
            return self.arrive(kind, j, g.pack());
        }
        let msg = Msg {
            kind,
            a: j as u64,
            b: g.pack(),
        };
        match held {
            Held::Yes => self.push(node, msg),
            Held::No => self.send(node, msg),
        }
    }

    fn arrive(&self, kind: u64, j: usize, w: u64) {
        let m = self.m;
        match kind {
            LEND => m.lent[j].store(w, Ordering::Release),
            DISPATCH => {
                m.disp[j].store(w, Ordering::Relaxed);
                m.mbox_ev[j].fetch_or(1, Ordering::Release);
            }
            _ => {
                m.reply[j].store(w, Ordering::Relaxed);
                m.mbox_ev[j].fetch_or(1, Ordering::Release);
            }
        }
    }

    /// A message of srv; the node lock is held.
    pub(super) fn srv_handle(&mut self, msg: Msg) {
        let policy = self.m.p.policy;
        match msg.kind {
            CALL => {
                let g = Grant::unpack(msg.a);
                let a = self.with_ep(Held::Yes, |ep| ep.call(g, policy));
                self.act(a, Held::Yes);
            }
            FREE => {
                let j = msg.a as usize;
                if let Next::Serve(g) = self.with_ep(Held::Yes, |ep| ep.finish(j)) {
                    self.act(Some(Action::Dispatch(j, g)), Held::Yes);
                }
            }
            kind => self.arrive(kind, msg.a as usize, msg.b),
        }
    }

    pub(super) fn srv_thread(&mut self) {
        let m = self.m;
        let mut job = self.wait_job();
        while let Some(g) = job {
            busy(m.p.work_ns / 2);
            let (own, lent) = self.serve(g);
            busy(m.p.work_ns / 2);
            self.to(REPLY, own.caller, own, Held::No);
            job = self.next_job(lent);
        }
    }

    /// Runs a request on its caller's grant, in chunks. Between chunks it
    /// takes a context lent to this thread and runs on it from then on; on
    /// a spent grant it stalls (still taking interrupts) until the grant's
    /// next period or a lend. A request is a length of wall time on the
    /// core, and a grant is charged the wall time of each chunk, interrupts
    /// included, as a kernel charges the context that holds the core; so
    /// the refusal threshold (the longest request) is in the same unit as
    /// the charge. Returns the caller's grant and the lent one.
    fn serve(&mut self, mut own: Grant) -> (Grant, Option<Grant>) {
        let m = self.m;
        let mut left = if own.caller == m.threads {
            SHORT_NS
        } else {
            LONG_NS
        };
        let mut lent = None;
        while left > 0 {
            if lent.is_none() {
                lent = self.take_lent();
            }
            let g = lent.as_mut().unwrap_or(&mut own);
            m.refresh(g);
            let c = left.min(g.rem).min(CHUNK_NS);
            if c == 0 {
                self.user(CHUNK_NS);
                continue;
            }
            let t = Instant::now();
            self.user(c);
            let spent = t.elapsed().as_nanos() as u64;
            g.rem = g.rem.saturating_sub(spent);
            left = left.saturating_sub(spent);
        }
        (own, lent)
    }

    fn take_lent(&self) -> Option<Grant> {
        let slot = &self.m.lent[self.id];
        if slot.load(Ordering::Relaxed) == 0 {
            return None;
        }
        Some(Grant::unpack(slot.swap(0, Ordering::Acquire)))
    }

    /// After a reply: tell the endpoint; serve what it hands back, or the
    /// lender whose context this thread holds (or is about to), or wait.
    fn next_job(&mut self, lent: Option<Grant>) -> Option<Grant> {
        let m = self.m;
        let me = self.id;
        if m.node_of(0) != self.node {
            let msg = Msg {
                kind: FREE,
                a: me as u64,
                b: 0,
            };
            self.send(m.node_of(0), msg);
            return lent.or_else(|| self.wait_job());
        }
        match self.with_ep(Held::No, |ep| ep.finish(me)) {
            Next::Serve(g) => {
                assert!(lent.is_none(), "a lent context left behind");
                Some(g)
            }
            Next::Lender => lent.or_else(|| self.wait_job()),
            Next::Idle => {
                assert!(lent.is_none(), "a lent context left behind");
                self.wait_job()
            }
        }
    }

    /// Idles until a request or a lent context reaches this thread; `None`
    /// once both clients are done (none is then waiting for a reply).
    fn wait_job(&mut self) -> Option<Grant> {
        let m = self.m;
        let fg = m.threads;
        let mut charged = false;
        loop {
            charged |= self.poll(Mode::Interruptible);
            let job = if m.mbox_ev[self.id].swap(0, Ordering::Acquire) != 0 {
                Some(Grant::unpack(m.disp[self.id].swap(0, Ordering::Relaxed)))
            } else {
                self.take_lent()
            };
            if job.is_some() {
                if !charged {
                    platform::interrupt(m.p.ipi_ns);
                }
                return job;
            }
            let bg_done = m.roles[fg + 1] != Role::Bg || m.bye[fg + 1].load(Ordering::Acquire);
            if bg_done && m.bye[fg].load(Ordering::Acquire) {
                return None;
            }
        }
    }

    /// One call on the grant `g`, which the reply updates; false if the
    /// kernel refused it (its budget left is below the longest request).
    fn srv_call(&mut self, g: &mut Grant) -> bool {
        let m = self.m;
        let policy = m.p.policy;
        m.refresh(g);
        if policy == Policy::Refuse && g.rem < LONG_NS {
            busy(m.p.work_ns / 2);
            return false;
        }
        let home = m.node_of(0);
        if home == self.node {
            let call = *g;
            let a = self.with_ep(Held::No, |ep| ep.call(call, policy));
            self.act(a, Held::No);
        } else {
            busy(m.p.work_ns / 2);
            let msg = Msg {
                kind: CALL,
                a: g.pack(),
                b: 0,
            };
            self.send(home, msg);
        }
        self.wait_event(|_| false);
        *g = Grant::unpack(m.reply[self.id].load(Ordering::Relaxed));
        busy(m.p.work_ns / 2);
        true
    }

    /// Class 0: its calls; class 2: calls refused.
    pub(super) fn srv_fg(&mut self) {
        let m = self.m;
        let mut g = Grant::fresh(self.id);
        let mut next = Instant::now();
        while self.running() {
            let every = FG_EVERY_NS / 2 + self.rand() % FG_EVERY_NS;
            next = next.max(Instant::now()) + Duration::from_nanos(every);
            self.user(next.saturating_duration_since(Instant::now()).as_nanos() as u64);
            let t = Instant::now();
            let done = self.srv_call(&mut g);
            self.record(if done { 0 } else { 2 }, t);
        }
        m.bye[self.id].store(true, Ordering::Release);
    }

    /// Class 1: its calls; class 2: calls refused, after which it waits for
    /// its next period.
    pub(super) fn srv_bg(&mut self) {
        let m = self.m;
        let mut g = Grant::fresh(self.id);
        while self.running() {
            self.user(BG_GAP_NS);
            let t = Instant::now();
            if self.srv_call(&mut g) {
                self.record(1, t);
                continue;
            }
            self.record(2, t);
            while m.window(self.id) == g.window {
                self.user(CHUNK_NS);
            }
        }
        m.bye[self.id].store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Ep, Grant, Next, Policy};

    fn grant(caller: usize) -> Grant {
        Grant {
            caller,
            window: 3,
            rem: 1000,
        }
    }

    /// Two threads, the foreground client is core 2: a background call
    /// takes thread 0, a foreground call thread 1; a second background call
    /// queues; with lending, a second foreground call lends its context to
    /// thread 0 (the one at the lower class), which serves it next even
    /// though thread 1 finishes first and the background call is ahead.
    #[test]
    fn a_lender_is_served_by_its_borrower_next() {
        let mut ep = Ep::new(2);
        let (fg, bg) = (grant(2), grant(3));
        assert_eq!(ep.call(bg, Policy::Lend), Some(Action::Dispatch(0, bg)));
        assert_eq!(ep.call(fg, Policy::Lend), Some(Action::Dispatch(1, fg)));
        assert_eq!(ep.call(bg, Policy::Lend), None);
        assert_eq!(ep.call(fg, Policy::Lend), Some(Action::Lend(0, fg)));
        assert_eq!(ep.finish(1), Next::Serve(bg));
        assert_eq!(ep.finish(0), Next::Lender);
        assert_eq!(ep.finish(1), Next::Idle);
        assert_eq!(ep.finish(0), Next::Idle);
        ep.check_idle();
        assert_eq!(Grant::unpack(fg.pack()), fg);
    }
}
