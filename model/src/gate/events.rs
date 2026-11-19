//! The columns of the event tables of "Gate words" (D-152), for the
//! words of the CPU side: what a fresh start, a death by R-step, a seal
//! and a resume write. Each function executes its column cell by cell.

use super::*;

impl Machine {
    /// **Death**, by R-step: the member word fencing (the node's gate
    /// refuses every write from now), its cores stopped (a stopped core
    /// uses nothing it cached; it invalidates its whole TLB when it next
    /// enters a node), then, after the barrier, R6: the class bytes of
    /// every chunk taken by the generation and of its floor 0, its gate
    /// words whole 0 (the kept kernel tables unchanged: "no core of X
    /// runs, and the next fresh start writes them"); the member word dead.
    pub(crate) fn death(&mut self, n: u16) -> Gated {
        let member = self.gate.members[usize::from(n)];
        require(member.state.writes() || member.state == NodeState::Sealed)?;
        self.gate.members[usize::from(n)].state = NodeState::Fencing;
        for p in 0..self.layout[usize::from(n)].cores.len() {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
        self.gate.members[usize::from(n)].state = NodeState::Fenced;
        // R1 by the unit's holder: the node's devices detached, their
        // domain invalidated at the unit's confirmation; on a failed unit a
        // device stays attached and the node stays fenced (the barrier
        // never holds).
        let devices: Vec<u16> = self
            .unit
            .devices
            .iter()
            .filter(|&(_, &owner)| owner == n)
            .map(|(&d, _)| d)
            .collect();
        if !devices.is_empty() && self.unit.failed {
            return Ok(());
        }
        for device in devices {
            self.unit.contexts.remove(&device);
            self.unit.devices.remove(&device);
        }
        self.iotlb.retain(|&(domain, _), _| domain != n);
        self.unit.domains[usize::from(n)] = None;
        // The holder drops its services of the node's records at its own
        // R0, later: a late answer may still come, judged by the record's
        // compare-and-swap.
        // R6, the barrier holding.
        let (start, end) = self.layout[usize::from(n)].floor;
        let layout = self.layout[usize::from(n)].clone();
        for chunk in 0..self.chunks {
            let first = chunk * PAGES_PER_CHUNK;
            let taken = matches!(
                self.gate.owners[chunk as usize],
                Owner::Taken { node, generation } if node == n && generation == member.generation
            );
            let floor = start <= first && first < end;
            if !(taken || floor) {
                continue;
            }
            for page in first..first + PAGES_PER_CHUNK {
                if page == layout.kernel_root || page == layout.kernel_table {
                    continue;
                }
                if self.gate.classes[page as usize] != Class::Data {
                    self.gate.classes[page as usize] = Class::Data;
                    self.gate.tables.remove(&page);
                }
            }
            self.gate.map_counts[chunk as usize] = 0;
            self.gate.grant_counts[chunk as usize] = 0;
            self.gate.not_data[chunk as usize] = (first..first + PAGES_PER_CHUNK)
                .filter(|&q| self.gate.classes[q as usize] != Class::Data)
                .count() as u64;
        }
        self.gate.members[usize::from(n)].state = NodeState::Dead;
        Ok(())
    }

    /// **Fresh**: dead (*g*) → starting (*g* + 1). G13 writes 0 the class
    /// bytes and the gate words of its floor; the kernel tables written
    /// anew (the kernel table's leaves of the dead generation's runs gone);
    /// the gate part zeroed: every core's loaded root the kernel's, the tag
    /// table zeroed with entry 0 the kernel's root, every request *free*,
    /// pending sets and idle words 0. Each core invalidates its whole TLB
    /// as it enters.
    pub(crate) fn fresh(&mut self, n: u16) -> Gated {
        let member = self.gate.members[usize::from(n)];
        require(member.state == NodeState::Dead)?;
        let layout = self.layout[usize::from(n)].clone();
        let (start, end) = layout.floor;
        for page in start..end {
            if page == layout.kernel_root || page == layout.kernel_table {
                continue;
            }
            if self.gate.classes[page as usize] != Class::Data {
                self.gate.classes[page as usize] = Class::Data;
                self.gate.tables.remove(&page);
            }
        }
        for chunk in Self::chunk_of(start)..Self::chunk_of(end) {
            let first = chunk * PAGES_PER_CHUNK;
            self.gate.map_counts[chunk as usize] = 0;
            self.gate.grant_counts[chunk as usize] = 0;
            self.gate.not_data[chunk as usize] = (first..first + PAGES_PER_CHUNK)
                .filter(|&q| self.gate.classes[q as usize] != Class::Data)
                .count() as u64;
        }
        let mut root = [Entry::Absent; ENTRIES];
        root[ENTRIES - 1] = Entry::Shared(layout.kernel_table);
        self.gate.tables.insert(layout.kernel_root, root);
        self.gate
            .tables
            .insert(layout.kernel_table, [Entry::Absent; ENTRIES]);
        self.gate.members[usize::from(n)] = Member {
            state: NodeState::Starting { resume: false },
            generation: member.generation + 1,
        };
        if !self.old.fresh_keeps_gate_part {
            let cores = layout.cores.len();
            let mut tags = vec![TagEntry::default(); TAGS];
            tags[0].root = Some(layout.kernel_root);
            self.gate.nodes[usize::from(n)] = NodeGate {
                loaded: vec![(layout.kernel_root, 0); cores],
                tags,
                requests: vec![Request::Free; cores],
                pending: vec![BTreeSet::new(); cores],
                idle: vec![IdleWord::default(); cores],
            };
        }
        for p in 0..layout.cores.len() {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
        self.fresh_unit(n);
        self.parks.drop[usize::from(n)] = [false; parks::SPIS];
        for p in 0..layout.cores.len() {
            let core = self.core(n, p);
            self.parks.parked[core] = false;
        }
        self.gate.members[usize::from(n)].state = NodeState::Live;
        Ok(())
    }

    /// **Seal**: live → sealing → sealed. The seal completes its cores'
    /// requests before the park (the call refuses while one is in use);
    /// each core's park call loads the kernel's root.
    pub(crate) fn seal(&mut self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state == NodeState::Live)?;
        let node = &self.gate.nodes[usize::from(n)];
        require(node.requests.iter().all(|r| *r == Request::Free))?;
        self.sealing(n)?;
        let kernel = self.layout[usize::from(n)].kernel_root;
        let cores = self.layout[usize::from(n)].cores.len();
        for p in 0..cores {
            self.gate.nodes[usize::from(n)].loaded[p] = (kernel, 0);
            if p + 1 < cores {
                self.park_call(n, p)?;
            }
        }
        self.sealed_call(n, cores - 1)
    }

    /// **Resume**: sealed → starting (*g*), resume bit 1 → live; the
    /// kernel's root at the entry stub; the gate part unchanged (its
    /// requests all *free* since the seal). Each core invalidates its
    /// whole TLB as it enters.
    pub(crate) fn resume(&mut self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state == NodeState::Sealed)?;
        // A resume needs `seal_parked` equal to the park request on every
        // core.
        let parked = (0..self.layout[usize::from(n)].cores.len()).all(|p| {
            let core = self.core(n, p);
            self.parks.seal_parked[core] == self.parks.park_requests[core]
        });
        require(parked)?;
        self.gate.members[usize::from(n)].state = NodeState::Starting { resume: true };
        for p in 0..self.layout[usize::from(n)].cores.len() {
            let core = self.core(n, p);
            self.parks.parked[core] = false;
        }
        for p in 0..self.layout[usize::from(n)].cores.len() {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
        self.gate.members[usize::from(n)].state = NodeState::Live;
        Ok(())
    }

    /// The barrier before a watermark returns: a request that asks every
    /// other core of the node, idle ones included, and invalidates
    /// nothing.
    pub(crate) fn barrier(&mut self, n: u16, p: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let node = &self.gate.nodes[usize::from(n)];
        let asked: BTreeSet<usize> = (0..node.loaded.len())
            .filter(|&q| q != p && !(self.old.kernel_run_skips_idle && node.idle[q].idle))
            .collect();
        self.start_request(n, p, Invalidates::Barrier, asked, Clearing::default())
    }
}
