//! Debugging (8.5): the debug area of each node in the crash area of this
//! boot, its two trace rings and the rate limit of the kernel ring, the
//! node record and the stop record, `debug_write` and `trace_emit`; the
//! I/O ports of x86-64; `entropy_read`.
//!
//! What the kernel traces in the model: its start (code 1), a fault
//! recorded in a thread (5), a failure of the node (6), a transition of a
//! member or role word it made (9), an ownership word that failed its
//! check when the gate read it (10), and the entries suppressed (3). DMA
//! faults (2), units that do not confirm (4), hardware errors (7) and
//! interrupts on unclaimed vectors (8) have no source in the model.

use crate::{Model, ObjId, Outcome, Output, SlotAddr};
use dodos_abi::{
    DEBUG_AREA_MAGIC, DEBUG_AREA_VERSION, DEBUG_WRITE_MAX, ENTROPY_MAX_BYTES, Error, IoPortRange,
    ObjectKind, REG_WORDS, Rights, TRACE_BURST, TRACE_BURST_NS, TRACE_FLAG_CONTINUED,
    TRACE_KERNEL_ENTRIES, TRACE_TEXT_BYTES, TRACE_USER_ENTRIES, TraceCode, TraceEntry,
    ioport_check, ports_meet, trace_index,
};

/// The debug area of a node: its header's fields, its record, its rings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DebugArea {
    pub(crate) magic: u32,
    pub(crate) version: u16,
    pub(crate) generation: u32,
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
/// record, and the debug area of each node.
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
            next_kernel: 1,
            next_user: 1,
            suppressed: 0,
            record: None,
            kernel: vec![TraceEntry::default(); TRACE_KERNEL_ENTRIES as usize],
            user: vec![TraceEntry::default(); TRACE_USER_ENTRIES as usize],
        }
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
        ring[trace_index(seq, entries) as usize] = entry;
        *next = seq + 1;
    }
}

impl CrashArea {
    /// The crash area of this boot: cleared, its sequence one more than the
    /// previous area's, every debug area starting at sequence 1.
    pub(crate) fn new(boot_id: u64, previous: u64, nodes: usize) -> Self {
        Self {
            boot_id,
            sequence: previous + 1,
            stop: None,
            areas: (0..nodes).map(|_| DebugArea::new(1)).collect(),
        }
    }

    /// The node record a reader accepts: one whose commit is this area's
    /// boot identifier.
    pub(crate) fn accepted(&self, node: u16) -> Option<Record> {
        self.areas[node as usize]
            .record
            .filter(|r| r.commit == self.boot_id)
    }

    /// The record of a node: commit 0, the rest, then commit. The fence
    /// handler (reason 5) writes only if the area holds no committed record
    /// of the same generation.
    pub(crate) fn write_record(&mut self, node: u16, record: Record) {
        let boot_id = self.boot_id;
        let area = &mut self.areas[node as usize];
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

/// The rate limit of one code of the kernel ring, in the node block: the
/// start of its window, its entries in it, the entries it suppressed.
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
        TraceEntry {
            seq: 0,
            instant: self.now,
            word2: u64::from(code as u16) | flags | u64::from(generation) << 32,
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
        let me = self.node as usize;
        if code != TraceCode::Failure {
            let mut limit = self.limits[index];
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
                    self.table.crash.areas[me].write(false, report);
                    self.reported += limit.suppressed;
                    limit.suppressed = 0;
                }
            }
            if limit.count >= TRACE_BURST {
                limit.suppressed += 1;
                self.table.crash.areas[me].suppressed += 1;
                self.limits[index] = limit;
                return;
            }
            limit.count += 1;
            self.limits[index] = limit;
        }
        let entry = self.entry(code, 0, stamp, payload);
        self.table.crash.areas[me].write(false, entry);
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

    /// A failure the node found in itself: its record, an entry of code 6.
    pub(crate) fn fail(&mut self, reason: u32) {
        let member = self.table.member(self.node);
        let record = Record {
            commit: 0,
            generation: member.generation(),
            reason,
            instant: self.now,
            member: member.raw(),
        };
        self.table.crash.write_record(self.node, record);
        self.trace(TraceCode::Failure, None, [u64::from(reason), 0, 0]);
    }

    /// A failure found by the node, as an event of the node.
    pub(crate) fn failed(&mut self, reason: u32) {
        *self.last_event = None;
        let entry = self.begin();
        self.fail(reason);
        self.end(entry);
    }

    /// A trace ring of this node, oldest entry first, as a reader keeps it.
    pub fn ring(&self, user: bool) -> Vec<TraceEntry> {
        let area = &self.table.crash.areas[self.node as usize];
        let (ring, next, entries) = match user {
            true => (&area.user, area.next_user, TRACE_USER_ENTRIES),
            false => (&area.kernel, area.next_kernel, TRACE_KERNEL_ENTRIES),
        };
        (next.saturating_sub(entries).max(1)..next)
            .map(|s| ring[trace_index(s, entries) as usize])
            .filter(|e| e.seq != 0)
            .collect()
    }

    /// The kernel entries suppressed by the rate limit since machine
    /// start, as the header counts them.
    pub fn suppressed(&self) -> u64 {
        self.table.crash.areas[self.node as usize].suppressed
    }

    /// The record of a node a reader accepts: (generation, reason).
    pub fn crash_record(&self, node: u16) -> Option<(u32, u32)> {
        self.table
            .crash
            .accepted(node)
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
        let me = self.node as usize;
        let first = self.table.crash.areas[me].next_user;
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
            self.table.crash.areas[me].write(true, entry);
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
        let me = self.node as usize;
        let first = self.table.crash.areas[me].next_user;
        let entry = self.entry(TraceCode::UserEmit, 0, stamp, payload);
        self.table.crash.areas[me].write(true, entry);
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
        let cap = self.cap(thread, slot, right)?;
        let held = self.is(cap, ObjectKind::IoPorts, right)?;
        Ok(range_of(held.badge))
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
        let (rights, range) = (held.rights, range_of(held.badge));
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
        let generator = &mut self.entropy.0;
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
pub(crate) fn range_of(badge: u64) -> IoPortRange {
    IoPortRange::new(badge & 0xFFFF, badge >> 16 & 0xFFFF).unwrap_or(IoPortRange::ALL)
}
