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

    /// G5's map count 0 for a page leaving *data* (Gate words, the page
    /// class byte, every row from *data*; D-153, gap 28). Without it a page
    /// the node maps writable becomes one of its own tables (pass 29's seed
    /// 8, kept as a test).
    pub(super) fn unmapped(&self, page: Page) -> bool {
        let count = self.gate.map_counts[Self::chunk_of(page) as usize];
        (count == 0 || self.old.class_change_ignores_map_count) && !self.giving(page)
    }

    /// While a request names a give or a destruction of a chunk, every
    /// write that would raise one of its counts is refused (Gate words, the
    /// ownership words and the page class byte; D-153, gap 30). Without it
    /// the completion gives a chunk that holds a root (pass 29's seeds 75
    /// and 151, kept as tests).
    pub(super) fn giving(&self, page: Page) -> bool {
        if self.old.counts_free_during_give {
            return false;
        }
        let chunk = Self::chunk_of(page);
        self.gate.nodes.iter().flat_map(|n| &n.requests).any(|r| {
            matches!(r, Request::InUse { what: Invalidates::KernelRun { chunk: c, .. }, .. } if *c == chunk)
        })
    }

    /// Whether a live, revoking or pinned grant's anchor covers `page`:
    /// the grant count when 0 (the small machine has no grant entries on
    /// its CPU side yet, so the count is the whole answer).
    pub(super) fn anchored(&self, page: Page) -> bool {
        self.gate.grant_counts[Self::chunk_of(page) as usize] != 0
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
    /// space's table. The ownership word taken by the node with its
    /// generation, or the page in its floor; no anchor covers it; the gate
    /// clears the page; non-data count + 1.
    pub(crate) fn take_table(&mut self, n: u16, page: Page, level: u8) -> Gated {
        self.writes(n)?;
        require(self.holds(n, page) && !self.anchored(page) && self.unmapped(page))?;
        require((1..LEVELS).contains(&level))?;
        self.cas_class(
            page,
            Class::Data,
            Class::Table {
                level,
                hook: Hook::Loose,
            },
        )?;
        self.gate.tables.insert(page, [Entry::Absent; ENTRIES]);
        self.kernel_view_read_only(n, page);
        Ok(())
    }

    /// *data* → aspace root *unbound*: `untyped_retype` of the space; the
    /// gate clears the page and writes the node's shared entries.
    pub(crate) fn retype_root(&mut self, n: u16, page: Page) -> Gated {
        self.writes(n)?;
        require(self.holds(n, page) && !self.anchored(page) && self.unmapped(page))?;
        self.cas_class(page, Class::Data, Class::Root(RootState::Unbound))?;
        let mut entries = [Entry::Absent; ENTRIES];
        entries[ENTRIES - 1] = Entry::Shared(self.layout[usize::from(n)].kernel_table);
        self.gate.tables.insert(page, entries);
        self.kernel_view_read_only(n, page);
        Ok(())
    }

    /// CONTRACT-GAP 29 (Gate words, the page class byte, rows *data* →
    /// table, root or *ITS table*; `memory_take` maps a run "into the
    /// kernel's own address space with large entries"): no row says what
    /// becomes of the kernel's own writable mapping of a page that turns
    /// into a table, though G2 says no kernel maps a table writable, and a
    /// change of a writable kernel translation needs an invalidation on
    /// every core of the node. Executed as written, the node's kernel
    /// writes its own table through its own space (the exploration's seed
    /// 0, once G5's map count was added). The model reads the most careful
    /// one: the class change rewrites the page's kernel-space leaf read-only
    /// and invalidates it on every core of the node, idle ones included,
    /// in the same call (as ARM64's broadcast does).
    pub(super) fn kernel_view_read_only(&mut self, n: u16, page: Page) {
        let kernel = self.layout[usize::from(n)].kernel_table;
        let chunk = Self::chunk_of(page);
        let slot = (chunk % ENTRIES as u64) as usize;
        let entries = self.gate.tables.get_mut(&kernel).expect("the kernel table");
        if let Entry::Leaf {
            page: first,
            write: true,
        } = entries[slot]
            && Self::chunk_of(first) == chunk
            && first == page
        {
            entries[slot] = Entry::Leaf {
                page: first,
                write: false,
            };
            for q in 0..self.layout[usize::from(n)].cores.len() {
                let core = self.core(n, q);
                self.invalidate(core, Flush::KernelSpace);
            }
        }
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
        require(self.gate.nodes[usize::from(n)].tags[tag].root == Some(root))?;
        require(self.holds(n, page) && !self.giving(page))?;
        require(!write || self.class(page)? == Class::Data)?;
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
    /// whose owes bit it sets.
    fn asked_for_tag(&mut self, n: u16, p: usize, tag: usize) -> BTreeSet<usize> {
        let node = &mut self.gate.nodes[usize::from(n)];
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
        self.writes(n)?;
        require(e0 < ENTRIES - 1 && e1 < ENTRIES)?;
        require(self.gate.nodes[usize::from(n)].tags[tag].root == Some(root))?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let Entry::Table(table) = self.gate.tables[&root][e0] else {
            return Err(Refused::Condition);
        };
        let Entry::Leaf { page, .. } = self.gate.tables[&table][e1] else {
            return Err(Refused::Condition);
        };
        self.gate.tables.get_mut(&table).expect("a table")[e1] = Entry::Absent;
        let flush = Flush::Range {
            tag,
            va: vec![(e0, e1)],
        };
        self.invalidate_here(n, p, &flush);
        let asked = self.asked_for_tag(n, p, tag);
        let record = Clearing {
            leaves: vec![page],
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
        self.writes(n)?;
        require(e0 < ENTRIES - 1)?;
        require(self.gate.nodes[usize::from(n)].tags[tag].root == Some(root))?;
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
        self.invalidate_here(
            n,
            p,
            &Flush::Range {
                tag,
                va: va.clone(),
            },
        );
        let asked = self.asked_for_tag(n, p, tag);
        let record = Clearing {
            unhooked: vec![child],
            ..Clearing::default()
        };
        self.start_request(n, p, Invalidates::Range { tag, root, va }, asked, record)
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
        self.kernel_view_writable(n, page);
        Ok(())
    }

    /// A page back to *data*: its kernel-space leaf writable again (a
    /// change that widens rights needs no invalidation).
    pub(super) fn kernel_view_writable(&mut self, n: u16, page: Page) {
        let kernel = self.layout[usize::from(n)].kernel_table;
        let slot = (Self::chunk_of(page) % ENTRIES as u64) as usize;
        let entries = self.gate.tables.get_mut(&kernel).expect("the kernel table");
        if entries[slot] == (Entry::Leaf { page, write: false }) {
            entries[slot] = Entry::Leaf { page, write: true };
        }
    }

    // The tag table.

    /// `bind(tag, root)`: the root *unbound* → *live*, the tag's entry 0
    /// → (root, ∅). Tag 0 is the kernel's.
    pub(crate) fn bind(&mut self, n: u16, tag: usize, root: Page) -> Gated {
        self.writes(n)?;
        require(tag != 0 && tag < TAGS && self.holds(n, root))?;
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
    pub(crate) fn load(&mut self, n: u16, p: usize, root: Page, tag: usize) -> Gated {
        self.writes(n)?;
        require(tag < TAGS)?;
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
        self.writes(n)?;
        let kernel = self.layout[usize::from(n)].kernel_root;
        let node = &mut self.gate.nodes[usize::from(n)];
        node.loaded[p] = (kernel, 0);
        node.idle[p].idle = true;
        Ok(())
    }

    /// The asked core's gate call at its interrupt: for each request
    /// pending, the kernel's root loaded first if its loaded root is a
    /// *dying* root the request names; then the invalidation; then the
    /// acknowledgement, the last one completing the request.
    pub(crate) fn ack(&mut self, n: u16, p: usize) -> Gated {
        self.writes(n)?;
        let pending: Vec<usize> = self.gate.nodes[usize::from(n)].pending[p]
            .iter()
            .copied()
            .collect();
        for q in pending {
            let Request::InUse { what, .. } = self.gate.nodes[usize::from(n)].requests[q].clone()
            else {
                continue;
            };
            let core = self.core(n, p);
            let kernel = self.layout[usize::from(n)].kernel_root;
            match what {
                Invalidates::Range { tag, va, .. } => {
                    self.invalidate(core, Flush::Range { tag, va })
                }
                Invalidates::Tag { tag, root } => {
                    if !self.old.dying_without_kernel_load
                        && self.gate.nodes[usize::from(n)].loaded[p].0 == root
                        && self.class(root)? == Class::Root(RootState::Dying)
                    {
                        self.gate.nodes[usize::from(n)].loaded[p] = (kernel, 0);
                    }
                    self.invalidate(core, Flush::Tag(tag));
                    // Out of the tag's set: a whole-tag invalidation while
                    // its loaded tag is another.
                    if self.gate.nodes[usize::from(n)].loaded[p].1 != tag {
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

    /// *in use* → *free*: the counts of the recorded run lowered, the
    /// recorded tables written *loose*, a give's words written.
    fn complete(&mut self, n: u16, q: usize) {
        let Request::InUse { record, what, .. } = std::mem::replace(
            &mut self.gate.nodes[usize::from(n)].requests[q],
            Request::Free,
        ) else {
            return;
        };
        for page in record.leaves {
            let chunk = Self::chunk_of(page) as usize;
            self.gate.map_counts[chunk] = self.gate.map_counts[chunk].saturating_sub(1);
            if !self.holds(n, page) {
                self.lower_use(n, page);
            }
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
        // A give's words, written once every core invalidated the run.
        if let Invalidates::KernelRun { chunk, to } = what {
            let after = Owner::Free { node: to };
            self.gate.owners[chunk as usize] = after;
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
        self.writes(n)?;
        require(tag != 0 && tag < TAGS)?;
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

    /// aspace root *live* → *dying*: the first step of the destruction,
    /// asking every core of the tag's set and every core whose loaded root
    /// is the root.
    pub(crate) fn destroy(&mut self, n: u16, p: usize, root: Page) -> Gated {
        self.writes(n)?;
        require(self.holds(n, root))?;
        require(self.gate.nodes[usize::from(n)].requests[p] == Request::Free)?;
        let tag = self.tag_of(n, root).ok_or(Refused::Condition)?;
        require(tag != 0)?;
        self.cas_class(
            root,
            Class::Root(RootState::Live),
            Class::Root(RootState::Dying),
        )?;
        let node = &self.gate.nodes[usize::from(n)];
        let mut asked: BTreeSet<usize> = node.tags[tag].set.clone();
        asked.extend((0..node.loaded.len()).filter(|&q| node.loaded[q].0 == root));
        // The caller's own part (D-153, gap 26): it loads the kernel's
        // root if it had this one loaded, and invalidates the tag.
        if asked.remove(&p) {
            let kernel = self.layout[usize::from(n)].kernel_root;
            if self.gate.nodes[usize::from(n)].loaded[p].0 == root {
                self.gate.nodes[usize::from(n)].loaded[p] = (kernel, 0);
            }
            let core = self.core(n, p);
            self.invalidate(core, Flush::Tag(tag));
            self.gate.nodes[usize::from(n)].tags[tag].set.remove(&p);
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
        let tag = self.tag_of(n, root).ok_or(Refused::Condition)?;
        let in_flight = self.gate.nodes[usize::from(n)].requests.iter().any(|r| {
            matches!(r, Request::InUse { what: Invalidates::Tag { root: r, .. }, .. } if *r == root)
        });
        require(!in_flight)?;
        let node = &self.gate.nodes[usize::from(n)];
        require(!node.loaded.iter().any(|&(_, t)| t == tag))?;
        let entries = self.gate.tables[&root];
        let level = LEVELS - 1;
        for entry in entries.iter() {
            let Entry::Table(child) = *entry else {
                continue;
            };
            let leaves = self.gate.tables[&child];
            for leaf in leaves {
                if let Entry::Leaf { page, .. } = leaf {
                    let chunk = Self::chunk_of(page) as usize;
                    self.gate.map_counts[chunk] = self.gate.map_counts[chunk].saturating_sub(1);
                }
            }
            self.gate.tables.insert(child, [Entry::Absent; ENTRIES]);
            for leaf in leaves {
                if let Entry::Leaf { page, .. } = leaf
                    && !self.holds(n, page)
                {
                    self.lower_use(n, page);
                }
            }
            self.gate.tables.insert(child, [Entry::Absent; ENTRIES]);
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
            self.kernel_view_writable(n, child);
        }
        self.gate.tables.remove(&root);
        self.gate.nodes[usize::from(n)].tags[tag] = TagEntry::default();
        self.cas_class(root, Class::Root(RootState::Dying), Class::Data)?;
        self.kernel_view_writable(n, root);
        Ok(())
    }

    // Ownership words and the kernel's own space.

    /// `memory_take` of a chunk: *free* → *taken* (the rules of taking:
    /// a free chunk, or one taken by a generation that is over); the whole
    /// gate word 0, the class bytes of the run 0; the run mapped in the
    /// kernel's own table (one leaf per chunk, entry chunk mod E).
    pub(crate) fn take(&mut self, n: u16, chunk: u64) -> Gated {
        self.writes(n)?;
        require(chunk < self.chunks)?;
        let takeable = match self.gate.owners[chunk as usize] {
            Owner::Free { .. } => true,
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
        self.gate.tables.get_mut(&kernel).expect("the kernel table")[slot] = Entry::Leaf {
            page: first,
            write: true,
        };
        Ok(())
    }

    /// `memory_give` of a chunk to node `to`: G5's counts all 0; the run
    /// unmapped from the kernel's own space and that invalidated on every
    /// core of the node, idle ones included, before the ownership word is
    /// written *free* for `to` (at the request's completion).
    pub(crate) fn give(&mut self, n: u16, p: usize, chunk: u64, to: u16) -> Gated {
        self.writes(n)?;
        require(chunk < self.chunks && to != n)?;
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
        let kernel = self.layout[usize::from(n)].kernel_table;
        let slot = (chunk % ENTRIES as u64) as usize;
        require(
            matches!(self.gate.tables[&kernel][slot], Entry::Leaf { page, .. } if page == chunk * PAGES_PER_CHUNK),
        )?;
        self.gate.tables.get_mut(&kernel).expect("the kernel table")[slot] = Entry::Absent;
        let core = self.core(n, p);
        self.invalidate(core, Flush::KernelSpace);
        let node = &self.gate.nodes[usize::from(n)];
        let asked: BTreeSet<usize> = (0..node.loaded.len())
            .filter(|&q| q != p && !(self.old.kernel_run_skips_idle && node.idle[q].idle))
            .collect();
        self.start_request(
            n,
            p,
            Invalidates::KernelRun {
                chunk,
                to: Some(to),
            },
            asked,
            Clearing::default(),
        )
    }
}
