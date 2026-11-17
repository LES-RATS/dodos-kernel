//! Ways to reset the machine: the firmware's ResetSystem (bootstrap core
//! only), and the ACPI reset register, usable from any core (the watchdog).

use crate::{acpi, con, x86};

/// ResetSystem of the runtime services (`EFI_RESET_TYPE`).
pub fn firmware(kind: u32) -> ! {
    let st = con::CON.lock().expect("console").as_ref().map(|c| c.st);
    if let Some(st) = st {
        // SAFETY: runtime services are valid; ResetSystem does not return.
        unsafe { ((*(*st).runtime_services).reset_system)(kind, 0, 0, core::ptr::null()) }
    }
    acpi_reset()
}

/// Writes the FADT reset register (system I/O space 1 or memory space 0),
/// then the reset control port 0xCF9 with 0x06 as Linux does
/// (arch/x86/kernel/reboot.c, BOOT_CF9: "Request hard reset"), then stops.
pub fn acpi_reset() -> ! {
    if let Some((space, addr, value)) = acpi::reset_reg() {
        // SAFETY: the register the firmware names for a reset.
        unsafe {
            match space {
                1 => x86::outb(addr as u16, value),
                0 => (addr as *mut u8).write_volatile(value),
                _ => {}
            }
        }
    }
    // SAFETY: the reset control port of Intel chipsets; only reached when
    // the FADT register did not reset the machine.
    unsafe {
        x86::outb(0xCF9, 0x02);
        x86::outb(0xCF9, 0x06);
    }
    loop {
        x86::cli();
        x86::hlt();
    }
}
