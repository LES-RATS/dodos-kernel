//! Pass 43: directed scenarios closing every contract-bound gap under the full
//! check at every step.

use super::pass17::kill;
use super::units::{IOMMU_ROLE, attach, role_slot};
use super::*;
use crate::system::Event;
use crate::{Map, Message, Range, Transfer};
use dodos_abi::{
    CSPACE_MAX_SLOTS, DEBUG_WRITE_MAX, ENTROPY_MAX_BYTES, IDLE_STATES_MAX, IOSPACE_MAX_DEVICES,
    IrqSource, MAX_CAPS, MAX_WORDS, NODE_WATCH_SLOTS, PENDING_STEP, REMOVAL_STEP,
    REQUESTS_PER_PEER, RESERVES_MAX, RING_ANSWER_RESERVE, RING_KERNEL_ENTRIES, RING_USER_ENTRIES,
    ROLES_PLAIN_MAX, SCHED_MAX_DEBT, SCHED_MIN_BUDGET_NS, SENDS_PER_PEER,
    SENDS_RESERVED_FOR_MANAGE, START_SLOT_CSPACE, START_SLOT_DEBUG, START_SLOT_IRQ_CONTROL,
    START_SLOT_MAILBOX, START_SLOT_SCHED, TIME_EVENT_STEP, TREE_DEPTH,
};

fn frame_rights() -> u32 {
    Rights::READ
        .union(Rights::WRITE)
        .union(Rights::SEND)
        .union(Rights::DERIVE)
        .bits()
}

fn trim_start_sched(m: &mut S, node: u16) {
    let sched = m
        .sys
        .node(node)
        .object_at(m.t[node as usize], s(START_SLOT_SCHED))
        .expect("start sched");
    if m.sys.node(node).scheds[&sched].budget > 2 * MS {
        let core = m.sys.node(node).core_number(0) as u64;
        m.ok(
            node,
            Syscall::SchedConfigure {
                sched: s(START_SLOT_SCHED),
                node: node_cap(node),
                core,
                budget: MS,
                period: 10 * MS,
                class: 1,
            },
        );
    }
}

fn hand_role_to(m: &mut S, role: usize, target: u16) {
    let holder = m.sys.role(role).holder();
    if holder == target {
        return;
    }
    let cap = role_slot(m, holder, role);
    let out = m.call(
        holder,
        Syscall::RoleHand {
            role: cap,
            node: node_cap(target),
        },
    );
    assert_eq!(out, WAITS);
    m.run(30 * MS);
    assert_eq!(m.sys.role(role).holder(), target);
}

fn spawn_thread_min_budget(m: &mut S, node: u16, slot: u32, core: u64) -> ObjId {
    trim_start_sched(m, node);
    let ut = untyped_slot(m.sys.node(node), m.t[node as usize]);
    let mut calls = thread_calls(ut, node, slot, core);
    if let Syscall::SchedConfigure { budget, .. } = &mut calls[4] {
        *budget = SCHED_MIN_BUDGET_NS;
    }
    for call in calls {
        m.ok(node, call);
    }
    m.sys
        .node(node)
        .object_at(m.t[node as usize], s(slot))
        .expect("spawned thread")
}

/// Fires events matching `pred` until none matches (bounded), checking after
/// every event.
fn fire_while(m: &mut S, pred: impl Fn(&System, Event) -> bool) {
    for _ in 0..10_000 {
        let next = m.sys.events().into_iter().find(|&ev| pred(&m.sys, ev));
        let Some(ev) = next else { break };
        m.sys.fire(ev);
        assert_eq!(m.sys.check(), Ok(()));
        m.sample_bounds();
    }
}

fn is_ring_for(sys: &System, ev: Event, node: u16) -> bool {
    matches!(ev, Event::Action(i) if sys.actions()[i].1 == crate::nodes::HwAction::Ring { node })
}

/// Contract `RING_KERNEL_ENTRIES` (64), `RING_ANSWER_RESERVE` (34 = 16 + 8 + 10),
/// `REQUESTS_PER_PEER` (16), `SENDS_PER_PEER` (8, 6 without `manage`, 2 with `manage`):
/// Fill kernel ring 1 -> 3 to 30 non-answer entries (`64 - 34`), then have node 3
/// send 15 `UnitRequest`s, 1 `ReportAsk` (answered by a 28-item, 11-entry `Report`),
/// and 8 `NodeSend`s (6 without `manage`, 2 with `manage`) while node 3 does not
/// consume 1 -> 3. Node 1's answers push 1 -> 3 to **exactly 64 entries** (`30 + 15 + 11 + 8`),
/// and draining restores every credit cleanly.
#[test]
fn kernel_ring_reaches_exact_64_using_full_34_entry_answer_reserve() {
    let mut mach = machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]);
    // Two IOMMUs: node 1 holds IOMMU 0 (to answer 15 UnitRequests from node 3),
    // node 2 holds IOMMU 1 (whose immediate class-2 successor when 2 dies is
    // node 3, which asks node 1 for a 28-item, 11-entry Report!).
    mach.iommus.push(crate::boot::IommuUnit {
        segment: 1,
        domains: 256,
    });
    mach.floors.floor_node = 6 * mach.geometry.chunk();
    let mut m = S::on(&mach);

    // Role 0 = IOMMU 0 (held by node 1), role 1 = IOMMU 1 (hand to node 2).
    let iommu0 = 0usize;
    let iommu1 = 1usize;
    assert_eq!(m.sys.role(iommu0).holder(), 1);
    hand_role_to(&mut m, iommu1, 2);

    // Give node 1 29 MSI handles on segment 1 (IOMMU 1) so a ReportAsk from node 3
    // produces a full 28-item (11-entry) Report with `next != REPORT_END`!
    for i in 0..29u32 {
        let claim = Syscall::IrqClaim {
            control: s(START_SLOT_IRQ_CONTROL),
            source: IrqSource::msi(1, 0, (i % 32) as u8, 0).raw(),
            core: 1,
            vector: u64::from(48 + i),
            destination: s(200 + i),
        };
        if m.call(1, claim) == WAITS {
            m.run(20 * MS);
            assert!(matches!(m.woken(1), Some(Ok(Output::Msi { .. }))));
        }
    }
    assert_eq!(m.sys.node(1).items(iommu1).len(), 29);

    // Node 3 grants 32 frames to node 1 so node 1 can generate 30 `Release`s
    // toward node 3 (each `Release` takes 1 entry on kernel ring 1 -> 3, stopping
    // at 30 = `64 - 34` while the remaining 2 stay `release_pending`).
    let reply1 = m.retype(1, ObjectKind::Reply, 0, 120);
    m.ok(1, reply1);
    let ut3 = untyped_slot(m.sys.node(3), m.t[3]);
    for i in 0..32u32 {
        m.ok(3, retype_from(ut3, ObjectKind::Frame, 1, 200 + i));
    }
    let rx1 = spawn_thread_min_budget(&mut m, 1, 130, 1);
    for g in 0..8u32 {
        let first = g * 4;
        assert_eq!(
            m.call_by(
                1,
                rx1,
                Syscall::Recv {
                    endpoint: s(START_SLOT_MAILBOX),
                    reply: s(120),
                    range: Range {
                        first: s(260 + first),
                        count: 4,
                    },
                    wait: 0,
                },
            ),
            WAITS
        );
        let send = Syscall::NodeSend {
            node: node_cap(1),
            message: Message {
                caps: (first..first + 4)
                    .map(|i| Transfer::Derive(s(200 + i), frame_rights()))
                    .collect(),
                ..Message::default()
            },
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call(3, send), WAITS);
        m.settle();
        assert_eq!(m.woken(3), Some(&Ok(Output::Sent { flags: 0 })));
    }

    // Prepare node 3's IoSpace + 16 mapped frames + 16 worker threads (to unmap them,
    // sending 15 UnitRequests + 1 queued on the 17th credit to node 1 on IOMMU 0),
    // plus 9 sender threads for `NodeSend`s (6 without `manage`, 1 refused, 2 with
    // `manage`, 1 refused).
    let ios3 = m.retype(3, ObjectKind::IoSpace, 0, 150);
    m.ok(3, ios3);
    let attach3 = attach(&m, 3, 150, 0x10);
    assert_eq!(m.call(3, attach3), WAITS);
    m.run(20 * MS);
    assert_eq!(m.woken(3), Some(&DONE));

    let tables3 = table_slot(m.sys.node(3), m.t[3]);
    let mut unit_threads = Vec::new();
    for i in 0..16u32 {
        let base = 300 + 4 * i;
        let t = spawn_thread_min_budget(&mut m, 3, base, 3);
        m.ok(3, retype_from(ut3, ObjectKind::Frame, 1, base + 3));
        m.ok(
            3,
            Syscall::IoSpaceMap(Map {
                space: s(150),
                frame: s(base + 3),
                untyped: s(tables3),
                address: 0x1000_0000 + (u64::from(i) + 1) * 0x10_0000,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            }),
        );
        unit_threads.push((t, base + 3));
    }
    // Derive a `node(1)` capability WITHOUT `Rights::MANAGE` in slot 140.
    m.ok(
        3,
        Syscall::CapDerive {
            source: node_cap(1),
            destination: s(140),
            rights: Rights::SEND.bits(),
            badge: 0,
        },
    );
    let mut send_threads = Vec::new();
    for i in 0..9u32 {
        let base = 370 + 3 * i;
        send_threads.push(spawn_thread_min_budget(&mut m, 3, base, 3));
    }

    // Kill node 2 (by node 0): node 3 is the immediate successor of node 2 for IOMMU 1.
    // Step until node 3 holds IOMMU 1 and has sent a ReportAsk to node 1, without
    // letting node 1 consume that ReportAsk yet!
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(2));
    let mut ask_sent = false;
    for _ in 0..5_000 {
        if m.sys.role(iommu1).holder() == 3
            && m.sys
                .node(3)
                .rebuilds
                .get(&iommu1)
                .is_some_and(|r| r.node == 1 && r.asked.is_some())
        {
            ask_sent = true;
            break;
        }
        let events = m.sys.events();
        let next = events
            .into_iter()
            .find(|&ev| !(m.sys.role(iommu1).holder() == 3 && is_ring_for(&m.sys, ev, 1)));
        match next {
            Some(ev) => {
                m.sys.fire(ev);
                assert_eq!(m.sys.check(), Ok(()));
                m.sample_bounds();
            }
            None => {
                let at = m.sys.now() + 1_000_000;
                m.sys.move_clock(at);
            }
        }
    }
    assert!(ask_sent, "node 3 must send ReportAsk to node 1 for IOMMU 1");

    // Step 1: Node 1 deletes all 32 remote frames -> 30 `Release`s enter kernel ring
    // 1 -> 3 (`64 - 34 = 30`), and the 31st & 32nd set `release_pending` (hitting the
    // 34-entry answer reserve!).
    for i in 0..32u32 {
        m.ok(1, Syscall::CapDelete(s(260 + i)));
    }
    let k13_used = |m: &S| {
        let t = m.sys.canonical();
        t.rings[1][3]
            .kernel
            .produced
            .wrapping_sub(t.beacons[3].consumed[1].0)
    };
    assert_eq!(
        k13_used(&m),
        RING_KERNEL_ENTRIES - RING_ANSWER_RESERVE,
        "non-answer messages must stop at exactly 30 entries (leaving 34-entry answer reserve)"
    );

    // Step 2: Node 3 submits 16 `IoSpaceUnmap` unit requests to IOMMU 0 (holder = node 1).
    // Combined with the 1 `ReportAsk` already in flight, the first 15 fill all 16 request
    // credits toward node 1 (`REQUESTS_PER_PEER = 16`). The 16th `IoSpaceUnmap` (17th
    // request) stays queued in `unit_requests`!
    for &(thread, frame_slot) in &unit_threads {
        assert_eq!(
            m.call_by(3, thread, Syscall::IoSpaceUnmap(s(frame_slot))),
            WAITS
        );
    }
    assert_eq!(
        m.sys.node(3).peers[1].credits[..REQUESTS_PER_PEER]
            .iter()
            .filter(|c| c.is_some())
            .count(),
        REQUESTS_PER_PEER
    );

    // Step 3: Node 3 sends 6 `NodeSend`s without `manage`, tries a 7th without `manage`
    // (`Busy`), then sends 2 with `manage` (`SENDS_PER_PEER = 8`), tries a 9th (`Busy`).
    for &thread in &send_threads[..SENDS_PER_PEER - SENDS_RESERVED_FOR_MANAGE] {
        let call = Syscall::NodeSend {
            node: s(140),
            message: Message::default(),
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call_by(3, thread, call), WAITS);
    }
    // 7th without manage -> Busy!
    assert_eq!(
        m.call_by(
            3,
            send_threads[6],
            Syscall::NodeSend {
                node: s(140),
                message: Message::default(),
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Busy)
    );
    // 7th and 8th with manage -> Waits!
    for &thread in &send_threads[6..8] {
        let call = Syscall::NodeSend {
            node: node_cap(1),
            message: Message::default(),
            doorbell: SlotAddr::NONE,
        };
        assert_eq!(m.call_by(3, thread, call), WAITS);
    }
    // 9th with manage -> Busy!
    assert_eq!(
        m.call_by(
            3,
            send_threads[8],
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message::default(),
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Busy)
    );

    // Step 4: Let node 1 process its rings and unit confirmations WITHOUT letting
    // node 3 consume kernel ring 1 -> 3 (hold back node 3's Ring action, IPI, and
    // Timer so node 3's recovery reserve does not poll ring 1 -> 3 early).
    // All 24 answers (15 UnitAnswers + 1 11-entry Report + 8 SendAnswers = 34 entries)
    // land in the 34-entry answer reserve!
    fire_while(&mut m, |sys, ev| {
        !is_ring_for(sys, ev, 3)
            && !matches!(
                ev,
                Event::Ipi { node: 3, .. } | Event::Timer { node: 3, .. }
            )
    });
    assert_eq!(
        k13_used(&m),
        RING_KERNEL_ENTRIES,
        "30 non-answers + 15 UnitAnswers + 11-entry Report + 8 SendAnswers = 64 entries"
    );

    // Step 5: Now let everything drain and settle cleanly.
    m.run(100 * MS);
    assert_eq!(k13_used(&m), 0);
    assert!(m.sys.node(3).rebuilds.is_empty());
    assert_eq!(m.sys.node(1).items(iommu1).len(), 29);
}

/// Contract `UNIT_REQUESTS` (64) & holder's unit service table (`16 * (n - 1)`):
/// On a 5-node machine, peers 0, 2, 3, 4 each send 16 `IoSpaceUnmap` `UnitRequest`s
/// to holder 1 before the unit confirms, filling holder 1's `unit_service` to
/// **64 entries** (`REQUESTS_PER_PEER * (5 - 1)`). Confirming the unit drains all 64 cleanly.
#[test]
fn unit_service_table_reaches_16_per_peer_across_four_peers() {
    let mut mach = machine(Arch::X86_64, 5, &[&[0], &[1], &[2], &[3], &[4]]);
    mach.floors.floor_node = 5 * mach.geometry.chunk();
    let mut m = S::on(&mach);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);

    // Each peer 0, 2, 3, 4 attaches one device, maps 16 frames locally, and prepares
    // 16 worker threads to call `IoSpaceUnmap` (each sends a `UnitRequest` to holder 1).
    let mut calls_by_peer: Vec<(u16, Vec<(ObjId, Syscall)>)> = Vec::new();
    for peer in [0u16, 2, 3, 4] {
        let ios = m.retype(peer, ObjectKind::IoSpace, 0, 150);
        m.ok(peer, ios);
        let att = attach(&m, peer, 150, 0x10 + u64::from(peer));
        assert_eq!(m.call(peer, att), WAITS);
        m.run(20 * MS);
        assert_eq!(m.woken(peer), Some(&DONE));

        let ut = untyped_slot(m.sys.node(peer), m.t[peer as usize]);
        let tables = table_slot(m.sys.node(peer), m.t[peer as usize]);
        let mut peer_calls = Vec::new();
        for i in 0..REQUESTS_PER_PEER as u32 {
            let base = 170 + 4 * i;
            let t = spawn_thread_min_budget(&mut m, peer, base, u64::from(peer));
            m.ok(peer, retype_from(ut, ObjectKind::Frame, 1, base + 3));
            m.ok(
                peer,
                Syscall::IoSpaceMap(Map {
                    space: s(150),
                    frame: s(base + 3),
                    untyped: s(tables),
                    address: 0x1000_0000 + (u64::from(i) + 1) * 0x10_0000,
                    first: 0,
                    count: 1,
                    rights: Rights::READ.bits(),
                }),
            );
            peer_calls.push((t, Syscall::IoSpaceUnmap(s(base + 3))));
        }
        calls_by_peer.push((peer, peer_calls));
    }

    // Hold IOMMU confirmation briefly (< IOMMU_WAIT_NS = 10 ms) while node 1's
    // recovery reserve ingests all 4 * 16 = 64 requests into `unit_service`.
    m.sys.break_unit(IOMMU_ROLE);
    for (peer, calls) in calls_by_peer {
        for (thread, call) in calls {
            assert_eq!(m.call_by(peer, thread, call), WAITS);
        }
    }
    m.run(2 * MS);
    assert_eq!(
        m.sys.node(1).unit_service.len(),
        REQUESTS_PER_PEER * 4,
        "unit_service must hold all 16 * (5 - 1) = 64 concurrent requests"
    );

    // Let confirmations and answers drain cleanly before `IOMMU_WAIT_NS`.
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = false);
    m.run(50 * MS);
    assert!(m.sys.node(1).unit_service.is_empty());
}

/// Contract `IOSPACE_MAX_DEVICES` (16), `IRQ_TABLE_ENTRIES` (192), 193rd claim
/// refused (`Busy`), and `Report` pagination (`28` items, `11` ring entries,
/// `ceil(208 / 28) = 8` asks across one rebuild):
/// Node 0 attaches 16 devices to one `IoSpace` (17th -> `Busy`), claims all 192
/// vectors (48..=239) on core 0 (193rd -> `Busy`), giving 208 items on IOMMU 0.
/// Killing the IOMMU holder causes successor node 2 to rebuild via **8 asks**
/// (7 full 28-item pages + 1 final 12-item page), preserving every device and handle.
#[test]
fn iospace_16_devices_192_irq_handles_and_8_ask_rebuild() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    mach.floors.floor_node = 5 * mach.geometry.chunk();
    let mut m = S::on(&mach);
    hand_role_to(&mut m, IOMMU_ROLE, 1);

    // Attach 16 devices to one IoSpace; 17th must be refused with Busy.
    let ios = m.retype(0, ObjectKind::IoSpace, 0, 150);
    m.ok(0, ios);
    for d in 0..IOSPACE_MAX_DEVICES as u64 {
        let call = attach(&m, 0, 150, 0x10 + d);
        assert_eq!(m.call(0, call), WAITS);
        m.settle();
        assert_eq!(m.woken(0), Some(&DONE));
    }
    let seventeenth = attach(&m, 0, 150, 0x10 + IOSPACE_MAX_DEVICES as u64);
    assert_eq!(m.call(0, seventeenth), Err(Error::Busy));

    // Claim all 192 IRQ handles (vectors 48..=239) on core 0.
    let entries = dodos_abi::x86_64::IRQ_TABLE_ENTRIES as u32;
    for i in 0..entries {
        let claim = Syscall::IrqClaim {
            control: s(START_SLOT_IRQ_CONTROL),
            source: IrqSource::msi(0, (i / 32) as u8, (i % 32) as u8, 0).raw(),
            core: 0,
            vector: u64::from(dodos_abi::x86_64::IRQ_VECTOR_FIRST) + u64::from(i),
            destination: s(200 + i),
        };
        assert_eq!(m.call(0, claim), WAITS);
        m.settle();
        assert!(matches!(m.woken(0), Some(Ok(Output::Msi { .. }))));
    }
    // 193rd claim on core 0 (reusing a vector in range) must be refused Busy.
    assert_eq!(
        m.call(
            0,
            Syscall::IrqClaim {
                control: s(START_SLOT_IRQ_CONTROL),
                source: IrqSource::msi(0, 7, 0, 0).raw(),
                core: 0,
                vector: u64::from(dodos_abi::x86_64::IRQ_VECTOR_FIRST),
                destination: s(200 + entries),
            },
        ),
        Err(Error::Busy)
    );
    assert_eq!(
        m.sys.node(0).items(IOMMU_ROLE).len(),
        IOSPACE_MAX_DEVICES + entries as usize
    );

    // Kill holder node 1; node 2 takes over and rebuilds via ceil(208 / 28) = 8 asks.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(80 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert_eq!(m.bounds.max_node_handles, entries as usize);
    assert_eq!(m.bounds.max_iospace_devices, IOSPACE_MAX_DEVICES);
    assert_eq!(m.bounds.max_rebuild_asks_per_node, 8);
    assert!(m.bounds.report_28_more);
    assert!(m.bounds.irq_193rd_refused);
    assert!(m.bounds.iospace_17th_refused);
}

/// Contract `NODE_WATCH_SLOTS` (4, 5th refused) and `IDLE_STATES_MAX` (15, 16th refused):
/// Register all 4 watch slots and refuse slot 4; install all 15 idle states and
/// refuse 16 states.
#[test]
fn node_watch_4_slots_and_idle_15_states_and_refusals() {
    let mut m = S::on(&machine(Arch::Aarch64, 2, &[&[0], &[1]]));
    let ut0 = untyped_slot(m.sys.node(0), m.t[0]);

    // Register all 4 watch slots (0..4) on node 0.
    for i in 0..NODE_WATCH_SLOTS as u32 {
        m.ok(0, retype_from(ut0, ObjectKind::Notify, 0, 150 + i));
        m.ok(
            0,
            Syscall::NodeWatch {
                node: node_cap(0),
                index: u64::from(i),
                notify: s(150 + i),
            },
        );
    }
    // 5th slot (index 4) -> Invalid.
    m.ok(0, retype_from(ut0, ObjectKind::Notify, 0, 154));
    assert_eq!(
        m.call(
            0,
            Syscall::NodeWatch {
                node: node_cap(0),
                index: NODE_WATCH_SLOTS,
                notify: s(154),
            },
        ),
        Err(Error::Invalid)
    );

    // Install all 15 idle states (`IDLE_STATES_MAX`) on core 0.
    let mut words = Vec::with_capacity(IDLE_STATES_MAX * dodos_abi::IDLE_ENTRY_WORDS);
    for i in 0..IDLE_STATES_MAX as u64 {
        words.push((i + 1) * 10_000); // min residency
        words.push((i + 1) * 5_000); // exit latency
        words.push(dodos_abi::IDLE_METHOD_PSCI | (((1u64 << 16) | i) << 32));
        words.push(dodos_abi::IDLE_FLAG_LOSES_CONTEXT);
    }
    m.ok(
        0,
        Syscall::NodeIdle {
            node: node_cap(0),
            core: 0,
            words: words.clone(),
        },
    );
    // 16 states (64 words) -> Invalid.
    words.extend_from_slice(&[200_000, 100_000, dodos_abi::IDLE_METHOD_PSCI, 0]);
    assert_eq!(
        m.call(
            0,
            Syscall::NodeIdle {
                node: node_cap(0),
                core: 0,
                words,
            },
        ),
        Err(Error::Invalid)
    );

    // Kill node 1 and verify all 4 watch notifies fire.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(20 * MS);
    for i in 0..NODE_WATCH_SLOTS as u32 {
        assert!(
            matches!(m.call(0, Syscall::Poll(s(150 + i))), Ok(Output::Word(w)) if w != 0),
            "watch slot {i} must fire on state change"
        );
    }
    assert_eq!(m.bounds.max_watch_slots, NODE_WATCH_SLOTS as usize);
    assert!(m.bounds.watch_slot_5th_refused);
    assert_eq!(m.bounds.max_idle_states, IDLE_STATES_MAX);
    assert!(m.bounds.idle_16th_refused);
}

/// Contract `RESERVES_MAX` (8, min 2) and `ROLES_PLAIN_MAX` (64):
/// Boot a multi-node machine with 8 reserves and 64 plain roles, claim reserve #7
/// and plain role #63, hand plain role #63 across nodes, kill the holder, and
/// verify boot refuses out-of-range counts (1 reserve, 9 reserves, 65 plain roles).
#[test]
fn eight_reserves_and_64_plain_roles_and_boot_refusals() {
    let mut bad = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    bad.floors.reserves = 1;
    assert_eq!(Model::boot(&bad).err(), Some(crate::boot::Refusal::Floors));
    bad.floors.reserves = RESERVES_MAX + 1;
    assert_eq!(Model::boot(&bad).err(), Some(crate::boot::Refusal::Floors));
    bad.floors.reserves = 2;
    bad.floors.plain_roles = ROLES_PLAIN_MAX;
    assert_eq!(Model::boot(&bad).err(), Some(crate::boot::Refusal::Floors));

    let mut mach = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    mach.floors.reserves = RESERVES_MAX;
    mach.floors.plain_roles = ROLES_PLAIN_MAX - 1;
    let mut m = S::on(&mach);

    let last_reserve = m
        .sys
        .canonical()
        .role_records
        .iter()
        .rposition(|r| r.kind == dodos_abi::RoleKind::Reserve)
        .expect("reserve roles");
    let last_plain = m
        .sys
        .canonical()
        .role_records
        .iter()
        .rposition(|r| r.kind == dodos_abi::RoleKind::Plain)
        .expect("plain roles");

    let r_cap = role_slot(&m, 1, last_reserve);
    let p_cap = role_slot(&m, 1, last_plain);
    assert!(matches!(
        m.call(1, Syscall::RoleClaim(r_cap)),
        Ok(Output::Epoch(_))
    ));
    assert!(matches!(
        m.call(1, Syscall::RoleClaim(p_cap)),
        Ok(Output::Epoch(_))
    ));

    // Hand plain role #63 from node 1 to node 0, then back to node 1.
    assert!(matches!(
        m.call(
            1,
            Syscall::RoleHand {
                role: p_cap,
                node: node_cap(0),
            },
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(m.sys.role(last_plain).holder(), 0);
    let p0 = role_slot(&m, 0, last_plain);
    assert!(matches!(
        m.call(
            0,
            Syscall::RoleHand {
                role: p0,
                node: node_cap(1),
            },
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(m.sys.role(last_plain).holder(), 1);

    // Kill node 1; plain role #63 is claimable once dead (and fenced), reserve #7 once dead.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    let r0 = role_slot(&m, 0, last_reserve);
    assert!(matches!(
        m.call(0, Syscall::RoleClaim(p0)),
        Ok(Output::Epoch(_))
    ));
    assert!(matches!(
        m.call(0, Syscall::RoleClaim(r0)),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(m.bounds.max_reserves, RESERVES_MAX as usize);
    assert_eq!(m.bounds.max_plain_roles, ROLES_PLAIN_MAX as usize);
}

/// Contract recovery reserve (`1/64` of core 0, `10 ms` period):
/// Core 0 admits contexts up to `1 - 1/64` and refuses exceeding it (`Busy`),
/// while core 1 admits up to 100%.
#[test]
fn recovery_reserve_refuses_core0_over_63_64_and_allows_core1_full() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0, 1]]));
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    // Core 0 already has the start-up context; check exact admission boundary.
    // Period = 64 ms: 1/64 = 1 ms reserved, so max total budget on core 0 is 63 ms.
    // Reconfigure start-up context to 63 ms / 64 ms -> succeeds!
    m.ok(
        0,
        Syscall::SchedConfigure {
            sched: s(START_SLOT_SCHED),
            node: node_cap(0),
            core: 0,
            budget: 63 * MS,
            period: 64 * MS,
            class: 1,
        },
    );
    assert_eq!(
        m.sys.node(0).cores[0].admitted,
        dodos_abi::ADMISSION_WHOLE_CORE
    );
    // Asking for even 10 µs more on core 0 must be refused Budget (since 1/64
    // is reserved for the recovery reserve).
    m.ok(0, retype_from(ut, ObjectKind::Sched, 0, 150));
    assert_eq!(
        m.call(
            0,
            Syscall::SchedConfigure {
                sched: s(150),
                node: node_cap(0),
                core: 0,
                budget: SCHED_MIN_BUDGET_NS,
                period: 64 * MS,
                class: 1,
            },
        ),
        Err(Error::Budget)
    );
    assert!(m.bounds.admission_refused);
    // Core 1 has no recovery reserve: 100% bandwidth (64 ms / 64 ms) succeeds.
    m.ok(
        0,
        Syscall::SchedConfigure {
            sched: s(150),
            node: node_cap(0),
            core: 1,
            budget: 64 * MS,
            period: 64 * MS,
            class: 1,
        },
    );
    assert!(m.bounds.admission_refused);
}

/// Contract `MAX_WORDS` (64), `MAX_CAPS` / `SEND_GRANTS_MAX` (4), and refusals
/// (`65` words -> `Invalid`, `5` grants -> `Invalid`) on `node_send`.
#[test]
fn node_send_64_words_4_grants_and_over_limit_refusals() {
    let mut m = S::two();
    let ut0 = untyped_slot(m.sys.node(0), m.t[0]);
    for i in 0..5u32 {
        m.ok(0, retype_from(ut0, ObjectKind::Frame, 1, 150 + i));
    }
    let reply1 = m.retype(1, ObjectKind::Reply, 0, 120);
    m.ok(1, reply1);
    let rx1 = spawn_thread_min_budget(&mut m, 1, 130, 1);
    assert_eq!(
        m.call_by(
            1,
            rx1,
            Syscall::Recv {
                endpoint: s(START_SLOT_MAILBOX),
                reply: s(120),
                range: Range {
                    first: s(200),
                    count: 4,
                },
                wait: 0,
            },
        ),
        WAITS
    );

    // 65 words -> Invalid.
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    words: vec![1; MAX_WORDS + 1],
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Invalid)
    );
    // 5 frame grants -> Invalid.
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    caps: (0..MAX_CAPS as u32 + 1)
                        .map(|i| Transfer::Derive(s(150 + i), frame_rights()))
                        .collect(),
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Invalid)
    );

    // 64 words + 4 frame grants -> 10 user-ring entries, delivered whole.
    let payload: Vec<u64> = (0..MAX_WORDS as u64).map(|w| 0x1000 + w).collect();
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    label: 42,
                    words: payload.clone(),
                    caps: (0..MAX_CAPS as u32)
                        .map(|i| Transfer::Derive(s(150 + i), frame_rights()))
                        .collect(),
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        WAITS
    );
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    let delivered = m.sys.node(1).woken(rx1).expect("receiver woken");
    match delivered {
        Ok(Output::Message(d)) => {
            assert_eq!(d.words, payload);
            assert_eq!(d.received.header.caps(), MAX_CAPS);
        }
        other => panic!("expected 64-word delivery, got {other:?}"),
    }
    assert_eq!(m.bounds.max_msg_words, MAX_WORDS);
    assert_eq!(m.bounds.max_send_grants, MAX_CAPS);
    assert!(m.bounds.msg_words_over_64_refused);
    assert!(m.bounds.send_grants_over_4_refused);
}

/// Contract `PENDING_STEP` (32 pending signals and 32 pending releases per event):
/// Grant 40 frames from node 0 to node 1. Fill 1 -> 0 user ring to 64 and signal
/// 36 grants (`pending_signals = 36 > 32`); drain and verify `PENDING_STEP`.
/// Then fill 1 -> 0 kernel ring to 30 (`64 - 34`) and delete 36 remote frames
/// (`pending_releases = 36 > 32`); drain and verify bounded release flushing.
#[test]
fn pending_signals_and_pending_releases_both_exceed_32_step() {
    let mut mach = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    mach.floors.floor_node = 5 * mach.geometry.chunk();
    let mut m = S::on(&mach);

    let ut0 = untyped_slot(m.sys.node(0), m.t[0]);
    let bell0 = retype_from(ut0, ObjectKind::Notify, 0, 120);
    m.ok(0, bell0);
    let reply1 = m.retype(1, ObjectKind::Reply, 0, 120);
    m.ok(1, reply1);
    let rx1 = spawn_thread_min_budget(&mut m, 1, 130, 1);

    // Grant 72 frames (18 batches of 4): 36 for signal + release test, 36 for filling
    // the kernel ring to 30 before triggering 36 pending releases.
    for i in 0..72u32 {
        m.ok(0, retype_from(ut0, ObjectKind::Frame, 1, 200 + i));
    }
    for batch in 0..18u32 {
        let first = batch * 4;
        assert_eq!(
            m.call_by(
                1,
                rx1,
                Syscall::Recv {
                    endpoint: s(START_SLOT_MAILBOX),
                    reply: s(120),
                    range: Range {
                        first: s(200 + first),
                        count: 4,
                    },
                    wait: 0,
                },
            ),
            WAITS
        );
        assert_eq!(
            m.call(
                0,
                Syscall::NodeSend {
                    node: node_cap(1),
                    message: Message {
                        caps: (first..first + 4)
                            .map(|i| Transfer::Derive(s(200 + i), frame_rights()))
                            .collect(),
                        ..Message::default()
                    },
                    doorbell: s(120),
                },
            ),
            WAITS
        );
        m.settle();
        assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    }

    // Part A: fill user ring 1 -> 0 to 64 entries using 64 signals (1 entry each).
    for i in 0..RING_USER_ENTRIES as u32 {
        m.ok(1, Syscall::GrantSignal(s(200 + i)));
    }
    // Next 36 signals (on uses 36..72) cannot fit -> 36 `signal_pending`!
    for i in 0..36u32 {
        m.ok(1, Syscall::GrantSignal(s(236 + i)));
    }
    assert_eq!(
        m.sys.canonical().uses[1]
            .values()
            .filter(|u| u.signal_pending)
            .count(),
        36
    );
    // Drain user ring and verify all 36 pending signals eventually flush cleanly.
    m.run(30 * MS);
    assert_eq!(
        m.sys.canonical().uses[1]
            .values()
            .filter(|u| u.signal_pending)
            .count(),
        0
    );

    // Part B: delete 30 remote frames -> kernel ring 1 -> 0 hits 30 (answer reserve),
    // then delete 36 more -> 36 `release_pending` (> PENDING_STEP = 32)!
    for i in 0..66u32 {
        m.ok(1, Syscall::CapDelete(s(200 + i)));
    }
    let pending_rel = m.sys.canonical().uses[1]
        .values()
        .filter(|u| u.release_pending)
        .count();
    assert_eq!(pending_rel, 36);
    assert!(pending_rel > PENDING_STEP);
    m.run(50 * MS);
    assert_eq!(
        m.sys.canonical().uses[1]
            .values()
            .filter(|u| u.release_pending)
            .count(),
        0
    );
    assert!(m.bounds.max_pending_signals >= 36);
    assert!(m.bounds.max_pending_releases >= 36);
    assert!(m.bounds.pending_step_32);
}

/// Contract `CSPACE_MAX_SLOTS` (2^20), `REMOVAL_SLOTS` (128), and `REMOVAL_STEP` (32):
/// Create a CSpace with `CSPACE_MAX_SLOTS` max slots, refuse `CSPACE_MAX_SLOTS + 1`,
/// grow a 256-slot CSpace to its max and refuse growing past max, delete it so the
/// first step stops at `REMOVAL_SLOTS` (128) returning `Again`, and revoke 35
/// children so the first step stops at `REMOVAL_STEP` (32) returning `Again`.
#[test]
fn cspace_max_slots_removal_128_slots_and_32_units() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    // CSpace with max = CSPACE_MAX_SLOTS succeeds; + CSPACE_SEGMENT_SLOTS is Invalid.
    k.ok(k.retype(ObjectKind::CSpace, u64::from(CSPACE_MAX_SLOTS), 150));
    k.no(
        k.retype(
            ObjectKind::CSpace,
            u64::from(CSPACE_MAX_SLOTS) + u64::from(dodos_abi::CSPACE_SEGMENT_SLOTS),
            151,
        ),
        Error::Invalid,
    );

    // CSpace with max = 256 starts at 0 slots: grows 0 -> 128 -> 256, then NoSlot.
    k.ok(k.retype(ObjectKind::CSpace, 256, 152));
    assert_eq!(
        k.call(
            k.t,
            Syscall::CSpaceGrow {
                cspace: s(152),
                untyped: s(k.ut),
            },
        ),
        Ok(Output::Slots(128))
    );
    assert_eq!(
        k.call(
            k.t,
            Syscall::CSpaceGrow {
                cspace: s(152),
                untyped: s(k.ut),
            },
        ),
        Ok(Output::Slots(256))
    );
    assert_eq!(
        k.call(
            k.t,
            Syscall::CSpaceGrow {
                cspace: s(152),
                untyped: s(k.ut),
            },
        ),
        Err(Error::NoSlot)
    );

    // Delete the 256-slot CSpace: first step scans 128 slots (REMOVAL_SLOTS) -> Again,
    // second finishes cleanly!
    k.no(Syscall::CapDelete(s(152)), Error::Again);
    k.ok(Syscall::CapDelete(s(152)));

    // Revoke 35 derived children of a notify: first step removes REMOVAL_STEP (32) -> Again.
    k.ok(k.retype(ObjectKind::Notify, 0, 155));
    for i in 0..(REMOVAL_STEP as u32 + 3) {
        k.ok(Syscall::CapDerive {
            source: s(155),
            destination: s(160 + i),
            rights: Rights::SEND.bits(),
            badge: u64::from(i + 1),
        });
    }
    k.no(Syscall::CapRevoke(s(155)), Error::Again);
    k.ok(Syscall::CapRevoke(s(155)));

    assert_eq!(k.bounds.max_cspace_slots, CSPACE_MAX_SLOTS);
    assert!(k.bounds.cspace_over_max_refused);
    assert!(k.bounds.removal_128_slots_again);
    assert!(k.bounds.removal_32_again);
}

/// Contract `TREE_DEPTH` (64):
/// Derive a frame down to depth 64; deriving depth 65 locally is `Invalid`,
/// `node_send` granting the depth-64 frame is `Invalid` (D-141), while `node_send`
/// granting the depth-63 frame succeeds with anchor depth 64 and the receiver
/// cannot derive it to depth 65 (`Invalid`).
#[test]
fn tree_depth_64_refused_locally_and_across_grant() {
    let mut m = S::two();
    let ut0 = untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(0, retype_from(ut0, ObjectKind::Frame, 1, 150));

    // Untyped is at depth 1, frame at slot 150 is at depth 2. Derive down to depth 64.
    let mut cur_slot = 150u32;
    let mut cur_depth = {
        let obj = m.sys.node(0).object_at(m.t[0], s(cur_slot)).unwrap();
        let root = m.sys.node(0).objects[&obj].root;
        m.sys.node(0).depth(root)
    };
    let mut depth_63_slot = 0u32;
    while cur_depth < TREE_DEPTH {
        let next_slot = cur_slot + 1;
        m.ok(
            0,
            Syscall::CapDerive {
                source: s(cur_slot),
                destination: s(next_slot),
                rights: frame_rights(),
                badge: 0,
            },
        );
        cur_slot = next_slot;
        cur_depth += 1;
        if cur_depth == TREE_DEPTH - 1 {
            depth_63_slot = cur_slot;
        }
    }
    let depth_64_slot = cur_slot;
    // Depth 65 locally -> Invalid.
    assert_eq!(
        m.call(
            0,
            Syscall::CapDerive {
                source: s(depth_64_slot),
                destination: s(depth_64_slot + 1),
                rights: frame_rights(),
                badge: 0,
            },
        ),
        Err(Error::Invalid)
    );
    // Granting depth-64 frame (whose anchor would be depth 65) -> Invalid!
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    caps: vec![Transfer::Derive(s(depth_64_slot), frame_rights())],
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Invalid)
    );

    // Granting depth-63 frame succeeds (anchor at depth 64); receiver's derive -> Invalid!
    let reply1 = m.retype(1, ObjectKind::Reply, 0, 120);
    m.ok(1, reply1);
    let rx1 = spawn_thread_min_budget(&mut m, 1, 130, 1);
    assert_eq!(
        m.call_by(
            1,
            rx1,
            Syscall::Recv {
                endpoint: s(START_SLOT_MAILBOX),
                reply: s(120),
                range: Range {
                    first: s(200),
                    count: 1,
                },
                wait: 0,
            },
        ),
        WAITS
    );
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    caps: vec![Transfer::Derive(s(depth_63_slot), frame_rights())],
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        WAITS
    );
    m.settle();
    assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    assert_eq!(
        m.call(
            1,
            Syscall::CapDerive {
                source: s(200),
                destination: s(201),
                rights: frame_rights(),
                badge: 0,
            },
        ),
        Err(Error::Invalid)
    );
    assert_eq!(m.bounds.max_tree_depth, TREE_DEPTH);
    assert!(m.bounds.tree_depth_65_refused);
}

/// Contract `GRANTS` (4,096 grant entries and 4,096 use entries):
/// Fill node 0's grant table and node 1's use table with 4,096 genuine grants
/// (checked end-to-end). Verify node 0's 4,097th grant returns `Busy`
/// (`grants_full_refused`), and node 2 sending 1 frame to node 1 drops the cap
/// with `caps_dropped` (`uses_full_dropped`), then verify full `sys.check()`.
#[test]
fn grants_and_uses_reach_4096_with_full_check_and_refusals() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    // Need enough ordinary memory on node 0 for 4,096 frames (16 MB) + CSpace pages.
    let chunk = mach.geometry.chunk();
    mach.floors.floor_node = 14 * chunk;
    let mut m = S::on(&mach);

    let ut0 = untyped_slot(m.sys.node(0), m.t[0]);
    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    let ut2 = untyped_slot(m.sys.node(2), m.t[2]);
    let reply1 = m.retype(1, ObjectKind::Reply, 0, 60);
    m.ok(1, reply1);
    let rx1 = spawn_thread_min_budget(&mut m, 1, 70, 1);

    // Create a fresh 8,192-slot CSpace on node 1 in slot 180 and grow it to
    // 4,224 slots (33 segments) to receive all 4,096 remote frames + 1 extra.
    m.ok(1, retype_from(ut1, ObjectKind::CSpace, 8192, 180));
    for _ in 0..33 {
        assert!(matches!(
            m.sys.apply(
                1,
                m.t[1],
                Syscall::CSpaceGrow {
                    cspace: s(180),
                    untyped: s(ut1),
                },
            ),
            Ok(Output::Slots(_))
        ));
    }

    // Four distinct frames on node 0, granted 1,024 times (4,096 total) via `Transfer::Derive`.
    for i in 0..MAX_CAPS as u32 {
        m.ok(0, retype_from(ut0, ObjectKind::Frame, 1, 196 + i));
    }
    m.ok(2, retype_from(ut2, ObjectKind::Frame, 1, 199));

    let batches = (dodos_abi::GRANTS / MAX_CAPS) as u32;
    let caps: Vec<Transfer> = (0..MAX_CAPS as u32)
        .map(|i| Transfer::Derive(s(196 + i), frame_rights()))
        .collect();
    for batch in 0..batches {
        let first = 1 + batch * MAX_CAPS as u32;
        assert_eq!(
            m.sys.apply(
                1,
                rx1,
                Syscall::Recv {
                    endpoint: s(START_SLOT_MAILBOX),
                    reply: s(60),
                    range: Range {
                        first: SlotAddr::through(180, first),
                        count: MAX_CAPS as u32,
                    },
                    wait: 0,
                },
            ),
            WAITS
        );
        assert_eq!(
            m.sys.apply(
                0,
                m.t[0],
                Syscall::NodeSend {
                    node: node_cap(1),
                    message: Message {
                        caps: caps.clone(),
                        ..Message::default()
                    },
                    doorbell: SlotAddr::NONE,
                },
            ),
            WAITS
        );
        m.sys.settle(1_000);
        assert_eq!(m.woken(0), Some(&Ok(Output::Sent { flags: 0 })));
    }
    assert_eq!(m.sys.check(), Ok(()));
    m.sample_bounds();
    assert_eq!(m.sys.canonical().grants[0].len(), dodos_abi::GRANTS);
    assert_eq!(m.sys.canonical().uses[1].len(), dodos_abi::GRANTS);

    // 4,097th grant from node 0 -> Busy (`grants_full_refused`).
    assert_eq!(
        m.call(
            0,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    caps: vec![Transfer::Derive(s(199), frame_rights())],
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        Err(Error::Busy)
    );

    // Node 2 (empty grant table) sends 1 frame to node 1 (full 4,096 use table):
    // delivered with FLAG_CAPS_DROPPED and SendAnswer { dropped: true }!
    assert_eq!(
        m.call_by(
            1,
            rx1,
            Syscall::Recv {
                endpoint: s(START_SLOT_MAILBOX),
                reply: s(60),
                range: Range {
                    first: SlotAddr::through(180, 1 + dodos_abi::GRANTS as u32),
                    count: 1,
                },
                wait: 0,
            },
        ),
        WAITS
    );
    assert_eq!(
        m.call(
            2,
            Syscall::NodeSend {
                node: node_cap(1),
                message: Message {
                    caps: vec![Transfer::Derive(s(199), frame_rights())],
                    ..Message::default()
                },
                doorbell: SlotAddr::NONE,
            },
        ),
        WAITS
    );
    m.settle();
    assert_eq!(
        m.woken(2),
        Some(&Ok(Output::Sent {
            flags: dodos_abi::Header::FLAG_CAPS_DROPPED,
        }))
    );
    assert!(m.sys.canonical().grants[2].is_empty());
    assert_eq!(m.bounds.max_grants, dodos_abi::GRANTS);
    assert_eq!(m.bounds.max_uses, dodos_abi::GRANTS);
    assert!(m.bounds.grants_full_refused);
    assert!(m.bounds.uses_full_dropped);
}

/// Contract `TIME_EVENT_STEP` (16), `DEBUG_WRITE_MAX` (512), `ENTROPY_MAX_BYTES`
/// (64), `MAP_COUNT_MAX`, and `SCHED_MAX_DEBT` (`2^26` exact & forgiven):
/// Exercises every single-node bound and refusal under full `m.check()`.
#[test]
fn time_event_step_16_debug_entropy_map_count_and_sched_debt() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));

    // TIME_EVENT_STEP (16): arm 20 timers to the same deadline; one timer interrupt
    // serves at most 16 and leaves 4 for the next step.
    k.ok(k.retype(ObjectKind::Notify, 0, 150));
    let deadline = k.m.now + 5 * MS;
    for i in 0..(TIME_EVENT_STEP as u32 + 4) {
        k.ok(k.retype(ObjectKind::Timer, 0, 160 + i));
        assert!(matches!(
            k.call(
                k.t,
                Syscall::TimerSet {
                    timer: s(160 + i),
                    notify: s(150),
                    deadline,
                    tolerance: 0,
                },
            ),
            Ok(Output::WasArmed(_))
        ));
    }
    k.m.set_now(deadline);
    k.bounds.sample_model(&k.m);
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    let still_armed =
        k.m.timers
            .values()
            .flatten()
            .filter(|a| a.instant <= k.m.now)
            .count();
    assert_eq!(
        still_armed, 4,
        "one timer interrupt fires at most 16 due timers"
    );
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    assert!(k.m.timers.values().flatten().next().is_none());

    // DEBUG_WRITE_MAX (512): 512 ok, 513 Invalid.
    k.ok(Syscall::DebugWrite {
        debug: s(START_SLOT_DEBUG),
        length: DEBUG_WRITE_MAX,
        words: vec![0; (DEBUG_WRITE_MAX / 8) as usize],
    });
    k.no(
        Syscall::DebugWrite {
            debug: s(START_SLOT_DEBUG),
            length: DEBUG_WRITE_MAX + 1,
            words: vec![0; (DEBUG_WRITE_MAX / 8) as usize],
        },
        Error::Invalid,
    );

    // ENTROPY_MAX_BYTES (64): 64 ok, 65 Invalid.
    assert!(matches!(
        k.call(
            k.t,
            Syscall::EntropyRead {
                length: ENTROPY_MAX_BYTES,
                mix: [1, 2, 3, 4],
            },
        ),
        Ok(Output::Bytes(_))
    ));
    k.no(
        Syscall::EntropyRead {
            length: ENTROPY_MAX_BYTES + 1,
            mix: [1, 2, 3, 4],
        },
        Error::Invalid,
    );

    // MAP_COUNT_MAX: scale machine.map_count_max to current chunk map count, verify
    // next ASpaceMap returns Busy, then restore.
    k.ok(k.retype(ObjectKind::ASpace, 0, 190));
    k.ok(k.retype(ObjectKind::Frame, 1, 191));
    k.ok(Syscall::CapDerive {
        source: s(191),
        destination: s(192),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let tables = k.tables();
    let map_cap_at = |frame: u32, addr: u64| {
        Syscall::ASpaceMap(Map {
            space: s(190),
            frame: s(frame),
            untyped: s(tables),
            address: addr,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        })
    };
    k.ok(map_cap_at(191, 0x1000_0000));
    let frame_obj = k.m.object_at(k.t, s(191)).unwrap();
    let chunk = k.m.frames[&frame_obj].base / k.m.geometry().chunk();
    k.m.machine.map_count_max = k.m.table.map_count(chunk);
    k.no(map_cap_at(192, 0x1010_0000), Error::Busy);
    k.m.machine.map_count_max = dodos_abi::MAP_COUNT_MAX;
    k.ok(map_cap_at(192, 0x1010_0000));
    assert_eq!(k.m.check(), Ok(()));

    // SCHED_MAX_DEBT (2^26): charge SCHED_MAX_DEBT + 1 budgets and sample before
    // replenishment so bounds captures both max_sched_debt and debt.forgiven.
    let sched_obj = k.m.object_at(k.t, s(START_SLOT_SCHED)).unwrap();
    let b = SCHED_MIN_BUDGET_NS;
    let p = 10 * MS;
    k.ok(Syscall::SchedConfigure {
        sched: s(START_SLOT_SCHED),
        node: node_cap(0),
        core: 0,
        budget: b,
        period: p,
        class: 1,
    });
    // Exhaust and replenish right at its deadline so the new (b, p) parameters
    // take effect while the context is running from the ready queue.
    let deadline = k.m.scheds[&sched_obj].deadline;
    k.m.advance(deadline);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.scheds[&sched_obj].budget, b);
    assert_eq!(k.m.cores[0].running, Some((sched_obj, true)));
    let over = k.m.now + (SCHED_MAX_DEBT + 1) * b + 3;
    k.m.set_now(over);
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    k.bounds.sample_model(&k.m);
    let (instant, debt) = k.m.scheds[&sched_obj].throttled.expect("throttled");
    assert_eq!((debt.budgets, debt.forgiven), (SCHED_MAX_DEBT, true));
    k.m.set_now(instant);
    k.m.timer_interrupt(0);
    assert_eq!(k.m.check(), Ok(()));
    assert_eq!(k.m.scheds[&sched_obj].throttled, None);
    assert_eq!(k.m.scheds[&sched_obj].remaining, b as i64);
    k.bounds.sample_model(&k.m);

    assert!(k.bounds.time_event_step_16);
    assert_eq!(k.bounds.max_debug_write, DEBUG_WRITE_MAX);
    assert!(k.bounds.debug_write_over_512_refused);
    assert_eq!(k.bounds.max_entropy_bytes, ENTROPY_MAX_BYTES);
    assert!(k.bounds.entropy_over_64_refused);
    assert!(k.bounds.map_count_max_refused);
    assert_eq!(k.bounds.max_sched_debt, SCHED_MAX_DEBT);
    assert!(k.bounds.sched_debt_forgiven);
}

/// Contract `CORE_ANSWER_NS` (10 ms) across nodes:
/// Node 1 has two cores; core 2 stops answering shootdowns. Node 1 unmaps a shared
/// page, waits `CORE_ANSWER_NS`, accuses itself with `CrashReason::CoreSilent` (9),
/// and node 0 fences and reclaims node 1 cleanly.
#[test]
fn core_answer_timeout_self_accusation_reclaimed_by_peer() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1, 2]]));
    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    let tables1 = table_slot(m.sys.node(1), m.t[1]);

    // Map a shared frame into an ASpace and run a thread on core 2 in that ASpace.
    let asp = m.retype(1, ObjectKind::ASpace, 0, 150);
    m.ok(1, asp);
    m.ok(1, retype_from(ut1, ObjectKind::Frame, 1, 151));
    m.ok(
        1,
        Syscall::ASpaceMap(Map {
            space: s(150),
            frame: s(151),
            untyped: s(tables1),
            address: 0x1000_0000,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    // Spawn a thread on core 2 using ASpace 150.
    m.ok(1, retype_from(ut1, ObjectKind::Thread, 0, 160));
    m.ok(1, retype_from(ut1, ObjectKind::Sched, 0, 161));
    m.ok(
        1,
        Syscall::ThreadConfigure {
            thread: s(160),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(150),
            fault: SlotAddr::NONE,
            page: SlotAddr::NONE,
        },
    );
    m.ok(
        1,
        Syscall::SchedConfigure {
            sched: s(161),
            node: node_cap(1),
            core: 2,
            budget: 5 * MS,
            period: 10 * MS,
            class: 1,
        },
    );
    m.ok(
        1,
        Syscall::SchedBind {
            sched: s(161),
            thread: s(160),
        },
    );
    m.ok(1, Syscall::ThreadResume(s(160)));
    m.run(2 * MS);

    // Hang core 2 (non-NMI hang so fence NMI still parks it!) and unmap frame 151.
    m.sys.hang(2, false);
    assert_eq!(m.call(1, Syscall::ASpaceUnmap(s(151))), WAITS);
    // Advance past CORE_ANSWER_NS (10 ms) + fence/reclaim.
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    assert_eq!(
        m.sys.canonical().beacons[1].panic.map(|(r, _, _)| r),
        Some(dodos_abi::CrashReason::CoreSilent as u64)
    );
    assert!(m.bounds.core_answer_timeout);
}

/// Regression for pass 43 (seed 254): a secondary core that previously died
/// while running a thread (leaving `idle_until == 0` in its core beacon) must
/// publish its new `heartbeat` and `idle_until` when a fresh generation enters
/// idle on that core; otherwise `watch_own_cores` falsely accuses the healthy
/// node with `CrashReason::CoreSilent` after `STALE_NS`.
#[test]
fn reused_secondary_core_entering_idle_publishes_beacon_and_is_not_falsely_accused() {
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1, 2]]);
    mach.membership.stale_ms = 40;
    mach.membership.sweep_ms = 100;
    let mut m = S::on(&mach);

    // Run a thread on secondary core 2 so `core_beacons[2].idle_until` becomes 0.
    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(1, retype_from(ut1, ObjectKind::Thread, 0, 160));
    m.ok(1, retype_from(ut1, ObjectKind::Sched, 0, 161));
    m.ok(
        1,
        Syscall::ThreadConfigure {
            thread: s(160),
            cspace: s(START_SLOT_CSPACE),
            aspace: s(dodos_abi::START_SLOT_ASPACE),
            fault: SlotAddr::NONE,
            page: SlotAddr::NONE,
        },
    );
    m.ok(
        1,
        Syscall::SchedConfigure {
            sched: s(161),
            node: node_cap(1),
            core: 2,
            budget: 50 * MS,
            period: 100 * MS,
            class: 1,
        },
    );
    m.ok(
        1,
        Syscall::SchedBind {
            sched: s(161),
            thread: s(160),
        },
    );
    m.ok(1, Syscall::ThreadResume(s(160)));
    m.run(2 * MS);
    assert_eq!(m.sys.canonical().core_beacons[2].idle_until.0, 0);

    // Kill node 1 while core 2 is busy, wait until Dead, then fresh-start node 1
    // on the same cores (1 and 2) with core 2 idle.
    m.supervisor(0, m.t[0]);
    m.ok(0, kill(1));
    m.run(40 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.ok(0, super::pass17::fresh(1, 0b110));
    m.run(20 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(
        m.sys.canonical().core_beacons[2].idle_until.0,
        u64::MAX,
        "reused idle secondary core must publish idle_until = u64::MAX on entry"
    );

    // Advance well past STALE_NS and sweep: healthy node 1 must stay Live.
    m.run(150 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    assert_eq!(m.sys.canonical().beacons[1].panic, None);
}
