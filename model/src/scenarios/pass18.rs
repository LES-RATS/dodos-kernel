//! The scenarios of pass 18 (D-135): the corrections of the tenth review of
//! the contract (`Docs/reviews/2026-11-17-contract-review-10-*.md`) and of
//! the ninth review of the model (`2026-11-17-model-review-9-*.md`), each a
//! test named after the sentence it guards.

use super::pass17::{fresh, kill};
use super::*;
use crate::nodes::HwAction;
use crate::system::{Event, Hardware};

/// Fires the first action in flight that `pick` chooses, moving the clock
/// until one is there.
fn fire_action(m: &mut S, pick: impl Fn(&(u16, HwAction)) -> bool) {
    for _ in 0..1000 {
        if let Some(index) = m.sys.actions().iter().position(&pick) {
            m.sys.fire(Event::Action(index));
            return;
        }
        match m
            .sys
            .events()
            .into_iter()
            .find(|e| !matches!(e, Event::Action(_)))
        {
            Some(event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    }
    panic!("no such action");
}

/// Node 1 of cores 1 and 2 killed and dead, then started fresh again: its
/// start actions are in flight, none carried out.
fn restarting(arch: Arch) -> S {
    let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
    m.ok(0, kill(1));
    m.run(dodos_abi::FENCE_NS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, fresh(1, 0b110));
    m
}

// The stop of a core, by architecture (the ninth model review, findings 1
// to 4; Node slots and core words, the start request and the entry stub).

#[test]
fn a_late_start_wake_of_a_running_core_changes_nothing_on_arm64_and_risc_v() {
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
        let parked = m.sys.canonical().core_beacons[1].parked;
        // `CPU_ON` or `sbi_hart_start` to a core already on starts nothing.
        m.sys.late_wake(1);
        m.settle();
        assert_eq!(m.sys.hardware(1), Hardware::Running, "{arch:?}");
        assert_eq!(m.sys.canonical().core_beacons[1].parked, parked);
        assert_eq!(m.state(1), MemberState::Live);
    }
}

#[test]
fn a_stop_that_reaches_an_unmarked_core_changes_nothing_on_every_architecture() {
    for arch in [Arch::X86_64, Arch::Aarch64, Arch::Riscv64] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1, 2]]));
        // A fence signal on a word without the fence mark: the gate sends
        // none (G10), and the handler parks only on a marked word.
        m.sys.push_action(0, HwAction::Stop { core: 1 });
        m.settle();
        assert_eq!(m.sys.hardware(1), Hardware::Running, "{arch:?}");
        assert_eq!(m.state(1), MemberState::Live, "{arch:?}");
    }
}

#[test]
fn an_arm64_core_that_waits_in_its_node_for_the_first_core_is_stopped_by_the_fence() {
    let mut m = restarting(Arch::Aarch64);
    // Core 2 enters first: it registered SDEI event 0 in the entry stub and
    // waits for core 1, which builds the node.
    fire_action(&mut m, |a| matches!(a, (_, HwAction::Start { core: 2 })));
    assert_eq!(m.sys.hardware(2), Hardware::Entering);
    m.ok(0, kill(1));
    fire_action(&mut m, |a| matches!(a, (_, HwAction::Stop { core: 2 })));
    assert_eq!(m.sys.hardware(2), Hardware::Parking);
    assert_eq!(
        m.sys.canonical().core_beacons[2].parked,
        m.sys.park_request(2)
    );
}

#[test]
fn an_x86_64_core_in_vmx_root_that_waits_in_its_node_is_stopped_by_its_nmi() {
    let mut m = restarting(Arch::X86_64);
    fire_action(&mut m, |a| matches!(a, (_, HwAction::Start { core: 2 })));
    for _ in 0..4 {
        if m.sys.hardware(2) == Hardware::Entering {
            break;
        }
        m.sys.fire(Event::EntryStep { core: 2 });
    }
    assert_eq!(m.sys.hardware(2), Hardware::Entering);
    // INIT is blocked in VMX root operation; the node kernel's NMI handler,
    // installed by the entry stub, parks the core.
    m.sys.set_vmx(2, true);
    m.ok(0, kill(1));
    fire_action(&mut m, |a| matches!(a, (_, HwAction::Stop { core: 2 })));
    assert_eq!(m.sys.hardware(2), Hardware::Stub { window: false });
    let beacon = m.sys.canonical().core_beacons[2];
    assert_eq!(beacon.parked, m.sys.park_request(2));
    // The handler wrote the crash record of this generation (the first
    // kill wrote one of the generation before).
    let record = m.sys.canonical().crash.areas[2].record;
    assert_eq!(
        record.map(|r| (r.reason, r.generation)),
        Some((
            dodos_abi::CrashReason::Fenced as u32,
            m.sys.member(1).generation()
        ))
    );
}

#[test]
fn a_stale_wake_of_a_halted_park_stub_writes_no_parked() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.sys.hardware(1), Hardware::Stub { window: false });
    // A park request is raised while the stub is halted, then a stale wake
    // reaches it: the stub's NMI handler writes no `parked` (D-149: only
    // the stub started by a SIPI writes it).
    m.sys.edit_table(|t| t.park_requests[1] += 1);
    assert_eq!(m.sys.check(), Ok(()));
    m.sys.late_wake(1);
    assert_eq!(m.sys.check(), Ok(()));
    assert_ne!(
        m.sys.canonical().core_beacons[1].parked,
        m.sys.park_request(1)
    );
}

// The start-up layout (the ninth model review, finding 8, and its mutation
// M3; Start-up state).

#[test]
fn the_start_up_refuses_a_layout_that_leaves_the_core_firmware_booted_in_no_node() {
    let machine = machine(Arch::X86_64, 2, &[&[1]]);
    assert_eq!(machine.layout().err(), Some(crate::boot::Refusal::Nodes));
}

#[test]
fn one_node_of_class_0_on_more_cores_than_one_window_runs_on_the_booted_core() {
    let mut machine = machine(Arch::X86_64, 65, &[&[0]]);
    // The bootstrap core of class 0: its cleared BSP flag not declared
    // honoured (D-149).
    machine.membership.flags = 0;
    machine.cores_supported = 128;
    assert_eq!(machine.initial_nodes(), vec![vec![0]]);
    machine.cores.truncate(64);
    assert_eq!(machine.initial_nodes(), vec![(0..64).collect::<Vec<_>>()]);
}

// The fence record (the ninth model review, finding 5; The fence, the fence
// record, gap 6, D-134).

#[test]
fn the_fence_record_is_written_anew_by_the_first_fence_action_not_by_the_accusation() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.ok(0, kill(1));
    m.run(10 * MS);
    let old = m.sys.member(1).generation();
    let confirmed = m.sys.canonical().fences[1].confirmed;
    assert!(confirmed.is_some());
    m.ok(0, fresh(1, 0b110));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    let new = m.sys.member(1).generation();
    // The accusation: the record still concerns the generation before.
    m.ok(0, kill(1));
    let record = m.sys.canonical().fences[1].clone();
    assert_eq!((record.generation, record.confirmed), (old, confirmed));
    assert_eq!(record.begun(new), None);
    // The first fence action writes it anew.
    m.run(MS);
    let record = m.sys.canonical().fences[1].clone();
    assert_eq!(record.generation, new);
    assert!(record.begun(new).is_some());
}

#[test]
fn the_fence_record_keeps_the_last_stop_of_a_core_at_its_number_mod_64() {
    let mut m = S::on(&machine(Arch::X86_64, 66, &[&[0], &[65]]));
    m.ok(0, kill(1));
    m.run(MS);
    let record = m.sys.canonical().fences[1].clone();
    assert_eq!(
        record.last_stop.keys().copied().collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(Some(record.last_stop(65)), record.started);
}

// The claim of an irq over several units (the ninth model review, finding 7
// and its mutation M23; Devices and shared hardware, several remapping
// units, gap 12).

fn claim_on(core: u64, source: u64, vector: u64, destination: u32) -> Syscall {
    Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source,
        core,
        vector,
        destination: s(destination),
    }
}

#[test]
fn a_claim_whose_first_unit_is_held_by_the_claimer_and_failed_still_asks_the_later_units() {
    let mut m = super::pass17::three_units();
    let roles = m.sys.node(1).segment_roles(0);
    assert_eq!(m.sys.role(roles[0]).holder(), 1);
    // A first claim finds the unit broken: it is failed from then on.
    m.sys.break_unit(roles[0]);
    let first = dodos_abi::IrqSource::msi(0, 3, 2, 0).raw();
    assert_eq!(m.call(1, claim_on(1, first, 49, 211)), WAITS);
    m.run(2 * dodos_abi::IOMMU_WAIT_NS + 60 * MS);
    assert!(m.sys.role(roles[0]).failed());
    let later: Vec<u64> = roles[1..]
        .iter()
        .map(|&r| super::pass17::submissions(&m, r))
        .collect();
    // The first unit is this node's and failed: it is passed over at once,
    // and the call waits for the later ones.
    let msi = dodos_abi::IrqSource::msi(0, 3, 1, 0).raw();
    assert_eq!(m.call(1, claim_on(1, msi, 48, 210)), WAITS);
    m.run(dodos_abi::IOMMU_WAIT_NS + 60 * MS);
    assert_eq!(m.woken(1), Some(&Err(Error::Busy)));
    for (i, &role) in roles[1..].iter().enumerate() {
        let asked = m
            .sys
            .node(m.sys.role(role).holder())
            .table
            .units
            .get(&role)
            .map_or(0, |u| u.next);
        assert_eq!(asked, later[i] + 1, "unit of role {role}");
    }
}

#[test]
fn a_claim_that_finds_its_first_unit_failed_before_a_working_one_ends_busy() {
    let mut m = super::pass17::three_units();
    let roles = m.sys.node(0).segment_roles(0);
    m.sys.break_unit(roles[0]);
    let msi = dodos_abi::IrqSource::msi(0, 3, 1, 0).raw();
    assert_eq!(m.call(0, claim_on(0, msi, 48, 210)), WAITS);
    m.run(dodos_abi::IOMMU_WAIT_NS + 60 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
}

#[test]
fn a_line_claim_served_at_once_by_the_claimers_own_ioapic_returns_its_result() {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.remapping = false;
    let mut m = S::on(&machine);
    let role = m
        .sys
        .node(0)
        .ioapic_role(5)
        .expect("an IOAPIC serves GSI 5");
    let holder = m.sys.role(role).holder();
    let line = dodos_abi::IrqSource::line(5, false, false).raw();
    let outcome = m.call(holder, claim_on(u64::from(holder), line, 48, 210));
    assert_eq!(
        outcome,
        Ok(Output::Msi {
            address: 0,
            data: 0
        })
    );
    assert!(m.sys.node(holder).unit_requests.iter().all(Option::is_none));
}

// What the holder's gate checks of a unit request's operands (the ninth
// model review, finding 6 and its mutations M26 and M31; Devices and shared
// hardware, the operands of each operation, G9).

use super::pass17::{hold_identifier, holder_requester_third, iospace_root, judged};
use crate::rings::{Target, UnitOp};

#[test]
fn the_holder_refuses_an_attach_whose_root_is_not_an_iospace_table_of_the_requester() {
    let (mut m, holder, requester, third) = holder_requester_third();
    hold_identifier(&mut m, requester, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let attach = |root| UnitOp::Attach {
        device: 0x10,
        domain: own,
        root,
    };
    // No root; a page of the requester's floor that is not an iospace
    // root; the root of another node, pinned for another device.
    let data = m.sys.canonical().floors[requester as usize][0].0 + 4 * 4096;
    let foreign = iospace_root(&mut m, third, 0x11);
    assert!(judged(&mut m, holder, requester, 0, attach(0)));
    assert!(judged(&mut m, holder, requester, 0, attach(data)));
    assert!(judged(&mut m, holder, requester, 0, attach(foreign)));
    let root = iospace_root(&mut m, requester, 0x10);
    assert!(!judged(&mut m, holder, requester, 0, attach(root)));
}

#[test]
fn the_holder_refuses_a_detach_of_the_requesters_device_under_another_domain_number() {
    let (mut m, holder, requester, _) = holder_requester_third();
    hold_identifier(&mut m, requester, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let root = iospace_root(&mut m, requester, 0x10);
    let attach = UnitOp::Attach {
        device: 0x10,
        domain: own,
        root,
    };
    assert!(!judged(&mut m, holder, requester, 0, attach));
    let detach = |domain| UnitOp::Detach {
        device: 0x10,
        domain,
        root,
    };
    assert!(judged(&mut m, holder, requester, 0, detach(own + 1)));
    let unit = &m.sys.node(holder).table.units[&0];
    assert!(unit.entries.contains_key(&0x10));
    assert!(!judged(&mut m, holder, requester, 0, detach(own)));
}

#[test]
fn the_holder_refuses_a_masked_redirection_entry_whose_second_operand_is_not_0() {
    let (mut m, _, requester, third) = holder_requester_third();
    let role = m.sys.node(0).ioapic_role(5).unwrap();
    let holder = m.sys.role(role).holder();
    let requester = if holder == requester {
        third
    } else {
        requester
    };
    let masked = |target| UnitOp::Redirection {
        gsi: 5,
        claimed: false,
        level: true,
        low: false,
        target,
    };
    let remapped = Target::Remapped {
        handle: super::pass17::handle(requester, 48),
        vector: 50,
    };
    assert!(judged(&mut m, holder, requester, role, masked(remapped)));
    assert!(!judged(
        &mut m,
        holder,
        requester,
        role,
        masked(Target::Masked)
    ));
}

#[test]
fn the_holder_refuses_an_interrupt_entry_cache_invalidation_of_a_handle_of_no_core() {
    let (mut m, holder, requester, _) = holder_requester_third();
    // Handle 65,000 names core 338, which this machine of 3 cores lacks: a
    // faulty requester's operand is refused, never a panic.
    let op = UnitOp::InterruptCache { handle: 65_000 };
    assert!(judged(&mut m, holder, requester, 0, op));
}

// The owner word of a line (Interrupts, D-135).

/// Node `node` claims line `gsi` on its own core (node n runs on core n)
/// into slot `slot`, and its claim completes.
pub(super) fn claim_line(m: &mut S, node: u16, gsi: u32, slot: u32) -> crate::Outcome {
    let line = dodos_abi::IrqSource::line(gsi, false, false).raw();
    let outcome = m.call(node, claim_on(u64::from(node), line, 48, slot));
    match outcome {
        WAITS => {
            m.run(2 * MS);
            m.woken(node).cloned().expect("the claim ended")
        }
        other => other,
    }
}

#[test]
fn a_node_cannot_claim_a_line_that_another_live_node_holds() {
    let (mut m, _, _, _) = holder_requester_third();
    let role = m.sys.node(0).ioapic_role(5).unwrap();
    let holder = m.sys.role(role).holder();
    let others: Vec<u16> = (0..3).filter(|&n| n != holder).collect();
    let (a, b) = (others[0], others[1]);
    assert!(matches!(
        claim_line(&mut m, a, 5, 210),
        Ok(Output::Msi { .. })
    ));
    // A second claim of the same line by the same node is refused too.
    assert_eq!(claim_line(&mut m, a, 5, 211), Err(Error::Busy));
    let taken =
        dodos_abi::Ownership::taken(dodos_abi::Owner::Node(a), m.sys.member(a).generation());
    assert_eq!(m.sys.canonical().line_word(5), taken);
    // Node b's own irq_control and core: refused by the line's word.
    assert_eq!(claim_line(&mut m, b, 5, 210), Err(Error::Busy));
    // A forged redirection entry from b, aimed at b's core, is refused by
    // the IOAPIC's holder.
    {
        let forged = UnitOp::Redirection {
            gsi: 5,
            claimed: true,
            level: false,
            low: false,
            target: Target::Remapped {
                handle: super::pass17::handle(b, 48),
                vector: 48,
            },
        };
        assert!(judged(&mut m, holder, b, role, forged));
    }
    // The irq removed, its word is free and b may claim the line. The
    // removal asks the IOAPIC's holder to mask the line first: a request,
    // the line word still a's until it is answered (D-137).
    assert_eq!(m.call(a, Syscall::CapDelete(s(210))), Err(Error::Again));
    assert_eq!(m.sys.canonical().line_word(5), taken);
    assert_eq!(claim_line(&mut m, b, 5, 212), Err(Error::Busy));
    // Then the invalidation of the line's remapping entry in the units of
    // the IOAPIC's segment (pass 21): the word stays a's until its answer.
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        m.settle();
        outcome = m.call(a, Syscall::CapDelete(s(210)));
        if outcome != Err(Error::Again) {
            break;
        }
        assert_eq!(m.sys.canonical().line_word(5), taken);
    }
    assert_eq!(outcome, DONE);
    let free = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed);
    assert_eq!(m.sys.canonical().line_word(5), free);
    assert!(matches!(
        claim_line(&mut m, b, 5, 210),
        Ok(Output::Msi { .. })
    ));
}

#[test]
fn r6_frees_the_line_words_of_a_generation_that_is_over() {
    let (mut m, holder, a, b) = holder_requester_third();
    assert!(matches!(
        claim_line(&mut m, a, 5, 210),
        Ok(Output::Msi { .. })
    ));
    let killer = if holder != a { holder } else { b };
    m.ok(killer, kill(a));
    m.run(20 * MS);
    assert_eq!(m.state(a), MemberState::Dead);
    assert!(!m.sys.canonical().lines.contains_key(&5));
    assert!(matches!(
        claim_line(&mut m, b, 5, 210),
        Ok(Output::Msi { .. })
    ));
}

#[test]
fn a_line_beyond_the_last_line_word_is_unsupported() {
    let mut machine = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    machine.ioapics[0].first_gsi = dodos_abi::IRQ_LINES_MAX as u32 - 4;
    let mut m = S::on(&machine);
    let beyond = dodos_abi::IRQ_LINES_MAX as u32 + 2;
    let line = dodos_abi::IrqSource::line(beyond, false, false).raw();
    assert_eq!(
        m.call(0, claim_on(0, line, 48, 210)),
        Err(Error::Unsupported)
    );
}

// A resume while a peer is unfenced, or died and started again (the ninth
// model review, finding 9; `node_start` mode 1: R0, R2 and R3 for every node
// whose generation ended while it was sealed, R4 and R5 for those still
// fenced or unfenced).

/// Node 2's next fence cannot be confirmed: its core is in VMX root
/// operation, which blocks INIT, and its kernel cannot run the NMI handler.
fn unfence(m: &mut S, killer: u16) {
    m.sys.set_vmx(2, true);
    m.sys.hang(2, false);
    m.ok(killer, kill(2));
    m.run(dodos_abi::FENCE_NS + 10 * MS);
    assert_eq!(m.state(2), MemberState::Unfenced);
}

#[test]
fn a_node_resumes_while_a_peer_is_unfenced() {
    use super::pass17::{resume, seal};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    unfence(&mut m, 0);
    m.ok(0, resume(1));
    m.run(50 * MS);
    assert_eq!(m.state(1), MemberState::Live);
}

#[test]
fn a_resume_keeps_the_reclaim_of_a_later_generation_of_a_peer_that_died_while_sealed() {
    use super::pass17::{resume, seal};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.ok(0, seal(1));
    m.run(10 * MS);
    m.ok(0, kill(2));
    m.run(20 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    m.ok(0, fresh(2, 0b100));
    m.run(20 * MS);
    assert_eq!(m.state(2), MemberState::Live);
    let later = m.sys.member(2).generation();
    unfence(&mut m, 0);
    m.ok(0, resume(1));
    // At the step that makes node 1 live, its reclaim of node 2 is the
    // reclaim of the later generation, not one begun again for the
    // generation that ended while it was sealed.
    for _ in 0..10_000 {
        if m.state(1) == MemberState::Live {
            break;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    }
    assert_eq!(m.state(1), MemberState::Live);
    let reclaim = m.sys.node(1).reclaims.get(&2).map(|r| r.generation);
    assert_eq!(reclaim, Some(later));
}

// The checker's G4 and G6 judged on their own (the ninth model review,
// finding 11).

#[test]
fn a_map_count_above_the_leaves_of_a_floor_chunk_is_reported_by_g4() {
    let mut m = S::two();
    let frame = m.retype(0, ObjectKind::Frame, 1, 150);
    m.ok(0, frame);
    let object = m.sys.node(0).object_at(m.t[0], s(150)).unwrap();
    let chunk = m.sys.node(0).frames[&object].base >> m.sys.node(0).geometry().chunk_shift();
    // A count above the leaves (none) of a chunk of node 0's floor: the
    // node's own check allows it on a machine of several nodes, G4 does not.
    m.sys.edit_table(|t| {
        std::sync::Arc::make_mut(&mut t.gate_words)[chunk as usize] += 1;
    });
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(report, "G4: a map count is not the leaves of its chunk");
}

#[test]
fn a_grant_freed_while_an_unfenced_holder_translates_to_it_is_reported_by_g6() {
    let mut m = S::two();
    super::pass17::granted_to_1(&mut m);
    let map = Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(110),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(1, map);
    // Node 1's fence cannot be confirmed: it is unfenced, still holds its
    // translation, and its own check no longer runs, so G6 alone sees a
    // grant freed under it.
    m.sys.set_vmx(1, true);
    m.sys.hang(1, false);
    m.ok(0, kill(1));
    m.run(dodos_abi::FENCE_NS + 10 * MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    m.sys.edit_table(|t| t.grants[0].clear());
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(
        report,
        "G6: a grant freed while its holder translates to it"
    );
}

#[test]
fn a_core_that_two_nodes_run_is_reported_by_p2_even_when_the_first_is_named() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    // Node 1's kernel runs on core 0 too, which names node 0: the first
    // runner found matches the word and hid the second.
    m.sys.node_mut(1).cores[0].number = 0;
    let report = m.sys.check().expect_err("spoiled");
    assert!(
        report.ends_with("P2: a core executes a node its word does not name"),
        "{report}"
    );
}

// The mutation M5 of the ninth model review: an IOAPIC on another PCI
// segment (Devices and shared hardware, several remapping units: "the
// IOAPIC of a line, whose segment the DMAR gives").

#[test]
fn the_claim_of_a_remapped_line_asks_the_units_of_its_ioapics_segment_only() {
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    let mut other = machine.iommus[0];
    other.segment = 1;
    machine.iommus.push(other);
    machine.ioapics[0].segment = 1;
    let mut m = S::on(&machine);
    let asked = |m: &S, unit: usize| {
        let role = m.sys.node(0).segment_roles(machine.iommus[unit].segment)[0];
        super::pass17::submissions(m, role)
    };
    let before = (asked(&m, 0), asked(&m, 1));
    let claimer = (0..3)
        .find(|&n| n != m.sys.role(m.sys.node(0).ioapic_role(5).unwrap()).holder())
        .unwrap();
    assert!(matches!(
        claim_line(&mut m, claimer, 5, 210),
        Ok(Output::Msi { .. })
    ));
    assert_eq!((asked(&m, 0), asked(&m, 1)), (before.0, before.1 + 1));
}

// The retry of a start and the sweep at due events of the first core
// (D-135: Node slots and core words, the start request; Detection, the
// sweep), and the `Ping` removed.

#[test]
fn a_start_wake_is_sent_again_every_start_retry_ns_until_the_core_takes_it() {
    let mut m = S::two();
    m.ok(0, kill(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // The core never takes its wake.
    m.sys.hang(1, true);
    m.ok(0, fresh(1, 0b10));
    let from = m.sys.now();
    // Time passes from one armed instant to the next, not in steps.
    m.sys.advance(from + 10 * MS);
    assert_eq!(m.sys.check(), Ok(()));
    let wakes: Vec<u64> = m
        .sys
        .sent
        .iter()
        .filter(|(at, node, a)| *at >= from && *node == 0 && *a == HwAction::Start { core: 1 })
        .map(|(at, _, _)| *at)
        .collect();
    assert!(wakes.len() >= 9, "{wakes:?}");
    assert!(
        wakes
            .windows(2)
            .all(|w| w[1] - w[0] == dodos_abi::START_RETRY_NS),
        "{wakes:?}"
    );
}

#[test]
fn the_first_core_of_an_idle_live_node_keeps_its_sweep_armed() {
    let mut m = S::two();
    // Past the instants of the first sweep's observations.
    m.run(600 * MS);
    // Neither node has work: each first core is idle with its sweep armed,
    // never with nothing armed.
    for core in 0..2 {
        let idle_until = m.sys.canonical().core_beacons[core].idle_until.0;
        assert_ne!(idle_until, u64::MAX, "core {core}");
        assert!(idle_until <= m.sys.now() + m.sys.canonical().sweep_ns);
    }
}

#[test]
fn a_kind_9_message_is_one_the_receiver_does_not_know() {
    assert_eq!(dodos_abi::NodeMessage::from_raw(9), None);
    let mut m = S::two();
    let header = dodos_abi::RingHeader {
        kind: 9,
        length: 1,
        sender: 0,
        sender_generation: m.sys.member(0).generation(),
        receiver_generation: m.sys.member(1).generation(),
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    let before = m.sys.node(1).refused.0;
    m.sys.inject_raw(0, 1, false, header, 1);
    m.run(20 * MS);
    assert_eq!(m.sys.node(1).refused.0, before + 1);
}

// The state word of an invalidation request (A node of several cores,
// D-135: the completing core writes *done*, its own core starts the next).

#[test]
fn the_core_that_completes_a_request_writes_its_state_word_done() {
    use super::cores::{FRAME, two_threads};
    use crate::cores::RequestState;
    let (mut k, _) = two_threads(Arch::X86_64, 2);
    assert_eq!(k.call(k.t, Syscall::ASpaceUnmap(s(FRAME))), WAITS);
    assert_eq!(k.m.cores[0].request_state, RequestState::InUse);
    // Core 1's acknowledgement completes core 0's request.
    k.m.take_interrupts(1);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.cores[0].request_state, RequestState::Done);
    assert_eq!(k.m.cores[0].request, None);
}

#[test]
fn a_line_word_taken_with_no_irq_of_its_node_is_reported() {
    let mut m = S::two();
    super::pass17::hold_line(&mut m, 0, 5);
    let report = m.sys.check().expect_err("spoiled");
    assert_eq!(report, "node 0: 96: line words and claimed lines");
}
