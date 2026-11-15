use dodos_abi::ObjectKind::{Frame, IoSpace, Notify, Reply, Sched, Thread};
use dodos_abi::*;
use dodos_model::*;

const DONE: Outcome = Ok(Output::Done);
const WAITS: Outcome = Ok(Output::Waits);
const UT: SlotAddr = SlotAddr::own(25);
const MAILBOX: SlotAddr = SlotAddr::own(START_SLOT_MAILBOX);
const ASPACE: SlotAddr = SlotAddr::own(START_SLOT_ASPACE);
const MS: u64 = 1_000_000;

const IOMMU_ROLE_3: u32 = 20;
const RESERVE_ROLE_2: u32 = 21;

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(16 + n)
}

fn retype(kind: ObjectKind, size: u64, destination: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: UT,
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(destination),
    }
}

fn stop(n: u32, mode: u64) -> Syscall {
    Syscall::CoreStop {
        core: core(n),
        mode,
    }
}

fn start(n: u32, mode: u64) -> Syscall {
    Syscall::CoreStart {
        core: core(n),
        mode,
        image: 0,
        module: 0,
    }
}

fn msg(label: u32, caps: Vec<Transfer>) -> Message {
    Message {
        label,
        flags: 0,
        words: vec![label.into()],
        caps,
        marks: 0,
    }
}

fn two() -> (System, Vec<ObjId>) {
    let mut machine = Machine::example();
    machine.membership.stale_ms = 10;
    machine.membership.stuck_ms = 10;
    machine.membership.sweep_ms = 100;
    System::boot(&machine).unwrap()
}

fn three() -> (System, Vec<ObjId>) {
    let mut machine = Machine::example();
    let mut third = machine.cores[1];
    third.hw_id = 4;
    machine.cores.push(third);
    machine.membership.stale_ms = 10;
    machine.membership.stuck_ms = 10;
    machine.membership.sweep_ms = 100;
    System::boot(&machine).unwrap()
}

fn spawn(sys: &mut System, t: &[ObjId], node: u16, slot: u32) -> ObjId {
    let spare = 4000 - 2 * (slot - 200);
    assert_eq!(
        sys.apply(node, t[node as usize], retype(Thread, 0, slot)),
        DONE
    );
    assert_eq!(
        sys.apply(node, t[node as usize], retype(Frame, 1, spare)),
        DONE
    );
    assert_eq!(
        sys.apply(
            node,
            t[node as usize],
            Syscall::ThreadConfigure {
                thread: s(slot),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(spare),
            },
        ),
        DONE
    );
    assert_eq!(
        sys.apply(node, t[node as usize], retype(Sched, 0, spare - 1)),
        DONE
    );
    assert_eq!(
        sys.apply(
            node,
            t[node as usize],
            Syscall::SchedConfigure {
                sched: s(spare - 1),
                core: core(node.into()),
                budget: 10_000,
                period: 10_000_000,
                class: 1,
            },
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            node,
            t[node as usize],
            Syscall::SchedBind {
                sched: s(spare - 1),
                thread: s(slot),
            },
        ),
        DONE
    );
    assert_eq!(
        sys.apply(node, t[node as usize], Syscall::ThreadResume(s(slot))),
        DONE
    );
    sys.node(node).object_at(t[node as usize], s(slot)).unwrap()
}

fn setup_two_for_spawn() -> (System, Vec<ObjId>) {
    let (mut sys, t) = two();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 5_000_000,
                    period: 10_000_000,
                    class: 1,
                },
            ),
            DONE
        );
    }
    (sys, t)
}

#[test]
fn f1a_a_grant_name_is_not_reused_across_generations_of_its_node() {
    let (mut sys, t) = three();
    for node in 0..3u16 {
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
    }
    // Node 0 grants an unmapped Notify (grant 0, serial 0) to node 2.
    assert_eq!(sys.apply(0, t[0], retype(Notify, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            2,
            t[2],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::Send {
                target: core(2),
                message: msg(1, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);
    assert_eq!(sys.check(), Ok(()));

    // Node 1 seals node 2 (its remote cap is unmapped, maps_of[0] == false).
    assert_eq!(sys.apply(1, t[1], stop(2, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 2 * MS, MS / 10);
    assert_eq!(sys.member(2).state(), MemberState::Sealed);

    // Node 1 kills node 0 -> node 0 becomes Dead without waiting for sealed node 2.
    assert_eq!(sys.apply(1, t[1], stop(0, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 10 * MS, MS / 10);
    assert_eq!(sys.member(0).state(), MemberState::Dead);

    // Node 1 restarts node 0 fresh; node 0 reuses grant = 0, serial = 0 for node 1!
    assert_eq!(sys.apply(1, t[1], start(0, CORE_START_FRESH)), DONE);
    let t0 = sys.node(0).object_at_start(START_SLOT_THREAD);
    assert_eq!(sys.apply(0, t0, retype(Notify, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t0,
            Syscall::Send {
                target: core(1),
                message: msg(2, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);
    // The new generation's grant has a new serial: the sealed holder's use
    // entry names generation 1 of node 0, which names nothing now.
    assert_eq!(sys.node(0).grant_entry(0).map(|e| e.2), Some(1));
    assert_eq!(sys.check(), Ok(()));
    // Resumed, node 2 drops its remote capability of the ended generation
    // before any thread runs, and sends no release.
    assert_eq!(sys.apply(1, t[1], start(2, CORE_START_RESUME)), DONE);
    assert!(sys.pending().iter().all(|p| p.from != 2));
    assert_eq!(
        sys.apply(2, t[2], Syscall::CapDelete(s(110))),
        Err(Error::Invalid)
    );
    sys.settle(32);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f1b_a_holder_resumed_after_the_granter_restarted_cannot_free_a_new_grant() {
    let (mut sys, t) = three();
    for node in 0..3u16 {
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
    }
    // Node 0 grants an unmapped Notify (grant 0, serial 0) to node 2.
    assert_eq!(sys.apply(0, t[0], retype(Notify, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            2,
            t[2],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::Send {
                target: core(2),
                message: msg(1, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);

    // Seal node 2, kill node 0, restart node 0 fresh.
    assert_eq!(sys.apply(1, t[1], stop(2, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 2 * MS, MS / 10);
    assert_eq!(sys.apply(1, t[1], stop(0, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 10 * MS, MS / 10);
    assert_eq!(sys.apply(1, t[1], start(0, CORE_START_FRESH)), DONE);
    let t0 = sys.node(0).object_at_start(START_SLOT_THREAD);

    // Restarted node 0 takes a fresh Frame block and grants it to node 1 (grant = 0, serial = 0).
    let chunk = sys.node(0).geometry().chunk();
    let shift = u64::from(sys.node(0).geometry().chunk_shift());
    let free = Machine::example().memory[4].base + 12 * chunk;
    assert_eq!(
        sys.apply(
            0,
            t0,
            Syscall::MemoryTake {
                control: s(START_SLOT_MEMORY_CONTROL),
                kind: 0,
                place: free,
                size: shift,
                destination: s(200),
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t0,
            Syscall::UntypedRetype {
                untyped: s(200),
                kind: Frame,
                size: 1,
                page: PageSize::Page as u64,
                memory: 0,
                destination: s(100),
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t0,
            Syscall::Send {
                target: core(1),
                message: msg(2, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);
    // Node 1 maps the granted frame at 1 GB!
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ASpaceMap(Map {
                space: ASPACE,
                frame: s(110),
                untyped: SlotAddr::NONE,
                address: 1 << 30,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            })
        ),
        DONE
    );
    let mapped_frame = sys.node(1).translated(t[1], 1 << 30);
    assert!(mapped_frame.is_some());

    // Resume node 2; its remote capability of node 0's first generation is
    // dropped with no release, so node 1's grant stays live and mapped.
    assert_eq!(sys.apply(1, t[1], start(2, CORE_START_RESUME)), DONE);
    assert_eq!(
        sys.apply(2, t[2], Syscall::CapDelete(s(110))),
        Err(Error::Invalid)
    );
    sys.settle(32);
    assert_eq!(sys.check(), Ok(()));
    // Deleting the frame on node 0 must revoke node 1's mapping first: the
    // removal waits for node 1, and the chunk cannot be given meanwhile.
    assert_eq!(sys.apply(0, t0, Syscall::CapDelete(s(100))), WAITS);
    assert!(sys.node(1).translated(t[1], 1 << 30).is_some());
    sys.settle(32);
    assert!(sys.node(1).translated(t[1], 1 << 30).is_none());
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f2a_a_transfer_to_a_destroyed_receiver_is_refused_and_frees_its_proxy() {
    let (mut sys, t) = setup_two_for_spawn();
    let creator0 = spawn(&mut sys, &t, 0, 290);
    let r = spawn(&mut sys, &t, 1, 200);
    assert_eq!(sys.apply(0, t[0], retype(Notify, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            1,
            r,
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::Send {
                target: core(1),
                message: msg(1, vec![Transfer::Move(s(100))]),
            }
        ),
        WAITS
    );
    let req_idx = sys.find(|b| matches!(b, Body::Request { .. })).unwrap();
    sys.deliver(req_idx);
    let ask_idx = sys.find(|b| matches!(b, Body::TransferAsk { .. })).unwrap();
    sys.deliver(ask_idx);
    assert_eq!(sys.node(0).object_at(creator0, s(100)), None);

    // Before TransferGrant arrives, node 1 deletes receiver thread r!
    while sys.apply(1, t[1], Syscall::CapDelete(s(200))) != DONE {}
    let grant_idx = sys
        .find(|b| matches!(b, Body::TransferGrant { .. }))
        .unwrap();
    sys.deliver(grant_idx);
    sys.settle(32);
    // The grants are refused: the moved capability is back in its slot, and
    // the sender learns that the capabilities were dropped.
    assert_eq!(
        sys.node(0).woken(t[0]),
        Some(&Ok(Output::Sent {
            flags: Header::FLAG_CAPS_DROPPED
        }))
    );
    assert!(sys.node(0).object_at(creator0, s(100)).is_some());
    assert_eq!(sys.check(), Ok(()));

    // Repeat 15 more times to exhaust all 16 proxies for peer 0 on node 1!
    let t_spawn0 = [creator0, t[1]];
    for i in 1..PROXIES_PER_PEER as u32 {
        let sender = spawn(&mut sys, &t_spawn0, 0, 200 + i);
        let recver = spawn(&mut sys, &t, 1, 200 + i);
        assert_eq!(sys.apply(0, creator0, retype(Notify, 0, 300 + i)), DONE);
        assert_eq!(
            sys.apply(
                1,
                recver,
                Syscall::Recv {
                    endpoint: MAILBOX,
                    reply: s(99),
                    range: Range {
                        first: s(300 + i),
                        count: 1,
                    },
                    wait: 0,
                }
            ),
            WAITS
        );
        assert_eq!(
            sys.apply(
                0,
                sender,
                Syscall::Send {
                    target: core(1),
                    message: msg(1, vec![Transfer::Move(s(300 + i))]),
                }
            ),
            WAITS
        );
        let req_idx = sys.find(|b| matches!(b, Body::Request { .. })).unwrap();
        sys.deliver(req_idx);
        let ask_idx = sys.find(|b| matches!(b, Body::TransferAsk { .. })).unwrap();
        sys.deliver(ask_idx);
        while sys.apply(1, t[1], Syscall::CapDelete(s(200 + i))) != DONE {}
        let grant_idx = sys
            .find(|b| matches!(b, Body::TransferGrant { .. }))
            .unwrap();
        sys.deliver(grant_idx);
        sys.settle(32);
    }
    // No proxy of node 0 on node 1 is lost: a seventeenth message goes
    // through.
    let sender17 = spawn(&mut sys, &t_spawn0, 0, 220);
    let recver17 = spawn(&mut sys, &t, 1, 220);
    assert_eq!(
        sys.apply(
            1,
            recver17,
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range::NONE,
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            sender17,
            Syscall::Send {
                target: core(1),
                message: msg(99, vec![]),
            }
        ),
        WAITS
    );
    sys.settle(64);
    assert_eq!(
        sys.node(0).woken(sender17),
        Some(&Ok(Output::Sent { flags: 0 }))
    );
    assert!(matches!(
        sys.node(1).woken(recver17),
        Some(Ok(Output::Message(_)))
    ));
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f2b_a_revocation_whose_thread_is_destroyed_still_frees_its_grant() {
    let (mut sys, t) = setup_two_for_spawn();
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 100)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::Send {
                target: core(1),
                message: msg(1, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ASpaceMap(Map {
                space: ASPACE,
                frame: s(110),
                untyped: SlotAddr::NONE,
                address: 1 << 30,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            })
        ),
        DONE
    );
    let helper = spawn(&mut sys, &t, 0, 200);
    assert_eq!(sys.apply(0, helper, Syscall::CapRevoke(s(100))), WAITS);
    assert_eq!(sys.node(0).grant_entry(0).map(|e| e.3), Some(1));
    while sys.apply(0, t[0], Syscall::CapDelete(s(200))) != DONE {}
    sys.settle(32);
    // The holder removed its mapping and the entry is freed, though the
    // thread that asked is gone.
    assert!(sys.node(1).translated(t[1], 1 << 30).is_none());
    assert_eq!(sys.node(0).grant_entry(0), None);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f3a_a_move_to_a_node_accused_before_it_answers_ends_both_waits() {
    let (mut sys, t) = three();
    for node in 0..3u16 {
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 5_000_000,
                    period: 10_000_000,
                    class: 1,
                },
            ),
            DONE
        );
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
    }
    let helper0 = spawn(&mut sys, &t, 0, 200);
    assert_eq!(sys.apply(0, t[0], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], retype(Sched, 0, 102)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedConfigure {
                sched: s(102),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedSetCores {
                sched: s(102),
                core: core(0),
                window: 0,
                mask: 0b11,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedBind {
                sched: s(102),
                thread: s(100),
            }
        ),
        DONE
    );

    assert_eq!(sys.apply(1, t[1], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(1, t[1], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(1, t[1], retype(Sched, 0, 102)), DONE);
    let mut r1 = 109;
    for src in [100, 102] {
        r1 += 1;
        assert_eq!(
            sys.apply(
                0,
                t[0],
                Syscall::Recv {
                    endpoint: MAILBOX,
                    reply: s(99),
                    range: Range {
                        first: s(r1),
                        count: 1,
                    },
                    wait: 0,
                }
            ),
            WAITS
        );
        assert_eq!(
            sys.apply(
                1,
                t[1],
                Syscall::Send {
                    target: core(0),
                    message: msg(1, vec![Transfer::Derive(s(src), Rights::MANAGE.bits())]),
                }
            ),
            WAITS
        );
        sys.settle(32);
    }

    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadMove {
                thread: s(100),
                destination: s(110),
                sched: s(111),
                core: core(1),
                budget: 0,
                period: 0,
                class: 0,
            }
        ),
        WAITS
    );
    assert!(sys.find(|b| matches!(b, Body::Move { .. })).is_some());

    assert_eq!(sys.apply(2, t[2], stop(1, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 20 * MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Dead);
    assert!(sys.pending().is_empty());

    // The caller gets `gone`; the thread is as before the call: resumed, it
    // runs on its own context.
    assert_eq!(sys.node(0).woken(t[0]), Some(&Err(Error::Gone)));
    let src_id = sys.node(0).object_at(helper0, s(100)).unwrap();
    assert_eq!(sys.apply(0, helper0, Syscall::ThreadResume(s(100))), DONE);
    assert!(sys.node(0).runnable().contains(&src_id));
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f3b_a_transfer_from_a_node_accused_before_its_grants_ends_the_receivers_wait() {
    let (mut sys, t) = three();
    for node in 0..3u16 {
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
    }
    assert_eq!(sys.apply(2, t[2], retype(Notify, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            2,
            t[2],
            Syscall::Send {
                target: core(1),
                message: msg(1, vec![Transfer::Move(s(100))]),
            }
        ),
        WAITS
    );
    let req_idx = sys.find(|b| matches!(b, Body::Request { .. })).unwrap();
    sys.deliver(req_idx);
    assert!(
        sys.find(|b| matches!(b, Body::TransferAsk { .. }))
            .is_some()
    );

    assert_eq!(sys.apply(0, t[0], stop(2, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 20 * MS, MS / 10);
    assert_eq!(sys.member(2).state(), MemberState::Dead);
    assert!(sys.pending().is_empty());

    assert_eq!(sys.node(1).woken(t[1]), Some(&Err(Error::Gone)));
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f4_a_reserve_whose_holder_dies_with_a_taken_chunk_can_be_claimed_again() {
    let (mut sys, t) = two();
    let chunk = sys.node(0).geometry().chunk();
    let shift = u64::from(sys.node(0).geometry().chunk_shift());
    let reserve_addr = Machine::example().memory[4].base + 6 * chunk;

    assert_eq!(
        sys.apply(1, t[1], Syscall::RoleClaim(s(RESERVE_ROLE_2))),
        Ok(Output::Epoch(1))
    );
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::MemoryTake {
                control: s(START_SLOT_MEMORY_CONTROL),
                kind: 0,
                place: reserve_addr,
                size: shift,
                destination: s(100),
            }
        ),
        DONE
    );

    assert_eq!(sys.apply(0, t[0], stop(1, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 20 * MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Dead);
    assert_eq!(sys.role(2).state(), RoleState::Vacant);

    // R6 freed the chunk with the reserve's code: the reserve can be
    // claimed, and its chunk taken again.
    let index = reserve_addr >> shift;
    assert_eq!(
        sys.node(0).ownership(index),
        Ownership::free(Owner::Reserve(0)).raw()
    );
    assert!(matches!(
        sys.apply(0, t[0], Syscall::RoleClaim(s(RESERVE_ROLE_2))),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::MemoryTake {
                control: s(START_SLOT_MEMORY_CONTROL),
                kind: 0,
                place: reserve_addr,
                size: shift,
                destination: s(100),
            }
        ),
        DONE
    );
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f5a_step_5_of_a_hand_over_runs_r1_for_a_node_already_dying() {
    let (mut sys, t) = three();

    // Node 0 attaches device 0x10 through node 1 (holder of IOMMU unit role 0, cap at slot 20).
    assert_eq!(sys.apply(0, t[0], retype(IoSpace, 0, 100)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::IoSpaceAttach {
                iommu: s(19),
                iospace: s(100),
                device: 0x10,
            }
        ),
        WAITS
    );
    sys.run(sys.now() + MS, MS / 10);
    assert_eq!(sys.node(0).woken(t[0]), Some(&DONE));

    // Node 1 hands IOMMU unit role (slot 20) to node 2 -> RoleState::Handing(1 -> 2).
    assert!(matches!(
        sys.apply(
            1,
            t[1],
            Syscall::RoleHand {
                role: s(IOMMU_ROLE_3),
                core: core(2),
            }
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(sys.role(0).state(), RoleState::Handing);

    // Node 2 kills node 0 and seals node 1 before node 1 runs isolate(0).
    assert_eq!(sys.apply(2, t[2], stop(0, CORE_STOP_KILL)), DONE);
    assert_eq!(sys.apply(2, t[2], stop(1, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 250 * MS, MS);
    // Node 2 took the unit at step 5 and blocked node 0's device there
    // (R1), so node 0's barrier holds: it is dead, and no context entry of
    // the unit names it.
    assert_eq!(sys.member(1).state(), MemberState::Sealed);
    assert_eq!(sys.role(0).state(), RoleState::Held);
    assert_eq!(sys.role(0).holder(), 2);
    assert_eq!(sys.member(0).state(), MemberState::Dead);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f5b_a_seal_of_the_target_of_a_hand_over_waits_for_it_and_hands_on() {
    let (mut sys, t) = three();
    assert!(matches!(
        sys.apply(
            1,
            t[1],
            Syscall::RoleHand {
                role: s(IOMMU_ROLE_3),
                core: core(2),
            }
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(sys.role(0).state(), RoleState::Handing);
    assert_eq!(sys.apply(0, t[0], stop(2, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 300 * MS, MS);
    // Node 2, sealing, completed step 5 and handed the role on: it is
    // sealed, and the unit is held by a live node.
    assert_eq!(sys.member(2).state(), MemberState::Sealed);
    assert_eq!(sys.role(0).state(), RoleState::Held);
    assert_ne!(sys.role(0).holder(), 2);
    assert_eq!(sys.member(sys.role(0).holder()).state(), MemberState::Live);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f6a_killing_a_node_that_shares_a_core_stops_the_node_not_the_core() {
    let (mut sys, t) = two();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 1_000_000,
                    period: 10_000_000,
                    class: 1,
                }
            ),
            DONE
        );
    }
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::CorePlace {
                core: core(1),
                physical: 0,
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], stop(1, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 5 * MS, MS / 10);
    // Node 1's block was saved while node 0 ran: it is fenced without a
    // stop of core 0, which goes on running node 0.
    assert_eq!(sys.core(0), CoreState::Running);
    assert!(sys.runs(0) && !sys.runs(1));
    assert_eq!(sys.member(1).state(), MemberState::Dead);
    assert_eq!(sys.check(), Ok(()));

    // Part 2: 3-node machine, node 2 sealed, nodes 0 & 1 on core 0 -> all 3 nodes non-running!
    let (mut sys3, t3) = three();
    for node in 0..2u16 {
        assert_eq!(
            sys3.apply(
                node,
                t3[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 1_000_000,
                    period: 10_000_000,
                    class: 1,
                }
            ),
            DONE
        );
    }
    assert_eq!(
        sys3.apply(
            0,
            t3[0],
            Syscall::CorePlace {
                core: core(1),
                physical: 0,
            }
        ),
        DONE
    );
    assert_eq!(sys3.apply(0, t3[0], stop(2, CORE_STOP_SEAL)), DONE);
    sys3.run(sys3.now() + 2 * MS, MS / 10);
    assert_eq!(sys3.member(2).state(), MemberState::Sealed);
    assert_eq!(sys3.apply(0, t3[0], stop(1, CORE_STOP_KILL)), DONE);
    sys3.run(sys3.now() + 2 * MS, MS / 10);
    assert!(sys3.runs(0) && !sys3.runs(1) && !sys3.runs(2));
    assert_eq!(sys3.check(), Ok(()));
}

#[test]
fn f6b_sealing_a_node_that_shares_a_core_leaves_the_core_running() {
    let (mut sys, t) = three();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 1_000_000,
                    period: 10_000_000,
                    class: 1,
                }
            ),
            DONE
        );
    }
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::CorePlace {
                core: core(1),
                physical: 0,
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(2, t[2], stop(1, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 2 * MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Sealed);
    assert_eq!(sys.member(0).state(), MemberState::Live);
    assert_eq!(sys.core(0), CoreState::Running);
    assert!(sys.runs(0) && !sys.runs(1));
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f7_stopping_two_nodes_of_one_core_follows_the_table_of_transitions() {
    let (mut sys, t) = three();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 1_000_000,
                    period: 10_000_000,
                    class: 1,
                }
            ),
            DONE
        );
    }
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::CorePlace {
                core: core(1),
                physical: 0,
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(2, t[2], stop(0, CORE_STOP_KILL)), DONE);
    assert_eq!(sys.apply(2, t[2], stop(1, CORE_STOP_RETIRE)), DONE);
    // Check sys.check() after EVERY single recovery/settle/advance step (member_writes is ephemeral!).
    for _ in 0..500 {
        if sys.recover(1) == 0 && sys.settle(1) == 0 {
            let now = sys.now();
            sys.advance(now + MS / 10);
        }
        assert_eq!(sys.check(), Ok(()));
    }
    assert_eq!(sys.member(0).state(), MemberState::Dead);
    assert_eq!(sys.member(1).state(), MemberState::Dead);
}

#[test]
fn f8_an_unfenced_node_whose_driver_dies_is_driven_again() {
    let (mut sys, t) = three();
    sys.hang(2, false);
    sys.set_vmx(2, true);
    assert_eq!(sys.apply(0, t[0], stop(2, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + FENCE_NS + MS, MS / 10);
    assert_eq!(sys.member(2).state(), MemberState::Unfenced);
    assert_eq!(sys.member(2).driver(), 0);

    assert_eq!(sys.apply(1, t[1], stop(0, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 20 * MS, MS / 10);
    assert_eq!(sys.member(0).state(), MemberState::Dead);

    sys.set_vmx(2, false);
    sys.run(sys.now() + 500 * MS, MS);
    // Node 1 drives the fence again; once node 2 left VMX root operation,
    // INIT stops it, and it is reclaimed.
    assert_eq!(sys.member(2).state(), MemberState::Dead);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f9_a_pinned_grant_is_freed_when_its_grantee_restarted_before_the_unpin() {
    let (mut sys, t) = two();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(node, t[node as usize], retype(Reply, 0, 99)),
            DONE
        );
    }
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 100)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::Recv {
                endpoint: MAILBOX,
                reply: s(99),
                range: Range {
                    first: s(110),
                    count: 1,
                },
                wait: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::Send {
                target: core(1),
                message: msg(1, vec![Transfer::Derive(s(100), Rights::READ.bits())]),
            }
        ),
        WAITS
    );
    sys.settle(32);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ASpaceMap(Map {
                space: ASPACE,
                frame: s(110),
                untyped: SlotAddr::NONE,
                address: 1 << 30,
                first: 0,
                count: 1,
                rights: Rights::READ.bits(),
            })
        ),
        DONE
    );

    assert_eq!(sys.apply(0, t[0], stop(1, CORE_STOP_KILL)), DONE);
    for _ in 0..1000 {
        if sys.member(1).state() == MemberState::Dead {
            break;
        }
        if sys.recover(1) == 0 && sys.settle(1) == 0 {
            let now = sys.now();
            sys.advance(now + MS / 10);
        }
    }
    assert_eq!(sys.member(1).state(), MemberState::Dead);

    assert_eq!(sys.apply(0, t[0], start(1, CORE_START_FRESH)), DONE);
    assert_eq!(sys.member(1).state(), MemberState::Live);
    assert_eq!(sys.member(1).generation(), 2);

    sys.run(sys.now() + 50 * MS, MS);
    assert_eq!(sys.node(0).grant_entry(0), None);
    assert_eq!(sys.apply(0, t[0], Syscall::CapRevoke(s(100))), DONE);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f10a_the_context_of_a_move_in_flight_is_destroyed_only_after_the_answer() {
    let (mut sys, t) = setup_two_for_spawn();
    let helper = spawn(&mut sys, &t, 0, 200);
    assert_eq!(sys.apply(0, t[0], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], retype(Sched, 0, 102)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedConfigure {
                sched: s(102),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedSetCores {
                sched: s(102),
                core: core(0),
                window: 0,
                mask: 0b11,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedBind {
                sched: s(102),
                thread: s(100),
            }
        ),
        DONE
    );
    for i in 0..35u32 {
        assert_eq!(
            sys.apply(
                0,
                t[0],
                Syscall::CapDerive {
                    source: s(102),
                    destination: s(300 + i),
                    rights: Rights::READ.bits(),
                    badge: 0,
                }
            ),
            DONE
        );
    }

    assert_eq!(sys.apply(1, t[1], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(1, t[1], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(1, t[1], retype(Sched, 0, 102)), DONE);
    for (idx, src) in [100u32, 102].into_iter().enumerate() {
        assert_eq!(
            sys.apply(
                0,
                t[0],
                Syscall::Recv {
                    endpoint: MAILBOX,
                    reply: s(99),
                    range: Range {
                        first: s(110 + idx as u32),
                        count: 1,
                    },
                    wait: 0,
                }
            ),
            WAITS
        );
        assert_eq!(
            sys.apply(
                1,
                t[1],
                Syscall::Send {
                    target: core(0),
                    message: msg(1, vec![Transfer::Derive(s(src), Rights::MANAGE.bits())]),
                }
            ),
            WAITS
        );
        sys.settle(32);
    }

    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadMove {
                thread: s(100),
                destination: s(110),
                sched: s(111),
                core: core(1),
                budget: 0,
                period: 0,
                class: 0,
            }
        ),
        WAITS
    );
    // The removal waits for the answer and changes nothing meanwhile.
    for _ in 0..3 {
        assert_eq!(
            sys.apply(0, helper, Syscall::CapDelete(s(102))),
            Err(Error::Again)
        );
        assert_eq!(sys.check(), Ok(()));
    }
    sys.settle(32);
    assert_eq!(sys.node(0).woken(t[0]), Some(&DONE));
    assert_eq!(sys.check(), Ok(()));
    while sys.apply(0, helper, Syscall::CapDelete(s(102))) != DONE {}
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f10b_a_move_into_a_dying_destination_sched_is_gone() {
    let (mut sys, t) = setup_two_for_spawn();
    assert_eq!(sys.apply(0, t[0], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], retype(Sched, 0, 102)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedConfigure {
                sched: s(102),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedSetCores {
                sched: s(102),
                core: core(0),
                window: 0,
                mask: 0b11,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedBind {
                sched: s(102),
                thread: s(100),
            }
        ),
        DONE
    );

    assert_eq!(sys.apply(1, t[1], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(1, t[1], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(1, t[1], retype(Sched, 0, 102)), DONE);
    for (idx, src) in [100u32, 102].into_iter().enumerate() {
        assert_eq!(
            sys.apply(
                0,
                t[0],
                Syscall::Recv {
                    endpoint: MAILBOX,
                    reply: s(99),
                    range: Range {
                        first: s(110 + idx as u32),
                        count: 1,
                    },
                    wait: 0,
                }
            ),
            WAITS
        );
        assert_eq!(
            sys.apply(
                1,
                t[1],
                Syscall::Send {
                    target: core(0),
                    message: msg(1, vec![Transfer::Derive(s(src), Rights::MANAGE.bits())]),
                }
            ),
            WAITS
        );
        sys.settle(32);
    }

    for i in 0..35u32 {
        assert_eq!(
            sys.apply(
                1,
                t[1],
                Syscall::CapDerive {
                    source: s(102),
                    destination: s(300 + i),
                    rights: Rights::READ.bits(),
                    badge: 0,
                }
            ),
            DONE
        );
    }

    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadMove {
                thread: s(100),
                destination: s(110),
                sched: s(111),
                core: core(1),
                budget: 0,
                period: 0,
                class: 0,
            }
        ),
        WAITS
    );

    assert_eq!(
        sys.apply(1, t[1], Syscall::CapDelete(s(102))),
        Err(Error::Again)
    );
    let move_idx = sys.find(|b| matches!(b, Body::Move { .. })).unwrap();
    sys.deliver(move_idx);
    sys.settle(32);
    // The destination sched is dying: the move is `gone`, and nothing was
    // admitted on node 1.
    assert_eq!(sys.node(0).woken(t[0]), Some(&Err(Error::Gone)));
    assert_eq!(sys.check(), Ok(()));
    while sys.apply(1, t[1], Syscall::CapDelete(s(102))) != DONE {}
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f10c_the_context_of_a_move_in_flight_cannot_be_unbound_or_rebound() {
    let (mut sys, t) = setup_two_for_spawn();
    let helper = spawn(&mut sys, &t, 0, 200);
    assert_eq!(sys.apply(0, t[0], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], retype(Sched, 0, 102)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedConfigure {
                sched: s(102),
                core: core(0),
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedSetCores {
                sched: s(102),
                core: core(0),
                window: 0,
                mask: 0b11,
            }
        ),
        DONE
    );
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::SchedBind {
                sched: s(102),
                thread: s(100),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], retype(Thread, 0, 105)), DONE);
    assert_eq!(sys.apply(0, t[0], retype(Frame, 1, 106)), DONE);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadConfigure {
                thread: s(105),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(106),
            }
        ),
        DONE
    );

    assert_eq!(sys.apply(1, t[1], retype(Thread, 0, 100)), DONE);
    assert_eq!(sys.apply(1, t[1], retype(Frame, 1, 101)), DONE);
    assert_eq!(
        sys.apply(
            1,
            t[1],
            Syscall::ThreadConfigure {
                thread: s(100),
                cspace: s(START_SLOT_CSPACE),
                aspace: ASPACE,
                fault: SlotAddr::NONE,
                page: s(101),
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(1, t[1], retype(Sched, 0, 102)), DONE);
    for (idx, src) in [100u32, 102].into_iter().enumerate() {
        assert_eq!(
            sys.apply(
                0,
                t[0],
                Syscall::Recv {
                    endpoint: MAILBOX,
                    reply: s(99),
                    range: Range {
                        first: s(110 + idx as u32),
                        count: 1,
                    },
                    wait: 0,
                }
            ),
            WAITS
        );
        assert_eq!(
            sys.apply(
                1,
                t[1],
                Syscall::Send {
                    target: core(0),
                    message: msg(1, vec![Transfer::Derive(s(src), Rights::MANAGE.bits())]),
                }
            ),
            WAITS
        );
        sys.settle(32);
    }

    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::ThreadMove {
                thread: s(100),
                destination: s(110),
                sched: s(111),
                core: core(1),
                budget: 0,
                period: 0,
                class: 0,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(0, helper, Syscall::SchedUnbind(s(102))),
        Err(Error::Busy)
    );
    assert_eq!(
        sys.apply(
            0,
            helper,
            Syscall::SchedBind {
                sched: s(102),
                thread: s(105),
            }
        ),
        Err(Error::Busy)
    );
    let move_idx = sys.find(|b| matches!(b, Body::Move { .. })).unwrap();
    sys.deliver(move_idx);
    sys.settle(32);
    assert_eq!(sys.node(0).woken(t[0]), Some(&DONE));
    // The context went with the thread: the source's is unbound and not
    // configured, and thread 105 never ran on it.
    let Ok(Output::SchedRead { state, .. }) = sys.apply(0, helper, Syscall::SchedRead(s(102)))
    else {
        panic!()
    };
    assert_eq!(state & (SCHED_STATE_CONFIGURED | SCHED_STATE_HOME), 0);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f11_the_sweep_never_accuses_a_sealed_node() {
    let (mut sys, t) = setup_two_for_spawn();
    // Node 1 has its first thread runnable when sealed.
    assert_eq!(sys.apply(0, t[0], stop(1, CORE_STOP_SEAL)), DONE);
    sys.run(sys.now() + 2 * MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Sealed);
    // Well past SWEEP_NS + STALE_NS, the parked node is still sealed and can
    // be resumed with its generation.
    sys.run(sys.now() + SWEEP_NS + STALE_NS + 20 * MS, MS);
    assert_eq!(sys.member(1).state(), MemberState::Sealed);
    assert_eq!(sys.member(1).evidence(), Evidence::None);
    assert_eq!(sys.apply(0, t[0], start(1, CORE_START_RESUME)), DONE);
    sys.run(sys.now() + MS, MS / 10);
    assert_eq!(sys.member(1).state(), MemberState::Live);
    assert_eq!(sys.member(1).generation(), 1);
    assert_eq!(sys.check(), Ok(()));
}

#[test]
fn f7b_a_node_parked_on_a_stopped_core_that_holds_a_unit_role_is_accused_not_sealed() {
    let (mut sys, t) = three();
    for node in 0..2u16 {
        assert_eq!(
            sys.apply(
                node,
                t[node as usize],
                Syscall::SchedConfigure {
                    sched: s(START_SLOT_SCHED),
                    core: core(node.into()),
                    budget: 1_000_000,
                    period: 10_000_000,
                    class: 1,
                }
            ),
            DONE
        );
    }
    // Node 1 holds unit role 0 and is placed on core 0, where node 0 runs.
    assert_eq!(sys.role(0).holder(), 1);
    assert_eq!(
        sys.apply(
            0,
            t[0],
            Syscall::CorePlace {
                core: core(1),
                physical: 0,
            }
        ),
        DONE
    );
    assert_eq!(sys.apply(0, t[0], Syscall::Yield), DONE);
    // Node 0 hangs on core 0: its stop stops the core. Node 1's block is
    // saved, but a unit role names it: it is accused with evidence host,
    // never sealed, and its role goes to a live node.
    sys.hang(0, true);
    assert_eq!(sys.apply(2, t[2], stop(0, CORE_STOP_KILL)), DONE);
    sys.run(sys.now() + 30 * MS, MS / 10);
    assert_eq!(sys.member(1).evidence(), Evidence::Host);
    assert_eq!(sys.member(1).state(), MemberState::Dead);
    assert_eq!(sys.role(0).holder(), 2);
    assert_eq!(sys.check(), Ok(()));
}
