use dodos_abi::{Call, Error, MAX_CAPS, MAX_WORDS, ObjectKind, PageSize, Rights};
use dodos_model::{
    CallResult, CallStatus, CapTransfer, KernelConfig, KernelObject, KernelState, Message,
    ObjectId, ROOT_ASPACE_SLOT, ROOT_CSPACE_SLOT, ROOT_DEVICE_UNTYPED_SLOT, ROOT_THREAD_SLOT,
    ROOT_UNTYPED_SLOT, RetypeSpec, Slot, SlotIndex, SlotRange, SysCall, ThreadId, ThreadRegs,
};

/// Tiny deterministic PRNG (SplitMix64) with zero external dependencies.
#[derive(Clone, Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn gen_range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next_u64() % (upper as u64)) as usize
        }
    }

    fn chance(&mut self, num: usize, den: usize) -> bool {
        self.gen_range(den) < num
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        if slice.is_empty() {
            None
        } else {
            Some(&slice[self.gen_range(slice.len())])
        }
    }
}

struct CSpaceView {
    total_slots: usize,
    empty: Vec<SlotIndex>,
    revoked: Vec<SlotIndex>,
    live: Vec<(SlotIndex, ObjectKind)>,
    untyped: Vec<SlotIndex>,
    cspaces: Vec<SlotIndex>,
    endpoints: Vec<SlotIndex>,
    replies: Vec<SlotIndex>,
    notifies: Vec<SlotIndex>,
    threads: Vec<SlotIndex>,
    aspaces: Vec<SlotIndex>,
}

fn inspect_cspace(state: &KernelState, cspace_id: ObjectId) -> CSpaceView {
    let mut view = CSpaceView {
        total_slots: 0,
        empty: Vec::new(),
        revoked: Vec::new(),
        live: Vec::new(),
        untyped: Vec::new(),
        cspaces: Vec::new(),
        endpoints: Vec::new(),
        replies: Vec::new(),
        notifies: Vec::new(),
        threads: Vec::new(),
        aspaces: Vec::new(),
    };

    let Some(KernelObject::CSpace(cs)) = state.objects.get(&cspace_id) else {
        return view;
    };
    view.total_slots = cs.slots.len();
    for (idx, slot) in cs.slots.iter().enumerate() {
        match slot {
            Slot::Empty => view.empty.push(idx),
            Slot::Revoked => view.revoked.push(idx),
            Slot::Live(cap_id) => {
                if let Some(entry) = state.caps.get(cap_id)
                    && let Some(obj) = state.objects.get(&entry.object)
                {
                    let kind = obj.kind();
                    view.live.push((idx, kind));
                    match kind {
                        ObjectKind::Untyped => view.untyped.push(idx),
                        ObjectKind::CSpace => view.cspaces.push(idx),
                        ObjectKind::Endpoint => view.endpoints.push(idx),
                        ObjectKind::Reply => view.replies.push(idx),
                        ObjectKind::Notify => view.notifies.push(idx),
                        ObjectKind::Thread => view.threads.push(idx),
                        ObjectKind::ASpace => view.aspaces.push(idx),
                        _ => {}
                    }
                }
            }
        }
    }
    view
}

fn random_rights(rng: &mut SplitMix64) -> Rights {
    let all = [
        Rights::READ,
        Rights::WRITE,
        Rights::EXEC,
        Rights::GRANT,
        Rights::SEND,
        Rights::RECV,
        Rights::MANAGE,
    ];
    let mut r = Rights::NONE;
    for right in all {
        if rng.chance(1, 2) {
            r = r.union(right);
        }
    }
    if rng.chance(1, 4) {
        r = Rights::ALL;
    }
    r
}

fn random_message(rng: &mut SplitMix64, view: &CSpaceView) -> Message {
    let label = (rng.next_u64() & 0xffff) as u32;
    let word_len = match rng.gen_range(5) {
        0 => 0,
        1 => rng.gen_range(5),
        2 => MAX_WORDS,
        _ => rng.gen_range(MAX_WORDS + 1),
    };
    let words: Vec<u64> = (0..word_len).map(|_| rng.next_u64()).collect();

    let cap_count = if view.live.is_empty() || rng.chance(1, 2) {
        0
    } else {
        rng.gen_range(MAX_CAPS + 1)
    };
    let mut caps = Vec::with_capacity(cap_count);
    for _ in 0..cap_count {
        if let Some(&(slot, _)) = rng.pick(&view.live) {
            if rng.chance(1, 2) {
                caps.push(CapTransfer::Move { slot });
            } else {
                caps.push(CapTransfer::Derive {
                    slot,
                    rights: random_rights(rng),
                });
            }
        }
    }

    Message::new(label, words, caps, 0).expect("word and cap counts within ABI limits")
}

fn pick_slot_range(rng: &mut SplitMix64, view: &CSpaceView) -> SlotRange {
    if view.total_slots == 0 || rng.chance(1, 5) {
        return SlotRange::empty();
    }
    if rng.chance(1, 10) {
        // Deliberately pick an out-of-range or occupied range sometimes.
        let start = rng.gen_range(view.total_slots + 4);
        let count = rng.gen_range(MAX_CAPS + 2);
        return SlotRange::new(start, count);
    }
    if let Some(&start) = rng.pick(&view.empty) {
        let count = 1 + rng.gen_range(MAX_CAPS);
        SlotRange::new(start, count)
    } else {
        SlotRange::new(rng.gen_range(view.total_slots), rng.gen_range(MAX_CAPS + 1))
    }
}

fn pick_dst_slot(rng: &mut SplitMix64, view: &CSpaceView) -> SlotIndex {
    if rng.chance(4, 5)
        && let Some(&slot) = rng.pick(&view.empty)
    {
        slot
    } else {
        rng.gen_range(view.total_slots + 2)
    }
}

fn random_call(
    rng: &mut SplitMix64,
    state: &KernelState,
    caller: ThreadId,
    is_root: bool,
    runnable_count: usize,
) -> SysCall {
    let Some(KernelObject::Thread(t)) = state.objects.get(&caller.0) else {
        return SysCall::Yield;
    };
    let Some(cspace_id) = t.cspace else {
        return SysCall::Yield;
    };
    let view = inspect_cspace(state, cspace_id);
    let last_runnable = runnable_count <= 1;

    // Pick protected boot/self slots conservatively on destructive ops so
    // long sequences stay rich, while still occasionally exercising teardown.
    let pick_deletable_slot = |rng: &mut SplitMix64, view: &CSpaceView| -> SlotIndex {
        let min_safe = if is_root { 8 } else { 2 };
        if last_runnable || rng.chance(19, 20) {
            let candidates: Vec<SlotIndex> = view
                .live
                .iter()
                .map(|&(s, _)| s)
                .filter(|&s| s >= min_safe)
                .chain(view.revoked.iter().copied())
                .collect();
            if let Some(&s) = rng.pick(&candidates) {
                return s;
            }
            return view.total_slots + 1;
        }
        if !view.revoked.is_empty() && rng.chance(1, 3) {
            return *rng.pick(&view.revoked).unwrap();
        }
        if let Some(&(s, _)) = rng.pick(&view.live) {
            return s;
        }
        rng.gen_range(view.total_slots.max(1) + 2)
    };

    match rng.gen_range(20) {
        // UntypedRetype
        0..=2 => {
            let untyped = rng
                .pick(&view.untyped)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let dst_cspace = rng.pick(&view.cspaces).copied().unwrap_or(ROOT_CSPACE_SLOT);
            let dst_view = state
                .caps
                .get(
                    match state.objects.get(&cspace_id) {
                        Some(KernelObject::CSpace(cs)) => cs.slots.get(dst_cspace),
                        _ => None,
                    }
                    .and_then(|s| match s {
                        Slot::Live(id) => Some(id),
                        _ => None,
                    })
                    .unwrap_or(&dodos_model::CapId(0)),
                )
                .map(|e| inspect_cspace(state, e.object));
            let dst_slot = dst_view
                .as_ref()
                .map(|v| pick_dst_slot(rng, v))
                .unwrap_or_else(|| pick_dst_slot(rng, &view));

            let spec = match rng.gen_range(9) {
                0 => RetypeSpec::Endpoint,
                1 => RetypeSpec::Reply,
                2 => RetypeSpec::Notify,
                3 => RetypeSpec::Thread,
                4 => RetypeSpec::CSpace {
                    slots: 4 + rng.gen_range(16),
                },
                5 => RetypeSpec::Untyped {
                    bytes: 4096 * (1 + rng.gen_range(4)) as u64,
                },
                6 => RetypeSpec::ASpace,
                7 => RetypeSpec::Frame {
                    page_size: if rng.chance(3, 4) {
                        PageSize::Size4K
                    } else {
                        PageSize::Size2M
                    },
                    count: 1 + rng.gen_range(3),
                },
                _ => RetypeSpec::Unsupported(ObjectKind::Timer),
            };
            SysCall::UntypedRetype {
                untyped,
                spec,
                dst_cspace,
                dst_slot,
            }
        }
        // CapDerive
        3..=4 => {
            let src_cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let dst_cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let src_slot = rng
                .pick(&view.live)
                .map(|&(s, _)| s)
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let dst_slot = pick_dst_slot(rng, &view);
            let badge = if rng.chance(1, 2) {
                0
            } else {
                1u64 << rng.gen_range(8)
            };
            SysCall::CapDerive {
                src_cspace,
                src_slot,
                dst_cspace,
                dst_slot,
                rights: random_rights(rng),
                badge,
            }
        }
        // CapMove
        5 => {
            let src_cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let dst_cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let src_slot = pick_deletable_slot(rng, &view);
            let dst_slot = pick_dst_slot(rng, &view);
            SysCall::CapMove {
                src_cspace,
                src_slot,
                dst_cspace,
                dst_slot,
            }
        }
        // CapDelete
        6 => {
            let cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let slot = pick_deletable_slot(rng, &view);
            SysCall::CapDelete { cspace, slot }
        }
        // CapRevoke
        7 => {
            let cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let slot = pick_deletable_slot(rng, &view);
            SysCall::CapRevoke { cspace, slot }
        }
        // CapInspect
        8 => {
            let cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
            let slot = rng.gen_range(view.total_slots + 2);
            SysCall::CapInspect { cspace, slot }
        }
        // ThreadConfigure / ReadRegs / WriteRegs / Resume / Suspend
        9..=10 => {
            let thread = rng.pick(&view.threads).copied().unwrap_or(ROOT_THREAD_SLOT);
            match rng.gen_range(5) {
                0 => {
                    let cspace = rng.pick(&view.cspaces).copied().unwrap_or(0);
                    let aspace = if rng.chance(1, 2) {
                        rng.pick(&view.aspaces).copied()
                    } else {
                        None
                    };
                    let fault_ep = if rng.chance(1, 3) {
                        rng.pick(&view.endpoints).copied()
                    } else {
                        None
                    };
                    SysCall::ThreadConfigure {
                        thread,
                        cspace,
                        aspace,
                        fault_ep,
                    }
                }
                1 => SysCall::ThreadReadRegs { thread },
                2 => SysCall::ThreadWriteRegs {
                    thread,
                    regs: ThreadRegs {
                        pc: rng.next_u64(),
                        sp: rng.next_u64(),
                        args: [rng.next_u64(); dodos_abi::REG_WORDS],
                    },
                },
                3 => SysCall::ThreadResume { thread },
                _ => {
                    if last_runnable || (is_root && thread == ROOT_THREAD_SLOT) {
                        SysCall::ThreadResume { thread }
                    } else {
                        SysCall::ThreadSuspend { thread }
                    }
                }
            }
        }
        // Call (avoid blocking if this is the last runnable thread)
        11..=12 => {
            if last_runnable {
                return SysCall::Yield;
            }
            let endpoint = rng
                .pick(&view.endpoints)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let msg = random_message(rng, &view);
            let reply_recv_slots = pick_slot_range(rng, &view);
            SysCall::Call {
                endpoint,
                msg,
                reply_recv_slots,
            }
        }
        // Recv
        13..=14 => {
            if last_runnable {
                return SysCall::Yield;
            }
            let endpoint = rng
                .pick(&view.endpoints)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let reply = rng
                .pick(&view.replies)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let recv_slots = pick_slot_range(rng, &view);
            SysCall::Recv {
                endpoint,
                reply,
                recv_slots,
            }
        }
        // ReplyRecv
        15 => {
            if last_runnable {
                return SysCall::Yield;
            }
            let reply = rng
                .pick(&view.replies)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let endpoint = rng
                .pick(&view.endpoints)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            let reply_msg = random_message(rng, &view);
            let recv_slots = pick_slot_range(rng, &view);
            SysCall::ReplyRecv {
                reply,
                reply_msg,
                endpoint,
                recv_slots,
            }
        }
        // Send (to Endpoint or Reply)
        16 => {
            let target = if rng.chance(1, 2) && !view.replies.is_empty() {
                *rng.pick(&view.replies).unwrap()
            } else if !view.endpoints.is_empty() {
                *rng.pick(&view.endpoints).unwrap()
            } else {
                rng.gen_range(view.total_slots + 2)
            };
            let msg = random_message(rng, &view);
            SysCall::Send { target, msg }
        }
        // Signal / Wait / Poll
        17..=18 => {
            let ntfn = rng
                .pick(&view.notifies)
                .copied()
                .unwrap_or_else(|| rng.gen_range(view.total_slots + 2));
            match rng.gen_range(3) {
                0 => SysCall::Signal { notify: ntfn },
                1 => SysCall::Poll { target: ntfn },
                _ => {
                    if last_runnable {
                        SysCall::Signal { notify: ntfn }
                    } else {
                        SysCall::Wait { target: ntfn }
                    }
                }
            }
        }
        // Yield or Unsupported
        _ => {
            if rng.chance(1, 2) {
                SysCall::Yield
            } else {
                SysCall::Unsupported(Call::TimerSet)
            }
        }
    }
}

#[derive(Default)]
struct RunStats {
    steps: usize,
    ipc_woke: usize,
    cap_transfers: usize,
    revoke_again: usize,
    gone_errors: usize,
    blocked_calls: usize,
}

fn setup_worker_pool(state: &mut KernelState, root: ThreadId) -> Vec<(ThreadId, SlotIndex)> {
    // Give root some initial shared objects in slots 8..16 and 3 worker threads in slots 20..26.
    // Endpoint 0: slot 8, Endpoint 1: slot 9, Reply 0: slot 10, Reply 1: slot 11, Notify: slot 12.
    let setup_calls = [
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 8,
        },
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Endpoint,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 9,
        },
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 10,
        },
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Reply,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 11,
        },
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Notify,
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 12,
        },
        SysCall::UntypedRetype {
            untyped: ROOT_UNTYPED_SLOT,
            spec: RetypeSpec::Untyped { bytes: 65536 },
            dst_cspace: ROOT_CSPACE_SLOT,
            dst_slot: 13,
        },
    ];
    for call in setup_calls {
        let out = state.apply(root, call);
        assert_eq!(out.status, CallStatus::Completed(CallResult::ok()));
        state.check_invariants().unwrap();
    }

    let mut workers = Vec::new();
    for i in 0..3 {
        let cs_slot = 20 + i * 2;
        let th_slot = 21 + i * 2;
        assert_eq!(
            state
                .apply(
                    root,
                    SysCall::UntypedRetype {
                        untyped: ROOT_UNTYPED_SLOT,
                        spec: RetypeSpec::CSpace { slots: 24 },
                        dst_cspace: ROOT_CSPACE_SLOT,
                        dst_slot: cs_slot,
                    },
                )
                .status,
            CallStatus::Completed(CallResult::ok())
        );
        assert_eq!(
            state
                .apply(
                    root,
                    SysCall::UntypedRetype {
                        untyped: ROOT_UNTYPED_SLOT,
                        spec: RetypeSpec::Thread,
                        dst_cspace: ROOT_CSPACE_SLOT,
                        dst_slot: th_slot,
                    },
                )
                .status,
            CallStatus::Completed(CallResult::ok())
        );
        // Populate worker CSpace: 0 = own CSpace, 1 = own Thread, 2 = EP8 (badged), 3 = EP9, 4 = Reply10/11, 5 = Notify12, 6 = SubUntyped13.
        let derives = [
            (cs_slot, 0, Rights::ALL, 0),
            (th_slot, 1, Rights::ALL, 0),
            (
                8,
                2,
                Rights::SEND.union(Rights::RECV).union(Rights::GRANT),
                (i as u64) + 1,
            ),
            (
                9,
                3,
                Rights::SEND.union(Rights::RECV).union(Rights::GRANT),
                (i as u64) + 10,
            ),
            (10 + (i % 2), 4, Rights::ALL, 0),
            (12, 5, Rights::SEND.union(Rights::RECV), 1u64 << i),
            (13, 6, Rights::ALL, 0),
        ];
        for (src_slot, dst_slot, rights, badge) in derives {
            assert_eq!(
                state
                    .apply(
                        root,
                        SysCall::CapDerive {
                            src_cspace: ROOT_CSPACE_SLOT,
                            src_slot,
                            dst_cspace: cs_slot,
                            dst_slot,
                            rights,
                            badge,
                        },
                    )
                    .status,
                CallStatus::Completed(CallResult::ok())
            );
        }
        assert_eq!(
            state
                .apply(
                    root,
                    SysCall::ThreadConfigure {
                        thread: th_slot,
                        cspace: cs_slot,
                        aspace: Some(ROOT_ASPACE_SLOT),
                        fault_ep: None,
                    },
                )
                .status,
            CallStatus::Completed(CallResult::ok())
        );
        assert_eq!(
            state
                .apply(root, SysCall::ThreadResume { thread: th_slot })
                .status,
            CallStatus::Completed(CallResult::ok())
        );
        state.check_invariants().unwrap();
        let all_runnable = state.runnable_threads();
        let worker_id = *all_runnable
            .iter()
            .find(|&&tid| tid != root && !workers.iter().any(|&(w, _)| w == tid))
            .expect("worker thread must be runnable");
        workers.push((worker_id, cs_slot));
    }
    let _ = ROOT_DEVICE_UNTYPED_SLOT;
    workers
}

fn run_seed(seed: u64, max_steps: usize) -> (KernelState, RunStats) {
    let mut rng = SplitMix64::new(seed);
    let mut state = KernelState::new(KernelConfig {
        init_cspace_slots: 64,
        init_untyped_bytes: 512 * 1024,
        init_device_untyped_bytes: Some(16 * 1024),
        revoke_step_budget: 3,
    });
    state.check_invariants().unwrap();

    let root = state.root_thread().unwrap();
    let _workers = setup_worker_pool(&mut state, root);
    let mut stats = RunStats::default();

    for step in 0..max_steps {
        let runnable = state.runnable_threads();
        if runnable.is_empty() {
            break;
        }
        let caller = runnable[rng.gen_range(runnable.len())];
        let call = random_call(&mut rng, &state, caller, caller == root, runnable.len());
        let outcome = state.apply(caller, call.clone());
        stats.steps += 1;

        match &outcome.status {
            CallStatus::Blocked => stats.blocked_calls += 1,
            CallStatus::Completed(res) => {
                if res.error == Error::Again {
                    stats.revoke_again += 1;
                } else if res.error == Error::Gone {
                    stats.gone_errors += 1;
                }
                if let dodos_model::CallValue::Received {
                    caps_transferred, ..
                }
                | dodos_model::CallValue::Reply {
                    caps_transferred, ..
                } = res.value
                {
                    stats.cap_transfers += caps_transferred;
                }
            }
        }
        for w in &outcome.woken {
            stats.ipc_woke += 1;
            if w.result.error == Error::Gone {
                stats.gone_errors += 1;
            }
            if let dodos_model::CallValue::Received {
                caps_transferred, ..
            }
            | dodos_model::CallValue::Reply {
                caps_transferred, ..
            } = w.result.value
            {
                stats.cap_transfers += caps_transferred;
            }
        }

        if let Err(err) = state.check_invariants() {
            panic!(
                "invariant violation on seed {seed} step {step} caller {caller:?} call {call:?}: {err:?}"
            );
        }
    }

    (state, stats)
}

#[test]
fn random_multi_thread_call_sequences_preserve_all_invariants() {
    let seeds = [
        0x1111_2222_3333_4444,
        0xdead_beef_cafe_babe,
        0x0123_4567_89ab_cdef,
        0xfeed_face_1234_5678,
        0x9988_7766_5544_3322,
        0x0f0e_0d0c_0b0a_0908,
    ];

    let mut total_steps = 0;
    let mut total_woke = 0;
    let mut total_caps = 0;
    let mut total_again = 0;
    let mut total_gone = 0;
    let mut total_blocked = 0;

    for &seed in &seeds {
        let (s1, stats) = run_seed(seed, 500);
        // Determinism check: replaying the exact same seed yields bit-identical state.
        let (s2, _) = run_seed(seed, 500);
        assert_eq!(s1, s2, "execution must be deterministic for seed {seed}");

        total_steps += stats.steps;
        total_woke += stats.ipc_woke;
        total_caps += stats.cap_transfers;
        total_again += stats.revoke_again;
        total_gone += stats.gone_errors;
        total_blocked += stats.blocked_calls;
    }

    assert!(total_steps >= 2000, "too few steps executed: {total_steps}");
    assert!(total_blocked > 0, "generator never exercised blocking");
    assert!(
        total_woke > 0,
        "generator never exercised IPC/notify wakeups"
    );
    assert!(total_gone > 0, "generator never exercised Gone errors");
    assert!(total_again > 0, "generator never exercised bounded Again");
    assert!(total_caps > 0, "generator never exercised cap transfers");
}
