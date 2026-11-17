//! The global allocator: power-of-two size classes over one region taken
//! from the firmware at start, behind a spin lock, so that every core can
//! allocate without calling the firmware (boot services are for the
//! bootstrap core only). Freed blocks go back to their class's list.
//! ponytail: no coalescing; the trial reallocates the same sizes run after
//! run, so the lists are reused. Upgrade to a buddy allocator if a run of
//! mixed sizes exhausts the region.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use dodos_trial_multicore::rt::Mutex;

struct Heap {
    next: usize,
    end: usize,
    free: [usize; 48],
}

pub struct Allocator(Mutex<Heap>);

#[global_allocator]
pub static HEAP: Allocator = Allocator(Mutex::new(Heap {
    next: 0,
    end: 0,
    free: [0; 48],
}));

/// Gives the allocator `len` bytes at `base`.
pub fn init(base: usize, len: usize) {
    let mut h = HEAP.0.lock().expect("heap");
    h.next = base;
    h.end = base + len;
}

fn class(l: Layout) -> u32 {
    l.size()
        .max(l.align())
        .max(16)
        .next_power_of_two()
        .trailing_zeros()
}

// SAFETY: blocks never overlap: a block is either on one free list or cut
// once from the region; its size is the class size, at least the layout's
// size, and it is aligned to that size (a power of two at least the
// layout's alignment).
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let c = class(l) as usize;
        let size = 1usize << c;
        let mut h = self.0.lock().expect("heap");
        if c >= h.free.len() {
            return null_mut();
        }
        let head = h.free[c];
        if head != 0 {
            // SAFETY: a free block holds the address of the next one.
            h.free[c] = unsafe { *(head as *const usize) };
            return head as *mut u8;
        }
        let start = (h.next + size - 1) & !(size - 1);
        if start + size > h.end {
            return null_mut();
        }
        h.next = start + size;
        start as *mut u8
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let c = class(l) as usize;
        let mut h = self.0.lock().expect("heap");
        // SAFETY: the block is ours again and at least 16 bytes long.
        unsafe { *(p as *mut usize) = h.free[c] };
        h.free[c] = p as usize;
    }
}
