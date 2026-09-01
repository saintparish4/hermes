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
struct AuthorityRow {
    admin: String,
    proxy_count: i64,
}

#[derive(Serialize)]
struct AuthorityList {
    note: &'static str,
    count: usize,
    authorities: Vec<AuthorityRow>,
}

pub fn router(store: Store, static_dir: PathBuf) -> Router {
    let index = static_dir.join("index.html");
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/proxies", get(list_proxies))
        // axum 0.8 uses `{param}`, not the `:param` syntax of 0.7 and earlier.
        .route("/proxies/{address}", get(get_proxy))
        .route("/authorities", get(list_authorities))
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

/// Groups by each proxy's *immediate* admin, because authority resolution does not exist
/// yet. Swapping the grouping key to `terminal_authority` will not change this route's
/// contract, which is why I shipped the route before the resolver.
async fn list_authorities(State(store): State<Store>) -> ApiResult<Json<AuthorityList>> {
    let rows = store.admin_rollup().await?;
    Ok(Json(AuthorityList {
        note: "grouped by immediate admin, not resolved terminal authority",
        count: rows.len(),
        authorities: rows
            .into_iter()
            .map(|(admin, proxy_count)| AuthorityRow { admin, proxy_count })
            .collect(),
    }))
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
                    beacon: None,
                    code_size: 1971,
                    scanned_at: 1_700_000_000,
                },
                hermes_core::ProxyRecord {
                    address: "0xDead".into(),
                    label: None,
                    kind: "eoa".into(),
                    implementation: None,
                    admin: None,
                    beacon: None,
                    code_size: 0,
                    scanned_at: 1_700_000_000,
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
        assert_eq!(v["count"], 1, "the EOA row must not appear in /proxies");
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
        assert_eq!(v["count"], 2);
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
        assert_eq!(v["total_scanned"], 2);
        assert_eq!(v["covered_proxies"], 1);
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
