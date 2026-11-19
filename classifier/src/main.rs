//! `classify check [ARCH...]`: checks the classifications and prints the
//! counts per class and the undecided encodings.
//! `classify generate DIR`: writes the scan lists and the register
//! definitions of the three architectures into DIR.
//! `classify items ARCH`: lists the encodings of the specification, one
//! per line: key, `domain` or `-`, what the specification says of it.
//! `classify scan LIST FILE [--gate SECTION]...`: scans an ELF object or
//! image with a scan list; exits with 1 if anything is reported.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use dodos_classifier::{ARCHES, Arch, Class, Error, build, regs, scan};

fn check(names: &[String]) -> Result<(), Error> {
    let arches: Vec<Arch> = if names.is_empty() {
        ARCHES.to_vec()
    } else {
        names
            .iter()
            .map(|n| Arch::from_name(n).ok_or_else(|| Error(format!("unknown architecture {n}"))))
            .collect::<Result<_, _>>()?
    };
    for arch in arches {
        let (spec, classes, list) = build(arch)?;
        let domain = spec.items.iter().filter(|i| i.domain).count();
        let counts: Vec<String> = classes
            .counts()
            .iter()
            .map(|(c, n)| format!("{} {n}", c.name()))
            .collect();
        println!(
            "{}: {} ({domain} encodings in the domain, {} classified, {} patterns): {}",
            arch.name(),
            spec.release,
            classes.lines.len(),
            list.len(),
            counts.join(", ")
        );
        for (key, line) in classes
            .lines
            .iter()
            .filter(|(_, l)| l.class == Class::Undecided)
        {
            println!(
                "  undecided: {key} ({})",
                line.reason.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    std::fs::write(path, text).map_err(|e| Error(format!("{}: {e}", path.display())))
}

fn generate(dir: &str) -> Result<(), Error> {
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir).map_err(|e| Error(format!("{}: {e}", dir.display())))?;
    for arch in ARCHES {
        let (spec, classes, list) = build(arch)?;
        write(
            &dir.join(format!("{}.scan", arch.name())),
            &scan::write_list(arch, &list),
        )?;
        if !spec.regs.is_empty() {
            write(
                &dir.join(format!("{}.rs", arch.name())),
                &regs::rust(&spec, &classes),
            )?;
            write(
                &dir.join(format!("{}.zig", arch.name())),
                &regs::zig(&spec, &classes),
            )?;
        }
    }
    Ok(())
}

fn scan_file(args: &[String]) -> Result<bool, Error> {
    let [list, file, rest @ ..] = args else {
        return Err(Error(
            "usage: classify scan LIST FILE [--gate SECTION]...".into(),
        ));
    };
    let mut gate = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--gate", Some(s)) => gate.push(s.as_str()),
            _ => return Err(Error(format!("unexpected argument {a}"))),
        }
    }
    let (arch, entries) = scan::read_list(&dodos_classifier::read(Path::new(list))?)?;
    let bytes = std::fs::read(file).map_err(|e| Error(format!("{file}: {e}")))?;
    let findings = scan::scan(arch, &entries, &bytes, &gate)?;
    for f in &findings {
        println!(
            "{file}: {}+{:#x}: {} {}",
            f.section,
            f.offset,
            f.verdict.name(),
            f.key
        );
    }
    Ok(findings.is_empty())
}

fn items(name: &str) -> Result<(), Error> {
    let arch =
        Arch::from_name(name).ok_or_else(|| Error(format!("unknown architecture {name}")))?;
    for i in arch.load()?.items {
        println!(
            "{}\t{}\t{}",
            i.key,
            if i.domain { "domain" } else { "-" },
            i.note
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]).map(|()| true),
        Some("generate") if args.len() == 2 => generate(&args[1]).map(|()| true),
        Some("scan") => scan_file(&args[1..]),
        Some("items") if args.len() == 2 => items(&args[1]).map(|()| true),
        _ => Err(Error("usage: classify check [ARCH...] | generate DIR | items ARCH | scan LIST FILE [--gate SECTION]...".into())),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
