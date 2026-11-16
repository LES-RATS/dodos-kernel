//! Threads: faults, and the register calls.

use crate::{Model, ObjId, Outcome, Output, Parcel, Wait, Window, locked};
use dodos_abi::x86_64::{
    REG_DR0, REG_DR3, REG_DR7, REG_RFLAGS, REG_RIP, REG_RSP, REG_UINTR, REGS_CANONICAL, REGS_WORDS,
    dr7_valid, rflags,
};
use dodos_abi::{
    Error, FAULT_REPLY_REDIRECT, FAULT_REPLY_RESUME, FaultKind, ObjectKind, Rights, SlotAddr,
};

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
        *self.last_event = None;
        let core = self.core_of(thread);
        let entry = self.begin_on(core);
        locked(&mut self.threads, &self.locks, thread).outcome = None;
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
    /// a dying one, the thread is suspended at once and the fault dropped.
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
        let raising = locked(&mut self.threads, &self.locks, thread);
        (raising.faulting, raising.raised) = (true, Some((kind as u32, word0)));
        self.enter(thread, endpoint, parcel, window);
    }

    /// Faults: a fault that is not delivered is recorded in the thread,
    /// which becomes suspended; its registers are unchanged.
    pub(crate) fn record(&mut self, thread: ObjId, kind: u32, words: [u64; 5]) {
        let recorded = locked(&mut self.threads, &self.locks, thread);
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
        let resumed = locked(&mut self.threads, &self.locks, thread);
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
        let written = locked(&mut self.threads, &self.locks, target);
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
