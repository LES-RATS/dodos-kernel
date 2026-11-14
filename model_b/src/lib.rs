//! Executable reference model B of the DodOS kernel, one core.
//!
//! Source of truth: `architecture/kernel-interface.md` in the documentation
//! repository. This crate is a specification that runs: a plain,
//! deterministic Rust program that says what the kernel must do, so the real
//! kernel can later be tested against it call by call on random call
//! sequences. It is not a kernel: no hardware, no `unsafe`, no threads, no
//! async, no clock, no randomness inside the model, std only.
//!
//! One state type, one entry point: [`State::apply`] applies a call made by
//! a given thread and returns the [`Outcome`] (error code, returned values,
//! whether the thread blocked, and any message delivered to it while it was
//! blocked). The test picks which runnable thread acts next
//! ([`State::runnable`]); there is no scheduling policy.
//!
//! Modelled, per the task scope:
//!
//! - untyped memory and `untyped_retype`, with the object kinds needed below
//!   (untyped, cspace, frame, thread, endpoint, notify; reply objects are
//!   created by `call`);
//! - capability spaces: the flat slot table, copy (`cap_derive`), move, delete;
//! - derivation with rights attenuation and the derivation tree;
//! - revocation: `cap_revoke` removes everything derived from a capability in
//!   bounded steps (`again`), objects whose last capability goes are
//!   destroyed, and a later call on a dead capability returns `gone`;
//! - threads at the level IPC needs: runnable, blocked on receive, blocked on
//!   reply, blocked on wait (blocked on send exists as a state but no call
//!   ever enters it: the contract says `send` never blocks);
//! - synchronous IPC on endpoints: `send`, `recv`, `reply_recv`, `call`,
//!   `send` on a reply object, message header, register/page words, badge,
//!   capability transfer all-or-nothing with `caps_dropped`;
//! - notifications: `signal`, `wait`, `poll`, accumulated badge bits.
//!
//! Out of scope (calls return `unsupported`): address spaces and mappings,
//! frames beyond retype, scheduling contexts, timers, interrupts, I/O ports,
//! IOMMU, tasks and marks (threads carry a fixed task id and empty marks,
//! the minimum IPC stamping needs), multi-core, debug objects, vcpu.
//!
//! Every place where the contract is silent, ambiguous or contradictory is
//! marked in the code with `// CONTRACT-GAP: ...`; the report of this model
//! lists them all with the section of the contract concerned.
//!
//! Ideas borrowed from seL4 / seL4 MCS are cited in comments where they
//! appear (derivation tree and revocation, untyped memory, flat cspace,
//! badges, reply objects); no code is copied or translated from any kernel.

#![forbid(unsafe_code)]

mod calls;
mod invariants;
mod state;
#[cfg(test)]
mod tests;

pub use calls::{Args, Outcome, Transfer};
pub use invariants::check_invariants;
pub use state::{
    BOOT_CSPACE_SLOTS, BOOT_DEVICE_BASE, BOOT_DEVICE_BYTES, BOOT_MEMORY_BASE, BOOT_MEMORY_BYTES,
    BOOT_SLOT_CSPACE, BOOT_SLOT_DEVICE, BOOT_SLOT_THREAD, BOOT_SLOT_UNTYPED, BOOT_THREAD, Cap,
    CapId, Endpoint, Inbox, Incoming, Notify, Object, REVOKE_STEP, Reply, ResolvedTransfer,
    SLOT_BYTES, SMALL_BYTES, SlotIdx, SlotState, State, THREAD_BYTES, Thread, ThreadId,
    ThreadState, TransferMode,
};
