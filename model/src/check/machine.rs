//! The invariants of a machine of several nodes (`System::check`), each
//! named after the sentence of the contract it checks: those of every
//! node's kernel, then those that only the whole machine shows.

use crate::rings::Completes;
use crate::system::Hardware;
use crate::table::{GrantState, RingEntry};
use crate::{Model, System};
use dodos_abi::{CoreState, CoreWord, MemberState, RING_KERNEL_ENTRIES};

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
        // D-147: on ARM64 `parked` is written only at a fence.
        ensure(
            !self.parked_unfenced,
            "PARKED: an ARM64 parked written with no fence mark",
        )?;
        // A node that is accused runs nothing, and its memory is being
        // reclaimed: its kernel's state is not judged against the table.
        // A faulty kernel's own state is not judged: only what it does to
        // the others (the rules below).
        for (slot, model) in self
            .models()
            .filter(|(slot, _)| self.running_node(*slot) && !self.faulty.contains(slot))
        {
            model.check().map_err(|e| format!("node {slot}: {e}"))?;
        }
        if self.models().next().is_none() {
            return Ok(());
        }
        // Only the machine start-up fills the slot of the replacement
        // kernel image; no call writes it (D-136).
        ensure(
            self.canonical().replacement == self.machine.replacement,
            "D-136: the replacement slot written after the machine start-up",
        )?;
        self.check_cores()?;
        self.check_fence_order()?;
        self.check_fence_rounds()?;
        // D-146: no observation is taken of a sealed node.
        let table = self.canonical();
        ensure(
            table
                .beacons
                .iter()
                .flat_map(|b| &b.observations)
                .all(|o| o.state != MemberState::Sealed),
            "OBSERVATION: an observation taken of a sealed node",
        )?;
        self.check_counts()?;
        self.check_grants()?;
        self.check_rings()?;
        self.check_units()?;
        self.check_r6_proof()?;
        self.check_granted()
    }

    /// G4, G5 (D-148): every chunk's grant count is the live, revoking or
    /// pinned grant entries whose anchor touches it, and so is every device
    /// entry's; the memory under such an anchor stays its granter's, taken
    /// by that generation, and of class data, whatever the granter's kernel
    /// does.
    fn check_granted(&self) -> Check {
        let table = self.canonical();
        let mut counts = vec![0u16; table.gate_words.len()];
        let mut devices = vec![0u16; table.devices.len()];
        for (granter, grants) in table.grants.iter().enumerate() {
            for entry in grants.values() {
                if !matches!(
                    entry.state,
                    GrantState::Live | GrantState::Revoking | GrantState::Pinned
                ) {
                    continue;
                }
                let (chunks, device) = table.frame_touches(&entry.anchor.frame);
                for &chunk in &chunks {
                    counts[chunk as usize] += 1;
                    let g = table.generation(granter as u16);
                    let at = chunk << table.geometry.chunk_shift();
                    let reserve = dodos_abi::Ownership::from_raw(table.owners[chunk as usize])
                        .is_some_and(|w| {
                            matches!(w.owner(), dodos_abi::Owner::Reserve(j)
                                if table.holds_reserve(granter as u16, j))
                        });
                    let kept = table.holds_page(at.max(entry.anchor.frame.base), granter as u16, g)
                        || reserve
                        || table.member(granter as u16).state() != MemberState::Live;
                    ensure(kept, "G5: granted memory owned by another than its granter")?;
                }
                if let Some(index) = device {
                    devices[index] += 1;
                }
                let frame = entry.anchor.frame;
                let page = table.geometry.page();
                let bytes = frame.pages << table.geometry.page_shift[frame.page as usize];
                let classed = (frame.base / page..(frame.base + bytes) / page)
                    .any(|p| table.classes.contains_key(&p));
                // Shadowed since D-154 by the gate word's rule (TABLES, in
                // each node's check): a page not *data* lies in a table
                // chunk, whose grant count is 0, so G4's count below would
                // not be the grants over it.
                ensure(
                    !classed,
                    "G5: a page under a grant's anchor not of class data",
                )?;
            }
        }
        let gate: Vec<u16> = (0..table.gate_words.len() as u64)
            .map(|c| table.grant_count(c))
            .collect();
        ensure(
            gate == counts && *table.device_grants == devices,
            "G4: a grant count that is not the grants over its chunk",
        )
    }

    /// Node slots and core words: "a core executes node N only while its
    /// word names N's current generation", "no core executes a dead node";
    /// a core word never reads *leaving* (no rule writes it) nor fails its
    /// check; a free word names no node.
    fn check_cores(&self) -> Check {
        let table = self.canonical();
        // A member word passes its check: the resume bit only in a starting
        // word (D-144).
        for member in &table.members {
            ensure(
                dodos_abi::Member::from_raw(member.raw()).is_some(),
                "member word: a word that fails its check",
            )?;
        }
        for core in 0..table.core_words.len() {
            let word = CoreWord::from_raw(table.core_words[core]);
            ensure(word.is_some(), "core word: a word that fails its check")?;
            let word = word.expect("checked");
            ensure(
                word.state() != CoreState::Leaving,
                "core word: leaving is written",
            )?;
            // A word that names a node names a slot of the table, so that
            // P3 can read its member word (the checker names, it never
            // panics; ver-D, Mistral's note on `check_cores`).
            ensure(
                word.state() == CoreState::Free || (word.slot() as usize) < table.members.len(),
                "core word: names a slot past the table",
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
            // D-143, ARM64 and RISC-V: a core that runs its node's kernel
            // with its park request raised, its word marked (D-146), has the
            // fence's stop or its node's park in flight; a woken core with
            // it raised was parked by the read at its wake (`node_idle`).
            let raised = word.is_marked();
            // D-149: a park its firmware refused is never confirmed by it.
            ensure(
                self.hardware(core) != Hardware::Refused
                    || table.stopped[core] != table.park_requests[core],
                "REFUSED: a core whose park was refused counted confirmed",
            )?;
            // G15 (D-149): an x86-64 core counted confirmed under a marked
            // word is in the park stub, which only a SIPI started; no word
            // its own kernel wrote confirmed it.
            ensure(
                self.machine.arch != crate::Arch::X86_64
                    || !raised
                    || table.core_beacons[core].parked != table.park_requests[core]
                    || matches!(self.hardware(core), Hardware::Stub { .. }),
                "G15: an x86-64 core confirmed while it runs",
            )?;
            let in_flight = self.actions().iter().any(|&(_, a)| {
                a == crate::nodes::HwAction::Stop { core }
                    || a == crate::nodes::HwAction::Park { core }
            });
            ensure(
                self.machine.arch == crate::Arch::X86_64
                    || self.hardware(core) != Hardware::Running
                    || !raised
                    || in_flight,
                "PARKED: a core runs with its park request raised and no stop or park in flight",
            )?;
            // A core on its way to sleep or asleep runs nothing of its node,
            // unless the interrupt that will wake it is pending (a placement
            // from another core and its reschedule interrupt).
            if matches!(
                self.hardware(core),
                Hardware::Suspending { .. } | Hardware::Asleep { .. }
            ) {
                let runs = self.models().any(|(n, m)| {
                    n == word.slot() && m.position(core as u64).is_some_and(|p| m.runs_unwoken(p))
                });
                ensure(!runs, "SLEEP: a core asleep runs a thread of its node")?;
            }
            if self.hardware(core) != Hardware::Running {
                continue;
            }
            // D-149: no node runs on an x86-64 core whose local APIC may keep
            // an in-service bit from its last node.
            ensure(
                !self.in_service.get(core).copied().unwrap_or(false),
                "ISR: a core runs a node with an in-service bit from its last one",
            )?;
            let (slot, generation) = (word.slot(), word.generation());
            let named = matches!(word.state(), CoreState::Owned | CoreState::Joining);
            // Every node whose kernel runs on the core is the one its word
            // names (the ninth model review, finding 14: the first runner
            // found hid a second one).
            let mut runners = self
                .models()
                .filter(|(_, m)| m.position(core as u64).is_some_and(|p| !m.is_silent(p)));
            ensure(
                named && runners.all(|(n, _)| n == slot),
                "P2: a core executes a node its word does not name",
            )?;
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
            // A chunk *none* of a floor is its node's (judged exactly while
            // that node runs); a kept chunk holds no counted leaf (G4).
            let base = chunk as u64 * table.geometry.chunk();
            let floor = table
                .floors
                .iter()
                .position(|f| f.iter().any(|&(start, end)| start <= base && base < end));
            let alive = owner.is_some_and(|w| match (w.state(), w.owner()) {
                (dodos_abi::OwnState::None, _) => floor.is_none_or(|n| self.running_node(n as u16)),
                (_, dodos_abi::Owner::Node(n)) => {
                    self.has_generation(n, w.generation()) && self.running_node(n)
                }
                _ => true,
            });
            let counted = if alive { held == count } else { held <= count };
            ensure(counted, "G4: a map count is not the leaves of its chunk")?;
        }
        // A device entry as a chunk: its count is its leaves while the node
        // that owns it runs, and at least them otherwise (a frozen count,
        // G5; ver2-D 5).
        for (entry, &held) in entries.iter().enumerate() {
            let count = table.device_leaves[entry];
            let word = table.devices[entry].owner;
            let alive = word.state() == dodos_abi::OwnState::Taken
                && matches!(word.owner(), dodos_abi::Owner::Node(n)
                    if self.has_generation(n, word.generation()) && self.running_node(n));
            let counted = if alive { held == count } else { held <= count };
            ensure(
                counted,
                "G4: a device leaf count is not the leaves of its entry",
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
                // A use entry names a granting slot of the table.
                let granter = table.members.get(used.granter as usize);
                let granter = granter.ok_or("G6: a use entry names a node past the table")?;
                let over = granter.generation() != used.generation
                    || matches!(granter.state(), MemberState::Dead);
                // A unit that failed keeps the leaves counted for ever, and
                // the grant stays: G6 holds then too (IOMMU: "a frame
                // another node granted stays pinned").
                if used.leaves == 0 {
                    continue;
                }
                // Nor does the granting generation end under such a count:
                // a holder whose kernel runs leaves R2 undone and never
                // acknowledges it, so its barrier never holds (Reclaim, R2,
                // D-142). A fenced holder's leaves count no more once every
                // live node acknowledged it (the barrier).
                ensure(
                    !over || !self.running_node(slot),
                    "G6: a granting generation over while a running holder counts a leaf",
                )?;
                if over {
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
                let grantee = table.members.get(entry.grantee as usize);
                let grantee = *grantee.ok_or("G6: a grant entry names a grantee past the table")?;
                let over = grantee.generation() != entry.generation
                    || !matches!(
                        grantee.state(),
                        MemberState::Starting
                            | MemberState::Live
                            | MemberState::Sealing
                            | MemberState::Sealed
                    );
                let sending = model.peers_sending(entry.grantee, (grant, entry.serial));
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
                if self.faulty.contains(&from) || self.faulty.contains(&to) {
                    continue;
                }
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
                self.check_reserve(x, y, peer)?;
                let member = table.member(y);
                let answering = matches!(
                    member.state(),
                    MemberState::Starting | MemberState::Live | MemberState::Sealing
                );
                // A sealed or resuming node keeps its credits toward a peer
                // whose generation ended while it was sealed until its
                // resume runs R0 for it (Grants, "Generations"). An accused
                // node's credits are not judged: it runs nothing, and the
                // peer's R0 drops its requests unanswered ("What ends with
                // a generation"; pass 24, a `Revoke` in flight when the
                // granting node was killed).
                if !answering
                    || !self.running_node(x)
                    || !self.has_generation(y, member.generation())
                    || model.ended_while_sealed(y).is_some()
                {
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
        // A starting node holds no unit role, held or as the target of a
        // handing (D-146; its plain and reserve roles are named by no
        // message).
        for (role, word) in table.roles.iter().enumerate() {
            if table.role_records[role].kind != dodos_abi::RoleKind::Unit {
                continue;
            }
            let named = match word.state() {
                dodos_abi::RoleState::Held => Some(word.holder()),
                dodos_abi::RoleState::Handing => Some(word.target()),
                _ => None,
            };
            ensure(
                named.is_none_or(|n| table.member(n).state() != MemberState::Starting),
                "UNIT ROLE: a starting node holds a unit role",
            )?;
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

    /// The fence, inside the event that makes it: "the driver raises p's
    /// park request, then marks p's word, then stops p". In the trace of
    /// the event, a core marked was raised before, and a core both marked
    /// and stopped was marked before it was stopped. The state after the
    /// event cannot tell the orders apart (the eighth model review).
    /// G15, R6's proof (D-149): a dead generation's grant entries and
    /// attach words were freed only once no grantee's use entry counted a
    /// leaf under them and no attach word beside its identifiers read
    /// *attached*, whatever `acked` and `maps_of` said.
    fn check_r6_proof(&self) -> Check {
        let table = self.canonical();
        for x in 0..table.nodes {
            let member = table.member(x);
            if member.state() != MemberState::Dead {
                continue;
            }
            let g = member.generation();
            let counted = table
                .uses
                .iter()
                .flat_map(|u| u.values())
                .any(|u| u.granter == x && u.generation == g && u.leaves > 0);
            ensure(
                !counted,
                "R6: a dead generation's grant still counted by its grantee",
            )?;
            let taken = dodos_abi::Ownership::taken(dodos_abi::Owner::Node(x), g).raw();
            let attached = table.attach.iter().any(|(key, &raw)| {
                table.identifiers.get(key) == Some(&taken)
                    && dodos_abi::AttachWord::from_raw(raw).and_then(dodos_abi::AttachWord::state)
                        == Some(dodos_abi::AttachState::Attached)
            });
            ensure(!attached, "R6: a dead generation's device still attached")?;
        }
        Ok(())
    }

    /// The fence sends its stop again once per round to every core not
    /// yet confirmed (ARM64, D-143; the RISC-V cooperative fence, D-146):
    /// while a live driver whose first core runs fences a node, no
    /// unconfirmed core of it went two rounds without a stop (one round,
    /// and the driver's own lateness, a sleeping core's exit latency among
    /// it).
    fn check_fence_rounds(&self) -> Check {
        let table = self.canonical();
        for node in 0..table.nodes {
            let member = table.member(node);
            if member.state() != MemberState::Fencing {
                continue;
            }
            let driver = member.driver();
            let g = member.generation();
            let Some(model) = self
                .models()
                .find(|&(slot, _)| slot == driver)
                .map(|(_, m)| m)
            else {
                continue;
            };
            let record = &table.fences[node as usize];
            // The rounds are due events of the driver's first core: a
            // driver whose core hung or parked drives nothing.
            let first = model.core_number(0);
            if table.member(driver).state() != MemberState::Live
                || model.is_silent(0)
                || matches!(self.hardware(first), Hardware::Hung { .. } | Hardware::Off)
                || !model.fences.contains_key(&node)
                || record.begun(g).is_none()
            {
                continue;
            }
            let round = model.fence_round(node, g);
            let now = model.membership_now();
            for core in table.cores_of(node, g) {
                ensure(
                    model.park_confirmed(core) || record.last_stop(core) + 2 * round >= now,
                    "FENCE ROUND: an unconfirmed core of a fenced node went two rounds without a stop",
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn check_fence_order(&self) -> Check {
        use crate::table::FenceStep;
        let trace = &self.canonical().fence_trace;
        for (at, step) in trace.iter().enumerate() {
            let before = &trace[..at];
            match *step {
                FenceStep::Marked(core) => ensure(
                    before.contains(&FenceStep::Raised(core)),
                    "fence: a word marked before its park request was raised",
                )?,
                FenceStep::Stopped(core) => ensure(
                    !trace[at..].contains(&FenceStep::Marked(core)),
                    "fence: a stop sent before its word was marked",
                )?,
                FenceStep::Raised(_) => {}
            }
        }
        Ok(())
    }

    /// RESERVE: "the kernel ring Y → X keeps `REQUESTS_PER_PEER` +
    /// `SENDS_PER_PEER` + 10 entries (34) for the answers to X: one entry
    /// for each credit X can have in use toward Y, and 10 more because a
    /// `Report` takes up to 11 entries; a new holder has at most one
    /// `ReportAsk` in flight to each node". The premise (one `ReportAsk`),
    /// and the conclusion: the entries of the answers still owed to X (11
    /// for a `ReportAsk`, 1 for any other credit whose answer is not yet in
    /// the ring Y → X) fit in what the ring has free.
    pub(crate) fn check_reserve(
        &self,
        x: u16,
        y: u16,
        peer: &[Option<crate::rings::Credit>],
    ) -> Check {
        let asks = peer
            .iter()
            .flatten()
            .filter(|c| c.kind == dodos_abi::NodeMessage::ReportAsk)
            .count();
        if !self.faulty.contains(&x) {
            ensure(
                asks <= 1,
                "RESERVE: more than one ReportAsk in flight to a peer",
            )?;
        }
        if self.faulty.contains(&y) {
            return Ok(());
        }
        let still_owed: u64 = peer
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i as u8, c.as_ref()?)))
            .filter(|&(i, c)| !self.in_ring(y, x, i, c.serial))
            .map(|(_, c)| match c.kind {
                dodos_abi::NodeMessage::ReportAsk => dodos_abi::RING_MESSAGE_ENTRIES_MAX as u64,
                _ => 1,
            })
            .sum();
        let table = self.canonical();
        let used = table.rings[y as usize][x as usize]
            .kernel
            .produced
            .wrapping_sub(table.beacons[x as usize].consumed[y as usize].0);
        let free = RING_KERNEL_ENTRIES.saturating_sub(used);
        ensure(
            still_owed <= free,
            "RESERVE: answers still owed exceed the free entries of the ring",
        )
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

    /// Whether a send credit toward `peer` covers grant entry `grant`,
    /// named with its serial.
    pub(crate) fn peers_sending(&self, peer: u16, grant: (u16, u32)) -> bool {
        self.peers[peer as usize].credits.iter().flatten().any(
            |c| matches!(&c.completes, Completes::Send { grants, .. } if grants.contains(&grant)),
        )
    }

    /// Whether this node serves the unit request of `from` with a credit.
    pub(crate) fn serves(&self, from: u16, credit: u8) -> bool {
        self.unit_service.contains_key(&(from, credit))
    }
}
