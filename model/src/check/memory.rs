//! The invariants of mapping records, address spaces and their tables.

use super::{Check, ensure};
use crate::{Mapping, Model, ObjId, Record};
use dodos_abi::{IOSPACE_MAX_DEVICES, Rights};
use std::collections::{BTreeMap, BTreeSet};

impl Model {
    /// Mapping records:
    /// 35. every full mapping record names exactly the translations it
    ///     made, and every translation belongs to exactly one record: the
    ///     translations of a space are those of the records of its list,
    ///     each page at its address, with the record's rights and the page
    ///     size of the frame; a record maps 1 to *E* pages that are in its
    ///     frame, inside the user range, and no two translations overlap;
    /// 36. a space lists, once each, exactly the capabilities whose records
    ///     name it;
    /// 37. the tables a space holds are at least those its translations
    ///     hook: one table of level *k* per span of *P* × *E*^*k* bytes that
    ///     holds a page smaller than the span; on the reference machine
    ///     (four levels, *E* = 512) one per 512 GB that holds a page, one per
    ///     1 GB that holds a page smaller than 1 GB, one per 2 MB that holds
    ///     a 4 KB page; an iospace of 39 bits has three levels;
    /// 38. the rights of a mapping in an aspace include `read` and not both
    ///     `write` and `exec`; in an iospace they are `read` or `write` or
    ///     both, inside its address width, of a page size every unit
    ///     supports;
    /// 39. a device attached through an `iommu` is in the list of the
    ///     iospace it names, and that list holds only such devices, at most
    ///     16, all of one unit and none the kernel uses; a unit gives no more
    ///     domain numbers than it has, and lists once each the devices
    ///     attached through it.
    pub(super) fn check_spaces(&self) -> Check {
        let mut named: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (&id, cap) in &self.caps {
            // A record unmapping stays in its space's list until its
            // invalidation is complete (D-131).
            if let Record::Mapping(mapping) | Record::Unmapping(mapping) = cap.record {
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
                // Shadowed by 34 (only a frame capability has a record),
                // which runs first; kept so that the check names a fault
                // instead of panicking on an index.
                let frame = self
                    .frames
                    .get(&held.object)
                    .ok_or("35: record of a frame")?;
                let size = self
                    .machine
                    .geometry
                    .bytes(frame.page)
                    .ok_or("35: page size")?;
                let inside = count >= 1
                    && first
                        .checked_add(count)
                        .is_some_and(|end| end <= frame.pages);
                let io = self.iospaces.contains_key(&id);
                let limit = match io {
                    true => 1u128 << self.machine.io_width,
                    false => self.machine.geometry.user_end as u128,
                };
                let end = (count as u128) * (size as u128) + address as u128;
                let below = end <= limit && address.is_multiple_of(size);
                ensure(inside && below, "35: pages of a record")?;
                let (r, w, x) = (Rights::READ, Rights::WRITE, Rights::EXEC);
                let valid = match io {
                    true => {
                        let sizes = self.machine.io_page_sizes & 1 << frame.page as u32 != 0;
                        rights != Rights::NONE && r.union(w).contains(rights) && sizes
                    }
                    false => rights.contains(r) && !rights.contains(w.union(x)),
                };
                ensure(valid, "38: rights of a mapping")?;
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
            let geometry = self.machine.geometry;
            let bytes = |size| geometry.bytes(size).map_or(0, u128::from);
            let mut end = 0;
            for (&at, t) in &space.translations {
                ensure(u128::from(at) >= end, "35: translations that overlap")?;
                end = u128::from(at) + bytes(t.size);
            }
            // The levels of the space, and the bytes a table of each level
            // covers: P × E^k. A page needs a table of every level that
            // covers more than the page.
            let span = |k: u32| u128::from(geometry.page()) * u128::from(geometry.entries).pow(k);
            let levels = match self.iospaces.contains_key(&id) {
                true => (1..=64)
                    .find(|&k| span(k) >= 1u128 << self.machine.io_width)
                    .unwrap_or(64),
                false => geometry.levels,
            };
            let mut hooked = BTreeSet::new();
            for (&at, t) in &space.translations {
                for k in (1..levels).filter(|&k| span(k) > bytes(t.size)) {
                    hooked.insert((k, u128::from(at) / span(k)));
                }
            }
            ensure(space.tables >= hooked.len() as u64, "37: tables of a space")?;
        }
        ensure(
            named.is_empty(),
            "36: record of a space that does not exist",
        )?;
        self.check_devices()
    }

    fn check_devices(&self) -> Check {
        for (&id, devices) in &self.iospaces {
            let attached = |&(iommu, device): &(ObjId, u16)| {
                self.iommus
                    .get(&iommu)
                    .is_some_and(|unit| unit.attached.get(&device) == Some(&id))
            };
            ensure(devices.iter().all(attached), "39: device of an iospace")?;
            let one_unit = devices.iter().all(|&(iommu, _)| iommu == devices[0].0);
            let kernel = devices.iter().any(|&(iommu, device)| {
                let unit = self.iommus.get(&iommu).map(|unit| unit.unit);
                unit.is_some_and(|unit| self.machine.kernel_devices.contains(&(unit, device)))
            });
            let few = devices.len() <= IOSPACE_MAX_DEVICES;
            let distinct = devices.iter().collect::<BTreeSet<_>>().len() == devices.len();
            ensure(
                one_unit && few && !kernel && distinct,
                "39: devices of an iospace",
            )?;
        }
        for (&id, unit) in &self.iommus {
            let listed = |(&device, iospace): (&u16, &ObjId)| {
                self.iospaces
                    .get(iospace)
                    .is_some_and(|devices| devices.contains(&(id, device)))
            };
            ensure(unit.attached.iter().all(listed), "39: device of a unit")?;
            let mut order = unit.order.clone();
            order.sort();
            let keys: Vec<u16> = unit.attached.keys().copied().collect();
            ensure(order == keys, "39: list of the devices of a unit")?;
            let domains: BTreeSet<_> = unit.attached.values().collect();
            let limit = self.machine.iommus.get(unit.unit).map_or(0, |u| u.domains);
            ensure(
                domains.len() as u32 <= limit,
                "39: domain numbers of a unit",
            )?;
        }
        // A domain number is taken from this node slot's range on each unit
        // of its iospace, and no two iospaces of one unit share one, nor
        // with a number lost to a destroyed iospace (Devices and shared
        // hardware, D-134; ver2-AD D3). It is kept from the attach, before
        // the device enters the header, so it is not tied to a device.
        let mut taken = BTreeSet::new();
        for (space, &domain) in &self.domains {
            let units = self.aspaces.get(space).map(|a| &a.units);
            ensure(
                units.is_some_and(|units| !units.is_empty()),
                "39: domain number of an iospace on no unit",
            )?;
            for &unit in units.into_iter().flatten() {
                ensure(
                    self.domain_range(unit, self.node).contains(&domain),
                    "39: domain number outside the node's range",
                )?;
                ensure(
                    taken.insert((unit, domain)) && !self.lost_domains.contains(&(unit, domain)),
                    "39: domain number shared",
                )?;
            }
        }
        Ok(())
    }
}
