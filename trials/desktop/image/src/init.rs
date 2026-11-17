//! Stopping a core by INIT and restarting it by SIPI, as the fence does it
//! on x86-64 (kernel-interface.md, "The fence", steps 1 to 5; the open
//! points "the delays INIT and SIPI need on the reference desktop's cores",
//! "the time of a fence per number of cores of a node"; D-117, D-129).
//!
//! - `init.sipi`: a director core sends INIT to a target, waits `d_init`,
//!   sends a SIPI to the trampoline, waits `d_sipi`, sends a second SIPI
//!   (or none, `sipi=1`), and waits up to 100 ms for the target to confirm
//!   the park request from the trampoline. The SDM's example is 10 ms and
//!   200 µs (vol. 3A 11.4.4.1, table 11-1: an example, not a stated
//!   requirement). Swept: d_init 10 ms, 1 ms, 100 µs, 10 µs, 0; d_sipi
//!   200 µs, 10 µs, one SIPI only; on a P and an E target spinning with
//!   interrupts masked. Each failure is counted, then the target is
//!   recovered with the SDM's delays (up to three times; then it is lost
//!   and the step stops using it). `entry` is from the INIT to the first
//!   instruction of the trampoline (same TSC, unchanged by INIT), `parked`
//!   to the confirmation seen by the director.
//! - `init.state`: the SDM's delays, the target in other states: halted
//!   with interrupts masked, in MWAIT (deepest enumerated hint) with
//!   interrupts masked, spinning inside its NMI handler (NMIs blocked: the
//!   kernel "too damaged to run its NMI handler"), and in VMX root
//!   operation when VMX is enabled ("The INIT signal is blocked whenever a
//!   logical processor is in VMX root operation", vol. 3C 26.8; §25.8 in
//!   the -086 edition the contract cites): there it reports whether the
//!   INIT was blocked, and whether it took effect at VMXOFF (held) or was
//!   lost (the SDM does not say).
//! - `fence.nmi`, `fence.init`: the fence of a node of 1, 2, 4 and 6 cores:
//!   by the NMI handler parking each core (a kernel that can still run it),
//!   and by INIT and SIPI to every core spinning in its NMI handler, from
//!   the first stop sent to the last confirmation (`FENCE_NS`).
//! - `init.smm`: INIT sent while the target is in SMM ("INIT operations
//!   are inhibited when the processor enters SMM", vol. 3C 34.5.1; "blocked
//!   after RSM if and only if the logical processor will be in VMX root
//!   operation", 34.14.2; whether an INIT received during SMM is held or
//!   lost is not said). Only with `smi_cmd=<value>` in CONFIG.TXT, a value
//!   the firmware ignores on its SMI command port (ACPI FADT `SMI_CMD`);
//!   no default exists, since an unknown command may do anything. Not run
//!   otherwise, and stated so in the results.

use crate::run::Ctx;
use crate::smp::{self, CoreKind};
use crate::{apic, out, percpu, sipi, stats, tsc, vmx, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use dodos_trial_multicore::rt::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum Victim {
    Spin = 0,
    Halt = 1,
    Mwait = 2,
    NmiBlocked = 3,
    Vmx = 4,
    /// Writes the SMI command value to the SMI command port in a loop.
    Smi = 5,
}

static VICTIM: AtomicU32 = AtomicU32::new(0);
static MWAIT_HINT: AtomicU32 = AtomicU32::new(0);
static READY: AtomicU64 = AtomicU64::new(0);
/// The VMX victim counts here, and leaves VMX operation when asked.
static COUNTER: AtomicU64 = AtomicU64::new(0);
static VMXOFF_NOW: AtomicU64 = AtomicU64::new(0);
static SMI_PORT: AtomicU32 = AtomicU32::new(0);
static SMI_VALUE: AtomicU32 = AtomicU32::new(0);
static SMI_SEEN: AtomicU64 = AtomicU64::new(0);
pub static VMX_STATUS: AtomicU64 = AtomicU64::new(0);

/// The job of a target: reach its state, say so, and never return (only
/// INIT, or an NMI that parks it, gets it out).
fn victim(_core: usize) {
    let v = VICTIM.load(Ordering::Acquire);
    x86::cli();
    if v == Victim::NmiBlocked as u32 {
        percpu::me().nmi_park.store(2, Ordering::Release);
    }
    if v == Victim::Vmx as u32 {
        let r = vmx::on();
        VMX_STATUS.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release);
        READY.fetch_add(1, Ordering::AcqRel);
        if r.is_ok() {
            while VMXOFF_NOW.load(Ordering::Acquire) == 0 {
                COUNTER.fetch_add(1, Ordering::Relaxed);
            }
            vmx::off();
        }
        loop {
            COUNTER.fetch_add(1, Ordering::Relaxed);
        }
    }
    if v == Victim::Smi as u32 {
        let (port, value) = (
            SMI_PORT.load(Ordering::Relaxed) as u16,
            SMI_VALUE.load(Ordering::Relaxed) as u8,
        );
        let smi0 = x86::safe_rdmsr(x86::MSR_SMI_COUNT).unwrap_or(0);
        READY.fetch_add(1, Ordering::AcqRel);
        loop {
            if VMXOFF_NOW.load(Ordering::Relaxed) == 0 {
                // SAFETY: the FADT's SMI command port with a value the
                // owner chose as ignored by the firmware (opt-in).
                unsafe { x86::outb(port, value) };
            }
            let c = COUNTER.fetch_add(1, Ordering::Relaxed);
            if c.is_multiple_of(64) {
                let smi = x86::safe_rdmsr(x86::MSR_SMI_COUNT).unwrap_or(0);
                SMI_SEEN.store(smi.wrapping_sub(smi0), Ordering::Relaxed);
            }
        }
    }
    READY.fetch_add(1, Ordering::AcqRel);
    match v {
        1 => loop {
            x86::hlt();
        },
        2 => loop {
            let line = AtomicU64::new(0);
            // SAFETY: MONITOR on a local line, MWAIT with interrupts masked
            // and ECX = 0: only INIT, NMI, SMI or a store wakes it.
            unsafe {
                asm!("monitor", in("rax") line.as_ptr(), in("ecx") 0u32, in("edx") 0u32, options(nostack));
                asm!("mwait", in("eax") MWAIT_HINT.load(Ordering::Relaxed), in("ecx") 0u32, options(nostack));
            }
        },
        // SAFETY: an endless loop with interrupts masked.
        _ => unsafe { asm!("2:", "jmp 2b", options(noreturn)) },
    }
}

#[derive(Clone, Copy)]
pub enum Sipis {
    Two(u64),
    One,
}

pub struct Outcome {
    pub ok: bool,
    pub entry: u64,
    pub parked: u64,
    pub recoveries: u32,
}

fn wait_parked(targets: &[usize], reqs: &[u64], limit_ns: u64) -> Option<u64> {
    let t0 = x86::rdtsc();
    let end = t0 + tsc::ns_to_ticks(limit_ns);
    loop {
        if targets
            .iter()
            .zip(reqs)
            .all(|(&t, &r)| percpu::of(t).parked.load(Ordering::Acquire) == r)
        {
            return Some(x86::rdtsc());
        }
        if x86::rdtsc() > end {
            return None;
        }
        x86::pause();
    }
}

fn raise(t: usize) -> u64 {
    percpu::of(t).park_request.fetch_add(1, Ordering::AcqRel) + 1
}

/// INIT, delays, SIPIs to `targets`; returns the send instant.
fn init_sipi(targets: &[usize], d_init_ns: u64, sipis: Sipis, vector: u8) -> u64 {
    let apics: Vec<u32> = targets.iter().map(|&t| smp::cores()[t].apic).collect();
    let t0 = x86::rdtsc();
    for &a in &apics {
        apic::send(a, apic::INIT, 0);
    }
    tsc::spin_ns(d_init_ns);
    for &a in &apics {
        apic::send(a, apic::STARTUP, vector);
    }
    if let Sipis::Two(d) = sipis {
        tsc::spin_ns(d);
        for &a in &apics {
            apic::send(a, apic::STARTUP, vector);
        }
    }
    t0
}

/// The director's part of one stop and restart of `t`.
fn direct(t: usize, d_init_ns: u64, sipis: Sipis, vector: u8, state: Victim) -> Outcome {
    let ready_end = x86::rdtsc() + tsc::ns_to_ticks(1_000_000_000);
    while READY.load(Ordering::Acquire) == 0 && x86::rdtsc() < ready_end {
        x86::pause();
    }
    if state == Victim::NmiBlocked {
        apic::send(smp::cores()[t].apic, apic::NMI, 0);
        tsc::spin_ns(1_000_000);
    }
    let req = raise(t);
    let t0 = init_sipi(&[t], d_init_ns, sipis, vector);
    if let Some(t1) = wait_parked(&[t], &[req], 100_000_000) {
        let e = percpu::of(t).restart_tsc.load(Ordering::Acquire);
        return Outcome {
            ok: true,
            entry: e.wrapping_sub(t0),
            parked: t1 - t0,
            recoveries: 0,
        };
    }
    for k in 1..=3 {
        let req = raise(t);
        init_sipi(&[t], 10_000_000, Sipis::Two(200_000), vector);
        if wait_parked(&[t], &[req], 200_000_000).is_some() {
            return Outcome {
                ok: false,
                entry: 0,
                parked: 0,
                recoveries: k,
            };
        }
    }
    smp::set_state(t, smp::LOST);
    Outcome {
        ok: false,
        entry: 0,
        parked: 0,
        recoveries: 4,
    }
}

/// One stop and restart of `t` directed by `d`.
pub fn cycle(
    d: usize,
    t: usize,
    state: Victim,
    d_init_ns: u64,
    sipis: Sipis,
    vector: u8,
) -> Result<Outcome, String> {
    READY.store(0, Ordering::Release);
    VICTIM.store(state as u32, Ordering::Release);
    smp::start_on(&[t], &victim);
    let out: Mutex<Option<Outcome>> = Mutex::new(None);
    smp::run_on(&[d], 5_000, &|_| {
        smp::mark("init director");
        *out.lock().expect("out") = Some(direct(t, d_init_ns, sipis, vector, state));
    })?;
    let o = out.lock().expect("out").take().ok_or("no outcome")?;
    if smp::state(t) != smp::LOST {
        smp::wait(&[t], 2_000)?;
    }
    Ok(o)
}

fn summary(name: &str, keys: &str, outs: &[Outcome]) {
    let failures = outs.iter().filter(|o| !o.ok).count();
    let lost = outs.iter().any(|o| o.recoveries > 3);
    let mut entry: Vec<u64> = outs.iter().filter(|o| o.ok).map(|o| o.entry).collect();
    let mut parked: Vec<u64> = outs.iter().filter(|o| o.ok).map(|o| o.parked).collect();
    out!(
        "R {name}.count {keys} n={} failures={failures} lost={}",
        outs.len(),
        u8::from(lost)
    );
    if !entry.is_empty() {
        stats::report_ticks(&alloc::format!("{name}.entry"), keys, &mut entry);
        stats::report_ticks(&alloc::format!("{name}.parked"), keys, &mut parked);
    }
}

/// A director and a target of `kind`, different cores.
fn pick(kind: CoreKind) -> Option<(usize, usize)> {
    let w = smp::workers();
    let t = *w.iter().find(|&&i| smp::kind(i) == kind)?;
    let d = *w.iter().find(|&&i| i != t)?;
    Some((d, t))
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let vector = sipi::vector().ok_or("no trampoline page below 1 MB: nothing restarts a core")?;
    out!("I init trampoline_vector={vector:#x}");
    let reps = cx.reps(200, 3);
    let sdm = (10_000_000u64, Sipis::Two(200_000));
    for kind in [CoreKind::P, CoreKind::E, CoreKind::Unknown] {
        let Some((d, t)) = pick(kind) else { continue };
        for d_init_us in [10_000u64, 1000, 100, 10, 0] {
            for (sipi_name, sipis) in [
                ("2x200us", Sipis::Two(200_000)),
                ("2x10us", Sipis::Two(10_000)),
                ("1", Sipis::One),
            ] {
                let mut outs = Vec::with_capacity(reps);
                for _ in 0..reps {
                    if smp::state(t) == smp::LOST {
                        break;
                    }
                    outs.push(cycle(d, t, Victim::Spin, d_init_us * 1000, sipis, vector)?);
                }
                let k = alloc::format!(
                    "target={t} kind={} director={d} state=spin d_init_us={d_init_us} sipi={sipi_name}",
                    kind.name()
                );
                summary("init.sipi", &k, &outs);
            }
            crate::con::flush();
        }
        let mut states = alloc::vec![Victim::Halt, Victim::NmiBlocked];
        if let Some(&h) = crate::ipi::mwait_hints().last() {
            MWAIT_HINT.store(h, Ordering::Relaxed);
            states.push(Victim::Mwait);
        }
        for state in states {
            let mut outs = Vec::new();
            for _ in 0..cx.reps(50, 3) {
                if smp::state(t) == smp::LOST {
                    break;
                }
                outs.push(cycle(d, t, state, sdm.0, sdm.1, vector)?);
            }
            let k = alloc::format!(
                "target={t} kind={} director={d} state={state:?} d_init_us=10000 sipi=2x200us",
                kind.name()
            );
            summary("init.state", &k, &outs);
        }
        if vmx::available() && smp::state(t) != smp::LOST {
            vmx_case(d, t, vector)?;
        }
    }
    fences(cx, vector)?;
    smm(cx, vector)?;
    Ok(())
}

/// INIT to a core in VMX root operation: blocked? held until VMXOFF?
fn vmx_case(d: usize, t: usize, vector: u8) -> Result<(), String> {
    READY.store(0, Ordering::Release);
    VMXOFF_NOW.store(0, Ordering::Release);
    VMX_STATUS.store(0, Ordering::Release);
    VICTIM.store(Victim::Vmx as u32, Ordering::Release);
    smp::start_on(&[t], &victim);
    let line: Mutex<String> = Mutex::new(String::new());
    smp::run_on(&[d], 10_000, &|_| {
        while READY.load(Ordering::Acquire) == 0 {
            x86::pause();
        }
        if VMX_STATUS.load(Ordering::Acquire) != 1 {
            *line.lock().expect("line") = "vmxon=failed".into();
            return;
        }
        apic::send(smp::cores()[t].apic, apic::INIT, 0);
        tsc::spin_ns(20_000_000);
        let c0 = COUNTER.load(Ordering::Acquire);
        tsc::spin_ns(1_000_000);
        let blocked = COUNTER.load(Ordering::Acquire) != c0;
        VMXOFF_NOW.store(1, Ordering::Release);
        tsc::spin_ns(20_000_000);
        let c1 = COUNTER.load(Ordering::Acquire);
        tsc::spin_ns(1_000_000);
        let held = COUNTER.load(Ordering::Acquire) == c1;
        let req = raise(t);
        if !held {
            apic::send(smp::cores()[t].apic, apic::INIT, 0);
            tsc::spin_ns(10_000_000);
        }
        apic::send(smp::cores()[t].apic, apic::STARTUP, vector);
        tsc::spin_ns(200_000);
        apic::send(smp::cores()[t].apic, apic::STARTUP, vector);
        let back = wait_parked(&[t], &[req], 200_000_000).is_some();
        *line.lock().expect("line") = alloc::format!(
            "vmxon=ok init_blocked_in_vmx_root={} init_taken_at_vmxoff={} restarted={}",
            u8::from(blocked),
            u8::from(held),
            u8::from(back)
        );
    })?;
    let l = core::mem::take(&mut *line.lock().expect("line"));
    out!("R init.vmx target={t} kind={} {l}", smp::kind(t).name());
    if smp::state(t) != smp::LOST {
        let _ = smp::wait(&[t], 2_000);
    }
    Ok(())
}

fn fences(cx: &Ctx, vector: u8) -> Result<(), String> {
    let w = smp::workers();
    let Some(&d) = w.last() else { return Ok(()) };
    let pool: Vec<usize> = w.iter().copied().filter(|&i| i != d).collect();
    for n in [1usize, 2, 4, 6] {
        if pool.len() < n {
            continue;
        }
        let targets = &pool[..n];
        for by_init in [false, true] {
            let mut times = Vec::new();
            let mut fails = 0;
            for _ in 0..cx.reps(20, 2) {
                if targets.iter().any(|&t| smp::state(t) == smp::LOST) {
                    break;
                }
                READY.store(0, Ordering::Release);
                VICTIM.store(
                    if by_init {
                        Victim::NmiBlocked
                    } else {
                        Victim::Spin
                    } as u32,
                    Ordering::Release,
                );
                smp::start_on(targets, &victim);
                let res: Mutex<Option<u64>> = Mutex::new(None);
                smp::run_on(&[d], 10_000, &|_| {
                    while READY.load(Ordering::Acquire) < n as u64 {
                        x86::pause();
                    }
                    let apics: Vec<u32> = targets.iter().map(|&t| smp::cores()[t].apic).collect();
                    if !by_init {
                        for &t in targets {
                            percpu::of(t).nmi_park.store(1, Ordering::Release);
                        }
                    }
                    let reqs: Vec<u64> = targets.iter().map(|&t| raise(t)).collect();
                    let t0 = x86::rdtsc();
                    for &a in &apics {
                        apic::send(a, apic::NMI, 0);
                    }
                    if by_init {
                        tsc::spin_ns(1_000_000);
                        init_sipi(targets, 10_000_000, Sipis::Two(200_000), vector);
                    }
                    let done = wait_parked(targets, &reqs, 100_000_000);
                    *res.lock().expect("res") = done.map(|t1| t1 - t0);
                    if !by_init && done.is_some() {
                        // Start them again: a new start request and an NMI.
                        for &t in targets {
                            percpu::of(t).start_request.fetch_add(1, Ordering::AcqRel);
                        }
                        for &a in &apics {
                            apic::send(a, apic::NMI, 0);
                        }
                    }
                })?;
                match *res.lock().expect("res") {
                    Some(t) => times.push(t),
                    None => {
                        fails += 1;
                        for &t in targets {
                            let req = raise(t);
                            init_sipi(&[t], 10_000_000, Sipis::Two(200_000), vector);
                            if wait_parked(&[t], &[req], 200_000_000).is_none() {
                                smp::set_state(t, smp::LOST);
                            }
                        }
                    }
                }
                let alive: Vec<usize> = targets
                    .iter()
                    .copied()
                    .filter(|&t| smp::state(t) != smp::LOST)
                    .collect();
                smp::wait(&alive, 2_000)?;
            }
            let kinds: String = targets.iter().map(|&t| smp::kind(t).name()).collect();
            let name = if by_init { "fence.init" } else { "fence.nmi" };
            let k = alloc::format!("cores={n} kinds={kinds} failures={fails}");
            stats::report_ticks(name, &k, &mut times);
        }
        crate::con::flush();
    }
    Ok(())
}

/// INIT sent while the target is in SMM: opt-in (module comment). The
/// target writes the chosen value to the SMI command port in a loop, so it
/// spends much of its time in SMM; the director sends INIT, waits 20 ms and
/// looks at the target's counter: stopped, the INIT was taken (held to RSM,
/// or it arrived outside SMM); still counting, it was lost. Then the target
/// stops its SMIs and is restarted by INIT and SIPI.
fn smm(cx: &Ctx, vector: u8) -> Result<(), String> {
    let Some(cmd) = cx.get("smi_cmd").and_then(crate::run::parse_num) else {
        out!(
            "I init.smm not run: no smi_cmd in CONFIG.TXT (an unknown SMI command may do anything)"
        );
        return Ok(());
    };
    let Some((port, reserved)) = crate::acpi::smi_cmd() else {
        out!("I init.smm not run: the FADT names no SMI command port");
        return Ok(());
    };
    if reserved.contains(&(cmd as u8)) {
        out!("I init.smm not run: smi_cmd={cmd:#x} is one of the FADT's own commands {reserved:?}");
        return Ok(());
    }
    let Some((d, t)) = pick(CoreKind::P).or_else(|| pick(CoreKind::Unknown)) else {
        return Ok(());
    };
    SMI_PORT.store(port, Ordering::Relaxed);
    SMI_VALUE.store(cmd as u32, Ordering::Relaxed);
    let (mut taken, mut lost, mut smis) = (0u32, 0u32, 0u64);
    for k in 0..cx.reps(50, 2) {
        if smp::state(t) == smp::LOST {
            break;
        }
        READY.store(0, Ordering::Release);
        VMXOFF_NOW.store(0, Ordering::Release);
        SMI_SEEN.store(0, Ordering::Release);
        VICTIM.store(Victim::Smi as u32, Ordering::Release);
        smp::start_on(&[t], &victim);
        let verdict: Mutex<Option<bool>> = Mutex::new(None);
        smp::run_on(&[d], 10_000, &|_| {
            while READY.load(Ordering::Acquire) == 0 {
                x86::pause();
            }
            tsc::spin_ns(100_000 + (k as u64 * 37_000) % 2_000_000);
            apic::send(smp::cores()[t].apic, apic::INIT, 0);
            tsc::spin_ns(20_000_000);
            let c0 = COUNTER.load(Ordering::Acquire);
            tsc::spin_ns(1_000_000);
            let running = COUNTER.load(Ordering::Acquire) != c0;
            VMXOFF_NOW.store(1, Ordering::Release);
            let req = raise(t);
            if running {
                tsc::spin_ns(1_000_000);
                apic::send(smp::cores()[t].apic, apic::INIT, 0);
                tsc::spin_ns(10_000_000);
            }
            apic::send(smp::cores()[t].apic, apic::STARTUP, vector);
            tsc::spin_ns(200_000);
            apic::send(smp::cores()[t].apic, apic::STARTUP, vector);
            if wait_parked(&[t], &[req], 200_000_000).is_none() {
                smp::set_state(t, smp::LOST);
            }
            *verdict.lock().expect("verdict") = Some(!running);
        })?;
        smis += SMI_SEEN.load(Ordering::Acquire);
        match *verdict.lock().expect("verdict") {
            Some(true) => taken += 1,
            Some(false) => lost += 1,
            None => {}
        }
        if smp::state(t) != smp::LOST {
            smp::wait(&[t], 2_000)?;
        }
    }
    out!(
        "R init.smm target={t} port={port:#x} value={cmd:#x} inits_taken={taken} inits_lost={lost} smis_seen={smis} lost_core={}",
        u8::from(smp::state(t) == smp::LOST)
    );
    Ok(())
}
