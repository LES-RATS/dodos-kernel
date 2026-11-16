//! The time-stamp counter as the clock of every measurement. Its rate is
//! taken from CPUID leaf 15H when complete (TSC = ART × EBX / EAX, ART at
//! ECX Hz; SDM vol. 1 table 21-53), and measured against the ACPI PM timer
//! (3.579545 MHz, ACPI 6.5 FADT `PM_TMR_BLK`, from the EDK2 header) or the
//! firmware's Stall. The measured value is used; all three are reported.
//! The TSC is not changed by INIT (vol. 3A table 12-1), so a core restarted
//! by INIT and SIPI reads the same clock.

use crate::{acpi, con, out, x86};
use core::sync::atomic::{AtomicU64, Ordering};

static HZ: AtomicU64 = AtomicU64::new(1_000_000_000);

pub fn hz() -> u64 {
    HZ.load(Ordering::Relaxed)
}

pub fn ticks_to_ns(t: u64) -> u64 {
    (u128::from(t) * 1_000_000_000 / u128::from(hz())) as u64
}

pub fn ticks_to_ns_f(t: u64) -> f64 {
    t as f64 * 1e9 / hz() as f64
}

pub fn ns_to_ticks(ns: u64) -> u64 {
    (u128::from(ns) * u128::from(hz()) / 1_000_000_000) as u64
}

pub fn now_ns() -> u64 {
    ticks_to_ns(x86::rdtsc())
}

/// Spins for `ns` on the TSC.
pub fn spin_ns(ns: u64) {
    let end = x86::rdtsc() + ns_to_ticks(ns);
    while x86::rdtsc() < end {
        x86::pause();
    }
}

fn pm_timer(port: u16) -> u32 {
    // SAFETY: the PM timer port given by the FADT; reading has no effect.
    unsafe { x86::inl(port) & 0x00FF_FFFF }
}

/// Measures the rate against the PM timer over about 100 ms (24-bit
/// counter: wraps every 4.7 s, so one wrap at most is handled).
fn against_pm(port: u16) -> u64 {
    let start = pm_timer(port);
    let t0 = x86::rdtsc();
    let want = 357_954; // 100 ms of 3.579545 MHz
    let mut now;
    loop {
        now = pm_timer(port);
        if (now.wrapping_sub(start) & 0x00FF_FFFF) >= want {
            break;
        }
    }
    let t1 = x86::rdtsc();
    let pm = u64::from(now.wrapping_sub(start) & 0x00FF_FFFF);
    (u128::from(t1 - t0) * 3_579_545 / u128::from(pm)) as u64
}

fn against_stall() -> u64 {
    let t0 = x86::rdtsc();
    con::stall_us(100_000);
    (x86::rdtsc() - t0) * 10
}

pub fn calibrate() {
    let max = x86::cpuid(0, 0).eax;
    let l15 = if max >= 0x15 { x86::cpuid(0x15, 0) } else { x86::Cpuid::default() };
    let cpuid_hz = if l15.eax != 0 && l15.ebx != 0 && l15.ecx != 0 {
        u64::from(l15.ecx) * u64::from(l15.ebx) / u64::from(l15.eax)
    } else {
        0
    };
    let base_mhz = if max >= 0x16 { x86::cpuid(0x16, 0).eax & 0xFFFF } else { 0 };
    let pm = acpi::pm_timer_port().map(against_pm);
    let stall = against_stall();
    let used = pm.unwrap_or(stall);
    HZ.store(used, Ordering::Relaxed);
    out!(
        "I tsc hz={used} source={} cpuid15_hz={cpuid_hz} cpuid16_base_mhz={base_mhz} pm_hz={} stall_hz={stall}",
        if pm.is_some() { "pm-timer" } else { "stall" },
        pm.unwrap_or(0)
    );
}
