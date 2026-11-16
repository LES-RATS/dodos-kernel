//! Prepares the machine at every boot: the bootstrap core's block and IDT,
//! the APIC mode, the TSC rate, the page tables, the other cores.
//!
//! The APIC mode: the kernel will use x2APIC (kernel.md, gate entries "the
//! x2APIC end of interrupt"), so when the firmware left xAPIC mode the image
//! moves every core to x2APIC, the bootstrap core first. The firmware keeps
//! running on the bootstrap core afterwards; if its timer stops there, the
//! line `F x2apic firmware-timer=stopped` says so. A hang during the switch
//! is remembered (`x2apic-try` without `x2apic-ok` in STATE.TXT) and the
//! next boots keep xAPIC mode. `x2apic=keep` in CONFIG.TXT keeps it too.

use crate::run::Ctx;
use crate::{apic, con, efi, idt, out, paging, percpu, smp, tsc, x86};
use alloc::string::String;

fn st() -> *mut efi::SystemTable {
    con::CON.lock().expect("console").as_ref().expect("console").st
}

/// Whether a firmware timer event still fires (EVT_TIMER, TimerRelative,
/// 10 ms) within 500 ms.
fn firmware_timer_runs() -> bool {
    let st = st();
    // SAFETY: boot services are valid; the event is closed after.
    unsafe {
        let bs = (*st).boot_services;
        let mut ev: efi::Event = core::ptr::null_mut();
        if ((*bs).create_event)(0x8000_0000, 0, 0, 0, &mut ev) != efi::SUCCESS {
            return false;
        }
        ((*bs).set_timer)(ev, 2, 100_000);
        let end = x86::rdtsc() + tsc::ns_to_ticks(500_000_000);
        let mut fired = false;
        while x86::rdtsc() < end {
            if ((*bs).check_event)(ev) == efi::SUCCESS {
                fired = true;
                break;
            }
        }
        ((*bs).close_event)(ev);
        fired
    }
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    percpu::install(0, 0);
    idt::install();
    apic::detect();
    tsc::calibrate();
    let can_x2 = x86::bit(x86::cpuid(1, 0).ecx, 21);
    let tried = !cx.records("x2apic-try").is_empty();
    let ok = !cx.records("x2apic-ok").is_empty();
    let firmware_mode = if apic::x2() { "x2apic" } else { "xapic" };
    if !apic::x2() && can_x2 && cx.get("x2apic") != Some("keep") && (!tried || ok) {
        if !tried {
            cx.record("x2apic-try");
        }
        apic::enter_x2apic_local();
        apic::detect();
        let runs = firmware_timer_runs();
        out!(
            "I x2apic switched=1 firmware-timer={}",
            if runs { "runs" } else { "stopped" }
        );
        if !runs {
            out!("F x2apic firmware-timer=stopped");
        }
        if !ok {
            cx.record("x2apic-ok");
        }
    }
    out!(
        "I apic firmware-mode={firmware_mode} mode={} x2apic-supported={can_x2} bsp-apic={}",
        if apic::x2() { "x2apic" } else { "xapic" },
        apic::id()
    );
    paging::build(st());
    let n = smp::start(st())?;
    out!("I smp started={n} cores={}", smp::cores().len());
    crate::identity::assign_kinds(cx)?;
    for (i, c) in smp::cores().iter().enumerate() {
        out!(
            "I core index={i} apic={} thread={} role={:?} kind={} state={}",
            c.apic,
            c.thread,
            c.role,
            c.kind.name(),
            smp::state(i)
        );
    }
    Ok(())
}
