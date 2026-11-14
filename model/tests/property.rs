//! Random call sequences: every invariant holds after every call, a seed
//! always gives the same state, and the generator reaches every modelled call.

use dodos_abi::*;
use dodos_model::*;
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
// is of class `c`; class 7 is left for moves and receive ranges.
const UNTYPED: u32 = 0;
const CSPACE: u32 = 1;
const THREAD: u32 = 2;
const ENDPOINT: u32 = 3;
const REPLY: u32 = 4;
const NOTIFY: u32 = 5;
const OTHER: u32 = 6;
const SPARE: u32 = 7;

const KINDS: [(ObjectKind, u32, u64); 10] = [
    (ObjectKind::Untyped, UNTYPED, 1 << 16),
    (ObjectKind::CSpace, CSPACE, 256),
    (ObjectKind::Thread, THREAD, 0),
    (ObjectKind::Endpoint, ENDPOINT, 0),
    (ObjectKind::Reply, REPLY, 0),
    (ObjectKind::Notify, NOTIFY, 0),
    (ObjectKind::Frame, OTHER, 3),
    (ObjectKind::ASpace, OTHER, 0),
    (ObjectKind::Sched, OTHER, 0),
    (ObjectKind::Timer, OTHER, 0),
];

/// The calls, numbered as in `Generator::call`.
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

/// The generator of one call: it looks at the state only to prefer a slot
/// that is full, or one that is not, for the thread that makes the call.
struct Generator<'a> {
    rng: &'a mut Rng,
    model: &'a Model,
    thread: ObjId,
}

impl Generator<'_> {
    /// A slot of a class, in the caller's capability space or through
    /// another, and now and then any address at all.
    fn slot(&mut self, class: u32) -> SlotAddr {
        let index = 18 + 8 * self.rng.below(8) as u32 + class;
        match self.rng.below(100) {
            0 => SlotAddr::from_raw(self.rng.next()),
            1 => SlotAddr::own(self.rng.below(140) as u32),
            2..12 => SlotAddr::through(18 + 8 * self.rng.below(8) as u32 + CSPACE, index),
            _ => SlotAddr::own(index),
        }
    }

    /// A slot of a class, after a few tries for one that holds a capability
    /// (`Wanted::Full`) or for one that does not.
    fn find(&mut self, class: u32, wanted: Wanted) -> SlotAddr {
        for _ in 0..6 {
            let slot = self.slot(class);
            let full = self.model.object_at(self.thread, slot).is_some();
            if full == matches!(wanted, Wanted::Full) {
                return slot;
            }
        }
        self.slot(class)
    }

    /// A capability of a class: often one of the start-up capabilities.
    fn held(&mut self, class: u32) -> SlotAddr {
        let start_up = match class {
            UNTYPED => START_SLOT_FIRST + self.rng.chance(10) as u32,
            CSPACE => START_SLOT_CSPACE,
            _ => return self.find(class, Wanted::Full),
        };
        if self.rng.chance(40) {
            SlotAddr::own(start_up)
        } else {
            self.find(class, Wanted::Full)
        }
    }

    fn free(&mut self, class: u32) -> SlotAddr {
        self.find(class, Wanted::Free)
    }

    fn rights(&mut self) -> Rights {
        if self.rng.chance(60) {
            Rights::ALL
        } else {
            Rights::from_bits(self.rng.below(256) as u32).unwrap()
        }
    }

    /// The untyped to take memory from: mostly the first one, so that a
    /// capability space is often grown from the untyped it was created from.
    fn memory(&mut self) -> SlotAddr {
        if self.rng.chance(60) {
            SlotAddr::own(START_SLOT_FIRST)
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
        SlotAddr::through(via, 18 + 8 * self.rng.below(8) as u32 + class)
    }

    /// The source of a move that the depths can refuse: an
    /// untyped capability or a cspace capability, here or in another space.
    /// Seldom a start-up capability: once moved, the rest of the run finds
    /// little to do.
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
        while self.rng.chance(25) {
            let class = self.rng.below(8) as u32;
            caps.push(match self.rng.below(100) {
                0..35 => Transfer::Move(self.bound()),
                35..60 => Transfer::Move(self.held(class)),
                _ => Transfer::Derive(self.held(class), self.rights()),
            });
        }
        let words = if self.rng.chance(1) {
            65
        } else {
            self.rng.below(4)
        };
        Message {
            label: self.rng.next() as u32,
            flags: (self.rng.below(200) == 0) as u32,
            words: (0..words).collect(),
            caps,
        }
    }

    fn range(&mut self) -> Range {
        Range {
            first: if self.rng.chance(40) {
                self.inside(SPARE)
            } else {
                self.free(SPARE)
            },
            count: self.rng.below(6) as u32,
        }
    }

    /// One call, and its place in `NAMES`.
    fn call(&mut self) -> (usize, Syscall) {
        let class = self.rng.below(8) as u32;
        let near = if self.rng.chance(80) { class } else { SPARE };
        let answered = if self.rng.chance(50) { ENDPOINT } else { REPLY };
        let none = |rng: &mut Rng| rng.chance(60);
        let call = match self.rng.below(38) {
            0..3 => Syscall::CapDerive {
                source: self.held(class),
                destination: self.free(near),
                rights: self.rights(),
                badge: if self.rng.chance(70) {
                    0
                } else {
                    self.rng.below(8)
                },
            },
            3 => Syscall::CapMove {
                source: self.find(class, Wanted::Full),
                destination: self.free(near),
            },
            // Rarely a start-up capability: that ends most of the run.
            4 if self.rng.below(300) == 0 => {
                Syscall::CapDelete(SlotAddr::own(self.rng.below(18) as u32))
            }
            4 => Syscall::CapDelete(self.slot(class)),
            5 if self.rng.chance(2) => Syscall::CapRevoke(self.held(UNTYPED)),
            5 => Syscall::CapRevoke(self.find(class, Wanted::Full)),
            6 => Syscall::CapInspect(self.held(class)),
            7..12 => {
                let (kind, class, size) = self.rng.pick(&KINDS);
                Syscall::UntypedRetype {
                    untyped: self.memory(),
                    kind,
                    size: if self.rng.chance(95) {
                        size
                    } else {
                        self.rng.below(300)
                    },
                    page: PageSize::Size4K,
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
            // The moves that the depths can refuse, both ways.
            // A start-up untyped, the least deep, into another space.
            29 if self.rng.chance(40) => Syscall::CapMove {
                source: SlotAddr::own(START_SLOT_FIRST + self.rng.chance(30) as u32),
                destination: {
                    let via = self.find(CSPACE, Wanted::Full).index();
                    SlotAddr::through(via, 18 + 8 * self.rng.below(8) as u32)
                },
            },
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
                    label: 0,
                    flags: 0,
                    words: Vec::new(),
                    caps: vec![Transfer::Move(self.bound())],
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
        };
        let name = match call {
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
        };
        (name, call)
    }
}

enum Wanted {
    Full,
    Free,
}

/// What a run reached.
#[derive(Debug, Default, PartialEq)]
struct Reached {
    /// Calls that did not fail, by their place in `NAMES`.
    ok: [u32; 19],
    /// Calls that returned each error, by its code.
    errors: [u32; 11],
    /// Times no thread could run, and the run went on from the start-up
    /// state.
    restarts: u32,
    /// Steps at which several threads could run.
    several_threads: u32,
    /// Messages received with capabilities, with the flag `caps_dropped`,
    /// and with the flag `notification`.
    with_caps: u32,
    dropped: u32,
    notifications: u32,
    /// Threads woken with `gone`.
    woken_gone: u32,
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
    match call {
        &Syscall::CapMove {
            source,
            destination,
        } if source.via() | destination.via() != 0 => {
            let derived = Syscall::CapDerive {
                source: SlotAddr::own(START_SLOT_THREAD),
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
                    *transfer = Transfer::Derive(SlotAddr::own(START_SLOT_THREAD), Rights::NONE);
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

impl Reached {
    fn note(&mut self, outcome: &Outcome) {
        if let Ok(Output::Message(delivery)) = outcome {
            let header = delivery.received.header;
            self.with_caps += (header.caps() > 0) as u32;
            self.dropped += (header.flags() & Header::FLAG_CAPS_DROPPED != 0) as u32;
            self.notifications += (header.flags() & Header::FLAG_NOTIFICATION != 0) as u32;
        }
    }
}

fn run(seed: u64, reached: &mut Reached) -> Model {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (mut model, _) = Model::boot();
    for step in 0..STEPS {
        let threads = model.runnable();
        // A thread is of use if its capability space holds something: one
        // that is bound to nothing, or to a destroyed or empty capability
        // space, only gets errors.
        let space = SlotAddr::own(START_SLOT_CSPACE);
        let of_use = |thread: &ObjId| model.object_at(*thread, space).is_some();
        if !threads.iter().any(of_use) {
            // Nothing of use can run: go on from the start-up state.
            reached.restarts += 1;
            (model, _) = Model::boot();
            continue;
        }
        reached.several_threads += (threads.len() > 1) as u32;
        let mut thread = rng.pick(&threads);
        while !of_use(&thread) && rng.chance(80) {
            thread = rng.pick(&threads);
        }
        let mut generator = Generator {
            rng: &mut rng,
            model: &model,
            thread,
        };
        let (mut name, mut call) = generator.call();
        // The last thread of use seldom makes a call that waits.
        let last = threads.iter().filter(|thread| of_use(thread)).count() == 1;
        while last && BLOCKING.contains(&name) && generator.rng.chance(98) {
            (name, call) = generator.call();
        }
        let risk = at_risk(&model, thread, &call);
        let moves = matches!(call, Syscall::CapMove { .. });
        let outcome = model.apply(thread, call);
        match (risk, &outcome) {
            (Some(risk), Ok(Output::Done)) => reached.moved[risk.kind] += 1,
            (Some(risk), Err(Error::Invalid)) if moves && risk.fits_derived => {
                reached.refused[risk.kind] += 1;
            }
            (Some(_), Ok(Output::Sent { flags: 0 })) => reached.sent += 1,
            (Some(risk), Ok(Output::Sent { .. })) if risk.fits_derived => reached.sent_refused += 1,
            _ => {}
        }
        if let Err(broken) = model.check() {
            panic!("seed {seed}, step {step}, {}: {broken}", NAMES[name]);
        }
        match &outcome {
            Ok(_) => reached.ok[name] += 1,
            Err(error) => reached.errors[*error as usize] += 1,
        }
        reached.note(&outcome);
        // Counted at every step until the woken thread makes its next call.
        for woken in model
            .runnable()
            .iter()
            .filter_map(|&thread| model.woken(thread))
        {
            reached.note(woken);
            reached.woken_gone += (*woken == Err(Error::Gone)) as u32;
        }
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
    let codes = Error::Gone as usize..=Error::Unsupported as usize;
    for (code, &count) in codes.clone().zip(&reached.errors[codes]) {
        let never = code == Error::Budget as usize;
        assert_eq!(count == 0, never, "error code {code}");
    }
    assert!(reached.several_threads as usize > STEPS);
    assert!(reached.with_caps > 0 && reached.dropped > 0 && reached.notifications > 0);
    assert!(reached.woken_gone > 0);
}

/// The invariant "removal ends" is checked after every call above. It says
/// something only if the moves that could break it are tried: here they are
/// counted, accepted and refused, for both kinds and both ways of moving.
#[test]
fn moves_that_the_depths_rule_on_are_accepted_and_refused() {
    let reached = all_seeds();
    println!(
        "cap_move: untyped {} accepted, {} refused; cspace {} accepted, {} refused",
        reached.moved[0], reached.refused[0], reached.moved[1], reached.refused[1]
    );
    println!(
        "send: {} delivered, {} refused",
        reached.sent, reached.sent_refused
    );
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
    for seed in 0..4 {
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
