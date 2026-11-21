//! Grants between nodes, from the table of "Gate words" (D-152), "The grant
//! entry and the use entry": the granter's grant entries, the grantee's
//! use entries and their counts, a grantee's leaf toward a granted page,
//! and R6's proof at the granter's death.

use super::*;

/// The state of a grant entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantState {
    Live,
    Revoking,
    Pinned,
}

/// A grant entry of the granting node (living part).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub(crate) page: Page,
    pub(crate) grantee: u16,
    /// The grantee's generation when granted.
    pub(crate) generation: u32,
    pub(crate) serial: u64,
    pub(crate) state: GrantState,
    /// The grantee's use index, once answered.
    pub(crate) use_index: Option<usize>,
    /// Read by the check only: the grant was made while its grantee was
    /// dead, a generation already over (the gate does not read the
    /// grantee's member word; only a faulty kernel asks for it), so no R6
    /// of the grantee is left to free it.
    pub(crate) to_dead: bool,
}

/// A use entry of the holding node (living part).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Use {
    pub(crate) granter: u16,
    pub(crate) grant: usize,
    pub(crate) serial: u64,
    pub(crate) generation: u32,
    /// The leaves of the holder's tables under the grant.
    pub(crate) count: u64,
}

/// A grantee's leaf toward a granted page: (holder, root, va).
pub(crate) type RemoteLeaf = (u16, Page, (usize, usize));
/// What it is counted under: (granter, grant, use index).
pub(crate) type RemoteUse = (u16, usize, usize);

/// Grants and uses of the small machine: at most `GRANTS` per node.
pub(crate) const GRANTS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GrantGate {
    pub(crate) grants: Vec<[Option<Grant>; GRANTS]>,
    pub(crate) serials: Vec<[u64; GRANTS]>,
    pub(crate) uses: Vec<[Option<Use>; GRANTS]>,
    /// A grantee's leaves toward granted pages: (holder, root, va) → (granter,
    /// grant, use index).
    pub(crate) remote: BTreeMap<RemoteLeaf, RemoteUse>,
}

impl GrantGate {
    pub(crate) fn new(nodes: u16) -> Self {
        Self {
            grants: vec![[None; GRANTS]; usize::from(nodes)],
            serials: vec![[0; GRANTS]; usize::from(nodes)],
            uses: vec![[None; GRANTS]; usize::from(nodes)],
            remote: BTreeMap::new(),
        }
    }
}

impl Machine {
    /// grant *free* → *live*: the granter's gate, `node_send`; the anchor's
    /// frame over memory it holds; the grant count + 1.
    pub(crate) fn grant(&mut self, n: u16, slot: usize, page: Page, to: u16) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())?;
        require(slot < GRANTS && to != n && usize::from(to) < self.layout.len())?;
        require(self.grants.grants[usize::from(n)][slot].is_none())?;
        require(self.holds(n, page) && !self.giving(n, page))?;
        require(self.gate.classes[page as usize] == Class::Data)?;
        // G6 (D-154): an anchor only over memory whose chunks' gate words
        // have the table bit clear; so a table chunk's grant count is 0.
        require(!self.table_chunk(page) || self.old.anchors_over_table_chunks)?;
        let generation = self.gate.members[usize::from(to)].generation;
        let to_dead = self.gate.members[usize::from(to)].state == NodeState::Dead;
        let serial = self.grants.serials[usize::from(n)][slot];
        self.grants.grants[usize::from(n)][slot] = Some(Grant {
            page,
            grantee: to,
            generation,
            serial,
            state: GrantState::Live,
            use_index: None,
            to_dead,
        });
        self.gate.grant_counts[Self::chunk_of(page) as usize] += 1;
        Ok(())
    }

    /// use *free* → written: the grantee's gate at delivery (the grant, its
    /// serial, the granter's generation); the granter's gate records the
    /// index at the answer, once per serial.
    pub(crate) fn deliver(&mut self, to: u16, index: usize, granter: u16, slot: usize) -> Gated {
        require(self.gate.members[usize::from(to)].state.writes())?;
        require(index < GRANTS && slot < GRANTS && usize::from(granter) < self.layout.len())?;
        require(self.grants.uses[usize::from(to)][index].is_none())?;
        let grant = self.grants.grants[usize::from(granter)][slot].ok_or(Refused::Condition)?;
        require(
            grant.grantee == to && grant.state == GrantState::Live && grant.use_index.is_none(),
        )?;
        require(grant.generation == self.gate.members[usize::from(to)].generation)?;
        self.grants.uses[usize::from(to)][index] = Some(Use {
            granter,
            grant: slot,
            serial: grant.serial,
            generation: self.gate.members[usize::from(granter)].generation,
            count: 0,
        });
        self.grants.grants[usize::from(granter)][slot]
            .as_mut()
            .expect("the grant")
            .use_index = Some(index);
        Ok(())
    }

    /// A grantee's leaf toward a granted page: counted only in a use entry
    /// naming the grant, its serial and the granter's generation, the grant
    /// *live* and *answered*; the use count + 1. The map count of the
    /// page's chunk is the granter's leaves, written by the granter's gate
    /// alone (G4: "a grantee's leaves are counted in its use entry, so each
    /// count has one writer"): the grant count holds the chunk meanwhile.
    pub(crate) fn map_remote(
        &mut self,
        n: u16,
        root: Page,
        tag: usize,
        (e0, e1): (usize, usize),
        index: usize,
    ) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes() && index < GRANTS)?;
        let used = self.grants.uses[usize::from(n)][index].ok_or(Refused::Condition)?;
        let grant =
            self.grants.grants[usize::from(used.granter)][used.grant].ok_or(Refused::Condition)?;
        require(grant.state == GrantState::Live && grant.use_index == Some(index))?;
        require(grant.serial == used.serial && grant.grantee == n)?;
        require(self.gate.members[usize::from(used.granter)].generation == used.generation)?;
        require(e0 < ENTRIES - 1 && e1 < ENTRIES)?;
        require(self.gate.classes.get(root as usize) == Some(&Class::Root(RootState::Live)))?;
        require(self.tag_holds(n, tag, root))?;
        let Entry::Table(table) = self.gate.tables[&root][e0] else {
            return Err(Refused::Condition);
        };
        require(self.gate.tables[&table][e1] == Entry::Absent)?;
        self.gate.tables.get_mut(&table).expect("a table")[e1] = Entry::Leaf {
            page: grant.page,
            write: false,
        };
        self.grants.uses[usize::from(n)][index]
            .as_mut()
            .expect("the use")
            .count += 1;
        self.grants
            .remote
            .insert((n, root, (e0, e1)), (used.granter, used.grant, index));
        Ok(())
    }

    /// *live* → *revoking*: the granter's revocation.
    pub(crate) fn revoke(&mut self, n: u16, slot: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes() && slot < GRANTS)?;
        let grant = self.grants.grants[usize::from(n)][slot]
            .as_mut()
            .ok_or(Refused::Condition)?;
        require(grant.state == GrantState::Live)?;
        grant.state = GrantState::Revoking;
        Ok(())
    }

    /// Whether the use entry a grant records counts no leaf under it, or no
    /// longer names it.
    pub(super) fn use_released(&self, granter: u16, slot: usize, grant: Grant) -> bool {
        let Some(index) = grant.use_index else {
            return true;
        };
        let Some(used) = self.grants.uses[usize::from(grant.grantee)][index] else {
            return true;
        };
        let names = used.granter == granter && used.grant == slot && used.serial == grant.serial;
        !names || used.count == 0
    }

    /// *live*, *revoking* or *pinned* → *free*, by the granter's own call:
    /// its gate writes only while its node does (G13; the grant entry's
    /// cell Fence: "X's gate refuses").
    pub(crate) fn free_grant(&mut self, n: u16, slot: usize) -> Gated {
        require(self.gate.members[usize::from(n)].state.writes())?;
        self.release_grant(n, slot)
    }

    /// *live*, *revoking* or *pinned* → *free*, serial + 1: the granter's
    /// gate (or R6), the use entry at the recorded index counting 0 or no
    /// longer naming the grant; the grant count − 1.
    pub(super) fn release_grant(&mut self, n: u16, slot: usize) -> Gated {
        require(slot < GRANTS)?;
        let grant = self.grants.grants[usize::from(n)][slot].ok_or(Refused::Condition)?;
        require(self.use_released(n, slot, grant))?;
        self.grants.grants[usize::from(n)][slot] = None;
        self.grants.serials[usize::from(n)][slot] += 1;
        let chunk = Self::chunk_of(grant.page) as usize;
        self.gate.grant_counts[chunk] = self.gate.grant_counts[chunk].saturating_sub(1);
        Ok(())
    }
}
