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
    // Restricted rows are counted apart: the named keys are the gate, but the contract limits
    // what they can install, so they are neither a plain confirmation nor a contradiction.
    let mut by: BTreeMap<&str, (u64, u64, u64, u64)> = BTreeMap::new();
    for (s, v) in verdicts {
        let e = by.entry(s.as_str()).or_default();
        match v.outcome {
            Outcome::Confirmed => e.0 += 1,
            Outcome::Contradicted => e.1 += 1,
            Outcome::Undetermined => e.2 += 1,
            Outcome::Restricted => e.3 += 1,
        }
    }
    println!(
        "stratum | frame | confirmed | restricted | contradicted | undetermined | gate confirmed among determined (95% Wilson)"
    );
    let mut results = Vec::new();
    for (s, size) in frame {
        let (confirmed, x, u, restricted) = by.get(s.as_str()).copied().unwrap_or_default();
        let c = confirmed + restricted;
        let r = wilson(c, c + x);
        let shown = match (r.estimate, r.low, r.high) {
            (Some(p), Some(lo), Some(hi)) => {
                format!("{:.0}% [{:.0}%, {:.0}%]", p * 100.0, lo * 100.0, hi * 100.0)
            }
            _ => "no determined trial".into(),
        };
        println!("{s} | {size} | {confirmed} | {restricted} | {x} | {u} | {shown}");
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

pub struct BacktestPlan<'a> {
    pub endpoints: &'a crate::Endpoints,
    pub logs_url: &'a str,
    /// Upgrades to judge, not counting deployments skipped on the way.
    pub limit: usize,
    pub seed: u64,
    pub out: &'a Path,
}

/// One past upgrade, judged against what Hermes would have said the block before.
#[derive(Serialize)]
struct Judged {
    address: Address,
    /// `proxy` or `beacon`.
    subject: &'static str,
    block: u64,
    transaction: alloy::primitives::B256,
    judgement: hermes_core::backtest::Judgement,
    observed: hermes_core::backtest::Observed,
    /// The root and key count Hermes reads at `block - 1`, and why there is no count when
    /// there is none.
    root: Option<Node>,
    keys_required: Option<u32>,
    depth_unknown_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct BacktestReport {
    model_version: i64,
    seed: u64,
    /// Upgrade sightings in the index's scope, before deployments were set aside.
    candidates: usize,
    deployments_skipped: usize,
    unreadable: usize,
    judged: Vec<Judged>,
}

/// Upgrade sightings of indexed proxies and of the beacons they read, in a seeded order, and
/// the indexed proxies by lowercased address.
async fn upgrade_candidates(
    store: &Store,
    seed: u64,
) -> anyhow::Result<(Vec<(String, i64)>, std::collections::HashSet<String>)> {
    let rows = store.upgrade_entries().await?;
    let proxies: std::collections::HashSet<String> =
        rows.iter().map(|(r, _)| r.address.to_lowercase()).collect();
    let beacons: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|(r, _)| r.beacon.as_ref().map(|b| b.to_lowercase()))
        .collect();
    let candidates: Vec<(String, i64)> = store
        .upgrade_sightings()
        .await?
        .into_iter()
        .filter(|(a, _)| {
            let a = a.to_lowercase();
            proxies.contains(&a) || beacons.contains(&a)
        })
        .collect();
    let one_stratum = vec![String::new(); candidates.len()];
    let order = stratified(&one_stratum, candidates.len(), seed)
        .into_values()
        .next()
        .unwrap_or_default();
    Ok((
        order.into_iter().map(|i| candidates[i].clone()).collect(),
        proxies,
    ))
}

/// What Hermes reads about `address` at `block`: the walk's start and the probes under it.
async fn read_before(
    endpoints: &crate::Endpoints,
    address: Address,
    block: u64,
    is_proxy: bool,
) -> anyhow::Result<Option<(&'static str, Option<Node>, HashMap<Node, AuthorityProbe>)>> {
    let checked = hermes_scan::verify::Checked {
        on: String::new(),
        base_block: block,
        ethereum_block: None,
        how: String::new(),
        urls: Vec::new(),
    };
    let (scanner, authorities) = endpoints.at_blocks(&checked).await?;
    if !is_proxy {
        let node = Node::base(address);
        return Ok(Some((
            "beacon",
            Some(node),
            authorities.collect(vec![node]).await,
        )));
    }
    let target = hermes_scan::Target {
        address,
        label: None,
    };
    let scanned = hermes_scan::scan_and_resolve(&scanner, &authorities, &[target], 0).await;
    Ok(scanned.records.first().map(|r| {
        (
            "proxy",
            hermes_core::graph::entry_of(r),
            scanned.probes.clone(),
        )
    }))
}

/// Judge sighted upgrades of indexed proxies and their beacons, newest model, at the block
/// before each. Deployments (no code the block before) are set aside, not judged.
pub async fn backtest(store: &Store, plan: BacktestPlan<'_>) -> anyhow::Result<()> {
    use hermes_scan::backtest::Reader;
    let (ordered, proxies) = upgrade_candidates(store, plan.seed).await?;
    let pace = std::time::Duration::from_millis(plan.endpoints.call_interval_ms);
    let reader = Reader::new(hermes_scan::connect(&plan.endpoints.rpc_url).await?, pace);
    let logs = Reader::new(hermes_scan::connect(plan.logs_url).await?, pace);
    let mut report = BacktestReport {
        model_version: MODEL_VERSION,
        seed: plan.seed,
        candidates: ordered.len(),
        deployments_skipped: 0,
        unreadable: 0,
        judged: Vec::new(),
    };
    for (address, block) in ordered {
        if report.judged.len() >= plan.limit {
            break;
        }
        let (Ok(address), Ok(block)) = (address.parse::<Address>(), u64::try_from(block)) else {
            continue;
        };
        let is_proxy = proxies.contains(&format!("{address:#x}"));
        judge_sighting(
            (&reader, &logs),
            plan.endpoints,
            &mut report,
            address,
            block,
            is_proxy,
        )
        .await?;
    }
    print_backtest(&report);
    std::fs::write(plan.out, serde_json::to_string_pretty(&report)? + "\n")?;
    eprintln!("wrote {}", plan.out.display());
    Ok(())
}

/// The root, upgrade role and admin role of a role-gated root whose holders are not listed.
fn unlisted_role_gate(
    published: Option<&hermes_core::Resolution>,
    probes: &HashMap<Node, AuthorityProbe>,
) -> Option<(Address, alloy::primitives::B256, alloy::primitives::B256)> {
    let root = published?.terminal;
    let gate = probes.get(&root)?.roles.as_ref()?;
    gate.members
        .is_none()
        .then_some((root.address, gate.role, gate.admin_role))
}

/// Judge every transaction that emitted `Upgraded` from `address` in `block`.
async fn judge_sighting(
    (reader, logs): (
        &hermes_scan::backtest::Reader,
        &hermes_scan::backtest::Reader,
    ),
    endpoints: &crate::Endpoints,
    report: &mut BacktestReport,
    address: Address,
    block: u64,
    is_proxy: bool,
) -> anyhow::Result<()> {
    use hermes_core::backtest::{Judgement, judge};
    let Ok(hashes) = logs.upgrade_transactions(address, block).await else {
        report.unreadable += 1;
        return Ok(());
    };
    match reader.had_code(address, block - 1).await {
        Some(true) => {}
        Some(false) => {
            report.deployments_skipped += 1;
            return Ok(());
        }
        None => {
            report.unreadable += 1;
            return Ok(());
        }
    }
    let Some((subject, entry, probes)) =
        read_before(endpoints, address, block - 1, is_proxy).await?
    else {
        report.unreadable += 1;
        return Ok(());
    };
    let resolution = entry.map(|e| hermes_core::resolve(e, &probes));
    let published = resolution.as_ref().filter(|r| r.unresolved().is_none());
    for hash in hashes {
        let threshold_of = |a: Address| probes.get(&Node::base(a)).and_then(|p| p.threshold);
        let seen = match reader.transaction(hash).await {
            Ok((tx, receipt)) => hermes_scan::backtest::observed(&tx, &receipt, &threshold_of),
            Err(_) => None,
        };
        let Some(mut seen) = seen else {
            report.unreadable += 1;
            continue;
        };
        if let Some(gate) = unlisted_role_gate(published, &probes) {
            let (root, role, admin) = gate;
            let held = reader.has_role(root, role, seen.sender, block - 1).await;
            let held_admin = reader.has_role(root, admin, seen.sender, block - 1).await;
            seen.sender_held_root_role = match (held, held_admin) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
        }
        let judgement = match entry {
            Some(e) => judge(e, &probes, resolution.as_ref(), &seen),
            None => Judgement::Unresolved,
        };
        eprintln!("{address} at {block}: {judgement:?}");
        report.judged.push(Judged {
            address,
            subject,
            block,
            transaction: hash,
            judgement,
            observed: seen,
            root: published.map(|r| r.terminal),
            keys_required: published.and_then(|r| r.compromise_depth),
            depth_unknown_reason: published.and_then(|r| r.depth_gap()).map(|g| g.as_str()),
        });
    }
    Ok(())
}

fn print_backtest(report: &BacktestReport) {
    use hermes_core::backtest::Judgement;
    let mut tally: BTreeMap<String, u64> = BTreeMap::new();
    let (mut consistent, mut contradicted) = (0, 0);
    for j in &report.judged {
        let label = match j.judgement {
            Judgement::Unresolved => "unresolved".to_string(),
            Judgement::Consistent(w) => {
                consistent += 1;
                format!(
                    "consistent: {}",
                    serde_json::to_string(&w).unwrap_or_default()
                )
            }
            Judgement::Contradicted(w) => {
                contradicted += 1;
                format!(
                    "contradicted: {}",
                    serde_json::to_string(&w).unwrap_or_default()
                )
            }
            Judgement::Ambiguous(w) => {
                format!(
                    "ambiguous: {}",
                    serde_json::to_string(&w).unwrap_or_default()
                )
            }
        };
        *tally.entry(label).or_default() += 1;
    }
    println!(
        "{} upgrade sightings in scope; {} deployments set aside; {} unreadable; {} judged",
        report.candidates,
        report.deployments_skipped,
        report.unreadable,
        report.judged.len()
    );
    for (label, n) in &tally {
        println!("  {label}: {n}");
    }
    let r = wilson(consistent, consistent + contradicted);
    if let (Some(p), Some(lo), Some(hi)) = (r.estimate, r.low, r.high) {
        println!(
            "consistent among decided: {:.0}% [{:.0}%, {:.0}%] (95% Wilson, {} decided)",
            p * 100.0,
            lo * 100.0,
            hi * 100.0,
            consistent + contradicted
        );
    }
}
