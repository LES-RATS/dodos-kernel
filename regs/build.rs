//! Prints the register definitions of ARM64 and RISC-V, for Rust (included
//! by `lib.rs`) and for Zig (D-126), into `OUT_DIR`, after the classifier's
//! check: the build fails on an unclassified encoding.

use std::path::Path;

use dodos_classifier::{Arch, build, regs};

fn main() {
    let out = std::env::var_os("OUT_DIR").expect("OUT_DIR");
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        println!("cargo::rerun-if-env-changed={}", arch.env());
        let (spec, classes, _) = build(arch).unwrap_or_else(|e| panic!("{e}"));
        for (ext, text) in [
            ("rs", regs::rust(&spec, &classes)),
            ("zig", regs::zig(&spec, &classes)),
        ] {
            let path = Path::new(&out).join(format!("{}.{ext}", arch.name()));
            std::fs::write(&path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
    println!("cargo::rerun-if-changed=../classifier/classes");
}
