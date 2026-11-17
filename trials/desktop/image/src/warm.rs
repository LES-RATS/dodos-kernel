//! P-016: does memory survive a warm reset? (kernel-interface.md, open
//! point "Whether memory survives the warm reset"; D-125 point 1.) A known
//! pattern is written to four regions of 1 MB taken from the firmware at
//! spread addresses (above 16 MB, above 1 GB, above 4 GB, and near the top
//! of memory), the regions and a nonce are recorded on the stick, and the
//! machine is reset; the next boot reads the regions back before anything
//! else of this step and counts the 8-byte words still intact. Methods,
//! each `reps` times: the firmware's ResetSystem warm and cold (UEFI
//! runtime service, `EfiResetWarm`, `EfiResetCold`), and the ACPI FADT
//! reset register (what a kernel without firmware services would use).
//! The run sheet asks that the stick stay first in the boot order, so the
//! machine comes back to the image by itself.

use crate::efi::{self, MemoryDescriptor};
use crate::run::Ctx;
use crate::{con, out, reset};
use alloc::string::String;
use alloc::vec::Vec;

const METHODS: [&str; 3] = ["uefi-warm", "acpi", "uefi-cold"];
const REGION: u64 = 1 << 20;

fn word(nonce: u64, addr: u64) -> u64 {
    // splitmix64 of the nonce and the address.
    let mut z = nonce ^ addr.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The firmware's memory map (also used by `vmx`).
pub fn memory_map_pub() -> Vec<MemoryDescriptor> {
    memory_map()
}

fn memory_map() -> Vec<MemoryDescriptor> {
    let st = con::CON
        .lock()
        .expect("console")
        .as_ref()
        .expect("console")
        .st;
    let mut size = 0usize;
    let (mut key, mut dsize, mut ver) = (0usize, 0usize, 0u32);
    // SAFETY: boot services are valid; the first call asks for the size,
    // the second fills a buffer larger than it.
    unsafe {
        let bs = (*st).boot_services;
        ((*bs).get_memory_map)(
            &mut size,
            core::ptr::null_mut(),
            &mut key,
            &mut dsize,
            &mut ver,
        );
        size += 16 * dsize.max(48);
        let mut buf = alloc::vec![0u8; size];
        if ((*bs).get_memory_map)(
            &mut size,
            buf.as_mut_ptr().cast(),
            &mut key,
            &mut dsize,
            &mut ver,
        ) != efi::SUCCESS
            || dsize == 0
        {
            return Vec::new();
        }
        (0..size / dsize)
            .map(|k| {
                buf.as_ptr()
                    .add(k * dsize)
                    .cast::<MemoryDescriptor>()
                    .read_unaligned()
            })
            .collect()
    }
}

/// Four regions of free memory, taken from the firmware.
fn take_regions() -> Vec<u64> {
    let st = con::CON
        .lock()
        .expect("console")
        .as_ref()
        .expect("console")
        .st;
    let free: Vec<(u64, u64)> = memory_map()
        .iter()
        .filter(|d| d.kind == efi::CONVENTIONAL_MEMORY && d.number_of_pages * 4096 >= 2 * REGION)
        .map(|d| {
            (
                d.physical_start,
                d.physical_start + d.number_of_pages * 4096,
            )
        })
        .collect();
    let top = free.iter().map(|r| r.1).max().unwrap_or(0);
    let mut out = Vec::new();
    for want in [16u64 << 20, 1 << 30, 4 << 30, top.saturating_sub(64 << 20)] {
        let Some(&(s, e)) = free
            .iter()
            .find(|r| r.1 >= want + REGION && r.1 - r.0.max(want) >= REGION)
        else {
            continue;
        };
        let mut a = ((s.max(want)) + 0xFFFFF) & !0xFFFFF;
        if a + REGION > e {
            a = s;
        }
        if out.contains(&a) {
            continue;
        }
        let mut got = a;
        // SAFETY: boot services are valid; asks for exactly these pages.
        let r = unsafe {
            ((*(*st).boot_services).allocate_pages)(
                efi::ALLOCATE_ADDRESS,
                efi::LOADER_DATA,
                (REGION / 4096) as usize,
                &mut got,
            )
        };
        if r == efi::SUCCESS {
            out.push(a);
        }
    }
    out
}

fn fill(nonce: u64, regions: &[u64]) {
    for &r in regions {
        for k in 0..REGION / 8 {
            let a = r + k * 8;
            // SAFETY: the region was taken from the firmware for this.
            unsafe { (a as *mut u64).write_volatile(word(nonce, a)) };
        }
    }
}

fn check(nonce: u64, r: u64) -> u64 {
    (0..REGION / 8)
        .filter(|k| {
            let a = r + k * 8;
            // SAFETY: memory the firmware reported; only read.
            unsafe { (a as *const u64).read_volatile() == word(nonce, a) }
        })
        .count() as u64
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

pub fn run(cx: &mut Ctx) -> Result<(), String> {
    let reps = cx.reps(2, 1) as u32;
    let armed: Vec<String> = cx.records("wr ").iter().map(|s| String::from(*s)).collect();
    let done: Vec<String> = cx
        .records("wr-done ")
        .iter()
        .map(|s| String::from(*s))
        .collect();
    // A reset armed in an earlier boot and not yet read back.
    if let Some(last) = armed.last() {
        let (m, r) = (
            field(last, "method").unwrap_or("?"),
            field(last, "rep").unwrap_or("?"),
        );
        let tag = alloc::format!("wr-done method={m} rep={r}");
        if !done.iter().any(|d| d == &tag) {
            let nonce = field(last, "nonce")
                .and_then(crate::run::parse_num)
                .unwrap_or(0);
            let regions: Vec<u64> = field(last, "regions")
                .unwrap_or("")
                .split(',')
                .filter_map(crate::run::parse_num)
                .collect();
            let mut intact_regions = 0;
            let mut words = 0;
            for &a in &regions {
                let ok = check(nonce, a);
                words += ok;
                if ok == REGION / 8 {
                    intact_regions += 1;
                }
                out!(
                    "R warm.region method={m} rep={r} address={a:#x} intact_words={ok} of={}",
                    REGION / 8
                );
            }
            out!(
                "R warm.reset method={m} rep={r} regions_intact={intact_regions} regions={} words_intact_pct={:.2} armed_boot={} read_boot={}",
                regions.len(),
                100.0 * words as f64 / (regions.len().max(1) as u64 * REGION / 8) as f64,
                field(last, "boot").unwrap_or("?"),
                cx.boot
            );
            con::flush();
            cx.record(&tag);
        }
    }
    let done: Vec<String> = cx
        .records("wr-done ")
        .iter()
        .map(|s| String::from(*s))
        .collect();
    for m in METHODS {
        for r in 0..reps {
            let tag = alloc::format!("wr-done method={m} rep={r}");
            if done.iter().any(|d| d == &tag) {
                continue;
            }
            let regions = take_regions();
            if regions.is_empty() {
                return Err("no free memory region could be taken".into());
            }
            let nonce = crate::x86::rdtsc() ^ u64::from(cx.boot) << 48;
            fill(nonce, &regions);
            let list: Vec<String> = regions.iter().map(|a| alloc::format!("{a:#x}")).collect();
            cx.record(&alloc::format!(
                "wr method={m} rep={r} nonce={nonce:#x} regions={} boot={}",
                list.join(","),
                cx.boot
            ));
            cx.record(&alloc::format!("resume warm-reset {}", cx.boot));
            out!(
                "I warm.reset arming method={m} rep={r}: the machine resets now and the image starts again"
            );
            con::flush();
            con::stall_us(1_000_000);
            match m {
                "uefi-warm" => reset::firmware(efi::RESET_WARM),
                "uefi-cold" => reset::firmware(efi::RESET_COLD),
                _ => reset::acpi_reset(),
            }
        }
    }
    Ok(())
}
