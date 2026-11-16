//! DodOS desktop measurement image: a UEFI application that runs, unattended,
//! every measure the design documents ask of the reference desktop, and
//! writes the results to the screen, the serial port when there is one, and
//! `\DODOS\RESULTS.TXT` on its own stick. The plan, the meaning of every
//! result line and what QEMU exercised are in `trials/desktop/README.md`.

#![no_std]
#![no_main]

extern crate alloc;

mod acpi;
mod apic;
mod clock;
mod con;
mod efi;
mod gate;
mod heap;
mod identity;
mod idt;
mod paging;
mod percpu;
mod plan;
mod reset;
mod run;
mod setup;
mod smp;
mod stats;
mod tsc;
mod x86;

use core::panic::PanicInfo;

/// Bytes taken from the firmware for the heap of every core.
const HEAP_BYTES: usize = 256 << 20;

#[unsafe(no_mangle)]
extern "efiapi" fn efi_main(image: efi::Handle, st: *mut efi::SystemTable) -> efi::Status {
    // SAFETY: the firmware's arguments; boot services are valid until the
    // image ends (it never calls ExitBootServices).
    unsafe {
        let bs = (*st).boot_services;
        let mut heap = 0u64;
        let pages = HEAP_BYTES / 4096;
        if ((*bs).allocate_pages)(efi::ALLOCATE_ANY_PAGES, efi::LOADER_DATA, pages, &mut heap)
            != efi::SUCCESS
        {
            return efi::NOT_FOUND;
        }
        heap::init(heap as usize, HEAP_BYTES);
        con::init(st, image);
        acpi::init(st);
    }
    run::main();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    run::panicked(info)
}
