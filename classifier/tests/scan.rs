//! The scanner of G11 on small objects assembled by the Zig toolchain of the
//! Nix shell: an instruction refused outside the gate is reported in
//! `.text` and not in `.text.gate`; an instruction refused everywhere is
//! reported in both. The lists go through their text form first.

use std::path::PathBuf;
use std::process::Command;

use dodos_classifier::scan::{Finding, read_list, scan, write_list};
use dodos_classifier::{Arch, Verdict, build};

const GATE: &str = ".text.gate";

/// Assembles `source` for `arch` and scans it with the generated list.
fn assemble_and_scan(arch: Arch, name: &str, source: &str) -> Vec<Finding> {
    let dir = std::env::temp_dir().join(format!("dodos-scan-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("t.S");
    let obj: PathBuf = dir.join("t.o");
    std::fs::write(&src, source).unwrap();
    let target = format!("{}-freestanding-none", arch.name());
    let out = Command::new("zig")
        .args(["cc", "-target", &target, "-c"])
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("zig: run inside nix develop");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (_, _, list) = build(arch).unwrap();
    let (arch_back, list) = read_list(&write_list(arch, &list)).unwrap();
    assert_eq!(arch_back, arch);
    let bytes = std::fs::read(&obj).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    scan(arch, &list, &bytes, &[GATE]).unwrap()
}

fn keys_in<'f>(findings: &'f [Finding], section: &str) -> Vec<&'f str> {
    findings
        .iter()
        .filter(|f| f.section == section)
        .map(|f| f.key.as_str())
        .collect()
}

#[test]
fn arm64_scan_by_word() {
    let f = assemble_and_scan(
        Arch::Aarch64,
        "arm",
        ".arch armv9.4-a\n.arch_extension sme\n.text\n\
         msr ttbr0_el1, x0\nmrs x1, ttbr0_el1\nmsr tpidr_el1, x2\ntlbi vae1, x3\ntlbi vae1is, x3\n\
         smstart\nsmstop sm\nmsr dbgbcr5_el1, x4\n\
         .section .text.gate,\"ax\"\nmsr ttbr0_el1, x0\nsmc #0\nmsr scr_el3, x0\nmsr s3_0_c15_c2_0, x0\n",
    );
    assert_eq!(
        keys_in(&f, ".text"),
        [
            "reg TTBR0_EL1",
            "sys TLBI.VAE1IS",
            "pstate SVCRSMZA",
            "pstate SVCRSM",
            "reg DBGBCR<m>_EL1"
        ]
    );
    assert_eq!(
        keys_in(&f, GATE),
        ["reg SCR_EL3", "reg S3_<op1>_C<Cn>_C<Cm>_<op2>"]
    );
    assert!(
        f.iter()
            .filter(|x| x.section == GATE)
            .all(|x| x.verdict == Verdict::Never)
    );
    assert!(f.iter().all(|x| x.offset % 4 == 0));
}

// With compressed instructions a 4-byte instruction may start at any even
// offset: the scan steps by 2 bytes.
#[test]
fn riscv_scan_by_halfword() {
    let f = assemble_and_scan(
        Arch::Riscv64,
        "rv",
        ".option arch, +zicsr, +c, +zicbom\n.text\n\
         c.nop\ncsrw satp, a0\ncsrr a1, satp\ncsrw sscratch, a2\ncbo.inval (a0)\nc.ebreak\n\
         .section .text.gate,\"ax\"\ncsrw satp, a0\necall\ncsrw mstatus, a0\n",
    );
    let text: Vec<&Finding> = f.iter().filter(|x| x.section == ".text").collect();
    assert_eq!(
        text.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(),
        ["csr satp", "insn cbo.inval"]
    );
    assert_eq!(text[0].offset, 2, "csrw satp follows a 2-byte c.nop");
    assert_eq!(keys_in(&f, GATE), ["csr mstatus"]);
}

// Every byte offset: a forbidden encoding hidden in the immediate of an
// allowed instruction is reported too.
#[test]
fn x86_scan_by_byte() {
    let f = assemble_and_scan(
        Arch::X86_64,
        "x86",
        ".text\nnop\nwrmsr\nrdmsr\nmov %rax, %cr3\nmov %cr2, %rax\nxsetbv\nmovl $0x300f, %eax\n\
         .section .text.gate,\"ax\"\nwrmsr\nmov %rax, %cr3\nsyscall\n",
    );
    let text: Vec<&Finding> = f.iter().filter(|x| x.section == ".text").collect();
    assert_eq!(
        text.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(),
        [
            "iclass WRMSR",
            "iclass MOV_CR.write",
            "iclass XSETBV",
            "iclass WRMSR"
        ]
    );
    assert_eq!(text[0].offset, 1, "wrmsr follows a 1-byte nop");
    assert_eq!(keys_in(&f, GATE), ["iclass SYSCALL"]);
}

// D-125: the instructions the hand-written lists missed come out of the
// tool. Each is assembled in a section of its own and must be reported
// there with its own class name.
#[test]
fn the_missed_instructions_are_found_in_a_binary() {
    let cases = [
        ("WRMSRNS", "wrmsrns"),
        ("WRMSRLIST", "wrmsrlist"),
        ("RDMSR/RDMSR_GPR64u64_IMM32", "rdmsr $0x10, %rax"),
        ("WRMSRNS/WRMSRNS_IMM32_GPR64u64", "wrmsrns %rax, $0x10"),
        ("LMSW", "lmsw %ax"),
        ("CLTS", "clts"),
        ("XSETBV", "xsetbv"),
        ("INVLPGB", "invlpgb"),
        ("TLBSYNC", "tlbsync"),
        ("ERETS", "erets"),
        ("ERETU", "eretu"),
        ("LKGS", "lkgs %ax"),
        ("LOADIWKEY", "loadiwkey %xmm1, %xmm2"),
        ("SEAMCALL", "seamcall"),
        ("SEAMOPS", "seamops"),
        ("SEAMRET", "seamret"),
        ("TDCALL", "tdcall"),
        ("PVALIDATE", "pvalidate"),
        ("RMPADJUST", "rmpadjust"),
        ("RMPUPDATE", "rmpupdate"),
        ("PSMASH", "psmash"),
        ("HRESET", "hreset $1"),
        ("SETSSBSY", "setssbsy"),
        ("CLRSSBSY", "clrssbsy (%rax)"),
        ("WRUSSD", "wrussd %eax, (%rbx)"),
        ("WRUSSQ", "wrussq %rax, (%rbx)"),
    ];
    let mut source = String::new();
    for (i, (_, insn)) in cases.iter().enumerate() {
        source.push_str(&format!(".section .t{i},\"ax\"\n{insn}\n"));
    }
    let f = assemble_and_scan(Arch::X86_64, "missed", &source);
    for (i, (class, insn)) in cases.iter().enumerate() {
        let here = keys_in(&f, &format!(".t{i}"));
        let wanted = format!("iclass {class}");
        assert!(
            here.iter()
                .any(|k| *k == wanted || k.starts_with(&format!("{wanted}/"))),
            "{insn}: {here:?}"
        );
    }
    let arm = assemble_and_scan(
        Arch::Aarch64,
        "missed-arm",
        ".arch armv9.4-a\n.arch_extension sme\n.section .t0,\"ax\"\nsmstart\n.section .t1,\"ax\"\nsmstop\n",
    );
    assert_eq!(keys_in(&arm, ".t0"), ["pstate SVCRSMZA"]);
    assert_eq!(keys_in(&arm, ".t1"), ["pstate SVCRSMZA"]);
}

// D-126: the generated Zig file of the register layer compiles, every
// declaration analysed.
#[test]
fn generated_zig_compiles() {
    let dir = std::env::temp_dir().join(format!("dodos-zig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for arch in [Arch::Aarch64, Arch::Riscv64] {
        let (spec, classes, _) = build(arch).unwrap();
        let path = dir.join(format!("{}.zig", arch.name()));
        std::fs::write(&path, dodos_classifier::regs::zig(&spec, &classes)).unwrap();
        let out = Command::new("zig")
            .args(["build-obj", "-fno-emit-bin", "--cache-dir"])
            .arg(dir.join("cache"))
            .arg("--global-cache-dir")
            .arg(dir.join("global"))
            .arg(&path)
            .output()
            .expect("zig: run inside nix develop");
        assert!(
            out.status.success(),
            "{}: {}",
            arch.name(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
