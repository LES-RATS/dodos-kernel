//! Cores and roles (8.4): the calls of the group, detection with evidence,
//! the fence on the hardware of `System`, reclaim, the barrier, restart,
//! seal and resume, and roles, on the two nodes of `Machine::example`
//! (node 0 on the bootstrap core, of fence class 1; node 1 on a core of
//! fence class 2, which holds both unit roles).

use dodos_abi::ObjectKind::{Frame, Notify, Reply};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const UT: SlotAddr = SlotAddr::own(25);
const MS: u64 = 1_000_000;
/// The role slots of the start-up space: the IOMMU unit, the IOAPIC, two
/// reserves, the plain role `platform`.
const IOMMU_ROLE: u32 = 19;
const IOAPIC_ROLE: u32 = 20;
const RESERVE: u32 = 21;
const PLATFORM: u32 = 23;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

fn retype(kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

fn stop(n: u32, mode: u64) -> Syscall {
    Syscall::CoreStop {
        core: core(n),
        mode,
    }
}

fn start(n: u32, mode: u64) -> Syscall {
    Syscall::CoreStart {
        core: core(n),
        mode,
        image: 0,
        module: 0,
    }
}

/// The machine, the first thread of each node, and every invariant after
/// every event.
struct S {
    sys: System,
    t: Vec<ObjId>,
}

impl S {
    fn new() -> Self {
        Self::on(&Machine::example())
    }

    fn on(machine: &Machine) -> Self {
        let (sys, t) = System::boot(machine).unwrap();
        Self { sys, t }
    }

    #[track_caller]
    fn on_node(&mut self, node: u16, call: Syscall) -> Outcome {
        let outcome = self.sys.apply(node, self.t[node as usize], call);
        assert_eq!(self.sys.check(), Ok(()));
        outcome
    }

    #[track_caller]
    fn ok(&mut self, node: u16, call: Syscall) {
        assert_eq!(self.on_node(node, call), DONE);
    }

    #[track_caller]
    fn no(&mut self, node: u16, call: Syscall, error: Error) {
        assert_eq!(self.on_node(node, call), Err(error));
    }

    /// The machine runs for `ns`, in steps of 100 µs, every invariant
    /// checked at each step.
    #[track_caller]
    fn run(&mut self, ns: u64) {
        let to = self.sys.now() + ns;
        while self.sys.now() < to {
            let at = (self.sys.now() + 100_000).min(to);
            self.sys.run(at, 100_000);
            assert_eq!(self.sys.check(), Ok(()));
        }
    }

    fn state(&self, node: u16) -> MemberState {
        self.sys.member(node).state()
    }

    /// A node restarted fresh: its first thread is the new one.
    fn restarted(&mut self, node: u16) {
        let first = self.sys.node(node).object_at_start(START_SLOT_THREAD);
        self.t[node as usize] = first;
    }
}

// The calls of the group.

#[test]
fn core_status_reads_the_member_word_the_placement_and_the_counters() {
    let mut m = S::new();
    let Ok(Output::Status(words)) = m.on_node(0, Syscall::CoreStatus(core(1))) else {
        panic!("status");
    };
    let member = Member::from_raw(words[0]).unwrap();
    assert_eq!(
        (member.state(), member.generation()),
        (MemberState::Live, 1)
    );
    // Node 1 on its own core 1, of fence class 2, usable, not parked.
    assert_eq!(words[1], 1 | 2 << CORE_STATUS_FENCE_SHIFT);
    assert_eq!(words[6], Machine::example().perf_caps);
    // `read` is needed.
    m.ok(
        0,
        Syscall::CapDerive {
            source: core(1),
            destination: s(100),
            rights: Rights::WRITE.bits(),
            badge: 0,
        },
    );
    m.no(0, Syscall::CoreStatus(s(100)), Error::Denied);
}

#[test]
fn core_watch_names_the_callers_node_and_is_signalled_at_each_change() {
    let mut m = S::new();
    m.ok(0, retype(Notify, 0, 100));
    let watch = |n, index, notify| Syscall::CoreWatch {
        core: core(n),
        index,
        notify,
    };
    m.no(0, watch(1, 0, s(100)), Error::Invalid);
    m.no(0, watch(0, 4, s(100)), Error::Invalid);
    m.ok(0, watch(0, 0, s(100)));
    m.no(0, watch(0, 0, s(100)), Error::Busy);
    // Unbinding succeeds on an unbound index.
    m.ok(0, watch(0, 3, SlotAddr::NONE));
    // A change of a member word: the notify is signalled.
    m.ok(0, stop(1, CORE_STOP_KILL));
    assert_eq!(m.on_node(0, Syscall::Poll(s(100))), Ok(Output::Word(1)));
    m.run(5 * MS);
    assert_eq!(m.on_node(0, Syscall::Poll(s(100))), Ok(Output::Word(1)));
}

#[test]
fn core_idle_judges_its_table_entry_by_entry() {
    let mut m = S::new();
    let idle = |words: Vec<u64>| Syscall::CoreIdle {
        core: core(0),
        words,
    };
    m.ok(0, idle(vec![]));
    m.ok(
        0,
        idle(vec![
            10,
            20,
            IDLE_METHOD_HALT,
            0,
            30,
            40,
            IDLE_METHOD_MWAIT | 0x20 << 32,
            IDLE_FLAG_LOSES_CONTEXT,
        ]),
    );
    for words in [
        vec![1, 2, 0],
        vec![1 << 63, 0, 0, 0],
        vec![10, 0, 0, 0, 10, 0, 0, 0],
        vec![10, 0, IDLE_METHOD_PSCI, 0],
        vec![10, 0, 0, 2],
        vec![0; 64],
    ] {
        m.no(0, idle(words), Error::Invalid);
    }
    // On another node the call is carried there.
    let other = Syscall::CoreIdle {
        core: core(1),
        words: vec![],
    };
    assert_eq!(m.on_node(0, other), WAITS);
    m.run(MS);
    assert_eq!(m.sys.node(0).woken(m.t[0]), Some(&DONE));
}

#[test]
fn core_perf_is_judged_and_unsupported_without_the_register() {
    let mut m = S::new();
    let perf = |n, lowest, highest| Syscall::CorePerf {
        core: core(n),
        lowest,
        highest,
        preference: 0x80,
    };
    m.ok(0, perf(0, 10, 40));
    m.no(0, perf(0, 41, 40), Error::Invalid);
    m.no(0, perf(0, 10, 256), Error::Invalid);
    assert_eq!(m.on_node(0, perf(1, 10, 40)), WAITS);
    let mut machine = Machine::example();
    machine.perf_request = false;
    let mut m = S::on(&machine);
    m.no(0, perf(0, 41, 40), Error::Unsupported);
}

#[test]
fn core_place_is_judged_in_order() {
    let mut m = S::new();
    let place = |n, physical| Syscall::CorePlace {
        core: core(n),
        physical,
    };
    m.no(0, place(1, 2), Error::Invalid);
    // Both nodes keep half a core each with the recovery reserves: node 1
    // placed on core 0 would ask more than one core.
    m.no(0, place(1, 0), Error::Budget);
    // A core of a kind that lacks a feature of the node's kind.
    let mut machine = Machine::example();
    let mut lesser = machine.kinds[0];
    lesser.features &= !(1 << x86_64::AMX);
    machine.kinds.push(lesser);
    machine.cores[0].kind = 1;
    let mut m = S::on(&machine);
    m.no(0, place(1, 0), Error::Busy);
}

#[test]
fn core_stop_is_judged_and_never_leaves_no_live_node() {
    let mut m = S::new();
    m.no(0, stop(1, 3), Error::Invalid);
    m.ok(0, stop(1, CORE_STOP_KILL));
    // Already fencing: busy for every mode.
    m.no(0, stop(1, CORE_STOP_KILL), Error::Busy);
    m.no(0, stop(1, CORE_STOP_SEAL), Error::Busy);
    // The last live node.
    m.no(0, stop(0, CORE_STOP_KILL), Error::Busy);
    m.no(0, stop(0, CORE_STOP_RETIRE), Error::Busy);
}

#[test]
fn core_start_is_judged_in_order() {
    let mut m = S::new();
    let call = |mode, image, module| Syscall::CoreStart {
        core: core(1),
        mode,
        image,
        module,
    };
    m.no(0, call(2, 0, 0), Error::Invalid);
    m.no(0, call(0, 2, 0), Error::Invalid);
    m.no(0, call(0, 0, 9), Error::Invalid);
    m.no(0, call(1, 0, 1), Error::Invalid);
    // A live node: busy for both modes.
    m.no(0, call(0, 0, 0), Error::Busy);
    m.no(0, call(1, 0, 0), Error::Busy);
}

// Stop, fence, reclaim, restart.

#[test]
fn a_killed_node_is_fenced_reclaimed_and_dead_then_restarted_fresh() {
    let mut m = S::new();
    m.ok(0, stop(1, CORE_STOP_KILL));
    let member = m.sys.member(1);
    assert_eq!(
        (member.state(), member.evidence(), member.driver()),
        (MemberState::Fencing, Evidence::Kill, 0)
    );
    // The NMI puts core 1 in the handler's loop; INIT and SIPI park it.
    m.run(2 * MS);
    assert_eq!(m.sys.core(1), CoreState::Parked);
    // Its unit roles go to node 0 after SETTLE_NS, then the barrier holds.
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert!(!m.sys.has(1));
    for unit in 0..2 {
        assert_eq!(m.sys.role(unit).holder(), 0);
    }
    // A fresh start: generation 2, its own start-up state.
    m.ok(0, start(1, CORE_START_FRESH));
    assert_eq!(m.sys.member(1).generation(), 2);
    assert_eq!(m.state(1), MemberState::Live);
    m.restarted(1);
    m.ok(1, retype(Notify, 0, 100));
}

#[test]
fn a_node_that_reports_a_failure_is_accused_with_evidence_self() {
    let mut m = S::new();
    m.sys.panic(1, 7);
    m.run(MS / 10);
    assert_eq!(m.sys.member(1).evidence(), Evidence::SelfReported);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn a_hung_node_is_accused_stale_by_the_sweep() {
    let mut m = S::new();
    m.sys.hang(1, false);
    // The sweep observes it at its first look, and the heartbeat does not
    // move for STALE_NS.
    m.run(SWEEP_NS + STALE_NS + 10 * MS);
    let member = m.sys.member(1);
    assert!(
        matches!(member.state(), MemberState::Fenced | MemberState::Dead),
        "{member:?}"
    );
    assert_eq!(member.evidence(), Evidence::Stale);
}

#[test]
fn an_idle_node_that_does_not_consume_the_sweeps_ping_is_accused_stuck() {
    let mut m = S::new();
    // Node 1's first thread stops: its core is idle with no timer, so the
    // sweep pings it; hung, it never consumes the ping.
    m.ok(1, Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    m.sys.hang(1, false);
    m.run(SWEEP_NS + STUCK_NS + 10 * MS);
    let member = m.sys.member(1);
    assert!(
        matches!(member.state(), MemberState::Fenced | MemberState::Dead),
        "{member:?}"
    );
    assert_eq!(member.evidence(), Evidence::Stuck);
}

#[test]
fn a_hung_bootstrap_core_that_cannot_run_its_handler_is_unfenced() {
    let mut m = S::new();
    m.sys.hang(0, false);
    m.ok(1, stop(0, CORE_STOP_KILL));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(0), MemberState::Unfenced);
    // Its unit roles... it holds none; it never becomes dead while its
    // stop is not confirmed.
    m.run(SWEEP_NS);
    assert_eq!(m.state(0), MemberState::Unfenced);
}

#[test]
fn a_core_in_vmx_root_operation_is_unfenced_then_fenced_when_it_leaves_it() {
    let mut m = S::new();
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, stop(1, CORE_STOP_KILL));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    // Its unit roles are taken over after FENCE_NS + SETTLE_NS.
    m.run(2 * MS);
    for unit in 0..2 {
        assert_eq!(m.sys.role(unit).holder(), 0);
    }
    // It leaves VMX root operation: the next try confirms the stop.
    m.sys.set_vmx(1, false);
    m.run(SWEEP_NS + 5 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

#[test]
fn a_retired_core_is_unusable_for_a_fresh_start() {
    let mut m = S::new();
    m.ok(0, stop(1, CORE_STOP_RETIRE));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.no(0, start(1, CORE_START_FRESH), Error::Busy);
    let Ok(Output::Status(words)) = m.on_node(0, Syscall::CoreStatus(core(1))) else {
        panic!("status");
    };
    assert_ne!(words[1] & CORE_STATUS_UNUSABLE, 0);
}

#[test]
fn a_node_that_stops_itself_parks_and_another_drives_its_fence() {
    let mut m = S::new();
    m.ok(1, stop(1, CORE_STOP_KILL));
    assert_eq!(m.sys.core(1), CoreState::Parked);
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
}

// Seal and resume.

#[test]
fn a_sealed_node_hands_its_unit_roles_and_resumes_with_its_generation() {
    let mut m = S::new();
    m.ok(1, retype(Notify, 0, 100));
    m.ok(0, stop(1, CORE_STOP_SEAL));
    assert_eq!(m.state(1), MemberState::Sealing);
    m.run(MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    assert_eq!(m.sys.core(1), CoreState::Parked);
    for unit in 0..2 {
        assert_eq!(
            (m.sys.role(unit).state(), m.sys.role(unit).holder()),
            (RoleState::Held, 0)
        );
    }
    m.ok(0, start(1, CORE_START_RESUME));
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.member(1).generation(), 1);
    // Its objects are as they were.
    assert_eq!(m.on_node(1, Syscall::Poll(s(100))), Err(Error::WouldWait));
}

#[test]
fn a_resume_with_an_image_that_cannot_read_the_block_accuses_itself() {
    let mut machine = Machine::example();
    machine.replacement = true;
    machine.replacement_reads_sealed = false;
    let mut m = S::on(&machine);
    m.ok(0, stop(1, CORE_STOP_SEAL));
    m.run(MS);
    let resume = Syscall::CoreStart {
        core: core(1),
        mode: CORE_START_RESUME,
        image: CORE_IMAGE_REPLACEMENT,
        module: 0,
    };
    m.ok(0, resume);
    m.run(10 * MS);
    let member = m.sys.member(1);
    assert_eq!(
        (member.state(), member.evidence()),
        (MemberState::Dead, Evidence::SelfReported)
    );
}

#[test]
fn the_replacement_image_needs_a_full_slot() {
    let mut m = S::new();
    m.ok(0, stop(1, CORE_STOP_SEAL));
    m.run(MS);
    let resume = Syscall::CoreStart {
        core: core(1),
        mode: CORE_START_RESUME,
        image: CORE_IMAGE_REPLACEMENT,
        module: 0,
    };
    m.no(0, resume, Error::Busy);
}

// Roles.

#[test]
fn role_claim_takes_a_vacant_plain_or_reserve_role() {
    let mut m = S::new();
    m.no(0, Syscall::RoleClaim(s(IOMMU_ROLE)), Error::Invalid);
    assert_eq!(
        m.on_node(0, Syscall::RoleClaim(s(PLATFORM))),
        Ok(Output::Epoch(1))
    );
    // Held by the caller's node: no change.
    assert_eq!(
        m.on_node(0, Syscall::RoleClaim(s(PLATFORM))),
        Ok(Output::Epoch(1))
    );
    // Held by a live node.
    m.no(1, Syscall::RoleClaim(s(PLATFORM)), Error::Busy);
    assert_eq!(
        m.on_node(1, Syscall::RoleClaim(s(RESERVE))),
        Ok(Output::Epoch(1))
    );
}

#[test]
fn role_hand_moves_a_plain_role_or_vacates_it() {
    let mut m = S::new();
    let hand = |core| Syscall::RoleHand {
        role: s(PLATFORM),
        core,
    };
    m.no(0, hand(core(1)), Error::Busy);
    m.on_node(0, Syscall::RoleClaim(s(PLATFORM))).unwrap();
    assert_eq!(m.on_node(0, hand(core(0))), Ok(Output::Epoch(1)));
    assert_eq!(m.on_node(0, hand(core(1))), Ok(Output::Epoch(2)));
    assert_eq!(m.sys.role(4).holder(), 1);
    assert_eq!(m.on_node(1, hand(SlotAddr::NONE)), Ok(Output::Epoch(3)));
    assert_eq!(m.sys.role(4).state(), RoleState::Vacant);
}

#[test]
fn a_unit_role_is_handed_in_steps_and_never_to_a_lesser_fence_class() {
    let mut m = S::new();
    let hand = |role, core| Syscall::RoleHand {
        role: s(role),
        core,
    };
    m.no(1, hand(IOMMU_ROLE, SlotAddr::NONE), Error::Invalid);
    // Node 0's core is of class 1, below node 1's.
    m.no(1, hand(IOMMU_ROLE, core(0)), Error::Busy);
    m.no(0, hand(IOAPIC_ROLE, core(0)), Error::Busy);
    // With equal classes the hand-over goes through handing.
    let mut machine = Machine::example();
    machine.cores[1].fence = 1;
    let mut m = S::on(&machine);
    // Unit role 0 is node 0's: hand it to node 1.
    assert_eq!(
        m.on_node(0, hand(IOMMU_ROLE, core(1))),
        Ok(Output::Epoch(2))
    );
    assert_eq!(m.sys.role(0).state(), RoleState::Handing);
    m.run(MS);
    assert_eq!(
        (
            m.sys.role(0).state(),
            m.sys.role(0).holder(),
            m.sys.role(0).epoch()
        ),
        (RoleState::Held, 1, 3)
    );
}

// Reclaim and requests between nodes.

#[test]
fn a_thread_waiting_on_a_fencing_node_gets_gone() {
    let mut m = S::new();
    // Node 1 waits on node 0's mailbox... node 0 waits on node 1's.
    m.ok(1, retype(Reply, 0, 99));
    let call = Syscall::Call {
        endpoint: core(1),
        message: Message::default(),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(m.on_node(0, call), WAITS);
    m.sys.hang(1, false);
    let b = {
        // A second node-0 thread is not needed: node 0's recovery ends the
        // wait once it sees node 1 fencing, which the sweep finds.
        m.t[0]
    };
    m.run(SWEEP_NS + STALE_NS + 10 * MS);
    assert_eq!(m.sys.node(0).woken(b), Some(&Err(Error::Gone)));
}

#[test]
fn an_iospace_attach_on_a_node_without_the_unit_role_is_a_request_to_its_holder() {
    let mut m = S::new();
    m.ok(0, retype(ObjectKind::IoSpace, 0, 100));
    let attach = Syscall::IoSpaceAttach {
        iommu: s(18),
        iospace: s(100),
        device: 0x10,
    };
    assert_eq!(m.on_node(0, attach), WAITS);
    assert!(
        m.sys
            .find(|b| matches!(b, Body::UnitRequest { .. }))
            .is_some()
    );
    m.run(MS);
    assert_eq!(m.sys.node(0).woken(m.t[0]), Some(&DONE));
}

#[test]
fn a_request_with_an_old_epoch_is_refused_and_sent_to_the_new_holder() {
    let mut machine = Machine::example();
    machine.cores[1].fence = 1;
    let mut m = S::on(&machine);
    // The IOMMU role is node 0's; node 1 asks it, then node 0 hands it to
    // node 1 before the request arrives.
    m.ok(1, retype(ObjectKind::IoSpace, 0, 100));
    let attach = Syscall::IoSpaceAttach {
        iommu: s(18),
        iospace: s(100),
        device: 0x10,
    };
    assert_eq!(m.on_node(1, attach), WAITS);
    let hand = Syscall::RoleHand {
        role: s(IOMMU_ROLE),
        core: core(1),
    };
    m.on_node(0, hand).unwrap();
    m.run(MS);
    assert_eq!(m.sys.node(1).woken(m.t[1]), Some(&DONE));
    let _ = Frame;
}

// Reclaim of what crossed nodes.

/// Node `granter` makes a frame in slot 100 and gives node `holder` a copy
/// in slot 110, which the holder maps at 1 GB.
fn granted_frame(m: &mut S, granter: u16, holder: u16) {
    m.ok(granter, retype(Frame, 1, 100));
    m.ok(holder, retype(Reply, 0, 99));
    let recv = Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(99),
        range: Range {
            first: s(110),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(m.on_node(holder, recv), WAITS);
    let send = Syscall::Send {
        target: core(holder.into()),
        message: Message {
            caps: vec![Transfer::Derive(
                s(100),
                Rights::READ.union(Rights::DERIVE).bits(),
            )],
            ..Message::default()
        },
    };
    assert_eq!(m.on_node(granter, send), WAITS);
    m.run(MS / 10);
    let map = Syscall::ASpaceMap(Map {
        space: s(START_SLOT_ASPACE),
        frame: s(110),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(holder, map);
}

#[test]
fn a_holder_removes_what_it_maps_of_a_fenced_node_before_it_acknowledges() {
    let mut m = S::new();
    granted_frame(&mut m, 0, 1);
    assert!(m.sys.node(1).translated(m.t[1], 1 << 30).is_some());
    m.ok(1, stop(0, CORE_STOP_KILL));
    m.run(5 * MS);
    // Node 0 was on the bootstrap core: its handler parked it.
    assert_eq!(m.state(0), MemberState::Dead);
    assert_eq!(m.sys.node(1).translated(m.t[1], 1 << 30), None);
}

#[test]
fn the_frames_granted_to_a_fenced_node_stay_pinned_until_it_is_dead() {
    let mut m = S::new();
    granted_frame(&mut m, 0, 1);
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, stop(1, CORE_STOP_KILL));
    m.run(FENCE_NS + 5 * MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    // The grant is pinned: removing its frame does not finish.
    m.no(0, Syscall::CapRevoke(s(100)), Error::Again);
    m.sys.set_vmx(1, false);
    m.run(SWEEP_NS + 5 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // Unpinned at its death: the revocation finishes.
    m.ok(0, Syscall::CapRevoke(s(100)));
}

/// Three nodes: node 2 on a core of its own, of fence class 2.
fn three() -> S {
    let mut machine = Machine::example();
    let mut third = machine.cores[1];
    third.hw_id = 4;
    machine.cores.push(third);
    let (sys, t) = System::boot(&machine).unwrap();
    S { sys, t }
}

#[test]
fn a_node_parked_on_a_fenced_core_is_accused_host_without_a_valid_seal() {
    let mut m = three();
    // Nodes 0 and 1 share core 0: each keeps a tenth of a core.
    for node in 0..2u16 {
        m.ok(
            node,
            Syscall::SchedConfigure {
                sched: s(START_SLOT_SCHED),
                core: core(node.into()),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            },
        );
    }
    m.ok(
        0,
        Syscall::CorePlace {
            core: core(1),
            physical: 0,
        },
    );
    // Node 2 kills node 0: when core 0 is stopped, node 1, which ran there
    // with no valid seal, is fenced with evidence host in the same step.
    m.ok(2, stop(0, CORE_STOP_KILL));
    m.run(5 * MS);
    let member = m.sys.member(1);
    assert_eq!(member.evidence(), Evidence::Host);
    m.run(10 * MS);
    assert_eq!(
        (m.state(0), m.state(1)),
        (MemberState::Dead, MemberState::Dead)
    );
}

#[test]
fn a_sealed_node_parked_on_a_fenced_core_stays_sealed() {
    let mut m = three();
    for node in 0..2u16 {
        m.ok(
            node,
            Syscall::SchedConfigure {
                sched: s(START_SLOT_SCHED),
                core: core(node.into()),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            },
        );
    }
    m.ok(2, stop(1, CORE_STOP_SEAL));
    m.run(MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    m.ok(
        0,
        Syscall::CorePlace {
            core: core(1),
            physical: 0,
        },
    );
    m.ok(2, stop(0, CORE_STOP_KILL));
    m.run(10 * MS);
    assert_eq!(
        (m.state(0), m.state(1)),
        (MemberState::Dead, MemberState::Sealed)
    );
}

#[test]
fn a_node_dying_while_starting_is_accused_stale_after_start_ns() {
    let mut m = S::new();
    m.ok(0, stop(1, CORE_STOP_KILL));
    m.run(10 * MS);
    m.ok(0, start(1, CORE_START_FRESH));
    m.sys.hang(1, false);
    m.run(SWEEP_NS + STALE_NS + 10 * MS);
    let member = m.sys.member(1);
    assert!(matches!(
        member.state(),
        MemberState::Fenced | MemberState::Dead
    ));
    assert_eq!(member.generation(), 2);
}

#[test]
fn a_plain_role_of_a_fenced_holder_can_be_claimed_a_reserve_role_only_once_it_is_dead() {
    let mut m = S::new();
    assert_eq!(
        m.on_node(1, Syscall::RoleClaim(s(PLATFORM))),
        Ok(Output::Epoch(1))
    );
    assert_eq!(
        m.on_node(1, Syscall::RoleClaim(s(RESERVE))),
        Ok(Output::Epoch(1))
    );
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, stop(1, CORE_STOP_KILL));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    assert_eq!(
        m.on_node(0, Syscall::RoleClaim(s(PLATFORM))),
        Ok(Output::Epoch(2))
    );
    m.no(0, Syscall::RoleClaim(s(RESERVE)), Error::Busy);
    m.sys.set_vmx(1, false);
    m.run(SWEEP_NS + 5 * MS);
    // At its death the reserve role became vacant.
    assert_eq!(m.sys.role(2).state(), RoleState::Vacant);
    assert_eq!(
        m.on_node(0, Syscall::RoleClaim(s(RESERVE))),
        Ok(Output::Epoch(3))
    );
}

#[test]
fn a_fence_whose_driver_dies_is_driven_again_by_another_node() {
    // Node 0 kills node 2 and is then killed by node 1 before the fence of
    // node 2 ends.
    let mut m = three();
    m.sys.hang(2, false);
    m.sys.set_vmx(2, true);
    m.ok(0, stop(2, CORE_STOP_KILL));
    m.ok(1, stop(0, CORE_STOP_KILL));
    m.run(MS);
    // Node 1 drives the fence of node 2 now.
    assert_eq!(m.sys.member(2).driver(), 1);
    m.sys.set_vmx(2, false);
    m.run(SWEEP_NS + 20 * MS);
    assert_eq!(
        (m.state(0), m.state(2)),
        (MemberState::Dead, MemberState::Dead)
    );
    assert_eq!(m.state(1), MemberState::Live);
}

// Detection never accuses a node that beats.

#[test]
fn nodes_that_beat_and_consume_their_rings_are_never_accused() {
    let mut m = S::new();
    // Node 1 idle with no timer: the sweep pings it, and it consumes the
    // pings.
    m.ok(1, Syscall::ThreadSuspend(s(START_SLOT_THREAD)));
    m.run(3 * SWEEP_NS);
    assert_eq!(
        (m.state(0), m.state(1)),
        (MemberState::Live, MemberState::Live)
    );
}

#[test]
fn a_firmware_stall_shorter_than_stale_ns_accuses_nobody() {
    let mut m = S::new();
    m.run(SWEEP_NS + MS);
    m.sys.stall(STALE_NS / 2);
    m.run(SWEEP_NS);
    assert_eq!(
        (m.state(0), m.state(1)),
        (MemberState::Live, MemberState::Live)
    );
}

#[test]
fn core_place_refuses_a_core_marked_unusable() {
    let mut m = three();
    m.ok(0, stop(2, CORE_STOP_RETIRE));
    m.run(10 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    m.no(
        0,
        Syscall::CorePlace {
            core: core(1),
            physical: 2,
        },
        Error::Busy,
    );
}

#[test]
fn init_waits_for_the_grace_of_the_nmi() {
    let mut m = S::new();
    m.ok(0, stop(1, CORE_STOP_KILL));
    m.run(NMI_GRACE_NS / 2);
    assert_eq!(m.state(1), MemberState::Fencing);
    assert_eq!(m.sys.core(1), CoreState::Looping);
    m.run(NMI_GRACE_NS);
    assert_eq!(m.sys.core(1), CoreState::Parked);
}

#[test]
fn a_node_that_is_not_live_does_not_accuse_the_last_live_node() {
    let mut m = S::new();
    // Node 1 seals; node 0 hangs before it takes the unit roles: node 1,
    // sealing, has evidence, but node 0 is the only live node.
    m.ok(0, stop(1, CORE_STOP_SEAL));
    m.sys.hang(0, true);
    m.run(SWEEP_NS + STALE_NS + 10 * MS);
    assert_eq!(m.state(0), MemberState::Live);
    assert_eq!(m.state(1), MemberState::Sealing);
}
