//! The per-core block, reached through GS (IA32_GS_BASE) by the assembly
//! handlers. The first words have fixed offsets that the stubs of `idt` use.

use crate::x86;
use core::mem::offset_of;
use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_CORES: usize = 64;

#[repr(C, align(4096))]
pub struct PerCpu {
    pub self_ptr: u64,
    /// Where a fault resumes while armed (0: not armed).
    pub fixup_rip: AtomicU64,
    /// Bit 63 set when a fault was taken, vector bits 0-7, error code 8-39.
    pub fixup_vector: AtomicU64,
    pub fixup_cr2: AtomicU64,
    pub irq_count: AtomicU64,
    pub irq_tsc: AtomicU64,
    /// APIC ID answered by the pong vector.
    pub pong_dest: AtomicU64,
    pub index: u64,
    pub nmi_count: AtomicU64,
    pub nmi_tsc: AtomicU64,
    /// 1: the next NMI parks this core (the fence's NMI handler); 2: the
    /// next NMI spins in the handler (NMIs blocked).
    pub nmi_park: AtomicU64,
    /// The park request this core last confirmed (the contract's `parked`).
    pub parked: AtomicU64,
    pub park_request: AtomicU64,
    pub start_request: AtomicU64,
    pub timer_count: AtomicU64,
    /// SSP read at the entry of the last exception (0 without shadow stacks).
    pub ssp_entry: AtomicU64,
    /// TSC at the first instruction of the trampoline after the last SIPI.
    pub restart_tsc: AtomicU64,
    /// Top of this core's stack, for a restart.
    pub stack_top: u64,
    /// The interrupt descriptor table of this core.
    pub idt: [u128; 256],
}

pub const FIXUP_RIP: usize = offset_of!(PerCpu, fixup_rip);
pub const FIXUP_VECTOR: usize = offset_of!(PerCpu, fixup_vector);
pub const IRQ_COUNT: usize = offset_of!(PerCpu, irq_count);
pub const IRQ_TSC: usize = offset_of!(PerCpu, irq_tsc);
pub const PONG_DEST: usize = offset_of!(PerCpu, pong_dest);
pub const NMI_COUNT: usize = offset_of!(PerCpu, nmi_count);
pub const NMI_TSC: usize = offset_of!(PerCpu, nmi_tsc);
pub const NMI_PARK: usize = offset_of!(PerCpu, nmi_park);
pub const TIMER_COUNT: usize = offset_of!(PerCpu, timer_count);
pub const SSP_ENTRY: usize = offset_of!(PerCpu, ssp_entry);
pub const STACK_TOP: usize = offset_of!(PerCpu, stack_top);

pub static mut CPUS: [PerCpu; MAX_CORES] = [const {
    PerCpu {
        self_ptr: 0,
        fixup_rip: AtomicU64::new(0),
        fixup_vector: AtomicU64::new(0),
        fixup_cr2: AtomicU64::new(0),
        irq_count: AtomicU64::new(0),
        irq_tsc: AtomicU64::new(0),
        pong_dest: AtomicU64::new(0),
        index: 0,
        nmi_count: AtomicU64::new(0),
        nmi_tsc: AtomicU64::new(0),
        nmi_park: AtomicU64::new(0),
        parked: AtomicU64::new(0),
        park_request: AtomicU64::new(0),
        start_request: AtomicU64::new(0),
        timer_count: AtomicU64::new(0),
        ssp_entry: AtomicU64::new(0),
        restart_tsc: AtomicU64::new(0),
        stack_top: 0,
        idt: [0; 256],
    }
}; MAX_CORES];

/// The block of core `i` (the image's own numbering, `smp::CORES`).
pub fn of(i: usize) -> &'static PerCpu {
    // SAFETY: shared access to atomics and to fields written once before
    // the core starts (`index`, `stack_top`, `idt` while it is not loaded).
    unsafe { &*(&raw const CPUS).cast::<PerCpu>().add(i) }
}

/// The block of the calling core.
pub fn me() -> &'static PerCpu {
    let p: u64;
    // SAFETY: GS base points at this core's block (`install`).
    unsafe { core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, readonly)) };
    // SAFETY: `p` is the block's own address, set by `install`.
    unsafe { &*(p as *const PerCpu) }
}

pub fn my_index() -> usize {
    me().index as usize
}

/// Points GS at block `i` on the calling core and fills its fixed words.
pub fn install(i: usize, stack_top: u64) {
    // SAFETY: block `i` belongs to the calling core from now on; nothing
    // else writes these two fields.
    unsafe {
        let b = (&raw mut CPUS).cast::<PerCpu>().add(i);
        (*b).self_ptr = b as u64;
        (*b).index = i as u64;
        if stack_top != 0 {
            (*b).stack_top = stack_top;
        }
        x86::wrmsr(x86::IA32_GS_BASE, b as u64);
    }
    me().fixup_rip.store(0, Ordering::Relaxed);
}
