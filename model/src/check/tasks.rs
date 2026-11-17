//! The invariants of tasks and marks, and of interrupts.

use super::{Check, ensure};
use crate::{IrqState, Model};
use dodos_abi::Marks;
use dodos_abi::x86_64::{IRQ_VECTOR_FIRST, IRQ_VECTOR_LAST};
use std::collections::BTreeSet;

impl Model {
    /// Tasks and marks:
    /// 48. a thread attached to a task is listed once by it, and a task
    ///     lists only threads attached to it;
    /// 49. task identifiers are of this node, given out once each, from 1 up
    ///     to the counter; marks, of tasks and of threads, have no reserved
    ///     bit;
    /// 50. the running thread is not attached to a revoked task.
    pub(super) fn check_tasks(&self) -> Check {
        let mut ids = BTreeSet::new();
        for (&id, task) in &self.tasks {
            let given = task.id.node() == self.node
                && (1..=self.task_counter).contains(&task.id.counter())
                && ids.insert(task.id.raw());
            ensure(given, "49: task identifier")?;
            ensure(Marks::from_bits(task.marks).is_ok(), "49: marks of a task")?;
            let attached = task
                .threads
                .iter()
                .all(|thread| self.threads.get(thread).is_some_and(|t| t.task == Some(id)));
            let once = task.threads.iter().collect::<BTreeSet<_>>().len() == task.threads.len();
            ensure(attached && once, "48: threads of a task")?;
        }
        // A counter of at least 1 makes a raw identifier that is never
        // `TaskId::NONE` (0), so no line asks it (ver-D D3).
        for (&id, thread) in &self.threads {
            let listed = thread.task.is_none_or(|task| {
                self.tasks
                    .get(&task)
                    .is_some_and(|t| t.threads.contains(&id))
            });
            ensure(listed, "48: task of a thread")?;
            ensure(
                Marks::from_bits(thread.marks).is_ok(),
                "49: marks of a thread",
            )?;
        }
        for core in &self.cores {
            let revoked = core
                .thread
                .and_then(|thread| self.threads.get(&thread)?.task);
            ensure(
                revoked.is_none_or(|task| !self.tasks.get(&task).is_some_and(|t| t.revoked))
                    || core.ipis.contains(&crate::Ipi::Reschedule),
                "50: running thread of a revoked task",
            )?;
        }
        Ok(())
    }

    /// Interrupts:
    /// 51. the vectors of the claimed entries are claimable and distinct, and
    ///     so are the inputs of the claimed lines; an entry is idle if and
    ///     only if it is bound to no notify, its notify is live and lists it
    ///     once, and a notify lists only the entries bound to it; a level
    ///     source has nothing pending.
    pub(super) fn check_irqs(&self) -> Check {
        let mut vectors = BTreeSet::new();
        let mut lines = BTreeSet::new();
        for (&id, irq) in &self.irqs {
            let claimable = (IRQ_VECTOR_FIRST..=IRQ_VECTOR_LAST).contains(&irq.vector);
            ensure(
                claimable && vectors.insert(irq.vector),
                "51: vector of an irq",
            )?;
            let line = !irq.source.is_line() || lines.insert(irq.source.id());
            ensure(line, "51: line of an irq")?;
            let idle = (irq.state == IrqState::Idle) == irq.notify.is_none();
            let listed = irq.notify.is_none_or(|(notify, _)| {
                let notify = self.notifies.get(&notify);
                notify.is_some_and(|n| n.irqs.iter().filter(|&&i| i == id).count() == 1)
            });
            ensure(idle && listed, "51: binding of an irq")?;
            ensure(
                !(irq.source.level() && irq.pending),
                "51: pending level source",
            )?;
        }
        for (&id, notify) in &self.notifies {
            let bound = notify.irqs.iter().all(|irq| {
                self.irqs
                    .get(irq)
                    .is_some_and(|irq| irq.notify.is_some_and(|(n, _)| n == id))
            });
            ensure(bound, "51: irqs of a notify")?;
        }
        Ok(())
    }
}
