//! P-003, first mechanism (D-120): page tables typed as supervisor
//! shadow-stack pages, written only by WRSSQ. Runs on one core of each
//! kind when CPUID.07H.0:ECX.CET_SS[7] is set. QEMU has no CET: this step
//! runs only on the real machine (README, "What QEMU ran").
//!
//! Facts used, SDM 325462-093:
//! - a linear address is a shadow-stack address when the R/W flag is 0 and
//!   the dirty flag is 1 in the entry that maps the page, and R/W is 1 in
//!   every other entry of the walk (vol. 3A 5.6.1); the window's upper
//!   entries are writable;
//! - CR4.CET (bit 23) can be set only with CR0.WP set (vol. 3A 2.5);
//! - WRSSQ at CPL 0 needs CR4.CET, IA32_S_CET.SH_STK_EN and WR_SHSTK_EN
//!   (vol. 2D WRSSD/WRSSQ, operation), so supervisor shadow stacks are on
//!   while it runs: a valid SSP is set first with SETSSBSY, which checks
//!   a non-busy token holding its own address at IA32_PL0_SSP and loads
//!   SSP from it (vol. 2B SETSSBSY); CLRSSBSY clears the busy bit and SSP
//!   (vol. 2A CLRSSBSY);
//! - a fault taken with shadow stacks on pushes CS, LIP, SSP on the shadow
//!   stack and IRET checks them (vol. 3A 7.12.1.1): the recovery of `idt`
//!   rewrites the LIP with WRSSQ.
//!
//! Known risk, stated plainly: if a fault happens between enabling shadow
//! stacks and SETSSBSY (SSP not yet valid), its delivery faults again and
//! the core triple-faults: the machine resets, and the progress file makes
//! the next boot skip this step (`F cet hung`).
//!
//! Measured: `cet.wrssq` (OPS = 64 and 512 WRSSQ to a shadow-stack page,
//! the batch of one gate call), `cet.callret` (64 CALL/RET pairs with
//! supervisor shadow stacks on and off), `cet.indirect` (64 indirect CALLs
//! to an ENDBR64 target with IBT on and off); checked: `cet.walk` (a page
//! table mapped only as a shadow-stack page still serves the walk, gets
//! its accessed and dirty bits, and takes a new entry written by WRSSQ),
//! and that ordinary stores to shadow-stack pages fault.

use crate::gate::{self, Sample};
use crate::paging::{self, A, D, P, RW};
use crate::run::Ctx;
use crate::smp;
use crate::x86::{self, bit, cpuid};
use crate::clock;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use dodos_trial_multicore::rt::Mutex;

/// What one window of the shadow-stack asm does.
#[repr(C)]
struct Window {
    /// Linear address of the token (also IA32_PL0_SSP).
    token: u64,
    /// 0: WRSSQ loop of `n` from `a`; 1: one WRSSQ of `b` at `a`;
    /// 2: `n` CALL/RET pairs; 3: `n` indirect CALLs to an ENDBR64 target.
    mode: u64,
    a: u64,
    b: u64,
    n: u64,
    pmu: u64,
    /// The value written to IA32_S_CET for the window.
    cet: u64,
    /// 1: SETSSBSY at entry and CLRSSBSY at exit (shadow stacks on).
    ss: u64,
    /// Out: TSC and cycles before and after.
    t0: u64,
    c0: u64,
    t1: u64,
    c1: u64,
}

/// Runs one window. Returns the fault taken, if any.
///
/// # Safety
/// CR4.CET is set, the token page is a shadow-stack page whose token holds
/// its own address, interrupts are masked.
unsafe fn window(w: &mut Window) -> Option<x86::Fault> {
    let f: u64;
    // SAFETY: the caller's conditions; every exit path, the recovery one
    // included (label 9), writes IA32_S_CET back to 0.
    unsafe {
        asm!(
            "mov qword ptr gs:[{fv}], 0",
            "lea rax, [rip + 9f]",
            "mov gs:[{fr}], rax",
            "mov ecx, 0x6A2",
            "mov eax, [rsi + 48]",
            "xor edx, edx",
            "wrmsr",
            "cmp qword ptr [rsi + 56], 0",
            "je 3f",
            "setssbsy",
            "3:",
            "lfence",
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            "mov [rsi + 64], rax",
            "cmp qword ptr [rsi + 40], 0",
            "je 4f",
            "mov ecx, 0x40000001",
            "rdpmc",
            "shl rdx, 32",
            "or rax, rdx",
            "mov [rsi + 72], rax",
            "4:",
            "mov rax, [rsi + 8]",
            "cmp rax, 0",
            "jne 20f",
            "mov rcx, [rsi + 32]",
            "mov rdi, [rsi + 16]",
            "mov rdx, [rsi + 24]",
            "40:",
            "wrssq qword ptr [rdi], rdx",
            "add rdi, 8",
            "dec rcx",
            "jnz 40b",
            "jmp 30f",
            "20:",
            "cmp rax, 1",
            "jne 22f",
            "mov rdi, [rsi + 16]",
            "mov rdx, [rsi + 24]",
            "wrssq qword ptr [rdi], rdx",
            "jmp 30f",
            "22:",
            "cmp rax, 2",
            "jne 25f",
            "mov rcx, [rsi + 32]",
            "23:",
            "call 24f",
            "dec rcx",
            "jnz 23b",
            "jmp 30f",
            "24:",
            "ret",
            "25:",
            "mov rcx, [rsi + 32]",
            "lea rdi, [rip + 27f]",
            "26:",
            "call rdi",
            "dec rcx",
            "jnz 26b",
            "jmp 30f",
            "27:",
            "endbr64",
            "ret",
            "30:",
            "rdtscp",
            "lfence",
            "shl rdx, 32",
            "or rax, rdx",
            "mov [rsi + 80], rax",
            "cmp qword ptr [rsi + 40], 0",
            "je 5f",
            "mov ecx, 0x40000001",
            "rdpmc",
            "shl rdx, 32",
            "or rax, rdx",
            "mov [rsi + 88], rax",
            "5:",
            "cmp qword ptr [rsi + 56], 0",
            "je 9f",
            "mov rdi, [rsi]",
            "clrssbsy [rdi]",
            "9:",
            "mov ecx, 0x6A2",
            "xor eax, eax",
            "xor edx, edx",
            "wrmsr",
            "mov qword ptr gs:[{fr}], 0",
            "mov {f}, gs:[{fv}]",
            fr = const crate::percpu::FIXUP_RIP,
            fv = const crate::percpu::FIXUP_VECTOR,
            f = out(reg) f,
            in("rsi") core::ptr::from_mut(w),
            out("rax") _, out("rcx") _, out("rdx") _, out("rdi") _,
        )
    };
    x86::fault_of(f)
}

/// An ordinary 8-byte store to `va`, returning the fault.
fn plain_store(va: u64) -> Option<x86::Fault> {
    let f: u64;
    // SAFETY: the recovery of `idt` resumes at label 2.
    unsafe {
        asm!(
            "mov qword ptr gs:[{fv}], 0",
            "lea rax, [rip + 2f]",
            "mov gs:[{fr}], rax",
            "mov qword ptr [{p}], 1",
            "2:",
            "mov qword ptr gs:[{fr}], 0",
            "mov {f}, gs:[{fv}]",
            fr = const crate::percpu::FIXUP_RIP,
            fv = const crate::percpu::FIXUP_VECTOR,
            p = in(reg) va,
            f = out(reg) f,
            out("rax") _,
            options(nostack)
        )
    };
    x86::fault_of(f)
}

fn fault_text(f: Option<x86::Fault>) -> String {
    f.map_or("none".into(), |f| alloc::format!("v{}e{:#x}", f.vector, f.error))
}

struct Setup {
    token: u64,
    token_frame: u64,
}

impl Setup {
    /// Puts the token back (through the frame's identity mapping) after a
    /// window that may have left it busy.
    fn reset_token(&self) {
        // SAFETY: the token frame is ours; identity mapped and writable.
        unsafe { ((self.token_frame + 0xFF8) as *mut u64).write_volatile(self.token) };
    }
    fn window(&self, mode: u64, a: u64, b: u64, n: u64, pmu: bool, cet: u64, ss: bool) -> (Window, Option<x86::Fault>) {
        let mut w = Window {
            token: self.token,
            mode,
            a,
            b,
            n,
            pmu: u64::from(pmu),
            cet,
            ss: u64::from(ss),
            t0: 0,
            c0: 0,
            t1: 0,
            c1: 0,
        };
        // SAFETY: CR4.CET set by `measure`, token in place, interrupts
        // masked by the caller.
        let f = unsafe { window(&mut w) };
        if f.is_some() {
            self.reset_token();
        }
        (w, f)
    }
}

const SS: u64 = x86::CET_SH_STK_EN | x86::CET_WR_SHSTK_EN;

fn measure(core: usize, samples: usize, out: &Mutex<Vec<Sample>>) {
    let pmu = clock::enable_cycles();
    let info = |l: String| gate::info(out, l);
    if x86::cr0() & x86::CR0_WP == 0 {
        info(alloc::format!("F cet core={core} cr0.wp=0"));
        return;
    }
    let token_frame = paging::frame();
    let target = paging::frame();
    paging::map(10, token_frame, P | A | D);
    paging::map(11, target, P | A | D);
    let s = Setup { token: paging::window_va(10) + 0xFF8, token_frame };
    s.reset_token();
    let cr4 = x86::cr4();
    // SAFETY: WP is set (checked); nothing is enabled in IA32_S_CET yet.
    if unsafe { x86::safe_set_cr4(cr4 | x86::CR4_CET) }.is_err() {
        info(alloc::format!("F cet core={core} cr4.cet=fault"));
        return;
    }
    // SAFETY: the SSP that SETSSBSY will load; used only inside windows.
    unsafe { x86::wrmsr(x86::IA32_PL0_SSP, s.token) };
    smp::mark("cet ordinary store");
    let ordinary = plain_store(paging::window_va(11));
    smp::mark("cet one wrssq");
    let (_, f1) = s.window(1, paging::window_va(11), 0x5A5A, 0, false, SS, true);
    // SAFETY: reads the target frame through its identity mapping.
    let landed = unsafe { (target as *const u64).read_volatile() } == 0x5A5A;
    info(alloc::format!(
        "I cet.basic core={core} ordinary_store={} wrssq_fault={} wrssq_landed={}",
        fault_text(ordinary),
        fault_text(f1),
        u8::from(landed)
    ));
    if f1.is_some() {
        // SAFETY: CR4 as found.
        unsafe { x86::set_cr4(cr4) };
        return;
    }
    smp::mark("cet walk");
    walk(core, &s, out);
    let push = |name: &'static str, extra: String, t: Vec<u64>, c: Vec<u64>| {
        out.lock().expect("out").push(Sample {
            name,
            keys: alloc::format!("core={core} kind={}{extra}", smp::kind(core).name()),
            ticks: t,
            cycles: c,
        });
    };
    let mut run = |name: &'static str, extra: String, mode: u64, n: u64, cet: u64, ss: bool| {
        let (mut t, mut c) = (Vec::with_capacity(samples), Vec::with_capacity(samples));
        for _ in 0..=samples {
            let (w, f) = s.window(mode, paging::window_va(11), 1, n, pmu, cet, ss);
            if f.is_some() {
                info(alloc::format!("F {name} core={core}{extra} fault={}", fault_text(f)));
                return;
            }
            t.push(w.t1 - w.t0);
            c.push(w.c1.wrapping_sub(w.c0));
        }
        t.remove(0);
        c.remove(0);
        push(name, extra, t, if pmu { c } else { Vec::new() });
    };
    smp::mark("cet wrssq");
    run("cet.wrssq", " ops=64".into(), 0, 64, SS, true);
    run("cet.wrssq", " ops=512".into(), 0, 512, SS, true);
    smp::mark("cet callret");
    run("cet.callret", " ops=64 shstk=0".into(), 2, 64, 0, false);
    run("cet.callret", " ops=64 shstk=1".into(), 2, 64, SS, true);
    if bit(cpuid(7, 0).edx, 20) {
        smp::mark("cet indirect");
        run("cet.indirect", " ops=64 ibt=0".into(), 3, 64, 0, false);
        run("cet.indirect", " ops=64 ibt=1".into(), 3, 64, x86::CET_ENDBR_EN, false);
    }
    // SAFETY: CR4 as found; IA32_S_CET is 0 after every window.
    unsafe { x86::set_cr4(cr4) };
}

/// The page-walk check: the window's own page table, mapped only as a
/// shadow-stack page at window page 12, gets entry 20 written by WRSSQ.
fn walk(core: usize, s: &Setup, out: &Mutex<Vec<Sample>>) {
    let pt = paging::window_table();
    paging::map(12, pt, P | A | D);
    let alias = paging::window_va(12);
    let (fa, fb) = (paging::frame(), paging::frame());
    // SAFETY: two frames of ours, through their identity mappings.
    unsafe {
        (fa as *mut u64).write_volatile(0xAAAA);
        (fb as *mut u64).write_volatile(0xBBBB);
    }
    let entry = alias + 20 * 8;
    let va = paging::window_va(20);
    // SAFETY: reads entry 20 of the window's table through its identity
    // mapping.
    let read_entry = || unsafe { ((pt + 20 * 8) as *const u64).read_volatile() };
    let (_, f1) = s.window(1, entry, fa | P | RW, 0, false, SS, true);
    x86::invlpg(va);
    // SAFETY: window page 20 maps `fa` if the walk took the entry.
    let first = unsafe { (va as *const u64).read_volatile() };
    let accessed = read_entry() & A != 0;
    // SAFETY: as above, writable.
    unsafe { ((va + 8) as *mut u64).write_volatile(7) };
    let dirty = read_entry() & D != 0;
    let (_, f2) = s.window(1, entry, fb | P | RW, 0, false, SS, true);
    x86::invlpg(va);
    // SAFETY: window page 20 now maps `fb`.
    let second = unsafe { (va as *const u64).read_volatile() };
    let pt_store = plain_store(entry);
    gate::info(out, alloc::format!(
        "I cet.walk core={core} wrss_faults={}/{} walk_first={} accessed={} dirty={} walk_after_rewrite={} ordinary_store_to_table={}",
        fault_text(f1),
        fault_text(f2),
        u8::from(first == 0xAAAA),
        u8::from(accessed),
        u8::from(dirty),
        u8::from(second == 0xBBBB),
        fault_text(pt_store)
    ));
    // SAFETY: clears the test entries through the table's identity
    // mapping.
    unsafe {
        ((pt + 20 * 8) as *mut u64).write_volatile(0);
        ((pt + 12 * 8) as *mut u64).write_volatile(0);
    }
    x86::invlpg(va);
    x86::invlpg(alias);
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    if !bit(cpuid(7, 0).ecx, 7) {
        crate::out!("I cet present=0 (CPUID.07H.0:ECX.CET_SS[7] = 0): nothing to measure");
        return Ok(());
    }
    let samples = cx.reps(2000, 50);
    let results: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
    for core in gate::one_per_kind() {
        smp::run_on(&[core], 30_000, &|i| x86::without_interrupts(|| measure(i, samples, &results)))?;
        gate::report(&results);
    }
    Ok(())
}
