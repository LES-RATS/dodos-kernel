//! P-003: the cost of the write-gate mechanisms that do not need shadow
//! stacks (those are in `cet`), against a plain store, on one performance
//! and one efficiency core (kernel.md "The write gate: mechanisms", rows 2
//! to 4; gate-page-tables.md part 9, row 3).
//!
//! Every sample times `OPS` operations; the result lines give the time of
//! the whole sample (`ops=` says how many), in TSC nanoseconds and, when
//! the core counts them, in core cycles. Interrupts are masked throughout.
//!
//! - `gate.store`: `OPS` 8-byte stores to one page (the baseline).
//! - `gate.cr3`: `OPS` round trips of MOV to CR3 to a second root and back:
//!   with PCID and bit 63 set (no flush, SDM vol. 3A 5.10.4.1), and plain.
//! - `gate.cr0wp`: `OPS` times clear CR0.WP, store to a read-only page, set
//!   CR0.WP (the Nested Kernel method; WP cannot be cleared while CR4.CET is
//!   set, vol. 3A 2.5, so this runs with CET off).
//! - `gate.pks`: `OPS` times open key 1 in IA32_PKRS, store to a page of key
//!   1, close it, with WRMSR and, when present, WRMSRNS (vol. 2D WRMSRNS,
//!   opcode NP 0F 01 C6); keys in PTE bits 62:59 and PKRS AD/WD bits 2i and
//!   2i + 1 (vol. 3A 5.6.2). Also checks that a store with the key closed
//!   faults with the PK bit (bit 5) of the error code.
//! - `gate.invlpg`, `gate.invpcid`: one local invalidation and the reload
//!   of the entry by the next access.

use crate::paging::{self, A, D, P, RW};
use crate::run::Ctx;
use crate::smp::{self, CoreKind};
use crate::x86::{self, bit, cpuid};
use crate::{clock, stats};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use dodos_trial_multicore::rt::Mutex;

pub const OPS: usize = 64;

pub struct Sample {
    pub name: &'static str,
    pub keys: String,
    pub ticks: Vec<u64>,
    pub cycles: Vec<u64>,
}

/// An information line made on another core, printed by `report` (only the
/// bootstrap core prints).
pub fn info(out: &Mutex<Vec<Sample>>, line: String) {
    out.lock().expect("out").push(Sample { name: "", keys: line, ticks: Vec::new(), cycles: Vec::new() });
}

pub fn report(results: &Mutex<Vec<Sample>>) {
    for mut s in results.lock().expect("results").drain(..) {
        if s.ticks.is_empty() {
            crate::con::line(&s.keys);
            continue;
        }
        stats::report_ticks(s.name, &s.keys, &mut s.ticks);
        if s.cycles.iter().any(|&c| c != 0) {
            let k = alloc::format!("{} clock=core", s.keys);
            stats::report_unit(s.name, &k, &mut s.cycles, "cycles");
        }
    }
}

/// One performance and one efficiency worker (or the first two workers
/// when the kinds are unknown).
pub fn one_per_kind() -> Vec<usize> {
    let w = smp::workers();
    let mut out = Vec::new();
    for k in [CoreKind::P, CoreKind::E, CoreKind::Unknown] {
        if let Some(&i) = w.iter().find(|&&i| smp::kind(i) == k) {
            out.push(i);
        }
    }
    out
}

fn keys(core: usize, extra: &str) -> String {
    alloc::format!("core={core} kind={} ops={OPS}{extra}", smp::kind(core).name())
}

fn measure(core: usize, samples: usize, out: &Mutex<Vec<Sample>>) {
    smp::mark("gate pmu");
    let pmu = clock::enable_cycles();
    smp::mark("gate store");
    let push = |name: &'static str, extra: &str, (t, c): (Vec<u64>, Vec<u64>)| {
        out.lock().expect("out").push(Sample { name, keys: keys(core, extra), ticks: t, cycles: c });
    };
    let data = paging::frame();
    let ro = paging::frame();
    paging::map(2, data, P | RW | A | D);
    paging::map(3, ro, P | A | D);
    let (v2, v3) = (paging::window_va(2), paging::window_va(3));
    push("gate.store", "", clock::sample(samples, pmu, || {
        for k in 0..OPS as u64 {
            // SAFETY: window page 2 maps a frame of ours, writable.
            unsafe { ((v2 + k * 8) as *mut u64).write_volatile(k) };
        }
    }));
    smp::mark("gate cr3");
    // CR3 round trips: a second root that maps the same as ours.
    let gate_root = paging::frame();
    // SAFETY: copies our PML4 into a fresh page: same translations.
    unsafe { core::ptr::copy_nonoverlapping(paging::root() as *const u8, gate_root as *mut u8, 4096) };
    let ours = x86::cr3();
    push("gate.cr3", " pcid=0", clock::sample(samples, pmu, || {
        for _ in 0..OPS {
            // SAFETY: both roots map all the image uses.
            unsafe {
                x86::set_cr3(gate_root);
                x86::set_cr3(ours);
            }
        }
    }));
    if bit(cpuid(1, 0).ecx, 17) {
        let cr4 = x86::cr4();
        // SAFETY: CR3[11:0] = 0 (our root is page aligned, PCID 0) as
        // setting PCIDE requires (vol. 2A MOV to CR4); restored after.
        if unsafe { x86::safe_set_cr4(cr4 | x86::CR4_PCIDE) }.is_ok() {
            let (a, b) = (ours | 1 | 1 << 63, gate_root | 2 | 1 << 63);
            push("gate.cr3", " pcid=1 noflush=1", clock::sample(samples, pmu, || {
                for _ in 0..OPS {
                    // SAFETY: as above, PCIDs 1 and 2, no flush.
                    unsafe {
                        x86::set_cr3(b);
                        x86::set_cr3(a);
                    }
                }
            }));
            // SAFETY: back to PCID 0 then PCIDE off.
            unsafe {
                x86::set_cr3(ours);
                x86::set_cr4(cr4);
            }
        }
    }
    smp::mark("gate cr0wp");
    let cr0 = x86::cr0();
    if x86::cr4() & x86::CR4_CET == 0 {
        push("gate.cr0wp", "", clock::sample(samples, pmu, || {
            for k in 0..OPS as u64 {
                // SAFETY: WP off only around one store to our read-only page.
                unsafe {
                    x86::set_cr0(cr0 & !x86::CR0_WP);
                    ((v3 + k * 8) as *mut u64).write_volatile(k);
                    x86::set_cr0(cr0);
                }
            }
        }));
    }
    smp::mark("gate pks");
    pks(core, samples, pmu, data, out);
    smp::mark("gate invlpg");
    // Local invalidation of window page 2 and the walk the next store pays.
    push("gate.invlpg", "", clock::sample(samples, pmu, || {
        for k in 0..OPS as u64 {
            x86::invlpg(v2);
            // SAFETY: as above.
            unsafe { ((v2 + k * 8) as *mut u64).write_volatile(k) };
        }
    }));
    if bit(cpuid(7, 0).ebx, 10) {
        let desc: [u64; 2] = [0, v2];
        push("gate.invpcid", " type=0", clock::sample(samples, pmu, || {
            for k in 0..OPS as u64 {
                // SAFETY: individual-address invalidation of PCID 0 (type
                // 0, descriptor PCID in bits 11:0 and address in 127:64).
                unsafe {
                    asm!("invpcid {t}, [{d}]", t = in(reg) 0u64, d = in(reg) desc.as_ptr(), options(nostack));
                    ((v2 + k * 8) as *mut u64).write_volatile(k);
                }
            }
        }));
    }
}

const PKS_KEY: u64 = 1;
const WD1: u64 = 1 << (2 * PKS_KEY + 1);

#[inline(always)]
fn wrmsrns(msr: u32, v: u64) {
    // SAFETY: WRMSRNS (0F 01 C6) writes EDX:EAX to MSR ECX like WRMSR; the
    // caller checked CPUID.07H.01H:EAX.WRMSRNS[19].
    unsafe {
        asm!(".byte 0x0f, 0x01, 0xc6", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32, options(nostack))
    };
}

fn pks(core: usize, samples: usize, pmu: bool, data: u64, out: &Mutex<Vec<Sample>>) {
    smp::mark("pks enter");
    let l7 = cpuid(7, 0);
    if !bit(l7.ecx, 31) {
        info(out, alloc::format!("I gate.pks core={core} present=0"));
        return;
    }
    let keyed = paging::frame();
    smp::mark("pks frame");
    paging::map(4, keyed, P | RW | A | D | PKS_KEY << 59);
    smp::mark("pks mapped");
    let v4 = paging::window_va(4);
    let cr4 = x86::cr4();
    // SAFETY: PKRS set before CR4.PKS (see `identity`): only key 1 is
    // write-disabled, and only window page 4 has key 1; restored after.
    let on = unsafe {
        x86::safe_wrmsr(x86::IA32_PKRS, WD1).and_then(|()| x86::safe_set_cr4(cr4 | x86::CR4_PKS))
    };
    if on.is_err() {
        info(out, alloc::format!("I gate.pks core={core} present=1 enable=fault"));
        return;
    }
    smp::mark("pks closed store");
    // A store with the key closed must fault, with PK in the error code.
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
            p = in(reg) v4,
            f = out(reg) f,
            out("rax") _,
            options(nostack)
        )
    };
    let fault = x86::fault_of(f);
    info(out, alloc::format!(
        "I gate.pks core={core} present=1 enable=ok pkrs={:#x} cr4={:#x} pte={:#x} closed_store={} error={:#x} pk_bit={}",
        x86::safe_rdmsr(x86::IA32_PKRS).unwrap_or(u64::MAX),
        x86::cr4(),
        // SAFETY: reads window entry 4.
        unsafe { paging::window_pte(4).read_volatile() },
        fault.map_or("no-fault", |x| if x.vector == 14 { "page-fault" } else { "other-fault" }),
        fault.map_or(0, |x| x.error),
        fault.map_or(0, |x| u8::from(x.error >> 5 & 1 == 1))
    ));
    smp::mark("pks loops");
    let push = |name: &'static str, extra: &str, (t, c): (Vec<u64>, Vec<u64>)| {
        out.lock().expect("out").push(Sample { name, keys: keys(core, extra), ticks: t, cycles: c });
    };
    push("gate.pks", " write=wrmsr", clock::sample(samples, pmu, || {
        for k in 0..OPS as u64 {
            // SAFETY: opens key 1 around one store to its page.
            unsafe {
                x86::wrmsr(x86::IA32_PKRS, 0);
                ((v4 + k * 8) as *mut u64).write_volatile(k);
                x86::wrmsr(x86::IA32_PKRS, WD1);
            }
        }
    }));
    if bit(cpuid(7, 1).eax, 19) {
        push("gate.pks", " write=wrmsrns", clock::sample(samples, pmu, || {
            for k in 0..OPS as u64 {
                wrmsrns(x86::IA32_PKRS, 0);
                // SAFETY: key 1 is open.
                unsafe { ((v4 + k * 8) as *mut u64).write_volatile(k) };
                wrmsrns(x86::IA32_PKRS, WD1);
            }
        }));
    }
    let _ = data;
    // SAFETY: PKRS 0 then CR4 as found: every access permitted again.
    unsafe {
        x86::wrmsr(x86::IA32_PKRS, 0);
        x86::set_cr4(cr4);
    }
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let samples = cx.reps(2000, 50);
    let results: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
    for core in one_per_kind() {
        smp::run_on(&[core], 30_000, &|i| x86::without_interrupts(|| measure(i, samples, &results)))?;
        report(&results);
    }
    Ok(())
}
