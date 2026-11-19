//! The unit side of the gate, from the tables of "Gate words" (D-152): the
//! request record and its state word, the requester's gate record, the
//! holder's service records and submissions, the domain word, an
//! iospace's root (one level: its entries are leaves), and the unit's own
//! context entries and cache. One IOMMU unit, whose role a node holds;
//! each node has one domain number (its slot's range), and devices
//! attached at start to a node's iospace root.

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

/// A request record: words 0 to 2 (the operation and its operands; here
/// the invalidation of a domain), and word 3, the state word: a serial
/// never reset and a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    /// The domain number to invalidate (words 0 to 2), if written.
    pub(crate) words: Option<u16>,
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
/// serial, the domain it read) and its submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) slot: u16,
    pub(crate) entry: usize,
    pub(crate) serial: u64,
    pub(crate) domain: u16,
    pub(crate) submission: Option<usize>,
}

/// A submission to the unit: the domains it invalidates, and the status
/// the unit writes at its completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Submission {
    pub(crate) domains: BTreeSet<u16>,
    pub(crate) complete: bool,
}

/// The unit side's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnitGate {
    /// The role word: its holder and the *failed* bit.
    pub(crate) holder: u16,
    pub(crate) failed: bool,
    /// Per node slot, per entry: the request record (living part).
    pub(crate) records: Vec<[Record; UNIT_ENTRIES]>,
    /// Per node, per entry: the requester's gate record (gate part).
    pub(crate) gate_records: Vec<[Option<GateRecord>; UNIT_ENTRIES]>,
    /// The holder's service entries and the unit's submissions.
    pub(crate) services: Vec<Service>,
    pub(crate) submissions: Vec<Submission>,
    /// The domain word of each node's number: the root bound to it.
    pub(crate) domains: Vec<Option<Page>>,
    /// The unit's context entries: device → domain number.
    pub(crate) contexts: BTreeMap<u16, u16>,
    /// The owner of each device (its identifier word, *taken*).
    pub(crate) devices: BTreeMap<u16, u16>,
    /// The leaves of records answered *failed* or *refused*: their counts
    /// kept until the machine restarts.
    pub(crate) kept: Vec<(u16, Page)>,
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
            failed: false,
            records: vec![[empty; UNIT_ENTRIES]; usize::from(nodes)],
            gate_records: vec![[None, None]; usize::from(nodes)],
            services: Vec::new(),
            submissions: Vec::new(),
            domains: vec![None; usize::from(nodes)],
            contexts: BTreeMap::new(),
            devices: BTreeMap::new(),
            kept: Vec::new(),
        }
    }
}

impl Machine {
    fn unit_writes(&self, n: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())
    }

    /// *data* → iospace root 40 + 0, *live*, bound to the node's domain
    /// number (the domain word 0 → *R* at the first pin, done here with
    /// the device's attach at start).
    pub(crate) fn retype_ioroot(&mut self, n: u16, page: Page) -> Gated {
        self.unit_writes(n)?;
        // The row *data* → iospace root, as for any table (with G5's map
        // count, CONTRACT-GAP 28, and the kernel's view, CONTRACT-GAP 29).
        require(self.holds(n, page) && !self.anchored(page) && self.unmapped(page))?;
        require(self.gate.classes[page as usize] == Class::Data)?;
        self.gate.classes[page as usize] = Class::IoRoot;
        self.gate.not_data[Self::chunk_of(page) as usize] += 1;
        self.gate.tables.insert(page, [Entry::Absent; ENTRIES]);
        self.kernel_view_read_only(n, page);
        Ok(())
    }

    /// Device `device` of node `n` attached to its root (start-up's
    /// attach: the attach word *attached*, the domain word bound, the
    /// context entry written).
    pub(crate) fn attach_at_start(&mut self, n: u16, device: u16, root: Page) {
        self.unit.domains[usize::from(n)] = Some(root);
        self.unit.contexts.insert(device, n);
        self.unit.devices.insert(device, n);
    }

    /// Each node's iospace root at the last page of its floor, its device
    /// (numbered as the node) attached to it; node 0 holds the unit's role.
    pub(crate) fn with_units(mut self) -> Self {
        for n in 0..self.layout.len() as u16 {
            let root = self.layout[usize::from(n)].floor.1 - 1;
            self.retype_ioroot(n, root).expect("an iospace root");
            self.attach_at_start(n, n, root);
        }
        self
    }

    /// Each node's user space at start: a root at its floor's third page,
    /// bound to tag 1, a table at the fourth hooked at entry 0.
    pub(crate) fn with_spaces(mut self) -> Self {
        for n in 0..self.layout.len() as u16 {
            let first = self.layout[usize::from(n)].floor.0;
            self.retype_root(n, first + 2).expect("a root");
            self.bind(n, 1, first + 2).expect("bound");
            self.take_table(n, first + 3, LEVELS - 1).expect("a table");
            self.hook(n, first + 2, 0, first + 3).expect("hooked");
        }
        self
    }

    /// A leaf of the iospace written by its owner's gate: map count + 1.
    pub(crate) fn io_map(&mut self, n: u16, e: usize, page: Page) -> Gated {
        self.unit_writes(n)?;
        let root = self.unit.domains[usize::from(n)].ok_or(Refused::Condition)?;
        // CONTRACT-GAP 30: not into a chunk whose give is in flight.
        require(e < ENTRIES && self.holds(n, page) && !self.giving(page))?;
        require(self.gate.classes[page as usize] == Class::Data)?;
        require(self.gate.tables[&root][e] == Entry::Absent)?;
        self.gate.tables.get_mut(&root).expect("a root")[e] = Entry::Leaf { page, write: true };
        self.gate.map_counts[Self::chunk_of(page) as usize] += 1;
        Ok(())
    }

    /// A leaf cleared, the request opened: (*s*, not *open*) → (*s* + 1,
    /// *open*), words 0 to 2 written first, *s* + 1 kept in the gate
    /// record with the run cleared.
    pub(crate) fn io_unmap(&mut self, n: u16, entry: usize, e: usize) -> Gated {
        self.unit_writes(n)?;
        require(entry < UNIT_ENTRIES && e < ENTRIES)?;
        let root = self.unit.domains[usize::from(n)].ok_or(Refused::Condition)?;
        let Entry::Leaf { page, .. } = self.gate.tables[&root][e] else {
            return Err(Refused::Condition);
        };
        let record = self.unit.records[usize::from(n)][entry];
        let reopen = self.old.words_while_open;
        require(record.state != RecordState::Open || reopen)?;
        require(self.unit.gate_records[usize::from(n)][entry].is_none() || reopen)?;
        self.gate.tables.get_mut(&root).expect("a root")[e] = Entry::Absent;
        let serial = record.serial + 1;
        self.unit.records[usize::from(n)][entry] = Record {
            words: Some(n),
            serial,
            state: RecordState::Open,
        };
        let mut leaves = self.unit.gate_records[usize::from(n)][entry]
            .take()
            .map(|g| g.leaves)
            .unwrap_or_default();
        leaves.push(page);
        self.unit.gate_records[usize::from(n)][entry] = Some(GateRecord { serial, leaves });
        Ok(())
    }

    /// The holder's gate accepts an *open* record: the state word read
    /// (acquire) and found (*s*, *open*), then words 0 to 2; the role word
    /// held by its node.
    pub(crate) fn accept(&mut self, h: u16, slot: u16, entry: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        require(usize::from(slot) < self.unit.records.len() && entry < UNIT_ENTRIES)?;
        let record = self.unit.records[usize::from(slot)][entry];
        require(record.state == RecordState::Open)?;
        let domain = record.words.ok_or(Refused::Condition)?;
        let taken = self
            .unit
            .services
            .iter()
            .any(|s| s.slot == slot && s.entry == entry && s.serial == record.serial);
        require(!taken)?;
        self.unit.services.push(Service {
            slot,
            entry,
            serial: record.serial,
            domain,
            submission: None,
        });
        Ok(())
    }

    /// The holder's submission: one covering every record accepted before
    /// it and not yet submitted.
    pub(crate) fn submit(&mut self, h: u16) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        let id = self.unit.submissions.len();
        let mut domains = BTreeSet::new();
        for service in self
            .unit
            .services
            .iter_mut()
            .filter(|s| s.submission.is_none())
        {
            service.submission = Some(id);
            domains.insert(service.domain);
        }
        require(!domains.is_empty())?;
        self.unit.submissions.push(Submission {
            domains,
            complete: false,
        });
        Ok(())
    }

    /// The unit performs a submission: it invalidates the domains named
    /// (its cache of them emptied) and writes the completion status.
    pub(crate) fn unit_completes(&mut self, id: usize) -> Gated {
        require(!self.unit.failed)?;
        let submission = self
            .unit
            .submissions
            .get_mut(id)
            .ok_or(Refused::Condition)?;
        require(!submission.complete)?;
        submission.complete = true;
        let domains = submission.domains.clone();
        self.iotlb
            .retain(|&(domain, _), _| !domains.contains(&domain));
        Ok(())
    }

    /// (*s*, *open*) → (*s*, *done*): the holder's gate, once the unit
    /// completed a submission made after it accepted the record; the state
    /// word read and found (*s*, *open*).
    pub(crate) fn answer(&mut self, h: u16, service: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h)?;
        let s = *self.unit.services.get(service).ok_or(Refused::Condition)?;
        let submitted = s.submission.ok_or(Refused::Condition)?;
        require(self.unit.submissions[submitted].complete)?;
        let record = &mut self.unit.records[usize::from(s.slot)][s.entry];
        let expected = match self.old.holder_expects_void {
            true => RecordState::Void,
            false => RecordState::Open,
        };
        if record.serial != s.serial || record.state != expected {
            return Err(Refused::Cas);
        }
        record.state = RecordState::Done;
        self.unit.services.remove(service);
        Ok(())
    }

    /// The requester's gate releases on the state word at the serial its
    /// gate record holds: *done* lowers the counts of the run and frees the
    /// gate record; *failed* retires and keeps the counts; *refused* keeps
    /// them.
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
                self.unit.gate_records[usize::from(n)][entry] = None;
                Ok(())
            }
            RecordState::Failed | RecordState::Refused => {
                // *failed* retires and keeps the counts; *refused* keeps
                // them.
                self.unit
                    .kept
                    .extend(gate.leaves.iter().map(|&page| (n, page)));
                self.unit.gate_records[usize::from(n)][entry] = None;
                Ok(())
            }
            _ => Err(Refused::Condition),
        }
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
    /// failing (the domain word the words name bound to no root).
    pub(crate) fn refuse(&mut self, h: u16, slot: u16, entry: usize) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && !self.unit.failed)?;
        require(usize::from(slot) < self.unit.records.len() && entry < UNIT_ENTRIES)?;
        let record = self.unit.records[usize::from(slot)][entry];
        require(record.state == RecordState::Open)?;
        let domain = record.words.ok_or(Refused::Condition)?;
        require(
            self.unit
                .domains
                .get(usize::from(domain))
                .copied()
                .flatten()
                .is_none(),
        )?;
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

    /// The unit's cache: every leaf of a domain whose root a context entry
    /// names, kept until a completed submission invalidates the domain.
    pub(crate) fn fill_unit(&mut self) {
        for (&_device, &domain) in &self.unit.contexts {
            let Some(root) = self.unit.domains[usize::from(domain)] else {
                continue;
            };
            let Some(entries) = self.gate.tables.get(&root) else {
                continue;
            };
            for (e, entry) in entries.iter().enumerate() {
                if let Entry::Leaf { page, .. } = *entry {
                    self.iotlb.insert((domain, e), page);
                }
            }
        }
    }
}
