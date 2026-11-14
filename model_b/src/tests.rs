//! Unit tests for every modelled call and every error path the contract
//! names, plus one property test: long random call sequences from several
//! threads with a hand-written deterministic PRNG, the invariant checker run
//! after every step, and fixed seeds so a failure reproduces.

use crate::*;
use dodos_abi::{Call, Error, Header, Marks, ObjectKind, PageSize, Rights};

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn free_slot(st: &State, t: ThreadId) -> u32 {
    let cs = st.thread_cspace(t).expect("live cspace");
    match st.objects.get(&cs) {
        Some(Object::CSpace { slots }) => slots
            .iter()
            .position(|s| matches!(s, SlotState::Empty))
            .expect("a free slot") as u32,
        _ => panic!("no cspace"),
    }
}

fn first_free(st: &State, t: ThreadId) -> Option<u32> {
    let cs = st.thread_cspace(t)?;
    match st.objects.get(&cs)? {
        Object::CSpace { slots } => slots
            .iter()
            .position(|s| matches!(s, SlotState::Empty))
            .map(|i| i as u32),
        _ => None,
    }
}

fn cap_of(st: &State, t: ThreadId, slot: u32) -> Cap {
    // resolve is pub(crate); tests live in the crate.
    let cs = st.thread_cspace(t).unwrap();
    let Object::CSpace { slots } = st.objects.get(&cs).unwrap() else {
        panic!()
    };
    match slots[slot as usize] {
        SlotState::Full(id) => st.caps[&id].clone(),
        _ => panic!("slot {slot} not full"),
    }
}

fn untyped_size(st: &State, t: ThreadId, slot: u32) -> u64 {
    let cap = cap_of(st, t, slot);
    match st.objects[&cap.obj] {
        Object::Untyped { size, .. } => size,
        _ => panic!("not untyped"),
    }
}

#[allow(clippy::too_many_arguments)] // test helper mirroring the call's registers
fn retype(
    st: &mut State,
    t: ThreadId,
    src: u32,
    dst: u32,
    kind: ObjectKind,
    size: u64,
    page: PageSize,
    aux: Option<u32>,
) -> Outcome {
    let mut a = Args::new();
    a.slot = src;
    a.dst_slot = dst;
    a.kind = kind;
    a.size = size;
    a.page_size = page;
    a.aux_slot = aux;
    st.apply(t, Call::UntypedRetype, &a)
}

fn derive(st: &mut State, t: ThreadId, src: u32, dst: u32, rights: Rights, badge: u64) -> Outcome {
    let mut a = Args::new();
    a.slot = src;
    a.dst_slot = dst;
    a.rights = rights;
    a.badge = badge;
    st.apply(t, Call::CapDerive, &a)
}

fn simple(st: &mut State, t: ThreadId, call: Call, slot: u32) -> Outcome {
    let mut a = Args::new();
    a.slot = slot;
    st.apply(t, call, &a)
}

fn msg_args(words: &[u64], caps: usize, transfers: Vec<Transfer>) -> Args {
    let mut a = Args::new();
    a.header = Header::new(0x1234, words.len(), caps, 0).expect("valid header");
    a.words = words.to_vec();
    a.transfers = transfers;
    a
}

fn ipc_setup() -> (State, ThreadId, ThreadId) {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let t1 = out.values[0] as ThreadId;
    (st, t0, t1)
}

// ---------------------------------------------------------------------
// Boot state
// ---------------------------------------------------------------------

#[test]
fn boot_state_is_well_formed() {
    let st = State::new();
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
    assert_eq!(st.runnable(), vec![BOOT_THREAD]);
    assert_eq!(st.thread_cspace(BOOT_THREAD), Some(0));
    // Boot slots hold full-rights capabilities to the four boot objects.
    for (slot, kind) in [
        (BOOT_SLOT_CSPACE, ObjectKind::CSpace),
        (BOOT_SLOT_UNTYPED, ObjectKind::Untyped),
        (BOOT_SLOT_DEVICE, ObjectKind::Untyped),
        (BOOT_SLOT_THREAD, ObjectKind::Thread),
    ] {
        let cap = cap_of(&st, BOOT_THREAD, slot);
        assert_eq!(cap.rights, Rights::ALL);
        assert_eq!(st.objects[&cap.obj].kind(), kind);
        assert_eq!(cap.parent, None);
    }
    // State is a comparable value.
    assert_eq!(st, st.clone());
}

// ---------------------------------------------------------------------
// untyped_retype
// ---------------------------------------------------------------------

#[test]
fn retype_creates_each_kind_and_consumes_memory() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    let before = untyped_size(&st, t, BOOT_SLOT_UNTYPED);

    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let ep = out.values[0];
    assert!(matches!(st.objects[&ep], Object::Endpoint(_)));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - SMALL_BYTES
    );
    // New capability: full rights, child of the untyped capability.
    let cap = cap_of(&st, t, 10);
    assert_eq!(cap.obj, ep);
    assert_eq!(cap.rights, Rights::ALL);
    assert_eq!(cap.parent, Some(cap_of(&st, t, BOOT_SLOT_UNTYPED).id));

    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert!(matches!(st.objects[&out.values[0]], Object::Notify(_)));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - 2 * SMALL_BYTES
    );

    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        12,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let new_thread = out.values[0] as ThreadId;
    assert!(matches!(st.objects[&out.values[0]], Object::Thread));
    assert!(st.runnable().contains(&new_thread));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - 2 * SMALL_BYTES - THREAD_BYTES
    );

    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        13,
        ObjectKind::Frame,
        0,
        PageSize::Size2M,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert!(matches!(
        st.objects[&out.values[0]],
        Object::Frame {
            page_size: PageSize::Size2M
        }
    ));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - 2 * SMALL_BYTES - THREAD_BYTES - PageSize::Size2M.bytes()
    );

    // A cspace of 8 slots costs 8 * 32 bytes.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        14,
        ObjectKind::CSpace,
        8,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let cs = out.values[0];
    assert_eq!(st.cspace_len(cs), Some(8));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - 2 * SMALL_BYTES - THREAD_BYTES - PageSize::Size2M.bytes() - 8 * SLOT_BYTES
    );

    // Splitting untyped: the new range is carved from the front.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        15,
        ObjectKind::Untyped,
        4096,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    match st.objects[&out.values[0]] {
        Object::Untyped {
            base, size, device, ..
        } => {
            assert_eq!(size, 4096);
            assert!(!device);
            let _ = base;
        }
        _ => panic!(),
    }
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn retype_error_paths() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    // no_memory: more than what remains.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Untyped,
        u64::MAX,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::NoMemory);
    // invalid: destination occupied.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        BOOT_SLOT_CSPACE,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // invalid: destination out of range.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        9999,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // invalid: zero size for an untyped split.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Untyped,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // invalid: source slot is not an untyped (wrong object type).
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_CSPACE,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // invalid: empty source slot.
    let out = retype(
        &mut st,
        t,
        60,
        13,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // invalid: retype destination must be in the caller's own cspace.
    let mut a = Args::new();
    a.slot = BOOT_SLOT_UNTYPED;
    a.dst_slot = 10;
    a.kind = ObjectKind::Endpoint;
    a.dst_cspace_slot = Some(BOOT_SLOT_CSPACE);
    assert_eq!(st.apply(t, Call::UntypedRetype, &a).error, Error::Invalid);
    // denied: untyped capability without `write`.
    let out = derive(&mut st, t, BOOT_SLOT_UNTYPED, 20, Rights::READ, 0);
    assert_eq!(out.error, Error::Ok);
    let out = retype(
        &mut st,
        t,
        20,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Denied);
    // unsupported: kinds out of scope for the one-core model.
    for kind in [
        ObjectKind::ASpace,
        ObjectKind::Sched,
        ObjectKind::Timer,
        ObjectKind::Task,
        ObjectKind::Core,
        ObjectKind::Debug,
        ObjectKind::Reply,
    ] {
        let out = retype(
            &mut st,
            t,
            BOOT_SLOT_UNTYPED,
            10,
            kind,
            0,
            PageSize::Size4K,
            None,
        );
        assert_eq!(out.error, Error::Unsupported, "kind {kind:?}");
    }
    // nothing was consumed by failed calls
    assert_eq!(untyped_size(&st, t, BOOT_SLOT_UNTYPED), BOOT_MEMORY_BYTES);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn device_memory_can_only_become_frames() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    // Endpoint from device memory: invalid.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_DEVICE,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Invalid);
    // Frame from device memory: ok.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_DEVICE,
        10,
        ObjectKind::Frame,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert!(matches!(st.objects[&out.values[0]], Object::Frame { .. }));
    // Splitting device untyped stays device memory.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_DEVICE,
        11,
        ObjectKind::Untyped,
        4096,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    match st.objects[&out.values[0]] {
        Object::Untyped { device, size, .. } => {
            assert!(device);
            assert_eq!(size, 4096);
        }
        _ => panic!(),
    }
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cspace_grows_by_supplying_memory() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        8,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let cs = out.values[0];
    assert_eq!(st.cspace_len(cs), Some(8));
    // Growing needs `manage` on the cspace capability: derive without it.
    let out = derive(&mut st, t, 10, 11, Rights::READ.union(Rights::WRITE), 0);
    assert_eq!(out.error, Error::Ok);
    let before = untyped_size(&st, t, BOOT_SLOT_UNTYPED);
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::CSpace,
        4,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Denied);
    assert_eq!(untyped_size(&st, t, BOOT_SLOT_UNTYPED), before);
    // Growing through the full-rights capability works.
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        4,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert_eq!(st.cspace_len(cs), Some(12));
    assert_eq!(
        untyped_size(&st, t, BOOT_SLOT_UNTYPED),
        before - 4 * SLOT_BYTES
    );
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn retype_thread_binds_to_a_cspace() {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    // A second cspace.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        16,
        PageSize::Size4K,
        None,
    );
    let cs_b = out.values[0];
    // A thread bound to it through the auxiliary slot.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        Some(10),
    );
    assert_eq!(out.error, Error::Ok);
    let t1 = out.values[0] as ThreadId;
    assert_eq!(st.thread_cspace(t1), Some(cs_b));
    // Auxiliary slot that is not a cspace: invalid.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        12,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        Some(BOOT_SLOT_UNTYPED),
    );
    assert_eq!(out.error, Error::Invalid);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// cap_derive / cap_move / cap_delete / cap_revoke / cap_inspect
// ---------------------------------------------------------------------

#[test]
fn cap_derive_attenuates_and_badges() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    let out = derive(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        Rights::READ.union(Rights::DERIVE),
        0xAB,
    );
    assert_eq!(out.error, Error::Ok);
    let id = out.values[0];
    let cap = cap_of(&st, t, 10);
    assert_eq!(cap.id, id);
    assert_eq!(cap.rights, Rights::READ.union(Rights::DERIVE));
    assert_eq!(cap.badge, 0xAB);
    assert_eq!(cap.parent, Some(cap_of(&st, t, BOOT_SLOT_UNTYPED).id));
    // inspect reads kind, rights, badge, object
    let out = simple(&mut st, t, Call::CapInspect, 10);
    assert_eq!(out.error, Error::Ok);
    assert_eq!(
        out.values,
        vec![
            ObjectKind::Untyped as u64,
            (Rights::READ.union(Rights::DERIVE)).bits() as u64,
            0xAB,
            cap.obj
        ]
    );
    // Deriving from a capability without `derive`: denied, even fewer rights.
    let out = derive(&mut st, t, BOOT_SLOT_UNTYPED, 12, Rights::READ, 0);
    assert_eq!(out.error, Error::Ok);
    let out = derive(&mut st, t, 12, 13, Rights::READ, 0);
    assert_eq!(out.error, Error::Denied);
    // Adding a right the source does not have: denied.
    let out = derive(&mut st, t, 12, 11, Rights::READ.union(Rights::EXEC), 0);
    assert_eq!(out.error, Error::Denied);
    // Destination occupied or out of range: invalid.
    let out = derive(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        BOOT_SLOT_CSPACE,
        Rights::READ,
        0,
    );
    assert_eq!(out.error, Error::Invalid);
    let out = derive(&mut st, t, BOOT_SLOT_UNTYPED, 9999, Rights::READ, 0);
    assert_eq!(out.error, Error::Invalid);
    // Empty source slot: invalid.
    let out = derive(&mut st, t, 60, 13, Rights::READ, 0);
    assert_eq!(out.error, Error::Invalid);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_derive_into_another_cspace() {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        8,
        PageSize::Size4K,
        None,
    );
    let cs_b = out.values[0];
    let mut a = Args::new();
    a.slot = BOOT_SLOT_UNTYPED;
    a.dst_slot = 3;
    a.rights = Rights::READ;
    a.dst_cspace_slot = Some(10);
    let out = st.apply(t0, Call::CapDerive, &a);
    assert_eq!(out.error, Error::Ok);
    let cap_b = match st.objects.get(&cs_b) {
        Some(Object::CSpace { slots }) => match slots[3] {
            SlotState::Full(id) => st.caps[&id].clone(),
            _ => panic!("slot 3 of cspace B not full"),
        },
        _ => panic!(),
    };
    assert_eq!(cap_b.rights, Rights::READ);
    assert_eq!(cap_b.parent, Some(cap_of(&st, t0, BOOT_SLOT_UNTYPED).id));
    // dst_cspace_slot that is not a cspace: invalid.
    let mut a = Args::new();
    a.slot = BOOT_SLOT_UNTYPED;
    a.dst_slot = 3;
    a.rights = Rights::READ;
    a.dst_cspace_slot = Some(BOOT_SLOT_UNTYPED);
    assert_eq!(st.apply(t0, Call::CapDerive, &a).error, Error::Invalid);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_move_moves_the_node() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    let out = derive(&mut st, t, BOOT_SLOT_UNTYPED, 10, Rights::READ, 0);
    let id = out.values[0];
    let mut a = Args::new();
    a.slot = 10;
    a.dst_slot = 11;
    let out = st.apply(t, Call::CapMove, &a);
    assert_eq!(out.error, Error::Ok);
    // Same node, new slot; the tree is untouched.
    assert_eq!(cap_of(&st, t, 11).id, id);
    assert_eq!(cap_of(&st, t, 11).loc, (st.thread_cspace(t).unwrap(), 11));
    let cs = st.thread_cspace(t).unwrap();
    match st.objects.get(&cs) {
        Some(Object::CSpace { slots }) => assert!(matches!(slots[10], SlotState::Empty)),
        _ => panic!(),
    }
    // Moving onto itself is a no-op; onto an occupied slot: invalid.
    let mut a = Args::new();
    a.slot = 11;
    a.dst_slot = 11;
    assert_eq!(st.apply(t, Call::CapMove, &a).error, Error::Ok);
    let mut a = Args::new();
    a.slot = 11;
    a.dst_slot = BOOT_SLOT_CSPACE;
    assert_eq!(st.apply(t, Call::CapMove, &a).error, Error::Invalid);
    // Empty source: invalid.
    let mut a = Args::new();
    a.slot = 60;
    a.dst_slot = 12;
    assert_eq!(st.apply(t, Call::CapMove, &a).error, Error::Invalid);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_delete_destroys_object_when_last_cap_goes() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    let out = retype(
        &mut st,
        t,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    let ep = out.values[0];
    // A second capability to the same object keeps it alive.
    let out = derive(&mut st, t, 10, 11, Rights::SEND, 0);
    assert_eq!(out.error, Error::Ok);
    assert_eq!(simple(&mut st, t, Call::CapDelete, 10).error, Error::Ok);
    assert!(st.objects.contains_key(&ep));
    assert_eq!(simple(&mut st, t, Call::CapInspect, 11).error, Error::Ok);
    // Deleting the last capability destroys the object; the slot is empty,
    // so a later call on it is `invalid`.
    assert_eq!(simple(&mut st, t, Call::CapDelete, 11).error, Error::Ok);
    assert!(!st.objects.contains_key(&ep));
    assert_eq!(
        simple(&mut st, t, Call::CapInspect, 11).error,
        Error::Invalid
    );
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_delete_reparents_children_in_the_tree() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    // chain: untyped(1) -> a(10) -> b(11)
    let a = derive(&mut st, t, BOOT_SLOT_UNTYPED, 10, Rights::ALL, 0).values[0];
    let b = derive(&mut st, t, 10, 11, Rights::READ, 0).values[0];
    let root = cap_of(&st, t, BOOT_SLOT_UNTYPED).id;
    assert_eq!(st.caps[&b].parent, Some(a));
    // delete the middle node: b is re-parented to the root
    assert_eq!(simple(&mut st, t, Call::CapDelete, 10).error, Error::Ok);
    assert_eq!(st.caps[&b].parent, Some(root));
    // revoking the root now kills b as well
    let out = simple(&mut st, t, Call::CapRevoke, BOOT_SLOT_UNTYPED);
    assert_eq!(out.error, Error::Ok);
    assert_eq!(simple(&mut st, t, Call::CapInspect, 11).error, Error::Gone);
    assert_eq!(
        simple(&mut st, t, Call::CapInspect, 10).error,
        Error::Invalid,
        "slot 10 was deleted, not revoked"
    );
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_revoke_steps_and_kills_the_whole_subtree() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    // 10 children of the untyped capability: more than REVOKE_STEP.
    for i in 0..10u32 {
        let out = derive(&mut st, t, BOOT_SLOT_UNTYPED, 20 + i, Rights::READ, 0);
        assert_eq!(out.error, Error::Ok);
    }
    // First call deletes REVOKE_STEP nodes and returns `again`.
    let out = simple(&mut st, t, Call::CapRevoke, BOOT_SLOT_UNTYPED);
    assert_eq!(out.error, Error::Again);
    // Calls on any revoking capability are already `gone` (rule 2).
    assert_eq!(simple(&mut st, t, Call::CapInspect, 20).error, Error::Gone);
    assert_eq!(simple(&mut st, t, Call::CapInspect, 29).error, Error::Gone);
    // Repeat until done.
    let out = simple(&mut st, t, Call::CapRevoke, BOOT_SLOT_UNTYPED);
    assert_eq!(out.error, Error::Ok);
    // All slots are dead tombstones now.
    for i in 0..10u32 {
        assert_eq!(
            simple(&mut st, t, Call::CapInspect, 20 + i).error,
            Error::Gone,
            "slot {}",
            20 + i
        );
    }
    assert_eq!(
        simple(&mut st, t, Call::CapInspect, BOOT_SLOT_UNTYPED).error,
        Error::Gone
    );
    // Revoking again: gone. Revoking an empty slot: invalid.
    assert_eq!(
        simple(&mut st, t, Call::CapRevoke, BOOT_SLOT_UNTYPED).error,
        Error::Gone
    );
    assert_eq!(
        simple(&mut st, t, Call::CapRevoke, 60).error,
        Error::Invalid
    );
    // The untyped object had no capability left and was destroyed.
    assert!(
        !st.objects
            .values()
            .any(|o| matches!(o, Object::Untyped { device: false, .. }))
    );
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn cap_revoke_destroys_objects_and_wakes_blocked_threads() {
    let (mut st, t0, t1) = ipc_setup();
    // t1 blocks in recv on the endpoint.
    let mut a = Args::new();
    a.slot = 10;
    let out = st.apply(t1, Call::Recv, &a);
    assert_eq!(out.error, Error::Ok);
    assert!(out.blocked);
    // Revoke the endpoint capability: the endpoint is destroyed and t1 is
    // woken with `gone`.
    let out = simple(&mut st, t0, Call::CapRevoke, 10);
    assert_eq!(out.error, Error::Ok);
    assert!(st.runnable().contains(&t1));
    // The completion is returned by t1's next apply.
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    assert_eq!(out.error, Error::Gone);
    assert_eq!(out.inbox, Some(Inbox::Error(Error::Gone)));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// IPC: send / recv / reply_recv / call / reply
// ---------------------------------------------------------------------

#[test]
fn send_delivers_to_a_waiting_receiver_with_badge() {
    let (mut st, t0, t1) = ipc_setup();
    // t1 blocks in recv on the endpoint.
    let mut a = Args::new();
    a.slot = 10;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    // t0 derives a badged endpoint capability and sends through it.
    let out = derive(&mut st, t0, 10, 13, Rights::SEND.union(Rights::RECV), 0xAB);
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(&[11, 22], 0, vec![]);
    a.slot = 13;
    let out = st.apply(t0, Call::Send, &a);
    assert_eq!(out.error, Error::Ok);
    assert!(!out.blocked, "send never blocks");
    // t1's next call returns the delivered message.
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    let Some(Inbox::Message {
        received,
        words,
        reply_slot,
    }) = out.inbox
    else {
        panic!("expected a message, got {:?}", out.inbox)
    };
    assert_eq!(received.header.words(), 2);
    assert_eq!(received.badge, 0xAB);
    assert_eq!(received.marks, Marks::NONE);
    assert_eq!(received.task, st.threads[&t0].task);
    assert_eq!(words, vec![11, 22]);
    assert_eq!(reply_slot, None);
    assert!(st.runnable().contains(&t1));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn send_queues_and_recv_takes_in_fifo_order() {
    let (mut st, t0, t1) = ipc_setup();
    for w in [1u64, 2, 3] {
        let mut a = msg_args(&[w], 0, vec![]);
        a.slot = 10;
        assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    }
    // No receiver was waiting: messages are queued on the endpoint.
    for w in [1u64, 2, 3] {
        let mut a = Args::new();
        a.slot = 10;
        let out = st.apply(t1, Call::Recv, &a);
        assert_eq!(out.error, Error::Ok);
        assert!(!out.blocked);
        match out.inbox {
            Some(Inbox::Message { words, .. }) => assert_eq!(words, vec![w]),
            other => panic!("expected message {w}, got {other:?}"),
        }
    }
    // Queue empty now: recv blocks.
    let mut a = Args::new();
    a.slot = 10;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn ipc_rights_and_type_errors() {
    let (mut st, t0, t1) = ipc_setup();
    // send needs SEND on the endpoint capability.
    let out = derive(&mut st, t0, 10, 14, Rights::RECV, 0);
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 14;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Denied);
    // recv needs RECV.
    let out = derive(&mut st, t0, 10, 15, Rights::SEND, 0);
    assert_eq!(out.error, Error::Ok);
    let mut a = Args::new();
    a.slot = 15;
    assert_eq!(st.apply(t0, Call::Recv, &a).error, Error::Denied);
    assert_eq!(st.apply(t0, Call::ReplyRecv, &a).error, Error::Denied);
    // call needs SEND.
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 14;
    assert_eq!(st.apply(t0, Call::Call, &a).error, Error::Denied);
    // Wrong object types.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        16,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 16;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Invalid);
    assert_eq!(st.apply(t0, Call::Call, &a).error, Error::Invalid);
    let mut a = Args::new();
    a.slot = BOOT_SLOT_UNTYPED;
    assert_eq!(st.apply(t0, Call::Recv, &a).error, Error::Invalid);
    assert_eq!(st.apply(t0, Call::Signal, &a).error, Error::Invalid);
    assert_eq!(st.apply(t0, Call::Poll, &a).error, Error::Invalid);
    let mut a = Args::new();
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Signal, &a).error, Error::Invalid);
    assert_eq!(st.apply(t0, Call::Poll, &a).error, Error::Invalid);
    // Bad message shape: header word/cap counts must match the payload.
    let mut a = Args::new();
    a.slot = 10;
    a.header = Header::new(0, 3, 0, 0).unwrap();
    a.words = vec![1, 2];
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Invalid);
    let mut a = Args::new();
    a.slot = 10;
    a.header = Header::new(0, 1, 1, 0).unwrap();
    a.words = vec![1];
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Invalid);
    // Out-of-range receive range: invalid.
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 9999;
    a.recv_count = 1;
    assert_eq!(st.apply(t1, Call::Recv, &a).error, Error::Invalid);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn call_and_reply_round_trip() {
    let (mut st, t0, t1) = ipc_setup();
    // t0 calls and blocks; the reply capability lands in its cspace.
    let mut a = msg_args(&[7, 8], 0, vec![]);
    a.slot = 10;
    let out = st.apply(t0, Call::Call, &a);
    assert_eq!(out.error, Error::Ok);
    assert!(out.blocked);
    let reply_slot = out.values[0] as u32;
    assert!(matches!(
        st.objects[&cap_of(&st, t0, reply_slot).obj],
        Object::Reply(_)
    ));
    // t1 answers with reply_recv: it gets the message and the reply capability.
    let mut a = Args::new();
    a.slot = 10;
    let out = st.apply(t1, Call::ReplyRecv, &a);
    assert_eq!(out.error, Error::Ok);
    let Some(Inbox::Message {
        received,
        words,
        reply_slot: rs,
    }) = out.inbox
    else {
        panic!("expected message, got {:?}", out.inbox)
    };
    assert_eq!(words, vec![7, 8]);
    assert_eq!(received.header.label(), 0x1234);
    let rs = rs.expect("reply_recv must hand over the reply capability");
    // The reply capability now sits in t1's cspace (same cspace here).
    assert!(matches!(
        st.objects[&cap_of(&st, t1, rs).obj],
        Object::Reply(_)
    ));
    // t1 sends the reply on the reply object.
    let mut a = msg_args(&[42], 0, vec![]);
    a.slot = rs;
    let out = st.apply(t1, Call::Send, &a);
    assert_eq!(out.error, Error::Ok);
    // t0 is woken with the reply message.
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    assert_eq!(out.error, Error::Ok);
    match out.inbox {
        Some(Inbox::Message {
            words, received, ..
        }) => {
            assert_eq!(words, vec![42]);
            assert_eq!(received.header.words(), 1);
        }
        other => panic!("expected reply, got {other:?}"),
    }
    // The reply was one-shot: its slot is dead, the object is gone.
    assert_eq!(simple(&mut st, t1, Call::CapInspect, rs).error, Error::Gone);
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = rs;
    assert_eq!(st.apply(t1, Call::Send, &a).error, Error::Gone);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn call_waits_in_queue_until_a_server_receives() {
    let (mut st, t0, t1) = ipc_setup();
    let mut a = msg_args(&[9], 0, vec![]);
    a.slot = 10;
    assert!(st.apply(t0, Call::Call, &a).blocked);
    // Nobody was waiting: the message is queued with its reply object.
    let mut a = Args::new();
    a.slot = 10;
    let out = st.apply(t1, Call::ReplyRecv, &a);
    match out.inbox {
        Some(Inbox::Message {
            words, reply_slot, ..
        }) => {
            assert_eq!(words, vec![9]);
            assert!(reply_slot.is_some());
        }
        other => panic!("got {other:?}"),
    }
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn plain_recv_does_not_take_the_reply_capability() {
    let (mut st, t0, t1) = ipc_setup();
    let mut a = msg_args(&[5], 0, vec![]);
    a.slot = 10;
    let out = st.apply(t0, Call::Call, &a);
    let reply_slot = out.values[0] as u32;
    let mut a = Args::new();
    a.slot = 10;
    let out = st.apply(t1, Call::Recv, &a);
    match out.inbox {
        Some(Inbox::Message { reply_slot, .. }) => assert!(reply_slot.is_none()),
        other => panic!("got {other:?}"),
    }
    // The reply capability stayed with the caller; the caller is still blocked.
    assert!(matches!(
        st.threads[&t0].state,
        ThreadState::BlockedReply { .. }
    ));
    assert_eq!(
        cap_of(&st, t0, reply_slot).obj,
        cap_of(&st, t0, reply_slot).obj
    );
    // Deleting the reply capability wakes the caller with `gone`.
    assert_eq!(
        simple(&mut st, t1, Call::CapDelete, reply_slot).error,
        Error::Ok
    );
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    assert_eq!(out.inbox, Some(Inbox::Error(Error::Gone)));
    assert!(matches!(st.threads[&t0].state, ThreadState::Runnable));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn endpoint_destroyed_while_caller_blocked_wakes_it_gone() {
    let (mut st, t0, t1) = ipc_setup();
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 10;
    assert!(st.apply(t0, Call::Call, &a).blocked);
    // t1 deletes its capability to the endpoint (same cspace): the endpoint
    // is destroyed and t0's blocked call completes with `gone`.
    assert_eq!(simple(&mut st, t1, Call::CapDelete, 10).error, Error::Ok);
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    assert_eq!(out.inbox, Some(Inbox::Error(Error::Gone)));
    assert!(matches!(st.threads[&t0].state, ThreadState::Runnable));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// Capability transfer in messages
// ---------------------------------------------------------------------

#[test]
fn grant_right_is_required_to_transfer_capabilities() {
    let (mut st, t0, t1) = ipc_setup();
    // Endpoint capability without `grant`.
    let out = derive(&mut st, t0, 10, 14, Rights::SEND.union(Rights::RECV), 0);
    assert_eq!(out.error, Error::Ok);
    // A capability to transfer: a notify.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(
        &[1],
        1,
        vec![Transfer {
            slot: 20,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 14;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Denied);
    // Without capabilities in the message, no grant is needed.
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 14;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    // Drain that queued message so t1 can block again below.
    let mut a = Args::new();
    a.slot = 10;
    let out = st.apply(t1, Call::Recv, &a);
    assert!(out.inbox.is_some());
    // Through the full-rights capability it works.
    let cap20 = cap_of(&st, t0, 20);
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 30;
    a.recv_count = 2;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    let mut a = msg_args(
        &[1],
        1,
        vec![Transfer {
            slot: 20,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    match out.inbox {
        Some(Inbox::Message { received, .. }) => {
            assert_eq!(received.header.caps(), 1);
            assert_eq!(received.header.flags() & Header::FLAG_CAPS_DROPPED, 0);
        }
        other => panic!("got {other:?}"),
    }
    // The capability moved: slot 20 is empty, slot 30 holds the same node.
    assert_eq!(cap_of(&st, t1, 30).id, cap20.id);
    let cs = st.thread_cspace(t1).unwrap();
    match st.objects.get(&cs) {
        Some(Object::CSpace { slots }) => assert!(matches!(slots[20], SlotState::Empty)),
        _ => panic!(),
    }
    assert!(matches!(
        st.objects[&cap_of(&st, t1, 30).obj],
        Object::Notify(_)
    ));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn transfer_move_and_derive_and_revocation_of_derived() {
    let (mut st, t0, t1) = ipc_setup();
    // A notify to move, and one to derive from.
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        21,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    // t1 waits with a receive range of 2 slots at base 30.
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 30;
    a.recv_count = 2;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    // t0 sends one moved and one derived capability.
    let cap20 = cap_of(&st, t0, 20);
    let mut a = msg_args(
        &[1],
        2,
        vec![
            Transfer {
                slot: 20,
                mode: TransferMode::Move,
            },
            Transfer {
                slot: 21,
                mode: TransferMode::Derive(Rights::SEND),
            },
        ],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    assert!(out.inbox.is_some());
    let moved = cap_of(&st, t1, 30);
    let derived = cap_of(&st, t1, 31);
    let src21 = cap_of(&st, t0, 21);
    // Move: same node in the new slot, sender's slot empty.
    assert_eq!(moved.id, cap20.id);
    assert!(matches!(st.objects[&moved.obj], Object::Notify(_)));
    // Derive: new child of the sender's node, attenuated rights, badge 0.
    assert_eq!(derived.parent, Some(src21.id));
    assert_eq!(derived.rights, Rights::SEND);
    assert_eq!(derived.badge, 0);
    assert!(
        st.caps.contains_key(&src21.id),
        "sender keeps its capability"
    );
    // Revoking the sender's capability kills the derived one in the receiver.
    assert_eq!(simple(&mut st, t0, Call::CapRevoke, 21).error, Error::Ok);
    assert_eq!(simple(&mut st, t1, Call::CapInspect, 31).error, Error::Gone);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn transfer_is_all_or_nothing_with_caps_dropped() {
    let (mut st, t0, t1) = ipc_setup();
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        21,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    // Receiver declares room for only one capability: both are dropped.
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 30;
    a.recv_count = 1;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    let mut a = msg_args(
        &[1],
        2,
        vec![
            Transfer {
                slot: 20,
                mode: TransferMode::Move,
            },
            Transfer {
                slot: 21,
                mode: TransferMode::Move,
            },
        ],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    match out.inbox {
        Some(Inbox::Message { received, .. }) => {
            assert_ne!(
                received.header.flags() & Header::FLAG_CAPS_DROPPED,
                0,
                "caps_dropped must be set"
            );
        }
        other => panic!("got {other:?}"),
    }
    // Nothing moved: the sender kept both, the receiver's range is untouched.
    assert!(matches!(
        st.objects[&cap_of(&st, t0, 20).obj],
        Object::Notify(_)
    ));
    let cs = st.thread_cspace(t1).unwrap();
    match st.objects.get(&cs) {
        Some(Object::CSpace { slots }) => assert!(matches!(slots[30], SlotState::Empty)),
        _ => panic!(),
    }
    // Occupied slot in the range: also dropped.
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 20; // occupied by the notify capability
    a.recv_count = 2;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    let mut a = msg_args(
        &[2],
        1,
        vec![Transfer {
            slot: 20,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t1, Call::CapInspect, 10);
    match out.inbox {
        Some(Inbox::Message { received, .. }) => {
            assert_ne!(received.header.flags() & Header::FLAG_CAPS_DROPPED, 0);
        }
        other => panic!("got {other:?}"),
    }
    assert!(st.caps.contains_key(&cap_of(&st, t0, 20).id));
    // Bad transfer slot fails the whole send.
    let mut a = msg_args(
        &[3],
        1,
        vec![Transfer {
            slot: 99,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Invalid);
    // Impossible derivation (right not held): denied.
    let out = derive(&mut st, t0, 20, 22, Rights::READ.union(Rights::DERIVE), 0);
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(
        &[4],
        1,
        vec![Transfer {
            slot: 22,
            mode: TransferMode::Derive(Rights::EXEC),
        }],
    );
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Denied);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn caller_is_told_about_dropped_caps_in_the_reply() {
    let (mut st, t0, t1) = ipc_setup();
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    // t0 calls with one capability; t1 receives with no room for it.
    let mut a = msg_args(
        &[1],
        1,
        vec![Transfer {
            slot: 20,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 10;
    a.recv_base = 30;
    a.recv_count = 0; // no room in the reply either
    let out = st.apply(t0, Call::Call, &a);
    assert!(out.blocked);
    let mut a = Args::new();
    a.slot = 10;
    a.recv_base = 40;
    a.recv_count = 0; // too few slots: dropped
    let out = st.apply(t1, Call::ReplyRecv, &a);
    let Some(Inbox::Message {
        received,
        reply_slot,
        ..
    }) = out.inbox
    else {
        panic!()
    };
    assert_ne!(received.header.flags() & Header::FLAG_CAPS_DROPPED, 0);
    let rs = reply_slot.expect("reply capability");
    // The sender's capability was not moved.
    assert!(st.caps.contains_key(&cap_of(&st, t0, 20).id));
    // t1 replies; the caller sees caps_dropped in the reply's header.
    let mut a = msg_args(&[2], 0, vec![]);
    a.slot = rs;
    assert_eq!(st.apply(t1, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    match out.inbox {
        Some(Inbox::Message { received, .. }) => {
            assert_ne!(received.header.flags() & Header::FLAG_CAPS_DROPPED, 0);
        }
        other => panic!("got {other:?}"),
    }
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn call_needs_a_free_slot_for_the_reply_capability() {
    let (mut st, t0, _t1) = ipc_setup();
    // Fill every remaining slot of the cspace with derived capabilities.
    while let Some(s) = first_free(&st, t0) {
        let out = derive(&mut st, t0, BOOT_SLOT_UNTYPED, s, Rights::READ, 0);
        assert_eq!(out.error, Error::Ok);
    }
    assert_eq!(first_free(&st, t0), None);
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 10;
    assert_eq!(st.apply(t0, Call::Call, &a).error, Error::NoSlot);
    assert!(matches!(st.threads[&t0].state, ThreadState::Runnable));
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------

#[test]
fn signal_wait_poll_accumulate_badge_bits() {
    let (mut st, t0, t1) = ipc_setup();
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    // poll on an empty notify: would_wait.
    assert_eq!(simple(&mut st, t0, Call::Poll, 20).error, Error::WouldWait);
    // signal accumulates bits with no waiter.
    let mut a = Args::new();
    a.slot = 20;
    a.badge = 0b01;
    assert_eq!(st.apply(t0, Call::Signal, &a).values, vec![0b01]);
    let mut a = Args::new();
    a.slot = 20;
    a.badge = 0b10;
    st.apply(t1, Call::Signal, &a);
    let out = simple(&mut st, t0, Call::Poll, 20);
    assert_eq!(out.error, Error::Ok);
    assert_eq!(out.values, vec![0b11]);
    assert_eq!(simple(&mut st, t0, Call::Poll, 20).error, Error::WouldWait);
    // wait blocks; a signal from another thread wakes it with the bits and
    // clears the word.
    assert!(simple(&mut st, t0, Call::Wait, 20).blocked);
    let mut a = Args::new();
    a.slot = 20;
    a.badge = 0x40;
    assert_eq!(st.apply(t1, Call::Signal, &a).error, Error::Ok);
    assert_eq!(
        simple(&mut st, t1, Call::Poll, 20).error,
        Error::WouldWait,
        "word was consumed by the waiter"
    );
    // The woken thread's next apply carries the notification.
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    assert_eq!(out.inbox, Some(Inbox::Notification(0x40)));
    // wait with bits already pending returns immediately.
    let mut a = Args::new();
    a.slot = 20;
    a.badge = 0x80;
    st.apply(t1, Call::Signal, &a);
    let out = simple(&mut st, t0, Call::Wait, 20);
    assert_eq!(out.error, Error::Ok);
    assert!(!out.blocked);
    assert_eq!(out.values, vec![0x80]);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn signal_wakes_all_waiters_and_needs_rights() {
    let (mut st, t0, t1) = ipc_setup();
    // A third thread to signal with.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        21,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        None,
    );
    let t2 = out.values[0] as ThreadId;
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert!(simple(&mut st, t0, Call::Wait, 20).blocked);
    assert!(simple(&mut st, t1, Call::Wait, 20).blocked);
    let mut a = Args::new();
    a.slot = 20;
    a.badge = 0x5;
    assert_eq!(st.apply(t2, Call::Signal, &a).error, Error::Ok);
    // Both waiters woke with the same bits.
    for t in [t0, t1] {
        let out = simple(&mut st, t, Call::CapInspect, BOOT_SLOT_UNTYPED);
        assert_eq!(out.inbox, Some(Inbox::Notification(0x5)));
    }
    // signal needs SEND, wait/poll need RECV.
    let out = derive(&mut st, t0, 20, 22, Rights::RECV, 0);
    assert_eq!(out.error, Error::Ok);
    let mut a = Args::new();
    a.slot = 22;
    a.badge = 1;
    assert_eq!(st.apply(t0, Call::Signal, &a).error, Error::Denied);
    let out = derive(&mut st, t0, 20, 23, Rights::SEND, 0);
    assert_eq!(out.error, Error::Ok);
    assert_eq!(simple(&mut st, t0, Call::Wait, 23).error, Error::Denied);
    assert_eq!(simple(&mut st, t0, Call::Poll, 23).error, Error::Denied);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn wait_on_endpoint_with_bound_notify() {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        None,
    );
    let t1 = out.values[0] as ThreadId;
    // A notify, then an endpoint bound to it at creation time.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        Some(10),
    );
    assert_eq!(out.error, Error::Ok);
    // wait on the endpoint blocks on the bound notify.
    assert!(simple(&mut st, t0, Call::Wait, 11).blocked);
    let mut a = Args::new();
    a.slot = 10;
    a.badge = 0x77;
    assert_eq!(st.apply(t1, Call::Signal, &a).error, Error::Ok);
    let out = simple(&mut st, t0, Call::CapInspect, BOOT_SLOT_UNTYPED);
    assert_eq!(out.inbox, Some(Inbox::Notification(0x77)));
    // An endpoint without a bound notify: invalid.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        12,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    assert_eq!(simple(&mut st, t0, Call::Wait, 12).error, Error::Invalid);
    // A queued message on an endpoint with a bound notify signals it.
    let out = derive(&mut st, t0, 11, 13, Rights::SEND.union(Rights::GRANT), 0x10);
    assert_eq!(out.error, Error::Ok);
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 13;
    assert_eq!(st.apply(t1, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t0, Call::Poll, 10);
    assert_eq!(out.values, vec![0x10]);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// Cross-cspace: move and IPC transfer
// ---------------------------------------------------------------------

#[test]
fn caps_cross_cspaces_by_move_and_by_ipc() {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    // cspace B in slot 10, thread t1 bound to it.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        32,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        Some(10),
    );
    let t1 = out.values[0] as ThreadId;
    // An endpoint in the boot cspace.
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        12,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    // Move the endpoint capability into cspace B (slot 0 of B).
    let mut a = Args::new();
    a.slot = 12;
    a.dst_slot = 0;
    a.dst_cspace_slot = Some(10);
    assert_eq!(st.apply(t0, Call::CapMove, &a).error, Error::Ok);
    // t1 can now receive on it (slot 0 of its own cspace).
    let mut a = Args::new();
    a.slot = 0;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    // t0 no longer holds the endpoint capability in slot 12.
    assert_eq!(
        simple(&mut st, t0, Call::CapInspect, 12).error,
        Error::Invalid
    );
    // Second part: keep a derived endpoint capability, then move the original.
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        10,
        ObjectKind::CSpace,
        32,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        11,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        Some(10),
    );
    let t1 = out.values[0] as ThreadId;
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        12,
        ObjectKind::Endpoint,
        0,
        PageSize::Size4K,
        None,
    );
    let out = derive(
        &mut st,
        t0,
        12,
        13,
        Rights::SEND.union(Rights::RECV).union(Rights::GRANT),
        0x33,
    );
    assert_eq!(out.error, Error::Ok);
    let mut a = Args::new();
    a.slot = 12;
    a.dst_slot = 0;
    a.dst_cspace_slot = Some(10);
    assert_eq!(st.apply(t0, Call::CapMove, &a).error, Error::Ok);
    let mut a = Args::new();
    a.slot = 0;
    a.recv_base = 5;
    a.recv_count = 2;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    // A notify to transfer by IPC.
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        20,
        ObjectKind::Notify,
        0,
        PageSize::Size4K,
        None,
    );
    let mut a = msg_args(
        &[1],
        1,
        vec![Transfer {
            slot: 20,
            mode: TransferMode::Move,
        }],
    );
    a.slot = 13;
    assert_eq!(st.apply(t0, Call::Send, &a).error, Error::Ok);
    let out = simple(&mut st, t1, Call::CapInspect, 0);
    match out.inbox {
        Some(Inbox::Message { received, .. }) => {
            assert_eq!(received.badge, 0x33);
            assert_eq!(received.header.caps(), 1);
        }
        other => panic!("got {other:?}"),
    }
    // The notify capability now sits in cspace B, slot 5.
    let cap = cap_of(&st, t1, 5);
    assert!(matches!(st.objects[&cap.obj], Object::Notify(_)));
    // And t1 can use it: poll on it.
    assert_eq!(simple(&mut st, t1, Call::Poll, 5).error, Error::WouldWait);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// Scope and dead cspaces
// ---------------------------------------------------------------------

#[test]
fn out_of_scope_calls_return_unsupported() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    for call in [
        Call::ASpaceMap,
        Call::ASpaceUnmap,
        Call::ASpaceProtect,
        Call::ThreadConfigure,
        Call::ThreadReadRegs,
        Call::ThreadWriteRegs,
        Call::ThreadResume,
        Call::ThreadSuspend,
        Call::Yield,
        Call::SchedConfigure,
        Call::TimerSet,
        Call::IrqBind,
        Call::TaskCreate,
        Call::CoreStart,
        Call::DebugWrite,
        Call::TraceEmit,
    ] {
        let mut a = Args::new();
        a.slot = BOOT_SLOT_UNTYPED;
        assert_eq!(st.apply(t, call, &a).error, Error::Unsupported, "{call:?}");
    }
}

#[test]
fn dead_cspace_makes_every_call_gone() {
    let mut st = State::new();
    let t = BOOT_THREAD;
    // Delete the only capability to the boot cspace.
    assert_eq!(
        simple(&mut st, t, Call::CapDelete, BOOT_SLOT_CSPACE).error,
        Error::Ok
    );
    assert_eq!(st.thread_cspace(t), None);
    assert_eq!(
        simple(&mut st, t, Call::CapInspect, BOOT_SLOT_UNTYPED).error,
        Error::Gone
    );
    let mut a = msg_args(&[1], 0, vec![]);
    a.slot = 10;
    assert_eq!(st.apply(t, Call::Send, &a).error, Error::Gone);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

#[test]
fn blocked_thread_cannot_make_a_new_call() {
    let (mut st, _t0, t1) = ipc_setup();
    let mut a = Args::new();
    a.slot = 10;
    assert!(st.apply(t1, Call::Recv, &a).blocked);
    assert_eq!(simple(&mut st, t1, Call::CapInspect, 10).error, Error::Busy);
    // A call on a thread that does not exist: gone.
    assert_eq!(simple(&mut st, 999, Call::CapInspect, 0).error, Error::Gone);
    assert!(
        check_invariants(&st).is_empty(),
        "{:?}",
        check_invariants(&st)
    );
}

// ---------------------------------------------------------------------
// Property test: deterministic PRNG, random call sequences, invariants
// after every step, fixed seeds.
// ---------------------------------------------------------------------

/// splitmix64, hand-written so the crate needs no dev-dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_add(0xA076_1D64_78BD_642F))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

fn random_rights(rng: &mut Rng) -> Rights {
    Rights::from_bits(rng.below(256) as u32).unwrap_or(Rights::NONE)
}

fn random_kind(rng: &mut Rng) -> ObjectKind {
    match rng.below(10) {
        0 => ObjectKind::Untyped,
        1 => ObjectKind::CSpace,
        2 => ObjectKind::Frame,
        3 => ObjectKind::Thread,
        4 => ObjectKind::Endpoint,
        5 => ObjectKind::Notify,
        // Out of scope on purpose, to exercise `unsupported`.
        6 => ObjectKind::ASpace,
        7 => ObjectKind::Sched,
        8 => ObjectKind::Task,
        _ => ObjectKind::Reply,
    }
}

fn random_call(rng: &mut Rng, st: &State, caller: ThreadId) -> (Call, Args) {
    let len = st
        .thread_cspace(caller)
        .and_then(|c| st.cspace_len(c))
        .unwrap_or(1)
        .max(1) as u64;
    let slot = |rng: &mut Rng| rng.below(len) as u32;
    let maybe_dst_cspace = |rng: &mut Rng| {
        if rng.below(8) == 0 {
            Some(rng.below(len) as u32)
        } else {
            None
        }
    };
    let mut a = Args::new();
    let w = rng.below(100);
    match w {
        0..=9 => {
            a.slot = slot(rng);
            a.dst_slot = slot(rng);
            a.rights = random_rights(rng);
            a.badge = rng.next();
            a.dst_cspace_slot = maybe_dst_cspace(rng);
            (Call::CapDerive, a)
        }
        10..=17 => {
            a.slot = slot(rng);
            a.dst_slot = slot(rng);
            a.dst_cspace_slot = maybe_dst_cspace(rng);
            (Call::CapMove, a)
        }
        18..=25 => {
            a.slot = slot(rng);
            (Call::CapDelete, a)
        }
        26..=31 => {
            a.slot = slot(rng);
            (Call::CapRevoke, a)
        }
        32..=36 => {
            a.slot = slot(rng);
            (Call::CapInspect, a)
        }
        37..=48 => {
            a.slot = slot(rng);
            a.dst_slot = slot(rng);
            a.kind = random_kind(rng);
            a.size = if rng.below(2) == 0 {
                rng.below(1 << 12)
            } else {
                rng.below(1 << 28)
            };
            a.page_size =
                [PageSize::Size4K, PageSize::Size2M, PageSize::Size1G][rng.below(3) as usize];
            a.aux_slot = if rng.below(3) == 0 {
                Some(slot(rng))
            } else {
                None
            };
            (Call::UntypedRetype, a)
        }
        49..=60 => {
            a.slot = slot(rng);
            random_message(rng, &mut a, len);
            (Call::Send, a)
        }
        61..=70 => {
            a.slot = slot(rng);
            a.recv_base = slot(rng);
            a.recv_count = rng.below(5) as u32;
            (Call::Recv, a)
        }
        71..=78 => {
            a.slot = slot(rng);
            a.recv_base = slot(rng);
            a.recv_count = rng.below(5) as u32;
            (Call::ReplyRecv, a)
        }
        79..=88 => {
            a.slot = slot(rng);
            random_message(rng, &mut a, len);
            a.recv_base = slot(rng);
            a.recv_count = rng.below(5) as u32;
            (Call::Call, a)
        }
        89..=93 => {
            a.slot = slot(rng);
            a.badge = rng.next();
            (Call::Signal, a)
        }
        94..=97 => {
            a.slot = slot(rng);
            (Call::Wait, a)
        }
        _ => {
            a.slot = slot(rng);
            (Call::Poll, a)
        }
    }
}

fn random_message(rng: &mut Rng, a: &mut Args, len: u64) {
    let words = rng.below(9) as usize; // 0..=8, sometimes over the 6 register words
    let caps = rng.below(5) as usize; // 0..=MAX_CAPS
    a.header = Header::new(rng.next() as u32, words, caps, 0).expect("valid header");
    a.words = (0..words).map(|_| rng.next()).collect();
    a.transfers = (0..caps)
        .map(|_| Transfer {
            slot: rng.below(len) as u32,
            mode: if rng.below(2) == 0 {
                TransferMode::Move
            } else {
                TransferMode::Derive(random_rights(rng))
            },
        })
        .collect();
}

fn run_sequence(seed: u64, steps: usize) -> State {
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    // A second cspace and threads on both cspaces.
    let cs_b_slot = free_slot(&st, t0);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        cs_b_slot,
        ObjectKind::CSpace,
        24,
        PageSize::Size4K,
        None,
    );
    assert_eq!(out.error, Error::Ok);
    for i in 0..3 {
        let dst = free_slot(&st, t0);
        let out = retype(
            &mut st,
            t0,
            BOOT_SLOT_UNTYPED,
            dst,
            ObjectKind::Thread,
            0,
            PageSize::Size4K,
            None,
        );
        assert_eq!(out.error, Error::Ok);
        let _ = i;
    }
    let dst = free_slot(&st, t0);
    let out = retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        dst,
        ObjectKind::Thread,
        0,
        PageSize::Size4K,
        Some(cs_b_slot),
    );
    assert_eq!(out.error, Error::Ok);
    let mut rng = Rng::new(seed);
    for step in 0..steps {
        let runnable = st.runnable();
        if runnable.is_empty() {
            break; // every thread is blocked: nothing more can happen
        }
        let caller = runnable[rng.below(runnable.len() as u64) as usize];
        let (call, args) = random_call(&mut rng, &st, caller);
        let _ = st.apply(caller, call, &args);
        let problems = check_invariants(&st);
        assert!(
            problems.is_empty(),
            "seed {seed} step {step} call {call:?} caller {caller}: {problems:?}"
        );
    }
    st
}

#[test]
fn property_random_sequences_keep_invariants_and_are_deterministic() {
    const STEPS: usize = 1500;
    for seed in [1u64, 2, 3, 4, 5, 6, 7, 8, 42, 2026] {
        let a = run_sequence(seed, STEPS);
        let b = run_sequence(seed, STEPS);
        assert_eq!(a, b, "seed {seed}: the model is not deterministic");
        let problems = check_invariants(&a);
        assert!(problems.is_empty(), "seed {seed}: {problems:?}");
    }
}

#[test]
fn property_test_exercises_all_calls() {
    // Guard for the generator itself: over the fixed seeds, every modelled
    // call must appear at least once, otherwise the property test above does
    // not cover it.
    let mut seen: Vec<Call> = Vec::new();
    let mut st = State::new();
    let t0 = BOOT_THREAD;
    let cs_b_slot = free_slot(&st, t0);
    retype(
        &mut st,
        t0,
        BOOT_SLOT_UNTYPED,
        cs_b_slot,
        ObjectKind::CSpace,
        24,
        PageSize::Size4K,
        None,
    );
    let mut rng = Rng::new(7);
    for _ in 0..4000 {
        let runnable = st.runnable();
        if runnable.is_empty() {
            break;
        }
        let caller = runnable[rng.below(runnable.len() as u64) as usize];
        let (call, args) = random_call(&mut rng, &st, caller);
        if !seen.contains(&call) {
            seen.push(call);
        }
        let _ = st.apply(caller, call, &args);
        assert!(
            check_invariants(&st).is_empty(),
            "{:?}",
            check_invariants(&st)
        );
    }
    for call in [
        Call::CapDerive,
        Call::CapMove,
        Call::CapDelete,
        Call::CapRevoke,
        Call::CapInspect,
        Call::UntypedRetype,
        Call::Send,
        Call::Recv,
        Call::ReplyRecv,
        Call::Call,
        Call::Signal,
        Call::Wait,
        Call::Poll,
    ] {
        assert!(seen.contains(&call), "{call:?} never generated");
    }
}
