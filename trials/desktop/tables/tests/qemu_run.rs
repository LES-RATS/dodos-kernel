//! The tables of the full QEMU run kept in `trials/desktop/qemu-results`:
//! every line parses, every step's measures get a table, and the run had
//! no failure.

use dodos_trial_desktop_tables::{parse_all, report};

const RUN: &str = include_str!("../../qemu-results/RESULTS.TXT");
const SHOOT14: &str = include_str!("../../qemu-results/RESULTS-SHOOT-14CORES.TXT");

#[test]
fn every_result_line_of_the_qemu_run_parses() {
    let r_lines = RUN.lines().filter(|l| l.starts_with("R ")).count();
    let parsed = parse_all(RUN).iter().filter(|l| l.kind == 'R').count();
    assert!(
        r_lines > 300,
        "the fixture is the full run: {r_lines} result lines"
    );
    assert_eq!(parsed, r_lines);
}

#[test]
fn the_report_of_the_qemu_run_has_every_step_and_no_failure() {
    let r = report(RUN);
    assert!(
        r.contains("Boots in these results: 4"),
        "four boots: three restarts of warm-reset"
    );
    for section in [
        "## Features by core (P-002)",
        "## Write-gate mechanisms, cost per operation (P-003)",
        "## Leaving a sleep state",
        "## debug.serial",
        "## stall",
        "## gate.pks",
        "## ipi.rtt",
        "## ipi.matrix",
        "## shoot",
        "## warm.reset",
        "## init.sipi.count",
        "## init.state.count",
        "## fence.init",
        "## bsp.park",
        "## bsp.rehearsal",
        "## The multicore trial on this machine",
        "## Suite a1t",
        "## Suite br",
    ] {
        assert!(r.contains(section), "missing {section}");
    }
    assert!(
        r.contains("## Failures and hangs\n\nNone."),
        "the QEMU run had failures"
    );
}

#[test]
fn the_fourteen_core_run_shoots_down_thirteen_others_with_the_bootstrap_core() {
    let lines = parse_all(SHOOT14);
    assert!(lines.iter().any(|l| l.name == "shoot"
        && l.get("others") == Some("13")
        && l.get("bsp_target") == Some("1")));
}
