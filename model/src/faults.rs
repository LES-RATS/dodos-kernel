//! Threads: faults, and the register calls.

use crate::{Model, ObjId, Outcome, Output, Parcel, Syscall, Wait, Window, live};
use dodos_abi::x86_64::{
    REG_DR0, REG_DR3, REG_DR7, REG_RFLAGS, REG_RIP, REG_RSP, REG_UINTR, REGS_CANONICAL, REGS_WORDS,
    dr7_valid, rflags,
};
use dodos_abi::{
    Error, FAULT_REPLY_REDIRECT, FAULT_REPLY_RESUME, FaultKind, ObjectKind, Rights, SlotAddr,
};

/// DR6's reset value, the upper 32 bits 0 (Intel SDM §20.2.3).
pub(crate) const DR6_RESET: u64 = 0xFFFF_0FF0;
/// The causes DR6 names: B0 to B3, BD, BS and BT.
const DR6_BD: u64 = 1 << 13;
const DR6_BS: u64 = 1 << 14;
const DR6_BT: u64 = 1 << 15;
pub(crate) const DR6_CAUSES: u64 = 0xF | DR6_BD | DR6_BS | DR6_BT;

impl Model {
    /// A fault of `thread`, which runs: a page fault, another processor
    /// exception or a system call made outside the code page (kinds 1 to
    /// 6). `exception` is words 2 to 4 of the fault message: the exception
    /// number, the access or the error code, and the address.
    ///
    /// The faults *no address space* and *no context* are raised by the
    /// kernel itself, never given here. A fault is an event of the kernel,
    /// as a call is.
    pub fn fault(&mut self, thread: ObjId, kind: FaultKind, exception: [u64; 3]) {
        assert!(self.runnable().contains(&thread), "{thread:?} does not run");
        assert!(
            (kind as u32) <= FaultKind::BadCall as u32 || kind == FaultKind::HardwareBreakpoint,
            "{kind:?} is the kernel's"
        );
        // A hardware breakpoint hits only a thread whose control word
        // enables one; the exception words are then the bits that hit and
        // the status word (x86-64: DR6), words 3 and 4 of the fault.
        assert!(
            kind != FaultKind::HardwareBreakpoint
                || self.threads[&thread].regs[REG_DR7] & 0xFF != 0,
            "no breakpoint is enabled"
        );
        self.fault_once(thread, kind, exception);
    }

    /// A #DB taken at CPL0 on the first instruction of the system-call
    /// entry of `thread`, then the call, in one event (x86-64, D-143,
    /// D-146): `causes` are the bits the processor sets in DR6. `None` if
    /// the #DB is a bug of the kernel: the node accuses itself and the call
    /// is not made.
    pub fn call_with_debug(
        &mut self,
        thread: ObjId,
        causes: u64,
        call: Syscall,
    ) -> Option<Outcome> {
        self.debug_entry(thread, causes, true)
            .then(|| self.apply(thread, call))
    }

    /// A #DB taken at CPL0 on the first instruction of the kernel's entry
    /// of a fault of `thread` (a `MOV SS` before a faulting instruction),
    /// then the fault, in one event; BS there is a bug of the kernel. Says
    /// whether the fault was taken.
    pub fn fault_with_debug(
        &mut self,
        thread: ObjId,
        causes: u64,
        kind: FaultKind,
        exception: [u64; 3],
    ) -> bool {
        let taken = self.debug_entry(thread, causes, false);
        if taken {
            self.fault_once(thread, kind, exception);
        }
        taken
    }

    /// The #DB handler at CPL0: it reads DR6 once and loads its reset
    /// value; each cause it names is judged alone: an enabled breakpoint of
    /// the thread is a fault of kind 9, kept in the thread and raised when
    /// it would next return to user mode; BS at the first instruction of
    /// the system-call entry (`at_entry`) is ignored; BS elsewhere, BD, BT,
    /// or no cause at all is a bug of the kernel, which accuses itself in
    /// its own event. Says whether the entry goes on.
    fn debug_entry(&mut self, thread: ObjId, causes: u64, at_entry: bool) -> bool {
        assert_eq!(self.machine.arch, crate::Arch::X86_64);
        assert!(self.runnable().contains(&thread), "{thread:?} does not run");
        assert_eq!(causes & !DR6_CAUSES, 0, "not a cause of DR6");
        let core = self.core_of(thread);
        self.cores[core].dr6 |= causes;
        let read = self.read_dr6(core);
        let control = self.threads[&thread].regs[REG_DR7];
        let enabled = (0..4)
            .filter(|i| control >> (2 * i) & 3 != 0)
            .fold(0, |bits, i| bits | 1 << i);
        let hit = read & 0xF & enabled;
        let step = read & DR6_BS != 0;
        let bug = read & (DR6_BD | DR6_BT) != 0 || step && !at_entry || hit == 0 && !step;
        if bug {
            *self.last_event = None;
            let entry = self.begin_on(core);
            self.accuse_self(dodos_abi::CrashReason::KernelException as u32);
            self.end(entry);
            self.auto_deliver();
            return false;
        }
        if hit != 0 {
            live(&mut self.threads, thread).debug_kept = Some(hit << 32 | read);
        }
        true
    }

    /// Kind 9 kept from a #DB at CPL0, raised as the fault *no context* is:
    /// at the end of the event in which its thread is runnable with a
    /// context, before it runs any user instruction, words 0 and 1 where it
    /// would resume and its stack pointer, word 3 the hits of bits 32-35,
    /// word 4 the DR6 value of bits 0-31; a fault of the entry itself goes
    /// first, a thread also marked *no context* raises kind 8 first and
    /// keeps kind 9, and a thread that waits or is suspended keeps it;
    /// raising it, delivered or recorded, clears the word (D-146, D-147).
    pub(crate) fn raise_kept_debug(&mut self) {
        let kept: Vec<ObjId> = self
            .threads
            .iter()
            .filter(|(_, t)| t.debug_kept.is_some() && !t.lost && Self::runnable_thread(t))
            .map(|(&id, _)| id)
            .collect();
        for thread in kept {
            let Some(word) = live(&mut self.threads, thread).debug_kept.take() else {
                continue;
            };
            let (hit, read) = (word >> 32 & 0xF, word & 0xFFFF_FFFF);
            self.raise(thread, FaultKind::HardwareBreakpoint, [1, hit, read]);
        }
    }

    /// The #DB handler reads DR6 once and loads it with its reset value
    /// (Intel SDM §20.2.3): the value read.
    fn read_dr6(&mut self, core: usize) -> u64 {
        DR6_RESET | std::mem::take(&mut self.cores[core].dr6)
    }

    /// A fault, with no lock held by another core.
    pub(crate) fn fault_once(&mut self, thread: ObjId, kind: FaultKind, exception: [u64; 3]) {
        *self.last_event = None;
        let core = self.core_of(thread);
        let entry = self.begin_on(core);
        live(&mut self.threads, thread).outcome = None;
        // A hardware breakpoint at CPL3: the processor sets the causes of
        // word 4 in DR6, and the handler reads it once and loads its reset
        // value, as at CPL0 (D-143); word 4 is the value read.
        let mut exception = exception;
        if kind == FaultKind::HardwareBreakpoint && self.machine.arch == crate::Arch::X86_64 {
            self.cores[core].dr6 |= exception[2] & DR6_CAUSES;
            exception[2] = self.read_dr6(core);
        }
        // A spurious fault (D-129): a page fault on an address the node's
        // own tables translate with the access asked is resolved on the
        // faulting core, without a message.
        let spurious = kind == FaultKind::Page && self.spurious(thread, exception[2], exception[1]);
        if !spurious {
            self.raise(thread, kind, exception);
        }
        self.end(entry);
        self.auto_deliver();
    }

    /// Faults: delivery is a `call` by the faulting thread on its fault
    /// endpoint, with an empty receive range and wait number 0, sent even
    /// when its capability space is dying. Without a fault endpoint, or with
    /// a dying one, the thread is suspended at once and the fault recorded in
    /// the thread.
    pub(crate) fn raise(&mut self, thread: ObjId, kind: FaultKind, exception: [u64; 3]) {
        self.event.raised.push((thread, kind as u32));
        let faulting = &self.threads[&thread];
        // Words: where the thread resumes after an answer with label 0, the
        // stack pointer, then the words of the exception.
        let [number, access, address] = exception;
        let words = [
            faulting.regs[REG_RIP],
            faulting.regs[REG_RSP],
            number,
            access,
            address,
        ];
        let endpoint = faulting
            .fault
            .filter(|&(endpoint, _)| !self.dying(endpoint));
        let Some((endpoint, badge)) = endpoint else {
            self.record(thread, kind as u32, words);
            return;
        };
        // A fault *no context* goes one level only: if the first thread
        // that waits to receive has no own context, it is not delivered and
        // the waiting receiver is not disturbed.
        let passive = self
            .receiver(endpoint)
            .is_some_and(|(server, ..)| self.threads[&server].home.is_none());
        if kind == FaultKind::NoContext && passive {
            self.record(thread, kind as u32, words);
            return;
        }
        let word0 = words[0];
        let words = words.to_vec();
        let parcel = Parcel {
            label: kind as u32,
            words,
            badge,
            sender: thread,
            caps: Vec::new(),
            marks: 0,
        };
        // An empty receive range names no slot; its space is not looked at.
        let window = Window {
            cspace: faulting.cspace.unwrap_or(endpoint),
            first: 0,
            count: 0,
        };
        let raising = live(&mut self.threads, thread);
        (raising.faulting, raising.raised) = (true, Some((kind as u32, word0)));
        self.enter(thread, endpoint, parcel, window);
    }

    /// Faults: a fault that is not delivered is recorded in the thread,
    /// which becomes suspended; its registers are unchanged.
    pub(crate) fn record(&mut self, thread: ObjId, kind: u32, words: [u64; 5]) {
        let recorded = live(&mut self.threads, thread);
        (recorded.suspended, recorded.recorded) = (true, Some((kind, words)));
        self.observe();
        // Debugging: code 5, the fault recorded in its thread.
        let payload = [u64::from(kind), words[0], words[4]];
        self.trace(dodos_abi::TraceCode::FaultRecorded, Some(thread), payload);
    }

    /// Faults, the answer: with label 0 the thread resumes where its fault
    /// left it; with label 1 and at least 2 words, at the instruction and
    /// stack pointers of words 0 and 1 (a non-canonical address counts as
    /// another label); with any other label it is left suspended. A fault
    /// wait that ends without an answer leaves it suspended. A thread
    /// suspended while it waited gets the answer's effect and runs when it
    /// is resumed.
    pub(crate) fn answered(&mut self, thread: ObjId, outcome: Outcome) {
        let resumed = live(&mut self.threads, thread);
        let Ok(Output::Message(answer)) = outcome else {
            resumed.suspended = true;
            self.observe();
            return;
        };
        let header = answer.received.header;
        let redirect = match answer.words[..] {
            [rip, rsp, ..]
                if header.words() >= 2
                    && self.machine.geometry.canonical(rip)
                    && self.machine.geometry.canonical(rsp) =>
            {
                Some((rip, rsp))
            }
            _ => None,
        };
        match (header.label(), redirect) {
            (FAULT_REPLY_RESUME, _) => {}
            (FAULT_REPLY_REDIRECT, Some((rip, rsp))) => {
                (resumed.regs[REG_RIP], resumed.regs[REG_RSP]) = (rip, rsp);
            }
            _ => resumed.suspended = true,
        }
        self.observe();
    }

    /// Register calls: `read` on the thread; the caller's message page,
    /// judged first among the plain arguments; `busy` if the thread is not
    /// suspended, so a thread never reads its own registers. A waiting
    /// thread can be read.
    pub(crate) fn thread_read_regs(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::READ)?;
        self.page_of(thread)?;
        let read = &self.threads[&target];
        if !read.suspended || read.leaving {
            return Err(Error::Busy);
        }
        // The register block, then the fault record: its kind in the low
        // 32 bits of a word (0 if none), and its five words.
        let (kind, words) = read.recorded.unwrap_or((0, [0; 5]));
        let mut block = read.regs.clone();
        block.push(u64::from(kind));
        block.extend(words);
        Ok(Output::Regs(block))
    }

    /// Register calls: `write` on the thread; the caller's message page;
    /// exactly the architecture's number of words; the canonical words in
    /// order. `busy` if the thread is not suspended, then if it waits. The
    /// bits of the flags word that user code cannot change are forced.
    pub(crate) fn thread_write_regs(
        &mut self,
        thread: ObjId,
        target: SlotAddr,
        block: Vec<u64>,
    ) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::WRITE)?;
        self.page_of(thread)?;
        if block.len() != REGS_WORDS {
            return Err(Error::Invalid);
        }
        // Judged word by word: the canonical addresses, the breakpoint
        // addresses inside the user range, the breakpoint control within the
        // user mask (`invalid`), and the user-interrupt word, which must be 0
        // (`unsupported`, the placed hook).
        let user_end = self.machine.geometry.user_end;
        for (index, &word) in block.iter().enumerate() {
            let bad = match index {
                i if REGS_CANONICAL.contains(&i) => !self.machine.geometry.canonical(word),
                REG_DR0..=REG_DR3 => word >= user_end,
                REG_DR7 => !dr7_valid(word),
                REG_UINTR if word != 0 => return Err(Error::Unsupported),
                _ => false,
            };
            if bad {
                return Err(Error::Invalid);
            }
        }
        let written = live(&mut self.threads, target);
        // A node of several cores: a thread marked *leaving its core* has
        // its registers still on another core.
        if !written.suspended || written.wait != Wait::None || written.leaving {
            return Err(Error::Busy);
        }
        written.regs = block;
        written.regs[REG_RFLAGS] = rflags(written.regs[REG_RFLAGS]);
        Ok(Output::Done)
    }

    /// Bindings: the register calls from a thread without a message page
    /// are `invalid`.
    fn page_of(&self, thread: ObjId) -> Result<(), Error> {
        self.threads[&thread].page.map(|_| ()).ok_or(Error::Invalid)
    }
}
