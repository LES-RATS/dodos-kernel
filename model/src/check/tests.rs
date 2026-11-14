//! Each invariant is reported when a state breaks it: `spoiled` takes a
//! valid state, breaks one thing, and returns what `check` reports.

use crate::*;
use dodos_abi::{Header, LONGEST_WALK, MAX_CAPS, MAX_WORDS, TREE_DEPTH};

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
    let Place { space, index } = model.caps.remove(&root).unwrap().place;
    live(&mut model.cspaces, space).slots[index as usize] = Slot::Empty;
}

fn places(model: &Model) -> At {
    let space = object(model, START_SLOT_CSPACE);
    At {
        space,
        untyped: object(model, START_SLOT_FIRST),
        endpoint: object(model, EP),
        reply: object(model, RP),
        notify: object(model, NT),
        ready: object(model, START_SLOT_THREAD),
        waiter: object(model, WAITER),
        untyped_cap: cap(model, START_SLOT_FIRST),
        endpoint_cap: cap(model, EP),
        notify_cap: cap(model, NT),
        copy: cap(model, COPY),
        later_copy: cap(model, COPY + 1),
        nowhere: Window {
            cspace: space,
            first: 0,
            count: 0,
        },
    }
}

fn spoiled(spoil: impl FnOnce(&mut Model, At)) -> &'static str {
    let mut model = state();
    assert_eq!(model.check(), Ok(()));
    let at = places(&model);
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
    let broken = spoiled(|m, at| {
        m.derive(
            at.copy,
            Rights::ALL,
            0,
            Place {
                space: at.space,
                index: 100,
            },
        )
    });
    assert_eq!(broken, "3: rights of a derivation");
}

#[test]
fn derivation_that_lost_the_badge_of_its_parent_is_reported() {
    let broken = spoiled(|m, at| {
        m.derive(
            at.copy,
            Rights::NONE,
            0,
            Place {
                space: at.space,
                index: 100,
            },
        );
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
        live(&mut m.caps, at.copy).place = Place {
            space: at.space,
            index: 0,
        };
        live(&mut m.cspaces, at.space).slots[COPY as usize] = Slot::Empty;
    });
    assert_eq!(broken, "5: slot 0");
}

#[test]
fn capability_that_is_not_in_the_slot_it_names_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.caps, at.copy).place = Place {
            space: at.space,
            index: 100,
        }
    });
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
fn object_older_than_the_untyped_it_was_taken_from_is_reported() {
    let broken = spoiled(|m, at| {
        let other = child_untyped(m, at, 100);
        live(&mut m.objects, at.reply).from = Some(other);
    });
    assert_eq!(broken, "26: object older than its untyped");
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
    assert_eq!(broken, "16: bound to nothing");
}

/// Copies of a copy that is at depth 3, each derived from the one before,
/// down to the depth given.
fn chain_down_to(m: &mut Model, at: At, depth: usize) {
    let mut parent = at.later_copy;
    for index in 0..depth as u32 - 3 {
        let rights = m.caps[&parent].rights;
        m.derive(
            parent,
            rights,
            0,
            Place {
                space: at.space,
                index: 40 + index,
            },
        );
        parent = *m.caps[&parent].children.last().unwrap();
    }
}

#[test]
fn tree_of_depth_65_is_reported() {
    let broken = spoiled(|m, at| chain_down_to(m, at, TREE_DEPTH + 1));
    assert_eq!(broken, "20: depth of the tree");
}

#[test]
fn tree_of_depth_64_is_accepted() {
    let mut model = state();
    let at = places(&model);
    chain_down_to(&mut model, at, TREE_DEPTH);
    assert_eq!(model.check(), Ok(()));
    assert_eq!(model.census().deepest, TREE_DEPTH);
}

#[test]
fn capability_held_in_a_capability_space_that_descends_from_it_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = m.create(
            ObjectKind::CSpace,
            128,
            Some(at.untyped_cap),
            Rights::ALL,
            Place {
                space: at.space,
                index: 40,
            },
        );
        let inner = m.caps[&inner].object;
        live(&mut m.cspaces, inner).slots = vec![Slot::Empty; 128];
        live(&mut m.untypeds, at.untyped).live += 2;
        m.place(
            at.untyped_cap,
            Place {
                space: inner,
                index: 1,
            },
        );
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
        live(&mut m.caps, root).dying = true;
    });
    assert_eq!(broken, "22: thread of a dying reply object");
}

/// Marks the first capability space as dying, with its sweep at `next`.
fn dying_space(m: &mut Model, at: At, next: usize) {
    let root = m.objects[&at.space].root;
    live(&mut m.caps, root).dying = true;
    let held = m.caps.keys().copied().collect();
    live(&mut m.cspaces, at.space).sweep = Some(Sweep { next, held });
}

#[test]
fn capability_that_a_dying_space_did_not_hold_when_it_began_to_die_is_reported() {
    let broken = spoiled(|m, at| {
        dying_space(m, at, 1);
        m.derive(
            at.copy,
            Rights::NONE,
            0,
            Place {
                space: at.space,
                index: 100,
            },
        );
    });
    assert_eq!(broken, "23: new capability in a dying space");
}

#[test]
fn dying_space_without_a_sweep_is_reported() {
    let broken = spoiled(|m, at| {
        let root = m.objects[&at.space].root;
        live(&mut m.caps, root).dying = true;
    });
    assert_eq!(broken, "27: sweep of a dying space");
}

#[test]
fn sweep_of_a_space_that_is_not_dying_is_reported() {
    let broken = spoiled(|m, at| {
        dying_space(m, at, 1);
        let root = m.objects[&at.space].root;
        live(&mut m.caps, root).dying = false;
    });
    assert_eq!(broken, "27: sweep of a dying space");
}

#[test]
fn full_slot_behind_the_sweep_is_reported() {
    let broken = spoiled(|m, at| dying_space(m, at, EP as usize + 1));
    assert_eq!(broken, "27: full slot behind the sweep");
}

#[test]
fn sweep_that_stands_at_slot_0_is_reported() {
    let broken = spoiled(|m, at| dying_space(m, at, 0));
    assert_eq!(broken, "27: index of the sweep");
}

#[test]
fn sweep_that_stands_beyond_the_table_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = inner_space(m, at, 40);
        let root = m.objects[&inner].root;
        live(&mut m.caps, root).dying = true;
        let sweep = Sweep {
            next: 129,
            held: Vec::new(),
        };
        live(&mut m.cspaces, inner).sweep = Some(sweep);
    });
    assert_eq!(broken, "27: index of the sweep");
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
        m.derive(
            at.untyped_cap,
            Rights::NONE,
            5,
            Place {
                space: at.space,
                index: 100,
            },
        );
    });
    assert_eq!(broken, "3: badge on another kind");
}

#[test]
fn start_up_capability_with_a_parent_is_reported() {
    let broken = spoiled(|m, at| {
        let debug = cap(m, START_SLOT_DEBUG);
        live(&mut m.caps, at.untyped_cap).parent = Some(debug);
        live(&mut m.caps, debug).children.push(at.untyped_cap);
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
        m.create(
            ObjectKind::Notify,
            0,
            Some(from),
            rights,
            Place {
                space: at.space,
                index: 100,
            },
        );
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
        live(&mut m.caps, root).dying = true;
    });
    assert_eq!(broken, "22: thread of a dying reply object");
}

#[test]
fn watermark_above_zero_while_nothing_is_live_is_reported() {
    let broken = spoiled(|m, _| {
        let device = object(m, START_SLOT_FIRST + 1);
        live(&mut m.untypeds, device).watermark = 8;
    });
    assert_eq!(broken, "18: watermark is zero when nothing is live");
}

#[test]
fn walk_of_a_removal_longer_than_192_capabilities_is_reported() {
    let broken = spoiled(|m, _| m.longest_walk = LONGEST_WALK + 1);
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

/// A capability space with one segment, made from the first untyped, whose
/// root capability, at depth 2, is in a slot of the first space.
fn inner_space(m: &mut Model, at: At, index: u32) -> ObjId {
    let root = m.create(
        ObjectKind::CSpace,
        128,
        Some(at.untyped_cap),
        Rights::ALL,
        Place {
            space: at.space,
            index,
        },
    );
    let inner = m.caps[&root].object;
    live(&mut m.cspaces, inner).slots = vec![Slot::Empty; 128];
    let untyped = live(&mut m.untypeds, at.untyped);
    untyped.live += 2;
    inner
}

#[test]
fn untyped_capability_less_deep_than_the_space_that_holds_it_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = inner_space(m, at, 40);
        let device = cap(m, START_SLOT_FIRST + 1);
        m.place(
            device,
            Place {
                space: inner,
                index: 1,
            },
        );
    });
    assert_eq!(broken, "28: depth of what a space holds");
}

#[test]
fn root_capability_of_a_space_held_in_a_space_of_the_same_depth_is_reported() {
    let broken = spoiled(|m, at| {
        let (first, second) = (inner_space(m, at, 40), inner_space(m, at, 41));
        let root = m.objects[&second].root;
        m.place(
            root,
            Place {
                space: first,
                index: 1,
            },
        );
    });
    assert_eq!(broken, "28: depth of what a space holds");
}

#[test]
fn root_capability_whose_exec_differs_from_its_untyped_capability_is_reported() {
    let broken = spoiled(|m, at| {
        let root = m.objects[&at.reply].root;
        live(&mut m.caps, root).rights = Rights::ALL.without(Rights::EXEC);
    });
    assert_eq!(broken, "3: exec of a root capability");
}

#[test]
fn state_of_another_kind_than_its_object_is_reported() {
    let broken = spoiled(|m, at| {
        m.replies.insert(at.endpoint, ReplyState::Idle);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn thread_bound_to_an_object_that_is_not_a_capability_space_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).cspace = Some(at.endpoint));
    assert_eq!(broken, "16: capability space of a thread");
}

#[test]
fn maximum_that_is_not_a_whole_number_of_segments_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).max = 200);
    assert_eq!(broken, "7: maximum");
}

#[test]
fn message_that_waits_with_another_thread_than_its_sender_is_reported() {
    let broken = spoiled(|m, at| {
        let caller = object(m, CALLER);
        if let Wait::Call { parcel, .. } = &mut live(&mut m.threads, caller).wait {
            parcel.sender = at.ready;
        }
    });
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn receive_range_of_more_than_4_slots_that_waits_is_reported() {
    let broken = spoiled(|m, _| {
        let caller = object(m, CALLER);
        if let Wait::Call { window, .. } = &mut live(&mut m.threads, caller).wait {
            (window.first, window.count) = (40, 5);
        }
    });
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn receive_range_beyond_the_table_that_waits_is_reported() {
    let broken = spoiled(|m, _| {
        let caller = object(m, CALLER);
        if let Wait::Call { window, .. } = &mut live(&mut m.threads, caller).wait {
            (window.first, window.count) = (127, 2);
        }
    });
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn unknown_flag_kept_by_an_armed_reply_object_is_reported() {
    let broken = spoiled(|m, at| {
        let caller = object(m, CALLER);
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.threads, caller).wait = Wait::Reply(at.reply);
        *live(&mut m.replies, at.reply) = ReplyState::Armed {
            caller,
            window: at.nowhere,
            flags: Header::FLAG_NOTIFICATION,
        };
    });
    assert_eq!(broken, "29: range or flags of a reply object");
}

// One test for each condition of an invariant that the tests above leave
// unguarded.

/// A child untyped of the first untyped, in a slot of the first space.
fn child_untyped(m: &mut Model, at: At, index: u32) -> ObjId {
    let place = Place {
        space: at.space,
        index,
    };
    let cap = m.create(
        ObjectKind::Untyped,
        4096,
        Some(at.untyped_cap),
        Rights::ALL,
        place,
    );
    live(&mut m.untypeds, at.untyped).live += 1;
    m.caps[&cap].object
}

#[test]
fn object_whose_root_is_not_a_child_of_its_untyped_is_reported() {
    let broken = spoiled(|m, at| {
        let other = child_untyped(m, at, 100);
        let place = Place {
            space: at.space,
            index: 101,
        };
        let later = m.create(
            ObjectKind::Notify,
            0,
            Some(at.untyped_cap),
            Rights::ALL,
            place,
        );
        let later = m.caps[&later].object;
        live(&mut m.objects, later).from = Some(other);
    });
    assert_eq!(broken, "11: root capability is a child of the untyped's");
}

#[test]
fn child_listed_twice_by_its_parent_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.caps, at.endpoint_cap)
            .children
            .insert(0, at.copy)
    });
    assert_eq!(broken, "1: parent lists its child once");
}

#[test]
fn maximum_of_no_slot_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = inner_space(m, at, 40);
        let space = live(&mut m.cspaces, inner);
        (space.max, space.slots) = (0, Vec::new());
        live(&mut m.untypeds, at.untyped).live -= 1;
    });
    assert_eq!(broken, "7: maximum");
}

#[test]
fn maximum_above_2_to_the_20_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).max = (1 << 20) + 128);
    assert_eq!(broken, "7: maximum");
}

#[test]
fn start_up_space_without_all_its_slots_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).max = 256);
    assert_eq!(broken, "7: size of the start-up space");
}

#[test]
fn capability_space_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        let space = CSpace {
            max: 128,
            slots: Vec::new(),
            sweep: None,
        };
        m.cspaces.insert(ObjId(u32::MAX), space);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn untyped_without_its_state_is_reported() {
    let broken = spoiled(|m, _| {
        let device = object(m, START_SLOT_FIRST + 1);
        m.untypeds.remove(&device);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn capability_space_without_its_state_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = inner_space(m, at, 40);
        m.cspaces.remove(&inner);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn thread_without_its_state_is_reported() {
    let broken = spoiled(|m, _| {
        let caller = object(m, CALLER);
        m.threads.remove(&caller);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn endpoint_without_its_state_is_reported() {
    let broken = spoiled(|m, at| {
        m.endpoints.remove(&at.endpoint);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn notify_without_its_state_is_reported() {
    let broken = spoiled(|m, at| {
        m.notifies.remove(&at.notify);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn child_untyped_of_device_memory_that_is_not_device_memory_is_reported() {
    let broken = spoiled(|m, at| {
        let (device, from) = (
            object(m, START_SLOT_FIRST + 1),
            cap(m, START_SLOT_FIRST + 1),
        );
        let rights = Rights::ALL.without(Rights::EXEC);
        let child = m.create(
            ObjectKind::Untyped,
            4096,
            Some(from),
            rights,
            Place {
                space: at.space,
                index: 100,
            },
        );
        let child = m.caps[&child].object;
        live(&mut m.untypeds, child).device = false;
        live(&mut m.untypeds, device).live += 1;
    });
    assert_eq!(broken, "25: kind on device memory");
}

#[test]
fn object_whose_identity_was_not_given_out_is_reported() {
    let broken = spoiled(|m, _| {
        // The debug object, under an identity that is still to give.
        let debug = cap(m, START_SLOT_DEBUG);
        let old = m.caps[&debug].object;
        let object = m.objects.remove(&old).unwrap();
        let new = ObjId(m.next_id + 5);
        m.objects.insert(new, object);
        live(&mut m.caps, debug).object = new;
    });
    assert_eq!(broken, "26: identity");
}

#[test]
fn root_capability_older_than_its_object_is_reported() {
    let broken = spoiled(|m, at| {
        // The debug capability becomes the root capability of the reply
        // object, which is younger.
        without_root(m, at.reply);
        let debug = cap(m, START_SLOT_DEBUG);
        let old = m.caps[&debug].object;
        m.objects.remove(&old);
        live(&mut m.caps, debug).object = at.reply;
        live(&mut m.objects, at.reply).root = debug;
    });
    assert_eq!(broken, "26: root capability older than its object");
}

/// Changes the wait of the thread that waits in the queue of the endpoint.
fn with_caller(change: impl FnOnce(&mut Parcel, &mut Window)) -> &'static str {
    spoiled(|m, _| {
        let caller = object(m, CALLER);
        if let Wait::Call { parcel, window, .. } = &mut live(&mut m.threads, caller).wait {
            change(parcel, window);
        }
    })
}

#[test]
fn message_of_more_than_64_words_that_waits_is_reported() {
    let broken = with_caller(|parcel, _| parcel.words = vec![0; MAX_WORDS + 1]);
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn message_of_more_than_4_capabilities_that_waits_is_reported() {
    let listed = Transfer::Move(SlotAddr::own(EP));
    let broken = with_caller(|parcel, _| parcel.caps = vec![listed; MAX_CAPS + 1]);
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn receive_range_with_slot_0_that_waits_is_reported() {
    let broken = with_caller(|_, window| (window.first, window.count) = (0, 1));
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn receive_range_of_a_receiver_beyond_the_table_is_reported() {
    let broken = spoiled(|m, at| {
        let caller = object(m, CALLER);
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.threads, caller).wait = Wait::None;
        live(&mut m.notifies, at.notify).waiters.clear();
        live(&mut m.endpoints, at.endpoint)
            .queue
            .push_back(at.waiter);
        *live(&mut m.replies, at.reply) = ReplyState::Attached(at.waiter);
        live(&mut m.threads, at.waiter).wait = Wait::Recv {
            endpoint: at.endpoint,
            reply: at.reply,
            window: Window {
                first: 127,
                count: 2,
                ..at.nowhere
            },
        };
    });
    assert_eq!(broken, "29: message or range of a waiting thread");
}

#[test]
fn receive_range_kept_by_an_armed_reply_object_beyond_the_table_is_reported() {
    let broken = spoiled(|m, at| {
        let caller = object(m, CALLER);
        live(&mut m.endpoints, at.endpoint).queue.clear();
        live(&mut m.threads, caller).wait = Wait::Reply(at.reply);
        *live(&mut m.replies, at.reply) = ReplyState::Armed {
            caller,
            window: Window {
                first: 127,
                count: 2,
                ..at.nowhere
            },
            flags: 0,
        };
    });
    assert_eq!(broken, "29: range or flags of a reply object");
}

#[test]
fn thread_that_waits_while_bound_to_nothing_is_reported() {
    let broken = spoiled(|m, at| {
        let waiter = live(&mut m.threads, at.waiter);
        (waiter.suspended, waiter.cspace) = (true, None);
    });
    assert_eq!(broken, "16: bound to nothing");
}

#[test]
fn thread_with_an_outcome_while_bound_to_nothing_is_reported() {
    let broken = spoiled(|m, at| {
        let ready = live(&mut m.threads, at.ready);
        (ready.suspended, ready.cspace) = (true, None);
        ready.outcome = Some(Err(Error::Gone));
    });
    assert_eq!(broken, "16: bound to nothing");
}

/// Gives the first thread, which does not wait, the outcome of a wait.
fn with_outcome(outcome: Outcome) -> &'static str {
    spoiled(|m, at| live(&mut m.threads, at.ready).outcome = Some(outcome))
}

#[test]
fn outcome_of_a_wait_that_says_the_thread_waits_is_reported() {
    assert_eq!(with_outcome(Ok(Output::Waits)), "30: outcome of a wait");
}

#[test]
fn outcome_of_a_wait_with_another_error_than_gone_is_reported() {
    assert_eq!(with_outcome(Err(Error::Busy)), "30: outcome of a wait");
}

#[test]
fn outcome_of_a_wait_with_a_word_of_zero_is_reported() {
    assert_eq!(with_outcome(Ok(Output::Word(0))), "30: outcome of a wait");
}

#[test]
fn outcome_of_a_wait_whose_words_differ_from_its_header_is_reported() {
    let header = Header::new(0, 2, 0, 0).unwrap();
    let delivery = Delivery {
        received: Received {
            header,
            badge: 0,
            marks: dodos_abi::Marks::NONE,
            task: 0,
        },
        words: vec![1],
    };
    let broken = with_outcome(Ok(Output::Message(delivery)));
    assert_eq!(broken, "30: outcome of a wait");
}

#[test]
fn watermark_that_is_not_a_multiple_of_8_bytes_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.untypeds, at.untyped).watermark += 1);
    assert_eq!(broken, "18: watermark on 8 bytes");
}

#[test]
fn census_counts_what_the_state_holds() {
    let mut model = state();
    let boot = object(&model, START_SLOT_THREAD);
    let census = Census {
        // The untyped, the endpoint, a copy of its capability.
        deepest: 3,
        callers: 1,
        waiters: 1,
        ..Census::default()
    };
    assert_eq!(model.census(), census);
    let own = SlotAddr::own;
    let mut on = |call: Syscall| model.apply(boot, call);
    // The first thread receives the call and binds the notify.
    let served = Syscall::Recv {
        endpoint: own(EP),
        reply: own(RP),
        range: Range::NONE,
    };
    on(served).unwrap();
    let bound = Syscall::NotifyBind {
        notify: own(NT),
        endpoint: own(EP),
    };
    on(bound).unwrap();
    // A copy of the endpoint capability in slot 40, 33 copies of that copy
    // and 33 more copies of the endpoint capability.
    let copies = (41..74)
        .map(|slot| (40, slot))
        .chain((80..113).map(|slot| (EP, slot)));
    for (source, destination) in [(EP, 40)].into_iter().chain(copies) {
        let copy = Syscall::CapDerive {
            source: own(source),
            destination: own(destination),
            rights: Rights::ALL,
            badge: 0,
        };
        on(copy).unwrap();
    }
    assert_eq!(on(Syscall::CapDelete(own(40))), Err(Error::Again));
    let census = model.census();
    assert_eq!((census.callers, census.served, census.bound), (0, 1, 1));
    assert_eq!((census.deepest, census.dying_copies), (4, 1));
    assert_eq!(census.dying, []);
    // Deleting the endpoint capability itself makes the object dying.
    let deleted = model.apply(boot, Syscall::CapDelete(own(EP)));
    assert_eq!(deleted, Err(Error::Again));
    let census = model.census();
    assert_eq!(
        (census.dying, census.dying_copies),
        (vec![ObjectKind::Endpoint], 0)
    );
}

// The conditions that the fourth review found unguarded.

#[test]
fn child_that_does_not_exist_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.caps, at.later_copy)
            .children
            .push(CapId(u32::MAX))
    });
    assert_eq!(broken, "2: children name it");
}

#[test]
fn full_slot_whose_capability_does_not_exist_is_reported() {
    let broken =
        spoiled(|m, at| live(&mut m.cspaces, at.space).slots[100] = Slot::Full(CapId(u32::MAX)));
    assert_eq!(broken, "6: capability of a full slot");
}

#[test]
fn untyped_state_without_an_object_is_reported() {
    let broken = spoiled(|m, at| {
        let untyped = m.untypeds[&at.untyped].clone();
        m.untypeds.insert(
            ObjId(u32::MAX),
            Untyped {
                live: 0,
                watermark: 0,
                ..untyped
            },
        );
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn thread_state_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        let thread = Thread {
            suspended: true,
            ..Thread::default()
        };
        m.threads.insert(ObjId(u32::MAX), thread);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn endpoint_state_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        m.endpoints.insert(ObjId(u32::MAX), Endpoint::default());
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn notify_state_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        m.notifies.insert(ObjId(u32::MAX), Notify::default());
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn capability_space_state_of_an_object_of_another_kind_is_reported() {
    let broken = spoiled(|m, at| {
        let space = CSpace {
            max: 128,
            slots: Vec::new(),
            sweep: None,
        };
        m.cspaces.insert(at.endpoint, space);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn capability_whose_identity_was_not_given_out_is_reported() {
    // The most recent capability: every object is older.
    let broken = spoiled(|m, at| m.next_id = at.later_copy.0);
    assert_eq!(broken, "26: identity");
}
