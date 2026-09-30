//! The kept graph and its history, through the production pipeline.
//!
//! Replayed from fixtures recorded at pinned blocks. The two `sep29-*` fixtures are one beacon
//! proxy read one block apart, on either side of the moment its beacon passed from one key to a
//! 2-of-5 Safe on 2026-09-29. Their expectations were written from a hand check that did not
//! use Hermes (`tests/verified.json`); this file asks what the store makes of the pair.

use hermes_core::graph::replay;
use hermes_core::graph_store::{EventFilter, SubjectType};
use hermes_core::{Cause, Chain, Field, Node, Store};
use hermes_scan::{AuthorityScanner, Endpoint, Fixture, Scanner, Target, scan_into};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn fixture(name: &str) -> Fixture {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/fixtures/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// One `hermes scan` of `f`'s target, reading at `f`'s blocks and stamped with them.
async fn observe(store: &Store, f: &Fixture, at: i64) {
    let base = Arc::new(f.replay(Chain::Base));
    let scanner = Scanner::new(base.clone(), 1);
    let authorities = AuthorityScanner::new(
        Endpoint::new(base, Duration::ZERO),
        Endpoint::new(Arc::new(f.replay(Chain::Ethereum)), Duration::ZERO),
        1,
    );
    let block = |chain| f.blocks.get(&chain).map(|b| *b as i64);
    let obs = store
        .begin_observation(block(Chain::Base), block(Chain::Ethereum), at)
        .await
        .unwrap();
    let target = Target {
        address: f.target,
        label: None,
    };
    scan_into(store, &scanner, Some(&authorities), &[target], 50, &obs)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_sep29_transfer_is_recorded_as_it_happened() {
    let (before, after) = (fixture("sep29-before"), fixture("sep29-after"));
    let store = Store::open("sqlite::memory:").await.unwrap();
    observe(&store, &before, 1_000).await;
    assert!(
        store
            .events(&EventFilter::default())
            .await
            .unwrap()
            .is_empty(),
        "first sight is not a change"
    );
    observe(&store, &after, 2_000).await;

    let events = store
        .events(&EventFilter {
            include_reinterpretations: true,
            ascending: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let summary: Vec<(SubjectType, Field, Option<String>, Option<String>)> = events
        .iter()
        .map(|e| {
            (
                e.subject_type,
                e.change.field,
                e.change.old.clone(),
                e.change.new.clone(),
            )
        })
        .collect();
    let eoa = "0x21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc";
    let safe = "0x6454cf0127a153295435160768C85225Cd19Bf15";
    assert_eq!(
        summary,
        vec![
            (
                SubjectType::Proxy,
                Field::Root,
                Some(format!("base:{eoa}")),
                Some(format!("base:{safe}"))
            ),
            (
                SubjectType::Proxy,
                Field::KeysRequired,
                Some("1".into()),
                Some("2".into())
            ),
            (
                SubjectType::Node,
                Field::Owner,
                Some(eoa.into()),
                Some(safe.into())
            ),
        ]
    );
    assert!(events.iter().all(|e| e.cause == Cause::Chain));
    assert!(
        events
            .iter()
            .all(|e| (e.last_old_block, e.first_new_block) == (Some(51_927_177), Some(51_927_178))),
        "the change is bracketed by the two blocks the reads were pinned to"
    );
    assert_eq!(events[1].direction, Some("rose"));

    let beacon: Node = Node::base(
        "0xe68ED13998fd48497EAA3b52e20823605D8d7706"
            .parse()
            .unwrap(),
    );
    let owners: Vec<_> = store
        .edges_from(beacon, true)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.relation == hermes_core::Relation::Owner)
        .map(|e| (e.to.address.to_checksum(None), e.closed_block))
        .collect();
    assert_eq!(
        owners,
        vec![
            (eoa.to_string(), Some(51_927_178)),
            (safe.to_string(), None)
        ]
    );
}

/// The claim behind every offline command: walking what the store kept gives the answer the
/// scan published, for every verified shape at once.
#[tokio::test]
async fn walking_the_stored_graph_reproduces_every_published_root() {
    let names = [
        "proxy-admin",
        "l2-standard-bridge",
        "proxy-admin-under-safe",
        "usdbc",
        "eoa-admin",
        "unknown-admin",
        "beacon-proxy",
        "uups-owner",
        "usd-plus",
        "sep29-after",
    ];
    for name in names {
        let f = fixture(name);
        let store = Store::open("sqlite::memory:").await.unwrap();
        observe(&store, &f, 1_000).await;
        let row = store
            .get_proxy(&f.target.to_checksum(None))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{name}: no row"));
        let Some(entry) = hermes_core::graph::entry_of(&row) else {
            continue;
        };
        let probes = store.probes_from(&[entry]).await.unwrap();
        let (_, r) = replay(&row, &probes).unwrap();
        if r.unresolved().is_some() {
            assert_eq!(row.terminal_authority, None, "{name}");
            continue;
        }
        assert_eq!(
            row.terminal_authority.as_deref(),
            Some(r.terminal.address.to_checksum(None).as_str()),
            "{name}"
        );
        assert_eq!(
            row.terminal_chain.as_deref(),
            Some(r.terminal.chain.as_str()),
            "{name}"
        );
        assert_eq!(
            row.compromise_depth,
            r.compromise_depth.map(i64::from),
            "{name}"
        );
        assert_eq!(
            row.resolution_confidence.as_deref(),
            Some(r.confidence.as_str()),
            "{name}"
        );
    }
}
