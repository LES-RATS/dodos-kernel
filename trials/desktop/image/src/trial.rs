//! The multicore trial (`trials/multicore`, run on the M5 in
//! `results-mac-m5.md`) on the desktop's real cores: the same workloads and
//! suites, worker `i` on the `i`-th core of an order, and one more value of
//! the interrupt cost, `real`, where the model's busy wait of `ipi_ns` is
//! replaced by a real interrupt taken by that core (a self-IPI, waited for).
//! It settles the locking variant of a node (kernel-interface.md, open
//! point "Locking inside a node of several cores": L1, one node lock with
//! the lock-free slot read and signal = the trial's group lock with
//! `Lookup::Slot` and `Signals::Atomic`; L2, a lock per object = its fine
//! locks and range locks; against the same number of one-core nodes =
//! candidate 1).
//!
//! Default suites: `locks,a1t,a2t,a3t,ct,a3n,br` (lookups with changes,
//! IPC between cores, signals from many threads, a process beside others,
//! the atomic notify, unmaps by every thread), 3 repetitions of 150 ms,
//! nodes of up to 6 cores (`trial_suites`, `trial_reps`, `trial_ms`,
//! `trial_max_k` change them). Orders: P cores first (the main run), and
//! for `locks` alone E cores first and P and E alternating, which give the
//! hand-over of a queue lock between kinds. Output: the trial's Markdown
//! tables as `T ` lines of RESULTS.TXT, and its CSV in TRIAL-<order>.CSV.

use crate::idt::V_SELF;
use crate::run::Ctx;
use crate::smp::{self, CoreKind};
use crate::{apic, con, percpu, tsc, x86};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use dodos_trial_multicore::bench::{self, Opts, Sinks};
use dodos_trial_multicore::platform::IPI_REAL;
use dodos_trial_multicore::rt::{self, Duration, Host, Mutex};

/// The cores of the current order, and the job the trial posted.
static ORDER: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static JOB_DATA: AtomicUsize = AtomicUsize::new(0);
static JOB_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// CPU number of each core for the "on P" column: E cores first.
static CPU_OF: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static E_CORES: AtomicUsize = AtomicUsize::new(0);

fn trial_job(core: usize) {
    let pos = ORDER.lock().expect("order").iter().position(|&c| c == core).expect("core in order");
    // SAFETY: the fat pointer stored by `start`, alive until `wait` returns.
    let f: &(dyn Fn(usize) + Sync) = unsafe {
        core::mem::transmute::<(usize, usize), &(dyn Fn(usize) + Sync)>((
            JOB_DATA.load(Ordering::Acquire),
            JOB_VTABLE.load(Ordering::Acquire),
        ))
    };
    f(pos);
}

fn start(n: usize, f: &(dyn Fn(usize) + Sync)) {
    // SAFETY: splits the fat pointer; `wait` returns only when every worker
    // has finished with it.
    let (d, v) = unsafe { core::mem::transmute::<&(dyn Fn(usize) + Sync), (usize, usize)>(f) };
    JOB_DATA.store(d, Ordering::Release);
    JOB_VTABLE.store(v, Ordering::Release);
    let on: Vec<usize> = ORDER.lock().expect("order")[..n].to_vec();
    smp::start_on(&on, &trial_job);
}

fn wait() {
    let on: Vec<usize> = ORDER.lock().expect("order").clone();
    let busy: Vec<usize> = on.into_iter().filter(|&c| c != 0).collect();
    if let Err(e) = smp::wait(&busy, 300_000) {
        panic!("trial: {e}");
    }
}

fn interrupt() {
    let me = percpu::me();
    let c0 = me.irq_count.load(Ordering::Acquire);
    apic::self_ipi(V_SELF);
    while me.irq_count.load(Ordering::Acquire) == c0 {
        x86::pause();
    }
}

fn current_cpu() -> Option<usize> {
    CPU_OF.lock().expect("cpus").get(percpu::my_index()).copied()
}

fn describe() -> String {
    let w = smp::workers();
    let p = w.iter().filter(|&&i| smp::kind(i) == CoreKind::P).count();
    alloc::format!(
        "bare metal (DodOS desktop image), {} workers: {p} P and {} E, TSC {} Hz",
        w.len(),
        w.len() - p,
        tsc::hz()
    )
}

static HOST: Host = Host {
    now_ns: tsc::now_ns,
    start,
    wait,
    interrupt,
    current_cpu,
    efficiency_cores: 0,
    describe,
};

fn install() {
    let w = smp::workers();
    let mut cpu_of = vec![usize::MAX; smp::cores().len()];
    let es: Vec<usize> = w.iter().copied().filter(|&i| smp::kind(i) == CoreKind::E).collect();
    let rest: Vec<usize> = w.iter().copied().filter(|&i| smp::kind(i) != CoreKind::E).collect();
    for (n, &c) in es.iter().chain(&rest).enumerate() {
        cpu_of[c] = n;
    }
    *CPU_OF.lock().expect("cpus") = cpu_of;
    E_CORES.store(es.len(), Ordering::Relaxed);
    // `efficiency_cores` is a field: install a copy with the count.
    let host = alloc::boxed::Box::leak(alloc::boxed::Box::new(Host {
        efficiency_cores: es.len(),
        ..HOST
    }));
    rt::install(host);
}

/// The cores in `order`: "pfirst", "efirst" or "alternate".
fn order(name: &str) -> Vec<usize> {
    let w = smp::workers();
    let p: Vec<usize> = w.iter().copied().filter(|&i| smp::kind(i) != CoreKind::E).collect();
    let e: Vec<usize> = w.iter().copied().filter(|&i| smp::kind(i) == CoreKind::E).collect();
    match name {
        "efirst" => e.iter().chain(&p).copied().collect(),
        "alternate" => {
            let mut v = Vec::new();
            for k in 0..p.len().max(e.len()) {
                v.extend(p.get(k));
                v.extend(e.get(k));
            }
            v
        }
        _ => p.iter().chain(&e).copied().collect(),
    }
}

fn run_order(cx: &Ctx, name: &str, suites: &str) -> Result<(), String> {
    install();
    let cores = order(name);
    if cores.len() < 2 {
        return Err("fewer than two workers".into());
    }
    *ORDER.lock().expect("order") = cores.clone();
    let mut o = Opts::new(cores.len());
    o.suites = suites.split(',').map(str::to_string).collect();
    o.reps = cx.num("trial_reps", if cx.flag("quick") { 1 } else { 3 }) as usize;
    o.dur = Duration::from_millis(cx.num("trial_ms", if cx.flag("quick") { 20 } else { 150 }));
    o.max_k = (cx.num("trial_max_k", 6) as usize).min(cores.len());
    o.ipis = vec![0, 1000, IPI_REAL];
    let csv_name = alloc::format!("TRIAL-{}.CSV", name.to_ascii_uppercase());
    con::append_file(&csv_name, alloc::format!("{}\n", bench::csv_head()).as_bytes());
    let mut csv = String::new();
    let mut emit = |s: &str| {
        for l in s.lines() {
            con::line(&alloc::format!("T {l}"));
        }
        con::flush();
    };
    let mut add_csv = |s: &str| {
        csv.push_str(s);
        csv.push('\n');
        if csv.len() > 32 << 10 {
            con::append_file(&csv_name, csv.as_bytes());
            csv.clear();
        }
    };
    let mut progress = |s: &str| con::line(&alloc::format!("I trial {s}"));
    let kinds: String = cores.iter().map(|&c| smp::kind(c).name()).collect();
    con::line(&alloc::format!("I trial order={name} cores={kinds} suites={suites}"));
    bench::run(
        o,
        vec![None; cores.len()],
        &alloc::format!("one worker per core, order {name} ({kinds})"),
        Sinks { out: &mut emit, csv: Some(&mut add_csv), progress: &mut progress },
    );
    con::append_file(&csv_name, csv.as_bytes());
    Ok(())
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let suites = cx.get("trial_suites").unwrap_or("locks,a1t,a2t,a3t,ct,a3n,br").to_string();
    run_order(cx, "pfirst", &suites)
}

pub fn run_efirst(cx: &mut Ctx) -> Result<(), String> {
    run_order(cx, "efirst", "locks")
}

pub fn run_alternate(cx: &mut Ctx) -> Result<(), String> {
    run_order(cx, "alternate", "locks")
}
