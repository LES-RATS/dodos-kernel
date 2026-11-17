//! The invariants of memory between nodes and of the gate over page
//! tables, on the state of one node and the machine table it holds.

use super::{Check, ensure};
use crate::{Block, Model, ObjId};
use dodos_abi::{Ownership, Rights};
use std::collections::{BTreeMap, BTreeSet};

/// An ownership word as the invariants read it: (state, owner code,
/// generation), or `None` if it fails its check. Read from the raw word,
/// not through the model's helpers.
fn read(raw: u64) -> Option<(u64, u16, u32)> {
    Ownership::from_raw(raw).map(|_| (raw & 3, (raw >> 16) as u16, (raw >> 32) as u32))
}

/// Invariant 52: a write of an ownership or identifier word, judged with
/// what the writer's gate read when it wrote (recorded with the write), so
/// that a journal kept over several events is judged event by event.
fn judge_write(write: &crate::table::WordWrite) -> Check {
    let ensure_write = |holds: bool| ensure(holds, "52: write of an ownership word");
    let before = read(write.before);
    let Some((after_state, after_owner, after_g)) = read(write.after) else {
        return ensure_write(false);
    };
    let Some((state, owner, g)) = before else {
        return ensure_write(false);
    };
    let writer = write.writer;
    let reserve = (0xFFF0..0xFFF8).contains(&owner);
    // The word as its writer held it: taken by its generation, or by a
    // reserve whose role it held.
    let held = state == 2 && g == write.generation && (owner == writer || reserve && write.holds);
    match after_state {
        // A take: the word was free or taken, and the rules of taking give
        // it to the writer (its own, unclaimed, of a node that is dead or
        // whose generation in the word is over, or of a reserve it holds),
        // with the writer's current generation.
        2 => {
            let allowed = match owner {
                o if o == writer => state == 1 || g != write.generation,
                0xFFFF => true,
                _ if reserve => write.holds,
                _ => write.owner.is_some_and(|m| {
                    m.state() == dodos_abi::MemberState::Dead || state == 2 && m.generation() != g
                }),
            };
            let named = after_owner == writer || after_owner == owner;
            ensure_write(state != 0 && allowed && named && after_g == write.generation)
        }
        // A release or a give: the word was held by the writer; or
        // `memory_split` claims an unused entry that is unclaimed, free with
        // the writer as owner; or R6 frees a reserve word taken by the holder
        // that role names, fenced or dead, with its generation.
        1 => {
            let claimed = matches!(write.word, crate::table::Word::Entry(_))
                && state == 1
                && owner == 0xFFFF
                && after_owner == writer;
            let cleared = state == 2
                && reserve
                && owner == after_owner
                && write.holder.is_some_and(|m| {
                    matches!(
                        m.state(),
                        dodos_abi::MemberState::Fenced | dodos_abi::MemberState::Dead
                    ) && m.generation() == g
                });
            // An identifier word is detached to free, unclaimed, and so is a
            // line word, also by R6 when the generation that took it is over
            // (Interrupts, D-135).
            let word = write.word;
            let identifier = matches!(
                word,
                crate::table::Word::Identifier(_) | crate::table::Word::Line(_)
            );
            let line_cleared = matches!(word, crate::table::Word::Line(_))
                && state == 2
                && after_owner == 0xFFFF
                && write.owner.is_some_and(|m| {
                    matches!(
                        m.state(),
                        dodos_abi::MemberState::Fenced | dodos_abi::MemberState::Dead
                    ) && m.generation() == g
                });
            let freed = held && (!identifier || after_owner == 0xFFFF);
            ensure_write(freed || claimed || cleared || line_cleared)
        }
        _ => ensure_write(false),
    }
}

impl Model {
    /// Memory between nodes (8.2) and the gate over page tables:
    /// 52. every ownership word written in the event was written by a node
    ///     allowed to write it: a take of a word the rules of taking give
    ///     it, or a release of a word it held;
    /// 53. no chunk is covered by two blocks;
    /// 54. a block lies on words *taken* by the current generation of this
    ///     node (or of a reserve whose role it holds), or on its device
    ///     entry taken the same way; an untyped of the start-up lies in the
    ///     node's floor;
    /// 55. a *free* word covers no block and no translation;
    /// 56. the words *none* are exactly those of machine start-up;
    /// 57. the node maps a chunk in its kernel space exactly when its word
    ///     reads *taken* by it with its generation;
    /// 58. (G) every translation leads to memory the node owns, its floor,
    ///     the global read-only set (never writable), or a device entry it
    ///     owns, and never to a table page; every map count equals the
    ///     leaves that target its chunk;
    /// 59. the pages of class *table* are exactly the root tables and the
    ///     lower tables of the node's spaces.
    /// The leaves of this node's translations that target each chunk, as
    /// invariant 58 counts them (memory under a grant counts in its use
    /// entry instead).
    pub(crate) fn chunk_leaves(&self) -> Vec<u64> {
        self.leaves_by_target().0
    }

    /// The leaves of this node's translations that target each entry of
    /// the device table (G4), as invariant 58 counts them.
    pub(crate) fn entry_leaves(&self) -> Vec<u64> {
        self.leaves_by_target().1
    }

    /// The leaves of ordinary memory by chunk, and of device memory by
    /// entry of the device table; a leaf of the global read-only set counts
    /// nowhere (G2, G4), and a leaf under a grant in its use entry (G6).
    fn leaves_by_target(&self) -> (Vec<u64>, Vec<u64>) {
        let table = &self.table;
        let shift = table.geometry.chunk_shift();
        let mut leaves = vec![0u64; table.owners.len()];
        let mut entries = vec![0u64; table.devices.len()];
        for space in self.aspaces.values() {
            for t in space.translations.values() {
                let object = self.caps[&t.cap].object;
                if self.remotes.contains_key(&object) {
                    continue;
                }
                let (Some(&frame), Some(bytes)) =
                    (self.frames.get(&object), table.geometry.bytes(t.size))
                else {
                    continue;
                };
                // Where the page lies, read from the tables, not from the
                // gate's helpers: device memory is a used entry of the
                // device table, the global read-only set a kept range.
                let start = frame.base + t.page * bytes;
                let end = start + bytes;
                if frame.memory != dodos_abi::MemoryType::WriteBack {
                    let entry = table.devices.iter().position(|entry| {
                        let base = entry.base();
                        entry.is_used() && base <= start && end <= base + (1 << entry.log2())
                    });
                    if let Some(entry) = entry {
                        entries[entry] += 1;
                    }
                    continue;
                }
                if frame.kept || table.kept.iter().any(|&(s, e)| s <= start && end <= e) {
                    continue;
                }
                for c in start >> shift..end.div_ceil(1 << shift) {
                    if let Some(count) = leaves.get_mut(c as usize) {
                        *count += 1;
                    }
                }
            }
        }
        // A record *unmapping* counts its leaves until its invalidation is
        // complete, and a failed unit's are kept for ever (D-131).
        for cap in self.caps.values() {
            let crate::Record::Unmapping(m) = cap.record else {
                continue;
            };
            if self.remotes.contains_key(&cap.object) {
                continue;
            }
            let Some(&frame) = self.frames.get(&cap.object) else {
                continue;
            };
            let Some(bytes) = table.geometry.bytes(frame.page) else {
                continue;
            };
            for page in m.first..m.first + m.count {
                let start = frame.base + page * bytes;
                let end = start + bytes;
                if frame.memory != dodos_abi::MemoryType::WriteBack {
                    let entry = table.devices.iter().position(|entry| {
                        let base = entry.base();
                        entry.is_used() && base <= start && end <= base + (1 << entry.log2())
                    });
                    if let Some(entry) = entry {
                        entries[entry] += 1;
                    }
                    continue;
                }
                if frame.kept || table.kept.iter().any(|&(s, e)| s <= start && end <= e) {
                    continue;
                }
                for c in start >> shift..end.div_ceil(1 << shift) {
                    if let Some(count) = leaves.get_mut(c as usize) {
                        *count += 1;
                    }
                }
            }
        }
        for (&chunk, &frozen) in &self.frozen {
            if let Some(count) = leaves.get_mut(chunk as usize) {
                *count += frozen;
            }
        }
        for (&entry, &frozen) in &self.frozen_entries {
            if let Some(count) = entries.get_mut(entry) {
                *count += frozen;
            }
        }
        (leaves, entries)
    }

    pub(super) fn check_table(&self) -> Check {
        let table = &self.table;
        let shift = table.geometry.chunk_shift();
        let node = self.node;
        let generation = table.members[node as usize].generation();
        let holds = |j: u16| {
            table
                .role_records
                .iter()
                .zip(&table.roles)
                .any(|(record, word)| {
                    record.kind == dodos_abi::RoleKind::Reserve
                        && record.object == j
                        && word.state() == dodos_abi::RoleState::Held
                        && word.holder() == node
                })
        };
        let mine_read = |state: u64, owner: u16, g: u32| {
            state == 2
                && g == generation
                && (owner == node || (0xFFF0..0xFFF8).contains(&owner) && holds(owner - 0xFFF0))
        };
        let mine = |raw: u64| read(raw).is_some_and(|(state, owner, g)| mine_read(state, owner, g));
        table.checked.set(true);
        for write in table.writes.iter() {
            judge_write(write)?;
        }
        // The chunks of the blocks and the leaves of the translations, for
        // invariant 55, judged before the others so that it can fail alone.
        let page = table.geometry.page();
        let mut leaves = vec![0u64; table.owners.len()];
        let mut targets = Vec::new();
        let mut granted: BTreeMap<u16, u64> = BTreeMap::new();
        for space in self.aspaces.values() {
            for t in space.translations.values() {
                let object = self.caps[&t.cap].object;
                let frame = self.frames.get(&object).ok_or("58: frame")?;
                let bytes = table.geometry.bytes(t.size).ok_or("58: page size")?;
                let start = frame.base + t.page * bytes;
                // Memory granted to the node and not revoked (G, class 2):
                // its leaves count in the use entry of the grant (G6).
                if let Some(remote) = self.remotes.get(&object) {
                    let entry = table.grants[remote.node as usize].get(&remote.grant);
                    let granted_here =
                        entry.is_some_and(|e| e.serial == remote.serial && e.grantee == node);
                    ensure(granted_here, "58: translation under no grant")?;
                    *granted.entry(remote.use_entry).or_default() += 1;
                    continue;
                }
                targets.push((start, start + bytes, t.rights));
                for c in start >> shift..(start + bytes).div_ceil(1 << shift) {
                    if let Some(count) = leaves.get_mut(c as usize) {
                        *count += 1;
                    }
                }
            }
        }
        // A record *unmapping* still counts its leaves until its
        // invalidation is complete (G4, G6, D-131).
        for cap in self.caps.values() {
            if let crate::Record::Unmapping(m) = cap.record
                && let Some(remote) = self.remotes.get(&cap.object)
            {
                *granted.entry(remote.use_entry).or_default() += m.count;
            }
        }
        let block_chunks: BTreeSet<u64> = self
            .untypeds
            .values()
            .filter(|untyped| untyped.block == Some(Block::Run))
            .flat_map(|untyped| untyped.base >> shift..(untyped.base + untyped.bytes) >> shift)
            .collect();
        // One pass over the words: 55 on each free word; the counts of the
        // words this node holds (57) and of the words none (56), judged
        // below against the kernel's runs and the words of start-up.
        let (mut held, mut zero) = (0, 0);
        for (chunk, &raw) in table.owners.iter().enumerate() {
            zero += usize::from(raw == 0);
            let word = read(raw);
            held += usize::from(word.is_some_and(|(state, owner, g)| mine_read(state, owner, g)));
            if word.is_some_and(|(state, ..)| state == 1) {
                let chunk = chunk as u64;
                ensure(!block_chunks.contains(&chunk), "55: block on a free word")?;
                ensure(
                    leaves[chunk as usize] == 0,
                    "55: translation to a free word",
                )?;
            }
        }
        // The blocks of this node, as chunks or entries.
        let mut covered: BTreeMap<u64, ObjId> = BTreeMap::new();
        let mut entries: BTreeMap<usize, ObjId> = BTreeMap::new();
        for (&id, untyped) in &self.untypeds {
            match untyped.block {
                Some(Block::Run) => {
                    for chunk in untyped.base >> shift..(untyped.base + untyped.bytes) >> shift {
                        ensure(
                            covered.insert(chunk, id).is_none(),
                            "53: two blocks on a chunk",
                        )?;
                        ensure(
                            mine(table.owners[chunk as usize]),
                            "54: block on a word not taken",
                        )?;
                    }
                }
                Some(Block::Entry(index)) => {
                    ensure(
                        entries.insert(index, id).is_none(),
                        "53: two blocks on an entry",
                    )?;
                    let entry = table.devices.get(index).ok_or("54: block of no entry")?;
                    let range = (entry.base(), entry.base() + (1 << entry.log2()));
                    let same = range == (untyped.base, untyped.base + untyped.bytes);
                    ensure(
                        same && mine(entry.owner.raw()),
                        "54: block on an entry not taken",
                    )?;
                }
                None if self.objects.get(&id).is_some_and(|o| o.from.is_none()) => {
                    let (start, end) = (untyped.base, untyped.base + untyped.bytes);
                    let floor = table.floors[node as usize]
                        .iter()
                        .any(|&(s, e)| s <= start && end <= e);
                    ensure(floor, "54: start-up untyped outside the floor")?;
                }
                None => {}
            }
        }
        // 57: the kernel maps exactly the chunks it holds, except a run it
        // gave or destroyed whose words wait for the invalidation of the
        // kernel's own space on its other cores (G5, D-131): no longer
        // mapped, still held until the request completes.
        let word = |c: &u64| table.owners.get(*c as usize).copied();
        let runs = self.kernel_runs.iter().all(|c| word(c).is_some_and(mine));
        let waiting: BTreeSet<u64> = self
            .cores
            .iter()
            .filter_map(|c| c.request.as_ref())
            .filter_map(|r| match &r.completes {
                crate::cores::Completes::Run(freed) if freed.block == Block::Run => {
                    Some(freed.chunks(shift))
                }
                _ => None,
            })
            .flatten()
            .collect();
        let held_waiting = waiting.iter().all(|c| word(c).is_some_and(mine));
        ensure(
            runs && held_waiting && held == self.kernel_runs.len() + waiting.len(),
            "57: kernel space and ownership",
        )?;
        // 56: the words none are those of machine start-up.
        let none = table.none.iter().all(|c| word(c) == Some(0));
        ensure(
            none && zero == table.none.len(),
            "56: a word changed from or to none",
        )?;
        // 58: every translation, and the map counts.
        let owned_entries: Vec<(u64, u64)> = table
            .devices
            .iter()
            .filter(|entry| entry.is_used() && mine(entry.owner.raw()))
            .map(|entry| (entry.base(), entry.base() + (1 << entry.log2())))
            .collect();
        let device_owned =
            |start: u64, end: u64| owned_entries.iter().any(|&(s, e)| s <= start && end <= e);
        let kept = |start: u64, end: u64| table.kept.iter().any(|&(s, e)| s <= start && end <= e);
        let floor = |start: u64, end: u64| {
            table.floors[node as usize]
                .iter()
                .any(|&(s, e)| s <= start && end <= e)
        };
        for &(start, end, rights) in &targets {
            let chunks = start >> shift..end.div_ceil(1 << shift);
            let owned = chunks
                .clone()
                .all(|c| mine(table.owners.get(c as usize).copied().unwrap_or(0)));
            let writable = owned || floor(start, end) || device_owned(start, end);
            let reached = writable || kept(start, end);
            ensure(reached, "58: translation to memory the node does not own")?;
            let read_only = !rights.contains(Rights::WRITE);
            ensure(
                writable || read_only,
                "58: writable translation to read-only memory",
            )?;
            let table_page =
                (start / page..end.div_ceil(page)).any(|p| table.classes.contains_key(&p));
            ensure(!table_page, "58: translation to a table page")?;
        }
        // Alone, the node's leaves are every leaf; on a machine of several
        // models, the others' count too, and `System::check` sums them. A
        // leaf of device memory counts in its entry, one of the global
        // read-only set nowhere (G2, G4).
        let (chunk_leaves, entry_leaves) = self.leaves_by_target();
        let counts: Vec<u64> = (0..chunk_leaves.len() as u64)
            .map(|c| table.map_count(c))
            .collect();
        let counted = match self.shared {
            false => chunk_leaves == counts && entry_leaves == *table.device_leaves,
            true => {
                chunk_leaves.iter().zip(&counts).all(|(l, m)| l <= m)
                    && entry_leaves
                        .iter()
                        .zip(table.device_leaves.iter())
                        .all(|(l, m)| l <= m)
            }
        };
        ensure(counted, "58: map count")?;
        for (&entry, used) in &table.uses[node as usize] {
            let counted = granted.get(&entry).copied().unwrap_or(0);
            ensure(used.leaves == counted, "58: leaves of a use entry")?;
        }
        // 59: the table pages, with their classes: the root of a space of
        // the class of its top level (an iospace's, iospace table), each
        // hooked table of the class of its level, a free one of a table
        // class; and each gate word counts the pages of its chunk not of
        // class data.
        let mut tables: BTreeMap<u64, Option<dodos_abi::PageClass>> = BTreeMap::new();
        for (&id, space) in &self.aspaces {
            let io = self.iospaces.contains_key(&id);
            let class = |level: u32| match io {
                true => Some(dodos_abi::PageClass::IOSPACE_TABLE),
                false => dodos_abi::PageClass::table(level as u8),
            };
            if let Some(root) = space.root {
                let top = match io {
                    true => 0,
                    false => table.geometry.levels,
                };
                tables.insert(root / page, class(top));
            }
            for (&(level, _), &at) in &space.hooked {
                tables.insert(at / page, class(level));
            }
            for &at in &space.free {
                tables.insert(at / page, None);
            }
            let taken = space.hooked.len() + space.free.len();
            let followed = space.root.is_none() || space.tables == taken as u64;
            ensure(followed, "59: tables of a space")?;
        }
        let classes_right = tables.iter().all(|(page, class)| {
            let stored = table.classes.get(page);
            match class {
                Some(class) => stored == Some(class),
                None => stored
                    .is_some_and(|c| *c == dodos_abi::PageClass::IOSPACE_TABLE || c.raw() > 16),
            }
        });
        ensure(classes_right, "59: page classes")?;
        let stored_only = table
            .classes
            .values()
            .all(|c| c.raw() != 0 && dodos_abi::PageClass::stored(c.raw()).is_some());
        ensure(stored_only, "59: page classes")?;
        let pages = table.geometry.chunk() / page;
        let mut stored: BTreeMap<u64, u64> = BTreeMap::new();
        for &at in table.classes.keys() {
            *stored.entry(at / pages).or_default() += 1;
        }
        for (chunk, &raw) in table.gate_words.iter().enumerate() {
            let counted = stored.get(&(chunk as u64)).copied().unwrap_or(0);
            ensure(
                raw >> 32 == counted,
                "59: pages not of class data in a gate word",
            )?;
        }
        let tables: BTreeSet<u64> = tables.into_keys().collect();
        let classes: BTreeSet<u64> = table.classes.keys().copied().collect();
        // Alone, the node's tables are every table page; on a machine of
        // several models, `System::check` takes the union.
        let held = match self.shared {
            false => tables == classes,
            true => tables.is_subset(&classes),
        };
        ensure(held, "59: page classes")?;
        Ok(())
    }
}
