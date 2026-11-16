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
