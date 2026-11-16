//! VMX: entering and leaving VMX operation (for `init`), and the cost of
//! two-stage translation under a small monitor (D-117 point 3, the optional
//! hardening layer; fencing-without-cooperation.md part 7, item 3: "a
//! TLB-miss microbenchmark (random walk over 2^16 pages, as Dune) ... with
//! 2 MB and 1 GB stage-2 blocks").
//!
//! The monitor is the smallest that runs the image's own code as a guest
//! on the same core: identity EPT over the RAM of the firmware's memory map
//! (write-back), the guest's state copied from the host's, no exit but
//! VMCALL (and the unconditional ones, which the guest code avoids: no
//! CPUID, no MSR access). The same workloads run natively and in the guest,
//! timed by the guest itself with RDTSC (no RDTSC exiting, TSC offset 0):
//! `random4k`, a pointer chase over 2^16 pages of 4 KB mapped with 4 KB
//! guest pages (every hop a TLB miss; under EPT, a two-dimensional walk);
//! `seq4k`, a sequential read of 64 MB through the same mapping; `compute`,
//! a register-only loop. Lines `ept.<workload> mode=native|ept2m|ept1g`.
//! QEMU emulates no VMX: this runs only on the real machine (README).
//!
//! SDM 325462-093, vol. 3C/3D: enabling (26.7: CR4.VMXE, IA32_FEATURE_CONTROL
//! lock bit 0 and bit 2), VMXON region (27.11.5: 4 KB aligned, revision
//! identifier in bits 30:0 from IA32_VMX_BASIC), capability MSRs and the
//! allowed settings of each control (appendix A: allowed-0 settings in bits
//! 31:0, allowed-1 in 63:32; the TRUE MSRs when IA32_VMX_BASIC bit 55 is
//! set), field encodings (appendix B), EPTP (table 27-9: memory type 6 =
//! WB in bits 2:0, walk length minus one in 5:3), EPT entries (tables 31-2
//! to 31-7: read, write, execute in bits 2:0; memory type in 5:3 and bit 7
//! for a large page), controls ("activate secondary controls" primary bit
//! 31, "enable EPT" secondary bit 1, "host address-space size" exit bit 9,
//! "IA-32e mode guest" entry bit 9), exit reasons (2 triple fault, 18
//! VMCALL, 48 EPT violation, 49 EPT misconfiguration; appendix C).

use crate::efi;
use crate::run::Ctx;
use crate::smp;
use crate::{paging, percpu, stats, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};
use dodos_trial_multicore::rt::Mutex;

const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_EXIT_CTLS: u32 = 0x483;
const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;
const IA32_VMX_TRUE_PINBASED_CTLS: u32 = 0x48D;
const IA32_VMX_TRUE_PROCBASED_CTLS: u32 = 0x48E;
const IA32_VMX_TRUE_EXIT_CTLS: u32 = 0x48F;
const IA32_VMX_TRUE_ENTRY_CTLS: u32 = 0x490;

/// VMX is usable: CPUID.01H:ECX.VMX[5] and IA32_FEATURE_CONTROL locked
/// with VMX outside SMX enabled (bits 0 and 2).
pub fn available() -> bool {
    x86::bit(x86::cpuid(1, 0).ecx, 5)
        && x86::safe_rdmsr(x86::IA32_FEATURE_CONTROL).is_ok_and(|f| f & 0b101 == 0b101)
}

static REGIONS: [AtomicU64; percpu::MAX_CORES] = [const { AtomicU64::new(0) }; percpu::MAX_CORES];

fn vmx_ok(flags: u64) -> Result<(), String> {
    // CF (bit 0): VMfailInvalid; ZF (bit 6): VMfailValid.
    if flags & 1 != 0 {
        return Err("VMfailInvalid".into());
    }
    if flags & 0x40 != 0 {
        return Err(alloc::format!("VMfailValid error {}", vmread(0x4400).unwrap_or(u64::MAX)));
    }
    Ok(())
}

/// Enters VMX operation on the calling core.
pub fn on() -> Result<(), String> {
    if !available() {
        return Err("VMX not available".into());
    }
    let i = percpu::my_index();
    let mut region = REGIONS[i].load(Ordering::Relaxed);
    if region == 0 {
        region = paging::frame();
        REGIONS[i].store(region, Ordering::Relaxed);
    }
    // SAFETY: the capability MSRs exist with VMX; the VMXON region is a
    // 4 KB frame of ours; CR0 and CR4 are set within their fixed bits.
    unsafe {
        let basic = x86::rdmsr(IA32_VMX_BASIC);
        (region as *mut u32).write_volatile(basic as u32 & 0x7FFF_FFFF);
        let cr0 = (x86::cr0() | x86::rdmsr(IA32_VMX_CR0_FIXED0)) & x86::rdmsr(IA32_VMX_CR0_FIXED1);
        let cr4 = (x86::cr4() | x86::CR4_VMXE | x86::rdmsr(IA32_VMX_CR4_FIXED0)) & x86::rdmsr(IA32_VMX_CR4_FIXED1);
        x86::set_cr0(cr0);
        x86::set_cr4(cr4);
        let flags: u64;
        asm!("vmxon [{p}]", "pushfq", "pop {f}", p = in(reg) &region, f = out(reg) flags);
        vmx_ok(flags)
    }
}

/// Leaves VMX operation and clears CR4.VMXE.
pub fn off() {
    // SAFETY: in VMX root operation (after `on`); VMXE may be cleared after
    // VMXOFF (26.7).
    unsafe {
        asm!("vmxoff");
        x86::set_cr4(x86::cr4() & !x86::CR4_VMXE);
    }
}

fn vmwrite(field: u64, v: u64) -> Result<(), String> {
    let flags: u64;
    // SAFETY: a VMCS is current; a write to a field of it.
    unsafe { asm!("vmwrite {f}, {v}", "pushfq", "pop {r}", f = in(reg) field, v = in(reg) v, r = out(reg) flags) };
    vmx_ok(flags).map_err(|e| alloc::format!("vmwrite {field:#x}: {e}"))
}

fn vmread(field: u64) -> Result<u64, String> {
    let (v, flags): (u64, u64);
    // SAFETY: a VMCS is current; a read of a field of it.
    unsafe { asm!("vmread {v}, {f}", "pushfq", "pop {r}", f = in(reg) field, v = out(reg) v, r = out(reg) flags) };
    if flags & 0x41 != 0 { Err(alloc::format!("vmread {field:#x} failed")) } else { Ok(v) }
}

/// A control value: wanted bits, forced to what the capability MSR allows.
fn control(msr: u32, true_msr: u32, want: u32) -> Result<u32, String> {
    // SAFETY: capability MSRs, present with VMX.
    let (basic, cap) = unsafe { (x86::rdmsr(IA32_VMX_BASIC), x86::rdmsr(msr)) };
    let cap = if basic >> 55 & 1 == 1 && true_msr != 0 {
        // SAFETY: the TRUE MSRs exist when bit 55 is set.
        unsafe { x86::rdmsr(true_msr) }
    } else {
        cap
    };
    let (must, may) = (cap as u32, (cap >> 32) as u32);
    if want & !may != 0 {
        return Err(alloc::format!("control bits {:#x} not allowed by MSR {msr:#x}", want & !may));
    }
    Ok(want | must)
}

/// Kinds of memory the image counts as RAM for the identity EPT.
fn is_ram(kind: u32) -> bool {
    matches!(kind, 1..=7 | 9 | 10 | 14)
}

/// The RAM ranges of the firmware's memory map, sorted.
fn ram_ranges() -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = crate::warm::memory_map_pub()
        .iter()
        .filter(|d| is_ram(d.kind))
        .map(|d| (d.physical_start, d.physical_start + d.number_of_pages * 4096))
        .collect();
    v.sort_unstable();
    v
}

/// How much of [s, s + len) is RAM: 0 none, 1 part, 2 all.
fn cover(ram: &[(u64, u64)], s: u64, len: u64) -> u8 {
    let e = s + len;
    let got: u64 = ram.iter().map(|&(a, b)| b.min(e).saturating_sub(a.max(s))).sum();
    if got == 0 { 0 } else if got >= len { 2 } else { 1 }
}

const EPT_RWX: u64 = 0b111;
const EPT_WB: u64 = 6 << 3;
const EPT_LARGE: u64 = 1 << 7;

/// Builds identity EPT tables over the RAM; `gb` maps whole RAM gigabytes
/// with 1 GB pages, otherwise with 2 MB pages. Returns the EPTP.
fn build_ept(gb: bool) -> u64 {
    let ram = ram_ranges();
    let top = ram.iter().map(|r| r.1).max().unwrap_or(0);
    let pml4 = paging::frame();
    let set = |t: u64, i: u64, v: u64| {
        // SAFETY: tables of ours, identity mapped.
        unsafe { ((t + i * 8) as *mut u64).write_volatile(v) };
    };
    for g in 0..top.div_ceil(1 << 30) {
        let base = g << 30;
        let c = cover(&ram, base, 1 << 30);
        if c == 0 {
            continue;
        }
        let l4 = g >> 9;
        // SAFETY: reads an entry of our own table.
        let mut pdpt = unsafe { ((pml4 + l4 * 8) as *const u64).read_volatile() } & paging::ADDR;
        if pdpt == 0 {
            pdpt = paging::frame();
            set(pml4, l4, pdpt | EPT_RWX);
        }
        if c == 2 && gb {
            set(pdpt, g & 511, base | EPT_RWX | EPT_WB | EPT_LARGE);
            continue;
        }
        let pd = paging::frame();
        set(pdpt, g & 511, pd | EPT_RWX);
        for m in 0..512u64 {
            let b2 = base + (m << 21);
            match cover(&ram, b2, 1 << 21) {
                2 => set(pd, m, b2 | EPT_RWX | EPT_WB | EPT_LARGE),
                1 => {
                    let pt = paging::frame();
                    set(pd, m, pt | EPT_RWX);
                    for k in 0..512u64 {
                        let b4 = b2 + (k << 12);
                        if cover(&ram, b4, 4096) == 2 {
                            set(pt, k, b4 | EPT_RWX | EPT_WB);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    pml4 | 6 | 3 << 3
}

/// The workloads, run natively or by the guest.
#[repr(C)]
struct Work {
    kind: u64,
    base: u64,
    pages: u64,
    hops: u64,
    out_ticks: u64,
    out_sum: u64,
}

static WORK: Mutex<Work> = Mutex::new(Work { kind: 0, base: 0, pages: 0, hops: 0, out_ticks: 0, out_sum: 0 });
static WORK_PTR: AtomicU64 = AtomicU64::new(0);

fn workload(w: &mut Work) {
    let t0 = x86::tsc_begin();
    let mut sum = 0u64;
    match w.kind {
        0 => {
            let mut p = w.base;
            for _ in 0..w.hops {
                // SAFETY: every page of the chase holds the address of the
                // next one, through the 4 KB mapping built by `prepare`.
                p = unsafe { (p as *const u64).read_volatile() };
            }
            sum = p;
        }
        1 => {
            let bytes = (64 << 20).min(w.pages * 4096);
            let mut a = w.base;
            while a < w.base + bytes {
                // SAFETY: inside the mapped buffer.
                sum = sum.wrapping_add(unsafe { (a as *const u64).read_volatile() });
                a += 64;
            }
        }
        _ => {
            let mut x = 0x9E37_79B9_7F4A_7C15u64;
            for _ in 0..w.hops {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
            }
            sum = x;
        }
    }
    w.out_ticks = x86::tsc_end() - t0;
    w.out_sum = sum;
}

extern "sysv64" fn guest_main() {
    // SAFETY: the BSP-side code set WORK_PTR to a live `Work` before the
    // VM entry and reads it only after the VM exit.
    let w = unsafe { &mut *(WORK_PTR.load(Ordering::Acquire) as *mut Work) };
    workload(w);
}

global_asm!(
    ".global dodos_guest_entry",
    "dodos_guest_entry:",
    "and rsp, -16",
    "call {main}",
    "vmcall",
    "ud2",
    main = sym guest_main,
);

unsafe extern "C" {
    fn dodos_guest_entry();
}

/// One run of `w` in a guest under EPT `eptp`. Returns the exit reason.
fn run_guest(eptp: u64, guest_stack: u64, tss: u64) -> Result<u64, String> {
    // SAFETY: capability MSR present with VMX.
    let basic = unsafe { x86::rdmsr(IA32_VMX_BASIC) };
    let vmcs = paging::frame();
    // SAFETY: a fresh 4 KB frame; revision identifier in bits 30:0.
    unsafe { (vmcs as *mut u32).write_volatile(basic as u32 & 0x7FFF_FFFF) };
    let flags: u64;
    // SAFETY: in VMX root operation; clears and loads our VMCS.
    unsafe {
        asm!("vmclear [{p}]", "vmptrld [{p}]", "pushfq", "pop {f}", p = in(reg) &vmcs, f = out(reg) flags);
    }
    vmx_ok(flags)?;
    let pin = control(IA32_VMX_PINBASED_CTLS, IA32_VMX_TRUE_PINBASED_CTLS, 0)?;
    let proc1 = control(IA32_VMX_PROCBASED_CTLS, IA32_VMX_TRUE_PROCBASED_CTLS, 1 << 31)?;
    let proc2 = control(IA32_VMX_PROCBASED_CTLS2, 0, 1 << 1)?;
    let exit = control(IA32_VMX_EXIT_CTLS, IA32_VMX_TRUE_EXIT_CTLS, 1 << 9)?;
    let entry = control(IA32_VMX_ENTRY_CTLS, IA32_VMX_TRUE_ENTRY_CTLS, 1 << 9)?;
    let gdt = x86::sgdt();
    let idt = x86::sidt();
    let cs = u64::from(x86::cs());
    let ds: u64;
    // SAFETY: reads DS.
    unsafe { asm!("mov {0:x}, ds", out(reg) ds) };
    let gs_base = x86::safe_rdmsr(x86::IA32_GS_BASE).unwrap_or(0);
    let fields: Vec<(u64, u64)> = alloc::vec![
        (0x4000, u64::from(pin)),
        (0x4002, u64::from(proc1)),
        (0x401E, u64::from(proc2)),
        (0x400C, u64::from(exit)),
        (0x4012, u64::from(entry)),
        (0x4004, 0),
        (0x201A, eptp),
        (0x2800, u64::MAX),
        (0x6800, x86::cr0()),
        (0x6802, x86::cr3()),
        (0x6804, x86::cr4()),
        (0x6004, x86::cr0()),
        (0x6006, x86::cr4()),
        (0x681A, 0x400),
        (0x681C, guest_stack),
        (0x681E, dodos_guest_entry as usize as u64),
        (0x6820, 0x2),
        (0x0802, cs),
        (0x4802, 0xFFFF_FFFF),
        (0x4816, 0xA09B),
        (0x6808, 0),
        (0x0800, ds),
        (0x0804, ds),
        (0x0806, ds),
        (0x0808, ds),
        (0x080A, ds),
        (0x4800, 0xFFFF_FFFF),
        (0x4804, 0xFFFF_FFFF),
        (0x4806, 0xFFFF_FFFF),
        (0x4808, 0xFFFF_FFFF),
        (0x480A, 0xFFFF_FFFF),
        (0x4814, 0xC093),
        (0x4818, 0xC093),
        (0x481A, 0xC093),
        (0x481C, 0xC093),
        (0x481E, 0xC093),
        (0x6810, gs_base),
        (0x480C, 0),
        (0x4820, 1 << 16),
        (0x080E, 0x40),
        (0x480E, 0x67),
        (0x4822, 0x8B),
        (0x6814, tss),
        (0x4810, u64::from(gdt.limit)),
        (0x6816, gdt.base),
        (0x4812, u64::from(idt.limit)),
        (0x6818, idt.base),
        // Every other field VM entry checks, written so that nothing is
        // left to the VMCS's initial contents (unspecified after VMCLEAR).
        (0x080C, 0),
        (0x6806, 0),
        (0x680A, 0),
        (0x680C, 0),
        (0x680E, 0),
        (0x6812, 0),
        (0x2802, 0),
        (0x482A, 0),
        (0x6824, 0),
        (0x6826, 0),
        (0x6822, 0),
        (0x4824, 0),
        (0x4826, 0),
        (0x4016, 0),
        (0x400A, 0),
        (0x400E, 0),
        (0x4010, 0),
        (0x4014, 0),
        (0x4006, 0),
        (0x4008, 0),
        (0x6000, 0),
        (0x6002, 0),
        (0x2010, 0),
    ];
    for (f, v) in fields {
        vmwrite(f, v)?;
    }
    let host: [(u64, u64); 13] = [
        (0x6C00, x86::cr0()),
        (0x6C02, x86::cr3()),
        (0x6C04, x86::cr4()),
        (0x0C00, ds),
        (0x0C02, cs),
        (0x0C04, ds),
        (0x0C06, ds),
        (0x0C08, ds),
        (0x0C0A, ds),
        (0x0C0C, 0x40),
        (0x6C08, gs_base),
        (0x6C0A, tss),
        (0x6C0C, gdt.base),
    ];
    for (f, v) in host {
        vmwrite(f, v)?;
    }
    vmwrite(0x6C0E, idt.base)?;
    vmwrite(0x6C06, 0)?;
    vmwrite(0x4C00, 0)?;
    vmwrite(0x6C10, 0)?;
    vmwrite(0x6C12, 0)?;
    let status: u64;
    // SAFETY: VMLAUNCH with host RSP and RIP pointing back here; the
    // registers the compiler may keep are pushed before and popped after
    // the exit; the vector registers are saved by FXSAVE around the guest.
    unsafe {
        asm!(
            "push rbp",
            "push rbx",
            "sub rsp, 528",
            "mov rax, rsp",
            "add rax, 15",
            "and rax, -16",
            "fxsave64 [rax]",
            "push rax",
            "mov rax, 0x6C14",
            "vmwrite rax, rsp",
            "lea rdx, [rip + 33f]",
            "mov rax, 0x6C16",
            "vmwrite rax, rdx",
            "vmlaunch",
            "pushfq",
            "pop rax",
            "and rax, 0x41",
            "or rax, 0x100",
            "jmp 34f",
            "33:",
            "xor eax, eax",
            "34:",
            "pop rdx",
            "fxrstor64 [rdx]",
            "add rsp, 528",
            "pop rbx",
            "pop rbp",
            out("rax") status,
            out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
            out("r8") _, out("r9") _, out("r10") _, out("r11") _,
            out("r12") _, out("r13") _, out("r14") _, out("r15") _,
        )
    };
    if status != 0 {
        return Err(alloc::format!(
            "VMLAUNCH failed (flags {:#x}, VM-instruction error {})",
            status & 0x41,
            vmread(0x4400).unwrap_or(u64::MAX)
        ));
    }
    let reason = vmread(0x4402)? & 0xFFFF;
    // SAFETY: clears the VMCS before its frame is forgotten.
    unsafe { asm!("vmclear [{p}]", p = in(reg) &vmcs) };
    Ok(reason)
}

const BUFFER_PAGES: u64 = 1 << 16;
const NAMES: [&str; 3] = ["random4k", "seq4k", "compute"];

/// The buffer of the workloads, mapped with 4 KB pages at 1 TB (PML4 entry
/// 2), with a random cyclic chase written in it.
fn prepare() -> u64 {
    let bytes = BUFFER_PAGES * 4096;
    let l = core::alloc::Layout::from_size_align(bytes as usize, 1 << 21).expect("layout");
    // SAFETY: a non-zero layout; leaked on purpose.
    let phys = unsafe { alloc::alloc::alloc_zeroed(l) } as u64;
    assert!(phys != 0, "no memory for the EPT buffer");
    let va = paging::map_4k_range(2, phys, bytes);
    let mut order: Vec<u64> = (0..BUFFER_PAGES).collect();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..order.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        order.swap(i, (s % (i as u64 + 1)) as usize);
    }
    for k in 0..order.len() {
        let (from, to) = (order[k], order[(k + 1) % order.len()]);
        // SAFETY: inside the buffer, through its identity mapping.
        unsafe { ((phys + from * 4096) as *mut u64).write_volatile(va + to * 4096) };
    }
    va + order[0] * 4096
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    if !available() {
        crate::out!("I ept present=0: VMX absent or disabled by the firmware (IA32_FEATURE_CONTROL)");
        return Ok(());
    }
    // SAFETY: capability MSR present with VMX.
    let cap = unsafe { x86::rdmsr(IA32_VMX_EPT_VPID_CAP) };
    crate::out!(
        "I ept cap={cap:#x} walk4={} wb={} page2m={} page1g={}",
        cap >> 6 & 1,
        cap >> 14 & 1,
        cap >> 16 & 1,
        cap >> 17 & 1
    );
    if cap >> 6 & 1 == 0 || cap >> 14 & 1 == 0 {
        return Err("EPT without a 4-level walk or write-back tables".into());
    }
    let start = prepare();
    let runs = cx.reps(10, 2);
    let modes: Vec<(&str, Option<u64>)> = {
        let mut m = alloc::vec![("native", None)];
        if cap >> 16 & 1 == 1 {
            m.push(("ept2m", Some(build_ept(false))));
        }
        if cap >> 17 & 1 == 1 {
            m.push(("ept1g", Some(build_ept(true))));
        }
        m
    };
    for core in crate::gate::one_per_kind() {
        let lines: Mutex<Vec<(String, Vec<u64>)>> = Mutex::new(Vec::new());
        smp::run_on(&[core], 600_000, &|i| {
            x86::without_interrupts(|| {
                let on_ok = on();
                if let Err(e) = &on_ok {
                    lines.lock().expect("l").push((alloc::format!("F ept core={i} vmxon={e}"), Vec::new()));
                    return;
                }
                let stack = alloc::vec![0u8; 64 << 10].leak().as_ptr() as u64 + (64 << 10);
                let tss = paging::frame();
                for (kind, name) in NAMES.iter().enumerate() {
                    for (mode, eptp) in &modes {
                        let mut v = Vec::new();
                        for _ in 0..=runs {
                            let mut w = WORK.lock().expect("work");
                            *w = Work { kind: kind as u64, base: start, pages: BUFFER_PAGES, hops: 1 << 20, out_ticks: 0, out_sum: 0 };
                            match eptp {
                                None => workload(&mut w),
                                Some(e) => {
                                    WORK_PTR.store(core::ptr::from_mut(&mut *w) as u64, Ordering::Release);
                                    match run_guest(*e, stack, tss) {
                                        Ok(18) => {}
                                        Ok(r) => {
                                            let qual = vmread(0x6400).unwrap_or(0);
                                            let gpa = vmread(0x2400).unwrap_or(0);
                                            lines.lock().expect("l").push((alloc::format!("F ept core={i} mode={mode} exit_reason={r} qualification={qual:#x} gpa={gpa:#x}"), Vec::new()));
                                            off();
                                            return;
                                        }
                                        Err(e) => {
                                            lines.lock().expect("l").push((alloc::format!("F ept core={i} mode={mode} {e}"), Vec::new()));
                                            off();
                                            return;
                                        }
                                    }
                                }
                            }
                            v.push(w.out_ticks);
                        }
                        v.remove(0);
                        let k = alloc::format!("core={i} kind={} mode={mode}", smp::kind(i).name());
                        lines.lock().expect("l").push((alloc::format!("{name}|{k}"), v));
                    }
                }
                off();
            });
        })?;
        for (k, mut v) in lines.lock().expect("l").drain(..) {
            if v.is_empty() {
                crate::con::line(&k);
            } else if let Some((name, keys)) = k.split_once('|') {
                stats::report_ticks(&alloc::format!("ept.{name}"), keys, &mut v);
            }
        }
    }
    let _ = efi::LOADER_DATA;
    Ok(())
}
