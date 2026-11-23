//! The CPU side of the gate, from the tables of "Gate words" (D-152): the
//! page class byte (tables and roots), the gate word's counts, a core's
//! loaded root and tag, the tag table, and a core's invalidation request,
//! pending set and idle word. One function per row: `n` is the calling
//! node, `p` the position of the calling core in it.

use super::*;

impl Machine {
    fn writes(&self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())
    }

    /// A call made on core `p` of node `n`: the node's gate writes, and
    /// the position is one of the node's cores (whatever the kernel
    /// passes, the gate indexes no word of a core the node does not have).
    pub(super) fn writes_on(&self, n: u16, p: usize) -> Gated {
        self.writes(n)?;
        require(p < self.layout[usize::from(n)].cores.len())
    }

    /// The check of every call that names a root and its tag: the entry of
    /// `tag` in node `n`'s tag table holds `root` (a tag beyond the table
    /// holds nothing). Untagged, in its place: the tag is 0, the root's
    /// byte reads *live* or *dying* and the node owns the root (the tag
    /// table, untagged mode, D-158).
    pub(super) fn tag_holds(&self, n: u16, tag: usize, root: Page) -> bool {
        if self.is_untagged() {
            let class = self.gate.classes.get(root as usize);
            let live = matches!(class, Some(Class::Root(RootState::Live | RootState::Dying)));
            return tag == 0 && live && self.holds(n, root);
        }
        let tags = &self.gate.nodes[usize::from(n)].tags;
        tags.get(tag).is_some_and(|t| t.root == Some(root))
    }

    /// The call that loads the kernel's root on core `p`. Untagged, a root
    /// load like any other: the core's loaded root written first, then its
    /// whole TLB invalidated (D-158), so that it holds translations of the
    /// root it has loaded and of no other.
    pub(super) fn load_kernel_root(&mut self, n: u16, p: usize) {
        let kernel = self.layout[usize::from(n)].kernel_root;
        self.gate.nodes[usize::from(n)].loaded[p] = (kernel, 0);
        if self.is_untagged() && !self.old.untagged_load_keeps_the_tlb {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
    }

    pub(super) fn class(&self, page: Page) -> Gated<Class> {
        self.gate
            .classes
            .get(page as usize)
            .copied()
            .ok_or(Refused::Condition)
    }

    /// The compare-and-swap of a class byte: written only if it reads
    /// `before`.
    pub(super) fn cas_class(&mut self, page: Page, before: Class, after: Class) -> Gated {
        let slot = &mut self.gate.classes[page as usize];
        if *slot != before {
            return Err(Refused::Cas);
        }
        *slot = after;
        let chunk = Self::chunk_of(page) as usize;
        match (before == Class::Data, after == Class::Data) {
            (true, false) => self.gate.not_data[chunk] += 1,
            (false, true) => self.gate.not_data[chunk] -= 1,
            _ => {}
        }
        Ok(())
    }

    /// Whether `page` lies in a table chunk: bit 58 of its chunk's gate
    /// word (G4, D-154).
    pub(crate) fn table_chunk(&self, page: Page) -> bool {
        self.gate
            .table_bits
            .get(Self::chunk_of(page) as usize)
            .copied()
            .unwrap_or(false)
    }

    /// The conditions of every row of the page class byte from *data*
    /// (Gate words, the page class byte; G5; D-154): the ownership word
    /// taken by the node with its generation, or the page in its floor;
    /// the chunk's gate word with the table bit set; no give or destruction
    /// of the chunk in flight (D-153, gap 30). No leaf and no anchor ever
    /// targets a table chunk (G3, G6), so no map count is read, no grant
    /// table walked, no core asked, and the kernel's large leaf, already
    /// read-only, is not touched.
    pub(super) fn leaves_data(&self, n: u16, page: Page) -> bool {
        let tables = self.table_chunk(page) || self.old.tables_in_ordinary_chunks;
        self.holds(n, page) && tables && !self.giving(n, page)
    }

    /// While a request names a give or a destruction of a chunk, every
    /// write that would raise one of its counts is refused (Gate words, the
    /// ownership words and the page class byte; D-153, gap 30). Without it
    /// the completion gives a chunk that holds a root (pass 29's seeds 75
    /// and 151, kept as tests). The requests read are those of the calling
    /// node `n`, the only one whose give of a chunk it holds can be in
    /// flight: a request of a node that died with a give in flight is never
    /// read again (a core's invalidation request, cell Death), so it holds
    /// nothing of the chunk's next owner.
    pub(super) fn giving(&self, n: u16, page: Page) -> bool {
        if self.old.counts_free_during_give {
            return false;
        }
        let chunk = Self::chunk_of(page);
        self.gate.nodes[usize::from(n)]
            .requests
            .iter()
            .any(|r| matches!(r, Request::InUse { what: Invalidates::KernelRun { chunk: c, .. }, .. } if *c == chunk))
    }

    /// The tag whose entry holds `root`, if any.
    pub(crate) fn tag_of(&self, n: u16, root: Page) -> Option<usize> {
        self.gate.nodes[usize::from(n)]
            .tags
            .iter()
            .position(|t| t.root == Some(root))
    }

    // The page class byte.

    /// *data* → table *k*, *loose*: the call that takes a page for a
    /// space's table, from the space's table untyped. The gate clears the
    /// page; non-data count + 1.
    pub(crate) fn take_table(&mut self, n: u16, page: Page, level: u8) -> Gated {
        self.writes(n)?;
        require(self.leaves_data(n, page))?;
        require((1..LEVELS).contains(&level))?;
        require(self.class(page)? == Class::Data)?;
        self.apply_target(n, &[page], Target::Table(level));
        Ok(())
    }

    /// *data* → aspace root *unbound*: `untyped_retype` of the space, the
    /// root a page of the table untyped the call names (D-154); the gate
    /// clears the page and writes the node's shared entries.
    pub(crate) fn retype_root(&mut self, n: u16, page: Page) -> Gated {
        self.writes(n)?;
        require(self.leaves_data(n, page))?;
        require(self.class(page)? == Class::Data)?;
        self.apply_target(n, &[page], Target::Root);
        Ok(())
    }

    // The kernel's own space (Gate words, kept kernel tables; D-154): a
    // chunk taken is mapped by one large leaf, writable for kind 0 and
    // read-only for kind 2, never rewritten between take and give; a
    // node's table floor is mapped read-only and its ordinary floor
    // writable, by leaves the kernel table of the small machine does not
    // hold (kept memory).

    /// The kernel-table entry of `chunk` in node `n`'s kernel table, if it
    /// maps that chunk.
    pub(super) fn kernel_entry(&self, n: u16, chunk: u64) -> (usize, Entry) {
        let kernel = self.layout[usize::from(n)].kernel_table;
        let slot = (chunk % ENTRIES as u64) as usize;
        let entry = self.gate.tables[&kernel][slot];
        let ours = matches!(entry, Entry::Large { chunk: c, .. } if c == chunk);
        (slot, if ours { entry } else { Entry::Absent })
    }

    /// The bytes of pages that leave *data*, and what the gate writes into
    /// them (cleared; an aspace root's shared entries).
    pub(super) fn apply_target(&mut self, n: u16, pages: &[Page], target: Target) {
        for &page in pages {
            let after = match target {
                Target::Table(level) => Class::Table {
                    level,
                    hook: Hook::Loose,
                },
                Target::Root => Class::Root(RootState::Unbound),
                Target::IoRoot => Class::IoRoot {
                    devices: 0,
                    dying: false,
                },
                Target::Its { .. } => Class::ItsTable { pinned: false },
            };
            if self.cas_class(page, Class::Data, after).is_err() {
                continue;
            }
            self.kernel.data.remove(&page);
            match target {
                Target::Table(_) | Target::IoRoot => {
                    self.gate.tables.insert(page, [Entry::Absent; ENTRIES]);
                }
                Target::Root => {
                    let mut entries = [Entry::Absent; ENTRIES];
                    entries[ENTRIES - 1] = Entry::Shared(self.layout[usize::from(n)].kernel_table);
                    self.gate.tables.insert(page, entries);
                }
                Target::Its { .. } => {}
            }
        }
        if let Target::Its { first, k } = target {
            self.dev.its_objects.insert(first, k);
        }
    }

    /// How node `n`'s kernel reaches `page` through its own space now, on
    /// core `p`: the large leaf its TLB holds for the page's chunk, or the
    /// kept leaf of its floor (the table floor read-only, the ordinary
    /// floor writable); `None`, a fault.
    fn kernel_view(&self, n: u16, p: usize, page: Page) -> Option<bool> {
        let layout = &self.layout[usize::from(n)];
        let (start, end) = layout.floor;
        if start <= page && page < end {
            let (first, last) = layout.table_floor;
            return Some(!(first <= page && page < last));
        }
        let chunk = Self::chunk_of(page);
        let slot = (chunk % ENTRIES as u64) as usize;
        match self.tlb[self.core(n, p)].large.get(&slot) {
            Some(&(c, write)) if c == chunk => Some(write),
            _ => None,
        }
    }

    /// A kernel access, by core `p`, to `page` through its own view: the
    /// translation the core holds now. None: a fault. A store goes through
    /// a writable translation only, so never into a table chunk, mapped
    /// read-only from take to give (D-154).
    pub(crate) fn kernel_access(
        &mut self,
        n: u16,
        p: usize,
        page: Page,
        value: Option<u64>,
    ) -> Gated {
        self.writes(n)?;
        require(p < self.layout[usize::from(n)].cores.len())?;
        require(page < self.chunks * PAGES_PER_CHUNK)?;
        let write = self.kernel_view(n, p, page).ok_or(Refused::Condition)?;
        if let Some(value) = value {
            require(write)?;
            self.kernel.data.insert(page, value);
            if self.gate.classes.get(page as usize) != Some(&Class::Data) {
                self.kernel.wrote_tables.push((n, page));
            }
        }
        Ok(())
    }

    /// *loose* → *hooked* at level *k*: the call that writes the entry
    /// above it. The parent a *live* root (the small machine has two
    /// levels) the call names, of the node; the gate writes the child
    /// hooked, then the hooking store.
    pub(crate) fn hook(&mut self, n: u16, root: Page, e0: usize, child: Page) -> Gated {
        self.writes(n)?;
        require(e0 < ENTRIES - 1 && self.holds(n, root) && self.holds(n, child))?;
        require(self.class(root)? == Class::Root(RootState::Live))?;
        require(self.gate.tables[&root][e0] == Entry::Absent)?;
        let level = LEVELS - 1;
        self.cas_class(
            child,
            Class::Table {
                level,
                hook: Hook::Loose,
            },
            Class::Table {
                level,
                hook: Hook::Hooked,
            },
        )?;
        self.gate.links.remove(&child);
        self.gate.tables.get_mut(&root).expect("a root")[e0] = Entry::Table(child);
        Ok(())
    }

    /// A leaf written: map count + 1 (G3: the page held by the node, a
    /// writable leaf only toward *data*).
    pub(crate) fn map(
        &mut self,
        n: u16,
        root: Page,
        tag: usize,
        (e0, e1): (usize, usize),
        page: Page,
        write: bool,
    ) -> Gated {
        self.writes(n)?;
        require(e0 < ENTRIES - 1 && e1 < ENTRIES)?;
        require(self.class(root)? == Class::Root(RootState::Live))?;
        require(self.tag_holds(n, tag, root))?;
        require(self.holds(n, page) && !self.giving(n, page))?;
        require(!write || self.class(page)? == Class::Data)?;
        // G3 (3) (D-154): a leaf never points into a chunk whose gate word
        // has the table bit set; so its map count is always 0.
        require(!self.table_chunk(page) || self.old.leaves_into_table_chunks)?;
        let Entry::Table(table) = self.gate.tables[&root][e0] else {
            return Err(Refused::Condition);
        };
        require(self.gate.tables[&table][e1] == Entry::Absent)?;
        self.gate.tables.get_mut(&table).expect("a table")[e1] = Entry::Leaf { page, write };
        self.gate.map_counts[Self::chunk_of(page) as usize] += 1;
        Ok(())
    }

    /// The asked set of a clearing call for a user tag: the tag's set less
    /// the caller and less each core whose idle mark the gate reads set,
    /// whose owes bit it sets. Untagged: every other core of the node whose
    /// loaded root is the root, read after the call cleared its entries; a
    /// core that loads the root later invalidates in its load, and no owes
    /// bit is set (the tag table, untagged mode, D-158).
    fn asked_for_tag(&mut self, n: u16, p: usize, tag: usize, root: Page) -> BTreeSet<usize> {
        let node = &mut self.gate.nodes[usize::from(n)];
        if dodos_abi::tags_untagged(self.tag_width) {
            let cores = 0..node.loaded.len();
            return cores
                .filter(|&q| q != p && node.loaded[q].0 == root)
                .collect();
        }
        let mut asked = BTreeSet::new();
        for &q in &node.tags[tag].set.clone() {
            if q == p {
                continue;
            }
            if node.idle[q].idle {
                node.idle[q].owes = true;
            } else {
                asked.insert(q);
            }
        }
        asked
    }

    /// The request of core `p` from *free* to *in use*; the asked cores'
    /// pending bits set (their interrupts are the scheduler's: an asked
    /// core acknowledges at its next gate call). Completes at once when
    /// nobody is asked.
    pub(super) fn start_request(
        &mut self,
        n: u16,
        p: usize,
        what: Invalidates,
        asked: BTreeSet<usize>,
        record: Clearing,
    ) -> Gated {
        let node = &mut self.gate.nodes[usize::from(n)];
        if node.requests[p] != Request::Free {
            return Err(Refused::Cas);
        }
        for &q in &asked {
            node.pending[q].insert(p);
        }
        let empty = asked.is_empty();
        node.requests[p] = Request::InUse {
            what,
            asked,
            acked: BTreeSet::new(),
            record,
        };
        if empty {
            self.complete(n, p);
        }
        Ok(())
    }

    /// The caller's own invalidation in a clearing call (Gate words, a
    /// core's invalidation request, row *free* → *in use*; D-153, gap 26).
    fn invalidate_here(&mut self, n: u16, p: usize, what: &Flush) {
        if self.old.no_own_invalidation {
            return;
        }
        let core = self.core(n, p);
        self.invalidate(core, what.clone());
    }

    /// A leaf cleared: the clearing call. The root and tag named, the tag's
    /// entry holding the root; the request *in use*, the leaf's count kept
    /// until the request completes.
    pub(crate) fn unmap(
        &mut self,
        n: u16,
        p: usize,
        root: Page,
        tag: usize,
        (e0, e1): (usize, usize),
    ) -> Gated {
        self.writes_on(n, p)?;
        require(e0 < ENTRIES - 1 && e1 < ENTRIES)?;
        require(self.tag_holds(n, tag, root))?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let Entry::Table(table) = self.gate.tables[&root][e0] else {
            return Err(Refused::Condition);
        };
        let Entry::Leaf { page, .. } = self.gate.tables[&table][e1] else {
            return Err(Refused::Condition);
        };
        self.gate.tables.get_mut(&table).expect("a table")[e1] = Entry::Absent;
        let counted = self.cleared(n, root, (e0, e1));
        let flush = Flush::Range {
            tag,
            va: vec![(e0, e1)],
        };
        self.invalidate_here(n, p, &flush);
        let asked = self.asked_for_tag(n, p, tag, root);
        let record = Clearing {
            leaves: vec![(page, counted)],
            ..Clearing::default()
        };
        self.start_request(
            n,
            p,
            Invalidates::Range {
                tag,
                root,
                va: vec![(e0, e1)],
            },
            asked,
            record,
        )
    }

    /// *hooked* → *unhooking*: the call that clears the entry above it,
    /// every entry of the child read non-present; completed into *loose*
    /// by the request's completion.
    pub(crate) fn unhook(&mut self, n: u16, p: usize, root: Page, tag: usize, e0: usize) -> Gated {
        self.writes_on(n, p)?;
        require(e0 < ENTRIES - 1)?;
        require(self.tag_holds(n, tag, root))?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let Entry::Table(child) = self.gate.tables[&root][e0] else {
            return Err(Refused::Condition);
        };
        require(self.gate.tables[&child].iter().all(|e| *e == Entry::Absent))?;
        let level = LEVELS - 1;
        self.cas_class(
            child,
            Class::Table {
                level,
                hook: Hook::Hooked,
            },
            Class::Table {
                level,
                hook: Hook::Unhooking,
            },
        )?;
        self.gate.tables.get_mut(&root).expect("a root")[e0] = Entry::Absent;
        let va: Vec<(usize, usize)> = (0..ENTRIES).map(|e1| (e0, e1)).collect();
        let flush = self.unhook_flush(tag, &va);
        self.invalidate_here(n, p, &flush);
        let asked = self.asked_for_tag(n, p, tag, root);
        let record = Clearing {
            unhooked: vec![child],
            ..Clearing::default()
        };
        self.start_request(n, p, Invalidates::Range { tag, root, va }, asked, record)
    }

    /// What a core invalidates for a change that unhooked a non-leaf
    /// entry: the addresses it covered, and on RISC-V the space's whole
    /// ASID (`SFENCE.VMA` with `rs1 = x0`), since an invalidation by
    /// address leaves the cached non-leaf entry there (TLB, the request of
    /// a core; D-154, M3).
    fn unhook_flush(&self, tag: usize, va: &[(usize, usize)]) -> Flush {
        match self.riscv && !self.old.riscv_unhook_by_address {
            true => Flush::Tag(tag),
            false => Flush::Range {
                tag,
                va: va.to_vec(),
            },
        }
    }

    /// *loose* → *loose*, first word changed: `table_link(table, next)`,
    /// both *loose*, of one kind, of the node (`next` 0 for none).
    pub(crate) fn table_link(&mut self, n: u16, table: Page, next: Option<Page>) -> Gated {
        self.writes(n)?;
        let loose = Class::Table {
            level: LEVELS - 1,
            hook: Hook::Loose,
        };
        require(self.holds(n, table) && self.class(table)? == loose)?;
        if let Some(next) = next {
            require(next != table && self.holds(n, next) && self.class(next)? == loose)?;
            self.gate.links.insert(table, next);
        } else {
            self.gate.links.remove(&table);
        }
        Ok(())
    }

    /// *loose* → *data*: `table_free`; first word written 0; non-data − 1.
    pub(crate) fn table_free(&mut self, n: u16, page: Page) -> Gated {
        self.writes(n)?;
        require(self.holds(n, page))?;
        let level = LEVELS - 1;
        self.cas_class(
            page,
            Class::Table {
                level,
                hook: Hook::Loose,
            },
            Class::Data,
        )?;
        self.gate.links.remove(&page);
        self.gate.tables.remove(&page);
        Ok(())
    }

    /// An interrupt to another core with an ordinary vector (G10, D-155):
    /// on x86-64 the vector lies in `IKV_FIRST..=IKV_LAST`, the caller
    /// names the target by core number, any core of the machine, and the
    /// gate composes the ICR (fixed delivery, physical destination, edge,
    /// no shorthand). An NMI's vector, an exception's or a claimable one
    /// is refused: a faulty kernel forges none toward another node.
    pub(crate) fn send_ipi(&mut self, n: u16, target: usize, vector: u8) -> Gated {
        self.writes(n)?;
        require(target < self.tlb.len())?;
        require(dodos_abi::x86_64::ikv_valid(vector) || self.old.any_vector_in_icr)?;
        self.icr.push((n, target, vector));
        Ok(())
    }

    // The tag table.

    /// `bind(tag, root)`: the root *unbound* → *live*, the tag's entry 0
    /// → (root, ∅). Tag 0 is the kernel's. Untagged, `bind(0, R)` is the
    /// only bind: R from *unbound* to *live*, no entry written; a bind
    /// that names another tag is `invalid` (D-158).
    pub(crate) fn bind(&mut self, n: u16, tag: usize, root: Page) -> Gated {
        self.writes(n)?;
        if self.is_untagged() {
            require(tag == 0 && self.holds(n, root))?;
            return self.cas_class(
                root,
                Class::Root(RootState::Unbound),
                Class::Root(RootState::Live),
            );
        }
        // G7 (D-155): one tag is the gate's own, bound to no kernel or user
        // root; the tag table holds the others.
        let usable = tag < self.tags() || (self.old.gate_tag_bound && Some(tag) == self.gate_tag());
        require(tag != 0 && usable && self.holds(n, root))?;
        let entry = &self.gate.nodes[usize::from(n)].tags[tag];
        require(entry.root.is_none() && entry.set.is_empty())?;
        let second = self.old.root_under_two_tags
            && self.class(root)? == Class::Root(RootState::Live)
            && self.tag_of(n, root).is_some();
        if !second {
            self.cas_class(
                root,
                Class::Root(RootState::Unbound),
                Class::Root(RootState::Live),
            )?;
        }
        self.gate.nodes[usize::from(n)].tags[tag].root = Some(root);
        Ok(())
    }

    /// The root load (G7): the root *live*, the tag's entry holding it; an
    /// owes bit invalidates every tag first; the core joins the tag's set.
    /// Untagged, the load of (R, 0): R's byte *live* and the node owning
    /// R; the core's loaded root written first, then its whole TLB
    /// invalidated, then the root loaded, in one gate call; no set, and
    /// the owes bit never set (D-158).
    pub(crate) fn load(&mut self, n: u16, p: usize, root: Page, tag: usize) -> Gated {
        self.writes_on(n, p)?;
        if self.is_untagged() {
            require(tag == 0 && self.holds(n, root))?;
            require(self.class(root)? == Class::Root(RootState::Live))?;
            let node = &mut self.gate.nodes[usize::from(n)];
            node.idle[p] = IdleWord::default();
            node.loaded[p] = (root, 0);
            if !self.old.untagged_load_keeps_the_tlb {
                let core = self.core(n, p);
                self.invalidate(core, Flush::All);
            }
            return Ok(());
        }
        // G7 (D-155): the gate never loads its own tag for a root.
        require(tag < self.tags() || (self.old.gate_tag_bound && Some(tag) == self.gate_tag()))?;
        require(self.class(root)? == Class::Root(RootState::Live))?;
        require(self.gate.nodes[usize::from(n)].tags[tag].root == Some(root))?;
        let owes = self.gate.nodes[usize::from(n)].idle[p].owes;
        if owes {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
        let node = &mut self.gate.nodes[usize::from(n)];
        node.idle[p] = IdleWord::default();
        node.loaded[p] = (root, tag);
        node.tags[tag].set.insert(p);
        Ok(())
    }

    /// The call that loads the kernel's root before idle: the idle mark
    /// set.
    pub(crate) fn idle(&mut self, n: u16, p: usize) -> Gated {
        self.writes_on(n, p)?;
        self.load_kernel_root(n, p);
        self.gate.nodes[usize::from(n)].idle[p].idle = true;
        Ok(())
    }

    /// The asked core's gate call at its interrupt: for each request
    /// pending, the kernel's root loaded first if its loaded root is a
    /// *dying* root the request names; then the invalidation; then the
    /// acknowledgement, the last one completing the request.
    pub(crate) fn ack(&mut self, n: u16, p: usize) -> Gated {
        self.writes_on(n, p)?;
        let pending: Vec<usize> = self.gate.nodes[usize::from(n)].pending[p]
            .iter()
            .copied()
            .collect();
        for q in pending {
            let Request::InUse { what, record, .. } =
                self.gate.nodes[usize::from(n)].requests[q].clone()
            else {
                continue;
            };
            let core = self.core(n, p);
            match what {
                // An asked core does on its own what the caller did: for a
                // change that unhooked a table, RISC-V's whole-ASID fence
                // (D-154, M3).
                Invalidates::Range { tag, va, .. } => {
                    let flush = match record.unhooked.is_empty() {
                        true => Flush::Range { tag, va },
                        false => self.unhook_flush(tag, &va),
                    };
                    self.invalidate(core, flush)
                }
                Invalidates::Tag { tag, root } => {
                    if !self.old.dying_without_kernel_load
                        && self.gate.nodes[usize::from(n)].loaded[p].0 == root
                        && self.class(root)? == Class::Root(RootState::Dying)
                    {
                        self.load_kernel_root(n, p);
                    }
                    self.invalidate(core, Flush::Tag(tag));
                    // Out of the tag's set: a whole-tag invalidation while
                    // its loaded tag is another. Untagged, no set is kept.
                    if !self.is_untagged() && self.gate.nodes[usize::from(n)].loaded[p].1 != tag {
                        self.gate.nodes[usize::from(n)].tags[tag].set.remove(&p);
                    }
                }
                Invalidates::KernelRun { .. } => self.invalidate(core, Flush::KernelSpace),
                Invalidates::Barrier => {}
            }
            let node = &mut self.gate.nodes[usize::from(n)];
            node.pending[p].remove(&q);
            let done = match &mut node.requests[q] {
                Request::InUse { asked, acked, .. } => {
                    acked.insert(p);
                    acked == asked
                }
                Request::Free => false,
            };
            if done {
                self.complete(n, q);
            }
        }
        Ok(())
    }

    /// The count a leaf just cleared at (`root`, `va`) of node `n` is in:
    /// the use entry its record names, for a leaf toward a granted page,
    /// whose record of a present leaf ends here; the map count of its
    /// chunk otherwise.
    fn cleared(&mut self, n: u16, root: Page, va: (usize, usize)) -> Counted {
        match self.grants.remote.remove(&(n, root, va)) {
            Some((_, _, index)) => Counted::Use(index),
            None => Counted::Map,
        }
    }

    /// A cleared leaf's count lowered, at the completion of its
    /// invalidation or at a teardown: the count the record of the clearing
    /// names, and no other (G4, G6).
    fn lower(&mut self, n: u16, page: Page, counted: Counted) {
        let count = match counted {
            Counted::Map => &mut self.gate.map_counts[Self::chunk_of(page) as usize],
            Counted::Use(index) => match self.grants.uses[usize::from(n)][index].as_mut() {
                Some(used) => &mut used.count,
                None => return,
            },
        };
        *count = count.saturating_sub(1);
    }

    /// *in use* → *free*: the counts of the recorded run lowered, the
    /// recorded tables written *loose*, a give's words written.
    fn complete(&mut self, n: u16, q: usize) {
        let Request::InUse { record, what, .. } = std::mem::replace(
            &mut self.gate.nodes[usize::from(n)].requests[q],
            Request::Free,
        ) else {
            return;
        };
        for (page, counted) in record.leaves {
            self.lower(n, page, counted);
        }
        let level = LEVELS - 1;
        for table in record.unhooked {
            let _ = self.cas_class(
                table,
                Class::Table {
                    level,
                    hook: Hook::Unhooking,
                },
                Class::Table {
                    level,
                    hook: Hook::Loose,
                },
            );
        }
        // The words of a give or of a block's destruction, written once
        // every core invalidated the run, by a compare-and-swap from the
        // value read when the request was opened, *taken* by the giving
        // generation (Gate words, the ownership words, row *taken* →
        // *free*; D-159): a word that reads anything else is no longer this
        // request's to write. A give names the word's next owner; a
        // destruction keeps it.
        if let Invalidates::KernelRun { chunk, to } = what {
            let before = Owner::Taken {
                node: n,
                generation: self.gate.members[usize::from(n)].generation,
            };
            let word = &mut self.gate.owners[chunk as usize];
            if *word == before {
                *word = Owner::Free {
                    node: Some(to.unwrap_or(n)),
                };
            }
            return;
        }
        if let (Invalidates::Tag { tag, .. }, None) = (&what, record.dying) {
            // An unbind: the root back to *unbound*, the tag's entry 0,
            // once no core's loaded tag is the tag and its set is empty.
            let tag = *tag;
            self.finish_unbind(n, tag);
        }
    }

    /// The completion of `unbind`: (R, ∅) → 0 and the root *live* →
    /// *unbound*, when no core's loaded tag is the tag.
    fn finish_unbind(&mut self, n: u16, tag: usize) {
        let node = &self.gate.nodes[usize::from(n)];
        let loaded = node.loaded.iter().any(|&(_, t)| t == tag);
        let Some(root) = node.tags[tag].root else {
            return;
        };
        if loaded || !node.tags[tag].set.is_empty() {
            return;
        }
        if self
            .cas_class(
                root,
                Class::Root(RootState::Live),
                Class::Root(RootState::Unbound),
            )
            .is_ok()
        {
            self.gate.nodes[usize::from(n)].tags[tag].root = None;
        }
    }

    /// `unbind(tag)`: a whole-tag request asking the tag's whole set, idle
    /// cores included (Gate words, the tag table; D-153, gap 27): an idle
    /// core left out would never leave the set.
    pub(crate) fn unbind(&mut self, n: u16, p: usize, tag: usize) -> Gated {
        self.writes_on(n, p)?;
        require(tag != 0 && tag < self.tags())?;
        let Some(root) = self.gate.nodes[usize::from(n)].tags[tag].root else {
            return Err(Refused::Condition);
        };
        require(self.class(root)? == Class::Root(RootState::Live))?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let core = self.core(n, p);
        let mut asked: BTreeSet<usize> = self.gate.nodes[usize::from(n)].tags[tag].set.clone();
        if self.old.unbind_skips_idle {
            let idle = &self.gate.nodes[usize::from(n)].idle;
            asked.retain(|&q| !idle[q].idle);
        }
        if asked.remove(&p) {
            self.invalidate(core, Flush::Tag(tag));
            if self.gate.nodes[usize::from(n)].loaded[p].1 != tag {
                self.gate.nodes[usize::from(n)].tags[tag].set.remove(&p);
            }
        }
        self.start_request(
            n,
            p,
            Invalidates::Tag { tag, root },
            asked,
            Clearing::default(),
        )
    }

    /// `unbind(0, R)` of the untagged mode: R from *live* to *unbound* at
    /// once, with an empty asked set, on the condition that no core's
    /// loaded root is R; otherwise `busy` (the tag table, D-158). No core
    /// holds a translation of a root it has not loaded, so nothing is
    /// invalidated.
    pub(crate) fn unbind_root(&mut self, n: u16, p: usize, root: Page) -> Gated {
        self.writes_on(n, p)?;
        require(self.is_untagged() && self.holds(n, root))?;
        let node = &self.gate.nodes[usize::from(n)];
        require(!node.loaded.iter().any(|&(loaded, _)| loaded == root))?;
        self.cas_class(
            root,
            Class::Root(RootState::Live),
            Class::Root(RootState::Unbound),
        )
    }

    /// aspace root *live* → *dying*: the first step of the destruction,
    /// asking every core of the tag's set and every core whose loaded root
    /// is the root. A root *unbound* → *dying*: nothing to ask, no tag is
    /// bound to it and no core loaded it (the page class byte; D-159: the
    /// row was missing).
    pub(crate) fn destroy(&mut self, n: u16, p: usize, root: Page) -> Gated {
        self.writes_on(n, p)?;
        require(self.holds(n, root))?;
        if self.class(root)? == Class::Root(RootState::Unbound) {
            return self.cas_class(
                root,
                Class::Root(RootState::Unbound),
                Class::Root(RootState::Dying),
            );
        }
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        // Untagged, every root is loaded with tag 0 and no entry names it:
        // the asked set is the cores whose loaded root is the root, "the
        // caller included when it has the root loaded" (D-158).
        let untagged = self.is_untagged();
        let tag = match untagged {
            true => 0,
            false => self.tag_of(n, root).ok_or(Refused::Condition)?,
        };
        require(untagged || tag != 0)?;
        self.cas_class(
            root,
            Class::Root(RootState::Live),
            Class::Root(RootState::Dying),
        )?;
        let node = &self.gate.nodes[usize::from(n)];
        let mut asked: BTreeSet<usize> = match untagged {
            true => BTreeSet::new(),
            false => node.tags[tag].set.clone(),
        };
        asked.extend((0..node.loaded.len()).filter(|&q| node.loaded[q].0 == root));
        // The caller's own part (D-153, gap 26): it loads the kernel's
        // root if it had this one loaded, and invalidates the tag.
        if asked.remove(&p) {
            if self.gate.nodes[usize::from(n)].loaded[p].0 == root {
                self.load_kernel_root(n, p);
            }
            let core = self.core(n, p);
            self.invalidate(core, Flush::Tag(tag));
            if !untagged {
                self.gate.nodes[usize::from(n)].tags[tag].set.remove(&p);
            }
        }
        let record = Clearing {
            dying: Some(root),
            ..Clearing::default()
        };
        self.start_request(n, p, Invalidates::Tag { tag, root }, asked, record)
    }

    /// Root *dying* → torn down, then *data*: the request asked at dying
    /// complete (no request names the root); every leaf cleared with its
    /// count lowered at once, every child *loose* then *data*, the root
    /// last, the tag's entry 0.
    pub(crate) fn teardown(&mut self, n: u16, root: Page) -> Gated {
        self.writes(n)?;
        require(self.holds(n, root))?;
        require(self.class(root)? == Class::Root(RootState::Dying))?;
        // The tag the root died under; none for a root that died unbound,
        // nor untagged, where the last teardown call writes no entry (D-158).
        let tag = self.tag_of(n, root);
        let in_flight = self.gate.nodes[usize::from(n)].requests.iter().any(|r| {
            matches!(r, Request::InUse { what: Invalidates::Tag { root: r, .. }, .. } if *r == root)
        });
        require(!in_flight)?;
        let node = &self.gate.nodes[usize::from(n)];
        require(!node.loaded.iter().any(|&(_, t)| Some(t) == tag))?;
        let entries = self.gate.tables[&root];
        let level = LEVELS - 1;
        for (e0, entry) in entries.iter().enumerate() {
            let Entry::Table(child) = *entry else {
                continue;
            };
            let leaves = self.gate.tables[&child];
            self.gate.tables.insert(child, [Entry::Absent; ENTRIES]);
            for (e1, leaf) in leaves.into_iter().enumerate() {
                if let Entry::Leaf { page, .. } = leaf {
                    let counted = self.cleared(n, root, (e0, e1));
                    self.lower(n, page, counted);
                }
            }
            self.cas_class(
                child,
                Class::Table {
                    level,
                    hook: Hook::Hooked,
                },
                Class::Table {
                    level,
                    hook: Hook::Loose,
                },
            )?;
            self.cas_class(
                child,
                Class::Table {
                    level,
                    hook: Hook::Loose,
                },
                Class::Data,
            )?;
            self.gate.tables.remove(&child);
        }
        self.gate.tables.remove(&root);
        if let Some(tag) = tag {
            self.gate.nodes[usize::from(n)].tags[tag] = TagEntry::default();
        }
        self.cas_class(root, Class::Root(RootState::Dying), Class::Data)?;
        Ok(())
    }

    // Ownership words and the kernel's own space.

    /// `memory_take` of a chunk: *free* → *taken* (the rules of taking:
    /// a free chunk, or one taken by a generation that is over); the class
    /// bytes of the run 0; the whole gate word written, 0 for kind 0 and
    /// the table bit with its three counts 0 for kind 2; the run mapped in
    /// the kernel's own table by one large leaf (entry chunk mod E),
    /// writable for kind 0 and read-only for kind 2 (D-154).
    pub(crate) fn take(&mut self, n: u16, chunk: u64, kind: Take) -> Gated {
        self.writes(n)?;
        require(chunk < self.chunks)?;
        let dead = |node: u16| self.gate.members[usize::from(node)].state == NodeState::Dead;
        let takeable = match self.gate.owners[chunk as usize] {
            // Rules 1 to 3 of taking (Chunks, floors and the ownership
            // words): a word *free* with a node as owner is memory released
            // or given to that node, and stays that node's while it is not
            // dead; unclaimed memory is anyone's.
            Owner::Free { node: None } => true,
            Owner::Free { node: Some(owner) } => owner == n || dead(owner),
            Owner::Taken { node, generation } => {
                let m = self.gate.members[usize::from(node)];
                m.generation != generation || m.state == NodeState::Dead
            }
            Owner::Floor { .. } => false,
        };
        require(takeable)?;
        let kernel = self.layout[usize::from(n)].kernel_table;
        let slot = (chunk % ENTRIES as u64) as usize;
        require(self.gate.tables[&kernel][slot] == Entry::Absent)?;
        let first = chunk * PAGES_PER_CHUNK;
        require(
            (first..first + PAGES_PER_CHUNK).all(|q| self.gate.classes[q as usize] == Class::Data),
        )?;
        let g = self.gate.members[usize::from(n)].generation;
        self.gate.owners[chunk as usize] = Owner::Taken {
            node: n,
            generation: g,
        };
        self.gate.map_counts[chunk as usize] = 0;
        self.gate.grant_counts[chunk as usize] = 0;
        self.gate.table_bits[chunk as usize] = kind == Take::Tables;
        let write = kind == Take::Ordinary || self.old.table_chunks_mapped_writable;
        let leaf = Entry::Large { chunk, write };
        self.gate.tables.get_mut(&kernel).expect("the kernel table")[slot] = leaf;
        Ok(())
    }

    /// `memory_give` of a chunk to node `to`: G5's counts all 0; the run
    /// unmapped from the kernel's own space and that invalidated on every
    /// core of the node, idle ones included, before the ownership word is
    /// written *free* for `to` (at the request's completion).
    pub(crate) fn give(&mut self, n: u16, p: usize, chunk: u64, to: u16) -> Gated {
        require(to != n && usize::from(to) < self.layout.len())?;
        self.release_run(n, p, chunk, Some(to))
    }

    /// The destruction of the block of a chunk: as a give, the word
    /// written *free* with the same owner (The machine table, releasing and
    /// giving).
    pub(crate) fn destroy_block(&mut self, n: u16, p: usize, chunk: u64) -> Gated {
        self.release_run(n, p, chunk, None)
    }

    /// The row *taken* → *free* of an ownership word, for a give (`to` the
    /// node it goes to) and for a block's destruction (`None`).
    fn release_run(&mut self, n: u16, p: usize, chunk: u64, to: Option<u16>) -> Gated {
        self.writes_on(n, p)?;
        require(chunk < self.chunks)?;
        // The machine table, releasing and giving (D-159, settling gap 49):
        // "A give or a destruction of a block is `busy` while a request of
        // the node already names a give or a destruction of one of its
        // chunks". The compare-and-swap of the completion alone does not
        // hold G5: a chunk given twice, freed by the first completion and
        // taken again by its node reads the value the second completion
        // expects (`a_second_give_would_free_a_chunk_its_node_took_again`).
        require(!self.giving(n, chunk * PAGES_PER_CHUNK))?;
        let g = self.gate.members[usize::from(n)].generation;
        require(
            self.gate.owners[chunk as usize]
                == Owner::Taken {
                    node: n,
                    generation: g,
                },
        )?;
        require(
            self.gate.map_counts[chunk as usize] == 0
                && self.gate.not_data[chunk as usize] == 0
                && self.gate.grant_counts[chunk as usize] == 0,
        )?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        // The large leaf, writable or read-only, leaves the kernel's own
        // space; a table chunk is given as an ordinary one is, once it
        // holds no table (its count of pages not *data* 0).
        let (slot, entry) = self.kernel_entry(n, chunk);
        if entry != Entry::Absent {
            let kernel = self.layout[usize::from(n)].kernel_table;
            self.gate.tables.get_mut(&kernel).expect("the kernel table")[slot] = Entry::Absent;
        }
        let core = self.core(n, p);
        self.invalidate(core, Flush::KernelSpace);
        let node = &self.gate.nodes[usize::from(n)];
        let asked: BTreeSet<usize> = (0..node.loaded.len())
            .filter(|&q| q != p && !(self.old.kernel_run_skips_idle && node.idle[q].idle))
            .collect();
        self.start_request(
            n,
            p,
            Invalidates::KernelRun { chunk, to },
            asked,
            Clearing::default(),
        )
    }
}
