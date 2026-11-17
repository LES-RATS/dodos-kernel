# The executable model of the kernel contract

`dodos-model` is the reference model of `architecture/kernel-interface.md`
(documentation repository): every rule it implements names the section of
the contract it applies, and a whole check of the contract's invariants
runs after every step of every test. It follows the shape of nodes decided
on 16 November 2026 (D-128, D-129, D-131): a node covers *N* physical cores
fixed when it starts (*N* = 1 is a particular case), the cores of a node
share its state under one node lock (D-138), and nodes share only single-writer words of
the machine table and the eleven kinds of message on two rings per ordered
pair (kind 9 reserved since the sweep's `Ping` was removed, D-135).

## What it holds

- `Model` is the kernel of one node: its objects and capabilities, one part
  per core (`cores.rs`: what the core runs, its queues, timers, interrupt
  table, idle word, invalidation request, pending set, interrupts, loaded
  root and the translations its TLB may hold), its locks (`locks.rs`: the
  trace of each event under the node lock, the gate lock inside it, a
  `signal` that wakes no one taking none; the barrier before a watermark
  returns, a request of the core with nothing to invalidate, is in
  `cores.rs` and `caps.rs`), its
  rings (`rings.rs`: the
  check of every message received, credits, the reserve of answers), its
  grants (`grants.rs`), its unit requests and unit service table
  (`units.rs`), its roles (`roles.rs`), and the membership, fence, reclaim,
  seal, resume and start of nodes (`nodes.rs`). `Model::boot` makes node 0
  alone beside phantom nodes, as on one core.
- `System` (`system.rs`) is the machine: one `Model` per node slot that
  runs, the shared machine table, and the physical cores as the hardware has
  them (running, hung, in the park stub, off). It interleaves the events of
  every core of every node (interrupts, timers, the hardware's actions, the
  park stub's halt), in a fixed order (`settle`, `advance`, `run`) or as a
  seeded generator chooses (`step`), so that a failure replays.
- `check.rs` and `check/` are the invariants, each named after the sentence
  of the contract it checks: those of one node (`Model::check`, with
  `check/node.rs` for the rules of several cores) and those of the whole
  machine (`System::check` in `check/machine.rs`: P2, P3, G4, G6, LEAK,
  CREDIT, RESERVE, ONE-HOLDER, the order of the fence, judged on the trace
  of the event as well as on the state after it).

## Tests

- `src/scenarios/`: scenarios of the new shape, inside the crate so that
  they can read what a node keeps. `cores.rs` (a node of several cores),
  `membership.rs` (the node calls, detection, fence, seal and resume,
  starts), `crossing.rs` (what crosses between nodes), `units.rs` (unit
  requests and roles), `reviews.rs` (one test per scenario the reviews
  broke the text with, and a table of where the others are), `pass17.rs`
  (the corrections of D-134), `pass18.rs` (the ninth model review judged,
  and what D-135 changed: line words, failed tries and the quiet word,
  the watermark ready to return, the state word of a request, the retry
  of a start, the removal of the `Ping`), `pass19.rs` (the tenth model
  review judged finding by finding, and what D-137 changed: the classes of
  an iospace by level and its root's count of devices, the attach words,
  the rebuild from the reports of a device's root and a line's operands,
  the quiet word's masked arrivals and its four orders, the closed bounds
  and the start-up checks, marks without a lock, `recv` before its lends,
  the new *k*, a fault as an event, `task_revoke`'s first call, the
  removal of a line's `irq` as a request, tables freed after their
  invalidation, the stopped time of the debugger, the sweep of sealed
  nodes, the replacement slot), `pass20.rs` (what D-138 left: the
  watermark's barrier, on ARM64 too and before a give or the destruction
  of a block, marks written under the node lock, the lock-free signal, the
  bound of a request and its start-up check, a fault of the model older
  than the pass, a block of a run destroyed on two cores; then D-139, the
  twelfth contract review: the attach word against every event, with a
  device in the header only after its attach's done and a *detaching*
  mark, the bound of the tables a change unhooks, a line's mask in its
  `Report`, an MSI's removal as a request and the handle a failed unit
  did not confirm retired, `idle_until`, the fence record and the waits on
  the clock less the stopped time),
  `mistral.rs` (one test per mutation of the
  eighth model review that left every test green), `checker.rs` (the
  checker fed a bad state, one per invariant of a node of several cores
  and of the machine; those of one node are in `check/tests.rs`), and
  `explore.rs` (seeded explorations that port the two explorers of the
  research drafts, steered so that each reaches a full ring, a release
  crossing a revoke, a core stopping in a request and an applied
  `Handed`, asserted over the seeds). `DODOS_SEED=<n>` replays one seed
  of an exploration with its actions printed; `DODOS_COUNTS` prints what
  the explorations reached.
- `tests/`: the calls on one node, judged argument by argument, and the
  random walk of `property.rs` with its coverage floors (three quarters of
  the counts measured on 17 November 2026, rounded up).
- `verify` runs the tests that depend on the page on three geometries (4,
  16 and 32 KB pages); the explorations run once.

## Where the contract is silent

A behaviour the contract does not fix is marked `CONTRACT-GAP` and a number
in the code, and written in a draft of the documentation repository with
the reading implemented and the text proposed. None is open: the sixteen of
pass 15 (`research/drafts/model-15-contract-gaps.md`) are settled in the
contract since pass 16 (D-133), and each branch that applies one names the
contract's sentence and "gap *n*". Two were settled otherwise than the
model first read them, and the model follows: a node's cores lie in one
window of 64 core numbers (gap 7, checked after every step), and the claim
of an MSI asks every IOMMU unit of the source's segment, one after another
(gap 12).

## What it does not model

- The cost of the hardware: a lock is a step of the trace, not a time; the
  bounds of D-042 are checked by the order and the count of locks, not by a
  clock.
- The insides of the architectures: the fence acts on a core's state as
  `System` holds it (NMI, INIT and SIPI, SDEI, the stop extension), not on
  firmware.
- Nodes that take and give cores at run time: the contract only reserves
  words for it, and so does the model.
- Real concurrency: every event runs alone, as the node lock makes them;
  the lock-free slot read, its check under the lock and the atomic OR of a
  `signal` are not interleaved with other events (the small models of the
  contract's list are for that).
- The lock per object (variant L2): it left the contract of version 1 by
  D-138, and its model left with it: the order by kind, the tries and
  their two rounds, `Model::hold` and the scripted second core, the
  `again` of tries and its count, the escalation to the quiet word, the
  entry words and `locks::entry_orders`, the bound *k* (`locks::bound`),
  their tests and the L2 values of the machine's configuration. The text
  is in `research/drafts/locking-l2.md` of the documentation repository,
  and this branch's history keeps the model (commit `04503d5`). `abi/` had
  no value of L2 to keep: the words the contract reserves for it (lock
  words, the quiet word, the entry words) are not in `abi/` yet.
- The debugger stub of a development image: only its release word and its
  stopped time, as `System::stub_stop` (a test's input) raises and lowers
  them; the clock of membership leaves the stop out for the observations,
  the seal, the evidence, the fence record, the waits (a request's start,
  a unit's submission, a start, a hand-over) and `idle_until`.
- The invalidation of the interrupt entry cache after the masking of a
  line with interrupt remapping, at the removal of its `irq`: the model
  masks the line; the requests for its remapping entry are modelled for
  an MSI only.
