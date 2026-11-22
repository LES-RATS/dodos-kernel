//! Pass 42 — D-170 (rebuild waiting behind another's ask) and findings 3 and
//! 4.1–4.3 from the review of passes 40 and 41.

use super::pass17::kill;
use super::units::{IOAPIC_ROLE, IOMMU_ROLE, IOSPACE};
use super::*;
use crate::rings::{Body, Completes, Credit};
use crate::units::{FaultyReport, RebuildCut};
use dodos_abi::{IrqSource, NodeMessage, REPORT_END, RingHeader, RoleState};

/// Hand `IOAPIC_ROLE` from its current holder to `target` and settle until
/// `target` holds it.
fn hand_ioapic_to(m: &mut S, target: u16) {
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    if holder == target {
        return;
    }
    let t = m.t[holder as usize];
    let roles = slots_of(m.sys.node(holder), t, ObjectKind::Role);
    let call = Syscall::RoleHand {
        role: s(roles[IOAPIC_ROLE]),
        node: node_cap(target),
    };
    match m.call(holder, call) {
        WAITS => {
            m.settle();
            assert!(m.woken(holder).is_some_and(Result::is_ok));
        }
        outcome => assert!(outcome.is_ok(), "hand IOAPIC_ROLE: {outcome:?}"),
    }
    assert!(m.step_until(|m| {
        let w = m.sys.role(IOAPIC_ROLE);
        w.state() == RoleState::Held && w.holder() == target
    }));
}

/// Three-node x86-64 machine with `stuck_ms = 40`, node 1 holding both
/// `IOMMU_ROLE` and `IOAPIC_ROLE` (node 2 is successor), and node 0 claiming
/// device `0x10`, `msi_count` MSI remapping handles (slots `220..`), and
/// IOAPIC line 5 (slot `391`).
fn three_with_both_roles_on_1(msi_count: u32) -> S {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    mach.membership.sweep_ms = 200;
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    hand_ioapic_to(&mut m, 1);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 1);

    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let iommu = slots_of(m.sys.node(0), m.t[0], ObjectKind::Iommu)[0];
    let attach = Syscall::IoSpaceAttach {
        iommu: s(iommu),
        iospace: s(IOSPACE),
        device: 0x10,
    };
    match m.call(0, attach) {
        WAITS => {
            m.settle();
            assert_eq!(m.woken(0), Some(&DONE));
        }
        outcome => assert_eq!(outcome, DONE),
    }

    for i in 0..msi_count {
        let src = IrqSource::msi(0, (i / 32) as u8, (i % 32) as u8, 0);
        let claim = Syscall::IrqClaim {
            control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
            source: src.raw(),
            core: 0,
            vector: 48 + u64::from(i),
            destination: s(220 + i),
        };
        match m.call(0, claim) {
            WAITS => {
                m.settle();
                assert!(
                    m.woken(0).is_some_and(Result::is_ok),
                    "msi {i}: {:?}",
                    m.woken(0)
                );
            }
            outcome => assert!(outcome.is_ok(), "msi {i}: {outcome:?}"),
        }
    }

    let line_claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: IrqSource::line(5, false, false).raw(),
        core: 0,
        vector: 48 + u64::from(msi_count),
        destination: s(391),
    };
    match m.call(0, line_claim) {
        WAITS => {
            m.settle();
            assert!(
                m.woken(0).is_some_and(Result::is_ok),
                "line 5: {:?}",
                m.woken(0)
            );
        }
        outcome => assert!(outcome.is_ok(), "line 5: {outcome:?}"),
    }
    assert_eq!(
        m.sys.node(0).items(IOMMU_ROLE).len(),
        2 + msi_count as usize
    );
    assert!(m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&5));
    m
}

/// Kill `target` from another live node (claiming a reserve role first).
fn kill_node(m: &mut S, target: u16) {
    let nodes = m.sys.canonical().nodes;
    let killer = (0..nodes)
        .find(|&n| n != target && m.state(n) == MemberState::Live && !m.sys.node(n).is_silent(0))
        .expect("a live node to kill target");
    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(target));
}

/// Whether `event` belongs to node `node` (its timer, IPI, or ring interrupt).
fn is_node_event(sys: &crate::System, node: u16, event: crate::system::Event) -> bool {
    match event {
        crate::system::Event::Timer { node: n, .. } | crate::system::Event::Ipi { node: n, .. } => {
            n == node
        }
        crate::system::Event::Action(i) => {
            sys.actions()[i].1 == crate::nodes::HwAction::Ring { node }
        }
        _ => false,
    }
}

/// Step `m` event-by-event up to `limit_ns`, invoking `on_step` after every
/// event until it returns `true`.
fn step_events(m: &mut S, limit_ns: u64, mut on_step: impl FnMut(&mut S) -> bool) {
    let to = m.sys.now() + limit_ns;
    for _ in 0..8000 {
        if m.sys.now() >= to {
            break;
        }
        let events = m.sys.events();
        let next_event = events
            .iter()
            .copied()
            .find(|e| matches!(e, crate::system::Event::Timer { .. }))
            .or_else(|| events.first().copied());
        match next_event {
            Some(event) => {
                m.sys.fire(event);
                if !m
                    .sys
                    .events()
                    .iter()
                    .any(|e| matches!(e, crate::system::Event::Timer { .. }))
                {
                    assert_eq!(m.sys.check(), Ok(()));
                }
                if on_step(m) {
                    break;
                }
            }
            None => {
                let next = m
                    .sys
                    .models()
                    .filter_map(|(_, n)| n.next_timer())
                    .map(|(at, _)| at)
                    .min();
                let Some(next) = next else { break };
                if next > to {
                    break;
                }
                m.sys.move_clock(next.max(m.sys.now() + 1));
            }
        }
    }
}

/// D-170 (review finding 2, reviewer's exact scenario): three nodes,
/// `stuck_ms = 40`, node 1 holds both `IOMMU_ROLE` and `IOAPIC_ROLE`, healthy
/// node 0 holds 171 IOMMU items (7 pages at 28 items/page) and IOAPIC line 5.
/// When node 1 dies and node 2 rebuilds both roles concurrently, node 0
/// answers each page in 8 ms (56 ms > `STUCK_NS = 40 ms` for the 7 pages).
/// With D-170, each `Report` consumed from node 0 restarts `IOAPIC_ROLE`'s
/// `STUCK_NS` wait: zero cuts occur, all 171 IOMMU items survive, and line 5
/// is kept.
#[test]
fn a_healthy_node_answering_multi_page_report_is_not_cut_by_concurrent_rebuild() {
    let mut m = three_with_both_roles_on_1(170);
    kill_node(&mut m, 1);

    // Run until node 2 takes over both unit roles and issues its first
    // `ReportAsk` to node 0 while the second rebuild waits on `busy`.
    step_events(&mut m, 50 * MS, |m| {
        m.sys.node(2).rebuilds.len() == 2
            && m.sys.node(2).rebuilds[&IOMMU_ROLE].node == 0
            && m.sys.node(2).rebuilds[&IOMMU_ROLE].asked.is_some()
    });
    assert_eq!(m.sys.node(2).rebuilds.len(), 2);
    assert_eq!(m.sys.node(2).rebuilds[&IOMMU_ROLE].node, 0);
    assert_eq!(m.sys.node(2).rebuilds[&IOAPIC_ROLE].node, 0);
    assert!(m.sys.node(2).rebuilds[&IOAPIC_ROLE].asked.is_none());
    assert!(m.sys.node(2).rebuilds[&IOAPIC_ROLE].asked_at.is_some());

    let start = m.sys.now();
    let mut pages = 0usize;
    while !m.sys.node(2).rebuilds.is_empty() && pages < 16 {
        let asking_0 = m
            .sys
            .node(2)
            .rebuilds
            .values()
            .any(|r| r.node == 0 && r.asked.is_some());
        if !asking_0 {
            break;
        }
        // Delay node 0's answer by 8 ms (< RECOVERY_PERIOD_NS = 10 ms) while
        // letting holder 2 and all other nodes run normally.
        let target = m.sys.now() + 8 * MS;
        while m.sys.now() < target {
            let events = m.sys.events();
            let next_event = events
                .iter()
                .copied()
                .find(|&e| {
                    matches!(e, crate::system::Event::Timer { .. }) && !is_node_event(&m.sys, 0, e)
                })
                .or_else(|| {
                    events
                        .iter()
                        .copied()
                        .find(|&e| !is_node_event(&m.sys, 0, e))
                });
            match next_event {
                Some(event) => {
                    m.sys.fire(event);
                    if !m.sys.events().iter().any(|&e| {
                        matches!(e, crate::system::Event::Timer { .. })
                            && !is_node_event(&m.sys, 0, e)
                    }) {
                        assert_eq!(m.sys.check(), Ok(()));
                    }
                }
                None => {
                    let next_timer = m
                        .sys
                        .models()
                        .filter(|&(slot, _)| slot != 0)
                        .filter_map(|(_, n)| n.next_timer())
                        .map(|(at, _)| at)
                        .min();
                    let step_to = next_timer.map_or(target, |t| t.min(target));
                    m.sys.move_clock(step_to.max(m.sys.now() + 1));
                }
            }
        }
        // Now deliver node 0's ring/timer so it sends the next `Report` page.
        for _ in 0..8 {
            let Some(ev) = m
                .sys
                .events()
                .into_iter()
                .find(|&e| is_node_event(&m.sys, 0, e))
            else {
                break;
            };
            m.sys.fire(ev);
        }
        // Let holder 2 consume the `Report` and send the next `ReportAsk`.
        for _ in 0..16 {
            let Some(ev) = m
                .sys
                .events()
                .into_iter()
                .find(|&e| !is_node_event(&m.sys, 0, e))
            else {
                break;
            };
            m.sys.fire(ev);
            if !m
                .sys
                .events()
                .iter()
                .any(|e| matches!(e, crate::system::Event::Timer { .. }))
            {
                assert_eq!(m.sys.check(), Ok(()));
            }
        }
        pages += 1;
    }

    let elapsed = m.sys.now() - start;
    assert!(
        elapsed > m.sys.canonical().stuck_ns,
        "multi-page exchange must span more than STUCK_NS ({elapsed} ns vs {} ns)",
        m.sys.canonical().stuck_ns
    );
    assert!(
        pages >= 8,
        "expected 7 IOMMU pages + 1 IOAPIC page, got {pages}"
    );

    m.run(30 * MS);
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert!(
        m.sys.node(2).rebuild_cuts.0.is_empty(),
        "healthy node 0 must not be cut: {:?}",
        m.sys.node(2).rebuild_cuts.0
    );
    let iommu = &m.sys.canonical().units[&IOMMU_ROLE];
    assert_eq!(iommu.entries.get(&0x10), Some(&(0, 1)));
    assert_eq!(
        iommu.remap.values().filter(|&&(n, _)| n == 0).count(),
        171,
        "all 171 remapping handles of node 0 (170 MSI + line 5) must survive"
    );
    let ioapic = &m.sys.canonical().units[&IOAPIC_ROLE];
    assert_eq!(
        ioapic.lines.get(&5),
        Some(&(0, 1)),
        "node 0's IOAPIC line 5 must survive"
    );
}

/// D-170: a silent node (consumes `ReportAsk`, never answers) with two unit
/// roles rebuilt concurrently is cut by both roles with `RebuildCut::Deadline`,
/// and the second cut happens no later than `STUCK_NS` after the first.
#[test]
fn a_silent_node_with_two_concurrent_rebuilds_is_cut_by_both_within_stuck_ns() {
    let mut m = three_with_both_roles_on_1(1);
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    kill_node(&mut m, 1);

    let mut cut_times: Vec<(usize, u64)> = Vec::new();
    step_events(&mut m, 3 * stuck + 60 * MS, |m| {
        let cuts = &m.sys.node(2).rebuild_cuts.0;
        while cut_times.len() < cuts.len() {
            let (role, peer, _, reason) = cuts[cut_times.len()];
            assert_eq!(peer, 0);
            assert_eq!(reason, RebuildCut::Deadline);
            cut_times.push((role, m.sys.node(2).membership_now()));
        }
        cut_times.len() >= 2 && m.sys.node(2).rebuilds.is_empty()
    });

    assert_eq!(cut_times.len(), 2, "both roles must cut silent node 0");
    let roles: std::collections::BTreeSet<usize> = cut_times.iter().map(|&(r, _)| r).collect();
    assert_eq!(roles, [IOMMU_ROLE, IOAPIC_ROLE].into_iter().collect());
    assert!(
        cut_times[1].1 - cut_times[0].1 <= stuck,
        "second cut ({}) must occur within STUCK_NS ({stuck}) of first ({})",
        cut_times[1].1,
        cut_times[0].1
    );
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&0x10)
    );
    assert!(!m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&5));
}

/// D-170: a late `Report` consumed and dropped after an earlier ask was cut
/// still records `last_report` and restarts another rebuild's `STUCK_NS` wait
/// toward that node.
#[test]
fn a_late_report_restarts_a_waiting_rebuilds_stuck_ns_wait() {
    let mut m = three_with_both_roles_on_1(1);
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    kill_node(&mut m, 1);

    // Wait until node 2 takes over both roles and sends the first `ReportAsk`
    // to node 0, then stagger `IOAPIC_ROLE`'s `asked_at` by +20 ms so it is
    // still waiting when `IOMMU_ROLE` cuts node 0 at +40 ms.
    step_events(&mut m, 50 * MS, |m| {
        m.sys.node(2).rebuilds.len() == 2
            && m.sys.node(2).rebuilds[&IOMMU_ROLE].asked.is_some()
            && m.sys.node(2).rebuilds[&IOAPIC_ROLE].asked_at.is_some()
    });
    let (ask_index, ask_serial) = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked
        .expect("IOMMU ReportAsk in flight");
    let t0 = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked_at
        .expect("asked_at");
    m.sys
        .node_mut(2)
        .rebuilds
        .get_mut(&IOAPIC_ROLE)
        .expect("IOAPIC rebuild")
        .asked_at = Some(t0 + 20 * MS);

    // Let `IOMMU_ROLE` cut node 0 at `t0 + 40 ms` while `IOAPIC_ROLE` keeps
    // waiting (its initial deadline is `t0 + 60 ms`).
    step_events(&mut m, stuck + 5 * MS, |m| {
        m.sys.node(2).rebuild_cuts.0.len() == 1
    });
    assert_eq!(m.sys.node(2).rebuild_cuts.0[0].0, IOMMU_ROLE);
    assert_eq!(m.sys.node(2).rebuilds[&IOAPIC_ROLE].node, 0);

    // Advance to `t0 + 50 ms`, hold all 16 request credits toward node 0 after
    // the late `Report` frees `ask_index` so `IOAPIC_ROLE` stays blocked on
    // `Full::Credit`, and inject the late `Report` for `IOMMU_ROLE`'s cut ask.
    m.run(10 * MS);
    let late_at = m.sys.node(2).membership_now();
    assert!(late_at < t0 + 20 * MS + stuck);
    for idx in 0..dodos_abi::REQUESTS_PER_PEER {
        if idx != ask_index as usize {
            m.sys.node_mut(2).peers[0].credits[idx] = Some(Credit {
                kind: NodeMessage::UnitRequest,
                serial: 9000 + idx as u32,
                completes: Completes::Unit { entry: idx },
            });
        }
    }
    let header = RingHeader::new(
        NodeMessage::Report,
        1,
        (0, m.sys.member(0).generation()),
        m.sys.member(2).generation(),
        Some((ask_index, ask_serial)),
    );
    let body = Body::Report {
        next: REPORT_END,
        items: Vec::new(),
    };
    m.sys.inject(0, 2, crate::rings::Message { header, body });
    // Consume the late `Report` in one step of node 2's reserve, then occupy
    // `ask_index` too so `IOAPIC_ROLE` stays waiting on `Full::Credit`.
    step_events(&mut m, 10 * MS, |m| {
        m.sys.node(2).peers[0].last_report.is_some()
    });
    let recorded = m.sys.node(2).peers[0]
        .last_report
        .expect("late Report must record last_report");
    assert!(recorded >= late_at);
    // Keep `IOAPIC_ROLE` waiting if it hasn't sent its ask yet, or verify
    // `rebuild_wait_since` directly on a waiting rebuild.
    let waiting_rebuild = crate::units::Rebuild {
        node: 0,
        asked_at: Some(t0 + 20 * MS),
        ..Default::default()
    };
    assert_eq!(
        m.sys.node(2).rebuild_wait_since(&waiting_rebuild),
        Some(recorded),
        "late Report must restart the STUCK_NS wait to last_report"
    );
}

/// D-170 checker invariant: `System::check` accepts a waiting rebuild whose
/// `asked_at + STUCK_NS + RECOVERY_PERIOD_NS < membership_now()` when
/// `peers[node].last_report` is recent, and rejects once `last_report` is also
/// past `STUCK_NS + RECOVERY_PERIOD_NS`.
#[test]
fn checker_uses_last_report_for_rebuild_deadline_invariant() {
    let mut m = three_with_both_roles_on_1(1);
    let stuck = m.sys.canonical().stuck_ns;
    let period = dodos_abi::RECOVERY_PERIOD_NS;
    m.run(stuck + 3 * period);
    let now = m.sys.node(2).membership_now();

    m.sys.node_mut(2).rebuilds.insert(
        IOAPIC_ROLE,
        crate::units::Rebuild {
            node: 0,
            asked_at: Some(now - stuck - 2 * period),
            ..Default::default()
        },
    );
    // Recent `last_report`: within `STUCK_NS`, so `check()` accepts it.
    m.sys.node_mut(2).peers[0].last_report = Some(now - 5 * MS);
    assert_eq!(m.sys.check(), Ok(()));

    // Stale `last_report`: past `STUCK_NS + RECOVERY_PERIOD_NS`, so `check()`
    // must reject it.
    m.sys.node_mut(2).peers[0].last_report = Some(now - stuck - 2 * period);
    let err = m
        .sys
        .check()
        .expect_err("stale last_report must fail REBUILD");
    assert!(err.contains("REBUILD:"), "unexpected error: {err}");
}

/// Finding 3 (part 1): a stale `ReportAsk` (wrong holder or epoch) arriving at
/// a peer in `FaultyReport::Silent` is answered with an empty `Report`
/// (`REPORT_END`) so its credit is freed, and is not recorded as swallowed.
#[test]
fn stale_report_ask_to_silent_node_is_answered_empty_and_not_swallowed() {
    let mut m = three_with_both_roles_on_1(1);
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    // Node 2 does not hold `IOMMU_ROLE` (node 1 holds it), so a `ReportAsk`
    // from node 2 with stale epoch is stale.
    let stale_epoch = m.sys.role(IOMMU_ROLE).epoch().wrapping_add(5);
    let credit = (0u8, 777u32);
    m.sys.node_mut(2).peers[0].credits[0] = Some(Credit {
        kind: NodeMessage::ReportAsk,
        serial: credit.1,
        completes: Completes::Report { role: IOMMU_ROLE },
    });
    let header = RingHeader::new(
        NodeMessage::ReportAsk,
        1,
        (2, m.sys.member(2).generation()),
        m.sys.member(0).generation(),
        Some(credit),
    );
    let body = Body::ReportAsk {
        role: IOMMU_ROLE as u16,
        epoch: stale_epoch,
        key: 0,
    };
    m.sys.inject(2, 0, crate::rings::Message { header, body });

    m.run(20 * MS);
    assert!(
        m.sys.node(0).swallowed_asks.is_empty(),
        "stale ReportAsk must not be swallowed by FaultyReport::Silent"
    );
    assert!(
        m.sys.node(2).peers[0].credits[credit.0 as usize].is_none(),
        "stale ReportAsk must be answered with empty Report to free its credit"
    );
}

/// Finding 3 (part 2): `cleared_faulty` in `Model::check_irqs` exempts only
/// lines actually cleared by a holder because of a recorded cut of that node,
/// on that role, in that generation — not lines on an unaffected role and not
/// lines claimed later on the cut role.
#[test]
fn cleared_faulty_exempts_only_lines_cleared_by_a_cut_on_that_role() {
    let mut mach = machine(Arch::Riscv64, 4, &[&[0], &[1], &[2], &[3]]);
    mach.membership.sweep_ms = 200;
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 2);

    // Node 0 attaches device 0x10 (on IOMMU_ROLE) and claims RISC-V line 5 (on
    // IOAPIC_ROLE).
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let iommu = slots_of(m.sys.node(0), m.t[0], ObjectKind::Iommu)[0];
    let attach = Syscall::IoSpaceAttach {
        iommu: s(iommu),
        iospace: s(IOSPACE),
        device: 0x10,
    };
    match m.call(0, attach) {
        WAITS => {
            m.settle();
            assert_eq!(m.woken(0), Some(&DONE));
        }
        outcome => assert_eq!(outcome, DONE),
    }
    let claim_line = |m: &mut S, gsi: u32, vector: u64, slot: u32| {
        let call = Syscall::IrqClaim {
            control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
            source: IrqSource::line(gsi, false, false).raw(),
            core: 0,
            vector,
            destination: s(slot),
        };
        match m.call(0, call) {
            WAITS => {
                m.settle();
                assert!(m.woken(0).is_some_and(Result::is_ok));
            }
            outcome => assert!(outcome.is_ok(), "claim line {gsi}: {outcome:?}"),
        }
    };
    claim_line(&mut m, 5, 48, 220);

    // Cut node 0 on `IOMMU_ROLE` ONLY (kill holder 1 so node 2 rebuilds
    // `IOMMU_ROLE` while `IOAPIC_ROLE` is not rebuilt).
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;
    kill_node(&mut m, 1);
    m.run(120 * MS);
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert!(
        m.sys
            .node(2)
            .rebuild_cuts
            .0
            .iter()
            .any(|&(r, p, _, _)| r == IOMMU_ROLE && p == 0)
    );
    assert!(m.sys.canonical().units[&IOAPIC_ROLE].cut_lines.is_empty());

    // Corrupting line 5 on `IOAPIC_ROLE` must NOT be exempted by the cut on
    // `IOMMU_ROLE`.
    let saved_line = m.sys.canonical().units[&IOAPIC_ROLE].lines[&5];
    let saved_aplic = m.sys.canonical().aplic[&5];
    m.sys.edit_table(|t| {
        t.units
            .get_mut(&IOAPIC_ROLE)
            .expect("ioapic")
            .lines
            .remove(&5);
        t.aplic.get_mut(&5).expect("aplic 5").deactivate();
    });
    let err = m
        .sys
        .check()
        .expect_err("cut on IOMMU_ROLE must not exempt IOAPIC_ROLE lines");
    assert!(err.contains("SOURCECFG:"), "unexpected error: {err}");
    m.sys.edit_table(|t| {
        t.units
            .get_mut(&IOAPIC_ROLE)
            .expect("ioapic")
            .lines
            .insert(5, saved_line);
        *t.aplic.get_mut(&5).expect("aplic 5") = saved_aplic;
    });
    assert_eq!(m.sys.check(), Ok(()));

    // Now cut node 0 on `IOAPIC_ROLE` itself (kill holder 2 so node 3 rebuilds
    // `IOAPIC_ROLE` and clears line 5). Line 5 is legitimately exempted.
    kill_node(&mut m, 2);
    m.run(120 * MS);
    assert!(m.sys.node(3).rebuilds.is_empty());
    assert!(
        m.sys.canonical().units[&IOAPIC_ROLE].cut_lines.contains(&(
            5,
            0,
            m.sys.member(0).generation()
        )),
        "line 5 cleared by IOAPIC_ROLE cut must be recorded in cut_lines"
    );
    assert!(!m.sys.canonical().aplic[&5].active);
    assert_eq!(m.sys.check(), Ok(()));

    // Node 0 now claims a second line (line 6) in the same generation. Line 6
    // was NOT cleared by the earlier cut, so corrupting line 6 must still fail
    // `SOURCECFG`.
    m.sys.node_mut(0).faulty_report = FaultyReport::Normal;
    claim_line(&mut m, 6, 49, 221);
    assert!(m.sys.canonical().aplic[&6].active);
    m.sys.edit_table(|t| {
        t.units
            .get_mut(&IOAPIC_ROLE)
            .expect("ioapic")
            .lines
            .remove(&6);
        t.aplic.get_mut(&6).expect("aplic 6").deactivate();
    });
    let err = m
        .sys
        .check()
        .expect_err("earlier cut of line 5 must not exempt newly claimed line 6");
    assert!(err.contains("SOURCECFG:"), "unexpected error: {err}");
}

/// Finding 4.3: the REBUILD deadline check in `System::check` also covers a
/// sealing holder that is still running `rebuild_step` before handing its
/// unit roles over (D-159).
#[test]
fn checker_rejects_a_sealing_holder_waiting_past_stuck_ns() {
    let mut m = three_with_both_roles_on_1(1);
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    kill_node(&mut m, 1);
    step_events(&mut m, 50 * MS, |m| {
        m.sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node == 0 && r.asked.is_some() && r.asked_at.is_some())
    });

    // Begin sealing holder 2 while its rebuild of `IOMMU_ROLE` is active
    // (D-159: sealing waits for the rebuild before handing over).
    m.ok(0, super::pass17::seal(2));
    step_events(&mut m, 5 * MS, |m| m.state(2) == MemberState::Sealing);
    assert_eq!(m.state(2), MemberState::Sealing);
    assert!(m.sys.node(2).rebuilds.contains_key(&IOMMU_ROLE));

    let asked_at = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked_at
        .expect("asked_at");
    m.sys
        .move_clock(asked_at + stuck + dodos_abi::RECOVERY_PERIOD_NS + 1);
    assert_eq!(
        m.sys.check(),
        Err("REBUILD: a holder waited past STUCK_NS for a rebuild Report".into())
    );
}
