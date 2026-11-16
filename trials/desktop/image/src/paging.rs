//! The image's page tables (4-level paging, SDM vol. 3A 5.5): the first
//! 512 GB identity mapped with 1 GB pages (2 MB pages when the processor
//! lacks them, CPUID.80000001H:EDX[26]), writable and executable, memory
//! type left to the MTRRs as the firmware does (PAT entry 0); and a test
//! window of 4 KB pages at 512 GB (`WINDOW`), whose entries the measures
//! write. All tables lie below 4 GB so that the 32-bit part of the
//! trampoline can load CR3.

use crate::efi::{self, SystemTable};
use crate::x86;
use core::sync::atomic::{AtomicU64, Ordering};

pub const P: u64 = 1;
pub const RW: u64 = 1 << 1;
pub const A: u64 = 1 << 5;
pub const D: u64 = 1 << 6;
pub const PS: u64 = 1 << 7;
pub const ADDR: u64 = 0x000F_FFFF_FFFF_F000;

/// Base of the test window: 512 entries of 4 KB.
pub const WINDOW: u64 = 512 << 30;

static ROOT: AtomicU64 = AtomicU64::new(0);
static WINDOW_PT: AtomicU64 = AtomicU64::new(0);

pub fn root() -> u64 {
    ROOT.load(Ordering::Relaxed)
}

/// Physical address of the page table that maps the window.
pub fn window_table() -> u64 {
    WINDOW_PT.load(Ordering::Relaxed)
}

fn page_below_4g(st: *mut SystemTable) -> u64 {
    let mut a: u64 = 0xFFFF_F000;
    // SAFETY: boot services are valid; one page below 4 GB, then cleared.
    unsafe {
        let bs = (*st).boot_services;
        let r = ((*bs).allocate_pages)(efi::ALLOCATE_MAX_ADDRESS, efi::LOADER_DATA, 1, &mut a);
        assert!(r == efi::SUCCESS, "no page below 4 GB for the page tables");
        core::ptr::write_bytes(a as *mut u8, 0, 4096);
    }
    a
}

/// A page below 4 GB taken from the firmware, cleared (also used by the
/// trampoline and the measures).
pub fn low_page(st: *mut SystemTable) -> u64 {
    page_below_4g(st)
}

fn entry(table: u64, i: u64) -> *mut u64 {
    (table + i * 8) as *mut u64
}

/// Builds the tables. Bootstrap core, before any core loads them.
pub fn build(st: *mut SystemTable) {
    let gb_pages = x86::bit(x86::cpuid(0x8000_0001, 0).edx, 26);
    let pml4 = page_below_4g(st);
    let pdpt = page_below_4g(st);
    // SAFETY: the new tables are ours, identity mapped, cleared.
    unsafe {
        *entry(pml4, 0) = pdpt | P | RW;
        for g in 0..512u64 {
            if gb_pages {
                *entry(pdpt, g) = g << 30 | P | RW | PS;
            } else if g < 64 {
                let pd = page_below_4g(st);
                for m in 0..512u64 {
                    *entry(pd, m) = g << 30 | m << 21 | P | RW | PS;
                }
                *entry(pdpt, g) = pd | P | RW;
            }
        }
        let wpdpt = page_below_4g(st);
        let wpd = page_below_4g(st);
        let wpt = page_below_4g(st);
        *entry(pml4, 1) = wpdpt | P | RW;
        *entry(wpdpt, 0) = wpd | P | RW;
        *entry(wpd, 0) = wpt | P | RW;
        WINDOW_PT.store(wpt, Ordering::Relaxed);
    }
    ROOT.store(pml4, Ordering::Relaxed);
}

/// The entry that maps window page `i`.
pub fn window_pte(i: u64) -> *mut u64 {
    entry(window_table(), i)
}

pub fn window_va(i: u64) -> u64 {
    WINDOW + i * 4096
}

/// Maps window page `i` to `phys` with `flags` and invalidates it locally.
pub fn map(i: u64, phys: u64, flags: u64) {
    // SAFETY: the window's table is ours; one entry changes.
    unsafe { window_pte(i).write_volatile(phys & ADDR | flags) };
    x86::invlpg(window_va(i));
}

/// A cleared 4 KB frame from the heap (identity mapped: its address is its
/// physical address), never freed.
pub fn frame() -> u64 {
    let l = core::alloc::Layout::from_size_align(4096, 4096).expect("layout");
    // SAFETY: a non-zero layout; the frame is leaked on purpose.
    let p = unsafe { alloc::alloc::alloc_zeroed(l) };
    assert!(!p.is_null(), "out of frames");
    p as u64
}

/// Maps `len` bytes at `phys` with 4 KB pages at the start of PML4 entry
/// `slot` (each entry covers 512 GB) and returns the virtual address. The
/// tables come from the heap; the range must fit one page directory
/// pointer table (512 GB).
pub fn map_4k_range(slot: u64, phys: u64, len: u64) -> u64 {
    let pml4 = root();
    let pdpt = frame();
    // SAFETY: our own tables, identity mapped; the slot was empty.
    unsafe {
        *entry(pml4, slot) = pdpt | P | RW;
        let pages = len.div_ceil(4096);
        for p in 0..pages {
            let (i3, i2, i1) = (p >> 18 & 511, p >> 9 & 511, p & 511);
            if *entry(pdpt, i3) == 0 {
                *entry(pdpt, i3) = frame() | P | RW;
            }
            let pd = *entry(pdpt, i3) & ADDR;
            if *entry(pd, i2) == 0 {
                *entry(pd, i2) = frame() | P | RW;
            }
            let pt = *entry(pd, i2) & ADDR;
            *entry(pt, i1) = (phys + p * 4096) | P | RW;
        }
    }
    slot << 39
}
