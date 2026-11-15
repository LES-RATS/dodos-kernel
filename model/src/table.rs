//! The machine table (Memory and devices between nodes): its fixed part,
//! written once at machine start-up, and its living part, written only
//! through the write gate of a node. One table serves every node of a
//! machine; a node that runs alone holds it.

use dodos_abi::{
    DeviceEntry, Geometry, Governs, Member, MemberState, Owner, Ownership, RoleKind, RoleState,
    RoleWord,
};
use std::collections::{BTreeMap, BTreeSet};

/// The fixed record of a role: its kind, what it governs, and the index of
/// that object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoleRecord {
    pub(crate) kind: RoleKind,
    pub(crate) governs: Governs,
    pub(crate) object: u16,
}

/// The class of a page (The gate over page tables, G2). A page of memory a
/// node holds that is not listed is of class *data*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PageClass {
    /// A table page of an `aspace` or an `iospace`, its root included.
    Table,
}

/// One write of the living part, kept until the end of the event so that
/// the invariants can judge who wrote what (8.2, invariant 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WordWrite {
    pub(crate) chunk: Option<u64>,
    pub(crate) entry: Option<usize>,
    pub(crate) writer: u16,
    pub(crate) before: u64,
    pub(crate) after: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Table {
    // The fixed part.
    pub(crate) geometry: Geometry,
    pub(crate) nodes: u16,
    /// The floor of each node, as ranges [start, end).
    pub(crate) floors: Vec<Vec<(u64, u64)>>,
    /// The ranges of each reserve.
    pub(crate) reserves: Vec<Vec<(u64, u64)>>,
    pub(crate) role_records: Vec<RoleRecord>,
    /// The global read-only set: the kept ranges, the kept image, the crash
    /// areas and the feedback tables (G, class 3).
    pub(crate) kept: Vec<(u64, u64)>,
    /// The chunks whose word is *none*, fixed at machine start-up (8.2,
    /// invariant 5).
    pub(crate) none: BTreeSet<u64>,
    /// The fence class of the core of each node.
    pub(crate) fence: Vec<u8>,
    // The living part.
    /// The ownership words, one per chunk, as raw words: a stray write is a
    /// word that fails its check.
    pub(crate) owners: Vec<u64>,
    /// The map count of each chunk (G4).
    pub(crate) maps: Vec<u64>,
    /// The page classes, by page number, of the pages not of class *data*.
    pub(crate) classes: BTreeMap<u64, PageClass>,
    pub(crate) devices: Vec<DeviceEntry>,
    pub(crate) members: Vec<Member>,
    pub(crate) roles: Vec<RoleWord>,
    /// The writes of the current event.
    pub(crate) writes: Vec<WordWrite>,
}

impl Table {
    pub(crate) fn member(&self, node: u16) -> Member {
        self.members[node as usize]
    }

    pub(crate) fn generation(&self, node: u16) -> u32 {
        self.member(node).generation()
    }

    /// Whether a node's member word is dead.
    pub(crate) fn dead(&self, node: u16) -> bool {
        self.member(node).state() == MemberState::Dead
    }

    /// Whether `node` holds the role of reserve `j`.
    pub(crate) fn holds_reserve(&self, node: u16, j: u16) -> bool {
        self.role_records
            .iter()
            .zip(&self.roles)
            .any(|(record, word)| {
                record.kind == RoleKind::Reserve
                    && record.object == j
                    && word.state() == RoleState::Held
                    && word.holder() == node
            })
    }

    /// Who may take a word (rules 1 to 4 of taking), for `node`.
    pub(crate) fn may_take(&self, word: Ownership, node: u16) -> bool {
        let dead = match word.owner() {
            Owner::Node(owner) => owner < self.nodes && self.dead(owner),
            _ => false,
        };
        let holds = match word.owner() {
            Owner::Reserve(j) => self.holds_reserve(node, j),
            _ => false,
        };
        word.may_take(node, self.generation(node), dead, holds)
    }

    /// The write gate: writes an ownership word by compare-and-swap, after
    /// reading the writer's own member word. Returns false, writing
    /// nothing, if the writer's gate refuses or the word changed.
    pub(crate) fn write_owner(
        &mut self,
        writer: u16,
        chunk: u64,
        before: u64,
        after: Ownership,
    ) -> bool {
        if !self.member(writer).writes() || self.owners[chunk as usize] != before {
            return false;
        }
        self.owners[chunk as usize] = after.raw();
        self.writes.push(WordWrite {
            chunk: Some(chunk),
            entry: None,
            writer,
            before,
            after: after.raw(),
        });
        true
    }

    /// The write gate on the ownership word of a device entry.
    pub(crate) fn write_entry(&mut self, writer: u16, index: usize, entry: DeviceEntry) -> bool {
        if !self.member(writer).writes() {
            return false;
        }
        let before = self.devices[index].owner.raw();
        self.devices[index] = entry;
        self.writes.push(WordWrite {
            chunk: None,
            entry: Some(index),
            writer,
            before,
            after: entry.owner.raw(),
        });
        true
    }
}
