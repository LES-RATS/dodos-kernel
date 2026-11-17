//! The interrupt descriptor table of each core: the firmware's gates kept
//! (on the bootstrap core the firmware's timer must keep running), the 32
//! exceptions and the image's own vectors replaced. The handlers of the
//! image's vectors are short assembly stubs that save only what they use, as
//! a kernel's interrupt path would; exceptions go to Rust with a full frame.
//!
//! Gate format: SDM vol. 3A 7.14.1 (64-bit IDT gate descriptors), type 0xE
//! (interrupt gate), present, DPL 0, IST 0. Shadow-stack frame of an
//! exception at the same privilege level: CS, LIP and SSP pushed in that
//! order (vol. 3A 7.12.1.1, figure 7-5), so the saved LIP is at SSP + 8.

use crate::percpu::{self, PerCpu};
use crate::x86::{self, DescriptorPtr};
use core::arch::global_asm;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// The image's vectors.
pub const V_FLAG: u8 = 0xE0;
pub const V_PONG: u8 = 0xE1;
pub const V_SHOOT: u8 = 0xE2;
pub const V_SELF: u8 = 0xE3;
pub const V_TIMER: u8 = 0xE4;
pub const V_SPURIOUS: u8 = 0xFF;

/// Read by the stubs: 1 when the local APIC is in x2APIC mode, and the
/// xAPIC register base otherwise.
#[unsafe(no_mangle)]
pub static DODOS_X2APIC: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static DODOS_XAPIC_BASE: AtomicU64 = AtomicU64::new(0xFEE0_0000);
/// The address the shootdown vector invalidates, and its acknowledgements.
#[unsafe(no_mangle)]
pub static DODOS_SHOOT_VA: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static DODOS_SHOOT_ACKS: AtomicU64 = AtomicU64::new(0);

/// The saved state of an exception, as `exc_common` pushes it.
#[repr(C)]
pub struct Frame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

macro_rules! eoi {
    () => {
        concat!(
            "cmp byte ptr [rip + DODOS_X2APIC], 0\n",
            "je 51f\n",
            "mov ecx, 0x80B\n",
            "xor eax, eax\n",
            "xor edx, edx\n",
            "wrmsr\n",
            "jmp 52f\n",
            "51:\n",
            "mov rax, [rip + DODOS_XAPIC_BASE]\n",
            "mov dword ptr [rax + 0xB0], 0\n",
            "52:\n"
        )
    };
}

global_asm!(
    ".macro EXC n, err",
    ".global dodos_exc_\\n",
    "dodos_exc_\\n:",
    "endbr64",
    ".if \\err == 0",
    "push 0",
    ".endif",
    "push \\n",
    "jmp dodos_exc_common",
    ".endm",
    "EXC 0,0", "EXC 1,0", "EXC 3,0", "EXC 4,0", "EXC 5,0", "EXC 6,0", "EXC 7,0",
    "EXC 8,1", "EXC 9,0", "EXC 10,1", "EXC 11,1", "EXC 12,1", "EXC 13,1", "EXC 14,1",
    "EXC 15,0", "EXC 16,0", "EXC 17,1", "EXC 18,0", "EXC 19,0", "EXC 20,0", "EXC 21,1",
    "EXC 22,0", "EXC 23,0", "EXC 24,0", "EXC 25,0", "EXC 26,0", "EXC 27,0", "EXC 28,0",
    "EXC 29,1", "EXC 30,1", "EXC 31,0",
    "dodos_exc_common:",
    "push rax", "push rcx", "push rdx", "push rsi", "push rdi", "push r8", "push r9",
    "push r10", "push r11", "push rbx", "push rbp", "push r12", "push r13", "push r14",
    "push r15",
    // The SSP right after delivery (RDSSPQ is a NOP without shadow stacks,
    // vol. 2B RDSSPD/RDSSPQ, so RAX stays 0).
    "xor eax, eax",
    "rdsspq rax",
    "mov gs:[{ssp}], rax",
    "mov rbx, rsp",
    "sub rsp, 528",
    "and rsp, -16",
    "fxsave64 [rsp]",
    "mov rdi, rbx",
    "cld",
    "call {handler}",
    "fxrstor64 [rsp]",
    "mov rsp, rbx",
    "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx", "pop r11", "pop r10",
    "pop r9", "pop r8", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rax",
    "add rsp, 16",
    "iretq",
    // Records the arrival instant and counts.
    ".global dodos_irq_flag",
    "dodos_irq_flag:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "rdtsc",
    "shl rdx, 32",
    "or rax, rdx",
    "mov gs:[{irq_tsc}], rax",
    "inc qword ptr gs:[{irq_count}]",
    eoi!(),
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    // Answers with the flag vector to `pong_dest`, then counts.
    ".global dodos_irq_pong",
    "dodos_irq_pong:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "cmp byte ptr [rip + DODOS_X2APIC], 0",
    "je 61f",
    "mov ecx, 0x830",
    "mov edx, gs:[{pong}]",
    "mov eax, 0x40E0",
    "wrmsr",
    "jmp 62f",
    "61:",
    "mov rax, [rip + DODOS_XAPIC_BASE]",
    "mov ecx, gs:[{pong}]",
    "shl ecx, 24",
    "mov dword ptr [rax + 0x310], ecx",
    "mov dword ptr [rax + 0x300], 0x40E0",
    "62:",
    "inc qword ptr gs:[{irq_count}]",
    eoi!(),
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    // Invalidates the shootdown address and acknowledges.
    ".global dodos_irq_shoot",
    "dodos_irq_shoot:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "mov rax, [rip + DODOS_SHOOT_VA]",
    "invlpg [rax]",
    "lock inc qword ptr [rip + DODOS_SHOOT_ACKS]",
    "inc qword ptr gs:[{irq_count}]",
    eoi!(),
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    // Counts only: the cost of taking an interrupt.
    ".global dodos_irq_self",
    "dodos_irq_self:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "inc qword ptr gs:[{irq_count}]",
    eoi!(),
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    ".global dodos_irq_timer",
    "dodos_irq_timer:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "inc qword ptr gs:[{timer}]",
    eoi!(),
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    ".global dodos_irq_spurious",
    "dodos_irq_spurious:",
    "endbr64",
    "iretq",
    // NMI: records and counts; when asked, parks the core instead of
    // returning (the fence's NMI handler): IRET to the park entry with
    // interrupts masked, so that NMIs are unblocked while it waits.
    ".global dodos_nmi",
    "dodos_nmi:",
    "endbr64",
    "push rax", "push rcx", "push rdx",
    "rdtsc",
    "shl rdx, 32",
    "or rax, rdx",
    "mov gs:[{nmi_tsc}], rax",
    "inc qword ptr gs:[{nmi_count}]",
    "cmp qword ptr gs:[{nmi_park}], 0",
    "jne 71f",
    "pop rdx", "pop rcx", "pop rax",
    "iretq",
    "71:",
    // Mode 2: spin inside the handler with NMIs blocked (a kernel too
    // damaged to park itself); only INIT gets the core out.
    "cmp qword ptr gs:[{nmi_park}], 2",
    "jne 72f",
    "73:",
    "pause",
    "jmp 73b",
    "72:",
    "mov qword ptr gs:[{nmi_park}], 0",
    "pop rdx", "pop rcx", "pop rax",
    "lea rax, [rip + dodos_park_from_nmi]",
    "mov [rsp], rax",
    "and qword ptr [rsp + 16], -513",
    "iretq",
    ".pushsection .data",
    ".balign 8",
    "dodos_exc_table:",
    ".quad dodos_exc_0, dodos_exc_1, dodos_nmi, dodos_exc_3, dodos_exc_4, dodos_exc_5",
    ".quad dodos_exc_6, dodos_exc_7, dodos_exc_8, dodos_exc_9, dodos_exc_10, dodos_exc_11",
    ".quad dodos_exc_12, dodos_exc_13, dodos_exc_14, dodos_exc_15, dodos_exc_16, dodos_exc_17",
    ".quad dodos_exc_18, dodos_exc_19, dodos_exc_20, dodos_exc_21, dodos_exc_22, dodos_exc_23",
    ".quad dodos_exc_24, dodos_exc_25, dodos_exc_26, dodos_exc_27, dodos_exc_28, dodos_exc_29",
    ".quad dodos_exc_30, dodos_exc_31",
    ".popsection",
    "dodos_park_from_nmi:",
    "mov rsp, gs:[{stack_top}]",
    "call {park}",
    "ud2",
    ssp = const percpu::SSP_ENTRY,
    irq_tsc = const percpu::IRQ_TSC,
    irq_count = const percpu::IRQ_COUNT,
    pong = const percpu::PONG_DEST,
    timer = const percpu::TIMER_COUNT,
    nmi_tsc = const percpu::NMI_TSC,
    nmi_count = const percpu::NMI_COUNT,
    nmi_park = const percpu::NMI_PARK,
    stack_top = const percpu::STACK_TOP,
    handler = sym exception,
    park = sym crate::smp::park_after_nmi,
);

unsafe extern "C" {
    fn dodos_irq_flag();
    fn dodos_irq_pong();
    fn dodos_irq_shoot();
    fn dodos_irq_self();
    fn dodos_irq_timer();
    fn dodos_irq_spurious();
    /// The handlers of vectors 0 to 31 (2 is the NMI).
    static dodos_exc_table: [u64; 32];
}

fn gate(handler: u64, cs: u16) -> u128 {
    let h = u128::from(handler);
    (h & 0xFFFF) | u128::from(cs) << 16 | 0x8E << 40 | (h >> 16 & 0xFFFF) << 48 | (h >> 32) << 64
}

/// Builds the calling core's IDT in its per-core block, from the IDT the
/// firmware gave it, and loads it.
pub fn install() {
    let fw = x86::sidt();
    let me = percpu::me();
    let cs = x86::cs();
    let p = core::ptr::from_ref(me).cast_mut();
    // SAFETY: the per-core block is this core's own; the firmware IDT is
    // readable (identity mapped) and has `limit + 1` bytes.
    unsafe {
        let idt = &raw mut (*p).idt;
        let fw_entries = (usize::from(fw.limit) + 1) / 16;
        if fw.base != 0 && fw.base != (*idt).as_ptr() as u64 {
            let src = fw.base as *const u128;
            for v in 0..fw_entries.min(256) {
                (*idt)[v] = src.add(v).read_unaligned();
            }
        }
        for (v, &h) in dodos_exc_table.iter().enumerate() {
            (*idt)[v] = gate(h, cs);
        }
        let ours: [(u8, unsafe extern "C" fn()); 6] = [
            (V_FLAG, dodos_irq_flag),
            (V_PONG, dodos_irq_pong),
            (V_SHOOT, dodos_irq_shoot),
            (V_SELF, dodos_irq_self),
            (V_TIMER, dodos_irq_timer),
            (V_SPURIOUS, dodos_irq_spurious),
        ];
        for (v, h) in ours {
            (*idt)[usize::from(v)] = gate(h as usize as u64, cs);
        }
        let ptr = DescriptorPtr {
            limit: 256 * 16 - 1,
            base: (*idt).as_ptr() as u64,
        };
        x86::lidt(&ptr);
    }
}

/// Exceptions: resumes at the armed recovery point, or stops the image.
extern "sysv64" fn exception(f: &mut Frame) {
    let me: &PerCpu = percpu::me();
    let resume = me.fixup_rip.swap(0, Ordering::Relaxed);
    if resume != 0 {
        let word = 1 << 63 | (f.error & 0xFFFF_FFFF) << 8 | (f.vector & 0xFF);
        me.fixup_vector.store(word, Ordering::Relaxed);
        me.fixup_cr2.store(x86::cr2(), Ordering::Relaxed);
        let ssp = me.ssp_entry.load(Ordering::Relaxed);
        if ssp != 0 {
            // The shadow stack holds the interrupted LIP at SSP + 8 and
            // IRET checks it: write the new one there with WRSSQ.
            // SAFETY: SSP + 8 is the LIP slot of this frame (figure 7-5),
            // a shadow-stack address while shadow stacks are on.
            unsafe { core::arch::asm!("wrssq [{}], {}", in(reg) ssp + 8, in(reg) resume) };
        }
        f.rip = resume;
        return;
    }
    let cr2 = x86::cr2();
    panic!(
        "exception {} error {:#x} at rip {:#x} rsp {:#x} cr2 {:#x} on core {}",
        f.vector, f.error, f.rip, f.rsp, cr2, me.index
    );
}
