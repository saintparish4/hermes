//! Refusing to publish a batch that looks like the node lying.
//!
//! The store already refuses the one lie it can recognize row by row: a proxy whose code
//! vanished. A node that answers zero words for every slot of a live proxy is subtler. Each
//! row looks like an honest reclassification, "transparent" becoming "not upgradeable", and
//! each one on its own is possible. A batch where a large share of known proxies does it at
//! once is not a chain event, it is an endpoint failing, and publishing it would report live
//! upgrade authorities as safe. Stale but honest beats fresh and wrong.

use crate::classify::ProxyKind;
use crate::store::ProxyRecord;
use std::collections::HashSet;

/// The share of previously covered proxies in one batch that may stop being covered before the
/// batch is refused. Real transitions (an admin renouncing, a proxy re-pointed at nothing) are
/// rare; a fifth of a batch at once is not a real transition.
pub const MAX_LOSS: f64 = 0.2;

/// Below this many previously covered proxies in a batch, one legitimate change is too large a
/// share to judge by. The row-level guards still apply.
pub const MIN_SAMPLE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canary {
    /// Rows in the batch that were covered proxies before this run.
    pub known: usize,
    /// How many of those now come back as something that is not a covered proxy.
    pub lost: usize,
}

impl Canary {
    pub fn tripped(self) -> bool {
        self.known >= MIN_SAMPLE && (self.lost as f64) > MAX_LOSS * self.known as f64
    }
}

/// Compare a batch against what was covered before the run. `covered_before` holds lowercased
/// addresses.
pub fn check(covered_before: &HashSet<String>, batch: &[ProxyRecord]) -> Canary {
    let mut canary = Canary { known: 0, lost: 0 };
    for r in batch {
        if !covered_before.contains(&r.address.to_ascii_lowercase()) {
            continue;
        }
        canary.known += 1;
        if !ProxyKind::is_covered_str(&r.kind) {
            canary.lost += 1;
        }
    }
    canary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: usize, kind: &str) -> ProxyRecord {
        ProxyRecord {
            address: format!("0x{i:040X}"),
            kind: kind.into(),
            ..Default::default()
        }
    }

    fn known(n: usize) -> HashSet<String> {
        (0..n).map(|i| format!("0x{i:040x}")).collect()
    }

    #[test]
    fn a_batch_where_a_quarter_of_known_proxies_go_blank_is_refused() {
        let batch: Vec<_> = (0..20)
            .map(|i| {
                row(
                    i,
                    if i < 5 {
                        "not_upgradeable"
                    } else {
                        "transparent"
                    },
                )
            })
            .collect();
        let c = check(&known(20), &batch);
        assert_eq!(c, Canary { known: 20, lost: 5 });
        assert!(c.tripped());
    }

    /// One admin renouncing is a real transition and has to land.
    #[test]
    fn a_single_real_transition_is_published() {
        let batch: Vec<_> = (0..20)
            .map(|i| row(i, if i == 0 { "not_upgradeable" } else { "uups" }))
            .collect();
        assert!(!check(&known(20), &batch).tripped());
    }

    /// Newly discovered addresses have nothing to lose, so a batch of them never trips it.
    #[test]
    fn only_previously_covered_rows_count() {
        let batch: Vec<_> = (0..20).map(|i| row(i, "not_upgradeable")).collect();
        assert_eq!(check(&HashSet::new(), &batch), Canary { known: 0, lost: 0 });
        assert!(!check(&HashSet::new(), &batch).tripped());
    }

    #[test]
    fn a_sample_too_small_to_judge_is_left_to_the_row_guards() {
        let batch: Vec<_> = (0..4).map(|i| row(i, "eoa")).collect();
        assert!(!check(&known(4), &batch).tripped());
    }

    #[test]
    fn every_covered_kind_counts_as_still_covered() {
        let batch: Vec<_> = ["transparent", "uups", "beacon", "eip1822", "admin_only"]
            .iter()
            .enumerate()
            .map(|(i, k)| row(i, k))
            .collect();
        assert_eq!(check(&known(5), &batch).lost, 0);
    }
}
