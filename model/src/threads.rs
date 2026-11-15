//! Threads: configuration, bindings, resumption, suspension, cancellation.

use crate::{CapId, Model, ObjId, Outcome, Output, Record, Wait, live};
use dodos_abi::{Error, MemoryType, ObjectKind, PageSize, Rights, SlotAddr};

/// The four bindings that `thread_configure` replaces at once.
pub(crate) struct Bindings {
    pub(crate) cspace: SlotAddr,
    pub(crate) aspace: SlotAddr,
    pub(crate) fault: SlotAddr,
    pub(crate) page: SlotAddr,
}

/// Leaves `list` if `id` is in it.
fn leave(list: &mut Vec<ObjId>, id: ObjId) {
    list.retain(|&other| other != id);
}

impl Model {
    pub(crate) fn thread_configure(
        &mut self,
        thread: ObjId,
        target: SlotAddr,
        bindings: Bindings,
    ) -> Outcome {
        let manage = Rights::MANAGE;
        let target = self.object(thread, target, ObjectKind::Thread, manage)?;
        let space = self.object(thread, bindings.cspace, ObjectKind::CSpace, manage)?;
        // Threads: the aspace, the fault endpoint and the message page may
        // be 0.
        let aspace = self.optional(thread, bindings.aspace, ObjectKind::ASpace, manage)?;
        let fault = match bindings.fault.is_none() {
            true => None,
            false => {
                let held = self.held(thread, bindings.fault, ObjectKind::Endpoint, Rights::SEND)?;
                Some((held.object, held.badge))
            }
        };
        let page = match bindings.page.is_none() {
            true => None,
            false => Some(self.message_page(thread, bindings.page)?),
        };
        let bound = &self.threads[&target];
        // Threads: the thread must be suspended and must not wait: a wait
        // holds a receive range of the old capability space.
        if !bound.suspended || bound.wait != Wait::None {
            return Err(Error::Busy);
        }
        // Then the record of the message page's capability must be empty,
        // unless it already holds this thread's binding (D-110).
        if let Some(page) = page {
            let record = self.caps[&page].record;
            if record != Record::Empty && record != Record::Binding(target) {
                return Err(Error::Busy);
            }
        }
        self.bind_space(target, Some(space));
        self.bind_aspace(target, aspace);
        self.bind_fault(target, fault);
        self.bind_page(target, page);
        Ok(Output::Done)
    }

    /// Threads: the message page is a frame of exactly one page (index 0) of
    /// ordinary memory (`invalid`, judged with its kind), with `read` and
    /// `write`.
    fn message_page(&self, thread: ObjId, at: SlotAddr) -> Result<CapId, Error> {
        let cap = self.cap(thread, at, Rights::READ)?;
        let object = self.caps[&cap].object;
        let one_page = self.frames.get(&object).is_some_and(|frame| {
            frame.page == PageSize::Page
                && frame.pages == 1
                && frame.memory == MemoryType::WriteBack
        });
        if self.objects[&object].kind == ObjectKind::Frame && !one_page {
            return Err(Error::Invalid);
        }
        self.is(cap, ObjectKind::Frame, Rights::READ.union(Rights::WRITE))?;
        Ok(cap)
    }

    /// Threads: `busy` if the thread is bound to no capability space (a
    /// thread whose space is destroyed is bound to none, D-110); this `busy`
    /// comes before "already in the state asked for", which succeeds. A
    /// thread bound to a dying space can be resumed.
    pub(crate) fn thread_resume(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        self.bound_space(target).or(Err(Error::Busy))?;
        // Register calls: resuming a suspended thread clears its fault
        // record.
        let resumed = live(&mut self.threads, target);
        if resumed.suspended {
            resumed.recorded = None;
        }
        resumed.suspended = false;
        self.observe();
        Ok(Output::Done)
    }

    /// Threads: suspending a suspended thread succeeds, a thread bound to no
    /// space too, and suspending a thread that waits leaves its wait as it
    /// is.
    pub(crate) fn thread_suspend(&mut self, thread: ObjId, target: SlotAddr) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        live(&mut self.threads, target).suspended = true;
        self.observe();
        Ok(Output::Done)
    }

    /// Threads: with a wait number other than 0, the word "cancelled up to"
    /// of the thread becomes the greater of itself and the number; then the
    /// wait of the thread is ended if its wait number is not 0 and not
    /// greater than the word. With wait number 0 the word does not change
    /// and the wait is ended whatever its number.
    ///
    /// A wait that is ended: the thread is taken out of its queue and its
    /// call returns `cancelled`; a reply object attached to it or armed with
    /// it becomes idle. Nothing of its message moves: the capabilities it
    /// lists are transferred only at delivery. A suspended thread stays
    /// suspended and finds `cancelled` when it runs again. The thread may be
    /// bound to no capability space, or to one that is dying: only the
    /// capability space of the caller is looked at.
    pub(crate) fn thread_cancel(
        &mut self,
        thread: ObjId,
        target: SlotAddr,
        number: u64,
    ) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        let cancelled = live(&mut self.threads, target);
        cancelled.cancelled_up_to = cancelled.cancelled_up_to.max(number);
        let ended = match number {
            0 => true,
            _ => cancelled.number != 0 && cancelled.number <= cancelled.cancelled_up_to,
        };
        // Threads: a thread whose wait is already over, and that has not run
        // since, does not wait: it keeps the result it has. The result says
        // nothing about what the thread was doing.
        // Calls on an object of another node: a wait for another node is
        // not ended here, but a wait on an object of the other node, which a
        // proxy holds, is ended there; its call returns `cancelled` then.
        // A request still waiting for a free proxy leaves its list, and the
        // call returns `cancelled` at once (model choice).
        if let Wait::Node(id) = cancelled.wait {
            if ended && self.cancel_remote(id) == Some(true) {
                self.unlink(target);
                self.wake(target, Err(Error::Cancelled));
            }
            return Ok(Output::Done);
        }
        if ended && cancelled.wait != Wait::None {
            self.unlink(target);
            self.wake(target, Err(Error::Cancelled));
        }
        Ok(Output::Done)
    }

    // Bindings: each is a link both ways, so undoing it is one step. A
    // thread goes to the head of the list of what it is bound to (here the
    // end of the vector) after it leaves the list of what it was bound to.

    pub(crate) fn bind_space(&mut self, thread: ObjId, space: Option<ObjId>) {
        let old = std::mem::replace(&mut live(&mut self.threads, thread).cspace, space);
        if let Some(old) = old {
            leave(&mut live(&mut self.cspaces, old).threads, thread);
        }
        if let Some(space) = space {
            live(&mut self.cspaces, space).threads.push(thread);
        }
    }

    pub(crate) fn bind_aspace(&mut self, thread: ObjId, aspace: Option<ObjId>) {
        let old = std::mem::replace(&mut live(&mut self.threads, thread).aspace, aspace);
        if let Some(old) = old {
            leave(&mut live(&mut self.aspaces, old).threads, thread);
        }
        if let Some(aspace) = aspace {
            live(&mut self.aspaces, aspace).threads.push(thread);
        }
    }

    pub(crate) fn bind_fault(&mut self, thread: ObjId, fault: Option<(ObjId, u64)>) {
        let old = std::mem::replace(&mut live(&mut self.threads, thread).fault, fault);
        if let Some((old, _)) = old {
            leave(&mut live(&mut self.endpoints, old).threads, thread);
        }
        if let Some((endpoint, _)) = fault {
            live(&mut self.endpoints, endpoint).threads.push(thread);
        }
    }

    /// Bindings: the message page is bound through the record of the frame
    /// capability given; the thread names the capability.
    pub(crate) fn bind_page(&mut self, thread: ObjId, page: Option<CapId>) {
        let old = std::mem::replace(&mut live(&mut self.threads, thread).page, page);
        if let Some(old) = old {
            live(&mut self.caps, old).record = Record::Empty;
        }
        if let Some(page) = page {
            live(&mut self.caps, page).record = Record::Binding(thread);
        }
    }

    /// The derivation tree: when a thread is destroyed, each of its
    /// bindings is undone in the same step.
    pub(crate) fn unbind_all(&mut self, thread: ObjId) {
        self.bind_space(thread, None);
        self.bind_aspace(thread, None);
        self.bind_fault(thread, None);
        self.bind_page(thread, None);
    }
}
