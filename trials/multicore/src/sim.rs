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

use crate::hist::Hist;
use crate::platform;
use crate::sync::{Guard, LockKind, MAX_CORES, Msg, Padded, Ring, busy};
use std::hint::black_box;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering, fence};
use std::thread;
use std::time::{Duration, Instant};

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
    /// it has finished, reads again.
    Seqlock,
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
}

/// Latency classes. Their meaning depends on the workload (README).
pub const CLASSES: usize = 4;
/// Lines of node state touched by a full operation (half by a half one).
const LINES: usize = 4;
/// Lines of node state per process, and processes whose lines are distinct.
const PROC_LINES: usize = 16;
const PROC_REGIONS: usize = 4;
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
    state: Box<[Padded<AtomicU64>]>,
    /// Sequence counter of each process region (capability space).
    seqs: Box<[Padded<AtomicU64>]>,
    inbox: Box<[Ring]>,
    applied: Box<[AtomicU64]>,
    members: Padded<AtomicU64>,
    /// Nodes that pushed since the last look: the stand-in for the interrupt
    /// a sender raises, so that an idle node reads one line, not every ring.
    doorbell: Padded<AtomicU64>,
}

struct Machine {
    nodes: Box<[Node]>,
    node_of: Box<[AtomicUsize]>,
    proc_of: Vec<usize>,
    proc_nodes: Vec<Vec<usize>>,
    proc_cores: Vec<Vec<usize>>,
    proc_home: Vec<usize>,
    roles: Vec<Role>,
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
                state: (0..PROC_LINES * PROC_REGIONS)
                    .map(|_| Padded::default())
                    .collect(),
                seqs: (0..PROC_REGIONS).map(|_| Padded::default()).collect(),
                inbox: (0..locked.len()).map(|_| Ring::default()).collect(),
                doorbell: Padded::default(),
                applied: (0..nprocs).map(|_| AtomicU64::new(0)).collect(),
                members: Padded(AtomicU64::new(
                    (0..n).filter(|&c| node_of[c] == i).map(|c| 1u64 << c).sum(),
                )),
            })
            .collect();
        let pad = |_| Padded::default();
        Machine {
            nodes,
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
    std::iter::from_fn(move || {
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

    fn timed(&mut self, class: usize, op: impl FnOnce(&mut Self)) {
        let t = Instant::now();
        op(self);
        let ns = t.elapsed().as_nanos() as u64;
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
        if charged && !per_message {
            busy(m.p.ipi_ns);
        }
        if bell {
            for from in bits(node.doorbell.swap(0, Ordering::Acquire)) {
                let ring = &node.inbox[from];
                while ring.ready() {
                    let _g = self.lock();
                    // Another core of the node may have taken it.
                    let Some(msg) = ring.pop() else { break };
                    if per_message {
                        busy(m.p.ipi_ns);
                    }
                    self.handle(msg);
                }
            }
        }
        if tlb {
            for origin in bits(m.mbox_tlb[self.id].swap(0, Ordering::Acquire)) {
                if per_message {
                    busy(m.p.ipi_ns);
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
            SIGNAL => {
                self.work(m.proc_of[b], LINES / 2, m.p.work_ns / 2);
                m.mbox_ev[a].fetch_or(1 << b, Ordering::Release);
            }
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
        let seq = &m.nodes[self.node].seqs[pr % PROC_REGIONS];
        let s = seq.load(Ordering::Relaxed);
        seq.store(s + 1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.work(pr, LINES, ns);
        seq.store(s + 2, Ordering::Release);
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
        m.waits[self.id].store(expected as u64, Ordering::Relaxed);
        self.out.changes[pr] += 1;
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

    fn wait_acks(&mut self) {
        let since = Instant::now();
        while self.m.waits[self.id].load(Ordering::Acquire) != 0 {
            self.poll(Mode::Kernel);
            self.stuck(since, "a wait for acknowledgements");
        }
    }

    fn lookup(&mut self) {
        if self.m.p.lookup == Lookup::Seqlock {
            return self.read_lookup();
        }
        let _g = self.lock();
        self.work(self.proc, LINES, self.m.p.work_ns);
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
                std::hint::spin_loop();
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

    /// Makes `target` runnable with an event from this core; lock held.
    fn deliver(&self, target: usize, kind: u64) {
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
            self.push(to, msg);
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
                    busy(m.p.ipi_ns);
                }
                return got;
            }
            if quit(self) {
                return 0;
            }
            self.stuck(since, "an idle wait");
        }
    }

    fn half_op(&mut self) {
        let _g = self.lock();
        self.work(self.proc, LINES / 2, self.m.p.work_ns / 2);
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
                    let _g = c.lock();
                    c.work(c.proc, LINES / 2, m.p.work_ns / 2);
                    c.deliver(server, EVENT);
                }
                c.wait_event(|_| false);
                c.half_op();
            });
        }
        m.bye[server].store(true, Ordering::Release);
    }

    fn server(&mut self, client: usize) {
        let m = self.m;
        while self.wait_event(|c| m.bye[c.id].load(Ordering::Acquire)) != 0 {
            self.half_op();
            self.user(m.p.gap_ns);
            let _g = self.lock();
            self.work(self.proc, LINES / 2, m.p.work_ns / 2);
            self.deliver(client, EVENT);
        }
    }

    /// The check half of a signal, then the apply half here (same node) or
    /// on the notify's node (message).
    fn producer(&mut self, waiter: usize) {
        let m = self.m;
        while self.running() {
            self.user(m.p.gap_ns);
            self.timed(0, |c| {
                let _g = c.lock();
                c.work(c.proc, LINES / 2, m.p.work_ns / 2);
                if m.node_of(waiter) == c.node {
                    c.work(c.proc, LINES / 2, m.p.work_ns / 2);
                }
                c.deliver(waiter, SIGNAL);
            });
        }
    }

    fn waiter(&mut self) {
        while self.wait_event(|c| !c.running()) != 0 {
            self.half_op();
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
    let (outs, secs) = thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let (m, start) = (&m, &start);
                let cpu = cpus.get(i).copied().flatten();
                s.spawn(move || {
                    platform::prepare_worker(cpu);
                    let mut core = Core::new(m, i);
                    start.wait();
                    core.run(m.roles[i]);
                    core.out
                })
            })
            .collect();
        start.wait();
        thread::sleep(p.warmup);
        m.phase.store(MEASURE, Ordering::Relaxed);
        let t = Instant::now();
        thread::sleep(p.dur);
        m.phase.store(STOP, Ordering::Relaxed);
        let secs = t.elapsed().as_secs_f64();
        let outs: Vec<CoreOut> = handles
            .into_iter()
            .map(|h| h.join().expect("worker panicked"))
            .collect();
        (outs, secs)
    });
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
}

#[cfg(test)]
mod tests {
    use super::{Adds, Cand, Initiators, Irq, Lookup, Params, Workload, run};
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
            lookup: Lookup::Seqlock,
            adds: Adds::Lazy,
            replica_pct: 25,
            irq: Irq::PerMessage,
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
        for make in [first, params] {
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
