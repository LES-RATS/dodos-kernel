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
    TableLink {
        n: u16,
        table: Page,
        next: Option<Page>,
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
    /// `unbind(0, R)` of the untagged mode, which names its root (D-158).
    UnbindRoot {
        n: u16,
        p: usize,
        root: Page,
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
        kind: Take,
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
    /// Authorized seal-initiation event (`node_stop` mode 1: live → sealing,
    /// which in the contract requires `node` `manage` and another live node),
    /// exposed separately from `Op::Seal` to test out-of-order seal steps.
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
        root: Page,
        e: usize,
        page: Page,
    },
    IoUnmap {
        n: u16,
        root: Page,
        entry: usize,
        e: usize,
    },
    IoAttach {
        n: u16,
        device: u16,
        root: Page,
        entry: usize,
    },
    IoDetach {
        n: u16,
        device: u16,
        entry: usize,
    },
    IoDestroy {
        n: u16,
        root: Page,
    },
    IoTeardown {
        n: u16,
        root: Page,
    },
    ItsRetype {
        n: u16,
        first: Page,
        k: u8,
    },
    /// A kernel load (`None`) or store through core `p`'s own view.
    KernelAccess {
        n: u16,
        p: usize,
        page: Page,
        value: Option<u64>,
    },
    ItsClaim {
        n: u16,
        device: u16,
        first: Page,
        event: u32,
        entry: usize,
    },
    ItsUnmap {
        n: u16,
        device: u16,
        entry: usize,
    },
    ItsDestroy {
        n: u16,
        first: Page,
    },
    ClaimMsi {
        n: u16,
        core: usize,
        v: usize,
    },
    RemoveMsi {
        n: u16,
        core: usize,
        v: usize,
        entry: usize,
    },
    TakeEntry {
        n: u16,
        entry: usize,
    },
    GiveEntry {
        n: u16,
        entry: usize,
    },
    SplitEntry {
        n: u16,
        entry: usize,
    },
    DeviceLookup {
        n: u16,
        entry: usize,
    },
    PortIo {
        n: u16,
        port: u16,
    },
    Spurious {
        intid: u32,
    },
    /// Death's steps, by the fence's driver and the unit's holder.
    Fence {
        n: u16,
    },
    TakeOver {
        s: u16,
    },
    R1Start {
        h: u16,
        x: u16,
    },
    R1Unit {
        h: u16,
        x: u16,
    },
    R1Done {
        h: u16,
        x: u16,
    },
    R6 {
        n: u16,
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
    /// Hardware/holder event: the unit failed to confirm within `IOMMU_WAIT_NS`
    /// and its holder's gate set bit 63 (*failed*) of the role word (Roles).
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
    PinRevoking {
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
    /// The destruction of the block of a chunk (its run unmapped, its word
    /// written *free* with the same owner).
    DestroyBlock {
        n: u16,
        p: usize,
        chunk: u64,
    },
    /// An interrupt to core `target` with `vector` (G10).
    Ipi {
        n: u16,
        target: usize,
        vector: u8,
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
            Op::KernelAccess { n, p, page, value } => self.kernel_access(n, p, page, value),
            Op::Ipi { n, target, vector } => self.send_ipi(n, target, vector),
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
            Op::TableLink { n, table, next } => self.table_link(n, table, next),
            Op::Bind { n, tag, root } => self.bind(n, tag, root),
            Op::Load { n, p, root, tag } => self.load(n, p, root, tag),
            Op::Idle { n, p } => self.idle(n, p),
            Op::Ack { n, p } => self.ack(n, p),
            Op::Unbind { n, p, tag } => self.unbind(n, p, tag),
            Op::UnbindRoot { n, p, root } => self.unbind_root(n, p, root),
            Op::Destroy { n, p, root } => self.destroy(n, p, root),
            Op::Teardown { n, root } => self.teardown(n, root),
            Op::Take { n, chunk, kind } => self.take(n, chunk, kind),
            Op::Barrier { n, p } => self.barrier(n, p),
            Op::IoRoot { n, page } => self.retype_ioroot(n, page),
            Op::ClaimSpi { n, spi } => self.claim_spi(n, spi),
            Op::SpiArrives { spi } => self.spi_arrives(spi),
            Op::AckSpi { n, spi } => self.ack_spi(n, spi),
            Op::RemoveSpi { n, spi } => self.remove_spi(n, spi),
            Op::Sealing { n } => self.sealing(n),
            Op::ParkCall { n, p } => self.park_call(n, p),
            Op::SealedCall { n, p } => self.sealed_call(n, p),
            Op::IoMap { n, root, e, page } => self.io_map(n, root, e, page),
            Op::IoUnmap { n, root, entry, e } => self.io_unmap(n, root, entry, e),
            Op::IoAttach {
                n,
                device,
                root,
                entry,
            } => self.iospace_attach(n, device, root, entry),
            Op::IoDetach { n, device, entry } => self.iospace_detach(n, device, entry),
            Op::IoDestroy { n, root } => self.io_destroy(n, root),
            Op::IoTeardown { n, root } => self.io_teardown(n, root),
            Op::ItsRetype { n, first, k } => self.retype_its(n, first, k),
            Op::ItsClaim {
                n,
                device,
                first,
                event,
                entry,
            } => self.its_claim(n, device, first, event, entry),
            Op::ItsUnmap { n, device, entry } => self.its_unmap(n, device, entry),
            Op::ItsDestroy { n, first } => self.its_destroy(n, first),
            Op::ClaimMsi { n, core, v } => self.claim_msi(n, core, v),
            Op::RemoveMsi { n, core, v, entry } => self.remove_msi(n, core, v, entry),
            Op::TakeEntry { n, entry } => self.take_entry(n, entry),
            Op::GiveEntry { n, entry } => self.give_entry(n, entry),
            Op::SplitEntry { n, entry } => self.split_entry(n, entry),
            Op::DeviceLookup { n, entry } => self.device_lookup(n, entry),
            Op::PortIo { n, port } => self.port_io(n, port),
            Op::Spurious { intid } => self.arrival_spurious(intid),
            Op::Fence { n } => self.fence(n),
            Op::TakeOver { s } => self.take_over(s),
            Op::R1Start { h, x } => self.r1_start(h, x),
            Op::R1Unit { h, x } => self.r1_unit(h, x),
            Op::R1Done { h, x } => self.r1_done(h, x),
            Op::R6 { n } => self.r6(n),
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
            Op::PinRevoking { n, slot } => self.pin_revoking(n, slot),
            Op::FreeGrant { n, slot } => self.free_grant(n, slot),
            Op::Refuse { h, slot, entry } => self.refuse(h, slot, entry),
            Op::Death { n } => self.death(n),
            Op::Fresh { n } => self.fresh(n),
            Op::Seal { n } => self.seal(n),
            Op::Resume { n } => self.resume(n),
            Op::Give { n, p, chunk, to } => self.give(n, p, chunk, to),
            Op::DestroyBlock { n, p, chunk } => self.destroy_block(n, p, chunk),
            Op::Write { n, page, value } => {
                // Only a data page in an ordinary (non-table) chunk its node
                // holds is mapped writable in a kernel's view (G2, D-154): any
                // other store faults.
                require(
                    page < self.chunks * PAGES_PER_CHUNK
                        && self.holds(n, page)
                        && !self.gate.table_bits[Self::chunk_of(page) as usize]
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

impl Machine {
    /// The next step a healthy holder and requester take on the first
    /// record that is open or answered and not yet released.
    fn healthy_holder_step(&self) -> Option<Op> {
        let h = self.unit.holder;
        for slot in 0..self.layout.len() as u16 {
            for entry in 0..unit::UNIT_ENTRIES {
                let record = self.unit.records[usize::from(slot)][entry];
                let gate = &self.unit.gate_records[usize::from(slot)][entry];
                if record.state != unit::RecordState::Open {
                    if gate.is_some() && record.state != unit::RecordState::Void {
                        return Some(Op::Release { n: slot, entry });
                    }
                    continue;
                }
                let service =
                    self.unit.services.iter().position(|s| {
                        s.slot == slot && s.entry == entry && s.serial == record.serial
                    });
                return Some(match service {
                    None => Op::Accept { h, slot, entry },
                    Some(i) => match self.unit.services[i].submission {
                        None => Op::Submit { h },
                        Some(id) if !self.unit.submissions[id].complete => Op::UnitCompletes {
                            id: self
                                .unit
                                .submissions
                                .iter()
                                .position(|s| !s.complete)
                                .unwrap_or(id),
                        },
                        Some(_) => Op::Answer { h, service: i },
                    },
                });
            }
        }
        None
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
        // The page a call turns from *data*: mostly a *data* page of a
        // table chunk the node holds, sometimes any page, one of an
        // ordinary chunk among them (D-154: refused).
        let loose: Vec<Page> = held
            .iter()
            .copied()
            .filter(|&q| self.table_chunk(q) && self.gate.classes[q as usize] == Class::Data)
            .collect();
        let tpage = match rng.below(5) {
            0 => page,
            _ => pick(rng, loose),
        };
        let root = pick(rng, roots.clone());
        let child = pick(rng, tables);
        // The tags of the architecture: the table's and the gate's own.
        let arch_tags = self.gate.nodes[usize::from(n)].tags.len().max(2);
        let bound: Vec<Page> = self.gate.nodes[usize::from(n)]
            .tags
            .iter()
            .enumerate()
            .filter(|(_, t)| t.root.is_some())
            .map(|(i, _)| i as u64)
            .collect();
        let tag = match bound.is_empty() || rng.below(3) == 0 {
            true => rng.below(arch_tags as u64) as usize,
            false => bound[rng.below(bound.len() as u64) as usize] as usize,
        };
        // Untagged, every root is named with tag 0 (D-158), and the root a
        // call mostly names is a *live* one of the node, which no entry of
        // the tag table holds.
        let untagged = self.is_untagged();
        let live: Vec<Page> = held
            .iter()
            .copied()
            .filter(|&q| self.gate.classes[q as usize] == Class::Root(RootState::Live))
            .collect();
        // Untagged, now and then another node's *live* root, which a faulty
        // kernel names though its node does not own it; and mostly a root
        // other than the one core `p` has loaded, so that a load leaves
        // translations of the root before (finding 7 of the review of
        // passes 33 and 34: neither the ownership in `tag_holds` nor the
        // invalidation of a load was seen by this exploration).
        let foreign: Vec<Page> = (0..pages)
            .filter(|&q| self.gate.classes[q as usize] == Class::Root(RootState::Live))
            .filter(|&q| !self.holds(n, q))
            .collect();
        let loaded = self.gate.nodes[usize::from(n)].loaded[p].0;
        let others: Vec<Page> = live.iter().copied().filter(|&q| q != loaded).collect();
        let named = match untagged {
            true if !foreign.is_empty() && rng.below(6) == 0 => {
                Some(foreign[rng.below(foreign.len() as u64) as usize])
            }
            true if !others.is_empty() => Some(others[rng.0 as usize % others.len()]),
            true => (!live.is_empty()).then(|| live[rng.0 as usize % live.len()]),
            false => self.gate.nodes[usize::from(n)].tags[tag].root,
        };
        let mut e0 = rng.below(ENTRIES as u64) as usize;
        let mut e1 = rng.below(ENTRIES as u64) as usize;
        // Mostly consistent: the root its tag holds, a hooked entry of it,
        // a leaf there; sometimes not.
        let mut root = root;
        if rng.below(4) != 0
            && let Some(bound_root) = named
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
        // Now and then an argument out of its range (pass 32, lot 6's
        // M-5): a position at which the node has no core, a tag beyond the
        // tag table, a node or a chunk that does not exist, a run that ends
        // beyond the address space. The gate refuses each. The generator's
        // state is read here, not advanced, so every other step of a seed
        // is what it was.
        let wild = rng.0.is_multiple_of(97);
        let own_core = self.core(n, p);
        let (p, tag) = match wild {
            true => (p + cores as usize, tag + arch_tags),
            false => (p, tag),
        };
        let beyond = |value: u64, range: u64| value + u64::from(wild) * range;
        let category = match rng.below(5) {
            0 => 15,
            1 => 16,
            2 => 17,
            // Untagged, a root load more often: a core that changes roots
            // with translations cached is what the load's invalidation is
            // for (finding 7 of the review of passes 33 and 34).
            _ if untagged && rng.below(3) == 0 => 9,
            _ => rng.below(15),
        };
        match category {
            0 => Op::TakeTable { n, page: tpage },
            1 if rng.below(2) == 0 => Op::RetypeRoot { n, page: tpage },
            // An interrupt to any core, mostly with an inter-kernel vector,
            // sometimes with any (an NMI's, a claimable one).
            1 if rng.below(4) == 0 => Op::Ipi {
                n,
                target: rng.below(self.tlb.len() as u64 + 1) as usize,
                vector: match rng.below(3) {
                    0 => rng.below(256) as u8,
                    _ => dodos_abi::x86_64::IKV_FIRST + rng.below(4) as u8,
                },
            },
            1 => Op::KernelAccess {
                n,
                p,
                page,
                value: (rng.below(2) == 0).then(|| rng.next()),
            },
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
            7 if rng.below(2) == 0 => Op::TableLink {
                n,
                table: child,
                next: (rng.below(3) != 0).then_some(page),
            },
            7 => Op::TableFree { n, page },
            // Untagged, mostly `bind(0, R)` of an unbound root of the node,
            // so that a node has several live roots for its cores to load
            // in turn (finding 7 of the review of passes 33 and 34).
            8 if untagged && rng.below(4) != 0 => {
                let unbound: Vec<Page> = roots
                    .iter()
                    .copied()
                    .filter(|&q| self.gate.classes[q as usize] == Class::Root(RootState::Unbound))
                    .collect();
                let root = match unbound.is_empty() {
                    true => root,
                    false => unbound[rng.below(unbound.len() as u64) as usize],
                };
                Op::Bind { n, tag: 0, root }
            }
            8 => Op::Bind { n, tag, root },
            9 => Op::Load { n, p, root, tag },
            10 => Op::Idle { n, p },
            11 => Op::Ack { n, p },
            12 if untagged => Op::UnbindRoot { n, p, root },
            12 => Op::Unbind { n, p, tag },
            13 => match rng.below(2) {
                0 => Op::Destroy { n, p, root },
                _ => Op::Teardown { n, root },
            },
            16 => {
                let slot = rng.below(grants::GRANTS as u64) as usize;
                let other = rng.below(nodes) as u16;
                match rng.below(6) {
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
                    4 => Op::PinRevoking { n, slot },
                    _ => Op::FreeGrant { n, slot },
                }
            }
            17 => {
                let device = rng.below(u64::from(devices::DEVICES)) as u16;
                let entry = rng.below(unit::UNIT_ENTRIES as u64) as usize;
                // Death's steps mostly by the holder, for a fenced node.
                let fenced: Vec<u16> = (0..nodes as u16)
                    .filter(|&x| self.gate.members[usize::from(x)].state == NodeState::Fenced)
                    .collect();
                let x = match fenced.is_empty() || rng.below(4) == 0 {
                    true => rng.below(nodes) as u16,
                    false => fenced[rng.below(fenced.len() as u64) as usize],
                };
                let h = match rng.below(4) {
                    0 => n,
                    _ => self.unit.holder,
                };
                let objects: Vec<Page> = self.dev.its_objects.keys().copied().collect();
                let first = pick(rng, objects);
                let core = match rng.below(4) {
                    0 => rng.below(self.tlb.len() as u64) as usize,
                    _ => own_core,
                };
                let v = rng.below(devices::VECTORS as u64) as usize;
                // Half the time, the next step a healthy holder takes for a
                // fenced node (the walk, its submission and completion,
                // done, R6), then its fresh start once dead.
                if !fenced.is_empty() && rng.below(2) == 0 {
                    let word = self.dev.r1[usize::from(x)];
                    let g = self.gate.members[usize::from(x)].generation;
                    let end = self.r1_units(x).len();
                    let h = self.unit.holder;
                    let pending = self.unit.submissions.iter().position(|s| !s.complete);
                    let confirmed = self
                        .unit
                        .submissions
                        .iter()
                        .any(|s| s.complete && s.r1.contains(&(x, g)));
                    return match word.state {
                        _ if !self.gate.members[usize::from(h)].state.writes() => {
                            Op::TakeOver { s: n }
                        }
                        devices::R1State::Done if word.generation == g => Op::R6 { n: x },
                        devices::R1State::Walking(c) if word.generation == g && c < end => {
                            Op::R1Unit { h, x }
                        }
                        devices::R1State::Walking(_) if word.generation == g => {
                            match (confirmed || self.unit.failed, pending) {
                                (true, _) => Op::R1Done { h, x },
                                (false, Some(id)) => Op::UnitCompletes { id },
                                (false, None) => Op::Submit { h },
                            }
                        }
                        _ => Op::R1Start { h, x },
                    };
                }
                // A quarter of the time, a healthy holder's next step on the
                // first open or answered record, or a sealed node resumed.
                if rng.below(4) == 0 {
                    if let Some(sealed) = (0..nodes as u16)
                        .find(|&x| self.gate.members[usize::from(x)].state == NodeState::Sealed)
                        && rng.below(3) == 0
                    {
                        return Op::Resume { n: sealed };
                    }
                    if let Some(op) = self.healthy_holder_step() {
                        return op;
                    }
                }
                let dead: Vec<u16> = (0..nodes as u16)
                    .filter(|&x| self.gate.members[usize::from(x)].state == NodeState::Dead)
                    .collect();
                if !dead.is_empty() && rng.below(3) == 0 {
                    return Op::Fresh {
                        n: dead[rng.below(dead.len() as u64) as usize],
                    };
                }
                if rng.below(4) == 0 {
                    let entry = rng.below(self.ports.entries.len() as u64 + 1) as usize;
                    let port = ports::PORTS[rng.below(ports::PORTS.len() as u64) as usize];
                    return match rng.below(6) {
                        0 => Op::TakeEntry { n, entry },
                        1 => Op::GiveEntry { n, entry },
                        2 => Op::SplitEntry { n, entry },
                        3 => Op::DeviceLookup { n, entry },
                        4 => Op::PortIo { n, port },
                        _ => Op::Spurious {
                            intid: 1018 + rng.below(8) as u32,
                        },
                    };
                }
                match rng.below(12) {
                    0 => Op::ItsRetype {
                        n,
                        first: match wild {
                            true => u64::MAX - tpage,
                            false => tpage,
                        },
                        k: rng.below(3) as u8,
                    },
                    1 => Op::ItsClaim {
                        n,
                        device,
                        first,
                        event: rng.below(6) as u32,
                        entry,
                    },
                    // Mostly a device whose ITS word is set, with a free
                    // entry of the node.
                    2 => {
                        let set: Vec<u16> = (0..devices::DEVICES)
                            .filter(|&d| self.dev.its[usize::from(d)].0 != devices::Bound::Zero)
                            .collect();
                        let device = match set.is_empty() || rng.below(4) == 0 {
                            true => device,
                            false => set[rng.below(set.len() as u64) as usize],
                        };
                        let entry = (0..unit::UNIT_ENTRIES)
                            .find(|&e| self.unit.gate_records[usize::from(n)][e].is_none())
                            .unwrap_or(entry);
                        Op::ItsUnmap { n, device, entry }
                    }
                    3 => Op::ItsDestroy { n, first },
                    4 => Op::ClaimMsi { n, core, v },
                    5 => Op::RemoveMsi { n, core, v, entry },
                    6 if rng.below(40) == 0 => Op::Fence { n: x },
                    6 => Op::TakeOver { s: n },
                    7 => Op::R1Start { h, x },
                    8 | 9 => Op::R1Unit { h, x },
                    10 => match rng.below(3) {
                        0 => Op::Submit { h },
                        1 => Op::UnitCompletes {
                            id: self
                                .unit
                                .submissions
                                .iter()
                                .position(|s| !s.complete)
                                .unwrap_or(0),
                        },
                        _ => Op::R1Done { h, x },
                    },
                    _ => Op::R6 { n: x },
                }
            }
            15 => {
                let mut entry = rng.below(unit::UNIT_ENTRIES as u64) as usize;
                let mut slot = rng.below(nodes) as u16;
                let mut e1 = e1;
                // Mostly the unit's actual state: a present leaf, an open
                // record, a submitted service, a gate record.
                let ioroots: Vec<Page> = held
                    .iter()
                    .copied()
                    .filter(|&q| matches!(self.gate.classes[q as usize], Class::IoRoot { .. }))
                    .collect();
                let ioroot = pick(rng, ioroots);
                if rng.below(4) != 0 {
                    if let Some(entries) = self.gate.tables.get(&ioroot) {
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
                let device = rng.below(u64::from(devices::DEVICES)) as u16;
                match rng.below(7) {
                    0 => Op::IoMap {
                        n,
                        root: ioroot,
                        e: e1,
                        page,
                    },
                    1 => Op::IoUnmap {
                        n,
                        root: ioroot,
                        entry,
                        e: e1,
                    },
                    2 => Op::Accept { h: n, slot, entry },
                    3 if rng.below(80) == 0 => Op::UnitFails,
                    3 if rng.below(4) == 0 => Op::Refuse { h: n, slot, entry },
                    3 if rng.below(4) == 0 => Op::IoRoot { n, page: tpage },
                    3 => Op::Submit { h: n },
                    4 if rng.below(2) == 0 => match rng.below(4) {
                        0 => Op::IoAttach {
                            n,
                            device,
                            root: ioroot,
                            entry,
                        },
                        1 => Op::IoDetach { n, device, entry },
                        2 => Op::IoDestroy { n, root: ioroot },
                        _ => Op::IoTeardown { n, root: ioroot },
                    },
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
                // Mostly a sealed node, the one a resume can concern.
                3 => Op::Resume {
                    n: (0..nodes as u16)
                        .find(|&x| self.gate.members[usize::from(x)].state == NodeState::Sealed)
                        .filter(|_| rng.below(4) != 0)
                        .unwrap_or(n),
                },
                _ => Op::Barrier { n, p },
            },
            14 if rng.below(2) == 0 => {
                let chunk = beyond(rng.below(self.chunks), self.chunks);
                match rng.below(2) {
                    // A third of the takes are of kind 2, a table chunk.
                    0 => Op::Take {
                        n,
                        chunk,
                        kind: match rng.below(3) {
                            0 => Take::Tables,
                            _ => Take::Ordinary,
                        },
                    },
                    // A give, or one time in four a block's destruction
                    // (the generator's state read, not advanced). Three in four of
                    // them name a chunk the node holds, table chunks
                    // among them (pass 35: no give or destruction
                    // happened with a chunk drawn from all of them).
                    _ => {
                        let to = beyond(rng.below(nodes), nodes) as u16;
                        let held: Vec<u64> = (0..self.chunks)
                            .filter(|&c| {
                                matches!(self.gate.owners[c as usize],
                                    Owner::Taken { node, .. } if node == n)
                            })
                            .collect();
                        let chunk = match held.is_empty() || rng.below(4) == 0 {
                            true => chunk,
                            false => held[rng.below(held.len() as u64) as usize],
                        };
                        match rng.0.is_multiple_of(4) {
                            true => Op::DestroyBlock { n, p, chunk },
                            false => Op::Give { n, p, chunk, to },
                        }
                    }
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
