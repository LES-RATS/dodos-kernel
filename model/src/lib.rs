//! Executable reference model of the DodOS kernel (one core).
//!
//! Source of truth: `architecture/kernel-interface.md` in the documentation
//! repository. This crate models the deterministic state transitions of one
//! core's kernel for differential testing of the real kernel implementation.

#![forbid(unsafe_code)]

mod caps;
mod invariants;
mod ipc;
mod retype;
mod state;
mod thread;
mod types;

use dodos_abi::Error;

pub use invariants::InvariantError;
pub use state::{
    ASpaceObject, CSpaceObject, CapEntry, EdgeKind, EndpointObject, FrameObject, KernelObject,
    KernelState, NotifyObject, PendingMessage, RecvSource, ReplyObject, ReplyState, Slot,
    ThreadObject, ThreadState, UntypedObject,
};
pub use types::{
    CallOutcome, CallResult, CallStatus, CallValue, CapId, CapInfo, CapTransfer, KernelConfig,
    Message, ObjectId, ROOT_ASPACE_SLOT, ROOT_CSPACE_SLOT, ROOT_DEVICE_UNTYPED_SLOT,
    ROOT_THREAD_SLOT, ROOT_UNTYPED_SLOT, RetypeSpec, SlotIndex, SlotRange, SysCall, ThreadId,
    ThreadRegs, Wakeup,
};

impl KernelState {
    /// Applies one system call invoked by `caller` and returns the immediate
    /// result (or `Blocked`) together with any threads woken up.
    pub fn apply(&mut self, caller: ThreadId, call: SysCall) -> CallOutcome {
        let caller_cspace = match self.objects.get(&caller.0) {
            Some(KernelObject::Thread(t)) => match t.state {
                ThreadState::Runnable => match t.cspace {
                    Some(cs) => cs,
                    None => return CallOutcome::err(Error::Invalid),
                },
                ThreadState::Suspended
                | ThreadState::BlockedOnSend { .. }
                | ThreadState::BlockedOnReceive { .. }
                | ThreadState::BlockedOnReply { .. } => {
                    return CallOutcome::err(Error::Busy);
                }
            },
            Some(_) => return CallOutcome::err(Error::Invalid),
            None => return CallOutcome::err(Error::Gone),
        };

        match call {
            SysCall::CapDerive {
                src_cspace,
                src_slot,
                dst_cspace,
                dst_slot,
                rights,
                badge,
            } => caps::cap_derive(
                self,
                caller_cspace,
                (src_cspace, src_slot),
                (dst_cspace, dst_slot),
                rights,
                badge,
            ),
            SysCall::CapMove {
                src_cspace,
                src_slot,
                dst_cspace,
                dst_slot,
            } => caps::cap_move(
                self,
                caller_cspace,
                src_cspace,
                src_slot,
                dst_cspace,
                dst_slot,
            ),
            SysCall::CapDelete { cspace, slot } => {
                caps::cap_delete(self, caller_cspace, cspace, slot)
            }
            SysCall::CapRevoke { cspace, slot } => {
                caps::cap_revoke(self, caller_cspace, cspace, slot)
            }
            SysCall::CapInspect { cspace, slot } => {
                caps::cap_inspect(self, caller_cspace, cspace, slot)
            }
            SysCall::UntypedRetype {
                untyped,
                spec,
                dst_cspace,
                dst_slot,
            } => retype::untyped_retype(self, caller_cspace, untyped, spec, dst_cspace, dst_slot),
            SysCall::Call {
                endpoint,
                msg,
                reply_recv_slots,
            } => ipc::call(self, caller, caller_cspace, endpoint, msg, reply_recv_slots),
            SysCall::ReplyRecv {
                reply,
                reply_msg,
                endpoint,
                recv_slots,
            } => ipc::reply_recv(
                self,
                caller,
                caller_cspace,
                reply,
                reply_msg,
                endpoint,
                recv_slots,
            ),
            SysCall::Recv {
                endpoint,
                reply,
                recv_slots,
            } => ipc::recv(self, caller, caller_cspace, endpoint, reply, recv_slots),
            SysCall::Send { target, msg } => ipc::send(self, caller_cspace, target, msg),
            SysCall::Signal { notify } => ipc::signal(self, caller_cspace, notify),
            SysCall::Wait { target } => ipc::wait(self, caller, caller_cspace, target),
            SysCall::Poll { target } => ipc::poll(self, caller_cspace, target),
            SysCall::ThreadConfigure {
                thread,
                cspace,
                aspace,
                fault_ep,
            } => thread::thread_configure(self, caller_cspace, thread, cspace, aspace, fault_ep),
            SysCall::ThreadReadRegs { thread } => {
                thread::thread_read_regs(self, caller_cspace, thread)
            }
            SysCall::ThreadWriteRegs { thread, regs } => {
                thread::thread_write_regs(self, caller_cspace, thread, regs)
            }
            SysCall::ThreadResume { thread } => thread::thread_resume(self, caller_cspace, thread),
            SysCall::ThreadSuspend { thread } => {
                thread::thread_suspend(self, caller_cspace, thread)
            }
            SysCall::Yield => CallOutcome::completed(CallResult::ok()),
            SysCall::Unsupported(_) => CallOutcome::err(Error::Unsupported),
        }
    }

    /// Verifies all structural, capability, lifecycle, and IPC invariants over
    /// the entire state. See [`invariants::check_invariants`] for the full list.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        invariants::check_invariants(self)
    }
}
