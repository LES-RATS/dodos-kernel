//! Values that the x86-64 architecture layer fixes (Values of the machine,
//! Interrupts, Register calls, Unlike cores, I/O ports).

use crate::{ASPACE_PAGES, FeatureList, Geometry, IOSPACE_PAGES};

/// *P*, the large pages, *E* and *L* with four levels of tables.
pub const PAGE: u64 = 4096;
pub const LARGE: [u64; 2] = [2 << 20, 1 << 30];
pub const TABLE_ENTRIES: u32 = 512;
pub const LEVELS: u32 = 4;
/// The first address beyond the user range: 255 × 512 GB.
pub const USER_RANGE_END: u64 = 0x7F80_0000_0000;
/// The machine's values as the entry `Memory` publishes them, with 1 GB
/// pages.
pub const GEOMETRY: Geometry = Geometry {
    page_shift: [12, 21, 30, 0],
    entries: TABLE_ENTRIES,
    levels: LEVELS,
    user_end: USER_RANGE_END,
};
/// Two pages, a table page, the smallest untyped, a chunk.
pub const ASPACE_BYTES: u64 = ASPACE_PAGES * PAGE;
pub const IOSPACE_BYTES: u64 = IOSPACE_PAGES * PAGE;
pub const TABLE_BYTES: u64 = PAGE;
pub const UNTYPED_MIN_BYTES: u64 = PAGE;
pub const CHUNK_BYTES: u64 = LARGE[0];
/// The start-up page and the message page at *P* and 2 × *P*; no segment of
/// the first service below the first large page.
pub const START_MESSAGE_PAGE_ADDR: u64 = PAGE;
pub const START_PAGE_ADDR: u64 = 2 * PAGE;
pub const FIRST_SERVICE_MIN_ADDR: u64 = LARGE[0];

/// The save area of x87, SSE and AVX state (`X86_XSTATE_AVX_SIZE`): a
/// thread of mask 0 on the reference machine is 1,024 + 832, rounded up to
/// 2,048 bytes.
pub const XSTATE_X87_SSE_AVX_BYTES: u32 = 832;

/// State features (bits 0-30) and the first instruction feature.
pub const AVX: u32 = 0;
pub const AVX512: u32 = 1;
/// Large: always optional, disabled through the extended feature disable
/// register.
pub const AMX: u32 = 2;
pub const APX: u32 = 3;
pub const FEATURES: FeatureList = FeatureList {
    state: 1 << AVX | 1 << AVX512 | 1 << AMX | 1 << APX,
    with_length: 0,
    large: 1 << AMX,
};

/// The vectors that `irq_claim` accepts; the others are the kernel's.
pub const IRQ_VECTOR_FIRST: u8 = 48;
pub const IRQ_VECTOR_LAST: u8 = 239;
/// The ordinary inter-kernel vectors (G10, D-155): the kernel's own
/// interrupts to the cores of its node (reschedule, leave, invalidation)
/// and the interrupt of a ring, one vector each. The gate composes the ICR
/// only for a vector in this range (fixed delivery, physical destination,
/// edge, no shorthand), the target named by core number. Inside 240 to 255:
/// outside the exceptions and outside the claimable vectors.
pub const IKV_FIRST: u8 = 240;
pub const IKV_LAST: u8 = 243;

/// Whether the gate sends `vector` to another core as an ordinary
/// inter-kernel interrupt (G10, D-155).
pub const fn ikv_valid(vector: u8) -> bool {
    IKV_FIRST <= vector && vector <= IKV_LAST
}

/// Whether `vector` may be written for a line without remapping (G10,
/// operation 5, D-155): a claimable vector, 48 to 239.
pub const fn line_vector_valid(vector: u8) -> bool {
    IRQ_VECTOR_FIRST <= vector && vector <= IRQ_VECTOR_LAST
}

/// The PCID the gate keeps for its own translation context, bound to no
/// kernel or user root (G7, D-155): the tag table holds PCIDs 0 to 4,094.
pub const PCID_BITS: u32 = 12;
pub const GATE_PCID: u16 = 4095;

/// Entries of the interrupt table of one physical core: one per claimable
/// vector (D-129).
pub const IRQ_TABLE_ENTRIES: usize = 192;
/// With interrupt remapping, a core of this number or more cannot be
/// claimed on: its handles (core number × 192 + vector − 48) would not fit
/// in the remapping table.
pub const IRQ_MAX_CORES_REMAPPED: u32 = 341;

/// Words of the register block.
pub const REGS_WORDS: usize = 26;
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
/// The breakpoint addresses DR0 to DR3, the breakpoint control DR7, and the
/// word of user interrupts (always 0 today).
pub const REG_DR0: usize = 20;
pub const REG_DR1: usize = 21;
pub const REG_DR2: usize = 22;
pub const REG_DR3: usize = 23;
pub const REG_DR7: usize = 24;
pub const REG_UINTR: usize = 25;
/// Hardware breakpoints of a user thread.
pub const HW_BREAKPOINTS: usize = 4;
/// The bits of DR7 a user thread may set (Intel SDM, volume 3, 18.2.4):
/// the local enables L0-L3 (bits 0, 2, 4, 6) and the condition and length
/// fields R/W0-3 and LEN0-3 (bits 16-31). Not the global enables, not LE
/// and GE, not RTM, not the general-detect bit GD (13).
pub const DR7_USER_MASK: u64 = 0xFFFF_0055;
/// The condition 0b10 of a R/W field is the I/O condition (with CR4.DE).
pub const DR7_CONDITION_IO: u64 = 0b10;
/// DR7.GD, general detect (bit 13, Intel SDM vol. 3B §20.2.4): refused in
/// production and in development builds (Debugging a component, D-155).
pub const DR7_GD: u64 = 1 << 13;

/// Whether a breakpoint control word is one a user thread may set: `GD`
/// clear, no bit outside the user mask, and no condition field naming I/O.
pub const fn dr7_valid(word: u64) -> bool {
    // D-155: general detect is refused by name, whatever the user mask is.
    if word & DR7_GD != 0 {
        return false;
    }
    if word & !DR7_USER_MASK != 0 {
        return false;
    }
    let mut n = 0;
    while n < HW_BREAKPOINTS {
        let condition = word >> (16 + 4 * n) & 0b11;
        let length = word >> (18 + 4 * n) & 0b11;
        // An I/O breakpoint (`CR4.DE` set), a length of 10 (undefined on
        // most processors) and a length on an execution breakpoint are
        // refused (Intel SDM §20.2.4; D-141).
        if condition == DR7_CONDITION_IO || length == 0b10 || (condition == 0 && length != 0) {
            return false;
        }
        n += 1;
    }
    true
}

/// The memory the kernel keeps at start for the tables of one VT-d unit in
/// legacy translation mode: a root table of 4 KB and 256 context tables of
/// 4 KB ("The root-table is 4-KByte in size and contains 256 root-entries",
/// "Each context-table contains 256 entries", VT-d §3.4.2), 1 MB and 4 KB
/// (IOMMU, D-142: "1 MB in all" left out the root table).
pub const VTD_UNIT_TABLE_BYTES: u64 = 4096 + 256 * 4096;

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

/// The address of a remappable MSI for the remapping handle `handle`, whose
/// data is 0 (Interrupts, with interrupt remapping): bit 4 the remappable
/// format, bits 5-19 the handle's bits 0-14, bit 2 its bit 15.
pub const fn msi_remapped(handle: u64) -> u64 {
    0xFEE0_0000 | (handle & 0x7FFF) << 5 | 1 << 4 | (handle >> 15 & 1) << 2
}

/// The address and data of an MSI in physical destination, fixed delivery
/// mode, edge (Interrupts, without interrupt remapping): `None` for an APIC
/// ID above 255, which this format cannot reach.
pub const fn msi_direct(apic_id: u32, vector: u8) -> Option<(u64, u64)> {
    // 255 is the broadcast to every local APIC (Intel SDM §13.6.2.1), so it
    // names no core (D-141).
    if apic_id >= 255 {
        return None;
    }
    Some((0xFEE0_0000 | (apic_id as u64) << 12, vector as u64))
}

/// rflags as a write gives it, with the bits user code cannot change forced
/// to their fixed values.
pub const fn rflags(written: u64) -> u64 {
    written & RFLAGS_USER_MASK | RFLAGS_FIXED
}

/// The ports the kernel keeps for the whole boot: the legacy PIC.
pub const KERNEL_PORTS: [(u64, u64); 2] = [(0x20, 0x21), (0xA0, 0xA1)];
/// The PCI configuration mechanism: allowed only to the holder of the plain
/// role `platform`.
pub const PCI_CONFIG_PORTS: (u64, u64) = (0xCF8, 0xCFF);
/// The ports of a 16550 UART, from its base.
pub const UART_PORTS: u64 = 8;
