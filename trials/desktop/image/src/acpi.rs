//! The ACPI tables the image reads: found from the RSDP the firmware lists
//! in its configuration table (`EFI_ACPI_20_TABLE_GUID`), through the XSDT.
//! Layouts from the EDK2 headers `IndustryStandard/Acpi65.h` (ACPI 6.5):
//! RSDP XSDT address at offset 24; FADT `PM_TMR_BLK` at 76, `Flags` at
//! 112, `RESET_REG` (generic address: space, width, offset, size, address
//! at +4) at 116, `RESET_VALUE` at 128, `RESET_REG_SUP` flag bit 10;
//! `SMI_CMD` at 48, `ACPI_ENABLE`..`PSTATE_CNT` at 52-55, `CST_CNT` at 95;
//! MADT entries from offset 44, I/O APIC entry type 1 (address at +4).

use crate::efi::{self, SystemTable};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

static XSDT: AtomicU64 = AtomicU64::new(0);

pub fn rd8(a: u64) -> u8 {
    // SAFETY: ACPI tables are in memory the firmware reports and maps.
    unsafe { (a as *const u8).read_volatile() }
}
fn rd16(a: u64) -> u16 {
    u16::from(rd8(a)) | u16::from(rd8(a + 1)) << 8
}
pub fn rd32(a: u64) -> u32 {
    u32::from(rd16(a)) | u32::from(rd16(a + 2)) << 16
}
pub fn rd64(a: u64) -> u64 {
    u64::from(rd32(a)) | u64::from(rd32(a + 4)) << 32
}

/// Finds the XSDT.
///
/// # Safety
/// `st` is the firmware's system table.
pub unsafe fn init(st: *mut SystemTable) {
    // SAFETY: the configuration table has `number_of_table_entries` entries.
    unsafe {
        let n = (*st).number_of_table_entries;
        let t = (*st).configuration_table;
        for i in 0..n {
            let e = &*t.add(i);
            if e.vendor_guid == efi::ACPI_20_TABLE_GUID {
                let rsdp = e.vendor_table as u64;
                XSDT.store(rd64(rsdp + 24), Ordering::Relaxed);
            }
        }
    }
}

/// Every table with signature `sig`.
pub fn tables(sig: &[u8; 4]) -> Vec<u64> {
    let x = XSDT.load(Ordering::Relaxed);
    if x == 0 {
        return Vec::new();
    }
    let len = u64::from(rd32(x + 4));
    (0..(len.saturating_sub(36)) / 8)
        .map(|i| rd64(x + 36 + i * 8))
        .filter(|&t| t != 0 && rd32(t) == u32::from_le_bytes(*sig))
        .collect()
}

pub fn table(sig: &[u8; 4]) -> Option<u64> {
    tables(sig).first().copied()
}

pub fn signatures() -> Vec<[u8; 4]> {
    let x = XSDT.load(Ordering::Relaxed);
    if x == 0 {
        return Vec::new();
    }
    let len = u64::from(rd32(x + 4));
    (0..(len.saturating_sub(36)) / 8)
        .map(|i| rd32(rd64(x + 36 + i * 8)).to_le_bytes())
        .collect()
}

pub fn pm_timer_port() -> Option<u16> {
    let f = table(b"FACP")?;
    let port = rd32(f + 76);
    (port != 0 && port < 0x10000).then_some(port as u16)
}

/// The reset register: (address space, address, value), when the FADT says
/// it is supported.
pub fn reset_reg() -> Option<(u8, u64, u8)> {
    let f = table(b"FACP")?;
    if rd32(f + 4) < 129 || rd32(f + 112) & (1 << 10) == 0 {
        return None;
    }
    Some((rd8(f + 116), rd64(f + 120), rd8(f + 128)))
}

/// SMI_CMD and the values the FADT reserves on it.
pub fn smi_cmd() -> Option<(u32, [u8; 5])> {
    let f = table(b"FACP")?;
    let port = rd32(f + 48);
    (port != 0).then(|| {
        (
            port,
            [rd8(f + 52), rd8(f + 53), rd8(f + 54), rd8(f + 55), rd8(f + 95)],
        )
    })
}

/// The I/O APICs of the MADT: (id, address).
pub fn ioapics() -> Vec<(u8, u64)> {
    let Some(m) = table(b"APIC") else {
        return Vec::new();
    };
    let len = u64::from(rd32(m + 4));
    let mut out = Vec::new();
    let mut p = m + 44;
    while p + 2 <= m + len {
        let (kind, l) = (rd8(p), u64::from(rd8(p + 1)));
        if l == 0 {
            break;
        }
        if kind == 1 {
            out.push((rd8(p + 2), u64::from(rd32(p + 4))));
        }
        p += l;
    }
    out
}

/// The MCFG allocations: (ECAM base, segment, first bus, last bus).
pub fn ecam() -> Vec<(u64, u16, u8, u8)> {
    let Some(m) = table(b"MCFG") else {
        return Vec::new();
    };
    let len = u64::from(rd32(m + 4));
    (0..(len.saturating_sub(44)) / 16)
        .map(|i| {
            let e = m + 44 + i * 16;
            (rd64(e), rd16(e + 8), rd8(e + 10), rd8(e + 11))
        })
        .collect()
}

