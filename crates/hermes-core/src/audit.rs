//! Drawing an audit set and saying how sure a rate measured on it is.
//!
//! A table of hand-verified rows says the code is right on those rows. It says nothing about how
//! often it is wrong on the rest. That needs rows nobody chose, drawn at random within strata so a
//! rare kind is not left out by chance, and a rate that carries its interval, because a rate
//! from twelve rows and one from four hundred must not read the same.
//!
//! Pure: deterministic for a seed, no I/O.

use serde::Serialize;
use std::collections::BTreeMap;

/// A small deterministic generator, so an audit drawn with a seed can be drawn again.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Up to `per_stratum` items from each stratum, drawn without replacement, in a fixed order for
/// a fixed seed. Returns indices into `strata`, grouped by stratum name.
///
/// Equal allocation on purpose: the strata are kinds of authority, and the small ones are where
/// a rule nobody tested tends to live. The overall rate weights each stratum back by its size.
pub fn stratified(
    strata: &[String],
    per_stratum: usize,
    seed: u64,
) -> BTreeMap<String, Vec<usize>> {
    let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in strata.iter().enumerate() {
        by.entry(s.clone()).or_default().push(i);
    }
    let mut rng = XorShift(seed.max(1));
    for members in by.values_mut() {
        for i in (1..members.len()).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            members.swap(i, j);
        }
        members.truncate(per_stratum);
    }
    by
}

/// A proportion and its 95% Wilson score interval.
///
/// Wilson rather than the textbook `p ± 1.96·sqrt(p(1-p)/n)`, which gives a zero-width interval
/// at 0 or n successes: twelve confirmations out of twelve would read as certainty.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rate {
    pub successes: u64,
    pub trials: u64,
    pub estimate: Option<f64>,
    pub low: Option<f64>,
    pub high: Option<f64>,
}

const Z95: f64 = 1.959_963_984_540_054;

pub fn wilson(successes: u64, trials: u64) -> Rate {
    if trials == 0 {
        return Rate {
            successes,
            trials,
            estimate: None,
            low: None,
            high: None,
        };
    }
    let n = trials as f64;
    let p = successes as f64 / n;
    let z2 = Z95 * Z95;
    let centre = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let half = Z95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / (1.0 + z2 / n);
    Rate {
        successes,
        trials,
        estimate: Some(p),
        low: Some((centre - half).max(0.0)),
        high: Some((centre + half).min(1.0)),
    }
}

/// One stratum's share of the frame and what its sample found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StratumResult {
    /// Items in the frame (the population the sample was drawn from).
    pub frame: u64,
    pub successes: u64,
    pub trials: u64,
}

/// The frame-wide proportion, each stratum weighted by its share of the frame, with a normal
/// interval from the stratified variance and the finite-population correction. `None` when some
/// stratum in the frame has no trials, since its rate would then be a guess.
pub fn weighted(strata: &[StratumResult]) -> Option<(f64, f64, f64)> {
    let total: u64 = strata.iter().map(|s| s.frame).sum();
    if total == 0 || strata.iter().any(|s| s.frame > 0 && s.trials == 0) {
        return None;
    }
    let total = total as f64;
    let (mut p, mut var) = (0.0, 0.0);
    for s in strata.iter().filter(|s| s.frame > 0) {
        let w = s.frame as f64 / total;
        let n = s.trials as f64;
        let ph = s.successes as f64 / n;
        let fpc = if s.frame > 1 {
            (s.frame as f64 - n) / (s.frame as f64 - 1.0)
        } else {
            0.0
        };
        p += w * ph;
        var += w * w * ph * (1.0 - ph) / n * fpc.max(0.0);
    }
    let half = Z95 * var.sqrt();
    Some((p, (p - half).max(0.0), (p + half).min(1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_draws_the_same_sample_and_another_seed_need_not() {
        let strata: Vec<String> = (0..40).map(|i| format!("s{}", i % 3)).collect();
        let a = stratified(&strata, 5, 7);
        assert_eq!(a, stratified(&strata, 5, 7));
        assert_ne!(a, stratified(&strata, 5, 8));
        assert!(a.values().all(|v| v.len() == 5));
        for (name, picks) in &a {
            assert!(picks.iter().all(|&i| &strata[i] == name));
            let mut distinct = picks.clone();
            distinct.dedup();
            assert_eq!(distinct.len(), picks.len(), "without replacement");
        }
    }

    #[test]
    fn a_stratum_smaller_than_the_allocation_is_taken_whole() {
        let strata = vec!["big".to_string(); 10]
            .into_iter()
            .chain(["rare".to_string()])
            .collect::<Vec<_>>();
        let s = stratified(&strata, 4, 1);
        assert_eq!(s["rare"], vec![10]);
        assert_eq!(s["big"].len(), 4);
    }

    /// The reason for Wilson: all successes is not certainty.
    #[test]
    fn twelve_of_twelve_is_not_a_certainty() {
        let r = wilson(12, 12);
        assert_eq!(r.estimate, Some(1.0));
        let low = r.low.unwrap();
        assert!(low < 0.76 && low > 0.75, "{low}");
        assert_eq!(r.high, Some(1.0));
        assert_eq!(wilson(0, 0).estimate, None);
    }

    #[test]
    fn the_wilson_interval_matches_a_known_value() {
        // 8 of 10: Wilson 95% is about [0.490, 0.943].
        let r = wilson(8, 10);
        assert!((r.low.unwrap() - 0.4902).abs() < 1e-3);
        assert!((r.high.unwrap() - 0.9433).abs() < 1e-3);
    }

    #[test]
    fn a_stratum_with_no_trials_leaves_the_overall_rate_unknown() {
        let s = [
            StratumResult {
                frame: 100,
                successes: 9,
                trials: 10,
            },
            StratumResult {
                frame: 5,
                successes: 0,
                trials: 0,
            },
        ];
        assert_eq!(weighted(&s), None);
    }

    #[test]
    fn strata_are_weighted_by_their_share_of_the_frame() {
        let s = [
            StratumResult {
                frame: 90,
                successes: 10,
                trials: 10,
            },
            StratumResult {
                frame: 10,
                successes: 0,
                trials: 10,
            },
        ];
        let (p, low, high) = weighted(&s).unwrap();
        assert!((p - 0.9).abs() < 1e-9);
        assert!(low <= p && p <= high);
        let whole = [StratumResult {
            frame: 10,
            successes: 5,
            trials: 10,
        }];
        let (_, low, high) = weighted(&whole).unwrap();
        assert_eq!((low, high), (0.5, 0.5), "a census has no sampling error");
    }
}
