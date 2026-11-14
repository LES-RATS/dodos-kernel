//! Values that the x86-64 architecture layer fixes (Interrupts, Register
//! calls).

/// The vectors that `irq_claim` accepts; the others are the kernel's.
pub const IRQ_VECTOR_FIRST: u8 = 48;
pub const IRQ_VECTOR_LAST: u8 = 239;
/// Entries of the interrupt table of one node: one per claimable vector.
pub const IRQ_TABLE_ENTRIES: usize = 192;
/// With interrupt remapping, a node of this index or more cannot claim: its
/// handles would not fit in the remapping table.
pub const IRQ_MAX_NODES_REMAPPED: u32 = 341;

/// Words of the register block.
pub const REGS_WORDS: usize = 20;
pub const REG_RIP: usize = 0;
pub const REG_RSP: usize = 1;
pub const REG_RFLAGS: usize = 2;
pub const REG_RAX: usize = 3;
pub const REG_RBX: usize = 4;
pub const REG_RCX: usize = 5;
pub const REG_RDX: usize = 6;
pub const REG_RSI: usize = 7;
pub const REG_RDI: usize = 8;
pub const REG_RBP: usize = 9;
/// r8 is word 10, and r8 + n is word 10 + n, up to r15.
pub const REG_R8: usize = 10;
pub const REG_FS_BASE: usize = 18;
pub const REG_GS_BASE: usize = 19;
/// The words that must be canonical addresses.
pub const REGS_CANONICAL: [usize; 4] = [REG_RIP, REG_RSP, REG_FS_BASE, REG_GS_BASE];
/// The bits of rflags that user code can change: CF, PF, AF, ZF, SF, TF, DF,
/// OF, AC and ID.
pub const RFLAGS_USER_MASK: u64 = 0x0024_0DD5;
/// The value of the other bits: IF and the reserved bit 1.
pub const RFLAGS_FIXED: u64 = 0x202;

/// A canonical address with four levels of tables: bits 47 to 63 all equal.
pub const fn canonical(address: u64) -> bool {
    let top = address >> 47;
    top == 0 || top == (1 << 17) - 1
}

/// rflags as a write gives it, with the bits user code cannot change forced
/// to their fixed values.
pub const fn rflags(written: u64) -> u64 {
    written & RFLAGS_USER_MASK | RFLAGS_FIXED
}
