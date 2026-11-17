//! The bootstrap core (D-117, the open point "The bootstrap core on
//! x86-64"; kernel-interface.md "The fence", step 2: the bootstrap core is
//! stopped and confirmed by the NMI of step 1 alone, and restarted through
//! the stub without INIT).
//!
//! - `bsp.nmi`: an NMI from a worker to the bootstrap core spinning with
//!   interrupts masked: from the send to its handler's count.
//! - `bsp.park`: the bootstrap core confirms a park request and halts with
//!   interrupts masked; a worker raises the start request and sends an NMI;
//!   from that NMI to the bootstrap core running again (no INIT).
//! - `bsp.flag`: whether IA32_APIC_BASE bit 8 (the BSP flag, "R/W" in SDM
//!   vol. 4 table 2-2) reads back cleared after a write of 0, then set.
//! - `bsp.init`: with the flag cleared, a worker sends INIT, 10 ms, SIPI,
//!   200 µs, SIPI to the bootstrap core. "Subsequent INITs ... examine its
//!   BSP flag ... to determine whether it should execute the BIOS boot-
//!   strap code (if it is the BSP) or enter a wait-for-SIPI state (if it is
//!   an AP)" (vol. 3A 11.4.2). If clearing the flag makes it an AP, it
//!   comes back through the trampoline, which restores the firmware's
//!   state (registers saved here, descriptor tables, page tables, the
//!   local APIC's registers, which INIT resets: vol. 3A 13.12.5.1) and
//!   resumes the run: then the bootstrap core is of fence class 2. If not,
//!   it runs the firmware's reset code: the machine restarts or hangs, the
//!   progress file records it, and the next boot reports `bsp-init hung`.
//!   Only after `bsp.flag` showed the flag can be cleared.
//! - With `bsp_init_flag_set=1` only: INIT to the bootstrap core with its
//!   flag set, the case the SDM describes; it ends the run.

use crate::run::Ctx;
use crate::{apic, out, paging, percpu, sipi, smp, stats, tsc, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use dodos_trial_multicore::rt::Mutex;

/// The bootstrap core's state across INIT: stack, frame and return point
/// inside `init_cycle`, and the local APIC registers.
#[repr(C)]
struct Saved {
    rbx: u64,
    rbp: u64,
    rsp: u64,
    rip: u64,
    armed: AtomicU64,
    resumed_tsc: AtomicU64,
    apic: [(u32, u32); 10],
}

static mut SAVED: Saved = Saved {
    rbx: 0,
    rbp: 0,
    rsp: 0,
    rip: 0,
    armed: AtomicU64::new(0),
    resumed_tsc: AtomicU64::new(0),
    apic: [(0, 0); 10],
};

static RESENDS: AtomicU64 = AtomicU64::new(0);

fn saved() -> *mut Saved {
    &raw mut SAVED
}

/// The APIC registers put back after INIT, in this order (the timer's
/// initial count last: writing it starts the firmware's timer again).
const APIC_REGS: [u32; 10] = [
    apic::SVR,
    apic::TPR,
    apic::LVT_LINT0,
    apic::LVT_LINT1,
    apic::LVT_ERROR,
    apic::LVT_THERMAL,
    apic::LVT_PERF,
    apic::TIMER_DIVIDE,
    apic::LVT_TIMER,
    apic::TIMER_INITIAL,
];

fn save_apic() {
    let s = saved();
    for (k, &r) in APIC_REGS.iter().enumerate() {
        // SAFETY: only the bootstrap core touches SAVED, outside INIT.
        unsafe { (*s).apic[k] = (r, apic::read(r)) };
    }
}

/// The core whose restart goes back into `init_cycle` (the bootstrap
/// core, or a worker for the rehearsal); `u64::MAX` for none.
pub static RESUME_CORE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Called by the trampoline's Rust entry on `RESUME_CORE`: puts back what
/// INIT cleared and jumps into `init_cycle` after its spin loop.
pub fn resumed(i: usize, tsc: u64) -> ! {
    percpu::install(i, 0);
    let me = percpu::me();
    let idt = x86::DescriptorPtr {
        limit: 256 * 16 - 1,
        base: me.idt.as_ptr() as u64,
    };
    // SAFETY: this core's IDT; the page tables it used before INIT (the
    // firmware's on the bootstrap core, saved by `smp::save_fw` in
    // `setup`; the image's on a worker).
    unsafe {
        x86::lidt(&idt);
        x86::set_cr3(if i == 0 {
            (*(&raw const smp::FW).cast::<smp::FwState>()).cr3
        } else {
            paging::root()
        });
    }
    if apic::x2() {
        apic::enter_x2apic_local();
    }
    let s = saved();
    // SAFETY: SAVED was written by `init_cycle` before the INIT; the jump
    // goes back into that function's frame, still on the stack.
    unsafe {
        for &(r, v) in &(*s).apic {
            apic::write(r, v);
        }
        // A timer in TSC-deadline mode (LVT bits 18:17 = 10b) is armed by a
        // write of IA32_TSC_DEADLINE, which INIT leaves as it was (vol. 3A
        // table 12-1) but which may have passed: arm it 1 ms from now so
        // that the firmware's tick comes back and re-arms itself.
        let lvt = (*s)
            .apic
            .iter()
            .find(|(r, _)| *r == apic::LVT_TIMER)
            .map_or(0, |&(_, v)| v);
        if i == 0 && lvt >> 17 & 0b11 == 0b10 && lvt >> 16 & 1 == 0 {
            x86::wrmsr(
                x86::IA32_TSC_DEADLINE,
                x86::rdtsc() + tsc::ns_to_ticks(1_000_000),
            );
        }
        (*s).resumed_tsc.store(tsc, Ordering::Release);
        me.parked
            .store(me.park_request.load(Ordering::Relaxed), Ordering::Release);
        asm!(
            "mov rsp, [{s} + 16]",
            "mov rbp, [{s} + 8]",
            "mov rbx, [{s} + 0]",
            "jmp qword ptr [{s} + 24]",
            s = in(reg) s,
            options(noreturn)
        )
    }
}

/// Clears the BSP flag, arms the director and spins with interrupts masked
/// until INIT; returns after the trampoline brought the core back.
fn init_cycle() {
    let s = saved();
    save_apic();
    // SAFETY: the stack pointer, frame pointer and RBX are saved and put
    // back by `resumed` before it jumps to label 3; every other register is
    // declared clobbered, so the compiler keeps nothing in them across.
    unsafe {
        asm!(
            "mov [{s} + 0], rbx",
            "mov [{s} + 8], rbp",
            "mov [{s} + 16], rsp",
            "lea rax, [rip + 3f]",
            "mov [{s} + 24], rax",
            "mov qword ptr [{s} + 32], 1",
            "2:",
            "pause",
            "jmp 2b",
            "3:",
            s = in(reg) s,
            out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
            out("r8") _, out("r9") _, out("r10") _, out("r11") _, out("r12") _,
            out("r13") _, out("r14") _, out("r15") _,
            clobber_abi("C"),
        )
    };
}

fn director() -> Option<usize> {
    smp::workers().first().copied()
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let d = director().ok_or("no worker to direct")?;
    let bsp_apic = smp::cores()[0].apic;
    let me = percpu::me();
    // NMI latency to the bootstrap core.
    let n = cx.reps(1000, 20);
    let lat: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    let stop = AtomicU64::new(0);
    smp::run_on(&[0, d], 60_000, &|i| {
        if i == 0 {
            x86::without_interrupts(|| {
                while stop.load(Ordering::Acquire) == 0 {
                    x86::pause();
                }
            });
        } else {
            let b = percpu::of(0);
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                tsc::spin_ns(20_000);
                let c0 = b.nmi_count.load(Ordering::Acquire);
                let t0 = x86::tsc_begin();
                apic::send(bsp_apic, apic::NMI, 0);
                while b.nmi_count.load(Ordering::Acquire) == c0 {
                    x86::pause();
                }
                v.push(b.nmi_tsc.load(Ordering::Acquire).wrapping_sub(t0));
            }
            *lat.lock().expect("lat") = v;
            stop.store(1, Ordering::Release);
        }
    })?;
    stats::report_ticks(
        "bsp.nmi",
        "director=0 from=worker",
        &mut lat.lock().expect("lat"),
    );
    crate::con::flush();
    // Park without INIT, woken by NMI and the start request.
    let wake: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    let reps = cx.reps(200, 5);
    let resumed_at = AtomicU64::new(0);
    smp::run_on(&[0, d], 120_000, &|i| {
        if i == 0 {
            x86::without_interrupts(|| {
                for _ in 0..reps {
                    // Confirm the request this core reads, then halt until
                    // a new start request (an NMI wakes the halt).
                    let last = me.parked.load(Ordering::Acquire);
                    while me.park_request.load(Ordering::Acquire) == last {
                        x86::pause();
                    }
                    let s0 = me.start_request.load(Ordering::Acquire);
                    me.parked
                        .store(me.park_request.load(Ordering::Acquire), Ordering::Release);
                    while me.start_request.load(Ordering::Acquire) == s0 {
                        x86::hlt();
                    }
                    resumed_at.store(x86::rdtsc(), Ordering::Release);
                }
            });
        } else {
            let b = percpu::of(0);
            let mut v = Vec::with_capacity(reps);
            let mut resends = 0u64;
            for _ in 0..reps {
                let req = b.park_request.fetch_add(1, Ordering::AcqRel) + 1;
                while b.parked.load(Ordering::Acquire) != req {
                    x86::pause();
                }
                tsc::spin_ns(100_000);
                resumed_at.store(0, Ordering::Release);
                let t0 = x86::tsc_begin();
                b.start_request.fetch_add(1, Ordering::AcqRel);
                // An NMI that lands between the core's check and its HLT
                // is lost: send again every millisecond, as the fence's
                // driver repeats its steps (kernel-interface.md, step 5).
                let mut r = 0;
                while r == 0 {
                    apic::send(bsp_apic, apic::NMI, 0);
                    let end = x86::rdtsc() + tsc::ns_to_ticks(1_000_000);
                    while r == 0 && x86::rdtsc() < end {
                        r = resumed_at.load(Ordering::Acquire);
                    }
                    if r == 0 {
                        resends += 1;
                    }
                }
                v.push(r.wrapping_sub(t0));
            }
            RESENDS.store(resends, Ordering::Relaxed);
            *wake.lock().expect("wake") = v;
        }
    })?;
    let k = alloc::format!(
        "wake=nmi+start_request resends={}",
        RESENDS.load(Ordering::Relaxed)
    );
    stats::report_ticks("bsp.park", &k, &mut wake.lock().expect("wake"));
    crate::con::flush();
    // Can the BSP flag be cleared?
    // SAFETY: IA32_APIC_BASE exists; the flag is written back at once.
    let base = unsafe { x86::rdmsr(x86::IA32_APIC_BASE) };
    // SAFETY: clears only bit 8, then restores the value read.
    let w = unsafe { x86::safe_wrmsr(x86::IA32_APIC_BASE, base & !x86::APIC_BASE_BSP) };
    let read = x86::safe_rdmsr(x86::IA32_APIC_BASE).unwrap_or(base);
    // SAFETY: the value read first.
    unsafe { x86::wrmsr(x86::IA32_APIC_BASE, base) };
    let clearable = w.is_ok() && read & x86::APIC_BASE_BSP == 0;
    out!(
        "R bsp.flag before={} write={} after_clear={} clearable={}",
        u8::from(base & x86::APIC_BASE_BSP != 0),
        if w.is_ok() { "ok" } else { "fault" },
        u8::from(read & x86::APIC_BASE_BSP != 0),
        u8::from(clearable)
    );
    crate::con::flush();
    rehearsal(cx, d)?;
    if !clearable {
        out!("I bsp.init not run: the BSP flag does not clear");
        return Ok(());
    }
    init_runs(cx, d)
}

/// The bootstrap core's restart path, rehearsed on a worker: the same
/// saving, INIT, 10 ms, SIPI, 200 µs, SIPI, trampoline, restoring and jump
/// back, where nothing depends on the BSP flag. It runs first so that a
/// fault in that path shows on a worker, not on the bootstrap core.
fn rehearsal(cx: &Ctx, d: usize) -> Result<(), String> {
    let vector = sipi::vector().ok_or("no trampoline")?;
    let Some(&w) = smp::workers().iter().find(|&&i| i != d) else {
        out!("I bsp.rehearsal not run: needs two workers");
        return Ok(());
    };
    let w_apic = smp::cores()[w].apic;
    let mut times = Vec::new();
    for _ in 0..cx.reps(20, 3) {
        RESUME_CORE.store(w as u64, Ordering::Release);
        // SAFETY: SAVED is used by one core at a time (the step is serial).
        unsafe {
            (*saved()).armed.store(0, Ordering::Release);
            (*saved()).resumed_tsc.store(0, Ordering::Release);
        }
        let sent = AtomicU64::new(0);
        let target = |_: usize| {
            x86::cli();
            init_cycle();
            x86::sti();
        };
        smp::start_on(&[w], &target);
        let director = |_: usize| {
            // SAFETY: read through atomics only.
            let s = unsafe { &*saved() };
            while s.armed.load(Ordering::Acquire) == 0 {
                x86::pause();
            }
            sent.store(x86::rdtsc(), Ordering::Release);
            apic::send(w_apic, apic::INIT, 0);
            tsc::spin_ns(10_000_000);
            apic::send(w_apic, apic::STARTUP, vector);
            tsc::spin_ns(200_000);
            apic::send(w_apic, apic::STARTUP, vector);
        };
        smp::run_on(&[d], 5_000, &director)?;
        smp::wait(&[w], 5_000)?;
        // SAFETY: written by `resumed` before it jumped back.
        let r = unsafe { (*saved()).resumed_tsc.load(Ordering::Acquire) };
        times.push(r.wrapping_sub(sent.load(Ordering::Acquire)));
    }
    RESUME_CORE.store(u64::MAX, Ordering::Release);
    let k = alloc::format!(
        "core={w} kind={} d_init_us=10000 sipi=2x200us",
        smp::kind(w).name()
    );
    stats::report_ticks("bsp.rehearsal", &k, &mut times);
    crate::con::flush();
    Ok(())
}

/// INIT and SIPI to the bootstrap core with its flag cleared.
fn init_runs(cx: &Ctx, d: usize) -> Result<(), String> {
    let vector = sipi::vector().ok_or("no trampoline")?;
    let _ = sipi::entry(0);
    let bsp_apic = smp::cores()[0].apic;
    let me = percpu::me();
    let mut times = Vec::new();
    for _ in 0..cx.reps(20, 2) {
        RESUME_CORE.store(0, Ordering::Release);
        // SAFETY: the bootstrap core's own SAVED, before arming.
        unsafe {
            (*saved()).armed.store(0, Ordering::Release);
            (*saved()).resumed_tsc.store(0, Ordering::Release);
        }
        let req = me.park_request.fetch_add(1, Ordering::AcqRel) + 1;
        let sent = AtomicU64::new(0);
        let job = |_: usize| {
            // SAFETY: SAVED is read through atomics only here.
            let s = unsafe { &*saved() };
            while s.armed.load(Ordering::Acquire) == 0 {
                x86::pause();
            }
            sent.store(x86::rdtsc(), Ordering::Release);
            apic::send(bsp_apic, apic::INIT, 0);
            tsc::spin_ns(10_000_000);
            apic::send(bsp_apic, apic::STARTUP, vector);
            tsc::spin_ns(200_000);
            apic::send(bsp_apic, apic::STARTUP, vector);
        };
        smp::start_on(&[d], &job);
        let was = x86::interrupts_enabled();
        x86::cli();
        // SAFETY: clears the BSP flag on the bootstrap core (it was shown to
        // read back cleared); set again after the cycle.
        let base = unsafe { x86::rdmsr(x86::IA32_APIC_BASE) };
        unsafe { x86::wrmsr(x86::IA32_APIC_BASE, base & !x86::APIC_BASE_BSP) };
        init_cycle();
        // SAFETY: the BSP flag back as it was.
        unsafe { x86::wrmsr(x86::IA32_APIC_BASE, base) };
        if was {
            x86::sti();
        }
        smp::wait(&[d], 5_000)?;
        // SAFETY: written by `resumed` before it jumped back.
        let r = unsafe { (*saved()).resumed_tsc.load(Ordering::Acquire) };
        times.push(r.wrapping_sub(sent.load(Ordering::Acquire)));
        let _ = req;
    }
    out!(
        "R bsp.init.count n={} restarted={} flag_restored=1 firmware_timer_after={}",
        times.len(),
        times.len(),
        if crate::setup::firmware_timer_runs() {
            "runs"
        } else {
            "stopped"
        }
    );
    stats::report_ticks("bsp.init.entry", "d_init_us=10000 sipi=2x200us", &mut times);
    if cx.flag("bsp_init_flag_set") {
        out!("I bsp.init_flag_set: INIT to the bootstrap core with its flag set; expect a reset");
        crate::con::flush();
        let job = |_: usize| apic::send(bsp_apic, apic::INIT, 0);
        smp::start_on(&[d], &job);
        loop {
            x86::cli();
            x86::hlt();
        }
    }
    Ok(())
}
