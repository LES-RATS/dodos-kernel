//! Measured trial for the DodOS kernel: may a node run on more than one core
//! at the same time? See README.md for the method.
//!
//! Usage: dodos-trial-multicore
//!        [--suites locks,a1,a2,a3,b,c,d,a1f,a2f,a3f,bf,cf,a1t,a2t,a3t,ct,a3n,br,srv]
//!        [--reps N]
//!        [--ms N] [--max-k N] [--lock ttas|ticket|queue] [--csv FILE]

use dodos_trial_multicore::bench::{self, Opts, Sinks};
use dodos_trial_multicore::platform;
use std::io::Write as _;
use std::time::Duration;

fn main() {
    platform::prepare_worker(None);
    let cpus = platform::worker_cpus();
    let mut o = Opts::new(cpus.len());
    let mut csv: Option<std::fs::File> = None;
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
            "--lock" => o.lock = bench::parse_lock(v),
            "--csv" => {
                let mut f = std::fs::File::create(v).expect("cannot create the CSV file");
                writeln!(f, "{}", bench::csv_head()).expect("CSV write");
                csv = Some(f);
            }
            other => panic!("unknown option {other}; see the head of main.rs"),
        }
    }
    let pinned = cpus.iter().flatten().count();
    let pinning = if pinned > 0 {
        format!("{pinned} workers pinned, one per physical core")
    } else {
        "none possible; user-interactive QoS".into()
    };
    let mut out = |s: &str| println!("{s}");
    let mut progress = |s: &str| {
        eprint!("\r{s}   ");
        let _ = std::io::stderr().flush();
    };
    let mut write_csv = |s: &str| {
        if let Some(f) = csv.as_mut() {
            writeln!(f, "{s}").expect("CSV write");
        }
    };
    let sinks = Sinks {
        out: &mut out,
        csv: Some(&mut write_csv),
        progress: &mut progress,
    };
    bench::run(o, cpus, &pinning, sinks);
    eprintln!();
}
