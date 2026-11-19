//! The invariants of (G) held against every node, faulty ones included,
//! after every step of the gate model: no translation of a core reaches
//! memory its node does not hold, or memory whose count the gate has
//! lowered while the translation can still be used; no cached table is
//! anything but a table of the node; every word reads a state of its
//! table.

use super::*;

pub(crate) type Check = Result<(), String>;

fn ensure(cond: bool, rule: &str) -> Check {
    if cond { Ok(()) } else { Err(rule.to_string()) }
}

impl Machine {
    /// Every invariant.
    pub(crate) fn check(&self) -> Check {
        self.check_classes()?;
        self.check_tags()?;
        self.check_counts()?;
        self.check_reach()
    }

    /// The class byte and the non-data count.
    fn check_classes(&self) -> Check {
        for chunk in 0..self.chunks {
            let first = chunk * PAGES_PER_CHUNK;
            let counted = (first..first + PAGES_PER_CHUNK)
                .filter(|&p| self.gate.classes[p as usize] != Class::Data)
                .count() as u64;
            ensure(
                counted == self.gate.not_data[chunk as usize],
                "GATE WORD: the count of pages not data",
            )?;
        }
        for (page, class) in self.gate.classes.iter().enumerate() {
            let has_entries = self.gate.tables.contains_key(&(page as u64));
            ensure(
                has_entries == (*class != Class::Data),
                "CLASS: a table's entries and its class byte disagree",
            )?;
        }
        Ok(())
    }

    /// The tag table: a bound root reads *live* or *dying*; a root is
    /// bound to one tag at most; a core's loaded root is the root its
    /// loaded tag's entry holds, or the kernel's with tag 0.
    fn check_tags(&self) -> Check {
        for (n, node) in self.gate.nodes.iter().enumerate() {
            // A dead or accused node's gate part is never read again (its
            // death column: unchanged, zeroed at the next fresh start).
            let state = self.gate.members[n].state;
            if matches!(
                state,
                NodeState::Dead | NodeState::Fencing | NodeState::Fenced
            ) {
                continue;
            }
            for (tag, entry) in node.tags.iter().enumerate().skip(1) {
                if let Some(root) = entry.root {
                    let class = self.gate.classes[root as usize];
                    ensure(
                        matches!(class, Class::Root(RootState::Live | RootState::Dying)),
                        "TAG: a tag bound to a root that is not live or dying",
                    )?;
                    let twice = node.tags.iter().filter(|t| t.root == Some(root)).count() > 1;
                    ensure(!twice, "TAG: a root bound to two tags")?;
                    let _ = tag;
                }
            }
            for &(root, tag) in &node.loaded {
                ensure(
                    node.tags[tag].root == Some(root),
                    "LOADED: a core's loaded root is not its tag's",
                )?;
                ensure(
                    tag == 0
                        || self.gate.classes[root as usize] == Class::Root(RootState::Live)
                        || self.gate.classes[root as usize] == Class::Root(RootState::Dying),
                    "LOADED: a core's loaded root is not a live or dying root",
                )?;
            }
            let _ = n;
        }
        Ok(())
    }

    /// A chunk's map count holds every present leaf toward it and every
    /// leaf a request in use cleared and has not yet seen invalidated.
    fn check_counts(&self) -> Check {
        let mut needed = vec![0u64; self.chunks as usize];
        for (page, entries) in &self.gate.tables {
            if matches!(self.gate.classes[*page as usize], Class::Table { .. }) {
                for entry in entries {
                    if let Entry::Leaf { page, .. } = entry {
                        needed[Self::chunk_of(*page) as usize] += 1;
                    }
                }
            }
        }
        for (n, node) in self.gate.nodes.iter().enumerate() {
            // A dead or accused node's requests are never read again; R6
            // clears the counts they held.
            let state = self.gate.members[n].state;
            if matches!(
                state,
                NodeState::Dead | NodeState::Fencing | NodeState::Fenced
            ) {
                continue;
            }
            for request in &node.requests {
                if let Request::InUse { record, .. } = request {
                    for &page in &record.leaves {
                        needed[Self::chunk_of(page) as usize] += 1;
                    }
                }
            }
        }
        for chunk in 0..self.chunks as usize {
            ensure(
                self.gate.map_counts[chunk] >= needed[chunk],
                "COUNT: a map count below the leaves it holds",
            )?;
        }
        Ok(())
    }

    /// (G) on the CPU side: what each core can use now, under its loaded
    /// tag, reaches only memory its node holds; a cached table is a table
    /// of the node; a cached leaf whose leaf is gone is still counted by a
    /// request in use, so that its memory cannot be reused.
    fn check_reach(&self) -> Check {
        for core in 0..self.tlb.len() {
            let (n, p) = self.position(core);
            if !self.gate.members[usize::from(n)].state.writes() {
                continue;
            }
            let (root, tag) = self.gate.nodes[usize::from(n)].loaded[p];
            let tlb = &self.tlb[core];
            for (&(t, _), &table) in &tlb.tables {
                if t != tag {
                    continue;
                }
                let class = self
                    .gate
                    .classes
                    .get(table as usize)
                    .copied()
                    .unwrap_or(Class::Data);
                let kernel = table == self.layout[usize::from(n)].kernel_table;
                ensure(
                    kernel || matches!(class, Class::Table { .. }) && self.holds(n, table),
                    "G: a core walks a cached table that is no table of its node",
                )?;
            }
            let live = self.reachable(n, root);
            let mut counted: BTreeSet<Page> = BTreeSet::new();
            for request in &self.gate.nodes[usize::from(n)].requests {
                if let Request::InUse { record, what, .. } = request {
                    counted.extend(record.leaves.iter().copied());
                    // A kernel run being unmapped: its words are written
                    // only at the request's completion.
                    if let Invalidates::KernelRun { chunk, .. } = what {
                        let first = chunk * PAGES_PER_CHUNK;
                        counted.extend(first..first + PAGES_PER_CHUNK);
                    }
                }
            }
            for (&(t, va), &(page, write)) in &tlb.leaves {
                if t != tag {
                    continue;
                }
                ensure(
                    self.holds(n, page),
                    "G: a translation reaches memory its node does not hold",
                )?;
                ensure(
                    !write || self.gate.classes[page as usize] == Class::Data,
                    "G: a writable translation reaches a page that is not data",
                )?;
                let present = live.get(&va) == Some(&(page, write));
                ensure(
                    present || counted.contains(&page),
                    "G: a stale translation whose count the gate lowered",
                )?;
            }
        }
        Ok(())
    }

    /// The leaves a walk from `root` reaches now.
    fn reachable(&self, _n: u16, root: Page) -> BTreeMap<(usize, usize), (Page, bool)> {
        let mut out = BTreeMap::new();
        let Some(entries) = self.gate.tables.get(&root) else {
            return out;
        };
        for (e0, entry) in entries.iter().enumerate() {
            let (Entry::Table(t) | Entry::Shared(t)) = *entry else {
                continue;
            };
            if let Some(leaves) = self.gate.tables.get(&t) {
                for (e1, leaf) in leaves.iter().enumerate() {
                    if let Entry::Leaf { page, write } = *leaf {
                        out.insert((e0, e1), (page, write));
                    }
                }
            }
        }
        out
    }
}
