//! Memory and devices: mapping records, address spaces and their tables.

use crate::caps::Clearing;
use crate::{
    ASpace, CapId, Map, Mapping, Model, ObjId, Outcome, Output, Record, Translation, live,
};
use dodos_abi::{
    Error, Geometry, IOSPACE_MAX_DEVICES, ObjectKind, PageSize, Rights, SlotAddr,
    io_mapping_rights, mapping_range, mapping_rights,
};
use std::collections::BTreeSet;

/// Address spaces and their tables: the lower tables hooked in a space of
/// `levels` levels. A hooked table always has a present entry, so the
/// hooked tables are those on the paths of the translations: a table of
/// level k (1 for the last, `levels` − 1 for the one under the root) covers
/// P × E^k bytes, and a page of size index s is a leaf of the table of
/// level s + 1, under one table of each level above it.
pub(crate) fn hooked(space: &ASpace, levels: u32, geometry: Geometry) -> u64 {
    tables(
        space.translations.iter().map(|(&at, t)| (at, t.size)),
        levels,
        geometry,
    )
    .len() as u64
}

/// The lower tables on the paths of pages, each named by its level and the
/// address bits above what it covers.
pub(crate) fn tables(
    pages: impl Iterator<Item = (u64, PageSize)>,
    levels: u32,
    geometry: Geometry,
) -> BTreeSet<(u32, u64)> {
    let page = u32::from(geometry.page_shift[0]);
    let per_level = geometry.entries.trailing_zeros();
    let mut found = BTreeSet::new();
    for (at, size) in pages {
        let leaf = (u32::from(geometry.page_shift[size as usize]) - page) / per_level + 1;
        for level in leaf..levels {
            found.insert((level, at >> (page + per_level * level)));
        }
    }
    found
}

impl Model {
    /// Mapping records: fills the record of a frame capability with a
    /// mapping in `space`, writes its translations, and places the
    /// capability at the head of the space's list.
    pub(crate) fn record_mapping(&mut self, cap: CapId, space: ObjId, mapping: Mapping) {
        let frame = self.frames[&self.caps[&cap].object];
        let size = frame.page;
        let bytes = self.bytes(size);
        let aspace = live(&mut self.aspaces, space);
        for page in mapping.first..mapping.first + mapping.count {
            let at = mapping.address + (page - mapping.first) * bytes;
            let translation = Translation {
                cap,
                page,
                size,
                rights: mapping.rights,
            };
            aspace.translations.insert(at, translation);
            self.table.gated.0 = true;
            self.added.0 += 1;
        }
        aspace.records.push(cap);
        live(&mut self.caps, cap).record = Record::Mapping(mapping);
        self.count_leaves(cap, frame, mapping, 1);
        self.rehook(space);
    }

    /// The gate over page tables, G4: the chunks that the leaves of a
    /// mapping target, one entry per leaf and chunk: a leaf of a page size
    /// larger than a chunk counts in each chunk it covers.
    pub(crate) fn leaf_chunks(&self, frame: crate::Frame, first: u64, count: u64) -> Vec<u64> {
        let bytes = self.bytes(frame.page);
        let shift = self.table.geometry.chunk_shift();
        let mut chunks = Vec::new();
        for page in first..first + count {
            let at = frame.base + page * bytes;
            chunks.extend(at >> shift..(at + bytes).div_ceil(1 << shift));
        }
        chunks
    }

    /// G2 and G4: what the leaves of a frame are counted in. A page of
    /// device memory or of the global read-only set lies in no chunk: a
    /// device leaf counts in the leaf count of its device entry, a leaf of
    /// the global read-only set in no count; a leaf of ordinary memory in
    /// the map counts of the chunks it touches.
    pub(crate) fn leaf_target(&self, frame: crate::Frame) -> Target {
        if frame.memory != dodos_abi::MemoryType::WriteBack {
            let end = frame.base + frame.pages * self.bytes(frame.page);
            let index = self.table.devices.iter().position(|entry| {
                let base = entry.base();
                entry.is_used() && base <= frame.base && end <= base + (1 << entry.log2())
            });
            // A device frame lies in an entry of the device table; one that
            // lies in none (a state only a test builds) is counted nowhere.
            return index.map_or(Target::Nothing, Target::Entry);
        }
        let end = frame.base + frame.pages * self.bytes(frame.page);
        let kept = self
            .table
            .kept
            .iter()
            .any(|&(s, e)| s <= frame.base && end <= e);
        match frame.kept || kept {
            true => Target::Nothing,
            false => Target::Chunks,
        }
    }

    /// G4: adds or takes the leaves of a mapping to the map counts, or to
    /// the leaf count of a device entry; the leaves of a frame under a
    /// grant count in its use entry (G6).
    fn count_leaves(&mut self, cap: CapId, frame: crate::Frame, mapping: Mapping, sign: i64) {
        if let Some(remote) = self.remotes.get(&self.caps[&cap].object) {
            let node = self.node as usize;
            if let Some(used) = self.table.uses[node].get_mut(&remote.use_entry) {
                used.leaves = used
                    .leaves
                    .checked_add_signed(sign * mapping.count as i64)
                    .expect("leaves");
            }
            return;
        }
        match self.leaf_target(frame) {
            Target::Chunks => {
                for chunk in self.leaf_chunks(frame, mapping.first, mapping.count) {
                    self.table.add_leaves(chunk, sign);
                }
            }
            Target::Entry(index) => {
                let leaves = &mut std::sync::Arc::make_mut(&mut self.table.device_leaves)[index];
                *leaves = leaves
                    .checked_add_signed(sign * mapping.count as i64)
                    .expect("a leaf count never goes below 0");
            }
            Target::Nothing => {}
        }
    }

    /// The tables of a space made from an untyped follow its translations
    /// (G2): a table no translation needs any more goes to the free list,
    /// keeping its class; a table needed and not hooked is taken from the
    /// free list (most recently freed first), and the gate gives it the
    /// class of its level (an iospace's tables are of class *iospace
    /// table*). The start-up aspace's tables are kept memory: not followed.
    pub(crate) fn rehook(&mut self, space: ObjId) {
        self.rehook_by(space, None);
    }

    /// `rehook`, for the clearing of the record of `by`: the tables it
    /// unhooks wait for the invalidation of that record before they join
    /// the free list (D-137).
    fn rehook_by(&mut self, space: ObjId, by: Option<CapId>) {
        // The start-up aspace follows its translations once the start-up
        // has placed its root and its tables in the table floor (D-154).
        if self.aspaces[&space].root.is_none() {
            return;
        }
        let levels = self.levels(space);
        let io = self.iospaces.contains_key(&space);
        let geometry = self.machine.geometry;
        let aspace = &self.aspaces[&space];
        let pages = aspace.translations.iter().map(|(&at, t)| (at, t.size));
        let needed = tables(pages, levels, geometry);
        let gone: Vec<(u32, u64)> = aspace
            .hooked
            .keys()
            .filter(|key| !needed.contains(key))
            .copied()
            .collect();
        let new: Vec<(u32, u64)> = needed
            .iter()
            .filter(|key| !aspace.hooked.contains_key(key))
            .copied()
            .collect();
        let aspace = live(&mut self.aspaces, space);
        for key in gone.iter().rev() {
            let page = aspace.hooked.remove(key).expect("hooked");
            match by {
                Some(cap) => aspace.unhooked.push((cap, page)),
                None => aspace.free.push(page),
            }
        }
        let mut classes = Vec::new();
        for key in new {
            let page = aspace.free.pop().expect("a table on the free list");
            aspace.hooked.insert(key, page);
            classes.push((page, key.0));
        }
        let page = geometry.page();
        for (at, level) in classes {
            // G2: a table of the class of the level it is hooked at, an
            // iospace's as *iospace table of level k* (D-137).
            let class = match io {
                true => dodos_abi::PageClass::iospace_table(level as u8).expect("a level"),
                false => dodos_abi::PageClass::table(level as u8).expect("a level"),
            };
            self.table.set_class(at / page, Some(class));
        }
    }

    /// The class of the root table of a new space: a page table of level
    /// *L*, or an iospace root with no device attached (D-137).
    pub(crate) fn root_class(&self, space: ObjId) -> dodos_abi::PageClass {
        match self.iospaces.contains_key(&space) {
            true => dodos_abi::PageClass::iospace_root(0).expect("0 devices"),
            false => {
                let levels = self.machine.geometry.levels as u8;
                dodos_abi::PageClass::table(levels).expect("a level")
            }
        }
    }

    /// Mapping records: empties the record of a frame capability that holds
    /// a mapping, in one gate call: its entries are cleared, its leaves'
    /// counts lowered, and it leaves the space's list. For a node of one
    /// core, for ARM64, and for a space whose tag no other core loaded.
    pub(crate) fn empty_mapping(&mut self, cap: CapId) {
        if self.clear_mapping(cap).is_some() {
            self.finish_unmapping(cap);
        }
    }

    /// The entries of a mapping record are cleared and the tables left
    /// empty unhooked; the record is *unmapping*, its leaves still counted.
    /// Returns the addresses of its pages.
    pub(crate) fn clear_mapping(&mut self, cap: CapId) -> Option<Vec<u64>> {
        let Record::Mapping(mapping) = self.caps[&cap].record else {
            return None;
        };
        let aspace = live(&mut self.aspaces, mapping.space);
        let addresses: Vec<u64> = aspace
            .translations
            .iter()
            .filter(|(_, t)| t.cap == cap)
            .map(|(&at, _)| at)
            .collect();
        aspace.translations.retain(|_, t| t.cap != cap);
        self.table.gated.0 = true;
        live(&mut self.caps, cap).record = Record::Unmapping(mapping);
        self.rehook_by(mapping.space, Some(cap));
        Some(addresses)
    }

    /// The completion of an unmapping: through the gate the counts of its
    /// leaves are lowered (G4, G6), the record is emptied and it leaves the
    /// space's list.
    pub(crate) fn finish_unmapping(&mut self, cap: CapId) {
        let Some(Record::Unmapping(mapping)) = self.caps.get(&cap).map(|c| c.record) else {
            return;
        };
        if let Some(aspace) = self.aspaces.get_mut(&mapping.space) {
            aspace.records.retain(|&other| other != cap);
            // Its unhooked tables join the free list now (D-137).
            let (done, kept): (Vec<_>, Vec<_>) =
                aspace.unhooked.iter().partition(|&&(by, _)| by == cap);
            aspace.unhooked = kept;
            aspace.free.extend(done.into_iter().map(|(_, page)| page));
        }
        live(&mut self.caps, cap).record = Record::Empty;
        let frame = self.frames[&self.caps[&cap].object];
        self.count_leaves(cap, frame, mapping, -1);
    }

    /// An unmapping a failed unit never confirmed: the record is emptied
    /// with its counts kept, which are never lowered (IOMMU); under a grant
    /// the count is the use entry's, which the holder then never frees
    /// (D-142).
    pub(crate) fn freeze_unmapping(&mut self, cap: CapId) {
        let Some(Record::Unmapping(mapping)) = self.caps.get(&cap).map(|c| c.record) else {
            return;
        };
        if let Some(aspace) = self.aspaces.get_mut(&mapping.space) {
            aspace.records.retain(|&other| other != cap);
        }
        live(&mut self.caps, cap).record = Record::Empty;
        let object = self.caps[&cap].object;
        if let Some(remote) = self.remotes.get(&object) {
            *self.frozen_uses.entry(remote.use_entry).or_default() += mapping.count;
            return;
        }
        let frame = self.frames[&object];
        match self.leaf_target(frame) {
            Target::Chunks => {
                for chunk in self.leaf_chunks(frame, mapping.first, mapping.count) {
                    *self.frozen.entry(chunk).or_default() += 1;
                }
            }
            Target::Entry(index) => *self.frozen_entries.entry(index).or_default() += mapping.count,
            Target::Nothing => {}
        }
    }

    /// The unmapping of a record (`aspace_unmap`, `iospace_unmap`, a
    /// removal, a `Revoke`'s step, R2): its entries cleared, and their
    /// invalidation on every core of the node that may hold them (TLB) or
    /// by the unit of an iospace that serves devices (IOMMU). `waiter` is
    /// the thread of a call, which waits; a step has none. `Again` with
    /// nothing changed when the request of the core, or an entry of the
    /// unit request table, is not free.
    pub(crate) fn begin_unmap(
        &mut self,
        cap: CapId,
        waiter: Option<ObjId>,
    ) -> Result<Unmapped, Error> {
        let Record::Mapping(mapping) = self.caps[&cap].record else {
            return Ok(Unmapped::Done);
        };
        let space = mapping.space;
        // A dying aspace whose tag is invalidated on every core: its records
        // are emptied with their counts lowered at once.
        if self.aspaces[&space].flushed {
            self.empty_mapping(cap);
            return Ok(Unmapped::Done);
        }
        if self.iospaces.contains_key(&space) {
            let Some(iommu) = self.unit_of_space(space) else {
                self.empty_mapping(cap);
                return Ok(Unmapped::Done);
            };
            if self.unit_requests.iter().all(Option::is_some) {
                return Err(Error::Again);
            }
            let role = self.unit_role(self.iommus[&iommu].unit);
            self.clear_mapping(cap);
            let who = match waiter {
                Some(thread) => crate::units::Waiter::Thread(Some(thread)),
                None => crate::units::Waiter::Record(cap),
            };
            let domain = self.domains.get(&space).copied();
            let op = crate::rings::UnitOp::Invalidate { domain };
            self.unit_request(role, op, who, Some(cap))?;
            return Ok(Unmapped::Waits);
        }
        self.request_free()?;
        let addresses = self.clear_mapping(cap).expect("a mapping");
        let flush = crate::cores::Flush::Range { space, addresses };
        match self.start_request(flush, crate::cores::Completes::Record(cap), waiter) {
            crate::cores::Started::Complete => {
                self.finish_unmapping(cap);
                Ok(Unmapped::Done)
            }
            crate::cores::Started::Asked => Ok(Unmapped::Waits),
            crate::cores::Started::InUse => unreachable!("judged free"),
        }
    }
}

/// What `begin_unmap` did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Unmapped {
    /// The record is empty, its counts lowered.
    Done,
    /// The record is unmapping; the waiter waits, or the step ends.
    Waits,
}

/// What the leaves of a frame are counted in (G4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// The map counts of the chunks they touch: ordinary memory.
    Chunks,
    /// The leaf count of this entry of the device table: device memory.
    Entry(usize),
    /// No count: the global read-only set.
    Nothing,
}

/// IOMMU: the levels of the tables of an `iospace` of `width` address bits.
pub(crate) fn io_levels(geometry: Geometry, width: u32) -> u32 {
    let page = u32::from(geometry.page_shift[0]);
    (width - page).div_ceil(geometry.entries.trailing_zeros())
}

impl Model {
    /// The levels of the tables of a space: *L* for an `aspace`; for an
    /// `iospace`, as many as its address width needs (x86-64: four with 48
    /// bits, three with 39).
    pub(crate) fn levels(&self, space: ObjId) -> u32 {
        let geometry = self.machine.geometry;
        match self.iospaces.contains_key(&space) {
            true => io_levels(geometry, self.machine.io_width),
            false => geometry.levels,
        }
    }

    /// `aspace_map` and `iospace_map`, in the order of their checks.
    pub(crate) fn aspace_map(&mut self, thread: ObjId, map: Map, kind: ObjectKind) -> Outcome {
        // 1. The space: slot, kind, `write`. A dying space is `gone`.
        let space = self.object(thread, map.space, kind, Rights::WRITE)?;
        // 2. The frame: slot, kind; the rights it needs depend on the rights
        // argument. Then the untyped, unless it is 0: slot, kind, `write`,
        // then that it is the space's own table untyped, the one named
        // when the space was created (`invalid`; any untyped is `invalid`
        // for the start-up aspace, made from none; D-154).
        // Memory under a grant: the gate maps a frame of another node only
        // while the grant is live (an invalid remote capability is `gone`,
        // judged with its slot).
        let cap = self.cap(thread, map.frame, Rights::READ)?;
        self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        let untyped = self.optional(thread, map.untyped, ObjectKind::Untyped, Rights::WRITE)?;
        if untyped.is_some() && untyped != self.aspaces[&space].table_untyped {
            return Err(Error::Invalid);
        }
        // 3. The plain arguments.
        let frame = self.frames[&self.caps[&cap].object];
        let (limit, rights) = match kind {
            ObjectKind::ASpace => (self.machine.geometry.user_end, mapping_rights(map.rights)),
            // IOMMU: the frame's page size must be one every unit supports
            // (`unsupported`, the first plain check); the range lies inside
            // the iospace's address width; the rights are a non-empty subset
            // of `read` and `write`.
            _ => {
                if self.machine.io_page_sizes & 1 << frame.page as u32 == 0 {
                    return Err(Error::Unsupported);
                }
                (1 << self.machine.io_width, io_mapping_rights(map.rights))
            }
        };
        mapping_range(
            map.address,
            map.first,
            map.count,
            frame.pages,
            self.bytes(frame.page),
            limit,
        )?;
        let rights = rights?;
        if !self.caps[&cap].rights.contains(rights) {
            return Err(Error::Denied);
        }
        // 4. The state: the record is full (`busy`, even if it names this
        // space), unless it is a partial mapping of the same call, which
        // goes on where it stopped (G3); some address of the range not yet
        // written is translated (`busy`); a page's class refuses the leaf
        // (`denied`, G3); a map count would pass its maximum (`busy`, G4);
        // the tables needed are more than the free list and the untyped
        // supply (`no_memory`), and nothing is taken.
        // The same call is the same frame capability, space, address, first
        // page, page count and rights (G3).
        let done = match self.caps[&cap].record {
            Record::Empty => 0,
            Record::Unmapping(_) => return Err(Error::Busy),
            Record::Mapping(m)
                if m.partial
                    && m.space == space
                    && m.address == map.address
                    && m.first == map.first
                    && m.asked == map.count
                    && m.rights == rights =>
            {
                m.count
            }
            _ => return Err(Error::Busy),
        };
        let size = self.bytes(frame.page);
        let start = map.address + done * size;
        let end = map.address + map.count * size;
        let aspace = &self.aspaces[&space];
        let before = aspace.translations.range(..start).next_back();
        let covers = before.is_some_and(|(&at, t)| at + self.bytes(t.size) > start);
        if covers || aspace.translations.range(start..end).next().is_some() {
            return Err(Error::Busy);
        }
        // G3, the bound: this step writes the leaves whose checks fit in the
        // gate's chunks per call (*E*; a leaf of device memory reads one
        // word), at least one.
        let budget = self.machine.gate_chunks;
        let device = frame.memory != dodos_abi::MemoryType::WriteBack;
        let target = self.leaf_target(frame);
        let cost = |model: &Self, page: u64| match target {
            Target::Chunks => model.leaf_chunks(frame, page, 1).len() as u64,
            Target::Entry(_) | Target::Nothing => 1,
        };
        let (mut step, mut spent) = (0, 0);
        while done + step < map.count {
            let c = cost(self, map.first + done + step);
            if step > 0 && spent + c > budget {
                break;
            }
            spent += c;
            step += 1;
        }
        let first = map.first + done;
        let remote_frame = self.remotes.contains_key(&self.caps[&cap].object);
        // G3 (3) and (4): the class of every page the step's leaves target.
        let leaf = dodos_abi::Leaf {
            write: rights.contains(Rights::WRITE),
            exec: rights.contains(Rights::EXEC),
            device_type: device,
        };
        if (first..first + step).any(|page| !self.class_of(frame, page).permits(leaf)) {
            return Err(Error::Denied);
        }
        // G3 (3) (D-154): a leaf never points into a chunk whose gate word
        // has the table bit set, whatever the class of its page.
        let in_tables = target == Target::Chunks
            && self
                .leaf_chunks(frame, first, step)
                .iter()
                .any(|&chunk| self.table.table_chunk(chunk));
        if in_tables && !remote_frame {
            return Err(Error::Denied);
        }
        // The gate over page tables, G4: a map count, or the leaf count of
        // a device entry, that would pass its maximum is `busy`. A leaf of
        // the global read-only set is counted nowhere.
        let remote = self.remotes.contains_key(&self.caps[&cap].object);
        let max = self.machine.map_count_max;
        let overflow = !remote
            && match self.leaf_target(frame) {
                Target::Chunks => {
                    let mut added: std::collections::BTreeMap<u64, u64> = Default::default();
                    for chunk in self.leaf_chunks(frame, first, step) {
                        *added.entry(chunk).or_default() += 1;
                    }
                    added
                        .iter()
                        .any(|(&chunk, &leaves)| self.table.map_count(chunk) + leaves > max)
                }
                Target::Entry(index) => self.table.device_leaves[index] + step > max,
                Target::Nothing => false,
            };
        if overflow {
            return Err(Error::Busy);
        }
        let aspace = &self.aspaces[&space];
        let levels = self.levels(space);
        let held: Vec<(u64, PageSize)> = aspace
            .translations
            .iter()
            .map(|(&at, t)| (at, t.size))
            .collect();
        let geometry = self.machine.geometry;
        let hooked = tables(held.iter().copied(), levels, geometry);
        let pages = (done..done + step).map(|n| (map.address + n * size, frame.page));
        let wanted = tables(held.into_iter().chain(pages), levels, geometry).len() as u64;
        // The free list holds the tables neither hooked nor unhooked: an
        // unhooked table joins it only at the completion of the invalidation
        // of its unhooking (D-137; the final review's verification of lot B,
        // O16 = D1: counted as free, they made `rehook` pop an empty list).
        let free = aspace.tables - hooked.len() as u64 - aspace.unhooked.len() as u64;
        let missing = (wanted - hooked.len() as u64).saturating_sub(free);
        let table = self.page();
        let supplied = untyped.map_or(0, |untyped| {
            let untyped = &self.untypeds[&untyped];
            let start = untyped.watermark.next_multiple_of(table);
            untyped.bytes.saturating_sub(start) / table
        });
        if missing > supplied {
            return Err(Error::NoMemory);
        }
        // Where the units cache entries that are not present, a map of an
        // iospace that serves devices asks the holder to invalidate, and its
        // last state check is `again` with nothing changed when the unit
        // request table is full (D-141).
        let asks =
            self.machine.caching_mode && self.iospaces.get(&space).is_some_and(|s| !s.is_empty());
        if asks && self.unit_requests.iter().all(Option::is_some) {
            return Err(Error::Again);
        }
        // The barrier's dependency: bit X of `maps_of` is set through the
        // gate before the first translation to memory of node X. Under a
        // grant, the use entry's count is raised before the grant entry is
        // read again; a grant no longer live with its serial undoes the
        // count and refuses (`gone`).
        if let Some(remote) = self.remotes.get(&self.caps[&cap].object).copied() {
            let entry = self.table.grants[remote.node as usize].get(&remote.grant);
            let live_grant = entry.is_some_and(|e| {
                e.state == crate::table::GrantState::Live && e.serial == remote.serial
            });
            if !live_grant {
                return Err(Error::Gone);
            }
            // The holder's gate counts a leaf only in the use entry the
            // grant entry records once answered: before the granting node
            // has consumed the `SendAnswer`, `again` with nothing changed
            // (Grants, the count says what the holder translates, D-141).
            let entry = entry.expect("live");
            if !entry.answered {
                return Err(Error::Again);
            }
            if entry.use_index != Some(remote.use_entry) {
                return Err(Error::Gone);
            }
            // A use entry whose root is removed counts no new leaf (Grants,
            // the count says what the holder translates, D-142).
            let uses = &self.table.uses[self.node as usize];
            if uses
                .get(&remote.use_entry)
                .is_some_and(|u| u.root.is_none())
            {
                return Err(Error::Gone);
            }
            let granter = remote.node as usize;
            self.table.beacons[self.node as usize].maps_of[granter] = true;
        }
        // Effect: the missing tables are taken, free list first, then the
        // untyped, cleared within the call; the record is filled and the
        // capability goes to the head of the space's list.
        for _ in 0..missing {
            let untyped = untyped.expect("an untyped supplies the tables");
            let taken = self
                .take(untyped, table, table, Clearing::WithinCall)
                .expect("checked above");
            // G2: the page goes to the free list; the gate gives it the
            // class of the level it is hooked at (`rehook`).
            let at = self.untypeds[&untyped].base + taken.at;
            live(&mut self.aspaces, space).free.push(at);
        }
        live(&mut self.aspaces, space).tables += missing;
        let partial = done + step < map.count;
        if done > 0 {
            self.extend_mapping(cap, step, partial);
        } else {
            let mapping = Mapping {
                space,
                address: map.address,
                first: map.first,
                count: step,
                rights,
                partial,
                asked: map.count,
            };
            self.record_mapping(cap, space, mapping);
        }
        if partial {
            return Err(Error::Again);
        }
        // The last step asks the unit (operation 3, the iospace's domain
        // number) and waits in *waiting for a unit*: done ends the call, a
        // unit found failed `busy` with the mapping kept and counted.
        if asks && let Some(iommu) = self.unit_of_space(space) {
            let role = self.unit_role(self.iommus[&iommu].unit);
            let domain = self.domains.get(&space).copied();
            let op = crate::rings::UnitOp::Invalidate { domain };
            let waiter = crate::units::Waiter::Thread(Some(thread));
            self.unit_request(role, op, waiter, None)?;
            return Ok(Output::Waits);
        }
        Ok(Output::Done)
    }

    /// The class of a page of a frame (G2): device memory is of class
    /// device, a kept frame or range of class global read-only, a table
    /// page of its class, every other page of ordinary memory data. A leaf
    /// smaller than a chunk reads the class byte of each page it covers; a
    /// leaf that covers whole chunks reads each chunk's count of pages not
    /// of class *data* in its gate word, one word per chunk (G3, the
    /// bound), and is refused if one is not 0.
    pub(crate) fn class_of(&self, frame: crate::Frame, page: u64) -> dodos_abi::PageClass {
        use dodos_abi::PageClass;
        if frame.memory != dodos_abi::MemoryType::WriteBack {
            return PageClass::DEVICE;
        }
        let bytes = self.bytes(frame.page);
        let (start, end) = (frame.base + page * bytes, frame.base + (page + 1) * bytes);
        if frame.kept || self.table.kept.iter().any(|&(s, e)| s <= start && end <= e) {
            return PageClass::GLOBAL_READ_ONLY;
        }
        let small = self.page();
        let classes = &self.table.classes;
        let shift = self.table.geometry.chunk_shift();
        if bytes >= 1 << shift {
            let chunks = start >> shift..end >> shift;
            let counted = chunks
                .into_iter()
                .any(|c| self.table.gate_word(c).not_data() != 0);
            if !counted {
                return PageClass::DATA;
            }
        }
        match classes.range(start / small..end.div_ceil(small)).next() {
            Some((_, &class)) => class,
            None => PageClass::DATA,
        }
    }

    /// The next step of a partial mapping: `more` pages written after those
    /// it holds, their leaves counted.
    fn extend_mapping(&mut self, cap: CapId, more: u64, partial: bool) {
        let Record::Mapping(mut mapping) = self.caps[&cap].record else {
            unreachable!("a partial mapping")
        };
        let frame = self.frames[&self.caps[&cap].object];
        let bytes = self.bytes(frame.page);
        let aspace = live(&mut self.aspaces, mapping.space);
        for page in mapping.first + mapping.count..mapping.first + mapping.count + more {
            let at = mapping.address + (page - mapping.first) * bytes;
            let translation = Translation {
                cap,
                page,
                size: frame.page,
                rights: mapping.rights,
            };
            aspace.translations.insert(at, translation);
            self.table.gated.0 = true;
            self.added.0 += 1;
        }
        let added = Mapping {
            first: mapping.first + mapping.count,
            count: more,
            ..mapping
        };
        mapping.count += more;
        mapping.partial = partial;
        live(&mut self.caps, cap).record = Record::Mapping(mapping);
        self.count_leaves(cap, frame, added, 1);
        self.rehook(mapping.space);
    }

    /// `aspace_unmap` and `iospace_unmap`: no right on the capability
    /// itself, as for `cap_delete`. Succeeds without effect if the record
    /// is empty; `busy` if it names a thread or a space of the other kind.
    /// A record that names a dying space is unmapped as usual. The tables
    /// left empty go to the free list.
    pub(crate) fn unmap(&mut self, thread: ObjId, frame: SlotAddr, kind: ObjectKind) -> Outcome {
        let cap = self.cap(thread, frame, Rights::READ)?;
        self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        match self.caps[&cap].record {
            Record::Empty => Ok(Output::Done),
            Record::Binding(_) | Record::Unmapping(_) => Err(Error::Busy),
            Record::Mapping(mapping) if self.objects[&mapping.space].kind != kind => {
                Err(Error::Busy)
            }
            // The unit of an iospace that serves devices has failed: `busy`,
            // and the record is emptied with its counts kept.
            Record::Mapping(mapping)
                if self.unit_of_space(mapping.space).is_some_and(|iommu| {
                    self.unit_failed(self.unit_role(self.iommus[&iommu].unit))
                }) =>
            {
                self.clear_mapping(cap);
                self.freeze_unmapping(cap);
                Err(Error::Busy)
            }
            Record::Mapping(_) => match self.begin_unmap(cap, Some(thread))? {
                Unmapped::Done => Ok(Output::Done),
                Unmapped::Waits => Ok(Output::Waits),
            },
        }
    }

    /// `aspace_protect`: replaces the rights of the whole mapping.
    pub(crate) fn aspace_protect(
        &mut self,
        thread: ObjId,
        aspace: SlotAddr,
        frame: SlotAddr,
        rights: u32,
    ) -> Outcome {
        let space = self.object(thread, aspace, ObjectKind::ASpace, Rights::WRITE)?;
        let cap = self.cap(thread, frame, Rights::READ)?;
        self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        let rights = mapping_rights(rights)?;
        if !self.caps[&cap].rights.contains(rights) {
            return Err(Error::Denied);
        }
        let Record::Mapping(mut mapping) = self.caps[&cap].record else {
            return Err(Error::Busy);
        };
        if mapping.space != space || mapping.partial {
            return Err(Error::Busy);
        }
        let reduced = !rights.contains(mapping.rights);
        // G3: the class reads of a protect follow the bound: a mapping that
        // touches more chunks than one call of the gate checks is `busy`;
        // then the class of each page must allow the new rights (`denied`).
        let frame = self.frames[&self.caps[&cap].object];
        let device = frame.memory != dodos_abi::MemoryType::WriteBack;
        let touched = match device {
            true => mapping.count,
            false => self.leaf_chunks(frame, mapping.first, mapping.count).len() as u64,
        };
        if touched > self.machine.gate_chunks {
            return Err(Error::Busy);
        }
        let leaf = dodos_abi::Leaf {
            write: rights.contains(Rights::WRITE),
            exec: rights.contains(Rights::EXEC),
            device_type: device,
        };
        let pages = mapping.first..mapping.first + mapping.count;
        if pages
            .clone()
            .any(|page| !self.class_of(frame, page).permits(leaf))
        {
            return Err(Error::Denied);
        }
        // A reduction is invalidated on the cores that may hold it, the
        // caller waiting for its core's request; `again` with nothing
        // changed if that request is in use. Asking for the rights the
        // mapping already has changes nothing.
        if mapping.rights == rights {
            return Ok(Output::Done);
        }
        if reduced {
            self.request_free()?;
        }
        mapping.rights = rights;
        live(&mut self.caps, cap).record = Record::Mapping(mapping);
        let aspace = live(&mut self.aspaces, space);
        let mut addresses = Vec::new();
        for (&at, translation) in aspace.translations.iter_mut().filter(|(_, t)| t.cap == cap) {
            translation.rights = rights;
            addresses.push(at);
        }
        if !reduced {
            return Ok(Output::Done);
        }
        let flush = crate::cores::Flush::Range { space, addresses };
        match self.start_request(flush, crate::cores::Completes::Protect, Some(thread)) {
            crate::cores::Started::Asked => Ok(Output::Waits),
            _ => Ok(Output::Done),
        }
    }

    /// `iospace_attach`: attaches a device of the unit to an iospace, or
    /// detaches it with iospace 0.
    pub(crate) fn iospace_attach(
        &mut self,
        thread: ObjId,
        iommu: SlotAddr,
        iospace: SlotAddr,
        device: u64,
    ) -> Outcome {
        let iommu = self.object(thread, iommu, ObjectKind::Iommu, Rights::MANAGE)?;
        let iospace = self.optional(thread, iospace, ObjectKind::IoSpace, Rights::MANAGE)?;
        // The device: bus, device and function in 16 bits; higher bits set
        // are `invalid`.
        let device = u16::try_from(device).or(Err(Error::Invalid))?;
        let unit = &self.iommus[&iommu];
        let attached = unit.attached.get(&device).copied();
        let Some(iospace) = iospace else {
            // With iospace 0, the state in this order (D-139): the unit has
            // failed (`busy`); an attach of this node in flight, the attach
            // word *pinned* (`busy`); a device of another node (`denied`); a
            // device in no header of this node (no effect); a detach in
            // flight, its entry *detaching* (`busy`). The device leaves the
            // header at the answer, done or *failed* (the attach word against
            // every event).
            let key = self.identifier_key(unit.unit, device);
            let role = self.unit_role(unit.unit);
            if self.unit_failed(role) {
                return Err(Error::Busy);
            }
            let pinned =
                self.table.attach_word(key).state() == Some(dodos_abi::AttachState::Pinned);
            // *Pinned* is `busy` for every caller, before the owner's
            // `denied` (the order of iospace 0, D-139).
            if pinned {
                return Err(Error::Busy);
            }
            let Some(from) = attached else {
                return match self.holds_identifier(key) {
                    true => Ok(Output::Done),
                    false => Err(Error::Denied),
                };
            };
            if unit.detaching.contains(&device) {
                return Err(Error::Busy);
            }
            if self.unit_requests.iter().all(Option::is_some) {
                return Err(Error::Again);
            }
            let domain = self.domains.get(&from).copied().unwrap_or(0);
            let root = self.aspaces[&from].root.unwrap_or(0);
            live(&mut self.iommus, iommu).detaching.insert(device);
            let op = crate::rings::UnitOp::Detach {
                device,
                domain,
                root,
            };
            let waiter = crate::units::Waiter::Thread(Some(thread));
            self.unit_request(role, op, waiter, None)?;
            return Ok(Output::Waits);
        };
        // The state, in this order, "the unit has failed (`busy`" first
        // (IOMMU, rules of `iospace_attach`, gap 11).
        let role = self.unit_role(unit.unit);
        if self.unit_failed(role) {
            return Err(Error::Busy);
        }
        if self.machine.kernel_devices.contains(&(unit.unit, device)) {
            return Err(Error::Busy);
        }
        // A device identifier has an owner: its word must be free for this
        // node to take, or already this node's (`denied`).
        let key = self.identifier_key(unit.unit, device);
        if !self.holds_identifier(key) {
            return Err(Error::Denied);
        }
        // Already attached to this iospace with no request in flight:
        // success; otherwise an attach word not 0 is `busy` (an attach or a
        // detach in flight, or another iospace: D-139).
        let root = self.aspaces[&iospace].root.unwrap_or(0);
        let word = self.table.attach_word(key);
        let here = word.state() == Some(dodos_abi::AttachState::Attached) && word.root() == root;
        if attached == Some(iospace) && here && !unit.detaching.contains(&device) {
            return Ok(Output::Done);
        }
        if attached.is_some() || word != dodos_abi::AttachWord::NONE {
            return Err(Error::Busy);
        }
        let served = &self.iospaces[&iospace];
        let other_unit = served.first().is_some_and(|&(other, _)| other != iommu)
            || self.attaching(root).any(|(r, _)| r != role);
        if other_unit {
            return Err(Error::Busy);
        }
        if usize::from(self.table.attached_to(root)) >= IOSPACE_MAX_DEVICES {
            return Err(Error::Busy);
        }
        // A domain number for the iospace on that unit, taken from this
        // node slot's range at its first device and freed with its last
        // (Devices and shared hardware, domain numbers, D-134).
        let domain = match self.domains.get(&iospace) {
            Some(&domain) => domain,
            None => self.free_domain(unit.unit).ok_or(Error::Busy)?,
        };
        // A call that finds no free entry of the unit request table returns
        // `again` with nothing changed.
        if self.unit_requests.iter().all(Option::is_some) {
            return Err(Error::Again);
        }
        let taken = dodos_abi::Ownership::taken(
            dodos_abi::Owner::Node(self.node),
            self.table.generation(self.node),
        );
        // In one gate call: the identifier word taken, the attach word
        // pinned with the iospace's root, the root's count of devices
        // raised (D-137).
        self.table.write_identifier(self.node, key, Some(taken));
        let pinned = dodos_abi::AttachWord::new(dodos_abi::AttachState::Pinned, root);
        let pinned = pinned.expect("a root aligned on a page");
        self.table
            .swap_attach(key, dodos_abi::AttachWord::NONE, pinned);
        self.set_root_class(root);
        self.domains.insert(iospace, domain);
        let unit = self.iommus[&iommu].unit;
        live(&mut self.aspaces, iospace).units.insert(unit);
        // The context entry, written by the holder of the unit's role: a
        // unit request, which the thread waits for. The device enters the
        // iospace's header only at its answer done (D-139).
        let op = crate::rings::UnitOp::Attach {
            device,
            domain,
            root,
        };
        self.unit_request(role, op, crate::units::Waiter::Thread(Some(thread)), None)?;
        Ok(Output::Waits)
    }

    /// IOMMU, in a removal step: the context entry of the device is
    /// cleared by a unit request no thread waits for; at its answer
    /// (`detach_answered`) the device leaves the header of its iospace and
    /// its identifier word becomes free (D-139). On a failed unit the
    /// device leaves the header at once, its words as they are. `Again`
    /// with nothing changed when no entry of the unit request table is
    /// free.
    pub(crate) fn detach(&mut self, iommu: ObjId, device: u16) -> Result<(), Error> {
        let role = self.unit_role(self.iommus[&iommu].unit);
        // One request at a time per device: a detach in flight is waited
        // for, not sent again (D-139).
        if self.iommus[&iommu].detaching.contains(&device) {
            return Err(Error::Again);
        }
        // On a failed unit the answer would be *failed*: the device leaves
        // the header, its attach word and identifier word as they are.
        if self.unit_failed(role) {
            self.detach_local(iommu, device);
            return Ok(());
        }
        if self.unit_requests.iter().all(Option::is_some) {
            return Err(Error::Again);
        }
        let iospace = self.iommus[&iommu].attached[&device];
        let domain = self.domains.get(&iospace).copied().unwrap_or(0);
        let root = self.aspaces[&iospace].root.unwrap_or(0);
        live(&mut self.iommus, iommu).detaching.insert(device);
        let op = crate::rings::UnitOp::Detach {
            device,
            domain,
            root,
        };
        self.unit_request(role, op, crate::units::Waiter::Step, None)?;
        Ok(())
    }

    /// The attaches in flight to the iospace whose root is `root`: the role
    /// and device of each entry of the unit request table that asks one.
    pub(crate) fn attaching(&self, root: u64) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.unit_requests
            .iter()
            .flatten()
            .filter_map(move |e| match e.op {
                crate::rings::UnitOp::Attach {
                    device, root: r, ..
                } if r == root => Some((e.role, device)),
                _ => None,
            })
    }

    /// The iommu object of the unit a role governs.
    fn iommu_of(&self, role: usize) -> Option<ObjId> {
        let record = self.table.role_records[role];
        (record.governs == dodos_abi::Governs::IommuUnit).then_some(())?;
        let unit = usize::from(record.object);
        self.iommus
            .iter()
            .find(|(_, u)| u.unit == unit)
            .map(|(&id, _)| id)
    }

    /// At the answer of an attach (the attach word against every event,
    /// D-139): done with the word *attached* with its root, the device
    /// enters its iospace's header; otherwise (done on *pinned*, the holder
    /// having refused it, or *failed*) *pinned* goes back to 0 with the
    /// root's count and the identifier word, and *attached* stays. Returns
    /// whether the device entered the header.
    pub(crate) fn attach_answered(
        &mut self,
        role: usize,
        device: u16,
        root: u64,
        result: dodos_abi::UnitResult,
    ) -> bool {
        let (Some(key), Some(iommu)) = (self.attach_key(role, device), self.iommu_of(role)) else {
            return false;
        };
        let iospace = self
            .iospaces
            .keys()
            .copied()
            .find(|id| self.aspaces[id].root == Some(root));
        let word = self.table.attach_word(key);
        let attached =
            word.state() == Some(dodos_abi::AttachState::Attached) && word.root() == root;
        if result == dodos_abi::UnitResult::Done && attached {
            if let Some(iospace) = iospace {
                live(&mut self.iospaces, iospace).push((iommu, device));
                let unit = live(&mut self.iommus, iommu);
                unit.attached.insert(device, iospace);
                unit.order.push(device);
            }
            return true;
        }
        let pinned = word.state() == Some(dodos_abi::AttachState::Pinned) && word.root() == root;
        if pinned
            && self
                .table
                .swap_attach(key, word, dodos_abi::AttachWord::NONE)
        {
            self.table.write_identifier(self.node, key, None);
            self.set_root_class(root);
            if let Some(iospace) = iospace
                && self.iospaces[&iospace].is_empty()
                && self.attaching(root).next().is_none()
            {
                self.domains.remove(&iospace);
            }
        }
        false
    }

    /// The class of an iospace's root: 48 + the devices whose attach word
    /// names it (G2, D-137), written by the owner's gate.
    pub(crate) fn set_root_class(&mut self, root: u64) {
        let devices = self.table.attached_to(root);
        let class = dodos_abi::PageClass::iospace_root(devices).expect("at most 16 devices");
        self.table
            .set_class(root / self.machine.geometry.page(), Some(class));
    }

    /// At the answer of a detach, done or *failed* (the attach word against
    /// every event, D-139): the owner's gate writes the attach word 0 only
    /// from *detached* with the root of the iospace the device's entry
    /// names, lowers the root's count and writes the identifier word
    /// *free*, unclaimed (D-137); the device leaves the header, unless a
    /// done finds the word still *attached* (the holder refused the
    /// request), which only clears its *detaching* mark. Returns whether
    /// the device was detached.
    pub(crate) fn detach_answered(
        &mut self,
        role: usize,
        device: u16,
        result: dodos_abi::UnitResult,
        waiter: crate::units::Waiter,
    ) -> bool {
        let (Some(key), Some(iommu)) = (self.attach_key(role, device), self.iommu_of(role)) else {
            return false;
        };
        live(&mut self.iommus, iommu).detaching.remove(&device);
        let Some(&iospace) = self.iommus[&iommu].attached.get(&device) else {
            return false;
        };
        let root = self.aspaces[&iospace].root.unwrap_or(0);
        let word = self.table.attach_word(key);
        let detached =
            word.state() == Some(dodos_abi::AttachState::Detached) && word.root() == root;
        if detached
            && self
                .table
                .swap_attach(key, word, dodos_abi::AttachWord::NONE)
        {
            self.table.write_identifier(self.node, key, None);
            self.set_root_class(root);
        }
        // A detach the holder refused (done on a word still *attached*):
        // at a call the device stays in the header; at a step of a
        // destruction it leaves it as at *failed*, so no step asks again
        // for ever (the attach word against every event, D-140).
        let refused = result == dodos_abi::UnitResult::Done
            && word.state() == Some(dodos_abi::AttachState::Attached)
            && waiter != crate::units::Waiter::Step;
        if !refused {
            self.detach_local(iommu, device);
        }
        detached
    }

    /// The identifier word of the device of the unit that a role governs.
    pub(crate) fn attach_key(&self, role: usize, device: u16) -> Option<u32> {
        let record = self.table.role_records[role];
        (record.governs == dodos_abi::Governs::IommuUnit)
            .then(|| self.identifier_key(usize::from(record.object), device))
    }

    /// The identifier word of a device of an IOMMU unit: its segment and its
    /// identifier.
    fn identifier_key(&self, unit: usize, device: u16) -> u32 {
        u32::from(self.machine.iommus[unit].segment) << 16 | u32::from(device)
    }

    /// Whether this node may attach the device whose identifier word is
    /// `key`: the word is free for it to take (rules of taking), or taken
    /// by its current generation.
    fn holds_identifier(&self, key: u32) -> bool {
        let raw = self.table.identifiers.get(&key).copied();
        let raw = raw.unwrap_or(dodos_abi::Ownership::free(dodos_abi::Owner::Unclaimed).raw());
        let Some(word) = dodos_abi::Ownership::from_raw(raw) else {
            return false;
        };
        let mine = word.state() == dodos_abi::OwnState::Taken
            && word.owner() == dodos_abi::Owner::Node(self.node)
            && word.generation() == self.table.generation(self.node);
        mine || self.table.may_take(word, self.node)
    }

    /// The lowest domain number of this node slot's range on a unit that
    /// none of its iospaces holds, if any is left.
    fn free_domain(&self, unit: usize) -> Option<u16> {
        let lost = self
            .lost_domains
            .iter()
            .filter(|&&(u, _)| u == unit)
            .map(|&(_, domain)| domain);
        let used: BTreeSet<u16> = self
            .domains
            .iter()
            .filter(|(space, _)| self.aspaces[space].units.contains(&unit))
            .map(|(_, &domain)| domain)
            .chain(lost)
            .collect();
        self.domain_range(unit, self.node)
            .find(|domain| !used.contains(domain))
    }

    /// The node's own record of a detach: the device leaves the unit's
    /// order and the iospace's header; the iospace keeps the unit and its
    /// domain number while the device's word still reads *attached*
    /// (D-141), and frees the domain number with its last device otherwise.
    /// The words are written by the caller (`detach_answered`), not here.
    fn detach_local(&mut self, iommu: ObjId, device: u16) {
        let unit = live(&mut self.iommus, iommu);
        unit.order.retain(|&other| other != device);
        unit.detaching.remove(&device);
        let iospace = unit.attached.remove(&device);
        let index = unit.unit;
        let iospace = iospace.expect("the device is attached");
        live(&mut self.iospaces, iospace).retain(|&found| found != (iommu, device));
        // A device that leaves with its word still *attached* may still
        // reach the iospace's tables: the iospace keeps the unit and its
        // domain number (D-141).
        let key = self.identifier_key(index, device);
        if self.table.attach_word(key).state() == Some(dodos_abi::AttachState::Attached) {
            self.kept.insert(iospace, iommu);
        }
        if self.iospaces[&iospace].is_empty() && !self.kept.contains_key(&iospace) {
            self.domains.remove(&iospace);
        }
    }

    /// The unit through which an unmap in a space is invalidated: the unit
    /// of the devices its header lists, or the unit it keeps for a device
    /// that left with its word *attached*; none for an aspace.
    pub(crate) fn unit_of_space(&self, space: ObjId) -> Option<ObjId> {
        let served = self.iospaces.get(&space)?;
        served
            .first()
            .map(|&(iommu, _)| iommu)
            .or_else(|| self.kept.get(&space).copied())
    }
}
