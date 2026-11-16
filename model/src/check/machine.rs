//! The invariants of a machine of several nodes (`System::check`), each
//! named after the sentence of the contract it checks: those of every
//! node's kernel, then those that only the whole machine shows.

use crate::rings::Completes;
use crate::system::Hardware;
use crate::table::{GrantState, RingEntry};
use crate::{Model, System};
use dodos_abi::{CoreState, CoreWord, MemberState, RING_ANSWER_RESERVE, RING_KERNEL_ENTRIES};

type Check = Result<(), String>;

fn ensure(holds: bool, invariant: &str) -> Check {
    if holds {
        Ok(())
    } else {
        Err(invariant.to_string())
    }
}

impl System {
    /// Every invariant: each node's (`Model::check`), then the machine's.
    pub fn check(&self) -> Check {
        // R-1 of the second verification: a park request only grows, so
        // that a confirmation of an earlier request is never taken for one
        // of a later stop.
        ensure(!self.park_went_back, "fence: a park request went back")?;
        // A node that is accused runs nothing, and its memory is being
        // reclaimed: its kernel's state is not judged against the table.
        for (slot, model) in self.models().filter(|(slot, _)| self.running_node(*slot)) {
            model.check().map_err(|e| format!("node {slot}: {e}"))?;
        }
        if self.models().next().is_none() {
            return Ok(());
        }
        self.check_cores()?;
        self.check_counts()?;
        self.check_grants()?;
        self.check_rings()?;
        self.check_units()
    }

    /// Node slots and core words: "a core executes node N only while its
    /// word names N's current generation", "no core executes a dead node";
    /// a core word never reads *leaving* (no rule writes it) nor fails its
    /// check; a free word names no node.
    fn check_cores(&self) -> Check {
        let table = self.canonical();
        for core in 0..table.core_words.len() {
            let word = CoreWord::from_raw(table.core_words[core]);
            ensure(word.is_some(), "core word: a word that fails its check")?;
            let word = word.expect("checked");
            ensure(
                word.state() != CoreState::Leaving,
                "core word: leaving is written",
            )?;
            if word.state() == CoreState::Free {
                ensure(
                    word.slot() == dodos_abi::CORE_NO_SLOT && word.generation() == 0,
                    "core word: a free word names a node",
                )?;
            }
            // The fence: "the driver raises p's park request, then marks
            // p's word, then stops p": a stop is never in flight under a
            // word that is not marked (the first verification's Test 4: a
            // stop under an unmarked word landed after another node had
            // taken the core), and a marked word has its park request
            // raised.
            let stopping = self
                .actions()
                .iter()
                .any(|(_, a)| *a == crate::nodes::HwAction::Stop { core });
            ensure(
                !stopping || word.is_marked(),
                "fence: a stop in flight under a word that is not marked",
            )?;
            ensure(
                !word.is_marked() || table.park_requests[core] != 0,
                "fence: a word marked before its park request was raised",
            )?;
            if self.hardware(core) != Hardware::Running {
                continue;
            }
            let (slot, generation) = (word.slot(), word.generation());
            let named = matches!(word.state(), CoreState::Owned | CoreState::Joining);
            ensure(named, "P2: a core executes a node its word does not name")?;
            let member = table.member(slot);
            ensure(
                member.generation() == generation && member.state() != MemberState::Dead,
                "P3: a core executes a dead node or a generation that is over",
            )?;
        }
        Ok(())
    }

    /// G4: "the map count: the number of leaves of the owner's tables ...
    /// together with the leaves cleared from them whose invalidation is not
    /// yet complete": the leaves every kernel holds, summed per chunk, are
    /// the count; a chunk whose owner's kernel is gone (its generation
    /// over, before R6) keeps at least what is left.
    fn check_counts(&self) -> Check {
        let table = self.canonical();
        let mut leaves = vec![0u64; table.owners.len()];
        let mut entries = vec![0u64; table.devices.len()];
        for (_, model) in self.models().filter(|(slot, _)| self.running_node(*slot)) {
            for (c, n) in model.chunk_leaves().into_iter().enumerate() {
                leaves[c] += n;
            }
            for (e, n) in model.entry_leaves().into_iter().enumerate() {
                entries[e] += n;
            }
        }
        for (chunk, &held) in leaves.iter().enumerate() {
            let count = table.map_count(chunk as u64);
            let owner = dodos_abi::Ownership::from_raw(table.owners[chunk]);
            let alive = owner.is_some_and(|w| match w.owner() {
                dodos_abi::Owner::Node(n) => {
                    self.has_generation(n, w.generation()) && self.running_node(n)
                }
                _ => true,
            });
            let counted = if alive { held == count } else { held <= count };
            ensure(counted, "G4: a map count is not the leaves of its chunk")?;
        }
        for (entry, &held) in entries.iter().enumerate() {
            ensure(
                held <= table.device_leaves[entry],
                "G4: a leaf count below its leaves",
            )?;
        }
        Ok(())
    }

    /// G6 and Grants: "a granted frame never returns to an untyped while its
    /// holder may translate to it": a use entry whose node holds leaves
    /// under a grant of a generation not over names a grant entry with its
    /// serial that is used (live, revoking or pinned) and names that node;
    /// "no grant entry stays unanswered with nothing in flight".
    fn check_grants(&self) -> Check {
        let table = self.canonical();
        for (slot, model) in self.models() {
            for used in table.uses[slot as usize].values() {
                let over = table.member(used.granter).generation() != used.generation
                    || matches!(table.member(used.granter).state(), MemberState::Dead);
                if used.leaves == 0 || over || model.leaves_frozen(used) {
                    continue;
                }
                let entry = table.grants[used.granter as usize].get(&used.grant);
                let held = entry.is_some_and(|e| e.serial == used.serial && e.grantee == slot);
                ensure(held, "G6: a grant freed while its holder translates to it")?;
            }
            for (&grant, entry) in &table.grants[slot as usize] {
                if entry.answered || entry.state == GrantState::Pinned {
                    continue;
                }
                let grantee = table.member(entry.grantee);
                let over = grantee.generation() != entry.generation
                    || !matches!(
                        grantee.state(),
                        MemberState::Starting
                            | MemberState::Live
                            | MemberState::Sealing
                            | MemberState::Sealed
                    );
                let sending = model.peers_sending(entry.grantee, grant);
                ensure(
                    over || sending,
                    "LEAK: a grant entry unanswered with nothing in flight",
                )?;
            }
        }
        Ok(())
    }

    /// The rings: no ring overflows ("a producer index" never more than the
    /// ring's entries ahead of the consumer for a correct sender), and the
    /// answers owed on a kernel ring never exceed its reserve; "a credit in
    /// use always has its request or its answer in flight" (or is served
    /// by the holder, or its peer cannot answer: sealed or accused).
    fn check_rings(&self) -> Check {
        let table = self.canonical();
        let n = table.nodes;
        for from in 0..n {
            for to in (0..n).filter(|&t| t != from) {
                let pair = &table.rings[from as usize][to as usize];
                let consumed = table.beacons[to as usize].consumed[from as usize];
                ensure(
                    pair.kernel.produced.wrapping_sub(consumed.0) <= RING_KERNEL_ENTRIES
                        && pair.user.produced.wrapping_sub(consumed.1)
                            <= dodos_abi::RING_USER_ENTRIES,
                    "rings: a ring overflows",
                )?;
            }
        }
        for (x, model) in self.models() {
            for y in (0..n).filter(|&y| y != x) {
                let peer = &model.peers_of(y);
                let owed = peer.iter().flatten().count() as u64;
                ensure(
                    owed <= RING_ANSWER_RESERVE,
                    "RESERVE: answers owed exceed the reserve",
                )?;
                let member = table.member(y);
                let answering = matches!(
                    member.state(),
                    MemberState::Starting | MemberState::Live | MemberState::Sealing
                );
                if !answering || !self.has_generation(y, member.generation()) {
                    continue;
                }
                for (index, credit) in peer.iter().enumerate() {
                    let Some(credit) = credit else { continue };
                    let request = self.in_ring(x, y, index as u8, credit.serial);
                    let answer = self.in_ring(y, x, index as u8, credit.serial);
                    let served = self.served(y, x, index as u8);
                    let kept = matches!(credit.completes, Completes::Unit { .. }) && served;
                    // A `Release` of the grant in flight or pending ends a
                    // revocation too: the `Revoke` it crossed is refused.
                    let released = match credit.completes {
                        Completes::Revoke { grant } => self.released(y, x, grant),
                        _ => false,
                    };
                    ensure(
                        request || answer || kept || released,
                        "CREDIT: a credit in use with neither its request nor its answer in flight",
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Roles: "two nodes never hold a unit role at once": at most one node
    /// that runs maps a unit's registers.
    fn check_units(&self) -> Check {
        let table = self.canonical();
        for unit in table.units.values() {
            let running = unit.mapped.iter().filter(|&&n| table.writes(n)).count();
            ensure(running <= 1, "ONE-HOLDER: two nodes map a unit's registers")?;
        }
        Ok(())
    }

    /// Whether the message with this credit is in the ring `from` → `to`,
    /// not yet consumed.
    fn in_ring(&self, from: u16, to: u16, credit: u8, serial: u32) -> bool {
        let table = self.canonical();
        let pair = &table.rings[from as usize][to as usize];
        let consumed = table.beacons[to as usize].consumed[from as usize];
        [(&pair.kernel, consumed.0), (&pair.user, consumed.1)]
            .into_iter()
            .any(|(ring, start)| {
                (start..ring.produced).any(|at| match ring.at(at) {
                    Some(RingEntry::Message(m)) => m.header.credit() == Some((credit, serial)),
                    _ => false,
                })
            })
    }

    /// Whether the holder `from` has a `Release` of grant `grant` of `to`
    /// in its kernel ring to `to`, or pending in its use entry.
    fn released(&self, from: u16, to: u16, grant: u16) -> bool {
        let table = self.canonical();
        let ring = &table.rings[from as usize][to as usize].kernel;
        let start = table.beacons[to as usize].consumed[from as usize].0;
        let in_ring = (start..ring.produced).any(|at| match ring.at(at) {
            Some(RingEntry::Message(m)) => {
                matches!(m.body, crate::rings::Body::Release { grant: g, .. } if g == grant)
            }
            _ => false,
        });
        let pending = table.uses[from as usize]
            .values()
            .any(|u| u.granter == to && u.grant == grant && u.release_pending);
        in_ring || pending
    }

    /// Whether node `holder` keeps the unit request of `from` with this
    /// credit in its unit service table.
    fn served(&self, holder: u16, from: u16, credit: u8) -> bool {
        self.models()
            .find(|(slot, _)| *slot == holder)
            .is_some_and(|(_, m)| m.serves(from, credit))
    }

    /// Whether a node's kernel runs or is sealed: starting, live, sealing
    /// or sealed with the generation its state was built for.
    fn running_node(&self, node: u16) -> bool {
        let member = self.canonical().member(node);
        matches!(
            member.state(),
            MemberState::Starting | MemberState::Live | MemberState::Sealing | MemberState::Sealed
        ) && self.has_generation(node, member.generation())
    }

    /// Whether node slot `node` has a kernel built for `generation`.
    fn has_generation(&self, node: u16, generation: u32) -> bool {
        self.models()
            .any(|(slot, m)| slot == node && m.generation_built() == generation)
    }
}

impl Model {
    /// The credits of this node toward a peer.
    pub(crate) fn peers_of(&self, peer: u16) -> Vec<Option<crate::rings::Credit>> {
        self.peers[peer as usize].credits.clone()
    }

    /// Whether a send credit toward `peer` covers grant entry `grant`.
    pub(crate) fn peers_sending(&self, peer: u16, grant: u16) -> bool {
        self.peers[peer as usize].credits.iter().flatten().any(
            |c| matches!(&c.completes, Completes::Send { grants, .. } if grants.contains(&grant)),
        )
    }

    /// Whether this node serves the unit request of `from` with a credit.
    pub(crate) fn serves(&self, from: u16, credit: u8) -> bool {
        self.unit_service.contains_key(&(from, credit))
    }

    /// Whether a use entry's leaves are kept by a failed unit (never
    /// lowered, IOMMU).
    pub(crate) fn leaves_frozen(&self, _used: &crate::table::UseEntry) -> bool {
        false
    }
}
