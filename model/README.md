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
  the resume ends), `pass24_deepseek.rs` (DeepSeek's verification of
  pass 24, its eleven tests named after what they hold, each failing when
  the code it guards is reverted: among them the first map before the
  answer, `again` while the granting node is live or sealed and `gone`
  once it is accused, and a block of a device entry under a live grant,
  `busy` for a give and a split), `pass25.rs` (the tables of each VT-d
  unit kept at start-up among the kept ranges, and a machine with no room
  for them refused; then the faults the combined exploration found, each
  with the seed that found it: a rebuild whose last invalidation a unit
  never confirms, a request answered *elsewhere* sent again at once, an
  observation of a generation that ended, a heartbeat counted again from
  0, a node started afresh with its clock at 0 and its cores choosing
  nothing, its debug areas' suppressed entries), `pass26.rs` (D-144, the
  four gaps of pass 25 decided, each with a scenario that fails without
  its correction: R1 by the node that hands a unit role during steps 1
  to 3; a hand-over ended without step 4 writing held with a new epoch,
  its two ends; evidence stuck against a starting node at `START_NS`;
  nothing but an answer written toward a node starting afresh until it
  is live, a rebuild's `ReportAsk` waiting for it or going on without
  it, and the resume bit that keeps a resume under a rebuild from
  waiting for ever; and the late report of a rebuild), `pass27.rs` (D-145,
  gap 21 decided: a seal hands a failed unit's role without the drain,
  the epoch stopping at e + 2; a unit that fails during a seal's drain
  ends that hand-over and is handed at the seal's next step; a take-over
  of a failed unit's role, held or handing, is not a rebuild; at step 1
  the holder of a failed unit answers *failed*),
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
  exploration is there), and `combinations.rs` (pass 25: five nodes
  with two grants, one of 32 frames, two devices, a line claimed in
  turn, chunks given and an entry split, the IOMMU role handed, and every
  node accused by each kind of evidence, fenced or not, sealed, resumed
  and started again, at the low end of the times of membership; each
  seed takes three of thirteen drivers of rare states, every triple in
  turn; 26 events probed after every step, and the seeds that reach each
  pair, each triple of a resume and two others and each triple of a
  death and two others counted; each seed ends with no node accused that
  was not harmed and no wait left; drivers hold the short states open: a
  resume whose first wake is lost, under a rebuild, or beside an
  accusation, and a pending release whose holder's events are held back;
  `verify` runs 26 seeds, and `cargo test --release -p dodos-model --lib
  every_combination -- --ignored` explores 858, `DODOS_COMBINATION_SEEDS`
  another number and `DODOS_COMBINATION_FIRST` the first, and fails on a
  combination no seed reaches that `IMPOSSIBLE` does not exclude; on 18
  November 2026 (pass 27), 1,716 seeds in two ranges reached every pair
  and 753 of the 778 combinations, none excluded since D-145 (a seal hands
  a failed unit), and left 23 triples of a resume and two of a death, so
  that run fails). Every exploration ends
  with the same liveness (`explore::assert_live`): the harness records
  each harm (a stop that succeeds, a hung core, a failure) and each start,
  and a node accused without one, or a start whose starter was harmed or
  sealed, fails the seed.
  `DODOS_SEED=<n>` replays one seed
  of an exploration with its actions printed; `DODOS_COUNTS` prints what
  the explorations reached.
- `tests/`: the calls on one node, judged argument by argument, and the
  random walk of `property.rs` with its coverage floors: three quarters
  of the counts measured on 17 November 2026 (pass 23), rounded up, the
  smallest of the three page sizes; since pass 31 the walk runs 48 seeds
  (24 before: D-154 moved the start-up slots and the chunks, and 24 seeds
  no longer reached the rarest events on every page size), and three
  floors were measured again on 19 November 2026 (a sched dying, a copy
  under `cap_delete`, a thread that lost its context), a fourth added
  (an `its_table` dying). The directed cases of each seed count
  in no floor, so every floor is above what they give alone
  (`DODOS_PROPERTY_STEPS=0` runs them alone); the kinds seen dying are
  every kind made from an untyped, the irq, the role and the
  memory_control, and a breakpoint hit (a fault of kind 9) is given and
  waited on. `many_seeds`, ignored by `verify`, walks 2,000 more seeds
  (`DODOS_PROPERTY_SEEDS` and `DODOS_PROPERTY_FIRST` set another number
  and the first) and names any seed that fails: `cargo test --release -p
  dodos-model --test property many_seeds -- --ignored`.
- The checker: each of its 225 rules that go through `ensure`, switched
  off alone (its condition read as true), makes at least one test fail
  (pass 26's sweep of 224: 210 by a test of the checker, 14 by another
  test of the library; pass 27's rule HANDED by its checker test; pass
  28's new rules, 257 `ensure` calls in all, each by its own checker test,
  the whole sweep not run again); a check that cannot fail is deleted or says which
  earlier rule shadows it, and the checker names an invariant for a value
  of the state it would index, never panics.
- `verify` runs the tests that depend on the page on three geometries (4,
  16 and 32 KB pages); the explorations run once.

## Pass 28 (D-143, D-146 to D-149)

- Sleep that loses the context: `Suspending` and `Asleep`, the events
  `Sleep`, `PowerDown`, `ShallowReturn` and `Wake`; the gate's reads of
  the fence mark before a suspend, at a shallow return and at a wake
  (ARM64), at every return (RISC-V); a timer of a sleeping core taken at
  its wake, at most an exit latency late; the ARM64 fence and the RISC-V
  cooperative fence in rounds of 11.2 ms; `node_idle` reading context loss
  from the parameter, the PSCI format from `PSCI_FEATURES`.
- The first thread runs before live; the combined exploration rotates
  x86-64, ARM64 and RISC-V by round of the 286 triples, with a node of two
  cores whose second start is held back, and probes a wake from sleep and
  a first thread before live.
- Interrupts: the APLIC as machine state (input, pending, enable,
  `sourcecfg`, MSIs on their way), `clrienum` at an arrival, `setipnum`
  then `setienum` at `irq_ack` and an arming bind, a rebuild that enables a
  reported level source; no mask at arrival with a directed end of
  interrupt, on ARM64 and RISC-V; a level line without one needs one node.
- x86-64: a #DB at CPL0 is the start of its entry; each DR6 cause judged
  alone; DR6 reset at CPL0 and CPL3; kind 9 kept in one word, raised before
  any user instruction, after kind 8.
- Membership and reclaim: observations stamped with the member word;
  messages held for a fresh start sent once it no longer reads starting
  fresh; `node_stop` only from a live or sealing node; acknowledgements by
  R5 alone; R6's proof (no counted leaf, no attached device) and R1's
  attach words written detached at the unit's confirmation.
- The fence: INIT on x86-64 every round, the NMI confirming nothing,
  `parked` and the firmware's answers read-only to node kernels; the
  bootstrap core's class from `BootMembership`'s flags; parks through the
  park routine on ARM64, `sbi_hart_stop` on RISC-V, a refused park never
  confirmed; in-service bits cleared before a core runs a node it entered
  without an INIT.
- The grant count of D-148 in the gate word, held by G5 against a faulty
  kernel's give and retype (`gate_give_chunk`, `gate_retype_page`);
  `System::make_faulty` judges no faulty kernel's own state, only the
  machine's rules.
- Two model faults found by the long run: a node that had just written
  sealed accused another in the same step (seed 867); a node starting
  afresh accused another (seed 1434).

## Pass 29 (D-152): the gate from the tables of "Gate words"

`model/src/gate/` (compiled for tests only) is a small machine whose state
is the gate-written words, each a type with exactly the states of its
table, and whose operations are the rows: one function per transition,
with its writer, its condition and its compare-and-swap; the event columns
are functions too (`events.rs`, death split into fence, R1 and R6 in
`devices.rs`). The hardware caches everything it may after every step
(TLBs and paging-structure caches, the unit's contexts and IOTLB, the ITS's
mappings, the interrupt entry cache), and the unit's queue completes in
order.

- CPU side (`cpu.rs`): the page class byte with its hook states and the
  free-list link, kept kernel tables, the gate word's counts, loaded roots
  and tags, the tag table, requests, pending sets and idle words, take and
  give.
- Unit side (`unit.rs`, `devices.rs`): request records with operations 1
  to 3, ITS commands and IEC invalidations; identifier, attach and domain
  words; an iospace root's device count and *dying*; the ITS word and the
  `its_table` object; the R1 word and both bitmaps, one unit per call;
  remapping entries and the retired word; take-over with its rebuild and
  the hand-over at a seal.
- Interrupts and seals (`parks.rs`): line words and drop bits, the park
  requests raised by the sealing call, `seal_parked`.
- Grants (`grants.rs`), ports (`ports.rs`).
- Faulty kernels (`faulty.rs`): every node calls any operation with any
  arguments in any order, and writes any data page it holds; the checker
  (`check.rs`, `check_devices`, `check_ports`: 47 rules) holds (G) and the
  tables after every step. 200 seeds of 400 steps in `verify`; 30,000 seeds
  of 800 steps clean (`faulty_kernels_keep_g_long_run`, ignored).
- The nine seeds of D-150 and D-151 and the scenario of each gap below
  fail under the old reading (`OldRules`) and hold under the tables.
- `abi/`: the D-152 class byte, `ItsWord`, `RequestWord`, `R1Word`,
  `SuspendWord`, `StartMachine` of 128 bytes (layout 6), `ObjectKind::ItsTable`.
- The suspend draft (`suspend.rs`, experimental, outside the conformance
  set): every interleaving under each variant of the open rules.

## Pass 30 (D-153)

- Gaps 26 to 28 and 30 to 39 settled in the tables of Gate words: every
  `CONTRACT-GAP` marker in `model/src/gate/` and `abi/src/lib.rs` points
  to its D-153 cell, R6 (`Death`) clears a chunk's kept counts while
  keeping the per-generation check (gap 37), and `settled_tests.rs` adds
  explicit regression scenarios with `OldRules` switches for gaps 26,
  27, 28 and 30 alongside the scenarios of gaps 31 to 39.
- Gap 29 (`823e473`, in its own commit while under review): the kernel's
  own writable view of a chunk splits its large leaf into a kept level-1
  table drawn from another `data` page of the chunk (`from_data`,
  `complete_kernel`), returns a page back to `data` to writable in the
  split table (`kernel_view_writable`), and merges the split table back
  into the large leaf (`merge`) when the chunk's count of pages not
  `data` falls to 1, with the faulty-kernel actor exercising `Merge`,
  `KernelAccess` and the split (`cpu.rs`, `faulty.rs`,
  `kernel_view_tests.rs`). Seven behaviours the new text leaves open or
  breaks are marked `CONTRACT-GAP` 40 to 46.
- The suspend draft (`suspend.rs`, `suspend_tests.rs`, outside the
  conformance set): `SUSPEND_PARK_NS`, Q4 (`alarm_on_watchdog_only`: one
  watchdog core that parks keeping a wake at `SUSPEND_PARK_NS`, the
  others off, is enough to abandon for a hung *S* without a wedge), and
  the second half of Q5 (`pm1_late_sleep`, `wake_accepts_resuming`: a
  PM1 write that gives up to *resuming* and then enters the sleep late
  wedges at the waking vector unless the waking vector accepts *resuming*
  as well as *asleep*).
- Two cores racing inside gate calls under the gate lock (`gate/race.rs`):
  the calls whose words another node reads (an attach of a device and the
  release of the answer, on one shared request entry), split into their
  word accesses across two cores of node *X*, with the holder *H*, the
  fence (stopping a core at any word access with its call half done and
  its gate lock held), R1 and R6; every interleaving holds the invariants
  and never wedges under the lock, and without the lock two cores tear
  the open record.
- A faulty node in the combined exploration (`scenarios/combinations.rs`,
  `check/machine.rs`): each seed designates `seed % 5` as a faulty node
  that attempts forbidden gate calls (`gate_give_chunk` on a chunk with
  `grant_count > 0` or `map_count > 0`, `gate_park_self` while not
  sealing) and injects forged ring messages (wrong sender slot, a
  `Release` while `uses` still counts leaves, forged `RevokeStep`,
  `UnitRequest`, `UnitAnswer`, `Send` and `Report` credits, an unreadable
  raw header) and early `acked`/`maps_of` beacon words during a peer's
  `Fenced` state while the healthy nodes run all 28 combination events
  and recoveries.

## Pass 31 (D-154, D-155)

- D-154, dedicated table chunks, replaces pass 30's split and merge of the
  kernel's large leaf (`823e473`), and the gaps 40 to 46 leave with it.
  Bit 58 of the gate word is the table bit (`abi/`: `GateWord::TABLE`,
  `is_table`, `consistent`; `MEMORY_TAKE_TABLES`, `SLOT_UNTYPED_TABLE`,
  `START_UNTYPED_TABLE`, `BootFloors.floor_tables`, `OBJECT_SIZES_ENTRIES`
  = 22).
- The node model (`Model`): every node slot has a table floor after its
  ordinary floor (`floors`, at least one chunk, refused otherwise), whose
  chunks carry the table bit from machine start-up and whose untypeds are
  table untypeds; the start-up aspace's root, tables and reserve of free
  tables lie there. `memory_take` kind 2 makes a table untyped (no clean
  mark, no `exec`, the whole gate word written with the table bit, the run
  mapped read-only). `untyped_retype` of an `aspace` or an `iospace` takes
  one page of header from the ordinary untyped and one page of root from
  the table untyped its size argument names (judged as a capability
  argument); `aspace_map` and `iospace_map` take that table untyped; an
  `its_table` comes from a table untyped only, and a table untyped makes
  nothing else but a child table untyped. A table untyped waits for the
  spaces whose tables it holds (`again` at its removal, `busy` at a give).
  R6 clears the table bit with the counts, a fresh start writes its floors'
  words anew (G13). New checker rules, each with its test in
  `scenarios/pass31.rs`: `TABLES:` a gate word no gate writes (a table
  chunk counts no leaf and no grant, an ordinary chunk no page not *data*),
  the kernel's view of a table chunk, `exec` on a table untyped, a table of
  a space outside its table untyped, a kind a table untyped does not make.
  G5's rule on a page under an anchor is now shadowed by the first.
- The gate model (`model/src/gate/`): `Take::Tables`, the table bit per
  chunk, a read-only large leaf never rewritten, the rows from *data*
  asking for the table bit alone (no map count, no grant walk, no request
  of a core), G3 and G6 refusing a leaf and an anchor toward a table chunk,
  each node's floor one table chunk then one ordinary chunk, the kept
  kernel root and table out of the chunks. `table_chunk_tests.rs` shows
  each attack breaking the checker under its earlier reading
  (`tables_in_ordinary_chunks`, `leaves_into_table_chunks`,
  `anchors_over_table_chunks`, `table_chunks_mapped_writable`) and refused
  under the tables, R6 and G13, a seal and a resume; the faulty actor takes
  chunks of both kinds and tries all of it (200 seeds of 400 steps in
  `verify`; 30,000 seeds of 800 steps clean on 19 November 2026).
- The corrections of D-153's review that D-154 carries: F9 (an identifier
  word freed only when its attach word and its ITS word read 0: as pass
  30 had it), F10 (the ITS word *pinned* → 0 at a *failed* or *refused*
  mapping answer: `a_failed_its_mapping_unpins_its_table`), F11 (the
  clearing call of an `unbind` takes its own core out of the set: as pass
  30 had it), F12 (22 object sizes: the start-up page's bytes), F13 (kept
  counts end at R6 once no attach word reads *attached*: as pass 30 had
  it), F14 (R6 retires every handle of a dead node's cores once any IOMMU
  unit has failed, in the node model too:
  `r6_retires_every_handle_of_a_dead_nodes_cores_once_a_unit_failed`), F15
  and S1 (one segment and one unit in the small machine: nothing to
  change), M3 (RISC-V: a change that unhooks a table invalidates the whole
  ASID, an invalidation by address dropping leaf entries only:
  `riscv_unhooks_a_table_with_a_whole_asid_fence`).
- D-155: `abi/` gains `IKV_FIRST` and `IKV_LAST` (240 to 243, one vector
  for each of the kernel's four interrupts to other cores: the contract
  fixes only "inside 240 to 255"), `ikv_valid`, `line_vector_valid`,
  `GATE_PCID`, `usable_tags` and `gate_tag`, and `dr7_valid` refuses
  `DR7.GD` by name. The gate model reserves the gate's tag (G7: bound to
  no root, loaded for none) and composes an ICR only for an inter-kernel
  vector (G10); a register write with `DR7.GD` is `invalid`.
- The combined exploration's faulty node also asks its gate for a table in
  an ordinary chunk and for a leaf and an anchor toward its table floor.
- The review of pass 31 against D-156 (four findings, each with its test
  in `scenarios/pass31.rs` or `check/tests.rs`): the sweep of a dying
  `cspace` runs in two bounded passes (`Sweep::first_deferred`, `pass2`;
  pass 1 defers the root capability of a table untyped that spaces still
  name, invariant 27 follows); a child table untyped destroyed with pieces
  lost behind a failed unit stays a lost live piece of its parent, so no
  ancestor's watermark returns over them; `holds_frame` refuses a table
  chunk (G6, `busy` at `node_send`); invariant 58 names a translation to a
  table chunk itself (G3 (3)). The ITS word of an `its_table` is in the
  gate model only, so the sweep defers no `its_table` here.

## Pass 32 (the full audit's confirmed defects)

A full audit of the model, re-judged by another model with a failing test
per finding, found defects of the model against the contract as it stands.
No decision is applied here; each fix names the sentence it follows, and
comes with its scenario (`gate/pass32_tests.rs`, `scenarios/pass32.rs`,
`scenarios/pass32_node.rs`, `tests/calls.rs`).

- The gate model. A second give of a chunk is refused while a request
  names its give, and the completion writes the word by compare-and-swap
  (two cores giving one chunk freed the next owner's word; the contract
  states the compare-and-swap only, gap 49). `memory_take` follows rules
  1 to 3 of taking for a word free with a node as owner. A grantee's leaf
  is counted in its use entry alone, and the record of a clearing names
  the count its leaf is in (`Counted`): the granter's map count has one
  writer, and the checker holds each use count equal to its leaves,
  present or being invalidated. The counts of a chunk read the calling
  node's requests only. A fenced granter frees no grant; sealed is written
  only by a node that holds no unit role and has no open record; the
  holder answers *failed* a record opened after the unit failed; an
  argument out of its range is refused, and the faulty driver passes some.
- Several nodes. A `SendAnswer` acts on the entries its send wrote, named
  with their serials. A reserve chunk taken by its role's holder reads
  taken by that node wherever a word is judged (`Table::taken_by`: R4's
  audit, the checks of a unit request, the gate's own calls). A seal hands
  no unit over while its node rebuilds it, and a node that sealed after
  its report was asked is passed over as sealed. Unfenced marks every
  unconfirmed core unusable on every architecture. A kind is refused on
  the ring that does not carry it; a kept frame touches no chunk.
  `node_stop` mode 3 is `unsupported`, and modes 0 and 2 ask a reserve
  role of the caller's node: the harnesses claim one for a node before it
  kills or retires (`S::supervisor`), as the supervisor that does it would.
- One node. Destroying a context that runs on another core sends that core
  the reschedule interrupt. The root capability of an untyped that has a
  descendant does not move, by `cap_move` or by a *move* in a message,
  judged at the call and again at delivery (D-131). The removal of an
  irq behind a remapping unit is a request also on the node that holds
  the unit (D-150). Moving a waiting context to another core wakes
  nothing. Unbinding masks a level source at once. Trace code 5 carries
  the exception number; a debug area whose magic or version is wrong is
  traced (code 12) as it is cleared; `thread_resume` reads the target's
  own space.

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
the reading implemented and the text proposed. The sixteen of pass 15
(`research/drafts/model-15-contract-gaps.md`) are settled in the contract
since pass 16 (D-133), and each branch that applies one names the
contract's sentence and "gap *n*". The four found in pass 25 (17 to 20)
are settled by D-144, and gap 21, found in pass 26 (the seal of the
holder of a failed unit's role never ended), by D-145. Pass 28 found
three: gap 23 (word 3 of a kind 9 kept while a debugger rewrites the
control word), settled by D-147; gap 24 (the start-up's passage on a core
of class 0 against The fence) and gap 25 (which unconfirmed cores the
fence marks unusable), both settled by D-151. Pass 29 found fourteen, all
settled by D-153 in cells of Gate words (26 to 28 and 30 to 39 as the model
read them; 29 by a new design, the split and merge of a chunk's large leaf,
see Pass 30); each scenario stays as a regression test with the earlier
reading as an `OldRules` switch:
26 and 27 (a core's request: the caller's own invalidation, an unbind's
asked set), 28 and 29 (the class byte's rows from *data*: G5's map count,
the kernel's own writable view), 30 (counts between a give's call and its
completion), 31 (a free-list link naming a freed table), 32 (what R1's
invalidation covers), 33 (handles a failed unit never confirmed), 34 (an
identifier freed under its ITS word), 35 (an ITS unmapping served before
its mapping), 36 (a fencing requester's record served), 37 (two cells of
the gate word's table), 38 (a second request of a device while its first
is open), 39 (`its_table`'s kind number). Pass 30's model of gap 29's
split and merge found seven open points in the new cells (40 to 46, in
`823e473`); D-154 closes them all by removing the split and the merge:
tables live in dedicated table chunks (see Pass 31). Pass 31 found two,
both settled by D-157 (no `CONTRACT-GAP` marker left):
gap 47 (`abi/src/machine.rs`: on RISC-V with `ASIDLEN = 0`, where the
gate reserves no ASID, the tag table holds its one tag, entry 0) and
gap 48 (`model/src/boot.rs`: the start-up aspace's root and tables drawn
from the first table-floor untyped stay permanent live pieces for the
life of the node, so its watermark never returns); D-157 also records
that D-156's two-pass sweep defers pinned roots per dying `cspace`.
Pass 32 found three, open: gap 49 (`model/src/gate/cpu.rs`: a second
give of a chunk whose give is in flight, which the compare-and-swap of
the completion alone does not stop; refused here), gap 50
(`model/src/sched.rs`: the width of the products of an exhaustion after a
very long stop; left as it is, the model panics there) and gap 51
(`model/src/faults.rs`: a kept kind 9 of a thread that is not marked and
has no context; raised, as before). Two earlier gaps were settled otherwise than the
model first read them, and the model follows: a node's cores lie in one
window of 64 core numbers (gap 7, checked after every step), and the
claim of an MSI asks every IOMMU unit of the source's segment, one after
another (gap 12).

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
