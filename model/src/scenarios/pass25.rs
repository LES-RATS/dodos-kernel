//! Pass 25: the tables of the IOMMU units kept at start-up (IOMMU, Start-up
//! state, C7 of D-142): 1 MB and 4 KB per VT-d unit, among the kept ranges,
//! out of the ordinary memory, and a machine with no room for them refused.

use super::*;
use crate::boot::{IommuUnit, Refusal, RegionKind};
use dodos_abi::x86_64::VTD_UNIT_TABLE_BYTES;

/// The kept ranges of the machine table that are a unit's tables.
fn unit_tables(machine: &Machine) -> Vec<(u64, u64)> {
    let (model, _) = Model::boot(machine).expect("the machine boots");
    let bytes = VTD_UNIT_TABLE_BYTES.next_multiple_of(machine.geometry.page());
    let image = machine.image_base;
    let kept = model.table.kept.iter().copied();
    kept.filter(|&(s, e)| e - s == bytes && s != image)
        .collect()
}

/// The example with `units` VT-d units, no usable memory below 4 GB, and
/// `chunks` chunks of usable memory from 4 GB, the top two holding the
/// crash areas.
fn small(units: usize, chunks: u64) -> Machine {
    let mut machine = Machine::example();
    let chunk = machine.geometry.chunk();
    machine.memory[1].kind = RegionKind::Unusable;
    let ram = machine.memory[5].base;
    let end = machine.memory[6].base + machine.memory[6].len;
    machine.memory[5].len = chunks * chunk;
    machine.memory[6].base = ram + chunks * chunk;
    machine.memory[6].len = end - machine.memory[6].base;
    machine.crash = (ram + (chunks - 2) * chunk, Some(ram + (chunks - 1) * chunk));
    machine.iommus = vec![
        IommuUnit {
            segment: 0,
            domains: 256,
        };
        units
    ];
    machine
}

#[test]
fn the_tables_of_each_vtd_unit_are_kept_at_start_up_out_of_the_ordinary_memory() {
    let machine = Machine::example();
    let tables = unit_tables(&machine);
    assert_eq!(tables.len(), machine.iommus.len());
    let (model, _) = Model::boot(&machine).expect("the machine boots");
    let (chunk, page) = (machine.geometry.chunk(), machine.geometry.page());
    let none = dodos_abi::Ownership::none().raw();
    for &(start, end) in &tables {
        assert!(start.is_multiple_of(page));
        // In usable memory, clear of every other kept range.
        assert!(
            machine.memory.iter().any(|r| {
                r.kind == RegionKind::Usable && r.base <= start && end <= r.base + r.len
            })
        );
        let overlaps = |&(s, e): &(u64, u64)| s < end && start < e;
        let kept = model.table.kept.iter().filter(|r| overlaps(r)).count();
        assert_eq!(kept, 1);
        // In no chunk, no floor and no device entry.
        assert!((start / chunk..end.div_ceil(chunk)).all(|c| model.ownership(c) == none));
        assert!(!model.table.floors.iter().flatten().any(overlaps));
        let used = model.table.devices.iter().filter(|d| d.is_used());
        assert!(
            !used
                .map(|d| (d.base(), d.base() + (1 << d.log2())))
                .any(|r| overlaps(&r))
        );
    }
    // The example has room for them in its usable memory below 4 GB, which
    // holds no chunk: its ordinary memory has as many chunks as with no
    // unit.
    assert!(tables.iter().all(|&(_, end)| end <= 4 << 20));
    let words = 1u64 << (machine.phys_bits - machine.geometry.chunk_shift());
    let chunks = |model: &Model| (0..words).filter(|&c| model.ownership(c) != none).count();
    let mut no_unit = machine.clone();
    no_unit.iommus.clear();
    let (other, _) = Model::boot(&no_unit).expect("the machine boots");
    assert_eq!(chunks(&model), chunks(&other));
}

#[test]
fn every_vtd_unit_has_tables_of_its_own() {
    let mut machine = Machine::example();
    machine.iommus.push(IommuUnit {
        segment: 1,
        domains: 256,
    });
    let tables = unit_tables(&machine);
    assert_eq!(tables.len(), 2);
    assert!(tables[0].1 <= tables[1].0 || tables[1].1 <= tables[0].0);
}

#[test]
fn a_machine_with_no_room_for_the_tables_of_its_units_is_refused() {
    // The fewest chunks with which the machine boots with no unit.
    let fewest = (3..64)
        .find(|&chunks| Model::boot(&small(0, chunks)).is_ok())
        .expect("a machine that boots");
    // A unit's tables take a chunk out of the ordinary memory there: the
    // floors no longer fit, refusal 4 (Start-up state: "the floors, the
    // reserves and the kept memory do not fit in the ordinary memory").
    assert_eq!(Model::boot(&small(1, fewest)).err(), Some(Refusal::Floors));
    let one_more = small(1, fewest + 1);
    assert_eq!(unit_tables(&one_more).len(), 1);
}

#[test]
fn arm64_and_riscv_keep_no_vtd_tables() {
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let mut machine = Machine::example();
        machine.arch = arch;
        assert!(unit_tables(&machine).is_empty(), "{arch:?}");
    }
}

// The combined exploration's seed 8 (`combinations.rs`): a take-over's
// rebuild ends with an invalidation of the whole unit; on a unit that never
// confirms it, the rebuild never ended, and a request it kept was never
// answered. At `IOMMU_WAIT_NS` the unit is failed, and the requests are
// answered *failed* (IOMMU; Roles, take-over after a death, D-131). A
// rebuild with a working unit ends within one event, so the test takes the
// role over, then puts the new holder at that last step with a unit that
// stops confirming.
#[test]
fn a_request_kept_by_a_rebuild_whose_unit_never_confirms_is_answered_failed() {
    use super::units::{IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    attached(&mut m, 2);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    m.ok(0, super::pass17::kill(1));
    m.run(200 * MS);
    let holder = m.sys.role(IOMMU_ROLE).holder();
    assert!(holder != 1 && m.sys.node(holder).rebuilds.is_empty());
    m.sys.break_unit(IOMMU_ROLE);
    let node = m.sys.node_mut(holder);
    let serial = node.submit(
        IOMMU_ROLE,
        crate::rings::UnitOp::Invalidate { domain: None },
    );
    let closing = Some((serial, node.membership_now()));
    let rebuild = crate::units::Rebuild {
        node: 3,
        closing,
        ..Default::default()
    };
    node.rebuilds.insert(IOMMU_ROLE, rebuild);
    // The unit's queue is in the machine table, which every kernel reads.
    let table = node.table.clone();
    m.sys.edit_table(|t| *t = table);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.run(10 * dodos_abi::IOMMU_WAIT_NS);
    assert!(
        m.sys.node(holder).rebuilds.is_empty(),
        "the rebuild never ended"
    );
    assert!(m.sys.role(IOMMU_ROLE).failed());
    assert_eq!(m.woken(0), Some(&Err(Error::Busy)));
}

// CONTRACT-GAP 18, the combined exploration's seed 20: a request answered
// *elsewhere* during steps 1 to 3 of a hand-over is kept queued while the
// role word reads the epoch it was sent with (Roles, D-141). A hand-over
// that ends without step 4, here because the unit fails during the drain
// (`role_hand` returns `busy`), leaves the word held by the same node with
// the same epoch: the request is never sent again and its thread waits for
// ever. The test holds the reading the model implements until the
// contract settles it.
#[test]
fn gap_18_a_request_answered_elsewhere_by_a_hand_over_that_ends_without_step_4_stays_queued() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 1);
    attached(&mut m, 2);
    let epoch = m.sys.role(IOMMU_ROLE).epoch();
    m.sys.break_unit(IOMMU_ROLE);
    // A request node 1 has begun: node 0's attach, submitted to the unit,
    // which never confirms it.
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(2),
    };
    // Step 1: node 1 stops serving; the drain waits for the unit.
    assert_eq!(m.call(1, hand), WAITS);
    let detach = super::pass17::detach_of(attach(&m, 2, IOSPACE, DEVICE));
    assert_eq!(m.call(2, detach), WAITS);
    let elsewhere = |m: &S| {
        m.sys
            .node(2)
            .unit_requests
            .iter()
            .flatten()
            .any(|e| e.elsewhere)
    };
    assert!(m.step_until(elsewhere), "never answered elsewhere");
    m.run(10 * dodos_abi::IOMMU_WAIT_NS);
    assert_eq!(m.woken(1), Some(&Err(Error::Busy)));
    let word = m.sys.role(IOMMU_ROLE);
    assert!(word.failed() && word.holder() == 1 && word.epoch() == epoch);
    // The request is still queued, and node 2's thread still waits.
    assert!(elsewhere(&m));
    assert!(matches!(
        m.sys.node(2).threads[&m.t[2]].wait,
        crate::Wait::Unit(_)
    ));
}

// The combined exploration's seeds 12 and 20: a requester answered
// *elsewhere* by a node in steps 1 to 3 of a hand-over sent its request
// again at once, to the same node with the same epoch, which answered
// *elsewhere* again, at the same instant, for ever. It keeps the request
// queued while the word reads that epoch and sends it once the word does
// (Roles, D-141): here to the target, once it holds the role.
#[test]
fn a_request_answered_elsewhere_is_sent_again_only_once_the_epoch_changes() {
    use super::units::{DEVICE, IOMMU_ROLE, IOSPACE, attach, attached, three};
    let mut m = three();
    attached(&mut m, 2);
    let set_broken = |m: &mut S, broken: bool| {
        for n in 0..3 {
            let unit = m.sys.node_mut(n).table.units.entry(IOMMU_ROLE);
            unit.or_default().broken = broken;
        }
    };
    set_broken(&mut m, true);
    let iospace = m.retype(0, ObjectKind::IoSpace, 0, IOSPACE);
    m.ok(0, iospace);
    let call = attach(&m, 0, IOSPACE, 0x11);
    assert_eq!(m.call(0, call), WAITS);
    m.settle();
    let roles = slots_of(m.sys.node(1), m.t[1], ObjectKind::Role);
    let hand = Syscall::RoleHand {
        role: s(roles[IOMMU_ROLE]),
        node: node_cap(2),
    };
    assert_eq!(m.call(1, hand), WAITS);
    let detach = super::pass17::detach_of(attach(&m, 2, IOSPACE, DEVICE));
    assert_eq!(m.call(2, detach), WAITS);
    let elsewhere = |m: &S| {
        m.sys
            .node(2)
            .unit_requests
            .iter()
            .flatten()
            .any(|e| e.elsewhere)
    };
    assert!(m.step_until(elsewhere));
    let sent = |m: &S| m.sys.canonical().rings[2][1].kernel.produced;
    let before = sent(&m);
    m.run(dodos_abi::IOMMU_WAIT_NS / 2);
    assert_eq!(
        sent(&m),
        before,
        "the request was sent again at the same epoch"
    );
    // The unit drains: the hand-over completes, node 2 takes the role and
    // serves its own request.
    set_broken(&mut m, false);
    m.run(50 * MS);
    assert_eq!(m.sys.role(IOMMU_ROLE).holder(), 2);
    assert_eq!(m.woken(2), Some(&DONE));
}

// The combined exploration's seed 47: the observation of a core, recorded
// for one generation of a node, accused its next generation, started on
// the same core while the observation lived: the heartbeat is a count of
// the events of the core's kernel, which starts again with the node, and
// it read the value observed again. Evidence stale is an observation of
// "a core whose word names X's generation" (Cores, Detection): it judges
// that generation only.
#[test]
fn an_observation_of_a_generation_that_ended_never_accuses_the_next() {
    use crate::table::{Observation, Watched};
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.sys.fail(2, 0);
    m.run(100 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    let old = m.sys.member(2).generation();
    m.ok(1, super::pass17::fresh(2, 1 << 2));
    m.run(5 * MS);
    assert_eq!(m.state(2), MemberState::Live);
    // Node 1's observation of core 2 for the old generation, older than
    // STALE_NS, whose heartbeat the new generation's core reads again.
    let (now, stale) = (m.sys.now(), m.sys.canonical().stale_ns);
    let heartbeat = m.sys.canonical().core_beacons[2].heartbeat.0;
    m.sys.edit_table(|t| {
        t.core_beacons[2].idle_until.0 = 0;
        t.beacons[1].observations.push(Observation {
            node: 2,
            generation: old,
            watched: Watched::Heartbeat { core: 2, heartbeat },
            at: now - stale,
        });
    });
    assert_eq!(m.sys.node(1).evidence(2), None);
}

// The combined exploration's seed 7: a node started afresh built its state
// with its clock at 0, so its first observations of the other nodes were
// stamped at instant 0, older than `STALE_NS`, and its first look accused
// every healthy node it observed with evidence stale. The clock is the
// machine's (Scheduling and time, Clock; Cores, Detection: observations
// "stamped with the clock"). Seen event by event, the start also ended
// with the first thread ready on a core that ran nothing and had no
// reschedule pending (invariant 46).
#[test]
fn a_node_started_afresh_stamps_its_observations_with_the_machines_clock() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.run(dodos_abi::SWEEP_NS / 2);
    m.sys.fail(2, 0);
    m.run(100 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    let started = m.sys.now();
    let old = m.sys.member(2).generation();
    m.ok(1, super::pass17::fresh(2, 1 << 2));
    // The start's events, at the instant they are due: the new kernel
    // reads the machine's clock from the event that builds it.
    let built = |m: &S| m.sys.has(2) && m.sys.node(2).generation_built() == old + 1;
    assert!(m.step_until(built));
    assert_eq!(m.sys.node(2).now, m.sys.now());
    assert!(m.step_until(|m| m.state(2) == MemberState::Live));
    m.run(dodos_abi::SWEEP_NS);
    assert!(
        m.sys.canonical().beacons[2]
            .observations
            .iter()
            .all(|o| o.at >= started),
        "{:?}",
        m.sys.canonical().beacons[2].observations
    );
    for n in 0..3 {
        assert_eq!(
            m.state(n),
            MemberState::Live,
            "node {n}: {:?}",
            m.sys.member(n)
        );
    }
}

// The combined exploration's seed 131: each kernel counted its core's
// heartbeat from 0, so a node started afresh on a core wrote again values
// the core's previous kernel had written. Another node observed the new
// generation at its start with the old kernel's last value, and the new
// kernel's count reached that value past `STALE_NS`: a node that beat was
// accused stale. The heartbeat is the core beacon's counter, which the
// next kernel of the core goes on from (Cores, Beacons).
#[test]
fn a_core_heartbeat_goes_on_across_the_generations_of_its_node() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    m.run(200 * MS);
    m.sys.fail(2, 0);
    m.run(100 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    let last = m.sys.canonical().core_beacons[2].heartbeat.0;
    assert!(last > 1);
    m.ok(1, super::pass17::fresh(2, 1 << 2));
    let beat =
        |m: &S| m.state(2) == MemberState::Live && m.sys.canonical().core_beacons[2].ran.1 == 2;
    assert!(m.step_until(beat));
    m.run(MS);
    assert!(m.sys.canonical().core_beacons[2].heartbeat.0 > last);
}

// The combined exploration's seed 164: the header of a core's debug area
// counts the kernel entries suppressed by the rate limit since machine
// start (Debugging, the trace rings); a kernel started afresh on the core
// counted from 0 beside a header that held its predecessor's, and the
// check of what is counted once failed (invariant 91).
#[test]
fn a_node_started_afresh_counts_the_suppressed_entries_of_its_cores_earlier_kernels() {
    let mut m = S::on(&machine(Arch::X86_64, 3, &[&[0], &[1], &[2]]));
    // Node 2's kernel is rate limited: 20 entries of one code in a window.
    let now = m.sys.now();
    m.sys.node_mut(2).set_now(now);
    for _ in 0..20 {
        m.sys
            .node_mut(2)
            .refuse_trace(0, dodos_abi::NodeMessage::Send, 0);
    }
    let table = m.sys.node(2).table.clone();
    m.sys.edit_table(|t| *t = table);
    assert!(m.sys.canonical().crash.areas[2].suppressed > 0);
    m.sys.fail(2, 0);
    m.run(100 * MS);
    assert_eq!(m.state(2), MemberState::Dead);
    m.ok(1, super::pass17::fresh(2, 1 << 2));
    m.run(5 * MS);
    assert_eq!(m.state(2), MemberState::Live);
}
