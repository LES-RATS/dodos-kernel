//! The invariants of unlike cores (8.3), read from the state of one node
//! and the published values of its kinds.

use super::{Check, ensure};
use crate::Model;
use dodos_abi::{FEATURE_USER_INTERRUPTS, FeatureMask, thread_bytes};

impl Model {
    /// Unlike cores:
    /// 74. the state features enabled on the core are, while a thread
    ///     runs, the baseline of the node's own kind and the optional
    ///     features of the thread's mask;
    /// 76. a thread's area is at least what its mask needs on the node's
    ///     own kind;
    /// 77. the instruction count of a counted context changed in the event
    ///     exactly when its consumed time did (on a machine with a counter;
    ///     without one it stays 0); an uncounted context has count 0 and
    ///     class 0;
    /// 78. the destination thread of a `thread_move` placed in the event
    ///     has effective marks that contain the source's.
    pub(super) fn check_cores(&self) -> Check {
        let list = self.machine.features();
        let kind = self.machine.own_kind(self.node);
        let baseline =
            (kind.features as u32) & list.state & !list.large & !(1 << FEATURE_USER_INTERRUPTS);
        if let Some(thread) = self.running() {
            let mask = self.threads[&thread].mask as u32;
            let optional = mask & list.state & !(1 << FEATURE_USER_INTERRUPTS);
            ensure(
                self.enabled == baseline | optional,
                "74: features enabled for the running thread",
            )?;
        }
        for thread in self.threads.values() {
            let needed = thread_bytes(FeatureMask::from_raw(thread.mask), list, &kind);
            ensure(
                needed.is_some_and(|n| n <= thread.area),
                "76: save area of a thread",
            )?;
        }
        for (id, sched) in &self.scheds {
            if !sched.counted {
                ensure(
                    sched.count == 0 && sched.hw_class == 0,
                    "77: count of an uncounted context",
                )?;
                continue;
            }
            // Counted since the start of the event, and not given new
            // values by a configuration or a move in it.
            let Some(&(consumed, count, true)) = self.event.counts.get(id) else {
                continue;
            };
            if self.event.resets.contains(id) {
                continue;
            }
            let time = sched.consumed != consumed;
            let counted = sched.count != count;
            let same = match self.machine.counter {
                true => time == counted,
                false => sched.count == 0,
            };
            ensure(
                same,
                "77: instructions and time charged at different events",
            )?;
        }
        for &(thread, marks) in &self.event.moved {
            let effective = self.marks(thread).map_or(0, |m| m.bits());
            ensure(effective & marks == marks, "78: marks lost by a move")?;
        }
        Ok(())
    }
}
