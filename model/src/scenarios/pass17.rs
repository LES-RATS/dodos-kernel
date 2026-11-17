//! The scenarios of pass 17 (D-134): the corrections of the ninth review of
//! the contract (`Docs/reviews/2026-11-16-contract-review-9-*.md`) and of
//! the eighth review of the model (`2026-11-16-model-review-8-*.md`), each
//! a test named after the sentence it guards.

use super::*;
use crate::Range;
use crate::rings::Body;
use crate::system::Hardware;
use dodos_abi::{
    CoreState, CoreWord, NODE_START_FRESH, NODE_START_RESUME, NODE_STOP_KILL, NODE_STOP_SEAL,
    RingHeader, START_SLOT_MAILBOX, START_SLOT_THREAD, TaskId,
};

const REPLY: u32 = 100;
const GOT: u32 = 110;

fn recv() -> Syscall {
    Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(REPLY),
        range: Range {
            first: s(GOT),
            count: 4,
        },
        wait: 0,
    }
}

fn send(to: u16) -> Syscall {
    Syscall::NodeSend {
        node: node_cap(to),
        message: crate::Message {
            words: vec![7],
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    }
}

pub(super) fn kill(n: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode: NODE_STOP_KILL,
        core: 0,
    }
}

pub(super) fn seal(n: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(n),
        mode: NODE_STOP_SEAL,
        core: 0,
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

pub(super) fn fresh(n: u16, mask: u64) -> Syscall {
    Syscall::NodeStart {
        node: node_cap(n),
        mode: NODE_START_FRESH,
        image: 0,
        module: 0,
        window: 0,
        mask,
    }
}

fn forged_send(sender: u16, stamp: u64) -> crate::rings::Message {
    let body = Body::Send {
        header: dodos_abi::Header::new(7, 1, 0, 0).unwrap(),
        stamp: (stamp, 0),
        grants: vec![],
        words: vec![7],
    };
    crate::rings::Message {
        header: RingHeader::new(
            dodos_abi::NodeMessage::Send,
            body.entries() as u8,
            (sender, 1),
            1,
            Some((dodos_abi::REQUESTS_PER_PEER as u8, 1)),
        ),
        body,
    }
}

// The stamp of a sender attached to no task (What crosses between nodes,
// the one check: "a `Send`'s stamp carries 0 ... or a task identifier of
// the sender's node with a counter that is not 0", D-134).

#[test]
fn a_thread_attached_to_no_task_sends_to_another_node_and_gets_its_answer() {
    let mut m = S::two();
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    assert_eq!(m.call(0, recv()), WAITS);
    // Node 1's first thread leaves its task: its stamp is 0, which reads
    // as node 0 and was refused from any node but node 0.
    m.ok(
        1,
        Syscall::TaskAttach {
            thread: s(START_SLOT_THREAD),
            task: SlotAddr::NONE,
        },
    );
    assert_eq!(m.call(1, send(0)), WAITS);
    m.settle();
    assert_eq!(m.sys.node(0).refused.0, 0);
    assert_eq!(m.woken(1), Some(&Ok(Output::Sent { flags: 0 })));
    let Some(Ok(Output::Message(delivery))) = m.woken(0) else {
        panic!("node 0 receives the message: {:?}", m.woken(0));
    };
    assert_eq!(delivery.received.task, 0);
    assert_eq!(delivery.words, vec![7]);
}

#[test]
fn a_send_whose_stamp_has_a_counter_of_0_is_refused() {
    for stamp in [TaskId::new(1, 0).raw(), TaskId::new(2, 0).raw()] {
        let mut m = S::two();
        let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
        m.ok(0, reply);
        assert_eq!(m.call(0, recv()), WAITS);
        m.sys.inject(1, 0, forged_send(1, stamp));
        m.settle();
        assert_eq!(m.sys.node(0).refused.0, 1, "{stamp:#x}");
        assert_eq!(m.woken(0), None);
    }
}

#[test]
fn a_send_whose_stamp_is_0_is_accepted_from_any_node() {
    let mut m = S::two();
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    assert_eq!(m.call(0, recv()), WAITS);
    m.sys.inject(1, 0, forged_send(1, 0));
    m.settle();
    assert_eq!(m.sys.node(0).refused.0, 0);
    assert!(matches!(m.woken(0), Some(Ok(Output::Message(_)))));
}

// The fence signal and the start (Node slots and core words, the start
// request and the entry stub, D-134).

#[test]
fn a_start_wake_that_reaches_a_core_already_running_changes_nothing() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1, 2]]));
    let before = m.sys.canonical().core_beacons[1].parked;
    let request = m.sys.park_request(1);
    // A late wake of a start reaches core 1, which runs node 1: the
    // kernel's handler finds no fence mark and returns.
    m.sys.late_wake(1);
    m.settle();
    assert_eq!(m.sys.hardware(1), Hardware::Running);
    assert_eq!(m.sys.canonical().core_beacons[1].parked, before);
    assert_eq!(m.sys.park_request(1), request);
    assert_eq!(m.state(1), MemberState::Live);
}

#[test]
fn a_fence_nmi_that_lands_in_the_entry_stub_gives_no_false_confirmation() {
    // Node 0 is on the bootstrap core, which INIT does not stop: only the
    // NMI and the entry stub's own read of its word can park it.
    let mut m = S::on(&machine(crate::Arch::X86_64, 2, &[&[0], &[1]]));
    m.ok(0, kill(0));
    m.run(10 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    m.ok(1, fresh(0, 0b1));
    // The park stub takes the request and leaves for the entry stub.
    for _ in 0..1000 {
        if m.sys.hardware(0) == (Hardware::EntryStub { installed: false }) {
            break;
        }
        let Some(&event) = m.sys.events().first() else {
            m.sys.move_clock(m.sys.now() + 100_000);
            continue;
        };
        m.sys.fire(event);
    }
    assert_eq!(m.sys.hardware(0), Hardware::EntryStub { installed: false });
    // Node 0 is accused while its core is in the entry stub, before the
    // stub installed the kernel's handler: the fence raises the park
    // request, marks the word and sends the NMI, which the stub's handler
    // only returns from.
    m.ok(1, kill(0));
    let request = loop {
        let stop = m
            .sys
            .actions()
            .iter()
            .position(|(_, a)| *a == crate::nodes::HwAction::Stop { core: 0 });
        if let Some(index) = stop {
            let request = m.sys.park_request(0);
            m.sys.fire(crate::system::Event::Action(index));
            break request;
        }
        let events: Vec<_> = m
            .sys
            .events()
            .into_iter()
            .filter(|e| !matches!(e, crate::system::Event::EntryStep { .. }))
            .collect();
        match events.first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
    };
    assert_eq!(m.sys.check(), Ok(()));
    assert_eq!(m.sys.hardware(0), Hardware::EntryStub { installed: false });
    assert_ne!(
        m.sys.canonical().core_beacons[0].parked,
        request,
        "no confirmation from inside the entry stub"
    );
    // The entry stub installs the handler, reads its word, finds the mark,
    // and parks: that confirms the request.
    m.sys.fire(crate::system::Event::EntryStep { core: 0 });
    m.sys.fire(crate::system::Event::EntryStep { core: 0 });
    assert!(matches!(m.sys.hardware(0), Hardware::Stub { .. }));
    assert_eq!(m.sys.canonical().core_beacons[0].parked, request);
    m.run(20 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    assert_eq!(
        CoreWord::from_raw(m.sys.core_word(0)).unwrap().state(),
        CoreState::Free
    );
}

// The resume of a sealed node after a peer died or started fresh (Grants,
// "Generations": "it runs R0, R2 and R3 for every node whose generation
// ended while it was sealed (R3 frees at once the grants it made to a
// generation already over)").

/// Node 0 grants node 1 its frame at slot 101; node 1 holds it at `GOT`.
pub(super) fn granted_to_1(m: &mut S) {
    granted(m);
}

fn granted(m: &mut S) {
    let reply = m.retype(1, ObjectKind::Reply, 0, REPLY);
    m.ok(1, reply);
    let frame = m.retype(0, ObjectKind::Frame, 1, 101);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv()), WAITS);
    let rights = Rights::READ.union(Rights::WRITE).bits();
    let call = Syscall::NodeSend {
        node: node_cap(1),
        message: crate::Message {
            caps: vec![crate::Transfer::Derive(s(101), rights)],
            ..crate::Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
}

#[test]
fn a_resume_after_a_peer_died_and_started_fresh_runs_r0_and_frees_the_grants_to_it() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    granted(&mut m);
    assert_eq!(m.sys.canonical().grants[0].len(), 1);
    // Node 1 stops answering; node 0 sends to it and waits for the answer.
    m.sys.hang(1, true);
    assert_eq!(m.call(0, send(1)), WAITS);
    m.settle();
    m.ok(2, seal(0));
    m.run(5 * MS);
    assert_eq!(m.state(0), MemberState::Sealed);
    // While node 0 is sealed, node 1 is killed, reclaimed and started
    // again with a new generation.
    m.ok(2, kill(1));
    m.run(30 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(2, fresh(1, 0b10));
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    // The resume ends node 0's wait with `gone`, frees its credit toward
    // node 1, and frees the grant made to the generation that is over.
    m.ok(2, resume(0));
    m.run(20 * MS);
    assert_eq!(m.state(0), MemberState::Live);
    assert_eq!(m.woken(0), Some(&Err(Error::Gone)));
    assert!(m.sys.canonical().grants[0].is_empty());
    assert_eq!(m.sys.node(0).peers[1].credits.iter().flatten().count(), 0);
}

// RESERVE (What crosses between nodes, "Answers always have room"): the
// check of a 28-item `Report`, at the boundary and beyond it.

fn credit(
    kind: dodos_abi::NodeMessage,
    completes: crate::rings::Completes,
) -> crate::rings::Credit {
    crate::rings::Credit {
        kind,
        serial: 1,
        completes,
    }
}

/// Node 0's 24 credits toward node 1: one `ReportAsk`, 15 `Revoke`s, 8
/// sends.
fn full_credits() -> Vec<Option<crate::rings::Credit>> {
    use crate::rings::Completes;
    use dodos_abi::NodeMessage;
    let mut peer = vec![Some(credit(
        NodeMessage::ReportAsk,
        Completes::Report { role: 0 },
    ))];
    peer.extend((1..16).map(|g| Some(credit(NodeMessage::Revoke, Completes::Revoke { grant: g }))));
    peer.extend((0..8).map(|_| {
        Some(credit(
            NodeMessage::Send,
            Completes::Send {
                grants: vec![],
                thread: None,
                manage: true,
            },
        ))
    }));
    peer
}

fn ping() -> crate::rings::Message {
    crate::rings::Message {
        header: RingHeader::new(dodos_abi::NodeMessage::Ping, 1, (1, 1), 1, None),
        body: Body::Ping,
    }
}

#[test]
fn reserve_keeps_room_for_a_report_of_28_items_and_fails_beyond_it() {
    let room = (dodos_abi::RING_KERNEL_ENTRIES - dodos_abi::RING_ANSWER_RESERVE) as usize;
    let mut m = S::two();
    for _ in 0..room {
        m.sys.inject(1, 0, ping());
    }
    // 30 entries that are not answers: 34 free, 34 owed.
    assert_eq!(m.sys.check_reserve(0, 1, &full_credits()), Ok(()));
    // Node 1 answers the `ReportAsk` with 28 items, 11 entries: 23 free, 23
    // still owed.
    let report = Body::Report {
        next: u64::MAX,
        items: (0..28).map(crate::rings::Item::Handle).collect(),
    };
    assert_eq!(report.entries(), dodos_abi::RING_MESSAGE_ENTRIES_MAX as u64);
    let answer = crate::rings::Message {
        header: RingHeader::new(dodos_abi::NodeMessage::Report, 11, (1, 1), 1, Some((0, 1))),
        body: report,
    };
    m.sys.inject(1, 0, answer);
    assert_eq!(m.sys.check_reserve(0, 1, &full_credits()), Ok(()));
    // One more entry that is not an answer: 22 free for 23 owed.
    m.sys.inject(1, 0, ping());
    assert_eq!(
        m.sys.check_reserve(0, 1, &full_credits()),
        Err("RESERVE: answers still owed exceed the free entries of the ring".into())
    );
    // A second `ReportAsk` in flight to one node breaks the premise.
    let mut two_asks = full_credits();
    two_asks[1] = Some(credit(
        dodos_abi::NodeMessage::ReportAsk,
        crate::rings::Completes::Report { role: 1 },
    ));
    let fresh_ring = S::two();
    assert_eq!(
        fresh_ring.sys.check_reserve(0, 1, &two_asks),
        Err("RESERVE: more than one ReportAsk in flight to a peer".into())
    );
}

// The claim over several remapping units (Devices and shared hardware,
// several remapping units, gap 12, D-134).

/// Four nodes of one core each on a machine with three IOMMU units on PCI
/// segment 0: the unit roles go to nodes 1, 2 and 3.
fn three_units() -> S {
    let mut machine = machine(crate::Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]);
    let first = machine.iommus[0];
    machine.iommus.push(first);
    machine.iommus.push(first);
    S::on(&machine)
}

fn claim(source: u64, vector: u64, destination: u32) -> Syscall {
    Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source,
        core: 0,
        vector,
        destination: s(destination),
    }
}

fn submissions(m: &S, role: usize) -> u64 {
    m.sys.node(0).table.units.get(&role).map_or(0, |u| u.next)
}

#[test]
fn a_claim_passes_a_failed_unit_asks_the_later_ones_and_ends_busy_keeping_the_irq() {
    let mut m = three_units();
    let roles = m.sys.node(0).segment_roles(0);
    assert_eq!(roles.len(), 3);
    let third = submissions(&m, roles[2]);
    m.sys.break_unit(roles[1]);
    let msi = dodos_abi::IrqSource::msi(0, 3, 1, 0).raw();
    assert_eq!(m.call(0, claim(msi, 48, 210)), WAITS);
    m.run(dodos_abi::IOMMU_WAIT_NS + 60 * MS);
    // The second unit failed; the third was still asked; the call ends
    // `busy`; the irq and its root capability stay as the claim wrote them.
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    assert!(m.sys.role(roles[1]).failed());
    assert_eq!(submissions(&m, roles[2]), third + 1);
    let irq = m.sys.node(0).object_at(m.t[0], s(210));
    assert!(irq.is_some_and(|o| m.sys.node(0).irqs.contains_key(&o)));
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
}

#[test]
fn a_claim_whose_waiting_thread_is_destroyed_still_asks_every_unit() {
    let mut m = three_units();
    let roles = m.sys.node(0).segment_roles(0);
    let before: Vec<u64> = roles.iter().map(|&r| submissions(&m, r)).collect();
    // A second thread of node 0 claims; the claimer is destroyed while it
    // waits for the first unit.
    m.ok(0, place_first(0, 0));
    let thread = m.thread_on(0, 300, 0);
    let msi = dodos_abi::IrqSource::msi(0, 3, 1, 0).raw();
    assert_eq!(m.call_by(0, thread, claim(msi, 49, 211)), WAITS);
    let mut outcome = m.call(0, Syscall::CapDelete(s(300)));
    while outcome == Err(Error::Again) {
        outcome = m.call(0, Syscall::CapDelete(s(300)));
    }
    assert_eq!(outcome, DONE);
    m.run(60 * MS);
    for (i, &role) in roles.iter().enumerate() {
        assert_eq!(submissions(&m, role), before[i] + 1, "unit {i}");
    }
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_none));
}

// The claim of a remapped line writes a remapping entry too: the
// invalidation of the interrupt entry cache of each unit of the IOAPIC's
// segment comes before the IOAPIC's redirection entry (Interrupts, D-134).

#[test]
fn a_claim_of_a_remapped_line_invalidates_the_interrupt_entry_cache_before_the_redirection_entry() {
    let mut m = three_units();
    let roles = m.sys.node(0).segment_roles(0);
    let ioapic = m
        .sys
        .node(0)
        .ioapic_role(5)
        .expect("an IOAPIC serves GSI 5");
    let before: Vec<u64> = roles.iter().map(|&r| submissions(&m, r)).collect();
    let line = dodos_abi::IrqSource::line(5, true, false).raw();
    assert_eq!(m.call(0, claim(line, 50, 212)), WAITS);
    // Event by event: the IOAPIC's entry is written only after every unit
    // of the segment confirmed its invalidation.
    let mut seen_line_before_units = false;
    for _ in 0..20_000 {
        let lines = m
            .sys
            .node(0)
            .table
            .units
            .get(&ioapic)
            .is_some_and(|u| u.lines.contains_key(&5));
        let units_done = roles
            .iter()
            .zip(&before)
            .all(|(&r, &b)| submissions(&m, r) > b);
        seen_line_before_units |= lines && !units_done;
        if m.woken(0).is_some() {
            break;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => m.sys.move_clock(m.sys.now() + 100_000),
        }
        assert_eq!(m.sys.check(), Ok(()));
    }
    assert_eq!(
        m.woken(0),
        Some(&Ok(Output::Msi {
            address: 0,
            data: 0
        }))
    );
    assert!(!seen_line_before_units);
    let unit = &m.sys.node(0).table.units[&ioapic];
    assert!(unit.lines.contains_key(&5));
    // The remapping entry of the handle is in each unit of the segment.
    let handle = 50 - 48;
    for &role in &roles {
        assert!(m.sys.node(0).table.units[&role].remap.contains_key(&handle));
    }
}

// The holder's gate checks the operands of a unit request (Devices and
// shared hardware, the operands of each operation, D-134): a request that
// names what its requester does not hold changes nothing on the unit, is
// traced, and is answered done.

/// Three nodes; the holder of the IOMMU role, a requester, and a third
/// node whose numbers and cores the requester names.
fn holder_requester_third() -> (S, u16, u16, u16) {
    let m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let holder = m.sys.role(0).holder();
    let requester = (0..3).find(|&n| n != holder).unwrap();
    let third = (0..3).find(|&n| n != holder && n != requester).unwrap();
    (m, holder, requester, third)
}

/// The holder judges `op` from `requester`; returns whether its gate
/// refused it, and checks that a refusal changed nothing on the unit.
fn judged(m: &mut S, holder: u16, requester: u16, role: usize, op: crate::rings::UnitOp) -> bool {
    let epoch = m.sys.role(role).epoch();
    let before = m.sys.node(holder).refused.0;
    let unit = m.sys.node(holder).table.units.get(&role).cloned();
    let model = m.sys.node_mut(holder);
    model.unit_requested(requester, Some((0, 1)), (role as u16, epoch, op));
    let refused = m.sys.node(holder).refused.0 == before + 1;
    if refused {
        let after = m.sys.node(holder).table.units.get(&role).cloned();
        assert_eq!(
            after.map(|u| (u.entries, u.lines)),
            unit.map(|u| (u.entries, u.lines))
        );
    }
    refused
}

fn hold_identifier(m: &mut S, node: u16, device: u16) {
    let generation = m.sys.member(node).generation();
    m.sys.edit_table(|t| {
        let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(node), generation);
        t.identifiers.insert(u32::from(device), taken.raw());
    });
}

#[test]
fn the_holder_refuses_an_attach_under_a_domain_number_of_another_node() {
    let (mut m, holder, requester, third) = holder_requester_third();
    hold_identifier(&mut m, requester, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let other = *m.sys.node(holder).domain_range(0, third).start();
    let attach = |domain| crate::rings::UnitOp::Attach {
        device: 0x10,
        domain,
        root: 1 << 30,
    };
    assert!(judged(&mut m, holder, requester, 0, attach(other)));
    assert!(judged(&mut m, holder, requester, 0, attach(0)));
    assert!(!judged(&mut m, holder, requester, 0, attach(own)));
    assert_eq!(m.sys.node(holder).table.units[&0].domains[&0x10], own);
}

#[test]
fn the_holder_refuses_an_attach_of_a_device_whose_identifier_word_names_another_node() {
    let (mut m, holder, requester, third) = holder_requester_third();
    hold_identifier(&mut m, third, 0x10);
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let op = crate::rings::UnitOp::Attach {
        device: 0x10,
        domain: own,
        root: 1 << 30,
    };
    assert!(judged(&mut m, holder, requester, 0, op));
}

#[test]
fn the_holder_refuses_a_detach_of_a_context_entry_of_another_node() {
    let (mut m, holder, requester, third) = holder_requester_third();
    hold_identifier(&mut m, third, 0x10);
    let domain = *m.sys.node(holder).domain_range(0, third).start();
    let attach = crate::rings::UnitOp::Attach {
        device: 0x10,
        domain,
        root: 1 << 30,
    };
    assert!(!judged(&mut m, holder, third, 0, attach));
    let detach = crate::rings::UnitOp::Detach {
        device: 0x10,
        domain,
    };
    assert!(judged(&mut m, holder, requester, 0, detach));
    assert!(
        m.sys.node(holder).table.units[&0]
            .entries
            .contains_key(&0x10)
    );
}

#[test]
fn the_holder_refuses_an_invalidation_of_a_domain_number_of_another_node() {
    let (mut m, holder, requester, third) = holder_requester_third();
    let own = *m.sys.node(holder).domain_range(0, requester).start();
    let other = *m.sys.node(holder).domain_range(0, third).start();
    let invalidate = |domain| crate::rings::UnitOp::Invalidate { domain };
    assert!(judged(
        &mut m,
        holder,
        requester,
        0,
        invalidate(Some(other))
    ));
    assert!(judged(&mut m, holder, requester, 0, invalidate(None)));
    assert!(!judged(&mut m, holder, requester, 0, invalidate(Some(own))));
}

#[test]
fn the_holder_refuses_an_interrupt_entry_cache_invalidation_of_a_handle_of_another_nodes_core() {
    let (mut m, holder, requester, third) = holder_requester_third();
    // Node n runs on core n: its handles are n × 192 to n × 192 + 191.
    let handle = |core: u16| core * dodos_abi::x86_64::IRQ_TABLE_ENTRIES as u16 + 3;
    let op = |h| crate::rings::UnitOp::InterruptCache { handle: h };
    assert!(judged(&mut m, holder, requester, 0, op(handle(third))));
    assert!(!judged(&mut m, holder, requester, 0, op(handle(requester))));
}

#[test]
fn the_holder_of_an_ioapic_refuses_a_redirection_entry_aimed_at_another_nodes_core() {
    let (mut m, _, requester, third) = holder_requester_third();
    let role = m.sys.node(0).ioapic_role(5).unwrap();
    let holder = m.sys.role(role).holder();
    let (requester, third) = match holder == requester {
        true => (third, (0..3).find(|&n| n != holder && n != third).unwrap()),
        false => (requester, third),
    };
    let redirection = |target| crate::rings::UnitOp::Redirection {
        gsi: 5,
        claimed: true,
        level: true,
        low: false,
        target,
    };
    let handle = |core: u16| core * dodos_abi::x86_64::IRQ_TABLE_ENTRIES as u16 + 2;
    let remapped = |core| crate::rings::Target::Remapped {
        handle: handle(core),
        vector: 50,
    };
    // Core n has the APIC ID 2n (`machine`).
    let direct = |core: u16| crate::rings::Target::Direct {
        vector: 50,
        apic: 2 * u32::from(core),
    };
    assert!(judged(
        &mut m,
        holder,
        requester,
        role,
        redirection(remapped(third))
    ));
    assert!(judged(
        &mut m,
        holder,
        requester,
        role,
        redirection(direct(third))
    ));
    assert!(judged(
        &mut m,
        holder,
        requester,
        role,
        redirection(crate::rings::Target::Masked)
    ));
    assert!(!judged(
        &mut m,
        holder,
        requester,
        role,
        redirection(remapped(requester))
    ));
    // Masking a line another node claimed is refused too.
    let mask = crate::rings::UnitOp::Redirection {
        gsi: 5,
        claimed: false,
        level: true,
        low: false,
        target: crate::rings::Target::Masked,
    };
    assert!(judged(&mut m, holder, third, role, mask));
    assert!(!judged(&mut m, holder, requester, role, mask));
}

// Domain numbers at start-up (Start-up state: the start-up refuses a layout
// that leaves an IOMMU unit fewer than one domain number per node slot).

#[test]
fn the_start_up_refuses_a_unit_with_fewer_domain_numbers_than_node_slots() {
    let mut machine = machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.iommus[0].domains = 4;
    assert_eq!(
        System::boot(&machine).err(),
        Some(crate::boot::Refusal::Nodes)
    );
    machine.iommus[0].domains = 5;
    assert!(System::boot(&machine).is_ok());
}

#[test]
fn each_node_slot_takes_its_domain_numbers_from_its_own_range() {
    let machine = machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    let m = S::on(&machine);
    let total = machine.iommus[0].domains;
    let per = (total - 2) / 3;
    for slot in 0..3u16 {
        let range = m.sys.node(0).domain_range(0, slot);
        assert_eq!(u32::from(*range.start()), 1 + u32::from(slot) * per);
        assert_eq!(u32::from(*range.end()), (u32::from(slot) + 1) * per);
        assert!(!range.contains(&0) && !range.contains(&((total - 1) as u16)));
    }
}

// One node on a machine with a core of fence class 0 (Start-up state,
// D-134): every usable core if they lie in one window and have the same
// state features, otherwise the core firmware booted.

#[test]
fn one_node_on_a_machine_of_class_0_holds_the_boot_core_alone_when_the_cores_differ_in_state() {
    let mut machine = machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.cores[2].fence = 0;
    assert_eq!(machine.initial_nodes(), vec![vec![0, 1, 2]]);
    // A second kind, with another set of state features, on core 1.
    let mut other = machine.kinds[0];
    other.features ^= 1 << 2;
    machine.kinds.push(other);
    machine.cores[1].kind = 1;
    assert_eq!(machine.initial_nodes(), vec![vec![0]]);
}

// L2 (A node of several cores, "The order of L2 and its bound", D-134): a
// lock found through an object the call holds, earlier in the order, is
// tried; the trace keeps the order the call took its locks in.

#[test]
fn under_l2_a_call_records_its_locks_in_the_order_it_took_them_with_tries_apart() {
    let mut machine = one_node(crate::Arch::X86_64, 2);
    machine.locking = crate::Locking::L2;
    let mut k = N::on(&machine);
    k.place_first(0);
    let receiver = k.thread_on(300, 0);
    k.ok(k.retype(ObjectKind::Endpoint, 0, 310));
    k.ok(k.retype(ObjectKind::Reply, 0, 311));
    let recv = Syscall::Recv {
        endpoint: s(310),
        reply: s(311),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.call(receiver, recv), WAITS);
    let call = Syscall::Call {
        endpoint: s(310),
        message: crate::Message::default(),
        range: Range::NONE,
        wait: 0,
    };
    assert_eq!(k.call(k.t, call), WAITS);
    // The delivery lends the caller's context: the model finds it after the
    // lock of the core it queues on, earlier in the order, so it is a try,
    // not a wait; the trace is the order of the call, not a sorted list.
    let context = k.m.threads[&k.t].home.unwrap();
    let steps = &k.m.locks.steps;
    let core = steps
        .iter()
        .position(|s| matches!(s, crate::locks::Step::Lock(crate::locks::LockId::Core(_))));
    let tried = steps
        .iter()
        .position(|s| *s == crate::locks::Step::Try(crate::locks::LockId::Sched(context)));
    assert!(core.is_some() && tried > core, "{steps:?}");
}

// A requester takes the domain number of a new iospace from its own node
// slot's range (Devices and shared hardware, domain numbers, D-134).

#[test]
fn an_attach_through_another_nodes_unit_uses_a_domain_number_of_the_requesters_range() {
    let mut m = S::on(&machine(crate::Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let holder = m.sys.role(0).holder();
    let requester = (0..3).find(|&n| n != holder).unwrap();
    let iospace = m.retype(requester, ObjectKind::IoSpace, 0, 150);
    m.ok(requester, iospace);
    let iommu = s(slots_of(
        m.sys.node(requester),
        m.t[requester as usize],
        ObjectKind::Iommu,
    )[0]);
    let attach = Syscall::IoSpaceAttach {
        iommu,
        iospace: s(150),
        device: 0x10,
    };
    if m.call(requester, attach) == WAITS {
        m.run(30 * MS);
    }
    assert_eq!(m.woken(requester), Some(&DONE));
    let unit = &m.sys.canonical().units[&0];
    assert_eq!(unit.entries.get(&0x10).map(|e| e.0), Some(requester));
    let domain = unit.domains[&0x10];
    assert!(
        m.sys
            .node(holder)
            .domain_range(0, requester)
            .contains(&domain)
    );
}
