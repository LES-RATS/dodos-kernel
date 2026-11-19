//! The device side of the gate, from the tables of "Gate words" (D-152):
//! the identifier words (ownership format), the attach word, the domain
//! word, an iospace root's count of devices and its *dying* state, the ITS
//! word and the `its_table` object (kind 17, D-152 complement), the R1 word
//! and the identifier and line bitmaps, the remapping entries and the
//! retired word; the unit role's take-over, and death split into its steps
//! (fence, R1 by the holder one unit per call, R6).
//!
//! The hardware is adversarial as on the CPU side: the unit caches every
//! context entry and every leaf a cached context reaches, the ITS keeps
//! every mapping it consumed, and the interrupt entry cache every present
//! remapping entry, each until a completed submission invalidates it.

use super::check::{Check, ensure};
use super::unit::{Submission, UNIT_ENTRIES, Words};
use super::*;

/// The device identifiers of the small machine (one PCI segment).
pub(crate) const DEVICES: u16 = 3;
/// Vectors per core that a remapping handle names.
pub(crate) const VECTORS: usize = 2;
/// The largest *k* of an `its_table` (2^*k* pages).
pub(crate) const ITS_K_MAX: u8 = 4;
/// Event identifiers an ITS table page holds in the small machine.
pub(crate) const ITS_EVENTS_PER_PAGE: u64 = 2;

/// A remapping handle: (physical core, vector).
pub(crate) type Handle = (usize, usize);

/// The state in bits 0-1 of an attach word or an ITS word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bound {
    Zero,
    Pinned,
    Attached,
    Detached,
}

/// The state of an R1 word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum R1State {
    None,
    /// Walking, with its cursor.
    Walking(usize),
    Done,
}

/// An R1 word: the generation it concerns and its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct R1Word {
    pub(crate) generation: u32,
    pub(crate) state: R1State,
}

/// One unit of an R1 walk: 64 words of a bitmap (the small machine's
/// bitmaps fit in one), one identifier, ITS or line word, or the handles of
/// one core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum R1Unit {
    IdBitmap,
    Id(u16),
    Its(u16),
    LineBitmap,
    Line(usize),
    Core(usize),
}

/// What a walk cleared that the next submission invalidates (the holder's
/// gate part).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Walk {
    pub(crate) contexts: BTreeSet<u16>,
    pub(crate) domains: BTreeSet<u16>,
    pub(crate) its: Vec<(u16, Option<(Page, u8)>)>,
    pub(crate) iec: BTreeSet<Handle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceGate {
    /// The identifier word of each device: *taken* (node, generation), or
    /// *free*.
    pub(crate) ids: Vec<Option<(u16, u32)>>,
    /// The attach word beside it: a state and a root.
    pub(crate) attach: Vec<(Bound, Page)>,
    /// The ITS word beside it: a state, the table's first page and *k*.
    pub(crate) its: Vec<(Bound, Page, u8)>,
    /// The `its_table` objects: first page → *k*.
    pub(crate) its_objects: BTreeMap<Page, u8>,
    /// Per node slot: the identifier bitmap and the line bitmap.
    pub(crate) id_bits: Vec<BTreeSet<u16>>,
    pub(crate) line_bits: Vec<BTreeSet<usize>>,
    /// Per node slot: the R1 word of the unit role.
    pub(crate) r1: Vec<R1Word>,
    /// The walks the holder runs (its gate part).
    pub(crate) walks: BTreeMap<u16, Walk>,
    /// The unit's remapping entries (kept memory): handle → (node,
    /// generation) whose gate composed it.
    pub(crate) remap: BTreeMap<Handle, (u16, u32)>,
    /// The retired word: the handles retired.
    pub(crate) retired: BTreeSet<Handle>,
    /// For the checker only: when each handle was last claimed and when it
    /// was retired (a step count).
    pub(crate) claimed_at: BTreeMap<Handle, u64>,
    pub(crate) retired_at: BTreeMap<Handle, u64>,
    pub(crate) clock: u64,
    /// Hardware: the unit's cached contexts, the ITS's consumed mappings,
    /// the interrupt entry cache.
    pub(crate) context_cache: BTreeMap<u16, (u16, Page)>,
    pub(crate) its_mapped: BTreeMap<u16, (Page, u8)>,
    pub(crate) iec: BTreeMap<Handle, (u16, u32)>,
}

impl DeviceGate {
    pub(crate) fn new(nodes: u16) -> Self {
        Self {
            ids: vec![None; usize::from(DEVICES)],
            attach: vec![(Bound::Zero, 0); usize::from(DEVICES)],
            its: vec![(Bound::Zero, 0, 0); usize::from(DEVICES)],
            its_objects: BTreeMap::new(),
            id_bits: vec![BTreeSet::new(); usize::from(nodes)],
            line_bits: vec![BTreeSet::new(); usize::from(nodes)],
            // Start-up: (0, done, 0) for every slot.
            r1: vec![
                R1Word {
                    generation: 0,
                    state: R1State::Done,
                };
                usize::from(nodes)
            ],
            walks: BTreeMap::new(),
            remap: BTreeMap::new(),
            retired: BTreeSet::new(),
            claimed_at: BTreeMap::new(),
            retired_at: BTreeMap::new(),
            clock: 0,
            context_cache: BTreeMap::new(),
            its_mapped: BTreeMap::new(),
            iec: BTreeMap::new(),
        }
    }
}

/// The pages of an `its_table` run.
fn run(first: Page, k: u8) -> std::ops::Range<Page> {
    first..first + (1u64 << k)
}

impl DeviceGate {
    /// A retired bit set (set only).
    pub(crate) fn retire(&mut self, handle: Handle) {
        self.clock += 1;
        if self.retired.insert(handle) {
            self.retired_at.insert(handle, self.clock);
        }
    }
}

impl Machine {
    fn generation(&self, n: u16) -> u32 {
        self.gate.members[usize::from(n)].generation
    }

    /// Whether identifier word `device` is takeable by the rules of taking:
    /// *free*, or *taken* by a generation that is over.
    fn id_takeable(&self, device: u16, by: u16) -> bool {
        match self.dev.ids[usize::from(device)] {
            None => true,
            Some((owner, g)) => {
                let m = self.gate.members[usize::from(owner)];
                // Its own word (kept for the ITS word, D-153, gap 34).
                m.generation != g || m.state == NodeState::Dead || owner == by
            }
        }
    }

    /// An iospace root's count of devices ± 1 (its class byte 40 + *n*).
    fn ioroot_devices(&mut self, root: Page, up: bool) -> Gated {
        let Class::IoRoot { devices, dying } = self.class(root)? else {
            return Err(Refused::Condition);
        };
        let after = match up {
            true => devices.checked_add(1).filter(|&d| d <= 16),
            false => devices.checked_sub(1),
        }
        .ok_or(Refused::Condition)?;
        self.cas_class(
            root,
            Class::IoRoot { devices, dying },
            Class::IoRoot {
                devices: after,
                dying,
            },
        )
    }

    // The attach word, the identifier word and the domain word.

    /// The gate refuses an attach, a detach, an ITS mapping or an ITS
    /// unmapping of a device while another record of the node naming it is
    /// *open* (Gate words, the attach word and the ITS word; D-153, gap 38;
    /// pass 29's seeds 4473 and 18395, kept as tests).
    fn device_request_open(&self, n: u16, device: u16, its: bool) -> bool {
        !self.old.concurrent_device_requests
            && self.unit.records[usize::from(n)].iter().any(|r| {
                let words = match r.words {
                    Some(Words::Attach { device, .. } | Words::Detach { device, .. }) => {
                        Some((device, false))
                    }
                    Some(Words::ItsMap { device, .. } | Words::ItsUnmap { device }) => {
                        Some((device, true))
                    }
                    _ => None,
                };
                r.state == unit::RecordState::Open && words == Some((device, its))
            })
    }

    /// `iospace_attach` with an iospace: in one gate call the identifier
    /// word *taken*, the attach word 0 → *pinned* (*R*), the root 40 + *n*
    /// → 40 + *n* + 1 (*n* < 16, *live*), the domain word bound (0 → *R*)
    /// or checked, the identifier bitmap's bit set, and operation 1 asked.
    pub(crate) fn iospace_attach(
        &mut self,
        n: u16,
        device: u16,
        root: Page,
        entry: usize,
    ) -> Gated {
        self.unit_writes(n)?;
        require(device < DEVICES && entry < UNIT_ENTRIES)?;
        require(!self.unit.failed)?;
        require(self.id_takeable(device, n))?;
        require(self.dev.attach[usize::from(device)].0 == Bound::Zero)?;
        require(self.ioroot_of(n, root, false) && !self.device_request_open(n, device, false))?;
        let domain = self.unit.domains[usize::from(n)];
        require(domain.is_none() || domain == Some(root))?;
        let words = Words::Attach {
            device,
            domain: n,
            root,
        };
        // The record first (it may be busy), then the words.
        self.open_record(n, entry, words, Vec::new())?;
        self.ioroot_devices(root, true)?;
        let g = self.generation(n);
        self.dev.ids[usize::from(device)] = Some((n, g));
        self.dev.attach[usize::from(device)] = (Bound::Pinned, root);
        self.unit.domains[usize::from(n)] = Some(root);
        self.dev.id_bits[usize::from(n)].insert(device);
        Ok(())
    }

    /// `iospace_attach` with iospace 0: operation 2 asked, naming the root
    /// the attach word names; the attach word *attached* and the
    /// identifier word the node's.
    pub(crate) fn iospace_detach(&mut self, n: u16, device: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        require(device < DEVICES && !self.unit.failed)?;
        require(self.dev.ids[usize::from(device)] == Some((n, self.generation(n))))?;
        let (Bound::Attached, root) = self.dev.attach[usize::from(device)] else {
            return Err(Refused::Condition);
        };
        require(!self.device_request_open(n, device, false))?;
        let words = Words::Detach {
            device,
            domain: n,
            root,
        };
        self.open_record(n, entry, words, Vec::new())
    }

    /// An identifier word is freed only when its attach word and its ITS
    /// word both read 0, by whichever answer brings the second to 0 (Gate
    /// words, the ownership words, row *taken* → *free*; D-153, gap 34;
    /// pass 29's seed 2845, kept as a test).
    fn free_identifier(&mut self, n: u16, device: u16, by_attach: bool) {
        let d = usize::from(device);
        let both = self.dev.attach[d].0 == Bound::Zero && self.dev.its[d].0 == Bound::Zero;
        let old = self.old.identifier_freed_by_attach_word;
        if old && by_attach || !old && both {
            self.dev.ids[d] = None;
            self.dev.id_bits[usize::from(n)].remove(&device);
        }
    }

    /// The words an answer moves, written by the requester's gate when it
    /// releases on the state word.
    pub(super) fn released(&mut self, n: u16, words: Words, state: unit::RecordState) {
        let g = self.generation(n);
        match words {
            // *pinned* or *detached* → 0 at the answer, the identifier word
            // *free*, the root 40 + *n* − 1, the domain word 0 at *n* = 0;
            // only if the word reads so with the root the entry names.
            Words::Attach { device, root, .. } | Words::Detach { device, root, .. } => {
                let d = usize::from(device);
                let word = self.dev.attach[d];
                let ends = word == (Bound::Pinned, root) || word == (Bound::Detached, root);
                if ends
                    && self.dev.ids[d] == Some((n, g))
                    && self.ioroot_devices(root, false).is_ok()
                {
                    self.dev.attach[d] = (Bound::Zero, 0);
                    self.free_identifier(n, device, true);
                    if self.gate.classes[root as usize]
                        == (Class::IoRoot {
                            devices: 0,
                            dying: false,
                        })
                        && self.unit.domains[usize::from(n)] == Some(root)
                    {
                        self.unit.domains[usize::from(n)] = None;
                    }
                }
            }
            // The ITS word *pinned* or *detached* → 0 at the answer of the
            // unmapping; the run's bits 6-7 written 0.
            Words::ItsUnmap { device } => {
                let d = usize::from(device);
                let (state, table, k) = self.dev.its[d];
                if matches!(state, Bound::Pinned | Bound::Detached)
                    && self.dev.ids[d] == Some((n, g))
                {
                    self.dev.its[d] = (Bound::Zero, 0, 0);
                    self.free_identifier(n, device, false);
                    for page in run(table, k) {
                        if let Some(c) = self.gate.classes.get_mut(page as usize)
                            && *c == (Class::ItsTable { pinned: true })
                        {
                            *c = Class::ItsTable { pinned: false };
                        }
                    }
                }
            }
            // The retired bit at the state word *failed* of an invalidation
            // of the interrupt entry cache.
            Words::Iec { handle } if state == unit::RecordState::Failed => {
                self.dev.retire(handle);
            }
            _ => {}
        }
    }

    /// iospace root 40 + 0 *live* → *dying*: the destruction; the domain
    /// word does not name it.
    pub(crate) fn io_destroy(&mut self, n: u16, root: Page) -> Gated {
        self.unit_writes(n)?;
        require(self.holds(n, root))?;
        require(self.unit.domains[usize::from(n)] != Some(root))?;
        self.cas_class(
            root,
            Class::IoRoot {
                devices: 0,
                dying: false,
            },
            Class::IoRoot {
                devices: 0,
                dying: true,
            },
        )
    }

    /// A *dying* iospace root torn down: "nothing more, since every detach
    /// was confirmed and each detach invalidates its device's context and
    /// the whole domain"; each cleared leaf's count lowered at once; the
    /// root cleared whole, then *data*.
    pub(crate) fn io_teardown(&mut self, n: u16, root: Page) -> Gated {
        self.unit_writes(n)?;
        require(self.ioroot_of(n, root, true))?;
        let entries = self.gate.tables[&root];
        for entry in entries {
            if let Entry::Leaf { page, .. } = entry {
                let chunk = Self::chunk_of(page) as usize;
                self.gate.map_counts[chunk] = self.gate.map_counts[chunk].saturating_sub(1);
            }
        }
        self.cas_class(
            root,
            Class::IoRoot {
                devices: 0,
                dying: true,
            },
            Class::Data,
        )?;
        self.gate.tables.remove(&root);
        self.kernel_view_writable(n, root);
        Ok(())
    }

    // The ITS word and the `its_table` object.

    /// `untyped_retype` of kind 17: 2^*k* pages of *data* → *ITS table*,
    /// unpinned, cleared in the call; *k* above 4 `invalid`. As for a
    /// table: held, no anchor, G5's map count and no give in flight (D-153,
    /// gaps 28 and 30), and the kernel's view first (D-153, gap 29).
    pub(crate) fn retype_its(
        &mut self,
        n: u16,
        p: usize,
        first: Page,
        k: u8,
        split: Option<Page>,
    ) -> Gated {
        self.unit_writes(n)?;
        require(k <= ITS_K_MAX)?;
        let pages = run(first, k);
        // Aligned on its size, so within one chunk (16 pages at most in a
        // chunk of 512 on the reference machine; the small machine's chunk
        // holds 4, so k ≤ 2 here).
        require(first.is_multiple_of(1 << k) && pages.end <= self.chunks * PAGES_PER_CHUNK)?;
        require(Machine::chunk_of(first) == Machine::chunk_of(pages.end - 1))?;
        for page in pages.clone() {
            require(self.holds(n, page) && !self.anchored(page) && self.unmapped(page))?;
            require(self.gate.classes[page as usize] == Class::Data)?;
        }
        self.leave_data(n, p, pages.collect(), split, Target::Its { first, k })
    }

    /// The first `irq_claim` of an LPI of the device, naming an
    /// `its_table`: the identifier word names the node; every page of the
    /// object *ITS table* with bits 6-7 0, written 1; the ITS word 0 →
    /// *pinned*; the ITS mapping asked of the holder.
    ///
    /// A later claim of an LPI of the device writes nothing: it is
    /// `invalid` if it names another object than the one pinned, or an
    /// event identifier beyond the object's 2^*k* pages.
    pub(crate) fn its_claim(
        &mut self,
        n: u16,
        device: u16,
        first: Page,
        event: u32,
        entry: usize,
    ) -> Gated {
        self.unit_writes(n)?;
        require(device < DEVICES && !self.unit.failed)?;
        require(self.dev.ids[usize::from(device)] == Some((n, self.generation(n))))?;
        let fits = |k: u8| u64::from(event) < (1u64 << k) * ITS_EVENTS_PER_PAGE;
        let (state, table, k) = self.dev.its[usize::from(device)];
        if state != Bound::Zero {
            return require(table == first && fits(k));
        }
        require(!self.device_request_open(n, device, true))?;
        let k = *self.dev.its_objects.get(&first).ok_or(Refused::Condition)?;
        require(fits(k))?;
        for page in run(first, k) {
            require(self.holds(n, page))?;
            require(self.gate.classes[page as usize] == Class::ItsTable { pinned: false })?;
        }
        let words = Words::ItsMap {
            device,
            table: first,
            k,
        };
        self.open_record(n, entry, words, Vec::new())?;
        for page in run(first, k) {
            self.gate.classes[page as usize] = Class::ItsTable { pinned: true };
        }
        self.dev.its[usize::from(device)] = (Bound::Pinned, first, k);
        Ok(())
    }

    /// The removal of the device's last LPI `irq`: the unmapping asked.
    pub(crate) fn its_unmap(&mut self, n: u16, device: u16, entry: usize) -> Gated {
        self.unit_writes(n)?;
        require(device < DEVICES)?;
        require(self.dev.ids[usize::from(device)] == Some((n, self.generation(n))))?;
        require(self.dev.its[usize::from(device)].0 != Bound::Zero)?;
        require(!self.device_request_open(n, device, true))?;
        self.open_record(n, entry, Words::ItsUnmap { device }, Vec::new())
    }

    /// The destruction of an `its_table`: refused (`busy`) while its ITS
    /// word is not 0, its pages pinned; the pages back to *data*.
    pub(crate) fn its_destroy(&mut self, n: u16, first: Page) -> Gated {
        self.unit_writes(n)?;
        let k = *self.dev.its_objects.get(&first).ok_or(Refused::Condition)?;
        for page in run(first, k) {
            require(self.holds(n, page))?;
            require(self.gate.classes[page as usize] == Class::ItsTable { pinned: false })?;
        }
        for page in run(first, k) {
            self.cas_class(page, Class::ItsTable { pinned: false }, Class::Data)?;
            self.kernel_view_writable(n, page);
        }
        self.dev.its_objects.remove(&first);
        Ok(())
    }

    // The remapping entry and the retired word.

    /// `irq_claim` of an MSI: the entry empty → present, composed by the
    /// gate; the core word names the node; the vector's retired bit clear.
    pub(crate) fn claim_msi(&mut self, n: u16, core: usize, v: usize) -> Gated {
        self.unit_writes(n)?;
        require(v < VECTORS && self.layout[usize::from(n)].cores.contains(&core))?;
        require(!self.dev.retired.contains(&(core, v)))?;
        require(!self.dev.remap.contains_key(&(core, v)))?;
        let g = self.generation(n);
        self.dev.remap.insert((core, v), (n, g));
        self.dev.clock += 1;
        self.dev.claimed_at.insert((core, v), self.dev.clock);
        Ok(())
    }

    /// A removal: present → empty, and the invalidation of the interrupt
    /// entry cache for the handle asked in the same call.
    pub(crate) fn remove_msi(&mut self, n: u16, core: usize, v: usize, entry: usize) -> Gated {
        self.unit_writes(n)?;
        let g = self.generation(n);
        require(self.dev.remap.get(&(core, v)) == Some(&(n, g)))?;
        self.open_record(n, entry, Words::Iec { handle: (core, v) }, Vec::new())?;
        self.dev.remap.remove(&(core, v));
        Ok(())
    }

    // Death, split into its steps.

    /// The fence: the member word fencing (the node's gate refuses every
    /// write from now), its cores stopped (a stopped core uses nothing it
    /// cached; it invalidates its whole TLB when it next enters a node),
    /// then fenced; R3.
    pub(crate) fn fence(&mut self, n: u16) -> Gated {
        let member = self.gate.members[usize::from(n)];
        require(member.state.writes() || member.state == NodeState::Sealed)?;
        self.gate.members[usize::from(n)].state = NodeState::Fencing;
        for p in 0..self.layout[usize::from(n)].cores.len() {
            let core = self.core(n, p);
            self.invalidate(core, Flush::All);
        }
        self.gate.members[usize::from(n)].state = NodeState::Fenced;
        // R3 by every granter, at fenced: its grants to the generation
        // pinned.
        for granter in 0..self.layout.len() {
            for grant in self.grants.grants[granter].iter_mut().flatten() {
                if grant.grantee == n && grant.state == grants::GrantState::Live {
                    grant.state = grants::GrantState::Pinned;
                }
            }
        }
        Ok(())
    }

    /// The take-over of the unit role by `s`, its holder no longer live:
    /// held (*s*, *e* + 1); the old holder's service records and walks die
    /// with its gate part; unless the unit failed, the rebuild: a context
    /// entry kept or written only for a device whose attach word reads
    /// *attached* with the root its reporter's domain word names and whose
    /// identifier word names the reporter with its generation, kept without
    /// a report for a sealed node, every other cleared; then the whole unit
    /// invalidated. A failed unit is not rebuilt.
    pub(crate) fn take_over(&mut self, s: u16) -> Gated {
        let h = self.unit.holder;
        let old = self.gate.members[usize::from(h)].state;
        require(s != h && self.gate.members[usize::from(s)].state.writes())?;
        require(!old.writes() && old != NodeState::Sealed)?;
        self.unit.holder = s;
        self.unit.epoch += 1;
        self.unit.services.clear();
        self.dev.walks.clear();
        if self.unit.failed {
            return Ok(());
        }
        let mut contexts = BTreeMap::new();
        for device in 0..DEVICES {
            let d = usize::from(device);
            let (Bound::Attached, root) = self.dev.attach[d] else {
                continue;
            };
            let Some((x, g)) = self.dev.ids[d] else {
                continue;
            };
            let m = self.gate.members[usize::from(x)];
            let reports = m.state.writes() || m.state == NodeState::Sealed;
            if reports && m.generation == g && self.unit.domains[usize::from(x)] == Some(root) {
                contexts.insert(device, (x, root));
            }
        }
        self.unit.contexts = contexts;
        self.iotlb.clear();
        self.dev.context_cache.clear();
        self.dev.iec.clear();
        Ok(())
    }

    /// The units of node `x`'s R1 walk, from its bitmaps (which only its
    /// own gate writes, and it is fencing).
    pub(crate) fn r1_units(&self, x: u16) -> Vec<R1Unit> {
        let mut units = vec![R1Unit::IdBitmap];
        let ids = &self.dev.id_bits[usize::from(x)];
        units.extend(ids.iter().map(|&i| R1Unit::Id(i)));
        units.extend(ids.iter().map(|&i| R1Unit::Its(i)));
        units.push(R1Unit::LineBitmap);
        units.extend(
            self.dev.line_bits[usize::from(x)]
                .iter()
                .map(|&l| R1Unit::Line(l)),
        );
        units.extend(
            self.layout[usize::from(x)]
                .cores
                .iter()
                .map(|&c| R1Unit::Core(c)),
        );
        units
    }

    /// R1 word any older generation → (*g*, walking, 0): the holder's,
    /// for a node fencing, fenced, unfenced or dead whose word is not
    /// (*g*, done); a successor restarts a walk the old holder left.
    pub(crate) fn r1_start(&mut self, h: u16, x: u16) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && usize::from(x) < self.layout.len())?;
        let m = self.gate.members[usize::from(x)];
        require(matches!(
            m.state,
            NodeState::Fencing | NodeState::Fenced | NodeState::Dead
        ))?;
        let word = self.dev.r1[usize::from(x)];
        let done = R1Word {
            generation: m.generation,
            state: R1State::Done,
        };
        require(word != done)?;
        let restart = word.generation == m.generation && !self.dev.walks.contains_key(&x);
        require(word.generation < m.generation || word.state == R1State::None || restart)?;
        self.dev.r1[usize::from(x)] = R1Word {
            generation: m.generation,
            state: R1State::Walking(0),
        };
        self.dev.walks.insert(x, Walk::default());
        Ok(())
    }

    /// walking, cursor *c* → *c* + 1: one unit per gate call, each word
    /// visited acted on only if it names *x* with *g*.
    pub(crate) fn r1_unit(&mut self, h: u16, x: u16) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && self.dev.walks.contains_key(&x))?;
        let g = self.generation(x);
        let R1Word {
            generation,
            state: R1State::Walking(c),
        } = self.dev.r1[usize::from(x)]
        else {
            return Err(Refused::Condition);
        };
        require(generation == g)?;
        let units = self.r1_units(x);
        let unit = *units.get(c).ok_or(Refused::Condition)?;
        let failed = self.unit.failed;
        let names = |dev: &DeviceGate, i: u16| dev.ids[usize::from(i)] == Some((x, g));
        match unit {
            R1Unit::IdBitmap | R1Unit::LineBitmap => {}
            R1Unit::Id(i) => {
                if names(&self.dev, i)
                    && self.dev.attach[usize::from(i)].0 == Bound::Attached
                    && !failed
                {
                    let context = self.unit.contexts.remove(&i);
                    let walk = self.dev.walks.get_mut(&x).expect("the walk");
                    walk.contexts.insert(i);
                    walk.domains.insert(context.map_or(x, |(d, _)| d));
                }
            }
            R1Unit::Its(i) => {
                if names(&self.dev, i)
                    && self.dev.its[usize::from(i)].0 == Bound::Attached
                    && !failed
                {
                    let walk = self.dev.walks.get_mut(&x).expect("the walk");
                    walk.its.push((i, None));
                }
            }
            R1Unit::Line(l) => {
                if l < parks::SPIS && self.parks.lines[l] == Some((x, g)) {
                    self.parks.lines[l] = None;
                    // The distributor's holder deactivates X's SPIs.
                    self.parks.active[l] = false;
                }
            }
            // R1's closing invalidation covers every handle of each core it
            // visits, present or not (Gate words, the R1 word, row walking →
            // done; D-153, gap 32; pass 29's seed 101, kept as a test).
            R1Unit::Core(core) => {
                for v in 0..VECTORS {
                    let present = self.dev.remap.get(&(core, v)).is_some_and(|&(o, _)| o == x);
                    if present {
                        self.dev.remap.remove(&(core, v));
                        if failed {
                            self.dev.retire((core, v));
                        }
                    }
                    if !failed && (present || !self.old.r1_present_handles_only) {
                        let walk = self.dev.walks.get_mut(&x).expect("the walk");
                        walk.iec.insert((core, v));
                    }
                }
            }
        }
        self.dev.r1[usize::from(x)].state = R1State::Walking(c + 1);
        Ok(())
    }

    /// What the walks cleared, into the holder's next submission; a walk
    /// past its last unit named in it.
    pub(super) fn drain_walks(&mut self, submission: &mut Submission) {
        let ended: Vec<u16> = self
            .dev
            .walks
            .keys()
            .copied()
            .filter(|&x| {
                self.dev.r1[usize::from(x)].state == R1State::Walking(self.r1_units(x).len())
            })
            .collect();
        for walk in self.dev.walks.values_mut() {
            let walk = std::mem::take(walk);
            submission.contexts.extend(walk.contexts);
            submission.domains.extend(walk.domains);
            submission.its.extend(walk.its);
            submission.iec.extend(walk.iec);
        }
        for x in ended {
            submission.r1.insert((x, self.generation(x)));
        }
    }

    /// walking → done: the holder's, at the completion of an invalidation
    /// submitted after the walk's last unit (on a failed unit, at the last
    /// unit); the attach and ITS words of *x*'s generation that the walk
    /// cleared written *detached* (R1 detaches every *attached* word).
    pub(crate) fn r1_done(&mut self, h: u16, x: u16) -> Gated {
        self.unit_writes(h)?;
        require(self.unit.holder == h && self.dev.walks.contains_key(&x))?;
        let g = self.generation(x);
        let end = self.r1_units(x).len();
        require(
            self.dev.r1[usize::from(x)]
                == R1Word {
                    generation: g,
                    state: R1State::Walking(end),
                },
        )?;
        let failed = self.unit.failed;
        let confirmed = self
            .unit
            .submissions
            .iter()
            .any(|s| s.complete && s.r1.contains(&(x, g)));
        require(failed || confirmed)?;
        if !failed {
            for d in 0..usize::from(DEVICES) {
                if self.dev.ids[d] != Some((x, g)) {
                    continue;
                }
                if self.dev.attach[d].0 == Bound::Attached
                    && !self.unit.contexts.contains_key(&(d as u16))
                {
                    self.dev.attach[d].0 = Bound::Detached;
                }
                if self.dev.its[d].0 == Bound::Attached {
                    self.dev.its[d].0 = Bound::Detached;
                }
            }
        }
        self.dev.r1[usize::from(x)].state = R1State::Done;
        self.dev.walks.remove(&x);
        Ok(())
    }

    /// The device side of R6, after the barrier: refused (the node stays
    /// fenced) while an attach or ITS word of the generation reads
    /// *attached* or a line word still names it, or while the R1 word of a
    /// unit that has not failed is not (*g*, done); then the identifier
    /// words *free* with their attach and ITS words 0, the domain word 0,
    /// the bitmaps cleared whole, and on a failed unit the present entries
    /// toward the node's cores retired.
    pub(super) fn r6_devices_hold(&self, n: u16) -> Gated {
        let g = self.generation(n);
        let done = self.dev.r1[usize::from(n)]
            == R1Word {
                generation: g,
                state: R1State::Done,
            };
        require(self.unit.failed || done)?;
        for d in 0..usize::from(DEVICES) {
            if self.dev.ids[d] == Some((n, g)) {
                require(self.dev.attach[d].0 != Bound::Attached)?;
                require(self.dev.its[d].0 != Bound::Attached)?;
            }
        }
        require(self.parks.lines.iter().all(|&l| l != Some((n, g))))
    }

    /// The device side's writes of R6, once `r6_devices_hold` holds.
    pub(super) fn r6_devices(&mut self, n: u16) {
        let g = self.generation(n);
        for d in 0..usize::from(DEVICES) {
            if self.dev.ids[d] == Some((n, g)) {
                self.dev.attach[d] = (Bound::Zero, 0);
                self.dev.its[d] = (Bound::Zero, 0, 0);
                self.dev.ids[d] = None;
            }
        }
        self.unit.domains[usize::from(n)] = None;
        // The kept counts of the node's records end at R6, which reaches
        // here only once no attach word of the generation reads *attached*
        // (Gate words, the gate word, cell Death; D-153, gap 37).
        self.unit.kept.retain(|&(m, _, _)| m != n);
        self.dev.id_bits[usize::from(n)].clear();
        self.dev.line_bits[usize::from(n)].clear();
        // On a failed unit serving the node's cores, R6 retires every handle
        // of those cores, present or not (Gate words, the retired word, row 0
        // → 1; D-153, gap 33; pass 29's seed 1233, kept as a test).
        if self.unit.failed {
            let handles: Vec<Handle> = self
                .dev
                .remap
                .iter()
                .filter(|&(_, &(o, _))| o == n)
                .map(|(&h, _)| h)
                .collect();
            for handle in handles {
                self.dev.remap.remove(&handle);
                self.dev.retire(handle);
            }
            if !self.old.r6_retires_present_only {
                for &core in &self.layout[usize::from(n)].cores {
                    for v in 0..VECTORS {
                        self.dev.retire((core, v));
                    }
                }
            }
        }
    }

    /// A healthy recovery of node `n` by the unit's holder: a take-over
    /// first if `n` held the role, then the walk, one unit per call, the
    /// submission, its completion and done.
    pub(crate) fn recover(&mut self, n: u16) {
        let h = self.unit.holder;
        if !self.gate.members[usize::from(h)].state.writes() {
            let successor = (0..self.layout.len() as u16)
                .find(|&s| s != h && self.gate.members[usize::from(s)].state.writes());
            match successor {
                Some(s) => self.take_over(s).expect("the take-over"),
                None => return,
            }
        }
        let h = self.unit.holder;
        if self.r1_start(h, n).is_err() {
            return;
        }
        while self.r1_unit(h, n).is_ok() {}
        if !self.unit.failed {
            self.submit(h).expect("the walk's submission");
            self.complete_all();
        }
        self.r1_done(h, n).expect("done");
    }

    /// The `its_table` objects whose pages R6 or a fresh start turned back
    /// into data are gone with them.
    pub(super) fn forget_its_objects(&mut self) {
        let classes = &self.gate.classes;
        self.dev
            .its_objects
            .retain(|&first, _| matches!(classes[first as usize], Class::ItsTable { .. }));
    }

    // The checker's rules, one per row of the tables above.

    pub(super) fn check_devices(&self) -> Check {
        let running = |x: u16| {
            let s = self.gate.members[usize::from(x)].state;
            s.writes() || s == NodeState::Sealed
        };
        // Attach words: a root that is an iospace root, a taken identifier;
        // each root's count of devices is the attach words naming it.
        let mut counted: BTreeMap<Page, u8> = BTreeMap::new();
        for d in 0..usize::from(DEVICES) {
            let (state, root) = self.dev.attach[d];
            if state == Bound::Zero {
                continue;
            }
            ensure(
                matches!(self.gate.classes[root as usize], Class::IoRoot { .. }),
                "ATTACH: an attach word names a page that is no iospace root",
            )?;
            ensure(
                self.dev.ids[d].is_some(),
                "ATTACH: an attach word beside a free identifier word",
            )?;
            *counted.entry(root).or_default() += 1;
        }
        for (page, class) in self.gate.classes.iter().enumerate() {
            if let Class::IoRoot { devices, .. } = *class {
                ensure(
                    devices == counted.get(&(page as u64)).copied().unwrap_or(0),
                    "COUNT: an iospace root's count of devices is not its attach words",
                )?;
            }
        }
        // The domain word: names a root of its node with devices; a root
        // with devices is the one its node's domain word names.
        for x in 0..self.layout.len() as u16 {
            if let Some(root) = self.unit.domains[usize::from(x)] {
                ensure(
                    matches!(self.gate.classes[root as usize], Class::IoRoot { devices, .. } if devices > 0),
                    "DOMAIN: a domain word names a root with no device",
                )?;
                ensure(
                    self.holds(x, root),
                    "DOMAIN: a domain word names a root its node does not hold",
                )?;
            }
        }
        for (page, class) in self.gate.classes.iter().enumerate() {
            if let Class::IoRoot { devices, .. } = *class
                && devices > 0
            {
                let named = self.unit.domains.contains(&Some(page as u64));
                ensure(named, "DOMAIN: a root with devices no domain word names")?;
            }
        }
        // (G) for devices: every context the unit holds or caches names an
        // iospace root, the root the device's attach word names *attached*,
        // under the domain word that names it, of the identifier's owner.
        let contexts = self.unit.contexts.iter().chain(&self.dev.context_cache);
        for (&device, &(domain, root)) in contexts {
            let d = usize::from(device);
            ensure(
                matches!(self.gate.classes[root as usize], Class::IoRoot { .. }),
                "G: the unit walks a page that is no iospace root",
            )?;
            ensure(
                self.dev.attach[d] == (Bound::Attached, root),
                "G: the unit translates for a device whose attach word is not attached",
            )?;
            ensure(
                self.unit.domains[usize::from(domain)] == Some(root),
                "G: a context under a domain number bound to another root",
            )?;
            ensure(
                self.dev.ids[d].map(|(o, _)| o) == Some(domain),
                "G: a context under a domain number outside its device's owner's range",
            )?;
        }
        // The ITS writes only into pinned ITS tables, for a word attached.
        for (&device, &(table, k)) in &self.dev.its_mapped {
            ensure(
                self.dev.its[usize::from(device)] == (Bound::Attached, table, k),
                "G: the ITS maps a device whose ITS word is not attached",
            )?;
            for page in run(table, k) {
                ensure(
                    self.gate.classes.get(page as usize) == Some(&Class::ItsTable { pinned: true }),
                    "G: the ITS writes into a page that is no pinned ITS table",
                )?;
            }
        }
        for d in 0..usize::from(DEVICES) {
            let (state, table, k) = self.dev.its[d];
            if state == Bound::Zero {
                continue;
            }
            for page in run(table, k) {
                ensure(
                    self.gate.classes.get(page as usize) == Some(&Class::ItsTable { pinned: true }),
                    "ITS: an ITS word names a run that is not pinned",
                )?;
            }
        }
        for (&first, &k) in &self.dev.its_objects {
            for page in run(first, k) {
                ensure(
                    matches!(
                        self.gate.classes.get(page as usize),
                        Some(Class::ItsTable { .. })
                    ),
                    "ITS: an its_table whose pages are not ITS tables",
                )?;
            }
        }
        // Bitmaps: the identifier bitmap is the node's words; the line
        // bitmap a superset of its lines.
        for x in 0..self.layout.len() as u16 {
            if !running(x) {
                continue;
            }
            let g = self.generation(x);
            let ids: BTreeSet<u16> = (0..DEVICES)
                .filter(|&i| self.dev.ids[usize::from(i)] == Some((x, g)))
                .collect();
            ensure(
                ids == self.dev.id_bits[usize::from(x)],
                "BITMAP: the identifier bitmap is not the node's identifier words",
            )?;
            for (l, line) in self.parks.lines.iter().enumerate() {
                ensure(
                    *line != Some((x, g)) || self.dev.line_bits[usize::from(x)].contains(&l),
                    "BITMAP: a line of the node missing from its line bitmap",
                )?;
            }
        }
        // R1: a dead node's word reads (g, done) unless the unit failed;
        // once done, nothing of the generation is left at the unit.
        for x in 0..self.layout.len() as u16 {
            let m = self.gate.members[usize::from(x)];
            let done = self.dev.r1[usize::from(x)]
                == R1Word {
                    generation: m.generation,
                    state: R1State::Done,
                };
            if m.state == NodeState::Dead {
                ensure(
                    done || self.unit.failed,
                    "R1: a node dead before its R1 word reads done",
                )?;
            }
            let over = matches!(
                m.state,
                NodeState::Fencing | NodeState::Fenced | NodeState::Dead
            );
            if !(over && done) {
                continue;
            }
            let g = m.generation;
            ensure(
                self.parks.lines.iter().all(|&l| l != Some((x, g))),
                "R1: a line word of a generation whose R1 is done",
            )?;
            if self.unit.failed {
                continue;
            }
            for (&device, _) in self.unit.contexts.iter().chain(&self.dev.context_cache) {
                ensure(
                    self.dev.ids[usize::from(device)] != Some((x, g)),
                    "R1: the unit translates for a device of a generation whose R1 is done",
                )?;
            }
            for &device in self.dev.its_mapped.keys() {
                ensure(
                    self.dev.ids[usize::from(device)] != Some((x, g)),
                    "R1: the ITS maps a device of a generation whose R1 is done",
                )?;
            }
        }
        // Remapping: a present entry is composed for a core of the node, of
        // a generation not over; the cache holds no entry of a generation
        // that is over unless its handle is retired; a retired handle is
        // never present again.
        for (&(core, _), &(x, g)) in &self.dev.remap {
            ensure(
                self.layout[usize::from(x)].cores.contains(&core),
                "REMAP: a remapping entry toward a core of another node",
            )?;
            let m = self.gate.members[usize::from(x)];
            ensure(
                m.generation == g && m.state != NodeState::Dead,
                "REMAP: a remapping entry of a generation that is over",
            )?;
        }
        for (&handle, &(x, g)) in &self.dev.iec {
            let m = self.gate.members[usize::from(x)];
            let over = m.generation != g || m.state == NodeState::Dead;
            ensure(
                !over || self.dev.retired.contains(&handle),
                "IEC: the unit caches an interrupt entry of a generation that is over",
            )?;
        }
        // A handle a faulty kernel claimed again before the answer of its
        // invalidation may be retired while present (its own node's loss);
        // no claim follows a retirement.
        for (handle, &retired) in &self.dev.retired_at {
            let claimed = self.dev.claimed_at.get(handle).copied().unwrap_or(0);
            ensure(
                !self.dev.remap.contains_key(handle) || claimed < retired,
                "RETIRED: a handle claimed after it was retired",
            )?;
        }
        Ok(())
    }
}
