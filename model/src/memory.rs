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
        }
        aspace.records.push(cap);
        live(&mut self.caps, cap).record = Record::Mapping(mapping);
        self.count_leaves(cap, frame, mapping, 1);
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

    /// G4: adds or takes the leaves of a mapping to the map counts; the
    /// leaves of a frame under a grant count in its use entry (G6).
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
        for chunk in self.leaf_chunks(frame, mapping.first, mapping.count) {
            let Some(count) =
                std::sync::Arc::make_mut(&mut self.table.maps).get_mut(chunk as usize)
            else {
                continue;
            };
            *count = count
                .checked_add_signed(sign)
                .expect("a map count never goes below 0");
        }
    }

    /// Mapping records: empties the record of a frame capability that holds
    /// a mapping, and removes its translations and its place in the list.
    pub(crate) fn empty_mapping(&mut self, cap: CapId) {
        let Record::Mapping(mapping) = self.caps[&cap].record else {
            return;
        };
        let aspace = live(&mut self.aspaces, mapping.space);
        aspace.translations.retain(|_, t| t.cap != cap);
        aspace.records.retain(|&other| other != cap);
        live(&mut self.caps, cap).record = Record::Empty;
        let frame = self.frames[&self.caps[&cap].object];
        self.count_leaves(cap, frame, mapping, -1);
    }
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
        // then that it is the space's own untyped (`invalid`; any untyped
        // is `invalid` for the start-up aspace, made from none).
        // Memory under a grant: the gate maps a frame of another node only
        // while the grant is live (an invalid remote capability is `gone`,
        // judged with its slot).
        let cap = self.cap(thread, map.frame, Rights::READ)?;
        self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        let untyped = self.optional(thread, map.untyped, ObjectKind::Untyped, Rights::WRITE)?;
        if untyped.is_some() && untyped != self.objects[&space].from {
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
        let done = match self.caps[&cap].record {
            Record::Empty => 0,
            Record::Mapping(m)
                if m.partial
                    && m.space == space
                    && m.address == map.address
                    && m.first == map.first
                    && m.rights == rights
                    && m.count < map.count =>
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
        let cost = |model: &Self, page: u64| match device {
            true => 1,
            false => model.leaf_chunks(frame, page, 1).len() as u64,
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
        // G3 (3) and (4): the class of every page the step's leaves target.
        let leaf = dodos_abi::Leaf {
            write: rights.contains(Rights::WRITE),
            exec: rights.contains(Rights::EXEC),
            device_type: device,
        };
        if (first..first + step).any(|page| !self.class_of(frame, page).permits(leaf)) {
            return Err(Error::Denied);
        }
        // The gate over page tables, G4: a map count that would pass its
        // maximum is `busy`.
        let mut added: std::collections::BTreeMap<u64, u64> = Default::default();
        for chunk in self.leaf_chunks(frame, first, step) {
            *added.entry(chunk).or_default() += 1;
        }
        let remote = self.remotes.contains_key(&self.caps[&cap].object);
        let overflow = !remote
            && added.iter().any(|(&chunk, &leaves)| {
                let count = self.table.maps.get(chunk as usize).copied().unwrap_or(0);
                count + leaves > self.machine.map_count_max
            });
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
        let free = aspace.tables - hooked.len() as u64;
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
        // The barrier's dependency: bit X of `maps_of` is set through the
        // gate before the first translation to memory of node X.
        if let Some(remote) = self.remotes.get(&self.caps[&cap].object) {
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
            // G2: the gate turns a page of class data into a table page.
            let at = self.untypeds[&untyped].base + taken.at;
            std::sync::Arc::make_mut(&mut self.table.classes)
                .insert(at / table, crate::table::PageClass::Table);
            live(&mut self.aspaces, space).table_pages.push(at);
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
            };
            self.record_mapping(cap, space, mapping);
        }
        match partial {
            true => Err(Error::Again),
            false => Ok(Output::Done),
        }
    }

    /// The class of a page of a frame (G2): device memory is of class
    /// device, a kept frame or range of class global read-only, a table
    /// page of its class, every other page of ordinary memory data. For a
    /// leaf larger than a page, the first class of a page it covers that is
    /// not data, as the gate reads it from the count of each chunk's gate
    /// word.
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
        match self
            .table
            .classes
            .range(start / small..end.div_ceil(small))
            .next()
        {
            Some(_) => PageClass::table(1).expect("level 1"),
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
            Record::Empty => return Ok(Output::Done),
            Record::Binding(_) => return Err(Error::Busy),
            Record::Mapping(mapping) if self.objects[&mapping.space].kind != kind => {
                return Err(Error::Busy);
            }
            Record::Mapping(_) => self.empty_mapping(cap),
        }
        Ok(Output::Done)
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
        mapping.rights = rights;
        live(&mut self.caps, cap).record = Record::Mapping(mapping);
        let aspace = live(&mut self.aspaces, space);
        for translation in aspace.translations.values_mut().filter(|t| t.cap == cap) {
            translation.rights = rights;
        }
        Ok(Output::Done)
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
            // With iospace 0: detached if attached, otherwise no effect. The
            // context entry is cleared by the holder of the unit's role. A
            // device attached by another live node is `denied`.
            let key = self.identifier_key(unit.unit, device);
            if attached.is_none() && !self.holds_identifier(key) {
                return Err(Error::Denied);
            }
            if attached.is_some() {
                let unit = self.iommus[&iommu].unit;
                self.detach_local(iommu, device);
                return self.unit_request(Some(thread), unit, crate::roles::UnitOp::Detach(device));
            }
            return Ok(Output::Done);
        };
        // The state, in this order.
        if self.machine.kernel_devices.contains(&(unit.unit, device)) {
            return Err(Error::Busy);
        }
        // A device identifier has an owner: its word must be free for this
        // node to take, or already this node's (`denied`).
        let key = self.identifier_key(unit.unit, device);
        if !self.holds_identifier(key) {
            return Err(Error::Denied);
        }
        match attached {
            Some(other) if other != iospace => return Err(Error::Busy),
            Some(_) => return Ok(Output::Done),
            None => {}
        }
        let served = &self.iospaces[&iospace];
        if served.first().is_some_and(|&(other, _)| other != iommu) {
            return Err(Error::Busy);
        }
        if served.len() >= IOSPACE_MAX_DEVICES {
            return Err(Error::Busy);
        }
        // A domain number for the iospace on that unit, given at its first
        // device and freed with its last.
        let domains = unit.attached.values().collect::<BTreeSet<_>>().len() as u32;
        let limit = self.machine.iommus[unit.unit].domains;
        if served.is_empty() && domains >= limit {
            return Err(Error::Busy);
        }
        let taken = dodos_abi::Ownership::taken(
            dodos_abi::Owner::Node(self.node),
            self.table.generation(self.node),
        );
        self.table.write_identifier(self.node, key, Some(taken));
        live(&mut self.iospaces, iospace).push((iommu, device));
        let unit = live(&mut self.iommus, iommu);
        unit.attached.insert(device, iospace);
        unit.order.push(device);
        let unit = unit.unit;
        // The node's report, and the context entry, written by the holder
        // of the unit's role.
        self.table.reports[self.node as usize].push((unit, device));
        self.unit_request(Some(thread), unit, crate::roles::UnitOp::Attach(device))
    }

    /// IOMMU: the context entry of a device is cleared, and the device
    /// leaves the header of its iospace.
    pub(crate) fn detach(&mut self, iommu: ObjId, device: u16) {
        let unit = self.iommus[&iommu].unit;
        self.detach_local(iommu, device);
        let _ = self.unit_request(None, unit, crate::roles::UnitOp::Detach(device));
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

    /// The node's own record of a detach, and its report; the identifier
    /// word becomes free and unclaimed.
    fn detach_local(&mut self, iommu: ObjId, device: u16) {
        let index = self.iommus[&iommu].unit;
        let key = self.identifier_key(index, device);
        self.table.write_identifier(self.node, key, None);
        self.table.reports[self.node as usize].retain(|&r| r != (index, device));
        let unit = live(&mut self.iommus, iommu);
        unit.order.retain(|&other| other != device);
        let iospace = unit.attached.remove(&device);
        let iospace = iospace.expect("the device is attached");
        live(&mut self.iospaces, iospace).retain(|&found| found != (iommu, device));
    }
}
