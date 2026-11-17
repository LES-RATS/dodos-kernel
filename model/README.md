# The executable model of the kernel contract

`dodos-model` is the reference model of `architecture/kernel-interface.md`
(documentation repository): every rule it implements names the section of
the contract it applies, and a whole check of the contract's invariants
runs after every step of every test. It follows the shape of nodes decided
on 16 November 2026 (D-128, D-129, D-131): a node covers *N* physical cores
fixed when it starts (*N* = 1 is a particular case), the cores of a node
share its state under locks, and nodes share only single-writer words of
the machine table and the eleven kinds of message on two rings per ordered
pair (kind 9 reserved since the sweep's `Ping` was removed, D-135).

## What it holds

- `Model` is the kernel of one node: its objects and capabilities, one part
  per core (`cores.rs`: what the core runs, its queues, timers, interrupt
  table, idle word, invalidation request, pending set, interrupts, loaded
  root and the translations its TLB may hold), its locks (`locks.rs`: the
  trace of each event under variant L1 or L2; under L2 the locks in the
  order the event found them, a lock earlier than one held recorded as a
  try, never sorted; a lock a test scripts as held by another core across
  events makes a try fail, and the event then makes the contract's rounds:
  the second round, `again` with nothing changed, the escalation to the
  quiet word, a `signal` and an arrival that never return `again`), its
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
  of a start, the removal of the `Ping`), `mistral.rs` (one test per mutation of the
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
- Real concurrency under L2: every event runs alone; a failed try comes
  only from a lock a test scripts as held by another core (`Holder`), whose
  event ends, with its effect, when the lock is waited for. The order of
  the trace is the order in which the model's code touches the objects,
  close to but not always a kernel's order of discovery; in particular the
  model locks a woken thread and its reply object by their own words,
  where the contract guards them by the lock of the object waited on.
- The waits for the quiet word and for the entry words of other cores:
  another core is in a kernel event only while a test makes it hold a
  lock, and an escalation lets every such holder end its event.
- The debugger stub and its release word (development images only): the
  suspension of the clocks of membership while it is raised (D-135) has
  no executable check.
