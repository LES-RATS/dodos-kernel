//! The local APIC in either mode. Register offsets are those of the xAPIC
//! (SDM vol. 3A table 13-1); in x2APIC mode register at offset `o` is MSR
//! 0x800 + o / 16 (vol. 3A 13.12.1.2; vol. 4 table 2-2 lists 80BH EOI, 830H
//! ICR, 83FH SELF IPI). ICR fields: vol. 3A 13.6.1, figure 13-12; in x2APIC
//! mode the destination is bits 63:32 and there is no delivery-status bit
//! (13.12.9).

use crate::idt::{DODOS_X2APIC, DODOS_XAPIC_BASE, V_SPURIOUS};
use crate::x86;
use core::sync::atomic::Ordering;

pub const ID: u32 = 0x20;
pub const VERSION: u32 = 0x30;
pub const TPR: u32 = 0x80;
pub const SVR: u32 = 0xF0;
pub const LVT_TIMER: u32 = 0x320;
pub const LVT_THERMAL: u32 = 0x330;
pub const LVT_PERF: u32 = 0x340;
pub const LVT_LINT0: u32 = 0x350;
pub const LVT_LINT1: u32 = 0x360;
pub const LVT_ERROR: u32 = 0x370;
pub const TIMER_INITIAL: u32 = 0x380;
pub const TIMER_DIVIDE: u32 = 0x3E0;

/// ICR delivery modes, bits 10:8.
pub const FIXED: u32 = 0 << 8;
pub const NMI: u32 = 4 << 8;
pub const INIT: u32 = 5 << 8;
pub const STARTUP: u32 = 6 << 8;
const LEVEL_ASSERT: u32 = 1 << 14;
const DELIVERY_PENDING: u32 = 1 << 12;
const LOGICAL: u32 = 1 << 11;

pub fn x2() -> bool {
    DODOS_X2APIC.load(Ordering::Relaxed) != 0
}

fn mmio(off: u32) -> *mut u32 {
    (DODOS_XAPIC_BASE.load(Ordering::Relaxed) + u64::from(off)) as *mut u32
}

pub fn read(off: u32) -> u32 {
    if x2() {
        // SAFETY: an x2APIC register MSR exists for every xAPIC register
        // read here.
        unsafe { x86::rdmsr(0x800 + off / 16) as u32 }
    } else {
        // SAFETY: the xAPIC page is mapped uncached (MTRR) at its base.
        unsafe { mmio(off).read_volatile() }
    }
}

pub fn write(off: u32, v: u32) {
    if x2() {
        // SAFETY: as `read`; the value is a valid register value.
        unsafe { x86::wrmsr(0x800 + off / 16, u64::from(v)) };
    } else {
        // SAFETY: as `read`.
        unsafe { mmio(off).write_volatile(v) };
    }
}

/// This core's APIC ID as the ICR destination wants it.
pub fn id() -> u32 {
    if x2() { read(ID) } else { read(ID) >> 24 }
}

/// Sends an IPI with `mode` and `vector` to the core with APIC ID `dest`.
pub fn send(dest: u32, mode: u32, vector: u8) {
    send_icr(dest, mode | LEVEL_ASSERT | u32::from(vector));
}

/// Sends to every core whose logical ID matches `mask` in `cluster`
/// (x2APIC logical mode only, vol. 3A 13.12.10.2).
pub fn send_logical(cluster: u32, mask: u32, vector: u8) {
    send_icr(
        cluster << 16 | mask,
        FIXED | LEVEL_ASSERT | LOGICAL | u32::from(vector),
    );
}

fn send_icr(dest: u32, low: u32) {
    if x2() {
        // SAFETY: a valid ICR value; sending an IPI is the intent.
        unsafe { x86::wrmsr(0x830, u64::from(dest) << 32 | u64::from(low)) };
    } else {
        // SAFETY: the xAPIC ICR; the high half first, the low half sends.
        unsafe {
            mmio(0x310).write_volatile(dest << 24);
            mmio(0x300).write_volatile(low);
            while mmio(0x300).read_volatile() & DELIVERY_PENDING != 0 {
                x86::pause();
            }
        }
    }
}

pub fn self_ipi(vector: u8) {
    if x2() {
        // SAFETY: the SELF IPI register takes a vector.
        unsafe { x86::wrmsr(0x83F, u64::from(vector)) };
    } else {
        // Shorthand "self" (bits 19:18 = 01), fixed, edge.
        send_icr(0, 1 << 18 | LEVEL_ASSERT | u32::from(vector));
    }
}

/// Software-enables this core's APIC with the image's spurious vector and
/// accepts every priority; masks the timer, the error and the performance
/// entries (LVT mask bit 16).
pub fn enable_local() {
    write(SVR, 0x100 | u32::from(V_SPURIOUS));
    write(TPR, 0);
    write(LVT_TIMER, 1 << 16);
    write(LVT_ERROR, 1 << 16);
}

/// Arms a one-shot timer interrupt `vector` at TSC value `deadline`
/// (TSC-deadline mode, LVT bits 18:17 = 10b, vol. 3A 13.5.4.1).
pub fn arm_deadline(vector: u8, deadline: u64) {
    write(LVT_TIMER, 2 << 17 | u32::from(vector));
    // SAFETY: IA32_TSC_DEADLINE exists when CPUID.01H:ECX.TSC_DEADLINE[24]
    // is set, which the caller checked.
    unsafe { x86::wrmsr(x86::IA32_TSC_DEADLINE, deadline) };
}

/// The global mode of the machine, as seen from IA32_APIC_BASE.
pub fn detect() {
    // SAFETY: IA32_APIC_BASE exists on every processor with an APIC.
    let base = unsafe { x86::rdmsr(x86::IA32_APIC_BASE) };
    DODOS_X2APIC.store(u8::from(base & x86::APIC_BASE_EXTD != 0), Ordering::Relaxed);
    DODOS_XAPIC_BASE.store(base & 0x000F_FFFF_FFFF_F000, Ordering::Relaxed);
}

/// Puts the calling core in x2APIC mode if it is in xAPIC mode
/// (EN = 1, EXTD = 0 to EN = 1, EXTD = 1: the one valid way, vol. 3A
/// 13.12.5.1, figure 13-27).
pub fn enter_x2apic_local() {
    // SAFETY: as `detect`; the transition is a valid one.
    unsafe {
        let base = x86::rdmsr(x86::IA32_APIC_BASE);
        if base & x86::APIC_BASE_EXTD == 0 {
            x86::wrmsr(
                x86::IA32_APIC_BASE,
                base | x86::APIC_BASE_EN | x86::APIC_BASE_EXTD,
            );
        }
    }
}
