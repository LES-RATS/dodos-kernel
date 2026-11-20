//! The columns of the event tables of "Gate words" (D-152), for the
//! words of the CPU side: what a fresh start, a death by R-step, a seal
//! and a resume write. Each function executes its column cell by cell.

use super::*;

impl Machine {
    /// **Death**, by R-step: the fence (`devices.rs`), R1 by the unit's
    /// holder (a take-over first if the node held the role), then R6. A
    /// node whose R6 is refused stays fenced (its memory not reused).
    pub(crate) fn death(&mut self, n: u16) -> Gated {
        self.fence(n)?;
        self.recover(n);
        let _ = self.r6(n);
        Ok(())
    }

    /// **R6**, after the barrier: refused while the device side holds the
    /// generation (`r6_devices`); the dead node's use entries freed (R3
    /// pinned the grants to it at fenced), R6's proof on its grant entries; then the class bytes
    /// of every chunk taken by the generation and of its floor 0, its gate
    /// words whole 0 (the kept kernel tables unchanged: "no core of X
    /// runs, and the next fresh start writes them"); the member word dead.
    pub(crate) fn r6(&mut self, n: u16) -> Gated {
        let member = self.gate.members[usize::from(n)];
        require(member.state == NodeState::Fenced)?;
        // Every refusal judged before any write: the device side, and R6's
        // proof on each grant entry of the node (its use count; the node's
        // own use entries, freed below, count nothing of its grants).
        self.r6_devices_hold(n)?;
        for slot in 0..grants::GRANTS {
            if let Some(grant) = self.grants.grants[usize::from(n)][slot] {
                require(self.old.r6_frees_on_acked || self.use_released(n, slot, grant))?;
            }
        }
        self.r6_devices(n);
        // The dead node's use entries freed (its tables gone with R6 below),
        // then the granters free their pinned grants on them.
        self.grants.uses[usize::from(n)] = [None; grants::GRANTS];
        self.grants.remote.retain(|&(holder, _, _), _| holder != n);
        for granter in 0..self.layout.len() as u16 {
            for slot in 0..grants::GRANTS {
                if self.grants.grants[usize::from(granter)][slot].is_some_and(|g| g.grantee == n) {
                    let _ = self.release_grant(granter, slot);
                }
            }
        }
        // R6's proof: each grant entry of the dead node freed only on its
        // use count; one still counted stops the walk (the node stays
        // fenced, its memory not reused).
        for slot in 0..grants::GRANTS {
            let Some(grant) = self.grants.grants[usize::from(n)][slot] else {
                continue;
            };
            if self.old.r6_frees_on_acked {
                self.grants.grants[usize::from(n)][slot] = None;
                let chunk = Self::chunk_of(grant.page) as usize;
                self.gate.grant_counts[chunk] = self.gate.grant_counts[chunk].saturating_sub(1);
            } else if self.release_grant(n, slot).is_err() {
                return Err(Refused::Condition);
            }
        }
        // R6, the barrier holding.
        let (start, end) = self.layout[usize::from(n)].floor;
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
                if self.gate.classes[page as usize] != Class::Data {
                    self.gate.classes[page as usize] = Class::Data;
                    self.gate.tables.remove(&page);
                    self.gate.links.remove(&page);
                }
            }
            // R6 (D-154): the whole gate word, its three counts and its
            // table bit, of every chunk the generation took and of its
            // floor, ordinary and table.
            self.gate.map_counts[chunk as usize] = 0;
            self.gate.grant_counts[chunk as usize] = 0;
            self.gate.table_bits[chunk as usize] = false;
            self.gate.not_data[chunk as usize] = (first..first + PAGES_PER_CHUNK)
                .filter(|&q| self.gate.classes[q as usize] != Class::Data)
                .count() as u64;
        }
        self.forget_its_objects();
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
            if self.gate.classes[page as usize] != Class::Data {
                self.gate.classes[page as usize] = Class::Data;
                self.gate.tables.remove(&page);
                self.gate.links.remove(&page);
            }
        }
        // G13 (D-154): the whole gate word of its floor's chunks, 0 for its
        // ordinary floor, the table bit with counts 0 for its table floor.
        for chunk in Self::chunk_of(start)..Self::chunk_of(end) {
            let first = chunk * PAGES_PER_CHUNK;
            self.gate.map_counts[chunk as usize] = 0;
            self.gate.grant_counts[chunk as usize] = 0;
            self.gate.table_bits[chunk as usize] = first == layout.table_floor.0;
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
            let mut tags = vec![TagEntry::default(); ARCH_TAGS];
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
        self.forget_its_objects();
        // G13: its use table emptied; its serials never reset.
        self.grants.uses[usize::from(n)] = [None; grants::GRANTS];
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
        // The seal waits for the answers to its records sent (the request
        // record, cell Seal); judged before any write.
        require(!self.record_open(n))?;
        // Unit roles handed over (a target that runs; none, no seal): the
        // role word held by the target, its service records handed with the
        // unit.
        if self.unit.holder == n {
            let target = (0..self.layout.len() as u16)
                .find(|&s| s != n && self.gate.members[usize::from(s)].state == NodeState::Live)
                .ok_or(Refused::Condition)?;
            self.unit.holder = target;
            self.unit.epoch += 1;
        }
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
        self.writes_on(n, p)?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let node = &self.gate.nodes[usize::from(n)];
        let asked: BTreeSet<usize> = (0..node.loaded.len())
            .filter(|&q| q != p && !(self.old.kernel_run_skips_idle && node.idle[q].idle))
            .collect();
        self.start_request(n, p, Invalidates::Barrier, asked, Clearing::default())
    }
}
