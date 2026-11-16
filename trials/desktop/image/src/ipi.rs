//! Interrupts between cores and the time to leave each sleep state
//! (kernel.md line 71 and progress.md: "real inter-core interrupt
//! latency"; the trial's `ipi` parameter; the wake-up across cores of
//! kernel-interface.md "A node of several cores").
//!
//! - `ipi.self`: a fixed self-IPI through the SELF IPI register, taken and
//!   ended (entry, a three-register handler, EOI, IRET): what a core pays
//!   per interrupt.
//! - `ipi.rtt`: core A sends a fixed IPI to B; B's handler answers with a
//!   fixed IPI to A; A's handler counts; A measures from the send to the
//!   count. Pairs P→P, E→E, P→E, E→P; B spinning with interrupts enabled
//!   (`target=spin`), halted (`hlt`), or in MWAIT with each hint CPUID leaf
//!   05H enumerates (`mwait=0xNN`: EAX[7:4] the C-state minus one, EAX[3:0]
//!   the sub-state, SDM vol. 2B MWAIT table 1-3; vol. 1 table 21-15). The
//!   exit time of a state is its round trip minus the spinning one. Each
//!   MWAIT line also gives the change of the model-specific core C6 and C7
//!   residency counters (MSRs 3FDH and 3FEH, read with recovery) over the
//!   samples, to show whether the state was really entered.
//! - `ipi.oneway`: A sends, B's handler stamps the TSC; B's stamp minus A's
//!   send instant. Valid only with synchronised TSCs (the identity lines
//!   give IA32_TSC_ADJUST per core).
//! - `ipi.mem`: the same round trip by two cache-line writes, no interrupt.
//! - `ipi.matrix`: the spinning round trip between every ordered pair of
//!   workers, fewer samples.

use crate::idt::{V_FLAG, V_PONG, V_SELF};
use crate::run::Ctx;
use crate::smp::{self, CoreKind};
use crate::{apic, percpu, stats, tsc, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use dodos_trial_multicore::rt::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Spin,
    Hlt,
    Mwait(u32),
    /// Answers a memory flag instead of an interrupt.
    Mem,
}

impl Target {
    fn name(self) -> String {
        match self {
            Target::Spin => "spin".into(),
            Target::Hlt => "hlt".into(),
            Target::Mwait(h) => alloc::format!("mwait={h:#04x}"),
            Target::Mem => "mem".into(),
        }
    }
}

static STOP: AtomicU64 = AtomicU64::new(0);
static FLAG: AtomicU64 = AtomicU64::new(0);
static ACK: AtomicU64 = AtomicU64::new(0);
#[repr(align(64))]
struct Line(AtomicU64);
static MONITORED: Line = Line(AtomicU64::new(0));

fn monitor_mwait(hint: u32) {
    // SAFETY: MONITOR arms the line of MONITORED; MWAIT with ECX = 0 waits
    // until an interrupt (interrupts are enabled) or a store to it.
    unsafe {
        asm!("monitor", in("rax") MONITORED.0.as_ptr(), in("ecx") 0u32, in("edx") 0u32, options(nostack));
        asm!("mwait", in("eax") hint, in("ecx") 0u32, options(nostack));
    }
}

/// The loop of the target core until STOP.
pub fn target_loop(t: Target) {
    x86::sti();
    let mut seen = 0;
    while STOP.load(Ordering::Acquire) == 0 {
        match t {
            Target::Spin => x86::pause(),
            Target::Hlt => x86::hlt(),
            Target::Mwait(h) => monitor_mwait(h),
            Target::Mem => {
                let f = FLAG.load(Ordering::Acquire);
                if f != seen {
                    seen = f;
                    ACK.store(f, Ordering::Release);
                }
            }
        }
    }
}

/// Ends a target loop and wakes a halted or waiting target.
pub fn stop(target_apic: u32) {
    STOP.store(1, Ordering::Release);
    apic::send(target_apic, apic::FIXED, V_FLAG);
}

fn wait_count(c: &AtomicU64, from: u64) {
    let end = x86::rdtsc() + tsc::ns_to_ticks(100_000_000);
    while c.load(Ordering::Acquire) == from {
        if x86::rdtsc() > end {
            panic!("an interrupt did not arrive within 100 ms");
        }
        x86::pause();
    }
}

/// Round trips from the calling core to `b`, `n` samples, `gap_ns` apart.
fn rtt(b: usize, t: Target, n: usize, gap_ns: u64) -> Vec<u64> {
    let me = percpu::me();
    let b_apic = smp::cores()[b].apic;
    percpu::of(b).pong_dest.store(u64::from(apic::id()), Ordering::Relaxed);
    let mut v = Vec::with_capacity(n);
    for k in 0..=n {
        tsc::spin_ns(gap_ns);
        let t0;
        let t1;
        if t == Target::Mem {
            let seq = k as u64 + 1;
            t0 = x86::tsc_begin();
            FLAG.store(seq, Ordering::Release);
            while ACK.load(Ordering::Acquire) != seq {
                x86::pause();
            }
            t1 = x86::tsc_end();
        } else {
            let c0 = me.irq_count.load(Ordering::Acquire);
            t0 = x86::tsc_begin();
            apic::send(b_apic, apic::FIXED, V_PONG);
            wait_count(&me.irq_count, c0);
            t1 = x86::tsc_end();
        }
        if k > 0 {
            v.push(t1 - t0);
        }
    }
    v
}

fn oneway(b: usize, n: usize) -> Vec<u64> {
    let tb = percpu::of(b);
    let b_apic = smp::cores()[b].apic;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        tsc::spin_ns(5_000);
        let c0 = tb.irq_count.load(Ordering::Acquire);
        let t0 = x86::tsc_begin();
        apic::send(b_apic, apic::FIXED, V_FLAG);
        wait_count(&tb.irq_count, c0);
        v.push(tb.irq_tsc.load(Ordering::Acquire).wrapping_sub(t0));
    }
    v
}

/// One pair measurement: B runs `t`, A measures; returns A's samples and,
/// for MWAIT, the C6 and C7 residency changes on B.
fn pair(a: usize, b: usize, t: Target, n: usize, gap_ns: u64, oneway_too: bool) -> Result<(Vec<u64>, Vec<u64>, String), String> {
    STOP.store(0, Ordering::Release);
    let out: Mutex<(Vec<u64>, Vec<u64>, String)> = Mutex::new((Vec::new(), Vec::new(), String::new()));
    let ready = AtomicU64::new(0);
    smp::run_on(&[a, b], 120_000, &|i| {
        if i == b {
            smp::mark("ipi target");
            let r0 = [x86::safe_rdmsr(0x3FD).ok(), x86::safe_rdmsr(0x3FE).ok()];
            ready.store(1, Ordering::Release);
            target_loop(t);
            let r1 = [x86::safe_rdmsr(0x3FD).ok(), x86::safe_rdmsr(0x3FE).ok()];
            let d = |k: usize| r0[k].zip(r1[k]).map_or("none".into(), |(x, y)| alloc::format!("{}", y.wrapping_sub(x)));
            if matches!(t, Target::Mwait(_)) {
                out.lock().expect("out").2 = alloc::format!(" c6_residency={} c7_residency={}", d(0), d(1));
            }
        } else {
            smp::mark("ipi initiator");
            while ready.load(Ordering::Acquire) == 0 {
                x86::pause();
            }
            let r = rtt(b, t, n, gap_ns);
            let o = if oneway_too { oneway(b, n) } else { Vec::new() };
            stop(smp::cores()[b].apic);
            let mut g = out.lock().expect("out");
            g.0 = r;
            g.1 = o;
        }
    })?;
    Ok(out.into_inner_spin())
}

/// `Mutex::into_inner` for the bare mutex, by taking the value out.
trait IntoInner<T> {
    fn into_inner_spin(self) -> T;
}
impl<T: Default> IntoInner<T> for Mutex<T> {
    fn into_inner_spin(self) -> T {
        core::mem::take(&mut *self.lock().expect("mutex"))
    }
}

/// The MWAIT hints of CPUID leaf 05H: for C-state n ≥ 1 with k sub-states
/// (EDX nibble n), hints ((n - 1) << 4) | s, s < k. With MONITOR but no
/// enumeration, hint 0 (C1) alone.
pub fn mwait_hints() -> Vec<u32> {
    if !x86::bit(x86::cpuid(1, 0).ecx, 3) {
        return Vec::new();
    }
    let l5 = if x86::cpuid(0, 0).eax >= 5 { x86::cpuid(5, 0) } else { x86::Cpuid::default() };
    let mut h = Vec::new();
    if l5.ecx & 1 == 1 {
        for n in 1..8u32 {
            for s in 0..(l5.edx >> (4 * n) & 0xF) {
                h.push((n - 1) << 4 | s);
            }
        }
    }
    if h.is_empty() {
        h.push(0);
    }
    h
}

/// Representative pairs: P→P, E→E, P→E, E→P (two cores of each kind).
pub fn pairs() -> Vec<(usize, usize)> {
    let w = smp::workers();
    let of = |k: CoreKind| w.iter().copied().filter(|&i| smp::kind(i) == k).collect::<Vec<_>>();
    let (p, e, u) = (of(CoreKind::P), of(CoreKind::E), of(CoreKind::Unknown));
    let mut out = Vec::new();
    if p.len() >= 2 {
        out.push((p[0], p[1]));
    }
    if e.len() >= 2 {
        out.push((e[0], e[1]));
    }
    if !p.is_empty() && !e.is_empty() {
        out.push((p[0], e[0]));
        out.push((e[0], p[0]));
    }
    if out.is_empty() && u.len() >= 2 {
        out.push((u[0], u[1]));
    }
    out
}

fn keys(a: usize, b: usize, t: Target) -> String {
    alloc::format!(
        "from={a} to={b} from_kind={} to_kind={} target={}",
        smp::kind(a).name(),
        smp::kind(b).name(),
        t.name()
    )
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let n = cx.reps(20_000, 200);
    let n_idle = cx.reps(2_000, 50);
    // Self-IPI on one core of each kind.
    for core in crate::gate::one_per_kind() {
        let v: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        smp::run_on(&[core], 60_000, &|_| {
            let me = percpu::me();
            let mut s = Vec::with_capacity(n);
            for _ in 0..n {
                let c0 = me.irq_count.load(Ordering::Acquire);
                let t0 = x86::tsc_begin();
                apic::self_ipi(V_SELF);
                wait_count(&me.irq_count, c0);
                s.push(x86::tsc_end() - t0);
            }
            *v.lock().expect("v") = s;
        })?;
        let k = alloc::format!("core={core} kind={}", smp::kind(core).name());
        stats::report_ticks("ipi.self", &k, &mut v.lock().expect("v"));
    }
    for (a, b) in pairs() {
        let (mut r, mut o, _) = pair(a, b, Target::Spin, n, 2_000, true)?;
        stats::report_ticks("ipi.rtt", &keys(a, b, Target::Spin), &mut r);
        stats::report_ticks("ipi.oneway", &keys(a, b, Target::Spin), &mut o);
        let (mut m, _, _) = pair(a, b, Target::Mem, n, 2_000, false)?;
        stats::report_ticks("ipi.mem", &keys(a, b, Target::Mem), &mut m);
        let (mut h, _, _) = pair(a, b, Target::Hlt, n_idle, 50_000, false)?;
        stats::report_ticks("ipi.rtt", &keys(a, b, Target::Hlt), &mut h);
        for hint in mwait_hints() {
            let t = Target::Mwait(hint);
            let (mut s, _, res) = pair(a, b, t, n_idle, 1_000_000, false)?;
            let k = alloc::format!("{}{res}", keys(a, b, t));
            stats::report_ticks("ipi.rtt", &k, &mut s);
        }
        crate::con::flush();
    }
    let w = smp::workers();
    let nm = cx.reps(500, 20);
    for &a in &w {
        for &b in &w {
            if a != b {
                let (mut r, _, _) = pair(a, b, Target::Spin, nm, 2_000, false)?;
                stats::report_ticks("ipi.matrix", &keys(a, b, Target::Spin), &mut r);
            }
        }
    }
    Ok(())
}
