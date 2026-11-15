//! Liveness of recovery (8.4), by bounded exploration: in each scenario a
//! node is stopped (or sealed, or hands a unit role over, or waits for a
//! unit's holder), and at every step of what follows another node crashes;
//! with fair runs of the nodes that remain, every crashed node becomes
//! dead, every unit role is held by a live node, a live node remains, and
//! every thread that waited for a unit's holder got its answer or `gone`.
//! Every invariant is checked after every step. At the end every state of a
//! member word, a role word and a grant entry has a way out: no node is
//! left fencing, fenced or unfenced, no role names a dead node, and no
//! grant is pinned or names a generation that is over (the states the sixth
//! review found stuck).
//!
//! The crashed node hangs with its NMI handler still able to run, so that
//! its stop succeeds (the fairness assumption of 8.4). The times are the
//! shortest the boot image may state: `STALE_NS` and `STUCK_NS` 10 ms,
//! `SWEEP_NS` 100 ms.

use dodos_abi::*;
use dodos_model::*;

const MS: u64 = 1_000_000;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

/// Three nodes: node 0 on the bootstrap core (class 1), nodes 1 and 2 on
/// cores of class 2, which hold the two unit roles.
fn machine() -> Machine {
    let mut machine = Machine::example();
    let mut third = machine.cores[1];
    third.hw_id = 4;
    machine.cores.push(third);
    machine.membership.stale_ms = 10;
    machine.membership.stuck_ms = 10;
    machine.membership.sweep_ms = 100;
    machine
}

/// One step of the machine: a unit of some node's recovery reserve, else a
/// message, else 100 µs of time.
fn step(sys: &mut System) {
    if sys.recover(1) == 0 && sys.settle(1) == 0 {
        let now = sys.now();
        sys.advance(now + MS / 10);
    }
    assert_eq!(sys.check(), Ok(()));
}

/// What a scenario starts with, and the nodes it stops.
struct Scenario {
    name: &'static str,
    begin: fn(&mut System, &[ObjId]),
    stopped: &'static [u16],
    /// The node whose thread waits for a unit's holder, if any.
    waiting: Option<u16>,
}

fn kill(sys: &mut System, t: &[ObjId]) {
    let call = Syscall::CoreStop {
        core: core(2),
        mode: CORE_STOP_KILL,
    };
    assert_eq!(sys.apply(0, t[0], call), Ok(Output::Done));
}

fn seal(sys: &mut System, t: &[ObjId]) {
    let call = Syscall::CoreStop {
        core: core(1),
        mode: CORE_STOP_SEAL,
    };
    assert_eq!(sys.apply(0, t[0], call), Ok(Output::Done));
}

fn hand(sys: &mut System, t: &[ObjId]) {
    // Unit role 0 (the IOMMU unit, slot 20 with three `core` roots) is
    // node 1's: it hands it to node 2.
    let call = Syscall::RoleHand {
        role: s(20),
        core: core(2),
    };
    let outcome = sys.apply(1, t[1], call);
    assert!(matches!(outcome, Ok(Output::Epoch(_))), "{outcome:?}");
}

fn attach(sys: &mut System, t: &[ObjId]) {
    // Node 0 asks node 1, holder of the IOMMU unit's role, for an entry.
    let make = Syscall::UntypedRetype {
        untyped: s(25),
        kind: ObjectKind::IoSpace,
        size: 0,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(100),
    };
    assert_eq!(sys.apply(0, t[0], make), Ok(Output::Done));
    let call = Syscall::IoSpaceAttach {
        iommu: s(19),
        iospace: s(100),
        device: 0x10,
    };
    assert_eq!(sys.apply(0, t[0], call), Ok(Output::Waits));
}

fn grant(sys: &mut System, t: &[ObjId]) {
    // Node 0 grants a frame to node 2, which maps it.
    let make = Syscall::UntypedRetype {
        untyped: s(25),
        kind: ObjectKind::Frame,
        size: 1,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(100),
    };
    assert_eq!(sys.apply(0, t[0], make), Ok(Output::Done));
    let reply = Syscall::UntypedRetype {
        untyped: s(25),
        kind: ObjectKind::Reply,
        size: 0,
        page: 0,
        memory: 0,
        destination: s(99),
    };
    assert_eq!(sys.apply(2, t[2], reply), Ok(Output::Done));
    let recv = Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(99),
        range: Range {
            first: s(110),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(sys.apply(2, t[2], recv), Ok(Output::Waits));
    let send = Syscall::Send {
        target: core(2),
        message: Message {
            label: 1,
            flags: 0,
            words: vec![1],
            caps: vec![Transfer::Derive(s(100), Rights::READ.bits())],
            marks: 0,
        },
    };
    assert_eq!(sys.apply(0, t[0], send), Ok(Output::Waits));
    sys.settle(32);
    let map = Syscall::ASpaceMap(Map {
        space: s(START_SLOT_ASPACE),
        frame: s(110),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    assert_eq!(sys.apply(2, t[2], map), Ok(Output::Done));
}

fn reserve(sys: &mut System, t: &[ObjId]) {
    // Node 2 claims a reserve role and takes a chunk of its reserve.
    let claim = Syscall::RoleClaim(s(22));
    assert!(matches!(sys.apply(2, t[2], claim), Ok(Output::Epoch(_))));
    let shift = sys.node(2).geometry().chunk_shift();
    let base = (0..1u64 << 20)
        .map(|c| c << shift)
        .find(|&at| sys.node(2).ownership(at >> shift) == Ownership::free(Owner::Reserve(0)).raw())
        .expect("a chunk of reserve 0");
    let take = Syscall::MemoryTake {
        control: s(START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: base,
        size: u64::from(shift),
        destination: s(120),
    };
    assert_eq!(sys.apply(2, t[2], take), Ok(Output::Done));
}

const SCENARIOS: [Scenario; 6] = [
    Scenario {
        name: "grant",
        begin: grant,
        stopped: &[],
        waiting: None,
    },
    Scenario {
        name: "reserve",
        begin: reserve,
        stopped: &[],
        waiting: None,
    },
    Scenario {
        name: "kill",
        begin: kill,
        stopped: &[2],
        waiting: None,
    },
    Scenario {
        name: "seal",
        begin: seal,
        stopped: &[],
        waiting: None,
    },
    Scenario {
        name: "hand",
        begin: hand,
        stopped: &[],
        waiting: None,
    },
    Scenario {
        name: "attach",
        begin: attach,
        stopped: &[],
        waiting: Some(0),
    },
];

/// Steps until the scenario has nothing left to do, bounded.
fn length(scenario: &Scenario) -> usize {
    let (mut sys, t) = System::boot(&machine()).unwrap();
    (scenario.begin)(&mut sys, &t);
    let mut steps = 0;
    while steps < 400 {
        let before = sys.clone();
        step(&mut sys);
        steps += 1;
        let quiet = sys.pending().is_empty() && sys.now() > before.now();
        let settled = scenario
            .stopped
            .iter()
            .all(|&n| sys.member(n).state() == MemberState::Dead);
        if quiet && settled && steps > 2 {
            break;
        }
    }
    steps
}

/// The end state of a run: the crashed and stopped nodes dead, every unit
/// role held by a live node, a live node, the waiting thread answered.
fn recovered(sys: &System, t: &[ObjId], scenario: &Scenario, crashed: u16) -> Result<(), String> {
    for &node in scenario.stopped.iter().chain([&crashed]) {
        let state = sys.member(node).state();
        if state != MemberState::Dead {
            return Err(format!("node {node} is {state:?}"));
        }
    }
    for role in 0..2 {
        let word = sys.role(role);
        let live = sys.member(word.holder()).state() == MemberState::Live;
        if word.state() != RoleState::Held || !live {
            return Err(format!("unit role {role}: {word:?}"));
        }
    }
    way_out(sys)?;
    if let Some(node) = scenario.waiting.filter(|&n| n != crashed) {
        let woken = sys.node(node).woken(t[node as usize]);
        if !matches!(woken, Some(Ok(Output::Done)) | Some(Err(Error::Gone))) {
            return Err(format!("the waiting thread got {woken:?}"));
        }
    }
    Ok(())
}

/// Every member word, role word and grant entry is in a state that a rule
/// leads out of: no node between accusation and death, no role naming a
/// dead node, no grant pinned or made to a generation that is over.
fn way_out(sys: &System) -> Result<(), String> {
    let nodes = 3u16;
    for node in 0..nodes {
        let state = sys.member(node).state();
        if !matches!(
            state,
            MemberState::Live | MemberState::Sealed | MemberState::Dead
        ) {
            return Err(format!("node {node} stays {state:?}"));
        }
    }
    for role in 0..sys.node_count_roles() {
        let word = sys.role(role);
        if word.state() != RoleState::Vacant
            && sys.member(word.holder()).state() == MemberState::Dead
        {
            return Err(format!("role {role} names dead node {}", word.holder()));
        }
    }
    for node in (0..nodes).filter(|&n| sys.has(n)) {
        for (grant, (grantee, generation, _, state)) in sys.node(node).grant_entries() {
            let over = sys.member(grantee).state() == MemberState::Dead
                || sys.member(grantee).generation() != generation;
            if state == 2 || over {
                return Err(format!("grant {grant} of node {node} has no way out"));
            }
        }
    }
    Ok(())
}

/// Every crash at every step of one scenario is recovered from.
fn explore(name: &str) {
    let scenario = SCENARIOS
        .iter()
        .find(|s| s.name == name)
        .expect("a scenario");
    {
        let steps = length(scenario);
        for at in 0..=steps {
            for crashed in (0..3u16).filter(|n| !scenario.stopped.contains(n)) {
                let (mut sys, t) = System::boot(&machine()).unwrap();
                (scenario.begin)(&mut sys, &t);
                for _ in 0..at {
                    step(&mut sys);
                }
                // A crash: the node's core hangs, if it still runs.
                if sys.runs(crashed) {
                    sys.hang(crashed, true);
                } else if sys.member(crashed).state() == MemberState::Sealed {
                    // A sealed node is crashed by a kill of its own core.
                    let killer = (0..3u16).find(|&n| n != crashed && sys.runs(n)).unwrap();
                    let call = Syscall::CoreStop {
                        core: core(crashed.into()),
                        mode: CORE_STOP_KILL,
                    };
                    let thread = sys.node(killer).object_at_start(START_SLOT_THREAD);
                    let _ = sys.apply(killer, thread, call);
                }
                let until = sys.now() + 400 * MS;
                while sys.now() < until {
                    let at = (sys.now() + MS).min(until);
                    sys.run(at, MS);
                    assert_eq!(sys.check(), Ok(()), "{} at step {at}", scenario.name);
                }
                let outcome = recovered(&sys, &t, scenario, crashed);
                assert_eq!(
                    outcome,
                    Ok(()),
                    "{}: node {crashed} crashed at step {at}",
                    scenario.name
                );
            }
        }
    }
}

// One test per scenario, so that they run in parallel.
#[test]
fn every_crash_during_a_kill_is_recovered_from() {
    explore("kill");
}

#[test]
fn every_crash_during_a_seal_is_recovered_from() {
    explore("seal");
}

#[test]
fn every_crash_during_a_hand_over_is_recovered_from() {
    explore("hand");
}

#[test]
fn every_crash_while_a_thread_waits_for_a_unit_holder_is_recovered_from() {
    explore("attach");
}

#[test]
fn every_crash_while_a_frame_is_granted_and_mapped_is_recovered_from() {
    explore("grant");
}

#[test]
fn every_crash_while_a_reserve_chunk_is_taken_is_recovered_from() {
    explore("reserve");
}
