//! Values that the ARM64 architecture layer fixes (Unlike cores).

use crate::{ExceptionUse, FaultKind, FeatureList};

/// State features: SVE and SME have a length; SME is large; ZT0 of SME2.
pub const SVE: u32 = 0;
pub const SME: u32 = 1;
pub const SME2_ZT0: u32 = 2;
pub const FEATURES: FeatureList = FeatureList {
    state: 1 << SVE | 1 << SME | 1 << SME2_ZT0,
    with_length: 1 << SVE | 1 << SME,
    large: 1 << SME,
};

// The control words of a user thread's hardware breakpoints and
// watchpoints (Debugging a component, D-142): `DBGBCR<n>_EL1` and
// `DBGWCR<n>_EL1`, their fields read in Arm DDI 0487 M.d, D24.3.2 and
// D24.3.11. A thread's word generates exceptions at EL0 only: HMC 0, SSCE
// 0, SSC 0b00 and PMC or PAC 0b10 (D2.8.4, Table D2-8; D2.9.4, Table
// D2-13); an unlinked address match, with no linking and no context
// matching (BT 0b0000 and BT2 0; WT 0 and WT2 0); the linked numbers LBN
// and LBNX 0.

/// `DBGBCR<n>_EL1`: E (bit 0), PMC (bits 2:1), BAS (bits 8:5).
pub const DBGBCR_E: u64 = 1;
pub const DBGBCR_PMC_SHIFT: u32 = 1;
pub const DBGBCR_BAS_SHIFT: u32 = 5;
/// `DBGWCR<n>_EL1`: E (bit 0), PAC (bits 2:1), LSC (bits 4:3), BAS (bits
/// 12:5), MASK (bits 28:24).
pub const DBGWCR_E: u64 = 1;
pub const DBGWCR_PAC_SHIFT: u32 = 1;
pub const DBGWCR_LSC_SHIFT: u32 = 3;
pub const DBGWCR_BAS_SHIFT: u32 = 5;
pub const DBGWCR_MASK_SHIFT: u32 = 24;
/// PMC and PAC 0b10: EL0 only, with HMC 0 and SSC 0b00.
pub const DEBUG_EL0: u64 = 0b10;
/// The bits a user thread's word may set: E, PMC and BAS of a breakpoint;
/// E, PAC, LSC, BAS and MASK of a watchpoint. HMC (13), SSC (15:14), LBN
/// (19:16), BT (23:20) or WT (20) and WT2 (22), SSCE (29), LBNX (31:30),
/// a breakpoint's BT2 (3) and MASK (28:24), and bits 63:32 stay 0.
pub const DBGBCR_USER_MASK: u64 = DBGBCR_E | 0b11 << DBGBCR_PMC_SHIFT | 0b1111 << DBGBCR_BAS_SHIFT;
pub const DBGWCR_USER_MASK: u64 = DBGWCR_E
    | 0b11 << DBGWCR_PAC_SHIFT
    | 0b11 << DBGWCR_LSC_SHIFT
    | 0xFF << DBGWCR_BAS_SHIFT
    | 0b1_1111 << DBGWCR_MASK_SHIFT;

/// Whether a breakpoint control word is one a user thread may set: 0 (no
/// breakpoint), or within the user mask with PMC 0b10 and BAS 0b1111, the
/// value for A64 instructions (D24.3.2: "Use for A64 and A32
/// instructions"; the other values are for T32, which a thread of this
/// kernel does not run).
pub const fn dbgbcr_valid(word: u64) -> bool {
    word == 0
        || (word & !DBGBCR_USER_MASK == 0
            && word >> DBGBCR_PMC_SHIFT & 0b11 == DEBUG_EL0
            && word >> DBGBCR_BAS_SHIFT & 0b1111 == 0b1111)
}

/// Whether a watchpoint control word is one a user thread may set: 0 (no
/// watchpoint), or within the user mask with PAC 0b10, LSC not 0b00 (a
/// reserved value), BAS nonzero with its set bits contiguous ("All other
/// values are reserved and must not be used by software", D24.3.11), and
/// MASK 0 or 3 to 31 ("All other values are reserved").
pub const fn dbgwcr_valid(word: u64) -> bool {
    if word == 0 {
        return true;
    }
    let bas = word >> DBGWCR_BAS_SHIFT & 0xFF;
    let mask = word >> DBGWCR_MASK_SHIFT & 0b1_1111;
    // Contiguous: shifted down to its lowest set bit, one more is a power
    // of two.
    let low = if bas == 0 {
        0
    } else {
        bas >> bas.trailing_zeros()
    };
    word & !DBGWCR_USER_MASK == 0
        && word >> DBGWCR_PAC_SHIFT & 0b11 == DEBUG_EL0
        && word >> DBGWCR_LSC_SHIFT & 0b11 != 0
        && bas != 0
        && (low + 1).is_power_of_two()
        && (mask == 0 || mask >= 3)
}

// The exception classes of `ESR_ELx.EC`, bits 31:26 of the syndrome
// register, and what each is when taken from a user thread (Faults).
// Rebuilt from Arm DDI 0487 K.a (March 2024): the field EC of ESR_EL1,
// ESR_EL2 and ESR_EL3, read in Arm's register description of the same
// release (2024-03), which the register chapter of DDI 0487 K.a is made
// from. The values are compared with Linux's `arch/arm64/include/asm/esr.h`
// in the tests (`abi/src/tests.rs`).

/// Where EC lies in `ESR_ELx`.
pub const ESR_EC_SHIFT: u32 = 26;
pub const ESR_EC_MASK: u64 = 0x3F << ESR_EC_SHIFT;

/// Unknown reason (an instruction that is undefined at the level, among
/// others).
pub const EC_UNKNOWN: u8 = 0x00;
/// A trapped WFI, WFE, WFIT or WFET.
pub const EC_WFX: u8 = 0x01;
/// AArch32: MCR or MRC to coprocessor 15; MCRR or MRRC to 15; MCR or MRC to
/// 14; LDC or STC.
pub const EC_CP15_MCR_MRC: u8 = 0x03;
pub const EC_CP15_MCRR_MRRC: u8 = 0x04;
pub const EC_CP14_MCR_MRC: u8 = 0x05;
pub const EC_CP14_LDC_STC: u8 = 0x06;
/// SME, SVE, Advanced SIMD or floating point trapped by `CPACR_EL1.FPEN`
/// (or `CPTR_EL2`, `CPTR_EL3`).
pub const EC_FP_SIMD: u8 = 0x07;
/// EL2 only: a trapped VMRS of an ID group (AArch32).
pub const EC_CP10_VMRS: u8 = 0x08;
/// EL2 and above: a pointer authentication instruction trapped by
/// `HCR_EL2.API` or `SCR_EL3.API`.
pub const EC_PAC_TRAP: u8 = 0x09;
/// A trapped LD64B or ST64B* (FEAT_LS64).
pub const EC_LD64B: u8 = 0x0A;
/// AArch32: MRRC to coprocessor 14.
pub const EC_CP14_MRRC: u8 = 0x0C;
/// Branch Target Exception (FEAT_BTI).
pub const EC_BTI: u8 = 0x0D;
/// Illegal Execution state.
pub const EC_ILLEGAL_STATE: u8 = 0x0E;
/// AArch32: SVC; EL2 and above: HVC, SMC.
pub const EC_SVC32: u8 = 0x11;
pub const EC_HVC32: u8 = 0x12;
pub const EC_SMC32: u8 = 0x13;
/// A trapped MSRR, MRRS or 128-bit system instruction (FEAT_SYSREG128,
/// FEAT_SYSINSTR128).
pub const EC_SYS128: u8 = 0x14;
/// SVC in AArch64: the system call. EL2 and above: HVC, SMC.
pub const EC_SVC64: u8 = 0x15;
pub const EC_HVC64: u8 = 0x16;
pub const EC_SMC64: u8 = 0x17;
/// A trapped MSR, MRS or system instruction in AArch64.
pub const EC_SYS64: u8 = 0x18;
/// SVE trapped by `CPACR_EL1.ZEN` (or `CPTR_EL2`, `CPTR_EL3`).
pub const EC_SVE: u8 = 0x19;
/// EL2 only: a trapped ERET, ERETAA or ERETAB.
pub const EC_ERET: u8 = 0x1A;
/// A TSTART that its level does not allow (FEAT_TME).
pub const EC_TSTART: u8 = 0x1B;
/// A pointer authentication failure (FEAT_FPAC).
pub const EC_PAC_FAIL: u8 = 0x1C;
/// SME trapped by `CPACR_EL1.SMEN` (or `CPTR_EL2`, `CPTR_EL3`), or an
/// instruction illegal for `PSTATE.SM` or `PSTATE.ZA`.
pub const EC_SME: u8 = 0x1D;
/// EL3 only: a granule protection check (FEAT_RME); an implementation
/// defined exception to EL3.
pub const EC_GPC: u8 = 0x1E;
pub const EC_IMPDEF_EL3: u8 = 0x1F;
/// Instruction abort from a lower level, and at the same level.
pub const EC_IABT_LOWER: u8 = 0x20;
pub const EC_IABT_SAME: u8 = 0x21;
/// PC alignment fault.
pub const EC_PC_ALIGN: u8 = 0x22;
/// Data abort from a lower level, and at the same level.
pub const EC_DABT_LOWER: u8 = 0x24;
pub const EC_DABT_SAME: u8 = 0x25;
/// SP alignment fault.
pub const EC_SP_ALIGN: u8 = 0x26;
/// Memory operation exception (FEAT_MOPS).
pub const EC_MOPS: u8 = 0x27;
/// A trapped floating-point exception from AArch32, and from AArch64.
pub const EC_FP_TRAP32: u8 = 0x28;
pub const EC_FP_TRAP64: u8 = 0x2C;
/// Guarded control stack exception (FEAT_GCS).
pub const EC_GCS: u8 = 0x2D;
/// SError exception.
pub const EC_SERROR: u8 = 0x2F;
/// Breakpoint, software step and watchpoint exceptions, from a lower level
/// and at the same level.
pub const EC_BREAKPOINT_LOWER: u8 = 0x30;
pub const EC_BREAKPOINT_SAME: u8 = 0x31;
pub const EC_STEP_LOWER: u8 = 0x32;
pub const EC_STEP_SAME: u8 = 0x33;
pub const EC_WATCHPOINT_LOWER: u8 = 0x34;
pub const EC_WATCHPOINT_SAME: u8 = 0x35;
/// AArch32: BKPT; EL2 only: vector catch.
pub const EC_BKPT32: u8 = 0x38;
pub const EC_VECTOR_CATCH32: u8 = 0x3A;
/// BRK in AArch64.
pub const EC_BRK64: u8 = 0x3C;
/// PMU exception (FEAT_EBEP).
pub const EC_PMU: u8 = 0x3D;

/// Every exception class the architecture defines; every other value is
/// reserved.
pub const EXCEPTION_CLASSES: [u8; 49] = [
    EC_UNKNOWN,
    EC_WFX,
    EC_CP15_MCR_MRC,
    EC_CP15_MCRR_MRRC,
    EC_CP14_MCR_MRC,
    EC_CP14_LDC_STC,
    EC_FP_SIMD,
    EC_CP10_VMRS,
    EC_PAC_TRAP,
    EC_LD64B,
    EC_CP14_MRRC,
    EC_BTI,
    EC_ILLEGAL_STATE,
    EC_SVC32,
    EC_HVC32,
    EC_SMC32,
    EC_SYS128,
    EC_SVC64,
    EC_HVC64,
    EC_SMC64,
    EC_SYS64,
    EC_SVE,
    EC_ERET,
    EC_TSTART,
    EC_PAC_FAIL,
    EC_SME,
    EC_GPC,
    EC_IMPDEF_EL3,
    EC_IABT_LOWER,
    EC_IABT_SAME,
    EC_PC_ALIGN,
    EC_DABT_LOWER,
    EC_DABT_SAME,
    EC_SP_ALIGN,
    EC_MOPS,
    EC_FP_TRAP32,
    EC_FP_TRAP64,
    EC_GCS,
    EC_SERROR,
    EC_BREAKPOINT_LOWER,
    EC_BREAKPOINT_SAME,
    EC_STEP_LOWER,
    EC_STEP_SAME,
    EC_WATCHPOINT_LOWER,
    EC_WATCHPOINT_SAME,
    EC_BKPT32,
    EC_VECTOR_CATCH32,
    EC_BRK64,
    EC_PMU,
];

/// What an exception taken from a user thread is (Faults), from its
/// syndrome (`ESR_ELx`). A user thread runs in AArch64 at EL0, so every
/// AArch32 class, every class taken at the kernel's own level and every
/// class of a level above EL0's reach is not an exception of the thread. An
/// instruction or data abort is classified by its fault status code
/// (`abort_use`). `None` for a reserved class or fault status code.
pub const fn exception_use(syndrome: u64) -> Option<ExceptionUse> {
    let ec = ((syndrome & ESR_EC_MASK) >> ESR_EC_SHIFT) as u8;
    if ec == EC_IABT_LOWER || ec == EC_DABT_LOWER {
        return abort_use((syndrome & 0x3F) as u8);
    }
    if ec == EC_SME {
        return sme_use((syndrome & 0x7) as u8);
    }
    class_use(ec)
}

/// An SME exception, by its trap code (`ESR_ELx` bits 2:0, Arm DDI 0487,
/// D24.2, read in M.d): 0b000 SME trapped, the feature SME; 0b100 the
/// register ZT0 trapped, the feature `SME2_ZT0`; 0b001, 0b010 and 0b011 an
/// instruction refused by the thread's streaming or ZA state, an undefined
/// instruction whatever the mask (the final review's verification of lot A,
/// H1: every code read as SME, so a ZT0 trap looped). `None` for a reserved
/// code.
const fn sme_use(code: u8) -> Option<ExceptionUse> {
    use crate::ExceptionUse::{Fault, Feature};
    match code {
        0b000 => Some(Feature(SME)),
        0b100 => Some(Feature(SME2_ZT0)),
        0b001..=0b011 => Some(Fault(FaultKind::Undefined)),
        _ => None,
    }
}

/// What an exception of class `ec` other than an EL0 abort or SME trap is.
const fn class_use(ec: u8) -> Option<ExceptionUse> {
    use crate::ExceptionUse::{Call, Fault, Feature, NotThread, Own};
    use FaultKind::{Arithmetic, Debug, HardwareBreakpoint, Protection, Undefined};
    Some(match ec {
        EC_UNKNOWN | EC_WFX | EC_LD64B | EC_SYS128 | EC_SYS64 | EC_TSTART | EC_PAC_TRAP => {
            Fault(Undefined)
        }
        // Advanced SIMD and floating point are the baseline of every
        // thread: a trap of them is the kernel's lazy state.
        EC_FP_SIMD => Own,
        EC_SVE => Feature(SVE),
        EC_BTI | EC_ILLEGAL_STATE | EC_PC_ALIGN | EC_SP_ALIGN | EC_PAC_FAIL | EC_GCS => {
            Fault(Protection)
        }
        // The kernel restarts the copy or set sequence (its registers say
        // where it stands), as for a thread moved between unlike cores.
        EC_MOPS => Own,
        EC_SVC64 => Call,
        EC_FP_TRAP64 => Fault(Arithmetic),
        EC_BREAKPOINT_LOWER | EC_WATCHPOINT_LOWER => Fault(HardwareBreakpoint),
        EC_STEP_LOWER | EC_BRK64 => Fault(Debug),
        EC_CP15_MCR_MRC | EC_CP15_MCRR_MRRC | EC_CP14_MCR_MRC | EC_CP14_LDC_STC | EC_CP10_VMRS
        | EC_CP14_MRRC | EC_SVC32 | EC_HVC32 | EC_SMC32 | EC_HVC64 | EC_SMC64 | EC_ERET
        | EC_GPC | EC_IMPDEF_EL3 | EC_IABT_SAME | EC_DABT_SAME | EC_FP_TRAP32 | EC_SERROR
        | EC_BREAKPOINT_SAME | EC_STEP_SAME | EC_WATCHPOINT_SAME | EC_BKPT32
        | EC_VECTOR_CATCH32 | EC_PMU => NotThread,
        _ => return None,
    })
}

/// What an instruction or data abort from a user thread is, by the fault
/// status code (IFSC or DFSC, bits 5:0 of the syndrome; Arm DDI 0487 K.a,
/// ESR_EL1). A translation fault, at any level (−2 and −1 with FEAT_D128
/// and FEAT_LPA2), and a permission fault are page faults (a permission
/// fault has the address translated: word 3 bit 3, *present*). An
/// alignment fault, a synchronous external abort not on a walk, a tag check
/// fault and the implementation defined fault of an unsupported exclusive or
/// atomic access are protection faults (the rule of Faults: an access fault
/// that is not a translation fault). An access flag fault and an address
/// size fault cannot come from tables the gate wrote (G3 (4) presets the
/// accessed bit and bounds the output address): a bug of the kernel. An
/// abort on a walk, a parity or ECC error, a granule protection fault, a TLB
/// conflict and an unsupported atomic update are not the thread's. `None`
/// for a reserved value.
pub const fn abort_use(fsc: u8) -> Option<ExceptionUse> {
    use crate::ExceptionUse::{Fault, NotThread};
    use FaultKind::{Page, Protection};
    Some(match fsc {
        0x04..=0x07 | 0x2A | 0x2B | 0x0C..=0x0F => Fault(Page),
        0x10 | 0x11 | 0x21 | 0x35 => Fault(Protection),
        0x00..=0x03 | 0x29 | 0x2C | 0x08..=0x0B => NotThread,
        0x12..=0x18 | 0x1B..=0x1F | 0x22..=0x28 | 0x30 | 0x31 | 0x34 => NotThread,
        _ => return None,
    })
}

// Interrupts (GICv3 and GICv3.1, Arm IHI 0069): the identifiers that are
// never claimable and the bases of the claimable ranges, whose ends the
// distributor reports (`GICD_TYPER`).

/// SGIs 0-15 and PPIs 16-31, per core: never claimable.
pub const GIC_PRIVATE_LAST: u32 = 31;
/// The first SPI, and the last that any distributor implements.
pub const GIC_SPI_FIRST: u32 = 32;
pub const GIC_SPI_LAST_MAX: u32 = 1019;
/// Extended PPIs, per core: never claimable.
pub const GIC_EPPI_FIRST: u32 = 1056;
pub const GIC_EPPI_LAST: u32 = 1119;
/// Extended SPIs (when `GICD_TYPER.ESPI` is 1), at most 1,024 of them.
pub const GIC_ESPI_FIRST: u32 = 4096;
pub const GIC_ESPI_LAST_MAX: u32 = 5119;
/// The first LPI.
pub const GIC_LPI_FIRST: u32 = 8192;
/// Entries of the interrupt table of one node, as on x86-64.
pub const IRQ_TABLE_ENTRIES: usize = 192;
