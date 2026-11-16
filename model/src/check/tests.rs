//! Each invariant is reported when a state breaks it: `spoiled` takes a
//! valid state, breaks one thing, and returns what `check` reports.

use crate::*;
use dodos_abi::{
    FAULT_WORDS, FaultKind, Header, LONGEST_WALK, MAX_CAPS, MAX_WORDS, START_SLOT_CSPACE,
    START_SLOT_DEBUG, START_SLOT_THREAD, TREE_DEPTH,
};
use dodos_abi::{Owner, Ownership};

/// The untyped of one chunk of the floor of `Machine::example`, and the
/// block of its device range, which `state` takes.
const UNTYPED: u32 = 25;
const DEVICE: u32 = 64;
const EP: u32 = 40;
const RP: u32 = 41;
const NT: u32 = 42;
const WAITER: u32 = 43;
const COPY: u32 = 44;
const CALLER: u32 = 46;

/// An endpoint, a reply object and a notify, a thread that waits on the
/// notify, a thread in the queue of the endpoint, and two copies of the
/// endpoint capability in `COPY` and after.
fn state() -> Model {
    let (mut model, boot) = Model::boot(&Machine::example()).unwrap();
    let own = SlotAddr::own;
    let take = Syscall::MemoryTake {
        control: own(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 1,
        place: 0,
        size: 0,
        destination: own(DEVICE),
    };
    model.apply(boot, take).unwrap();
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
                untyped: own(UNTYPED),
                kind,
                size: 0,
                page: 0,
                memory: 0,
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
                    page: SlotAddr::NONE,
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
                rights: Rights::SEND.bits(),
                badge: 0,
            },
        );
    }
    let (waiter, caller) = (object(&model, WAITER), object(&model, CALLER));
    let mut on = |thread: ObjId, call: Syscall| model.apply(thread, call).unwrap();
    on(waiter, Syscall::Wait(own(NT), 0));
    on(
        caller,
        Syscall::Call {
            endpoint: own(EP),
            message: Message::default(),
            range: Range::NONE,
            wait: 0,
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
        untyped: object(model, UNTYPED),
        endpoint: object(model, EP),
        reply: object(model, RP),
        notify: object(model, NT),
        ready: object(model, START_SLOT_THREAD),
        waiter: object(model, WAITER),
        untyped_cap: cap(model, UNTYPED),
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

/// A waiting thread with an outcome, waiting on a fault of `kind` whose
/// word 0 is its instruction pointer plus `off`.
fn outcome_on_a_fault(kind: FaultKind, off: u64) -> &'static str {
    spoiled(|m, at| {
        let waiter = live(&mut m.threads, at.waiter);
        let rip = waiter.regs[dodos_abi::x86_64::REG_RIP];
        waiter.outcome = Some(Ok(Output::Done));
        waiter.faulting = true;
        waiter.raised = Some((kind as u32, rip + off));
    })
}

#[test]
fn outcome_while_waiting_on_a_fault_is_reported_unless_of_kind_7_or_8_at_the_address_after_the_call()
 {
    assert_eq!(
        outcome_on_a_fault(FaultKind::Page, 0),
        "16: outcome while waiting"
    );
    assert_eq!(
        outcome_on_a_fault(FaultKind::NoContext, 4),
        "16: outcome while waiting"
    );
    // Kinds 7 and 8 at the address after the call are allowed by 16; the
    // state then fails later invariants of the faulting wait, not 16.
    for kind in [FaultKind::NoAddressSpace, FaultKind::NoContext] {
        assert_ne!(outcome_on_a_fault(kind, 0), "16: outcome while waiting");
    }
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
fn thread_bound_to_a_space_that_does_not_list_it_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).threads.clear());
    assert_eq!(broken, "32: capability space of a thread");
}

#[test]
fn space_that_lists_a_thread_bound_elsewhere_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).cspace = None);
    assert_eq!(broken, "32: thread listed by a capability space");
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
                index: 200 + index,
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
                index: 60,
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
        let inner = inner_space(m, at, 60);
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
    let broken = spoiled(|m, _| {
        let (debug, space) = (cap(m, START_SLOT_DEBUG), cap(m, START_SLOT_CSPACE));
        live(&mut m.caps, debug).parent = Some(space);
        live(&mut m.caps, space).children.push(debug);
    });
    assert_eq!(broken, "11: parent of a start-up capability");
}

#[test]
fn exec_on_device_memory_is_reported() {
    let broken = spoiled(|m, _| {
        let device = cap(m, DEVICE);
        live(&mut m.caps, device).rights = Rights::ALL;
    });
    assert_eq!(broken, "25: exec on device memory");
}

#[test]
fn kernel_object_on_device_memory_is_reported() {
    let broken = spoiled(|m, at| {
        let device = object(m, DEVICE);
        let from = cap(m, DEVICE);
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
        let device = object(m, DEVICE);
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
        let inner = inner_space(m, at, 60);
        let untyped = cap(m, 24);
        m.place(
            untyped,
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
        let (first, second) = (inner_space(m, at, 60), inner_space(m, at, 61));
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
    assert_eq!(broken, "32: capability space of a thread");
}

#[test]
fn maximum_that_is_not_a_whole_number_of_segments_is_reported() {
    let broken = spoiled(|m, at| {
        let space = live(&mut m.cspaces, at.space);
        space.max = space.slots.len() as u32 + 72;
    });
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
            (window.first, window.count) = (4223, 2);
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
        let inner = inner_space(m, at, 60);
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
    let broken = spoiled(|m, at| {
        let space = live(&mut m.cspaces, at.space);
        space.max = space.slots.len() as u32 + 128;
    });
    assert_eq!(broken, "7: size of the start-up space");
}

#[test]
fn capability_space_without_an_object_is_reported() {
    let broken = spoiled(|m, _| {
        let space = CSpace {
            max: 128,
            slots: Vec::new(),
            sweep: None,
            threads: Vec::new(),
        };
        m.cspaces.insert(ObjId(u32::MAX), space);
    });
    assert_eq!(broken, "10: state without an object");
}

#[test]
fn untyped_without_its_state_is_reported() {
    let broken = spoiled(|m, _| {
        let untyped = object(m, UNTYPED);
        m.untypeds.remove(&untyped);
    });
    assert_eq!(broken, "10: state of an object");
}

#[test]
fn capability_space_without_its_state_is_reported() {
    let broken = spoiled(|m, at| {
        let inner = inner_space(m, at, 60);
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
        let (device, from) = (object(m, DEVICE), cap(m, DEVICE));
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
                first: 4223,
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
                first: 4223,
                count: 2,
                ..at.nowhere
            },
            flags: 0,
        };
    });
    assert_eq!(broken, "29: range or flags of a reply object");
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
fn outcome_of_a_wait_with_another_error_than_gone_or_cancelled_is_reported() {
    assert_eq!(with_outcome(Err(Error::Busy)), "30: outcome of a wait");
    assert_eq!(
        with_outcome(Err(Error::Unsupported)),
        "30: outcome of a wait"
    );
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
fn wait_number_of_a_thread_that_does_not_wait_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).number = 1);
    assert_eq!(broken, "31: wait number of a thread");
}

#[test]
fn wait_number_already_cancelled_while_the_thread_waits_is_reported() {
    let broken = spoiled(|m, at| {
        let waiter = live(&mut m.threads, at.waiter);
        (waiter.number, waiter.cancelled_up_to) = (3, 3);
    });
    assert_eq!(broken, "31: wait number of a thread");
    // A greater number is the state of a thread that waits with it.
    let mut model = state();
    let at = places(&model);
    let waiter = live(&mut model.threads, at.waiter);
    (waiter.number, waiter.cancelled_up_to) = (4, 3);
    assert_eq!(model.check(), Ok(()));
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
        // Twenty at start-up, five objects, two copies.
        capabilities: 57,
        // The untyped, the endpoint, a copy of its capability.
        deepest: 3,
        callers: 1,
        waiters: 1,
        // Two segments, the start-up page, the message page.
        mappings: 4,
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
        wait: 0,
    };
    on(served).unwrap();
    let bound = Syscall::NotifyBind {
        notify: own(NT),
        endpoint: own(EP),
    };
    on(bound).unwrap();
    // A copy of the endpoint capability in slot 60, 33 copies of that copy
    // and 33 more copies of the endpoint capability.
    let copies = (161..194)
        .map(|slot| (60, slot))
        .chain((200..233).map(|slot| (EP, slot)));
    for (source, destination) in [(EP, 60)].into_iter().chain(copies) {
        let copy = Syscall::CapDerive {
            source: own(source),
            destination: own(destination),
            rights: Rights::ALL.bits(),
            badge: 0,
        };
        on(copy).unwrap();
    }
    assert_eq!(on(Syscall::CapDelete(own(60))), Err(Error::Again));
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
            threads: Vec::new(),
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

// The invariants of the D-110 changes.

#[test]
fn thread_listed_twice_by_its_space_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.cspaces, at.space).threads.push(at.ready));
    assert_eq!(broken, "32: capability space of a thread");
}

#[test]
fn address_space_that_does_not_list_its_thread_is_reported() {
    let broken = spoiled(|m, at| {
        let aspace = live(&mut m.threads, at.ready).aspace.unwrap();
        live(&mut m.aspaces, aspace).threads.clear();
    });
    assert_eq!(broken, "32: address space of a thread");
}

#[test]
fn fault_endpoint_that_lists_a_thread_bound_elsewhere_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.endpoints, at.endpoint).threads.push(at.ready));
    assert_eq!(broken, "32: thread listed by a fault endpoint");
}

#[test]
fn message_page_whose_record_names_another_thread_is_reported() {
    let broken = spoiled(|m, at| {
        let page = m.threads[&at.ready].page.unwrap();
        live(&mut m.caps, page).record = Record::Binding(at.waiter);
    });
    assert_eq!(broken, "33: message page of a thread");
}

#[test]
fn record_that_names_a_thread_without_that_page_is_reported() {
    let broken = spoiled(|m, at| {
        let page = live(&mut m.threads, at.ready).page.take().unwrap();
        live(&mut m.caps, page).record = Record::Binding(at.waiter);
    });
    assert_eq!(broken, "33: record of a message page");
}

#[test]
fn message_page_of_two_pages_is_reported() {
    let broken = spoiled(|m, at| {
        let page = m.threads[&at.ready].page.unwrap();
        live(&mut m.frames, m.caps[&page].object).pages = 2;
    });
    assert_eq!(broken, "33: message page of a thread");
}

#[test]
fn frame_of_more_than_e_pages_or_of_the_wrong_memory_type_is_reported() {
    let page = |m: &Model, at: At| m.caps[&m.threads[&at.ready].page.unwrap()].object;
    let broken = spoiled(|m, at| {
        let frame = page(m, at);
        live(&mut m.frames, frame).pages = u64::from(m.machine.geometry.entries) + 1;
    });
    assert_eq!(broken, "34: frame");
    let broken = spoiled(|m, at| {
        let frame = page(m, at);
        live(&mut m.frames, frame).memory = MemoryType::Uncached;
    });
    assert_eq!(broken, "34: frame");
}

#[test]
fn record_on_a_capability_that_is_not_for_a_frame_is_reported() {
    let broken =
        spoiled(|m, at| live(&mut m.caps, at.endpoint_cap).record = Record::Binding(at.ready));
    assert_eq!(broken, "34: record of another kind");
}

#[test]
fn clean_mark_below_the_watermark_or_on_device_memory_is_reported() {
    let broken = spoiled(|m, at| {
        let untyped = live(&mut m.untypeds, at.untyped);
        untyped.clean = untyped.watermark - 8;
    });
    assert_eq!(broken, "18: clean mark");
    let broken = spoiled(|m, at| {
        let untyped = live(&mut m.untypeds, at.untyped);
        untyped.clean = untyped.bytes + 8;
    });
    assert_eq!(broken, "18: clean mark");
    let broken = spoiled(|m, _| {
        let device = m.untypeds.values_mut().find(|u| u.device).unwrap();
        device.clean = 8;
    });
    assert_eq!(broken, "18: clean mark");
}

// The invariants of mapping records and address spaces.

/// The address space of the first thread.
fn start_aspace(m: &Model, at: At) -> ObjId {
    m.threads[&at.ready].aspace.unwrap()
}

#[test]
fn translation_that_no_record_made_is_reported() {
    let broken = spoiled(|m, at| {
        let space = start_aspace(m, at);
        let aspace = live(&mut m.aspaces, space);
        let (&at, &t) = aspace.translations.iter().next().unwrap();
        aspace.translations.insert(at + (1 << 30), t);
    });
    assert_eq!(broken, "35: translations of the records");
}

#[test]
fn record_whose_translation_is_missing_is_reported() {
    let broken = spoiled(|m, at| {
        let space = start_aspace(m, at);
        let aspace = live(&mut m.aspaces, space);
        let first = *aspace.translations.keys().next().unwrap();
        aspace.translations.remove(&first);
    });
    assert_eq!(broken, "35: translations of the records");
}

#[test]
fn translation_with_other_rights_than_its_record_is_reported() {
    let broken = spoiled(|m, at| {
        let space = start_aspace(m, at);
        let aspace = live(&mut m.aspaces, space);
        let t = aspace.translations.values_mut().next().unwrap();
        t.rights = Rights::ALL;
    });
    assert_eq!(broken, "35: translations of the records");
}

#[test]
fn record_beyond_its_frame_is_reported() {
    let broken = spoiled(|m, at| {
        let cap = m.aspaces[&start_aspace(m, at)].records[0];
        if let Record::Mapping(mapping) = &mut live(&mut m.caps, cap).record {
            mapping.first = mapping.count;
        }
    });
    assert_eq!(broken, "35: pages of a record");
}

#[test]
fn record_missing_from_the_list_of_its_space_is_reported() {
    let broken = spoiled(|m, at| {
        let space = start_aspace(m, at);
        live(&mut m.aspaces, space).records.pop();
    });
    assert_eq!(broken, "36: list of the records of a space");
}

#[test]
fn space_with_fewer_tables_than_its_translations_hook_is_reported() {
    let broken = spoiled(|m, at| {
        // One table fewer than the translations hook.
        let space = start_aspace(m, at);
        let geometry = m.machine.geometry;
        let hooked = crate::memory::hooked(&m.aspaces[&space], geometry.levels, geometry);
        live(&mut m.aspaces, space).tables = hooked - 1;
    });
    assert_eq!(broken, "37: tables of a space");
}

#[test]
fn space_with_as_many_tables_as_its_translations_hook_is_accepted() {
    let mut model = state();
    let at = places(&model);
    let space = start_aspace(&model, at);
    let geometry = model.machine.geometry;
    let hooked = crate::memory::hooked(&model.aspaces[&space], geometry.levels, geometry);
    live(&mut model.aspaces, space).tables = hooked;
    assert_eq!(model.check(), Ok(()));
}

#[test]
fn mapping_without_read_in_an_aspace_is_reported() {
    let broken = spoiled(|m, at| {
        let space = start_aspace(m, at);
        let cap = m.aspaces[&space].records[0];
        if let Record::Mapping(mapping) = &mut live(&mut m.caps, cap).record {
            mapping.rights = Rights::WRITE;
        }
        for t in live(&mut m.aspaces, space).translations.values_mut() {
            if t.cap == cap {
                t.rights = Rights::WRITE;
            }
        }
    });
    assert_eq!(broken, "38: rights of a mapping");
}

/// An iospace made from the untyped, with device 0x10 of the IOMMU unit
/// attached.
fn with_device(m: &mut Model) -> (ObjId, ObjId) {
    let boot = object(m, START_SLOT_THREAD);
    let own = SlotAddr::own;
    let retype = Syscall::UntypedRetype {
        untyped: own(UNTYPED),
        kind: ObjectKind::IoSpace,
        size: 0,
        page: 0,
        memory: 0,
        destination: own(60),
    };
    // The clearing of the space may take several calls (one 64 KB step
    // each): a space of two pages of 32 KB does.
    while m.apply(boot, retype.clone()) == Err(Error::Again) {}
    let attach = Syscall::IoSpaceAttach {
        iommu: own(18),
        iospace: own(60),
        device: 0x10,
    };
    m.apply(boot, attach).unwrap();
    (object(m, 60), object(m, 18))
}

#[test]
fn device_listed_by_an_iospace_but_not_attached_is_reported() {
    let broken = spoiled(|m, _| {
        let (_, iommu) = with_device(m);
        live(&mut m.iommus, iommu).attached.clear();
    });
    assert_eq!(broken, "39: device of an iospace");
}

#[test]
fn device_attached_but_not_listed_by_its_iospace_is_reported() {
    let broken = spoiled(|m, _| {
        let (iospace, _) = with_device(m);
        live(&mut m.iospaces, iospace).clear();
    });
    assert_eq!(broken, "39: device of a unit");
}

#[test]
fn device_the_kernel_uses_attached_to_an_iospace_is_reported() {
    let broken = spoiled(|m, _| {
        let (iospace, iommu) = with_device(m);
        live(&mut m.iospaces, iospace).push((iommu, 0xf8));
        live(&mut m.iommus, iommu).attached.insert(0xf8, iospace);
    });
    assert_eq!(broken, "39: devices of an iospace");
}

#[test]
fn unit_with_more_domains_than_it_has_is_reported() {
    let broken = spoiled(|m, _| {
        with_device(m);
        m.machine.iommus[0].domains = 0;
    });
    assert_eq!(broken, "39: domain numbers of a unit");
}

#[test]
fn thread_that_waits_on_a_fault_but_not_as_a_caller_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.waiter).faulting = true);
    assert_eq!(broken, "40: fault wait");
}

#[test]
fn fault_message_with_capabilities_is_reported() {
    let broken = spoiled(|m, _| {
        let caller = object(m, CALLER);
        let waiting = live(&mut m.threads, caller);
        waiting.faulting = true;
        if let Wait::Call { parcel, .. } = &mut waiting.wait {
            (parcel.label, parcel.words) = (1, vec![0; FAULT_WORDS]);
            parcel.caps = vec![Transfer::Move(SlotAddr::own(EP))];
        }
    });
    assert_eq!(broken, "40: fault wait");
}

#[test]
fn fault_message_that_waits_is_accepted() {
    let mut model = state();
    let caller = object(&model, CALLER);
    let waiting = live(&mut model.threads, caller);
    waiting.faulting = true;
    if let Wait::Call { parcel, .. } = &mut waiting.wait {
        (parcel.label, parcel.words) = (8, vec![0; FAULT_WORDS]);
    }
    assert_eq!(model.check(), Ok(()));
}

// The invariants of scheduling and time.

/// The context of the first thread.
fn first_context(m: &Model, at: At) -> ObjId {
    m.threads[&at.ready].home.unwrap()
}

#[test]
fn context_out_of_its_queue_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        live(&mut m.scheds, context).queue = None;
    });
    assert_eq!(broken, "41: queue of a context");
}

#[test]
fn context_in_the_queue_of_another_key_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        if let Some(queued) = &mut live(&mut m.scheds, context).queue {
            queued.key += 1;
        }
    });
    assert_eq!(broken, "41: queue of a context");
}

#[test]
fn admitted_sum_that_is_not_the_sum_of_the_bandwidths_is_reported() {
    let broken = spoiled(|m, _| m.admitted -= 1);
    assert_eq!(broken, "42: admitted sum");
}

#[test]
fn context_with_a_budget_below_10_microseconds_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        live(&mut m.scheds, context).budget = 9_999;
    });
    assert_eq!(broken, "43: parameters and state of a context");
}

#[test]
fn throttled_context_with_budget_left_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        live(&mut m.scheds, context).throttled = Some((m.now + 1, 1));
    });
    // Throttled with its holder runnable: it belongs in a slack queue.
    assert_eq!(broken, "41: queue of a context");
}

#[test]
fn home_that_its_context_does_not_name_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        live(&mut m.scheds, context).home = None;
    });
    assert_eq!(broken, "41: queue of a context");
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        live(&mut m.threads, at.ready).home = None;
        let _ = context;
    });
    assert_eq!(broken, "44: home of a context");
}

#[test]
fn lend_whose_reply_object_is_not_in_its_chain_is_reported() {
    let broken = spoiled(|m, at| {
        let context = first_context(m, at);
        m.lends.insert(at.reply, (context, at.waiter));
    });
    assert_eq!(broken, "44: lend");
}

#[test]
fn marked_thread_that_runs_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).lost = true);
    assert_eq!(broken, "45: thread that lost its context");
}

#[test]
fn running_context_that_is_not_the_choice_is_reported() {
    let broken = spoiled(|m, _| m.running = None);
    assert_eq!(broken, "46: running context");
}

#[test]
fn armed_timer_its_notify_does_not_list_is_reported() {
    let broken = spoiled(|m, at| {
        let boot = at.ready;
        let own = SlotAddr::own;
        let retype = Syscall::UntypedRetype {
            untyped: own(UNTYPED),
            kind: ObjectKind::Timer,
            size: 0,
            page: 0,
            memory: 0,
            destination: own(60),
        };
        m.apply(boot, retype).unwrap();
        let set = Syscall::TimerSet {
            timer: own(60),
            notify: own(NT),
            deadline: 1_000_000,
            tolerance: 0,
        };
        m.apply(boot, set).unwrap();
        live(&mut m.notifies, at.notify).timers.clear();
    });
    assert_eq!(broken, "47: armed timer");
}

// The invariants of tasks and marks, and of interrupts.

#[test]
fn thread_whose_task_does_not_list_it_is_reported() {
    let broken = spoiled(|m, at| {
        let task = m.threads[&at.ready].task.unwrap();
        live(&mut m.tasks, task).threads.clear();
    });
    assert_eq!(broken, "48: task of a thread");
}

#[test]
fn task_that_lists_a_thread_attached_elsewhere_is_reported() {
    let broken = spoiled(|m, at| {
        let task = m.threads[&at.ready].task.unwrap();
        live(&mut m.tasks, task).threads.push(at.waiter);
    });
    assert_eq!(broken, "48: threads of a task");
}

#[test]
fn task_identifier_not_given_out_or_with_a_reserved_mark_is_reported() {
    let broken = spoiled(|m, at| {
        let task = m.threads[&at.ready].task.unwrap();
        live(&mut m.tasks, task).id = dodos_abi::TaskId::new(0, 99);
    });
    assert_eq!(broken, "49: task identifier");
    let broken = spoiled(|m, at| {
        let task = m.threads[&at.ready].task.unwrap();
        live(&mut m.tasks, task).marks = 1 << 5;
    });
    assert_eq!(broken, "49: marks of a task");
    let broken = spoiled(|m, at| live(&mut m.threads, at.waiter).marks = 1 << 5);
    assert_eq!(broken, "49: marks of a thread");
}

#[test]
fn running_thread_of_a_revoked_task_is_reported() {
    let broken = spoiled(|m, at| {
        let task = m.threads[&at.ready].task.unwrap();
        live(&mut m.tasks, task).revoked = true;
    });
    assert_eq!(broken, "50: running thread of a revoked task");
}

/// An irq claimed on vector 48 into slot 61, bound to the notify.
fn with_irq(m: &mut Model) -> ObjId {
    let boot = object(m, START_SLOT_THREAD);
    let own = SlotAddr::own;
    let claim = Syscall::IrqClaim {
        control: own(dodos_abi::START_SLOT_IRQ_CONTROL),
        source: dodos_abi::IrqSource::msi(0, 1, 0, 0).raw(),
        vector: 48,
        destination: own(61),
    };
    m.apply(boot, claim).unwrap();
    let bind = Syscall::IrqBind {
        irq: own(61),
        notify: own(NT),
    };
    m.apply(boot, bind).unwrap();
    object(m, 61)
}

#[test]
fn irq_of_a_vector_that_cannot_be_claimed_is_reported() {
    let broken = spoiled(|m, _| {
        let irq = with_irq(m);
        live(&mut m.irqs, irq).vector = 47;
    });
    assert_eq!(broken, "51: vector of an irq");
}

#[test]
fn bound_irq_that_its_notify_does_not_list_is_reported() {
    let broken = spoiled(|m, at| {
        with_irq(m);
        live(&mut m.notifies, at.notify).irqs.clear();
    });
    assert_eq!(broken, "51: binding of an irq");
}

#[test]
fn idle_irq_bound_to_a_notify_is_reported() {
    let broken = spoiled(|m, _| {
        let irq = with_irq(m);
        live(&mut m.irqs, irq).state = IrqState::Idle;
    });
    assert_eq!(broken, "51: binding of an irq");
}

#[test]
fn irq_whose_root_capability_is_not_a_child_of_an_irq_control_is_reported() {
    let broken = spoiled(|m, _| {
        let irq = with_irq(m);
        let root = m.objects[&irq].root;
        orphan(m, root);
        live(&mut m.caps, root).parent = None;
    });
    assert_eq!(broken, "11: parent of an irq");
}

// Memory between nodes and the gate over page tables.

/// A block of one chunk of ordinary memory in slot 300, taken by the first
/// thread: the first chunk past the floors and the reserves.
fn with_block(m: &mut Model) -> (ObjId, u64) {
    let thread = object(m, START_SLOT_THREAD);
    let geometry = m.machine.geometry;
    let place = m.machine.memory[4].base + 8 * geometry.chunk();
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place,
        size: u64::from(geometry.chunk_shift()),
        destination: SlotAddr::own(300),
    };
    assert_eq!(m.apply(thread, take), Ok(Output::Done));
    (object(m, 300), place >> geometry.chunk_shift())
}

fn spoiled_with_block(spoil: impl FnOnce(&mut Model, ObjId, u64)) -> &'static str {
    let mut model = state();
    let (block, chunk) = with_block(&mut model);
    assert_eq!(model.check(), Ok(()));
    spoil(&mut model, block, chunk);
    model.check().expect_err("the state is spoiled")
}

#[test]
fn take_of_a_word_the_rules_of_taking_refuse_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        let before = Ownership::free(Owner::Node(1)).raw();
        let after = Ownership::taken(Owner::Node(0), 1).raw();
        let write = m.table.journal(0, crate::table::Word::Chunk(chunk), before, after);
        m.table.writes.push(write);
    });
    assert_eq!(broken, "52: write of an ownership word");
}

/// Argon b1 of the seventh review: the journal of writes is kept until the
/// next check of the invariants, so a write that is wrong inside an event
/// is reported even when the check comes after later events.
#[test]
fn a_wrong_write_is_reported_by_a_check_made_after_later_events() {
    let broken = spoiled_with_block(|m, _, chunk| {
        // An event starts the journal (the check before it judged the old
        // one); the wrong write is made in it; two more events follow.
        let thread = *m.threads.keys().next().expect("a thread");
        let _ = m.apply(thread, Syscall::Yield);
        let before = Ownership::free(Owner::Node(1)).raw();
        let after = Ownership::taken(Owner::Node(0), 1).raw();
        let write = m.table.journal(0, crate::table::Word::Chunk(chunk), before, after);
        m.table.writes.push(write);
        for _ in 0..2 {
            let _ = m.apply(thread, Syscall::Yield);
        }
    });
    assert_eq!(broken, "52: write of an ownership word");
}

#[test]
fn release_of_a_word_the_writer_did_not_hold_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        let before = Ownership::taken(Owner::Node(1), 1).raw();
        let after = Ownership::free(Owner::Node(0)).raw();
        let write = m.table.journal(0, crate::table::Word::Chunk(chunk), before, after);
        m.table.writes.push(write);
    });
    assert_eq!(broken, "52: write of an ownership word");
}

#[test]
fn two_blocks_on_one_chunk_are_reported() {
    let broken = spoiled_with_block(|m, block, _| {
        let device = object(m, DEVICE);
        let (base, bytes) = (m.untypeds[&block].base, m.untypeds[&block].bytes);
        let other = live(&mut m.untypeds, device);
        (other.base, other.bytes, other.block) = (base, bytes, Some(crate::Block::Run));
    });
    assert_eq!(broken, "53: two blocks on a chunk");
}

#[test]
fn block_on_a_word_not_taken_by_its_node_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        std::sync::Arc::make_mut(&mut m.table.owners)[chunk as usize] =
            Ownership::taken(Owner::Node(0), 7).raw();
    });
    assert_eq!(broken, "54: block on a word not taken");
}

#[test]
fn start_up_untyped_outside_the_floor_is_reported() {
    let broken = spoiled(|m, _| {
        let untyped = object(m, UNTYPED);
        live(&mut m.untypeds, untyped).base = 0;
    });
    assert_eq!(broken, "54: start-up untyped outside the floor");
}

#[test]
fn block_on_a_free_word_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        std::sync::Arc::make_mut(&mut m.table.owners)[chunk as usize] =
            Ownership::free(Owner::Node(0)).raw();
    });
    assert_eq!(broken, "55: block on a free word");
}

#[test]
fn translation_to_a_free_word_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        // The message page of the first thread now lies on a free chunk.
        let start = object(m, dodos_abi::START_SLOT_MESSAGE_PAGE);
        let shift = m.machine.geometry.chunk_shift();
        live(&mut m.frames, start).base = (chunk + 1) << shift;
    });
    assert_eq!(broken, "55: translation to a free word");
}

#[test]
fn word_changed_from_none_is_reported() {
    let broken = spoiled(|m, _| {
        // A chunk of the floor of node 1.
        let chunk = (m.machine.memory[4].base >> m.machine.geometry.chunk_shift()) + 3;
        std::sync::Arc::make_mut(&mut m.table.owners)[chunk as usize] =
            Ownership::taken(Owner::Node(1), 1).raw();
    });
    assert_eq!(broken, "56: a word changed from or to none");
}

#[test]
fn kernel_space_that_maps_a_chunk_the_node_does_not_hold_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        m.kernel_runs.insert(chunk + 1);
    });
    assert_eq!(broken, "57: kernel space and ownership");
    let broken = spoiled_with_block(|m, _, chunk| {
        m.kernel_runs.remove(&chunk);
    });
    assert_eq!(broken, "57: kernel space and ownership");
}

#[test]
fn translation_to_memory_of_another_node_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        std::sync::Arc::make_mut(&mut m.table.owners)[chunk as usize + 1] =
            Ownership::taken(Owner::Node(1), 1).raw();
        let start = object(m, dodos_abi::START_SLOT_MESSAGE_PAGE);
        let shift = m.machine.geometry.chunk_shift();
        live(&mut m.frames, start).base = (chunk + 1) << shift;
    });
    assert_eq!(broken, "58: translation to memory the node does not own");
}

#[test]
fn writable_translation_to_the_kept_image_is_reported() {
    let broken = spoiled(|m, _| {
        // The kept frame of the executable segment, mapped read and write.
        let cap = cap(m, 27);
        let rw = Rights::READ.union(Rights::WRITE);
        let Record::Mapping(mut mapping) = m.caps[&cap].record else {
            panic!("mapped");
        };
        mapping.rights = rw;
        live(&mut m.caps, cap).record = Record::Mapping(mapping);
        let space = live(&mut m.aspaces, mapping.space);
        for t in space.translations.values_mut().filter(|t| t.cap == cap) {
            t.rights = rw;
        }
    });
    assert_eq!(broken, "58: writable translation to read-only memory");
}

#[test]
fn translation_to_a_table_page_is_reported() {
    let broken = spoiled(|m, _| {
        let start = object(m, dodos_abi::START_SLOT_MESSAGE_PAGE);
        let page = m.frames[&start].base / m.machine.geometry.page();
        std::sync::Arc::make_mut(&mut m.table.classes).insert(page, crate::table::PageClass::Table);
    });
    assert_eq!(broken, "58: translation to a table page");
}

#[test]
fn map_count_that_is_not_the_number_of_leaves_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        std::sync::Arc::make_mut(&mut m.table.maps)[chunk as usize] += 1
    });
    assert_eq!(broken, "58: map count");
}

#[test]
fn page_class_of_no_table_is_reported() {
    let broken = spoiled_with_block(|m, _, chunk| {
        let pages = m.machine.geometry.chunk() / m.machine.geometry.page();
        std::sync::Arc::make_mut(&mut m.table.classes)
            .insert(chunk * pages, crate::table::PageClass::Table);
    });
    assert_eq!(broken, "59: page classes");
}

// The settled problems of pass 11.

#[test]
fn context_that_entered_a_queue_off_the_change_list_is_reported() {
    let broken = spoiled(|m, _| {
        m.event.entered_from = 0;
        m.event.changes.clear();
    });
    assert_eq!(broken, "60: order of entry and the change list");
}

#[test]
fn parameters_in_force_changed_without_a_replenishment_are_reported() {
    let broken = spoiled(|m, _| {
        // The same bandwidth, so that the admitted sum still holds.
        let first = object(m, dodos_abi::START_SLOT_SCHED);
        let sched = live(&mut m.scheds, first);
        (sched.budget, sched.period) = (2 * sched.budget, 2 * sched.period);
    });
    assert_eq!(broken, "61: parameters in force changed");
}

#[test]
fn call_that_put_five_contexts_in_new_places_is_reported() {
    let broken = spoiled(|m, _| *m.last_event = Some((crate::sched::EventKind::Call, 5)));
    assert_eq!(broken, "62: contexts put in a new queue position");
    let broken = spoiled(|m, _| *m.last_event = Some((crate::sched::EventKind::TaskRevoke, 33)));
    assert_eq!(broken, "62: contexts put in a new queue position");
    let broken = spoiled(|m, _| *m.last_event = Some((crate::sched::EventKind::Removal, 97)));
    assert_eq!(broken, "62: contexts put in a new queue position");
}

#[test]
fn removal_step_that_put_96_contexts_in_new_places_is_accepted() {
    let mut model = state();
    *model.last_event = Some((crate::sched::EventKind::Removal, 96));
    assert_eq!(model.check(), Ok(()));
}

#[test]
fn recorded_fault_of_a_thread_that_is_not_suspended_is_reported() {
    let broken = spoiled(|m, at| live(&mut m.threads, at.ready).recorded = Some((1, [0; 5])));
    assert_eq!(
        broken,
        "63: recorded fault of a thread that is not suspended"
    );
}

#[test]
fn fault_no_context_delivered_to_a_passive_receiver_is_reported() {
    let broken = spoiled(|m, _| m.event.no_context_delivered.push(false));
    assert_eq!(
        broken,
        "64: fault no context delivered to a passive receiver"
    );
}

#[test]
fn second_fault_of_a_thread_in_one_event_is_reported() {
    let broken = spoiled(|m, at| {
        m.event.raised.push((at.ready, 8));
        m.event.raised.push((at.ready, 2));
    });
    assert_eq!(broken, "64: a second fault in one event");
}

#[test]
fn clean_mark_over_memory_that_was_not_cleared_is_reported() {
    let broken = spoiled(|m, _| {
        let id = object(m, UNTYPED);
        let untyped = live(&mut m.untypeds, id);
        untyped.clean = untyped.watermark + 64;
        untyped.zero.clear();
    });
    assert_eq!(broken, "65: clean mark over memory not cleared");
}

#[test]
fn device_of_a_unit_missing_from_its_list_is_reported() {
    let broken = spoiled(|m, at| {
        let thread = object(m, START_SLOT_THREAD);
        let _ = at;
        let space = Syscall::UntypedRetype {
            untyped: SlotAddr::own(UNTYPED),
            kind: ObjectKind::IoSpace,
            size: 0,
            page: 0,
            memory: 0,
            destination: SlotAddr::own(300),
        };
        while m.apply(thread, space.clone()) == Err(Error::Again) {}
        let attach = Syscall::IoSpaceAttach {
            iommu: SlotAddr::own(18),
            iospace: SlotAddr::own(300),
            device: 0x10,
        };
        m.apply(thread, attach).unwrap();
        let unit = object(m, 18);
        live(&mut m.iommus, unit).order.clear();
    });
    assert_eq!(broken, "39: list of the devices of a unit");
}

// Unlike cores.

#[test]
fn enabled_features_other_than_the_running_threads_are_reported() {
    let broken = spoiled(|m, _| m.enabled ^= 1 << dodos_abi::x86_64::AMX);
    assert_eq!(broken, "74: features enabled for the running thread");
}

#[test]
fn save_area_smaller_than_the_mask_needs_is_reported() {
    let broken = spoiled(|m, at| {
        live(&mut m.threads, at.ready).mask = 1 << dodos_abi::x86_64::AMX;
    });
    assert_eq!(broken, "74: features enabled for the running thread");
    let broken = spoiled(|m, at| live(&mut m.threads, at.waiter).area = 1024);
    assert_eq!(broken, "76: save area of a thread");
}

#[test]
fn count_of_an_uncounted_context_is_reported() {
    let broken = spoiled(|m, at| {
        let context = m.threads[&at.ready].home.unwrap();
        live(&mut m.scheds, context).hw_class = 1;
    });
    assert_eq!(broken, "77: count of an uncounted context");
}

#[test]
fn instructions_charged_without_time_are_reported() {
    let broken = spoiled(|m, at| {
        let context = m.threads[&at.ready].home.unwrap();
        let sched = live(&mut m.scheds, context);
        sched.counted = true;
        let before = (sched.consumed, sched.count, true);
        sched.count += 1;
        m.event.counts.insert(context, before);
    });
    assert_eq!(
        broken,
        "77: instructions and time charged at different events"
    );
}

#[test]
fn time_charged_without_instructions_is_reported() {
    let broken = spoiled(|m, at| {
        let context = m.threads[&at.ready].home.unwrap();
        let sched = live(&mut m.scheds, context);
        sched.counted = true;
        let before = (sched.consumed, sched.count, true);
        sched.consumed += 1;
        m.event.counts.insert(context, before);
    });
    assert_eq!(
        broken,
        "77: instructions and time charged at different events"
    );
}

#[test]
fn marks_lost_by_a_move_are_reported() {
    let broken = spoiled(|m, at| m.event.moved.push((at.waiter, 1)));
    assert_eq!(broken, "78: marks lost by a move");
}

// Debugging.

#[test]
fn a_ring_entry_out_of_its_place_or_a_wrong_next_sequence_is_reported() {
    let broken =
        spoiled(|m, _| std::sync::Arc::make_mut(&mut m.table.crash).areas[0].next_kernel += 1);
    assert_eq!(broken, "88: kernel ring");
    let broken = spoiled(|m, _| {
        let area = &mut std::sync::Arc::make_mut(&mut m.table.crash).areas[0];
        area.user[3] = area.kernel[0];
        area.user[3].seq = 1;
    });
    assert_eq!(broken, "88: user ring");
}

#[test]
fn a_code_in_the_wrong_ring_is_reported() {
    let broken =
        spoiled(|m, _| std::sync::Arc::make_mut(&mut m.table.crash).areas[0].kernel[0].word2 |= 16);
    assert_eq!(broken, "89: a user code in the kernel ring");
    let broken = spoiled(|m, _| {
        let area = &mut std::sync::Arc::make_mut(&mut m.table.crash).areas[0];
        area.user[0] = area.kernel[0];
        area.next_user = 2;
    });
    assert_eq!(broken, "89: a kernel code in the user ring");
}

#[test]
fn a_call_on_debug_that_wrote_a_kernel_entry_or_wrong_entries_is_reported() {
    let broken = spoiled(|m, _| {
        m.event.user_entries = Some((1, 0, (0, 0)));
        m.event.kernel_entries = 1;
    });
    assert_eq!(broken, "89: a call on debug wrote a kernel entry");
    let broken = spoiled(|m, _| m.event.user_entries = Some((1, 1, (0, 0))));
    assert_eq!(broken, "90: entries of a call on debug");
}

#[test]
fn entries_over_the_rate_limit_or_suppressed_entries_not_counted_are_reported() {
    let broken = spoiled(|m, _| {
        let entry = std::sync::Arc::make_mut(&mut m.table.crash).areas[0].kernel[0];
        for _ in 0..17 {
            let mut copy = entry;
            copy.word2 = (copy.word2 & !0xFFFF) | 9;
            std::sync::Arc::make_mut(&mut m.table.crash).areas[0].write_raw(false, copy);
        }
    });
    assert_eq!(broken, "91: more than 16 entries of a code in a window");
    let broken =
        spoiled(|m, _| std::sync::Arc::make_mut(&mut m.table.crash).areas[0].suppressed += 1);
    assert_eq!(broken, "91: suppressed entries not counted once");
}

#[test]
fn a_record_of_another_boot_or_a_written_previous_area_is_reported() {
    let broken = spoiled(|m, _| {
        std::sync::Arc::make_mut(&mut m.table.crash).areas[1].record = Some(crate::debug::Record {
            commit: 7,
            generation: 1,
            reason: 1,
            instant: 0,
            member: 0,
        });
    });
    assert_eq!(broken, "92: a record committed with another boot");
    let broken = spoiled(|m, _| m.table.previous = Some((*m.table.crash).clone()));
    assert_eq!(broken, "92: the previous crash area written");
}

#[test]
fn an_access_to_a_refused_port_or_a_wide_ioports_child_is_reported() {
    let broken = spoiled(|m, _| m.event.ports = Some((0x21, 0x22)));
    assert_eq!(broken, "93: an access to a refused port");
    let broken = spoiled(|m, _| m.event.ports = Some((0xCFC, 0xCFF)));
    assert_eq!(broken, "93: an access to a refused port");
    let broken = spoiled(|m, at| {
        let ports = m
            .objects
            .values()
            .find(|o| o.kind == ObjectKind::IoPorts)
            .map(|o| o.root)
            .unwrap();
        let all = Rights::ALL.without(Rights::EXEC);
        let place = |index| Place {
            space: at.space,
            index,
        };
        m.derive(ports, all, 0x10 | 0x20 << 16, place(4090));
        let child = *m.caps[&ports].children.last().unwrap();
        m.derive(child, all, 0x10 | 0x30 << 16, place(4091));
    });
    assert_eq!(broken, "93: ioports range outside its parent's");
}

#[test]
fn a_breakpoint_word_outside_the_user_range_is_reported() {
    let broken =
        spoiled(|m, at| live(&mut m.threads, at.waiter).regs[dodos_abi::x86_64::REG_DR0] = 1 << 63);
    assert_eq!(broken, "95: breakpoint words of a thread");
    let broken =
        spoiled(|m, at| live(&mut m.threads, at.waiter).regs[dodos_abi::x86_64::REG_UINTR] = 1);
    assert_eq!(broken, "95: breakpoint words of a thread");
}

// The gate over page tables, judged by the calls (sixth review, DeepSeek).

/// A frame of one page in slot 120, and its frame record.
fn with_frame() -> (Model, ObjId, Frame) {
    let (mut m, t) = Model::boot(&Machine::example()).unwrap();
    let retype = Syscall::UntypedRetype {
        untyped: SlotAddr::own(UNTYPED),
        kind: ObjectKind::Frame,
        size: 1,
        page: dodos_abi::PageSize::Page as u64,
        memory: 0,
        destination: SlotAddr::own(120),
    };
    assert_eq!(m.apply(t, retype), Ok(Output::Done));
    let object = m.object_at(t, SlotAddr::own(120)).unwrap();
    let frame = m.frames[&object];
    (m, t, frame)
}

fn map_frame(rights: Rights) -> Syscall {
    Syscall::ASpaceMap(Map {
        space: SlotAddr::own(dodos_abi::START_SLOT_ASPACE),
        frame: SlotAddr::own(120),
        untyped: SlotAddr::NONE,
        address: 1 << 30,
        first: 0,
        count: 1,
        rights: rights.bits(),
    })
}

/// DeepSeek F1 and F2: the call itself reads the class of every page a leaf
/// targets (G3 (3) and (4)), and refuses with `denied`: a table page; a
/// global read-only page for `write`; a device page for `exec`.
#[test]
fn a_leaf_whose_page_class_refuses_it_is_denied_by_the_call() {
    let (mut m, t, frame) = with_frame();
    let page = m.geometry().page();
    std::sync::Arc::make_mut(&mut m.table.classes)
        .insert(frame.base / page, crate::table::PageClass::Table);
    assert_eq!(m.apply(t, map_frame(Rights::READ)), Err(Error::Denied));
    let (mut m, t, frame) = with_frame();
    let object = m.object_at(t, SlotAddr::own(120)).unwrap();
    live(&mut m.frames, object).kept = true;
    let rw = Rights::READ.union(Rights::WRITE);
    assert_eq!(m.apply(t, map_frame(rw)), Err(Error::Denied));
    assert_eq!(m.apply(t, map_frame(Rights::READ)), Ok(Output::Done));
    let _ = frame;
    let (mut m, t, _) = with_frame();
    let object = m.object_at(t, SlotAddr::own(120)).unwrap();
    live(&mut m.frames, object).memory = dodos_abi::MemoryType::Uncached;
    let rx = Rights::READ.union(Rights::EXEC);
    assert_eq!(m.apply(t, map_frame(rx)), Err(Error::Denied));
    assert_eq!(m.apply(t, map_frame(Rights::READ)), Ok(Output::Done));
    // A protect that would add what the class refuses is denied too.
    let protect = Syscall::ASpaceProtect {
        aspace: SlotAddr::own(dodos_abi::START_SLOT_ASPACE),
        frame: SlotAddr::own(120),
        rights: rx.bits(),
    };
    assert_eq!(m.apply(t, protect), Err(Error::Denied));
}

/// DeepSeek F5 (M08): a leaf of the second large page counts in each of the
/// E chunks it covers.
#[test]
fn a_leaf_of_the_second_large_page_counts_in_every_chunk_it_covers() {
    let (m, _) = Model::boot(&Machine::example()).unwrap();
    let frame = Frame {
        page: dodos_abi::PageSize::Large2,
        pages: 2,
        memory: dodos_abi::MemoryType::WriteBack,
        base: 0,
        kept: false,
    };
    let chunks = m.leaf_chunks(frame, 1, 1);
    let entries = u64::from(m.geometry().entries);
    assert_eq!(chunks, (entries..2 * entries).collect::<Vec<_>>());
}

/// DeepSeek F5 (M20): the gate refuses to give a block one of whose chunks
/// has a map count, even when no piece of it lives (G5).
#[test]
fn the_gate_refuses_to_give_a_block_whose_map_count_is_not_zero() {
    let (mut m, t) = Model::boot(&Machine::example()).unwrap();
    let shift = m.geometry().chunk_shift();
    let free = (0..m.table.owners.len() as u64)
        .find(|&c| m.ownership(c) == Ownership::free(Owner::Unclaimed).raw())
        .unwrap();
    let take = Syscall::MemoryTake {
        control: SlotAddr::own(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: 0,
        place: free << shift,
        size: u64::from(shift),
        destination: SlotAddr::own(120),
    };
    assert_eq!(m.apply(t, take), Ok(Output::Done));
    std::sync::Arc::make_mut(&mut m.table.maps)[free as usize] = 1;
    let give = Syscall::MemoryGive {
        control: SlotAddr::own(dodos_abi::START_SLOT_MEMORY_CONTROL),
        untyped: SlotAddr::own(120),
        node: 1,
    };
    assert_eq!(m.apply(t, give.clone()), Err(Error::Busy));
    std::sync::Arc::make_mut(&mut m.table.maps)[free as usize] = 0;
    assert_eq!(m.apply(t, give), Ok(Output::Done));
}

/// DeepSeek F5 (M11): a use entry counts the leaves its node holds under
/// the grant (G6); one that counts others is reported.
#[test]
fn a_use_entry_that_counts_leaves_its_node_does_not_hold_is_reported() {
    let broken = spoiled(|m, _| {
        let node = m.node as usize;
        m.table.uses[node].insert(
            9,
            crate::table::UseEntry {
                granter: 1,
                grant: 0,
                serial: 0,
                generation: 1,
                leaves: 1,
            },
        );
    });
    assert_eq!(broken, "58: leaves of a use entry");
}

/// 88: the identifier word of an attached device names its node.
#[test]
fn an_attached_device_whose_identifier_word_is_not_its_nodes_is_reported() {
    let broken = spoiled(|m, _| {
        m.table
            .identifiers
            .insert(4, Ownership::free(Owner::Unclaimed).raw());
        let taken = Ownership::taken(Owner::Node(0), m.table.generation(0)).raw();
        m.table.identifiers.insert(7, taken);
    });
    assert_eq!(broken, "88: identifier words and attached devices");
}
