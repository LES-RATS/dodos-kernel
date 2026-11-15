//! Measured trial for the DodOS kernel: may a node run on more than one core
//! at the same time? See README.md for the method.
//!
//! Usage: dodos-trial-multicore [--suites locks,a1,a2,a3,b,c,d,a1f,a2f,a3f,bf,cf]
//!        [--reps N]
//!        [--ms N] [--max-k N] [--lock ttas|ticket|queue] [--csv FILE]

mod hist;
mod platform;
mod sim;
mod sync;

use sim::{Adds, CLASSES, Cand, Initiators, Irq, Lookup, Params, RunOut, Workload};
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};
use sync::LockKind;

struct Opts {
    suites: Vec<String>,
    reps: usize,
    dur: Duration,
    max_k: usize,
    lock: LockKind,
    csv: Option<std::fs::File>,
}

fn parse_lock(s: &str) -> LockKind {
    match s {
        "ttas" => LockKind::Ttas,
        "ticket" => LockKind::Ticket,
        "queue" => LockKind::Queue,
        _ => panic!("unknown lock {s}"),
    }
}

fn opts(cores: usize) -> Opts {
    let mut o = Opts {
        suites: [
            "locks", "a1", "a2", "a3", "b", "c", "d", "a1f", "a2f", "a3f", "bf", "cf",
        ]
        .map(String::from)
        .to_vec(),
        reps: 5,
        dur: Duration::from_millis(150),
        max_k: cores.saturating_sub(2).max(2),
        lock: LockKind::Queue,
        csv: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    for pair in args.chunks(2) {
        let v = pair.get(1).map_or("", String::as_str);
        let num = || {
            v.parse::<u64>()
                .unwrap_or_else(|_| panic!("{} wants a number", pair[0]))
        };
        match pair[0].as_str() {
            "--suites" => o.suites = v.split(',').map(String::from).collect(),
            "--reps" => o.reps = num() as usize,
            "--ms" => o.dur = Duration::from_millis(num()),
            "--max-k" => o.max_k = num() as usize,
            "--lock" => o.lock = parse_lock(v),
            "--csv" => {
                let mut f = std::fs::File::create(v).expect("cannot create the CSV file");
                let classes =
                    (0..CLASSES).map(|c| format!("c{c}_n,c{c}_p50,c{c}_p99,c{c}_p999,c{c}_max"));
                let head = "suite,rep,labels,column,workload,k,n,lock,work_ns,gap_ns,ipi_ns,change_pct,migrations,stalls,ops_per_s";
                let classes: Vec<String> = classes.collect();
                let tail = "samples,preferred,lookup,adds,replica_pct,irq,retries";
                writeln!(f, "{head},{},{tail}", classes.join(",")).expect("CSV write");
                o.csv = Some(f);
            }
            other => panic!("unknown option {other}; see the head of main.rs"),
        }
    }
    o
}

/// One row of a table: its labels, and what to run.
struct Row {
    labels: Vec<String>,
    w: Workload,
    k: usize,
    n: usize,
    p: Params,
}

/// One column: a candidate with a lock.
struct Col {
    name: String,
    cand: Cand,
    lock: LockKind,
    v: Variant,
}

/// The settings of the switches of the second measurement.
#[derive(Clone, Copy)]
struct Variant {
    lookup: Lookup,
    adds: Adds,
    replica_pct: u64,
    irq: Irq,
}

/// The settings of the first measurement.
const FIRST: Variant = Variant {
    lookup: Lookup::Locked,
    adds: Adds::Eager,
    replica_pct: 100,
    irq: Irq::PerBatch,
};

/// Each unfairness of the first measurement removed.
const FAIR: Variant = Variant {
    lookup: Lookup::Seqlock,
    adds: Adds::Lazy,
    replica_pct: 25,
    irq: Irq::PerBatch,
};

fn col(name: &str, cand: Cand, lock: LockKind, v: Variant) -> Col {
    Col {
        name: name.into(),
        cand,
        lock,
        v,
    }
}

#[derive(Clone, Copy)]
enum Metric {
    /// Operations per second over the classes of the mask.
    Rate(u8),
    /// p50 / p99 / p99.9 of one class, in microseconds.
    Lat(usize),
    /// Fewest operations of a worker over most.
    Fair,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn si(x: f64) -> String {
    match x {
        x if x >= 1e6 => format!("{:.2}M", x / 1e6),
        x if x >= 1e3 => format!("{:.0}k", x / 1e3),
        x => format!("{x:.0}"),
    }
}

fn rate(r: &RunOut, mask: u8) -> f64 {
    (0..CLASSES)
        .filter(|c| mask & (1 << c) != 0)
        .map(|c| r.ops[c])
        .sum::<u64>() as f64
        / r.secs
}

fn quantiles(r: &RunOut, class: usize) -> [f64; 3] {
    [0.5, 0.99, 0.999].map(|q| r.hist[class].quantile(q) as f64 / 1000.0)
}

fn cell(runs: &[RunOut], m: Metric) -> String {
    match m {
        Metric::Rate(mask) => {
            let v: Vec<f64> = runs.iter().map(|r| rate(r, mask)).collect();
            let (lo, hi) = (
                v.iter().copied().fold(f64::MAX, f64::min),
                v.iter().copied().fold(0.0, f64::max),
            );
            format!("{} ({}–{})", si(median(v.clone())), si(lo), si(hi))
        }
        Metric::Lat(class) => {
            if runs.iter().all(|r| r.hist[class].n == 0) {
                return "–".into();
            }
            let q: Vec<[f64; 3]> = runs.iter().map(|r| quantiles(r, class)).collect();
            let med = |i: usize| median(q.iter().map(|x| x[i]).collect());
            let lo = q.iter().map(|x| x[2]).fold(f64::MAX, f64::min);
            let hi = q.iter().map(|x| x[2]).fold(0.0, f64::max);
            format!(
                "{:.2} / {:.2} / {:.2} ({lo:.2}–{hi:.2})",
                med(0),
                med(1),
                med(2)
            )
        }
        Metric::Fair => {
            let v = runs
                .iter()
                .map(|r| {
                    let lo = r.per_core.iter().min().copied().unwrap_or(0) as f64;
                    let hi = r.per_core.iter().max().copied().unwrap_or(1).max(1) as f64;
                    lo / hi
                })
                .collect();
            format!("{:.2}", median(v))
        }
    }
}

struct Bench {
    o: Opts,
    cpus: Vec<Option<usize>>,
}

impl Bench {
    fn emit(&self, s: &str) {
        println!("{s}");
    }

    fn csv(&mut self, suite: &str, rep: usize, row: &Row, col: &Col, r: &RunOut) {
        let Some(f) = self.o.csv.as_mut() else { return };
        let mut line = format!(
            "{suite},{rep},{},{},{:?},{},{},{},{},{},{},{},{},{},{:.0}",
            row.labels.join(" "),
            col.name.replace(',', ";"),
            row.w,
            row.k,
            row.n,
            col.lock.name(),
            row.p.work_ns,
            row.p.gap_ns,
            row.p.ipi_ns,
            row.p.change_pct,
            r.migrations,
            r.stalls,
            rate(r, 0b1111)
        );
        for h in &r.hist {
            let _ = write!(
                line,
                ",{},{},{},{},{}",
                h.n,
                h.quantile(0.5),
                h.quantile(0.99),
                h.quantile(0.999),
                h.max
            );
        }
        let v = col.v;
        let _ = writeln!(
            f,
            "{line},{},{},{:?},{:?},{},{:?},{}",
            r.samples, r.preferred, v.lookup, v.adds, v.replica_pct, v.irq, r.retries
        );
    }

    /// Runs every row under every column, `reps` times, the repetitions
    /// outermost so that a burst of outside load spreads over all cells.
    fn suite(
        &mut self,
        name: &str,
        heads: &[&str],
        rows: Vec<Row>,
        cols: Vec<Col>,
        metrics: &[(&str, Metric)],
    ) {
        let started = Instant::now();
        self.emit(&format!(
            "\n## Suite {name}\n\nload average before: {}",
            platform::load_average()
        ));
        let mut runs: Vec<Vec<Vec<RunOut>>> = rows
            .iter()
            .map(|_| cols.iter().map(|_| Vec::new()).collect())
            .collect();
        for rep in 0..self.o.reps {
            for (i, row) in rows.iter().enumerate() {
                for (j, col) in cols.iter().enumerate() {
                    let p = Params {
                        lock: col.lock,
                        dur: self.o.dur,
                        lookup: col.v.lookup,
                        adds: col.v.adds,
                        replica_pct: col.v.replica_pct,
                        irq: col.v.irq,
                        ..row.p
                    };
                    let r = sim::run(row.w, col.cand, row.k, row.n, p, &self.cpus);
                    self.csv(&name.replace(',', ";"), rep, row, col, &r);
                    runs[i][j].push(r);
                }
            }
            eprint!("\r{name}: repetition {}/{} done   ", rep + 1, self.o.reps);
            let _ = std::io::stderr().flush();
        }
        eprintln!();
        self.emit(&format!(
            "load average after: {}; {:.0} s\n",
            platform::load_average(),
            started.elapsed().as_secs_f64()
        ));
        for &(title, metric) in metrics {
            let mut t = format!("{title}\n\n| {} |", heads.join(" | "));
            for c in &cols {
                let _ = write!(t, " {} |", c.name);
            }
            t.push_str(" on P | stalls |\n|");
            t.push_str(&"---|".repeat(heads.len() + cols.len() + 2));
            for (i, row) in rows.iter().enumerate() {
                let _ = write!(t, "\n| {} |", row.labels.join(" | "));
                for cell_runs in &runs[i] {
                    let _ = write!(t, " {} |", cell(cell_runs, metric));
                }
                let all = runs[i].iter().flatten();
                let (s, p) = all
                    .clone()
                    .fold((0, 0), |(s, p), r| (s + r.samples, p + r.preferred));
                let stalls: u64 = all.map(|r| r.stalls).sum();
                let on_p = if s == 0 {
                    "–".into()
                } else {
                    format!("{:.0}%", 100.0 * p as f64 / s as f64)
                };
                let _ = write!(t, " {on_p} | {stalls} |");
            }
            t.push('\n');
            self.emit(&t);
        }
        let mut retries = String::new();
        for (j, c) in cols.iter().enumerate() {
            let tried: u64 = runs.iter().flat_map(|r| &r[j]).map(|r| r.retries).sum();
            let looked: u64 = runs
                .iter()
                .flat_map(|r| &r[j])
                .map(|r| r.ops[0] + r.ops[2])
                .sum();
            if tried > 0 {
                let _ = write!(
                    retries,
                    " {}: {:.2};",
                    c.name,
                    1000.0 * tried as f64 / looked.max(1) as f64
                );
            }
        }
        if !retries.is_empty() {
            self.emit(&format!(
                "Seqlock reads repeated per 1000 lookups:{retries}\n"
            ));
        }
        let mig: u64 = runs.iter().flatten().flatten().map(|r| r.migrations).sum();
        let samples: u64 = runs.iter().flatten().flatten().map(|r| r.samples).sum();
        self.emit(&format!(
            "Migrations seen: {mig} over {samples} samples (one sample per measured operation).\n"
        ));
    }
}

fn candidates(lock: LockKind) -> Vec<Col> {
    [Cand::PerCore, Cand::Fixed(2), Cand::Fixed(4), Cand::Sized]
        .into_iter()
        .map(|cand| col(&cand.name(), cand, lock, FIRST))
        .collect()
}

fn base(o: &Opts) -> Params {
    Params {
        work_ns: 200,
        gap_ns: 1000,
        ipi_ns: 1000,
        tlb_ns: 100,
        switch_ns: 1000,
        change_pct: 10,
        lock: o.lock,
        lookup: FIRST.lookup,
        adds: FIRST.adds,
        replica_pct: FIRST.replica_pct,
        irq: FIRST.irq,
        warmup: Duration::from_millis(20),
        dur: o.dur,
    }
}

const WORKS: [u64; 3] = [50, 200, 1000];
const IPIS: [u64; 2] = [0, 1000];

fn row(labels: Vec<String>, w: Workload, k: usize, n: usize, p: Params) -> Row {
    Row { labels, w, k, n, p }
}

fn run_suite(b: &mut Bench, name: &str) {
    let ks: Vec<usize> = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64]
        .into_iter()
        .filter(|&k| k <= b.o.max_k)
        .collect();
    let even: Vec<usize> = ks.iter().copied().filter(|k| k % 2 == 0).collect();
    let p0 = base(&b.o);
    let lat = |c| Metric::Lat(c);
    let l = b.o.lock;
    match name {
        "locks" => {
            let mut rows = Vec::new();
            for gap in [0, 1000] {
                for w in WORKS {
                    for &k in ks.iter().filter(|&&k| k >= 2) {
                        let p = Params {
                            work_ns: w,
                            gap_ns: gap,
                            ..p0
                        };
                        rows.push(row(
                            vec![gap.to_string(), w.to_string(), k.to_string()],
                            Workload::CapOps,
                            k,
                            k,
                            p,
                        ));
                    }
                }
            }
            let cols = [LockKind::Ttas, LockKind::Ticket, LockKind::Queue]
                .map(|lock| col(lock.name(), Cand::Sized, lock, FIRST))
                .into();
            let m = [
                (
                    "Throughput, operations/s, median (min–max)",
                    Metric::Rate(0b1011),
                ),
                (
                    "Latency of a locked operation, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(0),
                ),
                ("Fairness: fewest operations of a core / most", Metric::Fair),
            ];
            b.suite(
                "locks: one group of k cores, three locks",
                &["gap ns", "W ns", "k"],
                rows,
                cols,
                &m,
            );
        }
        "a1" => {
            let mut rows = Vec::new();
            for change in [1, 10] {
                for ipi in IPIS {
                    for w in WORKS {
                        for &k in &ks {
                            let p = Params {
                                work_ns: w,
                                ipi_ns: ipi,
                                change_pct: change,
                                ..p0
                            };
                            let l = vec![
                                change.to_string(),
                                ipi.to_string(),
                                w.to_string(),
                                k.to_string(),
                            ];
                            rows.push(row(l, Workload::CapOps, k, k, p));
                        }
                    }
                }
            }
            let m = [
                (
                    "Throughput, operations/s, median (min–max)",
                    Metric::Rate(0b1011),
                ),
                (
                    "Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(0),
                ),
                ("Change latency (derive/delete), µs", lat(1)),
            ];
            let cols = candidates(b.o.lock);
            b.suite(
                "a1: capability operations of one k-thread process",
                &["change %", "ipi ns", "W ns", "k"],
                rows,
                cols,
                &m,
            );
        }
        "a2" | "a3" => {
            let w = if name == "a2" {
                Workload::Ipc
            } else {
                Workload::Signal
            };
            let mut rows = Vec::new();
            for ipi in IPIS {
                for work in WORKS {
                    for &k in &even {
                        let p = Params {
                            work_ns: work,
                            ipi_ns: ipi,
                            ..p0
                        };
                        rows.push(row(
                            vec![ipi.to_string(), work.to_string(), k.to_string()],
                            w,
                            k,
                            k,
                            p,
                        ));
                    }
                }
            }
            let (title, what) = if name == "a2" {
                (
                    "a2: IPC call and reply, k/2 pairs of one process",
                    "Round trip",
                )
            } else {
                ("a3: k-1 threads signal one notify", "Signal call")
            };
            let lt = format!("{what} latency, µs: p50 / p99 / p99.9 (p99.9 min–max)");
            let m = [
                (
                    "Throughput, operations/s, median (min–max)",
                    Metric::Rate(0b1),
                ),
                (lt.as_str(), lat(0)),
            ];
            b.suite(
                title,
                &["ipi ns", "W ns", "k"],
                rows,
                candidates(b.o.lock),
                &m,
            );
        }
        "b" => {
            let mut rows = Vec::new();
            for (who, init) in [("one", Initiators::One), ("all", Initiators::All)] {
                for ipi in IPIS {
                    for work in WORKS {
                        for &k in ks.iter().filter(|&&k| k >= 2) {
                            let p = Params {
                                work_ns: work,
                                ipi_ns: ipi,
                                ..p0
                            };
                            let l =
                                vec![who.into(), ipi.to_string(), work.to_string(), k.to_string()];
                            rows.push(row(l, Workload::Unmap(init), k, k, p));
                        }
                    }
                }
            }
            let m = [
                ("Throughput, unmaps/s, median (min–max)", Metric::Rate(0b1)),
                (
                    "Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(0),
                ),
            ];
            b.suite(
                "b: unmap seen by every core of a k-thread process",
                &["initiators", "ipi ns", "W ns", "k"],
                rows,
                candidates(b.o.lock),
                &m,
            );
        }
        "c" => {
            let mut rows = Vec::new();
            let big = b.o.max_k;
            for n in [4.min(big), big] {
                for work in [200, 1000] {
                    for k in [1, 2, 3, 5, 6].into_iter().filter(|&k| k < n) {
                        let p = Params {
                            work_ns: work,
                            ..p0
                        };
                        rows.push(row(
                            vec![n.to_string(), work.to_string(), k.to_string()],
                            Workload::Mixed,
                            k,
                            n,
                            p,
                        ));
                    }
                }
            }
            let m = [
                (
                    "Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(2),
                ),
                ("Independent processes: operations/s", Metric::Rate(0b100)),
                ("The k-thread process: operations/s", Metric::Rate(0b1011)),
            ];
            b.suite(
                "c: one k-thread process and n-k single-thread processes",
                &["n", "W ns", "k"],
                rows,
                candidates(b.o.lock),
                &m,
            );
        }
        "d" => {
            let mut rows = Vec::new();
            for switch in [0, 1000] {
                for work in [200, 1000] {
                    for r in [0, 100, 1000, 10_000] {
                        let p = Params {
                            work_ns: work,
                            switch_ns: switch,
                            ..p0
                        };
                        let l = vec![switch.to_string(), work.to_string(), r.to_string()];
                        rows.push(row(l, Workload::Move { rate: r }, 1, 3, p));
                    }
                }
            }
            let cols = vec![col("3 sized", Cand::Sized, b.o.lock, FIRST)];
            let m = [
                (
                    "Throughput of the 3 cores, operations/s",
                    Metric::Rate(0b111),
                ),
                (
                    "Kernel operations other than the first after a move, µs",
                    lat(0),
                ),
                ("Move (quiesce, leave, switch, join), µs", lat(1)),
                ("First operation after a move, µs", lat(2)),
            ];
            b.suite(
                "d: one core moving between group A and group B (one fixed core each)",
                &["switch ns", "W ns", "moves/s"],
                rows,
                cols,
                &m,
            );
        }
        "a1f" => {
            let mut rows = Vec::new();
            for change in [1, 10] {
                for ipi in IPIS {
                    for w in WORKS {
                        for &k in ks.iter().filter(|&&k| k >= 2) {
                            let p = Params {
                                work_ns: w,
                                ipi_ns: ipi,
                                change_pct: change,
                                ..p0
                            };
                            let l = vec![
                                change.to_string(),
                                ipi.to_string(),
                                w.to_string(),
                                k.to_string(),
                            ];
                            rows.push(row(l, Workload::CapOps, k, k, p));
                        }
                    }
                }
            }
            let lazy = Variant {
                adds: Adds::Lazy,
                ..FIRST
            };
            let cheap = Variant {
                replica_pct: 25,
                ..FIRST
            };
            let per_msg = Variant {
                irq: Irq::PerMessage,
                ..FAIR
            };
            let cols = vec![
                col("1 first", Cand::PerCore, l, FIRST),
                col("1 lazy adds", Cand::PerCore, l, lazy),
                col("1 replica 25%", Cand::PerCore, l, cheap),
                col("1 fair", Cand::PerCore, l, FAIR),
                col("1 fair, irq/msg", Cand::PerCore, l, per_msg),
                col("2/2 first", Cand::Fixed(2), l, FIRST),
                col("2/2 fair", Cand::Fixed(2), l, FAIR),
                col("2/4 first", Cand::Fixed(4), l, FIRST),
                col("2/4 fair", Cand::Fixed(4), l, FAIR),
                col("3 first", Cand::Sized, l, FIRST),
                col("3 fair", Cand::Sized, l, FAIR),
            ];
            let m = [
                (
                    "Throughput, operations/s, median (min–max)",
                    Metric::Rate(0b1011),
                ),
                (
                    "Lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(0),
                ),
                ("Delete (removal) latency, µs", lat(1)),
                ("Derive (addition) latency, µs", lat(3)),
            ];
            let heads = ["change %", "ipi ns", "W ns", "k"];
            b.suite(
                "a1f: capability operations, first and fair settings",
                &heads,
                rows,
                cols,
                &m,
            );
        }
        "a2f" | "a3f" => {
            let w = if name == "a2f" {
                Workload::Ipc
            } else {
                Workload::Signal
            };
            let mut rows = Vec::new();
            for ipi in IPIS {
                for work in WORKS {
                    for &k in &even {
                        let p = Params {
                            work_ns: work,
                            ipi_ns: ipi,
                            ..p0
                        };
                        rows.push(row(
                            vec![ipi.to_string(), work.to_string(), k.to_string()],
                            w,
                            k,
                            k,
                            p,
                        ));
                    }
                }
            }
            // No lookup, addition or copy here: only the interrupt switch
            // applies, and only to messages between nodes.
            let per_msg = Variant {
                irq: Irq::PerMessage,
                ..FIRST
            };
            let cols = vec![
                col("1 batched", Cand::PerCore, l, FIRST),
                col("1 irq/msg", Cand::PerCore, l, per_msg),
                col("2/2", Cand::Fixed(2), l, FIRST),
                col("2/4", Cand::Fixed(4), l, FIRST),
                col("3", Cand::Sized, l, FIRST),
            ];
            let (title, what) = if name == "a2f" {
                ("a2f: IPC call and reply, interrupt switch", "Round trip")
            } else {
                (
                    "a3f: k-1 threads signal one notify, interrupt switch",
                    "Signal call",
                )
            };
            let lt = format!("{what} latency, µs: p50 / p99 / p99.9 (p99.9 min–max)");
            let m = [
                (
                    "Throughput, operations/s, median (min–max)",
                    Metric::Rate(0b1),
                ),
                (lt.as_str(), lat(0)),
            ];
            b.suite(title, &["ipi ns", "W ns", "k"], rows, cols, &m);
        }
        "bf" => {
            let mut rows = Vec::new();
            for (who, init) in [("one", Initiators::One), ("all", Initiators::All)] {
                for ipi in IPIS {
                    for work in WORKS {
                        for &k in ks.iter().filter(|&&k| k >= 2) {
                            let p = Params {
                                work_ns: work,
                                ipi_ns: ipi,
                                ..p0
                            };
                            let l =
                                vec![who.into(), ipi.to_string(), work.to_string(), k.to_string()];
                            rows.push(row(l, Workload::Unmap(init), k, k, p));
                        }
                    }
                }
            }
            let per_msg = Variant {
                irq: Irq::PerMessage,
                ..FAIR
            };
            let cols = vec![
                col("1 first", Cand::PerCore, l, FIRST),
                col("1 fair", Cand::PerCore, l, FAIR),
                col("1 fair, irq/msg", Cand::PerCore, l, per_msg),
                col("2/2 first", Cand::Fixed(2), l, FIRST),
                col("2/2 fair", Cand::Fixed(2), l, FAIR),
                col("2/4 first", Cand::Fixed(4), l, FIRST),
                col("2/4 fair", Cand::Fixed(4), l, FAIR),
                col("3", Cand::Sized, l, FIRST),
            ];
            let m = [
                ("Throughput, unmaps/s, median (min–max)", Metric::Rate(0b1)),
                (
                    "Unmap latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(0),
                ),
            ];
            let heads = ["initiators", "ipi ns", "W ns", "k"];
            b.suite("bf: unmap, first and fair settings", &heads, rows, cols, &m);
        }
        "cf" => {
            let mut rows = Vec::new();
            let big = b.o.max_k;
            for n in [4.min(big), big] {
                for work in [200, 1000] {
                    for k in [1, 2, 3, 5, 6].into_iter().filter(|&k| k < n) {
                        let p = Params {
                            work_ns: work,
                            ..p0
                        };
                        rows.push(row(
                            vec![n.to_string(), work.to_string(), k.to_string()],
                            Workload::Mixed,
                            k,
                            n,
                            p,
                        ));
                    }
                }
            }
            let mut cols = Vec::new();
            for cand in [Cand::PerCore, Cand::Fixed(2), Cand::Fixed(4), Cand::Sized] {
                cols.push(col(&format!("{} first", cand.name()), cand, l, FIRST));
                cols.push(col(&format!("{} fair", cand.name()), cand, l, FAIR));
            }
            let m = [
                (
                    "Independent processes: lookup latency, µs: p50 / p99 / p99.9 (p99.9 min–max)",
                    lat(2),
                ),
                ("Independent processes: operations/s", Metric::Rate(0b100)),
                ("The k-thread process: operations/s", Metric::Rate(0b1011)),
            ];
            b.suite(
                "cf: k-thread process beside single-thread ones, first and fair settings",
                &["n", "W ns", "k"],
                rows,
                cols,
                &m,
            );
        }
        other => panic!("unknown suite {other}"),
    }
}

fn clock_resolution() -> (u128, f64) {
    let mut best = u128::MAX;
    let mut last = Instant::now();
    for _ in 0..200_000 {
        let now = Instant::now();
        let d = (now - last).as_nanos();
        if d > 0 {
            best = best.min(d);
        }
        last = now;
    }
    let t = Instant::now();
    for _ in 0..1_000_000 {
        std::hint::black_box(Instant::now());
    }
    (best, t.elapsed().as_nanos() as f64 / 1e6)
}

fn main() {
    platform::prepare_worker(None);
    let cpus = platform::worker_cpus();
    let o = opts(cpus.len());
    let (res, cost) = clock_resolution();
    let pinned = cpus.iter().flatten().count();
    let mut b = Bench { o, cpus };
    let head = format!(
        "# Multicore trial\n\nhost: {}\npinning: {}\nclock: resolution {res} ns, {cost:.1} ns per read\nreps {}, {} ms per run, max k {}, lock of the group candidates: {}",
        platform::describe(),
        if pinned > 0 {
            format!("{pinned} workers pinned, one per physical core")
        } else {
            "none possible; user-interactive QoS".into()
        },
        b.o.reps,
        b.o.dur.as_millis(),
        b.o.max_k,
        b.o.lock.name()
    );
    b.emit(&head);
    for s in b.o.suites.clone() {
        run_suite(&mut b, &s);
    }
}
