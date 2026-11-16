//! The scan lists in text, an ELF reader and the binary scanner of G11.
//!
//! A list has a header line `arch NAME unit N` and one line per pattern:
//! `VERDICT VALUE MASK PREFIXES KEY`, with the value and the mask as hex
//! bytes in memory order and the prefixes as hex bytes (48 for REX.W) or
//! `-`. At each step of the
//! unit in an executable section, the most specific matching pattern
//! decides (ties go to the stricter verdict): `never` is reported anywhere,
//! `gate` outside the allowed sections, `allow` nowhere.

use std::fmt::Write as _;

use crate::{Arch, Entry, Error, Pattern, Verdict};

pub fn write_list(arch: Arch, entries: &[Entry]) -> String {
    let mut out = format!("arch {} unit {}\n", arch.name(), arch.unit());
    for e in entries {
        let _ = writeln!(
            out,
            "{} {} {} {} {}",
            e.verdict.name(),
            hex(&e.pattern.value),
            hex(&e.pattern.mask),
            if e.pattern.prefixes.is_empty() {
                "-".to_owned()
            } else {
                hex(&e.pattern.prefixes)
            },
            e.key
        );
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, Error> {
    if !s.len().is_multiple_of(2) {
        return Err(Error(format!("odd hex {s}")));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| Error(format!("hex {s}"))))
        .collect()
}

pub fn read_list(text: &str) -> Result<(Arch, Vec<Entry>), Error> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
    let arch = match head.as_slice() {
        ["arch", a, "unit", _] => Arch::from_name(a).ok_or_else(|| Error(format!("arch {a}")))?,
        _ => return Err(Error("scan list without its header".into())),
    };
    let mut entries = Vec::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let w: Vec<&str> = line.splitn(5, ' ').collect();
        let [verdict, value, mask, prefix, key] = w.as_slice() else {
            return Err(Error(format!("scan list line {line}")));
        };
        let verdict = match *verdict {
            "allow" => Verdict::Allow,
            "gate" => Verdict::Gate,
            "never" => Verdict::Never,
            v => return Err(Error(format!("verdict {v}"))),
        };
        let prefixes = match *prefix {
            "-" => Vec::new(),
            p => unhex(p)?,
        };
        let (value, mask) = (unhex(value)?, unhex(mask)?);
        if value.len() != mask.len() || value.is_empty() {
            return Err(Error(format!("scan list line {line}")));
        }
        entries.push(Entry {
            verdict,
            pattern: Pattern {
                value,
                mask,
                prefixes,
            },
            key: (*key).to_owned(),
        });
    }
    Ok((arch, entries))
}

/// An executable section of an ELF file.
pub struct Section<'a> {
    pub name: String,
    pub bytes: &'a [u8],
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?)).ok()
}

/// The executable sections of a little-endian ELF64 object or image, after
/// checking that its machine is `arch`.
pub fn sections(file: &[u8], arch: Arch) -> Result<Vec<Section<'_>>, Error> {
    let bad = |what: &str| Error(format!("not a little-endian ELF64 file: {what}"));
    if file.get(..4) != Some(b"\x7fELF") || file.get(4) != Some(&2) || file.get(5) != Some(&1) {
        return Err(bad("header"));
    }
    let machine = u16_at(file, 0x12).ok_or_else(|| bad("machine"))?;
    let want = match arch {
        Arch::Aarch64 => 183,
        Arch::Riscv64 => 243,
        Arch::X86_64 => 62,
    };
    if machine != want {
        return Err(Error(format!(
            "ELF machine {machine} is not {}",
            arch.name()
        )));
    }
    let shoff = u64_at(file, 0x28).ok_or_else(|| bad("section table"))?;
    let shentsize = usize::from(u16_at(file, 0x3A).ok_or_else(|| bad("section table"))?);
    let shnum = usize::from(u16_at(file, 0x3C).ok_or_else(|| bad("section table"))?);
    let shstrndx = usize::from(u16_at(file, 0x3E).ok_or_else(|| bad("section table"))?);
    let header = |i: usize| shoff + i * shentsize;
    let strtab = u64_at(file, header(shstrndx) + 24).ok_or_else(|| bad("names"))?;
    let mut out = Vec::new();
    for i in 0..shnum {
        let h = header(i);
        let name_off = u32_at(file, h).ok_or_else(|| bad("section"))? as usize;
        let kind = u32_at(file, h + 4).ok_or_else(|| bad("section"))?;
        let flags = u64_at(file, h + 8).ok_or_else(|| bad("section"))?;
        let (off, size) = (u64_at(file, h + 24), u64_at(file, h + 32));
        const SHT_NOBITS: u32 = 8;
        const SHF_EXECINSTR: usize = 4;
        if kind == SHT_NOBITS || flags & SHF_EXECINSTR == 0 {
            continue;
        }
        let (off, size) = off.zip(size).ok_or_else(|| bad("section"))?;
        let bytes = file
            .get(off..off + size)
            .ok_or_else(|| bad("section past the end"))?;
        let name_bytes = file.get(strtab + name_off..).ok_or_else(|| bad("names"))?;
        let end = name_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(name_bytes.len());
        out.push(Section {
            name: String::from_utf8_lossy(&name_bytes[..end]).into_owned(),
            bytes,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub section: String,
    pub offset: usize,
    pub verdict: Verdict,
    pub key: String,
}

/// The pattern that decides at `at`, if any.
fn decide<'e>(entries: &'e [Entry], bytes: &[u8], at: usize) -> Option<&'e Entry> {
    let mut best: Option<&Entry> = None;
    for e in entries.iter().filter(|e| e.pattern.matches_at(bytes, at)) {
        let better = best.is_none_or(|b| {
            (e.pattern.specificity(), e.verdict) > (b.pattern.specificity(), b.verdict)
        });
        if better {
            best = Some(e);
        }
    }
    best
}

/// Scans the executable sections of `file`; `gate` names the sections where
/// gate-only encodings are allowed (the gate's and the start-up code's).
// ponytail: every pattern at every offset, linear; index the patterns by a
// fixed byte if images grow past a few MB.
pub fn scan(
    arch: Arch,
    entries: &[Entry],
    file: &[u8],
    gate: &[&str],
) -> Result<Vec<Finding>, Error> {
    let mut out = Vec::new();
    for s in sections(file, arch)? {
        let in_gate = gate.contains(&s.name.as_str());
        let mut at = 0;
        while at < s.bytes.len() {
            if let Some(e) = decide(entries, s.bytes, at) {
                let report = match e.verdict {
                    Verdict::Never => true,
                    Verdict::Gate => !in_gate,
                    Verdict::Allow => false,
                };
                if report {
                    out.push(Finding {
                        section: s.name.clone(),
                        offset: at,
                        verdict: e.verdict,
                        key: e.key.clone(),
                    });
                }
            }
            at += arch.unit();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{decide, read_list, write_list};
    use crate::{Arch, Entry, Pattern, Verdict};

    fn entry(verdict: Verdict, mask: u32, value: u32) -> Entry {
        Entry {
            verdict,
            pattern: Pattern::word(mask, value, 4),
            key: format!("k {value:x}"),
        }
    }

    #[test]
    fn the_most_specific_pattern_decides() {
        let list = [
            entry(Verdict::Never, 0xFFF0_0000, 0xD510_0000),
            entry(Verdict::Allow, 0xFFFF_FFE0, 0xD518_D080),
        ];
        let word = 0xD518_D081u32.to_le_bytes();
        assert_eq!(decide(&list, &word, 0).unwrap().verdict, Verdict::Allow);
        let other = 0xD518_2000u32.to_le_bytes();
        assert_eq!(decide(&list, &other, 0).unwrap().verdict, Verdict::Never);
    }

    #[test]
    fn a_tie_goes_to_the_stricter_verdict() {
        let list = [
            entry(Verdict::Allow, 0xFFFF_FFFF, 1),
            entry(Verdict::Gate, 0xFFFF_FFFF, 1),
        ];
        assert_eq!(
            decide(&list, &1u32.to_le_bytes(), 0).unwrap().verdict,
            Verdict::Gate
        );
    }

    #[test]
    fn a_list_reads_back_as_written() {
        let mut list = vec![entry(Verdict::Gate, 0xFFFF_FFE0, 0xD518_2000)];
        list.push(Entry {
            verdict: Verdict::Never,
            pattern: Pattern {
                value: vec![0x0F, 0x30],
                mask: vec![0xFF, 0xFF],
                prefixes: vec![0x66, 0x48],
            },
            key: "iclass X/Y Z".into(),
        });
        let (arch, back) = read_list(&write_list(Arch::X86_64, &list)).unwrap();
        assert_eq!(arch, Arch::X86_64);
        assert_eq!(back, list);
    }
}
