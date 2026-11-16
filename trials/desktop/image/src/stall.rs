//! The longest time a core loses to the firmware (system management mode)
//! or anything else below the image: every worker reads the TSC in a tight
//! loop with interrupts masked for `seconds`, and keeps the longest gap
//! between two reads and how many gaps passed 10 µs, 100 µs and 1 ms. With
//! MSR_SMI_COUNT before and after where the core has it (model-specific).
//! Sets the floor of `STALE_NS`, `STUCK_NS` and `CORE_ANSWER_NS`
//! (kernel-interface.md, "Numbers": "above the longest firmware stall of the
//! reference machines").

use crate::run::Ctx;
use crate::{out, smp, tsc, x86};
use alloc::string::String;
use alloc::vec::Vec;
use dodos_trial_multicore::rt::Mutex;

#[derive(Default, Clone, Copy)]
struct Gaps {
    max: u64,
    over: [u64; 3],
    smi: Option<(u64, u64)>,
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let secs = cx.num("stall_seconds", if cx.flag("quick") { 1 } else { 30 });
    let on = smp::workers();
    let res: Vec<Mutex<Gaps>> = (0..smp::cores().len()).map(|_| Mutex::new(Gaps::default())).collect();
    let limits = [10_000, 100_000, 1_000_000].map(tsc::ns_to_ticks);
    let span = tsc::ns_to_ticks(secs * 1_000_000_000);
    smp::run_on(&on, secs * 1000 + 30_000, &|i| {
        smp::mark("stall loop");
        let smi0 = x86::safe_rdmsr(x86::MSR_SMI_COUNT).ok();
        let mut g = Gaps::default();
        x86::without_interrupts(|| {
            let start = x86::rdtsc();
            let mut last = start;
            while last - start < span {
                let now = x86::rdtsc();
                let d = now - last;
                if d > g.max {
                    g.max = d;
                }
                for (k, &l) in limits.iter().enumerate() {
                    if d > l {
                        g.over[k] += 1;
                    }
                }
                last = now;
            }
        });
        let smi1 = x86::safe_rdmsr(x86::MSR_SMI_COUNT).ok();
        g.smi = smi0.zip(smi1);
        *res[i].lock().expect("gaps") = g;
    })?;
    for &i in &on {
        let g = *res[i].lock().expect("gaps");
        out!(
            "R stall core={i} kind={} seconds={secs} max_ns={} over_10us={} over_100us={} over_1ms={} smi={}",
            smp::kind(i).name(),
            tsc::ticks_to_ns(g.max),
            g.over[0],
            g.over[1],
            g.over[2],
            g.smi.map_or("none".into(), |(a, b)| alloc::format!("{}", b.wrapping_sub(a)))
        );
    }
    Ok(())
}
