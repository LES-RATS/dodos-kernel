//! Seeded explorations: what the two explorers of the contract's research
//! drafts check (`node-protocol-explorer.py`, `kernel-shape-explorer.py`),
//! on the model itself. Each scenario sets up a machine, then interleaves
//! the actions of its nodes and the events of their cores (interrupts,
//! timers, the hardware's actions, the park stub's halt) as a seeded
//! generator chooses them, with the whole check after every step: the
//! properties of the explorers are invariants of `System::check` and of
//! each node's check (ISOLATION is G6, CREDIT, LEAK, RESERVE, ONE-HOLDER,
//! SERVED, RECEIVER, SENDER, P2, P3). Then the machine runs until it is
//! quiet, and the scenario's liveness is asserted: every wait ends, every
//! revocation and every unit request ends, every death is reclaimed.
//!
//! A failure names its seed, and the run replays from it.
//!
//! These scenarios do not depend on the page: `verify` runs them once,
//! without `DODOS_PAGE`, and skips them by name in its runs of 16 KB and
//! 32 KB pages (they pass there too; the audits of lot 8a, F12 and F45:
//! they returned before their first assertion there, reported as passed).

use super::pass17::{fresh, kill, seal};
use super::*;
use crate::Wait;
use crate::system::Rng;
use dodos_abi::NODE_START_RESUME;

const SEEDS: u64 = 24;

/// The seed `DODOS_SEED` names, read once: a value that does not parse is
/// refused, never taken as unset (ver2-D 7).
pub(super) fn replayed() -> Option<u64> {
    let seed = std::env::var("DODOS_SEED").ok()?;
    Some(seed.parse().expect("DODOS_SEED is a seed number"))
}

/// The seeds of a scenario: `DODOS_SEED` replays one, with its actions
/// printed.
pub(super) fn seeds(count: u64) -> Vec<u64> {
    match replayed() {
        Some(seed) => vec![seed],
        None => (0..count).collect(),
    }
}

pub(super) fn say(what: std::fmt::Arguments) {
    if replayed().is_some() {
        eprintln!("{what}");
    }
}

pub(super) fn resume(n: u16) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_RESUME,
        image: 0,
        module: 0,
        window: 0,
        mask: 0,
    }
}

/// A call by a node's first thread if it can run and its node runs; the
/// outcome is not judged (the check after it is).
pub(super) fn try_call(m: &mut S, node: u16, call: Syscall) -> Option<Outcome> {
    if !m.sys.has(node) || m.state(node) != MemberState::Live {
        return None;
    }
    let thread = m.t[node as usize];
    if !m.sys.node(node).runnable().contains(&thread) {
        return None;
    }
    let core = m.sys.node(node).core_of(thread);
    if m.sys.node(node).is_silent(core) {
        return None;
    }
    say(format_args!("{} node {node}: {call:?}", m.sys.now()));
    // A kill or a retire needs a reserve role (`node_stop`, D-150): the
    // node claims one first, as the supervisor that stops a node would.
    if super::stops_with_evidence(&call) {
        m.supervisor(node, thread);
    }
    let outcome = m.call(node, call);
    say(format_args!("  -> {outcome:?}"));
    Some(outcome)
}

/// Events at the current instant, chosen by the generator, then time moves
/// by up to 2 ms.
fn churn(m: &mut S, rng: &mut Rng) {
    let count = rng.below(12);
    m.shuffle(rng, count);
    if rng.below(3) == 0 {
        let ns = 1 + rng.below(2_000_000) as u64;
        say(format_args!("{} run {ns}", m.sys.now()));
        m.run(ns);
    }
}

/// The machine runs until nothing is due for a while.
fn quiet(m: &mut S) {
    m.run_by(200 * MS, MS);
}

/// A node's thread that waits for another node, a unit or its cores.
pub(super) fn waits_bounded(m: &S, node: u16) -> bool {
    m.sys.has(node)
        && m.sys
            .node(node)
            .threads
            .values()
            .any(|t| matches!(t.wait, Wait::Node { .. } | Wait::Unit(_) | Wait::Cores(_)))
}

/// The liveness every exploration ends with, once its machine is quiet
/// (pass 26): no node accused that a harm does not explain (`Harm`), no
/// healthy node whose rebuild report was cut (D-169), and no live node
/// whose thread waits for another node, a unit or its cores.
#[track_caller]
pub(super) fn assert_live(m: &S, seed: u64, nodes: u16) {
    for n in 0..nodes {
        let member = m.sys.member(n);
        let state = member.state();
        let down = matches!(
            state,
            MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced | MemberState::Dead
        );
        let panic = m.sys.canonical().beacons[n as usize].panic;
        assert!(
            !down || m.harm.accepts(n, member, panic),
            "seed {seed}: node {n} accused unharmed: {member:?}, panic {panic:?}, harmed {:?}, sealed {:?}",
            m.harm.harmed,
            m.harm.sealed
        );
        if m.sys.has(n) {
            for &cut in &m.sys.node(n).rebuild_cuts.0 {
                let _ = assert_cut_explained(m, seed, n, cut);
            }
        }
        if state == MemberState::Live && waits_bounded(m, n) {
            let node = m.sys.node(n);
            let waits: Vec<_> = node.threads.values().map(|t| t.wait.clone()).collect();
            panic!(
                "seed {seed}: a wait of node {n} never ended: {waits:?}, requests {:?}, \
                 rebuilds {:?}, handing {:?}, roles {:?}, members {:?}",
                node.unit_requests.iter().flatten().collect::<Vec<_>>(),
                node.rebuilds,
                node.handing,
                (0..2).map(|r| m.sys.role(r)).collect::<Vec<_>>(),
                (0..nodes).map(|n| m.sys.member(n)).collect::<Vec<_>>(),
            );
        }
    }
}

/// D-169 healthy-node guarantee: no healthy node's rebuild report is ever
/// ended by the `STUCK_NS` deadline, by an item check, or as malformed.
/// Returns the `FaultyReport` mode under which the peer handled the ask that
/// produced the cut, if any.
#[track_caller]
pub(super) fn assert_cut_explained(
    m: &S,
    seed: u64,
    holder: u16,
    cut: crate::units::RebuildCutRecord,
) -> Option<crate::units::FaultyReport> {
    let at = (cut.peer, cut.generation);
    let holder_gen = m.sys.node(holder).generation_built();
    let faulty = cut.credit.and_then(|(index, serial)| {
        m.sys
            .canonical()
            .faulty_asks
            .0
            .get(&crate::units::ReportAskId {
                peer: cut.peer,
                peer_gen: cut.generation,
                holder,
                holder_gen,
                index,
                serial,
            })
            .copied()
    });
    let orphan =
        m.harm.starts.get(&at).is_some_and(|starter| {
            m.harm.harmed.contains(starter) || m.harm.sealed.contains(starter)
        });
    let explained = match cut.reason {
        crate::units::RebuildCut::Deadline => {
            faulty == Some(crate::units::FaultyReport::Silent)
                || m.harm.harmed.contains(&at)
                || orphan
        }
        crate::units::RebuildCut::BadItem => faulty == Some(crate::units::FaultyReport::BadItems),
        crate::units::RebuildCut::Malformed => false,
    };
    assert!(
        explained,
        "seed {seed}: healthy node ({}, gen {}) cut on role {} by holder {holder} \
         (gen {holder_gen}, {:?}, credit {:?}) at {} ns; member={:?}, harmed={:?}, sealed={:?}, \
         starts={:?}, faulty={faulty:?}",
        cut.peer,
        cut.generation,
        cut.role,
        cut.reason,
        cut.credit,
        m.sys.now(),
        m.sys.member(cut.peer),
        m.harm.harmed,
        m.harm.sealed,
        m.harm.starts,
    );
    faulty
}

/// A failed unit's role held by a sealing node: the state of gap 21 of
/// pass 26, which a seal now leaves within its step (D-145).
pub(super) fn sealing_holds_a_failed_unit(m: &S) -> bool {
    (0..m.sys.canonical().roles.len()).any(|role| {
        let word = m.sys.role(role);
        word.failed()
            && word.state() == dodos_abi::RoleState::Held
            && m.state(word.holder()) == MemberState::Sealing
    })
}

/// After a death, a node that started again has a new first thread.
pub(super) fn refresh(m: &mut S) {
    for node in 0..m.t.len() as u16 {
        if m.sys.has(node) {
            let first = m
                .sys
                .node(node)
                .object_at_start(dodos_abi::START_SLOT_THREAD);
            if m.sys.node(node).threads.contains_key(&first) {
                m.t[node as usize] = first;
            }
        }
    }
}

/// A `Release` crossing a `Revoke` of one grant: the grant entry of node 0
/// is revoking while node 1's `Release` of it is in the ring to node 0, or
/// pending in its use entry (Grants, "A `Release` crossing a `Revoke`").
fn crossed(m: &S) -> bool {
    let table = m.sys.canonical();
    let revoking = table.grants[0]
        .values()
        .any(|e| e.state == crate::table::GrantState::Revoking);
    let ring = &table.rings[1][0].kernel;
    let start = table.beacons[0].consumed[1].0;
    let release = (start..ring.produced).any(|at| {
        matches!(
            ring.at(at),
            Some(crate::table::RingEntry::Message(crate::rings::Message {
                body: crate::rings::Body::Release { .. },
                ..
            }))
        )
    });
    let pending = table.uses[1].values().any(|u| u.release_pending);
    revoking && (release || pending)
}

/// What the explorations reached, summed over their seeds: each must be
/// reached at least once (the eighth model review found the seeds never
/// reached a full ring, a core stopping in a request or an applied
/// `Handed`, and a crossing once in 132 runs).
fn reached(what: &str, count: usize) {
    if std::env::var("DODOS_COUNTS").is_ok() {
        eprintln!("reached {what}: {count}");
    }
    if replayed().is_none() {
        assert!(count >= 1, "the seeds never reach {what}");
    }
}

// The node-protocol explorer: `crossing`.

#[test]
fn crossing_a_release_and_a_revoke_of_one_grant_in_every_order() {
    let mut crossings = 0;
    for seed in seeds(SEEDS) {
        say(format_args!("seed {seed}"));
        let mut rng = Rng(seed + 1);
        // Node 1 of two cores, so that an unmap there may need the other
        // core's acknowledgement.
        let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
        let reply = m.retype(1, ObjectKind::Reply, 0, 100);
        m.ok(1, reply);
        let frame = m.retype(0, ObjectKind::Frame, 1, 101);
        m.ok(0, frame);
        let recv = Syscall::Recv {
            endpoint: s(dodos_abi::START_SLOT_MAILBOX),
            reply: s(100),
            range: crate::Range {
                first: s(110),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(m.call(1, recv), WAITS);
        let rights = Rights::READ.union(Rights::WRITE).bits();
        let send = Syscall::NodeSend {
            node: node_cap(1),
            message: crate::Message {
                caps: vec![crate::Transfer::Derive(s(101), rights)],
                ..crate::Message::default()
            },
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call(0, send), WAITS);
        m.settle();
        let map = Syscall::ASpaceMap(crate::Map {
            space: s(dodos_abi::START_SLOT_ASPACE),
            frame: s(110),
            untyped: SlotAddr::NONE,
            address: 1 << 30,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        });
        let _ = try_call(&mut m, 1, map);
        for _ in 0..40 {
            match rng.below(6) {
                0 => {
                    let _ = try_call(&mut m, 0, Syscall::CapRevoke(s(101)));
                }
                1 => {
                    let _ = try_call(&mut m, 1, Syscall::CapDelete(s(110)));
                }
                2 => {
                    let _ = try_call(&mut m, 1, Syscall::ASpaceUnmap(s(110)));
                }
                // Steered: the revocation and the release at once, neither
                // node having read the other's message.
                3 => {
                    let _ = try_call(&mut m, 0, Syscall::CapRevoke(s(101)));
                    let _ = try_call(&mut m, 1, Syscall::CapDelete(s(110)));
                }
                _ => churn(&mut m, &mut rng),
            }
            if crossed(&m) {
                crossings += 1;
                break;
            }
        }
        // Liveness: every revocation ends; the machine is quiet.
        for _ in 0..3 {
            quiet(&mut m);
            let _ = try_call(&mut m, 0, Syscall::CapRevoke(s(101)));
        }
        assert!(
            !waits_bounded(&m, 0),
            "seed {seed}: a revocation never ended"
        );
        assert!(
            m.sys.node(0).grant_entries().is_empty(),
            "seed {seed}: the grant outlived its revocation"
        );
        assert_live(&m, seed, 2);
    }
    reached("a Release crossing a Revoke", crossings);
}

// D-142: a granted frame mapped in an iospace whose unit fails, at any
// point of a map, an unmap, a removal by the holder, a revocation and the
// granting node's death.

/// Whether the holder (node 2) counts a leaf under a grant of node 0 while
/// the IOMMU unit has failed: the state D-142 is about.
fn failed_under_a_remote_frame(m: &S) -> bool {
    let failed = m.sys.role(super::units::IOMMU_ROLE).failed();
    failed
        && m.sys.canonical().uses[2]
            .values()
            .any(|u| u.granter == 0 && u.leaves > 0)
}

/// Whether the holder (node 2) resumes with a record under node 0's grant
/// unmapping: its R2 waits for the unit, whose role node 1 holds.
fn resume_asks_a_unit(m: &S) -> bool {
    m.sys.has(2)
        && m.state(2) == MemberState::Starting
        && m.sys.node(2).caps.values().any(|c| {
            matches!(c.record, crate::Record::Unmapping(_))
                && m.sys
                    .node(2)
                    .remotes
                    .get(&c.object)
                    .is_some_and(|r| r.node == 0)
        })
}

#[test]
fn a_unit_fails_under_a_granted_frame_at_any_step_of_its_use() {
    let (mut reached_failed, mut pinned, mut resumed_kept) = (0, 0, 0);
    let mut resume_asked_unit = 0;
    for seed in seeds(SEEDS) {
        say(format_args!("seed {seed}"));
        let mut rng = Rng(seed + 401);
        // Node 1 holds the unit's role; node 2 holds the grant and maps it
        // into an iospace with a device behind the unit.
        let mut m = super::units::three();
        super::units::attached(&mut m, 2);
        let reply = m.retype(2, ObjectKind::Reply, 0, 100);
        m.ok(2, reply);
        let frame = m.retype(0, ObjectKind::Frame, 1, 101);
        m.ok(0, frame);
        let recv = Syscall::Recv {
            endpoint: s(dodos_abi::START_SLOT_MAILBOX),
            reply: s(100),
            range: crate::Range {
                first: s(110),
                count: 1,
            },
            wait: 0,
        };
        assert_eq!(m.call(2, recv), WAITS);
        let send = Syscall::NodeSend {
            node: node_cap(2),
            message: crate::Message {
                caps: vec![crate::Transfer::Derive(s(101), Rights::READ.bits())],
                ..crate::Message::default()
            },
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call(0, send), WAITS);
        m.settle();
        let ut = super::table_slot(m.sys.node(2), m.t[2]);
        let map = Syscall::IoSpaceMap(crate::Map {
            space: s(super::units::IOSPACE),
            frame: s(110),
            untyped: s(ut),
            address: 0x1000_0000,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        });
        // Steered: mapped first, so that most seeds fail the unit under a
        // leaf.
        let _ = try_call(&mut m, 2, map.clone());
        let (mut broken, mut killed, mut seen) = (false, false, false);
        let (mut resumed, mut asked) = (false, false);
        // Steered, one seed in three: the holder sealed over its mapping,
        // the granting node killed, then the holder resumed, so that R2 of
        // the resume asks node 1's unit, then the random actions.
        if seed % 3 == 0 {
            let _ = try_call(&mut m, 1, seal(2));
            m.run(50 * MS);
            killed = try_call(&mut m, 1, kill(0)) == Some(DONE);
            m.run(dodos_abi::FENCE_NS + MS);
            if m.state(2) == MemberState::Sealed {
                resumed |= try_call(&mut m, 1, resume(2)) == Some(DONE);
            }
        }
        for _ in 0..40 {
            match rng.below(11) {
                0 => {
                    let _ = try_call(&mut m, 2, map.clone());
                }
                1 => {
                    let _ = try_call(&mut m, 2, Syscall::IoSpaceUnmap(s(110)));
                }
                2 => {
                    let _ = try_call(&mut m, 2, Syscall::CapDelete(s(110)));
                }
                3 => {
                    let _ = try_call(&mut m, 0, Syscall::CapRevoke(s(101)));
                }
                4 if !broken => {
                    say(format_args!("{} unit fails", m.sys.now()));
                    m.sys.break_unit(super::units::IOMMU_ROLE);
                    broken = true;
                }
                5 if !killed && rng.below(4) == 0 => {
                    killed = try_call(&mut m, 1, kill(0)) == Some(DONE);
                }
                // The holder sealed and resumed by node 1 (Grants,
                // "Generations": a resume over a generation that ended).
                6 => {
                    let _ = try_call(&mut m, 1, seal(2));
                }
                7 if m.state(2) == MemberState::Sealed => {
                    resumed |= try_call(&mut m, 1, resume(2)) == Some(DONE);
                }
                _ => churn(&mut m, &mut rng),
            }
            seen |= failed_under_a_remote_frame(&m);
            asked |= resume_asks_a_unit(&m);
        }
        for _ in 0..3 {
            quiet(&mut m);
            let _ = try_call(&mut m, 0, Syscall::CapRevoke(s(101)));
        }
        if m.state(2) == MemberState::Sealed {
            resumed |= try_call(&mut m, 1, resume(2)) == Some(DONE);
        }
        if !asked {
            asked = m.step_until(|m| resume_asks_a_unit(m) || m.state(2) != MemberState::Starting)
                && resume_asks_a_unit(&m);
        }
        quiet(&mut m);
        seen |= failed_under_a_remote_frame(&m);
        reached_failed += usize::from(seen);
        resume_asked_unit += usize::from(asked);
        // A resume reads its rings from the end of R0 on, so the answer of
        // the unit R2 asked comes and the holder is never accused stuck
        // (What crosses between nodes, "At a resume", D-142).
        assert_ne!(
            m.sys.member(2).evidence(),
            dodos_abi::Evidence::Stuck,
            "seed {seed}: the holder was accused stuck"
        );
        // A count the failed unit kept: the grant is pinned while the
        // granting node lives, and a dead granting node never is (C1, C2).
        // Otherwise every revocation ends and a death is reclaimed.
        let kept = m.sys.canonical().uses[2]
            .values()
            .any(|u| u.granter == 0 && u.leaves > 0 && u.root.is_none());
        let state = m.state(0);
        // A resume does not wait for an R2 that only the kept count leaves
        // undone (D-142): with no remote capability of node 0 left, the
        // holder is live again.
        let accused = matches!(state, MemberState::Fenced | MemberState::Unfenced);
        let removed = m.sys.has(2) && !m.sys.node(2).remotes.values().any(|r| r.node == 0);
        if kept && accused && resumed && removed {
            assert_eq!(
                m.state(2),
                MemberState::Live,
                "seed {seed}: the resume waited"
            );
            resumed_kept += 1;
        }
        if kept {
            assert_ne!(state, MemberState::Dead, "seed {seed}: C2");
            if state == MemberState::Live {
                let entries = &m.sys.canonical().grants[0];
                assert!(
                    entries
                        .values()
                        .all(|e| e.state == crate::table::GrantState::Pinned),
                    "seed {seed}: C1, {entries:?}"
                );
                pinned += usize::from(!entries.is_empty());
            }
        } else if killed && matches!(m.state(2), MemberState::Live | MemberState::Dead) {
            assert_eq!(state, MemberState::Dead, "seed {seed}: never reclaimed");
        }
        if state == MemberState::Live {
            assert!(
                !waits_bounded(&m, 0),
                "seed {seed}: a revocation never ended"
            );
        }
        assert_live(&m, seed, 3);
    }
    reached("a failed unit under a remote frame", reached_failed);
    reached("a grant pinned by a failed unit", pinned);
    reached(
        "a holder resumed beside a dead granter's kept count",
        resumed_kept,
    );
    reached(
        "a resume whose R2 asks another node's unit",
        resume_asked_unit,
    );
}

// The node-protocol explorer: `unit`.

#[test]
fn unit_a_request_across_a_hand_over_and_the_deaths_of_its_parties() {
    let mut handed = 0;
    for seed in seeds(SEEDS) {
        let mut rng = Rng(seed + 101);
        let mut handed_here = 0;
        // Node 0 (class 1) requests; nodes 1, 2 and 3 (class 2) may hold
        // the IOMMU role.
        let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
        let iospace = m.retype(0, ObjectKind::IoSpace, 0, 150);
        m.ok(0, iospace);
        let iommu = s(slots_of(m.sys.node(0), m.t[0], ObjectKind::Iommu)[0]);
        let attach = Syscall::IoSpaceAttach {
            iommu,
            iospace: s(150),
            device: 0x10,
        };
        // A thread of its own asks; the first thread of node 0 kills.
        m.ok(0, super::place_first(0, 0));
        let requester = m.thread_on(0, 300, 0);
        assert_eq!(m.call_by(0, requester, attach), WAITS);
        let mut dead = 0;
        for _ in 0..40 {
            match rng.below(8) {
                0 => {
                    let holder = m.sys.role(0).holder();
                    let target = 1 + rng.below(3) as u16;
                    if m.sys.has(holder) {
                        let role = s(slots_of(
                            m.sys.node(holder),
                            m.t[holder as usize],
                            ObjectKind::Role,
                        )[0]);
                        let hand = Syscall::RoleHand {
                            role,
                            node: node_cap(target),
                        };
                        let _ = try_call(&mut m, holder, hand);
                    }
                }
                1 if dead < 2 => {
                    // A death of the holder or of the target of a handing
                    // word, by node 0.
                    let word = m.sys.role(0);
                    let victim = match rng.below(2) {
                        0 => word.holder(),
                        _ => word.target(),
                    };
                    if (1..4).contains(&victim) && try_call(&mut m, 0, kill(victim)) == Some(DONE) {
                        dead += 1;
                    }
                }
                _ => churn(&mut m, &mut rng),
            }
            handed_here = handed_here.max(applied_handed(&m));
        }
        quiet(&mut m);
        quiet(&mut m);
        handed += handed_here.max(applied_handed(&m));
        // Liveness: the request ended done, and the unit's context entry
        // names node 0's iospace.
        assert_eq!(m.sys.node(0).woken(requester), Some(&DONE), "seed {seed}");
        assert!(
            m.sys.node(0).unit_requests.iter().all(Option::is_none),
            "seed {seed}"
        );
        let unit = &m.sys.canonical().units[&0];
        assert!(unit.entries.contains_key(&0x10), "seed {seed}");
        assert_live(&m, seed, 4);
    }
    reached("a Handed applied", handed);
}

/// The `Handed`s applied by the nodes that run.
fn applied_handed(m: &S) -> usize {
    (0..m.t.len() as u16)
        .filter(|&n| m.sys.has(n))
        .map(|n| *m.sys.node(n).handed_applied as usize)
        .sum()
}

// The node-protocol explorer: `rings`.

#[test]
fn rings_a_send_and_a_signal_while_the_sender_seals_and_the_receiver_dies_and_starts_fresh() {
    for seed in seeds(SEEDS) {
        let mut rng = Rng(seed + 201);
        let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
        // Node 2 supervises: it seals, resumes, kills and starts the others.
        let reply = m.retype(1, ObjectKind::Reply, 0, 100);
        m.ok(1, reply);
        let frame = m.retype(0, ObjectKind::Frame, 1, 101);
        m.ok(0, frame);
        let bell = m.retype(0, ObjectKind::Notify, 0, 102);
        m.ok(0, bell);
        let recv = Syscall::Recv {
            endpoint: s(dodos_abi::START_SLOT_MAILBOX),
            reply: s(100),
            range: crate::Range {
                first: s(110),
                count: 1,
            },
            wait: 0,
        };
        let _ = try_call(&mut m, 1, recv.clone());
        let rights = Rights::READ.union(Rights::SEND).bits();
        let send = Syscall::NodeSend {
            node: node_cap(1),
            message: crate::Message {
                caps: vec![crate::Transfer::Derive(s(101), rights)],
                ..crate::Message::default()
            },
            doorbell: s(102),
        };
        let _ = try_call(&mut m, 0, send);
        for _ in 0..40 {
            match rng.below(10) {
                0 => {
                    let _ = try_call(&mut m, 2, seal(0));
                }
                1 => {
                    let _ = try_call(&mut m, 2, resume(0));
                }
                2 => {
                    let _ = try_call(&mut m, 2, kill(1));
                }
                3 => {
                    if try_call(&mut m, 2, fresh(1, 0b10)) == Some(DONE) {
                        m.run(3 * MS);
                        refresh(&mut m);
                        let reply = m.retype(1, ObjectKind::Reply, 0, 100);
                        let _ = try_call(&mut m, 1, reply);
                    }
                }
                4 => {
                    let _ = try_call(&mut m, 1, Syscall::GrantSignal(s(110)));
                }
                5 => {
                    let _ = try_call(&mut m, 1, recv.clone());
                }
                _ => churn(&mut m, &mut rng),
            }
        }
        if m.state(0) == MemberState::Sealed {
            let _ = try_call(&mut m, 2, resume(0));
        }
        quiet(&mut m);
        // Liveness: every wait of node 0 for node 1 ended, by the answer or
        // by `gone`.
        if m.sys.has(0) && m.state(0) == MemberState::Live {
            assert!(!waits_bounded(&m, 0), "seed {seed}: a wait never ended");
        }
        assert_live(&m, seed, 3);
    }
}

// The node-protocol explorer: `waiter`.

#[test]
fn waiter_threads_that_wait_for_a_send_answer_are_destroyed_and_the_receiver_dies() {
    // What the seeds reach (the audit of lot 8a, F12: nothing showed a send
    // in flight when node 1 died, or a waiting sender destroyed).
    let (mut in_flight_at_death, mut waiter_destroyed) = (0, 0);
    for seed in seeds(SEEDS) {
        let mut rng = Rng(seed + 301);
        let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
        m.ok(0, super::place_first(0, 0));
        let senders: Vec<(u32, ObjId)> = (0..3)
            .map(|i| (300 + 4 * i, m.thread_on(0, 300 + 4 * i, 0)))
            .collect();
        let frames: Vec<u32> = (0..3).map(|i| 200 + i).collect();
        for &f in &frames {
            let frame = m.retype(0, ObjectKind::Frame, 1, f);
            m.ok(0, frame);
        }
        for _ in 0..40 {
            match rng.below(8) {
                0 => {
                    let k = rng.below(3);
                    let (slot, thread) = senders[k];
                    let ready = m.sys.has(0)
                        && m.sys.node(0).threads.contains_key(&thread)
                        && m.sys.node(0).runnable().contains(&thread);
                    if ready && m.state(0) == MemberState::Live {
                        let send = Syscall::NodeSend {
                            node: node_cap(1),
                            message: crate::Message {
                                caps: vec![crate::Transfer::Derive(
                                    s(frames[k]),
                                    Rights::READ.bits(),
                                )],
                                ..crate::Message::default()
                            },
                            doorbell: SlotAddr::NONE,
                        };
                        // Steered: one time in two, the sender is destroyed
                        // while it waits for the answer.
                        if m.call_by(0, thread, send) == WAITS && rng.below(2) == 0 {
                            let deleted = try_call(&mut m, 0, Syscall::CapDelete(s(slot)));
                            waiter_destroyed += usize::from(deleted == Some(DONE));
                        }
                    }
                }
                1 => {
                    let (slot, thread) = senders[rng.below(3)];
                    let waiting = m.sys.has(0)
                        && m.sys.node(0).threads.contains_key(&thread)
                        && !m.sys.node(0).runnable().contains(&thread);
                    let deleted = try_call(&mut m, 0, Syscall::CapDelete(s(slot)));
                    waiter_destroyed += usize::from(waiting && deleted == Some(DONE));
                }
                2 => {
                    let sends = m.sys.has(0) && m.sys.node(0).peers[1].sends() > 0;
                    let killed = try_call(&mut m, 2, kill(1)) == Some(DONE);
                    in_flight_at_death += usize::from(sends && killed);
                }
                _ => churn(&mut m, &mut rng),
            }
        }
        quiet(&mut m);
        // Liveness: no thread of node 0 waits for node 1, and no credit of
        // node 0 toward node 1 is in use.
        assert!(!waits_bounded(&m, 0), "seed {seed}");
        assert_eq!(m.sys.node(0).peers[1].sends(), 0, "seed {seed}");
        assert_live(&m, seed, 3);
    }
    reached(
        "a send in flight when its receiver is killed",
        in_flight_at_death,
    );
    reached("a waiting sender destroyed", waiter_destroyed);
}

// The kernel-shape explorer: the fence of a node of several cores, the
// bootstrap core, seals and resumes, fresh starts and the barrier.

#[test]
fn deaths_seals_resumes_and_fresh_starts_of_nodes_of_several_cores() {
    // What the seeds reach (the audit of lot 8a, F12): each kind of step
    // done at least once, and the end states the liveness judges.
    let mut done = [0usize; 4];
    let (mut hangs, mut unfenced, mut dead) = (0, 0, 0);
    for seed in seeds(SEEDS) {
        let mut rng = Rng(seed + 401);
        let arch = [Arch::X86_64, Arch::Aarch64, Arch::Riscv64][seed as usize % 3];
        let mut m = S::on(&machine(arch, 5, &[&[0, 1], &[2, 3], &[4]]));
        let mut hung = false;
        // A hung core that no stop of its architecture reaches: the x86-64
        // bootstrap core, or an ARM64 core, whose handler cannot run.
        let mut unstoppable = false;
        for _ in 0..30 {
            let actor = rng.below(3) as u16;
            let victim = (actor + 1 + rng.below(2) as u16) % 3;
            match rng.below(9) {
                0 => {
                    done[0] += usize::from(try_call(&mut m, actor, kill(victim)) == Some(DONE));
                }
                1 => {
                    done[1] += usize::from(try_call(&mut m, actor, seal(victim)) == Some(DONE));
                }
                2 => {
                    done[2] += usize::from(try_call(&mut m, actor, resume(victim)) == Some(DONE));
                }
                3 => {
                    let mask = match victim {
                        0 => 0b11,
                        1 => 0b1100,
                        _ => 0b10000,
                    };
                    if try_call(&mut m, actor, fresh(victim, mask)) == Some(DONE) {
                        done[3] += 1;
                        m.run(3 * MS);
                        refresh(&mut m);
                    }
                }
                4 if rng.below(4) == 0 && !hung => {
                    // A core hangs, able or not to run its handler: once
                    // per seed (two silent nodes and a killed one leave no
                    // node to recover them, a lost machine).
                    let core = rng.below(5);
                    let word = m.sys.core_word(core);
                    if dodos_abi::CoreWord::from_raw(word)
                        .is_some_and(|w| w.state() == dodos_abi::CoreState::Owned)
                        && m.sys.hardware(core) == crate::system::Hardware::Running
                    {
                        let nmi = rng.below(2) == 0;
                        say(format_args!("{} hang core {core} nmi {nmi}", m.sys.now()));
                        m.hang(core, nmi);
                        hung = true;
                        hangs += 1;
                        unstoppable =
                            !nmi && (arch == Arch::Aarch64 || (arch == Arch::X86_64 && core == 0));
                    }
                }
                _ => churn(&mut m, &mut rng),
            }
        }
        m.run_by(dodos_abi::SWEEP_NS + 300 * MS, MS);
        // Liveness: every node that was accused ended dead or unfenced; at
        // least one node is live.
        let live = (0..3).filter(|&n| m.state(n) == MemberState::Live).count();
        assert!(live >= 1, "seed {seed}: no node is live");
        say(format_args!(
            "members {:?} roles {:?} acked {:?} hw {:?}",
            (0..3).map(|n| m.sys.member(n)).collect::<Vec<_>>(),
            (0..2).map(|r| m.sys.role(r)).collect::<Vec<_>>(),
            m.sys
                .canonical()
                .beacons
                .iter()
                .map(|b| b.acked.clone())
                .collect::<Vec<_>>(),
            (0..5).map(|c| m.sys.hardware(c)).collect::<Vec<_>>()
        ));
        for n in 0..3 {
            let state = m.state(n);
            assert!(
                !matches!(state, MemberState::Fencing | MemberState::Fenced),
                "seed {seed}: node {n} stays {state:?}"
            );
            // Unfenced only for a core that no stop reaches.
            assert!(
                state != MemberState::Unfenced || unstoppable,
                "seed {seed}: node {n} unfenced with every core stoppable"
            );
            unfenced += usize::from(state == MemberState::Unfenced);
            dead += usize::from(state == MemberState::Dead);
        }
        assert_live(&m, seed, 3);
    }
    for (what, count) in ["a kill", "a seal", "a resume", "a fresh start"]
        .into_iter()
        .zip(done)
    {
        reached(what, count);
    }
    reached("a hung core", hangs);
    reached("a node left unfenced", unfenced);
    reached("a node left dead", dead);
}

// The kernel-shape explorer: `credits`.

#[test]
fn credits_user_and_kernel_messages_with_receivers_that_never_read() {
    let mut full = 0;
    for seed in seeds(SEEDS / 2) {
        let mut rng = Rng(seed + 501);
        let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
        // Steered: in half of the seeds node 1's core stops answering, so
        // that its user ring fills until a send no longer fits.
        if seed % 2 == 0 {
            m.hang(1, true);
        }
        m.ok(0, super::place_first(0, 0));
        let senders: Vec<ObjId> = (0..10).map(|i| m.thread_on(0, 300 + 4 * i, 0)).collect();
        let frame = m.retype(0, ObjectKind::Frame, 1, 200);
        m.ok(0, frame);
        for _ in 0..60 {
            match rng.below(6) {
                0 | 1 => {
                    let thread = senders[rng.below(senders.len())];
                    if m.sys.has(0)
                        && m.state(0) == MemberState::Live
                        && m.sys.node(0).runnable().contains(&thread)
                    {
                        // Messages of 60 words: 9 entries each (⌈(16 + 8 ×
                        // 63) / 64⌉), so the 64 entries of the ring fill
                        // before the 8 sends in flight are used: the
                        // seventh leaves one entry, and the next is `busy`.
                        let send = Syscall::NodeSend {
                            node: node_cap(1),
                            message: crate::Message {
                                words: vec![7; 60],
                                ..crate::Message::default()
                            },
                            doorbell: SlotAddr::NONE,
                        };
                        let outcome = m.call_by(0, thread, send);
                        // A send refused by the full ring (the ninth model
                        // review, finding 12: the room after any send was
                        // counted, `busy` or not).
                        if outcome == Err(Error::Busy)
                            && m.sys.has(0)
                            && !m.sys.node(0).ring_room(1, dodos_abi::NodeMessage::Send, 9)
                        {
                            full += 1;
                        }
                        assert!(
                            matches!(
                                outcome,
                                Ok(Output::Waits) | Err(Error::Busy) | Err(Error::Gone)
                            ),
                            "seed {seed}: {outcome:?}"
                        );
                    }
                }
                2 if rng.below(5) == 0 => {
                    let _ = try_call(&mut m, 2, kill(1));
                }
                3 if rng.below(5) == 0 => {
                    if try_call(&mut m, 2, fresh(1, 0b10)) == Some(DONE) {
                        m.run(3 * MS);
                        refresh(&mut m);
                    }
                }
                _ => churn(&mut m, &mut rng),
            }
        }
        quiet(&mut m);
        assert!(!waits_bounded(&m, 0), "seed {seed}");
        assert_live(&m, seed, 3);
    }
    reached("a full ring", full);
}

// A core of a node stops in the middle of a request of another core: the
// request waits for it, and the node accuses itself after
// `CORE_ANSWER_NS` (TLB, a core that does not acknowledge; A node of
// several cores).

#[test]
fn a_core_stops_in_the_middle_of_a_request_of_another_core() {
    let mut stopped_in_request = 0;
    for seed in seeds(SEEDS / 2) {
        let mut rng = Rng(seed + 601);
        // Node 1 of cores 1 and 2: a thread on core 2 has its space loaded;
        // node 0 supervises.
        let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
        m.ok(1, super::place_first(1, 1));
        let _ = m.thread_on(1, 300, 2);
        m.settle();
        let frame = m.retype(1, ObjectKind::Frame, 1, 150);
        m.ok(1, frame);
        let map = Syscall::ASpaceMap(crate::Map {
            space: s(dodos_abi::START_SLOT_ASPACE),
            frame: s(150),
            untyped: SlotAddr::NONE,
            address: 1 << 30,
            first: 0,
            count: 1,
            rights: Rights::READ.union(Rights::WRITE).bits(),
        });
        let _ = try_call(&mut m, 1, map);
        let mut hung = false;
        for _ in 0..30 {
            match rng.below(6) {
                0 => {
                    let _ = try_call(&mut m, 1, Syscall::ASpaceUnmap(s(150)));
                }
                // Steered: core 2 stops while a request of core 1 asks it.
                1 | 2 if !hung => {
                    let asks = m.sys.has(1)
                        && m.sys.node(1).cores.iter().any(|c| {
                            c.request
                                .as_ref()
                                .is_some_and(|r| r.asked & 0b10 != 0 && r.acked & 0b10 == 0)
                        });
                    if asks {
                        m.hang(2, rng.below(2) == 0);
                        hung = true;
                        stopped_in_request += 1;
                    }
                }
                3 => {
                    let _ = try_call(&mut m, 1, map_again());
                }
                _ => churn(&mut m, &mut rng),
            }
        }
        m.run_by(dodos_abi::CORE_ANSWER_NS + 100 * MS, MS);
        // Liveness: a node whose core stopped in a request accused itself
        // and was reclaimed, or the request completed.
        if hung {
            assert_ne!(m.state(1), MemberState::Live, "seed {seed}");
        }
        assert!(
            !m.sys.has(1)
                || m.sys.node(1).cores.iter().all(|c| c.request.is_none())
                || m.state(1) != MemberState::Live,
            "seed {seed}"
        );
        assert_live(&m, seed, 2);
    }
    reached(
        "a core stopping in the middle of a request",
        stopped_in_request,
    );
}

fn map_again() -> Syscall {
    Syscall::ASpaceMap(crate::Map {
        space: s(dodos_abi::START_SLOT_ASPACE),
        frame: s(150),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.union(Rights::WRITE).bits(),
    })
}
