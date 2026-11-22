//! Pass 41 — D-169: a node that does not answer a rebuild, and items that
//! fail their check (Roles, take-over after a death and *The reports are
//! pulled*).
//!
//! Each test exercises one rule of D-169 on the machine:
//! - a silent node (consumes `ReportAsk`, never answers) is cut at `STUCK_NS`,
//!   its claims are cleared, healthy peers' claims are kept, and the rebuild
//!   ends;
//! - the deadline judges on the clock less the stopped time and is a due event
//!   of the holder;
//! - waiting with no free `ReportAsk` credit toward a node also cuts at
//!   `STUCK_NS`;
//! - a node that leaves `ReportAsk` unconsumed is accused with
//!   `Evidence::Stuck` via the holder's kernel-ring observation;
//! - a late `Report` after the deadline is dropped and frees its credit;
//! - a `Report` with one bad item among good ones (device, handle, or line)
//!   refuses the whole `Report` (trace code 11), ends that node's report
//!   without further asks, and clears its claims;
//! - a faulty peer answering with strictly rising keys and bad items is cut on
//!   the first ask, and a multi-page healthy report finishes within
//!   `ceil(items / 28) + 1` asks, each within `STUCK_NS`;
//! - the checker rejects a rebuild missing its deadline or waiting past
//!   `STUCK_NS`.

use super::pass17::kill;
use super::units::{DEVICE, IOAPIC_ROLE, IOMMU_ROLE, attached, three};
use super::*;
use crate::rings::{Body, Item, Target};
use crate::table::RingEntry;
use crate::units::{FaultyReport, RebuildCut};
use dodos_abi::{Evidence, IrqSource, NodeMessage, REPORT_END, RingHeader, TraceCode};

/// Count of trace code 11 entries recorded by `node` for `NodeMessage::Report`.
fn report_refusals(m: &S, node: u16) -> usize {
    let core = m.sys.node(node).core_number(0);
    m.sys
        .node(node)
        .trace_ring(core, false)
        .iter()
        .filter(|e| {
            e.word2 & 0xFFFF == TraceCode::Refused as u64
                && e.payload[1] == NodeMessage::Report as u64
        })
        .count()
}

/// A four-node machine with node 0 attaching `0x10`, node 3 attaching `0x11`,
/// and node 1 holding `IOMMU_ROLE` (node 2 is successor).
fn four_with_two_devices() -> S {
    let mut mach = machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]);
    mach.membership.sweep_ms = 100;
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    for (node, dev) in [(0u16, 0x10u64), (3u16, 0x11u64)] {
        let iospace = m.retype(node, ObjectKind::IoSpace, 0, super::units::IOSPACE);
        m.ok(node, iospace);
        let iommu = slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Iommu)[0];
        let call = Syscall::IoSpaceAttach {
            iommu: s(iommu),
            iospace: s(super::units::IOSPACE),
            device: dev,
        };
        match m.call(node, call) {
            WAITS => {
                m.run(30 * MS);
                assert_eq!(m.woken(node), Some(&DONE));
            }
            outcome => assert_eq!(outcome, DONE),
        }
    }
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    m
}

/// Kill `target` from a live node that holds a reserve role, or fail `target`
/// directly if no other live node holds a reserve.
fn kill_node(m: &mut S, target: u16) {
    let nodes = m.sys.canonical().nodes;
    let killer = (0..nodes).find(|&n| {
        n != target
            && m.state(n) == MemberState::Live
            && m.sys.canonical().holds_a_reserve(n)
            && !m.sys.node(n).is_silent(0)
    });
    match killer {
        Some(k) => m.ok(k, kill(target)),
        None => m.fail(target),
    }
}

/// Step `m` event-by-event up to `limit_ns`, invoking `on_step` after every
/// fired event until it returns true.
fn step_events(m: &mut S, limit_ns: u64, mut on_step: impl FnMut(&mut S) -> bool) {
    let to = m.sys.now() + limit_ns;
    for _ in 0..8000 {
        if m.sys.now() >= to {
            break;
        }
        let events = m.sys.events();
        // Fire due timers first so no throttled context is left past its
        // replenishment instant when `check()` runs (invariant 43).
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

/// D-169: a silent node (consumes `ReportAsk`, never answers) is cut at
/// `STUCK_NS` (as an exact due event of the holder), its claims are cleared,
/// healthy nodes' claims are kept, and the rebuild ends.
#[test]
fn a_silent_node_is_cut_at_stuck_ns_and_the_rebuild_ends() {
    let mut m = four_with_two_devices();
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    // Kill holder 1 so successor 2 takes over and rebuilds `IOMMU_ROLE`.
    kill_node(&mut m, 1);
    assert!(m.step_until(|m| {
        m.sys.node(2).rebuilds.get(&IOMMU_ROLE).is_some_and(|r| {
            r.node == 0 && r.asked.is_some() && !m.sys.node(0).swallowed_asks.is_empty()
        })
    }));
    let asked_at = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked_at
        .expect("asked_at recorded");
    let refusals_before = report_refusals(&m, 2);

    // Just before `STUCK_NS`, the rebuild still waits on node 0.
    let before_deadline = asked_at + stuck - MS;
    m.run(before_deadline.saturating_sub(m.sys.now()));
    assert!(
        m.sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node == 0 && r.asked.is_some()),
        "node 0 was cut before STUCK_NS"
    );

    // The deadline `asked_at + STUCK_NS` is a due event of the holder.
    assert!(
        m.sys
            .node(2)
            .reserve_due()
            .is_some_and(|due| due <= asked_at + stuck),
        "reserve_due did not schedule the rebuild deadline"
    );

    // Reach `STUCK_NS`: node 0 is cut with trace code 11, node 3 reports
    // normally, and the rebuild finishes.
    m.run(MS + 5 * MS);
    assert!(
        m.sys.node(2).rebuilds.is_empty(),
        "rebuild did not finish after STUCK_NS"
    );
    assert!(
        report_refusals(&m, 2) > refusals_before,
        "cutting silent node 0 wrote no trace code 11"
    );
    assert!(
        m.sys
            .node(2)
            .rebuild_cuts
            .0
            .iter()
            .any(|&(role, peer, _, reason)| {
                role == IOMMU_ROLE && peer == 0 && reason == RebuildCut::Deadline
            })
    );
    // Silent node 0's context entry is cleared; healthy node 3's is kept.
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(
        !entries.contains_key(&0x10),
        "silent node 0's context entry was not cleared"
    );
    assert!(
        entries.contains_key(&0x11),
        "healthy node 3's context entry was lost"
    );
    // Healthy node 3 was never cut.
    assert!(
        !m.sys
            .node(2)
            .rebuild_cuts
            .0
            .iter()
            .any(|&(_, peer, _, _)| peer == 3)
    );
}

/// D-169: the rebuild deadline judges on the clock less the stopped time, as
/// every bound of membership.
#[test]
fn the_rebuild_deadline_judges_on_the_clock_less_the_stopped_time() {
    let mut m = four_with_two_devices();
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    kill_node(&mut m, 1);
    assert!(m.step_until(|m| {
        m.sys.node(2).rebuilds.get(&IOMMU_ROLE).is_some_and(|r| {
            r.node == 0 && r.asked.is_some() && !m.sys.node(0).swallowed_asks.is_empty()
        })
    }));
    let asked_at = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked_at
        .expect("asked_at recorded");

    // Advance half of `STUCK_NS`, then stop the machine in the debugger stub
    // for a full `STUCK_NS`, then run a few ms so node 2's reserve runs.
    let half = asked_at + stuck / 2;
    m.run(half.saturating_sub(m.sys.now()));
    m.sys.stub_stop(stuck);
    assert_eq!(m.sys.check(), Ok(()));
    m.run(5 * MS);

    // Wall clock is now past `asked_at + stuck`, but membership time is still
    // `asked_at + stuck / 2 + 5 ms < asked_at + stuck`: node 0 must NOT be cut yet.
    assert!(m.sys.now() > asked_at + stuck);
    assert!(
        m.sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node == 0 && r.asked.is_some()),
        "node 0 was cut during stopped time"
    );

    // Advance the remaining membership time to `STUCK_NS`: now cut.
    m.run(stuck / 2 + 5 * MS);
    assert!(
        m.sys.node(2).rebuilds.is_empty(),
        "rebuild did not finish after membership STUCK_NS"
    );
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&0x10)
    );
}

/// D-169: a rebuild whose cursor reaches a node while no `ReportAsk` credit
/// toward it is free (another rebuild already has a `ReportAsk` in flight to
/// that node) also cuts at `STUCK_NS` from reaching it.
#[test]
fn a_rebuild_waiting_with_no_free_credit_cuts_at_stuck_ns() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    mach.membership.sweep_ms = 100;
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    // Ensure node 1 holds both IOMMU_ROLE and IOAPIC_ROLE.
    let ioapic_holder = m.sys.role(IOAPIC_ROLE).holder();
    if ioapic_holder != 1 {
        let role_slots = slots_of(
            m.sys.node(ioapic_holder),
            m.t[ioapic_holder as usize],
            ObjectKind::Role,
        );
        let call = Syscall::RoleHand {
            role: s(role_slots[IOAPIC_ROLE]),
            node: node_cap(1),
        };
        let _ = m.call(ioapic_holder, call);
        m.run(30 * MS);
    }
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 1);

    // Node 0 attaches a device on IOMMU and claims GSI 5 on IOAPIC.
    attached(&mut m, 0);
    let irq_claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: IrqSource::line(5, false, false).raw(),
        core: 0,
        vector: 48,
        destination: s(200),
    };
    let _ = m.call(0, irq_claim);
    m.run(30 * MS);
    assert!(m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&5));

    // Make node 0 silent on `ReportAsk`, then kill node 1 so node 2 rebuilds
    // BOTH roles concurrently.
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;
    kill_node(&mut m, 1);

    // Wait until one rebuild sent a `ReportAsk` to node 0 and the second
    // rebuild reached node 0 with `asked == None` and `asked_at.is_some()`
    // (blocked on the one-ask-in-flight rule).
    assert!(m.step_until(|m| {
        let rebuilds = &m.sys.node(2).rebuilds;
        rebuilds.len() == 2
            && rebuilds.values().any(|r| r.node == 0 && r.asked.is_some())
            && rebuilds
                .values()
                .any(|r| r.node == 0 && r.asked.is_none() && r.asked_at.is_some())
    }));

    // After `STUCK_NS`, BOTH rebuilds cut node 0 and finish.
    m.run(stuck + 10 * MS);
    assert!(
        m.sys.node(2).rebuilds.is_empty(),
        "both rebuilds did not finish after STUCK_NS"
    );
    let deadline_cuts = m
        .sys
        .node(2)
        .rebuild_cuts
        .0
        .iter()
        .filter(|&&(_, peer, _, reason)| peer == 0 && reason == RebuildCut::Deadline)
        .count();
    assert_eq!(
        deadline_cuts, 2,
        "expected both rebuilds to cut node 0 at STUCK_NS"
    );
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
    assert!(!m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&5));
}

/// D-169: while the rebuild cursor waits for a node's `Report`, the holder
/// keeps a kernel-ring observation toward that node so a node that leaves
/// `ReportAsk` unconsumed is accused with `Evidence::Stuck`.
#[test]
fn a_node_that_never_consumes_report_ask_is_accused_stuck() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    mach.membership.sweep_ms = 100;
    // Set stale_ms > stuck_ms so the accusation must come from the ring
    // observation (`Evidence::Stuck`), not from heartbeat staleness.
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    attached(&mut m, 0);
    let stuck = m.sys.canonical().stuck_ns;

    // Hang node 0 so it never consumes its kernel ring, then kill holder 1.
    m.hang(0, true);
    kill_node(&mut m, 1);

    // Wait until successor 2 writes `ReportAsk` to node 0 and opens a
    // kernel-ring observation toward node 0.
    assert!(m.step_until(|m| {
        m.sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node == 0 && r.asked.is_some())
            && m.sys.canonical().beacons[2].observations.iter().any(|o| {
                o.node == 0 && matches!(o.watched, crate::table::Watched::Ring { user: false, .. })
            })
    }));

    // At `STUCK_NS`, node 0 is accused with `Evidence::Stuck` and the rebuild
    // finishes.
    m.run(stuck + 5 * MS);
    let member0 = m.sys.member(0);
    assert_eq!(
        member0.evidence(),
        Evidence::Stuck,
        "node 0 leaving ReportAsk unconsumed was not accused stuck: {member0:?}"
    );
    assert!(
        m.sys.node(2).rebuilds.is_empty(),
        "rebuild did not finish after node 0 went stuck"
    );
}

/// D-169: a late `Report` arriving after the deadline is dropped and frees its
/// credit without restoring the cut node's claims.
#[test]
fn a_late_report_after_the_deadline_is_dropped_and_its_credit_freed() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    mach.membership.sweep_ms = 100;
    mach.membership.stale_ms = 200;
    mach.membership.stuck_ms = 40;
    let mut m = S::on(&mach);
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let stuck = m.sys.canonical().stuck_ns;

    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;
    kill_node(&mut m, 1);
    assert!(m.step_until(|m| {
        m.sys.node(2).rebuilds.get(&IOMMU_ROLE).is_some_and(|r| {
            r.node == 0 && r.asked.is_some() && !m.sys.node(0).swallowed_asks.is_empty()
        })
    }));
    let (credit_idx, credit_ser) = m.sys.node(2).rebuilds[&IOMMU_ROLE]
        .asked
        .expect("ask in flight");

    // Advance past `STUCK_NS`: node 0 is cut, its context entry is cleared,
    // and the rebuild ends; credit `(credit_idx, credit_ser)` stays occupied.
    m.run(stuck + 10 * MS);
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16))
    );
    assert!(
        m.sys.node(2).peers_of(0)[usize::from(credit_idx)].is_some(),
        "credit should still be in use before the late Report arrives"
    );

    // Now node 0 sends the late `Report` carrying its valid device item.
    let g0 = m.sys.member(0).generation();
    let g2 = m.sys.member(2).generation();
    let body = Body::Report {
        next: REPORT_END,
        items: vec![Item::Device(DEVICE as u16, root)],
    };
    let header = RingHeader::new(
        NodeMessage::Report,
        body.entries() as u8,
        (0, g0),
        g2,
        Some((credit_idx, credit_ser)),
    );
    m.sys.inject(0, 2, crate::rings::Message { header, body });
    m.run(5 * MS);

    // The late `Report` freed its credit and changed nothing on the unit.
    assert!(
        m.sys.node(2).peers_of(0)[usize::from(credit_idx)].is_none(),
        "late Report did not free its credit"
    );
    assert!(
        !m.sys.canonical().units[&IOMMU_ROLE]
            .entries
            .contains_key(&(DEVICE as u16)),
        "late Report restored a cut node's context entry"
    );
}

/// D-169: a `Report` with one bad item among good ones (tested for a device,
/// a handle, and a line) refuses the whole `Report` (trace code 11), ends that
/// node's report immediately without further asks, and clears its claims.
#[test]
fn a_report_with_one_bad_item_among_good_ones_ends_the_report_and_clears_its_claims() {
    // Part A: valid device + bad device (owned by node 3) on IOMMU rebuild.
    {
        let mut m = four_with_two_devices();
        let root3 = m.sys.canonical().attach_word(0x11).root();
        let refusals_before = report_refusals(&m, 2);
        let mut edited = false;
        let mut asks_to_0 = 0usize;
        let mut last_prod = m.sys.canonical().rings[2][0].kernel.produced;

        kill_node(&mut m, 1);
        step_events(&mut m, 200 * MS, |m| {
            let ring20 = &m.sys.canonical().rings[2][0].kernel;
            if ring20.produced > last_prod {
                for at in last_prod..ring20.produced {
                    if let Some(RingEntry::Message(msg)) = ring20.at(at)
                        && matches!(msg.body, Body::ReportAsk { role, .. } if usize::from(role) == IOMMU_ROLE)
                    {
                        asks_to_0 += 1;
                    }
                }
                last_prod = ring20.produced;
            }
            m.sys.edit_table(|t| {
                let start = t.beacons[2].consumed[0].0;
                let ring = &mut std::sync::Arc::make_mut(&mut t.rings)[0][2].kernel;
                for at in start..ring.produced {
                    let idx = (at % ring.entries.len() as u64) as usize;
                    if let Some(RingEntry::Message(msg)) = &mut ring.entries[idx]
                        && let Body::Report { next, items } = &mut msg.body
                        && !edited
                        && !items.is_empty()
                    {
                        items.push(Item::Device(0x11, root3));
                        *next = Item::Device(0x12, 0).key();
                        edited = true;
                    }
                }
            });
            edited && m.sys.node(2).rebuilds.is_empty()
        });
        assert!(edited, "never intercepted node 0's Report");
        assert!(m.sys.node(2).rebuilds.is_empty());
        assert_eq!(asks_to_0, 1, "holder kept asking after a bad device item");
        assert!(report_refusals(&m, 2) > refusals_before);
        assert!(
            m.sys
                .node(2)
                .rebuild_cuts
                .0
                .iter()
                .any(|&(r, p, _, reason)| r == IOMMU_ROLE
                    && p == 0
                    && reason == RebuildCut::BadItem)
        );
        let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
        assert!(
            !entries.contains_key(&0x10),
            "node 0's valid device in the bad Report was not cleared"
        );
        assert!(
            entries.contains_key(&0x11),
            "healthy node 3's device was lost"
        );
    }

    // Part B: valid device + bad handle (outside node 0's cores) on IOMMU rebuild.
    {
        let mut m = four_with_two_devices();
        let refusals_before = report_refusals(&m, 2);
        let mut edited = false;
        let mut asks_to_0 = 0usize;
        let mut last_prod = m.sys.canonical().rings[2][0].kernel.produced;

        kill_node(&mut m, 1);
        step_events(&mut m, 200 * MS, |m| {
            let ring20 = &m.sys.canonical().rings[2][0].kernel;
            if ring20.produced > last_prod {
                for at in last_prod..ring20.produced {
                    if let Some(RingEntry::Message(msg)) = ring20.at(at)
                        && matches!(msg.body, Body::ReportAsk { role, .. } if usize::from(role) == IOMMU_ROLE)
                    {
                        asks_to_0 += 1;
                    }
                }
                last_prod = ring20.produced;
            }
            m.sys.edit_table(|t| {
                let start = t.beacons[2].consumed[0].0;
                let ring = &mut std::sync::Arc::make_mut(&mut t.rings)[0][2].kernel;
                for at in start..ring.produced {
                    let idx = (at % ring.entries.len() as u64) as usize;
                    if let Some(RingEntry::Message(msg)) = &mut ring.entries[idx]
                        && let Body::Report { next, items } = &mut msg.body
                        && !edited
                        && !items.is_empty()
                    {
                        // Handle belonging to core 3 (node 3), not node 0.
                        let bad_handle = (3 * dodos_abi::x86_64::IRQ_TABLE_ENTRIES) as u32;
                        items.push(Item::Handle(bad_handle));
                        *next = Item::Handle(bad_handle + 1).key();
                        edited = true;
                    }
                }
            });
            edited && m.sys.node(2).rebuilds.is_empty()
        });
        assert!(edited, "never intercepted node 0's Report");
        assert!(m.sys.node(2).rebuilds.is_empty());
        assert_eq!(asks_to_0, 1, "holder kept asking after a bad handle item");
        assert!(report_refusals(&m, 2) > refusals_before);
        assert!(
            !m.sys.canonical().units[&IOMMU_ROLE]
                .entries
                .contains_key(&0x10),
            "node 0's valid device in the Report with a bad handle was not cleared"
        );
    }

    // Part C: valid line (GSI 5) + bad line (GSI 6 owned by node 3) on IOAPIC rebuild.
    {
        let mut m = four_with_two_devices();
        let ioapic_holder = m.sys.role(IOAPIC_ROLE).holder();
        if ioapic_holder != 1 {
            let role_slots = slots_of(
                m.sys.node(ioapic_holder),
                m.t[ioapic_holder as usize],
                ObjectKind::Role,
            );
            let _ = m.call(
                ioapic_holder,
                Syscall::RoleHand {
                    role: s(role_slots[IOAPIC_ROLE]),
                    node: node_cap(1),
                },
            );
            m.run(30 * MS);
        }
        assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), 1);
        for (node, gsi, core) in [(0u16, 5u32, 0u64), (3u16, 6u32, 3u64)] {
            let _ = m.call(
                node,
                Syscall::IrqClaim {
                    control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
                    source: IrqSource::line(gsi, false, false).raw(),
                    core,
                    vector: 48,
                    destination: s(200),
                },
            );
            m.run(30 * MS);
        }
        assert!(m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&5));
        assert!(m.sys.canonical().units[&IOAPIC_ROLE].lines.contains_key(&6));

        let target0 = m.sys.node(0).line_target(0, 48);
        let refusals_before = report_refusals(&m, 2);
        let mut edited = false;
        let mut asks_to_0_ioapic = 0usize;
        let mut last_prod = m.sys.canonical().rings[2][0].kernel.produced;

        kill_node(&mut m, 1);
        step_events(&mut m, 200 * MS, |m| {
            let ring20 = &m.sys.canonical().rings[2][0].kernel;
            if ring20.produced > last_prod {
                for at in last_prod..ring20.produced {
                    if let Some(RingEntry::Message(msg)) = ring20.at(at)
                        && matches!(msg.body, Body::ReportAsk { role, .. } if usize::from(role) == IOAPIC_ROLE)
                    {
                        asks_to_0_ioapic += 1;
                    }
                }
                last_prod = ring20.produced;
            }
            m.sys.edit_table(|t| {
                let start = t.beacons[2].consumed[0].0;
                let ring = &mut std::sync::Arc::make_mut(&mut t.rings)[0][2].kernel;
                for at in start..ring.produced {
                    let idx = (at % ring.entries.len() as u64) as usize;
                    if let Some(RingEntry::Message(msg)) = &mut ring.entries[idx]
                        && let Body::Report { next, items } = &mut msg.body
                        && !edited
                        && items.iter().any(|i| matches!(i, Item::Line(5, ..)))
                    {
                        items.push(Item::Line(6, false, false, target0, true));
                        *next = Item::Line(7, false, false, Target::Masked, false).key();
                        edited = true;
                    }
                }
            });
            edited && m.sys.node(2).rebuilds.is_empty()
        });
        assert!(edited, "never intercepted node 0's IOAPIC Report");
        assert!(m.sys.node(2).rebuilds.is_empty());
        assert_eq!(
            asks_to_0_ioapic, 1,
            "holder kept asking after a bad line item"
        );
        assert!(report_refusals(&m, 2) > refusals_before);
        let lines = &m.sys.canonical().units[&IOAPIC_ROLE].lines;
        assert!(
            !lines.contains_key(&5),
            "node 0's valid line in the bad Report was not cleared"
        );
        assert!(lines.contains_key(&6), "healthy node 3's line was lost");
    }
}

/// D-169 bound: a faulty peer answering with strictly rising keys and bad
/// items (`FaultyReport::BadItems`) is cut on its first ask, and a healthy
/// multi-page peer (30 items) finishes within `ceil(items / 28) + 1` asks,
/// each answered within `STUCK_NS`.
#[test]
fn a_faulty_bad_items_peer_is_cut_on_first_ask_and_multi_page_report_meets_bound() {
    // Faulty peer (`FaultyReport::BadItems`): cut on ask 1 within `STUCK_NS`.
    {
        let mut m = four_with_two_devices();
        let stuck = m.sys.canonical().stuck_ns;
        m.sys.node_mut(0).faulty_report = FaultyReport::BadItems;
        let mut asks_to_0 = 0usize;
        let mut last_prod = m.sys.canonical().rings[2][0].kernel.produced;
        let mut first_ask_at = None;

        kill_node(&mut m, 1);
        step_events(&mut m, 200 * MS, |m| {
            let ring20 = &m.sys.canonical().rings[2][0].kernel;
            if ring20.produced > last_prod {
                for at in last_prod..ring20.produced {
                    if let Some(RingEntry::Message(msg)) = ring20.at(at)
                        && matches!(msg.body, Body::ReportAsk { role, .. } if usize::from(role) == IOMMU_ROLE)
                    {
                        asks_to_0 += 1;
                        first_ask_at.get_or_insert(m.sys.now());
                    }
                }
                last_prod = ring20.produced;
            }
            first_ask_at.is_some() && m.sys.node(2).rebuilds.is_empty()
        });
        assert!(m.sys.node(2).rebuilds.is_empty());
        assert_eq!(asks_to_0, 1, "BadItems peer was asked more than once");
        assert!(
            m.sys.now() - first_ask_at.unwrap() <= stuck,
            "rebuild did not finish within STUCK_NS"
        );
        assert!(
            m.sys
                .node(2)
                .rebuild_cuts
                .0
                .iter()
                .any(|&(r, p, _, reason)| r == IOMMU_ROLE
                    && p == 0
                    && reason == RebuildCut::BadItem)
        );
        assert!(
            !m.sys.canonical().units[&IOMMU_ROLE]
                .entries
                .contains_key(&0x10)
        );
        assert!(
            m.sys.canonical().units[&IOMMU_ROLE]
                .entries
                .contains_key(&0x11)
        );
    }

    // Healthy peer with 30 remapping handles (> 28 items): spans 2 pages,
    // finishes in `2 <= ceil(30 / 28) + 1 = 3` asks, each within `STUCK_NS`,
    // with zero cuts and all 30 handles kept.
    {
        let mut m = three();
        let stuck = m.sys.canonical().stuck_ns;
        let count = 30u32;
        for i in 0..count {
            let claim = Syscall::IrqClaim {
                control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
                source: IrqSource::msi(0, 0, i as u8, 0).raw(),
                core: 0,
                vector: u64::from(48 + i),
                destination: s(200 + i),
            };
            let _ = m.call(0, claim);
            m.run(20 * MS);
        }
        assert_eq!(m.sys.node(0).items(IOMMU_ROLE).len(), count as usize);

        let mut ask_times: Vec<u64> = Vec::new();
        let mut last_prod = m.sys.canonical().rings[2][0].kernel.produced;
        kill_node(&mut m, 1);
        step_events(&mut m, 200 * MS, |m| {
            let ring20 = &m.sys.canonical().rings[2][0].kernel;
            if ring20.produced > last_prod {
                for at in last_prod..ring20.produced {
                    if let Some(RingEntry::Message(msg)) = ring20.at(at)
                        && matches!(msg.body, Body::ReportAsk { role, .. } if usize::from(role) == IOMMU_ROLE)
                    {
                        ask_times.push(m.sys.now());
                    }
                }
                last_prod = ring20.produced;
            }
            !ask_times.is_empty() && m.sys.node(2).rebuilds.is_empty()
        });
        assert!(m.sys.node(2).rebuilds.is_empty());
        let bound = (count as usize).div_ceil(crate::rings::REPORT_ITEMS) + 1;
        assert_eq!(ask_times.len(), 2, "30 items must take 2 ReportAsks");
        assert!(ask_times.len() <= bound);
        for pair in ask_times.windows(2) {
            assert!(pair[1] - pair[0] <= stuck);
        }
        assert!(
            m.sys.node(2).rebuild_cuts.0.is_empty(),
            "healthy multi-page node was cut"
        );
        assert_eq!(
            m.sys.canonical().units[&IOMMU_ROLE].remap.len(),
            count as usize
        );
    }
}

/// The checker's `REBUILD` invariants reject an in-flight `ReportAsk` without
/// `asked_at` and a live running holder waiting past `STUCK_NS`.
#[test]
fn checker_rejects_a_rebuild_without_deadline_or_past_stuck_ns() {
    let mut m = four_with_two_devices();
    let stuck = m.sys.canonical().stuck_ns;
    m.sys.node_mut(0).faulty_report = FaultyReport::Silent;

    kill_node(&mut m, 1);
    assert!(m.step_until(|m| {
        m.sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node == 0 && r.asked.is_some())
    }));
    let saved = m.sys.node(2).rebuilds[&IOMMU_ROLE].asked_at;

    // 1. Missing `asked_at` while an ask is in flight.
    m.sys
        .node_mut(2)
        .rebuilds
        .get_mut(&IOMMU_ROLE)
        .unwrap()
        .asked_at = None;
    assert_eq!(
        m.sys.check(),
        Err("REBUILD: an in-flight ReportAsk has no deadline recorded".into())
    );

    // 2. Waiting past `STUCK_NS + RECOVERY_PERIOD_NS`.
    m.sys
        .node_mut(2)
        .rebuilds
        .get_mut(&IOMMU_ROLE)
        .unwrap()
        .asked_at = saved;
    m.sys
        .move_clock(saved.unwrap() + stuck + dodos_abi::RECOVERY_PERIOD_NS + 1);
    assert_eq!(
        m.sys.check(),
        Err("REBUILD: a live holder waited past STUCK_NS for a rebuild Report".into())
    );
}
