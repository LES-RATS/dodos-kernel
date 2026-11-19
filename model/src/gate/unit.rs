//! The unit side of the gate, from the tables of "Gate words" (D-152): the
//! request record and its state word, the requester's gate record, the
//! holder's service records and submissions, and the unit's own context
//! entries and caches. One IOMMU unit, whose role a node holds; the ITS
//! and the interrupt remapping of that unit are served by the same holder
//! and fail with it (the small machine has one unit). Each node has one
//! domain number (its slot's range is one number). The words a request
//! moves (attach, domain, ITS, remapping) are in `devices.rs`.

use super::devices::{Bound, Handle};
use super::*;

/// The entries of a node slot's unit request table in the small machine.
pub(crate) const UNIT_ENTRIES: usize = 2;

/// The state of a request record's state word (bits 0-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordState {
    Void,
    Open,
    Done,
    Failed,
    Refused,
}

/// Words 0 to 2 of a request record: the operation and its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Words {
    /// Operation 1: attach `device` to `root` under domain number `domain`.
    Attach {
        device: u16,
        domain: u16,
        root: Page,
    },
    /// Operation 2: detach it (naming the root its header entry names).
    Detach {
        device: u16,
        domain: u16,
        root: Page,
    },
    /// Operation 3: invalidate the translations of a domain.
    Invalidate { domain: u16 },
    /// An ITS command: the device's `MAPD` with the valid bit set.
    ItsMap { device: u16, table: Page, k: u8 },
    /// The unmapping `MAPD` that the removal of its last LPI asks.
    ItsUnmap { device: u16 },
    /// An invalidation of the interrupt entry cache for one handle.
    Iec { handle: Handle },
}

/// A request record: words 0 to 2, and word 3, the state word: a serial
/// never reset and a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) words: Option<Words>,
    pub(crate) serial: u64,
    pub(crate) state: RecordState,
}

/// The requester's gate record (gate part): the serial it opened, the run
/// cleared and the count it is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GateRecord {
    pub(crate) serial: u64,
    pub(crate) leaves: Vec<Page>,
}

/// The holder's service entry: the record it accepted (slot, entry,
/// serial, the words it read) and its submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) slot: u16,
    pub(crate) entry: usize,
    pub(crate) serial: u64,
    pub(crate) words: Words,
    pub(crate) submission: Option<usize>,
}

/// A submission to the unit: what it invalidates (domains' translations,
/// devices' cached contexts, handles of the interrupt entry cache), the
/// ITS commands it queues, the R1 walks whose last unit preceded it, and
/// whether the unit completed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Submission {
    pub(crate) domains: BTreeSet<u16>,
    pub(crate) contexts: BTreeSet<u16>,
    pub(crate) its: Vec<(u16, Option<(Page, u8)>)>,
    pub(crate) iec: BTreeSet<Handle>,
    /// The walks (node, generation) whose last unit preceded it.
    pub(crate) r1: BTreeSet<(u16, u32)>,
    pub(crate) complete: bool,
}

impl Submission {
    fn is_empty(&self) -> bool {
        self.domains.is_empty()
            && self.contexts.is_empty()
            && self.its.is_empty()
            && self.iec.is_empty()
            && self.r1.is_empty()
    }
}

/// The unit side's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnitGate {
    /// The role word: its holder, its epoch and the *failed* bit.
    pub(crate) holder: u16,
    pub(crate) epoch: u32,
    pub(crate) failed: bool,
    /// Per node slot, per entry: the request record (living part).
    pub(crate) records: Vec<[Record; UNIT_ENTRIES]>,
    /// Per node, per entry: the requester's gate record (gate part).
    pub(crate) gate_records: Vec<[Option<GateRecord>; UNIT_ENTRIES]>,
    /// The holder's service entries (its gate part) and the unit's
    /// submissions.
    pub(crate) services: Vec<Service>,
    pub(crate) submissions: Vec<Submission>,
    /// The domain word of each number (the number of node slot `n` is
    /// `n`): the root bound to it.
    pub(crate) domains: Vec<Option<Page>>,
    /// The unit's context entries (kept memory): device → (domain, root).
    pub(crate) contexts: BTreeMap<u16, (u16, Page)>,
    /// The leaves of records answered *failed* or *refused*: their counts
    /// kept until the machine restarts: (node, generation, page).
    pub(crate) kept: Vec<(u16, u32, Page)>,
}

impl UnitGate {
    pub(crate) fn new(nodes: u16) -> Self {
        let empty = Record {
            words: None,
            serial: 0,
            state: RecordState::Void,
        };
        Self {
            holder: 0,
            epoch: 0,
            failed: false,
            records: vec![[empty; UNIT_ENTRIES]; usize::from(nodes)],
            gate_records: vec![[None, None]; usize::from(nodes)],
            services: Vec::new(),
            submissions: Vec::new(),
            domains: vec![None; usize::from(nodes)],
            contexts: BTreeMap::new(),
            kept: Vec::new(),
        }
    }
}

impl Machine {
    pub(super) fn unit_writes(&self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())
    }

    /// Whether node `n`'s generation holds an iospace root `root`, *live*
    /// (`dying` false) or *dying*.
    pub(super) fn ioroot_of(&self, n: u16, root: Page, dying: bool) -> bool {
        self.holds(n, root)
            && matches!(
                self.gate.classes.get(root as usize),
                Some(Class::IoRoot { dying: d, .. }) if *d == dying
            )
    }

    /// *data* → iospace root 40 + 0, *live*.
    pub(crate) fn retype_ioroot(
        &mut self,
        n: u16,
        p: usize,
        page: Page,
        split: Option<Page>,
    ) -> Gated {
        self.unit_writes(n)?;
        // The row *data* → iospace root, as for any table (G5's map count
        // and no give in flight, D-153 gaps 28 and 30; the kernel's view
        // first, gap 29).
        require(self.holds(n, page) && !self.anchored(page) && self.unmapped(page))?;
        require(self.gate.classes.get(page as usize) == Some(&Class::Data))?;
        self.leave_data(n, p, vec![page], split, Target::IoRoot)
    }

    /// Each node's iospace root at the last page of its floor, its device
    /// (numbered as the node) attached to it through the unit (the attach
    /// asked, accepted, submitted, completed, answered and released); node
    /// 0 holds the unit's role.
    pub(crate) fn with_units(mut self) -> Self {
        for n in 0..self.layout.len() as u16 {
            self.attach_through_unit(n, n);
        }
        self
    }

    /// Node `n`'s iospace root at the last page of its floor (made if it
    /// is data), and device `device` attached to it by the whole request.
    pub(crate) fn attach_through_unit(&mut self, n: u16, device: u16) {
        let root = self.layout[usize::from(n)].floor.1 - 1;
        if self.gate.classes[root as usize] == Class::Data {
            self.retype_ioroot(n, 0, root, None)
                .expect("an iospace root");
        }
        let entry = (0..UNIT_ENTRIES)
            .find(|&e| self.unit.gate_records[usize::from(n)][e].is_none())
            .expect("a free entry");
        self.iospace_attach(n, device, root, entry)
            .expect("the attach asked");
        let h = self.unit.holder;
        self.accept(h, n, entry).expect("accepted");
        self.submit(h).expect("submitted");
        self.complete_all();
        let serial = self.unit.records[usize::from(n)][entry].serial;
        let service = self
            .unit
            .services
            .iter()
            .position(|s| s.slot == n && s.entry == entry && s.serial == serial)
            .expect("its service");
        self.answer(h, service).expect("answered");
        self.release(n, entry).expect("released");
    }

    /// Each node's user space at start: a root at its floor's third page,
    /// bound to tag 1, a table at the fourth hooked at entry 0.
    pub(crate) fn with_spaces(mut self) -> Self {
        for n in 0..self.layout.len() as u16 {
            let first = self.layout[usize::from(n)].floor.0;
            self.retype_root(n, 0, first + 2, None).expect("a root");
            self.bind(n, 1, first + 2).expect("bound");
            self.take_table(n, 0, first + 3, LEVELS - 1, None)
                .expect("a table");
            self.hook(n, first + 2, 0, first + 3).expect("hooked");
        }
        self
    }

    /// A leaf of a *live* iospace root written by its owner's gate: map
    /// count + 1.
    pub(crate) fn io_map(&mut self, n: u16, root: Page, e: usize, page: Page) -> Gated {
        self.unit_writes(n)?;
        require(self.ioroot_of(n, root, false))?;
        // Not into a chunk whose give is in flight (D-153, gap 30).
        require(e < ENTRIES && self.holds(n, page) && !self.giving(page))?;
        require(self.gate.classes[page as usize] == Class::Data)?;
        require(self.gate.tables[&root][e] == Entry::Absent)?;
        self.gate.tables.get_mut(&root).expect("a root")[e] = Entry::Leaf { page, write: true };
        self.gate.map_counts[Self::chunk_of(page) as usize] += 1;
        Ok(())
    }

    /// (*s*, not *open*) → (*s* + 1, *open*): the requester's gate, words
    /// 0 to 2 written first, *s* + 1 kept in the gate record with the run
    /// cleared (none but for an unmap).
    pub(super) fn open_record(
        &mut self,
        n: u16,
        entry: usize,
        words: Words,
        leaves: Vec<Page>,
    ) -> Gated {
        require(entry < UNIT_ENTRIES)?;
        let record = self.unit.records[usize::from(n)][entry];
        let reopen = self.old.words_while_open;
        require(record.state != RecordState::Open || reopen)?;
        require(self.unit.gate_records[usize::from(n)][entry].is_none() || reopen)?;
        let serial = record.serial + 1;
        self.unit.records[usize::from(n)][entry] = Record {
            words: Some(words),
            serial,
            state: RecordState::Open,
        };
        let mut kept = self.unit.gate_records[usize::from(n)][entry]
            .take()
            .map(|g| g.leaves)
            .unwrap_or_default();
        kept.extend(leaves);
        self.unit.gate_records[usize::from(n)][entry] = Some(GateRecord {
            serial,
            leaves: kept,
        });
        Ok(())
    }

    /// A leaf cleared, the invalidation of the node's domain asked
    /// (operation 3) in the same call.
    pub(crate) fn io_unmap(&mut self, n: u16, root: Page, entry: usize, e: usize) -> Gated {
        self.unit_writes(n)?;
        require(entry < UNIT_ENTRIES && e < ENTRIES)?;
        require(self.ioroot_of(n, root, false) || self.ioroot_of(n, root, true))?;
        let Entry::Leaf { page, .. } = self.gate.tables[&root][e] else {
            return Err(Refused::Condition);
        };
        self.open_record(n, entry, Words::Invalidate { domain: n }, vec![page])?;
        self.gate.tables.get_mut(&root).expect("a root")[e] = Entry::Absent;
        Ok(())
    }

    /// The operation's checks, as the holder's gate reads them (Devices
    /// and shared hardware): `slot` is the requester.
    fn checks(&self, slot: u16, words: Words) -> bool {
        let g = self.gate.members[usize::from(slot)].generation;
        let names = |device: u16| {
            self.dev.ids.get(usize::from(device)).copied().flatten() == Some((slot, g))
        };
        match words {
            Words::Invalidate { domain } => domain == slot,
            Words::Attach {
                device,
                domain,
                root,
            } => {
                names(device)
                    && domain == slot
                    && self.unit.domains[usize::from(domain)] == Some(root)
                    && matches!(
                        self.dev.attach[usize::from(device)],
                        (Bound::Pinned | Bound::Attached, r) if r == root
                    )
            }
            Words::Detach {
                device,
                domain,
                root,
            } => {
                let context = self.unit.contexts.get(&device).copied();
                names(device)
                    && domain == slot
                    && self.unit.domains[usize::from(domain)] == Some(root)
                    && (context == Some((domain, root))
                        || context.is_none()
                            && self.dev.attach[usize::from(device)] == (Bound::Detached, root))
            }
            Words::ItsMap { device, table, k } => {
                names(device)
                    && matches!(
                        self.dev.its[usize::from(device)],
                        (Bound::Pinned | Bound::Attached, t, kk) if t == table && kk == k
                    )
            }
            // An ITS unmapping is served only while the ITS word reads
            // *attached* or *detached* (Gate words, the ITS word; D-153, gap
            // 35; pass 29's seed 3442, kept as a test).
            Words::ItsUnmap { device } => {
                names(device)
                    && (self.old.its_unmap_unordered
                        || matches!(
                            self.dev.its[usize::from(device)].0,
                            Bound::Attached | Bound::Detached
                        ))
            }
            Words::Iec { handle } => self.layout[usize::from(slot)].cores.contains(&handle.0),
        }
    }

    /// The holder's gate accepts an *open* record: the state word read
    /// (acquire) and found (*s*, *open*), then words 0 to 2; the role word
    /// held by its node; the operation's checks. An attach moves the attach
    /// word *pinned* → *attached* before it writes the context entry; a
    /// detach clears the context entry; an ITS map moves the ITS word
    /// *pinned* → *attached* just before its `MAPD`.
    pub(crate) fn accept(&mut self, h: u16, slot: u16, entry: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        require(usize::from(slot) < self.unit.records.len() && entry < UNIT_ENTRIES)?;
        let record = self.unit.records[usize::from(slot)][entry];
        require(record.state == RecordState::Open)?;
        let words = record.words.ok_or(Refused::Condition)?;
        let taken = self
            .unit
            .services
            .iter()
            .any(|s| s.slot == slot && s.entry == entry && s.serial == record.serial);
        require(!taken && self.checks(slot, words))?;
        // The holder answers a record only from a requester starting, live,
        // sealing or sealed (Gate words, the request record, row (*s*,
        // *open*) → (*s*, *done*); D-153, gap 36; pass 29's seed 265, kept as
        // a test).
        let requester = self.gate.members[usize::from(slot)].state;
        require(
            self.old.serve_fenced_requesters
                || requester.writes()
                || requester == NodeState::Sealed,
        )?;
        match words {
            Words::Attach {
                device,
                domain,
                root,
            } => {
                self.dev.attach[usize::from(device)] = (Bound::Attached, root);
                self.unit.contexts.insert(device, (domain, root));
            }
            Words::Detach { device, .. } => {
                self.unit.contexts.remove(&device);
            }
            Words::ItsMap { device, table, k } => {
                self.dev.its[usize::from(device)] = (Bound::Attached, table, k);
            }
            Words::Invalidate { .. } | Words::ItsUnmap { .. } | Words::Iec { .. } => {}
        }
        self.unit.services.push(Service {
            slot,
            entry,
            serial: record.serial,
            words,
            submission: None,
        });
        Ok(())
    }

    /// The holder's submission: one covering every record accepted before
    /// it and not yet submitted, and what the R1 walks cleared since the
    /// last one.
    pub(crate) fn submit(&mut self, h: u16) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        let id = self.unit.submissions.len();
        let mut submission = Submission::default();
        for service in self
            .unit
            .services
            .iter_mut()
            .filter(|s| s.submission.is_none())
        {
            service.submission = Some(id);
            match service.words {
                Words::Invalidate { domain } => {
                    submission.domains.insert(domain);
                }
                Words::Attach { device, domain, .. } | Words::Detach { device, domain, .. } => {
                    submission.domains.insert(domain);
                    submission.contexts.insert(device);
                }
                Words::ItsMap { device, table, k } => {
                    submission.its.push((device, Some((table, k))));
                }
                Words::ItsUnmap { device } => submission.its.push((device, None)),
                Words::Iec { handle } => {
                    submission.iec.insert(handle);
                }
            }
        }
        self.drain_walks(&mut submission);
        require(!submission.is_empty())?;
        self.unit.submissions.push(submission);
        Ok(())
    }

    /// The unit performs a submission: it invalidates what it names (its
    /// caches of them emptied), consumes the ITS commands in order, and
    /// writes the completion status. The queue is consumed in order: a
    /// submission completes only after every earlier one.
    pub(crate) fn unit_completes(&mut self, id: usize) -> Gated {
        require(!self.unit.failed)?;
        require(
            self.unit.submissions[..id.min(self.unit.submissions.len())]
                .iter()
                .all(|s| s.complete),
        )?;
        let submission = self
            .unit
            .submissions
            .get_mut(id)
            .ok_or(Refused::Condition)?;
        require(!submission.complete)?;
        submission.complete = true;
        let submission = submission.clone();
        self.iotlb
            .retain(|&(domain, _), _| !submission.domains.contains(&domain));
        self.dev
            .context_cache
            .retain(|device, _| !submission.contexts.contains(device));
        self.dev
            .iec
            .retain(|handle, _| !submission.iec.contains(handle));
        for (device, mapping) in submission.its {
            match mapping {
                Some(run) => {
                    self.dev.its_mapped.insert(device, run);
                }
                None => {
                    self.dev.its_mapped.remove(&device);
                }
            }
        }
        Ok(())
    }

    /// A healthy unit: every submission completed, in order.
    pub(crate) fn complete_all(&mut self) {
        while let Some(id) = self.unit.submissions.iter().position(|s| !s.complete) {
            self.unit_completes(id).expect("completed");
        }
    }

    /// (*s*, *open*) → (*s*, *done*): the holder's gate, once the unit
    /// completed a submission made after it accepted the record; the state
    /// word read and found (*s*, *open*). A detach writes the attach word
    /// *attached* → *detached*, an ITS unmap the ITS word, at that
    /// completion.
    pub(crate) fn answer(&mut self, h: u16, service: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h)?;
        let s = *self.unit.services.get(service).ok_or(Refused::Condition)?;
        let submitted = s.submission.ok_or(Refused::Condition)?;
        require(self.unit.submissions[submitted].complete)?;
        let record = self.unit.records[usize::from(s.slot)][s.entry];
        let expected = match self.old.holder_expects_void {
            true => RecordState::Void,
            false => RecordState::Open,
        };
        if record.serial != s.serial || record.state != expected {
            return Err(Refused::Cas);
        }
        match s.words {
            Words::Detach { device, root, .. } => {
                let word = &mut self.dev.attach[usize::from(device)];
                if *word == (Bound::Attached, root) {
                    word.0 = Bound::Detached;
                }
            }
            Words::ItsUnmap { device } => {
                let word = &mut self.dev.its[usize::from(device)];
                if word.0 == Bound::Attached {
                    word.0 = Bound::Detached;
                }
            }
            _ => {}
        }
        self.unit.records[usize::from(s.slot)][s.entry].state = RecordState::Done;
        self.unit.services.remove(service);
        Ok(())
    }

    /// The requester's gate releases on the state word at the serial its
    /// gate record holds: *done* lowers the counts of the run and frees the
    /// gate record; *failed* retires and keeps the counts; *refused* keeps
    /// them. The words the answer moves (`devices.rs`): the attach word
    /// *pinned* or *detached* → 0; the ITS word at an unmapping's answer;
    /// the retired bit at a *failed* invalidation of the interrupt entry
    /// cache.
    pub(crate) fn release(&mut self, n: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        require(entry < UNIT_ENTRIES)?;
        let gate = self.unit.gate_records[usize::from(n)][entry]
            .clone()
            .ok_or(Refused::Condition)?;
        let record = self.unit.records[usize::from(n)][entry];
        require(record.serial == gate.serial)?;
        match record.state {
            RecordState::Done => {
                for page in gate.leaves {
                    let chunk = Self::chunk_of(page) as usize;
                    self.gate.map_counts[chunk] = self.gate.map_counts[chunk].saturating_sub(1);
                }
            }
            RecordState::Failed | RecordState::Refused => {
                // *failed* retires and keeps the counts; *refused* keeps
                // them.
                let g = self.gate.members[usize::from(n)].generation;
                self.unit
                    .kept
                    .extend(gate.leaves.iter().map(|&page| (n, g, page)));
            }
            _ => return Err(Refused::Condition),
        }
        self.unit.gate_records[usize::from(n)][entry] = None;
        if let Some(words) = record.words {
            self.released(n, words, record.state);
        }
        Ok(())
    }

    /// The unit fails: the role word's *failed* bit, set by the holder's
    /// gate at `IOMMU_WAIT_NS`; every *open* record of the unit → (*s*,
    /// *failed*), and every service entry answered and cleared.
    pub(crate) fn unit_fails(&mut self) -> Gated {
        require(!self.unit.failed)?;
        self.unit.failed = true;
        for records in self.unit.records.iter_mut() {
            for record in records.iter_mut() {
                if record.state == RecordState::Open {
                    record.state = RecordState::Failed;
                }
            }
        }
        self.unit.services.clear();
        Ok(())
    }

    /// (*s*, *open*) → (*s*, *refused*): the holder's gate, the checks
    /// failing.
    pub(crate) fn refuse(&mut self, h: u16, slot: u16, entry: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        require(usize::from(slot) < self.unit.records.len() && entry < UNIT_ENTRIES)?;
        let record = self.unit.records[usize::from(slot)][entry];
        require(record.state == RecordState::Open)?;
        let words = record.words.ok_or(Refused::Condition)?;
        require(!self.checks(slot, words))?;
        self.unit.records[usize::from(slot)][entry].state = RecordState::Refused;
        Ok(())
    }

    /// **Fresh**, for the unit side: each *open* record of the slot written
    /// (*s*, *void*), the serial kept; the gate records zeroed.
    pub(crate) fn fresh_unit(&mut self, n: u16) {
        for record in self.unit.records[usize::from(n)].iter_mut() {
            if self.old.serial_reset_at_fresh {
                *record = Record {
                    words: None,
                    serial: 0,
                    state: RecordState::Void,
                };
            } else if record.state == RecordState::Open {
                record.state = RecordState::Void;
            }
        }
        self.unit.gate_records[usize::from(n)] = [None, None];
    }

    /// The unit's caches: every context entry, kept until a completed
    /// submission invalidates the device's context; every leaf of the root
    /// a cached context names, kept until a completed submission
    /// invalidates the domain; every present remapping entry, kept until an
    /// invalidation of the interrupt entry cache.
    pub(crate) fn fill_unit(&mut self) {
        for (&device, &context) in &self.unit.contexts {
            self.dev.context_cache.insert(device, context);
        }
        for (&_device, &(domain, root)) in &self.dev.context_cache {
            let Some(entries) = self.gate.tables.get(&root) else {
                continue;
            };
            if !matches!(self.gate.classes[root as usize], Class::IoRoot { .. }) {
                continue;
            }
            for (e, entry) in entries.iter().enumerate() {
                if let Entry::Leaf { page, .. } = *entry {
                    self.iotlb.insert((domain, e), page);
                }
            }
        }
        for (&handle, &owner) in &self.dev.remap {
            self.dev.iec.insert(handle, owner);
        }
    }
}
