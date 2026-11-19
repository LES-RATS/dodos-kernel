//! The verification of the thirteenth model review: each confirmed fault
//! with the test that fails without its correction. States are reached by
//! user calls, by values of the machine, or by what a kernel writes into
//! its own rings (`System::inject`).

use super::*;
use crate::rings::Body;
use crate::{Map, Message, Range, Transfer};
use dodos_abi::{Header, NodeMessage, START_SLOT_ASPACE, START_SLOT_MAILBOX};

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

fn send_obj(to: u16) -> Syscall {
    Syscall::NodeSend {
        node: node_cap(to),
        message: Message {
            label: 7,
            flags: 0,
            words: vec![7],
            caps: vec![Transfer::Derive(s(OBJ), rights())],
            marks: 0,
        },
        doorbell: SlotAddr::NONE,
    }
}

/// Three nodes; node 0 grants node 1 one frame (slot `OBJ`), which node 1
/// receives in slot `GOT` and maps; settled.
fn granted_and_mapped() -> S {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    for node in [0, 1] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    assert_eq!(m.call(0, send_obj(1)), WAITS);
    m.settle();
    let map = Syscall::ASpaceMap(Map {
        space: s(START_SLOT_ASPACE),
        frame: s(GOT),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.union(Rights::WRITE).bits(),
    });
    assert_eq!(m.call(1, map), DONE);
    m
}

// Finding 1. A `RevokeStep` that reports the root removed frees the grant
// entry, and the anchor stays until the revoking thread's next call; in
// between, the index must not be handed to a new grant ("the cost is an
// entry that stays used", Grants, release by the holder).
#[test]
fn a_grant_whose_anchor_stays_is_not_reused_by_the_next_node_send() {
    let mut m = granted_and_mapped();
    let grant = m.sys.node(0).grant_entries()[0].0;
    m.ok(0, place_first(0, 0));
    let other = m.thread_on(0, 300, 0);
    assert_eq!(m.call(0, Syscall::CapRevoke(s(OBJ))), WAITS);
    m.run(30 * MS);
    // The holder removed its root and answered; node 0 freed the entry,
    // and the removing thread, woken with `again`, has not called again.
    assert!(!m.sys.canonical().grants[0].contains_key(&grant));
    assert!(m.sys.node(0).anchor(grant).is_some());
    let outcome = m.sys.apply(0, other, send_obj(2));
    assert_eq!(m.sys.check(), Ok(()), "{outcome:?}");
    // The new grant took another index; the old anchor's removal ends.
    let granted: Vec<u16> = m.sys.canonical().grants[0].keys().copied().collect();
    assert!(!granted.contains(&grant), "{granted:?}");
    m.settle();
    let again = m.call(0, Syscall::CapRevoke(s(OBJ)));
    assert!(m.sys.node(0).anchor(grant).is_none(), "{again:?}");
}

// Finding 2. At most 16 entries per ring per interrupt (Processing): a
// message that would carry the step past its sixteenth entry waits for
// the next interrupt.
#[test]
fn one_interrupt_never_serves_more_than_sixteen_entries() {
    let mut m = S::two();
    let reply = m.retype(0, ObjectKind::Reply, 0, REPLY);
    m.ok(0, reply);
    for i in 0..4u64 {
        // A `Send` of five entries.
        let body = Body::Send {
            header: Header::new(7, 30, 0, 0).expect("a header"),
            stamp: (0, 0),
            grants: vec![],
            words: vec![i; 30],
        };
        let send =
            super::pass22::forged(&m, NodeMessage::Send, 1, 0, Some((16 + i as u8, 1)), body);
        assert_eq!(send.body.entries(), 5);
        m.sys.inject(1, 0, send);
    }
    let start = m.sys.canonical().beacons[0].consumed[1].1;
    m.sys.fire(crate::system::Event::Ipi { node: 0, core: 0 });
    let served = m.sys.canonical().beacons[0].consumed[1].1 - start;
    assert!(
        served <= dodos_abi::RING_USER_STEP,
        "{served} entries in one interrupt"
    );
}

#[track_caller]
fn refused(machine: &Machine) -> bool {
    let refusal = System::boot(machine).err();
    eprintln!("{refusal:?}");
    refusal == Some(crate::boot::Refusal::MachineValues)
}

// Finding 3. MAXPHYADDR "below the end of a usable or kept range" is
// refused (Start-up state, D-141).
#[test]
fn a_kept_range_past_maxphyaddr_is_refused() {
    let mut machine = Machine::example();
    assert!(System::boot(&machine).is_ok());
    machine.kept.push((1 << machine.phys_bits, 4096));
    assert!(refused(&machine));
}

// Finding 4. A crash area whose range overlaps another kept range is
// refused (Start-up state, D-141).
#[test]
fn a_crash_area_over_a_kept_range_is_refused() {
    let mut machine = Machine::example();
    machine.kept.push((machine.crash.0 + 4096, 4096));
    assert!(refused(&machine));
    // The previous crash area as well.
    let mut machine = Machine::example();
    let previous = machine.crash.1.expect("a previous crash area");
    machine.kept.push((previous + 4096, 4096));
    assert!(refused(&machine));
}

// Missed by the review. With 31 cores or more a crash area takes two
// chunks, so two crash areas one chunk apart overlap: the same refusal.
#[test]
fn two_crash_areas_closer_than_their_length_are_refused() {
    let cores: Vec<usize> = (0..32).collect();
    let mut machine = machine(Arch::X86_64, 32, &[&cores]);
    let chunk = machine.geometry.chunk();
    let crash_len = (4096 + 32 * dodos_abi::DEBUG_AREA_BYTES as u64).next_multiple_of(chunk);
    assert_eq!(crash_len, machine.crash_len());
    // On 16 KB and 32 KB pages a chunk holds 32 debug areas: two areas on
    // distinct chunk boundaries cannot overlap there.
    if crash_len == chunk {
        return;
    }
    // Apart by their length (as `machine` places them), the machine boots.
    assert!(System::boot(&machine).is_ok());
    let (crash, _) = machine.crash;
    machine.crash = (crash, Some(crash + chunk));
    assert!(refused(&machine));
}

fn map_got() -> Syscall {
    Syscall::ASpaceMap(Map {
        space: s(START_SLOT_ASPACE),
        frame: s(GOT),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: Rights::READ.union(Rights::WRITE).bits(),
    })
}

// Finding 12 (own-15). The holder's kernel answers, in its own ring, a use
// index other than the one its gate wrote; the granting node records it.
// A map under that grant must be `gone`: a leaf counted in another entry
// than the recorded one would let the granting node read 0 under a live
// translation (Grants, the count says what the holder translates, D-141).
#[test]
fn a_map_under_a_grant_that_recorded_another_use_index_is_gone() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    for node in [0, 1] {
        let reply = m.retype(node, ObjectKind::Reply, 0, REPLY);
        m.ok(node, reply);
    }
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv(GOT)), WAITS);
    assert_eq!(m.call(0, send_obj(1)), WAITS);
    // Node 1's kernel rewrites, in its own ring to node 0, the use index of
    // its `SendAnswer` before node 0 consumes it.
    let grant = m.sys.node(0).grant_entries()[0].0;
    for _ in 0..100_000 {
        m.sys.edit_table(|t| {
            for entry in std::sync::Arc::make_mut(&mut t.rings)[1][0]
                .kernel
                .entries
                .iter_mut()
                .flatten()
            {
                if let crate::table::RingEntry::Message(msg) = entry
                    && let Body::SendAnswer { uses, .. } = &mut msg.body
                {
                    *uses = vec![Some(4095)];
                }
            }
        });
        if m.sys.canonical().grants[0][&grant].answered {
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
    assert_eq!(m.sys.check(), Ok(()));
    let grant = m.sys.node(0).grant_entries()[0].0;
    assert_eq!(m.sys.canonical().grants[0][&grant].use_index, Some(4095));
    assert_eq!(m.call(1, map_got()), Err(crate::Error::Gone));
}

// Finding 14 (own-02, the direction that survives). A frame of a reserve
// chunk the node took while it holds the reserve role is memory its node
// holds: the gate writes its anchor (G3 (2), D-141).
#[test]
fn a_frame_of_a_reserve_chunk_the_node_holds_is_granted() {
    let mut m = super::units::three();
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
    assert!(matches!(
        m.call(0, Syscall::RoleClaim(s(roles[2]))),
        Ok(Output::Epoch(_))
    ));
    m.ok(
        0,
        super::pass22::take(0, chunk << shift, u64::from(shift), 160),
    );
    m.ok(0, retype_from(160, ObjectKind::Frame, 1, OBJ));
    assert_eq!(m.call(0, send_obj(1)), WAITS);
}

// Finding 13 (own-01). The serial is 30 bits: an entry freed at the
// largest serial is reused with serial 0 (Grants, the grant table).
#[test]
fn a_grant_freed_at_the_largest_serial_comes_back_with_serial_0() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    let frame = m.retype(0, ObjectKind::Frame, 1, OBJ);
    m.ok(0, frame);
    // Node 0's own word of the living part, as 2^30 - 1 reuses leave it.
    m.sys.edit_table(|t| {
        t.serials[0].insert(0, 0x3FFF_FFFF);
    });
    // Node 1 receives nothing: the `SendAnswer` says not delivered and the
    // entry is freed.
    assert_eq!(m.call(0, send_obj(1)), WAITS);
    m.settle();
    assert!(m.sys.canonical().grants[0].is_empty());
    assert_eq!(m.sys.canonical().serials[0].get(&0), Some(&0));
}

/// The Refused entries of core `core`'s kernel trace that name a `Report`.
fn report_refusals(m: &S, core: usize) -> usize {
    m.sys
        .node(0)
        .trace_ring(core, false)
        .iter()
        .filter(|e| {
            e.word2 & 0xFFFF == dodos_abi::TraceCode::Refused as u64
                && e.payload[1] == NodeMessage::Report as u64
        })
        .count()
}

/// Node 0 attaches the device, node 0 kills node 1 (the unit's holder) and
/// node 2 rebuilds the unit, asking node 0; `rewrite` edits each `Report`
/// node 0 has written in its own ring to node 2 (a faulty kernel's plain
/// stores). Returns the model once the rebuild has ended.
fn rebuild_with(mut rewrite: impl FnMut(&mut u64, &mut Vec<crate::rings::Item>)) -> S {
    let mut m = super::units::three();
    super::units::attached(&mut m, 0);
    let kill = Syscall::NodeStop {
        node: node_cap(1),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(0, kill);
    let to = m.sys.now() + 200 * MS;
    let (mut seen, mut fired) = (false, 0);
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
        m.sys.edit_table(|t| {
            for entry in std::sync::Arc::make_mut(&mut t.rings)[0][2]
                .kernel
                .entries
                .iter_mut()
                .flatten()
            {
                if let crate::table::RingEntry::Message(msg) = entry
                    && let Body::Report { next, items } = &mut msg.body
                {
                    rewrite(next, items);
                }
            }
        });
        fired += 1;
        seen |= !m.sys.node(2).rebuilds.is_empty();
    }
    assert!(seen, "node 2 never rebuilt");
    assert!(m.sys.node(2).rebuilds.is_empty(), "the rebuild never ended");
    m.run(MS);
    m
}

// Finding 16 (own-04). A `Report` whose count passes 28 is traced and
// ends that node's report (Roles, the reports are pulled, D-141).
#[test]
fn a_report_of_29_items_is_refused_and_traced() {
    use crate::rings::Item;
    let m = rebuild_with(|next, items| {
        if items.len() != 29 {
            *items = (0..29).map(Item::Handle).collect();
            *next = dodos_abi::REPORT_END;
        }
    });
    assert!(report_refusals(&m, 2) > 0);
}

// Finding 18 (own-08). The next key of a `Report` is the key of the next
// item, so the item a node reports first when asked from it has that key:
// it is taken (Roles, the reports are pulled).
#[test]
fn a_report_asked_from_a_key_takes_the_item_of_that_key() {
    let baseline = report_refusals(&rebuild_with(|_, _| {}), 2);
    // Node 0's first `Report` names its first item as the next key and
    // carries no item; the second, unedited, begins with that item.
    let mut edited = false;
    let m = rebuild_with(|next, items| {
        if !edited && !items.is_empty() {
            *next = items[0].key();
            items.clear();
            edited = true;
        }
    });
    assert!(edited);
    assert_eq!(report_refusals(&m, 2), baseline);
}

// Finding 19 (own-09). A reported device whose identifier word names
// another node is "traced (code 11) and claiming nothing" (Roles,
// take-over after a death, D-141).
#[test]
fn a_reported_device_that_fails_its_check_is_traced() {
    use super::units::{DEVICE, IOMMU_ROLE, attached, three};
    let mut m = three();
    attached(&mut m, 0);
    let root = m.sys.canonical().attach_word(DEVICE as u32).root();
    let holder = m.sys.role(IOMMU_ROLE).holder();
    let other = (2, m.sys.canonical().generation(2));
    let owner = (0, m.sys.canonical().generation(0));
    let node = m.sys.node_mut(holder);
    let before = node.refused.0;
    assert!(!node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root, other));
    assert_eq!(node.refused.0, before + 1);
    assert!(node.rebuilt_attach(IOMMU_ROLE, DEVICE as u16, root, owner));
    assert_eq!(node.refused.0, before + 1);
}

// Finding 7 (p22-23). A next sequence of 2^63 is cleared even when the
// ring agrees with it, so the bound is the only reason (Debugging, D-141).
#[test]
fn a_debug_area_at_2_to_the_63_that_agrees_with_its_ring_is_cleared() {
    let m = super::pass22::restart_over(|area| {
        let next = 1u64 << 63;
        area.next_user = next;
        let at = dodos_abi::trace_index(next - 1, dodos_abi::TRACE_USER_ENTRIES);
        area.user[at as usize].seq = next - 1;
    });
    assert!(m.sys.canonical().crash.areas[1].next_user < 1 << 63);
}

// Finding 21 (own-11). A resume consumes what waited in its rings at most
// one message per unit; with more entries than one step's units it goes on
// at its next step (`node_start` mode resume, H8).
#[test]
fn a_resume_with_more_waiting_entries_than_a_step_takes_them_in_several_steps() {
    use super::pass17::{resume, seal};
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    m.ok(1, seal(0));
    m.run(20 * MS);
    assert_eq!(m.state(0), MemberState::Sealed);
    let header = dodos_abi::RingHeader {
        kind: NodeMessage::Send as u8,
        length: 0,
        sender: 1,
        sender_generation: m.sys.canonical().generation(1),
        receiver_generation: m.sys.canonical().generation(0),
        credit: u32::from(dodos_abi::CREDIT_NONE),
    };
    for _ in 0..60 {
        m.sys.inject_raw(1, 0, true, header, 1);
    }
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

// Finding 25 (own-19). An iospace pinned by a failed unit loses its root
// and its tables to its untyped: `lost` counts both (D-141).
#[test]
fn a_pinned_iospace_loses_its_root_and_its_tables() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, three};
    let mut m = three();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    // Its root and its tables are pieces of a child table untyped (D-154).
    let tables = super::table_slot(m.sys.node(0), m.t[0]);
    m.ok(0, retype_from(tables, ObjectKind::Untyped, 1 << 20, 160));
    m.ok(
        0,
        retype_from(ut, ObjectKind::IoSpace, s(160).raw(), IOSPACE),
    );
    m.ok(0, retype_from(ut, ObjectKind::Frame, 1, 161));
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    let map = Syscall::IoSpaceMap(Map {
        space: s(IOSPACE),
        frame: s(161),
        untyped: s(160),
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    });
    if m.call(0, map) == WAITS {
        m.run(30 * MS);
        assert_eq!(m.woken(0), Some(&DONE));
    }
    let node = m.sys.node(0);
    let iospace = node.object_at(m.t[0], s(IOSPACE)).expect("an iospace");
    let tables = node.aspaces[&iospace].tables;
    assert!(tables > 0);
    let untyped = node.object_at(m.t[0], s(160)).expect("an untyped");
    let lost = node.untypeds[&untyped].lost;
    m.sys.break_unit(IOMMU_ROLE);
    let mut outcome = Err(crate::Error::Again);
    for _ in 0..8 {
        outcome = m.call(0, Syscall::CapDelete(s(IOSPACE)));
        if outcome != Err(crate::Error::Again) {
            break;
        }
        m.run(10 * MS);
    }
    assert_eq!(outcome, DONE);
    let after = m.sys.node(0).untypeds[&untyped].lost;
    assert_eq!(u64::from(after - lost), 1 + tables);
}

// Finding 22 (own-14). A device entry whose owner is killed keeps its
// count frozen until R6, while the owner's kernel counts nothing: the
// check judges it "at least" its leaves, not exactly (G5; ver2-D 5).
#[test]
fn a_device_count_frozen_by_a_killed_owner_is_judged_at_least() {
    let devices = S::two().sys.canonical().devices.clone();
    let index = devices
        .iter()
        .position(|e| e.is_used())
        .expect("a device entry");
    let mut m = super::pass22::frozen_device_leaf(index as u64);
    let kill = Syscall::NodeStop {
        node: node_cap(0),
        mode: dodos_abi::NODE_STOP_KILL,
        core: 0,
    };
    m.ok(2, kill);
    // Checked every 100 µs while node 0 is fenced, dead and reclaimed.
    m.run(30 * MS);
    assert_ne!(m.state(0), MemberState::Live);
}

// Finding 9 (p22-47). Where the units cache absent entries, a map of an
// iospace that serves devices asks the holder; when the unit request table
// is full its last state check is `again` with nothing changed (D-141).
// The table is filled by calls: 64 threads, each waiting on such a map.
#[test]
fn an_iospace_map_that_must_ask_a_full_unit_request_table_is_again_with_nothing_changed() {
    let (mut m, tables, iospace) = super::pass22::caching_iospace();
    let ut = untyped_slot(m.sys.node(0), m.t[0]);
    m.ok(0, place_first(0, 0));
    let map = |frame: u32, address: u64| {
        Syscall::IoSpaceMap(Map {
            space: s(iospace),
            frame: s(frame),
            untyped: s(tables),
            address,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        })
    };
    for i in 0..dodos_abi::UNIT_REQUESTS as u32 {
        let slot = 1000 + 4 * i;
        let mut calls = thread_calls(ut, 0, slot, 0);
        if let Syscall::SchedConfigure { budget, .. } = &mut calls[4] {
            *budget = dodos_abi::SCHED_MIN_BUDGET_NS;
        }
        for call in calls {
            m.ok(0, call);
        }
        m.ok(0, retype_from(ut, ObjectKind::Frame, 1, slot + 3));
        let thread = m.sys.node(0).object_at(m.t[0], s(slot)).expect("a thread");
        let address = 0x1000_0000 + (u64::from(i) + 1) * 0x10_0000;
        assert_eq!(m.call_by(0, thread, map(slot + 3, address)), WAITS, "{i}");
    }
    assert!(m.sys.node(0).unit_requests.iter().all(Option::is_some));
    assert_eq!(m.call(0, map(160, 0x1000_0000)), Err(crate::Error::Again));
    let frame = m.sys.node(0).object_at(m.t[0], s(160)).expect("the frame");
    let root = m.sys.node(0).objects[&frame].root;
    assert!(!matches!(
        m.sys.node(0).caps[&root].record,
        crate::Record::Mapping(_)
    ));
}
