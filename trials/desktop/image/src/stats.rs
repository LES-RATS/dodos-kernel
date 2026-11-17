//! Statistics of a sample and the result lines (format in
//! `trials/desktop/README.md`, "Results").

use crate::tsc;

/// Sorts `v` and returns n, min, p50, p99, p99.9, max and the mean. A
/// quantile q is the value of rank ceil(q × n).
pub fn summary(v: &mut [u64]) -> [u64; 7] {
    if v.is_empty() {
        return [0; 7];
    }
    v.sort_unstable();
    let n = v.len();
    let q = |num: usize, den: usize| v[(n * num).div_ceil(den).max(1) - 1];
    let mean = v.iter().map(|&x| u128::from(x)).sum::<u128>() / n as u128;
    [
        n as u64,
        v[0],
        q(50, 100),
        q(99, 100),
        q(999, 1000),
        v[n - 1],
        mean as u64,
    ]
}

/// Prints `R <name> <keys> n= min= p50= p99= p999= max= mean= unit=ns` from
/// samples in TSC ticks, converted to nanoseconds with one decimal.
pub fn report_ticks(name: &str, keys: &str, v: &mut [u64]) {
    let s = summary(v);
    let ns = |t: u64| tsc::ticks_to_ns_f(t);
    crate::out!(
        "R {name} {keys} n={} min={:.1} p50={:.1} p99={:.1} p999={:.1} max={:.1} mean={:.1} unit=ns",
        s[0],
        ns(s[1]),
        ns(s[2]),
        ns(s[3]),
        ns(s[4]),
        ns(s[5]),
        ns(s[6])
    );
}

/// The same for samples already in a unit (`cycles`, or ns × 10 shown as
/// ns with one decimal when `tenths`).
pub fn report_unit(name: &str, keys: &str, v: &mut [u64], unit: &str) {
    let s = summary(v);
    crate::out!(
        "R {name} {keys} n={} min={} p50={} p99={} p999={} max={} mean={} unit={unit}",
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        s[6]
    );
}
