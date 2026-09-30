//! `/v1`: the graph, what it reaches, and how it changed.
//!
//! Beside the original routes rather than replacing them, because `static/index.html` fetches
//! those and it is the page that must never break. Every response here carries a `scope`: the
//! index is a sample, the graph is as of one observation, and "first seen" means first seen by
//! Hermes. A count without that context reads as a census.

use crate::{ApiResult, not_found};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use hermes_core::graph_store::{EventFilter, StoredEvent};
use hermes_core::time::iso8601;
use hermes_core::view::{self, event_line, event_when, key_view, node_view, proxy_view, scope};
use hermes_core::{Chain, Node, Store};
use serde::{Deserialize, Serialize};

/// The most events one request returns.
pub const MAX_EVENTS: i64 = 1_000;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/v1/coverage", get(coverage))
        .route("/v1/authorities", get(authorities))
        .route("/v1/authorities/{chain}/{address}", get(authority))
        .route("/v1/proxies/{address}", get(proxy))
        .route("/v1/nodes/{chain}/{address}", get(node))
        .route("/v1/keys/{address}", get(key))
        .route("/v1/signers", get(signers))
        .route("/v1/changes", get(changes))
        .route("/v1/changes.atom", get(changes_atom))
}

fn bad_request(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

/// A node from path segments, or the message a 400 should carry.
fn parse_node(chain: &str, address: &str) -> Result<Node, String> {
    let chain = Chain::parse(chain)
        .ok_or_else(|| format!("unknown chain {chain}; use base or ethereum"))?;
    let address = address
        .parse()
        .map_err(|_| format!("{address} is not an address"))?;
    Ok(Node { chain, address })
}

#[derive(Serialize)]
struct Scoped<T: Serialize> {
    #[serde(flatten)]
    body: T,
    scope: view::Scope,
}

async fn scoped<T: Serialize>(store: &Store, body: T) -> ApiResult<Response> {
    Ok(Json(Scoped {
        body,
        scope: scope(store).await?,
    })
    .into_response())
}

async fn coverage(State(store): State<Store>) -> ApiResult<Response> {
    let c = store.coverage().await?;
    scoped(&store, c).await
}

#[derive(Serialize)]
struct AuthorityList {
    count: usize,
    authorities: Vec<hermes_core::store::AuthorityRow>,
}

async fn authorities(State(store): State<Store>) -> ApiResult<Response> {
    let authorities = store.authority_rollup().await?;
    scoped(
        &store,
        AuthorityList {
            count: authorities.len(),
            authorities,
        },
    )
    .await
}

#[derive(Serialize)]
struct AuthorityDetail {
    authority: hermes_core::store::AuthorityRow,
    proxies: Vec<hermes_core::ProxyRecord>,
    node: Option<view::NodeView>,
}

/// One root: its rollup row, every proxy that resolves to it, and the node itself. The chain
/// is in the path, so an address that is a root on both chains is two resources, not a 409.
async fn authority(
    State(store): State<Store>,
    Path((chain, address)): Path<(String, String)>,
) -> ApiResult<Response> {
    let node = match parse_node(&chain, &address) {
        Ok(n) => n,
        Err(m) => return Ok(bad_request(m)),
    };
    let checksummed = node.address.to_checksum(None);
    let Some(authority) = store.authority_rollup().await?.into_iter().find(|a| {
        a.address.eq_ignore_ascii_case(&checksummed)
            && a.chain.as_deref() == Some(node.chain.as_str())
    }) else {
        return Ok(not_found("unknown authority", &address));
    };
    let proxies = store
        .proxies_for_authority(&checksummed, Some(node.chain.as_str()))
        .await?;
    Ok(Json(AuthorityDetail {
        authority,
        proxies,
        node: node_view(&store, node).await?,
    })
    .into_response())
}

async fn proxy(State(store): State<Store>, Path(address): Path<String>) -> ApiResult<Response> {
    Ok(match proxy_view(&store, &address).await? {
        Some(v) => Json(v).into_response(),
        None => not_found("unknown address", &address),
    })
}

async fn node(
    State(store): State<Store>,
    Path((chain, address)): Path<(String, String)>,
) -> ApiResult<Response> {
    let node = match parse_node(&chain, &address) {
        Ok(n) => n,
        Err(m) => return Ok(bad_request(m)),
    };
    Ok(match node_view(&store, node).await? {
        Some(v) => Json(v).into_response(),
        None => not_found("unknown node", &address),
    })
}

async fn key(State(store): State<Store>, Path(address): Path<String>) -> ApiResult<Response> {
    Ok(match key_view(&store, &address).await? {
        Some(v) => Json(v).into_response(),
        None => not_found("unknown key", &address),
    })
}

#[derive(Deserialize)]
struct SignersQuery {
    min_shared: Option<i64>,
}

#[derive(Serialize)]
struct SignerPairs {
    min_shared: i64,
    pairs: Vec<hermes_core::graph_store::SharedSigners>,
}

async fn signers(State(store): State<Store>, Query(q): Query<SignersQuery>) -> ApiResult<Response> {
    let min_shared = q.min_shared.unwrap_or(2).max(1);
    let pairs = store.shared_signers(min_shared).await?;
    scoped(&store, SignerPairs { min_shared, pairs }).await
}

#[derive(Debug, Default, Deserialize)]
pub struct ChangesQuery {
    /// `24h`, `7d`, a date, or Unix seconds.
    since: Option<String>,
    address: Option<String>,
    limit: Option<i64>,
    /// Changes Hermes made by reading the chain differently. Hidden unless asked for.
    #[serde(default)]
    include_reinterpretations: bool,
}

fn filter(q: &ChangesQuery) -> Result<EventFilter, String> {
    let since = match &q.since {
        None => None,
        Some(s) => Some(
            hermes_core::time::parse_since(s, crate::now())
                .ok_or_else(|| format!("cannot read since={s}"))?,
        ),
    };
    Ok(EventFilter {
        address: q.address.clone(),
        since,
        include_reinterpretations: q.include_reinterpretations,
        limit: Some(q.limit.unwrap_or(100).clamp(1, MAX_EVENTS)),
        ..Default::default()
    })
}

#[derive(Serialize)]
struct Changes {
    count: usize,
    changes: Vec<Change>,
}

#[derive(Serialize)]
struct Change {
    #[serde(flatten)]
    event: StoredEvent,
    text: String,
    when: String,
}

/// Newest first.
async fn changes(State(store): State<Store>, Query(q): Query<ChangesQuery>) -> ApiResult<Response> {
    let f = match filter(&q) {
        Ok(f) => f,
        Err(m) => return Ok(bad_request(m)),
    };
    let changes: Vec<Change> = store
        .events(&f)
        .await?
        .into_iter()
        .map(|event| Change {
            text: event_line(&event),
            when: event_when(&event),
            event,
        })
        .collect();
    scoped(
        &store,
        Changes {
            count: changes.len(),
            changes,
        },
    )
    .await
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The base URL requests arrived at, for the absolute links Atom wants.
fn origin(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("http");
    format!("{proto}://{host}")
}

/// The same changes as an Atom feed, so "watch this" needs a feed reader and nothing else.
pub fn atom(events: &[StoredEvent], origin: &str) -> String {
    let updated = events.iter().map(|e| e.observed_at).max().unwrap_or(0);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\">\n\
         <title>Hermes: upgrade authority changes on Base</title>\n\
         <id>{origin}/v1/changes.atom</id>\n\
         <link rel=\"self\" href=\"{origin}/v1/changes.atom\"/>\n\
         <updated>{}</updated>\n\
         <author><name>Hermes</name></author>\n",
        iso8601(updated)
    );
    for e in events {
        let subject = e.subject.address.to_checksum(None);
        out.push_str(&format!(
            "<entry>\n<title>{}</title>\n<id>{origin}/v1/changes#{}</id>\n\
             <link href=\"{origin}/v1/changes?address={subject}\"/>\n\
             <updated>{}</updated>\n<content type=\"text\">{} Observed {}. Cause: {}.</content>\n\
             </entry>\n",
            xml(&event_line(e)),
            e.id,
            iso8601(e.observed_at),
            xml(&event_line(e)),
            xml(&event_when(e)),
            e.cause.as_str()
        ));
    }
    out.push_str("</feed>\n");
    out
}

async fn changes_atom(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Response> {
    let f = match filter(&q) {
        Ok(f) => f,
        Err(m) => return Ok(bad_request(m)),
    };
    let events = store.events(&f).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")],
        atom(&events, &origin(&headers)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use crate::router;
    use alloy::primitives::{Address, address};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use hermes_core::store::checksum;
    use hermes_core::{AuthorityProbe, Code, MODEL_VERSION, Node, ProxyRecord, Store};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tower::ServiceExt;

    const PROXY: Address = address!("9Caa0e7277ce86A4644F2D10b72561080531b674");
    const BEACON: Address = address!("e68ED13998fd48497EAA3b52e20823605D8d7706");
    const EOA: Address = address!("21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc");
    const SAFE: Address = address!("6454cf0127a153295435160768C85225Cd19Bf15");
    const K1: Address = address!("29d792846a0F2769007aEdb05D4Fe0184Cae7719");

    fn key() -> AuthorityProbe {
        AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        }
    }

    fn row(root: Address, kind: &str, keys: i64, block: i64) -> ProxyRecord {
        ProxyRecord {
            address: checksum(PROXY),
            kind: "beacon".into(),
            beacon: Some(checksum(BEACON)),
            code_size: 508,
            scanned_at: block,
            terminal_authority: Some(checksum(root)),
            terminal_chain: Some("base".into()),
            authority_kind: Some(kind.into()),
            compromise_depth: Some(keys),
            timelock_seconds: Some(0),
            resolution_confidence: Some("high".into()),
            upgrade_path: Some("beacon".into()),
            scanned_block: Some(block),
            model_version: Some(MODEL_VERSION),
            ..Default::default()
        }
    }

    /// The Sep 29 transfer, observed twice.
    async fn app() -> axum::Router {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let before = HashMap::from([
            (
                Node::base(BEACON),
                AuthorityProbe {
                    owner: Some(EOA),
                    ..Default::default()
                },
            ),
            (Node::base(EOA), key()),
        ]);
        let obs = store
            .begin_observation(Some(10), Some(5), 1_000)
            .await
            .unwrap();
        store
            .write_batch(&obs, &[row(EOA, "eoa", 1, 10)], &before)
            .await
            .unwrap();
        let after = HashMap::from([
            (
                Node::base(BEACON),
                AuthorityProbe {
                    owner: Some(SAFE),
                    ..Default::default()
                },
            ),
            (
                Node::base(SAFE),
                AuthorityProbe {
                    owners: Some(vec![K1, EOA]),
                    threshold: Some(2),
                    ..Default::default()
                },
            ),
            (Node::base(EOA), key()),
            (Node::base(K1), key()),
        ]);
        let obs = store
            .begin_observation(Some(20), Some(6), 2_000)
            .await
            .unwrap();
        store
            .write_batch(&obs, &[row(SAFE, "safe", 2, 20)], &after)
            .await
            .unwrap();
        router(store, PathBuf::from("static"))
    }

    async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String) {
        let r = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let bytes = r.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn json(app: &axum::Router, uri: &str) -> serde_json::Value {
        let (status, body) = get(app, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        serde_json::from_str(&body).unwrap()
    }

    #[tokio::test]
    async fn a_proxy_comes_with_the_path_to_its_root_and_the_scope_of_the_answer() {
        let app = app().await;
        let v = json(&app, &format!("/v1/proxies/{PROXY:#x}")).await;
        assert_eq!(v["upgrade_entry"], "beacon");
        let tree = &v["tree"];
        assert_eq!(tree["relation"], "beacon");
        assert_eq!(tree["node"]["address"], checksum(BEACON));
        assert_eq!(tree["children"][0]["kind"], "safe");
        assert_eq!(tree["children"][0]["children"].as_array().unwrap().len(), 2);
        assert_eq!(v["replayed"]["matches_row"], true);
        assert_eq!(v["scope"]["index"], "sample");
        assert_eq!(v["scope"]["as_of"]["base_block"], 20);
    }

    #[tokio::test]
    async fn a_safe_controls_what_it_owns_and_one_signer_only_takes_part() {
        let app = app().await;
        let safe = json(&app, &format!("/v1/nodes/base/{SAFE:#x}")).await;
        assert_eq!(
            safe["blast_radius"]["controls"].as_array().unwrap().len(),
            1
        );
        let signer = json(&app, &format!("/v1/keys/{K1:#x}")).await;
        assert_eq!(signer["owner_of"].as_array().unwrap().len(), 1);
        let radius = &signer["blast_radius"];
        assert!(radius["controls"].as_array().unwrap().is_empty());
        assert_eq!(radius["participates"].as_array().unwrap().len(), 1);
        assert_eq!(
            radius["compromised"].as_array().unwrap().len(),
            2,
            "a key is taken on both chains"
        );
    }

    /// The history of the beacon: its old owner edge closed, the new one open.
    #[tokio::test]
    async fn a_node_shows_the_edges_it_has_lost() {
        let app = app().await;
        let beacon = json(&app, &format!("/v1/nodes/base/{BEACON:#x}")).await;
        let history = beacon["history"].as_array().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["to"]["address"], checksum(EOA));
        assert_eq!(history[0]["closed_block"], 20);
    }

    #[tokio::test]
    async fn changes_state_the_fact_and_the_direction_and_nothing_else() {
        let app = app().await;
        let v = json(&app, "/v1/changes").await;
        assert_eq!(v["count"], 3);
        let texts: Vec<&str> = v["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap())
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.ends_with("keys required 1 → 2 (rose)"))
        );
        assert!(
            v["changes"][0]["when"]
                .as_str()
                .unwrap()
                .starts_with("between base blocks 10 and 20")
        );
        for t in &texts {
            for judgment in [
                "risk", "danger", "severity", "critical", "vulnerab", "unsafe",
            ] {
                assert!(!t.to_lowercase().contains(judgment), "{t}");
            }
        }
        let later = json(&app, "/v1/changes?since=2001").await;
        assert_eq!(later["count"], 0);
    }

    #[tokio::test]
    async fn the_feed_is_atom_with_one_entry_per_change() {
        let app = app().await;
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/changes.atom")
                    .header("host", "hermes.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            r.headers()["content-type"],
            "application/atom+xml; charset=utf-8"
        );
        let body =
            String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        assert!(body.starts_with("<?xml"));
        assert_eq!(body.matches("<entry>").count(), 3);
        assert!(body.contains("<id>http://hermes.example/v1/changes.atom</id>"));
        assert!(body.contains("<updated>1970-01-01T00:33:20Z</updated>"));
    }

    #[tokio::test]
    async fn a_root_on_one_chain_is_its_own_resource() {
        let app = app().await;
        let v = json(&app, &format!("/v1/authorities/base/{SAFE:#x}")).await;
        assert_eq!(v["authority"]["proxy_count"], 1);
        assert_eq!(v["node"]["node"]["kind"], "safe");
        let (missing, _) = get(&app, &format!("/v1/authorities/ethereum/{SAFE:#x}")).await;
        assert_eq!(missing, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_malformed_request_is_400_and_an_unknown_one_is_404() {
        let app = app().await;
        for uri in [
            "/v1/nodes/solana/0x00".to_string(),
            format!("/v1/nodes/base/{}", "0xnot"),
            "/v1/changes?since=yesterday".to_string(),
        ] {
            assert_eq!(get(&app, &uri).await.0, StatusCode::BAD_REQUEST, "{uri}");
        }
        let nobody = "0x00000000000000000000000000000000000000ff";
        for uri in [
            format!("/v1/nodes/base/{nobody}"),
            format!("/v1/proxies/{nobody}"),
            format!("/v1/keys/{nobody}"),
        ] {
            assert_eq!(get(&app, &uri).await.0, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn every_collection_says_what_it_is_a_sample_of() {
        let app = app().await;
        for uri in [
            "/v1/coverage",
            "/v1/authorities",
            "/v1/signers",
            "/v1/changes",
        ] {
            let v = json(&app, uri).await;
            assert_eq!(v["scope"]["index"], "sample", "{uri}");
            assert_eq!(
                v["scope"]["first_seen_means"],
                "first seen by hermes, not first set on chain"
            );
        }
    }
}
