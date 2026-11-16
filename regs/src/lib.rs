//! The system registers the kernel may touch on ARM64 and RISC-V, generated
//! by the build from the pinned machine-readable specifications and the
//! classifications of `classifier/classes` (D-125). A register of class
//! `gate` is marked `gate_only`; registers of the other refused classes are
//! not defined. x86-64 has no open register data: its few registers are
//! written by hand where they are used.
//!
//! The same constants are printed for the Zig layer as `aarch64.zig` and
//! `riscv64.zig` beside these files in `OUT_DIR` (D-126).

#![no_std]
#![forbid(unsafe_code)]

pub mod aarch64 {
    include!(concat!(env!("OUT_DIR"), "/aarch64.rs"));
}

pub mod riscv64 {
    include!(concat!(env!("OUT_DIR"), "/riscv64.rs"));
}

#[cfg(test)]
mod tests {
    use crate::{aarch64, riscv64};

    // Linux v7.3, an independent source of encodings: arch/arm64/tools/sysreg
    // `Sysreg TTBR0_EL1 3 0 2 0 0`, and arch/arm64/include/asm/sysreg.h
    // `SYS_TPIDR_EL0 sys_reg(3, 3, 13, 0, 2)`.
    #[test]
    fn arm64_encodings_match_linux() {
        const T: aarch64::SysReg = aarch64::TTBR0_EL1;
        let t = T;
        assert_eq!((t.op0, t.op1, t.crn, t.crm, t.op2), (3, 0, 2, 0, 0));
        const { assert!(T.gate_only) };
        const P: aarch64::SysReg = aarch64::TPIDR_EL0;
        let p = P;
        assert_eq!((p.op0, p.op1, p.crn, p.crm, p.op2), (3, 3, 13, 0, 2));
        const { assert!(!P.gate_only) };
        assert_eq!(aarch64::ttbr0_el1::ASID.lsb, 48);
    }

    // The ratified privileged specification, src/priv/csrs.adoc: satp is
    // 0x180, stopei (added by hand, not in the database) 0x15C.
    #[test]
    fn riscv_csrs_match_the_ratified_table() {
        assert_eq!(riscv64::SATP.address, 0x180);
        const { assert!(riscv64::SATP.gate_only) };
        assert_eq!(riscv64::STOPEI.address, 0x15C);
        const { assert!(!riscv64::STOPEI.gate_only) };
    }
}
