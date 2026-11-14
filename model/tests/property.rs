//! Random call sequences: every invariant holds after every call, a seed
//! always gives the same state, and the generator reaches every modelled call.

use dodos_abi::*;
use dodos_model::*;

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

    fn message(&mut self) -> Message {
        let mut caps = Vec::new();
        while self.rng.chance(25) {
            let class = self.rng.below(8) as u32;
            let source = self.held(class);
            caps.push(if self.rng.chance(50) {
                Transfer::Move(source)
            } else {
                Transfer::Derive(source, self.rights())
            });
        }
        let words = if self.rng.chance(1) {
            65
        } else {
            self.rng.below(4)
        };
        Message {
            label: self.rng.next() as u32,
            words: (0..words).collect(),
            caps,
        }
    }

    fn range(&mut self) -> Range {
        Range {
            first: self.free(SPARE),
            count: self.rng.below(6) as u32,
        }
    }

    /// One call, and its place in `NAMES`.
    fn call(&mut self) -> (usize, Syscall) {
        let class = self.rng.below(8) as u32;
        let near = if self.rng.chance(80) { class } else { SPARE };
        let answered = if self.rng.chance(50) { ENDPOINT } else { REPLY };
        let none = |rng: &mut Rng| rng.chance(60);
        let call = match self.rng.below(30) {
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
                    untyped: self.held(UNTYPED),
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
            12 => Syscall::CSpaceGrow {
                cspace: self.held(CSPACE),
                untyped: self.held(UNTYPED),
            },
            13 | 14 => Syscall::ThreadConfigure {
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
        let outcome = model.apply(thread, call);
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

#[test]
fn invariants_hold_after_every_call_of_random_sequences() {
    let mut reached = Reached::default();
    for seed in 0..SEEDS {
        run(seed, &mut reached);
    }
    println!("{reached:#?}");
    // "other" stands for the calls that are not modelled: it never succeeds.
    for (name, &ok) in NAMES.iter().zip(&reached.ok) {
        assert_eq!(ok == 0, *name == "other", "{name}");
    }
    for code in Error::Gone as usize..=Error::Unsupported as usize {
        let never = code == Error::Budget as usize;
        assert_eq!(reached.errors[code] == 0, never, "error code {code}");
    }
    assert!(reached.several_threads as usize > STEPS);
    assert!(reached.with_caps > 0 && reached.dropped > 0 && reached.notifications > 0);
    assert!(reached.woken_gone > 0);
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
