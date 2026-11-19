//! The invariants of debugging, the I/O ports and breakpoints (8.5), read
//! from the debug area of this node and the state of its threads.

use super::{Check, ensure};
use crate::Model;
use dodos_abi::x86_64::{
    KERNEL_PORTS, PCI_CONFIG_PORTS, REG_DR0, REG_DR3, REG_DR7, REG_UINTR, UART_PORTS, dr7_valid,
};
use dodos_abi::{
    TRACE_BURST, TRACE_BURST_NS, TRACE_FLAG_CONTINUED, TRACE_KERNEL_ENTRIES, TRACE_USER_ENTRIES,
    TraceEntry,
};

/// 88: a ring whose next sequence is `next` holds each entry of sequence s
/// at index (s − 1) mod N, the greatest being next − 1.
fn ring_holds(ring: &[TraceEntry], next: u64, entries: u64) -> bool {
    // A next sequence of 0, or an entry whose window passes 2^64, is
    // reported, never a panic of the check (the final review's
    // verifications of lots D and AD).
    if next == 0 {
        return false;
    }
    let placed = ring.iter().enumerate().all(|(index, entry)| {
        entry.seq == 0
            || (entry.seq < next
                && entry
                    .seq
                    .checked_add(entries)
                    .is_some_and(|top| top >= next)
                && (entry.seq - 1) % entries == index as u64)
    });
    let last = next == 1 || ring[((next - 2) % entries) as usize].seq == next - 1;
    placed && last
}

impl Model {
    /// Debugging:
    /// 88. in each ring, the next sequence is one more than the greatest
    ///     written, and the entry of sequence s is at index (s − 1) mod N;
    /// 89. the user ring holds only codes 16 and 17, the kernel ring never
    ///     them, and a call on `debug` writes no kernel entry;
    /// 90. the entries of a `debug_write` or `trace_emit` are consecutive,
    ///     all but the last continued, with the caller's task identifier
    ///     and marks;
    /// 91. in each window of the rate limit (opened by an entry 1 ms after
    ///     the previous window), at most 16 entries of one code other than
    ///     3 and 6; the header counts every suppressed entry, reported by
    ///     an entry of code 3 or still counted in the core's part of the
    ///     node block;
    /// 92. a committed core record of this boot has the boot identifier as
    ///     its commit; the previous crash area is never written;
    /// 93 (in `check_tree`) and: no port access touched a port refused to
    ///     this node; off x86-64 none succeeds;
    /// 95. a breakpoint word of a thread lies in the user range, its control
    ///     word within the user mask, its user-interrupt word 0.
    pub(super) fn check_debug(&self) -> Check {
        for position in 0..self.width() {
            self.check_area(position)?;
            // D-143: the #DB handler loads DR6 with its reset value before
            // it returns, at CPL0 and CPL3, so no cause is left for the
            // next #DB to read.
            ensure(
                self.cores[position].dr6 == 0,
                "DR6: a core's DR6 is not at its reset value after an event",
            )?;
        }
        // D-146: kind 9 from a #DB at CPL0 is raised at the end of the event
        // in which its thread is runnable: none keeps it after an event.
        ensure(
            self.threads
                .values()
                .all(|t| t.debug_kept.is_none() || t.lost || !Self::runnable_thread(t)),
            "KIND 9: a runnable thread keeps a breakpoint fault after an event",
        )?;
        self.check_records()
    }

    /// 88 to 91 for the debug area of the core at `position`; 89 and 90
    /// for a call on `debug` judge the area of the core the event ran on.
    fn check_area(&self, position: usize) -> Check {
        let core = &self.cores[position];
        let area = &self.table.crash.areas[core.number as usize];
        let here = position == *self.at;
        ensure(
            ring_holds(&area.kernel, area.next_kernel, TRACE_KERNEL_ENTRIES),
            "88: kernel ring",
        )?;
        ensure(
            ring_holds(&area.user, area.next_user, TRACE_USER_ENTRIES),
            "88: user ring",
        )?;
        let user_codes = area
            .user
            .iter()
            .filter(|e| e.seq != 0)
            .all(|e| matches!(e.code(), 16 | 17));
        ensure(user_codes, "89: a kernel code in the user ring")?;
        let kernel_codes = area
            .kernel
            .iter()
            .filter(|e| e.seq != 0)
            .all(|e| e.code() < 16);
        ensure(kernel_codes, "89: a user code in the kernel ring")?;
        if let Some((first, count, stamp)) = self.event.user_entries.filter(|_| here) {
            ensure(
                self.event.kernel_entries == 0,
                "89: a call on debug wrote a kernel entry",
            )?;
            let entries = TRACE_USER_ENTRIES;
            let written: Vec<&TraceEntry> = (first..first + count)
                .map(|s| &area.user[((s - 1) % entries) as usize])
                .collect();
            let consecutive = written.iter().zip(first..).all(|(e, s)| e.seq == s);
            let continued = written
                .iter()
                .enumerate()
                .all(|(i, e)| (e.word2 & TRACE_FLAG_CONTINUED != 0) == (i as u64 + 1 < count));
            let stamped = written.iter().all(|e| (e.task, e.marks) == stamp);
            ensure(
                consecutive && continued && stamped,
                "90: entries of a call on debug",
            )?;
        }
        // 91: the windows, rebuilt from the entries of each code.
        for code in (1..16u16).filter(|&c| c != 3 && c != 6) {
            let mut instants: Vec<u64> = area
                .kernel
                .iter()
                .filter(|e| e.seq != 0 && e.code() == code)
                .map(|e| e.instant)
                .collect();
            instants.sort();
            let mut window: Option<(u64, u32)> = None;
            for at in instants {
                window = match window {
                    Some((start, n)) if at < start + TRACE_BURST_NS => Some((start, n + 1)),
                    _ => Some((at, 1)),
                };
                ensure(
                    window.is_some_and(|(_, n)| n <= TRACE_BURST),
                    "91: more than 16 entries of a code in a window",
                )?;
            }
        }
        let pending: u64 = core.limits.iter().map(|l| l.suppressed).sum();
        ensure(
            area.suppressed == core.reported + pending,
            "91: suppressed entries not counted once",
        )
    }

    /// 92, 93 and 95.
    fn check_records(&self) -> Check {
        let boot_id = self.table.crash.boot_id;
        for record in self.table.crash.areas.iter().filter_map(|a| a.record) {
            ensure(
                record.commit == 0 || record.commit == boot_id,
                "92: a record committed with another boot",
            )?;
        }
        ensure(
            self.table.previous == self.machine.previous_crash,
            "92: the previous crash area written",
        )?;
        if let Some((first, last)) = self.event.ports {
            let mut refused: Vec<(u64, u64)> = KERNEL_PORTS.to_vec();
            if let Some(base) = self.machine.console_port {
                refused.push((base, base + UART_PORTS - 1));
            }
            let platform = self
                .table
                .role_records
                .iter()
                .zip(&self.table.roles)
                .any(|(r, w)| {
                    r.kind == dodos_abi::RoleKind::Plain
                        && r.object == dodos_abi::ROLE_PLAIN_PLATFORM
                        && w.state() == dodos_abi::RoleState::Held
                        && w.holder() == self.node
                });
            if !platform {
                refused.push(PCI_CONFIG_PORTS);
            }
            let touched = refused.iter().any(|&(a, b)| a <= last && first <= b);
            ensure(!touched, "93: an access to a refused port")?;
            ensure(
                self.machine.arch == crate::boot::Arch::X86_64,
                "93: a port access off x86-64",
            )?;
        }
        let user_end = self.table.geometry.user_end;
        for thread in self.threads.values() {
            let regs = &thread.regs;
            let addresses = regs[REG_DR0..=REG_DR3].iter().all(|&a| a < user_end);
            ensure(
                addresses && dr7_valid(regs[REG_DR7]) && regs[REG_UINTR] == 0,
                "95: breakpoint words of a thread",
            )?;
        }
        Ok(())
    }
}
