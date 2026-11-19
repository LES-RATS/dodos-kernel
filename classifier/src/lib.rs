//! The classifier of rule G11 (`kernel-interface.md`, "The gate over page
//! tables", and "Architectures", D-125).
//!
//! It reads a pinned snapshot of each architecture's machine-readable
//! specification (Arm's AARCHMRS, `riscv-unified-db`, Intel XED) and the
//! classification file of the architecture, checks that every encoding of
//! the specification's domain has exactly one class, and produces the scan
//! lists of the binary check and the register definitions for Rust and Zig.

#![forbid(unsafe_code)]

pub mod aarch64;
pub mod classes;
pub mod json;
pub mod regs;
pub mod riscv64;
pub mod scan;
pub mod x86_64;

use std::fmt;
use std::path::PathBuf;

pub use classes::{Class, Classification};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Aarch64,
    Riscv64,
    X86_64,
}

pub const ARCHES: [Arch; 3] = [Arch::Aarch64, Arch::Riscv64, Arch::X86_64];

impl Arch {
    pub fn name(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::Riscv64 => "riscv64",
            Arch::X86_64 => "x86_64",
        }
    }

    pub fn from_name(name: &str) -> Option<Arch> {
        ARCHES.into_iter().find(|a| a.name() == name)
    }

    /// The step of the scan in bytes (G11): every 4-byte word on ARM64,
    /// every 2-byte offset on RISC-V (compressed instructions), every byte
    /// on x86-64.
    pub fn unit(self) -> usize {
        match self {
            Arch::Aarch64 => 4,
            Arch::Riscv64 => 2,
            Arch::X86_64 => 1,
        }
    }

    /// The environment variable that names the specification's directory,
    /// set by the Nix development shell (`flake.nix`).
    pub fn env(self) -> &'static str {
        match self {
            Arch::Aarch64 => "DODOS_SPEC_AARCH64",
            Arch::Riscv64 => "DODOS_SPEC_RISCV64",
            Arch::X86_64 => "DODOS_SPEC_X86_64",
        }
    }

    /// The classification file of the repository.
    pub fn classes(self) -> &'static str {
        match self {
            Arch::Aarch64 => include_str!("../classes/aarch64.txt"),
            Arch::Riscv64 => include_str!("../classes/riscv64.txt"),
            Arch::X86_64 => include_str!("../classes/x86_64.txt"),
        }
    }

    pub fn spec_dir(self) -> Result<PathBuf, Error> {
        std::env::var_os(self.env())
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error(format!(
                    "{} is not set: run inside `nix develop`",
                    self.env()
                ))
            })
    }

    /// Reads the pinned specification of the architecture.
    pub fn load(self) -> Result<Spec, Error> {
        let dir = self.spec_dir()?;
        match self {
            Arch::Aarch64 => aarch64::load(&dir),
            Arch::Riscv64 => riscv64::load(&dir),
            Arch::X86_64 => x86_64::load(&dir),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub fn read(path: &std::path::Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {e}", path.display())))
}

/// A byte pattern in memory order: a word matches when `word & mask ==
/// value` byte by byte. On x86-64 a pattern may also need prefixes just
/// before it: a mandatory prefix (66, F2, F3) among them, or a REX prefix
/// with W set (written 48) as the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub value: Vec<u8>,
    pub mask: Vec<u8>,
    pub prefixes: Vec<u8>,
}

impl Pattern {
    /// A little-endian instruction word of `bytes` bytes (ARM64, RISC-V).
    pub fn word(mask: u32, value: u32, bytes: usize) -> Pattern {
        Pattern {
            value: value.to_le_bytes()[..bytes].to_vec(),
            mask: mask.to_le_bytes()[..bytes].to_vec(),
            prefixes: Vec::new(),
        }
    }

    /// Fixed bits; the most specific matching pattern decides.
    pub fn specificity(&self) -> u32 {
        let bits: u32 = self.mask.iter().map(|m| m.count_ones()).sum();
        bits + 8 * self.prefixes.len() as u32
    }

    pub fn matches_at(&self, bytes: &[u8], at: usize) -> bool {
        let Some(window) = bytes.get(at..at + self.value.len()) else {
            return false;
        };
        let body = window
            .iter()
            .zip(&self.mask)
            .zip(&self.value)
            .all(|((b, m), v)| b & m == *v);
        body && self
            .prefixes
            .iter()
            .all(|p| x86_64::prefix_before(bytes, at, *p))
    }
}

/// What a class governs for one pattern of an encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Writes a register, or executes an instruction: the class decides.
    Write,
    /// Only reads a register: allowed wherever the register is accessible
    /// (class `kernel` or `gate`), refused for the other classes.
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub pattern: Pattern,
    pub access: Access,
}

/// One encoding of the specification, the unit that gets one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The key used by the classification file: a kind and a name.
    pub key: String,
    /// In the domain that must be classified; items outside it may be
    /// classified (by hand, when they matter) but need not be.
    pub domain: bool,
    pub rules: Vec<Rule>,
    /// What the specification says about the item, for review: the
    /// privilege level of a CSR, XED's category, Arm's group.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegEncoding {
    Sys {
        op0: u8,
        op1: u8,
        crn: u8,
        crm: u8,
        op2: u8,
    },
    Csr(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub lsb: u8,
    pub width: u8,
}

/// A register for the generated definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reg {
    pub name: String,
    /// The key of its item in the classification.
    pub key: String,
    pub encoding: RegEncoding,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub arch: Arch,
    /// The release of the specification and its licence notice, printed in
    /// the generated files.
    pub release: String,
    pub notice: String,
    pub items: Vec<Item>,
    pub regs: Vec<Reg>,
}

impl Spec {
    pub fn item(&self, key: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.key == key)
    }
}

/// The verdict of the scan for a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Allow,
    /// Allowed only in the gate's sections (and the start-up code).
    Gate,
    /// Refused everywhere: `unreachable`, `unused` and `undecided`.
    Never,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Gate => "gate",
            Verdict::Never => "never",
        }
    }

    pub fn of(class: Class, access: Access) -> Verdict {
        match (class, access) {
            (Class::Kernel, _) | (Class::Gate, Access::Read) => Verdict::Allow,
            (Class::Gate, Access::Write) => Verdict::Gate,
            _ => Verdict::Never,
        }
    }
}

/// One line of a scan list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub verdict: Verdict,
    pub pattern: Pattern,
    pub key: String,
}

/// The scan list of a checked classification, sorted by key.
pub fn scan_list(spec: &Spec, classes: &Classification) -> Vec<Entry> {
    let mut out = Vec::new();
    for item in &spec.items {
        let Some(class) = classes.class_of(&item.key) else {
            continue;
        };
        for rule in &item.rules {
            out.push(Entry {
                verdict: Verdict::of(class, rule.access),
                pattern: rule.pattern.clone(),
                key: item.key.clone(),
            });
        }
    }
    out
}

/// Loads, checks and lists one architecture: the build's entry point.
pub fn build(arch: Arch) -> Result<(Spec, Classification, Vec<Entry>), Error> {
    let mut spec = arch.load()?;
    let classes = Classification::check(&mut spec, arch.classes())?;
    let list = scan_list(&spec, &classes);
    Ok((spec, classes, list))
}
