//! Requests to the holder of a unit role whose thread is destroyed (review
//! 7: Opus N1 of the verification of Hydrogen's report, and its sibling in
//! Hydrogen's verification of the new findings): such a request holds no
//! wait on the holder, so it completes without the thread (Calls on an
//! object of another node). Taken from Hydrogen's unit tests, with the
//! assertions turned to what the contract asks; ring order is enforced by
//! `System::deliver`.

use super::*;
use crate::Output;
use dodos_abi::ObjectKind::{self, Frame, IoSpace, Reply, Sched, Thread};
use dodos_abi::{
    PageSize, RoleState, START_SLOT_ASPACE, START_SLOT_CSPACE, START_SLOT_FIRST, START_SLOT_SCHED,
    SlotAddr,
};

const DONE: crate::Outcome = Ok(crate::Output::Done);
const WAITS: crate::Outcome = Ok(crate::Output::Waits);
const ASPACE: SlotAddr = SlotAddr::own(START_SLOT_ASPACE);

const fn s(index: u32) -> SlotAddr {
    SlotAddr::own(index)
}

const fn core(n: u32) -> SlotAddr {
    SlotAddr::own(START_SLOT_FIRST + n)
}

const fn iommu_cap(nodes: usize) -> SlotAddr {
    SlotAddr::own(START_SLOT_FIRST + nodes as u32)
}

const fn role0_cap(nodes: usize) -> SlotAddr {
    SlotAddr::own(START_SLOT_FIRST + nodes as u32 + 1)
}

const fn ut_cap(nodes: usize) -> SlotAddr {
    SlotAddr::own(START_SLOT_FIRST + nodes as u32 + 6)
}

fn retype(nodes: usize, kind: ObjectKind, size: u64, dest: u32) -> Syscall {
    Syscall::UntypedRetype {
        untyped: ut_cap(nodes),
        kind,
        size,
        page: PageSize::Page as u64,
        memory: 0,
        destination: s(dest),
    }
}

fn retype_done(
    sys: &mut System,
    node: u16,
    caller: ObjId,
    nodes: usize,
    kind: ObjectKind,
    size: u64,
    dest: u32,
) {
    loop {
        match sys.apply(node, caller, retype(nodes, kind, size, dest)) {
            Ok(Output::Done) => break,
            Err(dodos_abi::Error::Again) => continue,
            other => panic!("unexpected retype outcome: {other:?}"),
        }
    }
}

fn machine_n(nodes: usize) -> Machine {
    let mut machine = Machine::example();
    while machine.cores.len() < nodes {
        let mut extra = machine.cores[1];
        extra.hw_id = 2 * machine.cores.len() as u64;
        machine.cores.push(extra);
    }
    machine.membership.stale_ms = 10;
    machine.membership.stuck_ms = 10;
    machine.membership.sweep_ms = 100;
    machine
}

fn boot_with(m: Machine) -> (System, Vec<ObjId>) {
    let nodes = m.cores.len();
    let (mut sys, t) = System::boot(&m).unwrap();
    for node in 0..nodes as u16 {
        let sched = Syscall::SchedConfigure {
            sched: s(START_SLOT_SCHED),
            core: core(node.into()),
            budget: 1_000_000,
            period: 10_000_000,
            class: 1,
        };
        assert_eq!(sys.apply(node, t[node as usize], sched), DONE);
        assert_eq!(
            sys.apply(node, t[node as usize], retype(nodes, Reply, 0, 99)),
            DONE
        );
    }
    (sys, t)
}

fn boot_n(nodes: usize) -> (System, Vec<ObjId>) {
    boot_with(machine_n(nodes))
}

fn spawn_thread(
    sys: &mut System,
    nodes: usize,
    t: &[ObjId],
    node: u16,
    slot: u32,
    spare: u32,
) -> ObjId {
    let root = t[node as usize];
    for call in [
        retype(nodes, Thread, 0, slot),
        retype(nodes, Frame, 1, spare),
        Syscall::ThreadConfigure {
            thread: s(slot),
            cspace: s(START_SLOT_CSPACE),
            aspace: ASPACE,
            fault: SlotAddr::NONE,
            page: s(spare),
        },
        retype(nodes, Sched, 0, spare - 1),
        Syscall::SchedConfigure {
            sched: s(spare - 1),
            core: core(node.into()),
            budget: 500_000,
            period: 10_000_000,
            class: 1,
        },
        Syscall::SchedBind {
            sched: s(spare - 1),
            thread: s(slot),
        },
        Syscall::ThreadResume(s(slot)),
    ] {
        assert_eq!(sys.apply(node, root, call), DONE);
    }
    sys.node(node).object_at(root, s(slot)).unwrap()
}

#[test]
fn a_detach_whose_thread_is_destroyed_still_clears_the_context_entry() {
    // 3-node machine: core 0 fence=1, core 1 & 2 fence=2 -> node 1 holds IOMMU role 0.
    let (mut sys, t) = boot_n(3);
    assert_eq!(sys.role(0).holder(), 1);

    retype_done(&mut sys, 0, t[0], 3, IoSpace, 0, 100);
    // Attach device 0x10 and control device 0x11 via node 1.
    for dev in [0x10u64, 0x11u64] {
        let attach = Syscall::IoSpaceAttach {
            iommu: iommu_cap(3),
            iospace: s(100),
            device: dev,
        };
        assert_eq!(sys.apply(0, t[0], attach), WAITS);
        sys.settle(16);
        assert_eq!(sys.node(0).woken(t[0]), Some(&DONE));
    }
    assert_eq!(
        sys.table().units[&0].entries.get(&0x10),
        Some(&(0, sys.table().generation(0)))
    );
    assert_eq!(
        sys.table().units[&0].entries.get(&0x11),
        Some(&(0, sys.table().generation(0)))
    );

    let detacher = spawn_thread(&mut sys, 3, &t, 0, 200, 3000);
    let keeper = spawn_thread(&mut sys, 3, &t, 0, 201, 2997);

    // Node 1 starts handing IOMMU role 0 to node 2 -> role is Handing.
    assert!(matches!(
        sys.apply(
            1,
            t[1],
            Syscall::RoleHand {
                role: role0_cap(3),
                core: core(2),
            }
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(sys.role(0).state(), RoleState::Handing);

    // Both detacher (0x10) and keeper (0x11) detach while role 0 is Handing.
    assert_eq!(
        sys.apply(
            0,
            detacher,
            Syscall::IoSpaceAttach {
                iommu: iommu_cap(3),
                iospace: SlotAddr::NONE,
                device: 0x10,
            }
        ),
        WAITS
    );
    assert_eq!(
        sys.apply(
            0,
            keeper,
            Syscall::IoSpaceAttach {
                iommu: iommu_cap(3),
                iospace: SlotAddr::NONE,
                device: 0x11,
            }
        ),
        WAITS
    );
    assert!(sys.table().reports[0].is_empty());

    // Destroy detacher: unlink deletes its Request::Unit (`ipc.rs:709`).
    assert_eq!(sys.apply(0, t[0], Syscall::CapDelete(s(200))), DONE);

    // Finish hand-over; node 2 takes role 0 in recover(), then settle() delivers keeper's retry.
    sys.settle(32);
    sys.recover(16);
    sys.settle(32);
    assert_eq!(sys.role(0).state(), RoleState::Held);
    assert_eq!(sys.role(0).holder(), 2);

    // Control succeeded: 0x11's context entry was cleared.
    assert_eq!(sys.node(0).woken(keeper), Some(&DONE));
    assert_eq!(sys.table().units[&0].entries.get(&0x11), None);

    // The request of the destroyed thread completed without it: device
    // 0x10's context entry is cleared too, before the iospace goes.
    assert_eq!(sys.table().units[&0].entries.get(&0x10), None);
    assert_eq!(sys.apply(0, t[0], Syscall::CapDelete(s(100))), DONE);
    assert_eq!(sys.check(), Ok(()));
    assert!(sys.table().reports[0].is_empty());
}

#[test]
fn an_attach_whose_thread_is_destroyed_still_writes_the_context_entry() {
    let (mut sys, t) = boot_n(3);
    assert_eq!(sys.role(0).holder(), 1);
    retype_done(&mut sys, 0, t[0], 3, IoSpace, 0, 100);
    let attacher = spawn_thread(&mut sys, 3, &t, 0, 200, 3000);

    assert!(matches!(
        sys.apply(
            1,
            t[1],
            Syscall::RoleHand {
                role: role0_cap(3),
                core: core(2),
            }
        ),
        Ok(Output::Epoch(_))
    ));
    assert_eq!(sys.role(0).state(), RoleState::Handing);

    // Attacher starts IoSpaceAttach while role is Handing, then is destroyed.
    assert_eq!(
        sys.apply(
            0,
            attacher,
            Syscall::IoSpaceAttach {
                iommu: iommu_cap(3),
                iospace: s(100),
                device: 0x20,
            }
        ),
        WAITS
    );
    assert_eq!(sys.apply(0, t[0], Syscall::CapDelete(s(200))), DONE);

    // The hand-over ends; the request waiting for a live holder goes to it.
    sys.settle(32);
    sys.recover(16);
    sys.settle(32);
    assert_eq!(sys.role(0).holder(), 2);
    assert_eq!(sys.check(), Ok(()));
    // The request completed without its thread: node 0's report and the
    // unit's context entry agree.
    assert!(sys.table().reports[0].contains(&(0, 0x20)));
    assert_eq!(
        sys.table().units[&0].entries.get(&0x20),
        Some(&(0, sys.table().generation(0)))
    );
}
