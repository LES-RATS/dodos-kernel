//! The start-up trampoline: where a core restarted by INIT and SIPI begins
//! (the contract's "park stub, a page of kept memory below 1 MB",
//! kernel-interface.md, "The fence"). A SIPI with vector VV starts the core
//! in real mode at VV000H (SDM vol. 3A 11.4.4.2; 13.6.1, "Start-Up"). The
//! code goes to protected mode, then to IA-32e mode in the order the SDM
//! gives (PAE, CR3, IA32_EFER.LME, then CR0.PG; vol. 3A 12.8.5), finds its
//! entry by its x2APIC ID (CPUID 0BH EDX), puts back the firmware's CR0,
//! CR4, IA32_EFER and descriptor tables, and calls Rust on its stack.
//! INIT leaves the TSC, the MTRRs, the caches and the x2APIC mode unchanged
//! and clears CR0 (except CD, NW), CR4, IA32_EFER, GS base and the other
//! APIC registers (vol. 3A table 12-1; 13.12.5.1).

use crate::percpu::{self, MAX_CORES};
use crate::smp::{self, FW};
use crate::x86;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Offsets in the trampoline page.
const TR_GDT: usize = 0xF00;
const TR_GDTR: usize = 0xF20;
const TR_JMP32: usize = 0xF28;
const TR_JMP64: usize = 0xF30;
const TR_CR3: usize = 0xF38;
const TR_TABLE: usize = 0xF40;
const TR_COUNT: usize = 0xF48;
const TR_ENTRY: usize = 0xF50;

/// One core's record, found by the trampoline (offsets used by its code).
#[repr(C)]
pub struct Entry {
    pub apic: u32,
    pub index: u32,
    pub stack_top: u64,
    pub tsc: u64,
    pub cr0: u64,
    pub cr4: u64,
    pub efer: u64,
    pub gdtr: [u8; 16],
    pub cs: u64,
    pub ds: u64,
    pub pat: u64,
    pub reserved: u64,
}

const ENTRY_SIZE: usize = core::mem::size_of::<Entry>();

global_asm!(
    ".global dodos_tramp_start",
    ".global dodos_tramp_pm32",
    ".global dodos_tramp_lm64",
    ".global dodos_tramp_end",
    "dodos_tramp_start:",
    ".code16",
    "cli",
    "cld",
    "rdtsc",
    "mov esi, eax",
    "mov edi, edx",
    "mov ax, cs",
    "mov ds, ax",
    "xor ebp, ebp",
    "mov bp, ax",
    "shl ebp, 4",
    // lgdt [TR_GDTR] with a 32-bit base (66 0F 01 /2, disp16).
    ".byte 0x66, 0x0f, 0x01, 0x16",
    ".short {gdtr}",
    "mov eax, cr0",
    "or eax, 1",
    "mov cr0, eax",
    // jmp far dword [TR_JMP32] (66 FF /5, disp16): to the 32-bit code.
    ".byte 0x66, 0xff, 0x2e",
    ".short {jmp32}",
    ".code32",
    "dodos_tramp_pm32:",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "mov eax, cr4",
    "or eax, 0x20",
    "mov cr4, eax",
    "mov eax, [ebp + {cr3}]",
    "mov cr3, eax",
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or eax, 0x900",
    "wrmsr",
    "mov eax, cr0",
    "or eax, 0x80010001",
    "mov cr0, eax",
    // jmp far [ebp + TR_JMP64] (FF /5, disp32): to the 64-bit code.
    ".byte 0xff, 0xad",
    ".long {jmp64}",
    ".code64",
    "dodos_tramp_lm64:",
    "mov r8d, esi",
    "mov r9d, edi",
    "xor eax, eax",
    "cpuid",
    "cmp eax, 0xB",
    "jb 21f",
    "mov eax, 0xB",
    "xor ecx, ecx",
    "cpuid",
    "mov r10d, edx",
    "jmp 22f",
    "21:",
    "mov eax, 1",
    "cpuid",
    "shr ebx, 24",
    "mov r10d, ebx",
    "22:",
    "mov rbx, [rbp + {table}]",
    "mov ecx, [rbp + {count}]",
    "23:",
    "cmp [rbx], r10d",
    "je 25f",
    "add rbx, {esize}",
    "dec ecx",
    "jnz 23b",
    "24:",
    "cli",
    "hlt",
    "jmp 24b",
    "25:",
    "mov rsp, [rbx + 8]",
    "shl r9, 32",
    "or r8, r9",
    "mov [rbx + 16], r8",
    // The firmware's mode registers, so that Rust code may use SSE.
    "mov rax, [rbx + 32]",
    "mov cr4, rax",
    "mov rax, [rbx + 24]",
    "mov cr0, rax",
    "mov ecx, 0xC0000080",
    "mov eax, [rbx + 40]",
    "mov edx, [rbx + 44]",
    "wrmsr",
    "mov ecx, 0x277",
    "mov eax, [rbx + 80]",
    "mov edx, [rbx + 84]",
    "wrmsr",
    // The firmware's GDT and selectors.
    "lgdt [rbx + 48]",
    "mov rax, [rbx + 64]",
    "push rax",
    "lea rax, [rip + 26f]",
    "push rax",
    "retfq",
    "26:",
    "mov ax, [rbx + 72]",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "mov rdi, rbx",
    "mov rax, [rbp + {entry}]",
    "call rax",
    "ud2",
    "dodos_tramp_end:",
    gdtr = const TR_GDTR,
    jmp32 = const TR_JMP32,
    jmp64 = const TR_JMP64,
    cr3 = const TR_CR3,
    table = const TR_TABLE,
    count = const TR_COUNT,
    entry = const TR_ENTRY,
    esize = const ENTRY_SIZE,
);

unsafe extern "C" {
    static dodos_tramp_start: u8;
    static dodos_tramp_pm32: u8;
    static dodos_tramp_lm64: u8;
    static dodos_tramp_end: u8;
}

/// The trampoline page (0 until `install`) and the table of entries.
static PAGE: AtomicU64 = AtomicU64::new(0);
static TABLE: AtomicU64 = AtomicU64::new(0);

/// The SIPI vector of the trampoline, if installed.
pub fn vector() -> Option<u8> {
    let p = PAGE.load(Ordering::Relaxed);
    (p != 0).then_some((p >> 12) as u8)
}

pub fn entry(i: usize) -> &'static Entry {
    // SAFETY: the table has MAX_CORES entries, written by `install`.
    unsafe { &*((TABLE.load(Ordering::Relaxed) as *const Entry).add(i)) }
}

/// Copies the trampoline to `page` (below 1 MB) and fills the entries of
/// every core: the firmware state saved by `smp::save_fw` and a fresh
/// stack (the bootstrap core gets one for its resume path only).
pub fn install(page: u64) {
    assert!(
        page < 0x10_0000 && page & 0xFFF == 0,
        "trampoline page not below 1 MB"
    );
    // Addresses of the assembly above, one block of code.
    let (start, pm32, lm64, end) = (
        &raw const dodos_tramp_start as u64,
        &raw const dodos_tramp_pm32 as u64,
        &raw const dodos_tramp_lm64 as u64,
        &raw const dodos_tramp_end as u64,
    );
    let len = (end - start) as usize;
    assert!(len < TR_GDT, "trampoline code too long");
    let n = smp::cores().len();
    let mut table: Vec<Entry> = Vec::with_capacity(MAX_CORES);
    for i in 0..MAX_CORES {
        // SAFETY: FW entries are plain data written by the cores.
        let f = unsafe { *(&raw const FW).cast::<smp::FwState>().add(i) };
        let mut gdtr = [0u8; 16];
        gdtr[..2].copy_from_slice(&{ f.gdt.limit }.to_le_bytes());
        gdtr[2..10].copy_from_slice(&{ f.gdt.base }.to_le_bytes());
        let stack = if i < n {
            alloc::vec![0u8; 64 << 10].leak().as_ptr() as u64 + (64 << 10)
        } else {
            0
        };
        table.push(Entry {
            apic: smp::cores().get(i).map_or(u32::MAX, |c| c.apic),
            index: i as u32,
            stack_top: if i == 0 {
                stack
            } else {
                percpu::of(i).stack_top.max(stack)
            },
            tsc: 0,
            cr0: f.cr0,
            cr4: f.cr4,
            efer: f.efer,
            gdtr,
            cs: u64::from(f.cs),
            ds: u64::from(f.ds),
            pat: f.pat,
            reserved: 0,
        });
    }
    let t = table.leak().as_ptr() as u64;
    TABLE.store(t, Ordering::Relaxed);
    // SAFETY: the page is ours (taken from the firmware below 1 MB).
    unsafe {
        let p = page as *mut u8;
        core::ptr::copy_nonoverlapping(start as *const u8, p, len);
        let w64 = |off: usize, v: u64| ((page as usize + off) as *mut u64).write_unaligned(v);
        let w32 = |off: usize, v: u32| ((page as usize + off) as *mut u32).write_unaligned(v);
        let w16 = |off: usize, v: u16| ((page as usize + off) as *mut u16).write_unaligned(v);
        w64(TR_GDT, 0);
        w64(TR_GDT + 8, 0x00CF_9A00_0000_FFFF);
        w64(TR_GDT + 16, 0x00CF_9200_0000_FFFF);
        w64(TR_GDT + 24, 0x00AF_9A00_0000_FFFF);
        w16(TR_GDTR, 31);
        w32(TR_GDTR + 2, (page + TR_GDT as u64) as u32);
        w32(TR_JMP32, (page + (pm32 - start)) as u32);
        w16(TR_JMP32 + 4, 0x08);
        w32(TR_JMP64, (page + (lm64 - start)) as u32);
        w16(TR_JMP64 + 4, 0x18);
        w64(TR_CR3, crate::paging::root());
        w64(TR_TABLE, t);
        w32(TR_COUNT, MAX_CORES as u32);
        w64(TR_ENTRY, restart_entry as *const () as u64);
    }
    PAGE.store(page, Ordering::Relaxed);
}

/// What a restarted core runs first, on its own stack, with the firmware's
/// mode registers and GDT back.
extern "sysv64" fn restart_entry(e: &Entry) -> ! {
    let i = e.index as usize;
    if crate::bsp::RESUME_CORE.load(Ordering::Acquire) == i as u64 {
        crate::bsp::resumed(i, e.tsc);
    }
    percpu::install(i, 0);
    let me = percpu::me();
    let idt = x86::DescriptorPtr {
        limit: 256 * 16 - 1,
        base: me.idt.as_ptr() as u64,
    };
    // SAFETY: this core's IDT, built at its first start.
    unsafe { x86::lidt(&idt) };
    smp::ap_local_setup();
    me.restart_tsc.store(e.tsc, Ordering::Release);
    me.nmi_park.store(0, Ordering::Relaxed);
    me.parked
        .store(me.park_request.load(Ordering::Relaxed), Ordering::Release);
    smp::forget_job(i);
    smp::serve(i)
}
