# Multicore trial: may a node run on more than one core at the same time?

A user-space bench for the open question of `architecture/kernel.md`,
"Nodes, cores and load", in the documentation repository. It simulates the
kernel-side synchronisation of the three candidates with real threads on real
cores, so the costs that differ between them (cache lines that move, locks,
rings between kernels) are paid for real. What is the same in the three
(system call entry and exit) is left out. What cannot be done from user space
(interrupts, TLB invalidation, switching the kernel's mappings) is modelled by
a busy wait of a stated length, and is a parameter.

Results on the Apple M5 are in [results-mac-m5.md](results-mac-m5.md); the raw
runs (`--csv` output) are in `data/`.

## Run

    nix develop -c cargo run --release -p dodos-trial-multicore -- [options]

| Option | Default | Meaning |
|---|---|---|
| `--suites` | `locks,a1,a2,a3,b,c,d,a1f,a2f,a3f,bf,cf,a1t,a2t,a3t,ct` | Which workloads to run (`f`: second measurement, `t`: third, below) |
| `--reps` | 5 | Repetitions of every cell; the tables give the median and the range |
| `--ms` | 150 | Measured time of one run (after 20 ms of warm-up) |
| `--max-k` | cores - 2 | Largest number of cores given to the multithreaded process |
| `--lock` | `queue` | Lock of the group candidates: `ttas`, `ticket` or `queue` |
| `--csv` | none | One line per run, every class, for later analysis |

A full run takes about 55 minutes on 10 cores (15 for the first seven
suites, 23 for the `f` suites, 16 for the `t` suites). Markdown goes to standard
output, progress to standard error.

On Linux the bench pins worker `i` to the `i`-th physical core (the first
hardware thread of each core, read from sysfs) with `sched_setaffinity`, and
leaves the second hardware thread idle, as the kernel will (D-028). Pinning
fails loudly if the process is not allowed on those CPUs. For clean numbers on
an x86-64 desktop: performance governor, turbo off or fixed, nothing else
running, `--max-k` at most the number of physical cores minus one.

On macOS no pinning exists. Workers ask for the user-interactive QoS class,
which prefers performance cores, and every run records the CPU of each
operation (`pthread_cpu_number_np`): the "on P" column is the share of
operations that ran on a performance core, and migrations are counted.

## The model

One worker thread is one core. A **node** is a block of kernel state with a
lock (none on a node of one core) and one inbound ring per other node. The
candidates differ only in how cores are grouped into nodes, and the same code
runs all three:

| Column | Candidate | Nodes |
|---|---|---|
| `1 per-core` | 1. No | One node per core, no lock |
| `2 fixed/2`, `2 fixed/4` | 2. Fixed groups | Cores `0-1`, `2-3`, ... (or `0-3`, `4-7`, ...) share a node and its lock |
| `3 sized` | 3. Dynamic groups | The k cores of the multithreaded process share one node; every other core is a node alone |

The multithreaded process runs on cores `0..k`. Its **home node** is the node
of core 0.

**Kernel work.** Every kernel operation costs `W` of compute (swept: 50, 200,
1000 ns) and reads and writes 4 cache lines of the node's state (the state of
each process is its own 16 lines, so that two processes on one node share the
lock, not the data). The compute is modelled; the line traffic is real: in a
group the lines move between cores, on a node of one core they stay in its
cache.

**What each operation does.**

- *Lookup* (an operation that reads the capability space, as every call does):
  take the node lock, do `W`. On a node of one core: no lock.
- *Change* of a shared structure (capability derive or delete; unmap): sent to
  the home node (a message if the caller is elsewhere), applied there (`W`
  under its lock), sent to every other node of the process, applied there
  (`W` each, under their lock), acknowledged to the caller, which waits for
  all acknowledgements while serving its own node's messages. A capability
  change is acknowledged once per node. An unmap is acknowledged once per core
  of the process, after a local TLB invalidation (`tlb` = 100 ns); the cores
  of a group are reached through a per-core mailbox in shared memory, other
  nodes by message. Inside one node a change is just `W` under the lock.
- *IPC call and reply*: each one-way trip is `W/2` on the sender (under its
  lock) and `W/2` on the receiver after it wakes. Between nodes the event is a
  message; inside a node it is a write to the receiver's mailbox.
- *Signal*: `W/2` on the signaller; the other `W/2` (applying the bits to the
  notify) is done by the signaller under the lock inside a node, or by the
  notify's node on receipt of the message. The waiter does `W/2` per wake-up,
  whatever number of signals it collects.
- *Move of a core* (candidate 3): take the old node's lock, `W` (save the
  thread, update the membership), release; acknowledge every invalidation
  already addressed to this core; `switch` (modelled: kernel mappings and TLB,
  0 or 1000 ns); take the new node's lock, `W`, join its membership.

**Interrupts.** A core that receives a message or a wake-up while it runs user
code or idles pays `ipi` (0 or 1000 ns) once per batch, in every candidate:
an IPI in candidate 1, a reschedule or shootdown IPI in a group. A core that
spins in the kernel waiting for acknowledgements pays nothing more. User code
is cut in 100 ns slices and checks between slices, which stands for immediate
interrupt delivery. Senders ring a per-node doorbell line so that a node reads
one line, not every ring, when nothing has arrived.

**User time.** Between two kernel calls a thread computes `gap` = 1 µs (the
lock suite also runs `gap` = 0, the worst case). This sets how much of its
time a core spends in the kernel, which decides how much a lock is contended.
With the clock reads of the measurement itself, one call plus its gap takes
about 1.4 µs at `W` = 50 ns on one core (0.72 M calls per second).

**Busy waits** (kernel compute, user time, interrupt, TLB, switch) run to a
deadline read from the clock. A first version used a loop calibrated in
iterations; on the M5 the same loop ran up to 12 % faster or slower depending
on the code around it (a lock or no lock), which changed the offered load
between candidates by as much as the effects measured. Those runs were
discarded. The price of deadlines is the clock's granularity (below).

**Rings.** One single-producer single-consumer ring per ordered pair of nodes
(the node lock makes a group one producer and one consumer). A message is one
128-byte slot published by its sequence word, so a delivery moves one line;
the consumer's position is on its own line and the producer reads it only when
the ring looks full. 128 slots; synchronous traffic between two nodes is
bounded well below that (each core has at most one change in flight), and a
ring that stays full for 10 s stops the run with an error rather than a hang.

**Locks.** Three are implemented: test-and-test-and-set with exponential
backoff, a ticket lock (fair, all waiters spin on one line), and Anderson's
array queue lock (fair, each waiter spins on its own line, the cache
behaviour of an MCS lock without pointers). The `locks` suite measures them on
one group; the other suites use the queue lock (the reason is in the results).

**Unsafe code** is confined to `src/platform.rs`: the foreign calls that set
the QoS class (macOS), pin a thread (`sched_setaffinity`, Linux) and read the
current CPU. Every other file is safe Rust. The crate cannot carry
`#![forbid(unsafe_code)]` for that reason; its tests never call the foreign
functions, so Miri and the sanitizer run them.

**Self-checks.** After every run: the sum of all node-state lines equals the
number of line updates the workers made (a lock that let two cores in would
lose updates), and every node of every process applied every change of that
process. Every wait has a 10 s limit that panics. `cargo test` runs every
workload under every candidate briefly, and tests the locks, the ring and the
histogram.

## Switches (second measurement)

The first measurement left three choices that favoured one side. Each is now
a switch, and the `f` suites measure both settings side by side in the same
pass ("first" = as in the first measurement, "fair" = every switch on).

1. **Lookups without the lock (candidates 2 and 3).** A seqlock per
   capability space: a writer (a change, under the queue lock) makes the
   counter odd, changes the lines, makes it even; a reader reads the counter,
   reads its lines, does its `W`, and reads again from the start if the
   counter was odd or has moved. Chosen over a read-copy-update path because
   it needs no grace periods and no deferred freeing (the kernel has no heap,
   and a capability slot is type-stable memory that is never freed, so a
   torn read is only retried, never followed), and because its cost to a
   reader is visible: a reader that overlaps a writer waits for it, or wastes
   its whole read and starts again. The read section is the whole `W` of the
   lookup, which is pessimistic for the reader (a real lookup reads the slot
   in a fraction of the call); the suites print how many reads were repeated
   per 1000 lookups. Readers write nothing shared, so their lines stay shared
   in every cache. Changes, IPC and signals still take the lock.
2. **Lazy additions and a cheaper copy (candidate 1, and candidate 2 between
   groups).** A derive only adds rights, so it waits for its own node's copy
   only (applied in the home node's order); the other copies apply it later.
   A delete, and every unmap, still waits for every copy. This needs a
   contract point: a thread on another node may briefly find the new slot
   empty (`invalid`); the TLB rule already accepts the same for added
   mappings. The cost of applying a change to a non-home copy is a parameter
   (`replica_pct`, 100 or 25 % of `W`): a copy may cost less because the home
   node has already checked rights, depth and the destination slot, and the
   copy only writes the result; it may not, because each node keeps its own
   derivation links for its own revocation walks, and a page-table copy may
   need a table allocated and cleared. Lines touched are unchanged.
3. **Interrupts per batch or per message (candidate 1).** The first
   measurement already charged one interrupt per batch: a core that finds
   messages handles every message already in its rings for one `ipi`. The
   switch adds the pessimistic setting, one `ipi` per message, to show what
   batching is worth.

## Switches (third measurement): finer locks in a group

The second measurement left every write of a group under one lock. The
third adds the locks a real group kernel would plausibly have, as one more
switch (`Locks::Fine`), and shortens the lock-free lookup to one slot read
(`Lookup::Slot`). Candidate 1 keeps its first and fair settings unchanged
(it has no lock to refine). Each fine lock is legal only if the kernel
guarantees what is listed with it; the bench assumes these guarantees, it
does not check them.

| Lock | Used by | What a real kernel must guarantee for it to be legal |
|---|---|---|
| One per endpoint | IPC: both halves of a call and of a reply take the lock of the server's endpoint, and work on its own lines | The endpoint lock covers its queue of callers and receivers and the reply objects armed through it; a thread waits on one object at a time, and its state is changed only under that object's lock or by its own core; run queues are per core, and a wake-up of another core goes through a lock-free mailbox and an interrupt; the capability lookup before the call is lock-free (below); destroying an endpoint drains its queues under its lock; lock order is object, then group lock (taken only to send a message to another group) |
| One per notification object | Signals: the check half is a lock-free slot read plus `W/2` with no lock; the apply half and the waiter's half take the notify's lock | As for endpoints; and the receive path on a node takes the group lock before the notify lock (the only place both are held in that order), so no sender may wait for a group lock while it holds a notify lock; binding a notify to an endpoint must take both locks in a fixed order |
| One per slot range of a capability space (8 ranges) | Capability changes inside one group: a change takes the locks of the two ranges it writes, in index order, and no group lock | A change writes only those two ranges: a derive (source and destination slots, and the derivation link between them) and a delete of a capability with no descendants; a delete or revoke of a subtree crosses ranges without bound and needs a lock of the whole space (or hand-over-hand locking along the tree), which is not measured; a transfer of capabilities by IPC between two spaces takes their range locks in a global order; slots are type-stable memory so a lock-free reader never follows a freed slot |
| Lock-free read of one slot | Lookups, and the check half of a signal | The read is a seqlock on the slot's range: the writer makes the counter odd, writes, makes it even; the reader rereads if the counter was odd or moved. The rest of the call's `W` runs after the read, on state the calling thread owns |

Unchanged with `Locks::Fine`: an unmap (it changes the page tables of the
whole address space and keeps the group lock; `b` is therefore not rerun),
and a capability change of a process spread over several groups (candidate
2 with k larger than the group), whose protocol between groups keeps the
group lock.

Latency classes since the second measurement: 0 lookups; 1 deletes (and, in
`b`, unmaps); 2 lookups of the independent processes (`c`); 3 derives.

## Workloads

| Suite | What | Measured | Why |
|---|---|---|---|
| `locks` | Candidate 3 with one group of k cores, all operations under the lock; `gap` 0 and 1 µs | Throughput, latency of a locked operation, fairness (fewest operations of a core over most) | Choose an honest lock before comparing designs |
| `a1` | k threads of one process: lookups, and 1 % or 10 % changes (derive/delete) | Throughput; latency of lookups (class 0), deletes (class 1) and derives (class 3; in the first measurement class 1 held both) | The everyday kernel use of a multithreaded process: the lock against replicas kept by messages |
| `a2` | k/2 client-server pairs of one process, IPC call and reply; the pairs are aligned with the groups (cores 0-1, 2-3, ...) | Round trips per second, round-trip latency | IPC between threads of one process, with lock contention between pairs in a group |
| `a3` | k-1 threads signal one notify on which one thread waits | Signals per second, latency of the signal call | Many-to-one completion: one lock, or all messages to one node |
| `b` | Unmap with TLB invalidation by one thread, or by all k at once; the others run user code | Unmaps per second, unmap latency | A change every core must see |
| `c` | n cores: the process of `a1` (10 % changes) on cores `0..k`, a single-thread process that only looks up its own capabilities on each other core | Latency and throughput of the independent processes (class 2); throughput of the k-thread process | Does a group's lock or message traffic disturb the others? |
| `d` | Candidate 3: group A (core 0) and group B (core 1), and core 2 that moves from one to the other 0, 100, 1000 or 10 000 times per second; 1 % of operations are unmaps inside the group | Move latency (class 1), first operation after a move (class 2), other operations (class 0), total throughput | What a move costs and how often it can be afforded. A and B alternate between one and two cores, so the load is the same with and without moves |
| `a1f`, `bf`, `cf` | As `a1`, `b`, `c` (k ≥ 2 for `a1f`), each candidate with the first and the fair settings; `a1f` also turns each switch of candidate 1 on alone | As above, plus repeated seqlock reads | Whether the conclusions survive removing the unfair points |
| `a2f`, `a3f` | As `a2`, `a3`; candidate 1 with interrupts per batch and per message | As above | Only the interrupt switch applies to IPC and signals |
| `a1t`, `a2t`, `a3t`, `ct` | As the `f` suites; groups with the group lock and with fine locks, candidate 1 with its first and fair settings | As above | Whether finer locks change the rows candidate 1 kept |

Latencies are per operation, from the caller's side, in µs: p50 / p99 /
p99.9, the median over the repetitions, with the range of the p99.9 over the
repetitions in parentheses. Throughputs are the median and the range over the
repetitions. "stalls" counts operations of 500 µs or more over all cells of a
row: a preemption of a worker thread by the host, which no kernel would
suffer.

## What this bench cannot say

- **Interrupt cost, TLB invalidation, switching mappings.** Modelled by busy
  loops of a stated length. The real `ipi` on x86-64 (x2APIC send, interrupt
  entry and exit, with the mitigations the kernel needs) must be measured on
  the bare-metal kernel; the answer to `a1` and `b` turns on it.
- **ARM broadcast TLB invalidation** (`tlbi ... is`) lets a group invalidate
  its cores' TLBs without interrupts; candidate 1 keeps one page table per
  node and still needs a message per node. Not modelled: every unmap here
  pays per-core interrupts, as on x86-64.
- **Host noise.** The host schedules the worker threads. A worker preempted
  while holding a lock, or while every other core waits for its
  acknowledgement, makes a tail no kernel would have. On macOS threads also
  migrate and land on efficiency cores. The "on P" and "stalls" columns and
  the spread of the repetitions say how far to trust a row.
- **The clock** on Apple chips ticks every 41.7 ns, so latencies below about
  0.1 µs are quantised, and a busy wait of 50 ns lasts 50 to about 90 ns
  (the same in every candidate). On x86-64 Linux the clock resolves 1 ns.
- **Kernel state** is 16 lines per process; a real kernel touches more, and a
  moved core has more to warm up than here.
