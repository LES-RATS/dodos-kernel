//! The multicore trial as a library: the simulated kernels (`sim`), their
//! primitives (`sync`), the suites and tables (`bench`). The `std` feature
//! (default) runs it on host threads (`main.rs`); without it the bare-metal
//! image of `trials/desktop` runs the same code on real cores.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bench;
pub mod hist;
pub mod platform;
pub mod rt;
pub mod sim;
pub mod sync;

/// What the tests that start spinning threads need from the host.
#[cfg(test)]
mod host {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static HOST: Mutex<()> = Mutex::new(());

    /// Held by every test that starts spinning threads, so that no other
    /// test's threads compete with a timing test's for cores.
    pub fn alone() -> MutexGuard<'static, ()> {
        HOST.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the host has a hardware thread for each of `n` spinning
    /// workers. A timing test compares latencies of workers that must run at
    /// the same time; with fewer cores it measures the host's scheduler, so
    /// it says so and does not run.
    pub fn cores_for(n: usize, test: &str) -> bool {
        let have = std::thread::available_parallelism().map_or(1, |n| n.get());
        if have < n {
            eprintln!("{test}: not run, it needs {n} hardware threads and the host has {have}");
        }
        have >= n
    }
}
