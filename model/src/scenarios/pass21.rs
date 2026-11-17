//! The scenarios of pass 21 (D-140): the corrections of the thirteenth
//! contract review, and the twelfth model review judged. Each test is named
//! after the sentence it guards.

use super::pass17::detach_of;
use super::*;
use crate::cores::Completes;
use crate::locks::{Cost, Step};
use crate::rings::{Target, UnitOp};
use dodos_abi::{AttachState, AttachWord, UnitResult};

/// The detach operation of a device as an `iospace_attach` with iospace 0.
/// Node 0 of four, with three IOMMU units, attaches the device through the
/// first; returns the machine and the root its attach word names.
fn attached_on_the_first_of_three_units() -> (S, u64) {
    use super::units::{DEVICE, attached};
    let mut m = super::pass17::three_units();
    attached(&mut m, 0);
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(AttachState::Attached));
    (m, word.root())
}

// Devices and shared hardware, the operands of each operation: a detach is
// tied to its requester as an attach is (D-140).

#[test]
fn a_forged_detach_from_another_node_through_another_unit_is_refused() {
    use super::units::DEVICE;
    let (mut m, root) = attached_on_the_first_of_three_units();
    // Node 1 asks the holder of the second unit, where the device has no
    // context entry, to detach node 0's device.
    let role = 1;
    let holder = m.sys.role(role).holder();
    let forged = UnitOp::Detach {
        device: DEVICE as u16,
        domain: 0,
        root,
    };
    assert!(super::pass17::judged(&mut m, holder, 1, role, forged));
    m.run(10 * MS);
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    assert_eq!(word.state(), Some(AttachState::Attached));
    // Even where the word reads *detached* with that root, the identifier
    // word names node 0, not the requester.
    let detached = AttachWord::new(AttachState::Detached, root).unwrap().raw();
    m.sys.edit_table(|t| {
        t.attach.insert(DEVICE as u32, detached);
    });
    assert!(super::pass17::judged(&mut m, holder, 1, role, forged));
}

#[test]
fn an_owners_detach_on_the_wrong_unit_with_no_entry_is_refused_unless_the_word_reads_detached() {
    use super::units::DEVICE;
    let (mut m, root) = attached_on_the_first_of_three_units();
    let role = 1;
    let holder = m.sys.role(role).holder();
    let detach = UnitOp::Detach {
        device: DEVICE as u16,
        domain: 0,
        root,
    };
    // The word reads *attached*: the real unit still translates.
    assert!(super::pass17::judged(&mut m, holder, 0, role, detach));
    // A detach served again once the word reads *detached* with that root:
    // accepted, it is idempotent.
    let detached = AttachWord::new(AttachState::Detached, root).unwrap().raw();
    m.sys.edit_table(|t| {
        t.attach.insert(DEVICE as u32, detached);
    });
    assert!(!super::pass17::judged(&mut m, holder, 0, role, detach));
    // With another root, refused.
    let other = UnitOp::Detach {
        device: DEVICE as u16,
        domain: 0,
        root: root + 0x1000,
    };
    assert!(super::pass17::judged(&mut m, holder, 0, role, other));
}

// Interrupts, the irq: the retired word of each core survives R6 and the
// next start of the core (D-140); and R1 on a failed unit holds only the
// nodes whose devices sat behind it (Reclaim, R1, D-140).

#[test]
fn a_retired_vector_stays_retired_after_r6_and_the_next_start_of_its_core() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let n = (1..3).find(|&n| n != holder).unwrap();
    let killer = (0..3).find(|&k| k != holder && k != n).unwrap_or(holder);
    let claim = |slot| Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: u64::from(n),
        vector: 48,
        destination: s(slot),
    };
    super::pass17::succeeds(&mut m, n, claim(210), 30 * MS);
    let handle = super::pass17::handle(n, 48);
    assert!(m.sys.canonical().irq_tables.contains_key(&handle));
    // The unit never confirms again: R1 for n fails it. n had no device
    // behind it, so its barrier holds all the same and R6 runs.
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    m.ok(killer, super::pass17::kill(n));
    for _ in 0..20 {
        if m.state(n) == MemberState::Dead {
            break;
        }
        m.run(10 * MS);
    }
    assert!(m.sys.canonical().roles[IOMMU_ROLE].failed());
    assert_eq!(
        m.state(n),
        MemberState::Dead,
        "R1 done for a node with no device"
    );
    assert!(m.sys.canonical().retired.contains(&handle));
    assert!(!m.sys.canonical().irq_tables.contains_key(&handle));
    // n starts again on the same core: the entry stub cleared the table,
    // and the vector is still refused.
    m.ok(killer, super::pass17::fresh(n, 1 << n));
    m.run(30 * MS);
    assert_eq!(m.state(n), MemberState::Live);
    m.restarted(n);
    assert_eq!(m.call(n, claim(211)), Err(Error::Busy));
}

// A node of several cores: the third order, at a bind (D-140).

#[test]
fn a_bind_on_a_node_of_several_cores_orders_its_write_of_the_binding_before_its_read_of_the_word() {
    for (cores, ordered) in [(2, true), (1, false)] {
        let mut k = N::on(&one_node(Arch::X86_64, cores));
        k.ok(k.retype(ObjectKind::Notify, 0, 350));
        k.ok(k.retype(ObjectKind::Endpoint, 0, 351));
        k.ok(Syscall::NotifyBind {
            notify: s(350),
            endpoint: s(351),
        });
        let fenced = k.m.locks.steps.contains(&Step::Pay(Cost::Fence));
        assert_eq!(fenced, ordered, "{cores} cores");
    }
}

// TLB, the request of a core: its start instant is read under the node
// lock, when it is written (D-140).

#[test]
fn the_start_instant_of_a_request_is_read_under_the_node_lock() {
    let (mut k, _) = super::pass20::ready_untyped(Arch::X86_64, 2);
    k.m.lock_wait = 3 * MS;
    let entry = k.m.membership_now();
    let retype = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, retype), Err(Error::Again));
    let request = k.m.cores[0].request.clone().expect("the barrier");
    assert_eq!(request.started, entry + 3 * MS);
}

// G9: the target core of a line's route on ARM64 and RISC-V (D-140).

#[test]
fn the_route_of_a_line_on_arm64_and_risc_v_names_only_a_core_of_the_requester() {
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1], &[2]]));
        let role = m.sys.node(0).ioapic_role(5).unwrap();
        let holder = m.sys.role(role).holder();
        let a = (0..3).find(|&n| n != holder).unwrap();
        let b = (0..3).find(|&n| n != holder && n != a).unwrap_or(holder);
        let claim = Syscall::IrqClaim {
            control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
            source: dodos_abi::IrqSource::line(5, false, false).raw(),
            core: u64::from(a),
            vector: 48,
            destination: s(210),
        };
        super::pass17::succeeds(&mut m, a, claim, 30 * MS);
        let route = |core: u16| UnitOp::Redirection {
            gsi: 5,
            claimed: true,
            level: false,
            low: false,
            target: Target::Direct {
                vector: 48,
                apic: 2 * u32::from(core),
            },
        };
        // The affinity, or the target hart, of another node's core: refused.
        assert!(
            super::pass17::judged(&mut m, holder, a, role, route(b)),
            "{arch:?}"
        );
        assert!(
            !super::pass17::judged(&mut m, holder, a, role, route(a)),
            "{arch:?}"
        );
    }
}

// The attach word against every event: a holder that answers done to a
// detach it did not serve (D-140).

#[test]
fn a_detach_the_holder_refused_at_a_step_of_a_destruction_takes_the_device_out_of_the_header() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let key = DEVICE as u32;
    let word = m.sys.canonical().attach_word(key);
    let node = m.sys.node_mut(0);
    node.detach_answered(
        IOMMU_ROLE,
        DEVICE as u16,
        UnitResult::Done,
        crate::units::Waiter::Step,
    );
    assert_eq!(m.sys.node(0).census().devices, 0);
    assert_eq!(m.sys.canonical().attach_word(key), word);
}

// Interrupts, the irq: the removal of a line's irq invalidates its
// remapping entry after the line is masked (pass 20 left it unmodelled).

/// Node `a` (holding no unit role) claims line 5 on its core with
/// interrupt remapping; returns the machine, `a` and the handle.
fn line_claimed() -> (S, u16, u16) {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let a = (0..3).find(|&n| n != holder).unwrap();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, true, false).raw(),
        core: u64::from(a),
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, a, claim, 30 * MS);
    let handle = super::pass17::handle(a, 48);
    (m, a, handle)
}

fn remapped(m: &S, handle: u16) -> bool {
    use super::units::IOMMU_ROLE;
    m.sys.canonical().units[&IOMMU_ROLE]
        .remap
        .contains_key(&u32::from(handle))
}

#[test]
fn the_removal_of_a_lines_irq_invalidates_its_remapping_entry_after_the_mask() {
    use super::units::IOMMU_ROLE;
    let (mut m, a, handle) = line_claimed();
    assert!(remapped(&m, handle));
    let before = super::pass17::submissions(&m, IOMMU_ROLE);
    // The first step asks the IOAPIC's holder to mask the line; the entry
    // is still there.
    assert_eq!(m.call(a, Syscall::CapDelete(s(210))), Err(Error::Again));
    assert!(remapped(&m, handle));
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        m.settle();
        outcome = m.call(a, Syscall::CapDelete(s(210)));
        if outcome != Err(Error::Again) {
            break;
        }
    }
    assert_eq!(outcome, DONE);
    assert!(!remapped(&m, handle));
    assert!(super::pass17::submissions(&m, IOMMU_ROLE) > before);
    assert!(m.sys.canonical().retired.is_empty());
}

#[test]
fn a_lines_vector_whose_invalidation_a_failed_unit_did_not_confirm_is_retired() {
    use super::units::IOMMU_ROLE;
    let (mut m, a, handle) = line_claimed();
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    let mut outcome = Err(Error::Again);
    for _ in 0..12 {
        outcome = m.call(a, Syscall::CapDelete(s(210)));
        if outcome != Err(Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    assert!(m.sys.canonical().retired.contains(&handle));
}

// The twelfth model review (DeepSeek), finding by finding.

#[test]
fn deepseek12_1_a_line_whose_entry_a_rebuild_wrote_masked_raises_nothing_until_it_is_armed() {
    let (mut m, a, _) = line_claimed();
    let role = m.sys.node(a).ioapic_role(5).unwrap();
    m.sys.edit_table(|t| {
        t.units.entry(role).or_default().masked.insert(5);
    });
    let irq = |m: &S| *m.sys.node(a).irqs.values().next().expect("the irq");
    // Bound to a notify: armed, so unmasked at its controller.
    let notify = m.retype(a, ObjectKind::Notify, 0, 211);
    m.ok(a, notify);
    m.ok(
        a,
        Syscall::IrqBind {
            irq: s(210),
            notify: s(211),
        },
    );
    assert!(!m.sys.canonical().units[&role].masked.contains(&5));
    // Masked again (a rebuild): an arrival raises nothing.
    m.sys.edit_table(|t| {
        t.units.entry(role).or_default().masked.insert(5);
    });
    m.sys.node_mut(a).interrupt_on(0, 48);
    assert_eq!(irq(&m).state, crate::IrqState::Armed);
    m.sys.edit_table(|t| {
        t.units.entry(role).or_default().masked.remove(&5);
    });
    m.sys.node_mut(a).interrupt_on(0, 48);
    assert_eq!(irq(&m).state, crate::IrqState::Fired);
}

#[test]
fn deepseek12_5_a_detach_on_a_failed_unit_is_busy_and_leaves_the_header() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    m.sys
        .edit_table(|t| t.roles[IOMMU_ROLE] = t.roles[IOMMU_ROLE].failed_now());
    let detach = detach_of(attach(&m, 0, IOSPACE, DEVICE));
    assert_eq!(m.call(0, detach), Err(Error::Busy));
    assert_eq!(m.sys.node(0).census().devices, 1);
}

#[test]
fn deepseek12_6_a_barrier_while_the_cores_request_is_in_use_starts_nothing() {
    let (mut k, first) = super::pass20::ready_untyped(Arch::X86_64, 2);
    let retype = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, retype), Err(Error::Again));
    let barrier = k.m.cores[0].request.clone().expect("the barrier");
    // A second untyped ready at a later epoch, on the same core.
    k.ok(k.retype(ObjectKind::Untyped, 1 << 16, 360));
    k.ok(retype_from(360, ObjectKind::Sched, 0, 361));
    k.ok(Syscall::CapDelete(s(361)));
    let second = k.m.object_at(k.t, s(360)).unwrap();
    assert!(k.m.untypeds[&second].ready > k.m.untypeds[&first].ready);
    let retype = retype_from(360, ObjectKind::Notify, 0, 362);
    assert_eq!(k.call(k.t, retype), Err(Error::Again));
    assert_eq!(k.m.cores[0].request, Some(barrier));
    assert_eq!(k.m.check(), Ok(()));
}

#[test]
fn deepseek12_7_the_due_event_of_a_request_falls_at_its_start_plus_core_answer_ns_on_the_hardware_clock()
 {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(1);
    let _ = k.thread_on(300, 1);
    k.m.table.stopped_time = 50 * MS;
    let later = k.m.now + 100 * MS;
    k.m.set_now(later);
    k.m.start_barrier();
    let at = *k.m.at;
    let request = k.m.cores[at].request.clone().expect("a request");
    let due = request.started + k.m.table.core_answer_ns + 50 * MS;
    assert_eq!(k.m.request_due(at), Some(due));
    assert!(k.m.hardware(at).is_some_and(|h| h <= due));
}

#[test]
fn deepseek12_8_a_second_attach_on_a_pinned_word_is_busy_and_a_failed_attach_frees_the_identifier()
{
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    let mut m = three();
    m.ok(0, super::place_first(0, 0));
    let other = m.thread_on(0, 300, 0);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call.clone()), WAITS);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let again = call;
    assert_eq!(m.call_by(0, other, again), Err(Error::Busy));
    let node = m.sys.node_mut(0);
    node.attach_answered(IOMMU_ROLE, DEVICE as u16, root, UnitResult::Failed);
    assert_eq!(m.sys.canonical().identifiers.get(&(DEVICE as u32)), None);
}

#[test]
fn deepseek12_8_an_answer_judged_by_a_word_of_another_root_changes_nothing() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let key = DEVICE as u32;
    let root = m.sys.canonical().attach_word(key).root();
    // A detach: the word reads *detached* with another root.
    let other = AttachWord::new(AttachState::Detached, root + 0x1000)
        .unwrap()
        .raw();
    m.sys.edit_table(|t| {
        t.attach.insert(key, other);
    });
    let node = m.sys.node_mut(0);
    node.detach_answered(
        IOMMU_ROLE,
        DEVICE as u16,
        UnitResult::Done,
        crate::units::Waiter::Thread(None),
    );
    assert_eq!(m.sys.canonical().attach_word(key).raw(), other);
}

#[test]
fn deepseek12_8_a_detach_in_flight_from_a_destruction_is_not_asked_again() {
    use super::units::{DEVICE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let node = m.sys.node_mut(0);
    let iommu = *node.iommus.keys().next().unwrap();
    assert_eq!(node.detach(iommu, DEVICE as u16), Ok(()));
    let asked = node.unit_requests.iter().flatten().count();
    assert_eq!(node.detach(iommu, DEVICE as u16), Err(Error::Again));
    assert_eq!(node.unit_requests.iter().flatten().count(), asked);
}

#[test]
fn deepseek12_9_the_start_up_judges_core_answer_ns_for_the_largest_n_the_image_supports() {
    let mut machine = one_node(Arch::X86_64, 2);
    machine.cores_supported = 64;
    // 2 × 64 × 10 µs + 5 µs = 1.285 ms, above a CORE_ANSWER_NS of 1 ms.
    machine.hold_ns = 10_000;
    machine.delivery_ns = 5_000;
    machine.membership.core_answer_ms = 1;
    assert_eq!(
        Model::boot(&machine).err(),
        Some(crate::boot::Refusal::Floors)
    );
}

#[test]
fn deepseek12_m25_a_core_answer_ns_equal_to_the_healthy_wait_is_refused() {
    let mut machine = one_node(Arch::X86_64, 4);
    machine.cores_supported = 4;
    // 2 × 4 × 1.1875 ms + 0.5 ms = 10 ms exactly: "not above" is refused.
    machine.hold_ns = 1_187_500;
    machine.delivery_ns = 500_000;
    assert_eq!(
        Model::boot(&machine).err(),
        Some(crate::boot::Refusal::Floors)
    );
}

#[test]
fn deepseek12_10_a_barrier_that_completes_last_with_an_older_epoch_does_not_lower_the_barrier_epoch()
 {
    let (mut k, _) = super::pass20::ready_untyped(Arch::X86_64, 3);
    let one = k.thread_on(300, 1);
    let retype = retype_from(350, ObjectKind::Notify, 0, 352);
    assert_eq!(k.call(k.t, retype), Err(Error::Again));
    let older = k.m.kernel_epoch;
    // A second untyped ready at a later epoch; core 1 starts its barrier.
    k.ok(k.retype(ObjectKind::Untyped, 1 << 16, 360));
    k.ok(retype_from(360, ObjectKind::Sched, 0, 361));
    k.ok(Syscall::CapDelete(s(361)));
    let newer = k.m.kernel_epoch;
    assert!(newer > older);
    let retype = retype_from(360, ObjectKind::Notify, 0, 362);
    assert_eq!(k.call(one, retype), Err(Error::Again));
    assert_eq!(
        k.m.cores[1].request.as_ref().map(|r| r.completes.clone()),
        Some(Completes::Barrier(newer))
    );
    // Core 1's barrier completes first, core 0's last.
    k.m.take_interrupts(0);
    k.m.take_interrupts(2);
    assert_eq!(k.m.barrier_epoch, newer);
    k.m.take_interrupts(1);
    assert_eq!(k.m.barrier_epoch, newer);
}

#[test]
fn deepseek12_m29_the_removal_of_an_msi_waits_for_the_answer_of_its_invalidation() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 3, 1, 0).raw(),
        core: 0,
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, 0, claim, 30 * MS);
    assert_ne!(m.sys.role(IOMMU_ROLE).holder(), 0);
    assert_eq!(m.call(0, Syscall::CapDelete(s(210))), Err(Error::Again));
    assert_eq!(m.call(0, Syscall::CapDelete(s(210))), Err(Error::Again));
    assert_eq!(m.sys.node(0).irqs.len(), 1);
}

#[test]
fn deepseek12_m33_the_reclaim_of_a_second_death_of_one_slot_starts_afresh() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, super::pass17::kill(2));
    m.run(30 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    m.ok(0, super::pass17::fresh(2, 0b100));
    m.run(30 * MS);
    assert_eq!(m.state(2), MemberState::Live);
    let generation = m.sys.member(2).generation();
    m.ok(0, super::pass17::kill(2));
    m.run(MS);
    // Node 1 holds a record of the second death's generation; node 0 may
    // have none yet (the final review's E26: both were conditional).
    let reclaim = m.sys.node(1).reclaims.get(&2).expect("node 1's record");
    assert_eq!(reclaim.generation, generation);
    if let Some(reclaim) = m.sys.node(0).reclaims.get(&2) {
        assert_eq!(reclaim.generation, generation);
    }
    m.run(30 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
}

#[test]
fn deepseek12_m16_the_report_of_an_edge_line_whose_irq_is_dying_says_masked() {
    use crate::rings::Item;
    let mut m = super::units::three();
    let holder = m.sys.role(super::units::IOMMU_ROLE).holder();
    let a = (0..3).find(|&n| n != holder).unwrap();
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, false, false).raw(),
        core: u64::from(a),
        vector: 48,
        destination: s(210),
    };
    super::pass17::succeeds(&mut m, a, claim, 30 * MS);
    let role = m.sys.node(a).ioapic_role(5).unwrap();
    let unmasked = |m: &S| {
        m.sys
            .node(a)
            .items(role)
            .iter()
            .any(|item| matches!(item, Item::Line(5, _, _, _, unmasked) if *unmasked))
    };
    assert!(unmasked(&m), "an edge line claimed is unmasked");
    let node = m.sys.node_mut(a);
    node.irqs.values_mut().for_each(|irq| irq.removing = true);
    assert!(!unmasked(&m), "an edge line whose irq is dying is masked");
}

#[test]
fn deepseek12_m20_a_rebuild_writes_no_entry_for_a_report_whose_root_differs_from_the_word() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let owner = (0, m.sys.canonical().generation(0));
    let node = m.sys.node_mut(holder);
    assert!(node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root, owner));
    assert!(!node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root + 0x1000, owner));
}

#[test]
fn deepseek12_m15_an_attach_answer_judged_by_a_pinned_word_of_another_root_changes_nothing() {
    use super::units::{DEVICE, IOMMU_ROLE};
    let (mut m, root) = super::pass20::attach_in_flight();
    let word = m.sys.canonical().attach_word(DEVICE as u32);
    let node = m.sys.node_mut(0);
    node.attach_answered(IOMMU_ROLE, DEVICE as u16, root + 0x1000, UnitResult::Failed);
    assert_eq!(m.sys.canonical().attach_word(DEVICE as u32), word);
}

#[test]
fn deepseek12_8_a_detach_of_a_device_another_node_pinned_is_busy_before_denied() {
    use super::units::{DEVICE, IOSPACE, attach};
    let (mut m, _) = super::pass20::attach_in_flight();
    let other = (1..3).find(|&n| n != m.sys.role(super::units::IOMMU_ROLE).holder());
    let n = other.unwrap_or(1);
    let detach = detach_of(attach(&m, n, IOSPACE, DEVICE));
    assert_eq!(m.call(n, detach), Err(Error::Busy));
}

#[test]
fn deepseek12_11_the_destruction_of_a_device_block_whose_watermark_is_ready_waits_for_the_barrier()
{
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    let index = (0..k.m.device_count())
        .find(|&i| k.m.device_entry(i).base() == 2 << 20)
        .expect("the device range") as u64;
    k.ok(Syscall::MemoryTake {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 1,
        place: index,
        size: 0,
        destination: s(220),
    });
    let block = k.m.object_at(k.t, s(220)).unwrap();
    k.ok(Syscall::UntypedRetype {
        untyped: s(220),
        kind: ObjectKind::Frame,
        size: 1,
        page: 0,
        memory: 1,
        destination: s(221),
    });
    k.ok(Syscall::CapDelete(s(221)));
    assert!(k.m.untypeds[&block].ready.is_some());
    // The step that would destroy the block returns the watermark first:
    // it starts the barrier and returns `again`, the block still there.
    assert_eq!(k.call(k.t, Syscall::CapDelete(s(220))), Err(Error::Again));
    assert!(k.m.untypeds.contains_key(&block));
    k.interrupts();
    let mut steps = 0;
    while k.call(k.t, Syscall::CapDelete(s(220))) == Err(Error::Again) {
        k.interrupts();
        steps += 1;
        assert!(steps < 8);
    }
    assert!(!k.m.untypeds.contains_key(&block));
}

#[test]
fn a_node_with_a_device_behind_a_failed_unit_stays_fenced() {
    use super::units::{IOMMU_ROLE, attached, three};
    let mut m = three();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let n = (1..3).find(|&n| n != holder).unwrap();
    let killer = (0..3).find(|&k| k != holder && k != n).unwrap_or(holder);
    attached(&mut m, n);
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    m.ok(killer, super::pass17::kill(n));
    m.run(100 * MS);
    assert!(m.sys.canonical().roles[IOMMU_ROLE].failed());
    assert_eq!(m.state(n), MemberState::Fenced);
}
