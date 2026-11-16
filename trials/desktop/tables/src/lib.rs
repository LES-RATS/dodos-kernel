//! Reads the result lines of the desktop image (`\DODOS\RESULTS.TXT`, one
//! or several boots) and writes Markdown: one table per measure, the
//! failures, the trial's own tables, and the derived tables the documents
//! ask for (features by kind, gate cost per operation, sleep-state exit).
//! The line format is in `trials/desktop/README.md`, "Results".

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One line of the results.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// `R` result, `I` information, `F` failure, `T` trial table.
    pub kind: char,
    pub name: String,
    pub keys: Vec<(String, String)>,
    /// The words that are not `key=value`.
    pub words: Vec<String>,
}

impl Line {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    pub fn num(&self, key: &str) -> Option<f64> {
        self.get(key)?.parse().ok()
    }
}

/// Splits on spaces, keeping `key="a b"` together and dropping the quotes.
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parses one line; `None` for a line that is not a result line.
pub fn parse(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut chars = line.chars();
    let kind = chars.next()?;
    if !matches!(kind, 'R' | 'I' | 'F' | 'T') || chars.next() != Some(' ') {
        return None;
    }
    let rest = &line[2..];
    if kind == 'T' {
        return Some(Line {
            kind,
            name: rest.to_string(),
            keys: Vec::new(),
            words: Vec::new(),
        });
    }
    let mut t = tokens(rest).into_iter();
    let name = t.next()?;
    let (mut keys, mut words) = (Vec::new(), Vec::new());
    for w in t {
        match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => keys.push((k.to_string(), v.to_string())),
            _ => words.push(w),
        }
    }
    Some(Line {
        kind,
        name,
        keys,
        words,
    })
}

pub fn parse_all(text: &str) -> Vec<Line> {
    text.lines().filter_map(parse).collect()
}

const STATS: [&str; 8] = ["n", "min", "p50", "p99", "p999", "max", "mean", "unit"];

/// The table of one measure: every key column in first-seen order, the
/// statistics last.
fn table(lines: &[&Line]) -> String {
    let mut cols: Vec<String> = Vec::new();
    for l in lines {
        for (k, _) in &l.keys {
            if !STATS.contains(&k.as_str()) && !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    let stats: Vec<&str> = STATS
        .iter()
        .copied()
        .filter(|s| lines.iter().any(|l| l.get(s).is_some()))
        .collect();
    let mut t = String::from("|");
    for c in cols.iter().map(String::as_str).chain(stats.iter().copied()) {
        let _ = write!(t, " {c} |");
    }
    t.push_str("\n|");
    t.push_str(&"---|".repeat(cols.len() + stats.len()));
    for l in lines {
        t.push_str("\n|");
        for c in cols.iter().map(String::as_str).chain(stats.iter().copied()) {
            let _ = write!(t, " {} |", l.get(c).unwrap_or(""));
        }
    }
    t.push('\n');
    t
}

/// The features of the cores, and whether performance and efficiency
/// cores differ in any of them (P-002).
fn features(lines: &[Line]) -> String {
    let f: Vec<&Line> = lines
        .iter()
        .filter(|l| l.kind == 'I' && l.name == "features")
        .collect();
    if f.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Features by core (P-002)\n\n");
    out.push_str(&table(&f));
    let mut by_kind: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut differ = Vec::new();
    for l in &f {
        let kind = l.get("kind").unwrap_or("?").to_string();
        let entry = by_kind.entry(kind).or_default();
        for (k, v) in &l.keys {
            if matches!(
                k.as_str(),
                "core" | "apic" | "kind" | "sig" | "native_model"
            ) {
                continue;
            }
            match entry.get(k) {
                Some(old) if old != v && !differ.contains(k) => differ.push(k.clone()),
                None => {
                    entry.insert(k.clone(), v.clone());
                }
                _ => {}
            }
        }
    }
    let kinds: Vec<&String> = by_kind.keys().collect();
    if kinds.len() > 1 {
        let (a, b) = (&by_kind[kinds[0]], &by_kind[kinds[1]]);
        for (k, v) in a {
            if b.get(k).is_some_and(|w| w != v) && !differ.contains(k) {
                differ.push(k.clone());
            }
        }
    }
    let _ = writeln!(
        out,
        "\nFeatures that differ between cores: {}\n",
        if differ.is_empty() {
            "none".to_string()
        } else {
            differ.join(", ")
        }
    );
    out
}

/// Gate costs per operation: the median of a sample divided by `ops`.
fn per_op(lines: &[Line]) -> String {
    let g: Vec<&Line> = lines
        .iter()
        .filter(|l| {
            l.kind == 'R'
                && (l.name.starts_with("gate.") || l.name.starts_with("cet."))
                && l.get("ops").is_some()
        })
        .collect();
    if g.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Write-gate mechanisms, cost per operation (P-003)\n\n| measure | kind | variant | clock | p50 per op | p99 per op | unit |\n|---|---|---|---|---|---|---|\n",
    );
    for l in g {
        let ops = l.num("ops").unwrap_or(1.0);
        let variant: Vec<String> = l
            .keys
            .iter()
            .filter(|(k, _)| {
                matches!(
                    k.as_str(),
                    "pcid" | "noflush" | "write" | "shstk" | "ibt" | "type"
                )
            })
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.2} | {:.2} | {} |",
            l.name,
            l.get("kind").unwrap_or(""),
            variant.join(" "),
            l.get("clock").unwrap_or("tsc"),
            l.num("p50").unwrap_or(0.0) / ops,
            l.num("p99").unwrap_or(0.0) / ops,
            l.get("unit").unwrap_or("")
        );
    }
    out.push('\n');
    out
}

/// The time to leave each sleep state: its round trip minus the round
/// trip to the same core spinning.
fn sleep_exit(lines: &[Line]) -> String {
    let rtt: Vec<&Line> = lines
        .iter()
        .filter(|l| l.kind == 'R' && l.name == "ipi.rtt")
        .collect();
    let spin = |from: &str, to: &str| {
        rtt.iter()
            .find(|l| {
                l.get("from") == Some(from)
                    && l.get("to") == Some(to)
                    && l.get("target") == Some("spin")
            })
            .and_then(|l| l.num("p50"))
    };
    let mut rows = String::new();
    for l in &rtt {
        let target = l.get("target").unwrap_or("");
        if target == "spin" {
            continue;
        }
        let (from, to) = (l.get("from").unwrap_or(""), l.get("to").unwrap_or(""));
        if let (Some(base), Some(p50)) = (spin(from, to), l.num("p50")) {
            let _ = writeln!(
                rows,
                "| {from}→{to} | {}→{} | {target} | {p50:.1} | {base:.1} | {:.1} | {} |",
                l.get("from_kind").unwrap_or(""),
                l.get("to_kind").unwrap_or(""),
                p50 - base,
                l.get("c6_residency").unwrap_or("")
            );
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    format!(
        "## Leaving a sleep state (round trip minus the spinning one)\n\n| pair | kinds | target state | p50 rtt ns | p50 spinning ns | exit ns | C6 residency change |\n|---|---|---|---|---|---|---|\n{rows}\n"
    )
}

/// The whole report.
pub fn report(text: &str) -> String {
    let lines = parse_all(text);
    let mut out = String::from("# DodOS desktop measurements\n\n");
    let boots: Vec<&Line> = lines
        .iter()
        .filter(|l| l.kind == 'I' && l.name == "image")
        .collect();
    let _ = writeln!(out, "Boots in these results: {}\n", boots.len());
    if let Some(c) = lines.iter().find(|l| l.kind == 'I' && l.name == "cpu") {
        let _ = writeln!(
            out,
            "Processor: {}\n",
            c.keys
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    out.push_str(&features(&lines));
    out.push_str(&per_op(&lines));
    out.push_str(&sleep_exit(&lines));
    let mut names: Vec<&str> = Vec::new();
    for l in lines.iter().filter(|l| l.kind == 'R') {
        if !names.contains(&l.name.as_str()) {
            names.push(&l.name);
        }
    }
    for n in names {
        let group: Vec<&Line> = lines
            .iter()
            .filter(|l| l.kind == 'R' && l.name == n)
            .collect();
        let _ = writeln!(out, "## {n}\n");
        out.push_str(&table(&group));
        out.push('\n');
    }
    let fails: Vec<&Line> = lines.iter().filter(|l| l.kind == 'F').collect();
    out.push_str("## Failures and hangs\n\n");
    if fails.is_empty() {
        out.push_str("None.\n");
    }
    for f in fails {
        let kv: Vec<String> = f.keys.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = writeln!(out, "- `{}` {} {}", f.name, f.words.join(" "), kv.join(" "));
    }
    let trial: Vec<&Line> = lines.iter().filter(|l| l.kind == 'T').collect();
    if !trial.is_empty() {
        out.push_str("\n## The multicore trial on this machine\n\n");
        for t in trial {
            out.push_str(&t.name);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{parse, report, tokens};

    #[test]
    fn quoted_values_stay_one_token() {
        assert_eq!(tokens(r#"a=1 b="x y" c"#), ["a=1", "b=x y", "c"]);
    }

    #[test]
    fn a_result_line_gives_its_keys_and_words() {
        let l = parse("R ipi.rtt from=2 to=4 n=10 p50=5.0 unit=ns").expect("line");
        assert_eq!((l.kind, l.name.as_str()), ('R', "ipi.rtt"));
        assert_eq!(l.get("to"), Some("4"));
        assert_eq!(l.num("p50"), Some(5.0));
        let f = parse("F trial hung in an earlier boot").expect("line");
        assert_eq!(f.words, ["hung", "in", "an", "earlier", "boot"]);
        assert!(parse("DodOS desktop measurements").is_none());
    }

    #[test]
    fn gate_cost_is_divided_by_ops() {
        let r = report(
            "R gate.store core=2 kind=P ops=64 n=5 min=1 p50=128.0 p99=256.0 p999=256 max=256 mean=128 unit=ns\n",
        );
        assert!(
            r.contains("| gate.store | P |  | tsc | 2.00 | 4.00 | ns |"),
            "{r}"
        );
    }

    #[test]
    fn sleep_exit_subtracts_the_spinning_round_trip() {
        let r = report(
            "R ipi.rtt from=2 to=4 from_kind=P to_kind=P target=spin n=1 p50=100.0 unit=ns\n\
             R ipi.rtt from=2 to=4 from_kind=P to_kind=P target=hlt n=1 p50=350.0 unit=ns\n",
        );
        assert!(
            r.contains("| 2→4 | P→P | hlt | 350.0 | 100.0 | 250.0 |"),
            "{r}"
        );
    }

    #[test]
    fn features_that_differ_between_kinds_are_named() {
        let r = report(
            "I features core=0 kind=P pks=1 fred=0\nI features core=8 kind=E pks=0 fred=0\n",
        );
        assert!(r.contains("Features that differ between cores: pks"), "{r}");
    }
}
