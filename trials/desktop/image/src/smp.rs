//! The cores. The firmware's MP services (PI 1.8 volume 2,
//! `EFI_MP_SERVICES_PROTOCOL.StartupThisAP`, non-blocking) start each
//! application processor once in `ap_start`, which moves to the image's
//! stack, IDT and page tables and serves jobs until the end; a core
//! restarted by INIT and SIPI comes back through the trampoline of `sipi`
//! to the same loop. The bootstrap core is core 0 and keeps the firmware.
//!
//! Roles: one *worker* per physical core (its first hardware thread), the
//! measured cores; the other hardware threads of a core halt with
//! interrupts masked (D-028: the second thread stays idle), except one,
//! the *watchdog*, which wakes every 10 ms and resets the machine through
//! the ACPI reset register when a step runs past its limit (README,
//! "Safety"). Without a second thread anywhere, the last worker is the
//! watchdog.

use crate::efi::{self, MpServices, ProcessorInformation};
use crate::percpu::{self, MAX_CORES};
use crate::{apic, con, idt, out, paging, run, tsc, x86};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoreKind {
    /// CPUID.1AH:EAX[31:24] = 40H, "Intel Core" (SDM vol. 1 table 21-63).
    P,
    /// 20H, "Intel Atom".
    E,
    /// No hybrid information.
    Unknown,
}

impl CoreKind {
    pub fn name(self) -> &'static str {
        match self {
            CoreKind::P => "P",
            CoreKind::E => "E",
            CoreKind::Unknown => "U",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Bsp,
    Worker,
    Sibling,
    Watchdog,
}

#[derive(Clone, Copy, Debug)]
pub struct Core {
    pub mp: usize,
    pub apic: u32,
    pub thread: u32,
    pub kind: CoreKind,
    pub role: Role,
}

/// The cores, fixed by `start` before any application processor runs.
static CORES: AtomicPtr<Vec<Core>> = AtomicPtr::new(null_mut());

pub fn cores() -> &'static [Core] {
    let p = CORES.load(Ordering::Acquire);
    if p.is_null() {
        return &[];
    }
    // SAFETY: set once from a leaked Box and never changed after.
    unsafe { &*p }
}

/// The workers in measuring order: performance cores, then efficiency
/// cores, then unknown; the bootstrap core is not among them.
pub fn workers() -> Vec<usize> {
    let mut w: Vec<usize> = (0..cores().len())
        .filter(|&i| cores()[i].role == Role::Worker && state(i) == IDLE)
        .collect();
    w.sort_by_key(|&i| match cores()[i].kind {
        CoreKind::P => 0,
        CoreKind::E => 1,
        CoreKind::Unknown => 2,
    });
    w
}

pub fn kind(i: usize) -> CoreKind {
    cores()[i].kind
}

pub const OFF: u32 = 0;
pub const IDLE: u32 = 1;
pub const PANICKED: u32 = 3;
pub const LOST: u32 = 4;

#[repr(align(128))]
struct Ctl {
    state: AtomicU32,
    seq: AtomicU64,
    done: AtomicU64,
    job_data: AtomicUsize,
    job_vtable: AtomicUsize,
    panic: [AtomicU64; 32],
}

static CTL: [Ctl; MAX_CORES] = [const {
    Ctl {
        state: AtomicU32::new(OFF),
        seq: AtomicU64::new(0),
        done: AtomicU64::new(0),
        job_data: AtomicUsize::new(0),
        job_vtable: AtomicUsize::new(0),
        panic: [const { AtomicU64::new(0) }; 32],
    }
}; MAX_CORES];

pub fn state(i: usize) -> u32 {
    CTL[i].state.load(Ordering::Acquire)
}

pub fn set_state(i: usize, s: u32) {
    CTL[i].state.store(s, Ordering::Release);
}

/// The arguments `ap_start` finds at its entry: stack top and core index.
#[repr(C)]
struct Boot {
    stack_top: u64,
    index: u64,
}

static mut BOOT: [Boot; MAX_CORES] = [const { Boot { stack_top: 0, index: 0 } }; MAX_CORES];

const STACK_BYTES: usize = 256 << 10;

global_asm!(
    // Called by the firmware with the MS x64 convention: RCX = &Boot.
    ".global dodos_ap_fw_entry",
    "dodos_ap_fw_entry:",
    "mov rax, [rcx]",
    "mov rdi, [rcx + 8]",
    "mov rsp, rax",
    "and rsp, -16",
    "call {start}",
    "ud2",
    start = sym ap_start,
);

unsafe extern "efiapi" {
    fn dodos_ap_fw_entry(arg: *mut core::ffi::c_void);
}

/// The state of the firmware on each core, which the trampoline puts back
/// after INIT and SIPI (descriptor tables, paging and mode registers).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FwState {
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub efer: u64,
    pub gdt: x86::DescriptorPtr,
    pub cs: u16,
}

pub static mut FW: [FwState; MAX_CORES] = [FwState {
    cr0: 0,
    cr3: 0,
    cr4: 0,
    efer: 0,
    gdt: x86::DescriptorPtr { limit: 0, base: 0 },
    cs: 0,
}; MAX_CORES];

pub fn save_fw(i: usize) {
    // SAFETY: entry `i` belongs to the calling core; EFER exists in long
    // mode.
    unsafe {
        let f = &raw mut FW[i];
        (*f).cr0 = x86::cr0();
        (*f).cr3 = x86::cr3();
        (*f).cr4 = x86::cr4();
        (*f).efer = x86::rdmsr(x86::IA32_EFER);
        (*f).gdt = x86::sgdt();
        (*f).cs = x86::cs();
    }
}

/// Entry of an application processor on its own stack.
extern "sysv64" fn ap_start(i: usize) -> ! {
    save_fw(i);
    // SAFETY: core `i`'s boot record, written before StartupThisAP.
    let top = unsafe { (*(&raw const BOOT))[i].stack_top };
    percpu::install(i, top);
    idt::install();
    // SAFETY: the image's page tables map all memory as the firmware's did
    // (identity), and add only the test window.
    unsafe { x86::set_cr3(paging::root()) };
    ap_local_setup();
    serve(i)
}

/// What every core of the image does to its APIC on entry and restart.
pub fn ap_local_setup() {
    if apic::x2() {
        apic::enter_x2apic_local();
    }
    apic::enable_local();
}

/// The loop of a core: workers serve jobs, siblings halt, the watchdog
/// watches.
pub fn serve(i: usize) -> ! {
    set_state(i, IDLE);
    match cores().get(i).map(|c| c.role) {
        Some(Role::Sibling) => loop {
            x86::cli();
            x86::hlt();
        },
        Some(Role::Watchdog) => watchdog_loop(),
        _ => {}
    }
    x86::sti();
    let c = &CTL[i];
    loop {
        let s = c.seq.load(Ordering::Acquire);
        if s != c.done.load(Ordering::Relaxed) {
            let job: &(dyn Fn(usize) + Sync) = unsafe {
                // SAFETY: the two words of a `&dyn Fn` posted by `post`,
                // valid until `done` reaches `s` (the poster waits for it).
                core::mem::transmute::<(usize, usize), &(dyn Fn(usize) + Sync)>((
                    c.job_data.load(Ordering::Relaxed),
                    c.job_vtable.load(Ordering::Relaxed),
                ))
            };
            job(i);
            x86::sti();
            c.done.store(s, Ordering::Release);
        } else {
            x86::pause();
        }
    }
}

fn post(i: usize, f: &(dyn Fn(usize) + Sync)) {
    // SAFETY: splits the fat pointer; `wait` keeps `f` alive until done.
    let (d, v) = unsafe { core::mem::transmute::<&(dyn Fn(usize) + Sync), (usize, usize)>(f) };
    let c = &CTL[i];
    c.job_data.store(d, Ordering::Relaxed);
    c.job_vtable.store(v, Ordering::Relaxed);
    c.seq.fetch_add(1, Ordering::Release);
}

/// Waits until every core of `on` has finished its job, a core panicked or
/// was lost, or `limit_ms` passed.
pub fn wait(on: &[usize], limit_ms: u64) -> Result<(), String> {
    let end = x86::rdtsc() + tsc::ns_to_ticks(limit_ms * 1_000_000);
    for &i in on {
        let c = &CTL[i];
        loop {
            if c.done.load(Ordering::Acquire) == c.seq.load(Ordering::Relaxed) {
                break;
            }
            match state(i) {
                PANICKED => return Err(alloc::format!("core {i} panicked: {}", panic_text(i))),
                LOST => return Err(alloc::format!("core {i} lost")),
                _ => {}
            }
            if x86::rdtsc() > end {
                return Err(alloc::format!("core {i} did not finish within {limit_ms} ms"));
            }
            x86::pause();
        }
    }
    Ok(())
}

/// Runs `f(i)` on every core of `on` at once (the bootstrap core, 0, runs
/// its part inline) and waits for all.
pub fn run_on(on: &[usize], limit_ms: u64, f: &(dyn Fn(usize) + Sync)) -> Result<(), String> {
    for &i in on.iter().filter(|&&i| i != 0) {
        post(i, f);
    }
    if on.contains(&0) {
        f(0);
    }
    let aps: Vec<usize> = on.iter().copied().filter(|&i| i != 0).collect();
    wait(&aps, limit_ms)
}

/// Posts `f` to the cores of `on` without waiting (they start at once).
pub fn start_on(on: &[usize], f: &(dyn Fn(usize) + Sync)) {
    for &i in on {
        post(i, f);
    }
}

/// Recovers a core that was restarted while it held a job: its loop starts
/// afresh, so the job counts as done.
pub fn forget_job(i: usize) {
    let c = &CTL[i];
    c.done.store(c.seq.load(Ordering::Relaxed), Ordering::Release);
}

fn panic_text(i: usize) -> String {
    let mut b = Vec::new();
    for w in &CTL[i].panic {
        b.extend_from_slice(&w.load(Ordering::Relaxed).to_le_bytes());
    }
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// The panic handler of an application processor: records the message for
/// the bootstrap core (no allocation: the heap lock could be held) and
/// stops the core.
pub fn core_panicked(info: &PanicInfo) -> ! {
    struct Fixed([u8; 255], usize);
    impl Write for Fixed {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.1 < self.0.len() {
                    self.0[self.1] = b;
                    self.1 += 1;
                }
            }
            Ok(())
        }
    }
    let mut f = Fixed([0; 255], 0);
    let _ = write!(f, "{}", info.message());
    if let Some(l) = info.location() {
        let _ = write!(f, " at {}:{}", l.file(), l.line());
    }
    let i = percpu::my_index();
    for (k, w) in CTL[i].panic.iter().enumerate() {
        let mut b = [0u8; 8];
        for (j, x) in b.iter_mut().enumerate() {
            *x = *f.0.get(k * 8 + j).unwrap_or(&0);
        }
        w.store(u64::from_le_bytes(b), Ordering::Relaxed);
    }
    set_state(i, PANICKED);
    loop {
        x86::cli();
        x86::hlt();
    }
}

/// After the fence's NMI handler: confirms the park request, halts with
/// interrupts masked, and on a new start request (sent with an NMI) serves
/// jobs again. The interrupted job is abandoned.
pub extern "sysv64" fn park_after_nmi() -> ! {
    let me = percpu::me();
    me.parked.store(me.park_request.load(Ordering::Relaxed), Ordering::Release);
    let start = me.start_request.load(Ordering::Acquire);
    while me.start_request.load(Ordering::Acquire) == start {
        x86::cli();
        x86::hlt();
    }
    let i = percpu::my_index();
    forget_job(i);
    serve(i)
}

static DEADLINE: AtomicU64 = AtomicU64::new(0);

/// Sets the watchdog: reset the machine if `ticks` pass before the next
/// call (0 disarms).
pub fn watch(ticks: u64) {
    let d = if ticks == 0 { 0 } else { x86::rdtsc() + ticks };
    DEADLINE.store(d, Ordering::Release);
}

fn watchdog_loop() -> ! {
    let deadline_timer = x86::bit(x86::cpuid(1, 0).ecx, 24);
    x86::sti();
    loop {
        if deadline_timer {
            apic::arm_deadline(idt::V_TIMER, x86::rdtsc() + tsc::ns_to_ticks(10_000_000));
            x86::hlt();
        } else {
            tsc::spin_ns(10_000_000);
        }
        let d = DEADLINE.load(Ordering::Acquire);
        if d != 0 && x86::rdtsc() > d {
            for b in b"\r\nWATCHDOG: a step ran past its limit; resetting the machine.\r\n" {
                con::serial_byte(*b);
            }
            crate::reset::acpi_reset();
        }
    }
}

/// Starts every enabled application processor. Returns the number started.
pub fn start(st: *mut efi::SystemTable) -> Result<usize, String> {
    let mut mp: *mut core::ffi::c_void = null_mut();
    // SAFETY: boot services are valid.
    let bs = unsafe { (*st).boot_services };
    let found = unsafe { ((*bs).locate_protocol)(&efi::MP_SERVICES_GUID, null_mut(), &mut mp) };
    let mut list = Vec::new();
    let bsp_apic = x86::apic_id_cpuid();
    let shift = if x86::cpuid(0, 0).eax >= 0xB { x86::cpuid(0xB, 0).eax & 0x1F } else { 0 };
    list.push(Core {
        mp: usize::MAX,
        apic: bsp_apic,
        thread: bsp_apic & ((1 << shift) - 1),
        kind: CoreKind::Unknown,
        role: Role::Bsp,
    });
    let mp = mp.cast::<MpServices>();
    if found == efi::SUCCESS {
        let (mut n, mut enabled) = (0usize, 0usize);
        // SAFETY: the firmware's MP services.
        unsafe { ((*mp).get_number_of_processors)(mp, &mut n, &mut enabled) };
        for k in 0..n {
            let mut info = ProcessorInformation::default();
            // SAFETY: as above; `info` is large enough.
            let r = unsafe { ((*mp).get_processor_info)(mp, k, &mut info) };
            if r != efi::SUCCESS
                || info.status_flag & efi::PROCESSOR_AS_BSP_BIT != 0
                || info.status_flag & efi::PROCESSOR_ENABLED_BIT == 0
                || list.len() == MAX_CORES
            {
                continue;
            }
            let apic = info.processor_id as u32;
            let thread = apic & ((1 << shift) - 1);
            list.push(Core {
                mp: k,
                apic,
                thread,
                kind: CoreKind::Unknown,
                role: if thread == 0 { Role::Worker } else { Role::Sibling },
            });
        }
    } else {
        out!("F smp no MP services protocol: only the bootstrap core measures");
    }
    // The watchdog: the last second thread, or the last worker.
    let wd = list
        .iter()
        .rposition(|c| c.role == Role::Sibling)
        .or_else(|| list.iter().rposition(|c| c.role == Role::Worker));
    if let Some(w) = wd.filter(|_| list.len() > 2) {
        list[w].role = Role::Watchdog;
    }
    CORES.store(Box::into_raw(Box::new(list)), Ordering::Release);
    let n = cores().len();
    let mut started = 0;
    for i in 1..n {
        let stack = alloc::vec![0u8; STACK_BYTES].leak();
        let top = stack.as_ptr() as u64 + STACK_BYTES as u64;
        // SAFETY: core `i`'s boot record, read by it after the call below.
        unsafe {
            let b = &raw mut (*(&raw mut BOOT))[i];
            (*b).stack_top = top;
            (*b).index = i as u64;
        }
        let mut ev: efi::Event = null_mut();
        // SAFETY: boot services and MP services are valid; the procedure
        // never returns, so the firmware's event never signals (harmless).
        let r = unsafe {
            ((*bs).create_event)(0, 0, 0, 0, &mut ev);
            ((*mp).startup_this_ap)(
                mp,
                dodos_ap_fw_entry,
                cores()[i].mp,
                ev,
                0,
                (&raw mut (*(&raw mut BOOT))[i]).cast(),
                null_mut(),
            )
        };
        if r != efi::SUCCESS {
            out!("F smp core={i} apic={} startup_status={r:#x}", cores()[i].apic);
            continue;
        }
        let end = x86::rdtsc() + tsc::ns_to_ticks(2_000_000_000);
        while state(i) != IDLE && x86::rdtsc() < end {
            x86::pause();
        }
        if state(i) == IDLE {
            started += 1;
        } else {
            out!("F smp core={i} apic={} did not reach the image within 2 s", cores()[i].apic);
            set_state(i, LOST);
        }
    }
    Ok(started)
}

/// Sets the kinds once each core told its own (`identity`).
pub fn set_kinds(kinds: &[CoreKind]) {
    let p = CORES.load(Ordering::Acquire);
    // SAFETY: only the bootstrap core writes, while no core reads kinds
    // (between two steps).
    unsafe {
        for (c, k) in (*p).iter_mut().zip(kinds) {
            c.kind = *k;
        }
    }
}

/// Whether the bootstrap core is the caller (for modules shared by both).
pub fn on_bsp() -> bool {
    run::is_bsp()
}

use alloc::boxed::Box;
