//! Pass 32: the defects of the model of several nodes that the full audit
//! found and that an independent re-judge confirmed (lot 5). Each scenario
//! is made of calls and ring messages a node can make, a faulty one where
//! the scenario says so, and fails without the rule it names.

use super::pass17::kill;
use super::pass22::take;
use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
use super::*;
use crate::rings::Body;
use crate::{Message, Range, Transfer};
use dodos_abi::{
    CoreWord, FENCE_NS, MEMORY_TAKE_RUN, MEMORY_TAKE_TABLES, NODE_STOP_KILL, NODE_STOP_RETIRE,
    NODE_STOP_SEAL, NodeMessage, Owner, Ownership, PageClass, RingHeader, RoleKind, RoleState,
    START_SLOT_MACHINE_LIVING, START_SLOT_MAILBOX,
};

pub(super) const REPLY: u32 = 100;
pub(super) const FRAME: u32 = 101;
const SECOND: u32 = 102;
const GOT: u32 = 110;
const BLOCK: u32 = 160;

pub(super) fn recv() -> Syscall {
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

/// A `node_send` to node `to` that grants the frame of slot `frame` with
/// `rights`.
pub(super) fn send(to: u16, frame: u32, rights: Rights) -> Syscall {
    Syscall::NodeSend {
        node: node_cap(to),
        message: Message {
            label: 7,
            words: vec![7],
            caps: vec![Transfer::Derive(s(frame), rights.bits())],
            ..Message::default()
        },
        doorbell: SlotAddr::NONE,
    }
}

/// A message of node 1's generation 1 to a node of generation 1.
pub(super) fn from_node_1(
    kind: NodeMessage,
    credit: Option<(u8, u32)>,
    body: Body,
) -> crate::rings::Message {
    crate::rings::Message {
        header: RingHeader::new(kind, body.entries() as u8, (1, 1), 1, credit),
        body,
    }
}

/// Fires the events of the nodes in `only`, the others holding still,
/// until `until` holds.
pub(super) fn drive(m: &mut S, only: &[u16], until: impl Fn(&S) -> bool) -> bool {
    use crate::system::Event;
    for _ in 0..20_000 {
        if until(m) {
            return true;
        }
        let mine = m.sys.events().into_iter().find(|event| match *event {
            Event::Ipi { node, .. } | Event::Timer { node, .. } => only.contains(&node),
            Event::Action(i) => only.contains(&m.sys.actions()[i].0),
            _ => true,
        });
        match mine {
            Some(event) => m.sys.fire(event),
            None => {
                let at = m.sys.now() + 10_000;
                m.sys.advance(at);
            }
        }
    }
    until(m)
}

/// One event, the earliest; or, with none left at this instant, the whole
/// check, then the clock moved to the next timer (`to` at most), whose
/// events can then happen. False once nothing is left to do.
pub(super) fn pump(m: &mut S, to: u64) -> bool {
    if let Some(&event) = m.sys.events().first() {
        m.sys.fire(event);
        return true;
    }
    assert_eq!(m.sys.check(), Ok(()));
    let timers = m.sys.models().filter_map(|(_, n)| n.next_timer());
    let Some(next) = timers.map(|(at, _)| at).min() else {
        return false;
    };
    m.sys.move_clock(next.max(m.sys.now() + 1).min(to));
    true
}

/// The reserve role of index `j` claimed by node `node`.
fn claim_reserve(m: &mut S, node: u16, j: u16) {
    let table = m.sys.canonical();
    let role = table
        .role_records
        .iter()
        .position(|r| r.kind == RoleKind::Reserve && r.object == j)
        .expect("the reserve role");
    let slot = super::units::role_slot(m, node, role);
    let claimed = m.call(node, Syscall::RoleClaim(slot));
    assert!(matches!(claimed, Ok(Output::Epoch(_))), "{claimed:?}");
}

/// The first chunk whose ownership word reads *free* with `owner`.
fn free_chunk(m: &S, owner: Owner) -> u64 {
    let free = Ownership::free(owner).raw();
    let owners = &m.sys.canonical().owners;
    owners.iter().position(|&w| w == free).expect("a chunk") as u64
}

/// Node 0 holds reserve role 0 and takes a chunk of that reserve as a
/// block of `kind` in `BLOCK`; the chunk.
fn reserve_block(m: &mut S, kind: u64) -> u64 {
    let shift = u64::from(m.sys.canonical().geometry.chunk_shift());
    let chunk = free_chunk(m, Owner::Reserve(0));
    claim_reserve(m, 0, 0);
    m.ok(0, take(kind, chunk << shift, shift, BLOCK));
    chunk
}

// Lot 5, F-01. A `SendAnswer` acts on the grants its send made, each the
// entry of that serial, written *answered* once per serial by a
// compare-and-swap from unanswered (G6; "it cannot act on another node's
// grants", the one check). Faulty node 1 releases a grant before it
// answers the send; node 0 grants the freed entry anew to node 2, which
// maps the frame; node 1 then answers its send "not delivered". The
// answer names an entry whose serial is over: node 2's grant stays.
#[test]
fn a_send_answer_acts_only_on_the_grants_its_send_made() {
    let mut m = three();
    for frame in [FRAME, SECOND] {
        let retype = m.retype(0, ObjectKind::Frame, 1, frame);
        m.ok(0, retype);
    }
    m.ok(0, super::place_first(0, 0));
    let second = m.thread_on(0, 300, 0);
    let reply = m.retype(2, ObjectKind::Reply, 0, REPLY);
    m.ok(2, reply);
    assert_eq!(m.call(2, recv()), WAITS);
    let rights = Rights::READ.union(Rights::WRITE);

    // Node 0 grants a frame to node 1, whose kernel does not answer.
    assert_eq!(m.call(0, send(1, FRAME, rights)), WAITS);
    let (grant, serial) = {
        let entries = m.sys.node(0).grant_entries();
        assert_eq!(entries.len(), 1);
        (entries[0].0, entries[0].1.2)
    };
    let (credit, credit_serial) = m.sys.node(0).peers[1]
        .credits
        .iter()
        .enumerate()
        .find_map(|(i, c)| c.as_ref().map(|c| (i as u8, c.serial)))
        .expect("the send's credit");
    // Node 1 releases the grant before any answer: the entry is freed (no
    // use entry counts a leaf under an entry not answered).
    let release = Body::Release { grant, serial };
    m.sys
        .inject(1, 0, from_node_1(NodeMessage::Release, None, release));
    assert!(drive(&mut m, &[0], |m| m
        .sys
        .node(0)
        .grant_entries()
        .is_empty()));
    // Node 0 grants another frame to node 2: the same entry, the next
    // serial. Node 2 answers and maps it.
    assert_eq!(m.call_by(0, second, send(2, SECOND, rights)), WAITS);
    let answered = |m: &S| {
        let entry = m.sys.canonical().grants[0].get(&grant);
        entry.is_some_and(|g| g.answered)
    };
    assert!(drive(&mut m, &[0, 2], answered));
    let entry = m.sys.canonical().grants[0][&grant];
    assert_eq!((entry.grantee, entry.serial), (2, serial + 1));
    m.ok(
        2,
        Syscall::ASpaceMap(crate::Map {
            space: s(dodos_abi::START_SLOT_ASPACE),
            frame: s(GOT),
            untyped: SlotAddr::NONE,
            address: 1 << 30,
            first: 0,
            count: 1,
            rights: Rights::READ.bits(),
        }),
    );
    // Node 1 now answers its send, "not delivered".
    let answer = Body::SendAnswer {
        delivered: false,
        dropped: false,
        uses: vec![None],
    };
    let answer = from_node_1(
        NodeMessage::SendAnswer,
        Some((credit, credit_serial)),
        answer,
    );
    m.sys.inject(1, 0, answer);
    let freed = |m: &S| m.sys.node(0).peers[1].credits[credit as usize].is_none();
    assert!(drive(&mut m, &[0], freed), "the answer frees its credit");
    assert_eq!(m.sys.check(), Ok(()));
    let after = m.sys.canonical().grants[0].get(&grant).copied();
    assert_eq!(after, Some(entry), "node 2's grant, with its leaf");
}

/// The index and serial of the credit node 0 holds towards node 1 that is
/// not `except`.
fn credit_to_1(m: &S, except: Option<u8>) -> (u8, u32) {
    m.sys.node(0).peers[1]
        .credits
        .iter()
        .enumerate()
        .filter(|&(i, _)| Some(i as u8) != except)
        .find_map(|(i, c)| c.as_ref().map(|c| (i as u8, c.serial)))
        .expect("the send's credit")
}

/// A `SendAnswer` of node 1, "not delivered", on `credit`.
fn not_delivered(credit: (u8, u32)) -> crate::rings::Message {
    let answer = Body::SendAnswer {
        delivered: false,
        dropped: false,
        uses: vec![None],
    };
    from_node_1(NodeMessage::SendAnswer, Some(credit), answer)
}

// Pass 34, R-3 of the review of pass 32 (Grants, release by the holder,
// D-159 (4); G6: the answer is written once per serial). Node 1 releases a
// grant its send made before it answers; node 0 grants the same entry anew
// to node 1 itself, by a second send: the next serial, the same grantee.
// The first send's late "not delivered" answer then names a serial no
// entry holds: only the serial tells the two grants apart, and the second
// stays, unanswered. Its own answer is then applied.
#[test]
fn a_late_answer_leaves_the_entry_granted_anew_to_the_same_node() {
    let mut m = three();
    for frame in [FRAME, SECOND] {
        let retype = m.retype(0, ObjectKind::Frame, 1, frame);
        m.ok(0, retype);
    }
    m.ok(0, super::place_first(0, 0));
    let second = m.thread_on(0, 300, 0);
    let rights = Rights::READ.union(Rights::WRITE);
    assert_eq!(m.call(0, send(1, FRAME, rights)), WAITS);
    let (grant, serial) = {
        let entries = m.sys.node(0).grant_entries();
        assert_eq!(entries.len(), 1);
        (entries[0].0, entries[0].1.2)
    };
    let first = credit_to_1(&m, None);
    let release = Body::Release { grant, serial };
    m.sys
        .inject(1, 0, from_node_1(NodeMessage::Release, None, release));
    assert!(drive(&mut m, &[0], |m| m
        .sys
        .node(0)
        .grant_entries()
        .is_empty()));
    assert_eq!(m.call_by(0, second, send(1, SECOND, rights)), WAITS);
    let entry = m.sys.canonical().grants[0][&grant];
    assert_eq!((entry.grantee, entry.serial), (1, serial + 1));
    let again = credit_to_1(&m, Some(first.0));
    m.sys.inject(1, 0, not_delivered(first));
    let freed = |m: &S| m.sys.node(0).peers[1].credits[first.0 as usize].is_none();
    assert!(
        drive(&mut m, &[0], freed),
        "the late answer frees its credit"
    );
    assert_eq!(m.sys.check(), Ok(()));
    let after = m.sys.canonical().grants[0].get(&grant).copied();
    assert_eq!(after, Some(entry), "the late answer freed the new grant");
    m.sys.inject(1, 0, not_delivered(again));
    let gone = |m: &S| !m.sys.canonical().grants[0].contains_key(&grant);
    assert!(drive(&mut m, &[0], gone), "the grant's own answer frees it");
    assert_eq!(m.sys.check(), Ok(()));
}

// Lot 5, F-02. A chunk of a reserve taken by the holder of its role keeps
// the reserve's owner code with the holder's generation (rule 4 of
// taking), and reads *taken* by that node wherever an ownership word is
// judged: R4's audit of the node's live blocks passes over it, and the
// machine does not stop at the node's next recovery.
#[test]
fn a_reserve_chunk_taken_by_its_roles_holder_passes_the_audit_of_r4() {
    let mut m = three();
    let chunk = reserve_block(&mut m, MEMORY_TAKE_RUN);
    let word = Ownership::from_raw(m.sys.canonical().owners[chunk as usize]).expect("a word");
    assert_eq!(
        (word.owner(), word.generation()),
        (Owner::Reserve(0), m.sys.member(0).generation())
    );
    m.ok(0, kill(2));
    m.run(60 * MS);
    assert!(!m.sys.stopped, "{:?}", m.sys.canonical().crash.stop);
    assert_eq!(m.state(2), MemberState::Dead);
}

// Lot 5, F-02. The holder of a unit's role serves the attach of an
// iospace whose root lies in a table chunk of a reserve the requester
// holds: the root is in memory the ownership table gives to the requester
// with its generation.
#[test]
fn an_iospace_whose_root_lies_in_a_reserve_table_chunk_is_attached() {
    let mut m = three();
    reserve_block(&mut m, MEMORY_TAKE_TABLES);
    let untyped = untyped_slot(m.sys.node(0), m.t[0]);
    let iospace = retype_from(untyped, ObjectKind::IoSpace, s(BLOCK).raw(), IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, DEVICE);
    assert_eq!(m.call(0, call), WAITS);
    m.run(30 * MS);
    assert_eq!(m.woken(0), Some(&DONE));
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(entries.contains_key(&(DEVICE as u16)));
}

// Lot 5, F-02. A faulty kernel's own gate calls judge a reserve chunk as
// the kernel's calls do: a page of a reserve table chunk the node took
// leaves *data*, and one of a reserve's ordinary chunk takes a leaf.
#[test]
fn the_gate_reads_a_reserve_chunk_its_node_took_as_the_nodes() {
    let page = |m: &S, chunk: u64| {
        let geometry = m.sys.canonical().geometry;
        (chunk << geometry.chunk_shift()) / geometry.page()
    };
    let mut m = three();
    let chunk = reserve_block(&mut m, MEMORY_TAKE_TABLES);
    let table = page(&m, chunk) + 1;
    let class = PageClass::table(1).expect("a level");
    assert!(m.sys.node_mut(0).gate_retype_page(table, class));
    let mut m = three();
    let chunk = reserve_block(&mut m, MEMORY_TAKE_RUN);
    assert!(m.sys.node(0).gate_leaf_allowed(page(&m, chunk)));
    assert!(!m.sys.node(1).gate_leaf_allowed(page(&m, chunk)));
}

// Lot 5, F-03. A seal does not hand over a unit its node is rebuilding:
// the hand-over waits for the rebuild, as `role_hand` is `busy` for it.
// Node 2 took over the unit of node 1, killed, and is asking the other
// nodes for their reports when it is asked to seal: the rebuild ends
// first (its closing invalidation, its credits freed), then the role goes
// to the successor.
#[test]
fn a_seal_hands_no_unit_over_while_its_node_rebuilds_it() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    attached(&mut m, 0);
    m.supervisor(2, m.t[2]);
    assert_eq!(m.sys.apply(2, m.t[2], kill(1)), DONE);
    let to = m.sys.now() + 400 * MS;
    let asking = |m: &S| {
        let rebuild = m
            .sys
            .has(2)
            .then(|| m.sys.node(2).rebuilds.get(&IOMMU_ROLE));
        rebuild.flatten().is_some_and(|r| r.node > 0)
    };
    while !asking(&m) {
        assert!(m.sys.now() < to && pump(&mut m, to), "no rebuild");
    }
    let seal = Syscall::NodeStop {
        node: node_cap(2),
        mode: NODE_STOP_SEAL,
        core: 0,
    };
    assert_eq!(m.sys.apply(0, m.t[0], seal), DONE);
    let mut closed = false;
    let end = m.sys.now() + 300 * MS;
    while m.sys.now() < end && pump(&mut m, end) {
        let node = m.sys.node(2);
        let rebuild = node.rebuilds.get(&IOMMU_ROLE);
        assert!(
            rebuild.is_none() || !node.handing.contains_key(&IOMMU_ROLE),
            "a unit handed over while it is rebuilt"
        );
        closed |= rebuild.is_some_and(|r| r.closing.is_some());
    }
    assert!(closed, "the rebuild ended without its closing invalidation");
    assert!(m.sys.node(2).rebuilds.is_empty());
    assert_eq!(m.state(2), MemberState::Sealed);
    let word = m.sys.role(IOMMU_ROLE);
    assert_eq!((word.state(), word.holder()), (RoleState::Held, 3));
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(entries.contains_key(&(DEVICE as u16)));
}

// Lot 5, F-04. "A node that seals during the rebuild [...] is then
// treated as sealed" (Roles, the rebuild): one that sealed after its
// `ReportAsk` was sent and before it answered is passed over as sealed,
// so its entries are kept even if it resumes before the rebuild ends. The
// member word is moved as pass 22's scenario of a node passed over sealed
// moves it.
#[test]
fn a_node_sealed_after_its_report_was_asked_is_passed_over_as_sealed() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    attached(&mut m, 0);
    let member = |state| {
        move |t: &mut crate::table::Table| {
            let w = t.member(0);
            t.members[0] = dodos_abi::Member::new(state, w.evidence(), w.driver(), w.generation())
                .with_class(w.class());
        }
    };
    m.supervisor(2, m.t[2]);
    assert_eq!(m.sys.apply(2, m.t[2], kill(1)), DONE);
    let to = m.sys.now() + 400 * MS;
    let cursor = |m: &S| {
        let rebuild = m
            .sys
            .has(2)
            .then(|| m.sys.node(2).rebuilds.get(&IOMMU_ROLE));
        rebuild.flatten().map(|r| (r.node, r.asked.is_some()))
    };
    // The rebuild has sent its ask to node 0, which seals before it
    // consumes it.
    while cursor(&m) != Some((0, true)) {
        assert!(m.sys.now() < to && pump(&mut m, to), "no ask to node 0");
    }
    m.sys.edit_table(member(MemberState::Sealed));
    // Node 0 resumes once the rebuild has passed it.
    while cursor(&m).is_some_and(|(node, _)| node == 0) {
        assert!(m.sys.now() < to && pump(&mut m, to));
    }
    m.sys.edit_table(member(MemberState::Live));
    while !m.sys.node(2).rebuilds.is_empty() {
        assert!(
            m.sys.now() < to && pump(&mut m, to),
            "the rebuild never ends"
        );
    }
    let entries = &m.sys.canonical().units[&IOMMU_ROLE].entries;
    assert!(
        entries.contains_key(&(DEVICE as u16)),
        "the rebuild cleared the context entry of a node that sealed under it"
    );
}

// Lot 5, F-05. The write of unfenced sets the flag unusable on every core
// of the node not confirmed, on every architecture (The fence; D-151):
// an x86-64 core in VMX root operation that hangs takes no INIT.
#[test]
fn an_unconfirmed_core_is_marked_unusable_at_unfenced_on_x86_64() {
    let mut m = S::two();
    claim_reserve(&mut m, 0, 0);
    m.sys.hang(1, false);
    m.sys.set_vmx(1, true);
    m.ok(0, kill(1));
    m.run(FENCE_NS + MS);
    assert_eq!(m.state(1), MemberState::Unfenced);
    let word = CoreWord::from_raw(m.sys.canonical().core_words[1]).expect("a core word");
    assert!(word.is_unusable());
}

// Lot 5, F-10. The kernel ring carries the kernels' own kinds and the
// user ring kinds 11 and 12 (Two rings per ordered pair of nodes): a
// `Release`, well formed, that a faulty node writes on its user ring is
// a kind that ring does not know, and is refused.
#[test]
fn a_kernel_message_on_the_user_ring_is_refused() {
    let mut m = S::two();
    let reply = m.retype(1, ObjectKind::Reply, 0, REPLY);
    m.ok(1, reply);
    let frame = m.retype(0, ObjectKind::Frame, 1, FRAME);
    m.ok(0, frame);
    assert_eq!(m.call(1, recv()), WAITS);
    assert_eq!(m.call(0, send(1, FRAME, Rights::READ)), WAITS);
    m.settle();
    let entries = m.sys.node(0).grant_entries();
    let (grant, serial) = (entries[0].0, entries[0].1.2);
    let release = Body::Release { grant, serial };
    let message = from_node_1(NodeMessage::Release, None, release);
    let length = message.body.entries();
    let refused = m.sys.node(0).refused.0;
    m.sys.edit_table(|t| {
        let pair = &mut std::sync::Arc::make_mut(&mut t.rings)[1][0];
        let at = pair.user.produced;
        pair.user.set(at, crate::table::RingEntry::Message(message));
        pair.user.produced += length;
    });
    m.sys.interrupt(0, crate::Ipi::Ring);
    m.run(20 * MS);
    assert_eq!(m.sys.node(0).grant_entries().len(), 1);
    assert_eq!(m.sys.node(0).refused.0, refused + 1);
}

// Lot 5, F-12. An anchor over the global read-only set raises no count,
// and no gate word of a chunk that no node owns is ever written after
// machine start-up (G4, D-151): the grant of a kept frame, which every
// node may map read-only anyway, writes no gate word.
#[test]
fn a_grant_of_a_kept_frame_raises_no_count() {
    let mut m = S::two();
    let reply = m.retype(1, ObjectKind::Reply, 0, REPLY);
    m.ok(1, reply);
    assert_eq!(m.call(1, recv()), WAITS);
    let before = m.sys.canonical().gate_words.clone();
    let kept = send(1, START_SLOT_MACHINE_LIVING, Rights::READ);
    assert_eq!(m.call(0, kept), WAITS);
    assert_eq!(m.sys.node(0).grant_entries().len(), 1);
    assert_eq!(m.sys.canonical().gate_words, before);
    m.settle();
    assert!(matches!(m.woken(0), Some(Ok(Output::Sent { .. }))));
}

// Lot 5, F-13. `node_stop` mode 3 is `unsupported`, judged first among
// the plain arguments (D-152).
#[test]
fn node_stop_mode_3_is_unsupported() {
    let mut m = S::two();
    let stop = |core| Syscall::NodeStop {
        node: node_cap(1),
        mode: 3,
        core,
    };
    m.no(0, stop(0), Error::Unsupported);
    m.no(0, stop(7), Error::Unsupported);
    let beyond = Syscall::NodeStop {
        node: node_cap(1),
        mode: 4,
        core: 0,
    };
    m.no(0, beyond, Error::Invalid);
}

// Lot 5, F-13. Modes 0 and 2 are `busy` unless the caller's node holds a
// reserve role: the evidence kill and retire need no proof, and the gate
// accepts them only from such a node (D-150). A refused retire marks no
// core.
#[test]
fn a_kill_and_a_retire_need_a_reserve_role() {
    let mut m = S::two();
    let retire = Syscall::NodeStop {
        node: node_cap(1),
        mode: NODE_STOP_RETIRE,
        core: 1,
    };
    let stop = |mode| Syscall::NodeStop {
        node: node_cap(1),
        mode,
        core: 0,
    };
    // Applied to the system itself: the harness's own calls claim a
    // reserve role before such a stop.
    let busy = Err(Error::Busy);
    assert_eq!(m.sys.apply(0, m.t[0], stop(NODE_STOP_KILL)), busy);
    assert_eq!(m.sys.apply(0, m.t[0], retire.clone()), busy);
    assert_eq!(m.sys.check(), Ok(()));
    let word = CoreWord::from_raw(m.sys.canonical().core_words[1]).expect("a core word");
    assert!(!word.is_unusable());
    assert_eq!(m.state(1), MemberState::Live);
    // A seal asks for no reserve role.
    m.ok(0, stop(NODE_STOP_SEAL));
    let mut m = S::two();
    claim_reserve(&mut m, 0, 1);
    m.ok(0, retire);
    assert_eq!(m.state(1), MemberState::Fencing);
}

// Pass 34, R-4 of the review of pass 32. The gate of a faulty kernel reads
// a reserve chunk its node took while it holds the role as its node's, as
// the honest `memory_give` does (rule 4 of taking; the ownership row
// *taken* (X, g) → *free*: X's, G5's counts all 0): it gives it.
#[test]
fn the_gate_gives_a_reserve_chunk_its_node_took() {
    let mut m = three();
    let chunk = reserve_block(&mut m, MEMORY_TAKE_RUN);
    assert!(m.sys.node_mut(0).gate_give_chunk(chunk, 1));
    let word = Ownership::from_raw(m.sys.canonical().owners[chunk as usize]).expect("a word");
    assert_eq!(word, Ownership::free(Owner::Node(1)));
}
