//! The RPC boundary, exercised with recorded answers and with a node that will not answer.

use alloy::primitives::{Address, B256, Bytes};
use futures::future::BoxFuture;
use hermes_core::Chain;
use hermes_scan::{
    AuthorityScanner, ChainRpc, Endpoint, Fixture, Scanner, Target, scan_and_resolve,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn fixture(name: &str) -> Fixture {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/fixtures/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A node that fails every request the way a rate limiter or an outage does.
struct Outage;

impl ChainRpc for Outage {
    fn storage(&self, _: Address, _: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        Box::pin(async { Err(anyhow::anyhow!("HTTP error 503 Service Unavailable")) })
    }
    fn code(&self, _: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async { Err(anyhow::anyhow!("HTTP error 503 Service Unavailable")) })
    }
    fn call(&self, _: Address, _: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async { Err(anyhow::anyhow!("HTTP error 503 Service Unavailable")) })
    }
}

/// The step-1 honesty rule, end to end. This proxy's admin has no code on Base, and with
/// Ethereum answering it resolves to a genuine key. With Ethereum down, nothing has established
/// that no L1 contract stands behind the alias, so the only honest answer is no answer: the
/// proxy must come back unresolved, never as "one key".
///
/// Time is paused so the scanner's real backoff schedule runs instantly.
#[tokio::test(start_paused = true)]
async fn an_unreachable_l1_leaves_a_codeless_admin_unresolved_rather_than_one_key() {
    let f = fixture("eoa-admin");
    let base = Arc::new(f.replay(Chain::Base));
    let scanner = Scanner::new(base.clone(), 1);
    let authorities = AuthorityScanner::new(
        Endpoint::new(base, Duration::ZERO),
        Endpoint::new(Arc::new(Outage), Duration::ZERO),
        1,
    );
    let target = Target {
        address: f.target,
        label: None,
    };
    let scanned = scan_and_resolve(&scanner, &authorities, &[target], 0).await;

    let record = scanned
        .records
        .first()
        .expect("the proxy itself was read fine");
    assert_eq!(record.kind, "transparent");
    assert_eq!(
        record.terminal_authority, None,
        "an unread L1 must not become a verdict"
    );
    assert_eq!(record.authority_kind, None);
    assert_eq!(record.compromise_depth, None);
    assert_eq!(scanned.counts.resolved, 0);
}

/// The number that governs whether a scan finishes is calls per address, not wall clock. Five
/// slots plus code for anything with a slot set; an empty-looking address is read twice.
#[tokio::test(start_paused = true)]
async fn the_slot_scan_stays_within_its_call_budget() {
    for (name, budget) in [("l2-standard-bridge", 6), ("usd-plus", 6), ("weth9", 12)] {
        let f = fixture(name);
        let base = Arc::new(f.replay(Chain::Base));
        let scanner = Scanner::new(base.clone(), 1);
        let results = scanner.scan(vec![f.target]).await;
        assert!(results[0].1.is_ok(), "{name}");
        assert_eq!(
            base.reads_answered(),
            budget,
            "{name}: a refactor that changes this changes how fast every scan hits the rate limit"
        );
    }
}
