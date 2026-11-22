//! Harness instrumentation for contract bounds (pass 43).
//!
//! Measures the highest value reached and whether the refusal/wait/boundary
//! path was taken for each bound of `kernel-interface.md`. Lives strictly in
//! the test harness: no model logic is altered.

use crate::rings::Body;
use crate::system::System;
use crate::table::{RingEntry, Table};
use crate::{Model, Outcome, Record, Syscall};
use dodos_abi::{
    CrashReason, Error, Evidence, IDLE_ENTRY_WORDS, IDLE_STATES_MAX, IOSPACE_MAX_DEVICES, MAX_CAPS,
    MAX_WORDS, MemberState, NODE_WATCH_SLOTS, OBSERVATIONS_MAX, PENDING_STEP, REPORT_END,
    REQUESTS_PER_PEER, RING_ANSWER_RESERVE, RING_KERNEL_ENTRIES, RING_USER_ENTRIES, RING_USER_STEP,
    RoleKind, SENDS_PER_PEER, SENDS_RESERVED_FOR_MANAGE, TIME_EVENT_STEP,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Shared bounds accumulated across all scenario tests in the lib test binary.
static SCENARIO_BOUNDS: OnceLock<Mutex<Bounds>> = OnceLock::new();
static ACTIVE_HARNESSES: AtomicUsize = AtomicUsize::new(0);

/// RAII guard tracking active `N` / `S` harnesses so the summary test can wait
/// for concurrent scenario tests to finish when `DODOS_BOUNDS` is set.
#[derive(Debug)]
pub(crate) struct Active(());

impl Active {
    pub(crate) fn enter() -> Self {
        ACTIVE_HARNESSES.fetch_add(1, Ordering::Relaxed);
        Self(())
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE_HARNESSES.fetch_sub(1, Ordering::Release);
    }
}

/// Waits briefly until all concurrent scenario harnesses have finished.
pub(crate) fn wait_for_active() {
    let mut idle = 0u32;
    for _ in 0..6_000 {
        if ACTIVE_HARNESSES.load(Ordering::Acquire) == 0 {
            idle += 1;
            if idle >= 20 {
                break;
            }
        } else {
            idle = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Merges `bounds` into the test-binary-wide scenario accumulator.
pub(crate) fn record_scenario(bounds: &Bounds) {
    let lock = SCENARIO_BOUNDS.get_or_init(|| Mutex::new(Bounds::default()));
    if let Ok(mut guard) = lock.lock() {
        guard.merge(bounds);
    }
}

/// Snapshot of the test-binary-wide scenario accumulator.
pub(crate) fn scenario_snapshot() -> Bounds {
    let lock = SCENARIO_BOUNDS.get_or_init(|| Mutex::new(Bounds::default()));
    lock.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Per-`System` cursor so each ring message is scanned at most once and
/// rebuild asks per `(holder, role, epoch, peer)` are counted accurately.
#[derive(Default)]
pub(crate) struct RingScan {
    seen_kernel: Vec<Vec<u64>>,
    seen_user: Vec<Vec<u64>>,
    rebuild_asks: BTreeMap<(u16, u16, u32, u16), usize>,
}

impl RingScan {
    fn ensure(&mut self, nodes: usize) {
        if self.seen_kernel.len() != nodes {
            self.seen_kernel = vec![vec![0; nodes]; nodes];
            self.seen_user = vec![vec![0; nodes]; nodes];
        }
    }
}

/// High-water marks and boundary/refusal hits for every contract bound.
#[derive(Clone, Debug, Default)]
pub(crate) struct Bounds {
    // #### Numbers table
    pub(crate) max_nodes: usize,
    pub(crate) max_cores_per_node: usize,
    pub(crate) stale_accused: bool,
    pub(crate) stale_starting_accused: bool,
    pub(crate) stuck_accused: bool,
    pub(crate) rebuild_stuck_cut: bool,
    pub(crate) sweep_fired: bool,
    pub(crate) fence_timeout_unfenced: bool,
    pub(crate) settle_waited: bool,
    pub(crate) start_timeout: bool,
    pub(crate) start_retried: bool,
    pub(crate) seal_accused: bool,
    pub(crate) core_answer_timeout: bool,
    pub(crate) max_core0_admitted: u64,
    pub(crate) recovery_period_waited: bool,
    pub(crate) admission_refused: bool,
    pub(crate) max_watch_slots: usize,
    pub(crate) watch_slot_5th_refused: bool,
    pub(crate) max_idle_states: usize,
    pub(crate) idle_16th_refused: bool,
    pub(crate) max_observations: usize,
    pub(crate) obs_9th_dropped: bool,
    pub(crate) max_reserves: usize,
    pub(crate) max_plain_roles: usize,
    pub(crate) max_kernel_ring_used: u64,
    pub(crate) kernel_ring_hit_reserve: bool,
    pub(crate) kernel_ring_used_reserve: bool,
    pub(crate) kernel_ring_full: bool,
    pub(crate) max_user_ring_used: u64,
    pub(crate) user_ring_full: bool,
    pub(crate) max_req_credits: usize,
    pub(crate) req_credit_17th_refused: bool,
    pub(crate) max_send_credits: usize,
    pub(crate) send_credit_7th_no_manage_refused: bool,
    pub(crate) send_credit_9th_refused: bool,
    pub(crate) max_unit_requests: usize,
    pub(crate) unit_requests_65th_refused: bool,
    pub(crate) max_unit_service: usize,

    // Bounds stated in the text
    pub(crate) max_report_items: usize,
    pub(crate) max_ring_msg_entries: usize,
    pub(crate) report_28_more: bool,
    pub(crate) report_bad_rejected: bool,
    pub(crate) max_node_handles: usize,
    pub(crate) max_rebuild_asks_per_node: usize,
    pub(crate) irq_193rd_refused: bool,
    pub(crate) removal_32_again: bool,
    pub(crate) removal_unmapping_stop: bool,
    pub(crate) removal_128_slots_again: bool,
    pub(crate) max_cspace_slots: u32,
    pub(crate) max_cspace_grown_slots: usize,
    pub(crate) cspace_over_max_refused: bool,
    pub(crate) max_sched_debt: u64,
    pub(crate) sched_debt_forgiven: bool,
    pub(crate) max_sealed_nodes: usize,
    pub(crate) sweep_2_sealed_capped: bool,
    pub(crate) max_msg_words: usize,
    pub(crate) msg_words_over_64_refused: bool,

    // Additional text/ABI bounds
    pub(crate) max_send_grants: usize,
    pub(crate) send_grants_over_4_refused: bool,
    pub(crate) user_ring_step_16: bool,
    pub(crate) max_pending_signals: usize,
    pub(crate) max_pending_releases: usize,
    pub(crate) pending_step_32: bool,
    pub(crate) max_iospace_devices: usize,
    pub(crate) iospace_17th_refused: bool,
    pub(crate) iommu_wait_timeout: bool,
    pub(crate) max_tree_depth: usize,
    pub(crate) max_walk: usize,
    pub(crate) tree_depth_65_refused: bool,
    pub(crate) max_grants: usize,
    pub(crate) max_uses: usize,
    pub(crate) grants_full_refused: bool,
    pub(crate) uses_full_dropped: bool,
    pub(crate) max_due_timers: usize,
    pub(crate) time_event_step_16: bool,
    pub(crate) clear_step_again: bool,
    pub(crate) max_debug_write: u64,
    pub(crate) debug_write_over_512_refused: bool,
    pub(crate) trace_burst_suppressed: bool,
    pub(crate) max_entropy_bytes: u64,
    pub(crate) entropy_over_64_refused: bool,
    pub(crate) map_count_max_refused: bool,
}

impl Bounds {
    pub(crate) fn merge(&mut self, o: &Self) {
        self.max_nodes = self.max_nodes.max(o.max_nodes);
        self.max_cores_per_node = self.max_cores_per_node.max(o.max_cores_per_node);
        self.stale_accused |= o.stale_accused;
        self.stale_starting_accused |= o.stale_starting_accused;
        self.stuck_accused |= o.stuck_accused;
        self.rebuild_stuck_cut |= o.rebuild_stuck_cut;
        self.sweep_fired |= o.sweep_fired;
        self.fence_timeout_unfenced |= o.fence_timeout_unfenced;
        self.settle_waited |= o.settle_waited;
        self.start_timeout |= o.start_timeout;
        self.start_retried |= o.start_retried;
        self.seal_accused |= o.seal_accused;
        self.core_answer_timeout |= o.core_answer_timeout;
        self.max_core0_admitted = self.max_core0_admitted.max(o.max_core0_admitted);
        self.recovery_period_waited |= o.recovery_period_waited;
        self.admission_refused |= o.admission_refused;
        self.max_watch_slots = self.max_watch_slots.max(o.max_watch_slots);
        self.watch_slot_5th_refused |= o.watch_slot_5th_refused;
        self.max_idle_states = self.max_idle_states.max(o.max_idle_states);
        self.idle_16th_refused |= o.idle_16th_refused;
        self.max_observations = self.max_observations.max(o.max_observations);
        self.obs_9th_dropped |= o.obs_9th_dropped;
        self.max_reserves = self.max_reserves.max(o.max_reserves);
        self.max_plain_roles = self.max_plain_roles.max(o.max_plain_roles);
        self.max_kernel_ring_used = self.max_kernel_ring_used.max(o.max_kernel_ring_used);
        self.kernel_ring_hit_reserve |= o.kernel_ring_hit_reserve;
        self.kernel_ring_used_reserve |= o.kernel_ring_used_reserve;
        self.kernel_ring_full |= o.kernel_ring_full;
        self.max_user_ring_used = self.max_user_ring_used.max(o.max_user_ring_used);
        self.user_ring_full |= o.user_ring_full;
        self.max_req_credits = self.max_req_credits.max(o.max_req_credits);
        self.req_credit_17th_refused |= o.req_credit_17th_refused;
        self.max_send_credits = self.max_send_credits.max(o.max_send_credits);
        self.send_credit_7th_no_manage_refused |= o.send_credit_7th_no_manage_refused;
        self.send_credit_9th_refused |= o.send_credit_9th_refused;
        self.max_unit_requests = self.max_unit_requests.max(o.max_unit_requests);
        self.unit_requests_65th_refused |= o.unit_requests_65th_refused;
        self.max_unit_service = self.max_unit_service.max(o.max_unit_service);
        self.max_report_items = self.max_report_items.max(o.max_report_items);
        self.max_ring_msg_entries = self.max_ring_msg_entries.max(o.max_ring_msg_entries);
        self.report_28_more |= o.report_28_more;
        self.report_bad_rejected |= o.report_bad_rejected;
        self.max_node_handles = self.max_node_handles.max(o.max_node_handles);
        self.max_rebuild_asks_per_node = self
            .max_rebuild_asks_per_node
            .max(o.max_rebuild_asks_per_node);
        self.irq_193rd_refused |= o.irq_193rd_refused;
        self.removal_32_again |= o.removal_32_again;
        self.removal_unmapping_stop |= o.removal_unmapping_stop;
        self.removal_128_slots_again |= o.removal_128_slots_again;
        self.max_cspace_slots = self.max_cspace_slots.max(o.max_cspace_slots);
        self.max_cspace_grown_slots = self.max_cspace_grown_slots.max(o.max_cspace_grown_slots);
        self.cspace_over_max_refused |= o.cspace_over_max_refused;
        self.max_sched_debt = self.max_sched_debt.max(o.max_sched_debt);
        self.sched_debt_forgiven |= o.sched_debt_forgiven;
        self.max_sealed_nodes = self.max_sealed_nodes.max(o.max_sealed_nodes);
        self.sweep_2_sealed_capped |= o.sweep_2_sealed_capped;
        self.max_msg_words = self.max_msg_words.max(o.max_msg_words);
        self.msg_words_over_64_refused |= o.msg_words_over_64_refused;
        self.max_send_grants = self.max_send_grants.max(o.max_send_grants);
        self.send_grants_over_4_refused |= o.send_grants_over_4_refused;
        self.user_ring_step_16 |= o.user_ring_step_16;
        self.max_pending_signals = self.max_pending_signals.max(o.max_pending_signals);
        self.max_pending_releases = self.max_pending_releases.max(o.max_pending_releases);
        self.pending_step_32 |= o.pending_step_32;
        self.max_iospace_devices = self.max_iospace_devices.max(o.max_iospace_devices);
        self.iospace_17th_refused |= o.iospace_17th_refused;
        self.iommu_wait_timeout |= o.iommu_wait_timeout;
        self.max_tree_depth = self.max_tree_depth.max(o.max_tree_depth);
        self.max_walk = self.max_walk.max(o.max_walk);
        self.tree_depth_65_refused |= o.tree_depth_65_refused;
        self.max_grants = self.max_grants.max(o.max_grants);
        self.max_uses = self.max_uses.max(o.max_uses);
        self.grants_full_refused |= o.grants_full_refused;
        self.uses_full_dropped |= o.uses_full_dropped;
        self.max_due_timers = self.max_due_timers.max(o.max_due_timers);
        self.time_event_step_16 |= o.time_event_step_16;
        self.clear_step_again |= o.clear_step_again;
        self.max_debug_write = self.max_debug_write.max(o.max_debug_write);
        self.debug_write_over_512_refused |= o.debug_write_over_512_refused;
        self.trace_burst_suppressed |= o.trace_burst_suppressed;
        self.max_entropy_bytes = self.max_entropy_bytes.max(o.max_entropy_bytes);
        self.entropy_over_64_refused |= o.entropy_over_64_refused;
        self.map_count_max_refused |= o.map_count_max_refused;
    }

    /// Samples one `Model` (standalone or part of a `System`) cheaply.
    pub(crate) fn sample_model(&mut self, node: &Model) {
        self.max_nodes = self.max_nodes.max(usize::from(node.table.nodes));
        self.max_cores_per_node = self.max_cores_per_node.max(node.cores.len());
        let me = usize::from(node.node);
        if me < node.table.beacons.len() {
            if let Some((reason, _, _)) = node.table.beacons[me].panic
                && reason == CrashReason::CoreSilent as u64
            {
                self.core_answer_timeout = true;
            }
            self.max_observations = self
                .max_observations
                .max(node.table.beacons[me].observations.len());
            self.max_grants = self.max_grants.max(node.table.grants[me].len());
            self.max_uses = self.max_uses.max(node.table.uses[me].len());
        }
        for area in &node.table.crash.areas {
            if area.suppressed > 0 {
                self.trace_burst_suppressed = true;
            }
        }
        if let Some(c0) = node.cores.first() {
            self.max_core0_admitted = self.max_core0_admitted.max(c0.admitted);
        }
        let watches = node.watchers.iter().filter(|w| w.is_some()).count();
        self.max_watch_slots = self.max_watch_slots.max(watches);
        for core in &node.cores {
            self.max_idle_states = self.max_idle_states.max(core.idle_table.len());
        }
        let ur = node.unit_requests.iter().filter(|e| e.is_some()).count();
        self.max_unit_requests = self.max_unit_requests.max(ur);
        self.max_unit_service = self.max_unit_service.max(node.unit_service.len());
        if node.steps_at.0 > 0 {
            self.recovery_period_waited = true;
        }
        if node.next_sweep > node.table.sweep_ns {
            self.sweep_fired = true;
        }
        for start in node.starts.values() {
            if start.last > start.since {
                self.start_retried = true;
            }
            if node.membership_now() >= start.since + dodos_abi::START_NS {
                self.start_timeout = true;
            }
        }
        for (peer_idx, peer) in node.peers.iter().enumerate() {
            let req = peer.credits[..REQUESTS_PER_PEER]
                .iter()
                .filter(|c| c.is_some())
                .count();
            let snd = peer.credits[REQUESTS_PER_PEER..]
                .iter()
                .filter(|c| c.is_some())
                .count();
            self.max_req_credits = self.max_req_credits.max(req);
            self.max_send_credits = self.max_send_credits.max(snd);
            if req == REQUESTS_PER_PEER {
                let p = peer_idx as u16;
                let wants_unit = node.unit_requests.iter().flatten().any(|e| {
                    matches!(e.stage, crate::units::Stage::Queued)
                        && node.table.roles[e.role].holder() == p
                });
                let wants_ask = node
                    .rebuilds
                    .values()
                    .any(|r| r.node == p && r.asked.is_none() && r.closing.is_none());
                if wants_unit || wants_ask {
                    self.req_credit_17th_refused = true;
                }
            }
        }
        for &cut in &node.rebuild_cuts.0 {
            match cut.reason {
                crate::units::RebuildCut::Deadline => self.rebuild_stuck_cut = true,
                crate::units::RebuildCut::BadItem | crate::units::RebuildCut::Malformed => {
                    self.report_bad_rejected = true;
                }
            }
        }
        let irq_handles = node
            .irqs
            .values()
            .filter(|i| !i.source.is_core_event() && !i.removing)
            .count();
        self.max_node_handles = self.max_node_handles.max(irq_handles);
        for devs in node.iospaces.values() {
            self.max_iospace_devices = self.max_iospace_devices.max(devs.len());
        }
        for (&cap_id, cap) in &node.caps {
            self.max_tree_depth = self.max_tree_depth.max(node.depth(cap_id));
            if matches!(cap.record, Record::Unmapping(_)) {
                self.removal_unmapping_stop = true;
            }
        }
        self.max_walk = self.max_walk.max(node.longest_walk);
        for cs in node.cspaces.values() {
            self.max_cspace_slots = self.max_cspace_slots.max(cs.max);
            self.max_cspace_grown_slots = self.max_cspace_grown_slots.max(cs.slots.len());
        }
        for sched in node.scheds.values() {
            if let Some((_, debt)) = sched.throttled {
                self.max_sched_debt = self.max_sched_debt.max(debt.budgets);
                if debt.forgiven {
                    self.sched_debt_forgiven = true;
                }
            }
        }
        let due_timers = node
            .timers
            .values()
            .flatten()
            .filter(|a| a.instant <= node.now)
            .count();
        self.max_due_timers = self.max_due_timers.max(due_timers);
        if due_timers > TIME_EVENT_STEP {
            self.time_event_step_16 = true;
        }
    }

    /// Samples a whole `System` (table, rings, beacons, all live nodes).
    pub(crate) fn sample_system(&mut self, sys: &System, scan: &mut RingScan) {
        let table = sys.canonical();
        let n = usize::from(table.nodes);
        self.max_nodes = self.max_nodes.max(n);
        scan.ensure(n);

        let reserves = table
            .role_records
            .iter()
            .filter(|r| r.kind == RoleKind::Reserve)
            .count();
        let plain = table
            .role_records
            .iter()
            .filter(|r| r.kind == RoleKind::Plain)
            .count();
        self.max_reserves = self.max_reserves.max(reserves);
        self.max_plain_roles = self.max_plain_roles.max(plain);

        for word in &table.roles {
            if word.failed() {
                self.iommu_wait_timeout = true;
            }
        }

        let mut sealed_count = 0usize;
        for node_idx in 0..n {
            let member = table.members[node_idx];
            match member.state() {
                MemberState::Fencing | MemberState::Fenced | MemberState::Unfenced => {
                    match member.evidence() {
                        Evidence::Stale => self.stale_accused = true,
                        Evidence::Stuck => self.stuck_accused = true,
                        Evidence::Seal => self.seal_accused = true,
                        _ => {}
                    }
                    if member.state() == MemberState::Unfenced {
                        self.fence_timeout_unfenced = true;
                    }
                    if member.state() == MemberState::Fenced
                        && let Some(confirmed) = table.fences[node_idx].confirmed
                        && table.fences[node_idx].generation == member.generation()
                        && sys.now() < confirmed + dodos_abi::SETTLE_NS
                    {
                        self.settle_waited = true;
                    }
                }
                MemberState::Sealed => sealed_count += 1,
                _ => {}
            }
            if let Some((reason, _, _)) = table.beacons[node_idx].panic
                && reason == CrashReason::CoreSilent as u64
            {
                self.core_answer_timeout = true;
            }
            let obs = table.beacons[node_idx].observations.len();
            self.max_observations = self.max_observations.max(obs);
            let grants = table.grants[node_idx].len();
            let uses = table.uses[node_idx].len();
            self.max_grants = self.max_grants.max(grants);
            self.max_uses = self.max_uses.max(uses);
            let pending_sig = table.uses[node_idx]
                .values()
                .filter(|u| u.signal_pending)
                .count();
            let pending_rel = table.uses[node_idx]
                .values()
                .filter(|u| u.release_pending)
                .count();
            self.max_pending_signals = self.max_pending_signals.max(pending_sig);
            self.max_pending_releases = self.max_pending_releases.max(pending_rel);
            if pending_sig > PENDING_STEP || pending_rel > PENDING_STEP {
                self.pending_step_32 = true;
            }
            if pending_sig > 0 {
                // Signal pending is set when user ring is full or target is starting fresh.
                for u in table.uses[node_idx].values().filter(|u| u.signal_pending) {
                    let g = usize::from(u.granter);
                    let used = table.rings[node_idx][g]
                        .user
                        .produced
                        .wrapping_sub(table.beacons[g].consumed[node_idx].1);
                    if used == RING_USER_ENTRIES {
                        self.user_ring_full = true;
                    }
                }
            }
            if pending_rel > 0 {
                for u in table.uses[node_idx].values().filter(|u| u.release_pending) {
                    let g = usize::from(u.granter);
                    let used = table.rings[node_idx][g]
                        .kernel
                        .produced
                        .wrapping_sub(table.beacons[g].consumed[node_idx].0);
                    if used <= RING_KERNEL_ENTRIES
                        && used + 1 + RING_ANSWER_RESERVE > RING_KERNEL_ENTRIES
                    {
                        self.kernel_ring_hit_reserve = true;
                    }
                }
            }
        }
        self.max_sealed_nodes = self.max_sealed_nodes.max(sealed_count);
        for area in &table.crash.areas {
            if area.suppressed > 0 {
                self.trace_burst_suppressed = true;
            }
        }

        // Scan rings and observe usage + new messages.
        self.sample_rings(sys, table, scan);

        // Sample each live/present node model.
        for (id, node) in sys.models() {
            self.sample_model(node);
            // Detect 2-sealed-per-sweep cap: sweep ran while >= 3 other nodes are sealed.
            if sealed_count >= 3 && node.next_sweep > table.sweep_ns {
                let others_sealed = (0..n)
                    .filter(|&z| {
                        z != usize::from(id) && table.members[z].state() == MemberState::Sealed
                    })
                    .count();
                if others_sealed >= 3 {
                    self.sweep_2_sealed_capped = true;
                }
            }
            // Detect 9th observation dropped: observations full (8) and needed() wants another.
            let me = usize::from(id);
            if table.beacons[me].observations.len() >= OBSERVATIONS_MAX
                && table.writes(id)
                && self.wants_extra_observation(node, table)
            {
                self.obs_9th_dropped = true;
            }
        }
    }

    fn wants_extra_observation(&self, node: &Model, table: &Table) -> bool {
        let me = usize::from(node.node);
        let obs = &table.beacons[me].observations;
        for z in 0..table.nodes {
            if z == node.node || !table.writes(z) {
                continue;
            }
            let member = table.members[usize::from(z)];
            let g = member.generation();
            let used_k = table.rings[me][usize::from(z)]
                .kernel
                .produced
                .wrapping_sub(table.beacons[usize::from(z)].consumed[me].0);
            let used_u = table.rings[me][usize::from(z)]
                .user
                .produced
                .wrapping_sub(table.beacons[usize::from(z)].consumed[me].1);
            if used_k > 0 && used_k <= RING_KERNEL_ENTRIES {
                let has = obs.iter().any(|o| {
                    o.node == z
                        && o.holds(member)
                        && matches!(o.watched, crate::table::Watched::Ring { user: false, .. })
                });
                if !has {
                    return true;
                }
            }
            if used_u > 0 && used_u <= RING_USER_ENTRIES {
                let has = obs.iter().any(|o| {
                    o.node == z
                        && o.holds(member)
                        && matches!(o.watched, crate::table::Watched::Ring { user: true, .. })
                });
                if !has {
                    return true;
                }
            }
            let waiting = node.peers[usize::from(z)]
                .credits
                .iter()
                .any(Option::is_some);
            if waiting {
                for core in table.cores_of(z, g) {
                    let has = obs.iter().any(|o| {
                        o.node == z
                            && o.holds(member)
                            && matches!(o.watched, crate::table::Watched::Heartbeat { core: c, .. } if c == core)
                    });
                    if !has {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn sample_rings(&mut self, sys: &System, table: &Table, scan: &mut RingScan) {
        let n = usize::from(table.nodes);
        for from in 0..n {
            for to in 0..n {
                if from == to {
                    continue;
                }
                let pair = &table.rings[from][to];
                let consumed = table.beacons[to].consumed[from];
                let k_used = pair.kernel.produced.wrapping_sub(consumed.0);
                if k_used <= RING_KERNEL_ENTRIES {
                    self.max_kernel_ring_used = self.max_kernel_ring_used.max(k_used);
                    if k_used > RING_KERNEL_ENTRIES - RING_ANSWER_RESERVE {
                        self.kernel_ring_used_reserve = true;
                    }
                    if k_used >= RING_KERNEL_ENTRIES - RING_ANSWER_RESERVE && sys.has(from as u16) {
                        let sender = sys.node(from as u16);
                        let p = to as u16;
                        let wants_unit = sender.unit_requests.iter().flatten().any(|e| {
                            matches!(e.stage, crate::units::Stage::Queued)
                                && table.roles[e.role].holder() == p
                        });
                        let wants_ask = sender
                            .rebuilds
                            .values()
                            .any(|r| r.node == p && r.asked.is_none() && r.closing.is_none());
                        if wants_unit || wants_ask {
                            self.kernel_ring_hit_reserve = true;
                        }
                    }
                    if k_used == RING_KERNEL_ENTRIES {
                        self.kernel_ring_full = true;
                    }
                }
                let u_used = pair.user.produced.wrapping_sub(consumed.1);
                if u_used <= RING_USER_ENTRIES {
                    self.max_user_ring_used = self.max_user_ring_used.max(u_used);
                    if u_used > RING_USER_STEP {
                        self.user_ring_step_16 = true;
                    }
                    if u_used == RING_USER_ENTRIES {
                        self.user_ring_full = true;
                    }
                }

                // Scan newly produced kernel ring messages.
                let k_seen = scan.seen_kernel[from][to];
                if pair.kernel.produced > k_seen {
                    let start =
                        k_seen.max(pair.kernel.produced.saturating_sub(RING_KERNEL_ENTRIES));
                    let from_normal = sys.has(from as u16)
                        && sys.node(from as u16).faulty_report
                            == crate::units::FaultyReport::Normal;
                    for idx in start..pair.kernel.produced {
                        if let Some(RingEntry::Message(msg)) = pair.kernel.at(idx) {
                            let genuine = msg.header.sender == from as u16;
                            if genuine {
                                let len = usize::from(msg.header.length);
                                if len <= dodos_abi::RING_MESSAGE_ENTRIES_MAX {
                                    self.max_ring_msg_entries = self.max_ring_msg_entries.max(len);
                                }
                                match &msg.body {
                                    Body::Report { next, items }
                                        if from_normal
                                            && items.len() <= crate::rings::REPORT_ITEMS =>
                                    {
                                        self.max_report_items =
                                            self.max_report_items.max(items.len());
                                        if items.len() == crate::rings::REPORT_ITEMS
                                            && *next != REPORT_END
                                        {
                                            self.report_28_more = true;
                                        }
                                    }
                                    Body::ReportAsk { role, epoch, .. } => {
                                        let key = (from as u16, *role, *epoch, to as u16);
                                        let count = scan.rebuild_asks.entry(key).or_insert(0);
                                        *count += 1;
                                        self.max_rebuild_asks_per_node =
                                            self.max_rebuild_asks_per_node.max(*count);
                                    }
                                    Body::RevokeStep { removed: false } => {
                                        self.removal_32_again = true;
                                    }
                                    Body::SendAnswer { dropped: true, .. } => {
                                        // Check if dropped because uses table was full.
                                        if table.uses[from].len() >= dodos_abi::GRANTS {
                                            self.uses_full_dropped = true;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    scan.seen_kernel[from][to] = pair.kernel.produced;
                }

                // Scan newly produced user ring messages.
                let u_seen = scan.seen_user[from][to];
                if pair.user.produced > u_seen {
                    let start = u_seen.max(pair.user.produced.saturating_sub(RING_USER_ENTRIES));
                    for idx in start..pair.user.produced {
                        if let Some(RingEntry::Message(msg)) = pair.user.at(idx)
                            && msg.header.sender == from as u16
                        {
                            let len = usize::from(msg.header.length);
                            if len <= dodos_abi::RING_MESSAGE_ENTRIES_MAX {
                                self.max_ring_msg_entries = self.max_ring_msg_entries.max(len);
                            }
                            if let Body::Send { grants, words, .. } = &msg.body {
                                if words.len() <= MAX_WORDS {
                                    self.max_msg_words = self.max_msg_words.max(words.len());
                                }
                                if grants.len() <= MAX_CAPS {
                                    self.max_send_grants = self.max_send_grants.max(grants.len());
                                }
                            }
                        }
                    }
                    scan.seen_user[from][to] = pair.user.produced;
                }
            }
        }
    }
}

/// Small `Copy` summary of a `Syscall` extracted before `apply` consumes it.
#[derive(Clone, Copy)]
pub(crate) enum CallMeta {
    NodeWatch {
        index: u64,
    },
    NodeIdle {
        words: usize,
    },
    NodeSend {
        node: dodos_abi::SlotAddr,
        words: usize,
        caps: usize,
    },
    LocalMsg {
        words: usize,
    },
    Removal,
    UntypedRetype {
        kind: dodos_abi::ObjectKind,
        size: u64,
    },
    CapDerive,
    CSpaceGrow,
    IoSpaceAttach,
    UnitCall {
        irq_claim: bool,
    },
    SchedConfigure,
    DebugWrite {
        len: u64,
    },
    EntropyRead {
        bytes: u64,
    },
    ASpaceMap,
    Other,
}

impl CallMeta {
    pub(crate) fn of(call: &Syscall) -> Self {
        match call {
            Syscall::NodeWatch { index, .. } => Self::NodeWatch { index: *index },
            Syscall::NodeIdle { words, .. } => Self::NodeIdle { words: words.len() },
            Syscall::NodeSend { node, message, .. } => Self::NodeSend {
                node: *node,
                words: message.words.len(),
                caps: message.caps.len(),
            },
            Syscall::Send { message, .. }
            | Syscall::Call { message, .. }
            | Syscall::ReplyRecv { message, .. } => Self::LocalMsg {
                words: message.words.len(),
            },
            Syscall::CapDelete(_) | Syscall::CapRevoke(_) => Self::Removal,
            Syscall::UntypedRetype { kind, size, .. } => Self::UntypedRetype {
                kind: *kind,
                size: *size,
            },
            Syscall::CapDerive { .. } => Self::CapDerive,
            Syscall::CSpaceGrow { .. } => Self::CSpaceGrow,
            Syscall::IoSpaceAttach { .. } => Self::IoSpaceAttach,
            Syscall::IoSpaceMap(_) | Syscall::IoSpaceUnmap(_) => {
                Self::UnitCall { irq_claim: false }
            }
            Syscall::IrqClaim { .. } => Self::UnitCall { irq_claim: true },
            Syscall::SchedConfigure { .. } => Self::SchedConfigure,
            Syscall::DebugWrite { length, .. } => Self::DebugWrite { len: *length },
            Syscall::EntropyRead { length, .. } => Self::EntropyRead { bytes: *length },
            Syscall::ASpaceMap(_) => Self::ASpaceMap,
            _ => Self::Other,
        }
    }
}

impl Bounds {
    /// Samples a syscall and its outcome (refusal and boundary paths).
    pub(crate) fn sample_call(&mut self, model: &Model, call: CallMeta, outcome: &Outcome) {
        match call {
            CallMeta::NodeWatch { index } => {
                if index >= NODE_WATCH_SLOTS && *outcome == Err(Error::Invalid) {
                    self.watch_slot_5th_refused = true;
                }
            }
            CallMeta::NodeIdle { words } => {
                if words > IDLE_STATES_MAX * IDLE_ENTRY_WORDS && *outcome == Err(Error::Invalid) {
                    self.idle_16th_refused = true;
                }
            }
            CallMeta::NodeSend { node, words, caps } => {
                if words > MAX_WORDS && *outcome == Err(Error::Invalid) {
                    self.msg_words_over_64_refused = true;
                } else if outcome.is_ok() {
                    self.max_msg_words = self.max_msg_words.max(words);
                    self.max_send_grants = self.max_send_grants.max(caps);
                }
                if caps > MAX_CAPS && *outcome == Err(Error::Invalid) {
                    self.send_grants_over_4_refused = true;
                }
                if *outcome == Err(Error::Busy) {
                    if model.table.grants[usize::from(model.node)].len() + caps > dodos_abi::GRANTS
                    {
                        self.grants_full_refused = true;
                    }
                    let target = node
                        .index()
                        .checked_sub(dodos_abi::START_SLOT_FIRST)
                        .filter(|&i| (i as usize) < model.peers.len())
                        .map(|i| i as u16)
                        .or_else(|| {
                            model.cspaces.values().find_map(|cs| {
                                match cs.slots.get(node.index() as usize)? {
                                    crate::Slot::Full(cid) => {
                                        let obj = model.caps.get(cid)?.object;
                                        model.nodes.get(&obj).copied()
                                    }
                                    _ => None,
                                }
                            })
                        });
                    if let Some(target_idx) = target {
                        let sends = model.peers[usize::from(target_idx)].sends();
                        if sends >= SENDS_PER_PEER {
                            self.send_credit_9th_refused = true;
                        } else if sends >= SENDS_PER_PEER - SENDS_RESERVED_FOR_MANAGE {
                            self.send_credit_7th_no_manage_refused = true;
                        }
                        let used = model
                            .ring_of(model.node, target_idx, true)
                            .produced
                            .wrapping_sub(model.consumed(model.node, target_idx, true));
                        let need = dodos_abi::message_entries(3 + caps + words) as u64;
                        if used <= RING_USER_ENTRIES && used + need > RING_USER_ENTRIES {
                            self.user_ring_full = true;
                        }
                    }
                }
            }
            CallMeta::LocalMsg { words } => {
                if words > MAX_WORDS && *outcome == Err(Error::Invalid) {
                    self.msg_words_over_64_refused = true;
                } else if outcome.is_ok() {
                    self.max_msg_words = self.max_msg_words.max(words);
                }
            }
            CallMeta::Removal => {
                if *outcome == Err(Error::Again) {
                    self.removal_32_again = true;
                    if model.cspaces.values().any(|cs| {
                        cs.sweep
                            .as_ref()
                            .is_some_and(|s| s.next >= dodos_abi::REMOVAL_SLOTS)
                    }) {
                        self.removal_128_slots_again = true;
                    }
                    if model
                        .peers
                        .iter()
                        .any(|p| p.credits[..REQUESTS_PER_PEER].iter().all(Option::is_some))
                    {
                        self.req_credit_17th_refused = true;
                    }
                }
            }
            CallMeta::UntypedRetype { kind, size } => {
                if *outcome == Err(Error::Again) {
                    self.clear_step_again = true;
                }
                if kind == dodos_abi::ObjectKind::CSpace
                    && size > u64::from(dodos_abi::CSPACE_MAX_SLOTS)
                    && *outcome == Err(Error::Invalid)
                {
                    self.cspace_over_max_refused = true;
                }
                if *outcome == Err(Error::Invalid)
                    && model
                        .caps
                        .keys()
                        .any(|&id| model.depth(id) >= dodos_abi::TREE_DEPTH)
                {
                    self.tree_depth_65_refused = true;
                }
            }
            CallMeta::CapDerive => {
                if *outcome == Err(Error::Invalid)
                    && model
                        .caps
                        .keys()
                        .any(|&id| model.depth(id) >= dodos_abi::TREE_DEPTH)
                {
                    self.tree_depth_65_refused = true;
                }
            }
            CallMeta::CSpaceGrow => {
                if *outcome == Err(Error::NoSlot)
                    || *outcome == Err(Error::Busy)
                    || (*outcome == Err(Error::Invalid)
                        && model
                            .cspaces
                            .values()
                            .any(|cs| cs.slots.len() >= cs.max as usize))
                {
                    self.cspace_over_max_refused = true;
                }
            }
            CallMeta::IoSpaceAttach => {
                if *outcome == Err(Error::Busy)
                    && model
                        .iospaces
                        .values()
                        .any(|d| d.len() >= IOSPACE_MAX_DEVICES)
                {
                    self.iospace_17th_refused = true;
                }
                if matches!(*outcome, Err(Error::Busy | Error::Again))
                    && model.unit_requests.iter().all(Option::is_some)
                {
                    self.unit_requests_65th_refused = true;
                }
            }
            CallMeta::UnitCall { irq_claim } => {
                if matches!(*outcome, Err(Error::Busy | Error::Again))
                    && model.unit_requests.iter().all(Option::is_some)
                {
                    self.unit_requests_65th_refused = true;
                }
                if *outcome == Err(Error::Busy)
                    && irq_claim
                    && model.irqs.values().filter(|i| !i.removing).count()
                        >= dodos_abi::x86_64::IRQ_TABLE_ENTRIES
                {
                    self.irq_193rd_refused = true;
                }
            }
            CallMeta::SchedConfigure => {
                if *outcome == Err(Error::Budget) {
                    self.admission_refused = true;
                }
            }
            CallMeta::DebugWrite { len } => {
                if len > dodos_abi::DEBUG_WRITE_MAX && *outcome == Err(Error::Invalid) {
                    self.debug_write_over_512_refused = true;
                } else if outcome.is_ok() {
                    self.max_debug_write = self.max_debug_write.max(len);
                }
            }
            CallMeta::EntropyRead { bytes } => {
                if bytes > dodos_abi::ENTROPY_MAX_BYTES && *outcome == Err(Error::Invalid) {
                    self.entropy_over_64_refused = true;
                } else if outcome.is_ok() {
                    self.max_entropy_bytes = self.max_entropy_bytes.max(bytes);
                }
            }
            CallMeta::ASpaceMap => {
                if *outcome == Err(Error::Busy)
                    && model.table.gate_words.iter().any(|&w| {
                        dodos_abi::GateWord::from_raw(w).is_some_and(|g| {
                            u64::from(g.map_count()) >= model.machine.map_count_max
                        })
                    })
                {
                    self.map_count_max_refused = true;
                }
            }
            CallMeta::Other => {}
        }
        // Also check if a starting node was accused stale (START_NS).
        for m in &model.table.members {
            if m.state() == MemberState::Fencing && m.evidence() == Evidence::Stale {
                let g = m.generation();
                if model.starts.values().any(|s| s.generation == g) {
                    self.stale_starting_accused = true;
                    self.start_timeout = true;
                }
            }
        }
    }

    pub(crate) fn print(&self, label: &str) {
        let yn = |b: bool| if b { "yes" } else { "NO" };
        eprintln!("\n=== Contract bounds coverage ({label}) ===");
        eprintln!("| Bound | Contract | Max reached | Refusal / boundary path |");
        eprintln!("|---|---|---|---|");
        eprintln!(
            "| NODES_MAX | 1024 | {} | — (scaled down in harness) |",
            self.max_nodes
        );
        eprintln!(
            "| CORES_PER_KERNEL_MAX | 64 | {} | — (scaled down in harness) |",
            self.max_cores_per_node
        );
        eprintln!(
            "| STALE_NS | 100 ms (10..1000 ms) | — | accused stale={}, starting={} |",
            yn(self.stale_accused),
            yn(self.stale_starting_accused)
        );
        eprintln!(
            "| STUCK_NS | 100 ms (10..1000 ms) | — | accused stuck={}, rebuild_cut={} |",
            yn(self.stuck_accused),
            yn(self.rebuild_stuck_cut)
        );
        eprintln!(
            "| SWEEP_NS | 1 s (100 ms..60 s) | — | sweep_fired={} |",
            yn(self.sweep_fired)
        );
        eprintln!(
            "| FENCE_NS | 30 ms (23..1000 ms) | — | unfenced_at_timeout={} |",
            yn(self.fence_timeout_unfenced)
        );
        eprintln!(
            "| SETTLE_NS | 1 ms | — | settle_waited={} |",
            yn(self.settle_waited)
        );
        eprintln!(
            "| START_NS | 1 s | — | start_timeout={} |",
            yn(self.start_timeout)
        );
        eprintln!(
            "| START_RETRY_NS | 1 ms | — | start_retried={} |",
            yn(self.start_retried)
        );
        eprintln!(
            "| SEAL_NS | 1 s (10 ms..60 s) | — | accused_seal={} |",
            yn(self.seal_accused)
        );
        eprintln!(
            "| CORE_ANSWER_NS | 10 ms (1..100 ms) | — | core_silent_panic={} |",
            yn(self.core_answer_timeout)
        );
        eprintln!(
            "| Recovery reserve | 1/64, 10 ms | admitted={:#x} | period_waited={}, admission_refused={} |",
            self.max_core0_admitted,
            yn(self.recovery_period_waited),
            yn(self.admission_refused)
        );
        eprintln!(
            "| NODE_WATCH_SLOTS | 4 | {} | 5th_refused={} |",
            self.max_watch_slots,
            yn(self.watch_slot_5th_refused)
        );
        eprintln!(
            "| IDLE_STATES_MAX | 15 | {} | 16th_refused={} |",
            self.max_idle_states,
            yn(self.idle_16th_refused)
        );
        eprintln!(
            "| OBSERVATIONS_MAX | 8 | {} | 9th_dropped={} |",
            self.max_observations,
            yn(self.obs_9th_dropped)
        );
        eprintln!("| RESERVES_MAX | 8 (min 2) | {} | — |", self.max_reserves);
        eprintln!("| ROLES_PLAIN_MAX | 64 | {} | — |", self.max_plain_roles);
        eprintln!(
            "| RING_KERNEL_ENTRIES | 64 | {} | hit_reserve={}, used_reserve={}, full={} |",
            self.max_kernel_ring_used,
            yn(self.kernel_ring_hit_reserve),
            yn(self.kernel_ring_used_reserve),
            yn(self.kernel_ring_full)
        );
        eprintln!(
            "| RING_USER_ENTRIES | 64 | {} | full={} |",
            self.max_user_ring_used,
            yn(self.user_ring_full)
        );
        eprintln!(
            "| REQUESTS_PER_PEER | 16 | {} | 17th_refused={} |",
            self.max_req_credits,
            yn(self.req_credit_17th_refused)
        );
        eprintln!(
            "| SENDS_PER_PEER | 8 (6 no-manage) | {} | 7th_no_manage={}, 9th_refused={} |",
            self.max_send_credits,
            yn(self.send_credit_7th_no_manage_refused),
            yn(self.send_credit_9th_refused)
        );
        eprintln!(
            "| UNIT_REQUESTS | 64 | {} (service={}) | 65th_refused={} |",
            self.max_unit_requests,
            self.max_unit_service,
            yn(self.unit_requests_65th_refused)
        );
        eprintln!(
            "| RING_ANSWER_RESERVE | 34 | k_used={} | hit_reserve={}, used_reserve={} |",
            self.max_kernel_ring_used,
            yn(self.kernel_ring_hit_reserve),
            yn(self.kernel_ring_used_reserve)
        );
        eprintln!(
            "| Report items / entries | 28 / 11 | {} / {} | 28_and_more={}, bad_rejected={} |",
            self.max_report_items,
            self.max_ring_msg_entries,
            yn(self.report_28_more),
            yn(self.report_bad_rejected)
        );
        eprintln!(
            "| IRQ_TABLE_ENTRIES / asks | 192 / 8 | handles={}, asks={} | 193rd_refused={} |",
            self.max_node_handles,
            self.max_rebuild_asks_per_node,
            yn(self.irq_193rd_refused)
        );
        eprintln!(
            "| REMOVAL_STEP / unmapping | 32 / 1 | — | step_32_again={}, unmapping_stop={} |",
            yn(self.removal_32_again),
            yn(self.removal_unmapping_stop)
        );
        eprintln!(
            "| REMOVAL_SLOTS | 128 | — | slots_128_again={} |",
            yn(self.removal_128_slots_again)
        );
        eprintln!(
            "| CSPACE_MAX_SLOTS | 2^20 (1048576) | max={}, grown={} | over_max_refused={} |",
            self.max_cspace_slots,
            self.max_cspace_grown_slots,
            yn(self.cspace_over_max_refused)
        );
        eprintln!(
            "| SCHED_MAX_DEBT | 2^26 (67108864) | {} | forgiven={} |",
            self.max_sched_debt,
            yn(self.sched_debt_forgiven)
        );
        eprintln!(
            "| Two sealed per sweep | 2 | sealed_at_once={} | capped_at_2={} |",
            self.max_sealed_nodes,
            yn(self.sweep_2_sealed_capped)
        );
        eprintln!(
            "| MAX_WORDS | 64 | {} | over_64_refused={} |",
            self.max_msg_words,
            yn(self.msg_words_over_64_refused)
        );
        eprintln!(
            "| MAX_CAPS / SEND_GRANTS_MAX | 4 | {} | over_4_refused={} |",
            self.max_send_grants,
            yn(self.send_grants_over_4_refused)
        );
        eprintln!(
            "| RING_USER_STEP | 16 | u_used={} | over_16_step={} |",
            self.max_user_ring_used,
            yn(self.user_ring_step_16)
        );
        eprintln!(
            "| PENDING_STEP | 32 | sig={}, rel={} | over_32_step={} |",
            self.max_pending_signals,
            self.max_pending_releases,
            yn(self.pending_step_32)
        );
        eprintln!(
            "| IOSPACE_MAX_DEVICES | 16 | {} | 17th_refused={} |",
            self.max_iospace_devices,
            yn(self.iospace_17th_refused)
        );
        eprintln!(
            "| IOMMU_WAIT_NS | 10 ms | — | unit_failed={} |",
            yn(self.iommu_wait_timeout)
        );
        eprintln!(
            "| TREE_DEPTH / LONGEST_WALK | 64 / 191 | {} / {} | depth_65_refused={} |",
            self.max_tree_depth,
            self.max_walk,
            yn(self.tree_depth_65_refused)
        );
        eprintln!(
            "| GRANTS (grant/use table) | 4096 | grants={}, uses={} | grants_full={}, uses_dropped={} |",
            self.max_grants,
            self.max_uses,
            yn(self.grants_full_refused),
            yn(self.uses_full_dropped)
        );
        eprintln!(
            "| TIME_EVENT_STEP | 16 | due={} | over_16_step={} |",
            self.max_due_timers,
            yn(self.time_event_step_16)
        );
        eprintln!(
            "| CLEAR_STEP_BYTES | 65536 | — | again={} |",
            yn(self.clear_step_again)
        );
        eprintln!(
            "| DEBUG_WRITE_MAX | 512 | {} | over_512_refused={} |",
            self.max_debug_write,
            yn(self.debug_write_over_512_refused)
        );
        eprintln!(
            "| TRACE_BURST | 16 / 1 ms | — | suppressed={} |",
            yn(self.trace_burst_suppressed)
        );
        eprintln!(
            "| ENTROPY_MAX_BYTES | 64 | {} | over_64_refused={} |",
            self.max_entropy_bytes,
            yn(self.entropy_over_64_refused)
        );
        eprintln!(
            "| MAP_COUNT_MAX | u32::MAX | — | refused={} |",
            yn(self.map_count_max_refused)
        );
    }
}
