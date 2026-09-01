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

#[derive(Debug, Deserialize)]
pub struct ProxyQuery {
    /// `?all=true` includes EOAs, non-proxies and the ZeppelinOS pattern.
    #[serde(default)]
    pub all: bool,
}

#[derive(Serialize)]
struct ProxyList {
    count: usize,
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
    let proxies = store.list_proxies(!q.all).await?;
    Ok(Json(ProxyList {
        count: proxies.len(),
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

async fn get_authority(
    State(store): State<Store>,
    Path(address): Path<String>,
) -> ApiResult<Response> {
    let proxies = store.proxies_for_authority(&address).await?;
    let Some(authority) = store
        .authority_rollup()
        .await?
        .into_iter()
        .find(|a| a.address.eq_ignore_ascii_case(&address))
    else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "unknown authority", "address": address })),
        )
            .into_response());
    };
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
