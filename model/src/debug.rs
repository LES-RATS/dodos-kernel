//! Debugging (8.5): the debug area of each physical core in the crash area
//! of this boot (D-129), its two trace rings and the rate limit of the
//! kernel ring, counted per core, the core record and the stop record, `debug_write` and `trace_emit`; the
//! I/O ports of x86-64; `entropy_read`.
//!
//! What the kernel traces in the model, in the kernel ring: its start
//! (code 1), the entries suppressed (3), a unit that does not confirm (4),
//! a fault recorded in a thread (5), a failure of the node (6), a
//! transition of a member or role word it made (9), an ownership word that
//! failed its check when the gate read it (10), a message refused by the
//! check (11) and a debug area cleared at the start (12); in the user ring,
//! `debug_write` (16) and `trace_emit` (17). DMA faults (2), hardware
//! errors (7) and interrupts on unclaimed vectors (8) have no source in the
//! model.

use crate::{Model, ObjId, Outcome, Output, SlotAddr};
use dodos_abi::{
    DEBUG_AREA_MAGIC, DEBUG_AREA_VERSION, DEBUG_WRITE_MAX, ENTROPY_MAX_BYTES, Error, IoPortRange,
    ObjectKind, REG_WORDS, Rights, TRACE_BURST, TRACE_BURST_NS, TRACE_FLAG_CONTINUED,
    TRACE_KERNEL_ENTRIES, TRACE_TEXT_BYTES, TRACE_USER_ENTRIES, TraceCode, TraceEntry,
    ioport_check, ports_meet, trace_index,
};

/// The debug area of a physical core: its header's fields (the node slot
/// and generation of the node it last ran), its record, its rings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DebugArea {
    pub(crate) magic: u32,
    pub(crate) version: u16,
    pub(crate) generation: u32,
    pub(crate) slot: u16,
    pub(crate) next_kernel: u64,
    pub(crate) next_user: u64,
    pub(crate) suppressed: u64,
    pub(crate) record: Option<Record>,
    pub(crate) kernel: Vec<TraceEntry>,
    pub(crate) user: Vec<TraceEntry>,
}

/// A node record: its commit, generation, reason, instant and the member
/// word as last seen (the detail words, the text and the copies of the
/// rings are not modelled).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Record {
    pub(crate) commit: u64,
    pub(crate) generation: u32,
    pub(crate) reason: u32,
    pub(crate) instant: u64,
    pub(crate) member: u64,
}

/// A crash area: its header's boot identifier and sequence, its stop
/// record, and the debug area of each physical core (D-129).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrashArea {
    pub(crate) boot_id: u64,
    pub(crate) sequence: u64,
    pub(crate) stop: Option<(u64, u16, u32)>,
    pub(crate) areas: Vec<DebugArea>,
}

impl DebugArea {
    pub(crate) fn new(generation: u32) -> Self {
        Self {
            magic: DEBUG_AREA_MAGIC,
            version: DEBUG_AREA_VERSION,
            generation,
            slot: 0,
            next_kernel: 1,
            next_user: 1,
            suppressed: 0,
            record: None,
            kernel: vec![TraceEntry::default(); TRACE_KERNEL_ENTRIES as usize],
            user: vec![TraceEntry::default(); TRACE_USER_ENTRIES as usize],
        }
    }

    /// Whether a kept area's next sequences can be continued: each at least
    /// 1, below 2^63, and 1 or the sequence of the entry before it in its
    /// ring (D-141).
    fn continues(&self) -> bool {
        let holds = |ring: &[TraceEntry], next: u64, entries: u64| {
            (1..1 << 63).contains(&next)
                && (next == 1
                    || trace_index(next - 1, entries)
                        .is_some_and(|at| ring[at as usize].seq == next - 1))
        };
        holds(&self.kernel, self.next_kernel, TRACE_KERNEL_ENTRIES)
            && holds(&self.user, self.next_user, TRACE_USER_ENTRIES)
    }

    /// Writes one entry as it is, for the tests of the invariants.
    #[cfg(test)]
    pub(crate) fn write_raw(&mut self, user: bool, entry: TraceEntry) {
        self.write(user, entry)
    }

    /// Writes one entry into a ring: word 0 last, then the next sequence.
    fn write(&mut self, user: bool, mut entry: TraceEntry) {
        let (ring, next, entries) = match user {
            true => (&mut self.user, &mut self.next_user, TRACE_USER_ENTRIES),
            false => (
                &mut self.kernel,
                &mut self.next_kernel,
                TRACE_KERNEL_ENTRIES,
            ),
        };
        let seq = *next;
        entry.seq = seq;
        let at = trace_index(seq, entries).expect("a sequence of at least 1");
        ring[at as usize] = entry;
        *next = seq + 1;
    }
}

impl CrashArea {
    /// The crash area of this boot: cleared, its sequence one more than the
    /// previous area's, the debug area of every physical core starting at
    /// sequence 1.
    pub(crate) fn new(boot_id: u64, previous: u64, cores: usize) -> Self {
        Self {
            boot_id,
            // A previous header of sequence 2^64 - 1 is not valid: the
            // area of this boot then follows none (D-141).
            sequence: previous.checked_add(1).unwrap_or(1),
            stop: None,
            areas: (0..cores).map(|_| DebugArea::new(1)).collect(),
        }
    }

    /// The core record a reader accepts: one whose commit is this area's
    /// boot identifier.
    pub(crate) fn accepted(&self, core: usize) -> Option<Record> {
        self.areas[core].record.filter(|r| r.commit == self.boot_id)
    }

    /// The record of a core: commit 0, the rest, then commit. The fence
    /// handler (reason 5) writes only if the area holds no committed record
    /// of the same generation.
    pub(crate) fn write_record(&mut self, core: usize, record: Record) {
        let boot_id = self.boot_id;
        let area = &mut self.areas[core];
        let held = area
            .record
            .is_some_and(|r| r.commit == boot_id && r.generation == record.generation);
        if record.reason == 5 && held {
            return;
        }
        area.record = Some(Record {
            commit: boot_id,
            ..record
        });
    }
}

/// The rate limit of one code of the kernel ring, in the core's part of the
/// node block: the start of its window, its entries in it, the entries it
/// suppressed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Limit {
    pub(crate) window: Option<u64>,
    pub(crate) count: u32,
    pub(crate) suppressed: u64,
}

impl Model {
    /// The task identifier and effective marks a thread stamps on an
    /// entry.
    fn stamp(&self, thread: Option<ObjId>) -> (u64, u64) {
        let Some(thread) = thread else {
            return (0, 0);
        };
        let task = self.threads[&thread]
            .task
            .map_or(0, |t| self.tasks[&t].id.raw());
        (task, self.marks(thread).map_or(0, |m| m.bits()))
    }

    fn entry(
        &self,
        code: TraceCode,
        flags: u64,
        (task, marks): (u64, u64),
        payload: [u64; 3],
    ) -> TraceEntry {
        let generation = self.table.generation(self.node);
        let continued = flags & dodos_abi::TRACE_FLAG_CONTINUED != 0;
        TraceEntry {
            seq: 0,
            instant: self.now,
            word2: TraceEntry::word2_of(code as u16, continued, self.node, generation),
            task,
            marks,
            payload,
        }
    }

    /// A kernel entry: each code but 6 at most 16 per window of 1 ms; one
    /// over the limit is counted, and the first entry allowed in a new
    /// window is preceded by an entry of code 3 with that count.
    pub(crate) fn trace(&mut self, code: TraceCode, thread: Option<ObjId>, payload: [u64; 3]) {
        let stamp = self.stamp(thread);
        let index = code as usize;
        let now = self.now;
        let me = self.here().number as usize;
        if code != TraceCode::Failure {
            let mut limit = self.here().limits[index];
            if limit.window.is_none_or(|at| now >= at + TRACE_BURST_NS) {
                limit.window = Some(now);
                limit.count = 0;
                if limit.suppressed > 0 {
                    let report = self.entry(
                        TraceCode::Suppressed,
                        0,
                        (0, 0),
                        [index as u64, limit.suppressed, 0],
                    );
                    std::sync::Arc::make_mut(&mut self.table.crash).areas[me].write(false, report);
                    self.here_mut().reported += limit.suppressed;
                    limit.suppressed = 0;
                }
            }
            if limit.count >= TRACE_BURST {
                limit.suppressed += 1;
                std::sync::Arc::make_mut(&mut self.table.crash).areas[me].suppressed += 1;
                self.here_mut().limits[index] = limit;
                return;
            }
            limit.count += 1;
            self.here_mut().limits[index] = limit;
        }
        let entry = self.entry(code, 0, stamp, payload);
        std::sync::Arc::make_mut(&mut self.table.crash).areas[me].write(false, entry);
        self.event.kernel_entries += 1;
    }

    /// The transitions of member and role words this node made in the
    /// event: code 9.
    pub(crate) fn trace_transitions(&mut self) {
        let me = self.node;
        let members: Vec<(u16, u64)> = self
            .table
            .member_writes
            .iter()
            .filter(|w| w.1 == me)
            .map(|w| (w.0, w.3.raw()))
            .collect();
        let roles: Vec<(usize, u64)> = self
            .table
            .role_writes
            .iter()
            .filter(|w| w.1 == me)
            .map(|w| (w.0, w.3.raw()))
            .collect();
        for (node, word) in members {
            self.trace(TraceCode::Transition, None, [u64::from(node), word, 0]);
        }
        for (role, word) in roles {
            self.trace(TraceCode::Transition, None, [role as u64, word, 1]);
        }
    }

    /// A failure the node found in itself: the record of the core that
    /// found it, an entry of code 6.
    pub(crate) fn fail(&mut self, reason: u32) {
        let member = self.table.member(self.node);
        let record = Record {
            commit: 0,
            generation: member.generation(),
            reason,
            instant: self.now,
            member: member.raw(),
        };
        let core = self.here().number as usize;
        std::sync::Arc::make_mut(&mut self.table.crash).write_record(core, record);
        self.trace(TraceCode::Failure, None, [u64::from(reason), 0, 0]);
    }

    /// Each core that starts in the node keeps its debug area if the
    /// header's magic and version are right and each next sequence is at
    /// least 1, below 2^63 and agrees with its ring, with the node slot and
    /// the generation, and goes on from the next sequences found there;
    /// otherwise it clears the area, starts at 1 and writes one entry of
    /// code 12 (D-141: the header is a faulty kernel's data).
    pub(crate) fn enter_debug_areas(&mut self) {
        let generation = self.table.generation(self.node);
        for core in self
            .cores
            .iter()
            .map(|c| c.number as usize)
            .collect::<Vec<_>>()
        {
            let found = {
                let area = &self.table.crash.areas[core];
                (area.next_kernel, area.next_user)
            };
            let cleared = self.cleared_entry(found);
            let area = &mut std::sync::Arc::make_mut(&mut self.table.crash).areas[core];
            let right = area.magic == DEBUG_AREA_MAGIC && area.version == DEBUG_AREA_VERSION;
            if !right || !area.continues() {
                *area = DebugArea::new(generation);
                area.write(false, cleared);
            }
            (area.generation, area.slot) = (generation, self.node);
        }
    }

    /// The entry of code 12 that a cleared area starts with.
    fn cleared_entry(&self, (kernel, user): (u64, u64)) -> TraceEntry {
        let mut entry = self.entry(TraceCode::AreaCleared, 0, (0, 0), [kernel, user, 0]);
        entry.seq = 0;
        entry
    }

    /// A trace ring of the node's first core, oldest entry first, as a
    /// reader keeps it.
    pub fn ring(&self, user: bool) -> Vec<TraceEntry> {
        self.trace_ring(self.cores[0].number as usize, user)
    }

    /// A trace ring of a physical core, by core number.
    pub fn trace_ring(&self, core: usize, user: bool) -> Vec<TraceEntry> {
        let area = &self.table.crash.areas[core];
        let (ring, next, entries) = match user {
            true => (&area.user, area.next_user, TRACE_USER_ENTRIES),
            false => (&area.kernel, area.next_kernel, TRACE_KERNEL_ENTRIES),
        };
        (next.saturating_sub(entries).max(1)..next)
            .filter_map(|s| trace_index(s, entries).map(|at| ring[at as usize]))
            .filter(|e| e.seq != 0)
            .collect()
    }

    /// The kernel entries suppressed by the rate limit since machine
    /// start, as the header counts them.
    pub fn suppressed(&self) -> u64 {
        self.table.crash.areas[self.cores[0].number as usize].suppressed
    }

    /// The generation and node slot in the header of a core's debug area.
    pub fn area_generation(&self, core: usize) -> (u32, u16) {
        let area = &self.table.crash.areas[core];
        (area.generation, area.slot)
    }

    /// The record of a core a reader accepts: (generation, reason).
    pub fn crash_record(&self, core: usize) -> Option<(u32, u32)> {
        self.table
            .crash
            .accepted(core)
            .map(|r| (r.generation, r.reason))
    }

    /// `debug_write`: the text is the first ⌈length / 8⌉ message words; it
    /// goes into the user ring of the caller's node, 24 bytes an entry.
    pub(crate) fn debug_write(
        &mut self,
        thread: ObjId,
        debug: SlotAddr,
        length: u64,
        words: &[u64],
    ) -> Outcome {
        self.object(thread, debug, ObjectKind::Debug, Rights::WRITE)?;
        if length > DEBUG_WRITE_MAX {
            return Err(Error::Invalid);
        }
        let needed = length.div_ceil(8) as usize;
        if needed > REG_WORDS && self.threads[&thread].page.is_none() {
            return Err(Error::Invalid);
        }
        let mut bytes: Vec<u8> = (0..length as usize)
            .map(|k| (words.get(k / 8).copied().unwrap_or(0) >> (8 * (k % 8))) as u8)
            .collect();
        let entries = length.div_ceil(TRACE_TEXT_BYTES) as usize;
        bytes.resize(entries * TRACE_TEXT_BYTES as usize, 0);
        let stamp = self.stamp(Some(thread));
        let me = self.here().number as usize;
        let first = std::sync::Arc::make_mut(&mut self.table.crash).areas[me].next_user;
        for (i, chunk) in bytes.chunks(TRACE_TEXT_BYTES as usize).enumerate() {
            let word =
                |w: usize| u64::from_le_bytes(chunk[8 * w..8 * w + 8].try_into().expect("8 bytes"));
            let flags = if i + 1 < entries {
                TRACE_FLAG_CONTINUED
            } else {
                0
            };
            let entry = self.entry(
                TraceCode::UserText,
                flags,
                stamp,
                [word(0), word(1), word(2)],
            );
            std::sync::Arc::make_mut(&mut self.table.crash).areas[me].write(true, entry);
        }
        self.event.user_entries = Some((first, entries as u64, stamp));
        Ok(Output::Done)
    }

    /// `trace_emit`: one entry of code 17.
    pub(crate) fn trace_emit(
        &mut self,
        thread: ObjId,
        debug: SlotAddr,
        payload: [u64; 3],
    ) -> Outcome {
        self.object(thread, debug, ObjectKind::Debug, Rights::WRITE)?;
        let stamp = self.stamp(Some(thread));
        let me = self.here().number as usize;
        let first = std::sync::Arc::make_mut(&mut self.table.crash).areas[me].next_user;
        let entry = self.entry(TraceCode::UserEmit, 0, stamp, payload);
        std::sync::Arc::make_mut(&mut self.table.crash).areas[me].write(true, entry);
        self.event.user_entries = Some((first, 1, stamp));
        Ok(Output::Done)
    }

    /// The ports refused to this node: the kernel's (the legacy PIC, the
    /// console's UART), and the PCI configuration ports unless it holds the
    /// plain role `platform`.
    fn ports_refused(&self, first: u64, last: u64) -> bool {
        let mut refused: Vec<(u64, u64)> = dodos_abi::x86_64::KERNEL_PORTS.to_vec();
        if let Some(base) = self.machine.console_port {
            refused.push((base, base + dodos_abi::x86_64::UART_PORTS - 1));
        }
        if !self.platform_holder() {
            refused.push(dodos_abi::x86_64::PCI_CONFIG_PORTS);
        }
        refused
            .iter()
            .any(|&range| ports_meet(range, (first, last)))
    }

    /// Whether this node holds the plain role `platform`.
    fn platform_holder(&self) -> bool {
        self.table
            .role_records
            .iter()
            .zip(&self.table.roles)
            .any(|(record, word)| {
                record.kind == dodos_abi::RoleKind::Plain
                    && record.object == dodos_abi::ROLE_PLAIN_PLATFORM
                    && word.state() == dodos_abi::RoleState::Held
                    && word.holder() == self.node
            })
    }

    /// The range an `ioports` capability carries.
    fn ports_of(&self, thread: ObjId, slot: SlotAddr, right: Rights) -> Result<IoPortRange, Error> {
        // Through a `cspace` capability, reading a slot as the target of a
        // call needs `read` (Slots and addresses): the right of the call is
        // judged on the `ioports` capability itself (the final review's
        // verification of lot B, O9).
        let cap = self.cap(thread, slot, Rights::READ)?;
        let held = self.is(cap, ObjectKind::IoPorts, right)?;
        // A word that names no range admits no port: `busy`, as a port
        // refused to the node, never every port.
        range_of(held.badge).ok_or(Error::Busy)
    }

    /// `ioport_in` and `ioport_out`.
    pub(crate) fn ioport(
        &mut self,
        thread: ObjId,
        ioports: SlotAddr,
        port: u64,
        width: u64,
        value: Option<u64>,
    ) -> Outcome {
        self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
        if self.machine.arch != crate::boot::Arch::X86_64 {
            return Err(Error::Unsupported);
        }
        let right = if value.is_some() {
            Rights::WRITE
        } else {
            Rights::READ
        };
        let range = self.ports_of(thread, ioports, right)?;
        ioport_check(range, port, width, value)?;
        if self.ports_refused(port, port + width - 1) {
            return Err(Error::Busy);
        }
        self.event.ports = Some((port, port + width - 1));
        match value {
            Some(value) => {
                for byte in 0..width {
                    self.table
                        .ports
                        .insert((port + byte) as u16, (value >> (8 * byte)) as u8);
                }
                Ok(Output::Done)
            }
            None => {
                let read = (0..width).fold(0, |read, byte| {
                    let at = (port + byte) as u16;
                    read | u64::from(self.table.ports.get(&at).copied().unwrap_or(0)) << (8 * byte)
                });
                Ok(Output::Value(read))
            }
        }
    }

    /// `ioports_issue`: a child of the source with a range inside its own.
    pub(crate) fn ioports_issue(
        &mut self,
        thread: ObjId,
        ioports: SlotAddr,
        destination: SlotAddr,
        first: u64,
        last: u64,
    ) -> Outcome {
        self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
        if self.machine.arch != crate::boot::Arch::X86_64 {
            return Err(Error::Unsupported);
        }
        let parent = self.cap(thread, ioports, Rights::READ)?;
        let held = self.is(parent, ObjectKind::IoPorts, Rights::DERIVE)?;
        let (rights, range) = (held.rights, range_of(held.badge).ok_or(Error::Invalid)?);
        let place = self.destination(thread, destination)?;
        let new = IoPortRange::new(first, last).ok_or(Error::Invalid)?;
        if new.first() < range.first() || new.last() > range.last() {
            return Err(Error::Invalid);
        }
        self.fits(place.space, parent, crate::caps::Placing::Derived)?;
        self.vacant(place)?;
        self.derive(parent, rights, new.raw(), place);
        Ok(Output::Done)
    }

    /// `entropy_read`: the four words are mixed into the node's generator,
    /// which then fills ⌈length / 8⌉ words with `length` bytes.
    pub(crate) fn entropy_read(&mut self, thread: ObjId, length: u64, mix: [u64; 4]) -> Outcome {
        self.addressed_space(thread, SlotAddr::NONE, Rights::READ)?;
        if length > ENTROPY_MAX_BYTES {
            return Err(Error::Invalid);
        }
        let words = length.div_ceil(8) as usize;
        if words > REG_WORDS && self.threads[&thread].page.is_none() {
            return Err(Error::Invalid);
        }
        let generator = &mut self.here_mut().entropy;
        for word in mix {
            *generator ^= word;
            *generator = generator
                .rotate_left(17)
                .wrapping_mul(0x2545_F491_4F6C_DD1D)
                | 1;
        }
        let mut bytes = Vec::new();
        for i in 0..words {
            *generator ^= *generator >> 12;
            *generator ^= *generator << 25;
            *generator ^= *generator >> 27;
            let mut word = generator.wrapping_mul(0x2545_F491_4F6C_DD1D);
            let left = length - 8 * i as u64;
            if left < 8 {
                word &= (1 << (8 * left)) - 1;
            }
            bytes.push(word);
        }
        Ok(Output::Bytes(bytes))
    }
}

/// The range in the word of an `ioports` slot: first in bits 0-15, last in
/// bits 16-31.
pub(crate) fn range_of(badge: u64) -> Option<IoPortRange> {
    IoPortRange::new(badge & 0xFFFF, badge >> 16 & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use crate::{Machine, Model};

    #[test]
    fn failures_are_never_held_back_by_the_rate_limit() {
        let (mut m, _) = Model::boot(&Machine::example()).unwrap();
        for _ in 0..20 {
            let entry = m.begin_on(0);
            m.fail(1);
            m.end(entry);
            assert_eq!(m.check(), Ok(()));
        }
        let failures = m.ring(false).iter().filter(|e| e.code() == 6).count();
        assert_eq!((failures, m.suppressed()), (20, 0));
    }

    /// Argon F3 (mutants 49, 50, 52): the sequence of a new crash area is
    /// one more than the previous one's; a reader accepts only a record
    /// committed with this boot's identifier; a record that is not a fence's
    /// replaces a held one, a fence's does not.
    #[test]
    fn crash_area_sequence_commit_and_overwrite_follow_the_record_rules() {
        let area = super::CrashArea::new(7, 41, 2);
        assert_eq!(area.sequence, 42);
        let mut area = super::CrashArea::new(7, 0, 2);
        let record = |reason| super::Record {
            commit: 0,
            generation: 1,
            reason,
            instant: u64::from(reason),
            member: 0,
        };
        area.areas[0].record = Some(super::Record {
            commit: 6,
            ..record(1)
        });
        assert_eq!(area.accepted(0), None);
        area.write_record(0, record(1));
        area.write_record(0, record(2));
        assert_eq!(area.accepted(0).map(|r| r.reason), Some(2));
        area.write_record(0, record(5));
        assert_eq!(area.accepted(0).map(|r| r.reason), Some(2));
    }

    /// `entropy_read` changes no state but the generator's (the final
    /// review's F2 Opus 13, M8: the generator was not compared, so the test
    /// held by an equality that held of anything).
    #[test]
    fn entropy_read_changes_no_state_but_the_generators() {
        let (mut m, boot) = Model::boot(&Machine::example()).unwrap();
        let before = m.clone();
        let read = crate::Syscall::EntropyRead {
            length: 13,
            mix: [1, 2, 3, 4],
        };
        assert!(m.apply(boot, read).is_ok());
        assert_ne!(m.cores[0].entropy, before.cores[0].entropy);
        m.cores[0].entropy = before.cores[0].entropy;
        assert_eq!(m, before);
    }

    /// Argon F3 (mutants 79, 92): a thread whose capability space is dying
    /// gets `gone` before any argument of a port call or of `entropy_read`
    /// is looked at, and before the `unsupported` of an architecture
    /// without ports.
    #[test]
    fn a_dying_space_is_gone_before_the_port_and_entropy_checks() {
        use crate::boot::Arch;
        use dodos_abi::{Error, ObjectKind, Rights, START_SLOT_ASPACE, START_SLOT_SCHED};
        let mut machine = Machine::example();
        machine.arch = Arch::Aarch64;
        let (mut m, boot) = Model::boot(&machine).unwrap();
        let own = crate::SlotAddr::own;
        let make = |kind, size, slot| crate::Syscall::UntypedRetype {
            untyped: own(25),
            kind,
            size,
            page: 0,
            memory: 0,
            destination: own(slot),
        };
        for (kind, size, slot) in [
            (ObjectKind::CSpace, 256, 100),
            (ObjectKind::Thread, 0, 101),
            (ObjectKind::Sched, 0, 102),
        ] {
            assert_eq!(
                m.apply(boot, make(kind, size, slot)),
                Ok(crate::Output::Done)
            );
        }
        let calls = [
            // The start-up context fills the node's sum: lowered first.
            crate::Syscall::SchedConfigure {
                sched: own(START_SLOT_SCHED),
                node: own(16),
                core: 0,
                budget: 5_000_000,
                period: 10_000_000,
                class: 1,
            },
            crate::Syscall::ThreadConfigure {
                thread: own(101),
                cspace: own(100),
                aspace: own(START_SLOT_ASPACE),
                fault: crate::SlotAddr::NONE,
                page: crate::SlotAddr::NONE,
            },
            crate::Syscall::SchedConfigure {
                sched: own(102),
                node: own(16),
                core: 0,
                budget: 1_000_000,
                period: 10_000_000,
                class: 1,
            },
            crate::Syscall::SchedBind {
                sched: own(102),
                thread: own(101),
            },
            crate::Syscall::ThreadResume(own(101)),
        ];
        for call in calls {
            assert_eq!(m.apply(boot, call), Ok(crate::Output::Done));
        }
        // A space with 40 capabilities in it takes two steps to remove:
        // after the first it is dying.
        let grow = crate::Syscall::CSpaceGrow {
            cspace: own(100),
            untyped: own(25),
        };
        assert!(m.apply(boot, grow).is_ok());
        for index in 1..=40 {
            let copy = crate::Syscall::CapDerive {
                source: own(START_SLOT_ASPACE),
                destination: crate::SlotAddr::through(100, index),
                rights: Rights::READ.bits(),
                badge: 0,
            };
            assert_eq!(m.apply(boot, copy), Ok(crate::Output::Done));
        }
        let child = m.object_at(boot, own(101)).unwrap();
        let mut deleted = m.apply(boot, crate::Syscall::CapDelete(own(100)));
        assert_eq!(deleted, Err(Error::Again));
        let port = crate::Syscall::IoportIn {
            ioports: own(7),
            port: 0x80,
            width: 1,
        };
        assert_eq!(m.apply(child, port), Err(Error::Gone));
        let entropy = crate::Syscall::EntropyRead {
            length: 100,
            mix: [0; 4],
        };
        assert_eq!(m.apply(child, entropy), Err(Error::Gone));
        while deleted == Err(Error::Again) {
            deleted = m.apply(boot, crate::Syscall::CapDelete(own(100)));
        }
    }
}
