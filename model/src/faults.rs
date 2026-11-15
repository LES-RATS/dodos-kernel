//! Threads: faults, and the register calls.

use crate::{Model, ObjId, Outcome, Output, Parcel, Wait, Window, live};
use dodos_abi::x86_64::{
    REG_RFLAGS, REG_RIP, REG_RSP, REGS_CANONICAL, REGS_WORDS, canonical, rflags,
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
            (kind as u32) <= FaultKind::BadCall as u32,
            "{kind:?} is the kernel's"
        );
        let entry = self.begin();
        live(&mut self.threads, thread).outcome = None;
        self.raise(thread, kind, exception);
        self.end(entry);
    }

    /// Faults: delivery is a `call` by the faulting thread on its fault
    /// endpoint, with an empty receive range and wait number 0, sent even
    /// when its capability space is dying. Without a fault endpoint, or with
    /// a dying one, the thread is suspended at once and the fault dropped.
    pub(crate) fn raise(&mut self, thread: ObjId, kind: FaultKind, exception: [u64; 3]) {
        let faulting = &self.threads[&thread];
        let endpoint = faulting
            .fault
            .filter(|&(endpoint, _)| !self.dying(endpoint));
        let Some((endpoint, badge)) = endpoint else {
            live(&mut self.threads, thread).suspended = true;
            return;
        };
        // Words: where the thread resumes after an answer with label 0, the
        // stack pointer, then the words of the exception.
        let [number, access, address] = exception;
        let words = vec![
            faulting.regs[REG_RIP],
            faulting.regs[REG_RSP],
            number,
            access,
            address,
        ];
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
        live(&mut self.threads, thread).faulting = true;
        self.enter(thread, endpoint, parcel, window);
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
            return;
        };
        let header = answer.received.header;
        let redirect = match answer.words[..] {
            [rip, rsp, ..] if header.words() >= 2 && canonical(rip) && canonical(rsp) => {
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
    }

    /// Register calls: `read` on the thread; the caller's message page,
    /// judged first among the plain arguments; `busy` if the thread is not
    /// suspended, so a thread never reads its own registers. A waiting
    /// thread can be read.
    pub(crate) fn thread_read_regs(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::READ)?;
        self.page_of(thread)?;
        let read = &self.threads[&target];
        if !read.suspended {
            return Err(Error::Busy);
        }
        Ok(Output::Regs(read.regs.clone()))
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
        if REGS_CANONICAL.iter().any(|&word| !canonical(block[word])) {
            return Err(Error::Invalid);
        }
        let written = live(&mut self.threads, target);
        if !written.suspended || written.wait != Wait::None {
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
