# Multicore trial: results on the Apple M5

15 November 2026, 00:57 to 01:25. Method and cost model: [README.md](README.md).
Raw runs (one line per run, every class): `data/pass-a.csv.gz`,
`data/pass-b.csv.gz`. Full tables of both passes: appendices A and B below.

**A second series** (lookups without the lock in the groups, lazy additions
and cheaper copies in candidate 1, interrupts per message) follows the first
one: [Second series](#second-series-the-unfair-points-as-switches). It changes
several conclusions of the first; read both. **A third series** (finer locks in a group, a
lock-free read of one slot) follows the second:
[Third series](#third-series-finer-locks-in-a-group). **A fourth series**
(a notify updated by one atomic OR, unmaps under range locks, a busy shared
server under three policies) follows the third, with corrections to the
earlier series: [Fourth series](#fourth-series-atomic-notify-range-locks-for-unmap-a-busy-shared-server).

## Conditions, and how far to trust them

- **Host**: Apple M5, 4 performance + 6 efficiency cores, macOS, no pinning.
  Workers asked for the user-interactive QoS class. Other jobs ran the whole
  time (another agent's compilations and property tests): load average
  between 2 and 14 during the two passes.
- **Runs**: two full passes (A then B), each 5 repetitions of every cell,
  150 ms measured per run after 20 ms of warm-up, repetitions outermost.
  Tables below pool both passes (10 runs per cell). Two earlier passes were
  discarded: they used a busy loop calibrated in iterations, and that loop
  ran up to 12 % faster or slower depending on whether a lock surrounded it,
  which changed the offered load between candidates (README, "Busy waits").
- **Reproducibility of medians**: per cell, median throughput of pass B over
  pass A: p10 0.97 to 0.99, median 1.00, p90 1.00 to 1.02 in every suite,
  except IPC (`a2`), p90 1.24. Within a pass, the spread (max - min) /
  median of throughput over the 5 repetitions has a median of 1 to 8 % per
  suite and a p90 of 3 to 66 %; IPC was the noisiest (median 44 % in pass A,
  6 % in pass B).
- **Tails**: the p99.9 often varies 2 to 10 times between repetitions of the
  same cell, and single runs show 1 to 28 ms tails: host preemption of a
  worker. **Read p50 and p99 as measurements and p99.9 as an upper bound
  polluted by the host.** The "stalls" column counts operations of 500 µs or
  more.
- **Which cores**: the "on P" column is the share of operations that ran on a
  performance core. k = 1 to 3: 90 to 100 %; k = 4: 82 to 99 %; k = 6: about
  60 %; k = 8: about 47 %. **Rows with k = 6 and 8 mix core types and say
  little about a homogeneous machine**; they are kept because the effect of
  k on the lock is large enough to show through.
- **Clock**: 41.7 ns ticks; latencies under 0.1 µs are quantised, and a 50 ns
  busy wait lasts 50 to about 90 ns (the same in every candidate).
- **Baseline**: one core alone does 0.70 M calls/s at W = 50 ns, 0.64 M at
  200 ns, 0.43 M at 1000 ns (a call plus 1 µs of user time plus the
  measurement's own clock reads). Every candidate is within 1 % of this at
  k = 1.
- **Interrupts and TLB invalidation are modelled, not measured** (`ipi` 0 or
  1 µs, `tlb` 100 ns). The answer to `a1` and `b` turns on `ipi`.

Cell format below: throughput (median of 10); latency p50 / p99.9 in µs.
"1" is candidate 1 (one core per node), "2/2" and "2/4" candidate 2 with
groups of 2 and 4, "3" candidate 3 (one group sized to the process). User
time between calls: 1 µs.

## Locks (one group of k cores, every operation under the lock)

| gap ns | W ns | k | TTAS + backoff | ticket | queue (Anderson) |
|---|---|---|---|---|---|
| 0 | 50 | 4 | 5.61M; 0.29 / 46.1 | 5.81M; 0.62 / 1.1 | 6.08M; 0.58 / 1.0 |
| 0 | 200 | 4 | 3.07M; 0.33 / 46.1 | 3.25M; 1.12 / 2.3 | 3.36M; 1.12 / 4.0 |
| 0 | 1000 | 4 | 893k; 2.14 / 196 | 903k; 4.29 / 11.5 | 913k; 4.29 / 12.3 |
| 0 | 200 | 8 | 2.75M; 0.58 / 96.8 | 2.42M; 3.10 / 12.1 | 2.64M; 2.85 / 10.7 |
| 1000 | 200 | 4 | 2.44M; 0.25 / 0.8 | 2.41M; 0.29 / 0.7 | 2.43M; 0.29 / 0.8 |
| 1000 | 1000 | 4 | 846k; 2.11 / 76.3 | 905k; 2.98 / 13.5 | 904k; 2.98 / 12.1 |
| 1000 | 1000 | 8 | 829k; 2.14 / 220 | 824k; 8.19 / 21.0 | 842k; 7.97 / 22.7 |

The backoff lock has the best median and the worst tail by one to two orders
of magnitude under contention (p99.9 30 to 620 µs), and is unfair (the slowest
core does 63 to 96 % of the operations of the fastest at `gap` 0). Ticket and queue locks are
fair (1.00) and within 5 % of each other, the queue lock ahead by 2 to 10 % at
6 to 8 cores with no user time. **The other suites use the queue lock**: a
kernel needs the bounded wait more than the median, and on more cores a lock
whose waiters spin on their own line degrades less (from memory, the usual
result for MCS-type locks; here seen only up to 8 cores).

A single lock saturates near 1 / (W + handover): about 0.91 M operations/s at
W = 1 µs, whatever the number of cores. That ceiling drives most of what
follows.

## a. Kernel objects of one process from k threads

### a1. Capability lookups, and 1 % or 10 % changes (derive/delete)

Throughput; lookup p50 / p99.9:

| change % | ipi | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|---|
| 1 | 1000 | 50 | 4 | 2.65M; 0.08 / 0.2 | 2.56M; 0.15 / 0.5 | 2.61M; 0.17 / 0.6 | 2.58M; 0.17 / 0.6 |
| 1 | 1000 | 200 | 4 | 2.44M; 0.21 / 0.3 | 2.34M; 0.29 / 0.7 | 2.32M; 0.37 / 0.9 | 2.33M; 0.35 / 0.8 |
| 1 | 1000 | 1000 | 4 | 1.62M; 1.00 / 1.1 | 1.58M; 1.07 / 2.3 | 914k; 2.98 / 10.9 | 913k; 2.98 / 11.7 |
| 1 | 1000 | 200 | 8 | 4.62M; 0.21 / 0.3 | 4.32M; 0.33 / 1.1 | 4.28M; 0.37 / 1.3 | 2.78M; 1.43 / 13.3 |
| 1 | 1000 | 1000 | 8 | 3.13M; 1.00 / 1.1 | 2.98M; 1.15 / 3.9 | 1.65M; 3.33 / 11.9 | 837k; 7.90 / 23.6 |
| 10 | 0 | 200 | 4 | 2.21M; 0.21 / 0.3 | 2.20M; 0.29 / 0.7 | 2.39M; 0.29 / 0.7 | 2.38M; 0.31 / 0.8 |
| 10 | 1000 | 50 | 4 | 1.98M; 0.08 / 0.2 | 2.06M; 0.17 / 0.5 | 2.59M; 0.17 / 0.6 | 2.59M; 0.17 / 0.6 |
| 10 | 1000 | 200 | 4 | 1.82M; 0.21 / 0.3 | 1.90M; 0.29 / 0.7 | 2.38M; 0.29 / 0.7 | 2.39M; 0.29 / 0.8 |
| 10 | 1000 | 1000 | 4 | 1.20M; 1.00 / 1.1 | 1.27M; 1.07 / 2.3 | 911k; 2.98 / 11.0 | 912k; 2.98 / 10.9 |
| 10 | 1000 | 50 | 8 | 3.14M; 0.08 / 0.2 | 3.18M; 0.21 / 1.0 | 3.44M; 0.25 / 1.1 | 4.19M; 0.48 / 4.1 |
| 10 | 1000 | 200 | 8 | 2.85M; 0.21 / 0.3 | 2.93M; 0.33 / 1.3 | 3.08M; 0.42 / 1.7 | 2.67M; 1.57 / 10.1 |
| 10 | 1000 | 1000 | 8 | 1.87M; 1.00 / 1.1 | 1.91M; 1.15 / 3.1 | 1.42M; 3.20 / 10.6 | 836k; 8.06 / 21.6 |

Change latency, p50 / p99 / p99.9:

| change % | ipi | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|---|
| 10 | 0 | 200 | 4 | 1.20 / 2.38 / 16.4 | 1.20 / 2.07 / 6.2 | 0.37 / 0.54 / 0.9 | 0.37 / 0.56 / 0.9 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.79 / 13.5 | 1.87 / 2.90 / 6.9 | 0.25 / 0.48 / 0.7 | 0.25 / 0.48 / 0.8 |
| 10 | 1000 | 200 | 4 | 3.10 / 4.70 / 21.6 | 2.11 / 3.09 / 8.2 | 0.37 / 0.54 / 0.9 | 0.37 / 0.56 / 0.9 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.68 / 23.2 | 3.81 / 6.75 / 13.8 | 3.07 / 3.54 / 11.0 | 3.07 / 3.38 / 10.9 |
| 10 | 1000 | 200 | 8 | 4.06 / 9.79 / 36.1 | 3.90 / 6.82 / 15.2 | 2.80 / 5.76 / 11.5 | 1.66 / 2.27 / 10.0 |
| 10 | 1000 | 1000 | 8 | 6.59 / 12.22 / 24.6 | 6.34 / 12.48 / 37.1 | 9.79 / 18.69 / 39.9 | 8.13 / 9.22 / 22.0 |

What the numbers say:

- **The lookup of candidate 1 never depends on k**: p50 = W (0.08, 0.21,
  1.00 µs), p99.9 within 0.1 µs of it. In a group the lookup grows with the
  group: at W = 1 µs, p50 2.98 µs with 4 cores (3 times W) and 8 µs with 8
  (low trust, E cores), p99.9 11 to 24 µs. This is the cost D-026 named ("a
  lock makes the duration of a call depend on other cores"), now with a size.
- **Changes are where candidate 1 pays**: 3 to 5 µs p50 with a 1 µs interrupt,
  against 0.25 to 0.4 µs in a group at W ≤ 200 ns, k = 4 (8 to 12 times).
  With `ipi` = 0 (perfect delivery) the gap is 3 times (1.2 against 0.37 µs).
- **Throughput depends on W times k times the change rate.** With 1 %
  changes candidate 1 is ahead or level in every row: by under 5 % at
  W ≤ 200 ns and k = 4, by 1.8 times at W = 1 µs and k = 4 (the lock is
  saturated), by 3.7 to 3.9 times at W = 1 µs and k = 8. With 10 % changes and a 1 µs interrupt, the
  group is ahead by 31 % at W ≤ 200 ns and k = 4 (each change interrupts the
  three other cores of candidate 1), and candidate 1 is ahead by 32 % at
  W = 1 µs. Groups of 2 sit between, close to candidate 1.

### a2. IPC call and reply (k/2 pairs, each pair inside a group when it can be)

| ipi | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|
| 0 | 200 | 4 | 558k; 2.14 / 7.6 | 538k; 2.26 / 8.0 | 471k; 2.78 / 9.3 | 462k; 2.80 / 8.9 |
| 1000 | 50 | 4 | 385k; 3.78 / 8.9 | 368k; 3.94 / 11.4 | 346k; 4.26 / 12.5 | 354k; 4.29 / 10.7 |
| 1000 | 200 | 4 | 354k; 4.16 / 11.3 | 346k; 4.29 / 12.1 | 333k; 4.64 / 11.3 | 329k; 4.64 / 14.1 |
| 1000 | 1000 | 4 | 280k; 5.63 / 13.5 | 271k; 5.82 / 14.3 | 263k; 6.21 / 15.2 | 261k; 6.21 / 16.3 |
| 1000 | 200 | 8 | 712k; 4.22 / 11.9 | 692k; 4.42 / 12.5 | 632k; 4.70 / 20.2 | 608k; 4.86 / 28.4 |
| 1000 | 1000 | 8 | 558k; 5.70 / 21.5 | 522k; 5.89 / 38.0 | 480k; 6.27 / 95.2 | 313k; 9.73 / 354 |

Round trips per second; round-trip p50 / p99.9. A cross-core IPC costs a
wake-up in every candidate, so the designs are close: candidate 1 is ahead by
2 to 9 % with interrupts, by up to 20 % without, and by 1.8 times at
W = 1 µs and k = 8, where four pairs share one lock in candidate 3. Pairs
inside a pair group (2/2) are within 2 to 5 % of candidate 1.

### a3. k - 1 threads signal one notify

| ipi | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|
| 1000 | 50 | 4 | 2.10M; 0.08 / 0.2 | 1.45M; 0.21 / 0.4 | 1.97M; 0.17 / 0.8 | 1.96M; 0.17 / 0.7 |
| 1000 | 200 | 4 | 1.95M; 0.21 / 0.3 | 1.37M; 0.29 / 0.7 | 1.65M; 0.42 / 1.0 | 1.64M; 0.42 / 1.1 |
| 1000 | 1000 | 4 | 1.57M; 0.58 / 0.7 | 1.06M; 0.68 / 1.9 | 723k; 2.72 / 11.6 | 721k; 2.72 / 12.9 |
| 1000 | 200 | 8 | 4.45M; 0.25 / 0.5 | 2.68M; 0.50 / 20.1 | 1.95M; 0.58 / 4.0 | 1.60M; 2.72 / 12.1 |
| 1000 | 1000 | 8 | 1.86M; 0.64 / 340 | 1.25M; 1.62 / 9.8 | 1.16M; 2.13 / 9.2 | 694k; 8.51 / 22.5 |

Signals per second; latency of the signal call p50 / p99.9. A signal to
another node returns once the message is in the ring, so candidate 1 is
ahead of candidate 3: by 7 to 19 % at W ≤ 200 ns and k = 4, by 2.2 times at
W = 1 µs. Groups of 2 are the worst at W ≤ 200 ns (the waiter shares its
group's lock with a producer) and second at W = 1 µs. Its
limit is the notify's node, which applies every signal: at W = 1 µs and
k = 8 that node saturates, rings fill, and the p99.9 jumps to 340 µs
(back-pressure), where the groups stay at 9 to 22 µs.

## b. An unmap every core of the process must see

| initiators | ipi | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|---|
| one | 0 | 200 | 4 | 405k; 1.07 / 6.6 | 352k; 1.49 / 8.1 | 448k; 0.87 / 6.9 | 448k; 0.87 / 5.6 |
| one | 1000 | 200 | 4 | 284k; 2.11 / 8.2 | 207k; 3.42 / 9.1 | 308k; 1.87 / 7.8 | 308k; 1.87 / 8.1 |
| one | 1000 | 1000 | 4 | 194k; 3.71 / 12.7 | 155k; 4.99 / 12.2 | 241k; 2.66 / 10.9 | 242k; 2.72 / 10.0 |
| one | 1000 | 200 | 8 | 240k; 2.78 / 10.1 | 174k; 4.29 / 14.1 | 163k; 4.42 / 12.4 | 273k; 2.30 / 9.3 |
| all | 1000 | 50 | 4 | 705k; 2.37 / 10.7 | 572k; 3.20 / 12.0 | 1.09M; 2.13 / 7.8 | 1.09M; 2.14 / 7.6 |
| all | 1000 | 200 | 4 | 643k; 2.66 / 10.3 | 550k; 3.52 / 10.7 | 979k; 2.66 / 9.3 | 978k; 2.69 / 7.9 |
| all | 1000 | 1000 | 4 | 412k; 5.02 / 17.2 | 387k; 7.10 / 22.0 | 550k; 5.82 / 16.0 | 547k; 5.86 / 18.3 |
| all | 1000 | 200 | 8 | 678k; 5.98 / 19.3 | 558k; 8.13 / 22.3 | 460k; 9.92 / 27.3 | 869k; 5.54 / 17.3 |

Unmaps per second; unmap p50 / p99.9. One group covering the process
(candidate 3, or 2/4 with k ≤ 4) is fastest: with one initiator, 11 % lower
p50 than candidate 1 at W = 200 ns and 27 % at W = 1 µs (candidate 1 applies
the change twice on the critical path, at home and on each copy); with every
thread unmapping, 1.33 to 1.55 times the throughput. Both pay one interrupt
per other core, so the gap is the extra apply and the extra hop, not the
interrupt. **A fixed group smaller than the process is the worst case**
(2/2 at k = 4, 2/4 at k = 8): the change crosses nodes and then fans out
inside each, and its p50 is 1.3 to 1.9 times that of candidate 3.

## c. A k-thread process beside independent single-thread processes

Lookup latency of the independent processes, p50 / p99 / p99.9:

| n | W | k | 1 | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|---|
| 4 | 200 | 2 | 0.21 / 0.25 / 0.29 | 0.29 / 0.42 / 0.58 | 0.29 / 0.46 / 0.74 | 0.21 / 0.25 / 0.29 |
| 4 | 1000 | 1 | 1.00 / 1.04 / 1.12 | 1.07 / 1.20 / 1.90 | 2.94 / 3.23 / 12.42 | 1.00 / 1.04 / 1.12 |
| 4 | 1000 | 3 | 1.00 / 1.07 / 1.15 | 1.07 / 2.11 / 2.24 | 2.98 / 3.36 / 14.59 | 1.00 / 1.04 / 1.12 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 | 0.33 / 0.62 / 0.91 | 0.42 / 1.12 / 1.66 | 0.21 / 0.29 / 0.29 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.12 | 1.15 / 1.90 / 2.40 | 3.20 / 3.65 / 10.50 | 1.00 / 1.07 / 1.12 |

(Pass A; pass B within the noise, see appendix B.) In candidates 1 and 3 the
independent processes see nothing of the big process: same p50, p99 and
p99.9 as alone. With fixed groups they share a lock with whoever else is in
their group: groups of 2 add 7 to 57 % to the p50 and up to double the p99;
groups of 4 at W = 1 µs triple the p50, multiply the p99.9 by about 10, and
halve their throughput. This is a property of candidate 2, not of locks: candidate 3
keeps the lock inside the process that asked for the group.

## d. Moving a core between nodes (candidate 3)

| switch | W | moves/s | throughput, 3 cores | move p50 / p99.9 | first op after move p50 / p99.9 | other ops p50 / p99.9 |
|---|---|---|---|---|---|---|
| 0 | 200 | 0 | 1.84M | – | – | 0.25 / 2.2 |
| 0 | 200 | 10 000 | 1.84M | 0.62 / 2.8 | 0.29 / 2.2 | 0.29 / 2.2 |
| 0 | 1000 | 0 | 1.24M | – | – | 1.04 / 3.9 |
| 0 | 1000 | 10 000 | 1.22M | 2.24 / 4.7 | 1.07 / 3.9 | 1.04 / 3.9 |
| 1000 | 200 | 10 000 | 1.84M | 1.66 / 5.8 | 0.29 / 2.2 | 0.29 / 2.2 |
| 1000 | 1000 | 10 000 | 1.22M | 3.26 / 11.9 | 1.07 / 4.8 | 1.04 / 3.9 |

A move costs the kernel work at each end (2 W, modelled), the switch of
mappings (modelled), and about 0.2 µs of measured protocol (two lock
acquisitions, the membership lines, the drain of pending invalidations). The
first operation on the new node is not measurably slower: with 16 lines of
state per process there is little to warm up (a real kernel has more).
Neither the other cores' latency nor the total throughput moves beyond the
noise up to 1000 moves per second; at 10 000 per second the throughput of
the three cores drops by 1 to 2 % at W = 1 µs, consistent with 2 to 3 µs
lost per move. At about 3 µs per move, 3000 moves per second cost about 1 %
of one core. The cost of a move is not the obstacle to candidate 3
on these numbers; its protocol (membership, in-flight invalidations) is what
needs the formal model.

## Which candidate each workload favours (on this host, with these parameters)

| Workload | Favours | By how much | Depends on |
|---|---|---|---|
| Lookups (every call) | 1 | Latency independent of k; in a group of 4 the p50 is 1.4 to 3 times candidate 1's, more at 8 | W and k |
| Capability changes | 3 (or 2/4 if k ≤ 4) | 8 to 12 times lower latency at W ≤ 200 ns with a 1 µs interrupt | `ipi`, the change rate |
| Throughput of a1 | 1 with 1 % changes; 3 with 10 % changes and W ≤ 200 ns; 1 at W = 1 µs | Under 5 % to 3.9 times | W, k, change rate, `ipi` |
| IPC | 1, narrowly | 2 to 9 % (1.8 times at W = 1 µs, k = 8) | Lock sharing between pairs |
| Many-to-one signal | 1 | 7 % to 2.2 times; tail breaks when the notify's node saturates | W |
| Unmap | 3 | 11 to 27 % lower latency, 1.3 to 1.55 times throughput | The process fitting in one group |
| Other processes' latency | 1 and 3 equally | Fixed groups of 4: 3 times p50, 10 times p99.9 | Candidate 2 only |
| Core move | 3 is affordable | About 0.2 µs of protocol plus modelled work | – |

No candidate wins everywhere. Candidate 2 with fixed groups is best in a few
rows of `a1` (by up to 8 %, with 10 % changes, where it sits between the
other two) and the worst in `b` and `c`; the
contest these numbers leave open is between candidates 1 and 3, and it is
decided by three quantities the Mac cannot measure: the real cost of an
interrupt, the real kernel work per call, and the rate of changes to shared
structures in real programs.

## What the Mac cannot tell, to measure on x86-64

- **Interrupts and TLB invalidation**: modelled at 0 or 1 µs and 100 ns. The
  real IPI round trip and `invlpg` cost, with the mitigations the kernel
  needs, decide `a1` and `b`. This needs the bare-metal kernel, not this
  bench.
- **Pinned, uniform cores**: here k ≥ 4 is partly on efficiency cores and
  nothing is pinned. On Linux the bench pins one worker per physical core.
- **Topology**: a desktop x86-64 has no efficiency cores but may have
  several L3 domains (AMD CCDs), where a line costs much more across domains:
  groups that straddle them, and messages between them, should be measured
  separately; fixed groups that match an L3 domain are the case for
  candidate 2.
- **SMT pairs** (the smallest group, using the hardware thread D-028 leaves
  idle): the M5 has no SMT.
- **Clock resolution**: 1 ns on x86-64 instead of 41.7 ns.
- **ARM broadcast TLB invalidation** would favour groups further; not
  modelled, not testable from user space.

## Where the simulation could be unfair

Against candidate 1:
- Every copy applies the full W of a change; a copy update could be cheaper
  than the original operation.
- Every change waits for every acknowledgement. An addition (derive, map)
  could be applied lazily, as the TLB guarantee already allows ("translations
  that were added may cause no fault"); only removals need the wait.
- One home node serialises every change of the process; ownership per object
  (the node that holds a capability) would spread that load.
- Each message to a running core pays a full interrupt, coalesced only per
  batch.

Against candidates 2 and 3:
- One lock covers everything, lookups included. Lookups without the lock
  (read-copy-update) or per-object locks would remove most of the lookup
  cost measured here; that is a different, harder kernel to verify.
- A user-space lock holder can be preempted by the host; a kernel would run
  with interrupts off. The p99.9 of groups suffers more from this than the
  p99.9 of candidate 1. The "stalls" column bounds it.

For candidate 2 or 3, or neutral:
- IPC pairs are placed inside groups when they fit; a placement that splits
  pairs would hurt candidate 2.
- In a group, acknowledgements of an unmap are atomic decrements in shared
  memory; in candidate 1 they are messages. That is the design, not a bias,
  but it is where candidate 3's advantage in `b` comes from.
- Kernel state is 16 lines per process; a real kernel touches more lines per
  call, which would raise the cost of a lock handover and of a move.

The bench does not measure memory: candidate 1 keeps a copy of the page
tables and of the capability replicas per node of the process.


## Second series: the unfair points as switches

15 November 2026, 01:55 to 02:41. Three choices of the first series favoured
one side; each is now a switch (README, "Switches"), and every cell below
runs both settings side by side in the same pass:

1. **Lookups without the lock** in candidates 2 and 3: a seqlock per
   capability space. Changes still take the queue lock and make the counter
   odd; a reader that overlaps one waits or reads again. The read section is
   the whole `W` of the lookup (pessimistic for the reader).
2. **Lazy additions** (a derive waits only for its own node's copy; a delete
   and an unmap still wait for every copy), and **a cheaper copy** (a
   non-home copy costs 25 % of `W` instead of 100 %), for candidate 1 and for
   candidate 2 between groups.
3. **Interrupts per batch or per message** for candidate 1. A correction to
   the first report: the first series already charged one interrupt per
   batch (every message already in the rings for one `ipi`). That was not an
   unfair point. The switch adds the pessimistic setting, one `ipi` per
   message, to show what batching is worth.

"first" = the settings of the first series; "fair" = switches 1 and 2 on,
interrupts per batch. Two full passes of 5 repetitions; the tables pool them
(10 runs per cell). Per cell, median throughput of pass 2 over pass 1: median
1.00 to 1.01, p90 at most 1.04 in every suite; within-pass spread median 1 to
2 %. Load average 4 to 9 (other jobs running). Rows with k = 8 ran about half
on efficiency cores, as before; rows with k = 4 are the ones to trust.

Seqlock reads repeated (a writer came in during the read) per 1000 lookups,
over the whole suite: in `a1f` 18 (groups of 2), 33 (groups of 4), 54
(candidate 3); in `cf` 9, 22, 33. The time a reader spends waiting for a
writer is in its latency, below.

### What changed against the first series (k = 4 unless stated)

- **a1, 1 % changes**: the candidates are now level, within 4 %, at every
  W (first series: candidate 1 ahead by up to 1.8 times at W = 1 µs,
  because one lock saturated near 0.91 M operations/s). Lock-free lookups
  remove that ceiling: candidate 3 fair does 1.68 M at W = 1 µs, the same as
  candidate 1. At k = 8, candidate 3 fair is ahead by 0 to 8 %.
- **a1, 10 % changes, 1 µs interrupt**: candidate 3 fair is ahead at every W:
  by 33 % (W = 50 ns), 31 % (200 ns) and 17 % (1 µs) over candidate 1 fair;
  at k = 8 by 1.6, 1.5 and 1.2 times. The first series had candidate 1 ahead
  by 32 % at W = 1 µs; that reversal is the main change. With no interrupt
  cost (`ipi` 0) the gap is 3 to 8 %.
- **Lookup latency**: with the seqlock the group's p50 equals W, as in
  candidate 1 (first series: 3 times W at k = 4, 8 times at k = 8). The tail
  is what remains: p99 up to 3.5 times W and p99.9 up to 5 times W at
  W = 1 µs with 10 % changes (5 and 8 times at k = 8), against about 1.05
  times W in candidate 1. That is the honest cost of a reader overlapping
  writers.
- **Changes**: lazy additions (with the cheaper copy) cut candidate 1's
  derive p50 by 43 to 45 % (2.85 to 1.57 µs at W = 50 ns), and a 25 % copy cuts its delete p50 by up
  to 13 %. Groups still win both: candidate 3 fair does a derive or a delete
  in 0.25 to 1.2 µs against 1.6 to 4.4 µs for candidate 1 fair. Lock-free
  lookups also shorten the group's changes (at k = 8, W = 1 µs: delete p50
  8.2 to 1.4 µs), because readers no longer queue on the lock in front of
  writers.
- **Each switch of candidate 1 alone** (a1f, 10 %, `ipi` 1 µs, W = 1 µs,
  k = 4): lazy additions +2.5 %, cheaper copy +7 %, both +10 % (1.20 to
  1.32 M operations/s). Neither closes the gap to candidate 3 fair (1.55 M).
- **Interrupts per message** cost candidate 1 0 to 10 % in a1, up to 21 % in
  b when every thread unmaps, nothing measurable in IPC, and break the
  many-to-one signal: at `ipi` 1 µs its throughput falls to 0.2 to 0.46
  times and its p99.9 rises to 0.85 to 6 ms (the notify's core pays
  1 µs per signal). Batching, already in the first series, is essential to
  candidate 1 there.
- **IPC and signals**: no switch applies to the groups (both write state and
  keep the lock). Unchanged: candidate 1 ahead of candidate 3 by 4 to 7 % in
  IPC at k = 4 (1.7 to 2.3 times at k = 8, W = 1 µs), and by 7 % to 2.2
  times in signals.
- **Unmap**: the cheaper copy narrows candidate 3's lead with one initiator:
  p50 1.87 against 1.95 µs at W = 200 ns (4 %, first series 11 %), 2.66
  against 2.94 µs at W = 1 µs (10 %, first series 27 %). With every thread
  unmapping and a 1 µs interrupt, candidate 3 keeps 1.44 to 1.49 times the
  throughput at W ≤ 200 ns and 1.07 times at W = 1 µs; with `ipi` 0,
  candidate 1 fair is ahead at every W, by 6 to 21 %.
- **Independent processes (c)**: with lock-free lookups, fixed groups no
  longer disturb independent processes that only look up: same p50, p99 and
  p99.9 as candidates 1 and 3 (first series: groups of 4 tripled their p50 at
  W = 1 µs). Their operations that write (IPC, signals, changes) still share
  the group's lock; `c` does not measure those, so the first series' penalty
  of fixed groups still holds for them.

### Which candidate each workload favours under the fair settings

| Workload | Favours | By how much (k = 4; k = 8 in parentheses, low trust) |
|---|---|---|
| a1, 1 % changes | Level | Within 4 % (candidate 3 by 0 to 8 %) |
| a1, 10 % changes, `ipi` 1 µs | 3 | 17 to 33 % (1.2 to 1.6 times) |
| a1, 10 % changes, `ipi` 0 | 3, narrowly | 3 to 8 % (up to 1.2 times) |
| Lookup latency | 1 in the tail only | Same p50; group p99 up to 3.5 times W, p99.9 up to 5 times W (8 at k = 8) |
| Change latency | 3 | 2.3 to 11 times lower p50 with a 1 µs interrupt |
| IPC | 1, narrowly | 4 to 7 % (1.7 to 2.3 times at W = 1 µs) |
| Many-to-one signal | 1 | 7 % to 2.2 times; needs batched interrupts |
| Unmap, one initiator | 3, narrowly | 4 to 10 % lower p50 |
| Unmap, all threads | 3 with `ipi` 1 µs; 1 with `ipi` 0 | 1.07 to 1.49 times; 1.06 to 1.21 times |
| Independent processes, lookups | Level | Fixed groups now equal; writes not measured |

### What the second series still cannot decide on this Mac

- The real interrupt cost: the 10 % rows turn on it (3 to 8 % gap at
  `ipi` 0, 17 to 33 % at 1 µs). It needs the bare-metal kernel on x86-64.
- The real change rate of programs and the real W: with 1 % changes the
  candidates are level.
- The real cost of a copy (25 % or 100 % of W), and whether the contract
  accepts lazy additions (another thread may briefly see the new slot
  empty).
- The seqlock read section: here the whole W; a real lookup reads the slot in
  a fraction of the call, so the group's tail is overstated. It needs the
  kernel's code.
- Writes in a shared group (IPC, signals) still take one lock; a
  finer-grained group kernel is not measured.
- Everything listed for the first series about the host: no pinning,
  efficiency cores at k ≥ 6, 41.7 ns clock, host preemption in the p99.9.

### Tables (pooled, 10 runs per cell)

Cells: throughput; or latency p50 / p99 / p99.9 in µs; or throughput; p50 /
p99.9. Full tables of both passes: appendices C and D. Raw runs:
`data/pass-f1.csv.gz`, `data/pass-f2.csv.gz` (in those files the column name
"1 fair, irq/msg" holds an unquoted comma; later runs write it with a
semicolon).

a1f throughput, 1 % changes, `ipi` 1 µs (labels: change %, ipi ns, W ns, k):

| labels | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 1000 50 4 | 2.69M | 2.70M | 2.69M | 2.70M | 2.70M | 2.63M | 2.72M | 2.68M | 2.80M | 2.69M | 2.79M |
| 1 1000 50 8 | 5.07M | 5.12M | 5.07M | 5.13M | 5.11M | 4.75M | 5.13M | 4.55M | 5.23M | 3.88M | 5.55M |
| 1 1000 200 4 | 2.46M | 2.48M | 2.48M | 2.49M | 2.49M | 2.40M | 2.51M | 2.45M | 2.57M | 2.45M | 2.57M |
| 1 1000 200 8 | 4.70M | 4.73M | 4.71M | 4.76M | 4.75M | 4.39M | 4.76M | 4.26M | 4.86M | 2.57M | 5.10M |
| 1 1000 1000 4 | 1.63M | 1.63M | 1.64M | 1.65M | 1.65M | 1.59M | 1.66M | 918k | 1.68M | 915k | 1.68M |
| 1 1000 1000 8 | 3.15M | 3.18M | 3.23M | 3.25M | 3.24M | 2.94M | 3.26M | 1.64M | 3.29M | 834k | 3.35M |

a1f throughput, 10 % changes, `ipi` 0:

| labels | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 10 0 50 4 | 2.50M | 2.54M | 2.53M | 2.57M | 2.56M | 2.46M | 2.58M | 2.65M | 2.75M | 2.65M | 2.74M |
| 10 0 50 8 | 4.14M | 4.34M | 4.21M | 4.41M | 4.41M | 3.96M | 4.32M | 4.02M | 4.66M | 4.14M | 5.29M |
| 10 0 200 4 | 2.24M | 2.28M | 2.30M | 2.33M | 2.33M | 2.22M | 2.35M | 2.41M | 2.50M | 2.42M | 2.52M |
| 10 0 200 8 | 3.76M | 3.89M | 3.89M | 4.02M | 4.03M | 3.56M | 4.00M | 3.51M | 4.28M | 2.69M | 4.71M |
| 10 0 1000 4 | 1.37M | 1.39M | 1.48M | 1.50M | 1.51M | 1.36M | 1.49M | 911k | 1.55M | 914k | 1.55M |
| 10 0 1000 8 | 2.25M | 2.26M | 2.58M | 2.61M | 2.61M | 2.04M | 2.51M | 1.38M | 2.57M | 842k | 2.58M |

a1f throughput, 10 % changes, `ipi` 1 µs:

| labels | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 10 1000 50 4 | 1.99M | 2.05M | 2.02M | 2.07M | 2.03M | 2.08M | 2.20M | 2.65M | 2.74M | 2.66M | 2.75M |
| 10 1000 50 8 | 3.14M | 3.29M | 3.20M | 3.32M | 2.99M | 3.13M | 3.40M | 3.37M | 3.81M | 4.08M | 5.28M |
| 10 1000 200 4 | 1.84M | 1.89M | 1.86M | 1.92M | 1.88M | 1.91M | 2.04M | 2.42M | 2.51M | 2.42M | 2.51M |
| 10 1000 200 8 | 2.92M | 2.99M | 3.00M | 3.10M | 2.83M | 2.83M | 3.14M | 2.92M | 3.56M | 2.65M | 4.71M |
| 10 1000 1000 4 | 1.20M | 1.23M | 1.28M | 1.32M | 1.30M | 1.26M | 1.39M | 913k | 1.55M | 915k | 1.55M |
| 10 1000 1000 8 | 1.89M | 1.89M | 2.05M | 2.11M | 2.06M | 1.89M | 2.23M | 1.39M | 2.36M | 838k | 2.59M |

a1f lookup latency p50 / p99 / p99.9, µs, 10 % changes, `ipi` 1 µs:

| labels | 1 first | 1 fair | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair |
|---|---|---|---|---|---|---|---|---|
| 10 1000 50 4 | 0.08 / 0.12 / 0.2 | 0.08 / 0.12 / 0.2 | 0.12 / 0.35 / 0.5 | 0.08 / 0.25 / 0.3 | 0.12 / 0.39 / 0.5 | 0.08 / 0.29 / 0.4 | 0.12 / 0.37 / 0.5 | 0.08 / 0.29 / 0.4 |
| 10 1000 50 8 | 0.08 / 0.17 / 0.2 | 0.08 / 0.17 / 0.2 | 0.21 / 0.74 / 1.1 | 0.12 / 0.33 / 0.6 | 0.29 / 0.91 / 1.3 | 0.12 / 0.42 / 0.6 | 0.60 / 1.22 / 1.7 | 0.12 / 0.50 / 0.8 |
| 10 1000 200 4 | 0.21 / 0.25 / 0.3 | 0.21 / 0.25 / 0.3 | 0.29 / 0.58 / 0.7 | 0.21 / 0.54 / 0.8 | 0.29 / 0.52 / 0.7 | 0.21 / 0.66 / 1.0 | 0.29 / 0.50 / 0.7 | 0.21 / 0.66 / 1.0 |
| 10 1000 200 8 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.37 / 1.07 / 1.5 | 0.25 / 0.74 / 1.1 | 0.50 / 1.46 / 2.1 | 0.25 / 0.87 / 1.4 | 1.62 / 2.29 / 8.3 | 0.25 / 1.41 / 2.2 |
| 10 1000 1000 4 | 1.00 / 1.04 / 1.1 | 1.00 / 1.04 / 1.0 | 1.07 / 2.11 / 2.3 | 1.00 / 2.70 / 3.7 | 2.98 / 3.38 / 8.9 | 1.00 / 3.52 / 5.0 | 2.98 / 3.25 / 7.8 | 1.00 / 3.52 / 4.9 |
| 10 1000 1000 8 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.20 / 2.50 / 3.1 | 1.04 / 3.33 / 5.2 | 3.23 / 4.54 / 9.3 | 1.04 / 4.70 / 7.2 | 8.06 / 8.83 / 18.3 | 1.07 / 5.25 / 7.7 |

a1f delete latency, 10 % changes, `ipi` 1 µs:

| labels | 1 first | 1 fair | 1 fair, irq/msg | 2/2 fair | 2/4 fair | 3 first | 3 fair |
|---|---|---|---|---|---|---|---|
| 10 1000 50 4 | 2.85 / 3.86 / 14.0 | 2.78 / 3.79 / 10.2 | 2.78 / 4.67 / 10.2 | 1.79 / 2.53 / 2.8 | 0.25 / 0.52 / 0.7 | 0.25 / 0.52 / 0.7 | 0.25 / 0.50 / 0.7 |
| 10 1000 50 8 | 3.68 / 6.46 / 15.4 | 3.65 / 6.24 / 15.4 | 4.06 / 9.79 / 19.1 | 3.65 / 6.05 / 9.5 | 2.37 / 4.45 / 5.4 | 0.70 / 1.38 / 2.3 | 0.50 / 1.11 / 1.5 |
| 10 1000 200 4 | 3.10 / 3.86 / 8.5 | 2.98 / 3.76 / 10.5 | 2.98 / 5.12 / 15.6 | 2.00 / 2.78 / 3.6 | 0.42 / 0.72 / 0.9 | 0.37 / 0.64 / 0.9 | 0.42 / 0.77 / 1.1 |
| 10 1000 200 8 | 4.03 / 6.91 / 15.2 | 3.90 / 6.56 / 12.7 | 4.32 / 10.56 / 18.9 | 4.06 / 6.91 / 9.9 | 2.53 / 4.54 / 5.7 | 1.72 / 2.46 / 8.4 | 0.58 / 1.41 / 1.8 |
| 10 1000 1000 4 | 4.99 / 7.26 / 13.9 | 4.35 / 6.27 / 12.5 | 4.42 / 7.78 / 12.3 | 2.94 / 4.13 / 6.1 | 1.20 / 2.42 / 3.6 | 3.07 / 3.34 / 7.6 | 1.20 / 2.37 / 3.5 |
| 10 1000 1000 8 | 6.59 / 11.01 / 16.8 | 6.05 / 11.58 / 20.7 | 6.24 / 14.21 / 20.5 | 5.22 / 9.66 / 14.0 | 3.55 / 6.66 / 10.1 | 8.19 / 9.09 / 19.2 | 1.41 / 4.02 / 5.4 |

a1f derive latency, 10 % changes, `ipi` 1 µs:

| labels | 1 first | 1 lazy adds | 1 fair | 2/2 fair | 2/4 fair | 3 fair |
|---|---|---|---|---|---|---|
| 10 1000 50 4 | 2.85 / 3.90 / 13.1 | 1.64 / 2.53 / 5.4 | 1.57 / 2.48 / 5.6 | 0.50 / 2.40 / 2.7 | 0.25 / 0.48 / 0.7 | 0.25 / 0.50 / 0.6 |
| 10 1000 50 8 | 3.68 / 6.40 / 15.3 | 2.11 / 4.48 / 7.7 | 2.10 / 4.48 / 7.7 | 2.21 / 4.26 / 6.3 | 0.87 / 4.06 / 5.0 | 0.50 / 1.10 / 1.5 |
| 10 1000 200 4 | 3.10 / 3.86 / 9.0 | 1.90 / 2.74 / 4.0 | 1.78 / 2.46 / 3.9 | 0.74 / 2.67 / 3.2 | 0.42 / 0.72 / 1.0 | 0.42 / 0.76 / 1.1 |
| 10 1000 200 8 | 4.03 / 6.88 / 13.8 | 2.43 / 5.12 / 8.1 | 2.30 / 4.99 / 7.4 | 2.53 / 5.09 / 6.7 | 1.15 / 4.13 / 5.3 | 0.58 / 1.36 / 1.8 |
| 10 1000 1000 4 | 4.99 / 7.39 / 14.7 | 3.52 / 5.70 / 6.9 | 2.78 / 4.86 / 6.6 | 2.13 / 3.87 / 4.6 | 1.20 / 2.42 / 3.6 | 1.20 / 2.37 / 3.5 |
| 10 1000 1000 8 | 6.59 / 11.07 / 17.7 | 4.54 / 9.34 / 13.1 | 3.82 / 8.90 / 12.6 | 3.52 / 7.62 / 11.1 | 2.75 / 5.57 / 7.9 | 1.41 / 4.00 / 5.3 |

a2f round trips/s; round-trip p50 / p99.9 (labels: ipi ns, W ns, k):

| labels | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|
| 0 50 4 | 640k; 1.74 / 2.1 | 642k; 1.74 / 2.4 | 625k; 1.82 / 2.6 | 600k; 1.90 / 2.4 | 598k; 1.95 / 3.4 |
| 0 50 8 | 1.23M; 1.82 / 3.2 | 1.23M; 1.93 / 3.0 | 1.15M; 2.16 / 3.0 | 1.02M; 2.54 / 7.1 | 931k; 2.91 / 8.8 |
| 0 200 4 | 572k; 2.11 / 2.7 | 572k; 2.11 / 3.0 | 561k; 2.21 / 3.0 | 536k; 2.30 / 4.6 | 537k; 2.30 / 3.6 |
| 0 200 8 | 1.10M; 2.35 / 4.8 | 1.10M; 2.29 / 4.0 | 1.06M; 2.34 / 5.4 | 859k; 3.26 / 9.2 | 723k; 4.14 / 12.4 |
| 0 1000 4 | 400k; 3.62 / 7.3 | 400k; 3.62 / 7.4 | 395k; 3.65 / 7.6 | 381k; 3.81 / 9.4 | 383k; 3.81 / 8.9 |
| 0 1000 8 | 785k; 3.78 / 9.4 | 788k; 3.65 / 9.9 | 759k; 3.98 / 10.2 | 653k; 4.58 / 12.4 | 338k; 10.50 / 23.2 |
| 1000 50 4 | 390k; 3.74 / 7.3 | 392k; 3.74 / 6.5 | 379k; 3.87 / 7.5 | 373k; 4.00 / 6.8 | 373k; 4.00 / 8.1 |
| 1000 50 8 | 758k; 3.98 / 10.1 | 762k; 3.89 / 9.3 | 718k; 4.22 / 10.7 | 682k; 4.54 / 10.6 | 685k; 4.48 / 11.9 |
| 1000 200 4 | 362k; 4.10 / 9.6 | 365k; 4.10 / 5.6 | 355k; 4.22 / 9.9 | 347k; 4.42 / 9.2 | 346k; 4.42 / 7.1 |
| 1000 200 8 | 712k; 4.22 / 9.7 | 709k; 4.26 / 10.7 | 687k; 4.35 / 10.6 | 642k; 4.90 / 12.0 | 619k; 5.09 / 13.8 |
| 1000 1000 4 | 285k; 5.63 / 10.8 | 286k; 5.57 / 10.5 | 280k; 5.70 / 12.5 | 275k; 5.89 / 9.5 | 274k; 5.89 / 14.1 |
| 1000 1000 8 | 560k; 5.70 / 13.4 | 563k; 5.70 / 12.7 | 538k; 6.21 / 13.2 | 515k; 6.40 / 14.7 | 333k; 10.62 / 21.8 |

a3f signals/s; signal call p50 / p99.9:

| labels | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 |
|---|---|---|---|---|---|
| 0 50 4 | 2.10M; 0.08 / 0.2 | 2.10M; 0.08 / 0.2 | 1.89M; 0.21 / 0.4 | 1.96M; 0.17 / 0.5 | 1.96M; 0.17 / 0.6 |
| 0 50 8 | 4.75M; 0.17 / 0.4 | 4.75M; 0.17 / 0.4 | 3.41M; 0.33 / 58.9 | 2.77M; 0.42 / 1.4 | 2.61M; 1.33 / 6.3 |
| 0 200 4 | 1.96M; 0.21 / 0.3 | 1.96M; 0.21 / 0.3 | 1.67M; 0.33 / 0.7 | 1.60M; 0.50 / 0.9 | 1.61M; 0.50 / 1.2 |
| 0 200 8 | 4.47M; 0.25 / 0.5 | 4.49M; 0.25 / 0.5 | 2.95M; 0.50 / 2.7 | 1.90M; 0.77 / 2.6 | 1.54M; 3.18 / 11.8 |
| 0 1000 4 | 1.57M; 0.58 / 0.7 | 1.57M; 0.58 / 0.7 | 1.08M; 0.66 / 1.8 | 672k; 3.22 / 10.0 | 674k; 3.20 / 8.0 |
| 0 1000 8 | 1.86M; 0.66 / 344.1 | 1.87M; 0.66 / 344.1 | 1.26M; 1.76 / 5.6 | 1.06M; 2.45 / 9.2 | 679k; 8.83 / 20.6 |
| 1000 50 4 | 2.10M; 0.08 / 0.2 | 957k; 0.08 / 847.9 | 1.45M; 0.21 / 0.4 | 1.98M; 0.17 / 0.5 | 1.97M; 0.17 / 0.7 |
| 1000 50 8 | 4.74M; 0.17 / 0.4 | 935k; 0.12 / 3014.7 | 3.30M; 0.33 / 10.8 | 2.68M; 0.42 / 1.2 | 2.80M; 1.12 / 5.5 |
| 1000 200 4 | 1.96M; 0.21 / 0.3 | 868k; 0.17 / 1032.2 | 1.38M; 0.29 / 0.7 | 1.65M; 0.42 / 0.9 | 1.65M; 0.42 / 0.8 |
| 1000 200 8 | 4.47M; 0.25 / 0.5 | 851k; 0.21 / 3604.5 | 2.38M; 0.56 / 12.9 | 2.00M; 0.68 / 2.0 | 1.68M; 2.78 / 10.0 |
| 1000 1000 4 | 1.57M; 0.58 / 0.7 | 655k; 0.54 / 1785.9 | 1.06M; 0.66 / 1.8 | 727k; 2.72 / 7.5 | 727k; 2.72 / 7.7 |
| 1000 1000 8 | 1.87M; 0.66 / 346.1 | 645k; 0.58 / 6094.8 | 1.15M; 2.29 / 6.5 | 1.14M; 2.26 / 7.4 | 695k; 8.70 / 18.9 |

bf unmaps/s; unmap p50 / p99.9 (labels: initiators, ipi ns, W ns, k):

| labels | 1 first | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 |
|---|---|---|---|---|---|---|---|---|
| one 0 50 4 | 448k; 0.87 / 5.1 | 462k; 0.79 / 4.9 | 457k; 0.79 / 4.5 | 378k; 1.28 / 6.8 | 389k; 1.20 / 6.7 | 467k; 0.74 / 4.8 | 467k; 0.74 / 4.3 | 468k; 0.74 / 5.1 |
| one 0 50 8 | 350k; 1.54 / 7.5 | 353k; 1.49 / 7.1 | 353k; 1.49 / 7.1 | 270k; 2.26 / 7.8 | 279k; 2.13 / 8.2 | 246k; 2.70 / 8.2 | 249k; 2.67 / 8.4 | 392k; 1.20 / 6.2 |
| one 0 200 4 | 402k; 1.12 / 5.5 | 428k; 0.95 / 4.2 | 419k; 1.00 / 5.5 | 345k; 1.54 / 6.2 | 360k; 1.41 / 5.2 | 441k; 0.87 / 5.7 | 442k; 0.87 / 4.4 | 441k; 0.87 / 4.5 |
| one 0 200 8 | 321k; 1.74 / 8.0 | 331k; 1.66 / 7.1 | 334k; 1.66 / 7.0 | 251k; 2.53 / 9.0 | 263k; 2.38 / 7.8 | 231k; 2.98 / 8.9 | 240k; 2.82 / 8.6 | 374k; 1.33 / 6.1 |
| one 0 1000 4 | 246k; 2.67 / 8.6 | 298k; 2.00 / 7.7 | 301k; 1.95 / 7.3 | 220k; 3.14 / 9.0 | 265k; 2.40 / 8.1 | 325k; 1.70 / 7.4 | 325k; 1.70 / 6.3 | 326k; 1.70 / 6.7 |
| one 0 1000 8 | 210k; 3.39 / 10.9 | 253k; 2.62 / 8.6 | 251k; 2.62 / 8.5 | 178k; 4.13 / 11.8 | 202k; 3.54 / 9.7 | 169k; 4.61 / 10.7 | 194k; 3.84 / 10.5 | 287k; 2.14 / 8.3 |
| one 1000 50 4 | 305k; 1.90 / 7.3 | 313k; 1.82 / 7.0 | 311k; 1.82 / 7.0 | 218k; 3.17 / 9.5 | 223k; 3.09 / 9.2 | 318k; 1.74 / 7.5 | 317k; 1.74 / 8.2 | 317k; 1.74 / 7.4 |
| one 1000 50 8 | 253k; 2.56 / 9.6 | 255k; 2.54 / 8.9 | 256k; 2.53 / 9.6 | 181k; 4.10 / 12.0 | 187k; 4.00 / 12.2 | 169k; 4.26 / 11.5 | 169k; 4.19 / 10.6 | 278k; 2.24 / 8.2 |
| one 1000 200 4 | 286k; 2.11 / 8.0 | 299k; 1.95 / 7.5 | 297k; 2.00 / 8.0 | 207k; 3.39 / 10.6 | 213k; 3.26 / 9.5 | 305k; 1.87 / 8.3 | 306k; 1.87 / 7.6 | 306k; 1.87 / 7.9 |
| one 1000 200 8 | 237k; 2.85 / 9.9 | 246k; 2.70 / 9.0 | 245k; 2.72 / 9.3 | 173k; 4.35 / 11.7 | 180k; 4.16 / 11.3 | 161k; 4.48 / 12.3 | 165k; 4.42 / 11.1 | 269k; 2.37 / 9.0 |
| one 1000 1000 4 | 198k; 3.65 / 11.4 | 231k; 2.94 / 10.2 | 232k; 2.91 / 9.8 | 156k; 4.99 / 13.2 | 176k; 4.29 / 11.4 | 246k; 2.66 / 8.6 | 246k; 2.66 / 9.0 | 245k; 2.66 / 8.9 |
| one 1000 1000 8 | 172k; 4.42 / 13.2 | 197k; 3.71 / 12.0 | 198k; 3.68 / 11.4 | 135k; 5.98 / 14.5 | 152k; 5.25 / 13.0 | 128k; 6.14 / 13.9 | 142k; 5.31 / 12.7 | 221k; 3.20 / 10.0 |
| all 0 50 4 | 1.15M; 1.46 / 6.2 | 1.17M; 1.15 / 5.9 | 1.17M; 1.07 / 5.4 | 844k; 2.53 / 7.1 | 876k; 2.30 / 6.7 | 1.09M; 2.27 / 6.3 | 1.10M; 2.24 / 5.6 | 1.10M; 2.27 / 5.3 |
| all 0 50 8 | 937k; 4.74 / 15.9 | 936k; 4.90 / 15.9 | 943k; 4.90 / 14.8 | 676k; 8.10 / 20.0 | 680k; 8.19 / 20.2 | 499k; 12.42 / 26.9 | 526k; 10.94 / 27.4 | 1.07M; 6.02 / 14.3 |
| all 0 200 4 | 990k; 2.14 / 7.7 | 1.04M; 1.46 / 7.3 | 1.03M; 1.55 / 6.6 | 750k; 3.20 / 8.2 | 804k; 2.96 / 9.5 | 957k; 2.78 / 7.7 | 960k; 2.78 / 7.8 | 960k; 2.78 / 7.6 |
| all 0 200 8 | 835k; 5.28 / 17.8 | 850k; 5.63 / 18.3 | 845k; 5.60 / 16.9 | 601k; 9.60 / 22.0 | 643k; 8.83 / 21.0 | 470k; 13.25 / 30.0 | 499k; 11.71 / 28.2 | 954k; 7.14 / 15.7 |
| all 0 1000 4 | 556k; 4.48 / 15.4 | 658k; 2.78 / 8.6 | 654k; 2.82 / 8.6 | 444k; 6.66 / 15.7 | 486k; 6.78 / 11.7 | 543k; 5.95 / 12.9 | 543k; 5.95 / 12.0 | 544k; 5.95 / 11.2 |
| all 0 1000 8 | 509k; 8.32 / 26.9 | 528k; 11.58 / 26.5 | 530k; 11.46 / 26.9 | 402k; 16.00 / 33.5 | 432k; 15.42 / 30.7 | 332k; 21.50 / 40.4 | 360k; 19.97 / 37.6 | 521k; 14.34 / 24.1 |
| all 1000 50 4 | 709k; 2.34 / 6.8 | 730k; 2.24 / 7.9 | 575k; 2.78 / 9.6 | 570k; 3.23 / 8.7 | 570k; 3.20 / 10.0 | 1.09M; 2.27 / 7.5 | 1.09M; 2.27 / 5.8 | 1.09M; 2.27 / 6.9 |
| all 1000 50 8 | 743k; 5.31 / 16.5 | 754k; 5.41 / 14.8 | 551k; 7.26 / 23.6 | 590k; 7.42 / 20.0 | 593k; 7.26 / 19.3 | 485k; 9.22 / 26.4 | 497k; 8.70 / 25.9 | 926k; 4.35 / 13.4 |
| all 1000 200 4 | 647k; 2.96 / 10.2 | 662k; 2.51 / 9.5 | 532k; 3.26 / 9.7 | 544k; 3.62 / 12.1 | 558k; 3.46 / 8.8 | 954k; 2.78 / 7.6 | 954k; 2.78 / 8.8 | 956k; 2.78 / 6.9 |
| all 1000 200 8 | 678k; 5.98 / 18.7 | 697k; 6.02 / 16.2 | 524k; 8.19 / 25.5 | 550k; 8.32 / 21.8 | 557k; 8.32 / 20.4 | 447k; 10.62 / 27.5 | 468k; 9.73 / 25.7 | 854k; 5.89 / 15.6 |
| all 1000 1000 4 | 408k; 5.06 / 18.7 | 504k; 4.61 / 11.5 | 538k; 4.16 / 12.7 | 390k; 7.10 / 19.2 | 469k; 6.72 / 12.0 | 541k; 5.95 / 15.2 | 541k; 5.95 / 13.1 | 542k; 5.95 / 11.7 |
| all 1000 1000 8 | 447k; 8.83 / 27.6 | 477k; 10.11 / 28.9 | 414k; 13.38 / 35.8 | 384k; 15.04 / 32.8 | 417k; 14.91 / 29.6 | 327k; 17.41 / 37.4 | 363k; 17.15 / 34.6 | 498k; 14.66 / 24.8 |

cf independent processes, lookup p50 / p99 / p99.9 (labels: n, W ns, k):

| labels | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair |
|---|---|---|---|---|---|---|---|---|
| 4 200 1 | 0.21 / 0.21 / 0.2 | 0.21 / 0.21 / 0.3 | 0.29 / 0.46 / 0.6 | 0.21 / 0.21 / 0.3 | 0.29 / 0.48 / 0.6 | 0.21 / 0.21 / 0.2 | 0.21 / 0.21 / 0.3 | 0.21 / 0.23 / 0.3 |
| 4 200 2 | 0.21 / 0.21 / 0.3 | 0.21 / 0.23 / 0.2 | 0.29 / 0.46 / 0.5 | 0.21 / 0.21 / 0.2 | 0.29 / 0.50 / 0.7 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.2 | 0.21 / 0.21 / 0.2 |
| 4 200 3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.2 | 0.29 / 0.58 / 0.7 | 0.21 / 0.25 / 0.2 | 0.31 / 0.54 / 0.7 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 |
| 4 1000 1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.07 / 1.25 / 1.5 | 1.00 / 1.00 / 1.1 | 2.94 / 3.10 / 8.7 | 1.00 / 1.02 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.02 / 1.1 |
| 4 1000 2 | 1.00 / 1.00 / 1.1 | 1.00 / 1.02 / 1.1 | 1.07 / 1.25 / 1.3 | 1.00 / 1.00 / 1.1 | 2.98 / 3.20 / 7.8 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.0 | 1.00 / 1.02 / 1.0 |
| 4 1000 3 | 1.00 / 1.02 / 1.1 | 1.00 / 1.00 / 1.1 | 1.07 / 2.11 / 2.3 | 1.00 / 1.04 / 1.1 | 2.98 / 3.23 / 9.6 | 1.00 / 1.02 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 |
| 8 200 1 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.33 / 0.62 / 0.7 | 0.21 / 0.29 / 0.3 | 0.42 / 0.66 / 0.8 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 2 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.33 / 0.62 / 0.7 | 0.21 / 0.29 / 0.3 | 0.42 / 0.70 / 0.9 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.33 / 0.62 / 0.9 | 0.21 / 0.29 / 0.3 | 0.42 / 0.66 / 0.9 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 5 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.33 / 0.66 / 1.0 | 0.21 / 0.29 / 0.3 | 0.44 / 1.26 / 1.9 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 6 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.33 / 0.62 / 0.7 | 0.21 / 0.29 / 0.3 | 0.42 / 1.25 / 1.9 | 0.21 / 0.33 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 1000 1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.15 / 1.51 / 1.7 | 1.00 / 1.07 / 1.1 | 3.22 / 3.49 / 9.7 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 2 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.15 / 1.49 / 1.6 | 1.00 / 1.07 / 1.1 | 3.23 / 3.66 / 10.4 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 3 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.15 / 1.90 / 2.4 | 1.00 / 1.07 / 1.1 | 3.23 / 3.66 / 10.2 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 5 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.14 / 2.14 / 2.5 | 1.00 / 1.07 / 1.1 | 3.23 / 4.42 / 9.7 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 6 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.12 / 1.46 / 1.6 | 1.00 / 1.07 / 1.1 | 3.17 / 4.42 / 8.9 | 1.00 / 1.12 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |

cf all processes together (the k-thread one and the independent ones), operations/s:

| labels | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair |
|---|---|---|---|---|---|---|---|---|
| 4 200 1 | 2.56M | 2.55M | 2.44M | 2.55M | 2.39M | 2.55M | 2.55M | 2.55M |
| 4 200 2 | 2.34M | 2.38M | 2.43M | 2.52M | 2.41M | 2.53M | 2.49M | 2.53M |
| 4 200 3 | 2.09M | 2.15M | 2.01M | 2.15M | 2.40M | 2.52M | 2.45M | 2.52M |
| 4 1000 1 | 1.69M | 1.69M | 1.63M | 1.69M | 928k | 1.70M | 1.70M | 1.70M |
| 4 1000 2 | 1.55M | 1.59M | 1.64M | 1.68M | 922k | 1.69M | 1.67M | 1.68M |
| 4 1000 3 | 1.39M | 1.46M | 1.34M | 1.47M | 917k | 1.64M | 1.34M | 1.64M |
| 8 200 1 | 5.14M | 5.13M | 4.75M | 5.13M | 4.58M | 5.13M | 5.12M | 5.13M |
| 8 200 2 | 4.89M | 4.93M | 4.74M | 5.10M | 4.56M | 5.10M | 5.00M | 5.11M |
| 8 200 3 | 4.62M | 4.71M | 4.28M | 4.66M | 4.55M | 5.06M | 4.90M | 5.07M |
| 8 200 5 | 4.02M | 4.12M | 3.68M | 4.03M | 3.40M | 4.02M | 4.54M | 4.96M |
| 8 200 6 | 3.65M | 3.80M | 3.55M | 3.87M | 3.28M | 3.85M | 3.85M | 4.89M |
| 8 1000 1 | 3.45M | 3.45M | 3.20M | 3.45M | 1.76M | 3.46M | 3.46M | 3.45M |
| 8 1000 2 | 3.30M | 3.34M | 3.22M | 3.42M | 1.74M | 3.44M | 3.39M | 3.42M |
| 8 1000 3 | 3.11M | 3.19M | 2.89M | 3.19M | 1.73M | 3.36M | 3.01M | 3.36M |
| 8 1000 5 | 2.68M | 2.82M | 2.51M | 2.81M | 1.53M | 2.72M | 2.12M | 3.13M |
| 8 1000 6 | 2.44M | 2.62M | 2.39M | 2.69M | 1.49M | 2.63M | 1.69M | 2.98M |


## Third series: finer locks in a group

15 November 2026, 04:52 to 05:24. The second series left every write of a
group under one lock, and named it as the main limit. The third series adds
the locks a real group kernel would plausibly have, as a switch, and shortens
the lock-free lookup to one slot read (README, "Switches (third
measurement)", which also lists what a kernel must guarantee for each lock
to be legal):

- IPC: a lock per endpoint (the server's), and the endpoint's own lines.
- Signals: a lock per notification object; the check half of a signal is a
  lock-free slot read plus `W/2` with no lock.
- Capability changes inside one group: the locks of the two slot ranges they
  write (8 ranges per space), in order; no group lock.
- Lookups: a seqlock read of one slot; the rest of the call's `W` runs after
  the read.
- Unchanged: unmaps, and changes of a process spread over several groups,
  keep the group lock; `b` is therefore not rerun (second series stands).

Candidate 1 keeps its first and fair settings of the second series, and both
interrupt costs are measured. "fine" = the second series' fair settings plus
fine locks and the slot read; "3 slot read" = the second series' fair
settings with only the slot read. Two full passes of 5 repetitions, pooled
(10 runs per cell). Per cell, median throughput of pass 2 over pass 1: median
1.00 in every suite, p90 at most 1.03; within-pass spread median 1 to 2 %.
Load average 2 to 7. Rows with k = 8 ran about half on efficiency cores.

Seqlock reads repeated per 1000 lookups: 0.2 with the slot read (groups of 2,
4 and candidate 3 alike), against 18 to 57 with the whole-call read of the
second series. The second series' lookup tail was mostly an artefact of the
long read section.

**The bench caught no new bug in this series.** About 2100 short stress runs
and the 2 × 1052 measured runs passed the self-checks, which now also cover
the lines of endpoints and notification objects (a lost update under a fine
lock would fail the run).

### Per workload, k = 4 (k = 8 in parentheses), against the second series

| Workload | Second series (fair) | Third series (fine) | Winner changes? |
|---|---|---|---|
| a1, 1 % changes, `ipi` 1 µs | Level, within 4 % | 3 ahead by 2 to 4 % (6 to 8 %) | No: still level at k = 4 |
| a1, 10 % changes, `ipi` 0 | 3 by 3 to 8 % | 3 by 7 to 11 % (22 to 26 %) | No; margin grows |
| a1, 10 % changes, `ipi` 1 µs | 3 by 17 to 33 % | 3 by 27 to 32 % (1.54 to 1.65 times) | No; margin grows at W = 1 µs |
| Lookup latency, W = 1 µs, 10 % | Same p50; group p99 3.5 × W, p99.9 5 × W | Same p50; group p99 1.7 × W, p99.9 2.1 × W | No: candidate 1 keeps the tail (1.04 × W) |
| Change latency, `ipi` 1 µs | 3, 2.3 to 11 × lower | 3, 2.3 to 9.6 × lower | No |
| a2 IPC | 1 by 4 to 7 % (1.7 to 2.3 × at W = 1 µs) | 1 by 1 to 2 % (2 to 4 %) | Nearly: from a lead to the edge of the noise |
| a3 many-to-one signal | 1 by 7 % to 2.2 × | 1 by 8 % to 1.7 × (1.6 to 2.2 ×) | No |
| b unmap | 3 by 4 to 10 % (one thread) | Not rerun: unmaps keep the group lock | – |
| c independent processes | Level (lookups only) | Level | No |

Reading:

- **In a1 the slot read does the work, not the fine locks.** "3 slot read"
  and "3 fine" are within 1 % in every row: one process has one capability
  space, so range locks only split its own changes, which are few. The gain
  over the second series is at W = 1 µs (1.54 to 1.66 M operations/s at k = 4,
  2.58 to 3.30 M at k = 8), where the long read section had made readers
  wait for writers.
- **IPC: the endpoint lock removes the group lock's penalty.** Candidate 3
  with fine locks matches groups of 2 and is within 1 to 2 % of candidate 1
  at k = 4. At k = 8 and W = 1 µs it goes from 0.34 M to 0.76 M round trips/s
  (candidate 1: 0.79 M). The remaining gap is the lock handover of an
  uncontended lock, and is at the edge of what two passes resolve.
- **Signals: candidate 1 keeps the row**, by 8 to 18 % at W ≤ 200 ns and
  1.5 to 1.7 times at W = 1 µs. A signal in a group still applies its bits
  synchronously under the notify's lock, which all producers share; in
  candidate 1 the producer returns once the message is in the ring and the
  notify's node applies the signals one after another. The notify lock helps
  the group at W = 1 µs (0.67 to 0.72 M becomes 0.91 to 1.06 M signals/s at
  k = 4) but does not
  close the gap. A notify updated with one atomic OR, with a lock only to wake
  a waiter, would be the next step; not measured. With one interrupt per
  message, candidate 1 falls behind every group here (0.65 to 0.96 M).
- **Fixed groups**: with fine locks, groups of 4 behave like candidate 3 when
  the process fits (k ≤ 4), as before. Groups of 2 gain little in a1 and
  lose 10 to 33 % in signals at k = 8: the notify's group, the bottleneck
  there, now takes two locks (its group lock for the ring, then the
  notify's) for each signal that comes from another group.

### What the third series still cannot decide on this Mac

- The real interrupt cost, the real change rate and the real W: they still
  decide the a1 rows with 10 % changes (7 to 11 % at `ipi` 0, 27 to 32 % at
  1 µs).
- Whether a kernel can meet the guarantees of the fine locks (README table):
  above all, a delete or revoke of a subtree needs a lock of the whole space,
  or hand-over-hand locking, which is not measured; only leaf deletes are.
- The cost of an uncontended lock handover on x86-64, which is now the whole
  IPC gap (1 to 2 %).
- A lock-free notify (atomic OR), the change that could move the signal row.
- Unmaps with finer page-table locks (per range of the address space).
- Everything about the host listed for the first series: no pinning,
  efficiency cores at k ≥ 6, 41.7 ns clock, host preemption in the p99.9.

### Tables (pooled, 10 runs per cell)

Cells: throughput; or latency p50 / p99 / p99.9 in µs; or throughput; p50 /
p99.9. Full tables of both passes: appendices E and F. Raw runs:
`data/pass-t1.csv.gz`, `data/pass-t2.csv.gz`.

a1t throughput, 1 % changes, `ipi` 1 µs (labels: change %, ipi ns, W ns, k):

| labels | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|---|---|---|
| 1 1000 50 4 | 2.65M | 2.67M | 2.70M | 2.70M | 2.76M | 2.78M | 2.76M | 2.78M | 2.78M |
| 1 1000 50 8 | 5.12M | 5.14M | 5.14M | 5.14M | 5.25M | 5.27M | 5.53M | 5.56M | 5.57M |
| 1 1000 200 4 | 2.45M | 2.46M | 2.49M | 2.49M | 2.55M | 2.56M | 2.55M | 2.55M | 2.55M |
| 1 1000 200 8 | 4.65M | 4.75M | 4.74M | 4.76M | 4.85M | 4.85M | 5.08M | 5.10M | 5.11M |
| 1 1000 1000 4 | 1.62M | 1.64M | 1.66M | 1.66M | 1.67M | 1.69M | 1.67M | 1.68M | 1.68M |
| 1 1000 1000 8 | 3.15M | 3.23M | 3.25M | 3.26M | 3.28M | 3.32M | 3.33M | 3.44M | 3.44M |

a1t throughput, 10 % changes, `ipi` 0:

| labels | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|---|---|---|
| 10 0 50 4 | 2.49M | 2.55M | 2.56M | 2.58M | 2.72M | 2.73M | 2.71M | 2.74M | 2.73M |
| 10 0 50 8 | 4.16M | 4.41M | 4.35M | 4.38M | 4.67M | 4.72M | 5.31M | 5.41M | 5.40M |
| 10 0 200 4 | 2.24M | 2.32M | 2.35M | 2.37M | 2.50M | 2.51M | 2.49M | 2.51M | 2.52M |
| 10 0 200 8 | 3.71M | 4.00M | 3.97M | 3.99M | 4.26M | 4.34M | 4.67M | 4.97M | 4.93M |
| 10 0 1000 4 | 1.37M | 1.50M | 1.49M | 1.56M | 1.54M | 1.66M | 1.54M | 1.66M | 1.66M |
| 10 0 1000 8 | 2.23M | 2.62M | 2.51M | 2.72M | 2.57M | 2.93M | 2.58M | 3.29M | 3.30M |

a1t throughput, 10 % changes, `ipi` 1 µs:

| labels | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|---|---|---|
| 10 1000 50 4 | 2.00M | 2.08M | 2.19M | 2.20M | 2.72M | 2.74M | 2.72M | 2.73M | 2.74M |
| 10 1000 50 8 | 3.13M | 3.30M | 3.37M | 3.43M | 3.81M | 3.85M | 5.33M | 5.41M | 5.43M |
| 10 1000 200 4 | 1.83M | 1.92M | 2.03M | 2.04M | 2.50M | 2.51M | 2.49M | 2.52M | 2.51M |
| 10 1000 200 8 | 2.89M | 3.09M | 3.18M | 3.23M | 3.56M | 3.61M | 4.67M | 4.97M | 4.95M |
| 10 1000 1000 4 | 1.20M | 1.31M | 1.39M | 1.42M | 1.54M | 1.66M | 1.54M | 1.66M | 1.66M |
| 10 1000 1000 8 | 1.85M | 2.14M | 2.22M | 2.30M | 2.37M | 2.59M | 2.58M | 3.28M | 3.30M |

a1t lookup latency p50 / p99 / p99.9, µs, 10 % changes, `ipi` 1 µs:

| labels | 1 fair | 2/2 fine | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|
| 10 1000 50 4 | 0.08 / 0.12 / 0.1 | 0.08 / 0.12 / 0.2 | 0.08 / 0.12 / 0.2 | 0.08 / 0.29 / 0.4 | 0.08 / 0.12 / 0.2 | 0.08 / 0.12 / 0.2 |
| 10 1000 50 8 | 0.08 / 0.17 / 0.2 | 0.12 / 0.21 / 0.3 | 0.12 / 0.21 / 0.4 | 0.12 / 0.50 / 0.8 | 0.12 / 0.27 / 0.5 | 0.12 / 0.25 / 0.5 |
| 10 1000 200 4 | 0.21 / 0.25 / 0.2 | 0.21 / 0.25 / 0.5 | 0.21 / 0.33 / 0.5 | 0.21 / 0.66 / 1.0 | 0.21 / 0.29 / 0.5 | 0.21 / 0.31 / 0.5 |
| 10 1000 200 8 | 0.21 / 0.29 / 0.3 | 0.25 / 0.35 / 0.5 | 0.25 / 0.37 / 0.6 | 0.25 / 1.46 / 2.2 | 0.21 / 0.50 / 0.7 | 0.21 / 0.54 / 0.7 |
| 10 1000 1000 4 | 1.00 / 1.04 / 1.1 | 1.00 / 1.14 / 2.0 | 1.00 / 1.70 / 2.1 | 1.00 / 3.52 / 5.0 | 1.00 / 1.74 / 2.1 | 1.00 / 1.74 / 2.1 |
| 10 1000 1000 8 | 1.00 / 1.07 / 1.1 | 1.04 / 1.20 / 2.1 | 1.04 / 1.74 / 2.2 | 1.07 / 5.31 / 7.7 | 1.04 / 2.03 / 2.2 | 1.04 / 2.08 / 2.4 |

a1t delete latency, 10 % changes, `ipi` 1 µs:

| labels | 1 fair | 2/2 fine | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|
| 10 1000 50 4 | 2.78 / 3.33 / 10.8 | 1.78 / 2.54 / 3.6 | 0.29 / 0.46 / 0.6 | 0.25 / 0.52 / 0.7 | 0.25 / 0.50 / 0.7 | 0.29 / 0.46 / 0.6 |
| 10 1000 50 8 | 3.68 / 6.78 / 14.0 | 3.68 / 6.02 / 8.9 | 2.30 / 4.42 / 5.5 | 0.46 / 1.00 / 1.3 | 0.48 / 1.14 / 1.5 | 0.46 / 0.98 / 1.3 |
| 10 1000 200 4 | 2.98 / 3.71 / 10.4 | 2.00 / 2.78 / 4.1 | 0.42 / 0.74 / 0.9 | 0.42 / 0.74 / 1.0 | 0.39 / 0.74 / 1.0 | 0.42 / 0.74 / 0.9 |
| 10 1000 200 8 | 3.94 / 7.42 / 16.5 | 3.81 / 6.40 / 10.2 | 2.53 / 4.67 / 6.1 | 0.58 / 1.41 / 1.9 | 0.56 / 1.25 / 1.8 | 0.62 / 1.26 / 1.7 |
| 10 1000 1000 4 | 4.38 / 6.62 / 22.7 | 2.98 / 4.61 / 7.8 | 1.20 / 2.27 / 3.2 | 1.20 / 2.40 / 3.5 | 1.20 / 2.37 / 3.5 | 1.20 / 2.27 / 3.0 |
| 10 1000 1000 8 | 5.95 / 11.52 / 19.3 | 5.28 / 10.11 / 14.5 | 3.66 / 7.71 / 11.1 | 1.43 / 4.06 / 5.4 | 1.43 / 4.10 / 6.4 | 1.49 / 3.17 / 4.6 |

a1t derive latency, 10 % changes, `ipi` 1 µs:

| labels | 1 fair | 2/2 fine | 2/4 fine | 3 fair | 3 slot read | 3 fine |
|---|---|---|---|---|---|---|
| 10 1000 50 4 | 1.54 / 2.11 / 3.6 | 0.42 / 2.43 / 2.9 | 0.29 / 0.46 / 0.6 | 0.25 / 0.52 / 0.8 | 0.25 / 0.50 / 0.6 | 0.29 / 0.46 / 0.6 |
| 10 1000 50 8 | 2.11 / 4.80 / 8.1 | 2.22 / 4.48 / 6.4 | 0.87 / 4.05 / 5.1 | 0.46 / 1.00 / 1.3 | 0.48 / 1.11 / 1.5 | 0.46 / 0.95 / 1.3 |
| 10 1000 200 4 | 1.74 / 2.43 / 3.2 | 0.64 / 2.64 / 3.2 | 0.42 / 0.74 / 1.0 | 0.42 / 0.74 / 1.0 | 0.37 / 0.74 / 1.0 | 0.42 / 0.74 / 0.9 |
| 10 1000 200 8 | 2.30 / 5.22 / 9.9 | 2.37 / 4.74 / 7.2 | 1.15 / 4.16 / 5.3 | 0.58 / 1.41 / 2.0 | 0.54 / 1.25 / 1.8 | 0.62 / 1.26 / 1.7 |
| 10 1000 1000 4 | 2.78 / 4.90 / 8.5 | 1.98 / 4.03 / 6.5 | 1.20 / 2.27 / 3.0 | 1.20 / 2.40 / 3.5 | 1.20 / 2.40 / 3.9 | 1.20 / 2.27 / 3.9 |
| 10 1000 1000 8 | 3.81 / 8.70 / 13.2 | 3.52 / 7.87 / 11.5 | 2.66 / 5.73 / 8.3 | 1.38 / 4.02 / 5.5 | 1.43 / 4.10 / 6.4 | 1.49 / 3.22 / 4.7 |

a2t round trips/s; round-trip p50 / p99.9 (labels: ipi ns, W ns, k):

| labels | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine |
|---|---|---|---|---|---|---|---|---|
| 0 50 4 | 638k; 1.74 / 4.0 | 640k; 1.74 / 2.9 | 624k; 1.82 / 2.9 | 623k; 1.82 / 3.4 | 597k; 1.93 / 4.4 | 624k; 1.82 / 2.8 | 598k; 1.93 / 5.6 | 624k; 1.82 / 3.6 |
| 0 50 8 | 1.22M; 1.85 / 4.2 | 1.23M; 1.82 / 4.1 | 1.17M; 2.11 / 4.6 | 1.16M; 1.89 / 4.9 | 991k; 2.70 / 9.2 | 1.18M; 1.89 / 5.3 | 860k; 3.23 / 11.5 | 1.17M; 2.01 / 4.7 |
| 0 200 4 | 570k; 2.11 / 5.9 | 570k; 2.11 / 4.3 | 558k; 2.21 / 3.6 | 559k; 2.21 / 2.9 | 535k; 2.30 / 6.5 | 559k; 2.21 / 4.6 | 533k; 2.30 / 4.4 | 558k; 2.21 / 6.4 |
| 0 200 8 | 1.09M; 2.27 / 6.8 | 1.10M; 2.29 / 5.4 | 1.05M; 2.48 / 7.5 | 1.06M; 2.46 / 5.6 | 866k; 3.30 / 9.8 | 1.06M; 2.34 / 7.4 | 693k; 4.11 / 15.2 | 1.05M; 2.50 / 4.7 |
| 0 1000 4 | 398k; 3.62 / 11.5 | 398k; 3.62 / 10.8 | 393k; 3.68 / 10.0 | 394k; 3.68 / 11.5 | 379k; 3.81 / 13.4 | 393k; 3.66 / 12.2 | 378k; 3.81 / 12.9 | 394k; 3.68 / 8.7 |
| 0 1000 8 | 787k; 3.66 / 10.6 | 785k; 3.74 / 10.5 | 759k; 3.95 / 10.0 | 756k; 3.98 / 10.6 | 660k; 4.48 / 12.7 | 760k; 3.92 / 11.0 | 335k; 10.24 / 25.7 | 755k; 3.92 / 11.0 |
| 1000 50 4 | 389k; 3.74 / 9.9 | 391k; 3.74 / 8.0 | 379k; 3.90 / 8.2 | 380k; 3.87 / 9.7 | 371k; 4.00 / 10.3 | 379k; 3.89 / 9.9 | 371k; 4.00 / 9.9 | 380k; 3.87 / 9.0 |
| 1000 50 8 | 760k; 3.89 / 10.6 | 762k; 3.84 / 10.5 | 729k; 4.13 / 11.0 | 736k; 4.13 / 10.6 | 696k; 4.35 / 11.1 | 725k; 4.19 / 9.9 | 668k; 4.64 / 13.0 | 729k; 3.94 / 10.0 |
| 1000 200 4 | 362k; 4.16 / 8.1 | 364k; 4.10 / 8.3 | 354k; 4.29 / 9.3 | 354k; 4.29 / 9.0 | 345k; 4.42 / 10.3 | 354k; 4.22 / 10.9 | 345k; 4.42 / 11.3 | 354k; 4.26 / 10.2 |
| 1000 200 8 | 707k; 4.26 / 11.5 | 710k; 4.29 / 12.2 | 692k; 4.45 / 12.4 | 685k; 4.58 / 11.8 | 650k; 4.80 / 13.2 | 694k; 4.42 / 12.0 | 600k; 5.25 / 14.3 | 693k; 4.42 / 11.8 |
| 1000 1000 4 | 284k; 5.63 / 12.5 | 285k; 5.57 / 11.5 | 279k; 5.76 / 14.8 | 280k; 5.70 / 12.6 | 273k; 5.89 / 12.5 | 280k; 5.70 / 11.8 | 274k; 5.89 / 12.7 | 280k; 5.70 / 12.4 |
| 1000 1000 8 | 561k; 5.70 / 14.0 | 561k; 5.76 / 13.4 | 539k; 6.08 / 14.0 | 541k; 6.05 / 14.2 | 513k; 6.37 / 15.2 | 540k; 6.08 / 13.8 | 338k; 10.37 / 23.0 | 540k; 6.05 / 13.3 |

a3t signals/s; signal call p50 / p99.9:

| labels | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine |
|---|---|---|---|---|---|---|---|---|
| 0 50 4 | 2.09M; 0.08 / 0.2 | 2.09M; 0.08 / 0.2 | 1.87M; 0.21 / 0.5 | 1.86M; 0.21 / 0.5 | 1.91M; 0.21 / 0.6 | 1.91M; 0.21 / 0.6 | 1.93M; 0.19 / 0.6 | 1.91M; 0.21 / 0.6 |
| 0 50 8 | 4.72M; 0.17 / 0.4 | 4.72M; 0.17 / 0.4 | 3.58M; 0.33 / 53.2 | 2.39M; 0.44 / 1.6 | 2.72M; 0.42 / 1.5 | 2.50M; 0.42 / 0.8 | 2.57M; 1.36 / 7.6 | 2.70M; 1.28 / 6.3 |
| 0 200 4 | 1.95M; 0.21 / 0.3 | 1.94M; 0.21 / 0.4 | 1.66M; 0.33 / 0.7 | 1.63M; 0.33 / 0.6 | 1.60M; 0.50 / 1.3 | 1.66M; 0.46 / 0.7 | 1.60M; 0.50 / 0.7 | 1.66M; 0.46 / 1.1 |
| 0 200 8 | 4.44M; 0.25 / 0.5 | 4.45M; 0.25 / 0.5 | 2.76M; 0.48 / 7.9 | 2.25M; 0.64 / 9.0 | 2.11M; 0.58 / 2.1 | 2.00M; 0.68 / 2.6 | 1.58M; 3.04 / 11.3 | 1.98M; 2.18 / 10.5 |
| 0 1000 4 | 1.56M; 0.58 / 0.7 | 1.56M; 0.58 / 0.7 | 1.07M; 0.68 / 1.7 | 1.12M; 0.70 / 1.7 | 671k; 3.22 / 8.1 | 901k; 1.79 / 7.4 | 672k; 3.22 / 8.6 | 908k; 1.79 / 2.6 |
| 0 1000 8 | 1.87M; 0.66 / 340.0 | 1.87M; 0.62 / 340.0 | 1.10M; 2.53 / 8.0 | 1.16M; 2.09 / 9.7 | 1.09M; 2.42 / 9.2 | 1.05M; 2.50 / 11.3 | 681k; 8.83 / 24.1 | 1.05M; 5.28 / 15.4 |
| 1000 50 4 | 2.10M; 0.08 / 0.2 | 955k; 0.08 / 827.4 | 1.45M; 0.21 / 0.4 | 1.44M; 0.21 / 0.4 | 1.96M; 0.17 / 0.5 | 1.94M; 0.21 / 0.5 | 1.95M; 0.17 / 0.7 | 1.94M; 0.21 / 0.4 |
| 1000 50 8 | 4.70M; 0.19 / 0.4 | 932k; 0.12 / 3014.7 | 3.21M; 0.29 / 57.9 | 2.96M; 0.42 / 1.4 | 2.69M; 0.42 / 1.2 | 2.50M; 0.42 / 1.0 | 2.78M; 1.14 / 6.5 | 2.98M; 0.99 / 6.0 |
| 1000 200 4 | 1.94M; 0.21 / 0.3 | 868k; 0.17 / 1007.6 | 1.36M; 0.29 / 0.7 | 1.35M; 0.33 / 0.6 | 1.63M; 0.44 / 1.0 | 1.66M; 0.42 / 0.9 | 1.63M; 0.44 / 0.8 | 1.67M; 0.42 / 0.7 |
| 1000 200 8 | 4.42M; 0.25 / 0.5 | 847k; 0.21 / 3588.1 | 2.73M; 0.50 / 13.5 | 2.22M; 0.64 / 26.1 | 2.05M; 0.66 / 3.2 | 1.98M; 0.68 / 4.3 | 1.66M; 2.82 / 11.3 | 2.16M; 1.89 / 9.3 |
| 1000 1000 4 | 1.56M; 0.58 / 0.7 | 654k; 0.54 / 1753.1 | 1.06M; 0.66 / 1.8 | 1.03M; 0.70 / 1.7 | 723k; 2.78 / 8.7 | 1.06M; 1.60 / 4.4 | 723k; 2.72 / 8.8 | 1.06M; 1.56 / 2.3 |
| 1000 1000 8 | 1.87M; 0.66 / 344.1 | 643k; 0.58 / 6094.8 | 1.23M; 1.78 / 7.8 | 1.13M; 1.95 / 8.5 | 1.14M; 2.24 / 8.4 | 1.09M; 2.37 / 9.6 | 703k; 8.45 / 21.1 | 1.13M; 4.86 / 14.1 |

ct independent processes, lookup p50 / p99 / p99.9 (labels: n, W ns, k):

| labels | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine |
|---|---|---|---|---|---|---|---|---|
| 4 200 1 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.2 | 0.21 / 0.21 / 0.3 | 0.21 / 0.23 / 0.2 | 0.21 / 0.21 / 0.2 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.25 / 0.3 |
| 4 200 2 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.23 / 0.3 | 0.21 / 0.21 / 0.2 | 0.21 / 0.25 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.23 / 0.3 |
| 4 200 3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.25 / 0.3 | 0.21 / 0.25 / 0.3 | 0.21 / 0.21 / 0.2 | 0.21 / 0.23 / 0.3 | 0.21 / 0.21 / 0.3 | 0.21 / 0.21 / 0.3 |
| 4 1000 1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.04 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.04 / 1.1 |
| 4 1000 2 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.02 / 1.1 | 1.00 / 1.04 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.04 / 1.1 |
| 4 1000 3 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.04 / 1.1 | 1.00 / 1.04 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.04 / 1.1 | 1.00 / 1.00 / 1.1 | 1.00 / 1.00 / 1.1 |
| 8 200 1 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 2 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 5 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 200 6 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 | 0.21 / 0.29 / 0.3 |
| 8 1000 1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 2 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 3 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 5 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.02 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |
| 8 1000 6 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 | 1.00 / 1.07 / 1.1 |

ct all processes together, operations/s:

| labels | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine |
|---|---|---|---|---|---|---|---|---|
| 4 200 1 | 2.53M | 2.53M | 2.53M | 2.53M | 2.53M | 2.52M | 2.53M | 2.53M |
| 4 200 2 | 2.32M | 2.36M | 2.51M | 2.51M | 2.52M | 2.51M | 2.51M | 2.51M |
| 4 200 3 | 2.08M | 2.14M | 2.13M | 2.14M | 2.49M | 2.50M | 2.49M | 2.50M |
| 4 1000 1 | 1.69M | 1.68M | 1.69M | 1.69M | 1.68M | 1.69M | 1.69M | 1.68M |
| 4 1000 2 | 1.55M | 1.58M | 1.67M | 1.68M | 1.67M | 1.68M | 1.67M | 1.68M |
| 4 1000 3 | 1.38M | 1.46M | 1.46M | 1.49M | 1.63M | 1.67M | 1.63M | 1.67M |
| 8 200 1 | 5.10M | 5.11M | 5.11M | 5.12M | 5.11M | 5.11M | 5.11M | 5.11M |
| 8 200 2 | 4.87M | 4.92M | 5.08M | 5.08M | 5.08M | 5.09M | 5.08M | 5.08M |
| 8 200 3 | 4.61M | 4.67M | 4.65M | 4.66M | 5.05M | 5.05M | 5.04M | 5.06M |
| 8 200 5 | 4.00M | 4.11M | 4.04M | 4.03M | 4.00M | 4.04M | 4.95M | 5.01M |
| 8 200 6 | 3.66M | 3.78M | 3.84M | 3.89M | 3.85M | 3.87M | 4.88M | 4.99M |
| 8 1000 1 | 3.44M | 3.45M | 3.45M | 3.43M | 3.45M | 3.44M | 3.45M | 3.44M |
| 8 1000 2 | 3.28M | 3.33M | 3.41M | 3.42M | 3.41M | 3.42M | 3.41M | 3.42M |
| 8 1000 3 | 3.10M | 3.19M | 3.18M | 3.19M | 3.35M | 3.41M | 3.35M | 3.41M |
| 8 1000 5 | 2.67M | 2.83M | 2.80M | 2.83M | 2.73M | 2.87M | 3.11M | 3.37M |
| 8 1000 6 | 2.41M | 2.62M | 2.70M | 2.73M | 2.62M | 2.76M | 2.96M | 3.35M |

## Fourth series: atomic notify, range locks for unmap, a busy shared server

15 November 2026, 13:33 to 13:56 (a first pair of passes, 12:52 to 13:15,
was discarded: see the corrections at the end). Three things the third
series left unmeasured, each as a switch or a workload (README, "Switches
and suites (fourth measurement)", which also lists what a kernel must
guarantee for each to be legal):

- `a3n`: the signal of a group is one atomic OR on the notify's word; the
  notify's lock is taken only when its thread waits, to wake it
  (`sync::Notify`; the no-lost-wake-up argument is in its comment, and
  `notify_loses_no_wake_up` hammers the race; with the registration made
  check-then-set instead of one fetch-or, the test fails in 3 runs out of 3).
  Candidate 1 gets the same: the notify's node applies the message with that
  OR instead of `W/2`. Three regimes: as in `a3t`; a busy receiver (20 µs of
  its own work per wake-up: rarely waiting); an idle one (producers 20 µs
  apart: nearly always waiting).
- `br`: an unmap in a group under the lock of the one address-space range it
  changes instead of the group lock.
- `srv`: a passive server shared by a background client (50 µs requests,
  back to back, budget 120 µs per 250 µs) and a foreground client (5 µs
  requests every 125 to 375 µs), with 1 or 2 server threads, with and without
  the background client, under (a) FIFO, (b) lending of the waiter's
  context, (c) refusal of a call whose budget is below the longest request.

Two full passes of 5 repetitions, pooled (10 runs per cell); `srv` runs are
600 ms. Per cell, median throughput of pass 2 over pass 1: `a3n` p10 0.997,
median 1.001, p90 1.007; `br` 0.994, 1.000, 1.009; `srv` (background calls)
1.000, 1.000, 1.000. Latency p50, pass 2 over pass 1: p10 0.985 to p90 1.013
in every suite; most cells are identical to the clock tick. Within-pass
spread of throughput: median 1.1 % (`a3n`), 2.0 % (`br`), 0.1 % (`srv`), p90
2.7, 4.7 and 0.5 %. **Below, a throughput difference under 2 % or a latency
difference under one tick (0.04 µs) is called level, not a win.** Load
average 2.5 to 6 (the bench's own threads). Rows with k = 8 ran about half
on efficiency cores, as before; rows with k = 4 are the ones to trust.

### a3n: notify by one atomic OR (k = 4, `ipi` 1 µs)

Signals/s; signal latency p50 / p99.9 in µs over every signal (both passes
gave the same p50 to the tick; the p99.9 is given for pass 1 | pass 2);
share of signals that found the thread waiting and took the lock.

| Regime | W | 1 locked apply | 1 OR apply | 3 notify lock (third series) | 3 atomic | lock taken | 3 atomic against best of 1 |
|---|---|---|---|---|---|---|---|
| as a3t | 50 | 2.09M; 0.08 / 0.29 \| 0.29 | 2.09M; 0.08 / 0.33 \| 0.25 | 1.95M; 0.21 / 0.58 \| 0.66 | 1.93M; 0.25 / 0.50 \| 0.46 | 67 % | −8 % |
| as a3t | 200 | 1.94M; 0.21 / 0.37 \| 0.37 | 1.94M; 0.21 / 0.42 \| 0.37 | 1.67M; 0.42 / 1.00 \| 1.15 | 1.80M; 0.33 / 0.62 \| 0.58 | 67–70 % | −7 % |
| as a3t | 1000 | 1.56M; 0.58 / 0.78 \| 0.74 | 1.56M; 0.58 / 0.79 \| 0.79 | 1.06M; 1.62 / 8.0 \| 7.3 | 1.47M; 0.70 / 1.00 \| 1.04 | 67–69 % | −6 % |
| busy | 50 | 2.09M; 0.08 / 0.25 \| 0.25 | 2.09M; 0.08 / 0.33 \| 0.29 | 1.95M; 0.21 / 0.54 \| 0.54 | 2.10M; 0.08 / 0.25 \| 0.25 | 0 % | level (+0.4 %) |
| busy | 200 | 1.94M; 0.21 / 0.33 \| 0.42 | 1.94M; 0.21 / 0.42 \| 0.42 | 1.67M; 0.42 / 1.00 \| 0.95 | 1.96M; 0.21 / 0.37 \| 0.33 | 0 % | level (+0.9 %) |
| busy | 1000 | 1.56M; 0.58 / 0.79 \| 0.78 | 1.56M; 0.58 / 0.79 \| 0.83 | 1.17M; 1.15 / 4.7 \| 4.1 | 1.57M; 0.58 / 0.74 \| 0.74 | 0 % | level (+0.6 %) |
| idle | 50 | 116k; 0.08 / 3.14 \| 3.20 | 116k; 0.08 / 3.20 \| 3.23 | 115k; 0.21 / 1.20 \| 1.12 | 115k; 0.21 / 1.04 \| 0.95 | 87–88 % | p50 +0.13 µs, p99.9 −2.2 µs |
| idle | 200 | 115k; 0.21 / 3.26 \| 3.26 | 116k; 0.21 / 3.33 \| 3.39 | 114k; 0.46 / 1.25 \| 1.28 | 115k; 0.33 / 1.00 \| 1.20 | 83–87 % | p50 +0.12 µs, p99.9 −2.2 µs |
| idle | 1000 | 114k; 0.58 / 3.65 \| 3.74 | 114k; 0.58 / 3.74 \| 3.74 | 111k; 1.20 / 7.0 \| 5.4 | 113k; 0.70 / 1.79 \| 1.54 | 82 % | p50 +0.12 µs, p99.9 −2.0 µs |

`ipi` 0 gives the same picture (appendices G and H). Reading:

- **As in `a3t`, candidate 1 keeps the row, by 6 to 8 % instead of 8 % to
  1.7 times.** The atomic notify is level with the notify lock at W = 50 ns
  and gains 8 % at 200 ns and 39 % at 1 µs (the OR replaces `W/2` under the
  lock), but in this regime the waiting thread wakes, takes the bits and
  waits again within about a microsecond, so two signals in three find it
  waiting and take the slow path (the notify's lock, then the wake-up
  through the mailbox), and the waiter's registration takes the same lock.
  The margin is 6 to 8 times the pass-to-pass difference (under 1 %).
- **A busy receiver: level.** No signal takes the lock (0.0 % in every
  cell); candidate 3 is 0.4 to 0.9 % ahead of candidate 1 at k = 4 and 0.7 to
  1.6 % at k = 8, inside the noise. Not visible in these numbers, and not
  measured: candidate 1 interrupts the busy receiver's core once per batch
  of signals, the group does not interrupt it at all.
- **An idle receiver: the slow path costs one lock and a wake-up, about
  0.12 µs of median** (0.33 against 0.21 µs at W = 200 ns), three ticks, both
  passes. Throughput is set by the producers' 20 µs and is level. The p99.9
  is lower in the groups (0.95 to 1.8 µs against 3.1 to 3.7 µs for candidate
  1, in both passes and at every W and `ipi`, at k = 4 only; at k = 8
  candidate 1's p99.9 is 0.4 to 0.9 µs). The cause of that candidate-1 tail
  was not found; treat it as unexplained, not as a property of the design.
- **The OR apply also helps candidate 1** where its notify's node was the
  bottleneck: at k = 8, W = 1 µs (low trust), 1.87 M becomes 3.61 M
  signals/s, and the p99.9 of 344 to 348 µs (rings full, back-pressure, as in
  the first three series) becomes 0.87 µs. At k = 4 it changes nothing. The
  back-pressure reported since the first series came from charging `W/2` per
  applied signal on one core; with a one-OR apply it is gone.
- At k = 8 (low trust): as in `a3t`, candidate 1 (OR apply) ahead by 1.5 to
  10 % (3.44 against 3.61 M at W = 1 µs, `ipi` 0); busy receiver level.

### br: unmap with a lock per address-space range (k = 4; k = 8 in parentheses)

Unmaps/s; unmap p50 / p99.9 in µs.

| Initiators | ipi | W | 1 fair | 3 group lock | 3 range locks | Range against group lock | Range against 1 |
|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 467k; 0.74 / 7.9 | 467k; 0.74 / 7.5 | 474k; 0.70 / 7.4 | +1.5 %, level | +1.6 %, level |
| one | 0 | 200 | 418k; 1.00 / 8.4 | 439k; 0.87 / 8.3 | 447k; 0.87 / 8.0 | +1.8 %, level | +7 % |
| one | 1000 | 1000 | 228k; 2.93 / 14.9 | 245k; 2.66 / 13.4 | 247k; 2.62 / 12.9 | +1.0 %, level | +8 % |
| all | 0 | 50 | 1.17M; 1.07 / 10.6 | 1.09M; 2.27 / 11.4 | 1.50M; 1.25 / 8.8 | +37 % | +28 % (×1.70) |
| all | 0 | 200 | 1.04M; 1.43 / 12.8 | 944k; 2.82 / 12.6 | 1.39M; 1.46 / 9.3 | +47 % | +33 % (×1.79) |
| all | 0 | 1000 | 656k; 2.82 / 18.2 | 539k; 5.95 / 22.8 | 953k; 3.04 / 13.1 | +77 % | +45 % (×2.04) |
| all | 1000 | 50 | 731k; 2.26 / 15.2 | 1.07M; 2.24 / 12.3 | 1.13M; 1.53 / 11.3 | +6 % (pass difference 5 %) | +55 % (×1.54) |
| all | 1000 | 200 | 676k; 2.53 / 15.6 | 940k; 2.82 / 13.6 | 1.36M; 1.46 / 9.9 | +45 % | ×2.01 (×1.71) |
| all | 1000 | 1000 | 484k; 4.74 / 22.1 | 538k; 5.95 / 23.2 | 951k; 3.04 / 13.6 | +77 % | ×1.97 (×2.28) |

Against the second series (`bf`, same cells): candidate 3 with the group
lock and candidate 1 fair reproduce within 2 to 4 % (all, `ipi` 1 µs:
1.07M, 940k, 538k now against 1.09M, 956k, 541k then for candidate 3;
731k, 676k, 484k against 730k, 662k, 504k for candidate 1; all, `ipi` 0,
W = 200 ns: 944k and 1.04M against 960k and 1.04M; one initiator: 439k
against 441k).

- **With one initiator nothing changes**: there is no contention, so range
  locks and the group lock are level (1 to 2 %, at the edge of the noise),
  and candidate 3 keeps its 5 to 13 % lower p50 than candidate 1.
- **With every thread unmapping, the group lock was the limit, and range
  locks widen candidate 3's lead and reverse the second series' `ipi` 0
  row.** The second series had candidate 1 ahead by 6 to 21 % at `ipi` 0;
  with range locks candidate 3 is ahead by 28 to 45 % there (k = 8: 1.7 to
  2.0 times). At `ipi` 1 µs, where the group lock already led by 1.07 to
  1.49 times, range locks take the lead to 1.55 to 2.0 times. The unmap p50
  falls to candidate 1's level or below (1.46 against 1.43 µs at `ipi` 0,
  W = 200 ns; 1.46 against 2.53 µs at `ipi` 1 µs), and the p99.9 is lower
  (9 to 14 against 11 to 22 µs). The only small gain over the group lock is
  W = 50 ns with a 1 µs interrupt (+6 %, about this cell's pass difference),
  where the lock is held briefly and the interrupts dominate.
- **What is not covered**: an unmap that spans ranges, one that frees a
  page-table page, a map that installs an intermediate table, and above all
  the revocation of a memory capability whose derived mappings lie in
  several ranges or address spaces (removing a derivation subtree). That
  operation crosses ranges and spaces without bound, needs a lock of the
  whole space or hand-over-hand locking, and its lock order against the
  capability range locks is not settled; it is not measured, and the
  numbers above say nothing about it. Moves of cores during an unmap (`d`)
  were not rerun with range locks.

### srv: the busy shared server

Foreground call latency, p50 / p99 / p99.9 in µs (10 runs of 600 ms,
about 2400 calls each), with the background client, `ipi` 1 µs. "1":
candidate 1; "3": the server's threads as one group, clients alone; "3 all":
clients and server in one group. With one thread, "1" and "3" run the same
code (a control of the noise).

| Threads | Policy | 1 | 3 | 3 all | Background calls/s |
|---|---|---|---|---|---|
| 1 | (a) fifo | 50.9 / 170 / 174 | 50.9 / 168 / 173 | 50.7 / 170 / 174 | 9.58k |
| 1 | (b) lend | 33.3 / 58.9 / 64.0 | 33.3 / 58.9 / 67.1 | 33.8 / 59.9 / 63.0 | 10.83k to 10.90k |
| 1 | (c) refuse | 8.51 / 57.9 / 59.7 | 8.45 / 57.9 / 60.7 | 8.45 / 59.4 / 60.4 | 8.00k (4.0k refused/s) |
| 2 | (a) fifo | 9.60 / 14.8 / 34.6 | 8.70 / 12.4 / 27.5 | 8.45 / 11.6 / 30.6 | 9.57k |
| 2 | (b) lend | 9.60 / 14.4 / 42.0 | 8.64 / 12.4 / 28.8 | 8.45 / 11.6 / 33.2 | 9.57k |
| 2 | (c) refuse | 8.58 / 13.4 / 39.2 | 8.70 / 13.2 / 37.4 | 8.45 / 11.6 / 42.2 | 8.00k |

With `ipi` 0, one thread: fifo 51 to 52 / 172 to 173 µs (p50 / p99), lend
32 to 33 / 57.9, refuse 6.5 / 57; two threads 6.4 to 6.7 / 9.4 to 10.8 in
every column.

Without the background client (the common path, no contention), p50 in µs:

| Threads | ipi | (a) fifo: 1 / 3 / 3 all | (b) lend | (c) refuse |
|---|---|---|---|---|
| 1 | 0 | 6.40 / 6.40 / 6.40 | 6.40 / 6.40 / 6.40 | 6.40 / 6.40 / 6.40 |
| 1 | 1000 | 8.45 / 8.45 / 8.45 | 8.45 / 8.45 / 8.45 | 8.45 / 8.45 / 8.45 |
| 2 | 0 | 6.46 / 6.72 / 6.46 | 6.46 / 6.75 / 6.46 | 6.43 / 6.75 / 6.46 |
| 2 | 1000 | 8.45 / 9.86 / 8.45 | 8.45 / 9.41 / 8.45 | 8.45 / 9.79 / 8.45 |

- **With one server thread the policy decides everything and the candidate
  nothing**: the three layouts are within 1 µs of each other under every
  policy (the p99.9 within 4 µs), about the pass-to-pass difference of one
  cell. Under FIFO the foreground waits behind a background request that
  stops halfway for its budget: p50 51 µs, p99 168 to 172 µs (the rest of a
  request plus a stall of about 130 µs). Lending removes the stall: p99 58 to
  60 µs, at most one background request run on the foreground's budget; the
  background gets 13 to 14 % more calls through, paid by the foreground's
  budget. Refusal removes it too: p99 57 to 59 µs, and the p50 drops to 6.5
  to 8.5 µs because the server now idles 60 % of the time; the background
  loses 16 % of its calls. **Neither policy brings the tail below one long
  request (50 µs); only the written rule does.**
- **Two threads (the written rule)**: p50 6.4 to 9.6 µs, p99 9.4 to 15 µs,
  under every policy (nobody queues, so the policies are level). Here the
  candidate matters, through interrupts: with `ipi` 1 µs and the background
  client holding the first server thread, candidate 1 is 0.9 to 1.2 µs
  slower (9.60 against 8.70 and 8.45 µs, 10 to 14 %), because its second
  server thread is on another node and a call to the busy endpoint costs a
  dispatch message and one more interrupt. Under refusal the first thread is
  often free and the gap closes (8.58 against 8.70 and 8.45). With `ipi` 0
  the gap is 0.1 to 0.3 µs.
- **The server group without contention is slower than it should be**: with
  two idle threads, a call may interrupt the second thread, which hands the
  request to the first (the endpoint picks the lowest free thread), so two
  interrupts instead of one: 9.4 to 9.9 µs against 8.45 µs. A kernel would
  serve the call on the core that took the interrupt. This is a weakness of
  the model against candidate 3 in that one row, not a measured cost.
- **What (b) and (c) cost when nobody contends: nothing this bench can
  resolve.** With one thread, and in the "1" and "3 all" layouts with two,
  the p50 is identical to the tick in every policy (6.40, 6.46 and 8.45 µs).
  In the server-group layout with two threads the p50 moves by up to 0.45 µs
  between policies (9.86, 9.41, 9.79 µs), as much as the two passes of one
  cell differ, and the policies' branches are not even taken there. The
  checks are a comparison on the caller's own grant (c) and one field of the
  endpoint (b): a few nanoseconds, below the 41.7 ns tick.
- Modelling limits (README): no third load competes for a server core, so a
  lent class matters only through the budget; with such a load FIFO would be
  worse and lending better. Budget donation across nodes (candidate 1, two
  threads) is assumed to need only the grant carried by the call and the
  reply.

### Which candidate each fourth-series row favours

| Row (k = 4) | Favours | Margin | Noise |
|---|---|---|---|
| Many-to-one signal, as in `a3t`, atomic notify | 1 | 6 to 8 % | ±1 % |
| Many-to-one signal, busy receiver | Level | 0.4 to 0.9 % | ±1 % |
| Many-to-one signal, idle receiver | 1 in the median, 3 in the p99.9 | p50 +0.12 µs; p99.9 −2.0 to −2.2 µs (cause in candidate 1 unknown) | one tick |
| Unmap, one initiator, range locks | 3, unchanged | 5 to 13 % lower p50 | ±1 % |
| Unmap, all threads, range locks | 3 | 28 to 45 % (`ipi` 0); 1.55 to 2.0 times (`ipi` 1 µs) | ±1 % |
| Busy server, 1 thread | Level; the policy decides | p99: fifo 170, lend 59, refuse 58 µs | ±1 µs |
| Busy server, 2 threads, contended | 3 with `ipi` 1 µs; level with `ipi` 0 | 0.9 to 1.2 µs (10 to 14 %); 0.1 to 0.3 µs | ±0.5 µs |
| Policy cost, uncontended | – | not resolved (under 0.04 µs) | – |

### What the fourth series still cannot decide on this Mac

- The real interrupt cost: it decides the two-thread server row, and how
  much candidate 1's per-batch interrupt of a busy receiver costs it.
- Whether a kernel can meet the range-lock conditions, and how a subtree
  revocation would be locked: not measured.
- A notify whose registration is also lock-free (only the wake-up locked)
  could shrink the slow path that still costs candidate 3 the `a3t` regime;
  not measured.
- Competing load at an intermediate class on the server's core, which would
  separate lending from FIFO further.
- Everything about the host listed for the first series: no pinning,
  efficiency cores at k ≥ 6, 41.7 ns clock, host preemption in the p99.9.

### Corrections: the first three series, and this one

- **An interrupt charged twice in groups.** `poll` let every core of a
  group that saw the doorbell during its 1 µs interrupt entry pay an
  interrupt for the same message. This touched only candidate 2 where a
  process spans groups (and signals to another group), at `ipi` 1 µs.
  Checked by running the third series' binary and the fixed one side by side
  (`a3t` and `bf`, k ≤ 4, 3 repetitions each): candidates 1 and 3 within 3 %
  in every cell (median 0.6 %); candidate 2 with groups of 2 gains 3 to 16 %
  in `a3t` (median 6 %) and up to 14 % in `bf` (`all`, W = 50 ns). The
  earlier tables therefore overstate the cost of fixed groups by up to that
  much in those cells; no conclusion about candidates 1 and 3 changes. Fixed
  in this series (one core of a node takes one interrupt per batch); the
  earlier tables are left as measured.
- **A first fix of that was wrong, and two passes were discarded.** It took
  the doorbell and the invalidation mailbox before charging the interrupt,
  which lost what arrives during the entry (a handler reads its mailbox
  after the entry) and let a core in user code take an acknowledgement from
  a core spinning in the kernel. The side-by-side check above caught it
  (candidate 3 lost 15 to 34 % in `bf`, `all`, `ipi` 1 µs, where it never
  receives a message); the two passes run with it were discarded and the
  series rerun. In the discarded passes `a3n` and `srv` agreed with the
  kept ones within the noise (they send no message to a group of several
  cores); `br` at `ipi` 1 µs did not.
- **A lost-update race on the space-wide seqlock counter** (`write_stripes`
  in `sim.rs`): with fine locks, two changes of different ranges both
  increment the counter of the whole capability space with a plain load and
  store, so increments can be lost and the counter can stay odd. No measured
  setting reads that counter with fine locks (the slot read uses the range
  counters), so no result is wrong, but every fine change writes one more
  shared line than it needs: a small bias against fine locks in `a1t` and
  `ct`. Fine locks with the whole-call seqlock would be incorrect. Not fixed,
  so the third series' code path stays as it ran.
- **Small**: the third series summary gives candidate 1's IPC lead at k = 4
  as "1 to 2 %"; the table gives 1.0 to 2.4 %.

### Tables

Raw runs: `data/pass-q1.csv.gz`, `data/pass-q2.csv.gz` (the workload name
`Server { threads: 1; bg: On }` is written with `;` instead of `,`). Full
tables of both passes: appendices G and H.

## Appendix A: full tables, pass A


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 16.6 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite locks: one group of k cores, three locks

load average before: { 1.92 4.97 4.85 }
load average after: { 4.37 5.02 4.87 }; 82 s

Throughput, operations/s, median (min–max)

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 5.66M (5.14M–5.69M) | 6.87M (4.81M–6.92M) | 6.78M (3.14M–6.82M) | 92% | 45 |
| 0 | 50 | 3 | 5.67M (4.94M–5.69M) | 6.23M (4.71M–6.30M) | 6.20M (4.20M–6.22M) | 94% | 26 |
| 0 | 50 | 4 | 5.64M (5.42M–5.65M) | 5.86M (4.92M–5.89M) | 6.15M (5.09M–6.19M) | 97% | 0 |
| 0 | 50 | 6 | 5.00M (4.94M–5.03M) | 4.42M (4.28M–4.44M) | 4.59M (2.09M–4.63M) | 68% | 66 |
| 0 | 50 | 8 | 4.82M (4.56M–4.84M) | 4.08M (3.49M–4.10M) | 4.47M (4.37M–4.57M) | 46% | 179 |
| 0 | 200 | 2 | 3.19M (3.16M–3.29M) | 3.56M (3.56M–3.60M) | 3.56M (3.54M–3.60M) | 100% | 3 |
| 0 | 200 | 3 | 3.13M (3.13M–3.17M) | 3.44M (3.42M–3.45M) | 3.41M (3.35M–3.43M) | 100% | 8 |
| 0 | 200 | 4 | 3.07M (2.68M–3.08M) | 3.31M (2.82M–3.31M) | 3.41M (3.01M–3.41M) | 97% | 19 |
| 0 | 200 | 6 | 2.87M (2.84M–2.89M) | 2.55M (2.52M–2.57M) | 2.70M (2.64M–2.71M) | 74% | 49 |
| 0 | 200 | 8 | 2.75M (2.72M–2.77M) | 2.46M (2.43M–2.46M) | 2.64M (2.29M–2.68M) | 59% | 58 |
| 0 | 1000 | 2 | 903k (897k–908k) | 924k (922k–925k) | 927k (923k–930k) | 100% | 2 |
| 0 | 1000 | 3 | 897k (896k–898k) | 916k (915k–916k) | 916k (916k–919k) | 100% | 0 |
| 0 | 1000 | 4 | 895k (892k–896k) | 911k (903k–914k) | 914k (913k–918k) | 99% | 6 |
| 0 | 1000 | 6 | 850k (844k–851k) | 842k (831k–845k) | 853k (849k–858k) | 68% | 40 |
| 0 | 1000 | 8 | 855k (852k–859k) | 833k (830k–834k) | 852k (849k–857k) | 59% | 1085 |
| 1000 | 50 | 2 | 1.36M (1.26M–1.36M) | 1.34M (1.27M–1.36M) | 1.35M (1.34M–1.36M) | 96% | 7 |
| 1000 | 50 | 3 | 2.01M (2.00M–2.02M) | 2.01M (1.99M–2.02M) | 2.01M (1.99M–2.02M) | 100% | 0 |
| 1000 | 50 | 4 | 2.68M (2.65M–2.68M) | 2.67M (2.60M–2.68M) | 2.67M (2.63M–2.68M) | 99% | 0 |
| 1000 | 50 | 6 | 3.73M (3.60M–3.77M) | 3.60M (3.51M–3.71M) | 3.72M (3.69M–3.76M) | 65% | 12 |
| 1000 | 50 | 8 | 4.07M (3.79M–4.09M) | 4.28M (4.24M–4.36M) | 4.40M (4.36M–4.46M) | 50% | 41 |
| 1000 | 200 | 2 | 1.25M (1.24M–1.25M) | 1.23M (1.22M–1.23M) | 1.24M (1.23M–1.24M) | 100% | 0 |
| 1000 | 200 | 3 | 1.85M (1.81M–1.85M) | 1.83M (1.82M–1.83M) | 1.84M (1.83M–1.85M) | 100% | 0 |
| 1000 | 200 | 4 | 2.45M (2.37M–2.46M) | 2.42M (2.30M–2.43M) | 2.44M (2.31M–2.44M) | 94% | 0 |
| 1000 | 200 | 6 | 2.52M (2.36M–2.58M) | 2.74M (2.62M–2.74M) | 2.80M (1.05M–2.81M) | 64% | 120 |
| 1000 | 200 | 8 | 2.51M (1.79M–2.54M) | 2.57M (937k–2.57M) | 2.79M (1.14M–2.85M) | 49% | 335 |
| 1000 | 1000 | 2 | 828k (814k–829k) | 820k (814k–821k) | 824k (812k–829k) | 99% | 0 |
| 1000 | 1000 | 3 | 848k (590k–850k) | 919k (625k–919k) | 920k (874k–921k) | 94% | 54 |
| 1000 | 1000 | 4 | 850k (813k–856k) | 915k (723k–918k) | 915k (821k–916k) | 84% | 74 |
| 1000 | 1000 | 6 | 824k (774k–825k) | 830k (10k–848k) | 852k (480k–860k) | 54% | 270 |
| 1000 | 1000 | 8 | 833k (588k–834k) | 827k (355k–837k) | 847k (566k–862k) | 38% | 573 |

Latency of a locked operation, µs: p50 / p99 / p99.9 (p99.9 min–max)

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.29 / 0.87 / 6.53 (4.54–13.06) | 0.25 / 0.29 / 0.33 (0.33–7.49) | 0.25 / 0.29 / 0.33 (0.33–10.11) | 92% | 45 |
| 0 | 50 | 3 | 0.29 / 4.67 / 33.28 (31.74–37.89) | 0.42 / 0.50 / 0.54 (0.54–3.62) | 0.42 / 0.50 / 0.54 (0.54–4.74) | 94% | 26 |
| 0 | 50 | 4 | 0.29 / 12.80 / 46.08 (44.03–46.08) | 0.62 / 0.70 / 0.91 (0.87–1.12) | 0.58 / 0.66 / 0.91 (0.74–1.04) | 97% | 0 |
| 0 | 50 | 6 | 0.33 / 23.55 / 70.66 (69.63–72.70) | 1.25 / 1.57 / 2.24 (2.14–4.16) | 1.20 / 1.49 / 3.62 (2.53–4.86) | 68% | 66 |
| 0 | 50 | 8 | 0.33 / 34.30 / 89.09 (89.09–91.14) | 1.87 / 2.21 / 7.10 (5.44–20.48) | 1.66 / 2.00 / 7.74 (6.40–19.71) | 46% | 179 |
| 0 | 200 | 2 | 0.29 / 1.54 / 37.38 (16.13–76.80) | 0.50 / 0.54 / 0.58 (0.58–0.62) | 0.50 / 0.54 / 0.62 (0.58–0.62) | 100% | 3 |
| 0 | 200 | 3 | 0.29 / 12.93 / 50.18 (48.13–57.86) | 0.83 / 0.91 / 1.00 (1.00–1.20) | 0.83 / 0.91 / 1.00 (1.00–1.28) | 100% | 8 |
| 0 | 200 | 4 | 0.58 / 19.20 / 46.08 (44.03–59.39) | 1.12 / 1.33 / 1.95 (1.70–2.43) | 1.12 / 1.28 / 1.66 (1.41–4.35) | 97% | 19 |
| 0 | 200 | 6 | 0.33 / 33.28 / 86.02 (74.75–109.57) | 2.24 / 2.53 / 6.91 (3.81–7.68) | 2.11 / 2.43 / 6.46 (3.20–6.85) | 74% | 49 |
| 0 | 200 | 8 | 0.42 / 44.54 / 99.33 (95.23–112.64) | 3.10 / 3.49 / 11.01 (10.24–11.39) | 2.85 / 3.33 / 10.11 (8.58–11.14) | 59% | 58 |
| 0 | 1000 | 2 | 1.04 / 32.00 / 89.09 (80.90–119.81) | 2.11 / 2.14 / 2.27 (2.21–5.57) | 2.08 / 2.14 / 2.24 (2.21–5.70) | 100% | 2 |
| 0 | 1000 | 3 | 2.11 / 52.74 / 155.65 (153.60–163.84) | 3.20 / 3.33 / 7.55 (7.42–9.09) | 3.20 / 3.33 / 8.19 (4.35–8.32) | 100% | 0 |
| 0 | 1000 | 4 | 2.14 / 80.90 / 202.75 (192.51–206.85) | 4.29 / 4.48 / 9.86 (7.62–11.65) | 4.29 / 4.42 / 11.52 (7.74–12.42) | 99% | 6 |
| 0 | 1000 | 6 | 1.15 / 97.28 / 196.61 (180.22–219.14) | 6.98 / 7.23 / 14.21 (13.70–15.23) | 6.85 / 7.23 / 14.21 (12.03–15.10) | 68% | 40 |
| 0 | 1000 | 8 | 1.12 / 137.22 / 622.59 (573.44–688.13) | 9.34 / 9.73 / 18.69 (16.13–20.74) | 9.22 / 9.73 / 18.94 (16.64–19.97) | 59% | 1085 |
| 1000 | 50 | 2 | 0.12 / 0.25 / 0.33 (0.33–0.58) | 0.12 / 0.29 / 0.33 (0.33–5.76) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 96% | 7 |
| 1000 | 50 | 3 | 0.12 / 0.29 / 0.33 (0.33–0.50) | 0.12 / 0.29 / 0.33 (0.33–0.37) | 0.12 / 0.29 / 0.33 (0.33–0.37) | 100% | 0 |
| 1000 | 50 | 4 | 0.12 / 0.33 / 0.50 (0.37–0.79) | 0.12 / 0.33 / 0.42 (0.37–0.66) | 0.12 / 0.33 / 0.50 (0.37–0.58) | 99% | 0 |
| 1000 | 50 | 6 | 0.25 / 0.83 / 5.12 (2.69–8.96) | 0.33 / 0.62 / 0.83 (0.70–8.19) | 0.29 / 0.54 / 0.79 (0.70–3.52) | 65% | 12 |
| 1000 | 50 | 8 | 0.29 / 11.01 / 28.67 (28.42–31.49) | 0.54 / 0.91 / 1.41 (1.15–6.21) | 0.46 / 0.87 / 1.54 (1.15–1.87) | 50% | 41 |
| 1000 | 200 | 2 | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 100% | 0 |
| 1000 | 200 | 3 | 0.25 / 0.42 / 0.58 (0.54–6.40) | 0.29 / 0.42 / 0.50 (0.50–0.54) | 0.29 / 0.46 / 0.50 (0.50–0.66) | 100% | 0 |
| 1000 | 200 | 4 | 0.25 / 0.54 / 0.58 (0.58–1.00) | 0.29 / 0.46 / 0.70 (0.50–6.02) | 0.29 / 0.46 / 0.66 (0.54–5.44) | 94% | 0 |
| 1000 | 200 | 6 | 0.54 / 16.64 / 39.94 (38.40–40.45) | 0.87 / 1.15 / 2.11 (1.33–6.72) | 0.83 / 1.12 / 1.57 (1.36–27.14) | 64% | 120 |
| 1000 | 200 | 8 | 0.58 / 27.90 / 55.81 (41.47–57.34) | 1.78 / 2.08 / 7.49 (7.10–49.66) | 1.49 / 1.95 / 8.00 (4.06–135.17) | 49% | 335 |
| 1000 | 1000 | 2 | 1.04 / 1.33 / 1.58 (1.33–2.11) | 1.07 / 1.25 / 1.33 (1.33–1.46) | 1.07 / 1.25 / 1.33 (1.33–8.32) | 99% | 0 |
| 1000 | 1000 | 3 | 2.11 / 15.49 / 26.11 (23.81–64.00) | 1.87 / 2.03 / 6.27 (2.24–10.88) | 1.87 / 2.03 / 3.10 (2.27–6.98) | 94% | 54 |
| 1000 | 1000 | 4 | 2.11 / 36.35 / 77.82 (70.66–100.35) | 2.98 / 3.23 / 8.32 (6.21–24.06) | 2.98 / 3.26 / 9.98 (8.45–23.81) | 84% | 74 |
| 1000 | 1000 | 6 | 1.25 / 86.02 / 176.13 (157.70–184.32) | 5.70 / 6.08 / 14.85 (12.42–28573.70) | 5.63 / 6.08 / 15.36 (10.50–1097.73) | 54% | 270 |
| 1000 | 1000 | 8 | 2.14 / 109.57 / 225.28 (215.04–868.35) | 8.19 / 8.58 / 17.41 (14.72–4456.45) | 7.94 / 8.45 / 18.69 (17.66–1261.57) | 38% | 573 |

Fairness: fewest operations of a core / most

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.95 | 1.00 | 1.00 | 92% | 45 |
| 0 | 50 | 3 | 0.85 | 1.00 | 1.00 | 94% | 26 |
| 0 | 50 | 4 | 0.82 | 1.00 | 1.00 | 97% | 0 |
| 0 | 50 | 6 | 0.84 | 1.00 | 1.00 | 68% | 66 |
| 0 | 50 | 8 | 0.92 | 1.00 | 1.00 | 46% | 179 |
| 0 | 200 | 2 | 0.80 | 1.00 | 1.00 | 100% | 3 |
| 0 | 200 | 3 | 0.91 | 1.00 | 1.00 | 100% | 8 |
| 0 | 200 | 4 | 0.76 | 1.00 | 1.00 | 97% | 19 |
| 0 | 200 | 6 | 0.85 | 1.00 | 1.00 | 74% | 49 |
| 0 | 200 | 8 | 0.81 | 1.00 | 1.00 | 59% | 58 |
| 0 | 1000 | 2 | 0.75 | 1.00 | 1.00 | 100% | 2 |
| 0 | 1000 | 3 | 0.82 | 1.00 | 1.00 | 100% | 0 |
| 0 | 1000 | 4 | 0.76 | 1.00 | 1.00 | 99% | 6 |
| 0 | 1000 | 6 | 0.94 | 1.00 | 1.00 | 68% | 40 |
| 0 | 1000 | 8 | 0.80 | 1.00 | 1.00 | 59% | 1085 |
| 1000 | 50 | 2 | 1.00 | 0.99 | 1.00 | 96% | 7 |
| 1000 | 50 | 3 | 1.00 | 1.00 | 1.00 | 100% | 0 |
| 1000 | 50 | 4 | 1.00 | 1.00 | 1.00 | 99% | 0 |
| 1000 | 50 | 6 | 1.00 | 0.99 | 0.99 | 65% | 12 |
| 1000 | 50 | 8 | 0.98 | 1.00 | 1.00 | 50% | 41 |
| 1000 | 200 | 2 | 1.00 | 1.00 | 1.00 | 100% | 0 |
| 1000 | 200 | 3 | 1.00 | 1.00 | 1.00 | 100% | 0 |
| 1000 | 200 | 4 | 1.00 | 1.00 | 1.00 | 94% | 0 |
| 1000 | 200 | 6 | 0.96 | 1.00 | 0.99 | 64% | 120 |
| 1000 | 200 | 8 | 0.96 | 1.00 | 1.00 | 49% | 335 |
| 1000 | 1000 | 2 | 1.00 | 1.00 | 1.00 | 99% | 0 |
| 1000 | 1000 | 3 | 0.96 | 1.00 | 1.00 | 94% | 54 |
| 1000 | 1000 | 4 | 0.90 | 1.00 | 1.00 | 84% | 74 |
| 1000 | 1000 | 6 | 0.78 | 1.00 | 1.00 | 54% | 270 |
| 1000 | 1000 | 8 | 0.82 | 1.00 | 1.00 | 38% | 573 |

Migrations seen: 45636 over 171796238 samples (one sample per measured operation).


### Suite a1: capability operations of one k-thread process

load average before: { 4.37 5.02 4.87 }
load average after: { 8.23 6.12 5.36 }; 262 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 703k (694k–704k) | 702k (696k–705k) | 701k (698k–703k) | 701k (688k–706k) | 96% | 0 |
| 1 | 0 | 50 | 2 | 1.39M (1.21M–1.40M) | 1.36M (1.32M–1.36M) | 1.36M (1.10M–1.36M) | 1.35M (1.30M–1.36M) | 94% | 18 |
| 1 | 0 | 50 | 3 | 2.07M (2.05M–2.07M) | 2.02M (1.95M–2.04M) | 1.98M (1.89M–2.01M) | 1.97M (1.63M–2.00M) | 91% | 18 |
| 1 | 0 | 50 | 4 | 2.74M (2.69M–2.75M) | 2.60M (2.54M–2.64M) | 2.54M (2.35M–2.61M) | 2.50M (2.32M–2.64M) | 84% | 31 |
| 1 | 0 | 50 | 6 | 3.99M (2.72M–4.10M) | 3.80M (3.70M–3.86M) | 3.75M (3.69M–3.80M) | 3.70M (3.48M–3.74M) | 57% | 249 |
| 1 | 0 | 50 | 8 | 5.38M (5.28M–5.42M) | 4.96M (4.33M–5.12M) | 4.84M (4.54M–4.94M) | 4.08M (3.63M–4.68M) | 46% | 131 |
| 1 | 0 | 200 | 1 | 644k (632k–646k) | 643k (637k–647k) | 641k (633k–645k) | 639k (596k–648k) | 93% | 2 |
| 1 | 0 | 200 | 2 | 1.27M (1.19M–1.28M) | 1.21M (1.07M–1.24M) | 1.21M (1.15M–1.25M) | 1.21M (1.15M–1.23M) | 90% | 11 |
| 1 | 0 | 200 | 3 | 1.87M (1.20M–1.90M) | 1.83M (1.80M–1.86M) | 1.80M (1.75M–1.84M) | 1.79M (1.70M–1.84M) | 89% | 33 |
| 1 | 0 | 200 | 4 | 2.51M (2.49M–2.53M) | 2.35M (2.33M–2.43M) | 2.31M (1.91M–2.41M) | 2.31M (464k–2.41M) | 84% | 62 |
| 1 | 0 | 200 | 6 | 3.73M (851k–3.76M) | 3.46M (2.94M–3.50M) | 3.44M (2.30M–3.45M) | 2.78M (2.47M–2.82M) | 59% | 339 |
| 1 | 0 | 200 | 8 | 4.91M (687k–4.92M) | 4.55M (4.45M–4.65M) | 4.38M (3.99M–4.49M) | 2.76M (2.36M–2.86M) | 48% | 484 |
| 1 | 0 | 1000 | 1 | 426k (425k–427k) | 427k (418k–427k) | 425k (419k–427k) | 427k (421k–427k) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 841k (839k–844k) | 825k (819k–829k) | 824k (813k–827k) | 825k (822k–829k) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.25M (1.25M–1.25M) | 1.23M (1.22M–1.23M) | 919k (915k–920k) | 916k (914k–922k) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.65M (1.64M–1.66M) | 1.58M (1.55M–1.60M) | 883k (876k–915k) | 907k (863k–918k) | 92% | 16 |
| 1 | 0 | 1000 | 6 | 2.46M (2.44M–2.48M) | 2.31M (2.29M–2.34M) | 1.61M (1.44M–1.63M) | 840k (799k–851k) | 64% | 105 |
| 1 | 0 | 1000 | 8 | 3.27M (3.23M–3.29M) | 3.07M (2.99M–3.11M) | 1.67M (1.61M–1.67M) | 827k (800k–847k) | 48% | 126 |
| 1 | 1000 | 50 | 1 | 701k (699k–705k) | 702k (692k–703k) | 701k (687k–703k) | 702k (697k–705k) | 99% | 0 |
| 1 | 1000 | 50 | 2 | 1.37M (1.28M–1.38M) | 1.35M (1.29M–1.36M) | 1.36M (1.30M–1.36M) | 1.36M (1.30M–1.36M) | 97% | 4 |
| 1 | 1000 | 50 | 3 | 2.02M (1.99M–2.02M) | 1.98M (1.94M–2.00M) | 1.97M (1.95M–2.02M) | 2.01M (1.88M–2.02M) | 98% | 0 |
| 1 | 1000 | 50 | 4 | 2.63M (2.61M–2.67M) | 2.56M (2.21M–2.59M) | 2.56M (2.47M–2.63M) | 2.57M (2.46M–2.65M) | 87% | 19 |
| 1 | 1000 | 50 | 6 | 3.86M (3.82M–3.87M) | 3.65M (3.60M–3.68M) | 3.61M (3.52M–3.64M) | 3.62M (3.56M–3.73M) | 62% | 44 |
| 1 | 1000 | 50 | 8 | 5.07M (5.05M–5.11M) | 4.75M (4.57M–4.81M) | 4.62M (4.50M–4.71M) | 4.11M (3.87M–4.61M) | 47% | 66 |
| 1 | 1000 | 200 | 1 | 645k (636k–647k) | 644k (644k–645k) | 641k (638k–645k) | 645k (638k–647k) | 99% | 0 |
| 1 | 1000 | 200 | 2 | 1.26M (1.25M–1.26M) | 1.23M (1.22M–1.24M) | 1.22M (1.21M–1.24M) | 1.23M (1.22M–1.24M) | 99% | 0 |
| 1 | 1000 | 200 | 3 | 1.85M (1.83M–1.86M) | 1.82M (1.80M–1.83M) | 1.84M (1.78M–1.85M) | 1.82M (1.76M–1.84M) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 2.44M (2.36M–2.45M) | 2.34M (2.28M–2.36M) | 2.38M (2.28M–2.39M) | 2.33M (2.29M–2.39M) | 89% | 11 |
| 1 | 1000 | 200 | 6 | 3.62M (3.53M–3.64M) | 3.38M (3.35M–3.41M) | 3.33M (2.75M–3.35M) | 2.73M (2.70M–2.84M) | 61% | 81 |
| 1 | 1000 | 200 | 8 | 4.65M (3.53M–4.70M) | 4.36M (4.18M–4.43M) | 4.33M (4.21M–4.38M) | 2.77M (2.54M–2.85M) | 47% | 268 |
| 1 | 1000 | 1000 | 1 | 425k (415k–426k) | 426k (415k–426k) | 426k (419k–426k) | 426k (417k–426k) | 100% | 0 |
| 1 | 1000 | 1000 | 2 | 833k (817k–837k) | 826k (821k–828k) | 826k (808k–828k) | 822k (806k–829k) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 1.23M (1.22M–1.23M) | 1.22M (1.19M–1.22M) | 916k (879k–922k) | 919k (878k–920k) | 96% | 0 |
| 1 | 1000 | 1000 | 4 | 1.62M (1.57M–1.62M) | 1.58M (787k–1.60M) | 915k (831k–917k) | 915k (731k–919k) | 91% | 88 |
| 1 | 1000 | 1000 | 6 | 2.39M (135k–2.41M) | 2.27M (1.75M–2.29M) | 1.60M (323k–1.64M) | 846k (715k–852k) | 63% | 404 |
| 1 | 1000 | 1000 | 8 | 3.16M (2.04M–3.18M) | 2.95M (1.65M–3.00M) | 1.67M (929k–1.70M) | 840k (337k–848k) | 49% | 557 |
| 10 | 0 | 50 | 1 | 702k (687k–704k) | 699k (687k–705k) | 701k (679k–703k) | 702k (641k–704k) | 95% | 0 |
| 10 | 0 | 50 | 2 | 1.33M (698k–1.34M) | 1.35M (1.23M–1.36M) | 1.36M (1.15M–1.36M) | 1.36M (1.20M–1.36M) | 91% | 36 |
| 10 | 0 | 50 | 3 | 1.92M (1.86M–1.93M) | 1.92M (1.84M–1.92M) | 2.01M (1.92M–2.01M) | 1.98M (1.94M–2.01M) | 97% | 3 |
| 10 | 0 | 50 | 4 | 2.49M (2.19M–2.50M) | 2.46M (2.31M–2.48M) | 2.60M (2.47M–2.62M) | 2.58M (2.47M–2.60M) | 94% | 30 |
| 10 | 0 | 50 | 6 | 3.33M (3.31M–3.37M) | 3.21M (3.19M–3.25M) | 3.30M (3.25M–3.34M) | 3.64M (3.50M–3.68M) | 64% | 51 |
| 10 | 0 | 50 | 8 | 4.18M (3.96M–4.19M) | 4.01M (3.90M–4.05M) | 4.12M (4.11M–4.14M) | 4.28M (3.21M–4.46M) | 49% | 114 |
| 10 | 0 | 200 | 1 | 645k (637k–646k) | 645k (643k–646k) | 644k (641k–646k) | 644k (637k–647k) | 99% | 0 |
| 10 | 0 | 200 | 2 | 1.21M (1.10M–1.21M) | 1.24M (1.17M–1.24M) | 1.23M (1.17M–1.24M) | 1.24M (1.20M–1.24M) | 96% | 9 |
| 10 | 0 | 200 | 3 | 1.74M (1.41M–1.74M) | 1.73M (1.08M–1.74M) | 1.83M (1.46M–1.84M) | 1.83M (1.68M–1.84M) | 90% | 57 |
| 10 | 0 | 200 | 4 | 2.19M (895k–2.25M) | 2.21M (2.03M–2.24M) | 2.39M (2.25M–2.41M) | 2.40M (2.26M–2.41M) | 82% | 118 |
| 10 | 0 | 200 | 6 | 2.94M (1.88M–3.06M) | 2.91M (1.29M–2.95M) | 2.97M (1.22M–3.00M) | 2.65M (1.08M–2.71M) | 61% | 241 |
| 10 | 0 | 200 | 8 | 3.55M (25k–3.77M) | 3.56M (2.23M–3.61M) | 3.63M (2.02M–3.67M) | 2.56M (1.10M–2.76M) | 47% | 944 |
| 10 | 0 | 1000 | 1 | 427k (420k–428k) | 426k (420k–427k) | 427k (422k–427k) | 426k (423k–427k) | 98% | 0 |
| 10 | 0 | 1000 | 2 | 763k (763k–766k) | 823k (802k–829k) | 823k (806k–827k) | 822k (815k–826k) | 98% | 0 |
| 10 | 0 | 1000 | 3 | 1.07M (1.07M–1.08M) | 1.08M (1.07M–1.08M) | 913k (907k–918k) | 912k (903k–920k) | 99% | 0 |
| 10 | 0 | 1000 | 4 | 1.36M (1.33M–1.36M) | 1.35M (1.31M–1.36M) | 896k (863k–914k) | 907k (869k–914k) | 90% | 8 |
| 10 | 0 | 1000 | 6 | 1.81M (1.81M–1.83M) | 1.71M (1.69M–1.74M) | 1.34M (1.33M–1.35M) | 824k (179k–843k) | 60% | 243 |
| 10 | 0 | 1000 | 8 | 2.19M (958k–2.22M) | 2.10M (1.08M–2.13M) | 1.39M (915k–1.43M) | 826k (810k–849k) | 47% | 779 |
| 10 | 1000 | 50 | 1 | 702k (682k–704k) | 702k (687k–703k) | 699k (622k–705k) | 702k (688k–704k) | 91% | 1 |
| 10 | 1000 | 50 | 2 | 1.18M (1.15M–1.18M) | 1.35M (1.28M–1.36M) | 1.34M (1.28M–1.36M) | 1.34M (1.25M–1.36M) | 94% | 0 |
| 10 | 1000 | 50 | 3 | 1.57M (1.10M–1.60M) | 1.64M (921k–1.66M) | 1.98M (1.73M–2.01M) | 2.01M (1.67M–2.02M) | 91% | 87 |
| 10 | 1000 | 50 | 4 | 1.98M (1.49M–2.00M) | 2.03M (1.96M–2.08M) | 2.57M (2.43M–2.62M) | 2.60M (2.47M–2.62M) | 88% | 42 |
| 10 | 1000 | 50 | 6 | 2.60M (2.41M–2.61M) | 2.59M (2.20M–2.63M) | 2.73M (2.61M–2.79M) | 3.65M (3.62M–3.70M) | 61% | 104 |
| 10 | 1000 | 50 | 8 | 3.14M (1.86M–3.18M) | 3.19M (3.17M–3.22M) | 3.43M (3.41M–3.45M) | 4.18M (3.75M–4.43M) | 48% | 431 |
| 10 | 1000 | 200 | 1 | 645k (625k–646k) | 645k (640k–646k) | 645k (641k–647k) | 645k (643k–648k) | 99% | 0 |
| 10 | 1000 | 200 | 2 | 1.08M (1.08M–1.08M) | 1.24M (1.22M–1.24M) | 1.23M (1.22M–1.24M) | 1.23M (1.21M–1.24M) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 1.46M (1.46M–1.47M) | 1.52M (1.51M–1.53M) | 1.84M (1.70M–1.84M) | 1.83M (1.71M–1.84M) | 96% | 0 |
| 10 | 1000 | 200 | 4 | 1.82M (1.80M–1.84M) | 1.91M (1.76M–1.91M) | 2.38M (2.27M–2.41M) | 2.39M (2.27M–2.40M) | 93% | 4 |
| 10 | 1000 | 200 | 6 | 2.39M (2.33M–2.40M) | 2.42M (2.40M–2.45M) | 2.54M (2.49M–2.56M) | 2.73M (2.66M–2.76M) | 61% | 58 |
| 10 | 1000 | 200 | 8 | 2.89M (2.62M–2.94M) | 2.95M (2.87M–2.98M) | 3.09M (3.03M–3.15M) | 2.72M (2.62M–2.84M) | 47% | 81 |
| 10 | 1000 | 1000 | 1 | 426k (410k–427k) | 426k (424k–427k) | 426k (358k–427k) | 427k (423k–428k) | 90% | 8 |
| 10 | 1000 | 1000 | 2 | 710k (505k–712k) | 824k (781k–828k) | 815k (753k–825k) | 825k (808k–826k) | 94% | 25 |
| 10 | 1000 | 1000 | 3 | 964k (955k–968k) | 999k (989k–1.00M) | 917k (884k–918k) | 916k (873k–919k) | 97% | 3 |
| 10 | 1000 | 1000 | 4 | 1.19M (1.16M–1.20M) | 1.27M (1.20M–1.27M) | 878k (869k–915k) | 909k (850k–916k) | 91% | 16 |
| 10 | 1000 | 1000 | 6 | 1.56M (1.54M–1.58M) | 1.59M (1.58M–1.62M) | 1.32M (1.31M–1.33M) | 826k (819k–849k) | 62% | 43 |
| 10 | 1000 | 1000 | 8 | 1.88M (1.87M–1.91M) | 1.91M (1.88M–1.95M) | 1.41M (1.34M–1.49M) | 830k (804k–844k) | 48% | 122 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 96% | 0 |
| 1 | 0 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 0.12 / 0.25 / 0.33 (0.33–0.46) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 94% | 18 |
| 1 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.37 (0.33–0.62) | 0.17 / 0.33 / 0.54 (0.37–0.66) | 0.17 / 0.29 / 0.42 (0.37–0.58) | 91% | 18 |
| 1 | 0 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.17 / 0.42 / 0.50 (0.37–0.58) | 0.25 / 0.50 / 0.62 (0.54–0.70) | 0.25 / 0.50 / 0.62 (0.42–5.70) | 84% | 31 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.42 / 0.66 (0.62–0.78) | 0.25 / 0.54 / 0.74 (0.74–0.91) | 0.29 / 0.58 / 0.70 (0.66–1.07) | 57% | 249 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.42 / 0.70 (0.70–0.87) | 0.25 / 0.58 / 1.00 (0.83–2.11) | 0.54 / 1.12 / 3.14 (0.83–8.45) | 46% | 131 |
| 1 | 0 | 200 | 1 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.46) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 93% | 2 |
| 1 | 0 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.42 / 0.66 (0.50–0.95) | 0.29 / 0.46 / 0.66 (0.50–4.74) | 0.29 / 0.42 / 0.62 (0.58–0.70) | 90% | 11 |
| 1 | 0 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.29 / 0.42 / 0.62 (0.50–3.81) | 0.29 / 0.50 / 0.74 (0.54–5.95) | 0.29 / 0.58 / 0.74 (0.54–9.22) | 89% | 33 |
| 1 | 0 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 0.74 (0.66–4.61) | 0.37 / 0.62 / 0.78 (0.70–3.55) | 0.37 / 0.62 / 1.28 (0.91–25.34) | 84% | 62 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.70 / 0.91 (0.83–1.00) | 0.37 / 0.70 / 1.15 (1.07–1.25) | 0.79 / 1.07 / 6.66 (5.57–22.02) | 59% | 339 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.66 / 1.12 (0.95–1.20) | 0.37 / 0.87 / 2.30 (1.28–10.88) | 1.49 / 2.11 / 13.82 (11.01–65.54) | 48% | 484 |
| 1 | 0 | 1000 | 1 | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.07–7.04) | 1.00 / 1.04 / 1.12 (1.07–6.40) | 1.00 / 1.04 / 1.07 (1.04–1.54) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.07 / 1.25 / 1.33 (1.33–8.19) | 1.07 / 1.25 / 4.22 (1.74–9.60) | 1.07 / 1.25 / 1.82 (1.33–2.21) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.12 (1.04–1.15) | 1.04 / 1.28 / 2.14 (2.11–4.16) | 1.87 / 2.03 / 7.49 (6.66–10.50) | 1.87 / 2.11 / 9.34 (3.23–10.37) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.04 / 1.12 (1.04–5.25) | 1.07 / 1.70 / 6.08 (2.21–11.01) | 2.98 / 3.52 / 16.38 (10.62–26.11) | 2.98 / 3.46 / 16.64 (10.11–16.90) | 92% | 16 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.12 / 1.95 / 2.50 (2.43–7.04) | 2.69 / 3.90 / 8.96 (7.87–11.39) | 5.70 / 6.53 / 17.15 (16.64–74.75) | 64% | 105 |
| 1 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–1.15) | 1.15 / 2.03 / 2.56 (2.43–5.18) | 3.36 / 4.06 / 14.59 (7.94–35.33) | 8.00 / 11.90 / 32.51 (19.71–212.99) | 48% | 126 |
| 1 | 1000 | 50 | 1 | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 99% | 0 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.29–0.50) | 0.12 / 0.25 / 0.33 (0.33–0.46) | 0.12 / 0.25 / 0.33 (0.33–0.46) | 97% | 4 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.37 (0.33–0.42) | 0.17 / 0.29 / 0.42 (0.33–4.42) | 0.17 / 0.29 / 0.37 (0.33–0.58) | 98% | 0 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.12 / 0.29 / 0.50 (0.37–0.58) | 0.17 / 0.42 / 0.70 (0.37–6.59) | 0.17 / 0.42 / 0.62 (0.50–5.95) | 87% | 19 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.21 / 0.46 / 0.74 (0.62–0.79) | 0.29 / 0.62 / 0.95 (0.74–1.79) | 0.29 / 0.62 / 0.87 (0.74–6.34) | 62% | 44 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.46 / 0.79 (0.74–0.95) | 0.29 / 0.62 / 0.95 (0.87–1.04) | 0.42 / 0.83 / 6.34 (1.90–26.11) | 47% | 66 |
| 1 | 1000 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 99% | 0 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.42 / 0.50 (0.46–0.74) | 0.29 / 0.46 / 0.54 (0.50–2.56) | 0.29 / 0.42 / 0.54 (0.50–0.62) | 99% | 0 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.29–0.37) | 0.25 / 0.37 / 0.54 (0.50–0.66) | 0.29 / 0.46 / 0.54 (0.50–6.27) | 0.29 / 0.46 / 0.74 (0.54–8.13) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.46) | 0.29 / 0.54 / 0.74 (0.70–2.78) | 0.29 / 0.54 / 0.95 (0.70–6.46) | 0.29 / 0.58 / 6.66 (0.79–9.09) | 89% | 11 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 0.91 (0.87–3.46) | 0.37 / 0.74 / 1.12 (1.12–1.25) | 0.83 / 1.33 / 6.40 (4.42–8.45) | 61% | 81 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 1.25 (1.00–4.93) | 0.37 / 0.87 / 1.28 (1.20–1.49) | 1.41 / 2.14 / 15.36 (13.18–45.57) | 47% | 268 |
| 1 | 1000 | 1000 | 1 | 1.00 / 1.04 / 1.12 (1.07–8.45) | 1.00 / 1.04 / 1.15 (1.07–8.45) | 1.00 / 1.04 / 1.07 (1.04–7.36) | 1.00 / 1.04 / 1.12 (1.12–6.91) | 100% | 0 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.04 / 1.07 (1.04–8.06) | 1.07 / 1.25 / 1.49 (1.33–5.82) | 1.07 / 1.25 / 4.06 (1.36–5.82) | 1.07 / 1.25 / 4.74 (1.62–8.19) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.20 (1.04–7.23) | 1.04 / 1.25 / 2.85 (2.11–9.09) | 1.87 / 2.11 / 7.94 (5.44–14.72) | 1.87 / 2.03 / 7.81 (6.53–9.73) | 96% | 0 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.12 (1.04–1.20) | 1.07 / 2.00 / 6.78 (2.21–32.77) | 2.98 / 3.33 / 13.70 (9.73–38.40) | 2.98 / 3.39 / 12.16 (8.96–13.95) | 91% | 88 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–26.62) | 1.12 / 2.08 / 2.50 (2.40–23.81) | 2.53 / 3.62 / 9.22 (6.78–1179.65) | 5.63 / 6.78 / 17.15 (13.18–76.80) | 63% | 404 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–6.72) | 1.15 / 2.11 / 2.91 (2.40–10.62) | 3.33 / 4.03 / 11.14 (10.37–42.50) | 7.87 / 13.70 / 51.71 (18.18–4980.74) | 49% | 557 |
| 10 | 0 | 50 | 1 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.29) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 95% | 0 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 0.12 / 0.25 / 0.33 (0.33–0.58) | 91% | 36 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.12 / 0.29 / 0.42 (0.42–0.58) | 0.12 / 0.29 / 0.37 (0.33–0.58) | 0.12 / 0.29 / 0.37 (0.37–4.06) | 97% | 3 |
| 10 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.12 / 0.33 / 0.50 (0.46–0.70) | 0.17 / 0.33 / 0.46 (0.42–0.66) | 0.17 / 0.37 / 0.58 (0.54–6.27) | 94% | 30 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.58 / 0.87 (0.83–1.25) | 0.25 / 0.70 / 1.00 (0.95–3.23) | 0.29 / 0.58 / 0.87 (0.70–3.26) | 64% | 51 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.70 / 1.04 (1.00–1.07) | 0.29 / 0.87 / 1.25 (1.15–1.28) | 0.46 / 1.00 / 3.78 (3.33–8.58) | 49% | 114 |
| 10 | 0 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 99% | 0 |
| 10 | 0 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.42 / 0.54 (0.50–0.66) | 0.29 / 0.42 / 0.50 (0.50–0.70) | 0.29 / 0.42 / 0.50 (0.46–0.54) | 96% | 9 |
| 10 | 0 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.25 / 0.50 / 0.66 (0.66–0.83) | 0.29 / 0.46 / 0.54 (0.50–0.74) | 0.29 / 0.46 / 0.54 (0.54–4.99) | 90% | 57 |
| 10 | 0 | 200 | 4 | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.29 / 0.58 / 0.74 (0.70–0.95) | 0.29 / 0.46 / 0.83 (0.62–8.00) | 0.29 / 0.46 / 0.83 (0.54–7.87) | 82% | 118 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.83 / 1.12 (1.04–1.28) | 0.37 / 1.04 / 1.49 (1.36–6.08) | 0.87 / 1.41 / 5.18 (3.04–14.85) | 61% | 241 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–3.20) | 0.33 / 0.91 / 1.28 (1.15–1.36) | 0.42 / 1.20 / 1.70 (1.66–11.65) | 1.62 / 2.24 / 9.98 (8.70–54.27) | 47% | 944 |
| 10 | 0 | 1000 | 1 | 1.00 / 1.04 / 1.07 (1.07–6.08) | 1.00 / 1.04 / 1.12 (1.07–1.12) | 1.00 / 1.04 / 1.07 (1.07–1.12) | 1.00 / 1.04 / 1.07 (1.04–1.15) | 98% | 0 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.07 / 1.25 / 1.36 (1.33–1.87) | 1.07 / 1.25 / 1.49 (1.33–7.87) | 1.07 / 1.25 / 1.57 (1.33–5.76) | 98% | 0 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.04 / 1.12 (1.07–4.22) | 1.07 / 2.08 / 2.24 (2.24–4.99) | 1.87 / 2.14 / 7.49 (6.21–10.50) | 1.87 / 2.03 / 7.04 (5.95–15.23) | 99% | 0 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.07 / 2.11 / 4.61 (2.43–6.40) | 2.98 / 3.49 / 15.36 (10.37–16.13) | 2.98 / 3.55 / 13.18 (9.86–14.08) | 90% | 8 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–5.12) | 1.20 / 2.30 / 2.85 (2.78–8.83) | 2.24 / 4.29 / 7.49 (4.99–17.15) | 5.70 / 6.34 / 15.74 (12.16–5767.17) | 60% | 243 |
| 10 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–6.53) | 1.20 / 2.40 / 3.10 (3.04–16.00) | 3.23 / 4.48 / 11.01 (9.09–27.90) | 8.00 / 9.60 / 22.78 (18.69–119.81) | 47% | 779 |
| 10 | 1000 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 91% | 1 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 0.12 / 0.29 / 0.37 (0.33–0.46) | 0.12 / 0.25 / 0.33 (0.33–0.50) | 94% | 0 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.12 / 0.29 / 0.42 (0.42–0.62) | 0.12 / 0.29 / 0.54 (0.33–2.30) | 0.12 / 0.29 / 0.37 (0.33–0.58) | 91% | 87 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.17 / 0.42 / 0.62 (0.46–0.70) | 0.17 / 0.42 / 1.12 (0.46–5.89) | 0.17 / 0.33 / 0.62 (0.46–6.34) | 88% | 42 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.58 / 0.83 (0.83–1.04) | 0.25 / 0.70 / 1.00 (1.00–4.54) | 0.29 / 0.58 / 1.41 (0.79–7.62) | 61% | 104 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.66 / 1.00 (0.95–1.00) | 0.25 / 0.79 / 1.12 (1.07–1.15) | 0.46 / 0.78 / 5.95 (1.66–8.70) | 48% | 431 |
| 10 | 1000 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 99% | 0 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.29 / 0.42 / 0.50 (0.50–0.62) | 0.29 / 0.42 / 0.54 (0.50–1.12) | 0.29 / 0.42 / 0.54 (0.50–4.99) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.25 / 0.50 / 0.62 (0.62–0.70) | 0.29 / 0.46 / 0.54 (0.50–2.78) | 0.29 / 0.46 / 0.66 (0.54–1.57) | 96% | 0 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.29 / 0.58 / 0.74 (0.70–1.15) | 0.29 / 0.46 / 1.49 (0.58–6.91) | 0.29 / 0.50 / 0.83 (0.66–6.21) | 93% | 4 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.83 / 1.12 (1.12–1.20) | 0.37 / 1.04 / 1.36 (1.36–5.82) | 0.83 / 1.15 / 6.91 (4.61–11.90) | 61% | 58 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.95 / 1.28 (1.25–1.41) | 0.42 / 1.15 / 1.79 (1.62–8.13) | 1.49 / 1.95 / 10.50 (8.45–20.99) | 47% | 81 |
| 10 | 1000 | 1000 | 1 | 1.00 / 1.04 / 1.07 (1.07–7.68) | 1.00 / 1.04 / 1.07 (1.07–2.00) | 1.00 / 1.04 / 1.12 (1.04–7.30) | 1.00 / 1.04 / 1.07 (1.04–4.48) | 90% | 8 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.04 / 1.07 (1.04–5.31) | 1.07 / 1.25 / 1.70 (1.33–7.55) | 1.07 / 1.25 / 3.58 (1.58–11.14) | 1.07 / 1.25 / 1.82 (1.33–7.62) | 94% | 25 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.12 (1.04–1.12) | 1.04 / 2.11 / 2.24 (2.24–3.23) | 1.90 / 2.03 / 6.91 (2.66–11.26) | 1.90 / 2.08 / 7.30 (5.06–9.73) | 97% | 3 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.07 (1.07–1.12) | 1.07 / 2.11 / 2.62 (2.27–4.74) | 2.98 / 3.49 / 11.01 (8.45–23.30) | 2.98 / 3.26 / 11.14 (8.32–15.10) | 91% | 16 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.15 / 2.30 / 2.91 (2.66–9.09) | 2.24 / 4.29 / 6.53 (5.12–11.39) | 5.89 / 6.27 / 14.08 (13.31–15.36) | 62% | 43 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.15 / 2.43 / 3.07 (2.94–9.86) | 3.20 / 4.48 / 10.37 (8.96–19.20) | 8.06 / 9.34 / 21.76 (19.97–32.51) | 48% | 122 |

Change latency (derive/delete), µs

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 0.12 / 0.17 / 0.17 (0.17–0.46) | 0.08 / 0.17 / 0.25 (0.17–0.62) | 0.08 / 0.17 / 0.21 (0.17–0.83) | 0.12 / 0.17 / 0.17 (0.17–0.83) | 96% | 0 |
| 1 | 0 | 50 | 2 | 0.58 / 0.79 / 1.20 (0.91–2260.99) | 0.21 / 0.29 / 0.37 (0.33–0.79) | 0.21 / 0.29 / 0.50 (0.37–1.07) | 0.21 / 0.33 / 0.50 (0.33–3.33) | 94% | 18 |
| 1 | 0 | 50 | 3 | 0.78 / 1.12 / 7.74 (6.27–24.58) | 0.74 / 1.25 / 6.27 (1.15–11.90) | 0.25 / 0.50 / 0.66 (0.42–5.82) | 0.25 / 0.37 / 0.50 (0.42–5.82) | 91% | 18 |
| 1 | 0 | 50 | 4 | 0.87 / 1.66 / 12.29 (4.00–1441.79) | 1.07 / 1.87 / 4.61 (2.66–5.89) | 0.29 / 0.62 / 0.91 (0.70–4.48) | 0.29 / 0.58 / 0.70 (0.50–7.74) | 84% | 31 |
| 1 | 0 | 50 | 6 | 1.46 / 3.10 / 958.46 (7.81–8650.75) | 1.66 / 3.23 / 8.58 (3.46–344.06) | 1.41 / 2.14 / 5.31 (2.91–7.04) | 0.37 / 0.70 / 1.00 (0.83–7.87) | 57% | 249 |
| 1 | 0 | 50 | 8 | 1.78 / 6.14 / 17.66 (9.86–491.52) | 1.90 / 4.06 / 58.37 (4.16–2785.28) | 1.62 / 3.14 / 16.13 (6.91–1114.11) | 0.66 / 1.25 / 2.37 (0.87–8.45) | 46% | 131 |
| 1 | 0 | 200 | 1 | 0.25 / 0.29 / 0.46 (0.29–10.62) | 0.25 / 0.29 / 0.42 (0.29–0.66) | 0.25 / 0.29 / 0.66 (0.33–3.90) | 0.25 / 0.37 / 0.66 (0.29–0.95) | 93% | 2 |
| 1 | 0 | 200 | 2 | 0.87 / 1.25 / 15.10 (7.49–1769.47) | 0.33 / 0.54 / 0.83 (0.54–6.21) | 0.33 / 0.62 / 1.00 (0.50–8.06) | 0.33 / 0.50 / 1.00 (0.83–1.15) | 90% | 11 |
| 1 | 0 | 200 | 3 | 1.07 / 4.74 / 22.27 (14.98–19136.51) | 1.04 / 1.82 / 16.64 (9.22–21.25) | 0.37 / 0.54 / 0.95 (0.58–4.86) | 0.37 / 0.70 / 3.52 (0.58–8.45) | 89% | 33 |
| 1 | 0 | 200 | 4 | 1.46 / 3.94 / 20.48 (11.39–385.02) | 1.41 / 2.53 / 8.06 (3.04–13.06) | 0.42 / 0.74 / 3.10 (0.87–5.12) | 0.42 / 0.74 / 6.59 (1.07–135.17) | 84% | 62 |
| 1 | 0 | 200 | 6 | 2.08 / 4.61 / 827.39 (227.33–13631.49) | 2.08 / 3.78 / 9.98 (5.63–8323.07) | 1.70 / 3.04 / 22.27 (9.86–10878.98) | 0.87 / 1.15 / 5.57 (4.42–32.26) | 59% | 339 |
| 1 | 0 | 200 | 8 | 2.00 / 7.04 / 60.42 (12.03–16384.00) | 2.43 / 4.74 / 53.25 (10.24–802.82) | 2.03 / 4.48 / 23.04 (8.32–1245.18) | 1.54 / 2.24 / 12.80 (10.75–50.69) | 48% | 484 |
| 1 | 0 | 1000 | 1 | 1.04 / 1.07 / 1.12 (1.07–4.74) | 1.04 / 1.07 / 1.12 (1.07–9.09) | 1.04 / 1.07 / 1.28 (1.12–5.82) | 1.04 / 1.07 / 1.12 (1.07–1.74) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 2.43 / 3.52 / 4.16 (3.62–16.00) | 1.12 / 1.25 / 1.46 (1.33–8.83) | 1.12 / 1.25 / 4.61 (1.33–10.50) | 1.12 / 1.25 / 1.41 (1.28–4.42) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.91 / 6.53 / 19.46 (13.18–34.82) | 2.72 / 4.29 / 12.03 (11.14–13.95) | 1.90 / 2.11 / 6.78 (2.11–9.86) | 1.95 / 2.27 / 7.74 (3.10–13.70) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.26 / 6.72 / 26.88 (17.15–696.32) | 3.39 / 6.34 / 32.77 (12.67–1212.42) | 3.04 / 3.62 / 15.49 (4.80–19.46) | 3.04 / 3.65 / 11.01 (3.23–17.66) | 92% | 16 |
| 1 | 0 | 1000 | 6 | 4.16 / 8.32 / 737.28 (12.29–1114.11) | 4.67 / 7.68 / 24.83 (18.94–311.30) | 7.23 / 10.75 / 90.11 (13.57–1212.42) | 5.76 / 6.34 / 14.85 (12.80–74.75) | 64% | 105 |
| 1 | 0 | 1000 | 8 | 4.29 / 9.34 / 135.17 (13.95–950.27) | 4.99 / 8.83 / 17.66 (14.59–348.16) | 10.11 / 22.02 / 589.82 (25.60–1048.58) | 8.06 / 12.16 / 24.83 (16.38–52.74) | 48% | 126 |
| 1 | 1000 | 50 | 1 | 0.08 / 0.17 / 0.17 (0.17–0.46) | 0.08 / 0.17 / 0.17 (0.17–0.54) | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.08 / 0.17 / 0.21 (0.17–0.58) | 99% | 0 |
| 1 | 1000 | 50 | 2 | 1.58 / 2.08 / 9.86 (8.83–2392.06) | 0.21 / 0.33 / 0.37 (0.33–4.61) | 0.21 / 0.29 / 0.37 (0.33–0.78) | 0.21 / 0.29 / 0.37 (0.33–0.62) | 97% | 4 |
| 1 | 1000 | 50 | 3 | 2.72 / 3.14 / 16.13 (7.42–36.86) | 1.74 / 2.69 / 13.31 (2.21–24.06) | 0.25 / 0.37 / 0.74 (0.46–10.50) | 0.25 / 0.37 / 0.46 (0.42–3.33) | 98% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 6.72 / 29.44 (23.81–59.90) | 1.87 / 3.04 / 10.75 (9.86–1048.58) | 0.25 / 0.62 / 0.83 (0.46–15.23) | 0.25 / 0.62 / 0.83 (0.74–8.13) | 87% | 19 |
| 1 | 1000 | 50 | 6 | 3.52 / 6.72 / 524.29 (43.52–778.24) | 3.49 / 4.86 / 9.60 (7.62–14.85) | 2.53 / 3.49 / 9.98 (6.59–11.14) | 0.37 / 0.70 / 1.07 (0.87–7.74) | 62% | 44 |
| 1 | 1000 | 50 | 8 | 3.65 / 7.42 / 131.07 (14.34–389.12) | 3.81 / 5.12 / 14.46 (7.94–307.20) | 2.69 / 4.03 / 10.88 (8.58–11.90) | 0.50 / 0.95 / 2.40 (2.03–24.32) | 47% | 66 |
| 1 | 1000 | 200 | 1 | 0.25 / 0.29 / 0.37 (0.29–0.79) | 0.25 / 0.29 / 0.37 (0.29–2.03) | 0.25 / 0.33 / 0.87 (0.29–5.12) | 0.25 / 0.29 / 0.37 (0.29–4.93) | 99% | 0 |
| 1 | 1000 | 200 | 2 | 1.87 / 2.21 / 9.98 (5.18–11.78) | 0.33 / 0.46 / 0.54 (0.50–8.83) | 0.33 / 0.50 / 0.95 (0.58–5.95) | 0.33 / 0.46 / 0.91 (0.50–4.29) | 99% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 5.18 / 19.97 (12.16–28.93) | 2.00 / 3.26 / 12.67 (7.74–26.88) | 0.37 / 0.50 / 2.98 (0.54–8.32) | 0.37 / 0.50 / 3.10 (0.62–19.20) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 3.14 / 7.17 / 28.42 (13.82–901.12) | 2.14 / 4.42 / 14.08 (9.22–27.90) | 0.37 / 0.62 / 3.14 (1.41–9.60) | 0.37 / 0.79 / 6.78 (0.95–9.09) | 89% | 11 |
| 1 | 1000 | 200 | 6 | 3.78 / 8.96 / 53.25 (14.08–827.39) | 3.74 / 4.86 / 17.66 (10.37–30.46) | 2.66 / 3.46 / 14.21 (11.01–3178.50) | 0.91 / 1.36 / 5.63 (2.14–8.83) | 61% | 81 |
| 1 | 1000 | 200 | 8 | 4.03 / 10.11 / 161.79 (16.90–4063.23) | 4.06 / 6.72 / 59.39 (10.11–1179.65) | 2.78 / 4.42 / 14.21 (11.14–106.50) | 1.49 / 2.50 / 16.38 (8.70–33.28) | 47% | 268 |
| 1 | 1000 | 1000 | 1 | 1.04 / 1.07 / 1.54 (1.15–13.44) | 1.04 / 1.07 / 1.28 (1.12–8.32) | 1.04 / 1.07 / 1.12 (1.07–10.24) | 1.04 / 1.07 / 4.86 (1.12–13.57) | 100% | 0 |
| 1 | 1000 | 1000 | 2 | 3.46 / 4.54 / 11.90 (4.54–20.74) | 1.12 / 1.28 / 4.22 (1.33–6.98) | 1.12 / 1.28 / 4.10 (1.36–4.54) | 1.12 / 1.28 / 4.35 (1.33–6.53) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 4.74 / 9.22 / 20.48 (19.46–31.23) | 3.62 / 5.70 / 23.55 (5.95–31.49) | 1.95 / 2.27 / 6.40 (2.11–11.39) | 1.95 / 2.11 / 7.17 (2.24–14.59) | 96% | 0 |
| 1 | 1000 | 1000 | 4 | 5.18 / 12.03 / 33.79 (26.88–3244.03) | 3.78 / 7.87 / 21.76 (13.06–13500.42) | 3.04 / 3.39 / 19.71 (14.46–40.45) | 3.04 / 3.49 / 12.54 (9.98–19.46) | 91% | 88 |
| 1 | 1000 | 1000 | 6 | 6.21 / 9.73 / 655.36 (13.06–31457.28) | 5.89 / 8.32 / 15.74 (14.46–2097.15) | 7.04 / 11.14 / 29.95 (17.66–21757.95) | 5.70 / 7.74 / 17.41 (6.40–942.08) | 63% | 404 |
| 1 | 1000 | 1000 | 8 | 6.40 / 11.26 / 35.33 (16.13–10223.62) | 6.21 / 10.62 / 200.70 (13.57–12713.98) | 9.73 / 16.90 / 61.95 (25.09–13762.56) | 8.00 / 15.23 / 64.00 (13.57–4194.30) | 49% | 557 |
| 10 | 0 | 50 | 1 | 0.12 / 0.17 / 0.21 (0.17–0.46) | 0.08 / 0.17 / 0.25 (0.17–0.83) | 0.08 / 0.17 / 0.21 (0.17–0.95) | 0.12 / 0.17 / 0.17 (0.17–0.62) | 95% | 0 |
| 10 | 0 | 50 | 2 | 0.58 / 0.79 / 5.50 (0.87–2588.67) | 0.21 / 0.29 / 0.42 (0.37–0.78) | 0.21 / 0.29 / 0.37 (0.37–0.70) | 0.21 / 0.33 / 0.42 (0.37–4.54) | 91% | 36 |
| 10 | 0 | 50 | 3 | 0.74 / 1.25 / 11.78 (4.22–12.67) | 0.74 / 1.12 / 6.34 (4.86–7.49) | 0.21 / 0.37 / 0.50 (0.42–0.70) | 0.21 / 0.37 / 0.54 (0.46–6.78) | 97% | 3 |
| 10 | 0 | 50 | 4 | 0.87 / 1.62 / 15.62 (10.37–348.16) | 0.87 / 1.54 / 3.23 (1.82–7.49) | 0.25 / 0.42 / 0.62 (0.54–1.04) | 0.25 / 0.46 / 0.70 (0.70–7.17) | 94% | 30 |
| 10 | 0 | 50 | 6 | 1.46 / 3.14 / 11.26 (8.96–22.78) | 1.74 / 3.65 / 5.25 (4.54–15.10) | 1.46 / 2.78 / 5.25 (3.87–13.18) | 0.33 / 0.70 / 1.00 (0.83–1.04) | 64% | 51 |
| 10 | 0 | 50 | 8 | 1.74 / 4.99 / 16.26 (13.31–18.94) | 2.11 / 4.86 / 11.90 (6.53–23.81) | 1.70 / 3.87 / 9.47 (6.59–16.13) | 0.54 / 1.12 / 4.54 (2.37–8.83) | 49% | 114 |
| 10 | 0 | 200 | 1 | 0.25 / 0.29 / 0.29 (0.29–0.66) | 0.25 / 0.29 / 0.29 (0.29–0.33) | 0.25 / 0.29 / 0.33 (0.29–0.33) | 0.25 / 0.29 / 0.33 (0.29–0.37) | 99% | 0 |
| 10 | 0 | 200 | 2 | 0.83 / 1.15 / 6.08 (1.25–50.18) | 0.33 / 0.50 / 0.58 (0.54–0.83) | 0.33 / 0.46 / 0.54 (0.54–0.83) | 0.33 / 0.46 / 0.54 (0.50–0.58) | 96% | 9 |
| 10 | 0 | 200 | 3 | 1.07 / 1.58 / 10.50 (5.76–190.46) | 1.04 / 1.46 / 6.85 (4.61–835.58) | 0.33 / 0.50 / 0.58 (0.58–0.87) | 0.33 / 0.50 / 0.62 (0.58–5.38) | 90% | 57 |
| 10 | 0 | 200 | 4 | 1.20 / 7.68 / 16.26 (7.81–8192.00) | 1.15 / 2.14 / 8.06 (2.62–43.52) | 0.37 / 0.54 / 0.95 (0.83–9.09) | 0.37 / 0.54 / 1.46 (0.62–8.70) | 82% | 118 |
| 10 | 0 | 200 | 6 | 1.87 / 4.61 / 11.65 (11.26–4063.23) | 2.08 / 4.29 / 11.78 (6.40–9437.18) | 1.82 / 3.55 / 9.09 (5.89–704.51) | 0.95 / 1.49 / 4.86 (4.35–28.67) | 61% | 241 |
| 10 | 0 | 200 | 8 | 2.11 / 20.99 / 225.28 (13.31–38797.31) | 2.50 / 5.95 / 27.14 (7.55–3538.94) | 2.24 / 4.86 / 14.08 (10.37–679.94) | 1.70 / 2.37 / 11.52 (9.34–58.88) | 47% | 944 |
| 10 | 0 | 1000 | 1 | 1.04 / 1.07 / 1.15 (1.07–7.10) | 1.04 / 1.07 / 1.12 (1.12–1.36) | 1.04 / 1.07 / 1.15 (1.12–1.20) | 1.04 / 1.07 / 1.12 (1.07–1.28) | 98% | 0 |
| 10 | 0 | 1000 | 2 | 2.40 / 3.52 / 8.00 (5.12–9.73) | 1.12 / 1.28 / 1.41 (1.36–1.78) | 1.12 / 1.28 / 1.58 (1.46–6.66) | 1.12 / 1.28 / 2.11 (1.41–6.78) | 98% | 0 |
| 10 | 0 | 1000 | 3 | 2.72 / 5.25 / 11.90 (10.75–22.53) | 2.72 / 4.29 / 11.78 (9.47–22.02) | 1.95 / 2.21 / 7.74 (6.72–9.73) | 1.95 / 2.11 / 8.58 (5.12–15.62) | 99% | 0 |
| 10 | 0 | 1000 | 4 | 3.39 / 6.27 / 16.38 (12.42–28.42) | 3.46 / 6.66 / 16.38 (11.39–22.53) | 3.07 / 3.62 / 16.00 (12.03–26.37) | 3.04 / 3.65 / 14.21 (9.34–16.00) | 90% | 8 |
| 10 | 0 | 1000 | 6 | 4.22 / 9.22 / 56.32 (14.46–184.32) | 4.99 / 9.73 / 16.38 (14.72–107.52) | 6.98 / 11.78 / 19.97 (16.00–72.70) | 5.76 / 6.46 / 16.90 (11.52–4259.84) | 60% | 243 |
| 10 | 0 | 1000 | 8 | 4.99 / 10.62 / 20.99 (18.43–4980.74) | 5.70 / 11.26 / 24.83 (18.18–8126.46) | 9.98 / 18.43 / 202.75 (25.60–6881.28) | 8.06 / 10.24 / 27.14 (21.25–101.38) | 47% | 779 |
| 10 | 1000 | 50 | 1 | 0.12 / 0.17 / 0.21 (0.17–0.58) | 0.08 / 0.17 / 0.21 (0.17–0.37) | 0.08 / 0.17 / 0.21 (0.17–0.79) | 0.12 / 0.17 / 0.21 (0.17–0.62) | 91% | 1 |
| 10 | 1000 | 50 | 2 | 1.57 / 1.79 / 7.04 (4.93–14.72) | 0.21 / 0.33 / 0.37 (0.37–0.79) | 0.21 / 0.33 / 0.62 (0.37–0.87) | 0.21 / 0.33 / 0.42 (0.37–0.87) | 94% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.33 / 20.99 (10.75–2719.74) | 1.74 / 2.30 / 13.06 (5.12–2883.58) | 0.21 / 0.37 / 0.70 (0.42–3.10) | 0.21 / 0.37 / 0.42 (0.42–0.78) | 91% | 87 |
| 10 | 1000 | 50 | 4 | 2.85 / 6.78 / 23.04 (9.22–2195.46) | 1.90 / 3.36 / 8.06 (5.70–15.23) | 0.25 / 0.50 / 1.20 (0.58–6.34) | 0.25 / 0.42 / 1.00 (0.62–6.85) | 88% | 42 |
| 10 | 1000 | 50 | 6 | 3.46 / 6.08 / 16.26 (12.03–157.70) | 2.91 / 4.67 / 9.09 (6.66–153.60) | 2.30 / 3.87 / 8.96 (6.27–31.23) | 0.37 / 0.70 / 1.87 (0.87–7.49) | 61% | 104 |
| 10 | 1000 | 50 | 8 | 3.65 / 7.62 / 26.11 (14.46–2523.14) | 3.52 / 5.70 / 10.50 (8.70–12.03) | 2.40 / 4.67 / 7.74 (6.27–20.22) | 0.50 / 0.91 / 5.95 (1.87–8.83) | 48% | 431 |
| 10 | 1000 | 200 | 1 | 0.25 / 0.29 / 0.33 (0.29–0.54) | 0.25 / 0.29 / 0.33 (0.29–0.50) | 0.25 / 0.29 / 0.29 (0.29–0.37) | 0.25 / 0.29 / 0.33 (0.29–0.33) | 99% | 0 |
| 10 | 1000 | 200 | 2 | 1.82 / 2.14 / 6.53 (2.24–11.14) | 0.33 / 0.46 / 0.58 (0.54–1.07) | 0.33 / 0.46 / 0.58 (0.54–0.83) | 0.33 / 0.50 / 0.62 (0.54–5.57) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.94 / 3.55 / 15.23 (11.52–18.43) | 2.00 / 2.40 / 6.34 (5.57–16.90) | 0.33 / 0.50 / 0.58 (0.58–4.74) | 0.33 / 0.50 / 0.74 (0.58–2.14) | 96% | 0 |
| 10 | 1000 | 200 | 4 | 3.10 / 4.80 / 19.71 (12.16–26.62) | 2.11 / 3.10 / 8.70 (7.81–10.50) | 0.37 / 0.54 / 1.87 (0.62–6.98) | 0.37 / 0.54 / 1.41 (0.79–6.85) | 93% | 4 |
| 10 | 1000 | 200 | 6 | 3.78 / 7.10 / 16.38 (12.80–141.31) | 3.26 / 5.44 / 9.47 (7.55–11.90) | 2.56 / 4.16 / 9.22 (7.23–30.46) | 0.87 / 1.25 / 8.19 (5.44–11.78) | 61% | 58 |
| 10 | 1000 | 200 | 8 | 4.03 / 10.37 / 39.94 (13.57–450.56) | 3.90 / 6.66 / 13.18 (10.24–23.81) | 2.78 / 5.70 / 11.14 (10.62–79.87) | 1.57 / 2.08 / 10.37 (8.06–20.22) | 47% | 81 |
| 10 | 1000 | 1000 | 1 | 1.04 / 1.07 / 1.15 (1.12–11.52) | 1.04 / 1.07 / 1.12 (1.07–2.78) | 1.04 / 1.07 / 1.12 (1.07–5.44) | 1.04 / 1.07 / 1.70 (1.07–5.25) | 90% | 8 |
| 10 | 1000 | 1000 | 2 | 3.46 / 4.48 / 10.50 (4.80–2457.60) | 1.12 / 1.28 / 2.08 (1.36–11.52) | 1.12 / 1.28 / 5.12 (1.90–12.67) | 1.12 / 1.28 / 1.58 (1.36–9.98) | 94% | 25 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.66 / 14.08 (10.24–32.51) | 3.65 / 5.95 / 12.29 (7.94–23.55) | 1.95 / 2.11 / 6.98 (2.27–10.24) | 1.95 / 2.14 / 6.02 (5.31–11.14) | 97% | 3 |
| 10 | 1000 | 1000 | 4 | 4.99 / 8.06 / 25.86 (15.49–33.28) | 3.81 / 7.10 / 13.31 (8.13–16.38) | 3.07 / 3.58 / 10.62 (8.45–22.27) | 3.07 / 3.36 / 11.26 (8.13–16.00) | 91% | 16 |
| 10 | 1000 | 1000 | 6 | 6.08 / 10.11 / 17.66 (14.85–172.03) | 5.76 / 10.50 / 16.38 (15.36–27.39) | 7.10 / 12.29 / 18.43 (15.87–23.81) | 5.95 / 6.46 / 14.59 (12.80–16.13) | 62% | 43 |
| 10 | 1000 | 1000 | 8 | 6.59 / 11.65 / 21.25 (17.66–45.57) | 6.34 / 12.80 / 25.60 (16.38–278.53) | 9.86 / 18.69 / 50.69 (24.83–290.82) | 8.13 / 9.34 / 25.34 (18.18–29.18) | 48% | 122 |

Migrations seen: 267449 over 403979432 samples (one sample per measured operation).


### Suite a2: IPC call and reply, k/2 pairs of one process

load average before: { 8.23 6.12 5.36 }
load average after: { 12.41 8.07 6.18 }; 91 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 316k (164k–322k) | 308k (297k–314k) | 293k (286k–314k) | 308k (230k–314k) | 84% | 39 |
| 0 | 50 | 4 | 617k (421k–625k) | 583k (292k–624k) | 515k (355k–539k) | 504k (379k–524k) | 59% | 109 |
| 0 | 50 | 6 | 911k (879k–929k) | 778k (282k–887k) | 715k (475k–833k) | 778k (253k–783k) | 48% | 280 |
| 0 | 50 | 8 | 804k (438k–1.23M) | 903k (640k–1.19M) | 854k (405k–1.05M) | 857k (564k–989k) | 42% | 586 |
| 0 | 200 | 2 | 273k (141k–283k) | 269k (265k–281k) | 272k (262k–281k) | 276k (237k–281k) | 80% | 37 |
| 0 | 200 | 4 | 558k (227k–561k) | 540k (337k–547k) | 457k (367k–480k) | 472k (222k–476k) | 68% | 132 |
| 0 | 200 | 6 | 734k (298k–843k) | 760k (383k–799k) | 722k (558k–757k) | 678k (362k–691k) | 51% | 276 |
| 0 | 200 | 8 | 1.09M (823k–1.12M) | 1.05M (300k–1.08M) | 696k (360k–932k) | 769k (207k–792k) | 42% | 453 |
| 0 | 1000 | 2 | 198k (195k–198k) | 195k (180k–196k) | 195k (156k–196k) | 194k (190k–196k) | 90% | 10 |
| 0 | 1000 | 4 | 395k (393k–399k) | 382k (193k–385k) | 350k (177k–361k) | 283k (255k–369k) | 72% | 130 |
| 0 | 1000 | 6 | 475k (371k–594k) | 560k (370k–582k) | 502k (446k–532k) | 352k (207k–357k) | 53% | 278 |
| 0 | 1000 | 8 | 763k (467k–807k) | 752k (378k–758k) | 612k (309k–670k) | 291k (230k–359k) | 43% | 542 |
| 1000 | 50 | 2 | 180k (173k–195k) | 187k (104k–191k) | 181k (135k–190k) | 179k (108k–189k) | 74% | 82 |
| 1000 | 50 | 4 | 380k (194k–385k) | 363k (213k–372k) | 288k (265k–359k) | 352k (278k–357k) | 61% | 154 |
| 1000 | 50 | 6 | 563k (376k–578k) | 550k (475k–570k) | 532k (497k–540k) | 477k (173k–506k) | 56% | 219 |
| 1000 | 50 | 8 | 689k (297k–738k) | 713k (382k–737k) | 701k (240k–716k) | 688k (384k–700k) | 42% | 403 |
| 1000 | 200 | 2 | 178k (125k–181k) | 172k (168k–177k) | 175k (170k–177k) | 174k (127k–177k) | 82% | 32 |
| 1000 | 200 | 4 | 351k (170k–357k) | 344k (249k–353k) | 328k (145k–345k) | 327k (322k–332k) | 68% | 133 |
| 1000 | 200 | 6 | 523k (487k–537k) | 517k (507k–519k) | 502k (498k–504k) | 491k (480k–492k) | 56% | 62 |
| 1000 | 200 | 8 | 712k (239k–719k) | 694k (263k–699k) | 484k (243k–666k) | 493k (283k–638k) | 43% | 436 |
| 1000 | 1000 | 2 | 141k (125k–142k) | 126k (116k–141k) | 137k (134k–141k) | 140k (112k–140k) | 85% | 38 |
| 1000 | 1000 | 4 | 280k (209k–285k) | 270k (154k–275k) | 263k (195k–267k) | 262k (245k–272k) | 65% | 101 |
| 1000 | 1000 | 6 | 353k (223k–421k) | 390k (230k–403k) | 394k (298k–401k) | 352k (128k–357k) | 52% | 264 |
| 1000 | 1000 | 8 | 558k (244k–565k) | 374k (234k–549k) | 203k (131k–527k) | 247k (113k–354k) | 41% | 753 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 2.08 / 4.22 (2.14–360.45) | 1.82 / 2.24 / 3.52 (2.94–9.47) | 1.82 / 2.30 / 7.36 (2.69–10.37) | 1.82 / 2.27 / 6.08 (2.40–17.92) | 84% | 39 |
| 0 | 50 | 4 | 1.82 / 2.14 / 7.62 (5.95–12.93) | 1.87 / 2.40 / 13.57 (2.94–52.74) | 2.50 / 3.07 / 13.31 (3.94–14.72) | 2.50 / 3.10 / 9.73 (3.23–38.91) | 59% | 109 |
| 0 | 50 | 6 | 1.87 / 2.14 / 7.49 (3.90–13.70) | 2.11 / 2.40 / 13.31 (7.23–139.26) | 2.24 / 2.98 / 13.70 (3.87–25.60) | 2.43 / 3.14 / 13.31 (7.23–655.36) | 48% | 280 |
| 0 | 50 | 8 | 1.82 / 2.14 / 20.48 (2.50–206.85) | 2.14 / 2.40 / 9.73 (5.44–25.60) | 2.43 / 3.07 / 20.22 (12.29–235.52) | 2.69 / 3.36 / 19.46 (12.03–125.95) | 42% | 586 |
| 0 | 200 | 2 | 2.14 / 2.43 / 10.37 (2.56–892.93) | 2.27 / 2.66 / 8.83 (2.69–12.29) | 2.27 / 2.66 / 7.87 (3.74–12.03) | 2.24 / 2.53 / 7.30 (2.43–11.65) | 80% | 37 |
| 0 | 200 | 4 | 2.14 / 2.50 / 9.47 (3.58–120.83) | 2.27 / 2.66 / 9.47 (3.20–25.86) | 2.78 / 3.55 / 9.34 (4.10–111.62) | 2.78 / 3.62 / 9.22 (6.27–491.52) | 68% | 132 |
| 0 | 200 | 6 | 2.21 / 2.53 / 15.36 (4.03–247.81) | 2.43 / 2.69 / 9.34 (2.98–131.07) | 2.66 / 3.55 / 9.09 (3.74–72.70) | 3.07 / 3.87 / 10.37 (4.16–121.86) | 51% | 276 |
| 0 | 200 | 8 | 2.24 / 2.56 / 10.37 (3.52–48.13) | 2.37 / 2.72 / 9.73 (6.91–2719.74) | 2.82 / 3.58 / 17.15 (11.01–1359.87) | 3.49 / 4.22 / 37.89 (25.09–4653.06) | 42% | 453 |
| 0 | 1000 | 2 | 3.65 / 4.00 / 12.16 (5.57–16.00) | 3.68 / 4.22 / 12.03 (7.62–17.41) | 3.68 / 4.22 / 17.92 (11.14–24.32) | 3.74 / 4.16 / 11.52 (4.35–23.30) | 90% | 10 |
| 0 | 1000 | 4 | 3.65 / 4.03 / 10.24 (6.78–11.01) | 3.78 / 4.22 / 10.62 (9.22–1359.87) | 4.16 / 5.70 / 13.06 (11.26–61.95) | 4.16 / 5.70 / 13.44 (12.54–19.97) | 72% | 130 |
| 0 | 1000 | 6 | 3.65 / 4.03 / 27.65 (9.86–151.55) | 3.90 / 4.29 / 12.54 (6.21–16.90) | 4.16 / 5.82 / 15.36 (9.34–50.18) | 7.04 / 7.87 / 18.69 (15.74–1900.54) | 53% | 278 |
| 0 | 1000 | 8 | 3.68 / 4.06 / 11.14 (9.47–249.86) | 3.94 / 4.22 / 20.22 (11.90–210.94) | 4.29 / 5.89 / 31.49 (25.34–1048.58) | 9.60 / 11.39 / 917.50 (25.34–2883.58) | 43% | 542 |
| 1000 | 50 | 2 | 3.78 / 4.10 / 12.03 (8.70–44.03) | 3.90 / 4.42 / 12.03 (9.22–720.90) | 3.90 / 4.48 / 12.67 (5.82–48.13) | 3.90 / 4.42 / 14.46 (8.70–520.19) | 74% | 82 |
| 1000 | 50 | 4 | 3.81 / 4.16 / 8.96 (5.38–99.33) | 4.00 / 4.48 / 25.34 (6.02–163.84) | 4.29 / 4.74 / 14.72 (7.04–182.27) | 4.29 / 4.67 / 12.29 (7.55–96.26) | 61% | 154 |
| 1000 | 50 | 6 | 3.87 / 4.16 / 14.85 (9.09–41.98) | 4.10 / 4.48 / 10.88 (9.47–44.03) | 4.22 / 4.67 / 12.16 (5.25–32.51) | 4.35 / 4.74 / 14.98 (5.95–1703.94) | 56% | 219 |
| 1000 | 50 | 8 | 3.87 / 4.22 / 22.53 (13.70–2260.99) | 4.06 / 4.48 / 22.78 (9.86–270.34) | 4.22 / 4.74 / 23.04 (12.54–819.20) | 4.35 / 4.86 / 33.79 (13.44–311.30) | 42% | 403 |
| 1000 | 200 | 2 | 4.16 / 4.48 / 13.70 (7.81–88.06) | 4.29 / 4.80 / 14.98 (9.73–22.27) | 4.29 / 4.80 / 13.57 (5.18–23.81) | 4.29 / 4.48 / 14.72 (11.65–274.43) | 82% | 32 |
| 1000 | 200 | 4 | 4.16 / 4.61 / 13.06 (11.01–247.81) | 4.35 / 4.80 / 13.44 (10.75–48.64) | 4.67 / 4.99 / 13.57 (11.01–1343.49) | 4.67 / 5.06 / 18.69 (13.18–29.70) | 68% | 133 |
| 1000 | 200 | 6 | 4.22 / 4.54 / 12.42 (10.37–32.26) | 4.48 / 4.80 / 12.80 (9.34–17.66) | 4.61 / 4.99 / 12.80 (11.52–16.38) | 4.74 / 5.12 / 11.52 (6.91–14.98) | 56% | 62 |
| 1000 | 200 | 8 | 4.16 / 4.54 / 16.64 (11.65–2850.82) | 4.42 / 4.80 / 23.81 (11.01–3112.96) | 4.67 / 5.06 / 28.16 (11.52–450.56) | 4.80 / 5.31 / 94.21 (14.98–389.12) | 43% | 436 |
| 1000 | 1000 | 2 | 5.63 / 5.95 / 14.21 (7.62–25.34) | 5.82 / 6.40 / 32.26 (9.09–57.34) | 5.82 / 6.27 / 14.46 (9.22–22.02) | 5.70 / 6.27 / 14.46 (10.88–62.98) | 85% | 38 |
| 1000 | 1000 | 4 | 5.63 / 6.14 / 16.13 (10.88–188.42) | 5.89 / 6.40 / 17.41 (11.39–43.01) | 6.21 / 6.59 / 13.57 (13.06–133.12) | 6.21 / 6.59 / 17.15 (11.26–56.83) | 65% | 101 |
| 1000 | 1000 | 6 | 5.70 / 6.08 / 35.33 (15.36–1392.64) | 6.02 / 6.40 / 18.69 (12.93–671.74) | 6.21 / 6.66 / 18.43 (13.82–107.52) | 7.04 / 7.62 / 16.00 (14.34–1835.01) | 52% | 264 |
| 1000 | 1000 | 8 | 5.70 / 6.08 / 21.76 (13.44–1736.70) | 5.95 / 6.40 / 274.43 (14.08–4096.00) | 6.14 / 6.59 / 4325.38 (16.38–7077.89) | 9.47 / 10.62 / 1638.40 (23.30–5832.70) | 41% | 753 |

Migrations seen: 41746 over 31666729 samples (one sample per measured operation).


### Suite a3: k-1 threads signal one notify

load average before: { 12.41 8.07 6.18 }
load average after: { 8.21 7.81 6.26 }; 88 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 705k (629k–712k) | 663k (598k–666k) | 666k (601k–667k) | 661k (633k–665k) | 87% | 8 |
| 0 | 50 | 4 | 2.04M (2.02M–2.10M) | 1.86M (1.71M–1.89M) | 1.92M (1.77M–1.97M) | 1.89M (1.77M–1.96M) | 88% | 6 |
| 0 | 50 | 6 | 3.37M (2.34M–3.41M) | 2.71M (2.68M–2.75M) | 2.34M (2.27M–2.36M) | 2.45M (2.31M–2.48M) | 62% | 109 |
| 0 | 50 | 8 | 4.72M (4.71M–4.75M) | 3.55M (3.45M–3.60M) | 2.72M (2.64M–2.74M) | 2.65M (2.59M–2.75M) | 47% | 28 |
| 0 | 200 | 2 | 657k (651k–659k) | 575k (572k–578k) | 576k (563k–578k) | 576k (574k–580k) | 99% | 0 |
| 0 | 200 | 4 | 1.95M (1.92M–1.95M) | 1.66M (1.64M–1.68M) | 1.58M (1.28M–1.61M) | 1.59M (1.32M–1.61M) | 95% | 3 |
| 0 | 200 | 6 | 3.18M (2.53M–3.20M) | 2.32M (1.87M–2.36M) | 1.73M (699k–1.74M) | 1.59M (1.54M–1.63M) | 63% | 181 |
| 0 | 200 | 8 | 4.46M (4.41M–4.48M) | 2.67M (1.67M–2.76M) | 1.99M (384k–2.22M) | 1.50M (51k–1.70M) | 48% | 709 |
| 0 | 1000 | 2 | 525k (518k–527k) | 401k (391k–402k) | 401k (372k–403k) | 400k (394k–401k) | 95% | 6 |
| 0 | 1000 | 4 | 1.56M (1.55M–1.57M) | 1.06M (841k–1.08M) | 618k (42k–673k) | 670k (410k–674k) | 85% | 155 |
| 0 | 1000 | 6 | 1.87M (1.21M–1.89M) | 1.22M (521k–1.28M) | 976k (879k–1.04M) | 643k (383k–670k) | 58% | 502 |
| 0 | 1000 | 8 | 1.86M (1.12M–1.88M) | 1.21M (355k–1.27M) | 1.09M (439k–1.19M) | 671k (429k–696k) | 48% | 655 |
| 1000 | 50 | 2 | 709k (686k–709k) | 663k (633k–667k) | 665k (652k–665k) | 663k (626k–667k) | 94% | 0 |
| 1000 | 50 | 4 | 2.09M (977k–2.11M) | 1.44M (1.17M–1.45M) | 1.94M (1.63M–1.97M) | 1.95M (1.26M–1.98M) | 87% | 62 |
| 1000 | 50 | 6 | 3.40M (2.96M–3.42M) | 2.54M (2.19M–2.55M) | 1.73M (1.70M–1.75M) | 2.84M (2.80M–2.89M) | 62% | 59 |
| 1000 | 50 | 8 | 4.72M (4.34M–4.76M) | 3.36M (2.43M–3.42M) | 2.65M (2.08M–2.73M) | 2.82M (93k–3.07M) | 48% | 298 |
| 1000 | 200 | 2 | 660k (652k–661k) | 574k (568k–578k) | 575k (570k–576k) | 575k (571k–578k) | 100% | 0 |
| 1000 | 200 | 4 | 1.95M (1.92M–1.95M) | 1.37M (1.31M–1.37M) | 1.64M (1.59M–1.65M) | 1.64M (1.62M–1.65M) | 97% | 1 |
| 1000 | 200 | 6 | 3.20M (3.18M–3.20M) | 2.25M (2.18M–2.27M) | 1.58M (1.34M–1.60M) | 1.68M (1.46M–1.74M) | 64% | 61 |
| 1000 | 200 | 8 | 4.45M (3.77M–4.47M) | 2.71M (1.89M–2.93M) | 1.89M (796k–2.18M) | 1.71M (753k–1.78M) | 47% | 679 |
| 1000 | 1000 | 2 | 526k (518k–528k) | 379k (371k–400k) | 383k (286k–388k) | 381k (281k–387k) | 95% | 17 |
| 1000 | 1000 | 4 | 1.57M (1.52M–1.57M) | 1.06M (998k–1.09M) | 722k (668k–725k) | 721k (670k–723k) | 90% | 12 |
| 1000 | 1000 | 6 | 1.87M (1.84M–1.89M) | 1.26M (1.15M–1.31M) | 1.00M (811k–1.02M) | 662k (137k–685k) | 62% | 192 |
| 1000 | 1000 | 8 | 1.87M (1.01M–1.88M) | 1.25M (620k–1.28M) | 1.16M (265k–1.21M) | 687k (277k–711k) | 47% | 398 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.29) | 0.17 / 0.25 / 0.29 (0.29–0.54) | 0.17 / 0.25 / 0.29 (0.29–0.50) | 0.17 / 0.25 / 0.37 (0.29–0.74) | 87% | 8 |
| 0 | 50 | 4 | 0.08 / 0.25 / 0.29 (0.21–0.50) | 0.21 / 0.46 / 0.62 (0.46–0.66) | 0.17 / 0.50 / 0.78 (0.50–7.10) | 0.17 / 0.54 / 0.74 (0.46–8.70) | 88% | 6 |
| 0 | 50 | 6 | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.29 / 0.58 / 0.70 (0.66–0.79) | 0.42 / 0.95 / 1.15 (1.15–1.25) | 0.66 / 1.12 / 5.63 (1.41–6.53) | 62% | 109 |
| 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.29 / 0.58 / 57.86 (53.76–58.37) | 0.42 / 1.12 / 1.46 (1.46–3.94) | 1.28 / 2.11 / 8.70 (7.17–20.74) | 47% | 28 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.42) | 0.42 / 0.50 / 0.54 (0.50–0.66) | 0.42 / 0.50 / 0.70 (0.50–4.93) | 0.42 / 0.46 / 0.50 (0.50–0.58) | 99% | 0 |
| 0 | 200 | 4 | 0.21 / 0.29 / 0.37 (0.29–0.42) | 0.33 / 0.62 / 0.74 (0.66–0.87) | 0.50 / 1.00 / 5.44 (1.04–10.88) | 0.50 / 0.87 / 4.06 (1.49–5.18) | 95% | 3 |
| 0 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.46 / 0.87 / 12.03 (2.53–12.42) | 0.50 / 1.70 / 2.14 (2.00–27.14) | 1.74 / 2.66 / 8.06 (6.78–15.74) | 63% | 181 |
| 0 | 200 | 8 | 0.25 / 0.42 / 0.46 (0.46–0.46) | 0.50 / 1.25 / 7.55 (5.50–16.00) | 0.58 / 1.41 / 5.18 (1.74–3407.87) | 2.78 / 4.29 / 13.57 (12.03–17301.50) | 48% | 709 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.70) | 1.15 / 1.25 / 1.28 (1.28–1.46) | 1.15 / 1.25 / 1.33 (1.28–6.78) | 1.15 / 1.25 / 1.33 (1.25–4.74) | 95% | 6 |
| 0 | 1000 | 4 | 0.58 / 0.66 / 0.74 (0.66–0.83) | 0.66 / 1.74 / 2.21 (1.79–7.23) | 3.26 / 4.10 / 24.83 (9.73–15073.28) | 3.20 / 3.74 / 12.93 (4.48–204.80) | 85% | 155 |
| 0 | 1000 | 6 | 0.62 / 0.78 / 143.36 (139.26–149.50) | 1.87 / 2.69 / 8.06 (7.10–1736.70) | 0.87 / 3.81 / 4.61 (4.42–12.16) | 5.89 / 7.17 / 16.90 (15.36–1490.94) | 58% | 502 |
| 0 | 1000 | 8 | 0.62 / 0.74 / 339.97 (274.43–344.06) | 1.70 / 2.62 / 15.23 (8.58–6619.14) | 2.03 / 3.23 / 11.39 (9.34–3112.96) | 8.58 / 10.37 / 20.48 (19.97–1622.02) | 48% | 655 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.25) | 0.17 / 0.25 / 0.29 (0.29–0.54) | 0.17 / 0.25 / 0.29 (0.29–0.46) | 0.17 / 0.25 / 0.29 (0.29–0.54) | 94% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.25 (0.21–0.33) | 0.21 / 0.37 / 0.62 (0.42–0.66) | 0.17 / 0.46 / 0.83 (0.74–2.30) | 0.17 / 0.62 / 0.79 (0.46–0.91) | 87% | 62 |
| 1000 | 50 | 6 | 0.12 / 0.25 / 0.29 (0.29–0.33) | 0.29 / 0.54 / 0.66 (0.66–0.70) | 0.33 / 0.74 / 1.04 (1.00–1.04) | 0.42 / 0.83 / 1.07 (1.04–1.33) | 62% | 59 |
| 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.42) | 0.33 / 0.58 / 60.93 (52.22–72.70) | 0.42 / 0.83 / 1.28 (1.20–1.46) | 1.04 / 1.95 / 6.98 (2.78–12451.84) | 48% | 298 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.42 / 0.50 / 0.54 (0.54–0.58) | 0.42 / 0.50 / 0.54 (0.54–0.58) | 0.42 / 0.50 / 0.54 (0.50–0.58) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.29 / 0.37 (0.25–0.42) | 0.29 / 0.58 / 0.66 (0.66–0.91) | 0.42 / 0.70 / 1.04 (1.04–3.90) | 0.42 / 0.70 / 1.15 (0.74–7.94) | 97% | 1 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.42 / 0.87 / 12.03 (8.83–19.97) | 0.46 / 1.46 / 2.00 (1.87–5.12) | 1.49 / 2.21 / 10.75 (6.46–14.34) | 64% | 61 |
| 1000 | 200 | 8 | 0.25 / 0.42 / 0.46 (0.46–0.50) | 0.46 / 0.78 / 20.48 (11.52–107.52) | 0.58 / 1.20 / 5.38 (2.00–59.39) | 2.56 / 3.58 / 12.16 (9.86–516.10) | 47% | 679 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.66 (0.62–0.74) | 1.15 / 1.62 / 1.74 (1.62–9.47) | 1.15 / 1.62 / 1.66 (1.66–14.34) | 1.15 / 1.62 / 1.70 (1.66–24.58) | 95% | 17 |
| 1000 | 1000 | 4 | 0.58 / 0.62 / 0.74 (0.70–0.83) | 0.66 / 1.70 / 1.87 (1.82–2.27) | 2.72 / 3.23 / 13.31 (8.32–16.90) | 2.72 / 3.46 / 13.31 (11.01–14.59) | 90% | 12 |
| 1000 | 1000 | 6 | 0.66 / 0.79 / 141.31 (141.31–151.55) | 1.74 / 2.78 / 7.10 (5.70–10.62) | 0.87 / 3.52 / 4.61 (4.35–9.09) | 5.89 / 6.78 / 17.92 (14.08–9699.33) | 62% | 192 |
| 1000 | 1000 | 8 | 0.62 / 0.78 / 339.97 (262.14–352.26) | 1.90 / 2.78 / 10.75 (8.06–21.76) | 2.11 / 3.04 / 10.24 (8.06–6488.06) | 8.58 / 11.39 / 24.58 (19.46–1540.10) | 47% | 398 |

Migrations seen: 69016 over 116118247 samples (one sample per measured operation).


### Suite b: unmap seen by every core of a k-thread process

load average before: { 8.21 7.81 6.26 }
load average after: { 5.54 6.64 6.11 }; 217 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 481k (475k–498k) | 502k (487k–502k) | 504k (502k–505k) | 501k (499k–502k) | 99% | 0 |
| one | 0 | 50 | 3 | 455k (445k–483k) | 460k (456k–471k) | 486k (485k–488k) | 486k (481k–487k) | 99% | 0 |
| one | 0 | 50 | 4 | 443k (418k–462k) | 378k (369k–391k) | 473k (446k–477k) | 468k (419k–477k) | 97% | 5 |
| one | 0 | 50 | 6 | 370k (351k–379k) | 300k (295k–323k) | 314k (308k–321k) | 420k (406k–424k) | 65% | 13 |
| one | 0 | 50 | 8 | 352k (292k–353k) | 275k (268k–283k) | 252k (247k–257k) | 400k (376k–401k) | 51% | 19 |
| one | 0 | 200 | 2 | 440k (428k–442k) | 479k (477k–480k) | 478k (472k–479k) | 479k (477k–480k) | 100% | 0 |
| one | 0 | 200 | 3 | 417k (408k–432k) | 429k (397k–431k) | 463k (460k–464k) | 462k (459k–463k) | 100% | 0 |
| one | 0 | 200 | 4 | 404k (387k–408k) | 352k (322k–355k) | 448k (427k–451k) | 448k (431k–449k) | 99% | 0 |
| one | 0 | 200 | 6 | 345k (338k–350k) | 281k (275k–285k) | 293k (288k–300k) | 398k (395k–404k) | 65% | 8 |
| one | 0 | 200 | 8 | 319k (289k–328k) | 250k (243k–259k) | 236k (217k–238k) | 374k (364k–381k) | 48% | 14 |
| one | 0 | 1000 | 2 | 258k (251k–259k) | 343k (342k–345k) | 346k (343k–347k) | 345k (343k–347k) | 100% | 0 |
| one | 0 | 1000 | 3 | 251k (249k–255k) | 250k (243k–252k) | 337k (323k–339k) | 337k (336k–339k) | 100% | 1 |
| one | 0 | 1000 | 4 | 248k (246k–249k) | 225k (224k–228k) | 327k (322k–329k) | 328k (293k–330k) | 98% | 3 |
| one | 0 | 1000 | 6 | 215k (206k–224k) | 189k (183k–191k) | 200k (195k–203k) | 303k (299k–306k) | 65% | 1 |
| one | 0 | 1000 | 8 | 210k (203k–211k) | 176k (167k–180k) | 171k (165k–175k) | 287k (279k–291k) | 49% | 5 |
| one | 1000 | 50 | 2 | 322k (319k–332k) | 334k (334k–334k) | 334k (333k–335k) | 334k (333k–335k) | 100% | 0 |
| one | 1000 | 50 | 3 | 317k (315k–320k) | 321k (313k–325k) | 326k (325k–329k) | 324k (324k–328k) | 100% | 1 |
| one | 1000 | 50 | 4 | 308k (299k–313k) | 220k (216k–224k) | 320k (314k–322k) | 320k (318k–322k) | 99% | 0 |
| one | 1000 | 50 | 6 | 266k (263k–270k) | 191k (187k–192k) | 198k (197k–202k) | 290k (283k–293k) | 65% | 14 |
| one | 1000 | 50 | 8 | 254k (242k–258k) | 184k (179k–185k) | 168k (158k–175k) | 276k (225k–280k) | 49% | 16 |
| one | 1000 | 200 | 2 | 300k (298k–306k) | 322k (321k–324k) | 322k (320k–323k) | 322k (318k–324k) | 99% | 0 |
| one | 1000 | 200 | 3 | 292k (289k–301k) | 295k (259k–303k) | 315k (314k–316k) | 314k (313k–316k) | 100% | 4 |
| one | 1000 | 200 | 4 | 285k (279k–290k) | 209k (205k–210k) | 309k (308k–311k) | 309k (306k–309k) | 99% | 1 |
| one | 1000 | 200 | 6 | 250k (234k–255k) | 181k (170k–185k) | 187k (184k–192k) | 283k (278k–284k) | 68% | 16 |
| one | 1000 | 200 | 8 | 241k (233k–243k) | 175k (153k–176k) | 163k (159k–164k) | 273k (263k–275k) | 52% | 11 |
| one | 1000 | 1000 | 2 | 203k (200k–206k) | 257k (256k–257k) | 256k (253k–257k) | 256k (255k–257k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 199k (195k–203k) | 201k (200k–204k) | 253k (241k–254k) | 253k (247k–253k) | 100% | 0 |
| one | 1000 | 1000 | 4 | 196k (192k–199k) | 157k (155k–158k) | 246k (237k–249k) | 245k (192k–248k) | 98% | 3 |
| one | 1000 | 1000 | 6 | 173k (170k–176k) | 140k (134k–141k) | 144k (140k–146k) | 229k (225k–232k) | 68% | 6 |
| one | 1000 | 1000 | 8 | 173k (171k–174k) | 137k (134k–140k) | 121k (114k–129k) | 222k (174k–225k) | 48% | 19 |
| all | 0 | 50 | 2 | 869k (793k–875k) | 853k (847k–865k) | 853k (850k–857k) | 853k (829k–862k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.05M (1.00M–1.07M) | 926k (914k–951k) | 1.02M (991k–1.02M) | 1.02M (1.01M–1.02M) | 100% | 0 |
| all | 0 | 50 | 4 | 1.14M (1.12M–1.15M) | 861k (854k–863k) | 1.12M (1.11M–1.14M) | 1.14M (1.12M–1.20M) | 100% | 0 |
| all | 0 | 50 | 6 | 1.01M (1.01M–1.02M) | 693k (677k–719k) | 650k (646k–665k) | 1.11M (1.07M–1.13M) | 68% | 44 |
| all | 0 | 50 | 8 | 943k (927k–949k) | 674k (625k–698k) | 509k (492k–543k) | 1.09M (1.07M–1.10M) | 49% | 37 |
| all | 0 | 200 | 2 | 785k (694k–812k) | 765k (762k–768k) | 765k (758k–772k) | 763k (753k–766k) | 100% | 0 |
| all | 0 | 200 | 3 | 844k (835k–936k) | 811k (796k–823k) | 898k (884k–909k) | 893k (886k–898k) | 100% | 5 |
| all | 0 | 200 | 4 | 1.01M (990k–1.03M) | 745k (731k–767k) | 983k (936k–992k) | 984k (964k–987k) | 100% | 0 |
| all | 0 | 200 | 6 | 891k (885k–932k) | 628k (617k–667k) | 599k (588k–631k) | 983k (959k–1.00M) | 68% | 42 |
| all | 0 | 200 | 8 | 835k (802k–844k) | 608k (594k–630k) | 471k (456k–483k) | 968k (937k–980k) | 49% | 67 |
| all | 0 | 1000 | 2 | 402k (380k–442k) | 474k (457k–478k) | 474k (468k–475k) | 474k (473k–478k) | 100% | 0 |
| all | 0 | 1000 | 3 | 512k (458k–526k) | 485k (485k–488k) | 524k (519k–532k) | 522k (522k–529k) | 100% | 0 |
| all | 0 | 1000 | 4 | 554k (547k–567k) | 440k (422k–449k) | 546k (540k–553k) | 550k (546k–551k) | 100% | 15 |
| all | 0 | 1000 | 6 | 512k (512k–514k) | 406k (396k–411k) | 396k (387k–401k) | 524k (518k–533k) | 67% | 53 |
| all | 0 | 1000 | 8 | 504k (458k–508k) | 413k (396k–421k) | 338k (330k–364k) | 525k (512k–533k) | 50% | 67 |
| all | 1000 | 50 | 2 | 454k (442k–457k) | 782k (777k–794k) | 773k (763k–786k) | 798k (771k–808k) | 100% | 0 |
| all | 1000 | 50 | 3 | 604k (597k–631k) | 525k (517k–533k) | 1.01M (1.00M–1.02M) | 1.00M (943k–1.02M) | 100% | 0 |
| all | 1000 | 50 | 4 | 715k (694k–732k) | 571k (560k–574k) | 1.07M (975k–1.10M) | 1.09M (1.06M–1.10M) | 99% | 0 |
| all | 1000 | 50 | 6 | 726k (713k–750k) | 575k (564k–595k) | 553k (552k–562k) | 889k (876k–915k) | 67% | 106 |
| all | 1000 | 50 | 8 | 733k (708k–748k) | 596k (593k–618k) | 482k (474k–487k) | 925k (903k–934k) | 49% | 60 |
| all | 1000 | 200 | 2 | 419k (389k–421k) | 765k (754k–768k) | 767k (764k–768k) | 765k (764k–767k) | 100% | 1 |
| all | 1000 | 200 | 3 | 564k (556k–578k) | 518k (509k–526k) | 889k (873k–892k) | 887k (870k–895k) | 100% | 0 |
| all | 1000 | 200 | 4 | 644k (642k–656k) | 549k (545k–552k) | 978k (960k–985k) | 973k (972k–978k) | 100% | 0 |
| all | 1000 | 200 | 6 | 660k (649k–662k) | 545k (540k–547k) | 525k (517k–527k) | 864k (856k–872k) | 67% | 55 |
| all | 1000 | 200 | 8 | 676k (670k–682k) | 552k (548k–566k) | 463k (440k–472k) | 865k (850k–873k) | 49% | 21 |
| all | 1000 | 1000 | 2 | 296k (280k–305k) | 474k (472k–476k) | 474k (471k–477k) | 474k (468k–477k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 385k (382k–409k) | 427k (425k–429k) | 521k (514k–523k) | 523k (447k–527k) | 99% | 5 |
| all | 1000 | 1000 | 4 | 412k (351k–415k) | 387k (372k–389k) | 549k (480k–551k) | 546k (537k–547k) | 97% | 22 |
| all | 1000 | 1000 | 6 | 438k (430k–442k) | 375k (368k–383k) | 365k (356k–376k) | 509k (500k–515k) | 67% | 78 |
| all | 1000 | 1000 | 8 | 449k (442k–451k) | 381k (375k–383k) | 328k (327k–336k) | 494k (410k–498k) | 50% | 149 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.70 / 0.87 / 1.58 (0.95–4.80) | 0.62 / 0.74 / 4.16 (0.79–8.45) | 0.62 / 0.74 / 0.83 (0.83–3.14) | 0.62 / 0.74 / 4.86 (3.74–6.21) | 99% | 0 |
| one | 0 | 50 | 3 | 0.79 / 1.12 / 8.96 (1.90–12.42) | 0.74 / 1.04 / 8.70 (6.53–14.21) | 0.70 / 0.79 / 5.31 (3.65–6.53) | 0.70 / 0.79 / 5.12 (3.74–9.47) | 99% | 0 |
| one | 0 | 50 | 4 | 0.83 / 1.20 / 9.09 (4.48–12.16) | 1.25 / 1.90 / 9.60 (5.06–16.90) | 0.74 / 0.91 / 8.32 (1.54–14.85) | 0.74 / 1.04 / 8.32 (4.48–14.59) | 97% | 5 |
| one | 0 | 50 | 6 | 1.28 / 1.74 / 9.22 (6.08–24.58) | 1.90 / 2.78 / 8.83 (4.48–14.46) | 1.87 / 2.40 / 6.91 (4.86–13.70) | 1.00 / 1.25 / 7.36 (3.81–12.29) | 65% | 13 |
| one | 0 | 50 | 8 | 1.49 / 1.95 / 9.34 (6.53–12.93) | 2.21 / 3.36 / 10.11 (8.45–11.39) | 2.62 / 3.26 / 8.70 (7.62–12.54) | 1.15 / 1.33 / 6.40 (5.50–8.58) | 51% | 19 |
| one | 0 | 200 | 2 | 0.91 / 1.04 / 3.39 (1.20–4.93) | 0.74 / 0.87 / 0.95 (0.91–4.74) | 0.74 / 0.87 / 1.79 (0.91–5.31) | 0.74 / 0.87 / 0.95 (0.91–3.94) | 100% | 0 |
| one | 0 | 200 | 3 | 1.00 / 1.20 / 5.12 (1.25–7.55) | 0.95 / 1.20 / 6.34 (5.57–14.34) | 0.79 / 0.91 / 4.42 (1.15–5.06) | 0.79 / 0.91 / 4.74 (1.00–7.49) | 100% | 0 |
| one | 0 | 200 | 4 | 1.07 / 1.49 / 8.83 (6.46–17.66) | 1.49 / 1.82 / 8.83 (4.03–17.66) | 0.87 / 1.07 / 7.10 (1.66–16.38) | 0.87 / 0.95 / 5.95 (3.04–14.85) | 99% | 0 |
| one | 0 | 200 | 6 | 1.49 / 1.90 / 8.13 (4.99–9.60) | 2.21 / 3.04 / 8.58 (7.23–9.86) | 2.03 / 2.62 / 7.62 (5.25–9.22) | 1.12 / 1.36 / 7.23 (4.22–8.32) | 65% | 8 |
| one | 0 | 200 | 8 | 1.74 / 2.21 / 11.78 (9.09–50.69) | 2.53 / 3.74 / 10.24 (8.06–49.15) | 2.85 / 3.62 / 12.42 (8.83–66.56) | 1.28 / 1.49 / 10.75 (7.23–28.67) | 48% | 14 |
| one | 0 | 1000 | 2 | 2.50 / 2.66 / 8.96 (6.27–11.65) | 1.54 / 1.66 / 8.32 (5.82–9.47) | 1.54 / 1.66 / 4.61 (1.74–9.09) | 1.54 / 1.62 / 3.55 (1.70–8.96) | 100% | 0 |
| one | 0 | 1000 | 3 | 2.62 / 2.78 / 10.62 (4.61–13.70) | 2.62 / 2.85 / 10.37 (8.83–13.06) | 1.58 / 1.70 / 8.32 (2.66–17.15) | 1.58 / 1.70 / 8.70 (4.74–9.98) | 100% | 1 |
| one | 0 | 1000 | 4 | 2.66 / 2.82 / 9.47 (9.22–13.70) | 3.04 / 3.46 / 10.11 (8.96–15.49) | 1.66 / 2.00 / 9.09 (8.00–13.18) | 1.66 / 1.82 / 10.24 (1.82–38.91) | 98% | 3 |
| one | 0 | 1000 | 6 | 3.14 / 3.78 / 10.24 (8.06–17.92) | 3.90 / 5.31 / 11.90 (8.83–27.39) | 3.65 / 4.35 / 8.96 (7.30–12.67) | 1.90 / 2.21 / 9.86 (5.31–13.57) | 65% | 1 |
| one | 0 | 1000 | 8 | 3.36 / 5.06 / 15.87 (10.37–44.03) | 4.29 / 5.50 / 11.65 (11.39–58.37) | 4.42 / 5.12 / 13.70 (10.24–51.71) | 2.11 / 2.43 / 11.65 (9.22–28.42) | 49% | 5 |
| one | 1000 | 50 | 2 | 1.74 / 1.87 / 5.82 (4.48–7.04) | 1.62 / 1.74 / 5.50 (1.82–7.36) | 1.62 / 1.74 / 2.94 (1.87–6.46) | 1.62 / 1.74 / 4.48 (1.82–6.40) | 100% | 0 |
| one | 1000 | 50 | 3 | 1.78 / 1.90 / 6.34 (5.38–10.24) | 1.70 / 1.87 / 7.10 (1.95–10.75) | 1.70 / 1.79 / 7.74 (1.87–9.73) | 1.70 / 1.79 / 8.19 (1.95–10.62) | 100% | 1 |
| one | 1000 | 50 | 4 | 1.82 / 2.14 / 13.70 (6.53–17.41) | 3.10 / 3.90 / 13.57 (8.58–19.97) | 1.70 / 2.00 / 9.86 (1.95–16.90) | 1.70 / 1.82 / 9.60 (1.90–12.80) | 99% | 0 |
| one | 1000 | 50 | 6 | 2.30 / 2.82 / 10.37 (5.57–11.90) | 3.87 / 4.54 / 12.42 (11.39–14.46) | 3.62 / 4.48 / 12.03 (9.22–12.42) | 2.03 / 2.27 / 8.96 (6.40–12.16) | 65% | 14 |
| one | 1000 | 50 | 8 | 2.56 / 3.33 / 12.29 (10.75–47.10) | 4.03 / 4.93 / 13.82 (12.16–40.96) | 4.16 / 7.17 / 27.65 (10.50–77.82) | 2.21 / 2.53 / 12.16 (11.14–96.26) | 49% | 16 |
| one | 1000 | 200 | 2 | 1.90 / 2.11 / 7.49 (2.11–14.59) | 1.74 / 1.87 / 7.68 (1.95–9.09) | 1.74 / 1.87 / 6.78 (3.78–9.98) | 1.74 / 1.87 / 6.66 (1.95–13.06) | 99% | 0 |
| one | 1000 | 200 | 3 | 2.03 / 2.24 / 10.62 (2.85–10.75) | 2.03 / 2.14 / 6.78 (2.27–11.14) | 1.79 / 1.87 / 6.34 (2.00–9.34) | 1.79 / 1.90 / 9.09 (2.00–11.01) | 100% | 4 |
| one | 1000 | 200 | 4 | 2.11 / 2.69 / 11.65 (2.69–14.98) | 3.39 / 3.65 / 11.14 (3.74–15.74) | 1.87 / 1.95 / 8.58 (2.66–8.96) | 1.87 / 2.08 / 8.19 (5.50–9.34) | 99% | 1 |
| one | 1000 | 200 | 6 | 2.56 / 2.98 / 8.58 (7.49–12.42) | 4.06 / 4.67 / 11.78 (7.42–12.93) | 3.90 / 4.42 / 12.03 (7.42–12.93) | 2.14 / 2.40 / 8.32 (6.21–14.34) | 68% | 16 |
| one | 1000 | 200 | 8 | 2.78 / 3.23 / 9.73 (9.60–17.66) | 4.29 / 4.80 / 13.44 (11.52–97.28) | 4.42 / 5.06 / 12.54 (10.50–13.44) | 2.30 / 2.50 / 8.45 (7.42–28.42) | 52% | 11 |
| one | 1000 | 1000 | 2 | 3.52 / 3.65 / 11.52 (4.16–13.82) | 2.53 / 2.66 / 5.82 (2.72–9.86) | 2.53 / 2.66 / 9.47 (2.82–13.06) | 2.53 / 2.66 / 9.73 (6.34–10.50) | 100% | 0 |
| one | 1000 | 1000 | 3 | 3.62 / 3.81 / 9.98 (8.45–20.48) | 3.58 / 3.74 / 10.11 (6.98–18.69) | 2.56 / 2.66 / 6.66 (4.00–19.46) | 2.56 / 2.66 / 7.68 (4.29–17.15) | 100% | 0 |
| one | 1000 | 1000 | 4 | 3.68 / 3.90 / 11.39 (8.32–25.60) | 4.99 / 5.44 / 12.42 (10.11–22.27) | 2.66 / 2.98 / 13.82 (3.26–19.20) | 2.66 / 3.04 / 11.65 (9.60–98.30) | 98% | 3 |
| one | 1000 | 1000 | 6 | 4.16 / 7.42 / 16.38 (10.75–28.93) | 5.76 / 7.04 / 14.46 (12.03–18.69) | 5.44 / 6.66 / 14.59 (8.83–53.76) | 2.94 / 3.23 / 9.47 (7.36–16.90) | 68% | 6 |
| one | 1000 | 1000 | 8 | 4.42 / 4.99 / 14.08 (12.80–19.97) | 5.89 / 6.85 / 15.23 (13.06–23.30) | 6.08 / 14.08 / 60.42 (12.93–182.27) | 3.10 / 4.29 / 13.70 (9.09–49.15) | 48% | 19 |
| all | 0 | 50 | 2 | 0.78 / 1.20 / 6.21 (3.49–7.17) | 1.00 / 1.15 / 5.63 (1.20–8.32) | 1.00 / 1.15 / 5.57 (1.20–5.70) | 1.00 / 1.15 / 4.74 (1.25–13.57) | 100% | 0 |
| all | 0 | 50 | 3 | 1.00 / 1.66 / 8.06 (4.74–18.43) | 1.33 / 2.30 / 11.90 (2.40–15.23) | 1.57 / 1.74 / 6.14 (1.82–12.67) | 1.57 / 1.74 / 7.36 (1.90–10.50) | 100% | 0 |
| all | 0 | 50 | 4 | 1.33 / 2.24 / 8.83 (4.00–13.70) | 2.43 / 3.87 / 8.32 (6.40–11.90) | 2.14 / 2.40 / 9.34 (3.39–16.00) | 2.14 / 2.37 / 10.75 (5.82–14.59) | 100% | 0 |
| all | 0 | 50 | 6 | 2.62 / 6.40 / 13.82 (9.09–13.95) | 5.38 / 9.34 / 15.62 (12.29–25.34) | 5.95 / 9.98 / 17.66 (12.80–24.83) | 3.78 / 5.31 / 12.54 (7.49–20.22) | 68% | 44 |
| all | 0 | 50 | 8 | 4.67 / 11.90 / 18.18 (15.10–41.98) | 8.19 / 14.46 / 22.02 (20.22–129.02) | 11.39 / 19.46 / 33.79 (24.58–53.25) | 5.82 / 8.45 / 18.94 (14.34–26.62) | 49% | 37 |
| all | 0 | 200 | 2 | 1.00 / 1.46 / 1.57 (1.54–1.70) | 1.28 / 1.41 / 1.49 (1.46–3.20) | 1.25 / 1.41 / 1.54 (1.46–9.98) | 1.28 / 1.41 / 1.70 (1.46–8.70) | 100% | 0 |
| all | 0 | 200 | 3 | 1.25 / 2.24 / 7.10 (4.48–14.08) | 2.27 / 2.66 / 7.30 (2.82–10.24) | 1.95 / 2.14 / 7.10 (6.40–10.88) | 2.00 / 2.14 / 6.91 (5.44–9.22) | 100% | 5 |
| all | 0 | 200 | 4 | 2.03 / 2.72 / 8.06 (6.59–11.65) | 3.14 / 4.61 / 14.85 (6.66–26.88) | 2.69 / 2.91 / 8.06 (6.21–19.46) | 2.69 / 2.85 / 8.19 (6.08–19.46) | 100% | 0 |
| all | 0 | 200 | 6 | 3.14 / 7.10 / 12.42 (12.03–15.74) | 5.95 / 10.62 / 16.64 (13.70–32.77) | 7.04 / 12.16 / 22.53 (18.43–25.60) | 4.74 / 6.08 / 12.03 (10.37–18.69) | 68% | 42 |
| all | 0 | 200 | 8 | 5.18 / 13.44 / 20.48 (18.69–137.22) | 8.96 / 16.00 / 32.00 (23.04–141.31) | 12.93 / 19.71 / 36.86 (28.67–108.54) | 6.98 / 9.09 / 21.76 (14.98–55.30) | 49% | 67 |
| all | 0 | 1000 | 2 | 2.56 / 3.94 / 9.34 (8.96–21.50) | 2.85 / 2.98 / 9.22 (6.21–22.53) | 2.85 / 2.98 / 10.24 (3.10–15.23) | 2.85 / 2.98 / 7.36 (5.76–9.86) | 100% | 0 |
| all | 0 | 1000 | 3 | 3.10 / 5.31 / 12.03 (10.11–26.88) | 4.74 / 5.12 / 13.82 (12.03–26.88) | 4.29 / 4.48 / 13.31 (8.19–21.25) | 4.29 / 4.48 / 9.98 (7.94–14.34) | 100% | 0 |
| all | 0 | 1000 | 4 | 4.48 / 6.34 / 14.59 (12.42–22.78) | 6.53 / 9.34 / 18.69 (13.70–28.16) | 5.82 / 6.66 / 12.42 (10.50–26.62) | 5.82 / 6.27 / 18.69 (12.03–25.60) | 100% | 15 |
| all | 0 | 1000 | 6 | 7.49 / 13.31 / 25.09 (18.94–29.70) | 11.78 / 18.18 / 27.39 (21.76–33.28) | 12.03 / 19.20 / 29.18 (27.39–55.81) | 10.24 / 11.14 / 22.02 (15.74–25.34) | 67% | 53 |
| all | 0 | 1000 | 8 | 8.58 / 20.48 / 34.82 (28.42–331.78) | 15.23 / 25.34 / 36.35 (32.51–115.71) | 20.99 / 30.72 / 46.59 (37.89–74.75) | 14.21 / 15.62 / 26.62 (23.04–135.17) | 50% | 67 |
| all | 1000 | 50 | 2 | 1.66 / 2.08 / 5.70 (2.21–9.73) | 1.04 / 1.79 / 3.81 (1.79–8.45) | 1.04 / 1.79 / 6.66 (1.87–8.70) | 1.04 / 1.79 / 5.57 (1.87–6.66) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.21 / 3.23 / 6.59 (4.29–10.88) | 2.00 / 3.52 / 7.94 (3.90–11.14) | 1.57 / 1.82 / 8.19 (1.87–8.83) | 1.57 / 1.82 / 8.06 (2.00–17.92) | 100% | 0 |
| all | 1000 | 50 | 4 | 2.30 / 4.35 / 14.46 (5.89–18.43) | 3.20 / 5.12 / 13.57 (6.66–26.11) | 2.11 / 2.82 / 12.67 (2.72–18.69) | 2.11 / 2.40 / 9.09 (2.56–13.31) | 99% | 0 |
| all | 1000 | 50 | 6 | 3.62 / 7.36 / 13.18 (9.22–14.08) | 5.25 / 9.73 / 16.90 (15.36–40.96) | 5.25 / 9.86 / 17.92 (12.16–42.50) | 3.20 / 5.31 / 13.44 (8.32–18.69) | 67% | 106 |
| all | 1000 | 50 | 8 | 5.44 / 11.26 / 22.78 (17.66–106.50) | 7.10 / 13.44 / 23.30 (19.20–58.37) | 8.96 / 17.41 / 31.74 (24.32–75.78) | 4.16 / 7.87 / 18.94 (14.21–32.00) | 49% | 60 |
| all | 1000 | 200 | 2 | 1.95 / 2.50 / 8.19 (2.82–26.88) | 1.25 / 1.41 / 6.53 (1.82–7.10) | 1.25 / 1.41 / 1.87 (1.79–5.63) | 1.25 / 1.41 / 1.90 (1.49–7.30) | 100% | 1 |
| all | 1000 | 200 | 3 | 2.50 / 3.65 / 10.37 (7.74–16.64) | 2.40 / 3.74 / 9.47 (6.78–19.97) | 2.00 / 2.14 / 8.19 (5.70–15.62) | 1.95 / 2.14 / 8.32 (7.36–16.38) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.69 / 4.99 / 17.92 (8.06–20.99) | 3.52 / 5.50 / 15.36 (7.55–24.06) | 2.66 / 3.33 / 12.29 (3.20–17.92) | 2.69 / 3.07 / 11.65 (7.17–14.21) | 100% | 0 |
| all | 1000 | 200 | 6 | 4.16 / 8.06 / 13.82 (12.67–18.43) | 5.95 / 10.50 / 16.64 (13.95–24.06) | 5.95 / 10.88 / 20.99 (13.18–23.30) | 3.94 / 6.27 / 13.70 (10.50–16.90) | 67% | 55 |
| all | 1000 | 200 | 8 | 6.02 / 11.39 / 22.27 (17.92–82.94) | 8.32 / 15.10 / 38.91 (22.02–45.06) | 9.73 / 17.92 / 29.18 (26.62–111.62) | 5.50 / 9.09 / 19.71 (13.70–72.70) | 49% | 21 |
| all | 1000 | 1000 | 2 | 3.49 / 4.86 / 9.60 (6.78–12.42) | 2.85 / 2.98 / 8.58 (5.18–9.22) | 2.85 / 2.98 / 8.70 (3.07–10.11) | 2.85 / 2.98 / 9.47 (4.74–12.93) | 100% | 0 |
| all | 1000 | 1000 | 3 | 4.86 / 6.40 / 11.78 (11.39–13.70) | 4.00 / 6.46 / 14.08 (10.37–14.85) | 4.35 / 4.48 / 12.16 (9.86–40.45) | 4.35 / 4.48 / 13.82 (9.09–96.26) | 99% | 5 |
| all | 1000 | 1000 | 4 | 5.12 / 8.96 / 25.60 (9.47–112.64) | 7.10 / 10.11 / 23.55 (21.50–39.42) | 5.82 / 6.78 / 19.46 (15.23–118.78) | 5.89 / 6.72 / 21.25 (19.20–25.34) | 97% | 22 |
| all | 1000 | 1000 | 6 | 6.59 / 13.57 / 29.18 (25.34–42.50) | 11.26 / 17.41 / 26.62 (23.30–62.46) | 11.39 / 17.41 / 23.81 (21.50–32.51) | 10.24 / 12.16 / 22.78 (15.49–32.26) | 67% | 78 |
| all | 1000 | 1000 | 8 | 8.70 / 17.66 / 37.38 (26.88–74.75) | 14.85 / 24.06 / 37.38 (30.98–151.55) | 16.90 / 29.70 / 51.20 (38.91–59.39) | 14.59 / 16.90 / 31.49 (23.55–258.05) | 50% | 149 |

Migrations seen: 119625 over 90735959 samples (one sample per measured operation).


### Suite c: one k-thread process and n-k single-thread processes

load average before: { 5.54 6.64 6.11 }
load average after: { 6.79 6.76 6.18 }; 58 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.29 / 0.46 / 0.62 (0.58–0.66) | 0.29 / 0.50 / 1.00 (0.70–1.49) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 99% | 0 |
| 4 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.29 / 0.42 / 0.58 (0.50–0.66) | 0.29 / 0.46 / 0.74 (0.54–1.25) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 99% | 0 |
| 4 | 200 | 3 | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.29 / 0.54 / 0.70 (0.70–0.74) | 0.29 / 0.46 / 0.79 (0.58–5.31) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 98% | 21 |
| 4 | 1000 | 1 | 1.00 / 1.04 / 1.12 (1.04–1.15) | 1.07 / 1.20 / 1.90 (1.54–5.12) | 2.94 / 3.23 / 12.42 (9.60–16.90) | 1.00 / 1.04 / 1.12 (1.07–1.20) | 99% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.04 / 1.12 (1.04–1.15) | 1.07 / 1.25 / 4.80 (1.28–6.02) | 2.94 / 3.26 / 14.34 (12.80–14.59) | 1.00 / 1.04 / 1.15 (1.07–1.25) | 99% | 0 |
| 4 | 1000 | 3 | 1.00 / 1.07 / 1.15 (1.07–1.58) | 1.07 / 2.11 / 2.24 (2.24–7.49) | 2.98 / 3.36 / 14.59 (12.67–19.97) | 1.00 / 1.04 / 1.12 (1.07–1.15) | 99% | 0 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.66 (0.62–1.41) | 0.37 / 0.62 / 0.74 (0.74–7.23) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 3 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.58 / 0.66 (0.66–0.66) | 0.37 / 0.62 / 0.79 (0.74–0.79) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 12 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.83 (0.79–0.91) | 0.42 / 0.66 / 0.83 (0.74–3.90) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 50% | 3 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.91 (0.91–0.91) | 0.42 / 1.12 / 1.66 (1.57–1.82) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 24 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.66 (0.62–0.70) | 0.42 / 1.07 / 1.66 (1.49–7.30) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 26 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.12 (1.12–2.00) | 1.12 / 1.46 / 2.72 (1.62–7.17) | 3.14 / 3.46 / 10.75 (10.11–19.20) | 1.00 / 1.07 / 1.12 (1.07–5.18) | 49% | 17 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.66 (1.07–5.06) | 1.15 / 1.54 / 6.59 (1.62–8.00) | 3.20 / 3.78 / 19.46 (9.73–20.74) | 1.00 / 1.07 / 1.15 (1.12–5.57) | 49% | 33 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.12 (1.07–5.82) | 1.15 / 1.90 / 2.40 (2.37–7.23) | 3.20 / 3.65 / 10.50 (10.50–12.03) | 1.00 / 1.07 / 1.12 (1.12–1.12) | 49% | 8 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.15 / 2.11 / 2.62 (2.43–3.94) | 3.20 / 4.42 / 10.37 (8.83–12.67) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 49% | 18 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.12 / 1.46 / 2.30 (1.62–5.70) | 3.14 / 4.42 / 10.11 (8.83–10.62) | 1.00 / 1.07 / 1.12 (1.12–1.15) | 49% | 14 |

Independent processes: operations/s

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 1.91M (1.91M–1.92M) | 1.83M (1.81M–1.84M) | 1.81M (1.80M–1.83M) | 1.90M (1.90M–1.91M) | 99% | 0 |
| 4 | 200 | 2 | 1.27M (1.27M–1.27M) | 1.22M (1.21M–1.22M) | 1.21M (1.20M–1.21M) | 1.27M (1.27M–1.27M) | 99% | 0 |
| 4 | 200 | 3 | 634k (633k–638k) | 524k (520k–526k) | 603k (526k–604k) | 636k (629k–639k) | 98% | 21 |
| 4 | 1000 | 1 | 1.27M (1.26M–1.27M) | 1.23M (1.22M–1.23M) | 692k (688k–696k) | 1.27M (1.27M–1.27M) | 99% | 0 |
| 4 | 1000 | 2 | 846k (843k–848k) | 818k (817k–822k) | 459k (458k–459k) | 846k (842k–846k) | 99% | 0 |
| 4 | 1000 | 3 | 423k (421k–424k) | 356k (350k–357k) | 228k (227k–229k) | 423k (423k–424k) | 99% | 0 |
| 8 | 200 | 1 | 4.44M (3.97M–4.45M) | 4.19M (4.16M–4.21M) | 4.07M (3.98M–4.08M) | 4.43M (4.38M–4.45M) | 49% | 3 |
| 8 | 200 | 2 | 3.80M (3.66M–3.81M) | 3.58M (3.57M–3.60M) | 3.48M (3.46M–3.49M) | 3.82M (3.79M–3.82M) | 49% | 12 |
| 8 | 200 | 3 | 3.18M (3.17M–3.18M) | 2.88M (2.86M–2.89M) | 2.87M (2.84M–2.91M) | 3.18M (3.16M–3.19M) | 50% | 3 |
| 8 | 200 | 5 | 1.90M (1.90M–1.91M) | 1.66M (1.62M–1.67M) | 1.33M (1.28M–1.34M) | 1.91M (1.91M–1.92M) | 49% | 24 |
| 8 | 200 | 6 | 1.27M (1.27M–1.28M) | 1.20M (1.18M–1.21M) | 876k (841k–889k) | 1.28M (1.27M–1.29M) | 49% | 26 |
| 8 | 1000 | 1 | 3.03M (3.01M–3.03M) | 2.84M (2.82M–2.86M) | 1.55M (1.39M–1.56M) | 3.02M (2.98M–3.03M) | 49% | 17 |
| 8 | 1000 | 2 | 2.58M (2.57M–2.60M) | 2.40M (2.37M–2.44M) | 1.31M (1.29M–1.32M) | 2.59M (2.57M–2.60M) | 49% | 33 |
| 8 | 1000 | 3 | 2.16M (2.13M–2.16M) | 1.96M (1.96M–1.99M) | 1.11M (1.09M–1.12M) | 2.16M (2.15M–2.16M) | 49% | 8 |
| 8 | 1000 | 5 | 1.29M (1.29M–1.30M) | 1.12M (1.11M–1.13M) | 602k (594k–607k) | 1.30M (1.29M–1.30M) | 49% | 18 |
| 8 | 1000 | 6 | 864k (864k–867k) | 819k (813k–821k) | 413k (411k–417k) | 864k (863k–866k) | 49% | 14 |

The k-thread process: operations/s

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 640k (638k–644k) | 611k (606k–612k) | 601k (599k–610k) | 635k (634k–637k) | 99% | 0 |
| 4 | 200 | 2 | 1.06M (1.06M–1.07M) | 1.22M (1.20M–1.22M) | 1.21M (1.20M–1.21M) | 1.21M (1.21M–1.23M) | 99% | 0 |
| 4 | 200 | 3 | 1.45M (1.45M–1.45M) | 1.49M (1.49M–1.49M) | 1.81M (1.56M–1.81M) | 1.81M (1.77M–1.82M) | 98% | 21 |
| 4 | 1000 | 1 | 423k (422k–423k) | 409k (408k–411k) | 231k (230k–232k) | 424k (423k–425k) | 99% | 0 |
| 4 | 1000 | 2 | 708k (706k–709k) | 820k (815k–824k) | 459k (458k–459k) | 820k (818k–825k) | 99% | 0 |
| 4 | 1000 | 3 | 961k (959k–964k) | 989k (979k–994k) | 684k (681k–686k) | 910k (905k–919k) | 99% | 0 |
| 8 | 200 | 1 | 640k (542k–641k) | 598k (592k–602k) | 581k (567k–584k) | 637k (631k–638k) | 49% | 3 |
| 8 | 200 | 2 | 1.05M (1.03M–1.06M) | 1.19M (1.18M–1.19M) | 1.16M (1.15M–1.16M) | 1.19M (1.18M–1.20M) | 49% | 12 |
| 8 | 200 | 3 | 1.42M (1.41M–1.43M) | 1.42M (1.41M–1.44M) | 1.72M (1.70M–1.74M) | 1.70M (1.66M–1.75M) | 50% | 3 |
| 8 | 200 | 5 | 2.10M (2.04M–2.12M) | 2.07M (2.03M–2.11M) | 2.18M (2.09M–2.22M) | 2.77M (2.68M–2.79M) | 49% | 24 |
| 8 | 200 | 6 | 2.41M (2.37M–2.43M) | 2.40M (2.39M–2.47M) | 2.51M (2.37M–2.53M) | 2.64M (2.52M–2.73M) | 49% | 26 |
| 8 | 1000 | 1 | 432k (430k–433k) | 409k (399k–411k) | 221k (198k–224k) | 433k (426k–433k) | 49% | 17 |
| 8 | 1000 | 2 | 702k (701k–713k) | 804k (797k–811k) | 431k (407k–434k) | 803k (776k–821k) | 49% | 33 |
| 8 | 1000 | 3 | 961k (949k–966k) | 961k (945k–970k) | 644k (626k–650k) | 866k (851k–872k) | 49% | 8 |
| 8 | 1000 | 5 | 1.40M (1.38M–1.40M) | 1.40M (1.36M–1.40M) | 954k (934k–969k) | 840k (820k–849k) | 49% | 18 |
| 8 | 1000 | 6 | 1.57M (1.56M–1.59M) | 1.61M (1.61M–1.62M) | 1.11M (1.11M–1.13M) | 843k (821k–849k) | 49% | 14 |

Migrations seen: 90172 over 141272211 samples (one sample per measured operation).


### Suite d: one core moving between group A and group B (one fixed core each)

load average before: { 6.79 6.76 6.18 }
load average after: { 6.09 6.61 6.13 }; 15 s

Throughput of the 3 cores, operations/s

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | 1.84M (1.84M–1.85M) | 100% | 0 |
| 0 | 200 | 100 | 1.84M (1.83M–1.85M) | 100% | 0 |
| 0 | 200 | 1000 | 1.83M (1.70M–1.85M) | 98% | 7 |
| 0 | 200 | 10000 | 1.84M (1.84M–1.85M) | 100% | 0 |
| 0 | 1000 | 0 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 0 | 1000 | 100 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 0 | 1000 | 1000 | 1.23M (1.23M–1.23M) | 100% | 0 |
| 0 | 1000 | 10000 | 1.22M (1.21M–1.23M) | 100% | 0 |
| 1000 | 200 | 0 | 1.85M (1.84M–1.85M) | 100% | 0 |
| 1000 | 200 | 100 | 1.84M (1.84M–1.84M) | 100% | 0 |
| 1000 | 200 | 1000 | 1.84M (1.83M–1.85M) | 100% | 0 |
| 1000 | 200 | 10000 | 1.84M (1.83M–1.84M) | 100% | 0 |
| 1000 | 1000 | 0 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 1000 | 1000 | 100 | 1.23M (1.23M–1.23M) | 100% | 0 |
| 1000 | 1000 | 1000 | 1.23M (1.22M–1.23M) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.22M (1.22M–1.22M) | 100% | 0 |

Kernel operations other than the first after a move, µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | 0.25 / 0.46 / 2.24 (2.21–2.27) | 100% | 0 |
| 0 | 200 | 100 | 0.29 / 0.46 / 2.27 (2.24–2.27) | 100% | 0 |
| 0 | 200 | 1000 | 0.29 / 0.46 / 2.27 (2.24–2.27) | 98% | 7 |
| 0 | 200 | 10000 | 0.29 / 0.46 / 2.24 (2.24–2.27) | 100% | 0 |
| 0 | 1000 | 0 | 1.04 / 1.41 / 4.48 (3.87–5.76) | 100% | 0 |
| 0 | 1000 | 100 | 1.04 / 1.41 / 4.29 (3.87–6.21) | 100% | 0 |
| 0 | 1000 | 1000 | 1.04 / 1.70 / 3.94 (3.87–4.29) | 100% | 0 |
| 0 | 1000 | 10000 | 1.04 / 1.90 / 3.90 (3.87–6.85) | 100% | 0 |
| 1000 | 200 | 0 | 0.25 / 0.42 / 2.24 (2.21–2.24) | 100% | 0 |
| 1000 | 200 | 100 | 0.29 / 0.46 / 2.27 (2.27–2.27) | 100% | 0 |
| 1000 | 200 | 1000 | 0.29 / 0.46 / 2.27 (2.24–2.27) | 100% | 0 |
| 1000 | 200 | 10000 | 0.29 / 0.46 / 2.24 (2.24–2.27) | 100% | 0 |
| 1000 | 1000 | 0 | 1.04 / 1.33 / 3.87 (3.87–5.63) | 100% | 0 |
| 1000 | 1000 | 100 | 1.04 / 1.33 / 3.87 (3.87–4.99) | 100% | 0 |
| 1000 | 1000 | 1000 | 1.04 / 1.78 / 3.90 (3.87–5.12) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.04 / 1.90 / 3.90 (3.87–5.06) | 100% | 0 |

Move (quiesce, leave, switch, join), µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | – | 100% | 0 |
| 0 | 200 | 100 | 0.66 / 3.74 / 3.74 (0.70–4.86) | 100% | 0 |
| 0 | 200 | 1000 | 0.66 / 3.81 / 3.87 (3.74–9.60) | 98% | 7 |
| 0 | 200 | 10000 | 0.62 / 0.91 / 4.16 (1.87–6.40) | 100% | 0 |
| 0 | 1000 | 0 | – | 100% | 0 |
| 0 | 1000 | 100 | 2.27 / 3.94 / 3.94 (3.26–5.25) | 100% | 0 |
| 0 | 1000 | 1000 | 2.27 / 6.08 / 6.14 (3.62–14.08) | 100% | 0 |
| 0 | 1000 | 10000 | 2.24 / 3.26 / 11.14 (3.68–20.22) | 100% | 0 |
| 1000 | 200 | 0 | – | 100% | 0 |
| 1000 | 200 | 100 | 1.66 / 4.54 / 4.54 (4.48–4.80) | 100% | 0 |
| 1000 | 200 | 1000 | 1.70 / 4.42 / 5.06 (1.90–7.36) | 100% | 0 |
| 1000 | 200 | 10000 | 1.66 / 1.95 / 7.36 (4.99–12.54) | 100% | 0 |
| 1000 | 1000 | 0 | – | 100% | 0 |
| 1000 | 1000 | 100 | 3.39 / 4.99 / 4.99 (4.10–9.34) | 100% | 0 |
| 1000 | 1000 | 1000 | 3.33 / 4.29 / 4.35 (4.29–10.50) | 100% | 0 |
| 1000 | 1000 | 10000 | 3.26 / 4.42 / 11.90 (5.82–19.97) | 100% | 0 |

First operation after a move, µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | – | 100% | 0 |
| 0 | 200 | 100 | 0.25 / 0.54 / 0.54 (0.54–0.58) | 100% | 0 |
| 0 | 200 | 1000 | 0.29 / 1.58 / 2.85 (2.24–10.50) | 98% | 7 |
| 0 | 200 | 10000 | 0.29 / 1.57 / 2.27 (2.21–3.07) | 100% | 0 |
| 0 | 1000 | 0 | – | 100% | 0 |
| 0 | 1000 | 100 | 1.12 / 2.11 / 2.11 (2.11–3.39) | 100% | 0 |
| 0 | 1000 | 1000 | 1.12 / 3.81 / 3.87 (3.81–12.42) | 100% | 0 |
| 0 | 1000 | 10000 | 1.07 / 3.49 / 6.34 (3.81–12.80) | 100% | 0 |
| 1000 | 200 | 0 | – | 100% | 0 |
| 1000 | 200 | 100 | 0.29 / 0.54 / 0.54 (0.46–0.58) | 100% | 0 |
| 1000 | 200 | 1000 | 0.29 / 1.62 / 1.82 (0.58–2.24) | 100% | 0 |
| 1000 | 200 | 10000 | 0.29 / 1.57 / 2.21 (2.14–2.82) | 100% | 0 |
| 1000 | 1000 | 0 | – | 100% | 0 |
| 1000 | 1000 | 100 | 1.66 / 2.11 / 2.11 (2.11–5.70) | 100% | 0 |
| 1000 | 1000 | 1000 | 2.03 / 3.52 / 3.78 (2.27–8.19) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.07 / 3.07 / 6.91 (3.81–7.81) | 100% | 0 |

Migrations seen: 10356 over 18892053 samples (one sample per measured operation).


## Appendix B: full tables, pass B


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 17.6 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite locks: one group of k cores, three locks

load average before: { 4.80 6.28 6.02 }
load average after: { 6.29 6.44 6.11 }; 82 s

Throughput, operations/s, median (min–max)

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 5.61M (4.85M–5.62M) | 6.75M (5.26M–6.80M) | 6.73M (5.07M–6.81M) | 89% | 18 |
| 0 | 50 | 3 | 5.61M (3.72M–5.65M) | 6.14M (4.90M–6.20M) | 6.11M (4.76M–6.16M) | 88% | 38 |
| 0 | 50 | 4 | 5.57M (4.75M–5.61M) | 5.74M (1.67M–5.90M) | 5.64M (2.60M–6.15M) | 84% | 178 |
| 0 | 50 | 6 | 4.91M (4.69M–4.96M) | 4.15M (3.93M–4.42M) | 4.50M (3.25M–4.59M) | 60% | 174 |
| 0 | 50 | 8 | 4.73M (4.58M–4.75M) | 4.06M (3.88M–4.12M) | 4.44M (4.40M–4.53M) | 43% | 176 |
| 0 | 200 | 2 | 3.25M (3.24M–3.29M) | 3.50M (3.38M–3.55M) | 3.51M (3.38M–3.55M) | 98% | 1 |
| 0 | 200 | 3 | 3.14M (3.08M–3.16M) | 3.36M (3.32M–3.40M) | 3.35M (3.34M–3.38M) | 99% | 4 |
| 0 | 200 | 4 | 3.07M (3.00M–3.08M) | 3.25M (2.73M–3.25M) | 3.33M (2.88M–3.37M) | 96% | 1 |
| 0 | 200 | 6 | 2.86M (2.86M–2.87M) | 2.52M (2.44M–2.55M) | 2.67M (2.63M–2.71M) | 72% | 61 |
| 0 | 200 | 8 | 2.75M (2.46M–2.76M) | 2.39M (2.30M–2.41M) | 2.65M (2.46M–2.67M) | 58% | 289 |
| 0 | 1000 | 2 | 901k (896k–905k) | 916k (916k–920k) | 918k (914k–920k) | 100% | 1 |
| 0 | 1000 | 3 | 892k (867k–896k) | 907k (892k–915k) | 908k (891k–917k) | 98% | 12 |
| 0 | 1000 | 4 | 890k (862k–894k) | 901k (708k–908k) | 905k (853k–912k) | 93% | 50 |
| 0 | 1000 | 6 | 852k (851k–856k) | 829k (22k–850k) | 843k (391k–856k) | 68% | 380 |
| 0 | 1000 | 8 | 857k (613k–858k) | 824k (287k–828k) | 767k (205k–850k) | 60% | 1570 |
| 1000 | 50 | 2 | 1.34M (1.05M–1.36M) | 1.33M (1.31M–1.35M) | 1.34M (1.32M–1.35M) | 97% | 3 |
| 1000 | 50 | 3 | 2.00M (1.88M–2.01M) | 1.99M (1.45M–2.00M) | 1.99M (1.11M–2.01M) | 90% | 40 |
| 1000 | 50 | 4 | 2.65M (1.57M–2.68M) | 2.63M (1.21M–2.67M) | 2.64M (1.90M–2.67M) | 91% | 66 |
| 1000 | 50 | 6 | 3.66M (2.66M–3.71M) | 3.67M (1.96M–3.69M) | 3.70M (1.75M–3.73M) | 63% | 118 |
| 1000 | 50 | 8 | 3.94M (3.19M–3.95M) | 4.13M (1.35M–4.23M) | 4.32M (1.46M–4.44M) | 49% | 300 |
| 1000 | 200 | 2 | 1.24M (1.21M–1.24M) | 1.22M (1.19M–1.23M) | 1.23M (1.21M–1.24M) | 97% | 0 |
| 1000 | 200 | 3 | 1.84M (1.79M–1.85M) | 1.81M (1.51M–1.82M) | 1.83M (1.71M–1.84M) | 94% | 18 |
| 1000 | 200 | 4 | 2.42M (2.33M–2.45M) | 2.40M (2.29M–2.42M) | 2.42M (2.36M–2.44M) | 93% | 0 |
| 1000 | 200 | 6 | 2.53M (2.33M–2.53M) | 2.67M (2.64M–2.72M) | 2.74M (2.69M–2.78M) | 64% | 60 |
| 1000 | 200 | 8 | 2.55M (2.39M–2.56M) | 2.54M (2.34M–2.57M) | 2.80M (2.60M–2.82M) | 47% | 111 |
| 1000 | 1000 | 2 | 824k (819k–826k) | 809k (773k–817k) | 819k (816k–825k) | 99% | 5 |
| 1000 | 1000 | 3 | 840k (826k–842k) | 911k (878k–916k) | 910k (874k–917k) | 96% | 3 |
| 1000 | 1000 | 4 | 845k (734k–846k) | 904k (669k–912k) | 904k (840k–914k) | 87% | 70 |
| 1000 | 1000 | 6 | 822k (818k–829k) | 830k (330k–842k) | 848k (196k–855k) | 52% | 369 |
| 1000 | 1000 | 8 | 827k (490k–831k) | 822k (129k–832k) | 839k (149k–852k) | 38% | 912 |

Latency of a locked operation, µs: p50 / p99 / p99.9 (p99.9 min–max)

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.29 / 0.91 / 6.78 (6.53–15.23) | 0.25 / 0.33 / 0.37 (0.33–0.58) | 0.25 / 0.29 / 0.37 (0.33–0.58) | 89% | 18 |
| 0 | 50 | 3 | 0.29 / 6.53 / 32.51 (31.74–35.84) | 0.42 / 0.50 / 0.79 (0.54–1.00) | 0.46 / 0.50 / 0.74 (0.58–0.91) | 88% | 38 |
| 0 | 50 | 4 | 0.29 / 12.93 / 46.08 (44.54–54.27) | 0.62 / 0.83 / 4.74 (0.74–11.39) | 0.62 / 0.91 / 4.99 (1.00–10.62) | 84% | 178 |
| 0 | 50 | 6 | 0.33 / 23.55 / 70.66 (66.56–77.82) | 1.33 / 1.66 / 9.34 (2.14–11.90) | 1.20 / 1.49 / 7.17 (5.12–20.22) | 60% | 174 |
| 0 | 50 | 8 | 0.33 / 33.28 / 88.06 (84.99–96.26) | 1.82 / 2.24 / 10.11 (7.17–16.64) | 1.66 / 2.11 / 9.86 (7.30–11.26) | 43% | 176 |
| 0 | 200 | 2 | 0.25 / 1.58 / 45.57 (37.38–72.70) | 0.50 / 0.58 / 0.66 (0.62–0.87) | 0.50 / 0.58 / 0.66 (0.62–0.87) | 98% | 1 |
| 0 | 200 | 3 | 0.29 / 14.85 / 56.32 (50.18–56.83) | 0.83 / 0.95 / 1.54 (1.12–5.12) | 0.83 / 0.95 / 1.41 (1.00–4.86) | 99% | 4 |
| 0 | 200 | 4 | 0.29 / 19.20 / 47.10 (46.08–55.30) | 1.15 / 1.46 / 7.30 (2.00–7.49) | 1.12 / 1.36 / 7.10 (1.54–10.37) | 96% | 1 |
| 0 | 200 | 6 | 0.33 / 33.79 / 72.70 (71.68–78.85) | 2.24 / 2.56 / 11.65 (9.34–12.16) | 2.11 / 2.40 / 10.11 (8.45–10.88) | 72% | 61 |
| 0 | 200 | 8 | 0.58 / 42.50 / 90.11 (86.02–99.33) | 3.10 / 3.55 / 14.08 (12.80–17.41) | 2.85 / 3.39 / 12.80 (8.83–30.46) | 58% | 289 |
| 0 | 1000 | 2 | 1.04 / 32.00 / 95.23 (91.14–113.66) | 2.11 / 2.21 / 6.53 (2.24–7.49) | 2.11 / 2.21 / 7.17 (5.18–9.86) | 100% | 1 |
| 0 | 1000 | 3 | 1.07 / 58.88 / 157.70 (153.60–165.89) | 3.20 / 3.39 / 11.39 (7.23–14.34) | 3.20 / 3.36 / 12.80 (3.39–14.59) | 98% | 12 |
| 0 | 1000 | 4 | 2.11 / 82.94 / 192.51 (184.32–200.70) | 4.29 / 4.67 / 17.41 (11.39–94.21) | 4.29 / 4.61 / 16.13 (11.01–17.92) | 93% | 50 |
| 0 | 1000 | 6 | 1.12 / 92.16 / 180.22 (167.94–389.12) | 6.98 / 8.32 / 19.71 (11.14–16777.22) | 6.91 / 8.83 / 19.46 (14.98–3440.64) | 68% | 380 |
| 0 | 1000 | 8 | 1.12 / 128.00 / 614.40 (557.06–999.42) | 9.34 / 13.18 / 25.86 (20.74–3473.41) | 9.22 / 16.64 / 88.06 (23.30–7077.89) | 60% | 1570 |
| 1000 | 50 | 2 | 0.12 / 0.29 / 0.33 (0.33–0.54) | 0.17 / 0.29 / 0.33 (0.33–0.50) | 0.17 / 0.25 / 0.33 (0.33–0.46) | 97% | 3 |
| 1000 | 50 | 3 | 0.12 / 0.29 / 0.46 (0.37–5.70) | 0.12 / 0.33 / 0.37 (0.37–0.62) | 0.12 / 0.33 / 0.42 (0.37–0.66) | 90% | 40 |
| 1000 | 50 | 4 | 0.12 / 0.33 / 1.12 (0.50–1.41) | 0.12 / 0.37 / 0.70 (0.37–25.34) | 0.12 / 0.37 / 0.70 (0.46–0.78) | 91% | 66 |
| 1000 | 50 | 6 | 0.25 / 1.33 / 6.98 (6.91–13.44) | 0.29 / 0.58 / 0.83 (0.70–1.95) | 0.29 / 0.58 / 0.83 (0.79–8.32) | 63% | 118 |
| 1000 | 50 | 8 | 0.29 / 12.16 / 30.21 (23.81–30.46) | 0.58 / 0.95 / 5.70 (1.12–68.61) | 0.46 / 0.91 / 5.57 (1.82–21.50) | 49% | 300 |
| 1000 | 200 | 2 | 0.25 / 0.42 / 0.54 (0.54–0.79) | 0.29 / 0.42 / 0.54 (0.50–0.70) | 0.29 / 0.46 / 0.54 (0.50–0.70) | 97% | 0 |
| 1000 | 200 | 3 | 0.25 / 0.42 / 0.58 (0.58–1.15) | 0.29 / 0.46 / 0.58 (0.50–1.28) | 0.29 / 0.46 / 0.54 (0.54–4.67) | 94% | 18 |
| 1000 | 200 | 4 | 0.25 / 0.58 / 1.49 (0.62–6.72) | 0.29 / 0.50 / 1.12 (0.54–2.03) | 0.29 / 0.50 / 0.95 (0.54–1.49) | 93% | 0 |
| 1000 | 200 | 6 | 0.50 / 16.90 / 39.94 (38.91–41.98) | 0.87 / 1.15 / 6.66 (2.40–6.91) | 0.83 / 1.12 / 5.57 (2.08–6.66) | 64% | 60 |
| 1000 | 200 | 8 | 0.54 / 27.90 / 56.32 (55.81–58.37) | 1.78 / 2.11 / 10.24 (7.49–23.04) | 1.46 / 1.87 / 9.22 (7.49–32.77) | 47% | 111 |
| 1000 | 1000 | 2 | 1.04 / 1.33 / 4.16 (1.41–7.23) | 1.07 / 1.25 / 2.27 (1.95–6.66) | 1.07 / 1.25 / 5.06 (1.33–5.63) | 99% | 5 |
| 1000 | 1000 | 3 | 2.11 / 15.62 / 30.21 (29.95–46.59) | 1.90 / 2.08 / 9.09 (2.72–9.60) | 1.90 / 2.11 / 9.47 (2.14–9.98) | 96% | 3 |
| 1000 | 1000 | 4 | 2.11 / 37.89 / 74.75 (72.70–99.33) | 2.98 / 3.49 / 16.38 (11.14–36.86) | 2.98 / 3.46 / 14.34 (8.19–16.26) | 87% | 70 |
| 1000 | 1000 | 6 | 1.20 / 81.92 / 169.98 (155.65–176.13) | 5.70 / 9.34 / 18.69 (12.80–4915.20) | 5.63 / 7.10 / 19.20 (14.34–7733.25) | 52% | 369 |
| 1000 | 1000 | 8 | 2.11 / 105.47 / 217.09 (206.85–1228.80) | 8.19 / 11.14 / 21.76 (19.97–12189.70) | 8.00 / 11.52 / 23.30 (17.66–10485.76) | 38% | 912 |

Fairness: fewest operations of a core / most

| gap ns | W ns | k | ttas | ticket | queue | on P | stalls |
|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.97 | 1.00 | 1.00 | 89% | 18 |
| 0 | 50 | 3 | 0.88 | 1.00 | 1.00 | 88% | 38 |
| 0 | 50 | 4 | 0.84 | 1.00 | 1.00 | 84% | 178 |
| 0 | 50 | 6 | 0.87 | 1.00 | 1.00 | 60% | 174 |
| 0 | 50 | 8 | 0.84 | 1.00 | 1.00 | 43% | 176 |
| 0 | 200 | 2 | 0.95 | 1.00 | 1.00 | 98% | 1 |
| 0 | 200 | 3 | 0.92 | 1.00 | 1.00 | 99% | 4 |
| 0 | 200 | 4 | 0.90 | 1.00 | 1.00 | 96% | 1 |
| 0 | 200 | 6 | 0.78 | 1.00 | 1.00 | 72% | 61 |
| 0 | 200 | 8 | 0.82 | 1.00 | 1.00 | 58% | 289 |
| 0 | 1000 | 2 | 0.98 | 1.00 | 1.00 | 100% | 1 |
| 0 | 1000 | 3 | 0.96 | 1.00 | 1.00 | 98% | 12 |
| 0 | 1000 | 4 | 0.94 | 1.00 | 1.00 | 93% | 50 |
| 0 | 1000 | 6 | 0.90 | 1.00 | 1.00 | 68% | 380 |
| 0 | 1000 | 8 | 0.73 | 1.00 | 1.00 | 60% | 1570 |
| 1000 | 50 | 2 | 1.00 | 1.00 | 1.00 | 97% | 3 |
| 1000 | 50 | 3 | 1.00 | 1.00 | 1.00 | 90% | 40 |
| 1000 | 50 | 4 | 1.00 | 1.00 | 1.00 | 91% | 66 |
| 1000 | 50 | 6 | 0.99 | 0.99 | 0.99 | 63% | 118 |
| 1000 | 50 | 8 | 0.97 | 1.00 | 0.99 | 49% | 300 |
| 1000 | 200 | 2 | 1.00 | 1.00 | 1.00 | 97% | 0 |
| 1000 | 200 | 3 | 1.00 | 1.00 | 1.00 | 94% | 18 |
| 1000 | 200 | 4 | 1.00 | 1.00 | 1.00 | 93% | 0 |
| 1000 | 200 | 6 | 0.98 | 0.99 | 1.00 | 64% | 60 |
| 1000 | 200 | 8 | 0.97 | 0.99 | 1.00 | 47% | 111 |
| 1000 | 1000 | 2 | 1.00 | 1.00 | 1.00 | 99% | 5 |
| 1000 | 1000 | 3 | 0.98 | 1.00 | 1.00 | 96% | 3 |
| 1000 | 1000 | 4 | 0.98 | 1.00 | 1.00 | 87% | 70 |
| 1000 | 1000 | 6 | 0.85 | 1.00 | 1.00 | 52% | 369 |
| 1000 | 1000 | 8 | 0.71 | 1.00 | 1.00 | 38% | 912 |

Migrations seen: 86613 over 165882768 samples (one sample per measured operation).


### Suite a1: capability operations of one k-thread process

load average before: { 6.29 6.44 6.11 }
load average after: { 8.33 6.74 6.26 }; 262 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 703k (701k–705k) | 703k (702k–705k) | 704k (702k–705k) | 703k (703k–705k) | 100% | 0 |
| 1 | 0 | 50 | 2 | 1.40M (1.39M–1.40M) | 1.36M (1.35M–1.36M) | 1.36M (1.35M–1.37M) | 1.36M (1.33M–1.36M) | 100% | 0 |
| 1 | 0 | 50 | 3 | 2.07M (2.06M–2.07M) | 2.03M (2.02M–2.04M) | 2.02M (1.97M–2.02M) | 2.02M (1.96M–2.02M) | 100% | 0 |
| 1 | 0 | 50 | 4 | 2.74M (2.72M–2.76M) | 2.63M (2.61M–2.67M) | 2.62M (2.57M–2.65M) | 2.61M (2.59M–2.69M) | 99% | 5 |
| 1 | 0 | 50 | 6 | 4.05M (4.00M–4.07M) | 3.77M (3.74M–3.88M) | 3.66M (3.60M–3.77M) | 3.59M (3.56M–3.78M) | 67% | 37 |
| 1 | 0 | 50 | 8 | 5.36M (5.34M–5.38M) | 4.95M (4.93M–5.03M) | 4.77M (4.67M–4.83M) | 4.15M (3.93M–4.40M) | 50% | 0 |
| 1 | 0 | 200 | 1 | 646k (641k–648k) | 645k (643k–647k) | 646k (644k–647k) | 646k (646k–646k) | 100% | 0 |
| 1 | 0 | 200 | 2 | 1.28M (1.27M–1.28M) | 1.24M (1.24M–1.24M) | 1.24M (1.24M–1.25M) | 1.24M (1.24M–1.25M) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.90M (1.90M–1.91M) | 1.86M (1.85M–1.86M) | 1.83M (1.66M–1.84M) | 1.84M (1.83M–1.85M) | 99% | 9 |
| 1 | 0 | 200 | 4 | 2.51M (2.51M–2.52M) | 2.42M (2.41M–2.45M) | 2.42M (2.40M–2.43M) | 2.40M (2.39M–2.45M) | 100% | 0 |
| 1 | 0 | 200 | 6 | 3.78M (3.77M–3.80M) | 3.45M (3.39M–3.53M) | 3.42M (3.37M–3.46M) | 2.56M (2.55M–2.64M) | 66% | 9 |
| 1 | 0 | 200 | 8 | 4.90M (4.89M–4.94M) | 4.60M (4.57M–4.71M) | 4.42M (4.33M–4.53M) | 2.72M (2.57M–2.77M) | 49% | 23 |
| 1 | 0 | 1000 | 1 | 426k (418k–427k) | 426k (419k–427k) | 426k (420k–427k) | 426k (426k–427k) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 842k (839k–844k) | 828k (801k–828k) | 828k (825k–829k) | 827k (824k–828k) | 99% | 4 |
| 1 | 0 | 1000 | 3 | 1.25M (1.11M–1.25M) | 1.23M (1.22M–1.23M) | 921k (920k–923k) | 921k (921k–924k) | 98% | 23 |
| 1 | 0 | 1000 | 4 | 1.66M (1.66M–1.66M) | 1.61M (1.60M–1.62M) | 918k (914k–921k) | 916k (913k–919k) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 2.47M (2.46M–2.48M) | 2.30M (2.28M–2.32M) | 1.59M (1.58M–1.64M) | 849k (834k–858k) | 66% | 19 |
| 1 | 0 | 1000 | 8 | 3.28M (3.27M–3.28M) | 3.09M (3.06M–3.10M) | 1.67M (1.64M–1.69M) | 834k (827k–845k) | 49% | 8 |
| 1 | 1000 | 50 | 1 | 704k (701k–706k) | 704k (703k–704k) | 704k (702k–705k) | 704k (703k–705k) | 100% | 0 |
| 1 | 1000 | 50 | 2 | 1.38M (1.37M–1.38M) | 1.36M (1.36M–1.37M) | 1.36M (1.35M–1.37M) | 1.36M (1.35M–1.36M) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.02M (2.02M–2.03M) | 2.00M (1.96M–2.01M) | 2.02M (1.98M–2.02M) | 2.01M (1.97M–2.02M) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.66M (2.61M–2.68M) | 2.57M (2.51M–2.61M) | 2.63M (2.49M–2.64M) | 2.61M (2.20M–2.66M) | 93% | 23 |
| 1 | 1000 | 50 | 6 | 3.90M (3.88M–3.91M) | 3.71M (3.66M–3.72M) | 3.65M (3.54M–3.66M) | 3.70M (493k–3.74M) | 60% | 86 |
| 1 | 1000 | 50 | 8 | 5.06M (913k–5.09M) | 4.62M (2.22M–4.65M) | 4.66M (4.45M–4.73M) | 4.29M (3.00M–4.60M) | 47% | 800 |
| 1 | 1000 | 200 | 1 | 646k (567k–647k) | 644k (631k–646k) | 645k (635k–647k) | 642k (607k–647k) | 96% | 4 |
| 1 | 1000 | 200 | 2 | 1.25M (1.23M–1.26M) | 1.22M (1.16M–1.24M) | 1.20M (1.16M–1.24M) | 1.21M (1.19M–1.24M) | 92% | 8 |
| 1 | 1000 | 200 | 3 | 1.86M (1.35M–1.86M) | 1.79M (1.26M–1.83M) | 1.82M (1.73M–1.85M) | 1.80M (1.75M–1.84M) | 91% | 60 |
| 1 | 1000 | 200 | 4 | 2.44M (1.73M–2.46M) | 2.34M (2.22M–2.38M) | 2.30M (1.19M–2.42M) | 2.32M (2.09M–2.41M) | 79% | 116 |
| 1 | 1000 | 200 | 6 | 3.61M (2.76M–3.64M) | 3.35M (1.74M–3.41M) | 3.27M (2.78M–3.39M) | 2.75M (2.69M–2.82M) | 59% | 216 |
| 1 | 1000 | 200 | 8 | 4.62M (3.58M–4.67M) | 4.30M (1.00M–4.42M) | 4.20M (3.03M–4.38M) | 2.81M (1.91M–2.83M) | 46% | 587 |
| 1 | 1000 | 1000 | 1 | 425k (423k–427k) | 425k (425k–429k) | 426k (408k–427k) | 426k (423k–426k) | 88% | 4 |
| 1 | 1000 | 1000 | 2 | 826k (797k–834k) | 813k (767k–828k) | 813k (786k–830k) | 819k (552k–827k) | 85% | 40 |
| 1 | 1000 | 1000 | 3 | 1.23M (905k–1.23M) | 1.21M (868k–1.22M) | 911k (664k–920k) | 871k (808k–923k) | 86% | 121 |
| 1 | 1000 | 1000 | 4 | 1.62M (1.26M–1.63M) | 1.56M (1.54M–1.60M) | 876k (854k–918k) | 885k (852k–916k) | 80% | 56 |
| 1 | 1000 | 1000 | 6 | 2.40M (2.39M–2.41M) | 2.28M (1.36M–2.30M) | 1.61M (1.61M–1.62M) | 852k (643k–864k) | 59% | 207 |
| 1 | 1000 | 1000 | 8 | 3.08M (2.80M–3.17M) | 2.99M (2.93M–3.02M) | 1.64M (1.54M–1.70M) | 834k (808k–858k) | 48% | 232 |
| 10 | 0 | 50 | 1 | 705k (693k–707k) | 701k (691k–705k) | 701k (686k–706k) | 702k (698k–706k) | 99% | 0 |
| 10 | 0 | 50 | 2 | 1.33M (1.31M–1.34M) | 1.35M (1.31M–1.36M) | 1.34M (1.31M–1.35M) | 1.36M (1.31M–1.36M) | 99% | 0 |
| 10 | 0 | 50 | 3 | 1.92M (1.91M–1.93M) | 1.91M (1.85M–1.93M) | 2.00M (1.94M–2.01M) | 2.00M (1.94M–2.01M) | 100% | 0 |
| 10 | 0 | 50 | 4 | 2.48M (2.40M–2.49M) | 2.44M (2.38M–2.45M) | 2.60M (2.49M–2.61M) | 2.57M (2.49M–2.61M) | 99% | 0 |
| 10 | 0 | 50 | 6 | 3.32M (3.26M–3.34M) | 3.18M (2.71M–3.22M) | 3.32M (3.31M–3.33M) | 3.64M (3.62M–3.68M) | 66% | 46 |
| 10 | 0 | 50 | 8 | 4.15M (4.04M–4.16M) | 4.00M (3.91M–4.04M) | 4.07M (4.06M–4.12M) | 4.26M (4.11M–4.43M) | 50% | 24 |
| 10 | 0 | 200 | 1 | 646k (641k–648k) | 645k (640k–646k) | 644k (641k–645k) | 644k (641k–648k) | 100% | 0 |
| 10 | 0 | 200 | 2 | 1.20M (1.19M–1.21M) | 1.23M (1.22M–1.24M) | 1.23M (1.22M–1.24M) | 1.22M (1.20M–1.23M) | 99% | 0 |
| 10 | 0 | 200 | 3 | 1.74M (1.55M–1.74M) | 1.74M (1.49M–1.74M) | 1.83M (1.45M–1.84M) | 1.82M (1.72M–1.83M) | 93% | 47 |
| 10 | 0 | 200 | 4 | 2.22M (2.17M–2.23M) | 2.20M (2.09M–2.21M) | 2.39M (2.33M–2.40M) | 2.37M (2.31M–2.40M) | 94% | 8 |
| 10 | 0 | 200 | 6 | 2.92M (1.66M–3.00M) | 2.89M (2.85M–2.93M) | 2.94M (1.11M–2.97M) | 2.73M (2.53M–2.77M) | 62% | 294 |
| 10 | 0 | 200 | 8 | 3.64M (3.09M–3.75M) | 3.61M (3.55M–3.74M) | 3.58M (3.57M–3.64M) | 2.72M (2.71M–2.74M) | 48% | 217 |
| 10 | 0 | 1000 | 1 | 427k (423k–427k) | 426k (425k–426k) | 426k (422k–427k) | 426k (424k–427k) | 100% | 0 |
| 10 | 0 | 1000 | 2 | 764k (760k–767k) | 820k (809k–826k) | 824k (817k–827k) | 825k (816k–828k) | 100% | 4 |
| 10 | 0 | 1000 | 3 | 1.08M (1.07M–1.08M) | 1.08M (918k–1.08M) | 915k (911k–917k) | 915k (910k–917k) | 99% | 19 |
| 10 | 0 | 1000 | 4 | 1.36M (1.34M–1.37M) | 1.35M (1.14M–1.36M) | 907k (906k–914k) | 912k (906k–914k) | 97% | 14 |
| 10 | 0 | 1000 | 6 | 1.81M (1.79M–1.82M) | 1.71M (1.67M–1.76M) | 1.33M (1.30M–1.35M) | 834k (819k–846k) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 2.23M (2.18M–2.24M) | 2.10M (2.08M–2.13M) | 1.42M (1.39M–1.45M) | 828k (821k–845k) | 49% | 58 |
| 10 | 1000 | 50 | 1 | 703k (700k–704k) | 702k (691k–704k) | 701k (695k–701k) | 702k (696k–703k) | 100% | 0 |
| 10 | 1000 | 50 | 2 | 1.18M (1.16M–1.18M) | 1.35M (1.29M–1.36M) | 1.35M (1.29M–1.35M) | 1.35M (1.33M–1.36M) | 99% | 0 |
| 10 | 1000 | 50 | 3 | 1.60M (1.58M–1.60M) | 1.66M (1.60M–1.67M) | 2.00M (1.87M–2.01M) | 2.01M (1.89M–2.01M) | 96% | 2 |
| 10 | 1000 | 50 | 4 | 1.99M (1.95M–1.99M) | 2.06M (1.97M–2.07M) | 2.60M (2.50M–2.63M) | 2.58M (2.28M–2.62M) | 95% | 16 |
| 10 | 1000 | 50 | 6 | 2.61M (2.55M–2.64M) | 2.60M (1.73M–2.63M) | 2.69M (2.62M–2.78M) | 3.64M (3.63M–3.67M) | 65% | 69 |
| 10 | 1000 | 50 | 8 | 3.15M (2.95M–3.18M) | 3.18M (3.14M–3.35M) | 3.45M (3.25M–3.46M) | 4.20M (3.80M–4.35M) | 48% | 122 |
| 10 | 1000 | 200 | 1 | 646k (637k–650k) | 643k (634k–647k) | 643k (641k–645k) | 645k (637k–648k) | 100% | 0 |
| 10 | 1000 | 200 | 2 | 1.08M (1.07M–1.08M) | 1.23M (1.22M–1.24M) | 1.23M (1.22M–1.24M) | 1.24M (1.22M–1.24M) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 1.47M (1.40M–1.47M) | 1.52M (1.49M–1.53M) | 1.83M (1.78M–1.85M) | 1.83M (1.72M–1.84M) | 98% | 7 |
| 10 | 1000 | 200 | 4 | 1.81M (1.55M–1.83M) | 1.90M (1.82M–1.91M) | 2.38M (1.88M–2.40M) | 2.39M (1.32M–2.41M) | 91% | 61 |
| 10 | 1000 | 200 | 6 | 2.36M (2.30M–2.42M) | 2.35M (2.30M–2.40M) | 2.50M (2.13M–2.55M) | 2.57M (2.51M–2.70M) | 62% | 92 |
| 10 | 1000 | 200 | 8 | 2.85M (85k–2.90M) | 2.84M (693k–2.98M) | 3.07M (1.01M–3.14M) | 2.65M (768k–2.73M) | 48% | 627 |
| 10 | 1000 | 1000 | 1 | 426k (420k–427k) | 426k (418k–427k) | 426k (422k–427k) | 427k (424k–427k) | 93% | 1 |
| 10 | 1000 | 1000 | 2 | 709k (700k–712k) | 825k (811k–826k) | 825k (810k–827k) | 825k (782k–826k) | 97% | 2 |
| 10 | 1000 | 1000 | 3 | 965k (237k–971k) | 1.00M (497k–1.00M) | 915k (571k–917k) | 915k (726k–918k) | 93% | 171 |
| 10 | 1000 | 1000 | 4 | 1.20M (1.08M–1.20M) | 1.27M (1.22M–1.27M) | 911k (854k–917k) | 912k (858k–914k) | 93% | 13 |
| 10 | 1000 | 1000 | 6 | 1.54M (1.52M–1.57M) | 1.57M (1.55M–1.61M) | 1.29M (745k–1.33M) | 840k (811k–858k) | 63% | 200 |
| 10 | 1000 | 1000 | 8 | 1.71M (1.27M–1.88M) | 1.91M (1.70M–1.96M) | 1.42M (1.38M–1.46M) | 847k (477k–850k) | 46% | 638 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 100% | 0 |
| 1 | 0 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 0.33 (0.29–0.58) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.29 / 0.37 (0.33–0.37) | 0.12 / 0.29 / 0.37 (0.33–0.50) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.29 / 0.42 (0.37–0.42) | 0.17 / 0.29 / 0.42 (0.37–0.62) | 0.17 / 0.33 / 0.37 (0.37–0.58) | 99% | 5 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.46 / 0.70 (0.66–0.74) | 0.29 / 0.62 / 0.91 (0.74–0.95) | 0.33 / 0.70 / 0.91 (0.83–1.07) | 67% | 37 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.50 / 1.00 (0.83–1.00) | 0.29 / 0.66 / 1.07 (1.00–4.99) | 0.58 / 1.07 / 1.74 (1.33–8.58) | 50% | 0 |
| 1 | 0 | 200 | 1 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 100% | 0 |
| 1 | 0 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 100% | 0 |
| 1 | 0 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.29 / 0.46 / 0.54 (0.50–0.83) | 0.29 / 0.46 / 0.54 (0.50–0.54) | 99% | 9 |
| 1 | 0 | 200 | 4 | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.29 / 0.42 / 0.58 (0.58–0.66) | 0.29 / 0.46 / 0.66 (0.50–0.74) | 0.29 / 0.46 / 0.66 (0.50–0.70) | 100% | 0 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.70 / 1.00 (0.87–1.04) | 0.37 / 0.74 / 1.15 (1.12–1.25) | 1.04 / 1.36 / 2.72 (1.54–4.86) | 66% | 9 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 1.04 (0.95–1.15) | 0.42 / 0.95 / 1.36 (1.25–1.46) | 1.57 / 2.21 / 9.22 (6.66–10.62) | 49% | 23 |
| 1 | 0 | 1000 | 1 | 1.00 / 1.04 / 1.07 (1.04–8.58) | 1.00 / 1.04 / 1.07 (1.04–7.17) | 1.00 / 1.04 / 1.07 (1.07–5.82) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.07 / 1.25 / 1.33 (1.33–5.57) | 1.07 / 1.25 / 1.33 (1.33–4.00) | 1.07 / 1.25 / 1.36 (1.33–3.52) | 99% | 4 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–6.72) | 1.04 / 1.25 / 2.08 (2.08–6.14) | 1.87 / 2.03 / 5.70 (4.03–6.98) | 1.87 / 2.03 / 5.44 (2.08–6.14) | 98% | 23 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.07 / 1.57 / 2.14 (2.11–6.91) | 2.98 / 3.14 / 9.60 (6.72–11.65) | 2.98 / 3.36 / 11.52 (8.00–11.90) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–3.81) | 1.15 / 1.95 / 2.40 (2.37–2.40) | 3.07 / 3.87 / 6.78 (4.86–8.96) | 5.70 / 6.21 / 13.95 (13.31–17.92) | 66% | 19 |
| 1 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.15 / 2.03 / 2.43 (2.37–2.50) | 3.36 / 4.03 / 10.11 (9.60–10.62) | 8.19 / 8.70 / 18.43 (16.90–22.02) | 49% | 8 |
| 1 | 1000 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 100% | 0 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.29–0.46) | 0.12 / 0.29 / 0.37 (0.33–0.37) | 0.12 / 0.29 / 0.33 (0.33–0.46) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.17 / 0.29 / 0.42 (0.42–0.50) | 0.17 / 0.33 / 0.42 (0.37–0.62) | 0.17 / 0.33 / 0.54 (0.42–0.58) | 93% | 23 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.42 / 0.66 (0.58–0.66) | 0.25 / 0.54 / 0.70 (0.70–0.87) | 0.29 / 0.54 / 0.74 (0.70–215.04) | 60% | 86 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.42 / 0.70 (0.62–1.00) | 0.25 / 0.54 / 0.79 (0.79–0.87) | 0.42 / 0.66 / 5.06 (1.70–16.64) | 47% | 800 |
| 1 | 1000 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 96% | 4 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.29 / 0.42 / 0.50 (0.46–0.66) | 0.29 / 0.42 / 0.58 (0.50–0.66) | 0.29 / 0.42 / 0.50 (0.46–0.66) | 92% | 8 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.29 / 0.42 / 0.58 (0.50–0.66) | 0.29 / 0.42 / 0.54 (0.50–0.70) | 0.29 / 0.42 / 0.54 (0.50–0.70) | 91% | 60 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.29 / 0.58 / 0.70 (0.58–0.87) | 0.37 / 0.62 / 0.87 (0.54–4.86) | 0.37 / 0.62 / 0.74 (0.54–0.78) | 79% | 116 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.87 (0.79–1.04) | 0.37 / 0.74 / 1.15 (1.07–1.28) | 0.83 / 1.04 / 2.62 (1.41–5.76) | 59% | 216 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 1.00 (0.79–1.41) | 0.37 / 0.87 / 1.20 (1.15–6.72) | 1.46 / 1.82 / 8.96 (8.58–22.27) | 46% | 587 |
| 1 | 1000 | 1000 | 1 | 1.00 / 1.04 / 1.04 (1.04–1.12) | 1.00 / 1.07 / 1.12 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.07–8.83) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 88% | 4 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.07 / 1.12 (1.04–7.49) | 1.07 / 1.28 / 1.78 (1.36–8.45) | 1.07 / 1.28 / 2.08 (1.33–6.34) | 1.07 / 1.25 / 1.70 (1.33–14.46) | 85% | 40 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.07–1.12) | 1.04 / 1.46 / 2.30 (2.11–5.89) | 1.87 / 2.21 / 9.98 (6.08–15.10) | 1.90 / 2.37 / 8.70 (2.21–30.46) | 86% | 121 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.07 / 1.12 (1.04–7.17) | 1.12 / 1.95 / 2.24 (2.14–8.70) | 3.20 / 3.49 / 10.24 (9.22–16.64) | 3.14 / 3.46 / 11.26 (9.34–16.00) | 80% | 56 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–5.82) | 1.12 / 2.08 / 2.40 (2.37–8.96) | 2.69 / 3.78 / 8.06 (7.87–12.93) | 5.57 / 6.66 / 16.00 (13.31–105.47) | 59% | 207 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–5.38) | 1.15 / 2.11 / 4.86 (2.53–12.29) | 3.46 / 4.16 / 12.42 (7.81–49.15) | 7.94 / 10.75 / 21.50 (17.41–90.11) | 48% | 232 |
| 10 | 0 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 99% | 0 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.12 / 0.25 / 0.33 (0.33–2.53) | 0.12 / 0.29 / 0.33 (0.33–0.50) | 0.12 / 0.25 / 0.33 (0.33–3.39) | 99% | 0 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.12 / 0.29 / 0.42 (0.42–0.91) | 0.12 / 0.29 / 0.42 (0.37–5.44) | 0.12 / 0.29 / 0.42 (0.37–5.44) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.29) | 0.17 / 0.37 / 0.50 (0.50–0.66) | 0.17 / 0.33 / 0.54 (0.42–7.36) | 0.17 / 0.33 / 0.62 (0.42–6.53) | 99% | 0 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.62 / 0.87 (0.83–0.91) | 0.25 / 0.70 / 1.00 (1.00–1.12) | 0.29 / 0.62 / 0.91 (0.83–5.44) | 66% | 46 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.70 / 1.04 (0.87–1.07) | 0.29 / 0.87 / 1.25 (1.20–1.28) | 0.50 / 1.07 / 3.81 (1.54–6.08) | 50% | 24 |
| 10 | 0 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 100% | 0 |
| 10 | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.29 / 0.42 / 0.50 (0.50–0.58) | 0.29 / 0.42 / 0.54 (0.50–0.62) | 0.29 / 0.46 / 0.54 (0.50–0.74) | 99% | 0 |
| 10 | 0 | 200 | 3 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.25 / 0.50 / 0.66 (0.66–0.74) | 0.29 / 0.46 / 0.58 (0.50–4.03) | 0.29 / 0.42 / 0.54 (0.50–3.10) | 93% | 47 |
| 10 | 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.29 / 0.58 / 0.74 (0.70–2.37) | 0.29 / 0.46 / 0.70 (0.54–3.14) | 0.33 / 0.46 / 0.70 (0.54–1.28) | 94% | 8 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.83 / 1.15 (1.07–1.20) | 0.37 / 1.04 / 1.41 (1.36–6.66) | 0.83 / 1.15 / 8.19 (1.66–9.60) | 62% | 294 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.91 / 1.28 (1.15–1.36) | 0.46 / 1.25 / 1.78 (1.66–6.40) | 1.54 / 2.24 / 9.09 (6.53–21.76) | 48% | 217 |
| 10 | 0 | 1000 | 1 | 1.00 / 1.04 / 1.04 (1.04–5.44) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.07–5.95) | 1.00 / 1.04 / 1.07 (1.04–2.40) | 100% | 0 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.07 / 1.25 / 1.36 (1.33–6.53) | 1.07 / 1.25 / 1.87 (1.33–6.59) | 1.07 / 1.25 / 1.36 (1.33–8.19) | 100% | 4 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.04 / 2.08 / 2.24 (2.24–10.11) | 1.90 / 2.08 / 5.82 (4.67–8.58) | 1.90 / 2.08 / 5.63 (5.12–9.60) | 99% | 19 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.04 / 1.07 (1.04–6.21) | 1.07 / 2.11 / 3.20 (2.27–6.14) | 2.98 / 3.46 / 10.62 (8.45–15.74) | 2.98 / 3.36 / 10.88 (8.32–15.10) | 97% | 14 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.20 / 2.37 / 2.85 (2.69–5.18) | 2.27 / 4.42 / 7.87 (5.25–9.22) | 5.70 / 6.34 / 14.08 (10.62–18.94) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.20 / 2.40 / 3.10 (2.85–3.52) | 3.23 / 4.54 / 10.11 (7.81–11.65) | 8.06 / 8.96 / 19.46 (18.43–34.30) | 49% | 58 |
| 10 | 1000 | 50 | 1 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 100% | 0 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.33–0.46) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 99% | 0 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.29 / 0.37 (0.37–0.58) | 0.12 / 0.29 / 0.37 (0.37–0.62) | 0.12 / 0.29 / 0.37 (0.33–0.62) | 96% | 2 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.17 / 0.33 / 0.50 (0.46–0.62) | 0.17 / 0.33 / 0.50 (0.42–0.62) | 0.17 / 0.37 / 0.50 (0.42–0.62) | 95% | 16 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.58 / 0.87 (0.78–1.00) | 0.25 / 0.70 / 1.00 (0.91–1.25) | 0.29 / 0.66 / 0.91 (0.78–5.63) | 65% | 69 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.70 / 1.04 (0.79–1.07) | 0.25 / 0.78 / 1.12 (1.07–4.10) | 0.50 / 1.12 / 2.82 (1.66–8.70) | 48% | 122 |
| 10 | 1000 | 200 | 1 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 100% | 0 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.29 / 0.42 / 0.50 (0.50–0.62) | 0.29 / 0.46 / 0.50 (0.50–0.58) | 0.29 / 0.42 / 0.50 (0.50–0.54) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.25 / 0.50 / 0.62 (0.62–0.95) | 0.29 / 0.46 / 0.54 (0.54–3.68) | 0.29 / 0.46 / 0.54 (0.54–6.34) | 98% | 7 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.29 / 0.58 / 0.70 (0.70–2.40) | 0.29 / 0.46 / 0.66 (0.54–1.04) | 0.29 / 0.50 / 0.70 (0.62–8.06) | 91% | 61 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 0.87 / 1.25 (1.15–4.10) | 0.42 / 1.07 / 1.49 (1.41–6.21) | 1.00 / 1.46 / 4.48 (1.82–14.46) | 62% | 92 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–4.54) | 0.33 / 0.91 / 1.25 (1.12–1.57) | 0.42 / 1.15 / 1.70 (1.62–14.85) | 1.57 / 2.27 / 9.09 (8.32–729.09) | 48% | 627 |
| 10 | 1000 | 1000 | 1 | 1.00 / 1.04 / 1.12 (1.04–7.17) | 1.00 / 1.04 / 1.07 (1.04–8.58) | 1.00 / 1.04 / 1.07 (1.07–4.03) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 93% | 1 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.07 / 1.25 / 1.36 (1.33–1.62) | 1.07 / 1.25 / 1.33 (1.33–1.87) | 1.07 / 1.25 / 1.36 (1.33–1.87) | 97% | 2 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.04–21.50) | 1.04 / 2.11 / 2.24 (2.24–46.08) | 1.90 / 2.03 / 6.72 (3.68–65.54) | 1.90 / 2.08 / 5.95 (2.14–24.83) | 93% | 171 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.07 / 2.11 / 2.30 (2.24–2.53) | 2.98 / 3.36 / 11.26 (4.42–13.06) | 2.98 / 3.26 / 10.62 (8.45–11.26) | 93% | 13 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.20 / 2.40 / 2.85 (2.66–2.91) | 2.24 / 4.29 / 7.62 (4.99–48.64) | 5.70 / 6.27 / 15.49 (12.67–19.46) | 63% | 200 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–7.04) | 1.15 / 2.40 / 3.07 (2.98–9.60) | 3.20 / 4.42 / 10.88 (8.32–11.90) | 8.00 / 8.96 / 19.46 (16.38–2162.69) | 46% | 638 |

Change latency (derive/delete), µs

| change % | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 1 | 0.12 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.12 / 0.17 / 0.17 (0.17–0.21) | 0.12 / 0.17 / 0.17 (0.17–0.25) | 100% | 0 |
| 1 | 0 | 50 | 2 | 0.58 / 0.83 / 3.49 (0.87–6.66) | 0.21 / 0.33 / 0.37 (0.37–0.42) | 0.21 / 0.29 / 0.37 (0.37–0.42) | 0.21 / 0.33 / 0.42 (0.37–3.52) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.79 / 1.15 / 15.74 (6.46–21.50) | 0.74 / 1.04 / 3.87 (1.12–7.49) | 0.25 / 0.37 / 0.42 (0.42–0.70) | 0.25 / 0.37 / 0.54 (0.42–1.20) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.91 / 1.49 / 16.38 (7.04–165.89) | 0.87 / 1.79 / 4.35 (2.53–6.40) | 0.25 / 0.37 / 0.50 (0.46–0.87) | 0.25 / 0.37 / 0.54 (0.46–1.15) | 99% | 5 |
| 1 | 0 | 50 | 6 | 1.58 / 2.66 / 311.30 (7.74–520.19) | 1.82 / 3.36 / 4.42 (4.06–6.91) | 1.66 / 2.82 / 3.78 (2.91–10.75) | 0.46 / 0.87 / 1.15 (0.79–1.33) | 67% | 37 |
| 1 | 0 | 50 | 8 | 1.79 / 3.52 / 10.62 (7.81–70.66) | 2.37 / 4.16 / 6.40 (5.82–8.83) | 1.87 / 3.36 / 9.73 (4.22–19.20) | 0.70 / 1.25 / 1.57 (1.36–8.45) | 50% | 0 |
| 1 | 0 | 200 | 1 | 0.25 / 0.29 / 0.29 (0.29–0.37) | 0.25 / 0.29 / 0.37 (0.29–15.74) | 0.25 / 0.29 / 0.29 (0.29–1.49) | 0.25 / 0.29 / 0.33 (0.29–0.37) | 100% | 0 |
| 1 | 0 | 200 | 2 | 0.83 / 1.15 / 4.29 (1.28–10.62) | 0.33 / 0.46 / 0.58 (0.54–0.83) | 0.33 / 0.46 / 0.54 (0.54–7.04) | 0.33 / 0.46 / 0.62 (0.54–2.72) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.04 / 1.49 / 5.57 (1.70–9.09) | 1.04 / 1.41 / 4.16 (2.30–23.55) | 0.37 / 0.54 / 0.62 (0.54–10.88) | 0.37 / 0.50 / 0.62 (0.54–5.25) | 99% | 9 |
| 1 | 0 | 200 | 4 | 1.15 / 2.50 / 17.15 (14.46–24.58) | 1.20 / 1.74 / 4.48 (2.56–15.36) | 0.37 / 0.54 / 0.87 (0.58–2.11) | 0.37 / 0.50 / 0.70 (0.58–1.12) | 100% | 0 |
| 1 | 0 | 200 | 6 | 1.90 / 3.14 / 11.39 (8.32–90.11) | 2.24 / 3.87 / 7.10 (6.91–9.34) | 1.82 / 3.04 / 8.83 (4.00–9.73) | 1.12 / 1.49 / 2.50 (1.57–5.06) | 66% | 9 |
| 1 | 0 | 200 | 8 | 2.11 / 3.94 / 14.98 (9.98–266.24) | 2.43 / 4.16 / 10.50 (7.62–11.01) | 2.24 / 4.00 / 10.50 (4.86–19.46) | 1.66 / 2.37 / 10.75 (10.62–11.65) | 49% | 23 |
| 1 | 0 | 1000 | 1 | 1.04 / 1.07 / 1.12 (1.07–12.93) | 1.04 / 1.07 / 6.78 (1.07–15.62) | 1.04 / 1.07 / 1.12 (1.07–7.36) | 1.04 / 1.07 / 1.15 (1.07–5.95) | 99% | 0 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.55 / 10.11 (5.82–23.30) | 1.12 / 1.28 / 1.82 (1.33–4.29) | 1.12 / 1.28 / 1.41 (1.33–1.41) | 1.12 / 1.28 / 1.36 (1.33–2.08) | 99% | 4 |
| 1 | 0 | 1000 | 3 | 2.91 / 4.61 / 10.62 (7.23–3670.02) | 2.72 / 3.74 / 13.06 (8.83–15.49) | 1.95 / 2.11 / 2.50 (2.11–10.50) | 1.95 / 2.11 / 6.91 (2.14–12.67) | 98% | 23 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.80 / 14.34 (5.12–29.44) | 3.39 / 4.86 / 13.18 (5.95–21.76) | 3.04 / 3.20 / 4.03 (3.23–17.15) | 3.07 / 3.49 / 5.70 (3.23–20.22) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 4.10 / 6.02 / 68.61 (11.90–1458.18) | 4.86 / 7.42 / 28.42 (8.83–202.75) | 7.42 / 10.24 / 18.94 (14.34–446.46) | 5.76 / 6.27 / 11.65 (7.36–19.20) | 66% | 19 |
| 1 | 0 | 1000 | 8 | 4.48 / 7.23 / 16.38 (13.44–88.06) | 5.06 / 8.32 / 16.38 (12.29–120.83) | 10.37 / 16.00 / 30.21 (23.04–42.50) | 8.32 / 8.83 / 17.66 (12.80–18.94) | 49% | 8 |
| 1 | 1000 | 50 | 1 | 0.12 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.21 (0.17–0.25) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.12 / 0.17 / 0.17 (0.17–0.21) | 100% | 0 |
| 1 | 1000 | 50 | 2 | 1.57 / 1.79 / 1.90 (1.90–3.10) | 0.21 / 0.33 / 0.42 (0.37–0.42) | 0.21 / 0.29 / 0.37 (0.37–0.37) | 0.21 / 0.29 / 0.37 (0.37–0.42) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.72 / 3.10 / 10.37 (6.08–16.38) | 1.74 / 2.08 / 8.19 (2.14–11.65) | 0.25 / 0.37 / 0.46 (0.42–0.62) | 0.25 / 0.37 / 0.46 (0.42–7.23) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 4.22 / 14.46 (9.34–573.44) | 1.87 / 2.85 / 4.29 (3.14–8.00) | 0.25 / 0.42 / 0.74 (0.46–4.35) | 0.25 / 0.46 / 0.79 (0.70–1.70) | 93% | 23 |
| 1 | 1000 | 50 | 6 | 3.49 / 4.54 / 15.87 (10.37–251.90) | 3.52 / 4.29 / 8.19 (5.31–10.75) | 2.37 / 3.07 / 8.06 (5.38–11.14) | 0.37 / 0.70 / 0.87 (0.83–9961.47) | 60% | 86 |
| 1 | 1000 | 50 | 8 | 3.74 / 9.98 / 471.04 (14.72–19922.94) | 3.74 / 5.82 / 3899.39 (11.26–15073.28) | 2.50 / 3.74 / 10.24 (5.25–1409.02) | 0.50 / 0.79 / 1.62 (0.95–8.32) | 47% | 800 |
| 1 | 1000 | 200 | 1 | 0.25 / 0.29 / 0.33 (0.29–0.87) | 0.25 / 0.29 / 0.46 (0.33–8.19) | 0.25 / 0.29 / 0.42 (0.37–11.90) | 0.25 / 0.29 / 0.37 (0.29–2.11) | 96% | 4 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.24 / 6.91 (2.21–372.74) | 0.33 / 0.46 / 0.58 (0.54–0.91) | 0.37 / 0.50 / 0.83 (0.54–1.00) | 0.33 / 0.46 / 0.58 (0.50–0.79) | 92% | 8 |
| 1 | 1000 | 200 | 3 | 3.07 / 4.03 / 8.00 (6.08–5308.42) | 2.03 / 2.53 / 9.47 (6.78–7274.50) | 0.37 / 0.50 / 0.83 (0.58–2.00) | 0.37 / 0.50 / 0.83 (0.62–2.43) | 91% | 60 |
| 1 | 1000 | 200 | 4 | 3.49 / 4.61 / 19.97 (16.90–8323.07) | 2.37 / 3.36 / 11.52 (3.68–1769.47) | 0.46 / 0.74 / 2.00 (0.58–1916.93) | 0.42 / 0.74 / 0.87 (0.58–0.87) | 79% | 116 |
| 1 | 1000 | 200 | 6 | 3.81 / 6.91 / 21.76 (10.88–4521.98) | 3.78 / 4.86 / 12.16 (7.62–25690.11) | 2.62 / 3.94 / 65.02 (9.47–3571.71) | 0.91 / 1.15 / 2.27 (1.46–2.78) | 59% | 216 |
| 1 | 1000 | 200 | 8 | 4.00 / 15.49 / 1196.03 (16.38–4653.06) | 4.16 / 5.57 / 311.30 (10.62–22020.10) | 2.78 / 6.27 / 311.30 (10.62–8192.00) | 1.54 / 2.21 / 9.73 (7.10–16.38) | 46% | 587 |
| 1 | 1000 | 1000 | 1 | 1.04 / 1.07 / 1.15 (1.07–4.00) | 1.04 / 1.07 / 1.62 (1.07–3.94) | 1.04 / 1.12 / 1.58 (1.15–9.34) | 1.04 / 1.07 / 1.25 (1.12–2.03) | 88% | 4 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.61 / 9.98 (4.86–1310.72) | 1.15 / 1.49 / 2.30 (1.36–5.70) | 1.12 / 1.36 / 1.82 (1.36–6.27) | 1.12 / 1.28 / 1.87 (1.28–24.32) | 85% | 40 |
| 1 | 1000 | 1000 | 3 | 4.74 / 6.85 / 18.94 (11.14–6094.85) | 3.65 / 6.46 / 18.43 (7.62–7274.50) | 1.95 / 2.37 / 8.83 (3.62–19.97) | 2.00 / 2.50 / 8.45 (2.11–29.70) | 86% | 121 |
| 1 | 1000 | 1000 | 4 | 5.44 / 11.90 / 50.18 (11.52–8650.75) | 4.29 / 6.21 / 15.23 (12.03–21.76) | 3.26 / 3.62 / 12.16 (3.26–26.11) | 3.23 / 3.58 / 9.22 (8.70–19.46) | 80% | 56 |
| 1 | 1000 | 1000 | 6 | 6.02 / 7.94 / 45.06 (15.62–851.97) | 5.70 / 12.29 / 153.60 (9.86–7667.71) | 7.04 / 11.14 / 30.21 (19.97–1179.65) | 5.70 / 6.53 / 16.00 (11.90–243.71) | 59% | 207 |
| 1 | 1000 | 1000 | 8 | 6.34 / 14.72 / 2293.76 (16.90–4915.20) | 6.02 / 9.98 / 67.58 (16.64–165.89) | 10.24 / 17.41 / 413.70 (24.58–2719.74) | 8.00 / 8.96 / 22.78 (17.15–78.85) | 48% | 232 |
| 10 | 0 | 50 | 1 | 0.12 / 0.17 / 0.21 (0.17–0.25) | 0.08 / 0.17 / 0.21 (0.17–0.66) | 0.12 / 0.17 / 0.17 (0.17–0.74) | 0.12 / 0.17 / 0.17 (0.17–0.21) | 99% | 0 |
| 10 | 0 | 50 | 2 | 0.54 / 0.79 / 3.10 (0.87–13.31) | 0.21 / 0.33 / 0.42 (0.42–3.49) | 0.21 / 0.33 / 0.42 (0.42–1.66) | 0.21 / 0.33 / 0.42 (0.37–4.99) | 99% | 0 |
| 10 | 0 | 50 | 3 | 0.78 / 1.15 / 9.60 (5.89–14.21) | 0.74 / 1.07 / 4.99 (1.49–17.92) | 0.25 / 0.37 / 0.58 (0.42–6.08) | 0.25 / 0.37 / 0.50 (0.46–7.17) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.91 / 1.49 / 15.87 (4.06–25.34) | 0.91 / 1.70 / 4.00 (2.56–15.23) | 0.25 / 0.42 / 0.70 (0.50–8.19) | 0.25 / 0.46 / 0.70 (0.54–7.04) | 99% | 0 |
| 10 | 0 | 50 | 6 | 1.54 / 3.65 / 10.62 (9.34–17.66) | 1.78 / 3.68 / 6.91 (5.18–32.26) | 1.46 / 2.82 / 5.18 (3.81–7.30) | 0.37 / 0.74 / 1.07 (1.04–6.53) | 66% | 46 |
| 10 | 0 | 50 | 8 | 1.78 / 4.48 / 13.06 (11.14–86.02) | 2.14 / 4.74 / 8.32 (6.27–11.52) | 1.78 / 3.94 / 8.45 (5.44–9.09) | 0.62 / 1.25 / 4.03 (1.74–7.23) | 50% | 24 |
| 10 | 0 | 200 | 1 | 0.25 / 0.29 / 0.33 (0.29–0.33) | 0.25 / 0.29 / 0.29 (0.29–0.37) | 0.25 / 0.29 / 0.29 (0.29–0.46) | 0.25 / 0.29 / 0.29 (0.29–0.37) | 100% | 0 |
| 10 | 0 | 200 | 2 | 0.87 / 1.20 / 3.49 (1.28–10.62) | 0.33 / 0.46 / 0.58 (0.54–0.74) | 0.33 / 0.50 / 0.58 (0.58–0.78) | 0.33 / 0.50 / 0.58 (0.54–1.12) | 99% | 0 |
| 10 | 0 | 200 | 3 | 1.04 / 1.57 / 6.27 (3.90–165.89) | 1.00 / 1.41 / 4.80 (1.74–16.90) | 0.37 / 0.50 / 0.70 (0.58–4.74) | 0.37 / 0.50 / 0.62 (0.54–5.44) | 93% | 47 |
| 10 | 0 | 200 | 4 | 1.20 / 2.03 / 16.64 (10.62–20.74) | 1.20 / 2.00 / 4.42 (2.56–12.29) | 0.37 / 0.54 / 0.87 (0.62–4.22) | 0.37 / 0.58 / 0.87 (0.58–1.95) | 94% | 8 |
| 10 | 0 | 200 | 6 | 1.87 / 4.42 / 11.01 (6.53–4030.46) | 2.08 / 4.42 / 8.00 (6.08–15.87) | 1.87 / 3.62 / 13.70 (5.12–4980.74) | 0.91 / 1.25 / 7.36 (1.74–9.86) | 62% | 294 |
| 10 | 0 | 200 | 8 | 2.11 / 8.96 / 69.63 (12.93–1228.80) | 2.53 / 5.57 / 21.50 (8.19–22.78) | 2.30 / 5.25 / 10.50 (6.53–31.49) | 1.62 / 2.37 / 8.83 (7.62–18.18) | 48% | 217 |
| 10 | 0 | 1000 | 1 | 1.04 / 1.07 / 1.12 (1.07–5.70) | 1.04 / 1.07 / 1.12 (1.07–1.20) | 1.04 / 1.07 / 1.20 (1.07–3.26) | 1.04 / 1.07 / 1.07 (1.07–1.82) | 100% | 0 |
| 10 | 0 | 1000 | 2 | 2.40 / 3.52 / 8.45 (6.72–15.62) | 1.12 / 1.33 / 1.74 (1.41–7.04) | 1.12 / 1.33 / 2.11 (1.36–5.44) | 1.12 / 1.28 / 1.41 (1.36–6.98) | 100% | 4 |
| 10 | 0 | 1000 | 3 | 2.72 / 4.61 / 12.29 (8.19–21.50) | 2.72 / 4.29 / 9.98 (7.55–290.82) | 1.95 / 2.11 / 5.63 (4.99–8.83) | 1.95 / 2.11 / 6.08 (2.27–8.58) | 99% | 19 |
| 10 | 0 | 1000 | 4 | 3.26 / 5.44 / 23.04 (9.34–31.74) | 3.49 / 6.46 / 18.43 (12.29–196.61) | 3.07 / 3.52 / 12.42 (6.91–13.31) | 3.07 / 3.49 / 10.75 (7.17–12.42) | 97% | 14 |
| 10 | 0 | 1000 | 6 | 4.35 / 8.19 / 18.18 (13.70–60.93) | 5.25 / 9.60 / 15.10 (12.93–98.30) | 7.10 / 12.29 / 21.50 (16.64–25.09) | 5.82 / 6.46 / 15.62 (10.62–17.41) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 4.86 / 10.11 / 20.99 (15.74–60.42) | 5.82 / 11.52 / 21.25 (17.66–88.06) | 10.11 / 18.18 / 29.70 (24.83–81.92) | 8.19 / 9.09 / 20.99 (18.43–30.72) | 49% | 58 |
| 10 | 1000 | 50 | 1 | 0.12 / 0.17 / 0.17 (0.17–0.25) | 0.08 / 0.17 / 0.21 (0.17–0.62) | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.12 / 0.17 / 0.17 (0.17–0.21) | 100% | 0 |
| 10 | 1000 | 50 | 2 | 1.58 / 1.82 / 5.31 (1.90–6.34) | 0.21 / 0.33 / 0.42 (0.37–0.62) | 0.21 / 0.33 / 0.37 (0.37–0.42) | 0.21 / 0.33 / 0.42 (0.37–0.42) | 99% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.14 / 10.88 (8.45–14.46) | 1.70 / 2.11 / 4.99 (2.37–12.16) | 0.25 / 0.37 / 0.46 (0.42–0.74) | 0.25 / 0.37 / 0.46 (0.42–0.78) | 96% | 2 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.46 / 11.01 (8.00–17.92) | 1.87 / 2.69 / 6.91 (4.54–8.45) | 0.25 / 0.46 / 0.58 (0.50–0.74) | 0.25 / 0.54 / 0.66 (0.50–0.78) | 95% | 16 |
| 10 | 1000 | 50 | 6 | 3.46 / 5.12 / 11.14 (9.73–15.10) | 2.94 / 4.80 / 7.23 (6.66–91.14) | 2.27 / 3.65 / 8.70 (4.48–12.67) | 0.42 / 0.79 / 1.07 (0.91–5.25) | 65% | 69 |
| 10 | 1000 | 50 | 8 | 3.68 / 6.40 / 13.82 (11.52–62.46) | 3.62 / 5.63 / 10.37 (7.62–10.75) | 2.40 / 4.54 / 7.42 (6.14–23.55) | 0.62 / 1.28 / 5.31 (1.70–9.09) | 48% | 122 |
| 10 | 1000 | 200 | 1 | 0.25 / 0.29 / 0.29 (0.29–0.37) | 0.25 / 0.29 / 0.33 (0.33–0.33) | 0.25 / 0.29 / 0.33 (0.29–0.33) | 0.25 / 0.29 / 0.29 (0.29–0.42) | 100% | 0 |
| 10 | 1000 | 200 | 2 | 1.82 / 2.14 / 5.12 (3.94–9.09) | 0.33 / 0.46 / 0.58 (0.58–3.87) | 0.33 / 0.50 / 0.58 (0.58–0.70) | 0.33 / 0.46 / 0.54 (0.54–0.58) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.94 / 3.52 / 8.96 (6.66–51.20) | 2.00 / 2.43 / 10.88 (2.62–17.15) | 0.37 / 0.50 / 0.62 (0.58–4.35) | 0.37 / 0.50 / 0.66 (0.58–6.53) | 98% | 7 |
| 10 | 1000 | 200 | 4 | 3.14 / 4.61 / 23.55 (17.15–442.37) | 2.11 / 3.07 / 7.42 (5.82–14.72) | 0.37 / 0.54 / 0.83 (0.62–1.54) | 0.37 / 0.62 / 0.83 (0.79–8.83) | 91% | 61 |
| 10 | 1000 | 200 | 6 | 3.81 / 6.08 / 13.82 (11.52–262.14) | 3.49 / 6.21 / 9.73 (8.83–139.26) | 2.66 / 4.42 / 7.04 (6.66–83.97) | 1.12 / 1.57 / 2.66 (2.11–13.70) | 62% | 92 |
| 10 | 1000 | 200 | 8 | 4.22 / 8.00 / 19.97 (15.74–22020.10) | 3.90 / 7.10 / 16.38 (9.47–15597.57) | 2.82 / 5.82 / 11.90 (8.70–3506.18) | 1.66 / 2.40 / 9.73 (8.32–1040.38) | 48% | 627 |
| 10 | 1000 | 1000 | 1 | 1.04 / 1.07 / 1.12 (1.07–6.34) | 1.04 / 1.07 / 1.12 (1.07–7.87) | 1.04 / 1.07 / 1.12 (1.07–1.79) | 1.04 / 1.07 / 1.12 (1.07–1.49) | 93% | 1 |
| 10 | 1000 | 1000 | 2 | 3.39 / 4.48 / 9.98 (5.44–14.98) | 1.12 / 1.28 / 1.54 (1.36–1.90) | 1.12 / 1.28 / 1.41 (1.36–2.50) | 1.12 / 1.28 / 1.41 (1.36–4.93) | 97% | 2 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.66 / 13.06 (10.50–11534.34) | 3.65 / 5.89 / 8.45 (8.19–3571.71) | 1.95 / 2.11 / 6.91 (6.08–159.74) | 1.95 / 2.11 / 5.70 (2.21–47.10) | 93% | 171 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.30 / 20.48 (10.88–180.22) | 3.81 / 6.72 / 14.59 (9.60–23.55) | 3.07 / 3.36 / 12.03 (3.39–13.57) | 3.07 / 3.39 / 10.62 (9.09–11.65) | 93% | 13 |
| 10 | 1000 | 1000 | 6 | 6.21 / 10.11 / 19.46 (16.64–274.43) | 6.08 / 10.62 / 15.74 (13.06–18.18) | 7.04 / 12.29 / 21.25 (15.74–4521.98) | 5.82 / 6.40 / 16.38 (11.78–19.97) | 63% | 200 |
| 10 | 1000 | 1000 | 8 | 6.78 / 41.47 / 83.97 (19.46–2015.23) | 6.34 / 12.42 / 48.64 (17.15–1015.81) | 9.60 / 18.69 / 29.18 (23.55–153.60) | 8.06 / 9.09 / 18.69 (16.90–4390.91) | 46% | 638 |

Migrations seen: 172510 over 405517544 samples (one sample per measured operation).


### Suite a2: IPC call and reply, k/2 pairs of one process

load average before: { 8.33 6.74 6.26 }
load average after: { 7.03 6.71 6.30 }; 87 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 317k (315k–321k) | 311k (307k–314k) | 312k (310k–315k) | 312k (310k–314k) | 100% | 0 |
| 0 | 50 | 4 | 633k (618k–640k) | 613k (583k–629k) | 587k (380k–603k) | 549k (486k–603k) | 87% | 37 |
| 0 | 50 | 6 | 919k (892k–924k) | 889k (846k–892k) | 785k (740k–805k) | 770k (720k–774k) | 62% | 8 |
| 0 | 50 | 8 | 1.21M (1.20M–1.24M) | 1.12M (1.02M–1.19M) | 971k (906k–1.05M) | 871k (800k–983k) | 46% | 91 |
| 0 | 200 | 2 | 285k (277k–288k) | 279k (265k–282k) | 278k (267k–283k) | 278k (266k–282k) | 98% | 0 |
| 0 | 200 | 4 | 557k (546k–574k) | 536k (532k–564k) | 479k (438k–541k) | 451k (444k–529k) | 87% | 5 |
| 0 | 200 | 6 | 828k (805k–848k) | 786k (783k–794k) | 712k (690k–730k) | 650k (630k–664k) | 60% | 11 |
| 0 | 200 | 8 | 1.08M (1.05M–1.11M) | 1.04M (1.01M–1.07M) | 883k (856k–908k) | 722k (672k–782k) | 46% | 5 |
| 0 | 1000 | 2 | 200k (194k–201k) | 198k (195k–199k) | 198k (196k–198k) | 197k (195k–198k) | 100% | 0 |
| 0 | 1000 | 4 | 391k (383k–401k) | 382k (374k–396k) | 339k (337k–385k) | 347k (334k–385k) | 86% | 5 |
| 0 | 1000 | 6 | 588k (572k–605k) | 559k (544k–587k) | 517k (513k–534k) | 352k (334k–360k) | 61% | 8 |
| 0 | 1000 | 8 | 797k (754k–800k) | 766k (732k–777k) | 640k (404k–691k) | 342k (323k–344k) | 47% | 120 |
| 1000 | 50 | 2 | 195k (191k–195k) | 190k (187k–191k) | 190k (188k–191k) | 190k (185k–191k) | 96% | 1 |
| 1000 | 50 | 4 | 390k (378k–392k) | 378k (362k–380k) | 366k (337k–374k) | 370k (344k–374k) | 90% | 2 |
| 1000 | 50 | 6 | 570k (559k–573k) | 543k (466k–546k) | 525k (509k–531k) | 513k (500k–518k) | 59% | 18 |
| 1000 | 50 | 8 | 757k (755k–770k) | 732k (715k–769k) | 688k (672k–704k) | 663k (653k–683k) | 46% | 22 |
| 1000 | 200 | 2 | 182k (178k–182k) | 178k (175k–179k) | 178k (174k–179k) | 178k (176k–179k) | 100% | 0 |
| 1000 | 200 | 4 | 363k (351k–364k) | 354k (339k–357k) | 345k (319k–347k) | 331k (313k–348k) | 91% | 7 |
| 1000 | 200 | 6 | 532k (487k–540k) | 511k (495k–524k) | 488k (477k–505k) | 472k (463k–484k) | 64% | 21 |
| 1000 | 200 | 8 | 707k (582k–714k) | 691k (662k–703k) | 637k (611k–665k) | 626k (595k–640k) | 47% | 34 |
| 1000 | 1000 | 2 | 142k (141k–143k) | 140k (139k–140k) | 140k (135k–140k) | 141k (139k–141k) | 100% | 0 |
| 1000 | 1000 | 4 | 280k (279k–285k) | 271k (267k–273k) | 264k (254k–266k) | 259k (247k–275k) | 81% | 8 |
| 1000 | 1000 | 6 | 417k (404k–422k) | 395k (377k–402k) | 388k (240k–394k) | 339k (325k–358k) | 58% | 110 |
| 1000 | 1000 | 8 | 558k (461k–565k) | 532k (489k–547k) | 501k (459k–515k) | 328k (307k–347k) | 49% | 159 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 1.87 / 6.91 (1.90–7.49) | 1.82 / 1.95 / 5.70 (2.03–6.34) | 1.82 / 1.95 / 6.08 (2.11–7.87) | 1.82 / 1.95 / 2.11 (2.03–8.96) | 100% | 0 |
| 0 | 50 | 4 | 1.74 / 1.95 / 6.27 (1.95–7.36) | 1.82 / 2.37 / 6.21 (2.72–19.46) | 2.00 / 2.40 / 7.49 (2.40–25.34) | 2.03 / 3.04 / 7.81 (4.29–10.24) | 87% | 37 |
| 0 | 50 | 6 | 1.95 / 2.37 / 4.29 (3.36–10.75) | 1.87 / 2.66 / 6.40 (2.82–19.97) | 2.56 / 3.39 / 6.14 (3.68–10.75) | 2.53 / 3.49 / 6.34 (4.16–13.82) | 62% | 8 |
| 0 | 50 | 8 | 1.87 / 2.37 / 5.12 (3.74–15.10) | 2.08 / 2.78 / 10.88 (5.38–41.98) | 2.78 / 3.62 / 10.50 (7.04–17.92) | 3.20 / 4.99 / 11.01 (10.50–55.30) | 46% | 91 |
| 0 | 200 | 2 | 2.11 / 2.24 / 6.27 (2.43–8.96) | 2.21 / 2.37 / 6.21 (2.40–11.78) | 2.21 / 2.37 / 7.04 (2.53–10.24) | 2.14 / 2.37 / 6.46 (4.42–16.64) | 98% | 0 |
| 0 | 200 | 4 | 2.11 / 2.66 / 6.59 (2.40–12.42) | 2.24 / 2.78 / 6.46 (2.40–11.26) | 2.72 / 3.65 / 9.34 (3.04–20.74) | 3.23 / 3.74 / 8.58 (4.74–12.93) | 87% | 5 |
| 0 | 200 | 6 | 2.24 / 2.72 / 8.06 (3.36–9.98) | 2.40 / 3.04 / 5.70 (3.20–9.60) | 2.85 / 4.06 / 8.19 (5.25–18.18) | 3.33 / 4.16 / 9.34 (5.95–11.78) | 60% | 11 |
| 0 | 200 | 8 | 2.50 / 2.72 / 6.27 (3.33–9.73) | 2.62 / 3.04 / 8.19 (5.25–9.98) | 3.20 / 4.10 / 9.98 (7.55–12.42) | 4.29 / 5.44 / 12.93 (12.16–17.15) | 46% | 5 |
| 0 | 1000 | 2 | 3.62 / 3.68 / 7.87 (4.00–19.46) | 3.68 / 3.81 / 7.36 (3.94–11.65) | 3.68 / 3.78 / 6.78 (3.94–11.65) | 3.68 / 3.81 / 9.86 (7.74–11.52) | 100% | 0 |
| 0 | 1000 | 4 | 3.65 / 4.22 / 7.74 (4.67–17.66) | 3.74 / 4.54 / 8.58 (6.27–17.66) | 4.22 / 6.08 / 9.73 (7.10–20.22) | 4.16 / 5.89 / 12.93 (5.18–19.71) | 86% | 5 |
| 0 | 1000 | 6 | 3.68 / 4.16 / 10.88 (5.31–46.08) | 3.94 / 4.54 / 8.70 (6.14–12.67) | 4.16 / 6.27 / 10.75 (8.32–24.83) | 7.10 / 8.06 / 16.64 (13.57–22.78) | 61% | 8 |
| 0 | 1000 | 8 | 3.65 / 4.16 / 10.24 (8.06–38.40) | 3.87 / 4.54 / 9.73 (8.96–41.47) | 4.42 / 6.40 / 14.08 (12.16–524.29) | 10.11 / 11.65 / 27.14 (20.48–430.08) | 47% | 120 |
| 1000 | 50 | 2 | 3.74 / 3.87 / 9.60 (4.06–14.72) | 3.87 / 4.03 / 7.42 (4.35–10.88) | 3.87 / 4.00 / 8.45 (4.06–12.03) | 3.87 / 4.00 / 10.75 (4.10–22.53) | 96% | 1 |
| 1000 | 50 | 4 | 3.74 / 3.87 / 8.83 (5.63–14.72) | 3.90 / 4.54 / 10.24 (7.62–14.21) | 4.03 / 4.86 / 10.88 (6.91–17.41) | 4.03 / 4.22 / 9.34 (5.12–12.42) | 90% | 2 |
| 1000 | 50 | 6 | 3.90 / 4.42 / 9.60 (6.08–12.42) | 3.94 / 4.86 / 9.22 (5.25–17.41) | 4.29 / 5.12 / 10.75 (6.27–20.74) | 4.48 / 5.06 / 11.26 (9.09–22.27) | 59% | 18 |
| 1000 | 50 | 8 | 3.90 / 4.42 / 10.50 (8.00–14.59) | 3.90 / 4.86 / 8.58 (8.19–20.99) | 4.48 / 5.12 / 11.39 (9.98–12.93) | 4.54 / 5.12 / 12.03 (10.24–54.78) | 46% | 22 |
| 1000 | 200 | 2 | 4.10 / 4.22 / 9.22 (8.32–19.46) | 4.22 / 4.42 / 8.58 (4.48–15.74) | 4.22 / 4.42 / 6.66 (4.48–20.22) | 4.22 / 4.42 / 5.57 (4.42–12.67) | 100% | 0 |
| 1000 | 200 | 4 | 4.10 / 4.29 / 9.22 (5.38–16.64) | 4.22 / 4.93 / 9.47 (4.42–17.92) | 4.42 / 4.67 / 10.62 (5.44–11.52) | 4.42 / 5.12 / 9.73 (5.31–26.37) | 91% | 7 |
| 1000 | 200 | 6 | 4.16 / 4.74 / 7.81 (5.25–43.52) | 4.42 / 5.25 / 11.65 (8.19–17.66) | 4.86 / 5.44 / 11.39 (7.36–19.20) | 4.99 / 5.44 / 11.65 (10.24–13.57) | 64% | 21 |
| 1000 | 200 | 8 | 4.29 / 4.74 / 11.26 (9.73–32.26) | 4.42 / 5.12 / 11.52 (9.47–13.06) | 4.99 / 5.38 / 13.44 (12.54–26.37) | 4.86 / 6.08 / 15.49 (14.72–31.23) | 47% | 34 |
| 1000 | 1000 | 2 | 5.57 / 5.82 / 13.31 (10.37–17.66) | 5.70 / 5.95 / 12.80 (9.22–17.92) | 5.70 / 5.89 / 12.03 (6.85–25.86) | 5.70 / 5.89 / 10.50 (7.62–15.49) | 100% | 0 |
| 1000 | 1000 | 4 | 5.63 / 6.27 / 13.44 (7.74–14.34) | 5.82 / 6.72 / 14.21 (10.11–25.09) | 6.21 / 6.85 / 15.62 (12.03–35.84) | 6.21 / 6.78 / 14.59 (9.09–16.90) | 81% | 8 |
| 1000 | 1000 | 6 | 5.70 / 6.27 / 14.21 (10.50–41.98) | 6.21 / 6.78 / 12.67 (7.81–20.74) | 6.27 / 6.91 / 15.23 (14.59–1228.80) | 7.23 / 8.45 / 18.94 (12.54–55.81) | 58% | 110 |
| 1000 | 1000 | 8 | 5.70 / 6.27 / 15.36 (12.80–96.26) | 5.82 / 6.78 / 16.38 (12.67–172.03) | 6.53 / 7.04 / 17.41 (13.31–107.52) | 10.62 / 12.42 / 25.60 (21.25–565.25) | 49% | 159 |

Migrations seen: 40330 over 35946749 samples (one sample per measured operation).


### Suite a3: k-1 threads signal one notify

load average before: { 7.03 6.71 6.30 }
load average after: { 9.83 8.17 6.92 }; 87 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 704k (699k–710k) | 664k (648k–669k) | 664k (659k–666k) | 665k (662k–666k) | 99% | 0 |
| 0 | 50 | 4 | 2.10M (2.02M–2.10M) | 1.87M (1.67M–1.89M) | 1.95M (1.62M–1.96M) | 1.97M (1.62M–1.98M) | 90% | 17 |
| 0 | 50 | 6 | 3.39M (3.32M–3.41M) | 2.72M (2.57M–2.75M) | 2.37M (2.31M–2.42M) | 2.38M (2.36M–2.51M) | 62% | 18 |
| 0 | 50 | 8 | 4.74M (4.67M–4.75M) | 3.29M (2.72M–4.11M) | 2.72M (2.25M–2.74M) | 2.57M (2.19M–2.58M) | 48% | 97 |
| 0 | 200 | 2 | 656k (644k–662k) | 575k (571k–579k) | 575k (570k–580k) | 573k (572k–576k) | 99% | 0 |
| 0 | 200 | 4 | 1.94M (1.91M–1.97M) | 1.65M (1.51M–1.68M) | 1.60M (1.35M–1.61M) | 1.60M (1.34M–1.62M) | 92% | 9 |
| 0 | 200 | 6 | 3.19M (3.16M–3.20M) | 2.31M (2.00M–2.44M) | 1.76M (1.49M–1.79M) | 1.57M (1.42M–1.59M) | 63% | 6 |
| 0 | 200 | 8 | 4.48M (4.45M–4.48M) | 2.55M (1.72M–2.92M) | 1.93M (1.81M–2.10M) | 1.53M (967k–1.70M) | 47% | 254 |
| 0 | 1000 | 2 | 527k (523k–528k) | 399k (396k–403k) | 401k (398k–402k) | 400k (383k–403k) | 99% | 1 |
| 0 | 1000 | 4 | 1.56M (1.53M–1.57M) | 1.08M (1000k–1.08M) | 667k (609k–674k) | 672k (633k–674k) | 96% | 0 |
| 0 | 1000 | 6 | 1.88M (1.87M–1.88M) | 1.21M (1.18M–1.26M) | 979k (381k–1.07M) | 653k (471k–660k) | 65% | 79 |
| 0 | 1000 | 8 | 1.87M (1.77M–1.88M) | 1.16M (969k–1.24M) | 1.07M (680k–1.13M) | 666k (504k–705k) | 50% | 526 |
| 1000 | 50 | 2 | 707k (702k–711k) | 664k (660k–666k) | 664k (660k–666k) | 663k (662k–668k) | 100% | 0 |
| 1000 | 50 | 4 | 2.11M (2.10M–2.12M) | 1.46M (1.45M–1.46M) | 1.97M (1.96M–1.98M) | 1.98M (1.95M–1.99M) | 100% | 0 |
| 1000 | 50 | 6 | 3.40M (3.37M–3.41M) | 2.48M (2.46M–2.54M) | 1.74M (1.28M–1.76M) | 2.80M (2.06M–2.87M) | 67% | 32 |
| 1000 | 50 | 8 | 4.71M (4.66M–4.75M) | 3.25M (2.87M–3.64M) | 2.70M (2.64M–2.76M) | 2.77M (2.57M–2.96M) | 49% | 61 |
| 1000 | 200 | 2 | 659k (655k–659k) | 574k (573k–577k) | 574k (571k–576k) | 577k (573k–578k) | 100% | 0 |
| 1000 | 200 | 4 | 1.96M (1.95M–1.96M) | 1.37M (1.36M–1.38M) | 1.65M (1.64M–1.66M) | 1.64M (1.64M–1.66M) | 100% | 0 |
| 1000 | 200 | 6 | 3.19M (3.18M–3.20M) | 2.11M (1.03M–2.30M) | 1.57M (1.35M–1.60M) | 1.69M (1.65M–1.77M) | 67% | 74 |
| 1000 | 200 | 8 | 4.45M (4.44M–4.46M) | 2.47M (2.04M–2.98M) | 2.02M (1.27M–2.19M) | 1.56M (1.23M–1.60M) | 50% | 465 |
| 1000 | 1000 | 2 | 527k (520k–528k) | 380k (377k–384k) | 382k (377k–387k) | 377k (370k–384k) | 100% | 0 |
| 1000 | 1000 | 4 | 1.57M (1.54M–1.57M) | 1.06M (984k–1.07M) | 725k (328k–728k) | 722k (336k–728k) | 91% | 68 |
| 1000 | 1000 | 6 | 1.88M (1.63M–1.88M) | 1.32M (1.20M–1.37M) | 978k (670k–1.05M) | 666k (544k–673k) | 60% | 109 |
| 1000 | 1000 | 8 | 1.86M (1.12M–1.87M) | 1.26M (1.13M–1.35M) | 1.16M (145k–1.20M) | 702k (484k–708k) | 48% | 599 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.29) | 0.17 / 0.21 / 0.29 (0.25–0.58) | 0.17 / 0.21 / 0.29 (0.29–0.33) | 0.17 / 0.21 / 0.29 (0.25–0.37) | 99% | 0 |
| 0 | 50 | 4 | 0.08 / 0.17 / 0.25 (0.17–0.33) | 0.21 / 0.42 / 0.58 (0.46–0.66) | 0.21 / 0.37 / 0.46 (0.42–1.00) | 0.17 / 0.33 / 0.50 (0.33–1.04) | 90% | 17 |
| 0 | 50 | 6 | 0.17 / 0.25 / 0.33 (0.29–0.37) | 0.33 / 0.58 / 0.70 (0.66–0.74) | 0.42 / 0.91 / 1.12 (1.12–1.25) | 0.70 / 1.28 / 2.21 (1.28–4.61) | 62% | 18 |
| 0 | 50 | 8 | 0.17 / 0.33 / 0.37 (0.37–0.42) | 0.33 / 0.62 / 71.68 (0.62–73.73) | 0.42 / 1.12 / 1.46 (1.41–1.66) | 1.36 / 2.27 / 7.62 (4.99–40.96) | 48% | 97 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.42) | 0.42 / 0.46 / 0.50 (0.50–0.66) | 0.42 / 0.50 / 0.50 (0.50–0.58) | 0.42 / 0.50 / 0.54 (0.50–0.58) | 99% | 0 |
| 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.33 / 0.66 / 0.87 (0.66–1.04) | 0.50 / 0.66 / 1.58 (1.04–2.62) | 0.50 / 0.66 / 1.00 (0.74–2.00) | 92% | 9 |
| 0 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.46 / 0.70 / 16.64 (1.41–19.46) | 0.50 / 1.70 / 1.95 (1.87–2.37) | 1.82 / 2.62 / 8.00 (5.25–8.96) | 63% | 6 |
| 0 | 200 | 8 | 0.25 / 0.42 / 0.46 (0.46–0.50) | 0.58 / 1.28 / 2.40 (1.36–11.14) | 0.74 / 1.57 / 2.21 (2.08–3.49) | 2.78 / 4.00 / 12.67 (11.01–589.82) | 47% | 254 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.87) | 1.15 / 1.25 / 1.28 (1.25–5.57) | 1.15 / 1.25 / 1.28 (1.25–5.25) | 1.15 / 1.25 / 1.25 (1.25–6.02) | 99% | 1 |
| 0 | 1000 | 4 | 0.58 / 0.66 / 0.79 (0.62–0.95) | 0.66 / 1.70 / 1.79 (1.74–5.12) | 3.23 / 3.78 / 9.09 (7.62–13.70) | 3.20 / 3.49 / 10.24 (4.48–13.57) | 96% | 0 |
| 0 | 1000 | 6 | 0.66 / 0.83 / 139.26 (137.22–141.31) | 1.70 / 2.85 / 7.74 (3.07–10.88) | 0.87 / 3.87 / 4.61 (4.42–6.66) | 6.21 / 7.23 / 13.57 (12.54–249.86) | 65% | 79 |
| 0 | 1000 | 8 | 0.62 / 0.79 / 339.97 (335.87–352.26) | 2.11 / 2.85 / 7.10 (5.44–491.52) | 2.43 / 3.14 / 7.81 (6.40–434.18) | 8.58 / 10.37 / 20.99 (18.94–1032.19) | 50% | 526 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 100% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.25) | 0.21 / 0.33 / 0.37 (0.37–0.42) | 0.17 / 0.33 / 0.46 (0.42–1.07) | 0.17 / 0.37 / 0.54 (0.42–0.83) | 100% | 0 |
| 1000 | 50 | 6 | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.74 (0.66–0.79) | 0.33 / 0.74 / 1.04 (1.00–1.12) | 0.42 / 1.04 / 1.25 (1.00–1.33) | 67% | 32 |
| 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.33–0.42) | 0.33 / 0.62 / 65.54 (14.46–76.80) | 0.37 / 0.87 / 1.25 (1.12–1.33) | 1.15 / 2.08 / 6.34 (3.26–7.10) | 49% | 61 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.42 / 0.50 / 0.54 (0.54–0.79) | 0.42 / 0.50 / 0.54 (0.54–0.66) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.29 / 0.58 / 0.66 (0.66–0.66) | 0.42 / 0.70 / 0.79 (0.74–0.91) | 0.42 / 0.74 / 1.04 (0.78–1.25) | 100% | 0 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.46 / 1.20 / 16.64 (0.83–20.22) | 0.50 / 1.41 / 1.87 (1.79–2.00) | 1.58 / 2.27 / 3.33 (2.53–6.34) | 67% | 74 |
| 1000 | 200 | 8 | 0.25 / 0.42 / 0.50 (0.46–0.50) | 0.50 / 1.25 / 19.71 (3.26–108.54) | 0.58 / 1.33 / 1.90 (1.82–20.74) | 2.91 / 3.68 / 12.03 (9.60–159.74) | 50% | 465 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.83) | 1.15 / 1.62 / 1.66 (1.66–6.21) | 1.15 / 1.62 / 1.70 (1.66–8.13) | 1.33 / 1.62 / 1.70 (1.66–1.70) | 100% | 0 |
| 1000 | 1000 | 4 | 0.58 / 0.62 / 0.70 (0.62–0.79) | 0.70 / 1.70 / 1.79 (1.74–2.21) | 2.78 / 2.91 / 7.42 (2.94–194.56) | 2.78 / 3.46 / 9.22 (3.52–47.10) | 91% | 68 |
| 1000 | 1000 | 6 | 0.62 / 0.74 / 143.36 (139.26–151.55) | 1.62 / 2.69 / 6.21 (4.00–9.47) | 0.83 / 3.46 / 4.48 (4.22–35.84) | 6.02 / 6.66 / 13.82 (12.29–52.74) | 60% | 109 |
| 1000 | 1000 | 8 | 0.66 / 0.79 / 339.97 (282.62–348.16) | 1.57 / 2.62 / 8.83 (2.94–26.62) | 2.14 / 3.04 / 9.09 (7.74–8650.75) | 8.45 / 9.47 / 20.22 (18.43–1196.03) | 48% | 599 |

Migrations seen: 37612 over 117937817 samples (one sample per measured operation).


### Suite b: unmap seen by every core of a k-thread process

load average before: { 9.83 8.17 6.92 }
load average after: { 6.20 7.07 6.71 }; 218 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 500k (476k–501k) | 503k (502k–506k) | 505k (501k–506k) | 505k (502k–505k) | 100% | 0 |
| one | 0 | 50 | 3 | 483k (464k–484k) | 479k (475k–482k) | 488k (486k–489k) | 489k (482k–490k) | 100% | 0 |
| one | 0 | 50 | 4 | 449k (411k–459k) | 381k (343k–392k) | 475k (428k–478k) | 477k (426k–478k) | 96% | 0 |
| one | 0 | 50 | 6 | 374k (348k–377k) | 305k (299k–314k) | 323k (311k–339k) | 424k (417k–427k) | 66% | 7 |
| one | 0 | 50 | 8 | 353k (347k–356k) | 271k (263k–287k) | 252k (248k–254k) | 400k (390k–403k) | 46% | 7 |
| one | 0 | 200 | 2 | 436k (422k–443k) | 479k (479k–480k) | 479k (471k–480k) | 479k (468k–480k) | 100% | 0 |
| one | 0 | 200 | 3 | 431k (410k–432k) | 431k (408k–434k) | 463k (462k–463k) | 462k (459k–464k) | 100% | 0 |
| one | 0 | 200 | 4 | 406k (341k–412k) | 352k (295k–354k) | 449k (410k–452k) | 449k (408k–454k) | 97% | 0 |
| one | 0 | 200 | 6 | 346k (308k–350k) | 275k (228k–282k) | 295k (271k–296k) | 401k (375k–407k) | 63% | 6 |
| one | 0 | 200 | 8 | 317k (263k–332k) | 247k (156k–255k) | 239k (117k–244k) | 382k (60k–384k) | 49% | 93 |
| one | 0 | 1000 | 2 | 259k (250k–260k) | 346k (340k–347k) | 346k (344k–347k) | 346k (286k–347k) | 96% | 7 |
| one | 0 | 1000 | 3 | 251k (190k–256k) | 251k (158k–252k) | 339k (330k–340k) | 338k (317k–340k) | 94% | 20 |
| one | 0 | 1000 | 4 | 245k (232k–250k) | 227k (204k–227k) | 330k (316k–332k) | 330k (315k–333k) | 92% | 3 |
| one | 0 | 1000 | 6 | 217k (206k–219k) | 191k (29k–193k) | 200k (14k–201k) | 304k (73k–306k) | 66% | 84 |
| one | 0 | 1000 | 8 | 213k (10k–214k) | 178k (108k–196k) | 173k (23k–176k) | 292k (100k–294k) | 50% | 100 |
| one | 1000 | 50 | 2 | 332k (303k–333k) | 335k (325k–335k) | 333k (332k–335k) | 334k (332k–336k) | 97% | 0 |
| one | 1000 | 50 | 3 | 324k (292k–325k) | 324k (298k–325k) | 328k (316k–328k) | 327k (310k–329k) | 95% | 2 |
| one | 1000 | 50 | 4 | 306k (285k–312k) | 221k (207k–223k) | 319k (294k–322k) | 320k (92k–323k) | 88% | 19 |
| one | 1000 | 50 | 6 | 266k (111k–270k) | 193k (68k–199k) | 196k (126k–209k) | 290k (18k–294k) | 66% | 83 |
| one | 1000 | 50 | 8 | 256k (13k–258k) | 185k (107k–188k) | 171k (79k–176k) | 244k (115k–285k) | 48% | 115 |
| one | 1000 | 200 | 2 | 298k (236k–306k) | 320k (308k–324k) | 321k (311k–324k) | 323k (321k–324k) | 93% | 10 |
| one | 1000 | 200 | 3 | 290k (81k–301k) | 282k (262k–303k) | 298k (237k–316k) | 302k (296k–316k) | 83% | 26 |
| one | 1000 | 200 | 4 | 265k (255k–290k) | 200k (190k–211k) | 301k (288k–312k) | 308k (291k–310k) | 84% | 2 |
| one | 1000 | 200 | 6 | 249k (237k–253k) | 182k (174k–183k) | 191k (185k–194k) | 284k (206k–285k) | 62% | 12 |
| one | 1000 | 200 | 8 | 238k (220k–243k) | 174k (165k–178k) | 164k (109k–165k) | 273k (257k–276k) | 48% | 25 |
| one | 1000 | 1000 | 2 | 205k (202k–206k) | 256k (256k–258k) | 256k (256k–257k) | 256k (255k–257k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 199k (190k–203k) | 203k (195k–204k) | 252k (239k–254k) | 251k (240k–253k) | 91% | 2 |
| one | 1000 | 1000 | 4 | 183k (91k–196k) | 145k (144k–155k) | 239k (232k–241k) | 236k (230k–243k) | 80% | 25 |
| one | 1000 | 1000 | 6 | 172k (153k–175k) | 134k (131k–141k) | 144k (143k–145k) | 230k (229k–231k) | 59% | 8 |
| one | 1000 | 1000 | 8 | 173k (166k–174k) | 134k (132k–135k) | 125k (121k–130k) | 218k (217k–223k) | 45% | 1 |
| all | 0 | 50 | 2 | 894k (791k–912k) | 845k (836k–853k) | 851k (839k–855k) | 847k (843k–852k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.04M (964k–1.06M) | 939k (922k–951k) | 1.01M (992k–1.03M) | 1.02M (995k–1.03M) | 100% | 0 |
| all | 0 | 50 | 4 | 1.02M (987k–1.17M) | 696k (605k–871k) | 1.01M (1.01M–1.15M) | 1.02M (1.01M–1.15M) | 89% | 32 |
| all | 0 | 50 | 6 | 985k (977k–1.03M) | 679k (668k–709k) | 655k (647k–664k) | 1.12M (1.09M–1.13M) | 63% | 56 |
| all | 0 | 50 | 8 | 924k (896k–941k) | 686k (664k–705k) | 508k (470k–569k) | 1.09M (984k–1.11M) | 47% | 57 |
| all | 0 | 200 | 2 | 801k (690k–810k) | 764k (748k–773k) | 769k (758k–774k) | 765k (756k–770k) | 100% | 0 |
| all | 0 | 200 | 3 | 914k (885k–947k) | 809k (800k–817k) | 898k (884k–901k) | 890k (882k–902k) | 100% | 0 |
| all | 0 | 200 | 4 | 1.01M (839k–1.03M) | 752k (570k–771k) | 984k (888k–989k) | 981k (914k–986k) | 96% | 3 |
| all | 0 | 200 | 6 | 886k (874k–901k) | 625k (606k–648k) | 599k (590k–629k) | 979k (960k–992k) | 69% | 45 |
| all | 0 | 200 | 8 | 839k (809k–856k) | 597k (592k–641k) | 482k (473k–517k) | 985k (974k–992k) | 50% | 29 |
| all | 0 | 1000 | 2 | 448k (375k–466k) | 476k (474k–476k) | 476k (472k–477k) | 476k (475k–477k) | 100% | 0 |
| all | 0 | 1000 | 3 | 526k (454k–533k) | 486k (484k–488k) | 523k (522k–523k) | 522k (506k–524k) | 100% | 0 |
| all | 0 | 1000 | 4 | 557k (548k–561k) | 448k (442k–449k) | 552k (549k–554k) | 549k (548k–551k) | 100% | 0 |
| all | 0 | 1000 | 6 | 514k (506k–516k) | 414k (408k–419k) | 395k (386k–402k) | 527k (520k–534k) | 68% | 47 |
| all | 0 | 1000 | 8 | 501k (485k–506k) | 415k (390k–420k) | 337k (334k–346k) | 541k (518k–556k) | 50% | 83 |
| all | 1000 | 50 | 2 | 458k (442k–459k) | 798k (777k–802k) | 798k (790k–810k) | 807k (788k–813k) | 100% | 0 |
| all | 1000 | 50 | 3 | 630k (602k–643k) | 533k (531k–533k) | 998k (964k–1.02M) | 1.01M (984k–1.02M) | 100% | 0 |
| all | 1000 | 50 | 4 | 693k (575k–715k) | 573k (567k–574k) | 1.10M (1.09M–1.11M) | 1.10M (991k–1.12M) | 98% | 13 |
| all | 1000 | 50 | 6 | 721k (720k–724k) | 578k (570k–612k) | 570k (554k–583k) | 908k (587k–916k) | 65% | 111 |
| all | 1000 | 50 | 8 | 739k (406k–747k) | 607k (568k–623k) | 487k (398k–490k) | 933k (896k–938k) | 48% | 415 |
| all | 1000 | 200 | 2 | 421k (409k–421k) | 763k (761k–768k) | 763k (760k–768k) | 761k (760k–771k) | 100% | 0 |
| all | 1000 | 200 | 3 | 571k (538k–573k) | 516k (510k–524k) | 895k (886k–895k) | 897k (895k–902k) | 100% | 0 |
| all | 1000 | 200 | 4 | 641k (631k–651k) | 551k (548k–554k) | 980k (973k–982k) | 981k (978k–984k) | 100% | 0 |
| all | 1000 | 200 | 6 | 659k (650k–662k) | 549k (534k–611k) | 532k (521k–541k) | 869k (864k–876k) | 65% | 49 |
| all | 1000 | 200 | 8 | 681k (670k–686k) | 561k (558k–590k) | 457k (456k–467k) | 871k (859k–877k) | 48% | 15 |
| all | 1000 | 1000 | 2 | 298k (274k–302k) | 474k (473k–476k) | 473k (472k–476k) | 476k (472k–477k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 386k (375k–399k) | 431k (423k–431k) | 522k (521k–522k) | 523k (519k–525k) | 100% | 0 |
| all | 1000 | 1000 | 4 | 411k (400k–418k) | 388k (336k–394k) | 550k (495k–552k) | 547k (513k–552k) | 96% | 12 |
| all | 1000 | 1000 | 6 | 436k (360k–441k) | 373k (339k–385k) | 365k (345k–392k) | 509k (491k–519k) | 65% | 66 |
| all | 1000 | 1000 | 8 | 450k (447k–452k) | 379k (369k–383k) | 330k (317k–337k) | 501k (479k–508k) | 49% | 15 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.66 / 0.79 / 0.87 (0.87–1.00) | 0.62 / 0.74 / 0.83 (0.79–1.33) | 0.62 / 0.74 / 0.79 (0.79–0.91) | 0.62 / 0.74 / 0.91 (0.79–2.14) | 100% | 0 |
| one | 0 | 50 | 3 | 0.70 / 0.83 / 1.74 (0.95–3.36) | 0.70 / 0.95 / 1.62 (1.00–4.16) | 0.70 / 0.79 / 0.83 (0.83–1.62) | 0.70 / 0.79 / 0.91 (0.83–3.87) | 100% | 0 |
| one | 0 | 50 | 4 | 0.83 / 1.07 / 4.54 (1.15–6.27) | 1.25 / 1.79 / 5.12 (1.62–7.62) | 0.74 / 0.83 / 2.24 (1.74–6.66) | 0.74 / 0.83 / 3.07 (0.95–5.57) | 96% | 0 |
| one | 0 | 50 | 6 | 1.28 / 1.66 / 7.10 (2.30–7.87) | 1.90 / 2.69 / 4.99 (4.29–7.23) | 1.74 / 2.37 / 5.70 (3.55–7.55) | 1.04 / 1.20 / 3.87 (2.62–4.99) | 66% | 7 |
| one | 0 | 50 | 8 | 1.49 / 1.82 / 6.21 (5.31–7.81) | 2.27 / 3.36 / 7.74 (7.30–17.92) | 2.62 / 3.23 / 10.37 (6.66–23.55) | 1.15 / 1.28 / 6.72 (5.38–11.78) | 46% | 7 |
| one | 0 | 200 | 2 | 0.91 / 1.12 / 1.87 (1.12–4.86) | 0.74 / 0.83 / 0.95 (0.91–3.68) | 0.74 / 0.83 / 0.91 (0.91–5.63) | 0.74 / 0.83 / 1.20 (0.87–4.29) | 100% | 0 |
| one | 0 | 200 | 3 | 0.95 / 1.07 / 3.49 (1.20–5.31) | 0.95 / 1.20 / 2.30 (1.28–7.74) | 0.79 / 0.87 / 1.00 (0.95–4.16) | 0.79 / 0.87 / 1.00 (0.95–5.44) | 100% | 0 |
| one | 0 | 200 | 4 | 1.07 / 1.25 / 4.61 (1.36–7.74) | 1.49 / 1.90 / 7.62 (1.87–17.15) | 0.87 / 0.91 / 4.74 (1.00–13.95) | 0.87 / 0.91 / 4.93 (1.00–13.82) | 97% | 0 |
| one | 0 | 200 | 6 | 1.54 / 1.87 / 7.36 (5.12–10.75) | 2.24 / 2.98 / 8.06 (4.86–66.56) | 2.08 / 2.62 / 7.74 (4.67–11.14) | 1.12 / 1.33 / 6.40 (2.40–11.52) | 63% | 6 |
| one | 0 | 200 | 8 | 1.74 / 2.14 / 11.14 (6.14–114.69) | 2.62 / 3.87 / 14.59 (7.62–532.48) | 2.82 / 3.49 / 8.32 (7.68–999.42) | 1.28 / 1.41 / 5.70 (5.25–4653.06) | 49% | 93 |
| one | 0 | 1000 | 2 | 2.50 / 2.62 / 4.48 (2.69–11.39) | 1.54 / 1.62 / 1.70 (1.70–6.40) | 1.54 / 1.62 / 1.82 (1.70–4.93) | 1.54 / 1.62 / 1.74 (1.70–4.06) | 96% | 7 |
| one | 0 | 1000 | 3 | 2.62 / 2.72 / 7.36 (5.44–12.67) | 2.62 / 2.78 / 6.53 (3.90–41.98) | 1.58 / 1.66 / 1.79 (1.74–13.70) | 1.58 / 1.66 / 6.46 (1.74–8.58) | 94% | 20 |
| one | 0 | 1000 | 4 | 2.66 / 2.85 / 9.73 (7.23–11.52) | 3.07 / 3.39 / 9.09 (6.59–10.50) | 1.66 / 1.74 / 6.72 (4.29–7.36) | 1.66 / 1.74 / 6.34 (2.21–9.09) | 92% | 3 |
| one | 0 | 1000 | 6 | 3.23 / 3.68 / 8.19 (6.78–18.18) | 3.81 / 4.74 / 10.75 (7.55–7864.32) | 3.65 / 4.22 / 8.83 (7.30–9961.47) | 1.90 / 2.14 / 5.70 (3.81–3932.16) | 66% | 84 |
| one | 0 | 1000 | 8 | 3.36 / 3.74 / 9.60 (8.96–17301.50) | 4.16 / 5.31 / 10.37 (9.60–1024.00) | 4.42 / 5.12 / 10.88 (9.47–12320.77) | 2.11 / 2.24 / 8.00 (6.27–2523.14) | 50% | 100 |
| one | 1000 | 50 | 2 | 1.62 / 1.79 / 1.87 (1.87–8.58) | 1.62 / 1.74 / 1.82 (1.79–2.91) | 1.62 / 1.74 / 1.87 (1.82–7.68) | 1.62 / 1.74 / 1.82 (1.79–2.37) | 97% | 0 |
| one | 1000 | 50 | 3 | 1.70 / 1.87 / 6.27 (2.08–10.24) | 1.70 / 1.82 / 5.70 (1.95–7.74) | 1.70 / 1.79 / 5.82 (2.03–8.96) | 1.70 / 1.79 / 3.55 (1.87–11.26) | 95% | 2 |
| one | 1000 | 50 | 4 | 1.87 / 2.40 / 8.19 (4.86–10.62) | 3.14 / 3.87 / 9.22 (5.70–12.29) | 1.74 / 2.08 / 7.23 (4.00–29.95) | 1.74 / 2.11 / 8.19 (6.14–1163.26) | 88% | 19 |
| one | 1000 | 50 | 6 | 2.30 / 2.69 / 5.76 (5.25–123.90) | 3.78 / 4.42 / 10.62 (7.74–1851.39) | 3.49 / 4.10 / 9.34 (7.49–290.82) | 2.03 / 2.24 / 8.45 (6.08–10878.98) | 66% | 83 |
| one | 1000 | 50 | 8 | 2.56 / 2.98 / 10.11 (8.45–8781.82) | 4.03 / 4.67 / 10.88 (10.50–786.43) | 4.16 / 4.80 / 11.14 (10.11–925.70) | 2.21 / 2.37 / 10.75 (7.87–262.14) | 48% | 115 |
| one | 1000 | 200 | 2 | 1.90 / 2.14 / 6.34 (2.69–13.95) | 1.74 / 1.95 / 4.16 (2.00–7.49) | 1.74 / 1.95 / 2.91 (1.95–6.53) | 1.74 / 1.87 / 3.49 (1.90–6.40) | 93% | 10 |
| one | 1000 | 200 | 3 | 2.08 / 2.24 / 6.21 (4.48–3276.80) | 1.95 / 2.56 / 6.21 (2.85–9.47) | 1.95 / 2.14 / 8.45 (4.54–18.69) | 2.00 / 2.14 / 5.44 (2.00–7.36) | 83% | 26 |
| one | 1000 | 200 | 4 | 2.43 / 2.66 / 8.13 (6.27–8.83) | 3.58 / 4.16 / 8.96 (7.17–9.34) | 1.87 / 2.24 / 6.53 (4.42–10.37) | 1.87 / 2.00 / 5.38 (2.03–9.47) | 84% | 2 |
| one | 1000 | 200 | 6 | 2.66 / 2.94 / 8.70 (6.21–10.62) | 4.10 / 4.61 / 11.01 (8.58–13.06) | 3.87 / 4.42 / 8.83 (7.62–10.88) | 2.21 / 2.37 / 7.23 (6.40–159.74) | 62% | 12 |
| one | 1000 | 200 | 8 | 2.78 / 3.23 / 10.50 (8.58–91.14) | 4.29 / 5.44 / 14.98 (13.57–24.83) | 4.42 / 5.06 / 12.29 (11.52–237.57) | 2.30 / 2.43 / 9.34 (7.68–13.06) | 48% | 25 |
| one | 1000 | 1000 | 2 | 3.49 / 3.62 / 7.23 (3.87–8.45) | 2.53 / 2.62 / 3.36 (2.72–9.73) | 2.53 / 2.62 / 6.78 (2.69–7.94) | 2.53 / 2.62 / 7.23 (2.85–9.73) | 100% | 0 |
| one | 1000 | 1000 | 3 | 3.62 / 3.81 / 9.98 (8.06–12.54) | 3.52 / 3.74 / 7.74 (6.98–10.75) | 2.56 / 2.69 / 7.68 (4.06–9.73) | 2.56 / 2.98 / 6.21 (5.70–9.34) | 91% | 2 |
| one | 1000 | 1000 | 4 | 4.03 / 4.35 / 12.93 (7.23–1474.56) | 5.44 / 6.27 / 11.26 (8.70–19.71) | 2.78 / 3.14 / 8.83 (6.98–19.20) | 2.85 / 3.23 / 8.58 (5.12–17.66) | 80% | 25 |
| one | 1000 | 1000 | 6 | 4.42 / 4.93 / 14.08 (12.03–19.97) | 6.08 / 6.78 / 13.82 (12.42–19.46) | 5.57 / 6.34 / 13.06 (10.50–19.71) | 3.04 / 3.20 / 9.98 (8.58–10.37) | 59% | 8 |
| one | 1000 | 1000 | 8 | 4.42 / 4.86 / 13.57 (12.67–14.08) | 6.08 / 6.91 / 15.62 (14.08–21.76) | 6.21 / 7.36 / 14.98 (14.46–20.22) | 3.14 / 3.49 / 14.85 (14.08–31.74) | 45% | 1 |
| all | 0 | 50 | 2 | 0.74 / 1.07 / 1.15 (1.15–1.46) | 1.00 / 1.15 / 1.28 (1.25–7.68) | 1.00 / 1.15 / 1.25 (1.20–3.74) | 1.00 / 1.15 / 1.25 (1.25–5.82) | 100% | 0 |
| all | 0 | 50 | 3 | 1.04 / 1.66 / 2.94 (1.90–7.23) | 1.33 / 2.27 / 5.95 (2.91–8.83) | 1.62 / 1.74 / 6.27 (2.21–7.30) | 1.57 / 1.74 / 2.50 (1.82–5.76) | 100% | 0 |
| all | 0 | 50 | 4 | 1.54 / 2.69 / 4.93 (4.42–7.42) | 3.46 / 5.70 / 8.83 (6.53–155.65) | 2.69 / 3.07 / 6.40 (5.82–8.83) | 2.66 / 3.07 / 6.78 (4.29–7.42) | 89% | 32 |
| all | 0 | 50 | 6 | 2.62 / 5.82 / 9.47 (8.58–12.42) | 5.63 / 9.34 / 13.57 (11.39–21.25) | 6.02 / 9.73 / 14.34 (13.44–17.41) | 3.81 / 5.25 / 9.73 (7.81–14.59) | 63% | 56 |
| all | 0 | 50 | 8 | 4.86 / 12.03 / 16.00 (15.49–33.79) | 8.06 / 14.21 / 22.78 (18.43–37.38) | 11.14 / 19.20 / 44.54 (24.32–153.60) | 5.76 / 8.13 / 18.18 (13.18–149.50) | 47% | 57 |
| all | 0 | 200 | 2 | 1.00 / 1.46 / 1.74 (1.49–4.74) | 1.28 / 1.41 / 1.54 (1.46–5.95) | 1.25 / 1.41 / 1.49 (1.49–4.61) | 1.28 / 1.41 / 1.57 (1.46–3.74) | 100% | 0 |
| all | 0 | 200 | 3 | 1.36 / 2.14 / 2.85 (2.24–5.57) | 2.24 / 2.66 / 8.19 (2.85–13.44) | 2.00 / 2.11 / 5.12 (2.21–7.30) | 2.00 / 2.14 / 5.82 (2.50–8.00) | 100% | 0 |
| all | 0 | 200 | 4 | 1.90 / 2.85 / 7.17 (5.82–9.60) | 3.20 / 4.61 / 8.58 (6.85–12.16) | 2.69 / 2.85 / 6.78 (4.54–7.74) | 2.69 / 2.85 / 5.57 (3.04–11.26) | 96% | 3 |
| all | 0 | 200 | 6 | 3.20 / 7.10 / 11.26 (10.50–12.93) | 6.27 / 10.37 / 15.74 (13.70–24.32) | 7.04 / 10.50 / 16.13 (14.46–23.55) | 4.80 / 5.95 / 9.98 (7.74–18.18) | 69% | 45 |
| all | 0 | 200 | 8 | 5.25 / 13.06 / 19.20 (16.38–48.64) | 9.22 / 15.62 / 21.76 (19.97–44.03) | 12.67 / 18.43 / 31.74 (24.06–46.59) | 6.85 / 8.70 / 14.72 (13.70–26.37) | 50% | 29 |
| all | 0 | 1000 | 2 | 2.56 / 3.81 / 4.42 (3.90–12.29) | 2.85 / 2.98 / 5.25 (3.07–7.74) | 2.82 / 2.98 / 5.89 (3.07–7.87) | 2.85 / 2.98 / 3.10 (3.07–5.18) | 100% | 0 |
| all | 0 | 1000 | 3 | 4.00 / 5.25 / 8.96 (5.31–14.21) | 4.74 / 4.99 / 9.34 (8.45–13.70) | 4.35 / 4.48 / 8.19 (4.93–9.98) | 4.35 / 4.48 / 8.70 (4.67–16.13) | 100% | 0 |
| all | 0 | 1000 | 4 | 4.48 / 6.21 / 12.67 (10.37–17.92) | 6.46 / 9.47 / 16.38 (14.34–19.46) | 5.82 / 6.02 / 11.14 (10.50–19.20) | 5.82 / 6.98 / 11.78 (11.01–17.92) | 100% | 0 |
| all | 0 | 1000 | 6 | 7.36 / 11.78 / 18.43 (17.41–22.78) | 11.65 / 16.90 / 23.55 (20.48–25.34) | 12.16 / 18.43 / 22.53 (22.02–23.55) | 10.11 / 10.88 / 16.38 (15.36–21.50) | 68% | 47 |
| all | 0 | 1000 | 8 | 8.70 / 20.22 / 29.70 (26.37–39.42) | 15.36 / 24.83 / 33.79 (31.74–43.52) | 21.25 / 29.70 / 38.91 (38.40–43.52) | 14.21 / 15.23 / 22.27 (21.25–23.81) | 50% | 83 |
| all | 1000 | 50 | 2 | 1.66 / 2.03 / 2.27 (2.14–5.82) | 1.04 / 1.79 / 1.90 (1.87–1.90) | 1.00 / 1.79 / 1.87 (1.87–5.44) | 1.04 / 1.79 / 1.87 (1.87–4.99) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.11 / 3.26 / 5.95 (3.78–10.50) | 1.95 / 3.52 / 3.78 (3.65–6.59) | 1.62 / 1.82 / 5.76 (1.90–15.36) | 1.57 / 1.78 / 5.57 (1.87–11.14) | 100% | 0 |
| all | 1000 | 50 | 4 | 2.40 / 4.67 / 10.50 (5.25–17.92) | 3.20 / 5.06 / 11.65 (7.81–13.70) | 2.21 / 2.37 / 7.10 (3.52–8.58) | 2.14 / 2.40 / 5.95 (4.86–10.62) | 98% | 13 |
| all | 1000 | 50 | 6 | 3.68 / 7.36 / 9.34 (8.96–13.31) | 5.31 / 9.60 / 13.82 (11.26–17.15) | 5.12 / 9.34 / 15.87 (11.26–16.26) | 3.33 / 5.12 / 9.34 (7.74–372.74) | 65% | 111 |
| all | 1000 | 50 | 8 | 5.38 / 10.75 / 17.66 (13.95–2621.44) | 7.10 / 13.18 / 17.66 (17.41–82.94) | 8.83 / 16.64 / 25.34 (22.53–1638.40) | 4.16 / 7.62 / 15.62 (12.29–63.49) | 48% | 415 |
| all | 1000 | 200 | 2 | 1.90 / 2.40 / 5.12 (2.50–6.27) | 1.28 / 1.41 / 1.87 (1.62–1.95) | 1.28 / 1.41 / 1.82 (1.74–3.49) | 1.28 / 1.41 / 1.90 (1.74–2.21) | 100% | 0 |
| all | 1000 | 200 | 3 | 2.50 / 3.62 / 7.04 (4.22–10.37) | 2.37 / 3.68 / 4.80 (3.87–7.94) | 2.00 / 2.14 / 6.53 (2.24–8.13) | 1.95 / 2.11 / 2.98 (2.27–5.76) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.62 / 4.99 / 8.19 (7.10–16.13) | 3.52 / 5.50 / 10.24 (7.23–12.67) | 2.69 / 2.85 / 7.04 (4.99–9.98) | 2.69 / 2.85 / 7.87 (6.40–8.96) | 100% | 0 |
| all | 1000 | 200 | 6 | 4.16 / 8.06 / 10.11 (9.86–13.70) | 5.95 / 10.50 / 14.21 (11.78–16.90) | 5.89 / 10.50 / 14.98 (12.80–16.64) | 4.06 / 6.14 / 12.67 (8.83–15.74) | 65% | 49 |
| all | 1000 | 200 | 8 | 5.95 / 11.01 / 18.18 (15.49–23.55) | 8.06 / 14.72 / 20.99 (20.22–25.34) | 10.11 / 17.92 / 26.11 (24.32–35.33) | 5.57 / 8.83 / 14.46 (13.06–28.16) | 48% | 15 |
| all | 1000 | 1000 | 2 | 3.49 / 4.80 / 8.58 (6.46–9.73) | 2.85 / 2.98 / 6.08 (4.42–7.49) | 2.85 / 2.98 / 6.21 (3.10–7.30) | 2.85 / 2.98 / 3.94 (3.07–9.34) | 100% | 0 |
| all | 1000 | 1000 | 3 | 4.86 / 6.40 / 10.62 (8.70–14.21) | 3.94 / 6.40 / 8.96 (6.66–16.90) | 4.35 / 4.48 / 9.47 (6.53–12.29) | 4.35 / 4.48 / 9.09 (4.67–10.50) | 100% | 0 |
| all | 1000 | 1000 | 4 | 4.99 / 8.83 / 13.82 (12.42–20.22) | 7.10 / 9.73 / 14.21 (11.52–22.53) | 5.82 / 6.53 / 11.90 (10.88–16.38) | 5.82 / 6.66 / 14.08 (11.01–17.41) | 96% | 12 |
| all | 1000 | 1000 | 6 | 6.85 / 13.44 / 21.25 (16.90–75.78) | 11.26 / 16.64 / 21.25 (20.48–25.34) | 11.39 / 17.15 / 21.25 (20.74–24.83) | 10.24 / 12.16 / 17.66 (15.10–27.65) | 65% | 66 |
| all | 1000 | 1000 | 8 | 8.70 / 17.41 / 29.44 (25.34–51.71) | 14.98 / 24.32 / 31.49 (30.72–57.34) | 17.15 / 28.42 / 37.38 (34.82–39.94) | 14.46 / 16.26 / 23.30 (22.02–26.88) | 49% | 15 |

Migrations seen: 68059 over 90199816 samples (one sample per measured operation).


### Suite c: one k-thread process and n-k single-thread processes

load average before: { 6.20 7.07 6.71 }
load average after: { 6.65 7.07 6.74 }; 59 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.46 / 0.50 (0.46–0.70) | 0.29 / 0.46 / 0.66 (0.54–0.74) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 96% | 0 |
| 4 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.29 / 0.37 / 0.46 (0.46–0.62) | 0.29 / 0.46 / 0.58 (0.54–0.70) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 96% | 0 |
| 4 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.58 / 0.70 (0.70–0.91) | 0.29 / 0.50 / 0.74 (0.54–0.79) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 96% | 0 |
| 4 | 1000 | 1 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.07 / 1.25 / 1.54 (1.33–1.95) | 2.94 / 3.10 / 9.09 (4.48–10.88) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 96% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.07 / 1.25 / 1.41 (1.28–1.58) | 2.98 / 3.36 / 7.81 (4.54–11.01) | 1.00 / 1.04 / 1.07 (1.07–1.12) | 94% | 0 |
| 4 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.07 / 2.11 / 2.27 (2.24–2.43) | 2.98 / 3.14 / 7.81 (4.10–10.88) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 95% | 8 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.70 (0.66–0.70) | 0.37 / 0.62 / 0.74 (0.70–0.79) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 4 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.70 (0.62–0.70) | 0.37 / 0.62 / 0.74 (0.70–0.79) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 7 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.83 (0.79–0.95) | 0.42 / 0.66 / 0.83 (0.79–5.57) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 50% | 9 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 1.04 (0.83–1.04) | 0.42 / 1.20 / 1.82 (1.62–5.82) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 48% | 14 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.66 (0.66–0.70) | 0.42 / 1.07 / 1.57 (1.49–1.82) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 7 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.12 / 1.49 / 2.00 (1.62–8.45) | 3.14 / 3.36 / 10.50 (9.73–11.90) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 48% | 31 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.12 (1.07–1.15) | 1.12 / 1.49 / 1.95 (1.62–7.62) | 3.20 / 3.55 / 10.50 (9.47–29.95) | 1.00 / 1.07 / 1.12 (1.07–1.20) | 48% | 59 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.12 / 1.90 / 2.40 (2.37–6.46) | 3.20 / 3.65 / 9.73 (9.34–24.83) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 48% | 66 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.12 / 2.11 / 2.50 (2.37–12.29) | 3.20 / 4.42 / 10.24 (8.83–15.49) | 1.00 / 1.07 / 1.07 (1.07–1.54) | 48% | 109 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–2.56) | 1.12 / 1.46 / 1.66 (1.58–10.24) | 3.23 / 4.48 / 11.26 (8.06–23.55) | 1.00 / 1.07 / 1.12 (1.07–2.37) | 48% | 53 |

Independent processes: operations/s

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 1.92M (1.92M–1.93M) | 1.84M (1.77M–1.85M) | 1.83M (1.73M–1.84M) | 1.92M (1.91M–1.93M) | 96% | 0 |
| 4 | 200 | 2 | 1.28M (1.27M–1.28M) | 1.23M (1.18M–1.25M) | 1.21M (1.15M–1.22M) | 1.28M (1.27M–1.28M) | 96% | 0 |
| 4 | 200 | 3 | 640k (637k–642k) | 529k (505k–534k) | 608k (574k–613k) | 639k (633k–646k) | 96% | 0 |
| 4 | 1000 | 1 | 1.27M (1.27M–1.27M) | 1.23M (1.21M–1.24M) | 697k (675k–698k) | 1.27M (1.27M–1.28M) | 96% | 0 |
| 4 | 1000 | 2 | 849k (845k–850k) | 824k (794k–826k) | 460k (445k–462k) | 849k (844k–855k) | 94% | 0 |
| 4 | 1000 | 3 | 424k (422k–426k) | 356k (346k–358k) | 230k (223k–230k) | 426k (424k–427k) | 95% | 8 |
| 8 | 200 | 1 | 4.46M (4.44M–4.47M) | 4.15M (4.12M–4.19M) | 4.07M (3.98M–4.09M) | 4.46M (4.44M–4.48M) | 49% | 4 |
| 8 | 200 | 2 | 3.82M (3.81M–3.83M) | 3.52M (3.43M–3.60M) | 3.49M (3.41M–3.56M) | 3.83M (3.82M–3.84M) | 49% | 7 |
| 8 | 200 | 3 | 3.18M (3.17M–3.19M) | 2.88M (2.80M–2.93M) | 2.85M (2.75M–2.91M) | 3.18M (3.16M–3.20M) | 50% | 9 |
| 8 | 200 | 5 | 1.91M (1.86M–1.92M) | 1.64M (1.55M–1.67M) | 1.31M (1.28M–1.35M) | 1.92M (1.89M–1.92M) | 48% | 14 |
| 8 | 200 | 6 | 1.27M (1.26M–1.28M) | 1.19M (1.17M–1.21M) | 882k (844k–886k) | 1.28M (1.28M–1.29M) | 49% | 7 |
| 8 | 1000 | 1 | 3.03M (3.03M–3.03M) | 2.79M (2.44M–2.87M) | 1.57M (1.56M–1.61M) | 3.03M (3.02M–3.03M) | 48% | 31 |
| 8 | 1000 | 2 | 2.59M (2.59M–2.60M) | 2.44M (2.35M–2.47M) | 1.33M (1.26M–1.33M) | 2.60M (2.50M–2.60M) | 48% | 59 |
| 8 | 1000 | 3 | 2.17M (2.15M–2.17M) | 1.96M (1.76M–1.99M) | 1.10M (1.07M–1.11M) | 2.17M (2.16M–2.17M) | 48% | 66 |
| 8 | 1000 | 5 | 1.30M (1.29M–1.30M) | 1.11M (1.04M–1.14M) | 596k (593k–607k) | 1.30M (1.29M–1.30M) | 48% | 109 |
| 8 | 1000 | 6 | 865k (856k–867k) | 802k (771k–835k) | 403k (398k–417k) | 864k (857k–867k) | 48% | 53 |

The k-thread process: operations/s

| n | W ns | k | 1 per-core | 2 fixed/2 | 2 fixed/4 | 3 sized | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 641k (630k–647k) | 617k (587k–621k) | 609k (575k–613k) | 638k (635k–641k) | 96% | 0 |
| 4 | 200 | 2 | 1.07M (1.06M–1.08M) | 1.22M (1.18M–1.23M) | 1.21M (1.15M–1.22M) | 1.22M (1.17M–1.24M) | 96% | 0 |
| 4 | 200 | 3 | 1.46M (1.44M–1.46M) | 1.50M (1.43M–1.51M) | 1.82M (1.72M–1.84M) | 1.80M (1.73M–1.82M) | 96% | 0 |
| 4 | 1000 | 1 | 424k (420k–426k) | 410k (401k–415k) | 232k (225k–233k) | 426k (424k–427k) | 96% | 0 |
| 4 | 1000 | 2 | 707k (700k–711k) | 820k (797k–830k) | 460k (445k–462k) | 817k (798k–824k) | 94% | 0 |
| 4 | 1000 | 3 | 965k (957k–966k) | 990k (939k–994k) | 690k (668k–692k) | 916k (873k–921k) | 95% | 8 |
| 8 | 200 | 1 | 642k (639k–643k) | 596k (594k–627k) | 579k (569k–583k) | 641k (638k–643k) | 49% | 4 |
| 8 | 200 | 2 | 1.07M (1.05M–1.08M) | 1.18M (1.15M–1.24M) | 1.16M (1.14M–1.18M) | 1.19M (1.11M–1.22M) | 49% | 7 |
| 8 | 200 | 3 | 1.43M (1.41M–1.44M) | 1.41M (1.36M–1.46M) | 1.69M (1.62M–1.72M) | 1.73M (1.71M–1.75M) | 50% | 9 |
| 8 | 200 | 5 | 2.09M (1.98M–2.12M) | 2.10M (2.06M–2.11M) | 2.14M (2.08M–2.19M) | 2.55M (2.49M–2.74M) | 48% | 14 |
| 8 | 200 | 6 | 2.40M (2.36M–2.42M) | 2.41M (2.39M–2.42M) | 2.51M (2.40M–2.53M) | 2.67M (2.50M–2.67M) | 49% | 7 |
| 8 | 1000 | 1 | 434k (433k–434k) | 403k (347k–410k) | 225k (222k–231k) | 434k (429k–435k) | 48% | 31 |
| 8 | 1000 | 2 | 713k (710k–716k) | 785k (781k–820k) | 435k (404k–440k) | 813k (787k–837k) | 48% | 59 |
| 8 | 1000 | 3 | 955k (920k–979k) | 955k (812k–959k) | 642k (623k–645k) | 838k (818k–862k) | 48% | 66 |
| 8 | 1000 | 5 | 1.39M (1.38M–1.39M) | 1.36M (1.24M–1.40M) | 935k (873k–971k) | 847k (827k–852k) | 48% | 109 |
| 8 | 1000 | 6 | 1.56M (1.54M–1.58M) | 1.57M (1.52M–1.61M) | 1.08M (1.07M–1.10M) | 826k (818k–852k) | 48% | 53 |

Migrations seen: 62246 over 143287118 samples (one sample per measured operation).


### Suite d: one core moving between group A and group B (one fixed core each)

load average before: { 6.65 7.07 6.74 }
load average after: { 5.97 6.90 6.68 }; 15 s

Throughput of the 3 cores, operations/s

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | 1.84M (1.84M–1.85M) | 94% | 0 |
| 0 | 200 | 100 | 1.84M (1.84M–1.85M) | 100% | 0 |
| 0 | 200 | 1000 | 1.85M (1.85M–1.86M) | 100% | 0 |
| 0 | 200 | 10000 | 1.85M (1.84M–1.85M) | 98% | 0 |
| 0 | 1000 | 0 | 1.24M (1.24M–1.24M) | 100% | 0 |
| 0 | 1000 | 100 | 1.23M (1.23M–1.23M) | 100% | 0 |
| 0 | 1000 | 1000 | 1.23M (1.23M–1.23M) | 100% | 0 |
| 0 | 1000 | 10000 | 1.22M (1.22M–1.23M) | 100% | 0 |
| 1000 | 200 | 0 | 1.86M (1.85M–1.86M) | 100% | 0 |
| 1000 | 200 | 100 | 1.84M (1.84M–1.85M) | 100% | 0 |
| 1000 | 200 | 1000 | 1.84M (1.82M–1.85M) | 100% | 0 |
| 1000 | 200 | 10000 | 1.84M (1.81M–1.84M) | 99% | 0 |
| 1000 | 1000 | 0 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 1000 | 1000 | 100 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 1000 | 1000 | 1000 | 1.23M (1.23M–1.24M) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.22M (1.22M–1.22M) | 99% | 0 |

Kernel operations other than the first after a move, µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | 0.25 / 0.46 / 2.24 (2.24–2.37) | 94% | 0 |
| 0 | 200 | 100 | 0.29 / 0.46 / 2.24 (2.24–2.27) | 100% | 0 |
| 0 | 200 | 1000 | 0.29 / 0.46 / 2.24 (2.21–2.24) | 100% | 0 |
| 0 | 200 | 10000 | 0.29 / 0.46 / 2.24 (1.82–2.27) | 98% | 0 |
| 0 | 1000 | 0 | 1.04 / 1.28 / 3.87 (3.81–3.87) | 100% | 0 |
| 0 | 1000 | 100 | 1.04 / 1.36 / 3.87 (3.87–3.90) | 100% | 0 |
| 0 | 1000 | 1000 | 1.04 / 1.36 / 3.87 (3.87–3.87) | 100% | 0 |
| 0 | 1000 | 10000 | 1.04 / 1.90 / 3.87 (3.87–3.87) | 100% | 0 |
| 1000 | 200 | 0 | 0.25 / 0.42 / 2.21 (1.79–2.24) | 100% | 0 |
| 1000 | 200 | 100 | 0.29 / 0.46 / 2.24 (2.24–2.27) | 100% | 0 |
| 1000 | 200 | 1000 | 0.29 / 0.46 / 2.24 (2.24–2.27) | 100% | 0 |
| 1000 | 200 | 10000 | 0.29 / 0.46 / 2.24 (2.24–2.94) | 99% | 0 |
| 1000 | 1000 | 0 | 1.04 / 1.28 / 3.87 (3.87–3.87) | 100% | 0 |
| 1000 | 1000 | 100 | 1.04 / 1.33 / 3.87 (3.87–3.87) | 100% | 0 |
| 1000 | 1000 | 1000 | 1.04 / 1.36 / 3.87 (3.87–3.87) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.04 / 1.90 / 3.87 (3.87–3.90) | 99% | 0 |

Move (quiesce, leave, switch, join), µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | – | 94% | 0 |
| 0 | 200 | 100 | 0.62 / 0.74 / 0.74 (0.66–3.49) | 100% | 0 |
| 0 | 200 | 1000 | 0.66 / 0.91 / 0.95 (0.91–1.07) | 100% | 0 |
| 0 | 200 | 10000 | 0.62 / 0.87 / 0.95 (0.91–4.42) | 98% | 0 |
| 0 | 1000 | 0 | – | 100% | 0 |
| 0 | 1000 | 100 | 2.27 / 9.34 / 9.34 (5.82–13.57) | 100% | 0 |
| 0 | 1000 | 1000 | 2.27 / 3.33 / 3.36 (3.33–6.34) | 100% | 0 |
| 0 | 1000 | 10000 | 2.24 / 3.23 / 4.48 (3.62–5.06) | 100% | 0 |
| 1000 | 200 | 0 | – | 100% | 0 |
| 1000 | 200 | 100 | 1.70 / 1.87 / 1.87 (1.87–11.39) | 100% | 0 |
| 1000 | 200 | 1000 | 1.70 / 4.48 / 6.91 (4.74–10.50) | 100% | 0 |
| 1000 | 200 | 10000 | 1.66 / 1.87 / 2.24 (1.95–8.58) | 99% | 0 |
| 1000 | 1000 | 0 | – | 100% | 0 |
| 1000 | 1000 | 100 | 3.33 / 13.18 / 13.18 (4.10–17.15) | 100% | 0 |
| 1000 | 1000 | 1000 | 3.33 / 4.48 / 7.23 (4.29–12.93) | 100% | 0 |
| 1000 | 1000 | 10000 | 3.26 / 4.29 / 9.34 (4.86–23.04) | 99% | 0 |

First operation after a move, µs

| switch ns | W ns | moves/s | 3 sized | on P | stalls |
|---|---|---|---|---|---|
| 0 | 200 | 0 | – | 94% | 0 |
| 0 | 200 | 100 | 0.25 / 0.58 / 0.58 (0.54–1.58) | 100% | 0 |
| 0 | 200 | 1000 | 0.29 / 1.54 / 1.66 (0.58–2.24) | 100% | 0 |
| 0 | 200 | 10000 | 0.29 / 0.87 / 2.21 (2.14–2.27) | 98% | 0 |
| 0 | 1000 | 0 | – | 100% | 0 |
| 0 | 1000 | 100 | 2.03 / 2.11 / 2.11 (2.08–7.23) | 100% | 0 |
| 0 | 1000 | 1000 | 2.00 / 2.27 / 3.78 (2.30–3.81) | 100% | 0 |
| 0 | 1000 | 10000 | 1.07 / 2.30 / 3.81 (3.81–9.09) | 100% | 0 |
| 1000 | 200 | 0 | – | 100% | 0 |
| 1000 | 200 | 100 | 0.29 / 0.58 / 0.58 (0.54–7.30) | 100% | 0 |
| 1000 | 200 | 1000 | 0.29 / 1.57 / 2.21 (1.54–7.23) | 100% | 0 |
| 1000 | 200 | 10000 | 0.29 / 0.74 / 2.21 (1.87–3.07) | 99% | 0 |
| 1000 | 1000 | 0 | – | 100% | 0 |
| 1000 | 1000 | 100 | 2.03 / 2.30 / 2.30 (2.11–8.58) | 100% | 0 |
| 1000 | 1000 | 1000 | 1.12 / 3.78 / 3.81 (3.78–13.31) | 100% | 0 |
| 1000 | 1000 | 10000 | 1.07 / 2.30 / 3.87 (3.81–6.46) | 99% | 0 |

Migrations seen: 3770 over 19418253 samples (one sample per measured operation).


## Appendix C: full tables, second series, pass 1


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 16.0 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a1f: capability operations, first and fair settings

load average before: { 5.79 5.58 4.92 }
load average after: { 4.71 6.42 6.28 }; 611 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 1.37M (1.35M–1.40M) | 1.39M (1.37M–1.40M) | 1.38M (1.37M–1.40M) | 1.38M (1.36M–1.40M) | 1.38M (1.36M–1.40M) | 1.31M (895k–1.37M) | 1.37M (1.05M–1.41M) | 1.31M (944k–1.37M) | 1.38M (1.23M–1.40M) | 1.31M (1.06M–1.37M) | 1.38M (1.23M–1.41M) | 84% | 30 |
| 1 | 0 | 50 | 3 | 2.06M (1.56M–2.09M) | 2.06M (1.37M–2.08M) | 2.05M (1.62M–2.09M) | 2.07M (2.01M–2.08M) | 2.06M (1.63M–2.09M) | 1.99M (1.95M–2.04M) | 2.06M (2.04M–2.09M) | 1.88M (1.86M–2.03M) | 2.07M (1.79M–2.11M) | 1.91M (1.81M–2.03M) | 2.06M (1.98M–2.11M) | 83% | 176 |
| 1 | 0 | 50 | 4 | 2.75M (2.73M–2.77M) | 2.77M (2.51M–2.78M) | 2.74M (2.06M–2.79M) | 2.74M (1.02M–2.79M) | 2.75M (2.22M–2.78M) | 2.65M (2.54M–2.70M) | 2.75M (2.72M–2.78M) | 2.56M (2.40M–2.71M) | 2.76M (2.32M–2.80M) | 2.64M (2.42M–2.70M) | 2.76M (2.73M–2.81M) | 86% | 113 |
| 1 | 0 | 50 | 6 | 4.11M (4.00M–4.14M) | 4.11M (3.95M–4.16M) | 4.04M (4.01M–4.13M) | 4.07M (4.06M–4.10M) | 4.06M (783k–4.10M) | 3.78M (1.17M–3.82M) | 4.07M (2.12M–4.09M) | 3.69M (2.20M–3.80M) | 4.10M (2.66M–4.11M) | 3.61M (783k–3.72M) | 4.14M (3.98M–4.16M) | 56% | 393 |
| 1 | 0 | 50 | 8 | 5.22M (238k–5.37M) | 5.34M (2.59M–5.36M) | 5.35M (2.90M–5.37M) | 5.34M (3.32M–5.40M) | 5.37M (4.61M–5.40M) | 5.00M (4.88M–5.04M) | 5.37M (5.32M–5.40M) | 4.77M (4.56M–4.87M) | 5.44M (5.27M–5.44M) | 3.81M (1.57M–4.50M) | 5.54M (4.44M–5.55M) | 46% | 1452 |
| 1 | 0 | 200 | 2 | 1.28M (1.25M–1.28M) | 1.28M (1.25M–1.29M) | 1.28M (1.25M–1.28M) | 1.28M (1.26M–1.29M) | 1.28M (1.26M–1.29M) | 1.24M (1.21M–1.25M) | 1.29M (1.27M–1.29M) | 1.24M (1.22M–1.25M) | 1.29M (1.23M–1.29M) | 1.23M (1.21M–1.25M) | 1.29M (1.28M–1.30M) | 99% | 0 |
| 1 | 0 | 200 | 3 | 1.91M (1.89M–1.91M) | 1.91M (1.88M–1.91M) | 1.90M (1.89M–1.92M) | 1.91M (1.74M–1.91M) | 1.91M (1.89M–1.91M) | 1.85M (1.27M–1.87M) | 1.91M (1.90M–1.92M) | 1.83M (1.66M–1.85M) | 1.92M (1.85M–1.93M) | 1.84M (1.34M–1.85M) | 1.92M (1.89M–1.93M) | 93% | 59 |
| 1 | 0 | 200 | 4 | 2.54M (1.16M–2.54M) | 2.53M (1.99M–2.54M) | 2.51M (2.48M–2.55M) | 2.55M (2.43M–2.55M) | 2.54M (1.19M–2.56M) | 2.38M (1.41M–2.47M) | 2.54M (2.50M–2.56M) | 2.30M (1.43M–2.46M) | 2.57M (2.18M–2.58M) | 2.35M (1.76M–2.45M) | 2.55M (1.71M–2.58M) | 78% | 310 |
| 1 | 0 | 200 | 6 | 3.74M (98k–3.80M) | 3.01M (429k–3.82M) | 3.46M (1.05M–3.79M) | 3.49M (650k–3.82M) | 3.02M (309k–3.83M) | 3.45M (2.20M–3.60M) | 3.73M (1.75M–3.84M) | 3.41M (3.24M–3.44M) | 3.79M (2.76M–3.85M) | 2.78M (2.57M–2.91M) | 3.83M (3.79M–3.90M) | 58% | 1313 |
| 1 | 0 | 200 | 8 | 4.86M (4.53M–4.94M) | 4.94M (892k–4.96M) | 4.95M (497k–4.95M) | 4.94M (501k–4.99M) | 2.76M (137k–4.98M) | 2.63M (672k–4.64M) | 4.96M (1.77M–4.97M) | 4.40M (729k–4.44M) | 4.89M (2.49M–5.04M) | 2.56M (134k–2.90M) | 5.08M (4.43M–5.12M) | 46% | 1969 |
| 1 | 0 | 1000 | 2 | 840k (639k–844k) | 838k (779k–844k) | 845k (836k–845k) | 843k (836k–847k) | 848k (831k–850k) | 824k (810k–832k) | 842k (833k–851k) | 828k (810k–831k) | 848k (842k–851k) | 818k (809k–828k) | 846k (838k–853k) | 96% | 40 |
| 1 | 0 | 1000 | 3 | 1.25M (941k–1.26M) | 1.25M (1.24M–1.26M) | 1.26M (493k–1.26M) | 1.26M (1.26M–1.26M) | 1.26M (1.25M–1.26M) | 1.23M (1.21M–1.23M) | 1.27M (1.26M–1.27M) | 920k (298k–923k) | 1.27M (1.17M–1.27M) | 894k (860k–921k) | 1.27M (1.26M–1.28M) | 89% | 104 |
| 1 | 0 | 1000 | 4 | 1.66M (1.63M–1.68M) | 1.67M (1.54M–1.68M) | 1.68M (1.57M–1.69M) | 1.68M (1.67M–1.70M) | 1.69M (1.68M–1.69M) | 1.57M (1.56M–1.61M) | 1.69M (1.67M–1.69M) | 864k (850k–920k) | 1.68M (1.13M–1.69M) | 864k (129k–918k) | 1.68M (1.35M–1.69M) | 76% | 147 |
| 1 | 0 | 1000 | 6 | 2.48M (420k–2.49M) | 2.47M (250k–2.49M) | 2.52M (1.20M–2.53M) | 2.49M (827k–2.53M) | 2.51M (1.84M–2.53M) | 2.30M (2.17M–2.33M) | 2.51M (2.41M–2.52M) | 1.60M (1.59M–1.64M) | 2.51M (2.49M–2.53M) | 843k (827k–851k) | 2.52M (2.49M–2.53M) | 59% | 549 |
| 1 | 0 | 1000 | 8 | 3.28M (3.05M–3.28M) | 3.29M (1.81M–3.30M) | 3.31M (530k–3.36M) | 3.35M (3.13M–3.36M) | 3.36M (3.13M–3.37M) | 3.07M (2.92M–3.08M) | 3.35M (3.05M–3.35M) | 1.64M (1.55M–1.66M) | 3.35M (3.21M–3.37M) | 825k (772k–848k) | 3.35M (3.30M–3.35M) | 46% | 764 |
| 1 | 1000 | 50 | 2 | 1.38M (1.35M–1.38M) | 1.38M (1.35M–1.39M) | 1.37M (1.36M–1.38M) | 1.38M (1.37M–1.39M) | 1.38M (1.30M–1.38M) | 1.35M (1.34M–1.37M) | 1.40M (1.39M–1.41M) | 1.36M (1.33M–1.37M) | 1.40M (1.39M–1.40M) | 1.35M (1.31M–1.37M) | 1.40M (1.36M–1.41M) | 99% | 2 |
| 1 | 1000 | 50 | 3 | 2.02M (1.19M–2.03M) | 2.02M (970k–2.04M) | 2.01M (2.00M–2.04M) | 2.03M (1.51M–2.04M) | 2.02M (1.91M–2.04M) | 1.68M (1.56M–2.01M) | 2.03M (1.31M–2.06M) | 1.88M (1.77M–2.03M) | 2.07M (1.76M–2.10M) | 1.89M (1.40M–2.02M) | 2.07M (2.01M–2.09M) | 77% | 259 |
| 1 | 1000 | 50 | 4 | 2.66M (2.62M–2.70M) | 2.68M (2.63M–2.71M) | 2.69M (2.61M–2.70M) | 2.68M (2.58M–2.71M) | 2.63M (2.61M–2.69M) | 2.51M (1.97M–2.63M) | 2.68M (1.09M–2.73M) | 2.21M (1.12M–2.70M) | 2.75M (1.83M–2.80M) | 1.95M (1.67M–2.70M) | 2.26M (1.79M–2.80M) | 74% | 201 |
| 1 | 1000 | 50 | 6 | 1.87M (695k–3.94M) | 3.61M (1.27M–3.99M) | 2.62M (903k–3.97M) | 3.33M (680k–3.97M) | 3.93M (934k–3.95M) | 3.67M (1.77M–3.72M) | 3.92M (3.61M–3.96M) | 3.53M (2.49M–3.69M) | 3.96M (3.15M–4.02M) | 3.61M (3.44M–3.71M) | 4.14M (4.12M–4.16M) | 57% | 1420 |
| 1 | 1000 | 50 | 8 | 5.06M (4.90M–5.08M) | 5.11M (4.88M–5.14M) | 5.07M (215k–5.08M) | 5.13M (4.80M–5.14M) | 5.11M (4.69M–5.13M) | 4.79M (4.54M–4.87M) | 5.13M (5.00M–5.14M) | 4.61M (4.41M–4.75M) | 5.21M (5.11M–5.24M) | 3.90M (3.88M–4.34M) | 5.47M (5.14M–5.55M) | 47% | 319 |
| 1 | 1000 | 200 | 2 | 1.26M (1.25M–1.27M) | 1.27M (1.26M–1.27M) | 1.27M (1.25M–1.27M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.27M) | 1.24M (1.09M–1.25M) | 1.29M (1.26M–1.30M) | 1.23M (1.18M–1.25M) | 1.29M (1.27M–1.30M) | 1.22M (1.18M–1.24M) | 1.29M (1.03M–1.29M) | 94% | 5 |
| 1 | 1000 | 200 | 3 | 1.87M (1.85M–1.88M) | 1.88M (1.02M–1.88M) | 1.86M (1.08M–1.88M) | 1.88M (1.86M–1.89M) | 1.87M (1.87M–1.89M) | 1.83M (1.76M–1.84M) | 1.89M (1.86M–1.89M) | 1.82M (1.73M–1.86M) | 1.91M (1.89M–1.93M) | 1.82M (1.70M–1.85M) | 1.92M (1.76M–1.93M) | 86% | 84 |
| 1 | 1000 | 200 | 4 | 2.46M (743k–2.47M) | 2.48M (1.07M–2.48M) | 2.46M (1.24M–2.49M) | 2.49M (1.46M–2.50M) | 2.48M (1.18M–2.49M) | 2.30M (2.28M–2.41M) | 2.50M (1.77M–2.51M) | 2.33M (2.03M–2.46M) | 2.57M (2.53M–2.59M) | 2.35M (2.28M–2.46M) | 2.56M (2.56M–2.59M) | 85% | 324 |
| 1 | 1000 | 200 | 6 | 3.59M (3.49M–3.64M) | 3.65M (3.58M–3.67M) | 3.63M (1.30M–3.68M) | 3.68M (3.35M–3.70M) | 3.67M (3.39M–3.69M) | 3.37M (3.31M–3.41M) | 3.69M (3.68M–3.70M) | 3.32M (3.29M–3.33M) | 3.75M (3.69M–3.76M) | 2.61M (1.96M–2.84M) | 3.89M (3.79M–3.90M) | 59% | 345 |
| 1 | 1000 | 200 | 8 | 4.69M (4.15M–4.72M) | 4.64M (4.14M–4.76M) | 4.70M (4.41M–4.75M) | 4.76M (4.68M–4.76M) | 4.74M (4.57M–4.77M) | 4.33M (3.45M–4.49M) | 4.76M (2.63M–4.79M) | 4.27M (3.75M–4.38M) | 4.76M (4.24M–4.87M) | 2.60M (502k–2.81M) | 5.08M (4.48M–5.10M) | 46% | 956 |
| 1 | 1000 | 1000 | 2 | 833k (824k–836k) | 835k (797k–840k) | 823k (626k–840k) | 839k (624k–841k) | 835k (738k–843k) | 813k (785k–829k) | 849k (844k–855k) | 797k (588k–829k) | 848k (746k–856k) | 816k (576k–828k) | 849k (841k–857k) | 80% | 155 |
| 1 | 1000 | 1000 | 3 | 1.24M (1.23M–1.25M) | 1.24M (604k–1.24M) | 1.25M (792k–1.25M) | 1.25M (610k–1.25M) | 1.24M (898k–1.25M) | 1.19M (719k–1.22M) | 1.19M (659k–1.25M) | 646k (355k–923k) | 1.11M (887k–1.27M) | 876k (482k–923k) | 1.26M (941k–1.27M) | 75% | 625 |
| 1 | 1000 | 1000 | 4 | 1.63M (569k–1.63M) | 1.63M (1.13M–1.64M) | 1.64M (731k–1.65M) | 1.65M (1.45M–1.66M) | 1.65M (1.37M–1.65M) | 1.56M (1.54M–1.60M) | 1.66M (1.49M–1.67M) | 886k (857k–921k) | 1.68M (1.67M–1.69M) | 891k (728k–918k) | 1.68M (1.21M–1.68M) | 80% | 407 |
| 1 | 1000 | 1000 | 6 | 2.39M (2.24M–2.40M) | 2.42M (1.52M–2.43M) | 2.45M (2.08M–2.46M) | 2.43M (2.38M–2.45M) | 2.45M (2.40M–2.46M) | 2.29M (2.23M–2.29M) | 2.46M (1.99M–2.47M) | 1.60M (1.54M–1.63M) | 2.48M (2.47M–2.50M) | 848k (823k–858k) | 2.53M (2.51M–2.54M) | 60% | 339 |
| 1 | 1000 | 1000 | 8 | 3.10M (1.31M–3.16M) | 3.06M (2.16M–3.18M) | 3.17M (2.45M–3.23M) | 3.16M (2.84M–3.25M) | 2.96M (2.72M–3.24M) | 3.00M (2.47M–3.03M) | 3.18M (3.15M–3.27M) | 1.56M (1.05M–1.70M) | 3.27M (1.74M–3.30M) | 764k (382k–853k) | 3.30M (2.61M–3.35M) | 47% | 2042 |
| 10 | 0 | 50 | 2 | 1.32M (1.28M–1.33M) | 1.33M (1.30M–1.33M) | 1.33M (1.29M–1.33M) | 1.33M (1.18M–1.34M) | 1.34M (1.09M–1.34M) | 1.35M (1.22M–1.35M) | 1.38M (1.35M–1.39M) | 1.35M (1.25M–1.35M) | 1.38M (1.36M–1.39M) | 1.34M (1.26M–1.35M) | 1.38M (1.31M–1.39M) | 91% | 18 |
| 10 | 0 | 50 | 3 | 1.90M (1.86M–1.91M) | 1.93M (1.86M–1.94M) | 1.92M (1.87M–1.93M) | 1.94M (1.82M–1.96M) | 1.95M (259k–1.96M) | 1.89M (746k–1.91M) | 1.97M (1.18M–1.98M) | 1.99M (1.32M–2.02M) | 2.06M (1.54M–2.07M) | 1.99M (1.66M–2.01M) | 2.05M (1.54M–2.07M) | 92% | 115 |
| 10 | 0 | 50 | 4 | 2.42M (1.72M–2.50M) | 2.46M (2.16M–2.54M) | 2.45M (2.40M–2.53M) | 2.48M (2.46M–2.57M) | 2.48M (2.37M–2.57M) | 2.46M (2.27M–2.47M) | 2.57M (2.43M–2.59M) | 2.57M (1.95M–2.66M) | 2.72M (2.68M–2.76M) | 2.60M (2.45M–2.68M) | 2.73M (2.68M–2.75M) | 85% | 115 |
| 10 | 0 | 50 | 6 | 3.29M (1.01M–3.34M) | 3.44M (2.04M–3.49M) | 3.36M (3.31M–3.40M) | 3.43M (3.41M–3.54M) | 3.48M (538k–3.54M) | 3.16M (2.43M–3.18M) | 3.45M (2.32M–3.47M) | 3.21M (1.43M–3.26M) | 3.64M (2.55M–3.69M) | 3.64M (2.90M–3.73M) | 4.00M (3.38M–4.03M) | 59% | 620 |
| 10 | 0 | 50 | 8 | 3.54M (562k–4.17M) | 3.75M (1.28M–4.34M) | 3.70M (1.78M–4.22M) | 3.34M (2.34M–4.42M) | 2.66M (2.45M–4.41M) | 3.91M (3.15M–4.00M) | 4.25M (1.75M–4.41M) | 3.98M (2.59M–4.05M) | 4.66M (3.87M–4.68M) | 4.08M (2.24M–4.38M) | 5.26M (4.44M–5.33M) | 47% | 2546 |
| 10 | 0 | 200 | 2 | 1.19M (1.06M–1.21M) | 1.19M (620k–1.22M) | 1.19M (632k–1.22M) | 1.20M (1.01M–1.23M) | 1.09M (521k–1.23M) | 1.05M (1.01M–1.24M) | 1.25M (1.07M–1.28M) | 1.17M (1.07M–1.24M) | 1.25M (1.09M–1.28M) | 1.20M (842k–1.24M) | 1.26M (1.25M–1.28M) | 78% | 197 |
| 10 | 0 | 200 | 3 | 1.74M (1.68M–1.74M) | 1.73M (1.66M–1.76M) | 1.76M (1.70M–1.77M) | 1.76M (310k–1.79M) | 1.78M (826k–1.79M) | 1.72M (1.39M–1.74M) | 1.80M (1.21M–1.81M) | 1.82M (1.01M–1.84M) | 1.89M (1.52M–1.90M) | 1.81M (1.39M–1.83M) | 1.90M (1.66M–1.90M) | 89% | 169 |
| 10 | 0 | 200 | 4 | 2.22M (2.18M–2.24M) | 2.25M (2.22M–2.28M) | 2.28M (2.23M–2.29M) | 2.30M (2.24M–2.33M) | 2.28M (2.23M–2.34M) | 2.20M (1.98M–2.23M) | 2.31M (2.29M–2.36M) | 2.36M (2.25M–2.43M) | 2.50M (2.47M–2.51M) | 2.39M (2.27M–2.44M) | 2.50M (2.47M–2.52M) | 86% | 29 |
| 10 | 0 | 200 | 6 | 2.94M (2.87M–3.03M) | 3.09M (3.07M–3.14M) | 3.08M (2.59M–3.15M) | 3.19M (3.09M–3.25M) | 3.18M (3.17M–3.24M) | 2.81M (2.68M–2.88M) | 3.21M (3.09M–3.22M) | 2.87M (2.71M–2.96M) | 3.33M (3.33M–3.38M) | 2.66M (2.63M–2.76M) | 3.66M (3.63M–3.66M) | 57% | 143 |
| 10 | 0 | 200 | 8 | 3.75M (1.64M–3.76M) | 3.84M (1.92M–3.89M) | 3.88M (1.36M–3.91M) | 3.97M (765k–4.01M) | 4.02M (2.19M–4.05M) | 3.56M (3.22M–3.59M) | 4.00M (3.87M–4.05M) | 3.56M (3.48M–3.63M) | 4.28M (4.23M–4.29M) | 2.72M (1.74M–2.80M) | 4.70M (2.93M–4.72M) | 46% | 1397 |
| 10 | 0 | 1000 | 2 | 761k (539k–762k) | 769k (731k–771k) | 792k (642k–794k) | 797k (790k–798k) | 797k (794k–799k) | 823k (810k–824k) | 837k (828k–838k) | 824k (799k–827k) | 837k (825k–839k) | 821k (792k–827k) | 838k (699k–839k) | 94% | 46 |
| 10 | 0 | 1000 | 3 | 1.08M (351k–1.08M) | 1.09M (441k–1.09M) | 1.14M (780k–1.14M) | 1.15M (449k–1.16M) | 1.15M (532k–1.16M) | 1.07M (613k–1.08M) | 1.16M (650k–1.16M) | 914k (866k–918k) | 1.20M (1.15M–1.21M) | 888k (766k–916k) | 1.20M (1.20M–1.21M) | 88% | 309 |
| 10 | 0 | 1000 | 4 | 1.36M (797k–1.37M) | 1.37M (1.24M–1.39M) | 1.20M (755k–1.48M) | 1.49M (1.14M–1.50M) | 1.49M (1.11M–1.51M) | 1.31M (1.05M–1.37M) | 1.47M (864k–1.49M) | 865k (410k–914k) | 1.53M (1.51M–1.56M) | 867k (734k–915k) | 1.54M (1.53M–1.55M) | 76% | 431 |
| 10 | 0 | 1000 | 6 | 1.81M (1.73M–1.84M) | 1.86M (1.79M–1.88M) | 2.01M (1.31M–2.04M) | 2.08M (98k–2.09M) | 2.06M (463k–2.12M) | 1.72M (862k–1.79M) | 2.02M (1.03M–2.06M) | 1.33M (312k–1.35M) | 2.05M (805k–2.05M) | 826k (157k–847k) | 1.88M (860k–2.11M) | 56% | 1272 |
| 10 | 0 | 1000 | 8 | 1.95M (6k–2.26M) | 1.91M (108k–2.30M) | 2.36M (727k–2.60M) | 2.54M (52k–2.66M) | 2.49M (1.16M–2.64M) | 2.06M (932k–2.07M) | 2.55M (1.74M–2.64M) | 1.33M (825k–1.44M) | 2.58M (1.84M–2.59M) | 842k (245k–850k) | 2.56M (2.40M–2.59M) | 46% | 2876 |
| 10 | 1000 | 50 | 2 | 1.17M (948k–1.17M) | 1.19M (1.12M–1.20M) | 1.18M (1.15M–1.18M) | 1.20M (1.18M–1.21M) | 1.21M (1.18M–1.21M) | 1.35M (1.26M–1.36M) | 1.39M (1.36M–1.39M) | 1.35M (1.27M–1.36M) | 1.39M (1.35M–1.39M) | 1.35M (1.09M–1.36M) | 1.38M (925k–1.39M) | 91% | 28 |
| 10 | 1000 | 50 | 3 | 1.59M (796k–1.60M) | 1.64M (1.05M–1.64M) | 1.59M (725k–1.60M) | 1.65M (998k–1.65M) | 1.63M (971k–1.64M) | 1.61M (1.29M–1.66M) | 1.73M (1.70M–1.75M) | 1.89M (1.84M–2.01M) | 2.05M (2.03M–2.08M) | 1.90M (1.87M–2.00M) | 2.04M (1.99M–2.07M) | 83% | 214 |
| 10 | 1000 | 50 | 4 | 1.93M (1.54M–2.00M) | 2.02M (1.98M–2.05M) | 1.96M (1.95M–2.02M) | 2.02M (1.76M–2.08M) | 1.98M (1.95M–2.04M) | 1.97M (1.96M–2.08M) | 2.13M (2.12M–2.21M) | 2.49M (2.44M–2.66M) | 2.73M (2.68M–2.76M) | 2.54M (2.33M–2.67M) | 2.70M (2.24M–2.75M) | 80% | 77 |
| 10 | 1000 | 50 | 6 | 2.56M (2.30M–2.62M) | 2.69M (2.36M–2.72M) | 2.60M (2.46M–2.66M) | 2.69M (2.69M–2.75M) | 2.53M (2.44M–2.56M) | 2.60M (2.47M–2.62M) | 2.78M (2.75M–2.80M) | 2.70M (1.80M–2.76M) | 3.08M (2.49M–3.12M) | 3.60M (1.96M–3.73M) | 4.01M (3.99M–4.03M) | 59% | 302 |
| 10 | 1000 | 50 | 8 | 3.15M (1.94M–3.19M) | 3.28M (2.94M–3.31M) | 3.19M (2.31M–3.23M) | 3.31M (2.90M–3.35M) | 3.00M (2.72M–3.06M) | 3.19M (3.08M–3.21M) | 3.42M (1.10M–3.45M) | 3.38M (2.09M–3.39M) | 3.82M (3.62M–3.84M) | 4.10M (419–4.23M) | 5.29M (2.61M–5.32M) | 46% | 928 |
| 10 | 1000 | 200 | 2 | 1.07M (1.05M–1.08M) | 1.10M (1.08M–1.10M) | 1.08M (1.07M–1.09M) | 1.11M (1.09M–1.11M) | 1.11M (1.01M–1.11M) | 1.22M (1.07M–1.24M) | 1.27M (1.25M–1.28M) | 1.22M (1.05M–1.24M) | 1.26M (785k–1.28M) | 1.11M (871k–1.24M) | 1.27M (1.14M–1.28M) | 84% | 44 |
| 10 | 1000 | 200 | 3 | 1.45M (756k–1.46M) | 1.48M (1.15M–1.50M) | 1.45M (557k–1.48M) | 1.51M (1.02M–1.52M) | 1.50M (806k–1.52M) | 1.48M (1.46M–1.52M) | 1.59M (1.58M–1.62M) | 1.73M (1.23M–1.85M) | 1.88M (1.43M–1.90M) | 1.71M (1.37M–1.83M) | 1.87M (1.69M–1.89M) | 77% | 291 |
| 10 | 1000 | 200 | 4 | 1.80M (981k–1.84M) | 1.85M (1.01M–1.89M) | 1.83M (526k–1.86M) | 1.89M (850k–1.93M) | 1.85M (428k–1.89M) | 1.83M (847k–1.92M) | 2.01M (1.43M–2.04M) | 2.34M (1.90M–2.43M) | 2.46M (2.33M–2.53M) | 2.25M (1.68M–2.44M) | 2.49M (2.47M–2.53M) | 78% | 639 |
| 10 | 1000 | 200 | 6 | 2.39M (2.31M–2.42M) | 2.44M (2.38M–2.48M) | 2.46M (2.44M–2.48M) | 2.52M (2.21M–2.55M) | 2.38M (859k–2.38M) | 2.39M (2.36M–2.44M) | 2.62M (2.46M–2.65M) | 2.40M (2.35M–2.55M) | 2.89M (2.81M–2.91M) | 2.59M (2.48M–2.80M) | 3.66M (3.61M–3.67M) | 58% | 378 |
| 10 | 1000 | 200 | 8 | 2.92M (677k–2.93M) | 2.87M (1.85M–3.00M) | 3.00M (2.64M–3.00M) | 3.10M (2.49M–3.11M) | 2.85M (726k–2.87M) | 2.82M (2.63M–2.96M) | 3.16M (3.03M–3.20M) | 2.94M (2.48M–3.10M) | 3.57M (3.41M–3.59M) | 2.65M (2.40M–2.69M) | 4.66M (4.45M–4.72M) | 47% | 1260 |
| 10 | 1000 | 1000 | 2 | 705k (670k–710k) | 721k (718k–725k) | 732k (726k–738k) | 743k (740k–749k) | 743k (738k–750k) | 821k (794k–827k) | 837k (831k–839k) | 822k (814k–828k) | 837k (833k–838k) | 820k (813k–827k) | 837k (827k–839k) | 96% | 11 |
| 10 | 1000 | 1000 | 3 | 965k (948k–967k) | 992k (973k–993k) | 1.01M (999k–1.02M) | 1.04M (1.02M–1.04M) | 1.03M (1.01M–1.04M) | 995k (957k–998k) | 1.09M (1.07M–1.09M) | 917k (866k–918k) | 1.21M (1.18M–1.21M) | 916k (885k–919k) | 1.21M (1.18M–1.21M) | 95% | 6 |
| 10 | 1000 | 1000 | 4 | 1.18M (1.18M–1.20M) | 1.22M (1.21M–1.24M) | 1.26M (1.26M–1.29M) | 1.30M (1.29M–1.32M) | 1.28M (1.28M–1.31M) | 1.22M (1.21M–1.26M) | 1.37M (1.36M–1.39M) | 878k (852k–913k) | 1.54M (1.52M–1.55M) | 874k (856k–916k) | 1.55M (1.52M–1.55M) | 86% | 19 |
| 10 | 1000 | 1000 | 6 | 1.55M (1.52M–1.58M) | 1.58M (1.05M–1.61M) | 1.68M (340k–1.71M) | 1.75M (840k–1.77M) | 1.70M (851k–1.71M) | 1.57M (1.35M–1.61M) | 1.83M (1.71M–1.85M) | 1.31M (979k–1.33M) | 1.90M (1.83M–1.91M) | 841k (626k–845k) | 2.11M (1.86M–2.11M) | 60% | 917 |
| 10 | 1000 | 1000 | 8 | 1.88M (69k–1.89M) | 1.88M (590k–1.92M) | 2.05M (381k–2.08M) | 2.12M (627k–2.15M) | 2.05M (834k–2.09M) | 1.90M (1.18M–1.94M) | 2.24M (1.08M–2.29M) | 1.36M (280k–1.43M) | 2.34M (1.17M–2.39M) | 839k (266k–841k) | 2.58M (1.98M–2.59M) | 47% | 2396 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.17 / 0.29 / 0.33 (0.33–0.50) | 0.08 / 0.12 / 0.21 (0.21–0.33) | 0.17 / 0.29 / 0.33 (0.33–0.50) | 0.08 / 0.12 / 0.21 (0.21–0.29) | 0.12 / 0.29 / 0.42 (0.33–0.54) | 0.08 / 0.12 / 0.21 (0.21–0.33) | 84% | 30 |
| 1 | 0 | 50 | 3 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.12 / 0.37 / 0.46 (0.33–0.54) | 0.08 / 0.17 / 0.21 (0.21–0.25) | 0.21 / 0.46 / 0.58 (0.37–0.66) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.21 / 0.50 / 0.58 (0.37–3.39) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 83% | 176 |
| 1 | 0 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.12–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.12 / 0.29 / 0.42 (0.37–0.58) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.17 / 0.42 / 0.62 (0.37–4.61) | 0.08 / 0.12 / 0.29 (0.29–2.14) | 0.17 / 0.37 / 0.58 (0.37–7.87) | 0.08 / 0.17 / 0.33 (0.29–0.50) | 86% | 113 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.21 / 0.42 / 0.78 (0.66–0.91) | 0.12 / 0.21 / 0.37 (0.33–0.37) | 0.25 / 0.54 / 0.83 (0.79–1.07) | 0.12 / 0.21 / 0.42 (0.37–0.46) | 0.29 / 0.58 / 0.91 (0.79–41.47) | 0.08 / 0.21 / 0.46 (0.42–0.46) | 56% | 393 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.46 / 0.91 (0.83–1.00) | 0.12 / 0.21 / 0.37 (0.33–0.42) | 0.29 / 0.74 / 1.12 (0.95–1.20) | 0.12 / 0.21 / 0.42 (0.42–0.50) | 0.70 / 1.15 / 4.99 (2.00–8.13) | 0.08 / 0.25 / 0.46 (0.46–0.50) | 46% | 1452 |
| 1 | 0 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.37) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.29 / 0.42 / 0.50 (0.46–0.79) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.29 / 0.42 / 0.50 (0.50–0.62) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.29 / 0.42 / 0.50 (0.46–0.62) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 99% | 0 |
| 1 | 0 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.25 / 0.42 / 0.54 (0.50–0.74) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.29 / 0.46 / 0.66 (0.54–4.86) | 0.21 / 0.25 / 0.66 (0.62–0.91) | 0.29 / 0.46 / 0.54 (0.50–0.74) | 0.21 / 0.25 / 0.66 (0.58–0.66) | 93% | 59 |
| 1 | 0 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.25–0.33) | 0.29 / 0.58 / 0.74 (0.58–0.83) | 0.21 / 0.29 / 0.66 (0.58–0.70) | 0.37 / 0.62 / 0.78 (0.58–8.19) | 0.21 / 0.33 / 0.74 (0.66–0.79) | 0.33 / 0.62 / 0.74 (0.54–6.53) | 0.21 / 0.29 / 0.74 (0.66–0.79) | 78% | 310 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.95) | 0.21 / 0.29 / 0.29 (0.29–0.37) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.37) | 0.21 / 0.29 / 0.29 (0.29–0.50) | 0.33 / 0.58 / 0.91 (0.87–1.54) | 0.21 / 0.33 / 0.66 (0.66–0.74) | 0.37 / 0.70 / 1.12 (1.07–1.28) | 0.21 / 0.33 / 0.79 (0.78–0.87) | 0.79 / 1.07 / 3.62 (1.90–12.42) | 0.21 / 0.50 / 0.83 (0.83–0.87) | 58% | 1313 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.37) | 0.33 / 0.62 / 1.04 (0.87–1.25) | 0.21 / 0.33 / 0.66 (0.58–0.74) | 0.42 / 0.91 / 1.46 (1.20–2.37) | 0.21 / 0.46 / 0.79 (0.70–0.83) | 1.41 / 2.24 / 9.47 (8.19–11141.12) | 0.21 / 0.62 / 0.87 (0.83–0.91) | 46% | 1969 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.07 / 1.25 / 1.46 (1.28–2.08) | 1.00 / 1.04 / 2.91 (1.82–2.94) | 1.07 / 1.25 / 1.33 (1.25–1.62) | 1.00 / 1.04 / 2.94 (2.91–2.94) | 1.07 / 1.25 / 1.54 (1.36–10.75) | 1.00 / 1.04 / 2.94 (2.94–2.98) | 96% | 40 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.07–5.89) | 1.00 / 1.00 / 1.07 (1.04–1.15) | 1.00 / 1.04 / 1.07 (1.04–1.25) | 1.04 / 1.28 / 2.08 (2.03–6.91) | 1.00 / 1.04 / 2.91 (2.91–2.98) | 1.87 / 2.03 / 5.70 (4.42–557.06) | 1.00 / 1.70 / 2.98 (2.98–3.07) | 1.90 / 2.37 / 8.70 (5.50–11.39) | 1.00 / 1.79 / 3.07 (2.98–3.10) | 89% | 104 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.07 / 1.07 (1.04–1.07) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.07–6.14) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.12 / 1.66 / 2.27 (2.11–6.34) | 1.00 / 1.82 / 3.04 (2.94–3.07) | 3.26 / 3.52 / 9.98 (6.78–16.26) | 1.00 / 2.24 / 3.14 (3.04–3.49) | 3.20 / 3.52 / 10.37 (8.83–7405.57) | 1.00 / 2.27 / 3.10 (3.04–4.35) | 76% | 147 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–13.57) | 1.00 / 1.07 / 1.12 (1.07–4.74) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.62) | 1.15 / 1.95 / 2.40 (2.40–5.95) | 1.00 / 2.11 / 3.10 (3.04–4.42) | 2.94 / 3.81 / 8.06 (4.74–16.26) | 1.00 / 2.50 / 3.23 (3.20–6.14) | 5.70 / 6.21 / 17.15 (13.06–29.95) | 1.00 / 2.69 / 3.26 (3.23–3.62) | 59% | 549 |
| 1 | 0 | 1000 | 8 | 1.04 / 1.07 / 1.07 (1.07–1.12) | 1.04 / 1.07 / 1.12 (1.07–2.27) | 1.04 / 1.07 / 1.12 (1.07–1.25) | 1.00 / 1.07 / 1.07 (1.07–6.78) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.15 / 2.00 / 2.50 (2.40–11.14) | 1.04 / 2.21 / 3.14 (3.10–5.57) | 3.39 / 4.16 / 11.78 (9.34–31.74) | 1.04 / 2.50 / 3.46 (3.36–4.35) | 8.00 / 8.96 / 21.76 (19.71–87.04) | 1.04 / 2.85 / 3.81 (3.78–3.90) | 46% | 764 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.25) | 0.08 / 0.08 / 0.12 (0.12–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.21) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.33 (0.33–0.42) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.12 / 0.25 / 0.33 (0.33–0.74) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.12 / 0.25 / 0.33 (0.33–0.37) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 99% | 2 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.17 / 0.17 (0.12–0.21) | 0.08 / 0.17 / 0.17 (0.12–0.21) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.12 / 0.37 / 0.46 (0.33–0.54) | 0.08 / 0.17 / 0.21 (0.21–0.25) | 0.25 / 0.50 / 0.58 (0.37–0.70) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.25 / 0.50 / 0.58 (0.37–0.66) | 0.08 / 0.17 / 0.33 (0.25–0.37) | 77% | 259 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.42 / 0.58 (0.37–0.58) | 0.08 / 0.17 / 0.25 (0.25–0.33) | 0.25 / 0.50 / 0.62 (0.37–5.95) | 0.08 / 0.17 / 0.37 (0.29–0.37) | 0.25 / 0.50 / 0.66 (0.46–1.41) | 0.08 / 0.17 / 0.33 (0.29–0.37) | 74% | 201 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.21 / 0.42 / 0.66 (0.62–0.83) | 0.08 / 0.21 / 0.33 (0.33–0.42) | 0.25 / 0.54 / 0.83 (0.78–1.00) | 0.12 / 0.21 / 0.42 (0.37–0.46) | 0.29 / 0.58 / 0.70 (0.70–0.91) | 0.08 / 0.21 / 0.42 (0.42–0.46) | 57% | 1420 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.25) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.46 / 0.83 (0.78–0.91) | 0.12 / 0.21 / 0.37 (0.33–0.42) | 0.25 / 0.58 / 0.95 (0.87–3.87) | 0.08 / 0.21 / 0.46 (0.42–0.50) | 0.70 / 1.15 / 4.61 (1.82–14.46) | 0.08 / 0.21 / 0.46 (0.46–0.50) | 47% | 319 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.29 / 0.46 / 0.50 (0.50–0.70) | 0.21 / 0.25 / 0.58 (0.58–0.66) | 0.29 / 0.42 / 0.50 (0.50–0.74) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.29 / 0.46 / 0.54 (0.50–0.74) | 0.21 / 0.29 / 0.58 (0.54–0.66) | 94% | 5 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.25 / 0.46 / 0.62 (0.54–0.87) | 0.21 / 0.25 / 0.54 (0.50–0.66) | 0.29 / 0.46 / 0.54 (0.54–0.87) | 0.21 / 0.25 / 0.66 (0.66–0.74) | 0.29 / 0.46 / 0.54 (0.54–5.57) | 0.21 / 0.25 / 0.66 (0.62–0.74) | 86% | 84 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.74 (0.58–0.87) | 0.21 / 0.29 / 0.58 (0.54–0.62) | 0.37 / 0.62 / 0.79 (0.54–0.95) | 0.21 / 0.33 / 0.74 (0.66–0.78) | 0.33 / 0.58 / 0.70 (0.66–0.91) | 0.21 / 0.33 / 0.79 (0.66–0.79) | 85% | 324 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.66 / 0.91 (0.91–1.12) | 0.21 / 0.37 / 0.70 (0.62–0.74) | 0.42 / 0.87 / 1.33 (1.28–4.67) | 0.21 / 0.37 / 0.79 (0.78–0.83) | 0.91 / 1.33 / 2.14 (1.49–10.88) | 0.21 / 0.50 / 0.87 (0.83–0.87) | 59% | 345 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 1.04 (1.00–1.25) | 0.21 / 0.33 / 0.66 (0.66–0.70) | 0.42 / 0.91 / 1.41 (1.20–1.57) | 0.21 / 0.42 / 0.79 (0.74–0.83) | 1.41 / 2.14 / 18.69 (7.10–4325.38) | 0.21 / 0.62 / 0.87 (0.87–0.87) | 46% | 956 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.07 / 1.07 (1.07–6.27) | 1.00 / 1.04 / 1.07 (1.04–4.93) | 1.00 / 1.07 / 1.12 (1.04–6.02) | 1.00 / 1.07 / 1.07 (1.04–5.63) | 1.00 / 1.04 / 1.07 (1.04–5.95) | 1.07 / 1.41 / 1.62 (1.33–5.12) | 1.00 / 1.07 / 2.94 (2.72–3.04) | 1.12 / 1.46 / 2.72 (1.33–12.67) | 1.00 / 1.07 / 2.98 (2.94–3.07) | 1.07 / 1.25 / 1.41 (1.25–14.08) | 1.00 / 1.04 / 2.98 (2.94–3.07) | 80% | 155 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.04–1.25) | 1.00 / 1.07 / 1.12 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.07 / 1.49 / 2.24 (2.11–2.85) | 1.00 / 1.07 / 2.98 (2.78–9.86) | 1.90 / 2.30 / 7.42 (2.08–884.74) | 1.00 / 1.70 / 3.10 (3.04–8.32) | 2.08 / 2.37 / 5.89 (2.14–83.97) | 1.00 / 1.70 / 3.07 (3.04–12.16) | 75% | 625 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.04–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.25) | 1.00 / 1.07 / 1.12 (1.07–1.20) | 1.12 / 2.00 / 2.53 (2.14–3.90) | 1.00 / 1.12 / 2.91 (2.72–2.98) | 3.20 / 3.46 / 9.09 (7.23–13.95) | 1.00 / 2.27 / 3.07 (3.04–3.20) | 3.14 / 3.49 / 10.75 (9.47–32.51) | 1.00 / 2.24 / 3.07 (3.07–24.32) | 80% | 407 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–5.95) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.20) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.15 / 2.08 / 2.40 (2.37–3.39) | 1.00 / 1.15 / 2.78 (2.69–10.37) | 2.72 / 3.81 / 6.78 (4.80–14.98) | 1.00 / 2.27 / 3.20 (3.20–3.58) | 5.63 / 6.78 / 16.64 (16.38–18.69) | 1.00 / 2.69 / 3.33 (3.23–3.74) | 60% | 339 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.12–5.25) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.15 / 2.08 / 2.53 (2.43–2.66) | 1.00 / 1.15 / 2.78 (2.56–3.04) | 3.39 / 4.06 / 10.88 (5.89–163.84) | 1.00 / 2.24 / 3.26 (3.23–9.09) | 8.00 / 9.47 / 311.30 (23.04–3833.86) | 1.00 / 2.85 / 3.87 (3.81–4.29) | 47% | 2042 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.12 / 0.29 / 0.33 (0.33–0.50) | 0.08 / 0.17 / 0.29 (0.29–0.37) | 0.12 / 0.29 / 0.33 (0.33–0.54) | 0.08 / 0.17 / 0.29 (0.29–0.46) | 0.12 / 0.29 / 0.33 (0.33–0.54) | 0.08 / 0.17 / 0.33 (0.29–0.42) | 91% | 18 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.12 / 0.29 / 0.46 (0.46–0.66) | 0.08 / 0.21 / 0.33 (0.33–0.42) | 0.12 / 0.33 / 0.42 (0.37–0.66) | 0.08 / 0.25 / 0.33 (0.33–0.46) | 0.12 / 0.33 / 0.50 (0.37–0.62) | 0.08 / 0.25 / 0.33 (0.33–0.46) | 92% | 115 |
| 10 | 0 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.17 / 0.37 / 0.50 (0.50–0.95) | 0.08 / 0.25 / 0.33 (0.33–0.58) | 0.21 / 0.50 / 0.66 (0.54–0.83) | 0.08 / 0.37 / 0.50 (0.37–0.62) | 0.17 / 0.46 / 0.70 (0.42–0.70) | 0.08 / 0.29 / 0.42 (0.37–0.54) | 85% | 115 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.21 / 0.66 / 0.95 (0.87–1.07) | 0.12 / 0.33 / 0.50 (0.46–0.58) | 0.25 / 0.79 / 1.12 (0.91–5.25) | 0.12 / 0.42 / 0.50 (0.50–0.54) | 0.29 / 0.62 / 0.83 (0.79–1.00) | 0.12 / 0.46 / 0.66 (0.58–0.78) | 59% | 620 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.74 / 1.07 (1.04–1.07) | 0.12 / 0.33 / 0.50 (0.46–0.50) | 0.29 / 0.95 / 1.28 (1.28–4.48) | 0.12 / 0.42 / 0.54 (0.50–0.54) | 0.46 / 0.83 / 2.03 (1.04–9.09) | 0.12 / 0.46 / 0.74 (0.74–0.91) | 47% | 2546 |
| 10 | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.33 (0.25–0.33) | 0.29 / 0.58 / 0.70 (0.50–0.74) | 0.21 / 0.46 / 0.78 (0.66–0.79) | 0.33 / 0.58 / 0.70 (0.50–0.74) | 0.21 / 0.42 / 0.79 (0.66–0.83) | 0.29 / 0.54 / 0.66 (0.54–0.70) | 0.21 / 0.42 / 0.70 (0.66–0.83) | 78% | 197 |
| 10 | 0 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.25 / 0.50 / 0.70 (0.70–0.87) | 0.21 / 0.54 / 0.70 (0.70–0.83) | 0.29 / 0.54 / 0.62 (0.58–10.88) | 0.21 / 0.54 / 0.70 (0.70–1.00) | 0.29 / 0.50 / 0.58 (0.58–1.20) | 0.21 / 0.58 / 0.70 (0.70–0.95) | 89% | 169 |
| 10 | 0 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.29 / 0.29 (0.25–0.33) | 0.29 / 0.62 / 1.00 (0.74–1.04) | 0.21 / 0.58 / 0.87 (0.70–1.41) | 0.33 / 0.66 / 0.87 (0.62–6.78) | 0.21 / 0.74 / 1.15 (1.04–1.25) | 0.29 / 0.58 / 0.87 (0.58–1.04) | 0.21 / 0.74 / 1.20 (0.91–1.33) | 86% | 29 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.91 / 1.25 (1.15–1.41) | 0.25 / 0.70 / 0.91 (0.91–1.04) | 0.42 / 1.15 / 1.57 (1.41–1.87) | 0.25 / 0.83 / 1.33 (1.28–1.33) | 0.91 / 1.41 / 4.00 (2.03–5.50) | 0.25 / 1.07 / 1.62 (1.62–1.70) | 57% | 143 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 1.00 / 1.36 (1.33–1.49) | 0.25 / 0.70 / 0.87 (0.87–0.95) | 0.46 / 1.28 / 1.79 (1.74–1.95) | 0.25 / 0.83 / 1.33 (1.33–1.33) | 1.49 / 2.00 / 9.73 (8.83–311.30) | 0.25 / 1.41 / 2.21 (2.08–2.24) | 46% | 1397 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.04 / 1.07 (1.04–1.15) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.07–3.68) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.04–1.15) | 1.07 / 1.25 / 1.82 (1.33–5.44) | 1.00 / 2.82 / 3.07 (3.07–3.14) | 1.07 / 1.25 / 1.36 (1.33–3.52) | 1.00 / 2.37 / 3.07 (3.07–3.14) | 1.07 / 1.25 / 2.08 (1.33–4.42) | 1.00 / 2.78 / 3.07 (3.07–3.26) | 94% | 46 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.04–2.00) | 1.00 / 1.04 / 1.07 (1.07–1.15) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.04–2.56) | 1.00 / 1.04 / 1.07 (1.04–1.20) | 1.07 / 2.08 / 2.30 (2.24–2.94) | 1.00 / 2.94 / 4.48 (4.48–4.61) | 1.90 / 2.30 / 4.74 (2.69–8.83) | 1.00 / 3.10 / 4.67 (4.29–5.12) | 2.00 / 2.37 / 7.36 (4.42–12.42) | 1.00 / 3.07 / 4.35 (4.22–4.86) | 88% | 309 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.04–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.04–1.07) | 1.12 / 2.14 / 2.50 (2.27–2.62) | 1.00 / 3.10 / 4.74 (4.67–5.25) | 3.23 / 3.52 / 9.60 (7.94–25.09) | 1.04 / 3.81 / 5.44 (4.86–5.70) | 3.23 / 3.58 / 10.50 (4.61–22.78) | 1.00 / 3.78 / 5.25 (4.86–5.57) | 76% | 431 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–14.85) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.15 / 2.30 / 2.78 (2.56–2.94) | 1.04 / 3.94 / 6.34 (6.21–6.53) | 2.24 / 4.42 / 9.47 (6.78–243.71) | 1.04 / 4.48 / 6.46 (6.21–6.72) | 5.63 / 6.27 / 24.32 (12.16–9043.97) | 1.04 / 4.67 / 6.53 (6.40–8.70) | 56% | 1272 |
| 10 | 0 | 1000 | 8 | 1.04 / 1.07 / 1.07 (1.07–18.43) | 1.00 / 1.07 / 1.07 (1.07–1.33) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.20) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.20 / 2.40 / 2.91 (2.85–3.20) | 1.04 / 4.42 / 7.87 (7.87–8.06) | 3.20 / 4.42 / 10.11 (5.38–60.42) | 1.07 / 5.25 / 7.62 (7.42–7.81) | 7.94 / 10.24 / 41.98 (16.26–8650.75) | 1.07 / 5.25 / 7.74 (7.68–8.32) | 46% | 2876 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.29 / 0.33 (0.33–0.54) | 0.08 / 0.17 / 0.29 (0.29–0.46) | 0.12 / 0.29 / 0.33 (0.33–0.50) | 0.08 / 0.17 / 0.29 (0.29–0.42) | 0.12 / 0.29 / 0.33 (0.33–0.50) | 0.08 / 0.17 / 0.29 (0.29–0.42) | 91% | 28 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.12 / 0.42 / 0.66 (0.46–0.87) | 0.08 / 0.25 / 0.46 (0.33–0.54) | 0.25 / 0.54 / 0.66 (0.37–0.79) | 0.08 / 0.33 / 0.50 (0.33–0.54) | 0.25 / 0.54 / 0.66 (0.42–0.70) | 0.08 / 0.29 / 0.46 (0.33–0.54) | 83% | 214 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.12–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.17 / 0.54 / 0.78 (0.50–0.87) | 0.08 / 0.29 / 0.46 (0.33–0.54) | 0.25 / 0.58 / 0.74 (0.54–0.79) | 0.08 / 0.37 / 0.54 (0.37–0.62) | 0.25 / 0.54 / 0.74 (0.42–8.70) | 0.08 / 0.37 / 0.58 (0.37–1.15) | 80% | 77 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.66 / 0.95 (0.91–1.15) | 0.12 / 0.33 / 0.54 (0.50–0.62) | 0.25 / 0.79 / 1.12 (1.00–1.28) | 0.12 / 0.42 / 0.54 (0.50–0.66) | 0.33 / 0.74 / 1.00 (0.79–1.07) | 0.12 / 0.46 / 0.74 (0.66–0.79) | 59% | 302 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.74 / 1.04 (0.95–1.15) | 0.12 / 0.33 / 0.58 (0.50–0.62) | 0.29 / 0.87 / 1.25 (1.15–1.33) | 0.12 / 0.42 / 0.62 (0.58–0.62) | 0.58 / 1.20 / 4.35 (1.49–39845.89) | 0.12 / 0.50 / 0.83 (0.70–0.87) | 46% | 928 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.46 / 0.58 (0.54–0.70) | 0.21 / 0.37 / 0.66 (0.66–0.83) | 0.29 / 0.46 / 0.58 (0.46–0.74) | 0.21 / 0.46 / 0.70 (0.66–0.83) | 0.29 / 0.58 / 0.70 (0.54–0.74) | 0.21 / 0.42 / 0.74 (0.66–0.79) | 84% | 44 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.25–0.33) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.33) | 0.29 / 0.62 / 0.83 (0.66–0.91) | 0.21 / 0.58 / 0.83 (0.70–0.83) | 0.37 / 0.66 / 0.79 (0.58–0.83) | 0.21 / 0.66 / 0.87 (0.70–0.91) | 0.33 / 0.66 / 0.79 (0.58–1.04) | 0.21 / 0.66 / 0.91 (0.70–1.04) | 77% | 291 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.25–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.70 / 1.00 (0.74–1.07) | 0.21 / 0.58 / 0.87 (0.74–0.91) | 0.37 / 0.66 / 0.79 (0.62–0.91) | 0.21 / 0.74 / 1.20 (0.95–1.25) | 0.37 / 0.66 / 0.83 (0.62–5.25) | 0.21 / 0.74 / 1.15 (1.00–1.25) | 78% | 639 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.91 / 1.20 (1.07–1.28) | 0.25 / 0.66 / 1.04 (0.95–1.12) | 0.46 / 1.28 / 1.70 (1.46–1.87) | 0.25 / 0.83 / 1.33 (1.28–1.33) | 0.91 / 1.41 / 2.21 (2.00–8.96) | 0.25 / 1.12 / 1.70 (1.62–1.74) | 58% | 378 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 1.04 / 1.46 (1.28–1.62) | 0.25 / 0.74 / 1.07 (1.04–1.12) | 0.50 / 1.46 / 2.00 (1.79–2.30) | 0.25 / 0.87 / 1.41 (1.41–1.41) | 1.62 / 2.27 / 8.70 (6.66–14.59) | 0.25 / 1.41 / 2.21 (2.21–2.24) | 47% | 1260 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.00 / 1.07 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.07 / 1.25 / 1.36 (1.33–2.69) | 1.00 / 2.56 / 3.07 (3.07–3.20) | 1.07 / 1.25 / 1.36 (1.33–2.08) | 1.00 / 1.20 / 3.07 (3.07–3.07) | 1.07 / 1.25 / 1.49 (1.33–1.66) | 1.00 / 1.41 / 3.07 (3.07–3.46) | 96% | 11 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–4.61) | 1.00 / 1.04 / 1.07 (1.07–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.04 / 1.07 (1.04–4.03) | 1.07 / 2.11 / 2.27 (2.27–7.94) | 1.00 / 2.72 / 3.39 (3.14–5.76) | 1.90 / 2.08 / 6.27 (2.14–14.34) | 1.00 / 3.04 / 4.42 (4.29–8.70) | 1.90 / 2.08 / 5.82 (2.21–8.83) | 1.00 / 3.04 / 4.54 (4.22–7.94) | 95% | 6 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.07 / 1.07 (1.04–3.52) | 1.00 / 1.07 / 1.07 (1.04–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.66) | 1.00 / 1.07 / 1.07 (1.04–4.06) | 1.00 / 1.07 / 1.07 (1.04–5.82) | 1.12 / 2.27 / 2.62 (2.27–7.23) | 1.00 / 2.82 / 4.06 (3.68–5.12) | 3.14 / 3.65 / 10.37 (9.60–16.38) | 1.00 / 3.74 / 5.25 (4.86–6.72) | 3.07 / 3.65 / 10.88 (6.91–13.44) | 1.00 / 3.52 / 4.93 (4.86–6.53) | 86% | 19 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.20) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–5.82) | 1.00 / 1.07 / 1.07 (1.07–3.39) | 1.00 / 1.07 / 1.07 (1.07–4.86) | 1.15 / 2.37 / 2.94 (2.72–8.19) | 1.04 / 3.10 / 4.74 (4.48–16.38) | 2.21 / 4.42 / 7.30 (4.99–88.06) | 1.04 / 4.16 / 6.08 (5.95–8.96) | 5.70 / 6.27 / 16.38 (11.14–87.04) | 1.04 / 4.86 / 6.46 (6.40–8.58) | 60% | 917 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–30.21) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–7.10) | 1.00 / 1.07 / 1.07 (1.07–5.50) | 1.00 / 1.07 / 1.07 (1.07–2.08) | 1.20 / 2.50 / 2.94 (2.91–3.90) | 1.04 / 3.26 / 5.12 (4.99–7.62) | 3.20 / 4.48 / 10.50 (7.74–65.02) | 1.04 / 4.67 / 7.23 (6.78–11.39) | 8.06 / 8.83 / 19.71 (18.18–5636.10) | 1.07 / 5.25 / 7.74 (7.55–10.24) | 47% | 2396 |

Delete (removal) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.66 / 1.25 / 2.72 (0.87–368.64) | 0.66 / 1.36 / 3.52 (0.87–10.62) | 0.62 / 1.00 / 3.62 (0.83–8.96) | 0.58 / 1.62 / 4.16 (0.79–524.29) | 0.58 / 0.79 / 3.49 (0.83–3.74) | 0.25 / 0.37 / 0.46 (0.42–3.23) | 0.25 / 0.33 / 0.42 (0.33–56.83) | 0.25 / 0.37 / 0.46 (0.37–0.95) | 0.25 / 0.29 / 0.42 (0.33–12.93) | 0.25 / 0.42 / 0.78 (0.42–13.82) | 0.21 / 0.33 / 1.04 (0.33–42.50) | 84% | 30 |
| 1 | 0 | 50 | 3 | 0.83 / 1.15 / 10.24 (1.28–5570.56) | 0.83 / 8.96 / 12.93 (5.18–5898.24) | 0.83 / 3.20 / 253.95 (1.15–5701.63) | 0.74 / 1.12 / 28.93 (3.20–892.93) | 0.74 / 1.25 / 7.04 (1.12–8519.68) | 1.00 / 1.54 / 5.06 (1.49–9.47) | 0.74 / 1.46 / 2.27 (1.04–8.45) | 0.29 / 0.62 / 0.79 (0.46–1.90) | 0.29 / 0.58 / 3.36 (0.62–7.62) | 0.33 / 0.62 / 0.74 (0.42–2.69) | 0.25 / 0.58 / 0.74 (0.50–3.26) | 83% | 176 |
| 1 | 0 | 50 | 4 | 1.20 / 1.78 / 5.63 (4.03–12.93) | 1.20 / 1.87 / 8.83 (5.44–5767.17) | 1.07 / 1.78 / 10.24 (4.99–10223.62) | 1.04 / 3.78 / 22.27 (8.06–19922.94) | 0.87 / 3.87 / 26.11 (4.86–7929.86) | 0.91 / 1.49 / 3.33 (1.49–8.70) | 0.83 / 1.41 / 3.90 (1.36–5.89) | 0.29 / 0.58 / 0.79 (0.46–10.24) | 0.25 / 0.58 / 3.39 (0.66–7.23) | 0.25 / 0.54 / 0.87 (0.42–8.45) | 0.29 / 0.62 / 3.26 (0.83–4.16) | 86% | 113 |
| 1 | 0 | 50 | 6 | 1.62 / 3.07 / 11.78 (7.55–446.46) | 1.62 / 3.04 / 16.90 (5.82–679.94) | 1.54 / 3.10 / 23.30 (8.58–256.00) | 1.54 / 2.62 / 10.75 (8.06–74.75) | 1.62 / 3.87 / 29.95 (10.24–22544.38) | 1.87 / 3.68 / 6.27 (5.44–31719.42) | 1.74 / 3.49 / 4.48 (3.68–25952.26) | 1.49 / 2.98 / 10.24 (3.04–13500.42) | 1.49 / 2.69 / 3.49 (3.39–6750.21) | 0.42 / 0.78 / 1.66 (1.00–11.78) | 0.42 / 0.79 / 1.00 (0.91–1.12) | 56% | 393 |
| 1 | 0 | 50 | 8 | 1.90 / 86.02 / 417.79 (11.39–22282.24) | 1.78 / 14.08 / 137.22 (12.80–12320.77) | 1.70 / 6.21 / 28.93 (10.37–6946.82) | 1.87 / 4.48 / 1212.42 (7.94–5898.24) | 1.70 / 3.65 / 389.12 (10.75–4030.46) | 2.14 / 4.10 / 13.70 (5.12–274.43) | 2.24 / 3.81 / 9.09 (5.57–12.80) | 2.03 / 3.52 / 13.18 (4.74–385.02) | 1.57 / 2.56 / 3.87 (3.20–43.52) | 0.83 / 1.28 / 7.74 (3.74–10.62) | 0.42 / 0.79 / 1.07 (0.83–4.61) | 46% | 1452 |
| 1 | 0 | 200 | 2 | 0.91 / 1.20 / 3.26 (1.25–15.74) | 0.91 / 1.25 / 6.66 (2.08–16.64) | 0.78 / 1.07 / 7.74 (1.20–21.76) | 0.78 / 1.12 / 8.00 (1.12–17.66) | 0.78 / 1.12 / 9.86 (3.58–11.01) | 0.37 / 0.50 / 0.54 (0.54–5.25) | 0.37 / 0.46 / 3.26 (0.46–3.49) | 0.37 / 0.46 / 0.62 (0.54–8.58) | 0.37 / 0.42 / 0.58 (0.46–5.95) | 0.37 / 0.46 / 0.58 (0.54–0.83) | 0.37 / 0.46 / 0.62 (0.46–3.49) | 99% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.90 / 6.66 (1.70–9.47) | 1.07 / 1.57 / 8.45 (2.21–565.25) | 0.95 / 1.70 / 8.83 (1.58–21.76) | 0.95 / 1.87 / 8.96 (4.29–3801.09) | 1.00 / 1.57 / 10.88 (2.14–105.47) | 1.07 / 1.49 / 7.81 (1.54–9830.40) | 0.91 / 1.28 / 4.22 (1.46–6.59) | 0.37 / 0.54 / 1.07 (0.58–1.36) | 0.42 / 0.54 / 3.33 (1.00–3.49) | 0.37 / 0.50 / 0.70 (0.50–1.07) | 0.37 / 0.54 / 3.36 (1.00–3.46) | 93% | 59 |
| 1 | 0 | 200 | 4 | 1.41 / 4.00 / 20.22 (4.03–8060.93) | 1.20 / 2.24 / 25.60 (9.86–8126.46) | 1.04 / 1.62 / 151.55 (2.30–638.98) | 1.33 / 3.58 / 76.80 (22.53–1998.85) | 1.36 / 3.90 / 19.71 (10.88–13238.27) | 1.49 / 2.37 / 10.11 (4.16–11403.26) | 1.33 / 2.11 / 3.52 (2.62–6.46) | 0.50 / 0.78 / 1.87 (0.91–6.78) | 0.50 / 0.74 / 2.66 (1.00–4.16) | 0.42 / 0.74 / 0.95 (0.74–5.63) | 0.42 / 0.78 / 3.52 (0.79–8.45) | 78% | 310 |
| 1 | 0 | 200 | 6 | 2.08 / 3.26 / 581.63 (8.00–29884.42) | 1.82 / 2015.23 / 6291.46 (7.81–37224.45) | 1.70 / 28.42 / 2064.38 (12.16–21757.95) | 1.87 / 3.33 / 8126.46 (13.57–46661.63) | 1.79 / 1835.01 / 5963.78 (13.18–31719.42) | 2.08 / 3.90 / 344.06 (8.00–9306.11) | 1.82 / 3.65 / 9.09 (3.26–16252.93) | 1.82 / 3.33 / 16.38 (9.47–231.42) | 1.57 / 2.78 / 4.06 (2.91–12582.91) | 0.87 / 1.25 / 2.03 (1.15–18.18) | 0.54 / 0.87 / 1.20 (1.12–1.20) | 58% | 1313 |
| 1 | 0 | 200 | 8 | 2.00 / 50.18 / 548.86 (12.16–1589.25) | 2.03 / 31.23 / 942.08 (10.37–21495.81) | 1.95 / 16.38 / 139.26 (12.42–43515.90) | 2.03 / 17.15 / 385.02 (10.88–34603.01) | 2.21 / 9175.04 / 14680.06 (13.95–41943.04) | 2.27 / 5177.34 / 10354.69 (6.66–51380.22) | 1.90 / 4.42 / 39.42 (6.53–20447.23) | 2.08 / 4.22 / 109.57 (7.36–46137.34) | 1.70 / 2.94 / 5.63 (4.22–17825.79) | 1.49 / 1.82 / 6.53 (1.95–14.34) | 0.54 / 0.87 / 1.12 (1.07–1.33) | 46% | 1969 |
| 1 | 0 | 1000 | 2 | 2.53 / 3.58 / 10.37 (3.65–8192.00) | 2.53 / 3.58 / 7.36 (6.91–3866.62) | 1.79 / 2.85 / 3.46 (2.94–9.86) | 1.82 / 2.85 / 5.44 (3.68–12.67) | 1.82 / 2.82 / 3.36 (2.94–1523.71) | 1.15 / 1.28 / 1.41 (1.33–4.80) | 1.15 / 1.25 / 1.46 (1.28–7.17) | 1.15 / 1.28 / 1.41 (1.28–6.27) | 1.15 / 1.25 / 1.70 (1.25–5.63) | 1.15 / 1.28 / 4.29 (1.33–10.88) | 1.15 / 1.25 / 1.82 (1.54–3.33) | 96% | 40 |
| 1 | 0 | 1000 | 3 | 2.91 / 4.61 / 13.70 (6.53–19922.94) | 2.98 / 4.86 / 59.39 (6.66–462.85) | 2.27 / 3.81 / 14.08 (10.37–44040.19) | 2.37 / 4.10 / 11.14 (8.45–548.86) | 2.30 / 3.87 / 13.82 (6.91–704.51) | 2.78 / 3.81 / 7.10 (4.67–557.06) | 1.87 / 3.10 / 8.83 (5.89–14.98) | 1.95 / 2.11 / 2.69 (2.11–12.93) | 1.20 / 1.36 / 4.74 (2.21–5.50) | 1.95 / 2.53 / 15.74 (4.93–23.30) | 1.25 / 1.58 / 2.69 (2.37–6.78) | 89% | 104 |
| 1 | 0 | 1000 | 4 | 3.52 / 5.44 / 11.78 (9.73–614.40) | 3.81 / 5.82 / 17.92 (7.30–1736.70) | 2.94 / 4.80 / 11.39 (4.99–3178.50) | 3.04 / 5.25 / 33.79 (9.86–454.66) | 2.94 / 4.86 / 27.90 (13.57–243.71) | 3.74 / 5.63 / 7.94 (6.46–14.72) | 2.40 / 4.10 / 4.67 (3.94–5.31) | 3.33 / 3.65 / 8.83 (3.90–19.20) | 1.25 / 2.21 / 4.74 (2.82–7.62) | 3.23 / 3.68 / 17.15 (3.90–30.21) | 1.28 / 1.95 / 2.78 (2.69–4.35) | 76% | 147 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.14 / 20.48 (11.26–29884.42) | 4.29 / 8.58 / 57.86 (14.34–20971.52) | 3.36 / 5.31 / 12.54 (8.19–16121.86) | 3.39 / 6.59 / 1359.87 (8.83–58720.26) | 3.46 / 6.91 / 37.38 (17.66–9043.97) | 4.86 / 7.74 / 380.93 (8.70–1310.72) | 3.14 / 4.74 / 6.59 (5.57–12.54) | 7.10 / 10.62 / 20.22 (11.90–450.56) | 2.53 / 3.94 / 5.38 (4.86–10.62) | 5.76 / 6.40 / 19.71 (12.29–688.13) | 1.36 / 2.43 / 2.91 (2.72–3.10) | 59% | 549 |
| 1 | 0 | 1000 | 8 | 4.48 / 7.10 / 17.41 (12.16–2359.30) | 4.42 / 8.00 / 36.35 (13.82–19922.94) | 3.87 / 6.78 / 933.89 (13.31–30146.56) | 3.74 / 10.37 / 153.60 (16.64–1703.94) | 3.65 / 9.34 / 210.94 (11.52–2686.98) | 4.99 / 8.96 / 270.34 (13.82–1245.18) | 3.26 / 5.70 / 13.31 (8.19–1851.39) | 10.62 / 17.92 / 348.16 (26.62–1785.86) | 2.66 / 4.06 / 5.18 (4.35–14.21) | 8.06 / 12.03 / 25.86 (8.96–4653.06) | 1.41 / 2.53 / 3.20 (2.78–3.81) | 46% | 764 |
| 1 | 1000 | 50 | 2 | 1.62 / 1.87 / 5.95 (1.90–10.62) | 1.66 / 1.87 / 6.59 (2.08–14.59) | 1.57 / 1.79 / 3.20 (1.82–10.88) | 1.57 / 1.82 / 7.04 (1.87–24.32) | 1.57 / 1.82 / 10.62 (1.82–1916.93) | 0.25 / 0.33 / 0.46 (0.42–1.54) | 0.25 / 0.33 / 0.46 (0.33–3.62) | 0.25 / 0.33 / 0.42 (0.37–1.07) | 0.25 / 0.33 / 1.28 (0.37–3.26) | 0.25 / 0.33 / 0.42 (0.42–0.54) | 0.25 / 0.46 / 3.14 (0.33–5.25) | 99% | 2 |
| 1 | 1000 | 50 | 3 | 2.91 / 4.54 / 25.34 (3.23–11141.12) | 3.04 / 4.10 / 380.93 (8.96–13238.27) | 2.82 / 3.78 / 14.08 (6.91–55.30) | 2.82 / 3.46 / 9.09 (3.23–10485.76) | 2.72 / 3.78 / 7.36 (3.90–1474.56) | 1.87 / 2.69 / 4521.98 (2.24–6422.53) | 1.79 / 2.40 / 62.98 (2.08–10747.90) | 0.37 / 0.66 / 0.83 (0.50–3.10) | 0.29 / 0.58 / 1.00 (0.70–3.39) | 0.37 / 0.66 / 0.87 (0.50–0.87) | 0.33 / 0.62 / 0.83 (0.54–7.74) | 77% | 259 |
| 1 | 1000 | 50 | 4 | 3.10 / 3.87 / 26.62 (8.06–235.52) | 3.10 / 4.74 / 49.66 (5.95–1179.65) | 3.20 / 3.65 / 10.62 (8.83–237.57) | 3.14 / 3.74 / 12.80 (10.24–483.33) | 3.20 / 4.61 / 93.18 (11.01–1622.02) | 2.30 / 3.07 / 7.23 (2.62–3768.32) | 1.90 / 4.67 / 9.09 (4.61–27787.26) | 0.37 / 0.66 / 1.15 (0.50–10.50) | 0.33 / 0.62 / 0.66 (0.54–3.65) | 0.37 / 0.66 / 0.83 (0.62–36.35) | 0.33 / 0.62 / 0.95 (0.83–6.14) | 74% | 201 |
| 1 | 1000 | 50 | 6 | 3.52 / 9437.18 / 29360.13 (13.44–34078.72) | 3.52 / 655.36 / 5832.70 (8.70–22282.24) | 3.52 / 1441.79 / 6553.60 (14.08–25427.97) | 3.49 / 1064.96 / 4587.52 (8.96–35127.30) | 3.55 / 6.21 / 475.14 (10.75–12189.70) | 3.52 / 4.48 / 10.75 (5.70–17825.79) | 3.49 / 4.29 / 5.89 (4.74–1523.71) | 2.43 / 3.65 / 9.73 (4.93–7077.89) | 2.24 / 3.04 / 4.48 (3.36–3178.50) | 0.42 / 0.70 / 1.00 (0.83–2.21) | 0.42 / 0.66 / 0.83 (0.83–1.20) | 57% | 1420 |
| 1 | 1000 | 50 | 8 | 3.65 / 11.01 / 192.51 (12.67–1589.25) | 3.68 / 5.57 / 22.53 (16.64–1392.64) | 3.65 / 8.06 / 77.82 (11.78–54001.66) | 3.65 / 5.70 / 29.44 (19.46–4194.30) | 3.68 / 6.91 / 19.20 (15.23–2326.53) | 3.87 / 4.99 / 20.99 (9.86–282.62) | 3.78 / 4.48 / 7.04 (5.82–29.44) | 2.62 / 3.94 / 112.64 (9.09–1261.57) | 2.53 / 3.74 / 4.99 (4.67–17.41) | 0.83 / 1.28 / 2.56 (1.70–8.70) | 0.42 / 0.79 / 0.95 (0.79–1.07) | 47% | 319 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.24 / 9.86 (7.10–794.62) | 1.90 / 2.24 / 12.80 (6.66–18.43) | 1.78 / 2.11 / 2.37 (2.27–5.31) | 1.78 / 2.11 / 7.04 (4.16–19.71) | 1.74 / 2.21 / 8.13 (2.11–13.18) | 0.37 / 0.50 / 0.58 (0.54–1.20) | 0.37 / 0.46 / 0.91 (0.46–2.94) | 0.37 / 0.46 / 0.87 (0.50–6.27) | 0.37 / 0.50 / 0.95 (0.54–5.57) | 0.37 / 0.50 / 1.07 (0.54–11.14) | 0.37 / 0.74 / 1.25 (0.66–33.79) | 94% | 5 |
| 1 | 1000 | 200 | 3 | 3.33 / 4.10 / 16.90 (3.62–155.65) | 3.14 / 4.16 / 14.46 (3.68–13238.27) | 2.98 / 4.16 / 10.50 (6.53–9568.26) | 2.94 / 4.06 / 13.57 (5.57–31.23) | 2.91 / 4.16 / 9.34 (4.74–12.42) | 2.08 / 2.98 / 5.89 (2.50–22.02) | 1.87 / 2.91 / 3.81 (2.27–86.02) | 0.37 / 0.50 / 0.66 (0.54–1.12) | 0.42 / 0.74 / 3.58 (0.87–3.74) | 0.37 / 0.50 / 0.58 (0.58–5.25) | 0.37 / 0.70 / 0.91 (0.54–4.03) | 86% | 84 |
| 1 | 1000 | 200 | 4 | 3.55 / 4.35 / 12.42 (8.06–55050.24) | 3.52 / 4.54 / 16.90 (14.08–13107.20) | 3.23 / 4.06 / 19.20 (11.01–12320.77) | 3.39 / 4.29 / 11.78 (3.90–14811.14) | 3.52 / 5.57 / 19.20 (10.62–16777.22) | 2.53 / 3.33 / 5.76 (4.99–12.93) | 2.24 / 3.10 / 5.12 (3.65–4849.66) | 0.50 / 0.79 / 1.07 (0.66–3.49) | 0.46 / 0.74 / 1.15 (0.74–3.46) | 0.42 / 0.70 / 1.04 (0.91–3.87) | 0.50 / 0.74 / 1.28 (1.07–3.33) | 85% | 324 |
| 1 | 1000 | 200 | 6 | 3.94 / 5.25 / 516.10 (12.93–1474.56) | 4.06 / 7.17 / 20.74 (16.64–1376.26) | 3.87 / 7.87 / 84.99 (11.26–10223.62) | 3.74 / 5.31 / 565.25 (7.49–4259.84) | 3.94 / 6.21 / 18.43 (11.90–1966.08) | 3.90 / 4.99 / 13.06 (8.58–1425.41) | 3.78 / 4.86 / 5.70 (5.44–6.08) | 2.98 / 4.10 / 9.73 (4.93–12.93) | 2.53 / 3.49 / 6.78 (4.74–8.58) | 1.00 / 1.46 / 9.34 (2.50–13.18) | 0.58 / 1.00 / 1.25 (1.15–1.36) | 59% | 345 |
| 1 | 1000 | 200 | 8 | 4.06 / 9.73 / 57.86 (48.64–1007.62) | 4.10 / 25.09 / 1048.58 (17.66–8060.93) | 3.90 / 9.86 / 40.96 (12.80–974.85) | 4.00 / 5.89 / 23.04 (14.59–1097.73) | 3.94 / 8.83 / 47.62 (14.98–1605.63) | 4.16 / 6.21 / 24.06 (12.67–5570.56) | 3.94 / 5.25 / 9.60 (7.10–24117.25) | 2.91 / 4.74 / 33.79 (6.91–4718.59) | 2.53 / 3.58 / 46.08 (7.36–6094.85) | 1.49 / 2.27 / 28.16 (7.55–2490.37) | 0.54 / 0.91 / 1.20 (1.12–9.86) | 46% | 956 |
| 1 | 1000 | 1000 | 2 | 3.52 / 4.93 / 14.46 (5.12–30.72) | 3.49 / 4.61 / 9.34 (4.61–3997.70) | 3.07 / 12.42 / 56.83 (3.87–8388.61) | 2.85 / 4.16 / 385.02 (9.47–10354.69) | 2.91 / 4.35 / 7.04 (3.87–5898.24) | 1.15 / 1.49 / 1.82 (1.33–12.67) | 1.15 / 1.49 / 1.79 (1.25–6.40) | 1.25 / 1.62 / 1.79 (1.33–7.55) | 1.20 / 1.57 / 2.53 (1.70–12.93) | 1.15 / 1.33 / 1.82 (1.33–4.74) | 1.15 / 1.54 / 1.95 (1.66–8.96) | 80% | 155 |
| 1 | 1000 | 1000 | 3 | 4.99 / 7.10 / 29.70 (14.46–1146.88) | 4.99 / 7.04 / 13.44 (9.34–16777.22) | 4.29 / 6.21 / 54.78 (8.96–9175.04) | 4.29 / 6.27 / 16.00 (6.72–23855.10) | 4.29 / 6.59 / 18.18 (13.31–16515.07) | 4.00 / 5.95 / 688.13 (15.49–9437.18) | 3.23 / 8.58 / 3178.50 (15.62–10092.54) | 2.00 / 2.43 / 6.27 (2.14–4521.98) | 1.25 / 1.70 / 3.26 (2.21–15.49) | 2.14 / 2.43 / 3.52 (2.11–5046.27) | 1.25 / 1.90 / 3.33 (2.62–4.80) | 75% | 625 |
| 1 | 1000 | 1000 | 4 | 5.38 / 7.10 / 14.21 (9.60–25690.11) | 5.44 / 8.58 / 540.67 (11.14–17563.65) | 4.61 / 8.06 / 24.06 (10.24–16646.14) | 4.74 / 6.59 / 22.02 (11.52–6225.92) | 4.80 / 8.06 / 413.70 (16.64–3964.93) | 4.67 / 6.34 / 13.44 (10.37–62.46) | 3.23 / 4.74 / 9.34 (6.08–1572.86) | 3.23 / 3.52 / 9.98 (6.66–24.06) | 1.25 / 1.79 / 4.16 (2.50–5.63) | 3.20 / 3.58 / 7.55 (5.25–22.53) | 1.20 / 2.30 / 4.42 (4.10–5111.81) | 80% | 407 |
| 1 | 1000 | 1000 | 6 | 6.08 / 11.14 / 401.41 (241.66–860.16) | 6.14 / 8.45 / 92.16 (16.13–30670.85) | 5.31 / 7.81 / 34.82 (13.82–1785.86) | 5.38 / 11.78 / 1032.19 (446.46–2981.89) | 5.31 / 8.70 / 462.85 (14.34–2392.06) | 5.82 / 8.06 / 15.62 (10.75–786.43) | 4.67 / 6.27 / 12.80 (7.04–5505.02) | 7.10 / 12.54 / 24.58 (15.10–4653.06) | 3.46 / 4.74 / 12.16 (5.57–14.21) | 5.70 / 6.46 / 17.15 (10.50–47.62) | 1.36 / 2.53 / 2.91 (2.72–2.98) | 60% | 339 |
| 1 | 1000 | 1000 | 8 | 6.40 / 30.98 / 274.43 (25.86–30932.99) | 6.53 / 311.30 / 2752.51 (20.22–7077.89) | 5.89 / 23.04 / 909.31 (79.87–5832.70) | 5.70 / 307.20 / 999.42 (37.38–5439.49) | 5.63 / 450.56 / 3276.80 (141.31–11141.12) | 6.14 / 10.75 / 557.06 (18.69–7536.64) | 5.06 / 7.04 / 11.14 (8.06–1245.18) | 10.50 / 483.33 / 1130.50 (151.55–6684.67) | 3.81 / 5.12 / 6.72 (5.82–2523.14) | 8.06 / 8.83 / 520.19 (24.58–3833.86) | 1.36 / 2.56 / 2.82 (2.72–9.34) | 47% | 2042 |
| 10 | 0 | 50 | 2 | 0.62 / 0.87 / 7.55 (1.04–23.81) | 0.62 / 0.87 / 6.08 (2.24–7.04) | 0.58 / 0.79 / 3.33 (0.91–9.22) | 0.58 / 0.83 / 7.23 (2.43–27.39) | 0.58 / 0.79 / 4.86 (2.94–67.58) | 0.25 / 0.37 / 0.50 (0.46–0.74) | 0.21 / 0.37 / 0.54 (0.42–0.66) | 0.25 / 0.37 / 0.50 (0.42–0.83) | 0.21 / 0.33 / 0.46 (0.42–0.74) | 0.25 / 0.37 / 0.50 (0.46–0.83) | 0.21 / 0.37 / 0.54 (0.46–0.74) | 91% | 18 |
| 10 | 0 | 50 | 3 | 0.83 / 1.49 / 11.78 (9.34–15.10) | 0.83 / 1.20 / 18.18 (5.95–35.84) | 0.74 / 1.15 / 7.23 (5.31–18.43) | 0.74 / 1.66 / 8.58 (2.91–32.51) | 0.74 / 1.12 / 6.98 (4.80–13893.63) | 0.79 / 1.28 / 9.47 (1.41–3309.57) | 0.74 / 1.15 / 7.04 (1.33–1998.85) | 0.25 / 0.46 / 0.58 (0.54–0.87) | 0.25 / 0.42 / 0.54 (0.54–0.91) | 0.25 / 0.46 / 0.70 (0.54–0.87) | 0.25 / 0.46 / 0.58 (0.54–0.87) | 92% | 115 |
| 10 | 0 | 50 | 4 | 1.25 / 2.11 / 12.29 (7.36–1900.54) | 1.25 / 2.11 / 8.70 (7.49–454.66) | 0.95 / 1.87 / 4.48 (3.33–21.76) | 1.20 / 2.03 / 8.00 (4.54–10.37) | 1.25 / 2.27 / 6.46 (3.52–172.03) | 0.95 / 1.62 / 2.30 (1.95–6.91) | 0.83 / 1.66 / 3.04 (1.70–6.02) | 0.33 / 0.66 / 0.87 (0.70–1.07) | 0.37 / 0.74 / 0.95 (0.58–1.25) | 0.25 / 0.66 / 0.87 (0.54–0.91) | 0.25 / 0.58 / 0.95 (0.58–0.95) | 85% | 115 |
| 10 | 0 | 50 | 6 | 1.54 / 6.08 / 15.10 (8.19–8126.46) | 1.49 / 5.25 / 14.21 (5.95–6291.46) | 1.49 / 3.14 / 14.21 (9.86–56.83) | 1.49 / 5.82 / 37.89 (7.55–106.50) | 1.46 / 3.26 / 11.39 (7.81–18612.22) | 1.87 / 3.68 / 10.62 (4.86–720.90) | 1.66 / 3.52 / 6.59 (4.22–5046.27) | 1.57 / 3.10 / 14.08 (6.66–1769.47) | 1.36 / 2.66 / 4.48 (3.46–819.20) | 0.42 / 0.74 / 1.07 (0.91–1.15) | 0.42 / 0.87 / 1.15 (1.07–1.33) | 59% | 620 |
| 10 | 0 | 50 | 8 | 1.74 / 4.93 / 524.29 (13.18–22020.10) | 1.74 / 5.12 / 335.87 (8.70–19922.94) | 1.74 / 4.86 / 217.09 (11.14–7274.50) | 1.74 / 38.91 / 2048.00 (8.83–9437.18) | 1.74 / 245.76 / 3407.87 (9.09–6094.85) | 2.14 / 4.67 / 20.48 (9.98–925.70) | 2.11 / 4.61 / 7.68 (5.63–5701.63) | 1.90 / 4.06 / 19.97 (5.31–884.74) | 1.62 / 3.46 / 6.85 (4.61–22.53) | 0.54 / 0.91 / 2.30 (1.15–8.19) | 0.46 / 0.91 / 1.28 (1.15–1.66) | 47% | 2546 |
| 10 | 0 | 200 | 2 | 0.91 / 1.20 / 5.44 (1.33–73.73) | 0.91 / 1.28 / 6.40 (1.33–5242.88) | 0.78 / 1.12 / 3.46 (1.20–778.24) | 0.78 / 1.20 / 5.38 (1.25–219.14) | 0.79 / 1.57 / 294.91 (1.90–4980.74) | 0.42 / 0.74 / 0.95 (0.58–1.00) | 0.33 / 0.70 / 0.91 (0.66–0.95) | 0.42 / 0.78 / 0.95 (0.58–0.95) | 0.37 / 0.70 / 0.95 (0.66–1.04) | 0.37 / 0.66 / 0.91 (0.62–0.95) | 0.37 / 0.58 / 0.70 (0.66–0.95) | 78% | 197 |
| 10 | 0 | 200 | 3 | 1.07 / 1.62 / 7.23 (3.94–11.78) | 1.25 / 2.03 / 8.19 (1.82–72.70) | 1.00 / 1.66 / 10.24 (5.57–14.46) | 1.00 / 1.54 / 6.78 (4.16–14286.85) | 0.95 / 1.54 / 6.21 (2.94–8060.93) | 1.07 / 1.54 / 4.86 (2.62–33.28) | 0.91 / 1.54 / 5.50 (1.79–1376.26) | 0.37 / 0.66 / 0.83 (0.70–33.28) | 0.37 / 0.62 / 0.83 (0.79–1.20) | 0.37 / 0.58 / 0.70 (0.70–1.07) | 0.37 / 0.66 / 0.83 (0.78–1.74) | 89% | 169 |
| 10 | 0 | 200 | 4 | 1.20 / 2.43 / 9.86 (7.74–18.18) | 1.20 / 2.37 / 10.11 (5.44–23.04) | 1.04 / 1.90 / 12.42 (8.58–22.78) | 1.04 / 2.21 / 17.66 (11.78–24.58) | 1.25 / 2.40 / 8.70 (1.87–237.57) | 1.25 / 2.40 / 5.38 (2.91–43.01) | 1.04 / 2.56 / 3.65 (2.03–10.24) | 0.42 / 0.83 / 1.04 (0.74–6.08) | 0.46 / 0.95 / 1.41 (1.20–1.57) | 0.42 / 0.74 / 1.00 (0.74–1.20) | 0.50 / 1.00 / 1.25 (0.83–1.57) | 86% | 29 |
| 10 | 0 | 200 | 6 | 2.11 / 4.35 / 10.88 (10.37–71.68) | 1.87 / 3.62 / 12.54 (6.91–38.40) | 1.70 / 3.55 / 11.65 (7.74–442.37) | 1.66 / 3.55 / 12.54 (7.87–454.66) | 1.70 / 3.36 / 9.73 (7.62–13.31) | 2.27 / 4.74 / 7.30 (6.66–116.74) | 1.90 / 3.78 / 6.40 (4.74–6.72) | 2.08 / 4.03 / 7.30 (5.25–10.75) | 1.62 / 3.10 / 4.74 (4.00–8.32) | 1.00 / 1.57 / 4.86 (2.14–7.62) | 0.66 / 1.36 / 1.79 (1.54–1.82) | 57% | 143 |
| 10 | 0 | 200 | 8 | 2.08 / 4.99 / 13.44 (12.42–7208.96) | 2.08 / 5.12 / 36.86 (14.72–3899.39) | 1.95 / 4.86 / 15.87 (10.11–6356.99) | 1.95 / 5.82 / 35.84 (20.99–19398.66) | 1.95 / 4.86 / 14.34 (10.88–5177.34) | 2.66 / 5.76 / 13.44 (9.60–115.71) | 2.27 / 4.74 / 9.98 (7.81–12.03) | 2.40 / 5.12 / 9.98 (9.09–49.15) | 1.78 / 3.68 / 5.82 (4.93–8.00) | 1.58 / 2.37 / 9.86 (8.96–323.58) | 0.58 / 1.36 / 1.82 (1.74–1.90) | 46% | 1397 |
| 10 | 0 | 1000 | 2 | 2.50 / 3.55 / 7.17 (6.78–2129.92) | 2.50 / 3.62 / 11.01 (8.45–32.00) | 1.78 / 2.82 / 6.91 (3.87–860.16) | 1.82 / 2.82 / 8.58 (3.39–8.83) | 1.87 / 2.82 / 10.24 (4.42–15.36) | 1.15 / 1.36 / 2.85 (1.46–7.81) | 1.15 / 1.41 / 2.14 (1.82–3.78) | 1.15 / 1.33 / 1.46 (1.41–5.31) | 1.15 / 1.41 / 2.21 (1.74–4.10) | 1.15 / 1.33 / 1.46 (1.46–4.99) | 1.15 / 1.41 / 1.82 (1.62–6.27) | 94% | 46 |
| 10 | 0 | 1000 | 3 | 2.78 / 4.61 / 8.83 (5.95–25952.26) | 2.82 / 4.74 / 13.18 (10.75–13107.20) | 2.27 / 3.78 / 10.88 (6.72–3211.26) | 2.40 / 4.16 / 16.64 (8.06–9961.47) | 2.37 / 3.87 / 19.71 (10.37–6488.06) | 2.78 / 4.42 / 7.17 (5.57–1966.08) | 1.90 / 3.58 / 9.34 (6.40–10092.54) | 1.95 / 2.43 / 5.57 (2.27–7.74) | 1.20 / 2.53 / 3.07 (2.56–3.26) | 2.03 / 2.40 / 9.73 (3.36–12.29) | 1.20 / 2.53 / 2.85 (2.40–2.98) | 88% | 309 |
| 10 | 0 | 1000 | 4 | 3.62 / 5.82 / 15.10 (8.32–5570.56) | 3.62 / 6.02 / 22.27 (13.31–753.66) | 2.91 / 5.44 / 1327.10 (8.45–8060.93) | 2.91 / 5.25 / 16.64 (8.00–1851.39) | 2.91 / 5.12 / 18.43 (11.65–4915.20) | 3.65 / 6.78 / 11.52 (7.42–3440.64) | 2.14 / 4.00 / 5.18 (4.22–4325.38) | 3.26 / 3.65 / 9.09 (6.40–532.48) | 1.33 / 2.72 / 3.78 (3.46–4.06) | 3.26 / 3.68 / 11.14 (3.94–20.99) | 1.28 / 2.69 / 3.78 (3.49–4.16) | 76% | 431 |
| 10 | 0 | 1000 | 6 | 4.22 / 7.62 / 18.18 (11.39–137.22) | 4.29 / 7.62 / 17.41 (13.82–925.70) | 3.49 / 7.23 / 25.60 (10.88–3309.57) | 3.55 / 7.30 / 19.20 (10.24–35127.30) | 3.49 / 7.68 / 15.10 (11.39–19922.94) | 5.12 / 9.60 / 14.59 (12.16–9306.11) | 3.04 / 6.08 / 8.96 (8.06–7602.18) | 7.04 / 12.29 / 19.46 (17.66–12320.77) | 2.62 / 5.50 / 9.09 (6.72–14680.06) | 5.76 / 6.53 / 18.94 (12.80–5570.56) | 1.41 / 3.62 / 4.99 (3.87–7733.25) | 56% | 1272 |
| 10 | 0 | 1000 | 8 | 4.80 / 24.58 / 1458.18 (14.72–28835.84) | 4.86 / 202.75 / 2080.77 (16.38–20971.52) | 3.90 / 8.83 / 491.52 (12.93–9961.47) | 3.94 / 59.39 / 438.27 (12.42–33554.43) | 3.94 / 20.74 / 860.16 (14.21–14417.92) | 5.70 / 12.16 / 290.82 (18.18–11665.41) | 3.36 / 7.49 / 53.76 (9.98–4784.13) | 9.86 / 18.43 / 843.78 (24.83–6750.21) | 2.85 / 6.27 / 8.96 (8.45–122.88) | 8.06 / 11.01 / 80.90 (16.64–5701.63) | 1.41 / 4.03 / 5.38 (5.25–7.04) | 46% | 2876 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 6.27 (1.95–679.94) | 1.62 / 1.87 / 6.85 (3.14–76.80) | 1.57 / 1.79 / 5.44 (1.87–9.34) | 1.57 / 1.79 / 6.53 (1.90–8.06) | 1.57 / 1.79 / 5.70 (1.90–7.94) | 0.25 / 0.37 / 0.46 (0.46–0.79) | 0.21 / 0.33 / 0.46 (0.42–0.74) | 0.25 / 0.37 / 0.46 (0.46–0.78) | 0.21 / 0.33 / 0.46 (0.42–0.74) | 0.25 / 0.37 / 0.46 (0.46–0.79) | 0.21 / 0.33 / 0.46 (0.42–0.78) | 91% | 28 |
| 10 | 1000 | 50 | 3 | 2.72 / 3.20 / 11.90 (3.36–3637.25) | 2.72 / 3.14 / 8.83 (3.39–3145.73) | 2.69 / 3.20 / 8.96 (6.40–5636.10) | 2.66 / 3.46 / 10.88 (4.74–3964.93) | 2.66 / 4.00 / 10.88 (7.74–5111.81) | 1.95 / 2.85 / 4.99 (3.20–204.80) | 1.95 / 2.69 / 6.53 (2.27–8.06) | 0.37 / 0.74 / 0.91 (0.54–1.07) | 0.33 / 0.70 / 0.91 (0.54–1.12) | 0.37 / 0.70 / 0.91 (0.62–1.36) | 0.29 / 0.66 / 0.87 (0.58–1.15) | 83% | 214 |
| 10 | 1000 | 50 | 4 | 3.20 / 4.42 / 15.49 (6.34–2359.30) | 3.14 / 4.16 / 12.29 (10.11–15.36) | 2.98 / 4.29 / 11.78 (7.23–16.26) | 3.07 / 4.35 / 11.78 (5.18–1425.41) | 3.07 / 5.50 / 10.88 (9.73–22.53) | 2.24 / 3.81 / 4.99 (2.98–13.44) | 2.03 / 3.39 / 4.48 (2.78–20.48) | 0.37 / 0.70 / 0.91 (0.70–1.07) | 0.33 / 0.78 / 1.00 (0.58–1.25) | 0.33 / 0.74 / 0.95 (0.58–9.09) | 0.37 / 0.78 / 1.20 (0.58–6.98) | 80% | 77 |
| 10 | 1000 | 50 | 6 | 3.55 / 6.14 / 13.44 (10.11–925.70) | 3.52 / 5.76 / 14.72 (10.75–638.98) | 3.46 / 6.14 / 13.31 (10.24–133.12) | 3.62 / 5.50 / 11.90 (10.11–210.94) | 3.68 / 7.81 / 15.49 (13.82–122.88) | 3.07 / 5.12 / 7.81 (6.78–9.86) | 3.07 / 5.18 / 8.19 (6.66–11.01) | 2.40 / 4.00 / 6.14 (4.86–1040.38) | 2.21 / 3.62 / 5.25 (4.61–33.28) | 0.46 / 0.91 / 1.15 (1.00–1.46) | 0.46 / 1.04 / 1.33 (1.07–1.36) | 59% | 302 |
| 10 | 1000 | 50 | 8 | 3.65 / 6.34 / 17.66 (11.39–3604.48) | 3.68 / 6.53 / 20.48 (11.78–1228.80) | 3.62 / 6.72 / 17.66 (12.42–2359.30) | 3.65 / 6.21 / 19.97 (12.67–1310.72) | 3.90 / 9.73 / 24.06 (15.23–1654.78) | 3.62 / 5.76 / 9.98 (6.98–192.51) | 3.62 / 5.82 / 9.86 (7.81–11534.34) | 2.62 / 4.99 / 8.96 (6.27–638.98) | 2.37 / 4.42 / 5.31 (5.25–5.70) | 0.70 / 1.36 / 4.61 (1.79–17825.79) | 0.50 / 1.07 / 1.41 (1.20–1.62) | 46% | 928 |
| 10 | 1000 | 200 | 2 | 1.87 / 2.21 / 4.99 (2.82–10.11) | 1.90 / 2.21 / 7.87 (2.30–8.83) | 1.74 / 2.08 / 6.34 (2.24–7.17) | 1.74 / 2.08 / 4.61 (2.43–8.00) | 1.74 / 2.30 / 4.03 (2.21–417.79) | 0.37 / 0.54 / 0.66 (0.66–1.00) | 0.37 / 0.54 / 0.74 (0.66–1.90) | 0.37 / 0.54 / 0.70 (0.58–1.00) | 0.37 / 0.62 / 0.79 (0.66–1.04) | 0.37 / 0.78 / 0.95 (0.62–1.04) | 0.37 / 0.66 / 0.91 (0.66–0.95) | 84% | 44 |
| 10 | 1000 | 200 | 3 | 3.04 / 4.42 / 18.94 (11.65–4849.66) | 3.07 / 4.16 / 12.16 (8.32–2392.06) | 2.94 / 4.16 / 15.36 (7.68–10354.69) | 2.94 / 4.06 / 9.86 (5.31–8519.68) | 2.91 / 4.86 / 10.75 (5.12–6291.46) | 2.14 / 2.98 / 7.10 (5.38–11.78) | 2.11 / 2.91 / 6.78 (2.56–12.03) | 0.50 / 0.83 / 0.95 (0.70–1.12) | 0.42 / 0.87 / 1.07 (0.74–1.12) | 0.46 / 0.83 / 0.95 (0.70–2.40) | 0.42 / 0.87 / 1.12 (0.79–1.15) | 77% | 291 |
| 10 | 1000 | 200 | 4 | 3.49 / 4.61 / 12.93 (9.73–7602.18) | 3.49 / 4.48 / 10.75 (4.99–6029.31) | 3.23 / 4.29 / 17.41 (9.60–18087.94) | 3.26 / 4.42 / 10.50 (8.58–7602.18) | 3.39 / 5.57 / 16.00 (10.11–9830.40) | 2.50 / 4.10 / 7.49 (3.81–6422.53) | 2.24 / 3.62 / 4.29 (3.04–165.89) | 0.50 / 0.79 / 0.95 (0.78–1.12) | 0.46 / 0.95 / 1.25 (0.91–1.41) | 0.50 / 0.83 / 1.00 (0.74–6.21) | 0.46 / 1.00 / 1.20 (0.95–1.36) | 78% | 639 |
| 10 | 1000 | 200 | 6 | 3.81 / 5.95 / 12.93 (10.24–19.97) | 3.81 / 5.57 / 12.42 (9.73–499.71) | 3.65 / 5.57 / 12.93 (10.62–14.85) | 3.68 / 5.50 / 12.54 (10.88–1687.55) | 3.81 / 8.32 / 15.62 (13.06–9175.04) | 3.46 / 5.82 / 9.34 (6.53–19.46) | 3.23 / 5.25 / 6.66 (5.70–21.76) | 3.04 / 4.99 / 8.00 (7.10–9.22) | 2.40 / 4.00 / 4.99 (4.54–6.21) | 1.00 / 1.58 / 3.07 (2.43–9.22) | 0.58 / 1.20 / 1.62 (1.49–1.79) | 58% | 378 |
| 10 | 1000 | 200 | 8 | 4.03 / 6.91 / 15.49 (13.82–7995.39) | 4.06 / 8.19 / 36.86 (11.39–5701.63) | 3.90 / 7.10 / 16.64 (12.93–933.89) | 3.90 / 6.72 / 15.36 (11.90–2490.37) | 4.22 / 11.52 / 35.33 (15.23–7012.35) | 4.10 / 7.81 / 21.25 (10.24–376.83) | 4.06 / 6.85 / 9.73 (8.19–159.74) | 3.20 / 6.66 / 13.82 (11.90–737.28) | 2.53 / 4.54 / 5.76 (5.44–7.23) | 1.70 / 2.43 / 8.70 (5.38–19.46) | 0.58 / 1.36 / 1.82 (1.78–2.40) | 47% | 1260 |
| 10 | 1000 | 1000 | 2 | 3.49 / 4.61 / 7.74 (5.31–13.31) | 3.49 / 4.86 / 6.08 (5.57–13.82) | 2.78 / 3.87 / 8.96 (3.90–10.75) | 2.78 / 3.87 / 8.32 (6.66–14.85) | 2.78 / 3.87 / 11.65 (9.34–24.32) | 1.15 / 1.33 / 2.11 (1.46–5.50) | 1.15 / 1.36 / 1.74 (1.66–3.26) | 1.15 / 1.33 / 1.74 (1.49–1.95) | 1.15 / 1.33 / 1.66 (1.49–2.00) | 1.15 / 1.33 / 1.49 (1.46–1.87) | 1.15 / 1.36 / 1.74 (1.54–4.10) | 96% | 11 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.53 / 11.01 (9.34–20.74) | 4.61 / 6.53 / 10.37 (7.10–20.99) | 3.90 / 5.82 / 15.36 (10.37–24.06) | 3.90 / 5.89 / 15.23 (7.42–24.83) | 3.94 / 7.04 / 23.30 (8.00–27.65) | 3.68 / 5.95 / 11.26 (7.74–17.41) | 2.85 / 4.06 / 5.12 (4.99–14.08) | 1.95 / 2.14 / 3.78 (2.27–16.13) | 1.20 / 2.30 / 2.62 (2.50–9.34) | 1.95 / 2.14 / 6.34 (2.24–9.98) | 1.20 / 2.30 / 3.20 (2.50–8.06) | 95% | 6 |
| 10 | 1000 | 1000 | 4 | 5.44 / 7.94 / 15.62 (13.82–26.88) | 5.44 / 8.06 / 12.54 (8.58–19.46) | 4.67 / 7.04 / 15.62 (12.16–26.62) | 4.61 / 6.98 / 12.67 (12.29–33.79) | 4.74 / 8.83 / 19.46 (9.47–33.28) | 4.80 / 8.06 / 12.03 (10.88–21.50) | 3.26 / 5.12 / 6.78 (6.21–16.38) | 3.20 / 3.81 / 11.52 (9.60–14.72) | 1.28 / 2.72 / 4.03 (3.39–9.09) | 3.14 / 3.74 / 9.86 (3.68–16.26) | 1.20 / 2.40 / 3.78 (3.46–4.03) | 86% | 19 |
| 10 | 1000 | 1000 | 6 | 6.21 / 9.86 / 18.94 (16.00–131.07) | 6.34 / 9.86 / 18.94 (16.00–5701.63) | 5.44 / 9.60 / 21.76 (16.00–22020.10) | 5.38 / 8.96 / 30.98 (13.57–8323.07) | 5.50 / 11.26 / 23.55 (15.10–10485.76) | 5.82 / 10.62 / 15.87 (13.82–876.54) | 4.48 / 7.74 / 10.62 (9.60–364.54) | 7.17 / 12.67 / 17.66 (16.64–1687.55) | 3.46 / 6.27 / 9.34 (7.36–111.62) | 5.82 / 6.46 / 17.92 (11.26–68.61) | 1.41 / 3.74 / 4.99 (4.16–8.70) | 60% | 917 |
| 10 | 1000 | 1000 | 8 | 6.59 / 11.26 / 17.92 (16.64–31981.57) | 6.78 / 19.46 / 169.98 (19.20–29360.13) | 5.89 / 11.78 / 33.28 (18.43–17563.65) | 5.82 / 12.03 / 26.88 (15.36–17563.65) | 6.08 / 14.85 / 30.21 (17.92–12189.70) | 6.66 / 13.18 / 20.74 (20.22–7536.64) | 5.06 / 9.86 / 14.34 (12.42–11665.41) | 9.98 / 19.46 / 33.28 (25.09–36175.87) | 3.52 / 6.66 / 11.01 (8.70–8912.90) | 8.19 / 9.09 / 20.22 (16.90–5505.02) | 1.41 / 4.03 / 5.63 (5.31–11.39) | 47% | 2396 |

Derive (addition) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.66 / 1.25 / 3.81 (0.87–161.79) | 0.33 / 1.15 / 1.62 (0.87–8.45) | 0.62 / 1.04 / 1.95 (0.79–8.83) | 0.37 / 1.33 / 6.98 (1.79–22.02) | 0.25 / 1.07 / 5.38 (0.83–8.70) | 0.25 / 0.33 / 0.50 (0.37–11.65) | 0.25 / 0.33 / 0.42 (0.33–3.87) | 0.25 / 0.37 / 0.42 (0.37–18.69) | 0.25 / 0.33 / 3.36 (0.37–23.81) | 0.25 / 0.50 / 0.79 (0.37–3.49) | 0.25 / 0.33 / 0.87 (0.33–1.04) | 84% | 30 |
| 1 | 0 | 50 | 3 | 0.83 / 1.12 / 6.72 (1.15–3637.25) | 0.62 / 0.91 / 8.32 (3.46–3702.78) | 0.83 / 3.52 / 110.59 (1.15–8781.82) | 0.54 / 0.87 / 4.06 (1.15–9.34) | 0.54 / 0.91 / 3.23 (0.83–2015.23) | 1.00 / 1.57 / 6.53 (3.49–9.86) | 0.37 / 1.15 / 1.33 (0.87–4.54) | 0.29 / 0.62 / 0.74 (0.42–2.94) | 0.29 / 0.58 / 3.39 (0.74–4.00) | 0.33 / 0.62 / 0.70 (0.42–0.83) | 0.25 / 0.58 / 0.79 (0.37–3.81) | 83% | 176 |
| 1 | 0 | 50 | 4 | 1.20 / 2.21 / 9.73 (2.56–18.94) | 0.74 / 1.36 / 3.94 (1.62–323.58) | 1.07 / 1.70 / 9.34 (4.67–9830.40) | 0.66 / 1.28 / 7.04 (5.82–9699.33) | 0.62 / 1.15 / 5.12 (1.04–6291.46) | 0.95 / 1.49 / 2.43 (1.49–10.11) | 0.62 / 1.15 / 4.06 (1.28–5.31) | 0.29 / 0.62 / 3.94 (0.46–5.12) | 0.25 / 0.62 / 3.26 (0.58–4.61) | 0.25 / 0.62 / 1.79 (0.46–11.01) | 0.29 / 0.66 / 0.83 (0.66–4.86) | 86% | 113 |
| 1 | 0 | 50 | 6 | 1.62 / 2.94 / 9.22 (8.83–442.37) | 1.04 / 2.00 / 7.42 (2.69–35.84) | 1.49 / 4.10 / 161.79 (9.34–1212.42) | 0.95 / 1.87 / 5.70 (3.36–1114.11) | 1.00 / 1.95 / 8.32 (2.50–12976.13) | 1.87 / 3.62 / 10.37 (5.57–28049.41) | 1.12 / 2.30 / 3.04 (2.43–6619.14) | 1.49 / 2.91 / 8.70 (4.42–12189.70) | 0.62 / 2.14 / 2.62 (2.24–5.70) | 0.42 / 0.79 / 1.12 (0.91–14680.06) | 0.42 / 0.79 / 1.04 (0.87–1.15) | 56% | 393 |
| 1 | 0 | 50 | 8 | 1.90 / 76.80 / 532.48 (12.29–24903.68) | 1.12 / 2.50 / 26.88 (8.58–8519.68) | 1.70 / 6.08 / 62.46 (15.62–6946.82) | 1.15 / 2.40 / 6.59 (3.52–1425.41) | 1.07 / 2.56 / 8.06 (3.55–540.67) | 2.14 / 4.10 / 7.68 (5.95–21.50) | 1.36 / 2.40 / 3.14 (2.94–4.93) | 2.03 / 3.49 / 8.96 (4.48–565.25) | 0.83 / 2.11 / 3.46 (2.69–9.09) | 0.83 / 1.28 / 2.27 (0.91–8.58) | 0.42 / 0.78 / 1.00 (0.87–3.14) | 46% | 1452 |
| 1 | 0 | 200 | 2 | 0.91 / 1.20 / 3.90 (2.30–14.21) | 0.37 / 1.15 / 1.28 (1.20–24.83) | 0.78 / 1.07 / 6.59 (1.12–27.14) | 0.37 / 1.07 / 4.48 (1.20–6.27) | 0.42 / 1.07 / 7.10 (1.15–16.38) | 0.37 / 0.46 / 0.66 (0.50–6.53) | 0.33 / 0.46 / 6.40 (3.26–12.93) | 0.37 / 0.50 / 0.58 (0.50–3.36) | 0.33 / 0.46 / 0.66 (0.46–6.14) | 0.37 / 0.50 / 0.62 (0.54–5.25) | 0.37 / 0.46 / 0.66 (0.54–3.46) | 99% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.66 / 5.31 (1.62–8.19) | 0.87 / 1.25 / 4.10 (1.33–6.40) | 0.95 / 1.54 / 10.62 (1.54–23.30) | 0.74 / 1.15 / 7.04 (3.90–1130.50) | 0.74 / 1.12 / 3.52 (1.33–163.84) | 1.07 / 1.46 / 5.44 (1.62–8912.90) | 0.46 / 1.07 / 1.66 (1.20–6.21) | 0.37 / 0.50 / 0.70 (0.58–1.04) | 0.42 / 0.46 / 3.36 (0.70–7.23) | 0.37 / 0.50 / 0.66 (0.58–1.04) | 0.37 / 0.50 / 3.39 (0.83–3.46) | 93% | 59 |
| 1 | 0 | 200 | 4 | 1.36 / 2.30 / 11.78 (3.39–7733.25) | 0.91 / 1.62 / 5.12 (1.54–1736.70) | 1.04 / 1.62 / 22.53 (1.66–1556.48) | 0.87 / 1.57 / 6.14 (2.27–438.27) | 0.87 / 1.54 / 6.53 (1.87–6619.14) | 1.46 / 2.50 / 11.52 (2.14–7143.42) | 0.78 / 1.79 / 2.53 (2.37–5.70) | 0.50 / 0.78 / 2.30 (0.95–7.10) | 0.50 / 0.74 / 0.95 (0.91–3.39) | 0.42 / 0.74 / 2.03 (0.66–9.47) | 0.42 / 0.74 / 2.27 (0.91–3.52) | 78% | 310 |
| 1 | 0 | 200 | 6 | 2.08 / 3.20 / 14.46 (8.58–29622.27) | 1.25 / 2.37 / 1523.71 (5.12–24641.54) | 1.70 / 227.33 / 4259.84 (7.23–22282.24) | 1.15 / 2.27 / 8.96 (6.78–14417.92) | 1.15 / 2.30 / 1163.26 (3.33–10092.54) | 2.08 / 3.90 / 307.20 (6.34–8650.75) | 1.28 / 2.56 / 3.04 (2.53–8781.82) | 1.82 / 3.10 / 10.50 (5.82–401.41) | 0.70 / 2.08 / 3.14 (2.21–113.66) | 0.87 / 1.15 / 7.10 (4.54–10.50) | 0.54 / 0.83 / 1.15 (1.12–1.41) | 58% | 1313 |
| 1 | 0 | 200 | 8 | 2.00 / 26.62 / 413.70 (10.50–1540.10) | 1.33 / 2.69 / 64.51 (5.95–17039.36) | 1.90 / 11.65 / 417.79 (11.39–51904.51) | 1.28 / 3.04 / 219.14 (3.46–14942.21) | 1.25 / 2.82 / 8650.75 (6.14–41943.04) | 2.24 / 4194.30 / 11665.41 (7.87–39321.60) | 1.33 / 2.69 / 7.49 (2.66–19660.80) | 2.08 / 4.16 / 82.94 (5.57–57671.68) | 0.91 / 2.56 / 4.67 (3.55–15728.64) | 1.49 / 2.37 / 13.95 (6.40–3801.09) | 0.58 / 0.87 / 1.12 (1.04–1.25) | 46% | 1969 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.62 / 7.36 (3.68–8388.61) | 2.37 / 3.55 / 5.76 (3.62–630.78) | 1.79 / 2.85 / 8.45 (2.91–9.73) | 1.66 / 2.82 / 6.40 (3.20–8.70) | 1.66 / 2.78 / 4.29 (2.82–14.08) | 1.15 / 1.28 / 1.57 (1.33–11.65) | 1.15 / 1.25 / 2.00 (1.66–2.11) | 1.15 / 1.28 / 1.49 (1.33–5.63) | 1.15 / 1.25 / 2.00 (1.66–4.48) | 1.15 / 1.28 / 2.94 (1.28–20.22) | 1.15 / 1.25 / 2.08 (1.33–4.54) | 96% | 40 |
| 1 | 0 | 1000 | 3 | 2.98 / 4.61 / 9.86 (4.80–14548.99) | 2.50 / 3.65 / 27.65 (3.62–417.79) | 2.27 / 3.87 / 15.74 (8.32–44040.19) | 1.74 / 2.85 / 13.44 (3.78–260.10) | 1.74 / 2.85 / 5.70 (3.07–737.28) | 2.78 / 3.81 / 26.11 (7.23–208.90) | 1.25 / 2.56 / 4.67 (2.82–5.57) | 1.95 / 2.11 / 10.62 (4.10–14.46) | 1.20 / 1.33 / 3.36 (2.24–4.29) | 1.95 / 2.53 / 12.29 (4.22–19.71) | 1.25 / 1.62 / 2.91 (2.08–29.18) | 89% | 104 |
| 1 | 0 | 1000 | 4 | 3.52 / 5.44 / 15.23 (6.21–1458.18) | 2.85 / 4.48 / 8.70 (6.66–1409.02) | 2.91 / 4.61 / 12.67 (7.49–4161.54) | 2.14 / 3.74 / 8.19 (4.74–48.64) | 2.11 / 3.36 / 7.55 (3.94–25.60) | 3.74 / 5.82 / 10.11 (7.62–16.38) | 1.79 / 3.23 / 4.06 (3.46–6.08) | 3.33 / 3.55 / 8.45 (4.06–17.41) | 1.25 / 1.90 / 4.42 (2.69–7.10) | 3.23 / 3.74 / 12.93 (9.47–34.30) | 1.25 / 1.87 / 2.69 (2.66–4.29) | 76% | 147 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.40 / 17.66 (13.06–33292.29) | 3.20 / 4.99 / 16.00 (12.80–17039.36) | 3.39 / 5.18 / 13.57 (10.24–18087.94) | 2.27 / 4.03 / 7.04 (6.02–39845.89) | 2.30 / 4.35 / 8.83 (5.70–950.27) | 4.80 / 7.55 / 16.00 (9.09–1228.80) | 2.43 / 3.74 / 10.11 (3.94–10.62) | 7.10 / 10.50 / 20.99 (12.80–499.71) | 1.54 / 3.23 / 4.22 (3.68–6.91) | 5.76 / 7.23 / 18.69 (14.72–208.90) | 1.36 / 2.30 / 2.91 (2.72–2.98) | 59% | 549 |
| 1 | 0 | 1000 | 8 | 4.48 / 7.55 / 24.32 (12.80–3145.73) | 3.14 / 5.70 / 16.38 (6.59–1933.31) | 3.81 / 7.23 / 364.54 (15.23–30146.56) | 2.50 / 4.80 / 122.88 (5.50–393.22) | 2.40 / 4.61 / 8.19 (5.70–393.22) | 5.06 / 8.70 / 18.94 (13.44–827.39) | 2.53 / 4.16 / 5.31 (5.06–54.78) | 10.37 / 17.15 / 243.71 (20.22–2424.83) | 2.08 / 3.58 / 4.29 (4.10–7.87) | 8.06 / 8.96 / 46.59 (15.74–917.50) | 1.41 / 2.53 / 2.91 (2.72–3.65) | 46% | 764 |
| 1 | 1000 | 50 | 2 | 1.62 / 1.82 / 3.10 (1.87–9.22) | 0.29 / 1.82 / 3.78 (1.87–20.74) | 1.57 / 1.79 / 5.95 (1.87–17.15) | 0.25 / 1.74 / 4.48 (1.78–7.04) | 0.25 / 1.74 / 4.54 (1.79–117.76) | 0.25 / 0.33 / 0.42 (0.37–0.78) | 0.21 / 0.33 / 0.37 (0.33–6.66) | 0.25 / 0.33 / 0.42 (0.37–5.31) | 0.25 / 0.37 / 3.23 (0.33–3.62) | 0.25 / 0.33 / 0.42 (0.37–0.54) | 0.25 / 0.46 / 3.20 (0.33–3.33) | 99% | 2 |
| 1 | 1000 | 50 | 3 | 2.85 / 6.21 / 13.44 (5.44–11010.05) | 1.70 / 2.40 / 10.88 (2.11–14024.70) | 2.82 / 3.52 / 12.93 (3.36–24.58) | 1.62 / 2.21 / 5.18 (2.14–1835.01) | 1.58 / 2.27 / 5.89 (4.35–3047.42) | 1.90 / 2.66 / 83.97 (2.11–3276.80) | 0.46 / 2.21 / 3.49 (1.87–1769.47) | 0.37 / 0.66 / 0.87 (0.46–1.00) | 0.29 / 0.54 / 0.91 (0.74–3.36) | 0.37 / 0.66 / 0.91 (0.46–2.66) | 0.33 / 0.58 / 0.87 (0.54–4.48) | 77% | 259 |
| 1 | 1000 | 50 | 4 | 3.07 / 3.74 / 23.30 (6.08–54.27) | 1.74 / 2.50 / 6.98 (2.82–174.08) | 3.20 / 3.68 / 10.88 (9.34–286.72) | 1.74 / 2.30 / 2.98 (2.08–598.02) | 1.79 / 2.72 / 5.25 (3.81–149.50) | 2.30 / 3.10 / 11.78 (2.72–4980.74) | 0.83 / 2.66 / 7.55 (4.61–7471.10) | 0.37 / 0.66 / 1.00 (0.42–23592.96) | 0.33 / 0.62 / 1.04 (0.70–3.49) | 0.37 / 0.66 / 1.00 (0.70–13.70) | 0.29 / 0.62 / 1.04 (0.74–10.50) | 74% | 201 |
| 1 | 1000 | 50 | 6 | 3.52 / 115.71 / 11927.55 (8.00–41418.75) | 1.95 / 3.33 / 851.97 (3.46–12976.13) | 3.52 / 3538.94 / 7340.03 (13.44–38797.31) | 1.90 / 3.39 / 5701.63 (3.90–19660.80) | 1.95 / 3.87 / 10.88 (8.06–10616.83) | 3.52 / 4.48 / 9.86 (6.27–30932.99) | 2.11 / 2.91 / 4.16 (3.46–12.03) | 2.43 / 3.49 / 10.50 (4.22–15073.28) | 0.58 / 2.69 / 3.36 (3.26–4.29) | 0.42 / 0.70 / 0.95 (0.79–10.11) | 0.42 / 0.62 / 0.83 (0.79–0.91) | 57% | 1420 |
| 1 | 1000 | 50 | 8 | 3.65 / 15.10 / 184.32 (11.26–512.00) | 2.08 / 3.46 / 8.45 (4.74–270.34) | 3.74 / 8.96 / 53.76 (12.67–51380.22) | 2.03 / 3.23 / 9.47 (4.80–1212.42) | 2.08 / 4.29 / 9.22 (8.32–1622.02) | 3.87 / 5.18 / 18.69 (11.39–4587.52) | 2.24 / 3.04 / 4.29 (3.55–6.02) | 2.56 / 3.90 / 24.32 (4.74–251.90) | 0.87 / 3.49 / 4.74 (4.16–13.57) | 0.83 / 1.25 / 4.48 (1.46–18.43) | 0.46 / 0.78 / 1.00 (0.87–1.04) | 47% | 319 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.24 / 4.99 (2.37–22.02) | 0.37 / 2.21 / 8.45 (2.24–35.33) | 1.78 / 2.08 / 25.34 (2.11–26.37) | 0.37 / 2.03 / 2.14 (2.11–4.93) | 0.37 / 2.08 / 4.74 (2.11–7.30) | 0.37 / 0.50 / 1.04 (0.54–14.21) | 0.37 / 0.46 / 1.04 (0.58–6.91) | 0.37 / 0.50 / 0.66 (0.58–1.07) | 0.37 / 0.70 / 1.36 (0.46–3.78) | 0.37 / 0.54 / 0.91 (0.70–9.47) | 0.37 / 0.74 / 1.90 (0.70–1769.47) | 94% | 5 |
| 1 | 1000 | 200 | 3 | 3.26 / 4.16 / 12.42 (7.23–200.70) | 1.90 / 2.82 / 9.34 (4.10–9175.04) | 2.98 / 4.54 / 37.38 (6.21–14417.92) | 1.74 / 2.53 / 6.91 (2.43–8.32) | 1.74 / 2.62 / 4.35 (2.37–19.71) | 2.08 / 2.94 / 6.78 (5.57–12.54) | 0.46 / 2.11 / 2.94 (2.21–6.27) | 0.37 / 0.50 / 0.58 (0.58–4.86) | 0.42 / 0.74 / 3.52 (0.70–4.42) | 0.37 / 0.50 / 0.62 (0.58–22.78) | 0.37 / 0.66 / 3.36 (0.70–3.68) | 86% | 84 |
| 1 | 1000 | 200 | 4 | 3.52 / 4.42 / 15.87 (8.70–41943.04) | 2.03 / 3.20 / 8.83 (2.50–11403.26) | 3.23 / 4.00 / 876.54 (4.99–18087.94) | 1.90 / 2.94 / 20.48 (3.04–9699.33) | 1.90 / 3.20 / 7.74 (5.31–7602.18) | 2.56 / 3.33 / 8.45 (3.07–16.64) | 1.00 / 2.94 / 5.06 (3.26–6029.31) | 0.50 / 0.79 / 1.00 (0.62–10.75) | 0.46 / 0.78 / 1.49 (0.70–3.46) | 0.42 / 0.70 / 1.04 (0.83–5.82) | 0.50 / 0.79 / 1.33 (1.07–3.68) | 85% | 324 |
| 1 | 1000 | 200 | 6 | 3.94 / 5.18 / 835.58 (8.45–4194.30) | 2.30 / 3.65 / 11.90 (9.98–49.66) | 3.87 / 6.53 / 491.52 (10.75–11010.05) | 2.14 / 3.33 / 4.86 (3.94–819.20) | 2.24 / 4.10 / 9.22 (4.86–278.53) | 3.90 / 5.38 / 9.73 (5.82–1097.73) | 2.30 / 3.46 / 4.67 (4.03–5.57) | 2.98 / 4.16 / 10.11 (4.99–11.39) | 0.74 / 3.23 / 4.22 (3.68–4.35) | 1.00 / 1.46 / 1.82 (1.62–6619.14) | 0.58 / 0.95 / 1.25 (1.20–1.33) | 59% | 345 |
| 1 | 1000 | 200 | 8 | 4.06 / 9.47 / 106.50 (20.74–1933.31) | 2.40 / 3.74 / 42.50 (4.61–663.55) | 3.90 / 10.50 / 638.98 (12.03–2588.67) | 2.27 / 3.78 / 6.40 (5.44–35.33) | 2.27 / 4.42 / 10.62 (6.40–141.31) | 4.16 / 6.53 / 22.78 (14.21–5570.56) | 2.40 / 3.55 / 7.04 (4.54–14942.21) | 2.91 / 4.74 / 83.97 (5.70–3833.86) | 1.00 / 3.10 / 4.29 (4.03–10.62) | 1.49 / 2.24 / 6.91 (2.50–3604.48) | 0.54 / 0.83 / 1.28 (1.12–1.33) | 46% | 956 |
| 1 | 1000 | 1000 | 2 | 3.52 / 4.86 / 14.46 (5.12–206.85) | 1.49 / 4.54 / 15.74 (6.53–2359.30) | 3.07 / 27.39 / 385.02 (4.03–10747.90) | 2.62 / 4.16 / 14.21 (5.95–2260.99) | 1.90 / 3.90 / 5.50 (3.90–5373.95) | 1.20 / 1.62 / 8.58 (1.41–9.22) | 1.15 / 1.49 / 2.21 (1.66–15.23) | 1.25 / 1.62 / 2.40 (1.33–12.16) | 1.20 / 1.58 / 2.11 (2.00–4.10) | 1.15 / 1.28 / 1.33 (1.28–3112.96) | 1.15 / 1.36 / 4.29 (2.21–9.09) | 80% | 155 |
| 1 | 1000 | 1000 | 3 | 4.99 / 6.98 / 15.49 (6.78–983.04) | 3.55 / 5.06 / 6.66 (5.57–11796.48) | 4.22 / 6.66 / 479.23 (6.08–6553.60) | 2.82 / 4.22 / 13.95 (7.87–23855.10) | 2.82 / 4.29 / 12.93 (5.38–9568.26) | 4.03 / 5.38 / 29.44 (5.95–9306.11) | 1.36 / 4.06 / 12.16 (3.68–7929.86) | 2.00 / 2.43 / 7.17 (2.11–9.86) | 1.20 / 1.66 / 4.00 (2.53–61.95) | 2.14 / 2.43 / 6.78 (2.53–258.05) | 1.25 / 1.74 / 4.35 (2.56–11.78) | 75% | 625 |
| 1 | 1000 | 1000 | 4 | 5.44 / 8.19 / 39.42 (7.74–14942.21) | 3.62 / 6.78 / 22.02 (7.49–2490.37) | 4.61 / 8.83 / 40.96 (8.45–9306.11) | 3.10 / 4.74 / 17.66 (6.66–1523.71) | 3.07 / 5.38 / 10.50 (6.46–2686.98) | 4.67 / 6.27 / 9.98 (7.68–56.32) | 2.62 / 4.29 / 9.09 (4.61–4653.06) | 3.23 / 3.52 / 9.60 (8.00–11.90) | 1.25 / 1.82 / 3.39 (2.50–4.22) | 3.20 / 3.62 / 21.25 (3.46–31.23) | 1.20 / 2.27 / 4.42 (2.62–25.86) | 80% | 407 |
| 1 | 1000 | 1000 | 6 | 6.08 / 14.59 / 438.27 (311.30–1359.87) | 4.03 / 5.82 / 7.74 (7.36–786.43) | 5.31 / 7.23 / 16.90 (11.78–3473.41) | 3.26 / 5.31 / 7.23 (5.82–119.81) | 3.23 / 5.82 / 8.70 (8.13–520.19) | 5.82 / 7.94 / 17.41 (10.88–532.48) | 3.23 / 4.61 / 5.50 (4.93–1441.79) | 7.36 / 11.01 / 131.07 (15.49–425.98) | 1.49 / 4.03 / 7.81 (4.74–9.73) | 5.70 / 10.37 / 19.97 (10.37–92.16) | 1.36 / 2.43 / 3.23 (2.72–6.02) | 60% | 339 |
| 1 | 1000 | 1000 | 8 | 6.46 / 44.54 / 1327.10 (25.09–30932.99) | 4.16 / 6.91 / 13.44 (11.90–4849.66) | 5.82 / 37.38 / 892.93 (38.40–5963.78) | 3.46 / 5.95 / 192.51 (9.86–991.23) | 3.39 / 6.78 / 119.81 (11.14–7012.35) | 6.14 / 28.67 / 323.58 (16.38–7274.50) | 3.39 / 5.12 / 7.87 (6.40–61.44) | 10.24 / 565.25 / 1441.79 (69.63–6488.06) | 2.43 / 4.54 / 5.95 (5.18–360.45) | 8.00 / 11.14 / 61.95 (23.81–901.12) | 1.36 / 2.53 / 2.85 (2.69–3.90) | 47% | 2042 |
| 10 | 0 | 50 | 2 | 0.62 / 0.87 / 5.95 (0.95–13.44) | 0.29 / 0.83 / 2.21 (1.07–4.48) | 0.58 / 0.79 / 1.49 (0.87–8.83) | 0.37 / 0.74 / 2.78 (1.20–7.94) | 0.29 / 0.74 / 1.36 (0.87–4.61) | 0.25 / 0.37 / 0.46 (0.46–0.70) | 0.25 / 0.37 / 0.54 (0.46–0.74) | 0.25 / 0.37 / 0.46 (0.46–0.87) | 0.25 / 0.33 / 0.46 (0.42–0.83) | 0.25 / 0.37 / 0.46 (0.46–0.83) | 0.25 / 0.37 / 0.54 (0.46–0.74) | 91% | 18 |
| 10 | 0 | 50 | 3 | 0.83 / 1.49 / 11.14 (8.32–23.81) | 0.62 / 1.00 / 5.38 (1.46–14.46) | 0.74 / 1.20 / 7.55 (4.29–16.90) | 0.58 / 1.12 / 8.00 (1.07–11.14) | 0.58 / 0.91 / 2.03 (1.25–3768.32) | 0.79 / 1.28 / 9.60 (3.20–4390.91) | 0.33 / 0.87 / 1.66 (0.95–5.76) | 0.25 / 0.46 / 0.58 (0.54–0.91) | 0.25 / 0.42 / 0.54 (0.54–0.91) | 0.25 / 0.50 / 0.74 (0.54–0.91) | 0.25 / 0.46 / 0.58 (0.54–0.83) | 92% | 115 |
| 10 | 0 | 50 | 4 | 1.25 / 2.11 / 10.62 (7.36–1900.54) | 0.74 / 1.74 / 3.26 (3.07–5.12) | 0.95 / 1.87 / 5.70 (3.04–22.02) | 0.74 / 1.62 / 4.16 (1.25–4.86) | 0.70 / 1.70 / 2.62 (1.41–13.57) | 0.95 / 1.62 / 2.37 (1.82–7.23) | 0.58 / 1.49 / 2.14 (1.49–4.10) | 0.33 / 0.66 / 0.83 (0.74–1.00) | 0.37 / 0.74 / 0.91 (0.58–1.20) | 0.25 / 0.70 / 0.87 (0.54–0.91) | 0.25 / 0.54 / 0.79 (0.58–0.91) | 85% | 115 |
| 10 | 0 | 50 | 6 | 1.54 / 5.38 / 14.85 (7.94–14548.99) | 1.00 / 2.27 / 8.32 (2.98–157.70) | 1.49 / 3.14 / 13.70 (9.09–61.95) | 0.95 / 2.27 / 5.70 (3.23–14.85) | 0.91 / 2.30 / 6.40 (2.94–8257.54) | 1.87 / 3.78 / 12.67 (6.53–1146.88) | 1.07 / 2.40 / 4.22 (3.04–8.00) | 1.57 / 3.10 / 12.54 (5.44–3735.55) | 0.58 / 2.11 / 2.94 (2.82–3.07) | 0.42 / 0.74 / 1.00 (1.00–1.20) | 0.42 / 0.87 / 1.12 (1.07–1.33) | 59% | 620 |
| 10 | 0 | 50 | 8 | 1.74 / 4.86 / 532.48 (10.37–14942.21) | 1.15 / 3.26 / 6.27 (4.93–335.87) | 1.74 / 5.38 / 294.91 (11.65–8388.61) | 1.12 / 3.14 / 29.95 (4.74–231.42) | 1.12 / 3.33 / 208.90 (4.54–851.97) | 2.11 / 4.67 / 14.85 (8.19–622.59) | 1.28 / 2.91 / 4.06 (4.03–2457.60) | 1.90 / 4.10 / 15.74 (5.31–1605.63) | 0.95 / 2.98 / 4.03 (4.00–6.21) | 0.54 / 0.91 / 2.27 (1.12–8.96) | 0.46 / 0.91 / 1.25 (1.15–1.62) | 47% | 2546 |
| 10 | 0 | 200 | 2 | 0.91 / 1.20 / 4.16 (1.33–161.79) | 0.46 / 1.20 / 4.99 (1.33–516.10) | 0.78 / 1.12 / 12.16 (1.20–6160.38) | 0.62 / 1.12 / 1.70 (1.20–368.64) | 0.58 / 1.41 / 34.82 (1.25–2228.22) | 0.42 / 0.78 / 0.95 (0.62–1.07) | 0.37 / 0.70 / 0.91 (0.66–1.04) | 0.42 / 0.79 / 0.95 (0.58–1.00) | 0.37 / 0.66 / 0.95 (0.66–1.00) | 0.37 / 0.66 / 0.87 (0.62–1.07) | 0.37 / 0.58 / 0.91 (0.66–1.07) | 78% | 197 |
| 10 | 0 | 200 | 3 | 1.07 / 1.62 / 7.04 (4.86–11.14) | 0.91 / 1.70 / 2.37 (1.62–12.29) | 1.00 / 1.66 / 7.68 (2.24–12.80) | 0.74 / 1.20 / 2.98 (1.54–4259.84) | 0.74 / 1.20 / 1.57 (1.36–647.17) | 1.07 / 1.57 / 6.66 (1.79–42.50) | 0.46 / 1.20 / 2.03 (1.28–37.89) | 0.37 / 0.66 / 0.79 (0.70–1.07) | 0.37 / 0.66 / 0.83 (0.79–1.58) | 0.37 / 0.58 / 0.79 (0.70–1.07) | 0.37 / 0.66 / 0.83 (0.78–2.11) | 89% | 169 |
| 10 | 0 | 200 | 4 | 1.20 / 2.43 / 10.37 (4.93–19.71) | 0.91 / 1.62 / 2.72 (1.82–5.70) | 1.04 / 1.90 / 11.90 (8.96–23.30) | 0.79 / 1.58 / 5.76 (1.74–11.14) | 0.83 / 1.95 / 3.07 (1.57–10.62) | 1.25 / 2.50 / 5.70 (2.69–15.74) | 0.79 / 2.03 / 2.82 (1.66–7.62) | 0.42 / 0.87 / 1.07 (0.74–8.83) | 0.46 / 0.95 / 1.46 (1.20–1.87) | 0.42 / 0.74 / 1.00 (0.70–1.20) | 0.50 / 1.00 / 1.25 (0.87–1.54) | 86% | 29 |
| 10 | 0 | 200 | 6 | 2.11 / 4.29 / 11.26 (8.96–51.71) | 1.33 / 2.82 / 5.38 (3.65–7.36) | 1.70 / 3.55 / 11.90 (8.32–933.89) | 1.15 / 2.66 / 4.74 (4.00–9.73) | 1.15 / 2.62 / 3.94 (3.90–5.06) | 2.27 / 4.67 / 8.96 (6.27–178.18) | 1.28 / 2.66 / 3.94 (3.36–4.61) | 2.08 / 4.06 / 7.23 (6.21–9.73) | 0.74 / 2.30 / 3.33 (2.94–4.03) | 1.00 / 1.54 / 3.46 (1.78–5.31) | 0.66 / 1.36 / 1.74 (1.49–1.90) | 57% | 143 |
| 10 | 0 | 200 | 8 | 2.08 / 5.06 / 13.70 (11.65–6684.67) | 1.41 / 3.68 / 9.86 (5.76–876.54) | 1.95 / 4.86 / 13.06 (10.62–8519.68) | 1.33 / 3.78 / 9.98 (7.49–9961.47) | 1.33 / 3.65 / 6.85 (5.70–87.04) | 2.66 / 5.70 / 12.67 (8.96–96.26) | 1.46 / 3.20 / 4.80 (4.22–8.19) | 2.40 / 5.25 / 9.98 (9.60–66.56) | 1.25 / 3.04 / 4.06 (4.03–4.61) | 1.58 / 2.08 / 9.60 (8.06–225.28) | 0.58 / 1.36 / 1.82 (1.74–1.87) | 46% | 1397 |
| 10 | 0 | 1000 | 2 | 2.50 / 3.58 / 8.32 (3.74–581.63) | 1.20 / 3.55 / 9.34 (3.74–143.36) | 1.78 / 2.82 / 6.40 (3.87–119.81) | 1.66 / 2.72 / 6.85 (3.36–8.19) | 1.62 / 2.78 / 5.31 (4.06–10.62) | 1.15 / 1.33 / 2.08 (1.41–9.09) | 1.15 / 1.41 / 2.50 (1.66–4.35) | 1.15 / 1.33 / 1.54 (1.46–4.00) | 1.15 / 1.41 / 1.82 (1.66–6.53) | 1.15 / 1.33 / 2.03 (1.49–4.74) | 1.15 / 1.41 / 1.66 (1.62–8.58) | 94% | 46 |
| 10 | 0 | 1000 | 3 | 2.78 / 4.61 / 9.34 (5.63–8781.82) | 2.50 / 4.22 / 7.81 (7.04–3407.87) | 2.27 / 3.81 / 10.75 (5.63–3178.50) | 1.74 / 3.36 / 7.87 (5.18–1835.01) | 1.74 / 3.33 / 8.06 (6.14–2818.05) | 2.78 / 4.42 / 9.73 (6.66–7012.35) | 1.28 / 2.50 / 3.62 (2.98–5.06) | 1.95 / 2.43 / 3.23 (2.37–7.74) | 1.20 / 2.53 / 3.04 (2.43–3.87) | 2.03 / 2.43 / 7.81 (4.06–11.39) | 1.20 / 2.53 / 2.82 (2.40–2.85) | 88% | 309 |
| 10 | 0 | 1000 | 4 | 3.62 / 5.76 / 13.82 (7.10–5177.34) | 2.78 / 4.93 / 10.62 (8.32–23.81) | 2.91 / 5.25 / 1441.79 (8.96–7995.39) | 2.08 / 4.16 / 6.53 (4.80–33.28) | 2.08 / 4.10 / 6.14 (4.61–14.21) | 3.65 / 6.72 / 12.29 (7.10–57.86) | 1.82 / 3.26 / 4.29 (3.55–14.98) | 3.26 / 3.68 / 9.60 (9.22–25.34) | 1.33 / 2.72 / 3.81 (3.49–4.06) | 3.33 / 3.68 / 9.60 (7.87–114.69) | 1.28 / 2.69 / 3.74 (3.39–3.94) | 76% | 431 |
| 10 | 0 | 1000 | 6 | 4.29 / 7.62 / 14.21 (12.03–507.90) | 3.07 / 6.14 / 9.98 (8.19–12.03) | 3.49 / 7.30 / 20.48 (11.65–5111.81) | 2.40 / 5.82 / 9.47 (8.32–11927.55) | 2.37 / 6.08 / 10.50 (7.36–5373.95) | 5.06 / 9.47 / 15.74 (13.82–9961.47) | 2.30 / 4.61 / 7.55 (6.08–3997.70) | 7.04 / 12.29 / 21.50 (17.66–14942.21) | 1.58 / 4.03 / 5.63 (5.06–483.33) | 5.70 / 6.46 / 34.82 (13.18–14155.78) | 1.41 / 3.74 / 4.80 (4.16–10485.76) | 56% | 1272 |
| 10 | 0 | 1000 | 8 | 4.80 / 24.83 / 1474.56 (12.54–37748.74) | 3.33 / 7.87 / 696.32 (10.75–15728.64) | 3.90 / 9.09 / 1392.64 (13.18–9961.47) | 2.66 / 7.04 / 31.74 (9.73–28049.41) | 2.69 / 6.98 / 20.48 (9.60–1146.88) | 5.70 / 12.03 / 323.58 (17.41–9437.18) | 2.43 / 5.70 / 10.75 (7.87–14.98) | 9.98 / 18.69 / 925.70 (22.53–6488.06) | 2.40 / 4.67 / 6.27 (6.14–8.06) | 8.00 / 11.39 / 37.38 (15.36–8388.61) | 1.41 / 4.00 / 5.44 (5.31–5.63) | 46% | 2876 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 6.21 (2.03–311.30) | 0.29 / 1.82 / 2.00 (1.95–11.01) | 1.57 / 1.79 / 4.86 (1.87–9.47) | 0.29 / 1.74 / 3.81 (1.87–6.46) | 0.25 / 1.74 / 3.10 (1.87–4.99) | 0.25 / 0.37 / 0.42 (0.42–0.79) | 0.25 / 0.33 / 0.46 (0.42–0.74) | 0.25 / 0.37 / 0.46 (0.46–0.74) | 0.25 / 0.33 / 0.46 (0.42–0.66) | 0.25 / 0.37 / 0.50 (0.46–0.79) | 0.25 / 0.33 / 0.46 (0.42–0.70) | 91% | 28 |
| 10 | 1000 | 50 | 3 | 2.72 / 3.20 / 12.67 (4.48–8126.46) | 1.62 / 2.03 / 4.86 (2.21–843.78) | 2.69 / 3.20 / 7.23 (4.61–7208.96) | 1.54 / 2.08 / 4.93 (2.14–442.37) | 1.54 / 2.78 / 5.31 (4.16–176.13) | 1.95 / 2.85 / 4.29 (2.40–745.47) | 0.46 / 2.24 / 2.43 (1.95–2.78) | 0.37 / 0.70 / 0.91 (0.54–1.07) | 0.33 / 0.66 / 0.87 (0.54–1.12) | 0.37 / 0.70 / 0.91 (0.66–1.41) | 0.29 / 0.66 / 0.87 (0.58–1.12) | 83% | 214 |
| 10 | 1000 | 50 | 4 | 3.20 / 4.42 / 13.70 (5.57–2293.76) | 1.74 / 2.85 / 7.87 (5.31–9.98) | 2.98 / 4.16 / 11.65 (7.87–16.38) | 1.70 / 2.85 / 6.85 (2.40–12.80) | 1.66 / 4.03 / 6.21 (5.63–12.93) | 2.24 / 3.78 / 5.95 (2.98–17.41) | 0.62 / 3.14 / 3.81 (2.72–8.70) | 0.37 / 0.74 / 0.95 (0.70–1.00) | 0.33 / 0.74 / 1.00 (0.62–1.20) | 0.37 / 0.74 / 0.95 (0.54–7.49) | 0.37 / 0.79 / 1.15 (0.58–7.04) | 80% | 77 |
| 10 | 1000 | 50 | 6 | 3.55 / 6.02 / 13.57 (11.78–1179.65) | 2.00 / 3.94 / 8.19 (5.31–50.18) | 3.46 / 6.08 / 16.38 (10.24–123.90) | 2.03 / 4.06 / 7.62 (5.63–13.06) | 2.14 / 5.82 / 9.09 (8.32–18.94) | 3.07 / 4.99 / 8.00 (6.27–10.50) | 2.08 / 4.00 / 5.25 (4.48–5.31) | 2.40 / 4.00 / 6.27 (4.74–950.27) | 0.62 / 3.23 / 4.16 (3.87–9.86) | 0.46 / 0.91 / 1.15 (0.91–1.20) | 0.46 / 1.00 / 1.28 (1.07–1.41) | 59% | 302 |
| 10 | 1000 | 50 | 8 | 3.65 / 6.27 / 16.90 (11.78–4063.23) | 2.11 / 4.42 / 8.83 (6.78–10.88) | 3.62 / 6.78 / 20.48 (11.39–3768.32) | 2.11 / 4.48 / 8.45 (6.08–10.50) | 2.27 / 7.30 / 10.88 (10.62–12.93) | 3.58 / 5.82 / 10.50 (8.70–159.74) | 2.21 / 4.16 / 6.08 (4.99–52.22) | 2.62 / 4.99 / 9.86 (6.27–3604.48) | 0.87 / 4.03 / 4.99 (4.93–5.12) | 0.70 / 1.36 / 4.93 (1.66–30932.99) | 0.50 / 1.07 / 1.46 (1.28–1.58) | 46% | 928 |
| 10 | 1000 | 200 | 2 | 1.90 / 2.21 / 4.99 (2.30–6.59) | 0.42 / 2.21 / 4.16 (2.30–6.78) | 1.74 / 2.08 / 6.98 (2.21–9.09) | 0.42 / 2.08 / 2.40 (2.14–5.63) | 0.50 / 2.21 / 2.66 (2.21–21.25) | 0.37 / 0.54 / 0.66 (0.62–0.91) | 0.37 / 0.54 / 0.87 (0.66–2.56) | 0.37 / 0.54 / 0.74 (0.58–1.00) | 0.37 / 0.62 / 0.79 (0.66–0.95) | 0.42 / 0.78 / 0.91 (0.62–0.95) | 0.37 / 0.66 / 0.91 (0.66–0.95) | 84% | 44 |
| 10 | 1000 | 200 | 3 | 3.04 / 4.54 / 23.81 (9.34–3178.50) | 1.95 / 2.85 / 7.04 (5.44–12.80) | 2.98 / 4.16 / 11.52 (5.95–7077.89) | 1.78 / 2.69 / 5.12 (2.43–111.62) | 1.79 / 3.49 / 6.08 (3.39–958.46) | 2.14 / 2.98 / 8.19 (6.27–9.98) | 0.58 / 2.43 / 2.66 (2.21–3.04) | 0.50 / 0.87 / 0.95 (0.70–1.04) | 0.42 / 0.83 / 1.04 (0.79–1.15) | 0.46 / 0.83 / 0.95 (0.70–1.04) | 0.42 / 0.87 / 1.15 (0.79–1.49) | 77% | 291 |
| 10 | 1000 | 200 | 4 | 3.49 / 4.61 / 12.42 (11.78–4849.66) | 2.08 / 3.20 / 4.48 (2.98–2392.06) | 3.26 / 4.29 / 23.30 (8.70–11272.19) | 1.90 / 3.10 / 5.44 (3.90–3178.50) | 1.95 / 4.06 / 6.14 (5.06–4849.66) | 2.50 / 4.10 / 8.45 (3.74–6029.31) | 0.79 / 3.36 / 3.90 (2.82–614.40) | 0.50 / 0.79 / 1.04 (0.78–1.20) | 0.46 / 0.95 / 1.15 (0.83–1.74) | 0.50 / 0.83 / 1.00 (0.74–5.57) | 0.46 / 1.00 / 1.20 (0.91–1.28) | 78% | 639 |
| 10 | 1000 | 200 | 6 | 3.81 / 5.95 / 12.80 (11.52–15.87) | 2.27 / 4.16 / 6.40 (5.12–8.58) | 3.65 / 5.57 / 11.39 (11.01–16.90) | 2.14 / 4.06 / 7.49 (5.95–36.86) | 2.27 / 6.40 / 9.34 (8.96–2621.44) | 3.46 / 5.82 / 10.75 (8.45–17.41) | 2.21 / 4.06 / 5.12 (4.48–11.90) | 3.04 / 5.12 / 9.60 (6.78–10.75) | 0.74 / 3.36 / 4.42 (4.00–5.18) | 1.00 / 1.57 / 2.24 (2.11–9.98) | 0.58 / 1.15 / 1.62 (1.49–1.82) | 58% | 378 |
| 10 | 1000 | 200 | 8 | 4.03 / 6.85 / 14.34 (12.67–9568.26) | 2.43 / 4.99 / 11.52 (6.72–194.56) | 3.90 / 7.04 / 15.62 (14.21–1130.50) | 2.30 / 5.06 / 8.32 (7.17–60.93) | 2.53 / 8.06 / 12.93 (10.62–5242.88) | 4.16 / 7.74 / 24.06 (9.09–380.93) | 2.50 / 5.12 / 7.49 (6.40–12.29) | 3.20 / 6.66 / 11.39 (10.50–1523.71) | 1.15 / 4.10 / 5.25 (4.99–5.44) | 1.70 / 2.43 / 8.45 (7.68–12.42) | 0.58 / 1.36 / 1.87 (1.70–2.11) | 47% | 1260 |
| 10 | 1000 | 1000 | 2 | 3.49 / 4.61 / 8.19 (7.04–28.93) | 1.28 / 4.67 / 5.44 (4.67–11.14) | 2.72 / 3.87 / 7.68 (5.82–11.26) | 1.79 / 3.74 / 6.27 (3.81–8.83) | 1.54 / 3.74 / 9.22 (3.87–12.54) | 1.15 / 1.33 / 1.49 (1.46–2.14) | 1.15 / 1.41 / 1.70 (1.62–2.72) | 1.15 / 1.33 / 1.49 (1.46–2.91) | 1.15 / 1.33 / 1.66 (1.54–1.74) | 1.15 / 1.33 / 1.46 (1.46–5.38) | 1.15 / 1.41 / 1.70 (1.62–8.83) | 96% | 11 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.59 / 12.54 (9.60–22.53) | 3.49 / 5.31 / 7.74 (5.76–13.31) | 3.90 / 5.82 / 14.21 (11.26–18.18) | 2.72 / 4.35 / 5.44 (5.06–10.88) | 2.72 / 4.99 / 12.80 (5.89–15.23) | 3.68 / 5.95 / 13.18 (10.37–18.18) | 1.28 / 3.10 / 3.87 (3.78–10.50) | 1.95 / 2.14 / 7.10 (2.24–10.62) | 1.20 / 2.30 / 2.56 (2.43–10.88) | 1.95 / 2.14 / 2.91 (2.24–10.24) | 1.20 / 2.30 / 2.56 (2.50–8.58) | 95% | 6 |
| 10 | 1000 | 1000 | 4 | 5.44 / 7.94 / 18.43 (10.88–32.26) | 3.74 / 6.21 / 7.49 (6.66–13.31) | 4.67 / 7.04 / 19.46 (10.62–20.74) | 2.98 / 5.57 / 11.01 (6.21–17.41) | 3.10 / 6.91 / 12.42 (7.36–29.70) | 4.74 / 8.00 / 15.74 (9.73–27.39) | 2.30 / 4.48 / 5.70 (4.29–11.90) | 3.20 / 3.90 / 14.59 (10.62–18.43) | 1.28 / 2.72 / 3.87 (3.49–8.58) | 3.14 / 3.87 / 11.65 (7.81–15.87) | 1.20 / 2.40 / 3.49 (3.46–6.02) | 86% | 19 |
| 10 | 1000 | 1000 | 6 | 6.21 / 10.11 / 17.15 (13.95–290.82) | 4.16 / 7.94 / 11.52 (10.88–1671.17) | 5.44 / 9.34 / 17.92 (13.70–16515.07) | 3.39 / 7.04 / 10.11 (8.70–4849.66) | 3.39 / 9.47 / 13.44 (12.29–471.04) | 5.82 / 11.01 / 18.43 (14.21–892.93) | 3.26 / 6.40 / 8.32 (7.30–78.85) | 7.10 / 12.80 / 21.76 (16.26–1851.39) | 1.58 / 4.61 / 6.78 (5.82–23.04) | 5.82 / 6.40 / 13.18 (8.96–107.52) | 1.41 / 3.78 / 4.99 (4.29–7.81) | 60% | 917 |
| 10 | 1000 | 1000 | 8 | 6.59 / 11.26 / 19.46 (16.13–22806.53) | 4.48 / 9.34 / 42.50 (11.90–6815.74) | 5.82 / 11.78 / 25.60 (17.66–19660.80) | 3.68 / 8.58 / 15.10 (11.39–5505.02) | 3.81 / 11.78 / 18.18 (15.36–3276.80) | 6.66 / 13.44 / 23.81 (17.66–4784.13) | 3.39 / 7.30 / 11.52 (9.34–4259.84) | 9.98 / 19.46 / 39.94 (23.30–19922.94) | 2.72 / 5.57 / 8.00 (7.62–770.05) | 8.19 / 9.09 / 21.25 (17.41–5439.49) | 1.41 / 4.00 / 5.38 (5.31–10.11) | 47% | 2396 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 17.63; 2/4 fair: 32.41; 3 fair: 52.53;

Migrations seen: 417918 over 1088414701 samples (one sample per measured operation).


### Suite a2f: IPC call and reply, interrupt switch

load average before: { 4.71 6.42 6.28 }
load average after: { 5.77 6.28 6.24 }; 108 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 321k (320k–321k) | 322k (319k–323k) | 314k (311k–314k) | 314k (313k–314k) | 314k (312k–315k) | 100% | 0 |
| 0 | 50 | 4 | 641k (635k–641k) | 641k (638k–642k) | 625k (619k–627k) | 598k (589k–604k) | 594k (581k–599k) | 100% | 0 |
| 0 | 50 | 6 | 914k (895k–925k) | 915k (896k–925k) | 871k (831k–882k) | 770k (749k–808k) | 731k (728k–741k) | 68% | 10 |
| 0 | 50 | 8 | 1.23M (1.21M–1.25M) | 1.22M (1.18M–1.24M) | 1.14M (1.12M–1.19M) | 1.02M (983k–1.05M) | 935k (886k–984k) | 50% | 6 |
| 0 | 200 | 2 | 286k (286k–287k) | 286k (285k–286k) | 282k (281k–282k) | 282k (281k–282k) | 282k (282k–282k) | 100% | 0 |
| 0 | 200 | 4 | 571k (569k–572k) | 572k (568k–573k) | 561k (550k–564k) | 529k (524k–536k) | 537k (528k–540k) | 99% | 0 |
| 0 | 200 | 6 | 815k (807k–834k) | 826k (808k–851k) | 800k (789k–813k) | 674k (669k–733k) | 657k (635k–676k) | 67% | 15 |
| 0 | 200 | 8 | 1.11M (1.08M–1.14M) | 1.09M (1.09M–1.13M) | 1.06M (1.05M–1.07M) | 897k (832k–1.07M) | 696k (650k–759k) | 49% | 2 |
| 0 | 1000 | 2 | 201k (200k–201k) | 201k (200k–201k) | 198k (198k–199k) | 198k (198k–198k) | 198k (198k–198k) | 100% | 0 |
| 0 | 1000 | 4 | 399k (399k–401k) | 400k (400k–400k) | 395k (394k–396k) | 381k (379k–383k) | 384k (381k–385k) | 100% | 0 |
| 0 | 1000 | 6 | 583k (572k–589k) | 582k (576k–586k) | 575k (555k–593k) | 513k (496k–531k) | 335k (329k–360k) | 67% | 10 |
| 0 | 1000 | 8 | 784k (753k–794k) | 785k (774k–820k) | 761k (734k–766k) | 652k (614k–683k) | 331k (331k–349k) | 50% | 5 |
| 1000 | 50 | 2 | 195k (192k–195k) | 195k (195k–196k) | 191k (190k–191k) | 191k (190k–191k) | 190k (190k–191k) | 100% | 0 |
| 1000 | 50 | 4 | 390k (389k–391k) | 392k (390k–393k) | 379k (379k–381k) | 373k (372k–374k) | 374k (372k–374k) | 100% | 0 |
| 1000 | 50 | 6 | 570k (566k–574k) | 561k (561k–574k) | 542k (530k–549k) | 517k (509k–538k) | 501k (498k–520k) | 67% | 14 |
| 1000 | 50 | 8 | 755k (746k–762k) | 757k (739k–773k) | 716k (703k–731k) | 678k (675k–706k) | 686k (661k–692k) | 50% | 11 |
| 1000 | 200 | 2 | 182k (181k–182k) | 182k (182k–182k) | 178k (178k–179k) | 179k (178k–179k) | 179k (178k–179k) | 100% | 0 |
| 1000 | 200 | 4 | 362k (362k–363k) | 364k (363k–365k) | 355k (351k–355k) | 346k (345k–348k) | 346k (343k–348k) | 99% | 0 |
| 1000 | 200 | 6 | 532k (523k–534k) | 532k (526k–533k) | 506k (454k–511k) | 500k (481k–511k) | 473k (465k–481k) | 66% | 25 |
| 1000 | 200 | 8 | 708k (683k–725k) | 710k (694k–719k) | 688k (684k–723k) | 653k (631k–671k) | 628k (601k–646k) | 50% | 8 |
| 1000 | 1000 | 2 | 143k (141k–143k) | 143k (143k–143k) | 141k (139k–141k) | 141k (140k–141k) | 140k (140k–141k) | 100% | 0 |
| 1000 | 1000 | 4 | 285k (284k–285k) | 286k (282k–286k) | 279k (279k–280k) | 275k (270k–276k) | 274k (271k–275k) | 100% | 0 |
| 1000 | 1000 | 6 | 417k (415k–421k) | 416k (415k–417k) | 405k (396k–406k) | 389k (384k–394k) | 337k (332k–350k) | 67% | 13 |
| 1000 | 1000 | 8 | 561k (554k–567k) | 563k (552k–567k) | 539k (535k–547k) | 516k (510k–522k) | 332k (331k–354k) | 49% | 1 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 1.87 / 4.74 (1.95–7.23) | 1.74 / 1.82 / 5.18 (1.90–10.88) | 1.82 / 1.90 / 2.03 (2.00–11.01) | 1.82 / 1.90 / 5.70 (2.03–8.32) | 1.82 / 1.90 / 3.20 (2.03–9.47) | 100% | 0 |
| 0 | 50 | 4 | 1.74 / 1.87 / 2.40 (1.95–12.16) | 1.74 / 1.87 / 7.30 (1.90–12.93) | 1.82 / 1.90 / 2.53 (2.03–12.29) | 1.95 / 2.30 / 4.00 (2.37–13.31) | 1.95 / 2.30 / 3.94 (2.62–12.42) | 100% | 0 |
| 0 | 50 | 6 | 1.87 / 2.37 / 2.56 (2.56–10.24) | 1.90 / 2.37 / 3.62 (2.56–9.98) | 2.08 / 2.69 / 2.94 (2.82–8.83) | 2.66 / 3.49 / 4.42 (3.46–10.24) | 2.72 / 3.49 / 6.91 (4.42–10.37) | 68% | 10 |
| 0 | 50 | 8 | 1.82 / 2.37 / 3.65 (2.78–11.26) | 2.00 / 2.37 / 3.36 (2.56–9.47) | 2.11 / 2.78 / 6.72 (2.91–11.14) | 2.50 / 3.58 / 8.13 (3.90–16.38) | 2.85 / 3.81 / 9.22 (7.74–13.31) | 50% | 6 |
| 0 | 200 | 2 | 2.11 / 2.21 / 3.39 (2.30–7.62) | 2.11 / 2.24 / 2.40 (2.27–9.98) | 2.14 / 2.27 / 5.95 (2.37–9.34) | 2.14 / 2.27 / 4.00 (2.37–6.27) | 2.14 / 2.27 / 5.70 (2.37–7.49) | 100% | 0 |
| 0 | 200 | 4 | 2.11 / 2.27 / 5.95 (2.27–8.83) | 2.11 / 2.21 / 2.37 (2.27–9.60) | 2.21 / 2.30 / 3.04 (2.37–14.34) | 2.30 / 3.23 / 6.98 (4.35–14.46) | 2.30 / 2.94 / 3.36 (3.04–16.90) | 99% | 0 |
| 0 | 200 | 6 | 2.30 / 2.72 / 2.91 (2.82–10.88) | 2.21 / 2.72 / 4.99 (2.82–9.09) | 2.24 / 2.98 / 4.22 (3.10–6.66) | 3.20 / 4.06 / 6.78 (4.74–8.83) | 3.23 / 4.10 / 8.19 (4.86–9.22) | 67% | 15 |
| 0 | 200 | 8 | 2.24 / 2.72 / 3.90 (2.94–6.72) | 2.40 / 2.72 / 6.85 (3.23–8.58) | 2.24 / 3.04 / 5.63 (3.87–10.50) | 3.14 / 4.06 / 8.96 (4.48–10.50) | 4.42 / 5.44 / 13.44 (11.14–13.82) | 49% | 2 |
| 0 | 1000 | 2 | 3.62 / 3.68 / 8.96 (3.87–10.75) | 3.62 / 3.68 / 8.06 (4.00–9.34) | 3.65 / 3.78 / 6.14 (3.90–9.09) | 3.65 / 3.78 / 9.47 (8.58–9.98) | 3.65 / 3.78 / 7.10 (4.03–8.70) | 100% | 0 |
| 0 | 1000 | 4 | 3.62 / 3.74 / 7.74 (3.81–11.01) | 3.62 / 3.74 / 6.91 (6.08–9.86) | 3.65 / 3.78 / 8.06 (4.74–9.60) | 3.81 / 5.12 / 9.86 (8.83–11.65) | 3.81 / 4.99 / 9.09 (5.12–10.37) | 100% | 0 |
| 0 | 1000 | 6 | 3.74 / 4.29 / 8.00 (4.93–9.86) | 3.81 / 4.22 / 9.60 (4.61–10.62) | 3.74 / 4.48 / 6.27 (4.61–9.34) | 4.35 / 6.34 / 12.42 (7.68–13.18) | 7.62 / 8.06 / 16.26 (14.59–18.94) | 67% | 10 |
| 0 | 1000 | 8 | 3.78 / 4.22 / 9.47 (6.27–10.88) | 3.68 / 4.22 / 10.37 (6.27–11.65) | 3.94 / 4.54 / 10.24 (5.95–11.01) | 4.61 / 6.34 / 12.54 (11.65–14.21) | 10.62 / 11.52 / 22.53 (22.02–28.42) | 50% | 5 |
| 1000 | 50 | 2 | 3.74 / 3.87 / 8.96 (4.00–16.00) | 3.74 / 3.87 / 8.96 (4.48–13.06) | 3.87 / 4.00 / 8.06 (4.10–10.50) | 3.87 / 4.00 / 8.96 (4.06–10.24) | 3.87 / 4.00 / 9.34 (4.16–9.86) | 100% | 0 |
| 1000 | 50 | 4 | 3.74 / 3.87 / 7.36 (5.95–10.37) | 3.74 / 3.81 / 7.81 (3.90–11.52) | 3.87 / 4.29 / 9.98 (5.06–12.29) | 4.00 / 4.16 / 8.83 (4.29–12.42) | 4.00 / 4.10 / 8.45 (4.29–8.83) | 100% | 0 |
| 1000 | 50 | 6 | 3.87 / 4.35 / 8.58 (4.67–11.90) | 4.03 / 4.42 / 10.24 (5.31–11.39) | 4.22 / 4.86 / 10.37 (5.25–11.39) | 4.48 / 5.12 / 10.75 (7.10–11.90) | 4.61 / 5.06 / 10.75 (8.06–13.31) | 67% | 14 |
| 1000 | 50 | 8 | 4.03 / 4.42 / 10.37 (5.76–11.78) | 4.00 / 4.42 / 9.86 (5.95–12.29) | 4.16 / 4.86 / 10.88 (9.86–12.29) | 4.61 / 5.12 / 11.39 (10.62–12.54) | 4.48 / 5.06 / 11.65 (10.88–13.82) | 50% | 11 |
| 1000 | 200 | 2 | 4.10 / 4.22 / 8.83 (5.82–11.90) | 4.10 / 4.22 / 9.60 (7.94–12.03) | 4.22 / 4.42 / 9.47 (5.25–9.73) | 4.22 / 4.42 / 8.06 (6.02–10.75) | 4.22 / 4.42 / 9.60 (4.42–10.62) | 100% | 0 |
| 1000 | 200 | 4 | 4.10 / 4.29 / 9.98 (5.25–10.62) | 4.10 / 4.22 / 6.34 (4.29–7.74) | 4.22 / 4.42 / 11.14 (5.82–16.13) | 4.42 / 4.86 / 9.34 (5.31–11.90) | 4.42 / 4.54 / 7.10 (4.93–18.94) | 99% | 0 |
| 1000 | 200 | 6 | 4.22 / 4.74 / 10.37 (6.08–14.34) | 4.29 / 4.74 / 6.21 (5.50–14.46) | 4.61 / 5.12 / 10.75 (7.42–13.82) | 4.67 / 5.31 / 11.90 (7.81–14.72) | 4.99 / 5.44 / 11.39 (7.36–12.54) | 66% | 25 |
| 1000 | 200 | 8 | 4.22 / 4.74 / 9.86 (6.02–18.69) | 4.22 / 4.74 / 11.01 (9.86–27.14) | 4.35 / 5.12 / 11.14 (8.45–16.90) | 4.74 / 5.25 / 12.42 (10.88–12.54) | 5.06 / 6.21 / 14.34 (11.01–33.79) | 50% | 8 |
| 1000 | 1000 | 2 | 5.57 / 5.70 / 10.62 (5.89–17.41) | 5.57 / 5.70 / 11.90 (11.26–14.59) | 5.70 / 5.89 / 12.42 (10.24–16.00) | 5.70 / 5.89 / 12.16 (10.50–15.74) | 5.70 / 5.89 / 12.54 (10.37–16.26) | 100% | 0 |
| 1000 | 1000 | 4 | 5.63 / 5.76 / 12.42 (9.60–15.74) | 5.57 / 5.76 / 10.50 (9.47–19.97) | 5.70 / 5.95 / 13.44 (10.88–18.18) | 5.89 / 6.08 / 8.19 (7.36–24.06) | 5.89 / 6.59 / 14.85 (10.62–23.04) | 100% | 0 |
| 1000 | 1000 | 6 | 5.76 / 6.27 / 11.65 (11.14–15.36) | 5.89 / 6.27 / 10.75 (7.68–16.90) | 6.08 / 6.72 / 13.70 (11.39–15.87) | 6.40 / 6.91 / 14.21 (12.80–15.36) | 7.62 / 8.19 / 15.10 (14.46–33.79) | 67% | 13 |
| 1000 | 1000 | 8 | 5.70 / 6.27 / 13.44 (13.18–26.62) | 5.70 / 6.27 / 13.18 (12.67–20.99) | 6.21 / 6.78 / 12.67 (12.42–17.92) | 6.34 / 6.98 / 14.85 (14.08–17.66) | 10.62 / 11.52 / 21.50 (20.99–27.39) | 49% | 1 |

Migrations seen: 35629 over 46449037 samples (one sample per measured operation).


### Suite a3f: k-1 threads signal one notify, interrupt switch

load average before: { 5.77 6.28 6.24 }
load average after: { 5.42 5.99 6.12 }; 108 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 708k (706k–714k) | 712k (707k–714k) | 668k (666k–669k) | 667k (666k–669k) | 669k (667k–670k) | 100% | 0 |
| 0 | 50 | 4 | 2.11M (2.10M–2.12M) | 2.11M (2.10M–2.11M) | 1.89M (1.88M–1.90M) | 1.95M (1.92M–1.98M) | 1.95M (1.94M–1.99M) | 100% | 0 |
| 0 | 50 | 6 | 3.41M (3.39M–3.43M) | 3.41M (3.39M–3.42M) | 2.74M (2.72M–2.96M) | 2.39M (2.36M–2.45M) | 2.41M (2.36M–2.46M) | 67% | 18 |
| 0 | 50 | 8 | 4.75M (4.72M–4.75M) | 4.75M (4.74M–4.76M) | 3.55M (3.18M–3.91M) | 2.78M (2.66M–2.80M) | 2.61M (2.58M–2.71M) | 50% | 39 |
| 0 | 200 | 2 | 662k (652k–664k) | 661k (653k–663k) | 575k (574k–580k) | 578k (570k–580k) | 575k (570k–580k) | 100% | 0 |
| 0 | 200 | 4 | 1.96M (1.96M–1.97M) | 1.95M (1.94M–1.97M) | 1.67M (1.65M–1.68M) | 1.60M (1.59M–1.62M) | 1.60M (1.59M–1.61M) | 100% | 0 |
| 0 | 200 | 6 | 3.19M (3.19M–3.20M) | 3.19M (3.19M–3.20M) | 2.33M (2.12M–2.41M) | 1.73M (1.70M–1.75M) | 1.57M (1.57M–1.64M) | 67% | 9 |
| 0 | 200 | 8 | 4.47M (4.43M–4.49M) | 4.49M (4.46M–4.49M) | 2.86M (2.72M–3.38M) | 2.00M (1.69M–2.30M) | 1.52M (1.49M–1.59M) | 49% | 156 |
| 0 | 1000 | 2 | 527k (524k–530k) | 529k (523k–529k) | 402k (398k–403k) | 403k (400k–404k) | 402k (399k–404k) | 100% | 0 |
| 0 | 1000 | 4 | 1.57M (1.56M–1.58M) | 1.57M (1.57M–1.58M) | 1.08M (1.08M–1.09M) | 673k (671k–677k) | 673k (664k–675k) | 100% | 0 |
| 0 | 1000 | 6 | 1.88M (1.87M–1.88M) | 1.88M (1.87M–1.89M) | 1.19M (1.13M–1.27M) | 980k (966k–1.05M) | 648k (644k–659k) | 67% | 33 |
| 0 | 1000 | 8 | 1.86M (1.85M–1.87M) | 1.87M (1.86M–1.87M) | 1.27M (1.03M–1.37M) | 1.07M (1.03M–1.13M) | 669k (605k–692k) | 49% | 67 |
| 1000 | 50 | 2 | 711k (702k–714k) | 706k (700k–711k) | 668k (663k–669k) | 668k (660k–671k) | 667k (664k–670k) | 100% | 0 |
| 1000 | 50 | 4 | 2.11M (2.10M–2.13M) | 957k (954k–958k) | 1.45M (1.45M–1.46M) | 1.98M (1.98M–1.99M) | 1.97M (1.95M–1.99M) | 100% | 1484 |
| 1000 | 50 | 6 | 3.41M (3.38M–3.43M) | 941k (939k–942k) | 2.54M (2.49M–2.63M) | 1.74M (1.74M–1.75M) | 2.79M (2.78M–2.82M) | 67% | 1464 |
| 1000 | 50 | 8 | 4.72M (4.68M–4.74M) | 935k (927k–938k) | 3.35M (2.08M–3.75M) | 2.67M (2.37M–2.73M) | 2.84M (2.73M–3.05M) | 49% | 1479 |
| 1000 | 200 | 2 | 661k (648k–667k) | 660k (648k–663k) | 575k (562k–582k) | 575k (564k–581k) | 577k (565k–578k) | 100% | 0 |
| 1000 | 200 | 4 | 1.94M (1.71M–1.97M) | 867k (865k–868k) | 1.37M (1.37M–1.38M) | 1.65M (1.65M–1.67M) | 1.66M (1.65M–1.66M) | 99% | 1260 |
| 1000 | 200 | 6 | 3.19M (3.17M–3.20M) | 858k (856k–860k) | 2.22M (2.06M–2.29M) | 1.60M (1.55M–1.63M) | 1.73M (1.66M–1.78M) | 66% | 1212 |
| 1000 | 200 | 8 | 4.47M (4.44M–4.48M) | 853k (841k–855k) | 2.45M (1.90M–2.84M) | 2.15M (2.00M–2.22M) | 1.70M (1.63M–1.79M) | 49% | 1304 |
| 1000 | 1000 | 2 | 527k (526k–528k) | 530k (528k–533k) | 381k (377k–400k) | 386k (384k–390k) | 381k (378k–393k) | 100% | 0 |
| 1000 | 1000 | 4 | 1.57M (1.56M–1.58M) | 653k (652k–655k) | 1.06M (1.05M–1.09M) | 726k (723k–729k) | 726k (718k–729k) | 100% | 742 |
| 1000 | 1000 | 6 | 1.88M (1.87M–1.89M) | 649k (649k–651k) | 1.18M (1.13M–1.29M) | 972k (965k–1.03M) | 670k (657k–691k) | 66% | 759 |
| 1000 | 1000 | 8 | 1.87M (1.85M–1.87M) | 645k (641k–646k) | 1.13M (1.03M–1.27M) | 1.15M (1.05M–1.21M) | 689k (676k–712k) | 49% | 666 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.17 / 0.21 / 0.25 (0.25–0.29) | 0.17 / 0.21 / 0.29 (0.25–0.29) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 100% | 0 |
| 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.17–0.29) | 0.08 / 0.12 / 0.21 (0.17–0.25) | 0.21 / 0.33 / 0.42 (0.37–0.58) | 0.17 / 0.37 / 0.50 (0.37–0.95) | 0.17 / 0.33 / 0.37 (0.37–0.91) | 100% | 0 |
| 0 | 50 | 6 | 0.17 / 0.25 / 0.29 (0.29–0.29) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.29 / 0.58 / 0.70 (0.62–0.70) | 0.37 / 0.95 / 1.12 (1.07–1.15) | 0.70 / 1.28 / 1.70 (1.49–1.95) | 67% | 18 |
| 0 | 50 | 8 | 0.17 / 0.33 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.29 / 0.58 / 59.39 (0.70–77.82) | 0.42 / 1.07 / 1.33 (1.25–1.49) | 1.33 / 2.14 / 6.02 (5.63–8.13) | 50% | 39 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.42 / 0.46 / 0.50 (0.50–0.54) | 0.42 / 0.46 / 0.50 (0.50–0.54) | 0.42 / 0.46 / 0.50 (0.50–0.54) | 100% | 0 |
| 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.33 / 0.62 / 0.66 (0.66–0.78) | 0.50 / 0.74 / 1.07 (0.74–1.36) | 0.50 / 0.66 / 1.15 (0.70–5.06) | 100% | 0 |
| 0 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.46 / 0.74 / 12.93 (8.96–16.90) | 0.54 / 1.70 / 1.95 (1.87–2.21) | 1.82 / 2.62 / 5.25 (3.49–7.55) | 67% | 9 |
| 0 | 200 | 8 | 0.25 / 0.42 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.50 / 0.91 / 1.41 (1.00–16.13) | 0.66 / 1.41 / 3.10 (1.78–9.34) | 3.23 / 4.22 / 12.93 (10.37–21.76) | 49% | 156 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.74) | 0.58 / 0.62 / 0.62 (0.62–1.04) | 1.15 / 1.25 / 1.28 (1.25–5.76) | 1.15 / 1.25 / 1.28 (1.25–5.31) | 1.15 / 1.25 / 1.28 (1.25–5.38) | 100% | 0 |
| 0 | 1000 | 4 | 0.58 / 0.62 / 0.78 (0.62–0.95) | 0.58 / 0.62 / 0.62 (0.62–0.74) | 0.66 / 1.74 / 1.79 (1.74–1.79) | 3.20 / 3.52 / 9.34 (5.38–13.44) | 3.20 / 3.52 / 8.58 (4.48–10.11) | 100% | 0 |
| 0 | 1000 | 6 | 0.66 / 0.79 / 145.41 (145.41–147.46) | 0.62 / 0.78 / 145.41 (143.36–147.46) | 2.08 / 2.82 / 4.80 (3.20–7.04) | 0.91 / 3.74 / 4.61 (4.42–4.61) | 6.27 / 7.30 / 13.95 (11.90–16.26) | 67% | 33 |
| 0 | 1000 | 8 | 0.66 / 0.83 / 344.06 (339.97–348.16) | 0.66 / 0.79 / 344.06 (339.97–348.16) | 1.70 / 2.82 / 6.78 (4.00–7.55) | 2.37 / 3.04 / 7.81 (6.27–10.62) | 8.96 / 10.24 / 21.50 (19.20–274.43) | 49% | 67 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 0.17 / 0.21 / 0.29 (0.25–0.29) | 0.17 / 0.21 / 0.29 (0.25–0.29) | 100% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.29) | 0.08 / 0.17 / 851.97 (843.78–868.35) | 0.21 / 0.33 / 0.42 (0.37–0.58) | 0.17 / 0.37 / 0.46 (0.42–0.58) | 0.17 / 0.37 / 0.58 (0.46–0.79) | 100% | 1484 |
| 1000 | 50 | 6 | 0.17 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 1851.39 (1802.24–1884.16) | 0.29 / 0.54 / 0.66 (0.62–0.83) | 0.33 / 0.74 / 1.00 (0.95–1.00) | 0.42 / 1.12 / 1.33 (1.25–1.36) | 67% | 1464 |
| 1000 | 50 | 8 | 0.17 / 0.33 / 0.42 (0.37–0.42) | 0.12 / 0.25 / 2981.89 (2883.58–3211.26) | 0.33 / 0.62 / 9.09 (0.66–58.88) | 0.42 / 0.87 / 1.25 (1.20–1.62) | 1.12 / 2.03 / 5.89 (3.23–11.39) | 49% | 1479 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.37) | 0.21 / 0.25 / 0.25 (0.25–0.46) | 0.42 / 0.50 / 0.54 (0.54–5.95) | 0.42 / 0.50 / 0.54 (0.54–6.08) | 0.42 / 0.50 / 0.54 (0.54–4.86) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.25 / 0.37 (0.29–0.42) | 0.17 / 0.25 / 1015.81 (999.42–1040.38) | 0.29 / 0.58 / 0.66 (0.66–0.74) | 0.42 / 0.70 / 0.91 (0.78–1.25) | 0.42 / 0.70 / 0.91 (0.79–1.07) | 99% | 1260 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.21 / 0.33 / 2195.46 (2195.46–2228.22) | 0.46 / 1.07 / 4.10 (1.36–20.22) | 0.46 / 1.41 / 1.82 (1.79–1.95) | 1.54 / 2.30 / 2.82 (2.62–7.94) | 66% | 1212 |
| 1000 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 3538.94 (3473.41–3964.93) | 0.54 / 1.33 / 11.39 (1.57–32.26) | 0.58 / 1.15 / 2.03 (1.87–2.11) | 2.72 / 3.65 / 10.11 (9.86–14.46) | 49% | 1304 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.62) | 0.58 / 0.62 / 0.62 (0.62–0.66) | 1.15 / 1.62 / 1.66 (1.62–4.22) | 1.15 / 1.62 / 1.66 (1.66–3.87) | 1.15 / 1.62 / 1.66 (1.62–4.61) | 100% | 0 |
| 1000 | 1000 | 4 | 0.58 / 0.66 / 0.70 (0.62–0.78) | 0.54 / 0.62 / 1785.86 (1753.09–1802.24) | 0.62 / 1.70 / 1.82 (1.74–1.87) | 2.72 / 2.94 / 7.49 (2.98–10.11) | 2.72 / 3.39 / 8.45 (3.58–15.23) | 100% | 742 |
| 1000 | 1000 | 6 | 0.66 / 0.79 / 145.41 (145.41–147.46) | 0.58 / 0.74 / 3735.55 (3604.48–3768.32) | 1.90 / 2.78 / 6.08 (3.94–7.10) | 0.87 / 3.46 / 4.42 (4.35–4.48) | 6.08 / 6.66 / 12.29 (11.14–16.38) | 66% | 759 |
| 1000 | 1000 | 8 | 0.66 / 0.79 / 348.16 (344.06–348.16) | 0.58 / 0.74 / 6094.85 (5963.78–6291.46) | 2.53 / 2.94 / 6.66 (4.22–8.83) | 2.14 / 2.69 / 7.42 (6.02–8.70) | 8.83 / 9.47 / 18.18 (17.66–22.27) | 49% | 666 |

Migrations seen: 43956 over 148815880 samples (one sample per measured operation).


### Suite bf: unmap, first and fair settings

load average before: { 5.42 5.99 6.12 }
load average after: { 4.97 5.34 5.73 }; 433 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 first | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 466k (463k–502k) | 484k (483k–513k) | 483k (481k–513k) | 492k (491k–492k) | 493k (491k–494k) | 492k (489k–493k) | 492k (491k–494k) | 493k (492k–493k) | 100% | 0 |
| one | 0 | 50 | 3 | 454k (450k–473k) | 471k (467k–486k) | 468k (467k–489k) | 453k (452k–464k) | 466k (463k–469k) | 482k (481k–483k) | 482k (479k–482k) | 481k (480k–483k) | 100% | 0 |
| one | 0 | 50 | 4 | 449k (446k–449k) | 460k (449k–474k) | 455k (450k–464k) | 376k (368k–388k) | 387k (383k–399k) | 467k (459k–468k) | 466k (461k–469k) | 467k (464k–471k) | 100% | 0 |
| one | 0 | 50 | 6 | 373k (361k–378k) | 379k (375k–385k) | 382k (377k–384k) | 300k (291k–310k) | 301k (294k–304k) | 310k (303k–318k) | 317k (314k–332k) | 410k (406k–415k) | 67% | 30 |
| one | 0 | 50 | 8 | 351k (302k–354k) | 352k (352k–354k) | 355k (346k–357k) | 267k (261k–283k) | 277k (264k–284k) | 246k (244k–250k) | 250k (247k–254k) | 391k (389k–396k) | 49% | 11 |
| one | 0 | 200 | 2 | 440k (439k–442k) | 465k (451k–469k) | 466k (465k–468k) | 466k (465k–467k) | 466k (466k–467k) | 467k (466k–468k) | 467k (465k–467k) | 467k (465k–467k) | 100% | 0 |
| one | 0 | 200 | 3 | 411k (406k–413k) | 435k (434k–437k) | 434k (429k–437k) | 428k (399k–430k) | 444k (430k–446k) | 453k (451k–454k) | 453k (404k–454k) | 453k (451k–454k) | 100% | 5 |
| one | 0 | 200 | 4 | 404k (395k–407k) | 428k (422k–433k) | 419k (414k–431k) | 345k (344k–346k) | 359k (358k–366k) | 441k (438k–442k) | 441k (439k–443k) | 441k (439k–443k) | 100% | 0 |
| one | 0 | 200 | 6 | 341k (339k–344k) | 357k (349k–363k) | 355k (350k–356k) | 272k (268k–276k) | 284k (282k–295k) | 284k (280k–285k) | 293k (287k–308k) | 392k (384k–395k) | 67% | 12 |
| one | 0 | 200 | 8 | 321k (319k–325k) | 331k (327k–335k) | 334k (333k–339k) | 258k (248k–265k) | 264k (257k–273k) | 231k (229k–241k) | 239k (235k–245k) | 373k (370k–377k) | 51% | 9 |
| one | 0 | 1000 | 2 | 253k (251k–254k) | 313k (312k–322k) | 312k (311k–314k) | 341k (340k–342k) | 341k (340k–342k) | 341k (341k–342k) | 341k (340k–342k) | 341k (340k–341k) | 100% | 0 |
| one | 0 | 1000 | 3 | 247k (246k–256k) | 308k (306k–309k) | 304k (302k–316k) | 251k (250k–251k) | 306k (297k–313k) | 333k (331k–333k) | 332k (325k–334k) | 332k (330k–333k) | 99% | 1 |
| one | 0 | 1000 | 4 | 246k (243k–247k) | 298k (293k–300k) | 301k (298k–303k) | 220k (220k–223k) | 264k (263k–269k) | 325k (323k–326k) | 326k (324k–327k) | 325k (325k–327k) | 100% | 1 |
| one | 0 | 1000 | 6 | 215k (209k–218k) | 264k (259k–265k) | 263k (261k–265k) | 187k (184k–193k) | 219k (207k–224k) | 198k (194k–202k) | 232k (231k–239k) | 300k (298k–300k) | 67% | 6 |
| one | 0 | 1000 | 8 | 210k (209k–212k) | 251k (248k–255k) | 252k (231k–253k) | 178k (173k–188k) | 194k (167k–214k) | 169k (163k–170k) | 192k (190k–194k) | 287k (234k–289k) | 49% | 25 |
| one | 1000 | 50 | 2 | 317k (316k–323k) | 324k (323k–332k) | 326k (321k–332k) | 329k (325k–331k) | 329k (328k–330k) | 330k (308k–330k) | 329k (319k–330k) | 329k (327k–330k) | 100% | 0 |
| one | 1000 | 50 | 3 | 315k (312k–315k) | 319k (313k–323k) | 322k (321k–323k) | 316k (311k–316k) | 320k (318k–322k) | 324k (309k–325k) | 324k (314k–325k) | 324k (324k–325k) | 100% | 0 |
| one | 1000 | 50 | 4 | 304k (298k–305k) | 313k (309k–320k) | 311k (309k–311k) | 218k (216k–222k) | 223k (220k–224k) | 318k (317k–318k) | 317k (304k–318k) | 317k (276k–318k) | 99% | 0 |
| one | 1000 | 50 | 6 | 264k (236k–274k) | 265k (260k–266k) | 265k (250k–280k) | 190k (182k–192k) | 185k (176k–191k) | 198k (178k–200k) | 202k (198k–203k) | 289k (286k–290k) | 67% | 18 |
| one | 1000 | 50 | 8 | 253k (252k–256k) | 254k (253k–260k) | 258k (240k–262k) | 181k (169k–184k) | 186k (172k–191k) | 169k (162k–169k) | 170k (167k–172k) | 278k (275k–279k) | 50% | 7 |
| one | 1000 | 200 | 2 | 305k (294k–305k) | 318k (306k–318k) | 316k (310k–317k) | 318k (316k–318k) | 318k (316k–318k) | 317k (316k–318k) | 318k (317k–318k) | 318k (317k–318k) | 100% | 0 |
| one | 1000 | 200 | 3 | 289k (287k–294k) | 305k (298k–306k) | 306k (301k–308k) | 293k (290k–294k) | 302k (299k–303k) | 311k (310k–312k) | 310k (304k–312k) | 312k (311k–312k) | 100% | 0 |
| one | 1000 | 200 | 4 | 286k (282k–287k) | 299k (294k–300k) | 297k (295k–298k) | 207k (206k–207k) | 213k (212k–214k) | 306k (300k–307k) | 306k (290k–306k) | 305k (297k–306k) | 99% | 0 |
| one | 1000 | 200 | 6 | 247k (242k–251k) | 254k (241k–257k) | 256k (247k–261k) | 172k (165k–178k) | 180k (169k–183k) | 188k (185k–192k) | 194k (192k–196k) | 279k (277k–280k) | 67% | 14 |
| one | 1000 | 200 | 8 | 238k (230k–239k) | 247k (233k–251k) | 246k (226k–248k) | 171k (144k–175k) | 181k (168k–183k) | 161k (159k–163k) | 165k (130k–168k) | 269k (241k–271k) | 51% | 31 |
| one | 1000 | 1000 | 2 | 205k (204k–206k) | 239k (238k–243k) | 243k (243k–244k) | 254k (254k–254k) | 254k (254k–254k) | 254k (253k–255k) | 254k (254k–255k) | 254k (254k–255k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 199k (199k–200k) | 235k (234k–236k) | 234k (233k–235k) | 199k (199k–204k) | 234k (234k–242k) | 250k (249k–250k) | 250k (249k–250k) | 250k (249k–250k) | 100% | 0 |
| one | 1000 | 1000 | 4 | 198k (195k–199k) | 231k (230k–234k) | 231k (224k–232k) | 156k (154k–156k) | 176k (175k–177k) | 246k (244k–246k) | 246k (245k–246k) | 245k (238k–246k) | 100% | 0 |
| one | 1000 | 1000 | 6 | 173k (152k–176k) | 204k (194k–208k) | 203k (196k–206k) | 137k (135k–139k) | 156k (145k–161k) | 144k (142k–146k) | 162k (162k–163k) | 229k (224k–230k) | 66% | 10 |
| one | 1000 | 1000 | 8 | 172k (161k–173k) | 197k (179k–199k) | 198k (195k–200k) | 135k (133k–137k) | 152k (141k–153k) | 128k (126k–129k) | 142k (126k–143k) | 221k (219k–223k) | 48% | 21 |
| all | 0 | 50 | 2 | 895k (788k–900k) | 892k (863k–898k) | 899k (863k–911k) | 823k (819k–825k) | 823k (820k–826k) | 825k (819k–828k) | 825k (821k–828k) | 823k (819k–828k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.07M (999k–1.07M) | 1.06M (1.06M–1.07M) | 1.07M (1.04M–1.08M) | 907k (898k–914k) | 896k (888k–904k) | 990k (985k–991k) | 995k (989k–998k) | 989k (980k–989k) | 100% | 0 |
| all | 0 | 50 | 4 | 1.15M (1.08M–1.16M) | 1.17M (1.15M–1.17M) | 1.17M (1.16M–1.18M) | 850k (831k–855k) | 872k (868k–885k) | 1.09M (1.09M–1.10M) | 1.10M (1.10M–1.11M) | 1.09M (1.09M–1.10M) | 100% | 0 |
| all | 0 | 50 | 6 | 1.00M (966k–1.01M) | 1.03M (1.03M–1.06M) | 1.04M (1.03M–1.04M) | 708k (659k–721k) | 693k (673k–713k) | 636k (635k–643k) | 660k (639k–695k) | 1.08M (1.07M–1.10M) | 69% | 47 |
| all | 0 | 50 | 8 | 935k (916k–954k) | 938k (888k–949k) | 930k (863k–960k) | 672k (649k–689k) | 672k (620k–691k) | 505k (495k–527k) | 519k (458k–544k) | 1.07M (1.02M–1.08M) | 50% | 76 |
| all | 0 | 200 | 2 | 690k (685k–707k) | 812k (808k–814k) | 811k (810k–815k) | 746k (742k–746k) | 746k (744k–747k) | 744k (743k–747k) | 745k (744k–748k) | 745k (742k–748k) | 100% | 0 |
| all | 0 | 200 | 3 | 851k (830k–908k) | 946k (899k–957k) | 945k (941k–976k) | 775k (756k–777k) | 782k (759k–782k) | 878k (856k–879k) | 879k (863k–880k) | 877k (862k–880k) | 100% | 5 |
| all | 0 | 200 | 4 | 992k (816k–999k) | 1.04M (1.03M–1.04M) | 1.03M (1.03M–1.04M) | 750k (748k–758k) | 802k (797k–809k) | 959k (947k–963k) | 956k (947k–965k) | 958k (947k–963k) | 100% | 22 |
| all | 0 | 200 | 6 | 905k (876k–906k) | 934k (926k–950k) | 952k (938k–963k) | 634k (620k–644k) | 627k (625k–641k) | 599k (582k–616k) | 613k (592k–638k) | 946k (898k–954k) | 68% | 135 |
| all | 0 | 200 | 8 | 834k (766k–838k) | 853k (793k–869k) | 845k (824k–855k) | 602k (597k–619k) | 679k (629k–702k) | 472k (459k–518k) | 501k (488k–517k) | 951k (850k–957k) | 49% | 112 |
| all | 0 | 1000 | 2 | 382k (379k–484k) | 498k (496k–503k) | 497k (494k–502k) | 467k (465k–468k) | 467k (466k–469k) | 466k (466k–468k) | 467k (465k–470k) | 467k (466k–469k) | 100% | 0 |
| all | 0 | 1000 | 3 | 461k (455k–490k) | 602k (601k–610k) | 600k (591k–604k) | 475k (473k–477k) | 475k (473k–477k) | 516k (515k–516k) | 516k (515k–517k) | 516k (516k–517k) | 100% | 0 |
| all | 0 | 1000 | 4 | 556k (550k–559k) | 658k (658k–661k) | 655k (650k–657k) | 444k (441k–446k) | 487k (479k–490k) | 543k (540k–545k) | 543k (541k–544k) | 544k (543k–545k) | 100% | 0 |
| all | 0 | 1000 | 6 | 508k (507k–516k) | 586k (578k–595k) | 587k (577k–589k) | 407k (394k–410k) | 422k (416k–435k) | 391k (388k–392k) | 403k (401k–405k) | 519k (506k–530k) | 67% | 88 |
| all | 0 | 1000 | 8 | 508k (505k–515k) | 526k (518k–534k) | 525k (504k–534k) | 401k (376k–423k) | 432k (418k–464k) | 330k (311k–339k) | 353k (314k–372k) | 524k (504k–534k) | 49% | 127 |
| all | 1000 | 50 | 2 | 456k (455k–457k) | 462k (458k–463k) | 627k (548k–752k) | 801k (799k–804k) | 800k (795k–803k) | 801k (792k–809k) | 800k (798k–809k) | 803k (800k–803k) | 100% | 0 |
| all | 1000 | 50 | 3 | 632k (605k–639k) | 635k (605k–641k) | 524k (515k–528k) | 530k (518k–531k) | 530k (522k–531k) | 982k (975k–990k) | 988k (975k–993k) | 985k (974k–985k) | 100% | 0 |
| all | 1000 | 50 | 4 | 710k (704k–729k) | 730k (722k–737k) | 575k (574k–579k) | 571k (570k–572k) | 570k (560k–571k) | 1.09M (1.08M–1.09M) | 1.09M (1.09M–1.10M) | 1.09M (1.08M–1.09M) | 100% | 0 |
| all | 1000 | 50 | 6 | 719k (717k–723k) | 738k (730k–744k) | 568k (560k–570k) | 566k (484k–577k) | 586k (567k–598k) | 548k (536k–561k) | 558k (555k–568k) | 902k (899k–907k) | 68% | 70 |
| all | 1000 | 50 | 8 | 741k (735k–751k) | 755k (752k–757k) | 550k (549k–559k) | 588k (585k–591k) | 589k (576k–630k) | 483k (469k–498k) | 503k (496k–512k) | 928k (924k–932k) | 50% | 67 |
| all | 1000 | 200 | 2 | 407k (405k–418k) | 425k (424k–446k) | 789k (777k–799k) | 743k (736k–745k) | 745k (744k–747k) | 745k (741k–747k) | 745k (741k–749k) | 745k (741k–747k) | 100% | 0 |
| all | 1000 | 200 | 3 | 543k (539k–572k) | 573k (572k–591k) | 513k (511k–527k) | 565k (541k–614k) | 576k (549k–609k) | 877k (871k–877k) | 877k (874k–879k) | 878k (875k–882k) | 100% | 0 |
| all | 1000 | 200 | 4 | 648k (645k–652k) | 661k (656k–678k) | 532k (532k–535k) | 544k (538k–546k) | 556k (554k–561k) | 955k (951k–961k) | 953k (952k–960k) | 956k (953k–963k) | 100% | 0 |
| all | 1000 | 200 | 6 | 653k (650k–658k) | 692k (684k–694k) | 548k (536k–550k) | 534k (524k–545k) | 558k (545k–562k) | 515k (499k–516k) | 529k (523k–537k) | 856k (842k–861k) | 68% | 115 |
| all | 1000 | 200 | 8 | 677k (657k–685k) | 691k (686k–703k) | 522k (377k–527k) | 550k (436k–641k) | 559k (541k–616k) | 450k (420k–458k) | 467k (435k–494k) | 850k (798k–860k) | 49% | 317 |
| all | 1000 | 1000 | 2 | 281k (279k–285k) | 359k (356k–367k) | 497k (494k–498k) | 465k (449k–466k) | 465k (456k–467k) | 466k (465k–468k) | 467k (466k–467k) | 467k (465k–468k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 386k (383k–397k) | 425k (421k–433k) | 471k (466k–473k) | 421k (416k–424k) | 476k (469k–477k) | 516k (515k–518k) | 516k (512k–516k) | 516k (514k–516k) | 100% | 0 |
| all | 1000 | 1000 | 4 | 408k (403k–409k) | 502k (478k–511k) | 536k (522k–541k) | 388k (377k–393k) | 466k (463k–469k) | 538k (524k–543k) | 543k (532k–544k) | 540k (535k–544k) | 99% | 0 |
| all | 1000 | 1000 | 6 | 443k (418k–447k) | 480k (473k–497k) | 460k (458k–487k) | 383k (375k–384k) | 409k (401k–418k) | 370k (362k–381k) | 399k (397k–403k) | 501k (496k–505k) | 67% | 90 |
| all | 1000 | 1000 | 8 | 450k (445k–451k) | 476k (475k–481k) | 415k (391k–427k) | 384k (357k–393k) | 417k (394k–437k) | 326k (323k–337k) | 363k (357k–367k) | 498k (486k–504k) | 50% | 35 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 first | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.79 / 0.91 / 1.00 (0.87–4.61) | 0.70 / 0.83 / 0.91 (0.83–4.42) | 0.70 / 0.83 / 0.91 (0.83–1.25) | 0.66 / 0.74 / 0.87 (0.83–1.07) | 0.66 / 0.74 / 0.83 (0.83–0.83) | 0.66 / 0.74 / 0.83 (0.79–6.40) | 0.66 / 0.74 / 0.83 (0.83–0.87) | 0.66 / 0.74 / 0.83 (0.79–0.83) | 100% | 0 |
| one | 0 | 50 | 3 | 0.83 / 0.95 / 2.08 (1.04–6.08) | 0.78 / 0.87 / 1.04 (0.95–6.53) | 0.78 / 0.87 / 1.04 (0.95–6.53) | 0.83 / 1.04 / 1.54 (1.07–8.00) | 0.78 / 1.00 / 1.12 (1.07–6.34) | 0.70 / 0.79 / 0.91 (0.87–4.35) | 0.70 / 0.79 / 0.87 (0.87–7.62) | 0.70 / 0.79 / 2.24 (0.83–6.66) | 100% | 0 |
| one | 0 | 50 | 4 | 0.87 / 1.00 / 4.00 (1.15–7.55) | 0.79 / 1.00 / 6.27 (1.58–7.68) | 0.79 / 1.12 / 5.18 (1.15–11.01) | 1.28 / 1.66 / 8.45 (1.74–13.82) | 1.20 / 1.66 / 7.87 (3.26–14.21) | 0.74 / 0.87 / 5.18 (3.62–12.42) | 0.74 / 0.87 / 3.94 (0.91–10.37) | 0.74 / 0.83 / 5.44 (1.49–7.81) | 100% | 0 |
| one | 0 | 50 | 6 | 1.28 / 1.66 / 6.27 (4.22–6.53) | 1.28 / 1.62 / 4.61 (4.29–6.21) | 1.25 / 1.62 / 4.48 (3.36–8.45) | 2.00 / 2.82 / 7.04 (4.48–9.47) | 1.95 / 2.78 / 7.04 (4.93–10.24) | 1.90 / 2.50 / 6.40 (4.35–9.34) | 1.82 / 2.43 / 7.49 (4.22–9.22) | 1.07 / 1.25 / 5.50 (3.55–7.68) | 67% | 30 |
| one | 0 | 50 | 8 | 1.54 / 1.90 / 7.62 (6.78–67.58) | 1.49 / 1.87 / 7.94 (5.57–9.22) | 1.49 / 1.87 / 7.81 (6.78–14.46) | 2.30 / 3.39 / 7.68 (7.23–10.62) | 2.11 / 3.39 / 8.83 (7.04–13.44) | 2.72 / 3.36 / 7.87 (7.81–11.78) | 2.62 / 3.36 / 9.09 (7.49–12.42) | 1.20 / 1.36 / 6.59 (4.42–8.83) | 49% | 11 |
| one | 0 | 200 | 2 | 0.91 / 1.04 / 1.15 (1.12–4.80) | 0.79 / 0.91 / 1.04 (1.04–1.07) | 0.79 / 0.91 / 1.00 (1.00–1.00) | 0.79 / 0.91 / 0.95 (0.95–4.42) | 0.79 / 0.87 / 0.95 (0.95–1.74) | 0.79 / 0.91 / 0.95 (0.95–1.00) | 0.79 / 0.87 / 0.95 (0.91–1.00) | 0.79 / 0.87 / 0.95 (0.95–1.00) | 100% | 0 |
| one | 0 | 200 | 3 | 1.04 / 1.20 / 4.00 (1.33–7.68) | 0.91 / 1.07 / 3.33 (1.20–5.12) | 0.95 / 1.07 / 4.22 (1.20–4.48) | 0.95 / 1.25 / 3.14 (1.28–6.66) | 0.87 / 1.12 / 1.33 (1.28–6.53) | 0.83 / 0.91 / 4.93 (1.04–6.53) | 0.83 / 0.91 / 2.94 (1.00–10.37) | 0.83 / 0.91 / 4.29 (1.00–5.18) | 100% | 5 |
| one | 0 | 200 | 4 | 1.12 / 1.28 / 4.93 (2.43–9.34) | 0.95 / 1.12 / 4.67 (1.28–8.19) | 1.00 / 1.20 / 6.40 (1.28–8.83) | 1.54 / 1.87 / 6.53 (4.16–8.13) | 1.41 / 1.82 / 5.44 (4.86–7.87) | 0.87 / 1.00 / 5.82 (1.90–8.83) | 0.87 / 1.00 / 4.42 (3.74–8.58) | 0.87 / 1.00 / 5.95 (1.33–8.13) | 100% | 0 |
| one | 0 | 200 | 6 | 1.54 / 1.90 / 6.66 (4.67–7.10) | 1.41 / 1.79 / 6.40 (3.26–8.45) | 1.46 / 1.79 / 5.57 (4.22–6.21) | 2.27 / 3.20 / 6.34 (5.38–10.50) | 2.14 / 2.85 / 5.82 (4.80–8.58) | 2.24 / 2.72 / 6.40 (4.42–7.42) | 2.08 / 2.62 / 6.78 (4.74–8.58) | 1.20 / 1.41 / 4.29 (3.65–9.60) | 67% | 12 |
| one | 0 | 200 | 8 | 1.74 / 2.14 / 8.83 (6.53–10.88) | 1.66 / 2.08 / 6.78 (6.27–9.98) | 1.66 / 2.00 / 7.42 (6.21–8.70) | 2.43 / 3.74 / 8.83 (7.62–10.62) | 2.37 / 3.46 / 7.62 (7.36–8.96) | 2.98 / 3.62 / 9.73 (7.17–10.88) | 2.82 / 3.52 / 8.70 (8.19–8.96) | 1.33 / 1.49 / 6.27 (4.74–9.09) | 51% | 9 |
| one | 0 | 1000 | 2 | 2.56 / 2.69 / 8.06 (2.78–11.39) | 1.82 / 1.95 / 2.08 (2.00–6.78) | 1.82 / 1.95 / 2.14 (2.08–7.62) | 1.57 / 1.66 / 2.00 (1.74–5.82) | 1.57 / 1.66 / 1.78 (1.74–7.10) | 1.57 / 1.66 / 1.79 (1.74–2.94) | 1.57 / 1.66 / 4.74 (1.74–5.95) | 1.57 / 1.66 / 1.79 (1.74–4.67) | 100% | 0 |
| one | 0 | 1000 | 3 | 2.66 / 2.82 / 9.86 (6.21–12.93) | 1.87 / 2.00 / 7.42 (2.11–10.24) | 1.90 / 2.03 / 5.18 (4.22–10.24) | 2.62 / 2.82 / 7.36 (4.48–9.47) | 1.87 / 2.11 / 5.70 (2.24–8.58) | 1.62 / 1.70 / 5.25 (1.95–5.70) | 1.62 / 1.70 / 5.31 (1.79–8.32) | 1.62 / 1.70 / 6.21 (1.82–7.74) | 99% | 1 |
| one | 0 | 1000 | 4 | 2.69 / 2.82 / 8.45 (7.23–11.26) | 2.00 / 2.11 / 7.04 (5.57–10.11) | 1.95 / 2.11 / 7.04 (2.27–8.96) | 3.14 / 3.52 / 8.06 (7.17–10.62) | 2.40 / 2.78 / 8.32 (2.85–9.86) | 1.70 / 1.79 / 8.19 (2.30–9.60) | 1.66 / 1.74 / 6.46 (1.82–8.96) | 1.70 / 1.78 / 7.10 (5.57–8.00) | 100% | 1 |
| one | 0 | 1000 | 6 | 3.26 / 3.74 / 8.19 (7.94–9.73) | 2.43 / 2.82 / 8.00 (5.57–8.70) | 2.40 / 2.78 / 6.21 (5.82–8.32) | 4.00 / 4.80 / 8.19 (7.23–8.96) | 3.14 / 4.29 / 9.22 (6.98–20.22) | 3.68 / 4.42 / 8.45 (6.40–10.11) | 2.98 / 3.58 / 6.53 (5.70–7.74) | 2.00 / 2.21 / 6.02 (4.54–7.04) | 67% | 6 |
| one | 0 | 1000 | 8 | 3.39 / 3.87 / 10.88 (9.09–14.59) | 2.62 / 3.04 / 8.45 (7.49–11.90) | 2.62 / 3.04 / 8.58 (8.00–29.44) | 4.10 / 5.44 / 11.78 (10.11–15.10) | 3.58 / 4.74 / 9.22 (8.70–178.18) | 4.61 / 5.25 / 10.62 (9.86–32.00) | 3.87 / 4.54 / 11.01 (9.09–14.21) | 2.14 / 2.27 / 8.83 (6.91–76.80) | 49% | 25 |
| one | 1000 | 50 | 2 | 1.78 / 1.90 / 3.68 (2.03–8.70) | 1.70 / 1.82 / 4.00 (1.90–10.24) | 1.70 / 1.82 / 2.56 (1.90–12.03) | 1.66 / 1.78 / 4.86 (1.87–13.06) | 1.66 / 1.78 / 5.82 (1.87–9.09) | 1.66 / 1.74 / 1.87 (1.82–18.94) | 1.66 / 1.78 / 5.12 (1.82–14.72) | 1.66 / 1.74 / 5.25 (2.00–7.36) | 100% | 0 |
| one | 1000 | 50 | 3 | 1.79 / 1.95 / 6.27 (4.00–9.22) | 1.78 / 1.87 / 5.76 (2.53–13.18) | 1.74 / 1.87 / 6.53 (2.56–8.06) | 1.79 / 1.90 / 6.02 (2.00–13.95) | 1.74 / 1.87 / 4.80 (2.85–8.83) | 1.70 / 1.79 / 6.59 (2.08–19.97) | 1.70 / 1.82 / 7.42 (2.37–16.38) | 1.70 / 1.79 / 6.98 (1.87–8.00) | 100% | 0 |
| one | 1000 | 50 | 4 | 1.90 / 2.11 / 7.49 (4.29–16.64) | 1.82 / 2.00 / 7.04 (2.69–12.29) | 1.82 / 2.00 / 7.36 (6.08–9.47) | 3.14 / 3.49 / 9.47 (6.78–10.62) | 3.07 / 3.36 / 8.83 (5.76–10.50) | 1.74 / 1.87 / 6.72 (2.11–8.58) | 1.74 / 1.87 / 8.19 (6.08–15.87) | 1.74 / 1.90 / 7.42 (7.23–45.06) | 99% | 0 |
| one | 1000 | 50 | 6 | 2.37 / 2.78 / 8.00 (5.25–60.42) | 2.37 / 2.91 / 8.83 (6.21–14.59) | 2.37 / 2.85 / 8.45 (6.46–12.80) | 3.87 / 4.61 / 10.37 (7.10–14.21) | 3.90 / 4.67 / 10.37 (8.45–47.10) | 3.65 / 4.22 / 10.24 (7.23–24.32) | 3.62 / 4.10 / 7.68 (6.40–12.80) | 2.08 / 2.27 / 7.23 (5.70–8.58) | 67% | 18 |
| one | 1000 | 50 | 8 | 2.56 / 2.98 / 9.09 (8.70–11.01) | 2.56 / 2.94 / 8.83 (8.13–9.86) | 2.53 / 2.91 / 9.73 (8.45–22.02) | 4.10 / 4.86 / 12.42 (9.98–64.51) | 4.00 / 4.54 / 12.80 (9.98–22.02) | 4.22 / 4.93 / 12.29 (9.34–60.93) | 4.16 / 6.08 / 14.08 (9.98–43.01) | 2.24 / 2.40 / 9.22 (7.42–12.29) | 50% | 7 |
| one | 1000 | 200 | 2 | 1.90 / 2.03 / 2.27 (2.24–7.49) | 1.79 / 1.90 / 2.08 (2.03–5.44) | 1.79 / 1.90 / 2.11 (2.03–5.31) | 1.79 / 1.87 / 2.00 (1.95–8.58) | 1.79 / 1.90 / 2.00 (1.95–8.70) | 1.79 / 1.90 / 2.94 (2.00–9.73) | 1.79 / 1.90 / 2.14 (1.95–8.70) | 1.79 / 1.87 / 2.00 (2.00–5.38) | 100% | 0 |
| one | 1000 | 200 | 3 | 2.03 / 2.21 / 6.59 (2.40–14.72) | 1.90 / 2.03 / 5.31 (2.21–7.81) | 1.87 / 2.03 / 6.34 (2.11–10.11) | 2.03 / 2.21 / 6.53 (2.30–8.96) | 1.90 / 2.08 / 6.27 (2.21–6.91) | 1.82 / 1.90 / 7.42 (2.03–7.87) | 1.82 / 1.90 / 6.72 (5.70–13.95) | 1.82 / 1.90 / 5.25 (2.00–7.17) | 100% | 0 |
| one | 1000 | 200 | 4 | 2.11 / 2.27 / 7.23 (6.72–8.83) | 1.95 / 2.14 / 7.68 (5.95–8.96) | 2.00 / 2.21 / 8.00 (5.76–8.83) | 3.39 / 3.74 / 10.62 (9.22–11.39) | 3.26 / 3.62 / 9.34 (8.45–10.37) | 1.87 / 2.00 / 7.68 (4.93–13.18) | 1.87 / 2.00 / 7.30 (5.95–18.18) | 1.87 / 1.95 / 9.47 (4.06–16.13) | 99% | 0 |
| one | 1000 | 200 | 6 | 2.66 / 3.07 / 8.19 (5.70–16.90) | 2.53 / 2.94 / 8.96 (6.66–17.41) | 2.53 / 2.85 / 7.55 (6.40–16.90) | 4.42 / 5.06 / 9.98 (8.06–13.95) | 4.16 / 4.93 / 10.37 (8.06–11.65) | 3.90 / 4.42 / 9.22 (8.19–10.11) | 3.78 / 4.22 / 9.09 (6.98–9.47) | 2.21 / 2.40 / 6.46 (5.44–7.87) | 67% | 14 |
| one | 1000 | 200 | 8 | 2.85 / 3.23 / 10.50 (8.45–27.39) | 2.69 / 3.04 / 9.60 (8.70–15.36) | 2.72 / 3.07 / 9.34 (8.45–35.84) | 4.35 / 4.99 / 11.52 (10.50–219.14) | 4.16 / 4.61 / 11.39 (10.75–45.57) | 4.48 / 5.12 / 12.42 (10.24–22.27) | 4.35 / 4.99 / 10.88 (10.50–239.62) | 2.37 / 2.53 / 9.86 (8.00–67.58) | 51% | 31 |
| one | 1000 | 1000 | 2 | 3.49 / 3.62 / 8.83 (3.74–11.01) | 2.82 / 2.91 / 6.21 (3.04–9.09) | 2.72 / 2.85 / 6.02 (2.94–9.86) | 2.56 / 2.66 / 7.23 (3.65–7.68) | 2.56 / 2.66 / 5.95 (2.78–7.23) | 2.56 / 2.66 / 2.82 (2.72–8.58) | 2.56 / 2.66 / 2.82 (2.78–8.96) | 2.56 / 2.66 / 2.82 (2.72–7.74) | 100% | 0 |
| one | 1000 | 1000 | 3 | 3.65 / 3.78 / 9.73 (3.90–10.62) | 2.85 / 3.04 / 8.83 (3.14–11.65) | 2.91 / 3.04 / 7.62 (7.23–9.60) | 3.62 / 3.78 / 7.36 (4.99–8.83) | 2.85 / 3.04 / 7.68 (5.38–9.60) | 2.62 / 2.69 / 6.78 (5.89–10.50) | 2.62 / 2.69 / 7.74 (3.33–8.83) | 2.62 / 2.69 / 6.27 (5.70–9.60) | 100% | 0 |
| one | 1000 | 1000 | 4 | 3.65 / 3.87 / 10.50 (8.45–12.42) | 2.94 / 3.23 / 10.11 (7.94–11.14) | 2.91 / 3.20 / 10.37 (9.34–15.62) | 4.99 / 5.25 / 12.80 (11.14–14.72) | 4.29 / 4.61 / 11.52 (9.60–14.85) | 2.66 / 2.78 / 8.58 (2.85–11.78) | 2.66 / 2.72 / 8.58 (8.13–9.98) | 2.66 / 2.72 / 10.62 (8.00–17.66) | 100% | 0 |
| one | 1000 | 1000 | 6 | 4.29 / 4.93 / 11.90 (10.11–72.70) | 3.52 / 4.00 / 10.88 (7.74–17.41) | 3.52 / 4.03 / 9.60 (8.83–11.14) | 5.82 / 6.78 / 12.42 (10.37–12.93) | 5.06 / 5.70 / 11.26 (9.34–13.95) | 5.57 / 6.08 / 10.37 (8.83–11.90) | 4.74 / 5.31 / 9.22 (8.32–11.39) | 2.98 / 3.23 / 8.00 (6.08–13.70) | 66% | 10 |
| one | 1000 | 1000 | 8 | 4.42 / 5.12 / 13.44 (12.29–82.94) | 3.68 / 4.35 / 13.06 (10.37–69.63) | 3.68 / 4.16 / 12.16 (11.01–17.15) | 5.95 / 6.98 / 15.87 (14.46–18.69) | 5.25 / 5.76 / 13.70 (11.65–20.74) | 6.08 / 6.85 / 13.82 (12.80–23.04) | 5.31 / 6.02 / 13.31 (11.14–221.18) | 3.20 / 3.33 / 11.01 (9.09–18.94) | 48% | 21 |
| all | 0 | 50 | 2 | 0.78 / 1.12 / 1.25 (1.20–6.34) | 0.74 / 1.04 / 1.12 (1.12–5.89) | 0.74 / 1.07 / 1.20 (1.15–1.25) | 1.07 / 1.20 / 1.28 (1.28–3.78) | 1.07 / 1.20 / 1.36 (1.28–4.10) | 1.07 / 1.25 / 1.28 (1.28–4.67) | 1.07 / 1.20 / 1.33 (1.28–1.36) | 1.07 / 1.25 / 1.28 (1.28–4.86) | 100% | 0 |
| all | 0 | 50 | 3 | 1.04 / 1.58 / 2.43 (1.74–8.06) | 0.95 / 1.54 / 3.90 (1.74–7.62) | 1.00 / 1.54 / 4.29 (1.66–8.45) | 1.62 / 2.37 / 5.31 (2.50–8.83) | 1.70 / 2.40 / 6.59 (2.50–7.74) | 1.66 / 1.82 / 2.82 (1.90–7.42) | 1.66 / 1.82 / 5.31 (1.95–6.21) | 1.66 / 1.82 / 6.21 (1.90–8.19) | 100% | 0 |
| all | 0 | 50 | 4 | 1.49 / 2.24 / 9.22 (4.74–16.38) | 1.15 / 2.08 / 6.21 (3.14–10.88) | 1.07 / 2.00 / 5.31 (2.30–11.26) | 2.50 / 3.94 / 6.66 (4.16–11.39) | 2.30 / 3.74 / 7.36 (4.42–7.94) | 2.27 / 2.43 / 6.59 (2.56–9.86) | 2.24 / 2.43 / 5.06 (2.50–7.42) | 2.27 / 2.43 / 5.50 (2.66–7.74) | 100% | 0 |
| all | 0 | 50 | 6 | 2.69 / 6.40 / 9.60 (8.96–12.03) | 2.53 / 6.46 / 8.96 (8.70–11.01) | 2.53 / 6.34 / 9.09 (9.09–12.54) | 5.25 / 9.34 / 13.57 (11.01–16.64) | 5.63 / 9.34 / 12.54 (11.78–14.59) | 6.46 / 9.98 / 14.21 (13.18–16.38) | 6.14 / 9.60 / 14.08 (12.03–16.13) | 4.03 / 5.44 / 10.75 (6.46–17.15) | 69% | 47 |
| all | 0 | 50 | 8 | 4.80 / 11.78 / 16.00 (14.72–16.26) | 4.86 / 12.42 / 15.74 (15.10–128.00) | 4.99 / 12.16 / 14.98 (14.21–141.31) | 8.00 / 14.21 / 22.27 (19.20–159.74) | 8.45 / 14.08 / 20.99 (17.92–159.74) | 12.16 / 17.66 / 26.88 (23.04–53.25) | 11.14 / 17.66 / 28.93 (24.83–294.91) | 6.02 / 8.19 / 14.98 (11.65–52.74) | 50% | 76 |
| all | 0 | 200 | 2 | 1.04 / 1.58 / 3.39 (1.66–4.99) | 1.07 / 1.41 / 1.62 (1.46–5.06) | 1.07 / 1.41 / 1.46 (1.41–2.03) | 1.33 / 1.46 / 1.62 (1.54–2.24) | 1.33 / 1.46 / 1.57 (1.54–6.02) | 1.33 / 1.46 / 2.66 (1.54–5.95) | 1.33 / 1.49 / 1.74 (1.54–7.10) | 1.33 / 1.49 / 1.79 (1.57–7.49) | 100% | 0 |
| all | 0 | 200 | 3 | 1.28 / 2.21 / 6.59 (4.80–9.47) | 1.28 / 2.00 / 4.74 (2.27–23.04) | 1.36 / 2.00 / 4.86 (2.03–7.04) | 2.53 / 2.78 / 6.53 (2.94–15.87) | 2.50 / 2.72 / 7.04 (2.91–16.26) | 2.03 / 2.21 / 5.31 (3.39–15.23) | 2.03 / 2.21 / 6.27 (2.40–15.10) | 2.03 / 2.21 / 6.53 (2.27–14.98) | 100% | 5 |
| all | 0 | 200 | 4 | 2.11 / 2.82 / 8.06 (3.90–21.76) | 1.46 / 2.56 / 7.81 (2.91–11.39) | 1.57 / 2.66 / 6.53 (5.06–8.83) | 3.20 / 4.61 / 8.32 (5.76–12.67) | 2.98 / 4.16 / 9.98 (8.06–12.42) | 2.78 / 2.94 / 7.49 (6.66–13.70) | 2.78 / 2.94 / 7.74 (3.36–10.37) | 2.78 / 2.98 / 9.86 (3.20–13.57) | 100% | 22 |
| all | 0 | 200 | 6 | 3.14 / 6.66 / 12.54 (10.24–14.21) | 2.94 / 7.62 / 10.75 (10.37–13.82) | 2.78 / 7.17 / 10.88 (9.47–11.52) | 6.21 / 10.50 / 14.98 (12.93–21.50) | 6.66 / 10.11 / 15.36 (13.06–26.88) | 7.10 / 10.75 / 16.90 (14.59–40.45) | 7.10 / 10.24 / 16.13 (13.95–26.11) | 4.99 / 6.21 / 10.75 (9.34–23.55) | 68% | 135 |
| all | 0 | 200 | 8 | 5.31 / 12.80 / 17.66 (16.38–221.18) | 5.50 / 14.59 / 18.69 (16.38–92.16) | 5.57 / 14.46 / 17.41 (16.38–30.98) | 9.73 / 15.62 / 22.27 (20.74–23.81) | 8.19 / 14.72 / 20.22 (18.18–22.53) | 12.93 / 18.94 / 30.21 (26.62–89.09) | 11.65 / 18.94 / 27.39 (25.86–68.61) | 7.17 / 9.09 / 17.41 (14.46–153.60) | 49% | 112 |
| all | 0 | 1000 | 2 | 2.62 / 3.90 / 7.04 (6.02–9.34) | 2.62 / 2.78 / 4.48 (3.07–6.85) | 2.62 / 2.78 / 7.10 (2.91–8.00) | 2.91 / 3.07 / 6.21 (3.33–7.68) | 2.91 / 3.07 / 4.00 (3.23–8.32) | 2.91 / 3.07 / 6.91 (3.14–8.13) | 2.91 / 3.07 / 3.26 (3.14–8.00) | 2.91 / 3.07 / 4.61 (3.20–8.32) | 100% | 0 |
| all | 0 | 1000 | 3 | 2.85 / 5.38 / 10.62 (7.49–12.29) | 2.30 / 4.10 / 6.40 (4.22–8.58) | 2.30 / 4.10 / 8.58 (7.36–14.85) | 4.93 / 5.12 / 11.78 (10.11–14.59) | 4.93 / 5.12 / 9.47 (7.68–15.10) | 4.42 / 4.61 / 9.73 (8.83–11.90) | 4.42 / 4.61 / 8.70 (8.13–11.39) | 4.42 / 4.61 / 8.19 (4.86–11.78) | 100% | 0 |
| all | 0 | 1000 | 4 | 4.48 / 6.27 / 15.36 (9.47–22.53) | 2.78 / 8.19 / 8.45 (8.32–13.18) | 2.82 / 5.57 / 8.32 (8.32–13.70) | 6.66 / 9.22 / 19.97 (11.39–22.27) | 6.78 / 7.17 / 11.52 (7.36–17.41) | 5.95 / 6.21 / 11.26 (7.36–20.74) | 5.95 / 6.14 / 14.08 (10.62–24.58) | 5.95 / 6.14 / 11.14 (6.66–17.15) | 100% | 0 |
| all | 0 | 1000 | 6 | 7.49 / 12.03 / 19.71 (17.41–20.99) | 6.08 / 13.18 / 16.90 (15.87–17.66) | 6.08 / 13.44 / 16.38 (15.87–17.41) | 11.90 / 16.90 / 23.55 (22.53–25.34) | 12.03 / 14.98 / 20.99 (20.22–21.50) | 12.54 / 18.69 / 22.78 (22.53–26.62) | 13.06 / 15.36 / 25.09 (21.76–26.88) | 10.37 / 11.14 / 16.38 (16.26–18.94) | 67% | 88 |
| all | 0 | 1000 | 8 | 8.45 / 19.71 / 26.62 (26.11–28.16) | 11.65 / 20.22 / 26.62 (25.09–66.56) | 10.75 / 21.50 / 26.88 (25.09–253.95) | 16.00 / 25.09 / 33.79 (32.00–217.09) | 15.10 / 23.81 / 30.72 (28.16–178.18) | 21.50 / 29.95 / 39.94 (36.86–262.14) | 19.71 / 25.09 / 39.94 (34.30–606.21) | 14.34 / 15.62 / 23.04 (22.27–25.60) | 49% | 127 |
| all | 1000 | 50 | 2 | 1.66 / 1.82 / 4.00 (2.11–4.80) | 1.62 / 1.78 / 4.93 (2.11–6.59) | 0.87 / 2.00 / 2.11 (2.08–5.12) | 1.07 / 1.79 / 1.90 (1.90–2.00) | 1.07 / 1.79 / 2.03 (1.90–4.86) | 1.07 / 1.79 / 1.90 (1.87–7.74) | 1.07 / 1.79 / 1.90 (1.87–1.90) | 1.07 / 1.78 / 1.90 (1.87–1.95) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.14 / 3.26 / 4.29 (3.55–8.45) | 2.11 / 3.52 / 8.06 (3.74–8.83) | 1.87 / 4.61 / 6.59 (6.08–9.47) | 2.00 / 3.62 / 7.04 (3.74–8.19) | 2.00 / 3.62 / 8.32 (6.40–9.73) | 1.66 / 1.87 / 5.25 (1.90–7.30) | 1.66 / 1.82 / 2.00 (1.95–7.87) | 1.66 / 1.82 / 5.95 (1.95–6.59) | 100% | 0 |
| all | 1000 | 50 | 4 | 2.37 / 4.42 / 6.08 (4.61–11.26) | 2.24 / 4.22 / 8.19 (5.95–11.14) | 2.78 / 6.02 / 10.62 (8.70–12.42) | 3.23 / 5.06 / 8.45 (5.82–11.01) | 3.20 / 4.99 / 10.75 (5.57–14.08) | 2.27 / 2.43 / 7.30 (5.82–8.00) | 2.27 / 2.40 / 3.90 (2.50–7.23) | 2.27 / 2.43 / 7.10 (4.29–8.32) | 100% | 0 |
| all | 1000 | 50 | 6 | 3.74 / 7.49 / 10.62 (9.47–12.80) | 3.65 / 7.36 / 9.09 (8.83–11.90) | 4.61 / 11.52 / 15.23 (13.70–17.41) | 5.44 / 9.73 / 13.70 (11.90–18.43) | 5.25 / 9.47 / 12.29 (11.65–14.98) | 5.44 / 9.86 / 14.72 (12.54–16.64) | 5.25 / 9.47 / 12.93 (11.65–16.13) | 3.46 / 5.44 / 10.24 (7.87–10.75) | 68% | 70 |
| all | 1000 | 50 | 8 | 5.38 / 10.50 / 16.38 (14.72–17.41) | 5.38 / 10.75 / 14.21 (13.82–16.64) | 7.30 / 19.46 / 23.81 (22.78–25.34) | 7.49 / 13.57 / 19.97 (18.43–21.25) | 7.30 / 13.44 / 19.46 (18.94–20.99) | 9.34 / 17.15 / 27.14 (22.02–30.46) | 8.58 / 16.00 / 26.11 (22.78–26.62) | 4.35 / 7.87 / 13.31 (12.42–13.82) | 50% | 67 |
| all | 1000 | 200 | 2 | 2.03 / 2.53 / 6.21 (2.53–7.36) | 1.87 / 2.00 / 2.37 (2.03–6.91) | 1.04 / 2.40 / 2.50 (2.43–2.50) | 1.33 / 1.49 / 2.11 (1.82–8.00) | 1.33 / 1.49 / 1.95 (1.87–2.30) | 1.33 / 1.49 / 2.00 (1.90–2.30) | 1.33 / 1.46 / 1.90 (1.57–2.08) | 1.33 / 1.49 / 1.82 (1.58–2.11) | 100% | 0 |
| all | 1000 | 200 | 3 | 2.62 / 3.74 / 10.11 (6.72–11.39) | 2.37 / 3.90 / 6.27 (5.57–8.83) | 2.11 / 4.74 / 5.44 (4.86–11.01) | 2.50 / 3.68 / 8.83 (5.70–9.86) | 2.53 / 3.68 / 4.99 (3.87–8.96) | 2.03 / 2.21 / 6.21 (2.56–7.49) | 2.03 / 2.21 / 6.34 (3.10–8.06) | 2.03 / 2.21 / 2.43 (2.24–5.70) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.94 / 4.93 / 9.98 (9.60–10.62) | 2.53 / 4.54 / 9.60 (8.83–13.18) | 3.26 / 6.34 / 7.87 (6.78–11.52) | 3.62 / 5.76 / 12.93 (10.75–15.62) | 3.49 / 5.31 / 9.34 (8.19–12.42) | 2.78 / 2.94 / 6.66 (3.07–8.32) | 2.78 / 2.98 / 9.86 (4.10–10.24) | 2.78 / 2.94 / 7.62 (3.26–9.73) | 100% | 0 |
| all | 1000 | 200 | 6 | 4.22 / 8.19 / 11.14 (9.60–15.10) | 4.16 / 8.06 / 12.42 (8.96–13.57) | 4.80 / 11.90 / 15.23 (14.98–17.15) | 6.21 / 10.62 / 15.10 (13.57–29.70) | 5.82 / 10.24 / 14.98 (12.54–16.90) | 6.27 / 10.88 / 15.74 (13.70–23.55) | 5.95 / 10.24 / 16.64 (12.80–18.43) | 4.42 / 6.27 / 12.03 (10.75–16.38) | 68% | 115 |
| all | 1000 | 200 | 8 | 6.02 / 11.14 / 18.94 (17.15–60.42) | 6.02 / 11.90 / 18.94 (16.00–40.45) | 8.19 / 20.99 / 25.86 (25.34–688.13) | 8.32 / 15.10 / 24.32 (17.66–778.24) | 8.13 / 14.72 / 25.09 (17.15–157.70) | 10.62 / 18.43 / 27.14 (26.11–204.80) | 9.73 / 17.15 / 24.32 (22.53–249.86) | 5.95 / 9.22 / 17.66 (14.21–159.74) | 49% | 317 |
| all | 1000 | 1000 | 2 | 3.62 / 4.93 / 7.74 (6.91–12.03) | 2.62 / 2.94 / 7.81 (3.10–10.88) | 2.62 / 2.78 / 5.63 (2.94–8.32) | 2.91 / 3.07 / 9.22 (3.14–20.99) | 2.91 / 3.07 / 8.13 (3.33–18.43) | 2.91 / 3.07 / 6.27 (4.10–8.45) | 2.91 / 3.07 / 4.22 (3.14–8.00) | 2.91 / 3.07 / 3.20 (3.14–7.87) | 100% | 0 |
| all | 1000 | 1000 | 3 | 4.99 / 6.34 / 10.88 (8.83–23.30) | 2.98 / 5.95 / 11.78 (8.32–24.32) | 3.74 / 4.29 / 11.65 (9.22–17.15) | 4.00 / 6.59 / 9.34 (6.85–27.14) | 4.86 / 5.12 / 11.78 (5.95–26.62) | 4.42 / 4.61 / 8.96 (7.74–13.06) | 4.42 / 4.61 / 10.24 (6.34–16.64) | 4.42 / 4.61 / 9.22 (8.58–12.16) | 100% | 0 |
| all | 1000 | 1000 | 4 | 5.06 / 8.96 / 18.94 (11.01–29.18) | 4.61 / 6.85 / 15.62 (11.39–31.23) | 4.16 / 7.49 / 16.38 (9.09–31.74) | 7.10 / 11.01 / 26.88 (13.70–32.51) | 6.78 / 8.58 / 16.64 (11.90–28.42) | 5.95 / 7.04 / 18.69 (11.26–28.93) | 5.95 / 6.14 / 12.16 (7.36–35.33) | 5.95 / 7.04 / 19.97 (9.34–28.93) | 99% | 0 |
| all | 1000 | 1000 | 6 | 6.66 / 13.18 / 21.50 (16.26–26.11) | 7.23 / 13.95 / 18.69 (16.64–24.06) | 7.87 / 22.27 / 24.06 (22.78–45.06) | 11.26 / 16.90 / 25.09 (20.48–27.90) | 11.65 / 15.62 / 20.22 (19.46–22.02) | 11.65 / 17.66 / 23.04 (21.76–27.65) | 12.03 / 15.62 / 21.25 (20.99–25.60) | 10.50 / 12.16 / 16.90 (16.13–18.94) | 67% | 90 |
| all | 1000 | 1000 | 8 | 8.83 / 17.41 / 22.53 (22.53–29.18) | 10.11 / 22.02 / 28.93 (28.67–55.81) | 13.18 / 28.93 / 36.35 (35.33–36.35) | 15.23 / 24.06 / 31.49 (29.95–73.73) | 14.85 / 22.53 / 29.44 (28.16–30.21) | 17.41 / 28.67 / 35.84 (33.79–37.89) | 17.15 / 24.58 / 35.33 (32.51–42.50) | 14.59 / 16.64 / 24.83 (23.30–63.49) | 50% | 35 |

Migrations seen: 141415 over 179389344 samples (one sample per measured operation).


### Suite cf: k-thread process beside single-thread ones, first and fair settings

load average before: { 4.97 5.34 5.73 }
load average after: { 6.89 5.91 5.90 }; 115 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.21 / 0.25 (0.25–0.37) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.29 / 0.46 / 0.54 (0.46–0.74) | 0.21 / 0.21 / 0.33 (0.25–0.42) | 0.29 / 0.50 / 0.74 (0.50–8.58) | 0.21 / 0.21 / 0.25 (0.25–0.42) | 0.21 / 0.21 / 0.29 (0.25–0.46) | 0.21 / 0.25 / 0.29 (0.25–0.46) | 97% | 0 |
| 4 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.46) | 0.21 / 0.21 / 0.25 (0.25–0.50) | 0.29 / 0.50 / 0.62 (0.46–5.89) | 0.21 / 0.25 / 0.29 (0.25–0.50) | 0.29 / 0.58 / 1.62 (0.70–9.60) | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.46) | 98% | 5 |
| 4 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.46) | 0.21 / 0.21 / 0.25 (0.25–0.42) | 0.29 / 0.58 / 0.74 (0.70–5.82) | 0.21 / 0.25 / 0.25 (0.25–0.46) | 0.29 / 0.58 / 1.79 (0.62–8.70) | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.21 / 0.25 / 0.33 (0.25–0.50) | 0.21 / 0.25 / 0.37 (0.25–0.46) | 98% | 0 |
| 4 | 1000 | 1 | 1.00 / 1.04 / 1.36 (1.04–8.00) | 1.00 / 1.04 / 5.31 (1.04–8.32) | 1.07 / 1.25 / 8.45 (1.33–9.47) | 1.00 / 1.04 / 1.20 (1.04–8.00) | 2.94 / 3.10 / 14.98 (6.59–23.04) | 1.00 / 1.04 / 1.07 (1.04–7.87) | 1.00 / 1.04 / 1.07 (1.04–7.42) | 1.00 / 1.04 / 1.07 (1.04–8.19) | 99% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.00 / 1.12 (1.04–6.40) | 1.00 / 1.04 / 1.12 (1.04–1.82) | 1.07 / 1.25 / 1.33 (1.28–3.81) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 2.94 / 3.14 / 7.81 (3.23–9.09) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–4.93) | 1.00 / 1.04 / 1.04 (1.04–1.20) | 99% | 0 |
| 4 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.04–1.36) | 1.00 / 1.00 / 1.07 (1.04–5.76) | 1.07 / 2.11 / 2.27 (2.24–9.73) | 1.00 / 1.04 / 1.07 (1.04–7.42) | 2.98 / 3.20 / 9.22 (3.26–22.78) | 1.00 / 1.04 / 1.07 (1.04–6.08) | 1.00 / 1.04 / 1.12 (1.04–1.78) | 1.00 / 1.04 / 1.07 (1.04–7.62) | 99% | 0 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 0.70 (0.70–2.56) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.37 / 0.62 / 0.74 (0.74–8.96) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 49% | 2 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.70 (0.66–4.42) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.42 / 0.70 / 0.91 (0.87–4.42) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 50% | 9 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.62 / 0.87 (0.74–4.42) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.37 / 0.66 / 0.87 (0.79–6.14) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 49% | 21 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 1.04 (1.00–1.15) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.46 / 1.33 / 1.87 (1.57–8.45) | 0.21 / 0.29 / 0.33 (0.33–0.37) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 49% | 15 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.70 (0.50–3.62) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.42 / 1.15 / 2.11 (1.57–6.53) | 0.21 / 0.33 / 0.33 (0.33–0.37) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 12 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.20 / 1.49 / 1.66 (1.62–10.37) | 1.00 / 1.07 / 1.07 (1.07–6.02) | 3.20 / 3.49 / 9.86 (6.78–29.44) | 1.00 / 1.07 / 1.07 (1.07–6.21) | 1.00 / 1.07 / 1.07 (1.07–6.66) | 1.00 / 1.07 / 1.07 (1.07–6.66) | 49% | 2 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.07 (1.07–6.91) | 1.00 / 1.07 / 1.07 (1.07–5.95) | 1.20 / 1.49 / 1.62 (1.62–9.09) | 1.00 / 1.07 / 1.07 (1.07–1.82) | 3.23 / 3.62 / 10.88 (10.11–15.87) | 1.00 / 1.07 / 1.07 (1.07–5.57) | 1.00 / 1.07 / 1.07 (1.07–1.82) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 49% | 19 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.12 (1.07–6.40) | 1.00 / 1.07 / 1.07 (1.07–4.80) | 1.15 / 1.95 / 2.50 (2.30–7.62) | 1.00 / 1.07 / 1.12 (1.07–6.14) | 3.23 / 3.55 / 10.50 (8.96–10.88) | 1.00 / 1.07 / 1.12 (1.12–5.76) | 1.00 / 1.07 / 1.07 (1.07–4.74) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 14 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.07 (1.07–5.50) | 1.00 / 1.07 / 1.07 (1.07–6.08) | 1.12 / 2.11 / 2.43 (2.30–9.60) | 1.00 / 1.07 / 2.03 (1.07–6.66) | 3.23 / 4.48 / 8.96 (6.53–31.49) | 1.00 / 1.07 / 1.15 (1.12–6.78) | 1.00 / 1.07 / 1.07 (1.07–6.40) | 1.00 / 1.07 / 1.07 (1.07–7.04) | 49% | 32 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–6.14) | 1.00 / 1.07 / 1.07 (1.07–6.91) | 1.15 / 1.49 / 1.62 (1.36–8.70) | 1.00 / 1.07 / 1.07 (1.07–3.87) | 3.14 / 4.42 / 10.75 (8.45–12.16) | 1.00 / 1.12 / 1.12 (1.12–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–12.67) | 49% | 95 |

Independent processes: operations/s

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 1.92M (1.88M–1.92M) | 1.91M (1.91M–1.93M) | 1.83M (1.81M–1.83M) | 1.91M (1.88M–1.92M) | 1.80M (1.75M–1.82M) | 1.91M (1.89M–1.93M) | 1.91M (1.88M–1.92M) | 1.91M (1.88M–1.93M) | 97% | 0 |
| 4 | 200 | 2 | 1.27M (1.25M–1.29M) | 1.27M (1.25M–1.29M) | 1.21M (1.19M–1.23M) | 1.27M (1.25M–1.28M) | 1.19M (1.16M–1.21M) | 1.27M (1.14M–1.29M) | 1.27M (1.26M–1.29M) | 1.27M (1.25M–1.29M) | 98% | 5 |
| 4 | 200 | 3 | 634k (630k–647k) | 638k (631k–648k) | 523k (512k–531k) | 549k (535k–553k) | 592k (581k–605k) | 636k (627k–647k) | 634k (624k–646k) | 631k (626k–638k) | 98% | 0 |
| 4 | 1000 | 1 | 1.26M (1.25M–1.28M) | 1.26M (1.25M–1.28M) | 1.22M (1.21M–1.24M) | 1.26M (1.25M–1.28M) | 692k (663k–698k) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 99% | 0 |
| 4 | 1000 | 2 | 848k (837k–853k) | 845k (840k–853k) | 824k (812k–826k) | 849k (844k–851k) | 462k (461k–462k) | 852k (848k–853k) | 850k (844k–851k) | 850k (846k–853k) | 99% | 0 |
| 4 | 1000 | 3 | 424k (421k–425k) | 424k (420k–426k) | 353k (344k–357k) | 383k (375k–386k) | 229k (220k–230k) | 425k (420k–427k) | 424k (420k–427k) | 426k (416k–426k) | 99% | 0 |
| 8 | 200 | 1 | 4.50M (4.42M–4.51M) | 4.49M (4.39M–4.53M) | 4.16M (4.14M–4.20M) | 4.49M (4.42M–4.51M) | 4.07M (3.94M–4.08M) | 4.50M (4.40M–4.51M) | 4.48M (4.38M–4.51M) | 4.49M (4.39M–4.53M) | 49% | 2 |
| 8 | 200 | 2 | 3.84M (3.77M–3.87M) | 3.85M (3.78M–3.88M) | 3.55M (3.45M–3.64M) | 3.86M (3.80M–3.87M) | 3.44M (3.39M–3.48M) | 3.85M (3.84M–3.86M) | 3.84M (3.82M–3.86M) | 3.85M (3.84M–3.87M) | 50% | 9 |
| 8 | 200 | 3 | 3.20M (3.20M–3.22M) | 3.21M (3.13M–3.23M) | 2.89M (2.86M–2.90M) | 3.11M (3.05M–3.12M) | 2.86M (2.51M–2.91M) | 3.21M (3.19M–3.21M) | 3.19M (3.12M–3.21M) | 3.20M (3.15M–3.21M) | 49% | 21 |
| 8 | 200 | 5 | 1.92M (1.90M–1.93M) | 1.93M (1.92M–1.93M) | 1.62M (1.58M–1.70M) | 1.78M (1.77M–1.79M) | 1.28M (1.27M–1.33M) | 1.45M (1.45M–1.49M) | 1.92M (1.88M–1.93M) | 1.92M (1.90M–1.93M) | 49% | 15 |
| 8 | 200 | 6 | 1.28M (1.25M–1.29M) | 1.28M (1.26M–1.29M) | 1.19M (1.13M–1.25M) | 1.28M (1.26M–1.29M) | 860k (834k–878k) | 979k (964k–982k) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 49% | 12 |
| 8 | 1000 | 1 | 3.02M (3.01M–3.02M) | 3.02M (3.00M–3.03M) | 2.79M (2.75M–2.82M) | 3.02M (2.97M–3.03M) | 1.55M (1.51M–1.56M) | 3.03M (2.97M–3.03M) | 3.02M (2.96M–3.03M) | 3.02M (2.96M–3.03M) | 49% | 2 |
| 8 | 1000 | 2 | 2.59M (2.54M–2.60M) | 2.59M (2.55M–2.60M) | 2.40M (2.34M–2.44M) | 2.59M (2.44M–2.60M) | 1.32M (1.30M–1.32M) | 2.60M (2.57M–2.60M) | 2.59M (2.58M–2.60M) | 2.59M (2.58M–2.60M) | 49% | 19 |
| 8 | 1000 | 3 | 2.16M (2.13M–2.16M) | 2.16M (2.14M–2.17M) | 1.94M (1.89M–2.00M) | 2.11M (2.08M–2.11M) | 1.10M (1.09M–1.11M) | 2.16M (2.14M–2.16M) | 2.16M (2.15M–2.16M) | 2.16M (2.16M–2.16M) | 49% | 14 |
| 8 | 1000 | 5 | 1.30M (1.28M–1.30M) | 1.30M (1.27M–1.30M) | 1.12M (1.11M–1.16M) | 1.21M (1.20M–1.23M) | 598k (582k–604k) | 1.08M (1.07M–1.08M) | 1.30M (1.27M–1.30M) | 1.30M (1.27M–1.30M) | 49% | 32 |
| 8 | 1000 | 6 | 864k (851k–867k) | 864k (847k–868k) | 799k (773k–847k) | 866k (859k–867k) | 401k (360k–413k) | 725k (720k–726k) | 865k (863k–866k) | 865k (616k–867k) | 49% | 95 |

The k-thread process: operations/s

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 646k (612k–649k) | 640k (639k–648k) | 607k (598k–612k) | 636k (626k–640k) | 599k (583k–608k) | 637k (628k–639k) | 636k (626k–649k) | 636k (624k–647k) | 97% | 0 |
| 4 | 200 | 2 | 1.07M (1.04M–1.07M) | 1.10M (1.08M–1.11M) | 1.21M (1.19M–1.22M) | 1.25M (1.23M–1.26M) | 1.19M (1.16M–1.21M) | 1.25M (1.17M–1.26M) | 1.20M (1.20M–1.23M) | 1.26M (1.23M–1.26M) | 98% | 5 |
| 4 | 200 | 3 | 1.45M (1.42M–1.45M) | 1.51M (1.49M–1.52M) | 1.49M (1.45M–1.50M) | 1.60M (1.57M–1.62M) | 1.78M (1.75M–1.82M) | 1.88M (1.84M–1.91M) | 1.80M (1.76M–1.85M) | 1.86M (1.83M–1.91M) | 98% | 0 |
| 4 | 1000 | 1 | 420k (415k–426k) | 417k (415k–426k) | 406k (401k–412k) | 420k (414k–426k) | 231k (221k–233k) | 423k (415k–426k) | 423k (415k–427k) | 424k (415k–426k) | 99% | 0 |
| 4 | 1000 | 2 | 706k (697k–710k) | 746k (738k–749k) | 819k (804k–827k) | 836k (833k–839k) | 462k (461k–462k) | 838k (826k–839k) | 821k (813k–824k) | 837k (833k–839k) | 99% | 0 |
| 4 | 1000 | 3 | 961k (953k–967k) | 1.04M (1.02M–1.04M) | 984k (958k–988k) | 1.09M (1.06M–1.09M) | 688k (660k–691k) | 1.21M (1.19M–1.22M) | 914k (905k–918k) | 1.21M (1.18M–1.22M) | 99% | 0 |
| 8 | 200 | 1 | 641k (629k–648k) | 639k (627k–646k) | 591k (580k–616k) | 636k (609k–641k) | 580k (565k–583k) | 641k (628k–642k) | 641k (624k–644k) | 641k (622k–645k) | 49% | 2 |
| 8 | 200 | 2 | 1.05M (1.03M–1.06M) | 1.08M (1.07M–1.09M) | 1.18M (1.14M–1.20M) | 1.25M (1.24M–1.28M) | 1.14M (1.13M–1.15M) | 1.25M (1.25M–1.26M) | 1.18M (1.14M–1.21M) | 1.26M (1.25M–1.27M) | 50% | 9 |
| 8 | 200 | 3 | 1.42M (1.40M–1.44M) | 1.49M (1.46M–1.52M) | 1.40M (1.38M–1.42M) | 1.56M (1.56M–1.59M) | 1.70M (1.53M–1.73M) | 1.86M (1.85M–1.88M) | 1.75M (1.68M–1.84M) | 1.86M (1.84M–1.88M) | 49% | 21 |
| 8 | 200 | 5 | 2.11M (2.08M–2.15M) | 2.19M (2.17M–2.24M) | 1.98M (1.97M–2.10M) | 2.26M (2.22M–2.29M) | 2.07M (2.05M–2.19M) | 2.53M (2.49M–2.57M) | 2.67M (2.49M–2.72M) | 3.04M (3.01M–3.07M) | 49% | 15 |
| 8 | 200 | 6 | 2.38M (2.33M–2.38M) | 2.54M (2.50M–2.57M) | 2.39M (2.32M–2.40M) | 2.61M (2.53M–2.64M) | 2.45M (2.35M–2.51M) | 2.87M (2.84M–2.89M) | 2.65M (2.52M–2.74M) | 3.60M (3.59M–3.63M) | 49% | 12 |
| 8 | 1000 | 1 | 431k (431k–433k) | 432k (431k–434k) | 397k (394k–409k) | 432k (425k–433k) | 220k (215k–221k) | 433k (429k–434k) | 433k (425k–434k) | 432k (424k–434k) | 49% | 2 |
| 8 | 1000 | 2 | 712k (691k–719k) | 738k (734k–759k) | 801k (771k–840k) | 826k (792k–846k) | 433k (430k–437k) | 839k (830k–843k) | 795k (770k–842k) | 841k (834k–852k) | 49% | 19 |
| 8 | 1000 | 3 | 958k (939k–968k) | 1.03M (1.02M–1.05M) | 946k (909k–966k) | 1.08M (1.05M–1.08M) | 643k (626k–650k) | 1.20M (1.19M–1.21M) | 856k (821k–870k) | 1.20M (1.19M–1.21M) | 49% | 14 |
| 8 | 1000 | 5 | 1.39M (1.35M–1.40M) | 1.53M (1.50M–1.57M) | 1.39M (1.35M–1.42M) | 1.58M (1.49M–1.60M) | 943k (916k–960k) | 1.64M (1.64M–1.67M) | 822k (816k–850k) | 1.82M (1.80M–1.84M) | 49% | 32 |
| 8 | 1000 | 6 | 1.57M (1.53M–1.58M) | 1.75M (1.72M–1.77M) | 1.59M (1.53M–1.67M) | 1.83M (1.80M–1.88M) | 1.08M (944k–1.11M) | 1.91M (1.87M–1.92M) | 832k (811k–843k) | 2.12M (1.56M–2.12M) | 49% | 95 |

Seqlock reads repeated per 1000 lookups: 2 fixed/2 fair: 8.60; 2 fixed/4 fair: 21.91; 3 sized fair: 32.69;

Migrations seen: 212375 over 298041632 samples (one sample per measured operation).


## Appendix D: full tables, second series, pass 2


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 13.2 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a1f: capability operations, first and fair settings

load average before: { 6.89 5.91 5.90 }
load average after: { 6.01 5.33 5.46 }; 605 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 1.40M (1.40M–1.41M) | 1.40M (1.40M–1.40M) | 1.40M (1.39M–1.41M) | 1.40M (1.39M–1.41M) | 1.40M (1.40M–1.41M) | 1.36M (1.35M–1.37M) | 1.41M (1.40M–1.41M) | 1.36M (1.35M–1.37M) | 1.41M (1.40M–1.41M) | 1.36M (1.35M–1.37M) | 1.41M (1.40M–1.41M) | 100% | 0 |
| 1 | 0 | 50 | 3 | 2.08M (2.07M–2.08M) | 2.09M (2.08M–2.09M) | 2.09M (2.08M–2.09M) | 2.09M (2.08M–2.10M) | 2.09M (2.08M–2.10M) | 2.04M (2.03M–2.05M) | 2.09M (2.08M–2.10M) | 2.03M (2.00M–2.03M) | 2.11M (2.09M–2.11M) | 2.02M (1.99M–2.03M) | 2.10M (2.09M–2.10M) | 100% | 0 |
| 1 | 0 | 50 | 4 | 2.77M (2.76M–2.79M) | 2.78M (2.76M–2.79M) | 2.78M (2.77M–2.79M) | 2.79M (2.77M–2.79M) | 2.78M (2.77M–2.80M) | 2.69M (2.66M–2.70M) | 2.78M (2.77M–2.80M) | 2.69M (2.64M–2.70M) | 2.81M (2.79M–2.82M) | 2.70M (2.68M–2.70M) | 2.81M (2.79M–2.82M) | 100% | 0 |
| 1 | 0 | 50 | 6 | 4.13M (4.13M–4.14M) | 4.15M (4.09M–4.17M) | 4.15M (4.12M–4.17M) | 4.12M (4.10M–4.12M) | 4.12M (4.10M–4.14M) | 3.75M (3.71M–3.83M) | 4.10M (4.08M–4.12M) | 3.67M (3.66M–3.84M) | 4.14M (4.12M–4.15M) | 3.62M (3.62M–3.74M) | 4.17M (4.16M–4.18M) | 67% | 10 |
| 1 | 0 | 50 | 8 | 5.35M (5.10M–5.36M) | 5.37M (4.96M–5.39M) | 5.36M (5.35M–5.36M) | 5.40M (5.38M–5.41M) | 5.40M (5.18M–5.42M) | 4.97M (4.74M–5.10M) | 5.38M (5.36M–5.38M) | 4.71M (4.68M–4.97M) | 5.44M (5.43M–5.44M) | 4.07M (3.80M–4.35M) | 5.56M (5.55M–5.57M) | 50% | 111 |
| 1 | 0 | 200 | 2 | 1.28M (1.28M–1.29M) | 1.28M (1.28M–1.29M) | 1.29M (1.28M–1.29M) | 1.29M (1.28M–1.29M) | 1.29M (1.28M–1.29M) | 1.24M (1.23M–1.25M) | 1.29M (1.29M–1.30M) | 1.25M (1.24M–1.25M) | 1.30M (1.29M–1.30M) | 1.24M (1.23M–1.25M) | 1.29M (1.29M–1.30M) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.91M (1.91M–1.91M) | 1.91M (1.90M–1.91M) | 1.91M (1.90M–1.92M) | 1.92M (1.90M–1.92M) | 1.92M (1.91M–1.92M) | 1.87M (1.85M–1.87M) | 1.93M (1.92M–1.93M) | 1.85M (1.85M–1.86M) | 1.93M (1.92M–1.94M) | 1.85M (1.84M–1.86M) | 1.94M (1.92M–1.94M) | 100% | 0 |
| 1 | 0 | 200 | 4 | 2.54M (2.53M–2.55M) | 2.55M (2.54M–2.56M) | 2.55M (2.53M–2.56M) | 2.56M (2.56M–2.57M) | 2.57M (2.55M–2.57M) | 2.46M (2.43M–2.47M) | 2.56M (2.55M–2.57M) | 2.46M (2.41M–2.47M) | 2.59M (2.57M–2.60M) | 2.47M (2.46M–2.47M) | 2.58M (2.56M–2.59M) | 100% | 0 |
| 1 | 0 | 200 | 6 | 3.81M (3.77M–3.81M) | 3.83M (3.82M–3.84M) | 3.80M (3.76M–3.82M) | 3.84M (3.83M–3.86M) | 3.84M (3.83M–3.84M) | 3.51M (3.44M–3.52M) | 3.83M (3.82M–3.83M) | 3.40M (3.37M–3.46M) | 3.86M (3.84M–3.86M) | 2.59M (2.48M–2.60M) | 3.90M (3.89M–3.92M) | 66% | 29 |
| 1 | 0 | 200 | 8 | 4.93M (4.93M–4.94M) | 4.95M (4.94M–4.96M) | 4.93M (4.93M–4.97M) | 4.99M (4.98M–5.00M) | 4.97M (4.97M–4.99M) | 4.62M (4.60M–4.65M) | 4.97M (4.96M–4.99M) | 4.40M (4.36M–4.58M) | 5.03M (5.02M–5.05M) | 2.58M (2.53M–2.68M) | 5.11M (5.11M–5.13M) | 50% | 0 |
| 1 | 0 | 1000 | 2 | 844k (841k–844k) | 845k (843k–846k) | 846k (846k–848k) | 848k (847k–849k) | 848k (846k–850k) | 830k (822k–832k) | 851k (848k–851k) | 829k (824k–831k) | 850k (847k–852k) | 830k (819k–834k) | 850k (848k–851k) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.25M (1.25M–1.26M) | 1.26M (1.26M–1.26M) | 1.26M (1.26M–1.26M) | 1.26M (1.26M–1.27M) | 1.26M (1.26M–1.27M) | 1.23M (1.23M–1.23M) | 1.27M (1.26M–1.27M) | 923k (921k–927k) | 1.27M (1.27M–1.27M) | 924k (923k–927k) | 1.27M (1.26M–1.27M) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.66M (1.66M–1.67M) | 1.67M (1.66M–1.67M) | 1.68M (1.67M–1.68M) | 1.68M (1.68M–1.69M) | 1.68M (1.68M–1.69M) | 1.62M (1.61M–1.62M) | 1.68M (1.68M–1.68M) | 921k (918k–922k) | 1.68M (1.68M–1.69M) | 922k (921k–922k) | 1.68M (1.68M–1.69M) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 2.48M (2.47M–2.48M) | 2.49M (2.49M–2.49M) | 2.52M (2.50M–2.52M) | 2.53M (2.52M–2.53M) | 2.53M (2.53M–2.53M) | 2.30M (2.28M–2.36M) | 2.51M (2.51M–2.52M) | 1.59M (1.58M–1.64M) | 2.51M (2.51M–2.52M) | 841k (838k–855k) | 2.53M (2.52M–2.53M) | 66% | 15 |
| 1 | 0 | 1000 | 8 | 3.28M (3.27M–3.28M) | 3.30M (3.29M–3.30M) | 3.35M (3.35M–3.35M) | 3.36M (3.36M–3.37M) | 3.36M (3.36M–3.37M) | 3.06M (3.04M–3.10M) | 3.34M (3.34M–3.35M) | 1.63M (1.62M–1.67M) | 3.35M (3.34M–3.36M) | 826k (818k–853k) | 3.35M (3.34M–3.36M) | 49% | 0 |
| 1 | 1000 | 50 | 2 | 1.38M (1.37M–1.38M) | 1.38M (1.38M–1.39M) | 1.38M (1.38M–1.39M) | 1.39M (1.38M–1.39M) | 1.39M (1.38M–1.39M) | 1.36M (1.35M–1.37M) | 1.41M (1.40M–1.41M) | 1.37M (1.35M–1.37M) | 1.41M (1.40M–1.41M) | 1.36M (1.36M–1.37M) | 1.41M (1.40M–1.41M) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.03M (2.03M–2.04M) | 2.04M (2.04M–2.05M) | 2.03M (2.03M–2.04M) | 2.04M (2.04M–2.05M) | 2.05M (2.03M–2.05M) | 2.00M (1.99M–2.02M) | 2.06M (2.05M–2.07M) | 2.03M (2.01M–2.04M) | 2.10M (2.09M–2.11M) | 2.02M (2.02M–2.04M) | 2.10M (2.09M–2.10M) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.69M (2.68M–2.70M) | 2.70M (2.70M–2.71M) | 2.68M (2.67M–2.70M) | 2.70M (2.69M–2.71M) | 2.71M (2.71M–2.72M) | 2.63M (2.62M–2.63M) | 2.73M (2.71M–2.73M) | 2.69M (2.67M–2.70M) | 2.81M (2.77M–2.82M) | 2.69M (2.68M–2.70M) | 2.81M (2.78M–2.81M) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.96M (3.94M–3.97M) | 3.99M (3.98M–4.00M) | 3.96M (3.95M–3.98M) | 3.96M (3.93M–3.98M) | 3.96M (3.95M–3.97M) | 3.66M (3.58M–3.76M) | 3.97M (3.96M–3.98M) | 3.60M (3.57M–3.73M) | 4.05M (4.03M–4.06M) | 3.62M (3.61M–3.71M) | 4.17M (4.17M–4.19M) | 66% | 5 |
| 1 | 1000 | 50 | 8 | 5.08M (5.07M–5.09M) | 5.12M (5.10M–5.13M) | 5.08M (5.06M–5.11M) | 5.14M (5.11M–5.15M) | 5.12M (5.10M–5.12M) | 4.65M (4.57M–4.78M) | 5.12M (5.09M–5.14M) | 4.53M (4.52M–4.61M) | 5.24M (5.23M–5.25M) | 3.85M (3.81M–4.19M) | 5.55M (5.53M–5.56M) | 50% | 0 |
| 1 | 1000 | 200 | 2 | 1.27M (1.26M–1.27M) | 1.27M (1.27M–1.27M) | 1.27M (1.26M–1.27M) | 1.27M (1.27M–1.27M) | 1.27M (1.26M–1.27M) | 1.24M (1.23M–1.25M) | 1.29M (1.28M–1.30M) | 1.24M (1.22M–1.25M) | 1.29M (1.28M–1.30M) | 1.24M (1.23M–1.25M) | 1.29M (1.28M–1.30M) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 1.87M (1.86M–1.87M) | 1.88M (1.87M–1.88M) | 1.87M (1.87M–1.88M) | 1.88M (1.88M–1.89M) | 1.89M (1.88M–1.89M) | 1.83M (1.83M–1.84M) | 1.89M (1.89M–1.90M) | 1.85M (1.84M–1.86M) | 1.93M (1.92M–1.93M) | 1.85M (1.85M–1.86M) | 1.92M (1.92M–1.94M) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 2.46M (2.44M–2.48M) | 2.49M (2.48M–2.49M) | 2.48M (2.48M–2.49M) | 2.49M (2.47M–2.50M) | 2.49M (2.48M–2.50M) | 2.41M (2.40M–2.42M) | 2.52M (2.51M–2.52M) | 2.46M (2.41M–2.47M) | 2.57M (2.55M–2.58M) | 2.45M (2.45M–2.47M) | 2.57M (2.56M–2.58M) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 3.66M (3.62M–3.67M) | 3.68M (3.63M–3.69M) | 3.66M (3.64M–3.67M) | 3.68M (3.64M–3.70M) | 3.68M (3.63M–3.70M) | 3.36M (3.31M–3.41M) | 3.70M (3.68M–3.72M) | 3.30M (3.29M–3.40M) | 3.76M (3.75M–3.77M) | 2.60M (2.54M–2.63M) | 3.90M (3.90M–3.92M) | 66% | 7 |
| 1 | 1000 | 200 | 8 | 4.70M (4.69M–4.71M) | 4.74M (4.72M–4.75M) | 4.72M (4.71M–4.74M) | 4.77M (4.75M–4.79M) | 4.75M (4.75M–4.79M) | 4.40M (4.31M–4.54M) | 4.77M (4.74M–4.78M) | 4.25M (4.24M–4.27M) | 4.86M (4.85M–4.87M) | 2.55M (2.54M–2.60M) | 5.12M (5.10M–5.13M) | 50% | 0 |
| 1 | 1000 | 1000 | 2 | 836k (834k–838k) | 839k (836k–840k) | 841k (839k–841k) | 841k (840k–844k) | 842k (823k–844k) | 830k (825k–831k) | 850k (847k–852k) | 828k (827k–829k) | 851k (849k–853k) | 831k (830k–832k) | 852k (849k–853k) | 100% | 2 |
| 1 | 1000 | 1000 | 3 | 1.24M (1.23M–1.24M) | 1.24M (1.24M–1.24M) | 1.25M (1.24M–1.25M) | 1.25M (1.24M–1.25M) | 1.25M (1.25M–1.25M) | 1.22M (1.22M–1.22M) | 1.25M (1.25M–1.26M) | 924k (923k–925k) | 1.27M (1.25M–1.27M) | 923k (923k–926k) | 1.27M (1.26M–1.27M) | 100% | 0 |
| 1 | 1000 | 1000 | 4 | 1.63M (1.62M–1.64M) | 1.64M (1.63M–1.64M) | 1.65M (1.64M–1.65M) | 1.65M (1.64M–1.65M) | 1.65M (1.65M–1.66M) | 1.60M (1.59M–1.60M) | 1.66M (1.66M–1.67M) | 922k (916k–922k) | 1.68M (1.67M–1.69M) | 922k (913k–922k) | 1.68M (1.68M–1.69M) | 100% | 0 |
| 1 | 1000 | 1000 | 6 | 2.41M (2.39M–2.42M) | 2.42M (2.42M–2.43M) | 2.45M (2.45M–2.45M) | 2.46M (2.46M–2.47M) | 2.46M (2.46M–2.46M) | 2.24M (2.24M–2.36M) | 2.47M (2.46M–2.47M) | 1.59M (1.58M–1.62M) | 2.49M (2.48M–2.49M) | 836k (832k–843k) | 2.53M (2.52M–2.53M) | 66% | 11 |
| 1 | 1000 | 1000 | 8 | 3.16M (3.06M–3.17M) | 3.18M (3.14M–3.19M) | 3.23M (3.23M–3.24M) | 3.25M (3.25M–3.25M) | 3.25M (3.24M–3.25M) | 2.91M (2.89M–3.01M) | 3.27M (3.20M–3.27M) | 1.64M (1.63M–1.77M) | 3.30M (3.29M–3.30M) | 836k (827k–849k) | 3.35M (3.35M–3.35M) | 49% | 67 |
| 10 | 0 | 50 | 2 | 1.32M (1.32M–1.32M) | 1.34M (1.34M–1.34M) | 1.33M (1.33M–1.33M) | 1.34M (1.34M–1.34M) | 1.34M (1.34M–1.35M) | 1.36M (1.35M–1.36M) | 1.39M (1.39M–1.39M) | 1.35M (1.34M–1.36M) | 1.39M (1.38M–1.40M) | 1.35M (1.34M–1.36M) | 1.39M (1.38M–1.40M) | 100% | 0 |
| 10 | 0 | 50 | 3 | 1.92M (1.92M–1.93M) | 1.95M (1.94M–1.95M) | 1.94M (1.93M–1.94M) | 1.96M (1.95M–1.96M) | 1.96M (1.95M–1.96M) | 1.91M (1.90M–1.91M) | 1.98M (1.97M–1.98M) | 2.01M (2.00M–2.02M) | 2.07M (2.06M–2.08M) | 2.01M (2.00M–2.01M) | 2.07M (2.06M–2.08M) | 100% | 0 |
| 10 | 0 | 50 | 4 | 2.51M (2.50M–2.51M) | 2.54M (2.53M–2.54M) | 2.53M (2.53M–2.53M) | 2.57M (2.56M–2.58M) | 2.57M (2.56M–2.58M) | 2.48M (2.46M–2.48M) | 2.59M (2.58M–2.60M) | 2.67M (2.60M–2.68M) | 2.76M (2.73M–2.77M) | 2.67M (2.61M–2.68M) | 2.76M (2.74M–2.77M) | 100% | 0 |
| 10 | 0 | 50 | 6 | 3.40M (3.33M–3.40M) | 3.49M (3.48M–3.50M) | 3.43M (3.42M–3.45M) | 3.53M (3.52M–3.54M) | 3.53M (3.46M–3.55M) | 3.18M (3.08M–3.39M) | 3.37M (3.30M–3.49M) | 3.04M (3.03M–3.28M) | 3.61M (3.55M–3.69M) | 3.54M (3.53M–3.72M) | 4.06M (4.00M–4.08M) | 67% | 6 |
| 10 | 0 | 50 | 8 | 4.14M (4.03M–4.17M) | 4.34M (4.32M–4.36M) | 4.22M (4.20M–4.24M) | 4.41M (4.41M–4.43M) | 4.42M (4.41M–4.43M) | 4.01M (3.86M–4.02M) | 4.33M (4.08M–4.40M) | 4.08M (3.82M–4.21M) | 4.67M (4.63M–4.69M) | 4.20M (3.84M–4.34M) | 5.33M (5.27M–5.36M) | 50% | 16 |
| 10 | 0 | 200 | 2 | 1.20M (1.20M–1.21M) | 1.21M (1.21M–1.22M) | 1.22M (952k–1.22M) | 1.23M (1.20M–1.23M) | 1.23M (1.23M–1.23M) | 1.24M (1.23M–1.24M) | 1.28M (1.28M–1.28M) | 1.24M (1.22M–1.24M) | 1.28M (1.27M–1.28M) | 1.23M (1.22M–1.24M) | 1.27M (1.26M–1.28M) | 99% | 15 |
| 10 | 0 | 200 | 3 | 1.73M (1.71M–1.74M) | 1.75M (1.74M–1.76M) | 1.77M (1.74M–1.77M) | 1.78M (1.76M–1.79M) | 1.78M (1.78M–1.79M) | 1.73M (1.73M–1.73M) | 1.81M (1.81M–1.81M) | 1.84M (1.82M–1.85M) | 1.90M (1.90M–1.91M) | 1.84M (1.80M–1.84M) | 1.90M (1.88M–1.91M) | 100% | 0 |
| 10 | 0 | 200 | 4 | 2.25M (2.24M–2.26M) | 2.28M (2.27M–2.29M) | 2.30M (2.30M–2.31M) | 2.33M (2.32M–2.34M) | 2.34M (2.32M–2.34M) | 2.23M (2.22M–2.23M) | 2.36M (2.35M–2.37M) | 2.43M (2.38M–2.45M) | 2.52M (2.50M–2.53M) | 2.43M (2.38M–2.44M) | 2.52M (2.51M–2.53M) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.95M (2.93M–3.03M) | 3.13M (3.05M–3.15M) | 3.16M (3.08M–3.16M) | 3.24M (3.22M–3.24M) | 3.22M (3.21M–3.24M) | 2.86M (2.80M–3.08M) | 3.12M (3.07M–3.28M) | 2.93M (2.76M–2.97M) | 3.38M (3.27M–3.39M) | 2.67M (2.47M–2.69M) | 3.66M (3.65M–3.67M) | 66% | 29 |
| 10 | 0 | 200 | 8 | 3.76M (3.76M–3.78M) | 3.91M (3.89M–3.92M) | 3.90M (3.87M–3.91M) | 4.05M (4.03M–4.07M) | 4.05M (3.99M–4.07M) | 3.57M (3.54M–3.66M) | 3.99M (3.95M–4.04M) | 3.40M (3.28M–3.56M) | 4.29M (4.25M–4.30M) | 2.68M (2.65M–2.75M) | 4.71M (4.70M–4.72M) | 50% | 22 |
| 10 | 0 | 1000 | 2 | 762k (761k–763k) | 772k (770k–773k) | 793k (792k–795k) | 800k (797k–801k) | 800k (800k–801k) | 827k (825k–828k) | 838k (838k–841k) | 827k (826k–828k) | 839k (837k–840k) | 827k (824k–829k) | 838k (836k–840k) | 100% | 0 |
| 10 | 0 | 1000 | 3 | 1.08M (1.08M–1.08M) | 1.09M (1.09M–1.10M) | 1.15M (1.14M–1.15M) | 1.16M (1.16M–1.16M) | 1.16M (1.15M–1.16M) | 1.08M (1.08M–1.08M) | 1.16M (1.16M–1.17M) | 918k (917k–921k) | 1.21M (1.21M–1.22M) | 919k (916k–920k) | 1.21M (1.21M–1.21M) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 1.37M (1.37M–1.37M) | 1.39M (1.39M–1.39M) | 1.48M (1.48M–1.49M) | 1.51M (1.50M–1.51M) | 1.51M (1.50M–1.51M) | 1.36M (1.36M–1.37M) | 1.49M (1.49M–1.49M) | 915k (909k–916k) | 1.55M (1.55M–1.56M) | 916k (912k–918k) | 1.55M (1.55M–1.56M) | 100% | 0 |
| 10 | 0 | 1000 | 6 | 1.81M (1.80M–1.82M) | 1.85M (1.84M–1.88M) | 2.02M (2.02M–2.06M) | 2.07M (2.06M–2.10M) | 2.08M (2.07M–2.08M) | 1.66M (1.65M–1.68M) | 2.00M (1.99M–2.02M) | 1.30M (1.29M–1.34M) | 2.03M (2.00M–2.07M) | 836k (829k–848k) | 2.11M (2.11M–2.11M) | 67% | 28 |
| 10 | 0 | 1000 | 8 | 2.25M (2.25M–2.26M) | 2.26M (2.26M–2.29M) | 2.60M (2.51M–2.61M) | 2.62M (2.60M–2.63M) | 2.65M (2.59M–2.68M) | 2.04M (1.91M–2.11M) | 2.51M (2.47M–2.58M) | 1.39M (1.35M–1.43M) | 2.57M (2.52M–2.59M) | 842k (826k–855k) | 2.59M (2.58M–2.59M) | 49% | 14 |
| 10 | 1000 | 50 | 2 | 1.17M (1.17M–1.17M) | 1.20M (1.19M–1.20M) | 1.18M (1.17M–1.18M) | 1.20M (1.20M–1.20M) | 1.21M (1.20M–1.21M) | 1.36M (1.35M–1.36M) | 1.39M (1.39M–1.39M) | 1.35M (1.34M–1.36M) | 1.39M (1.38M–1.39M) | 1.35M (1.34M–1.36M) | 1.39M (1.39M–1.40M) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 1.59M (1.59M–1.59M) | 1.64M (1.63M–1.64M) | 1.60M (1.60M–1.60M) | 1.65M (1.65M–1.65M) | 1.64M (1.62M–1.64M) | 1.65M (1.65M–1.66M) | 1.75M (1.75M–1.75M) | 2.01M (2.00M–2.02M) | 2.07M (2.07M–2.08M) | 2.01M (1.99M–2.02M) | 2.07M (2.06M–2.07M) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.00M (1.99M–2.01M) | 2.06M (2.05M–2.06M) | 2.02M (2.01M–2.02M) | 2.08M (2.07M–2.09M) | 2.03M (2.03M–2.04M) | 2.09M (2.08M–2.09M) | 2.20M (2.19M–2.21M) | 2.67M (2.64M–2.68M) | 2.76M (2.73M–2.77M) | 2.67M (2.64M–2.68M) | 2.75M (2.74M–2.77M) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 2.57M (2.55M–2.59M) | 2.66M (2.66M–2.69M) | 2.58M (2.58M–2.64M) | 2.69M (2.68M–2.73M) | 2.55M (2.53M–2.58M) | 2.52M (2.46M–2.65M) | 2.76M (2.74M–2.83M) | 2.63M (2.61M–2.75M) | 3.07M (3.04M–3.12M) | 3.59M (3.53M–3.60M) | 4.04M (4.02M–4.10M) | 67% | 23 |
| 10 | 1000 | 50 | 8 | 3.14M (3.05M–3.19M) | 3.29M (3.25M–3.30M) | 3.21M (3.15M–3.22M) | 3.33M (3.29M–3.34M) | 2.99M (2.96M–3.01M) | 3.11M (2.87M–3.14M) | 3.32M (3.23M–3.46M) | 3.37M (3.22M–3.42M) | 3.81M (3.79M–3.82M) | 4.06M (3.96M–4.20M) | 5.27M (5.26M–5.34M) | 50% | 14 |
| 10 | 1000 | 200 | 2 | 1.07M (1.07M–1.08M) | 1.10M (1.10M–1.10M) | 1.09M (1.08M–1.09M) | 1.11M (1.11M–1.11M) | 1.11M (1.10M–1.11M) | 1.24M (1.23M–1.24M) | 1.28M (1.28M–1.28M) | 1.24M (1.22M–1.24M) | 1.28M (1.27M–1.28M) | 1.24M (1.23M–1.24M) | 1.28M (1.27M–1.28M) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 1.46M (1.46M–1.47M) | 1.51M (1.50M–1.51M) | 1.48M (1.48M–1.48M) | 1.52M (1.52M–1.53M) | 1.51M (1.51M–1.52M) | 1.52M (1.51M–1.52M) | 1.62M (1.61M–1.62M) | 1.84M (1.80M–1.85M) | 1.90M (1.88M–1.90M) | 1.84M (1.81M–1.84M) | 1.90M (1.89M–1.91M) | 100% | 0 |
| 10 | 1000 | 200 | 4 | 1.84M (1.84M–1.84M) | 1.89M (1.89M–1.89M) | 1.87M (1.86M–1.87M) | 1.92M (1.91M–1.92M) | 1.88M (1.88M–1.89M) | 1.92M (1.91M–1.92M) | 2.04M (2.04M–2.04M) | 2.44M (2.38M–2.45M) | 2.53M (2.50M–2.53M) | 2.43M (2.38M–2.44M) | 2.52M (2.49M–2.53M) | 100% | 0 |
| 10 | 1000 | 200 | 6 | 2.38M (2.35M–2.42M) | 2.44M (2.43M–2.45M) | 2.42M (2.40M–2.47M) | 2.50M (2.49M–2.53M) | 2.38M (2.35M–2.42M) | 2.37M (2.31M–2.43M) | 2.61M (2.59M–2.68M) | 2.50M (2.40M–2.54M) | 2.88M (2.83M–2.91M) | 2.51M (2.47M–2.67M) | 3.67M (3.64M–3.68M) | 66% | 40 |
| 10 | 1000 | 200 | 8 | 2.91M (2.80M–2.92M) | 2.99M (2.97M–3.01M) | 3.00M (2.89M–3.02M) | 3.11M (3.10M–3.12M) | 2.83M (2.80M–2.87M) | 2.83M (2.74M–2.96M) | 3.13M (3.01M–3.22M) | 2.91M (2.90M–3.09M) | 3.56M (3.49M–3.67M) | 2.65M (2.55M–2.80M) | 4.71M (4.70M–4.73M) | 49% | 33 |
| 10 | 1000 | 1000 | 2 | 709k (708k–710k) | 726k (725k–727k) | 737k (736k–738k) | 750k (746k–750k) | 749k (747k–749k) | 825k (817k–826k) | 837k (832k–839k) | 826k (819k–827k) | 839k (836k–840k) | 826k (823k–828k) | 838k (836k–840k) | 100% | 0 |
| 10 | 1000 | 1000 | 3 | 966k (964k–968k) | 989k (986k–991k) | 1.02M (1.02M–1.02M) | 1.04M (1.04M–1.04M) | 1.04M (1.04M–1.04M) | 997k (991k–998k) | 1.09M (1.08M–1.09M) | 919k (907k–919k) | 1.21M (1.21M–1.22M) | 919k (919k–920k) | 1.21M (1.20M–1.22M) | 100% | 0 |
| 10 | 1000 | 1000 | 4 | 1.20M (1.20M–1.21M) | 1.23M (1.23M–1.24M) | 1.29M (1.28M–1.29M) | 1.32M (1.32M–1.32M) | 1.31M (1.30M–1.31M) | 1.27M (1.26M–1.27M) | 1.39M (1.39M–1.39M) | 916k (913k–918k) | 1.55M (1.55M–1.56M) | 916k (914k–917k) | 1.56M (1.55M–1.56M) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 1.55M (1.54M–1.56M) | 1.60M (1.59M–1.61M) | 1.69M (1.68M–1.70M) | 1.74M (1.72M–1.74M) | 1.71M (1.71M–1.72M) | 1.55M (1.54M–1.58M) | 1.81M (1.81M–1.86M) | 1.28M (1.27M–1.30M) | 1.89M (1.86M–1.90M) | 836k (825k–840k) | 2.11M (2.11M–2.11M) | 66% | 61 |
| 10 | 1000 | 1000 | 8 | 1.89M (1.88M–1.89M) | 1.89M (1.89M–1.90M) | 2.04M (2.04M–2.05M) | 2.11M (1.97M–2.14M) | 2.06M (2.05M–2.10M) | 1.89M (1.87M–1.92M) | 2.23M (2.15M–2.29M) | 1.39M (1.36M–1.40M) | 2.38M (2.29M–2.39M) | 838k (820k–846k) | 2.59M (2.58M–2.59M) | 49% | 69 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.29 / 0.37 (0.33–0.42) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.12 / 0.29 / 0.37 (0.33–0.42) | 0.08 / 0.12 / 0.29 (0.25–0.29) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.12 / 0.25 / 0.37 (0.37–0.46) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.12 / 0.33 / 0.37 (0.37–0.42) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.12 / 0.33 / 0.37 (0.37–0.37) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 100% | 0 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.25 / 0.54 / 0.87 (0.83–0.91) | 0.08 / 0.21 / 0.37 (0.33–0.42) | 0.29 / 0.62 / 1.00 (0.83–1.00) | 0.08 / 0.21 / 0.46 (0.46–0.46) | 0.33 / 0.70 / 0.87 (0.83–0.87) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 67% | 10 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.50 / 1.00 (0.95–1.20) | 0.08 / 0.25 / 0.42 (0.42–0.46) | 0.33 / 0.78 / 1.15 (0.91–1.20) | 0.08 / 0.25 / 0.50 (0.50–0.50) | 0.66 / 1.12 / 1.46 (1.25–1.57) | 0.08 / 0.25 / 0.50 (0.50–0.50) | 50% | 111 |
| 1 | 0 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.29 / 0.46 / 0.50 (0.50–0.50) | 0.21 / 0.25 / 0.54 (0.54–0.54) | 0.29 / 0.42 / 0.50 (0.50–0.50) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.29 / 0.42 / 0.50 (0.50–0.54) | 0.21 / 0.25 / 0.54 (0.54–0.54) | 100% | 0 |
| 1 | 0 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.25 / 0.42 / 0.54 (0.50–0.54) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.29 / 0.46 / 0.54 (0.54–0.54) | 0.21 / 0.25 / 0.62 (0.62–0.66) | 0.29 / 0.46 / 0.54 (0.50–0.54) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 100% | 0 |
| 1 | 0 | 200 | 4 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.29 / 0.46 / 0.58 (0.58–0.62) | 0.21 / 0.25 / 0.58 (0.58–0.58) | 0.29 / 0.46 / 0.54 (0.54–0.78) | 0.21 / 0.25 / 0.66 (0.66–0.66) | 0.29 / 0.46 / 0.54 (0.54–0.54) | 0.21 / 0.25 / 0.66 (0.66–0.66) | 100% | 0 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.70 / 1.00 (1.00–1.07) | 0.21 / 0.33 / 0.70 (0.66–0.74) | 0.42 / 0.83 / 1.28 (1.25–1.33) | 0.21 / 0.37 / 0.83 (0.79–0.83) | 1.00 / 1.36 / 1.49 (1.49–1.90) | 0.21 / 0.50 / 0.87 (0.87–0.87) | 66% | 29 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.66 / 1.12 (1.07–1.25) | 0.21 / 0.37 / 0.74 (0.70–0.79) | 0.42 / 1.00 / 1.41 (1.20–1.49) | 0.21 / 0.50 / 0.83 (0.78–0.83) | 1.74 / 2.24 / 8.83 (5.82–9.73) | 0.21 / 0.62 / 0.87 (0.87–0.91) | 50% | 0 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.07 / 1.25 / 1.33 (1.28–1.33) | 1.00 / 1.04 / 2.94 (2.91–2.94) | 1.07 / 1.25 / 1.33 (1.33–1.33) | 1.00 / 1.04 / 2.94 (2.91–2.94) | 1.07 / 1.25 / 1.33 (1.28–1.33) | 1.00 / 1.04 / 2.85 (2.85–2.94) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.04 / 1.28 / 2.03 (2.03–2.08) | 1.00 / 1.04 / 2.91 (2.91–2.91) | 1.87 / 2.03 / 2.08 (2.08–5.50) | 1.00 / 1.62 / 2.98 (2.98–3.04) | 1.87 / 2.03 / 2.08 (2.08–2.11) | 1.00 / 1.70 / 2.98 (2.98–3.04) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.07 / 1.49 / 2.11 (2.11–2.14) | 1.00 / 1.87 / 2.94 (2.91–2.98) | 2.98 / 3.10 / 3.65 (3.23–8.58) | 1.00 / 2.24 / 3.07 (3.04–3.07) | 2.98 / 3.10 / 3.23 (3.20–4.42) | 1.00 / 2.21 / 3.04 (3.04–3.07) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.15 / 1.95 / 2.40 (2.27–2.40) | 1.00 / 2.11 / 3.10 (3.04–3.10) | 3.20 / 3.90 / 4.74 (4.67–7.87) | 1.00 / 2.50 / 3.23 (3.20–3.26) | 5.82 / 6.14 / 11.14 (9.73–12.03) | 1.00 / 2.69 / 3.26 (3.23–3.33) | 66% | 15 |
| 1 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.15 / 2.03 / 2.43 (2.40–2.53) | 1.00 / 2.21 / 3.14 (3.07–3.14) | 3.49 / 4.10 / 8.83 (8.00–9.22) | 1.00 / 2.53 / 3.39 (3.36–3.65) | 8.32 / 8.83 / 17.41 (16.26–23.81) | 1.00 / 2.91 / 3.94 (3.68–4.03) | 49% | 0 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.21) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.12 / 0.29 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.29 (0.25–0.29) | 0.12 / 0.29 / 0.33 (0.33–0.37) | 0.08 / 0.12 / 0.29 (0.25–0.29) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.17 (0.12–0.21) | 0.08 / 0.08 / 0.17 (0.12–0.21) | 0.08 / 0.08 / 0.12 (0.12–0.21) | 0.12 / 0.29 / 0.37 (0.33–0.42) | 0.08 / 0.12 / 0.25 (0.21–0.25) | 0.12 / 0.33 / 0.37 (0.37–0.42) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.12 / 0.33 / 0.42 (0.37–0.42) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.25 / 0.50 / 0.83 (0.74–0.87) | 0.08 / 0.21 / 0.42 (0.37–0.42) | 0.29 / 0.62 / 0.95 (0.79–0.95) | 0.08 / 0.21 / 0.46 (0.42–0.46) | 0.33 / 0.70 / 0.91 (0.83–0.95) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 66% | 5 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.25 / 0.54 / 1.07 (0.87–1.12) | 0.08 / 0.25 / 0.42 (0.42–0.50) | 0.33 / 0.78 / 1.15 (1.12–1.15) | 0.08 / 0.25 / 0.50 (0.46–0.50) | 0.74 / 1.15 / 1.36 (1.33–1.54) | 0.08 / 0.25 / 0.50 (0.50–0.50) | 50% | 0 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.42 / 0.50 (0.50–3.87) | 0.21 / 0.25 / 0.54 (0.54–0.66) | 0.29 / 0.42 / 0.50 (0.50–5.31) | 0.21 / 0.25 / 0.54 (0.54–0.66) | 0.29 / 0.42 / 0.50 (0.50–0.54) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.21 / 0.25 / 0.54 (0.50–0.58) | 0.29 / 0.46 / 0.54 (0.50–3.46) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 0.29 / 0.46 / 0.54 (0.54–0.58) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.29 / 0.46 / 0.58 (0.58–0.66) | 0.21 / 0.25 / 0.54 (0.50–0.54) | 0.29 / 0.46 / 0.54 (0.54–6.27) | 0.21 / 0.29 / 0.66 (0.66–0.66) | 0.29 / 0.46 / 0.54 (0.54–3.55) | 0.21 / 0.29 / 0.66 (0.66–0.66) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 0.70 / 1.07 (0.95–4.93) | 0.21 / 0.37 / 0.70 (0.66–0.70) | 0.42 / 0.87 / 1.28 (1.28–1.28) | 0.21 / 0.37 / 0.83 (0.79–0.83) | 1.00 / 1.36 / 1.49 (1.46–1.54) | 0.21 / 0.50 / 0.87 (0.87–0.87) | 66% | 7 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.66 / 1.20 (1.12–1.33) | 0.21 / 0.37 / 0.70 (0.70–0.79) | 0.46 / 1.04 / 1.46 (1.46–1.49) | 0.21 / 0.46 / 0.83 (0.78–0.83) | 1.79 / 2.24 / 8.19 (6.21–8.96) | 0.21 / 0.62 / 0.87 (0.87–0.91) | 50% | 0 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.07 / 1.25 / 1.33 (1.33–1.74) | 1.00 / 1.04 / 2.94 (2.91–2.98) | 1.07 / 1.25 / 1.33 (1.33–1.33) | 1.00 / 1.04 / 2.94 (2.91–2.94) | 1.07 / 1.25 / 1.33 (1.33–1.33) | 1.00 / 1.04 / 2.91 (2.91–2.94) | 100% | 2 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–2.40) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.04 / 1.28 / 2.11 (2.11–2.11) | 1.00 / 1.04 / 2.78 (2.78–2.82) | 1.87 / 2.03 / 2.08 (2.08–2.08) | 1.00 / 1.62 / 2.98 (2.98–6.27) | 1.87 / 2.03 / 2.11 (2.08–2.14) | 1.00 / 1.66 / 2.98 (2.98–3.04) | 100% | 0 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.07 / 1.90 / 2.14 (2.11–2.14) | 1.00 / 1.04 / 2.69 (2.66–2.72) | 2.98 / 3.10 / 3.26 (3.20–9.47) | 1.00 / 2.24 / 3.04 (3.04–4.54) | 2.98 / 3.10 / 3.52 (3.23–7.87) | 1.00 / 2.21 / 3.04 (3.04–3.04) | 100% | 0 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.25 / 2.08 / 2.43 (2.27–2.43) | 1.00 / 1.15 / 2.66 (2.62–2.72) | 2.69 / 3.87 / 4.74 (4.67–5.25) | 1.00 / 2.27 / 3.14 (3.14–3.20) | 5.82 / 6.14 / 11.26 (9.09–13.06) | 1.00 / 2.69 / 3.26 (3.26–3.81) | 66% | 11 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.20 / 2.11 / 2.53 (2.43–8.58) | 1.00 / 1.15 / 2.69 (2.56–3.20) | 3.49 / 4.03 / 8.83 (8.45–9.86) | 1.00 / 2.27 / 3.23 (3.14–3.26) | 8.19 / 8.70 / 17.66 (17.41–19.20) | 1.00 / 2.91 / 3.90 (3.81–3.90) | 49% | 67 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.12 / 0.29 / 0.46 (0.46–0.46) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.12 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.25 / 0.33 (0.33–0.33) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.37 / 0.50 (0.50–0.50) | 0.08 / 0.25 / 0.33 (0.33–0.37) | 0.12 / 0.33 / 0.42 (0.42–0.62) | 0.08 / 0.29 / 0.37 (0.37–0.42) | 0.12 / 0.33 / 0.46 (0.42–0.54) | 0.08 / 0.29 / 0.37 (0.37–0.42) | 100% | 0 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.66 / 1.00 (0.78–1.15) | 0.12 / 0.33 / 0.58 (0.50–0.58) | 0.33 / 1.00 / 1.36 (1.12–1.41) | 0.12 / 0.42 / 0.58 (0.54–0.62) | 0.33 / 0.79 / 1.00 (0.91–1.04) | 0.08 / 0.46 / 0.66 (0.66–0.83) | 67% | 6 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.21 / 0.74 / 1.07 (1.04–1.12) | 0.12 / 0.33 / 0.50 (0.46–0.58) | 0.25 / 0.91 / 1.25 (1.20–1.54) | 0.12 / 0.42 / 0.54 (0.54–0.54) | 0.54 / 1.07 / 2.53 (1.54–5.95) | 0.12 / 0.50 / 0.79 (0.74–0.91) | 50% | 16 |
| 10 | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–5.50) | 0.21 / 0.25 / 0.25 (0.25–0.50) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.29 / 0.42 / 0.50 (0.50–0.54) | 0.21 / 0.37 / 0.66 (0.66–0.70) | 0.29 / 0.46 / 0.54 (0.50–0.54) | 0.21 / 0.37 / 0.70 (0.66–0.70) | 0.29 / 0.46 / 0.54 (0.50–0.70) | 0.21 / 0.37 / 0.70 (0.66–0.70) | 99% | 15 |
| 10 | 0 | 200 | 3 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.25 / 0.50 / 0.66 (0.66–0.70) | 0.21 / 0.54 / 0.70 (0.70–0.70) | 0.29 / 0.46 / 0.58 (0.58–0.62) | 0.21 / 0.58 / 0.70 (0.70–0.70) | 0.29 / 0.46 / 0.58 (0.54–0.62) | 0.21 / 0.58 / 0.70 (0.70–0.74) | 100% | 0 |
| 10 | 0 | 200 | 4 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.29 / 0.58 / 0.74 (0.74–0.74) | 0.21 / 0.58 / 0.74 (0.74–0.79) | 0.29 / 0.50 / 0.62 (0.62–0.66) | 0.21 / 0.66 / 1.00 (0.95–1.04) | 0.29 / 0.54 / 0.66 (0.62–0.74) | 0.21 / 0.66 / 1.00 (0.95–1.00) | 100% | 0 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.91 / 1.20 (0.95–1.36) | 0.25 / 0.74 / 1.00 (0.87–1.07) | 0.37 / 1.12 / 1.46 (1.41–1.74) | 0.25 / 0.83 / 1.33 (1.33–1.41) | 0.91 / 1.41 / 1.74 (1.66–2.62) | 0.25 / 1.04 / 1.66 (1.62–1.66) | 66% | 29 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 1.00 / 1.33 (1.15–1.41) | 0.25 / 0.70 / 0.87 (0.87–0.95) | 0.50 / 1.46 / 2.00 (1.74–2.11) | 0.25 / 0.83 / 1.28 (1.28–1.33) | 1.62 / 2.27 / 6.27 (3.39–9.09) | 0.25 / 1.41 / 2.21 (2.21–2.21) | 50% | 22 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.07 / 1.25 / 1.33 (1.33–1.36) | 1.00 / 2.82 / 3.07 (3.07–3.07) | 1.07 / 1.25 / 1.36 (1.33–1.36) | 1.00 / 2.85 / 3.07 (3.07–3.07) | 1.07 / 1.25 / 1.33 (1.33–1.36) | 1.00 / 2.85 / 3.07 (3.07–3.07) | 100% | 0 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.04 / 1.04 (1.04–1.04) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 1.07 / 2.08 / 2.27 (2.24–2.27) | 1.00 / 2.91 / 4.42 (4.35–4.48) | 1.90 / 2.08 / 2.43 (2.14–4.99) | 1.00 / 3.04 / 4.22 (4.16–4.22) | 1.90 / 2.08 / 2.14 (2.14–6.21) | 1.00 / 3.04 / 4.22 (4.22–4.35) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 1.07 / 2.11 / 2.27 (2.27–2.30) | 1.00 / 3.10 / 4.67 (4.54–4.74) | 2.98 / 3.23 / 6.59 (3.90–12.42) | 1.00 / 3.52 / 4.86 (4.86–4.93) | 2.98 / 3.23 / 6.21 (3.36–9.73) | 1.00 / 3.52 / 4.86 (4.86–4.86) | 100% | 0 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.25 / 2.43 / 2.91 (2.91–2.94) | 1.04 / 3.90 / 6.53 (6.27–6.59) | 2.37 / 4.42 / 5.12 (5.06–5.63) | 1.04 / 4.54 / 6.40 (6.21–6.66) | 5.82 / 6.27 / 12.42 (10.24–15.10) | 1.04 / 4.86 / 6.40 (6.34–6.53) | 67% | 28 |
| 10 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.25 / 2.53 / 3.10 (2.82–3.33) | 1.04 / 4.48 / 8.45 (7.94–8.70) | 3.33 / 4.61 / 7.30 (6.14–8.45) | 1.07 / 5.31 / 7.87 (7.62–8.19) | 8.06 / 8.83 / 17.66 (16.38–18.43) | 1.07 / 5.25 / 7.68 (7.62–7.74) | 49% | 14 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.08 / 0.08 / 0.12 (0.12–0.12) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 0.12 / 0.29 / 0.33 (0.33–0.33) | 0.08 / 0.17 / 0.29 (0.29–0.33) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.12 / 0.29 / 0.42 (0.42–0.42) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.12 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.29 / 0.37 (0.37–0.42) | 0.08 / 0.25 / 0.33 (0.33–0.33) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.12 / 0.33 / 0.50 (0.50–0.50) | 0.08 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.33 / 0.42 (0.42–0.58) | 0.08 / 0.29 / 0.37 (0.37–0.42) | 0.12 / 0.33 / 0.42 (0.42–0.62) | 0.08 / 0.29 / 0.37 (0.37–0.37) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.17 / 0.74 / 1.04 (0.87–1.15) | 0.12 / 0.33 / 0.62 (0.54–0.66) | 0.29 / 0.95 / 1.28 (1.04–1.28) | 0.12 / 0.46 / 0.62 (0.54–0.62) | 0.33 / 0.79 / 1.00 (0.95–1.04) | 0.08 / 0.46 / 0.74 (0.66–0.83) | 67% | 23 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.25 / 0.83 / 1.33 (1.07–1.49) | 0.12 / 0.42 / 0.66 (0.54–0.70) | 0.29 / 0.91 / 1.28 (1.15–1.57) | 0.12 / 0.42 / 0.66 (0.66–0.66) | 0.62 / 1.25 / 1.58 (1.49–2.66) | 0.12 / 0.54 / 0.87 (0.74–0.91) | 50% | 14 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.46 / 0.54 (0.50–0.54) | 0.21 / 0.42 / 0.66 (0.66–0.70) | 0.29 / 0.46 / 0.54 (0.54–0.54) | 0.21 / 0.42 / 0.66 (0.66–0.66) | 0.29 / 0.46 / 0.54 (0.50–0.54) | 0.21 / 0.37 / 0.66 (0.66–0.70) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.25 / 0.50 / 0.66 (0.66–0.66) | 0.21 / 0.50 / 0.70 (0.70–0.70) | 0.29 / 0.46 / 0.58 (0.54–0.62) | 0.21 / 0.58 / 0.70 (0.70–0.74) | 0.29 / 0.46 / 0.58 (0.54–0.62) | 0.21 / 0.58 / 0.70 (0.70–0.74) | 100% | 0 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.29 / 0.58 / 0.74 (0.74–0.74) | 0.21 / 0.54 / 0.74 (0.74–0.79) | 0.29 / 0.50 / 0.58 (0.58–0.79) | 0.21 / 0.66 / 1.00 (0.95–1.00) | 0.29 / 0.50 / 0.70 (0.58–0.74) | 0.21 / 0.66 / 1.00 (0.91–1.00) | 100% | 0 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.91 / 1.28 (1.15–1.41) | 0.25 / 0.66 / 1.04 (0.91–1.07) | 0.42 / 1.15 / 1.57 (1.41–1.74) | 0.25 / 0.83 / 1.33 (1.28–1.41) | 1.07 / 1.49 / 1.74 (1.70–7.62) | 0.25 / 1.07 / 1.70 (1.58–1.70) | 66% | 40 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 1.12 / 1.62 (1.33–1.74) | 0.25 / 0.70 / 1.12 (1.04–1.15) | 0.50 / 1.54 / 2.08 (1.66–2.11) | 0.25 / 0.87 / 1.41 (1.25–1.46) | 1.66 / 2.30 / 7.87 (5.25–12.03) | 0.25 / 1.41 / 2.21 (2.21–2.27) | 49% | 33 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.07 / 1.25 / 1.36 (1.33–1.36) | 1.00 / 2.85 / 3.07 (3.07–3.07) | 1.07 / 1.25 / 1.33 (1.33–1.36) | 1.00 / 2.72 / 3.07 (3.07–3.07) | 1.07 / 1.25 / 1.36 (1.33–1.36) | 1.00 / 2.82 / 3.07 (3.07–3.07) | 100% | 0 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.12) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.04 / 2.11 / 2.27 (2.24–9.98) | 1.00 / 2.72 / 3.26 (3.10–3.87) | 1.90 / 2.08 / 2.27 (2.21–11.90) | 1.00 / 3.04 / 4.22 (4.16–4.48) | 1.90 / 2.08 / 2.14 (2.14–2.21) | 1.00 / 3.07 / 4.22 (4.16–7.49) | 100% | 0 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.04) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.04) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.07 / 2.11 / 2.27 (2.27–2.30) | 1.00 / 2.69 / 3.68 (3.62–3.74) | 2.98 / 3.20 / 3.65 (3.33–8.19) | 1.00 / 3.52 / 4.86 (4.86–5.18) | 2.98 / 3.23 / 6.46 (3.33–8.06) | 1.00 / 3.52 / 4.86 (4.80–4.93) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.20 / 2.40 / 2.91 (2.82–2.94) | 1.04 / 3.07 / 4.74 (4.48–4.93) | 2.30 / 4.42 / 4.99 (4.99–5.89) | 1.04 / 4.06 / 6.02 (5.89–6.21) | 5.82 / 6.27 / 11.39 (8.83–11.78) | 1.04 / 4.86 / 6.46 (6.40–6.53) | 66% | 61 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.49) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.20 / 2.53 / 3.14 (3.04–3.23) | 1.04 / 3.36 / 5.25 (4.86–5.82) | 3.26 / 4.54 / 8.96 (6.08–10.75) | 1.04 / 4.74 / 7.17 (6.91–9.09) | 8.13 / 8.83 / 17.66 (16.64–21.50) | 1.07 / 5.25 / 7.68 (7.55–7.68) | 49% | 69 |

Delete (removal) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.66 / 0.83 / 0.91 (0.87–3.46) | 0.62 / 0.83 / 0.91 (0.87–1.25) | 0.58 / 0.79 / 0.87 (0.79–11.26) | 0.58 / 0.79 / 0.87 (0.79–0.91) | 0.58 / 0.78 / 0.83 (0.79–0.91) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.21 / 0.33 / 0.42 (0.33–0.46) | 0.25 / 0.33 / 0.37 (0.37–0.42) | 0.25 / 0.29 / 0.37 (0.33–0.42) | 0.25 / 0.33 / 0.42 (0.42–0.46) | 0.21 / 0.33 / 0.33 (0.29–0.58) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.79 / 1.12 / 3.87 (1.25–8.45) | 0.83 / 1.07 / 1.28 (1.20–5.06) | 0.74 / 1.04 / 1.28 (1.15–4.42) | 0.74 / 1.04 / 1.15 (1.07–4.54) | 0.74 / 1.04 / 1.25 (1.20–7.74) | 0.79 / 1.04 / 1.20 (1.12–1.25) | 0.70 / 0.95 / 1.20 (1.04–3.39) | 0.25 / 0.37 / 0.46 (0.42–0.70) | 0.25 / 0.33 / 0.42 (0.37–3.26) | 0.25 / 0.37 / 0.46 (0.42–0.50) | 0.25 / 0.33 / 0.50 (0.46–3.39) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.87 / 1.20 / 5.57 (1.33–6.72) | 0.87 / 1.25 / 3.87 (1.36–18.18) | 0.83 / 1.07 / 3.90 (1.41–6.78) | 0.79 / 1.07 / 4.42 (1.15–9.34) | 0.79 / 1.12 / 1.95 (1.15–14.85) | 0.91 / 1.33 / 1.62 (1.54–4.03) | 0.83 / 1.49 / 3.94 (3.90–4.29) | 0.25 / 0.42 / 0.46 (0.46–0.58) | 0.25 / 0.33 / 3.14 (0.42–3.39) | 0.25 / 0.42 / 0.46 (0.46–0.50) | 0.25 / 0.33 / 0.50 (0.42–3.46) | 100% | 0 |
| 1 | 0 | 50 | 6 | 1.78 / 2.94 / 9.47 (3.52–12.16) | 1.79 / 2.72 / 7.10 (3.46–1081.34) | 1.74 / 2.72 / 6.98 (4.35–9.98) | 1.66 / 2.78 / 8.32 (6.78–225.28) | 1.70 / 2.66 / 5.82 (3.14–8.96) | 2.21 / 3.68 / 4.48 (4.29–4.99) | 1.95 / 3.52 / 5.12 (4.16–7.74) | 1.90 / 3.04 / 3.68 (2.91–6.21) | 1.70 / 2.85 / 3.33 (2.98–3.39) | 0.50 / 0.87 / 1.12 (0.95–1.54) | 0.42 / 0.79 / 1.00 (1.00–1.20) | 67% | 10 |
| 1 | 0 | 50 | 8 | 2.03 / 3.55 / 10.75 (9.73–1900.54) | 2.03 / 3.46 / 147.46 (11.26–3211.26) | 2.00 / 3.49 / 11.39 (9.47–32.51) | 2.00 / 3.39 / 16.00 (8.19–33.28) | 1.95 / 3.36 / 11.01 (10.62–4849.66) | 2.50 / 4.16 / 7.23 (5.76–123.90) | 2.37 / 4.00 / 5.50 (4.86–10.62) | 2.21 / 3.62 / 7.62 (4.29–10.62) | 1.95 / 3.20 / 4.29 (3.94–4.48) | 0.74 / 1.25 / 1.41 (1.41–8.32) | 0.50 / 0.83 / 1.07 (1.04–1.07) | 50% | 111 |
| 1 | 0 | 200 | 2 | 0.91 / 1.20 / 1.28 (1.25–11.90) | 0.91 / 1.25 / 1.28 (1.25–4.48) | 0.78 / 1.07 / 1.12 (1.12–5.25) | 0.78 / 1.07 / 1.15 (1.12–15.62) | 0.78 / 1.07 / 1.15 (1.12–6.27) | 0.37 / 0.50 / 0.58 (0.54–0.62) | 0.37 / 0.46 / 0.54 (0.50–0.70) | 0.37 / 0.46 / 0.50 (0.50–0.70) | 0.37 / 0.46 / 0.46 (0.46–0.74) | 0.37 / 0.46 / 0.66 (0.54–12.42) | 0.33 / 0.46 / 0.66 (0.50–0.66) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.54 / 4.74 (1.70–26.11) | 1.07 / 1.54 / 1.70 (1.66–6.14) | 0.95 / 1.36 / 1.57 (1.46–25.86) | 0.95 / 1.36 / 1.62 (1.54–11.90) | 0.95 / 1.36 / 1.54 (1.46–1.74) | 1.07 / 1.46 / 2.72 (1.54–20.48) | 0.91 / 1.25 / 1.41 (1.36–5.63) | 0.37 / 0.50 / 0.58 (0.54–0.70) | 0.37 / 0.46 / 0.66 (0.66–0.83) | 0.37 / 0.50 / 0.62 (0.58–0.62) | 0.37 / 0.46 / 0.83 (0.70–1.12) | 100% | 0 |
| 1 | 0 | 200 | 4 | 1.15 / 1.62 / 6.85 (1.66–12.80) | 1.15 / 1.66 / 5.12 (4.35–9.34) | 1.04 / 1.49 / 6.27 (4.54–22.27) | 1.04 / 1.49 / 4.93 (1.58–9.09) | 1.04 / 1.46 / 4.16 (1.57–5.31) | 1.25 / 1.74 / 2.21 (2.03–4.42) | 1.00 / 1.36 / 4.22 (1.49–8.45) | 0.37 / 0.54 / 1.49 (0.62–3.52) | 0.42 / 0.46 / 3.20 (0.62–3.46) | 0.37 / 0.50 / 0.58 (0.54–0.74) | 0.42 / 0.46 / 3.26 (0.70–4.86) | 100% | 0 |
| 1 | 0 | 200 | 6 | 2.08 / 3.20 / 9.60 (6.91–13.44) | 2.08 / 3.04 / 7.62 (4.10–155.65) | 1.95 / 3.10 / 9.22 (5.57–1032.19) | 1.95 / 2.91 / 8.70 (8.32–10.88) | 1.95 / 2.98 / 9.34 (6.27–12.29) | 2.21 / 4.03 / 5.57 (4.93–8.58) | 2.08 / 3.52 / 4.29 (4.10–4.86) | 2.14 / 3.36 / 4.48 (3.74–5.57) | 1.87 / 2.82 / 3.65 (3.14–7.23) | 1.12 / 1.46 / 1.66 (1.49–3.20) | 0.58 / 0.95 / 1.28 (1.20–1.41) | 66% | 29 |
| 1 | 0 | 200 | 8 | 2.30 / 3.87 / 11.39 (10.11–17.66) | 2.30 / 4.00 / 12.54 (9.60–172.03) | 2.21 / 4.29 / 24.58 (13.06–60.42) | 2.21 / 3.65 / 16.00 (9.98–41.47) | 2.24 / 3.62 / 13.44 (8.58–19.46) | 2.66 / 4.48 / 7.04 (6.08–11.39) | 2.62 / 4.22 / 5.63 (5.12–7.74) | 2.40 / 4.06 / 7.74 (6.40–10.88) | 2.08 / 3.20 / 4.61 (3.90–8.45) | 1.87 / 2.37 / 2.85 (2.50–8.83) | 0.62 / 1.00 / 1.33 (1.12–1.46) | 50% | 0 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.58 / 7.87 (3.62–15.36) | 2.50 / 3.58 / 5.95 (3.65–14.72) | 1.79 / 2.82 / 5.70 (2.85–15.10) | 1.79 / 2.82 / 10.50 (2.85–13.06) | 1.79 / 2.82 / 3.33 (2.85–16.00) | 1.15 / 1.28 / 1.46 (1.33–4.61) | 1.15 / 1.25 / 1.62 (1.54–13.95) | 1.15 / 1.28 / 1.36 (1.33–1.41) | 1.15 / 1.25 / 1.82 (1.46–2.11) | 1.15 / 1.28 / 1.36 (1.33–1.54) | 1.15 / 1.25 / 2.00 (1.66–2.40) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.94 / 4.54 / 7.30 (5.31–12.29) | 2.98 / 4.48 / 5.12 (4.74–12.42) | 2.27 / 3.74 / 6.53 (3.90–30.98) | 2.27 / 3.74 / 10.11 (4.00–17.66) | 2.30 / 3.68 / 4.86 (4.16–6.78) | 2.78 / 3.81 / 4.74 (4.48–13.44) | 1.87 / 2.94 / 9.86 (3.52–15.74) | 1.95 / 2.11 / 2.14 (2.14–7.23) | 1.20 / 1.25 / 2.24 (2.24–2.37) | 1.95 / 2.11 / 2.11 (2.11–6.98) | 1.20 / 1.28 / 4.86 (2.37–8.13) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.67 / 5.31 (4.80–24.58) | 3.26 / 4.80 / 7.17 (4.86–19.97) | 2.53 / 3.94 / 6.08 (5.25–22.53) | 2.56 / 4.00 / 5.12 (4.22–17.66) | 2.56 / 3.90 / 5.95 (4.22–6.53) | 3.39 / 4.86 / 6.27 (5.06–9.86) | 1.95 / 2.91 / 3.52 (3.04–4.93) | 3.04 / 3.20 / 4.29 (3.33–13.31) | 1.20 / 2.00 / 3.39 (3.26–4.29) | 3.04 / 3.20 / 3.33 (3.23–8.83) | 1.20 / 1.82 / 3.39 (2.37–4.29) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.27 / 13.31 (9.22–565.25) | 4.29 / 6.14 / 12.42 (7.74–13.18) | 3.52 / 5.31 / 11.90 (10.75–614.40) | 3.49 / 5.25 / 10.11 (6.78–217.09) | 3.52 / 5.25 / 7.36 (6.46–10.75) | 4.99 / 7.42 / 11.78 (8.13–18.94) | 3.23 / 5.06 / 6.27 (6.08–8.19) | 7.49 / 10.11 / 15.74 (11.39–811.01) | 2.82 / 4.10 / 5.82 (4.93–11.52) | 5.89 / 6.21 / 7.74 (6.34–14.08) | 1.36 / 2.43 / 2.91 (2.85–3.46) | 66% | 15 |
| 1 | 0 | 1000 | 8 | 4.61 / 7.04 / 13.57 (12.93–19.71) | 4.54 / 7.04 / 19.71 (11.01–31.23) | 3.81 / 6.21 / 14.08 (11.26–17.15) | 3.87 / 6.27 / 16.38 (12.54–19.46) | 3.87 / 6.08 / 13.31 (11.65–16.38) | 5.44 / 8.19 / 14.59 (13.31–17.66) | 3.62 / 5.63 / 7.94 (6.98–13.18) | 11.01 / 16.00 / 21.50 (18.43–22.27) | 2.98 / 4.42 / 5.50 (5.38–12.42) | 8.45 / 8.83 / 19.46 (14.98–61.95) | 1.49 / 2.66 / 3.07 (2.85–4.42) | 49% | 0 |
| 1 | 1000 | 50 | 2 | 1.62 / 1.87 / 1.95 (1.87–6.59) | 1.66 / 1.82 / 1.90 (1.87–4.48) | 1.57 / 1.79 / 1.87 (1.82–5.44) | 1.57 / 1.79 / 1.87 (1.79–6.72) | 1.57 / 1.74 / 1.82 (1.82–3.14) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.25 / 0.33 / 0.33 (0.33–0.37) | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.25 / 0.29 / 0.37 (0.33–0.42) | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.25 / 0.29 / 0.33 (0.33–0.42) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.78 / 3.07 / 4.06 (3.10–7.36) | 2.78 / 3.07 / 3.23 (3.14–6.66) | 2.69 / 3.04 / 4.16 (3.10–8.58) | 2.69 / 3.04 / 5.82 (3.10–9.60) | 2.69 / 3.04 / 4.54 (4.00–41.98) | 1.82 / 2.08 / 2.24 (2.11–15.74) | 1.66 / 1.90 / 2.11 (2.03–3.04) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.33 / 0.42 (0.42–3.20) | 0.25 / 0.37 / 0.46 (0.42–0.54) | 0.25 / 0.33 / 0.42 (0.37–4.99) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 3.14 / 6.78 (4.48–8.58) | 2.85 / 3.14 / 7.10 (3.26–9.09) | 2.82 / 4.80 / 15.49 (5.89–26.11) | 2.78 / 3.74 / 9.34 (3.81–11.52) | 2.78 / 3.90 / 6.98 (4.16–9.09) | 1.95 / 2.43 / 5.06 (2.53–8.70) | 1.78 / 2.14 / 4.67 (2.43–5.25) | 0.25 / 0.42 / 0.46 (0.42–0.50) | 0.25 / 0.37 / 0.79 (0.46–3.39) | 0.25 / 0.42 / 0.46 (0.46–0.66) | 0.25 / 0.46 / 3.14 (0.46–3.46) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.68 / 4.48 / 9.98 (7.23–13.57) | 3.68 / 4.54 / 11.52 (10.50–15.36) | 3.68 / 4.74 / 11.78 (7.36–13.82) | 3.65 / 4.61 / 12.42 (10.62–1081.34) | 3.65 / 5.57 / 12.29 (11.01–14.08) | 3.74 / 4.86 / 7.68 (5.57–11.39) | 3.78 / 4.74 / 5.95 (5.12–8.58) | 2.72 / 3.68 / 4.16 (3.81–8.45) | 2.53 / 3.33 / 4.35 (3.62–4.54) | 0.50 / 0.83 / 1.07 (0.95–2.69) | 0.46 / 0.83 / 1.04 (0.87–1.04) | 66% | 5 |
| 1 | 1000 | 50 | 8 | 4.00 / 5.38 / 15.87 (11.90–19.20) | 4.00 / 5.31 / 13.95 (10.88–15.23) | 3.90 / 5.38 / 13.31 (12.67–31.74) | 3.90 / 5.25 / 15.10 (11.26–17.15) | 4.00 / 6.40 / 14.46 (13.18–28.16) | 4.35 / 5.70 / 11.52 (9.98–12.42) | 4.16 / 5.12 / 7.49 (5.70–12.16) | 3.10 / 4.35 / 6.27 (5.44–12.16) | 2.78 / 4.06 / 5.06 (4.86–5.82) | 0.87 / 1.28 / 1.79 (1.41–5.57) | 0.50 / 0.83 / 1.04 (1.00–1.12) | 50% | 0 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.21 / 2.53 (2.24–6.08) | 1.90 / 2.24 / 2.37 (2.27–6.02) | 1.78 / 2.11 / 2.14 (2.11–4.48) | 1.78 / 2.11 / 7.87 (2.14–12.42) | 1.74 / 2.08 / 2.14 (2.11–6.72) | 0.37 / 0.46 / 0.58 (0.58–4.86) | 0.37 / 0.46 / 0.54 (0.46–0.70) | 0.37 / 0.46 / 0.54 (0.50–5.31) | 0.33 / 0.46 / 0.54 (0.46–0.66) | 0.37 / 0.46 / 0.58 (0.54–0.95) | 0.37 / 0.46 / 0.66 (0.46–0.66) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 3.52 / 3.68 (3.65–9.86) | 3.04 / 3.49 / 3.90 (3.62–7.74) | 2.91 / 3.39 / 5.63 (3.65–6.78) | 2.91 / 3.36 / 4.61 (3.55–8.19) | 2.91 / 3.39 / 4.29 (4.10–8.58) | 2.08 / 2.43 / 2.66 (2.56–29.18) | 1.87 / 2.21 / 4.99 (2.37–6.40) | 0.37 / 0.50 / 0.54 (0.54–5.57) | 0.37 / 0.46 / 0.66 (0.66–3.33) | 0.37 / 0.50 / 0.58 (0.54–0.70) | 0.37 / 0.46 / 0.74 (0.58–2.69) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 3.10 / 3.58 / 6.40 (3.81–37.38) | 3.10 / 3.65 / 7.49 (4.00–8.96) | 2.98 / 3.68 / 8.06 (3.74–11.26) | 3.04 / 3.52 / 10.88 (3.65–27.39) | 2.98 / 4.35 / 8.70 (7.42–11.78) | 2.21 / 2.72 / 4.00 (3.04–29.70) | 2.00 / 2.27 / 5.06 (2.62–8.06) | 0.37 / 0.54 / 0.74 (0.66–7.62) | 0.42 / 0.54 / 3.46 (0.79–3.87) | 0.37 / 0.50 / 0.66 (0.58–0.95) | 0.42 / 0.50 / 1.87 (0.70–3.20) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 3.90 / 5.06 / 12.93 (9.22–19.97) | 4.03 / 4.99 / 13.70 (11.26–16.26) | 3.90 / 4.99 / 12.42 (10.75–13.95) | 3.94 / 5.06 / 11.90 (10.75–565.25) | 3.94 / 5.95 / 13.18 (10.88–22.02) | 4.16 / 5.31 / 8.00 (5.70–19.97) | 3.81 / 4.93 / 5.44 (5.25–5.95) | 3.07 / 4.10 / 4.80 (4.06–9.09) | 2.72 / 3.55 / 4.67 (4.42–5.89) | 1.07 / 1.46 / 1.58 (1.54–5.70) | 0.58 / 0.95 / 1.25 (1.20–1.28) | 66% | 7 |
| 1 | 1000 | 200 | 8 | 4.29 / 5.89 / 16.38 (13.06–17.66) | 4.29 / 5.76 / 13.82 (11.78–25.34) | 4.16 / 5.70 / 14.46 (13.06–15.10) | 4.16 / 5.57 / 13.18 (11.90–17.15) | 4.22 / 6.78 / 15.49 (13.82–212.99) | 4.48 / 5.89 / 11.14 (10.50–13.57) | 4.35 / 5.25 / 6.91 (6.66–8.83) | 3.36 / 4.99 / 9.73 (6.53–16.90) | 2.91 / 4.29 / 5.12 (5.12–7.36) | 1.90 / 2.37 / 4.00 (2.78–30.46) | 0.66 / 1.04 / 1.33 (1.28–1.41) | 50% | 0 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.54 / 8.70 (4.61–16.13) | 3.52 / 4.54 / 7.49 (4.61–9.34) | 2.78 / 3.81 / 3.90 (3.87–25.34) | 2.82 / 3.81 / 3.90 (3.87–15.74) | 2.78 / 3.81 / 10.11 (7.17–385.02) | 1.15 / 1.28 / 1.36 (1.28–1.49) | 1.15 / 1.25 / 1.87 (1.36–2.24) | 1.15 / 1.28 / 1.46 (1.33–1.78) | 1.15 / 1.25 / 2.21 (1.46–10.11) | 1.15 / 1.28 / 1.62 (1.33–12.93) | 1.15 / 1.25 / 1.74 (1.46–2.11) | 100% | 2 |
| 1 | 1000 | 1000 | 3 | 4.67 / 6.59 / 16.13 (7.04–33.28) | 4.74 / 6.53 / 7.17 (6.72–15.49) | 3.94 / 5.76 / 6.21 (6.08–9.98) | 4.00 / 5.82 / 8.45 (6.08–34.82) | 3.94 / 5.82 / 9.98 (6.08–14.08) | 3.68 / 5.06 / 10.37 (5.95–12.42) | 2.91 / 3.94 / 5.44 (4.93–12.42) | 1.95 / 2.11 / 2.21 (2.14–15.62) | 1.20 / 1.25 / 2.24 (1.62–12.54) | 1.95 / 2.11 / 2.21 (2.11–6.21) | 1.20 / 1.25 / 2.21 (1.95–2.40) | 100% | 0 |
| 1 | 1000 | 1000 | 4 | 5.12 / 6.72 / 9.86 (6.98–28.16) | 5.12 / 6.78 / 11.52 (6.85–16.64) | 4.29 / 5.89 / 7.74 (6.08–12.93) | 4.42 / 5.95 / 8.13 (7.04–29.95) | 4.35 / 6.02 / 7.49 (6.53–19.20) | 3.81 / 5.57 / 8.83 (6.40–8.96) | 2.94 / 4.00 / 5.06 (4.16–16.00) | 3.04 / 3.20 / 3.33 (3.26–7.49) | 1.20 / 1.90 / 4.16 (3.39–4.29) | 3.04 / 3.20 / 3.36 (3.33–4.10) | 1.20 / 1.49 / 2.30 (2.24–3.33) | 100% | 0 |
| 1 | 1000 | 1000 | 6 | 6.21 / 8.32 / 17.92 (14.08–925.70) | 6.21 / 8.06 / 15.87 (9.98–19.71) | 5.44 / 7.30 / 13.06 (7.87–105.47) | 5.44 / 7.17 / 13.44 (8.45–16.38) | 5.44 / 8.06 / 11.14 (10.50–17.41) | 6.21 / 8.13 / 11.39 (9.22–17.66) | 4.99 / 6.66 / 10.24 (7.68–11.39) | 7.74 / 10.75 / 14.21 (12.16–20.99) | 3.74 / 4.80 / 7.87 (5.89–9.47) | 5.95 / 6.27 / 9.22 (6.40–13.57) | 1.36 / 2.43 / 2.91 (2.82–3.07) | 66% | 11 |
| 1 | 1000 | 1000 | 8 | 6.53 / 9.60 / 352.26 (17.92–1081.34) | 6.59 / 8.83 / 17.66 (15.23–811.01) | 5.82 / 8.45 / 15.62 (14.21–20.99) | 5.82 / 8.58 / 16.13 (14.85–23.81) | 5.89 / 9.60 / 16.64 (13.82–24.83) | 6.34 / 9.98 / 15.49 (11.65–1441.79) | 5.25 / 7.04 / 8.83 (7.94–299.01) | 10.50 / 16.00 / 20.74 (18.18–30.72) | 3.87 / 5.12 / 6.34 (6.02–10.75) | 8.32 / 8.83 / 19.46 (12.67–27.65) | 1.49 / 2.66 / 2.98 (2.98–3.10) | 49% | 67 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 4.48 (0.91–6.14) | 0.62 / 0.83 / 1.04 (0.95–1.04) | 0.58 / 0.79 / 0.87 (0.87–1.04) | 0.58 / 0.79 / 0.91 (0.91–4.61) | 0.58 / 0.79 / 0.91 (0.87–0.95) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.21 / 0.33 / 0.46 (0.46–0.46) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.83 / 1.12 / 1.66 (1.33–8.00) | 0.83 / 1.12 / 1.49 (1.28–4.61) | 0.74 / 1.07 / 1.28 (1.25–1.41) | 0.74 / 1.07 / 1.28 (1.25–9.34) | 0.74 / 1.07 / 1.28 (1.25–3.62) | 0.83 / 1.12 / 1.36 (1.33–1.41) | 0.70 / 1.04 / 1.33 (1.25–1.46) | 0.25 / 0.42 / 0.54 (0.50–0.58) | 0.25 / 0.42 / 0.54 (0.54–0.58) | 0.25 / 0.42 / 0.54 (0.50–0.54) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.87 / 1.28 / 1.62 (1.49–8.96) | 0.87 / 1.33 / 3.46 (2.00–10.11) | 0.83 / 1.25 / 2.94 (1.41–4.99) | 0.83 / 1.33 / 5.18 (2.50–13.31) | 0.83 / 1.25 / 4.22 (1.49–6.40) | 0.95 / 1.58 / 1.95 (1.87–2.24) | 0.83 / 1.36 / 1.82 (1.62–2.66) | 0.25 / 0.46 / 0.58 (0.54–0.87) | 0.25 / 0.46 / 0.58 (0.58–0.83) | 0.25 / 0.50 / 0.62 (0.54–0.79) | 0.25 / 0.46 / 0.58 (0.58–0.78) | 100% | 0 |
| 10 | 0 | 50 | 6 | 1.49 / 2.98 / 6.59 (5.12–8.70) | 1.49 / 2.72 / 5.95 (4.61–9.60) | 1.46 / 2.85 / 5.82 (4.80–6.66) | 1.46 / 2.78 / 6.21 (5.57–11.39) | 1.49 / 3.04 / 7.62 (5.63–18.94) | 1.87 / 3.87 / 5.63 (3.39–11.39) | 1.90 / 4.35 / 5.76 (5.31–8.83) | 2.03 / 4.16 / 5.57 (4.48–9.86) | 1.57 / 3.36 / 4.93 (3.65–6.53) | 0.50 / 0.95 / 1.25 (1.20–2.91) | 0.42 / 0.87 / 1.12 (1.07–1.36) | 67% | 6 |
| 10 | 0 | 50 | 8 | 1.78 / 4.29 / 9.86 (9.34–13.06) | 1.74 / 3.90 / 10.62 (9.47–17.66) | 1.70 / 4.29 / 10.24 (8.83–12.93) | 1.70 / 3.87 / 9.47 (8.32–13.57) | 1.70 / 4.03 / 9.73 (9.09–20.22) | 2.21 / 4.61 / 6.72 (5.89–7.42) | 2.24 / 4.99 / 7.62 (6.34–9.09) | 1.87 / 3.94 / 6.53 (4.67–7.62) | 1.66 / 3.52 / 4.48 (4.42–4.86) | 0.62 / 1.25 / 4.80 (1.66–5.57) | 0.46 / 1.00 / 1.28 (1.20–1.58) | 50% | 16 |
| 10 | 0 | 200 | 2 | 0.91 / 1.20 / 1.33 (1.33–2.21) | 0.91 / 1.25 / 1.36 (1.33–5.82) | 0.78 / 1.07 / 1.20 (1.15–442.37) | 0.78 / 1.12 / 1.25 (1.20–14.72) | 0.78 / 1.12 / 1.25 (1.20–3.04) | 0.37 / 0.50 / 0.62 (0.58–0.66) | 0.33 / 0.54 / 0.70 (0.66–0.70) | 0.37 / 0.50 / 0.66 (0.62–1.90) | 0.33 / 0.54 / 0.70 (0.66–0.70) | 0.37 / 0.54 / 0.70 (0.66–0.87) | 0.37 / 0.54 / 0.70 (0.70–0.87) | 99% | 15 |
| 10 | 0 | 200 | 3 | 1.07 / 1.58 / 5.18 (1.79–21.25) | 1.07 / 1.66 / 13.82 (1.82–23.30) | 0.95 / 1.41 / 2.21 (1.62–24.06) | 0.95 / 1.46 / 5.06 (1.70–20.22) | 0.95 / 1.46 / 5.12 (4.06–10.62) | 1.07 / 1.49 / 2.00 (1.79–2.30) | 0.91 / 1.33 / 2.00 (1.62–3.94) | 0.37 / 0.58 / 0.70 (0.66–0.74) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.37 / 0.58 / 0.74 (0.70–0.79) | 0.37 / 0.66 / 0.79 (0.78–0.79) | 100% | 0 |
| 10 | 0 | 200 | 4 | 1.15 / 1.74 / 4.00 (2.53–8.00) | 1.15 / 1.79 / 6.72 (4.03–17.15) | 1.04 / 1.62 / 3.62 (2.08–10.50) | 1.04 / 1.62 / 6.34 (3.39–10.88) | 1.04 / 1.66 / 5.31 (2.00–13.82) | 1.25 / 2.08 / 2.56 (2.53–4.80) | 1.04 / 1.66 / 3.26 (1.87–4.10) | 0.37 / 0.62 / 0.74 (0.74–0.79) | 0.42 / 0.74 / 0.95 (0.83–1.12) | 0.37 / 0.66 / 0.91 (0.74–1.15) | 0.42 / 0.70 / 0.83 (0.83–0.87) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.14 / 4.29 / 8.96 (7.81–16.64) | 1.82 / 3.49 / 8.19 (5.57–9.09) | 1.66 / 3.46 / 6.53 (6.21–7.81) | 1.66 / 3.36 / 8.58 (6.46–10.50) | 1.70 / 3.36 / 6.53 (6.27–9.47) | 2.30 / 4.80 / 6.34 (4.06–8.83) | 2.24 / 4.67 / 6.34 (4.48–6.53) | 1.95 / 3.68 / 6.40 (4.42–7.30) | 1.62 / 2.94 / 3.87 (3.68–5.25) | 1.00 / 1.54 / 1.82 (1.78–2.62) | 0.66 / 1.41 / 1.87 (1.74–2.00) | 66% | 29 |
| 10 | 0 | 200 | 8 | 2.03 / 4.48 / 10.24 (9.47–13.57) | 2.03 / 4.48 / 9.73 (8.83–12.54) | 1.90 / 4.67 / 9.73 (8.32–10.50) | 1.95 / 4.54 / 9.98 (8.70–13.18) | 1.90 / 4.61 / 9.98 (9.34–14.08) | 2.72 / 5.70 / 8.45 (6.66–10.24) | 2.40 / 5.12 / 7.23 (6.59–8.83) | 2.72 / 5.82 / 8.58 (7.87–10.62) | 1.78 / 3.65 / 4.67 (4.48–6.53) | 1.70 / 2.43 / 6.40 (4.48–10.62) | 0.58 / 1.36 / 1.82 (1.74–1.82) | 50% | 22 |
| 10 | 0 | 1000 | 2 | 2.50 / 3.55 / 3.74 (3.65–8.19) | 2.50 / 3.58 / 4.74 (3.68–7.68) | 1.78 / 2.82 / 2.94 (2.91–4.74) | 1.82 / 2.82 / 3.04 (2.94–5.70) | 1.82 / 2.82 / 2.94 (2.91–4.00) | 1.15 / 1.36 / 1.49 (1.46–1.49) | 1.15 / 1.41 / 1.82 (1.66–1.87) | 1.15 / 1.36 / 1.54 (1.49–1.87) | 1.12 / 1.46 / 1.79 (1.74–1.82) | 1.15 / 1.36 / 1.49 (1.46–2.03) | 1.15 / 1.46 / 1.74 (1.66–1.79) | 100% | 0 |
| 10 | 0 | 1000 | 3 | 2.78 / 4.54 / 5.95 (5.31–8.13) | 2.82 / 4.61 / 5.95 (5.70–20.74) | 2.27 / 3.74 / 4.86 (4.22–12.42) | 2.37 / 3.78 / 4.93 (4.29–12.16) | 2.37 / 3.78 / 4.86 (4.61–15.62) | 2.78 / 4.29 / 8.96 (4.86–11.26) | 1.87 / 3.52 / 4.35 (4.10–5.82) | 1.95 / 2.14 / 2.69 (2.27–5.18) | 1.20 / 2.30 / 2.43 (2.40–2.53) | 1.95 / 2.14 / 2.24 (2.24–4.00) | 1.20 / 2.30 / 2.50 (2.43–2.53) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 3.23 / 5.06 / 8.00 (7.23–8.70) | 3.26 / 5.06 / 9.86 (8.45–32.51) | 2.56 / 4.29 / 5.44 (5.18–18.43) | 2.62 / 4.29 / 7.04 (5.31–10.11) | 2.62 / 4.29 / 7.49 (5.95–22.78) | 3.49 / 5.95 / 7.36 (7.10–10.50) | 2.00 / 3.68 / 4.42 (4.22–6.27) | 3.07 / 3.33 / 6.78 (3.74–11.78) | 1.20 / 2.37 / 3.46 (3.39–3.49) | 3.07 / 3.33 / 6.91 (3.49–9.86) | 1.20 / 2.37 / 3.46 (3.39–3.49) | 100% | 0 |
| 10 | 0 | 1000 | 6 | 4.48 / 8.00 / 12.42 (10.62–13.95) | 4.48 / 7.81 / 12.29 (11.78–18.69) | 3.68 / 7.10 / 11.26 (9.73–12.42) | 3.68 / 7.30 / 11.39 (10.62–14.08) | 3.65 / 7.23 / 12.16 (11.01–215.04) | 5.70 / 9.98 / 13.18 (12.29–14.59) | 3.52 / 6.78 / 9.73 (8.70–11.01) | 7.62 / 12.29 / 16.90 (14.85–17.66) | 2.78 / 5.89 / 7.42 (6.91–8.58) | 5.95 / 6.40 / 10.24 (8.83–16.13) | 1.41 / 3.74 / 4.22 (4.16–4.99) | 67% | 28 |
| 10 | 0 | 1000 | 8 | 4.74 / 8.83 / 13.31 (11.26–14.34) | 4.99 / 9.34 / 16.38 (15.10–20.22) | 3.90 / 8.70 / 13.44 (12.54–17.66) | 4.16 / 9.47 / 15.62 (13.82–18.69) | 4.03 / 9.22 / 15.62 (13.31–17.66) | 6.27 / 12.29 / 17.92 (14.59–19.46) | 4.06 / 9.47 / 14.21 (12.03–15.62) | 10.50 / 17.66 / 25.60 (21.25–28.16) | 2.94 / 6.53 / 9.47 (8.83–10.50) | 8.19 / 8.96 / 16.64 (15.23–19.97) | 1.41 / 4.00 / 5.25 (5.25–5.31) | 49% | 14 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 1.95 (1.90–1.95) | 1.62 / 1.87 / 5.57 (2.00–6.59) | 1.57 / 1.79 / 2.24 (1.87–5.25) | 1.57 / 1.79 / 1.95 (1.90–5.82) | 1.54 / 1.79 / 1.95 (1.90–5.70) | 0.25 / 0.37 / 0.46 (0.42–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.72 / 3.10 / 4.99 (3.52–9.34) | 2.72 / 3.10 / 6.08 (3.49–12.93) | 2.66 / 3.07 / 4.35 (3.36–8.32) | 2.66 / 3.10 / 3.46 (3.36–14.08) | 2.66 / 3.94 / 4.42 (4.16–13.70) | 1.79 / 2.21 / 2.56 (2.40–5.12) | 1.66 / 2.08 / 2.30 (2.27–5.95) | 0.25 / 0.42 / 0.54 (0.50–0.54) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.25 / 0.42 / 0.54 (0.50–0.58) | 0.25 / 0.42 / 0.54 (0.54–0.58) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.33 / 6.21 (4.00–15.49) | 2.85 / 3.49 / 6.85 (4.16–19.20) | 2.78 / 3.26 / 5.95 (4.29–8.45) | 2.78 / 3.23 / 6.40 (3.58–25.09) | 2.78 / 4.22 / 7.23 (5.38–20.22) | 1.90 / 2.69 / 3.10 (3.07–4.86) | 1.78 / 2.53 / 2.78 (2.72–2.85) | 0.25 / 0.46 / 0.58 (0.54–0.74) | 0.25 / 0.46 / 0.58 (0.58–0.87) | 0.25 / 0.46 / 0.54 (0.54–0.83) | 0.25 / 0.46 / 0.58 (0.58–0.70) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 3.68 / 5.95 / 10.75 (9.60–13.82) | 3.68 / 5.70 / 11.01 (10.24–12.54) | 3.65 / 5.82 / 9.34 (7.74–11.39) | 3.65 / 5.57 / 10.88 (10.75–11.78) | 3.78 / 7.62 / 11.52 (10.75–15.49) | 3.33 / 5.82 / 7.10 (5.89–7.42) | 3.23 / 5.44 / 6.53 (5.95–8.32) | 2.69 / 4.74 / 6.27 (5.31–7.10) | 2.37 / 4.16 / 5.44 (4.74–5.70) | 0.50 / 0.95 / 1.15 (1.12–1.20) | 0.42 / 1.04 / 1.28 (1.04–1.36) | 67% | 23 |
| 10 | 1000 | 50 | 8 | 3.68 / 6.59 / 14.98 (10.11–23.55) | 3.68 / 6.14 / 14.08 (11.26–18.18) | 3.65 / 6.53 / 14.98 (13.31–17.92) | 3.65 / 6.27 / 13.82 (11.39–16.13) | 4.06 / 9.86 / 18.94 (14.72–25.86) | 3.78 / 6.91 / 10.75 (8.06–11.78) | 4.06 / 6.78 / 9.09 (7.04–10.62) | 2.66 / 5.12 / 7.23 (6.72–10.11) | 2.40 / 4.48 / 5.44 (5.31–5.57) | 0.70 / 1.41 / 1.70 (1.57–3.26) | 0.58 / 1.20 / 1.54 (1.28–1.58) | 50% | 14 |
| 10 | 1000 | 200 | 2 | 1.90 / 2.21 / 2.30 (2.27–5.31) | 1.90 / 2.24 / 2.40 (2.37–6.46) | 1.74 / 2.08 / 4.86 (2.14–5.95) | 1.78 / 2.11 / 5.12 (2.27–7.74) | 1.74 / 2.08 / 2.94 (2.21–12.42) | 0.37 / 0.50 / 0.66 (0.62–0.66) | 0.33 / 0.54 / 0.70 (0.66–0.70) | 0.37 / 0.54 / 0.66 (0.66–0.66) | 0.33 / 0.54 / 0.70 (0.66–0.70) | 0.37 / 0.50 / 0.66 (0.62–0.66) | 0.33 / 0.50 / 0.70 (0.66–0.74) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.49 / 4.54 (3.94–5.44) | 2.98 / 3.52 / 4.29 (3.94–9.86) | 2.91 / 3.55 / 8.58 (6.02–19.46) | 2.91 / 3.52 / 5.95 (3.81–6.85) | 2.85 / 4.29 / 4.80 (4.67–13.31) | 2.08 / 2.43 / 2.78 (2.69–4.74) | 1.87 / 2.27 / 3.07 (2.56–5.50) | 0.37 / 0.58 / 0.70 (0.66–0.74) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.37 / 0.58 / 0.70 (0.70–0.74) | 0.37 / 0.62 / 0.79 (0.79–0.83) | 100% | 0 |
| 10 | 1000 | 200 | 4 | 3.10 / 3.78 / 4.74 (4.48–7.23) | 3.10 / 3.81 / 5.70 (5.31–12.42) | 2.98 / 3.65 / 6.66 (5.63–19.97) | 2.98 / 3.68 / 6.21 (4.10–16.00) | 2.98 / 4.93 / 15.23 (6.78–16.64) | 2.21 / 3.07 / 4.00 (3.74–5.12) | 2.00 / 2.72 / 3.10 (3.04–4.35) | 0.37 / 0.62 / 0.74 (0.70–1.12) | 0.42 / 0.70 / 0.87 (0.83–0.91) | 0.37 / 0.62 / 0.87 (0.70–1.00) | 0.42 / 0.74 / 1.04 (0.83–1.07) | 100% | 0 |
| 10 | 1000 | 200 | 6 | 3.87 / 5.95 / 10.88 (7.62–15.23) | 4.06 / 6.14 / 11.39 (10.24–26.62) | 3.90 / 6.08 / 10.11 (8.83–10.24) | 3.90 / 6.14 / 12.03 (10.88–12.54) | 4.03 / 8.19 / 14.08 (11.65–17.66) | 3.52 / 5.95 / 8.19 (6.78–9.86) | 3.36 / 5.57 / 6.66 (5.44–7.04) | 2.78 / 4.61 / 6.98 (5.82–11.39) | 2.53 / 4.16 / 5.12 (4.61–5.82) | 1.15 / 1.62 / 1.90 (1.79–7.36) | 0.58 / 1.36 / 1.74 (1.49–2.11) | 66% | 40 |
| 10 | 1000 | 200 | 8 | 4.03 / 6.85 / 12.67 (12.03–35.84) | 4.10 / 7.10 / 13.18 (11.65–57.34) | 3.87 / 6.53 / 13.18 (12.42–30.98) | 3.90 / 6.40 / 12.29 (11.52–13.70) | 4.42 / 10.37 / 18.94 (14.98–39.94) | 4.29 / 8.00 / 11.52 (10.24–15.87) | 4.16 / 6.91 / 9.98 (8.83–11.52) | 3.39 / 6.91 / 9.98 (8.70–12.93) | 2.56 / 4.61 / 5.57 (4.48–8.06) | 1.78 / 2.50 / 8.19 (5.70–12.42) | 0.58 / 1.41 / 1.87 (1.79–1.95) | 49% | 33 |
| 10 | 1000 | 1000 | 2 | 3.49 / 4.54 / 6.53 (5.57–8.32) | 3.49 / 4.54 / 6.78 (4.74–11.39) | 2.72 / 3.81 / 3.94 (3.90–8.83) | 2.78 / 3.81 / 6.78 (4.54–9.09) | 2.78 / 3.81 / 4.42 (3.94–7.23) | 1.15 / 1.33 / 1.46 (1.46–1.54) | 1.15 / 1.41 / 1.78 (1.70–2.08) | 1.15 / 1.33 / 1.49 (1.41–1.49) | 1.15 / 1.41 / 1.78 (1.62–1.82) | 1.15 / 1.36 / 1.49 (1.49–1.49) | 1.15 / 1.41 / 1.70 (1.66–1.78) | 100% | 0 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.53 / 9.47 (8.45–17.41) | 4.61 / 6.53 / 8.70 (7.23–11.78) | 3.90 / 5.70 / 7.49 (6.91–16.90) | 3.90 / 5.82 / 7.10 (6.21–11.01) | 3.94 / 6.78 / 8.19 (7.87–8.96) | 3.68 / 5.95 / 7.17 (6.27–15.49) | 2.85 / 4.06 / 6.21 (4.99–14.21) | 1.95 / 2.14 / 2.27 (2.24–11.90) | 1.20 / 2.30 / 2.43 (2.40–2.50) | 1.95 / 2.14 / 2.24 (2.24–2.24) | 1.20 / 2.30 / 2.43 (2.40–6.53) | 100% | 0 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.23 / 10.62 (9.73–21.76) | 4.99 / 7.04 / 8.32 (8.13–11.52) | 4.22 / 6.08 / 8.45 (7.10–23.30) | 4.29 / 6.14 / 7.23 (6.91–24.06) | 4.42 / 7.62 / 9.86 (9.34–20.74) | 3.87 / 6.53 / 8.19 (7.68–13.31) | 2.94 / 4.10 / 5.25 (5.06–5.95) | 3.07 / 3.33 / 3.46 (3.39–9.60) | 1.20 / 2.37 / 3.46 (3.39–4.29) | 3.07 / 3.33 / 6.91 (3.39–10.37) | 1.20 / 2.37 / 3.46 (3.39–3.58) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 6.27 / 9.73 / 14.72 (14.21–18.94) | 6.40 / 9.73 / 14.34 (13.31–15.10) | 5.50 / 9.22 / 14.98 (12.42–18.18) | 5.57 / 9.22 / 13.82 (12.93–17.15) | 5.70 / 11.14 / 14.34 (14.21–16.64) | 6.14 / 10.62 / 14.46 (13.31–15.36) | 4.74 / 8.06 / 10.50 (9.73–21.76) | 7.68 / 12.67 / 16.00 (14.98–18.18) | 3.52 / 5.95 / 7.94 (7.49–9.22) | 5.95 / 6.40 / 11.65 (7.94–13.44) | 1.41 / 3.74 / 4.74 (4.16–5.12) | 66% | 61 |
| 10 | 1000 | 1000 | 8 | 6.59 / 10.88 / 16.38 (14.98–16.90) | 7.04 / 11.78 / 18.18 (17.41–19.46) | 6.14 / 11.78 / 18.18 (15.87–24.83) | 6.14 / 11.39 / 20.48 (16.64–1359.87) | 6.34 / 14.21 / 19.20 (18.69–30.21) | 6.85 / 12.93 / 17.92 (16.38–19.46) | 5.44 / 9.47 / 13.70 (11.26–15.10) | 10.37 / 19.20 / 29.70 (26.62–31.74) | 3.55 / 6.66 / 9.86 (8.45–44.54) | 8.19 / 8.96 / 17.15 (15.49–20.48) | 1.41 / 4.00 / 5.31 (5.31–5.63) | 49% | 69 |

Derive (addition) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 lazy adds | 1 replica 25% | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 first | 3 fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.66 / 0.83 / 0.91 (0.87–3.62) | 0.25 / 0.83 / 0.87 (0.87–0.91) | 0.58 / 0.79 / 0.83 (0.79–5.57) | 0.25 / 0.74 / 0.79 (0.79–0.83) | 0.25 / 0.74 / 0.91 (0.79–2.91) | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.21 / 0.33 / 0.33 (0.33–0.37) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.21 / 0.33 / 0.33 (0.33–0.37) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.21 / 0.33 / 0.37 (0.33–0.42) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.79 / 1.07 / 1.33 (1.20–6.08) | 0.62 / 0.87 / 1.04 (0.95–6.08) | 0.74 / 1.04 / 1.25 (1.15–4.06) | 0.54 / 0.79 / 0.87 (0.83–1.07) | 0.54 / 0.79 / 0.95 (0.95–1.12) | 0.79 / 1.04 / 1.70 (1.25–6.46) | 0.33 / 0.83 / 0.95 (0.87–0.95) | 0.25 / 0.37 / 0.46 (0.42–0.50) | 0.25 / 0.33 / 0.46 (0.33–0.87) | 0.25 / 0.37 / 0.46 (0.42–0.66) | 0.25 / 0.33 / 0.46 (0.42–3.46) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.87 / 1.36 / 4.61 (1.33–6.59) | 0.62 / 0.91 / 3.14 (1.04–4.35) | 0.83 / 1.12 / 1.66 (1.20–5.31) | 0.58 / 0.79 / 3.68 (0.87–5.31) | 0.58 / 0.83 / 1.00 (0.87–4.48) | 0.91 / 1.28 / 1.57 (1.49–4.10) | 0.58 / 1.41 / 3.94 (2.37–4.61) | 0.25 / 0.42 / 0.46 (0.46–0.50) | 0.25 / 0.33 / 3.33 (0.46–3.39) | 0.25 / 0.42 / 0.46 (0.46–0.54) | 0.25 / 0.33 / 0.50 (0.46–0.54) | 100% | 0 |
| 1 | 0 | 50 | 6 | 1.79 / 2.78 / 8.13 (5.82–9.34) | 1.07 / 2.03 / 2.85 (2.82–7.62) | 1.74 / 2.72 / 8.19 (3.81–11.39) | 1.00 / 1.95 / 2.72 (2.50–80.90) | 1.04 / 1.90 / 2.72 (2.62–3.14) | 2.21 / 3.68 / 4.99 (4.48–5.12) | 1.28 / 2.40 / 2.98 (2.72–3.26) | 1.87 / 2.98 / 3.94 (3.04–7.81) | 0.74 / 2.24 / 3.10 (3.07–3.39) | 0.50 / 0.83 / 1.04 (0.91–1.07) | 0.42 / 0.79 / 1.00 (0.95–1.12) | 67% | 10 |
| 1 | 0 | 50 | 8 | 2.03 / 3.52 / 11.52 (8.70–1212.42) | 1.33 / 2.37 / 5.50 (4.80–11.39) | 2.00 / 3.46 / 11.26 (10.11–102.40) | 1.25 / 2.40 / 6.21 (3.23–7.94) | 1.20 / 2.43 / 6.08 (3.74–7.87) | 2.43 / 4.35 / 7.04 (5.50–15.49) | 1.49 / 2.62 / 3.65 (3.23–5.82) | 2.14 / 3.68 / 6.08 (3.49–11.01) | 0.95 / 2.69 / 3.46 (3.36–3.81) | 0.74 / 1.25 / 1.62 (1.28–3.10) | 0.54 / 0.83 / 1.04 (1.00–1.15) | 50% | 111 |
| 1 | 0 | 200 | 2 | 0.91 / 1.20 / 5.70 (1.25–9.09) | 0.37 / 1.20 / 1.28 (1.28–1.33) | 0.78 / 1.07 / 4.29 (1.20–15.49) | 0.37 / 1.07 / 1.12 (1.07–7.87) | 0.37 / 1.07 / 1.12 (1.12–3.68) | 0.37 / 0.46 / 0.70 (0.54–5.76) | 0.37 / 0.46 / 0.54 (0.54–0.74) | 0.37 / 0.46 / 0.58 (0.54–13.18) | 0.33 / 0.46 / 0.58 (0.46–0.70) | 0.37 / 0.46 / 0.62 (0.54–0.66) | 0.37 / 0.46 / 0.70 (0.62–0.79) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.49 / 8.06 (1.70–11.26) | 0.87 / 1.25 / 1.33 (1.28–1.49) | 0.95 / 1.41 / 3.23 (1.54–4.42) | 0.74 / 1.12 / 1.20 (1.15–3.87) | 0.74 / 1.12 / 1.20 (1.15–1.28) | 1.07 / 1.46 / 4.86 (1.54–6.72) | 0.46 / 1.04 / 1.20 (1.15–1.25) | 0.37 / 0.50 / 0.58 (0.54–0.62) | 0.37 / 0.46 / 0.70 (0.58–4.35) | 0.37 / 0.50 / 0.58 (0.54–0.70) | 0.37 / 0.46 / 3.26 (0.66–4.35) | 100% | 0 |
| 1 | 0 | 200 | 4 | 1.15 / 1.62 / 5.70 (1.74–8.83) | 0.91 / 1.33 / 5.44 (1.49–6.40) | 1.04 / 1.54 / 4.29 (1.70–24.58) | 0.78 / 1.12 / 3.68 (1.25–5.57) | 0.78 / 1.12 / 1.46 (1.28–7.94) | 1.20 / 1.74 / 2.03 (1.95–4.80) | 0.74 / 1.25 / 1.62 (1.41–5.63) | 0.37 / 0.54 / 0.70 (0.62–1.46) | 0.42 / 0.46 / 1.46 (0.66–3.36) | 0.37 / 0.50 / 0.62 (0.62–2.11) | 0.42 / 0.54 / 3.26 (0.62–6.27) | 100% | 0 |
| 1 | 0 | 200 | 6 | 2.08 / 3.26 / 10.11 (7.30–13.95) | 1.33 / 2.30 / 3.23 (3.04–4.80) | 1.95 / 3.07 / 9.34 (6.85–892.93) | 1.20 / 2.37 / 3.26 (2.72–3.81) | 1.20 / 2.27 / 3.33 (2.91–6.91) | 2.21 / 3.94 / 5.76 (4.42–8.70) | 1.46 / 2.50 / 3.20 (3.10–3.23) | 2.11 / 3.33 / 4.16 (3.68–7.74) | 0.87 / 2.37 / 3.10 (2.72–3.20) | 1.12 / 1.46 / 1.66 (1.54–7.62) | 0.58 / 0.95 / 1.25 (1.20–1.25) | 66% | 29 |
| 1 | 0 | 200 | 8 | 2.30 / 3.94 / 12.80 (11.01–14.08) | 1.54 / 2.85 / 8.96 (4.35–10.50) | 2.14 / 4.10 / 17.41 (15.23–55.81) | 1.41 / 2.72 / 4.48 (3.14–8.83) | 1.46 / 2.66 / 4.29 (3.36–10.37) | 2.62 / 4.48 / 7.23 (5.70–14.59) | 1.70 / 2.82 / 3.78 (3.46–4.06) | 2.40 / 4.06 / 8.32 (4.42–14.98) | 1.04 / 2.69 / 3.52 (3.10–3.87) | 1.87 / 2.37 / 10.37 (2.40–14.34) | 0.66 / 1.00 / 1.25 (1.15–1.28) | 50% | 0 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.58 / 6.59 (3.62–9.34) | 2.37 / 3.52 / 3.62 (3.62–3.65) | 1.79 / 2.82 / 3.87 (2.85–16.13) | 1.62 / 2.72 / 2.85 (2.78–3.04) | 1.20 / 2.78 / 2.85 (2.82–3.04) | 1.15 / 1.28 / 1.36 (1.33–1.41) | 1.12 / 1.25 / 2.14 (1.25–2.27) | 1.15 / 1.28 / 1.36 (1.33–4.99) | 1.15 / 1.25 / 1.66 (1.36–2.24) | 1.15 / 1.25 / 1.36 (1.33–1.49) | 1.15 / 1.25 / 2.24 (1.36–6.59) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.98 / 4.48 / 6.85 (4.80–8.83) | 2.50 / 3.62 / 6.40 (3.74–13.31) | 2.24 / 3.65 / 4.03 (4.00–25.86) | 1.74 / 2.82 / 3.36 (2.94–8.32) | 1.74 / 2.82 / 3.26 (2.94–7.81) | 2.82 / 3.81 / 7.74 (5.82–15.87) | 1.28 / 2.50 / 2.91 (2.85–6.34) | 1.95 / 2.11 / 2.14 (2.11–4.61) | 1.20 / 1.25 / 2.30 (2.21–6.21) | 1.95 / 2.11 / 2.21 (2.11–2.24) | 1.20 / 1.25 / 2.37 (2.24–15.62) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.74 / 7.62 (5.25–8.70) | 2.50 / 3.78 / 4.61 (4.00–6.91) | 2.56 / 3.94 / 7.30 (4.48–30.21) | 1.74 / 2.85 / 4.00 (3.04–5.63) | 1.74 / 2.85 / 5.82 (3.20–7.81) | 3.39 / 4.93 / 5.89 (4.99–12.80) | 1.78 / 2.72 / 3.07 (2.94–6.91) | 3.04 / 3.26 / 13.18 (3.26–19.46) | 1.20 / 1.87 / 3.46 (3.26–4.35) | 3.04 / 3.20 / 3.33 (3.26–10.88) | 1.20 / 1.87 / 4.35 (2.24–5.57) | 100% | 0 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.21 / 11.90 (9.73–18.18) | 3.14 / 4.74 / 6.21 (5.57–6.46) | 3.52 / 5.44 / 12.80 (8.58–360.45) | 2.40 / 4.10 / 6.08 (5.44–9.47) | 2.40 / 4.03 / 5.06 (4.93–6.72) | 4.93 / 7.62 / 16.00 (7.74–25.86) | 2.43 / 3.87 / 4.99 (4.42–10.37) | 7.49 / 10.62 / 17.66 (15.74–1032.19) | 1.66 / 3.46 / 4.16 (4.00–8.83) | 5.89 / 6.21 / 8.58 (6.34–13.57) | 1.36 / 2.50 / 2.85 (2.72–3.23) | 66% | 15 |
| 1 | 0 | 1000 | 8 | 4.61 / 7.04 / 15.10 (11.52–16.64) | 3.26 / 5.44 / 9.34 (6.53–12.16) | 3.81 / 6.40 / 13.06 (10.24–15.10) | 2.56 / 4.54 / 6.27 (5.82–13.82) | 2.53 / 4.61 / 6.14 (5.82–7.87) | 5.44 / 8.58 / 15.36 (10.37–30.21) | 2.66 / 4.35 / 7.17 (5.18–13.57) | 10.88 / 15.62 / 20.99 (18.69–28.42) | 2.14 / 3.78 / 4.61 (4.35–5.38) | 8.45 / 8.83 / 22.78 (14.08–34.82) | 1.49 / 2.62 / 3.04 (2.91–3.68) | 49% | 0 |
| 1 | 1000 | 50 | 2 | 1.66 / 1.82 / 1.90 (1.87–6.72) | 0.25 / 1.82 / 1.87 (1.87–4.67) | 1.57 / 1.79 / 1.82 (1.82–5.70) | 0.25 / 1.74 / 1.79 (1.79–1.82) | 0.25 / 1.74 / 1.79 (1.79–1.87) | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.21 / 0.33 / 0.37 (0.33–0.42) | 0.25 / 0.33 / 0.42 (0.37–0.46) | 0.21 / 0.33 / 0.33 (0.33–0.42) | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.25 / 0.33 / 0.42 (0.33–0.46) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.78 / 3.07 / 3.23 (3.10–23.55) | 1.62 / 1.87 / 2.00 (1.95–2.21) | 2.69 / 3.04 / 8.19 (3.23–21.25) | 1.54 / 1.79 / 1.95 (1.90–2.08) | 1.54 / 1.82 / 2.69 (2.62–17.92) | 1.79 / 2.08 / 2.30 (2.14–18.18) | 0.33 / 1.82 / 1.95 (1.87–1.95) | 0.25 / 0.37 / 0.46 (0.46–0.54) | 0.25 / 0.33 / 0.54 (0.42–0.54) | 0.25 / 0.37 / 0.46 (0.46–0.54) | 0.25 / 0.33 / 0.42 (0.37–3.39) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 3.20 / 7.68 (3.36–10.24) | 1.62 / 1.95 / 2.21 (2.11–5.63) | 2.78 / 4.80 / 8.83 (3.20–21.50) | 1.57 / 2.00 / 3.26 (2.11–13.70) | 1.54 / 2.00 / 3.04 (2.85–6.53) | 1.90 / 2.30 / 2.78 (2.56–11.90) | 0.78 / 2.03 / 2.72 (2.43–4.93) | 0.25 / 0.42 / 0.46 (0.46–0.54) | 0.25 / 0.33 / 2.53 (0.42–3.39) | 0.25 / 0.42 / 0.46 (0.46–0.70) | 0.25 / 0.58 / 3.39 (0.46–3.52) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.68 / 4.48 / 12.67 (9.34–14.34) | 2.00 / 3.07 / 4.00 (3.68–4.29) | 3.68 / 4.67 / 10.88 (6.66–12.42) | 1.90 / 3.23 / 4.10 (3.90–6.85) | 2.08 / 3.81 / 5.44 (4.74–6.27) | 3.74 / 4.93 / 5.76 (5.25–10.62) | 2.24 / 3.20 / 4.42 (3.90–9.73) | 2.72 / 3.68 / 6.21 (3.81–8.83) | 0.74 / 3.20 / 4.00 (3.81–4.35) | 0.50 / 0.87 / 1.04 (1.00–1.20) | 0.42 / 0.79 / 1.04 (0.91–1.04) | 66% | 5 |
| 1 | 1000 | 50 | 8 | 3.94 / 5.31 / 14.34 (13.06–15.87) | 2.27 / 3.65 / 4.74 (4.42–6.78) | 3.90 / 5.57 / 13.44 (12.29–51.20) | 2.14 / 3.55 / 6.21 (4.22–10.50) | 2.21 / 4.35 / 6.02 (5.50–10.75) | 4.35 / 5.70 / 10.37 (6.91–11.90) | 2.40 / 3.52 / 4.48 (4.42–5.12) | 3.07 / 4.54 / 7.74 (5.25–16.64) | 0.91 / 3.58 / 4.48 (4.42–8.45) | 0.87 / 1.28 / 6.21 (1.41–8.83) | 0.50 / 0.83 / 1.12 (1.07–1.15) | 50% | 0 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.21 / 2.27 (2.27–13.44) | 0.37 / 2.21 / 2.27 (2.24–5.82) | 1.78 / 2.08 / 6.21 (2.11–12.29) | 0.37 / 2.08 / 2.21 (2.14–11.65) | 0.37 / 2.03 / 2.11 (2.11–6.66) | 0.37 / 0.46 / 0.66 (0.58–4.74) | 0.37 / 0.46 / 5.70 (0.54–6.66) | 0.37 / 0.46 / 0.58 (0.54–13.95) | 0.37 / 0.46 / 0.46 (0.46–0.62) | 0.37 / 0.46 / 0.54 (0.50–5.57) | 0.37 / 0.46 / 0.54 (0.46–0.70) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 3.49 / 6.98 (3.68–13.70) | 1.87 / 2.24 / 2.56 (2.40–5.50) | 2.91 / 3.36 / 6.08 (3.62–10.75) | 1.74 / 2.11 / 2.27 (2.14–12.42) | 1.74 / 2.11 / 3.04 (2.56–4.74) | 2.08 / 2.43 / 3.10 (2.53–6.34) | 0.46 / 2.08 / 2.21 (2.14–4.86) | 0.37 / 0.50 / 0.58 (0.54–4.22) | 0.37 / 0.46 / 0.70 (0.58–2.00) | 0.37 / 0.50 / 0.66 (0.54–0.70) | 0.37 / 0.46 / 3.33 (0.62–3.49) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 3.10 / 3.62 / 7.62 (4.03–16.90) | 1.90 / 2.27 / 4.86 (2.56–6.85) | 2.98 / 3.49 / 7.74 (3.78–28.16) | 1.78 / 2.14 / 2.53 (2.30–7.10) | 1.74 / 2.62 / 5.63 (3.20–8.19) | 2.21 / 2.72 / 5.95 (3.04–8.58) | 0.54 / 2.21 / 2.62 (2.56–17.66) | 0.37 / 0.50 / 0.62 (0.58–7.36) | 0.42 / 0.54 / 1.28 (0.66–4.16) | 0.37 / 0.50 / 0.70 (0.62–4.80) | 0.42 / 0.50 / 1.20 (0.66–3.52) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 3.87 / 5.12 / 12.29 (11.65–25.34) | 2.24 / 3.55 / 4.80 (4.29–5.70) | 3.90 / 4.93 / 11.78 (10.50–14.08) | 2.21 / 3.62 / 7.04 (4.48–38.40) | 2.21 / 4.16 / 7.36 (5.31–12.93) | 4.10 / 5.31 / 6.78 (6.08–19.97) | 2.37 / 3.49 / 4.48 (4.35–4.61) | 3.10 / 4.16 / 5.31 (4.48–11.39) | 0.83 / 3.39 / 4.22 (4.22–4.67) | 1.07 / 1.46 / 1.62 (1.54–2.62) | 0.58 / 0.91 / 1.28 (1.25–1.41) | 66% | 7 |
| 1 | 1000 | 200 | 8 | 4.29 / 6.02 / 19.46 (13.18–24.83) | 2.53 / 4.03 / 5.44 (4.86–8.96) | 4.16 / 5.82 / 13.95 (12.67–14.34) | 2.43 / 3.90 / 5.12 (4.61–9.47) | 2.43 / 4.67 / 8.70 (5.95–11.14) | 4.48 / 6.02 / 13.70 (7.49–25.60) | 2.62 / 3.81 / 5.18 (4.29–5.50) | 3.36 / 4.93 / 7.49 (6.53–13.18) | 1.25 / 3.81 / 4.74 (4.54–4.93) | 1.90 / 2.37 / 6.14 (2.62–16.64) | 0.66 / 1.04 / 1.33 (1.28–1.41) | 50% | 0 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.54 / 10.75 (4.61–16.26) | 3.36 / 4.54 / 9.60 (4.61–15.87) | 2.78 / 3.81 / 4.00 (3.87–7.23) | 1.20 / 3.78 / 3.94 (3.87–6.91) | 2.56 / 3.81 / 11.26 (3.87–124.93) | 1.15 / 1.28 / 1.41 (1.33–1.95) | 1.15 / 1.25 / 2.14 (1.66–2.27) | 1.15 / 1.28 / 1.33 (1.33–1.41) | 1.15 / 1.25 / 5.89 (1.74–8.70) | 1.15 / 1.28 / 1.36 (1.33–14.46) | 1.15 / 1.25 / 1.82 (1.46–2.24) | 100% | 2 |
| 1 | 1000 | 1000 | 3 | 4.67 / 6.59 / 11.39 (8.06–15.62) | 3.49 / 4.61 / 8.32 (7.23–10.37) | 3.94 / 5.70 / 7.87 (5.95–14.46) | 2.72 / 3.81 / 6.78 (4.42–14.46) | 2.72 / 3.87 / 5.50 (4.61–5.95) | 3.68 / 4.99 / 8.45 (5.95–28.93) | 1.25 / 3.39 / 3.81 (3.62–4.99) | 1.95 / 2.11 / 2.14 (2.11–10.24) | 1.20 / 1.28 / 2.24 (2.14–6.98) | 1.95 / 2.11 / 4.74 (2.14–6.98) | 1.20 / 1.28 / 2.37 (2.37–4.74) | 100% | 0 |
| 1 | 1000 | 1000 | 4 | 5.12 / 6.78 / 11.90 (9.22–12.80) | 3.49 / 4.80 / 7.10 (5.31–10.37) | 4.35 / 5.95 / 6.59 (6.08–10.62) | 2.72 / 4.03 / 6.66 (5.76–10.62) | 2.78 / 3.87 / 5.95 (5.31–15.74) | 3.81 / 5.57 / 7.10 (6.08–12.29) | 2.72 / 3.78 / 6.14 (3.94–7.55) | 3.04 / 3.23 / 4.03 (3.33–12.29) | 1.20 / 2.00 / 4.29 (4.22–9.34) | 3.04 / 3.20 / 3.46 (3.33–4.61) | 1.20 / 1.46 / 2.37 (2.30–3.46) | 100% | 0 |
| 1 | 1000 | 1000 | 6 | 6.21 / 8.13 / 16.26 (11.90–1024.00) | 4.10 / 6.08 / 7.42 (6.59–8.58) | 5.44 / 7.49 / 14.85 (8.45–76.80) | 3.39 / 5.31 / 10.24 (7.23–12.42) | 3.39 / 6.08 / 8.70 (7.74–15.87) | 6.14 / 8.19 / 11.78 (9.47–21.25) | 3.46 / 4.86 / 6.14 (5.57–10.50) | 7.81 / 10.75 / 15.74 (11.78–22.02) | 1.66 / 4.35 / 4.86 (4.80–5.25) | 5.95 / 6.27 / 8.58 (7.10–11.01) | 1.36 / 2.56 / 2.98 (2.91–11.65) | 66% | 11 |
| 1 | 1000 | 1000 | 8 | 6.59 / 9.34 / 512.00 (15.74–1261.57) | 4.29 / 6.85 / 9.86 (7.62–56.83) | 5.82 / 8.45 / 16.38 (14.98–17.41) | 3.58 / 6.08 / 9.98 (9.73–12.42) | 3.58 / 6.91 / 11.26 (9.22–13.82) | 6.40 / 10.11 / 13.95 (12.93–1392.64) | 3.49 / 5.31 / 6.78 (6.27–65.54) | 10.37 / 15.87 / 20.22 (18.43–33.28) | 2.53 / 4.54 / 5.38 (5.18–5.89) | 8.32 / 8.83 / 17.92 (16.00–21.50) | 1.54 / 2.66 / 2.94 (2.91–2.98) | 49% | 67 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 1.07 (0.91–4.48) | 0.25 / 0.83 / 0.95 (0.95–0.95) | 0.58 / 0.79 / 0.87 (0.87–1.00) | 0.29 / 0.74 / 0.87 (0.87–0.91) | 0.29 / 0.74 / 0.87 (0.87–0.91) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.83 / 1.12 / 1.33 (1.25–6.40) | 0.62 / 0.95 / 1.12 (1.07–1.15) | 0.74 / 1.07 / 1.28 (1.25–1.28) | 0.54 / 0.87 / 1.07 (1.04–3.52) | 0.54 / 0.87 / 1.04 (1.04–1.07) | 0.79 / 1.12 / 1.36 (1.33–1.36) | 0.33 / 0.83 / 0.95 (0.95–1.00) | 0.25 / 0.42 / 0.54 (0.50–0.54) | 0.25 / 0.42 / 0.58 (0.54–0.58) | 0.25 / 0.42 / 0.54 (0.50–0.54) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.87 / 1.28 / 1.54 (1.49–14.72) | 0.66 / 1.07 / 1.36 (1.28–2.56) | 0.83 / 1.25 / 3.62 (1.46–5.25) | 0.58 / 1.00 / 2.24 (1.28–5.25) | 0.58 / 1.00 / 1.33 (1.15–2.66) | 0.95 / 1.58 / 1.95 (1.90–2.27) | 0.54 / 1.25 / 1.49 (1.49–1.90) | 0.25 / 0.46 / 0.54 (0.54–0.91) | 0.25 / 0.50 / 0.58 (0.58–0.79) | 0.25 / 0.50 / 0.62 (0.54–0.74) | 0.25 / 0.46 / 0.58 (0.58–0.78) | 100% | 0 |
| 10 | 0 | 50 | 6 | 1.49 / 2.98 / 6.85 (5.89–8.58) | 0.95 / 2.21 / 3.10 (3.04–3.14) | 1.46 / 2.85 / 5.82 (5.12–7.10) | 0.91 / 2.21 / 3.10 (3.04–5.76) | 0.95 / 2.40 / 3.46 (3.26–9.86) | 1.87 / 3.87 / 5.57 (3.46–10.88) | 1.33 / 3.20 / 4.16 (3.90–5.63) | 2.03 / 4.16 / 5.31 (4.22–9.22) | 0.66 / 2.78 / 3.81 (3.04–4.29) | 0.50 / 0.95 / 1.20 (1.20–3.26) | 0.42 / 0.87 / 1.12 (1.07–1.36) | 67% | 6 |
| 10 | 0 | 50 | 8 | 1.78 / 4.29 / 9.09 (8.70–12.16) | 1.12 / 3.10 / 4.48 (4.16–5.12) | 1.70 / 4.22 / 9.09 (8.32–11.90) | 1.07 / 3.04 / 4.48 (4.29–5.44) | 1.07 / 3.04 / 4.54 (4.29–4.93) | 2.21 / 4.61 / 6.66 (6.34–7.10) | 1.36 / 3.23 / 4.61 (3.87–5.82) | 1.87 / 4.00 / 6.91 (5.25–8.70) | 1.00 / 2.98 / 3.90 (3.78–4.06) | 0.62 / 1.25 / 4.29 (2.00–6.34) | 0.46 / 1.00 / 1.28 (1.20–1.54) | 50% | 16 |
| 10 | 0 | 200 | 2 | 0.91 / 1.20 / 1.33 (1.28–1.33) | 0.42 / 1.20 / 1.36 (1.33–3.04) | 0.78 / 1.07 / 1.15 (1.15–413.70) | 0.42 / 1.07 / 1.20 (1.12–8.45) | 0.42 / 1.07 / 1.25 (1.15–3.33) | 0.37 / 0.50 / 0.62 (0.62–0.66) | 0.37 / 0.54 / 0.70 (0.70–0.70) | 0.37 / 0.50 / 0.66 (0.66–0.66) | 0.37 / 0.54 / 0.70 (0.70–0.83) | 0.37 / 0.54 / 0.66 (0.66–1.36) | 0.37 / 0.54 / 0.70 (0.70–1.20) | 99% | 15 |
| 10 | 0 | 200 | 3 | 1.07 / 1.62 / 5.44 (1.78–19.71) | 0.87 / 1.33 / 1.74 (1.57–11.52) | 0.95 / 1.46 / 4.74 (1.70–18.69) | 0.74 / 1.15 / 1.46 (1.36–7.81) | 0.74 / 1.15 / 1.54 (1.46–4.35) | 1.07 / 1.49 / 1.82 (1.78–1.87) | 0.46 / 1.12 / 1.33 (1.28–1.33) | 0.37 / 0.58 / 0.70 (0.70–0.74) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.37 / 0.58 / 0.74 (0.66–0.78) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 100% | 0 |
| 10 | 0 | 200 | 4 | 1.15 / 1.74 / 4.35 (2.24–7.62) | 0.91 / 1.46 / 1.95 (1.82–8.70) | 1.04 / 1.62 / 3.58 (1.95–10.37) | 0.78 / 1.33 / 2.21 (1.62–4.86) | 0.78 / 1.33 / 1.82 (1.62–3.55) | 1.25 / 2.08 / 2.56 (2.50–2.91) | 0.78 / 1.46 / 2.14 (1.66–3.74) | 0.37 / 0.62 / 0.74 (0.70–0.78) | 0.42 / 0.70 / 0.87 (0.83–1.15) | 0.37 / 0.66 / 0.87 (0.74–1.07) | 0.42 / 0.70 / 0.83 (0.83–0.87) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.21 / 4.29 / 9.60 (8.83–14.59) | 1.28 / 2.85 / 4.22 (4.00–5.89) | 1.70 / 3.46 / 7.23 (5.57–8.70) | 1.12 / 2.66 / 3.94 (3.78–4.99) | 1.15 / 2.66 / 3.87 (3.62–4.06) | 2.30 / 4.86 / 6.40 (3.94–7.04) | 1.46 / 3.26 / 4.22 (3.36–4.54) | 1.95 / 3.65 / 5.82 (4.67–7.36) | 0.74 / 2.24 / 2.98 (2.82–4.35) | 1.00 / 1.54 / 1.87 (1.79–5.25) | 0.66 / 1.41 / 1.82 (1.74–1.87) | 66% | 29 |
| 10 | 0 | 200 | 8 | 2.03 / 4.48 / 9.73 (9.60–13.06) | 1.41 / 3.62 / 5.82 (4.86–7.17) | 1.90 / 4.74 / 9.98 (8.70–12.29) | 1.28 / 3.62 / 5.57 (4.61–6.08) | 1.28 / 3.52 / 5.63 (5.18–6.85) | 2.69 / 5.63 / 8.32 (7.04–8.83) | 1.49 / 3.36 / 4.61 (4.35–5.25) | 2.72 / 5.95 / 9.34 (8.83–13.06) | 1.25 / 2.94 / 4.29 (3.90–4.29) | 1.70 / 2.43 / 6.21 (2.78–9.47) | 0.58 / 1.36 / 1.78 (1.74–1.79) | 50% | 22 |
| 10 | 0 | 1000 | 2 | 2.50 / 3.58 / 5.57 (3.65–7.87) | 1.20 / 3.55 / 3.68 (3.68–5.44) | 1.78 / 2.82 / 2.94 (2.91–8.70) | 1.62 / 2.72 / 2.91 (2.85–4.74) | 1.62 / 2.69 / 2.85 (2.85–2.94) | 1.15 / 1.33 / 1.49 (1.46–2.78) | 1.15 / 1.41 / 1.70 (1.62–1.87) | 1.15 / 1.36 / 1.46 (1.46–1.54) | 1.15 / 1.46 / 1.79 (1.78–1.87) | 1.15 / 1.33 / 1.49 (1.46–2.00) | 1.15 / 1.41 / 1.78 (1.62–1.87) | 100% | 0 |
| 10 | 0 | 1000 | 3 | 2.78 / 4.54 / 5.82 (4.93–8.70) | 2.43 / 4.03 / 4.86 (4.61–13.82) | 2.27 / 3.74 / 4.61 (4.42–10.62) | 1.74 / 3.10 / 4.00 (3.90–4.86) | 1.74 / 3.14 / 4.06 (3.90–8.06) | 2.78 / 4.22 / 7.10 (4.80–7.94) | 1.25 / 2.40 / 2.98 (2.94–3.04) | 1.95 / 2.14 / 2.69 (2.21–6.02) | 1.20 / 2.30 / 2.43 (2.43–2.53) | 1.95 / 2.14 / 2.27 (2.24–8.00) | 1.20 / 2.30 / 2.50 (2.43–2.62) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 3.26 / 5.06 / 8.00 (6.02–9.22) | 2.50 / 4.61 / 7.23 (4.99–18.43) | 2.56 / 4.29 / 6.27 (5.25–22.02) | 1.79 / 3.65 / 4.86 (4.22–5.06) | 1.78 / 3.62 / 4.22 (4.16–5.12) | 3.49 / 5.95 / 7.23 (7.10–8.13) | 1.79 / 2.94 / 3.55 (3.39–3.81) | 3.07 / 3.33 / 8.70 (3.94–12.67) | 1.20 / 2.37 / 3.46 (3.39–3.49) | 3.07 / 3.33 / 3.90 (3.39–13.82) | 1.20 / 2.37 / 3.46 (3.39–3.49) | 100% | 0 |
| 10 | 0 | 1000 | 6 | 4.48 / 7.94 / 12.42 (10.50–15.62) | 3.23 / 6.40 / 9.73 (8.19–11.01) | 3.68 / 7.10 / 10.88 (10.37–12.80) | 2.53 / 5.82 / 8.45 (7.74–8.70) | 2.53 / 5.82 / 8.83 (8.45–9.86) | 5.70 / 9.86 / 12.80 (12.29–13.06) | 2.62 / 5.31 / 7.17 (6.78–7.36) | 7.62 / 12.42 / 16.38 (14.85–19.46) | 1.74 / 4.16 / 5.44 (4.80–5.70) | 5.95 / 6.46 / 9.47 (7.81–14.72) | 1.41 / 3.78 / 4.74 (4.29–4.99) | 67% | 28 |
| 10 | 0 | 1000 | 8 | 4.80 / 8.83 / 13.44 (12.80–14.46) | 3.49 / 7.62 / 11.14 (10.37–14.98) | 3.90 / 8.70 / 13.57 (12.67–16.26) | 2.85 / 7.87 / 11.01 (10.62–11.52) | 2.72 / 7.23 / 9.73 (9.60–12.54) | 6.27 / 12.16 / 18.69 (15.62–19.97) | 2.94 / 6.91 / 9.98 (9.60–12.93) | 10.50 / 17.92 / 25.86 (25.34–26.11) | 2.50 / 4.86 / 6.40 (6.34–7.17) | 8.19 / 8.96 / 17.15 (13.18–18.18) | 1.41 / 4.00 / 5.31 (5.25–5.31) | 49% | 14 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 1.95 (1.90–1.95) | 0.25 / 1.82 / 2.00 (1.95–4.22) | 1.57 / 1.79 / 1.90 (1.87–1.95) | 0.25 / 1.74 / 1.87 (1.87–1.95) | 0.25 / 1.74 / 1.87 (1.87–1.90) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.25 / 0.33 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.54) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.72 / 3.10 / 3.62 (3.33–9.47) | 1.62 / 2.03 / 2.24 (2.21–5.76) | 2.66 / 3.07 / 4.06 (3.36–6.40) | 1.54 / 2.00 / 2.14 (2.11–3.58) | 1.54 / 2.78 / 2.94 (2.91–11.65) | 1.79 / 2.21 / 2.40 (2.37–2.43) | 0.33 / 1.82 / 1.95 (1.90–1.95) | 0.25 / 0.42 / 0.50 (0.50–0.50) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.25 / 0.42 / 0.54 (0.50–0.66) | 0.25 / 0.42 / 0.54 (0.54–0.58) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.33 / 6.72 (3.78–19.97) | 1.62 / 2.21 / 4.67 (2.40–5.44) | 2.78 / 3.26 / 5.89 (5.70–8.96) | 1.57 / 2.11 / 3.55 (2.37–9.09) | 1.57 / 2.98 / 4.06 (4.03–9.73) | 1.90 / 2.69 / 3.07 (3.04–4.99) | 0.46 / 2.40 / 2.56 (2.56–2.66) | 0.25 / 0.46 / 0.58 (0.54–0.79) | 0.25 / 0.46 / 0.58 (0.58–0.91) | 0.25 / 0.46 / 0.58 (0.54–0.78) | 0.25 / 0.46 / 0.62 (0.54–0.66) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 3.68 / 5.82 / 11.01 (9.86–14.85) | 2.08 / 4.16 / 5.82 (5.44–7.04) | 3.65 / 5.89 / 11.14 (9.86–11.65) | 2.08 / 4.35 / 5.57 (4.80–5.95) | 2.21 / 6.08 / 8.45 (8.06–8.70) | 3.26 / 5.82 / 7.23 (6.59–7.49) | 2.08 / 4.42 / 5.44 (4.80–6.08) | 2.72 / 4.61 / 6.21 (5.06–7.23) | 0.66 / 3.49 / 4.86 (4.29–5.12) | 0.50 / 0.91 / 1.20 (1.15–1.20) | 0.46 / 1.00 / 1.28 (1.04–1.33) | 67% | 23 |
| 10 | 1000 | 50 | 8 | 3.68 / 6.53 / 14.59 (11.52–29.95) | 2.11 / 4.48 / 7.42 (6.21–11.01) | 3.65 / 6.40 / 14.08 (12.42–19.97) | 2.08 / 4.48 / 7.42 (6.08–9.73) | 2.43 / 7.94 / 12.16 (10.88–14.72) | 3.78 / 6.91 / 10.11 (8.19–13.95) | 2.50 / 4.99 / 6.53 (5.38–7.30) | 2.66 / 5.12 / 7.62 (7.04–9.47) | 0.87 / 4.10 / 5.12 (4.99–5.12) | 0.70 / 1.36 / 1.79 (1.70–2.50) | 0.58 / 1.15 / 1.54 (1.25–1.66) | 50% | 14 |
| 10 | 1000 | 200 | 2 | 1.90 / 2.21 / 2.37 (2.27–5.82) | 0.37 / 2.21 / 2.30 (2.30–2.37) | 1.78 / 2.08 / 4.93 (2.14–6.72) | 0.42 / 2.08 / 2.21 (2.14–2.69) | 0.42 / 2.03 / 2.21 (2.21–12.16) | 0.37 / 0.50 / 0.66 (0.66–0.70) | 0.37 / 0.54 / 0.70 (0.70–0.70) | 0.37 / 0.54 / 0.66 (0.62–0.70) | 0.37 / 0.50 / 0.70 (0.66–0.70) | 0.37 / 0.50 / 0.66 (0.62–0.66) | 0.37 / 0.50 / 0.66 (0.66–0.70) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.52 / 6.02 (3.81–6.78) | 1.87 / 2.40 / 2.66 (2.62–5.25) | 2.91 / 3.55 / 8.83 (3.81–16.64) | 1.74 / 2.27 / 2.53 (2.53–3.94) | 1.74 / 3.10 / 3.39 (3.36–6.72) | 2.08 / 2.43 / 4.16 (2.66–5.63) | 0.46 / 2.03 / 2.24 (2.21–2.27) | 0.37 / 0.58 / 0.70 (0.66–0.74) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.37 / 0.58 / 0.70 (0.66–0.74) | 0.37 / 0.66 / 0.79 (0.78–0.83) | 100% | 0 |
| 10 | 1000 | 200 | 4 | 3.10 / 3.78 / 4.86 (4.29–6.21) | 1.90 / 2.62 / 2.98 (2.91–7.62) | 2.98 / 3.65 / 8.13 (5.82–21.25) | 1.78 / 2.43 / 2.82 (2.72–7.74) | 1.74 / 3.46 / 6.72 (4.48–13.57) | 2.21 / 3.07 / 4.03 (3.58–5.06) | 0.74 / 2.66 / 2.91 (2.85–3.68) | 0.37 / 0.62 / 0.74 (0.70–1.04) | 0.42 / 0.70 / 0.87 (0.83–1.00) | 0.37 / 0.62 / 0.83 (0.74–0.95) | 0.42 / 0.74 / 0.95 (0.83–1.15) | 100% | 0 |
| 10 | 1000 | 200 | 6 | 3.87 / 6.02 / 10.75 (9.34–11.52) | 2.40 / 4.80 / 6.40 (6.21–8.83) | 3.90 / 6.21 / 9.60 (9.34–12.67) | 2.27 / 4.67 / 6.66 (5.95–6.91) | 2.43 / 6.40 / 9.34 (8.70–9.47) | 3.52 / 5.89 / 8.32 (7.10–8.83) | 2.21 / 4.22 / 5.38 (4.48–5.44) | 2.78 / 4.67 / 6.72 (5.44–8.13) | 0.79 / 3.68 / 4.67 (3.49–5.38) | 1.15 / 1.62 / 1.95 (1.74–7.68) | 0.58 / 1.33 / 1.74 (1.54–1.95) | 66% | 40 |
| 10 | 1000 | 200 | 8 | 4.03 / 6.91 / 13.31 (12.29–37.89) | 2.50 / 5.38 / 7.42 (6.85–25.09) | 3.90 / 6.53 / 13.31 (12.29–32.26) | 2.30 / 4.93 / 6.98 (6.66–7.81) | 2.69 / 8.45 / 12.29 (11.39–12.80) | 4.29 / 7.94 / 12.42 (11.14–14.59) | 2.62 / 5.06 / 6.59 (6.40–8.83) | 3.39 / 6.98 / 11.01 (9.34–13.18) | 1.15 / 4.16 / 5.31 (3.94–5.82) | 1.78 / 2.50 / 8.58 (4.99–11.65) | 0.58 / 1.36 / 1.82 (1.79–2.11) | 49% | 33 |
| 10 | 1000 | 1000 | 2 | 3.49 / 4.54 / 4.67 (4.61–5.44) | 1.20 / 4.48 / 4.61 (4.61–8.00) | 2.72 / 3.81 / 5.44 (4.74–8.06) | 1.78 / 3.74 / 3.90 (3.87–8.45) | 2.00 / 3.74 / 3.90 (3.90–4.03) | 1.15 / 1.33 / 1.49 (1.46–1.62) | 1.15 / 1.46 / 1.82 (1.70–2.11) | 1.15 / 1.36 / 1.46 (1.46–1.49) | 1.15 / 1.41 / 1.79 (1.70–1.82) | 1.15 / 1.36 / 1.49 (1.46–1.49) | 1.15 / 1.41 / 1.74 (1.66–1.87) | 100% | 0 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.53 / 8.58 (7.49–13.31) | 3.49 / 5.31 / 5.76 (5.76–9.47) | 3.90 / 5.70 / 8.06 (6.21–14.46) | 2.72 / 4.29 / 4.99 (4.99–9.60) | 2.72 / 4.99 / 5.95 (5.82–7.10) | 3.68 / 5.95 / 8.19 (6.46–14.21) | 1.25 / 3.14 / 4.06 (3.78–12.54) | 1.95 / 2.14 / 2.50 (2.24–11.90) | 1.20 / 2.30 / 2.43 (2.43–2.56) | 1.95 / 2.14 / 2.24 (2.24–3.26) | 1.20 / 2.30 / 2.50 (2.43–7.74) | 100% | 0 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.17 / 11.01 (9.73–22.27) | 3.49 / 5.70 / 6.66 (6.46–7.10) | 4.22 / 6.14 / 9.22 (7.17–28.16) | 2.78 / 4.80 / 5.57 (5.44–7.10) | 2.78 / 5.82 / 7.17 (7.10–9.09) | 3.90 / 6.66 / 8.19 (7.62–19.46) | 2.11 / 3.87 / 4.35 (4.29–4.74) | 3.07 / 3.33 / 3.46 (3.39–9.73) | 1.20 / 2.40 / 3.49 (3.46–4.29) | 3.07 / 3.33 / 6.91 (3.52–10.75) | 1.20 / 2.37 / 3.46 (3.46–3.49) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 6.27 / 9.73 / 15.62 (14.85–16.13) | 4.22 / 7.74 / 9.73 (9.22–11.52) | 5.50 / 9.09 / 14.59 (12.80–17.66) | 3.52 / 7.23 / 9.60 (8.96–13.95) | 3.58 / 9.09 / 12.03 (11.90–12.93) | 6.14 / 10.62 / 13.95 (13.18–18.18) | 3.36 / 6.40 / 8.19 (7.55–13.70) | 7.68 / 12.54 / 16.38 (15.74–24.32) | 1.66 / 4.54 / 5.89 (5.76–6.34) | 5.95 / 6.46 / 10.24 (8.96–11.39) | 1.41 / 3.78 / 4.80 (4.29–4.99) | 66% | 61 |
| 10 | 1000 | 1000 | 8 | 6.59 / 11.01 / 16.64 (16.26–19.46) | 4.54 / 9.34 / 12.54 (12.42–14.72) | 6.14 / 11.65 / 17.92 (16.90–27.14) | 3.94 / 8.96 / 12.03 (11.14–61.44) | 4.06 / 11.78 / 16.00 (14.34–17.41) | 6.85 / 12.80 / 19.20 (16.38–21.25) | 3.68 / 7.74 / 10.88 (9.60–12.93) | 10.37 / 18.69 / 26.37 (24.58–32.26) | 2.78 / 5.57 / 7.30 (7.23–32.51) | 8.19 / 8.96 / 17.15 (16.64–22.02) | 1.41 / 4.00 / 5.25 (5.25–5.31) | 49% | 69 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 17.89; 2/4 fair: 33.23; 3 fair: 55.25;

Migrations seen: 200067 over 1175780476 samples (one sample per measured operation).


### Suite a2f: IPC call and reply, interrupt switch

load average before: { 6.01 5.33 5.46 }
load average after: { 5.55 5.34 5.44 }; 108 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 321k (321k–322k) | 322k (319k–323k) | 314k (313k–315k) | 315k (314k–315k) | 314k (314k–316k) | 100% | 0 |
| 0 | 50 | 4 | 640k (639k–642k) | 642k (642k–644k) | 624k (623k–627k) | 600k (599k–602k) | 602k (597k–604k) | 100% | 0 |
| 0 | 50 | 6 | 905k (896k–925k) | 913k (901k–937k) | 867k (841k–877k) | 789k (770k–838k) | 775k (739k–783k) | 66% | 12 |
| 0 | 50 | 8 | 1.22M (1.17M–1.25M) | 1.23M (1.22M–1.24M) | 1.15M (1.12M–1.20M) | 1.02M (910k–1.04M) | 927k (894k–954k) | 49% | 9 |
| 0 | 200 | 2 | 286k (286k–287k) | 286k (286k–287k) | 282k (281k–282k) | 282k (281k–282k) | 282k (281k–282k) | 100% | 0 |
| 0 | 200 | 4 | 573k (571k–574k) | 572k (570k–573k) | 560k (559k–562k) | 540k (536k–542k) | 536k (533k–542k) | 100% | 0 |
| 0 | 200 | 6 | 824k (810k–853k) | 818k (805k–829k) | 786k (768k–799k) | 695k (670k–710k) | 636k (632k–695k) | 67% | 5 |
| 0 | 200 | 8 | 1.08M (1.07M–1.11M) | 1.10M (1.10M–1.11M) | 1.06M (1.03M–1.06M) | 854k (826k–866k) | 750k (672k–785k) | 50% | 2 |
| 0 | 1000 | 2 | 201k (200k–201k) | 201k (200k–201k) | 198k (198k–199k) | 198k (198k–199k) | 198k (195k–198k) | 100% | 0 |
| 0 | 1000 | 4 | 400k (399k–401k) | 400k (399k–401k) | 396k (394k–396k) | 381k (381k–385k) | 383k (382k–385k) | 100% | 0 |
| 0 | 1000 | 6 | 581k (580k–593k) | 581k (579k–591k) | 561k (554k–583k) | 525k (488k–526k) | 336k (333k–343k) | 67% | 5 |
| 0 | 1000 | 8 | 787k (771k–789k) | 788k (768k–795k) | 758k (740k–767k) | 655k (604k–711k) | 338k (331k–345k) | 49% | 21 |
| 1000 | 50 | 2 | 195k (195k–195k) | 196k (195k–197k) | 191k (189k–191k) | 191k (191k–191k) | 191k (191k–191k) | 100% | 0 |
| 1000 | 50 | 4 | 390k (388k–391k) | 392k (391k–393k) | 380k (379k–381k) | 373k (372k–374k) | 373k (372k–373k) | 100% | 0 |
| 1000 | 50 | 6 | 568k (565k–589k) | 570k (565k–584k) | 539k (535k–546k) | 525k (514k–544k) | 503k (500k–517k) | 67% | 8 |
| 1000 | 50 | 8 | 765k (746k–766k) | 762k (760k–774k) | 720k (698k–742k) | 688k (674k–716k) | 684k (661k–690k) | 50% | 3 |
| 1000 | 200 | 2 | 182k (182k–182k) | 182k (182k–183k) | 179k (178k–179k) | 179k (178k–179k) | 179k (178k–179k) | 100% | 0 |
| 1000 | 200 | 4 | 363k (362k–363k) | 365k (363k–365k) | 355k (354k–357k) | 347k (346k–348k) | 346k (344k–347k) | 100% | 0 |
| 1000 | 200 | 6 | 532k (526k–533k) | 527k (525k–533k) | 502k (500k–519k) | 487k (478k–494k) | 474k (468k–484k) | 67% | 7 |
| 1000 | 200 | 8 | 712k (695k–733k) | 709k (705k–712k) | 673k (660k–716k) | 640k (630k–651k) | 613k (611k–627k) | 50% | 5 |
| 1000 | 1000 | 2 | 143k (142k–143k) | 143k (139k–143k) | 141k (140k–141k) | 141k (140k–141k) | 141k (141k–141k) | 100% | 2 |
| 1000 | 1000 | 4 | 285k (284k–285k) | 286k (284k–286k) | 280k (280k–281k) | 275k (273k–275k) | 274k (274k–275k) | 100% | 0 |
| 1000 | 1000 | 6 | 415k (414k–419k) | 418k (414k–424k) | 401k (395k–406k) | 388k (383k–396k) | 337k (332k–349k) | 66% | 6 |
| 1000 | 1000 | 8 | 559k (545k–565k) | 564k (559k–569k) | 537k (527k–544k) | 510k (507k–517k) | 333k (331k–342k) | 50% | 2 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 1.82 / 1.90 (1.90–3.55) | 1.74 / 1.82 / 4.42 (1.90–4.86) | 1.82 / 1.90 / 2.03 (2.03–6.21) | 1.82 / 1.90 / 2.03 (2.00–6.53) | 1.82 / 1.90 / 5.31 (2.03–7.36) | 100% | 0 |
| 0 | 50 | 4 | 1.74 / 1.82 / 2.08 (1.90–5.70) | 1.74 / 1.87 / 2.21 (1.90–2.40) | 1.82 / 1.90 / 2.72 (2.03–3.33) | 1.90 / 2.30 / 2.40 (2.40–4.48) | 1.90 / 2.27 / 2.94 (2.85–4.22) | 100% | 0 |
| 0 | 50 | 6 | 2.00 / 2.37 / 2.72 (2.53–5.44) | 1.90 / 2.37 / 2.53 (2.53–3.68) | 1.87 / 2.72 / 2.91 (2.82–5.31) | 2.50 / 3.46 / 3.94 (3.65–8.45) | 2.53 / 3.23 / 3.90 (3.65–7.10) | 66% | 12 |
| 0 | 50 | 8 | 1.87 / 2.37 / 2.69 (2.56–7.74) | 1.90 / 2.30 / 2.62 (2.56–5.31) | 2.21 / 2.72 / 2.85 (2.82–8.32) | 2.56 / 3.49 / 6.85 (4.03–7.74) | 3.04 / 3.81 / 8.45 (4.74–9.86) | 49% | 9 |
| 0 | 200 | 2 | 2.11 / 2.24 / 3.33 (2.27–5.70) | 2.11 / 2.21 / 3.90 (2.27–5.82) | 2.21 / 2.27 / 2.40 (2.37–5.70) | 2.14 / 2.27 / 3.14 (2.37–6.08) | 2.14 / 2.27 / 2.37 (2.37–5.82) | 100% | 0 |
| 0 | 200 | 4 | 2.11 / 2.21 / 2.30 (2.27–6.40) | 2.11 / 2.24 / 3.39 (2.62–6.85) | 2.21 / 2.50 / 3.04 (2.37–7.62) | 2.30 / 2.85 / 3.04 (2.98–7.10) | 2.30 / 2.91 / 3.68 (2.94–8.00) | 100% | 0 |
| 0 | 200 | 6 | 2.27 / 2.69 / 2.85 (2.82–5.76) | 2.37 / 2.72 / 2.85 (2.85–4.00) | 2.27 / 2.98 / 3.94 (3.10–4.61) | 2.94 / 3.94 / 4.29 (4.10–7.04) | 3.36 / 4.16 / 7.23 (4.61–9.34) | 67% | 5 |
| 0 | 200 | 8 | 2.43 / 2.72 / 5.95 (2.94–7.23) | 2.21 / 2.72 / 3.74 (2.94–7.49) | 2.50 / 3.04 / 3.74 (3.20–7.94) | 3.26 / 4.22 / 9.73 (5.63–10.62) | 3.68 / 5.50 / 12.42 (10.11–13.18) | 50% | 2 |
| 0 | 1000 | 2 | 3.62 / 3.68 / 8.19 (3.78–8.96) | 3.62 / 3.68 / 3.81 (3.78–9.47) | 3.65 / 3.78 / 3.94 (3.90–8.45) | 3.65 / 3.78 / 8.00 (3.90–8.32) | 3.65 / 3.78 / 6.91 (3.90–13.95) | 100% | 0 |
| 0 | 1000 | 4 | 3.62 / 3.68 / 6.78 (3.78–8.83) | 3.62 / 3.81 / 7.94 (4.16–9.09) | 3.65 / 3.78 / 6.08 (3.87–9.60) | 3.81 / 5.06 / 8.19 (5.25–11.78) | 3.81 / 5.06 / 8.70 (5.18–11.65) | 100% | 0 |
| 0 | 1000 | 6 | 3.81 / 4.22 / 8.19 (4.80–10.88) | 3.74 / 4.22 / 5.06 (4.74–9.60) | 4.10 / 4.61 / 5.63 (4.93–10.88) | 4.42 / 6.14 / 7.10 (6.21–12.93) | 7.62 / 8.06 / 15.87 (15.36–16.90) | 67% | 5 |
| 0 | 1000 | 8 | 3.74 / 4.22 / 8.58 (6.08–11.52) | 3.65 / 4.22 / 9.34 (5.44–10.62) | 4.03 / 4.61 / 10.24 (8.83–16.38) | 4.54 / 6.40 / 12.03 (10.11–17.92) | 10.50 / 11.14 / 23.30 (20.48–131.07) | 49% | 21 |
| 1000 | 50 | 2 | 3.74 / 3.87 / 7.74 (4.03–8.58) | 3.74 / 3.87 / 4.03 (3.94–9.47) | 3.87 / 4.00 / 7.87 (4.10–14.21) | 3.87 / 4.00 / 8.70 (4.10–9.98) | 3.87 / 4.00 / 4.16 (4.10–8.96) | 100% | 0 |
| 1000 | 50 | 4 | 3.74 / 3.87 / 7.04 (4.35–10.11) | 3.74 / 3.81 / 6.08 (3.90–10.50) | 3.87 / 4.00 / 7.49 (4.06–8.83) | 4.00 / 4.16 / 5.12 (4.99–9.22) | 4.00 / 4.16 / 7.81 (4.22–9.98) | 100% | 0 |
| 1000 | 50 | 6 | 3.87 / 4.35 / 9.09 (4.48–10.24) | 3.81 / 4.42 / 6.98 (5.57–9.34) | 4.29 / 4.86 / 5.76 (5.25–10.24) | 4.48 / 5.06 / 8.06 (5.38–10.88) | 4.61 / 4.99 / 10.37 (6.40–11.90) | 67% | 8 |
| 1000 | 50 | 8 | 3.90 / 4.42 / 6.91 (5.76–11.78) | 3.81 / 4.42 / 9.09 (6.14–11.39) | 4.29 / 4.86 / 9.34 (7.23–11.01) | 4.48 / 5.06 / 10.11 (7.23–10.62) | 4.48 / 5.12 / 12.16 (9.73–14.34) | 50% | 3 |
| 1000 | 200 | 2 | 4.10 / 4.22 / 4.48 (4.29–9.34) | 4.10 / 4.22 / 7.87 (7.74–9.34) | 4.22 / 4.42 / 9.22 (8.19–10.88) | 4.22 / 4.42 / 5.76 (4.48–10.11) | 4.22 / 4.42 / 6.91 (4.48–10.11) | 100% | 0 |
| 1000 | 200 | 4 | 4.10 / 4.22 / 7.74 (4.42–11.65) | 4.10 / 4.22 / 5.44 (4.29–11.26) | 4.22 / 4.42 / 8.83 (4.48–11.39) | 4.42 / 4.48 / 5.95 (4.74–11.26) | 4.42 / 4.54 / 7.17 (6.59–13.06) | 100% | 0 |
| 1000 | 200 | 6 | 4.22 / 4.74 / 9.73 (5.31–10.62) | 4.35 / 4.74 / 7.55 (5.31–10.50) | 4.74 / 5.25 / 9.22 (5.38–10.24) | 4.93 / 5.38 / 10.37 (5.82–11.39) | 4.99 / 5.38 / 10.88 (6.78–12.93) | 67% | 7 |
| 1000 | 200 | 8 | 4.22 / 4.74 / 8.70 (6.98–11.26) | 4.29 / 4.74 / 10.37 (8.70–11.26) | 4.74 / 5.12 / 10.11 (7.23–11.78) | 4.99 / 5.31 / 11.26 (11.01–13.44) | 5.12 / 5.95 / 13.44 (12.80–14.08) | 50% | 5 |
| 1000 | 1000 | 2 | 5.63 / 5.70 / 6.53 (5.82–12.54) | 5.57 / 5.70 / 10.37 (5.82–32.26) | 5.70 / 5.89 / 9.09 (6.08–12.67) | 5.70 / 5.89 / 10.62 (7.10–12.03) | 5.70 / 5.89 / 10.88 (10.62–11.52) | 100% | 2 |
| 1000 | 1000 | 4 | 5.63 / 5.70 / 10.37 (6.91–11.52) | 5.57 / 5.70 / 10.37 (5.82–15.49) | 5.70 / 5.89 / 11.14 (8.83–13.82) | 5.89 / 6.08 / 9.60 (8.32–13.70) | 5.89 / 6.08 / 11.01 (9.34–14.85) | 100% | 0 |
| 1000 | 1000 | 6 | 5.89 / 6.27 / 10.50 (7.10–13.44) | 5.82 / 6.27 / 11.65 (10.75–13.82) | 6.21 / 6.78 / 11.65 (7.17–14.34) | 6.53 / 6.91 / 13.06 (10.50–13.95) | 7.62 / 8.06 / 14.59 (12.93–16.00) | 66% | 6 |
| 1000 | 1000 | 8 | 5.82 / 6.27 / 12.67 (11.78–15.23) | 5.70 / 6.21 / 12.54 (12.42–12.67) | 6.27 / 6.78 / 13.31 (12.67–14.34) | 6.53 / 7.04 / 14.21 (13.18–16.00) | 10.62 / 11.26 / 22.02 (20.48–22.02) | 50% | 2 |

Migrations seen: 23789 over 46449608 samples (one sample per measured operation).


### Suite a3f: k-1 threads signal one notify, interrupt switch

load average before: { 5.55 5.34 5.44 }
load average after: { 5.46 5.37 5.44 }; 109 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 710k (701k–714k) | 709k (703k–710k) | 667k (666k–667k) | 667k (664k–669k) | 666k (662k–668k) | 100% | 0 |
| 0 | 50 | 4 | 2.10M (2.09M–2.11M) | 2.10M (2.10M–2.11M) | 1.89M (1.87M–1.90M) | 1.97M (1.96M–1.97M) | 1.97M (1.93M–1.97M) | 100% | 0 |
| 0 | 50 | 6 | 3.41M (3.40M–3.42M) | 3.42M (3.39M–3.43M) | 2.78M (2.76M–2.95M) | 2.40M (2.37M–2.43M) | 2.44M (2.36M–2.53M) | 66% | 7 |
| 0 | 50 | 8 | 4.75M (4.71M–4.77M) | 4.76M (4.74M–4.77M) | 3.37M (2.99M–4.03M) | 2.76M (2.50M–2.91M) | 2.66M (2.56M–2.85M) | 50% | 38 |
| 0 | 200 | 2 | 662k (658k–663k) | 660k (655k–664k) | 580k (575k–581k) | 580k (576k–581k) | 577k (575k–578k) | 100% | 0 |
| 0 | 200 | 4 | 1.96M (1.95M–1.97M) | 1.96M (1.95M–1.96M) | 1.67M (1.66M–1.68M) | 1.61M (1.57M–1.62M) | 1.61M (1.60M–1.62M) | 100% | 0 |
| 0 | 200 | 6 | 3.20M (3.19M–3.21M) | 3.19M (3.18M–3.21M) | 2.31M (2.22M–2.36M) | 1.76M (1.60M–1.79M) | 1.54M (1.53M–1.59M) | 66% | 3 |
| 0 | 200 | 8 | 4.47M (4.45M–4.50M) | 4.49M (4.48M–4.50M) | 3.05M (2.15M–3.20M) | 1.88M (1.59M–2.10M) | 1.56M (1.50M–1.67M) | 49% | 117 |
| 0 | 1000 | 2 | 529k (527k–531k) | 528k (524k–531k) | 402k (400k–403k) | 402k (400k–403k) | 402k (400k–403k) | 100% | 0 |
| 0 | 1000 | 4 | 1.57M (1.57M–1.58M) | 1.57M (1.56M–1.58M) | 1.08M (1.08M–1.09M) | 671k (664k–676k) | 675k (672k–676k) | 100% | 0 |
| 0 | 1000 | 6 | 1.88M (1.86M–1.88M) | 1.88M (1.87M–1.89M) | 1.23M (1.10M–1.29M) | 1.03M (985k–1.06M) | 653k (649k–677k) | 67% | 10 |
| 0 | 1000 | 8 | 1.87M (1.86M–1.87M) | 1.87M (1.86M–1.88M) | 1.25M (1.13M–1.38M) | 1.04M (1.01M–1.16M) | 685k (671k–706k) | 49% | 4 |
| 1000 | 50 | 2 | 709k (706k–711k) | 707k (707k–709k) | 667k (665k–667k) | 665k (664k–669k) | 665k (664k–667k) | 100% | 0 |
| 1000 | 50 | 4 | 2.10M (2.09M–2.12M) | 958k (957k–958k) | 1.46M (1.45M–1.46M) | 1.98M (1.97M–1.99M) | 1.97M (1.96M–1.98M) | 100% | 1487 |
| 1000 | 50 | 6 | 3.39M (3.34M–3.40M) | 940k (939k–943k) | 2.41M (2.24M–2.53M) | 1.74M (1.73M–1.77M) | 2.84M (2.82M–2.96M) | 67% | 1482 |
| 1000 | 50 | 8 | 4.76M (4.69M–4.78M) | 935k (933k–936k) | 3.30M (2.34M–3.79M) | 2.68M (2.21M–2.69M) | 2.76M (2.76M–3.03M) | 49% | 1438 |
| 1000 | 200 | 2 | 662k (655k–664k) | 661k (656k–662k) | 578k (572k–578k) | 577k (574k–579k) | 578k (576k–582k) | 100% | 0 |
| 1000 | 200 | 4 | 1.96M (1.95M–1.96M) | 868k (864k–868k) | 1.38M (1.37M–1.38M) | 1.66M (1.65M–1.66M) | 1.65M (1.64M–1.66M) | 100% | 1232 |
| 1000 | 200 | 6 | 3.19M (3.18M–3.20M) | 857k (849k–865k) | 2.10M (1.85M–2.33M) | 1.59M (1.56M–1.61M) | 1.73M (1.68M–1.77M) | 66% | 1238 |
| 1000 | 200 | 8 | 4.46M (4.39M–4.49M) | 851k (848k–854k) | 2.30M (2.13M–3.14M) | 1.92M (1.68M–2.15M) | 1.65M (1.57M–1.74M) | 49% | 1246 |
| 1000 | 1000 | 2 | 528k (526k–530k) | 530k (527k–530k) | 385k (378k–393k) | 385k (382k–397k) | 385k (369k–395k) | 100% | 0 |
| 1000 | 1000 | 4 | 1.57M (1.56M–1.58M) | 655k (654k–655k) | 1.07M (1.05M–1.09M) | 727k (724k–730k) | 727k (725k–730k) | 100% | 748 |
| 1000 | 1000 | 6 | 1.88M (1.87M–1.89M) | 649k (640k–649k) | 1.12M (1.11M–1.34M) | 1000k (976k–1.08M) | 672k (667k–692k) | 66% | 725 |
| 1000 | 1000 | 8 | 1.87M (1.86M–1.88M) | 645k (643k–646k) | 1.18M (1.02M–1.35M) | 1.11M (1.07M–1.18M) | 696k (676k–711k) | 49% | 674 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 | 2/4 | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.17 / 0.21 / 0.29 (0.25–0.29) | 0.17 / 0.21 / 0.25 (0.25–0.29) | 0.17 / 0.25 / 0.29 (0.29–0.58) | 100% | 0 |
| 0 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.29) | 0.08 / 0.12 / 0.17 (0.17–0.29) | 0.21 / 0.33 / 0.42 (0.37–0.58) | 0.17 / 0.33 / 0.42 (0.37–0.70) | 0.17 / 0.37 / 0.74 (0.42–0.74) | 100% | 0 |
| 0 | 50 | 6 | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.12 / 0.25 / 0.29 (0.29–0.29) | 0.29 / 0.58 / 0.70 (0.66–0.70) | 0.42 / 0.91 / 1.07 (1.07–1.12) | 0.70 / 1.15 / 1.54 (1.41–2.62) | 66% | 7 |
| 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.42) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.33 / 0.58 / 58.37 (0.62–67.58) | 0.42 / 1.07 / 1.41 (1.33–1.58) | 1.28 / 2.03 / 6.66 (2.85–8.58) | 50% | 38 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.42 / 0.46 / 0.50 (0.50–0.50) | 0.42 / 0.46 / 0.50 (0.50–0.50) | 0.42 / 0.46 / 0.50 (0.50–0.50) | 100% | 0 |
| 0 | 200 | 4 | 0.21 / 0.25 / 0.37 (0.25–0.42) | 0.21 / 0.25 / 0.33 (0.25–0.37) | 0.33 / 0.62 / 0.66 (0.66–0.79) | 0.50 / 0.66 / 0.74 (0.70–1.87) | 0.50 / 0.66 / 1.20 (0.70–2.94) | 100% | 0 |
| 0 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.42 / 0.87 / 15.49 (13.95–18.94) | 0.54 / 1.66 / 1.87 (1.87–2.14) | 1.87 / 2.62 / 5.57 (2.82–7.87) | 66% | 3 |
| 0 | 200 | 8 | 0.25 / 0.42 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.46 / 0.91 / 3.94 (1.07–9.47) | 0.79 / 1.46 / 2.08 (1.57–4.48) | 3.14 / 4.16 / 11.26 (8.70–12.67) | 49% | 117 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.62) | 0.58 / 0.62 / 0.62 (0.62–0.66) | 1.15 / 1.20 / 1.25 (1.25–1.28) | 1.15 / 1.25 / 1.28 (1.25–1.28) | 1.15 / 1.20 / 1.25 (1.25–1.28) | 100% | 0 |
| 0 | 1000 | 4 | 0.58 / 0.62 / 0.70 (0.62–0.74) | 0.58 / 0.62 / 0.70 (0.62–0.79) | 0.66 / 1.70 / 1.74 (1.74–1.79) | 3.23 / 3.52 / 11.78 (3.62–12.16) | 3.20 / 3.49 / 7.17 (3.58–10.11) | 100% | 0 |
| 0 | 1000 | 6 | 0.66 / 0.79 / 145.41 (145.41–147.46) | 0.62 / 0.79 / 145.41 (143.36–145.41) | 2.00 / 2.78 / 5.63 (3.62–7.36) | 0.87 / 3.74 / 4.48 (4.29–4.61) | 6.27 / 7.23 / 14.72 (13.18–15.87) | 67% | 10 |
| 0 | 1000 | 8 | 0.66 / 0.79 / 348.16 (344.06–348.16) | 0.62 / 0.79 / 344.06 (344.06–344.06) | 2.03 / 2.82 / 4.48 (2.27–7.30) | 2.56 / 3.07 / 9.73 (6.91–10.50) | 8.83 / 10.11 / 19.46 (17.66–22.27) | 49% | 4 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 0.17 / 0.21 / 0.25 (0.25–0.29) | 0.17 / 0.21 / 0.29 (0.25–0.29) | 100% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.21) | 0.08 / 0.12 / 843.78 (827.39–851.97) | 0.21 / 0.33 / 0.37 (0.37–0.70) | 0.17 / 0.37 / 0.46 (0.42–0.74) | 0.17 / 0.42 / 0.74 (0.46–0.79) | 100% | 1487 |
| 1000 | 50 | 6 | 0.17 / 0.29 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 1835.01 (1785.86–1851.39) | 0.37 / 0.66 / 0.87 (0.74–38.91) | 0.33 / 0.74 / 1.00 (1.00–1.04) | 0.42 / 0.91 / 1.15 (0.91–1.28) | 67% | 1482 |
| 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.33–0.46) | 0.12 / 0.25 / 3047.42 (2949.12–3047.42) | 0.33 / 0.62 / 12.54 (0.70–65.54) | 0.42 / 0.87 / 1.20 (1.20–4.86) | 1.15 / 2.08 / 2.69 (2.40–11.14) | 49% | 1438 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.42 / 0.50 / 0.54 (0.54–0.62) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.25 / 0.33 (0.29–0.37) | 0.17 / 0.25 / 1032.19 (1024.00–1040.38) | 0.29 / 0.58 / 0.66 (0.66–0.66) | 0.42 / 0.70 / 0.74 (0.74–1.04) | 0.42 / 0.70 / 0.83 (0.74–1.15) | 100% | 1232 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.21 / 0.33 / 2195.46 (2162.69–2228.22) | 0.54 / 1.25 / 8.06 (1.41–20.74) | 0.50 / 1.41 / 1.82 (1.79–2.03) | 1.54 / 2.24 / 5.06 (2.94–5.44) | 66% | 1238 |
| 1000 | 200 | 8 | 0.29 / 0.42 / 0.50 (0.46–0.54) | 0.21 / 0.33 / 3604.48 (3538.94–3768.32) | 0.62 / 1.25 / 14.46 (1.95–16.13) | 0.74 / 1.46 / 2.00 (1.78–5.57) | 2.85 / 3.74 / 8.96 (8.19–12.67) | 49% | 1246 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.62) | 0.58 / 0.62 / 0.62 (0.62–0.62) | 1.15 / 1.62 / 1.66 (1.66–1.70) | 1.15 / 1.62 / 1.66 (1.66–1.66) | 1.15 / 1.58 / 1.66 (1.62–1.66) | 100% | 0 |
| 1000 | 1000 | 4 | 0.58 / 0.62 / 0.74 (0.62–0.78) | 0.54 / 0.62 / 1785.86 (1785.86–1802.24) | 0.66 / 1.70 / 1.78 (1.74–1.82) | 2.72 / 2.94 / 7.42 (2.98–9.60) | 2.72 / 2.94 / 6.85 (2.98–8.06) | 100% | 748 |
| 1000 | 1000 | 6 | 0.66 / 0.79 / 147.46 (145.41–147.46) | 0.58 / 0.74 / 3768.32 (3735.55–3801.09) | 2.53 / 2.91 / 6.78 (3.65–8.45) | 0.83 / 3.46 / 4.42 (4.29–4.48) | 6.14 / 6.66 / 12.16 (10.62–14.85) | 66% | 725 |
| 1000 | 1000 | 8 | 0.62 / 0.78 / 344.06 (344.06–348.16) | 0.58 / 0.74 / 6094.85 (5898.24–6225.92) | 2.11 / 2.85 / 6.21 (4.00–7.55) | 2.30 / 2.94 / 7.30 (4.61–9.60) | 8.70 / 9.47 / 19.20 (17.15–22.02) | 49% | 674 |

Migrations seen: 38360 over 148961726 samples (one sample per measured operation).


### Suite bf: unmap, first and fair settings

load average before: { 5.46 5.37 5.44 }
load average after: { 5.12 5.16 5.30 }; 449 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 first | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 468k (467k–503k) | 486k (484k–514k) | 484k (483k–514k) | 493k (491k–493k) | 493k (490k–494k) | 494k (493k–494k) | 493k (493k–494k) | 493k (491k–494k) | 100% | 0 |
| one | 0 | 50 | 3 | 453k (452k–474k) | 471k (470k–487k) | 471k (469k–488k) | 455k (453k–464k) | 467k (466k–468k) | 482k (480k–484k) | 482k (481k–483k) | 484k (482k–485k) | 100% | 0 |
| one | 0 | 50 | 4 | 447k (445k–450k) | 462k (457k–476k) | 460k (456k–462k) | 378k (366k–385k) | 397k (383k–398k) | 469k (466k–470k) | 468k (466k–469k) | 469k (468k–470k) | 100% | 0 |
| one | 0 | 50 | 6 | 370k (369k–373k) | 376k (372k–380k) | 381k (377k–384k) | 289k (286k–301k) | 299k (298k–303k) | 319k (305k–334k) | 322k (313k–330k) | 415k (402k–415k) | 68% | 12 |
| one | 0 | 50 | 8 | 349k (345k–353k) | 356k (353k–359k) | 352k (352k–357k) | 274k (262k–284k) | 280k (270k–290k) | 247k (243k–270k) | 249k (243k–255k) | 392k (391k–396k) | 50% | 2 |
| one | 0 | 200 | 2 | 441k (441k–442k) | 468k (452k–468k) | 468k (467k–468k) | 467k (466k–467k) | 467k (465k–468k) | 467k (463k–468k) | 467k (462k–468k) | 467k (462k–467k) | 100% | 0 |
| one | 0 | 200 | 3 | 410k (408k–412k) | 437k (431k–439k) | 434k (432k–439k) | 427k (399k–431k) | 444k (428k–447k) | 454k (450k–454k) | 453k (451k–454k) | 453k (453k–454k) | 100% | 0 |
| one | 0 | 200 | 4 | 400k (395k–409k) | 429k (415k–434k) | 418k (418k–429k) | 347k (343k–347k) | 360k (359k–367k) | 441k (440k–443k) | 443k (440k–444k) | 442k (441k–443k) | 100% | 0 |
| one | 0 | 200 | 6 | 341k (338k–341k) | 352k (343k–362k) | 355k (354k–358k) | 275k (265k–279k) | 284k (277k–291k) | 286k (278k–293k) | 302k (300k–304k) | 393k (391k–396k) | 67% | 18 |
| one | 0 | 200 | 8 | 322k (318k–324k) | 331k (328k–334k) | 336k (331k–340k) | 249k (239k–262k) | 260k (255k–276k) | 231k (228k–232k) | 244k (234k–246k) | 374k (370k–377k) | 50% | 7 |
| one | 0 | 1000 | 2 | 253k (252k–255k) | 313k (312k–322k) | 313k (313k–315k) | 342k (340k–342k) | 341k (335k–342k) | 342k (341k–342k) | 341k (334k–342k) | 341k (341k–342k) | 100% | 0 |
| one | 0 | 1000 | 3 | 247k (247k–256k) | 308k (304k–309k) | 304k (297k–317k) | 251k (250k–251k) | 306k (306k–314k) | 333k (326k–334k) | 333k (332k–334k) | 334k (332k–334k) | 100% | 0 |
| one | 0 | 1000 | 4 | 246k (246k–246k) | 298k (286k–299k) | 301k (299k–302k) | 221k (219k–222k) | 265k (261k–267k) | 325k (323k–326k) | 325k (324k–327k) | 326k (324k–327k) | 99% | 0 |
| one | 0 | 1000 | 6 | 215k (210k–216k) | 261k (248k–262k) | 262k (255k–265k) | 188k (185k–189k) | 218k (211k–223k) | 199k (192k–200k) | 231k (228k–233k) | 299k (294k–301k) | 67% | 21 |
| one | 0 | 1000 | 8 | 210k (210k–212k) | 253k (251k–255k) | 251k (246k–251k) | 178k (169k–180k) | 204k (195k–205k) | 169k (168k–170k) | 195k (191k–195k) | 287k (286k–290k) | 50% | 4 |
| one | 1000 | 50 | 2 | 317k (317k–324k) | 326k (325k–331k) | 327k (325k–332k) | 330k (329k–330k) | 330k (329k–330k) | 330k (329k–330k) | 330k (330k–330k) | 330k (330k–330k) | 100% | 0 |
| one | 1000 | 50 | 3 | 314k (314k–316k) | 319k (318k–323k) | 322k (321k–323k) | 316k (314k–317k) | 321k (317k–322k) | 325k (324k–325k) | 325k (324k–325k) | 325k (324k–325k) | 100% | 0 |
| one | 1000 | 50 | 4 | 305k (302k–307k) | 313k (310k–320k) | 311k (302k–312k) | 218k (216k–222k) | 222k (216k–224k) | 318k (316k–319k) | 317k (317k–318k) | 317k (316k–318k) | 100% | 0 |
| one | 1000 | 50 | 6 | 261k (251k–275k) | 270k (262k–272k) | 254k (253k–279k) | 187k (185k–191k) | 187k (180k–190k) | 201k (199k–208k) | 199k (198k–202k) | 288k (287k–290k) | 66% | 12 |
| one | 1000 | 50 | 8 | 253k (250k–256k) | 255k (248k–261k) | 253k (249k–259k) | 180k (175k–184k) | 187k (186k–195k) | 170k (166k–173k) | 168k (168k–170k) | 278k (272k–279k) | 51% | 5 |
| one | 1000 | 200 | 2 | 305k (295k–306k) | 318k (307k–318k) | 317k (310k–317k) | 318k (314k–318k) | 317k (314k–318k) | 317k (315k–318k) | 318k (315k–319k) | 318k (315k–319k) | 100% | 0 |
| one | 1000 | 200 | 3 | 290k (289k–294k) | 305k (299k–306k) | 307k (303k–307k) | 292k (291k–293k) | 303k (300k–305k) | 311k (310k–312k) | 311k (310k–312k) | 312k (310k–312k) | 100% | 0 |
| one | 1000 | 200 | 4 | 284k (282k–287k) | 299k (292k–301k) | 296k (295k–298k) | 207k (207k–208k) | 213k (212k–213k) | 305k (305k–306k) | 306k (304k–306k) | 306k (305k–307k) | 100% | 0 |
| one | 1000 | 200 | 6 | 245k (234k–248k) | 254k (248k–258k) | 254k (246k–259k) | 177k (170k–179k) | 183k (178k–188k) | 191k (185k–191k) | 194k (193k–200k) | 280k (278k–280k) | 67% | 14 |
| one | 1000 | 200 | 8 | 236k (233k–239k) | 245k (244k–250k) | 243k (234k–248k) | 174k (163k–176k) | 177k (174k–182k) | 160k (159k–164k) | 165k (163k–165k) | 269k (263k–271k) | 50% | 6 |
| one | 1000 | 1000 | 2 | 205k (202k–206k) | 239k (239k–244k) | 244k (243k–244k) | 254k (253k–254k) | 254k (254k–255k) | 254k (254k–255k) | 255k (254k–255k) | 254k (254k–255k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 199k (198k–200k) | 235k (234k–236k) | 234k (233k–235k) | 199k (198k–204k) | 235k (234k–242k) | 249k (249k–250k) | 250k (249k–250k) | 250k (249k–251k) | 100% | 0 |
| one | 1000 | 1000 | 4 | 198k (196k–198k) | 231k (229k–235k) | 233k (228k–233k) | 156k (155k–157k) | 176k (176k–177k) | 245k (244k–246k) | 245k (244k–246k) | 246k (245k–246k) | 100% | 0 |
| one | 1000 | 1000 | 6 | 170k (168k–178k) | 203k (193k–207k) | 202k (193k–205k) | 139k (133k–141k) | 152k (146k–153k) | 144k (142k–146k) | 163k (161k–163k) | 229k (228k–229k) | 68% | 13 |
| one | 1000 | 1000 | 8 | 172k (169k–173k) | 197k (194k–200k) | 199k (187k–201k) | 135k (132k–137k) | 151k (148k–154k) | 127k (125k–128k) | 142k (140k–143k) | 221k (220k–222k) | 50% | 8 |
| all | 0 | 50 | 2 | 897k (787k–901k) | 886k (869k–898k) | 899k (870k–908k) | 826k (823k–827k) | 825k (823k–828k) | 824k (811k–829k) | 825k (821k–828k) | 825k (813k–826k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.06M (987k–1.07M) | 1.07M (1.06M–1.07M) | 1.08M (1.05M–1.08M) | 910k (904k–912k) | 900k (891k–910k) | 989k (987k–993k) | 994k (986k–998k) | 991k (964k–991k) | 100% | 0 |
| all | 0 | 50 | 4 | 1.16M (1.15M–1.17M) | 1.16M (1.15M–1.17M) | 1.17M (1.16M–1.18M) | 842k (831k–849k) | 880k (864k–884k) | 1.09M (1.09M–1.10M) | 1.10M (1.09M–1.11M) | 1.10M (1.09M–1.10M) | 100% | 0 |
| all | 0 | 50 | 6 | 996k (969k–1.00M) | 1.04M (1.03M–1.04M) | 1.04M (1.03M–1.04M) | 684k (653k–707k) | 718k (699k–729k) | 639k (634k–654k) | 649k (638k–666k) | 1.09M (1.08M–1.10M) | 69% | 103 |
| all | 0 | 50 | 8 | 939k (928k–950k) | 931k (926k–941k) | 944k (934k–967k) | 679k (660k–818k) | 694k (653k–731k) | 496k (488k–506k) | 533k (503k–651k) | 1.07M (1.06M–1.08M) | 50% | 62 |
| all | 0 | 200 | 2 | 700k (685k–711k) | 811k (806k–815k) | 810k (808k–817k) | 747k (745k–749k) | 747k (745k–751k) | 748k (745k–750k) | 748k (747k–751k) | 748k (746k–748k) | 100% | 0 |
| all | 0 | 200 | 3 | 845k (840k–911k) | 947k (943k–957k) | 946k (943k–976k) | 780k (777k–785k) | 784k (783k–788k) | 878k (874k–879k) | 881k (876k–883k) | 879k (878k–882k) | 100% | 0 |
| all | 0 | 200 | 4 | 989k (985k–995k) | 1.04M (1.03M–1.05M) | 1.03M (1.02M–1.04M) | 749k (748k–754k) | 805k (790k–809k) | 956k (948k–960k) | 962k (943k–962k) | 962k (958k–964k) | 100% | 0 |
| all | 0 | 200 | 6 | 900k (878k–906k) | 942k (930k–955k) | 943k (941k–962k) | 639k (605k–660k) | 648k (632k–664k) | 594k (591k–623k) | 609k (609k–612k) | 951k (925k–981k) | 69% | 107 |
| all | 0 | 200 | 8 | 839k (824k–845k) | 846k (806k–857k) | 846k (734k–874k) | 598k (570k–673k) | 632k (622k–655k) | 468k (462k–497k) | 499k (471k–541k) | 960k (952k–969k) | 49% | 135 |
| all | 0 | 1000 | 2 | 402k (379k–458k) | 498k (495k–503k) | 498k (496k–504k) | 467k (466k–468k) | 467k (466k–468k) | 467k (467k–468k) | 467k (466k–468k) | 467k (465k–468k) | 100% | 0 |
| all | 0 | 1000 | 3 | 462k (454k–492k) | 602k (599k–609k) | 603k (602k–606k) | 477k (475k–478k) | 477k (476k–478k) | 515k (514k–518k) | 516k (516k–517k) | 516k (515k–517k) | 100% | 0 |
| all | 0 | 1000 | 4 | 556k (549k–561k) | 658k (656k–662k) | 654k (651k–655k) | 444k (439k–446k) | 485k (484k–491k) | 542k (541k–545k) | 544k (542k–544k) | 543k (537k–556k) | 100% | 0 |
| all | 0 | 1000 | 6 | 509k (501k–516k) | 586k (579k–588k) | 583k (577k–591k) | 402k (393k–410k) | 427k (414k–437k) | 389k (385k–414k) | 402k (396k–405k) | 520k (514k–526k) | 67% | 110 |
| all | 0 | 1000 | 8 | 509k (499k–510k) | 529k (525k–538k) | 535k (519k–538k) | 402k (378k–418k) | 431k (410k–440k) | 334k (329k–340k) | 361k (358k–369k) | 519k (504k–547k) | 49% | 76 |
| all | 1000 | 50 | 2 | 454k (452k–456k) | 462k (457k–464k) | 567k (539k–680k) | 805k (802k–815k) | 803k (798k–807k) | 804k (799k–807k) | 806k (804k–811k) | 806k (802k–809k) | 100% | 0 |
| all | 1000 | 50 | 3 | 639k (604k–643k) | 631k (606k–647k) | 526k (514k–528k) | 531k (519k–533k) | 532k (523k–534k) | 976k (970k–980k) | 989k (980k–990k) | 986k (979k–991k) | 100% | 0 |
| all | 1000 | 50 | 4 | 708k (704k–726k) | 731k (724k–736k) | 575k (573k–576k) | 570k (569k–570k) | 572k (567k–575k) | 1.09M (1.06M–1.09M) | 1.09M (1.07M–1.11M) | 1.09M (1.09M–1.10M) | 100% | 4 |
| all | 1000 | 50 | 6 | 722k (707k–725k) | 736k (729k–740k) | 571k (560k–576k) | 565k (551k–585k) | 582k (569k–602k) | 548k (547k–575k) | 557k (550k–579k) | 903k (873k–905k) | 68% | 130 |
| all | 1000 | 50 | 8 | 743k (741k–752k) | 754k (740k–755k) | 551k (549k–569k) | 605k (587k–615k) | 597k (568k–608k) | 489k (479k–494k) | 490k (482k–507k) | 923k (917k–928k) | 50% | 46 |
| all | 1000 | 200 | 2 | 405k (404k–419k) | 429k (424k–446k) | 792k (780k–804k) | 745k (742k–749k) | 747k (743k–748k) | 746k (741k–747k) | 746k (744k–751k) | 747k (717k–748k) | 100% | 0 |
| all | 1000 | 200 | 3 | 543k (540k–572k) | 573k (563k–592k) | 513k (511k–527k) | 566k (553k–573k) | 571k (560k–605k) | 874k (873k–878k) | 876k (852k–879k) | 876k (871k–877k) | 100% | 0 |
| all | 1000 | 200 | 4 | 647k (644k–649k) | 663k (650k–675k) | 533k (524k–534k) | 544k (537k–554k) | 559k (555k–566k) | 952k (950k–955k) | 955k (949k–957k) | 954k (951k–957k) | 100% | 0 |
| all | 1000 | 200 | 6 | 656k (650k–658k) | 689k (683k–696k) | 545k (542k–549k) | 542k (525k–547k) | 566k (550k–570k) | 516k (510k–517k) | 532k (529k–555k) | 863k (859k–882k) | 68% | 74 |
| all | 1000 | 200 | 8 | 680k (670k–681k) | 702k (691k–707k) | 528k (524k–531k) | 550k (544k–574k) | 548k (521k–577k) | 445k (444k–459k) | 468k (462k–479k) | 862k (854k–864k) | 49% | 75 |
| all | 1000 | 1000 | 2 | 278k (275k–279k) | 363k (357k–367k) | 499k (496k–499k) | 467k (465k–468k) | 466k (466k–468k) | 466k (466k–467k) | 467k (466k–468k) | 467k (466k–468k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 402k (387k–420k) | 433k (430k–436k) | 472k (468k–474k) | 422k (420k–424k) | 478k (476k–479k) | 515k (514k–516k) | 516k (515k–517k) | 517k (516k–517k) | 100% | 0 |
| all | 1000 | 1000 | 4 | 408k (406k–411k) | 505k (499k–513k) | 539k (536k–543k) | 393k (385k–395k) | 469k (468k–481k) | 541k (540k–543k) | 541k (535k–543k) | 543k (541k–543k) | 100% | 0 |
| all | 1000 | 1000 | 6 | 436k (424k–448k) | 481k (470k–495k) | 471k (444k–508k) | 378k (371k–400k) | 410k (402k–422k) | 368k (361k–375k) | 396k (395k–402k) | 500k (491k–515k) | 68% | 101 |
| all | 1000 | 1000 | 8 | 446k (443k–449k) | 478k (473k–482k) | 412k (393k–419k) | 385k (374k–387k) | 417k (392k–445k) | 334k (325k–335k) | 363k (360k–371k) | 498k (485k–501k) | 50% | 47 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 first | 1 fair | 1 fair, irq/msg | 2/2 first | 2/2 fair | 2/4 first | 2/4 fair | 3 | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.78 / 0.91 / 1.04 (0.87–1.04) | 0.70 / 0.83 / 0.91 (0.83–0.95) | 0.70 / 0.83 / 0.91 (0.83–0.95) | 0.66 / 0.74 / 0.83 (0.79–0.87) | 0.66 / 0.74 / 0.79 (0.79–0.87) | 0.66 / 0.74 / 0.79 (0.79–0.83) | 0.66 / 0.74 / 0.83 (0.79–0.83) | 0.66 / 0.74 / 0.79 (0.79–0.83) | 100% | 0 |
| one | 0 | 50 | 3 | 0.83 / 0.95 / 3.36 (1.04–4.74) | 0.78 / 0.87 / 1.04 (1.00–3.39) | 0.78 / 0.87 / 1.04 (0.95–1.66) | 0.83 / 1.04 / 3.52 (1.12–4.99) | 0.78 / 0.95 / 1.07 (1.04–4.22) | 0.70 / 0.79 / 3.74 (0.87–4.48) | 0.70 / 0.79 / 1.12 (0.87–5.44) | 0.70 / 0.79 / 0.87 (0.83–0.91) | 100% | 0 |
| one | 0 | 50 | 4 | 0.87 / 1.12 / 5.12 (1.82–8.06) | 0.79 / 0.95 / 4.48 (1.12–6.02) | 0.79 / 0.95 / 1.90 (1.12–6.27) | 1.28 / 1.66 / 6.78 (1.74–11.65) | 1.20 / 1.54 / 4.61 (1.74–7.81) | 0.74 / 0.83 / 1.12 (0.91–6.78) | 0.74 / 0.87 / 4.74 (2.56–5.12) | 0.74 / 0.87 / 3.55 (0.91–5.57) | 100% | 0 |
| one | 0 | 50 | 6 | 1.33 / 1.66 / 5.89 (4.10–6.34) | 1.28 / 1.62 / 4.67 (3.94–5.76) | 1.25 / 1.58 / 4.74 (4.03–6.08) | 2.03 / 2.85 / 6.40 (4.35–6.98) | 2.00 / 2.69 / 5.31 (4.16–6.98) | 1.79 / 2.50 / 4.35 (4.03–6.66) | 1.78 / 2.43 / 5.63 (5.25–6.66) | 1.07 / 1.25 / 3.78 (3.65–5.50) | 68% | 12 |
| one | 0 | 50 | 8 | 1.54 / 1.90 / 7.42 (5.38–9.22) | 1.49 / 1.87 / 6.72 (5.89–7.74) | 1.49 / 1.82 / 6.21 (5.31–8.70) | 2.24 / 3.39 / 7.87 (7.42–9.86) | 2.14 / 3.36 / 7.74 (6.14–9.60) | 2.69 / 3.36 / 8.19 (6.91–10.11) | 2.69 / 3.33 / 7.81 (7.10–10.62) | 1.20 / 1.36 / 5.38 (4.42–7.30) | 50% | 2 |
| one | 0 | 200 | 2 | 0.91 / 1.04 / 1.15 (1.12–1.49) | 0.79 / 0.91 / 1.00 (1.00–1.12) | 0.79 / 0.91 / 1.00 (1.00–1.00) | 0.79 / 0.87 / 0.95 (0.95–1.54) | 0.79 / 0.87 / 0.95 (0.91–1.36) | 0.79 / 0.87 / 0.95 (0.95–5.44) | 0.79 / 0.87 / 0.95 (0.91–5.76) | 0.79 / 0.91 / 0.95 (0.95–5.57) | 100% | 0 |
| one | 0 | 200 | 3 | 1.07 / 1.20 / 4.74 (1.33–6.21) | 0.91 / 1.04 / 4.03 (1.20–6.40) | 0.91 / 1.07 / 1.25 (1.20–5.76) | 0.95 / 1.28 / 4.74 (1.46–6.27) | 0.87 / 1.07 / 3.68 (1.15–6.53) | 0.83 / 0.95 / 3.52 (1.00–6.14) | 0.83 / 0.91 / 4.29 (1.00–5.70) | 0.83 / 0.91 / 1.00 (1.00–3.55) | 100% | 0 |
| one | 0 | 200 | 4 | 1.12 / 1.28 / 5.82 (1.36–7.74) | 0.95 / 1.12 / 4.10 (2.43–6.27) | 1.00 / 1.12 / 4.86 (1.95–5.89) | 1.54 / 1.87 / 5.70 (3.49–8.19) | 1.41 / 1.79 / 4.86 (3.04–7.87) | 0.87 / 1.04 / 4.61 (1.04–7.30) | 0.87 / 0.95 / 3.52 (1.04–6.40) | 0.87 / 0.95 / 4.22 (1.04–5.57) | 100% | 0 |
| one | 0 | 200 | 6 | 1.54 / 1.90 / 6.27 (4.10–6.78) | 1.46 / 1.78 / 4.74 (4.06–6.40) | 1.46 / 1.74 / 4.80 (3.74–6.21) | 2.24 / 3.14 / 6.21 (4.99–8.32) | 2.11 / 2.85 / 5.31 (4.48–7.55) | 2.21 / 2.78 / 5.12 (3.94–7.10) | 2.00 / 2.62 / 5.12 (4.74–7.49) | 1.20 / 1.36 / 3.90 (3.68–4.99) | 67% | 18 |
| one | 0 | 200 | 8 | 1.74 / 2.11 / 7.74 (6.59–8.70) | 1.66 / 2.03 / 7.49 (5.44–9.34) | 1.66 / 2.00 / 6.34 (5.31–7.74) | 2.56 / 3.78 / 9.09 (7.23–10.37) | 2.40 / 3.49 / 8.00 (7.30–8.83) | 2.98 / 3.65 / 8.83 (7.87–9.22) | 2.82 / 3.46 / 7.94 (7.30–8.96) | 1.36 / 1.49 / 5.95 (5.06–8.70) | 50% | 7 |
| one | 0 | 1000 | 2 | 2.56 / 2.69 / 6.91 (2.82–7.55) | 1.82 / 1.95 / 2.08 (1.95–5.57) | 1.82 / 1.95 / 2.08 (2.03–2.11) | 1.57 / 1.66 / 1.82 (1.74–6.66) | 1.57 / 1.66 / 1.82 (1.70–11.65) | 1.57 / 1.66 / 1.74 (1.74–2.37) | 1.57 / 1.66 / 1.79 (1.70–13.95) | 1.57 / 1.66 / 1.74 (1.70–4.61) | 100% | 0 |
| one | 0 | 1000 | 3 | 2.66 / 2.82 / 8.45 (2.78–9.47) | 1.87 / 2.00 / 6.78 (4.22–12.29) | 1.90 / 2.03 / 5.82 (2.03–15.74) | 2.62 / 2.82 / 6.98 (6.66–9.34) | 1.87 / 2.11 / 5.06 (2.24–7.17) | 1.62 / 1.70 / 7.04 (1.79–14.46) | 1.62 / 1.70 / 4.35 (1.79–6.72) | 1.62 / 1.70 / 1.82 (1.78–5.25) | 100% | 0 |
| one | 0 | 1000 | 4 | 2.66 / 2.94 / 9.34 (4.16–10.75) | 2.00 / 2.24 / 8.45 (3.04–15.87) | 1.95 / 2.14 / 7.49 (3.39–8.58) | 3.14 / 3.52 / 9.09 (6.91–9.86) | 2.40 / 2.78 / 7.87 (2.85–10.50) | 1.70 / 1.95 / 6.66 (5.38–8.70) | 1.70 / 1.90 / 6.08 (5.70–7.87) | 1.70 / 1.79 / 6.02 (1.87–8.83) | 99% | 0 |
| one | 0 | 1000 | 6 | 3.23 / 3.78 / 9.73 (6.78–10.50) | 2.50 / 2.82 / 7.10 (5.95–9.09) | 2.43 / 2.78 / 6.53 (4.29–8.58) | 3.94 / 4.80 / 9.47 (7.74–10.62) | 3.14 / 4.00 / 6.53 (6.21–8.96) | 3.65 / 4.35 / 6.78 (5.95–9.73) | 3.04 / 3.58 / 6.66 (5.95–8.70) | 2.00 / 2.21 / 5.31 (4.06–7.74) | 67% | 21 |
| one | 0 | 1000 | 8 | 3.39 / 3.81 / 11.01 (9.60–11.78) | 2.62 / 2.94 / 8.70 (8.00–9.47) | 2.66 / 3.04 / 8.45 (7.87–8.70) | 4.16 / 5.44 / 11.78 (9.98–13.06) | 3.49 / 4.61 / 10.11 (9.34–11.14) | 4.61 / 5.18 / 11.14 (9.60–12.29) | 3.81 / 4.42 / 9.86 (9.73–11.90) | 2.14 / 2.27 / 7.49 (7.10–8.70) | 50% | 4 |
| one | 1000 | 50 | 2 | 1.78 / 1.90 / 5.18 (2.00–6.27) | 1.70 / 1.82 / 1.95 (1.90–4.99) | 1.70 / 1.82 / 1.95 (1.90–1.95) | 1.66 / 1.74 / 1.82 (1.82–5.31) | 1.66 / 1.74 / 1.82 (1.79–4.93) | 1.66 / 1.74 / 1.82 (1.82–2.53) | 1.66 / 1.74 / 1.82 (1.82–3.26) | 1.66 / 1.74 / 1.82 (1.82–5.25) | 100% | 0 |
| one | 1000 | 50 | 3 | 1.79 / 1.95 / 7.04 (2.08–7.55) | 1.78 / 1.87 / 5.44 (2.00–6.27) | 1.74 / 1.82 / 5.57 (2.00–6.46) | 1.79 / 1.90 / 5.18 (2.08–8.83) | 1.74 / 1.87 / 6.40 (1.95–7.10) | 1.70 / 1.79 / 5.25 (1.90–7.49) | 1.70 / 1.79 / 5.31 (5.25–6.53) | 1.70 / 1.79 / 4.80 (1.90–6.53) | 100% | 0 |
| one | 1000 | 50 | 4 | 1.90 / 2.03 / 7.04 (2.14–10.11) | 1.82 / 2.00 / 7.04 (5.31–8.58) | 1.82 / 2.03 / 6.46 (2.11–13.06) | 3.20 / 3.52 / 10.50 (7.74–11.26) | 3.10 / 3.52 / 9.60 (7.68–13.44) | 1.74 / 1.87 / 7.49 (2.00–9.22) | 1.74 / 1.87 / 8.19 (6.40–9.22) | 1.74 / 1.87 / 6.78 (4.80–9.47) | 100% | 0 |
| one | 1000 | 50 | 6 | 2.40 / 2.82 / 8.96 (6.46–9.86) | 2.30 / 2.66 / 6.59 (5.63–8.58) | 2.56 / 3.04 / 7.49 (6.27–8.96) | 3.94 / 4.54 / 9.22 (6.08–11.90) | 3.94 / 4.74 / 9.98 (7.74–10.24) | 3.62 / 4.16 / 7.74 (6.34–8.83) | 3.62 / 4.16 / 9.09 (7.30–9.73) | 2.11 / 2.27 / 6.46 (4.74–9.73) | 66% | 12 |
| one | 1000 | 50 | 8 | 2.56 / 2.98 / 10.11 (8.32–13.31) | 2.53 / 2.98 / 8.96 (8.19–13.44) | 2.56 / 3.04 / 9.22 (8.58–12.54) | 4.10 / 4.61 / 11.14 (10.37–16.64) | 4.00 / 4.48 / 11.01 (9.60–12.67) | 4.29 / 4.86 / 11.52 (10.11–11.78) | 4.22 / 4.86 / 10.62 (10.11–12.03) | 2.24 / 2.40 / 7.94 (6.98–9.60) | 51% | 5 |
| one | 1000 | 200 | 2 | 1.90 / 2.03 / 2.27 (2.27–5.95) | 1.79 / 1.90 / 2.03 (2.00–2.21) | 1.79 / 1.90 / 2.03 (2.00–5.38) | 1.78 / 1.87 / 2.91 (1.95–8.32) | 1.78 / 1.87 / 2.00 (2.00–10.37) | 1.78 / 1.87 / 2.00 (2.00–7.55) | 1.78 / 1.87 / 1.95 (1.95–7.42) | 1.78 / 1.90 / 2.00 (1.95–7.68) | 100% | 0 |
| one | 1000 | 200 | 3 | 2.03 / 2.21 / 6.08 (2.37–14.59) | 1.90 / 2.03 / 5.95 (2.24–8.96) | 1.87 / 2.03 / 2.94 (2.11–6.46) | 2.03 / 2.21 / 6.40 (2.30–9.22) | 1.90 / 2.03 / 6.08 (2.14–6.98) | 1.82 / 1.90 / 6.21 (5.31–8.32) | 1.82 / 1.90 / 5.44 (2.03–8.45) | 1.82 / 1.90 / 2.03 (2.00–9.73) | 100% | 0 |
| one | 1000 | 200 | 4 | 2.11 / 2.72 / 9.47 (6.40–9.86) | 1.95 / 2.14 / 6.91 (5.89–10.37) | 2.00 / 2.24 / 8.58 (6.85–9.34) | 3.39 / 3.68 / 10.50 (8.06–11.52) | 3.26 / 3.62 / 9.73 (6.72–16.00) | 1.87 / 2.21 / 8.58 (7.23–9.22) | 1.87 / 2.00 / 8.00 (7.23–8.32) | 1.87 / 1.95 / 6.98 (2.24–8.83) | 100% | 0 |
| one | 1000 | 200 | 6 | 2.69 / 3.10 / 9.09 (5.76–29.18) | 2.50 / 2.98 / 8.19 (5.95–12.29) | 2.53 / 2.94 / 6.46 (5.12–10.62) | 4.22 / 4.80 / 10.24 (9.09–11.26) | 4.06 / 4.86 / 9.34 (7.23–10.50) | 3.90 / 4.42 / 9.34 (7.36–9.73) | 3.78 / 4.22 / 8.70 (7.04–9.98) | 2.21 / 2.40 / 5.89 (4.86–7.55) | 67% | 14 |
| one | 1000 | 200 | 8 | 2.85 / 3.26 / 9.09 (8.70–11.65) | 2.72 / 3.10 / 8.83 (8.13–9.98) | 2.72 / 3.14 / 8.83 (8.32–10.75) | 4.35 / 4.86 / 11.90 (9.73–12.80) | 4.22 / 5.12 / 10.88 (9.34–12.03) | 4.48 / 5.12 / 12.16 (10.62–13.44) | 4.42 / 4.99 / 11.26 (9.86–12.16) | 2.37 / 2.53 / 8.83 (7.55–11.01) | 50% | 6 |
| one | 1000 | 1000 | 2 | 3.49 / 3.62 / 8.19 (3.74–13.95) | 2.82 / 2.91 / 3.07 (2.94–5.31) | 2.72 / 2.85 / 3.04 (2.94–7.36) | 2.56 / 2.66 / 5.82 (2.78–8.32) | 2.56 / 2.66 / 2.78 (2.72–8.83) | 2.56 / 2.66 / 5.12 (2.78–7.49) | 2.56 / 2.66 / 2.78 (2.72–7.23) | 2.56 / 2.66 / 2.78 (2.72–6.66) | 100% | 0 |
| one | 1000 | 1000 | 3 | 3.65 / 3.78 / 8.83 (8.19–12.16) | 2.85 / 2.98 / 7.49 (3.33–9.60) | 2.91 / 3.04 / 6.08 (3.23–9.09) | 3.62 / 3.78 / 8.96 (3.78–10.37) | 2.85 / 3.04 / 5.76 (3.07–9.60) | 2.62 / 2.69 / 8.58 (5.89–9.60) | 2.62 / 2.69 / 6.40 (5.76–9.60) | 2.62 / 2.69 / 6.53 (2.78–8.70) | 100% | 0 |
| one | 1000 | 1000 | 4 | 3.65 / 3.94 / 11.52 (10.75–14.08) | 2.94 / 3.23 / 11.14 (7.62–12.67) | 2.91 / 3.10 / 8.58 (6.98–11.90) | 4.99 / 5.25 / 13.57 (11.14–13.82) | 4.22 / 4.54 / 9.86 (5.95–13.18) | 2.66 / 2.78 / 8.83 (7.49–11.90) | 2.66 / 2.78 / 8.96 (7.87–11.65) | 2.66 / 2.72 / 8.32 (7.55–9.98) | 100% | 0 |
| one | 1000 | 1000 | 6 | 4.48 / 4.99 / 11.26 (11.01–13.31) | 3.52 / 3.94 / 9.86 (7.62–10.50) | 3.62 / 4.16 / 10.11 (6.78–11.78) | 5.82 / 6.66 / 12.03 (11.14–14.98) | 5.12 / 6.08 / 10.88 (9.86–13.44) | 5.50 / 6.08 / 10.88 (8.83–12.93) | 4.74 / 5.31 / 10.11 (9.34–11.26) | 2.98 / 3.23 / 7.74 (5.82–8.96) | 68% | 13 |
| one | 1000 | 1000 | 8 | 4.42 / 4.93 / 12.54 (11.90–14.08) | 3.74 / 4.16 / 11.01 (10.11–13.31) | 3.68 / 4.06 / 11.14 (9.86–12.03) | 6.02 / 6.66 / 14.08 (12.80–14.59) | 5.25 / 5.82 / 12.67 (11.65–13.82) | 6.14 / 6.78 / 13.95 (12.54–14.85) | 5.31 / 5.95 / 12.16 (11.39–12.80) | 3.20 / 3.33 / 8.96 (8.70–10.24) | 50% | 8 |
| all | 0 | 50 | 2 | 0.79 / 1.15 / 1.25 (1.20–1.33) | 0.78 / 1.04 / 1.20 (1.12–1.41) | 0.74 / 1.07 / 1.15 (1.12–1.20) | 1.07 / 1.20 / 1.28 (1.28–2.94) | 1.07 / 1.20 / 1.28 (1.28–1.46) | 1.07 / 1.25 / 1.28 (1.28–11.39) | 1.07 / 1.20 / 1.28 (1.28–1.36) | 1.07 / 1.20 / 1.28 (1.28–11.52) | 100% | 0 |
| all | 0 | 50 | 3 | 1.04 / 1.58 / 4.16 (1.70–6.21) | 0.95 / 1.54 / 1.78 (1.70–6.14) | 0.95 / 1.54 / 3.26 (1.66–5.31) | 1.57 / 2.37 / 3.52 (2.50–6.91) | 1.66 / 2.40 / 5.12 (2.43–11.78) | 1.66 / 1.82 / 4.54 (1.90–6.08) | 1.66 / 1.82 / 4.99 (1.95–5.76) | 1.66 / 1.82 / 3.07 (1.90–14.98) | 100% | 0 |
| all | 0 | 50 | 4 | 1.41 / 2.21 / 5.50 (2.43–7.68) | 1.15 / 2.11 / 5.57 (2.69–7.94) | 1.07 / 1.95 / 5.57 (3.20–8.19) | 2.56 / 4.03 / 7.62 (5.95–10.88) | 2.30 / 3.68 / 5.70 (4.00–8.45) | 2.27 / 2.43 / 6.08 (3.81–7.55) | 2.24 / 2.43 / 7.62 (4.99–8.96) | 2.27 / 2.40 / 5.18 (2.53–8.00) | 100% | 0 |
| all | 0 | 50 | 6 | 2.66 / 6.46 / 9.47 (9.22–12.29) | 2.50 / 6.53 / 9.34 (9.09–11.26) | 2.50 / 6.46 / 9.22 (8.45–10.62) | 5.50 / 9.34 / 12.80 (11.78–14.72) | 5.38 / 9.22 / 12.42 (11.52–14.59) | 6.40 / 9.98 / 15.23 (12.93–17.66) | 6.27 / 9.60 / 15.10 (13.31–16.13) | 3.90 / 5.38 / 8.19 (5.89–8.96) | 69% | 103 |
| all | 0 | 50 | 8 | 4.74 / 11.65 / 15.62 (14.34–16.90) | 4.99 / 12.16 / 16.13 (14.46–17.41) | 4.86 / 11.90 / 14.72 (13.95–15.87) | 8.19 / 14.08 / 19.97 (16.64–21.76) | 8.00 / 14.08 / 19.97 (16.64–20.48) | 12.42 / 17.92 / 26.88 (25.60–29.70) | 10.75 / 17.41 / 26.88 (25.34–27.65) | 6.02 / 8.13 / 14.21 (12.03–15.74) | 50% | 62 |
| all | 0 | 200 | 2 | 1.07 / 1.58 / 1.70 (1.66–3.68) | 1.07 / 1.41 / 1.49 (1.46–1.49) | 1.07 / 1.41 / 1.49 (1.46–1.49) | 1.33 / 1.49 / 1.54 (1.54–2.94) | 1.33 / 1.49 / 1.70 (1.54–2.94) | 1.33 / 1.49 / 1.62 (1.54–2.08) | 1.33 / 1.46 / 1.57 (1.54–1.62) | 1.33 / 1.49 / 1.57 (1.54–1.62) | 100% | 0 |
| all | 0 | 200 | 3 | 1.28 / 2.21 / 4.99 (2.40–8.13) | 1.28 / 2.00 / 2.30 (2.14–6.78) | 1.36 / 2.00 / 3.23 (2.24–6.27) | 2.50 / 2.72 / 5.38 (2.85–8.45) | 2.43 / 2.69 / 4.93 (2.82–8.32) | 2.03 / 2.21 / 5.44 (2.30–7.62) | 2.03 / 2.21 / 5.25 (4.03–6.72) | 2.03 / 2.21 / 3.65 (2.27–6.27) | 100% | 0 |
| all | 0 | 200 | 4 | 2.14 / 2.85 / 7.36 (3.23–10.75) | 1.46 / 2.56 / 5.57 (2.91–8.06) | 1.54 / 2.66 / 6.66 (3.81–12.03) | 3.20 / 4.61 / 8.13 (5.38–14.59) | 2.94 / 4.10 / 7.04 (4.35–11.14) | 2.78 / 3.39 / 8.00 (3.33–10.37) | 2.78 / 2.98 / 7.87 (5.18–13.95) | 2.78 / 2.94 / 7.17 (3.07–8.45) | 100% | 0 |
| all | 0 | 200 | 6 | 3.14 / 6.66 / 10.88 (9.60–12.29) | 2.94 / 7.49 / 10.24 (10.11–11.90) | 2.85 / 7.17 / 10.24 (9.98–11.65) | 5.95 / 10.37 / 13.82 (12.29–16.64) | 6.46 / 9.98 / 13.31 (12.54–17.66) | 7.17 / 10.75 / 15.62 (14.08–19.46) | 7.04 / 10.24 / 15.10 (14.34–16.38) | 4.99 / 6.21 / 9.98 (7.42–12.29) | 69% | 107 |
| all | 0 | 200 | 8 | 5.25 / 12.80 / 17.92 (16.00–18.94) | 5.70 / 13.82 / 17.15 (16.26–47.10) | 5.82 / 14.46 / 16.64 (16.64–204.80) | 9.09 / 15.74 / 21.76 (19.97–141.31) | 9.22 / 15.23 / 21.76 (19.20–32.77) | 13.70 / 18.69 / 29.70 (26.11–34.30) | 11.78 / 18.69 / 28.16 (27.39–30.46) | 7.10 / 8.96 / 15.62 (13.95–15.87) | 49% | 135 |
| all | 0 | 1000 | 2 | 2.62 / 3.90 / 7.49 (3.90–7.55) | 2.62 / 2.78 / 3.33 (2.82–7.23) | 2.62 / 2.78 / 2.85 (2.82–6.21) | 2.91 / 3.07 / 6.59 (3.10–7.49) | 2.91 / 3.07 / 3.14 (3.10–8.45) | 2.91 / 3.07 / 3.23 (3.20–7.62) | 2.91 / 3.07 / 4.74 (3.26–6.21) | 2.91 / 3.07 / 3.14 (3.10–6.14) | 100% | 0 |
| all | 0 | 1000 | 3 | 2.85 / 5.38 / 11.14 (5.57–16.64) | 2.30 / 4.10 / 6.85 (5.25–10.75) | 2.30 / 4.10 / 6.53 (4.22–8.06) | 4.86 / 5.12 / 10.50 (6.66–14.21) | 4.86 / 5.12 / 7.87 (5.82–13.44) | 4.42 / 4.61 / 10.37 (7.30–12.29) | 4.42 / 4.61 / 9.09 (8.58–10.37) | 4.42 / 4.61 / 4.86 (4.74–9.73) | 100% | 0 |
| all | 0 | 1000 | 4 | 4.54 / 6.08 / 15.36 (9.22–20.74) | 2.78 / 8.19 / 11.65 (8.19–13.06) | 2.82 / 5.63 / 8.96 (8.32–11.39) | 6.66 / 9.22 / 14.46 (11.01–25.34) | 6.78 / 7.17 / 16.64 (10.11–24.83) | 5.95 / 6.21 / 14.72 (10.50–17.41) | 5.95 / 6.14 / 11.14 (10.62–13.57) | 5.95 / 6.14 / 11.26 (6.21–16.64) | 100% | 0 |
| all | 0 | 1000 | 6 | 7.68 / 11.78 / 21.50 (17.41–22.78) | 6.14 / 13.57 / 16.00 (15.87–18.69) | 6.21 / 13.06 / 16.26 (15.74–18.69) | 11.90 / 16.90 / 23.30 (22.27–25.09) | 11.90 / 14.85 / 19.46 (18.18–23.81) | 12.42 / 18.94 / 23.81 (20.74–25.09) | 13.06 / 15.36 / 23.55 (20.74–27.14) | 10.24 / 11.14 / 16.13 (15.36–18.94) | 67% | 110 |
| all | 0 | 1000 | 8 | 8.19 / 19.97 / 28.16 (24.06–28.67) | 11.52 / 21.25 / 26.37 (24.83–26.88) | 11.52 / 19.46 / 26.88 (24.06–28.67) | 15.74 / 25.60 / 33.28 (30.72–37.89) | 15.62 / 23.30 / 31.23 (28.93–46.59) | 21.25 / 29.44 / 40.45 (39.42–54.78) | 19.97 / 24.58 / 36.86 (34.30–40.45) | 14.46 / 17.66 / 24.83 (23.55–57.86) | 49% | 76 |
| all | 1000 | 50 | 2 | 1.66 / 1.87 / 5.63 (2.11–8.58) | 1.62 / 1.78 / 2.11 (1.90–5.12) | 1.57 / 1.95 / 2.08 (2.03–4.16) | 1.07 / 1.78 / 2.00 (1.87–3.81) | 1.07 / 1.79 / 1.90 (1.87–6.14) | 1.07 / 1.79 / 1.90 (1.87–5.89) | 1.07 / 1.78 / 1.87 (1.87–1.90) | 1.07 / 1.78 / 1.90 (1.87–4.35) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.11 / 3.26 / 7.81 (4.16–8.70) | 2.11 / 3.52 / 5.06 (3.74–8.19) | 1.87 / 4.61 / 6.66 (4.86–8.45) | 2.00 / 3.62 / 6.66 (3.68–6.85) | 1.95 / 3.62 / 3.74 (3.68–9.22) | 1.66 / 1.87 / 5.89 (2.66–6.59) | 1.66 / 1.82 / 4.48 (1.90–5.12) | 1.66 / 1.82 / 3.14 (1.90–5.70) | 100% | 0 |
| all | 1000 | 50 | 4 | 2.30 / 4.42 / 10.88 (5.50–13.44) | 2.24 / 4.22 / 7.30 (6.08–11.14) | 2.78 / 5.95 / 8.58 (7.04–10.24) | 3.23 / 5.06 / 10.75 (7.17–11.90) | 3.20 / 4.93 / 6.02 (5.44–15.36) | 2.27 / 2.43 / 7.62 (5.50–8.45) | 2.27 / 2.43 / 6.59 (2.56–8.19) | 2.27 / 2.40 / 5.70 (2.56–8.00) | 100% | 4 |
| all | 1000 | 50 | 6 | 3.74 / 7.49 / 12.54 (9.22–13.82) | 3.68 / 7.42 / 10.11 (8.96–13.44) | 4.54 / 11.26 / 13.95 (13.70–14.21) | 5.44 / 9.73 / 14.08 (11.52–18.18) | 5.18 / 9.34 / 11.39 (11.26–14.59) | 5.44 / 9.98 / 14.21 (13.44–16.64) | 5.31 / 9.47 / 14.21 (10.88–15.49) | 3.46 / 5.38 / 8.45 (6.91–12.03) | 68% | 130 |
| all | 1000 | 50 | 8 | 5.31 / 10.50 / 17.15 (13.95–18.18) | 5.44 / 11.14 / 15.23 (14.21–16.64) | 7.23 / 18.94 / 22.27 (21.76–23.55) | 7.10 / 13.57 / 19.71 (17.15–23.04) | 7.23 / 13.57 / 19.20 (17.41–21.25) | 9.09 / 16.38 / 25.60 (24.06–28.67) | 8.83 / 16.38 / 25.60 (22.27–27.14) | 4.42 / 7.94 / 13.70 (12.16–16.38) | 50% | 46 |
| all | 1000 | 200 | 2 | 2.03 / 2.43 / 4.93 (2.56–6.08) | 1.87 / 2.03 / 2.11 (2.03–5.57) | 1.04 / 2.40 / 2.50 (2.37–2.50) | 1.33 / 1.49 / 1.90 (1.54–3.74) | 1.33 / 1.46 / 1.87 (1.66–8.32) | 1.33 / 1.49 / 1.95 (1.78–10.88) | 1.33 / 1.46 / 1.87 (1.74–6.08) | 1.33 / 1.49 / 1.90 (1.74–12.03) | 100% | 0 |
| all | 1000 | 200 | 3 | 2.62 / 3.74 / 8.32 (6.21–10.11) | 2.30 / 3.90 / 6.21 (3.94–12.93) | 2.11 / 4.74 / 7.36 (5.44–8.58) | 2.53 / 3.68 / 7.55 (3.87–17.92) | 2.53 / 3.68 / 4.42 (3.81–9.47) | 2.03 / 2.21 / 6.40 (3.26–7.81) | 2.03 / 2.21 / 5.44 (2.27–15.23) | 2.03 / 2.21 / 3.14 (2.27–6.78) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.98 / 4.93 / 10.75 (6.78–13.44) | 2.50 / 4.54 / 6.59 (4.86–17.66) | 3.26 / 6.34 / 10.37 (9.34–12.67) | 3.62 / 5.70 / 11.65 (5.89–14.46) | 3.46 / 5.25 / 6.46 (6.02–11.26) | 2.78 / 3.07 / 7.74 (6.66–8.70) | 2.78 / 2.94 / 7.62 (5.95–9.47) | 2.78 / 2.94 / 6.78 (3.36–9.22) | 100% | 0 |
| all | 1000 | 200 | 6 | 4.22 / 8.13 / 14.08 (10.37–14.85) | 4.16 / 8.13 / 10.75 (10.50–13.95) | 4.86 / 12.16 / 15.23 (15.23–16.38) | 6.14 / 10.62 / 15.23 (13.57–17.41) | 5.63 / 10.11 / 12.54 (12.03–14.85) | 6.27 / 10.75 / 16.26 (13.82–18.18) | 5.89 / 10.24 / 14.34 (12.03–15.62) | 4.42 / 6.27 / 8.70 (8.06–11.52) | 68% | 74 |
| all | 1000 | 200 | 8 | 5.95 / 11.01 / 18.43 (14.98–20.22) | 5.95 / 11.26 / 15.62 (15.49–17.15) | 8.19 / 20.74 / 25.09 (24.83–25.60) | 8.32 / 14.98 / 21.25 (19.20–32.26) | 8.58 / 14.85 / 19.97 (18.43–21.50) | 10.62 / 18.43 / 27.65 (27.14–29.18) | 9.73 / 17.15 / 25.86 (24.83–27.90) | 5.76 / 9.09 / 14.46 (13.18–15.62) | 49% | 75 |
| all | 1000 | 1000 | 2 | 3.62 / 4.86 / 9.22 (7.10–9.60) | 2.62 / 2.94 / 3.74 (3.04–6.85) | 2.62 / 2.78 / 3.04 (2.85–7.30) | 2.91 / 3.07 / 7.23 (3.14–8.00) | 2.91 / 3.07 / 3.58 (3.14–6.85) | 2.91 / 3.07 / 6.66 (3.14–8.70) | 2.91 / 3.07 / 4.29 (3.14–7.17) | 2.91 / 3.07 / 3.46 (3.10–5.63) | 100% | 0 |
| all | 1000 | 1000 | 3 | 4.99 / 6.34 / 13.31 (8.32–13.70) | 3.04 / 5.95 / 8.32 (6.08–12.42) | 3.68 / 4.22 / 5.44 (4.99–11.14) | 3.94 / 6.59 / 9.22 (6.78–10.75) | 4.86 / 5.06 / 8.45 (5.18–9.73) | 4.42 / 4.61 / 11.52 (9.09–11.90) | 4.42 / 4.61 / 8.70 (8.19–11.78) | 4.42 / 4.61 / 7.10 (4.67–10.24) | 100% | 0 |
| all | 1000 | 1000 | 4 | 5.06 / 8.96 / 18.43 (11.14–25.60) | 4.61 / 6.78 / 10.37 (7.23–20.74) | 4.16 / 5.38 / 10.88 (9.22–13.70) | 7.10 / 9.73 / 17.92 (14.46–21.25) | 6.72 / 8.45 / 11.26 (8.83–16.64) | 5.95 / 6.66 / 15.10 (11.39–18.69) | 5.95 / 6.91 / 13.95 (10.37–17.92) | 5.95 / 6.14 / 11.52 (9.98–12.67) | 100% | 0 |
| all | 1000 | 1000 | 6 | 6.72 / 13.44 / 19.46 (18.43–23.04) | 7.30 / 13.82 / 18.18 (15.87–19.97) | 8.06 / 22.02 / 23.30 (22.78–24.32) | 11.39 / 16.90 / 22.02 (20.48–27.39) | 11.65 / 15.23 / 19.46 (18.18–22.53) | 11.65 / 17.41 / 23.81 (20.99–25.60) | 11.90 / 15.62 / 22.02 (20.74–24.58) | 10.62 / 12.16 / 16.38 (14.59–17.92) | 68% | 101 |
| all | 1000 | 1000 | 8 | 8.83 / 17.41 / 28.67 (27.14–31.74) | 10.11 / 22.02 / 28.93 (28.42–29.44) | 13.44 / 27.39 / 35.84 (34.82–36.35) | 14.85 / 24.06 / 32.77 (32.51–35.33) | 14.98 / 22.27 / 29.70 (27.65–31.74) | 16.90 / 27.90 / 37.38 (36.86–39.42) | 17.15 / 24.58 / 33.79 (32.77–39.94) | 14.72 / 16.38 / 24.83 (23.55–33.79) | 50% | 47 |

Migrations seen: 99021 over 183815971 samples (one sample per measured operation).


### Suite cf: k-thread process beside single-thread ones, first and fair settings

load average before: { 5.12 5.16 5.30 }
load average after: { 6.84 5.80 5.53 }; 119 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.29 / 0.46 / 0.62 (0.50–0.70) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.46 / 0.62 (0.54–5.44) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 98% | 5 |
| 4 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.42) | 0.21 / 0.25 / 0.25 (0.25–0.37) | 0.29 / 0.42 / 0.50 (0.50–0.74) | 0.21 / 0.21 / 0.25 (0.25–0.42) | 0.29 / 0.50 / 0.66 (0.54–4.61) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 99% | 0 |
| 4 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.29 / 0.58 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.33 / 0.54 / 0.66 (0.62–0.74) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 100% | 0 |
| 4 | 1000 | 1 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.07 / 1.20 / 1.41 (1.28–1.57) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 2.94 / 3.10 / 7.30 (3.62–10.37) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 100% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.07 / 1.25 / 1.33 (1.28–1.54) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 2.98 / 3.33 / 9.73 (3.55–15.36) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 100% | 0 |
| 4 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.07 / 2.11 / 2.27 (2.24–2.27) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 2.98 / 3.26 / 9.98 (3.39–11.78) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 100% | 0 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 0.62 / 0.70 (0.70–0.70) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.42 / 0.66 / 0.79 (0.79–0.83) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 50% | 8 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.70 (0.70–0.70) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.42 / 0.70 / 0.87 (0.79–0.91) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 50% | 0 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.37 / 0.66 / 0.83 (0.83–1.07) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.42 / 0.70 / 0.87 (0.83–0.91) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 50% | 7 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.66 / 1.00 (0.79–1.20) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.42 / 1.20 / 1.70 (1.49–2.00) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 48 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.33 / 0.62 / 0.70 (0.66–0.70) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.42 / 1.25 / 1.79 (1.62–1.90) | 0.21 / 0.33 / 0.33 (0.33–0.37) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 9 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.15 / 1.54 / 1.66 (1.58–1.82) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 3.23 / 3.49 / 9.09 (7.87–11.26) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 5 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.15 / 1.49 / 1.62 (1.58–1.90) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 3.26 / 3.68 / 9.34 (8.70–11.78) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 4 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.12 / 1.90 / 2.40 (2.14–2.56) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 3.23 / 3.68 / 9.98 (8.70–12.67) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 49% | 6 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.15 / 2.14 / 2.56 (2.50–2.72) | 1.00 / 1.07 / 1.12 (1.12–1.12) | 3.23 / 4.42 / 9.98 (5.25–10.75) | 1.00 / 1.07 / 1.12 (1.12–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 23 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.07 / 1.46 / 1.58 (1.57–1.95) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 3.20 / 4.42 / 8.83 (5.12–11.01) | 1.00 / 1.12 / 1.12 (1.12–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 11 |

Independent processes: operations/s

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 1.92M (1.91M–1.93M) | 1.92M (1.91M–1.92M) | 1.83M (1.79M–1.84M) | 1.91M (1.90M–1.92M) | 1.79M (1.73M–1.83M) | 1.91M (1.90M–1.92M) | 1.91M (1.91M–1.92M) | 1.92M (1.90M–1.92M) | 98% | 5 |
| 4 | 200 | 2 | 1.27M (1.26M–1.28M) | 1.28M (1.26M–1.28M) | 1.22M (1.21M–1.23M) | 1.27M (1.26M–1.28M) | 1.21M (1.19M–1.21M) | 1.27M (1.27M–1.28M) | 1.28M (1.27M–1.28M) | 1.27M (1.27M–1.28M) | 99% | 0 |
| 4 | 200 | 3 | 639k (627k–639k) | 638k (638k–642k) | 523k (522k–525k) | 546k (545k–549k) | 601k (595k–604k) | 640k (634k–641k) | 638k (634k–642k) | 638k (635k–640k) | 100% | 0 |
| 4 | 1000 | 1 | 1.27M (1.27M–1.27M) | 1.27M (1.26M–1.27M) | 1.23M (1.22M–1.24M) | 1.27M (1.27M–1.27M) | 697k (695k–698k) | 1.27M (1.27M–1.27M) | 1.27M (1.27M–1.28M) | 1.27M (1.27M–1.28M) | 100% | 0 |
| 4 | 1000 | 2 | 848k (847k–849k) | 848k (845k–848k) | 821k (816k–825k) | 848k (846k–849k) | 460k (456k–462k) | 849k (848k–849k) | 850k (845k–853k) | 848k (846k–851k) | 100% | 0 |
| 4 | 1000 | 3 | 424k (422k–425k) | 423k (422k–425k) | 355k (353k–356k) | 383k (382k–384k) | 229k (229k–230k) | 426k (424k–426k) | 425k (424k–426k) | 424k (423k–426k) | 100% | 0 |
| 8 | 200 | 1 | 4.50M (4.48M–4.50M) | 4.50M (4.49M–4.50M) | 4.13M (4.01M–4.18M) | 4.49M (4.49M–4.49M) | 3.99M (3.94M–4.01M) | 4.49M (4.48M–4.50M) | 4.48M (4.47M–4.50M) | 4.49M (4.48M–4.50M) | 50% | 8 |
| 8 | 200 | 2 | 3.84M (3.83M–3.85M) | 3.85M (3.84M–3.86M) | 3.57M (3.52M–3.65M) | 3.85M (3.84M–3.85M) | 3.41M (3.41M–3.44M) | 3.84M (3.83M–3.85M) | 3.84M (3.83M–3.85M) | 3.84M (3.84M–3.85M) | 50% | 0 |
| 8 | 200 | 3 | 3.20M (3.20M–3.21M) | 3.20M (3.20M–3.21M) | 2.86M (2.79M–2.92M) | 3.10M (3.10M–3.11M) | 2.84M (2.82M–2.86M) | 3.20M (3.19M–3.21M) | 3.20M (3.19M–3.21M) | 3.20M (3.19M–3.21M) | 50% | 7 |
| 8 | 200 | 5 | 1.92M (1.88M–1.93M) | 1.92M (1.92M–1.93M) | 1.64M (1.54M–1.70M) | 1.77M (1.77M–1.79M) | 1.29M (1.27M–1.33M) | 1.47M (1.47M–1.48M) | 1.92M (1.91M–1.92M) | 1.92M (1.92M–1.93M) | 49% | 48 |
| 8 | 200 | 6 | 1.28M (1.28M–1.28M) | 1.28M (1.28M–1.28M) | 1.19M (1.15M–1.22M) | 1.28M (1.28M–1.29M) | 852k (832k–880k) | 975k (968k–980k) | 1.28M (1.27M–1.29M) | 1.28M (1.28M–1.29M) | 49% | 9 |
| 8 | 1000 | 1 | 3.02M (3.02M–3.02M) | 3.02M (3.01M–3.02M) | 2.81M (2.70M–2.82M) | 3.02M (3.01M–3.02M) | 1.53M (1.53M–1.56M) | 3.02M (3.02M–3.03M) | 3.02M (3.02M–3.03M) | 3.02M (3.02M–3.02M) | 49% | 5 |
| 8 | 1000 | 2 | 2.59M (2.59M–2.59M) | 2.59M (2.58M–2.59M) | 2.43M (2.39M–2.44M) | 2.59M (2.58M–2.59M) | 1.31M (1.30M–1.32M) | 2.59M (2.59M–2.60M) | 2.59M (2.59M–2.60M) | 2.59M (2.58M–2.59M) | 49% | 4 |
| 8 | 1000 | 3 | 2.15M (2.15M–2.16M) | 2.16M (2.15M–2.16M) | 1.96M (1.93M–2.02M) | 2.11M (2.10M–2.12M) | 1.09M (1.09M–1.10M) | 2.16M (2.15M–2.17M) | 2.16M (2.16M–2.17M) | 2.16M (2.15M–2.16M) | 49% | 6 |
| 8 | 1000 | 5 | 1.30M (1.29M–1.30M) | 1.29M (1.29M–1.30M) | 1.11M (1.06M–1.14M) | 1.22M (1.21M–1.23M) | 595k (594k–599k) | 1.08M (1.08M–1.09M) | 1.30M (1.30M–1.30M) | 1.30M (1.30M–1.30M) | 49% | 23 |
| 8 | 1000 | 6 | 866k (861k–867k) | 863k (863k–865k) | 830k (811k–833k) | 862k (861k–863k) | 410k (395k–416k) | 724k (722k–727k) | 865k (863k–868k) | 865k (859k–867k) | 49% | 11 |

The k-thread process: operations/s

| n | W ns | k | 1 per-core first | 1 per-core fair | 2 fixed/2 first | 2 fixed/2 fair | 2 fixed/4 first | 2 fixed/4 fair | 3 sized first | 3 sized fair | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 638k (635k–645k) | 637k (628k–645k) | 612k (609k–614k) | 636k (634k–639k) | 596k (582k–609k) | 637k (633k–641k) | 638k (634k–641k) | 638k (631k–639k) | 98% | 5 |
| 4 | 200 | 2 | 1.07M (1.05M–1.07M) | 1.10M (1.09M–1.10M) | 1.22M (1.20M–1.22M) | 1.26M (1.25M–1.26M) | 1.21M (1.19M–1.21M) | 1.26M (1.25M–1.26M) | 1.22M (1.20M–1.23M) | 1.26M (1.25M–1.27M) | 99% | 0 |
| 4 | 200 | 3 | 1.45M (1.45M–1.46M) | 1.51M (1.51M–1.52M) | 1.49M (1.46M–1.49M) | 1.60M (1.60M–1.61M) | 1.80M (1.79M–1.81M) | 1.88M (1.87M–1.89M) | 1.81M (1.80M–1.82M) | 1.88M (1.87M–1.89M) | 100% | 0 |
| 4 | 1000 | 1 | 424k (420k–425k) | 424k (423k–425k) | 411k (408k–412k) | 423k (420k–424k) | 232k (232k–233k) | 425k (421k–427k) | 425k (424k–427k) | 425k (422k–425k) | 100% | 0 |
| 4 | 1000 | 2 | 707k (705k–708k) | 745k (744k–747k) | 819k (815k–821k) | 834k (829k–837k) | 460k (457k–462k) | 838k (835k–838k) | 822k (821k–826k) | 836k (835k–838k) | 100% | 0 |
| 4 | 1000 | 3 | 963k (962k–965k) | 1.04M (1.03M–1.04M) | 985k (983k–988k) | 1.09M (1.09M–1.09M) | 688k (687k–691k) | 1.21M (1.20M–1.21M) | 918k (914k–919k) | 1.21M (1.21M–1.22M) | 100% | 0 |
| 8 | 200 | 1 | 640k (638k–644k) | 640k (638k–642k) | 590k (565k–618k) | 638k (636k–640k) | 571k (562k–576k) | 639k (638k–641k) | 638k (636k–641k) | 638k (637k–640k) | 50% | 8 |
| 8 | 200 | 2 | 1.06M (1.03M–1.07M) | 1.09M (1.06M–1.11M) | 1.17M (1.13M–1.20M) | 1.26M (1.25M–1.27M) | 1.14M (1.13M–1.14M) | 1.25M (1.24M–1.27M) | 1.14M (1.11M–1.17M) | 1.25M (1.25M–1.26M) | 50% | 0 |
| 8 | 200 | 3 | 1.42M (1.40M–1.46M) | 1.50M (1.48M–1.51M) | 1.42M (1.37M–1.45M) | 1.55M (1.52M–1.56M) | 1.69M (1.68M–1.70M) | 1.85M (1.84M–1.86M) | 1.70M (1.66M–1.80M) | 1.85M (1.84M–1.86M) | 50% | 7 |
| 8 | 200 | 5 | 2.08M (1.93M–2.12M) | 2.19M (2.17M–2.25M) | 2.06M (1.90M–2.11M) | 2.24M (2.22M–2.29M) | 2.13M (2.07M–2.20M) | 2.56M (2.54M–2.56M) | 2.62M (2.51M–2.67M) | 3.03M (3.03M–3.04M) | 49% | 48 |
| 8 | 200 | 6 | 2.36M (2.34M–2.38M) | 2.51M (2.49M–2.52M) | 2.32M (2.25M–2.37M) | 2.56M (2.53M–2.63M) | 2.41M (2.37M–2.53M) | 2.88M (2.85M–2.88M) | 2.56M (2.52M–2.70M) | 3.60M (3.60M–3.62M) | 49% | 9 |
| 8 | 1000 | 1 | 431k (430k–431k) | 431k (428k–432k) | 405k (385k–421k) | 431k (429k–432k) | 219k (218k–224k) | 433k (431k–433k) | 433k (432k–434k) | 432k (431k–435k) | 49% | 5 |
| 8 | 1000 | 2 | 710k (701k–716k) | 752k (738k–761k) | 794k (781k–837k) | 833k (828k–838k) | 428k (421k–432k) | 840k (827k–847k) | 796k (769k–840k) | 832k (823k–846k) | 49% | 4 |
| 8 | 1000 | 3 | 960k (946k–964k) | 1.04M (1.03M–1.06M) | 954k (918k–979k) | 1.07M (1.06M–1.08M) | 632k (628k–641k) | 1.20M (1.19M–1.21M) | 848k (830k–856k) | 1.20M (1.19M–1.20M) | 49% | 6 |
| 8 | 1000 | 5 | 1.38M (1.38M–1.39M) | 1.53M (1.52M–1.55M) | 1.36M (1.31M–1.40M) | 1.60M (1.59M–1.61M) | 935k (933k–951k) | 1.67M (1.63M–1.67M) | 826k (819k–841k) | 1.84M (1.83M–1.84M) | 49% | 23 |
| 8 | 1000 | 6 | 1.57M (1.55M–1.58M) | 1.76M (1.75M–1.78M) | 1.55M (1.54M–1.59M) | 1.81M (1.81M–1.84M) | 1.09M (1.06M–1.11M) | 1.91M (1.89M–1.91M) | 822k (821k–833k) | 2.12M (2.12M–2.12M) | 49% | 11 |

Seqlock reads repeated per 1000 lookups: 2 fixed/2 fair: 8.56; 2 fixed/4 fair: 21.84; 3 sized fair: 33.24;

Migrations seen: 80429 over 304204935 samples (one sample per measured operation).


## Appendix E: full tables, third series, pass 1


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 15.0 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a1t: capability operations, fine group locks

load average before: { 4.04 2.62 1.84 }
load average after: { 5.48 4.75 3.33 }; 488 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 1.39M (1.36M–1.40M) | 1.39M (1.38M–1.40M) | 1.40M (1.38M–1.40M) | 1.40M (1.31M–1.41M) | 1.40M (1.39M–1.41M) | 1.41M (1.40M–1.41M) | 1.40M (1.40M–1.40M) | 1.40M (1.40M–1.41M) | 1.40M (1.39M–1.41M) | 99% | 2 |
| 1 | 0 | 50 | 3 | 2.08M (2.07M–2.08M) | 2.07M (2.05M–2.08M) | 2.08M (2.08M–2.09M) | 2.09M (2.07M–2.09M) | 2.09M (2.09M–2.10M) | 2.10M (2.09M–2.10M) | 2.08M (1.91M–2.10M) | 2.10M (2.07M–2.10M) | 2.09M (2.09M–2.10M) | 99% | 3 |
| 1 | 0 | 50 | 4 | 2.75M (2.73M–2.76M) | 2.76M (2.75M–2.78M) | 2.75M (2.75M–2.78M) | 2.76M (2.74M–2.79M) | 2.77M (2.75M–2.79M) | 2.78M (2.75M–2.80M) | 2.77M (2.74M–2.80M) | 2.78M (2.75M–2.79M) | 2.79M (2.76M–2.80M) | 99% | 0 |
| 1 | 0 | 50 | 6 | 4.05M (4.02M–4.06M) | 4.07M (4.00M–4.09M) | 4.07M (4.03M–4.08M) | 4.11M (4.05M–4.12M) | 4.09M (3.97M–4.13M) | 4.13M (4.10M–4.16M) | 4.13M (4.12M–4.16M) | 4.17M (4.15M–4.20M) | 4.18M (4.15M–4.20M) | 65% | 32 |
| 1 | 0 | 50 | 8 | 5.38M (5.26M–5.43M) | 5.37M (3.60M–5.45M) | 5.40M (5.11M–5.45M) | 5.43M (5.37M–5.46M) | 5.46M (5.36M–5.48M) | 5.49M (5.38M–5.50M) | 5.55M (5.42M–5.57M) | 5.57M (5.49M–5.59M) | 5.56M (5.49M–5.58M) | 49% | 170 |
| 1 | 0 | 200 | 2 | 1.27M (1.27M–1.28M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 1.28M (1.28M–1.29M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 1.29M (1.28M–1.29M) | 1.29M (1.27M–1.29M) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.89M (1.89M–1.91M) | 1.91M (1.90M–1.92M) | 1.91M (1.90M–1.92M) | 1.91M (1.67M–1.92M) | 1.92M (1.89M–1.93M) | 1.92M (1.90M–1.92M) | 1.92M (1.91M–1.93M) | 1.92M (1.92M–1.93M) | 1.92M (1.91M–1.93M) | 99% | 11 |
| 1 | 0 | 200 | 4 | 2.53M (2.51M–2.54M) | 2.53M (2.50M–2.55M) | 2.53M (2.52M–2.55M) | 2.53M (2.52M–2.55M) | 2.54M (2.31M–2.57M) | 2.54M (2.54M–2.58M) | 2.54M (2.52M–2.58M) | 2.55M (2.54M–2.56M) | 2.56M (2.55M–2.57M) | 99% | 2 |
| 1 | 0 | 200 | 6 | 3.80M (3.75M–3.80M) | 3.81M (3.78M–3.82M) | 3.81M (3.79M–3.82M) | 3.82M (3.42M–3.84M) | 3.84M (3.80M–3.86M) | 3.84M (3.82M–3.85M) | 3.88M (3.87M–3.90M) | 3.90M (3.88M–3.91M) | 3.89M (3.87M–3.91M) | 65% | 40 |
| 1 | 0 | 200 | 8 | 4.93M (4.91M–4.94M) | 4.98M (4.96M–5.00M) | 4.98M (4.80M–4.99M) | 4.96M (4.89M–4.99M) | 5.02M (4.93M–5.04M) | 5.01M (5.00M–5.05M) | 5.08M (5.07M–5.11M) | 5.12M (5.09M–5.13M) | 5.10M (5.04M–5.12M) | 49% | 10 |
| 1 | 0 | 1000 | 2 | 837k (834k–842k) | 844k (838k–847k) | 845k (841k–850k) | 849k (845k–852k) | 849k (844k–850k) | 849k (848k–852k) | 847k (847k–852k) | 850k (846k–852k) | 850k (849k–852k) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.25M (1.25M–1.25M) | 1.26M (1.26M–1.26M) | 1.26M (1.25M–1.26M) | 1.27M (1.26M–1.27M) | 1.26M (1.26M–1.27M) | 1.27M (1.27M–1.28M) | 1.27M (1.26M–1.27M) | 1.27M (1.26M–1.28M) | 1.27M (1.27M–1.28M) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.66M (1.65M–1.66M) | 1.68M (1.68M–1.69M) | 1.68M (1.67M–1.68M) | 1.69M (1.67M–1.69M) | 1.68M (1.67M–1.69M) | 1.69M (1.69M–1.71M) | 1.67M (1.67M–1.68M) | 1.69M (1.68M–1.70M) | 1.69M (1.67M–1.70M) | 99% | 0 |
| 1 | 0 | 1000 | 6 | 2.46M (2.41M–2.49M) | 2.53M (2.51M–2.53M) | 2.51M (2.50M–2.51M) | 2.53M (2.52M–2.53M) | 2.51M (2.51M–2.51M) | 2.55M (2.54M–2.55M) | 2.52M (2.51M–2.52M) | 2.56M (2.51M–2.58M) | 2.57M (2.55M–2.57M) | 65% | 28 |
| 1 | 0 | 1000 | 8 | 3.26M (3.09M–3.27M) | 3.35M (3.31M–3.36M) | 3.32M (3.29M–3.35M) | 3.35M (3.11M–3.38M) | 3.33M (3.32M–3.35M) | 3.40M (3.37M–3.41M) | 3.34M (3.33M–3.35M) | 3.44M (3.44M–3.45M) | 3.44M (3.40M–3.45M) | 49% | 68 |
| 1 | 1000 | 50 | 2 | 1.38M (1.37M–1.38M) | 1.38M (1.38M–1.39M) | 1.41M (1.40M–1.41M) | 1.40M (1.40M–1.41M) | 1.40M (1.39M–1.41M) | 1.40M (1.40M–1.41M) | 1.40M (1.39M–1.41M) | 1.41M (1.39M–1.41M) | 1.40M (1.40M–1.41M) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.02M (2.00M–2.04M) | 2.04M (2.01M–2.04M) | 2.05M (2.03M–2.06M) | 2.05M (2.03M–2.06M) | 2.08M (2.06M–2.09M) | 2.08M (1.97M–2.10M) | 2.08M (2.06M–2.10M) | 2.09M (2.08M–2.09M) | 2.08M (2.07M–2.10M) | 99% | 3 |
| 1 | 1000 | 50 | 4 | 2.66M (2.63M–2.68M) | 2.69M (2.64M–2.70M) | 2.70M (2.67M–2.71M) | 2.71M (2.69M–2.72M) | 2.76M (2.75M–2.80M) | 2.78M (2.76M–2.80M) | 2.77M (2.75M–2.78M) | 2.78M (2.77M–2.80M) | 2.77M (2.77M–2.80M) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.88M (3.83M–3.91M) | 3.93M (3.91M–3.94M) | 3.96M (3.94M–3.97M) | 3.96M (3.94M–3.97M) | 4.02M (4.00M–4.03M) | 4.05M (4.02M–4.05M) | 4.15M (4.12M–4.17M) | 4.19M (4.16M–4.21M) | 4.20M (4.18M–4.21M) | 66% | 20 |
| 1 | 1000 | 50 | 8 | 5.12M (5.11M–5.15M) | 5.15M (5.09M–5.16M) | 5.17M (5.12M–5.19M) | 5.17M (5.09M–5.19M) | 5.26M (4.57M–5.29M) | 5.28M (4.84M–5.30M) | 5.55M (5.41M–5.58M) | 5.58M (5.14M–5.60M) | 5.58M (5.57M–5.60M) | 49% | 29 |
| 1 | 1000 | 200 | 2 | 1.26M (1.25M–1.26M) | 1.27M (1.26M–1.27M) | 1.29M (1.28M–1.29M) | 1.28M (1.28M–1.28M) | 1.28M (1.28M–1.29M) | 1.28M (1.28M–1.29M) | 1.28M (1.28M–1.29M) | 1.28M (1.28M–1.29M) | 1.28M (1.28M–1.29M) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 1.86M (1.85M–1.87M) | 1.88M (1.86M–1.88M) | 1.88M (1.87M–1.90M) | 1.89M (1.88M–1.90M) | 1.92M (1.91M–1.93M) | 1.92M (1.90M–1.94M) | 1.91M (1.91M–1.94M) | 1.91M (1.91M–1.93M) | 1.91M (1.91M–1.94M) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 2.45M (2.44M–2.46M) | 2.46M (2.45M–2.49M) | 2.49M (2.48M–2.51M) | 2.49M (2.48M–2.50M) | 2.55M (2.54M–2.57M) | 2.56M (2.55M–2.56M) | 2.56M (2.54M–2.57M) | 2.55M (2.54M–2.57M) | 2.55M (2.53M–2.57M) | 99% | 0 |
| 1 | 1000 | 200 | 6 | 3.64M (3.61M–3.65M) | 3.66M (3.64M–3.67M) | 3.67M (3.66M–3.70M) | 3.69M (3.67M–3.70M) | 3.74M (3.73M–3.76M) | 3.74M (3.71M–3.76M) | 3.86M (3.57M–3.90M) | 3.87M (3.86M–3.90M) | 3.88M (3.85M–3.93M) | 64% | 36 |
| 1 | 1000 | 200 | 8 | 4.64M (4.51M–4.70M) | 4.74M (4.63M–4.78M) | 4.74M (4.63M–4.78M) | 4.76M (4.63M–4.79M) | 4.84M (4.80M–4.88M) | 4.84M (4.66M–4.87M) | 5.08M (5.02M–5.11M) | 5.10M (4.78M–5.13M) | 5.11M (5.09M–5.12M) | 48% | 41 |
| 1 | 1000 | 1000 | 2 | 832k (827k–836k) | 839k (831k–841k) | 846k (798k–849k) | 849k (844k–852k) | 847k (845k–850k) | 848k (845k–852k) | 848k (845k–851k) | 850k (848k–852k) | 851k (848k–854k) | 100% | 3 |
| 1 | 1000 | 1000 | 3 | 1.23M (1.23M–1.24M) | 1.25M (1.24M–1.25M) | 1.25M (1.23M–1.26M) | 1.25M (1.24M–1.26M) | 1.26M (1.25M–1.27M) | 1.27M (1.25M–1.27M) | 1.26M (1.26M–1.26M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 99% | 0 |
| 1 | 1000 | 1000 | 4 | 1.62M (1.61M–1.63M) | 1.65M (1.63M–1.65M) | 1.65M (1.64M–1.66M) | 1.66M (1.65M–1.67M) | 1.67M (1.65M–1.68M) | 1.69M (1.62M–1.69M) | 1.67M (1.66M–1.68M) | 1.68M (1.67M–1.69M) | 1.68M (1.67M–1.69M) | 98% | 7 |
| 1 | 1000 | 1000 | 6 | 2.39M (1.83M–2.40M) | 2.44M (2.42M–2.45M) | 2.46M (2.44M–2.47M) | 2.47M (2.46M–2.48M) | 2.48M (2.47M–2.49M) | 2.50M (2.50M–2.51M) | 2.50M (2.48M–2.53M) | 2.57M (2.54M–2.57M) | 2.56M (2.56M–2.58M) | 65% | 73 |
| 1 | 1000 | 1000 | 8 | 3.15M (2.78M–3.17M) | 3.23M (3.11M–3.25M) | 3.25M (3.20M–3.26M) | 3.26M (3.24M–3.29M) | 3.28M (3.24M–3.29M) | 3.33M (3.27M–3.33M) | 3.33M (3.28M–3.34M) | 3.45M (3.41M–3.45M) | 3.44M (3.38M–3.45M) | 48% | 43 |
| 10 | 0 | 50 | 2 | 1.32M (1.31M–1.33M) | 1.34M (1.33M–1.35M) | 1.38M (1.37M–1.39M) | 1.38M (1.38M–1.38M) | 1.37M (1.36M–1.38M) | 1.38M (1.37M–1.39M) | 1.37M (1.37M–1.38M) | 1.38M (1.38M–1.39M) | 1.38M (1.37M–1.39M) | 100% | 0 |
| 10 | 0 | 50 | 3 | 1.91M (1.87M–1.93M) | 1.95M (1.94M–1.97M) | 1.96M (1.95M–1.98M) | 1.97M (1.95M–1.99M) | 2.03M (2.03M–2.06M) | 2.05M (2.04M–2.07M) | 2.04M (2.04M–2.06M) | 2.06M (2.04M–2.08M) | 2.05M (2.03M–2.08M) | 100% | 0 |
| 10 | 0 | 50 | 4 | 2.49M (2.43M–2.51M) | 2.56M (2.52M–2.57M) | 2.57M (2.54M–2.58M) | 2.58M (2.54M–2.60M) | 2.72M (2.70M–2.75M) | 2.73M (2.70M–2.76M) | 2.71M (2.69M–2.75M) | 2.74M (2.71M–2.76M) | 2.74M (2.71M–2.75M) | 99% | 0 |
| 10 | 0 | 50 | 6 | 3.35M (3.28M–3.39M) | 3.47M (3.45M–3.56M) | 3.47M (3.36M–3.52M) | 3.45M (2.91M–3.52M) | 3.66M (3.58M–3.69M) | 3.69M (3.20M–3.72M) | 4.03M (3.97M–4.05M) | 4.07M (4.06M–4.08M) | 4.04M (4.02M–4.09M) | 66% | 112 |
| 10 | 0 | 50 | 8 | 4.16M (4.15M–4.16M) | 4.40M (4.33M–4.42M) | 4.35M (4.20M–4.41M) | 4.39M (4.33M–4.43M) | 4.68M (4.64M–4.69M) | 4.72M (4.70M–4.74M) | 5.31M (5.22M–5.35M) | 5.41M (5.40M–5.44M) | 5.41M (5.37M–5.42M) | 49% | 28 |
| 10 | 0 | 200 | 2 | 1.20M (1.19M–1.21M) | 1.22M (1.21M–1.23M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.27M–1.27M) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.73M (1.72M–1.75M) | 1.78M (1.76M–1.80M) | 1.80M (1.79M–1.81M) | 1.81M (1.79M–1.82M) | 1.88M (1.86M–1.89M) | 1.88M (1.81M–1.90M) | 1.88M (1.86M–1.89M) | 1.88M (1.87M–1.89M) | 1.89M (1.84M–1.89M) | 99% | 4 |
| 10 | 0 | 200 | 4 | 2.25M (2.21M–2.25M) | 2.33M (1.77M–2.33M) | 2.35M (2.18M–2.37M) | 2.37M (2.11M–2.38M) | 2.50M (2.32M–2.51M) | 2.49M (2.43M–2.53M) | 2.48M (2.46M–2.51M) | 2.50M (2.50M–2.53M) | 2.52M (2.42M–2.53M) | 95% | 21 |
| 10 | 0 | 200 | 6 | 3.03M (2.95M–3.05M) | 3.23M (3.16M–3.25M) | 3.18M (3.18M–3.25M) | 3.15M (3.08M–3.20M) | 3.36M (3.33M–3.39M) | 3.42M (3.40M–3.44M) | 3.66M (3.64M–3.67M) | 3.76M (3.58M–3.79M) | 3.75M (3.60M–3.78M) | 66% | 31 |
| 10 | 0 | 200 | 8 | 3.71M (2.43M–3.76M) | 3.99M (2.53M–4.06M) | 3.95M (3.76M–4.02M) | 4.00M (3.35M–4.04M) | 4.26M (4.20M–4.30M) | 4.34M (4.30M–4.38M) | 4.67M (4.59M–4.68M) | 4.97M (4.96M–5.00M) | 4.93M (4.56M–4.94M) | 49% | 377 |
| 10 | 0 | 1000 | 2 | 761k (759k–764k) | 798k (795k–801k) | 831k (824k–835k) | 844k (842k–846k) | 835k (830k–838k) | 844k (743k–845k) | 835k (778k–838k) | 844k (739k–847k) | 842k (790k–845k) | 99% | 11 |
| 10 | 0 | 1000 | 3 | 1.08M (1.06M–1.08M) | 1.16M (1.14M–1.16M) | 1.16M (1.16M–1.17M) | 1.19M (1.17M–1.20M) | 1.20M (1.19M–1.21M) | 1.25M (1.25M–1.26M) | 1.20M (1.20M–1.21M) | 1.25M (1.25M–1.25M) | 1.25M (1.24M–1.26M) | 99% | 1 |
| 10 | 0 | 1000 | 4 | 1.37M (1.36M–1.38M) | 1.50M (1.50M–1.51M) | 1.49M (1.35M–1.49M) | 1.56M (1.51M–1.56M) | 1.54M (1.50M–1.54M) | 1.66M (1.64M–1.66M) | 1.54M (1.54M–1.54M) | 1.66M (1.65M–1.66M) | 1.66M (1.66M–1.67M) | 99% | 18 |
| 10 | 0 | 1000 | 6 | 1.83M (1.81M–1.84M) | 2.07M (2.05M–2.09M) | 2.02M (2.01M–2.04M) | 2.14M (2.09M–2.15M) | 2.05M (2.02M–2.06M) | 2.27M (2.26M–2.29M) | 2.10M (2.09M–2.11M) | 2.48M (2.45M–2.49M) | 2.48M (2.46M–2.49M) | 66% | 38 |
| 10 | 0 | 1000 | 8 | 2.23M (2.16M–2.25M) | 2.59M (1.90M–2.67M) | 2.47M (2.44M–2.53M) | 2.73M (2.60M–2.78M) | 2.58M (2.55M–2.59M) | 2.93M (2.87M–2.95M) | 2.59M (2.58M–2.59M) | 3.29M (3.25M–3.30M) | 3.30M (3.23M–3.32M) | 49% | 151 |
| 10 | 1000 | 50 | 2 | 1.17M (1.16M–1.17M) | 1.20M (1.20M–1.20M) | 1.38M (1.36M–1.39M) | 1.39M (1.38M–1.40M) | 1.38M (1.37M–1.39M) | 1.39M (1.38M–1.39M) | 1.38M (1.38M–1.39M) | 1.38M (1.38M–1.39M) | 1.38M (1.38M–1.39M) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 1.59M (1.59M–1.60M) | 1.65M (1.63M–1.65M) | 1.75M (1.74M–1.75M) | 1.75M (1.75M–1.76M) | 2.06M (2.04M–2.07M) | 2.07M (2.06M–2.08M) | 2.07M (2.05M–2.08M) | 2.07M (2.06M–2.09M) | 2.07M (2.06M–2.08M) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.00M (1.99M–2.00M) | 2.09M (2.07M–2.09M) | 2.19M (2.18M–2.21M) | 2.20M (2.19M–2.21M) | 2.72M (2.71M–2.75M) | 2.74M (2.72M–2.77M) | 2.72M (2.55M–2.72M) | 2.73M (2.72M–2.75M) | 2.74M (2.70M–2.76M) | 99% | 0 |
| 10 | 1000 | 50 | 6 | 2.60M (2.58M–2.65M) | 2.71M (2.64M–2.75M) | 2.79M (2.75M–2.83M) | 2.82M (2.79M–2.87M) | 3.08M (3.06M–3.12M) | 3.12M (2.71M–3.14M) | 4.06M (4.02M–4.07M) | 4.07M (4.05M–4.09M) | 4.06M (4.05M–4.08M) | 66% | 47 |
| 10 | 1000 | 50 | 8 | 3.14M (3.09M–3.22M) | 3.33M (3.20M–3.35M) | 3.35M (3.16M–3.46M) | 3.39M (3.35M–3.47M) | 3.81M (3.69M–3.82M) | 3.86M (3.84M–3.89M) | 5.33M (5.31M–5.37M) | 5.42M (5.38M–5.44M) | 5.43M (5.37M–5.43M) | 49% | 0 |
| 10 | 1000 | 200 | 2 | 1.07M (1.07M–1.07M) | 1.10M (1.09M–1.11M) | 1.27M (1.26M–1.28M) | 1.27M (1.25M–1.27M) | 1.26M (1.26M–1.27M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.27M–1.28M) | 1.27M (1.26M–1.27M) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 1.46M (1.45M–1.46M) | 1.52M (1.52M–1.52M) | 1.61M (1.59M–1.61M) | 1.62M (1.61M–1.62M) | 1.89M (1.88M–1.89M) | 1.90M (1.89M–1.90M) | 1.90M (1.88M–1.90M) | 1.89M (1.89M–1.91M) | 1.89M (1.84M–1.90M) | 99% | 3 |
| 10 | 1000 | 200 | 4 | 1.83M (1.82M–1.84M) | 1.92M (1.91M–1.93M) | 2.03M (2.02M–2.04M) | 2.05M (2.04M–2.05M) | 2.51M (2.48M–2.52M) | 2.51M (2.50M–2.53M) | 2.49M (2.48M–2.51M) | 2.52M (2.51M–2.53M) | 2.51M (2.50M–2.52M) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 2.37M (2.36M–2.43M) | 2.51M (2.46M–2.54M) | 2.64M (2.61M–2.70M) | 2.61M (2.60M–2.69M) | 2.84M (2.80M–2.90M) | 2.90M (2.85M–2.94M) | 3.65M (3.52M–3.65M) | 3.76M (3.76M–3.79M) | 3.76M (3.73M–3.77M) | 65% | 21 |
| 10 | 1000 | 200 | 8 | 2.83M (2.72M–2.92M) | 3.10M (2.97M–3.11M) | 3.17M (3.01M–3.22M) | 3.22M (3.04M–3.26M) | 3.56M (3.54M–3.57M) | 3.61M (3.55M–3.62M) | 4.66M (4.60M–4.68M) | 4.96M (4.89M–4.98M) | 4.95M (4.87M–4.97M) | 49% | 50 |
| 10 | 1000 | 1000 | 2 | 707k (704k–709k) | 746k (744k–749k) | 829k (753k–831k) | 839k (782k–841k) | 826k (822k–836k) | 841k (797k–843k) | 825k (820k–827k) | 839k (798k–842k) | 839k (830k–841k) | 98% | 7 |
| 10 | 1000 | 1000 | 3 | 965k (957k–969k) | 1.04M (1.03M–1.04M) | 1.09M (1.08M–1.09M) | 1.11M (1.11M–1.11M) | 1.21M (1.20M–1.21M) | 1.25M (1.25M–1.26M) | 1.21M (1.10M–1.21M) | 1.25M (1.25M–1.26M) | 1.25M (1.25M–1.26M) | 99% | 13 |
| 10 | 1000 | 1000 | 4 | 1.20M (1.18M–1.21M) | 1.31M (1.29M–1.32M) | 1.39M (1.36M–1.39M) | 1.42M (1.41M–1.43M) | 1.55M (1.48M–1.55M) | 1.67M (1.64M–1.68M) | 1.54M (1.42M–1.55M) | 1.66M (1.64M–1.67M) | 1.66M (1.65M–1.67M) | 97% | 18 |
| 10 | 1000 | 1000 | 6 | 1.57M (1.55M–1.58M) | 1.77M (1.73M–1.77M) | 1.84M (1.80M–1.85M) | 1.88M (1.86M–1.90M) | 1.90M (1.86M–1.90M) | 2.04M (2.03M–2.07M) | 2.10M (2.09M–2.10M) | 2.48M (2.46M–2.49M) | 2.48M (2.47M–2.50M) | 66% | 50 |
| 10 | 1000 | 1000 | 8 | 1.85M (1.82M–1.88M) | 2.11M (2.10M–2.17M) | 2.23M (2.19M–2.27M) | 2.31M (2.24M–2.35M) | 2.37M (2.35M–2.39M) | 2.59M (2.57M–2.61M) | 2.59M (2.57M–2.59M) | 3.28M (3.16M–3.30M) | 3.31M (3.29M–3.32M) | 49% | 75 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.08 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.25 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.29) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 99% | 2 |
| 1 | 0 | 50 | 3 | 0.08 / 0.08 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.25) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.37) | 0.08 / 0.12 / 0.12 (0.12–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 99% | 3 |
| 1 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.25 (0.25–0.33) | 0.08 / 0.12 / 0.21 (0.17–0.21) | 0.08 / 0.12 / 0.33 (0.29–2.24) | 0.08 / 0.12 / 0.21 (0.17–0.25) | 0.08 / 0.12 / 0.33 (0.29–2.14) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.21 (0.12–0.25) | 99% | 0 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.37 (0.33–0.46) | 0.08 / 0.17 / 0.21 (0.21–0.25) | 0.08 / 0.17 / 0.42 (0.42–0.46) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.46 (0.46–0.50) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 65% | 32 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.21) | 0.08 / 0.17 / 0.37 (0.33–0.46) | 0.08 / 0.17 / 0.21 (0.21–0.25) | 0.08 / 0.21 / 0.46 (0.46–0.50) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.25 / 0.50 (0.50–0.54) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 49% | 170 |
| 1 | 0 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.58 (0.54–0.66) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.58 (0.54–0.62) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.58 (0.58–0.62) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 100% | 0 |
| 1 | 0 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.62 (0.54–0.62) | 0.21 / 0.25 / 0.29 (0.25–0.50) | 0.21 / 0.25 / 0.66 (0.66–2.24) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 0.21 / 0.25 / 0.66 (0.66–0.70) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 99% | 11 |
| 1 | 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.21 / 0.25 / 0.62 (0.58–1.49) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 0.21 / 0.29 / 0.70 (0.66–2.69) | 0.21 / 0.25 / 0.37 (0.33–0.46) | 0.21 / 0.29 / 0.70 (0.66–2.62) | 0.21 / 0.25 / 0.33 (0.33–0.42) | 0.21 / 0.25 / 0.37 (0.33–0.37) | 99% | 2 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.33 / 0.74 (0.70–0.78) | 0.21 / 0.29 / 0.37 (0.33–0.37) | 0.21 / 0.37 / 0.79 (0.79–0.83) | 0.21 / 0.29 / 0.42 (0.37–0.42) | 0.21 / 0.50 / 0.87 (0.83–0.91) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 65% | 40 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.33 / 0.74 (0.70–0.79) | 0.21 / 0.33 / 0.37 (0.33–0.37) | 0.21 / 0.50 / 0.83 (0.79–0.87) | 0.21 / 0.33 / 0.46 (0.42–0.46) | 0.21 / 0.66 / 0.91 (0.87–0.91) | 0.21 / 0.29 / 0.50 (0.50–0.54) | 0.21 / 0.29 / 0.50 (0.50–0.54) | 49% | 10 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.15) | 1.00 / 1.00 / 1.07 (1.04–2.00) | 1.00 / 1.04 / 2.94 (2.94–4.10) | 1.00 / 1.04 / 1.28 (1.20–2.03) | 1.00 / 1.04 / 2.94 (2.85–3.78) | 1.00 / 1.04 / 1.28 (1.20–1.54) | 1.00 / 1.04 / 2.94 (2.94–2.94) | 1.00 / 1.04 / 1.41 (1.25–2.14) | 1.00 / 1.04 / 1.33 (1.25–1.49) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.12 (1.07–1.15) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.04 / 2.91 (2.91–3.46) | 1.00 / 1.04 / 1.20 (1.12–1.54) | 1.00 / 1.70 / 3.04 (3.04–3.07) | 1.00 / 1.04 / 1.66 (1.62–1.79) | 1.00 / 1.66 / 3.04 (2.98–3.04) | 1.00 / 1.04 / 1.74 (1.62–5.12) | 1.00 / 1.04 / 1.66 (1.62–1.87) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.82 / 2.98 (2.91–2.98) | 1.00 / 1.04 / 1.33 (1.15–2.08) | 1.00 / 2.21 / 3.07 (3.04–6.66) | 1.00 / 1.07 / 1.87 (1.74–5.57) | 1.00 / 2.24 / 3.07 (3.04–4.86) | 1.00 / 1.04 / 1.87 (1.78–7.49) | 1.00 / 1.04 / 1.90 (1.82–5.95) | 99% | 0 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.15 (1.07–8.32) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 2.11 / 3.14 (3.10–3.14) | 1.00 / 1.12 / 1.41 (1.33–1.66) | 1.00 / 2.43 / 3.26 (3.23–3.87) | 1.00 / 1.12 / 1.95 (1.90–2.03) | 1.00 / 2.66 / 3.33 (3.26–3.74) | 1.00 / 1.12 / 2.08 (2.00–2.11) | 1.00 / 1.12 / 2.03 (2.00–3.74) | 65% | 28 |
| 1 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–5.25) | 1.00 / 1.07 / 1.07 (1.07–4.74) | 1.00 / 2.21 / 3.14 (3.10–5.25) | 1.04 / 1.12 / 1.46 (1.28–9.98) | 1.00 / 2.50 / 3.52 (3.39–4.06) | 1.04 / 1.12 / 1.95 (1.90–2.40) | 1.00 / 2.82 / 3.81 (3.74–3.90) | 1.00 / 1.12 / 2.08 (2.08–2.11) | 1.00 / 1.12 / 2.08 (2.08–2.14) | 49% | 68 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.08 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.25 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.33) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.21 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 99% | 3 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.33) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.33) | 0.08 / 0.12 / 0.21 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.37 (0.33–0.37) | 0.08 / 0.21 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.46 (0.42–0.46) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 66% | 20 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.42 (0.33–0.50) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.46 (0.46–0.54) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.25 / 0.54 (0.46–0.54) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 49% | 29 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.58 (0.58–0.58) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.66 (0.66–0.66) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.66 (0.66–0.66) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.25 / 0.54 (0.54–0.62) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.21 / 0.29 / 0.70 (0.70–0.74) | 0.21 / 0.25 / 0.33 (0.33–0.46) | 0.21 / 0.25 / 0.70 (0.66–0.74) | 0.21 / 0.25 / 0.37 (0.33–0.42) | 0.21 / 0.25 / 0.33 (0.33–0.46) | 99% | 0 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.37 / 0.70 (0.66–0.74) | 0.21 / 0.33 / 0.37 (0.33–0.37) | 0.21 / 0.37 / 0.83 (0.79–0.83) | 0.21 / 0.33 / 0.42 (0.37–0.46) | 0.21 / 0.54 / 0.87 (0.83–0.87) | 0.21 / 0.29 / 0.46 (0.46–0.70) | 0.21 / 0.29 / 0.46 (0.46–0.50) | 64% | 36 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.33 / 0.74 (0.70–0.74) | 0.21 / 0.33 / 0.37 (0.33–0.58) | 0.21 / 0.46 / 0.83 (0.83–0.87) | 0.21 / 0.33 / 0.42 (0.42–0.54) | 0.21 / 0.62 / 0.91 (0.87–1.00) | 0.21 / 0.29 / 0.50 (0.50–0.54) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 48% | 41 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.04 / 1.12 (1.04–4.03) | 1.00 / 1.04 / 1.07 (1.04–5.18) | 1.00 / 1.04 / 2.98 (2.94–9.34) | 1.00 / 1.04 / 1.36 (1.25–4.35) | 1.00 / 1.04 / 2.91 (2.91–4.42) | 1.00 / 1.04 / 1.49 (1.28–4.54) | 1.00 / 1.04 / 2.94 (2.91–3.04) | 1.00 / 1.04 / 1.41 (1.15–1.66) | 1.00 / 1.04 / 1.33 (1.15–1.90) | 100% | 3 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–3.14) | 1.00 / 1.00 / 1.07 (1.04–3.90) | 1.00 / 1.04 / 2.82 (2.78–6.98) | 1.00 / 1.04 / 1.25 (1.15–7.10) | 1.00 / 1.74 / 3.04 (2.98–5.57) | 1.00 / 1.04 / 1.78 (1.57–9.34) | 1.00 / 1.82 / 3.04 (2.98–4.54) | 1.00 / 1.04 / 1.74 (1.70–6.85) | 1.00 / 1.04 / 1.66 (1.57–9.09) | 99% | 0 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.00 / 1.07 (1.04–5.82) | 1.00 / 1.04 / 1.07 (1.04–5.89) | 1.00 / 1.04 / 2.72 (2.66–7.10) | 1.00 / 1.04 / 1.33 (1.20–6.40) | 1.00 / 2.24 / 3.07 (3.07–7.62) | 1.00 / 1.07 / 1.95 (1.82–8.83) | 1.00 / 2.24 / 3.10 (3.04–5.44) | 1.00 / 1.04 / 1.90 (1.79–7.74) | 1.00 / 1.07 / 2.03 (1.70–6.59) | 98% | 7 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–7.17) | 1.00 / 1.07 / 1.12 (1.07–1.62) | 1.00 / 1.15 / 2.78 (2.69–6.85) | 1.00 / 1.12 / 1.28 (1.28–2.14) | 1.00 / 2.27 / 3.20 (3.14–3.46) | 1.00 / 1.07 / 1.90 (1.87–2.11) | 1.00 / 2.69 / 3.26 (3.23–5.38) | 1.00 / 1.12 / 2.03 (2.00–6.66) | 1.00 / 1.07 / 2.03 (2.00–2.14) | 65% | 73 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–7.87) | 1.00 / 1.20 / 2.69 (2.66–6.59) | 1.04 / 1.12 / 1.41 (1.25–2.72) | 1.00 / 2.24 / 3.36 (3.20–5.82) | 1.04 / 1.12 / 1.90 (1.87–6.72) | 1.00 / 2.85 / 3.81 (3.74–7.74) | 1.00 / 1.12 / 2.11 (2.08–4.48) | 1.04 / 1.12 / 2.11 (2.08–6.91) | 48% | 43 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.17 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.21 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.29 / 0.37 (0.37–0.42) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.25 / 0.37 (0.33–0.42) | 0.08 / 0.12 / 0.21 (0.21–0.29) | 0.08 / 0.29 / 0.46 (0.42–0.54) | 0.08 / 0.12 / 0.25 (0.25–0.33) | 0.08 / 0.29 / 0.46 (0.42–0.54) | 0.08 / 0.12 / 0.25 (0.25–0.33) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 99% | 0 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.33 / 0.54 (0.50–0.58) | 0.08 / 0.21 / 0.33 (0.29–0.37) | 0.12 / 0.42 / 0.54 (0.54–0.62) | 0.08 / 0.21 / 0.37 (0.33–0.37) | 0.08 / 0.46 / 0.74 (0.74–0.79) | 0.08 / 0.25 / 0.42 (0.37–0.46) | 0.08 / 0.25 / 0.42 (0.37–0.46) | 66% | 112 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.37 / 0.50 (0.50–0.54) | 0.12 / 0.21 / 0.33 (0.33–0.33) | 0.12 / 0.42 / 0.58 (0.54–0.58) | 0.12 / 0.21 / 0.37 (0.37–0.37) | 0.12 / 0.50 / 0.83 (0.79–0.83) | 0.12 / 0.25 / 0.42 (0.37–0.46) | 0.12 / 0.29 / 0.46 (0.42–0.50) | 49% | 28 |
| 10 | 0 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.42 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.42 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.42 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 100% | 0 |
| 10 | 0 | 200 | 3 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.58 / 0.74 (0.74–0.74) | 0.21 / 0.25 / 0.46 (0.46–0.50) | 0.21 / 0.58 / 0.74 (0.74–0.95) | 0.21 / 0.25 / 0.50 (0.50–1.04) | 0.21 / 0.58 / 0.74 (0.74–1.07) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.29 / 0.50 (0.50–0.58) | 99% | 4 |
| 10 | 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–7.62) | 0.21 / 0.58 / 0.74 (0.74–6.53) | 0.21 / 0.29 / 0.46 (0.46–6.78) | 0.21 / 0.66 / 1.07 (0.95–6.66) | 0.21 / 0.33 / 0.50 (0.50–4.10) | 0.21 / 0.66 / 1.04 (0.95–1.70) | 0.21 / 0.33 / 0.50 (0.50–0.54) | 0.21 / 0.29 / 0.50 (0.50–0.54) | 95% | 21 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 1.00 (0.91–1.00) | 0.21 / 0.37 / 0.58 (0.50–0.58) | 0.25 / 0.83 / 1.36 (1.33–1.41) | 0.21 / 0.37 / 0.62 (0.58–0.62) | 0.25 / 1.15 / 1.74 (1.74–1.74) | 0.21 / 0.46 / 0.66 (0.62–0.66) | 0.21 / 0.46 / 0.70 (0.66–0.74) | 66% | 31 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.37) | 0.25 / 0.70 / 1.00 (0.91–1.04) | 0.25 / 0.33 / 0.54 (0.50–0.58) | 0.25 / 0.87 / 1.36 (1.36–1.46) | 0.25 / 0.37 / 0.62 (0.58–0.62) | 0.25 / 1.46 / 2.24 (2.21–2.53) | 0.21 / 0.50 / 0.66 (0.66–0.66) | 0.21 / 0.54 / 0.74 (0.70–0.74) | 49% | 377 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 2.94 / 3.10 (3.10–3.10) | 1.00 / 1.07 / 1.95 (1.90–2.00) | 1.00 / 2.94 / 3.10 (3.10–3.10) | 1.00 / 1.07 / 1.95 (1.90–16.90) | 1.00 / 2.94 / 3.10 (3.10–16.64) | 1.00 / 1.07 / 1.87 (1.74–27.90) | 1.00 / 1.12 / 2.00 (1.90–14.21) | 99% | 11 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–6.85) | 1.00 / 1.00 / 1.07 (1.04–4.99) | 1.00 / 2.94 / 4.48 (4.42–4.48) | 1.00 / 1.07 / 2.03 (2.03–7.94) | 1.00 / 3.07 / 4.29 (4.29–8.32) | 1.00 / 1.49 / 2.08 (2.03–5.38) | 1.00 / 3.07 / 4.29 (4.22–4.67) | 1.00 / 1.49 / 2.08 (2.03–5.50) | 1.00 / 1.49 / 2.08 (2.03–5.63) | 99% | 1 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.04–1.12) | 1.00 / 3.10 / 4.67 (4.61–9.34) | 1.00 / 1.15 / 2.03 (2.00–6.08) | 1.00 / 3.52 / 5.31 (4.93–12.29) | 1.00 / 1.74 / 2.08 (2.08–7.87) | 1.00 / 3.52 / 5.06 (4.80–6.27) | 1.00 / 1.74 / 2.08 (2.08–2.11) | 1.00 / 1.70 / 2.08 (2.08–2.08) | 99% | 18 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.04 / 3.94 / 6.27 (6.21–6.46) | 1.00 / 1.20 / 2.08 (2.08–2.14) | 1.04 / 4.48 / 6.40 (6.27–6.66) | 1.00 / 1.82 / 2.24 (2.21–2.30) | 1.04 / 4.80 / 6.53 (6.40–7.17) | 1.00 / 1.95 / 2.24 (2.21–2.37) | 1.00 / 2.00 / 2.30 (2.27–3.10) | 66% | 38 |
| 10 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.04 / 4.48 / 8.32 (8.06–8.32) | 1.04 / 1.25 / 2.08 (2.03–2.27) | 1.04 / 5.25 / 7.74 (7.62–10.37) | 1.04 / 1.78 / 2.21 (2.21–6.40) | 1.07 / 5.25 / 7.62 (7.49–8.06) | 1.04 / 2.03 / 2.24 (2.24–5.82) | 1.04 / 2.08 / 2.30 (2.27–7.36) | 49% | 151 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.29 / 0.37 (0.37–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.25 / 0.37 (0.37–0.37) | 0.08 / 0.12 / 0.25 (0.21–0.25) | 0.08 / 0.29 / 0.42 (0.42–0.46) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.08 / 0.29 / 0.46 (0.42–0.50) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 99% | 0 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.37 / 0.62 (0.54–0.62) | 0.08 / 0.21 / 0.33 (0.25–0.33) | 0.12 / 0.46 / 0.62 (0.58–0.66) | 0.08 / 0.21 / 0.37 (0.37–0.42) | 0.08 / 0.46 / 0.74 (0.74–0.79) | 0.08 / 0.25 / 0.46 (0.42–0.46) | 0.08 / 0.25 / 0.46 (0.42–0.46) | 66% | 47 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.42 / 0.66 (0.58–0.79) | 0.12 / 0.21 / 0.37 (0.29–0.37) | 0.12 / 0.46 / 0.70 (0.70–0.79) | 0.12 / 0.21 / 0.37 (0.33–0.37) | 0.12 / 0.50 / 0.79 (0.79–0.83) | 0.12 / 0.25 / 0.46 (0.37–0.46) | 0.12 / 0.29 / 0.46 (0.46–0.50) | 49% | 0 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.50 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.50 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.50 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.42–0.46) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.54 / 0.70 (0.70–0.74) | 0.21 / 0.25 / 0.46 (0.46–0.50) | 0.21 / 0.62 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.62 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.25 / 0.50 (0.50–0.62) | 99% | 3 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.54 / 0.79 (0.74–0.83) | 0.21 / 0.25 / 0.46 (0.42–0.46) | 0.21 / 0.66 / 1.04 (1.00–1.04) | 0.21 / 0.33 / 0.50 (0.50–0.58) | 0.21 / 0.66 / 1.04 (0.95–1.07) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.25 / 0.66 / 1.04 (0.83–1.07) | 0.21 / 0.37 / 0.54 (0.50–0.58) | 0.25 / 0.87 / 1.41 (1.33–1.49) | 0.21 / 0.37 / 0.66 (0.62–0.70) | 0.25 / 1.15 / 1.74 (1.70–1.79) | 0.21 / 0.46 / 0.66 (0.62–0.66) | 0.21 / 0.46 / 0.70 (0.66–0.70) | 65% | 21 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 1.12 (1.07–2.21) | 0.25 / 0.37 / 0.54 (0.50–0.62) | 0.25 / 0.91 / 1.49 (1.46–1.70) | 0.25 / 0.37 / 0.62 (0.62–0.62) | 0.25 / 1.46 / 2.27 (2.21–2.72) | 0.21 / 0.50 / 0.66 (0.66–0.66) | 0.21 / 0.54 / 0.74 (0.70–0.79) | 49% | 50 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.12 (1.04–1.12) | 1.00 / 2.94 / 3.10 (3.10–21.76) | 1.00 / 1.12 / 2.00 (1.95–13.31) | 1.00 / 2.94 / 3.10 (3.10–7.10) | 1.00 / 1.12 / 2.00 (1.95–13.70) | 1.00 / 2.98 / 3.68 (3.10–8.19) | 1.00 / 1.12 / 2.08 (2.00–12.67) | 1.00 / 1.12 / 2.03 (2.00–7.17) | 98% | 7 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.04 / 1.07 (1.07–4.74) | 1.00 / 1.04 / 1.12 (1.04–3.74) | 1.00 / 2.72 / 3.68 (3.20–4.16) | 1.00 / 1.07 / 2.00 (2.00–2.08) | 1.00 / 3.07 / 4.35 (4.22–4.67) | 1.00 / 1.49 / 2.08 (2.03–2.11) | 1.00 / 3.07 / 4.74 (4.22–10.50) | 1.00 / 1.41 / 2.08 (2.03–2.08) | 1.00 / 1.46 / 2.08 (2.03–2.11) | 99% | 13 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.12 (1.07–6.59) | 1.00 / 1.04 / 1.12 (1.07–6.78) | 1.00 / 2.78 / 4.03 (3.65–9.09) | 1.00 / 1.15 / 2.03 (2.00–5.95) | 1.00 / 3.52 / 5.50 (4.86–11.01) | 1.00 / 1.70 / 2.08 (2.08–8.13) | 1.00 / 3.52 / 5.18 (4.86–12.16) | 1.00 / 1.74 / 2.08 (2.08–5.82) | 1.00 / 1.74 / 2.11 (2.08–6.72) | 97% | 18 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.12 (1.07–2.11) | 1.00 / 1.07 / 1.07 (1.07–2.14) | 1.04 / 3.10 / 4.74 (4.61–5.82) | 1.00 / 1.15 / 2.08 (2.03–2.30) | 1.04 / 4.06 / 6.14 (5.95–6.53) | 1.00 / 1.82 / 2.24 (2.21–3.46) | 1.04 / 4.86 / 7.04 (6.53–7.62) | 1.00 / 2.00 / 2.24 (2.21–4.99) | 1.00 / 2.00 / 2.27 (2.24–4.10) | 66% | 50 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.04 / 3.33 / 5.44 (5.25–6.14) | 1.04 / 1.20 / 2.08 (2.03–6.53) | 1.04 / 4.74 / 7.42 (7.17–7.74) | 1.04 / 1.74 / 2.21 (2.21–2.30) | 1.07 / 5.25 / 7.74 (7.62–8.58) | 1.04 / 2.03 / 2.24 (2.24–6.46) | 1.04 / 2.08 / 2.37 (2.30–2.40) | 49% | 75 |

Delete (removal) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.62 / 0.83 / 0.95 (0.87–15.36) | 0.54 / 0.78 / 3.52 (0.83–8.83) | 0.21 / 0.33 / 0.62 (0.33–5.57) | 0.25 / 0.46 / 3.39 (0.46–5.89) | 0.21 / 0.29 / 3.26 (0.33–3.65) | 0.25 / 0.37 / 0.46 (0.37–5.44) | 0.21 / 0.29 / 0.46 (0.33–5.70) | 0.21 / 0.29 / 0.50 (0.33–3.39) | 0.25 / 0.37 / 0.37 (0.33–3.90) | 99% | 2 |
| 1 | 0 | 50 | 3 | 0.79 / 1.15 / 10.11 (9.73–32.77) | 0.74 / 3.74 / 12.16 (4.93–38.40) | 0.70 / 1.00 / 6.72 (1.04–9.73) | 0.70 / 2.27 / 9.73 (1.12–29.95) | 0.25 / 0.33 / 3.04 (0.42–3.90) | 0.25 / 0.37 / 0.58 (0.54–4.29) | 0.25 / 0.46 / 3.36 (0.42–4.86) | 0.25 / 0.33 / 3.23 (0.54–3.33) | 0.25 / 0.37 / 3.36 (0.37–5.95) | 99% | 3 |
| 1 | 0 | 50 | 4 | 0.87 / 4.06 / 21.76 (1.33–26.11) | 0.83 / 3.87 / 8.19 (4.86–17.15) | 0.83 / 1.28 / 5.18 (2.03–6.46) | 0.79 / 4.00 / 5.95 (2.50–8.13) | 0.25 / 0.66 / 3.36 (3.23–6.21) | 0.29 / 3.26 / 5.70 (0.54–6.21) | 0.29 / 0.58 / 3.39 (0.46–5.44) | 0.25 / 3.23 / 4.06 (0.50–5.06) | 0.29 / 3.26 / 4.48 (0.50–5.95) | 99% | 0 |
| 1 | 0 | 50 | 6 | 1.46 / 3.52 / 32.77 (13.95–851.97) | 1.46 / 7.62 / 20.48 (12.42–1654.78) | 1.66 / 3.04 / 9.09 (3.68–12.42) | 1.62 / 3.07 / 5.44 (3.74–22.78) | 1.33 / 2.30 / 4.03 (2.43–53.25) | 1.41 / 2.43 / 3.78 (3.04–4.74) | 0.37 / 0.70 / 1.07 (0.83–1.46) | 0.37 / 0.70 / 0.95 (0.79–1.12) | 0.42 / 0.70 / 0.91 (0.74–1.20) | 65% | 32 |
| 1 | 0 | 50 | 8 | 1.78 / 7.74 / 196.61 (16.00–819.20) | 1.70 / 7.74 / 1081.34 (21.50–8323.07) | 1.90 / 3.52 / 30.98 (3.65–3178.50) | 1.90 / 3.07 / 7.74 (4.93–12.03) | 1.54 / 2.82 / 3.90 (3.23–8.70) | 1.57 / 2.69 / 4.00 (3.07–11.52) | 0.42 / 0.74 / 1.07 (0.95–3.49) | 0.42 / 0.70 / 1.07 (0.87–2.37) | 0.42 / 0.62 / 0.83 (0.83–1.20) | 49% | 170 |
| 1 | 0 | 200 | 2 | 0.91 / 1.25 / 7.49 (1.82–17.66) | 0.74 / 1.07 / 1.15 (1.15–41.98) | 0.37 / 0.46 / 3.26 (0.54–5.25) | 0.37 / 0.50 / 3.46 (0.50–6.66) | 0.37 / 0.50 / 3.36 (0.46–5.44) | 0.37 / 0.62 / 3.36 (0.83–5.89) | 0.33 / 0.46 / 3.39 (0.50–8.58) | 0.37 / 0.46 / 0.78 (0.46–3.65) | 0.37 / 0.50 / 0.79 (0.62–3.33) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.82 / 16.00 (4.80–33.28) | 0.95 / 1.54 / 13.57 (4.00–38.91) | 0.87 / 3.55 / 5.57 (1.57–14.59) | 0.87 / 1.28 / 4.06 (1.41–3047.42) | 0.37 / 0.54 / 3.46 (0.74–6.46) | 0.42 / 0.50 / 0.91 (0.58–5.06) | 0.37 / 0.46 / 0.66 (0.50–6.21) | 0.42 / 0.46 / 2.66 (0.70–3.49) | 0.42 / 0.50 / 3.36 (0.54–4.03) | 99% | 11 |
| 1 | 0 | 200 | 4 | 1.15 / 3.52 / 9.34 (8.32–25.86) | 1.04 / 4.10 / 19.71 (2.03–294.91) | 1.00 / 3.90 / 5.50 (2.00–19.20) | 1.00 / 3.87 / 5.63 (1.49–14.72) | 0.42 / 3.14 / 5.31 (3.49–6.53) | 0.42 / 3.14 / 5.31 (0.78–5.82) | 0.42 / 2.78 / 3.55 (0.70–5.95) | 0.42 / 0.58 / 3.52 (3.46–6.27) | 0.42 / 0.62 / 3.87 (3.68–4.42) | 99% | 2 |
| 1 | 0 | 200 | 6 | 2.00 / 3.07 / 14.72 (12.16–892.93) | 1.74 / 3.36 / 282.62 (3.39–1146.88) | 2.11 / 3.55 / 4.42 (4.29–6.98) | 1.87 / 3.36 / 6.53 (4.99–4521.98) | 1.57 / 2.62 / 3.65 (2.94–20.74) | 1.62 / 2.69 / 3.68 (3.65–6.72) | 0.54 / 0.87 / 1.20 (1.04–2.56) | 0.50 / 0.83 / 1.15 (1.00–1.28) | 0.62 / 0.91 / 1.12 (0.95–1.36) | 65% | 40 |
| 1 | 0 | 200 | 8 | 2.00 / 3.49 / 13.06 (9.86–28.67) | 2.00 / 6.91 / 47.62 (11.78–84.99) | 2.11 / 3.68 / 11.65 (4.61–1835.01) | 2.21 / 4.16 / 10.62 (5.12–72.70) | 2.00 / 3.07 / 4.06 (3.23–8.70) | 1.87 / 3.10 / 4.16 (3.74–8.19) | 0.54 / 0.91 / 1.28 (1.15–1.28) | 0.54 / 0.83 / 1.25 (1.07–1.36) | 0.54 / 0.87 / 1.15 (1.07–2.66) | 49% | 10 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.62 / 10.11 (3.62–16.13) | 1.79 / 2.82 / 6.53 (5.44–16.38) | 1.12 / 1.25 / 2.37 (1.90–6.08) | 1.15 / 1.28 / 2.30 (1.62–4.42) | 1.12 / 1.25 / 2.27 (1.95–35.33) | 1.15 / 1.28 / 1.58 (1.28–8.32) | 1.12 / 1.25 / 6.72 (1.58–9.86) | 1.12 / 1.25 / 4.22 (2.14–5.82) | 1.15 / 1.28 / 2.11 (1.49–11.52) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.94 / 4.74 / 21.25 (13.57–58.37) | 2.27 / 3.87 / 20.74 (7.10–30.98) | 1.87 / 3.04 / 6.72 (3.26–25.86) | 1.87 / 3.20 / 7.49 (3.78–27.14) | 1.20 / 1.46 / 2.30 (2.27–7.04) | 1.20 / 1.28 / 4.35 (2.08–4.80) | 1.15 / 1.28 / 2.37 (2.14–5.63) | 1.20 / 1.62 / 5.38 (2.27–16.64) | 1.20 / 1.28 / 2.27 (2.24–5.44) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.74 / 8.70 (6.85–26.11) | 2.56 / 5.18 / 10.24 (6.59–28.67) | 1.95 / 4.29 / 5.31 (4.29–6.72) | 2.00 / 4.86 / 7.87 (3.26–8.00) | 1.20 / 1.87 / 4.16 (2.37–6.53) | 1.20 / 2.91 / 4.48 (2.21–8.19) | 1.20 / 2.21 / 4.16 (3.87–4.80) | 1.20 / 1.90 / 4.29 (2.30–9.47) | 1.20 / 4.16 / 4.86 (2.24–8.83) | 99% | 0 |
| 1 | 0 | 1000 | 6 | 4.06 / 9.47 / 43.01 (13.44–1376.26) | 3.39 / 5.89 / 20.22 (15.87–503.81) | 2.98 / 4.74 / 9.98 (5.12–11.39) | 2.98 / 4.74 / 10.50 (4.80–19.97) | 2.43 / 3.87 / 5.06 (4.93–9.60) | 2.53 / 4.00 / 9.22 (5.18–10.62) | 1.33 / 2.43 / 6.72 (2.82–8.83) | 1.33 / 2.21 / 3.04 (2.62–9.60) | 1.36 / 1.74 / 3.39 (2.53–8.00) | 65% | 28 |
| 1 | 0 | 1000 | 8 | 4.35 / 7.36 / 21.25 (15.36–671.74) | 3.65 / 7.17 / 24.32 (12.16–712.70) | 3.33 / 5.44 / 11.26 (5.95–39.94) | 3.26 / 6.08 / 11.65 (6.46–1024.00) | 2.62 / 4.10 / 6.02 (4.93–11.65) | 2.66 / 4.16 / 11.90 (5.57–13.06) | 1.36 / 2.56 / 3.65 (3.14–6.53) | 1.36 / 2.37 / 2.78 (2.72–3.52) | 1.46 / 2.24 / 2.91 (2.78–3.04) | 49% | 68 |
| 1 | 1000 | 50 | 2 | 1.58 / 1.82 / 5.95 (2.40–7.55) | 1.54 / 1.74 / 2.91 (1.74–6.72) | 0.21 / 0.33 / 0.42 (0.33–0.91) | 0.25 / 0.37 / 0.37 (0.37–3.14) | 0.21 / 0.33 / 0.33 (0.33–3.74) | 0.25 / 0.37 / 0.37 (0.37–0.66) | 0.25 / 0.33 / 0.33 (0.33–3.04) | 0.21 / 0.33 / 0.33 (0.33–5.95) | 0.25 / 0.37 / 0.42 (0.37–3.36) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.72 / 3.07 / 8.70 (5.31–21.76) | 2.69 / 3.04 / 7.74 (6.85–34.30) | 1.66 / 1.90 / 4.48 (2.00–8.19) | 1.66 / 1.95 / 7.62 (3.20–491.52) | 0.25 / 0.33 / 3.23 (0.42–5.76) | 0.29 / 0.37 / 3.33 (0.42–6.27) | 0.25 / 0.33 / 3.23 (0.42–6.14) | 0.25 / 0.33 / 0.50 (0.46–3.74) | 0.29 / 0.37 / 3.33 (0.50–3.49) | 99% | 3 |
| 1 | 1000 | 50 | 4 | 2.82 / 3.26 / 18.43 (8.19–43.01) | 2.78 / 4.10 / 18.18 (7.10–28.67) | 1.78 / 3.26 / 7.04 (4.86–12.67) | 1.74 / 4.61 / 4.99 (2.43–8.32) | 0.29 / 0.33 / 3.20 (0.46–3.68) | 0.29 / 0.46 / 3.36 (1.82–3.49) | 0.29 / 0.33 / 3.39 (0.58–5.89) | 0.29 / 0.58 / 3.46 (0.74–4.99) | 0.29 / 0.37 / 3.36 (0.37–3.49) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.65 / 4.48 / 11.01 (6.85–647.17) | 3.62 / 4.67 / 28.16 (5.63–73.73) | 3.49 / 4.54 / 5.31 (4.86–8.58) | 3.68 / 4.67 / 5.31 (4.86–17.66) | 2.40 / 3.26 / 4.35 (3.74–10.62) | 2.40 / 3.36 / 4.48 (3.52–8.06) | 0.42 / 0.79 / 0.95 (0.83–1.07) | 0.42 / 0.74 / 1.00 (0.83–1.07) | 0.42 / 0.70 / 0.87 (0.79–1.04) | 66% | 20 |
| 1 | 1000 | 50 | 8 | 3.81 / 5.44 / 14.98 (11.52–109.57) | 3.74 / 5.18 / 21.76 (13.06–430.08) | 3.78 / 5.12 / 10.37 (5.50–13.06) | 3.90 / 5.25 / 6.91 (6.40–99.33) | 2.43 / 3.65 / 4.61 (4.16–5111.81) | 2.56 / 4.00 / 5.25 (4.99–24.58) | 0.50 / 0.79 / 1.07 (0.83–1.20) | 0.42 / 0.74 / 0.95 (0.91–6.21) | 0.50 / 0.78 / 1.04 (0.74–1.28) | 49% | 29 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.24 / 3.94 (2.27–17.41) | 1.74 / 2.08 / 4.10 (2.11–24.83) | 0.33 / 0.46 / 0.50 (0.46–3.87) | 0.37 / 0.50 / 0.58 (0.50–3.81) | 0.37 / 0.46 / 0.66 (0.54–3.36) | 0.37 / 0.50 / 0.70 (0.50–4.93) | 0.37 / 0.46 / 0.70 (0.46–3.23) | 0.33 / 0.46 / 0.74 (0.46–5.44) | 0.37 / 0.50 / 0.74 (0.50–5.44) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 3.52 / 8.32 (3.52–15.36) | 2.91 / 3.46 / 8.96 (3.49–29.95) | 1.87 / 2.24 / 6.27 (4.86–13.44) | 1.87 / 2.21 / 5.44 (2.30–25.60) | 0.42 / 0.46 / 0.74 (0.66–5.38) | 0.42 / 0.50 / 0.58 (0.58–3.46) | 0.42 / 0.46 / 0.70 (0.66–3.65) | 0.42 / 0.46 / 0.70 (0.62–3.36) | 0.42 / 0.50 / 0.79 (0.70–6.46) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 3.10 / 4.00 / 15.23 (5.50–34.30) | 3.04 / 5.70 / 28.42 (9.98–294.91) | 1.95 / 2.40 / 5.06 (2.72–8.06) | 1.95 / 2.27 / 2.78 (2.56–7.55) | 0.42 / 0.70 / 3.52 (3.39–3.78) | 0.42 / 0.54 / 3.55 (3.46–5.63) | 0.42 / 0.58 / 3.55 (3.33–4.86) | 0.42 / 0.62 / 3.49 (3.46–4.74) | 0.42 / 0.54 / 3.39 (0.78–4.00) | 99% | 0 |
| 1 | 1000 | 200 | 6 | 3.81 / 5.12 / 13.18 (8.70–270.34) | 3.78 / 5.18 / 16.38 (12.54–647.17) | 3.78 / 4.99 / 6.21 (5.44–196.61) | 3.81 / 4.86 / 6.21 (5.95–12.93) | 2.53 / 3.39 / 4.67 (4.06–9.47) | 2.53 / 3.49 / 5.25 (4.54–15.23) | 0.54 / 0.87 / 1.25 (1.20–8.58) | 0.50 / 0.83 / 1.28 (1.07–3.07) | 0.58 / 0.91 / 1.33 (1.07–3.87) | 64% | 36 |
| 1 | 1000 | 200 | 8 | 4.03 / 9.73 / 335.87 (13.31–565.25) | 3.94 / 5.95 / 16.64 (12.80–524.29) | 4.03 / 5.44 / 8.13 (7.10–186.37) | 4.00 / 5.25 / 7.87 (6.14–51.20) | 2.72 / 3.87 / 5.25 (4.93–30.46) | 2.69 / 4.22 / 5.12 (4.61–149.50) | 0.54 / 0.91 / 1.25 (1.20–3.90) | 0.54 / 0.83 / 1.25 (1.15–11.39) | 0.54 / 0.91 / 1.20 (1.12–1.33) | 48% | 41 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.54 / 12.93 (4.61–16.38) | 2.78 / 3.94 / 13.44 (9.09–18.94) | 1.12 / 1.25 / 2.27 (1.79–417.79) | 1.15 / 1.28 / 5.57 (1.87–9.98) | 1.12 / 1.20 / 1.95 (1.25–4.61) | 1.15 / 1.28 / 2.03 (1.28–5.57) | 1.12 / 1.25 / 2.27 (1.90–6.21) | 1.12 / 1.25 / 2.24 (1.87–6.98) | 1.15 / 1.28 / 1.66 (1.28–4.29) | 100% | 3 |
| 1 | 1000 | 1000 | 3 | 4.67 / 6.53 / 11.39 (6.98–22.78) | 3.94 / 5.70 / 14.98 (13.82–37.89) | 2.85 / 4.00 / 13.82 (7.10–31.23) | 2.85 / 5.31 / 12.54 (4.74–20.74) | 1.20 / 1.33 / 4.99 (1.82–7.04) | 1.20 / 1.28 / 4.61 (2.24–13.57) | 1.20 / 1.25 / 5.06 (2.24–18.43) | 1.20 / 1.74 / 6.91 (2.30–8.32) | 1.20 / 1.28 / 2.40 (2.30–11.65) | 99% | 0 |
| 1 | 1000 | 1000 | 4 | 5.12 / 8.06 / 14.34 (8.13–40.96) | 4.42 / 7.74 / 16.90 (8.70–31.74) | 2.94 / 3.94 / 5.89 (4.06–13.95) | 2.94 / 4.80 / 7.62 (4.16–12.67) | 1.20 / 2.37 / 4.22 (3.39–8.19) | 1.20 / 4.03 / 4.74 (3.87–6.98) | 1.20 / 3.26 / 4.29 (4.22–6.40) | 1.20 / 3.90 / 4.16 (2.14–8.83) | 1.20 / 1.70 / 4.74 (2.27–15.23) | 98% | 7 |
| 1 | 1000 | 1000 | 6 | 6.14 / 12.67 / 471.04 (30.21–17825.79) | 5.31 / 13.70 / 499.71 (176.13–1556.48) | 4.67 / 6.40 / 8.58 (7.23–16.13) | 4.67 / 6.34 / 8.83 (6.98–10.62) | 3.55 / 4.80 / 8.32 (5.89–12.16) | 3.46 / 4.48 / 5.82 (5.25–413.70) | 1.33 / 2.27 / 2.85 (2.69–5.82) | 1.33 / 2.27 / 2.82 (2.56–8.83) | 1.36 / 1.70 / 2.69 (2.53–5.95) | 65% | 73 |
| 1 | 1000 | 1000 | 8 | 6.40 / 8.96 / 27.39 (15.49–1097.73) | 5.76 / 8.96 / 20.74 (19.46–647.17) | 5.12 / 7.10 / 11.65 (8.32–109.57) | 5.18 / 7.10 / 13.31 (7.49–1196.03) | 3.52 / 4.86 / 9.34 (5.82–47.10) | 3.62 / 4.74 / 7.30 (5.82–22.78) | 1.36 / 2.53 / 2.98 (2.85–11.65) | 1.36 / 2.43 / 3.14 (2.94–4.03) | 1.36 / 2.00 / 3.36 (2.53–16.38) | 48% | 43 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 3.10 (1.00–13.06) | 0.54 / 0.79 / 4.00 (0.91–13.44) | 0.25 / 0.33 / 0.50 (0.46–0.54) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.25 / 0.33 / 0.50 (0.50–1.33) | 0.25 / 0.37 / 0.50 (0.46–0.87) | 0.25 / 0.33 / 0.50 (0.50–0.54) | 0.21 / 0.33 / 0.50 (0.50–0.58) | 0.25 / 0.37 / 0.50 (0.46–0.54) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.79 / 1.15 / 7.10 (5.12–19.71) | 0.70 / 1.12 / 7.36 (5.82–16.13) | 0.70 / 1.12 / 9.34 (1.25–14.34) | 0.70 / 1.07 / 3.55 (1.70–11.26) | 0.25 / 0.46 / 0.62 (0.54–1.15) | 0.25 / 0.42 / 0.54 (0.54–0.66) | 0.25 / 0.42 / 0.54 (0.54–0.70) | 0.25 / 0.46 / 0.54 (0.54–0.66) | 0.25 / 0.42 / 0.62 (0.54–0.66) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.87 / 1.49 / 10.75 (5.82–21.25) | 0.83 / 1.33 / 11.26 (4.54–23.81) | 0.83 / 1.54 / 4.54 (2.21–10.11) | 0.83 / 1.36 / 2.94 (1.66–9.34) | 0.29 / 0.54 / 0.83 (0.58–3.23) | 0.29 / 0.50 / 0.74 (0.58–4.54) | 0.29 / 0.50 / 0.83 (0.54–2.24) | 0.29 / 0.54 / 0.83 (0.58–3.90) | 0.29 / 0.50 / 0.66 (0.58–3.20) | 99% | 0 |
| 10 | 0 | 50 | 6 | 1.46 / 3.23 / 12.54 (5.57–16.38) | 1.41 / 3.04 / 11.52 (8.19–46.59) | 1.66 / 3.65 / 5.63 (4.74–11.52) | 1.70 / 3.90 / 6.78 (4.99–65.02) | 1.41 / 2.69 / 4.80 (3.46–8.83) | 1.36 / 2.66 / 3.87 (3.33–51.71) | 0.42 / 0.83 / 1.20 (1.07–1.33) | 0.42 / 1.00 / 1.28 (1.25–1.36) | 0.42 / 0.83 / 1.15 (0.95–1.28) | 66% | 112 |
| 10 | 0 | 50 | 8 | 1.78 / 4.61 / 13.31 (11.14–19.46) | 1.70 / 4.42 / 13.31 (11.65–66.56) | 2.11 / 4.86 / 7.36 (6.46–17.41) | 2.11 / 4.67 / 8.70 (6.59–9.47) | 1.57 / 3.46 / 5.76 (4.48–7.17) | 1.62 / 3.49 / 5.18 (4.48–5.70) | 0.42 / 0.91 / 1.33 (1.25–1.41) | 0.46 / 1.04 / 1.46 (1.25–1.54) | 0.46 / 0.95 / 1.28 (1.20–1.54) | 49% | 28 |
| 10 | 0 | 200 | 2 | 0.87 / 1.25 / 5.70 (3.87–6.91) | 0.74 / 1.07 / 4.99 (1.25–7.30) | 0.33 / 0.50 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.79 (0.78–0.83) | 0.33 / 0.54 / 0.74 (0.74–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.83) | 0.33 / 0.54 / 0.74 (0.70–0.91) | 0.33 / 0.58 / 0.74 (0.70–0.95) | 0.37 / 0.50 / 0.83 (0.78–0.87) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.04 / 1.66 / 14.21 (4.16–18.94) | 0.91 / 1.58 / 13.18 (4.29–22.78) | 0.87 / 1.36 / 6.21 (4.54–12.16) | 0.87 / 1.36 / 7.04 (1.87–14.08) | 0.42 / 0.66 / 0.87 (0.79–5.06) | 0.42 / 0.62 / 0.91 (0.83–5.12) | 0.42 / 0.66 / 0.87 (0.79–5.44) | 0.42 / 0.70 / 0.91 (0.83–3.36) | 0.42 / 0.66 / 0.91 (0.83–5.31) | 99% | 4 |
| 10 | 0 | 200 | 4 | 1.15 / 1.95 / 16.38 (5.95–22.27) | 1.04 / 1.74 / 19.46 (2.40–327.68) | 1.00 / 1.62 / 5.44 (2.00–34.30) | 1.04 / 1.82 / 3.07 (2.24–58.37) | 0.42 / 0.74 / 1.00 (0.87–8.58) | 0.42 / 0.74 / 0.95 (0.87–7.62) | 0.42 / 0.74 / 0.87 (0.83–4.74) | 0.42 / 0.74 / 1.15 (0.95–4.67) | 0.42 / 0.74 / 0.95 (0.87–2.37) | 95% | 21 |
| 10 | 0 | 200 | 6 | 1.82 / 3.65 / 10.24 (9.60–31.49) | 1.66 / 3.62 / 9.47 (8.58–47.62) | 1.95 / 4.03 / 5.95 (4.80–9.22) | 2.08 / 4.61 / 6.27 (5.82–32.77) | 1.57 / 3.07 / 4.99 (4.10–10.88) | 1.58 / 3.07 / 4.74 (4.16–6.08) | 0.54 / 1.15 / 1.54 (1.49–1.57) | 0.54 / 1.15 / 1.54 (1.36–1.62) | 0.58 / 1.25 / 1.57 (1.49–5.31) | 66% | 31 |
| 10 | 0 | 200 | 8 | 2.11 / 5.38 / 17.66 (12.67–2326.53) | 1.95 / 9.98 / 55.30 (11.14–3309.57) | 2.30 / 5.44 / 10.62 (7.55–274.43) | 2.30 / 6.27 / 25.34 (7.36–1753.09) | 1.78 / 3.78 / 7.94 (4.48–13.18) | 1.78 / 3.87 / 6.72 (5.06–12.29) | 0.58 / 1.36 / 1.87 (1.82–5.18) | 0.58 / 1.25 / 1.70 (1.62–1.74) | 0.62 / 1.33 / 1.79 (1.66–1.87) | 49% | 377 |
| 10 | 0 | 1000 | 2 | 2.43 / 3.58 / 10.75 (3.65–11.14) | 1.82 / 2.82 / 6.08 (4.42–7.62) | 1.15 / 1.49 / 2.11 (1.87–2.21) | 1.15 / 1.57 / 2.27 (2.14–2.37) | 1.15 / 1.46 / 1.87 (1.74–2.03) | 1.15 / 1.62 / 2.27 (2.24–23.55) | 1.15 / 1.41 / 1.87 (1.74–16.00) | 1.15 / 1.90 / 2.30 (2.27–72.70) | 1.15 / 1.70 / 2.30 (2.27–15.74) | 99% | 11 |
| 10 | 0 | 1000 | 3 | 2.72 / 4.61 / 7.81 (6.08–25.60) | 2.30 / 3.78 / 9.60 (6.78–25.34) | 1.87 / 3.52 / 5.70 (4.16–8.19) | 1.87 / 3.65 / 4.16 (4.16–43.01) | 1.20 / 2.30 / 2.50 (2.43–9.47) | 1.20 / 2.08 / 2.40 (2.40–6.59) | 1.20 / 2.30 / 2.50 (2.50–3.20) | 1.20 / 2.27 / 2.85 (2.43–8.45) | 1.20 / 2.11 / 2.43 (2.40–8.83) | 99% | 1 |
| 10 | 0 | 1000 | 4 | 3.23 / 5.06 / 8.45 (7.81–24.58) | 2.56 / 4.61 / 17.41 (10.24–17.66) | 2.00 / 3.74 / 4.67 (4.22–155.65) | 2.03 / 4.03 / 8.96 (4.99–17.15) | 1.20 / 2.43 / 3.62 (3.49–10.88) | 1.20 / 2.30 / 2.69 (2.50–9.22) | 1.20 / 2.40 / 3.65 (3.49–4.99) | 1.20 / 2.40 / 3.46 (3.46–4.35) | 1.20 / 2.27 / 3.04 (2.53–3.07) | 99% | 18 |
| 10 | 0 | 1000 | 6 | 4.22 / 7.49 / 14.21 (12.67–41.47) | 3.52 / 7.23 / 14.72 (11.26–503.81) | 3.20 / 6.21 / 8.58 (7.62–9.47) | 3.36 / 7.42 / 10.37 (9.86–17.66) | 2.56 / 5.38 / 7.74 (6.59–9.60) | 2.85 / 6.59 / 9.22 (8.83–11.39) | 1.36 / 3.78 / 4.93 (4.29–5.06) | 1.36 / 3.39 / 4.48 (4.48–7.42) | 1.49 / 2.94 / 4.06 (3.94–4.29) | 66% | 38 |
| 10 | 0 | 1000 | 8 | 4.86 / 10.24 / 20.48 (16.64–57.86) | 4.29 / 9.98 / 18.69 (12.67–3276.80) | 3.78 / 8.70 / 15.74 (12.93–274.43) | 3.81 / 9.60 / 15.62 (12.16–151.55) | 2.82 / 6.21 / 9.73 (8.19–23.04) | 3.04 / 8.06 / 11.39 (11.01–80.90) | 1.36 / 4.00 / 5.38 (5.31–6.40) | 1.41 / 4.00 / 5.70 (5.31–12.16) | 1.41 / 3.14 / 4.80 (4.10–16.26) | 49% | 151 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 7.55 (5.18–11.52) | 1.54 / 1.79 / 6.78 (1.87–10.37) | 0.25 / 0.33 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 0.25 / 0.33 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.25 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.07 / 7.30 (3.33–11.14) | 2.66 / 3.07 / 10.11 (7.94–27.14) | 1.66 / 2.08 / 4.74 (2.27–6.46) | 1.66 / 2.08 / 2.43 (2.37–11.90) | 0.25 / 0.42 / 0.54 (0.54–0.58) | 0.29 / 0.37 / 0.54 (0.54–0.62) | 0.25 / 0.46 / 0.54 (0.54–0.62) | 0.25 / 0.46 / 0.54 (0.54–0.58) | 0.29 / 0.37 / 0.58 (0.54–0.62) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.82 / 3.39 / 12.93 (8.96–17.15) | 2.78 / 3.23 / 10.62 (7.49–19.97) | 1.78 / 2.56 / 4.54 (2.72–5.44) | 1.78 / 2.53 / 2.85 (2.78–7.87) | 0.29 / 0.50 / 0.58 (0.58–0.74) | 0.29 / 0.46 / 0.58 (0.58–1.82) | 0.29 / 0.54 / 0.78 (0.58–1.12) | 0.29 / 0.50 / 0.58 (0.58–1.41) | 0.29 / 0.50 / 0.70 (0.58–0.79) | 99% | 0 |
| 10 | 1000 | 50 | 6 | 3.46 / 5.18 / 11.78 (10.62–14.21) | 3.49 / 5.57 / 12.67 (8.58–61.44) | 2.94 / 5.25 / 7.62 (5.57–8.83) | 2.94 / 4.86 / 6.14 (5.89–8.19) | 2.30 / 4.03 / 5.31 (4.99–5.76) | 2.24 / 3.78 / 4.93 (4.61–540.67) | 0.42 / 0.87 / 1.12 (1.07–1.36) | 0.46 / 1.04 / 1.36 (1.33–1.46) | 0.50 / 1.00 / 1.33 (1.12–1.33) | 66% | 47 |
| 10 | 1000 | 50 | 8 | 3.87 / 6.66 / 13.70 (11.65–15.74) | 3.68 / 6.78 / 14.08 (11.01–19.71) | 3.78 / 6.40 / 9.22 (8.96–10.62) | 3.78 / 6.53 / 8.83 (8.32–12.67) | 2.37 / 4.48 / 7.04 (5.25–8.70) | 2.30 / 4.42 / 5.38 (5.25–6.14) | 0.42 / 0.95 / 1.25 (1.25–1.36) | 0.46 / 1.07 / 1.41 (1.28–1.66) | 0.50 / 1.00 / 1.36 (1.25–1.41) | 49% | 0 |
| 10 | 1000 | 200 | 2 | 1.87 / 2.21 / 2.30 (2.27–8.13) | 1.74 / 2.11 / 7.30 (2.40–10.24) | 0.33 / 0.54 / 0.74 (0.70–0.78) | 0.37 / 0.50 / 0.79 (0.74–0.83) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.79 (0.74–0.79) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.33 / 0.58 / 0.74 (0.74–0.78) | 0.37 / 0.50 / 0.79 (0.78–0.83) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.52 / 11.14 (6.02–12.16) | 2.85 / 3.49 / 8.58 (3.78–12.03) | 1.87 / 2.30 / 7.81 (2.66–17.15) | 1.87 / 2.37 / 10.11 (2.69–16.13) | 0.37 / 0.62 / 0.79 (0.79–0.83) | 0.42 / 0.66 / 0.87 (0.83–0.87) | 0.37 / 0.66 / 0.79 (0.78–0.83) | 0.42 / 0.70 / 0.79 (0.79–0.95) | 0.42 / 0.66 / 0.83 (0.83–6.46) | 99% | 3 |
| 10 | 1000 | 200 | 4 | 3.10 / 3.81 / 8.00 (7.23–17.15) | 2.98 / 3.68 / 11.52 (7.23–17.92) | 1.95 / 2.82 / 4.86 (3.07–10.50) | 1.95 / 2.72 / 3.14 (2.98–8.58) | 0.42 / 0.74 / 0.87 (0.83–1.12) | 0.42 / 0.74 / 0.91 (0.87–1.25) | 0.42 / 0.74 / 1.00 (0.83–1.95) | 0.42 / 0.74 / 0.95 (0.91–1.04) | 0.42 / 0.74 / 0.91 (0.87–0.95) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 3.90 / 6.02 / 13.06 (10.50–39.42) | 3.87 / 6.08 / 14.08 (9.47–143.36) | 3.20 / 5.12 / 6.59 (4.99–7.94) | 3.46 / 5.76 / 8.96 (7.10–11.01) | 2.62 / 4.48 / 6.34 (5.25–11.26) | 2.50 / 4.35 / 7.04 (4.99–10.75) | 0.54 / 1.15 / 1.57 (1.49–1.58) | 0.54 / 1.15 / 1.54 (1.46–1.57) | 0.58 / 1.15 / 1.49 (1.41–1.70) | 65% | 21 |
| 10 | 1000 | 200 | 8 | 4.10 / 9.73 / 28.93 (14.59–262.14) | 3.94 / 8.45 / 16.64 (14.21–262.14) | 3.90 / 6.53 / 16.38 (7.81–327.68) | 3.81 / 6.85 / 19.46 (9.47–190.46) | 2.53 / 4.61 / 7.17 (6.02–13.70) | 2.53 / 4.67 / 6.21 (5.82–29.18) | 0.58 / 1.36 / 2.00 (1.87–7.23) | 0.54 / 1.25 / 1.79 (1.62–8.19) | 0.62 / 1.33 / 1.70 (1.66–5.31) | 49% | 50 |
| 10 | 1000 | 1000 | 2 | 3.39 / 4.54 / 9.86 (4.61–16.64) | 2.72 / 3.78 / 8.45 (6.91–12.29) | 1.15 / 1.54 / 2.08 (1.87–18.69) | 1.15 / 1.70 / 2.37 (2.27–11.26) | 1.12 / 1.54 / 2.27 (2.00–7.42) | 1.15 / 1.70 / 2.30 (2.30–12.16) | 1.12 / 1.54 / 2.21 (2.00–8.45) | 1.15 / 1.90 / 4.06 (2.30–16.90) | 1.15 / 1.62 / 2.43 (2.30–9.60) | 98% | 7 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.66 / 14.46 (7.04–21.50) | 3.90 / 6.27 / 15.74 (6.21–30.72) | 2.85 / 4.80 / 8.96 (4.99–21.76) | 2.85 / 4.06 / 7.36 (4.74–8.32) | 1.20 / 2.30 / 2.50 (2.43–3.14) | 1.20 / 2.08 / 2.50 (2.40–6.72) | 1.20 / 2.30 / 2.50 (2.40–13.95) | 1.20 / 2.24 / 2.91 (2.43–5.18) | 1.20 / 2.11 / 2.43 (2.40–2.69) | 99% | 13 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.17 / 19.20 (11.26–27.65) | 4.42 / 6.72 / 23.30 (7.17–33.79) | 2.94 / 4.29 / 8.83 (5.31–16.90) | 2.98 / 4.54 / 9.09 (5.82–18.43) | 1.20 / 2.40 / 3.90 (3.46–11.14) | 1.20 / 2.27 / 2.85 (2.50–9.86) | 1.20 / 2.40 / 3.52 (3.52–9.47) | 1.20 / 2.37 / 3.52 (3.49–8.96) | 1.20 / 2.27 / 4.61 (2.53–8.06) | 97% | 18 |
| 10 | 1000 | 1000 | 6 | 6.02 / 10.24 / 21.25 (12.80–80.90) | 5.31 / 8.96 / 20.74 (16.38–286.72) | 4.48 / 7.36 / 10.75 (10.50–43.01) | 4.61 / 7.62 / 12.29 (10.50–14.21) | 3.39 / 5.95 / 9.34 (8.00–13.44) | 3.55 / 6.53 / 11.26 (9.47–16.00) | 1.36 / 3.78 / 5.18 (4.99–5.95) | 1.36 / 3.39 / 5.12 (4.35–22.27) | 1.41 / 2.82 / 3.87 (3.58–8.45) | 66% | 50 |
| 10 | 1000 | 1000 | 8 | 6.85 / 12.16 / 25.34 (18.18–401.41) | 6.21 / 11.52 / 18.94 (16.38–19.97) | 5.25 / 10.24 / 14.34 (12.29–61.95) | 5.12 / 10.11 / 14.08 (13.31–159.74) | 3.55 / 6.72 / 10.62 (9.73–16.90) | 3.65 / 7.62 / 11.01 (10.75–29.95) | 1.36 / 4.03 / 5.50 (5.31–6.91) | 1.41 / 4.10 / 8.58 (6.21–51.20) | 1.49 / 3.20 / 4.48 (4.16–5.57) | 49% | 75 |

Derive (addition) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.62 / 0.87 / 6.53 (1.20–548.86) | 0.25 / 0.78 / 3.49 (0.79–4.16) | 0.21 / 0.33 / 0.46 (0.42–3.20) | 0.25 / 0.42 / 3.26 (0.58–6.66) | 0.21 / 0.29 / 0.37 (0.33–3.26) | 0.25 / 0.37 / 0.54 (0.42–0.66) | 0.21 / 0.29 / 0.42 (0.37–6.27) | 0.21 / 0.33 / 0.58 (0.33–3.65) | 0.25 / 0.37 / 0.42 (0.33–9.34) | 99% | 2 |
| 1 | 0 | 50 | 3 | 0.79 / 1.12 / 8.83 (5.63–36.86) | 0.54 / 2.11 / 11.14 (8.58–33.28) | 0.37 / 0.83 / 3.94 (0.87–4.42) | 0.33 / 0.87 / 3.87 (0.91–6.46) | 0.25 / 0.33 / 3.20 (0.42–3.58) | 0.25 / 0.37 / 3.33 (0.83–3.49) | 0.25 / 0.33 / 3.39 (0.42–3.46) | 0.25 / 0.33 / 0.46 (0.42–3.20) | 0.25 / 0.37 / 3.33 (0.62–4.86) | 99% | 3 |
| 1 | 0 | 50 | 4 | 0.87 / 4.29 / 14.34 (1.36–18.94) | 0.58 / 0.91 / 3.68 (1.33–7.30) | 0.37 / 1.07 / 4.42 (3.87–6.21) | 0.50 / 1.36 / 4.35 (3.49–6.27) | 0.25 / 0.50 / 3.36 (0.58–6.02) | 0.29 / 3.26 / 4.86 (0.50–5.18) | 0.29 / 0.66 / 3.39 (0.42–6.21) | 0.25 / 3.23 / 3.46 (0.50–5.95) | 0.29 / 0.74 / 3.39 (0.37–6.08) | 99% | 0 |
| 1 | 0 | 50 | 6 | 1.49 / 4.03 / 43.01 (12.29–194.56) | 0.91 / 1.79 / 9.34 (2.14–46.08) | 1.07 / 2.08 / 2.94 (2.78–8.06) | 1.07 / 2.14 / 2.78 (2.53–10.50) | 0.58 / 1.74 / 2.72 (2.24–16.90) | 0.58 / 2.14 / 3.07 (2.40–4.29) | 0.37 / 0.70 / 1.00 (0.74–1.15) | 0.37 / 0.70 / 0.91 (0.78–3.23) | 0.42 / 0.70 / 1.04 (0.83–3.65) | 65% | 32 |
| 1 | 0 | 50 | 8 | 1.78 / 6.78 / 24.32 (9.98–655.36) | 1.00 / 2.03 / 6.98 (5.12–3899.39) | 1.20 / 2.11 / 4.99 (2.37–9.34) | 1.20 / 2.03 / 5.38 (2.37–18.43) | 0.83 / 2.37 / 3.81 (3.20–6.53) | 0.83 / 2.21 / 3.33 (2.78–9.34) | 0.42 / 0.70 / 1.04 (0.91–7.94) | 0.42 / 0.70 / 1.04 (0.95–5.44) | 0.42 / 0.62 / 0.91 (0.83–0.95) | 49% | 170 |
| 1 | 0 | 200 | 2 | 0.91 / 1.25 / 10.50 (1.25–27.90) | 0.37 / 1.07 / 1.62 (1.07–16.13) | 0.33 / 0.46 / 3.23 (0.70–6.21) | 0.37 / 0.50 / 3.49 (0.50–27.65) | 0.33 / 0.46 / 6.02 (0.58–7.36) | 0.37 / 0.54 / 3.49 (0.58–3.65) | 0.33 / 0.46 / 1.78 (0.46–6.08) | 0.33 / 0.46 / 0.66 (0.46–3.52) | 0.37 / 0.50 / 0.58 (0.50–5.95) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 2.94 / 14.46 (5.95–28.93) | 0.70 / 1.12 / 5.63 (1.15–13.06) | 0.50 / 1.07 / 3.90 (1.15–5.63) | 0.50 / 1.12 / 3.94 (1.12–2621.44) | 0.37 / 0.70 / 3.26 (0.62–6.08) | 0.42 / 0.50 / 0.70 (0.62–4.74) | 0.37 / 0.46 / 0.79 (0.70–4.42) | 0.37 / 0.46 / 0.78 (0.66–3.49) | 0.42 / 0.50 / 3.52 (0.66–4.29) | 99% | 11 |
| 1 | 0 | 200 | 4 | 1.15 / 4.29 / 16.13 (5.95–28.67) | 0.78 / 3.52 / 8.32 (1.54–17.66) | 0.74 / 3.36 / 8.83 (1.46–25.60) | 0.74 / 3.46 / 5.76 (1.36–10.11) | 0.42 / 3.26 / 3.62 (3.39–10.37) | 0.42 / 0.54 / 5.06 (0.58–8.06) | 0.37 / 0.74 / 3.52 (0.78–6.34) | 0.42 / 0.62 / 3.87 (3.49–5.44) | 0.42 / 0.54 / 3.62 (3.58–3.90) | 99% | 2 |
| 1 | 0 | 200 | 6 | 1.95 / 3.20 / 15.10 (10.24–1097.73) | 1.12 / 2.11 / 7.17 (3.26–319.49) | 1.36 / 2.50 / 3.36 (2.94–7.17) | 1.25 / 2.50 / 3.49 (2.40–4390.91) | 0.70 / 2.27 / 2.94 (2.08–10.11) | 0.70 / 2.37 / 3.26 (2.27–4.03) | 0.54 / 0.83 / 1.28 (1.15–6.91) | 0.50 / 0.83 / 1.25 (1.12–1.49) | 0.62 / 0.91 / 1.20 (1.04–1.82) | 65% | 40 |
| 1 | 0 | 200 | 8 | 2.00 / 4.35 / 13.82 (9.47–17.66) | 1.25 / 2.53 / 6.40 (3.52–81.92) | 1.36 / 2.40 / 4.06 (3.07–31.74) | 1.41 / 2.66 / 9.22 (3.49–169.98) | 1.15 / 2.72 / 5.12 (2.69–8.45) | 1.12 / 2.69 / 3.74 (3.07–3.87) | 0.54 / 0.91 / 1.28 (1.12–1.87) | 0.50 / 0.83 / 1.25 (1.15–1.54) | 0.54 / 0.91 / 1.20 (1.15–2.24) | 49% | 10 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.62 / 13.44 (5.63–22.02) | 1.62 / 2.78 / 4.42 (2.82–9.73) | 1.12 / 1.25 / 3.23 (1.66–7.10) | 1.15 / 1.41 / 4.22 (1.62–6.08) | 1.12 / 1.25 / 5.82 (1.70–10.37) | 1.15 / 1.28 / 2.03 (1.28–5.12) | 1.12 / 1.20 / 4.99 (1.82–13.06) | 1.12 / 1.28 / 4.16 (2.21–10.24) | 1.15 / 1.28 / 4.29 (1.28–10.24) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.94 / 4.67 / 15.10 (6.66–33.28) | 1.70 / 2.82 / 10.75 (7.10–11.14) | 1.28 / 2.50 / 2.94 (2.78–30.98) | 1.28 / 2.62 / 4.29 (2.85–25.34) | 1.15 / 1.28 / 2.40 (2.30–20.22) | 1.20 / 1.28 / 4.93 (1.90–5.89) | 1.15 / 1.33 / 2.37 (2.11–3.23) | 1.15 / 1.33 / 7.87 (4.10–10.11) | 1.20 / 1.28 / 4.61 (2.40–10.88) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.26 / 5.82 / 10.24 (6.53–17.15) | 1.74 / 2.91 / 4.99 (4.29–28.93) | 1.70 / 2.94 / 4.86 (4.29–17.92) | 1.70 / 3.10 / 5.70 (3.04–14.34) | 1.20 / 1.82 / 4.22 (2.30–7.49) | 1.20 / 1.79 / 4.74 (2.08–8.58) | 1.20 / 3.14 / 4.22 (2.30–4.80) | 1.20 / 2.08 / 4.35 (4.22–6.34) | 1.20 / 1.82 / 4.42 (2.40–6.66) | 99% | 0 |
| 1 | 0 | 1000 | 6 | 4.06 / 9.60 / 26.62 (6.85–1540.10) | 2.30 / 4.00 / 8.83 (6.53–12.54) | 2.24 / 3.87 / 6.27 (4.48–11.01) | 2.24 / 3.87 / 6.46 (4.80–9.86) | 1.54 / 3.20 / 4.06 (3.78–7.87) | 1.49 / 3.20 / 4.35 (3.81–8.00) | 1.33 / 2.27 / 2.72 (2.62–7.55) | 1.33 / 2.27 / 3.26 (2.85–7.17) | 1.36 / 1.79 / 4.93 (2.53–8.70) | 65% | 28 |
| 1 | 0 | 1000 | 8 | 4.35 / 7.17 / 49.15 (11.52–761.86) | 2.37 / 4.61 / 10.50 (6.02–81.92) | 2.50 / 4.10 / 5.44 (4.42–11.26) | 2.43 / 4.10 / 9.73 (4.42–89.09) | 2.11 / 3.74 / 4.80 (4.03–13.06) | 2.08 / 3.55 / 4.86 (4.29–10.50) | 1.36 / 2.43 / 3.23 (2.78–3.74) | 1.36 / 2.40 / 3.07 (2.56–8.83) | 1.46 / 2.00 / 2.98 (2.91–8.83) | 49% | 68 |
| 1 | 1000 | 50 | 2 | 1.58 / 1.82 / 2.00 (1.90–6.14) | 0.21 / 1.70 / 3.14 (1.87–11.14) | 0.21 / 0.29 / 0.33 (0.33–0.46) | 0.25 / 0.37 / 0.42 (0.37–0.83) | 0.21 / 0.33 / 0.46 (0.33–3.33) | 0.25 / 0.37 / 0.37 (0.37–0.62) | 0.21 / 0.29 / 0.37 (0.33–0.50) | 0.21 / 0.33 / 0.37 (0.33–5.57) | 0.25 / 0.37 / 0.42 (0.37–3.26) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.72 / 3.07 / 7.74 (5.31–13.70) | 1.54 / 1.79 / 4.99 (1.95–8.06) | 0.37 / 1.82 / 2.11 (1.90–5.31) | 0.37 / 1.82 / 4.48 (2.14–8.32) | 0.25 / 0.33 / 3.33 (0.42–5.57) | 0.29 / 0.42 / 3.33 (0.37–5.50) | 0.25 / 0.33 / 3.20 (0.46–3.55) | 0.25 / 0.33 / 0.50 (0.37–3.46) | 0.29 / 0.37 / 3.39 (0.42–3.68) | 99% | 3 |
| 1 | 1000 | 50 | 4 | 2.82 / 4.48 / 20.99 (7.49–32.51) | 1.54 / 2.00 / 5.82 (2.08–11.39) | 0.74 / 2.56 / 5.31 (5.12–7.10) | 0.58 / 3.23 / 4.86 (2.37–8.19) | 0.29 / 0.33 / 0.50 (0.42–3.78) | 0.29 / 0.42 / 3.39 (3.33–3.49) | 0.29 / 0.33 / 3.36 (0.42–4.86) | 0.29 / 0.58 / 3.46 (0.87–3.78) | 0.29 / 0.37 / 3.33 (0.42–3.65) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.65 / 4.54 / 12.67 (11.01–1458.18) | 1.95 / 3.20 / 4.03 (3.87–606.21) | 2.08 / 3.10 / 4.22 (3.55–7.62) | 2.21 / 3.26 / 4.35 (4.03–9.60) | 0.66 / 3.07 / 4.03 (3.52–6.34) | 0.66 / 3.20 / 4.42 (3.33–8.00) | 0.42 / 0.74 / 1.00 (0.83–1.04) | 0.37 / 0.74 / 0.95 (0.70–1.07) | 0.46 / 0.70 / 0.91 (0.70–0.95) | 66% | 20 |
| 1 | 1000 | 50 | 8 | 3.78 / 5.63 / 15.87 (13.70–110.59) | 2.08 / 3.49 / 8.00 (5.95–487.42) | 2.21 / 3.33 / 5.70 (3.68–12.29) | 2.37 / 3.46 / 7.62 (4.03–235.52) | 0.83 / 3.33 / 4.35 (4.22–38.91) | 0.87 / 3.78 / 4.48 (4.06–25.86) | 0.50 / 0.79 / 1.07 (0.87–1.12) | 0.42 / 0.74 / 1.00 (0.91–1.12) | 0.54 / 0.78 / 1.07 (0.83–1.12) | 49% | 29 |
| 1 | 1000 | 200 | 2 | 1.90 / 2.24 / 11.26 (5.95–19.20) | 0.33 / 2.08 / 6.08 (2.11–17.92) | 0.37 / 0.46 / 0.70 (0.54–3.62) | 0.37 / 0.50 / 3.39 (0.66–10.62) | 0.33 / 0.46 / 0.66 (0.46–6.08) | 0.37 / 0.50 / 0.74 (0.50–3.74) | 0.33 / 0.46 / 0.78 (0.62–6.08) | 0.33 / 0.46 / 0.79 (0.46–3.39) | 0.37 / 0.50 / 0.54 (0.50–3.23) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 2.98 / 3.55 / 8.32 (3.65–22.27) | 1.70 / 2.03 / 4.29 (2.11–11.39) | 0.50 / 2.08 / 3.52 (2.03–10.62) | 0.50 / 2.03 / 2.14 (2.11–4.67) | 0.37 / 0.54 / 1.62 (0.66–3.39) | 0.42 / 0.50 / 0.79 (0.54–3.46) | 0.37 / 0.46 / 0.70 (0.62–3.36) | 0.42 / 0.46 / 0.74 (0.66–6.27) | 0.42 / 0.50 / 3.52 (0.50–4.42) | 99% | 0 |
| 1 | 1000 | 200 | 4 | 3.14 / 4.61 / 18.69 (4.99–27.39) | 1.74 / 2.27 / 7.55 (4.61–16.64) | 0.83 / 2.37 / 5.12 (2.66–7.23) | 0.95 / 2.24 / 2.72 (2.50–6.21) | 0.42 / 0.70 / 3.39 (3.36–5.31) | 0.42 / 0.50 / 3.52 (0.78–3.68) | 0.42 / 0.54 / 3.39 (0.79–3.90) | 0.42 / 0.74 / 3.74 (0.78–3.81) | 0.42 / 0.62 / 3.52 (0.70–6.34) | 99% | 0 |
| 1 | 1000 | 200 | 6 | 3.81 / 4.99 / 10.88 (10.24–1458.18) | 2.14 / 3.49 / 8.58 (4.86–1146.88) | 2.37 / 3.49 / 5.06 (4.61–7.87) | 2.37 / 3.39 / 4.42 (3.94–11.26) | 0.78 / 3.26 / 4.06 (3.49–4.54) | 0.74 / 3.36 / 4.42 (4.16–9.98) | 0.54 / 0.87 / 1.25 (1.25–4.35) | 0.50 / 0.79 / 1.15 (1.07–2.24) | 0.58 / 0.91 / 1.25 (1.07–1.49) | 64% | 36 |
| 1 | 1000 | 200 | 8 | 4.03 / 8.32 / 319.49 (12.80–655.36) | 2.27 / 3.94 / 8.70 (4.93–76.80) | 2.43 / 3.74 / 4.80 (4.42–103.42) | 2.43 / 3.58 / 4.74 (4.29–43.01) | 1.28 / 3.78 / 4.86 (4.42–9.86) | 1.15 / 3.87 / 4.93 (4.48–45.06) | 0.54 / 0.87 / 1.36 (1.15–6.02) | 0.54 / 0.87 / 1.20 (1.12–3.87) | 0.58 / 0.91 / 1.25 (1.00–1.33) | 48% | 41 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.61 / 13.06 (4.61–18.43) | 2.62 / 3.78 / 11.01 (3.87–28.42) | 1.12 / 1.25 / 2.03 (1.25–5.50) | 1.15 / 1.28 / 1.62 (1.33–5.18) | 1.12 / 1.20 / 2.08 (1.49–6.78) | 1.15 / 1.28 / 2.27 (1.28–5.63) | 1.12 / 1.20 / 2.00 (1.36–16.26) | 1.12 / 1.25 / 2.11 (1.33–4.16) | 1.15 / 1.28 / 1.54 (1.28–4.10) | 100% | 3 |
| 1 | 1000 | 1000 | 3 | 4.67 / 6.53 / 9.98 (8.32–25.34) | 2.69 / 3.81 / 9.98 (4.16–12.29) | 1.28 / 3.46 / 6.91 (3.90–28.16) | 1.28 / 3.55 / 7.87 (4.00–9.73) | 1.15 / 1.36 / 3.26 (2.00–5.44) | 1.20 / 1.28 / 4.67 (2.08–7.36) | 1.15 / 1.28 / 2.53 (2.30–20.99) | 1.20 / 1.70 / 5.50 (2.21–9.60) | 1.20 / 1.28 / 2.37 (2.11–15.62) | 99% | 0 |
| 1 | 1000 | 1000 | 4 | 5.12 / 8.83 / 17.15 (9.60–47.62) | 2.78 / 5.70 / 8.45 (5.57–15.87) | 2.69 / 3.81 / 6.02 (3.94–15.74) | 2.69 / 3.90 / 7.68 (4.16–22.02) | 1.20 / 2.37 / 5.06 (3.26–6.66) | 1.20 / 1.90 / 4.61 (4.03–7.68) | 1.20 / 3.14 / 5.12 (4.22–5.76) | 1.20 / 2.37 / 4.74 (2.24–11.39) | 1.20 / 1.87 / 4.74 (2.14–11.39) | 98% | 7 |
| 1 | 1000 | 1000 | 6 | 6.14 / 11.39 / 397.31 (23.81–17563.65) | 3.23 / 5.82 / 188.42 (15.49–671.74) | 3.23 / 4.67 / 10.62 (5.89–14.21) | 3.26 / 4.74 / 5.95 (5.31–9.86) | 1.62 / 4.16 / 4.99 (4.74–6.85) | 1.49 / 3.90 / 5.44 (4.42–6.21) | 1.33 / 2.43 / 2.91 (2.62–5.82) | 1.28 / 2.27 / 3.26 (2.43–9.34) | 1.36 / 1.74 / 2.85 (2.72–5.12) | 65% | 73 |
| 1 | 1000 | 1000 | 8 | 6.40 / 10.11 / 24.83 (14.08–1769.47) | 3.55 / 6.08 / 8.58 (8.32–278.53) | 3.39 / 5.25 / 8.96 (6.40–22.53) | 3.46 / 5.31 / 10.75 (6.40–16.38) | 2.56 / 4.29 / 11.65 (6.40–12.93) | 2.40 / 4.48 / 5.57 (4.93–44.03) | 1.36 / 2.53 / 2.94 (2.78–7.42) | 1.36 / 2.43 / 2.94 (2.66–4.10) | 1.36 / 2.14 / 2.91 (2.66–8.70) | 48% | 43 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 4.48 (0.91–17.66) | 0.37 / 0.74 / 1.70 (0.87–9.47) | 0.25 / 0.33 / 0.50 (0.46–0.54) | 0.25 / 0.37 / 0.54 (0.50–0.54) | 0.25 / 0.33 / 0.50 (0.50–0.79) | 0.25 / 0.37 / 0.50 (0.50–0.66) | 0.25 / 0.33 / 0.50 (0.46–0.50) | 0.21 / 0.33 / 0.50 (0.50–0.54) | 0.25 / 0.37 / 0.50 (0.46–0.54) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.79 / 1.15 / 6.53 (5.12–17.66) | 0.54 / 0.87 / 3.33 (1.12–8.58) | 0.37 / 0.87 / 1.12 (0.95–7.36) | 0.37 / 0.83 / 0.95 (0.95–4.99) | 0.25 / 0.46 / 0.58 (0.54–1.28) | 0.25 / 0.42 / 0.58 (0.54–0.66) | 0.25 / 0.42 / 0.54 (0.54–0.66) | 0.25 / 0.46 / 0.54 (0.54–0.62) | 0.25 / 0.42 / 0.58 (0.54–0.70) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.87 / 1.49 / 10.62 (5.25–16.64) | 0.58 / 1.00 / 2.08 (1.15–10.88) | 0.54 / 1.33 / 3.07 (1.70–6.66) | 0.54 / 1.25 / 1.62 (1.54–6.53) | 0.29 / 0.50 / 0.87 (0.54–2.40) | 0.29 / 0.54 / 0.70 (0.58–4.06) | 0.29 / 0.50 / 0.70 (0.58–1.95) | 0.29 / 0.54 / 0.83 (0.58–3.14) | 0.29 / 0.54 / 0.79 (0.58–3.26) | 99% | 0 |
| 10 | 0 | 50 | 6 | 1.46 / 3.20 / 12.80 (6.53–15.62) | 0.91 / 2.27 / 4.35 (3.04–12.80) | 1.07 / 2.50 / 3.55 (3.10–10.11) | 1.12 / 2.50 / 4.29 (3.33–8.70) | 0.58 / 2.14 / 3.07 (2.82–5.18) | 0.58 / 2.03 / 3.04 (2.66–5.95) | 0.42 / 0.83 / 1.25 (1.04–1.33) | 0.42 / 1.00 / 1.33 (1.25–1.41) | 0.42 / 0.83 / 1.15 (0.95–1.28) | 66% | 112 |
| 10 | 0 | 50 | 8 | 1.78 / 4.54 / 13.57 (10.62–15.62) | 1.07 / 3.20 / 5.76 (5.18–17.66) | 1.28 / 3.10 / 4.67 (4.29–10.24) | 1.33 / 2.98 / 4.74 (3.81–6.08) | 0.95 / 2.91 / 4.00 (3.94–4.74) | 1.00 / 2.91 / 4.06 (3.87–4.29) | 0.42 / 0.91 / 1.28 (1.15–1.33) | 0.46 / 1.04 / 1.46 (1.20–1.46) | 0.46 / 0.95 / 1.28 (1.20–1.49) | 49% | 28 |
| 10 | 0 | 200 | 2 | 0.87 / 1.20 / 3.65 (2.50–7.81) | 0.58 / 1.07 / 3.14 (1.20–5.50) | 0.33 / 0.50 / 0.74 (0.74–0.74) | 0.37 / 0.50 / 0.79 (0.74–0.83) | 0.33 / 0.50 / 0.74 (0.74–0.74) | 0.37 / 0.50 / 0.79 (0.74–0.83) | 0.33 / 0.50 / 0.74 (0.70–0.95) | 0.33 / 0.58 / 0.74 (0.74–0.79) | 0.37 / 0.50 / 0.83 (0.74–1.36) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.04 / 1.66 / 14.34 (4.86–23.04) | 0.70 / 1.20 / 5.31 (1.33–10.62) | 0.50 / 1.07 / 1.36 (1.25–4.42) | 0.50 / 1.07 / 1.46 (1.25–6.78) | 0.42 / 0.66 / 0.91 (0.79–2.03) | 0.42 / 0.66 / 0.91 (0.83–6.78) | 0.42 / 0.66 / 0.83 (0.83–4.16) | 0.42 / 0.70 / 0.87 (0.83–3.33) | 0.42 / 0.66 / 1.07 (0.83–3.52) | 99% | 4 |
| 10 | 0 | 200 | 4 | 1.15 / 1.95 / 16.00 (6.02–23.04) | 0.78 / 1.36 / 8.19 (1.62–159.74) | 0.74 / 1.41 / 2.40 (1.70–18.18) | 0.74 / 1.54 / 2.11 (1.90–26.37) | 0.42 / 0.74 / 1.00 (0.87–8.83) | 0.42 / 0.74 / 0.95 (0.91–7.23) | 0.42 / 0.74 / 0.87 (0.83–3.52) | 0.42 / 0.78 / 1.15 (0.91–3.46) | 0.42 / 0.74 / 0.91 (0.87–1.15) | 95% | 21 |
| 10 | 0 | 200 | 6 | 1.82 / 3.62 / 13.70 (8.58–35.33) | 1.12 / 2.72 / 5.31 (3.87–10.37) | 1.33 / 2.78 / 3.94 (3.26–6.40) | 1.33 / 3.10 / 4.54 (4.35–7.87) | 0.70 / 2.27 / 4.16 (3.14–5.82) | 0.70 / 2.40 / 3.62 (3.23–4.22) | 0.54 / 1.12 / 1.49 (1.49–1.58) | 0.54 / 1.15 / 1.49 (1.36–1.54) | 0.58 / 1.25 / 1.54 (1.41–7.36) | 66% | 31 |
| 10 | 0 | 200 | 8 | 2.11 / 5.63 / 18.43 (13.95–3538.94) | 1.28 / 3.68 / 13.06 (5.25–161.79) | 1.49 / 3.55 / 9.47 (4.86–64.00) | 1.49 / 3.55 / 11.39 (4.80–45.06) | 1.25 / 3.04 / 4.61 (4.00–9.98) | 1.20 / 3.07 / 4.48 (4.29–8.06) | 0.54 / 1.36 / 2.00 (1.87–2.21) | 0.54 / 1.25 / 1.70 (1.62–1.74) | 0.62 / 1.33 / 1.79 (1.66–1.82) | 49% | 377 |
| 10 | 0 | 1000 | 2 | 2.43 / 3.55 / 7.62 (3.68–8.70) | 1.62 / 2.72 / 2.98 (2.78–6.91) | 1.15 / 1.49 / 2.03 (1.87–4.29) | 1.15 / 1.57 / 2.30 (2.24–2.30) | 1.15 / 1.41 / 1.74 (1.70–2.27) | 1.15 / 1.62 / 2.37 (2.27–36.35) | 1.15 / 1.41 / 1.82 (1.74–11.90) | 1.15 / 1.87 / 2.27 (2.27–34.82) | 1.15 / 1.74 / 2.37 (2.30–15.49) | 99% | 11 |
| 10 | 0 | 1000 | 3 | 2.72 / 4.61 / 7.49 (5.57–24.06) | 1.70 / 3.14 / 7.30 (3.78–11.52) | 1.28 / 2.43 / 2.98 (2.91–3.39) | 1.28 / 2.62 / 3.07 (2.98–15.62) | 1.20 / 2.30 / 2.53 (2.50–7.10) | 1.20 / 2.11 / 2.43 (2.40–7.87) | 1.20 / 2.30 / 2.50 (2.43–2.53) | 1.20 / 2.27 / 3.36 (2.62–8.32) | 1.20 / 2.11 / 2.43 (2.43–7.62) | 99% | 1 |
| 10 | 0 | 1000 | 4 | 3.23 / 5.06 / 8.96 (8.32–15.10) | 1.74 / 3.74 / 5.70 (4.80–12.54) | 1.74 / 2.94 / 3.55 (3.49–23.81) | 1.74 / 3.14 / 7.10 (3.62–10.88) | 1.20 / 2.43 / 3.62 (3.52–9.86) | 1.20 / 2.27 / 2.78 (2.56–9.34) | 1.20 / 2.40 / 4.03 (3.46–6.02) | 1.20 / 2.37 / 3.39 (3.33–4.93) | 1.20 / 2.27 / 2.72 (2.62–3.07) | 99% | 18 |
| 10 | 0 | 1000 | 6 | 4.22 / 7.49 / 13.18 (12.29–31.74) | 2.40 / 5.70 / 8.70 (7.94–10.24) | 2.43 / 4.86 / 7.04 (6.21–8.83) | 2.43 / 5.31 / 7.55 (6.91–8.19) | 1.54 / 3.94 / 5.70 (5.12–5.82) | 1.62 / 4.29 / 6.53 (5.12–7.55) | 1.36 / 3.74 / 4.16 (4.10–5.25) | 1.36 / 3.39 / 5.25 (4.48–8.06) | 1.46 / 2.91 / 4.03 (3.87–5.44) | 66% | 38 |
| 10 | 0 | 1000 | 8 | 4.80 / 10.11 / 20.99 (16.64–91.14) | 2.85 / 7.87 / 12.67 (9.73–45.06) | 2.72 / 6.53 / 11.14 (9.47–25.60) | 2.69 / 6.85 / 10.75 (8.70–36.35) | 2.37 / 4.74 / 6.85 (6.21–18.69) | 2.43 / 5.06 / 7.42 (6.53–24.83) | 1.36 / 4.00 / 5.31 (5.25–5.57) | 1.41 / 4.03 / 5.82 (5.44–15.74) | 1.41 / 3.14 / 4.48 (4.22–16.90) | 49% | 151 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 6.40 (2.72–11.39) | 0.25 / 1.74 / 4.61 (1.82–7.68) | 0.25 / 0.33 / 0.50 (0.46–0.54) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.25 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.25 / 0.33 / 0.50 (0.50–0.50) | 0.25 / 0.33 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.07 / 7.81 (3.23–8.13) | 1.49 / 1.95 / 3.49 (2.14–13.70) | 0.33 / 1.82 / 1.95 (1.90–2.03) | 0.33 / 1.82 / 1.90 (1.87–2.14) | 0.25 / 0.46 / 0.54 (0.54–0.58) | 0.29 / 0.37 / 0.54 (0.54–0.62) | 0.25 / 0.46 / 0.54 (0.54–0.62) | 0.25 / 0.46 / 0.54 (0.54–0.58) | 0.29 / 0.42 / 0.58 (0.54–0.58) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.82 / 3.49 / 12.29 (8.70–18.18) | 1.54 / 2.08 / 2.53 (2.43–6.46) | 0.42 / 2.43 / 2.78 (2.62–4.86) | 0.42 / 2.43 / 2.72 (2.62–6.02) | 0.29 / 0.50 / 0.62 (0.58–0.83) | 0.29 / 0.46 / 0.58 (0.58–1.00) | 0.29 / 0.54 / 0.83 (0.58–0.87) | 0.29 / 0.50 / 0.58 (0.58–1.36) | 0.29 / 0.46 / 0.74 (0.58–0.74) | 99% | 0 |
| 10 | 1000 | 50 | 6 | 3.46 / 4.99 / 11.90 (9.22–13.44) | 1.95 / 3.90 / 5.25 (4.86–29.18) | 2.00 / 4.16 / 5.44 (4.48–5.70) | 1.95 / 3.81 / 4.86 (4.61–5.31) | 0.62 / 3.65 / 4.74 (4.10–4.93) | 0.58 / 3.49 / 4.29 (4.06–5.06) | 0.42 / 0.87 / 1.12 (1.12–1.28) | 0.46 / 1.04 / 1.36 (1.25–1.36) | 0.50 / 1.00 / 1.28 (1.12–1.28) | 66% | 47 |
| 10 | 1000 | 50 | 8 | 3.87 / 6.59 / 13.44 (12.67–15.23) | 2.11 / 4.80 / 8.45 (6.72–11.14) | 2.27 / 4.74 / 6.85 (5.18–8.32) | 2.27 / 4.67 / 6.34 (5.70–7.87) | 0.87 / 4.06 / 5.31 (4.86–6.66) | 0.87 / 4.03 / 4.93 (4.86–5.31) | 0.42 / 0.91 / 1.25 (1.20–1.33) | 0.46 / 1.07 / 1.49 (1.25–1.62) | 0.50 / 1.00 / 1.36 (1.28–1.46) | 49% | 0 |
| 10 | 1000 | 200 | 2 | 1.87 / 2.21 / 5.44 (2.27–7.04) | 0.42 / 2.08 / 3.94 (2.14–9.73) | 0.33 / 0.50 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.79) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.83) | 0.33 / 0.54 / 0.74 (0.74–0.74) | 0.33 / 0.58 / 0.74 (0.74–0.78) | 0.37 / 0.50 / 0.79 (0.74–0.79) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.52 / 7.36 (4.03–11.26) | 1.70 / 2.24 / 4.48 (2.50–4.86) | 0.50 / 2.00 / 2.24 (2.14–8.32) | 0.50 / 2.03 / 2.40 (2.14–6.78) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.42 / 0.62 / 0.87 (0.83–0.87) | 0.37 / 0.66 / 0.79 (0.79–0.83) | 0.42 / 0.70 / 0.83 (0.79–0.91) | 0.42 / 0.66 / 0.87 (0.83–6.78) | 99% | 3 |
| 10 | 1000 | 200 | 4 | 3.10 / 3.81 / 9.09 (7.30–12.29) | 1.74 / 2.40 / 2.72 (2.66–7.30) | 0.70 / 2.62 / 4.10 (2.78–8.06) | 0.62 / 2.62 / 2.91 (2.72–7.36) | 0.42 / 0.74 / 0.87 (0.87–1.04) | 0.42 / 0.74 / 1.00 (0.87–1.28) | 0.42 / 0.74 / 0.95 (0.87–1.12) | 0.42 / 0.74 / 1.00 (0.87–1.12) | 0.42 / 0.74 / 0.87 (0.87–0.95) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 3.90 / 5.95 / 12.67 (10.62–29.44) | 2.21 / 4.54 / 7.55 (5.50–22.27) | 2.11 / 3.90 / 5.06 (3.90–5.57) | 2.24 / 4.54 / 5.76 (4.86–6.21) | 0.83 / 3.87 / 4.86 (4.06–7.49) | 0.74 / 3.74 / 4.86 (4.16–7.87) | 0.54 / 1.15 / 1.54 (1.49–1.66) | 0.54 / 1.15 / 1.54 (1.49–1.66) | 0.58 / 1.15 / 1.54 (1.41–1.70) | 65% | 21 |
| 10 | 1000 | 200 | 8 | 4.10 / 9.47 / 29.18 (13.95–253.95) | 2.30 / 5.25 / 10.62 (9.60–55.30) | 2.40 / 4.74 / 10.11 (5.76–52.22) | 2.37 / 4.99 / 13.82 (7.10–48.64) | 1.15 / 4.16 / 6.78 (5.38–9.34) | 1.12 / 4.22 / 5.25 (5.18–15.36) | 0.54 / 1.36 / 2.00 (1.78–9.09) | 0.54 / 1.25 / 1.70 (1.62–6.34) | 0.62 / 1.33 / 1.78 (1.62–5.50) | 49% | 50 |
| 10 | 1000 | 1000 | 2 | 3.39 / 4.54 / 11.26 (6.78–17.15) | 1.87 / 3.74 / 5.44 (4.00–10.62) | 1.15 / 1.49 / 2.00 (1.87–48.64) | 1.15 / 1.66 / 2.37 (2.30–16.13) | 1.12 / 1.54 / 2.08 (1.87–5.31) | 1.15 / 1.62 / 2.30 (2.27–14.34) | 1.12 / 1.54 / 4.86 (1.95–9.47) | 1.15 / 1.95 / 3.20 (2.30–13.44) | 1.15 / 1.70 / 2.40 (2.30–9.47) | 98% | 7 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.85 / 15.36 (9.09–24.58) | 2.69 / 4.48 / 11.39 (4.93–12.16) | 1.28 / 3.20 / 5.82 (3.74–10.88) | 1.28 / 3.39 / 3.87 (3.81–6.40) | 1.20 / 2.30 / 2.43 (2.43–2.78) | 1.20 / 2.08 / 2.43 (2.40–5.12) | 1.20 / 2.30 / 4.00 (2.43–17.15) | 1.20 / 2.27 / 3.10 (2.50–4.10) | 1.20 / 2.11 / 2.72 (2.40–3.33) | 99% | 13 |
| 10 | 1000 | 1000 | 4 | 4.93 / 7.23 / 19.97 (12.16–29.95) | 2.78 / 4.93 / 9.73 (5.38–16.38) | 2.03 / 4.03 / 8.19 (4.42–15.36) | 1.95 / 4.03 / 6.66 (4.86–11.65) | 1.20 / 2.43 / 4.16 (3.52–8.45) | 1.20 / 2.27 / 2.72 (2.50–7.81) | 1.20 / 2.40 / 3.49 (3.49–14.21) | 1.20 / 2.40 / 3.81 (3.26–11.14) | 1.20 / 2.27 / 4.42 (2.53–7.04) | 97% | 18 |
| 10 | 1000 | 1000 | 6 | 6.02 / 9.73 / 19.46 (13.18–73.73) | 3.33 / 6.78 / 11.14 (11.01–26.11) | 3.23 / 6.08 / 9.22 (7.94–13.31) | 3.26 / 6.08 / 9.86 (8.32–10.37) | 1.54 / 4.48 / 7.04 (6.21–8.45) | 1.62 / 4.48 / 7.04 (6.34–9.47) | 1.36 / 3.74 / 5.12 (4.42–6.14) | 1.36 / 3.46 / 4.80 (4.29–8.32) | 1.41 / 2.78 / 3.81 (3.74–4.74) | 66% | 50 |
| 10 | 1000 | 1000 | 8 | 6.78 / 12.16 / 22.02 (19.71–548.86) | 3.94 / 9.22 / 13.31 (11.26–13.57) | 3.52 / 8.00 / 12.16 (10.62–33.79) | 3.39 / 7.62 / 11.39 (10.11–69.63) | 2.72 / 5.70 / 9.22 (7.23–11.26) | 2.66 / 5.70 / 8.45 (7.62–13.70) | 1.36 / 4.00 / 6.40 (5.25–8.32) | 1.41 / 4.06 / 8.58 (5.57–29.18) | 1.49 / 3.23 / 4.74 (4.42–5.31) | 49% | 75 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 18.08; 2/2 fine: 0.20; 2/4 fair: 33.78; 2/4 fine: 0.27; 3 fair: 56.69; 3 slot read: 0.24; 3 fine: 0.24;

Migrations seen: 482008 over 1026456745 samples (one sample per measured operation).


### Suite a2t: IPC call and reply, lock per endpoint

load average before: { 5.48 4.75 3.33 }
load average after: { 4.75 4.89 3.65 }; 173 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 320k (318k–321k) | 319k (318k–320k) | 313k (311k–314k) | 313k (312k–315k) | 313k (311k–313k) | 312k (312k–313k) | 312k (312k–313k) | 312k (311k–313k) | 100% | 0 |
| 0 | 50 | 4 | 637k (634k–640k) | 639k (636k–642k) | 624k (622k–624k) | 624k (606k–626k) | 598k (591k–601k) | 624k (623k–627k) | 597k (581k–603k) | 624k (622k–628k) | 99% | 0 |
| 0 | 50 | 6 | 915k (906k–923k) | 897k (877k–932k) | 866k (844k–891k) | 869k (834k–899k) | 775k (744k–796k) | 884k (856k–894k) | 735k (708k–760k) | 868k (837k–891k) | 67% | 8 |
| 0 | 50 | 8 | 1.22M (1.16M–1.25M) | 1.23M (1.19M–1.26M) | 1.16M (1.13M–1.22M) | 1.16M (1.09M–1.18M) | 968k (907k–1.05M) | 1.16M (1.12M–1.19M) | 837k (812k–884k) | 1.16M (1.08M–1.17M) | 49% | 13 |
| 0 | 200 | 2 | 285k (285k–286k) | 285k (284k–286k) | 280k (278k–282k) | 280k (279k–282k) | 279k (279k–282k) | 280k (279k–282k) | 280k (278k–281k) | 280k (279k–281k) | 100% | 0 |
| 0 | 200 | 4 | 570k (568k–572k) | 571k (570k–571k) | 557k (556k–560k) | 559k (558k–560k) | 536k (520k–540k) | 558k (557k–560k) | 527k (517k–537k) | 558k (554k–559k) | 100% | 0 |
| 0 | 200 | 6 | 811k (801k–832k) | 805k (801k–819k) | 770k (768k–797k) | 797k (756k–801k) | 683k (650k–718k) | 779k (767k–829k) | 643k (620k–664k) | 762k (759k–801k) | 68% | 2 |
| 0 | 200 | 8 | 1.10M (1.00M–1.10M) | 1.10M (1.03M–1.11M) | 1.04M (965k–1.06M) | 1.05M (994k–1.09M) | 858k (842k–924k) | 1.07M (964k–1.10M) | 658k (654k–780k) | 1.05M (974k–1.07M) | 49% | 63 |
| 0 | 1000 | 2 | 200k (199k–201k) | 201k (200k–201k) | 198k (195k–198k) | 197k (197k–199k) | 197k (190k–199k) | 197k (196k–198k) | 198k (197k–198k) | 197k (178k–199k) | 100% | 6 |
| 0 | 1000 | 4 | 400k (394k–400k) | 399k (395k–400k) | 393k (370k–394k) | 392k (387k–395k) | 380k (372k–383k) | 391k (390k–394k) | 381k (377k–382k) | 393k (390k–395k) | 99% | 0 |
| 0 | 1000 | 6 | 583k (573k–595k) | 585k (573k–592k) | 566k (549k–572k) | 556k (549k–570k) | 513k (481k–535k) | 555k (549k–568k) | 330k (326k–350k) | 572k (551k–576k) | 67% | 24 |
| 0 | 1000 | 8 | 787k (772k–795k) | 785k (781k–795k) | 763k (709k–767k) | 754k (710k–764k) | 671k (588k–685k) | 758k (717k–762k) | 332k (329k–339k) | 751k (727k–772k) | 49% | 24 |
| 1000 | 50 | 2 | 194k (194k–195k) | 195k (194k–196k) | 190k (190k–191k) | 190k (187k–191k) | 191k (190k–191k) | 191k (187k–191k) | 191k (189k–191k) | 191k (190k–191k) | 100% | 0 |
| 1000 | 50 | 4 | 390k (388k–391k) | 392k (389k–392k) | 380k (374k–380k) | 380k (378k–381k) | 372k (370k–374k) | 380k (377k–381k) | 371k (370k–373k) | 380k (377k–381k) | 100% | 0 |
| 1000 | 50 | 6 | 563k (562k–571k) | 568k (562k–574k) | 549k (539k–556k) | 548k (530k–553k) | 532k (509k–536k) | 544k (524k–549k) | 500k (498k–522k) | 538k (531k–546k) | 67% | 15 |
| 1000 | 50 | 8 | 758k (756k–764k) | 761k (751k–763k) | 729k (708k–744k) | 735k (710k–746k) | 682k (674k–695k) | 719k (668k–727k) | 661k (652k–679k) | 729k (718k–732k) | 49% | 28 |
| 1000 | 200 | 2 | 181k (175k–181k) | 181k (178k–182k) | 177k (177k–178k) | 177k (177k–178k) | 177k (177k–178k) | 177k (177k–179k) | 177k (177k–178k) | 177k (176k–178k) | 100% | 1 |
| 1000 | 200 | 4 | 362k (358k–363k) | 364k (356k–365k) | 355k (350k–356k) | 353k (344k–357k) | 345k (341k–347k) | 354k (351k–355k) | 345k (335k–347k) | 354k (344k–355k) | 99% | 0 |
| 1000 | 200 | 6 | 527k (524k–529k) | 526k (522k–536k) | 513k (503k–516k) | 511k (498k–517k) | 483k (478k–506k) | 515k (510k–518k) | 468k (464k–485k) | 508k (499k–514k) | 67% | 3 |
| 1000 | 200 | 8 | 698k (689k–710k) | 709k (683k–714k) | 691k (682k–692k) | 675k (647k–687k) | 633k (631k–664k) | 689k (685k–698k) | 593k (582k–636k) | 691k (659k–704k) | 49% | 5 |
| 1000 | 1000 | 2 | 142k (142k–143k) | 143k (142k–143k) | 140k (140k–141k) | 140k (139k–141k) | 140k (139k–141k) | 140k (139k–141k) | 140k (139k–141k) | 140k (139k–141k) | 100% | 0 |
| 1000 | 1000 | 4 | 284k (279k–285k) | 286k (276k–286k) | 280k (273k–280k) | 279k (278k–281k) | 273k (272k–275k) | 280k (278k–280k) | 273k (272k–275k) | 280k (277k–280k) | 100% | 0 |
| 1000 | 1000 | 6 | 419k (412k–420k) | 414k (413k–423k) | 404k (396k–405k) | 408k (400k–411k) | 392k (382k–395k) | 405k (400k–414k) | 333k (328k–345k) | 399k (398k–407k) | 67% | 10 |
| 1000 | 1000 | 8 | 562k (555k–565k) | 547k (544k–560k) | 539k (531k–551k) | 540k (533k–562k) | 513k (505k–518k) | 535k (511k–541k) | 334k (323k–346k) | 539k (497k–546k) | 50% | 16 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 1.87 / 1.95 (1.95–6.08) | 1.74 / 1.87 / 1.95 (1.90–6.98) | 1.82 / 1.90 / 2.08 (2.00–7.74) | 1.82 / 1.90 / 2.08 (2.03–7.10) | 1.82 / 1.90 / 2.08 (2.03–8.00) | 1.82 / 1.90 / 2.08 (2.03–2.21) | 1.82 / 1.90 / 2.03 (2.03–6.02) | 1.82 / 1.90 / 2.03 (2.03–8.58) | 100% | 0 |
| 0 | 50 | 4 | 1.74 / 1.87 / 2.43 (1.90–9.09) | 1.74 / 1.87 / 2.00 (1.90–6.21) | 1.82 / 1.90 / 2.62 (2.00–6.27) | 1.82 / 1.90 / 3.39 (2.00–11.90) | 1.90 / 2.30 / 3.36 (2.40–6.27) | 1.82 / 1.90 / 2.82 (1.95–7.87) | 1.95 / 2.30 / 2.98 (2.40–6.53) | 1.82 / 1.90 / 2.56 (2.00–6.59) | 99% | 0 |
| 0 | 50 | 6 | 1.79 / 2.37 / 2.56 (2.50–9.98) | 2.08 / 2.37 / 2.62 (2.43–15.10) | 1.87 / 2.72 / 2.85 (2.82–13.06) | 1.95 / 2.69 / 2.85 (2.82–13.57) | 2.53 / 3.52 / 6.14 (3.68–23.04) | 1.87 / 2.69 / 3.49 (2.78–10.62) | 2.72 / 3.52 / 5.63 (4.06–12.93) | 2.03 / 2.69 / 4.35 (2.82–10.24) | 67% | 8 |
| 0 | 50 | 8 | 1.82 / 2.37 / 2.98 (2.56–32.51) | 1.82 / 2.37 / 2.82 (2.43–15.62) | 2.08 / 2.72 / 5.31 (3.33–31.49) | 1.87 / 2.72 / 3.20 (2.85–24.32) | 2.85 / 3.65 / 7.17 (5.31–30.21) | 1.87 / 2.69 / 4.86 (2.85–30.46) | 3.39 / 4.16 / 10.24 (9.73–54.78) | 2.11 / 2.72 / 3.39 (2.85–30.46) | 49% | 13 |
| 0 | 200 | 2 | 2.11 / 2.24 / 2.37 (2.30–10.11) | 2.11 / 2.24 / 5.89 (2.30–8.45) | 2.21 / 2.30 / 6.59 (2.40–7.10) | 2.21 / 2.30 / 2.43 (2.40–4.80) | 2.21 / 2.30 / 6.21 (2.37–8.70) | 2.21 / 2.30 / 2.98 (2.37–7.87) | 2.21 / 2.30 / 5.82 (2.37–11.78) | 2.21 / 2.30 / 2.50 (2.40–6.98) | 100% | 0 |
| 0 | 200 | 4 | 2.11 / 2.24 / 4.10 (2.30–7.23) | 2.11 / 2.24 / 3.10 (2.30–5.70) | 2.21 / 2.30 / 3.49 (2.37–9.60) | 2.21 / 2.30 / 2.78 (2.40–6.08) | 2.30 / 2.94 / 4.00 (3.04–7.68) | 2.21 / 2.30 / 3.39 (2.37–8.45) | 2.37 / 2.98 / 4.42 (3.10–9.47) | 2.21 / 2.30 / 3.68 (2.37–11.65) | 100% | 0 |
| 0 | 200 | 6 | 2.37 / 2.72 / 3.07 (2.94–10.50) | 2.50 / 2.72 / 3.65 (2.91–3.78) | 2.53 / 2.98 / 6.66 (3.10–13.18) | 2.27 / 2.94 / 6.08 (3.10–13.06) | 2.98 / 4.10 / 4.93 (4.35–25.60) | 2.30 / 2.98 / 7.62 (3.04–16.64) | 3.39 / 4.16 / 8.06 (4.86–15.74) | 2.72 / 2.98 / 3.14 (3.10–18.18) | 68% | 2 |
| 0 | 200 | 8 | 2.14 / 2.72 / 6.78 (3.04–55.30) | 2.27 / 2.72 / 6.08 (3.14–19.71) | 2.27 / 3.04 / 8.83 (4.67–37.38) | 2.50 / 2.98 / 4.80 (3.46–7.36) | 3.36 / 4.16 / 10.62 (7.36–12.93) | 2.24 / 2.98 / 7.23 (4.00–40.45) | 4.74 / 5.50 / 14.72 (12.93–65.54) | 2.53 / 2.98 / 4.16 (3.68–54.27) | 49% | 63 |
| 0 | 1000 | 2 | 3.62 / 3.74 / 9.34 (3.78–13.82) | 3.62 / 3.74 / 8.32 (5.25–9.34) | 3.65 / 3.78 / 7.49 (4.80–18.18) | 3.65 / 3.81 / 11.01 (3.94–12.42) | 3.65 / 3.81 / 11.01 (3.87–31.49) | 3.65 / 3.78 / 3.94 (3.90–15.74) | 3.65 / 3.78 / 8.83 (3.94–13.06) | 3.65 / 3.78 / 4.74 (3.94–32.00) | 100% | 6 |
| 0 | 1000 | 4 | 3.62 / 3.74 / 7.49 (4.06–19.46) | 3.62 / 3.78 / 9.60 (4.06–16.64) | 3.68 / 4.29 / 9.34 (4.42–25.34) | 3.68 / 4.42 / 12.54 (3.90–19.97) | 3.81 / 5.18 / 11.65 (6.78–24.83) | 3.68 / 4.10 / 15.23 (3.87–19.97) | 3.81 / 5.12 / 11.78 (8.32–23.04) | 3.68 / 3.81 / 8.45 (4.42–16.64) | 99% | 0 |
| 0 | 1000 | 6 | 3.74 / 4.16 / 7.04 (5.18–15.62) | 3.65 / 4.22 / 8.58 (5.38–11.14) | 3.90 / 4.54 / 8.70 (6.21–11.78) | 4.16 / 4.54 / 6.78 (6.46–12.16) | 4.42 / 6.08 / 12.03 (6.66–13.18) | 4.22 / 4.61 / 8.70 (8.32–11.90) | 7.74 / 8.19 / 14.98 (14.46–22.27) | 3.78 / 4.48 / 8.83 (4.74–22.78) | 67% | 24 |
| 0 | 1000 | 8 | 3.65 / 4.22 / 10.37 (6.53–23.30) | 3.68 / 4.22 / 8.19 (6.08–14.08) | 4.00 / 4.54 / 9.47 (8.19–49.15) | 4.03 / 4.61 / 9.34 (6.78–66.56) | 4.42 / 6.34 / 13.44 (10.75–107.52) | 4.00 / 4.54 / 11.26 (9.22–23.04) | 10.11 / 14.21 / 26.62 (23.55–133.12) | 3.87 / 4.61 / 11.65 (9.86–54.27) | 49% | 24 |
| 1000 | 50 | 2 | 3.74 / 3.90 / 11.39 (4.00–11.78) | 3.74 / 3.87 / 9.98 (3.94–12.03) | 3.87 / 4.00 / 8.45 (7.49–10.50) | 3.87 / 4.00 / 9.22 (4.16–15.62) | 3.87 / 4.00 / 9.47 (4.10–10.88) | 3.87 / 4.00 / 9.09 (8.45–19.71) | 3.87 / 4.00 / 9.22 (6.78–16.64) | 3.87 / 4.00 / 8.96 (4.10–10.88) | 100% | 0 |
| 1000 | 50 | 4 | 3.74 / 3.87 / 7.49 (3.94–15.23) | 3.74 / 3.81 / 7.36 (3.94–16.00) | 3.90 / 4.00 / 5.12 (4.16–17.66) | 3.87 / 4.00 / 8.58 (4.74–16.90) | 4.00 / 4.16 / 5.89 (4.29–16.38) | 3.90 / 4.00 / 4.16 (4.06–15.74) | 4.00 / 4.35 / 8.83 (4.29–16.00) | 3.90 / 4.00 / 5.25 (4.06–14.72) | 100% | 0 |
| 1000 | 50 | 6 | 4.03 / 4.42 / 5.44 (4.86–11.78) | 3.87 / 4.42 / 6.66 (5.31–12.54) | 4.10 / 4.80 / 7.04 (5.31–13.95) | 3.90 / 4.86 / 8.96 (5.82–13.57) | 4.29 / 5.06 / 9.22 (6.53–14.72) | 4.10 / 4.93 / 6.27 (5.31–15.74) | 4.61 / 5.12 / 9.60 (6.85–16.38) | 4.22 / 4.86 / 8.58 (5.31–13.82) | 67% | 15 |
| 1000 | 50 | 8 | 3.87 / 4.35 / 10.50 (7.30–15.74) | 3.87 / 4.42 / 9.60 (9.22–12.29) | 4.16 / 4.86 / 10.62 (9.34–15.36) | 3.94 / 4.80 / 10.50 (8.32–13.18) | 4.54 / 5.12 / 11.26 (9.34–54.78) | 4.42 / 4.93 / 10.24 (6.53–27.39) | 4.67 / 5.25 / 13.06 (12.16–28.67) | 3.94 / 4.86 / 10.24 (8.58–36.35) | 49% | 28 |
| 1000 | 200 | 2 | 4.16 / 4.22 / 7.87 (7.74–28.42) | 4.10 / 4.22 / 9.22 (8.13–17.15) | 4.22 / 4.42 / 9.34 (4.48–12.67) | 4.22 / 4.42 / 9.22 (4.48–13.82) | 4.22 / 4.42 / 9.09 (7.04–13.82) | 4.22 / 4.42 / 4.61 (4.42–15.62) | 4.22 / 4.42 / 10.62 (8.45–14.34) | 4.22 / 4.42 / 8.58 (5.31–15.49) | 100% | 1 |
| 1000 | 200 | 4 | 4.16 / 4.22 / 7.10 (4.35–17.41) | 4.10 / 4.29 / 7.62 (4.29–21.25) | 4.29 / 4.42 / 8.70 (4.48–19.46) | 4.29 / 4.86 / 5.38 (5.06–20.74) | 4.42 / 4.86 / 8.96 (8.00–21.50) | 4.22 / 4.42 / 7.36 (6.59–18.18) | 4.42 / 4.48 / 9.22 (4.61–22.53) | 4.22 / 4.42 / 10.62 (6.21–21.25) | 99% | 0 |
| 1000 | 200 | 6 | 4.35 / 4.74 / 8.96 (4.99–16.00) | 4.42 / 4.74 / 6.21 (5.25–17.41) | 4.35 / 5.12 / 9.09 (5.95–14.72) | 4.42 / 5.12 / 6.78 (5.70–15.87) | 4.93 / 5.31 / 10.62 (7.62–13.31) | 4.42 / 5.12 / 9.34 (6.14–15.74) | 5.06 / 5.44 / 8.13 (6.53–17.66) | 4.42 / 5.18 / 10.11 (5.25–14.72) | 67% | 3 |
| 1000 | 200 | 8 | 4.42 / 4.74 / 10.62 (6.21–16.38) | 4.16 / 4.74 / 11.01 (7.55–17.66) | 4.35 / 5.12 / 12.03 (10.50–14.72) | 4.74 / 5.12 / 11.65 (9.60–14.21) | 4.99 / 5.38 / 11.78 (7.87–15.74) | 4.42 / 5.12 / 10.37 (8.32–24.06) | 5.38 / 5.82 / 13.31 (12.93–15.36) | 4.42 / 5.12 / 10.24 (6.98–16.26) | 49% | 5 |
| 1000 | 1000 | 2 | 5.63 / 5.70 / 12.54 (6.40–14.72) | 5.57 / 5.70 / 12.42 (6.59–16.90) | 5.70 / 5.89 / 12.29 (11.78–14.34) | 5.70 / 5.89 / 12.03 (9.47–17.41) | 5.70 / 5.89 / 13.06 (7.10–17.41) | 5.70 / 5.89 / 12.42 (9.47–14.59) | 5.70 / 5.89 / 11.65 (10.88–16.38) | 5.70 / 5.89 / 11.65 (6.02–20.99) | 100% | 0 |
| 1000 | 1000 | 4 | 5.63 / 5.76 / 10.11 (6.78–24.06) | 5.57 / 5.70 / 10.24 (9.73–26.11) | 5.76 / 6.27 / 10.11 (6.72–24.83) | 5.76 / 5.89 / 12.42 (6.66–19.97) | 5.89 / 6.14 / 12.03 (7.68–23.55) | 5.70 / 5.89 / 11.14 (9.98–20.48) | 5.89 / 6.08 / 12.03 (10.11–23.30) | 5.70 / 5.89 / 9.98 (7.10–21.25) | 100% | 0 |
| 1000 | 1000 | 6 | 5.70 / 6.27 / 11.65 (10.75–17.41) | 5.76 / 6.27 / 9.73 (8.19–14.85) | 6.08 / 6.72 / 11.14 (10.11–15.62) | 5.89 / 6.72 / 11.90 (8.00–14.72) | 6.40 / 6.91 / 11.39 (11.01–15.74) | 5.82 / 6.78 / 11.14 (7.36–14.72) | 7.68 / 8.19 / 16.00 (13.70–20.99) | 6.21 / 6.78 / 11.39 (9.47–15.87) | 67% | 10 |
| 1000 | 1000 | 8 | 5.70 / 6.21 / 13.18 (11.52–15.10) | 5.82 / 6.27 / 13.44 (12.03–29.44) | 5.82 / 6.78 / 13.31 (12.03–35.84) | 5.82 / 6.78 / 14.34 (13.31–32.51) | 6.40 / 7.04 / 14.34 (12.54–35.33) | 6.02 / 6.78 / 13.57 (12.93–34.30) | 10.62 / 11.52 / 23.04 (19.97–46.08) | 6.02 / 6.78 / 13.70 (11.90–112.64) | 50% | 16 |

Migrations seen: 75469 over 74739195 samples (one sample per measured operation).


### Suite a3t: k-1 threads signal one notify, lock per notify

load average before: { 4.75 4.89 3.65 }
load average after: { 5.84 5.28 4.04 }; 173 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 704k (697k–708k) | 704k (697k–712k) | 665k (658k–668k) | 657k (653k–660k) | 663k (647k–667k) | 660k (656k–662k) | 664k (656k–670k) | 659k (654k–662k) | 100% | 0 |
| 0 | 50 | 4 | 2.09M (2.07M–2.10M) | 2.08M (2.08M–2.11M) | 1.86M (1.84M–1.89M) | 1.86M (1.85M–1.89M) | 1.91M (1.88M–1.96M) | 1.91M (1.85M–1.95M) | 1.93M (1.57M–1.98M) | 1.90M (1.81M–1.95M) | 98% | 5 |
| 0 | 50 | 6 | 3.39M (3.36M–3.40M) | 3.38M (3.36M–3.40M) | 2.71M (2.64M–2.78M) | 2.68M (2.49M–2.75M) | 2.35M (2.31M–2.43M) | 2.48M (2.43M–2.50M) | 2.42M (2.41M–2.51M) | 2.36M (2.31M–2.67M) | 67% | 23 |
| 0 | 50 | 8 | 4.73M (4.67M–4.73M) | 4.73M (4.71M–4.75M) | 3.66M (3.31M–4.03M) | 2.32M (2.17M–3.28M) | 2.74M (2.63M–3.11M) | 2.51M (2.39M–2.59M) | 2.59M (2.46M–2.66M) | 2.64M (2.58M–2.86M) | 49% | 227 |
| 0 | 200 | 2 | 656k (644k–664k) | 660k (639k–664k) | 574k (557k–577k) | 564k (560k–568k) | 568k (567k–578k) | 562k (560k–568k) | 573k (562k–577k) | 567k (565k–571k) | 100% | 0 |
| 0 | 200 | 4 | 1.94M (1.94M–1.96M) | 1.94M (1.93M–1.94M) | 1.66M (1.65M–1.68M) | 1.63M (1.60M–1.66M) | 1.60M (1.58M–1.61M) | 1.66M (1.65M–1.67M) | 1.60M (1.59M–1.61M) | 1.66M (1.65M–1.67M) | 100% | 0 |
| 0 | 200 | 6 | 3.18M (3.16M–3.19M) | 3.18M (3.15M–3.19M) | 2.27M (2.03M–2.36M) | 2.10M (1.90M–2.17M) | 1.72M (1.62M–1.78M) | 1.97M (1.92M–1.99M) | 1.57M (1.52M–1.60M) | 1.81M (1.78M–2.00M) | 66% | 20 |
| 0 | 200 | 8 | 4.45M (4.39M–4.50M) | 4.45M (4.44M–4.48M) | 2.43M (2.04M–3.00M) | 2.39M (2.10M–3.06M) | 2.18M (1.66M–2.25M) | 2.06M (1.94M–2.10M) | 1.59M (1.51M–1.63M) | 2.01M (1.91M–2.02M) | 49% | 262 |
| 0 | 1000 | 2 | 524k (520k–526k) | 525k (522k–528k) | 400k (387k–401k) | 395k (394k–399k) | 401k (399k–402k) | 396k (394k–397k) | 400k (397k–401k) | 396k (394k–397k) | 100% | 1 |
| 0 | 1000 | 4 | 1.57M (1.56M–1.57M) | 1.56M (1.55M–1.57M) | 1.08M (1.07M–1.08M) | 1.12M (1.11M–1.12M) | 672k (669k–674k) | 911k (894k–915k) | 673k (670k–674k) | 908k (899k–924k) | 100% | 0 |
| 0 | 1000 | 6 | 1.88M (1.87M–1.89M) | 1.89M (1.88M–1.89M) | 1.20M (1.14M–1.33M) | 1.06M (1.02M–1.13M) | 978k (964k–1.01M) | 1.06M (982k–1.09M) | 657k (648k–673k) | 964k (963k–1.01M) | 66% | 31 |
| 0 | 1000 | 8 | 1.87M (1.78M–1.89M) | 1.87M (1.86M–1.88M) | 1.18M (1.08M–1.34M) | 1.15M (1.02M–1.23M) | 1.12M (1.02M–1.18M) | 1.06M (1.03M–1.14M) | 683k (663k–703k) | 1.04M (1.02M–1.09M) | 50% | 85 |
| 1000 | 50 | 2 | 700k (696k–711k) | 702k (693k–705k) | 667k (654k–669k) | 661k (644k–664k) | 667k (658k–670k) | 660k (649k–664k) | 668k (658k–669k) | 661k (653k–662k) | 100% | 0 |
| 1000 | 50 | 4 | 2.11M (2.08M–2.11M) | 957k (955k–957k) | 1.45M (1.43M–1.45M) | 1.44M (1.43M–1.45M) | 1.96M (1.93M–1.98M) | 1.94M (1.93M–1.95M) | 1.94M (1.94M–1.97M) | 1.95M (1.93M–1.97M) | 100% | 1486 |
| 1000 | 50 | 6 | 3.39M (3.35M–3.40M) | 939k (929k–944k) | 2.47M (2.23M–2.55M) | 2.42M (2.29M–2.51M) | 1.73M (1.73M–1.74M) | 1.73M (1.72M–1.73M) | 2.82M (2.73M–2.84M) | 2.72M (2.67M–2.89M) | 67% | 1463 |
| 1000 | 50 | 8 | 4.67M (4.61M–4.74M) | 931k (929k–939k) | 3.09M (2.44M–3.52M) | 2.91M (1.79M–3.24M) | 2.68M (2.26M–2.70M) | 2.42M (2.36M–2.53M) | 2.81M (2.72M–2.99M) | 2.79M (2.72M–3.09M) | 50% | 1565 |
| 1000 | 200 | 2 | 658k (646k–663k) | 658k (654k–663k) | 570k (560k–573k) | 565k (562k–571k) | 570k (565k–574k) | 566k (561k–573k) | 572k (567k–575k) | 564k (563k–568k) | 100% | 0 |
| 1000 | 200 | 4 | 1.95M (1.94M–1.97M) | 868k (866k–869k) | 1.35M (1.33M–1.37M) | 1.35M (1.34M–1.36M) | 1.62M (1.61M–1.63M) | 1.67M (1.66M–1.68M) | 1.64M (1.61M–1.65M) | 1.67M (1.65M–1.68M) | 100% | 1253 |
| 1000 | 200 | 6 | 3.17M (3.17M–3.20M) | 859k (858k–861k) | 2.22M (2.03M–2.25M) | 2.14M (1.99M–2.29M) | 1.59M (1.58M–1.63M) | 1.63M (1.62M–1.64M) | 1.68M (1.64M–1.72M) | 2.15M (2.08M–2.21M) | 66% | 1248 |
| 1000 | 200 | 8 | 4.42M (4.36M–4.47M) | 852k (842k–854k) | 2.60M (1.93M–3.21M) | 2.03M (1.92M–2.80M) | 1.95M (1.85M–2.22M) | 1.87M (1.82M–2.09M) | 1.66M (1.60M–1.75M) | 2.16M (2.06M–2.23M) | 49% | 1400 |
| 1000 | 1000 | 2 | 522k (520k–525k) | 528k (525k–528k) | 375k (374k–386k) | 393k (392k–397k) | 387k (373k–390k) | 395k (393k–397k) | 383k (377k–386k) | 395k (393k–395k) | 100% | 0 |
| 1000 | 1000 | 4 | 1.56M (1.55M–1.58M) | 654k (654k–656k) | 1.08M (1.06M–1.10M) | 1.03M (1.03M–1.04M) | 724k (723k–727k) | 1.06M (1.06M–1.07M) | 724k (715k–727k) | 1.06M (1.06M–1.06M) | 100% | 760 |
| 1000 | 1000 | 6 | 1.88M (1.88M–1.89M) | 649k (647k–650k) | 1.22M (1.16M–1.26M) | 1.10M (1.06M–1.31M) | 1.01M (956k–1.05M) | 993k (928k–1.06M) | 681k (670k–685k) | 1.06M (1.05M–1.09M) | 66% | 745 |
| 1000 | 1000 | 8 | 1.87M (1.86M–1.88M) | 646k (642k–647k) | 1.25M (1.19M–1.38M) | 1.15M (877k–1.28M) | 1.08M (1.04M–1.22M) | 1.09M (965k–1.11M) | 710k (673k–715k) | 1.13M (1.08M–1.16M) | 50% | 699 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.12 / 0.12 / 0.17 (0.17–0.21) | 0.17 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.46) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 100% | 0 |
| 0 | 50 | 4 | 0.08 / 0.12 / 0.25 (0.17–0.37) | 0.08 / 0.12 / 0.21 (0.17–0.37) | 0.21 / 0.37 / 0.54 (0.42–0.66) | 0.21 / 0.33 / 0.46 (0.37–0.62) | 0.21 / 0.37 / 0.54 (0.42–8.32) | 0.21 / 0.46 / 0.62 (0.42–6.66) | 0.21 / 0.33 / 0.54 (0.42–7.87) | 0.21 / 0.46 / 0.66 (0.37–0.87) | 98% | 5 |
| 0 | 50 | 6 | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.33 (0.29–0.33) | 0.33 / 0.58 / 0.70 (0.66–0.74) | 0.29 / 0.54 / 8.06 (0.70–9.60) | 0.42 / 0.95 / 1.15 (1.15–1.15) | 0.37 / 0.70 / 0.91 (0.87–0.91) | 0.70 / 1.15 / 1.36 (1.28–3.74) | 0.79 / 1.25 / 1.41 (1.04–1.95) | 67% | 23 |
| 0 | 50 | 8 | 0.17 / 0.33 / 0.37 (0.37–0.42) | 0.17 / 0.33 / 0.37 (0.37–0.37) | 0.33 / 0.62 / 48.64 (0.66–69.63) | 0.46 / 1.25 / 1.46 (0.79–2.27) | 0.42 / 1.07 / 1.41 (1.33–1.54) | 0.42 / 0.70 / 0.83 (0.83–1.20) | 1.36 / 2.21 / 6.53 (3.94–8.96) | 1.33 / 1.95 / 6.02 (3.33–7.87) | 49% | 227 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.42 / 0.50 / 0.54 (0.50–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.50) | 0.42 / 0.50 / 0.50 (0.50–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.50) | 0.42 / 0.50 / 0.50 (0.50–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.54) | 100% | 0 |
| 0 | 200 | 4 | 0.21 / 0.25 / 0.33 (0.25–0.33) | 0.21 / 0.25 / 0.37 (0.25–0.37) | 0.33 / 0.62 / 0.66 (0.66–0.66) | 0.33 / 0.58 / 0.62 (0.62–0.74) | 0.50 / 0.66 / 1.12 (0.70–1.74) | 0.46 / 0.58 / 0.74 (0.62–1.00) | 0.50 / 0.66 / 0.74 (0.74–1.41) | 0.46 / 0.62 / 1.12 (0.62–1.28) | 100% | 0 |
| 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.25 / 0.33 / 0.42 (0.42–0.46) | 0.46 / 0.70 / 14.08 (10.11–14.59) | 0.58 / 1.54 / 1.70 (1.33–1.95) | 0.58 / 1.66 / 1.90 (1.90–2.11) | 0.50 / 1.15 / 1.41 (1.36–1.41) | 1.82 / 2.56 / 6.46 (3.68–10.62) | 1.46 / 1.78 / 3.23 (2.14–12.16) | 66% | 20 |
| 0 | 200 | 8 | 0.25 / 0.42 / 0.50 (0.46–0.54) | 0.25 / 0.42 / 0.46 (0.46–0.50) | 0.58 / 1.04 / 5.57 (1.12–29.18) | 0.58 / 1.62 / 24.83 (6.66–32.77) | 0.54 / 1.36 / 2.03 (1.87–2.21) | 0.62 / 1.33 / 1.82 (1.46–4.86) | 2.98 / 4.06 / 10.88 (10.37–12.93) | 2.14 / 2.91 / 11.39 (9.98–15.74) | 49% | 262 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.70) | 0.58 / 0.62 / 0.62 (0.62–0.66) | 1.15 / 1.25 / 1.28 (1.28–10.88) | 1.20 / 1.20 / 1.28 (1.28–2.37) | 1.15 / 1.25 / 1.28 (1.25–1.28) | 1.20 / 1.20 / 1.28 (1.25–1.28) | 1.15 / 1.25 / 1.28 (1.28–1.28) | 1.20 / 1.20 / 1.28 (1.25–1.33) | 100% | 1 |
| 0 | 1000 | 4 | 0.58 / 0.62 / 0.66 (0.62–0.74) | 0.58 / 0.62 / 0.70 (0.66–0.79) | 0.70 / 1.74 / 1.74 (1.74–1.79) | 0.70 / 1.66 / 1.74 (1.70–1.74) | 3.20 / 3.49 / 8.06 (3.62–12.03) | 1.79 / 2.30 / 6.53 (2.40–7.81) | 3.20 / 3.49 / 7.42 (3.62–11.14) | 1.79 / 2.30 / 2.69 (2.37–7.87) | 100% | 0 |
| 0 | 1000 | 6 | 0.66 / 0.79 / 141.31 (139.26–143.36) | 0.66 / 0.79 / 141.31 (139.26–143.36) | 2.03 / 2.82 / 4.48 (3.36–8.19) | 2.40 / 3.39 / 6.21 (3.52–10.50) | 1.00 / 3.81 / 4.61 (4.48–4.67) | 0.87 / 2.56 / 3.07 (2.94–3.23) | 6.21 / 7.30 / 14.85 (11.26–15.74) | 3.90 / 4.48 / 9.47 (5.50–12.42) | 66% | 31 |
| 0 | 1000 | 8 | 0.66 / 0.79 / 339.97 (335.87–348.16) | 0.62 / 0.79 / 339.97 (339.97–344.06) | 2.14 / 2.85 / 8.13 (5.50–9.73) | 1.87 / 3.39 / 9.34 (5.31–10.50) | 2.27 / 3.04 / 7.74 (7.10–9.47) | 2.50 / 2.85 / 11.14 (9.09–16.38) | 8.83 / 10.24 / 19.71 (16.64–37.38) | 5.44 / 6.53 / 15.36 (14.21–18.69) | 50% | 85 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.17 / 0.21 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 100% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.21) | 0.08 / 0.12 / 843.78 (811.01–860.16) | 0.21 / 0.33 / 0.46 (0.37–0.58) | 0.21 / 0.33 / 0.37 (0.37–0.62) | 0.17 / 0.37 / 0.50 (0.46–0.74) | 0.21 / 0.37 / 0.42 (0.37–0.66) | 0.17 / 0.42 / 0.58 (0.42–0.83) | 0.21 / 0.33 / 0.42 (0.37–0.70) | 100% | 1486 |
| 1000 | 50 | 6 | 0.17 / 0.29 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 1851.39 (1802.24–1867.78) | 0.33 / 0.62 / 0.79 (0.66–36.86) | 0.33 / 0.91 / 1.04 (0.66–1.36) | 0.33 / 0.74 / 1.04 (0.95–1.04) | 0.37 / 0.66 / 0.91 (0.87–0.91) | 0.42 / 1.04 / 1.33 (1.25–1.36) | 0.54 / 1.00 / 1.20 (1.07–1.25) | 67% | 1463 |
| 1000 | 50 | 8 | 0.21 / 0.37 / 0.46 (0.37–0.46) | 0.12 / 0.25 / 3014.66 (2916.35–3342.34) | 0.29 / 0.62 / 40.45 (0.74–73.73) | 0.42 / 1.20 / 1.46 (1.12–1.66) | 0.42 / 0.83 / 1.20 (1.15–1.87) | 0.42 / 0.70 / 0.87 (0.83–2.00) | 1.12 / 2.08 / 6.78 (5.57–9.09) | 1.15 / 1.82 / 6.21 (4.42–10.62) | 50% | 1565 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.54) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.54) | 0.42 / 0.50 / 0.54 (0.54–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.54) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.17 / 0.25 / 1032.19 (999.42–1032.19) | 0.33 / 0.58 / 0.66 (0.66–0.74) | 0.33 / 0.54 / 0.62 (0.62–0.70) | 0.46 / 0.74 / 0.87 (0.79–1.12) | 0.42 / 0.66 / 0.79 (0.70–1.00) | 0.42 / 0.70 / 0.79 (0.74–1.07) | 0.42 / 0.66 / 0.83 (0.74–1.07) | 100% | 1253 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.21 / 0.33 / 2195.46 (2162.69–2228.22) | 0.46 / 1.04 / 10.50 (0.91–20.48) | 0.50 / 1.33 / 1.70 (1.33–4.10) | 0.50 / 1.41 / 1.82 (1.79–2.03) | 0.50 / 1.12 / 1.49 (1.46–1.54) | 1.58 / 2.37 / 3.39 (3.04–7.74) | 0.95 / 1.62 / 1.82 (1.70–6.46) | 66% | 1248 |
| 1000 | 200 | 8 | 0.25 / 0.42 / 0.54 (0.46–0.54) | 0.21 / 0.33 / 3538.94 (3407.87–3604.48) | 0.50 / 1.25 / 18.69 (1.36–25.34) | 0.70 / 1.74 / 29.95 (15.36–36.86) | 0.70 / 1.36 / 2.82 (1.79–11.52) | 0.79 / 1.33 / 4.86 (1.36–6.91) | 2.82 / 3.74 / 10.88 (10.37–11.52) | 1.90 / 2.53 / 8.83 (6.66–10.62) | 49% | 1400 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.70) | 0.58 / 0.58 / 0.62 (0.62–0.62) | 1.36 / 1.62 / 1.66 (1.66–1.66) | 1.20 / 1.20 / 1.28 (1.28–1.49) | 1.15 / 1.62 / 1.66 (1.66–1.66) | 1.20 / 1.25 / 1.28 (1.28–1.46) | 1.15 / 1.62 / 1.66 (1.54–1.70) | 1.20 / 1.20 / 1.33 (1.28–1.41) | 100% | 0 |
| 1000 | 1000 | 4 | 0.58 / 0.62 / 0.66 (0.62–0.79) | 0.54 / 0.62 / 1753.09 (1703.94–1818.62) | 0.66 / 1.70 / 1.82 (1.74–1.87) | 0.70 / 1.25 / 1.70 (1.66–1.70) | 2.78 / 2.94 / 7.36 (2.91–9.22) | 1.62 / 1.87 / 2.03 (1.95–5.25) | 2.78 / 2.91 / 7.81 (2.98–10.24) | 1.46 / 1.87 / 2.14 (1.90–5.25) | 100% | 760 |
| 1000 | 1000 | 6 | 0.62 / 0.79 / 143.36 (139.26–143.36) | 0.58 / 0.74 / 3768.32 (3637.25–3801.09) | 2.03 / 2.82 / 4.29 (2.85–8.70) | 2.00 / 3.36 / 5.57 (2.21–8.32) | 0.83 / 3.46 / 4.35 (4.16–4.61) | 0.95 / 2.56 / 3.23 (3.20–3.39) | 5.95 / 6.72 / 14.34 (11.26–15.62) | 3.20 / 4.29 / 9.86 (5.50–25.09) | 66% | 745 |
| 1000 | 1000 | 8 | 0.66 / 0.79 / 339.97 (331.78–344.06) | 0.58 / 0.74 / 6029.31 (5898.24–6225.92) | 1.66 / 2.82 / 5.57 (2.82–8.70) | 1.87 / 3.36 / 8.13 (4.16–8.83) | 2.37 / 2.98 / 7.74 (4.48–9.98) | 2.37 / 3.20 / 9.86 (6.53–10.62) | 8.45 / 9.47 / 20.74 (16.26–26.88) | 4.86 / 5.63 / 13.82 (13.18–16.64) | 50% | 699 |

Migrations seen: 71915 over 227723223 samples (one sample per measured operation).


### Suite ct: k-thread process beside single-thread ones, fine group locks

load average before: { 5.84 5.28 4.04 }
load average after: { 6.91 5.82 4.40 }; 115 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 100% | 0 |
| 4 | 200 | 2 | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 100% | 0 |
| 4 | 200 | 3 | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.21 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 100% | 0 |
| 4 | 1000 | 1 | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.04 / 1.07 (1.07–7.74) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.04 / 1.07 (1.04–1.07) | 100% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.07–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.04 (1.04–5.82) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.12 (1.07–6.66) | 99% | 7 |
| 4 | 1000 | 3 | 1.00 / 1.00 / 1.12 (1.04–1.20) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 1.07 (1.04–1.28) | 1.00 / 1.04 / 1.07 (1.07–6.08) | 1.00 / 1.00 / 1.07 (1.04–4.42) | 1.00 / 1.04 / 1.07 (1.04–4.93) | 1.00 / 1.00 / 1.04 (1.04–3.81) | 1.00 / 1.04 / 1.07 (1.04–4.99) | 99% | 0 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 1 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 0 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 0 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 5 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 7 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–5.63) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 9 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 49% | 0 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 49% | 5 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–2.72) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.12 (1.12–1.12) | 1.04 / 1.07 / 1.12 (1.12–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 22 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.12 (1.12–1.12) | 1.00 / 1.07 / 1.12 (1.12–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 49% | 8 |

The k-thread process: operations/s

| n | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 632k (628k–638k) | 629k (626k–645k) | 627k (627k–633k) | 629k (626k–637k) | 628k (627k–629k) | 628k (625k–635k) | 632k (626k–634k) | 629k (626k–638k) | 100% | 0 |
| 4 | 200 | 2 | 1.06M (1.05M–1.06M) | 1.09M (1.09M–1.10M) | 1.24M (1.24M–1.25M) | 1.25M (1.24M–1.25M) | 1.24M (1.24M–1.25M) | 1.25M (1.24M–1.25M) | 1.24M (1.24M–1.25M) | 1.24M (1.24M–1.25M) | 100% | 0 |
| 4 | 200 | 3 | 1.44M (1.44M–1.45M) | 1.50M (1.50M–1.51M) | 1.59M (1.59M–1.60M) | 1.60M (1.59M–1.60M) | 1.86M (1.86M–1.86M) | 1.87M (1.86M–1.87M) | 1.86M (1.85M–1.87M) | 1.86M (1.86M–1.88M) | 100% | 0 |
| 4 | 1000 | 1 | 419k (419k–422k) | 421k (419k–423k) | 420k (419k–424k) | 419k (417k–423k) | 420k (418k–424k) | 420k (419k–423k) | 421k (418k–422k) | 419k (419k–420k) | 100% | 0 |
| 4 | 1000 | 2 | 702k (670k–707k) | 741k (738k–744k) | 827k (826k–831k) | 834k (831k–835k) | 828k (822k–833k) | 832k (829k–837k) | 826k (825k–828k) | 832k (826k–833k) | 99% | 7 |
| 4 | 1000 | 3 | 958k (954k–961k) | 1.03M (1.03M–1.04M) | 1.08M (1.07M–1.09M) | 1.11M (1.09M–1.11M) | 1.20M (1.19M–1.21M) | 1.25M (1.24M–1.26M) | 1.20M (1.20M–1.21M) | 1.24M (1.24M–1.25M) | 99% | 0 |
| 8 | 200 | 1 | 637k (632k–638k) | 635k (633k–642k) | 637k (635k–638k) | 637k (634k–639k) | 637k (634k–640k) | 636k (633k–638k) | 640k (636k–640k) | 636k (634k–640k) | 49% | 1 |
| 8 | 200 | 2 | 1.03M (1.01M–1.06M) | 1.08M (1.05M–1.11M) | 1.25M (1.24M–1.25M) | 1.25M (1.23M–1.25M) | 1.25M (1.23M–1.26M) | 1.25M (1.24M–1.26M) | 1.25M (1.23M–1.25M) | 1.25M (1.24M–1.26M) | 49% | 0 |
| 8 | 200 | 3 | 1.42M (1.41M–1.43M) | 1.48M (1.47M–1.50M) | 1.55M (1.54M–1.56M) | 1.56M (1.55M–1.58M) | 1.84M (1.83M–1.85M) | 1.86M (1.85M–1.87M) | 1.84M (1.84M–1.85M) | 1.87M (1.85M–1.89M) | 49% | 0 |
| 8 | 200 | 5 | 2.07M (2.05M–2.11M) | 2.19M (2.14M–2.21M) | 2.22M (2.18M–2.27M) | 2.26M (2.23M–2.27M) | 2.49M (2.47M–2.54M) | 2.55M (2.54M–2.58M) | 3.03M (3.02M–3.03M) | 3.08M (3.07M–3.09M) | 49% | 5 |
| 8 | 200 | 6 | 2.38M (2.33M–2.39M) | 2.50M (2.46M–2.51M) | 2.56M (2.54M–2.57M) | 2.61M (2.61M–2.63M) | 2.87M (2.81M–2.88M) | 2.91M (2.88M–2.92M) | 3.61M (3.59M–3.61M) | 3.70M (3.70M–3.73M) | 49% | 7 |
| 8 | 1000 | 1 | 430k (428k–430k) | 430k (428k–432k) | 429k (413k–431k) | 430k (428k–431k) | 430k (428k–431k) | 431k (429k–431k) | 429k (427k–430k) | 430k (429k–431k) | 49% | 9 |
| 8 | 1000 | 2 | 705k (695k–718k) | 741k (737k–750k) | 827k (821k–832k) | 849k (838k–854k) | 825k (820k–835k) | 847k (841k–855k) | 828k (816k–833k) | 849k (843k–855k) | 49% | 0 |
| 8 | 1000 | 3 | 955k (947k–977k) | 1.03M (1.02M–1.05M) | 1.07M (1.06M–1.08M) | 1.09M (1.09M–1.10M) | 1.19M (1.19M–1.21M) | 1.26M (1.26M–1.28M) | 1.19M (1.18M–1.22M) | 1.26M (1.26M–1.27M) | 49% | 5 |
| 8 | 1000 | 5 | 1.38M (1.37M–1.38M) | 1.53M (1.51M–1.54M) | 1.58M (1.53M–1.61M) | 1.62M (1.59M–1.63M) | 1.65M (1.63M–1.66M) | 1.81M (1.81M–1.81M) | 1.82M (1.81M–1.83M) | 2.08M (2.08M–2.09M) | 49% | 22 |
| 8 | 1000 | 6 | 1.55M (1.53M–1.57M) | 1.75M (1.74M–1.77M) | 1.83M (1.79M–1.84M) | 1.87M (1.85M–1.88M) | 1.89M (1.86M–1.92M) | 2.06M (2.04M–2.07M) | 2.10M (2.10M–2.11M) | 2.49M (2.49M–2.50M) | 49% | 8 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 8.59; 2/2 fine: 0.07; 2/4 fair: 21.79; 2/4 fine: 0.11; 3 fair: 33.68; 3 fine: 0.10;

Migrations seen: 85192 over 314993174 samples (one sample per measured operation).


## Appendix F: full tables, third series, pass 2


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 13.5 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a1t: capability operations, fine group locks

load average before: { 6.91 5.82 4.40 }
load average after: { 5.17 5.26 4.72 }; 497 s

Throughput, operations/s, median (min–max)

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 1.38M (1.38M–1.39M) | 1.39M (1.39M–1.40M) | 1.40M (1.39M–1.41M) | 1.40M (1.39M–1.41M) | 1.39M (1.38M–1.40M) | 1.40M (1.38M–1.41M) | 1.39M (1.38M–1.41M) | 1.39M (1.39M–1.41M) | 1.40M (1.39M–1.41M) | 100% | 0 |
| 1 | 0 | 50 | 3 | 2.07M (2.05M–2.08M) | 2.07M (2.06M–2.08M) | 2.07M (2.06M–2.08M) | 2.08M (2.07M–2.09M) | 2.08M (2.07M–2.09M) | 2.08M (2.08M–2.10M) | 2.08M (2.07M–2.08M) | 2.09M (2.07M–2.09M) | 2.09M (2.07M–2.10M) | 100% | 0 |
| 1 | 0 | 50 | 4 | 2.74M (2.72M–2.75M) | 2.74M (2.73M–2.75M) | 2.75M (2.74M–2.75M) | 2.76M (2.75M–2.76M) | 2.77M (2.76M–2.77M) | 2.78M (2.77M–2.79M) | 2.77M (2.76M–2.79M) | 2.78M (2.78M–2.79M) | 2.78M (2.74M–2.79M) | 99% | 0 |
| 1 | 0 | 50 | 6 | 4.05M (4.04M–4.06M) | 4.05M (4.05M–4.09M) | 4.07M (4.06M–4.09M) | 4.10M (4.09M–4.12M) | 4.11M (4.08M–4.12M) | 4.14M (4.12M–4.15M) | 4.14M (4.12M–4.15M) | 4.18M (4.17M–4.20M) | 4.19M (4.16M–4.20M) | 66% | 11 |
| 1 | 0 | 50 | 8 | 5.40M (5.38M–5.41M) | 5.42M (5.35M–5.43M) | 5.42M (5.38M–5.43M) | 5.42M (5.38M–5.46M) | 5.44M (5.42M–5.45M) | 5.46M (5.34M–5.50M) | 5.53M (5.43M–5.57M) | 5.56M (5.54M–5.60M) | 5.57M (5.55M–5.59M) | 49% | 29 |
| 1 | 0 | 200 | 2 | 1.28M (1.26M–1.28M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 1.29M (1.27M–1.29M) | 1.29M (1.27M–1.29M) | 1.28M (1.28M–1.29M) | 1.28M (1.27M–1.29M) | 1.28M (1.27M–1.29M) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.89M (1.88M–1.91M) | 1.90M (1.89M–1.92M) | 1.90M (1.88M–1.91M) | 1.90M (1.88M–1.92M) | 1.91M (1.89M–1.93M) | 1.91M (1.89M–1.93M) | 1.91M (1.89M–1.93M) | 1.92M (1.89M–1.93M) | 1.92M (1.83M–1.93M) | 100% | 2 |
| 1 | 0 | 200 | 4 | 2.50M (2.41M–2.54M) | 2.53M (2.50M–2.55M) | 2.54M (2.50M–2.55M) | 2.54M (2.50M–2.55M) | 2.55M (2.52M–2.56M) | 2.56M (2.52M–2.57M) | 2.55M (2.51M–2.57M) | 2.55M (2.52M–2.57M) | 2.55M (2.51M–2.57M) | 99% | 12 |
| 1 | 0 | 200 | 6 | 3.78M (3.74M–3.80M) | 3.79M (3.77M–3.81M) | 3.82M (3.77M–3.82M) | 3.81M (3.78M–3.82M) | 3.84M (3.80M–3.85M) | 3.84M (3.80M–3.85M) | 3.89M (3.85M–3.90M) | 3.89M (3.86M–3.90M) | 3.90M (3.85M–3.90M) | 65% | 39 |
| 1 | 0 | 200 | 8 | 4.89M (4.86M–4.93M) | 4.96M (4.95M–4.97M) | 4.96M (4.95M–5.00M) | 4.97M (4.94M–4.99M) | 5.00M (4.99M–5.04M) | 5.05M (5.00M–5.05M) | 5.10M (5.05M–5.11M) | 5.11M (5.08M–5.13M) | 5.12M (5.08M–5.12M) | 50% | 16 |
| 1 | 0 | 1000 | 2 | 841k (832k–843k) | 845k (837k–846k) | 847k (840k–850k) | 850k (841k–853k) | 848k (841k–854k) | 849k (841k–854k) | 847k (841k–853k) | 849k (842k–853k) | 851k (842k–855k) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.25M (1.24M–1.26M) | 1.26M (1.25M–1.27M) | 1.26M (1.25M–1.27M) | 1.27M (1.26M–1.27M) | 1.27M (1.26M–1.27M) | 1.27M (1.27M–1.28M) | 1.27M (1.26M–1.27M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.66M (1.65M–1.66M) | 1.68M (1.67M–1.68M) | 1.67M (1.66M–1.68M) | 1.68M (1.67M–1.69M) | 1.68M (1.65M–1.69M) | 1.69M (1.67M–1.70M) | 1.67M (1.55M–1.68M) | 1.69M (1.69M–1.70M) | 1.69M (1.69M–1.70M) | 99% | 5 |
| 1 | 0 | 1000 | 6 | 2.47M (2.44M–2.47M) | 2.52M (2.48M–2.52M) | 2.51M (2.50M–2.51M) | 2.52M (2.52M–2.54M) | 2.51M (2.51M–2.51M) | 2.54M (2.54M–2.55M) | 2.52M (2.51M–2.52M) | 2.57M (2.56M–2.57M) | 2.56M (2.56M–2.57M) | 66% | 16 |
| 1 | 0 | 1000 | 8 | 3.27M (3.25M–3.27M) | 3.35M (3.32M–3.35M) | 3.33M (3.31M–3.34M) | 3.38M (3.37M–3.38M) | 3.34M (3.33M–3.34M) | 3.41M (3.38M–3.42M) | 3.34M (3.31M–3.35M) | 3.44M (3.41M–3.45M) | 3.45M (3.42M–3.45M) | 49% | 13 |
| 1 | 1000 | 50 | 2 | 1.37M (1.36M–1.37M) | 1.38M (1.36M–1.38M) | 1.40M (1.38M–1.40M) | 1.40M (1.39M–1.40M) | 1.39M (1.38M–1.41M) | 1.39M (1.38M–1.41M) | 1.39M (1.38M–1.40M) | 1.39M (1.38M–1.40M) | 1.39M (1.39M–1.41M) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.01M (2.00M–2.02M) | 2.02M (2.01M–2.03M) | 2.03M (2.02M–2.04M) | 2.05M (2.03M–2.05M) | 2.08M (2.07M–2.08M) | 2.08M (2.08M–2.10M) | 2.08M (2.06M–2.09M) | 2.08M (2.08M–2.09M) | 2.08M (2.07M–2.10M) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.65M (2.64M–2.67M) | 2.66M (2.66M–2.68M) | 2.69M (2.67M–2.72M) | 2.69M (2.69M–2.71M) | 2.76M (2.75M–2.78M) | 2.78M (2.76M–2.79M) | 2.76M (2.75M–2.78M) | 2.78M (2.77M–2.79M) | 2.78M (2.77M–2.79M) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.88M (3.87M–3.89M) | 3.93M (3.92M–3.95M) | 3.93M (3.92M–3.95M) | 3.95M (3.93M–3.96M) | 4.00M (3.98M–4.03M) | 4.03M (4.02M–4.03M) | 4.13M (4.12M–4.15M) | 4.18M (4.16M–4.20M) | 4.18M (4.16M–4.20M) | 66% | 10 |
| 1 | 1000 | 50 | 8 | 5.12M (5.09M–5.13M) | 5.13M (5.11M–5.15M) | 5.14M (5.13M–5.15M) | 5.14M (5.10M–5.15M) | 5.24M (5.21M–5.26M) | 5.26M (5.25M–5.28M) | 5.52M (5.51M–5.53M) | 5.56M (5.55M–5.57M) | 5.56M (5.55M–5.57M) | 49% | 7 |
| 1 | 1000 | 200 | 2 | 1.25M (1.24M–1.26M) | 1.25M (1.24M–1.26M) | 1.27M (1.26M–1.29M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.27M–1.29M) | 1.27M (1.27M–1.29M) | 1.27M (1.26M–1.29M) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 1.84M (1.83M–1.87M) | 1.86M (1.85M–1.88M) | 1.87M (1.86M–1.89M) | 1.87M (1.87M–1.90M) | 1.90M (1.90M–1.91M) | 1.90M (1.90M–1.92M) | 1.90M (1.90M–1.92M) | 1.90M (1.90M–1.93M) | 1.90M (1.90M–1.92M) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 2.44M (2.43M–2.46M) | 2.45M (2.44M–2.47M) | 2.47M (2.47M–2.50M) | 2.48M (2.47M–2.49M) | 2.54M (2.52M–2.55M) | 2.55M (2.52M–2.57M) | 2.54M (2.52M–2.57M) | 2.54M (2.52M–2.56M) | 2.55M (2.52M–2.56M) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 3.64M (3.57M–3.67M) | 3.67M (3.65M–3.68M) | 3.68M (3.65M–3.69M) | 3.68M (3.66M–3.71M) | 3.74M (3.71M–3.75M) | 3.75M (3.73M–3.76M) | 3.88M (3.85M–3.89M) | 3.89M (3.85M–3.90M) | 3.89M (3.85M–3.91M) | 65% | 12 |
| 1 | 1000 | 200 | 8 | 4.65M (4.65M–4.71M) | 4.76M (4.72M–4.76M) | 4.76M (4.72M–4.78M) | 4.75M (4.73M–4.78M) | 4.86M (4.83M–4.87M) | 4.86M (4.82M–4.87M) | 5.08M (5.03M–5.10M) | 5.10M (5.01M–5.15M) | 5.10M (5.03M–5.14M) | 49% | 11 |
| 1 | 1000 | 1000 | 2 | 831k (826k–837k) | 839k (831k–842k) | 848k (840k–848k) | 850k (842k–852k) | 847k (841k–851k) | 848k (841k–849k) | 848k (841k–851k) | 851k (842k–851k) | 849k (844k–850k) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 1.23M (1.21M–1.23M) | 1.24M (1.24M–1.25M) | 1.24M (1.24M–1.25M) | 1.25M (1.24M–1.25M) | 1.26M (1.25M–1.26M) | 1.26M (1.26M–1.27M) | 1.26M (1.25M–1.26M) | 1.26M (1.26M–1.27M) | 1.26M (1.26M–1.28M) | 100% | 5 |
| 1 | 1000 | 1000 | 4 | 1.62M (1.62M–1.63M) | 1.64M (1.63M–1.65M) | 1.66M (1.65M–1.67M) | 1.66M (1.65M–1.66M) | 1.67M (1.67M–1.68M) | 1.69M (1.68M–1.70M) | 1.67M (1.66M–1.68M) | 1.69M (1.68M–1.70M) | 1.69M (1.68M–1.70M) | 99% | 0 |
| 1 | 1000 | 1000 | 6 | 2.40M (2.38M–2.41M) | 2.45M (2.43M–2.46M) | 2.45M (2.45M–2.46M) | 2.46M (2.46M–2.47M) | 2.46M (2.45M–2.47M) | 2.50M (2.49M–2.50M) | 2.51M (2.45M–2.51M) | 2.56M (2.55M–2.56M) | 2.56M (2.55M–2.57M) | 66% | 32 |
| 1 | 1000 | 1000 | 8 | 3.15M (3.13M–3.15M) | 3.23M (3.22M–3.25M) | 3.25M (3.24M–3.26M) | 3.26M (3.26M–3.28M) | 3.28M (3.27M–3.29M) | 3.31M (3.23M–3.33M) | 3.33M (3.32M–3.33M) | 3.44M (3.43M–3.44M) | 3.44M (3.43M–3.45M) | 49% | 17 |
| 10 | 0 | 50 | 2 | 1.32M (1.31M–1.32M) | 1.34M (1.32M–1.34M) | 1.37M (1.35M–1.38M) | 1.38M (1.36M–1.39M) | 1.37M (1.35M–1.38M) | 1.38M (1.36M–1.39M) | 1.37M (1.36M–1.38M) | 1.38M (1.37M–1.39M) | 1.38M (1.37M–1.38M) | 100% | 0 |
| 10 | 0 | 50 | 3 | 1.92M (1.90M–1.92M) | 1.95M (1.95M–1.96M) | 1.96M (1.95M–1.97M) | 1.97M (1.96M–1.98M) | 2.04M (2.02M–2.06M) | 2.05M (2.03M–2.06M) | 2.04M (2.02M–2.05M) | 2.05M (2.03M–2.07M) | 2.05M (2.03M–2.06M) | 100% | 0 |
| 10 | 0 | 50 | 4 | 2.49M (2.46M–2.49M) | 2.55M (2.53M–2.55M) | 2.56M (2.53M–2.57M) | 2.58M (2.55M–2.59M) | 2.71M (2.68M–2.73M) | 2.74M (2.70M–2.76M) | 2.72M (2.68M–2.74M) | 2.74M (2.69M–2.76M) | 2.73M (2.71M–2.76M) | 100% | 0 |
| 10 | 0 | 50 | 6 | 3.34M (3.30M–3.36M) | 3.52M (3.50M–3.53M) | 3.45M (3.40M–3.50M) | 3.46M (2.91M–3.52M) | 3.60M (3.54M–3.69M) | 3.66M (3.59M–3.71M) | 4.00M (3.99M–4.05M) | 4.07M (4.02M–4.09M) | 4.06M (4.05M–4.10M) | 66% | 33 |
| 10 | 0 | 50 | 8 | 4.18M (4.08M–4.20M) | 4.41M (4.39M–4.42M) | 4.35M (4.33M–4.35M) | 4.38M (4.32M–4.44M) | 4.67M (4.65M–4.68M) | 4.72M (4.70M–4.73M) | 5.32M (5.29M–5.35M) | 5.38M (5.38M–5.46M) | 5.38M (5.35M–5.40M) | 49% | 6 |
| 10 | 0 | 200 | 2 | 1.19M (1.19M–1.21M) | 1.22M (1.21M–1.23M) | 1.25M (1.25M–1.28M) | 1.26M (1.25M–1.28M) | 1.26M (1.25M–1.28M) | 1.26M (1.25M–1.29M) | 1.25M (1.25M–1.28M) | 1.26M (1.25M–1.28M) | 1.26M (1.25M–1.28M) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.73M (1.72M–1.74M) | 1.77M (1.77M–1.79M) | 1.79M (1.78M–1.81M) | 1.80M (1.76M–1.81M) | 1.88M (1.86M–1.90M) | 1.89M (1.87M–1.90M) | 1.88M (1.86M–1.89M) | 1.90M (1.88M–1.90M) | 1.90M (1.87M–1.90M) | 100% | 0 |
| 10 | 0 | 200 | 4 | 2.24M (2.23M–2.25M) | 2.32M (2.31M–2.33M) | 2.35M (2.32M–2.36M) | 2.36M (2.34M–2.38M) | 2.50M (2.48M–2.51M) | 2.52M (2.49M–2.53M) | 2.50M (2.47M–2.51M) | 2.53M (2.50M–2.53M) | 2.52M (2.49M–2.53M) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.98M (2.92M–3.04M) | 3.23M (3.21M–3.25M) | 3.18M (3.11M–3.23M) | 3.20M (3.18M–3.24M) | 3.35M (3.25M–3.37M) | 3.41M (3.37M–3.42M) | 3.66M (3.62M–3.66M) | 3.77M (3.73M–3.78M) | 3.73M (3.71M–3.76M) | 66% | 27 |
| 10 | 0 | 200 | 8 | 3.72M (3.60M–3.79M) | 4.01M (3.95M–4.05M) | 4.02M (3.77M–4.02M) | 3.98M (3.93M–4.07M) | 4.26M (4.17M–4.30M) | 4.35M (4.27M–4.37M) | 4.68M (4.61M–4.71M) | 4.95M (4.91M–4.98M) | 4.93M (4.93M–4.96M) | 49% | 67 |
| 10 | 0 | 1000 | 2 | 759k (744k–763k) | 797k (794k–802k) | 829k (818k–839k) | 840k (828k–842k) | 827k (825k–835k) | 841k (832k–844k) | 830k (828k–836k) | 840k (828k–846k) | 840k (798k–845k) | 100% | 3 |
| 10 | 0 | 1000 | 3 | 1.07M (1.07M–1.08M) | 1.16M (1.14M–1.16M) | 1.16M (1.15M–1.16M) | 1.19M (1.18M–1.19M) | 1.21M (1.20M–1.21M) | 1.25M (1.24M–1.26M) | 1.21M (1.20M–1.21M) | 1.26M (1.25M–1.26M) | 1.25M (1.25M–1.26M) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 1.36M (1.35M–1.37M) | 1.50M (1.49M–1.51M) | 1.48M (1.47M–1.49M) | 1.56M (1.55M–1.56M) | 1.54M (1.53M–1.55M) | 1.67M (1.65M–1.67M) | 1.54M (1.52M–1.55M) | 1.66M (1.64M–1.67M) | 1.66M (1.65M–1.67M) | 99% | 0 |
| 10 | 0 | 1000 | 6 | 1.79M (1.78M–1.81M) | 2.07M (2.05M–2.10M) | 2.03M (1.99M–2.03M) | 2.13M (2.10M–2.16M) | 2.04M (2.01M–2.05M) | 2.26M (2.25M–2.29M) | 2.09M (2.08M–2.10M) | 2.48M (2.47M–2.49M) | 2.48M (2.47M–2.49M) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 2.24M (2.23M–2.25M) | 2.64M (2.59M–2.67M) | 2.52M (2.49M–2.56M) | 2.68M (2.65M–2.74M) | 2.57M (2.54M–2.59M) | 2.93M (2.92M–2.94M) | 2.57M (2.57M–2.58M) | 3.29M (3.28M–3.30M) | 3.30M (3.28M–3.31M) | 49% | 10 |
| 10 | 1000 | 50 | 2 | 1.16M (1.15M–1.17M) | 1.20M (1.19M–1.21M) | 1.37M (1.36M–1.39M) | 1.38M (1.36M–1.40M) | 1.38M (1.36M–1.39M) | 1.38M (1.35M–1.39M) | 1.38M (1.36M–1.40M) | 1.38M (1.37M–1.40M) | 1.38M (1.37M–1.39M) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 1.59M (1.58M–1.59M) | 1.65M (1.63M–1.65M) | 1.74M (1.73M–1.74M) | 1.74M (1.74M–1.75M) | 2.04M (2.03M–2.06M) | 2.05M (2.04M–2.06M) | 2.04M (2.03M–2.06M) | 2.06M (2.04M–2.07M) | 2.05M (2.04M–2.06M) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 1.99M (1.97M–2.00M) | 2.07M (2.06M–2.08M) | 2.18M (2.17M–2.20M) | 2.19M (2.18M–2.20M) | 2.72M (2.70M–2.74M) | 2.74M (2.72M–2.75M) | 2.73M (2.68M–2.74M) | 2.73M (2.71M–2.74M) | 2.74M (2.71M–2.74M) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 2.58M (2.54M–2.60M) | 2.71M (2.66M–2.73M) | 2.80M (2.73M–2.85M) | 2.83M (2.78M–2.85M) | 3.10M (3.07M–3.12M) | 3.11M (3.08M–3.17M) | 4.03M (4.00M–4.07M) | 4.08M (4.04M–4.11M) | 4.05M (4.02M–4.11M) | 66% | 29 |
| 10 | 1000 | 50 | 8 | 3.12M (3.03M–3.18M) | 3.27M (3.24M–3.32M) | 3.37M (3.36M–3.45M) | 3.45M (3.40M–3.47M) | 3.80M (3.61M–3.88M) | 3.85M (3.83M–3.87M) | 5.34M (5.32M–5.36M) | 5.40M (5.39M–5.47M) | 5.43M (5.40M–5.46M) | 49% | 70 |
| 10 | 1000 | 200 | 2 | 1.07M (1.07M–1.08M) | 1.11M (1.10M–1.11M) | 1.28M (1.25M–1.28M) | 1.27M (1.26M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 1.27M (1.25M–1.28M) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 1.46M (1.39M–1.46M) | 1.52M (1.49M–1.53M) | 1.60M (1.59M–1.62M) | 1.61M (1.49M–1.62M) | 1.88M (1.86M–1.90M) | 1.89M (1.88M–1.90M) | 1.89M (1.87M–1.90M) | 1.90M (1.88M–1.90M) | 1.89M (1.88M–1.90M) | 99% | 12 |
| 10 | 1000 | 200 | 4 | 1.83M (1.83M–1.83M) | 1.92M (1.91M–1.92M) | 2.03M (2.02M–2.04M) | 2.04M (2.03M–2.05M) | 2.49M (2.44M–2.51M) | 2.51M (2.47M–2.52M) | 2.49M (2.48M–2.50M) | 2.51M (2.51M–2.53M) | 2.51M (2.49M–2.51M) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 2.34M (2.33M–2.39M) | 2.54M (2.51M–2.55M) | 2.62M (2.55M–2.66M) | 2.63M (2.57M–2.67M) | 2.88M (2.81M–2.90M) | 2.91M (2.87M–2.94M) | 3.64M (3.60M–3.67M) | 3.75M (3.73M–3.78M) | 3.74M (3.70M–3.78M) | 65% | 47 |
| 10 | 1000 | 200 | 8 | 2.89M (2.79M–2.91M) | 3.07M (3.02M–3.10M) | 3.21M (3.13M–3.24M) | 3.24M (3.20M–3.26M) | 3.56M (3.54M–3.56M) | 3.60M (3.60M–3.62M) | 4.68M (4.67M–4.71M) | 4.97M (4.94M–4.99M) | 4.97M (4.92M–4.99M) | 49% | 39 |
| 10 | 1000 | 1000 | 2 | 707k (703k–711k) | 747k (742k–749k) | 833k (827k–840k) | 843k (835k–845k) | 832k (828k–838k) | 840k (834k–846k) | 831k (826k–837k) | 842k (834k–847k) | 842k (834k–846k) | 100% | 1 |
| 10 | 1000 | 1000 | 3 | 963k (961k–968k) | 1.04M (1.03M–1.04M) | 1.08M (1.08M–1.09M) | 1.10M (1.10M–1.11M) | 1.21M (1.20M–1.21M) | 1.25M (1.25M–1.26M) | 1.21M (1.20M–1.21M) | 1.25M (1.25M–1.26M) | 1.25M (1.24M–1.25M) | 99% | 0 |
| 10 | 1000 | 1000 | 4 | 1.20M (1.19M–1.20M) | 1.32M (1.30M–1.32M) | 1.38M (1.38M–1.39M) | 1.42M (1.41M–1.42M) | 1.54M (1.54M–1.55M) | 1.66M (1.65M–1.67M) | 1.54M (1.53M–1.55M) | 1.66M (1.65M–1.66M) | 1.66M (1.65M–1.67M) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 1.55M (1.54M–1.57M) | 1.73M (1.73M–1.76M) | 1.84M (1.81M–1.84M) | 1.88M (1.85M–1.88M) | 1.89M (1.88M–1.90M) | 2.06M (2.05M–2.06M) | 2.10M (2.09M–2.10M) | 2.47M (2.47M–2.49M) | 2.47M (2.47M–2.49M) | 66% | 55 |
| 10 | 1000 | 1000 | 8 | 1.85M (1.83M–1.87M) | 2.14M (2.12M–2.16M) | 2.22M (2.15M–2.26M) | 2.30M (2.28M–2.31M) | 2.37M (2.37M–2.38M) | 2.59M (2.57M–2.59M) | 2.57M (2.56M–2.58M) | 3.28M (3.27M–3.29M) | 3.30M (3.29M–3.30M) | 49% | 10 |

Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.33) | 0.08 / 0.12 / 0.21 (0.12–0.25) | 0.08 / 0.12 / 0.29 (0.29–0.33) | 0.08 / 0.12 / 0.17 (0.12–0.25) | 0.08 / 0.12 / 0.21 (0.17–0.25) | 99% | 0 |
| 1 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.37 (0.33–0.42) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.21 / 0.42 (0.37–0.46) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 66% | 11 |
| 1 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.37 (0.29–0.46) | 0.12 / 0.21 / 0.25 (0.25–0.25) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 0.12 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.25 / 0.50 (0.46–0.54) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 49% | 29 |
| 1 | 0 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 100% | 0 |
| 1 | 0 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.58 (0.54–0.58) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 0.21 / 0.25 / 0.66 (0.54–0.66) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 100% | 2 |
| 1 | 0 | 200 | 4 | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.58 (0.58–0.58) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 0.21 / 0.29 / 0.66 (0.62–0.70) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.29 / 0.66 (0.62–0.70) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.25 / 0.37 (0.33–0.37) | 99% | 12 |
| 1 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.33 / 0.74 (0.70–0.74) | 0.21 / 0.33 / 0.37 (0.37–0.37) | 0.21 / 0.37 / 0.83 (0.79–0.83) | 0.21 / 0.29 / 0.37 (0.37–0.37) | 0.21 / 0.50 / 0.87 (0.83–0.87) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 65% | 39 |
| 1 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.33 / 0.70 (0.70–0.74) | 0.21 / 0.33 / 0.37 (0.37–0.37) | 0.21 / 0.50 / 0.79 (0.79–0.83) | 0.21 / 0.29 / 0.42 (0.42–0.46) | 0.21 / 0.62 / 0.91 (0.87–0.91) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 0.21 / 0.29 / 0.50 (0.50–0.54) | 50% | 16 |
| 1 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–7.87) | 1.00 / 1.04 / 2.91 (2.82–3.04) | 1.00 / 1.04 / 1.33 (1.20–1.54) | 1.00 / 1.04 / 2.94 (2.82–2.94) | 1.00 / 1.04 / 1.28 (1.15–1.36) | 1.00 / 1.04 / 2.94 (2.91–2.98) | 1.00 / 1.04 / 1.36 (1.15–1.41) | 1.00 / 1.04 / 1.25 (1.15–1.46) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.04) | 1.00 / 1.04 / 2.94 (2.91–2.94) | 1.00 / 1.04 / 1.20 (1.12–1.49) | 1.00 / 1.57 / 3.04 (2.98–3.04) | 1.00 / 1.04 / 1.66 (1.57–1.70) | 1.00 / 1.62 / 3.04 (2.98–3.07) | 1.00 / 1.04 / 1.66 (1.62–1.74) | 1.00 / 1.04 / 1.62 (1.49–1.74) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.07 (1.04–1.07) | 1.00 / 1.87 / 2.98 (2.94–2.98) | 1.00 / 1.04 / 1.25 (1.20–1.62) | 1.00 / 2.24 / 3.04 (3.04–7.62) | 1.00 / 1.04 / 1.82 (1.79–6.27) | 1.00 / 2.24 / 3.07 (3.07–4.03) | 1.00 / 1.04 / 1.82 (1.78–1.82) | 1.00 / 1.04 / 1.79 (1.78–1.82) | 99% | 5 |
| 1 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–6.53) | 1.00 / 2.11 / 3.10 (3.07–3.14) | 1.00 / 1.12 / 1.33 (1.28–1.36) | 1.00 / 2.50 / 3.26 (3.23–3.26) | 1.00 / 1.12 / 1.87 (1.87–1.95) | 1.00 / 2.72 / 3.33 (3.23–3.36) | 1.00 / 1.12 / 2.03 (2.00–2.08) | 1.00 / 1.12 / 2.03 (2.00–2.03) | 66% | 16 |
| 1 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 2.21 / 3.14 (3.10–3.20) | 1.04 / 1.12 / 1.33 (1.28–1.41) | 1.00 / 2.50 / 3.49 (3.46–3.58) | 1.04 / 1.12 / 1.90 (1.87–1.95) | 1.00 / 2.91 / 3.94 (3.81–5.57) | 1.04 / 1.12 / 2.08 (2.08–5.31) | 1.04 / 1.12 / 2.08 (2.08–5.12) | 49% | 13 |
| 1 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.12 / 0.17 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.29 (0.29–0.29) | 0.08 / 0.12 / 0.12 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.37 (0.33–0.42) | 0.08 / 0.21 / 0.25 (0.25–0.25) | 0.08 / 0.21 / 0.46 (0.42–0.46) | 0.08 / 0.17 / 0.25 (0.21–0.25) | 0.08 / 0.21 / 0.50 (0.46–0.50) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 66% | 10 |
| 1 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.21 / 0.42 (0.37–0.46) | 0.12 / 0.21 / 0.25 (0.25–0.25) | 0.08 / 0.21 / 0.46 (0.46–0.50) | 0.12 / 0.17 / 0.25 (0.25–0.25) | 0.08 / 0.25 / 0.54 (0.50–0.54) | 0.08 / 0.17 / 0.25 (0.25–0.29) | 0.08 / 0.17 / 0.29 (0.25–0.29) | 49% | 7 |
| 1 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.54 (0.42–0.58) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.58 (0.50–0.58) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.54 (0.50–0.58) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.50 (0.50–0.54) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.21 / 0.25 / 0.66 (0.62–0.66) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.54 (0.54–0.58) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 0.21 / 0.29 / 0.70 (0.66–0.70) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.25 / 0.66 (0.66–0.66) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.25 / 0.33 (0.33–0.33) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.37 / 0.66 (0.66–0.70) | 0.21 / 0.33 / 0.37 (0.33–0.37) | 0.21 / 0.37 / 0.79 (0.78–0.83) | 0.21 / 0.29 / 0.37 (0.37–0.42) | 0.21 / 0.50 / 0.87 (0.83–0.87) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 0.21 / 0.29 / 0.46 (0.46–0.46) | 65% | 12 |
| 1 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.33 / 0.70 (0.66–0.74) | 0.21 / 0.33 / 0.37 (0.33–0.37) | 0.21 / 0.42 / 0.79 (0.79–0.79) | 0.21 / 0.33 / 0.42 (0.42–0.42) | 0.21 / 0.62 / 0.91 (0.87–0.91) | 0.21 / 0.29 / 0.50 (0.50–0.58) | 0.21 / 0.29 / 0.50 (0.50–0.58) | 49% | 11 |
| 1 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–3.36) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.04 / 2.94 (2.11–3.04) | 1.00 / 1.04 / 1.28 (1.15–1.87) | 1.00 / 1.04 / 2.91 (2.85–2.94) | 1.00 / 1.04 / 1.33 (1.20–1.82) | 1.00 / 1.04 / 2.91 (2.85–2.94) | 1.00 / 1.04 / 1.28 (1.12–1.41) | 1.00 / 1.04 / 1.33 (1.25–1.54) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 1.00 / 1.00 / 1.12 (1.04–8.00) | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.04 / 2.82 (2.72–5.57) | 1.00 / 1.04 / 1.15 (1.15–6.91) | 1.00 / 1.58 / 3.04 (2.98–5.70) | 1.00 / 1.04 / 1.66 (1.62–5.57) | 1.00 / 1.70 / 3.04 (2.98–5.12) | 1.00 / 1.04 / 1.70 (1.54–4.99) | 1.00 / 1.04 / 1.66 (1.66–1.95) | 100% | 5 |
| 1 | 1000 | 1000 | 4 | 1.00 / 1.00 / 1.07 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.04 / 2.72 (2.62–2.82) | 1.00 / 1.04 / 1.28 (1.20–1.33) | 1.00 / 2.21 / 3.04 (2.98–3.07) | 1.00 / 1.04 / 1.79 (1.70–1.82) | 1.00 / 2.21 / 3.07 (2.82–3.33) | 1.00 / 1.04 / 1.79 (1.74–1.95) | 1.00 / 1.04 / 1.79 (1.74–1.87) | 99% | 0 |
| 1 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.15 / 2.69 (2.62–2.72) | 1.00 / 1.07 / 1.33 (1.25–4.99) | 1.00 / 2.30 / 3.20 (3.14–7.04) | 1.00 / 1.12 / 1.90 (1.87–4.16) | 1.00 / 2.66 / 3.33 (3.23–3.87) | 1.00 / 1.12 / 2.03 (2.00–2.14) | 1.00 / 1.07 / 2.00 (2.00–2.03) | 66% | 32 |
| 1 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.12 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.15 / 2.66 (2.56–2.69) | 1.04 / 1.12 / 1.33 (1.28–1.36) | 1.00 / 2.24 / 3.20 (3.20–3.33) | 1.04 / 1.12 / 1.90 (1.87–3.04) | 1.00 / 2.85 / 4.00 (3.87–4.03) | 1.04 / 1.12 / 2.08 (2.08–2.11) | 1.04 / 1.12 / 2.11 (2.08–2.11) | 49% | 17 |
| 10 | 0 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.17 / 0.33 (0.29–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.17 / 0.33 (0.29–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.29 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.29 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.12 / 0.17 (0.12–0.21) | 0.08 / 0.25 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.21 (0.21–0.25) | 0.08 / 0.29 / 0.42 (0.37–0.42) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.29 / 0.42 (0.37–0.46) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 0 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.33 / 0.50 (0.46–0.54) | 0.08 / 0.21 / 0.33 (0.29–0.33) | 0.12 / 0.42 / 0.58 (0.54–0.62) | 0.08 / 0.21 / 0.37 (0.33–0.42) | 0.08 / 0.46 / 0.74 (0.70–0.87) | 0.08 / 0.25 / 0.46 (0.42–0.46) | 0.08 / 0.25 / 0.42 (0.42–0.46) | 66% | 33 |
| 10 | 0 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.37 / 0.50 (0.50–0.50) | 0.12 / 0.21 / 0.33 (0.33–0.33) | 0.12 / 0.42 / 0.54 (0.54–0.58) | 0.12 / 0.21 / 0.37 (0.37–0.37) | 0.12 / 0.50 / 0.79 (0.78–0.79) | 0.12 / 0.25 / 0.46 (0.42–0.46) | 0.12 / 0.29 / 0.50 (0.46–0.50) | 49% | 6 |
| 10 | 0 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.42 / 0.70 (0.66–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.42 / 0.70 (0.62–0.70) | 0.21 / 0.25 / 0.46 (0.42–0.46) | 0.21 / 0.42 / 0.70 (0.58–0.70) | 0.21 / 0.25 / 0.46 (0.37–0.46) | 0.21 / 0.25 / 0.46 (0.42–0.46) | 100% | 0 |
| 10 | 0 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.54 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.50) | 0.21 / 0.58 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.58 / 0.74 (0.74–0.83) | 0.21 / 0.25 / 0.50 (0.46–0.50) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 100% | 0 |
| 10 | 0 | 200 | 4 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.58 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.66 / 1.07 (0.95–1.74) | 0.21 / 0.29 / 0.50 (0.50–0.58) | 0.21 / 0.66 / 1.04 (0.87–1.25) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 0.21 / 0.33 / 0.50 (0.50–0.50) | 100% | 0 |
| 10 | 0 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 0.95 (0.91–1.04) | 0.21 / 0.33 / 0.50 (0.50–0.54) | 0.25 / 0.83 / 1.33 (1.33–1.54) | 0.21 / 0.33 / 0.62 (0.58–0.62) | 0.25 / 1.12 / 1.74 (1.70–1.78) | 0.21 / 0.46 / 0.66 (0.62–0.66) | 0.21 / 0.46 / 0.70 (0.70–0.70) | 66% | 27 |
| 10 | 0 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 0.91 (0.87–1.54) | 0.25 / 0.33 / 0.54 (0.50–0.58) | 0.25 / 0.83 / 1.33 (1.28–2.82) | 0.25 / 0.37 / 0.58 (0.58–0.66) | 0.25 / 1.41 / 2.24 (2.21–4.80) | 0.21 / 0.50 / 0.66 (0.66–0.70) | 0.21 / 0.54 / 0.74 (0.70–0.74) | 49% | 67 |
| 10 | 0 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–7.74) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 2.91 / 3.10 (3.07–8.00) | 1.00 / 1.07 / 2.00 (2.00–7.68) | 1.00 / 2.94 / 3.10 (3.10–6.66) | 1.00 / 1.07 / 1.95 (1.90–6.40) | 1.00 / 2.91 / 3.10 (3.10–6.40) | 1.00 / 1.07 / 1.90 (1.87–7.87) | 1.00 / 1.07 / 1.95 (1.79–7.04) | 100% | 3 |
| 10 | 0 | 1000 | 3 | 1.00 / 1.00 / 1.07 (1.04–4.42) | 1.00 / 1.00 / 1.04 (1.04–6.66) | 1.00 / 2.94 / 4.48 (4.35–5.06) | 1.00 / 1.07 / 2.03 (2.03–2.08) | 1.00 / 3.04 / 4.29 (3.74–4.42) | 1.00 / 1.46 / 2.08 (2.08–2.11) | 1.00 / 3.04 / 4.29 (4.29–5.06) | 1.00 / 1.41 / 2.08 (2.03–2.08) | 1.00 / 1.46 / 2.08 (2.08–5.89) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 1.00 / 1.00 / 1.04 (1.04–4.93) | 1.00 / 1.04 / 1.07 (1.04–5.25) | 1.00 / 3.10 / 4.67 (4.67–7.17) | 1.00 / 1.12 / 2.00 (2.00–2.03) | 1.00 / 3.52 / 5.06 (4.86–8.58) | 1.00 / 1.70 / 2.08 (2.08–2.11) | 1.00 / 3.52 / 4.93 (4.86–10.62) | 1.00 / 1.74 / 2.08 (2.08–8.00) | 1.00 / 1.70 / 2.08 (2.08–2.72) | 99% | 0 |
| 10 | 0 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.07) | 1.00 / 1.07 / 1.07 (1.07–6.78) | 1.04 / 4.00 / 6.27 (6.27–8.96) | 1.00 / 1.20 / 2.03 (2.00–6.78) | 1.04 / 4.48 / 6.46 (6.34–9.22) | 1.00 / 1.79 / 2.21 (2.21–2.24) | 1.04 / 4.86 / 6.53 (6.53–6.98) | 1.00 / 1.95 / 2.21 (2.21–2.24) | 1.00 / 2.00 / 2.30 (2.27–2.30) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.04 / 4.54 / 8.32 (8.13–8.70) | 1.04 / 1.25 / 2.03 (2.00–2.08) | 1.04 / 5.31 / 7.74 (7.55–8.19) | 1.04 / 1.78 / 2.21 (2.21–2.24) | 1.07 / 5.31 / 7.74 (7.68–7.74) | 1.04 / 2.03 / 2.24 (2.24–2.24) | 1.04 / 2.08 / 2.37 (2.30–2.37) | 49% | 10 |
| 10 | 1000 | 50 | 2 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.17 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.17 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 0.08 / 0.12 / 0.12 (0.12–0.12) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.21 / 0.33 (0.33–0.33) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.25 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.25 / 0.37 (0.33–0.37) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.12 / 0.12 (0.12–0.17) | 0.08 / 0.25 / 0.33 (0.33–0.37) | 0.08 / 0.12 / 0.21 (0.21–0.21) | 0.08 / 0.29 / 0.42 (0.37–0.46) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.29 / 0.42 (0.37–0.42) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.33 / 0.58 (0.37–0.66) | 0.08 / 0.21 / 0.29 (0.29–0.37) | 0.12 / 0.42 / 0.54 (0.54–0.58) | 0.08 / 0.21 / 0.37 (0.33–0.42) | 0.08 / 0.46 / 0.74 (0.66–0.83) | 0.08 / 0.25 / 0.42 (0.37–0.46) | 0.08 / 0.25 / 0.46 (0.37–0.46) | 66% | 29 |
| 10 | 1000 | 50 | 8 | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.17 (0.17–0.17) | 0.12 / 0.42 / 0.62 (0.58–0.62) | 0.12 / 0.21 / 0.33 (0.29–0.33) | 0.12 / 0.42 / 0.62 (0.54–0.70) | 0.12 / 0.21 / 0.37 (0.33–0.37) | 0.12 / 0.50 / 0.79 (0.79–0.79) | 0.12 / 0.29 / 0.46 (0.37–0.46) | 0.12 / 0.25 / 0.42 (0.37–0.46) | 49% | 70 |
| 10 | 1000 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.25) | 0.21 / 0.21 / 0.25 (0.25–0.29) | 0.21 / 0.42 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.42 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.46 / 0.70 (0.70–0.70) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.25 / 0.46 (0.42–0.46) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 0.21 / 0.21 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.50 / 0.70 (0.70–0.74) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.62 / 0.74 (0.74–0.78) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.62 / 0.74 (0.74–0.74) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 0.21 / 0.25 / 0.50 (0.50–0.50) | 99% | 12 |
| 10 | 1000 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.54 / 0.78 (0.74–0.79) | 0.21 / 0.25 / 0.46 (0.46–0.46) | 0.21 / 0.66 / 1.04 (1.00–4.35) | 0.21 / 0.33 / 0.50 (0.50–1.95) | 0.21 / 0.66 / 1.04 (1.04–1.12) | 0.21 / 0.29 / 0.50 (0.50–0.50) | 0.21 / 0.33 / 0.50 (0.50–0.62) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 1.04 (0.95–1.07) | 0.21 / 0.37 / 0.54 (0.50–0.58) | 0.25 / 0.83 / 1.33 (1.28–1.36) | 0.21 / 0.37 / 0.62 (0.62–0.66) | 0.25 / 1.15 / 1.74 (1.70–3.55) | 0.21 / 0.46 / 0.66 (0.62–0.70) | 0.21 / 0.46 / 0.70 (0.66–0.70) | 65% | 47 |
| 10 | 1000 | 200 | 8 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.25 / 0.70 / 1.07 (1.04–1.15) | 0.25 / 0.33 / 0.54 (0.50–0.54) | 0.25 / 0.87 / 1.41 (1.36–1.46) | 0.25 / 0.37 / 0.62 (0.58–0.62) | 0.25 / 1.46 / 2.24 (2.21–2.27) | 0.21 / 0.50 / 0.66 (0.62–0.66) | 0.21 / 0.50 / 0.70 (0.66–0.74) | 49% | 39 |
| 10 | 1000 | 1000 | 2 | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 1.00 / 1.04 (1.04–1.07) | 1.00 / 2.91 / 3.10 (3.07–3.10) | 1.00 / 1.07 / 1.95 (1.46–2.03) | 1.00 / 2.91 / 3.10 (3.07–3.10) | 1.00 / 1.12 / 1.95 (1.57–2.03) | 1.00 / 2.91 / 3.10 (3.07–3.14) | 1.00 / 1.07 / 1.87 (1.58–2.00) | 1.00 / 1.07 / 1.90 (1.57–2.00) | 100% | 1 |
| 10 | 1000 | 1000 | 3 | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 1.00 / 1.04 (1.04–1.12) | 1.00 / 2.72 / 3.39 (3.14–3.81) | 1.00 / 1.07 / 2.03 (2.00–4.16) | 1.00 / 3.07 / 4.74 (4.22–5.06) | 1.00 / 1.49 / 2.08 (2.03–2.11) | 1.00 / 3.07 / 4.35 (4.29–4.86) | 1.00 / 1.49 / 2.08 (2.03–4.48) | 1.00 / 1.49 / 2.11 (2.08–5.70) | 99% | 0 |
| 10 | 1000 | 1000 | 4 | 1.00 / 1.04 / 1.12 (1.04–1.20) | 1.00 / 1.00 / 1.12 (1.04–2.03) | 1.00 / 2.72 / 3.78 (3.68–4.10) | 1.00 / 1.12 / 2.00 (2.00–2.08) | 1.00 / 3.52 / 4.93 (4.93–6.02) | 1.00 / 1.70 / 2.08 (2.08–2.14) | 1.00 / 3.52 / 4.93 (4.86–6.08) | 1.00 / 1.74 / 2.11 (2.08–2.69) | 1.00 / 1.74 / 2.08 (2.08–2.11) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.04 / 3.10 / 4.74 (4.61–4.86) | 1.00 / 1.15 / 2.08 (2.03–2.11) | 1.04 / 4.06 / 6.14 (5.95–6.21) | 1.00 / 1.79 / 2.21 (2.21–2.24) | 1.04 / 4.86 / 6.53 (6.46–6.91) | 1.00 / 1.95 / 2.21 (2.21–2.24) | 1.00 / 2.00 / 2.27 (2.27–2.27) | 66% | 55 |
| 10 | 1000 | 1000 | 8 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.04 / 3.36 / 5.50 (5.25–5.70) | 1.04 / 1.20 / 2.03 (2.00–2.11) | 1.04 / 4.74 / 7.23 (6.98–7.55) | 1.04 / 1.74 / 2.21 (2.21–2.21) | 1.07 / 5.31 / 7.74 (7.68–7.94) | 1.04 / 2.03 / 2.24 (2.24–2.27) | 1.04 / 2.08 / 2.37 (2.37–3.39) | 49% | 10 |

Delete (removal) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.62 / 0.83 / 0.91 (0.83–10.37) | 0.54 / 0.78 / 3.62 (0.83–6.66) | 0.21 / 0.33 / 0.37 (0.33–3.23) | 0.25 / 0.37 / 0.42 (0.37–5.95) | 0.25 / 0.33 / 0.37 (0.33–0.46) | 0.25 / 0.37 / 0.37 (0.37–1.87) | 0.25 / 0.33 / 0.46 (0.33–0.54) | 0.21 / 0.33 / 0.42 (0.37–0.50) | 0.25 / 0.37 / 0.37 (0.37–1.12) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.79 / 1.07 / 3.14 (1.20–23.55) | 0.74 / 1.04 / 4.06 (1.07–23.81) | 0.70 / 1.04 / 7.04 (1.25–8.58) | 0.74 / 1.04 / 4.54 (1.25–12.93) | 0.25 / 0.33 / 0.46 (0.33–3.36) | 0.29 / 0.37 / 0.50 (0.37–4.74) | 0.25 / 0.33 / 0.50 (0.46–3.36) | 0.25 / 0.33 / 0.50 (0.42–4.54) | 0.29 / 0.37 / 0.50 (0.37–3.65) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.87 / 2.14 / 15.49 (4.03–32.51) | 0.83 / 1.87 / 10.75 (8.06–22.27) | 0.83 / 1.20 / 3.90 (1.57–5.70) | 0.83 / 1.12 / 1.79 (1.41–7.74) | 0.25 / 0.58 / 2.94 (0.42–4.48) | 0.29 / 0.58 / 3.46 (0.70–4.74) | 0.25 / 0.37 / 3.26 (0.74–4.61) | 0.25 / 0.46 / 3.33 (0.46–5.12) | 0.29 / 0.62 / 3.49 (1.15–5.70) | 99% | 0 |
| 1 | 0 | 50 | 6 | 1.62 / 2.72 / 9.22 (7.42–14.46) | 1.58 / 3.23 / 393.22 (3.52–999.42) | 1.82 / 3.39 / 4.42 (4.16–5.12) | 1.74 / 3.33 / 4.67 (3.74–6.91) | 1.41 / 2.37 / 3.26 (2.43–7.36) | 1.46 / 2.62 / 3.26 (3.07–5.95) | 0.46 / 0.83 / 1.04 (0.95–1.20) | 0.42 / 0.79 / 1.00 (0.95–1.07) | 0.50 / 0.70 / 1.04 (0.91–1.28) | 66% | 11 |
| 1 | 0 | 50 | 8 | 1.95 / 3.68 / 17.41 (9.86–557.06) | 1.90 / 3.33 / 45.06 (9.86–1966.08) | 2.08 / 3.62 / 5.31 (3.65–8.96) | 2.24 / 4.06 / 6.14 (5.25–7.49) | 1.82 / 3.10 / 4.03 (3.46–6.40) | 1.70 / 3.04 / 4.06 (3.58–14.21) | 0.46 / 0.79 / 1.04 (0.91–1.15) | 0.46 / 0.79 / 1.04 (0.83–1.15) | 0.46 / 0.70 / 1.00 (0.83–1.07) | 49% | 29 |
| 1 | 0 | 200 | 2 | 0.87 / 1.20 / 5.70 (3.81–9.73) | 0.74 / 1.12 / 5.38 (1.15–22.53) | 0.37 / 0.46 / 3.23 (0.46–8.45) | 0.37 / 0.50 / 0.70 (0.50–5.12) | 0.33 / 0.46 / 0.50 (0.46–4.29) | 0.37 / 0.50 / 0.58 (0.50–3.36) | 0.37 / 0.46 / 0.58 (0.46–3.20) | 0.33 / 0.46 / 0.70 (0.46–3.68) | 0.37 / 0.50 / 0.70 (0.50–3.52) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.54 / 5.76 (1.66–14.34) | 0.95 / 1.41 / 4.74 (2.43–24.06) | 0.91 / 1.36 / 6.78 (1.54–12.80) | 0.91 / 1.28 / 1.62 (1.46–5.12) | 0.37 / 0.54 / 3.26 (0.58–4.74) | 0.42 / 0.50 / 3.39 (0.66–3.68) | 0.42 / 0.50 / 3.26 (0.74–4.10) | 0.37 / 0.46 / 3.36 (0.74–3.46) | 0.42 / 0.50 / 3.07 (0.74–3.49) | 100% | 2 |
| 1 | 0 | 200 | 4 | 1.15 / 2.78 / 22.53 (4.42–909.31) | 1.04 / 2.53 / 19.20 (1.74–28.67) | 1.04 / 1.33 / 5.31 (1.70–10.24) | 1.04 / 1.66 / 4.29 (1.90–6.14) | 0.42 / 0.50 / 0.83 (0.74–6.21) | 0.42 / 0.50 / 3.49 (0.74–3.68) | 0.42 / 0.66 / 3.52 (3.39–5.44) | 0.37 / 0.50 / 3.46 (0.74–3.52) | 0.42 / 0.54 / 3.58 (3.46–7.87) | 99% | 12 |
| 1 | 0 | 200 | 6 | 2.14 / 3.39 / 235.52 (9.09–1015.81) | 1.87 / 3.33 / 1392.64 (8.58–1490.94) | 2.08 / 3.58 / 4.54 (4.10–7.87) | 2.08 / 3.55 / 4.42 (4.10–5.12) | 1.62 / 2.66 / 3.36 (3.14–3.81) | 1.70 / 2.72 / 3.46 (2.98–8.45) | 0.54 / 0.91 / 1.20 (1.04–1.33) | 0.50 / 0.87 / 1.20 (1.15–1.28) | 0.58 / 0.91 / 1.15 (1.07–1.20) | 65% | 39 |
| 1 | 0 | 200 | 8 | 2.27 / 4.93 / 18.43 (11.90–157.70) | 2.11 / 3.90 / 147.46 (9.98–614.40) | 2.27 / 3.81 / 5.12 (4.86–6.02) | 2.43 / 4.16 / 6.14 (5.06–6.66) | 1.79 / 2.94 / 4.10 (3.26–5.06) | 1.78 / 2.78 / 4.00 (3.78–4.61) | 0.58 / 1.00 / 1.41 (1.15–1.46) | 0.54 / 0.95 / 1.28 (1.15–1.33) | 0.58 / 0.91 / 1.28 (1.07–1.36) | 50% | 16 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.58 / 9.09 (3.62–29.95) | 1.79 / 2.82 / 8.58 (2.85–16.90) | 1.12 / 1.25 / 2.08 (1.28–4.22) | 1.15 / 1.28 / 4.54 (1.33–8.00) | 1.12 / 1.25 / 1.87 (1.46–5.95) | 1.15 / 1.28 / 1.57 (1.33–2.30) | 1.15 / 1.25 / 2.30 (1.36–9.98) | 1.12 / 1.33 / 5.31 (2.27–6.91) | 1.15 / 1.28 / 1.33 (1.28–6.40) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.91 / 4.61 / 7.36 (4.86–23.04) | 2.27 / 3.78 / 14.85 (8.96–28.16) | 1.90 / 3.04 / 4.35 (3.36–13.95) | 1.90 / 2.98 / 5.18 (3.10–8.96) | 1.20 / 1.28 / 2.24 (1.90–4.42) | 1.20 / 1.28 / 2.24 (1.28–4.16) | 1.20 / 1.28 / 3.33 (2.24–8.83) | 1.15 / 1.33 / 4.48 (2.30–9.09) | 1.20 / 1.28 / 2.40 (2.27–8.32) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.86 / 8.45 (5.12–25.60) | 2.56 / 4.06 / 12.03 (6.40–25.34) | 2.00 / 3.23 / 4.74 (3.14–8.32) | 2.00 / 3.07 / 6.08 (3.23–10.37) | 1.20 / 2.14 / 3.46 (2.37–6.91) | 1.20 / 1.66 / 7.36 (4.35–9.22) | 1.20 / 3.14 / 3.90 (3.33–9.09) | 1.15 / 1.95 / 4.29 (2.14–4.74) | 1.20 / 1.49 / 4.42 (4.29–4.74) | 99% | 5 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.34 / 18.69 (11.90–1556.48) | 3.46 / 5.70 / 12.93 (10.11–100.35) | 3.10 / 4.86 / 10.37 (10.11–12.29) | 3.14 / 4.86 / 6.08 (4.99–11.39) | 2.82 / 4.16 / 9.47 (5.12–10.24) | 2.66 / 4.06 / 5.31 (4.42–8.19) | 1.36 / 2.40 / 2.98 (2.85–3.07) | 1.33 / 2.30 / 2.91 (2.72–9.73) | 1.41 / 1.74 / 2.82 (2.62–8.19) | 66% | 16 |
| 1 | 0 | 1000 | 8 | 4.48 / 7.10 / 13.70 (13.44–44.54) | 3.87 / 6.53 / 21.25 (14.34–389.12) | 3.36 / 5.25 / 7.62 (6.40–155.65) | 3.36 / 5.44 / 7.42 (6.91–12.42) | 2.72 / 4.16 / 5.38 (5.25–7.10) | 2.82 / 4.48 / 5.70 (5.57–6.78) | 1.41 / 2.69 / 5.70 (2.91–17.15) | 1.41 / 2.53 / 3.36 (3.14–4.22) | 1.46 / 2.11 / 2.91 (2.78–5.57) | 49% | 13 |
| 1 | 1000 | 50 | 2 | 1.62 / 1.82 / 1.90 (1.87–9.34) | 1.54 / 1.74 / 4.99 (1.82–7.04) | 0.25 / 0.33 / 0.42 (0.33–0.50) | 0.25 / 0.37 / 0.37 (0.37–0.42) | 0.25 / 0.33 / 0.37 (0.33–0.50) | 0.25 / 0.37 / 0.37 (0.37–0.62) | 0.25 / 0.33 / 0.33 (0.33–0.42) | 0.21 / 0.33 / 0.37 (0.33–0.46) | 0.25 / 0.37 / 0.37 (0.37–0.66) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.78 / 3.07 / 7.74 (4.35–22.27) | 2.69 / 3.07 / 7.55 (4.42–9.22) | 1.70 / 1.95 / 4.61 (2.08–12.93) | 1.70 / 1.95 / 6.27 (2.24–9.60) | 0.25 / 0.33 / 0.50 (0.37–4.48) | 0.29 / 0.37 / 0.37 (0.37–3.39) | 0.25 / 0.33 / 3.33 (0.42–3.55) | 0.25 / 0.33 / 0.42 (0.33–0.54) | 0.29 / 0.37 / 0.37 (0.37–3.49) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 4.29 / 11.78 (7.17–27.90) | 2.82 / 3.10 / 10.11 (6.14–20.99) | 1.79 / 2.14 / 4.99 (2.43–5.25) | 1.79 / 2.30 / 5.12 (2.43–7.68) | 0.25 / 0.33 / 3.33 (1.62–3.49) | 0.29 / 0.46 / 3.23 (0.42–3.49) | 0.25 / 0.42 / 3.39 (3.26–3.52) | 0.25 / 0.33 / 0.70 (0.46–3.46) | 0.29 / 0.46 / 3.33 (0.50–3.49) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.68 / 4.54 / 12.29 (10.24–483.33) | 3.65 / 4.61 / 8.96 (5.12–14.59) | 3.62 / 4.74 / 5.95 (5.12–8.83) | 3.65 / 4.67 / 7.42 (5.31–9.34) | 2.50 / 3.46 / 4.48 (3.81–4.54) | 2.40 / 3.26 / 4.61 (3.58–5.38) | 0.46 / 0.83 / 1.07 (1.04–1.12) | 0.42 / 0.79 / 1.04 (1.00–1.04) | 0.46 / 0.70 / 1.00 (0.91–1.04) | 66% | 10 |
| 1 | 1000 | 50 | 8 | 4.00 / 5.31 / 13.18 (11.14–20.99) | 3.90 / 5.31 / 15.23 (12.42–1212.42) | 3.87 / 4.86 / 6.78 (5.95–16.13) | 4.22 / 5.44 / 6.91 (6.27–10.88) | 2.56 / 3.81 / 5.06 (4.67–5.50) | 2.53 / 3.90 / 5.12 (5.06–6.59) | 0.50 / 0.83 / 1.12 (1.04–1.20) | 0.50 / 0.79 / 1.15 (1.04–1.20) | 0.54 / 0.78 / 1.07 (0.95–1.15) | 49% | 7 |
| 1 | 1000 | 200 | 2 | 1.87 / 2.21 / 9.47 (2.37–18.18) | 1.74 / 2.08 / 6.85 (2.11–12.67) | 0.33 / 0.46 / 0.66 (0.50–0.70) | 0.37 / 0.50 / 0.62 (0.50–3.94) | 0.37 / 0.46 / 0.62 (0.50–0.74) | 0.37 / 0.50 / 0.50 (0.50–0.50) | 0.37 / 0.46 / 0.74 (0.46–13.18) | 0.37 / 0.46 / 0.62 (0.50–3.81) | 0.37 / 0.50 / 0.54 (0.50–0.74) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 3.52 / 10.11 (3.68–23.81) | 2.91 / 3.87 / 11.14 (5.89–15.49) | 1.90 / 2.24 / 4.93 (2.62–13.31) | 1.87 / 2.24 / 6.14 (2.27–8.58) | 0.37 / 0.54 / 3.46 (0.70–4.86) | 0.42 / 0.50 / 0.87 (0.58–4.61) | 0.37 / 0.46 / 0.74 (0.62–4.74) | 0.37 / 0.46 / 0.70 (0.66–3.52) | 0.42 / 0.50 / 0.74 (0.50–3.78) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 3.14 / 4.22 / 17.92 (10.37–30.21) | 3.04 / 4.29 / 12.80 (10.11–26.37) | 2.00 / 2.37 / 4.99 (2.69–14.34) | 2.00 / 2.62 / 5.25 (4.93–8.58) | 0.42 / 0.78 / 3.49 (0.74–5.18) | 0.42 / 0.50 / 3.55 (0.91–4.03) | 0.42 / 0.50 / 3.39 (0.70–3.62) | 0.37 / 0.50 / 3.58 (3.39–3.74) | 0.42 / 0.54 / 3.49 (1.74–3.74) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 4.00 / 5.06 / 12.54 (12.29–1638.40) | 3.90 / 4.99 / 18.18 (6.66–495.62) | 3.78 / 4.99 / 7.17 (5.82–11.65) | 3.78 / 4.93 / 10.50 (5.95–15.74) | 2.56 / 3.49 / 5.18 (4.00–8.58) | 2.69 / 3.49 / 4.93 (3.65–11.65) | 0.58 / 0.95 / 1.33 (1.07–1.36) | 0.50 / 0.91 / 1.33 (1.15–1.36) | 0.62 / 0.91 / 1.15 (1.04–1.36) | 65% | 12 |
| 1 | 1000 | 200 | 8 | 4.10 / 5.63 / 19.71 (15.87–315.39) | 4.06 / 5.95 / 17.41 (12.80–364.54) | 4.16 / 5.25 / 7.23 (6.46–12.67) | 4.16 / 5.25 / 9.60 (7.10–11.78) | 2.69 / 3.94 / 4.93 (4.61–6.78) | 2.82 / 4.35 / 7.10 (5.38–8.70) | 0.58 / 1.04 / 1.25 (1.15–1.36) | 0.58 / 0.95 / 1.28 (1.25–3.62) | 0.54 / 0.91 / 1.28 (1.12–7.74) | 49% | 11 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.54 / 7.68 (4.61–43.52) | 2.78 / 3.81 / 7.04 (5.44–17.92) | 1.15 / 1.25 / 2.37 (1.36–6.91) | 1.15 / 1.28 / 1.66 (1.33–10.62) | 1.15 / 1.28 / 2.40 (1.33–5.82) | 1.15 / 1.41 / 9.73 (1.33–11.90) | 1.15 / 1.25 / 2.14 (1.74–2.30) | 1.12 / 1.25 / 1.90 (1.62–4.22) | 1.15 / 1.28 / 1.70 (1.57–4.42) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 4.67 / 6.46 / 19.46 (6.98–33.28) | 4.00 / 6.08 / 14.34 (11.39–16.38) | 2.91 / 4.00 / 14.08 (4.99–39.42) | 2.91 / 4.00 / 16.90 (8.45–27.90) | 1.20 / 1.66 / 4.29 (2.30–6.85) | 1.20 / 1.28 / 4.29 (2.00–6.40) | 1.20 / 1.28 / 2.37 (2.08–5.50) | 1.15 / 1.46 / 4.61 (2.21–7.23) | 1.20 / 1.28 / 4.29 (2.00–15.10) | 100% | 5 |
| 1 | 1000 | 1000 | 4 | 5.12 / 8.06 / 20.22 (10.75–24.58) | 4.42 / 6.08 / 11.26 (7.94–19.20) | 2.98 / 4.00 / 6.66 (4.42–13.18) | 2.98 / 4.29 / 6.66 (6.27–11.65) | 1.20 / 2.14 / 3.36 (2.50–4.16) | 1.20 / 1.33 / 4.29 (2.11–4.42) | 1.20 / 1.79 / 3.39 (2.30–4.42) | 1.20 / 1.82 / 4.29 (2.27–4.42) | 1.20 / 1.49 / 4.35 (2.40–5.18) | 99% | 0 |
| 1 | 1000 | 1000 | 6 | 6.08 / 8.13 / 22.53 (14.72–1441.79) | 5.44 / 7.49 / 16.64 (13.18–450.56) | 4.86 / 6.53 / 7.55 (7.10–8.19) | 4.80 / 6.66 / 13.06 (6.91–22.02) | 3.78 / 4.99 / 8.13 (5.89–19.46) | 3.68 / 4.86 / 12.54 (5.95–14.59) | 1.36 / 2.43 / 2.91 (2.78–9.34) | 1.36 / 2.27 / 2.85 (2.62–6.27) | 1.36 / 1.70 / 2.78 (2.43–3.04) | 66% | 32 |
| 1 | 1000 | 1000 | 8 | 6.59 / 10.11 / 28.16 (17.92–129.02) | 5.82 / 8.32 / 18.69 (13.06–647.17) | 5.06 / 7.10 / 9.22 (8.00–17.15) | 5.25 / 6.91 / 8.70 (8.00–12.16) | 3.74 / 4.86 / 6.66 (5.76–9.47) | 3.81 / 5.06 / 7.30 (6.46–56.83) | 1.46 / 2.69 / 4.06 (2.91–5.31) | 1.36 / 2.53 / 3.20 (3.04–3.62) | 1.41 / 2.08 / 2.91 (2.72–3.04) | 49% | 17 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 0.95 (0.91–2.94) | 0.54 / 0.74 / 1.28 (0.87–1.95) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.54) | 0.21 / 0.33 / 0.50 (0.42–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.21 / 0.37 / 0.50 (0.46–0.50) | 0.21 / 0.37 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.79 / 1.12 / 4.67 (1.25–7.30) | 0.70 / 1.07 / 3.55 (1.74–7.55) | 0.74 / 1.07 / 1.70 (1.28–4.48) | 0.74 / 1.07 / 1.36 (1.28–3.58) | 0.25 / 0.46 / 0.54 (0.54–0.70) | 0.29 / 0.37 / 0.54 (0.54–0.58) | 0.25 / 0.46 / 0.54 (0.50–0.78) | 0.25 / 0.46 / 0.58 (0.54–0.62) | 0.29 / 0.37 / 0.58 (0.54–0.62) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.91 / 1.36 / 7.62 (1.62–14.21) | 0.83 / 1.33 / 12.29 (3.68–21.25) | 0.83 / 1.66 / 3.46 (1.70–3.87) | 0.83 / 1.41 / 2.50 (1.62–6.40) | 0.25 / 0.50 / 0.62 (0.58–0.70) | 0.29 / 0.46 / 0.62 (0.58–0.70) | 0.25 / 0.50 / 0.74 (0.58–0.91) | 0.25 / 0.50 / 0.70 (0.58–0.95) | 0.29 / 0.46 / 0.62 (0.58–0.87) | 100% | 0 |
| 10 | 0 | 50 | 6 | 1.54 / 3.10 / 10.24 (6.14–11.39) | 1.46 / 2.82 / 6.59 (6.21–9.34) | 1.82 / 3.78 / 5.12 (4.61–6.59) | 1.74 / 3.81 / 5.82 (4.61–311.30) | 1.54 / 3.10 / 4.16 (3.46–5.12) | 1.54 / 3.07 / 4.00 (3.58–5.12) | 0.46 / 1.00 / 1.28 (1.15–1.41) | 0.50 / 1.12 / 1.41 (1.25–1.46) | 0.46 / 0.91 / 1.20 (1.12–1.25) | 66% | 33 |
| 10 | 0 | 50 | 8 | 1.74 / 4.35 / 10.75 (9.09–15.74) | 1.74 / 4.03 / 9.22 (9.09–13.18) | 2.21 / 4.67 / 7.49 (6.08–8.83) | 2.14 / 4.61 / 6.46 (6.40–8.19) | 1.62 / 3.46 / 4.48 (4.29–6.59) | 1.62 / 3.49 / 4.61 (4.54–4.99) | 0.46 / 1.00 / 1.33 (1.25–1.33) | 0.50 / 1.15 / 1.57 (1.36–1.62) | 0.54 / 1.07 / 1.41 (1.33–1.49) | 49% | 6 |
| 10 | 0 | 200 | 2 | 0.87 / 1.20 / 1.28 (1.28–1.54) | 0.74 / 1.07 / 1.25 (1.20–4.10) | 0.33 / 0.54 / 0.70 (0.66–0.74) | 0.37 / 0.50 / 0.79 (0.70–0.79) | 0.37 / 0.54 / 0.70 (0.66–0.74) | 0.37 / 0.50 / 0.74 (0.74–0.78) | 0.33 / 0.50 / 0.74 (0.66–0.74) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.79) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.07 / 1.54 / 5.25 (3.26–9.60) | 0.95 / 1.46 / 3.49 (1.78–9.22) | 0.91 / 1.36 / 3.81 (1.79–6.21) | 0.91 / 1.36 / 3.20 (1.70–16.13) | 0.37 / 0.66 / 0.83 (0.79–0.91) | 0.42 / 0.62 / 0.83 (0.83–0.87) | 0.37 / 0.66 / 0.83 (0.79–0.83) | 0.37 / 0.70 / 0.83 (0.83–0.87) | 0.42 / 0.58 / 0.83 (0.83–0.87) | 100% | 0 |
| 10 | 0 | 200 | 4 | 1.15 / 1.82 / 5.95 (3.87–18.43) | 1.04 / 1.70 / 9.22 (4.06–12.29) | 1.04 / 1.62 / 2.14 (1.95–7.17) | 1.04 / 1.62 / 2.21 (2.03–7.55) | 0.42 / 0.74 / 1.04 (0.87–3.74) | 0.42 / 0.74 / 0.87 (0.87–5.25) | 0.42 / 0.74 / 0.95 (0.91–3.14) | 0.37 / 0.74 / 0.91 (0.87–1.07) | 0.42 / 0.74 / 0.87 (0.87–1.25) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.11 / 4.22 / 10.50 (6.91–11.14) | 1.66 / 3.26 / 8.32 (7.81–9.09) | 1.90 / 3.78 / 5.70 (4.61–7.68) | 2.03 / 4.16 / 5.57 (5.44–6.27) | 1.62 / 3.04 / 4.42 (3.81–15.36) | 1.62 / 2.98 / 4.42 (4.00–5.31) | 0.58 / 1.20 / 1.66 (1.57–1.70) | 0.58 / 1.25 / 1.70 (1.66–1.79) | 0.62 / 1.20 / 1.57 (1.54–1.70) | 66% | 27 |
| 10 | 0 | 200 | 8 | 2.08 / 4.74 / 10.88 (9.86–68.61) | 1.95 / 4.61 / 11.78 (9.73–56.83) | 2.27 / 4.86 / 8.83 (6.91–178.18) | 2.40 / 5.44 / 8.45 (7.17–124.93) | 1.79 / 3.74 / 5.25 (4.67–33.79) | 1.79 / 3.78 / 5.44 (4.80–29.70) | 0.62 / 1.46 / 1.90 (1.87–8.70) | 0.58 / 1.33 / 1.74 (1.66–8.06) | 0.62 / 1.28 / 1.79 (1.70–2.69) | 49% | 67 |
| 10 | 0 | 1000 | 2 | 2.43 / 3.55 / 8.58 (3.65–21.25) | 1.79 / 2.78 / 5.70 (2.94–9.73) | 1.15 / 1.46 / 2.03 (1.66–8.45) | 1.15 / 1.62 / 2.30 (2.30–6.98) | 1.15 / 1.54 / 2.03 (1.90–10.37) | 1.15 / 1.70 / 2.37 (2.27–8.96) | 1.15 / 1.49 / 2.14 (1.79–6.78) | 1.15 / 1.95 / 2.27 (2.24–6.66) | 1.15 / 1.66 / 2.27 (2.11–14.08) | 100% | 3 |
| 10 | 0 | 1000 | 3 | 2.72 / 4.61 / 8.70 (7.94–34.30) | 2.30 / 3.78 / 9.34 (6.53–32.26) | 1.90 / 3.62 / 7.49 (4.10–19.46) | 1.90 / 3.74 / 5.82 (5.38–9.60) | 1.20 / 2.30 / 2.50 (2.50–2.56) | 1.20 / 2.11 / 2.43 (2.40–5.70) | 1.20 / 2.30 / 2.53 (2.50–4.54) | 1.15 / 2.24 / 2.56 (2.50–2.72) | 1.20 / 2.08 / 2.43 (2.40–7.62) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 3.26 / 5.18 / 8.70 (7.74–29.44) | 2.62 / 4.54 / 11.90 (5.95–29.70) | 2.00 / 3.74 / 5.25 (4.16–14.46) | 2.03 / 3.94 / 5.38 (4.99–9.22) | 1.20 / 2.43 / 3.52 (3.52–7.74) | 1.20 / 2.27 / 3.04 (2.43–4.22) | 1.20 / 2.43 / 3.52 (3.49–10.75) | 1.20 / 2.40 / 3.39 (3.10–9.47) | 1.20 / 2.27 / 3.04 (2.50–6.53) | 99% | 0 |
| 10 | 0 | 1000 | 6 | 4.48 / 8.13 / 14.59 (11.78–22.53) | 3.49 / 7.36 / 13.44 (11.52–243.71) | 3.20 / 6.27 / 9.98 (7.74–17.41) | 3.49 / 7.49 / 11.65 (10.50–15.23) | 2.62 / 5.44 / 7.36 (6.91–13.95) | 2.94 / 6.53 / 8.96 (8.58–9.22) | 1.41 / 3.81 / 4.99 (4.35–5.25) | 1.41 / 3.49 / 4.48 (4.48–5.25) | 1.46 / 2.91 / 4.06 (4.03–4.16) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 4.86 / 9.09 / 16.90 (14.34–18.94) | 4.03 / 8.83 / 15.23 (14.21–102.40) | 3.90 / 8.83 / 13.57 (13.31–16.64) | 4.16 / 9.73 / 14.72 (14.34–15.87) | 2.91 / 6.40 / 9.34 (8.83–11.26) | 3.04 / 8.13 / 12.29 (11.39–12.54) | 1.46 / 4.03 / 5.31 (5.31–5.44) | 1.46 / 4.16 / 5.95 (5.50–6.53) | 1.49 / 3.20 / 4.54 (4.35–4.80) | 49% | 10 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 4.16 (1.90–8.45) | 1.57 / 1.79 / 4.29 (1.90–4.99) | 0.25 / 0.37 / 0.50 (0.46–0.54) | 0.25 / 0.37 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.21 / 0.33 / 0.46 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.10 / 7.74 (3.23–13.57) | 2.66 / 3.14 / 7.42 (5.70–12.80) | 1.70 / 2.11 / 3.20 (2.37–5.70) | 1.70 / 2.11 / 5.38 (2.40–7.74) | 0.25 / 0.46 / 0.54 (0.54–0.62) | 0.29 / 0.37 / 0.58 (0.54–0.66) | 0.25 / 0.46 / 0.54 (0.54–0.54) | 0.25 / 0.46 / 0.54 (0.54–0.66) | 0.29 / 0.42 / 0.58 (0.54–0.62) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.39 / 12.03 (6.46–14.98) | 2.78 / 3.33 / 10.88 (6.78–25.09) | 1.79 / 2.53 / 2.91 (2.85–5.12) | 1.79 / 2.56 / 3.74 (2.82–5.44) | 0.25 / 0.54 / 0.78 (0.58–0.87) | 0.29 / 0.46 / 0.58 (0.58–0.58) | 0.25 / 0.50 / 0.74 (0.58–0.91) | 0.25 / 0.54 / 0.74 (0.58–1.00) | 0.29 / 0.46 / 0.58 (0.58–0.78) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 3.52 / 5.76 / 11.90 (10.37–13.18) | 3.52 / 5.31 / 13.95 (10.75–16.13) | 2.94 / 4.80 / 5.95 (5.25–8.19) | 2.94 / 4.99 / 8.00 (6.27–9.47) | 2.24 / 3.68 / 4.74 (4.48–7.17) | 2.27 / 4.06 / 5.38 (3.81–5.70) | 0.42 / 0.95 / 1.25 (1.12–1.46) | 0.42 / 1.04 / 1.33 (1.12–1.41) | 0.50 / 1.00 / 1.25 (0.95–1.28) | 66% | 29 |
| 10 | 1000 | 50 | 8 | 3.68 / 7.62 / 16.38 (12.42–165.89) | 3.68 / 6.78 / 13.31 (12.16–194.56) | 3.78 / 6.27 / 8.58 (8.45–9.86) | 3.65 / 5.76 / 8.96 (8.83–16.64) | 2.40 / 4.48 / 5.95 (5.06–23.81) | 2.37 / 4.42 / 5.89 (5.25–7.23) | 0.46 / 1.00 / 1.28 (1.28–1.33) | 0.54 / 1.20 / 1.54 (1.28–1.70) | 0.46 / 0.87 / 1.12 (1.07–1.33) | 49% | 70 |
| 10 | 1000 | 200 | 2 | 1.87 / 2.21 / 2.37 (2.30–4.54) | 1.74 / 2.08 / 6.66 (2.14–11.65) | 0.37 / 0.50 / 0.70 (0.70–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.79) | 0.37 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.79 (0.74–0.83) | 0.37 / 0.54 / 0.74 (0.70–0.74) | 0.33 / 0.58 / 0.74 (0.74–0.74) | 0.37 / 0.50 / 0.74 (0.74–0.79) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.52 / 9.22 (8.19–106.50) | 2.85 / 3.55 / 12.54 (6.53–23.81) | 1.87 / 2.30 / 2.72 (2.62–15.49) | 1.90 / 2.37 / 7.62 (3.46–290.82) | 0.37 / 0.66 / 0.83 (0.79–0.91) | 0.42 / 0.66 / 0.87 (0.83–0.87) | 0.37 / 0.66 / 0.83 (0.79–0.91) | 0.37 / 0.70 / 0.83 (0.79–0.87) | 0.42 / 0.62 / 0.83 (0.83–0.87) | 99% | 12 |
| 10 | 1000 | 200 | 4 | 3.10 / 4.10 / 17.41 (4.80–19.97) | 2.98 / 3.74 / 9.34 (6.53–26.11) | 2.00 / 2.72 / 3.20 (3.10–7.68) | 2.00 / 2.82 / 5.57 (3.07–8.70) | 0.42 / 0.74 / 1.07 (0.87–6.40) | 0.42 / 0.74 / 0.91 (0.87–5.63) | 0.42 / 0.74 / 1.15 (0.95–2.24) | 0.37 / 0.74 / 1.12 (0.95–1.36) | 0.42 / 0.74 / 1.00 (0.87–5.06) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 3.81 / 6.53 / 14.08 (11.39–52.74) | 3.65 / 5.57 / 10.75 (9.09–19.97) | 3.26 / 5.31 / 7.23 (6.78–8.96) | 3.33 / 5.44 / 7.74 (6.66–9.34) | 2.43 / 3.94 / 6.02 (4.80–6.91) | 2.40 / 4.03 / 5.95 (4.74–9.34) | 0.58 / 1.25 / 1.66 (1.57–6.72) | 0.54 / 1.25 / 1.82 (1.62–6.91) | 0.58 / 1.12 / 1.58 (1.36–7.04) | 65% | 47 |
| 10 | 1000 | 200 | 8 | 4.06 / 7.49 / 15.49 (13.44–18.18) | 3.94 / 7.10 / 16.38 (13.06–20.74) | 3.87 / 6.21 / 10.37 (8.06–10.37) | 3.81 / 6.08 / 8.70 (7.81–15.10) | 2.56 / 4.61 / 7.17 (5.50–9.60) | 2.56 / 4.67 / 6.08 (5.76–9.86) | 0.62 / 1.46 / 1.87 (1.87–2.03) | 0.58 / 1.28 / 1.78 (1.66–1.95) | 0.58 / 1.25 / 1.66 (1.54–1.70) | 49% | 39 |
| 10 | 1000 | 1000 | 2 | 3.46 / 4.54 / 12.54 (4.61–15.87) | 2.78 / 3.81 / 8.96 (4.00–9.73) | 1.15 / 1.46 / 1.78 (1.54–2.66) | 1.15 / 1.58 / 2.30 (2.14–2.40) | 1.15 / 1.46 / 1.87 (1.54–2.21) | 1.15 / 1.62 / 2.30 (2.21–2.37) | 1.15 / 1.46 / 1.87 (1.49–6.91) | 1.15 / 1.79 / 2.27 (2.14–2.27) | 1.15 / 1.62 / 2.27 (2.03–4.86) | 100% | 1 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.66 / 13.31 (11.26–23.04) | 3.90 / 5.82 / 9.34 (8.45–31.49) | 2.91 / 4.10 / 7.87 (6.21–17.41) | 2.91 / 4.10 / 7.81 (5.63–13.82) | 1.20 / 2.30 / 2.62 (2.50–7.36) | 1.20 / 2.11 / 2.53 (2.40–5.12) | 1.20 / 2.30 / 2.56 (2.53–4.29) | 1.15 / 2.27 / 2.69 (2.43–7.87) | 1.20 / 2.11 / 3.23 (2.56–8.45) | 99% | 0 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.87 / 19.71 (14.85–29.70) | 4.35 / 6.21 / 10.24 (6.91–32.00) | 2.98 / 4.16 / 9.34 (6.40–13.82) | 2.98 / 4.67 / 6.53 (5.31–12.93) | 1.20 / 2.43 / 3.55 (3.49–4.16) | 1.20 / 2.27 / 3.33 (2.66–4.67) | 1.20 / 2.40 / 3.49 (3.39–6.40) | 1.20 / 2.37 / 3.49 (2.98–7.49) | 1.20 / 2.27 / 2.82 (2.50–7.81) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 6.14 / 10.11 / 14.21 (13.82–21.50) | 5.57 / 9.47 / 19.71 (16.38–33.28) | 4.48 / 7.62 / 10.75 (9.34–12.42) | 4.54 / 7.55 / 9.86 (9.09–13.70) | 3.46 / 5.82 / 8.45 (7.62–11.26) | 3.52 / 6.53 / 9.47 (8.58–12.80) | 1.41 / 3.81 / 4.99 (4.22–5.31) | 1.41 / 3.52 / 4.86 (4.61–5.38) | 1.41 / 2.82 / 4.06 (3.74–4.48) | 66% | 55 |
| 10 | 1000 | 1000 | 8 | 6.85 / 12.16 / 21.50 (18.94–33.79) | 5.95 / 11.52 / 20.99 (18.69–23.55) | 5.44 / 9.86 / 13.70 (13.06–14.85) | 5.31 / 10.11 / 14.59 (14.21–14.98) | 3.58 / 6.66 / 9.73 (9.22–10.75) | 3.68 / 7.87 / 11.14 (11.01–12.16) | 1.46 / 4.06 / 5.44 (5.38–5.63) | 1.46 / 4.16 / 5.70 (5.50–8.32) | 1.49 / 3.14 / 4.74 (4.35–6.66) | 49% | 10 |

Derive (addition) latency, µs

| change % | ipi ns | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 slot read | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0 | 50 | 2 | 0.62 / 0.83 / 4.61 (0.87–17.15) | 0.25 / 0.74 / 3.55 (0.79–3.74) | 0.21 / 0.33 / 0.42 (0.33–1.12) | 0.25 / 0.37 / 0.42 (0.37–3.23) | 0.21 / 0.33 / 0.37 (0.33–0.42) | 0.25 / 0.37 / 0.37 (0.37–3.23) | 0.21 / 0.33 / 0.42 (0.33–0.50) | 0.21 / 0.33 / 0.37 (0.33–0.54) | 0.25 / 0.37 / 0.42 (0.37–3.26) | 100% | 0 |
| 1 | 0 | 50 | 3 | 0.79 / 1.12 / 5.12 (4.03–9.98) | 0.54 / 0.79 / 4.61 (0.87–9.60) | 0.37 / 0.87 / 3.58 (0.87–6.21) | 0.37 / 0.83 / 0.95 (0.91–8.19) | 0.25 / 0.33 / 0.50 (0.42–3.74) | 0.29 / 0.37 / 0.42 (0.37–3.62) | 0.25 / 0.33 / 3.23 (0.46–3.26) | 0.25 / 0.33 / 0.54 (0.46–3.52) | 0.29 / 0.37 / 0.46 (0.37–5.57) | 100% | 0 |
| 1 | 0 | 50 | 4 | 0.87 / 1.66 / 18.43 (4.48–32.77) | 0.58 / 1.28 / 5.12 (1.36–9.09) | 0.42 / 1.07 / 4.03 (1.36–8.96) | 0.50 / 1.04 / 3.87 (1.20–6.08) | 0.25 / 0.54 / 3.46 (0.42–4.74) | 0.29 / 0.54 / 3.49 (0.42–6.08) | 0.25 / 0.42 / 0.79 (0.54–4.74) | 0.25 / 0.46 / 3.46 (0.46–3.94) | 0.29 / 0.66 / 3.49 (3.14–8.70) | 99% | 0 |
| 1 | 0 | 50 | 6 | 1.62 / 2.85 / 9.86 (6.98–286.72) | 1.00 / 1.95 / 3.26 (2.40–26.62) | 1.15 / 2.37 / 3.04 (2.69–4.35) | 1.12 / 2.30 / 2.91 (2.78–6.46) | 0.62 / 2.08 / 2.91 (2.08–3.10) | 0.62 / 2.21 / 2.78 (2.24–3.49) | 0.46 / 0.83 / 1.12 (0.95–1.15) | 0.42 / 0.74 / 1.07 (1.04–1.33) | 0.50 / 0.70 / 0.95 (0.87–1.00) | 66% | 11 |
| 1 | 0 | 50 | 8 | 1.95 / 3.58 / 15.10 (9.34–17.41) | 1.15 / 2.40 / 4.93 (2.85–6.53) | 1.36 / 2.37 / 3.14 (2.72–4.00) | 1.41 / 2.56 / 3.62 (3.10–4.16) | 1.04 / 2.53 / 3.46 (2.85–3.62) | 0.95 / 2.43 / 3.55 (3.26–11.65) | 0.46 / 0.79 / 1.07 (0.91–1.15) | 0.46 / 0.78 / 1.07 (0.79–1.15) | 0.46 / 0.70 / 0.95 (0.87–1.04) | 49% | 29 |
| 1 | 0 | 200 | 2 | 0.87 / 1.20 / 6.66 (1.28–12.54) | 0.37 / 1.04 / 3.39 (1.07–12.42) | 0.33 / 0.46 / 0.70 (0.58–3.65) | 0.37 / 0.50 / 0.74 (0.54–3.46) | 0.33 / 0.46 / 0.74 (0.50–3.33) | 0.37 / 0.50 / 0.54 (0.50–0.70) | 0.37 / 0.46 / 0.62 (0.46–3.20) | 0.33 / 0.42 / 0.66 (0.46–3.46) | 0.37 / 0.50 / 0.58 (0.54–3.10) | 100% | 0 |
| 1 | 0 | 200 | 3 | 1.07 / 1.49 / 4.22 (1.62–16.64) | 0.70 / 1.07 / 3.39 (1.20–6.02) | 0.50 / 1.12 / 3.78 (1.20–4.61) | 0.46 / 1.07 / 1.25 (1.15–3.90) | 0.37 / 0.46 / 3.33 (0.74–3.46) | 0.42 / 0.50 / 0.83 (0.70–3.49) | 0.37 / 0.46 / 3.33 (0.70–3.46) | 0.37 / 0.46 / 0.70 (0.70–3.49) | 0.42 / 0.50 / 3.52 (0.54–9.09) | 100% | 2 |
| 1 | 0 | 200 | 4 | 1.15 / 3.07 / 17.66 (5.82–1310.72) | 0.78 / 1.15 / 4.22 (1.33–14.59) | 0.74 / 1.25 / 4.22 (1.36–5.12) | 0.78 / 1.41 / 4.16 (1.41–9.09) | 0.42 / 0.46 / 3.33 (0.66–5.50) | 0.42 / 0.50 / 3.62 (0.70–3.78) | 0.42 / 0.66 / 3.52 (3.39–4.10) | 0.37 / 0.62 / 3.55 (3.49–6.27) | 0.42 / 0.54 / 3.62 (3.46–4.80) | 99% | 12 |
| 1 | 0 | 200 | 6 | 2.11 / 3.49 / 21.50 (9.98–491.52) | 1.20 / 2.21 / 6.72 (2.85–8.58) | 1.46 / 2.43 / 3.46 (2.98–6.34) | 1.41 / 2.53 / 3.26 (2.94–4.54) | 0.79 / 2.27 / 2.94 (2.56–3.07) | 0.83 / 2.37 / 3.07 (2.56–3.10) | 0.54 / 0.91 / 1.28 (1.20–4.61) | 0.50 / 0.87 / 1.15 (1.12–1.36) | 0.62 / 0.91 / 1.20 (1.12–1.87) | 65% | 39 |
| 1 | 0 | 200 | 8 | 2.27 / 4.99 / 23.30 (18.69–1392.64) | 1.36 / 2.62 / 4.06 (3.33–348.16) | 1.54 / 2.62 / 3.55 (3.39–5.12) | 1.62 / 2.72 / 3.68 (3.62–11.26) | 1.15 / 2.50 / 3.46 (2.85–3.68) | 1.07 / 2.40 / 3.04 (2.85–4.00) | 0.62 / 0.95 / 1.33 (1.20–1.54) | 0.54 / 0.91 / 1.25 (1.07–1.36) | 0.58 / 0.91 / 1.20 (1.04–1.33) | 50% | 16 |
| 1 | 0 | 1000 | 2 | 2.50 / 3.55 / 12.16 (3.65–16.00) | 1.20 / 2.72 / 6.40 (2.82–16.13) | 1.12 / 1.25 / 2.00 (1.36–4.16) | 1.15 / 1.33 / 2.43 (1.28–12.80) | 1.15 / 1.25 / 1.62 (1.36–13.57) | 1.15 / 1.28 / 2.24 (1.66–2.37) | 1.12 / 1.25 / 2.27 (1.28–5.95) | 1.12 / 1.25 / 2.27 (1.25–23.55) | 1.15 / 1.28 / 2.27 (1.28–4.54) | 100% | 0 |
| 1 | 0 | 1000 | 3 | 2.91 / 4.54 / 10.37 (5.18–13.18) | 1.70 / 2.82 / 5.95 (2.94–15.74) | 1.28 / 2.40 / 2.94 (2.82–6.21) | 1.28 / 2.53 / 2.98 (2.85–5.63) | 1.20 / 1.28 / 2.11 (1.90–4.42) | 1.20 / 1.28 / 2.30 (1.79–4.86) | 1.20 / 1.46 / 4.35 (2.40–6.78) | 1.15 / 1.36 / 2.27 (2.21–5.06) | 1.20 / 1.28 / 2.30 (1.74–4.35) | 100% | 0 |
| 1 | 0 | 1000 | 4 | 3.33 / 4.80 / 13.82 (4.99–32.26) | 1.78 / 2.91 / 4.80 (4.03–8.58) | 1.82 / 2.98 / 4.99 (2.98–6.91) | 1.78 / 2.98 / 5.44 (4.99–8.70) | 1.20 / 1.87 / 3.46 (3.20–7.55) | 1.20 / 1.57 / 4.35 (1.74–6.91) | 1.20 / 2.30 / 4.35 (4.16–4.42) | 1.15 / 1.87 / 4.29 (2.14–4.35) | 1.20 / 2.03 / 4.42 (4.35–4.80) | 99% | 5 |
| 1 | 0 | 1000 | 6 | 4.29 / 6.27 / 12.67 (9.98–925.70) | 2.37 / 4.16 / 11.01 (5.63–524.29) | 2.43 / 3.87 / 7.36 (4.61–7.74) | 2.37 / 3.94 / 5.12 (4.54–8.83) | 1.70 / 3.52 / 4.16 (4.16–6.78) | 1.58 / 3.36 / 3.87 (3.81–9.47) | 1.36 / 2.50 / 2.91 (2.82–5.82) | 1.33 / 2.37 / 2.85 (2.78–3.04) | 1.41 / 1.87 / 2.85 (2.69–7.87) | 66% | 16 |
| 1 | 0 | 1000 | 8 | 4.48 / 6.91 / 17.66 (14.46–327.68) | 2.56 / 4.74 / 10.62 (9.34–1425.41) | 2.53 / 4.03 / 5.31 (4.61–19.97) | 2.50 / 4.22 / 5.31 (4.86–10.62) | 2.21 / 3.62 / 4.61 (4.16–7.36) | 2.11 / 3.74 / 4.80 (4.48–6.34) | 1.41 / 2.62 / 3.26 (2.98–6.66) | 1.41 / 2.50 / 3.36 (2.82–5.89) | 1.46 / 1.87 / 2.82 (2.78–3.49) | 49% | 13 |
| 1 | 1000 | 50 | 2 | 1.62 / 1.87 / 2.50 (1.87–8.00) | 0.25 / 1.74 / 1.90 (1.79–3.68) | 0.25 / 0.33 / 0.37 (0.33–0.46) | 0.25 / 0.37 / 0.37 (0.37–0.46) | 0.21 / 0.33 / 0.37 (0.37–0.42) | 0.25 / 0.37 / 0.37 (0.37–0.46) | 0.21 / 0.33 / 0.33 (0.33–0.33) | 0.21 / 0.33 / 0.42 (0.33–0.50) | 0.25 / 0.37 / 0.37 (0.37–0.46) | 100% | 0 |
| 1 | 1000 | 50 | 3 | 2.78 / 3.07 / 8.32 (6.91–12.54) | 1.54 / 1.82 / 4.99 (2.00–12.93) | 0.37 / 1.82 / 1.87 (1.87–4.86) | 0.33 / 1.82 / 1.90 (1.87–1.90) | 0.25 / 0.33 / 0.42 (0.42–3.65) | 0.29 / 0.37 / 0.46 (0.42–3.46) | 0.25 / 0.33 / 3.20 (0.37–3.49) | 0.25 / 0.33 / 0.46 (0.37–3.49) | 0.29 / 0.37 / 0.42 (0.37–3.33) | 100% | 0 |
| 1 | 1000 | 50 | 4 | 2.85 / 4.06 / 13.95 (3.26–29.95) | 1.57 / 1.90 / 10.11 (2.03–19.97) | 0.42 / 2.03 / 3.46 (2.40–5.25) | 0.46 / 2.27 / 4.80 (2.30–5.18) | 0.25 / 0.37 / 3.36 (0.42–3.46) | 0.29 / 0.46 / 3.39 (0.58–3.62) | 0.25 / 0.42 / 3.33 (3.26–3.52) | 0.25 / 0.33 / 3.36 (0.42–3.74) | 0.29 / 0.50 / 3.39 (3.33–3.78) | 100% | 0 |
| 1 | 1000 | 50 | 6 | 3.68 / 4.61 / 14.21 (5.44–15.87) | 1.95 / 3.14 / 4.06 (3.81–5.44) | 2.14 / 3.23 / 4.42 (4.16–6.21) | 2.14 / 3.20 / 4.29 (4.00–6.85) | 0.70 / 3.23 / 4.42 (3.90–6.66) | 0.66 / 3.20 / 4.03 (3.26–4.61) | 0.46 / 0.79 / 1.04 (1.04–1.15) | 0.42 / 0.74 / 1.04 (0.91–1.07) | 0.46 / 0.70 / 0.87 (0.83–1.12) | 66% | 10 |
| 1 | 1000 | 50 | 8 | 4.00 / 5.25 / 13.06 (12.67–14.46) | 2.21 / 3.52 / 8.00 (4.48–11.39) | 2.27 / 3.39 / 4.42 (3.74–4.61) | 2.56 / 3.81 / 4.99 (4.16–5.31) | 0.95 / 3.55 / 4.48 (4.42–4.61) | 0.91 / 3.62 / 4.61 (4.48–9.09) | 0.50 / 0.87 / 1.15 (1.04–1.15) | 0.50 / 0.79 / 1.07 (1.04–1.28) | 0.54 / 0.74 / 1.07 (1.00–1.12) | 49% | 7 |
| 1 | 1000 | 200 | 2 | 1.87 / 2.21 / 5.12 (2.24–16.00) | 0.37 / 2.03 / 7.74 (2.08–12.54) | 0.37 / 0.46 / 0.50 (0.46–0.74) | 0.37 / 0.50 / 0.50 (0.50–14.08) | 0.33 / 0.46 / 0.58 (0.50–0.70) | 0.37 / 0.50 / 0.50 (0.50–0.58) | 0.33 / 0.46 / 2.91 (0.50–12.29) | 0.33 / 0.46 / 0.54 (0.46–6.08) | 0.37 / 0.50 / 0.70 (0.50–8.83) | 100% | 0 |
| 1 | 1000 | 200 | 3 | 3.04 / 3.55 / 12.93 (3.68–20.48) | 1.70 / 2.24 / 6.78 (2.21–24.58) | 0.50 / 2.08 / 3.81 (2.21–9.86) | 0.50 / 2.08 / 2.21 (2.14–4.74) | 0.37 / 0.54 / 3.36 (0.66–3.52) | 0.42 / 0.50 / 0.83 (0.58–5.12) | 0.37 / 0.46 / 0.66 (0.50–5.76) | 0.37 / 0.46 / 0.66 (0.54–2.14) | 0.42 / 0.50 / 0.74 (0.62–4.42) | 100% | 0 |
| 1 | 1000 | 200 | 4 | 3.14 / 4.54 / 14.98 (7.62–39.42) | 1.74 / 2.37 / 7.68 (5.57–21.50) | 0.54 / 2.30 / 5.06 (4.16–8.70) | 0.87 / 2.50 / 5.44 (3.26–5.76) | 0.42 / 0.78 / 3.46 (0.74–6.40) | 0.42 / 0.50 / 3.62 (0.79–4.22) | 0.42 / 0.50 / 3.33 (0.70–3.52) | 0.37 / 0.54 / 3.52 (0.70–6.53) | 0.42 / 0.54 / 3.49 (0.70–4.00) | 100% | 0 |
| 1 | 1000 | 200 | 6 | 4.00 / 5.12 / 13.82 (9.86–26.62) | 2.21 / 3.52 / 6.59 (3.87–20.74) | 2.37 / 3.49 / 4.35 (4.22–9.09) | 2.30 / 3.46 / 4.48 (4.16–5.57) | 0.74 / 3.26 / 4.03 (3.74–4.42) | 0.83 / 3.39 / 4.16 (3.52–5.50) | 0.58 / 0.91 / 1.25 (1.07–1.33) | 0.54 / 0.87 / 1.15 (1.15–1.20) | 0.62 / 0.91 / 1.20 (1.00–1.33) | 65% | 12 |
| 1 | 1000 | 200 | 8 | 4.10 / 5.63 / 15.36 (14.34–786.43) | 2.37 / 3.94 / 6.85 (4.74–10.75) | 2.56 / 3.65 / 4.74 (4.42–14.21) | 2.53 / 3.65 / 5.25 (4.54–5.57) | 1.25 / 3.55 / 4.48 (4.16–4.80) | 1.28 / 3.74 / 4.80 (4.67–6.02) | 0.58 / 1.04 / 1.33 (1.15–1.36) | 0.58 / 0.95 / 1.33 (1.25–10.62) | 0.54 / 0.91 / 1.25 (1.20–2.43) | 49% | 11 |
| 1 | 1000 | 1000 | 2 | 3.49 / 4.54 / 8.96 (4.54–37.89) | 2.56 / 3.74 / 4.00 (3.81–9.60) | 1.12 / 1.25 / 3.36 (1.36–11.65) | 1.15 / 1.28 / 1.82 (1.54–5.95) | 1.15 / 1.28 / 2.24 (1.54–10.88) | 1.15 / 1.28 / 4.61 (1.28–14.72) | 1.12 / 1.25 / 1.70 (1.57–15.62) | 1.12 / 1.28 / 2.72 (1.90–5.31) | 1.15 / 1.28 / 1.62 (1.46–4.29) | 100% | 0 |
| 1 | 1000 | 1000 | 3 | 4.74 / 6.59 / 16.38 (10.75–26.62) | 2.69 / 3.87 / 6.72 (5.82–10.11) | 1.28 / 3.55 / 6.98 (3.87–9.86) | 1.28 / 3.49 / 6.46 (3.87–26.37) | 1.20 / 1.33 / 4.42 (2.27–6.98) | 1.20 / 1.28 / 2.30 (2.03–6.27) | 1.20 / 1.33 / 2.37 (2.27–27.39) | 1.15 / 1.49 / 4.22 (2.24–14.21) | 1.20 / 1.28 / 4.22 (1.82–4.86) | 100% | 5 |
| 1 | 1000 | 1000 | 4 | 5.12 / 7.49 / 15.23 (7.10–22.27) | 2.78 / 4.16 / 10.75 (4.10–22.02) | 2.78 / 3.81 / 5.63 (3.90–17.41) | 2.78 / 3.94 / 6.46 (5.95–6.98) | 1.20 / 2.14 / 3.39 (3.33–5.57) | 1.20 / 1.33 / 4.42 (2.03–5.95) | 1.20 / 2.30 / 3.46 (2.37–5.44) | 1.15 / 1.70 / 4.29 (4.22–4.80) | 1.20 / 1.62 / 4.29 (2.30–4.35) | 99% | 0 |
| 1 | 1000 | 1000 | 6 | 6.14 / 8.32 / 20.22 (14.34–1638.40) | 3.39 / 5.31 / 8.32 (6.40–11.90) | 3.39 / 4.86 / 5.95 (5.82–6.40) | 3.33 / 4.80 / 11.14 (6.40–12.42) | 1.70 / 4.42 / 5.95 (4.74–10.50) | 1.66 / 4.29 / 5.57 (4.99–11.78) | 1.36 / 2.50 / 3.10 (2.82–4.61) | 1.36 / 2.37 / 2.78 (2.66–2.98) | 1.36 / 1.78 / 2.72 (2.56–2.82) | 66% | 32 |
| 1 | 1000 | 1000 | 8 | 6.53 / 10.37 / 76.80 (12.80–671.74) | 3.55 / 5.95 / 10.75 (6.91–12.54) | 3.46 / 5.12 / 7.10 (6.08–9.60) | 3.52 / 5.25 / 7.04 (5.89–9.22) | 2.62 / 4.42 / 5.89 (4.99–8.83) | 2.40 / 4.48 / 5.44 (5.18–9.60) | 1.41 / 2.66 / 2.98 (2.91–6.08) | 1.36 / 2.43 / 2.94 (2.85–3.10) | 1.41 / 1.95 / 2.82 (2.62–3.04) | 49% | 17 |
| 10 | 0 | 50 | 2 | 0.62 / 0.83 / 2.24 (1.04–2.91) | 0.25 / 0.74 / 0.83 (0.83–0.87) | 0.21 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.54) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.25 / 0.33 / 0.46 (0.46–0.50) | 0.21 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 100% | 0 |
| 10 | 0 | 50 | 3 | 0.79 / 1.12 / 4.54 (1.46–6.91) | 0.54 / 0.83 / 1.04 (1.00–1.82) | 0.37 / 0.87 / 1.00 (0.95–1.00) | 0.33 / 0.87 / 1.00 (0.95–1.04) | 0.25 / 0.46 / 0.58 (0.54–0.62) | 0.29 / 0.37 / 0.58 (0.54–0.58) | 0.25 / 0.46 / 0.54 (0.54–0.62) | 0.25 / 0.46 / 0.58 (0.54–0.62) | 0.29 / 0.37 / 0.58 (0.54–0.62) | 100% | 0 |
| 10 | 0 | 50 | 4 | 0.91 / 1.36 / 6.21 (2.43–16.64) | 0.58 / 1.04 / 2.11 (1.25–4.16) | 0.58 / 1.36 / 2.56 (1.62–3.04) | 0.58 / 1.25 / 1.54 (1.49–2.62) | 0.25 / 0.50 / 0.58 (0.58–0.74) | 0.29 / 0.46 / 0.62 (0.58–0.66) | 0.25 / 0.50 / 0.74 (0.58–0.79) | 0.25 / 0.50 / 0.70 (0.58–0.95) | 0.29 / 0.46 / 0.58 (0.58–0.70) | 100% | 0 |
| 10 | 0 | 50 | 6 | 1.54 / 3.07 / 9.09 (7.04–11.26) | 0.91 / 2.21 / 3.10 (2.94–4.42) | 1.15 / 2.56 / 3.62 (3.39–3.94) | 1.15 / 2.43 / 3.49 (3.07–12.54) | 0.66 / 2.50 / 3.49 (2.78–4.29) | 0.66 / 2.43 / 3.26 (2.98–4.16) | 0.46 / 0.95 / 1.25 (1.15–1.41) | 0.50 / 1.07 / 1.36 (1.25–1.41) | 0.46 / 0.91 / 1.20 (1.15–1.28) | 66% | 33 |
| 10 | 0 | 50 | 8 | 1.74 / 4.29 / 10.24 (9.34–17.15) | 1.12 / 3.20 / 4.86 (4.35–5.12) | 1.33 / 3.04 / 4.16 (4.06–4.61) | 1.36 / 3.04 / 4.35 (3.94–5.12) | 1.00 / 2.91 / 3.78 (3.74–3.94) | 1.04 / 2.94 / 4.00 (3.81–4.10) | 0.46 / 1.00 / 1.28 (1.25–1.36) | 0.50 / 1.12 / 1.54 (1.33–1.62) | 0.54 / 1.07 / 1.36 (1.33–1.46) | 49% | 6 |
| 10 | 0 | 200 | 2 | 0.87 / 1.20 / 1.28 (1.25–1.66) | 0.42 / 1.00 / 1.12 (1.12–1.20) | 0.33 / 0.54 / 0.74 (0.66–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.79) | 0.37 / 0.54 / 0.74 (0.62–0.74) | 0.37 / 0.50 / 0.74 (0.74–0.79) | 0.37 / 0.54 / 0.70 (0.62–0.74) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.74 (0.74–0.79) | 100% | 0 |
| 10 | 0 | 200 | 3 | 1.07 / 1.57 / 6.14 (3.23–9.86) | 0.74 / 1.15 / 1.41 (1.33–3.87) | 0.50 / 1.12 / 1.33 (1.33–1.82) | 0.50 / 1.12 / 1.28 (1.28–6.21) | 0.37 / 0.66 / 0.83 (0.79–0.83) | 0.42 / 0.62 / 0.87 (0.83–0.87) | 0.37 / 0.66 / 0.83 (0.78–0.87) | 0.37 / 0.70 / 0.83 (0.83–0.95) | 0.42 / 0.62 / 0.83 (0.83–0.91) | 100% | 0 |
| 10 | 0 | 200 | 4 | 1.15 / 1.79 / 11.39 (4.42–19.71) | 0.78 / 1.36 / 2.69 (1.62–5.25) | 0.79 / 1.41 / 1.74 (1.70–4.06) | 0.78 / 1.46 / 1.78 (1.70–7.30) | 0.42 / 0.74 / 1.04 (0.87–3.46) | 0.42 / 0.74 / 0.91 (0.87–4.03) | 0.42 / 0.74 / 0.95 (0.87–3.68) | 0.37 / 0.74 / 0.91 (0.87–1.07) | 0.42 / 0.74 / 0.87 (0.87–1.25) | 100% | 0 |
| 10 | 0 | 200 | 6 | 2.08 / 4.16 / 10.24 (6.21–11.26) | 1.12 / 2.56 / 3.90 (3.65–4.42) | 1.33 / 2.69 / 3.52 (3.46–4.48) | 1.36 / 2.85 / 3.94 (3.81–4.16) | 0.74 / 2.27 / 3.26 (3.07–8.70) | 0.74 / 2.27 / 3.46 (2.91–3.87) | 0.58 / 1.20 / 1.58 (1.54–1.74) | 0.54 / 1.25 / 1.66 (1.62–1.70) | 0.62 / 1.20 / 1.57 (1.54–2.30) | 66% | 27 |
| 10 | 0 | 200 | 8 | 2.08 / 4.74 / 11.01 (8.70–75.78) | 1.28 / 3.65 / 5.57 (5.31–18.43) | 1.49 / 3.26 / 4.74 (4.29–56.32) | 1.54 / 3.49 / 4.86 (4.48–23.30) | 1.25 / 3.04 / 4.16 (3.94–16.38) | 1.20 / 3.04 / 4.35 (4.16–19.20) | 0.58 / 1.41 / 1.87 (1.87–8.19) | 0.58 / 1.33 / 1.78 (1.74–7.36) | 0.62 / 1.28 / 1.78 (1.62–4.03) | 49% | 67 |
| 10 | 0 | 1000 | 2 | 2.43 / 3.55 / 6.91 (6.02–28.67) | 1.58 / 2.66 / 5.89 (2.82–8.19) | 1.15 / 1.46 / 1.90 (1.74–8.83) | 1.15 / 1.62 / 2.30 (2.27–8.45) | 1.15 / 1.49 / 2.00 (1.74–7.94) | 1.15 / 1.74 / 2.30 (2.24–7.10) | 1.15 / 1.46 / 2.03 (1.90–5.82) | 1.15 / 1.90 / 2.27 (2.24–9.09) | 1.15 / 1.62 / 2.30 (2.14–9.86) | 100% | 3 |
| 10 | 0 | 1000 | 3 | 2.72 / 4.61 / 11.39 (8.19–30.21) | 1.70 / 3.14 / 5.31 (3.87–18.94) | 1.28 / 2.53 / 3.26 (3.04–6.91) | 1.28 / 2.66 / 3.33 (3.07–6.21) | 1.20 / 2.30 / 2.53 (2.50–2.56) | 1.20 / 2.08 / 2.50 (2.43–6.27) | 1.20 / 2.30 / 2.53 (2.43–4.16) | 1.15 / 2.27 / 2.56 (2.53–3.58) | 1.20 / 2.11 / 2.43 (2.40–6.34) | 100% | 0 |
| 10 | 0 | 1000 | 4 | 3.26 / 5.12 / 8.70 (6.78–29.18) | 1.78 / 3.74 / 4.80 (4.29–15.62) | 1.79 / 2.98 / 4.00 (3.49–8.45) | 1.79 / 3.10 / 4.06 (3.74–6.91) | 1.20 / 2.43 / 3.58 (3.49–8.96) | 1.20 / 2.27 / 2.85 (2.50–4.22) | 1.20 / 2.43 / 3.52 (3.46–13.57) | 1.20 / 2.37 / 4.61 (3.26–9.98) | 1.20 / 2.27 / 2.82 (2.50–5.12) | 99% | 0 |
| 10 | 0 | 1000 | 6 | 4.48 / 8.06 / 16.26 (12.54–483.33) | 2.37 / 5.70 / 8.45 (7.87–17.41) | 2.43 / 4.86 / 6.91 (6.40–12.42) | 2.53 / 5.44 / 7.87 (7.36–12.16) | 1.58 / 4.06 / 5.44 (5.31–10.50) | 1.74 / 4.29 / 5.57 (5.44–7.81) | 1.41 / 3.81 / 5.12 (4.74–5.31) | 1.41 / 3.49 / 4.48 (4.42–4.86) | 1.46 / 2.94 / 4.16 (4.00–4.48) | 66% | 51 |
| 10 | 0 | 1000 | 8 | 4.80 / 9.47 / 16.13 (14.08–18.18) | 2.72 / 7.23 / 10.75 (9.47–18.94) | 2.82 / 6.53 / 9.47 (9.09–10.24) | 2.91 / 7.10 / 11.52 (10.62–11.90) | 2.43 / 4.80 / 6.91 (6.21–8.70) | 2.50 / 5.12 / 7.10 (6.53–9.98) | 1.41 / 4.06 / 5.44 (5.38–5.50) | 1.46 / 4.10 / 5.70 (5.57–5.95) | 1.49 / 3.14 / 4.48 (4.29–4.74) | 49% | 10 |
| 10 | 1000 | 50 | 2 | 1.62 / 1.82 / 5.25 (1.95–8.70) | 0.25 / 1.74 / 1.87 (1.82–3.04) | 0.25 / 0.33 / 0.50 (0.50–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.21 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.50–0.50) | 0.21 / 0.37 / 0.50 (0.46–0.50) | 0.21 / 0.33 / 0.50 (0.46–0.50) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 100% | 0 |
| 10 | 1000 | 50 | 3 | 2.69 / 3.10 / 8.83 (3.33–11.26) | 1.54 / 1.95 / 4.48 (2.11–6.53) | 0.37 / 1.82 / 2.00 (1.95–2.11) | 0.33 / 1.82 / 1.90 (1.90–2.08) | 0.25 / 0.46 / 0.54 (0.54–0.70) | 0.29 / 0.42 / 0.58 (0.58–0.62) | 0.25 / 0.46 / 0.54 (0.54–0.54) | 0.25 / 0.46 / 0.58 (0.54–0.62) | 0.29 / 0.37 / 0.58 (0.58–0.66) | 100% | 0 |
| 10 | 1000 | 50 | 4 | 2.85 / 3.46 / 10.24 (6.46–12.93) | 1.57 / 2.11 / 5.44 (2.37–9.86) | 0.42 / 2.43 / 2.69 (2.66–4.16) | 0.46 / 2.43 / 3.04 (2.66–3.87) | 0.25 / 0.54 / 0.83 (0.58–0.83) | 0.29 / 0.46 / 0.58 (0.58–0.62) | 0.25 / 0.50 / 0.70 (0.58–0.91) | 0.25 / 0.50 / 0.70 (0.58–0.87) | 0.29 / 0.46 / 0.62 (0.58–0.74) | 100% | 0 |
| 10 | 1000 | 50 | 6 | 3.52 / 5.70 / 11.26 (10.37–14.34) | 2.00 / 3.94 / 6.66 (5.06–8.58) | 2.03 / 3.87 / 4.74 (4.10–5.89) | 2.00 / 3.81 / 4.86 (4.48–5.12) | 0.62 / 3.36 / 4.29 (4.03–4.48) | 0.66 / 3.55 / 4.80 (3.07–5.12) | 0.42 / 0.95 / 1.25 (1.12–1.41) | 0.42 / 1.00 / 1.36 (1.12–1.41) | 0.50 / 1.00 / 1.25 (0.95–1.28) | 66% | 29 |
| 10 | 1000 | 50 | 8 | 3.68 / 7.17 / 18.94 (13.57–135.17) | 2.11 / 4.80 / 8.00 (7.10–19.20) | 2.27 / 4.54 / 6.27 (5.31–7.62) | 2.21 / 4.16 / 6.46 (5.70–7.74) | 0.91 / 4.06 / 5.18 (4.54–9.60) | 0.87 / 4.06 / 5.12 (4.74–5.25) | 0.46 / 1.00 / 1.28 (1.28–1.33) | 0.50 / 1.15 / 1.54 (1.20–1.66) | 0.46 / 0.87 / 1.12 (1.07–1.33) | 49% | 70 |
| 10 | 1000 | 200 | 2 | 1.87 / 2.21 / 2.37 (2.24–4.74) | 0.37 / 2.03 / 2.69 (2.11–11.14) | 0.37 / 0.54 / 0.70 (0.70–0.74) | 0.37 / 0.50 / 0.78 (0.74–0.83) | 0.37 / 0.54 / 0.74 (0.70–0.74) | 0.37 / 0.50 / 0.79 (0.74–0.79) | 0.33 / 0.54 / 0.74 (0.70–0.74) | 0.33 / 0.58 / 0.74 (0.74–0.78) | 0.37 / 0.50 / 0.78 (0.74–0.79) | 100% | 0 |
| 10 | 1000 | 200 | 3 | 2.98 / 3.49 / 9.22 (5.57–99.33) | 1.70 / 2.24 / 5.70 (4.10–14.72) | 0.50 / 2.08 / 2.27 (2.21–8.45) | 0.46 / 2.08 / 2.56 (2.21–12.16) | 0.37 / 0.66 / 0.83 (0.79–0.87) | 0.42 / 0.62 / 0.87 (0.83–1.04) | 0.37 / 0.66 / 0.83 (0.79–0.91) | 0.37 / 0.70 / 0.83 (0.83–0.91) | 0.42 / 0.62 / 0.83 (0.83–0.87) | 99% | 12 |
| 10 | 1000 | 200 | 4 | 3.10 / 4.06 / 19.97 (4.54–27.65) | 1.74 / 2.43 / 3.52 (2.78–7.23) | 0.70 / 2.62 / 2.91 (2.85–5.70) | 0.66 / 2.66 / 3.52 (2.85–5.06) | 0.42 / 0.74 / 1.00 (0.91–5.82) | 0.42 / 0.74 / 0.91 (0.87–6.27) | 0.42 / 0.74 / 1.07 (0.87–2.08) | 0.37 / 0.74 / 1.07 (0.87–1.20) | 0.42 / 0.74 / 1.07 (0.87–6.27) | 99% | 0 |
| 10 | 1000 | 200 | 6 | 3.81 / 6.46 / 14.21 (12.80–51.20) | 2.11 / 4.16 / 6.53 (5.50–9.98) | 2.14 / 4.10 / 5.57 (4.67–5.89) | 2.21 / 4.22 / 5.57 (4.86–5.89) | 0.74 / 3.46 / 4.48 (4.10–5.06) | 0.74 / 3.46 / 4.93 (4.10–7.49) | 0.58 / 1.20 / 1.66 (1.57–7.68) | 0.54 / 1.25 / 1.66 (1.58–6.27) | 0.58 / 1.12 / 1.74 (1.54–6.08) | 65% | 47 |
| 10 | 1000 | 200 | 8 | 4.06 / 7.49 / 16.00 (14.21–17.66) | 2.30 / 5.18 / 7.74 (7.04–10.24) | 2.37 / 4.54 / 6.85 (5.57–7.49) | 2.37 / 4.54 / 6.59 (6.08–8.32) | 1.20 / 4.16 / 5.31 (4.99–7.68) | 1.15 / 4.16 / 5.31 (5.06–6.72) | 0.58 / 1.46 / 1.87 (1.79–2.00) | 0.58 / 1.33 / 1.79 (1.70–2.50) | 0.58 / 1.25 / 1.62 (1.41–1.87) | 49% | 39 |
| 10 | 1000 | 1000 | 2 | 3.46 / 4.54 / 7.87 (4.61–13.95) | 2.00 / 3.65 / 5.57 (3.81–11.26) | 1.15 / 1.46 / 1.78 (1.49–2.14) | 1.15 / 1.54 / 2.30 (2.14–2.37) | 1.15 / 1.46 / 1.87 (1.54–2.40) | 1.15 / 1.54 / 2.27 (2.24–3.26) | 1.15 / 1.46 / 1.90 (1.49–4.29) | 1.15 / 1.82 / 2.27 (2.00–2.37) | 1.15 / 1.57 / 2.30 (2.00–5.57) | 100% | 1 |
| 10 | 1000 | 1000 | 3 | 4.61 / 6.66 / 13.18 (11.14–25.60) | 2.69 / 4.29 / 5.89 (4.99–16.38) | 1.28 / 3.14 / 4.03 (3.78–6.08) | 1.28 / 3.49 / 5.95 (3.81–9.34) | 1.20 / 2.30 / 2.53 (2.50–5.95) | 1.20 / 2.11 / 2.50 (2.40–5.89) | 1.20 / 2.30 / 2.53 (2.43–4.35) | 1.15 / 2.27 / 2.62 (2.53–7.62) | 1.20 / 2.11 / 4.22 (2.43–6.34) | 99% | 0 |
| 10 | 1000 | 1000 | 4 | 4.99 / 7.74 / 19.71 (10.11–33.28) | 2.78 / 4.86 / 5.82 (5.70–16.64) | 2.08 / 4.00 / 5.44 (4.86–10.11) | 2.00 / 4.03 / 5.50 (4.29–11.26) | 1.20 / 2.43 / 3.52 (3.49–6.27) | 1.20 / 2.27 / 3.20 (2.78–4.99) | 1.20 / 2.43 / 3.52 (3.49–7.04) | 1.20 / 2.40 / 4.03 (3.23–4.35) | 1.20 / 2.27 / 2.82 (2.69–4.29) | 100% | 0 |
| 10 | 1000 | 1000 | 6 | 6.21 / 9.98 / 15.74 (12.42–26.88) | 3.52 / 7.30 / 11.26 (10.11–12.29) | 3.23 / 6.08 / 8.70 (8.06–9.98) | 3.26 / 6.21 / 8.45 (7.68–11.65) | 1.57 / 4.54 / 6.78 (6.14–7.36) | 1.66 / 4.48 / 6.66 (6.21–8.70) | 1.41 / 3.78 / 4.42 (4.29–5.25) | 1.41 / 3.52 / 4.48 (4.16–6.72) | 1.41 / 2.82 / 4.00 (3.78–4.99) | 66% | 55 |
| 10 | 1000 | 1000 | 8 | 6.85 / 12.16 / 21.25 (17.66–53.76) | 3.81 / 8.58 / 13.06 (11.14–14.98) | 3.65 / 8.00 / 10.88 (10.62–11.65) | 3.58 / 8.00 / 11.52 (10.62–13.06) | 2.78 / 5.57 / 7.36 (7.30–8.96) | 2.66 / 5.76 / 8.19 (7.62–9.34) | 1.41 / 4.06 / 5.44 (5.31–5.57) | 1.46 / 4.10 / 6.02 (5.38–8.19) | 1.49 / 3.20 / 4.48 (4.29–6.34) | 49% | 10 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 17.95; 2/2 fine: 0.20; 2/4 fair: 33.20; 2/4 fine: 0.28; 3 fair: 54.73; 3 slot read: 0.22; 3 fine: 0.23;

Migrations seen: 286733 over 1039587883 samples (one sample per measured operation).


### Suite a2t: IPC call and reply, lock per endpoint

load average before: { 5.17 5.26 4.72 }
load average after: { 5.45 5.32 4.83 }; 173 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 321k (318k–322k) | 320k (316k–321k) | 313k (312k–315k) | 313k (313k–315k) | 313k (313k–315k) | 314k (312k–314k) | 314k (312k–315k) | 314k (312k–315k) | 100% | 0 |
| 0 | 50 | 4 | 639k (636k–643k) | 640k (638k–642k) | 625k (622k–626k) | 623k (619k–625k) | 595k (584k–599k) | 624k (619k–626k) | 599k (591k–601k) | 624k (620k–626k) | 100% | 0 |
| 0 | 50 | 6 | 920k (918k–939k) | 922k (904k–928k) | 852k (845k–913k) | 871k (842k–902k) | 797k (770k–817k) | 855k (840k–880k) | 749k (717k–775k) | 860k (832k–894k) | 68% | 16 |
| 0 | 50 | 8 | 1.22M (1.21M–1.23M) | 1.24M (1.21M–1.26M) | 1.18M (1.09M–1.21M) | 1.16M (1.10M–1.19M) | 1.00M (963k–1.02M) | 1.19M (1.17M–1.21M) | 928k (829k–953k) | 1.17M (1.15M–1.18M) | 49% | 12 |
| 0 | 200 | 2 | 286k (286k–287k) | 286k (285k–287k) | 281k (281k–282k) | 281k (281k–282k) | 281k (280k–281k) | 281k (281k–282k) | 280k (280k–282k) | 281k (279k–282k) | 100% | 0 |
| 0 | 200 | 4 | 569k (569k–571k) | 570k (569k–572k) | 559k (556k–560k) | 559k (557k–562k) | 533k (527k–541k) | 560k (556k–561k) | 538k (531k–541k) | 559k (556k–560k) | 100% | 0 |
| 0 | 200 | 6 | 830k (803k–835k) | 813k (803k–834k) | 777k (768k–812k) | 805k (773k–849k) | 724k (687k–736k) | 800k (796k–810k) | 654k (637k–687k) | 804k (782k–806k) | 67% | 16 |
| 0 | 200 | 8 | 1.07M (938k–1.11M) | 1.09M (1.06M–1.11M) | 1.05M (1.04M–1.07M) | 1.06M (1.06M–1.08M) | 880k (824k–912k) | 1.06M (1.01M–1.06M) | 765k (566k–790k) | 1.06M (947k–1.08M) | 50% | 84 |
| 0 | 1000 | 2 | 200k (198k–201k) | 200k (199k–201k) | 198k (197k–198k) | 198k (198k–199k) | 198k (197k–198k) | 198k (198k–198k) | 198k (197k–199k) | 198k (197k–199k) | 100% | 0 |
| 0 | 1000 | 4 | 398k (398k–400k) | 398k (397k–400k) | 394k (392k–395k) | 394k (392k–395k) | 378k (377k–383k) | 393k (392k–395k) | 378k (376k–381k) | 394k (392k–396k) | 99% | 0 |
| 0 | 1000 | 6 | 578k (407k–586k) | 585k (569k–591k) | 574k (550k–577k) | 562k (552k–574k) | 537k (527k–541k) | 572k (551k–585k) | 342k (334k–350k) | 571k (549k–577k) | 67% | 61 |
| 0 | 1000 | 8 | 792k (540k–793k) | 784k (734k–789k) | 754k (749k–774k) | 761k (755k–773k) | 656k (646k–667k) | 766k (747k–769k) | 338k (327k–345k) | 755k (739k–765k) | 50% | 39 |
| 1000 | 50 | 2 | 195k (194k–195k) | 195k (195k–196k) | 190k (187k–191k) | 190k (190k–190k) | 190k (190k–191k) | 190k (190k–191k) | 190k (190k–191k) | 190k (184k–190k) | 100% | 2 |
| 1000 | 50 | 4 | 389k (386k–391k) | 390k (388k–392k) | 379k (376k–381k) | 380k (377k–380k) | 370k (370k–373k) | 378k (377k–380k) | 371k (371k–372k) | 379k (374k–380k) | 100% | 0 |
| 1000 | 50 | 6 | 575k (569k–580k) | 569k (552k–575k) | 548k (539k–556k) | 542k (525k–557k) | 520k (512k–539k) | 545k (536k–552k) | 499k (495k–526k) | 546k (527k–554k) | 67% | 20 |
| 1000 | 50 | 8 | 761k (732k–776k) | 763k (736k–772k) | 734k (719k–744k) | 738k (723k–742k) | 705k (696k–716k) | 732k (695k–739k) | 677k (658k–699k) | 734k (700k–740k) | 50% | 17 |
| 1000 | 200 | 2 | 182k (181k–182k) | 182k (182k–183k) | 177k (176k–178k) | 178k (177k–178k) | 178k (177k–178k) | 178k (176k–178k) | 178k (174k–178k) | 178k (177k–178k) | 100% | 0 |
| 1000 | 200 | 4 | 361k (360k–363k) | 364k (361k–365k) | 354k (352k–355k) | 354k (351k–356k) | 344k (342k–346k) | 353k (352k–355k) | 344k (344k–346k) | 354k (353k–354k) | 100% | 0 |
| 1000 | 200 | 6 | 532k (529k–535k) | 531k (522k–538k) | 511k (498k–516k) | 513k (507k–517k) | 501k (468k–504k) | 518k (514k–534k) | 481k (465k–487k) | 515k (511k–517k) | 66% | 42 |
| 1000 | 200 | 8 | 715k (706k–723k) | 711k (701k–714k) | 694k (688k–705k) | 693k (684k–700k) | 655k (634k–659k) | 695k (684k–701k) | 611k (575k–641k) | 695k (686k–696k) | 49% | 15 |
| 1000 | 1000 | 2 | 142k (142k–143k) | 143k (143k–143k) | 140k (140k–140k) | 140k (138k–140k) | 140k (139k–141k) | 140k (140k–140k) | 140k (140k–141k) | 140k (139k–140k) | 100% | 0 |
| 1000 | 1000 | 4 | 284k (240k–285k) | 285k (283k–286k) | 279k (278k–279k) | 280k (278k–281k) | 273k (273k–275k) | 280k (278k–280k) | 274k (271k–275k) | 279k (278k–281k) | 99% | 14 |
| 1000 | 1000 | 6 | 417k (415k–420k) | 421k (413k–425k) | 403k (397k–409k) | 405k (395k–409k) | 397k (382k–400k) | 404k (398k–411k) | 347k (326k–351k) | 404k (403k–409k) | 66% | 27 |
| 1000 | 1000 | 8 | 561k (557k–563k) | 563k (562k–563k) | 542k (508k–546k) | 544k (530k–546k) | 514k (507k–522k) | 543k (535k–548k) | 340k (331k–341k) | 541k (537k–550k) | 50% | 12 |

Round trip latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 1.74 / 1.87 / 2.53 (1.95–6.40) | 1.74 / 1.87 / 2.14 (1.95–11.90) | 1.82 / 1.90 / 2.11 (2.03–5.44) | 1.82 / 1.90 / 3.04 (2.08–6.08) | 1.82 / 1.90 / 2.03 (2.03–7.36) | 1.82 / 1.90 / 2.43 (2.00–6.27) | 1.82 / 1.90 / 2.11 (2.00–8.45) | 1.82 / 1.90 / 2.11 (2.03–9.47) | 100% | 0 |
| 0 | 50 | 4 | 1.74 / 1.90 / 5.95 (2.72–9.22) | 1.74 / 1.87 / 4.29 (1.90–8.32) | 1.82 / 1.95 / 4.29 (2.85–9.09) | 1.82 / 2.27 / 3.33 (2.78–9.60) | 1.95 / 2.37 / 6.98 (2.50–11.52) | 1.82 / 2.00 / 6.40 (2.00–9.47) | 1.90 / 2.40 / 7.42 (2.43–10.62) | 1.82 / 1.90 / 5.57 (2.00–8.83) | 100% | 0 |
| 0 | 50 | 6 | 1.79 / 2.30 / 4.42 (2.53–8.00) | 1.90 / 2.30 / 3.52 (2.53–7.10) | 2.24 / 2.72 / 3.20 (2.82–5.57) | 1.87 / 2.69 / 3.52 (2.78–7.36) | 2.40 / 3.46 / 3.90 (3.74–7.10) | 2.14 / 2.69 / 3.14 (2.78–5.38) | 2.66 / 3.49 / 8.13 (4.42–10.11) | 1.90 / 2.72 / 3.23 (2.82–7.49) | 68% | 16 |
| 0 | 50 | 8 | 1.87 / 2.37 / 5.44 (2.56–8.83) | 1.82 / 2.37 / 5.25 (2.69–8.96) | 2.11 / 2.69 / 2.85 (2.82–10.62) | 1.90 / 2.69 / 7.62 (2.82–10.50) | 2.66 / 3.65 / 9.22 (8.83–17.15) | 2.08 / 2.62 / 7.49 (2.82–9.09) | 2.91 / 4.00 / 11.65 (7.62–13.44) | 1.90 / 2.69 / 5.82 (3.39–9.34) | 49% | 12 |
| 0 | 200 | 2 | 2.11 / 2.21 / 2.30 (2.30–7.74) | 2.11 / 2.24 / 5.18 (2.40–7.87) | 2.21 / 2.30 / 5.25 (2.43–8.00) | 2.21 / 2.30 / 2.53 (2.37–6.91) | 2.21 / 2.30 / 5.76 (2.62–8.70) | 2.14 / 2.30 / 7.49 (2.37–8.19) | 2.21 / 2.30 / 5.06 (2.37–8.32) | 2.14 / 2.30 / 6.53 (5.50–8.58) | 100% | 0 |
| 0 | 200 | 4 | 2.11 / 2.24 / 6.27 (2.37–9.60) | 2.11 / 2.24 / 5.82 (2.66–9.09) | 2.21 / 2.30 / 3.65 (2.40–8.96) | 2.21 / 2.30 / 6.46 (2.40–9.34) | 2.30 / 3.23 / 8.06 (3.52–11.52) | 2.21 / 2.30 / 7.42 (2.78–8.83) | 2.30 / 2.91 / 3.46 (2.94–11.26) | 2.21 / 2.56 / 7.04 (2.98–10.50) | 100% | 0 |
| 0 | 200 | 6 | 2.24 / 2.69 / 5.70 (2.91–7.81) | 2.40 / 2.72 / 3.10 (2.94–8.00) | 2.53 / 2.98 / 7.74 (3.14–8.96) | 2.37 / 2.91 / 6.91 (3.04–8.19) | 2.72 / 3.78 / 6.27 (3.90–9.86) | 2.37 / 2.94 / 4.74 (3.07–7.94) | 3.36 / 4.10 / 5.82 (4.99–9.73) | 2.27 / 2.94 / 5.31 (3.10–9.86) | 67% | 16 |
| 0 | 200 | 8 | 2.37 / 2.72 / 7.87 (4.67–9.86) | 2.37 / 2.72 / 4.67 (3.20–9.34) | 2.53 / 2.98 / 7.17 (6.14–9.47) | 2.43 / 2.98 / 6.34 (3.81–7.87) | 3.10 / 4.16 / 9.34 (7.87–10.88) | 2.43 / 2.98 / 7.49 (3.10–9.22) | 3.74 / 5.25 / 15.62 (12.93–48.13) | 2.50 / 2.98 / 4.99 (4.03–69.63) | 50% | 84 |
| 0 | 1000 | 2 | 3.62 / 3.74 / 9.34 (8.00–16.00) | 3.62 / 3.74 / 11.01 (6.27–11.52) | 3.65 / 3.78 / 8.32 (7.10–12.80) | 3.65 / 3.78 / 8.70 (5.57–11.01) | 3.65 / 3.81 / 10.75 (8.32–13.44) | 3.65 / 3.78 / 9.73 (7.10–11.52) | 3.65 / 3.78 / 8.83 (3.90–12.03) | 3.65 / 3.81 / 10.11 (4.99–12.42) | 100% | 0 |
| 0 | 1000 | 4 | 3.62 / 3.87 / 11.52 (8.83–12.29) | 3.62 / 3.81 / 12.03 (6.72–13.82) | 3.68 / 3.81 / 10.75 (3.87–13.44) | 3.68 / 3.81 / 10.37 (5.31–14.59) | 3.81 / 5.25 / 15.23 (7.04–16.64) | 3.65 / 3.81 / 12.03 (6.66–13.82) | 3.81 / 5.18 / 14.08 (11.65–19.20) | 3.68 / 3.81 / 8.96 (6.40–14.72) | 99% | 0 |
| 0 | 1000 | 6 | 3.81 / 4.22 / 9.09 (4.48–10.88) | 3.68 / 4.16 / 10.11 (8.19–20.74) | 3.78 / 4.48 / 9.60 (5.25–11.39) | 3.90 / 4.35 / 10.88 (5.89–13.18) | 4.16 / 5.95 / 12.03 (8.96–12.42) | 3.90 / 4.48 / 9.73 (5.12–11.65) | 7.30 / 8.32 / 17.41 (16.00–18.69) | 3.90 / 4.48 / 8.45 (7.30–11.26) | 67% | 61 |
| 0 | 1000 | 8 | 3.68 / 4.22 / 10.88 (5.89–14.34) | 3.74 / 4.22 / 10.50 (9.34–23.55) | 3.90 / 4.48 / 10.37 (9.09–13.82) | 3.94 / 4.54 / 10.75 (6.27–11.52) | 4.48 / 6.46 / 12.29 (11.78–17.92) | 3.90 / 4.54 / 9.73 (7.04–11.78) | 10.37 / 11.52 / 23.55 (20.99–27.90) | 3.94 / 4.54 / 10.75 (9.73–13.70) | 50% | 39 |
| 1000 | 50 | 2 | 3.74 / 3.87 / 4.00 (4.00–10.11) | 3.74 / 3.87 / 9.60 (8.19–10.50) | 3.87 / 4.00 / 9.22 (7.87–14.46) | 3.87 / 4.00 / 9.47 (4.10–12.03) | 3.87 / 4.00 / 8.83 (8.32–12.03) | 3.87 / 4.00 / 8.45 (4.16–11.26) | 3.87 / 4.00 / 9.22 (5.82–10.62) | 3.87 / 4.00 / 8.45 (4.06–27.14) | 100% | 2 |
| 1000 | 50 | 4 | 3.74 / 3.94 / 12.03 (3.94–15.49) | 3.74 / 4.03 / 9.86 (3.90–15.36) | 3.90 / 4.06 / 10.88 (6.85–13.95) | 3.87 / 4.00 / 12.03 (4.06–16.00) | 4.00 / 4.42 / 12.42 (9.60–16.13) | 3.87 / 4.22 / 12.80 (5.44–16.38) | 4.00 / 4.29 / 12.29 (6.08–15.23) | 3.87 / 4.03 / 9.34 (8.58–15.62) | 100% | 0 |
| 1000 | 50 | 6 | 3.78 / 4.29 / 10.62 (6.91–11.90) | 3.90 / 4.35 / 11.14 (5.12–18.69) | 4.00 / 4.80 / 10.88 (6.59–12.29) | 4.16 / 4.86 / 8.83 (5.95–11.65) | 4.48 / 4.99 / 12.29 (8.32–23.04) | 4.10 / 4.86 / 10.62 (8.83–12.16) | 4.61 / 4.99 / 9.09 (8.00–12.80) | 3.94 / 4.86 / 10.50 (6.91–11.52) | 67% | 20 |
| 1000 | 50 | 8 | 3.90 / 4.29 / 10.62 (7.49–11.90) | 3.81 / 4.35 / 11.14 (6.14–11.39) | 4.10 / 4.74 / 11.26 (8.70–12.42) | 4.16 / 4.67 / 10.62 (8.19–13.57) | 4.35 / 4.93 / 10.88 (8.45–26.88) | 4.16 / 4.86 / 9.60 (8.45–11.90) | 4.61 / 5.06 / 11.39 (11.14–14.98) | 4.10 / 4.86 / 9.73 (9.22–11.39) | 50% | 17 |
| 1000 | 200 | 2 | 4.10 / 4.22 / 8.06 (4.42–10.62) | 4.10 / 4.22 / 9.47 (4.42–12.16) | 4.22 / 4.42 / 9.60 (4.48–12.42) | 4.22 / 4.42 / 9.34 (7.62–12.67) | 4.22 / 4.42 / 8.00 (5.70–11.01) | 4.22 / 4.42 / 9.47 (4.48–14.85) | 4.22 / 4.42 / 9.09 (4.48–14.46) | 4.22 / 4.42 / 10.37 (8.19–11.52) | 100% | 0 |
| 1000 | 200 | 4 | 4.16 / 4.29 / 9.09 (4.35–14.21) | 4.10 / 4.22 / 8.83 (5.89–14.85) | 4.29 / 4.48 / 9.98 (4.48–16.38) | 4.29 / 4.42 / 9.98 (8.32–17.41) | 4.42 / 4.80 / 14.98 (8.45–16.90) | 4.22 / 4.61 / 13.57 (4.48–17.66) | 4.42 / 4.86 / 13.44 (8.58–17.41) | 4.29 / 4.42 / 9.86 (9.09–16.13) | 100% | 0 |
| 1000 | 200 | 6 | 4.22 / 4.67 / 10.37 (5.44–12.54) | 4.22 / 4.74 / 11.52 (6.21–12.29) | 4.48 / 5.18 / 11.78 (5.82–13.18) | 4.48 / 5.12 / 12.54 (10.11–18.43) | 4.61 / 5.25 / 12.42 (11.26–18.43) | 4.35 / 4.99 / 11.78 (6.78–13.82) | 4.74 / 5.38 / 13.18 (12.67–16.90) | 4.29 / 5.12 / 12.16 (7.74–14.46) | 66% | 42 |
| 1000 | 200 | 8 | 4.16 / 4.74 / 12.03 (8.58–15.10) | 4.29 / 4.67 / 12.54 (10.62–15.10) | 4.48 / 5.12 / 12.42 (9.47–16.38) | 4.42 / 4.99 / 11.90 (9.98–21.25) | 4.74 / 5.31 / 13.57 (11.65–22.27) | 4.42 / 4.99 / 12.03 (9.47–20.22) | 5.12 / 6.40 / 16.38 (13.31–23.04) | 4.42 / 5.06 / 13.06 (11.52–14.08) | 49% | 15 |
| 1000 | 1000 | 2 | 5.57 / 5.76 / 9.60 (6.27–14.34) | 5.57 / 5.76 / 13.31 (10.62–16.13) | 5.70 / 5.89 / 14.59 (11.01–14.72) | 5.70 / 5.89 / 13.95 (9.34–16.38) | 5.70 / 5.89 / 13.18 (12.03–14.34) | 5.70 / 5.89 / 13.95 (10.62–17.15) | 5.70 / 5.89 / 14.46 (9.47–15.49) | 5.70 / 5.89 / 13.95 (11.01–16.26) | 100% | 0 |
| 1000 | 1000 | 4 | 5.63 / 5.76 / 14.21 (10.11–98.30) | 5.57 / 5.95 / 15.62 (10.50–19.71) | 5.70 / 6.21 / 17.15 (13.06–17.15) | 5.70 / 5.89 / 12.80 (7.74–19.46) | 5.89 / 6.66 / 14.85 (8.06–22.02) | 5.70 / 5.89 / 12.03 (9.86–17.92) | 5.89 / 6.21 / 14.59 (9.60–21.50) | 5.70 / 5.95 / 14.21 (11.65–16.90) | 99% | 14 |
| 1000 | 1000 | 6 | 5.76 / 6.27 / 13.70 (10.75–14.46) | 5.70 / 6.27 / 12.16 (7.30–13.44) | 5.89 / 6.78 / 13.95 (13.31–15.36) | 6.08 / 6.66 / 12.42 (11.78–13.70) | 6.21 / 6.91 / 12.80 (12.16–14.46) | 5.95 / 6.72 / 13.06 (12.54–14.34) | 7.17 / 8.19 / 15.87 (14.21–17.15) | 5.82 / 6.78 / 12.29 (10.62–13.82) | 66% | 27 |
| 1000 | 1000 | 8 | 5.70 / 6.27 / 14.08 (13.44–14.34) | 5.70 / 6.21 / 13.44 (11.52–14.21) | 6.08 / 6.78 / 14.21 (13.70–14.21) | 6.08 / 6.53 / 14.21 (12.29–22.27) | 6.34 / 6.85 / 15.49 (13.70–19.20) | 6.08 / 6.72 / 14.08 (12.93–16.26) | 10.24 / 11.78 / 23.04 (19.20–28.67) | 6.14 / 6.78 / 12.93 (11.52–16.00) | 50% | 12 |

Migrations seen: 65947 over 75235326 samples (one sample per measured operation).


### Suite a3t: k-1 threads signal one notify, lock per notify

load average before: { 5.45 5.32 4.83 }
load average after: { 5.40 5.35 4.94 }; 173 s

Throughput, operations/s, median (min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 701k (692k–705k) | 692k (688k–703k) | 659k (648k–663k) | 652k (645k–661k) | 661k (655k–665k) | 655k (639k–658k) | 655k (653k–665k) | 656k (649k–661k) | 99% | 1 |
| 0 | 50 | 4 | 2.09M (2.08M–2.10M) | 2.09M (2.07M–2.11M) | 1.87M (1.84M–1.88M) | 1.86M (1.72M–1.87M) | 1.92M (1.77M–1.98M) | 1.92M (1.91M–1.95M) | 1.95M (1.90M–1.95M) | 1.93M (1.91M–1.94M) | 98% | 1 |
| 0 | 50 | 6 | 3.37M (3.37M–3.39M) | 3.40M (3.36M–3.41M) | 2.71M (2.66M–2.72M) | 2.56M (2.50M–2.57M) | 2.34M (2.31M–2.44M) | 2.46M (2.44M–2.55M) | 2.41M (2.31M–2.49M) | 2.30M (2.29M–2.58M) | 67% | 24 |
| 0 | 50 | 8 | 4.72M (4.63M–4.75M) | 4.71M (4.55M–4.75M) | 3.55M (2.94M–3.65M) | 2.46M (2.10M–3.03M) | 2.71M (2.52M–2.73M) | 2.49M (2.34M–2.59M) | 2.55M (2.38M–2.76M) | 2.84M (2.56M–2.97M) | 49% | 293 |
| 0 | 200 | 2 | 663k (640k–663k) | 657k (643k–663k) | 568k (556k–578k) | 565k (561k–568k) | 569k (562k–578k) | 562k (557k–568k) | 569k (533k–577k) | 565k (562k–568k) | 100% | 1 |
| 0 | 200 | 4 | 1.95M (1.93M–1.96M) | 1.94M (1.93M–1.95M) | 1.65M (1.63M–1.67M) | 1.63M (1.62M–1.64M) | 1.59M (1.58M–1.61M) | 1.66M (1.65M–1.68M) | 1.59M (1.58M–1.61M) | 1.66M (1.63M–1.66M) | 100% | 0 |
| 0 | 200 | 6 | 3.17M (3.12M–3.19M) | 3.15M (3.14M–3.19M) | 2.31M (2.12M–2.42M) | 2.06M (2.02M–2.19M) | 1.67M (1.54M–1.75M) | 1.98M (1.95M–1.99M) | 1.54M (1.52M–1.56M) | 1.78M (1.76M–1.97M) | 66% | 23 |
| 0 | 200 | 8 | 4.42M (4.41M–4.47M) | 4.45M (4.44M–4.48M) | 2.89M (1.99M–3.32M) | 1.75M (1.57M–2.34M) | 2.03M (1.78M–2.33M) | 1.96M (1.90M–2.05M) | 1.52M (1.48M–1.65M) | 1.97M (1.94M–1.99M) | 49% | 270 |
| 0 | 1000 | 2 | 520k (515k–526k) | 523k (516k–525k) | 395k (392k–397k) | 393k (390k–397k) | 397k (392k–400k) | 392k (389k–395k) | 398k (396k–400k) | 393k (390k–397k) | 100% | 0 |
| 0 | 1000 | 4 | 1.55M (1.55M–1.56M) | 1.56M (1.55M–1.56M) | 1.07M (1.06M–1.08M) | 1.12M (1.11M–1.12M) | 667k (664k–674k) | 898k (892k–915k) | 671k (665k–672k) | 904k (863k–926k) | 99% | 0 |
| 0 | 1000 | 6 | 1.88M (1.86M–1.88M) | 1.88M (1.86M–1.89M) | 1.20M (1.11M–1.30M) | 1.13M (1.05M–1.19M) | 1.05M (962k–1.08M) | 1.00M (958k–1.08M) | 653k (642k–661k) | 966k (954k–1.04M) | 65% | 24 |
| 0 | 1000 | 8 | 1.87M (1.86M–1.88M) | 1.87M (1.86M–1.88M) | 1.04M (1.01M–1.23M) | 1.17M (931k–1.23M) | 1.01M (1.00M–1.22M) | 1.03M (921k–1.15M) | 667k (650k–712k) | 1.07M (1.03M–1.12M) | 50% | 93 |
| 1000 | 50 | 2 | 698k (685k–703k) | 697k (687k–706k) | 659k (652k–661k) | 658k (639k–660k) | 662k (652k–663k) | 656k (645k–660k) | 663k (654k–667k) | 660k (648k–661k) | 100% | 0 |
| 1000 | 50 | 4 | 2.09M (2.07M–2.10M) | 955k (951k–958k) | 1.44M (1.44M–1.45M) | 1.44M (1.43M–1.44M) | 1.95M (1.93M–1.98M) | 1.94M (1.92M–1.97M) | 1.95M (1.90M–1.99M) | 1.93M (1.92M–1.95M) | 100% | 1509 |
| 1000 | 50 | 6 | 3.37M (3.36M–3.41M) | 938k (936k–941k) | 2.52M (2.31M–2.53M) | 2.50M (2.26M–2.58M) | 1.74M (1.73M–1.74M) | 1.73M (1.72M–1.73M) | 2.83M (2.72M–2.87M) | 2.76M (2.66M–2.88M) | 67% | 1474 |
| 1000 | 50 | 8 | 4.72M (4.62M–4.74M) | 932k (930k–933k) | 3.32M (2.15M–3.41M) | 3.01M (2.25M–3.40M) | 2.69M (2.63M–2.81M) | 2.52M (2.46M–2.64M) | 2.75M (2.68M–3.08M) | 3.13M (2.73M–3.49M) | 49% | 1642 |
| 1000 | 200 | 2 | 652k (649k–658k) | 658k (645k–668k) | 572k (560k–574k) | 568k (555k–569k) | 569k (559k–576k) | 564k (554k–568k) | 569k (565k–572k) | 564k (560k–566k) | 100% | 0 |
| 1000 | 200 | 4 | 1.94M (1.93M–1.95M) | 869k (866k–869k) | 1.37M (1.35M–1.38M) | 1.35M (1.34M–1.36M) | 1.63M (1.62M–1.64M) | 1.66M (1.51M–1.67M) | 1.63M (1.61M–1.64M) | 1.67M (1.66M–1.68M) | 99% | 1282 |
| 1000 | 200 | 6 | 3.16M (3.14M–3.18M) | 857k (854k–860k) | 2.03M (1.85M–2.32M) | 2.10M (1.69M–2.15M) | 1.58M (1.56M–1.59M) | 1.63M (1.63M–1.65M) | 1.69M (1.66M–1.75M) | 2.26M (2.13M–2.36M) | 66% | 1282 |
| 1000 | 200 | 8 | 4.45M (4.35M–4.47M) | 847k (840k–849k) | 2.80M (2.51M–2.96M) | 2.23M (1.48M–2.73M) | 2.18M (1.71M–2.22M) | 2.08M (1.57M–2.19M) | 1.70M (1.52M–1.78M) | 2.16M (1.71M–2.31M) | 49% | 1495 |
| 1000 | 1000 | 2 | 525k (518k–526k) | 527k (520k–530k) | 386k (374k–391k) | 396k (393k–397k) | 376k (373k–379k) | 395k (393k–396k) | 380k (369k–395k) | 394k (391k–398k) | 100% | 0 |
| 1000 | 1000 | 4 | 1.56M (1.55M–1.57M) | 652k (649k–655k) | 1.05M (1.04M–1.07M) | 1.03M (1.01M–1.03M) | 717k (694k–724k) | 1.05M (1.04M–1.06M) | 722k (716k–727k) | 1.05M (1.03M–1.06M) | 99% | 756 |
| 1000 | 1000 | 6 | 1.87M (1.85M–1.88M) | 648k (647k–651k) | 1.20M (1.15M–1.30M) | 1.15M (1.06M–1.31M) | 1.03M (1.00M–1.05M) | 1.03M (998k–1.05M) | 679k (660k–689k) | 1.09M (1.08M–1.11M) | 65% | 790 |
| 1000 | 1000 | 8 | 1.86M (1.58M–1.87M) | 642k (631k–643k) | 1.23M (1.06M–1.25M) | 1.12M (1.11M–1.32M) | 1.18M (1.06M–1.20M) | 1.11M (1.05M–1.17M) | 701k (691k–708k) | 1.12M (1.12M–1.15M) | 49% | 833 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| ipi ns | W ns | k | 1 batched | 1 irq/msg | 2/2 group lock | 2/2 fine | 2/4 group lock | 2/4 fine | 3 group lock | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.37) | 0.08 / 0.12 / 0.17 (0.12–0.37) | 0.17 / 0.25 / 0.29 (0.29–4.42) | 0.21 / 0.25 / 0.33 (0.29–3.52) | 0.17 / 0.25 / 0.29 (0.29–0.83) | 0.21 / 0.25 / 0.29 (0.29–0.50) | 0.17 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 99% | 1 |
| 0 | 50 | 4 | 0.08 / 0.12 / 0.25 (0.17–0.37) | 0.08 / 0.12 / 0.17 (0.17–0.33) | 0.21 / 0.37 / 0.50 (0.42–0.70) | 0.21 / 0.33 / 0.50 (0.42–0.70) | 0.21 / 0.37 / 0.66 (0.37–7.23) | 0.21 / 0.37 / 0.66 (0.37–1.15) | 0.17 / 0.37 / 0.58 (0.42–1.00) | 0.21 / 0.37 / 0.62 (0.42–0.95) | 98% | 1 |
| 0 | 50 | 6 | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.33 / 0.58 / 0.70 (0.70–0.74) | 0.29 / 0.54 / 7.68 (5.44–16.00) | 0.42 / 0.95 / 1.15 (1.12–1.20) | 0.37 / 0.74 / 0.91 (0.87–0.91) | 0.74 / 1.15 / 3.07 (1.66–9.86) | 0.83 / 1.28 / 1.66 (1.46–5.57) | 67% | 24 |
| 0 | 50 | 8 | 0.17 / 0.33 / 0.37 (0.37–0.42) | 0.17 / 0.33 / 0.37 (0.37–0.37) | 0.29 / 0.62 / 54.78 (46.08–74.75) | 0.42 / 1.20 / 1.62 (1.46–15.87) | 0.42 / 1.12 / 1.49 (1.49–5.82) | 0.42 / 0.70 / 0.83 (0.79–5.18) | 1.36 / 2.27 / 8.00 (6.91–9.98) | 1.15 / 1.90 / 6.40 (4.99–8.58) | 49% | 293 |
| 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.42 / 0.50 / 0.54 (0.50–0.54) | 0.46 / 0.46 / 0.50 (0.50–0.50) | 0.42 / 0.50 / 0.54 (0.50–5.70) | 0.46 / 0.46 / 0.50 (0.50–4.99) | 0.42 / 0.50 / 0.54 (0.50–5.06) | 0.46 / 0.46 / 0.50 (0.50–0.87) | 100% | 1 |
| 0 | 200 | 4 | 0.21 / 0.25 / 0.29 (0.25–0.46) | 0.21 / 0.25 / 0.33 (0.25–0.42) | 0.33 / 0.62 / 0.66 (0.66–0.95) | 0.33 / 0.58 / 0.62 (0.62–0.83) | 0.50 / 0.74 / 1.36 (0.70–6.66) | 0.46 / 0.58 / 0.66 (0.62–2.27) | 0.50 / 0.66 / 0.74 (0.74–7.04) | 0.46 / 0.58 / 1.07 (0.62–6.08) | 100% | 0 |
| 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.50) | 0.25 / 0.33 / 0.42 (0.37–0.42) | 0.42 / 0.70 / 12.03 (8.45–15.49) | 0.62 / 1.20 / 1.62 (1.28–3.26) | 0.58 / 1.79 / 2.11 (1.95–2.27) | 0.54 / 1.15 / 1.41 (1.36–1.46) | 1.87 / 2.66 / 4.16 (2.85–10.37) | 1.49 / 1.82 / 5.31 (2.37–7.81) | 66% | 23 |
| 0 | 200 | 8 | 0.29 / 0.42 / 0.50 (0.46–0.54) | 0.25 / 0.42 / 0.50 (0.46–0.50) | 0.46 / 1.00 / 8.58 (5.95–14.59) | 1.36 / 1.70 / 6.34 (2.37–18.18) | 0.62 / 1.49 / 2.11 (1.46–5.50) | 0.74 / 1.20 / 3.36 (1.49–5.50) | 3.23 / 4.10 / 11.39 (9.47–14.72) | 2.24 / 2.91 / 9.98 (8.45–11.78) | 49% | 270 |
| 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.66) | 0.58 / 0.58 / 0.62 (0.62–0.66) | 1.15 / 1.25 / 1.33 (1.28–5.70) | 1.20 / 1.25 / 1.28 (1.28–5.57) | 1.15 / 1.25 / 1.28 (1.28–7.36) | 1.20 / 1.25 / 1.33 (1.25–7.42) | 1.15 / 1.25 / 1.33 (1.25–5.38) | 1.20 / 1.25 / 1.33 (1.25–5.82) | 100% | 0 |
| 0 | 1000 | 4 | 0.58 / 0.62 / 0.70 (0.62–0.91) | 0.58 / 0.62 / 0.74 (0.66–0.91) | 0.66 / 1.70 / 1.78 (1.74–1.90) | 0.70 / 1.66 / 1.74 (1.70–1.79) | 3.23 / 3.62 / 8.19 (3.52–16.64) | 1.79 / 2.37 / 7.62 (2.37–10.88) | 3.23 / 3.52 / 8.58 (7.23–16.64) | 1.79 / 2.30 / 2.37 (2.37–11.52) | 99% | 0 |
| 0 | 1000 | 6 | 0.66 / 0.83 / 137.22 (135.17–149.50) | 0.66 / 0.79 / 137.22 (137.22–147.46) | 2.00 / 2.82 / 4.06 (3.62–10.24) | 2.03 / 3.26 / 7.74 (3.49–10.50) | 0.83 / 3.78 / 4.48 (4.42–5.06) | 0.91 / 2.62 / 3.20 (3.10–3.36) | 6.21 / 7.30 / 13.82 (13.44–20.99) | 3.90 / 4.48 / 8.96 (8.00–14.34) | 65% | 24 |
| 0 | 1000 | 8 | 0.62 / 0.79 / 339.97 (339.97–344.06) | 0.62 / 0.79 / 339.97 (339.97–344.06) | 2.62 / 2.82 / 6.66 (5.31–15.36) | 2.30 / 3.46 / 10.24 (6.08–14.08) | 2.69 / 3.04 / 9.34 (8.96–11.39) | 2.50 / 3.36 / 11.39 (8.96–33.28) | 8.83 / 10.50 / 24.83 (21.50–29.70) | 5.06 / 6.53 / 16.13 (12.67–31.23) | 50% | 93 |
| 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.17 / 0.25 / 0.29 (0.29–0.66) | 0.21 / 0.25 / 0.33 (0.29–0.54) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.17 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 100% | 0 |
| 1000 | 50 | 4 | 0.08 / 0.12 / 0.21 (0.17–0.37) | 0.08 / 0.17 / 819.20 (794.62–843.78) | 0.21 / 0.33 / 0.42 (0.37–0.58) | 0.21 / 0.33 / 0.37 (0.37–0.54) | 0.17 / 0.37 / 0.74 (0.42–1.04) | 0.21 / 0.37 / 0.46 (0.42–0.95) | 0.17 / 0.37 / 0.78 (0.46–1.00) | 0.21 / 0.37 / 0.54 (0.42–0.83) | 100% | 1509 |
| 1000 | 50 | 6 | 0.17 / 0.29 / 0.33 (0.33–0.33) | 0.12 / 0.29 / 1818.62 (1736.70–1900.54) | 0.33 / 0.58 / 0.66 (0.66–1.49) | 0.33 / 0.54 / 0.66 (0.62–3.55) | 0.33 / 0.74 / 1.04 (0.95–1.04) | 0.33 / 0.66 / 0.91 (0.91–0.95) | 0.42 / 0.91 / 1.33 (1.04–2.21) | 0.46 / 0.95 / 1.28 (1.15–4.16) | 67% | 1474 |
| 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.46) | 0.12 / 0.25 / 3014.66 (2850.82–3145.73) | 0.29 / 0.62 / 59.39 (4.99–59.90) | 0.42 / 0.83 / 1.28 (0.74–51.20) | 0.42 / 0.83 / 1.25 (1.15–1.28) | 0.42 / 0.70 / 1.04 (0.87–1.49) | 1.15 / 2.11 / 6.21 (2.98–11.14) | 0.87 / 1.79 / 5.82 (2.27–10.24) | 49% | 1642 |
| 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.33) | 0.42 / 0.50 / 0.58 (0.54–0.74) | 0.46 / 0.46 / 0.58 (0.50–0.66) | 0.42 / 0.50 / 0.58 (0.54–0.87) | 0.46 / 0.46 / 0.54 (0.50–0.58) | 0.42 / 0.50 / 0.54 (0.54–0.62) | 0.46 / 0.46 / 0.54 (0.50–0.66) | 100% | 0 |
| 1000 | 200 | 4 | 0.21 / 0.29 / 0.33 (0.25–0.42) | 0.17 / 0.25 / 999.42 (991.23–1015.81) | 0.29 / 0.58 / 0.66 (0.66–0.87) | 0.33 / 0.54 / 0.66 (0.62–0.83) | 0.42 / 0.70 / 1.04 (0.74–5.31) | 0.42 / 0.70 / 1.00 (0.70–4.10) | 0.46 / 0.70 / 1.07 (0.79–5.12) | 0.42 / 0.62 / 0.74 (0.70–2.69) | 99% | 1282 |
| 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.21 / 0.33 / 2228.22 (2129.92–2228.22) | 0.54 / 1.20 / 11.14 (1.46–19.71) | 0.54 / 1.00 / 2.03 (1.66–5.12) | 0.50 / 1.46 / 1.87 (1.87–1.90) | 0.50 / 1.12 / 1.49 (1.46–1.54) | 1.49 / 2.27 / 8.06 (2.82–16.26) | 0.83 / 1.49 / 1.82 (1.79–5.82) | 66% | 1282 |
| 1000 | 200 | 8 | 0.25 / 0.42 / 0.50 (0.46–0.54) | 0.21 / 0.33 / 3670.02 (3571.71–3866.62) | 0.50 / 0.91 / 11.01 (5.50–22.27) | 0.62 / 1.66 / 24.83 (3.39–45.06) | 0.54 / 1.20 / 4.48 (2.24–5.76) | 0.54 / 1.12 / 3.68 (1.54–6.14) | 2.72 / 3.74 / 13.70 (10.88–16.00) | 1.70 / 2.50 / 9.86 (8.19–70.66) | 49% | 1495 |
| 1000 | 1000 | 2 | 0.58 / 0.62 / 0.66 (0.62–0.70) | 0.58 / 0.62 / 0.62 (0.62–0.66) | 1.15 / 1.62 / 1.66 (1.66–4.74) | 1.20 / 1.25 / 1.46 (1.28–2.94) | 1.33 / 1.62 / 1.66 (1.66–5.18) | 1.20 / 1.25 / 1.58 (1.28–3.23) | 1.15 / 1.62 / 1.66 (1.66–5.31) | 1.20 / 1.25 / 1.70 (1.28–4.74) | 100% | 0 |
| 1000 | 1000 | 4 | 0.58 / 0.62 / 0.74 (0.62–1.25) | 0.54 / 0.62 / 1736.70 (1703.94–1753.09) | 0.66 / 1.70 / 1.90 (1.79–5.89) | 0.70 / 1.28 / 1.74 (1.70–5.38) | 2.78 / 3.26 / 15.62 (3.49–19.20) | 1.57 / 2.03 / 8.96 (1.95–12.80) | 2.72 / 2.91 / 12.03 (2.98–16.90) | 1.58 / 2.21 / 8.70 (2.08–15.62) | 99% | 756 |
| 1000 | 1000 | 6 | 0.62 / 0.78 / 141.31 (139.26–143.36) | 0.58 / 0.74 / 3735.55 (3670.02–3801.09) | 1.74 / 2.82 / 8.32 (4.48–9.47) | 1.95 / 3.26 / 9.22 (2.69–10.24) | 0.83 / 3.49 / 4.48 (4.29–8.45) | 0.87 / 2.56 / 3.23 (3.20–7.17) | 5.82 / 6.78 / 16.38 (12.54–24.58) | 3.10 / 4.22 / 10.24 (6.53–16.38) | 65% | 790 |
| 1000 | 1000 | 8 | 0.62 / 0.78 / 344.06 (339.97–372.74) | 0.58 / 0.74 / 6094.85 (5898.24–6225.92) | 2.08 / 2.82 / 7.94 (7.74–20.22) | 2.00 / 3.39 / 8.58 (5.70–23.04) | 2.03 / 2.91 / 8.96 (7.10–24.06) | 2.24 / 2.94 / 9.34 (8.58–24.06) | 8.32 / 12.29 / 27.39 (17.66–126.98) | 4.86 / 5.70 / 14.59 (12.16–27.14) | 49% | 833 |

Migrations seen: 107275 over 227369993 samples (one sample per measured operation).


### Suite ct: k-thread process beside single-thread ones, fine group locks

load average before: { 5.40 5.35 4.94 }
load average after: { 7.33 6.09 5.27 }; 115 s

Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| n | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.25 / 0.33 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.25 (0.25–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 99% | 0 |
| 4 | 200 | 2 | 0.21 / 0.21 / 0.25 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.21–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 99% | 5 |
| 4 | 200 | 3 | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.21 / 0.29 (0.25–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.46) | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.21 / 0.21 / 0.29 (0.25–0.46) | 0.21 / 0.25 / 0.29 (0.25–0.50) | 0.21 / 0.21 / 0.25 (0.25–0.42) | 0.21 / 0.21 / 0.29 (0.25–0.33) | 99% | 0 |
| 4 | 1000 | 1 | 1.00 / 1.04 / 1.12 (1.07–1.20) | 1.00 / 1.04 / 1.15 (1.04–6.14) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.00 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.04 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.04 (1.04–1.25) | 1.00 / 1.04 / 1.07 (1.04–1.25) | 99% | 0 |
| 4 | 1000 | 2 | 1.00 / 1.00 / 1.12 (1.04–1.25) | 1.00 / 1.00 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.00 / 1.07 (1.04–1.57) | 1.00 / 1.04 / 1.12 (1.04–8.06) | 1.00 / 1.04 / 1.12 (1.04–6.59) | 1.00 / 1.04 / 1.12 (1.07–1.20) | 1.00 / 1.04 / 1.15 (1.04–6.91) | 98% | 8 |
| 4 | 1000 | 3 | 1.00 / 1.04 / 1.12 (1.04–5.25) | 1.00 / 1.00 / 1.07 (1.04–1.25) | 1.00 / 1.04 / 1.04 (1.04–1.20) | 1.00 / 1.04 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.07 (1.04–1.20) | 1.00 / 1.00 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.07 (1.04–1.25) | 1.00 / 1.00 / 1.04 (1.04–1.25) | 99% | 0 |
| 8 | 200 | 1 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 49% | 15 |
| 8 | 200 | 2 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 0 |
| 8 | 200 | 3 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.37) | 49% | 9 |
| 8 | 200 | 5 | 0.21 / 0.29 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 14 |
| 8 | 200 | 6 | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 0.21 / 0.29 / 0.29 (0.29–0.29) | 49% | 12 |
| 8 | 1000 | 1 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 49% | 8 |
| 8 | 1000 | 2 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.33) | 49% | 8 |
| 8 | 1000 | 3 | 1.00 / 1.07 / 1.07 (1.07–11.14) | 1.00 / 1.07 / 1.07 (1.07–13.95) | 1.00 / 1.07 / 1.07 (1.07–1.20) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 49% | 138 |
| 8 | 1000 | 5 | 1.00 / 1.07 / 1.07 (1.07–5.18) | 1.00 / 1.07 / 1.07 (1.07–7.04) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–8.19) | 1.00 / 1.07 / 1.12 (1.07–8.32) | 1.00 / 1.07 / 1.12 (1.12–7.62) | 1.00 / 1.07 / 1.07 (1.07–5.95) | 1.00 / 1.07 / 1.07 (1.07–9.22) | 48% | 125 |
| 8 | 1000 | 6 | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.66) | 1.00 / 1.07 / 1.07 (1.07–1.15) | 1.00 / 1.07 / 1.12 (1.07–10.11) | 1.00 / 1.07 / 1.12 (1.12–6.91) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 1.00 / 1.07 / 1.07 (1.07–1.12) | 48% | 59 |

The k-thread process: operations/s

| n | W ns | k | 1 first | 1 fair | 2/2 fair | 2/2 fine | 2/4 fair | 2/4 fine | 3 fair | 3 fine | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 4 | 200 | 1 | 634k (628k–647k) | 630k (626k–635k) | 632k (627k–645k) | 636k (628k–647k) | 630k (629k–642k) | 634k (628k–647k) | 633k (628k–646k) | 630k (628k–636k) | 99% | 0 |
| 4 | 200 | 2 | 1.06M (1.05M–1.07M) | 1.10M (1.09M–1.11M) | 1.25M (1.24M–1.26M) | 1.25M (1.15M–1.27M) | 1.25M (1.24M–1.27M) | 1.25M (1.24M–1.25M) | 1.25M (1.24M–1.26M) | 1.26M (1.24M–1.27M) | 99% | 5 |
| 4 | 200 | 3 | 1.45M (1.44M–1.46M) | 1.51M (1.50M–1.51M) | 1.59M (1.57M–1.60M) | 1.60M (1.58M–1.62M) | 1.86M (1.84M–1.88M) | 1.86M (1.85M–1.89M) | 1.86M (1.85M–1.88M) | 1.88M (1.86M–1.88M) | 99% | 0 |
| 4 | 1000 | 1 | 420k (419k–424k) | 420k (419k–424k) | 421k (419k–427k) | 422k (418k–427k) | 420k (418k–426k) | 422k (419k–424k) | 424k (418k–427k) | 420k (419k–427k) | 99% | 0 |
| 4 | 1000 | 2 | 703k (700k–709k) | 740k (739k–746k) | 829k (823k–832k) | 833k (830k–842k) | 822k (815k–832k) | 833k (828k–836k) | 828k (779k–834k) | 833k (822k–841k) | 98% | 8 |
| 4 | 1000 | 3 | 963k (949k–965k) | 1.04M (1.03M–1.04M) | 1.09M (1.08M–1.09M) | 1.11M (1.10M–1.12M) | 1.20M (1.20M–1.21M) | 1.25M (1.24M–1.26M) | 1.20M (1.20M–1.21M) | 1.25M (1.24M–1.26M) | 99% | 0 |
| 8 | 200 | 1 | 640k (624k–643k) | 635k (631k–644k) | 637k (636k–639k) | 639k (637k–645k) | 637k (623k–643k) | 635k (635k–644k) | 638k (578k–642k) | 640k (627k–643k) | 49% | 15 |
| 8 | 200 | 2 | 1.05M (1.04M–1.05M) | 1.09M (1.08M–1.11M) | 1.25M (1.24M–1.26M) | 1.25M (1.24M–1.26M) | 1.25M (1.23M–1.26M) | 1.26M (1.24M–1.27M) | 1.25M (1.24M–1.26M) | 1.25M (1.24M–1.25M) | 49% | 0 |
| 8 | 200 | 3 | 1.42M (1.41M–1.44M) | 1.48M (1.46M–1.50M) | 1.57M (1.54M–1.57M) | 1.57M (1.54M–1.63M) | 1.85M (1.84M–1.87M) | 1.86M (1.86M–1.87M) | 1.85M (1.83M–1.88M) | 1.86M (1.85M–1.88M) | 49% | 9 |
| 8 | 200 | 5 | 2.09M (2.05M–2.11M) | 2.20M (2.15M–2.21M) | 2.26M (2.19M–2.28M) | 2.24M (2.18M–2.28M) | 2.54M (2.51M–2.55M) | 2.58M (2.58M–2.60M) | 3.03M (3.03M–3.06M) | 3.10M (3.08M–3.11M) | 49% | 14 |
| 8 | 200 | 6 | 2.39M (2.32M–2.40M) | 2.49M (2.49M–2.56M) | 2.58M (2.49M–2.63M) | 2.63M (2.51M–2.64M) | 2.86M (2.80M–2.88M) | 2.89M (2.86M–2.92M) | 3.60M (3.59M–3.62M) | 3.71M (3.69M–3.73M) | 49% | 12 |
| 8 | 1000 | 1 | 431k (427k–432k) | 429k (428k–432k) | 430k (429k–433k) | 429k (423k–434k) | 430k (423k–433k) | 431k (427k–433k) | 430k (429k–432k) | 431k (430k–433k) | 49% | 8 |
| 8 | 1000 | 2 | 702k (698k–718k) | 745k (741k–757k) | 827k (816k–834k) | 847k (845k–853k) | 829k (819k–847k) | 850k (845k–852k) | 827k (812k–830k) | 846k (840k–852k) | 49% | 8 |
| 8 | 1000 | 3 | 945k (655k–973k) | 1.04M (712k–1.04M) | 1.08M (1.06M–1.10M) | 1.10M (1.10M–1.11M) | 1.19M (1.18M–1.23M) | 1.26M (1.26M–1.27M) | 1.19M (1.18M–1.21M) | 1.28M (1.26M–1.28M) | 49% | 138 |
| 8 | 1000 | 5 | 1.37M (1.34M–1.38M) | 1.53M (1.27M–1.54M) | 1.58M (1.55M–1.59M) | 1.60M (1.58M–1.63M) | 1.66M (1.59M–1.66M) | 1.80M (1.75M–1.82M) | 1.80M (1.80M–1.83M) | 2.09M (1.98M–2.10M) | 48% | 125 |
| 8 | 1000 | 6 | 1.55M (1.53M–1.57M) | 1.75M (1.73M–1.76M) | 1.83M (1.82M–1.84M) | 1.87M (1.83M–1.90M) | 1.90M (1.82M–1.91M) | 2.04M (2.00M–2.07M) | 2.09M (2.06M–2.10M) | 2.50M (2.49M–2.50M) | 48% | 59 |

Seqlock reads repeated per 1000 lookups: 2/2 fair: 8.75; 2/2 fine: 0.07; 2/4 fair: 21.51; 2/4 fine: 0.12; 3 fair: 32.61; 3 fine: 0.10;

Migrations seen: 148221 over 314919477 samples (one sample per measured operation).


## Appendix G: full tables, fourth series, pass 1


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 19.0 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a3n: k-1 threads signal one notify, notify by one atomic OR

load average before: { 2.54 3.38 3.69 }
load average after: { 5.77 4.89 4.30 }; 269 s

Throughput, signals/s, median (min–max)

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 707k (702k–710k) | 707k (701k–708k) | 657k (656k–663k) | 645k (642k–649k) | 100% | 0 |
| as a3t | 0 | 50 | 4 | 2.09M (2.08M–2.10M) | 2.09M (2.09M–2.10M) | 1.93M (1.92M–1.94M) | 1.93M (1.92M–1.95M) | 100% | 0 |
| as a3t | 0 | 50 | 6 | 3.40M (3.39M–3.42M) | 3.39M (3.35M–3.40M) | 2.53M (2.43M–2.70M) | 3.06M (3.04M–3.08M) | 66% | 27 |
| as a3t | 0 | 50 | 8 | 4.68M (4.66M–4.70M) | 4.69M (4.66M–4.70M) | 2.85M (2.81M–3.10M) | 4.28M (4.21M–4.28M) | 50% | 46 |
| as a3t | 0 | 200 | 2 | 654k (652k–661k) | 653k (652k–658k) | 567k (565k–568k) | 602k (600k–605k) | 100% | 0 |
| as a3t | 0 | 200 | 4 | 1.94M (1.94M–1.94M) | 1.94M (1.89M–1.94M) | 1.66M (1.63M–1.67M) | 1.81M (1.79M–1.81M) | 99% | 1 |
| as a3t | 0 | 200 | 6 | 3.19M (3.18M–3.21M) | 3.19M (3.16M–3.21M) | 1.92M (1.78M–2.01M) | 2.88M (2.86M–2.88M) | 66% | 66 |
| as a3t | 0 | 200 | 8 | 4.42M (4.42M–4.44M) | 4.41M (4.40M–4.41M) | 2.15M (2.08M–2.20M) | 4.06M (4.03M–4.06M) | 49% | 24 |
| as a3t | 0 | 1000 | 2 | 525k (525k–528k) | 525k (524k–528k) | 397k (397k–399k) | 490k (490k–493k) | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 1.56M (1.55M–1.57M) | 1.56M (1.56M–1.56M) | 890k (882k–901k) | 1.46M (1.44M–1.48M) | 98% | 0 |
| as a3t | 0 | 1000 | 6 | 1.88M (1.87M–1.89M) | 2.57M (2.55M–2.60M) | 987k (974k–1.02M) | 2.39M (2.35M–2.40M) | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 1.87M (1.87M–1.88M) | 3.61M (3.57M–3.62M) | 1.07M (1.02M–1.08M) | 3.44M (3.44M–3.48M) | 49% | 36 |
| as a3t | 1000 | 50 | 2 | 706k (702k–711k) | 705k (699k–706k) | 660k (657k–661k) | 646k (642k–651k) | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 2.09M (2.09M–2.10M) | 2.09M (2.08M–2.10M) | 1.95M (1.93M–1.95M) | 1.93M (1.92M–1.94M) | 99% | 0 |
| as a3t | 1000 | 50 | 6 | 3.40M (3.38M–3.41M) | 3.39M (3.38M–3.42M) | 2.79M (2.71M–2.93M) | 3.13M (3.13M–3.15M) | 66% | 46 |
| as a3t | 1000 | 50 | 8 | 4.72M (4.70M–4.74M) | 4.71M (4.69M–4.73M) | 3.17M (2.89M–3.49M) | 4.44M (4.42M–4.48M) | 50% | 7 |
| as a3t | 1000 | 200 | 2 | 655k (654k–658k) | 653k (650k–654k) | 568k (565k–571k) | 603k (602k–603k) | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 1.94M (1.93M–1.94M) | 1.94M (1.94M–1.94M) | 1.67M (1.67M–1.68M) | 1.80M (1.80M–1.80M) | 100% | 0 |
| as a3t | 1000 | 200 | 6 | 3.17M (3.16M–3.19M) | 3.19M (3.14M–3.19M) | 2.26M (2.15M–2.34M) | 2.97M (2.94M–2.98M) | 66% | 32 |
| as a3t | 1000 | 200 | 8 | 4.42M (4.39M–4.43M) | 4.41M (4.39M–4.43M) | 2.24M (2.21M–2.37M) | 4.18M (4.13M–4.22M) | 49% | 45 |
| as a3t | 1000 | 1000 | 2 | 524k (524k–526k) | 525k (522k–527k) | 396k (394k–398k) | 492k (491k–494k) | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 1.56M (1.56M–1.56M) | 1.56M (1.55M–1.56M) | 1.06M (1.06M–1.06M) | 1.47M (1.47M–1.47M) | 100% | 0 |
| as a3t | 1000 | 1000 | 6 | 1.87M (1.85M–1.88M) | 2.58M (2.55M–2.59M) | 1.08M (1.05M–1.11M) | 2.43M (2.40M–2.43M) | 66% | 43 |
| as a3t | 1000 | 1000 | 8 | 1.87M (1.85M–1.87M) | 3.63M (3.62M–3.63M) | 1.12M (1.08M–1.15M) | 3.57M (3.56M–3.58M) | 49% | 40 |
| busy | 0 | 50 | 2 | 706k (703k–707k) | 705k (704k–707k) | 698k (696k–702k) | 720k (716k–722k) | 100% | 0 |
| busy | 0 | 50 | 4 | 2.09M (2.08M–2.10M) | 2.09M (2.07M–2.09M) | 1.94M (1.93M–1.95M) | 2.10M (2.08M–2.11M) | 99% | 0 |
| busy | 0 | 50 | 6 | 3.37M (3.35M–3.42M) | 3.38M (3.37M–3.40M) | 2.83M (2.80M–2.89M) | 3.41M (3.39M–3.43M) | 66% | 32 |
| busy | 0 | 50 | 8 | 4.68M (4.65M–4.71M) | 4.70M (4.67M–4.71M) | 3.49M (3.25M–3.88M) | 4.76M (4.74M–4.78M) | 50% | 21 |
| busy | 0 | 200 | 2 | 654k (648k–656k) | 653k (650k–654k) | 620k (617k–622k) | 681k (675k–682k) | 100% | 0 |
| busy | 0 | 200 | 4 | 1.94M (1.93M–1.94M) | 1.94M (1.93M–1.94M) | 1.67M (1.66M–1.68M) | 1.95M (1.93M–1.97M) | 99% | 0 |
| busy | 0 | 200 | 6 | 3.19M (3.17M–3.19M) | 3.18M (3.17M–3.19M) | 2.48M (2.40M–2.57M) | 3.19M (3.16M–3.19M) | 66% | 41 |
| busy | 0 | 200 | 8 | 4.42M (4.40M–4.43M) | 4.39M (4.36M–4.41M) | 2.52M (2.36M–2.67M) | 4.45M (4.45M–4.46M) | 49% | 40 |
| busy | 0 | 1000 | 2 | 524k (524k–527k) | 525k (522k–529k) | 420k (419k–421k) | 539k (538k–541k) | 100% | 0 |
| busy | 0 | 1000 | 4 | 1.56M (1.56M–1.56M) | 1.56M (1.55M–1.56M) | 1.16M (1.15M–1.17M) | 1.57M (1.56M–1.57M) | 100% | 1 |
| busy | 0 | 1000 | 6 | 1.88M (1.88M–1.89M) | 2.57M (2.54M–2.58M) | 1.25M (1.17M–1.26M) | 2.58M (2.57M–2.59M) | 66% | 40 |
| busy | 0 | 1000 | 8 | 1.87M (1.86M–1.89M) | 3.62M (3.60M–3.62M) | 1.21M (1.19M–1.23M) | 3.63M (3.59M–3.64M) | 49% | 62 |
| busy | 1000 | 50 | 2 | 703k (699k–708k) | 704k (701k–706k) | 700k (697k–702k) | 721k (720k–726k) | 100% | 0 |
| busy | 1000 | 50 | 4 | 2.09M (2.09M–2.09M) | 2.09M (2.07M–2.10M) | 1.95M (1.93M–1.96M) | 2.10M (2.08M–2.10M) | 100% | 0 |
| busy | 1000 | 50 | 6 | 3.40M (3.38M–3.41M) | 3.37M (3.37M–3.41M) | 2.89M (2.81M–2.92M) | 3.41M (3.38M–3.43M) | 65% | 13 |
| busy | 1000 | 50 | 8 | 4.71M (4.70M–4.73M) | 4.69M (4.62M–4.72M) | 3.58M (3.40M–3.66M) | 4.75M (4.74M–4.78M) | 50% | 20 |
| busy | 1000 | 200 | 2 | 652k (650k–657k) | 652k (650k–658k) | 619k (617k–622k) | 678k (676k–679k) | 100% | 0 |
| busy | 1000 | 200 | 4 | 1.94M (1.93M–1.94M) | 1.93M (1.93M–1.95M) | 1.67M (1.66M–1.68M) | 1.94M (1.93M–1.96M) | 99% | 0 |
| busy | 1000 | 200 | 6 | 3.17M (3.15M–3.18M) | 3.16M (3.15M–3.20M) | 2.48M (2.27M–2.53M) | 3.18M (3.16M–3.20M) | 66% | 48 |
| busy | 1000 | 200 | 8 | 4.41M (4.40M–4.42M) | 4.42M (4.40M–4.43M) | 2.56M (2.48M–2.64M) | 4.46M (4.45M–4.47M) | 50% | 2 |
| busy | 1000 | 1000 | 2 | 525k (524k–528k) | 525k (522k–528k) | 420k (419k–422k) | 541k (539k–543k) | 100% | 0 |
| busy | 1000 | 1000 | 4 | 1.56M (1.55M–1.56M) | 1.56M (1.55M–1.56M) | 1.17M (1.15M–1.18M) | 1.57M (1.54M–1.58M) | 99% | 1 |
| busy | 1000 | 1000 | 6 | 1.88M (1.86M–1.89M) | 2.56M (2.53M–2.58M) | 1.25M (1.18M–1.28M) | 2.57M (2.55M–2.58M) | 66% | 48 |
| busy | 1000 | 1000 | 8 | 1.87M (1.85M–1.88M) | 3.61M (3.59M–3.62M) | 1.23M (1.15M–1.30M) | 3.63M (3.61M–3.65M) | 49% | 26 |
| idle | 0 | 50 | 2 | 39k (38k–39k) | 39k (39k–39k) | 39k (38k–39k) | 39k (38k–39k) | 100% | 0 |
| idle | 0 | 50 | 4 | 115k (115k–116k) | 115k (115k–116k) | 115k (113k–116k) | 116k (113k–117k) | 99% | 0 |
| idle | 0 | 50 | 6 | 198k (197k–201k) | 198k (196k–199k) | 197k (195k–197k) | 196k (196k–197k) | 64% | 0 |
| idle | 0 | 50 | 8 | 281k (280k–283k) | 281k (279k–283k) | 279k (278k–279k) | 281k (279k–282k) | 47% | 0 |
| idle | 0 | 200 | 2 | 39k (38k–39k) | 39k (38k–39k) | 38k (38k–39k) | 38k (38k–39k) | 100% | 0 |
| idle | 0 | 200 | 4 | 115k (115k–116k) | 115k (114k–115k) | 113k (112k–114k) | 114k (113k–114k) | 100% | 0 |
| idle | 0 | 200 | 6 | 197k (195k–198k) | 198k (197k–199k) | 195k (193k–196k) | 195k (194k–197k) | 64% | 0 |
| idle | 0 | 200 | 8 | 281k (278k–282k) | 279k (275k–281k) | 276k (275k–277k) | 279k (278k–280k) | 48% | 9 |
| idle | 0 | 1000 | 2 | 38k (38k–39k) | 38k (38k–38k) | 37k (37k–37k) | 38k (38k–38k) | 100% | 0 |
| idle | 0 | 1000 | 4 | 114k (112k–114k) | 114k (113k–115k) | 110k (109k–111k) | 112k (111k–113k) | 99% | 0 |
| idle | 0 | 1000 | 6 | 195k (193k–196k) | 195k (194k–195k) | 188k (187k–189k) | 193k (193k–194k) | 64% | 1 |
| idle | 0 | 1000 | 8 | 275k (273k–277k) | 276k (274k–279k) | 266k (266k–269k) | 275k (274k–277k) | 48% | 1 |
| idle | 1000 | 50 | 2 | 39k (39k–39k) | 39k (39k–39k) | 38k (38k–39k) | 39k (38k–39k) | 100% | 0 |
| idle | 1000 | 50 | 4 | 115k (115k–116k) | 116k (115k–117k) | 115k (115k–116k) | 114k (113k–116k) | 99% | 0 |
| idle | 1000 | 50 | 6 | 198k (197k–199k) | 198k (197k–200k) | 197k (195k–198k) | 197k (196k–199k) | 64% | 0 |
| idle | 1000 | 50 | 8 | 282k (278k–283k) | 283k (280k–284k) | 279k (276k–280k) | 282k (280k–283k) | 47% | 0 |
| idle | 1000 | 200 | 2 | 39k (38k–39k) | 39k (38k–39k) | 38k (38k–39k) | 38k (38k–39k) | 100% | 0 |
| idle | 1000 | 200 | 4 | 115k (114k–116k) | 116k (113k–117k) | 113k (113k–116k) | 114k (113k–116k) | 99% | 0 |
| idle | 1000 | 200 | 6 | 198k (197k–199k) | 198k (196k–199k) | 195k (193k–196k) | 196k (193k–197k) | 64% | 0 |
| idle | 1000 | 200 | 8 | 279k (278k–281k) | 279k (248k–280k) | 276k (275k–278k) | 280k (277k–282k) | 47% | 3 |
| idle | 1000 | 1000 | 2 | 38k (38k–39k) | 38k (38k–38k) | 37k (37k–37k) | 38k (38k–38k) | 100% | 0 |
| idle | 1000 | 1000 | 4 | 113k (113k–114k) | 114k (113k–114k) | 111k (110k–113k) | 113k (111k–114k) | 99% | 0 |
| idle | 1000 | 1000 | 6 | 196k (194k–197k) | 195k (193k–196k) | 188k (168k–189k) | 193k (191k–194k) | 64% | 9 |
| idle | 1000 | 1000 | 8 | 276k (274k–276k) | 276k (275k–278k) | 267k (267k–269k) | 276k (274k–277k) | 47% | 0 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.33 (0.33–0.33) | 100% | 0 |
| as a3t | 0 | 50 | 4 | 0.08 / 0.12 / 0.29 (0.21–0.33) | 0.08 / 0.12 / 0.33 (0.25–0.33) | 0.21 / 0.33 / 0.79 (0.62–1.04) | 0.25 / 0.33 / 0.54 (0.37–0.58) | 100% | 0 |
| as a3t | 0 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.62 / 1.15 / 4.93 (2.03–5.12) | 0.25 / 0.70 / 0.83 (0.79–0.83) | 66% | 27 |
| as a3t | 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 1.07 / 1.78 / 8.70 (6.21–9.22) | 0.25 / 0.83 / 1.04 (1.04–1.12) | 50% | 46 |
| as a3t | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.46 / 0.50 / 0.54 (0.54–0.54) | 0.33 / 0.37 / 0.42 (0.37–0.46) | 100% | 0 |
| as a3t | 0 | 200 | 4 | 0.21 / 0.25 / 0.42 (0.33–0.42) | 0.21 / 0.25 / 0.37 (0.33–0.62) | 0.46 / 0.70 / 1.28 (0.87–8.70) | 0.33 / 0.46 / 0.62 (0.50–0.66) | 99% | 1 |
| as a3t | 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.25 / 0.37 / 0.42 (0.42–0.42) | 1.28 / 1.78 / 7.55 (6.66–8.19) | 0.37 / 0.79 / 0.91 (0.91–0.91) | 66% | 66 |
| as a3t | 0 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.82 / 2.69 / 10.62 (8.83–12.67) | 0.33 / 0.95 / 1.15 (1.12–1.15) | 49% | 24 |
| as a3t | 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.66) | 0.58 / 0.62 / 0.66 (0.62–0.66) | 1.15 / 1.20 / 1.33 (1.28–1.57) | 0.70 / 0.74 / 0.79 (0.79–0.83) | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 0.58 / 0.62 / 0.79 (0.74–0.79) | 0.58 / 0.70 / 0.83 (0.79–0.87) | 1.79 / 2.30 / 10.62 (9.60–12.93) | 0.70 / 0.83 / 1.07 (0.83–1.15) | 98% | 0 |
| as a3t | 0 | 1000 | 6 | 0.62 / 0.74 / 141.31 (139.26–143.36) | 0.62 / 0.74 / 0.83 (0.79–0.83) | 3.65 / 4.48 / 13.57 (12.54–14.98) | 0.74 / 1.15 / 1.28 (1.28–1.28) | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 0.62 / 0.74 / 344.06 (339.97–348.16) | 0.62 / 0.79 / 0.87 (0.87–0.95) | 5.12 / 6.66 / 18.69 (15.62–19.97) | 0.66 / 1.25 / 1.49 (1.46–1.49) | 49% | 36 |
| as a3t | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 0.08 / 0.12 / 0.29 (0.25–0.33) | 0.08 / 0.17 / 0.33 (0.25–0.33) | 0.21 / 0.37 / 0.58 (0.46–0.79) | 0.25 / 0.33 / 0.50 (0.46–0.70) | 99% | 0 |
| as a3t | 1000 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.17 / 0.25 / 0.33 (0.33–0.33) | 0.46 / 0.95 / 1.28 (1.12–2.00) | 0.21 / 0.74 / 0.83 (0.83–0.87) | 66% | 46 |
| as a3t | 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.33–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.87 / 1.66 / 6.66 (6.14–7.74) | 0.21 / 0.91 / 1.07 (1.07–1.12) | 50% | 7 |
| as a3t | 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.42 / 0.46 / 0.54 (0.50–0.54) | 0.33 / 0.37 / 0.46 (0.42–0.46) | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 0.21 / 0.25 / 0.37 (0.33–0.42) | 0.21 / 0.29 / 0.42 (0.37–0.42) | 0.42 / 0.62 / 1.00 (0.74–1.36) | 0.33 / 0.46 / 0.62 (0.50–0.70) | 100% | 0 |
| as a3t | 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.83 / 1.49 / 5.18 (2.21–5.31) | 0.29 / 0.83 / 0.95 (0.91–0.95) | 66% | 32 |
| as a3t | 1000 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.74 / 2.62 / 10.24 (8.70–10.24) | 0.29 / 1.00 / 1.20 (1.15–1.20) | 49% | 45 |
| as a3t | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.66) | 0.58 / 0.62 / 0.62 (0.62–0.70) | 1.15 / 1.25 / 1.49 (1.28–1.95) | 0.70 / 0.74 / 0.83 (0.79–0.95) | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 0.58 / 0.62 / 0.78 (0.74–0.83) | 0.58 / 0.62 / 0.79 (0.74–0.87) | 1.62 / 1.87 / 8.00 (7.81–9.34) | 0.70 / 0.83 / 1.00 (0.91–1.07) | 100% | 0 |
| as a3t | 1000 | 1000 | 6 | 0.62 / 0.74 / 141.31 (141.31–143.36) | 0.62 / 0.74 / 0.83 (0.79–0.83) | 3.20 / 4.29 / 12.03 (11.14–14.08) | 0.66 / 1.20 / 1.33 (1.33–1.33) | 66% | 43 |
| as a3t | 1000 | 1000 | 8 | 0.62 / 0.74 / 348.16 (348.16–348.16) | 0.62 / 0.79 / 0.87 (0.87–0.87) | 4.86 / 5.89 / 17.66 (16.00–22.53) | 0.62 / 1.15 / 1.49 (1.46–1.54) | 49% | 40 |
| busy | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.21 / 0.25 (0.25–0.25) | 0.04 / 0.08 / 0.12 (0.12–0.12) | 100% | 0 |
| busy | 0 | 50 | 4 | 0.08 / 0.12 / 0.29 (0.21–0.29) | 0.08 / 0.12 / 0.33 (0.25–0.37) | 0.21 / 0.33 / 0.58 (0.46–0.79) | 0.08 / 0.12 / 0.25 (0.25–0.25) | 99% | 0 |
| busy | 0 | 50 | 6 | 0.17 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.46 / 0.79 / 1.12 (1.00–1.15) | 0.12 / 0.25 / 0.29 (0.29–0.33) | 66% | 32 |
| busy | 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.58 / 1.36 / 2.69 (2.11–5.50) | 0.17 / 0.29 / 0.37 (0.33–0.37) | 50% | 21 |
| busy | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.12 / 0.21 / 0.25 (0.21–0.25) | 100% | 0 |
| busy | 0 | 200 | 4 | 0.21 / 0.25 / 0.37 (0.29–0.46) | 0.21 / 0.25 / 0.42 (0.37–0.46) | 0.42 / 0.62 / 0.91 (0.83–1.15) | 0.21 / 0.29 / 0.37 (0.29–0.37) | 99% | 0 |
| busy | 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.62 / 1.28 / 2.85 (1.33–5.25) | 0.25 / 0.37 / 0.42 (0.42–0.46) | 66% | 41 |
| busy | 0 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.36 / 2.30 / 8.32 (6.98–8.83) | 0.25 / 0.42 / 0.46 (0.46–0.46) | 49% | 40 |
| busy | 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.70) | 0.58 / 0.62 / 0.66 (0.62–0.70) | 1.00 / 1.49 / 1.70 (1.66–1.70) | 0.50 / 0.58 / 0.62 (0.62–0.62) | 100% | 0 |
| busy | 0 | 1000 | 4 | 0.58 / 0.62 / 0.78 (0.74–0.91) | 0.58 / 0.62 / 0.79 (0.79–0.91) | 1.15 / 1.78 / 3.49 (2.30–6.27) | 0.58 / 0.62 / 0.74 (0.70–0.79) | 100% | 1 |
| busy | 0 | 1000 | 6 | 0.66 / 0.79 / 139.26 (137.22–141.31) | 0.62 / 0.74 / 0.83 (0.79–0.83) | 2.53 / 3.68 / 10.50 (8.45–11.52) | 0.62 / 0.74 / 0.79 (0.79–0.83) | 66% | 40 |
| busy | 0 | 1000 | 8 | 0.62 / 0.79 / 339.97 (335.87–344.06) | 0.66 / 0.79 / 0.87 (0.87–0.87) | 4.29 / 5.82 / 15.23 (14.46–17.66) | 0.62 / 0.83 / 0.87 (0.87–0.91) | 49% | 62 |
| busy | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.04 / 0.08 / 0.12 (0.12–0.12) | 100% | 0 |
| busy | 1000 | 50 | 4 | 0.08 / 0.12 / 0.25 (0.25–0.29) | 0.08 / 0.12 / 0.33 (0.21–0.33) | 0.21 / 0.33 / 0.54 (0.46–0.70) | 0.08 / 0.12 / 0.25 (0.25–0.29) | 100% | 0 |
| busy | 1000 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.17 / 0.25 / 0.33 (0.33–0.33) | 0.42 / 0.74 / 1.04 (0.95–4.61) | 0.12 / 0.25 / 0.29 (0.29–0.33) | 65% | 13 |
| busy | 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.33–0.42) | 0.54 / 1.36 / 4.99 (1.62–5.63) | 0.17 / 0.29 / 0.37 (0.33–0.37) | 50% | 20 |
| busy | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.12 / 0.21 / 0.25 (0.21–0.25) | 100% | 0 |
| busy | 1000 | 200 | 4 | 0.21 / 0.25 / 0.33 (0.33–0.37) | 0.21 / 0.25 / 0.42 (0.37–0.42) | 0.42 / 0.58 / 1.00 (0.83–1.07) | 0.21 / 0.25 / 0.37 (0.33–0.42) | 99% | 0 |
| busy | 1000 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.62 / 1.28 / 2.30 (1.62–4.10) | 0.25 / 0.37 / 0.42 (0.42–0.46) | 66% | 48 |
| busy | 1000 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.33 / 2.30 / 8.58 (7.23–8.96) | 0.25 / 0.42 / 0.46 (0.46–0.46) | 50% | 2 |
| busy | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.70) | 0.58 / 0.62 / 0.66 (0.62–0.66) | 1.00 / 1.49 / 1.70 (1.66–1.70) | 0.50 / 0.58 / 0.62 (0.58–0.62) | 100% | 0 |
| busy | 1000 | 1000 | 4 | 0.58 / 0.62 / 0.79 (0.74–0.83) | 0.58 / 0.62 / 0.79 (0.70–0.95) | 1.15 / 1.70 / 4.74 (2.08–9.86) | 0.58 / 0.62 / 0.74 (0.70–0.95) | 99% | 1 |
| busy | 1000 | 1000 | 6 | 0.62 / 0.78 / 141.31 (139.26–145.41) | 0.62 / 0.74 / 0.87 (0.79–0.87) | 2.53 / 3.65 / 10.62 (9.73–14.59) | 0.62 / 0.74 / 0.83 (0.79–0.83) | 66% | 48 |
| busy | 1000 | 1000 | 8 | 0.62 / 0.74 / 344.06 (339.97–348.16) | 0.62 / 0.79 / 0.87 (0.87–0.91) | 4.22 / 5.76 / 15.62 (13.57–18.43) | 0.62 / 0.79 / 0.87 (0.87–0.91) | 49% | 26 |
| idle | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.33) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 100% | 0 |
| idle | 0 | 50 | 4 | 0.08 / 0.21 / 3.20 (3.20–3.26) | 0.08 / 0.17 / 3.20 (2.91–3.26) | 0.21 / 0.33 / 1.07 (0.70–2.56) | 0.21 / 0.33 / 1.00 (0.87–1.20) | 99% | 0 |
| idle | 0 | 50 | 6 | 0.12 / 0.29 / 0.42 (0.33–0.46) | 0.17 / 0.29 / 0.46 (0.42–0.46) | 0.33 / 0.70 / 0.95 (0.83–0.95) | 0.33 / 0.74 / 0.83 (0.79–0.83) | 64% | 0 |
| idle | 0 | 50 | 8 | 0.17 / 0.29 / 0.42 (0.37–0.46) | 0.12 / 0.29 / 0.37 (0.37–0.46) | 0.42 / 0.78 / 1.07 (0.83–1.15) | 0.29 / 0.74 / 0.79 (0.74–0.79) | 47% | 0 |
| idle | 0 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.29–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.74) | 0.46 / 0.50 / 0.54 (0.50–0.58) | 0.33 / 0.37 / 0.46 (0.46–0.46) | 100% | 0 |
| idle | 0 | 200 | 4 | 0.21 / 0.33 / 3.26 (3.20–3.26) | 0.21 / 0.33 / 3.36 (3.26–3.39) | 0.46 / 0.62 / 1.20 (1.04–3.62) | 0.33 / 0.37 / 1.15 (0.87–2.11) | 100% | 0 |
| idle | 0 | 200 | 6 | 0.25 / 0.37 / 0.50 (0.46–0.54) | 0.29 / 0.42 / 0.54 (0.46–0.58) | 0.54 / 0.91 / 1.28 (1.15–1.58) | 0.42 / 0.83 / 0.87 (0.87–0.91) | 64% | 0 |
| idle | 0 | 200 | 8 | 0.29 / 0.42 / 0.54 (0.42–0.58) | 0.25 / 0.37 / 0.54 (0.50–0.54) | 0.58 / 1.04 / 1.54 (1.36–4.10) | 0.37 / 0.83 / 0.87 (0.74–0.87) | 48% | 9 |
| idle | 0 | 1000 | 2 | 0.58 / 0.62 / 0.70 (0.62–0.70) | 0.58 / 0.62 / 0.74 (0.70–1.46) | 1.15 / 1.25 / 1.33 (1.25–1.46) | 0.70 / 0.74 / 0.83 (0.79–0.83) | 100% | 0 |
| idle | 0 | 1000 | 4 | 0.58 / 0.66 / 3.65 (3.65–4.86) | 0.58 / 0.66 / 3.65 (3.65–3.78) | 1.20 / 1.79 / 5.25 (1.95–8.45) | 0.70 / 0.79 / 1.79 (1.41–2.78) | 99% | 0 |
| idle | 0 | 1000 | 6 | 0.62 / 0.74 / 0.87 (0.83–0.95) | 0.62 / 0.74 / 0.91 (0.83–0.91) | 1.41 / 2.43 / 3.26 (3.23–6.27) | 0.74 / 1.07 / 1.25 (1.12–1.25) | 64% | 1 |
| idle | 0 | 1000 | 8 | 0.62 / 0.74 / 0.87 (0.79–0.91) | 0.62 / 0.74 / 0.87 (0.79–0.91) | 1.36 / 2.62 / 7.36 (3.14–8.83) | 0.74 / 1.15 / 1.25 (1.25–1.25) | 48% | 1 |
| idle | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.25) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 100% | 0 |
| idle | 1000 | 50 | 4 | 0.08 / 0.17 / 3.14 (0.79–3.23) | 0.08 / 0.21 / 3.20 (0.91–3.33) | 0.21 / 0.29 / 1.20 (0.91–3.23) | 0.21 / 0.29 / 1.04 (0.79–1.36) | 99% | 0 |
| idle | 1000 | 50 | 6 | 0.12 / 0.29 / 0.42 (0.37–0.46) | 0.17 / 0.29 / 0.42 (0.42–0.50) | 0.37 / 0.70 / 0.91 (0.87–0.95) | 0.25 / 0.74 / 0.83 (0.79–0.83) | 64% | 0 |
| idle | 1000 | 50 | 8 | 0.17 / 0.33 / 0.42 (0.37–0.46) | 0.17 / 0.29 / 0.42 (0.37–0.42) | 0.33 / 0.74 / 1.12 (1.00–1.25) | 0.25 / 0.74 / 0.79 (0.62–0.79) | 47% | 0 |
| idle | 1000 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.29–0.54) | 0.21 / 0.25 / 0.29 (0.29–0.37) | 0.46 / 0.50 / 0.54 (0.50–0.58) | 0.33 / 0.37 / 0.46 (0.42–0.46) | 100% | 0 |
| idle | 1000 | 200 | 4 | 0.21 / 0.37 / 3.26 (3.10–3.33) | 0.21 / 0.33 / 3.33 (3.23–3.39) | 0.46 / 0.62 / 1.25 (1.04–1.36) | 0.33 / 0.42 / 1.00 (0.66–1.33) | 99% | 0 |
| idle | 1000 | 200 | 6 | 0.25 / 0.37 / 0.54 (0.46–0.54) | 0.29 / 0.42 / 0.54 (0.50–0.58) | 0.54 / 0.91 / 1.20 (1.12–1.57) | 0.37 / 0.83 / 0.87 (0.87–0.91) | 64% | 0 |
| idle | 1000 | 200 | 8 | 0.29 / 0.42 / 0.54 (0.50–0.58) | 0.25 / 0.37 / 0.50 (0.46–0.50) | 0.58 / 1.04 / 1.54 (1.46–1.70) | 0.33 / 0.83 / 0.87 (0.83–0.87) | 47% | 3 |
| idle | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.70 (0.62–0.79) | 0.58 / 0.62 / 0.66 (0.66–0.91) | 1.15 / 1.25 / 1.33 (1.28–1.36) | 0.70 / 0.74 / 0.83 (0.79–0.87) | 100% | 0 |
| idle | 1000 | 1000 | 4 | 0.58 / 0.70 / 3.65 (3.62–3.78) | 0.58 / 0.66 / 3.74 (3.46–3.78) | 1.20 / 1.78 / 7.04 (5.25–8.58) | 0.70 / 0.79 / 1.79 (1.04–3.74) | 99% | 0 |
| idle | 1000 | 1000 | 6 | 0.62 / 0.74 / 0.87 (0.83–0.91) | 0.62 / 0.74 / 0.87 (0.83–0.91) | 1.36 / 2.37 / 4.99 (3.46–6.72) | 0.70 / 1.07 / 1.25 (1.15–1.25) | 64% | 9 |
| idle | 1000 | 1000 | 8 | 0.62 / 0.74 / 0.87 (0.79–0.91) | 0.62 / 0.74 / 0.91 (0.83–0.91) | 1.36 / 2.53 / 6.78 (3.36–8.19) | 0.70 / 1.12 / 1.25 (1.20–1.28) | 47% | 0 |

Signals that found the thread waiting (took the lock), % of signals

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 99.8% | 100% | 0 |
| as a3t | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 67.7% | 100% | 0 |
| as a3t | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 51.6% | 66% | 27 |
| as a3t | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 42.9% | 50% | 46 |
| as a3t | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 99.7% | 100% | 0 |
| as a3t | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 68.7% | 99% | 1 |
| as a3t | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 52.3% | 66% | 66 |
| as a3t | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 42.2% | 49% | 24 |
| as a3t | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 99.8% | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 69.2% | 98% | 0 |
| as a3t | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 47.4% | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 31.6% | 49% | 36 |
| as a3t | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 98.8% | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 67.4% | 99% | 0 |
| as a3t | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 41.1% | 66% | 46 |
| as a3t | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 29.8% | 50% | 7 |
| as a3t | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 99.1% | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 69.9% | 100% | 0 |
| as a3t | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 37.9% | 66% | 32 |
| as a3t | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 28.3% | 49% | 45 |
| as a3t | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 98.9% | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 68.8% | 100% | 0 |
| as a3t | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 36.0% | 66% | 43 |
| as a3t | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 15.9% | 49% | 40 |
| busy | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 32 |
| busy | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 21 |
| busy | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 41 |
| busy | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 40 |
| busy | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 1 |
| busy | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 40 |
| busy | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 62 |
| busy | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 65% | 13 |
| busy | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 20 |
| busy | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 48 |
| busy | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 2 |
| busy | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 1 |
| busy | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 48 |
| busy | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 26 |
| idle | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 97.2% | 99% | 0 |
| idle | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 91.5% | 64% | 0 |
| idle | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 86.1% | 47% | 0 |
| idle | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 96.2% | 100% | 0 |
| idle | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 89.6% | 64% | 0 |
| idle | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 84.0% | 48% | 9 |
| idle | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 90.9% | 99% | 0 |
| idle | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 84.3% | 64% | 1 |
| idle | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 76.4% | 48% | 1 |
| idle | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 87.7% | 99% | 0 |
| idle | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 79.4% | 64% | 0 |
| idle | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 70.6% | 47% | 0 |
| idle | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 87.0% | 99% | 0 |
| idle | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 78.1% | 64% | 0 |
| idle | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 69.5% | 47% | 3 |
| idle | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 82.0% | 99% | 0 |
| idle | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 74.8% | 64% | 9 |
| idle | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 64.2% | 47% | 0 |

Migrations seen: 205647 over 337664729 samples (one sample per measured operation).


### Suite br: unmap, lock per address-space range

load average before: { 5.77 4.89 4.30 }
load average after: { 5.69 5.27 4.56 }; 168 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 fair | 3 group lock | 3 range locks | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 509k (491k–510k) | 491k (490k–492k) | 505k (499k–508k) | 100% | 0 |
| one | 0 | 50 | 3 | 480k (471k–495k) | 480k (479k–481k) | 486k (484k–488k) | 100% | 0 |
| one | 0 | 50 | 4 | 467k (445k–470k) | 467k (466k–468k) | 474k (470k–476k) | 99% | 1 |
| one | 0 | 50 | 6 | 375k (363k–390k) | 409k (398k–413k) | 414k (410k–421k) | 65% | 32 |
| one | 0 | 50 | 8 | 351k (345k–354k) | 385k (381k–388k) | 395k (393k–400k) | 51% | 11 |
| one | 0 | 200 | 2 | 466k (442k–466k) | 466k (465k–467k) | 478k (477k–479k) | 100% | 0 |
| one | 0 | 200 | 3 | 433k (421k–451k) | 450k (447k–453k) | 459k (459k–461k) | 100% | 0 |
| one | 0 | 200 | 4 | 419k (405k–430k) | 440k (438k–443k) | 447k (444k–449k) | 100% | 0 |
| one | 0 | 200 | 6 | 348k (336k–350k) | 383k (383k–389k) | 394k (390k–402k) | 68% | 38 |
| one | 0 | 200 | 8 | 331k (325k–332k) | 369k (360k–371k) | 373k (371k–381k) | 47% | 15 |
| one | 0 | 1000 | 2 | 320k (309k–322k) | 341k (340k–341k) | 345k (343k–345k) | 100% | 0 |
| one | 0 | 1000 | 3 | 306k (305k–315k) | 331k (331k–333k) | 336k (336k–337k) | 100% | 0 |
| one | 0 | 1000 | 4 | 301k (298k–305k) | 325k (323k–326k) | 329k (329k–331k) | 100% | 0 |
| one | 0 | 1000 | 6 | 261k (260k–263k) | 297k (281k–299k) | 300k (295k–303k) | 69% | 32 |
| one | 0 | 1000 | 8 | 249k (246k–253k) | 286k (282k–290k) | 290k (289k–293k) | 46% | 15 |
| one | 1000 | 50 | 2 | 337k (329k–337k) | 329k (328k–329k) | 334k (334k–335k) | 100% | 0 |
| one | 1000 | 50 | 3 | 318k (317k–329k) | 323k (323k–324k) | 326k (325k–327k) | 100% | 0 |
| one | 1000 | 50 | 4 | 317k (315k–318k) | 316k (313k–317k) | 320k (318k–321k) | 99% | 0 |
| one | 1000 | 50 | 6 | 267k (262k–274k) | 284k (282k–287k) | 289k (287k–291k) | 66% | 27 |
| one | 1000 | 50 | 8 | 252k (250k–257k) | 276k (267k–277k) | 280k (274k–280k) | 47% | 19 |
| one | 1000 | 200 | 2 | 316k (308k–318k) | 317k (317k–318k) | 322k (322k–323k) | 100% | 0 |
| one | 1000 | 200 | 3 | 303k (302k–310k) | 309k (309k–310k) | 313k (312k–314k) | 100% | 0 |
| one | 1000 | 200 | 4 | 298k (294k–299k) | 304k (300k–305k) | 308k (305k–308k) | 99% | 1 |
| one | 1000 | 200 | 6 | 251k (247k–253k) | 275k (270k–276k) | 278k (274k–282k) | 66% | 25 |
| one | 1000 | 200 | 8 | 242k (241k–247k) | 264k (262k–267k) | 270k (267k–272k) | 51% | 13 |
| one | 1000 | 1000 | 2 | 242k (236k–243k) | 253k (253k–254k) | 256k (255k–256k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 233k (233k–239k) | 248k (248k–249k) | 252k (251k–252k) | 100% | 0 |
| one | 1000 | 1000 | 4 | 231k (226k–232k) | 244k (244k–245k) | 247k (244k–248k) | 100% | 0 |
| one | 1000 | 1000 | 6 | 200k (186k–207k) | 223k (217k–226k) | 227k (214k–230k) | 67% | 36 |
| one | 1000 | 1000 | 8 | 195k (190k–198k) | 220k (217k–221k) | 222k (219k–223k) | 50% | 19 |
| all | 0 | 50 | 2 | 879k (875k–881k) | 818k (810k–819k) | 961k (960k–966k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.06M (1.05M–1.07M) | 986k (982k–988k) | 1.27M (1.26M–1.28M) | 100% | 0 |
| all | 0 | 50 | 4 | 1.17M (1.16M–1.19M) | 1.09M (1.09M–1.09M) | 1.50M (1.47M–1.50M) | 100% | 0 |
| all | 0 | 50 | 6 | 1.04M (1.04M–1.06M) | 1.06M (1.04M–1.07M) | 1.54M (1.53M–1.56M) | 68% | 219 |
| all | 0 | 50 | 8 | 985k (962k–1.00M) | 1.05M (1.04M–1.06M) | 1.71M (1.68M–1.72M) | 50% | 135 |
| all | 0 | 200 | 2 | 827k (793k–829k) | 733k (731k–740k) | 902k (898k–910k) | 100% | 0 |
| all | 0 | 200 | 3 | 965k (947k–975k) | 866k (860k–871k) | 1.17M (1.15M–1.18M) | 100% | 0 |
| all | 0 | 200 | 4 | 1.04M (940k–1.06M) | 945k (826k–948k) | 1.39M (1.34M–1.39M) | 98% | 3 |
| all | 0 | 200 | 6 | 954k (926k–960k) | 930k (908k–937k) | 1.49M (1.47M–1.50M) | 67% | 179 |
| all | 0 | 200 | 8 | 924k (907k–929k) | 941k (929k–950k) | 1.65M (1.62M–1.65M) | 49% | 107 |
| all | 0 | 1000 | 2 | 496k (495k–501k) | 464k (464k–465k) | 643k (642k–644k) | 100% | 0 |
| all | 0 | 1000 | 3 | 607k (606k–609k) | 512k (511k–513k) | 824k (823k–828k) | 100% | 0 |
| all | 0 | 1000 | 4 | 656k (648k–658k) | 540k (536k–542k) | 953k (951k–956k) | 100% | 0 |
| all | 0 | 1000 | 6 | 592k (581k–599k) | 507k (498k–515k) | 1.04M (982k–1.05M) | 67% | 177 |
| all | 0 | 1000 | 8 | 562k (551k–566k) | 506k (500k–524k) | 1.14M (1.11M–1.14M) | 50% | 197 |
| all | 1000 | 50 | 2 | 459k (451k–461k) | 803k (803k–808k) | 893k (872k–913k) | 100% | 0 |
| all | 1000 | 50 | 3 | 629k (621k–647k) | 966k (934k–996k) | 1.23M (1.03M–1.24M) | 100% | 28 |
| all | 1000 | 50 | 4 | 730k (703k–746k) | 1.07M (1.01M–1.08M) | 1.17M (1.06M–1.29M) | 100% | 0 |
| all | 1000 | 50 | 6 | 737k (729k–741k) | 874k (868k–881k) | 1.06M (1.04M–1.07M) | 66% | 165 |
| all | 1000 | 50 | 8 | 774k (770k–782k) | 896k (884k–899k) | 1.20M (1.17M–1.21M) | 49% | 187 |
| all | 1000 | 200 | 2 | 440k (422k–457k) | 737k (736k–738k) | 901k (897k–908k) | 100% | 0 |
| all | 1000 | 200 | 3 | 586k (564k–593k) | 861k (848k–882k) | 1.17M (1.16M–1.18M) | 100% | 0 |
| all | 1000 | 200 | 4 | 681k (669k–683k) | 953k (935k–961k) | 1.35M (1.34M–1.38M) | 100% | 0 |
| all | 1000 | 200 | 6 | 695k (677k–709k) | 824k (821k–844k) | 1.12M (1.09M–1.13M) | 67% | 217 |
| all | 1000 | 200 | 8 | 723k (713k–731k) | 834k (805k–843k) | 1.23M (1.20M–1.24M) | 49% | 130 |
| all | 1000 | 1000 | 2 | 367k (361k–376k) | 464k (461k–465k) | 642k (641k–646k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 439k (425k–441k) | 511k (511k–514k) | 822k (821k–823k) | 100% | 0 |
| all | 1000 | 1000 | 4 | 489k (472k–502k) | 539k (534k–544k) | 951k (949k–953k) | 100% | 0 |
| all | 1000 | 1000 | 6 | 491k (482k–497k) | 493k (483k–500k) | 1.03M (1.01M–1.03M) | 67% | 171 |
| all | 1000 | 1000 | 8 | 495k (490k–502k) | 479k (475k–487k) | 1.13M (1.12M–1.14M) | 50% | 175 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 fair | 3 group lock | 3 range locks | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.58 / 0.74 / 1.12 (0.83–1.41) | 0.66 / 0.74 / 0.87 (0.83–4.29) | 0.62 / 0.74 / 2.37 (0.79–5.31) | 100% | 0 |
| one | 0 | 50 | 3 | 0.70 / 0.87 / 5.31 (4.00–5.95) | 0.70 / 0.83 / 5.44 (4.61–6.21) | 0.70 / 0.79 / 5.57 (3.78–6.08) | 100% | 0 |
| one | 0 | 50 | 4 | 0.74 / 1.00 / 7.68 (6.27–8.58) | 0.74 / 0.91 / 7.49 (7.42–8.32) | 0.70 / 0.83 / 7.36 (4.86–7.87) | 99% | 1 |
| one | 0 | 50 | 6 | 1.25 / 1.66 / 8.06 (8.00–8.83) | 1.07 / 1.25 / 7.04 (6.53–7.36) | 1.00 / 1.20 / 6.66 (5.70–8.06) | 65% | 32 |
| one | 0 | 50 | 8 | 1.49 / 1.90 / 8.32 (7.74–9.60) | 1.20 / 1.36 / 8.19 (6.53–8.96) | 1.15 / 1.33 / 8.19 (5.76–9.09) | 51% | 11 |
| one | 0 | 200 | 2 | 0.79 / 0.91 / 1.12 (1.00–4.74) | 0.78 / 0.91 / 2.43 (0.95–3.39) | 0.74 / 0.83 / 1.87 (0.91–4.61) | 100% | 0 |
| one | 0 | 200 | 3 | 0.91 / 1.07 / 6.46 (4.86–7.17) | 0.83 / 0.95 / 5.82 (4.03–6.91) | 0.83 / 0.91 / 5.25 (3.52–6.78) | 100% | 0 |
| one | 0 | 200 | 4 | 1.00 / 1.20 / 8.13 (7.62–8.96) | 0.87 / 1.00 / 8.06 (5.06–8.83) | 0.87 / 0.95 / 8.06 (5.82–11.26) | 100% | 0 |
| one | 0 | 200 | 6 | 1.49 / 1.87 / 7.62 (6.66–8.06) | 1.20 / 1.36 / 7.74 (5.76–8.32) | 1.12 / 1.33 / 6.72 (5.31–8.06) | 68% | 38 |
| one | 0 | 200 | 8 | 1.66 / 2.11 / 9.34 (7.74–10.24) | 1.33 / 1.49 / 9.09 (6.78–9.47) | 1.28 / 1.46 / 8.83 (6.21–12.67) | 47% | 15 |
| one | 0 | 1000 | 2 | 1.74 / 1.87 / 6.27 (2.21–7.55) | 1.57 / 1.70 / 5.70 (1.82–7.04) | 1.54 / 1.62 / 5.82 (1.70–7.62) | 100% | 0 |
| one | 0 | 1000 | 3 | 1.87 / 2.03 / 9.34 (7.04–10.75) | 1.66 / 1.74 / 7.68 (5.18–8.19) | 1.58 / 1.70 / 8.19 (5.18–8.58) | 100% | 0 |
| one | 0 | 1000 | 4 | 1.95 / 2.14 / 10.88 (9.73–12.42) | 1.66 / 1.79 / 10.37 (8.96–11.78) | 1.62 / 1.74 / 10.50 (8.58–11.39) | 100% | 0 |
| one | 0 | 1000 | 6 | 2.40 / 2.82 / 10.50 (8.70–11.78) | 2.00 / 2.21 / 8.70 (7.94–10.75) | 1.90 / 2.14 / 8.96 (8.32–9.47) | 69% | 32 |
| one | 0 | 1000 | 8 | 2.66 / 3.20 / 11.52 (9.22–13.44) | 2.14 / 2.30 / 9.86 (7.74–10.50) | 2.11 / 2.27 / 10.24 (9.86–10.50) | 46% | 15 |
| one | 1000 | 50 | 2 | 1.58 / 1.74 / 6.46 (1.90–6.78) | 1.66 / 1.78 / 5.70 (1.87–8.00) | 1.62 / 1.74 / 6.27 (4.86–6.91) | 100% | 0 |
| one | 1000 | 50 | 3 | 1.78 / 1.90 / 8.45 (5.44–8.96) | 1.70 / 1.82 / 8.45 (5.70–9.34) | 1.70 / 1.79 / 8.32 (6.02–9.73) | 100% | 0 |
| one | 1000 | 50 | 4 | 1.74 / 2.00 / 10.62 (10.11–12.03) | 1.74 / 1.95 / 11.52 (10.88–11.78) | 1.70 / 1.82 / 10.88 (9.60–11.14) | 99% | 0 |
| one | 1000 | 50 | 6 | 2.30 / 2.78 / 10.50 (9.86–10.75) | 2.11 / 2.30 / 9.34 (9.09–9.98) | 2.03 / 2.27 / 9.34 (8.00–10.24) | 66% | 27 |
| one | 1000 | 50 | 8 | 2.53 / 3.07 / 11.39 (9.73–12.80) | 2.24 / 2.43 / 10.88 (10.50–11.90) | 2.21 / 2.37 / 9.86 (9.60–10.62) | 47% | 19 |
| one | 1000 | 200 | 2 | 1.79 / 1.90 / 6.40 (4.16–7.74) | 1.78 / 1.90 / 6.02 (2.00–6.78) | 1.70 / 1.87 / 5.25 (2.43–6.98) | 100% | 0 |
| one | 1000 | 200 | 3 | 1.90 / 2.08 / 9.09 (7.55–10.11) | 1.82 / 1.95 / 9.34 (5.63–9.86) | 1.82 / 1.90 / 9.22 (6.27–10.62) | 100% | 0 |
| one | 1000 | 200 | 4 | 1.95 / 2.24 / 12.67 (10.62–13.57) | 1.87 / 2.03 / 10.37 (9.86–12.42) | 1.82 / 2.00 / 11.65 (10.75–13.57) | 99% | 1 |
| one | 1000 | 200 | 6 | 2.53 / 2.98 / 11.01 (10.50–12.03) | 2.21 / 2.43 / 9.98 (9.22–10.75) | 2.14 / 2.40 / 9.86 (9.34–10.24) | 66% | 25 |
| one | 1000 | 200 | 8 | 2.72 / 3.33 / 11.26 (9.34–16.00) | 2.37 / 2.56 / 10.50 (8.83–12.03) | 2.30 / 2.53 / 10.24 (8.32–11.65) | 51% | 13 |
| one | 1000 | 1000 | 2 | 2.72 / 2.85 / 8.00 (5.25–9.09) | 2.56 / 2.69 / 8.32 (3.39–8.96) | 2.53 / 2.66 / 7.68 (5.76–8.83) | 100% | 0 |
| one | 1000 | 1000 | 3 | 2.85 / 3.04 / 11.01 (7.55–11.65) | 2.66 / 2.72 / 10.75 (7.49–11.01) | 2.56 / 2.69 / 9.60 (8.45–10.37) | 100% | 0 |
| one | 1000 | 1000 | 4 | 2.91 / 3.14 / 13.82 (13.57–15.87) | 2.66 / 2.91 / 13.82 (12.16–15.10) | 2.62 / 2.85 / 12.42 (12.03–14.72) | 100% | 0 |
| one | 1000 | 1000 | 6 | 3.55 / 4.10 / 12.67 (10.24–14.08) | 3.04 / 3.33 / 11.39 (9.86–12.16) | 2.94 / 3.23 / 11.14 (9.98–11.14) | 67% | 36 |
| one | 1000 | 1000 | 8 | 3.65 / 4.86 / 14.85 (11.78–18.94) | 3.20 / 3.87 / 11.90 (11.26–12.67) | 3.10 / 3.46 / 12.42 (10.62–13.31) | 50% | 19 |
| all | 0 | 50 | 2 | 0.66 / 1.00 / 4.06 (1.74–5.12) | 1.07 / 1.25 / 4.61 (1.33–4.74) | 0.66 / 1.07 / 2.98 (1.15–4.61) | 100% | 0 |
| all | 0 | 50 | 3 | 1.00 / 1.57 / 7.17 (4.42–8.32) | 1.66 / 1.82 / 7.74 (5.18–8.70) | 0.95 / 1.54 / 6.53 (4.10–7.17) | 100% | 0 |
| all | 0 | 50 | 4 | 1.07 / 2.00 / 9.86 (6.91–10.62) | 2.27 / 2.50 / 11.39 (9.47–12.16) | 1.25 / 1.87 / 8.70 (6.78–9.34) | 100% | 0 |
| all | 0 | 50 | 6 | 2.43 / 5.57 / 12.93 (11.90–14.34) | 4.03 / 5.57 / 13.44 (12.42–13.70) | 1.82 / 3.26 / 9.22 (7.74–10.11) | 68% | 219 |
| all | 0 | 50 | 8 | 4.10 / 10.50 / 17.41 (16.64–19.97) | 6.08 / 8.45 / 17.66 (16.64–22.27) | 2.21 / 4.42 / 10.88 (9.09–12.29) | 50% | 135 |
| all | 0 | 200 | 2 | 1.00 / 1.20 / 4.00 (2.14–4.48) | 1.36 / 1.49 / 5.70 (4.99–10.37) | 0.83 / 1.33 / 2.82 (1.36–5.44) | 100% | 0 |
| all | 0 | 200 | 3 | 1.46 / 1.90 / 7.87 (5.82–9.47) | 2.08 / 2.24 / 8.70 (6.21–11.78) | 1.07 / 1.95 / 6.40 (4.16–12.42) | 100% | 0 |
| all | 0 | 200 | 4 | 1.46 / 2.69 / 12.80 (11.65–61.44) | 2.82 / 3.04 / 12.42 (11.01–58.88) | 1.46 / 2.24 / 9.34 (5.57–11.39) | 98% | 3 |
| all | 0 | 200 | 6 | 2.82 / 6.27 / 14.59 (13.18–16.00) | 4.99 / 6.27 / 14.59 (13.06–15.62) | 2.21 / 3.78 / 9.60 (7.94–9.98) | 67% | 179 |
| all | 0 | 200 | 8 | 4.48 / 11.52 / 18.69 (17.66–20.74) | 7.17 / 9.34 / 18.43 (17.41–20.22) | 2.53 / 4.99 / 11.26 (9.60–13.57) | 49% | 107 |
| all | 0 | 1000 | 2 | 2.62 / 2.78 / 8.19 (3.04–8.45) | 2.94 / 3.07 / 8.32 (6.14–8.96) | 1.58 / 2.91 / 6.14 (6.02–6.46) | 100% | 0 |
| all | 0 | 1000 | 3 | 2.50 / 4.10 / 11.52 (7.10–12.42) | 4.42 / 4.61 / 13.44 (11.26–14.46) | 1.87 / 4.29 / 8.96 (6.72–9.73) | 100% | 0 |
| all | 0 | 1000 | 4 | 2.82 / 5.70 / 18.94 (15.49–19.71) | 5.95 / 6.27 / 23.81 (18.43–25.60) | 3.04 / 4.61 / 13.06 (10.75–13.57) | 100% | 0 |
| all | 0 | 1000 | 6 | 6.21 / 13.57 / 20.48 (17.66–23.04) | 10.37 / 11.39 / 23.04 (21.25–26.62) | 4.16 / 7.23 / 12.80 (11.78–14.34) | 67% | 177 |
| all | 0 | 1000 | 8 | 10.24 / 19.46 / 26.11 (24.32–30.72) | 14.59 / 16.26 / 28.42 (24.58–29.95) | 5.31 / 9.34 / 16.13 (14.85–18.18) | 50% | 197 |
| all | 1000 | 50 | 2 | 1.62 / 1.79 / 7.42 (2.21–7.68) | 1.12 / 1.57 / 5.31 (1.82–5.63) | 0.70 / 1.70 / 4.00 (1.79–6.46) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.08 / 3.39 / 10.11 (7.49–19.97) | 1.66 / 1.90 / 8.06 (7.30–14.85) | 0.95 / 1.82 / 6.85 (5.76–16.13) | 100% | 28 |
| all | 1000 | 50 | 4 | 2.27 / 4.35 / 16.13 (14.72–18.94) | 2.27 / 2.50 / 12.29 (10.88–16.90) | 1.46 / 2.00 / 10.88 (8.06–12.67) | 100% | 0 |
| all | 1000 | 50 | 6 | 3.52 / 6.78 / 15.10 (13.31–17.41) | 3.52 / 5.63 / 13.57 (11.78–15.23) | 2.14 / 3.68 / 11.14 (9.22–19.46) | 66% | 165 |
| all | 1000 | 50 | 8 | 5.06 / 9.47 / 18.69 (17.66–19.46) | 4.48 / 8.32 / 20.22 (14.72–28.67) | 2.43 / 4.74 / 12.42 (11.78–17.15) | 49% | 187 |
| all | 1000 | 200 | 2 | 1.74 / 1.95 / 6.78 (2.08–7.49) | 1.36 / 1.49 / 5.70 (2.08–6.72) | 0.79 / 1.33 / 4.16 (1.66–4.74) | 100% | 0 |
| all | 1000 | 200 | 3 | 2.03 / 3.78 / 10.37 (6.72–12.54) | 2.08 / 2.24 / 9.22 (6.34–10.37) | 1.07 / 1.95 / 7.04 (4.74–7.55) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.53 / 4.74 / 15.36 (14.85–17.66) | 2.72 / 3.23 / 13.44 (11.26–15.62) | 1.46 / 2.24 / 10.88 (6.66–11.01) | 100% | 0 |
| all | 1000 | 200 | 6 | 4.00 / 7.49 / 16.13 (14.98–16.64) | 5.06 / 6.53 / 15.62 (12.80–20.74) | 2.30 / 4.00 / 10.62 (10.24–11.39) | 67% | 217 |
| all | 1000 | 200 | 8 | 5.63 / 10.11 / 19.46 (18.94–25.09) | 5.95 / 9.47 / 20.22 (16.38–44.54) | 2.66 / 5.25 / 13.31 (10.75–38.40) | 49% | 130 |
| all | 1000 | 1000 | 2 | 2.56 / 2.85 / 8.83 (7.42–9.47) | 2.94 / 3.07 / 8.32 (3.20–9.60) | 1.58 / 2.91 / 5.63 (2.98–7.49) | 100% | 0 |
| all | 1000 | 1000 | 3 | 2.94 / 5.70 / 13.57 (9.34–16.38) | 4.42 / 4.61 / 13.95 (10.24–15.36) | 1.87 / 4.29 / 9.60 (8.83–10.62) | 100% | 0 |
| all | 1000 | 1000 | 4 | 4.74 / 6.78 / 22.27 (21.50–26.62) | 5.95 / 6.72 / 23.55 (20.22–26.62) | 3.04 / 4.61 / 13.70 (13.18–14.21) | 100% | 0 |
| all | 1000 | 1000 | 6 | 7.04 / 11.78 / 20.99 (20.22–22.27) | 10.50 / 12.29 / 23.30 (20.99–25.34) | 4.16 / 7.36 / 13.95 (13.06–15.36) | 67% | 171 |
| all | 1000 | 1000 | 8 | 9.86 / 20.22 / 27.90 (25.34–31.23) | 14.85 / 17.15 / 30.98 (25.09–32.51) | 5.31 / 9.47 / 15.23 (14.08–20.99) | 50% | 175 |

Migrations seen: 86083 over 85200569 samples (one sample per measured operation).


### Suite srv: a passive server shared by a background and a foreground client

load average before: { 5.69 5.27 4.56 }
load average after: { 4.82 5.00 4.60 }; 229 s

Foreground call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 52.22 / 172.03 / 176.13 (176.13–176.13) | 52.22 / 172.03 / 176.13 (176.13–176.13) | 50.18 / 174.08 / 176.13 (176.13–178.18) | 32.26 / 57.86 / 59.39 (58.37–3112.96) | 32.51 / 57.86 / 59.90 (59.39–63.49) | 33.28 / 57.86 / 61.95 (60.42–69.63) | 6.46 / 56.83 / 58.37 (58.37–68.61) | 6.53 / 56.83 / 59.39 (58.37–64.00) | 6.53 / 57.34 / 60.42 (59.39–74.75) | 100% | 6 |
| 1 | on | 1000 | 50.69 / 169.98 / 174.08 (172.03–178.18) | 50.18 / 167.94 / 172.03 (172.03–174.08) | 50.69 / 169.98 / 174.08 (174.08–174.08) | 33.28 / 58.88 / 64.00 (61.95–66.56) | 33.28 / 58.88 / 67.58 (64.51–84.99) | 33.79 / 59.90 / 65.02 (60.42–72.70) | 8.45 / 57.86 / 59.39 (58.88–59.90) | 8.45 / 57.86 / 60.42 (58.88–60.93) | 8.58 / 59.39 / 60.42 (60.42–60.42) | 100% | 0 |
| 1 | off | 0 | 6.40 / 9.73 / 34.82 (19.20–50.18) | 6.40 / 9.47 / 26.11 (23.55–50.18) | 6.40 / 9.34 / 32.00 (18.43–3211.26) | 6.40 / 9.47 / 28.42 (20.22–44.54) | 6.40 / 9.22 / 30.72 (14.72–34.30) | 6.40 / 8.32 / 26.88 (17.66–33.28) | 6.40 / 9.09 / 32.77 (15.10–43.01) | 6.40 / 9.22 / 16.26 (10.62–25.60) | 6.40 / 9.22 / 23.04 (18.69–29.95) | 100% | 2 |
| 1 | off | 1000 | 8.45 / 11.78 / 32.77 (28.93–41.47) | 8.45 / 11.52 / 36.35 (15.74–44.03) | 8.45 / 10.37 / 30.98 (20.22–36.35) | 8.45 / 11.65 / 23.04 (19.97–43.01) | 8.45 / 11.65 / 20.74 (16.90–33.79) | 8.45 / 11.26 / 40.45 (34.82–44.03) | 8.45 / 11.65 / 34.82 (15.23–40.45) | 8.45 / 11.65 / 29.70 (12.67–36.86) | 8.45 / 8.70 / 21.50 (20.22–41.98) | 100% | 0 |
| 2 | on | 0 | 6.66 / 10.88 / 30.46 (24.06–46.08) | 6.53 / 10.37 / 33.28 (22.53–41.98) | 6.40 / 9.47 / 23.55 (17.15–28.93) | 6.66 / 11.01 / 37.89 (28.93–43.01) | 6.53 / 10.11 / 20.22 (17.66–35.33) | 6.40 / 9.22 / 28.67 (22.02–52.22) | 6.46 / 10.24 / 35.33 (27.14–43.52) | 6.72 / 10.37 / 24.32 (17.15–29.95) | 6.40 / 9.34 / 26.62 (19.46–32.77) | 99% | 3 |
| 2 | on | 1000 | 9.60 / 14.34 / 36.86 (23.04–44.54) | 8.70 / 12.29 / 32.26 (22.27–49.15) | 8.45 / 11.52 / 34.82 (20.48–46.08) | 9.60 / 14.21 / 41.98 (21.25–57.86) | 8.58 / 12.29 / 26.88 (18.94–49.66) | 8.45 / 11.52 / 40.45 (23.30–44.03) | 8.58 / 13.82 / 36.86 (27.14–46.59) | 8.70 / 13.18 / 32.51 (24.58–38.40) | 8.45 / 11.52 / 44.54 (20.74–51.20) | 99% | 3 |
| 2 | off | 0 | 6.46 / 10.62 / 34.82 (23.55–41.98) | 6.66 / 10.37 / 29.95 (15.62–51.71) | 6.46 / 9.47 / 28.42 (19.97–47.10) | 6.46 / 10.50 / 28.67 (22.53–54.78) | 6.72 / 10.37 / 28.16 (22.78–44.54) | 6.46 / 9.73 / 27.90 (22.78–45.06) | 6.46 / 10.75 / 28.67 (20.48–39.94) | 6.85 / 10.50 / 35.84 (28.67–37.89) | 6.46 / 10.50 / 27.90 (24.58–35.84) | 99% | 0 |
| 2 | off | 1000 | 8.45 / 12.67 / 46.59 (27.39–2260.99) | 9.73 / 14.72 / 41.47 (30.21–45.57) | 8.45 / 11.90 / 27.39 (20.22–44.54) | 8.45 / 12.29 / 34.30 (30.98–61.95) | 8.83 / 13.57 / 32.26 (26.11–40.96) | 8.45 / 12.54 / 37.38 (19.20–47.62) | 8.45 / 12.29 / 33.79 (28.67–62.98) | 9.86 / 13.57 / 31.49 (30.72–38.40) | 8.45 / 11.90 / 32.00 (18.94–36.86) | 99% | 5 |

Background calls served/s

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 11k (10k–11k) | 11k (11k–11k) | 11k (11k–11k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 100% | 6 |
| 1 | on | 1000 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 11k (11k–11k) | 11k (11k–11k) | 11k (11k–11k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 100% | 0 |
| 1 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 2 |
| 1 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 0 |
| 2 | on | 0 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 99% | 3 |
| 2 | on | 1000 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 99% | 3 |
| 2 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 0 |
| 2 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 5 |

Calls refused/s

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 100% | 6 |
| 1 | on | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 100% | 0 |
| 1 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 2 |
| 1 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 0 |
| 2 | on | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 99% | 3 |
| 2 | on | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 99% | 3 |
| 2 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 0 |
| 2 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 5 |

Migrations seen: 36250 over 1805773 samples (one sample per measured operation).


## Appendix H: full tables, fourth series, pass 2


host: macos aarch64, Apple M5, 4 performance + 6 efficiency cores
pinning: none possible; user-interactive QoS
clock: resolution 41 ns, 13.4 ns per read
reps 5, 150 ms per run, max k 8, lock of the group candidates: queue

### Suite a3n: k-1 threads signal one notify, notify by one atomic OR

load average before: { 4.82 5.00 4.60 }
load average after: { 5.74 5.50 4.94 }; 268 s

Throughput, signals/s, median (min–max)

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 707k (703k–710k) | 707k (703k–709k) | 659k (657k–662k) | 645k (645k–649k) | 100% | 0 |
| as a3t | 0 | 50 | 4 | 2.09M (2.08M–2.11M) | 2.09M (2.07M–2.10M) | 1.94M (1.92M–1.95M) | 1.94M (1.93M–1.95M) | 99% | 0 |
| as a3t | 0 | 50 | 6 | 3.40M (3.28M–3.42M) | 3.38M (2.98M–3.41M) | 2.53M (2.45M–2.66M) | 3.05M (3.04M–3.06M) | 67% | 65 |
| as a3t | 0 | 50 | 8 | 4.69M (4.67M–4.70M) | 4.69M (4.63M–4.70M) | 2.87M (2.82M–2.91M) | 4.26M (4.22M–4.29M) | 50% | 39 |
| as a3t | 0 | 200 | 2 | 653k (652k–655k) | 654k (647k–655k) | 565k (565k–567k) | 604k (601k–604k) | 100% | 0 |
| as a3t | 0 | 200 | 4 | 1.94M (1.93M–1.95M) | 1.94M (1.93M–1.94M) | 1.66M (1.66M–1.68M) | 1.80M (1.79M–1.82M) | 99% | 0 |
| as a3t | 0 | 200 | 6 | 3.19M (3.15M–3.20M) | 3.18M (3.16M–3.21M) | 1.84M (1.83M–1.94M) | 2.87M (2.85M–2.89M) | 67% | 44 |
| as a3t | 0 | 200 | 8 | 4.42M (4.39M–4.42M) | 4.40M (4.38M–4.41M) | 2.10M (2.05M–2.13M) | 4.05M (4.05M–4.12M) | 50% | 45 |
| as a3t | 0 | 1000 | 2 | 526k (525k–527k) | 526k (522k–526k) | 396k (396k–397k) | 489k (488k–492k) | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 1.56M (1.55M–1.56M) | 1.56M (1.55M–1.57M) | 890k (868k–912k) | 1.47M (1.47M–1.48M) | 99% | 0 |
| as a3t | 0 | 1000 | 6 | 1.88M (1.86M–1.89M) | 2.57M (2.55M–2.57M) | 980k (969k–990k) | 2.38M (2.36M–2.40M) | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 1.87M (1.85M–1.88M) | 3.61M (3.60M–3.62M) | 1.04M (1.02M–1.08M) | 3.43M (3.42M–3.48M) | 49% | 49 |
| as a3t | 1000 | 50 | 2 | 705k (702k–711k) | 704k (701k–713k) | 660k (657k–662k) | 646k (645k–651k) | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 2.09M (2.08M–2.11M) | 2.09M (2.08M–2.10M) | 1.94M (1.93M–1.95M) | 1.94M (1.93M–1.95M) | 100% | 0 |
| as a3t | 1000 | 50 | 6 | 3.41M (3.36M–3.41M) | 3.40M (3.30M–3.42M) | 2.77M (2.75M–2.89M) | 3.16M (3.11M–3.19M) | 67% | 36 |
| as a3t | 1000 | 50 | 8 | 4.71M (4.67M–4.74M) | 4.70M (4.68M–4.72M) | 3.25M (2.78M–3.44M) | 4.45M (4.33M–4.48M) | 50% | 70 |
| as a3t | 1000 | 200 | 2 | 654k (651k–656k) | 652k (647k–656k) | 569k (566k–569k) | 603k (602k–606k) | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 1.94M (1.93M–1.95M) | 1.94M (1.93M–1.95M) | 1.67M (1.67M–1.69M) | 1.81M (1.80M–1.82M) | 99% | 0 |
| as a3t | 1000 | 200 | 6 | 3.20M (3.19M–3.20M) | 3.20M (3.19M–3.20M) | 2.20M (2.14M–2.26M) | 2.97M (2.95M–2.97M) | 66% | 41 |
| as a3t | 1000 | 200 | 8 | 4.42M (4.39M–4.42M) | 4.41M (4.39M–4.43M) | 2.24M (2.21M–2.27M) | 4.21M (4.18M–4.24M) | 50% | 24 |
| as a3t | 1000 | 1000 | 2 | 527k (525k–527k) | 527k (525k–528k) | 396k (395k–396k) | 492k (491k–493k) | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 1.56M (1.56M–1.56M) | 1.56M (1.55M–1.56M) | 1.06M (1.05M–1.06M) | 1.47M (1.47M–1.49M) | 99% | 0 |
| as a3t | 1000 | 1000 | 6 | 1.88M (1.87M–1.89M) | 2.56M (2.55M–2.58M) | 1.08M (1.05M–1.10M) | 2.45M (2.44M–2.46M) | 66% | 69 |
| as a3t | 1000 | 1000 | 8 | 1.87M (1.86M–1.88M) | 3.62M (3.61M–3.63M) | 1.11M (1.10M–1.12M) | 3.57M (3.57M–3.58M) | 50% | 62 |
| busy | 0 | 50 | 2 | 705k (702k–712k) | 707k (702k–710k) | 697k (696k–701k) | 722k (719k–724k) | 100% | 0 |
| busy | 0 | 50 | 4 | 2.09M (2.08M–2.10M) | 2.09M (2.08M–2.10M) | 1.95M (1.95M–1.96M) | 2.11M (2.09M–2.11M) | 99% | 1 |
| busy | 0 | 50 | 6 | 3.40M (3.36M–3.42M) | 3.40M (3.36M–3.42M) | 2.88M (2.80M–2.97M) | 3.42M (3.39M–3.43M) | 67% | 25 |
| busy | 0 | 50 | 8 | 4.69M (4.58M–4.70M) | 4.69M (4.66M–4.70M) | 3.60M (3.39M–3.81M) | 4.78M (4.77M–4.80M) | 50% | 22 |
| busy | 0 | 200 | 2 | 652k (649k–656k) | 653k (650k–654k) | 619k (619k–625k) | 677k (674k–680k) | 100% | 0 |
| busy | 0 | 200 | 4 | 1.94M (1.93M–1.95M) | 1.94M (1.93M–1.95M) | 1.68M (1.67M–1.68M) | 1.96M (1.95M–1.96M) | 99% | 0 |
| busy | 0 | 200 | 6 | 3.18M (3.16M–3.19M) | 3.18M (3.17M–3.20M) | 2.50M (2.40M–2.60M) | 3.20M (3.18M–3.20M) | 66% | 33 |
| busy | 0 | 200 | 8 | 4.42M (4.40M–4.43M) | 4.41M (4.40M–4.41M) | 2.55M (2.47M–2.64M) | 4.47M (4.46M–4.48M) | 49% | 51 |
| busy | 0 | 1000 | 2 | 526k (525k–529k) | 525k (523k–525k) | 420k (419k–424k) | 539k (539k–541k) | 100% | 0 |
| busy | 0 | 1000 | 4 | 1.56M (1.55M–1.56M) | 1.56M (1.55M–1.56M) | 1.16M (1.16M–1.17M) | 1.57M (1.57M–1.57M) | 100% | 0 |
| busy | 0 | 1000 | 6 | 1.88M (1.86M–1.90M) | 2.57M (2.54M–2.57M) | 1.22M (1.19M–1.28M) | 2.58M (2.57M–2.59M) | 66% | 56 |
| busy | 0 | 1000 | 8 | 1.87M (1.86M–1.88M) | 3.61M (3.57M–3.62M) | 1.24M (1.19M–1.29M) | 3.64M (3.62M–3.64M) | 50% | 51 |
| busy | 1000 | 50 | 2 | 706k (702k–707k) | 706k (702k–710k) | 698k (696k–702k) | 718k (718k–721k) | 100% | 0 |
| busy | 1000 | 50 | 4 | 2.09M (2.07M–2.10M) | 2.09M (2.09M–2.10M) | 1.95M (1.94M–1.96M) | 2.10M (2.10M–2.12M) | 99% | 0 |
| busy | 1000 | 50 | 6 | 3.40M (3.34M–3.41M) | 3.40M (3.36M–3.42M) | 2.88M (2.76M–2.99M) | 3.40M (3.39M–3.43M) | 67% | 33 |
| busy | 1000 | 50 | 8 | 4.71M (4.68M–4.73M) | 4.69M (4.61M–4.70M) | 3.63M (3.42M–3.90M) | 4.77M (4.75M–4.79M) | 50% | 22 |
| busy | 1000 | 200 | 2 | 653k (650k–660k) | 653k (650k–657k) | 620k (618k–621k) | 679k (678k–681k) | 100% | 0 |
| busy | 1000 | 200 | 4 | 1.94M (1.94M–1.95M) | 1.94M (1.93M–1.95M) | 1.68M (1.67M–1.69M) | 1.96M (1.95M–1.97M) | 99% | 0 |
| busy | 1000 | 200 | 6 | 3.18M (3.11M–3.19M) | 3.18M (3.15M–3.19M) | 2.47M (2.37M–2.56M) | 3.20M (2.87M–3.21M) | 66% | 37 |
| busy | 1000 | 200 | 8 | 4.41M (4.41M–4.42M) | 4.42M (4.39M–4.42M) | 2.52M (2.47M–2.64M) | 4.47M (4.46M–4.49M) | 49% | 33 |
| busy | 1000 | 1000 | 2 | 525k (523k–529k) | 525k (523k–528k) | 420k (420k–423k) | 539k (537k–539k) | 100% | 0 |
| busy | 1000 | 1000 | 4 | 1.56M (1.55M–1.56M) | 1.55M (1.55M–1.56M) | 1.17M (1.17M–1.17M) | 1.57M (1.57M–1.57M) | 99% | 0 |
| busy | 1000 | 1000 | 6 | 1.87M (1.86M–1.89M) | 2.56M (2.53M–2.58M) | 1.25M (1.21M–1.29M) | 2.57M (2.56M–2.58M) | 66% | 76 |
| busy | 1000 | 1000 | 8 | 1.87M (1.86M–1.88M) | 3.60M (3.56M–3.62M) | 1.24M (1.19M–1.27M) | 3.64M (3.63M–3.65M) | 49% | 67 |
| idle | 0 | 50 | 2 | 39k (39k–40k) | 39k (39k–39k) | 39k (38k–39k) | 39k (38k–39k) | 100% | 0 |
| idle | 0 | 50 | 4 | 116k (116k–118k) | 116k (116k–117k) | 115k (115k–117k) | 116k (115k–116k) | 100% | 0 |
| idle | 0 | 50 | 6 | 199k (198k–201k) | 198k (198k–200k) | 197k (195k–199k) | 197k (196k–199k) | 65% | 6 |
| idle | 0 | 50 | 8 | 282k (279k–283k) | 281k (280k–282k) | 278k (277k–280k) | 280k (279k–282k) | 48% | 0 |
| idle | 0 | 200 | 2 | 39k (38k–39k) | 39k (38k–39k) | 38k (38k–38k) | 38k (38k–39k) | 100% | 0 |
| idle | 0 | 200 | 4 | 115k (114k–116k) | 115k (114k–116k) | 115k (114k–115k) | 115k (114k–115k) | 99% | 0 |
| idle | 0 | 200 | 6 | 199k (197k–200k) | 199k (198k–199k) | 196k (193k–197k) | 197k (196k–197k) | 64% | 6 |
| idle | 0 | 200 | 8 | 281k (278k–282k) | 280k (280k–281k) | 277k (275k–278k) | 280k (278k–281k) | 47% | 0 |
| idle | 0 | 1000 | 2 | 38k (38k–38k) | 38k (38k–39k) | 37k (37k–38k) | 38k (38k–38k) | 99% | 0 |
| idle | 0 | 1000 | 4 | 114k (113k–114k) | 114k (113k–114k) | 112k (110k–112k) | 113k (112k–115k) | 99% | 0 |
| idle | 0 | 1000 | 6 | 196k (194k–197k) | 195k (195k–195k) | 189k (188k–191k) | 193k (193k–195k) | 65% | 6 |
| idle | 0 | 1000 | 8 | 276k (275k–277k) | 276k (274k–276k) | 266k (266k–268k) | 276k (275k–277k) | 48% | 0 |
| idle | 1000 | 50 | 2 | 39k (38k–39k) | 39k (39k–39k) | 39k (39k–39k) | 39k (38k–39k) | 100% | 0 |
| idle | 1000 | 50 | 4 | 116k (115k–117k) | 116k (115k–117k) | 116k (114k–116k) | 116k (115k–117k) | 99% | 0 |
| idle | 1000 | 50 | 6 | 199k (197k–200k) | 199k (197k–201k) | 197k (196k–197k) | 198k (197k–199k) | 64% | 0 |
| idle | 1000 | 50 | 8 | 281k (281k–283k) | 281k (280k–282k) | 279k (277k–281k) | 281k (280k–282k) | 48% | 0 |
| idle | 1000 | 200 | 2 | 39k (39k–39k) | 39k (38k–39k) | 38k (38k–38k) | 38k (38k–39k) | 100% | 0 |
| idle | 1000 | 200 | 4 | 115k (114k–116k) | 116k (115k–116k) | 114k (114k–114k) | 115k (114k–115k) | 99% | 0 |
| idle | 1000 | 200 | 6 | 197k (197k–199k) | 198k (198k–199k) | 197k (196k–197k) | 197k (196k–197k) | 64% | 1 |
| idle | 1000 | 200 | 8 | 280k (274k–281k) | 280k (277k–280k) | 276k (274k–277k) | 279k (277k–280k) | 48% | 0 |
| idle | 1000 | 1000 | 2 | 38k (38k–38k) | 38k (38k–39k) | 37k (37k–38k) | 38k (38k–38k) | 100% | 0 |
| idle | 1000 | 1000 | 4 | 114k (113k–115k) | 114k (113k–115k) | 111k (110k–112k) | 113k (113k–114k) | 99% | 0 |
| idle | 1000 | 1000 | 6 | 195k (194k–197k) | 196k (195k–197k) | 189k (188k–191k) | 194k (193k–195k) | 65% | 5 |
| idle | 1000 | 1000 | 8 | 276k (275k–278k) | 276k (274k–278k) | 268k (265k–268k) | 278k (275k–278k) | 48% | 0 |

Signal call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.21 / 0.25 / 0.29 (0.29–0.29) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 100% | 0 |
| as a3t | 0 | 50 | 4 | 0.08 / 0.17 / 0.25 (0.25–0.33) | 0.08 / 0.12 / 0.29 (0.25–0.33) | 0.21 / 0.37 / 0.79 (0.70–0.87) | 0.25 / 0.33 / 0.50 (0.50–0.66) | 99% | 0 |
| as a3t | 0 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.62 / 1.15 / 4.74 (1.74–6.21) | 0.29 / 0.70 / 0.83 (0.83–0.83) | 67% | 65 |
| as a3t | 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 1.07 / 1.82 / 8.58 (6.91–18.43) | 0.29 / 0.83 / 1.07 (1.04–1.12) | 50% | 39 |
| as a3t | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.29 (0.25–0.29) | 0.46 / 0.50 / 0.54 (0.50–0.54) | 0.33 / 0.37 / 0.42 (0.42–0.42) | 100% | 0 |
| as a3t | 0 | 200 | 4 | 0.21 / 0.25 / 0.42 (0.37–0.46) | 0.21 / 0.25 / 0.42 (0.37–0.46) | 0.46 / 0.58 / 1.33 (0.83–1.36) | 0.33 / 0.46 / 0.66 (0.58–0.70) | 99% | 0 |
| as a3t | 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.42) | 0.25 / 0.37 / 0.42 (0.42–0.42) | 1.36 / 1.79 / 7.04 (5.44–7.87) | 0.37 / 0.79 / 0.91 (0.91–0.91) | 67% | 44 |
| as a3t | 0 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.87 / 2.82 / 10.37 (9.09–11.01) | 0.33 / 0.95 / 1.15 (1.15–1.15) | 50% | 45 |
| as a3t | 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.66) | 0.58 / 0.62 / 0.66 (0.62–0.70) | 1.15 / 1.25 / 1.33 (1.28–1.33) | 0.70 / 0.74 / 0.83 (0.79–0.83) | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 0.58 / 0.66 / 0.79 (0.70–0.87) | 0.58 / 0.62 / 0.83 (0.70–0.91) | 1.79 / 2.30 / 11.14 (6.21–12.54) | 0.70 / 0.83 / 1.07 (0.87–1.20) | 99% | 0 |
| as a3t | 0 | 1000 | 6 | 0.62 / 0.78 / 141.31 (139.26–145.41) | 0.62 / 0.74 / 0.83 (0.83–0.83) | 3.68 / 4.48 / 13.57 (11.39–15.49) | 0.74 / 1.15 / 1.28 (1.28–1.28) | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 0.62 / 0.74 / 344.06 (344.06–348.16) | 0.66 / 0.79 / 0.87 (0.87–0.87) | 5.31 / 6.66 / 16.90 (15.36–18.94) | 0.70 / 1.28 / 1.54 (1.49–1.54) | 49% | 49 |
| as a3t | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.21 / 0.25 / 0.33 (0.29–0.33) | 0.21 / 0.29 / 0.33 (0.33–0.33) | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 0.08 / 0.12 / 0.29 (0.25–0.33) | 0.08 / 0.12 / 0.25 (0.25–0.33) | 0.21 / 0.37 / 0.66 (0.66–0.79) | 0.25 / 0.33 / 0.46 (0.37–0.58) | 100% | 0 |
| as a3t | 1000 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.33–0.33) | 0.17 / 0.29 / 0.33 (0.33–0.37) | 0.46 / 0.91 / 1.25 (1.15–2.50) | 0.21 / 0.74 / 0.83 (0.83–0.91) | 67% | 36 |
| as a3t | 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.74 / 1.70 / 5.70 (4.35–6.91) | 0.21 / 0.87 / 1.07 (1.07–1.12) | 50% | 70 |
| as a3t | 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.42 / 0.46 / 0.54 (0.50–0.58) | 0.33 / 0.37 / 0.46 (0.42–0.46) | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 0.21 / 0.29 / 0.37 (0.37–0.42) | 0.21 / 0.25 / 0.37 (0.33–0.42) | 0.42 / 0.62 / 1.15 (0.74–1.57) | 0.33 / 0.46 / 0.58 (0.50–0.66) | 99% | 0 |
| as a3t | 1000 | 200 | 6 | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.87 / 1.57 / 5.44 (2.91–5.76) | 0.29 / 0.83 / 0.95 (0.91–0.95) | 66% | 41 |
| as a3t | 1000 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.42–0.46) | 1.70 / 2.62 / 10.37 (8.13–17.41) | 0.29 / 0.95 / 1.20 (1.15–1.20) | 50% | 24 |
| as a3t | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.66 (0.62–0.66) | 0.58 / 0.62 / 0.66 (0.62–0.66) | 1.15 / 1.25 / 1.58 (1.49–1.70) | 0.70 / 0.74 / 0.83 (0.79–0.83) | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 0.58 / 0.62 / 0.74 (0.74–0.87) | 0.58 / 0.62 / 0.79 (0.70–0.83) | 1.62 / 1.87 / 7.30 (4.16–8.83) | 0.70 / 0.83 / 1.04 (0.87–1.12) | 99% | 0 |
| as a3t | 1000 | 1000 | 6 | 0.62 / 0.78 / 141.31 (141.31–145.41) | 0.62 / 0.74 / 0.83 (0.83–0.87) | 3.14 / 4.29 / 12.42 (11.78–12.80) | 0.66 / 1.20 / 1.33 (1.33–1.41) | 66% | 69 |
| as a3t | 1000 | 1000 | 8 | 0.62 / 0.78 / 344.06 (339.97–348.16) | 0.62 / 0.79 / 0.87 (0.87–0.87) | 4.86 / 5.95 / 16.38 (14.08–17.66) | 0.62 / 1.15 / 1.49 (1.46–1.49) | 50% | 62 |
| busy | 0 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.21 / 0.25 (0.25–0.25) | 0.04 / 0.08 / 0.12 (0.12–0.12) | 100% | 0 |
| busy | 0 | 50 | 4 | 0.08 / 0.17 / 0.29 (0.25–0.33) | 0.08 / 0.17 / 0.33 (0.21–0.33) | 0.21 / 0.29 / 0.58 (0.46–0.70) | 0.08 / 0.12 / 0.25 (0.12–0.29) | 99% | 1 |
| busy | 0 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 0.42 / 0.74 / 1.04 (0.91–1.07) | 0.12 / 0.25 / 0.29 (0.29–0.33) | 67% | 25 |
| busy | 0 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.54 / 1.46 / 4.61 (1.87–5.12) | 0.12 / 0.25 / 0.37 (0.33–0.37) | 50% | 22 |
| busy | 0 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.21 / 0.25 / 0.25 (0.25–0.29) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.12 / 0.21 / 0.25 (0.21–0.25) | 100% | 0 |
| busy | 0 | 200 | 4 | 0.21 / 0.25 / 0.37 (0.33–0.46) | 0.21 / 0.25 / 0.42 (0.37–0.42) | 0.42 / 0.58 / 1.00 (0.83–1.04) | 0.21 / 0.25 / 0.33 (0.29–0.37) | 99% | 0 |
| busy | 0 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.46) | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.62 / 1.28 / 1.82 (1.57–2.56) | 0.25 / 0.37 / 0.42 (0.42–0.46) | 66% | 33 |
| busy | 0 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.33 / 2.30 / 8.58 (7.49–19.97) | 0.25 / 0.42 / 0.46 (0.46–0.50) | 49% | 51 |
| busy | 0 | 1000 | 2 | 0.58 / 0.62 / 0.62 (0.62–0.66) | 0.58 / 0.62 / 0.66 (0.62–0.66) | 1.00 / 1.49 / 1.70 (1.66–1.70) | 0.50 / 0.58 / 0.62 (0.62–0.62) | 100% | 0 |
| busy | 0 | 1000 | 4 | 0.58 / 0.62 / 0.74 (0.70–0.83) | 0.58 / 0.62 / 0.83 (0.79–0.91) | 1.15 / 1.78 / 4.93 (2.94–6.78) | 0.58 / 0.62 / 0.74 (0.74–0.78) | 100% | 0 |
| busy | 0 | 1000 | 6 | 0.66 / 0.83 / 137.22 (129.02–143.36) | 0.62 / 0.74 / 0.83 (0.83–0.83) | 2.66 / 3.74 / 10.75 (9.98–11.26) | 0.62 / 0.74 / 0.83 (0.83–0.83) | 66% | 56 |
| busy | 0 | 1000 | 8 | 0.66 / 0.79 / 344.06 (339.97–344.06) | 0.66 / 0.79 / 0.87 (0.87–0.91) | 4.16 / 5.50 / 15.10 (12.42–16.38) | 0.62 / 0.79 / 0.87 (0.87–0.87) | 50% | 51 |
| busy | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.17) | 0.08 / 0.17 / 0.25 (0.25–0.25) | 0.04 / 0.08 / 0.12 (0.12–0.12) | 100% | 0 |
| busy | 1000 | 50 | 4 | 0.08 / 0.12 / 0.25 (0.21–0.29) | 0.08 / 0.17 / 0.29 (0.25–0.33) | 0.21 / 0.37 / 0.54 (0.37–0.83) | 0.08 / 0.12 / 0.25 (0.21–0.25) | 99% | 0 |
| busy | 1000 | 50 | 6 | 0.12 / 0.25 / 0.33 (0.29–0.37) | 0.17 / 0.25 / 0.33 (0.33–0.33) | 0.42 / 0.74 / 0.95 (0.83–5.06) | 0.12 / 0.25 / 0.33 (0.29–0.33) | 67% | 33 |
| busy | 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.33–0.37) | 0.17 / 0.29 / 0.37 (0.37–0.37) | 0.54 / 1.36 / 4.42 (2.50–8.06) | 0.17 / 0.25 / 0.37 (0.33–0.42) | 50% | 22 |
| busy | 1000 | 200 | 2 | 0.21 / 0.25 / 0.25 (0.25–0.33) | 0.21 / 0.25 / 0.25 (0.25–0.25) | 0.25 / 0.42 / 0.54 (0.54–0.54) | 0.12 / 0.21 / 0.25 (0.21–0.25) | 100% | 0 |
| busy | 1000 | 200 | 4 | 0.21 / 0.25 / 0.42 (0.33–0.46) | 0.21 / 0.25 / 0.42 (0.33–0.42) | 0.42 / 0.58 / 0.95 (0.70–1.15) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 99% | 0 |
| busy | 1000 | 200 | 6 | 0.25 / 0.37 / 0.42 (0.42–0.46) | 0.25 / 0.33 / 0.42 (0.42–0.42) | 0.62 / 1.28 / 1.95 (1.62–3.33) | 0.25 / 0.37 / 0.42 (0.42–0.42) | 66% | 37 |
| busy | 1000 | 200 | 8 | 0.25 / 0.37 / 0.46 (0.46–0.46) | 0.25 / 0.37 / 0.46 (0.46–0.46) | 1.36 / 2.37 / 8.58 (5.89–8.96) | 0.25 / 0.42 / 0.46 (0.46–0.46) | 49% | 33 |
| busy | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.66 (0.62–0.70) | 0.58 / 0.62 / 0.66 (0.62–0.70) | 1.00 / 1.49 / 1.70 (1.70–1.70) | 0.50 / 0.58 / 0.62 (0.62–0.62) | 100% | 0 |
| busy | 1000 | 1000 | 4 | 0.58 / 0.62 / 0.78 (0.70–0.83) | 0.58 / 0.66 / 0.83 (0.79–0.87) | 1.15 / 1.70 / 4.10 (3.10–5.82) | 0.58 / 0.62 / 0.74 (0.74–0.79) | 99% | 0 |
| busy | 1000 | 1000 | 6 | 0.66 / 0.79 / 139.26 (129.02–141.31) | 0.62 / 0.74 / 0.83 (0.83–0.87) | 2.56 / 3.74 / 10.11 (9.09–11.01) | 0.62 / 0.74 / 0.83 (0.83–0.83) | 66% | 76 |
| busy | 1000 | 1000 | 8 | 0.62 / 0.74 / 348.16 (335.87–352.26) | 0.62 / 0.79 / 0.91 (0.87–2.11) | 4.10 / 5.82 / 17.41 (14.21–18.18) | 0.62 / 0.79 / 0.87 (0.87–1.49) | 49% | 67 |
| idle | 0 | 50 | 2 | 0.08 / 0.12 / 0.21 (0.17–3.07) | 0.08 / 0.12 / 0.17 (0.17–0.25) | 0.21 / 0.25 / 0.33 (0.29–0.42) | 0.21 / 0.25 / 0.33 (0.29–0.66) | 100% | 0 |
| idle | 0 | 50 | 4 | 0.08 / 0.17 / 3.20 (3.10–3.23) | 0.08 / 0.17 / 3.26 (3.10–3.26) | 0.21 / 0.33 / 1.15 (0.83–1.28) | 0.21 / 0.25 / 1.12 (0.79–1.66) | 100% | 0 |
| idle | 0 | 50 | 6 | 0.12 / 0.25 / 0.37 (0.33–0.46) | 0.12 / 0.29 / 0.42 (0.37–0.42) | 0.37 / 0.66 / 0.91 (0.83–1.00) | 0.29 / 0.74 / 0.79 (0.62–0.79) | 65% | 6 |
| idle | 0 | 50 | 8 | 0.12 / 0.29 / 0.42 (0.37–0.46) | 0.17 / 0.33 / 0.42 (0.37–0.46) | 0.42 / 0.79 / 1.12 (0.87–1.20) | 0.29 / 0.74 / 0.79 (0.74–0.79) | 48% | 0 |
| idle | 0 | 200 | 2 | 0.21 / 0.25 / 0.29 (0.29–0.37) | 0.21 / 0.25 / 0.29 (0.25–0.42) | 0.42 / 0.50 / 0.54 (0.50–0.58) | 0.33 / 0.37 / 0.42 (0.37–0.74) | 100% | 0 |
| idle | 0 | 200 | 4 | 0.21 / 0.29 / 3.26 (3.23–3.33) | 0.21 / 0.29 / 3.26 (3.20–3.36) | 0.46 / 0.66 / 1.33 (0.87–2.40) | 0.33 / 0.37 / 1.00 (0.62–1.15) | 99% | 0 |
| idle | 0 | 200 | 6 | 0.29 / 0.42 / 0.54 (0.50–0.54) | 0.29 / 0.42 / 0.50 (0.46–0.54) | 0.62 / 1.00 / 1.25 (1.20–1.33) | 0.37 / 0.83 / 0.87 (0.87–0.87) | 64% | 6 |
| idle | 0 | 200 | 8 | 0.29 / 0.42 / 0.54 (0.46–0.54) | 0.25 / 0.37 / 0.50 (0.50–0.54) | 0.62 / 1.04 / 1.70 (1.57–1.95) | 0.37 / 0.83 / 0.87 (0.70–0.87) | 47% | 0 |
| idle | 0 | 1000 | 2 | 0.58 / 0.62 / 0.70 (0.66–0.74) | 0.58 / 0.62 / 0.70 (0.66–1.00) | 1.15 / 1.25 / 1.28 (1.28–5.18) | 0.70 / 0.74 / 0.87 (0.83–0.91) | 99% | 0 |
| idle | 0 | 1000 | 4 | 0.58 / 0.66 / 3.65 (3.65–3.78) | 0.58 / 0.66 / 3.68 (3.65–3.74) | 1.20 / 1.78 / 5.25 (2.50–9.47) | 0.70 / 0.74 / 1.62 (1.33–3.81) | 99% | 0 |
| idle | 0 | 1000 | 6 | 0.62 / 0.74 / 0.91 (0.83–0.91) | 0.62 / 0.74 / 0.87 (0.83–0.91) | 1.36 / 2.53 / 5.95 (3.58–6.46) | 0.74 / 1.07 / 1.20 (1.20–1.25) | 65% | 6 |
| idle | 0 | 1000 | 8 | 0.62 / 0.74 / 0.87 (0.87–0.91) | 0.66 / 0.74 / 0.87 (0.83–0.91) | 1.41 / 2.94 / 6.59 (4.10–8.06) | 0.74 / 1.07 / 1.20 (1.20–1.25) | 48% | 0 |
| idle | 1000 | 50 | 2 | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.08 / 0.12 / 0.17 (0.17–0.21) | 0.21 / 0.25 / 0.33 (0.33–0.33) | 0.21 / 0.25 / 0.33 (0.33–0.37) | 100% | 0 |
| idle | 1000 | 50 | 4 | 0.08 / 0.17 / 3.20 (3.14–3.33) | 0.08 / 0.21 / 3.23 (3.20–3.36) | 0.21 / 0.29 / 1.12 (0.66–1.25) | 0.21 / 0.33 / 0.95 (0.74–1.07) | 99% | 0 |
| idle | 1000 | 50 | 6 | 0.12 / 0.29 / 0.42 (0.37–0.50) | 0.12 / 0.29 / 0.42 (0.37–0.46) | 0.33 / 0.70 / 0.87 (0.87–0.95) | 0.25 / 0.74 / 0.79 (0.74–0.79) | 64% | 0 |
| idle | 1000 | 50 | 8 | 0.17 / 0.29 / 0.37 (0.37–0.46) | 0.17 / 0.29 / 0.42 (0.37–0.46) | 0.33 / 0.74 / 1.15 (0.87–1.25) | 0.25 / 0.74 / 0.78 (0.74–0.79) | 48% | 0 |
| idle | 1000 | 200 | 2 | 0.21 / 0.25 / 0.33 (0.25–0.33) | 0.21 / 0.25 / 0.33 (0.25–0.37) | 0.46 / 0.50 / 0.54 (0.50–0.58) | 0.33 / 0.37 / 0.42 (0.42–0.46) | 100% | 0 |
| idle | 1000 | 200 | 4 | 0.21 / 0.29 / 3.26 (3.23–3.33) | 0.21 / 0.33 / 3.39 (3.23–3.39) | 0.46 / 0.62 / 1.28 (0.95–1.28) | 0.33 / 0.37 / 1.20 (0.83–1.25) | 99% | 0 |
| idle | 1000 | 200 | 6 | 0.25 / 0.37 / 0.54 (0.50–0.58) | 0.25 / 0.37 / 0.54 (0.50–0.54) | 0.54 / 0.91 / 1.20 (1.12–1.25) | 0.37 / 0.79 / 0.87 (0.70–0.87) | 64% | 1 |
| idle | 1000 | 200 | 8 | 0.25 / 0.37 / 0.50 (0.46–0.58) | 0.25 / 0.37 / 0.54 (0.46–0.54) | 0.54 / 1.00 / 1.41 (1.33–1.95) | 0.33 / 0.83 / 0.83 (0.83–0.87) | 48% | 0 |
| idle | 1000 | 1000 | 2 | 0.58 / 0.62 / 0.66 (0.62–0.74) | 0.58 / 0.62 / 0.70 (0.62–0.70) | 1.15 / 1.25 / 1.28 (1.25–5.25) | 0.70 / 0.74 / 0.83 (0.79–0.87) | 100% | 0 |
| idle | 1000 | 1000 | 4 | 0.58 / 0.66 / 3.74 (3.62–3.78) | 0.58 / 0.66 / 3.74 (3.65–3.78) | 1.20 / 1.78 / 5.38 (2.21–8.19) | 0.70 / 0.74 / 1.54 (1.25–1.70) | 99% | 0 |
| idle | 1000 | 1000 | 6 | 0.62 / 0.74 / 0.87 (0.83–3.74) | 0.62 / 0.74 / 0.91 (0.87–0.91) | 1.36 / 2.40 / 5.82 (3.20–7.74) | 0.70 / 1.07 / 1.20 (1.15–1.25) | 65% | 5 |
| idle | 1000 | 1000 | 8 | 0.62 / 0.74 / 0.91 (0.83–0.91) | 0.62 / 0.74 / 0.91 (0.87–0.91) | 1.41 / 2.66 / 6.46 (3.26–7.36) | 0.70 / 1.07 / 1.25 (1.15–1.25) | 48% | 0 |

Signals that found the thread waiting (took the lock), % of signals

| regime | ipi ns | W ns | k | 1 locked apply | 1 OR apply | 3 notify lock | 3 atomic | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|
| as a3t | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 99.8% | 100% | 0 |
| as a3t | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 66.4% | 99% | 0 |
| as a3t | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 52.0% | 67% | 65 |
| as a3t | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 42.1% | 50% | 39 |
| as a3t | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 99.8% | 100% | 0 |
| as a3t | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 67.8% | 99% | 0 |
| as a3t | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 52.1% | 67% | 44 |
| as a3t | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 40.6% | 50% | 45 |
| as a3t | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 99.8% | 100% | 0 |
| as a3t | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 66.7% | 99% | 0 |
| as a3t | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 47.2% | 66% | 59 |
| as a3t | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 33.9% | 49% | 49 |
| as a3t | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 98.4% | 100% | 0 |
| as a3t | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 66.5% | 100% | 0 |
| as a3t | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 38.3% | 67% | 36 |
| as a3t | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 29.0% | 50% | 70 |
| as a3t | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 99.0% | 100% | 0 |
| as a3t | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 67.0% | 99% | 0 |
| as a3t | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 36.6% | 66% | 41 |
| as a3t | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 27.4% | 50% | 24 |
| as a3t | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 99.0% | 100% | 0 |
| as a3t | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 66.8% | 99% | 0 |
| as a3t | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 34.2% | 66% | 69 |
| as a3t | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 15.8% | 50% | 62 |
| busy | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 1 |
| busy | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 67% | 25 |
| busy | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 22 |
| busy | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 33 |
| busy | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 51 |
| busy | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 56 |
| busy | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 51 |
| busy | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 67% | 33 |
| busy | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 50% | 22 |
| busy | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 37 |
| busy | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 33 |
| busy | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 0.0% | 100% | 0 |
| busy | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 0.0% | 99% | 0 |
| busy | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 0.0% | 66% | 76 |
| busy | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 0.0% | 49% | 67 |
| idle | 0 | 50 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 0 | 50 | 4 | 0.0% | 0.0% | 0.0% | 97.4% | 100% | 0 |
| idle | 0 | 50 | 6 | 0.0% | 0.0% | 0.0% | 92.4% | 65% | 6 |
| idle | 0 | 50 | 8 | 0.0% | 0.0% | 0.0% | 86.2% | 48% | 0 |
| idle | 0 | 200 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 0 | 200 | 4 | 0.0% | 0.0% | 0.0% | 96.0% | 99% | 0 |
| idle | 0 | 200 | 6 | 0.0% | 0.0% | 0.0% | 90.0% | 64% | 6 |
| idle | 0 | 200 | 8 | 0.0% | 0.0% | 0.0% | 83.4% | 47% | 0 |
| idle | 0 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 99% | 0 |
| idle | 0 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 91.1% | 99% | 0 |
| idle | 0 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 83.9% | 65% | 6 |
| idle | 0 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 77.3% | 48% | 0 |
| idle | 1000 | 50 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 50 | 4 | 0.0% | 0.0% | 0.0% | 87.1% | 99% | 0 |
| idle | 1000 | 50 | 6 | 0.0% | 0.0% | 0.0% | 79.5% | 64% | 0 |
| idle | 1000 | 50 | 8 | 0.0% | 0.0% | 0.0% | 70.7% | 48% | 0 |
| idle | 1000 | 200 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 200 | 4 | 0.0% | 0.0% | 0.0% | 83.4% | 99% | 0 |
| idle | 1000 | 200 | 6 | 0.0% | 0.0% | 0.0% | 77.0% | 64% | 1 |
| idle | 1000 | 200 | 8 | 0.0% | 0.0% | 0.0% | 69.5% | 48% | 0 |
| idle | 1000 | 1000 | 2 | 0.0% | 0.0% | 0.0% | 100.0% | 100% | 0 |
| idle | 1000 | 1000 | 4 | 0.0% | 0.0% | 0.0% | 82.4% | 99% | 0 |
| idle | 1000 | 1000 | 6 | 0.0% | 0.0% | 0.0% | 73.1% | 65% | 5 |
| idle | 1000 | 1000 | 8 | 0.0% | 0.0% | 0.0% | 64.4% | 48% | 0 |

Migrations seen: 210792 over 337839527 samples (one sample per measured operation).


### Suite br: unmap, lock per address-space range

load average before: { 5.74 5.50 4.94 }
load average after: { 5.65 5.52 5.04 }; 168 s

Throughput, unmaps/s, median (min–max)

| initiators | ipi ns | W ns | k | 1 fair | 3 group lock | 3 range locks | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 509k (493k–511k) | 492k (489k–493k) | 505k (502k–506k) | 100% | 0 |
| one | 0 | 50 | 3 | 478k (473k–494k) | 480k (479k–481k) | 485k (485k–487k) | 100% | 0 |
| one | 0 | 50 | 4 | 466k (455k–469k) | 467k (464k–469k) | 474k (473k–475k) | 99% | 0 |
| one | 0 | 50 | 6 | 376k (369k–383k) | 409k (402k–412k) | 417k (416k–418k) | 66% | 18 |
| one | 0 | 50 | 8 | 350k (346k–355k) | 386k (384k–388k) | 392k (383k–395k) | 51% | 13 |
| one | 0 | 200 | 2 | 445k (442k–465k) | 466k (465k–468k) | 478k (477k–479k) | 100% | 0 |
| one | 0 | 200 | 3 | 452k (437k–453k) | 451k (450k–452k) | 459k (458k–460k) | 100% | 0 |
| one | 0 | 200 | 4 | 418k (411k–441k) | 439k (378k–441k) | 448k (445k–448k) | 98% | 6 |
| one | 0 | 200 | 6 | 351k (341k–355k) | 386k (383k–390k) | 397k (388k–399k) | 69% | 26 |
| one | 0 | 200 | 8 | 328k (321k–333k) | 367k (360k–372k) | 374k (371k–377k) | 52% | 17 |
| one | 0 | 1000 | 2 | 320k (309k–321k) | 341k (340k–342k) | 345k (344k–346k) | 100% | 0 |
| one | 0 | 1000 | 3 | 314k (305k–314k) | 331k (330k–332k) | 337k (336k–337k) | 100% | 0 |
| one | 0 | 1000 | 4 | 303k (296k–309k) | 325k (324k–327k) | 329k (326k–330k) | 100% | 0 |
| one | 0 | 1000 | 6 | 264k (250k–267k) | 295k (290k–298k) | 301k (298k–303k) | 68% | 25 |
| one | 0 | 1000 | 8 | 250k (244k–251k) | 282k (278k–288k) | 291k (289k–293k) | 48% | 17 |
| one | 1000 | 50 | 2 | 336k (330k–338k) | 328k (328k–329k) | 334k (334k–335k) | 100% | 0 |
| one | 1000 | 50 | 3 | 324k (323k–329k) | 323k (322k–324k) | 325k (325k–327k) | 100% | 0 |
| one | 1000 | 50 | 4 | 317k (313k–319k) | 316k (311k–317k) | 320k (318k–320k) | 99% | 0 |
| one | 1000 | 50 | 6 | 267k (262k–271k) | 284k (277k–286k) | 288k (286k–292k) | 67% | 38 |
| one | 1000 | 50 | 8 | 252k (248k–254k) | 274k (270k–276k) | 277k (275k–279k) | 52% | 16 |
| one | 1000 | 200 | 2 | 316k (306k–317k) | 317k (317k–317k) | 322k (322k–323k) | 100% | 0 |
| one | 1000 | 200 | 3 | 310k (299k–311k) | 309k (309k–310k) | 313k (312k–314k) | 100% | 0 |
| one | 1000 | 200 | 4 | 295k (268k–303k) | 304k (299k–305k) | 307k (306k–308k) | 99% | 0 |
| one | 1000 | 200 | 6 | 252k (250k–258k) | 277k (271k–278k) | 280k (271k–281k) | 64% | 25 |
| one | 1000 | 200 | 8 | 241k (238k–244k) | 266k (262k–267k) | 268k (261k–270k) | 49% | 16 |
| one | 1000 | 1000 | 2 | 242k (236k–243k) | 254k (251k–254k) | 256k (253k–257k) | 100% | 0 |
| one | 1000 | 1000 | 3 | 234k (231k–234k) | 248k (245k–249k) | 251k (246k–252k) | 100% | 0 |
| one | 1000 | 1000 | 4 | 228k (228k–231k) | 245k (244k–245k) | 247k (246k–248k) | 100% | 0 |
| one | 1000 | 1000 | 6 | 200k (195k–208k) | 224k (220k–227k) | 226k (224k–230k) | 68% | 33 |
| one | 1000 | 1000 | 8 | 197k (189k–201k) | 220k (214k–222k) | 221k (217k–224k) | 52% | 16 |
| all | 0 | 50 | 2 | 882k (868k–886k) | 816k (812k–822k) | 963k (958k–966k) | 100% | 0 |
| all | 0 | 50 | 3 | 1.06M (1.05M–1.07M) | 985k (981k–997k) | 1.27M (1.27M–1.27M) | 100% | 0 |
| all | 0 | 50 | 4 | 1.17M (1.17M–1.19M) | 1.09M (1.07M–1.11M) | 1.50M (1.49M–1.51M) | 100% | 0 |
| all | 0 | 50 | 6 | 1.05M (1.02M–1.06M) | 1.06M (1.04M–1.08M) | 1.55M (1.52M–1.56M) | 68% | 166 |
| all | 0 | 50 | 8 | 1.01M (1.00M–1.01M) | 1.05M (1.00M–1.06M) | 1.70M (1.68M–1.73M) | 50% | 109 |
| all | 0 | 200 | 2 | 829k (821k–835k) | 738k (735k–739k) | 905k (903k–908k) | 100% | 0 |
| all | 0 | 200 | 3 | 969k (954k–981k) | 867k (867k–869k) | 1.18M (1.17M–1.19M) | 100% | 0 |
| all | 0 | 200 | 4 | 1.05M (1.04M–1.08M) | 942k (933k–947k) | 1.39M (1.39M–1.40M) | 100% | 0 |
| all | 0 | 200 | 6 | 969k (961k–988k) | 922k (916k–933k) | 1.48M (1.47M–1.50M) | 67% | 163 |
| all | 0 | 200 | 8 | 911k (832k–920k) | 939k (909k–946k) | 1.64M (1.59M–1.64M) | 50% | 249 |
| all | 0 | 1000 | 2 | 496k (491k–501k) | 464k (463k–466k) | 644k (643k–646k) | 100% | 0 |
| all | 0 | 1000 | 3 | 604k (600k–606k) | 513k (510k–518k) | 823k (822k–828k) | 100% | 0 |
| all | 0 | 1000 | 4 | 655k (649k–662k) | 539k (538k–543k) | 954k (951k–957k) | 100% | 0 |
| all | 0 | 1000 | 6 | 591k (580k–604k) | 510k (489k–513k) | 1.03M (1.01M–1.04M) | 67% | 208 |
| all | 0 | 1000 | 8 | 558k (556k–564k) | 513k (511k–520k) | 1.15M (1.13M–1.15M) | 50% | 125 |
| all | 1000 | 50 | 2 | 460k (456k–461k) | 808k (804k–812k) | 911k (909k–916k) | 100% | 0 |
| all | 1000 | 50 | 3 | 623k (621k–632k) | 969k (951k–987k) | 1.24M (1.22M–1.24M) | 100% | 0 |
| all | 1000 | 50 | 4 | 732k (714k–753k) | 1.07M (1.06M–1.08M) | 1.11M (987k–1.25M) | 100% | 0 |
| all | 1000 | 50 | 6 | 740k (735k–751k) | 879k (869k–890k) | 1.08M (1.04M–1.08M) | 67% | 158 |
| all | 1000 | 50 | 8 | 779k (764k–784k) | 904k (901k–910k) | 1.20M (1.17M–1.21M) | 50% | 173 |
| all | 1000 | 200 | 2 | 450k (441k–466k) | 738k (731k–738k) | 903k (899k–906k) | 100% | 0 |
| all | 1000 | 200 | 3 | 590k (576k–593k) | 859k (855k–864k) | 1.17M (1.17M–1.17M) | 100% | 0 |
| all | 1000 | 200 | 4 | 674k (662k–681k) | 938k (920k–943k) | 1.36M (1.36M–1.38M) | 100% | 4 |
| all | 1000 | 200 | 6 | 692k (679k–695k) | 831k (811k–847k) | 1.12M (1.11M–1.12M) | 67% | 190 |
| all | 1000 | 200 | 8 | 723k (716k–735k) | 827k (822k–840k) | 1.24M (1.23M–1.25M) | 49% | 190 |
| all | 1000 | 1000 | 2 | 367k (366k–375k) | 464k (461k–465k) | 643k (639k–646k) | 100% | 0 |
| all | 1000 | 1000 | 3 | 439k (425k–440k) | 511k (511k–514k) | 822k (820k–825k) | 100% | 0 |
| all | 1000 | 1000 | 4 | 479k (469k–497k) | 537k (537k–546k) | 954k (947k–957k) | 100% | 0 |
| all | 1000 | 1000 | 6 | 496k (468k–500k) | 489k (489k–496k) | 1.02M (1.02M–1.04M) | 67% | 165 |
| all | 1000 | 1000 | 8 | 495k (488k–500k) | 488k (486k–489k) | 1.13M (1.12M–1.14M) | 50% | 101 |

Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| initiators | ipi ns | W ns | k | 1 fair | 3 group lock | 3 range locks | on P | stalls |
|---|---|---|---|---|---|---|---|---|
| one | 0 | 50 | 2 | 0.58 / 0.74 / 0.91 (0.83–2.66) | 0.66 / 0.74 / 1.15 (0.83–5.12) | 0.62 / 0.74 / 1.07 (0.79–1.33) | 100% | 0 |
| one | 0 | 50 | 3 | 0.70 / 0.87 / 5.06 (3.90–5.95) | 0.70 / 0.83 / 4.86 (3.90–6.14) | 0.70 / 0.79 / 5.18 (4.42–5.44) | 100% | 0 |
| one | 0 | 50 | 4 | 0.74 / 1.04 / 8.19 (4.80–8.58) | 0.74 / 0.87 / 7.23 (5.18–8.19) | 0.70 / 0.83 / 7.49 (4.61–8.13) | 99% | 0 |
| one | 0 | 50 | 6 | 1.28 / 1.66 / 7.62 (6.66–9.60) | 1.07 / 1.25 / 7.30 (5.18–7.49) | 1.00 / 1.20 / 6.91 (5.31–7.68) | 66% | 18 |
| one | 0 | 50 | 8 | 1.46 / 1.90 / 8.83 (6.98–9.22) | 1.20 / 1.36 / 8.06 (7.55–8.83) | 1.15 / 1.33 / 7.87 (6.21–8.83) | 51% | 13 |
| one | 0 | 200 | 2 | 0.87 / 1.04 / 3.46 (1.12–4.03) | 0.78 / 0.91 / 2.00 (0.95–4.48) | 0.70 / 0.83 / 1.04 (0.95–4.61) | 100% | 0 |
| one | 0 | 200 | 3 | 0.83 / 1.00 / 4.86 (4.16–5.95) | 0.83 / 0.95 / 5.12 (3.68–6.21) | 0.83 / 0.91 / 5.76 (4.16–6.27) | 100% | 0 |
| one | 0 | 200 | 4 | 1.00 / 1.20 / 8.58 (6.78–8.83) | 0.87 / 1.20 / 8.45 (7.49–11.26) | 0.83 / 0.95 / 7.55 (6.14–8.32) | 98% | 6 |
| one | 0 | 200 | 6 | 1.46 / 1.82 / 8.00 (6.91–8.45) | 1.20 / 1.36 / 7.42 (7.04–8.19) | 1.12 / 1.33 / 7.10 (6.27–7.30) | 69% | 26 |
| one | 0 | 200 | 8 | 1.70 / 2.14 / 9.60 (8.70–10.24) | 1.33 / 1.49 / 8.70 (7.42–9.47) | 1.28 / 1.46 / 8.45 (7.55–9.22) | 52% | 17 |
| one | 0 | 1000 | 2 | 1.74 / 1.87 / 6.14 (1.95–7.04) | 1.57 / 1.66 / 5.63 (1.79–6.98) | 1.54 / 1.62 / 5.70 (1.70–6.40) | 100% | 0 |
| one | 0 | 1000 | 3 | 1.79 / 2.00 / 8.45 (7.49–8.96) | 1.66 / 1.74 / 8.13 (5.06–9.09) | 1.58 / 1.70 / 6.91 (5.44–8.45) | 100% | 0 |
| one | 0 | 1000 | 4 | 1.90 / 2.24 / 11.65 (9.22–12.42) | 1.66 / 1.78 / 10.24 (9.09–11.78) | 1.66 / 1.82 / 10.50 (10.24–11.78) | 100% | 0 |
| one | 0 | 1000 | 6 | 2.40 / 2.82 / 10.11 (8.70–10.50) | 2.00 / 2.21 / 9.60 (7.87–10.11) | 1.90 / 2.14 / 8.96 (8.58–10.24) | 68% | 25 |
| one | 0 | 1000 | 8 | 2.62 / 3.10 / 11.01 (9.47–12.03) | 2.14 / 2.30 / 10.11 (8.32–11.14) | 2.11 / 2.27 / 9.86 (9.47–10.62) | 48% | 17 |
| one | 1000 | 50 | 2 | 1.58 / 1.74 / 6.40 (1.87–6.66) | 1.66 / 1.78 / 6.53 (1.87–6.59) | 1.62 / 1.74 / 4.16 (1.87–6.34) | 100% | 0 |
| one | 1000 | 50 | 3 | 1.70 / 1.87 / 8.45 (5.82–8.70) | 1.70 / 1.82 / 8.58 (5.57–9.86) | 1.70 / 1.79 / 8.13 (5.38–8.83) | 100% | 0 |
| one | 1000 | 50 | 4 | 1.74 / 1.95 / 10.62 (8.70–11.78) | 1.74 / 2.03 / 12.42 (9.34–13.31) | 1.70 / 1.82 / 10.88 (9.98–12.42) | 99% | 0 |
| one | 1000 | 50 | 6 | 2.30 / 2.72 / 10.24 (8.58–11.14) | 2.08 / 2.27 / 9.09 (7.74–10.24) | 2.03 / 2.27 / 8.83 (7.42–9.73) | 67% | 38 |
| one | 1000 | 50 | 8 | 2.56 / 3.14 / 11.39 (9.22–13.18) | 2.24 / 2.43 / 12.03 (8.70–13.31) | 2.21 / 2.37 / 10.24 (9.22–11.90) | 52% | 16 |
| one | 1000 | 200 | 2 | 1.79 / 1.90 / 6.53 (2.00–7.55) | 1.78 / 1.90 / 6.59 (2.00–7.55) | 1.70 / 1.87 / 6.59 (1.95–7.04) | 100% | 0 |
| one | 1000 | 200 | 3 | 1.82 / 2.08 / 9.34 (6.02–9.98) | 1.82 / 1.95 / 8.32 (7.62–9.34) | 1.82 / 1.90 / 8.58 (5.95–10.37) | 100% | 0 |
| one | 1000 | 200 | 4 | 2.00 / 2.21 / 11.90 (9.34–20.99) | 1.87 / 2.00 / 11.78 (9.98–15.10) | 1.82 / 2.08 / 11.65 (9.34–12.54) | 99% | 0 |
| one | 1000 | 200 | 6 | 2.50 / 2.94 / 10.37 (9.34–10.75) | 2.21 / 2.43 / 9.73 (8.19–10.37) | 2.14 / 2.40 / 9.60 (7.81–10.11) | 64% | 25 |
| one | 1000 | 200 | 8 | 2.72 / 3.23 / 11.52 (11.14–12.93) | 2.37 / 2.78 / 10.62 (9.98–13.06) | 2.30 / 2.91 / 11.26 (8.83–16.26) | 49% | 16 |
| one | 1000 | 1000 | 2 | 2.72 / 2.85 / 7.87 (3.04–15.62) | 2.56 / 2.69 / 7.36 (6.53–11.01) | 2.53 / 2.66 / 7.30 (2.72–8.83) | 100% | 0 |
| one | 1000 | 1000 | 3 | 2.85 / 3.04 / 12.54 (7.10–18.69) | 2.62 / 2.72 / 10.62 (7.30–16.00) | 2.56 / 2.69 / 10.62 (8.45–16.90) | 100% | 0 |
| one | 1000 | 1000 | 4 | 2.94 / 3.39 / 15.87 (14.08–16.64) | 2.66 / 2.82 / 13.31 (12.54–14.72) | 2.62 / 2.72 / 13.44 (10.62–14.46) | 100% | 0 |
| one | 1000 | 1000 | 6 | 3.52 / 4.10 / 13.82 (11.01–16.38) | 3.04 / 3.26 / 10.75 (9.60–12.42) | 2.94 / 3.23 / 11.78 (9.34–12.54) | 68% | 33 |
| one | 1000 | 1000 | 8 | 3.68 / 4.61 / 14.72 (11.39–35.33) | 3.20 / 3.81 / 12.16 (10.62–15.23) | 3.10 / 4.10 / 12.93 (12.03–16.26) | 52% | 16 |
| all | 0 | 50 | 2 | 0.74 / 1.07 / 3.33 (1.20–4.74) | 1.07 / 1.25 / 3.10 (1.33–4.99) | 0.66 / 1.07 / 2.14 (1.20–4.54) | 100% | 0 |
| all | 0 | 50 | 3 | 1.00 / 1.58 / 6.91 (4.42–7.49) | 1.66 / 1.82 / 7.30 (5.38–8.06) | 0.95 / 1.54 / 5.44 (5.12–5.76) | 100% | 0 |
| all | 0 | 50 | 4 | 1.07 / 1.87 / 10.62 (7.49–10.88) | 2.27 / 2.50 / 11.78 (9.60–13.82) | 1.25 / 1.87 / 8.96 (7.74–9.98) | 100% | 0 |
| all | 0 | 50 | 6 | 2.40 / 5.44 / 13.57 (11.14–14.08) | 4.03 / 5.63 / 12.93 (12.54–15.62) | 1.87 / 3.33 / 9.34 (7.23–10.37) | 68% | 166 |
| all | 0 | 50 | 8 | 4.06 / 10.24 / 16.64 (14.59–18.18) | 6.14 / 8.45 / 17.41 (16.00–38.91) | 2.21 / 4.48 / 11.26 (8.96–15.62) | 50% | 109 |
| all | 0 | 200 | 2 | 1.00 / 1.20 / 5.12 (1.33–5.44) | 1.36 / 1.49 / 4.99 (1.66–5.63) | 0.79 / 1.33 / 2.14 (1.41–5.12) | 100% | 0 |
| all | 0 | 200 | 3 | 1.41 / 1.82 / 8.13 (5.57–8.58) | 2.08 / 2.24 / 8.58 (5.89–9.60) | 1.07 / 1.95 / 6.40 (4.74–6.66) | 100% | 0 |
| all | 0 | 200 | 4 | 1.36 / 2.53 / 12.80 (10.24–13.70) | 2.82 / 3.07 / 12.80 (11.65–14.34) | 1.46 / 2.24 / 9.22 (6.34–10.37) | 100% | 0 |
| all | 0 | 200 | 6 | 2.82 / 6.14 / 13.44 (10.37–14.08) | 4.99 / 6.34 / 14.34 (13.31–15.49) | 2.21 / 3.87 / 10.50 (8.13–10.75) | 67% | 163 |
| all | 0 | 200 | 8 | 4.48 / 11.65 / 18.69 (16.90–196.61) | 7.17 / 9.22 / 19.46 (16.38–19.46) | 2.53 / 5.12 / 13.18 (10.24–37.38) | 50% | 249 |
| all | 0 | 1000 | 2 | 2.62 / 2.78 / 8.00 (3.14–8.13) | 2.94 / 3.07 / 8.70 (6.08–10.75) | 1.58 / 2.91 / 5.82 (3.04–6.40) | 100% | 0 |
| all | 0 | 1000 | 3 | 2.43 / 4.16 / 11.14 (8.19–19.71) | 4.42 / 4.61 / 14.21 (10.75–16.38) | 1.87 / 4.29 / 9.34 (6.40–10.11) | 100% | 0 |
| all | 0 | 1000 | 4 | 2.78 / 5.63 / 18.18 (15.62–18.18) | 5.95 / 6.85 / 22.53 (20.22–23.81) | 3.04 / 4.61 / 13.06 (10.88–13.82) | 100% | 0 |
| all | 0 | 1000 | 6 | 6.34 / 13.70 / 20.48 (19.20–29.18) | 10.50 / 11.39 / 23.55 (22.02–25.09) | 4.16 / 7.30 / 12.80 (11.01–13.70) | 67% | 208 |
| all | 0 | 1000 | 8 | 10.37 / 19.46 / 29.18 (26.11–29.95) | 14.59 / 16.90 / 28.42 (23.04–28.93) | 5.31 / 9.34 / 14.59 (13.44–16.38) | 50% | 125 |
| all | 1000 | 50 | 2 | 1.62 / 1.79 / 6.66 (2.21–7.36) | 1.07 / 1.62 / 5.63 (1.87–6.14) | 0.66 / 1.70 / 4.10 (1.82–4.29) | 100% | 0 |
| all | 1000 | 50 | 3 | 2.08 / 3.36 / 10.11 (6.66–11.90) | 1.66 / 1.90 / 8.32 (5.89–8.58) | 0.95 / 1.78 / 6.66 (5.57–7.30) | 100% | 0 |
| all | 1000 | 50 | 4 | 2.21 / 4.16 / 14.85 (13.31–16.38) | 2.24 / 2.50 / 12.29 (9.60–12.80) | 1.57 / 2.00 / 11.52 (9.73–12.29) | 100% | 0 |
| all | 1000 | 50 | 6 | 3.52 / 6.85 / 14.85 (13.31–15.87) | 3.52 / 5.63 / 13.31 (11.90–13.70) | 2.21 / 3.68 / 10.62 (9.34–12.42) | 67% | 158 |
| all | 1000 | 50 | 8 | 5.06 / 9.34 / 18.18 (16.64–22.02) | 4.42 / 8.19 / 16.90 (16.38–17.41) | 2.43 / 4.74 / 13.06 (10.62–17.15) | 50% | 173 |
| all | 1000 | 200 | 2 | 1.70 / 1.90 / 6.78 (2.08–7.49) | 1.36 / 1.49 / 5.57 (2.37–6.14) | 0.79 / 1.33 / 3.68 (1.87–4.48) | 100% | 0 |
| all | 1000 | 200 | 3 | 2.03 / 3.78 / 10.37 (8.32–11.78) | 2.08 / 2.24 / 9.22 (7.87–10.50) | 1.07 / 1.95 / 7.23 (4.86–7.87) | 100% | 0 |
| all | 1000 | 200 | 4 | 2.53 / 4.80 / 16.13 (14.59–18.18) | 2.82 / 3.14 / 13.70 (11.39–14.21) | 1.46 / 2.24 / 9.86 (7.04–10.88) | 100% | 4 |
| all | 1000 | 200 | 6 | 4.03 / 7.55 / 16.90 (15.62–17.41) | 4.74 / 6.46 / 13.70 (13.31–16.26) | 2.30 / 4.00 / 10.75 (8.58–11.26) | 67% | 190 |
| all | 1000 | 200 | 8 | 5.70 / 10.11 / 19.97 (18.43–21.25) | 5.95 / 9.34 / 18.43 (17.41–21.76) | 2.66 / 5.25 / 13.06 (10.37–13.57) | 49% | 190 |
| all | 1000 | 1000 | 2 | 2.56 / 2.85 / 8.32 (3.04–8.70) | 2.94 / 3.07 / 8.45 (3.23–8.96) | 1.58 / 2.91 / 6.98 (2.98–7.62) | 100% | 0 |
| all | 1000 | 1000 | 3 | 2.98 / 5.76 / 13.31 (12.29–14.72) | 4.42 / 4.61 / 13.95 (9.47–14.59) | 1.87 / 4.29 / 9.60 (6.40–11.26) | 100% | 0 |
| all | 1000 | 1000 | 4 | 4.74 / 6.91 / 22.02 (18.69–28.42) | 5.95 / 6.27 / 22.78 (17.66–26.88) | 3.04 / 4.61 / 12.93 (9.98–15.87) | 100% | 0 |
| all | 1000 | 1000 | 6 | 6.98 / 11.78 / 21.50 (19.97–24.58) | 10.50 / 12.54 / 23.55 (18.94–27.14) | 4.16 / 7.36 / 13.06 (11.39–14.08) | 67% | 165 |
| all | 1000 | 1000 | 8 | 9.86 / 20.22 / 25.60 (24.83–27.14) | 14.98 / 17.41 / 27.65 (24.83–30.46) | 5.31 / 9.47 / 15.49 (12.93–16.90) | 50% | 101 |

Migrations seen: 82478 over 85217220 samples (one sample per measured operation).


### Suite srv: a passive server shared by a background and a foreground client

load average before: { 5.65 5.52 5.04 }
load average after: { 4.62 4.91 4.87 }; 229 s

Foreground call latency, µs: p50 / p99 / p99.9 (p99.9 min–max)

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 52.22 / 172.03 / 176.13 (176.13–178.18) | 52.74 / 172.03 / 176.13 (174.08–176.13) | 51.20 / 172.03 / 176.13 (176.13–180.22) | 32.26 / 57.86 / 59.39 (58.37–74.75) | 32.77 / 57.86 / 65.54 (59.39–82.94) | 32.77 / 57.86 / 60.42 (59.39–503.81) | 6.53 / 56.83 / 58.37 (57.86–72.70) | 6.53 / 57.34 / 58.37 (58.37–62.98) | 6.53 / 56.83 / 58.37 (58.37–149.50) | 100% | 12 |
| 1 | on | 1000 | 51.20 / 169.98 / 172.03 (172.03–174.08) | 51.20 / 167.94 / 174.08 (172.03–174.08) | 50.69 / 169.98 / 174.08 (174.08–186.37) | 32.77 / 58.88 / 64.00 (60.93–81.92) | 33.79 / 59.39 / 65.54 (60.93–73.73) | 33.79 / 59.90 / 60.42 (60.42–74.75) | 8.58 / 57.86 / 61.95 (59.39–74.75) | 8.45 / 57.86 / 60.93 (58.88–64.51) | 8.45 / 59.39 / 60.42 (59.90–76.80) | 100% | 0 |
| 1 | off | 0 | 6.40 / 9.47 / 28.42 (20.99–37.89) | 6.40 / 9.34 / 18.43 (14.21–29.18) | 6.40 / 7.36 / 34.82 (15.49–47.62) | 6.40 / 9.47 / 28.93 (15.36–31.49) | 6.40 / 9.22 / 27.65 (16.90–29.70) | 6.40 / 6.66 / 16.00 (12.54–19.46) | 6.40 / 8.00 / 33.79 (14.46–43.52) | 6.40 / 9.60 / 30.21 (17.15–36.35) | 6.40 / 6.91 / 20.48 (10.11–1916.93) | 100% | 3 |
| 1 | off | 1000 | 8.45 / 11.90 / 43.01 (23.55–52.22) | 8.45 / 11.90 / 35.84 (21.76–35.84) | 8.45 / 8.96 / 28.67 (25.09–35.84) | 8.45 / 11.65 / 26.11 (25.86–38.40) | 8.45 / 11.78 / 35.33 (18.69–35.84) | 8.45 / 11.52 / 26.11 (17.66–36.86) | 8.45 / 11.14 / 26.88 (16.90–42.50) | 8.45 / 11.90 / 30.72 (20.99–40.45) | 8.45 / 9.47 / 32.26 (16.00–39.94) | 100% | 0 |
| 2 | on | 0 | 6.66 / 10.75 / 38.40 (30.21–39.94) | 6.53 / 10.50 / 38.40 (27.39–43.52) | 6.40 / 9.47 / 26.62 (25.86–34.30) | 6.66 / 10.62 / 37.38 (25.34–56.32) | 6.53 / 10.24 / 36.86 (26.11–49.66) | 6.40 / 9.60 / 33.28 (20.74–58.88) | 6.46 / 10.75 / 37.89 (20.99–43.52) | 6.72 / 10.11 / 25.60 (21.50–42.50) | 6.46 / 9.60 / 28.67 (18.94–43.52) | 99% | 1 |
| 2 | on | 1000 | 9.60 / 15.23 / 32.77 (23.04–43.52) | 8.70 / 12.42 / 24.83 (17.15–50.69) | 8.45 / 11.65 / 26.88 (26.62–34.30) | 9.60 / 14.72 / 41.98 (40.45–45.06) | 8.70 / 12.42 / 29.70 (17.66–55.30) | 8.45 / 11.65 / 25.34 (22.53–38.91) | 8.58 / 13.18 / 40.45 (19.46–48.64) | 8.70 / 13.18 / 41.47 (30.72–589.82) | 8.45 / 11.65 / 38.91 (20.99–53.25) | 99% | 7 |
| 2 | off | 0 | 6.46 / 10.37 / 30.46 (16.26–39.42) | 6.78 / 10.75 / 35.33 (19.97–49.15) | 6.46 / 10.37 / 27.39 (23.55–36.86) | 6.46 / 10.11 / 29.44 (18.18–37.89) | 6.78 / 10.50 / 29.95 (24.58–32.77) | 6.46 / 9.98 / 30.72 (21.25–47.62) | 6.40 / 9.98 / 27.90 (13.31–38.40) | 6.72 / 10.62 / 24.83 (16.38–49.15) | 6.40 / 9.60 / 35.33 (20.22–37.38) | 99% | 1 |
| 2 | off | 1000 | 8.45 / 12.42 / 27.14 (21.25–49.66) | 9.86 / 14.21 / 34.82 (28.42–37.89) | 8.45 / 11.65 / 34.30 (26.88–40.96) | 8.45 / 13.06 / 38.40 (27.39–45.06) | 9.86 / 13.70 / 32.77 (29.70–44.54) | 8.45 / 12.29 / 28.16 (24.58–37.38) | 8.45 / 14.21 / 34.82 (29.18–50.69) | 9.73 / 13.44 / 32.77 (30.98–42.50) | 8.45 / 11.78 / 35.84 (27.39–43.52) | 99% | 0 |

Background calls served/s

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 11k (11k–11k) | 11k (11k–11k) | 11k (11k–11k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 100% | 12 |
| 1 | on | 1000 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 11k (11k–11k) | 11k (11k–11k) | 11k (11k–11k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 100% | 0 |
| 1 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 3 |
| 1 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 0 |
| 2 | on | 0 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 99% | 1 |
| 2 | on | 1000 | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 10k (10k–10k) | 8k (8k–8k) | 8k (8k–8k) | 8k (8k–8k) | 99% | 7 |
| 2 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 1 |
| 2 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 0 |

Calls refused/s

| threads | background | ipi ns | 1 fifo | 3 fifo | 3 all fifo | 1 lend | 3 lend | 3 all lend | 1 refuse | 3 refuse | 3 all refuse | on P | stalls |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | on | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 100% | 12 |
| 1 | on | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 100% | 0 |
| 1 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 3 |
| 1 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 100% | 0 |
| 2 | on | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 99% | 1 |
| 2 | on | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 4k (4k–4k) | 4k (4k–4k) | 4k (4k–4k) | 99% | 7 |
| 2 | off | 0 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 1 |
| 2 | off | 1000 | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 0 (0–0) | 99% | 0 |

Migrations seen: 38482 over 1807898 samples (one sample per measured operation).

