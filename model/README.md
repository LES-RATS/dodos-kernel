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
  the clock less the stopped time), `pass21.rs` (D-140, the thirteenth
  contract review: a detach tied to its requester and refused on the
  wrong unit unless the word reads *detached* with its root, the retired
  word of each core in the machine table through R6 and the next start,
  R1 on a failed unit holding only the nodes whose devices sat behind it
  (and a unit that does not confirm R1 judged failed), the bind's full
  barrier, the start instant of a request read under the lock, the route
  of a line on ARM64 and RISC-V, a refused detach at a destruction step,
  the invalidation of a line's remapping entry at its removal; then the
  twelfth model review judged finding by finding), `pass22.rs` (D-141,
  the final review of pull request 3: a message's sender is its ring's
  writer; a grant freed only when the holder's use entry counts no leaf,
  and the holder maps only after the `SendAnswer` is consumed; the remote
  frame read from the anchor in the grant entry, its rights and depth
  included; `memory_control` placed only as the depth rule allows; a
  debug area or crash header of another boot judged as faulty data; the
  credits and the answer's room, the ring's indices, 16 entries per
  interrupt; then the faults of the model the attacks and the code read
  found: the start-up refusing the machine values out of range instead
  of panicking, the start-up page and its copied frames counted at their
  sizes and places, reserves and frozen entries, the rebuild of a unit
  from its reports (handles, sealed nodes, roots), roles handed twice, a
  core that goes idle answering a request that wakes a thread onto it,
  the resume consuming its rings before it is live and finishing R2 and
  R3 of a generation that ended while it was sealed, a refused start
  parking its core, a spare core off on ARM64 and RISC-V, the gate
  reading its member word for every word it writes, and the check's
  invariants 3, 28 (the `memory_control`), 52 and G4 made exact),
  `pass23.rs` (the rules the final review found held by no test or
  only by a test's name: a request and a core's heartbeat judged across
  a debugger stop on the clock less the stopped time, with no
  accusation of itself while the release word is raised, which the model
  made; the clearing of a rebuild's entries no report claims; each core's
  generator seeded from its node's seed and its number), `verify13.rs`
  (the corrections of the thirteenth model review's verification and a
  test for each of its eleven surviving mutations), `pass24.rs` (D-142: a
  granted frame mapped in an iospace whose unit fails during the holder's
  removal, during a revocation, with a `Release` crossing a `Revoke` and
  before the map; the use entry kept with no root, the grant pinned, the
  granting node's death that is never reclaimed and its control case, and
  a holder resumed beside a fenced or an unfenced granting node, which
  runs again without waiting for the R2 the kept count leaves undone;
  a resume reads its rings from the end of R0 on, so the answer of
  another node's unit to R2's request completes the record, or is
  *failed* and keeps the count, a message of an ended generation is
  still refused, and an answer wakes its thread, which runs only once
  the resume ends),
  `mistral.rs` (one test per mutation of the
  eighth model review that left every test green), `checker.rs` (the
  checker fed a bad state, one per invariant of a node of several cores
  and of the machine; those of one node are in `check/tests.rs`), and
  `explore.rs` (seeded explorations that port the two explorers of the
  research drafts, steered so that each reaches a full ring, a release
  crossing a revoke, a core stopping in a request, an applied `Handed`
  and a failed unit under a granted frame with its holder sealed and
  resumed, its R2 asking another node's unit (and never accused
  stuck), asserted over the seeds; the
  one-node walk of `tests/property.rs` holds no remote frame, so D-142's
  exploration is there). `DODOS_SEED=<n>` replays one seed
  of an exploration with its actions printed; `DODOS_COUNTS` prints what
  the explorations reached.
- `tests/`: the calls on one node, judged argument by argument, and the
  random walk of `property.rs` with its coverage floors: three quarters
  of the counts measured on 17 November 2026 (pass 23), rounded up, the
  smallest of the three page sizes. The directed cases of each seed count
  in no floor, so every floor is above what they give alone
  (`DODOS_PROPERTY_STEPS=0` runs them alone); the kinds seen dying are
  every kind made from an untyped, the irq, the role and the
  memory_control, and a breakpoint hit (a fault of kind 9) is given and
  waited on. `many_seeds`, ignored by `verify`, walks 2,000 more seeds
  (`DODOS_PROPERTY_SEEDS` and `DODOS_PROPERTY_FIRST` set another number
  and the first) and names any seed that fails: `cargo test --release -p
  dodos-model --test property many_seeds -- --ignored`.
- The checker: each of its 220 rules that go through `ensure`, switched
  off alone, makes at least one test fail (pass 23's sweep; the two it
  found held by nothing have a test since; pass 24's rule of G6 for a
  granting generation over under a counted leaf, and invariant 58 counting
  a frozen use entry, each have one); a check that cannot fail is
  deleted or says which earlier rule shadows it, and the checker names an
  invariant for a value of the state it would index, never panics.
- `verify` runs the tests that depend on the page on three geometries (4,
  16 and 32 KB pages); the explorations run once.

## Mutations shown equivalent

- C2 of D-142 in `audited` (no use entry naming the generation counts a
  leaf) is equivalent: `reclaim_step` writes R3 only once R2 is done, and
  R2 is not done under such a count, so `audited`, which needs R3, reads
  false already; the guard is written once, in `reclaim_step` (pass 24).

- The clause of `reclaim_step` that drops, in state dead, a reclaim
  record of an older generation (`r.generation != g`; the tenth model
  review's m25, the twelfth's m33) is equivalent: every reader of a
  `Reclaim` reaches it after `r0` (which replaces a record of another
  generation before R1 to R6 read it) or after the generation check of a
  unit role's take-over (`roles.rs`), so a record of an older generation
  left while the node is dead is replaced before any of its fields is
  read; only the map's contents while the node is dead differ, and
  nothing reads them then (R6's cursor is read only while the node is
  fenced and its barrier holds, after `reclaim_step` ran `r0`). The test
  `deepseek12_m33_the_reclaim_of_a_second_death_of_one_slot_starts_afresh`
  holds the reset of `r0` that makes it so. The tenth review's m18 (a
  start never over) is killed by pass 19's
  `a_start_whose_core_never_takes_its_wake_ends_at_start_ns`.

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
  contract's list are for that). So the check of the sequence words under
  the lock (D-138) has no code here: no event can change a segment
  between a lookup and its lock, and a step that recorded it would be a
  check no state can fail (the twelfth model review, finding 2, judged
  so). What the model holds of the orders is their cost: the bind's full
  barrier on a node of several cores (`Cost::Fence`), and the wait for
  the lock as a time (`Model::lock_wait`, a test's input), so that what is
  read under the lock, a request's start instant, is read after it.
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
- The line controllers of ARM64 and RISC-V (the GIC distributor, an
  APLIC domain) and the ITS: a line is an IOAPIC's input on every
  architecture, its route a redirection entry whose target names a core
  by its hardware identifier, which is how G9's check of the target core
  is tested on ARM64 and RISC-V.
