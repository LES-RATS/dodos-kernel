//! Memory and devices: mapping records, address spaces and their tables.

use crate::caps::Clearing;
use crate::{
    ASpace, CapId, Map, Mapping, Model, ObjId, Outcome, Output, Record, Translation, live,
};
use dodos_abi::{
    Error, IOSPACE_MAX_DEVICES, ObjectKind, PageSize, Rights, SlotAddr, TABLE_BYTES,
    io_mapping_rights, mapping_range, mapping_rights,
};
use std::collections::BTreeSet;

/// The user range and the reserved slot: on x86-64 with four levels of
/// tables, the user range runs from 0 to 255 × 512 GB.
///
/// CONTRACT-GAP (Values to add to `abi/`): the end of the user range is not
/// among the values of `abi/`, although user space and the model need it.
pub const USER_END: u64 = 255 << 39;

/// Address spaces and their tables: the lower tables hooked in a space of
/// `levels` levels (4 for an `aspace`; 4 or 3 for an `iospace`). A hooked
/// table always has a present entry, so the hooked tables are those on the
/// paths of the translations: a table of the second level per 512 GB that
/// holds one (with four levels), of the third per 1 GB that holds a page
/// smaller than 1 GB, of the last per 2 MB that holds a 4 KB page.
pub(crate) fn hooked(space: &ASpace, levels: u32) -> u64 {
    tables(
        space.translations.iter().map(|(&at, t)| (at, t.size)),
        levels,
    )
    .len() as u64
}

/// The lower tables on the paths of pages, each named by its level (3 for
/// the one under the root of four levels, 1 for the last) and the address
/// bits above what it covers.
pub(crate) fn tables(
    pages: impl Iterator<Item = (u64, PageSize)>,
    levels: u32,
) -> BTreeSet<(u32, u64)> {
    let mut found = BTreeSet::new();
    for (at, size) in pages {
        if levels == 4 {
            found.insert((3, at >> 39));
        }
        if size != PageSize::Size1G {
            found.insert((2, at >> 30));
        }
        if size == PageSize::Size4K {
            found.insert((1, at >> 21));
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
        let aspace = live(&mut self.aspaces, space);
        for page in mapping.first..mapping.first + mapping.count {
            let at = mapping.address + (page - mapping.first) * size.bytes();
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
    }
}

/// IOMMU: the identifiers of the devices of one unit, 16 bits on x86-64.
pub(crate) const DEVICE_IDS: u32 = 1 << 16;

impl Model {
    /// The levels of the tables of a space: four for an `aspace`; for an
    /// `iospace`, four with an address width of 48 bits, otherwise three.
    fn levels(&self, space: ObjId) -> u32 {
        match self.iospaces.contains_key(&space) && self.machine.io_width < 48 {
            true => 3,
            false => 4,
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
        let cap = self.cap(thread, map.frame, Rights::READ)?;
        self.is(cap, ObjectKind::Frame, Rights::NONE)?;
        let untyped = self.optional(thread, map.untyped, ObjectKind::Untyped, Rights::WRITE)?;
        if untyped.is_some() && untyped != self.objects[&space].from {
            return Err(Error::Invalid);
        }
        // 3. The plain arguments.
        let frame = self.frames[&self.caps[&cap].object];
        let (limit, rights) = match kind {
            ObjectKind::ASpace => (USER_END, mapping_rights(map.rights)),
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
            frame.page,
            limit,
        )?;
        let rights = rights?;
        if !self.caps[&cap].rights.contains(rights) {
            return Err(Error::Denied);
        }
        // 4. The state: the record is full (`busy`, even if it names this
        // space); some address of the range is translated (`busy`); the
        // tables needed are more than the free list and the untyped supply
        // (`no_memory`), and nothing is taken.
        if self.caps[&cap].record != Record::Empty {
            return Err(Error::Busy);
        }
        let size = frame.page.bytes();
        let end = map.address + map.count * size;
        let aspace = &self.aspaces[&space];
        let before = aspace.translations.range(..map.address).next_back();
        let covers = before.is_some_and(|(&at, t)| at + t.size.bytes() > map.address);
        if covers || aspace.translations.range(map.address..end).next().is_some() {
            return Err(Error::Busy);
        }
        let levels = self.levels(space);
        let held: Vec<(u64, PageSize)> = aspace
            .translations
            .iter()
            .map(|(&at, t)| (at, t.size))
            .collect();
        let hooked = tables(held.iter().copied(), levels);
        let pages = (0..map.count).map(|n| (map.address + n * size, frame.page));
        let wanted = tables(held.into_iter().chain(pages), levels).len() as u64;
        let free = aspace.tables - hooked.len() as u64;
        let missing = (wanted - hooked.len() as u64).saturating_sub(free);
        let supplied = untyped.map_or(0, |untyped| {
            let untyped = &self.untypeds[&untyped];
            let start = untyped.watermark.next_multiple_of(TABLE_BYTES as u64);
            untyped.bytes.saturating_sub(start) / TABLE_BYTES as u64
        });
        if missing > supplied {
            return Err(Error::NoMemory);
        }
        // Effect: the missing tables are taken, free list first, then the
        // untyped, cleared within the call; the record is filled and the
        // capability goes to the head of the space's list.
        for _ in 0..missing {
            let untyped = untyped.expect("an untyped supplies the tables");
            self.take(
                untyped,
                TABLE_BYTES as u64,
                TABLE_BYTES as u64,
                Clearing::WithinCall,
            )
            .expect("checked above");
        }
        live(&mut self.aspaces, space).tables += missing;
        let mapping = Mapping {
            space,
            address: map.address,
            first: map.first,
            count: map.count,
            rights,
        };
        self.record_mapping(cap, space, mapping);
        Ok(Output::Done)
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
        if mapping.space != space {
            return Err(Error::Busy);
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
            // With iospace 0: detached if attached, otherwise no effect.
            if attached.is_some() {
                self.detach(iommu, device);
            }
            return Ok(Output::Done);
        };
        // The state, in this order.
        if self.machine.kernel_devices.contains(&(unit.unit, device)) {
            return Err(Error::Busy);
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
        live(&mut self.iospaces, iospace).push((iommu, device));
        live(&mut self.iommus, iommu)
            .attached
            .insert(device, iospace);
        Ok(Output::Done)
    }

    /// IOMMU: the context entry of a device is cleared, and the device
    /// leaves the header of its iospace.
    pub(crate) fn detach(&mut self, iommu: ObjId, device: u16) {
        let iospace = live(&mut self.iommus, iommu).attached.remove(&device);
        let iospace = iospace.expect("the device is attached");
        live(&mut self.iospaces, iospace).retain(|&found| found != (iommu, device));
    }
}
