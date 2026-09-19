//! The hand-verified table, replayed offline.
//!
//! Each row in `tests/verified.json` names an address, what a person established it resolves
//! to by reading the chain without Hermes, and the blocks that was true at. Each fixture in
//! `tests/fixtures/` holds every answer the chain gave Hermes for that address at those same
//! blocks. Replaying one through the real pipeline and comparing it to the other asks the only
//! question that matters end to end: does Hermes say what is true?
//!
//! Green here means "we did not regress". Whether the chain has since moved is the scheduled
//! live job's question (`hermes verify`), and it is allowed to fail.

use hermes_core::Chain;
use hermes_scan::verify::{VerifiedRow, differences, load_table, render_markdown};
use hermes_scan::{AuthorityScanner, Endpoint, Fixture, Scanner, Target, scan_and_resolve};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn table() -> Vec<VerifiedRow> {
    load_table(&workspace().join("tests/verified.json")).expect("tests/verified.json")
}

fn fixture(name: &str) -> Fixture {
    let path = workspace().join(format!("tests/fixtures/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; record it with `hermes record`", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run the production pipeline against one fixture and return what it concluded.
async fn replay(row: &VerifiedRow, f: &Fixture) -> Vec<String> {
    let base = Arc::new(f.replay(Chain::Base));
    let ethereum = Arc::new(f.replay(Chain::Ethereum));
    let scanner = Scanner::new(base.clone(), 1);
    let authorities = AuthorityScanner::new(
        Endpoint::new(base, Duration::ZERO),
        Endpoint::new(ethereum, Duration::ZERO),
        1,
    );
    let target = Target {
        address: row.address,
        label: None,
    };
    let scanned = scan_and_resolve(&scanner, &authorities, &[target], 0).await;
    differences(&row.expected, scanned.records.first())
}

#[tokio::test]
async fn every_verified_address_replays_to_its_hand_checked_answer() {
    let rows = table();
    assert!(
        rows.len() >= 10,
        "the PRD bar is ten hand-verified protocols"
    );
    let checks = rows.iter().map(|row| async move {
        let f = fixture(&row.name);
        assert_eq!(
            f.target, row.address,
            "{}: fixture is for another address",
            row.name
        );
        assert_eq!(
            f.blocks.get(&Chain::Base),
            Some(&row.checked.base_block),
            "{}: the fixture must be recorded at the block the hand check was made at",
            row.name
        );
        if let Some(l1) = row.checked.ethereum_block {
            assert_eq!(f.blocks.get(&Chain::Ethereum), Some(&l1), "{}", row.name);
        }
        let diffs = replay(row, &f).await;
        (!diffs.is_empty()).then(|| {
            format!(
                "{} ({}):\n    {}",
                row.name,
                row.label,
                diffs.join("\n    ")
            )
        })
    });
    // Rows are independent, and each one waits out real confirming re-reads of empty code, so
    // they run together rather than one after another.
    let failures: Vec<String> = futures::future::join_all(checks)
        .await
        .into_iter()
        .flatten()
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The table covers the shapes that break resolvers, not ten Safes picked by TVL.
#[test]
fn the_table_is_curated_by_shape() {
    let rows = table();
    let expect = |f: &dyn Fn(&VerifiedRow) -> bool, what: &str| {
        assert!(rows.iter().any(f), "no row covers {what}");
    };
    let e = |r: &VerifiedRow| r.expected.clone();
    expect(
        &|r| e(r).terminal_chain.as_deref() == Some("ethereum"),
        "an authority reached through an L1 alias",
    );
    expect(&|r| e(r).kind == "admin_only", "an AdminOnly predeploy");
    expect(&|r| e(r).kind == "uups", "a UUPS proxy");
    expect(&|r| e(r).kind == "beacon", "a beacon proxy");
    expect(
        &|r| e(r).kind == "zeppelin_os",
        "a pattern Hermes does not cover",
    );
    expect(
        &|r| e(r).authority_kind.as_deref() == Some("eoa"),
        "a genuine single key",
    );
    expect(
        &|r| {
            e(r).authority_kind.as_deref() == Some("safe")
                && e(r).terminal_chain.as_deref() == Some("base")
        },
        "a Safe on Base",
    );
    expect(
        &|r| e(r).kind == "transparent" && e(r).terminal_authority.is_none(),
        "an admin whose answer is honestly unknown",
    );
    let names: std::collections::HashSet<_> = rows.iter().map(|r| &r.name).collect();
    assert_eq!(names.len(), rows.len(), "fixture names must be unique");
}

/// The page a human reads is generated from the file the suite checks, so the two cannot
/// disagree. Regenerate with `cargo test -p hermes-scan --test verified -- --ignored`.
#[test]
fn docs_verification_md_matches_the_table() {
    let doc = std::fs::read_to_string(workspace().join("docs/verification.md"))
        .expect("docs/verification.md");
    assert!(
        doc.contains(&render_markdown(&table())),
        "docs/verification.md is out of date with tests/verified.json; regenerate it with \
         `cargo test -p hermes-scan --test verified -- --ignored regenerate`"
    );
}

#[test]
#[ignore = "writes docs/verification.md; run by hand after editing tests/verified.json"]
fn regenerate_docs_verification_md() {
    let path = workspace().join("docs/verification.md");
    let doc = std::fs::read_to_string(&path).expect("docs/verification.md");
    let start = doc.find("<!-- table -->").expect("start marker") + "<!-- table -->\n".len();
    let end = doc.find("<!-- /table -->").expect("end marker");
    let updated = format!(
        "{}{}{}",
        &doc[..start],
        render_markdown(&table()),
        &doc[end..]
    );
    std::fs::write(&path, updated).unwrap();
}
