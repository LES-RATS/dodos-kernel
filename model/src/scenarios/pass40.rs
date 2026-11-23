//! Pass 40: D-168 (the sweep's cyclic cursor over sealed nodes) and the final
//! review's fixes on the model.

use super::*;
use dodos_abi::{Evidence, NODE_STOP_SEAL, SEAL_NS, SWEEP_NS};

fn seal(target: u16) -> Syscall {
    Syscall::NodeStop {
        node: node_cap(target),
        mode: NODE_STOP_SEAL,
        core: 0,
    }
}

/// D-168 (1): with one live node and three sealed nodes, each sweep reads
/// the seal instant of two sealed nodes from its cyclic cursor, so every
/// sealed node — including the third — is accused with evidence seal within
/// `ceil(s / 2) * SWEEP_NS + SEAL_NS`.
#[test]
fn sweep_cursor_accuses_third_sealed_node_with_evidence_seal() {
    let mut m = S::on(&machine(Arch::X86_64, 4, &[&[0], &[1], &[2], &[3]]));
    // Advance briefly after boot's initial sweep at t = 0 (next_sweep = SWEEP_NS),
    // then seal nodes 1, 2 and 3 so only node 0 stays live.
    m.run(10 * MS);
    for target in [1, 2, 3] {
        m.ok(0, seal(target));
    }
    m.run(10 * MS);
    for target in [1, 2, 3] {
        assert_eq!(m.state(target), MemberState::Sealed);
    }

    // Sweep 1 at SWEEP_NS happens before SEAL_NS has elapsed for the three
    // sealed nodes (sealed around t = 10 ms): it reads [1, 2] and advances the
    // cursor to 2, accusing none. Even past sealed_at + SEAL_NS (at t = 1.5 s),
    // no accusation happens until the next sweep.
    m.run(SWEEP_NS + 500 * MS - 20 * MS);
    for target in [1, 2, 3] {
        assert_eq!(
            m.state(target),
            MemberState::Sealed,
            "node {target} must stay sealed until a sweep reads it"
        );
    }

    // Sweep 2 at 2 * SWEEP_NS reads [3, 1] from cursor 2 (advancing cursor to
    // 1): node 3 (the third sealed node) and node 1 are accused with evidence
    // seal, while node 2 stays sealed because at most two are read per sweep.
    m.run(500 * MS + 20 * MS);
    assert_eq!(m.sys.member(3).evidence(), Evidence::Seal);
    assert_ne!(m.state(3), MemberState::Sealed);
    assert_eq!(m.sys.member(1).evidence(), Evidence::Seal);
    assert_ne!(m.state(1), MemberState::Sealed);
    assert_eq!(
        m.state(2),
        MemberState::Sealed,
        "sweep 2 reads only two sealed nodes ([3, 1]), leaving node 2 for sweep 3"
    );

    // Sweep 3 at 3 * SWEEP_NS reads the remaining sealed node (node 2) and
    // accuses it with evidence seal too.
    m.run(SWEEP_NS);
    assert_eq!(m.sys.member(2).evidence(), Evidence::Seal);
    assert_ne!(m.state(2), MemberState::Sealed);
    // All three were accused within ceil(3 / 2) * SWEEP_NS + SEAL_NS.
    assert!(m.sys.now() <= 2 * SWEEP_NS + SEAL_NS + 50 * MS);
}

/// Pass 40 (v1-E #1): `check_granted` must require `OwnState::Taken` and the
/// granter's current generation on a reserve chunk; a `Free` reserve chunk
/// belonging to a reserve the granter holds must be reported under G5.
#[test]
fn granted_free_reserve_chunk_is_reported() {
    let g = super::pass24::granted(false);
    let mut m = g.m;
    let granter = super::pass24::X;
    m.supervisor(granter, m.t[usize::from(granter)]);
    let t0 = &m.sys.node(0).table;
    let reserve = (0..16u16)
        .find(|&j| t0.holds_reserve(granter, j))
        .expect("granter holds a reserve role");
    for n in 0..3 {
        m.sys.make_faulty(n);
    }
    m.sys.edit_table(|t| {
        let free_reserve = dodos_abi::Ownership::free(dodos_abi::Owner::Reserve(reserve));
        std::sync::Arc::make_mut(&mut t.owners)[g.chunk as usize] = free_reserve.raw();
    });
    assert_eq!(
        m.sys.check().expect_err("free reserve chunk under grant"),
        "G5: granted memory owned by another than its granter"
    );
}

/// Pass 40 (v1-E #2): `check_granted` must verify ownership of a granted
/// device entry; a live granter holding a grant over a device entry it does
/// not own must be reported under G5.
#[test]
fn granted_device_owned_by_another_is_reported() {
    let g = super::pass24::granted(false);
    let mut m = g.m;
    let granter = super::pass24::X;
    for n in 0..3 {
        m.sys.make_faulty(n);
    }
    m.sys.edit_table(|t| {
        let dev_base = t.devices[0].base().expect("used device");
        let entry = t.grants[usize::from(granter)]
            .values_mut()
            .next()
            .expect("a grant entry");
        entry.anchor.frame.base = dev_base;
        entry.anchor.frame.pages = 1;
        let cleared = t.gate_word(g.chunk).with_grants(0).unwrap();
        std::sync::Arc::make_mut(&mut t.gate_words)[g.chunk as usize] = cleared.raw();
        std::sync::Arc::make_mut(&mut t.device_grants)[0] = 1;
        // Device 0 is free (not taken by granter).
        std::sync::Arc::make_mut(&mut t.devices)[0].owner =
            dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed);
    });
    assert_eq!(
        m.sys.check().expect_err("unowned device grant"),
        "G5: granted device owned by another than its granter"
    );
}

/// Pass 40 (v2-C #1): `hand_all_units` must skip phantom nodes (live slots
/// with no cores) and hand unit roles to the next real live successor.
#[test]
fn seal_hands_unit_role_past_phantom_to_real_successor() {
    // Three node slots: node 0 (core 0), node 1 (core 1), node 2 (phantom: live
    // with no cores). Equal fence class so cyclic order after node 1 visits
    // phantom node 2 before real node 0.
    let mut mach = machine(Arch::X86_64, 3, &[&[0], &[1]]);
    mach.cores[0].fence = 2;
    mach.cores[1].fence = 2;
    mach.boot_nodes = Some((3, vec![(0, 1), (0, 2)]));
    let mut m = S::on(&mach);
    m.sys.edit_table(|t| {
        t.members[2] =
            dodos_abi::Member::new(MemberState::Live, Evidence::None, dodos_abi::DRIVER_NONE, 1)
                .with_class(2);
    });
    assert!(m.sys.node(0).real(0));
    assert!(m.sys.node(0).real(1));
    assert!(!m.sys.node(0).real(2));
    assert_eq!(m.state(2), MemberState::Live);

    let unit_role = m
        .sys
        .node(0)
        .table
        .role_records
        .iter()
        .position(|r| r.kind == dodos_abi::RoleKind::Unit)
        .expect("a unit role");
    // Move the unit role onto node 1 so that its cyclic successor is phantom 2.
    if m.sys.role(unit_role).holder() != 1 {
        let holder = m.sys.role(unit_role).holder();
        let roles = slots_of(
            m.sys.node(holder),
            m.t[usize::from(holder)],
            ObjectKind::Role,
        );
        assert_eq!(
            m.call(
                holder,
                Syscall::RoleHand {
                    role: s(roles[unit_role]),
                    node: node_cap(1),
                },
            ),
            WAITS
        );
        m.run(20 * MS);
    }
    assert_eq!(m.sys.role(unit_role).holder(), 1);

    // Seal node 1: `dodos_abi::successor` would pick phantom node 2;
    // `hand_all_units` must skip the phantom and hand the unit role to node 0.
    m.ok(1, seal(1));
    m.run(20 * MS);

    assert_eq!(m.state(1), MemberState::Sealed);
    let word = m.sys.role(unit_role);
    assert_eq!(word.state(), dodos_abi::RoleState::Held);
    assert_eq!(
        word.holder(),
        0,
        "unit role must be handed to real node 0, not phantom node 2"
    );
}

// v2-B B-1: destroying a revoked task must wait while any of its threads is
// still leaving another core, just as `task_revoke` does.
#[test]
fn destroying_revoked_task_waits_for_thread_leaving_other_core() {
    let (mut k, other) = super::cores::two_threads(Arch::X86_64, 2);
    k.ok(k.retype(ObjectKind::Task, 0, 310));
    k.ok(Syscall::TaskAttach {
        thread: s(300),
        task: s(310),
    });
    assert_eq!(k.m.cores[1].thread, Some(other));
    // First call sets `revoked` and marks `other` leaving on core 1.
    assert_eq!(
        k.m.apply(k.t, Syscall::TaskRevoke(s(310))),
        Err(Error::Again)
    );
    assert!(k.m.threads[&other].leaving);
    // Deleting the task before core 1 handles the leave IPI must return
    // `Again` without suspending or detaching `other`.
    assert_eq!(
        k.m.apply(k.t, Syscall::CapDelete(s(310))),
        Err(Error::Again)
    );
    assert_eq!(k.m.check(), Ok(()));
    assert!(!k.m.threads[&other].suspended);
    assert!(k.m.threads[&other].task.is_some());
    // Once core 1 leaves `other`, deleting the task finishes cleanly.
    k.interrupts();
    k.ok(Syscall::CapDelete(s(310)));
    assert_eq!(k.m.check(), Ok(()));
}

// v2-B B-4: `end_locks` uses the per-event `table.gated` flag rather than
// the cross-event write journal, so an unchecked gate write does not cause a
// subsequent non-gate syscall to record `Lock(Gate)`.
#[test]
fn unchecked_gate_write_does_not_mark_next_event_as_taking_gate_lock() {
    let (mut k, _) = super::cores::two_threads(Arch::X86_64, 2);
    // Perform a gate write without calling `k.m.check()` afterward.
    let chunk = k
        .m
        .table
        .owners
        .iter()
        .position(|&raw| {
            dodos_abi::Ownership::from_raw(raw).is_some_and(|o| {
                o.state() == dodos_abi::OwnState::Free && o.owner() == dodos_abi::Owner::Unclaimed
            })
        })
        .expect("a free unclaimed chunk") as u64;
    let base = chunk * k.m.table.geometry.chunk();
    let take = Syscall::MemoryTake {
        control: s(dodos_abi::START_SLOT_MEMORY_CONTROL),
        kind: dodos_abi::MEMORY_TAKE_RUN,
        place: base,
        size: k.m.table.geometry.chunk_shift() as u64,
        destination: s(320),
    };
    assert_eq!(k.m.apply(k.t, take), DONE);
    assert!(!k.m.table.writes.is_empty());
    assert!(
        k.m.locks
            .steps
            .contains(&crate::locks::Step::Lock(crate::locks::LockId::Gate))
    );
    // Next event makes no gate call: must NOT record `Lock(Gate)` even
    // though `table.writes` was never cleared by `check()`.
    assert_eq!(k.m.apply(k.t, Syscall::Yield), DONE);
    assert!(
        !k.m.locks
            .steps
            .contains(&crate::locks::Step::Lock(crate::locks::LockId::Gate)),
        "Yield recorded Lock(Gate) from an earlier unchecked event"
    );
    assert_eq!(k.m.check(), Ok(()));
}

// v2-C #4: `take_over_due` must not allow immediate take-over when the fence
// record does not confirm the judged generation.
#[test]
fn take_over_waits_when_fence_record_missing_for_fenced_holder() {
    let mut m = S::on(&machine(Arch::X86_64, 2, &[&[0], &[1]]));
    let unit_role = m
        .sys
        .node(0)
        .table
        .role_records
        .iter()
        .position(|r| r.kind == dodos_abi::RoleKind::Unit)
        .expect("a unit role");
    let holder = m.sys.role(unit_role).holder();
    let other = 1 - holder;
    // Mark `holder` Fenced at generation 1 without any matching fence record.
    m.sys.edit_table(|t| {
        t.members[usize::from(holder)] =
            dodos_abi::Member::new(MemberState::Fenced, Evidence::None, other, 1).with_class(2);
        t.fences[usize::from(holder)] = crate::table::FenceRecord::default();
    });
    // Advance well past `SETTLE_NS`: `other` must not take over without a
    // matching confirmed fence record.
    m.sys.advance(10 * MS);
    assert_eq!(
        m.sys.role(unit_role).holder(),
        holder,
        "unit role was taken over without a confirmed fence record"
    );
}

// v2-C #7: boot refuses an IOMMU unit whose domain count exceeds u16 range.
#[test]
fn boot_refuses_iommu_with_more_than_65536_domains() {
    let mut mach = machine(Arch::X86_64, 2, &[&[0], &[1]]);
    mach.iommus[0].domains = 70_000;
    assert_eq!(
        crate::Model::boot(&mach).err(),
        Some(crate::Refusal::MachineValues)
    );
}
