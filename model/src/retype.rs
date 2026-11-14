use dodos_abi::{Error, Rights, SLOT_BYTES};

use crate::caps::{check_dst_slot_empty, lookup_cap_in_cspace, lookup_cspace_cap};
use crate::state::{
    ASpaceObject, CSpaceObject, EdgeKind, EndpointObject, FrameObject, KernelObject, KernelState,
    NotifyObject, ReplyObject, ReplyState, Slot, ThreadObject, ThreadState, UntypedObject,
};
use crate::types::{CallOutcome, CallResult, ObjectId, RetypeSpec, SlotIndex, ThreadRegs};

/// Implements `untyped_retype`.
pub(crate) fn untyped_retype(
    state: &mut KernelState,
    caller_cspace: ObjectId,
    untyped_slot: SlotIndex,
    spec: RetypeSpec,
    dst_cspace_slot: SlotIndex,
    dst_slot: SlotIndex,
) -> CallOutcome {
    if let RetypeSpec::Unsupported(_) = spec {
        return CallOutcome::err(Error::Unsupported);
    }

    // CONTRACT-GAP: Required capability rights for `untyped_retype` are not
    // specified in `kernel-interface.md`; we require `WRITE` on both `untyped`
    // and `dst_cspace`.
    let (untyped_cap_id, untyped_entry) =
        match lookup_cap_in_cspace(state, caller_cspace, untyped_slot, Rights::WRITE) {
            Ok(v) => v,
            Err(e) => return CallOutcome::err(e),
        };
    let untyped_id = untyped_entry.object;
    let (u_size, u_watermark, u_is_device) = match state.objects.get(&untyped_id) {
        Some(KernelObject::Untyped(u)) => (u.size, u.watermark, u.is_device),
        Some(_) => return CallOutcome::err(Error::Invalid),
        None => return CallOutcome::err(Error::Gone),
    };

    if u_is_device && !matches!(spec, RetypeSpec::Frame { .. }) {
        // CONTRACT-GAP: `kernel-interface.md` forbids creating non-frame objects
        // from device untyped memory without naming the error; we return `Invalid`.
        return CallOutcome::err(Error::Invalid);
    }

    let dst_cspace = match lookup_cspace_cap(state, caller_cspace, dst_cspace_slot, Rights::WRITE) {
        Ok(id) => id,
        Err(e) => return CallOutcome::err(e),
    };
    if let Err(e) = check_dst_slot_empty(state, dst_cspace, dst_slot) {
        return CallOutcome::err(e);
    }

    // CONTRACT-GAP: `kernel-interface.md` (Object sizes) does not specify alignment
    // rules or the processor-dependent thread vector state area size; we align each
    // allocation deterministically and charge a flat 1024 bytes for `Thread`.
    let (alloc_size, align) = match spec {
        RetypeSpec::Untyped { bytes } => {
            if bytes == 0 {
                return CallOutcome::err(Error::Invalid);
            }
            (bytes, 64)
        }
        RetypeSpec::CSpace { slots } => {
            if slots == 0 {
                return CallOutcome::err(Error::Invalid);
            }
            let Some(bytes) = (slots as u64).checked_mul(SLOT_BYTES as u64) else {
                return CallOutcome::err(Error::Invalid);
            };
            (bytes, 32)
        }
        RetypeSpec::ASpace => (4096, 4096),
        RetypeSpec::Frame { page_size, count } => {
            if count == 0 {
                return CallOutcome::err(Error::Invalid);
            }
            let Some(bytes) = page_size.bytes().checked_mul(count as u64) else {
                return CallOutcome::err(Error::Invalid);
            };
            (bytes, page_size.bytes())
        }
        RetypeSpec::Thread => (1024, 1024),
        RetypeSpec::Endpoint | RetypeSpec::Notify | RetypeSpec::Reply => (64, 64),
        RetypeSpec::Unsupported(_) => unreachable!(),
    };

    // CONTRACT-GAP: `kernel-interface.md` does not specify whether `Untyped`
    // uses a bump pointer or free list; we use a seL4-style bump watermark.
    let rem = u_watermark % align;
    let aligned = if rem == 0 {
        Some(u_watermark)
    } else {
        u_watermark.checked_add(align - rem)
    };
    let end = aligned.and_then(|start| start.checked_add(alloc_size));
    let Some(new_watermark) = end.filter(|&e| e <= u_size) else {
        return CallOutcome::err(Error::NoMemory);
    };

    let new_obj = match spec {
        RetypeSpec::Untyped { bytes } => KernelObject::Untyped(UntypedObject {
            size: bytes,
            watermark: 0,
            is_device: u_is_device,
            backed_count: 0,
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::CSpace { slots } => KernelObject::CSpace(CSpaceObject {
            slots: vec![Slot::Empty; slots],
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::ASpace => KernelObject::ASpace(ASpaceObject {
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Frame { page_size, count } => KernelObject::Frame(FrameObject {
            page_size,
            count,
            is_device: u_is_device,
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Thread => KernelObject::Thread(ThreadObject {
            state: ThreadState::Suspended,
            cspace: None,
            aspace: None,
            fault_ep: None,
            regs: ThreadRegs::default(),
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Endpoint => KernelObject::Endpoint(EndpointObject {
            send_queue: Default::default(),
            recv_queue: Default::default(),
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Notify => KernelObject::Notify(NotifyObject {
            bits: 0,
            wait_queue: Default::default(),
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Reply => KernelObject::Reply(ReplyObject {
            state: ReplyState::Unarmed,
            parent_untyped: Some(untyped_id),
        }),
        RetypeSpec::Unsupported(_) => unreachable!(),
    };

    if let Some(KernelObject::Untyped(u)) = state.objects.get_mut(&untyped_id) {
        u.watermark = new_watermark;
        u.backed_count += 1;
    }

    let new_obj_id = state.alloc_object_id();
    state.objects.insert(new_obj_id, new_obj);

    // CONTRACT-GAP: `kernel-interface.md` does not specify initial rights on
    // retyped capabilities; we grant `Rights::ALL` (without `EXEC` on device frames).
    let init_rights = if u_is_device {
        Rights::from_bits(Rights::ALL.bits() & !Rights::EXEC.bits()).expect("valid rights mask")
    } else {
        Rights::ALL
    };

    crate::caps::insert_cap(
        state,
        crate::caps::NewCap {
            dst_cspace,
            dst_slot,
            object: new_obj_id,
            rights: init_rights,
            badge: 0,
            parent: Some(untyped_cap_id),
            edge: EdgeKind::Retyped,
        },
    );

    CallOutcome::completed(CallResult::ok())
}
