//! Whole runs: batches, resuming after a kill, and the canary. Replayed from fixtures recorded
//! at the same blocks, merged into one node.

use alloy::primitives::{Address, B256, Bytes};
use futures::future::BoxFuture;
use hermes_core::{Chain, ProxyRecord, SeedRow, Store};
use hermes_scan::rpc::ChainReads;
use hermes_scan::{
    AuthorityScanner, ChainRpc, Endpoint, Fixture, ReplayRpc, Scanner, Target, scan_into,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const NOW: i64 = 1_800_000_000;
const FRESH_SINCE: i64 = NOW - 20 * 3600;

fn fixtures(names: &[&str]) -> Vec<Fixture> {
    names
        .iter()
        .map(|name| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../tests/fixtures/{name}.json"));
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        })
        .collect()
}

fn node(fs: &[Fixture], chain: Chain) -> Arc<ReplayRpc> {
    let mut reads = ChainReads::default();
    for f in fs {
        reads.merge(f.reads.get(&chain).cloned().unwrap_or_default());
    }
    Arc::new(ReplayRpc::new(
        format!("merged ({})", chain.as_str()),
        reads,
    ))
}

fn scanners(fs: &[Fixture]) -> (Scanner, AuthorityScanner) {
    let base = node(fs, Chain::Base);
    let authorities = AuthorityScanner::new(
        Endpoint::new(base.clone(), Duration::ZERO),
        Endpoint::new(node(fs, Chain::Ethereum), Duration::ZERO),
        1,
    );
    (Scanner::new(base, 1), authorities)
}

async fn seeded(fs: &[Fixture]) -> Store {
    let store = Store::open("sqlite::memory:").await.unwrap();
    let rows: Vec<SeedRow> = fs
        .iter()
        .enumerate()
        .map(|(i, f)| SeedRow {
            address: f.target.to_checksum(None),
            label: None,
            source: "event".into(),
            family: None,
            first_block: None,
            discovered_at: i as i64,
        })
        .collect();
    store.add_seeds(&rows).await.unwrap();
    store
}

/// One `hermes scan`: whatever is due, in batches of two.
async fn run(store: &Store, fs: &[Fixture], limit: Option<i64>) -> usize {
    let targets: Vec<Target> = store
        .due_for_scan(FRESH_SINCE, limit)
        .await
        .unwrap()
        .into_iter()
        .map(|(a, label)| Target {
            address: a.parse().unwrap(),
            label,
        })
        .collect();
    let (scanner, authorities) = scanners(fs);
    let report = scan_into(store, &scanner, Some(&authorities), &targets, 2, NOW)
        .await
        .unwrap();
    report.counts.ok
}

async fn rows(store: &Store) -> Vec<String> {
    let mut all: Vec<String> = store
        .list_proxies(false)
        .await
        .unwrap()
        .iter()
        .map(|r: &ProxyRecord| serde_json::to_string(r).unwrap())
        .collect();
    all.sort();
    all
}

/// Killing a run is cheap here because each batch is written before the next starts and a
/// written address is not due again. That property is asserted before an optimization makes
/// it quietly untrue.
#[tokio::test]
async fn a_run_killed_after_one_batch_resumes_to_the_same_rows_as_one_that_was_not() {
    let fs = fixtures(&[
        "eoa-admin",
        "unknown-admin",
        "usd-plus",
        "uups-unconfirmed",
        "weth9",
        "usdc",
    ]);

    let uninterrupted = seeded(&fs).await;
    assert_eq!(run(&uninterrupted, &fs, None).await, 6);

    let interrupted = seeded(&fs).await;
    assert_eq!(
        run(&interrupted, &fs, Some(2)).await,
        2,
        "killed after one batch"
    );
    assert_eq!(
        run(&interrupted, &fs, None).await,
        4,
        "the resumed run scans only what the first one never reached"
    );
    assert_eq!(
        run(&interrupted, &fs, None).await,
        0,
        "and a third finds nothing due"
    );

    assert_eq!(rows(&interrupted).await, rows(&uninterrupted).await);
}

/// A node that answers zero for every slot while insisting the code is there. Every read it
/// gives is a confirmed empty read, so each row on its own would be published as "not
/// upgradeable".
struct Blank;

impl ChainRpc for Blank {
    fn storage(&self, _: Address, _: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        Box::pin(async { Ok(B256::ZERO) })
    }
    fn code(&self, _: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async { Ok(Bytes::from(vec![0u8; 100])) })
    }
    fn call(&self, _: Address, _: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async { Err(anyhow::anyhow!("execution reverted")) })
    }
}

#[tokio::test]
async fn a_node_that_blanks_known_proxies_is_refused_and_the_store_is_left_as_it_was() {
    let fs = fixtures(&[
        "proxy-admin-under-safe",
        "usdbc",
        "eoa-admin",
        "unknown-admin",
        "usd-plus",
        "beacon-proxy",
    ]);
    let store = seeded(&fs).await;
    run(&store, &fs, None).await;
    let before = rows(&store).await;
    assert_eq!(store.coverage().await.unwrap().covered_proxies, 6);

    let targets: Vec<Target> = fs
        .iter()
        .map(|f| Target {
            address: f.target,
            label: None,
        })
        .collect();
    let blank = Scanner::new(Arc::new(Blank), 1);
    let refused = scan_into(&store, &blank, None, &targets, 50, NOW + 1).await;

    let err = refused.expect_err("six known proxies going blank at once must not publish");
    assert!(err.to_string().contains("refusing to publish"), "{err}");
    assert_eq!(
        rows(&store).await,
        before,
        "nothing from the refused batch was written"
    );
}
