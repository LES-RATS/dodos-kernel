//! RISC-V from the RISC-V unified database (`riscv-unified-db`).
//!
//! Items, by kind:
//! - `csr NAME`: a CSR of `spec/std/isa/csr`. Its patterns are the six CSR
//!   instructions naming its address; the reads (`csrrs`/`csrrc` with `x0`
//!   as source, `csrrsi`/`csrrci` with 0) are allowed wherever the CSR is
//!   accessible, every other form takes the class.
//! - `insn NAME`: an instruction of `spec/std/isa/inst`, in its RV64
//!   encoding. The domain is every instruction of the SYSTEM (1110011) and
//!   MISC-MEM (0001111) major opcodes, every one the database says user mode
//!   cannot always execute, the shadow-stack instructions (Zicfiss), the
//!   compressed may-be-operations they alias (Zcmop) and `c.ebreak`.
//!
//! The files are YAML; the reader takes the few keys it needs from the
//! layout every file of the database has (two-space indentation, one key
//! per line), and fails on a file where a needed key is missing.

use std::path::Path;

use crate::{Access, Arch, Error, Field, Item, Pattern, Reg, RegEncoding, Rule, Spec};

pub fn load(dir: &Path) -> Result<Spec, Error> {
    let (csrs, insts) = texts(dir)?;
    from_texts(&csrs, &insts)
}

/// The CSR files and the instruction files of the database.
pub fn texts(dir: &Path) -> Result<(Vec<String>, Vec<String>), Error> {
    let isa = dir.join("spec/std/isa");
    let read_all = |sub: &str| -> Result<Vec<String>, Error> {
        yaml_files(&isa.join(sub))?
            .iter()
            .map(|f| crate::read(f))
            .collect()
    };
    Ok((read_all("csr")?, read_all("inst")?))
}

fn yaml_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| Error(format!("{}: {e}", d.display())))?;
        for entry in rd {
            let p = entry.map_err(|e| Error(e.to_string()))?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "yaml") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn from_texts(csrs: &[String], insts: &[String]) -> Result<Spec, Error> {
    let mut items = Vec::new();
    let mut regs = Vec::new();
    for text in csrs {
        let y = Yaml::new(text);
        let name = y
            .top("name")
            .ok_or_else(|| Error(format!("CSR without a name: {}", first_line(text))))?;
        let address = y
            .top("address")
            .and_then(parse_int)
            .ok_or_else(|| Error(format!("CSR {name} without an address")))?;
        let address = u16::try_from(address).map_err(|_| Error(format!("CSR {name}: address")))?;
        let rv32 = y
            .block("definedBy")
            .iter()
            .any(|l| l.trim() == "- xlen: 32" || l.trim() == "xlen: 32");
        let mode = y.top("priv_mode").unwrap_or("?");
        let note = format!(
            "priv_mode {mode}{}{}",
            if y.top("writable") == Some("true") {
                ", writable"
            } else {
                ", read-only"
            },
            if rv32 { ", RV32 only" } else { "" }
        );
        items.push(csr_item(name, address, note));
        if !rv32 {
            regs.push(Reg {
                name: name.to_owned(),
                key: format!("csr {name}"),
                encoding: RegEncoding::Csr(address),
                fields: csr_fields(&y),
            });
        }
    }
    for text in insts {
        let y = Yaml::new(text);
        let name = y
            .top("name")
            .ok_or_else(|| Error(format!("instruction without a name: {}", first_line(text))))?;
        let defined = y.block("definedBy");
        if defined
            .iter()
            .any(|l| l.trim() == "- xlen: 32" || l.trim() == "xlen: 32")
        {
            continue;
        }
        let enc = y.block("encoding");
        let rv64 = enc.iter().position(|l| l.trim() == "RV64:");
        let m = enc[rv64.unwrap_or(0)..]
            .iter()
            .find_map(|l| l.trim().strip_prefix("match:"))
            .map(|s| s.trim().trim_matches('"'))
            .ok_or_else(|| Error(format!("instruction {name} without an encoding")))?;
        let pattern =
            match_pattern(m).ok_or_else(|| Error(format!("instruction {name}: match {m}")))?;
        let user = y
            .block("access")
            .iter()
            .find_map(|l| l.trim().strip_prefix("u:"))
            .map(str::trim)
            .unwrap_or("always");
        let exts: Vec<&str> = defined
            .iter()
            .filter_map(|l| l.trim().trim_start_matches("- ").strip_prefix("name:"))
            .map(str::trim)
            .collect();
        let opcode = (m.len() == 32).then(|| &m[25..]);
        let domain = matches!(opcode, Some("1110011" | "0001111"))
            || user != "always"
            || exts.iter().any(|e| *e == "Zicfiss" || *e == "Zcmop")
            || name == "c.ebreak";
        items.push(Item {
            key: format!("insn {name}"),
            domain,
            rules: vec![Rule {
                pattern,
                access: Access::Write,
            }],
            note: format!("defined by {}, user access {user}", exts.join(" ")),
        });
    }
    Ok(Spec {
        arch: Arch::Riscv64,
        release: "RISC-V unified database (riscv-software-src/riscv-unified-db, revision pinned in flake.nix)".into(),
        notice: "Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries, and contributors to the RISC-V \
                 Unified DB. SPDX-License-Identifier: BSD-3-Clause-Clear"
            .into(),
        items,
        regs,
    })
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|l| !l.starts_with('#') && !l.trim().is_empty())
        .unwrap_or("")
}

fn parse_int(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// A `match` string, most significant bit first, `-` for a free bit.
pub fn match_pattern(m: &str) -> Option<Pattern> {
    if m.len() != 32 && m.len() != 16 {
        return None;
    }
    let (mut mask, mut value) = (0u32, 0u32);
    for c in m.chars() {
        mask <<= 1;
        value <<= 1;
        match c {
            '0' => mask |= 1,
            '1' => {
                mask |= 1;
                value |= 1;
            }
            '-' => {}
            _ => return None,
        }
    }
    Some(Pattern::word(mask, value, m.len() / 8))
}

/// The ten patterns of a CSR address: six forms that may write, four that
/// only read (the more specific patterns win).
pub fn csr_item(name: &str, address: u16, note: String) -> Item {
    let csr = u32::from(address) << 20;
    let mut rules = Vec::new();
    for funct3 in [1u32, 2, 3, 5, 6, 7] {
        rules.push(Rule {
            pattern: Pattern::word(0xFFF0_707F, csr | (funct3 << 12) | 0x73, 4),
            access: Access::Write,
        });
    }
    // csrrs, csrrc with rs1 = x0 and csrrsi, csrrci with uimm = 0 read only.
    for funct3 in [2u32, 3, 6, 7] {
        rules.push(Rule {
            pattern: Pattern::word(0xFFFF_F07F, csr | (funct3 << 12) | 0x73, 4),
            access: Access::Read,
        });
    }
    Item {
        key: format!("csr {name}"),
        domain: true,
        rules,
        note,
    }
}

/// An `extra` line: `csr NAME ADDRESS SOURCE...`, a CSR the database lacks;
/// a source that says `RV32 only` gives no register definition.
pub fn extra(spec: &Spec, words: &[&str]) -> Result<(Item, Option<Reg>), String> {
    if spec.arch != Arch::Riscv64 {
        return Err("extra encodings exist only for RISC-V".into());
    }
    match words {
        ["csr", name, address, source @ ..] if !source.is_empty() => {
            if spec.item(&format!("csr {name}")).is_some() {
                return Err(format!("csr {name}: extra, but the specification has it"));
            }
            let a = parse_int(address)
                .and_then(|a| u16::try_from(a).ok())
                .filter(|a| *a < 0x1000)
                .ok_or_else(|| format!("extra csr {name}: address {address}"))?;
            let clash = spec.items.iter().find(|i| {
                i.key.starts_with("csr ")
                    && i.rules[0].pattern == csr_item(name, a, String::new()).rules[0].pattern
            });
            if let Some(other) = clash {
                return Err(format!(
                    "extra csr {name}: address {address} is {}",
                    other.key
                ));
            }
            let source = source.join(" ");
            let reg = (!source.contains("RV32 only")).then(|| Reg {
                name: (*name).to_owned(),
                key: format!("csr {name}"),
                encoding: RegEncoding::Csr(a),
                fields: Vec::new(),
            });
            Ok((csr_item(name, a, format!("extra, {source}")), reg))
        }
        _ => Err("extra needs: csr NAME ADDRESS SOURCE".into()),
    }
}

fn csr_fields(y: &Yaml) -> Vec<Field> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for l in y.block("fields") {
        let indent = l.len() - l.trim_start().len();
        let t = l.trim();
        if indent == 2 && t.ends_with(':') {
            current = Some(t.trim_end_matches(':').to_owned());
        } else if indent == 4 {
            let loc = t
                .strip_prefix("location_rv64:")
                .or_else(|| t.strip_prefix("location:"));
            if let (Some(name), Some(loc)) = (&current, loc) {
                let loc = loc.trim();
                let (hi, lo) = loc.split_once('-').unwrap_or((loc, loc));
                if let (Ok(hi), Ok(lo)) = (hi.trim().parse::<u8>(), lo.trim().parse::<u8>())
                    && hi >= lo
                    && !out.iter().any(|f: &Field| &f.name == name)
                {
                    {
                        out.push(Field {
                            name: name.clone(),
                            lsb: lo,
                            width: hi - lo + 1,
                        });
                    }
                }
            }
        }
    }
    out
}

/// The top-level keys of a YAML file of the database, and their blocks.
struct Yaml<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Yaml<'a> {
    fn new(text: &'a str) -> Yaml<'a> {
        Yaml {
            lines: text
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect(),
        }
    }

    /// A top-level scalar: `key: value`.
    fn top(&self, key: &str) -> Option<&'a str> {
        self.lines.iter().find_map(|l| {
            let rest = l.strip_prefix(key)?.strip_prefix(':')?;
            let v = rest.trim().trim_matches('"');
            (!v.is_empty()).then_some(v)
        })
    }

    /// The indented lines under a top-level key.
    fn block(&self, key: &str) -> Vec<&'a str> {
        let head = format!("{key}:");
        let Some(start) = self
            .lines
            .iter()
            .position(|l| l.trim_end() == head || l.starts_with(&format!("{head} ")))
        else {
            return Vec::new();
        };
        self.lines[start + 1..]
            .iter()
            .take_while(|l| l.is_empty() || l.starts_with(' '))
            .copied()
            .collect()
    }
}
