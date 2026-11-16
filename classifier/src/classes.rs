//! The classification file of an architecture, and its check.
//!
//! One line per encoding: `<kind> <name> <class> [<reason>]`, with `#`
//! starting a comment (at the start of a line or after a space). The reason names a `why` line, which quotes the
//! sentence of the contract that justifies the class; every class but
//! `kernel` needs one. A `extra` line adds an encoding that the
//! specification lacks (RISC-V only: the CSRs the unified database misses,
//! classified by hand from the ratified text, D-125).
//!
//! The check fails on: an encoding of the domain without a line; a line
//! naming an encoding the specification lacks; two lines for one encoding;
//! a class without its reason; a reason never defined or never used; an
//! `extra` encoding that the specification has after all.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Error, Spec, riscv64};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Used by the kernel, harmless to other nodes: allowed anywhere.
    Kernel,
    /// Changes translation, interrupt routing, debug state, the vector
    /// length or the protection the gate relies on, or reaches firmware:
    /// only in the gate and the start-up code.
    Gate,
    /// Traps or is undefined at the kernel's level: refused anyway.
    Unreachable,
    /// Not used by the kernel: refused.
    Unused,
    /// The owners must decide: refused everywhere until they do, and an
    /// image that holds it fails the scan.
    Undecided,
}

pub const CLASSES: [Class; 5] = [
    Class::Kernel,
    Class::Gate,
    Class::Unreachable,
    Class::Unused,
    Class::Undecided,
];

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Kernel => "kernel",
            Class::Gate => "gate",
            Class::Unreachable => "unreachable",
            Class::Unused => "unused",
            Class::Undecided => "undecided",
        }
    }

    fn parse(s: &str) -> Option<Class> {
        CLASSES.into_iter().find(|c| c.name() == s)
    }
}

#[derive(Debug, Clone)]
pub struct Line {
    pub class: Class,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Classification {
    pub lines: BTreeMap<String, Line>,
    /// Reason id to the quoted sentence and its source.
    pub reasons: BTreeMap<String, String>,
}

impl Classification {
    pub fn class_of(&self, key: &str) -> Option<Class> {
        self.lines.get(key).map(|l| l.class)
    }

    /// Checks `text` against `spec`, adding its `extra` encodings to it.
    pub fn check(spec: &mut Spec, text: &str) -> Result<Classification, Error> {
        let mut c = Classification::default();
        let mut errors = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            // A comment starts at a `#` that begins the line or follows a space,
            // so that `kernel.md#anchor` stays whole.
            let end = raw
                .char_indices()
                .find(|&(i, c)| c == '#' && (i == 0 || raw[..i].ends_with(char::is_whitespace)))
                .map_or(raw.len(), |(i, _)| i);
            let line = raw[..end].trim();
            if line.is_empty() {
                continue;
            }
            if let Err(e) = c.add(spec, line) {
                errors.push(format!("line {}: {e}", n + 1));
            }
        }
        let known: BTreeSet<&str> = spec.items.iter().map(|i| i.key.as_str()).collect();
        for (key, line) in &c.lines {
            if !known.contains(key.as_str()) {
                errors.push(format!("{key}: not in the specification"));
            }
            match &line.reason {
                None if line.class != Class::Kernel => {
                    errors.push(format!("{key}: class {} needs a reason", line.class.name()));
                }
                Some(r) if !c.reasons.contains_key(r) => {
                    errors.push(format!("{key}: reason {r} is not defined"));
                }
                _ => {}
            }
        }
        for item in &spec.items {
            let refused = c.class_of(&item.key).is_some_and(|k| k != Class::Kernel);
            if refused && item.note.starts_with(crate::x86_64::NOT_UNDERSTOOD) {
                errors.push(format!(
                    "{}: refused, but its pattern was not understood ({})",
                    item.key, item.note
                ));
            }
        }
        let used: BTreeSet<&str> = c
            .lines
            .values()
            .filter_map(|l| l.reason.as_deref())
            .collect();
        for r in c.reasons.keys() {
            if !used.contains(r.as_str()) {
                errors.push(format!("reason {r} is never used"));
            }
        }
        for item in spec.items.iter().filter(|i| i.domain) {
            if !c.lines.contains_key(&item.key) {
                errors.push(format!("{}: not classified ({})", item.key, item.note));
            }
        }
        if errors.is_empty() {
            Ok(c)
        } else {
            Err(Error(format!(
                "{}: {} classification errors\n{}",
                spec.arch.name(),
                errors.len(),
                errors.join("\n")
            )))
        }
    }

    fn add(&mut self, spec: &mut Spec, line: &str) -> Result<(), String> {
        let mut words = line.split_whitespace();
        let kind = words.next().unwrap_or("");
        match kind {
            "why" => {
                let id = words.next().ok_or("why without an id")?;
                let rest: Vec<&str> = words.collect();
                if rest.is_empty() {
                    return Err(format!("why {id} quotes nothing"));
                }
                if self.reasons.insert(id.to_owned(), rest.join(" ")).is_some() {
                    return Err(format!("why {id} defined twice"));
                }
                Ok(())
            }
            "extra" => {
                let (item, reg) = riscv64::extra(spec, words.collect::<Vec<_>>().as_slice())?;
                spec.items.push(item);
                spec.regs.extend(reg);
                Ok(())
            }
            _ => {
                let name = words.next().ok_or("a line needs a kind and a name")?;
                let class = words.next().ok_or("a line needs a class")?;
                let class = Class::parse(class).ok_or_else(|| format!("unknown class {class}"))?;
                let reason = words.next().map(str::to_owned);
                if let Some(extra) = words.next() {
                    return Err(format!("unexpected {extra}"));
                }
                let key = format!("{kind} {name}");
                if self
                    .lines
                    .insert(key.clone(), Line { class, reason })
                    .is_some()
                {
                    return Err(format!("{key} classified twice"));
                }
                Ok(())
            }
        }
    }

    /// Counts per class, for the report.
    pub fn counts(&self) -> BTreeMap<Class, usize> {
        let mut m = BTreeMap::new();
        for l in self.lines.values() {
            *m.entry(l.class).or_insert(0) += 1;
        }
        m
    }
}
