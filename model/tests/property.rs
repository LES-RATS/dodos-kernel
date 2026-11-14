//! Random call sequences: every invariant holds after every call, a seed
//! always gives the same state, and the generator reaches the calls, the
//! errors and the deep states that the coverage tests below name.
//!
//! The generator mixes two things. Single calls with random arguments, biased
//! towards slots that hold what the call needs. And scripts: a thread begins
//! something that takes many calls (a chain of copies down to depth 64, 33
//! copies of a capability and its removal, a process with its own capability
//! space, a nest of spaces and its removal) and finishes it over the next
//! steps, while the other threads go on making calls in between.

use dodos_abi::*;
use dodos_model::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::OnceLock;

const SEEDS: u64 = 24;
const STEPS: usize = 5000;

/// xorshift64* (Marsaglia, "Xorshift RNGs", 2003; multiplier from Vigna).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

// The bias towards valid arguments: the generator keeps each kind of object
// in its own class of slots, in every capability space. Slot `18 + 8 * n + c`
// is of class `c`; class 7 is left for moves and receive ranges. Every space
// that a script makes is laid out as the start-up space: a capability for the
// space itself in slot 1, an untyped in slot 16.
const UNTYPED: u32 = 0;
const CSPACE: u32 = 1;
const THREAD: u32 = 2;
const ENDPOINT: u32 = 3;
const REPLY: u32 = 4;
const NOTIFY: u32 = 5;
const OTHER: u32 = 6;
const SPARE: u32 = 7;

/// The slots from here on are left to the scripts.
const SCRIPT_FIRST: u32 = 82;
/// The slots of a capability space that a script makes: two segments.
const CHILD_SLOTS: u32 = 2 * CSPACE_SEGMENT_SLOTS;
/// The links of a chain of copies that starts at depth 2 and ends at 64.
const CHAIN_LINKS: usize = TREE_DEPTH - 2;

const HERE: SlotAddr = SlotAddr::own(START_SLOT_CSPACE);
const MEMORY: SlotAddr = SlotAddr::own(START_SLOT_FIRST);

const KINDS: [(ObjectKind, u32, u64); 9] = [
    (ObjectKind::Untyped, UNTYPED, 1 << 16),
    (ObjectKind::CSpace, CSPACE, 256),
    (ObjectKind::Thread, THREAD, 0),
    (ObjectKind::Endpoint, ENDPOINT, 0),
    (ObjectKind::Reply, REPLY, 0),
    (ObjectKind::Notify, NOTIFY, 0),
    (ObjectKind::Frame, OTHER, 3),
    (ObjectKind::ASpace, OTHER, 0),
    (ObjectKind::Sched, OTHER, 0),
];
/// The kinds of the groups not yet specified.
const NOT_MADE: [ObjectKind; 7] = [
    ObjectKind::Timer,
    ObjectKind::Irq,
    ObjectKind::IoPorts,
    ObjectKind::IoSpace,
    ObjectKind::Task,
    ObjectKind::Core,
    ObjectKind::Debug,
];
const PAGES: [PageSize; 5] = [
    PageSize::Size4K,
    PageSize::Size4K,
    PageSize::Size4K,
    PageSize::Size2M,
    PageSize::Size1G,
];

/// The kinds that the model makes, as `Census::dying` names them.
const MADE: [ObjectKind; 9] = [
    ObjectKind::Untyped,
    ObjectKind::CSpace,
    ObjectKind::ASpace,
    ObjectKind::Frame,
    ObjectKind::Thread,
    ObjectKind::Sched,
    ObjectKind::Endpoint,
    ObjectKind::Reply,
    ObjectKind::Notify,
];

/// The calls, numbered as `name_of` numbers them.
const NAMES: [&str; 19] = [
    "cap_derive",
    "cap_move",
    "cap_delete",
    "cap_revoke",
    "cap_inspect",
    "untyped_retype",
    "cspace_grow",
    "thread_configure",
    "thread_resume",
    "thread_suspend",
    "call",
    "recv",
    "reply_recv",
    "send",
    "signal",
    "wait",
    "poll",
    "notify_bind",
    "other",
];
/// `call`, `recv`, `reply_recv` and `wait`.
const BLOCKING: [usize; 4] = [10, 11, 12, 15];

fn name_of(call: &Syscall) -> usize {
    match call {
        Syscall::CapDerive { .. } => 0,
        Syscall::CapMove { .. } => 1,
        Syscall::CapDelete(_) => 2,
        Syscall::CapRevoke(_) => 3,
        Syscall::CapInspect(_) => 4,
        Syscall::UntypedRetype { .. } => 5,
        Syscall::CSpaceGrow { .. } => 6,
        Syscall::ThreadConfigure { .. } => 7,
        Syscall::ThreadResume(_) => 8,
        Syscall::ThreadSuspend(_) => 9,
        Syscall::Call { .. } => 10,
        Syscall::Recv { .. } => 11,
        Syscall::ReplyRecv { .. } => 12,
        Syscall::Send { .. } => 13,
        Syscall::Signal(_) => 14,
        Syscall::Wait(_) => 15,
        Syscall::Poll(_) => 16,
        Syscall::NotifyBind { .. } => 17,
        Syscall::Other(_) => 18,
    }
}

fn own(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

fn derive(source: SlotAddr, destination: SlotAddr) -> Syscall {
    Syscall::CapDerive {
        source,
        destination,
        rights: Rights::ALL,
        badge: 0,
    }
}

fn retype(kind: ObjectKind, size: u64, destination: SlotAddr) -> Syscall {
    Syscall::UntypedRetype {
        untyped: MEMORY,
        kind,
        size,
        page: PageSize::Size4K,
        destination,
    }
}

/// A chain of copies in one capability space: the slot of its first link,
/// the slot of its deepest, and the number of its links.
#[derive(Clone, Copy)]
struct Chain {
    first: u32,
    last: u32,
    links: usize,
}

/// What the generator remembers from one call to the next.
#[derive(Default)]
struct Plan {
    /// The calls that a thread still has to make to finish what it began.
    scripts: BTreeMap<ObjId, VecDeque<Syscall>>,
    /// The chain of copies that grows in each capability space.
    chains: BTreeMap<ObjId, Chain>,
    /// The slots of each capability space that a script filled, to empty
    /// them once what they held is removed.
    used: BTreeMap<ObjId, BTreeSet<u32>>,
}

enum Wanted {
    Full,
    Free,
}

/// The generator of the calls of one thread. It looks at the state only to
/// prefer a slot that is full, or one that is not, and to see what a
/// capability is before it builds on it.
struct Generator<'a> {
    rng: &'a mut Rng,
    model: &'a Model,
    thread: ObjId,
    /// The capability space of the thread, by the capability in its slot 1.
    space: Option<ObjId>,
    /// The start-up space.
    home: ObjId,
}

impl Generator<'_> {
    fn full(&self, slot: SlotAddr) -> bool {
        self.model.object_at(self.thread, slot).is_some()
    }

    /// The kind and the rights of the capability in a slot, asked of a copy
    /// of the state.
    fn inspect(&self, slot: SlotAddr) -> Option<(ObjectKind, Rights)> {
        match self
            .model
            .clone()
            .apply(self.thread, Syscall::CapInspect(slot))
        {
            Ok(Output::Inspect { kind, rights, .. }) => Some((kind, rights)),
            _ => None,
        }
    }

    /// Whether a slot holds a capability with every right.
    fn whole(&self, slot: SlotAddr) -> bool {
        self.inspect(slot)
            .is_some_and(|(_, rights)| rights.contains(Rights::ALL.without(Rights::EXEC)))
    }

    fn class_slot(&mut self, class: u32) -> u32 {
        18 + 8 * self.rng.below(8) as u32 + class
    }

    /// A slot of a class, in the caller's capability space or through
    /// another, and now and then any address at all.
    fn slot(&mut self, class: u32) -> SlotAddr {
        let index = self.class_slot(class);
        match self.rng.below(100) {
            0 => SlotAddr::from_raw(self.rng.next()),
            1 => own(self.rng.below(140) as u32),
            2..12 => SlotAddr::through(self.class_slot(CSPACE), index),
            _ => own(index),
        }
    }

    /// A slot of a class, after a few tries for one that holds a capability
    /// (`Wanted::Full`) or for one that does not.
    fn find(&mut self, class: u32, wanted: Wanted) -> SlotAddr {
        for _ in 0..6 {
            let slot = self.slot(class);
            if self.full(slot) == matches!(wanted, Wanted::Full) {
                return slot;
            }
        }
        self.slot(class)
    }

    /// A slot of a class in the caller's own space, full or free as wanted,
    /// if a few tries find one.
    fn find_own(&mut self, class: u32, wanted: Wanted) -> Option<u32> {
        let wanted = matches!(wanted, Wanted::Full);
        let tries: Vec<u32> = (0..8).map(|_| self.class_slot(class)).collect();
        tries
            .into_iter()
            .find(|&index| self.full(own(index)) == wanted)
    }

    /// A capability of a class: often one of those in the fixed slots.
    fn held(&mut self, class: u32) -> SlotAddr {
        let fixed = match class {
            UNTYPED => START_SLOT_FIRST + self.rng.chance(10) as u32,
            CSPACE => START_SLOT_CSPACE,
            _ => return self.find(class, Wanted::Full),
        };
        if self.rng.chance(40) {
            own(fixed)
        } else {
            self.find(class, Wanted::Full)
        }
    }

    fn free(&mut self, class: u32) -> SlotAddr {
        self.find(class, Wanted::Free)
    }

    fn rights(&mut self) -> Rights {
        if self.rng.chance(75) {
            Rights::ALL
        } else {
            Rights::from_bits(self.rng.below(256) as u32).unwrap()
        }
    }

    /// The untyped to take memory from: mostly the one in slot 16, so that a
    /// capability space is often grown from the untyped it was created from.
    fn memory(&mut self) -> SlotAddr {
        if self.rng.chance(60) {
            MEMORY
        } else {
            self.held(UNTYPED)
        }
    }

    /// A slot of a class in another capability space, or in the caller's
    /// own through its capability for it.
    fn inside(&mut self, class: u32) -> SlotAddr {
        let via = if self.rng.chance(10) {
            START_SLOT_CSPACE
        } else {
            self.find(CSPACE, Wanted::Full).index()
        };
        SlotAddr::through(via, self.class_slot(class))
    }

    /// The source of a move that the depths can refuse: an untyped
    /// capability or a cspace capability, here or in another space. Seldom
    /// one in a fixed slot: once moved, the rest of the run finds little to
    /// do.
    fn bound(&mut self) -> SlotAddr {
        let class = if self.rng.chance(50) { UNTYPED } else { CSPACE };
        match self.rng.below(100) {
            0..30 => self.inside(class),
            30..33 => self.held(class),
            _ => self.find(class, Wanted::Full),
        }
    }

    fn message(&mut self) -> Message {
        let mut caps = Vec::new();
        while self.rng.chance(30) {
            let class = self.rng.below(8) as u32;
            caps.push(match self.rng.below(100) {
                0..25 => Transfer::Move(self.bound()),
                25..55 => Transfer::Move(self.held(class)),
                _ => Transfer::Derive(self.held(class), self.rights()),
            });
        }
        // Mostly within the registers; up to the limit; seldom beyond it.
        let words = match self.rng.below(100) {
            0 => MAX_WORDS as u64 + 1,
            1..20 => 4 + self.rng.below(MAX_WORDS as u64 - 3),
            _ => self.rng.below(4),
        };
        Message {
            label: self.rng.next() as u32,
            flags: (self.rng.below(200) == 0) as u32,
            words: (0..words).collect(),
            caps,
        }
    }

    fn range(&mut self) -> Range {
        let first = if self.rng.chance(30) {
            self.inside(SPARE)
        } else {
            self.free(SPARE)
        };
        Range {
            first,
            count: self.rng.below(6) as u32,
        }
    }

    /// One call with random arguments.
    fn call(&mut self) -> Syscall {
        let class = self.rng.below(8) as u32;
        let near = if self.rng.chance(80) { class } else { SPARE };
        let answered = if self.rng.chance(50) { ENDPOINT } else { REPLY };
        let none = |rng: &mut Rng| rng.chance(60);
        match self.rng.below(38) {
            0..3 => Syscall::CapDerive {
                source: self.held(class),
                destination: self.free(near),
                rights: self.rights(),
                badge: match self.rng.below(10) {
                    0..7 => 0,
                    7 | 8 => self.rng.below(8),
                    _ => 1 << self.rng.below(64),
                },
            },
            3 => Syscall::CapMove {
                source: self.find(class, Wanted::Full),
                destination: self.free(near),
            },
            // Rarely a capability in a fixed slot: that ends most of the run.
            4 if self.rng.below(300) == 0 => Syscall::CapDelete(own(self.rng.below(18) as u32)),
            4 => Syscall::CapDelete(self.slot(class)),
            5 if self.rng.chance(2) => Syscall::CapRevoke(self.held(UNTYPED)),
            5 => Syscall::CapRevoke(self.find(class, Wanted::Full)),
            6 => Syscall::CapInspect(self.held(class)),
            7..12 => {
                let (mut kind, class, size) = self.rng.pick(&KINDS);
                if self.rng.chance(3) {
                    kind = self.rng.pick(&NOT_MADE);
                }
                Syscall::UntypedRetype {
                    untyped: self.memory(),
                    kind,
                    size: if self.rng.chance(95) {
                        size
                    } else {
                        self.rng.below(300)
                    },
                    page: self.rng.pick(&PAGES),
                    destination: self.free(class),
                }
            }
            12 | 13 => Syscall::CSpaceGrow {
                cspace: self.find(CSPACE, Wanted::Full),
                untyped: self.memory(),
            },
            14 => Syscall::ThreadConfigure {
                thread: self.held(THREAD),
                cspace: self.held(CSPACE),
                aspace: if none(self.rng) {
                    SlotAddr::NONE
                } else {
                    self.held(OTHER)
                },
                fault: if none(self.rng) {
                    SlotAddr::NONE
                } else {
                    self.held(ENDPOINT)
                },
            },
            15 | 16 => Syscall::ThreadResume(self.held(THREAD)),
            17 if self.rng.chance(30) => Syscall::ThreadSuspend(self.held(THREAD)),
            17..20 => Syscall::Call {
                endpoint: self.held(ENDPOINT),
                message: self.message(),
                range: self.range(),
            },
            20 | 21 => Syscall::Recv {
                endpoint: self.held(ENDPOINT),
                reply: self.held(REPLY),
                range: self.range(),
            },
            22 | 23 => Syscall::ReplyRecv {
                reply: self.held(REPLY),
                message: self.message(),
                endpoint: self.held(ENDPOINT),
                range: self.range(),
            },
            24 | 25 => Syscall::Send {
                target: self.held(answered),
                message: self.message(),
            },
            26 => Syscall::Signal(self.held(NOTIFY)),
            27 => Syscall::Wait(self.held(NOTIFY)),
            28 => Syscall::Poll(self.held(NOTIFY)),
            // A start-up untyped, the least deep, into another space.
            29 if self.rng.chance(40) => Syscall::CapMove {
                source: own(START_SLOT_FIRST + self.rng.chance(30) as u32),
                destination: {
                    let via = self.find(CSPACE, Wanted::Full).index();
                    SlotAddr::through(via, self.class_slot(UNTYPED))
                },
            },
            // The moves that the depths can refuse, both ways.
            29..32 => Syscall::CapMove {
                source: self.bound(),
                destination: if self.rng.chance(30) {
                    self.free(near)
                } else {
                    self.inside(near)
                },
            },
            // The same by IPC: a message that moves one, and a receiver whose
            // range is in another capability space.
            32..35 => Syscall::Send {
                target: self.held(ENDPOINT),
                message: Message {
                    caps: vec![Transfer::Move(self.bound())],
                    ..Message::default()
                },
            },
            35..37 => Syscall::Recv {
                endpoint: self.held(ENDPOINT),
                reply: self.held(REPLY),
                range: Range {
                    first: self.inside(SPARE),
                    count: 1 + self.rng.below(4) as u32,
                },
            },
            _ if self.rng.chance(80) => Syscall::NotifyBind {
                notify: self.held(NOTIFY),
                endpoint: if self.rng.chance(20) {
                    SlotAddr::NONE
                } else {
                    self.held(ENDPOINT)
                },
            },
            _ => Syscall::Other(self.rng.pick(&[Call::ThreadReadRegs, Call::Yield])),
        }
    }

    // The scripts. Each returns the calls that the thread makes over the
    // next steps, or none when its capability space does not lend itself.

    /// Free slots of the caller's own space among those left to the scripts.
    fn spare(&mut self, plan: &mut Plan, wanted: usize) -> Vec<u32> {
        let Some(space) = self.space else {
            return Vec::new();
        };
        let table = if space == self.home {
            CSPACE_SEGMENT_SLOTS
        } else {
            CHILD_SLOTS
        };
        let free = (SCRIPT_FIRST..table).filter(|&index| !self.full(own(index)));
        let slots: Vec<u32> = free.take(wanted).collect();
        plan.used.entry(space).or_default().extend(&slots);
        slots
    }

    /// Lengthens the chain of copies of this space, down to depth 64, or
    /// begins one from a capability of any class; and removes a chain that
    /// is complete, link by link from its first.
    fn chain(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let Some(space) = self.space else {
            return Vec::new();
        };
        let grown = plan
            .chains
            .get(&space)
            .copied()
            .filter(|chain| self.full(own(chain.last)));
        if let Some(chain) = grown.filter(|chain| chain.links >= CHAIN_LINKS) {
            plan.chains.remove(&space);
            return vec![Syscall::CapDelete(own(chain.first)); chain.links / REMOVAL_STEP + 2];
        }
        let class = self.rng.below(7) as u32;
        let begun = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)));
        let (mut from, first, links) = match (grown, begun) {
            (Some(chain), _) => (chain.last, Some(chain.first), chain.links),
            (None, Some(index)) => (index, None, 0),
            (None, None) => return Vec::new(),
        };
        let wanted = (CHAIN_LINKS - links).min(10 + self.rng.below(30) as usize);
        let slots = self.spare(plan, wanted);
        let (Some(&start), Some(&last)) = (slots.first(), slots.last()) else {
            return Vec::new();
        };
        let chain = Chain {
            first: first.unwrap_or(start),
            last,
            links: links + slots.len(),
        };
        plan.chains.insert(space, chain);
        let link = |&to: &u32| derive(own(std::mem::replace(&mut from, to)), own(to));
        slots.iter().map(link).collect()
    }

    /// 33 copies or more of a capability of any class, then its removal by
    /// `cap_delete` or `cap_revoke`, which takes several calls, then the
    /// slots of the copies emptied.
    fn fan(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let classes = [
            UNTYPED, CSPACE, THREAD, THREAD, ENDPOINT, ENDPOINT, REPLY, REPLY,
        ];
        let class = if self.rng.chance(50) {
            self.rng.pick(&classes)
        } else {
            self.rng.pick(&[NOTIFY, OTHER])
        };
        let Some(source) = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)))
        else {
            return Vec::new();
        };
        let wanted = REMOVAL_STEP + 1 + self.rng.below(6) as usize;
        let slots = self.spare(plan, wanted);
        if slots.len() < wanted {
            return Vec::new();
        }
        let removal = if self.rng.chance(60) {
            Syscall::CapDelete(own(source))
        } else {
            Syscall::CapRevoke(own(source))
        };
        let copies = slots.iter().map(|&to| derive(own(source), own(to)));
        let emptied = slots.iter().map(|&slot| Syscall::CapDelete(own(slot)));
        copies.chain(vec![removal; 3]).chain(emptied).collect()
    }

    /// A process: a capability space made from the untyped in slot 16, laid
    /// out as this one, with endpoints, reply objects and notifies of its
    /// own and copies of some of this space's, and a thread that runs in it.
    fn spawn(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let untyped = self
            .inspect(MEMORY)
            .is_some_and(|(kind, _)| kind == ObjectKind::Untyped);
        let classes = (
            self.find_own(CSPACE, Wanted::Free),
            self.find_own(THREAD, Wanted::Free),
        );
        let (Some(space), Some(thread)) = classes else {
            return Vec::new();
        };
        if !untyped || !self.whole(MEMORY) || self.model.runnable().len() > 12 {
            return Vec::new();
        }
        let _ = plan;
        let inside = |index: u32| SlotAddr::through(space, index);
        let grown = Syscall::CSpaceGrow {
            cspace: own(space),
            untyped: MEMORY,
        };
        let mut calls = vec![
            retype(ObjectKind::CSpace, CHILD_SLOTS.into(), own(space)),
            grown.clone(),
            grown,
            derive(MEMORY, inside(START_SLOT_FIRST)),
            derive(own(space), inside(START_SLOT_CSPACE)),
        ];
        for (kind, class) in [
            (ObjectKind::Endpoint, ENDPOINT),
            (ObjectKind::Reply, REPLY),
            (ObjectKind::Notify, NOTIFY),
            (ObjectKind::Frame, OTHER),
        ] {
            let size = (kind == ObjectKind::Frame) as u64;
            calls.push(retype(kind, size, inside(18 + class)));
            calls.push(retype(kind, size, inside(26 + class)));
            // A capability of this space, shared with the new one.
            if let Some(shared) = self.find_own(class, Wanted::Full) {
                calls.push(derive(own(shared), inside(34 + class)));
            }
        }
        calls.push(retype(ObjectKind::Thread, 0, own(thread)));
        calls.push(Syscall::ThreadConfigure {
            thread: own(thread),
            cspace: own(space),
            aspace: SlotAddr::NONE,
            fault: SlotAddr::NONE,
        });
        calls.push(Syscall::ThreadResume(own(thread)));
        calls
    }

    /// A nest of spaces, one at each depth, each holding in its slot 2 a
    /// copy of the untyped at its own depth, whose most recent child is the
    /// root capability of the next space; in the deepest, a chain of copies
    /// of an ordinary capability. Then its removal from the first space,
    /// whose walk is as long as the nest is deep, and the slots emptied.
    fn nest(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        if !self.whole(MEMORY) {
            return Vec::new();
        }
        let wanted = 24 + self.rng.below(22) as usize;
        let slots = self.spare(plan, wanted);
        let [held, roots @ ..] = slots.as_slice() else {
            return Vec::new();
        };
        if roots.len() < 12 {
            return Vec::new();
        }
        let mut calls = Vec::new();
        let mut untyped = MEMORY;
        for &root in roots {
            calls.push(derive(untyped, own(*held)));
            calls.push(Syscall::UntypedRetype {
                untyped,
                kind: ObjectKind::CSpace,
                size: CSPACE_SEGMENT_SLOTS.into(),
                page: PageSize::Size4K,
                destination: own(root),
            });
            calls.push(Syscall::CSpaceGrow {
                cspace: own(root),
                untyped: MEMORY,
            });
            untyped = SlotAddr::through(root, 2);
            calls.push(Syscall::CapMove {
                source: own(*held),
                destination: untyped,
            });
        }
        let deepest = |index: u32| SlotAddr::through(roots[roots.len() - 1], index);
        let class = self.rng.pick(&[ENDPOINT, NOTIFY, REPLY]);
        if let Some(ordinary) = self
            .find_own(class, Wanted::Full)
            .filter(|&index| self.whole(own(index)))
        {
            calls.push(derive(own(ordinary), deepest(1)));
            calls.push(derive(deepest(1), deepest(3)));
            calls.extend((3..40).map(|index| derive(deepest(index), deepest(index + 1))));
        }
        // One call for each space to sweep, and a few for the chain.
        calls.extend(vec![Syscall::CapDelete(own(roots[0])); roots.len() + 6]);
        calls.extend(slots.iter().map(|&slot| Syscall::CapDelete(own(slot))));
        calls
    }

    /// Empties the slots that scripts filled and that are no longer full.
    fn tidy(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        let Some(used) = self.space.and_then(|space| plan.used.get_mut(&space)) else {
            return Vec::new();
        };
        let stale: Vec<u32> = used
            .iter()
            .copied()
            .filter(|&slot| !self.full(own(slot)))
            .take(40)
            .collect();
        used.retain(|slot| !stale.contains(slot));
        stale
            .into_iter()
            .map(|slot| Syscall::CapDelete(own(slot)))
            .collect()
    }

    fn script(&mut self, plan: &mut Plan) -> Vec<Syscall> {
        match self.rng.below(100) {
            0..20 => self.spawn(plan),
            20..50 => self.chain(plan),
            50..75 => self.fan(plan),
            75..88 => self.nest(plan),
            _ => self.tidy(plan),
        }
    }
}

/// What a run reached.
#[derive(Debug, Default, PartialEq)]
struct Reached {
    /// Calls that did not fail, by their place in `NAMES`.
    ok: [u32; 19],
    /// Calls that returned each error, by its code.
    errors: [u32; 11],
    /// Times nothing of use could run, and the run went on from the
    /// start-up state.
    restarts: u32,
    /// Steps at which several threads could run.
    several_threads: u32,
    /// Calls made by threads whose capability space is not the start-up
    /// space, and those of them that did not fail.
    outside: u32,
    outside_ok: u32,
    /// Calls that a script made, and those of them that did not fail.
    scripted: u32,
    scripted_ok: u32,
    /// Messages received with capabilities, with the flag `caps_dropped`,
    /// and with the flag `notification`; each counted once, when received.
    with_caps: u32,
    dropped: u32,
    notifications: u32,
    /// Threads woken with `gone`, each counted once.
    woken_gone: u32,
    /// The deepest capability, and the longest walk of a removal.
    deepest: usize,
    longest_walk: usize,
    /// Steps after which an object of each kind was dying, by kind.
    dying: [u32; 16],
    /// Steps after which a capability under `cap_delete` was dying while its
    /// object was not.
    dying_copies: u32,
    /// Steps after which a thread waited to be received, waited for a reply
    /// through an armed reply object, waited to receive, waited on a notify;
    /// and after which a notify was bound.
    callers: u32,
    served: u32,
    receivers: u32,
    waiters: u32,
    bound: u32,
    /// `cap_move` of an untyped capability (first) or of a cspace
    /// capability (second) through a cspace capability: those accepted, and
    /// those refused only because of the depths.
    moved: [u32; 2],
    refused: [u32; 2],
    /// `send` that lists such a move and found a receiver: those delivered
    /// with their capabilities, and those dropped only because of the
    /// depths.
    sent: u32,
    sent_refused: u32,
}

impl Reached {
    fn note(&mut self, outcome: &Outcome) {
        self.woken_gone += (*outcome == Err(Error::Gone)) as u32;
        if let Ok(Output::Message(delivery)) = outcome {
            let header = delivery.received.header;
            self.with_caps += (header.caps() > 0) as u32;
            self.dropped += (header.flags() & Header::FLAG_CAPS_DROPPED != 0) as u32;
            self.notifications += (header.flags() & Header::FLAG_NOTIFICATION != 0) as u32;
        }
    }

    fn count(&mut self, model: &Model) {
        let census = model.census();
        self.deepest = self.deepest.max(census.deepest);
        self.longest_walk = self.longest_walk.max(model.longest_walk());
        let mut kinds = census.dying;
        kinds.dedup();
        kinds
            .iter()
            .for_each(|&kind| self.dying[kind as usize] += 1);
        self.dying_copies += (census.dying_copies > 0) as u32;
        self.callers += (census.callers > 0) as u32;
        self.served += (census.served > 0) as u32;
        self.receivers += (census.receivers > 0) as u32;
        self.waiters += (census.waiters > 0) as u32;
        self.bound += (census.bound > 0) as u32;
    }
}

/// What `at_risk` found about a call before it was made.
struct Risk {
    /// 0 for an untyped capability, 1 for a cspace capability.
    kind: usize,
    /// The call succeeds with a capability that every space can hold in
    /// its place: a copy of the thread capability in its start-up slot.
    fits_derived: bool,
}

/// Whether a call moves an untyped or a cspace capability, by `cap_move`
/// through a cspace capability or by `send`. It asks copies of the state.
fn at_risk(model: &Model, thread: ObjId, call: &Syscall) -> Option<Risk> {
    let on_copy = |call: Syscall| model.clone().apply(thread, call);
    let kind = |slot: SlotAddr| match on_copy(Syscall::CapInspect(slot)) {
        Ok(Output::Inspect {
            kind: ObjectKind::Untyped,
            ..
        }) => Some(0),
        Ok(Output::Inspect {
            kind: ObjectKind::CSpace,
            ..
        }) => Some(1),
        _ => None,
    };
    let ordinary = own(START_SLOT_THREAD);
    match call {
        &Syscall::CapMove {
            source,
            destination,
        } if source.via() | destination.via() != 0 => {
            let derived = Syscall::CapDerive {
                source: ordinary,
                destination,
                rights: Rights::NONE,
                badge: 0,
            };
            Some(Risk {
                kind: kind(source)?,
                fits_derived: on_copy(derived).is_ok(),
            })
        }
        Syscall::Send { target, message } => {
            let mut derived = message.clone();
            let mut found = None;
            for transfer in &mut derived.caps {
                if let Transfer::Move(slot) = *transfer {
                    found = found.or(kind(slot));
                    *transfer = Transfer::Derive(ordinary, Rights::NONE);
                }
            }
            let derived = Syscall::Send {
                target: *target,
                message: derived,
            };
            let fits_derived = on_copy(derived) == Ok(Output::Sent { flags: 0 });
            Some(Risk {
                kind: found?,
                fits_derived,
            })
        }
        _ => None,
    }
}

fn run(seed: u64, reached: &mut Reached) -> Model {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (mut model, first) = Model::boot();
    let home = model.object_at(first, HERE).expect("the start-up space");
    let mut plan = Plan::default();
    // Every thread that could run at some step: those that can be woken.
    let mut known: BTreeSet<ObjId> = BTreeSet::new();
    for step in 0..STEPS {
        let threads = model.runnable();
        known.extend(&threads);
        // A thread is of use if its capability space holds a capability for
        // itself in slot 1, as the start-up space and those of the scripts
        // do: one that is bound to an empty or a dying space only gets
        // errors.
        let space_of = |thread: &ObjId| model.object_at(*thread, HERE);
        let of_use: Vec<ObjId> = threads
            .iter()
            .copied()
            .filter(|thread| space_of(thread).is_some())
            .collect();
        if of_use.is_empty() {
            // Nothing of use can run: go on from the start-up state.
            reached.restarts += 1;
            (model, _) = Model::boot();
            (plan, known) = (Plan::default(), BTreeSet::new());
            continue;
        }
        reached.several_threads += (threads.len() > 1) as u32;
        // A thread in the middle of a script is picked more often.
        let scripted: Vec<ObjId> = of_use
            .iter()
            .copied()
            .filter(|thread| plan.scripts.contains_key(thread))
            .collect();
        let thread = match rng.below(100) {
            0..50 if !scripted.is_empty() => rng.pick(&scripted),
            0..95 => rng.pick(&of_use),
            _ => rng.pick(&threads),
        };
        let space = space_of(&thread);
        let mut generator = Generator {
            rng: &mut rng,
            model: &model,
            thread,
            space,
            home,
        };
        // The next call of the script of the thread; or the first call of a
        // new script; or one call with random arguments.
        let mut next = None;
        if generator.rng.chance(92) {
            next = plan.scripts.get_mut(&thread).and_then(VecDeque::pop_front);
        } else if !plan.scripts.contains_key(&thread)
            && generator
                .rng
                .chance(if space == Some(home) { 15 } else { 40 })
        {
            let mut script: VecDeque<Syscall> = generator.script(&mut plan).into();
            next = script.pop_front();
            plan.scripts.insert(thread, script);
        }
        if plan.scripts.get(&thread).is_some_and(VecDeque::is_empty) {
            plan.scripts.remove(&thread);
        }
        let from_script = next.is_some();
        let call = next.unwrap_or_else(|| {
            let mut call = generator.call();
            // The last thread of use seldom makes a call that waits.
            while of_use.len() == 1
                && BLOCKING.contains(&name_of(&call))
                && generator.rng.chance(98)
            {
                call = generator.call();
            }
            call
        });
        let name = name_of(&call);
        let risk = at_risk(&model, thread, &call);
        let moves = matches!(call, Syscall::CapMove { .. });
        let had_outcome: BTreeSet<ObjId> = known
            .iter()
            .copied()
            .filter(|thread| model.woken(*thread).is_some())
            .collect();

        let outcome = model.apply(thread, call);
        if let Err(broken) = model.check() {
            panic!("seed {seed}, step {step}, {}: {broken}", NAMES[name]);
        }

        match (risk, &outcome) {
            (Some(risk), Ok(Output::Done)) => reached.moved[risk.kind] += 1,
            (Some(risk), Err(Error::Invalid)) if moves && risk.fits_derived => {
                reached.refused[risk.kind] += 1
            }
            (Some(_), Ok(Output::Sent { flags: 0 })) => reached.sent += 1,
            (Some(risk), Ok(Output::Sent { .. })) if risk.fits_derived => reached.sent_refused += 1,
            _ => {}
        }
        match &outcome {
            Ok(_) => reached.ok[name] += 1,
            Err(error) => reached.errors[*error as usize] += 1,
        }
        reached.outside += (space != Some(home)) as u32;
        reached.outside_ok += (space != Some(home) && outcome.is_ok()) as u32;
        reached.scripted += from_script as u32;
        reached.scripted_ok += (from_script && outcome.is_ok()) as u32;
        // What the call returned, and what it woke: each outcome once.
        if outcome != Err(Error::Gone) {
            reached.note(&outcome);
        }
        let woken = known.iter().filter(|thread| !had_outcome.contains(thread));
        woken
            .filter_map(|&thread| model.woken(thread))
            .for_each(|outcome| reached.note(outcome));
        reached.count(&model);
    }
    model
}

/// Every seed, run once for the tests below. A run panics if an invariant
/// fails.
fn all_seeds() -> &'static Reached {
    static REACHED: OnceLock<Reached> = OnceLock::new();
    REACHED.get_or_init(|| {
        let mut reached = Reached::default();
        for seed in 0..SEEDS {
            run(seed, &mut reached);
        }
        println!("{reached:#?}");
        reached
    })
}

#[test]
fn invariants_hold_after_every_call_of_random_sequences() {
    let reached = all_seeds();
    // "other" stands for the calls that are not modelled: it never succeeds.
    for (name, &ok) in NAMES.iter().zip(&reached.ok) {
        assert_eq!(ok == 0, *name == "other", "{name}");
    }
    for (code, &count) in reached.errors.iter().enumerate().skip(1) {
        let never = code == Error::Budget as usize;
        assert_eq!(count == 0, never, "error code {code}");
    }
    assert!(reached.several_threads as usize > STEPS);
}

// The coverage of the 24 seeds. The floors are well under what was measured
// (in the comments), so that a change of the generator that loses a kind of
// state fails here.

#[test]
fn deepest_capability_and_longest_walk_are_reached() {
    let reached = all_seeds();
    // Measured: depth 64, a walk of 124 capabilities.
    assert_eq!(reached.deepest, TREE_DEPTH);
    assert!(reached.longest_walk >= 100, "{}", reached.longest_walk);
    assert!(reached.longest_walk < LONGEST_WALK);
}

#[test]
fn objects_of_every_kind_are_seen_dying() {
    let reached = all_seeds();
    // Measured, in steps: untyped 7,884, cspace 24,264, aspace 3, frame 9,
    // thread 2,459, sched 3, endpoint 344, reply 9, notify 460; a copy
    // under `cap_delete` 3,856.
    for kind in MADE {
        assert!(reached.dying[kind as usize] > 0, "{kind:?}");
    }
    assert!(reached.dying_copies >= 1000, "{}", reached.dying_copies);
}

#[test]
fn threads_outside_the_start_up_space_mostly_succeed() {
    let reached = all_seeds();
    // Measured: 13,787 of 21,501 calls.
    assert!(reached.outside >= 10_000, "{}", reached.outside);
    assert!(
        2 * reached.outside_ok > reached.outside,
        "{}",
        reached.outside_ok
    );
}

#[test]
fn waits_of_every_sort_and_their_ends_are_reached() {
    let reached = all_seeds();
    // Measured, in steps: callers queued 34,721, reply objects armed 7,285,
    // receivers 30,660, waiters on a notify 20,564, a notify bound 55,970.
    let waits = [
        reached.callers,
        reached.served,
        reached.receivers,
        reached.waiters,
        reached.bound,
    ];
    assert!(waits.iter().all(|&steps| steps >= 2000), "{waits:?}");
    // Measured, each once: 35 messages with capabilities, 65 with the
    // capabilities dropped, 52 notifications, 33 threads woken with `gone`.
    let ends = [
        reached.with_caps,
        reached.dropped,
        reached.notifications,
        reached.woken_gone,
    ];
    assert!(ends.iter().all(|&times| times >= 15), "{ends:?}");
}

/// The invariant "removal ends" is checked after every call above. It says
/// something only if the moves that could break it are tried: here they are
/// counted, accepted and refused, for both kinds and both ways of moving.
#[test]
fn moves_that_the_depths_rule_on_are_accepted_and_refused() {
    let reached = all_seeds();
    // Measured: untyped 2,682 accepted and 202 refused, cspace 320 and 127;
    // by `send` 28 delivered and 6 refused.
    for count in reached.moved.into_iter().chain(reached.refused) {
        assert!(count >= 50, "{reached:?}");
    }
    assert!(
        reached.sent >= 10 && reached.sent_refused >= 3,
        "{reached:?}"
    );
}

#[test]
fn same_seed_gives_the_same_state() {
    for seed in 0..2 {
        let (mut first, mut second) = (Reached::default(), Reached::default());
        assert_eq!(run(seed, &mut first), run(seed, &mut second));
        assert_eq!(first, second);
    }
}

#[test]
fn different_seeds_give_different_states() {
    let mut reached = Reached::default();
    assert_ne!(run(1, &mut reached), run(2, &mut reached));
}
