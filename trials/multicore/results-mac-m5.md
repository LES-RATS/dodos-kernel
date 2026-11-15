# Multicore trial: results on the Apple M5

15 November 2026, 00:57 to 01:25. Method and cost model: [README.md](README.md).
Raw runs (one line per run, every class): `data/pass-a.csv.gz`,
`data/pass-b.csv.gz`. Full tables of both passes: appendices A and B below.

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

