# Desktop trial: the measures that wait for the reference desktop

A bare-metal measurement image for x86-64: a UEFI application (Rust,
`no_std`, no dependency but the multicore trial's own library), booted from a
USB stick, that runs every measure below unattended and writes the results
to the screen, to the serial port when one answers at COM1, and to
`\DODOS\RESULTS.TXT` on its own stick. `tables/` turns that file into the
tables the documents need. The owner's steps at the machine are in
[RUNSHEET.md](RUNSHEET.md).

The reference desktop is expected to be an Intel Core i5-14600KF (D-116
point 4: 6 performance cores with two threads each, 8 efficiency cores, no
integrated graphics, to confirm). Nothing here assumes it: the image reads
the processor and adapts (the number and kinds of cores, the features).

Sources, all read for this work: the Intel SDM, combined volumes, order
325462-093, September 2026 (cited "vol. N section"; its numbering is two
chapters off the -086 edition the contract cites, see "Changes for the
documents"); the EDK2 reference headers (MdePkg, commit 8628578, 13 November
2026) for the UEFI 2.10, PI 1.8 and ACPI 6.5 structures, because uefi.org
answers automated clients with a challenge page and the specifications'
own text could not be fetched; Linux at commit aebb727 for the xHCI debug
capability ID, the PCI header offsets and the I/O APIC EOI register. Every
claim about the processor in the code cites its section; what could not be
checked says "not verified".

## The plan

One step per row, in the order the image runs them. "Settles" names the
decision or open point; "Chooses" says what result picks which option. Unless
stated, statistics are n, min, p50, p99, p99.9, max and mean, in TSC
nanoseconds (the TSC rate is measured against the ACPI PM timer, or the
firmware's Stall), and in core cycles from fixed counter 1 where the core
counts them. Every measure runs on application processors (*workers*, the
first hardware thread of each physical core); the bootstrap core keeps the
firmware and is measured only where named. Interrupts are masked during a
measurement unless it is about interrupts.

| Step | What is measured | Cores | Repetitions | Settles | Chooses |
|---|---|---|---|---|---|
| `identity` | CPUID leaves 01H, 07H.0, 07H.1, 05H, 06H, 1AH, 80000001H and the brand string on every core: CET shadow stack and IBT, CET_SSS, PKS, PKU, FRED, LKGS, WRMSRNS, x2APIC, INVPCID, PCID, TSC-deadline, VMX, MONITOR, hybrid, core type; MSRs IA32_APIC_BASE (BSP flag), IA32_FEATURE_CONTROL, IA32_HWP_CAPABILITIES, IA32_PAT, IA32_TSC_ADJUST, MSR_SMI_COUNT, MSR_PKG_CST_CONFIG_CONTROL. Then on each core: set CR4.CET and write IA32_S_CET; write IA32_PKRS and set CR4.PKS (with recovery: a #GP is reported, not fatal) | every worker and the bootstrap core | once | P-002 (D-116 point 4), and through it P-003 and P-004 | CET_SS and IBT on every core and `cet_enable=ok` everywhere: P-004 A (require CET) holds on this machine. A feature that differs between P and E cores (the derived table names it) is, for a state feature, a reason the node rule of D-129 keeps those kinds apart. `pks_enable=ok` with the PK fault of `gate` below: PKS exists despite Intel support's 2023 answer (kernel.md). FRED, LASS, UINTR: recorded only (FRED deferred, D-065) |
| `platform` | ACPI tables present (SPCR, DBG2); a 16550 at COM1 (scratch register); every PCI serial controller (class 0700h); every xHCI controller and whether it lists the USB Debug Capability (extended capability 10); every EHCI and its debug port capability (0Ah); each I/O APIC's version (EOI register from version 20h); the local APIC's EOI-broadcast suppression bit (SDM vol. 3A 13.4.8). Reads only | bootstrap core | once | P-007 (D-058); open point "end of interrupt of a level line from any node" (presence only) | A serial port (COM1 or PCI): option A of P-007 needs no purchase. DbC listed: option B is possible, on the controller named. Neither: a PCIe serial card is to buy |
| `stall` | Each worker reads the TSC in a tight loop with interrupts masked for 30 s and keeps the longest gap and the gaps over 10 µs, 100 µs, 1 ms; SMI count before and after | all workers at once | 30 s | `STALE_NS`, `STUCK_NS`, `CORE_ANSWER_NS` ("above the longest firmware stall of the reference machines") | The longest gap seen sets the floor; the documents' values (100 ms, 100 ms, 10 ms) stand if they are well above it (suggested: ten times) |
| `gate` | Per sample of 64 operations: an 8-byte store (baseline); MOV CR3 to a second root and back, without PCID and with PCID and bit 63 (no flush); clear CR0.WP, store to a read-only page, set WP; open PKS key 1 in IA32_PKRS, store to a page of key 1, close it (WRMSR, and WRMSRNS when present), after checking a store with the key closed faults with the PK bit; INVLPG and INVPCID type 0 then the store that reloads the entry | one P and one E worker | 2,000 samples | P-003 (kernel.md table rows 2 to 4) | The cost per operation of each window mechanism against a store. Fallback B (gate root with PCID) is judged by `gate.cr3 pcid=1`; C (PKS) only if `identity` and the fault check say it works |
| `cet` | Map a page as a supervisor shadow-stack page (R/W 0, dirty 1) and check an ordinary store faults; WRSSQ to it; then the window's own page table mapped only as a shadow-stack page: a new entry written by WRSSQ serves the walk, gets its accessed and dirty bits, is replaced by another WRSSQ and the walk follows; an ordinary store to the table faults. Costs: 64 and 512 WRSSQ per sample (512 = one gate call's batch of *E* entries); 64 CALL/RET pairs with supervisor shadow stacks off and on; 64 indirect CALLs to an ENDBR64 target with IBT off and on. Everything with supervisor shadow stacks on (both `SH_STK_EN` and `WR_SHSTK_EN`) only on a core whose CPUID.07H.01H:EDX.CET_SSS[18] is 1 | one P and one E worker | 2,000 samples | P-003 first mechanism (D-120); kernel.md "CET's cost" (proxies only, see below) | `cet.walk` all 1 and the stores faulting: mechanism A works on this processor and is chosen (no window); any 0, or `cet.sss cet_sss=0` (the kernel does not enable supervisor shadow stacks there, kernel.md row 1): fallback B. `cet.wrssq ops=512` against `gate.cr3`: the price of A per batch. The CALL/RET and indirect deltas bound CET's overhead per call and per indirect branch |
| `ipi` | Self-IPI taken and ended; round trip A→B→A by fixed IPIs (B's handler answers); one-way (B stamps the TSC); the same round trip by two cache-line writes; B spinning, halted, or in MWAIT with each hint CPUID 05H enumerates (core C6 and C7 residency read around); the round trip between every ordered pair of workers | pairs P→P, E→E, P→E, E→P; then all pairs | 20,000 spinning, 2,000 per idle state, 500 per pair of the matrix | the real interrupt cost the trial modelled (`ipi`); kernel.md line 71; the overhead gate "IPC round trip between two cores" (gates.md, "To set") | The real `ipi` value of the trial; the cross-kind penalty for placement; the exit time of each sleep state (derived table: round trip minus spinning) for the idle policy |
| `shoot` | Change a page-table entry, INVLPG locally, send the shootdown vector to each target (unicast) or once per x2APIC cluster (logical), wait for every acknowledgement | initiator and 1, 2, 5, 13 others: same kind, other kind, mixed; targets busy or halted; the bootstrap core joins at 13 | 5,000 | kernel-interface.md "TLB, one core and several" (the bound of `aspace_unmap`); open point "a shootdown inside a node of 2 to 14 cores"; kernel-shape-opus.md 8.2, second criterion | The cost of an unmap's wait per node size. 8.2: if a shootdown inside a node of 6 cores (`others=5`) is not faster than the same unmap done by messages between one-core nodes (5 × `ipi.rtt`, as a lower bound), case 3.1's main argument fails |
| `trial` | The multicore trial's suites `locks, a1t, a2t, a3t, ct, a3n, br` unchanged (lookups with changes, IPC between cores, signals from many threads, a process beside others, the atomic notify, unmaps by every thread), worker *i* on the *i*-th core (P cores first), nodes of 2 to 6 cores against the same number of one-core nodes, interrupt cost 0, 1000 ns and **real** (a real interrupt taken where the model charges `ipi_ns`) | 14 workers | 3 × 150 ms per cell | open point "Locking inside a node of several cores" (L1, L2); D-129 | L1 is the default; L2 where L1's IPC (`a2t`) or unmap (`br`) cost is too high (the open point's rule). kernel-shape-opus.md 8.2: C* is wrong for this machine if the weighted cost of the lock exceeds the weighted gain (the weights need traces, below) |
| `trial-efirst`, `trial-alternate` | The `locks` suite with E cores first, and P and E alternating | 14 workers | 3 × 150 ms | the hand-over of a queue lock between kinds (open point) | How much an E holder delays a P waiter (the bound of D-129 uses the slowest kind) |
| `ept` (risky) | VMXON; identity EPT over the RAM of the memory map with 2 MB pages, then with 1 GB pages; the same core runs as a guest (state copied from the host, exit only at VMCALL) a pointer chase over 2^16 pages of 4 KB (every hop a TLB miss), a sequential read of 64 MB, a register loop; the same natively | one P and one E worker | 10 runs each | D-117 point 3 (the optional monitor); fencing-without-cooperation.md part 7 item 3 | The ratio guest / native per workload and page size: the common-path price of the hardening monitor, for the owners' gate |
| `warm-reset` (risky, reboots) | Four 1 MB regions (above 16 MB, 1 GB, 4 GB, near the top) filled with a pattern, recorded on the stick, then a reset; the next boot counts the words intact. Methods: firmware warm, ACPI reset register, firmware cold | bootstrap core | 2 per method (6 restarts) | P-016 (D-125 point 1) | Intact after the warm methods: the crash record can stay in RAM. Not intact: the first stage keeps it in a UEFI variable or a file (D-125) |
| `init` (risky) | INIT, wait *d_init*, SIPI to the trampoline, wait *d_sipi*, second SIPI (or none); success when the target confirms its park request from the trampoline within 100 ms; failures counted, then recovered with the SDM's delays. Sweep *d_init* 10 ms, 1 ms, 100 µs, 10 µs, 0 × *d_sipi* 200 µs, 10 µs, one SIPI. Then the SDM's delays with the target halted, in its deepest MWAIT, spinning inside its NMI handler (NMIs blocked), and in VMX root operation (blocked? held until VMXOFF?). Fences of 1, 2, 4, 6 cores: by NMI (the handler parks) and by INIT (handlers dead), first stop to last confirmation. INIT during SMM only with `smi_cmd=` (below) | a P and an E target, another worker directs | 200 per sweep cell, 50 per state, 20 per fence | the open points on INIT and SIPI delays, `FENCE_NS`, "INIT during SMM"; fence classes (D-117, D-129) | The shortest delays with no failure in 200 can replace the SDM's example; `FENCE_NS` (20 ms) must exceed the worst `fence.init` with margin. INIT blocked in VMX root confirms class 1 for such a core; held or lost says how long a fence may take |
| `bsp` (risky, last) | NMI from a worker to the bootstrap core; the bootstrap core confirms a park request, halts with interrupts masked, and runs again on a start request sent with an NMI (resent every millisecond until it runs: the count is reported); the BSP flag written 0 and read back; the bootstrap core's restart path rehearsed on a worker (save, INIT, SIPI, trampoline, restore, jump back); then, only if the flag reads back 0, INIT and SIPI to the bootstrap core with its flag cleared | bootstrap core, one worker directs | 1,000 NMIs, 200 parks, 20 restarts | D-117 and the open point "The bootstrap core on x86-64" | `bsp.init` restarts every time: clearing the flag removes the exception, the bootstrap core is of fence class 2. The flag does not clear, or the machine resets (`F bsp hung` at the next boot): it stays class 1. `bsp.park`: the stub's restart without INIT works, and `resends` shows the lost-wakeup window (below) |

A full run takes about 30 minutes on the expected desktop (the trial is
about 17 of them: 1,800 runs per repetition at 175 ms, counted from the
trial's own suites; `stall` 30 s; `ipi` about a minute with the MWAIT
states; `init` under a minute), plus six automatic restarts for
`warm-reset` (about a minute each with the firmware's start), so about 40
minutes in all. Each step's own duration is in its `I step ... ends
seconds=` line.

## What cannot be measured without a kernel

Said plainly, so that nobody reads more into the numbers above:

- **A gate call with its checks** (G3: ownership word, page class, map
  counts): there is no gate yet. `cet.wrssq` and `gate.*` are the cost of
  the write mechanism alone; the checks come on top.
- **CET's cost on a null call, an IPC and an address-space switch**
  (kernel.md): there is no system call or IPC path. `cet.callret` and
  `cet.indirect` are per-branch proxies, not those figures.
- **The instruction scanner's false positives** (G11): a build-machine
  measure on the one-core kernel, not on this machine.
- **Lock hold times of real kernel events** (*H* and *h* in the bounds of
  D-042), the step bounds and budgets of P-015, the page fault of the memory
  service against Linux (P-017), IPC against seL4 (the gate of gates.md),
  `thread_move`, role requests, reclaim time, the G14 audit: they need the
  one-core kernel.
- **The weights of the trial's suites** (rates of `mmap`, `munmap`,
  `mprotect`, waits and calls traced from Ladybird and an inference engine,
  kernel-shape-opus.md part 7 item 9): traces on Linux, not this image. The
  trial's tables come out unweighted.
- **Posted writes after a stop** (`SETTLE_NS`): needs a device whose writes
  can be observed arriving; not measured.
- **The end of interrupt of a level line from another node**: needs a
  level-triggered device under a kernel; only the presence of the EOI
  register and of EOI-broadcast suppression is read.
- **The latency gates** (photodiode at 120 Hz), boot and shutdown baselines
  of other systems, the GPU trial, a per-core-type kernel binary: other
  setups entirely.
- **An NMI fence during SMM with the RIP check** (fencing draft part 7
  item 5): not done; `init.smm` covers INIT during SMM, opt-in.

## Safety

- **Order.** Measures that cannot hang the machine run first; then the
  risky ones: `ept`, `warm-reset` (restarts on purpose), `init`, and `bsp`
  last, its most dangerous part (INIT to the bootstrap core) at the very end.
- **Progress on the stick.** Before every step the image appends `start
  <step> <boot>` to `\DODOS\STATE.TXT` and flushes it; after it, `done`.
  At the next boot a step that started and never finished is reported
  (`F <step> hung in an earlier boot`) and skipped, and the run goes on with
  the next one. So a hang or a reset costs one step: reset the machine and
  the run resumes. Results are flushed to the stick after every step and at
  every heartbeat line.
- **Watchdogs.** The firmware's watchdog (boot services `SetWatchdogTimer`,
  step limit + 60 s) resets the machine while the bootstrap core is in the
  firmware; and the image's own watchdog runs on a second hardware thread
  (or the last worker without one), wakes every 10 ms, and resets the machine
  through the ACPI reset register (then port CF9h) when a step passes its
  limit, even with every other core stuck with interrupts masked.
- **Skipping.** Key S in the first 5 s skips every risky step; any key in the
  3 s before a risky step skips that step; `skip=a,b` in
  `\DODOS\CONFIG.TXT` skips steps by name; key R forgets the progress.
- **Panics.** A panic on the bootstrap core writes its message and the step
  to the stick, then resets after 10 s (the run resumes after the step); a
  panic on another core is reported by the bootstrap core, which stops using
  that core.
- **Disks.** The only file system the image opens is the one it was loaded
  from (`LoadedImage.DeviceHandle`); it never opens another volume or a block
  device, and never writes anywhere but `\DODOS` on its own stick.
- **Firmware.** The image never calls ExitBootServices; the firmware keeps
  running on the bootstrap core (its timer interrupts go through the image's
  IDT copy). The application processors are started by the firmware's MP
  services and then run only the image's code.

## Results

`\DODOS\RESULTS.TXT`, appended at every boot. One line per fact:

- `R <measure> key=value ... n= min= p50= p99= p999= max= mean= unit=` a
  result (statistics over the samples; `unit=ns` from the TSC, `unit=cycles`
  from the core's fixed counter, then `clock=core`);
- `R <measure> key=value ...` a result without statistics (counts, checks);
- `I <topic> key=value ...` information (processor, configuration, steps);
- `F <step> ...` a failure, a fault or a hang;
- `T <line>` a line of the multicore trial's own Markdown output; its CSV
  goes to `\DODOS\TRIAL-PFIRST.CSV` (and `EFIRST`, `ALTERNATE`), the format
  of `trials/multicore/data`.

`\DODOS\STATE.TXT` is the progress (`boot`, `start`, `done`, `failed`, and
the `wr` lines of `warm-reset`).

    nix develop -c cargo run -p dodos-trial-desktop-tables -- RESULTS.TXT > results.md

writes one table per measure, the failures, the trial's tables, and the
derived tables: features by core with those that differ between kinds,
gate cost per operation, and the exit time of each sleep state.

## Configuration

`\DODOS\CONFIG.TXT`, `key=value` per line, `#` comments. None is needed on
the desktop.

| Key | Meaning |
|---|---|
| `skip=a,b` | Steps not to run |
| `quick=1` | Few repetitions (QEMU) |
| `fake_kinds=1` | Without a hybrid leaf, call the first half of the workers P and the rest E (QEMU: makes every cross-kind path run) |
| `stall_seconds=N` | Length of `stall` (30) |
| `trial_suites=`, `trial_reps=`, `trial_ms=`, `trial_max_k=` | The trial's suites (`locks,a1t,a2t,a3t,ct,a3n,br`), repetitions (3), ms per run (150), largest node (6) |
| `smi_cmd=0xNN` | Enables `init.smm` with this value on the FADT's SMI command port. No default: an unknown SMI command may do anything; only with a value the firmware is known to ignore |
| `bsp_init_flag_set=1` | After `bsp.init`, sends INIT to the bootstrap core with its flag set (the SDM's case); ends the run |
| `x2apic=keep` | Leave the APIC in the firmware's mode |
| `serial=0` | No copy of the screen on COM1 |
| `debug=1` | Each core writes its progress marks to COM1 directly |
| `panic=halt` | Halt after a panic instead of resetting |
| `end=shutdown` | Power off at the end without waiting for a key |

## What QEMU ran, and what only the machine will

`qemu.sh OUT_DIR [CONFIG]` builds the image, writes a FAT stick image with
mtools, and boots it under `qemu-system-x86_64` (machine q35, OVMF, `-cpu
max,vendor=GenuineIntel`, an xHCI controller and a PCI serial card). The
host is an ARM64 Mac, so this is emulation (TCG): **no timing from QEMU
means anything**; it shows that every path runs. Configuration
`qemu-config.txt` (`quick=1`, `fake_kinds=1`). The run kept in
`qemu-results/` used 30 vCPUs (15 cores of 2 threads: 14 workers, 14
second threads, one of them the watchdog) and went through every step,
including three restarts (one per `warm-reset` method with `quick=1`); a
second run with 14 cores (the desktop's count) checked the shootdown to 13
others with the bootstrap core as a target. The stick image there is a FAT
volume without a partition table; the run sheet's stick has one MBR
partition (both are FAT file systems the firmware reads).

Exercised under QEMU: the start of every core through MP services; the
switch to x2APIC with the firmware's timer still running; identity on every
core and the PKS enable; the platform probe (COM1, the PCI serial card,
the xHCI controller's capability list, the I/O APIC version, the local
APIC version); the stall loop; `gate` store, CR3 without PCID, CR0.WP, PKS
with WRMSR and its fault check; self-IPIs, round trips, one-way, memory,
halted and MWAIT hint 0 targets, the pair matrix; shootdowns to 1, 2, 5 and
13 others (with the bootstrap core as a target in the 14-core run), unicast and logical,
busy and idle; the trial's suites with real interrupts on 14 workers and the
two other orders; the trampoline (real mode to long mode), the whole INIT
and SIPI sweep, every target state but VMX root, fences of 1 to 6 cores by
NMI and by INIT; the bootstrap core's NMI, park and restart by NMI (during development
one run lost a wake-up and the resend recovered it: `resends=1` in 5), the
BSP flag test, the
rehearsal of the bootstrap core's restart on a worker; three warm-reset
methods across four boots; resuming after a step that reset the machine;
the image's watchdog resetting the machine when a step hung (seen during
development, the spin fallback path, and again on 17 November: on a loaded
host `shoot` missed its acknowledgement guard, then 100 ms, which panicked
the initiator while the bootstrap core waited for it as a target, until
the step's watchdog reset the machine). A late acknowledgement now ends
only its line, reported as `F shoot` with its keys, and the guard is 1 s;
under load that path ran and reported each line.

Only the real machine will run: everything in `cet` (QEMU has no CET; the
instruction encodings were checked by disassembly against the SDM's
opcodes); `ept` and the VMX root case of `init` (no VMX under TCG);
`gate.cr3 pcid=1`, `gate.invpcid` and the TSC-deadline path of the watchdog
(TCG lacks PCID, INVPCID and TSC-deadline); WRMSRNS; MWAIT hints beyond 0
and the C6/C7 residency counters; the core-cycle clock (TCG reports no
architectural fixed counter); real P and E kinds; `bsp.init` on the real
bootstrap core (QEMU ignores a write of 0 to the BSP flag, so only the
rehearsal ran); `init.smm` (opt-in); the DbC capability (QEMU's xHCI has
none, so only the walk of the list ran); SPCR and DBG2 (QEMU has neither).

## Changes for the documents

Four changes this trial found while it was written are in the contract
since (Docs at 74011e1, D-134 and D-135), and the image follows them:

1. **The park stub's lost wake-up** (kernel-interface.md, "The fence"): an
   NMI that arrives after the stub has read its start request and before
   HLT is lost (the bootstrap-core test hit it under QEMU, `resends=1` in
   5). The contract now has the starter send the wake again every
   `START_RETRY_NS` (1 ms) until `started` equals its request; `bsp.park`
   resends every millisecond and reports the count.
2. **SDM edition.** The contract cites one edition, 325462-093 (September
   2026), as the image's comments do.
3. **WRSSQ at ring 0 needs supervisor shadow stacks on**: CR4.CET,
   `IA32_S_CET.SH_STK_EN` and `IA32_S_CET.WR_SHSTK_EN` (vol. 2D
   WRSSD/WRSSQ), and the kernel checks `CPUID.07H.01H:EDX.CET_SSS[18]`
   before it enables them (kernel.md, write gate row 1; P-002). `cet` sets
   both bits for its shadow-stack windows and runs them only on a core
   whose CET_SSS is 1; otherwise it reports `cet.sss cet_sss=0` and
   measures only CALL/RET without shadow stacks and IBT.
4. **IA32_PKRS on 12th and 13th generation hybrid parts** (SDM vol. 4
   table 2-46) is cited in kernel.md row 3. The run decides.

Still for the owners (the Docs repository is not touched here):

1. **After the run**: P-002, P-003 (with P-004), P-007, P-016 and the open
   points named in the plan above change status with the numbers; the
   inventory's group D loses the items this run covers; `plan/progress.md`
   (AGENTS.md: "Update plan/progress.md ... in the same change as the work")
   should record that the image exists and is waiting for the machine.
2. **What the run cannot give** (the section above) stays open in the
   documents, notably the trial's weights and the kernel-dependent numbers
   of P-015 and P-017.
