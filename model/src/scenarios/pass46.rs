//! Pass 46: review of pass 45 (calls after the due events, actions on a
//! draining core, who frees a line word, R1's line free, the tests that
//! survived their mutations, a stalled R6, line controllers other than the
//! IOAPIC).

use super::pass17::{fresh, kill};
use super::pass18::claim_line;
use super::units::{IOAPIC_ROLE, three};
use super::*;
use crate::system::Event;
use crate::table::{LineFree, Word};
use dodos_abi::{MemberState, Owner, Ownership};

const WRITE_52: &str = "52: write of an ownership word";

/// Node 0 of two arms 17 timers at one instant; at that instant one timer
/// interrupt takes 16 of them and leaves its core draining.
fn draining(m: &mut S) {
    m.ok(0, m.retype(0, ObjectKind::Notify, 0, 150));
    for i in 0..17u32 {
        m.ok(0, m.retype(0, ObjectKind::Timer, 0, 160 + i));
    }
    let deadline = m.sys.now() + 5 * MS;
    for i in 0..17u32 {
        let set = Syscall::TimerSet {
            timer: s(160 + i),
            notify: s(150),
            deadline,
            tolerance: 0,
        };
        assert_eq!(m.call(0, set), Ok(Output::WasArmed(0)));
    }
    m.run(deadline - m.sys.now() - 1);
    m.sys.move_clock(deadline);
    m.sys.fire(Event::Timer {
        node: 0,
        position: 0,
    });
    assert!(m.sys.node(0).timer_draining(0));
    assert_eq!(m.sys.node(0).timers.values().flatten().count(), 1);
}

/// The member word of `node` made *fencing*, self-reported, in its current
/// generation.
fn fencing(m: &mut S, node: u16) -> u32 {
    let g = m.sys.canonical().generation(node);
    let member = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        dodos_abi::DRIVER_NONE,
        g,
    );
    m.sys.edit_table(|t| t.members[node as usize] = member);
    g
}

/// Three nodes, line 5 claimed by a node that does not hold the IOAPIC's
/// role: the holder, the claimant and the third node.
fn line_of_a_non_holder() -> (S, u16, u16, u16) {
    let mut m = three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let others: Vec<u16> = (0..3).filter(|&n| n != holder).collect();
    let (claimant, third) = (others[0], others[1]);
    assert!(matches!(
        claim_line(&mut m, claimant, 5, 210),
        Ok(Output::Msi { .. })
    ));
    (m, holder, claimant, third)
}

/// The line word 5 written free in the last event, as its journal reads it.
fn line_5_free(m: &S) -> Option<crate::table::WordWrite> {
    let unclaimed = Ownership::free(Owner::Unclaimed).raw();
    m.sys
        .canonical()
        .writes
        .iter()
        .find(|w| w.word == Word::Line(5) && w.after == unclaimed)
        .copied()
}

/// The removal of the irq in slot 210 of `node`, step after step, until it
/// ends.
fn remove_210(m: &mut S, node: u16) {
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        m.settle();
        outcome = m.call(node, Syscall::CapDelete(s(210)));
        if outcome != Err(Error::Again) {
            break;
        }
    }
    assert_eq!(outcome, DONE);
}

// ---------------------------------------------------------------------------
// 1. No call before the due events (finding 1)
// ---------------------------------------------------------------------------

/// `review45_call_runs_on_a_draining_core`, turned around: the call takes
/// the due event left at its instant first.
#[test]
fn a_call_takes_the_due_events_left_at_its_instant_first() {
    let mut m = S::two();
    draining(&mut m);
    let later = m.sys.now() + 100 * MS;
    let set = Syscall::TimerSet {
        timer: s(160),
        notify: s(150),
        deadline: later,
        tolerance: 0,
    };
    assert!(m.call(0, set).is_ok());
    assert!(!m.sys.node(0).timer_draining(0));
    let armed: Vec<u64> = m
        .sys
        .node(0)
        .timers
        .values()
        .flatten()
        .map(|t| t.instant)
        .collect();
    assert_eq!(armed, [later], "the due timer fired before the call");
}

#[test]
fn checker_rejects_a_call_while_a_core_of_its_node_drains() {
    let mut m = S::two();
    draining(&mut m);
    let thread = m.t[0];
    let _ = m.sys.node_mut(0).apply(thread, Syscall::CapInspect(s(150)));
    assert_eq!(
        m.sys.check(),
        Err("node 0: 47: a call ran while a core of the node was draining due events".into())
    );
}

// ---------------------------------------------------------------------------
// 2. Actions on a draining core (finding 2)
// ---------------------------------------------------------------------------

/// A park of the draining core stays offered: the node chose to stop it
/// (the reading of `System::events`).
#[test]
fn an_action_stays_offered_on_a_draining_core() {
    let mut m = S::two();
    draining(&mut m);
    m.sys
        .node_mut(0)
        .hw_out
        .push(crate::nodes::HwAction::Park { core: 0 });
    // The action leaves the node's model at its next event.
    m.sys.fire(Event::Ipi {
        node: 0,
        position: 0,
    });
    assert!(m.sys.node(0).timer_draining(0));
    let events = m.sys.events();
    assert!(events.contains(&Event::Action(0)), "{events:?}");
    let timer = Event::Timer {
        node: 0,
        position: 0,
    };
    assert!(events.contains(&timer), "{events:?}");
}

/// An idle ARM64 core that drains does not go to sleep before its due
/// events; once they are taken, it may.
#[test]
fn a_draining_core_does_not_sleep() {
    let mut m = S::on(&machine(Arch::Aarch64, 2, &[&[0], &[1]]));
    let method = dodos_abi::IDLE_METHOD_PSCI | ((1u64 << 16) << 32);
    let entry = [200_000, 400_000, method, dodos_abi::IDLE_FLAG_LOSES_CONTEXT];
    let idle = Syscall::NodeIdle {
        node: node_cap(0),
        core: 0,
        words: entry.to_vec(),
    };
    m.ok(0, idle);
    m.ok(0, m.retype(0, ObjectKind::Notify, 0, 150));
    for i in 0..17u32 {
        m.ok(0, m.retype(0, ObjectKind::Timer, 0, 160 + i));
    }
    let deadline = m.sys.now() + 5 * MS;
    for i in 0..17u32 {
        let set = Syscall::TimerSet {
            timer: s(160 + i),
            notify: s(150),
            deadline,
            tolerance: 0,
        };
        assert_eq!(m.call(0, set), Ok(Output::WasArmed(0)));
    }
    m.ok(0, Syscall::ThreadSuspend(s(dodos_abi::START_SLOT_THREAD)));
    m.sys.move_clock(deadline);
    let timer = Event::Timer {
        node: 0,
        position: 0,
    };
    m.sys.fire(timer);
    assert!(m.sys.node(0).timer_draining(0));
    let events = m.sys.events();
    assert!(!events.contains(&Event::Sleep { core: 0 }), "{events:?}");
    m.sys.fire(timer);
    assert!(!m.sys.node(0).timer_draining(0));
    let events = m.sys.events();
    assert!(events.contains(&Event::Sleep { core: 0 }), "{events:?}");
}

// ---------------------------------------------------------------------------
// 3. Who frees a line word at a removal (D-150)
// ---------------------------------------------------------------------------

/// The removal by a node that does not hold the controller: the holder's
/// gate frees the word in the call that serves the masking with
/// *release*, while the owner's irq is still there, and the owner never
/// writes it.
#[test]
fn a_removal_frees_the_line_word_in_the_holders_masking_call() {
    let (mut m, holder, claimant, _) = line_of_a_non_holder();
    let generation = m.sys.canonical().generation(claimant);
    let taken = m.sys.canonical().line_word(5);
    assert_eq!(
        m.call(claimant, Syscall::CapDelete(s(210))),
        Err(Error::Again)
    );
    assert_eq!(m.sys.canonical().line_word(5), taken);
    let mut freed = None;
    for _ in 0..10_000 {
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.advance(m.sys.now() + 10_000),
        }
        freed = line_5_free(&m);
        assert_eq!(m.sys.check(), Ok(()));
        if freed.is_some() {
            break;
        }
    }
    let freed = freed.expect("the line word freed");
    assert_eq!(freed.writer, holder);
    let read = LineFree::Release {
        requester: claimant,
        generation,
    };
    assert_eq!(freed.line_free, Some(read));
    let irq = m
        .sys
        .node(claimant)
        .irqs
        .values()
        .find(|i| i.source.id() == 5);
    assert!(irq.is_some_and(|i| i.removing), "the irq is still removing");
    remove_210(&mut m, claimant);
    assert!(!m.sys.canonical().lines.contains_key(&5));
}

/// A node that holds the controller itself still makes the masking a
/// request: its gate frees the word in the call that serves it.
#[test]
fn the_holders_own_removal_frees_the_line_word_in_its_masking_call() {
    let mut m = three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let generation = m.sys.canonical().generation(holder);
    assert!(matches!(
        claim_line(&mut m, holder, 5, 210),
        Ok(Output::Msi { .. })
    ));
    let _ = m.call(holder, Syscall::CapDelete(s(210)));
    let freed = line_5_free(&m).expect("freed in the masking call");
    assert_eq!(freed.writer, holder);
    let read = LineFree::Release {
        requester: holder,
        generation,
    };
    assert_eq!(freed.line_free, Some(read));
    remove_210(&mut m, holder);
}

/// Once the holder's gate freed the line word in the masking call, the
/// line may be claimed again, by the same node too, while the dying irq
/// waits for its invalidation: `irq_claim` is `busy` on the word alone
/// (D-135, D-150; seeds 340, 975, 1114 and 1991 of `many_seeds`).
#[test]
fn a_released_line_may_be_claimed_again_before_its_removal_ends() {
    let mut m = three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    assert!(matches!(
        claim_line(&mut m, holder, 5, 210),
        Ok(Output::Msi { .. })
    ));
    let _ = m.call(holder, Syscall::CapDelete(s(210)));
    assert!(!m.sys.canonical().lines.contains_key(&5));
    let dying = |m: &S| {
        let irqs = &m.sys.node(holder).irqs;
        irqs.values().any(|i| i.source.id() == 5 && i.removing)
    };
    assert!(dying(&m), "the removal waits for its invalidation");
    let claim = Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::line(5, false, false).raw(),
        core: u64::from(holder),
        vector: 49,
        destination: s(211),
    };
    if m.call(holder, claim) == WAITS {
        m.run(2 * MS);
    }
    assert!(dying(&m));
    let generation = m.sys.canonical().generation(holder);
    let taken = Ownership::taken(Owner::Node(holder), generation);
    assert_eq!(m.sys.canonical().line_word(5), taken);
    remove_210(&mut m, holder);
    assert_eq!(m.sys.canonical().line_word(5), taken);
}

#[test]
fn checker_rejects_a_line_word_freed_by_its_owner() {
    let (mut m, _, claimant, _) = line_of_a_non_holder();
    m.sys.node_mut(claimant).table.write_line(claimant, 5, None);
    assert_eq!(m.sys.node(claimant).check(), Err(WRITE_52));
}

/// The holder's free in a masking call with *release* names the requester
/// the word names: a request of another node frees nothing.
#[test]
fn checker_rejects_a_release_free_for_another_requester() {
    let (m, holder, claimant, third) = line_of_a_non_holder();
    let free_for = |requester: u16| {
        let mut sys = m.sys.clone();
        sys.edit_table(|t| {
            let taken = t.lines[&5];
            let unclaimed = Ownership::free(Owner::Unclaimed).raw();
            let mut write = t.journal(holder, Word::Line(5), taken, unclaimed);
            let generation = t.generation(requester);
            write.line_free = Some(LineFree::Release {
                requester,
                generation,
            });
            t.lines.remove(&5);
            t.writes.push(write);
        });
        sys.node(holder).check()
    };
    assert_eq!(free_for(claimant), Ok(()));
    assert_eq!(free_for(third), Err(WRITE_52));
}

// ---------------------------------------------------------------------------
// 4. R1's free of a line word (finding 3)
// ---------------------------------------------------------------------------

/// `review45_checker_accepts_early_free_by_holder`, turned around: a
/// holder that frees an accused node's line word with no R1 walking for it
/// is refused.
#[test]
fn checker_rejects_a_holders_line_free_before_its_r1_walks() {
    let (mut m, holder, claimant, _) = line_of_a_non_holder();
    let g = fencing(&mut m, claimant);
    assert!(!m.sys.node(holder).reclaims.contains_key(&claimant));
    // The raw journal of the reviewer: no walk read at all.
    let mut raw = m.sys.clone();
    raw.edit_table(|t| {
        let taken = t.lines[&5];
        let unclaimed = Ownership::free(Owner::Unclaimed).raw();
        let write = t.journal(holder, Word::Line(5), taken, unclaimed);
        t.lines.remove(&5);
        t.writes.push(write);
    });
    assert_eq!(raw.node(holder).check(), Err(WRITE_52));
    // The holder's R1 with no reclaim record of the claimant.
    let mut early = m.sys.clone();
    early.node_mut(holder).r1(claimant, g);
    assert!(!early.node(holder).table.lines.contains_key(&5));
    assert_eq!(early.node(holder).check(), Err(WRITE_52));
    // Its R1 once R0 began the reclaim record: accepted.
    m.sys.node_mut(holder).r0(claimant, g);
    assert!(!m.sys.node(holder).table.lines.contains_key(&5));
    assert_eq!(m.sys.node(holder).check(), Ok(()));
}

// ---------------------------------------------------------------------------
// 5. Tests that survive their mutations (findings 4, 5, 7)
// ---------------------------------------------------------------------------

/// A node that does not hold the controller cannot free a line word in
/// its R1, even with its R1 walking and before it acknowledged.
#[test]
fn checker_rejects_an_r1_line_free_by_a_non_holder() {
    let (mut m, holder, claimant, third) = line_of_a_non_holder();
    let _ = fencing(&mut m, claimant);
    let free_by = |m: &S, writer: u16| {
        let mut sys = m.sys.clone();
        sys.edit_table(|t| {
            assert_eq!(t.beacons[writer as usize].acked[claimant as usize], 0);
            let taken = t.lines[&5];
            let unclaimed = Ownership::free(Owner::Unclaimed).raw();
            let mut write = t.journal(writer, Word::Line(5), taken, unclaimed);
            write.line_free = Some(LineFree::R1 { walking: true });
            t.lines.remove(&5);
            t.writes.push(write);
        });
        sys.node(writer).check()
    };
    assert_eq!(free_by(&m, holder), Ok(()));
    assert_eq!(free_by(&m, third), Err(WRITE_52));
}

/// The IOAPIC's holder claims a line and dies: its successor's R1 at the
/// take-over frees the line word before the successor's R5 (its R4 audit
/// kept long, so `r1_done` does not run in that event).
#[test]
fn the_successors_r1_at_the_take_over_frees_the_line_word_before_r5() {
    let mut m = three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let g_holder = m.sys.canonical().generation(holder);
    let killer = (0..3u16).find(|&n| n != holder).unwrap();
    assert!(matches!(
        claim_line(&mut m, holder, 5, 210),
        Ok(Output::Msi { .. })
    ));
    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(holder));
    assert!(m.step_until(|m| {
        m.sys.canonical().fences[holder as usize]
            .confirmed
            .is_some()
    }));
    let confirmed = m.sys.canonical().fences[holder as usize]
        .confirmed
        .expect("fence confirmed");
    let successor = (0..3u16)
        .find(|&n| n != holder && m.sys.member(n).class() == m.sys.member(holder).class())
        .unwrap();
    m.sys.advance(confirmed + dodos_abi::SETTLE_NS - 1);
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), holder);
    assert!(m.sys.canonical().lines.contains_key(&5));
    // More entries than one step audits, none of them a block to audit.
    m.sys
        .node_mut(successor)
        .reclaims
        .get_mut(&holder)
        .expect("reclaim")
        .audit = Some(vec![crate::ObjId(999_999); dodos_abi::REMOVAL_STEP + 1]);
    m.sys.move_clock(confirmed + dodos_abi::SETTLE_NS);
    m.sys.fire(Event::Timer {
        node: successor,
        position: 0,
    });
    assert_eq!(m.sys.role(IOAPIC_ROLE).holder(), successor);
    assert_ne!(
        m.sys.canonical().beacons[successor as usize].acked[holder as usize],
        g_holder,
        "no R5 yet"
    );
    assert!(!m.sys.canonical().lines.contains_key(&5));
    assert_eq!(m.sys.check(), Ok(()));
}

/// D-171's record needs a core still awaiting entry: a two-core start
/// whose cores all entered records nothing at `START_NS`.
#[test]
fn a_start_timeout_needs_a_core_awaiting_entry() {
    assert!(start_timeout_recorded(|_| {}));
    let entered = |node: &mut Model| {
        for core in &mut node.cores {
            core.awaiting_entry = false;
        }
    };
    assert!(!start_timeout_recorded(entered));
}

/// D-171's record needs more than one core. The state is forced: a node
/// of one core has entered it, so no core awaits entry.
#[test]
fn a_start_timeout_needs_more_than_one_core() {
    let one_core = |node: &mut Model| {
        node.cores.truncate(1);
        node.cores[0].awaiting_entry = true;
        assert_eq!(node.width(), 1);
    };
    assert!(!start_timeout_recorded(one_core));
}

/// Whether node 1, starting fresh on two cores, its first core entered,
/// changed by `change`, records a start timeout when it accuses itself
/// `CoreSilent` past `START_NS` from that entry.
fn start_timeout_recorded(change: impl FnOnce(&mut Model)) -> bool {
    let mut m = starting_two_cores();
    let g = m.sys.member(1).generation();
    m.sys.node_mut(1).start_entered = Some(0);
    change(m.sys.node_mut(1));
    m.sys.move_clock(m.sys.now() + dodos_abi::START_NS + MS);
    let node = m.sys.node_mut(1);
    node.accuse_self(dodos_abi::CrashReason::CoreSilent as u32);
    node.table.start_timeout_accusations.0.contains(&(1, g))
}

/// Node 1 of two cores (machine of three) started fresh by node 0, still
/// starting, its first core entered.
fn starting_two_cores() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, fresh(1, 0b110));
    for _ in 0..200 {
        let next = m
            .sys
            .events()
            .into_iter()
            .find(|&e| !matches!(e, Event::EntryStep { core: 2 }));
        let Some(event) = next else { break };
        m.sys.fire(event);
        if m.sys.has(1) && m.sys.node(1).start_entered.is_some() {
            break;
        }
    }
    assert_eq!(m.state(1), MemberState::Starting);
    m
}

/// A self-accusation `CoreSilent` of a fresh start of one core is no
/// harm, even with a start timeout recorded for it.
#[test]
fn harm_refuses_core_silent_of_a_fresh_start_of_one_core() {
    let mut m = S::two();
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, fresh(1, 1 << 1));
    m.hang(0, false);
    let g = m.sys.member(1).generation();
    assert!(m.harm.starts.get(&(1, g)).is_some_and(|s| !s.fresh_multi));
    m.sys.edit_table(|t| {
        t.start_timeout_accusations.0.insert((1, g));
    });
    let accused = dodos_abi::Member::new(
        MemberState::Fencing,
        dodos_abi::Evidence::SelfReported,
        1,
        g,
    );
    let panic = Some((dodos_abi::CrashReason::CoreSilent as u64, g, 1));
    assert!(!m.harm.accepts(1, accused, panic, m.sys.canonical()));
}

/// `review45_check_r6_proof_line_fires`: a dead generation's line word
/// still taken is refused.
#[test]
fn check_r6_proof_rejects_a_dead_generations_line_word() {
    let mut m = S::two();
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    let g = m.sys.canonical().generation(1);
    m.sys.edit_table(|t| {
        t.lines.insert(5, Ownership::taken(Owner::Node(1), g).raw());
    });
    let err = m.sys.check().expect_err("a dead generation's line word");
    assert!(
        err.ends_with("R6: a dead generation's line word still taken"),
        "{err}"
    );
}

/// Finding 7: a node left fenced for ever, its R6 stopped by a line word
/// nobody will free once R1 has run, fails the liveness of a quiet run.
#[test]
#[should_panic(expected = "R6 held back")]
fn a_stalled_r6_fails_the_quiet_liveness_check() {
    let mut m = three();
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, kill(victim));
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
    super::explore::quiet(&mut m);
}

// ---------------------------------------------------------------------------
// 6. Line controllers other than the IOAPIC (finding 6)
// ---------------------------------------------------------------------------

/// ARM64 boots its line controller as the GIC distributor's role, RISC-V
/// as an APLIC's; its holder frees a line word at a removal and in R1.
#[test]
fn the_gic_distributor_and_aplic_roles_free_line_words() {
    for (arch, governs) in [
        (Arch::Aarch64, dodos_abi::Governs::GicDistributor),
        (Arch::Riscv64, dodos_abi::Governs::Aplic),
    ] {
        let mut m = S::on(&machine(arch, 3, &[&[0], &[1], &[2]]));
        let role = m
            .sys
            .node(0)
            .ioapic_role(5)
            .expect("a controller of line 5");
        assert_eq!(m.sys.canonical().role_records[role].governs, governs);
        let holder = m.sys.role(role).holder();
        let others: Vec<u16> = (0..3).filter(|&n| n != holder).collect();
        let (a, b) = (others[0], others[1]);
        assert!(matches!(
            claim_line(&mut m, a, 5, 210),
            Ok(Output::Msi { .. })
        ));
        remove_210(&mut m, a);
        assert!(!m.sys.canonical().lines.contains_key(&5), "{arch:?}");
        assert!(matches!(
            claim_line(&mut m, a, 5, 210),
            Ok(Output::Msi { .. })
        ));
        m.supervisor(b, m.t[b as usize]);
        m.ok(b, kill(a));
        super::explore::quiet(&mut m);
        assert_eq!(m.state(a), MemberState::Dead, "{arch:?}");
        assert!(!m.sys.canonical().lines.contains_key(&5), "{arch:?}");
    }
}
