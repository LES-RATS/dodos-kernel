use dodos_abi::{Call, Error, Header, MAX_WORDS, Marks, ObjectKind, PageSize, REG_WORDS, Rights};
use dodos_model::{
    CallResult, CallStatus, CallValue, CapInfo, CapTransfer, KernelConfig, KernelState, Message,
    ROOT_ASPACE_SLOT, ROOT_CSPACE_SLOT, ROOT_DEVICE_UNTYPED_SLOT, ROOT_UNTYPED_SLOT, RetypeSpec,
    SlotIndex, SlotRange, SysCall, ThreadId, ThreadRegs,
};

fn apply_ok(state: &mut KernelState, tid: ThreadId, call: SysCall) {
    let outcome = state.apply(tid, call);
    assert_eq!(
        outcome.status,
        CallStatus::Completed(CallResult::ok()),
        "expected Ok, got {:?}",
        outcome
    );
    state.check_invariants().expect("invariants must hold");
}

fn apply_err(state: &mut KernelState, tid: ThreadId, call: SysCall, err: Error) {
    let outcome = state.apply(tid, call);
    assert_eq!(
        outcome.status,
        CallStatus::Completed(CallResult::err(err)),
        "expected {:?}, got {:?}",
        err,
        outcome
    );
    state.check_invariants().expect("invariants must hold");
}

fn spawn_thread(
    state: &mut KernelState,
    root: ThreadId,
    cspace_slot: SlotIndex,
    thread_slot: SlotIndex,
    cspace_cap_in_child_slot: SlotIndex,
    slots: usize,
) -> ThreadId {
    apply_ok(
        state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: cspace_slot,
        },
    );
    apply_ok(
        state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Thread,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: thread_slot,
        },
    );
    apply_ok(
        state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: cspace_slot,
            dst_cspace: cspace_slot,
            dst_slot: cspace_cap_in_child_slot,
            rights: Rights::ALL,
            badge: 0,
        },
    );
    apply_ok(
        state,
        root,
        SysCall::ThreadConfigure {
            thread: thread_slot,
            cspace: cspace_slot,
            aspace: Some(ROOT_ASPACE_SLOT),
            fault_ep: None,
        },
    );
    apply_ok(
        state,
        root,
        SysCall::ThreadResume {
            thread: thread_slot,
        },
    );
    *state
        .runnable_threads()
        .iter()
        .max()
        .expect("new thread is runnable")
}

#[test]
fn bootstrap_and_unsupported_calls() {
    let mut state = KernelState::new(KernelConfig::default());
    state.check_invariants().unwrap();
    let root = state.root_thread().unwrap();
    assert_eq!(state.runnable_threads(), vec![root]);

    for call in [
        Call::ASpaceMap,
        Call::ASpaceUnmap,
        Call::ASpaceProtect,
        Call::SchedConfigure,
        Call::SchedBind,
        Call::SchedUnbind,
        Call::SchedSetCores,
        Call::TimerSet,
        Call::TimerCancel,
        Call::IrqBind,
        Call::IrqAck,
        Call::IoSpaceMap,
        Call::IoSpaceUnmap,
        Call::IoSpaceAttach,
        Call::TaskCreate,
        Call::TaskAttach,
        Call::TaskMark,
        Call::TaskRevoke,
        Call::TaskRead,
        Call::CoreStart,
        Call::CoreStop,
        Call::CoreStatus,
        Call::DebugWrite,
        Call::TraceEmit,
    ] {
        apply_err(
            &mut state,
            root,
            SysCall::Unsupported(call),
            Error::Unsupported,
        );
    }

    apply_ok(&mut state, root, SysCall::Yield);
}

#[test]
fn untyped_retype_all_kinds_and_errors() {
    let mut state = KernelState::new(KernelConfig {
        init_untyped_bytes: 8192,
        init_device_untyped_bytes: Some(8192),
        ..KernelConfig::default()
    });
    let root = state.root_thread().unwrap();

    // Unsupported object kind.
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Unsupported(ObjectKind::Sched),
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::Unsupported,
    );

    // Invalid counts/sizes.
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Untyped { bytes: 0 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 0 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Frame {
                page_size: PageSize::Size4K,
                count: 0,
            },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::Invalid,
    );

    // Device untyped cannot create kernel objects, only frames without EXEC.
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_DEVICE_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::Invalid,
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_DEVICE_UNTYPED_SLOT,
            spec: RetypeSpec::Frame {
                page_size: PageSize::Size4K,
                count: 1,
            },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    let inspect = state.apply(
        root,
        SysCall::CapInspect {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    assert_eq!(
        inspect.status,
        CallStatus::Completed(CallResult::ok_with(CallValue::Inspect(CapInfo {
            kind: ObjectKind::Frame,
            rights: Rights::from_bits(Rights::ALL.bits() & !Rights::EXEC.bits()).unwrap(),
            badge: 0,
            is_root: false,
            child_count: 0,
        })))
    );

    // Wrong source type / occupied destination / out-of-bounds destination.
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_CSPACE_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::NoSlot,
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 999,
        },
        Error::Invalid,
    );

    // Exhaust ordinary untyped (8192 bytes) with a 4K frame + 4K ASpace, then hit NoMemory.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Frame {
                page_size: PageSize::Size4K,
                count: 1,
            },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::ASpace,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
        Error::NoMemory,
    );

    // Delete both objects backed by ROOT_UNTYPED_SLOT -> watermark resets -> allocation succeeds!
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 12,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Untyped { bytes: 2048 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
    );

    // Denied if untyped lacks WRITE.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: ROOT_UNTYPED_SLOT,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 14,
            rights: Rights::READ,
            badge: 0,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: 14,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 15,
        },
        Error::Denied,
    );
}

#[test]
fn derive_move_delete_inspect_and_badge_rules() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Thread,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );

    // Non-badgeable object rejects non-zero badge.
    apply_err(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 11,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
            rights: Rights::READ,
            badge: 42,
        },
        Error::Invalid,
    );

    // Derive badged endpoint capability into slot 12.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
            rights: Rights::SEND.union(Rights::DERIVE),
            badge: 0xbeef,
        },
    );

    // Cannot re-badge to a different value.
    apply_err(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
            rights: Rights::SEND,
            badge: 0xcafe,
        },
        Error::Denied,
    );

    // Badge 0 preserves parent's non-zero badge; cannot amplify rights.
    apply_err(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
            rights: Rights::RECV,
            badge: 0,
        },
        Error::Denied,
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    let info = state.apply(
        root,
        SysCall::CapInspect {
            cspace: ROOT_CSPACE_SLOT,
            slot: 13,
        },
    );
    assert_eq!(
        info.status,
        CallStatus::Completed(CallResult::ok_with(CallValue::Inspect(CapInfo {
            kind: ObjectKind::Endpoint,
            rights: Rights::SEND,
            badge: 0xbeef,
            is_root: false,
            child_count: 0,
        })))
    );

    // Derived cap without DERIVE cannot derive further, even with NONE rights.
    apply_err(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 13,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 14,
            rights: Rights::NONE,
            badge: 0,
        },
        Error::Denied,
    );

    // Move works even without DERIVE, preserving badge and clearing source slot.
    apply_ok(
        &mut state,
        root,
        SysCall::CapMove {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 13,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 14,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapInspect {
            cspace: ROOT_CSPACE_SLOT,
            slot: 13,
        },
        Error::Invalid,
    );

    // Delete middle node (slot 12) reparents slot 14 to slot 10; endpoint stays alive.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 12,
        },
    );
    let root_ep_info = state.apply(
        root,
        SysCall::CapInspect {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    assert_eq!(
        root_ep_info.status,
        CallStatus::Completed(CallResult::ok_with(CallValue::Inspect(CapInfo {
            kind: ObjectKind::Endpoint,
            rights: Rights::ALL,
            badge: 0,
            is_root: false,
            child_count: 1,
        })))
    );
}

#[test]
fn bounded_revoke_returns_again_and_marks_slots_gone() {
    let mut state = KernelState::new(KernelConfig {
        revoke_step_budget: 3,
        ..KernelConfig::default()
    });
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );

    // Derive a chain of 8 capabilities under slot 10.
    for i in 0..8 {
        let parent = if i == 0 { 10 } else { 10 + i };
        apply_ok(
            &mut state,
            root,
            SysCall::CapDerive {
                src_cspace: ROOT_CSPACE_SLOT,
                src_slot: parent,
                dst_cspace: ROOT_CSPACE_SLOT,
                dst_slot: 11 + i,
                rights: Rights::ALL,
                badge: 0,
            },
        );
    }

    // Budget is 3: requires 3 calls (3 + 3 + 2) before returning Ok.
    apply_err(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
        Error::Again,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
        Error::Again,
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );

    // Revoked slots return Gone when used, and cannot be overwritten until CapDelete clears them.
    apply_err(
        &mut state,
        root,
        SysCall::CapInspect {
            cspace: ROOT_CSPACE_SLOT,
            slot: 15,
        },
        Error::Gone,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Send {
            target: 15,
            msg: Message::empty(1),
        },
        Error::Gone,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 15,
            rights: Rights::SEND,
            badge: 0,
        },
        Error::NoSlot,
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 15,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 15,
            rights: Rights::SEND,
            badge: 0,
        },
    );
}

#[test]
fn bounded_delete_cspace_and_cspace_cycle() {
    let mut state = KernelState::new(KernelConfig {
        revoke_step_budget: 2,
        ..KernelConfig::default()
    });
    let root = state.root_thread().unwrap();

    // Create child CSpace in slot 10 with 6 slots, fill 5 slots with Endpoints.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 6 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    for idx in 0..5 {
        apply_ok(
            &mut state,
            root,
            SysCall::UntypedRetype {
                untyped: ROOT_UNTYPED_SLOT,
                spec: RetypeSpec::Endpoint,
                dst_cspace: 10,
                dst_slot: idx,
            },
        );
    }

    // Deleting slot 10 tears down the CSpace in batches of 2 -> Again, Again, Ok.
    apply_err(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
        Error::Again,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
        Error::Again,
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );

    // Self-referential CSpace cycle: C1 holds a cap to C2, an Endpoint, and a cap to itself.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 4 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 4 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapMove {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 11,
            dst_cspace: 10,
            dst_slot: 0,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: 10,
            dst_slot: 1,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 10,
            dst_slot: 2,
            rights: Rights::ALL,
            badge: 0,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    loop {
        let out = state.apply(
            root,
            SysCall::CapRevoke {
                cspace: ROOT_CSPACE_SLOT,
                slot: ROOT_UNTYPED_SLOT,
            },
        );
        state.check_invariants().unwrap();
        match out.status {
            CallStatus::Completed(CallResult {
                error: Error::Again,
                ..
            }) => continue,
            CallStatus::Completed(CallResult {
                error: Error::Ok, ..
            }) => break,
            other => panic!("unexpected revoke result: {:?}", other),
        }
    }
}

#[test]
fn ipc_call_recv_reply_recv_fast_and_slow_and_cap_transfer() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );

    let server = spawn_thread(&mut state, root, 20, 21, 0, 16);
    let client = spawn_thread(&mut state, root, 22, 23, 0, 16);

    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 20,
            dst_slot: 1,
            rights: Rights::RECV.union(Rights::GRANT),
            badge: 0,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapMove {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 11,
            dst_cspace: 20,
            dst_slot: 2,
        },
    );

    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 22,
            dst_slot: 1,
            rights: Rights::SEND.union(Rights::GRANT),
            badge: 77,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: 22,
            dst_slot: 2,
        },
    );

    // 1. Server calls Recv first -> blocks.
    let srv_out = state.apply(
        server,
        SysCall::Recv {
            endpoint: 1,
            reply: 2,
            recv_slots: SlotRange::new(4, 2),
        },
    );
    assert_eq!(srv_out.status, CallStatus::Blocked);
    assert!(!state.runnable_threads().contains(&server));
    state.check_invariants().unwrap();

    // 2. Client calls fast path (<= REG_WORDS, 0 caps) -> wakes server, blocks client on Reply.
    let fast_words = (0..REG_WORDS as u64).collect::<Vec<_>>();
    let fast_msg = Message::new(0x1234, fast_words.clone(), vec![], 0).unwrap();
    assert!(fast_msg.header.is_fast());
    let cli_out = state.apply(
        client,
        SysCall::Call {
            endpoint: 1,
            msg: fast_msg,
            reply_recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(cli_out.status, CallStatus::Blocked);
    assert_eq!(cli_out.woken.len(), 1);
    assert_eq!(cli_out.woken[0].thread, server);
    assert_eq!(
        cli_out.woken[0].result,
        CallResult::ok_with(CallValue::Received {
            info: dodos_abi::Received {
                header: Header::new(0x1234, REG_WORDS, 0, 0).unwrap(),
                badge: 77,
                marks: Marks::NONE,
                task: 0,
            },
            words: fast_words,
            caps_transferred: 0,
        })
    );
    state.check_invariants().unwrap();

    // 3. Server uses ReplyRecv -> wakes client with reply, then blocks for next message.
    let reply_msg = Message::new(99, vec![10, 20], vec![], 0).unwrap();
    let rr_out = state.apply(
        server,
        SysCall::ReplyRecv {
            reply: 2,
            reply_msg,
            endpoint: 1,
            recv_slots: SlotRange::new(4, 1),
        },
    );
    assert_eq!(rr_out.status, CallStatus::Blocked);
    assert_eq!(rr_out.woken.len(), 1);
    assert_eq!(rr_out.woken[0].thread, client);
    assert_eq!(
        rr_out.woken[0].result,
        CallResult::ok_with(CallValue::Reply {
            header: Header::new(99, 2, 0, 0).unwrap(),
            words: vec![10, 20],
            caps_transferred: 0,
        })
    );
    state.check_invariants().unwrap();

    // 4. Duplicate slot in Move/Derive in one message is rejected with Invalid.
    let dup_msg = Message::new(
        55,
        vec![1; MAX_WORDS],
        vec![
            CapTransfer::Derive {
                slot: 2,
                rights: Rights::SEND,
            },
            CapTransfer::Move { slot: 2 },
        ],
        0,
    )
    .unwrap();
    apply_err(
        &mut state,
        client,
        SysCall::Call {
            endpoint: 1,
            msg: dup_msg,
            reply_recv_slots: SlotRange::empty(),
        },
        Error::Invalid,
    );

    // Put a second Notify in client slot 3 and send [Derive(2), Move(3)] to test caps_dropped.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: 22,
            dst_slot: 3,
        },
    );
    let drop_msg = Message::new(
        56,
        vec![7; MAX_WORDS],
        vec![
            CapTransfer::Derive {
                slot: 2,
                rights: Rights::SEND,
            },
            CapTransfer::Move { slot: 3 },
        ],
        0,
    )
    .unwrap();
    let drop_out = state.apply(
        client,
        SysCall::Call {
            endpoint: 1,
            msg: drop_msg,
            reply_recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(drop_out.status, CallStatus::Blocked);
    assert_eq!(drop_out.woken.len(), 1);
    match &drop_out.woken[0].result.value {
        CallValue::Received {
            info,
            words,
            caps_transferred,
        } => {
            assert_eq!(info.header.flags(), Header::FLAG_CAPS_DROPPED);
            assert_eq!(info.header.caps(), 0);
            assert_eq!(*caps_transferred, 0);
            assert_eq!(words.len(), MAX_WORDS);
        }
        other => panic!("unexpected value: {:?}", other),
    }
    // Client's slot 3 was NOT moved (all-or-nothing!); root inspects client's CSpace (slot 22).
    let inspect_3 = state.apply(
        root,
        SysCall::CapInspect {
            cspace: 22,
            slot: 3,
        },
    );
    assert!(matches!(
        inspect_3.status,
        CallStatus::Completed(CallResult {
            error: Error::Ok,
            ..
        })
    ));

    // Server replies via ReplyRecv -> caller is told `FLAG_CAPS_DROPPED` in the reply!
    let rr2 = state.apply(
        server,
        SysCall::ReplyRecv {
            reply: 2,
            reply_msg: Message::empty(88),
            endpoint: 1,
            recv_slots: SlotRange::new(4, 2),
        },
    );
    assert_eq!(rr2.status, CallStatus::Blocked);
    assert_eq!(rr2.woken.len(), 1);
    assert_eq!(
        rr2.woken[0].result,
        CallResult::ok_with(CallValue::Reply {
            header: Header::new(88, 0, 0, Header::FLAG_CAPS_DROPPED).unwrap(),
            words: vec![],
            caps_transferred: 0,
        })
    );

    // 5. Server now has 2 slots (4..6): client sends [Derive(2), Move(3)] -> both transfer!
    let ok_msg = Message::new(
        57,
        vec![42],
        vec![
            CapTransfer::Derive {
                slot: 2,
                rights: Rights::SEND,
            },
            CapTransfer::Move { slot: 3 },
        ],
        0,
    )
    .unwrap();
    let ok_out = state.apply(
        client,
        SysCall::Call {
            endpoint: 1,
            msg: ok_msg,
            reply_recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(ok_out.status, CallStatus::Blocked);
    assert_eq!(ok_out.woken.len(), 1);
    match &ok_out.woken[0].result.value {
        CallValue::Received {
            info,
            caps_transferred,
            ..
        } => {
            assert_eq!(info.header.flags(), 0);
            assert_eq!(info.header.caps(), 2);
            assert_eq!(*caps_transferred, 2);
        }
        other => panic!("unexpected value: {:?}", other),
    }

    // Client's slot 3 is now empty (moved); root inspects while client is BlockedOnReply.
    apply_err(
        &mut state,
        root,
        SysCall::CapInspect {
            cspace: 22,
            slot: 3,
        },
        Error::Invalid,
    );

    // Server uses one-way Send on Reply to answer client.
    let oneway_reply = state.apply(
        server,
        SysCall::Send {
            target: 2,
            msg: Message::empty(100),
        },
    );
    assert_eq!(oneway_reply.status, CallStatus::Completed(CallResult::ok()));
    assert_eq!(oneway_reply.woken.len(), 1);
    assert_eq!(oneway_reply.woken[0].thread, client);
    apply_err(
        &mut state,
        server,
        SysCall::Send {
            target: 2,
            msg: Message::empty(101),
        },
        Error::Invalid,
    );

    // Client is runnable again: revokes slot 2 -> revokes the derived cap in server's slot 4!
    apply_ok(
        &mut state,
        client,
        SysCall::CapRevoke { cspace: 0, slot: 2 },
    );
    apply_err(
        &mut state,
        server,
        SysCall::CapInspect { cspace: 0, slot: 4 },
        Error::Gone,
    );
    state.check_invariants().unwrap();
}

#[test]
fn send_never_blocks_and_ipc_teardown_wakes_with_gone() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );

    apply_err(
        &mut state,
        root,
        SysCall::Send {
            target: 10,
            msg: Message::empty(1),
        },
        Error::WouldWait,
    );

    let client = spawn_thread(&mut state, root, 20, 21, 0, 8);
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 20,
            dst_slot: 1,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    let call_out = state.apply(
        client,
        SysCall::Call {
            endpoint: 1,
            msg: Message::empty(7),
            reply_recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(call_out.status, CallStatus::Blocked);

    apply_ok(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    let del_out = state.apply(
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    assert_eq!(del_out.status, CallStatus::Completed(CallResult::ok()));
    assert_eq!(del_out.woken.len(), 1);
    assert_eq!(del_out.woken[0].thread, client);
    assert_eq!(del_out.woken[0].result, CallResult::err(Error::Gone));
    state.check_invariants().unwrap();
}

#[test]
fn notifications_signal_wait_poll_and_teardown() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
            rights: Rights::SEND,
            badge: 0b0101,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
            rights: Rights::SEND,
            badge: 0b1010,
        },
    );

    apply_err(
        &mut state,
        root,
        SysCall::Poll { target: 10 },
        Error::WouldWait,
    );

    apply_ok(&mut state, root, SysCall::Signal { notify: 11 });
    apply_ok(&mut state, root, SysCall::Signal { notify: 12 });
    let poll_out = state.apply(root, SysCall::Poll { target: 10 });
    assert_eq!(
        poll_out.status,
        CallStatus::Completed(CallResult::ok_with(CallValue::NotifyBits(0b1111)))
    );
    apply_err(
        &mut state,
        root,
        SysCall::Poll { target: 10 },
        Error::WouldWait,
    );

    let waiter = spawn_thread(&mut state, root, 20, 21, 0, 8);
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 20,
            dst_slot: 1,
            rights: Rights::RECV,
            badge: 0,
        },
    );
    let wait_out = state.apply(waiter, SysCall::Wait { target: 1 });
    assert_eq!(wait_out.status, CallStatus::Blocked);

    let zero_sig = state.apply(root, SysCall::Signal { notify: 10 });
    assert!(zero_sig.woken.is_empty());

    let sig_out = state.apply(root, SysCall::Signal { notify: 11 });
    assert_eq!(sig_out.woken.len(), 1);
    assert_eq!(sig_out.woken[0].thread, waiter);
    assert_eq!(
        sig_out.woken[0].result,
        CallResult::ok_with(CallValue::NotifyBits(0b0101))
    );

    let wait2 = state.apply(waiter, SysCall::Wait { target: 1 });
    assert_eq!(wait2.status, CallStatus::Blocked);
    apply_ok(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    let del_ntfn = state.apply(
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 10,
        },
    );
    assert_eq!(del_ntfn.woken.len(), 1);
    assert_eq!(del_ntfn.woken[0].result, CallResult::err(Error::Gone));
    state.check_invariants().unwrap();
}

#[test]
fn thread_regs_suspend_and_reply_caller_gone() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Thread,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 8 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
    );

    apply_err(
        &mut state,
        root,
        SysCall::ThreadResume { thread: 10 },
        Error::Invalid,
    );

    let regs = ThreadRegs {
        pc: 0x4000_1000,
        sp: 0x7fff_0000,
        args: [1, 2, 3, 4, 5, 6],
    };
    apply_ok(
        &mut state,
        root,
        SysCall::ThreadWriteRegs { thread: 10, regs },
    );
    let read_out = state.apply(root, SysCall::ThreadReadRegs { thread: 10 });
    assert_eq!(
        read_out.status,
        CallStatus::Completed(CallResult::ok_with(CallValue::Regs(regs)))
    );

    apply_ok(
        &mut state,
        root,
        SysCall::ThreadConfigure {
            thread: 10,
            cspace: 11,
            aspace: None,
            fault_ep: Some(12),
        },
    );
    apply_ok(&mut state, root, SysCall::ThreadResume { thread: 10 });
    let worker = state.runnable_threads()[1];

    apply_err(
        &mut state,
        root,
        SysCall::ThreadWriteRegs { thread: 10, regs },
        Error::Busy,
    );

    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: 11,
            dst_slot: 0,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    let call_out = state.apply(
        worker,
        SysCall::Call {
            endpoint: 0,
            msg: Message::empty(5),
            reply_recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(call_out.status, CallStatus::Blocked);
    let recv_out = state.apply(
        root,
        SysCall::Recv {
            endpoint: 12,
            reply: 13,
            recv_slots: SlotRange::empty(),
        },
    );
    assert!(matches!(
        recv_out.status,
        CallStatus::Completed(CallResult {
            error: Error::Ok,
            value: CallValue::Received { .. }
        })
    ));

    apply_ok(&mut state, root, SysCall::ThreadSuspend { thread: 10 });
    apply_err(
        &mut state,
        root,
        SysCall::Send {
            target: 13,
            msg: Message::empty(9),
        },
        Error::Gone,
    );
    state.check_invariants().unwrap();
}

#[test]
fn queued_sender_with_revoked_cap_and_reply_destruction() {
    let mut state = KernelState::new(KernelConfig::default());
    let root = state.root_thread().unwrap();

    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
    );

    let c1 = spawn_thread(&mut state, root, 20, 21, 0, 8);
    let c2 = spawn_thread(&mut state, root, 22, 23, 0, 8);
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 20,
            dst_slot: 1,
            rights: Rights::SEND.union(Rights::GRANT),
            badge: 1,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 22,
            dst_slot: 1,
            rights: Rights::SEND,
            badge: 2,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: 20,
            dst_slot: 2,
            rights: Rights::SEND,
            badge: 0,
        },
    );

    // c1 blocks in Call carrying slot 2 (Notify cap).
    let msg1 = Message::new(11, vec![], vec![CapTransfer::Move { slot: 2 }], 0).unwrap();
    assert_eq!(
        state
            .apply(
                c1,
                SysCall::Call {
                    endpoint: 1,
                    msg: msg1,
                    reply_recv_slots: SlotRange::empty(),
                },
            )
            .status,
        CallStatus::Blocked
    );
    // c2 blocks in Call behind c1.
    assert_eq!(
        state
            .apply(
                c2,
                SysCall::Call {
                    endpoint: 1,
                    msg: Message::empty(22),
                    reply_recv_slots: SlotRange::empty(),
                },
            )
            .status,
        CallStatus::Blocked
    );

    // Root revokes slot 12 -> c1's slot 2 becomes Revoked while c1 is BlockedOnSend!
    apply_ok(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: ROOT_CSPACE_SLOT,
            slot: 12,
        },
    );

    // Root calls Recv -> skips c1 (waking c1 with Gone), delivers c2's message!
    let recv_out = state.apply(
        root,
        SysCall::Recv {
            endpoint: 10,
            reply: 11,
            recv_slots: SlotRange::new(13, 1),
        },
    );
    assert_eq!(recv_out.woken.len(), 1);
    assert_eq!(recv_out.woken[0].thread, c1);
    assert_eq!(recv_out.woken[0].result, CallResult::err(Error::Gone));
    match recv_out.status {
        CallStatus::Completed(CallResult {
            error: Error::Ok,
            value: CallValue::Received { info, .. },
        }) => {
            assert_eq!(info.header.label(), 22);
            assert_eq!(info.badge, 2);
        }
        other => panic!("expected c2 message, got {:?}", other),
    }

    // Now c2 is BlockedOnReply on slot 11; deleting slot 11 wakes c2 with Gone!
    let del_reply = state.apply(
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 11,
        },
    );
    assert_eq!(del_reply.woken.len(), 1);
    assert_eq!(del_reply.woken[0].thread, c2);
    assert_eq!(del_reply.woken[0].result, CallResult::err(Error::Gone));
    state.check_invariants().unwrap();
}

#[test]
fn all_remaining_error_paths() {
    let mut state = KernelState::new(KernelConfig {
        revoke_step_budget: 2,
        ..KernelConfig::default()
    });
    let root = state.root_thread().unwrap();

    // Create Endpoint(10), Reply(11), Notify(12), ChildCSpace(13), WorkerThread(14), SubUntyped(15).
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 4 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Thread,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 14,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Untyped { bytes: 4096 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 15,
        },
    );

    // Derive read-only CSpace cap (slot 16) and test Denied on CapMove/CapDelete/CapRevoke/CapInspect.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: ROOT_CSPACE_SLOT,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 16,
            rights: Rights::READ,
            badge: 0,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapMove {
            src_cspace: 16,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 17,
        },
        Error::Denied,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: 16,
            slot: 10,
        },
        Error::Denied,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapRevoke {
            cspace: 16,
            slot: 10,
        },
        Error::Denied,
    );
    // Write-only CSpace cap (slot 17) cannot CapInspect.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: ROOT_CSPACE_SLOT,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 17,
            rights: Rights::WRITE,
            badge: 0,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapInspect {
            cspace: 17,
            slot: 10,
        },
        Error::Denied,
    );

    // CapDelete on Empty -> Invalid; CapMove onto occupied slot -> NoSlot.
    apply_err(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 18,
        },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::CapMove {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        Error::NoSlot,
    );

    // Missing GRANT when sending caps -> Denied; missing SEND/RECV -> Denied; wrong object -> Invalid.
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 18,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    let cap_msg = Message::new(1, vec![], vec![CapTransfer::Move { slot: 12 }], 0).unwrap();
    apply_err(
        &mut state,
        root,
        SysCall::Call {
            endpoint: 18,
            msg: cap_msg,
            reply_recv_slots: SlotRange::empty(),
        },
        Error::Denied,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Recv {
            endpoint: 18,
            reply: 11,
            recv_slots: SlotRange::empty(),
        },
        Error::Denied,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Wait { target: 10 },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Poll { target: 10 },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Signal { notify: 10 },
        Error::Invalid,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Recv {
            endpoint: 10,
            reply: 11,
            recv_slots: SlotRange::new(0, 2), // Occupied!
        },
        Error::NoSlot,
    );
    apply_err(
        &mut state,
        root,
        SysCall::Recv {
            endpoint: 10,
            reply: 11,
            recv_slots: SlotRange::new(63, 2), // Out of bounds!
        },
        Error::Invalid,
    );

    // Configure & resume worker, block it on Recv with Reply(11), then verify Busy on Reply(11) & ThreadConfigure.
    apply_ok(
        &mut state,
        root,
        SysCall::ThreadConfigure {
            thread: 14,
            cspace: 13,
            aspace: None,
            fault_ep: None,
        },
    );
    apply_ok(&mut state, root, SysCall::ThreadResume { thread: 14 });
    let worker = state.runnable_threads()[1];
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 13,
            dst_slot: 0,
            rights: Rights::RECV,
            badge: 0,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 11,
            dst_cspace: 13,
            dst_slot: 1,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    assert_eq!(
        state
            .apply(
                worker,
                SysCall::Recv {
                    endpoint: 0,
                    reply: 1,
                    recv_slots: SlotRange::empty(),
                },
            )
            .status,
        CallStatus::Blocked
    );
    // Reusing Reply(11) while WaitingRecv returns Busy.
    apply_err(
        &mut state,
        root,
        SysCall::Recv {
            endpoint: 10,
            reply: 11,
            recv_slots: SlotRange::empty(),
        },
        Error::Busy,
    );
    // ThreadConfigure while BlockedOnReceive returns Busy; Yield on blocked thread returns Busy.
    apply_err(
        &mut state,
        root,
        SysCall::ThreadConfigure {
            thread: 14,
            cspace: 13,
            aspace: None,
            fault_ep: None,
        },
        Error::Busy,
    );
    apply_err(&mut state, worker, SysCall::Yield, Error::Busy);

    // Suspending worker clears Reply(11) back to Unarmed, so ReplyRecv skips reply and blocks cleanly.
    apply_ok(&mut state, root, SysCall::ThreadSuspend { thread: 14 });
    let rr = state.apply(
        root,
        SysCall::ReplyRecv {
            reply: 11,
            reply_msg: Message::empty(1),
            endpoint: 10,
            recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(rr.status, CallStatus::Blocked);
    state.check_invariants().unwrap();
}

#[test]
fn more_lifecycle_edge_cases() {
    let mut state = KernelState::new(KernelConfig {
        revoke_step_budget: 16,
        ..KernelConfig::default()
    });
    let root = state.root_thread().unwrap();

    // 1. ThreadResume returns Gone if the thread's configured CSpace was destroyed.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Thread,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::CSpace { slots: 4 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::ThreadConfigure {
            thread: 10,
            cspace: 11,
            aspace: None,
            fault_ep: None,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::CapDelete {
            cspace: ROOT_CSPACE_SLOT,
            slot: 11,
        },
    );
    apply_err(
        &mut state,
        root,
        SysCall::ThreadResume { thread: 10 },
        Error::Invalid,
    );

    // 2. ThreadSuspend cancels BlockedOnSend so a later Recv does not see the message.
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
    );
    apply_ok(
        &mut state,
        root,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
    );
    let sender = spawn_thread(&mut state, root, 20, 21, 0, 8);
    apply_ok(
        &mut state,
        root,
        SysCall::CapDerive {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 12,
            dst_cspace: 20,
            dst_slot: 1,
            rights: Rights::SEND,
            badge: 0,
        },
    );
    assert_eq!(
        state
            .apply(
                sender,
                SysCall::Call {
                    endpoint: 1,
                    msg: Message::empty(42),
                    reply_recv_slots: SlotRange::empty(),
                },
            )
            .status,
        CallStatus::Blocked
    );
    apply_ok(&mut state, root, SysCall::ThreadSuspend { thread: 21 });
    let recv_out = state.apply(
        root,
        SysCall::Recv {
            endpoint: 12,
            reply: 13,
            recv_slots: SlotRange::empty(),
        },
    );
    assert_eq!(recv_out.status, CallStatus::Blocked);
    // 3. CapRevoke returns Gone if revoking a SubUntyped destroys the last cap
    // to a child CSpace that holds the SubUntyped capability itself.
    let mut s2 = KernelState::new(KernelConfig::default());
    let r2 = s2.root_thread().unwrap();
    apply_ok(
        &mut s2,
        r2,
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Untyped { bytes: 8192 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
    );
    apply_ok(
        &mut s2,
        r2,
        SysCall::UntypedRetype {
            untyped: 10,
            spec: RetypeSpec::CSpace { slots: 4 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
    );
    apply_ok(
        &mut s2,
        r2,
        SysCall::CapMove {
            src_cspace: ROOT_CSPACE_SLOT,
            src_slot: 10,
            dst_cspace: 11,
            dst_slot: 0,
        },
    );
    apply_err(
        &mut s2,
        r2,
        SysCall::CapRevoke {
            cspace: 11,
            slot: 0,
        },
        Error::Gone,
    );
    s2.check_invariants().unwrap();
}
