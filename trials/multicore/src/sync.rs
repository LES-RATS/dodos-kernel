//! The synchronisation primitives under test: cache-line padding, the three
//! locks a group kernel could use, the ring between kernels, and the
//! busy wait that stands for kernel work.

use crate::rt::{Duration, Instant};
use alloc::boxed::Box;
use core::hint::{black_box, spin_loop};
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// One value alone in its cache line. 128 bytes: the line of Apple cores, and
/// two lines on x86-64, whose adjacent-line prefetcher pairs lines anyway.
#[repr(align(128))]
#[derive(Default)]
pub struct Padded<T>(pub T);

impl<T> Deref for Padded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Busy for `ns` of wall-clock time: the compute part of kernel work, user
/// work between calls, an interrupt entry, a TLB invalidation. It runs to a
/// deadline read from the clock, not a calibrated number of iterations: a
/// calibrated loop ran up to 12 % faster or slower depending on the code
/// around it (seen on the M5), which changed the offered load between
/// candidates. The cost is the clock's granularity: on Apple chips 41.7 ns,
/// so a 50 ns wait lasts between 50 and about 90 ns.
pub fn busy(ns: u64) {
    if ns == 0 {
        return;
    }
    let end = Instant::now() + Duration::from_nanos(ns);
    while Instant::now() < end {
        // A few dependent multiplications that touch no memory.
        let mut x = black_box(1u64);
        for _ in 0..4 {
            x = black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
        }
    }
}

/// The lock of a group kernel. `None` is the node of one core, which needs
/// none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockKind {
    None,
    /// Test-and-test-and-set with exponential backoff: cheap, unfair.
    Ttas,
    /// Ticket lock: fair (first come, first served), every waiter spins on one
    /// line.
    Ticket,
    /// Array queue lock (Anderson, 1990): fair, each waiter spins on its own
    /// line, as an MCS lock does, without pointers.
    Queue,
}

impl LockKind {
    pub fn name(self) -> &'static str {
        match self {
            LockKind::None => "none",
            LockKind::Ttas => "ttas",
            LockKind::Ticket => "ticket",
            LockKind::Queue => "queue",
        }
    }
}

/// Slots of the queue lock: at least the number of cores that can contend.
pub const MAX_CORES: usize = 64;

pub enum Lock {
    None,
    Ttas(Padded<AtomicBool>),
    Ticket {
        next: Padded<AtomicU32>,
        serving: Padded<AtomicU32>,
    },
    Queue {
        tail: Padded<AtomicU32>,
        slots: Box<[Padded<AtomicBool>]>,
    },
}

/// Held while the lock is held; the token is the queue slot to hand over.
pub struct Guard<'a> {
    lock: &'a Lock,
    token: u32,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        match self.lock {
            Lock::None => {}
            Lock::Ttas(held) => held.store(false, Ordering::Release),
            Lock::Ticket { serving, .. } => {
                serving.store(self.token.wrapping_add(1), Ordering::Release);
            }
            Lock::Queue { slots, .. } => {
                let next = (self.token as usize + 1) % MAX_CORES;
                slots[next].store(true, Ordering::Release);
            }
        }
    }
}

impl Lock {
    pub fn new(kind: LockKind) -> Lock {
        match kind {
            LockKind::None => Lock::None,
            LockKind::Ttas => Lock::Ttas(Padded::default()),
            LockKind::Ticket => Lock::Ticket {
                next: Padded::default(),
                serving: Padded::default(),
            },
            LockKind::Queue => Lock::Queue {
                tail: Padded::default(),
                slots: (0..MAX_CORES)
                    .map(|i| Padded(AtomicBool::new(i == 0)))
                    .collect(),
            },
        }
    }

    pub fn lock(&self) -> Guard<'_> {
        let token = match self {
            Lock::None => 0,
            Lock::Ttas(held) => {
                let mut backoff = 1u32;
                loop {
                    if !held.load(Ordering::Relaxed) && !held.swap(true, Ordering::Acquire) {
                        break 0;
                    }
                    for _ in 0..backoff {
                        spin_loop();
                    }
                    backoff = (backoff * 2).min(256);
                }
            }
            Lock::Ticket { next, serving } => {
                let mine = next.fetch_add(1, Ordering::Relaxed);
                while serving.load(Ordering::Acquire) != mine {
                    spin_loop();
                }
                mine
            }
            Lock::Queue { tail, slots } => {
                // MAX_CORES divides 2^32, so the wrap of `tail` keeps the order.
                let mine = tail.fetch_add(1, Ordering::Relaxed) % MAX_CORES as u32;
                let slot = &slots[mine as usize];
                while !slot.load(Ordering::Acquire) {
                    spin_loop();
                }
                slot.store(false, Ordering::Relaxed);
                mine
            }
        };
        Guard { lock: self, token }
    }
}

/// The bit of a notification word that says its thread waits (or is
/// registering to wait); the other 63 bits are signals.
const WAITING: u64 = 1 << 63;

/// What a signal had to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signalled {
    /// One atomic OR: nobody was waiting.
    Fast,
    /// The lock was taken and the waiter must now be woken by the caller.
    Wake,
    /// The lock was taken, but the waiter had already been woken, or had
    /// found the bits itself: nothing to wake.
    Slow,
}

/// A notification object whose signal is one atomic OR when its thread is
/// not waiting; only a signal that finds it waiting takes the lock.
///
/// No wake-up is lost. Every change of `word` is an atomic read-modify-write,
/// so they all fall in one total order (the modification order of `word`).
/// The waiter registers under `lock`: it sets WAITING with a fetch-or and
/// looks at the bits that same operation returns. A signal's fetch-or is
/// either before the waiter's in that order, and then the waiter sees the
/// bit and does not sleep; or after it, and then the signal sees WAITING and
/// takes `lock`, which it gets only after the waiter has either registered
/// (`waiter` set) or taken the bits and cleared WAITING, and released it; in
/// the first case the signal wakes it. WAITING is set only by the waiter
/// under `lock`, and cleared only under `lock` (by the signal that wakes it,
/// or by the waiter when it found bits), so a signal that sees it always
/// finds one of those two states once it holds the lock.
///
/// A wake-up may find no bit: two signals see WAITING, the first wakes the
/// waiter, which takes every bit and registers again before the second gets
/// the lock; the second then wakes it once more. The waiter treats `take`
/// returning 0 as "wait again".
pub struct Notify {
    word: Padded<AtomicU64>,
    lock: Lock,
    /// Whether the thread is registered as asleep; only under `lock`.
    waiter: Padded<AtomicBool>,
}

impl Notify {
    pub fn new(kind: LockKind) -> Notify {
        Notify {
            word: Padded::default(),
            lock: Lock::new(kind),
            waiter: Padded::default(),
        }
    }

    /// Sets `bits` (not bit 63).
    pub fn signal(&self, bits: u64) -> Signalled {
        debug_assert!(bits != 0 && bits & WAITING == 0);
        if self.word.fetch_or(bits, Ordering::AcqRel) & WAITING == 0 {
            return Signalled::Fast;
        }
        let _g = self.lock.lock();
        if !self.waiter.load(Ordering::Relaxed) {
            return Signalled::Slow;
        }
        self.waiter.store(false, Ordering::Relaxed);
        self.word.fetch_and(!WAITING, Ordering::Relaxed);
        Signalled::Wake
    }

    /// The pending bits; or 0 after registering as asleep, and the caller
    /// sleeps until a `Wake` reaches it, then calls `take`.
    pub fn take_or_wait(&self) -> u64 {
        if self.word.load(Ordering::Relaxed) != 0 {
            let got = self.take();
            if got != 0 {
                return got;
            }
        }
        let _g = self.lock.lock();
        if self.word.fetch_or(WAITING, Ordering::AcqRel) != 0 {
            return self.take();
        }
        self.waiter.store(true, Ordering::Relaxed);
        0
    }

    /// Takes the pending bits (and clears WAITING if the caller had set it).
    pub fn take(&self) -> u64 {
        self.word.swap(0, Ordering::Acquire) & !WAITING
    }
}

/// A message between kernels: a kind and two words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Msg {
    pub kind: u64,
    pub a: u64,
    pub b: u64,
}

#[repr(align(128))]
struct Slot {
    seq: AtomicU64,
    words: [AtomicU64; 3],
}

/// Slots per ring. Synchronous traffic between two nodes is bounded far below
/// this (see `sim.rs`); only asynchronous signals can fill a ring.
pub const RING_SLOTS: u64 = 128;

/// A one-producer, one-consumer ring, one per ordered pair of nodes, as the
/// per-pair channels of Barrelfish (from memory). A message is one cache
/// line, published by its sequence word, so a delivery moves one line. The
/// consumer publishes its position on its own line; the producer reads it
/// only when its cached copy says the ring is full. The node lock gives
/// one producer and one consumer when a node has several cores.
pub struct Ring {
    slots: Box<[Slot]>,
    head: Padded<AtomicU64>,
    tail: Padded<(AtomicU64, AtomicU64)>,
}

impl Default for Ring {
    fn default() -> Ring {
        Ring {
            slots: (0..RING_SLOTS)
                .map(|_| Slot {
                    seq: AtomicU64::new(0),
                    words: Default::default(),
                })
                .collect(),
            head: Padded::default(),
            tail: Padded::default(),
        }
    }
}

impl Ring {
    /// Producer side. Returns false if the ring stayed full for `patience`.
    pub fn push(&self, m: Msg, patience: Duration) -> bool {
        let (tail, cached_head) = &self.tail.0;
        let t = tail.load(Ordering::Relaxed);
        if t - cached_head.load(Ordering::Relaxed) >= RING_SLOTS {
            let start = Instant::now();
            loop {
                let h = self.head.load(Ordering::Acquire);
                cached_head.store(h, Ordering::Relaxed);
                if t - h < RING_SLOTS {
                    break;
                }
                if start.elapsed() > patience {
                    return false;
                }
                spin_loop();
            }
        }
        let slot = &self.slots[(t % RING_SLOTS) as usize];
        slot.words[0].store(m.kind, Ordering::Relaxed);
        slot.words[1].store(m.a, Ordering::Relaxed);
        slot.words[2].store(m.b, Ordering::Relaxed);
        slot.seq.store(t + 1, Ordering::Release);
        tail.store(t + 1, Ordering::Relaxed);
        true
    }

    /// A hint, readable by any core of the consumer node without the lock.
    pub fn ready(&self) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        self.slots[(h % RING_SLOTS) as usize]
            .seq
            .load(Ordering::Relaxed)
            == h + 1
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<Msg> {
        let h = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[(h % RING_SLOTS) as usize];
        if slot.seq.load(Ordering::Acquire) != h + 1 {
            return None;
        }
        let m = Msg {
            kind: slot.words[0].load(Ordering::Relaxed),
            a: slot.words[1].load(Ordering::Relaxed),
            b: slot.words[2].load(Ordering::Relaxed),
        };
        self.head.store(h + 1, Ordering::Release);
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::{Lock, LockKind, Msg, Notify, Padded, RING_SLOTS, Ring, Signalled};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    /// Panics if `done` stays false for 10 s: a lost wake-up hangs, it
    /// does not fail, so the test turns the hang into a failure.
    fn spin_until(what: &str, done: impl Fn() -> bool) {
        let since = Instant::now();
        while !done() {
            assert!(since.elapsed() < Duration::from_secs(10), "{what}");
            std::hint::spin_loop();
        }
    }

    /// Three producers each signal and wait until the waiter has seen that
    /// signal, so the waiter keeps going to sleep while signals race with
    /// its registration. A lost wake-up leaves a signal unseen.
    #[test]
    fn notify_loses_no_wake_up() {
        let rounds = if cfg!(miri) { 50 } else { 20_000 };
        let n = Notify::new(LockKind::Queue);
        let woken = AtomicBool::new(false);
        let seen: [AtomicU64; 3] = Default::default();
        let wakes = AtomicU64::new(0);
        let fasts = AtomicU64::new(0);
        thread::scope(|s| {
            for (p, mine) in seen.iter().enumerate() {
                let (n, woken, wakes, fasts) = (&n, &woken, &wakes, &fasts);
                s.spawn(move || {
                    for i in 0..rounds {
                        match n.signal(1 << p) {
                            Signalled::Wake => {
                                wakes.fetch_add(1, Ordering::Relaxed);
                                woken.store(true, Ordering::Release);
                            }
                            Signalled::Fast => _ = fasts.fetch_add(1, Ordering::Relaxed),
                            Signalled::Slow => {}
                        }
                        spin_until("signal never seen: lost wake-up", || {
                            mine.load(Ordering::Acquire) > i
                        });
                    }
                });
            }
            while seen.iter().any(|c| c.load(Ordering::Relaxed) < rounds) {
                let mut got = n.take_or_wait();
                if got == 0 {
                    spin_until("asleep with a signal pending: lost wake-up", || {
                        woken.swap(false, Ordering::Acquire)
                    });
                    got = n.take();
                }
                for (p, c) in seen.iter().enumerate() {
                    if got & (1 << p) != 0 {
                        c.fetch_add(1, Ordering::Release);
                    }
                }
            }
        });
        assert!(wakes.load(Ordering::Relaxed) > 0, "the slow path ran");
        assert!(fasts.load(Ordering::Relaxed) > 0, "the fast path ran");
    }

    /// A plain read then write, not an atomic add: a lost update shows a
    /// broken lock.
    fn hammer(kind: LockKind) -> u64 {
        let lock = Lock::new(kind);
        let count = Padded(AtomicU64::new(0));
        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..200 {
                        let _g = lock.lock();
                        let v = count.load(Ordering::Relaxed);
                        count.store(v + 1, Ordering::Relaxed);
                    }
                });
            }
        });
        count.load(Ordering::Relaxed)
    }

    #[test]
    fn every_lock_excludes() {
        for kind in [LockKind::Ttas, LockKind::Ticket, LockKind::Queue] {
            assert_eq!(hammer(kind), 800, "{kind:?}");
        }
    }

    #[test]
    fn ring_delivers_in_order_through_a_full_ring() {
        let ring = Ring::default();
        let n = RING_SLOTS * 3;
        thread::scope(|s| {
            s.spawn(|| {
                for i in 0..n {
                    let m = Msg {
                        kind: 1,
                        a: i,
                        b: !i,
                    };
                    assert!(ring.push(m, Duration::from_secs(60)));
                }
            });
            let mut next = 0;
            while next < n {
                if let Some(m) = ring.pop() {
                    assert_eq!(
                        m,
                        Msg {
                            kind: 1,
                            a: next,
                            b: !next
                        }
                    );
                    next += 1;
                }
            }
        });
        assert!(ring.pop().is_none());
    }

    #[test]
    fn full_ring_refuses_after_patience() {
        let ring = Ring::default();
        let m = Msg {
            kind: 0,
            a: 0,
            b: 0,
        };
        for _ in 0..RING_SLOTS {
            assert!(ring.push(m, Duration::ZERO));
        }
        assert!(!ring.push(m, Duration::ZERO));
    }
}
