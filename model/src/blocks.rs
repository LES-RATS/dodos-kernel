//! `memory_control` and its calls (Memory and devices between nodes): the
//! blocks of a node, taken from the ownership words or the device table,
//! given to another node, split, and destroyed.

use crate::caps::Placing;
use crate::{Block, Model, ObjId, Outcome, Output, live};
use dodos_abi::{
    DeviceEntry, Error, MEMORY_TAKE_ENTRY, MEMORY_TAKE_RUN, MEMORY_TAKE_TABLES, ObjectKind, Owner,
    Ownership, Rights, SlotAddr,
};

impl Model {
    /// The capability of a call on blocks: `manage` on the memory_control.
    fn control(&self, thread: ObjId, control: SlotAddr) -> Result<crate::CapId, Error> {
        let cap = self.cap(thread, control, Rights::READ)?;
        self.is(cap, ObjectKind::MemoryControl, Rights::MANAGE)?;
        Ok(cap)
    }

    /// The chunks of a run of `1 << size` bytes at `place`.
    fn run(&self, place: u64, size: u64) -> std::ops::Range<u64> {
        let shift = self.table.geometry.chunk_shift();
        place >> shift..(place + (1 << size)) >> shift
    }

    /// `memory_take`: a block, a child of the memory_control capability.
    pub(crate) fn memory_take(
        &mut self,
        thread: ObjId,
        control: SlotAddr,
        (kind, place, size): (u64, u64, u64),
        destination: SlotAddr,
    ) -> Outcome {
        // 1. The memory_control: slot, kind, `manage`. 2. The destination.
        let parent = self.control(thread, control)?;
        let at = self.destination(thread, destination)?;
        // 3. The plain arguments, each `invalid`.
        let geometry = self.table.geometry;
        let chunk_shift = u64::from(geometry.chunk_shift());
        let entries_shift = u64::from(geometry.entries.trailing_zeros());
        // The kind is 0, 1 or 2 (D-154): 2 a run of ordinary memory for
        // tables, a table untyped, its place and size as for kind 0.
        let run = kind == MEMORY_TAKE_RUN || kind == MEMORY_TAKE_TABLES;
        match kind {
            MEMORY_TAKE_RUN | MEMORY_TAKE_TABLES => {
                if !(chunk_shift..=chunk_shift + entries_shift).contains(&size)
                    || !place.is_multiple_of(1 << size)
                {
                    return Err(Error::Invalid);
                }
                let limit = (self.table.owners.len() as u64) << chunk_shift;
                let end = place.checked_add(1 << size);
                if end.is_none_or(|end| end > limit) {
                    return Err(Error::Invalid);
                }
                if self
                    .run(place, size)
                    .any(|c| self.table.owners[c as usize] == 0)
                {
                    return Err(Error::Invalid);
                }
            }
            MEMORY_TAKE_ENTRY => {
                if size != 0 || place >= self.table.devices.len() as u64 {
                    return Err(Error::Invalid);
                }
            }
            _ => return Err(Error::Invalid),
        }
        // The depth checks: one deeper than the memory_control capability,
        // and an untyped only in a space whose root is not deeper.
        self.fits(at.space, parent, Placing::Retyped(ObjectKind::Untyped))?;
        // 4. The state: a word that fails its check, is taken by the
        // current generation of this node, or has an owner the rules of
        // taking refuse, is `busy`; an unused entry is `busy`; then the
        // destination.
        let words: Vec<u64> = match run {
            true => self.run(place, size).collect(),
            false => vec![place],
        };
        for &index in &words {
            let raw = match run {
                true => self.table.owners[index as usize],
                false => self.table.devices[index as usize].owner.raw(),
            };
            let Some(word) = Ownership::from_raw(raw) else {
                return Err(Error::Busy);
            };
            if self.taken_here(word) || !self.table.may_take(word, self.node) {
                return Err(Error::Busy);
            }
        }
        if !run && !self.table.devices[place as usize].is_used() {
            return Err(Error::Busy);
        }
        self.vacant(at)?;
        // Effect, through the gate: each word becomes taken, in ascending
        // order, by this node and its generation, or by the reserve's code.
        let generation = self.table.generation(self.node);
        for &index in &words {
            match run {
                true => {
                    let before = self.table.owners[index as usize];
                    let owner = match Ownership::from_raw(before).map(|w| w.owner()) {
                        Some(Owner::Reserve(j)) => Owner::Reserve(j),
                        _ => Owner::Node(self.node),
                    };
                    let after = Ownership::taken(owner, generation);
                    assert!(self.table.write_owner(self.node, index, before, after));
                    // The gate maps the run into the kernel's own space
                    // with large entries, writable for kind 0 and
                    // read-only for kind 2, with page classes data, and
                    // writes the whole gate word of each chunk: 0 for kind
                    // 0, the table bit with its three counts 0 for kind 2
                    // (`memory_take`, G13, D-151, D-154).
                    self.kernel_runs.insert(index);
                    self.added.0 += 1;
                    let tables = kind == MEMORY_TAKE_TABLES;
                    match tables {
                        true => self.kernel_read_only.insert(index),
                        false => self.kernel_read_only.remove(&index),
                    };
                    let pages = self.table.geometry.chunk() / self.table.geometry.page();
                    let stored: Vec<u64> = self
                        .table
                        .classes
                        .range(index * pages..(index + 1) * pages)
                        .map(|(&page, _)| page)
                        .collect();
                    for page in stored {
                        self.table.set_class(page, None);
                    }
                    let word = match tables {
                        true => dodos_abi::GateWord::TABLE,
                        false => dodos_abi::GateWord::ORDINARY,
                    };
                    self.table.write_gate_word(index, word);
                }
                false => {
                    let mut entry = self.table.devices[index as usize];
                    entry.owner = Ownership::taken(Owner::Node(self.node), generation);
                    assert!(self.table.write_entry(self.node, index as usize, entry));
                    // G13: its leaf count is reset.
                    std::sync::Arc::make_mut(&mut self.table.device_leaves)[index as usize] = 0;
                }
            }
        }
        // The new untyped covers the run or the entry's range, with its
        // watermark at the start; ordinary memory is dirty, so the clean
        // mark is at the start. `exec` only for ordinary memory taken
        // through a memory_control capability that has it.
        let (base, bytes, device, block) = match run {
            true => (place, 1 << size, false, Block::Run),
            false => {
                let entry = self.table.devices[place as usize];
                (
                    entry
                        .base()
                        .expect("the model's device table holds no port entry"),
                    1 << entry.log2(),
                    true,
                    Block::Entry(place as usize),
                )
            }
        };
        // `exec` only for kind 0 taken through a memory_control capability
        // that has it, never for kind 1 or kind 2; device memory and table
        // memory have no clean mark (D-154).
        let tables = kind == MEMORY_TAKE_TABLES;
        let exec = !device && !tables && self.caps[&parent].rights.contains(Rights::EXEC);
        let rights = match exec {
            true => Rights::ALL,
            false => Rights::ALL.without(Rights::EXEC),
        };
        let cap = self.create(ObjectKind::Untyped, bytes, Some(parent), rights, at);
        let id = self.caps[&cap].object;
        live(&mut self.objects, id).from = None;
        let untyped = live(&mut self.untypeds, id);
        (untyped.base, untyped.device, untyped.block) = (base, device, Some(block));
        untyped.table = tables;
        Ok(Output::Done)
    }

    /// Whether a word is taken by this node's current generation: for a
    /// reserve's word, taken with this node's generation while this node
    /// holds the reserve's role (`role_claim`: such a word "was taken by the
    /// node the role word names, with the generation the word holds"; the
    /// final review's verification of lot B, finding 1).
    fn taken_here(&self, word: Ownership) -> bool {
        let generation = self.table.generation(self.node);
        self.table.taken_by(word.raw(), self.node, generation)
    }

    /// The block argument of `memory_give` and `memory_split`: `write` on
    /// the untyped, which must be the capability of a block of this node
    /// (`invalid`, judged right after its three checks).
    fn block(&self, thread: ObjId, untyped: SlotAddr) -> Result<(crate::CapId, ObjId), Error> {
        let cap = self.cap(thread, untyped, Rights::READ)?;
        let object = self.is(cap, ObjectKind::Untyped, Rights::WRITE)?.object;
        let block = self.untypeds[&object].block.is_some();
        if !block || self.objects[&object].root != cap {
            return Err(Error::Invalid);
        }
        Ok((cap, object))
    }

    /// The state shared by `memory_give` and `memory_split`: `busy` if the
    /// untyped capability has a descendant, then if a piece of it lives.
    fn idle_block(&self, cap: crate::CapId, object: ObjId) -> Result<(), Error> {
        if !self.caps[&cap].children.is_empty() || self.untypeds[&object].live != 0 {
            return Err(Error::Busy);
        }
        Ok(())
    }

    /// The physical address of frame object `frame` (the tests' input).
    #[cfg(test)]
    pub(crate) fn frame_base(&self, frame: ObjId) -> u64 {
        self.frames[&frame].base
    }

    /// A faulty kernel's call of its gate, its derivation list ignored:
    /// give chunk `chunk` to node `to` (D-148). The gate checks what it
    /// owns: the chunk's owner word taken by this node's generation, its
    /// map count and its grant count 0 (G5), and that no request of the
    /// node names a give or a destruction of the chunk (the gate's `busy`,
    /// D-159, settling gap 49). Says whether it gave.
    pub fn gate_give_chunk(&mut self, chunk: u64, to: u16) -> bool {
        let Some(&raw) = self.table.owners.get(chunk as usize) else {
            return false;
        };
        if self.run_in_flight(chunk) {
            return false;
        }
        let generation = self.table.generation(self.node);
        let owned = Ownership::from_raw(raw).is_some_and(|w| {
            w.state() == dodos_abi::OwnState::Taken
                && w.owner() == Owner::Node(self.node)
                && w.generation() == generation
        });
        // G5 (D-148, D-150, D-154): map count 0, grant count 0 and count of
        // pages not *data* 0, so a table chunk that holds a table, a root
        // or an ITS table is never given.
        let word = self.table.gate_word(chunk);
        if !owned || word.map_count() != 0 || word.grants() != 0 || word.not_data() != 0 {
            return false;
        }
        self.table
            .write_owner(self.node, chunk, raw, Ownership::free(Owner::Node(to)))
    }

    /// Whether a request of the node names a give or a destruction of
    /// `chunk` (The machine table, releasing and giving; D-159).
    fn run_in_flight(&self, chunk: u64) -> bool {
        let shift = self.table.geometry.chunk_shift();
        let requests = self.cores.iter().filter_map(|c| c.request.as_ref());
        requests
            .filter_map(|r| match &r.completes {
                crate::cores::Completes::Run(freed) => Some(freed),
                _ => None,
            })
            .any(|freed| freed.block == Block::Run && freed.chunks(shift).contains(&chunk))
    }

    /// A faulty kernel's call of its gate: change page `page`, of its own
    /// memory and of class data, to `class` (a table, say), its derivation
    /// list ignored. The gate asks for the ownership word taken by the node
    /// with its generation, or the page in its table floor, and for the
    /// table bit of the chunk's gate word (G5, the page class byte's rows
    /// from *data*, D-154): no leaf and no grant's anchor ever targets a
    /// table chunk, so it reads no map count and walks no grant table.
    /// Says whether it changed it.
    pub fn gate_retype_page(&mut self, page: u64, class: dodos_abi::PageClass) -> bool {
        let geometry = self.table.geometry;
        let chunk = (page * geometry.page()) >> geometry.chunk_shift();
        let generation = self.table.generation(self.node);
        let word = self.table.owners.get(chunk as usize);
        let owned = word.is_some_and(|&raw| self.table.taken_by(raw, self.node, generation));
        let floor = self.table.table_floors[self.node as usize]
            .iter()
            .any(|&(s, e)| s <= page * geometry.page() && page * geometry.page() < e);
        let data = !self.table.classes.contains_key(&page);
        if !(owned || floor) || !data || !self.table.table_chunk(chunk) {
            return false;
        }
        self.table.set_class(page, Some(class));
        true
    }

    /// A faulty kernel's call of its gate: a leaf toward page `page` of its
    /// own memory, whatever its frames say. G3 (3) refuses it into a chunk
    /// whose gate word has the table bit set (D-154). Says whether the
    /// gate would write it; nothing changes.
    pub fn gate_leaf_allowed(&self, page: u64) -> bool {
        let geometry = self.table.geometry;
        let at = page * geometry.page();
        let generation = self.table.generation(self.node);
        self.table.holds_page(at, self.node, generation)
            && !self.table.classes.contains_key(&page)
            && !self.table.table_chunk(at >> geometry.chunk_shift())
    }

    /// A faulty kernel's call of its gate: a grant entry whose anchor
    /// covers page `page`. G6 accepts an anchor only over ordinary memory
    /// the node holds whose chunks' gate words have the table bit clear
    /// (D-154). Says whether the gate would write it; nothing changes.
    pub fn gate_anchor_allowed(&self, page: u64) -> bool {
        self.gate_leaf_allowed(page)
    }

    /// `memory_give`: destroys the block and gives its chunks, or its
    /// device entry, to the node.
    pub(crate) fn memory_give(
        &mut self,
        thread: ObjId,
        control: SlotAddr,
        untyped: SlotAddr,
        node: u64,
    ) -> Outcome {
        self.control(thread, control)?;
        let (cap, object) = self.block(thread, untyped)?;
        if node >= u64::from(self.table.nodes) || node == u64::from(self.node) {
            return Err(Error::Invalid);
        }
        self.idle_block(cap, object)?;
        self.return_watermark(object)?;
        // The gate refuses a give while a map count of the run is not 0.
        if self.mapped_block(object) {
            return Err(Error::Busy);
        }
        let run = self.untypeds[&object].block == Some(Block::Run);
        if run {
            self.request_free()?;
        }
        // The capability is removed and the block destroyed; a run is
        // unmapped from the kernel's own space and that invalidated on every
        // core of the node, idle ones included, before any word is written
        // (D-131): the words are written at the completion. Nothing is sent
        // to the other node.
        let freed = self.take_block(object, Freed::Given(node as u16), false);
        self.remove_leaf(cap);
        let Some(freed) = freed else {
            return Ok(Output::Done);
        };
        if !run {
            self.write_freed(freed);
            return Ok(Output::Done);
        }
        let flush = crate::cores::Flush::Kernel(freed.chunks(self.table.geometry.chunk_shift()));
        match self.start_request(flush, crate::cores::Completes::Run(freed), Some(thread)) {
            crate::cores::Started::Asked => Ok(Output::Waits),
            _ => {
                self.write_freed(freed);
                Ok(Output::Done)
            }
        }
    }

    /// The destruction of a block, one unit of a removal step: a run is
    /// unmapped from the kernel's own space and that invalidated on every
    /// core of the node; on x86-64 and RISC-V in a node of several cores
    /// the step starts the request of its core and returns `again`, and the
    /// words are written at the completion. `None` once the words are
    /// written; `Some(true)` if this unit wrote them, `Some(false)` if the
    /// step must end with `again`.
    pub(crate) fn unmap_block(&mut self, object: ObjId) -> Option<bool> {
        let block = self.untypeds[&object].block?;
        // A watermark ready to return is returned first, by the barrier
        // (A node of several cores, D-138).
        if self.return_watermark(object).is_err() {
            return Some(false);
        }
        let in_flight = self.cores.iter().filter_map(|c| c.request.as_ref()).any(
            |r| matches!(&r.completes, crate::cores::Completes::Run(f) if f.object == Some(object)),
        );
        if in_flight || (block == Block::Run && self.request_free().is_err()) {
            return Some(false);
        }
        let freed = self.take_block(object, Freed::Destroyed, true)?;
        if block != Block::Run {
            self.write_freed(freed);
            return Some(true);
        }
        let flush = crate::cores::Flush::Kernel(freed.chunks(self.table.geometry.chunk_shift()));
        match self.start_request(flush, crate::cores::Completes::Run(freed), None) {
            crate::cores::Started::Complete => {
                self.write_freed(freed);
                Some(true)
            }
            _ => Some(false),
        }
    }

    /// `memory_split`: cuts the entry of a device block into two halves.
    pub(crate) fn memory_split(
        &mut self,
        thread: ObjId,
        control: SlotAddr,
        untyped: SlotAddr,
    ) -> Outcome {
        self.control(thread, control)?;
        let (cap, object) = self.block(thread, untyped)?;
        let Some(Block::Entry(index)) = self.untypeds[&object].block else {
            return Err(Error::Invalid);
        };
        let page = self.page().trailing_zeros();
        let entry = self.table.devices[index];
        if entry.log2() <= page {
            return Err(Error::Invalid);
        }
        self.idle_block(cap, object)?;
        self.return_watermark(object)?;
        // G5: a split destroys the block, so the entry's leaf count must be
        // 0 (D-141; the final review's verification of lot B, finding 2).
        if self.mapped_block(object) {
            return Err(Error::Busy);
        }
        let unclaimed = Ownership::free(Owner::Unclaimed);
        let spare = self
            .table
            .devices
            .iter()
            .position(|e| !e.is_used() && e.owner == unclaimed)
            .ok_or(Error::Busy)?;
        // The entry keeps the lower half; the unused entry of lowest index
        // that is unclaimed gets the upper half; both are free, this node
        // their owner.
        // The model's device table holds no port entry, whose split is
        // `invalid` (I/O ports, D-150).
        let half = entry.log2() - 1;
        let base = entry.base().expect("an entry of memory");
        let free = Ownership::free(Owner::Node(self.node));
        let halves = |at| DeviceEntry::used(at, half).expect("a half of a page or more");
        let lower = DeviceEntry {
            owner: free,
            ..halves(base)
        };
        let upper = DeviceEntry {
            owner: free,
            ..halves(base + (1 << half))
        };
        assert!(self.table.write_entry(self.node, spare, upper));
        assert!(self.table.write_entry(self.node, index, lower));
        live(&mut self.untypeds, object).block = None;
        self.remove_leaf(cap);
        Ok(Output::Index(spare as u64))
    }

    /// Whether a map count or a grant count of the run of a block, or the
    /// leaf or grant count of its device entry, is not 0 (G5; D-148: the
    /// grant count, so that a kernel that ignores its own derivation list
    /// cannot give, split or destroy granted memory).
    pub(crate) fn mapped_block(&self, object: ObjId) -> bool {
        let untyped = &self.untypeds[&object];
        match untyped.block {
            Some(Block::Run) => {
                let shift = self.table.geometry.chunk_shift();
                let chunks = untyped.base >> shift..(untyped.base + untyped.bytes) >> shift;
                // G5 (D-150, D-154): map count 0, grant count 0 and count
                // of pages not *data* 0.
                chunks.into_iter().any(|c| {
                    self.table.map_count(c) != 0
                        || self.table.grant_count(c) != 0
                        || self.table.gate_word(c).not_data() != 0
                })
            }
            Some(Block::Entry(index)) => {
                self.table.device_leaves[index] != 0 || self.table.device_grants[index] != 0
            }
            None => false,
        }
    }

    /// The destruction of a block when nothing is left to invalidate.
    pub(crate) fn free_block(&mut self, object: ObjId, freed: Freed) {
        if let Some(freed) = self.take_block(object, freed, false) {
            self.write_freed(freed);
        }
    }

    /// The run of a block leaves the kernel's own space (the gate unmaps
    /// it), and what its words become is kept for their write. A block that
    /// is destroyed by steps keeps its place until the words are written.
    fn take_block(&mut self, object: ObjId, freed: Freed, keep: bool) -> Option<FreedRun> {
        let untyped = self.untypeds.get_mut(&object)?;
        let block = match keep {
            true => untyped.block?,
            false => untyped.block.take()?,
        };
        let (base, bytes) = (untyped.base, untyped.bytes);
        let run = FreedRun {
            object: keep.then_some(object),
            block,
            base,
            bytes,
            freed,
        };
        if block == Block::Run {
            for chunk in run.chunks(self.table.geometry.chunk_shift()) {
                self.kernel_runs.remove(&chunk);
                self.kernel_read_only.remove(&chunk);
            }
        }
        Some(run)
    }

    /// Each word of a block becomes free, in ascending order, through the
    /// gate: with the same owner for a destruction (the reserve's code for
    /// a reserve chunk), with the node named for a give. Each is written by
    /// compare-and-swap from the value read when the request was opened,
    /// *taken* by this node with its generation: a word that reads
    /// anything else is no longer this request's to write (The machine
    /// table, releasing and giving; D-159).
    pub(crate) fn write_freed(&mut self, run: FreedRun) {
        let generation = self.table.generation(self.node);
        let owner = |before: u64| match (run.freed, Ownership::from_raw(before).map(|w| w.owner()))
        {
            (Freed::Given(node), _) => Owner::Node(node),
            (Freed::Destroyed, Some(Owner::Reserve(j))) => Owner::Reserve(j),
            (Freed::Destroyed, _) => Owner::Node(self.node),
        };
        match run.block {
            Block::Run => {
                for chunk in run.chunks(self.table.geometry.chunk_shift()) {
                    let before = self.table.owners[chunk as usize];
                    if !self.table.taken_by(before, self.node, generation) {
                        continue;
                    }
                    let after = Ownership::free(owner(before));
                    assert!(self.table.write_owner(self.node, chunk, before, after));
                }
            }
            Block::Entry(index) => {
                let mut entry = self.table.devices[index];
                entry.owner = Ownership::free(owner(entry.owner.raw()));
                assert!(self.table.write_entry(self.node, index, entry));
            }
        }
        if let Some(object) = run.object
            && let Some(untyped) = self.untypeds.get_mut(&object)
        {
            untyped.block = None;
        }
    }
}

/// How the words of a block become free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Freed {
    Destroyed,
    Given(u16),
}

/// The words of a block to write free once its run is unmapped from the
/// kernel's own space on every core: the block (its untyped, while a
/// destruction keeps it), its place, and what the words become.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FreedRun {
    pub(crate) object: Option<ObjId>,
    pub(crate) block: Block,
    pub(crate) base: u64,
    pub(crate) bytes: u64,
    pub(crate) freed: Freed,
}

impl FreedRun {
    /// The chunks of its run.
    pub(crate) fn chunks(&self, shift: u32) -> Vec<u64> {
        (self.base >> shift..(self.base + self.bytes) >> shift).collect()
    }
}
