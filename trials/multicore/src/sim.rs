//! The simulated kernels. One worker thread stands for one core. A node is a
//! block of kernel state with its lock (none on a node of one core) and its
//! inbound rings, one per other node. The three candidates differ only in how
//! cores are grouped into nodes:
//!
//! - candidate 1: one core per node, no lock, everything between cores by
//!   messages;
//! - candidate 2: fixed groups of `g` cores per node, behind the node lock;
//! - candidate 3: the cores of the multithreaded process form one node, the
//!   other cores one node each.
//!
//! So the same code runs all three, and what differs is measured, not
//! written twice. The cost model is in the README.

mod srv;

use crate::hist::Hist;
use crate::platform;
use crate::rt::{self, Barrier, Duration, Instant, Mutex};
use crate::sync::{Guard, LockKind, MAX_CORES, Msg, Notify, Padded, Ring, Signalled, busy};
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::{format, string::String, vec};
use core::hint::black_box;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering, fence};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cand {
    PerCore,
    Fixed(usize),
    Sized,
}

impl Cand {
    pub fn name(self) -> String {
        match self {
            Cand::PerCore => "1 per-core".into(),
            Cand::Fixed(g) => format!("2 fixed/{g}"),
            Cand::Sized => "3 sized".into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Compute inside one kernel operation (the critical section).
    pub work_ns: u64,
    /// User time between two kernel calls of a thread.
    pub gap_ns: u64,
    /// Interrupt entry and exit, paid by a core that a message or a wake-up
    /// reaches while it runs user code or idles.
    pub ipi_ns: u64,
    /// Local TLB invalidation of one range.
    pub tlb_ns: u64,
    /// Switch of a core from one node to another: kernel mapping and TLB.
    pub switch_ns: u64,
    /// Percentage of capability operations that change the capability space.
    pub change_pct: u64,
    pub lock: LockKind,
    /// How a lookup reads the capability space.
    pub lookup: Lookup,
    /// Whether a change that only adds waits for every copy (candidate 1, and
    /// candidate 2 between groups).
    pub adds: Adds,
    /// Cost of applying a change to a copy other than the home one, in
    /// percent of `work_ns`.
    pub replica_pct: u64,
    /// How interrupts are charged to a core reached while in user code.
    pub irq: Irq,
    /// Which lock a write takes in a group of several cores.
    pub locks: Locks,
    /// How a signal applies its bits to the notification object.
    pub signals: Signals,
    /// Which lock an unmap takes in a group of several cores.
    pub unmaps: Unmaps,
    /// What the kernel does for a caller of a busy passive server (srv).
    pub policy: Policy,
    /// User time of the thread waiting on the notify after each wake-up
    /// (a3n): it sets how often that thread is found waiting.
    pub rx_ns: u64,
    pub warmup: Duration,
    pub dur: Duration,
}

/// The read path of a lookup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup {
    /// Under the node lock, as every other operation.
    Locked,
    /// No lock: a sequence counter per capability space, which writers make
    /// odd while they change it; a reader that sees it odd, or changed when
    /// it has finished, reads again. The read section is the whole `W`.
    Seqlock,
    /// No lock, and the read section is one slot: a sequence counter per
    /// slot range; the rest of the call's `W` runs after the read.
    Slot,
}

/// The locks of a group kernel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Locks {
    /// One lock per group for every write.
    Group,
    /// A lock per endpoint or notification object for IPC and signals, and
    /// per slot range of a capability space for changes inside one group.
    /// Messages between groups, and every unmap, keep the group lock.
    Fine,
}

/// How a signal applies its bits (fourth series).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signals {
    /// `W/2` and two lines under the notify's lock (or the group lock), or
    /// on the notify's node when the message arrives.
    Locked,
    /// One atomic OR on the notify's word; the lock only when its thread
    /// waits (`sync::Notify`). Candidate 1 applies the message with that OR.
    Atomic,
}

/// The lock of an unmap in a group (fourth series).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unmaps {
    Group,
    /// The lock of the one range of the address space (of 8) it changes.
    /// Legal only under the conditions in the README.
    Range,
}

/// What the kernel does when a passive server is busy (workload srv).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    /// Nothing: callers queue in order.
    Fifo,
    /// A queued caller of a higher class lends its context (budget and
    /// class) to the thread serving a lower one, which finishes on it and
    /// serves the lender next.
    Lend,
    /// A call whose remaining budget is below the endpoint's longest
    /// request is refused at once.
    Refuse,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Adds {
    /// Every change waits for every copy's acknowledgement.
    Eager,
    /// A derive waits only for its own node's copy (ordered by the home
    /// node); a delete still waits for every copy.
    Lazy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Irq {
    /// One interrupt delivers every message already there.
    PerBatch,
    /// One interrupt per message (the pessimistic bound).
    PerMessage,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    /// a: capability lookups and changes from k threads of one process.
    CapOps,
    /// a: IPC call and reply, k/2 client-server pairs of one process.
    Ipc,
    /// a: k-1 threads signal one notify that one thread waits on.
    Signal,
    /// b: unmap with TLB invalidation; one initiator or all k.
    Unmap(Initiators),
    /// c: the k-thread process of `CapOps` beside `n - k` independent
    /// single-thread processes that only look up their own capabilities.
    Mixed,
    /// d: groups A and B, and one core that moves between them `rate` times
    /// per second (candidate 3 only).
    Move { rate: u64 },
    /// srv: a passive server of `threads` threads on cores `0..threads`, a
    /// foreground client on the next core, a background one after it.
    Server { threads: usize, bg: Bg },
}

/// Whether the background client of srv runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bg {
    On,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Initiators {
    One,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    CapOps,
    Lookups,
    Client(usize),
    Server(usize),
    Producer(usize),
    Waiter,
    Unmapper,
    User,
    Mobile(u64),
    Grouped,
    SrvThread,
    Fg,
    Bg,
}

/// Latency classes. Their meaning depends on the workload (README).
pub const CLASSES: usize = 4;
/// Lines of node state touched by a full operation (half by a half one).
const LINES: usize = 4;
/// Lines of node state per process, and processes whose lines are distinct.
const PROC_LINES: usize = 16;
const PROC_REGIONS: usize = 4;
/// Slot ranges of a capability space: each owns 2 of its 16 lines.
const STRIPES: usize = 8;
/// Lines of an endpoint or notification object.
const OBJ_LINES: usize = 4;
/// A user gap is cut in slices, with a poll (an interrupt) between slices.
const SLICE_NS: u64 = 100;
/// A wait longer than this is a bug of the simulation, never a result.
const STUCK: Duration = Duration::from_secs(10);
/// An operation at least this long is counted as an OS stall: a preemption of
/// the worker thread, which no kernel would suffer.
pub const STALL_NS: u64 = 500_000;

const WARMUP: u8 = 0;
const MEASURE: u8 = 1;
const STOP: u8 = 2;

const REQ: u64 = 1;
const APPLY: u64 = 2;
const ACK: u64 = 3;
const EVENT: u64 = 4;
const SIGNAL: u64 = 5;
/// srv: a call to the endpoint, a request handed to a server thread, a
/// context lent to one, a server thread free again, a reply.
const CALL: u64 = 6;
const DISPATCH: u64 = 7;
const LEND: u64 = 8;
const FREE: u64 = 9;
const REPLY: u64 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Capability delete: removes rights.
    CapRemove = 0,
    Unmap = 1,
    /// Capability derive: only adds.
    CapAdd = 2,
}

impl Change {
    fn from_word(w: u64) -> Change {
        match w {
            1 => Change::Unmap,
            2 => Change::CapAdd,
            _ => Change::CapRemove,
        }
    }
}

/// How a core waits: in the kernel spinning (a message costs nothing more),
/// or running user code or idle (a message costs an interrupt).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Kernel,
    Interruptible,
}

enum Held {
    Yes,
    No,
}

/// Which copy a change is applied to.
#[derive(Clone, Copy)]
enum Copy {
    Home,
    Replica,
}

struct Node {
    lock: crate::sync::Lock,
    locked: bool,
    /// Lock and sequence counter of each slot range of each process region.
    stripe_locks: Box<[crate::sync::Lock]>,
    stripe_seqs: Box<[Padded<AtomicU64>]>,
    /// Lock of each range of each process's address space (`Unmaps::Range`).
    as_locks: Box<[crate::sync::Lock]>,
    state: Box<[Padded<AtomicU64>]>,
    /// Sequence counter of each process region (capability space).
    seqs: Box<[Padded<AtomicU64>]>,
    inbox: Box<[Ring]>,
    applied: Box<[AtomicU64]>,
    members: Padded<AtomicU64>,
    /// Nodes that pushed since the last look: the stand-in for the interrupt
    /// a sender raises, so that an idle node reads one line, not every ring.
    doorbell: Padded<AtomicU64>,
    /// Set while a core of the node is in an interrupt entry for its rings.
    irq: Padded<AtomicBool>,
}

struct Machine {
    nodes: Box<[Node]>,
    node_of: Box<[AtomicUsize]>,
    proc_of: Vec<usize>,
    proc_nodes: Vec<Vec<usize>>,
    proc_cores: Vec<Vec<usize>>,
    proc_home: Vec<usize>,
    roles: Vec<Role>,
    /// The endpoint or notification object each core owns, with its lock.
    obj_locks: Box<[crate::sync::Lock]>,
    obj_state: Box<[Padded<AtomicU64>]>,
    /// The notify each core owns, for `Signals::Atomic`.
    notifies: Box<[Notify]>,
    /// srv: server threads, the endpoint (owned by core 0, so its lock is
    /// `obj_locks[0]`; the mutex inside only checks that lock), and per core
    /// the request handed to it, the context lent to it, the reply to it.
    threads: usize,
    ep: Padded<Mutex<srv::Ep>>,
    disp: Box<[Padded<AtomicU64>]>,
    lent: Box<[Padded<AtomicU64>]>,
    reply: Box<[Padded<AtomicU64>]>,
    t0: Instant,
    mbox_tlb: Box<[Padded<AtomicU64>]>,
    mbox_ev: Box<[Padded<AtomicU64>]>,
    waits: Box<[Padded<AtomicU64>]>,
    bye: Box<[Padded<AtomicBool>]>,
    phase: Padded<AtomicU8>,
    done: Padded<AtomicUsize>,
    drained: Padded<AtomicUsize>,
    p: Params,
    e_cores: usize,
}

/// What one run measured.
#[derive(Default)]
pub struct RunOut {
    pub ops: [u64; CLASSES],
    pub hist: [Hist; CLASSES],
    pub secs: f64,
    pub migrations: u64,
    pub samples: u64,
    pub preferred: u64,
    pub stalls: u64,
    /// Lookups read again because a writer was active (seqlock).
    pub retries: u64,
    /// Operations per worker that issues any, for fairness.
    pub per_core: Vec<u64>,
}

#[derive(Default)]
struct CoreOut {
    ops: [u64; CLASSES],
    hist: [Hist; CLASSES],
    migrations: u64,
    samples: u64,
    preferred: u64,
    touches: u64,
    changes: Vec<u64>,
    retries: u64,
}

/// Node of each core, the lock of each node, and the process of each core.
fn layout(w: Workload, cand: Cand, k: usize, n: usize) -> (Vec<usize>, Vec<bool>, Vec<usize>) {
    if let Workload::Move { .. } = w {
        // Group A: cores 0..k, group B: cores k..n-1, the mobile core n-1
        // starts in A.
        let mut node: Vec<usize> = (0..n).map(|c| usize::from(c >= k)).collect();
        node[n - 1] = 0;
        return (node.clone(), vec![true, true], node);
    }
    let node: Vec<usize> = (0..n)
        .map(|c| match cand {
            Cand::PerCore => c,
            Cand::Fixed(g) => c / g,
            Cand::Sized if c < k => 0,
            Cand::Sized => c - k + 1,
        })
        .collect();
    let count = node.iter().max().map_or(0, |m| m + 1);
    let locked = (0..count)
        .map(|i| match cand {
            Cand::PerCore => false,
            Cand::Fixed(_) => true,
            Cand::Sized => i == 0 && k > 1,
        })
        .collect();
    let proc: Vec<usize> = (0..n).map(|c| if c < k { 0 } else { c - k + 1 }).collect();
    (node, locked, proc)
}

fn roles(w: Workload, k: usize, n: usize) -> Vec<Role> {
    (0..n)
        .map(|c| match w {
            Workload::CapOps => Role::CapOps,
            Workload::Mixed if c < k => Role::CapOps,
            Workload::Mixed => Role::Lookups,
            Workload::Ipc if c % 2 == 0 => Role::Client(c + 1),
            Workload::Ipc => Role::Server(c - 1),
            Workload::Signal if c == 0 => Role::Waiter,
            Workload::Signal => Role::Producer(0),
            Workload::Unmap(Initiators::All) => Role::Unmapper,
            Workload::Unmap(Initiators::One) if c == 0 => Role::Unmapper,
            Workload::Unmap(Initiators::One) => Role::User,
            Workload::Move { rate } if c == n - 1 => Role::Mobile(rate),
            Workload::Move { .. } => Role::Grouped,
            Workload::Server { threads, .. } if c < threads => Role::SrvThread,
            Workload::Server { threads, .. } if c == threads => Role::Fg,
            Workload::Server { bg: Bg::On, .. } => Role::Bg,
            Workload::Server { bg: Bg::Off, .. } => Role::User,
        })
        .collect()
}

impl Machine {
    fn new(w: Workload, cand: Cand, k: usize, n: usize, p: Params) -> Machine {
        assert!(n <= MAX_CORES && k >= 1 && k <= n, "k={k} n={n}");
        if w == Workload::Ipc {
            assert!(
                k == n && n.is_multiple_of(2),
                "IPC needs an even number of cores"
            );
        }
        let threads = match w {
            Workload::Server { threads, .. } => threads,
            _ => 0,
        };
        assert!(
            threads == 0 || (k == threads && n == threads + 2),
            "srv layout"
        );
        let (node_of, locked, proc_of) = layout(w, cand, k, n);
        let nprocs = proc_of.iter().max().map_or(0, |m| m + 1);
        let mut proc_nodes = vec![Vec::new(); nprocs];
        let mut proc_cores = vec![Vec::new(); nprocs];
        for c in 0..n {
            proc_cores[proc_of[c]].push(c);
            if !proc_nodes[proc_of[c]].contains(&node_of[c]) {
                proc_nodes[proc_of[c]].push(node_of[c]);
            }
        }
        let proc_home = proc_nodes.iter().map(|ns| ns[0]).collect();
        let nodes = locked
            .iter()
            .enumerate()
            .map(|(i, &l)| Node {
                lock: crate::sync::Lock::new(if l { p.lock } else { LockKind::None }),
                locked: l,
                stripe_locks: (0..PROC_REGIONS * STRIPES)
                    .map(|_| crate::sync::Lock::new(if l { p.lock } else { LockKind::None }))
                    .collect(),
                stripe_seqs: (0..PROC_REGIONS * STRIPES)
                    .map(|_| Padded::default())
                    .collect(),
                as_locks: (0..PROC_REGIONS * STRIPES)
                    .map(|_| crate::sync::Lock::new(if l { p.lock } else { LockKind::None }))
                    .collect(),
                state: (0..PROC_LINES * PROC_REGIONS)
                    .map(|_| Padded::default())
                    .collect(),
                seqs: (0..PROC_REGIONS).map(|_| Padded::default()).collect(),
                inbox: (0..locked.len()).map(|_| Ring::default()).collect(),
                doorbell: Padded::default(),
                irq: Padded::default(),
                applied: (0..nprocs).map(|_| AtomicU64::new(0)).collect(),
                members: Padded(AtomicU64::new(
                    (0..n).filter(|&c| node_of[c] == i).map(|c| 1u64 << c).sum(),
                )),
            })
            .collect();
        let pad = |_| Padded::default();
        let obj_kind = |c: usize| {
            if locked[node_of[c]] {
                p.lock
            } else {
                LockKind::None
            }
        };
        let obj_locks = (0..n)
            .map(|c| crate::sync::Lock::new(obj_kind(c)))
            .collect();
        Machine {
            nodes,
            obj_locks,
            obj_state: (0..n * OBJ_LINES).map(pad).collect(),
            notifies: (0..n).map(|c| Notify::new(obj_kind(c))).collect(),
            threads,
            ep: Padded(Mutex::new(srv::Ep::new(threads))),
            disp: (0..n).map(pad).collect(),
            lent: (0..n).map(pad).collect(),
            reply: (0..n).map(pad).collect(),
            t0: Instant::now(),
            node_of: node_of.into_iter().map(AtomicUsize::new).collect(),
            proc_of,
            proc_nodes,
            proc_cores,
            proc_home,
            roles: roles(w, k, n),
            mbox_tlb: (0..n).map(pad).collect(),
            mbox_ev: (0..n).map(pad).collect(),
            waits: (0..n).map(pad).collect(),
            bye: (0..n).map(|_| Padded::default()).collect(),
            phase: Padded(AtomicU8::new(WARMUP)),
            done: Padded::default(),
            drained: Padded::default(),
            p,
            e_cores: platform::efficiency_cores(),
        }
    }

    fn node_of(&self, c: usize) -> usize {
        self.node_of[c].load(Ordering::Relaxed)
    }
}

struct Core<'m> {
    m: &'m Machine,
    id: usize,
    node: usize,
    proc: usize,
    rng: u64,
    last_cpu: Option<usize>,
    out: CoreOut,
}

fn bits(mut mask: u64) -> impl Iterator<Item = usize> {
    core::iter::from_fn(move || {
        (mask != 0).then(|| {
            let b = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            b
        })
    })
}

impl<'m> Core<'m> {
    fn new(m: &'m Machine, id: usize) -> Core<'m> {
        Core {
            m,
            id,
            node: m.node_of(id),
            proc: m.proc_of[id],
            rng: 0x9E37_79B9_7F4A_7C15 ^ (id as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03),
            last_cpu: platform::current_cpu(),
            out: CoreOut {
                changes: vec![0; m.proc_cores.len()],
                ..CoreOut::default()
            },
        }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn running(&self) -> bool {
        self.m.phase.load(Ordering::Relaxed) < STOP
    }

    fn lock(&self) -> Guard<'m> {
        self.m.nodes[self.node].lock.lock()
    }

    /// Kernel work on the state of process `proc` in this core's node:
    /// `lines` lines read and written (their transfer is measured), then
    /// `ns` of compute (modelled). Callers hold the node lock.
    fn work(&mut self, proc: usize, lines: usize, ns: u64) {
        let base = (proc % PROC_REGIONS) * PROC_LINES;
        let start = self.rand() as usize;
        let state = &self.m.nodes[self.node].state;
        for j in 0..lines {
            let l = &state[base + (start + j) % PROC_LINES];
            l.store(l.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        self.out.touches += lines as u64;
        busy(ns);
    }

    /// Whether this core's writes take the fine locks: only in a group.
    fn fine(&self) -> bool {
        self.m.p.locks == Locks::Fine && self.m.nodes[self.node].locked
    }

    /// The lock of the object `owner` owns (its endpoint or notify), or the
    /// group lock with coarse locks.
    fn obj_lock(&self, owner: usize) -> Guard<'m> {
        if self.fine() {
            self.m.obj_locks[owner].lock()
        } else {
            self.lock()
        }
    }

    /// Work on the object `owner` owns: its own lines with fine locks, the
    /// node state of its process with coarse ones (as in earlier series).
    fn obj_work(&mut self, owner: usize, lines: usize, ns: u64) {
        if !self.fine() {
            return self.work(self.m.proc_of[owner], lines, ns);
        }
        let start = self.rand() as usize;
        for j in 0..lines {
            let l = &self.m.obj_state[owner * OBJ_LINES + (start + j) % OBJ_LINES];
            l.store(l.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        self.out.touches += lines as u64;
        busy(ns);
    }

    /// Half an IPC or signal operation on `owner`'s object, under its lock.
    fn half_obj(&mut self, owner: usize) {
        let _g = self.obj_lock(owner);
        self.obj_work(owner, LINES / 2, self.m.p.work_ns / 2);
    }

    /// How a send is made: under the group lock already held (coarse), or
    /// taking it for the ring only (fine: an object lock is held instead).
    fn held(&self) -> Held {
        if self.fine() { Held::No } else { Held::Yes }
    }

    /// Two slot ranges, in lock order (one if both are the same).
    fn stripes(&mut self) -> (usize, usize) {
        let a = self.rand() as usize % STRIPES;
        let b = self.rand() as usize % STRIPES;
        (a.min(b), a.max(b))
    }

    /// Writes two slot ranges of `pr`'s capability space on this node
    /// (2 lines each), inside the write side of both seqlocks: the one of
    /// the space and the one of each range. The caller holds the locks.
    fn write_stripes(&mut self, pr: usize, (a, b): (usize, usize), ns: u64) {
        let node = &self.m.nodes[self.node];
        let region = pr % PROC_REGIONS;
        let seqs = [
            Some(&node.seqs[region]),
            Some(&node.stripe_seqs[region * STRIPES + a]),
            (b != a).then(|| &node.stripe_seqs[region * STRIPES + b]),
        ];
        for q in seqs.iter().flatten() {
            q.store(q.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        fence(Ordering::Release);
        for st in [a, b] {
            for j in 0..2 {
                let l = &node.state[region * PROC_LINES + st * 2 + j];
                l.store(l.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            }
        }
        self.out.touches += LINES as u64;
        busy(ns);
        for q in seqs.iter().flatten() {
            q.store(q.load(Ordering::Relaxed) + 1, Ordering::Release);
        }
    }

    fn timed(&mut self, class: usize, op: impl FnOnce(&mut Self)) {
        let t = Instant::now();
        op(self);
        self.record(class, t);
    }

    /// Records an operation that began at `since`, in `class`.
    fn record(&mut self, class: usize, since: Instant) {
        let ns = since.elapsed().as_nanos() as u64;
        if self.m.phase.load(Ordering::Relaxed) == MEASURE {
            self.out.hist[class].record(ns);
            self.out.ops[class] += 1;
            self.sample_cpu();
        }
    }

    fn sample_cpu(&mut self) {
        let cpu = platform::current_cpu();
        self.out.samples += 1;
        if cpu.is_some_and(|c| c >= self.m.e_cores) {
            self.out.preferred += 1;
        }
        if cpu != self.last_cpu {
            self.out.migrations += 1;
            self.last_cpu = cpu;
        }
    }

    fn stuck(&self, since: Instant, what: &str) {
        assert!(
            since.elapsed() < STUCK,
            "core {} stuck in {what}: a bug of the simulation",
            self.id
        );
    }

    /// Handles what has reached this core: messages on its node's rings and
    /// invalidation requests. Returns whether anything was found (and, in
    /// interruptible mode, an interrupt charged).
    ///
    /// One interrupt per batch reaches one core of a node: a core that finds
    /// another core of its node in the middle of an interrupt entry leaves
    /// the batch to it (`Node::irq`). Until the fourth series every core of
    /// a group that saw the doorbell paid an interrupt for the same message
    /// (README, "Fourth series"). What arrives during the entry is handled
    /// in the same batch, and a core spinning in the kernel takes messages
    /// at once, as before.
    fn poll(&mut self, mode: Mode) -> bool {
        let m = self.m;
        let node = &m.nodes[self.node];
        let bell = node.doorbell.load(Ordering::Relaxed) != 0;
        let tlb = m.mbox_tlb[self.id].load(Ordering::Relaxed) != 0;
        if !bell && !tlb {
            return false;
        }
        let charged = mode == Mode::Interruptible;
        let per_message = charged && m.p.irq == Irq::PerMessage;
        let mut claimed = false;
        if charged && !per_message {
            // An invalidation interrupts this core whatever the others do.
            if !tlb {
                if node.irq.swap(true, Ordering::Acquire) {
                    return false;
                }
                claimed = true;
            }
            platform::interrupt(m.p.ipi_ns);
        }
        let bells = if bell {
            node.doorbell.swap(0, Ordering::Acquire)
        } else {
            0
        };
        if claimed {
            node.irq.store(false, Ordering::Release);
        }
        for from in bits(bells) {
            let ring = &node.inbox[from];
            while ring.ready() {
                let _g = self.lock();
                // Another core of the node may have taken it.
                let Some(msg) = ring.pop() else { break };
                if per_message {
                    platform::interrupt(m.p.ipi_ns);
                }
                self.handle(msg);
            }
        }
        if tlb {
            for origin in bits(m.mbox_tlb[self.id].swap(0, Ordering::Acquire)) {
                if per_message {
                    platform::interrupt(m.p.ipi_ns);
                }
                busy(m.p.tlb_ns);
                self.ack(origin, Held::No);
            }
        }
        true
    }

    /// One message from another node; the node lock is held.
    fn handle(&mut self, msg: Msg) {
        let m = self.m;
        let (a, b) = (msg.a as usize, msg.b as usize);
        match msg.kind {
            REQ => self.home_apply(a, Change::from_word(msg.b)),
            APPLY => {
                self.apply_here(a, Copy::Replica);
                self.apply_tail(a, Change::from_word(msg.b));
            }
            ACK => {
                m.waits[a].fetch_sub(1, Ordering::Release);
            }
            EVENT => {
                m.mbox_ev[a].fetch_or(1 << b, Ordering::Release);
            }
            SIGNAL if m.p.signals == Signals::Atomic => {
                // Node lock, then the notify's lock if its thread waits.
                if m.notifies[a].signal(1 << (b % 63)) == Signalled::Wake {
                    m.mbox_ev[a].fetch_or(1 << b, Ordering::Release);
                }
            }
            SIGNAL => {
                // Node lock, then object lock: the only place both are held
                // in that order; a sender holding an object lock never
                // waits for this node's lock while holding a notify's.
                let _g = self.fine().then(|| m.obj_locks[a].lock());
                self.obj_work(a, LINES / 2, m.p.work_ns / 2);
                m.mbox_ev[a].fetch_or(1 << b, Ordering::Release);
            }
            CALL..=REPLY => self.srv_handle(msg),
            _ => unreachable!("message kind {}", msg.kind),
        }
    }

    fn push(&self, to: usize, msg: Msg) {
        let node = &self.m.nodes[to];
        let ok = node.inbox[self.node].push(msg, STUCK);
        assert!(ok, "ring {} -> {to} full for {STUCK:?}", self.node);
        node.doorbell.fetch_or(1 << self.node, Ordering::Release);
    }

    fn send(&self, to: usize, msg: Msg) {
        let _g = self.lock();
        self.push(to, msg);
    }

    fn ack(&self, origin: usize, held: Held) {
        let m = self.m;
        let to = m.node_of(origin);
        if to == self.node {
            m.waits[origin].fetch_sub(1, Ordering::Release);
            return;
        }
        let msg = Msg {
            kind: ACK,
            a: origin as u64,
            b: 0,
        };
        match held {
            Held::Yes => self.push(to, msg),
            Held::No => self.send(to, msg),
        }
    }

    /// Applies a change of `origin`'s process to this node's copy, inside
    /// the write side of the seqlock of that capability space.
    fn apply_here(&mut self, origin: usize, copy: Copy) {
        let m = self.m;
        let pr = m.proc_of[origin];
        let ns = match copy {
            Copy::Home => m.p.work_ns,
            Copy::Replica => m.p.work_ns * m.p.replica_pct / 100,
        };
        let st = self.stripes();
        self.write_stripes(pr, st, ns);
        m.nodes[self.node].applied[pr].fetch_add(1, Ordering::Relaxed);
    }

    /// On the home node, lock held: apply, then send to the other copies.
    fn home_apply(&mut self, origin: usize, change: Change) {
        let m = self.m;
        self.apply_here(origin, Copy::Home);
        for &n in &m.proc_nodes[m.proc_of[origin]] {
            if n != self.node {
                let msg = Msg {
                    kind: APPLY,
                    a: origin as u64,
                    b: change as u64,
                };
                self.push(n, msg);
            }
        }
        self.apply_tail(origin, change);
    }

    /// After this node's copy is changed, lock held: a capability change is
    /// acknowledged once per node (a lazy addition by the origin's node
    /// only); an unmap once per core of the process on this node, after its
    /// TLB invalidation.
    fn apply_tail(&mut self, origin: usize, change: Change) {
        let m = self.m;
        let lazy = m.p.adds == Adds::Lazy && change == Change::CapAdd;
        if change != Change::Unmap {
            if !lazy || self.node == m.node_of(origin) {
                self.ack(origin, Held::Yes);
            }
            return;
        }
        for &c in &m.proc_cores[m.proc_of[origin]] {
            if m.node_of(c) != self.node {
                continue;
            }
            if c == self.id {
                busy(m.p.tlb_ns);
                self.ack(origin, Held::Yes);
            } else {
                m.mbox_tlb[c].fetch_or(1 << origin, Ordering::Release);
            }
        }
    }

    /// A change of a shared structure of the process: applied on the home
    /// node, sent to every other node of the process, acknowledged.
    fn change(&mut self, change: Change) {
        let m = self.m;
        let pr = self.proc;
        let expected = match change {
            Change::CapAdd if m.p.adds == Adds::Lazy => 1,
            Change::CapAdd | Change::CapRemove => m.proc_nodes[pr].len(),
            Change::Unmap => m.proc_cores[pr].len(),
        };
        self.out.changes[pr] += 1;
        let one_group = m.nodes[self.node].locked && m.proc_nodes[pr].len() == 1;
        if self.fine() && change != Change::Unmap && m.proc_nodes[pr].len() == 1 {
            return self.fine_change(pr);
        }
        if change == Change::Unmap && m.p.unmaps == Unmaps::Range && one_group {
            return self.range_unmap(pr);
        }
        m.waits[self.id].store(expected as u64, Ordering::Relaxed);
        let home = m.proc_home[pr];
        if self.node == home {
            let _g = self.lock();
            self.home_apply(self.id, change);
        } else {
            let msg = Msg {
                kind: REQ,
                a: self.id as u64,
                b: change as u64,
            };
            self.send(home, msg);
        }
        self.wait_acks();
    }

    /// A capability change inside one group with fine locks: the locks of
    /// the two slot ranges it writes, in order, and no group lock. Legal
    /// only if the change touches nothing outside those ranges (below).
    fn fine_change(&mut self, pr: usize) {
        let m = self.m;
        let (a, b) = self.stripes();
        let locks = &m.nodes[self.node].stripe_locks;
        let region = pr % PROC_REGIONS;
        let _ga = locks[region * STRIPES + a].lock();
        let _gb = (b != a).then(|| locks[region * STRIPES + b].lock());
        self.write_stripes(pr, (a, b), m.p.work_ns);
        m.nodes[self.node].applied[pr].fetch_add(1, Ordering::Relaxed);
    }

    /// An unmap inside one group with a lock per range of the address space:
    /// the lock of the one range it changes, no group lock; the invalidation
    /// is posted under it, and acknowledged after it is released, as with
    /// the group lock. Legal only for an unmap inside one range that frees
    /// no page-table page (README, "Fourth series"). Workload b changes no
    /// capability, so sharing the range's state lines with `fine_change`
    /// loses no update.
    fn range_unmap(&mut self, pr: usize) {
        let m = self.m;
        let node = &m.nodes[self.node];
        m.waits[self.id].store(m.proc_cores[pr].len() as u64, Ordering::Relaxed);
        {
            let range = self.rand() as usize % STRIPES;
            let region = pr % PROC_REGIONS;
            let _g = node.as_locks[region * STRIPES + range].lock();
            for j in 0..LINES {
                let l = &node.state[region * PROC_LINES + range * 2 + j % 2];
                l.store(l.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            }
            self.out.touches += LINES as u64;
            busy(m.p.work_ns);
            node.applied[pr].fetch_add(1, Ordering::Relaxed);
            self.apply_tail(self.id, Change::Unmap);
        }
        self.wait_acks();
    }

    fn wait_acks(&mut self) {
        let since = Instant::now();
        while self.m.waits[self.id].load(Ordering::Acquire) != 0 {
            self.poll(Mode::Kernel);
            self.stuck(since, "a wait for acknowledgements");
        }
    }

    fn lookup(&mut self) {
        match self.m.p.lookup {
            Lookup::Locked => {
                let _g = self.lock();
                self.work(self.proc, LINES, self.m.p.work_ns);
            }
            Lookup::Seqlock => self.read_lookup(),
            Lookup::Slot => {
                self.slot_read();
                busy(self.m.p.work_ns);
            }
        }
    }

    /// One slot read under the seqlock of its range: the lock-free read of
    /// a capability, retried if a writer of that range came in.
    fn slot_read(&mut self) {
        let m = self.m;
        let node = &m.nodes[self.node];
        let region = self.proc % PROC_REGIONS;
        let st = self.rand() as usize % STRIPES;
        let seq = &node.stripe_seqs[region * STRIPES + st];
        loop {
            let s = seq.load(Ordering::Acquire);
            if !s.is_multiple_of(2) {
                core::hint::spin_loop();
                continue;
            }
            black_box(node.state[region * PROC_LINES + st * 2].load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if seq.load(Ordering::Relaxed) == s {
                return;
            }
            if m.phase.load(Ordering::Relaxed) == MEASURE {
                self.out.retries += 1;
            }
        }
    }

    /// A lookup that takes no lock: it only reads its lines, and pays what
    /// a writer costs it by reading again (the retries are counted).
    fn read_lookup(&mut self) {
        let m = self.m;
        let node = &m.nodes[self.node];
        let seq = &node.seqs[self.proc % PROC_REGIONS];
        let base = (self.proc % PROC_REGIONS) * PROC_LINES;
        let start = self.rand() as usize;
        loop {
            let s = seq.load(Ordering::Acquire);
            if !s.is_multiple_of(2) {
                // A writer is in: wait for it (the time is in the latency).
                core::hint::spin_loop();
                continue;
            }
            for j in 0..LINES {
                black_box(node.state[base + (start + j) % PROC_LINES].load(Ordering::Relaxed));
            }
            busy(m.p.work_ns);
            fence(Ordering::Acquire);
            if seq.load(Ordering::Relaxed) == s {
                return;
            }
            // A writer came in during the read: the whole read is wasted.
            if m.phase.load(Ordering::Relaxed) == MEASURE {
                self.out.retries += 1;
            }
        }
    }

    /// Unmap inside one group, with the dynamic membership of workload d.
    fn group_unmap(&mut self) {
        let m = self.m;
        {
            let _g = self.lock();
            self.work(self.proc, LINES, m.p.work_ns);
            let others = m.nodes[self.node].members.load(Ordering::Relaxed) & !(1 << self.id);
            m.waits[self.id].store(u64::from(others.count_ones()), Ordering::Relaxed);
            for c in bits(others) {
                m.mbox_tlb[c].fetch_or(1 << self.id, Ordering::Release);
            }
            busy(m.p.tlb_ns);
        }
        self.wait_acks();
    }

    /// User code: `ns` of compute in slices, any slice may be interrupted.
    /// The time a slice really took counts, so the clock's granularity is
    /// paid once, not per slice; the time interrupts take does not count, so
    /// they delay the next kernel call as they would on hardware.
    fn user(&mut self, ns: u64) {
        let mut left = ns;
        loop {
            let t = Instant::now();
            busy(left.min(SLICE_NS));
            left = left.saturating_sub(t.elapsed().as_nanos() as u64);
            self.poll(Mode::Interruptible);
            if left == 0 {
                break;
            }
        }
    }

    /// Makes `target` runnable with an event from this core.
    fn deliver(&self, target: usize, kind: u64, held: Held) {
        let m = self.m;
        let to = m.node_of(target);
        if to == self.node {
            m.mbox_ev[target].fetch_or(1 << self.id, Ordering::Release);
        } else {
            let msg = Msg {
                kind,
                a: target as u64,
                b: self.id as u64,
            };
            match held {
                Held::Yes => self.push(to, msg),
                Held::No => self.send(to, msg),
            }
        }
    }

    /// Idles until events arrive; the wake-up costs an interrupt. Returns 0
    /// if `quit` says to stop first.
    fn wait_event(&mut self, quit: impl Fn(&Self) -> bool) -> u64 {
        let m = self.m;
        let mut charged = false;
        let since = Instant::now();
        loop {
            charged |= self.poll(Mode::Interruptible);
            let got = m.mbox_ev[self.id].swap(0, Ordering::Acquire);
            if got != 0 {
                if !charged {
                    platform::interrupt(m.p.ipi_ns);
                }
                return got;
            }
            if quit(self) {
                return 0;
            }
            self.stuck(since, "an idle wait");
        }
    }

    fn cap_op(&mut self) {
        if self.rand() % 100 < self.m.p.change_pct {
            if self.rand().is_multiple_of(2) {
                self.timed(3, |c| c.change(Change::CapAdd));
            } else {
                self.timed(1, |c| c.change(Change::CapRemove));
            }
        } else {
            self.timed(0, Core::lookup);
        }
    }

    fn client(&mut self, server: usize) {
        let m = self.m;
        while self.running() {
            self.user(m.p.gap_ns);
            self.timed(0, |c| {
                {
                    let _g = c.obj_lock(server);
                    c.obj_work(server, LINES / 2, m.p.work_ns / 2);
                    c.deliver(server, EVENT, c.held());
                }
                c.wait_event(|_| false);
                c.half_obj(server);
            });
        }
        m.bye[server].store(true, Ordering::Release);
    }

    fn server(&mut self, client: usize) {
        let m = self.m;
        while self.wait_event(|c| m.bye[c.id].load(Ordering::Acquire)) != 0 {
            self.half_obj(self.id);
            self.user(m.p.gap_ns);
            let _g = self.obj_lock(self.id);
            self.obj_work(self.id, LINES / 2, m.p.work_ns / 2);
            self.deliver(client, EVENT, self.held());
        }
    }

    /// The check half of a signal, then the apply half here (same node) or
    /// on the notify's node (message).
    fn producer(&mut self, waiter: usize) {
        let m = self.m;
        let atomic = m.p.signals == Signals::Atomic
            && m.nodes[self.node].locked
            && m.node_of(waiter) == self.node;
        while self.running() {
            self.user(m.p.gap_ns);
            if atomic {
                self.atomic_signal(waiter);
                continue;
            }
            self.timed(0, |c| {
                if c.fine() {
                    // The check is a lookup: lock-free. Only the apply takes
                    // the notify's lock.
                    c.slot_read();
                    busy(m.p.work_ns / 2);
                    if m.node_of(waiter) == c.node {
                        let _g = c.obj_lock(waiter);
                        c.obj_work(waiter, LINES / 2, m.p.work_ns / 2);
                        c.deliver(waiter, SIGNAL, Held::No);
                    } else {
                        c.deliver(waiter, SIGNAL, Held::No);
                    }
                    return;
                }
                let _g = c.lock();
                c.work(c.proc, LINES / 2, m.p.work_ns / 2);
                if m.node_of(waiter) == c.node {
                    c.work(c.proc, LINES / 2, m.p.work_ns / 2);
                }
                c.deliver(waiter, SIGNAL, Held::Yes);
            });
        }
    }

    /// A signal inside a group with `Signals::Atomic`: the lock-free check,
    /// then one OR. Class 0 if that was all (no lock), class 1 if the
    /// notify's thread was waiting, so the lock was taken.
    fn atomic_signal(&mut self, waiter: usize) {
        let m = self.m;
        let t = Instant::now();
        self.slot_read();
        busy(m.p.work_ns / 2);
        let class = match m.notifies[waiter].signal(1 << (self.id % 63)) {
            Signalled::Fast => 0,
            Signalled::Wake => {
                self.deliver(waiter, EVENT, Held::No);
                1
            }
            Signalled::Slow => 1,
        };
        self.record(class, t);
    }

    fn waiter(&mut self) {
        let m = self.m;
        if m.p.signals == Signals::Atomic {
            return self.atomic_waiter();
        }
        while self.wait_event(|c| !c.running()) != 0 {
            self.half_obj(self.id);
            if m.p.rx_ns > 0 {
                self.user(m.p.rx_ns);
            }
        }
    }

    /// Takes the bits without a lock when there are some; otherwise
    /// registers under the notify's lock and sleeps until woken. Its half of
    /// the operation (`W/2`) runs on state it owns, with no lock.
    fn atomic_waiter(&mut self) {
        let m = self.m;
        let notify = &m.notifies[self.id];
        while self.running() {
            if notify.take_or_wait() == 0 {
                if self.wait_event(|c| !c.running()) == 0 {
                    break;
                }
                if notify.take() == 0 {
                    continue; // a wake-up with no bit: see `sync::Notify`
                }
            }
            busy(m.p.work_ns / 2);
            if m.p.rx_ns > 0 {
                self.user(m.p.rx_ns);
            }
        }
    }

    /// Workload d: leave this node, switch, join `to`.
    fn move_to(&mut self, to: usize) {
        let m = self.m;
        {
            let _g = self.lock();
            self.work(self.proc, LINES, m.p.work_ns);
            m.nodes[self.node]
                .members
                .fetch_and(!(1 << self.id), Ordering::Relaxed);
        }
        // Every invalidation that counted this core was posted under the
        // lock just taken, so it is visible now.
        for origin in bits(m.mbox_tlb[self.id].swap(0, Ordering::Acquire)) {
            busy(m.p.tlb_ns);
            m.waits[origin].fetch_sub(1, Ordering::Release);
        }
        busy(m.p.switch_ns);
        m.node_of[self.id].store(to, Ordering::Relaxed);
        self.node = to;
        self.proc = to;
        let _g = self.lock();
        self.work(self.proc, LINES, m.p.work_ns);
        m.nodes[to]
            .members
            .fetch_or(1 << self.id, Ordering::Relaxed);
    }

    /// Workload d: kernel operations in the group, 1 % of them unmaps. Inside
    /// one node a capability change costs what a lookup costs.
    fn grouped_op(&mut self, class: usize) {
        if self.rand().is_multiple_of(100) {
            self.timed(class, Core::group_unmap);
        } else {
            self.timed(class, Core::lookup);
        }
    }

    fn mobile(&mut self, rate: u64) {
        let m = self.m;
        let period = Duration::from_nanos(1_000_000_000 / rate.max(1));
        let mut next = Instant::now() + period;
        let mut cold = false;
        while self.running() {
            self.user(m.p.gap_ns);
            if rate > 0 && Instant::now() >= next {
                next += period;
                let to = 1 - self.node;
                self.timed(1, |c| c.move_to(to));
                cold = true;
            }
            self.grouped_op(if cold { 2 } else { 0 });
            cold = false;
        }
    }

    fn run(&mut self, role: Role) {
        let m = self.m;
        match role {
            Role::CapOps => {
                while self.running() {
                    self.user(m.p.gap_ns);
                    self.cap_op();
                }
            }
            Role::Lookups => {
                while self.running() {
                    self.user(m.p.gap_ns);
                    self.timed(2, Core::lookup);
                }
            }
            Role::Client(s) => self.client(s),
            Role::Server(c) => self.server(c),
            Role::Producer(w) => self.producer(w),
            Role::Waiter => self.waiter(),
            Role::Unmapper => {
                while self.running() {
                    self.user(m.p.gap_ns);
                    self.timed(0, |c| c.change(Change::Unmap));
                }
            }
            Role::User => {
                while self.running() {
                    self.user(m.p.gap_ns.max(SLICE_NS));
                }
            }
            Role::Mobile(rate) => self.mobile(rate),
            Role::SrvThread => self.srv_thread(),
            Role::Fg => self.srv_fg(),
            Role::Bg => self.srv_bg(),
            Role::Grouped => {
                while self.running() {
                    self.user(m.p.gap_ns);
                    self.grouped_op(0);
                }
            }
        }
        // Keep answering until every core has finished its last operation.
        m.done.fetch_add(1, Ordering::AcqRel);
        let since = Instant::now();
        while m.done.load(Ordering::Acquire) < m.roles.len() {
            self.poll(Mode::Kernel);
            self.stuck(since, "the final drain");
        }
        // Lazy additions may still be on their way to other copies: the home
        // node can still be sending them when their origin is done. Once
        // every core has passed this point no core is inside a handler, so
        // nothing more is sent, and one last look empties the rings.
        m.drained.fetch_add(1, Ordering::AcqRel);
        while m.drained.load(Ordering::Acquire) < m.roles.len() {
            self.poll(Mode::Kernel);
            self.stuck(since, "the final drain");
        }
        while self.poll(Mode::Kernel) {}
    }
}

/// Runs one configuration on `n` worker threads, `cpus[i]` the CPU of worker
/// `i` when the host can pin.
pub fn run(
    w: Workload,
    cand: Cand,
    k: usize,
    n: usize,
    p: Params,
    cpus: &[Option<usize>],
) -> RunOut {
    let m = Machine::new(w, cand, k, n, p);
    let start = Barrier::new(n + 1);
    let (m_ref, start_ref) = (&m, &start);
    let (outs, secs) = rt::scope(
        n,
        |i| {
            platform::prepare_worker(cpus.get(i).copied().flatten());
            let mut core = Core::new(m_ref, i);
            start_ref.wait();
            core.run(m_ref.roles[i]);
            core.out
        },
        || {
            start_ref.wait();
            rt::sleep(p.warmup);
            m_ref.phase.store(MEASURE, Ordering::Relaxed);
            let t = Instant::now();
            rt::sleep(p.dur);
            m_ref.phase.store(STOP, Ordering::Relaxed);
            t.elapsed().as_secs_f64()
        },
    );
    check(&m, &outs);
    let mut r = RunOut {
        secs,
        ..RunOut::default()
    };
    for o in &outs {
        for c in 0..CLASSES {
            r.ops[c] += o.ops[c];
            r.hist[c].merge(&o.hist[c]);
        }
        r.migrations += o.migrations;
        r.samples += o.samples;
        r.preferred += o.preferred;
        r.retries += o.retries;
        let mine: u64 = o.ops.iter().sum();
        if mine > 0 {
            r.per_core.push(mine);
        }
    }
    r.stalls = r.hist.iter().map(|h| h.above(STALL_NS)).sum();
    r
}

/// The simulation checks itself after every run: no line update was lost
/// (the locks excluded), and every copy of a process applied every change.
fn check(m: &Machine, outs: &[CoreOut]) {
    let touched: u64 = outs.iter().map(|o| o.touches).sum();
    let counted: u64 = m
        .nodes
        .iter()
        .flat_map(|n| n.state.iter())
        .chain(m.obj_state.iter())
        .map(|l| l.load(Ordering::Relaxed))
        .sum();
    assert_eq!(touched, counted, "a lock let two cores in");
    for (pr, nodes) in m.proc_nodes.iter().enumerate() {
        let issued: u64 = outs.iter().map(|o| o.changes[pr]).sum();
        for &n in nodes {
            let applied = m.nodes[n].applied[pr].load(Ordering::Relaxed);
            assert_eq!(applied, issued, "process {pr}: node {n} missed a change");
        }
    }
    let ep = m.ep.lock().expect("endpoint");
    ep.check_idle();
}

#[cfg(test)]
mod tests {
    use super::{
        Adds, Bg, Cand, Initiators, Irq, Locks, Lookup, Params, Policy, Signals, Unmaps, Workload,
        run,
    };
    use crate::sync::LockKind;
    use std::time::Duration;

    fn params(lock: LockKind) -> Params {
        Params {
            work_ns: 50,
            gap_ns: 100,
            ipi_ns: 100,
            tlb_ns: 20,
            switch_ns: 100,
            change_pct: 30,
            lock,
            lookup: Lookup::Slot,
            adds: Adds::Lazy,
            replica_pct: 25,
            irq: Irq::PerMessage,
            locks: Locks::Fine,
            signals: Signals::Atomic,
            unmaps: Unmaps::Range,
            policy: Policy::Lend,
            rx_ns: 300,
            warmup: Duration::from_millis(1),
            dur: Duration::from_millis(5),
        }
    }

    /// The first settings: lookups under the lock, every change eager.
    fn first(lock: LockKind) -> Params {
        Params {
            lookup: Lookup::Locked,
            adds: Adds::Eager,
            replica_pct: 100,
            irq: Irq::PerBatch,
            locks: Locks::Group,
            signals: Signals::Locked,
            unmaps: Unmaps::Group,
            policy: Policy::Fifo,
            rx_ns: 0,
            ..params(lock)
        }
    }

    /// The second series' fair settings: whole-call seqlock, group locks.
    fn second(lock: LockKind) -> Params {
        Params {
            lookup: Lookup::Seqlock,
            locks: Locks::Group,
            signals: Signals::Locked,
            unmaps: Unmaps::Group,
            policy: Policy::Refuse,
            ..params(lock)
        }
    }

    /// Every workload ends under every candidate, with both settings of
    /// every switch, and passes the self-checks of `run` (which include
    /// that lazy additions reach every copy); each measures operations.
    #[test]
    fn every_workload_runs_under_every_candidate() {
        let works = [
            Workload::CapOps,
            Workload::Ipc,
            Workload::Signal,
            Workload::Unmap(Initiators::One),
            Workload::Unmap(Initiators::All),
        ];
        for make in [first, second, params] {
            for w in works {
                for cand in [Cand::PerCore, Cand::Fixed(2), Cand::Sized] {
                    let r = run(w, cand, 4, 4, make(LockKind::Queue), &[]);
                    assert!(r.ops.iter().sum::<u64>() > 0, "{w:?} {cand:?}");
                }
            }
            let r = run(
                Workload::Mixed,
                Cand::Fixed(2),
                3,
                4,
                make(LockKind::Queue),
                &[],
            );
            assert!(r.ops[2] > 0, "independent processes measured");
        }
        let mv = Workload::Move { rate: 2000 };
        let r = run(mv, Cand::Sized, 2, 4, params(LockKind::Ttas), &[]);
        assert!(r.ops[1] > 0, "moves measured");
        // The server, every candidate and layout, every policy; the run's
        // self-check also finds the endpoint idle at the end.
        for policy in [Policy::Fifo, Policy::Lend, Policy::Refuse] {
            for threads in [1, 2] {
                for cand in [Cand::PerCore, Cand::Sized, Cand::Fixed(4)] {
                    let p = Params {
                        policy,
                        dur: Duration::from_millis(20),
                        ..params(LockKind::Queue)
                    };
                    let w = Workload::Server {
                        threads,
                        bg: Bg::On,
                    };
                    let r = run(w, cand, threads, threads + 2, p, &[]);
                    assert!(
                        r.ops[0] > 0 && r.ops[1] > 0,
                        "{policy:?} {threads} {cand:?}"
                    );
                }
            }
        }
    }

    /// One server thread, held by the background client: when its budget
    /// runs out mid-request the foreground caller waits for its next period
    /// too, unless the kernel lends the caller's context or refuses calls
    /// that cannot finish. The p90 is several times lower with either.
    #[test]
    fn lending_and_refusal_shorten_the_foreground_wait() {
        let p90 = |policy| {
            let p = Params {
                policy,
                ipi_ns: 0,
                dur: Duration::from_millis(100),
                ..params(LockKind::Queue)
            };
            let w = Workload::Server {
                threads: 1,
                bg: Bg::On,
            };
            run(w, Cand::PerCore, 1, 3, p, &[]).hist[0].quantile(0.9)
        };
        let fifo = p90(Policy::Fifo);
        for policy in [Policy::Lend, Policy::Refuse] {
            let other = p90(policy);
            assert!(2 * other < fifo, "{policy:?} {other} fifo {fifo}");
        }
    }

    /// A lazy derive returns before the other copies have applied it; an
    /// eager one waits for all of them, so it is slower on four nodes.
    #[test]
    fn lazy_additions_wait_less_than_eager_ones() {
        let lazy = Params {
            change_pct: 100,
            ipi_ns: 2000,
            ..params(LockKind::Queue)
        };
        let eager = Params {
            adds: Adds::Eager,
            ..lazy
        };
        let p50 = |p| run(Workload::CapOps, Cand::PerCore, 4, 4, p, &[]).hist[3].quantile(0.5);
        assert!(
            p50(lazy) * 2 < p50(eager),
            "lazy {} eager {}",
            p50(lazy),
            p50(eager)
        );
    }
}
