//! Values that the RISC-V architecture layer fixes (Unlike cores).

use crate::{ExceptionUse, FaultKind, FeatureList};

/// State features: the vector extension, which has a length.
pub const V: u32 = 0;
pub const FEATURES: FeatureList = FeatureList {
    state: 1 << V,
    with_length: 1 << V,
    large: 0,
};

// The exception codes of `scause` after a trap into supervisor mode, and
// what each is when taken from a user thread (Faults). Rebuilt from the
// RISC-V privileged specification, "Supervisor-Level ISA, Version 1.13",
// table "Supervisor cause (scause) register values after trap"
// (`src/priv/supervisor.adoc` of github.com/riscv/riscv-isa-manual at
// commit 5f1104a, 14 November 2026), and its hypervisor extension for the
// codes taken into HS-mode. The values are compared with Linux's
// `arch/riscv/include/asm/csr.h` in the tests (`abi/src/tests.rs`).

/// The interrupt bit of `scause` (bit 63 on RV64): a trap with it set is an
/// interrupt, not an exception.
pub const SCAUSE_INTERRUPT: u64 = 1 << 63;

pub const CAUSE_INSTRUCTION_MISALIGNED: u64 = 0;
pub const CAUSE_INSTRUCTION_ACCESS: u64 = 1;
pub const CAUSE_ILLEGAL_INSTRUCTION: u64 = 2;
/// EBREAK or C.EBREAK; `sepc` holds its address.
pub const CAUSE_BREAKPOINT: u64 = 3;
pub const CAUSE_LOAD_MISALIGNED: u64 = 4;
pub const CAUSE_LOAD_ACCESS: u64 = 5;
pub const CAUSE_STORE_MISALIGNED: u64 = 6;
pub const CAUSE_STORE_ACCESS: u64 = 7;
pub const CAUSE_ECALL_U: u64 = 8;
pub const CAUSE_ECALL_S: u64 = 9;
/// Hypervisor extension: ECALL from VS-mode.
pub const CAUSE_ECALL_VS: u64 = 10;
pub const CAUSE_INSTRUCTION_PAGE: u64 = 12;
pub const CAUSE_LOAD_PAGE: u64 = 13;
pub const CAUSE_STORE_PAGE: u64 = 15;
/// A software check (Zicfilp, Zicfiss): `stval` 2 a landing-pad fault, 3 a
/// shadow-stack fault.
pub const CAUSE_SOFTWARE_CHECK: u64 = 18;
pub const CAUSE_HARDWARE_ERROR: u64 = 19;
/// Hypervisor extension: guest page faults and virtual instruction.
pub const CAUSE_INSTRUCTION_GUEST_PAGE: u64 = 20;
pub const CAUSE_LOAD_GUEST_PAGE: u64 = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: u64 = 22;
pub const CAUSE_STORE_GUEST_PAGE: u64 = 23;

/// Every exception code the specification defines below 24 (the others
/// are reserved, or designated for custom use, which the kernel refuses).
pub const CAUSES: [u64; 20] = [
    CAUSE_INSTRUCTION_MISALIGNED,
    CAUSE_INSTRUCTION_ACCESS,
    CAUSE_ILLEGAL_INSTRUCTION,
    CAUSE_BREAKPOINT,
    CAUSE_LOAD_MISALIGNED,
    CAUSE_LOAD_ACCESS,
    CAUSE_STORE_MISALIGNED,
    CAUSE_STORE_ACCESS,
    CAUSE_ECALL_U,
    CAUSE_ECALL_S,
    CAUSE_ECALL_VS,
    CAUSE_INSTRUCTION_PAGE,
    CAUSE_LOAD_PAGE,
    CAUSE_STORE_PAGE,
    CAUSE_SOFTWARE_CHECK,
    CAUSE_HARDWARE_ERROR,
    CAUSE_INSTRUCTION_GUEST_PAGE,
    CAUSE_LOAD_GUEST_PAGE,
    CAUSE_VIRTUAL_INSTRUCTION,
    CAUSE_STORE_GUEST_PAGE,
];

/// What an exception taken into supervisor mode from a user thread is
/// (Faults), from `scause`. A misaligned access and an access fault are
/// protection faults (the rule of Faults); an illegal instruction is an
/// undefined instruction (the kernel first reads the instruction in `stval`:
/// a vector instruction while `sstatus.VS` is off is the access to the
/// state of feature `V`, `ExceptionUse::Feature(V)`); EBREAK is a debug
/// trap; ECALL from U-mode is the system call; a software check is a
/// protection fault. ECALL from S-mode, a hardware error and the codes of
/// the hypervisor extension (which the kernel does not use) are not the
/// thread's. `None` for an interrupt or a reserved or custom code.
pub const fn exception_use(scause: u64) -> Option<ExceptionUse> {
    use crate::ExceptionUse::{Call, Fault, NotThread};
    use FaultKind::{Debug, Page, Protection, Undefined};
    if scause & SCAUSE_INTERRUPT != 0 {
        return None;
    }
    Some(match scause {
        CAUSE_INSTRUCTION_MISALIGNED
        | CAUSE_INSTRUCTION_ACCESS
        | CAUSE_LOAD_MISALIGNED
        | CAUSE_LOAD_ACCESS
        | CAUSE_STORE_MISALIGNED
        | CAUSE_STORE_ACCESS
        | CAUSE_SOFTWARE_CHECK => Fault(Protection),
        CAUSE_ILLEGAL_INSTRUCTION => Fault(Undefined),
        CAUSE_BREAKPOINT => Fault(Debug),
        CAUSE_ECALL_U => Call,
        CAUSE_INSTRUCTION_PAGE | CAUSE_LOAD_PAGE | CAUSE_STORE_PAGE => Fault(Page),
        CAUSE_ECALL_S
        | CAUSE_ECALL_VS
        | CAUSE_HARDWARE_ERROR
        | CAUSE_INSTRUCTION_GUEST_PAGE
        | CAUSE_LOAD_GUEST_PAGE
        | CAUSE_VIRTUAL_INSTRUCTION
        | CAUSE_STORE_GUEST_PAGE => NotThread,
        _ => return None,
    })
}

// Interrupts (the AIA): the identities of a supervisor-level IMSIC
// interrupt file, 1 to the number it implements, at most 2,047; identity 0
// is never valid.
pub const IMSIC_IDENTITY_FIRST: u32 = 1;
pub const IMSIC_IDENTITY_MAX: u32 = 2047;
/// Entries of the interrupt table of one node, as on x86-64.
pub const IRQ_TABLE_ENTRIES: usize = 192;
