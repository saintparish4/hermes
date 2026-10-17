//! The oracle on the hand-verified rows, against a real fork.
//!
//! Ignored: it starts anvil and forks Base from the public endpoint, so it is a developer check,
//! like `hermes verify`. Run with `cargo test -p hermes-scan --test oracle -- --ignored`.
//!
//! Every row here has a count a person established without Hermes, so a contradiction means the
//! oracle is wrong, not the count. That is what it is for: before trusting a fork's "no" about
//! the index, see it say "yes" where the answer is known.

use hermes_core::graph::upgrade_entry_of;
use hermes_core::{Chain, ProxyRecord};
use hermes_scan::oracle::{Anvil, Outcome, check};
use hermes_scan::verify::load_table;
use hermes_scan::{AuthorityScanner, Endpoint, Fixture, Scanner, Target, scan_and_resolve};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn replayed(
    name: &str,
    address: alloy::primitives::Address,
) -> (ProxyRecord, Fixture, hermes_scan::Scanned) {
    let path = workspace().join(format!("tests/fixtures/{name}.json"));
    let f: Fixture = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let base = Arc::new(f.replay(Chain::Base));
    let ethereum = Arc::new(f.replay(Chain::Ethereum));
    let authorities = AuthorityScanner::new(
        Endpoint::new(base.clone(), Duration::ZERO),
        Endpoint::new(ethereum, Duration::ZERO),
        1,
    );
    let target = Target {
        address,
        label: None,
    };
    let scanned = scan_and_resolve(&Scanner::new(base, 1), &authorities, &[target], 0).await;
    (scanned.records[0].clone(), f, scanned)
}

/// Rows whose published answer the fork refutes, found by this test, each with what the EVM said.
/// They stay in the table with their hand-checked expectation, because the expectation is what
/// Hermes's rules give and what a person reading `owner()` concluded; this list is where the
/// fork's disagreement is kept, so it cannot quietly stop being seen.
const REFUTED: &[(&str, &str)] = &[
    (
        "uups-owner",
        "the implementation reverts `Ownable: caller is not the upgrader` for its owner(): \
         upgrades are gated by a separate upgrader, so the one-key root is not who can upgrade",
    ),
    (
        "multi-owner-account",
        "the implementation reverts `Only Mamo Strategy Registry can call` for its owner(), the \
         smart account: upgrades are gated by a registry contract, not by the account's signers",
    ),
];

#[tokio::test]
#[ignore = "starts anvil and forks Base over the network"]
async fn the_oracle_confirms_every_hand_verified_count_it_can_drive() {
    let rows = load_table(&workspace().join("tests/verified.json")).unwrap();
    let mut port = 8600;
    let mut seen = Vec::new();
    for row in rows
        .iter()
        .filter(|r| r.expected.compromise_depth.is_some())
        .filter(|r| std::env::var("ONLY").map_or(true, |only| only == r.name))
    {
        let (record, fixture, scanned) = replayed(&row.name, row.address).await;
        let entry = upgrade_entry_of(&record).unwrap();
        let block = fixture.blocks[&Chain::Base];
        port += 1;
        let anvil = Anvil::start("anvil", "https://mainnet.base.org", block, port)
            .await
            .unwrap();
        let keys = u32::try_from(record.compromise_depth.unwrap()).unwrap();
        let v = check(&anvil.evm, &scanned.probes, entry, row.address, keys).await;
        eprintln!("{}: {:?} {:?}", row.name, v.outcome, v.undriveable);
        seen.push((row.name.clone(), v));
    }
    let mut wrong = Vec::new();
    for (name, v) in &seen {
        let refuted = REFUTED.iter().any(|(r, _)| r == name);
        if refuted != (v.outcome == Outcome::Contradicted) {
            wrong.push(format!("{name}: {v:?}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    assert!(
        seen.iter().any(|(_, v)| v.outcome == Outcome::Confirmed),
        "nothing was confirmed"
    );
}
