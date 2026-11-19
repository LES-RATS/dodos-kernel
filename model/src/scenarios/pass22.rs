//! Pass 22: what D-141 changed in the contract, one scenario each, and the
//! faults the final review of the model found, each with the test that
//! fails without its correction. The attacks write what a faulty kernel
//! can write: its own rings (`System::inject`, or an entry it left there),
//! its own words of the living part (`System::edit_table`).

use super::*;
use crate::rings::{Body, UnitOp};
use crate::{Map, Message, Range, Transfer};
use dodos_abi::{Header, NodeMessage, RingHeader, START_SLOT_ASPACE, START_SLOT_MAILBOX};

const REPLY: u32 = 100;
const OBJ: u32 = 101;
const GOT: u32 = 110;

fn recv(first: u32) -> Syscall {
    Syscall::Recv {
        endpoint: s(START_SLOT_MAILBOX),
        reply: s(REPLY),
        range: Range {
            first: s(first),
            count: 4,
        },
        wait: 0,
    }
}

fn rights() -> u32 {
    Rights::READ
        .union(Rights::WRITE)
        .union(Rights::SEND)
        .union(Rights::DERIVE)
        .bits()
}

/// A message whose header names `sender`, as a kernel writes it into its
/// own ring with plain stores.
pub(crate) fn forged(
    m: &S,
    kind: NodeMessage,
    sender: u16,
    to: u16,
    credit: Option<(u8, u32)>,
    body: Body,
) -> crate::rings::Message {
    let t = m.sys.canonical();
    let header = RingHeader::new(
        kind,
        body.entries() as u8,
        (sender, t.generation(sender)),
        t.generation(to),
        credit,
    );
    crate::rings::Message { header, body }
}

fn map_got(address: u64) -> Syscall {
    Syscall::ASpaceMap(Map {
        space: s(START_SLOT_ASPACE),
        frame: s(GOT),
        untyped: SlotAddr::NONE,
        address,
        first: 0,
        count: 1,
        rights: Rights::READ.union(Rights::WRITE).bits(),
    })
}

/// Three nodes; node 0 sends node 1 one frame (slot `OBJ`) with `derive`;
/// node 1 receives it in slot `GOT`. Settled unless `settle` is false.
fn granted(settle: bool) -> S {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    for node in [0, 1] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    let message = Message {
        label: 7,
        flags: 0,
        words: vec![7],
        caps: vec![Transfer::Derive(s(OBJ), rights())],
        marks: 0,
    };
    let call = Syscall::NodeSend {
        node: node_cap(1),
        message,
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(0, call), WAITS);
    if settle {
        m.settle();
    }
    m
}

fn the_grant(m: &S) -> (u16, u32) {
    let grant = m.sys.node(0).grant_entries()[0].0;
    (grant, m.sys.canonical().grants[0][&grant].serial)
}

// Point 1: a message's sender is its ring's writer.

#[test]
fn a_release_written_on_a_third_nodes_ring_in_the_holders_name_is_refused() {
    // The holder maps nothing: from the holder's own ring the same
    // `Release` would free the grant, so only the sender rule keeps it.
    let mut m = granted(true);
    let (grant, serial) = the_grant(&m);
    let before = m.sys.node(0).refused.0;
    let release = forged(
        &m,
        NodeMessage::Release,
        1,
        0,
        None,
        Body::Release { grant, serial },
    );
    m.sys.inject(2, 0, release);
    m.run(20 * MS);
    assert_eq!(m.sys.node(0).refused.0, before + 1);
    assert!(m.sys.canonical().grants[0].contains_key(&grant));
}

#[test]
fn a_send_written_on_a_third_nodes_ring_in_another_nodes_name_is_refused() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    assert_eq!(m.call(0, recv(GOT)), WAITS);
    let task = dodos_abi::TaskId::new(1, 1).raw();
    let body = Body::Send {
        header: Header::new(7, 1, 0, 0).expect("a header"),
        stamp: (task, 0),
        grants: vec![],
        words: vec![42],
    };
    let send = forged(&m, NodeMessage::Send, 1, 0, Some((16, 1)), body);
    m.sys.inject(2, 0, send);
    m.run(20 * MS);
    assert!(m.woken(0).is_none(), "the forged send was delivered");
}

#[test]
fn a_detach_written_on_a_third_nodes_ring_in_the_owners_name_is_refused() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let device = DEVICE as u16;
    let unit = &m.sys.canonical().units[&IOMMU_ROLE];
    let domain = unit.domains[&device];
    let before = unit.entries.get(&device).copied();
    assert!(before.is_some());
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let op = UnitOp::Detach {
        device,
        domain,
        root: 0,
    };
    let body = Body::UnitRequest {
        role: IOMMU_ROLE as u16,
        epoch,
        op,
    };
    let request = forged(&m, NodeMessage::UnitRequest, 0, 1, Some((5, 77)), body);
    m.sys.inject(2, 1, request);
    m.run(30 * MS);
    let after = m.sys.canonical().units[&IOMMU_ROLE]
        .entries
        .get(&device)
        .copied();
    assert_eq!(after, before);
}

// Point 2: a grant is freed only when the holder translates nothing.

#[test]
fn a_release_of_the_holder_while_its_tables_translate_the_frame_frees_nothing() {
    let mut m = granted(true);
    assert_eq!(m.call(1, map_got(1 << 30)), DONE);
    let (grant, serial) = the_grant(&m);
    let before = m.sys.node(0).refused.0;
    // Node 1's kernel, faulty, writes a `Release` on its own ring while its
    // tables still translate the frame.
    let release = forged(
        &m,
        NodeMessage::Release,
        1,
        0,
        None,
        Body::Release { grant, serial },
    );
    m.sys.inject(1, 0, release);
    m.run(20 * MS);
    assert!(m.sys.canonical().grants[0].contains_key(&grant));
    assert_eq!(m.sys.node(0).refused.0, before + 1);
}

#[test]
fn a_revoke_step_reporting_the_root_removed_frees_nothing_while_the_holder_counts_a_leaf() {
    let mut m = granted(true);
    assert_eq!(m.call(1, map_got(1 << 30)), DONE);
    let (grant, _) = the_grant(&m);
    // Node 0 revokes its frame: the grant becomes revoking and a `Revoke`
    // goes to node 1, whose answer node 1's faulty kernel rewrites.
    let revoke = Syscall::CapRevoke(s(OBJ));
    assert_eq!(m.call(0, revoke), WAITS);
    m.sys.edit_table(|t| {
        // Node 1 has not consumed the `Revoke` yet: it answers in its own
        // ring that the root is removed, its leaf still counted.
        let _ = t;
    });
    let credit = m.sys.node(0).peers[1]
        .credits
        .iter()
        .position(|c| c.is_some())
        .map(|i| {
            (
                i as u8,
                m.sys.node(0).peers[1].credits[i]
                    .as_ref()
                    .expect("a credit")
                    .serial,
            )
        })
        .expect("the revoke's credit");
    let step = forged(
        &m,
        NodeMessage::RevokeStep,
        1,
        0,
        Some(credit),
        Body::RevokeStep { removed: true },
    );
    m.sys.inject(1, 0, step);
    m.sys.advance(m.sys.now() + MS);
    assert!(
        m.sys.canonical().grants[0].contains_key(&grant),
        "a RevokeStep freed a grant whose holder still counts a leaf"
    );
}

#[test]
fn a_map_of_a_received_frame_before_its_sender_consumed_the_answer_is_again() {
    let mut m = granted(false);
    // Node 1 receives the `Send` (its user ring is served at once), but node
    // 0 has not yet consumed the `SendAnswer`, served in its reserve.
    let to = m.sys.now() + MS / 2;
    while m.woken(1).is_none() && m.sys.now() < to {
        m.sys.advance(m.sys.now() + 10_000);
    }
    assert!(matches!(m.woken(1), Some(Ok(Output::Message(_)))));
    let (grant, _) = the_grant(&m);
    // The grant entry as it reads before node 0 consumes the answer, if the
    // steps above let it consume it already.
    let recorded = m.sys.canonical().grants[0][&grant];
    m.sys.edit_table(|t| {
        let entry = t.grants[0].get_mut(&grant).unwrap();
        (entry.answered, entry.use_index) = (false, None);
    });
    // (The machine is not checked in this edited state: no `Send` is in
    // flight for the entry any more.)
    let thread = m.t[1];
    assert_eq!(m.sys.apply(1, thread, map_got(1 << 30)), Err(Error::Again));
    m.sys.edit_table(|t| {
        t.grants[0].insert(grant, recorded);
    });
    m.settle();
    assert_eq!(m.call(1, map_got(1 << 30)), DONE);
}

// Point 3: what a holder maps is what the anchor says.

#[test]
fn a_remote_frame_is_the_anchors_frame_and_rights() {
    let m = granted(true);
    let (grant, _) = the_grant(&m);
    let anchor = m.sys.canonical().grants[0][&grant].anchor;
    let got = m.sys.node(1).object_at(m.t[1], s(GOT)).expect("the frame");
    assert_eq!(m.sys.node(1).frames[&got], anchor.frame);
    let root = m.sys.node(1).objects[&got].root;
    assert_eq!(m.sys.node(1).caps[&root].rights, anchor.rights);
    assert_eq!(m.sys.node(1).remotes[&got].depth, anchor.depth);
}

#[test]
fn a_send_whose_grant_states_other_rights_than_the_anchor_is_refused() {
    let mut m = granted(false);
    m.sys.edit_table(|t| {
        for pair in std::sync::Arc::make_mut(&mut t.rings)[0].iter_mut() {
            for entry in pair.user.entries.iter_mut().flatten() {
                if let crate::table::RingEntry::Message(msg) = entry
                    && let Body::Send { grants, .. } = &mut msg.body
                {
                    grants[0].rights = Rights::ALL;
                }
            }
        }
    });
    // The faulty sender loses its credit, so the machine's check is not
    // asked: the receiver's refusal is.
    m.sys.settle(200);
    assert!(m.woken(1).is_none(), "the send was delivered");
}

#[test]
fn a_send_whose_anchor_has_depth_0_is_refused() {
    let mut m = granted(false);
    let (grant, _) = the_grant(&m);
    m.sys.edit_table(move |t| {
        t.grants[0].get_mut(&grant).expect("the entry").anchor.depth = 0;
    });
    m.sys.settle(200);
    assert!(m.woken(1).is_none(), "the send was delivered");
}

#[test]
fn a_second_send_naming_an_answered_grant_is_refused() {
    let mut m = granted(true);
    let (grant, serial) = the_grant(&m);
    assert_eq!(m.call(1, recv(GOT + 4)), WAITS);
    let body = Body::Send {
        header: Header::new(7, 0, 1, 0).expect("a header"),
        stamp: (0, 0),
        grants: vec![crate::grants::Granted {
            grant,
            serial,
            rights: Rights::from_bits(rights()).expect("rights"),
        }],
        words: vec![],
    };
    let send = forged(&m, NodeMessage::Send, 0, 1, Some((16, 9)), body);
    m.sys.inject(0, 1, send);
    m.run(5 * MS);
    assert!(m.woken(1).is_none(), "a grant was placed twice");
}

#[test]
fn node_send_refuses_a_frame_whose_chunk_its_node_does_not_hold() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    let object = m.sys.node(0).object_at(m.t[0], s(OBJ)).expect("the frame");
    let base = m.sys.node(0).frames[&object].base;
    let chunk = base / m.sys.node(0).machine.geometry.chunk();
    // A word of the frame's chunk that names node 2: the gate refuses to
    // write an anchor over it.
    m.sys.edit_table(move |t| {
        t.floors[0].clear();
        std::sync::Arc::make_mut(&mut t.owners)[chunk as usize] =
            dodos_abi::Ownership::taken(dodos_abi::Owner::Node(2), 1).raw();
    });
    let message = Message {
        label: 7,
        flags: 0,
        words: vec![],
        caps: vec![Transfer::Derive(s(OBJ), rights())],
        marks: 0,
    };
    let call = Syscall::NodeSend {
        node: node_cap(1),
        message,
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.sys.apply(0, m.t[0], call), Err(Error::Busy));
}

// The credits of a request, and the room of its answer.

#[test]
fn a_unit_request_with_a_send_credit_is_refused() {
    use super::units::{DEVICE, IOMMU_ROLE, three};
    let mut m = three();
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let body = Body::UnitRequest {
        role: IOMMU_ROLE as u16,
        epoch,
        op: UnitOp::Invalidate { domain: Some(1) },
    };
    let request = forged(&m, NodeMessage::UnitRequest, 0, 1, Some((20, 3)), body);
    let before = m.sys.node(1).refused.0;
    m.sys.inject(0, 1, request);
    m.run(30 * MS);
    assert_eq!(m.sys.node(1).refused.0, before + 1);
    let _ = DEVICE;
}

#[test]
fn a_request_whose_answer_would_not_fit_is_refused() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    m.sys.hang(0, false);
    // Node 0 consumes nothing of the ring 1 -> 0: its consumer index stays
    // while node 1's producer index stands at the ring's size.
    m.sys.edit_table(|t| {
        std::sync::Arc::make_mut(&mut t.rings)[1][0].kernel.produced +=
            dodos_abi::RING_KERNEL_ENTRIES;
    });
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    let body = Body::UnitRequest {
        role: IOMMU_ROLE as u16,
        epoch,
        op: UnitOp::Invalidate { domain: Some(1) },
    };
    let request = forged(&m, NodeMessage::UnitRequest, 0, 1, Some((3, 3)), body);
    let before = m.sys.node(1).refused.0;
    m.sys.inject(0, 1, request);
    m.sys.settle(200);
    assert_eq!(m.sys.node(1).refused.0, before + 1);
}

// The ring's indices.

#[test]
fn a_producer_index_behind_the_consumer_moves_the_consumer_back() {
    let mut m = S::two();
    for _ in 0..4 {
        let body = Body::Signal {
            grant: 3,
            serial: 0,
        };
        let signal = forged(&m, NodeMessage::Signal, 1, 0, None, body);
        m.sys.inject(1, 0, signal);
        m.run(MS);
    }
    m.sys
        .edit_table(|t| std::sync::Arc::make_mut(&mut t.rings)[1][0].user.produced = 0);
    let body = Body::Signal {
        grant: 3,
        serial: 0,
    };
    let signal = forged(&m, NodeMessage::Signal, 1, 0, None, body);
    let before = m.sys.node(0).refused.0;
    m.sys.inject(1, 0, signal);
    m.sys.settle(100);
    let consumed = m.sys.canonical().beacons[0].consumed[1].1;
    assert_eq!(consumed, m.sys.canonical().rings[1][0].user.produced);
    // One refusal, the move back: the consumer reads no entry of the ring
    // between (read as the full-ring rule, it would take 64 stale entries).
    assert_eq!(m.sys.node(0).refused.0, before + 1);
}

#[test]
fn one_interrupt_serves_sixteen_entries_of_a_user_ring_not_sixteen_messages() {
    let mut m = S::two();
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    for i in 0..17u64 {
        let body = Body::Send {
            header: Header::new(7, 8, 0, 0).expect("a header"),
            stamp: (0, 0),
            grants: vec![],
            words: vec![i; 8],
        };
        let send = forged(&m, NodeMessage::Send, 1, 0, Some((16, i as u32 + 1)), body);
        assert_eq!(send.body.entries(), 2);
        m.sys.inject(1, 0, send);
    }
    let start = m.sys.canonical().beacons[0].consumed[1].1;
    m.sys.fire(crate::system::Event::Ipi { node: 0, core: 0 });
    let served = m.sys.canonical().beacons[0].consumed[1].1 - start;
    assert_eq!(served, dodos_abi::RING_USER_STEP);
}

// The model's faults found by the attacks of lot B.

const CONTROL: u32 = dodos_abi::START_SLOT_MEMORY_CONTROL;

pub(super) fn take(kind: u64, place: u64, size: u64, destination: u32) -> Syscall {
    Syscall::MemoryTake {
        control: s(CONTROL),
        kind,
        place,
        size,
        destination: s(destination),
    }
}

#[test]
fn a_reserve_chunk_taken_by_this_generation_is_busy() {
    use super::units::three;
    let mut m = three();
    let shift = m.sys.canonical().geometry.chunk_shift();
    let word = dodos_abi::Ownership::free(dodos_abi::Owner::Reserve(0)).raw();
    let chunk = m
        .sys
        .canonical()
        .owners
        .iter()
        .position(|&w| w == word)
        .expect("a reserve chunk") as u64;
    let roles = slots_of(m.sys.node(0), m.t[0], ObjectKind::Role);
    // Role index 2 is reserve 0 on the reference machine.
    assert!(matches!(
        m.call(0, Syscall::RoleClaim(s(roles[2]))),
        Ok(Output::Epoch(_))
    ));
    m.ok(0, take(0, chunk << shift, u64::from(shift), 160));
    m.ok(0, retype_from(160, ObjectKind::ASpace, 0, 162));
    m.no(
        0,
        take(0, chunk << shift, u64::from(shift), 161),
        Error::Busy,
    );
}

/// Node 0 maps a device frame into its iospace, the unit fails, the unmap
/// keeps the leaf counted, the frame goes; on the block of device entry
/// `index`.
pub(super) fn frozen_device_leaf(index: u64) -> S {
    use super::units::{IOMMU_ROLE, IOSPACE, attached, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    attached(&mut m, 0);
    m.ok(0, take(1, index, 0, 160));
    let frame = Syscall::UntypedRetype {
        untyped: s(160),
        kind: ObjectKind::Frame,
        size: 1,
        page: 0,
        memory: dodos_abi::MemoryType::Uncached as u64,
        destination: s(161),
    };
    m.ok(0, frame);
    let map = Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(161),
        untyped: s(ut),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(0, map);
    m.sys.break_unit(IOMMU_ROLE);
    assert_eq!(m.call(0, Syscall::IoSpaceUnmap(s(161))), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    m.ok(0, Syscall::CapDelete(s(161)));
    assert!(m.sys.canonical().device_leaves[index as usize] > 0);
    m
}

#[test]
fn memory_split_is_busy_while_the_entry_counts_a_leaf() {
    // The first entry larger than a page, which a split may cut.
    let page = Machine::example().geometry.page().trailing_zeros();
    let devices = S::two().sys.canonical().devices.clone();
    let index = devices
        .iter()
        .position(|e| e.is_used() && e.log2() > page)
        .expect("an entry to split");
    let mut m = frozen_device_leaf(index as u64);
    let split = Syscall::MemorySplit {
        control: s(CONTROL),
        untyped: s(160),
    };
    m.no(0, split, Error::Busy);
}

#[test]
fn the_pages_of_an_iospace_pinned_by_a_failed_unit_stay_out_of_their_untyped() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(0, retype_from(ut, ObjectKind::Untyped, 1 << 20, 160));
    m.ok(0, retype_from(160, ObjectKind::IoSpace, 0, IOSPACE));
    let iospace = m
        .sys
        .node(0)
        .object_at(m.t[0], s(IOSPACE))
        .expect("an iospace");
    let root = m.sys.node(0).aspaces[&iospace].root.expect("a root");
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    m.sys.break_unit(IOMMU_ROLE);
    let mut outcome = Err(Error::Again);
    for _ in 0..8 {
        outcome = m.call(0, Syscall::CapDelete(s(IOSPACE)));
        if outcome != Err(Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    m.ok(0, retype_from(160, ObjectKind::ASpace, 0, 162));
    let aspace = m.sys.node(0).object_at(m.t[0], s(162)).expect("an aspace");
    assert_ne!(m.sys.node(0).aspaces[&aspace].root, Some(root));
}

#[test]
fn after_a_failed_detach_an_unmap_on_the_iospace_keeps_its_counts() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    attached(&mut m, 0);
    m.ok(0, retype_from(ut, ObjectKind::Frame, 1, 160));
    let frame = m.sys.node(0).object_at(m.t[0], s(160)).expect("a frame");
    let chunk = m.sys.node(0).frames[&frame].base >> m.sys.canonical().geometry.chunk_shift();
    let before = m.sys.canonical().map_count(chunk);
    let map = Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(160),
        untyped: s(ut),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(0, map);
    assert_eq!(m.call(0, attach(&m, 0, 0, DEVICE)), WAITS);
    m.sys.break_unit(IOMMU_ROLE);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    m.no(0, Syscall::IoSpaceUnmap(s(160)), Error::Busy);
    assert_eq!(m.sys.canonical().map_count(chunk), before + 1);
}

#[test]
fn a_remote_frame_is_refused_as_a_message_page() {
    let mut m = granted(true);
    let ut1 = untyped_slot(m.sys.node(1), m.t[1]);
    m.ok(1, retype_from(ut1, ObjectKind::CSpace, 256, 121));
    m.ok(1, retype_from(ut1, ObjectKind::Thread, 0, 120));
    let configure = Syscall::ThreadConfigure {
        thread: s(120),
        cspace: s(121),
        aspace: SlotAddr::NONE,
        fault: SlotAddr::NONE,
        page: s(GOT),
    };
    m.no(1, configure, Error::Invalid);
}

#[test]
fn a_previous_crash_area_of_sequence_max_is_not_valid() {
    let mut machine = Machine::example();
    machine.previous_crash = Some(crate::debug::CrashArea::new(1, u64::MAX - 1, 1));
    let (model, _) = Model::boot(&machine).expect("the boot");
    assert_eq!(model.table.crash.sequence, 1);
    assert!(model.table.previous.is_none());
}

/// Node 1 is killed and dead; what its faulty generation left in the
/// header of its core's area is `edit`; node 2 starts node 1 again there.
pub(super) fn restart_over(edit: impl FnOnce(&mut crate::debug::DebugArea)) -> S {
    use super::units::three;
    let mut m = three();
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(2, kill);
    m.run(30 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    m.sys
        .edit_table(|t| edit(&mut std::sync::Arc::make_mut(&mut t.crash).areas[1]));
    let fresh = Syscall::NodeStart {
        node: node_cap(1),
        mode: dodos_abi::NODE_START_FRESH,
        image: 0,
        module: 0,
        window: 0,
        mask: 0b10,
    };
    m.ok(2, fresh);
    m.run(5 * MS);
    assert_eq!(m.state(1), MemberState::Live);
    m
}

#[test]
fn a_debug_area_whose_next_sequence_is_0_is_cleared_and_traced() {
    let m = restart_over(|area| area.next_kernel = 0);
    let ring = m.sys.node(0).trace_ring(1, false);
    assert_eq!(ring[0].seq, 1);
    assert_eq!(
        ring[0].word2 & 0xFFFF,
        dodos_abi::TraceCode::AreaCleared as u64
    );
}

#[test]
fn a_debug_area_whose_next_sequence_is_2_to_the_63_is_cleared() {
    let m = restart_over(|area| area.next_user = 1 << 63);
    assert!(m.sys.canonical().crash.areas[1].next_user < 1 << 63);
}

#[test]
fn a_debug_area_whose_next_sequence_disagrees_with_its_ring_is_cleared() {
    let m = restart_over(|area| area.next_kernel += 7);
    let ring = m.sys.node(0).trace_ring(1, false);
    assert_eq!(
        ring[0].word2 & 0xFFFF,
        dodos_abi::TraceCode::AreaCleared as u64
    );
}

#[test]
fn a_debug_area_that_agrees_is_continued() {
    let mut next = 0;
    let m = restart_over(|area| next = area.next_kernel);
    assert!(next > 1);
    assert!(m.sys.canonical().crash.areas[1].next_kernel >= next);
}

#[test]
fn a_context_configured_again_from_not_configured_reads_consumed_from_0() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0, 1]]));
    m.thread_on(0, 300, 1);
    m.sys.advance(500_000);
    let read = |m: &mut S| match m.call(0, Syscall::SchedRead(s(301))) {
        Ok(Output::SchedRead { consumed, .. }) => consumed,
        other => panic!("{other:?}"),
    };
    assert!(read(&mut m) > 0);
    let configure = |budget, period| Syscall::SchedConfigure {
        sched: s(301),
        node: node_cap(0),
        core: 1,
        budget,
        period,
        class: 0,
    };
    m.ok(0, configure(0, 0));
    m.ok(0, configure(1_000_000, 10_000_000));
    assert_eq!(read(&mut m), 0);
}

// Point 4: the memory_control restricted by the depth rule.

#[test]
fn a_memory_control_is_not_placed_in_a_space_made_from_its_block() {
    let mut m = S::two();
    let shift = m.sys.canonical().geometry.chunk_shift();
    let word = dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw();
    let chunk = m
        .sys
        .canonical()
        .owners
        .iter()
        .position(|&w| w == word)
        .expect("a free chunk") as u64;
    m.ok(0, take(0, chunk << shift, u64::from(shift), 160));
    m.ok(0, retype_from(160, ObjectKind::CSpace, 128, 4000));
    let grow = Syscall::CSpaceGrow {
        cspace: s(4000),
        untyped: s(160),
    };
    assert_eq!(m.call(0, grow), Ok(Output::Slots(128)));
    // A copy of a frame is placed there: the space has room.
    m.ok(0, retype_from(160, ObjectKind::Frame, 1, 4001));
    let frame = Syscall::CapMove {
        source: s(4001),
        destination: SlotAddr::through(4000, 7),
    };
    m.ok(0, frame);
    let into = Syscall::CapMove {
        source: s(CONTROL),
        destination: SlotAddr::through(4000, 5),
    };
    m.no(0, into, Error::Invalid);
    let copy = Syscall::CapDerive {
        source: s(CONTROL),
        destination: SlotAddr::through(4000, 6),
        rights: Rights::ALL.without(Rights::EXEC).bits(),
        badge: 0,
    };
    m.no(0, copy, Error::Invalid);
}

// The rebuild of a unit: the reports.

#[test]
fn a_reported_device_whose_identifier_word_names_another_node_is_not_written() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let other = (2, m.sys.canonical().generation(2));
    let owner = (0, m.sys.canonical().generation(0));
    let node = m.sys.node_mut(holder);
    assert!(!node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root, other));
    assert!(node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root, owner));
}

/// A report whose next key does not rise above the key asked ends that
/// node's report: the rebuild of the unit ends.
#[test]
fn a_report_whose_next_key_does_not_advance_ends_its_nodes_report() {
    let mut m = super::units::three();
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(0, kill);
    let to = m.sys.now() + 200 * MS;
    let mut seen = false;
    let mut fired = 0;
    while m.sys.now() < to && fired < 4000 {
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => {
                let next = m
                    .sys
                    .models()
                    .filter_map(|(_, n)| n.next_timer())
                    .map(|(at, _)| at)
                    .min();
                let Some(next) = next else { break };
                m.sys.move_clock(next.max(m.sys.now() + 1).min(to));
                continue;
            }
        }
        // Node 0 writes, in its own ring to node 2, every `Report` with the
        // key asked (0) as its next key.
        m.sys.edit_table(|t| {
            for entry in std::sync::Arc::make_mut(&mut t.rings)[0][2]
                .kernel
                .entries
                .iter_mut()
                .flatten()
            {
                if let crate::table::RingEntry::Message(msg) = entry
                    && let Body::Report { next, .. } = &mut msg.body
                {
                    *next = 0;
                }
            }
        });
        fired += 1;
        seen |= !m.sys.node(2).rebuilds.is_empty();
    }
    assert!(seen, "node 2 never rebuilt");
    assert!(m.sys.node(2).rebuilds.is_empty(), "the rebuild never ended");
}

// Roles: the hand-over and the rebuild (the code read of lot C).

#[test]
fn a_second_role_hand_of_a_role_in_hand_over_is_busy() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    m.ok(1, place_first(1, 1));
    let second = m.thread_on(1, 300, 1);
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(2),
    };
    assert_eq!(m.call(1, hand.clone()), WAITS);
    assert_eq!(m.call_by(1, second, hand), Err(Error::Busy));
}

#[test]
fn a_node_that_takes_a_unit_role_clears_the_entries_of_a_dead_generation() {
    use super::units::{DEVICE, IOMMU_ROLE, three};
    let mut m = three();
    let kill = Syscall::NodeStop {
        node: node_cap(0),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(2, kill);
    m.run(30 * MS);
    assert_eq!(m.state(0), MemberState::Dead);
    let g = m.sys.member(0).generation();
    // An entry of the dead generation still on the unit node 1 holds (a
    // state a planned hand-over must not carry over); node 1 hands the role
    // to node 2.
    m.sys.edit_table(move |t| {
        t.units
            .entry(IOMMU_ROLE)
            .or_default()
            .entries
            .insert(DEVICE as u16, (0, g));
    });
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(2),
    };
    assert_eq!(m.sys.apply(1, m.t[1], hand), WAITS);
    m.sys.settle(10_000);
    m.sys.advance(m.sys.now() + 30 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(!entries.contains_key(&(DEVICE as u16)));
}

// A context moved while a passive server runs on it, whose home's wait then
// ends: the context is marked moving until its old core has left it.

#[test]
fn a_moved_context_whose_holder_changes_runs_on_its_new_core_only_after_the_leave() {
    let mut k = N::on(&one_node(Arch::X86_64, 3));
    k.place_first(0);
    let home = k.thread_on(300, 1);
    let server = k.thread_on(400, 2);
    k.ok(k.retype(ObjectKind::Endpoint, 0, 500));
    k.ok(k.retype(ObjectKind::Reply, 0, 501));
    let none = Range {
        first: SlotAddr::NONE,
        count: 0,
    };
    let recv = Syscall::Recv {
        endpoint: s(500),
        reply: s(501),
        range: none,
        wait: 0,
    };
    assert_eq!(k.call(server, recv), WAITS);
    k.ok(Syscall::SchedUnbind(s(401)));
    let call = Syscall::Call {
        endpoint: s(500),
        message: Message::default(),
        range: none,
        wait: 0,
    };
    assert_eq!(k.call(home, call), WAITS);
    k.interrupts();
    let context = k.m.object_at(k.t, s(301)).expect("the context");
    assert_eq!(k.m.cores[1].thread, Some(server));
    // The context moves to core 2 while the server runs on it on core 1;
    // then the home's wait ends, so the home becomes its holder.
    k.ok(Syscall::SchedConfigure {
        sched: s(301),
        node: node_cap(0),
        core: 2,
        budget: 200_000,
        period: 10_000_000,
        class: 1,
    });
    assert!(k.m.threads[&server].leaving);
    k.ok(Syscall::ThreadCancel(s(300), 0));
    assert_eq!(k.m.holder(context), Some(home));
    k.m.take_interrupts(2);
    assert_eq!(k.m.check(), Ok(()));
    assert_ne!(k.m.cores[2].running.map(|(c, _)| c), Some(context));
    // Core 1 leaves the server; then core 2 may run the home on it.
    k.m.take_interrupts(1);
    k.interrupts();
    assert!(!k.m.scheds[&context].moving);
    assert_eq!(k.m.cores[2].running.map(|(c, _)| c), Some(context));
}

// The code read of lot B.

#[test]
fn an_aspace_map_while_tables_wait_for_their_invalidation_takes_new_tables_from_the_untyped() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    let other = k.thread_on(170, 1);
    assert_eq!(k.call(other, Syscall::Yield), DONE);
    let ut = k.ut;
    k.ok(k.retype(ObjectKind::ASpace, 0, 360));
    let space = k.m.object_at(k.t, s(360)).expect("an aspace");
    let map = |slot: u32, at: u64| {
        Syscall::ASpaceMap(Map {
            space: s(360),
            frame: s(slot),
            untyped: s(ut),
            address: at,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        })
    };
    for (slot, at) in [(350u32, 0u64), (351, 1 << 40)] {
        k.ok(k.retype(ObjectKind::Frame, 1, slot));
        k.ok(map(slot, at));
    }
    // Core 1 holds the tag of the space: the unmapping is asked of it, and
    // its three tables stay off the free list until core 1 answers.
    k.m.aspaces.get_mut(&space).expect("the aspace").cores |= 1 << 1;
    assert_eq!(k.call(k.t, Syscall::ASpaceUnmap(s(350))), WAITS);
    assert!(!k.m.aspaces[&space].unhooked.is_empty());
    let more = k.retype(ObjectKind::Frame, 1, 352);
    assert_eq!(k.call(other, more), DONE);
    assert_eq!(k.call(other, map(352, 1 << 38)), DONE);
}

#[test]
fn ioport_out_through_a_cspace_capability_needs_only_read_on_it() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    k.ok(k.retype(ObjectKind::CSpace, 128, 200));
    let grow = Syscall::CSpaceGrow {
        cspace: s(200),
        untyped: s(k.ut),
    };
    assert_eq!(k.call(k.t, grow), Ok(Output::Slots(128)));
    k.ok(Syscall::CapMove {
        source: s(dodos_abi::START_SLOT_IOPORTS),
        destination: SlotAddr::through(200, 1),
    });
    k.ok(Syscall::CapDerive {
        source: s(200),
        destination: s(201),
        rights: Rights::READ.bits(),
        badge: 0,
    });
    let out = Syscall::IoportOut {
        ioports: SlotAddr::through(201, 1),
        port: 0x80,
        width: 1,
        value: 0,
    };
    assert_eq!(k.call(k.t, out), DONE);
}

#[test]
fn an_ioports_word_that_names_no_range_admits_no_port() {
    assert!(crate::debug::range_of(1).is_none());
    assert_eq!(
        crate::debug::range_of(0xFFFF << 16),
        Some(dodos_abi::IoPortRange::ALL)
    );
}

#[test]
fn thread_set_features_of_a_thread_marked_leaving_its_core_is_busy() {
    let mut k = N::on(&one_node(Arch::X86_64, 2));
    k.place_first(0);
    k.thread_on(300, 1);
    k.interrupts();
    assert_eq!(
        k.call(k.t, Syscall::ThreadSuspend(s(300))),
        Err(Error::Again)
    );
    let set = Syscall::ThreadSetFeatures {
        thread: s(300),
        mask: 0,
    };
    k.no(set, Error::Busy);
}

// The start-up refuses the values of the machine out of range, never panics
// (D-141; the final review's verifications of lots A, C and D).

fn refused(machine: &Machine) -> Option<crate::boot::Refusal> {
    let outcome = std::panic::catch_unwind(|| Model::boot(machine).map(|_| ()));
    match outcome {
        Ok(result) => result.err(),
        Err(_) => panic!("the start-up panicked"),
    }
}

#[test]
fn the_start_up_refuses_the_values_of_the_machine_out_of_range() {
    use crate::boot::Refusal::{Floors, MachineValues};
    let base = Machine::example();
    type Edit = Box<dyn Fn(&mut Machine)>;
    let edits: Vec<(&str, Edit, crate::boot::Refusal)> = vec![
        (
            "a core of an unknown kind",
            Box::new(|m| m.cores[0].kind = 7),
            MachineValues,
        ),
        (
            "MAXPHYADDR below the chunk",
            Box::new(|m| m.phys_bits = 20),
            MachineValues,
        ),
        (
            "MAXPHYADDR below the memory",
            Box::new(|m| m.phys_bits = 30),
            MachineValues,
        ),
        (
            "MAXPHYADDR of 64",
            Box::new(|m| m.phys_bits = 64),
            MachineValues,
        ),
        (
            "an iospace width of 8",
            Box::new(|m| m.io_width = 8),
            MachineValues,
        ),
        (
            "an iospace width of 64",
            Box::new(|m| m.io_width = 64),
            MachineValues,
        ),
        (
            "a unit of 2^17 domains",
            Box::new(|m| m.iommus[0].domains = 1 << 17),
            MachineValues,
        ),
        (
            "a unit of 15 domains",
            Box::new(|m| m.iommus[0].domains = 15),
            MachineValues,
        ),
        (
            "a crash area off its chunk",
            Box::new(|m| m.crash.0 += m.geometry.page()),
            MachineValues,
        ),
        (
            "two usable ranges that overlap",
            Box::new(|m| {
                let first = m.memory[5];
                m.memory.push(first);
            }),
            MachineValues,
        ),
        (
            "a file of 2^64 - 1 bytes",
            Box::new(|m| m.files[0].len = u64::MAX),
            Floors,
        ),
        (
            "a BootNodes window of 2^58",
            Box::new(|m| m.boot_nodes = Some((1, vec![(1 << 58, 1)]))),
            MachineValues,
        ),
        (
            "a floor of 0",
            Box::new(|m| m.floors.floor_node = 0),
            Floors,
        ),
        (
            "a writable segment the start-up untyped cannot hold",
            Box::new(|m| m.segments[1].len = 8 * m.geometry.chunk()),
            Floors,
        ),
        (
            "a page shift near 255",
            Box::new(|m| m.geometry.page_shift = [12, 255, 1, 0]),
            MachineValues,
        ),
    ];
    for (what, edit, expected) in edits {
        let mut machine = base.clone();
        edit(&mut machine);
        assert_eq!(refused(&machine), Some(expected), "{what}");
    }
    let mut machine = super::machine(Arch::X86_64, 2, &[&[0], &[1]]);
    machine.boot_nodes = Some((2, vec![(1u64 << 58, 1), (0, 0b11)]));
    assert_eq!(refused(&machine), Some(MachineValues), "a window of 2^58");
}

#[test]
fn a_segment_without_read_is_mapped_readable() {
    let mut machine = Machine::example();
    let index = machine
        .segments
        .iter()
        .position(|s| s.rights.contains(Rights::EXEC))
        .expect("an executable segment");
    machine.segments[index].rights = Rights::EXEC;
    let (model, _) = Model::boot(&machine).expect("the boot");
    assert_eq!(model.check(), Ok(()));
}

#[test]
fn the_start_up_page_counts_its_records_at_the_sizes_of_the_abi() {
    // The reference machine's page was counted 2,168 bytes, with 40 bytes
    // for `StartBootTimes` (48), 80 for `StartMachine` (96) and 8 for a
    // table of firmware tables the model never lists (an empty entry): the
    // final review's verification of lot C, H1 = O2.
    // Pass 25: the example lists one more memory range (a descriptor of 48
    // bytes) and keeps the tables of its IOMMU unit (16 bytes in `Kept`).
    // D-152: `StartMachine` is 128 bytes (layout 6, eight more offsets).
    let machine = Machine::example();
    let bytes = crate::boot::start_page_bytes_of(&machine);
    assert_eq!(
        bytes,
        2168 + (48 - 40) + (96 - 80) - 8 + 48 + 16 + (128 - 96)
    );
    assert_eq!(std::mem::size_of::<dodos_abi::StartBootTimes>(), 48);
    assert_eq!(std::mem::size_of::<dodos_abi::StartMachine>(), 128);
}

#[test]
fn a_debugger_stop_during_r1_does_not_fail_the_unit() {
    use super::units::{IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let kill = Syscall::NodeStop {
        node: node_cap(0),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    // The unit confirms nothing for a while: R1's invalidation of node 0's
    // devices waits on it.
    m.ok(2, kill);
    m.sys
        .edit_table(|t| t.units.entry(IOMMU_ROLE).or_default().broken = true);
    m.sys.advance(m.sys.now() + MS);
    m.sys.stub_stop(5 * dodos_abi::IOMMU_WAIT_NS);
    // A step after the release judges the wait.
    m.sys.advance(m.sys.now() + MS);
    assert!(
        !m.sys.role(IOMMU_ROLE).failed(),
        "the stop's time failed the unit"
    );
    m.sys.advance(m.sys.now() + 2 * dodos_abi::IOMMU_WAIT_NS);
    assert!(m.sys.role(IOMMU_ROLE).failed());
}

// The code read of lot C.

#[test]
fn a_spurious_fault_inside_a_large_page_invalidates_the_page_whole() {
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let space = k.m.threads[&k.t].aspace.expect("the start-up space");
    let cap = *k.m.aspaces[&space].records.first().expect("a record");
    let base = 0x4000_0000u64;
    let translation = crate::Translation {
        cap,
        page: 0,
        size: PageSize::Large1,
        rights: Rights::READ,
    };
    let aspace = k.m.aspaces.get_mut(&space).expect("the space");
    aspace.translations.insert(base, translation);
    k.m.cores[0].loaded = Some(space);
    k.m.cores[0].cached = Default::default();
    k.m.cache_here();
    let cached = |k: &N| {
        k.m.cores[0]
            .tlb
            .get(&space)
            .is_some_and(|t| t.contains_key(&base))
    };
    assert!(cached(&k));
    assert!(k.m.spurious(k.t, base + 0x1000, dodos_abi::ACCESS_READ));
    assert!(!cached(&k), "the large page is still in the core's TLB");
}

#[test]
fn a_pending_signal_to_a_generation_that_is_over_is_dropped() {
    let mut m = S::two();
    let generation = m.sys.member(1).generation();
    let entry = crate::table::UseEntry {
        granter: 1,
        grant: 0,
        serial: 0,
        generation,
        root: Some(m.t[0]),
        leaves: 0,
        release_pending: false,
        signal_pending: true,
    };
    let model = m.sys.node_mut(0);
    model.table.uses[0].insert(0, entry);
    model.table.members[1] = dodos_abi::Member::new(
        MemberState::Dead,
        dodos_abi::Evidence::None,
        dodos_abi::DRIVER_NONE,
        generation,
    );
    model.send_pending();
    let still = m.sys.node(0).table.uses[0]
        .get(&0)
        .is_some_and(|u| u.signal_pending);
    assert!(!still);
}

#[test]
fn a_pending_signal_to_a_sealed_granter_waits() {
    let mut m = S::two();
    let generation = m.sys.member(1).generation();
    let entry = crate::table::UseEntry {
        granter: 1,
        grant: 0,
        serial: 0,
        generation,
        root: Some(m.t[0]),
        leaves: 0,
        release_pending: false,
        signal_pending: true,
    };
    let model = m.sys.node_mut(0);
    model.table.uses[0].insert(0, entry);
    model.table.members[1] = dodos_abi::Member::new(
        MemberState::Sealed,
        dodos_abi::Evidence::None,
        dodos_abi::DRIVER_NONE,
        generation,
    );
    model.send_pending();
    // D-147: sent to a sealed node of that generation as to any sealed
    // node: it waits in its ring until it resumes.
    let model = m.sys.node(0);
    assert!(!model.table.uses[0][&0].signal_pending);
    let written = model.ring_of(0, 1, false).produced + model.ring_of(0, 1, true).produced;
    assert_ne!(written, 0, "not in node 1's ring");
}

// Interrupts: a core event, a level line on another node, APIC ID 255.

fn irq_claim(source: u64, core: u64, vector: u64, destination: u32) -> Syscall {
    Syscall::IrqClaim {
        control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
        source,
        core,
        vector,
        destination: s(destination),
    }
}

#[test]
fn a_core_event_is_claimed_with_no_message_words_and_no_unit() {
    let mut machine = machine(Arch::X86_64, 1, &[&[0]]);
    machine.core_events = true;
    let mut k = N::on(&machine);
    let role = k.m.segment_roles(0)[0];
    let event = dodos_abi::IrqSource::core_event(1).raw();
    let outcome = k.call(k.t, irq_claim(event, 0, 48, 200));
    assert_eq!(
        outcome,
        Ok(Output::Msi {
            address: 0,
            data: 0
        })
    );
    let remap = k.m.table.units.get(&role).map(|u| u.remap.len());
    assert!(remap.is_none_or(|n| n == 0));
}

/// A level line claimed by node 0, which does not hold the IOAPIC's role
/// in a machine of three nodes (node 1 holds the unit roles).
fn level_line_claim(arch: Arch, directed: bool) -> Outcome {
    let mut machine = machine(arch, 3, &[&[0], &[1], &[2]]);
    machine.directed_eoi = directed;
    let mut m = S::on(&machine);
    let ioapic = m
        .sys
        .node(0)
        .ioapic_role(5)
        .expect("an IOAPIC serves GSI 5");
    assert_ne!(m.sys.role(ioapic).holder(), 0);
    let line = dodos_abi::IrqSource::line(5, true, false).raw();
    m.call(0, irq_claim(line, 0, 50, 212))
}

#[test]
fn a_level_line_on_a_node_without_the_ioapics_role_needs_a_directed_end_of_interrupt() {
    assert_eq!(
        level_line_claim(Arch::X86_64, false),
        Err(Error::Unsupported)
    );
    assert_ne!(
        level_line_claim(Arch::X86_64, true),
        Err(Error::Unsupported)
    );
    // RISC-V: any node may claim a level line, its gate writing the
    // APLIC's registers by number (D-146; `unsupported` before).
    assert_ne!(
        level_line_claim(Arch::Riscv64, true),
        Err(Error::Unsupported)
    );
    assert_ne!(
        level_line_claim(Arch::Aarch64, false),
        Err(Error::Unsupported)
    );
}

#[test]
fn a_claim_on_a_core_of_apic_id_255_without_remapping_is_unsupported() {
    let mut machine = machine(Arch::X86_64, 1, &[&[0]]);
    machine.remapping = false;
    machine.cores[0].hw_id = 255;
    let mut k = N::on(&machine);
    let msi = dodos_abi::IrqSource::msi(0, 1, 0, 0).raw();
    k.no(irq_claim(msi, 0, 48, 200), Error::Unsupported);
}

// `iospace_map` where the units cache entries that are not present.

pub(super) fn caching_iospace() -> (S, u32, u32) {
    use super::units::{IOSPACE, attached};
    let mut machine = machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]);
    machine.caching_mode = true;
    let mut m = S::on(&machine);
    attached(&mut m, 0);
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(0, retype_from(ut, ObjectKind::Frame, 1, 160));
    (m, ut, IOSPACE)
}

pub(super) fn io_map(ut: u32, iospace: u32) -> Syscall {
    Syscall::IoSpaceMap(Map {
        space: s(iospace),
        frame: s(160),
        untyped: s(ut),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    })
}

#[test]
fn an_iospace_map_where_units_cache_absent_entries_waits_for_their_invalidation() {
    let (mut m, ut, iospace) = caching_iospace();
    assert_eq!(m.call(0, io_map(ut, iospace)), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
}

#[test]
fn an_iospace_map_whose_unit_fails_is_busy_with_the_mapping_kept() {
    use super::units::IOMMU_ROLE;
    let (mut m, ut, iospace) = caching_iospace();
    m.sys.break_unit(IOMMU_ROLE);
    assert_eq!(m.call(0, io_map(ut, iospace)), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
    let frame = m.sys.node(0).object_at(m.t[0], s(160)).expect("the frame");
    let root = m.sys.node(0).objects[&frame].root;
    assert!(matches!(
        m.sys.node(0).caps[&root].record,
        crate::Record::Mapping(_)
    ));
}

#[test]
fn the_holder_judges_a_masked_line_with_the_generation_its_word_holds() {
    use super::units::{IOAPIC_ROLE, three};
    let mut m = three();
    let g = m.sys.canonical().generation(0);
    // GSI 5's word taken by an ended generation of node 0.
    let old = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(0), g + 7);
    m.sys.edit_table(move |t| {
        t.write_line(0, 5, Some(old));
    });
    let op = crate::rings::UnitOp::Redirection {
        gsi: 5,
        claimed: false,
        level: true,
        low: false,
        target: crate::rings::Target::Masked,
    };
    let holder = m.sys.role(IOAPIC_ROLE).holder();
    assert!(!m.sys.node(holder).operands_allowed(IOAPIC_ROLE, op, (0, g)));
}

#[test]
fn the_holder_serves_an_operation_only_on_a_unit_of_its_kind() {
    use super::units::{IOAPIC_ROLE, IOMMU_ROLE, three};
    let m = three();
    let g = m.sys.canonical().generation(0);
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let masked = crate::rings::UnitOp::Redirection {
        gsi: 5,
        claimed: false,
        level: true,
        low: false,
        target: crate::rings::Target::Masked,
    };
    assert!(
        m.sys
            .node(holder)
            .operands_allowed(IOAPIC_ROLE, masked, (0, g))
    );
    assert!(
        !m.sys
            .node(holder)
            .operands_allowed(IOMMU_ROLE, masked, (0, g))
    );
    let range = m.sys.node(holder).domain_range(0, 0);
    let invalidate = crate::rings::UnitOp::Invalidate {
        domain: Some(*range.start()),
    };
    assert!(
        m.sys
            .node(holder)
            .operands_allowed(IOMMU_ROLE, invalidate, (0, g))
    );
    assert!(
        !m.sys
            .node(holder)
            .operands_allowed(IOAPIC_ROLE, invalidate, (0, g))
    );
}

/// Runs the machine event by event for at most `ms`, letting node 0 rewrite
/// the `Report`s it leaves in its own ring to node 2.
fn run_rewriting_reports(m: &mut S, ms: u64, rewrite: impl Fn(&mut Body)) {
    let to = m.sys.now() + ms * MS;
    let mut fired = 0;
    while m.sys.now() < to && fired < 6000 {
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => {
                let next = m
                    .sys
                    .models()
                    .filter_map(|(_, n)| n.next_timer())
                    .map(|(at, _)| at)
                    .min();
                let Some(next) = next else { break };
                m.sys.move_clock(next.max(m.sys.now() + 1).min(to));
                continue;
            }
        }
        m.sys.edit_table(|t| {
            for entry in std::sync::Arc::make_mut(&mut t.rings)[0][2]
                .kernel
                .entries
                .iter_mut()
                .flatten()
            {
                if let crate::table::RingEntry::Message(msg) = entry
                    && matches!(msg.body, Body::Report { .. })
                {
                    rewrite(&mut msg.body);
                }
            }
        });
        fired += 1;
    }
}

#[test]
fn a_rebuild_clears_an_entry_whose_report_names_another_root() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(0, kill);
    run_rewriting_reports(&mut m, 300, |body| {
        if let Body::Report { items, .. } = body {
            for item in items.iter_mut() {
                if let crate::rings::Item::Device(_, root) = item {
                    *root += 0x1000;
                }
            }
        }
    });
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(m.sys.node(2).rebuilds.is_empty());
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(!entries.contains_key(&(DEVICE as u16)));
}

#[test]
fn a_rebuild_keeps_the_entries_of_a_node_it_passed_over_sealed_even_once_it_resumes() {
    use super::units::{DEVICE, IOMMU_ROLE, attached};
    // Four nodes, so that the rebuild waits for node 3's report after it
    // passed node 0.
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    attached(&mut m, 0);
    let member = |state| {
        move |t: &mut crate::table::Table| {
            let w = t.member(0);
            t.members[0] = dodos_abi::Member::new(state, w.evidence(), w.driver(), w.generation())
                .with_class(w.class());
        }
    };
    // Node 0 reads sealed (its words as a seal leaves them) while node 1,
    // the holder, is killed and node 2 rebuilds the unit.
    m.sys.edit_table(member(MemberState::Sealed));
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.sys.apply(2, m.t[2], kill).expect("the kill");
    let to = m.sys.now() + 300 * MS;
    let mut resumed = false;
    while m.sys.now() < to {
        let passed = m
            .sys
            .node(2)
            .rebuilds
            .get(&IOMMU_ROLE)
            .is_some_and(|r| r.node > 0);
        if passed && !resumed {
            // Node 0 resumes before the rebuild ends.
            m.sys.edit_table(member(MemberState::Live));
            resumed = true;
        }
        match m.sys.events().first() {
            Some(&event) => m.sys.fire(event),
            None => {
                let next = m
                    .sys
                    .models()
                    .filter_map(|(_, n)| n.next_timer())
                    .map(|(at, _)| at)
                    .min();
                let Some(next) = next else { break };
                m.sys.move_clock(next.max(m.sys.now() + 1).min(to));
            }
        }
        if resumed && m.sys.node(2).rebuilds.is_empty() {
            break;
        }
    }
    assert!(resumed, "the rebuild never passed node 0");
    assert!(m.sys.node(2).rebuilds.is_empty());
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(entries.contains_key(&(DEVICE as u16)));
}

#[test]
fn a_rebuild_keeps_the_reported_remapping_handles_and_clears_the_others() {
    use super::units::{IOMMU_ROLE, three};
    let mut m = three();
    let msi = dodos_abi::IrqSource::msi(0, 1, 0, 0).raw();
    let outcome = m.call(0, irq_claim(msi, 0, 50, 212));
    if outcome == WAITS {
        m.run(30 * MS);
    }
    let claimed = 50 - u32::from(dodos_abi::x86_64::IRQ_VECTOR_FIRST);
    assert!(
        m.sys
            .node(0)
            .items(IOMMU_ROLE)
            .contains(&crate::rings::Item::Handle(claimed))
    );
    let g = m.sys.canonical().generation(0);
    let stale = claimed + 7;
    m.sys.edit_table(move |t| {
        t.units
            .entry(IOMMU_ROLE)
            .or_default()
            .remap
            .insert(stale, (0, g));
    });
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.sys.apply(0, m.t[0], kill).expect("the kill");
    m.sys.settle(100_000);
    m.sys.advance(m.sys.now() + 300 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert!(m.sys.node(2).rebuilds.is_empty());
    let remap = &m.sys.canonical().units[&IOMMU_ROLE].remap;
    assert!(remap.contains_key(&claimed));
    assert!(!remap.contains_key(&stale));
}

// ver-C H4 and finding 8 of `v2-C-hydrogen.md`: a core that goes idle
// answers its pending set; the request it completes wakes a waiter, whose
// context is placed in its queue in the same event, and the core chooses
// again when that context is placed on it.

#[test]
fn a_core_going_idle_queues_the_waiter_its_answer_wakes() {
    let (mut k, other) = super::cores::two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Notify, 0, 330));
    assert_eq!(
        k.call(k.t, Syscall::ASpaceUnmap(s(super::cores::FRAME))),
        WAITS
    );
    assert_eq!(k.m.cores[1].pending.len(), 1);
    // Core 1 goes idle before it takes the invalidation interrupt.
    assert_eq!(k.call(other, Syscall::Wait(s(330), 0)), WAITS);
    assert_eq!(k.m.woken(k.t), Some(&DONE));
    assert!(k.m.cores[0].ipis.contains(&crate::Ipi::Reschedule));
}

#[test]
fn a_core_going_idle_runs_the_waiter_its_answer_wakes_onto_it() {
    let (mut k, other) = super::cores::two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Notify, 0, 330));
    assert_eq!(
        k.call(k.t, Syscall::ASpaceUnmap(s(super::cores::FRAME))),
        WAITS
    );
    // The waiter's context moves to core 1.
    assert_eq!(k.call(other, place_first(k.m.node, 1)), DONE);
    assert_eq!(k.call(other, Syscall::Wait(s(330), 0)), WAITS);
    assert_eq!(k.m.cores[1].thread, Some(k.t));
    assert!(!k.m.cores[1].idle.mark);
}

// ver-C H15: a spare core is in the park stub on x86-64 and off on ARM64
// and RISC-V.

#[test]
fn a_spare_core_is_parked_on_x86_64_and_off_on_arm64_and_riscv() {
    use crate::system::Hardware;
    for arch in [Arch::X86_64, Arch::Aarch64, Arch::Riscv64] {
        let m = S::on(&machine(arch, 3, &[&[0], &[1]]));
        let spare = m.sys.hardware(2);
        match arch {
            Arch::X86_64 => assert!(matches!(spare, Hardware::Stub { .. })),
            _ => assert_eq!(spare, Hardware::Off, "{arch:?}"),
        }
    }
}

// ver-C H8: a node that resumes consumes what waited in its rings, under
// the check, before it writes live; and finishes R2 and R3 of a generation
// that ended while it was sealed before it goes on.

#[test]
fn a_resume_consumes_what_waited_in_its_user_ring_before_it_is_live() {
    use super::pass17::{resume, seal};
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    m.ok(1, seal(0));
    m.run(20 * MS);
    assert_eq!(m.state(0), MemberState::Sealed);
    // An entry waits in node 0's user ring; its interrupt reached a sealed
    // node.
    let header = RingHeader {
        kind: NodeMessage::Send as u8,
        length: 0,
        sender: 1,
        sender_generation: m.sys.canonical().generation(1),
        receiver_generation: m.sys.canonical().generation(0),
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    m.sys.inject_raw(1, 0, true, header, 1);
    m.ok(1, resume(0));
    for _ in 0..100_000 {
        if m.state(0) == MemberState::Live {
            break;
        }
        match m.sys.events().first().copied() {
            Some(event) => m.sys.fire(event),
            None => {
                let next = m.sys.now() + 100_000;
                m.sys.move_clock(next);
            }
        }
    }
    assert_eq!(m.state(0), MemberState::Live);
    let table = m.sys.canonical();
    assert_eq!(
        table.beacons[0].consumed[1].1,
        table.rings[1][0].user.produced
    );
}

#[test]
fn a_resume_after_a_peer_died_while_sealed_waits_until_r2_removed_every_remote_capability_of_it() {
    use super::pass17::{kill, resume, seal};
    use crate::{Message, Range, Transfer};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 2 grants node 1 a frame, which node 1 copies 40 times: R2's
    // removal of its root takes two steps of 32 units.
    let frame = m.retype(2, ObjectKind::Frame, 1, 101);
    m.ok(2, frame);
    let reply = m.retype(1, ObjectKind::Reply, 0, 100);
    m.ok(1, reply);
    let recv = Syscall::Recv {
        endpoint: s(dodos_abi::START_SLOT_MAILBOX),
        reply: s(100),
        range: Range {
            first: s(110),
            count: 1,
        },
        wait: 0,
    };
    assert_eq!(m.call(1, recv), WAITS);
    let rights = dodos_abi::Rights::READ
        .union(dodos_abi::Rights::DERIVE)
        .bits();
    let send = Syscall::NodeSend {
        node: node_cap(1),
        message: Message {
            label: 7,
            words: vec![7],
            caps: vec![Transfer::Derive(s(101), rights)],
            ..Message::default()
        },
        doorbell: SlotAddr::NONE,
    };
    assert_eq!(m.call(2, send), WAITS);
    m.settle();
    for n in 0..40 {
        m.ok(
            1,
            Syscall::CapDerive {
                source: s(110),
                destination: s(600 + n),
                rights,
                badge: 0,
            },
        );
    }
    let held = |m: &S| {
        let node = m.sys.node(1);
        let remote = |object: &ObjId| node.remotes.get(object).is_some_and(|r| r.node == 2);
        node.caps.values().filter(|c| remote(&c.object)).count()
    };
    assert_eq!(held(&m), 41);
    m.ok(0, seal(1));
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Sealed);
    // Node 2 dies while node 1 is sealed: its generation ended.
    m.ok(0, kill(2));
    m.run(10 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    m.ok(0, resume(1));
    let mut live = false;
    for _ in 0..100_000 {
        if m.sys.settle(1) == 0 {
            let now = m.sys.now();
            m.sys.advance(now + 50_000);
        }
        if m.state(1) == MemberState::Live {
            live = true;
            break;
        }
    }
    assert!(live);
    assert_eq!(held(&m), 0);
}

// ver-C O22: a fresh start that the node refuses (its start-up page would
// not fit) leaves its core parked again, never entering for ever.

#[test]
fn a_start_whose_page_does_not_fit_parks_its_core_again() {
    use crate::system::Hardware;
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    m.ok(
        0,
        Syscall::NodeStop {
            node: node_cap(1),
            mode: dodos_abi::NODE_STOP_KILL,
            core: 0,
        },
    );
    m.run(10 * MS);
    assert_eq!(m.state(1), MemberState::Dead);
    // The machine now lists so many kinds of core that a start-up page
    // listing them passes its limit (a test's input).
    let kind = m.sys.machine.kinds[0];
    m.sys
        .machine
        .kinds
        .extend(std::iter::repeat_n(kind, 1 << 16));
    m.ok(
        0,
        Syscall::NodeStart {
            node: node_cap(1),
            mode: dodos_abi::NODE_START_FRESH,
            image: 0,
            module: 0,
            window: 0,
            mask: 0b10,
        },
    );
    m.run(dodos_abi::START_NS + 10 * MS);
    assert_ne!(m.state(1), MemberState::Live);
    assert!(
        matches!(m.sys.hardware(1), Hardware::Stub { .. }),
        "{:?}",
        m.sys.hardware(1)
    );
}

// ver-B O5: an address past the last ownership word is in no chunk.

#[test]
fn an_address_past_the_ownership_words_is_not_chunked() {
    let k = N::on(&Machine::example());
    let shift = k.m.table.geometry.chunk_shift();
    let past = (k.m.table.owners.len() as u64) << shift;
    assert!(!k.m.chunked_or_kept(past));
}

// ver-B O21: every write through a gate first reads the writer's member
// word; a node that does not write (here fenced) writes no line word and no
// identifier word, and the check reports a write journaled by such a node.

#[test]
fn a_gate_whose_node_does_not_write_writes_no_line_or_identifier_word() {
    use dodos_abi::{Member, Owner, Ownership};
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let me = k.m.node;
    let live = k.m.table.member(me);
    let fenced = Member::new(
        MemberState::Fenced,
        live.evidence(),
        live.driver(),
        live.generation(),
    );
    k.m.table.write_member(me, me, fenced);
    let taken = Ownership::taken(Owner::Node(me), live.generation());
    k.m.table.write_line(me, 5, Some(taken));
    k.m.table.write_identifier(me, 0x10, Some(taken));
    assert!(!k.m.table.lines.contains_key(&5));
    assert!(!k.m.table.identifiers.contains_key(&0x10));
    let unclaimed = Ownership::free(Owner::Unclaimed).raw();
    let write =
        k.m.table
            .journal(me, crate::table::Word::Line(5), unclaimed, taken.raw());
    k.m.table.write_member(me, me, live);
    k.m.table.writes.push(write);
    assert_eq!(k.m.check(), Err("52: write of an ownership word"));
}

// ver2-D 1: invariant 52 refuses a take journaled in the old owner's name
// (here unclaimed) instead of the writer's.

#[test]
fn a_take_journaled_in_the_old_owners_name_is_reported() {
    use dodos_abi::{Owner, Ownership};
    let mut k = N::on(&one_node(Arch::X86_64, 1));
    let me = k.m.node;
    let g = k.m.table.generation(me);
    let unclaimed = Ownership::free(Owner::Unclaimed).raw();
    let in_its_name = Ownership::taken(Owner::Unclaimed, g).raw();
    let write =
        k.m.table
            .journal(me, crate::table::Word::Line(5), unclaimed, in_its_name);
    k.m.table.writes.push(write);
    assert_eq!(k.m.check(), Err("52: write of an ownership word"));
}

// ver2-D 5: G4 for a device entry is the leaves while the node that owns
// the entry runs, as for a chunk, not only at least them.

#[test]
fn a_device_leaf_count_above_its_leaves_is_reported_while_its_owner_runs() {
    use super::units::{IOSPACE, attached, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    attached(&mut m, 0);
    m.ok(0, take(1, 0, 0, 160));
    let frame = Syscall::UntypedRetype {
        untyped: s(160),
        kind: ObjectKind::Frame,
        size: 1,
        page: 0,
        memory: dodos_abi::MemoryType::Uncached as u64,
        destination: s(161),
    };
    m.ok(0, frame);
    let map = Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(161),
        untyped: s(ut),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    m.ok(0, map);
    m.settle();
    m.sys
        .edit_table(|t| std::sync::Arc::make_mut(&mut t.device_leaves)[0] += 1);
    assert_eq!(
        m.sys.check(),
        Err("G4: a device leaf count is not the leaves of its entry".into())
    );
}

// ver2-D 4: a root remote capability has the anchor's rights, no more (3);
// its children, mappings and translations are bounded by it through 3, 38
// and 35.

#[test]
fn a_remote_root_with_rights_beyond_the_anchor_is_reported() {
    let mut m = granted(true);
    let got = m.sys.node(1).object_at(m.t[1], s(GOT)).expect("the frame");
    let root = m.sys.node(1).objects[&got].root;
    m.sys.node_mut(1).caps.get_mut(&root).unwrap().rights = Rights::ALL;
    assert_eq!(m.sys.check(), Err("node 1: 3: root capability".into()));
}

// v2-C-hydrogen findings 9 and 10: a `Send` whose words disagree with its
// header (65 words, more than a header can say) or whose marks have a
// reserved bit is refused by the receive check: the receiver neither
// panics nor delivers it nor writes the bit into a task's marks.

fn send_from_1(m: &mut S, words: usize, marks: u64) {
    let task = dodos_abi::TaskId::new(1, 1).raw();
    let body = Body::Send {
        header: Header::new(7, 1, 0, 0).expect("a header"),
        stamp: (task, marks),
        grants: vec![],
        words: vec![42; words],
    };
    let send = forged(m, NodeMessage::Send, 1, 0, Some((16, 1)), body);
    m.sys.inject(1, 0, send);
}

#[test]
fn a_send_whose_words_disagree_with_its_header_is_refused() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    assert_eq!(m.call(0, recv(GOT)), WAITS);
    let before = m.sys.node(0).refused.0;
    send_from_1(&mut m, dodos_abi::MAX_WORDS + 1, 0);
    m.run(20 * MS);
    assert!(m.woken(0).is_none(), "the send was delivered");
    assert_eq!(m.sys.node(0).refused.0, before + 1);
}

#[test]
fn a_send_whose_marks_have_a_reserved_bit_is_refused() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    assert_eq!(m.call(0, recv(GOT)), WAITS);
    let before = m.sys.node(0).refused.0;
    send_from_1(&mut m, 1, 0b100);
    m.run(20 * MS);
    assert!(m.woken(0).is_none(), "the send was delivered");
    assert_eq!(m.sys.node(0).refused.0, before + 1);
    // The same send with known marks is delivered.
    send_from_1(&mut m, 1, 0b01);
    m.run(20 * MS);
    assert!(m.woken(0).is_some());
}

// v2-C-hydrogen finding 14: a `SendAnswer` that says *not delivered* and
// still names a use entry is refused by the check, never recorded.

#[test]
fn a_send_answer_not_delivered_that_names_a_use_entry_is_refused() {
    let mut m = granted(false);
    let credits = &m.sys.node(0).peers[1].credits;
    let index = credits
        .iter()
        .position(|c| c.is_some())
        .expect("the send's credit");
    let credit = (index as u8, credits[index].as_ref().unwrap().serial);
    let before = m.sys.node(0).refused.0;
    let answer = Body::SendAnswer {
        delivered: false,
        dropped: false,
        uses: vec![Some(0)],
    };
    let answer = forged(&m, NodeMessage::SendAnswer, 1, 0, Some(credit), answer);
    m.sys.inject(1, 0, answer);
    m.settle();
    assert_eq!(m.sys.node(0).refused.0, before + 1);
    // The real answer then records the grant, delivered.
    let (grant, _) = the_grant(&m);
    assert!(m.sys.canonical().grants[0][&grant].answered);
    assert!(matches!(m.woken(0), Some(Ok(Output::Sent { .. }))));
}

// ver-C H2: the copied frames of the writable segments are counted where
// they are placed, from the start-up untyped's watermark, so the first
// service has 4,096 empty slots or more whatever the alignment.

#[test]
fn the_start_up_space_counts_the_copied_frames_at_their_place() {
    // A floor of 8 chunks, so that the start-up untyped holds large pages;
    // two writable segments of two chunks whose virtual addresses are 16 KB
    // past a chunk: from the untyped's base each is two runs of pages;
    // placed past the 16 KB of the first writable segment, physical and
    // virtual agree on the chunk and each is three frames (pages, a large
    // page, pages). The files move the count across each multiple of 128.
    // On 4 KB pages whatever the run's geometry: the addresses are chosen
    // for that chunk.
    for files in 0..128 {
        let mut machine = Machine::example_on(dodos_abi::x86_64::GEOMETRY);
        let chunk = machine.geometry.chunk();
        machine.floors.floor_node = 8 * chunk;
        for vaddr in [4 * chunk + 0x4000, 8 * chunk + 0x4000] {
            machine.segments.push(crate::boot::Segment {
                vaddr,
                len: 2 * chunk,
                rights: Rights::READ.union(Rights::WRITE),
                name: 4,
            });
        }
        let file = machine.files[0];
        machine.files.extend(std::iter::repeat_n(file, files));
        let k = N::on(&machine);
        let space =
            k.m.object_at(k.t, SlotAddr::own(dodos_abi::START_SLOT_CSPACE))
                .unwrap();
        let empty = k.m.cspaces[&space]
            .slots
            .iter()
            .filter(|s| matches!(s, crate::Slot::Empty))
            .count();
        assert!(
            empty >= dodos_abi::START_FREE_SLOTS as usize,
            "{files} files: {empty}"
        );
    }
}
