//! Judging the index against something other than itself: the EVM on a fork, and the chain's
//! own record of past upgrades.
//!
//! Both are developer tools. They read the chain, so they never run in CI, and what they write
//! is a report with its scope, not a change to the store.

use alloy::primitives::Address;
use hermes_core::audit::{StratumResult, stratified, weighted, wilson};
use hermes_core::authority::AuthorityProbe;
use hermes_core::blast::reachable;
use hermes_core::graph::{replay, upgrade_entry_of};
use hermes_core::{Chain, MODEL_VERSION, Node, ProxyRecord, Store};
use hermes_scan::oracle::{self, Anvil, Outcome, Undriveable, Verdict};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub struct OraclePlan<'a> {
    pub fork_url: &'a str,
    pub anvil: &'a str,
    pub port: u16,
    pub per_stratum: usize,
    pub seed: u64,
    pub out: &'a Path,
}

/// The stratum a counted row is drawn from: how the walk starts and what it ends at.
fn stratum(row: &ProxyRecord) -> String {
    format!(
        "{}/{}",
        row.upgrade_path.as_deref().unwrap_or("?"),
        row.authority_kind.as_deref().unwrap_or("?")
    )
}

#[derive(Serialize)]
struct OracleReport {
    model_version: i64,
    seed: u64,
    per_stratum: usize,
    /// Rows with a published key count, by stratum: the population the sample stands for.
    frame: BTreeMap<String, u64>,
    verdicts: Vec<(String, Verdict)>,
}

/// Whether every node under `entry` was last read at `block`.
async fn read_at(store: &Store, nodes: &[Node], block: i64) -> anyhow::Result<bool> {
    for n in nodes {
        if n.chain != Chain::Base {
            continue;
        }
        let seen = store.node(*n).await?.and_then(|s| s.last_seen_block);
        if seen != Some(block) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Draw the audit set, put each row's count to a fork at the row's own block, and write the
/// verdicts with their scope.
pub async fn oracle(store: &Store, plan: OraclePlan<'_>) -> anyhow::Result<()> {
    let rows: Vec<ProxyRecord> = store
        .upgrade_entries()
        .await?
        .into_iter()
        .filter(|(r, entry)| r.compromise_depth.is_some() && entry.is_some())
        .map(|(r, _)| r)
        .collect();
    let strata: Vec<String> = rows.iter().map(stratum).collect();
    let mut frame: BTreeMap<String, u64> = BTreeMap::new();
    for s in &strata {
        *frame.entry(s.clone()).or_default() += 1;
    }
    let drawn = stratified(&strata, plan.per_stratum, plan.seed);
    let probes = store.all_probes().await?;

    let mut by_block: BTreeMap<i64, Vec<&ProxyRecord>> = BTreeMap::new();
    let mut verdicts = Vec::new();
    for picks in drawn.values() {
        for &i in picks {
            let row = &rows[i];
            match row.scanned_block {
                Some(b) => by_block.entry(b).or_default().push(row),
                None => verdicts.push((stratum(row), unpinned(row))),
            }
        }
    }
    let total: usize = drawn.values().map(Vec::len).sum();
    eprintln!(
        "{total} rows drawn from {} counted, across {} strata and {} blocks",
        rows.len(),
        frame.len(),
        by_block.len()
    );
    for (block, rows) in by_block {
        let block_u64 = u64::try_from(block)?;
        let anvil = Anvil::start(plan.anvil, plan.fork_url, block_u64, plan.port).await?;
        for row in rows {
            let verdict = judge(store, &anvil, &probes, row, block).await?;
            eprintln!(
                "{} {:?}{}",
                row.address,
                verdict.outcome,
                verdict
                    .undriveable
                    .map_or(String::new(), |u| format!(" ({})", u.as_str()))
            );
            verdicts.push((stratum(row), verdict));
        }
    }
    print_summary(&frame, &verdicts);
    let report = OracleReport {
        model_version: MODEL_VERSION,
        seed: plan.seed,
        per_stratum: plan.per_stratum,
        frame,
        verdicts,
    };
    std::fs::write(plan.out, serde_json::to_string_pretty(&report)? + "\n")?;
    eprintln!("wrote {}", plan.out.display());
    Ok(())
}

fn unpinned(row: &ProxyRecord) -> Verdict {
    let entry = upgrade_entry_of(row).expect("drawn only from rows with an entry");
    let keys = u32::try_from(row.compromise_depth.unwrap_or(0)).unwrap_or(u32::MAX);
    oracle::not_run(address_of(row), entry, keys, Undriveable::NotAtOneBlock)
}

fn address_of(row: &ProxyRecord) -> Address {
    row.address.parse().unwrap_or(Address::ZERO)
}

async fn judge(
    store: &Store,
    anvil: &Anvil,
    probes: &HashMap<Node, AuthorityProbe>,
    row: &ProxyRecord,
    block: i64,
) -> anyhow::Result<Verdict> {
    let entry = upgrade_entry_of(row).expect("drawn only from rows with an entry");
    let published = u32::try_from(row.compromise_depth.unwrap_or(0)).unwrap_or(u32::MAX);
    let proxy = address_of(row);
    let under: Vec<Node> = reachable(entry.start(), probes).into_iter().collect();
    if !read_at(store, &under, block).await? {
        return Ok(oracle::not_run(
            proxy,
            entry,
            published,
            Undriveable::NotAtOneBlock,
        ));
    }
    let replayed = replay(row, probes).and_then(|(_, r)| r.compromise_depth);
    if replayed != Some(published) {
        return Ok(oracle::not_run(
            proxy,
            entry,
            published,
            Undriveable::ReplayDiffers,
        ));
    }
    Ok(oracle::check(&anvil.evm, probes, entry, proxy, published).await)
}

fn print_summary(frame: &BTreeMap<String, u64>, verdicts: &[(String, Verdict)]) {
    let mut by: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
    for (s, v) in verdicts {
        let e = by.entry(s.as_str()).or_default();
        match v.outcome {
            Outcome::Confirmed => e.0 += 1,
            Outcome::Contradicted => e.1 += 1,
            Outcome::Undetermined => e.2 += 1,
        }
    }
    println!(
        "stratum | frame | confirmed | contradicted | undetermined | confirmed among determined (95% Wilson)"
    );
    let mut results = Vec::new();
    for (s, size) in frame {
        let (c, x, u) = by.get(s.as_str()).copied().unwrap_or_default();
        let r = wilson(c, c + x);
        let shown = match (r.estimate, r.low, r.high) {
            (Some(p), Some(lo), Some(hi)) => {
                format!("{:.0}% [{:.0}%, {:.0}%]", p * 100.0, lo * 100.0, hi * 100.0)
            }
            _ => "no determined trial".into(),
        };
        println!("{s} | {size} | {c} | {x} | {u} | {shown}");
        results.push(StratumResult {
            frame: *size,
            successes: c,
            trials: c + x,
        });
    }
    match weighted(&results) {
        Some((p, lo, hi)) => println!(
            "weighted by the frame: {:.1}% [{:.1}%, {:.1}%] of determined counts confirmed",
            p * 100.0,
            lo * 100.0,
            hi * 100.0
        ),
        None => println!(
            "no frame-wide rate: some stratum has no determined trial, and its rate would be a guess"
        ),
    }
}
