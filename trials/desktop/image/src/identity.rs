//! P-002: the processor's identity and features on every core, and whether
//! each feature can be switched on there (a CPUID bit alone was already
//! contradicted for PKS: kernel.md "The write gate: mechanisms").
//!
//! CPUID bits, SDM vol. 1 chapter 21: leaf 01H ECX[3] MONITOR, [5] VMX,
//! [17] PCID, [21] X2APIC, [24] TSC_DEADLINE (table 21-9); leaf 07H.0 EBX[10]
//! INVPCID (21-19), ECX[7] CET_SS, ECX[31] PKS (21-20), EDX[5] UINTR,
//! EDX[15] HYBRID, EDX[20] CET_IBT (21-21); leaf 07H.1 EAX[6] LASS,
//! EAX[17] FRED, EAX[18] LKGS, EAX[19] WRMSRNS, EAX[22] HRESET (21-22),
//! EDX[18] CET_SSS (21-25); leaf 1AH EAX[31:24] core type (21-63); leaf 05H
//! MWAIT sub-states (21-15); leaf 80000001H EDX[26] 1 GB pages.

use crate::out;
use crate::run::Ctx;
use crate::smp::{self, CoreKind, Role};
use crate::x86::{self, bit, cpuid};
use alloc::string::String;
use alloc::vec::Vec;
use dodos_trial_multicore::rt::Mutex;

#[derive(Clone, Default)]
struct Id {
    sig: u32,
    l1c: u32,
    l7b: u32,
    l7c: u32,
    l7d: u32,
    l71a: u32,
    l71d: u32,
    l1a: u32,
    l5d: u32,
    l5c: u32,
    l6a: u32,
    ext_d: u32,
    apic: u32,
    apic_base: Option<u64>,
    feature_control: Option<u64>,
    hwp_caps: Option<u64>,
    pat: Option<u64>,
    tsc_adjust: Option<u64>,
    smi_count: Option<u64>,
    cst_config: Option<u64>,
    cet_on: &'static str,
    pks_on: &'static str,
}

/// Tries CR4.CET (with CR0.WP set, as required: vol. 3A 2.5) and IA32_S_CET
/// = 0, then IA32_PKRS = 0 and CR4.PKS, restoring CR4 after each.
fn try_enable(features: (bool, bool)) -> (&'static str, &'static str) {
    let cr4 = x86::cr4();
    let result = |ok: Result<(), x86::Fault>| if ok.is_ok() { "ok" } else { "fault" };
    let cet = if !features.0 {
        "absent"
    } else if x86::cr0() & x86::CR0_WP == 0 {
        "wp-clear"
    } else {
        // SAFETY: CET with no enable bit in IA32_S_CET changes nothing that
        // the image uses; CR4 is restored at once.
        let r = unsafe { x86::safe_set_cr4(cr4 | x86::CR4_CET) }
            .and_then(|()| unsafe { x86::safe_wrmsr(x86::IA32_S_CET, 0) });
        // SAFETY: the value read at entry.
        unsafe { x86::set_cr4(cr4) };
        result(r)
    };
    let pks = if !features.1 {
        "absent"
    } else {
        // PKRS first: its value at this point is unknown (QEMU showed one
        // that denied writes to key 0), and with CR4.PKS set it governs
        // every supervisor access, the image's own included.
        // SAFETY: PKRS 0 permits every access; CR4 is restored at once.
        let r = unsafe { x86::safe_wrmsr(x86::IA32_PKRS, 0) }
            .and_then(|()| unsafe { x86::safe_set_cr4(cr4 | x86::CR4_PKS) });
        // SAFETY: the value read at entry.
        unsafe { x86::set_cr4(cr4) };
        result(r)
    };
    (cet, pks)
}

fn read_id() -> Id {
    let max = cpuid(0, 0).eax;
    let leaf = |l: u32, s: u32| {
        if max >= l {
            cpuid(l, s)
        } else {
            x86::Cpuid::default()
        }
    };
    let l7 = leaf(7, 0);
    let l71 = if l7.eax >= 1 {
        leaf(7, 1)
    } else {
        x86::Cpuid::default()
    };
    let (cet_on, pks_on) = try_enable((bit(l7.ecx, 7), bit(l7.ecx, 31)));
    Id {
        sig: cpuid(1, 0).eax,
        l1c: cpuid(1, 0).ecx,
        l7b: l7.ebx,
        l7c: l7.ecx,
        l7d: l7.edx,
        l71a: l71.eax,
        l71d: l71.edx,
        l1a: leaf(0x1A, 0).eax,
        l5d: leaf(5, 0).edx,
        l5c: leaf(5, 0).ecx,
        l6a: leaf(6, 0).eax,
        ext_d: cpuid(0x8000_0001, 0).edx,
        apic: x86::apic_id_cpuid(),
        apic_base: x86::safe_rdmsr(x86::IA32_APIC_BASE).ok(),
        feature_control: x86::safe_rdmsr(x86::IA32_FEATURE_CONTROL).ok(),
        hwp_caps: x86::safe_rdmsr(x86::IA32_HWP_CAPABILITIES).ok(),
        pat: x86::safe_rdmsr(x86::IA32_PAT).ok(),
        tsc_adjust: x86::safe_rdmsr(x86::IA32_TSC_ADJUST).ok(),
        smi_count: x86::safe_rdmsr(x86::MSR_SMI_COUNT).ok(),
        cst_config: x86::safe_rdmsr(x86::MSR_PKG_CST_CONFIG_CONTROL).ok(),
        cet_on,
        pks_on,
    }
}

fn brand() -> String {
    let mut b = Vec::new();
    for l in 0x8000_0002..=0x8000_0004u32 {
        let r = cpuid(l, 0);
        for w in [r.eax, r.ebx, r.ecx, r.edx] {
            b.extend_from_slice(&w.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&b)
        .trim_matches(char::from(0))
        .trim()
        .replace(' ', "_")
}

fn hex(v: Option<u64>) -> String {
    v.map_or("fault".into(), |x| alloc::format!("{x:#x}"))
}

fn kind_of(id: &Id) -> CoreKind {
    match id.l1a >> 24 {
        0x40 => CoreKind::P,
        0x20 => CoreKind::E,
        _ => CoreKind::Unknown,
    }
}

pub fn run(_cx: &mut Ctx) -> Result<(), String> {
    let n = smp::cores().len();
    let ids: Vec<Mutex<Option<Id>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let on: Vec<usize> = (0..n)
        .filter(|&i| smp::state(i) == smp::IDLE || i == 0)
        .filter(|&i| matches!(smp::cores()[i].role, Role::Bsp | Role::Worker))
        .collect();
    smp::run_on(&on, 5000, &|i| {
        *ids[i].lock().expect("id") = Some(read_id());
    })?;
    let sig = cpuid(1, 0).eax;
    let (family, model) = (
        (sig >> 8 & 0xF) + (sig >> 20 & 0xFF),
        (sig >> 4 & 0xF) | (sig >> 12 & 0xF0),
    );
    out!(
        "I cpu brand={} signature={sig:#x} family={family:#x} model={model:#x} stepping={} cores={n}",
        brand(),
        sig & 0xF
    );
    for &i in &on {
        let Some(id) = ids[i].lock().expect("id").clone() else {
            continue;
        };
        let b = |v: u32, k: u32| u8::from(bit(v, k));
        out!(
            "I features core={i} apic={} kind={} sig={:#x} native_model={:#x} cet_ss={} ibt={} cet_sss={} pks={} pku={} fred={} lkgs={} wrmsrns={} x2apic={} invpcid={} pcid={} tsc_deadline={} vmx={} monitor={} hybrid={} uintr={} lass={} hreset={} page1g={} cet_enable={} pks_enable={}",
            id.apic,
            kind_of(&id).name(),
            id.sig,
            id.l1a & 0xFF_FFFF,
            b(id.l7c, 7),
            b(id.l7d, 20),
            b(id.l71d, 18),
            b(id.l7c, 31),
            b(id.l7c, 3),
            b(id.l71a, 17),
            b(id.l71a, 18),
            b(id.l71a, 19),
            b(id.l1c, 21),
            b(id.l7b, 10),
            b(id.l1c, 17),
            b(id.l1c, 24),
            b(id.l1c, 5),
            b(id.l1c, 3),
            b(id.l7d, 15),
            b(id.l7d, 5),
            b(id.l71a, 6),
            b(id.l71a, 22),
            b(id.ext_d, 26),
            id.cet_on,
            id.pks_on
        );
        out!(
            "I msrs core={i} apic_base={} bsp_flag={} feature_control={} hwp_caps={} pat={} tsc_adjust={} smi_count={} pkg_cst_config={} mwait_substates={:#x} mwait_ext={:#x} thermal_power={:#x}",
            hex(id.apic_base),
            id.apic_base
                .map_or(2, |v| u8::from(v & x86::APIC_BASE_BSP != 0)),
            hex(id.feature_control),
            hex(id.hwp_caps),
            hex(id.pat),
            hex(id.tsc_adjust),
            hex(id.smi_count),
            hex(id.cst_config),
            id.l5d,
            id.l5c,
            id.l6a
        );
    }
    Ok(())
}

/// Every boot (from `setup`): each worker tells its kind. With
/// `fake_kinds=1` and no hybrid leaf (QEMU), the first half of the workers
/// are named P and the rest E, so that every path across kinds runs.
pub fn assign_kinds(cx: &Ctx) -> Result<(), String> {
    let n = smp::cores().len();
    let found: Vec<Mutex<CoreKind>> = (0..n).map(|_| Mutex::new(CoreKind::Unknown)).collect();
    let on: Vec<usize> = (0..n)
        .filter(|&i| i == 0 || smp::state(i) == smp::IDLE)
        .filter(|&i| matches!(smp::cores()[i].role, Role::Bsp | Role::Worker))
        .collect();
    smp::run_on(&on, 5000, &|i| {
        let max = cpuid(0, 0).eax;
        let l1a = if max >= 0x1A { cpuid(0x1A, 0).eax } else { 0 };
        *found[i].lock().expect("kind") = match l1a >> 24 {
            0x40 => CoreKind::P,
            0x20 => CoreKind::E,
            _ => CoreKind::Unknown,
        };
    })?;
    let mut kinds: Vec<CoreKind> = found.iter().map(|k| *k.lock().expect("kind")).collect();
    if kinds.iter().all(|&k| k == CoreKind::Unknown) && cx.flag("fake_kinds") {
        let w: Vec<usize> = (0..n)
            .filter(|&i| smp::cores()[i].role == Role::Worker)
            .collect();
        for (k, &i) in w.iter().enumerate() {
            kinds[i] = if k < w.len().div_ceil(2) {
                CoreKind::P
            } else {
                CoreKind::E
            };
        }
        kinds[0] = CoreKind::P;
        out!("I kinds faked=1 (fake_kinds=1): first half of the workers P, the rest E");
    }
    smp::set_kinds(&kinds);
    Ok(())
}
