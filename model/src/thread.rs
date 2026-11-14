use dodos_abi::{Error, Rights};

use crate::caps::{cancel_thread_ipc_wait, lookup_cap_in_cspace};
use crate::state::{KernelObject, KernelState, ThreadState};
use crate::types::{CallOutcome, CallResult, CallValue, ObjectId, SlotIndex, ThreadId, ThreadRegs};

fn lookup_thread(
    state: &KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
    required: Rights,
) -> Result<ThreadId, Error> {
    let (_, entry) = lookup_cap_in_cspace(state, caller_cspace, thread_slot, required)?;
    match state.objects.get(&entry.object) {
        Some(KernelObject::Thread(_)) => Ok(ThreadId(entry.object)),
        Some(_) => Err(Error::Invalid),
        None => Err(Error::Gone),
    }
}

/// Implements `thread_configure`.
pub(crate) fn thread_configure(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
    cspace_slot: SlotIndex,
    aspace_slot: Option<SlotIndex>,
    fault_ep_slot: Option<SlotIndex>,
) -> CallOutcome {
    // CONTRACT-GAP: `kernel-interface.md` does not specify exact arguments,
    // required rights, or permitted states for `thread_configure`. We require
    // `MANAGE` on the thread, require a valid `CSpace` (with `aspace` and
    // `fault_ep` optional), and reject configuration while blocked (`Busy`).
    let target_tid = match lookup_thread(state, caller_cspace, thread_slot, Rights::MANAGE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };

    let cspace_id = match lookup_cap_in_cspace(state, caller_cspace, cspace_slot, Rights::NONE) {
        Ok((_, e)) => match state.objects.get(&e.object) {
            Some(KernelObject::CSpace(_)) => e.object,
            Some(_) => return CallOutcome::err(Error::Invalid),
            None => return CallOutcome::err(Error::Gone),
        },
        Err(e) => return CallOutcome::err(e),
    };

    let aspace_id = match aspace_slot {
        Some(slot) => match lookup_cap_in_cspace(state, caller_cspace, slot, Rights::NONE) {
            Ok((_, e)) => match state.objects.get(&e.object) {
                Some(KernelObject::ASpace(_)) => Some(e.object),
                Some(_) => return CallOutcome::err(Error::Invalid),
                None => return CallOutcome::err(Error::Gone),
            },
            Err(e) => return CallOutcome::err(e),
        },
        None => None,
    };

    let fault_ep_id = match fault_ep_slot {
        Some(slot) => match lookup_cap_in_cspace(state, caller_cspace, slot, Rights::SEND) {
            Ok((_, e)) => match state.objects.get(&e.object) {
                Some(KernelObject::Endpoint(_)) => Some(e.object),
                Some(_) => return CallOutcome::err(Error::Invalid),
                None => return CallOutcome::err(Error::Gone),
            },
            Err(e) => return CallOutcome::err(e),
        },
        None => None,
    };

    let Some(KernelObject::Thread(t)) = state.objects.get_mut(&target_tid.0) else {
        return CallOutcome::err(Error::Gone);
    };
    match t.state {
        ThreadState::Suspended | ThreadState::Runnable => {}
        ThreadState::BlockedOnSend { .. }
        | ThreadState::BlockedOnReceive { .. }
        | ThreadState::BlockedOnReply { .. } => {
            return CallOutcome::err(Error::Busy);
        }
    }

    t.cspace = Some(cspace_id);
    if aspace_slot.is_some() {
        t.aspace = aspace_id;
    }
    if fault_ep_slot.is_some() {
        t.fault_ep = fault_ep_id;
    }

    CallOutcome::completed(CallResult::ok())
}

/// Implements `thread_read_regs`.
pub(crate) fn thread_read_regs(
    state: &KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
) -> CallOutcome {
    let target_tid = match lookup_thread(state, caller_cspace, thread_slot, Rights::READ) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let Some(KernelObject::Thread(t)) = state.objects.get(&target_tid.0) else {
        return CallOutcome::err(Error::Gone);
    };
    CallOutcome::completed(CallResult::ok_with(CallValue::Regs(t.regs)))
}

/// Implements `thread_write_regs`.
pub(crate) fn thread_write_regs(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
    regs: ThreadRegs,
) -> CallOutcome {
    let target_tid = match lookup_thread(state, caller_cspace, thread_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    let Some(KernelObject::Thread(t)) = state.objects.get_mut(&target_tid.0) else {
        return CallOutcome::err(Error::Gone);
    };
    // CONTRACT-GAP: `kernel-interface.md` does not specify whether writing registers
    // requires the thread to be suspended; we require `Suspended` and return `Busy` otherwise.
    if t.state != ThreadState::Suspended {
        return CallOutcome::err(Error::Busy);
    }
    t.regs = regs;
    CallOutcome::completed(CallResult::ok())
}

/// Implements `thread_resume`.
pub(crate) fn thread_resume(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
) -> CallOutcome {
    let target_tid = match lookup_thread(state, caller_cspace, thread_slot, Rights::MANAGE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };

    let (t_state, cspace_opt) = match state.objects.get(&target_tid.0) {
        Some(KernelObject::Thread(t)) => (t.state.clone(), t.cspace),
        _ => return CallOutcome::err(Error::Gone),
    };

    match t_state {
        ThreadState::Suspended => {
            let Some(cs_id) = cspace_opt else {
                return CallOutcome::err(Error::Invalid);
            };
            if !matches!(state.objects.get(&cs_id), Some(KernelObject::CSpace(_))) {
                return CallOutcome::err(Error::Gone);
            }
            if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&target_tid.0) {
                t.state = ThreadState::Runnable;
            }
            CallOutcome::completed(CallResult::ok())
        }
        ThreadState::Runnable => CallOutcome::completed(CallResult::ok()),
        ThreadState::BlockedOnSend { .. }
        | ThreadState::BlockedOnReceive { .. }
        | ThreadState::BlockedOnReply { .. } => CallOutcome::err(Error::Busy),
    }
}

/// Implements `thread_suspend`.
pub(crate) fn thread_suspend(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    thread_slot: SlotIndex,
) -> CallOutcome {
    let target_tid = match lookup_thread(state, caller_cspace, thread_slot, Rights::MANAGE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    cancel_thread_ipc_wait(state, target_tid);
    if let Some(KernelObject::Thread(t)) = state.objects.get_mut(&target_tid.0) {
        t.state = ThreadState::Suspended;
    }
    CallOutcome::completed(CallResult::ok())
}
