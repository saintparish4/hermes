//! One scan, end to end: read the slots, classify, then walk every distinct admin to its root.
//!
//! This lives here rather than in the CLI so that the code path the deployment runs is the
//! same one the fixture tests replay. A test that re-assembled the pipeline by hand would
//! prove the pieces work, not that they are wired together the way production wires them.

use crate::probe::{ProbeOutcome, Scanner};
use crate::resolve::AuthorityScanner;
use alloy::primitives::Address;
use hermes_core::canary;
use hermes_core::store::checksum;
use hermes_core::{
    Classified, Node, ProxyRecord, Resolution, Store, UpgradeEntry, resolve, upgrade_entry,
    uups_implementation,
};
use std::collections::{HashMap, HashSet};

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

/// What one probe established, carried until resolution has written its root onto `record`.
struct Found {
    address: Address,
    record: ProxyRecord,
    classified: Classified,
}

impl ScanCounts {
    fn add(&mut self, other: ScanCounts) {
        self.ok += other.ok;
        self.failed += other.failed;
        self.rereads += other.rereads;
        self.unconfirmed += other.unconfirmed;
        self.resolved += other.resolved;
    }
}

async fn classify_targets(
    scanner: &Scanner,
    targets: &[Target],
    scanned_at: i64,
) -> (Vec<Found>, ScanCounts) {
    let results = scanner
        .scan(targets.iter().map(|t| t.address).collect())
        .await;
    let mut counts = ScanCounts::default();
    let found = results
        .iter()
        .filter_map(|(addr, outcome)| record(*addr, outcome, targets, scanned_at, &mut counts))
        .collect();
    (found, counts)
}

/// Scan `targets` and resolve what they found.
pub async fn scan_and_resolve(
    scanner: &Scanner,
    authorities: &AuthorityScanner,
    targets: &[Target],
    scanned_at: i64,
) -> Scanned {
    let (mut found, mut counts) = classify_targets(scanner, targets, scanned_at).await;
    counts.resolved = resolve_authorities(authorities, &mut found).await;
    Scanned {
        records: found.into_iter().map(|f| f.record).collect(),
        counts,
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    pub counts: ScanCounts,
    pub written: u64,
    pub batches: usize,
}

/// Scan `targets` into `store` a batch at a time.
///
/// Each batch is written before the next one starts, so a run killed anywhere loses at most one
/// batch, and the next run's `due_for_scan` carries on from there. Each batch also has to get
/// past the canary first: one in which too many known proxies suddenly stop being covered is an
/// endpoint failing, and the run stops rather than publish it. `authorities` of `None` only
/// classifies, which is the quick first pass a fresh deployment makes before opening its port.
pub async fn scan_into(
    store: &Store,
    scanner: &Scanner,
    authorities: Option<&AuthorityScanner>,
    targets: &[Target],
    batch: usize,
    scanned_at: i64,
) -> anyhow::Result<RunReport> {
    let covered_before = store.covered_addresses().await?;
    let mut report = RunReport::default();
    for chunk in targets.chunks(batch.max(1)) {
        let scanned = match authorities {
            Some(authorities) => scan_and_resolve(scanner, authorities, chunk, scanned_at).await,
            None => {
                let (found, counts) = classify_targets(scanner, chunk, scanned_at).await;
                Scanned {
                    records: found.into_iter().map(|f| f.record).collect(),
                    counts,
                }
            }
        };
        let canary = canary::check(&covered_before, &scanned.records);
        if canary.tripped() {
            anyhow::bail!(
                "refusing to publish a batch in which {} of {} known proxies stopped being \
                 covered; an endpoint failing is likelier than the chain changing, and nothing \
                 from this batch was written",
                canary.lost,
                canary.known
            );
        }
        report.written += store.upsert_many(&scanned.records).await?;
        report.counts.add(scanned.counts);
        report.batches += 1;
        tracing::info!(
            batch = report.batches,
            scanned = (report.batches * batch).min(targets.len()),
            of = targets.len(),
            "batch written"
        );
    }
    Ok(report)
}

/// The row one probe earns, or `None` when it earned nothing publishable.
fn record(
    addr: Address,
    outcome: &anyhow::Result<ProbeOutcome>,
    targets: &[Target],
    scanned_at: i64,
    counts: &mut ScanCounts,
) -> Option<Found> {
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
    let record = ProxyRecord {
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
    };
    Some(Found {
        address: addr,
        record,
        classified: c,
    })
}

fn skipped(addr: Address, why: &str) {
    tracing::warn!(%addr, why, "address skipped");
}

/// Ask each distinct UUPS implementation whether it really is one.
async fn confirm_uups(
    scanner: &AuthorityScanner,
    found: &[Found],
) -> HashMap<Address, Option<bool>> {
    let implementations: HashSet<Address> = found
        .iter()
        .filter_map(|f| uups_implementation(&f.classified))
        .collect();
    let mut confirmed = HashMap::new();
    for implementation in implementations {
        confirmed.insert(implementation, scanner.is_uups(implementation).await);
    }
    confirmed
}

/// Walk every proxy's upgrade entry to its root and write the answer back onto its row.
///
/// Done as a second pass rather than inline so the probe collection can be batched across
/// every entry at once. Entries are shared heavily — one ProxyAdmin governs twenty contracts
/// on Base, one beacon five — so resolving per proxy would re-walk the same subgraph each time.
async fn resolve_authorities(scanner: &AuthorityScanner, found: &mut [Found]) -> usize {
    let uups = confirm_uups(scanner, found).await;
    let entries: Vec<_> = found
        .iter()
        .map(|f| {
            let confirmed = uups_implementation(&f.classified)
                .and_then(|implementation| uups.get(&implementation).copied().flatten());
            upgrade_entry(f.address, &f.classified, confirmed)
        })
        .collect();
    let roots: Vec<Node> = entries
        .iter()
        .flatten()
        .flatten()
        .map(|entry| entry.start())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    tracing::info!(count = roots.len(), "resolving authorities");
    let probes = scanner.collect(roots).await;
    let mut resolved = 0;
    for (f, entry) in found.iter_mut().zip(entries) {
        match entry {
            Some(Ok(entry))
                if write_root(&mut f.record, entry, resolve(entry.start(), &probes)) =>
            {
                resolved += 1;
            }
            Some(Err(reason)) => f.record.unresolved_reason = Some(reason.as_str().into()),
            Some(Ok(_)) | None => {}
        }
    }
    resolved
}

/// Put one resolution onto a row, capped at what its entry point can support. Returns whether
/// the row now has a root.
///
/// An unresolved walk records why and nothing else. The store decides what that means for a
/// root it already holds: a real finding replaces it, an outage does not.
fn write_root(record: &mut ProxyRecord, entry: UpgradeEntry, r: Resolution) -> bool {
    let r = r.capped(entry.ceiling());
    if let Some(reason) = r.unresolved() {
        record.unresolved_reason = Some(reason.as_str().into());
        return false;
    }
    record.terminal_authority = Some(checksum(r.terminal.address));
    record.terminal_chain = Some(r.terminal.chain.as_str().into());
    record.authority_kind = Some(r.kind.as_str().into());
    record.compromise_depth = r.compromise_depth.map(i64::from);
    record.timelock_seconds = Some(r.timelock_seconds as i64);
    record.resolution_confidence = Some(r.confidence.as_str().into());
    record.upgrade_path = Some(entry.as_str().into());
    record.depth_unknown_reason = r.depth_gap().map(|gap| gap.as_str().into());
    true
}
