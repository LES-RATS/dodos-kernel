//! The two clocks of a microbenchmark: the TSC (wall time at the base
//! frequency) and, when the core has it, fixed counter 1, which counts the
//! core's own unhalted cycles (CPU_CLK_UNHALTED.CORE: SDM vol. 4 table 2-2,
//! IA32_FIXED_CTR1; enabled by IA32_FIXED_CTR_CTRL bits 4-5 and
//! IA32_PERF_GLOBAL_CTRL bit 33; architectural performance monitoring
//! version and fixed counters from CPUID leaf 0AH, vol. 1 table 21-30).
//! RDPMC with ECX[31:16] = 4000H reads fixed counter ECX[15:0] (vol. 2B
//! RDPMC: "Fixed-function counters use type 4000H").

use crate::x86;
use core::arch::asm;

/// Enables fixed counter 1 on the calling core. Returns whether it counts.
pub fn enable_cycles() -> bool {
    let a = x86::cpuid(0xA, 0);
    let version = a.eax & 0xFF;
    let fixed = a.edx & 0x1F;
    if version < 2 || fixed < 2 {
        return false;
    }
    let ctrl = x86::safe_rdmsr(x86::IA32_FIXED_CTR_CTRL).unwrap_or(0);
    let global = x86::safe_rdmsr(x86::IA32_PERF_GLOBAL_CTRL).unwrap_or(0);
    // SAFETY: enabling a counter changes nothing the image relies on.
    let ok = unsafe {
        x86::safe_wrmsr(x86::IA32_FIXED_CTR_CTRL, ctrl | 0x30).is_ok()
            && x86::safe_wrmsr(x86::IA32_PERF_GLOBAL_CTRL, global | 1 << 33).is_ok()
    };
    if !ok {
        return false;
    }
    let c0 = cycles();
    for _ in 0..1000 {
        x86::pause();
    }
    cycles() > c0
}

#[inline(always)]
pub fn cycles() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: fixed counter 1, enabled by `enable_cycles` (callers check).
    unsafe {
        asm!("rdpmc", in("ecx") 0x4000_0001u32, out("eax") lo, out("edx") hi, options(nomem, nostack))
    };
    u64::from(hi) << 32 | u64::from(lo)
}

/// Both clocks, after every earlier instruction (LFENCE first).
#[inline(always)]
pub fn now(pmu: bool) -> (u64, u64) {
    let t = x86::tsc_begin();
    let c = if pmu { cycles() } else { 0 };
    (t, c)
}

/// Measures `f` `samples` times; returns the TSC ticks and core cycles of
/// each run.
pub fn sample(
    samples: usize,
    pmu: bool,
    mut f: impl FnMut(),
) -> (alloc::vec::Vec<u64>, alloc::vec::Vec<u64>) {
    let mut t = alloc::vec::Vec::with_capacity(samples);
    let mut c = alloc::vec::Vec::with_capacity(samples);
    f();
    for _ in 0..samples {
        let a = now(pmu);
        f();
        let b = now(pmu);
        t.push(b.0 - a.0);
        c.push(b.1.wrapping_sub(a.1));
    }
    (t, c)
}
