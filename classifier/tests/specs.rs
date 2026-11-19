//! The readers against the figures the two verifications of the research
//! draft counted, and the rule of D-125: every encoding of the domain is
//! classified once, and a classification names only encodings of the
//! specification. These tests read the snapshots of `flake.nix`.

use std::sync::OnceLock;

use dodos_classifier::{Access, Arch, Classification, Spec, aarch64, json, riscv64, x86_64};

fn spec(arch: Arch) -> Spec {
    static SPECS: OnceLock<Vec<Spec>> = OnceLock::new();
    let all = SPECS.get_or_init(|| {
        [Arch::Aarch64, Arch::Riscv64, Arch::X86_64]
            .into_iter()
            .map(|a| a.load().unwrap_or_else(|e| panic!("{e}")))
            .collect()
    });
    all.iter().find(|s| s.arch == arch).cloned().unwrap()
}

fn count(spec: &Spec, f: impl Fn(&dodos_classifier::Item) -> bool) -> usize {
    spec.items.iter().filter(|i| f(i)).count()
}

// Research draft architecture-boundary.md, part 4.1, confirmed exact by the
// verification by Hydrogen (16 November 2026): 598 encodings written by MSR
// or MSRR, 13 MSR immediate targets, and the system instructions per family.
#[test]
fn arm64_counts_match_the_verified_figures() {
    let s = spec(Arch::Aarch64);
    let writable = count(&s, |i| {
        i.key.starts_with("reg ") && i.rules.iter().any(|r| r.access == Access::Write)
    });
    assert_eq!(writable, 598);
    assert_eq!(count(&s, |i| i.key.starts_with("pstate ")), 13);
    for (family, n) in [
        ("TLBI", 166),
        ("TLBIP", 120),
        ("PLBI", 54),
        ("DC", 40),
        ("GIC", 25),
        ("AT", 17),
        ("IC", 3),
    ] {
        let prefix = format!("sys {family}.");
        assert_eq!(count(&s, |i| i.key.starts_with(&prefix)), n, "{family}");
    }
}

// Part 4.2, confirmed by both verifications: 417 CSRs with an address, 343
// writable, 259 M, 77 U, 58 S, 19 VS, 4 D.
#[test]
fn riscv_counts_match_the_verified_figures() {
    let s = spec(Arch::Riscv64);
    assert_eq!(count(&s, |i| i.key.starts_with("csr ")), 417);
    assert_eq!(
        count(&s, |i| i.key.starts_with("csr ")
            && i.note.contains(", writable")),
        343
    );
    for (mode, n) in [("M", 259), ("U", 77), ("S", 58), ("VS", 19), ("D", 4)] {
        let tag = format!("priv_mode {mode},");
        assert_eq!(
            count(&s, |i| i.key.starts_with("csr ")
                && i.note.starts_with(&tag)),
            n,
            "{mode}"
        );
    }
}

// Part 4.3, confirmed by both verifications: 69 instruction classes marked
// RING0 (XED marks the same records CPL 0).
#[test]
fn x86_ring0_classes_match_the_verified_figure() {
    let s = spec(Arch::X86_64);
    let mut classes: Vec<&str> = s
        .items
        .iter()
        .filter(|i| i.note.contains("ring 0"))
        .map(|i| i.key["iclass ".len()..].split(['/', '.']).next().unwrap())
        .collect();
    classes.sort_unstable();
    classes.dedup();
    assert_eq!(classes.len(), 69);
}

// D-125: the instructions the hand-written lists missed are in the domain
// and refused outside the gate, from the data, not from a list.
#[test]
fn the_missed_instructions_are_classified_and_refused() {
    let mut s = spec(Arch::X86_64);
    let c = Classification::check(&mut s, Arch::X86_64.classes()).unwrap();
    let missed = [
        "WRMSRNS",
        "WRMSRLIST",
        "LMSW",
        "CLTS",
        "XSETBV",
        "INVLPGB",
        "TLBSYNC",
        "ERETS",
        "ERETU",
        "LKGS",
        "LOADIWKEY",
        "SEAMCALL",
        "SEAMOPS",
        "SEAMRET",
        "TDCALL",
        "PVALIDATE",
        "RMPADJUST",
        "RMPUPDATE",
        "PSMASH",
        "HRESET",
        "SETSSBSY",
        "CLRSSBSY",
        "WRUSSD",
        "WRUSSQ",
    ];
    for name in missed {
        let keys: Vec<&str> = s
            .items
            .iter()
            .filter(|i| i.domain && i.key["iclass ".len()..].split(['/', '.']).next() == Some(name))
            .map(|i| i.key.as_str())
            .collect();
        assert!(!keys.is_empty(), "{name} is not in the domain");
        for k in keys {
            let class = c.class_of(k).unwrap();
            assert_ne!(class, dodos_classifier::Class::Kernel, "{k}");
        }
    }
    let imm = c.class_of("iclass RDMSR/RDMSR_GPR64u64_IMM32").unwrap();
    assert_eq!(imm, dodos_classifier::Class::Gate, "the immediate MSR form");
    let mut a = spec(Arch::Aarch64);
    let c = Classification::check(&mut a, Arch::Aarch64.classes()).unwrap();
    for target in ["pstate SVCRSM", "pstate SVCRZA", "pstate SVCRSMZA"] {
        assert_eq!(
            c.class_of(target),
            Some(dodos_classifier::Class::Gate),
            "{target}"
        );
    }
}

fn errors(spec: &mut Spec, text: &str) -> String {
    match Classification::check(spec, text) {
        Ok(_) => String::new(),
        Err(e) => e.0,
    }
}

fn arm_register(name: &str) -> json::Json {
    let v = |bits: &str| format!(r#"{{"_type": "Values.Value", "value": "'{bits}'"}}"#);
    let text = format!(
        r#"{{"_type": "Register", "state": "AArch64", "name": "{name}", "fieldsets": [],
            "accessors": [{{"_type": "Accessors.SystemAccessor", "name": "A64.MSRregister",
            "encoding": [{{"asmvalue": "{name}", "encodings": {{"op0": {}, "op1": {}, "CRn": {}, "CRm": {}, "op2": {}}}}}]}}]}}"#,
        v("11"),
        v("000"),
        v("1011"),
        v("0101"),
        v("111")
    );
    json::parse(&text, &[]).unwrap()
}

// Item 4 of the task: a snapshot with one more register fails until it is
// classified; a classification of an encoding the specification lacks fails.
#[test]
fn arm64_added_register_fails_until_classified() {
    let dir = Arch::Aarch64.spec_dir().unwrap();
    let (mut regs, insns) = aarch64::parse(&dir).unwrap();
    if let json::Json::Arr(records) = &mut regs {
        records.push(arm_register("DODOS_TEST_EL1"));
    }
    let mut s = aarch64::from_json(&regs, &insns).unwrap();
    let base = Arch::Aarch64.classes();
    assert!(errors(&mut s.clone(), base).contains("reg DODOS_TEST_EL1: not classified"));
    let classified = format!("{base}\nreg DODOS_TEST_EL1 kernel\n");
    assert_eq!(errors(&mut s, &classified), "");
}

#[test]
fn riscv_added_csr_fails_until_classified() {
    let (mut csrs, insts) = riscv64::texts(&Arch::Riscv64.spec_dir().unwrap()).unwrap();
    csrs.push("name: dodostest\naddress: 0x5C0\npriv_mode: S\nwritable: true\n".into());
    let mut s = riscv64::from_texts(&csrs, &insts).unwrap();
    let base = Arch::Riscv64.classes();
    assert!(errors(&mut s.clone(), base).contains("csr dodostest: not classified"));
    assert_eq!(
        errors(&mut s, &format!("{base}\ncsr dodostest unused UNUSED\n")),
        ""
    );
}

#[test]
fn x86_added_ring0_instruction_fails_until_classified() {
    let mut texts = x86_64::texts(&Arch::X86_64.spec_dir().unwrap()).unwrap();
    texts.push(
        "{\nICLASS : DODOSTEST\nCPL : 0\nCATEGORY : SYSTEM\nPATTERN : 0x0F 0x0B\nOPERANDS :\n}\n"
            .into(),
    );
    let mut s = x86_64::from_texts(&texts).unwrap();
    let base = Arch::X86_64.classes();
    assert!(errors(&mut s.clone(), base).contains("iclass DODOSTEST: not classified"));
    assert_eq!(
        errors(&mut s, &format!("{base}\niclass DODOSTEST gate CR\n")),
        ""
    );
}

#[test]
fn a_classified_encoding_the_specification_lacks_fails() {
    for (arch, line) in [
        (Arch::Aarch64, "reg NO_SUCH_EL1 kernel"),
        (Arch::Riscv64, "csr nosuch kernel"),
        (Arch::X86_64, "iclass NOSUCH kernel"),
    ] {
        let mut s = spec(arch);
        let text = format!("{}\n{line}\n", arch.classes());
        let key = line.rsplit_once(' ').unwrap().0;
        assert!(
            errors(&mut s, &text).contains(&format!("{key}: not in the specification")),
            "{line}"
        );
    }
}

#[test]
fn a_second_line_or_a_missing_reason_fails() {
    let mut s = spec(Arch::X86_64);
    let twice = format!("{}\niclass WRMSR gate MSR\n", Arch::X86_64.classes());
    assert!(errors(&mut s, &twice).contains("iclass WRMSR classified twice"));
    let unreasoned = Arch::X86_64
        .classes()
        .replace("iclass CLTS gate CR", "iclass CLTS gate");
    assert!(errors(&mut s, &unreasoned).contains("iclass CLTS: class gate needs a reason"));
}

#[test]
fn an_extra_encoding_the_database_has_fails() {
    let mut s = spec(Arch::Riscv64);
    let text = format!("{}\nextra csr satp 0x180 a test\n", Arch::Riscv64.classes());
    assert!(errors(&mut s, &text).contains("csr satp: extra, but the specification has it"));
}

// D-125: the database is "checked against the ratified text". Every CSR of
// the ratified table (src/priv/csrs.adoc of the pinned manual) is in the
// database or added by an `extra` line, at the same address.
#[test]
fn every_ratified_csr_is_in_the_database_or_added() {
    let table = std::env::var_os("DODOS_SPEC_RISCV64_CSRS")
        .expect("DODOS_SPEC_RISCV64_CSRS: run inside nix develop");
    let table = std::fs::read_to_string(table).unwrap();
    let mut s = spec(Arch::Riscv64);
    Classification::check(&mut s, Arch::Riscv64.classes()).unwrap();
    let mut seen = 0;
    for line in table.lines() {
        // |`0x180` |SRW |csr:satp[] |...
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let (Some(addr), Some(name)) = (cells.get(1), cells.get(3)) else {
            continue;
        };
        let (Some(addr), Some(name)) = (
            addr.strip_prefix("`0x").and_then(|a| a.strip_suffix('`')),
            name.strip_prefix("csr:").and_then(|n| n.split('[').next()),
        ) else {
            continue;
        };
        let addr = u32::from_str_radix(addr, 16).unwrap();
        let item = s
            .item(&format!("csr {name}"))
            .unwrap_or_else(|| panic!("csr {name} is missing"));
        let word = u32::from_le_bytes(item.rules[0].pattern.value[..4].try_into().unwrap());
        assert_eq!(word >> 20, addr, "csr {name}");
        seen += 1;
    }
    assert!(seen > 200, "the table was not read ({seen} rows)");
}

// Linux v7.3, arch/arm64/include/asm/sysreg.h, encodes the GICv5 system
// instructions independently: GIC CDAFF is sys_insn(1, 0, 12, 1, 3) and
// GICR CDIA sys_insn(1, 0, 12, 3, 0).
#[test]
fn gicv5_encodings_match_linux() {
    let s = spec(Arch::Aarch64);
    for (key, op1, crn, crm, op2, base) in [
        ("sys GIC.CDAFF", 0, 12, 1, 3, 0xD508_0000u32),
        ("sys GICR.CDIA", 0, 12, 3, 0, 0xD528_0000u32),
    ] {
        let p = &s.item(key).unwrap().rules[0].pattern;
        let word = u32::from_le_bytes(p.value[..4].try_into().unwrap());
        assert_eq!(
            word,
            base | (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5),
            "{key}"
        );
    }
}
