//! The faulty-kernel actor: a node whose kernel is arbitrary code at
//! kernel privilege calls its own gate with any arguments in any order,
//! skips any kernel-side step (it never acknowledges, never unhooks,
//! never invalidates of its own accord) and writes any word its kernel
//! view maps writable. In the gate model every node may act so: the gate
//! alone must keep (G).

use super::*;

/// One gate call, or one write of a kernel-writable word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    TakeTable {
        n: u16,
        page: Page,
    },
    RetypeRoot {
        n: u16,
        page: Page,
    },
    Hook {
        n: u16,
        root: Page,
        e0: usize,
        child: Page,
    },
    Map {
        n: u16,
        root: Page,
        tag: usize,
        va: (usize, usize),
        page: Page,
        write: bool,
    },
    Unmap {
        n: u16,
        p: usize,
        root: Page,
        tag: usize,
        va: (usize, usize),
    },
    Unhook {
        n: u16,
        p: usize,
        root: Page,
        tag: usize,
        e0: usize,
    },
    TableFree {
        n: u16,
        page: Page,
    },
    Bind {
        n: u16,
        tag: usize,
        root: Page,
    },
    Load {
        n: u16,
        p: usize,
        root: Page,
        tag: usize,
    },
    Idle {
        n: u16,
        p: usize,
    },
    Ack {
        n: u16,
        p: usize,
    },
    Unbind {
        n: u16,
        p: usize,
        tag: usize,
    },
    Destroy {
        n: u16,
        p: usize,
        root: Page,
    },
    Teardown {
        n: u16,
        root: Page,
    },
    Take {
        n: u16,
        chunk: u64,
    },
    Barrier {
        n: u16,
        p: usize,
    },
    IoRoot {
        n: u16,
        page: Page,
    },
    ClaimSpi {
        n: u16,
        spi: usize,
    },
    SpiArrives {
        spi: usize,
    },
    AckSpi {
        n: u16,
        spi: usize,
    },
    RemoveSpi {
        n: u16,
        spi: usize,
    },
    Sealing {
        n: u16,
    },
    ParkCall {
        n: u16,
        p: usize,
    },
    SealedCall {
        n: u16,
        p: usize,
    },
    IoMap {
        n: u16,
        e: usize,
        page: Page,
    },
    IoUnmap {
        n: u16,
        entry: usize,
        e: usize,
    },
    Accept {
        h: u16,
        slot: u16,
        entry: usize,
    },
    Submit {
        h: u16,
    },
    /// The unit performs a submission (hardware).
    UnitCompletes {
        id: usize,
    },
    Answer {
        h: u16,
        service: usize,
    },
    Release {
        n: u16,
        entry: usize,
    },
    UnitFails,
    Grant {
        n: u16,
        slot: usize,
        page: Page,
        to: u16,
    },
    Deliver {
        to: u16,
        index: usize,
        granter: u16,
        slot: usize,
    },
    MapRemote {
        n: u16,
        root: Page,
        tag: usize,
        va: (usize, usize),
        index: usize,
    },
    Revoke {
        n: u16,
        slot: usize,
    },
    FreeGrant {
        n: u16,
        slot: usize,
    },
    Refuse {
        h: u16,
        slot: u16,
        entry: usize,
    },
    /// The global events, made by another node or the machine.
    Death {
        n: u16,
    },
    Fresh {
        n: u16,
    },
    Seal {
        n: u16,
    },
    Resume {
        n: u16,
    },
    Give {
        n: u16,
        p: usize,
        chunk: u64,
        to: u16,
    },
    /// A kernel's store into a page it maps writable (a data page).
    Write {
        n: u16,
        page: Page,
        value: u64,
    },
}

impl Machine {
    /// Applies one operation; the gate refuses or performs it.
    pub(crate) fn apply(&mut self, op: &Op) -> Gated {
        match *op {
            Op::TakeTable { n, page } => self.take_table(n, page, LEVELS - 1),
            Op::RetypeRoot { n, page } => self.retype_root(n, page),
            Op::Hook { n, root, e0, child } => self.hook(n, root, e0, child),
            Op::Map {
                n,
                root,
                tag,
                va,
                page,
                write,
            } => self.map(n, root, tag, va, page, write),
            Op::Unmap {
                n,
                p,
                root,
                tag,
                va,
            } => self.unmap(n, p, root, tag, va),
            Op::Unhook {
                n,
                p,
                root,
                tag,
                e0,
            } => self.unhook(n, p, root, tag, e0),
            Op::TableFree { n, page } => self.table_free(n, page),
            Op::Bind { n, tag, root } => self.bind(n, tag, root),
            Op::Load { n, p, root, tag } => self.load(n, p, root, tag),
            Op::Idle { n, p } => self.idle(n, p),
            Op::Ack { n, p } => self.ack(n, p),
            Op::Unbind { n, p, tag } => self.unbind(n, p, tag),
            Op::Destroy { n, p, root } => self.destroy(n, p, root),
            Op::Teardown { n, root } => self.teardown(n, root),
            Op::Take { n, chunk } => self.take(n, chunk),
            Op::Barrier { n, p } => self.barrier(n, p),
            Op::IoRoot { n, page } => self.retype_ioroot(n, page),
            Op::ClaimSpi { n, spi } => self.claim_spi(n, spi),
            Op::SpiArrives { spi } => self.spi_arrives(spi),
            Op::AckSpi { n, spi } => self.ack_spi(n, spi),
            Op::RemoveSpi { n, spi } => self.remove_spi(n, spi),
            Op::Sealing { n } => self.sealing(n),
            Op::ParkCall { n, p } => self.park_call(n, p),
            Op::SealedCall { n, p } => self.sealed_call(n, p),
            Op::IoMap { n, e, page } => self.io_map(n, e, page),
            Op::IoUnmap { n, entry, e } => self.io_unmap(n, entry, e),
            Op::Accept { h, slot, entry } => self.accept(h, slot, entry),
            Op::Submit { h } => self.submit(h),
            Op::UnitCompletes { id } => self.unit_completes(id),
            Op::Answer { h, service } => self.answer(h, service),
            Op::Release { n, entry } => self.release(n, entry),
            Op::UnitFails => self.unit_fails(),
            Op::Grant { n, slot, page, to } => self.grant(n, slot, page, to),
            Op::Deliver {
                to,
                index,
                granter,
                slot,
            } => self.deliver(to, index, granter, slot),
            Op::MapRemote {
                n,
                root,
                tag,
                va,
                index,
            } => self.map_remote(n, root, tag, va, index),
            Op::Revoke { n, slot } => self.revoke(n, slot),
            Op::FreeGrant { n, slot } => self.free_grant(n, slot),
            Op::Refuse { h, slot, entry } => self.refuse(h, slot, entry),
            Op::Death { n } => self.death(n),
            Op::Fresh { n } => self.fresh(n),
            Op::Seal { n } => self.seal(n),
            Op::Resume { n } => self.resume(n),
            Op::Give { n, p, chunk, to } => self.give(n, p, chunk, to),
            Op::Write { n, page, value } => {
                // Only a data page its node holds is mapped writable in a
                // kernel's view (G2): any other store faults.
                require(
                    self.holds(n, page)
                        && self.gate.classes.get(page as usize) == Some(&Class::Data),
                )?;
                self.kernel.data.insert(page, value);
                Ok(())
            }
        }
    }

    /// A step: the operation, then the hardware's caching.
    pub(crate) fn step(&mut self, op: &Op) -> Gated {
        let outcome = self.apply(op);
        self.fill();
        self.fill_unit();
        outcome
    }
}

/// A small generator (xorshift).
#[derive(Clone, Debug)]
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

impl Machine {
    /// A random operation of any node, any arguments.
    pub(crate) fn random_op(&self, rng: &mut Rng) -> Op {
        let nodes = self.layout.len() as u64;
        let n = rng.below(nodes) as u16;
        let cores = self.layout[usize::from(n)].cores.len() as u64;
        let p = rng.below(cores) as usize;
        let pages = self.chunks * PAGES_PER_CHUNK;
        // Mostly what the node has (its pages, roots, tables, bound tags),
        // sometimes anything: a faulty kernel names what it likes.
        let pick = |rng: &mut Rng, from: Vec<Page>| -> Page {
            if from.is_empty() || rng.below(4) == 0 {
                rng.below(pages)
            } else {
                from[rng.below(from.len() as u64) as usize]
            }
        };
        let held: Vec<Page> = (0..pages).filter(|&q| self.holds(n, q)).collect();
        let roots: Vec<Page> = held
            .iter()
            .copied()
            .filter(|&q| matches!(self.gate.classes[q as usize], Class::Root(_)))
            .collect();
        let tables: Vec<Page> = held
            .iter()
            .copied()
            .filter(|&q| matches!(self.gate.classes[q as usize], Class::Table { .. }))
            .collect();
        let page = pick(rng, held.clone());
        let root = pick(rng, roots);
        let child = pick(rng, tables);
        let bound: Vec<Page> = self.gate.nodes[usize::from(n)]
            .tags
            .iter()
            .enumerate()
            .filter(|(_, t)| t.root.is_some())
            .map(|(i, _)| i as u64)
            .collect();
        let tag = match bound.is_empty() || rng.below(3) == 0 {
            true => rng.below(TAGS as u64) as usize,
            false => bound[rng.below(bound.len() as u64) as usize] as usize,
        };
        let mut e0 = rng.below(ENTRIES as u64) as usize;
        let mut e1 = rng.below(ENTRIES as u64) as usize;
        // Mostly consistent: the root its tag holds, a hooked entry of it,
        // a leaf there; sometimes not.
        let mut root = root;
        if rng.below(4) != 0
            && let Some(bound_root) = self.gate.nodes[usize::from(n)].tags[tag].root
        {
            root = bound_root;
            if let Some(entries) = self.gate.tables.get(&root) {
                let hooked: Vec<usize> = (0..ENTRIES - 1)
                    .filter(|&i| matches!(entries[i], Entry::Table(_)))
                    .collect();
                if !hooked.is_empty() {
                    e0 = hooked[rng.below(hooked.len() as u64) as usize];
                    if let Entry::Table(table) = entries[e0]
                        && let Some(leaves) = self.gate.tables.get(&table)
                    {
                        let want_present = rng.below(2) == 0;
                        let fit: Vec<usize> = (0..ENTRIES)
                            .filter(|&i| matches!(leaves[i], Entry::Leaf { .. }) == want_present)
                            .collect();
                        if !fit.is_empty() {
                            e1 = fit[rng.below(fit.len() as u64) as usize];
                        }
                    }
                }
            }
        }
        let category = match rng.below(4) {
            0 => 15,
            1 => 16,
            _ => rng.below(15),
        };
        match category {
            0 => Op::TakeTable { n, page },
            1 => Op::RetypeRoot { n, page },
            2 => Op::Hook { n, root, e0, child },
            3 | 4 => Op::Map {
                n,
                root,
                tag,
                va: (e0, e1),
                page,
                write: rng.below(2) == 0,
            },
            5 | 6 => Op::Unmap {
                n,
                p,
                root,
                tag,
                va: (e0, e1),
            },
            7 if rng.below(2) == 0 => Op::Unhook {
                n,
                p,
                root,
                tag,
                e0,
            },
            7 => Op::TableFree { n, page },
            8 => Op::Bind { n, tag, root },
            9 => Op::Load { n, p, root, tag },
            10 => Op::Idle { n, p },
            11 => Op::Ack { n, p },
            12 => Op::Unbind { n, p, tag },
            13 => match rng.below(2) {
                0 => Op::Destroy { n, p, root },
                _ => Op::Teardown { n, root },
            },
            16 => {
                let slot = rng.below(grants::GRANTS as u64) as usize;
                let other = rng.below(nodes) as u16;
                match rng.below(5) {
                    0 => Op::Grant {
                        n,
                        slot,
                        page,
                        to: other,
                    },
                    1 => Op::Deliver {
                        to: n,
                        index: slot,
                        granter: other,
                        slot: rng.below(grants::GRANTS as u64) as usize,
                    },
                    2 => {
                        // A delivered use entry, and an absent leaf of a
                        // hooked table of the root its tag holds.
                        let used: Vec<usize> = (0..grants::GRANTS)
                            .filter(|&i| self.grants.uses[usize::from(n)][i].is_some())
                            .collect();
                        let index = match used.is_empty() {
                            true => slot,
                            false => used[rng.below(used.len() as u64) as usize],
                        };
                        let mut va = (e0, e1);
                        if let Some(entries) = self.gate.tables.get(&root)
                            && let Entry::Table(table) = entries[e0.min(ENTRIES - 1)]
                            && let Some(leaves) = self.gate.tables.get(&table)
                            && let Some(free) = (0..ENTRIES).find(|&i| leaves[i] == Entry::Absent)
                        {
                            va = (e0, free);
                        }
                        Op::MapRemote {
                            n,
                            root,
                            tag,
                            va,
                            index,
                        }
                    }
                    3 => Op::Revoke { n, slot },
                    _ => Op::FreeGrant { n, slot },
                }
            }
            15 => {
                let mut entry = rng.below(unit::UNIT_ENTRIES as u64) as usize;
                let mut slot = rng.below(nodes) as u16;
                let mut e1 = e1;
                // Mostly the unit's actual state: a present leaf, an open
                // record, a submitted service, a gate record.
                if rng.below(4) != 0 {
                    if let Some(entries) = self.unit.domains[usize::from(n)]
                        .and_then(|root| self.gate.tables.get(&root))
                    {
                        let present: Vec<usize> = (0..ENTRIES)
                            .filter(|&i| matches!(entries[i], Entry::Leaf { .. }))
                            .collect();
                        if !present.is_empty() && rng.below(2) == 0 {
                            e1 = present[rng.below(present.len() as u64) as usize];
                        }
                    }
                    let open: Vec<(u16, usize)> = (0..nodes as u16)
                        .flat_map(|s| (0..unit::UNIT_ENTRIES).map(move |i| (s, i)))
                        .filter(|&(s, i)| {
                            self.unit.records[usize::from(s)][i].state == unit::RecordState::Open
                        })
                        .collect();
                    if !open.is_empty() {
                        (slot, entry) = open[rng.below(open.len() as u64) as usize];
                    }
                    let mine: Vec<usize> = (0..unit::UNIT_ENTRIES)
                        .filter(|&i| self.unit.gate_records[usize::from(n)][i].is_some())
                        .collect();
                    if !mine.is_empty() && rng.below(2) == 0 {
                        entry = mine[rng.below(mine.len() as u64) as usize];
                    }
                }
                match rng.below(7) {
                    0 => Op::IoMap { n, e: e1, page },
                    1 => Op::IoUnmap { n, entry, e: e1 },
                    2 => Op::Accept { h: n, slot, entry },
                    3 if rng.below(20) == 0 => Op::UnitFails,
                    3 if rng.below(4) == 0 => Op::Refuse { h: n, slot, entry },
                    3 if rng.below(4) == 0 => Op::IoRoot { n, page },
                    3 => Op::Submit { h: n },
                    4 if rng.below(2) == 0 => Op::IoMap { n, e: e1, page },
                    4 => Op::UnitCompletes {
                        id: rng.below(self.unit.submissions.len() as u64 + 1) as usize,
                    },
                    5 => Op::Answer {
                        h: n,
                        service: rng.below(self.unit.services.len() as u64 + 1) as usize,
                    },
                    _ => Op::Release { n, entry },
                }
            }
            14 if rng.below(3) == 0 => {
                let spi = rng.below(parks::SPIS as u64) as usize;
                match rng.below(7) {
                    0 => Op::ClaimSpi { n, spi },
                    1 => Op::SpiArrives { spi },
                    2 => Op::AckSpi { n, spi },
                    3 => Op::RemoveSpi { n, spi },
                    4 => Op::Sealing { n },
                    5 => Op::ParkCall { n, p },
                    _ => Op::SealedCall { n, p },
                }
            }
            14 if rng.below(8) == 0 => match rng.below(5) {
                0 => Op::Death { n },
                1 => Op::Fresh { n },
                2 => Op::Seal { n },
                3 => Op::Resume { n },
                _ => Op::Barrier { n, p },
            },
            14 if rng.below(2) == 0 => {
                let chunk = rng.below(self.chunks);
                match rng.below(2) {
                    0 => Op::Take { n, chunk },
                    _ => Op::Give {
                        n,
                        p,
                        chunk,
                        to: rng.below(nodes) as u16,
                    },
                }
            }
            _ => Op::Write {
                n,
                page,
                value: rng.next(),
            },
        }
    }
}
