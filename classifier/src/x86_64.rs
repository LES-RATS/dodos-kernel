//! x86-64 from Intel XED's instruction data (`intelxed/xed`, `datafiles`).
//!
//! Items have the kind `iclass`: XED's instruction class, followed by
//! `/IFORM` for a record that names its form (so `RDMSR` and its immediate
//! form `RDMSR/RDMSR_GPR64u64_IMM32` are apart), and by `.write` or `.read`
//! for a record without a form in a class whose records differ in writing a
//! system register (`MOV_CR.write`, `MOV_CR.read`).
//!
//! The data files read are the decoder's: every file that a `*.cfg` of
//! `datafiles` names as `dec-instructions`. The domain is every record that
//! XED marks for ring 0 (`CPL 0` or attribute `RING0`), that writes a
//! system register (a control or debug register, the MSRs, XCR0, the
//! descriptor tables, TR, the shadow-stack pointer, the FS, GS and kernel GS
//! bases, the user interrupt flag), or whose category is a system one
//! (`SYSTEM_CATEGORIES`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{Access, Arch, Error, Item, Pattern, Rule, Spec};

const SYSTEM_REGS: [&str; 11] = [
    "CR0",
    "MSRS",
    "XCR0",
    "GDTR",
    "IDTR",
    "LDTR",
    "TR",
    "SSP",
    "FSBASE",
    "GSBASE",
    "IA32_KERNEL_GS_BASE",
];

const SYSTEM_CATEGORIES: [&str; 22] = [
    "SYSTEM",
    "IO",
    "IOSTRINGOP",
    "SYSCALL",
    "SYSRET",
    "INTERRUPT",
    "VTX",
    "SGX",
    "CET",
    "UINTR",
    "XSAVE",
    "KEYLOCKER",
    "KEYLOCKER_WIDE",
    "FRED",
    "LKGS",
    "PCONFIG",
    "SMAP",
    "RDWRFSGS",
    "MSRLIST",
    "WRMSRNS",
    "USER_MSR",
    "HRESET",
];

/// The start of the note of an item whose XED pattern was not understood.
pub const NOT_UNDERSTOOD: &str = "pattern not understood:";

/// The legacy prefixes and REX bytes that may stand before an opcode.
fn is_prefix(b: u8) -> bool {
    matches!(
        b,
        0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3 | 0x40..=0x4F
    )
}

/// REX with W set, as a required prefix: the byte just before the opcode.
pub const REX_W: u8 = 0x48;

/// Whether the prefix `p` stands before `at`: REX.W as the last prefix, a
/// legacy prefix anywhere among the prefixes just before (at most 14, the
/// longest an instruction allows).
pub fn prefix_before(bytes: &[u8], at: usize, p: u8) -> bool {
    if p == REX_W {
        return at > 0 && bytes[at - 1] & 0xF8 == REX_W;
    }
    bytes[at.saturating_sub(14)..at]
        .iter()
        .rev()
        .take_while(|b| is_prefix(**b))
        .any(|b| *b == p)
}

pub fn load(dir: &Path) -> Result<Spec, Error> {
    from_texts(&texts(dir)?)
}

/// The decoder's instruction files: those a `*.cfg` names as
/// `dec-instructions`.
pub fn texts(dir: &Path) -> Result<Vec<String>, Error> {
    let data = dir.join("datafiles");
    let mut files = BTreeSet::new();
    for cfg in walk(&data, "cfg")? {
        for line in crate::read(&cfg)?.lines() {
            let line = line.split('#').next().unwrap_or("");
            if let Some((k, v)) = line.split_once(':')
                && k.trim() == "dec-instructions"
            {
                files.insert(cfg.parent().unwrap_or(&data).join(v.trim()));
            }
        }
    }
    files.iter().map(|f| crate::read(f)).collect()
}

fn walk(dir: &Path, ext: &str) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| Error(format!("{}: {e}", d.display())))? {
            let p = e.map_err(|e| Error(e.to_string()))?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// One pattern of a record with its operands and form.
#[derive(Debug, Default, Clone)]
struct Form {
    iclass: String,
    cpl0: bool,
    category: String,
    pattern: String,
    operands: String,
    iform: Option<String>,
}

fn records(text: &str) -> Vec<Form> {
    let mut out = Vec::new();
    for block in text.split('{').skip(1) {
        let body = block.split('}').next().unwrap_or("");
        let mut common = Form::default();
        let mut forms: Vec<Form> = Vec::new();
        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("");
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "ICLASS" => common.iclass = v.to_owned(),
                "CPL" => common.cpl0 |= v == "0",
                "ATTRIBUTES" => common.cpl0 |= v.split_whitespace().any(|a| a == "RING0"),
                "CATEGORY" => common.category = v.to_owned(),
                "PATTERN" => forms.push(Form {
                    pattern: v.to_owned(),
                    ..Form::default()
                }),
                "OPERANDS" => {
                    if let Some(f) = forms.last_mut() {
                        f.operands = v.to_owned();
                    }
                }
                "IFORM" => {
                    if let Some(f) = forms.last_mut() {
                        f.iform = Some(v.to_owned());
                    }
                }
                _ => {}
            }
        }
        if common.iclass.is_empty() {
            continue;
        }
        for f in forms {
            out.push(Form {
                iclass: common.iclass.clone(),
                cpl0: common.cpl0,
                category: common.category.clone(),
                ..f
            });
        }
    }
    out
}

fn writes_system(operands: &str) -> bool {
    operands.split_whitespace().any(|op| {
        let Some((reg, rest)) = op
            .split_once('=')
            .map(|(_, r)| r)
            .and_then(|r| r.split_once(':'))
        else {
            return false;
        };
        let written = rest.split(':').next().unwrap_or("").contains('w');
        let sys = reg
            .strip_prefix("XED_REG_")
            .is_some_and(|r| SYSTEM_REGS.contains(&r))
            || reg == "CR_R()"
            || reg == "DR_R()";
        written && sys
    })
}

pub fn from_texts(texts: &[String]) -> Result<Spec, Error> {
    let forms: Vec<Form> = texts.iter().flat_map(|t| records(t)).collect();
    // The classes with forms that differ in writing a system register.
    let mut mixed: BTreeMap<&str, BTreeSet<bool>> = BTreeMap::new();
    for f in forms.iter().filter(|f| f.iform.is_none()) {
        mixed
            .entry(&f.iclass)
            .or_default()
            .insert(writes_system(&f.operands));
    }
    let mut items: Vec<Item> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for f in &forms {
        let key = match &f.iform {
            Some(form) => format!("iclass {}/{form}", f.iclass),
            None if mixed.get(f.iclass.as_str()).is_some_and(|s| s.len() == 2) => {
                let dir = if writes_system(&f.operands) {
                    "write"
                } else {
                    "read"
                };
                format!("iclass {}.{dir}", f.iclass)
            }
            None => format!("iclass {}", f.iclass),
        };
        let domain = f.cpl0
            || writes_system(&f.operands)
            || SYSTEM_CATEGORIES.contains(&f.category.as_str());
        // A token not understood stops the load in the domain; outside it the
        // item keeps no pattern, and the check refuses to classify it as
        // anything but `kernel`.
        let (patterns, note) = match translate(&f.pattern) {
            Ok(p) => (p, String::new()),
            Err(t) if domain => {
                return Err(Error(format!(
                    "{key}: XED pattern token {t} not understood"
                )));
            }
            Err(t) => (Vec::new(), format!("{NOT_UNDERSTOOD} {t}; ")),
        };
        let rules: Vec<Rule> = patterns
            .into_iter()
            .map(|pattern| Rule {
                pattern,
                access: Access::Write,
            })
            .collect();
        match index.get(&key) {
            Some(&i) => {
                items[i].domain |= domain;
                for r in rules {
                    if !items[i].rules.contains(&r) {
                        items[i].rules.push(r);
                    }
                }
            }
            None => {
                index.insert(key.clone(), items.len());
                items.push(Item {
                    key,
                    domain,
                    rules,
                    note: format!(
                        "{note}category {}{}",
                        f.category,
                        if f.cpl0 { ", ring 0" } else { "" }
                    ),
                });
            }
        }
    }
    Ok(Spec {
        arch: Arch::X86_64,
        release: "Intel XED instruction data (intelxed/xed, revision pinned in flake.nix)".into(),
        notice: "Copyright Intel Corporation. Licensed under the Apache License, Version 2.0"
            .into(),
        items,
        regs: Vec::new(),
    })
}

/// VEX or EVEX state of a pattern being translated.
#[derive(Default)]
struct Vex {
    evex: bool,
    map: Option<u8>,
    pp: u8,
}

/// Translates one XED decoder pattern into byte patterns. The kernel is
/// 64-bit code, so a pattern valid only outside 64-bit mode gives none.
/// What a byte pattern cannot say (a prefix that must be absent, the width
/// of operands) is left open: the pattern matches more than
/// the instruction, never less. `MOD!=3` becomes three patterns.
pub fn translate(pattern: &str) -> Result<Vec<Pattern>, String> {
    let mut opcode: Vec<u8> = Vec::new();
    let mut pending: Option<(u8, u32)> = None; // high bits of a byte and their count
    let (mut modrm_m, mut modrm_v, mut has_modrm) = (0u8, 0u8, false);
    let mut mods: Vec<u8> = Vec::new();
    let mut prefixes: Vec<u8> = Vec::new();
    let mut vex: Option<Vex> = None;
    for tok in pattern.split_whitespace() {
        let field = |t: &str, name: &str| -> Option<Option<u8>> {
            let inner = t.strip_prefix(name)?.strip_prefix('[')?.strip_suffix(']')?;
            Some(
                inner
                    .strip_prefix("0b")
                    .map(|b| u8::from_str_radix(&b.replace('_', ""), 2).unwrap_or(0)),
            )
        };
        if let Some(h) = tok.strip_prefix("0x") {
            opcode.push(u8::from_str_radix(h, 16).map_err(|_| tok.to_owned())?);
        } else if let Some(b) = tok.strip_prefix("0b") {
            let b = b.replace('_', "");
            pending = Some((
                u8::from_str_radix(&b, 2).map_err(|_| tok.to_owned())?,
                b.len() as u32,
            ));
        } else if let Some(v) = field(tok, "SRM") {
            let (hi, n) = pending.take().ok_or_else(|| tok.to_owned())?;
            match v {
                Some(low) => opcode.push((hi << (8 - n)) | low),
                // A register in the low bits: one byte per register.
                None => {
                    return Ok((0..8u8)
                        .map(|r| byte_pattern(&[(hi << (8 - n)) | r], None, &prefixes, &vex))
                        .collect());
                }
            }
        } else if let Some(v) = field(tok, "MOD") {
            has_modrm = true;
            if let Some(m) = v {
                modrm_m |= 0xC0;
                modrm_v |= m << 6;
            }
        } else if let Some(v) = field(tok, "REG") {
            has_modrm = true;
            if let Some(r) = v {
                modrm_m |= 0x38;
                modrm_v |= r << 3;
            }
        } else if let Some(v) = field(tok, "RM") {
            has_modrm = true;
            if let Some(r) = v {
                modrm_m |= 0x07;
                modrm_v |= r;
            }
        } else {
            match tok {
                "not64" | "mode16" | "mode32" => return Ok(Vec::new()),
                "MOD=3" => {
                    modrm_m |= 0xC0;
                    modrm_v |= 0xC0;
                }
                "MOD!=3" => mods = vec![0x00, 0x40, 0x80],
                "f3_refining_prefix" | "refining_f3" | "repe" => prefixes.push(0xF3),
                "f2_refining_prefix" | "repne" => prefixes.push(0xF2),
                "66_prefix" | "osz_refining_prefix" => prefixes.push(0x66),
                "rexw_prefix" => prefixes.push(REX_W),
                "VV1" => vex = Some(Vex::default()),
                "EVV" => {
                    vex = Some(Vex {
                        evex: true,
                        ..Vex::default()
                    })
                }
                "VNP" | "V66" | "VF3" | "VF2" => {
                    let v = vex.as_mut().ok_or_else(|| tok.to_owned())?;
                    v.pp = match tok {
                        "VNP" => 0,
                        "V66" => 1,
                        "VF3" => 2,
                        _ => 3,
                    };
                }
                "V0F" | "V0F38" | "V0F3A" | "MAP4" | "MAP5" | "MAP6" | "MAP7" => {
                    let v = vex.as_mut().ok_or_else(|| tok.to_owned())?;
                    v.map = Some(match tok {
                        "V0F" => 1,
                        "V0F38" => 2,
                        "V0F3A" => 3,
                        t => t[3..].parse().unwrap_or(0),
                    });
                }
                t if open_token(t) => {}
                t => return Err(t.to_owned()),
            }
        }
    }
    if pending.is_some() || opcode.is_empty() {
        return Err(format!("incomplete pattern {pattern}"));
    }
    let modrms: Vec<Option<(u8, u8)>> = match (has_modrm, mods.is_empty()) {
        (false, _) => vec![None],
        (true, true) => vec![Some((modrm_m, modrm_v))],
        (true, false) => mods
            .iter()
            .map(|m| Some((modrm_m | 0xC0, (modrm_v & 0x3F) | m)))
            .collect(),
    };
    Ok(modrms
        .into_iter()
        .map(|m| byte_pattern(&opcode, m, &prefixes, &vex))
        .collect())
}

/// Tokens that constrain nothing a byte pattern can hold: modes, absent
/// prefixes, operand widths, immediates, displacements, EVEX details.
fn open_token(t: &str) -> bool {
    t.ends_with("()")
        || t.contains("=")
        || t.starts_with("no")
        || t.starts_with("VL")
        || t.starts_with("SCC")
        || t.starts_with("NO_SCC")
        || matches!(
            t,
            "mode64"
                | "eamode16"
                | "eamode32"
                | "eamode64"
                | "W0"
                | "W1"
                | "EVEX"
                | "rex2_refining_prefix"
                | "NOEVSR"
                | "NOVSR"
                | "eanot16"
                | "lock_prefix"
                | "SIB"
        )
}

fn byte_pattern(
    opcode: &[u8],
    modrm: Option<(u8, u8)>,
    prefixes: &[u8],
    vex: &Option<Vex>,
) -> Pattern {
    let mut value = Vec::new();
    let mut mask = Vec::new();
    if let Some(v) = vex {
        let map = v.map.unwrap_or(1);
        if v.evex {
            // 62, then R X B R' 0 m m m, then W v v v v 1 p p, then z L'L b V' a a a.
            value.extend([0x62, map & 0x07, v.pp, 0]);
            mask.extend([0xFF, 0x07, 0x03, 0]);
        } else {
            // C4, then R X B m m m m m, then W v v v v L p p.
            value.extend([0xC4, map & 0x1F, v.pp]);
            mask.extend([0xFF, 0x1F, 0x03]);
        }
    }
    for b in opcode {
        value.push(*b);
        mask.push(0xFF);
    }
    if let Some((m, v)) = modrm {
        value.push(v);
        mask.push(m);
    }
    Pattern {
        value,
        mask,
        // A VEX or EVEX prefix replaces the legacy ones.
        prefixes: if vex.is_some() {
            Vec::new()
        } else {
            prefixes.to_vec()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn legacy_pattern_with_fixed_modrm() {
        let p =
            translate("0x0F 0x01 MOD[0b11] MOD=3 REG[0b010] RM[0b001] no_refining_prefix").unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].value, [0x0F, 0x01, 0xD1]);
        assert_eq!(p[0].mask, [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn memory_form_gives_three_mod_values() {
        let p = translate("0x0F 0x01 MOD[mm] MOD!=3 REG[0b110] RM[nnn] MODRM()").unwrap();
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|x| x.mask == [0xFF, 0xFF, 0xF8]));
    }

    #[test]
    fn modes_other_than_64_bit_give_no_pattern() {
        assert!(translate("0x9D mode32 66_prefix").unwrap().is_empty());
    }

    #[test]
    fn unknown_tokens_are_refused() {
        assert!(translate("0x0F FROBNICATE").is_err());
    }
}
