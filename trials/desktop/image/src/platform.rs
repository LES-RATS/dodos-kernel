//! What the board offers for debugging and interrupts (P-007, D-058; the
//! open points "the debugging means on the reference desktop" and "the end
//! of interrupt of a level line from any node"). Reads only; nothing is
//! configured.
//!
//! - The ACPI tables present, among them SPCR and DBG2 (firmware's own
//!   description of a console or debug port).
//! - A 16550 answering at COM1 (`con`), and every PCI function of class
//!   07h subclass 00h (serial controller).
//! - Every xHCI controller (class 0C0330h) and whether its extended
//!   capabilities list the USB Debug Capability (capability ID 10; the list
//!   starts at HCCPARAMS1[31:16] dwords from the register base, each entry
//!   with its ID in bits 7:0 and the next offset in dwords in bits 15:8:
//!   Linux drivers/usb/host/xhci-ext-caps.h, XHCI_EXT_CAPS_DEBUG = 10;
//!   the xHCI specification itself could not be fetched: not verified).
//! - Every EHCI controller (0C0320h) and whether it has the PCI debug port
//!   capability (ID 0Ah).
//! - PCI configuration through the MCFG's ECAM windows (offsets from Linux
//!   include/uapi/linux/pci_regs.h: vendor 00h, status 06h bit 4 capability
//!   list, class 08h, header type 0Eh bit 7 multi-function, BAR0 10h,
//!   capability pointer 34h).
//! - Each I/O APIC's version (register 1, bits 7:0; index at offset 0,
//!   data at 10h): version 20h and later have the EOI register at 40h, as
//!   Linux arch/x86/kernel/apic/io_apic.c reads it ("IO-APIC versions below
//!   0x20 don't support EOI register"); and the local APIC version
//!   register's bit 24, support for EOI-broadcast suppression (SDM vol. 3A
//!   13.4.8). Whether a level line ends correctly from another node needs a
//!   kernel and a level-triggered device: not measured here.

use crate::acpi::{self, rd8, rd32};
use crate::run::Ctx;
use crate::{apic, con, out};
use alloc::string::String;
use alloc::vec::Vec;

fn cfg(ecam: u64, bus: u8, dev: u8, func: u8) -> u64 {
    ecam + (u64::from(bus) << 20 | u64::from(dev) << 15 | u64::from(func) << 12)
}

struct Function {
    bus: u8,
    dev: u8,
    func: u8,
    base: u64,
    vendor: u32,
    class: u32,
}

fn functions() -> Vec<Function> {
    let mut out = Vec::new();
    for (ecam, seg, first, last) in acpi::ecam() {
        if seg != 0 {
            continue;
        }
        let ecam = ecam - (u64::from(first) << 20);
        for bus in first..=last {
            for dev in 0..32u8 {
                for func in 0..8u8 {
                    let b = cfg(ecam, bus, dev, func);
                    let id = rd32(b);
                    if id & 0xFFFF == 0xFFFF || id == 0 {
                        if func == 0 {
                            break;
                        }
                        continue;
                    }
                    out.push(Function {
                        bus,
                        dev,
                        func,
                        base: b,
                        vendor: id,
                        class: rd32(b + 8) >> 8,
                    });
                    if func == 0 && rd8(b + 0x0E) & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }
    out
}

fn bar64(f: &Function) -> u64 {
    let lo = rd32(f.base + 0x10);
    let hi = if lo & 0b110 == 0b100 {
        rd32(f.base + 0x14)
    } else {
        0
    };
    (u64::from(hi) << 32 | u64::from(lo)) & !0xF
}

/// The IDs of the PCI capabilities of `f`.
fn pci_caps(f: &Function) -> Vec<u8> {
    let mut ids = Vec::new();
    if rd32(f.base + 4) >> 16 & 0x10 == 0 {
        return ids;
    }
    let mut p = rd8(f.base + 0x34) & 0xFC;
    while p != 0 && ids.len() < 48 {
        ids.push(rd8(f.base + u64::from(p)));
        p = rd8(f.base + u64::from(p) + 1) & 0xFC;
    }
    ids
}

/// The xHCI extended capability IDs of the controller at `mmio`.
fn xhci_caps(mmio: u64) -> Vec<u8> {
    let mut ids = Vec::new();
    if mmio == 0 {
        return ids;
    }
    let off = u64::from(rd32(mmio + 0x10) >> 16) * 4;
    if off == 0 {
        return ids;
    }
    let mut p = mmio + off;
    while ids.len() < 64 {
        let v = rd32(p);
        ids.push(v as u8);
        let next = u64::from(v >> 8 & 0xFF) * 4;
        if next == 0 {
            break;
        }
        p += next;
    }
    ids
}

fn ioapic_version(base: u64) -> u32 {
    // SAFETY: the I/O APIC's index and data registers (MADT address);
    // selecting register 1 and reading it changes nothing.
    unsafe {
        (base as *mut u32).write_volatile(1);
        ((base + 0x10) as *const u32).read_volatile()
    }
}

pub fn run(_cx: &mut Ctx) -> Result<(), String> {
    let sigs: Vec<String> = acpi::signatures()
        .iter()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    out!("I acpi tables={}", sigs.join(","));
    out!(
        "R debug.serial com1={} spcr={} dbg2={}",
        u8::from(con::serial_present()),
        u8::from(acpi::table(b"SPCR").is_some()),
        u8::from(acpi::table(b"DBG2").is_some())
    );
    if let Some(s) = acpi::table(b"SPCR") {
        out!(
            "I spcr interface_type={} address_space={} address={:#x}",
            rd8(s + 36),
            rd8(s + 40),
            acpi::rd64(s + 44)
        );
    }
    let fns = functions();
    out!("I pci functions={}", fns.len());
    for f in &fns {
        let at = alloc::format!("{:02x}:{:02x}.{}", f.bus, f.dev, f.func);
        let id = alloc::format!("{:04x}:{:04x}", f.vendor & 0xFFFF, f.vendor >> 16);
        match f.class {
            c if c >> 8 == 0x0700 => out!("R debug.pci_serial at={at} id={id} class={c:#08x}"),
            0x0C0330 => {
                let caps = xhci_caps(bar64(f));
                out!(
                    "R debug.xhci at={at} id={id} mmio={:#x} dbc={} ext_caps={:?}",
                    bar64(f),
                    u8::from(caps.contains(&10)),
                    caps
                );
            }
            0x0C0320 => {
                let caps = pci_caps(f);
                out!(
                    "R debug.ehci at={at} id={id} debug_port={}",
                    u8::from(caps.contains(&0x0A))
                );
            }
            _ => {}
        }
    }
    for (id, base) in acpi::ioapics() {
        let v = ioapic_version(base);
        out!(
            "R ioapic id={id} address={base:#x} version={:#x} entries={} eoi_register={}",
            v & 0xFF,
            (v >> 16 & 0xFF) + 1,
            u8::from(v & 0xFF >= 0x20)
        );
    }
    let lv = apic::read(apic::VERSION);
    out!(
        "R lapic version={:#x} max_lvt={} eoi_broadcast_suppression={}",
        lv & 0xFF,
        (lv >> 16 & 0xFF) + 1,
        u8::from(lv >> 24 & 1 == 1)
    );
    Ok(())
}
