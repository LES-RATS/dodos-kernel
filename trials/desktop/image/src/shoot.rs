//! A TLB invalidation of other cores by interrupt and acknowledgement, as a
//! node of several cores does it on x86-64 (kernel-interface.md, "TLB, one
//! core and several": one interrupt, one gate call and one acknowledgement
//! per other core that loaded the tag) with 1, 2, 5 and 13 other cores
//! (D-129; the open point "a shootdown inside a node of 2 to 14 cores").
//!
//! The initiator changes the entry of window page 30 (between two frames),
//! invalidates it locally, sends the shootdown vector to each target (one
//! ICR write per target, `send=unicast`, or one per x2APIC cluster in
//! logical mode, `send=logical`, SDM vol. 3A 13.12.10.2), and waits until
//! every target's handler has run INVLPG and counted. Targets either spin
//! with interrupts enabled (`targets=busy`) or halt (`targets=idle`; the
//! kernel would not wait for an idle core, kernel-interface.md, so this
//! line shows what waking one costs). Target sets: the initiator's kind,
//! the other kind, and mixed. With 13 others the bootstrap core is one of
//! the targets (it takes the vector through its IDT; the firmware's own
//! interrupts keep running there).

use crate::idt::{DODOS_SHOOT_ACKS, DODOS_SHOOT_VA, V_SHOOT};
use crate::ipi::{self, Target};
use crate::paging::{self, A, D, P, RW};
use crate::run::Ctx;
use crate::smp::{self, CoreKind};
use crate::{apic, stats, tsc, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use dodos_trial_multicore::rt::Mutex;

static DONE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Send {
    Unicast,
    Logical,
}

fn shoot(targets: &[u32], send: Send, frames: (u64, u64), k: u64) -> u64 {
    let n = targets.len() as u64;
    let va = paging::window_va(30);
    paging::map(30, if k % 2 == 0 { frames.0 } else { frames.1 }, P | RW | A | D);
    DODOS_SHOOT_VA.store(va, Ordering::Relaxed);
    DODOS_SHOOT_ACKS.store(0, Ordering::Release);
    let t0 = x86::tsc_begin();
    match send {
        Send::Unicast => {
            for &t in targets {
                apic::send(t, apic::FIXED, V_SHOOT);
            }
        }
        Send::Logical => {
            let mut clusters: Vec<(u32, u32)> = Vec::new();
            for &t in targets {
                let (c, bit) = (t >> 4, 1u32 << (t & 0xF));
                match clusters.iter_mut().find(|x| x.0 == c) {
                    Some(x) => x.1 |= bit,
                    None => clusters.push((c, bit)),
                }
            }
            for (c, m) in clusters {
                apic::send_logical(c, m, V_SHOOT);
            }
        }
    }
    let end = t0 + tsc::ns_to_ticks(100_000_000);
    while DODOS_SHOOT_ACKS.load(Ordering::Acquire) < n {
        assert!(x86::rdtsc() < end, "shootdown not acknowledged within 100 ms");
        x86::pause();
    }
    x86::tsc_end() - t0
}

fn measure(init: usize, targets: &[usize], idle: bool, send: Send, n: usize) -> Result<Vec<u64>, String> {
    DONE.store(0, Ordering::Release);
    let out: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    let apics: Vec<u32> = targets.iter().map(|&t| smp::cores()[t].apic).collect();
    let frames = (paging::frame(), paging::frame());
    let ready = AtomicU64::new(0);
    let mut on = Vec::from(targets);
    on.push(init);
    // The bootstrap core keeps the firmware's page tables: it gets the
    // window only for this measurement.
    let bsp_in = targets.contains(&0);
    let fw_cr3 = x86::cr3();
    smp::run_on(&on, 300_000, &|i| {
        if i == init {
            smp::mark("shoot initiator");
            while ready.load(Ordering::Acquire) < targets.len() as u64 {
                x86::pause();
            }
            let gap = if idle { 50_000 } else { 2_000 };
            let mut v = Vec::with_capacity(n);
            for k in 0..=n as u64 {
                tsc::spin_ns(gap);
                let t = shoot(&apics, send, frames, k);
                if k > 0 {
                    v.push(t);
                }
            }
            *out.lock().expect("out") = v;
            DONE.store(1, Ordering::Release);
            for &t in &apics {
                apic::send(t, apic::FIXED, crate::idt::V_FLAG);
            }
        } else {
            smp::mark("shoot target");
            if i == 0 {
                // SAFETY: the image's tables map all the firmware's memory
                // as the firmware's do; restored below.
                unsafe { x86::set_cr3(paging::root()) };
            }
            let was = x86::interrupts_enabled();
            x86::sti();
            ready.fetch_add(1, Ordering::AcqRel);
            while DONE.load(Ordering::Acquire) == 0 {
                if idle {
                    x86::hlt();
                } else {
                    x86::pause();
                }
            }
            if !was {
                x86::cli();
            }
            if i == 0 {
                // SAFETY: the firmware's root, read before.
                unsafe { x86::set_cr3(fw_cr3) };
            }
        }
    })?;
    let _ = bsp_in;
    Ok(core::mem::take(&mut *out.lock().expect("out")))
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let n = cx.reps(5_000, 50);
    let w = smp::workers();
    let of = |k: CoreKind| w.iter().copied().filter(|&i| smp::kind(i) == k).collect::<Vec<_>>();
    let (p, e) = (of(CoreKind::P), of(CoreKind::E));
    let init = *w.first().ok_or("no worker")?;
    let others: Vec<usize> = w.iter().copied().filter(|&i| i != init).collect();
    let same: Vec<usize> = others.iter().copied().filter(|&i| smp::kind(i) == smp::kind(init)).collect();
    let other_kind: Vec<usize> = others.iter().copied().filter(|&i| smp::kind(i) != smp::kind(init)).collect();
    let sends = if apic::x2() { &[Send::Unicast, Send::Logical][..] } else { &[Send::Unicast][..] };
    let mut cases: Vec<(&str, Vec<usize>)> = Vec::new();
    for k in [1usize, 2, 5, 13] {
        if same.len() >= k {
            cases.push(("same", same[..k].to_vec()));
        }
        if other_kind.len() >= k {
            cases.push(("other", other_kind[..k].to_vec()));
        }
        // Mixed: alternate kinds; at 13 add the bootstrap core when the
        // workers alone are too few.
        let mut mixed: Vec<usize> = Vec::new();
        let (mut a, mut b) = (same.iter(), other_kind.iter());
        while mixed.len() < k {
            match (a.next(), b.next()) {
                (None, None) => break,
                (x, y) => mixed.extend(x.into_iter().chain(y).copied()),
            }
        }
        mixed.truncate(k);
        if mixed.len() < k && k == 13 && mixed.len() + 1 == k {
            mixed.push(0);
        }
        if mixed.len() == k {
            cases.push(("mixed", mixed));
        }
    }
    for (set, targets) in cases {
        for &send in sends {
            for idle in [false, true] {
                let mut v = measure(init, &targets, idle, send, n)?;
                let kinds: String = targets.iter().map(|&t| smp::kind(t).name()).collect();
                let k = alloc::format!(
                    "initiator={init} initiator_kind={} others={} set={set} kinds={kinds} targets={} send={} bsp_target={}",
                    smp::kind(init).name(),
                    targets.len(),
                    if idle { "idle" } else { "busy" },
                    if send == Send::Unicast { "unicast" } else { "logical" },
                    u8::from(targets.contains(&0))
                );
                stats::report_ticks("shoot", &k, &mut v);
            }
        }
        crate::con::flush();
    }
    let _ = (p, e, ipi::mwait_hints, Target::Spin);
    Ok(())
}
