//! The run: configuration, the progress kept on the stick, the steps in
//! order, and what a hang or a panic leaves behind.
//!
//! `\DODOS\STATE.TXT` records `start <step> <boot>` (written and flushed
//! before the step) and `done <step> <boot>`. At the next boot a step that
//! started and never finished is reported as hung (`F <step> hung ...`) and
//! skipped, a finished step is skipped, and the run goes on: after any hang
//! or panic, resetting the machine resumes the run at the next step.
//! `\DODOS\CONFIG.TXT` holds `key=value` lines (README, "Configuration").

use crate::{con, out, tsc, x86};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use dodos_trial_multicore::rt::Mutex;

/// What a step is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Prepares the machine; runs at every boot, never recorded.
    Setup,
    /// A measure that cannot hang the machine by design.
    Measure,
    /// A measure that can hang or reset the machine: asks before it runs
    /// (any key within 3 s skips it) and is skipped by the key S at start.
    Risky,
}

pub struct Step {
    pub name: &'static str,
    pub kind: Kind,
    /// Seconds after which the watchdogs reset the machine.
    pub limit_s: u64,
    pub run: fn(&mut Ctx) -> Result<(), String>,
}

pub struct Ctx {
    pub boot: u32,
    pub config: BTreeMap<String, String>,
    pub state: Vec<String>,
    pub skip_risky: bool,
}

impl Ctx {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.config.get(key).map(String::as_str)
    }
    pub fn num(&self, key: &str, default: u64) -> u64 {
        self.get(key).and_then(parse_num).unwrap_or(default)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.num(key, 0) != 0
    }
    /// Repetitions: `full` normally, `quick` with `quick=1` (QEMU).
    pub fn reps(&self, full: usize, quick: usize) -> usize {
        if self.flag("quick") { quick } else { full }
    }
    /// The state lines of this step, newest last.
    pub fn records(&self, prefix: &str) -> Vec<&str> {
        self.state
            .iter()
            .filter(|l| l.starts_with(prefix))
            .map(String::as_str)
            .collect()
    }
    /// Appends a line to STATE.TXT and flushes it to the stick.
    pub fn record(&mut self, line: &str) {
        let mut l = String::from(line);
        l.push('\n');
        con::append_file("STATE.TXT", l.as_bytes());
        self.state.push(line.to_string());
    }
}

pub fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn parse_config(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

static CURRENT: Mutex<Option<&'static str>> = Mutex::new(None);
static BSP_APIC: AtomicU32 = AtomicU32::new(u32::MAX);
static PANICKING: AtomicBool = AtomicBool::new(false);
static END_HALT: AtomicBool = AtomicBool::new(false);

pub fn is_bsp() -> bool {
    x86::apic_id_cpuid() == BSP_APIC.load(Ordering::Relaxed)
}

fn steps() -> Vec<Step> {
    crate::plan::steps()
}

pub fn main() -> ! {
    BSP_APIC.store(x86::apic_id_cpuid(), Ordering::Relaxed);
    let config = con::read_file("CONFIG.TXT")
        .map(|b| parse_config(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let mut state: Vec<String> = con::read_file("STATE.TXT")
        .map(|b| String::from_utf8_lossy(&b).lines().map(String::from).collect())
        .unwrap_or_default();
    let boot = 1 + state
        .iter()
        .filter_map(|l| l.strip_prefix("boot "))
        .filter_map(|n| n.trim().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    END_HALT.store(config.get("panic").is_some_and(|v| v == "halt"), Ordering::Relaxed);
    out!("I image name=dodos-trial-desktop boot={boot} volume={}", con::has_volume());
    con::line("DodOS desktop measurements. Keys in the next 5 s: S skip the risky steps,");
    con::line("R forget the progress and start over, Q stop now, any other key go on.");
    let k = con::wait_key(5000);
    if k == Some('R') {
        con::replace_file("STATE.TXT", b"");
        state.clear();
        out!("I progress forgotten by key R");
    }
    let mut cx = Ctx {
        boot,
        config,
        state,
        skip_risky: k == Some('S'),
    };
    cx.record(&alloc::format!("boot {boot}"));
    if k == Some('Q') {
        out!("I stopped by key Q");
        finish(&cx);
    }
    for s in steps() {
        run_step(&mut cx, &s);
    }
    out!("I run complete boot={boot}");
    finish(&cx);
}

fn status(cx: &Ctx, name: &str) -> Option<&'static str> {
    let mut last = None;
    for l in &cx.state {
        let mut w = l.split_whitespace();
        let (Some(verb), Some(step)) = (w.next(), w.next()) else {
            continue;
        };
        if step == name {
            last = match verb {
                "start" => Some("start"),
                "done" => Some("done"),
                "resume" => Some("resume"),
                "failed" => Some("failed"),
                _ => last,
            };
        }
    }
    last
}

fn run_step(cx: &mut Ctx, s: &Step) {
    let skipped: Vec<&str> = cx.get("skip").unwrap_or("").split(',').map(str::trim).collect();
    if skipped.contains(&s.name) {
        out!("I step {} skipped by configuration", s.name);
        return;
    }
    if s.kind != Kind::Setup {
        match status(cx, s.name) {
            Some("done") => {
                out!("I step {} already done", s.name);
                return;
            }
            Some("failed") => {
                out!("I step {} failed in an earlier boot; skipped", s.name);
                return;
            }
            Some("start") => {
                out!("F {} hung in an earlier boot (started, never finished); skipped", s.name);
                cx.record(&alloc::format!("failed {} {}", s.name, cx.boot));
                con::flush();
                return;
            }
            _ => {}
        }
    }
    if s.kind == Kind::Risky {
        if cx.skip_risky {
            out!("I step {} skipped by key S", s.name);
            return;
        }
        con::line(&alloc::format!("Next: {} (can hang the machine). Any key within 3 s skips it.", s.name));
        if con::wait_key(3000).is_some() {
            out!("I step {} skipped by key", s.name);
            cx.record(&alloc::format!("done {} {}", s.name, cx.boot));
            return;
        }
    }
    if s.kind != Kind::Setup {
        cx.record(&alloc::format!("start {} {}", s.name, cx.boot));
    }
    *CURRENT.lock().expect("current") = Some(s.name);
    out!("I step {} begins", s.name);
    con::flush();
    con::watchdog(s.limit_s as usize + 60);
    crate::smp::watch(tsc::ns_to_ticks(s.limit_s * 1_000_000_000));
    let t = x86::rdtsc();
    let r = (s.run)(cx);
    crate::smp::watch(0);
    con::watchdog(0);
    let secs = tsc::ticks_to_ns(x86::rdtsc() - t) / 1_000_000_000;
    match r {
        Ok(()) => out!("I step {} ends seconds={secs}", s.name),
        Err(e) => out!("F {} error=\"{e}\" seconds={secs}", s.name),
    }
    con::flush();
    if s.kind != Kind::Setup {
        cx.record(&alloc::format!("done {} {}", s.name, cx.boot));
    }
    *CURRENT.lock().expect("current") = None;
}

fn finish(cx: &Ctx) -> ! {
    con::flush();
    let end = cx.get("end").unwrap_or("wait");
    if end == "shutdown" {
        crate::reset::firmware(crate::efi::RESET_SHUTDOWN);
    }
    con::line("Done. Copy \\DODOS from the stick. Any key powers the machine off.");
    while con::key().is_none() {
        con::stall_us(50_000);
    }
    crate::reset::firmware(crate::efi::RESET_SHUTDOWN);
}

/// A panic on the bootstrap core records itself and the step, then resets
/// the machine so that the next boot resumes after the step (or halts with
/// `panic=halt`). A panic on another core records its message for the
/// bootstrap core and stops that core.
pub fn panicked(info: &PanicInfo) -> ! {
    if !is_bsp() {
        crate::smp::core_panicked(info);
    }
    if PANICKING.swap(true, Ordering::SeqCst) {
        loop {
            x86::cli();
            x86::hlt();
        }
    }
    let mut msg = con::LineBuf(String::new());
    let _ = write!(msg, "{}", info.message());
    if let Some(l) = info.location() {
        let _ = write!(msg, " at {}:{}", l.file(), l.line());
    }
    let step = CURRENT.try_lock().ok().and_then(|g| *g).unwrap_or("none");
    out!("F panic step={step} message=\"{}\"", msg.0.replace('"', "'"));
    con::flush();
    if step != "none" {
        con::append_file("STATE.TXT", alloc::format!("failed {step} 0\n").as_bytes());
    }
    if END_HALT.load(Ordering::Relaxed) {
        con::line("Halted after the panic (panic=halt).");
        loop {
            x86::hlt();
        }
    }
    con::line("Resetting in 10 s; the run resumes after this step.");
    con::stall_us(10_000_000);
    crate::reset::firmware(crate::efi::RESET_WARM);
}
