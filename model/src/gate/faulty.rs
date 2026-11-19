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
        match rng.below(15) {
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
