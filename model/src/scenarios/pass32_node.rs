//! Pass 32: the defects of the kernel of one node that the full audit
//! found and that an independent re-judge confirmed (lot 4; lot 3's one is
//! in `tests/calls.rs`, with the other calls on capabilities). Each
//! scenario is made of calls, and fails without the rule it names.

use super::cores::two_threads;
use super::pass17::succeeds;
use super::pass22::restart_over;
use super::units::IOMMU_ROLE;
use super::*;
use crate::debug::DebugArea;
use dodos_abi::{FaultKind, IrqSource, START_SLOT_IRQ_CONTROL, TraceCode};

const IRQ: u32 = 210;
const NOTIFY: u32 = 211;
const GSI: u32 = 5;

// Lot 4, L4-01. Destroying a context that runs on another core of the
// node sends that core the reschedule interrupt: what it runs is no
// longer what it would choose (Scheduling, the reschedule interrupt), and
// until it takes the interrupt it still holds the thread, whose context
// is gone.
#[test]
fn destroying_a_context_that_runs_on_another_core_sends_its_reschedule() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    assert!(k.m.cores[1].running.is_some());
    k.ok(Syscall::CapDelete(s(301)));
    assert!(k.m.cores[1].ipis.contains(&crate::Ipi::Reschedule));
    k.interrupts();
    assert_eq!(k.m.cores[1].thread, None);
    assert!(k.m.threads[&other].lost);
}

/// The claim of an MSI of segment 0 on core 1, vector 48.
fn msi_claim(destination: u32) -> Syscall {
    Syscall::IrqClaim {
        control: s(START_SLOT_IRQ_CONTROL),
        source: IrqSource::msi(0, 3, 1, 0).raw(),
        core: 1,
        vector: 48,
        destination: s(destination),
    }
}

/// The MSI claimed by node 1 of the reference machine, which holds the
/// role of the segment's IOMMU unit.
fn msi_on_the_holder() -> S {
    let mut m = S::two();
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    succeeds(&mut m, 1, msi_claim(IRQ), 30 * MS);
    assert!(!m.sys.canonical().units[&IOMMU_ROLE].remap.is_empty());
    m
}

fn requests_in_flight(m: &S, node: u16) -> usize {
    m.sys.node(node).unit_requests.iter().flatten().count()
}

// Lot 4, L4-02. The removal of an MSI's irq is a request also when the
// node holds every role concerned, making the same entry and serving it
// with no message (Interrupts, the irq; D-150 removed the shortcut of a
// node that held every unit): the step starts the request and returns
// `again`, and the capability goes once the unit confirmed.
#[test]
fn the_removal_of_an_msi_is_a_request_on_the_holder_of_its_unit_too() {
    let mut m = msi_on_the_holder();
    m.no(1, Syscall::CapDelete(s(IRQ)), Error::Again);
    assert_eq!(requests_in_flight(&m, 1), 1);
    m.run(30 * MS);
    assert_eq!(requests_in_flight(&m, 1), 0);
    m.ok(1, Syscall::CapDelete(s(IRQ)));
    assert!(m.sys.canonical().units[&IOMMU_ROLE].remap.is_empty());
}

// Lot 4, L4-02. The request is what retires the handle of a unit that
// does not confirm (D-139, D-150: "the shortcut [...] skipped the claim
// that must end first and the retirement at a failed unit"): the unit of
// the holder hangs, the removal's invalidation is answered *failed*, and
// the vector is not claimed again.
#[test]
fn the_handle_of_an_msi_removed_on_a_hung_unit_of_its_own_node_is_retired() {
    let mut m = msi_on_the_holder();
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    m.no(1, Syscall::CapDelete(s(IRQ)), Error::Again);
    m.run(60 * MS);
    assert!(!m.sys.canonical().retired.is_empty());
    m.ok(1, Syscall::CapDelete(s(IRQ)));
    m.no(1, msi_claim(IRQ + 2), Error::Busy);
}

/// A third thread, with its context 311 of 0.2 ms each 10 ms, waits on
/// core 0 behind the first thread's context for 3 ms; then its context is
/// configured anew on core number `core` with 2 ms each 10 ms. What its
/// context reads then: its budget, its remaining budget, its deadline and
/// its pending parameters.
fn reconfigured_on(core: u64) -> (u64, i64, u64, Option<(u64, u64)>) {
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    let third = k.thread_on(310, 0);
    let context = k.m.context_of(third).expect("its context");
    let now = k.m.now;
    k.m.set_now(now + 3 * MS);
    k.ok(Syscall::SchedConfigure {
        sched: s(311),
        node: node_cap(0),
        core,
        budget: 2 * MS,
        period: 10 * MS,
        class: 1,
    });
    let sched = &k.m.scheds[&context];
    (sched.budget, sched.remaining, sched.deadline, sched.pending)
}

// Lot 4, L4-03. A context that was eligible before and after an event is
// not woken (Scheduling contexts), and new parameters are pending until
// the next replenishment or the next reset of the wake-up rule: moving a
// waiting context to another core renews nothing. It keeps what the same
// call leaves on its own core.
#[test]
fn moving_a_waiting_context_to_another_core_does_not_wake_it() {
    let stayed = reconfigured_on(0);
    assert_eq!(stayed.3, Some((2 * MS, 10 * MS)), "pending");
    assert_eq!(reconfigured_on(1), stayed);
}

// Lot 4, L4-06. Trace code 5, a fault recorded in its thread: word 5 is
// the fault kind (low 32 bits) and the architecture's exception number
// (high 32), word 2 of the fault (What the kernel traces).
#[test]
fn the_trace_of_a_recorded_fault_carries_its_exception_number() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.m.fault(k.t, FaultKind::Page, [14, 2, 0xdead_0000]);
    let (kind, words) = k.m.threads[&k.t].recorded.expect("recorded");
    assert_eq!((kind, words[2]), (FaultKind::Page as u32, 14));
    let ring = k.m.trace_ring(0, false);
    let recorded = TraceCode::FaultRecorded as u64;
    let entry = ring.iter().rfind(|e| e.word2 & 0xFFFF == recorded);
    let payload = entry.expect("its trace entry").payload;
    assert_eq!(payload, [14 << 32 | 1, words[0], words[4]]);
}

// Lot 4, L4-08. Unbinding makes the irq idle and masks a level source
// where it is masked at its arrival; `irq_bind` with notify 0 masks a
// level source at once (Interrupts, the states of an irq). On RISC-V the
// APLIC source is disabled: a line that rises while the irq is idle and
// unbound sends no MSI.
#[test]
fn unbinding_an_armed_level_line_masks_it_at_once() {
    let mut m = S::on(&machine(Arch::Riscv64, 1, &[&[0]]));
    let claim = Syscall::IrqClaim {
        control: s(START_SLOT_IRQ_CONTROL),
        source: IrqSource::line(GSI, true, false).raw(),
        core: 0,
        vector: 48,
        destination: s(IRQ),
    };
    succeeds(&mut m, 0, claim, 30 * MS);
    let notify = m.retype(0, ObjectKind::Notify, 0, NOTIFY);
    m.ok(0, notify);
    let bind = |notify| Syscall::IrqBind {
        irq: s(IRQ),
        notify,
    };
    let enabled = |m: &S| m.sys.canonical().aplic[&GSI].enabled;
    m.ok(0, bind(s(NOTIFY)));
    assert!(enabled(&m));
    m.ok(0, bind(SlotAddr::NONE));
    assert!(!enabled(&m));
    m.sys.line_input(GSI, true);
    assert!(!m.sys.aplic_deliver(GSI), "an MSI for an idle irq");
    assert_eq!(m.sys.check(), Ok(()));
    // Bound again, the line that stayed high is signalled once.
    m.ok(0, bind(s(NOTIFY)));
    assert!(enabled(&m));
    assert!(m.sys.aplic_deliver(GSI));
    assert!(!m.sys.aplic_deliver(GSI));
}

// Lot 4, L4-09. A core that does not keep its debug area clears it,
// starts at 1 and writes one entry of code 12 (Debugging, D-141): for a
// header whose magic or version is wrong as for one whose next sequence
// is.
#[test]
fn a_debug_area_whose_magic_or_version_is_wrong_is_cleared_and_traced() {
    let edits: [fn(&mut DebugArea); 2] = [|area| area.magic ^= 1, |area| area.version ^= 1];
    for edit in edits {
        let m = restart_over(edit);
        let ring = m.sys.node(0).trace_ring(1, false);
        let cleared = TraceCode::AreaCleared as u64;
        assert_eq!((ring[0].seq, ring[0].word2 & 0xFFFF), (1, cleared));
    }
}

// Lot 4, L4-11. `thread_resume` is `busy` if the thread is bound to no
// capability space (Threads); a thread of a revoked task is still bound
// to its space, and is resumed.
#[test]
fn a_thread_of_a_revoked_task_is_resumed() {
    let (mut k, other) = two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Task, 0, 320));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(320),
    });
    // The revocation waits for the thread to leave its core.
    k.no(Syscall::TaskRevoke(s(320)), Error::Again);
    let thread = &k.m.threads[&other];
    let task = thread.task.expect("attached");
    assert!(k.m.tasks[&task].revoked && thread.cspace.is_some());
    k.ok(Syscall::ThreadResume(s(300)));
}
