//! Instructions and registers. Every MSR number and bit is from the Intel
//! SDM, order 325462-093 (September 2026), volume 4 table 2-2 unless noted;
//! control-register bits from volume 3A section 2.5.

use core::arch::asm;

pub const IA32_TSC_ADJUST: u32 = 0x3B;
pub const IA32_FEATURE_CONTROL: u32 = 0x3A;
pub const IA32_APIC_BASE: u32 = 0x1B;
/// Bits of IA32_APIC_BASE (vol. 4 table 2-2; vol. 3A 13.4.4): BSP flag
/// (R/W), x2APIC enable, APIC global enable.
pub const APIC_BASE_BSP: u64 = 1 << 8;
pub const APIC_BASE_EXTD: u64 = 1 << 10;
pub const APIC_BASE_EN: u64 = 1 << 11;
pub const IA32_MPERF: u32 = 0xE7;
pub const IA32_APERF: u32 = 0xE8;
pub const IA32_PAT: u32 = 0x277;
pub const IA32_FIXED_CTR1: u32 = 0x30A;
pub const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
pub const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
pub const IA32_U_CET: u32 = 0x6A0;
pub const IA32_S_CET: u32 = 0x6A2;
pub const IA32_PL0_SSP: u32 = 0x6A4;
pub const IA32_PKRS: u32 = 0x6E1;
pub const IA32_TSC_DEADLINE: u32 = 0x6E0;
pub const IA32_HWP_CAPABILITIES: u32 = 0x771;
pub const IA32_EFER: u32 = 0xC000_0080;
pub const IA32_GS_BASE: u32 = 0xC000_0101;
/// Model-specific (vol. 4 tables of the Core families; their chain down to
/// the 13th generation was not followed to the end: read with recovery).
pub const MSR_SMI_COUNT: u32 = 0x34;
pub const MSR_PKG_CST_CONFIG_CONTROL: u32 = 0xE2;
/// Bits of IA32_S_CET (vol. 4 table 2-2, IA32_U_CET format).
pub const CET_SH_STK_EN: u64 = 1 << 0;
pub const CET_WR_SHSTK_EN: u64 = 1 << 1;
pub const CET_ENDBR_EN: u64 = 1 << 2;

pub const CR0_WP: u64 = 1 << 16;
pub const CR4_PCIDE: u64 = 1 << 17;
pub const CR4_CET: u64 = 1 << 23;
pub const CR4_PKS: u64 = 1 << 24;
pub const CR4_VMXE: u64 = 1 << 13;

#[derive(Clone, Copy, Default, Debug)]
pub struct Cpuid {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

pub fn cpuid(leaf: u32, sub: u32) -> Cpuid {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    Cpuid {
        eax: r.eax,
        ebx: r.ebx,
        ecx: r.ecx,
        edx: r.edx,
    }
}

pub fn bit(v: u32, b: u32) -> bool {
    v >> b & 1 == 1
}

#[inline(always)]
pub fn rdtsc() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: reads the time-stamp counter.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    u64::from(hi) << 32 | u64::from(lo)
}

/// The TSC read after every earlier instruction has completed locally
/// (LFENCE then RDTSC, vol. 2B RDTSC: "If software requires RDTSC to be
/// executed only after all previous instructions have executed ... it can
/// execute LFENCE immediately before RDTSC").
#[inline(always)]
pub fn tsc_begin() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: fences and reads the time-stamp counter.
    unsafe { asm!("lfence", "rdtsc", out("eax") lo, out("edx") hi, options(nostack)) };
    u64::from(hi) << 32 | u64::from(lo)
}

/// RDTSCP then LFENCE: the read waits for earlier instructions and later
/// ones wait for it.
#[inline(always)]
pub fn tsc_end() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: reads the time-stamp counter.
    unsafe {
        asm!("rdtscp", "lfence", out("eax") lo, out("edx") hi, out("ecx") _, options(nostack))
    };
    u64::from(hi) << 32 | u64::from(lo)
}

/// # Safety
/// The MSR must exist; a fault is fatal (use `safe_rdmsr` otherwise).
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: the caller guarantees the MSR exists.
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nostack)) };
    u64::from(hi) << 32 | u64::from(lo)
}

/// # Safety
/// The write must be valid and keep the machine in a state the image
/// expects.
pub unsafe fn wrmsr(msr: u32, v: u64) {
    // SAFETY: the caller's guarantee.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32, options(nostack))
    };
}

/// A fault caught by the recovery of the calling core: vector and error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub vector: u8,
    pub error: u64,
}

/// RDMSR that returns the fault (#GP for an MSR the core lacks) instead of
/// stopping. Needs the image's IDT on this core (`idt::install`).
pub fn safe_rdmsr(msr: u32) -> Result<u64, Fault> {
    let (lo, hi): (u32, u32);
    let f: u64;
    // SAFETY: the recovery path of `idt` resumes at label 2 with the fault
    // recorded in the per-core block (`percpu`), which is cleared first.
    unsafe {
        asm!(
            "mov qword ptr gs:[{fv}], 0",
            "lea rax, [rip + 2f]",
            "mov gs:[{fr}], rax",
            "xor eax, eax",
            "xor edx, edx",
            "rdmsr",
            "2:",
            "mov qword ptr gs:[{fr}], 0",
            "mov {f}, gs:[{fv}]",
            fr = const crate::percpu::FIXUP_RIP,
            fv = const crate::percpu::FIXUP_VECTOR,
            f = out(reg) f,
            in("ecx") msr, out("eax") lo, out("edx") hi,
            options(nostack)
        )
    };
    fault_of(f).map_or(Ok(u64::from(hi) << 32 | u64::from(lo)), Err)
}

/// WRMSR that returns the fault instead of stopping.
///
/// # Safety
/// As `wrmsr`, when the write succeeds.
pub unsafe fn safe_wrmsr(msr: u32, v: u64) -> Result<(), Fault> {
    let f: u64;
    // SAFETY: as `safe_rdmsr`; the caller vouches for the write's effect.
    // The recovery address goes through a scratch register: EAX and EDX
    // already hold the value.
    unsafe {
        asm!(
            "mov qword ptr gs:[{fv}], 0",
            "lea {t}, [rip + 2f]",
            "mov gs:[{fr}], {t}",
            "wrmsr",
            "2:",
            "mov qword ptr gs:[{fr}], 0",
            "mov {f}, gs:[{fv}]",
            fr = const crate::percpu::FIXUP_RIP,
            fv = const crate::percpu::FIXUP_VECTOR,
            t = out(reg) _,
            f = out(reg) f,
            in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
            options(nostack)
        )
    };
    fault_of(f).map_or(Ok(()), Err)
}

/// Decodes the fault word of the per-core block: bit 63 set when a fault
/// was taken, vector in bits 0-7, error code in bits 8-39.
pub fn fault_of(f: u64) -> Option<Fault> {
    (f >> 63 == 1).then_some(Fault {
        vector: f as u8,
        error: f >> 8 & 0xFFFF_FFFF,
    })
}

macro_rules! cr {
    ($read:ident, $write:ident, $reg:literal) => {
        pub fn $read() -> u64 {
            let v: u64;
            // SAFETY: reading a control register has no side effect.
            unsafe { asm!(concat!("mov {}, ", $reg), out(reg) v, options(nomem, nostack)) };
            v
        }
        /// # Safety
        /// The value must keep the machine in a state the image expects.
        pub unsafe fn $write(v: u64) {
            // SAFETY: the caller's guarantee.
            unsafe { asm!(concat!("mov ", $reg, ", {}"), in(reg) v, options(nostack)) };
        }
    };
}
cr!(cr0, set_cr0, "cr0");
cr!(cr3, set_cr3, "cr3");
cr!(cr4, set_cr4, "cr4");

pub fn cr2() -> u64 {
    let v: u64;
    // SAFETY: reads CR2.
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack)) };
    v
}

pub fn rflags() -> u64 {
    let v: u64;
    // SAFETY: reads the flags.
    unsafe { asm!("pushfq", "pop {}", out(reg) v) };
    v
}

pub fn interrupts_enabled() -> bool {
    rflags() & (1 << 9) != 0
}

pub fn cli() {
    // SAFETY: masks maskable interrupts on this core.
    unsafe { asm!("cli", options(nomem, nostack)) };
}

pub fn sti() {
    // SAFETY: unmasks maskable interrupts; the image's IDT handles ours.
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Runs `f` with maskable interrupts masked, restoring the flag after.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let was = interrupts_enabled();
    cli();
    let r = f();
    if was {
        sti();
    }
    r
}

pub fn invlpg(va: u64) {
    // SAFETY: invalidates one TLB entry of this core.
    unsafe { asm!("invlpg [{}]", in(reg) va, options(nostack)) };
}

pub fn pause() {
    core::hint::spin_loop();
}

/// # Safety
/// Port I/O reaches devices: the caller knows the port.
pub unsafe fn outb(port: u16, v: u8) {
    // SAFETY: the caller's guarantee.
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) };
}

/// # Safety
/// As `outb`.
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: the caller's guarantee.
    unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack)) };
    v
}

/// # Safety
/// As `outb`.
pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    // SAFETY: the caller's guarantee.
    unsafe { asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack)) };
    v
}

/// # Safety
/// As `outb`.
pub unsafe fn outl(port: u16, v: u32) {
    // SAFETY: the caller's guarantee.
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack)) };
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct DescriptorPtr {
    pub limit: u16,
    pub base: u64,
}

pub fn sidt() -> DescriptorPtr {
    let mut p = DescriptorPtr::default();
    // SAFETY: stores the IDTR into `p`.
    unsafe { asm!("sidt [{}]", in(reg) &raw mut p, options(nostack)) };
    p
}

pub fn sgdt() -> DescriptorPtr {
    let mut p = DescriptorPtr::default();
    // SAFETY: stores the GDTR into `p`.
    unsafe { asm!("sgdt [{}]", in(reg) &raw mut p, options(nostack)) };
    p
}

/// # Safety
/// `p` must describe a valid IDT for this core.
pub unsafe fn lidt(p: &DescriptorPtr) {
    // SAFETY: the caller's guarantee.
    unsafe { asm!("lidt [{}]", in(reg) p, options(nostack)) };
}

/// # Safety
/// `p` must describe a valid GDT holding the current selectors.
pub unsafe fn lgdt(p: &DescriptorPtr) {
    // SAFETY: the caller's guarantee.
    unsafe { asm!("lgdt [{}]", in(reg) p, options(nostack)) };
}

pub fn cs() -> u16 {
    let v: u16;
    // SAFETY: reads CS.
    unsafe { asm!("mov {0:x}, cs", out(reg) v, options(nomem, nostack)) };
    v
}

/// Halts until an interrupt (or NMI, SMI, INIT) arrives.
pub fn hlt() {
    // SAFETY: waits; resumes after the next event.
    unsafe { asm!("hlt", options(nomem, nostack)) };
}

/// The x2APIC ID of the calling core from CPUID leaf 0BH (vol. 1 table
/// 21-31: EDX holds the x2APIC ID of the current logical processor), or the
/// initial APIC ID of leaf 01H EBX[31:24] when leaf 0BH is absent.
pub fn apic_id_cpuid() -> u32 {
    if cpuid(0, 0).eax >= 0xB && cpuid(0xB, 0).ebx != 0 {
        cpuid(0xB, 0).edx
    } else {
        cpuid(1, 0).ebx >> 24
    }
}

/// MOV to CR4 that returns the fault (#GP for a bit the core refuses)
/// instead of stopping.
///
/// # Safety
/// As `set_cr4`, when the write succeeds.
pub unsafe fn safe_set_cr4(v: u64) -> Result<(), Fault> {
    let f: u64;
    // SAFETY: as `safe_rdmsr`; the caller vouches for the value.
    unsafe {
        asm!(
            "mov qword ptr gs:[{fv}], 0",
            "lea rax, [rip + 2f]",
            "mov gs:[{fr}], rax",
            "mov cr4, {v}",
            "2:",
            "mov qword ptr gs:[{fr}], 0",
            "mov {f}, gs:[{fv}]",
            fr = const crate::percpu::FIXUP_RIP,
            fv = const crate::percpu::FIXUP_VECTOR,
            v = in(reg) v,
            f = out(reg) f,
            out("rax") _,
            options(nostack)
        )
    };
    fault_of(f).map_or(Ok(()), Err)
}
