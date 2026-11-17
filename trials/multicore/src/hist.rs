//! Latency histogram: exact below 64 ns, then 64 buckets per power of two
//! (under 1.6 % of error), so recording is one increment and no sample is
//! dropped.

use alloc::boxed::Box;
use alloc::vec;

const SUB: u64 = 64;
const BUCKETS: usize = 64 + 58 * 64;

pub struct Hist {
    counts: Box<[u64]>,
    pub n: u64,
    pub max: u64,
}

impl Default for Hist {
    fn default() -> Hist {
        Hist {
            counts: vec![0; BUCKETS].into_boxed_slice(),
            n: 0,
            max: 0,
        }
    }
}

fn bucket(v: u64) -> usize {
    if v < SUB {
        return v as usize;
    }
    let e = 63 - v.leading_zeros() as u64; // 6 or more
    (SUB + (e - 6) * SUB + ((v >> (e - 6)) - SUB)) as usize
}

/// The smallest value of a bucket.
fn floor(b: usize) -> u64 {
    let b = b as u64;
    if b < SUB {
        return b;
    }
    let e = (b - SUB) / SUB + 6;
    (SUB + (b - SUB) % SUB) << (e - 6)
}

impl Hist {
    pub fn record(&mut self, ns: u64) {
        self.counts[bucket(ns).min(BUCKETS - 1)] += 1;
        self.n += 1;
        self.max = self.max.max(ns);
    }

    pub fn merge(&mut self, o: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(o.counts.iter()) {
            *a += b;
        }
        self.n += o.n;
        self.max = self.max.max(o.max);
    }

    /// The value at quantile `q` (0..=1), in ns; 0 if empty.
    pub fn quantile(&self, q: f64) -> u64 {
        // The ceiling of n × q, without `f64::ceil` (not in `core`).
        let x = self.n as f64 * q;
        let t = x as u64;
        let rank = if (t as f64) < x { t + 1 } else { t }.max(1);
        let mut seen = 0;
        for (b, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= rank {
                return floor(b);
            }
        }
        0
    }

    /// Samples at or above `ns`.
    pub fn above(&self, ns: u64) -> u64 {
        self.counts[bucket(ns)..].iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{Hist, bucket, floor};

    #[test]
    fn bucket_floor_is_within_two_percent() {
        for v in (1..2_000_000u64).step_by(997).chain([63, 64, 65, 127, 128]) {
            let f = floor(bucket(v));
            assert!(f <= v && (v - f) * 64 <= v, "{v} -> {f}");
        }
    }

    #[test]
    fn quantiles_of_a_uniform_ramp() {
        let mut h = Hist::default();
        for v in 1..=1000 {
            h.record(v);
        }
        assert_eq!(h.quantile(0.5), 500); // the bucket [500, 504)
        assert_eq!(h.quantile(0.01), 10);
        assert!(h.quantile(0.999) >= 984);
        assert_eq!(h.max, 1000);
        assert_eq!(h.above(992), 9); // buckets of 8 from 512 on: 992..=1000
    }
}
