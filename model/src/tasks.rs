//! Tasks and marks: the unit of provenance and of stopping.

use crate::{Model, ObjId, Outcome, Output, Wait, locked};
use dodos_abi::{Error, Marks, ObjectKind, Rights, SlotAddr, TaskFlags, TaskId};

impl Model {
    /// The effective marks of a thread: those of its task if it is attached
    /// to one, otherwise its own word.
    pub fn marks(&self, thread: ObjId) -> Option<Marks> {
        let thread = self.threads.get(&thread)?;
        let task = thread.task.map(|task| self.tasks[&task].marks);
        Marks::from_bits(task.unwrap_or(thread.marks)).ok()
    }

    /// Adds marks to the effective marks of a thread.
    pub(crate) fn mark(&mut self, thread: ObjId, marks: u64) {
        match self.threads[&thread].task {
            Some(task) => locked(&mut self.tasks, &self.locks, task).marks |= marks,
            None => locked(&mut self.threads, &self.locks, thread).marks |= marks,
        }
    }

    /// Whether a thread is attached to a mark-aware task.
    pub(crate) fn aware(&self, thread: ObjId) -> bool {
        let task = self.threads[&thread].task;
        task.is_some_and(|task| self.tasks[&task].aware)
    }

    /// A revoked task answers `gone` to every call that names it, at the
    /// check of its capability, but `task_read`, `task_revoke` and the
    /// capability calls.
    ///
    /// A full slot whose object is a revoked task is `gone` before its kind
    /// and rights are judged; an empty or out-of-range slot is still
    /// `invalid` and a dead one `gone` (Tasks and marks).
    fn task(&self, thread: ObjId, at: SlotAddr, need: Rights) -> Result<ObjId, Error> {
        let cap = self.cap(thread, at, Rights::READ)?;
        let object = self.caps[&cap].object;
        if self.tasks.get(&object).is_some_and(|task| task.revoked) {
            return Err(Error::Gone);
        }
        Ok(self.is(cap, ObjectKind::Task, need)?.object)
    }

    pub(crate) fn task_attach(
        &mut self,
        thread: ObjId,
        target: SlotAddr,
        task: SlotAddr,
    ) -> Outcome {
        let target = self.object(thread, target, ObjectKind::Thread, Rights::MANAGE)?;
        let task = match task.is_none() {
            true => None,
            false => Some(self.task(thread, task, Rights::MANAGE)?),
        };
        let old = self.threads[&target].task;
        if old.is_some_and(|old| self.tasks[&old].revoked) {
            return Err(Error::Busy);
        }
        if task == old {
            return Ok(Output::Done);
        }
        let marks = self.marks(target).expect("a live thread").bits();
        self.detach_task(target);
        match task {
            // The thread joins the new task, whose marks become marks | m,
            // even when it is mark-aware.
            Some(task) => {
                let joined = locked(&mut self.tasks, &self.locks, task);
                joined.marks |= marks;
                joined.threads.push(target);
                locked(&mut self.threads, &self.locks, target).task = Some(task);
            }
            // Detaching sets the thread's own word to the marks of the task
            // it leaves; it succeeds on a thread attached to nothing.
            None => locked(&mut self.threads, &self.locks, target).marks |= marks,
        }
        Ok(Output::Done)
    }

    /// The thread leaves its task, keeping the task's marks in its own word.
    pub(crate) fn detach_task(&mut self, thread: ObjId) {
        let Some(task) = locked(&mut self.threads, &self.locks, thread).task.take() else {
            return;
        };
        let left = locked(&mut self.tasks, &self.locks, task);
        left.threads.retain(|&other| other != thread);
        let marks = left.marks;
        locked(&mut self.threads, &self.locks, thread).marks |= marks;
    }

    pub(crate) fn task_mark(&mut self, thread: ObjId, task: SlotAddr, marks: u64) -> Outcome {
        let task = self.task(thread, task, Rights::WRITE)?;
        let marks = Marks::from_bits(marks)?;
        locked(&mut self.tasks, &self.locks, task).marks |= marks.bits();
        Ok(Output::Done)
    }

    /// `task_exempt`: the aware task must be mark-aware, `denied`
    /// otherwise, judged as a right of its slot.
    pub(crate) fn task_exempt(
        &mut self,
        thread: ObjId,
        task: SlotAddr,
        aware: SlotAddr,
    ) -> Outcome {
        let task = self.task(thread, task, Rights::MANAGE)?;
        let aware = self.task(thread, aware, Rights::MANAGE)?;
        if !self.tasks[&aware].aware {
            return Err(Error::Denied);
        }
        locked(&mut self.tasks, &self.locks, task).aware = true;
        Ok(Output::Done)
    }

    /// `task_revoke`: the first call sets the flag revoked; each call then
    /// detaches up to 32 attached threads, most recently attached first,
    /// ending their waits with `gone` and suspending them. `again` while
    /// attached threads remain.
    pub(crate) fn task_revoke(&mut self, thread: ObjId, task: SlotAddr) -> Outcome {
        let cap = self.cap(thread, task, Rights::READ)?;
        let task = self.is(cap, ObjectKind::Task, Rights::MANAGE)?.object;
        locked(&mut self.tasks, &self.locks, task).revoked = true;
        for _ in 0..dodos_abi::REMOVAL_STEP {
            let Some(&attached) = self.tasks[&task].threads.last() else {
                return Ok(Output::Done);
            };
            self.stop(attached);
        }
        match self.tasks[&task].threads.is_empty() {
            true => Ok(Output::Done),
            false => Err(Error::Again),
        }
    }

    /// A step of `task_revoke` on one thread: its wait ends with `gone`, it
    /// is suspended and detached, keeping the marks of the task.
    pub(crate) fn stop(&mut self, thread: ObjId) {
        // A thread that waits for another node, a unit or its node's cores
        // is stopped when its call returns: its wait is not ended.
        let wait = &self.threads[&thread].wait;
        if *wait != Wait::None && !self.threads[&thread].bounded() {
            self.unlink(thread);
            self.wake(thread, Err(Error::Gone));
        }
        locked(&mut self.threads, &self.locks, thread).suspended = true;
        self.observe();
        self.detach_task(thread);
    }

    pub(crate) fn task_read(&mut self, thread: ObjId, task: SlotAddr) -> Outcome {
        let cap = self.cap(thread, task, Rights::READ)?;
        let task = &self.tasks[&self.is(cap, ObjectKind::Task, Rights::READ)?.object];
        let flag = |set: bool, bit: u64| if set { bit } else { 0 };
        Ok(Output::Task {
            id: task.id.raw(),
            marks: task.marks,
            flags: flag(task.aware, TaskFlags::AWARE) | flag(task.revoked, TaskFlags::REVOKED),
            threads: task.threads.len() as u64,
        })
    }

    /// Destroying a task: its attached threads are detached, most recently
    /// attached first, one unit each, keeping its marks; if it was revoked,
    /// each is also stopped. Says whether there was one.
    pub(crate) fn detach_one(&mut self, task: ObjId) -> bool {
        let Some(&thread) = self.tasks[&task].threads.last() else {
            return false;
        };
        match self.tasks[&task].revoked {
            true => self.stop(thread),
            false => self.detach_task(thread),
        }
        true
    }

    /// A new task: the next identifier of the node, marks 0, neither
    /// mark-aware nor revoked, no thread.
    pub(crate) fn next_task(&mut self) -> TaskId {
        self.task_counter += 1;
        TaskId::new(self.node, self.task_counter)
    }
}
