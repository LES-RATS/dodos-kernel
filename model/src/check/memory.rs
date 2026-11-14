//! The invariants of mapping records, address spaces and their tables.

use super::{Check, ensure};
use crate::{Mapping, Model, Record};
use dodos_abi::PageSize;
use std::collections::{BTreeMap, BTreeSet};

impl Model {
    /// Mapping records:
    /// 35. every full mapping record names exactly the translations it
    ///     made, and every translation belongs to exactly one record: the
    ///     translations of a space are those of the records of its list,
    ///     each page at its address, with the record's rights and the page
    ///     size of the frame; a record maps 1 to 512 pages that are in its
    ///     frame, inside the user range, and no two translations overlap;
    /// 36. a space lists, once each, exactly the capabilities whose records
    ///     name it;
    /// 37. the tables a space holds are at least those its translations
    ///     hook: one per 512 GB that holds a page, one per 1 GB that holds a
    ///     page smaller than 1 GB, one per 2 MB that holds a 4 KB page.
    pub(super) fn check_spaces(&self) -> Check {
        let mut named: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (&id, cap) in &self.caps {
            if let Record::Mapping(mapping) = cap.record {
                named.entry(mapping.space).or_default().push(id);
            }
        }
        for (&id, space) in &self.aspaces {
            let mut listed = space.records.clone();
            listed.sort();
            let mut wanted = named.remove(&id).unwrap_or_default();
            wanted.sort();
            ensure(listed == wanted, "36: list of the records of a space")?;
            let mut expected = BTreeMap::new();
            for cap in &space.records {
                let held = &self.caps[cap];
                let Record::Mapping(Mapping {
                    address,
                    first,
                    count,
                    rights,
                    ..
                }) = held.record
                else {
                    continue;
                };
                let frame = self
                    .frames
                    .get(&held.object)
                    .ok_or("35: record of a frame")?;
                let size = frame.page.bytes();
                let inside = count >= 1
                    && first
                        .checked_add(count)
                        .is_some_and(|end| end <= frame.pages);
                let end = (count as u128) * (size as u128) + address as u128;
                let below = end <= crate::memory::USER_END as u128 && address.is_multiple_of(size);
                ensure(inside && below, "35: pages of a record")?;
                for n in 0..count {
                    let at = address + n * size;
                    expected.insert(at, (*cap, first + n, frame.page, rights));
                }
            }
            let actual: BTreeMap<_, _> = space
                .translations
                .iter()
                .map(|(&at, t)| (at, (t.cap, t.page, t.size, t.rights)))
                .collect();
            ensure(actual == expected, "35: translations of the records")?;
            let mut end = 0;
            for (&at, t) in &space.translations {
                ensure(at >= end, "35: translations that overlap")?;
                end = at + t.size.bytes();
            }
            let mut per = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
            for (&at, t) in &space.translations {
                per[0].insert(at >> 39);
                if t.size != PageSize::Size1G {
                    per[1].insert(at >> 30);
                }
                if t.size == PageSize::Size4K {
                    per[2].insert(at >> 21);
                }
            }
            let hooked: usize = per.iter().map(BTreeSet::len).sum();
            ensure(space.tables >= hooked as u64, "37: tables of a space")?;
        }
        ensure(
            named.is_empty(),
            "36: record of a space that does not exist",
        )
    }
}
