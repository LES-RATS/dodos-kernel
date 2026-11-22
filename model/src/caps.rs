//! Capabilities and objects: the derivation tree, removal, object creation.

use crate::{
    ASpace, CSpace, Cap, CapId, Endpoint, Frame, Model, Notify, ObjId, Object, Outcome, Output,
    Place, Record, ReplyState, Sched, Slot, Sweep, Task, Thread, Untyped, live,
};
use dodos_abi::x86_64::{REG_RFLAGS, REGS_WORDS, RFLAGS_FIXED};
use dodos_abi::{
    CLEAR_STEP_BYTES, CSPACE_DIRECTORY_BYTES, CSPACE_MAX_SLOTS, CSPACE_SEGMENT_SLOTS,
    ENDPOINT_BYTES, Error, FeatureMask, Geometry, LONGEST_WALK, MemoryType, NOTIFY_BYTES,
    ObjectKind, PageSize, REMOVAL_SLOTS, REMOVAL_STEP, REPLY_BYTES, Rights, SCHED_BYTES,
    SLOT_BYTES, SlotAddr, TASK_BYTES, TIMER_BYTES, TREE_DEPTH, thread_bytes,
};

/// Slots and addresses: one segment of a capability space, 8 KB.
const SEGMENT_BYTES: u64 = CSPACE_SEGMENT_SLOTS as u64 * SLOT_BYTES as u64;

/// The size argument of `untyped_retype`, as the caller gives it. For a
/// frame: the number of pages, and the raw words of the page size and of
/// the memory type.
#[derive(Clone, Copy)]
pub(crate) struct SizeArgument {
    pub(crate) size: u64,
    pub(crate) page: u64,
    pub(crate) memory: u64,
}

/// Where `Model::take` placed an object, and where the clean mark of the
/// untyped stood before.
pub(crate) struct Taken {
    pub(crate) at: u64,
    clean: u64,
}

/// Rules of `untyped_retype`: whether a creation clears its memory in steps
/// that return `again`, or within the call (the tables of an address space
/// and the segments of a capability space, at most 24 KB). A child untyped
/// is not cleared.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Clearing {
    InSteps,
    WithinCall,
    None,
}

/// What an untyped is, for the kinds it makes (`untyped_retype`, D-154).
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Ordinary,
    Device,
    Tables,
}

/// How a capability comes to a slot, for `Model::fits`.
#[derive(Clone, Copy)]
pub(crate) enum Placing {
    Moved,
    /// A new capability derived from the one named.
    Derived,
    /// The root capability of a new object, made from the untyped named.
    Retyped(ObjectKind),
}

/// What the sweep of a dying capability space found, for `Model::swept`.
enum Swept {
    /// A capability still in a slot: it is removed next.
    Holds(CapId),
    /// No slot is left to examine, or the capability is no root of a space.
    Empty,
    /// The slots that the call may still examine ran out.
    OutOfBudget,
}

/// What `Model::undo` did with one unit of a removal step.
pub(crate) enum Undone {
    /// It undid one link to the object.
    Unit,
    /// Nothing is left to undo.
    Nothing,
    /// The step must end with `again`: a thread to leave another core, an
    /// invalidation of other cores or of a unit in flight, a count not yet
    /// 0, or no entry of the unit request table free.
    Wait,
}

/// What `cap_delete` and `cap_revoke` remove.
#[derive(Clone, Copy, PartialEq)]
enum Removal {
    Descendants,
    DescendantsThenSelf,
}

/// Capabilities and objects, with the table of Object sizes: the bytes and
/// the alignment of a new object, from its size argument. The kind is judged
/// before the size: device memory first (`invalid` but for a frame or a
/// child untyped), then a table untyped (`invalid` but for a child table
/// untyped or an `its_table`, which is `invalid` from any other untyped,
/// D-154), then a kind that this version does not make (`unsupported`) or
/// that is never made from an untyped (`invalid`, D-110). An `aspace` and
/// an `iospace` take here the one page of their header; their size
/// argument, the slot of a table untyped, is judged by the caller.
/// For a frame the size argument is judged in this order: the page size
/// value, whether the processor supports it, the memory type, the number of
/// pages.
fn footprint(
    kind: ObjectKind,
    arg: SizeArgument,
    source: Source,
    geometry: Geometry,
    thread: Result<u64, Error>,
) -> Result<(u64, u64, Option<Frame>), Error> {
    let page = geometry.page();
    let size = arg.size;
    let device = source == Source::Device;
    // Order of checks, step 3; `untyped_retype`: device memory makes only a
    // frame or a child untyped; a table untyped only a child table untyped
    // or an `its_table`, which no other untyped makes (D-154).
    let made = match source {
        Source::Device => matches!(kind, ObjectKind::Frame | ObjectKind::Untyped),
        Source::Tables => matches!(kind, ObjectKind::Untyped | ObjectKind::ItsTable),
        Source::Ordinary => kind != ObjectKind::ItsTable,
    };
    if !made {
        return Err(Error::Invalid);
    }
    let segments = size / u64::from(CSPACE_SEGMENT_SLOTS);
    let fixed = |bytes: usize| (bytes as u64, size == 0);
    let (bytes, valid) = match kind {
        // Capabilities and objects: a power of two, at least one page.
        ObjectKind::Untyped => (size, size.is_power_of_two() && size >= page),
        ObjectKind::Frame => {
            let size_index = PageSize::from_raw(arg.page).ok_or(Error::Invalid)?;
            let bytes = geometry.bytes(size_index).ok_or(Error::Unsupported)?;
            let memory = MemoryType::for_memory(arg.memory, device)?;
            if size == 0 || size > u64::from(geometry.entries) {
                return Err(Error::Invalid);
            }
            // Rules of `untyped_retype`: a frame takes exactly its number of
            // pages times its page size, aligned on its page size.
            let frame = Frame {
                page: size_index,
                pages: size,
                memory,
                base: 0,
                kept: false,
            };
            return Ok((size * bytes, bytes, Some(frame)));
        }
        // Slots and addresses: a non-zero multiple of 128, at most 2^20, and
        // a directory rounded up to a power of two.
        ObjectKind::CSpace => (
            (segments * CSPACE_DIRECTORY_BYTES as u64).next_power_of_two(),
            size > 0
                && size.is_multiple_of(CSPACE_SEGMENT_SLOTS.into())
                && size <= CSPACE_MAX_SLOTS.into(),
        ),
        // Address spaces and their tables: the header, one page of the
        // ordinary untyped; the root table is one page of the table untyped
        // the size argument names (D-154).
        ObjectKind::ASpace | ObjectKind::IoSpace => (dodos_abi::SPACE_HEADER_PAGES * page, true),
        // Object sizes: a thread is its fixed part and its save area,
        // rounded up to a power of two.
        ObjectKind::Thread => (thread?, true),
        ObjectKind::Sched => fixed(SCHED_BYTES),
        ObjectKind::Endpoint => fixed(ENDPOINT_BYTES),
        ObjectKind::Reply => fixed(REPLY_BYTES),
        ObjectKind::Notify => fixed(NOTIFY_BYTES),
        ObjectKind::Timer => fixed(TIMER_BYTES),
        ObjectKind::Task => fixed(TASK_BYTES),
        // D-152 complement: an `its_table` is 2^k pages, k at most 4 (above,
        // `invalid`), aligned on its size (D-151).
        ObjectKind::ItsTable => (
            page << size.min(4),
            size <= u64::from(dodos_abi::ITS_TABLE_K_MAX),
        ),
        // Order of checks, step 3: a kind never made from an untyped is
        // `invalid` (D-110); the reserved `vcpu` is `unsupported`.
        ObjectKind::Irq
        | ObjectKind::IrqControl
        | ObjectKind::Iommu
        | ObjectKind::MemoryControl
        | ObjectKind::Node
        | ObjectKind::Role
        | ObjectKind::Debug
        | ObjectKind::IoPorts => return Err(Error::Invalid),
        ObjectKind::Vcpu => return Err(Error::Unsupported),
    };
    // Rules of `untyped_retype`: aligned on the size of a child untyped, and
    // for the other objects on their size, or on a page if they are larger.
    let align = match kind {
        ObjectKind::Untyped | ObjectKind::ItsTable => bytes,
        _ => bytes.min(page),
    };
    valid.then_some((bytes, align, None)).ok_or(Error::Invalid)
}

impl Model {
    pub(crate) fn cap_derive(
        &mut self,
        thread: ObjId,
        source: SlotAddr,
        destination: SlotAddr,
        rights: u32,
        badge: u64,
    ) -> Outcome {
        let parent = self.cap(thread, source, Rights::READ)?;
        let source = &self.caps[&parent];
        let (object, held, kept) = (source.object, source.rights, source.badge);
        // Capabilities and objects: `derive` on the source.
        if !held.contains(Rights::DERIVE) {
            return Err(Error::Denied);
        }
        let place = self.destination(thread, destination)?;
        // Capabilities and objects: the rights word is a plain argument,
        // judged after the three checks of its slot and after the
        // destination. A bit that names no right is `invalid`, before the
        // subset.
        let rights = Rights::from_bits(rights).ok_or(Error::Invalid)?;
        // The new rights are a subset of the source's. Order of checks, step
        // 3: in the order of the table, the rights before the badge.
        if !held.contains(rights) {
            return Err(Error::Denied);
        }
        // Capabilities and objects: a badge only on an endpoint or a notify.
        let kind = self.objects[&object].kind;
        if badge != 0 && kind != ObjectKind::Endpoint && kind != ObjectKind::Notify {
            return Err(Error::Invalid);
        }
        // Capabilities and objects: another badge is `denied`, checked with
        // the plain arguments; the same one is accepted.
        if badge != 0 && kept != 0 && badge != kept {
            return Err(Error::Denied);
        }
        // Order of checks, step 3: the depth checks come last.
        self.check_fits(place.space, parent, Placing::Derived)?;
        self.vacant(place)?;
        let badge = if kept != 0 { kept } else { badge };
        self.derive(parent, rights, badge, place);
        Ok(Output::Done)
    }

    pub(crate) fn cap_move(
        &mut self,
        thread: ObjId,
        source: SlotAddr,
        destination: SlotAddr,
    ) -> Outcome {
        let cap = self.cap(thread, source, Rights::WRITE)?;
        let place = self.destination(thread, destination)?;
        self.check_fits(place.space, cap, Placing::Moved)?;
        // State, `busy` first: the root capability of an untyped that has a
        // descendant (`cap_move`, D-131).
        if self.root_with_copies(cap) {
            return Err(Error::Busy);
        }
        self.vacant(place)?;
        self.place(cap, place);
        Ok(Output::Done)
    }

    /// Whether a capability is the root capability of an untyped that has
    /// a descendant. It does not move, by `cap_move` or by a *move* in a
    /// message: its copies name its slot, where the watermark and the
    /// clean mark are (The capability slot, D-131).
    pub(crate) fn root_with_copies(&self, cap: CapId) -> bool {
        let untyped = |object: ObjId| self.objects[&object].kind == ObjectKind::Untyped;
        self.rooted(cap).is_some_and(untyped) && !self.caps[&cap].children.is_empty()
    }

    pub(crate) fn cap_delete(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        // Removal ends: the one call left to a thread whose capability space
        // is dying, on its own slots only.
        let own = self.bound_space(thread)?;
        if self.dying(own) && slot.via() != 0 {
            return Err(Error::Gone);
        }
        let space = self.through(own, slot, Rights::WRITE)?;
        match self.cspaces[&space].slots.get(slot.index() as usize) {
            // Slots and addresses: `cap_delete` on a dead slot makes it
            // empty.
            Some(Slot::Dead) => {
                let index = slot.index();
                *self.slot(Place { space, index }) = Slot::Empty;
                Ok(Output::Done)
            }
            // Removal ends: from the first step the capability is dying,
            // and only `cap_delete` can still use it.
            Some(&Slot::Full(cap)) => {
                live(&mut self.caps, cap).dying = true;
                self.remover = Some(thread);
                let outcome = self.remove(cap, Removal::DescendantsThenSelf);
                self.remover = None;
                outcome
            }
            Some(Slot::Empty) | None => Err(Error::Invalid),
        }
    }

    pub(crate) fn cap_revoke(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let cap = self.cap(thread, slot, Rights::WRITE)?;
        self.remover = Some(thread);
        let outcome = self.remove(cap, Removal::Descendants);
        self.remover = None;
        outcome
    }

    pub(crate) fn cap_inspect(&mut self, thread: ObjId, slot: SlotAddr) -> Outcome {
        let cap = &self.caps[&self.cap(thread, slot, Rights::READ)?];
        Ok(Output::Inspect {
            kind: self.objects[&cap.object].kind,
            rights: cap.rights,
            badge: cap.badge,
        })
    }

    pub(crate) fn untyped_retype(
        &mut self,
        thread: ObjId,
        untyped: SlotAddr,
        kind: ObjectKind,
        size: SizeArgument,
        destination: SlotAddr,
    ) -> Outcome {
        let thread_of = thread;
        let parent = self.cap(thread, untyped, Rights::READ)?;
        let &Cap { object, rights, .. } = self.is(parent, ObjectKind::Untyped, Rights::WRITE)?;
        let place = self.destination(thread, destination)?;
        // Order of checks, step 3: the kind is judged before the size
        // argument.
        let source = match &self.untypeds[&object] {
            untyped if untyped.device => Source::Device,
            untyped if untyped.table => Source::Tables,
            _ => Source::Ordinary,
        };
        let thread = match kind {
            ObjectKind::Thread => self.thread_size(size.size),
            _ => Ok(0),
        };
        let geometry = self.machine.geometry;
        let (bytes, align, frame) = match footprint(kind, size, source, geometry, thread) {
            Ok(v) => v,
            Err(e) => {
                #[cfg(test)]
                if e == Error::Invalid
                    && kind == ObjectKind::CSpace
                    && source == Source::Ordinary
                    && size.size > u64::from(CSPACE_MAX_SLOTS)
                    && size.size.is_multiple_of(u64::from(CSPACE_SEGMENT_SLOTS))
                {
                    self.bound_hits.0.cspace_over_max_refused = true;
                }
                return Err(e);
            }
        };
        // Order of checks, step 3 (D-154): for an `aspace` or an `iospace`
        // the size argument is the slot address of a table untyped with
        // `write`, checked here as a capability argument: slot, kind, the
        // table bit (`invalid`), `write`; a dying one is `gone`.
        let space = matches!(kind, ObjectKind::ASpace | ObjectKind::IoSpace);
        let tables = match space {
            true => Some(self.table_untyped(thread_of, size.size)?),
            false => None,
        };
        self.check_fits(place.space, parent, Placing::Retyped(kind))?;
        self.vacant(place)?;
        // Rules of `untyped_retype` (D-154): a state check after `no_slot`:
        // `again` with nothing changed while the watermark of either
        // untyped is ready to return, then `no_memory` if the root table
        // would end beyond the end of the table untyped, before any
        // clearing.
        let page = geometry.page();
        if let Some(tables) = tables {
            self.return_watermark(object)?;
            self.return_watermark(tables)?;
            let untyped = &self.untypeds[&tables];
            let end = untyped
                .watermark
                .checked_next_multiple_of(page)
                .and_then(|start| start.checked_add(page));
            if end.is_none_or(|end| end > untyped.bytes) {
                return Err(Error::NoMemory);
            }
        }
        // Rules of `untyped_retype`: the kernel clears every object it
        // creates, but a child untyped (D-110). A call that has not reached
        // the end of the new object returns `again` and makes nothing.
        let clearing = match kind {
            ObjectKind::Untyped => Clearing::None,
            _ => Clearing::InSteps,
        };
        let start = self.take(object, bytes, align, clearing)?;
        // Rules of `untyped_retype`: every right, but `exec` only if the
        // untyped capability has it, and never on device memory: the
        // start-up capabilities for device memory have no `exec`.
        let new = if rights.contains(Rights::EXEC) {
            Rights::ALL
        } else {
            Rights::ALL.without(Rights::EXEC)
        };
        let cap = self.create(kind, size.size, Some(parent), new, place);
        let id = self.caps[&cap].object;
        let base = self.untypeds[&object].base + start.at;
        if let Some(frame) = frame {
            self.frames.insert(id, Frame { base, ..frame });
        }
        // A child untyped: its clean mark starts at the end of the part of
        // its range that the parent's clean mark covered.
        if let Some(child) = self.untypeds.get_mut(&id) {
            child.clean = start.clean.saturating_sub(start.at).min(bytes);
            child.base = base;
            if child.clean > 0 {
                child.zero.push((0, child.clean));
            }
        }
        // The gate over page tables, G2 (D-154): the root table of a space
        // is one page of the table untyped, a page of a table chunk that
        // the gate clears and turns into a table page as it leaves *data*.
        if let Some(tables) = tables {
            let taken = self
                .take(tables, page, page, Clearing::WithinCall)
                .expect("judged to fit");
            let root = self.untypeds[&tables].base + taken.at;
            let class = self.root_class(id);
            let space = live(&mut self.aspaces, id);
            (space.root, space.table_untyped) = (Some(root), Some(tables));
            self.table.set_class(root / page, Some(class));
        }
        // An `its_table`: its 2^k pages turn into *ITS table*, cleared by
        // the gate in the retype's call (Gate words, the ITS word; D-154).
        if kind == ObjectKind::ItsTable {
            let pages = bytes / page;
            self.its_tables.insert(id, (base / page, pages));
            for at in base / page..base / page + pages {
                let class = dodos_abi::PageClass::its_table(false);
                self.table.set_class(at, Some(class));
            }
        }
        Ok(Output::Done)
    }

    pub(crate) fn cspace_grow(
        &mut self,
        thread: ObjId,
        cspace: SlotAddr,
        untyped: SlotAddr,
    ) -> Outcome {
        let space = self.object(thread, cspace, ObjectKind::CSpace, Rights::MANAGE)?;
        let untyped = self.object(thread, untyped, ObjectKind::Untyped, Rights::WRITE)?;
        // Slots and addresses: only from the untyped it was created from;
        // any capability for that untyped object is accepted. Errors:
        // `no_slot` also means that the space is at its maximum.
        // The derivation tree: the start-up space, created from no untyped,
        // never grows.
        if self.objects[&space].from != Some(untyped) {
            return Err(Error::Invalid);
        }
        let CSpace { max, slots, .. } = &self.cspaces[&space];
        if slots.len() >= *max as usize {
            #[cfg(test)]
            if *max >= CSPACE_MAX_SLOTS {
                self.bound_hits.0.cspace_over_max_refused = true;
            }
            return Err(Error::NoSlot);
        }
        // Rules of `untyped_retype`: a segment is 8 KB, aligned on its
        // size, and cleared within the call.
        self.take(untyped, SEGMENT_BYTES, SEGMENT_BYTES, Clearing::WithinCall)?;
        let space = live(&mut self.cspaces, space);
        let slots = space.slots.len() + CSPACE_SEGMENT_SLOTS as usize;
        space.slots.resize(slots, Slot::Empty);
        Ok(Output::Slots(slots as u32))
    }

    /// The size argument of an `aspace` or an `iospace` (D-154): the slot
    /// address of a table untyped, judged as a capability argument: its
    /// slot, its kind, that it is a table untyped (`invalid`), `write`.
    fn table_untyped(&self, thread: ObjId, slot: u64) -> Result<ObjId, Error> {
        let cap = self.cap(thread, SlotAddr::from_raw(slot), Rights::READ)?;
        let kind = self.objects[&self.caps[&cap].object].kind;
        let object = self.caps[&cap].object;
        if kind != ObjectKind::Untyped || !self.untypeds[&object].table {
            return Err(Error::Invalid);
        }
        Ok(self.is(cap, ObjectKind::Untyped, Rights::WRITE)?.object)
    }

    /// Unlike cores: the size argument of a thread is its feature mask,
    /// judged on the node's own kind (`invalid`, then `unsupported`); the
    /// thread's bytes follow from it.
    pub(crate) fn thread_size(&self, mask: u64) -> Result<u64, Error> {
        let kind = self.machine.kind_of(self.cores[0].number as usize);
        let list = self.machine.features();
        let mask = FeatureMask::from_raw(mask);
        mask.check(list, &kind)?;
        thread_bytes(mask, list, &kind).ok_or(Error::Unsupported)
    }

    /// The derivation tree: a start-up capability is at depth 1. The walk
    /// stops at a parent that does not exist, which `check` reports.
    pub(crate) fn depth(&self, cap: CapId) -> usize {
        let chain: Vec<CapId> =
            std::iter::successors(Some(cap), |cap| self.caps.get(cap)?.parent).collect();
        // Grants: a root remote capability has the depth of its anchor.
        let top = chain.last().and_then(|top| self.caps.get(top));
        let offset = top
            .and_then(|top| self.remotes.get(&top.object))
            .map_or(0, |remote| remote.depth.saturating_sub(1));
        chain.len() + offset
    }

    /// The derivation tree, checked with the plain arguments each time a
    /// capability is placed in a slot of `space`: the tree is at most 64
    /// deep, and a capability space holds only what is deeper than itself.
    /// The root capability of another space must be deeper than the root
    /// capability of `space`, and an untyped or `memory_control` capability
    /// not less deep (D-141).
    pub(crate) fn fits(&self, space: ObjId, source: CapId, placing: Placing) -> Result<(), Error> {
        let own = self.objects[&space].root;
        let kind = self.objects[&self.caps[&source].object].kind;
        // A space may hold its own root capability; a copy is not a root.
        let moved_root = self.rooted(source).is_some() && source != own;
        let (kind, root, depth) = match placing {
            Placing::Moved => (kind, moved_root, self.depth(source)),
            Placing::Derived => (kind, false, self.depth(source) + 1),
            Placing::Retyped(kind) => (kind, true, self.depth(source) + 1),
        };
        let deep_enough = match kind {
            // An `untyped` or a `memory_control` (a root or a copy) is
            // placed only if its depth is not smaller than the root's: a
            // `memory_control` derives untypeds, so a space made from one of
            // its blocks could hold it otherwise, a cycle (D-103 amended by
            // D-141).
            ObjectKind::Untyped | ObjectKind::MemoryControl => depth >= self.depth(own),
            ObjectKind::CSpace if root => depth > self.depth(own),
            _ => true,
        };
        (deep_enough && depth <= TREE_DEPTH)
            .then_some(())
            .ok_or(Error::Invalid)
    }

    /// Calls `fits` and, in test builds, records `tree_depth_65_refused` only
    /// when a valid placement fails specifically because `depth > TREE_DEPTH`.
    pub(crate) fn check_fits(
        &mut self,
        space: ObjId,
        source: CapId,
        placing: Placing,
    ) -> Result<(), Error> {
        let res = self.fits(space, source, placing);
        #[cfg(test)]
        if res == Err(Error::Invalid) {
            let own = self.objects[&space].root;
            let kind = self.objects[&self.caps[&source].object].kind;
            let moved_root = self.rooted(source).is_some() && source != own;
            let (kind, root, depth) = match placing {
                Placing::Moved => (kind, moved_root, self.depth(source)),
                Placing::Derived => (kind, false, self.depth(source) + 1),
                Placing::Retyped(kind) => (kind, true, self.depth(source) + 1),
            };
            let deep_enough = match kind {
                ObjectKind::Untyped | ObjectKind::MemoryControl => depth >= self.depth(own),
                ObjectKind::CSpace if root => depth > self.depth(own),
                _ => true,
            };
            if deep_enough && depth > TREE_DEPTH {
                self.bound_hits.0.tree_depth_65_refused = true;
            }
        }
        res
    }

    /// Rules of `untyped_retype`: memory is taken at a watermark that only
    /// moves up, aligned. `no_memory` if it does not fit; then the memory
    /// from the watermark to the end of the new object must be clear: the
    /// call clears from the clean mark, at most 64 KB, and returns `again`
    /// while the end is not reached. Device memory is never cleared.
    ///
    /// A step stops at the end of the new object if it comes first, and
    /// clears the padding before it with it.
    pub(crate) fn take(
        &mut self,
        untyped: ObjId,
        bytes: u64,
        align: u64,
        clearing: Clearing,
    ) -> Result<Taken, Error> {
        if clearing != Clearing::WithinCall {
            self.return_watermark(untyped)?;
        }
        let untyped = live(&mut self.untypeds, untyped);
        let start = untyped.watermark.checked_next_multiple_of(align);
        let end = start.and_then(|start| start.checked_add(bytes));
        let (start, end) = match (start, end) {
            (Some(start), Some(end)) if end <= untyped.bytes => (start, end),
            _ => return Err(Error::NoMemory),
        };
        let clean = untyped.clean;
        // Device memory and a table untyped have no clean mark: the gate
        // clears each page of a table untyped as it leaves *data*, within
        // the call (`memory_take`, D-154).
        let marked = !untyped.device && !untyped.table;
        if marked && clearing != Clearing::None && untyped.clean < end {
            // A step clears from the clean mark, padding included, at most
            // 64 KB, and stops at the end of the new object.
            let reached = match clearing {
                Clearing::InSteps => (untyped.clean + CLEAR_STEP_BYTES).min(end),
                _ => end,
            };
            untyped.zero.push((untyped.clean, reached));
            untyped.clean = reached;
            if reached < end {
                #[cfg(test)]
                {
                    self.bound_hits.0.clear_step_again = true;
                }
                return Err(Error::Again);
            }
        }
        untyped.watermark = end;
        // What lies below the watermark is taken: no longer known zero.
        untyped.zero.retain(|&(_, e)| e > end);
        for range in &mut untyped.zero {
            range.0 = range.0.max(end);
        }
        // When the watermark moves past the clean mark, the clean mark moves
        // with it.
        if marked {
            untyped.clean = untyped.clean.max(end);
        }
        untyped.live += 1;
        Ok(Taken { at: start, clean })
    }

    /// Rules of `untyped_retype`: when the last object made from the untyped
    /// is destroyed the watermark returns to the start, and so does the
    /// clean mark; this is the only moment the memory becomes usable again.
    fn release(&mut self, untyped: ObjId) {
        // On a node of several cores the watermark is only ready to return:
        // the destroying event raises the kernel epoch, and the memory is
        // reused after a barrier, since the lock-free paths reach object
        // memory before the node lock (A node of several cores, D-135,
        // D-138).
        let watched = self.width() > 1;
        if watched && self.untypeds[&untyped].live == 1 {
            self.kernel_epoch += 1;
            let epoch = self.kernel_epoch;
            self.locks.steps.push(crate::locks::Step::Epoch(epoch));
        }
        let epoch = self.kernel_epoch;
        let id = untyped;
        let untyped = live(&mut self.untypeds, untyped);
        untyped.live -= 1;
        match (untyped.live, watched) {
            (0, true) => {
                untyped.ready = Some(epoch);
                self.locks.steps.push(crate::locks::Step::Ready(id));
            }
            (0, false) => {
                (untyped.watermark, untyped.clean) = (0, 0);
                untyped.zero.clear();
            }
            _ => {}
        }
    }

    /// A watermark ready to return returns, and the clean mark with it,
    /// once a complete barrier covers the epoch at which it became ready:
    /// every other core of the node has then left the kernel event it was
    /// in at the destruction. Until then the call starts the barrier, if
    /// none in flight covers that epoch, and returns `again` with nothing
    /// changed (A node of several cores, D-134, D-135, D-138).
    pub(crate) fn return_watermark(&mut self, untyped: ObjId) -> Result<(), Error> {
        let Some(epoch) = self.untypeds[&untyped].ready else {
            return Ok(());
        };
        if epoch > self.barrier_epoch {
            let covered =
                self.cores.iter().filter_map(|c| c.request.as_ref()).any(
                    |r| matches!(r.completes, crate::cores::Completes::Barrier(e) if e >= epoch),
                );
            if !covered {
                self.start_barrier();
            }
            if epoch > self.barrier_epoch {
                return Err(Error::Again);
            }
        }
        let untyped = live(&mut self.untypeds, untyped);
        (untyped.watermark, untyped.clean, untyped.ready) = (0, 0, None);
        untyped.zero.clear();
        Ok(())
    }

    /// A new object and its root capability, in an empty slot. The parent
    /// is the untyped capability the memory comes from; a capability given
    /// at start-up has none.
    pub(crate) fn create(
        &mut self,
        kind: ObjectKind,
        size: u64,
        parent: Option<CapId>,
        rights: Rights,
        place: Place,
    ) -> CapId {
        let id = ObjId(self.next_id);
        self.next_id += 1;
        let from = parent.map(|parent| self.caps[&parent].object);
        match kind {
            ObjectKind::Untyped => {
                // A child untyped lies inside its parent; `untyped_retype`
                // places it.
                let untyped = Untyped {
                    base: 0,
                    block: None,
                    zero: Vec::new(),
                    bytes: size,
                    device: from
                        .and_then(|from| self.untypeds.get(&from))
                        .is_some_and(|from| from.device),
                    // A child of a table untyped is a table untyped (D-154).
                    table: from
                        .and_then(|from| self.untypeds.get(&from))
                        .is_some_and(|from| from.table),
                    watermark: 0,
                    clean: 0,
                    live: 0,
                    lost: 0,
                    ready: None,
                };
                self.untypeds.insert(id, untyped);
            }
            ObjectKind::CSpace => {
                // Slots and addresses: created with a maximum and no slot.
                // The one given at start-up has all its slots.
                let slots = if parent.is_none() { size as usize } else { 0 };
                let space = CSpace {
                    max: size as u32,
                    slots: vec![Slot::Empty; slots],
                    sweep: None,
                    threads: Vec::new(),
                };
                self.cspaces.insert(id, space);
            }
            ObjectKind::ASpace => {
                self.aspaces.insert(id, ASpace::default());
            }
            // IOMMU: an iospace has tables and records as an aspace has,
            // and the devices it serves.
            ObjectKind::IoSpace => {
                self.aspaces.insert(id, ASpace::default());
                self.iospaces.insert(id, Vec::new());
            }
            ObjectKind::Thread => {
                // Threads: a new thread is suspended and bound to nothing.
                // Register calls: the block of a new thread is all zeros, except
                // the flags at their fixed value.
                let mut regs = vec![0; REGS_WORDS];
                regs[REG_RFLAGS] = RFLAGS_FIXED;
                let thread = Thread {
                    suspended: true,
                    regs,
                    mask: size,
                    area: self.thread_size(size).expect("judged by the caller"),
                    ..Thread::default()
                };
                self.threads.insert(id, thread);
            }
            ObjectKind::Endpoint => {
                self.endpoints.insert(id, Endpoint::default());
            }
            ObjectKind::Reply => {
                self.replies.insert(id, ReplyState::Idle);
            }
            ObjectKind::Notify => {
                self.notifies.insert(id, Notify::default());
            }
            // Scheduling contexts: a new one is not configured, has no
            // home, class 2, the cores set of every core of the node that
            // holds it, and is placed on the node's first core.
            ObjectKind::Sched => {
                let sched = Sched {
                    class: 2,
                    cores: self.cores_set(),
                    core: 0,
                    ..Sched::default()
                };
                self.scheds.insert(id, sched);
            }
            // Timers: a new timer is idle.
            ObjectKind::Timer => {
                self.timers.insert(id, None);
            }
            // Tasks and marks: a new task has the next identifier, marks 0,
            // is neither mark-aware nor revoked, and has no thread.
            ObjectKind::Task => {
                let task = Task {
                    id: self.next_task(),
                    marks: 0,
                    aware: false,
                    revoked: false,
                    threads: Vec::new(),
                };
                self.tasks.insert(id, task);
            }
            _ => {}
        }
        // The derivation tree: the capability that `untyped_retype` returns,
        // or the one given at start-up, is the root capability.
        let root = self.new_cap(id, rights, 0, parent, place);
        self.objects.insert(
            id,
            Object {
                kind,
                root,
                from,
                boot: false,
            },
        );
        root
    }

    fn new_cap(
        &mut self,
        object: ObjId,
        rights: Rights,
        badge: u64,
        parent: Option<CapId>,
        place: Place,
    ) -> CapId {
        let id = CapId(self.next_id);
        self.next_id += 1;
        let cap = Cap {
            object,
            rights,
            badge,
            parent,
            children: Vec::new(),
            place,
            dying: false,
            record: Record::Empty,
        };
        self.caps.insert(id, cap);
        if let Some(parent) = parent {
            live(&mut self.caps, parent).children.push(id);
        }
        *self.slot(place) = Slot::Full(id);
        id
    }

    /// The derivation tree: a new capability for the same object, child of
    /// `parent`, in an empty slot.
    pub(crate) fn derive(&mut self, parent: CapId, rights: Rights, badge: u64, place: Place) {
        self.new_cap(
            self.caps[&parent].object,
            rights,
            badge,
            Some(parent),
            place,
        );
    }

    /// The derivation tree: puts a capability in an empty slot and empties
    /// the slot it was in. Nothing else about it changes.
    pub(crate) fn place(&mut self, cap: CapId, place: Place) {
        let old = std::mem::replace(&mut live(&mut self.caps, cap).place, place);
        *self.slot(old) = Slot::Empty;
        *self.slot(place) = Slot::Full(cap);
    }

    fn slot(&mut self, Place { space, index }: Place) -> &mut Slot {
        &mut live(&mut self.cspaces, space).slots[index as usize]
    }

    /// The derivation tree: removal in bounded steps. Per call, at most 32
    /// units of work are done (a capability removed, a thread woken, or a
    /// link of a destroyed object undone, D-110), and at most 128 slots of
    /// dying capability spaces are examined; `again` while work remains.
    /// `cap_revoke` on the root capability of a space starts below it, and
    /// so does not sweep the space.
    fn remove(&mut self, target: CapId, removal: Removal) -> Outcome {
        let (mut steps, mut slots) = (REMOVAL_STEP, REMOVAL_SLOTS);
        loop {
            let start = match removal {
                Removal::DescendantsThenSelf => target,
                Removal::Descendants => match self.caps[&target].children.last() {
                    Some(&child) => child,
                    None => return Ok(Output::Done),
                },
            };
            if steps == 0 {
                #[cfg(test)]
                {
                    self.bound_hits.0.removal_32_again = true;
                }
                return Err(Error::Again);
            }
            steps -= 1;
            let Some(next) = self.next_removed(start, &mut slots) else {
                #[cfg(test)]
                {
                    self.bound_hits.0.removal_128_slots_again = true;
                }
                return Err(Error::Again);
            };
            if let Some(waiter) = self.waiter(next) {
                // The derivation tree: destroying an object wakes every
                // thread that waits on it, with `gone`. IPC: threads that
                // wait on the same object are served first in, first out.
                self.unlink(waiter);
                self.wake(waiter, Err(Error::Gone));
                continue;
            }
            // What else destruction undoes (D-110): after the waiting threads
            // are woken, and for a `cspace` after its sweep, each link to the
            // object is undone before the root capability is removed.
            if let Some(object) = self.rooted(next) {
                match self.undo(object) {
                    Undone::Unit => continue,
                    Undone::Wait => return Err(Error::Again),
                    Undone::Nothing => {}
                }
            }
            // A removal step never waits: a record whose invalidation needs
            // other cores or a unit is cleared, its invalidation started,
            // and the step returns `again`; a capability whose record is
            // unmapping is not removed until it is empty.
            match self.caps[&next].record {
                Record::Unmapping(_) => {
                    #[cfg(test)]
                    {
                        self.bound_hits.0.removal_unmapping_stop = true;
                    }
                    return Err(Error::Again);
                }
                Record::Mapping(_) => match self.begin_unmap(next, None) {
                    Ok(crate::memory::Unmapped::Done) => {}
                    _ => {
                        #[cfg(test)]
                        if matches!(self.caps[&next].record, Record::Unmapping(_)) {
                            self.bound_hits.0.removal_unmapping_stop = true;
                        }
                        return Err(Error::Again);
                    }
                },
                _ => {}
            }
            // Grants: removing an anchor revokes its grant, and may wait for
            // the holder's node to remove what it holds of it.
            if self.grant_of(next).is_some() {
                match self.revoke_anchor(next, self.remover) {
                    Ok(true) => {}
                    Ok(false) => return Err(Error::Again),
                    Err(outcome) => return outcome,
                }
            }
            // Slots and addresses: `cap_delete` acts on the slot it names,
            // which becomes empty; a capability removed by someone else
            // leaves a dead slot.
            let left = if next == target {
                Slot::Empty
            } else {
                Slot::Dead
            };
            self.remove_cap(next, left);
            if next == target {
                return Ok(Output::Done);
            }
        }
    }

    /// The derivation tree, the fixed order: the most recently derived child
    /// first, a capability only after all its descendants, and the root
    /// capability of a `cspace` only after the capabilities in it, in index
    /// order. `None` when the slots that the call may still examine run out
    /// before the next capability to remove is found.
    ///
    /// Removal ends: an object is dying from the first step of a removal
    /// that reaches its root capability, even when that step only removes a
    /// copy; and the step that marks a reply object wakes, with `gone`, the
    /// thread attached to it or the caller it is armed with. That wake is
    /// not counted in the 32 of a step.
    fn next_removed(&mut self, mut cap: CapId, slots: &mut usize) -> Option<CapId> {
        for walk in 1.. {
            // The derivation tree: the walk is shorter than 192. Twice the
            // longest is a walk that does not end: a fault of the model, not
            // a state to report.
            assert!(
                walk <= 2 * LONGEST_WALK,
                "the walk of a removal does not end"
            );
            self.longest_walk = self.longest_walk.max(walk);
            if let Some(object) = self.rooted(cap) {
                live(&mut self.caps, cap).dying = true;
                if self.scheds.get(&object).is_some_and(|sched| !sched.retired) {
                    self.sched_dying(object);
                }
                if let Some(space) = self.cspaces.get_mut(&object) {
                    // The derivation tree: the sweep of a space starts at
                    // slot 1.
                    space.sweep.get_or_insert_with(|| {
                        let held = space.slots.iter().filter_map(|slot| match *slot {
                            Slot::Full(held) => Some(held),
                            Slot::Empty | Slot::Dead => None,
                        });
                        Sweep {
                            next: 1,
                            first_deferred: None,
                            pass2: false,
                            held: held.collect(),
                        }
                    });
                }
                let waiter = match self.replies.get(&object) {
                    Some(&ReplyState::Attached(thread)) => Some(thread),
                    Some(&ReplyState::Armed { caller, .. }) => Some(caller),
                    Some(ReplyState::Idle) | None => None,
                };
                if let Some(waiter) = waiter {
                    self.unlink(waiter);
                    self.wake(waiter, Err(Error::Gone));
                }
            }
            match self.caps[&cap].children.last() {
                Some(&child) => cap = child,
                None => match self.swept(cap, slots) {
                    Swept::Holds(held) => cap = held,
                    Swept::Empty => break,
                    Swept::OutOfBudget => return None,
                },
            }
        }
        Some(cap)
    }

    /// The first capability left in the capability space that `cap` is the
    /// root capability of, if `cap` is one.
    ///
    /// The derivation tree: a bounded sweep in two passes, at most twice the
    /// slots of the space examined (D-156). The dying space keeps the index
    /// where the sweep stands and the first slot it deferred. Pass 1 defers
    /// a slot that holds the root capability of a table untyped which spaces
    /// still name, and moves past it, so the spaces in the higher slots of
    /// the same dying space are removed first; at the end of the space,
    /// pass 2 resumes from the first deferred slot and defers nothing: a
    /// table untyped that a space outside still names is `again` there. A
    /// space may hold its own root capability, which is removed last.
    ///
    /// The root capability of an `its_table` whose ITS word is not 0 is
    /// deferred the same way by the contract; this model keeps no ITS word
    /// (the gate model does), so no `its_table` waits here.
    fn swept(&mut self, cap: CapId, slots: &mut usize) -> Swept {
        let space = self
            .rooted(cap)
            .and_then(|space| self.cspaces.get_mut(&space));
        let Some(space) = space else {
            return Swept::Empty;
        };
        let sweep = space.sweep.as_mut().expect("the walk marked the space");
        // The derivation tree: a slot counts as examined when the sweep
        // moves past it, so a full slot is not counted while its capability
        // is being removed. A step may end on the slot budget alone.
        loop {
            let Some(&slot) = space.slots.get(sweep.next) else {
                match sweep.first_deferred.take() {
                    Some(first) if !sweep.pass2 => {
                        (sweep.pass2, sweep.next) = (true, first);
                        continue;
                    }
                    _ => return Swept::Empty,
                }
            };
            let deferred = match slot {
                Slot::Full(held) if held != cap => {
                    // A table untyped counts a child table untyped among
                    // its live pieces, so a space that names a descendant
                    // defers it too.
                    let object = self.caps[&held].object;
                    let pinned = self.objects[&object].root == held
                        && self
                            .untypeds
                            .get(&object)
                            .is_some_and(|untyped| untyped.table && untyped.live > untyped.lost);
                    if sweep.pass2 || !pinned {
                        return Swept::Holds(held);
                    }
                    true
                }
                Slot::Full(_) | Slot::Empty | Slot::Dead => false,
            };
            let Some(left) = slots.checked_sub(1) else {
                return Swept::OutOfBudget;
            };
            *slots = left;
            if deferred {
                sweep.first_deferred.get_or_insert(sweep.next);
            }
            sweep.next += 1;
        }
    }

    /// The object that `cap` is the root capability of, if it is one.
    fn rooted(&self, cap: CapId) -> Option<ObjId> {
        let object = self.caps[&cap].object;
        (self.objects[&object].root == cap).then_some(object)
    }

    /// The first thread in a queue of the object that `cap` is the root
    /// capability of.
    fn waiter(&self, cap: CapId) -> Option<ObjId> {
        let object = self.rooted(cap)?;
        match self.objects[&object].kind {
            // A remote object has no queue on its holder.
            ObjectKind::Endpoint => self.endpoints.get(&object)?.queue.front().copied(),
            ObjectKind::Notify => self.notifies.get(&object)?.waiters.front().copied(),
            _ => None,
        }
    }

    /// What else destruction undoes (D-110): undoes one link to an object
    /// whose root capability is the next to remove, most recently made
    /// first, and says whether there was one.
    fn undo(&mut self, object: ObjId) -> Undone {
        let last = |list: &Vec<ObjId>| list.last().copied();
        let kind = self.objects[&object].kind;
        // A remote object has no links on its holder: its state is on the
        // granting node.
        if self.remotes.contains_key(&object) {
            return Undone::Nothing;
        }
        match kind {
            // The threads bound to a `cspace`, an `aspace` or an endpoint (as
            // their fault endpoint) are unbound.
            ObjectKind::CSpace => match last(&self.cspaces[&object].threads) {
                Some(thread) => self.bind_space(thread, None),
                None => return Undone::Nothing,
            },
            // Destruction of an address space: first the record of each
            // capability in its list is emptied, most recently mapped first,
            // then each thread is unbound.
            // Its first step invalidates every translation tagged for it on
            // every core that loaded the tag, by the request of its core when
            // that takes acknowledgements; the step returns `again` until the
            // request is complete. A record still unmapping by an earlier
            // unmap is waited for.
            ObjectKind::ASpace => {
                // The invalidation is not one of the 32 units of a step.
                if !self.aspaces[&object].flushed {
                    if self.request_free().is_err() {
                        return Undone::Wait;
                    }
                    let flush = crate::cores::Flush::Tag(object);
                    let completes = crate::cores::Completes::Destroy(object);
                    match self.start_request(flush, completes, None) {
                        crate::cores::Started::Complete => {
                            live(&mut self.aspaces, object).flushed = true;
                        }
                        _ => return Undone::Wait,
                    }
                }
                let aspace = &self.aspaces[&object];
                match (aspace.records.last().copied(), last(&aspace.threads)) {
                    (Some(cap), _) => match self.caps[&cap].record {
                        Record::Unmapping(_) => return Undone::Wait,
                        _ => self.empty_mapping(cap),
                    },
                    (None, Some(thread)) => self.bind_aspace(thread, None),
                    (None, None) => return Undone::Nothing,
                }
            }
            // A thread: first its place on a core, if it runs on another
            // core of its node; that core is asked to leave it, and the step
            // returns `again` until it has; the step "sets the thread's
            // suspended flag, so that its core does not choose it again once
            // it has left" (A node of several cores, gap 14).
            ObjectKind::Thread if self.running_core(object).is_some_and(|c| c != *self.at) => {
                live(&mut self.threads, object).suspended = true;
                self.mark_leaving(object);
                return Undone::Wait;
            }
            ObjectKind::Thread if self.threads[&object].leaving => return Undone::Wait,
            // A table untyped whose live pieces still belong to a surviving
            // space (its root table, its lower tables) waits for their
            // destruction: the step returns `again` (`memory_control` and
            // its calls, destruction of a block; D-154). The pieces lost
            // behind a failed unit are waited for by no one.
            ObjectKind::Untyped
                if self.untypeds[&object].table
                    && self.untypeds[&object].live > self.untypeds[&object].lost =>
            {
                return Undone::Wait;
            }
            // A block of a run: its map counts must be 0 (the step returns
            // `again` while translations are removed), then the run is
            // unmapped from the kernel's own space and that invalidated on
            // every core of the node, idle ones included, before any word is
            // written (G5).
            ObjectKind::Untyped if self.untypeds[&object].block.is_some() => {
                if self.mapped_block(object) {
                    return Undone::Wait;
                }
                match self.unmap_block(object) {
                    Some(true) => return Undone::Unit,
                    Some(false) => return Undone::Wait,
                    None => return Undone::Nothing,
                }
            }
            ObjectKind::Endpoint => match last(&self.endpoints[&object].threads) {
                Some(thread) => self.bind_fault(thread, None),
                None => return Undone::Nothing,
            },
            // Destroying a context clears its lends from the top of the
            // chain down.
            ObjectKind::Sched if self.clear_lend(object) => {}
            // Destroying a notify: its armed timers are disarmed, then its
            // bound irqs unbound, then its watch slots cleared (Cores), then
            // the grants that name it as a doorbell leave its list, one unit
            // each.
            ObjectKind::Notify
                if self.disarm_one(object)
                    || self.unbind_one_irq(object)
                    || self.unwatch_one(object)
                    || self.undoorbell_one(object) => {}
            // Destroying a task detaches its threads; if the task was
            // revoked, wait until any thread marked leaving (or still
            // running on another core) has left (same rule as `task_revoke`,
            // v2-B B-1).
            ObjectKind::Task
                if self.tasks[&object].revoked
                    && self.tasks[&object].threads.iter().any(|t| {
                        self.threads[t].leaving
                            || self.running_core(*t).is_some_and(|c| c != *self.at)
                    }) =>
            {
                return Undone::Wait;
            }
            ObjectKind::Task if self.detach_one(object) => {}
            ObjectKind::Irq => return self.irq_removal(object),
            // IOMMU: the destruction of an iospace first detaches each of its
            // devices, most recently attached first, then empties each record
            // of its list, most recently mapped first.
            // Once its devices are detached, it serves none, and its records
            // are emptied with their counts lowered once the detaches are
            // confirmed; a unit that failed keeps the counts.
            ObjectKind::IoSpace => {
                // While an attach to the iospace is in flight, the step
                // waits; the device its answer brings into the header is
                // detached by a later step (D-139).
                let root = self.aspaces[&object].root.unwrap_or(0);
                if self.attaching(root).next().is_some() {
                    return Undone::Wait;
                }
                // The most recently attached device whose detach is not
                // yet in flight; a device already *detaching* is not asked
                // again (one request at a time per device, D-139).
                let header = &self.iospaces[&object];
                let device = header
                    .iter()
                    .rev()
                    .find(|(iommu, device)| !self.iommus[iommu].detaching.contains(device))
                    .copied();
                if device.is_none() && !header.is_empty() {
                    return Undone::Wait;
                }
                // Once no device is left to detach, a detach still in flight
                // on a unit of the iospace holds the records and the root
                // (D-137, D-139).
                let units = self.aspaces[&object].units.clone();
                let roles: Vec<usize> = units.iter().map(|&u| self.unit_role(u)).collect();
                let pending = self.unit_requests.iter().flatten().any(|e| {
                    roles.contains(&e.role) && matches!(e.op, crate::rings::UnitOp::Detach { .. })
                });
                if device.is_none() && pending {
                    return Undone::Wait;
                }
                match (device, self.aspaces[&object].records.last().copied()) {
                    (Some((iommu, device)), _) => {
                        if self.detach(iommu, device).is_err() {
                            return Undone::Wait;
                        }
                    }
                    (None, Some(cap)) => match self.caps[&cap].record {
                        Record::Unmapping(_) => return Undone::Wait,
                        _ if roles.iter().any(|&r| self.unit_failed(r)) => {
                            self.clear_mapping(cap);
                            self.freeze_unmapping(cap);
                        }
                        _ => self.empty_mapping(cap),
                    },
                    // A detach is a unit request: the root is not removed
                    // while one is in flight, so the attach words of its
                    // devices read 0 (or the unit failed) before the root
                    // can turn back into data (D-137).
                    (None, None) => return Undone::Nothing,
                }
            }
            _ => return Undone::Nothing,
        }
        Undone::Unit
    }

    /// Removes a capability that has no descendant, and empties its slot.
    pub(crate) fn remove_leaf(&mut self, id: CapId) {
        self.remove_cap(id, Slot::Empty);
    }

    /// A remote capability whose grant is no longer valid: its slot becomes
    /// dead. One with descendants of its own stays, `gone` at each use.
    pub(crate) fn remove_leaf_dead(&mut self, id: CapId) {
        if self.caps[&id].children.is_empty() {
            self.remove_cap(id, Slot::Dead);
        }
    }

    /// One step of the removal of a capability and its descendants, as
    /// `cap_delete` makes it, for the holder of a revoking grant.
    pub(crate) fn remove_root(&mut self, cap: CapId) -> Outcome {
        self.remove(cap, Removal::DescendantsThenSelf)
    }

    /// Removes a capability that has no descendant, and leaves `left` in
    /// its slot. The derivation tree: the object is destroyed with its root
    /// capability. Removing a frame capability with a full record undoes
    /// the mapping or the binding it holds, in the same unit (D-110).
    fn remove_cap(&mut self, id: CapId, left: Slot) {
        match self.caps[&id].record {
            Record::Binding(thread) => self.bind_page(thread, None),
            Record::Mapping(_) => self.empty_mapping(id),
            Record::Unmapping(_) | Record::Empty => {}
        }
        let cap = self.caps.remove(&id).expect("names a live entry");
        if let Some(parent) = cap.parent {
            live(&mut self.caps, parent)
                .children
                .retain(|&child| child != id);
        }
        *self.slot(cap.place) = left;
        if self.objects[&cap.object].root == id {
            self.destroy(cap.object);
        }
    }

    /// Destroys an object on which no thread waits. Its memory goes back to
    /// the untyped it came from.
    fn destroy(&mut self, id: ObjId) {
        let object = self.objects.remove(&id).expect("names a live entry");
        // A remote object holds no state of its kind here: its grant is
        // released.
        if self.remotes.contains_key(&id) {
            self.frames.remove(&id);
            return self.remote_destroyed(id);
        }
        // Whether a table untyped is destroyed with pieces lost behind a
        // failed unit.
        let mut held = false;
        match object.kind {
            // Destruction of a block: its words become free (D-114).
            ObjectKind::Untyped => {
                self.free_block(id, crate::blocks::Freed::Destroyed);
                let untyped = self.untypeds.remove(&id);
                held = untyped.is_some_and(|untyped| untyped.lost > 0);
            }
            ObjectKind::CSpace => {
                // Slots and addresses: its segments die with it. The one
                // given at start-up took its segment from no untyped.
                let space = self.cspaces.remove(&id).expect("names a live entry");
                let segments = space.slots.len() / CSPACE_SEGMENT_SLOTS as usize;
                for untyped in object.from.iter().flat_map(|from| [*from].repeat(segments)) {
                    self.release(untyped);
                }
            }
            ObjectKind::Thread => {
                // Threads: a waiting thread leaves its queue when it is
                // destroyed. IPC: when a caller disappears the reply object
                // becomes idle. Bindings: each of its bindings is undone.
                self.unlink(id);
                self.thread_unscheduled(id);
                self.detach_task(id);
                self.unbind_all(id);
                self.threads.remove(&id);
            }
            ObjectKind::Frame => {
                self.frames.remove(&id);
            }
            // Destruction of an address space: its own memory and all its
            // tables, hooked or free, stop counting as live pieces of the
            // untyped, in the same step.
            ObjectKind::ASpace | ObjectKind::IoSpace => {
                let space = self.aspaces.remove(&id).expect("names a live entry");
                self.iospaces.remove(&id);
                let domain = self.domains.remove(&id);
                if let (Some(iommu), Some(domain)) = (self.kept.remove(&id), domain) {
                    self.lost_domains.insert((self.iommus[&iommu].unit, domain));
                }
                // G2: the gate turns the table pages back into data, only
                // once the root's count of devices is 0; a detach answered
                // *failed* leaves the attach word *attached*, and the pages
                // keep their class until the machine restarts (D-137).
                let page = self.machine.geometry.page();
                let pinned = space.root.is_some_and(|r| self.table.attached_to(r) > 0);
                for at in space.table_pages().filter(|_| !pinned) {
                    self.table.set_class(at / page, None);
                }
                // A pinned space's root and tables stay live pieces of their
                // table untyped, lost, so its watermark never returns over a
                // root a context entry of a failed unit may still name
                // (D-141; the final review's verification of lot B, finding
                // 3; D-154). The header goes back to its ordinary untyped.
                let pieces = 1 + space.tables as usize;
                match (space.table_untyped, pinned) {
                    (Some(tables), true) => {
                        live(&mut self.untypeds, tables).lost += pieces as u32;
                    }
                    // The root and the lower tables, hooked or free, stop
                    // counting as live pieces of the table untyped in the
                    // same step (D-154).
                    (Some(tables), false) => {
                        for _ in 0..pieces {
                            self.release(tables);
                        }
                    }
                    // The start-up aspace, made from no untyped.
                    (None, _) => {}
                }
            }
            // An `its_table`: its pages turn back into *data* (D-154).
            ObjectKind::ItsTable => {
                if let Some((first, pages)) = self.its_tables.remove(&id) {
                    for page in first..first + pages {
                        self.table.set_class(page, None);
                    }
                }
            }
            // A machine object is destroyed by no removal: removing its root
            // removes its descendants. The node keeps its own list of its
            // devices on the unit.
            ObjectKind::Iommu | ObjectKind::Node | ObjectKind::Role => {}
            ObjectKind::Endpoint => {
                let endpoint = self.endpoints.remove(&id).expect("names a live entry");
                if let Some(notify) = endpoint.notify {
                    live(&mut self.notifies, notify).endpoint = None;
                }
            }
            // The budget follows the call: destroying a reply object takes
            // its lend out of the chain, and marks its receiver.
            ObjectKind::Reply => {
                self.release_lend(id, None);
                self.replies.remove(&id);
            }
            ObjectKind::Sched => self.sched_destroyed(id),
            ObjectKind::Irq => self.irq_destroyed(id),
            ObjectKind::Task => {
                self.tasks.remove(&id);
            }
            ObjectKind::Timer => self.timer_destroyed(id),
            ObjectKind::Notify => {
                let notify = self.notifies.remove(&id).expect("names a live entry");
                if let Some(endpoint) = notify.endpoint {
                    live(&mut self.endpoints, endpoint).notify = None;
                }
            }
            _ => {}
        }
        match object.from {
            // The permanent hold of the pages a failed unit keeps is on the
            // watermark of their table untyped and of its ancestors: a
            // child destroyed with lost pieces stays a live piece of its
            // parent, lost in its turn, so the parent waits for no one and
            // no watermark above returns over those pages (Destruction of
            // an address space, D-137, D-156).
            Some(parent) if held => live(&mut self.untypeds, parent).lost += 1,
            Some(untyped) => self.release(untyped),
            None => {}
        }
    }
}
