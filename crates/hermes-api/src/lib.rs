//! The JSON API and static-file server. This binary is the only API; the frontend has no
//! server component.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use hermes_core::Store;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

/// `anyhow::Error` → 500, so handlers can use `?`.
pub struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// Rows per page when a request does not say.
pub const DEFAULT_PAGE: i64 = 100;
/// The most rows one request can have. At the PRD's "good" bar of 5,000 proxies an unpaged
/// `/proxies` is a multi-megabyte response, a failure no correctness test would ever catch.
pub const MAX_PAGE: i64 = 1_000;

#[derive(Debug, Deserialize)]
pub struct ProxyQuery {
    /// `?all=true` includes EOAs, non-proxies and the ZeppelinOS pattern.
    #[serde(default)]
    pub all: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuthorityQuery {
    /// `base` or `ethereum`. Only needed when one address is an authority on both.
    pub chain: Option<String>,
}

#[derive(Serialize)]
struct ProxyList {
    /// Rows in this page.
    count: usize,
    /// Rows in all pages.
    total: i64,
    limit: i64,
    offset: i64,
    proxies: Vec<hermes_core::ProxyRecord>,
}

#[derive(Serialize)]
struct AuthorityList {
    count: usize,
    authorities: Vec<hermes_core::store::AuthorityRow>,
}

#[derive(Serialize)]
struct AuthorityDetail {
    authority: hermes_core::store::AuthorityRow,
    proxies: Vec<hermes_core::ProxyRecord>,
}

pub fn router(store: Store, static_dir: PathBuf) -> Router {
    let index = static_dir.join("index.html");
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/proxies", get(list_proxies))
        // axum 0.8 uses `{param}`, not the `:param` syntax of 0.7 and earlier.
        .route("/proxies/{address}", get(get_proxy))
        .route("/authorities", get(list_authorities))
        .route("/authorities/{address}", get(get_authority))
        .route("/coverage", get(coverage))
        .fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(index)))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(store)
}

async fn list_proxies(
    State(store): State<Store>,
    Query(q): Query<ProxyQuery>,
) -> ApiResult<Json<ProxyList>> {
    let limit = q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0).max(0);
    let (proxies, total) = store.proxies_page(!q.all, limit, offset).await?;
    Ok(Json(ProxyList {
        count: proxies.len(),
        total,
        limit,
        offset,
        proxies,
    }))
}

async fn get_proxy(State(store): State<Store>, Path(address): Path<String>) -> ApiResult<Response> {
    match store.get_proxy(&address).await? {
        Some(p) => Ok(Json(p).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "unknown address", "address": address })),
        )
            .into_response()),
    }
}

/// Ranked by how many proxies each resolved root controls.
///
/// Proxies whose chain did not resolve are absent here on purpose. Bucketing them under a
/// placeholder authority would invent a row; `/coverage` reports them as the gap instead.
async fn list_authorities(State(store): State<Store>) -> ApiResult<Json<AuthorityList>> {
    let authorities = store.authority_rollup().await?;
    Ok(Json(AuthorityList {
        count: authorities.len(),
        authorities,
    }))
}

/// One authority and every proxy it controls.
///
/// The same address can be a root on Base and on Ethereum, and those are two authorities. When
/// that happens and no `?chain=` says which, the answer is a 409 naming both rather than
/// whichever row happened to sort first.
async fn get_authority(
    State(store): State<Store>,
    Path(address): Path<String>,
    Query(q): Query<AuthorityQuery>,
) -> ApiResult<Response> {
    let mut matches: Vec<_> = store
        .authority_rollup()
        .await?
        .into_iter()
        .filter(|a| a.address.eq_ignore_ascii_case(&address))
        .filter(|a| q.chain.is_none() || a.chain == q.chain)
        .collect();
    let authority = match matches.len() {
        0 => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "unknown authority", "address": address })),
            )
                .into_response());
        }
        1 => matches.remove(0),
        _ => {
            let chains: Vec<_> = matches.iter().map(|a| a.chain.clone()).collect();
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "address is an authority on more than one chain; pass ?chain=",
                    "address": address,
                    "chains": chains,
                })),
            )
                .into_response());
        }
    };
    let proxies = store
        .proxies_for_authority(&address, authority.chain.as_deref())
        .await?;
    Ok(Json(AuthorityDetail { authority, proxies }).into_response())
}

/// Bind and serve until Ctrl-C. Lives here so the CLI never needs to depend on axum.
pub async fn serve(store: Store, static_dir: PathBuf, port: u16) -> anyhow::Result<()> {
    let app = router(store, static_dir);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    println!("hermes serving on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}

async fn coverage(State(store): State<Store>) -> ApiResult<Json<hermes_core::store::Coverage>> {
    Ok(Json(store.coverage().await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn app_with_rows() -> Router {
        let store = Store::open("sqlite::memory:").await.unwrap();
        store
            .upsert_many(&[
                hermes_core::ProxyRecord {
                    address: "0x402E0d314fD6F55348Df7CC478bAb811826e3e91".into(),
                    label: Some("example".into()),
                    kind: "transparent".into(),
                    implementation: Some("0x6d9d".into()),
                    admin: Some("0x31e9".into()),
                    code_size: 1971,
                    scanned_at: 1_700_000_000,
                    terminal_authority: Some("0xSafe".into()),
                    terminal_chain: Some("base".into()),
                    authority_kind: Some("safe".into()),
                    compromise_depth: Some(2),
                    timelock_seconds: Some(0),
                    resolution_confidence: Some("high".into()),
                    ..Default::default()
                },
                // A proxy under a different immediate admin but the same root: these two must
                // come back as one authority, not two.
                hermes_core::ProxyRecord {
                    address: "0xBeef".into(),
                    kind: "uups".into(),
                    admin: Some("0xOtherProxyAdmin".into()),
                    code_size: 900,
                    scanned_at: 1_700_000_000,
                    terminal_authority: Some("0xSafe".into()),
                    terminal_chain: Some("base".into()),
                    authority_kind: Some("safe".into()),
                    compromise_depth: Some(2),
                    timelock_seconds: Some(0),
                    resolution_confidence: Some("high".into()),
                    ..Default::default()
                },
                // Unresolved on purpose: it must stay out of the ranking and stay counted.
                hermes_core::ProxyRecord {
                    address: "0xMystery".into(),
                    kind: "transparent".into(),
                    admin: Some("0xUnknownThing".into()),
                    code_size: 500,
                    scanned_at: 1_700_000_000,
                    ..Default::default()
                },
                hermes_core::ProxyRecord {
                    address: "0xDead".into(),
                    kind: "eoa".into(),
                    code_size: 0,
                    scanned_at: 1_700_000_000,
                    ..Default::default()
                },
            ])
            .await
            .unwrap();
        router(store, PathBuf::from("static"))
    }

    async fn body_json(r: Response) -> serde_json::Value {
        let b = r.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&b).unwrap()
    }

    #[tokio::test]
    async fn proxies_excludes_non_proxies_by_default() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/proxies")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["count"], 3, "the EOA row must not appear in /proxies");
    }

    #[tokio::test]
    async fn proxies_all_includes_everything() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/proxies?all=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert_eq!(v["count"], 4);
    }

    #[tokio::test]
    async fn proxies_come_in_pages_that_say_how_many_there_are_in_all() {
        let app = app_with_rows().await;
        let get = |uri: &'static str| {
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        };
        let first = body_json(get("/proxies?limit=2").await.unwrap()).await;
        assert_eq!(first["count"], 2);
        assert_eq!(first["total"], 3);
        let rest = body_json(get("/proxies?limit=2&offset=2").await.unwrap()).await;
        assert_eq!(rest["count"], 1);
        assert_ne!(
            first["proxies"][0]["address"],
            rest["proxies"][0]["address"]
        );
        let silly = body_json(get("/proxies?limit=999999&offset=-5").await.unwrap()).await;
        assert_eq!(silly["limit"], MAX_PAGE, "a page is never unbounded");
        assert_eq!(silly["offset"], 0);
    }

    #[tokio::test]
    async fn unknown_address_is_404_not_500() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/proxies/0xnope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn address_lookup_is_case_insensitive() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/proxies/0x402e0d314fd6f55348df7cc478bab811826e3e91")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "checksummed storage must not break lookups"
        );
    }

    #[tokio::test]
    async fn coverage_reports_honestly() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/coverage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert_eq!(v["total_scanned"], 4);
        assert_eq!(v["covered_proxies"], 3);
        assert_eq!(
            v["resolved_proxies"], 2,
            "the unresolved proxy must be visible as a gap, not hidden"
        );
        assert_eq!(v["distinct_authorities"], 1);
    }

    /// The grouping that the product rests on, asserted through the JSON rather than only in
    /// the store: two proxies, two different admins, one row.
    #[tokio::test]
    async fn authorities_group_on_the_resolved_root_not_the_immediate_admin() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/authorities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert_eq!(v["count"], 1);
        assert_eq!(v["authorities"][0]["address"], "0xSafe");
        assert_eq!(v["authorities"][0]["chain"], "base");
        assert_eq!(v["authorities"][0]["proxy_count"], 2);
        assert_eq!(v["authorities"][0]["compromise_depth"], 2);
    }

    /// Null is not zero, and it has to survive all the way into the response body. A depth
    /// serialized as 0 would read as "costs nothing to compromise".
    #[tokio::test]
    async fn an_unknown_depth_serializes_as_null_not_zero() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        store
            .upsert_many(&[hermes_core::ProxyRecord {
                address: "0xA".into(),
                kind: "transparent".into(),
                admin: Some("0xCycle".into()),
                code_size: 10,
                scanned_at: 1,
                terminal_authority: Some("0xCycle".into()),
                authority_kind: Some("ownable".into()),
                compromise_depth: None,
                timelock_seconds: Some(0),
                resolution_confidence: Some("medium".into()),
                ..Default::default()
            }])
            .await
            .unwrap();
        let app = router(store, PathBuf::from("static"));
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/authorities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(r).await;
        assert!(v["authorities"][0]["compromise_depth"].is_null());
        assert_eq!(v["authorities"][0]["timelock_seconds"], 0);
    }

    #[tokio::test]
    async fn one_authority_lists_every_proxy_it_controls() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/authorities/0xSafe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["authority"]["proxy_count"], 2);
        assert_eq!(v["proxies"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_unknown_authority_is_404_not_500() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/authorities/0xNobody")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    /// The corrected predeploy row: a Safe that lives on Ethereum must say so in the JSON, or a
    /// reader will go looking for it on Basescan and find nothing.
    #[tokio::test]
    async fn an_authority_on_ethereum_says_so() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        store
            .upsert_many(&[hermes_core::ProxyRecord {
                address: "0x4200000000000000000000000000000000000015".into(),
                kind: "transparent".into(),
                admin: Some("0x4200000000000000000000000000000000000018".into()),
                code_size: 10,
                scanned_at: 1,
                terminal_authority: Some("0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c".into()),
                terminal_chain: Some("ethereum".into()),
                authority_kind: Some("safe".into()),
                compromise_depth: Some(11),
                timelock_seconds: Some(0),
                resolution_confidence: Some("high".into()),
                ..Default::default()
            }])
            .await
            .unwrap();
        let v = body_json(
            router(store, PathBuf::from("static"))
                .oneshot(
                    Request::builder()
                        .uri("/authorities")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(v["authorities"][0]["chain"], "ethereum");
        assert_eq!(v["authorities"][0]["kind"], "safe");
        assert_eq!(v["authorities"][0]["compromise_depth"], 11);
    }

    /// One address, a root on both chains: without `?chain=` there is no single right answer,
    /// so the API names the ambiguity instead of picking.
    #[tokio::test]
    async fn an_address_that_is_an_authority_on_two_chains_needs_a_chain() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let row = |address: &str, chain: &str| hermes_core::ProxyRecord {
            address: address.into(),
            kind: "transparent".into(),
            code_size: 10,
            scanned_at: 1,
            terminal_authority: Some("0xSame".into()),
            terminal_chain: Some(chain.into()),
            authority_kind: Some("safe".into()),
            ..Default::default()
        };
        store
            .upsert_many(&[row("0xA", "base"), row("0xB", "ethereum")])
            .await
            .unwrap();
        let app = router(store, PathBuf::from("static"));
        let get = |uri: &'static str| {
            app.clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        };
        assert_eq!(
            get("/authorities/0xSame").await.unwrap().status(),
            StatusCode::CONFLICT
        );
        let r = get("/authorities/0xSame?chain=ethereum").await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["authority"]["chain"], "ethereum");
        assert_eq!(v["proxies"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn healthz_is_plain_ok() {
        let app = app_with_rows().await;
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
}
