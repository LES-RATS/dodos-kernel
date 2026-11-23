//! Pass 47: review of pass 46 (every due event of every node before a
//! call, the tests that survived their mutations, the liveness check).

use super::pass46::{WRITE_52, draining, fencing, line_of_a_non_holder};
use super::units::IOAPIC_ROLE;
use super::*;
use crate::cores::{Completes, Flush, Request, RequestState};
use crate::rings::{Target, UnitOp};
use crate::table::{LineFree, Word};
use dodos_abi::{MemberState, Owner, Ownership};

// ---------------------------------------------------------------------------
// 1. Every due event before any call, on every node (findings 1, 5, 6)
// ---------------------------------------------------------------------------

/// Node 0's core drains 17 timers due at one instant; a call of node 1 at
/// that instant takes them first.
#[test]
fn a_call_on_one_node_takes_the_due_events_of_another_node_first() {
    let mut m = S::two();
    draining(&mut m);
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(!m.sys.node(0).timer_draining(0));
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 0);
}

/// `review46::a_call_runs_before_a_core_answer_deadline`, turned around:
/// the `CORE_ANSWER_NS` due event of node 0's request is taken before a
/// call at its instant, and node 0 accuses itself there.
#[test]
fn a_call_takes_a_core_answer_deadline_of_another_node_first() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0, 1], &[2]]));
    let now = m.sys.now();
    {
        let node = m.sys.node_mut(0);
        node.cores[0].request = Some(Request {
            flush: Flush::Nothing,
            asked: 0b10,
            acked: 0,
            started: now,
            completes: Completes::Barrier(0),
            waiter: None,
        });
        node.cores[0].request_state = RequestState::InUse;
    }
    m.sys.move_clock(now + m.sys.canonical().core_answer_ns);
    assert!(m.sys.node(0).due_now(0));
    assert!(!m.sys.node(0).timer_draining(0));
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(m.sys.node(0).table.beacons[0].panic.is_some());
}

/// The sweep is a due event of the first core's timer queue (The sweep):
/// a call of node 1 at its instant comes after node 0's sweep.
#[test]
fn a_call_takes_the_sweep_of_another_node_first() {
    let mut m = S::two();
    m.settle();
    let sweep = m.sys.node(0).next_sweep;
    m.sys.move_clock(sweep);
    assert!(m.sys.node(0).reserve_timer_due());
    assert_eq!(m.call(1, Syscall::Yield), DONE);
    assert!(m.sys.node(0).next_sweep > sweep);
}

// ---------------------------------------------------------------------------
// 2. Tests that survived their mutations (findings 2, 3, 4)
// ---------------------------------------------------------------------------

/// The claimant's line word 5 written free by `writer`, journaled with
/// `line_free`, judged by the writer's checks.
fn free_line_5(m: &S, writer: u16, line_free: LineFree) -> Result<(), &'static str> {
    let mut sys = m.sys.clone();
    sys.edit_table(|t| {
        let taken = t.lines[&5];
        let unclaimed = Ownership::free(Owner::Unclaimed).raw();
        let mut write = t.journal(writer, Word::Line(5), taken, unclaimed);
        write.line_free = Some(line_free);
        t.lines.remove(&5);
        t.writes.push(write);
    });
    sys.node(writer).check()
}

/// Finding 2: the holder kept the reclaim record of an older generation of
/// the claimant, with no R1 recorded on the IOAPIC's role. Accused again,
/// the claimant's line word is freed by an R1 that does not walk for its
/// current generation: refused.
#[test]
fn checker_rejects_an_r1_line_free_on_a_reclaim_record_of_an_older_generation() {
    let (mut m, holder, claimant, _) = line_of_a_non_holder();
    let g = fencing(&mut m, claimant);
    let stale = crate::nodes::Reclaim {
        generation: g - 1,
        isolated: true,
        ..Default::default()
    };
    let model = m.sys.node_mut(holder);
    model.reclaims.insert(claimant, stale);
    model.r1(claimant, g);
    assert!(!model.table.lines.contains_key(&5));
    let write = model.table.writes.iter().find(|w| w.word == Word::Line(5));
    assert_eq!(
        write.and_then(|w| w.line_free),
        Some(LineFree::R1 { walking: false })
    );
    assert_eq!(m.sys.node(holder).check(), Err(WRITE_52));
}

/// Finding 3: the holder's free of a line word after it acknowledged the
/// claimant's generation (R6: "R6 frees no line word"), journaled as a
/// walking R1, is refused by the acknowledgement alone.
#[test]
fn checker_rejects_a_line_free_journaled_as_r1_after_the_holders_acknowledgement() {
    let (mut m, holder, claimant, _) = line_of_a_non_holder();
    let g = m.sys.canonical().generation(claimant);
    let fenced = dodos_abi::Member::new(
        dodos_abi::MemberState::Fenced,
        dodos_abi::Evidence::SelfReported,
        dodos_abi::DRIVER_NONE,
        g,
    );
    m.sys.edit_table(|t| t.members[claimant as usize] = fenced);
    let walking = LineFree::R1 { walking: true };
    assert_eq!(free_line_5(&m, holder, walking), Ok(()));
    m.sys
        .edit_table(|t| t.beacons[holder as usize].acked[claimant as usize] = g);
    assert_eq!(free_line_5(&m, holder, walking), Err(WRITE_52));
}

/// The assistant's mutation B: a release free journaled by a node that does
/// not hold the controller is refused; the holder's is lawful.
#[test]
fn checker_rejects_a_release_free_by_a_node_that_does_not_hold_the_controller() {
    let (m, holder, claimant, third) = line_of_a_non_holder();
    let release = LineFree::Release {
        requester: claimant,
        generation: m.sys.canonical().generation(claimant),
    };
    assert_eq!(free_line_5(&m, holder, release), Ok(()));
    assert_eq!(free_line_5(&m, third, release), Err(WRITE_52));
}

/// Finding 4: a line irq that is not removing holds its line word
/// (invariant 96); its word read free is refused.
#[test]
fn a_line_irq_not_removing_whose_word_is_free_is_reported() {
    let (mut m, _, claimant, _) = line_of_a_non_holder();
    assert_eq!(m.sys.node(claimant).check(), Ok(()));
    let model = m.sys.node_mut(claimant);
    assert!(
        model
            .irqs
            .values()
            .any(|irq| irq.source.is_line() && !irq.removing)
    );
    model.table.lines.remove(&5);
    assert_eq!(
        m.sys.node(claimant).check(),
        Err("96: line words and claimed lines")
    );
}

/// The assistant's mutation C: operation 5 claimed carries *release* 0
/// (bit 35 is masked only); the holder refuses a claimed release on the
/// requester's own line, which it serves with *release* 0.
#[test]
fn the_holder_refuses_a_release_on_a_claimed_entry() {
    let (m, holder, claimant, _) = line_of_a_non_holder();
    let model = m.sys.node(holder);
    let g = m.sys.canonical().generation(claimant);
    let apic = model.machine.core_list()[claimant as usize].hw_id as u32;
    let claimed = |release| UnitOp::Redirection {
        gsi: 5,
        claimed: true,
        release,
        level: false,
        low: false,
        target: Target::Direct { vector: 48, apic },
    };
    assert!(model.operands_allowed(IOAPIC_ROLE, claimed(false), (claimant, g)));
    assert!(!model.operands_allowed(IOAPIC_ROLE, claimed(true), (claimant, g)));
}

// ---------------------------------------------------------------------------
// 3. The liveness check (finding 7)
// ---------------------------------------------------------------------------

/// D-142, line 397: node `X` granted a frame to `Y`, which mapped it in an
/// iospace whose unit failed, so its use entry keeps the count for the
/// failed unit; `Y` is killed (its barrier never holds, a device was
/// behind the failed unit), then `X`. In a run `X`'s barrier never holds
/// either: `Y`'s `maps_of` bit for `X` stays, and nobody can acknowledge
/// `Y` (line 397). The check is judged on that state with the bit read
/// clear, so that the barrier holds while the count holds R6 back.
fn granter_held_back_by_a_failed_unit() -> S {
    use super::pass24::{GOT, THIRD, X, Y, granted, io_map, kept_without_root, until_done};
    use super::units::IOMMU_ROLE;
    let mut g = granted(false);
    let map = io_map(&g.m, GOT);
    g.m.ok(Y, map);
    g.m.sys.break_unit(IOMMU_ROLE);
    until_done(&mut g.m, Y, Syscall::CapDelete(s(GOT)));
    g.m.settle();
    kept_without_root(&g);
    let mut m = g.m;
    m.supervisor(THIRD, m.t[THIRD as usize]);
    m.ok(THIRD, super::pass17::kill(Y));
    m.run(200 * MS);
    m.ok(THIRD, super::pass17::kill(X));
    m.run_by(500 * MS, MS);
    assert_eq!(m.state(Y), MemberState::Fenced);
    assert_eq!(m.state(X), MemberState::Fenced);
    let g = m.sys.member(X).generation();
    assert!(!m.sys.node(THIRD).barrier_holds(X));
    m.sys
        .edit_table(|t| t.beacons[Y as usize].maps_of[X as usize] = false);
    assert!(m.sys.node(THIRD).barrier_holds(X));
    assert!(m.sys.node(THIRD).r6_held_back(X, g));
    let index = m.sys.canonical().grants[X as usize]
        .values()
        .find_map(|e| e.use_index)
        .expect("the grant");
    let leaves = m.sys.canonical().uses[Y as usize][&index].leaves;
    assert_eq!(m.sys.node(Y).frozen_uses.get(&index), Some(&leaves));
    m
}

/// The count a failed unit keeps is the one lawful hold-back of R6 once
/// the barrier holds.
#[test]
fn a_grant_counted_for_a_failed_unit_holds_r6_back_lawfully() {
    let m = granter_held_back_by_a_failed_unit();
    super::explore::assert_no_stalled_r6(&m);
}

/// The same count with no failed unit behind it (the grantee frozen all
/// the same) is a stall: a frozen grantee alone excuses nothing.
#[test]
#[should_panic(expected = "R6 held back")]
fn a_grant_counted_by_a_frozen_grantee_without_a_failed_unit_is_a_stall() {
    let mut m = granter_held_back_by_a_failed_unit();
    m.sys.node_mut(super::pass24::Y).frozen_uses.clear();
    super::explore::assert_no_stalled_r6(&m);
}

/// A fencing node whose driver is live and takes events is judged: the
/// fence ends by `FENCE_NS`.
#[test]
#[should_panic(expected = "left fencing after quiet by its driver")]
fn a_node_left_fencing_by_a_live_driver_fails_the_liveness_check() {
    let mut m = super::units::three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    let g = m.sys.canonical().generation(victim);
    let member = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        holder,
        g,
    );
    m.sys.edit_table(|t| t.members[victim as usize] = member);
    super::explore::assert_no_stalled_r6(&m);
}

/// The live node that sees the barrier hold is judged whatever its first
/// core: `pass46::a_stalled_r6_fails_the_quiet_liveness_check` with core 0
/// of every live node silent.
#[test]
#[should_panic(expected = "R6 held back")]
fn a_stalled_r6_is_judged_on_a_live_node_whose_core_0_is_silent() {
    let mut m = super::units::three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, super::pass17::kill(victim));
    assert!(m.step_until(|m| {
        m.sys
            .node(holder)
            .reclaims
            .get(&victim)
            .is_some_and(|r| r.r1.contains_key(&IOAPIC_ROLE))
    }));
    let g = m.sys.canonical().generation(victim);
    m.sys.edit_table(|t| {
        t.lines
            .insert(9, Ownership::taken(Owner::Node(victim), g).raw());
    });
    m.run_by(200 * MS, MS);
    assert_eq!(m.state(victim), MemberState::Fenced);
    for n in (0..3u16).filter(|&n| n != victim) {
        m.sys.node_mut(n).cores[0].silent = true;
    }
    super::explore::assert_no_stalled_r6(&m);
}
