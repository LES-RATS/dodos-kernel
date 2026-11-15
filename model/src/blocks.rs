//! `memory_control` and its calls (Memory and devices between nodes): the
//! blocks of a node, taken from the ownership words or the device table,
//! given to another node, split, and destroyed.

use crate::caps::Placing;
use crate::{Block, Model, ObjId, Outcome, Output, live};
use dodos_abi::{DeviceEntry, Error, ObjectKind, OwnState, Owner, Ownership, Rights, SlotAddr};

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
        match kind {
            0 => {
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
            1 => {
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
        let words: Vec<u64> = match kind {
            0 => self.run(place, size).collect(),
            _ => vec![place],
        };
        for &index in &words {
            let raw = match kind {
                0 => self.table.owners[index as usize],
                _ => self.table.devices[index as usize].owner.raw(),
            };
            let Some(word) = Ownership::from_raw(raw) else {
                return Err(Error::Busy);
            };
            if self.taken_here(word) || !self.table.may_take(word, self.node) {
                return Err(Error::Busy);
            }
        }
        if kind == 1 && !self.table.devices[place as usize].is_used() {
            return Err(Error::Busy);
        }
        self.vacant(at)?;
        // Effect, through the gate: each word becomes taken, in ascending
        // order, by this node and its generation, or by the reserve's code.
        let generation = self.table.generation(self.node);
        for &index in &words {
            match kind {
                0 => {
                    let before = self.table.owners[index as usize];
                    let owner = match Ownership::from_raw(before).map(|w| w.owner()) {
                        Some(Owner::Reserve(j)) => Owner::Reserve(j),
                        _ => Owner::Node(self.node),
                    };
                    let after = Ownership::taken(owner, generation);
                    assert!(self.table.write_owner(self.node, index, before, after));
                    // The gate maps the run into the kernel's own space,
                    // with page classes data and map counts 0.
                    self.kernel_runs.insert(index);
                    self.table.maps[index as usize] = 0;
                }
                _ => {
                    let mut entry = self.table.devices[index as usize];
                    entry.owner = Ownership::taken(Owner::Node(self.node), generation);
                    assert!(self.table.write_entry(self.node, index as usize, entry));
                }
            }
        }
        // The new untyped covers the run or the entry's range, with its
        // watermark at the start; ordinary memory is dirty, so the clean
        // mark is at the start. `exec` only for ordinary memory taken
        // through a memory_control capability that has it.
        let (base, bytes, device, block) = match kind {
            0 => (place, 1 << size, false, Block::Run),
            _ => {
                let entry = self.table.devices[place as usize];
                (
                    entry.base(),
                    1 << entry.log2(),
                    true,
                    Block::Entry(place as usize),
                )
            }
        };
        let exec = !device && self.caps[&parent].rights.contains(Rights::EXEC);
        let rights = match exec {
            true => Rights::ALL,
            false => Rights::ALL.without(Rights::EXEC),
        };
        let cap = self.create(ObjectKind::Untyped, bytes, Some(parent), rights, at);
        let id = self.caps[&cap].object;
        live(&mut self.objects, id).from = None;
        let untyped = live(&mut self.untypeds, id);
        (untyped.base, untyped.device, untyped.block) = (base, device, Some(block));
        Ok(Output::Done)
    }

    /// Whether a word is taken by this node's current generation: for a
    /// reserve's word, by a live block of this node.
    fn taken_here(&self, word: Ownership) -> bool {
        if word.state() != OwnState::Taken {
            return false;
        }
        match word.owner() {
            Owner::Node(owner) => {
                owner == self.node && word.generation() == self.table.generation(self.node)
            }
            Owner::Reserve(_) => false,
            Owner::Unclaimed => false,
        }
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
        // The gate refuses a give while a map count of the run is not 0.
        if self.mapped_block(object) {
            return Err(Error::Busy);
        }
        // Nothing is sent to the other node.
        self.free_block(object, Freed::Given(node as u16));
        self.remove_block_cap(cap);
        Ok(Output::Done)
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
        let half = entry.log2() - 1;
        let free = Ownership::free(Owner::Node(self.node));
        let lower = DeviceEntry {
            owner: free,
            ..DeviceEntry::used(entry.base(), half)
        };
        let upper = DeviceEntry {
            owner: free,
            ..DeviceEntry::used(entry.base() + (1 << half), half)
        };
        assert!(self.table.write_entry(self.node, spare, upper));
        assert!(self.table.write_entry(self.node, index, lower));
        live(&mut self.untypeds, object).block = None;
        self.remove_block_cap(cap);
        Ok(Output::Index(spare as u64))
    }

    /// Whether a map count of the run of a block is not 0 (G5).
    pub(crate) fn mapped_block(&self, object: ObjId) -> bool {
        let untyped = &self.untypeds[&object];
        match untyped.block {
            Some(Block::Run) => {
                let shift = self.table.geometry.chunk_shift();
                let chunks = untyped.base >> shift..(untyped.base + untyped.bytes) >> shift;
                chunks.into_iter().any(|c| self.table.maps[c as usize] != 0)
            }
            _ => false,
        }
    }

    /// The destruction of a block, or its give: the run is unmapped from
    /// the kernel's own space, and each word becomes free, in ascending
    /// order, through the gate: with the same owner for a destruction (the
    /// reserve's code for a reserve chunk), with the node named for a give.
    pub(crate) fn free_block(&mut self, object: ObjId, freed: Freed) {
        let untyped = live(&mut self.untypeds, object);
        let Some(block) = untyped.block.take() else {
            return;
        };
        let (base, bytes) = (untyped.base, untyped.bytes);
        let owner = |before: u64| match (freed, Ownership::from_raw(before).map(|w| w.owner())) {
            (Freed::Given(node), _) => Owner::Node(node),
            (Freed::Destroyed, Some(Owner::Reserve(j))) => Owner::Reserve(j),
            (Freed::Destroyed, _) => Owner::Node(self.node),
        };
        match block {
            Block::Run => {
                let shift = self.table.geometry.chunk_shift();
                for chunk in base >> shift..(base + bytes) >> shift {
                    let before = self.table.owners[chunk as usize];
                    let after = Ownership::free(owner(before));
                    self.kernel_runs.remove(&chunk);
                    assert!(self.table.write_owner(self.node, chunk, before, after));
                }
            }
            Block::Entry(index) => {
                let mut entry = self.table.devices[index];
                entry.owner = Ownership::free(owner(entry.owner.raw()));
                assert!(self.table.write_entry(self.node, index, entry));
            }
        }
    }

    /// The capability of a block, removed by `memory_give` or
    /// `memory_split`: its slot becomes empty and the untyped is destroyed.
    fn remove_block_cap(&mut self, cap: crate::CapId) {
        self.remove_leaf(cap);
    }
}

/// How the words of a block become free.
#[derive(Clone, Copy)]
pub(crate) enum Freed {
    Destroyed,
    Given(u16),
}
