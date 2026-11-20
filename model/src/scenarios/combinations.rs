//! A seeded exploration of the combined recovery states between nodes
//! (pass 25): the rare events of the contract between nodes, steered so
//! that they are in progress together. Five nodes of one core; two grants
//! (node 0 to node 2, mapped in an iospace behind the IOMMU unit, and
//! node 3 to node 4, mapped in an address space), each with a doorbell;
//! a device attached by nodes 2 and 4; a line that nodes 1 and 3 claim in
//! turn; a chunk that nodes 1 and 3 give each other and a device entry
//! node 1 splits; the IOMMU role handed between nodes; and every node
//! accused by each kind of evidence, fenced or left unfenced, sealed and
//! resumed, started again. The whole check runs after every step, as in
//! `explore.rs`.
//!
//! After every step the exploration notes which events are in progress
//! (`probe`), and counts the seeds that reach each pair of them, each
//! triple of a resume and two others, and each triple of a death and two
//! others (`Reach`). In the long run (`#[ignore]`, `--release`), a
//! combination no seed reaches fails the test, unless `IMPOSSIBLE` shows
//! from the contract that it cannot happen; the 26 seeds of `verify` reach
//! about half of them, and fail if they miss more than when measured
//! (`MISSED_BY_26_SEEDS`). Then the
//! machine runs until it is quiet, and its liveness is asserted: no node is
//! accused that was not harmed (killed, failed, hung, retired or sealed past
//! `SEAL_NS`), and no wait of a live node for another node, a unit or its
//! cores is left.
//!
//! `DODOS_SEED=<n>` replays one seed with its actions printed;
//! `DODOS_COUNTS` prints the table of combinations.

use super::explore::{refresh, replayed, say, try_call};
use super::pass17::{fresh, kill, resume, seal};
use super::units::{IOMMU_ROLE, IOSPACE};
use super::*;
use crate::rings::{Body, UnitOp};
use crate::system::Hardware;
use crate::table::{GrantState, RingEntry, Table};
use crate::{Message, Range, Transfer};
use dodos_abi::{
    AttachState, AttachWord, Evidence, IrqSource, Member, NODE_STOP_RETIRE, OwnState, Owner,
    Ownership, RoleState,
};
use std::collections::BTreeSet;

const NODES: u16 = 5;
const REPLY: u32 = 100;
const BELL: u32 = 102;
const BLOCK: u32 = 160;
const ENTRY: u32 = 170;
const IRQ: u32 = 210;
const RECEIVER: u32 = 300;
/// The slots of the table chunks' cycle (`Run::tables`): a table run, a
/// child table untyped cut from it, an `aspace` and an `iospace` on that
/// child, and a frame to map under them.
const TBLOCK: u32 = 400;
const TCHILD: u32 = 401;
const TASPACE: u32 = 402;
const TIOSPACE: u32 = 403;
const TFRAME: u32 = 404;
const STEPS: usize = 120;

/// A grant between two nodes: the granting node and its frames from
/// `obj`, the holder and the slots it receives them in from `got`, and
/// whether the holder maps the first in its iospace (else in its address
/// space).
#[derive(Clone, Copy)]
struct Pair {
    granter: u16,
    holder: u16,
    io: bool,
    obj: u32,
    got: u32,
    frames: u32,
}

/// Node 0 grants node 2 one frame, mapped behind the IOMMU unit; node 3
/// grants node 4 32 frames, so that their releases at once fill the
/// kernel ring to node 3 (64 entries, 34 kept for answers).
const PAIRS: [Pair; 2] = [
    Pair {
        granter: 0,
        holder: 2,
        io: true,
        obj: 101,
        got: 110,
        frames: 1,
    },
    Pair {
        granter: 3,
        holder: 4,
        io: false,
        obj: 400,
        got: 500,
        frames: 32,
    },
];
/// The nodes that attach a device, each its own.
const DEVICES: [(u16, u64); 2] = [(2, 0x10), (4, 0x11)];
/// The nodes that claim the line in turn, and its GSI.
const LINERS: [u16; 2] = [1, 3];
const GSI: u32 = 5;

/// The events, each a state in progress somewhere on the machine.
const KILLED: usize = 0;
const SELF: usize = 1;
const STALE: usize = 2;
const STUCK: usize = 3;
const RETIRED: usize = 4;
const SEAL_LATE: usize = 5;
const FENCED: usize = 6;
const UNFENCED: usize = 7;
const SEALED: usize = 8;
const RESUME: usize = 9;
const FRESH: usize = 10;
const FAILED_UNIT: usize = 11;
const HANDING: usize = 12;
const HAND_DEATH: usize = 13;
const UNANSWERED: usize = 14;
const REVOKING: usize = 15;
const RELEASE: usize = 16;
const CROSSING: usize = 17;
const RELEASE_PENDING: usize = 18;
const SIGNAL_PENDING: usize = 19;
const GIVEN: usize = 20;
const SPLIT: usize = 21;
const ATTACHED: usize = 22;
const DETACHED: usize = 23;
const MID_ATTACH: usize = 24;
const LINE: usize = 25;
/// A core waking from a sleep that lost its context (pass 28).
const WAKE: usize = 26;
/// A node starting afresh whose first thread runs before it is live
/// (pass 28).
const PRE_LIVE: usize = 27;
const EVENTS: usize = 28;
/// The node of two cores, whose start can leave its second core out for a
/// while: the window before live.
const WIDE: u16 = 4;
/// The architectures, one per round of the 286 triples of drivers.
const ARCHES: [Arch; 3] = [Arch::X86_64, Arch::Aarch64, Arch::Riscv64];

const NAMES: [&str; EVENTS] = [
    "accused kill",
    "accused self",
    "accused stale",
    "accused stuck",
    "accused retire",
    "accused seal",
    "fenced",
    "unfenced",
    "sealed",
    "resume",
    "fresh start",
    "failed unit",
    "role handing",
    "holder's death mid-hand",
    "grant unanswered",
    "revoking",
    "release",
    "release crossing revoke",
    "release pending",
    "signal pending",
    "chunk given",
    "entry split",
    "device attached",
    "device detached",
    "mid-attach",
    "line moving",
    "wake from sleep",
    "first thread before live",
];

/// The accusations: a node fencing, fenced or unfenced.
fn accused(state: MemberState) -> bool {
    matches!(
        state,
        MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced
    )
}

/// The messages of the kernel ring from `from` to `to` not yet consumed.
fn waiting(table: &Table, from: u16, to: u16) -> impl Iterator<Item = &Body> {
    let ring = &table.rings[from as usize][to as usize].kernel;
    let start = table.beacons[to as usize].consumed[from as usize].0;
    (start..ring.produced).filter_map(|at| match ring.at(at) {
        Some(RingEntry::Message(message)) => Some(&message.body),
        _ => None,
    })
}

/// A `Release` of grant `grant` of `granter` from its holder, in flight or
/// pending in the holder's use entry.
fn releasing(table: &Table, granter: u16, holder: u16, grant: u16) -> bool {
    let sent = waiting(table, holder, granter)
        .any(|b| matches!(b, Body::Release { grant: g, .. } if *g == grant));
    let pending = table.uses[holder as usize]
        .values()
        .any(|u| u.granter == granter && u.grant == grant && u.release_pending);
    sent || pending
}

/// The events in progress now, as a mask.
fn probe(m: &S, splittable: usize) -> u32 {
    let mut at = 0u32;
    let mut set = |event: usize, holds: bool| {
        if holds {
            at |= 1 << event;
        }
    };
    let table = m.sys.canonical();
    for n in 0..NODES {
        let member = m.sys.member(n);
        let state = member.state();
        if accused(state) {
            let kind = match member.evidence() {
                Evidence::Kill => KILLED,
                Evidence::SelfReported => SELF,
                Evidence::Stale => STALE,
                Evidence::Stuck => STUCK,
                Evidence::Retire => RETIRED,
                Evidence::Seal => SEAL_LATE,
                Evidence::None => continue,
            };
            set(kind, true);
        }
        set(FENCED, state == MemberState::Fenced);
        set(UNFENCED, state == MemberState::Unfenced);
        set(
            SEALED,
            matches!(state, MemberState::Sealing | MemberState::Sealed),
        );
        // A resume from the write of starting with the resume bit to live
        // (D-144), a fresh start from its write of starting.
        if state == MemberState::Starting {
            set(RESUME, member.resuming());
            set(FRESH, !member.resuming());
            // Its first thread runs before it is live (pass 28).
            set(
                PRE_LIVE,
                !member.resuming()
                    && m.sys.has(n)
                    && m.sys.node(n).generation_built() == member.generation()
                    && m.sys.node(n).is_ready(),
            );
        }
        if m.sys.has(n) {
            let node = m.sys.node(n);
            for entry in node.unit_requests.iter().flatten() {
                set(MID_ATTACH, matches!(entry.op, UnitOp::Attach { .. }));
                set(LINE, matches!(entry.op, UnitOp::Redirection { .. }));
            }
            set(LINE, node.irqs.values().any(|i| i.removing));
            set(
                DETACHED,
                node.iommus.values().any(|u| !u.detaching.is_empty()),
            );
        }
    }
    // A core on its way out of a sleep that lost its context.
    set(
        WAKE,
        (0..m.sys.core_count()).any(|core| {
            matches!(
                m.sys.hardware(core),
                Hardware::Asleep { wake: Some(_), .. } | Hardware::Suspending { roused: true, .. }
            )
        }),
    );
    let word = m.sys.role(IOMMU_ROLE);
    set(FAILED_UNIT, word.failed());
    if word.state() == RoleState::Handing {
        set(HANDING, true);
        let dying = |n: u16| accused(m.state(n)) || m.state(n) == MemberState::Dead;
        set(HAND_DEATH, dying(word.holder()) || dying(word.target()));
    }
    for granter in 0..NODES {
        for (&grant, entry) in &table.grants[granter as usize] {
            set(
                UNANSWERED,
                entry.state == GrantState::Live && !entry.answered,
            );
            if entry.state == GrantState::Revoking {
                set(REVOKING, true);
                set(CROSSING, releasing(table, granter, entry.grantee, grant));
            }
        }
        for holder in 0..NODES {
            let sent = waiting(table, holder, granter).any(|b| matches!(b, Body::Release { .. }));
            set(RELEASE, sent);
        }
    }
    for uses in &table.uses {
        for u in uses.values() {
            set(RELEASE, u.release_pending);
            set(RELEASE_PENDING, u.release_pending);
            set(SIGNAL_PENDING, u.signal_pending);
        }
    }
    let given = |&w: &u64| {
        Ownership::from_raw(w)
            .is_some_and(|o| o.state() == OwnState::Free && matches!(o.owner(), Owner::Node(_)))
    };
    set(GIVEN, table.owners.iter().any(given));
    set(
        SPLIT,
        table.devices.iter().filter(|e| e.is_used()).count() > splittable,
    );
    for &raw in table.attach.values() {
        match AttachWord::from_raw(raw).and_then(AttachWord::state) {
            Some(AttachState::Attached) => set(ATTACHED, true),
            Some(AttachState::Detached) => set(DETACHED, true),
            Some(AttachState::Pinned) => set(MID_ATTACH, true),
            None => {}
        }
    }
    at
}

/// The seeds that reach each combination, summed over the seeds.
struct Reach {
    /// The seeds that ended with a failed unit's role held by a sealing
    /// node (gap 21 of pass 26), and those that saw a failed unit's role
    /// handed (D-145).
    gap_21: usize,
    handed_failed: usize,
    /// The forged messages of a faulty node seen refused by their receiver.
    refused: usize,
    /// The seeds that reached each of `TABLE_STATES`.
    tables: [usize; TABLE_STATES.len()],
    pairs: Vec<Vec<usize>>,
    resume: Vec<Vec<usize>>,
    death: Vec<Vec<usize>>,
    events: Vec<usize>,
}

/// What one seed reached.
#[derive(Default)]
struct Seen {
    pairs: BTreeSet<(usize, usize)>,
    resume: BTreeSet<(usize, usize)>,
    death: BTreeSet<(usize, usize)>,
    events: BTreeSet<usize>,
    refused: usize,
    /// The states of D-154 to D-157 reached (`TABLE_STATES`).
    tables: BTreeSet<usize>,
}

/// The states of the table chunks' cycle and of the gate's defence of
/// them, which the long and the short runs must reach (pass 35: the audits
/// of lot 8 found the explorations never reached D-154 to D-157).
const TABLE_STATES: [&str; 11] = [
    "a table run taken",
    "a child table untyped cut",
    "an aspace on a child table untyped",
    "an iospace on a child table untyped",
    "a frame mapped under an aspace's table untyped",
    "a frame mapped under an iospace's table untyped",
    "an aspace destroyed with its mapping",
    "an iospace destroyed with its mapping",
    "a table run given or destroyed",
    "a node fenced while another holds its grants",
    "a faulty gate refused toward a table run it holds",
];

/// A forged message a faulty node wrote, to be refused by its receiver:
/// once the receiver's consumer passes it, with the same generation, its
/// count of refusals is above the one read at the write (the audit of lot
/// 8a, F6: the comments said "refused" and nothing asserted it).
struct Forged {
    from: u16,
    to: u16,
    generation: u32,
    user: bool,
    end: u64,
    refused: u64,
    action: usize,
}

/// The death of a node, around which the triples are counted: any
/// accusation. Its two others are among the events that are not an
/// accusation's evidence or outcome.
const DEATH_OTHERS: std::ops::Range<usize> = SEALED..EVENTS;
const DEATHS: [usize; 6] = [KILLED, SELF, STALE, STUCK, RETIRED, SEAL_LATE];

impl Seen {
    fn note(&mut self, mask: u32) {
        let on: Vec<usize> = (0..EVENTS).filter(|&e| mask & 1 << e != 0).collect();
        for (i, &a) in on.iter().enumerate() {
            self.events.insert(a);
            for &b in &on[i + 1..] {
                self.pairs.insert((a, b));
            }
        }
        if mask & 1 << RESUME != 0 {
            let others: Vec<usize> = on.iter().copied().filter(|&e| e != RESUME).collect();
            for (i, &a) in others.iter().enumerate() {
                for &b in &others[i + 1..] {
                    self.resume.insert((a, b));
                }
            }
        }
        if DEATHS.iter().any(|&d| mask & 1 << d != 0) {
            let others: Vec<usize> = on
                .iter()
                .copied()
                .filter(|e| DEATH_OTHERS.contains(e))
                .collect();
            for (i, &a) in others.iter().enumerate() {
                for &b in &others[i + 1..] {
                    self.death.insert((a, b));
                }
            }
        }
    }
}

impl Reach {
    fn new() -> Self {
        let square = || vec![vec![0; EVENTS]; EVENTS];
        Self {
            gap_21: 0,
            handed_failed: 0,
            refused: 0,
            tables: [0; TABLE_STATES.len()],
            pairs: square(),
            resume: square(),
            death: square(),
            events: vec![0; EVENTS],
        }
    }

    fn add(&mut self, seen: &Seen) {
        for &(a, b) in &seen.pairs {
            self.pairs[a][b] += 1;
        }
        for &(a, b) in &seen.resume {
            self.resume[a][b] += 1;
        }
        for &(a, b) in &seen.death {
            self.death[a][b] += 1;
        }
        for &e in &seen.events {
            self.events[e] += 1;
        }
        self.refused += seen.refused;
        for &state in &seen.tables {
            self.tables[state] += 1;
        }
    }
}

/// What steers a seed toward a rare state. Each seed takes three, every
/// triple of them in turn (`drivers`), beside the random actions of every
/// part of the machine.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Driver {
    Kill,
    Fail,
    Stale,
    Stuck,
    Retire,
    /// A node sealed and left past `SEAL_NS`, which nobody resumes.
    SealLate,
    /// The bootstrap core hangs where no stop reaches it.
    Unfence,
    /// A node sealed and resumed, again and again.
    Resume,
    /// A node killed and started afresh.
    Fresh,
    FailUnit,
    /// The IOMMU role handed, and its holder or target killed meanwhile.
    HandDeath,
    /// The 32 frames of the second pair released at once.
    ReleasePending,
    /// The user ring to a granting node flooded with signals.
    SignalPending,
}

const DRIVERS: [Driver; 13] = [
    Driver::Kill,
    Driver::Fail,
    Driver::Stale,
    Driver::Stuck,
    Driver::Retire,
    Driver::SealLate,
    Driver::Unfence,
    Driver::Resume,
    Driver::Fresh,
    Driver::FailUnit,
    Driver::HandDeath,
    Driver::ReleasePending,
    Driver::SignalPending,
];

/// The drivers of a seed: the triples of `DRIVERS` in turn.
fn drivers(seed: u64) -> [Driver; 3] {
    let n = DRIVERS.len();
    let triples: Vec<[Driver; 3]> = (0..n)
        .flat_map(|a| (a + 1..n).flat_map(move |b| (b + 1..n).map(move |c| [a, b, c])))
        .map(|[a, b, c]| [DRIVERS[a], DRIVERS[b], DRIVERS[c]])
        .collect();
    triples[seed as usize % triples.len()]
}

/// The exploration of one seed.
struct Run {
    m: S,
    rng: Rng,
    seen: Seen,
    drivers: [Driver; 3],
    /// The used entries of the device table at start.
    splittable: usize,
    /// The receiving thread of each holder.
    receivers: Vec<Option<ObjId>>,
    /// The drivers done once: the unit broken, the bootstrap core hung, a
    /// node sealed past `SEAL_NS`.
    once: BTreeSet<usize>,
    /// The node sealed past `SEAL_NS`, which nobody resumes, and the
    /// granting node a driver keeps sealed until it acts again.
    late: Option<u16>,
    kept: Option<u16>,
    /// The member words last printed by a replay.
    members: Vec<Member>,
    /// The run ended in the state of gap 21 of pass 26, and saw a failed
    /// unit's role handed.
    gap_21: bool,
    handed_failed: bool,
    /// A node whose events are held back, and for how many more churns.
    deferred: Option<(u16, u32)>,
    /// ARM64 and RISC-V: the start of the wide node's second core held
    /// back for so many churns, so that its first thread runs before it is
    /// live.
    late_start: u32,
    /// A node whose kernel also acts as a faulty kernel in this seed: it
    /// attempts gate writes that the gate must refuse and writes forged
    /// messages, sender slots, credits and beacon words that other nodes'
    /// gates and recovery reserves must refuse or withstand.
    faulty_node: u16,
    faulty_acts: usize,
    forged: Vec<Forged>,
    /// The spaces of the table chunks' cycle that map their frame.
    mapped: BTreeSet<(u16, u32)>,
}

impl Run {
    fn new(seed: u64) -> Self {
        // Five nodes, the last of two cores; the architecture by round.
        let cores: Vec<Vec<usize>> = (0..NODES as usize)
            .map(|c| match c as u16 == WIDE {
                true => vec![c, c + 1],
                false => vec![c],
            })
            .collect();
        let layout: Vec<&[usize]> = cores.iter().map(Vec::as_slice).collect();
        let arch = ARCHES[(seed / 286) as usize % ARCHES.len()];
        // The times of membership at the low end of their ranges, so that a
        // seal outlives `SEAL_NS` and a silent core is found stale within
        // the steps of a seed (Numbers: `SWEEP_NS` from 100 ms, `SEAL_NS`
        // and `STALE_NS` from 10 ms; `STUCK_NS` above the reserve's period
        // and `CORE_ANSWER_NS`).
        let mut machine = machine(arch, NODES as usize + 1, &layout);
        machine.membership.sweep_ms = 100;
        machine.membership.seal_ms = 30;
        machine.membership.stale_ms = 40;
        machine.membership.stuck_ms = 40;
        let m = S::on(&machine);
        let splittable = m
            .sys
            .canonical()
            .devices
            .iter()
            .filter(|e| e.is_used())
            .count();
        let faulty_node = (seed % u64::from(NODES)) as u16;
        let mut run = Self {
            m,
            rng: Rng(seed + 701),
            seen: Seen::default(),
            drivers: drivers(seed),
            splittable,
            receivers: vec![None; NODES as usize],
            once: BTreeSet::new(),
            late: None,
            kept: None,
            members: Vec::new(),
            gap_21: false,
            handed_failed: false,
            deferred: None,
            late_start: 0,
            faulty_node,
            faulty_acts: 0,
            forged: Vec::new(),
            mapped: BTreeSet::new(),
        };
        say(format_args!(
            "drivers {:?}, faulty node {}",
            run.drivers, run.faulty_node
        ));
        run.set_up();
        run
    }

    /// The message just written by `from` on a ring toward `to` is forged:
    /// noted, with `to`'s count of refusals.
    fn forged(&mut self, from: u16, to: u16) {
        let pair = &self.m.sys.canonical().rings[from as usize][to as usize];
        // Actions 6 and 8 write on the user ring, the others on the kernel
        // ring.
        let action = self.faulty_acts % 10;
        let user = matches!(action, 6 | 8);
        let end = match user {
            true => pair.user.produced,
            false => pair.kernel.produced,
        };
        self.forged.push(Forged {
            from,
            to,
            generation: self.m.sys.member(to).generation(),
            user,
            end,
            refused: self.m.sys.node(to).refused.0,
            action,
        });
    }

    /// Each forged message its receiver consumed is refused.
    fn judge_forged(&mut self) {
        for forged in std::mem::take(&mut self.forged) {
            let to = forged.to;
            let same = self.m.sys.has(to)
                && self.m.sys.member(to).generation() == forged.generation
                && self.m.sys.canonical().writes(to);
            if !same {
                continue;
            }
            let consumed =
                self.m.sys.canonical().beacons[to as usize].consumed[forged.from as usize];
            let consumed = if forged.user { consumed.1 } else { consumed.0 };
            if consumed.wrapping_sub(forged.end) >= 1 << 63 {
                self.forged.push(forged);
                continue;
            }
            assert!(
                self.m.sys.node(to).refused.0 > forged.refused,
                "the forged message of action {} from node {} to node {to} was not refused",
                forged.action,
                forged.from
            );
            self.seen.refused += 1;
        }
    }

    fn note(&mut self) {
        self.judge_forged();
        let table = self.m.sys.canonical();
        let fenced_held = (0..NODES).any(|x| {
            matches!(self.m.state(x), MemberState::Fenced | MemberState::Unfenced)
                && (0..NODES)
                    .any(|y| y != x && table.uses[y as usize].values().any(|u| u.granter == x))
        });
        if fenced_held {
            self.seen.tables.insert(9);
        }
        if replayed().is_some() {
            self.print_members();
        }
        let mask = probe(&self.m, self.splittable);
        self.seen.note(mask);
        let word = self.m.sys.role(IOMMU_ROLE);
        self.handed_failed |= word.failed() && word.state() == RoleState::Handing;
    }

    /// A replay prints every change of a member word, and for a new
    /// accusation the observations its driver held of the node.
    fn print_members(&mut self) {
        let members: Vec<Member> = (0..NODES).map(|n| self.m.sys.member(n)).collect();
        if members == self.members {
            return;
        }
        let table = self.m.sys.canonical();
        for (n, member) in members.iter().enumerate() {
            let was = self.members.get(n).map(|m| accused(m.state()));
            if accused(member.state()) && was == Some(false) {
                let beacons = table.beacons.get(usize::from(member.driver()));
                let held =
                    beacons.map(|b| b.observations.iter().filter(|o| usize::from(o.node) == n));
                let observations: Vec<_> = held.into_iter().flatten().collect();
                eprintln!(
                    "  node {n} accused by {}: {observations:?}",
                    member.driver()
                );
            }
        }
        eprintln!("{} members {members:?}", self.m.sys.now());
        self.members = members;
    }

    fn call(&mut self, node: u16, call: Syscall) -> Option<Outcome> {
        let outcome = try_call(&mut self.m, node, call);
        self.note();
        outcome
    }

    fn set_up(&mut self) {
        let m = &mut self.m;
        // ARM64 and RISC-V: every core idles in a state that loses its
        // context (`node_idle`, D-143, D-146).
        let lossy = match m.sys.arch() {
            Arch::Aarch64 => Some(dodos_abi::IDLE_METHOD_PSCI | 1 << 16 << 32),
            Arch::Riscv64 => Some(dodos_abi::IDLE_METHOD_SBI | 0x8000_0000 << 32),
            Arch::X86_64 => None,
        };
        for node in 0..NODES {
            let cores: Vec<u64> = match node == WIDE {
                true => vec![u64::from(node), u64::from(node) + 1],
                false => vec![u64::from(node)],
            };
            for core in cores.into_iter().filter(|_| lossy.is_some()) {
                let idle = Syscall::NodeIdle {
                    node: node_cap(node),
                    core,
                    words: vec![
                        200_000,
                        100_000,
                        lossy.expect("a lossy state"),
                        dodos_abi::IDLE_FLAG_LOSES_CONTEXT,
                    ],
                };
                m.ok(node, idle);
            }
        }
        for (node, device) in DEVICES {
            let iospace = m.retype(node, ObjectKind::IoSpace, 0, IOSPACE);
            m.ok(node, iospace);
            let call = attach_call(m, node, device, true);
            // Every seed starts with the device attached (the audit of lot
            // 8a, F15: a failed attach went unnoticed).
            match m.call(node, call) {
                WAITS => {
                    m.run(30 * MS);
                    assert_eq!(m.woken(node), Some(&DONE), "the attach of node {node}");
                }
                outcome => assert_eq!(outcome, DONE, "the attach of node {node}"),
            }
        }
        for pair in PAIRS {
            let reply = m.retype(pair.holder, ObjectKind::Reply, 0, REPLY);
            m.ok(pair.holder, reply);
            for i in 0..pair.frames {
                let frame = m.retype(pair.granter, ObjectKind::Frame, 1, pair.obj + i);
                m.ok(pair.granter, frame);
            }
            let bell = m.retype(pair.granter, ObjectKind::Notify, 0, BELL);
            m.ok(pair.granter, bell);
            let holder = pair.holder;
            m.ok(holder, super::place_first(holder, u64::from(holder)));
            let receiver = m.thread_on(holder, RECEIVER, u64::from(holder));
            self.receivers[holder as usize] = Some(receiver);
        }
        for pair in PAIRS {
            for group in 0..pair.frames.div_ceil(4) {
                self.grant(pair, group);
                self.m.settle();
            }
        }
        for pair in PAIRS {
            let map = map_call(&self.m, pair);
            let _ = self.call(pair.holder, map);
        }
    }

    /// The holder's receiver waits for up to four frames from `group`, and
    /// the granting node sends them, with its doorbell.
    fn grant(&mut self, pair: Pair, group: u32) {
        if self.receive(pair, group) {
            self.send(pair, group);
        }
    }

    /// The frames of a group: the first and how many.
    fn group(pair: Pair, group: u32) -> (u32, u32) {
        let first = 4 * group;
        (first, (pair.frames - first).min(4))
    }

    /// The holder's receiver waits for the frames of `group` in their
    /// slots, if it can and they are empty; whether it waits.
    fn receive(&mut self, pair: Pair, group: u32) -> bool {
        let holder = pair.holder;
        let Some(receiver) = self.receivers[holder as usize] else {
            return false;
        };
        let (first, count) = Self::group(pair, group);
        let m = &self.m;
        let t = m.t[holder as usize];
        let ready = m.sys.has(holder)
            && m.state(holder) == MemberState::Live
            && m.sys.node(holder).threads.contains_key(&receiver)
            && m.sys.node(holder).runnable().contains(&receiver)
            && !m
                .sys
                .node(holder)
                .is_silent(m.sys.node(holder).core_of(receiver))
            && (first..first + count)
                .all(|i| m.sys.node(holder).object_at(t, s(pair.got + i)).is_none());
        if !ready {
            return false;
        }
        let recv = Syscall::Recv {
            endpoint: s(dodos_abi::START_SLOT_MAILBOX),
            reply: s(REPLY),
            range: Range {
                first: s(pair.got + first),
                count,
            },
            wait: 0,
        };
        say(format_args!("{} node {holder}: receive", self.m.sys.now()));
        let _ = self.m.call_by(holder, receiver, recv);
        self.note();
        true
    }

    /// The granting node sends the frames of `group`, with its doorbell.
    fn send(&mut self, pair: Pair, group: u32) {
        let (first, count) = Self::group(pair, group);
        let rights = Rights::READ.union(Rights::WRITE).union(Rights::SEND).bits();
        let send = Syscall::NodeSend {
            node: node_cap(pair.holder),
            message: Message {
                caps: (first..first + count)
                    .map(|i| Transfer::Derive(s(pair.obj + i), rights))
                    .collect(),
                ..Message::default()
            },
            doorbell: s(BELL),
        };
        let _ = self.call(pair.granter, send);
    }

    /// The granting node a driver kept sealed is resumed.
    fn release_kept(&mut self) {
        if let Some(node) = self.kept.take()
            && self.m.state(node) == MemberState::Sealed
            && let Some(actor) = self.live_other(node)
        {
            let _ = self.call(actor, resume(node));
        }
    }

    /// The frames of a group released by the holder, then granted again.
    fn regrant(&mut self, pair: Pair, group: u32) {
        let (first, count) = Self::group(pair, group);
        for i in first..first + count {
            let _ = self.call(pair.holder, Syscall::CapDelete(s(pair.got + i)));
        }
        self.grant(pair, group);
    }

    /// Every frame of a pair the holder does not hold granted again, one
    /// message of four at a time, each delivered before the next.
    fn grant_all(&mut self, pair: Pair) {
        let holds = |run: &Self, i: u32| {
            run.m.sys.has(pair.holder)
                && run
                    .m
                    .sys
                    .node(pair.holder)
                    .object_at(run.m.t[pair.holder as usize], s(pair.got + i))
                    .is_some()
        };
        for group in 0..pair.frames.div_ceil(4) {
            if holds(self, 4 * group) {
                continue;
            }
            self.grant(pair, group);
            for _ in 0..8 {
                if holds(self, 4 * group) {
                    break;
                }
                self.churn();
            }
        }
    }

    /// Events at the current instant, each followed by the check and the
    /// probe, then time moves by up to 3 ms.
    fn churn(&mut self) {
        // A node whose events are held back: none of its interrupts or
        // timers is chosen, and time does not move, for a few steps (the
        // generator chooses which event happens now).
        let deferred = self
            .deferred
            .filter(|&(_, steps)| steps > 0)
            .map(|(n, _)| n);
        if let Some((node, steps)) = self.deferred {
            self.deferred = (steps > 1).then_some((node, steps - 1));
        }
        let late = self.late_start > 0;
        self.late_start = self.late_start.saturating_sub(1);
        let second = WIDE as usize + 1;
        for _ in 0..self.rng.below(12) {
            let events: Vec<_> = self
                .m
                .sys
                .events()
                .into_iter()
                .filter(|e| {
                    !matches!(e, crate::system::Event::Ipi { node, .. }
                        | crate::system::Event::Timer { node, .. } if Some(*node) == deferred)
                })
                .filter(|e| {
                    let crate::system::Event::Action(i) = *e else {
                        return true;
                    };
                    !late
                        || self.m.sys.actions()[i].1
                            != crate::nodes::HwAction::Start { core: second }
                })
                .collect();
            if events.is_empty() || self.m.sys.stopped {
                break;
            }
            let event = events[self.rng.below(events.len())];
            say(format_args!("{} event {event:?}", self.m.sys.now()));
            self.m.sys.fire(event);
            assert_eq!(self.m.sys.check(), Ok(()), "after {event:?}");
            self.note();
        }
        if deferred.is_none() && self.rng.below(2) == 0 {
            let ns = 1 + self.rng.below(3_000_000) as u64;
            self.run(ns);
        }
    }

    /// Time passes `ns`, checked and probed every `step`.
    fn run_by(&mut self, ns: u64, step: u64) {
        say(format_args!("{} run {ns}", self.m.sys.now()));
        let to = self.m.sys.now() + ns;
        while self.m.sys.now() < to && !self.m.sys.stopped {
            let at = (self.m.sys.now() + step).min(to);
            self.m.sys.advance(at);
            assert_eq!(self.m.sys.check(), Ok(()));
            self.note();
        }
    }

    /// Time passes `ns`, checked and probed every 100 µs.
    fn run(&mut self, ns: u64) {
        self.run_by(ns, 100_000);
    }

    /// Whether a node is live and unharmed in its generation.
    fn healthy(&self, n: u16) -> bool {
        let member = self.m.sys.member(n);
        member.state() == MemberState::Live
            && !self.m.harm.harmed.contains(&(n, member.generation()))
    }

    /// A node in `state`, chosen by the generator.
    fn any_in(&mut self, state: MemberState) -> Option<u16> {
        let nodes: Vec<u16> = (0..NODES).filter(|&n| self.m.state(n) == state).collect();
        (!nodes.is_empty()).then(|| nodes[self.rng.below(nodes.len())])
    }

    /// A healthy node other than `not`, chosen by the generator.
    fn live_other(&mut self, not: u16) -> Option<u16> {
        let live: Vec<u16> = (0..NODES)
            .filter(|&n| n != not && self.healthy(n))
            .collect();
        (!live.is_empty()).then(|| live[self.rng.below(live.len())])
    }

    /// Whether one more node may be harmed: at most three of the five are
    /// harmed or not live at once, so that two healthy nodes are left to
    /// recover them.
    fn may_harm(&self) -> bool {
        (0..NODES).filter(|&n| !self.healthy(n)).count() < 3
    }

    /// A healthy node to harm and a healthy node to act on it.
    fn victim(&mut self) -> Option<(u16, u16)> {
        if !self.may_harm() {
            return None;
        }
        let healthy: Vec<u16> = (0..NODES).filter(|&n| self.healthy(n)).collect();
        let victim = healthy[self.rng.below(healthy.len())];
        let actor = self.live_other(victim)?;
        Some((victim, actor))
    }

    fn kill(&mut self) {
        if let Some((victim, actor)) = self.victim() {
            let _ = self.call(actor, kill(victim));
        }
    }

    fn fail(&mut self) {
        if let Some((victim, _)) = self.victim() {
            say(format_args!("{} node {victim} fails", self.m.sys.now()));
            self.m.fail(victim);
            self.note();
        }
    }

    /// A core hangs; `nmi` whether its kernel can still run the fence's
    /// handler (on the bootstrap core, without it, no stop reaches it).
    fn hang(&mut self, victim: u16, nmi: bool) {
        let core = victim as usize;
        say(format_args!(
            "{} hang core {core} nmi {nmi}",
            self.m.sys.now()
        ));
        self.m.hang(core, nmi);
        self.note();
    }

    /// Stuck: the node's first thread suspends itself, so that its core
    /// is idle once its other threads wait; the core hangs, and a message
    /// waits in its ring.
    fn stuck(&mut self) {
        let Some((victim, actor)) = self.victim() else {
            return;
        };
        let suspend = Syscall::ThreadSuspend(s(dodos_abi::START_SLOT_THREAD));
        let _ = self.call(victim, suspend);
        if self.m.state(victim) == MemberState::Live
            && self.m.sys.node(victim).runnable().is_empty()
        {
            self.hang(victim, false);
            let send = Syscall::NodeSend {
                node: node_cap(victim),
                message: Message::default(),
                doorbell: SlotAddr::NONE,
            };
            let _ = self.call(actor, send);
        }
    }

    fn retire(&mut self) {
        if let Some((victim, actor)) = self.victim() {
            let retire = Syscall::NodeStop {
                node: node_cap(victim),
                mode: NODE_STOP_RETIRE,
                core: u64::from(victim),
            };
            let _ = self.call(actor, retire);
        }
    }

    fn seal(&mut self) -> Option<u16> {
        let (victim, _) = self.victim()?;
        self.seal_node(victim)
    }

    /// `node` sealed by a healthy node, if it is healthy and one more node
    /// may be harmed.
    fn seal_node(&mut self, victim: u16) -> Option<u16> {
        if !self.healthy(victim) || !self.may_harm() {
            return None;
        }
        let actor = self.live_other(victim)?;
        (self.call(actor, seal(victim)) == Some(DONE)).then_some(victim)
    }

    /// A healthy node sealed, then resumed as soon as it is sealed.
    fn quick_resume(&mut self) {
        let Some((victim, _)) = self.victim() else {
            return;
        };
        if self.seal_node(victim).is_none() {
            return;
        }
        for _ in 0..10 {
            if self.m.state(victim) == MemberState::Sealed {
                break;
            }
            self.churn();
        }
        self.recover(MemberState::Sealed);
    }

    /// A sealed node resumed, or a dead one started afresh, by a live
    /// node.
    fn recover(&mut self, state: MemberState) {
        let (late, kept) = (self.late, self.kept);
        let protected = move |n: u16| Some(n) == late || Some(n) == kept;
        let Some(target) = self.any_in(state).filter(|&n| !protected(n)) else {
            return;
        };
        let Some(actor) = self.live_other(target) else {
            return;
        };
        match state {
            // The resume's first wake lost, so that it lasts a retry.
            MemberState::Sealed => {
                if matches!(self.m.sys.hardware(target as usize), Hardware::Stub { .. }) {
                    self.m.sys.set_window(target as usize);
                }
                let _ = self.call(actor, resume(target));
            }
            _ => {
                // The wake of the start lost once, so that the start lasts
                // a retry (`START_RETRY_NS`); for the wide node, the wake of
                // its second core, so that its first thread runs before it
                // is live (on ARM64 and RISC-V, `CPU_ON` and
                // `sbi_hart_start` held back a few steps).
                let wide = target == WIDE;
                let late = target as usize + usize::from(wide);
                if matches!(self.m.sys.hardware(late), Hardware::Stub { .. }) {
                    self.m.sys.set_window(late);
                } else if wide {
                    self.late_start = 3 + self.rng.below(6) as u32;
                }
                let mask = match wide {
                    true => 0b11 << target,
                    false => 1 << target,
                };
                if self.call(actor, fresh(target, mask)) == Some(DONE) {
                    self.receivers[target as usize] = None;
                    // A grant between two other nodes during the start.
                    let pair = PAIRS[self.rng.below(PAIRS.len())];
                    self.regrant(pair, 0);
                    if wide {
                        self.before_live(target);
                    }
                }
            }
        }
    }

    /// The wide node's first thread, once its node is built and before
    /// it is live, sends to a live node and waits for the answer (pass 28:
    /// the window before live, interleaved with the rest).
    fn before_live(&mut self, node: u16) {
        let built = |run: &Self| {
            let member = run.m.sys.member(node);
            member.state() == MemberState::Starting
                && run.m.sys.has(node)
                && run.m.sys.node(node).generation_built() == member.generation()
                && run.m.sys.node(node).is_ready()
        };
        for _ in 0..8 {
            if built(self) || self.m.state(node) != MemberState::Starting {
                break;
            }
            self.churn();
        }
        if !built(self) {
            return;
        }
        self.m.restarted(node);
        let Some(peer) = self.live_other(node) else {
            return;
        };
        let send = Syscall::NodeSend {
            node: node_cap(peer),
            message: Message::default(),
            doorbell: SlotAddr::NONE,
        };
        say(format_args!(
            "{} node {node}: send before live",
            self.m.sys.now()
        ));
        let _ = self.call(node, send);
    }

    fn drive(&mut self, driver: Driver) {
        let first = |run: &mut Self| run.once.insert(driver as usize);
        match driver {
            Driver::Kill => self.kill(),
            Driver::Fail => self.fail(),
            Driver::Stale => {
                if let Some((victim, _)) = self.victim() {
                    self.hang(victim, true);
                }
            }
            Driver::Stuck => self.stuck(),
            Driver::Retire => self.retire(),
            // Sealed, then time passes until the node is accused with
            // evidence seal (within `SEAL_NS` + `SWEEP_NS`).
            // The driver is spent only by a seal that happened (the audit
            // of lot 8a, F5).
            Driver::SealLate => {
                if !self.once.contains(&(driver as usize))
                    && let Some(node) = self.seal()
                {
                    first(self);
                    self.late = Some(node);
                    let table = self.m.sys.canonical();
                    let end = self.m.sys.now() + table.seal_ns + table.sweep_ns;
                    while self.m.sys.now() < end && !accused(self.m.state(node)) {
                        self.run_by(5 * MS, 5 * MS);
                    }
                }
            }
            // In VMX root operation, where INIT is blocked (D-149: the
            // bootstrap core is stopped by INIT like the others).
            Driver::Unfence => {
                if self.healthy(0) && self.may_harm() && first(self) {
                    self.m.sys.set_vmx(0, true);
                    self.hang(0, false);
                }
            }
            // A holder sealed over what it holds, its granting node killed
            // one time in two meanwhile, then resumed: the resume's R2 has
            // remote capabilities to remove (Grants, "Generations").
            // One time in two the holder first releases its first frames
            // and waits for them again, and the granting node sends them
            // just before the seal: the grant is unanswered through the
            // seal and the resume.
            Driver::Resume => match self.any_in(MemberState::Sealed) {
                Some(_) => self.recover(MemberState::Sealed),
                None => {
                    let pair = PAIRS[self.rng.below(PAIRS.len())];
                    // Sent while the holder is live, and sealed before it
                    // serves its user ring.
                    if self.rng.below(2) == 0 {
                        self.regrant(pair, 0);
                    }
                    let sealed = self.seal_node(pair.holder).is_some();
                    let resume_now = self.rng.below(2) == 0;
                    if sealed
                        && self.rng.below(2) == 0
                        && self.healthy(pair.granter)
                        && self.may_harm()
                        && let Some(actor) = self.live_other(pair.granter)
                    {
                        let _ = self.call(actor, kill(pair.granter));
                    }
                    // One time in two resumed as soon as it is sealed; one
                    // time in three of those, under a rebuild: the IOMMU
                    // unit's holder killed, and the resume at its fence,
                    // so that its successor's rebuild asks the resuming
                    // node for its report (D-144).
                    if sealed && resume_now {
                        let holder = pair.holder;
                        for _ in 0..10 {
                            if self.m.state(holder) == MemberState::Sealed {
                                break;
                            }
                            self.churn();
                        }
                        let unit = self.m.sys.role(IOMMU_ROLE).holder();
                        if self.rng.below(3) == 0
                            && unit < NODES
                            && self.healthy(unit)
                            && self.may_harm()
                            && let Some(actor) = self.live_other(unit)
                        {
                            let _ = self.call(actor, kill(unit));
                            for _ in 0..20 {
                                if self.m.state(unit) != MemberState::Fencing {
                                    break;
                                }
                                self.churn();
                            }
                        }
                        self.recover(MemberState::Sealed);
                    }
                }
            },
            Driver::Fresh => match self.any_in(MemberState::Dead) {
                Some(_) => self.recover(MemberState::Dead),
                None => self.kill(),
            },
            Driver::FailUnit => {
                if first(self) {
                    say(format_args!("{} unit fails", self.m.sys.now()));
                    self.m.sys.break_unit(IOMMU_ROLE);
                    self.note();
                }
            }
            // Handed, and once the word reads handing, its holder or its
            // target killed.
            Driver::HandDeath => {
                if self.hand() {
                    let handing = |m: &S| m.sys.role(IOMMU_ROLE).state() != RoleState::Held;
                    for _ in 0..20 {
                        if handing(&self.m) {
                            break;
                        }
                        self.churn();
                    }
                }
                let word = self.m.sys.role(IOMMU_ROLE);
                if word.state() == RoleState::Handing {
                    let victim = match self.rng.below(2) {
                        0 => word.holder(),
                        _ => word.target(),
                    };
                    if victim < NODES
                        && self.may_harm()
                        && self.healthy(victim)
                        && let Some(actor) = self.live_other(victim)
                    {
                        let _ = self.call(actor, kill(victim));
                    }
                }
            }
            Driver::ReleasePending => {
                let pair = PAIRS[1];
                let held = |run: &Self| {
                    let node = run
                        .m
                        .sys
                        .has(pair.holder)
                        .then(|| run.m.sys.node(pair.holder));
                    let t = run.m.t[pair.holder as usize];
                    (0..pair.frames)
                        .filter(|&i| {
                            node.is_some_and(|n| n.object_at(t, s(pair.got + i)).is_some())
                        })
                        .count() as u32
                };
                self.release_kept();
                if held(self) == pair.frames {
                    // The granting node sealed one time in two, and kept
                    // so, so that the ring stays full and the release
                    // pending.
                    if self.rng.below(2) == 0 && self.seal_node(pair.granter).is_some() {
                        self.kept = Some(pair.granter);
                    }
                    for i in 0..pair.frames {
                        let _ = self.call(pair.holder, Syscall::CapDelete(s(pair.got + i)));
                    }
                    // The holder's events held back, so that its release
                    // stays pending while the rest goes on, and its R0 of
                    // a granting node accused meanwhile waits.
                    self.deferred = Some((pair.holder, 4 + self.rng.below(5) as u32));
                } else {
                    self.grant_all(pair);
                }
            }
            Driver::SignalPending => {
                let pair = PAIRS[self.rng.below(PAIRS.len())];
                self.release_kept();
                self.grant_all(pair);
                if self.rng.below(2) == 0 && self.seal_node(pair.granter).is_some() {
                    self.kept = Some(pair.granter);
                }
                for _ in 0..70 {
                    let signal = Syscall::GrantSignal(s(pair.got));
                    if self.call(pair.holder, signal) != Some(DONE) {
                        break;
                    }
                }
            }
        }
    }

    /// One action of a grant: a map, an unmap, a release, a revocation,
    /// the two at once, a new grant, or a signal.
    fn grants(&mut self) {
        let pair = PAIRS[self.rng.below(PAIRS.len())];
        let i = self.rng.below(pair.frames as usize) as u32;
        let (granter, holder) = (pair.granter, pair.holder);
        match self.rng.below(7) {
            0 if self.m.sys.has(holder) => {
                let map = map_call(&self.m, pair);
                let _ = self.call(holder, map);
            }
            1 => {
                let unmap = match pair.io {
                    true => Syscall::IoSpaceUnmap(s(pair.got)),
                    false => Syscall::ASpaceUnmap(s(pair.got)),
                };
                let _ = self.call(holder, unmap);
            }
            2 => {
                let _ = self.call(holder, Syscall::CapDelete(s(pair.got + i)));
            }
            3 => {
                let _ = self.call(granter, Syscall::CapRevoke(s(pair.obj + i)));
            }
            // The first frame mapped, then its revocation and its release
            // at once: a revocation of a frame the holder counts no leaf
            // under frees the grant with no `Revoke`.
            4 if self.m.sys.has(holder) => {
                let map = map_call(&self.m, pair);
                let _ = self.call(holder, map);
                let _ = self.call(granter, Syscall::CapRevoke(s(pair.obj)));
                let _ = self.call(holder, Syscall::CapDelete(s(pair.got)));
            }
            5 => self.regrant(pair, i / 4),
            _ => {
                let _ = self.call(holder, Syscall::GrantSignal(s(pair.got + i)));
            }
        }
    }

    /// A device attached or detached by its node.
    fn devices(&mut self) {
        let (node, device) = DEVICES[self.rng.below(DEVICES.len())];
        if !self.m.sys.has(node) {
            return;
        }
        let attach = self.rng.below(2) == 0;
        let t = self.m.t[node as usize];
        if self.m.sys.node(node).object_at(t, s(IOSPACE)).is_none() {
            let iospace = self.m.retype(node, ObjectKind::IoSpace, 0, IOSPACE);
            let _ = self.call(node, iospace);
            return;
        }
        let call = attach_call(&self.m, node, device, attach);
        let _ = self.call(node, call);
    }

    /// The line claimed by one of its two nodes, or removed.
    fn line(&mut self) {
        let node = LINERS[self.rng.below(LINERS.len())];
        if !self.m.sys.has(node) {
            return;
        }
        let t = self.m.t[node as usize];
        let call = match self.m.sys.node(node).object_at(t, s(IRQ)) {
            Some(_) => Syscall::CapDelete(s(IRQ)),
            None => Syscall::IrqClaim {
                control: s(dodos_abi::START_SLOT_IRQ_CONTROL),
                source: IrqSource::line(GSI, false, false).raw(),
                core: u64::from(node),
                vector: 48,
                destination: s(IRQ),
            },
        };
        let _ = self.call(node, call);
    }

    /// The IOMMU role handed by its holder to another node; whether it was
    /// asked.
    fn hand(&mut self) -> bool {
        let word = self.m.sys.role(IOMMU_ROLE);
        let holder = word.holder();
        if word.state() != RoleState::Held || holder >= NODES || !self.m.sys.has(holder) {
            return false;
        }
        let Some(target) = self.live_other(holder) else {
            return false;
        };
        let t = self.m.t[holder as usize];
        let roles = slots_of(self.m.sys.node(holder), t, ObjectKind::Role);
        let hand = Syscall::RoleHand {
            role: s(roles[IOMMU_ROLE]),
            node: node_cap(target),
        };
        self.call(holder, hand).is_some()
    }

    /// A chunk taken and given between nodes 1 and 3, or a device entry
    /// taken and split by one of them.
    fn memory(&mut self) {
        let node = LINERS[self.rng.below(LINERS.len())];
        if !self.m.sys.has(node) {
            return;
        }
        let other = LINERS.iter().copied().find(|&n| n != node).expect("two");
        let control = s(dodos_abi::START_SLOT_MEMORY_CONTROL);
        let table = self.m.sys.canonical();
        if self.rng.below(4) == 0 {
            let page = table.geometry.page().trailing_zeros();
            let free = |e: &&dodos_abi::DeviceEntry| e.owner.state() == OwnState::Free;
            let Some(index) = table
                .devices
                .iter()
                .position(|e| e.is_used() && e.log2() > page && free(&e))
            else {
                return;
            };
            if self.call(node, super::pass22::take(1, index as u64, 0, ENTRY)) == Some(DONE) {
                let split = Syscall::MemorySplit {
                    control,
                    untyped: s(ENTRY),
                };
                let _ = self.call(node, split);
            }
            return;
        }
        let t = self.m.t[node as usize];
        if self.m.sys.node(node).object_at(t, s(BLOCK)).is_some() {
            let give = Syscall::MemoryGive {
                control,
                untyped: s(BLOCK),
                node: u64::from(other),
            };
            let _ = self.call(node, give);
            return;
        }
        let shift = table.geometry.chunk_shift();
        let mine = Ownership::free(Owner::Node(node)).raw();
        let free = Ownership::free(Owner::Unclaimed).raw();
        let chunk = table
            .owners
            .iter()
            .position(|&w| w == mine)
            .or_else(|| table.owners.iter().position(|&w| w == free));
        if let Some(chunk) = chunk {
            let take = super::pass22::take(0, (chunk as u64) << shift, u64::from(shift), BLOCK);
            let _ = self.call(node, take);
        }
    }

    /// The table chunks of D-154 among the recovery states, one step of a
    /// cycle on a node: a table run taken (kind 2), a child table untyped
    /// cut from it, an `aspace` or an `iospace` made on the child, a frame
    /// mapped under it with the child as its table untyped, the space
    /// destroyed with its mapping in place, the child deleted, the run
    /// given to another node or destroyed. Any step may meet a death, a
    /// seal or a fresh start of the node or of another.
    fn tables(&mut self) {
        // A node already in the cycle, else one drawn; up to four steps.
        let started = (0..NODES).find(|&n| {
            self.healthy(n)
                && self.m.sys.has(n)
                && self
                    .m
                    .sys
                    .node(n)
                    .object_at(self.m.t[n as usize], s(TBLOCK))
                    .is_some()
        });
        let node = started.unwrap_or_else(|| self.rng.below(NODES as usize) as u16);
        for _ in 0..4 {
            if !self.healthy(node) || !self.m.sys.has(node) {
                return;
            }
            self.table_step(node);
        }
    }

    /// One step of the cycle of `tables` on `node`.
    fn table_step(&mut self, node: u16) {
        let t = self.m.t[node as usize];
        let model = self.m.sys.node(node);
        let has = |slot: u32| model.object_at(t, s(slot)).is_some();
        let (block, child, frame) = (has(TBLOCK), has(TCHILD), has(TFRAME));
        let spaces = [(TASPACE, has(TASPACE)), (TIOSPACE, has(TIOSPACE))];
        let ut = super::untyped_slot(model, t);
        let geometry = self.m.sys.canonical().geometry;
        let done = |outcome: Option<Outcome>| outcome == Some(DONE);
        if !block && !child {
            let shift = geometry.chunk_shift();
            let table = self.m.sys.canonical();
            let mine = Ownership::free(Owner::Node(node)).raw();
            let free = Ownership::free(Owner::Unclaimed).raw();
            let chunk = table
                .owners
                .iter()
                .position(|&w| w == mine)
                .or_else(|| table.owners.iter().position(|&w| w == free));
            if let Some(chunk) = chunk {
                let place = (chunk as u64) << shift;
                let take = super::pass22::take(
                    dodos_abi::MEMORY_TAKE_TABLES,
                    place,
                    u64::from(shift),
                    TBLOCK,
                );
                if done(self.call(node, take)) {
                    self.seen.tables.insert(0);
                }
            }
            return;
        }
        if !frame {
            let _ = self.call(node, retype_from(ut, ObjectKind::Frame, 1, TFRAME));
            return;
        }
        if block && !child {
            // One time in four the run goes: given or destroyed.
            if self.rng.below(4) == 0 {
                let control = s(dodos_abi::START_SLOT_MEMORY_CONTROL);
                let other = (node + 1 + self.rng.below(NODES as usize - 1) as u16) % NODES;
                let release = match self.rng.below(2) {
                    0 => Syscall::MemoryGive {
                        control,
                        untyped: s(TBLOCK),
                        node: u64::from(other),
                    },
                    _ => Syscall::CapDelete(s(TBLOCK)),
                };
                if done(self.call(node, release)) {
                    self.seen.tables.insert(8);
                }
            } else {
                let page = geometry.page();
                let cut = retype_from(TBLOCK, ObjectKind::Untyped, 8 * page, TCHILD);
                if done(self.call(node, cut)) {
                    self.seen.tables.insert(1);
                }
            }
            return;
        }
        let live: Vec<u32> = spaces.iter().filter(|s| s.1).map(|s| s.0).collect();
        let Some(&space) = live.first() else {
            // No space: one made on the child, or one time in four the
            // child deleted.
            if self.rng.below(4) == 0 {
                let _ = self.call(node, Syscall::CapDelete(s(TCHILD)));
                return;
            }
            let (slot, kind, state) = match self.rng.below(2) {
                0 => (TASPACE, ObjectKind::ASpace, 2),
                _ => (TIOSPACE, ObjectKind::IoSpace, 3),
            };
            let make = retype_from(ut, kind, s(TCHILD).raw(), slot);
            if done(self.call(node, make)) {
                self.seen.tables.insert(state);
                self.mapped.remove(&(node, slot));
            }
            return;
        };
        let io = space == TIOSPACE;
        if !self.mapped.contains(&(node, space)) {
            let map = crate::Map {
                space: s(space),
                frame: s(TFRAME),
                untyped: s(TCHILD),
                address: 0x4000_0000,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            };
            let call = match io {
                true => Syscall::IoSpaceMap(map),
                false => Syscall::ASpaceMap(map),
            };
            if done(self.call(node, call)) {
                self.mapped.insert((node, space));
                self.seen.tables.insert(if io { 5 } else { 4 });
            }
            return;
        }
        // Destroyed with its mapping in place, in bounded steps.
        for _ in 0..16 {
            match self.call(node, Syscall::CapDelete(s(space))) {
                Some(DONE) => {
                    self.mapped.remove(&(node, space));
                    self.seen.tables.insert(if io { 7 } else { 6 });
                    return;
                }
                Some(Err(Error::Again)) => self.run_by(MS, 100_000),
                _ => return,
            }
        }
    }

    /// A faulty node's action alongside the normal exploration: it attempts
    /// gate writes that its own gate must refuse when counts or state forbid
    /// them, and writes forged messages, sender slots, credits and early
    /// `acked`/`maps_of` beacon words that the other nodes' gates and
    /// recovery reserves must refuse or withstand without breaking machine
    /// invariants or liveness.
    fn faulty(&mut self) {
        use crate::rings::Message;
        use dodos_abi::{NodeMessage, RingHeader};
        let f = self.faulty_node;
        if !self.m.sys.has(f) || !self.m.sys.canonical().writes(f) {
            return;
        }
        let Some(to) = (1..NODES)
            .map(|step| (f + step) % NODES)
            .find(|&n| self.healthy(n))
        else {
            return;
        };
        let fg = self.m.sys.member(f).generation();
        let tg = self.m.sys.member(to).generation();
        let can_send = |m: &S, from: u16, dest: u16, user: bool, entries: u64| {
            let table = m.sys.canonical();
            let pair = &table.rings[from as usize][dest as usize];
            let consumed = table.beacons[dest as usize].consumed[from as usize];
            match user {
                true => {
                    pair.user.produced.wrapping_sub(consumed.1) + entries
                        <= dodos_abi::RING_USER_ENTRIES
                }
                false => {
                    pair.kernel.produced.wrapping_sub(consumed.0)
                        + entries
                        + dodos_abi::RING_ANSWER_RESERVE
                        <= dodos_abi::RING_KERNEL_ENTRIES
                }
            }
        };
        self.faulty_acts += 1;
        if self.faulty_acts.is_multiple_of(10) {
            let table = self.m.sys.canonical();
            let busy_chunk = (0..table.owners.len() as u64).find(|&c| {
                Ownership::from_raw(table.owners[c as usize]).is_some_and(|w| {
                    w.state() == OwnState::Taken
                        && w.owner() == Owner::Node(f)
                        && w.generation() == fg
                }) && (table.grant_count(c) > 0 || table.map_count(c) > 0)
            });
            if let Some(chunk) = busy_chunk {
                assert!(!self.m.sys.node_mut(f).gate_give_chunk(chunk, to));
            }
            if self.m.state(f) != MemberState::Sealing {
                assert!(!self.m.sys.gate_park_self(f, f as usize));
            }
            // D-154: its gate puts no table in an ordinary chunk it holds
            // (its ordinary floor's first page here), and writes no leaf
            // and no grant's anchor toward its table floor.
            let table = self.m.sys.canonical();
            let geometry = table.geometry;
            let class = dodos_abi::PageClass::table(1).expect("a class");
            let ordinary = table.floors[f as usize][0].0 / geometry.page();
            let tables = table.table_floors[f as usize][0].0 / geometry.page();
            let last = tables + geometry.chunk() / geometry.page() - 1;
            let node = self.m.sys.node_mut(f);
            assert!(!node.gate_retype_page(ordinary, class));
            assert!(!node.gate_leaf_allowed(last) && !node.gate_anchor_allowed(last));
            // The same toward the table run of `tables` it holds: no leaf,
            // no anchor into it, and no give while a table lies in it.
            let block = node.object_at(self.m.t[f as usize], s(TBLOCK));
            if let Some(base) = block.and_then(|b| node.untypeds.get(&b)).map(|u| u.base) {
                let chunk = base >> geometry.chunk_shift();
                let first = base / geometry.page();
                assert!(!node.gate_leaf_allowed(first) && !node.gate_anchor_allowed(first));
                if node.table.gate_word(chunk).not_data() > 0 {
                    assert!(!node.gate_give_chunk(chunk, to));
                }
                self.seen.tables.insert(10);
            }
        }
        let table = self.m.sys.canonical();
        match self.faulty_acts % 10 {
            0 => {}
            // 1: forged sender slot in header (speaking in a third node's
            // name): refused by `to` (`header.sender != from`, D-141).
            1 if can_send(&self.m, f, to, false, 1) => {
                let third = (0..NODES).find(|&z| z != f && z != to).unwrap_or(0);
                let header = RingHeader::new(
                    NodeMessage::Release,
                    1,
                    (third, self.m.sys.member(third).generation()),
                    tg,
                    None,
                );
                let body = Body::Release {
                    grant: 0,
                    serial: 1,
                };
                self.m.sys.inject(f, to, Message { header, body });
                self.forged(f, to);
            }
            // 2: forged `Release` for a live grant `f` holds while `uses[f]`
            // still counts leaves > 0: refused by the granter (D-141).
            2 => {
                let held = (0..NODES).find_map(|g| {
                    table.grants[g as usize]
                        .iter()
                        .find(|(_, e)| {
                            e.grantee == f
                                && e.generation == fg
                                && e.state == GrantState::Live
                                && e.use_index.is_some_and(|i| {
                                    table.uses[f as usize].get(&i).is_some_and(|u| u.leaves > 0)
                                })
                        })
                        .map(|(&id, e)| (g, id, e.serial))
                });
                if let Some((granter, grant, serial)) = held
                    && can_send(&self.m, f, granter, false, 1)
                {
                    let gg = self.m.sys.member(granter).generation();
                    let header = RingHeader::new(NodeMessage::Release, 1, (f, fg), gg, None);
                    let body = Body::Release { grant, serial };
                    self.m.sys.inject(f, granter, Message { header, body });
                    self.forged(f, granter);
                }
            }
            // 3: forged `RevokeStep` naming a credit `to` does not have in
            // use toward `f`: refused by `credit_in_use` (D-141).
            3 if can_send(&self.m, f, to, false, 1) => {
                let header = RingHeader::new(
                    NodeMessage::RevokeStep,
                    1,
                    (f, fg),
                    tg,
                    Some((15, 0x7ff_ffff)),
                );
                let body = Body::RevokeStep { removed: true };
                self.m.sys.inject(f, to, Message { header, body });
                self.forged(f, to);
            }
            // 4: forged `UnitRequest` to the IOMMU role holder with a send
            // credit (index 20, outside 0..16): refused by the holder's
            // receive check without occupying `unit_service`.
            4 => {
                let holder = self.m.sys.role(IOMMU_ROLE).holder();
                if holder != f
                    && holder < NODES
                    && self.m.sys.canonical().writes(holder)
                    && can_send(&self.m, f, holder, false, 1)
                {
                    let hg = self.m.sys.member(holder).generation();
                    let epoch = self.m.sys.role(IOMMU_ROLE).epoch();
                    let header =
                        RingHeader::new(NodeMessage::UnitRequest, 1, (f, fg), hg, Some((20, 1)));
                    let body = Body::UnitRequest {
                        entry: 0,
                        serial: 0,
                        role: IOMMU_ROLE as u16,
                        epoch,
                        op: crate::rings::UnitOp::Invalidate { domain: Some(1) },
                    };
                    self.m.sys.inject(f, holder, Message { header, body });
                    self.forged(f, holder);
                }
            }
            // 5: forged `UnitAnswer` to the IOMMU role holder naming a
            // request credit the holder does not have in use toward `f`:
            // refused by `credit_in_use` (D-141).
            5 => {
                let holder = self.m.sys.role(IOMMU_ROLE).holder();
                if holder != f
                    && holder < NODES
                    && self.m.sys.canonical().writes(holder)
                    && can_send(&self.m, f, holder, false, 1)
                {
                    let hg = self.m.sys.member(holder).generation();
                    let header = RingHeader::new(
                        NodeMessage::UnitAnswer,
                        1,
                        (f, fg),
                        hg,
                        Some((15, 0x7ff_fffe)),
                    );
                    let body = Body::UnitAnswer {
                        result: dodos_abi::UnitResult::Done,
                    };
                    self.m.sys.inject(f, holder, Message { header, body });
                    self.forged(f, holder);
                }
            }
            // 6: forged `Send` on the user ring with no credit (`None`,
            // outside the send credit range 16..24) and a stamp naming
            // another node: refused by `to`'s receive check.
            6 if can_send(&self.m, f, to, true, 1) => {
                let body = Body::Send {
                    header: dodos_abi::Header::new(0, 1, 0, 0).expect("valid"),
                    stamp: (u64::from(to) << 48 | 1, 0b100),
                    grants: Vec::new(),
                    words: vec![0; 1],
                };
                let header =
                    RingHeader::new(NodeMessage::Send, body.entries() as u8, (f, fg), tg, None);
                self.m.sys.inject(f, to, Message { header, body });
                self.forged(f, to);
            }
            // 7: forged `Report` to `to` with an unused credit: refused by
            // `to` (`credit_in_use`), leaving any active rebuild untouched.
            7 if can_send(&self.m, f, to, false, 1) => {
                let header =
                    RingHeader::new(NodeMessage::Report, 1, (f, fg), tg, Some((14, 0x7ff_fffc)));
                let body = Body::Report {
                    next: dodos_abi::REPORT_END,
                    items: vec![crate::rings::Item::Device(0, 0)],
                };
                self.m.sys.inject(f, to, Message { header, body });
                self.forged(f, to);
            }
            // 8: unreadable raw header on the user ring (length 0 or
            // reserved kind 9): `to` consumes 1 entry, traces `Refused` and
            // continues.
            8 if can_send(&self.m, f, to, true, 1) => {
                let mut bad = RingHeader::new(NodeMessage::Release, 0, (f, fg), tg, None);
                bad.kind = 9;
                self.m.sys.inject_raw(f, to, true, bad, 1);
                self.forged(f, to);
            }
            // 9: early `beacons[f].acked[x] = g` and `maps_of[x] = false`
            // written by `f` (when `f` holds no unit role, has run R0 and
            // R4 for `x`, and has no active grant or remote capability of
            // `x`)
            // while `x` is `Fenced`: R6's proof (`r6_held_back`, D-149)
            // still holds R6 back until every grantee's use entry counts no
            // leaf under `x`'s grants and no attach word of `x` reads
            // `Attached`.
            9 => {
                let holds_unit = table.role_records.iter().zip(&table.roles).any(|(r, w)| {
                    r.kind == dodos_abi::RoleKind::Unit
                        && w.state() != RoleState::Vacant
                        && (w.holder() == f || w.target() == f)
                });
                let fenced = (0..NODES).filter(|_| !holds_unit).find(|&x| {
                    let xg = self.m.sys.member(x).generation();
                    x != f
                        && self.m.state(x) == MemberState::Fenced
                        && self.m.sys.node(f).reclaims.get(&x).is_some_and(|r| {
                            // R4's audit ended too: `f`'s own check
                            // holds R5 after it (pass 34).
                            r.generation == xg
                                && r.isolated
                                && r.audit.as_ref().is_some_and(Vec::is_empty)
                        })
                        && !table.grants[x as usize].values().any(|e| e.grantee == f)
                        && !table.grants[f as usize].values().any(|e| e.grantee == x)
                });
                if let Some(x) = fenced {
                    let xg = self.m.sys.member(x).generation();
                    self.m.sys.edit_table(|t| {
                        t.beacons[f as usize].acked[x as usize] = xg;
                        t.beacons[f as usize].maps_of[x as usize] = false;
                    });
                }
            }
            _ => {}
        }
        self.m
            .sys
            .check()
            .expect("invariants hold after faulty action");
        self.note();
    }

    /// One step: a driver one time in three, an action of the machine's
    /// other parts, then events and time, and a faulty node's action.
    fn step(&mut self) {
        // A node started afresh has a new first thread once it runs.
        refresh(&mut self.m);
        // While a node is accused, a resume beside it: a healthy node
        // sealed and resumed at once (a resume lasts a few events, an
        // accusation its fence and reclaim).
        let accusation = (0..NODES).any(|n| accused(self.m.state(n)));
        if accusation && self.drivers.contains(&Driver::Resume) && self.rng.below(2) == 0 {
            self.quick_resume();
        }
        if self.rng.below(3) == 0 {
            let driver = self.drivers[self.rng.below(3)];
            self.drive(driver);
        }
        match self.rng.below(10) {
            0..=3 => self.grants(),
            4 => self.devices(),
            5 => self.line(),
            6 => {
                let _ = self.hand();
            }
            7 if self.rng.below(2) == 0 => self.memory(),
            7 => self.tables(),
            8 => self.recover(MemberState::Sealed),
            _ => self.recover(MemberState::Dead),
        }
        self.churn();
        self.faulty();
    }

    /// The machine runs until it is quiet; then no node is accused that was
    /// not harmed, and no live node waits for another node, a unit or its
    /// cores.
    fn liveness(&mut self, seed: u64) {
        for n in 0..NODES {
            if self.m.state(n) == MemberState::Sealed
                && let Some(actor) = self.live_other(n)
            {
                let _ = self.call(actor, resume(n));
            }
        }
        // A wait for a node starting afresh ends by `START_NS` (D-144).
        let sweep = self.m.sys.canonical().sweep_ns;
        self.run_by(sweep + 300 * MS, MS);
        if (0..NODES).any(|n| self.m.state(n) == MemberState::Starting) {
            self.run_by(dodos_abi::START_NS + sweep, MS);
        }
        self.gap_21 = super::explore::sealing_holds_a_failed_unit(&self.m);
        super::explore::assert_live(&self.m, seed, NODES);
    }
}

/// The attach of a node's device into its iospace, or its detach.
fn attach_call(m: &S, node: u16, device: u64, attach: bool) -> Syscall {
    let iommu = slots_of(m.sys.node(node), m.t[node as usize], ObjectKind::Iommu)[0];
    Syscall::IoSpaceAttach {
        iommu: s(iommu),
        iospace: if attach { s(IOSPACE) } else { SlotAddr::NONE },
        device,
    }
}

/// The holder's map of the first frame it holds.
fn map_call(m: &S, pair: Pair) -> Syscall {
    let map = |space, untyped| crate::Map {
        space,
        frame: s(pair.got),
        untyped,
        address: 0x1000_0000,
        first: 0,
        count: 1,
        rights: Rights::READ.bits(),
    };
    match pair.io {
        true => {
            // The iospace's table untyped: the holder's table floor's
            // (D-154).
            let ut = super::table_slot(m.sys.node(pair.holder), m.t[pair.holder as usize]);
            Syscall::IoSpaceMap(map(s(IOSPACE), s(ut)))
        }
        false => Syscall::ASpaceMap(map(s(dodos_abi::START_SLOT_ASPACE), SlotAddr::NONE)),
    }
}

/// One seed explored: what it reached, whether it ended in the state of
/// gap 21 of pass 26, and whether it handed a failed unit's role.
fn explore_one(seed: u64) -> (Seen, [bool; 2]) {
    say(format_args!("seed {seed}"));
    let explored = std::panic::catch_unwind(|| {
        let mut run = Run::new(seed);
        for _ in 0..STEPS {
            run.step();
        }
        run.liveness(seed);
        (run.seen, [run.gap_21, run.handed_failed])
    });
    explored.unwrap_or_else(|panic| {
        eprintln!("seed {seed} fails; DODOS_SEED={seed} replays it");
        std::panic::resume_unwind(panic)
    })
}

/// The seeds, explored on every core of the machine that runs the tests
/// (they are independent); the combinations reached. `DODOS_SEED`
/// replays one instead.
fn explore(seeds: Vec<u64>) -> Reach {
    let seeds = match replayed() {
        Some(seed) => vec![seed],
        None => seeds,
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let runs: Vec<(Seen, [bool; 2])> = std::thread::scope(|scope| {
        let seeds = &seeds;
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let mine = seeds.iter().skip(i).step_by(threads);
                scope.spawn(move || mine.map(|&s| explore_one(s)).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    let mut reach = Reach::new();
    for (seen, [gap_21, handed]) in &runs {
        reach.add(seen);
        reach.gap_21 += usize::from(*gap_21);
        reach.handed_failed += usize::from(*handed);
    }
    reach
}

/// The pairs no seed can reach, each with the sentences of the contract
/// that exclude it; a triple that holds such a pair is excluded too. None
/// since D-145: a failed unit under a hand-over, excluded in pass 25
/// (`role_hand` is `busy` on a failed unit, and a hand-over waited for a
/// drain a failed unit never makes), is handed by a seal now.
const IMPOSSIBLE: &[(usize, usize)] = &[];

fn report(reach: &Reach) -> Vec<String> {
    let mut missing = Vec::new();
    let print = std::env::var("DODOS_COUNTS").is_ok();
    let excluded = |a: usize, b: usize| IMPOSSIBLE.contains(&(a, b));
    if print {
        for (state, seeds) in TABLE_STATES.iter().zip(reach.tables) {
            eprintln!("seeds reaching {state}: {seeds}");
        }
        eprintln!("seeds ending in gap 21: {}", reach.gap_21);
        eprintln!("seeds that hand a failed unit: {}", reach.handed_failed);
        for e in 0..EVENTS {
            eprintln!("event {:>26}: {}", NAMES[e], reach.events[e]);
        }
    }
    for (anchor, table, range) in [
        ("pair", &reach.pairs, 0..EVENTS),
        ("resume", &reach.resume, 0..EVENTS),
        ("death", &reach.death, DEATH_OTHERS),
    ] {
        for a in range.clone() {
            for b in a + 1..range.end {
                if anchor == "resume" && (a == RESUME || b == RESUME) {
                    continue;
                }
                let count = table[a][b];
                if print {
                    eprintln!("{anchor} + {} + {}: {count}", NAMES[a], NAMES[b]);
                }
                if count == 0 && !excluded(a, b) {
                    missing.push(format!("{anchor} + {} + {}", NAMES[a], NAMES[b]));
                }
            }
        }
    }
    missing
}

/// The combinations the 26 seeds of `verify` do not reach, measured
/// (`DODOS_COUNTS=1` prints them).
const MISSED_BY_26_SEEDS: usize = 451;

/// The run of `verify`: 26 seeds spread over the triples of drivers, so
/// that each driver is among them; the check after every step and the
/// liveness of each seed are asserted, and each event is reached.
#[test]
fn combined_recovery_states_between_nodes() {
    // Over the three rounds of the triples, one per architecture.
    let seeds = 3 * 286;
    let reach = explore((0..26).map(|i| i * seeds / 26).collect());
    let missing = report(&reach);
    if replayed().is_none() {
        for e in 0..EVENTS {
            assert!(reach.events[e] > 0, "the seeds never reach {}", NAMES[e]);
        }
        // The 26 seeds reach part of the combinations only (the long run
        // below asserts every one): as many as they reached when this was
        // measured, so a change that loses some fails here (the audit of
        // lot 8a, F1: the list was thrown away).
        assert!(reach.refused > 0, "no forged message seen refused");
        for (state, &seeds) in TABLE_STATES.iter().zip(&reach.tables) {
            assert!(seeds > 0, "the seeds never reach {state}");
        }
        assert!(
            missing.len() <= MISSED_BY_26_SEEDS,
            "{} combinations never reached, {MISSED_BY_26_SEEDS} measured: {missing:#?}",
            missing.len()
        );
    }
}

/// The long run: three rounds of the 286 triples of drivers by default
/// (`DODOS_COMBINATION_SEEDS` sets another number, `DODOS_COMBINATION_FIRST`
/// the first seed, for a run in ranges); every combination not shown
/// impossible is reached by a seed.
#[test]
#[ignore = "long: run with --release; DODOS_COMBINATION_SEEDS"]
fn every_combination_of_recovery_states_is_reached() {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .map_or(default, |n| n.parse().expect("a number of seeds"))
    };
    let (count, first) = (
        number("DODOS_COMBINATION_SEEDS", 858),
        number("DODOS_COMBINATION_FIRST", 0),
    );
    let reach = explore((first..first + count).collect());
    let missing = report(&reach);
    if replayed().is_none() {
        assert!(missing.is_empty(), "never reached: {missing:#?}");
    }
}

// Seed 867 of the long run of pass 28 (the checker's rule ACCUSE, D-146):
// node 3's last seal step wrote sealed, its last gate call, and went on in
// the same step to accuse node 0, which had just failed. A node fences,
// reclaims and accuses nothing once it has written sealed.
#[test]
fn seed_867_of_pass_28_a_node_that_wrote_sealed_accuses_no_one() {
    let reach = explore(vec![867]);
    assert!(reach.events[SEALED] > 0);
}

// Seed 1434 of the long run of pass 28 (the checker's rule ACCUSE, D-146):
// node 4, starting afresh, accused node 3 with evidence self from its
// reserve. Only a live or sealing node writes fencing.
#[test]
fn seed_1434_of_pass_28_a_starting_node_accuses_no_one() {
    let reach = explore(vec![1434]);
    assert!(reach.events[FRESH] > 0);
}

// Seed 1109 of the long run of pass 26: a rebuild's `ReportAsk` to a node
// that sealed before reading it stayed in flight while the rebuild passed
// the node over; the node resumed and answered, and the late report moved
// the rebuild's cursor back while another node was asked, so that a second
// `ReportAsk` went to a node with one in flight (RESERVE). A report of a
// node no longer asked now frees its credit and moves nothing, and no ask
// goes to a node with one in flight (Roles, the reports are pulled).
#[test]
fn a_late_report_of_a_node_passed_over_moves_no_rebuild() {
    let _ = explore_one(1109);
}
