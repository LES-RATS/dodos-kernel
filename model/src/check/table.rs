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
        let mine = |raw: u64| match read(raw) {
            Some((2, owner, g)) if g == generation => {
                owner == node || (0xFFF0..0xFFF8).contains(&owner) && holds(owner - 0xFFF0)
            }
            _ => false,
        };
        for write in table.writes.iter() {
            let before = read(write.before);
            let after = read(write.after);
            let ensure_write = |holds: bool| ensure(holds, "52: write of an ownership word");
            let Some((after_state, after_owner, _)) = after else {
                return ensure_write(false);
            };
            match after_state {
                // A take: the word was free or taken, and the writer may
                // take it (its own, unclaimed, of a dead node, or of a
                // reserve it holds).
                2 => {
                    let Some((state, owner, g)) = before else {
                        return ensure_write(false);
                    };
                    let dead = owner < table.nodes
                        && table.members[owner as usize].state() == dodos_abi::MemberState::Dead;
                    let allowed = match owner {
                        o if o == write.writer => state == 1 || g != generation,
                        0xFFFF => true,
                        o if (0xFFF0..0xFFF8).contains(&o) => holds(o - 0xFFF0),
                        _ => dead,
                    };
                    let named = after_owner == write.writer || after_owner == owner;
                    ensure_write(state != 0 && allowed && named)?;
                }
                // A release or a give: the word was held by the writer; or
                // `memory_split` claims an unused entry that is unclaimed,
                // free with the writer as owner.
                1 => {
                    let unused = write.entry.is_some();
                    let claimed = unused
                        && before.is_some_and(|(state, owner, _)| state == 1 && owner == 0xFFFF)
                        && after_owner == write.writer;
                    ensure_write(mine(write.before) || claimed)?
                }
                _ => ensure_write(false)?,
            }
        }
        // The chunks of the blocks and the leaves of the translations, for
        // invariant 55, judged before the others so that it can fail alone.
        let page = table.geometry.page();
        let mut leaves = vec![0u64; table.owners.len()];
        let mut targets = Vec::new();
        for space in self.aspaces.values() {
            for t in space.translations.values() {
                let frame = self
                    .frames
                    .get(&self.caps[&t.cap].object)
                    .ok_or("58: frame")?;
                let bytes = table.geometry.bytes(t.size).ok_or("58: page size")?;
                let start = frame.base + t.page * bytes;
                targets.push((start, start + bytes, t.rights));
                for c in start >> shift..(start + bytes).div_ceil(1 << shift) {
                    if let Some(count) = leaves.get_mut(c as usize) {
                        *count += 1;
                    }
                }
            }
        }
        let block_chunks: BTreeSet<u64> = self
            .untypeds
            .values()
            .filter(|untyped| untyped.block == Some(Block::Run))
            .flat_map(|untyped| untyped.base >> shift..(untyped.base + untyped.bytes) >> shift)
            .collect();
        for (chunk, &raw) in table.owners.iter().enumerate() {
            if read(raw).is_some_and(|(state, ..)| state == 1) {
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
        // 57: the kernel maps exactly the chunks it holds.
        let held: BTreeSet<u64> = (0..table.owners.len() as u64)
            .filter(|&c| mine(table.owners[c as usize]))
            .collect();
        ensure(held == self.kernel_runs, "57: kernel space and ownership")?;
        // 56: the words none are those of machine start-up.
        let none: BTreeSet<u64> = (0..table.owners.len() as u64)
            .filter(|&c| table.owners[c as usize] == 0)
            .collect();
        ensure(none == table.none, "56: a word changed from or to none")?;
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
        ensure(leaves == table.maps, "58: map count")?;
        // 59: the table pages.
        let tables: BTreeSet<u64> = self
            .aspaces
            .values()
            .flat_map(|space| space.table_pages.iter().map(|at| at / page))
            .collect();
        let classes: BTreeSet<u64> = table.classes.keys().copied().collect();
        ensure(tables == classes, "59: page classes")?;
        Ok(())
    }
}
