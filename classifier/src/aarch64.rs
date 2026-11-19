//! ARM64 from Arm's AARCHMRS (the BSD-licensed A-profile package).
//!
//! Items, by kind:
//! - `reg NAME`: a system register, one item per name an accessor gives
//!   (`TTBR0_EL1`, `TTBR0_EL12`, `DBGBCR<m>_EL1` for an array), from its
//!   `MRS`, `MSR`, `MRRS` and `MSRR` accessors in `Registers.json`; writes
//!   take the class, reads are allowed when the register is accessible.
//! - `pstate NAME`: a target of `MSR (immediate)` (`DAIFSet`, `SVCRSM`).
//! - `sys FAMILY.NAME`: a system instruction accessor (`TLBI.VAE1IS`,
//!   `DC.ISW`, `GCSPUSHM`), encoded as the `SYS`, `SYSL` or `SYSP` form
//!   whose aliases in `Instructions.json` carry that family.
//! - `insn ID`: an A64 instruction of `Instructions.json`. The domain is the
//!   `control` group's system subgroups (exceptions, hints, barriers, PSTATE,
//!   system instructions and moves, register branches, extended hints); the
//!   other instructions may be classified by hand. The open package omits
//!   the pseudocode, so which load or store instruction is privileged is not
//!   in the data.

use std::collections::BTreeMap;
use std::path::Path;

use crate::json::{self, Json};
use crate::{Access, Arch, Error, Field, Item, Pattern, Reg, RegEncoding, Rule, Spec};

/// The subgroups of `control` that make the instruction domain.
const DOMAIN_GROUPS: [&str; 13] = [
    "exception",
    "systeminstrswithreg",
    "hints",
    "barriers",
    "pstate",
    "systeminstrs",
    "systemmove",
    "syspairinstrs",
    "systemmovepr",
    "branch_reg",
    "extendedhints",
    "tchange_reg",
    "tchange_imm",
];

/// Keys whose values the reader drops: access rules, conditions, prose.
const SKIP: [&str; 16] = [
    "access",
    "access_text",
    "assemble",
    "assembly_rules",
    "assertions",
    "condition",
    "configuration",
    "description",
    "display",
    "groups",
    "instances",
    "mapset",
    "operations",
    "purpose",
    "reset",
    "resets",
];

pub fn load(dir: &Path) -> Result<Spec, Error> {
    let (regs, insns) = parse(dir)?;
    from_json(&regs, &insns)
}

/// The two files of the package, as read (some keys dropped).
pub fn parse(dir: &Path) -> Result<(Json, Json), Error> {
    let text = crate::read(&dir.join("Registers.json"))?;
    let regs = json::parse(&text, &SKIP).map_err(|e| Error(format!("Registers.json: {e}")))?;
    drop(text);
    let text = crate::read(&dir.join("Instructions.json"))?;
    let insns = json::parse(&text, &SKIP).map_err(|e| Error(format!("Instructions.json: {e}")))?;
    Ok((regs, insns))
}

pub fn from_json(regs: &Json, insns: &Json) -> Result<Spec, Error> {
    let first = regs
        .arr()
        .first()
        .ok_or_else(|| Error("Registers.json is empty".into()))?;
    let meta = first.get("_meta");
    let release = format!(
        "Arm AARCHMRS {} (architecture {}, schema {})",
        meta.get("version").get("ref").str().unwrap_or("?"),
        meta.get("version").get("architecture").str().unwrap_or("?"),
        meta.get("version").get("schema").str().unwrap_or("?"),
    );
    let notice = format!(
        "{} {}",
        meta.get("license").get("copyright").str().unwrap_or(""),
        meta.get("license").get("info").str().unwrap_or("")
    );
    let a64 = insns
        .get("instructions")
        .arr()
        .iter()
        .find(|s| s.get("name").str() == Some("A64"))
        .ok_or_else(|| Error("Instructions.json has no A64".into()))?;
    let mut items = Vec::new();
    let mut families = BTreeMap::new();
    walk(a64, 0, 0, &[], &mut items, &mut families)?;
    let mut regs_out = Vec::new();
    registers(regs, &families, &mut items, &mut regs_out)?;
    for item in items.iter_mut().filter(|i| i.key.starts_with("reg ")) {
        if item.rules.iter().all(|r| r.access == Access::Read) {
            item.note.push_str(", read-only");
        }
    }
    Ok(Spec {
        arch: Arch::Aarch64,
        release,
        notice,
        items,
        regs: regs_out,
    })
}

/// The mask and value of a bit string such as `'01x1'`, most significant
/// bit first, placed at bit `lsb`.
pub fn bits(s: &str, lsb: u32) -> Result<(u32, u32), Error> {
    let body = s
        .strip_prefix('\'')
        .and_then(|b| b.strip_suffix('\''))
        .ok_or_else(|| Error(format!("bit string {s}")))?;
    let (mut m, mut v) = (0u32, 0u32);
    for (i, ch) in body.chars().rev().enumerate() {
        let bit = 1u32 << (lsb + i as u32);
        match ch {
            '0' => m |= bit,
            '1' => {
                m |= bit;
                v |= bit;
            }
            'x' => {}
            _ => return Err(Error(format!("bit string {s}"))),
        }
    }
    Ok((m, v))
}

fn encodeset(node: &Json) -> Result<(u32, u32), Error> {
    let (mut m, mut v) = (0, 0);
    for f in node.get("encoding").get("values").arr() {
        let lsb = f.get("range").get("start").int().unwrap_or(0) as u32;
        let s = f.get("value").get("value").str().unwrap_or("''");
        let (fm, fv) = bits(s, lsb)?;
        m |= fm;
        v |= fv;
    }
    Ok((m, v))
}

/// Walks the A64 decode tree; records instructions and the families of the
/// aliases of `SYS`, `SYSL` and `SYSP`. `path` is the group under the
/// instruction set and its subgroup, such as `control/exception`.
fn walk(
    node: &Json,
    mask: u32,
    value: u32,
    path: &[&str],
    items: &mut Vec<Item>,
    families: &mut BTreeMap<String, u32>,
) -> Result<(), Error> {
    let (m, v) = encodeset(node)?;
    let (mask, value) = (mask | m, value | v);
    let name = node.get("name").str().unwrap_or("");
    if node.get("_type").str() != Some("Instruction.Instruction") {
        let mut path = path.to_vec();
        if node.get("_type").str() == Some("Instruction.InstructionGroup") && path.len() < 2 {
            path.push(name);
        }
        for c in node.get("children").arr() {
            walk(c, mask, value, &path, items, families)?;
        }
        return Ok(());
    }
    let domain =
        path.first() == Some(&"control") && path.get(1).is_some_and(|g| DOMAIN_GROUPS.contains(g));
    items.push(Item {
        key: format!("insn {name}"),
        domain,
        rules: vec![Rule {
            pattern: Pattern::word(mask, value, 4),
            access: Access::Write,
        }],
        note: format!("group {}", path.join("/")),
    });
    if name.starts_with("SYS") {
        for alias in node.get("children").arr() {
            let literal = alias
                .get("assembly")
                .get("symbols")
                .arr()
                .iter()
                .find_map(|s| {
                    (s.get("_type").str() == Some("Instruction.Symbols.Literal"))
                        .then(|| s.get("value").str())
                        .flatten()
                });
            if let Some(fam) = literal {
                families.insert(fam.to_owned(), value);
            }
        }
    }
    Ok(())
}

/// The bits of one field of an accessor encoding for array index `n`.
/// Forms: `'0101'`; a concatenation `'011':p[3]`; an equation `m` with a
/// slice. A free variable (an implementation-defined name) is left open.
fn field(v: &Json, width: u32, index: Option<(&str, u64)>) -> Result<(u32, u32), Error> {
    let s = v.get("value").str().unwrap_or("");
    let take = |var: &str, hi: u32, lo: u32| -> Option<(u32, u32)> {
        let (iv, n) = index?;
        (iv == var).then(|| {
            let w = hi - lo + 1;
            ((1 << w) - 1, ((n >> lo) as u32) & ((1 << w) - 1))
        })
    };
    let full = (1u32 << width) - 1;
    match v.get("_type").str() {
        Some("Values.Value") => bits(s, 0),
        Some("Values.EquationValue") => {
            let r = &v.get("slice").arr()[0];
            let lo = r.get("start").int().unwrap_or(0) as u32;
            let w = r.get("width").int().unwrap_or(0) as u32;
            Ok(take(s, lo + w - 1, lo).map_or((0, 0), |(m, x)| (m & full, x & full)))
        }
        Some("Values.Group") => {
            let (mut m, mut x) = (0u32, 0u32);
            // Split on the colons outside brackets: `'011':p[3]`, `'1':m[1:0]`.
            let mut parts = Vec::new();
            let (mut depth, mut start) = (0, 0);
            for (i, ch) in s.char_indices() {
                match ch {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    ':' if depth == 0 => {
                        parts.push(&s[start..i]);
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            parts.push(&s[start..]);
            for part in parts {
                if part.starts_with('\'') {
                    let (pm, pv) = bits(part, 0)?;
                    let w = part.len() as u32 - 2;
                    m = (m << w) | pm;
                    x = (x << w) | pv;
                } else {
                    let (var, slice) = part
                        .split_once('[')
                        .ok_or_else(|| Error(format!("group {s}")))?;
                    let slice = slice.trim_end_matches(']');
                    let (hi, lo) = match slice.split_once(':') {
                        Some((h, l)) => (h.parse().unwrap_or(0), l.parse().unwrap_or(0)),
                        None => (slice.parse().unwrap_or(0), slice.parse().unwrap_or(0)),
                    };
                    let w = hi - lo + 1;
                    let (pm, pv) = take(var, hi, lo).unwrap_or((0, 0));
                    m = (m << w) | pm;
                    x = (x << w) | pv;
                }
            }
            Ok((m, x))
        }
        _ => Err(Error(format!("encoding value {v:?}"))),
    }
}

/// The base word of each accessor kind: SYS-like ones come from the aliases.
fn base(kind: &str, families: &BTreeMap<String, u32>) -> Option<(u32, Access, &'static str)> {
    Some(match kind {
        "MRS" => (0xD520_0000, Access::Read, "reg"),
        "MSRregister" => (0xD500_0000, Access::Write, "reg"),
        "MRRS" => (0xD560_0000, Access::Read, "reg"),
        "MSRRregister" => (0xD540_0000, Access::Write, "reg"),
        "MSRimmediate" => (0xD500_401F, Access::Write, "pstate"),
        "SYS" => (0xD508_0000, Access::Write, "sys"),
        "SYSL" => (0xD528_0000, Access::Write, "sys"),
        "SYSP" => (0xD548_0000, Access::Write, "sys"),
        fam => (*families.get(fam)?, Access::Write, "sys"),
    })
}

fn registers(
    regs: &Json,
    families: &BTreeMap<String, u32>,
    items: &mut Vec<Item>,
    out: &mut Vec<Reg>,
) -> Result<(), Error> {
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for r in regs.arr() {
        if r.get("state").str() != Some("AArch64") {
            continue;
        }
        let fields = fields_of(r);
        for acc in r.get("accessors").arr() {
            let kind = acc
                .get("name")
                .str()
                .unwrap_or("")
                .trim_start_matches("A64.");
            let (base_word, access, kind_key) = base(kind, families)
                .ok_or_else(|| Error(format!("accessor A64.{kind}: no SYS alias carries it")))?;
            let var = acc.get("index_variable").str();
            let range = acc.get("indexes").arr().first();
            let indices: Vec<u64> = match (var, range) {
                (Some(_), Some(rg)) => {
                    let s = rg.get("start").int().unwrap_or(0) as u64;
                    let w = rg.get("width").int().unwrap_or(0) as u64;
                    (s..s + w).collect()
                }
                _ => vec![0],
            };
            for enc in acc.get("encoding").arr() {
                let asm = enc.get("asmvalue").str();
                let name = match (kind_key, asm) {
                    ("sys", Some(a)) => format!("{kind}.{a}"),
                    ("sys", None) => kind.to_owned(),
                    (_, Some(a)) => a.to_owned(),
                    (_, None) => return Err(Error(format!("{kind} accessor without a name"))),
                };
                let key = format!("{kind_key} {name}");
                let e = enc.get("encodings");
                let mut rules = Vec::new();
                for &n in &indices {
                    let idx = var.map(|v| (v, n));
                    let f = |k: &str, w: u32| -> Result<(u32, u32), Error> {
                        match e.get(k) {
                            Json::Null => Ok((0, 0)),
                            v => field(v, w, idx),
                        }
                    };
                    let (m0, v0) = f("op0", 2)?;
                    let (m1, v1) = f("op1", 3)?;
                    let (mn, vn) = f("CRn", 4)?;
                    let (mm, vm) = f("CRm", 4)?;
                    let (m2, v2) = f("op2", 3)?;
                    let (mask, value) = if kind_key == "pstate" {
                        (
                            0xFFF8_F01F | (m1 << 16) | (mm << 8) | (m2 << 5),
                            base_word | (v1 << 16) | (vm << 8) | (v2 << 5),
                        )
                    } else {
                        // op0 sits in bits 20:19; bit 20 is fixed by the base
                        // word for registers (op0 is 2 or 3) and for SYS (op0 1
                        // is bits 20:19 = 01).
                        (
                            0xFFE0_0000
                                | (m0 << 19)
                                | (m1 << 16)
                                | (mn << 12)
                                | (mm << 8)
                                | (m2 << 5),
                            (base_word & !(3 << 19))
                                | (v0 << 19)
                                | (v1 << 16)
                                | (vn << 12)
                                | (vm << 8)
                                | (v2 << 5),
                        )
                    };
                    rules.push(Rule {
                        pattern: Pattern::word(mask, value, 4),
                        access,
                    });
                    let fixed = m0 == 3 && m1 == 7 && mn == 15 && mm == 15 && m2 == 7;
                    if kind_key == "reg" && fixed {
                        let rname = match var {
                            Some(v) => name.replace(&format!("<{v}>"), &n.to_string()),
                            None => name.clone(),
                        };
                        if !out.iter().any(|x: &Reg| x.name == rname) {
                            out.push(Reg {
                                name: rname,
                                key: key.clone(),
                                encoding: RegEncoding::Sys {
                                    op0: v0 as u8,
                                    op1: v1 as u8,
                                    crn: vn as u8,
                                    crm: vm as u8,
                                    op2: v2 as u8,
                                },
                                fields: fields.clone(),
                            });
                        }
                    }
                }
                match index.get(&key) {
                    Some(&i) => {
                        for rule in rules {
                            if !items[i].rules.contains(&rule) {
                                items[i].rules.push(rule);
                            }
                        }
                    }
                    None => {
                        index.insert(key.clone(), items.len());
                        items.push(Item {
                            key,
                            domain: true,
                            rules,
                            note: format!("register {}", r.get("name").str().unwrap_or("")),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// The plain fields of a register with one contiguous range each; a field
/// whose range differs between the register's layouts is left out.
fn fields_of(r: &Json) -> Vec<Field> {
    let mut seen: BTreeMap<String, Option<(u8, u8)>> = BTreeMap::new();
    for fs in r.get("fieldsets").arr() {
        for f in fs.get("values").arr() {
            if f.get("_type").str() != Some("Fields.Field") {
                continue;
            }
            let (Some(name), [range]) = (f.get("name").str(), f.get("rangeset").arr()) else {
                continue;
            };
            let lsb = range.get("start").int().unwrap_or(0) as u8;
            let width = range.get("width").int().unwrap_or(0) as u8;
            let e = seen.entry(name.to_owned()).or_insert(Some((lsb, width)));
            if *e != Some((lsb, width)) {
                *e = None;
            }
        }
    }
    seen.into_iter()
        .filter_map(|(name, r)| r.map(|(lsb, width)| Field { name, lsb, width }))
        .collect()
}
