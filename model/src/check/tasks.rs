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
    ///     so are the inputs of the lines claimed by entries that are not
    ///     removing (a removal's masking with *release* frees the line word,
    ///     and the line may be claimed again while the dying entry waits for
    ///     its invalidations: `irq_claim` is `busy` on the word alone, D-135,
    ///     D-150); an entry is idle if and
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
            let line = !irq.source.is_line() || irq.removing || lines.insert(irq.source.id());
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
            // D-143, D-146, RISC-V: an armed level line whose input is high
            // has an MSI on its way, the acknowledgement or the bind having
            // written `setipnum` and then `setienum`; a fired one is masked
            // at its APLIC, by the mask at its arrival.
            let riscv_level = self.machine.arch == crate::Arch::Riscv64
                && irq.source.is_line()
                && irq.source.level();
            let line = self
                .table
                .aplic
                .get(&irq.source.id())
                .copied()
                .unwrap_or_default();
            // D-169: if a holder cleared this specific line during a rebuild
            // because of a recorded cut of this node's current generation on
            // this line's controller role, exempt this line alone from the
            // controller-side liveness checks ("which costs only that node").
            let cleared_faulty = irq.source.is_line()
                && self.ioapic_role(irq.source.id()).is_some_and(|role| {
                    self.table.units.get(&role).is_some_and(|u| {
                        !u.lines.contains_key(&irq.source.id())
                            && u.cut_lines.contains(&(
                                irq.source.id(),
                                self.node,
                                self.table.generation(self.node),
                            ))
                    })
                });
            let lost = riscv_level
                && irq.state == IrqState::Armed
                && !self.line_masked(irq.source)
                && !cleared_faulty
                && line.high
                && line.sent == 0;
            ensure(
                !lost,
                "SETIPNUM: an armed level line high and no MSI on its way",
            )?;
            // (A rebuild may enable a fired one again: D-147.)
            // D-147: a claimed RISC-V line's source is active; an inactive
            // source has no pending or enable bit and sends nothing.
            ensure(
                self.machine.arch != crate::Arch::Riscv64
                    || !irq.source.is_line()
                    || irq.removing
                    || self.line_asking(irq.source.id())
                    || cleared_faulty
                    || line.active,
                "SOURCECFG: a claimed line's APLIC source inactive",
            )?;
            // D-148: on x86-64 without a directed end of interrupt a level
            // line is claimed only where one node runs.
            ensure(
                !self.masks_at_arrival()
                    || !irq.source.is_line()
                    || !irq.source.level()
                    || self.one_node_runs(),
                "LEVEL: a level line with no directed end of interrupt beside other nodes",
            )?;
            // D-147: but on x86-64 without a directed end of interrupt, no
            // node masks a level line at its arrival, and a rebuild never
            // leaves a claimed, live one masked that only its claimer could
            // unmask.
            ensure(
                self.masks_at_arrival()
                    || !irq.source.is_line()
                    || !irq.source.level()
                    || irq.removing
                    || cleared_faulty
                    || !self.line_masked(irq.source),
                "LINE MASK: a claimed, live level line masked at its controller",
            )?;
        }
        // A level source sends one MSI at a time: a line that fell and rose
        // while masked gives one arrival, not two (D-146).
        // D-146: a level line is masked at its APLIC at its arrival.
        ensure(
            !self.event.arrival_enabled,
            "CLRIENUM: an arrival left its level line enabled at its APLIC",
        )?;
        ensure(
            self.table.aplic.values().all(|line| line.sent <= 1),
            "APLIC: two MSIs of one level source on their way",
        )?;
        ensure(
            self.table
                .aplic
                .values()
                .all(|line| line.active || !line.pending && !line.enabled),
            "SOURCECFG: an inactive APLIC source pending or enabled",
        )?;
        // G9: this event wrote an APLIC register only for a level source
        // whose line word this node holds and that a core of it claimed.
        for &gsi in &self.event.aplic {
            let word = self.table.line_word(gsi);
            let held = word.owner() == dodos_abi::Owner::Node(self.node)
                && word.generation() == self.table.generation(self.node);
            let claimed = self
                .irqs
                .values()
                .any(|irq| irq.source.is_line() && irq.source.level() && irq.source.id() == gsi);
            ensure(
                held && claimed,
                "G9: an APLIC register written for a line the node does not hold",
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
