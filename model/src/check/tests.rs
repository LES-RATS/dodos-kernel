//! Each invariant is reported when a state breaks it: `spoiled` takes a
//! valid state, breaks one thing, and returns what `check` reports.

use crate::*;
use dodos_abi::TREE_DEPTH;

const EP: u32 = 20;
const RP: u32 = 21;
const NT: u32 = 22;
const WAITER: u32 = 23;
const COPY: u32 = 24;
const CALLER: u32 = 26;

/// An endpoint, a reply object and a notify, a thread that waits on the
/// notify, a thread in the queue of the endpoint, and two copies of the
/// endpoint capability in `COPY` and after.
fn state() -> Model {
    let (mut model, boot) = Model::boot();
    let own = SlotAddr::own;
    let mut on = |thread: ObjId, call: Syscall| model.apply(thread, call).unwrap();
    let mut threads = Vec::new();
    for (kind, slot) in [
        (ObjectKind::Endpoint, EP),
        (ObjectKind::Reply, RP),
        (ObjectKind::Notify, NT),
        (ObjectKind::Thread, WAITER),
        (ObjectKind::Thread, CALLER),
    ] {
        on(
            boot,
            Syscall::UntypedRetype {
                untyped: own(START_SLOT_FIRST),
                kind,
                size: 0,
                page: PageSize::Size4K,
                destination: own(slot),
            },
        );
        if kind == ObjectKind::Thread {
            on(
                boot,
                Syscall::ThreadConfigure {
                    thread: own(slot),
                    cspace: own(START_SLOT_CSPACE),
                    aspace: SlotAddr::NONE,
                    fault: SlotAddr::NONE,
                },
            );
            on(boot, Syscall::ThreadResume(own(slot)));
            threads.push(slot);
        }
    }
    for destination in [COPY, COPY + 1] {
        on(
            boot,
            Syscall::CapDerive {
                source: own(EP),
                destination: own(destination),
                rights: Rights::SEND,
                badge: 0,
            },
        );
    }
    let (waiter, caller) = (object(&model, WAITER), object(&model, CALLER));
    let mut on = |thread: ObjId, call: Syscall| model.apply(thread, call).unwrap();
    on(waiter, Syscall::Wait(own(NT)));
    on(
        caller,
        Syscall::Call {
            endpoint: own(EP),
            message: Message::default(),
            range: Range::NONE,
        },
    );
    assert_eq!(threads, [WAITER, CALLER]);
    model
}

/// The capability in a slot of the first capability space.
fn cap(model: &Model, index: u32) -> CapId {
    let space = model.cspaces.values().next().unwrap();
    match space.slots[index as usize] {
        Slot::Full(cap) => cap,
        other => panic!("slot {index} is {other:?}"),
    }
}

fn object(model: &Model, index: u32) -> ObjId {
    model.caps[&cap(model, index)].object
}

/// What the tests name in the state.
#[derive(Clone, Copy)]
struct At {
    space: ObjId,
    untyped: ObjId,
    endpoint: ObjId,
    reply: ObjId,
    notify: ObjId,
    /// The first thread, which is ready.
    ready: ObjId,
    waiter: ObjId,
    untyped_cap: CapId,
    endpoint_cap: CapId,
    notify_cap: CapId,
    copy: CapId,
    later_copy: CapId,
    nowhere: Window,
}

/// Takes a capability out of the children of its parent.
fn orphan(model: &mut Model, cap: CapId) {
    let parent = model.caps[&cap].parent.unwrap();
    live(&mut model.caps, parent)
        .children
        .retain(|&child| child != cap);
}

/// Removes the root capability of an object and leaves the object.
fn without_root(model: &mut Model, object: ObjId) {
    let root = model.objects[&object].root;
    orphan(model, root);
    let (space, index) = model.caps.remove(&root).unwrap().place;
    live(&mut model.cspaces, space).slots[index as usize] = Slot::Empty;
}

fn spoiled(spoil: impl FnOnce(&mut Model, At)) -> &'static str {
    let mut model = state();
    assert_eq!(model.check(), Ok(()));
    let space = object(&model, START_SLOT_CSPACE);
    let at = At {
        space,
        untyped: object(&model, START_SLOT_FIRST),
        endpoint: object(&model, EP),
        reply: object(&model, RP),
        notify: object(&model, NT),
        ready: object(&model, START_SLOT_THREAD),
        waiter: object(&model, WAITER),
        untyped_cap: cap(&model, START_SLOT_FIRST),
        endpoint_cap: cap(&model, EP),
        notify_cap: cap(&model, NT),
        copy: cap(&model, COPY),
        later_copy: cap(&model, COPY + 1),
        nowhere: Window {
            cspace: space,
            first: 0,
            count: 0,
        },
    };
    spoil(&mut model, at);
    model.check().expect_err("the state is spoiled")
}

#[test]
fn parent_that_does_not_exist_is_reported() {
    let broken = spoiled(|m, at| {
        orphan(m, at.copy);
        live(&mut m.caps, at.copy).parent = Some(CapId(u32::MAX));
    });
    assert_eq!(broken, "1: parent exists");
}

#[test]
fn parent_younger_than_its_child_is_reported() {
    let broken = spoiled(|m, at| {
        orphan(m, at.copy);
        live(&mut m.caps, at.copy).parent = Some(at.later_copy);
        live(&mut m.caps, at.later_copy).children.push(at.copy);
    });
    assert_eq!(broken, "1: parent is older");
}

#[test]
fn parent_that_does_not_list_its_child_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.caps, at.endpoint_cap).children.clear());
    assert_eq!(broken, "1: parent lists its child once");
}

#[test]
fn child_that_names_another_parent_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.caps, at.endpoint_cap)
            .children
            .push(at.notify_cap)
    });
    assert_eq!(broken, "2: children name it");
}

#[test]
fn children_out_of_the_order_of_derivation_are_reported() {
    let broken = spoiled(|m, at| live(&mut m.caps, at.endpoint_cap).children.reverse());
    assert_eq!(broken, "2: order of derivation");
}

#[test]
fn derivation_with_more_rights_than_its_parent_is_reported() {
    let broken = spoiled(|m, at| m.derive(at.copy, Rights::ALL, 0, (at.space, 100)));
    assert_eq!(broken, "3: rights of a derivation");
}

#[test]
fn derivation_that_lost_the_badge_of_its_parent_is_reported() {
    let broken = spoiled(|m, at| {
        m.derive(at.copy, Rights::NONE, 0, (at.space, 100));
        live(&mut m.caps, at.copy).badge = 5;
    });
    assert_eq!(broken, "3: badge of a derivation");
}

#[test]
fn capability_that_is_neither_a_root_nor_a_derivation_is_reported() {
    let broken = spoiled(|m, at| {
        orphan(m, at.copy);
        live(&mut m.caps, at.copy).parent = Some(at.untyped_cap);
        live(&mut m.caps, at.untyped_cap).children.push(at.copy);
    });
    assert_eq!(broken, "4: root or derivation");
}

#[test]
fn dead_slot_0_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).slots[0] = Slot::Dead);
    assert_eq!(broken, "5: slot 0");
}

#[test]
fn full_slot_0_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.cspaces, at.space).slots[0] = Slot::Full(at.copy);
        live(&mut m.caps, at.copy).place = (at.space, 0);
        live(&mut m.cspaces, at.space).slots[COPY as usize] = Slot::Empty;
    });
    assert_eq!(broken, "5: slot 0");
}

#[test]
fn capability_that_is_not_in_the_slot_it_names_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.caps, at.copy).place = (at.space, 100));
    assert_eq!(broken, "6: slot of a capability");
}

#[test]
fn full_slot_whose_capability_names_another_slot_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).slots[100] = Slot::Full(at.copy));
    assert_eq!(broken, "6: capability of a full slot");
}

#[test]
fn capability_space_with_a_partial_segment_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.cspaces, at.space).slots.pop();
    });
    assert_eq!(broken, "7: segments");
}

#[test]
fn capability_space_over_its_maximum_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).max = 0);
    assert_eq!(broken, "7: segments");
}

#[test]
fn capability_whose_object_does_not_exist_is_reported() {
    let broken = spoiled(|m, at| {
        m.objects.remove(&at.endpoint);
    });
    assert_eq!(broken, "8: object of a capability");
}

#[test]
fn object_whose_root_capability_does_not_exist_is_reported() {
    let broken = spoiled(|m, at| without_root(m, at.reply));
    assert_eq!(broken, "9: root capability exists");
}

#[test]
fn object_whose_root_capability_names_another_object_is_reported() {
    let broken = spoiled(|m, at| {
        without_root(m, at.reply);
        live(&mut m.objects, at.reply).root = at.copy;
    });
    assert_eq!(broken, "9: root capability names its object");
}

#[test]
fn object_without_the_state_of_its_kind_is_reported() {
    let broken = spoiled(|m, at| {
        m.replies.remove(&at.reply);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn state_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        m.replies.insert(ObjId(u32::MAX), ReplyState::Idle);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn object_taken_from_an_untyped_that_does_not_exist_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.objects, at.reply).from = Some(ObjId(u32::MAX)));
    assert_eq!(broken, "11: untyped is live");
}

#[test]
fn object_whose_root_is_not_a_child_of_its_untyped_is_reported() {
    let broken = spoiled(|m, at| {
        let place = (at.space, 100);
        let other = m.create(
            ObjectKind::Untyped,
            4096,
            Some(at.untyped_cap),
            Rights::ALL,
            place,
        );
        let other = m.caps[&other].object;
        live(&mut m.objects, at.reply).from = Some(other);
    });
    assert_eq!(broken, "11: root capability is a child of the untyped's");
}

#[test]
fn thread_in_the_queue_of_an_endpoint_it_does_not_wait_on_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.endpoints, at.endpoint)
            .queue
            .push_back(at.ready)
    });
    assert_eq!(broken, "12: queue of an endpoint");
}

#[test]
fn caller_and_receiver_in_one_queue_are_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.threads, at.ready).wait = Wait::Recv {
            endpoint: at.endpoint,
            reply: at.reply,
            window: at.nowhere,
        };
        live(&mut m.endpoints, at.endpoint)
            .queue
            .push_back(at.ready);
    });
    assert_eq!(broken, "12: callers and receivers together");
}

#[test]
fn thread_in_the_queue_of_a_notify_it_does_not_wait_on_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.notifies, at.notify).waiters.push_back(at.ready));
    assert_eq!(broken, "13: queue of a notify");
}

#[test]
fn notify_with_a_waiter_and_a_word_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.notifies, at.notify).word = 1);
    assert_eq!(broken, "13: word of a notify with a waiter");
}

#[test]
fn thread_queued_twice_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.notifies, at.notify)
            .waiters
            .push_back(at.waiter)
    });
    assert_eq!(broken, "14: a thread queued twice");
}

#[test]
fn waiting_thread_that_is_in_no_queue_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.notifies, at.notify).waiters.clear());
    assert_eq!(broken, "14: a waiting thread not queued");
}

#[test]
fn attached_reply_object_whose_thread_does_not_wait_with_it_is_reported() {
    let broken = spoiled(|m, at| *live(&mut m.replies, at.reply) = ReplyState::Attached(at.waiter));
    assert_eq!(broken, "15: thread of a reply object");
}

#[test]
fn armed_reply_object_whose_caller_does_not_wait_for_it_is_reported() {
    let broken = spoiled(|m, at| {
        *live(&mut m.replies, at.reply) = ReplyState::Armed {
            caller: at.waiter,
            window: at.nowhere,
            flags: 0,
        };
    });
    assert_eq!(broken, "15: thread of a reply object");
}

#[test]
fn thread_that_waits_for_a_reply_through_an_idle_reply_object_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).wait = Wait::Reply(at.reply));
    assert_eq!(broken, "15: reply object of a thread");
}

#[test]
fn thread_that_waits_to_receive_with_an_idle_reply_object_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.notifies, at.notify).waiters.clear();
        live(&mut m.endpoints, at.endpoint)
            .queue
            .push_back(at.waiter);
        live(&mut m.threads, at.waiter).wait = Wait::Recv {
            endpoint: at.endpoint,
            reply: at.reply,
            window: at.nowhere,
        };
        let caller = object(m, CALLER);
        live(&mut m.threads, caller).wait = Wait::None;
    });
    assert_eq!(broken, "15: reply object of a thread");
}

#[test]
fn outcome_of_a_thread_that_still_waits_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.waiter).outcome = Some(Ok(Output::Done)));
    assert_eq!(broken, "16: outcome while waiting");
}

#[test]
fn binding_that_only_the_endpoint_names_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.endpoints, at.endpoint).notify = Some(at.notify));
    assert_eq!(broken, "17: notify of an endpoint");
}

#[test]
fn binding_that_only_the_notify_names_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.notifies, at.notify).endpoint = Some(at.endpoint));
    assert_eq!(broken, "17: endpoint of a notify");
}

#[test]
fn watermark_beyond_the_untyped_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.untypeds, at.untyped).watermark = u64::MAX);
    assert_eq!(broken, "18: watermark within");
}

#[test]
fn watermark_at_zero_while_something_is_live_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.untypeds, at.untyped).watermark = 0);
    assert_eq!(broken, "18: watermark is zero when nothing is live");
}

#[test]
fn live_count_that_is_not_the_number_of_live_objects_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.untypeds, at.untyped).live += 1);
    assert_eq!(broken, "19: live count");
}

#[test]
fn thread_that_is_not_suspended_and_bound_to_nothing_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).cspace = None);
    assert_eq!(broken, "16: running while bound to nothing");
}

#[test]
fn tree_deeper_than_64_is_reported() {
    let broken = spoiled(|m, at| {
        let mut parent = at.later_copy;
        for index in 0..TREE_DEPTH as u32 {
            let rights = m.caps[&parent].rights;
            m.derive(parent, rights, 0, (at.space, 40 + index));
            parent = *m.caps[&parent].children.last().unwrap();
        }
    });
    assert_eq!(broken, "20: depth of the tree");
}

#[test]
fn capability_held_in_a_capability_space_that_descends_from_it_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = m.create(
            ObjectKind::CSpace,
            128,
            Some(at.untyped_cap),
            Rights::ALL,
            (at.space, 40),
        );
        let inner = m.caps[&inner].object;
        live(&mut m.cspaces, inner).slots = vec![Slot::Empty; 128];
        live(&mut m.untypeds, at.untyped).live += 2;
        m.place(at.untyped_cap, (inner, 1));
    });
    assert_eq!(broken, "21: removal that never ends");
}

#[test]
fn thread_attached_to_a_dying_reply_object_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.notifies, at.notify).waiters.clear();
        live(&mut m.endpoints, at.endpoint)
            .queue
            .push_back(at.waiter);
        live(&mut m.threads, at.waiter).wait = Wait::Recv {
            endpoint: at.endpoint,
            reply: at.reply,
            window: at.nowhere,
        };
        let caller = object(m, CALLER);
        live(&mut m.threads, caller).wait = Wait::None;
        *live(&mut m.replies, at.reply) = ReplyState::Attached(at.waiter);
        let root = m.objects[&at.reply].root;
        live(&mut m.caps, root).dying = Some(m.next_id);
    });
    assert_eq!(broken, "22: thread of a dying reply object");
}

#[test]
fn capability_created_in_a_capability_space_after_it_began_to_die_is_reported() {
    let broken = spoiled(|m, at| {
        let root = m.objects[&at.space].root;
        live(&mut m.caps, root).dying = Some(m.next_id);
        m.derive(at.copy, Rights::NONE, 0, (at.space, 100));
    });
    assert_eq!(broken, "23: new capability in a dying space");
}

#[test]
fn root_capability_without_every_right_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.caps, at.endpoint_cap).rights = Rights::NONE);
    assert_eq!(broken, "3: root capability");
}

#[test]
fn root_capability_with_a_badge_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.caps, at.endpoint_cap).badge = 5);
    assert_eq!(broken, "3: root capability");
}

#[test]
fn badge_on_a_capability_that_is_not_for_an_endpoint_or_a_notify_is_reported() {
    let broken = spoiled(|m, at| {
        m.derive(at.untyped_cap, Rights::NONE, 5, (at.space, 100));
    });
    assert_eq!(broken, "3: badge on another kind");
}

#[test]
fn start_up_capability_with_a_parent_is_reported() {
    let broken = spoiled(|m, at| {
        let device = cap(m, START_SLOT_FIRST + 1);
        live(&mut m.caps, device).parent = Some(at.untyped_cap);
        live(&mut m.caps, at.untyped_cap).children.insert(0, device);
    });
    assert_eq!(broken, "11: parent of a start-up capability");
}

#[test]
fn exec_on_device_memory_is_reported() {
    let broken = spoiled(|m, _| {
        let device = cap(m, START_SLOT_FIRST + 1);
        live(&mut m.caps, device).rights = Rights::ALL;
    });
    assert_eq!(broken, "25: exec on device memory");
}

#[test]
fn kernel_object_on_device_memory_is_reported() {
    let broken = spoiled(|m, at| {
        let device = object(m, START_SLOT_FIRST + 1);
        let from = cap(m, START_SLOT_FIRST + 1);
        let rights = Rights::ALL.without(Rights::EXEC);
        m.create(ObjectKind::Notify, 0, Some(from), rights, (at.space, 100));
        live(&mut m.untypeds, device).live += 1;
    });
    assert_eq!(broken, "25: kind on device memory");
}

#[test]
fn identity_that_was_not_given_out_is_reported() {
    let broken = spoiled(|m, _| m.next_id = 3);
    assert_eq!(broken, "26: identity");
}

#[test]
fn caller_of_a_dying_reply_object_is_reported() {
    let broken = spoiled(|m, at| {
        let caller = object(m, CALLER);
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.threads, caller).wait = Wait::Reply(at.reply);
        *live(&mut m.replies, at.reply) = ReplyState::Armed {
            caller,
            window: at.nowhere,
            flags: 0,
        };
        let root = m.objects[&at.reply].root;
        live(&mut m.caps, root).dying = Some(m.next_id);
    });
    assert_eq!(broken, "22: thread of a dying reply object");
}

#[test]
fn watermark_above_zero_while_nothing_is_live_is_reported() {
    let broken = spoiled(|m, _| {
        let device = object(m, START_SLOT_FIRST + 1);
        live(&mut m.untypeds, device).watermark = 1;
    });
    assert_eq!(broken, "18: watermark is zero when nothing is live");
}

#[test]
fn walk_of_a_removal_longer_than_192_capabilities_is_reported() {
    let broken = spoiled(|m, _| m.longest_walk = 193);
    assert_eq!(broken, "24: walk of a removal");
}

#[test]
fn walk_of_a_removal_is_measured() {
    let mut model = state();
    let boot = object(&model, START_SLOT_THREAD);
    assert_eq!(model.longest_walk, 0);
    // From the endpoint capability to its most recent copy.
    model
        .apply(boot, Syscall::CapDelete(SlotAddr::own(EP)))
        .unwrap();
    assert_eq!(model.longest_walk, 2);
}
