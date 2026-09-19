//! One scan, end to end: read the slots, classify, then walk every distinct admin to its root.
//!
//! This lives here rather than in the CLI so that the code path the deployment runs is the
//! same one the fixture tests replay. A test that re-assembled the pipeline by hand would
//! prove the pieces work, not that they are wired together the way production wires them.

use crate::probe::{ProbeOutcome, Scanner};
use crate::resolve::AuthorityScanner;
use alloy::primitives::Address;
use hermes_core::store::checksum;
use hermes_core::{Confidence, Node, ProxyRecord, resolve};
use std::collections::HashSet;

/// One address to scan, and the hand-written label to store with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub address: Address,
    pub label: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanCounts {
    pub ok: usize,
    pub failed: usize,
    /// Addresses whose first read looked empty and were read again.
    pub rereads: usize,
    /// Empty reads a second read never confirmed. Skipped, not stored.
    pub unconfirmed: usize,
    pub resolved: usize,
}

pub struct Scanned {
    pub records: Vec<ProxyRecord>,
    pub counts: ScanCounts,
}

/// Scan `targets` and resolve what they found.
pub async fn scan_and_resolve(
    scanner: &Scanner,
    authorities: &AuthorityScanner,
    targets: &[Target],
    scanned_at: i64,
) -> Scanned {
    let results = scanner
        .scan(targets.iter().map(|t| t.address).collect())
        .await;
    let mut counts = ScanCounts::default();
    let mut records: Vec<ProxyRecord> = results
        .iter()
        .filter_map(|(addr, outcome)| record(*addr, outcome, targets, scanned_at, &mut counts))
        .collect();
    counts.resolved = resolve_authorities(authorities, &mut records).await;
    Scanned { records, counts }
}

/// The row one probe earns, or `None` when it earned nothing publishable.
fn record(
    addr: Address,
    outcome: &anyhow::Result<ProbeOutcome>,
    targets: &[Target],
    scanned_at: i64,
    counts: &mut ScanCounts,
) -> Option<ProxyRecord> {
    let o = match outcome {
        Ok(o) => o,
        Err(e) => {
            counts.failed += 1;
            skipped(addr, &format!("probe failed: {e}"));
            return None;
        }
    };
    if o.needed_reread() {
        counts.rereads += 1;
    }
    // An unconfirmed empty read would classify as `NotUpgradeable` or `Eoa`, overwrite a
    // stored proxy, and report a live upgrade authority as safe. Leaving the previous row
    // untouched and stale is the better of the two wrong answers available here.
    let Some(c) = o.verdict() else {
        counts.unconfirmed += 1;
        skipped(
            addr,
            "empty read never confirmed; keeping any previous verdict",
        );
        return None;
    };
    counts.ok += 1;
    Some(ProxyRecord {
        address: checksum(addr),
        label: targets
            .iter()
            .find(|t| t.address == addr)
            .and_then(|t| t.label.clone()),
        kind: c.kind.as_str().to_string(),
        implementation: c.implementation.map(checksum),
        admin: c.admin.map(checksum),
        beacon: c.beacon.map(checksum),
        code_size: o.code_size as i64,
        scanned_at,
        // Filled in by the resolution pass.
        ..Default::default()
    })
}

fn skipped(addr: Address, why: &str) {
    tracing::warn!(%addr, why, "address skipped");
}

/// Walk every distinct admin to its root and write the answer back onto each proxy.
///
/// Done as a second pass rather than inline so the probe collection can be batched across
/// every admin at once. Admins are shared heavily — one ProxyAdmin governs twenty contracts
/// on Base — so resolving per proxy would re-walk the same subgraph twenty times.
async fn resolve_authorities(scanner: &AuthorityScanner, records: &mut [ProxyRecord]) -> usize {
    let admins: Vec<Node> = records
        .iter()
        .filter_map(|r| r.admin.as_ref()?.parse().ok().map(Node::base))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if admins.is_empty() {
        return 0;
    }

    tracing::info!(count = admins.len(), "resolving authorities");
    let probes = scanner.collect(admins).await;
    let mut resolved = 0;

    for record in records.iter_mut() {
        let Some(admin) = record.admin.as_ref().and_then(|a| a.parse().ok()) else {
            continue;
        };
        let r = resolve(Node::base(admin), &probes);
        // An unresolved chain leaves the columns untouched, so the store keeps whatever it
        // already knew instead of being told the authority disappeared.
        if r.confidence == Confidence::Unknown {
            continue;
        }
        record.terminal_authority = Some(checksum(r.terminal.address));
        record.terminal_chain = Some(r.terminal.chain.as_str().into());
        record.authority_kind = Some(r.kind.as_str().into());
        record.compromise_depth = r.compromise_depth.map(i64::from);
        record.timelock_seconds = Some(r.timelock_seconds as i64);
        record.resolution_confidence = Some(r.confidence.as_str().into());
        resolved += 1;
    }
    resolved
}
