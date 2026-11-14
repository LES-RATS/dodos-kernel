//! Memory and devices: mapping records, address spaces and their tables.

use crate::{ASpace, CapId, Mapping, Model, ObjId, Record, Translation, live};
use dodos_abi::PageSize;
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
