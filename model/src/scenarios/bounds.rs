//! Harness instrumentation for contract bounds (passes 43–44).
//!
//! Measures the highest value reached, whether the exact refusal/wait/boundary
//! path was taken, and (for cross-node pressure rows) whether the bound was
//! reached while a recovery involving a node concerned was in progress.
//! Lives strictly in the test build: no model semantics are altered.

use crate::rings::Body;
use crate::system::System;
use crate::table::{RingEntry, Table};
use crate::{Model, Outcome, Syscall};
use dodos_abi::{
    MAX_CAPS, MAX_WORDS, MemberState, REPORT_END, REQUESTS_PER_PEER, RING_KERNEL_ENTRIES,
    RING_USER_ENTRIES, RoleKind,
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

/// Exact bound-path hits recorded on a `Model` at the exact branch that
/// enforced each bound (stored inside `Ephemeral<BoundHits>` so model
/// equality and semantics are untouched).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BoundHits {
    pub(crate) stale_accused: bool,
    pub(crate) stale_starting_accused: bool,
    pub(crate) stuck_accused: bool,
    pub(crate) sweep_fired: bool,
    pub(crate) settle_waited: bool,
    pub(crate) start_timeout: bool,
    pub(crate) start_retried: bool,
    pub(crate) seal_accused: bool,
    pub(crate) core_answer_timeout: bool,
    pub(crate) recovery_period_waited: bool,
    pub(crate) admission_refused: bool,
    pub(crate) watch_slot_5th_refused: bool,
    pub(crate) idle_16th_refused: bool,
    pub(crate) obs_9th_dropped: bool,
    pub(crate) kernel_ring_hit_reserve: bool,
    pub(crate) kernel_ring_used_reserve: bool,
    pub(crate) kernel_ring_full: bool,
    pub(crate) user_ring_full: bool,
    pub(crate) req_credit_17th_refused: bool,
    pub(crate) send_credit_7th_no_manage_refused: bool,
    pub(crate) send_credit_9th_refused: bool,
    pub(crate) unit_requests_65th_refused: bool,
    pub(crate) irq_193rd_refused: bool,
    pub(crate) removal_32_again: bool,
    pub(crate) removal_unmapping_stop: bool,
    pub(crate) removal_128_slots_again: bool,
    pub(crate) cspace_over_max_refused: bool,
    pub(crate) sched_debt_forgiven: bool,
    pub(crate) sweep_2_sealed_capped: bool,
    pub(crate) msg_words_over_64_refused: bool,
    pub(crate) send_grants_over_4_refused: bool,
    pub(crate) user_ring_step_16: bool,
    pub(crate) pending_step_32: bool,
    pub(crate) iospace_17th_refused: bool,
    pub(crate) tree_depth_65_refused: bool,
    pub(crate) grants_full_refused: bool,
    pub(crate) uses_full_dropped: bool,
    pub(crate) max_due_events: usize,
    pub(crate) time_event_step_16: bool,
    pub(crate) clear_step_again: bool,
    pub(crate) debug_write_over_512_refused: bool,
    pub(crate) entropy_over_64_refused: bool,
    pub(crate) map_count_max_refused: bool,

    // Reached while a recovery (death, fence, seal, fresh start, or rebuild)
    // involving a node concerned was in progress at the exact instant of the hit.
    pub(crate) kernel_ring_hit_reserve_in_recovery: bool,
    pub(crate) kernel_ring_used_reserve_in_recovery: bool,
    pub(crate) kernel_ring_full_in_recovery: bool,
    pub(crate) req_credit_17th_in_recovery: bool,
    pub(crate) unit_requests_65th_in_recovery: bool,
    pub(crate) grants_full_in_recovery: bool,
    pub(crate) uses_dropped_in_recovery: bool,
    pub(crate) pending_step_32_in_recovery: bool,
    pub(crate) pending_step_capped: bool,
    pub(crate) time_event_step_16_in_recovery: bool,
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

    // Reached during recovery (Section 6)
    pub(crate) kernel_ring_hit_reserve_in_recovery: bool,
    pub(crate) kernel_ring_used_reserve_in_recovery: bool,
    pub(crate) kernel_ring_full_in_recovery: bool,
    pub(crate) req_credit_17th_in_recovery: bool,
    pub(crate) unit_requests_65th_in_recovery: bool,
    pub(crate) grants_full_in_recovery: bool,
    pub(crate) uses_dropped_in_recovery: bool,
    pub(crate) pending_step_32_in_recovery: bool,
    pub(crate) time_event_step_16_in_recovery: bool,
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
        self.kernel_ring_hit_reserve_in_recovery |= o.kernel_ring_hit_reserve_in_recovery;
        self.kernel_ring_used_reserve_in_recovery |= o.kernel_ring_used_reserve_in_recovery;
        self.kernel_ring_full_in_recovery |= o.kernel_ring_full_in_recovery;
        self.req_credit_17th_in_recovery |= o.req_credit_17th_in_recovery;
        self.unit_requests_65th_in_recovery |= o.unit_requests_65th_in_recovery;
        self.grants_full_in_recovery |= o.grants_full_in_recovery;
        self.uses_dropped_in_recovery |= o.uses_dropped_in_recovery;
        self.pending_step_32_in_recovery |= o.pending_step_32_in_recovery;
        self.time_event_step_16_in_recovery |= o.time_event_step_16_in_recovery;
    }

    fn merge_hits(&mut self, h: &BoundHits) {
        self.stale_accused |= h.stale_accused;
        self.stale_starting_accused |= h.stale_starting_accused;
        self.stuck_accused |= h.stuck_accused;
        self.sweep_fired |= h.sweep_fired;
        self.settle_waited |= h.settle_waited;
        self.start_timeout |= h.start_timeout;
        self.start_retried |= h.start_retried;
        self.seal_accused |= h.seal_accused;
        self.core_answer_timeout |= h.core_answer_timeout;
        self.recovery_period_waited |= h.recovery_period_waited;
        self.admission_refused |= h.admission_refused;
        self.watch_slot_5th_refused |= h.watch_slot_5th_refused;
        self.idle_16th_refused |= h.idle_16th_refused;
        self.obs_9th_dropped |= h.obs_9th_dropped;
        self.kernel_ring_hit_reserve |= h.kernel_ring_hit_reserve;
        self.kernel_ring_used_reserve |= h.kernel_ring_used_reserve;
        self.kernel_ring_full |= h.kernel_ring_full;
        self.user_ring_full |= h.user_ring_full;
        self.req_credit_17th_refused |= h.req_credit_17th_refused;
        self.send_credit_7th_no_manage_refused |= h.send_credit_7th_no_manage_refused;
        self.send_credit_9th_refused |= h.send_credit_9th_refused;
        self.unit_requests_65th_refused |= h.unit_requests_65th_refused;
        self.irq_193rd_refused |= h.irq_193rd_refused;
        self.removal_32_again |= h.removal_32_again;
        self.removal_unmapping_stop |= h.removal_unmapping_stop;
        self.removal_128_slots_again |= h.removal_128_slots_again;
        self.cspace_over_max_refused |= h.cspace_over_max_refused;
        self.sched_debt_forgiven |= h.sched_debt_forgiven;
        self.sweep_2_sealed_capped |= h.sweep_2_sealed_capped;
        self.msg_words_over_64_refused |= h.msg_words_over_64_refused;
        self.send_grants_over_4_refused |= h.send_grants_over_4_refused;
        self.user_ring_step_16 |= h.user_ring_step_16;
        self.pending_step_32 |= h.pending_step_32;
        self.iospace_17th_refused |= h.iospace_17th_refused;
        self.tree_depth_65_refused |= h.tree_depth_65_refused;
        self.grants_full_refused |= h.grants_full_refused;
        self.uses_full_dropped |= h.uses_full_dropped;
        self.max_due_timers = self.max_due_timers.max(h.max_due_events);
        self.time_event_step_16 |= h.time_event_step_16;
        self.clear_step_again |= h.clear_step_again;
        self.debug_write_over_512_refused |= h.debug_write_over_512_refused;
        self.entropy_over_64_refused |= h.entropy_over_64_refused;
        self.map_count_max_refused |= h.map_count_max_refused;
        self.kernel_ring_hit_reserve_in_recovery |= h.kernel_ring_hit_reserve_in_recovery;
        self.kernel_ring_used_reserve_in_recovery |= h.kernel_ring_used_reserve_in_recovery;
        self.kernel_ring_full_in_recovery |= h.kernel_ring_full_in_recovery;
        self.req_credit_17th_in_recovery |= h.req_credit_17th_in_recovery;
        self.unit_requests_65th_in_recovery |= h.unit_requests_65th_in_recovery;
        self.grants_full_in_recovery |= h.grants_full_in_recovery;
        self.uses_dropped_in_recovery |= h.uses_dropped_in_recovery;
        self.pending_step_32_in_recovery |= h.pending_step_32_in_recovery;
        self.time_event_step_16_in_recovery |= h.time_event_step_16_in_recovery;
    }

    /// Samples one `Model` (standalone or part of a `System`) cheaply.
    pub(crate) fn sample_model(&mut self, node: &Model) {
        self.merge_hits(&node.bound_hits.0);
        self.max_nodes = self.max_nodes.max(usize::from(node.table.nodes));
        self.max_cores_per_node = self.max_cores_per_node.max(node.cores.len());
        let me = usize::from(node.node);
        if me < node.table.beacons.len() {
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
        for peer in &node.peers {
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
        for &cap_id in node.caps.keys() {
            self.max_tree_depth = self.max_tree_depth.max(node.depth(cap_id));
        }
        self.max_walk = self.max_walk.max(node.longest_walk);
        for cs in node.cspaces.values() {
            self.max_cspace_slots = self.max_cspace_slots.max(cs.max);
            self.max_cspace_grown_slots = self.max_cspace_grown_slots.max(cs.slots.len());
        }
        for sched in node.scheds.values() {
            if let Some((_, debt)) = sched.throttled {
                self.max_sched_debt = self.max_sched_debt.max(debt.budgets);
            }
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
                MemberState::Unfenced => self.fence_timeout_unfenced = true,
                MemberState::Sealed => sealed_count += 1,
                _ => {}
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
        }
        self.max_sealed_nodes = self.max_sealed_nodes.max(sealed_count);
        for area in &table.crash.areas {
            if area.suppressed > 0 {
                self.trace_burst_suppressed = true;
            }
        }

        // Scan rings for high-water marks and report/ask messages.
        self.sample_rings(sys, table, scan);

        // Sample each live/present node model.
        for (_, node) in sys.models() {
            self.sample_model(node);
        }
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
                }
                let u_used = pair.user.produced.wrapping_sub(consumed.1);
                if u_used <= RING_USER_ENTRIES {
                    self.max_user_ring_used = self.max_user_ring_used.max(u_used);
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

/// Small `Copy` summary of a `Syscall` extracted before `apply` consumes it
/// (only used to record accepted high-water marks; refusals are recorded
/// directly inside `Model::bound_hits` at the exact check).
#[derive(Clone, Copy)]
pub(crate) enum CallMeta {
    NodeSend { words: usize, caps: usize },
    LocalMsg { words: usize },
    DebugWrite { len: u64 },
    EntropyRead { bytes: u64 },
    Other,
}

impl CallMeta {
    pub(crate) fn of(call: &Syscall) -> Self {
        match call {
            Syscall::NodeSend { message, .. } => Self::NodeSend {
                words: message.words.len(),
                caps: message.caps.len(),
            },
            Syscall::Send { message, .. }
            | Syscall::Call { message, .. }
            | Syscall::ReplyRecv { message, .. } => Self::LocalMsg {
                words: message.words.len(),
            },
            Syscall::DebugWrite { length, .. } => Self::DebugWrite { len: *length },
            Syscall::EntropyRead { length, .. } => Self::EntropyRead { bytes: *length },
            _ => Self::Other,
        }
    }
}

impl Bounds {
    /// Samples high-water marks of accepted syscalls and merges exact model hits.
    pub(crate) fn sample_call(&mut self, model: &Model, call: CallMeta, outcome: &Outcome) {
        self.merge_hits(&model.bound_hits.0);
        if outcome.is_ok() {
            match call {
                CallMeta::NodeSend { words, caps } => {
                    self.max_msg_words = self.max_msg_words.max(words);
                    self.max_send_grants = self.max_send_grants.max(caps);
                }
                CallMeta::LocalMsg { words } => {
                    self.max_msg_words = self.max_msg_words.max(words);
                }
                CallMeta::DebugWrite { len } => {
                    self.max_debug_write = self.max_debug_write.max(len);
                }
                CallMeta::EntropyRead { bytes } => {
                    self.max_entropy_bytes = self.max_entropy_bytes.max(bytes);
                }
                CallMeta::Other => {}
            }
        }
    }

    pub(crate) fn print(&self, label: &str) {
        let yn = |b: bool| if b { "yes" } else { "NO" };
        eprintln!("\n=== Contract bounds coverage ({label}) ===");
        eprintln!(
            "| Bound | Contract | Max reached | Refusal / boundary path | Reached during a recovery |"
        );
        eprintln!("|---|---|---|---|---|");
        eprintln!(
            "| NODES_MAX | 1024 | {} | — (scaled down in harness) | — |",
            self.max_nodes
        );
        eprintln!(
            "| CORES_PER_KERNEL_MAX | 64 | {} | — (scaled down in harness) | — |",
            self.max_cores_per_node
        );
        eprintln!(
            "| STALE_NS | 100 ms (10..1000 ms) | — | accused stale={}, starting={} | — |",
            yn(self.stale_accused),
            yn(self.stale_starting_accused)
        );
        eprintln!(
            "| STUCK_NS | 100 ms (10..1000 ms) | — | accused stuck={}, rebuild_cut={} | — |",
            yn(self.stuck_accused),
            yn(self.rebuild_stuck_cut)
        );
        eprintln!(
            "| SWEEP_NS | 1 s (100 ms..60 s) | — | sweep_fired={} | — |",
            yn(self.sweep_fired)
        );
        eprintln!(
            "| FENCE_NS | 30 ms (23..1000 ms) | — | unfenced_at_timeout={} | — |",
            yn(self.fence_timeout_unfenced)
        );
        eprintln!(
            "| SETTLE_NS | 1 ms | — | settle_waited={} | — |",
            yn(self.settle_waited)
        );
        eprintln!(
            "| START_NS | 1 s | — | start_timeout={} | — |",
            yn(self.start_timeout)
        );
        eprintln!(
            "| START_RETRY_NS | 1 ms | — | start_retried={} | — |",
            yn(self.start_retried)
        );
        eprintln!(
            "| SEAL_NS | 1 s (10 ms..60 s) | — | accused_seal={} | — |",
            yn(self.seal_accused)
        );
        eprintln!(
            "| CORE_ANSWER_NS | 10 ms (1..100 ms) | — | core_answer_timeout={} | — |",
            yn(self.core_answer_timeout)
        );
        eprintln!(
            "| Recovery reserve | 1/64, 10 ms | admitted={:#x} | period_waited={}, admission_refused={} | — |",
            self.max_core0_admitted,
            yn(self.recovery_period_waited),
            yn(self.admission_refused)
        );
        eprintln!(
            "| NODE_WATCH_SLOTS | 4 | {} | 5th_refused={} | — |",
            self.max_watch_slots,
            yn(self.watch_slot_5th_refused)
        );
        eprintln!(
            "| IDLE_STATES_MAX | 15 | {} | 16th_refused={} | — |",
            self.max_idle_states,
            yn(self.idle_16th_refused)
        );
        eprintln!(
            "| OBSERVATIONS_MAX | 8 | {} | 9th_dropped={} | — |",
            self.max_observations,
            yn(self.obs_9th_dropped)
        );
        eprintln!(
            "| RESERVES_MAX | 8 (min 2) | {} | — | — |",
            self.max_reserves
        );
        eprintln!(
            "| ROLES_PLAIN_MAX | 64 | {} | — | — |",
            self.max_plain_roles
        );
        eprintln!(
            "| RING_KERNEL_ENTRIES | 64 | {} | hit_reserve={}, used_reserve={}, full={} | hit={}, used={}, full={} |",
            self.max_kernel_ring_used,
            yn(self.kernel_ring_hit_reserve),
            yn(self.kernel_ring_used_reserve),
            yn(self.kernel_ring_full),
            yn(self.kernel_ring_hit_reserve_in_recovery),
            yn(self.kernel_ring_used_reserve_in_recovery),
            yn(self.kernel_ring_full_in_recovery)
        );
        eprintln!(
            "| RING_USER_ENTRIES | 64 | {} | full={} | — |",
            self.max_user_ring_used,
            yn(self.user_ring_full)
        );
        eprintln!(
            "| REQUESTS_PER_PEER | 16 | {} | 17th_refused={} | {} |",
            self.max_req_credits,
            yn(self.req_credit_17th_refused),
            yn(self.req_credit_17th_in_recovery)
        );
        eprintln!(
            "| SENDS_PER_PEER | 8 (6 no-manage) | {} | 7th_no_manage={}, 9th_refused={} | — |",
            self.max_send_credits,
            yn(self.send_credit_7th_no_manage_refused),
            yn(self.send_credit_9th_refused)
        );
        eprintln!(
            "| UNIT_REQUESTS | 64 | {} (service={}) | 65th_refused={} | {} |",
            self.max_unit_requests,
            self.max_unit_service,
            yn(self.unit_requests_65th_refused),
            yn(self.unit_requests_65th_in_recovery)
        );
        eprintln!(
            "| RING_ANSWER_RESERVE | 34 | k_used={} | hit_reserve={}, used_reserve={} | hit={}, used={} |",
            self.max_kernel_ring_used,
            yn(self.kernel_ring_hit_reserve),
            yn(self.kernel_ring_used_reserve),
            yn(self.kernel_ring_hit_reserve_in_recovery),
            yn(self.kernel_ring_used_reserve_in_recovery)
        );
        eprintln!(
            "| Report items / entries | 28 / 11 | {} / {} | 28_and_more={}, bad_rejected={} | — |",
            self.max_report_items,
            self.max_ring_msg_entries,
            yn(self.report_28_more),
            yn(self.report_bad_rejected)
        );
        eprintln!(
            "| IRQ_TABLE_ENTRIES / asks | 192 / 8 | handles={}, asks={} | 193rd_refused={} | — |",
            self.max_node_handles,
            self.max_rebuild_asks_per_node,
            yn(self.irq_193rd_refused)
        );
        eprintln!(
            "| REMOVAL_STEP / unmapping | 32 / 1 | — | step_32_again={}, unmapping_stop={} | — |",
            yn(self.removal_32_again),
            yn(self.removal_unmapping_stop)
        );
        eprintln!(
            "| REMOVAL_SLOTS | 128 | — | slots_128_again={} | — |",
            yn(self.removal_128_slots_again)
        );
        eprintln!(
            "| CSPACE_MAX_SLOTS | 2^20 (1048576) | max={}, grown={} | over_max_refused={} | — |",
            self.max_cspace_slots,
            self.max_cspace_grown_slots,
            yn(self.cspace_over_max_refused)
        );
        eprintln!(
            "| SCHED_MAX_DEBT | 2^26 (67108864) | {} | forgiven={} | — |",
            self.max_sched_debt,
            yn(self.sched_debt_forgiven)
        );
        eprintln!(
            "| Two sealed per sweep | 2 | sealed_at_once={} | capped_at_2={} | — |",
            self.max_sealed_nodes,
            yn(self.sweep_2_sealed_capped)
        );
        eprintln!(
            "| MAX_WORDS | 64 | {} | over_64_refused={} | — |",
            self.max_msg_words,
            yn(self.msg_words_over_64_refused)
        );
        eprintln!(
            "| MAX_CAPS / SEND_GRANTS_MAX | 4 | {} | over_4_refused={} | — |",
            self.max_send_grants,
            yn(self.send_grants_over_4_refused)
        );
        eprintln!(
            "| RING_USER_STEP | 16 | u_used={} | over_16_step={} | — |",
            self.max_user_ring_used,
            yn(self.user_ring_step_16)
        );
        eprintln!(
            "| PENDING_STEP | 32 | sig={}, rel={} | over_32_step={} | {} |",
            self.max_pending_signals,
            self.max_pending_releases,
            yn(self.pending_step_32),
            yn(self.pending_step_32_in_recovery)
        );
        eprintln!(
            "| IOSPACE_MAX_DEVICES | 16 | {} | 17th_refused={} | — |",
            self.max_iospace_devices,
            yn(self.iospace_17th_refused)
        );
        eprintln!(
            "| IOMMU_WAIT_NS | 10 ms | — | unit_failed={} | — |",
            yn(self.iommu_wait_timeout)
        );
        eprintln!(
            "| TREE_DEPTH / LONGEST_WALK | 64 / 191 | {} / {} | depth_65_refused={} | — |",
            self.max_tree_depth,
            self.max_walk,
            yn(self.tree_depth_65_refused)
        );
        eprintln!(
            "| GRANTS (grant/use table) | 4096 | grants={}, uses={} | grants_full={}, uses_dropped={} | grants={}, uses={} |",
            self.max_grants,
            self.max_uses,
            yn(self.grants_full_refused),
            yn(self.uses_full_dropped),
            yn(self.grants_full_in_recovery),
            yn(self.uses_dropped_in_recovery)
        );
        eprintln!(
            "| TIME_EVENT_STEP | 16 | due={} | over_16_step={} | {} |",
            self.max_due_timers,
            yn(self.time_event_step_16),
            yn(self.time_event_step_16_in_recovery)
        );
        eprintln!(
            "| CLEAR_STEP_BYTES | 65536 | — | again={} | — |",
            yn(self.clear_step_again)
        );
        eprintln!(
            "| DEBUG_WRITE_MAX | 512 | {} | over_512_refused={} | — |",
            self.max_debug_write,
            yn(self.debug_write_over_512_refused)
        );
        eprintln!(
            "| TRACE_BURST | 16 / 1 ms | — | suppressed={} | — |",
            yn(self.trace_burst_suppressed)
        );
        eprintln!(
            "| ENTROPY_MAX_BYTES | 64 | {} | over_64_refused={} | — |",
            self.max_entropy_bytes,
            yn(self.entropy_over_64_refused)
        );
        eprintln!(
            "| MAP_COUNT_MAX | 2^32-1 | — | max_refused={} | — |",
            yn(self.map_count_max_refused)
        );
    }
}
