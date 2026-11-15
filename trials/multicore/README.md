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
| `--suites` | `locks,a1,a2,a3,b,c,d,a1f,a2f,a3f,bf,cf,a1t,a2t,a3t,ct,a3n,br,srv` | Which workloads to run (`f`: second measurement, `t`: third, `a3n`, `br`, `srv`: fourth, below) |
| `--reps` | 5 | Repetitions of every cell; the tables give the median and the range |
| `--ms` | 150 | Measured time of one run (after 20 ms of warm-up) |
| `--max-k` | cores - 2 | Largest number of cores given to the multithreaded process |
| `--lock` | `queue` | Lock of the group candidates: `ttas`, `ticket` or `queue` |
| `--csv` | none | One line per run, every class, for later analysis |

A full run takes about 65 minutes on 10 cores (15 for the first seven
suites, 23 for the `f` suites, 16 for the `t` suites, 10 for the fourth
series; `srv` runs 4 times `--ms`). Markdown goes to standard output,
progress to standard error. A measurement is two such runs (passes) one
after the other, each with its own `--csv` file; the results compare the
medians of the two passes cell by cell, and pool their runs.

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
process; in `srv`, the endpoint ends with no caller queued and every server
thread free (and a mutex inside the endpoint panics if its lock ever let two
cores in). Every wait has a 10 s limit that panics. `cargo test` runs every
workload under every candidate briefly, and tests the locks, the ring, the
histogram, the atomic notify under a race, the endpoint's lending rule and
the effect of the server policies.

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

Unchanged with `Locks::Fine`: an unmap keeps the group lock (the fourth
measurement adds range locks for it as a separate switch, below), and a
capability change of a process spread over several groups (candidate 2
with k larger than the group), whose protocol between groups keeps the
group lock.

Latency classes since the second measurement: 0 lookups; 1 deletes (and, in
`b`, unmaps); 2 lookups of the independent processes (`c`); 3 derives.

## Switches and suites (fourth measurement)

Three more switches, each measured beside the setting it replaces, and a
new workload. The legality conditions are assumed, not checked, as above.

| Switch or lock | Used by | What a real kernel must guarantee for it to be legal |
|---|---|---|
| `Signals::Atomic`: a notify updated by one atomic OR (`sync::Notify`) | Signals: in a group, the check half (a lock-free slot read plus `W/2`) then one fetch-or on the notify's word; the notify's lock only when its thread waits. Candidate 1: the notify's node applies the message with the same OR, without the `W/2` | The word holds 63 signal bits and a WAITING bit. Signals touch it only with atomic read-modify-writes; WAITING and the registration of the waiting thread change only under the notify's lock; the waiter sets WAITING with a fetch-or and sleeps only if that fetch-or returned no bit. Then no wake-up is lost (the argument is in `sync::Notify`; `notify_loses_no_wake_up` hammers it). The wake-up is sent after the lock is released, through the target core's mailbox and an interrupt. A wake-up may find no bit (two signals both saw WAITING); the wait loops, once per such signal. A thread waits on one notify; binding a notify to an endpoint is not measured |
| `Unmaps::Range`: a lock per range of an address space (8 ranges) | Unmaps of a process that fits in one group: the lock of the range, `W` and the range's lines, the invalidations posted to the process's cores; acknowledged after the lock is released, as with the group lock | The unmap lies inside one range (one that spans ranges would take their locks in index order: not measured); it clears leaf entries only and frees no page-table page (those stay until the address space is destroyed); a map that adds an intermediate table installs it with a compare-and-swap on the parent entry; no core waits for acknowledgements while it holds a range lock, and a core spinning for a range lock keeps answering invalidations; the frame is freed only after every acknowledgement; the membership of the group is read without its lock, so a core that joins flushes its TLB and a core that leaves acknowledges what is pending (as in `d`). **Not covered**: revoking a memory capability whose derived mappings are in several ranges or address spaces (removing a derivation subtree): it crosses ranges and spaces without bound and needs a lock of the whole space or hand-over-hand locking, whose order against the capability range locks is not settled; it is not measured |
| `Policy`: what the kernel does for a caller of a busy passive server | `srv` only | Below |

**Interrupts per message to a group.** Until this measurement every core
of a group that saw the doorbell while running user code paid an interrupt
for the same message (each charged its entry before taking the doorbell).
Now a core in an interrupt entry for its node's rings marks the node, and
another core of the node that finds the mark leaves the batch to it; a core
spinning in the kernel still takes messages at once, and what arrives
during the entry is still handled in the same batch. On a node of one core
nothing changes. Checked against the third series' binary (`a3t` and `bf`,
k ≤ 4, 3 repetitions): candidates 1 and 3 within 3 % in every cell (median
0.6 %); candidate 2 with groups of 2 at `ipi` 1 µs gains 3 to 16 % in `a3t`
(median 6 %) and up to 14 % in `bf`. So the candidate-2 cells of the first
three series where a message reaches a group of several cores (a process
spanning groups, signals to another group) overstate its interrupt cost by
up to that much at `ipi` 1 µs; their candidate-1 and candidate-3 cells are
unaffected (no message reached a group of candidate 3 there).

**Workload `srv`: the busy shared server.** A passive server runs on the
budget of its caller. Server threads are on cores `0..t` (t = 1 or 2), the
foreground client on core t, the background client on core t + 1. Layouts:
candidate 1 (every core a node); candidate 3 with the server's threads as
one group and each client alone (with one thread, this is the same code as
candidate 1: a control); candidate 3 with clients and server in one group
(`3 all`, realised as a group of 4 that covers the 3 or 4 cores). Groups use
the endpoint lock (`Locks::Fine`).

- The background client calls back to back (2 µs apart) with requests of
  50 µs; the foreground client calls every 125 to 375 µs (uniform) with
  requests of 5 µs. A request is a length of wall time on a server core.
- Budgets: every client has a period of 250 µs (the two a third of a period
  apart); background 120 µs per period, foreground 100 µs. A call carries
  the caller's budget left (its grant), the server thread charges it the
  wall time of each 1 µs chunk (interrupts included), and the reply hands
  back what is left. On a spent grant the server thread stalls until the
  grant's next period. Background requests therefore stall in the middle:
  two fit in a period, the third runs 20 µs, waits about 130 µs, and ends in
  the next period.
- The endpoint (queue, free threads, what each thread serves) lives on the
  node of server thread 0 under its lock; in candidate 1 the second server
  thread is on another node and hears from it by message (dispatch, lend,
  free). Kernel work: `W/2` on each side of each one-way trip, as in `a2`.
  The endpoint hands a call to the lowest free thread, not to the core that
  took the interrupt: with two idle threads in one group that can cost a
  second interrupt (a weakness of the model against that layout).
- (a) `fifo`: callers queue in order. The written rule "a service on the
  display path has one thread per core or its own budget" is the case
  t = 2 (the foreground always finds a free thread).
- (b) `lend`: a queued caller of a higher class lends its grant to a thread
  serving a lower class, which runs on it from its next chunk on (a stalled
  thread resumes at once), and serves the lender next; no other thread
  takes a caller whose grant is lent.
- (c) `refuse`: a call whose grant is below the endpoint's longest request
  (50 µs) is refused at once on the caller's core; a refused background
  client waits for its next period.
- Measured: foreground call latency (class 0), background calls served
  (class 1), refused calls (class 2); with the background client (contention)
  and without it (the common path, where (b) and (c) only add their checks).
- Not modelled: classes beyond lending (no third load competes for a
  server core, so a lent class only matters through the budget); budget
  accounting of the clients' own user time; budget donation across nodes
  in candidate 1 is assumed to need only the grant carried by the call and
  the reply, which a contract would have to allow.

Latency classes of the fourth series: `a3n` 0 signals that took no lock
(every signal in the locked settings), 1 signals that found the thread
waiting and took the lock; `br` 0 unmaps; `srv` as above.

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
| `a3n` | As `a3t`, in three regimes: as `a3t`; the waiting thread busy 20 µs after each wake-up (rarely waiting); producers 20 µs apart (nearly always waiting). Candidates 1 and 3, locked and atomic apply | Signals/s, signal latency, share of signals that took the lock | Whether an atomic notify takes the signal row from candidate 1 |
| `br` | As `bf`: candidate 1 fair, candidate 3 with the group lock and with range locks | Unmaps/s, unmap latency | Whether finer locks change the unmap row |
| `srv` | A passive server, a background and a foreground client; 1 or 2 server threads; with and without the background client; three policies × three layouts | Foreground latency, background calls served, refusals | What a shared server costs the display path, and what each policy costs when nobody contends |

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
