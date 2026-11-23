//! Pass 45: review of pass 44 (R6 line words, capped timer priority,
//! checker blind spots, D-171 Harm::accepts, and recovery counters).

use super::pass17::kill;
use super::*;
use dodos_abi::{MemberState, Owner, Ownership};

// ---------------------------------------------------------------------------
// 1. R6 and line words (finding 6, D-150)
// ---------------------------------------------------------------------------

#[test]
fn r6_stops_at_line_word_naming_dead_generation() {
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, kill(victim));
    // Wait until the IOAPIC holder has already run R1 for the fencing victim,
    // before the victim is fenced and R6 can start.
    assert!(m.step_until(|m| {
        m.sys
            .node(holder)
            .reclaims
            .get(&victim)
            .is_some_and(|r| r.r1.contains_key(&ioapic))
    }));
    assert_eq!(m.state(victim), MemberState::Fencing);
    let gv = m.sys.canonical().generation(victim);
    m.sys.edit_table(|t| {
        let taken = Ownership::taken(Owner::Node(victim), gv).raw();
        t.lines.insert(9, taken);
    });
    assert!(m.sys.node(holder).r6_held_back(victim, gv));
    m.run(40 * MS);
    assert_eq!(m.state(victim), MemberState::Fenced);
    assert!(m.sys.canonical().lines.contains_key(&9));
}

#[test]
fn checker_rejects_r6_write_of_line_word() {
    // Even the controller holder cannot write free a line word during R6
    // (after R5 has acknowledged the dead generation), nor can a non-holder.
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let victim = (0..3u16).find(|&n| n != holder).unwrap();
    let non_holder = (0..3u16).find(|&n| n != holder && n != victim).unwrap();
    m.supervisor(holder, m.t[holder as usize]);
    m.ok(holder, kill(victim));
    assert!(m.step_until(|m| {
        m.sys
            .node(holder)
            .reclaims
            .get(&victim)
            .is_some_and(|r| r.r1.contains_key(&ioapic))
    }));
    let gv = m.sys.canonical().generation(victim);
    m.sys.edit_table(|t| {
        let taken = Ownership::taken(Owner::Node(victim), gv).raw();
        t.lines.insert(9, taken);
    });
    // Wait until victim is Fenced and all live nodes have acknowledged it (R5).
    m.run(40 * MS);
    assert_eq!(m.state(victim), MemberState::Fenced);
    assert_eq!(
        m.sys.canonical().beacons[holder as usize].acked[victim as usize],
        gv
    );
    assert_eq!(
        m.sys.canonical().beacons[non_holder as usize].acked[victim as usize],
        gv
    );

    let taken = Ownership::taken(Owner::Node(victim), gv).raw();
    let unclaimed = Ownership::free(Owner::Unclaimed).raw();

    // Case 1: controller holder journals an R6 free of line 9 after R5 ack.
    let mut sys_holder = m.sys.clone();
    sys_holder.edit_table(|t| {
        let write = t.journal(holder, crate::table::Word::Line(9), taken, unclaimed);
        t.lines.remove(&9);
        t.writes.push(write);
    });
    assert_eq!(
        sys_holder.node(holder).check(),
        Err("52: write of an ownership word")
    );
    assert!(
        sys_holder
            .check()
            .is_err_and(|e| e.ends_with("52: write of an ownership word"))
    );

    // Case 2: non-holder journals an R6 free of line 9.
    let mut sys_other = m.sys.clone();
    sys_other.edit_table(|t| {
        let write = t.journal(non_holder, crate::table::Word::Line(9), taken, unclaimed);
        t.lines.remove(&9);
        t.writes.push(write);
    });
    assert_eq!(
        sys_other.node(non_holder).check(),
        Err("52: write of an ownership word")
    );
    assert!(
        sys_other
            .check()
            .is_err_and(|e| e.ends_with("52: write of an ownership word"))
    );
}

#[test]
fn r1_controller_holder_frees_line_word_on_holder_death_and_takeover() {
    // When the IOAPIC holder itself claims a line and is killed, the successor
    // takes over the IOAPIC role, runs R1 in `after_take`, frees the line word,
    // and R6 then transitions the dead holder to Dead.
    let mut m = super::units::three();
    let ioapic = super::units::IOAPIC_ROLE;
    let holder = m.sys.role(ioapic).holder();
    let killer = (0..3u16).find(|&n| n != holder).unwrap();
    assert!(matches!(
        super::pass18::claim_line(&mut m, holder, 5, 210),
        Ok(Output::Msi { .. })
    ));
    assert!(m.sys.canonical().lines.contains_key(&5));

    m.supervisor(killer, m.t[killer as usize]);
    m.ok(killer, kill(holder));
    m.run(60 * MS);
    assert_eq!(m.state(holder), MemberState::Dead);
    assert!(!m.sys.canonical().lines.contains_key(&5));
}
