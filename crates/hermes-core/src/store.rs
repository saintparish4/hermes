//! SQLite persistence.
//!
//! Deliberately uses the runtime `sqlx::query` API rather than the `query!` macros: the macros
//! need a live `DATABASE_URL` (or a checked-in `.sqlx` cache) at *compile* time, which would
//! make `cargo build` fail in CI for a project whose CI has no database. Runtime queries cost
//! compile-time verification and buy a workspace that always builds.

use crate::classify::ProxyKind;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRecord {
    /// EIP-55 checksummed.
    pub address: String,
    pub label: Option<String>,
    pub kind: String,
    pub implementation: Option<String>,
    pub admin: Option<String>,
    pub beacon: Option<String>,
    pub code_size: i64,
    /// Unix seconds.
    pub scanned_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub total_scanned: i64,
    pub covered_proxies: i64,
    pub by_kind: Vec<(String, i64)>,
    pub distinct_admins: i64,
    pub last_scan: Option<i64>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS proxy (
    address     TEXT PRIMARY KEY NOT NULL,
    label       TEXT,
    kind        TEXT NOT NULL,
    impl_addr   TEXT,
    admin_addr  TEXT,
    beacon_addr TEXT,
    code_size   INTEGER NOT NULL DEFAULT 0,
    scanned_at  INTEGER NOT NULL
);
-- Exposure gets grouped by authority, and the admin column is what it groups on, so it is
-- indexed from the start.
CREATE INDEX IF NOT EXISTS idx_proxy_admin ON proxy(admin_addr);
CREATE INDEX IF NOT EXISTS idx_proxy_kind  ON proxy(kind);
"#;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// `url` is a SQLite URL, e.g. `sqlite://hermes.db`. The file is created if absent.
    pub async fn open(url: &str) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Upsert a batch in one transaction. Re-scans overwrite prior rows for the same address,
    /// with one exception.
    ///
    /// A row that once had code can never legitimately come back with none: deployed code is
    /// immutable. So an incoming `code_size` of zero against a stored non-zero one is the
    /// node lying, not the chain changing, and the whole update is skipped rather than
    /// allowed to rewrite a live proxy as an EOA. `scanned_at` does not move either, which is
    /// what makes the surviving row visibly stale instead of silently wrong.
    ///
    /// This deliberately does not guard kind-to-kind transitions. A proxy really can go from
    /// transparent to UUPS when an admin renounces, and refusing that would trade a rare
    /// wrong answer for a common one.
    pub async fn upsert_many(&self, records: &[ProxyRecord]) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        for r in records {
            let res = sqlx::query(
                r#"INSERT INTO proxy (address,label,kind,impl_addr,admin_addr,beacon_addr,code_size,scanned_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                   ON CONFLICT(address) DO UPDATE SET
                     label=COALESCE(excluded.label, proxy.label),
                     kind=excluded.kind,
                     impl_addr=excluded.impl_addr,
                     admin_addr=excluded.admin_addr,
                     beacon_addr=excluded.beacon_addr,
                     code_size=excluded.code_size,
                     scanned_at=excluded.scanned_at
                   WHERE NOT (proxy.code_size > 0 AND excluded.code_size = 0)"#,
            )
            .bind(&r.address).bind(&r.label).bind(&r.kind)
            .bind(&r.implementation).bind(&r.admin).bind(&r.beacon)
            .bind(r.code_size).bind(r.scanned_at)
            .execute(&mut *tx).await?;
            n += res.rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Proxies only (covered patterns), most recently scanned first.
    pub async fn list_proxies(&self, only_covered: bool) -> anyhow::Result<Vec<ProxyRecord>> {
        let sql = if only_covered {
            "SELECT * FROM proxy WHERE kind IN ('transparent','uups','beacon','eip1822','admin_only') \
             ORDER BY code_size DESC"
        } else {
            "SELECT * FROM proxy ORDER BY code_size DESC"
        };
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_record).collect())
    }

    pub async fn get_proxy(&self, address: &str) -> anyhow::Result<Option<ProxyRecord>> {
        let row = sqlx::query("SELECT * FROM proxy WHERE address = ?1 COLLATE NOCASE")
            .bind(address)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_to_record))
    }

    /// Groups by the *immediate* admin. This becomes `terminal_authority` once resolution
    /// lands, and the shape of the result does not change when it does.
    pub async fn admin_rollup(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let rows = sqlx::query(
            "SELECT admin_addr, COUNT(*) c FROM proxy \
             WHERE admin_addr IS NOT NULL GROUP BY admin_addr ORDER BY c DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get::<String, _>("admin_addr"), r.get::<i64, _>("c")))
            .collect())
    }

    pub async fn coverage(&self) -> anyhow::Result<Coverage> {
        let by_kind: Vec<(String, i64)> =
            sqlx::query("SELECT kind, COUNT(*) c FROM proxy GROUP BY kind ORDER BY c DESC")
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| (r.get::<String, _>("kind"), r.get::<i64, _>("c")))
                .collect();
        let covered = by_kind
            .iter()
            .filter(|(k, _)| {
                matches!(
                    k.as_str(),
                    "transparent" | "uups" | "beacon" | "eip1822" | "admin_only"
                )
            })
            .map(|(_, c)| c)
            .sum();
        let row = sqlx::query(
            "SELECT COUNT(*) total, COUNT(DISTINCT admin_addr) admins, MAX(scanned_at) last FROM proxy",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Coverage {
            total_scanned: row.get::<i64, _>("total"),
            covered_proxies: covered,
            by_kind,
            distinct_admins: row.get::<i64, _>("admins"),
            last_scan: row.get::<Option<i64>, _>("last"),
        })
    }
}

fn row_to_record(r: &sqlx::sqlite::SqliteRow) -> ProxyRecord {
    ProxyRecord {
        address: r.get("address"),
        label: r.get("label"),
        kind: r.get("kind"),
        implementation: r.get("impl_addr"),
        admin: r.get("admin_addr"),
        beacon: r.get("beacon_addr"),
        code_size: r.get("code_size"),
        scanned_at: r.get("scanned_at"),
    }
}

/// Store addresses EIP-55 checksummed so the API and the seed file agree byte for byte.
pub fn checksum(a: Address) -> String {
    a.to_checksum(None)
}

pub fn kind_str(k: ProxyKind) -> String {
    k.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem() -> Store {
        Store::open("sqlite::memory:").await.unwrap()
    }

    fn rec(addr: &str, kind: &str, admin: Option<&str>) -> ProxyRecord {
        ProxyRecord {
            address: addr.into(),
            label: None,
            kind: kind.into(),
            implementation: Some("0xImpl".into()),
            admin: admin.map(Into::into),
            beacon: None,
            code_size: 100,
            scanned_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn upsert_is_idempotent_and_updates_in_place() {
        let s = mem().await;
        s.upsert_many(&[rec("0xA", "transparent", Some("0xAdmin"))])
            .await
            .unwrap();
        s.upsert_many(&[rec("0xA", "uups", None)]).await.unwrap();
        let all = s.list_proxies(false).await.unwrap();
        assert_eq!(all.len(), 1, "same address must not duplicate");
        assert_eq!(all[0].kind, "uups", "re-scan must overwrite classification");
        assert_eq!(all[0].admin, None);
    }

    #[tokio::test]
    async fn label_survives_a_rescan_that_has_none() {
        let s = mem().await;
        let mut r = rec("0xA", "transparent", Some("0xAdmin"));
        r.label = Some("Aerodrome".into());
        s.upsert_many(&[r]).await.unwrap();
        s.upsert_many(&[rec("0xA", "transparent", Some("0xAdmin"))])
            .await
            .unwrap();
        let all = s.list_proxies(false).await.unwrap();
        assert_eq!(
            all[0].label.as_deref(),
            Some("Aerodrome"),
            "COALESCE must preserve labels"
        );
    }

    #[tokio::test]
    async fn only_covered_filters_out_eoa_and_zeppelinos() {
        let s = mem().await;
        s.upsert_many(&[
            rec("0xA", "transparent", Some("0xAdmin")),
            rec("0xB", "zeppelin_os", None),
            rec("0xC", "eoa", None),
            rec("0xD", "not_upgradeable", None),
        ])
        .await
        .unwrap();
        assert_eq!(s.list_proxies(true).await.unwrap().len(), 1);
        assert_eq!(s.list_proxies(false).await.unwrap().len(), 4);
    }

    /// The grouping property the whole aggregation rests on: many proxies, one admin, one row.
    #[tokio::test]
    async fn many_proxies_under_one_admin_collapse_to_one_authority_row() {
        let s = mem().await;
        let rows: Vec<_> = (0..7)
            .map(|i| rec(&format!("0x{i:040x}"), "transparent", Some("0xSharedAdmin")))
            .collect();
        s.upsert_many(&rows).await.unwrap();
        let rollup = s.admin_rollup().await.unwrap();
        assert_eq!(
            rollup.len(),
            1,
            "one admin must produce exactly one authority row"
        );
        assert_eq!(rollup[0], ("0xSharedAdmin".to_string(), 7));
    }

    /// The failure this exists to stop: a node briefly answers `0x` for a contract that has
    /// code, the scan believes it, and a live upgrade authority is republished as an EOA.
    #[tokio::test]
    async fn a_contract_losing_its_code_never_overwrites_a_stored_proxy() {
        let s = mem().await;
        s.upsert_many(&[rec("0xA", "transparent", Some("0xAdmin"))])
            .await
            .unwrap();

        let mut blank = rec("0xA", "eoa", None);
        blank.code_size = 0;
        blank.implementation = None;
        blank.scanned_at = 1_800_000_000;
        s.upsert_many(&[blank]).await.unwrap();

        let all = s.list_proxies(false).await.unwrap();
        assert_eq!(all[0].kind, "transparent", "deployed code cannot disappear");
        assert_eq!(all[0].admin.as_deref(), Some("0xAdmin"));
        assert_eq!(
            all[0].scanned_at, 1_700_000_000,
            "the surviving row must read as stale, not as freshly confirmed"
        );
    }

    /// The guard must not buy safety by freezing the store: an admin renouncing is a real
    /// transition and has to land.
    #[tokio::test]
    async fn a_renounced_admin_still_updates_the_stored_kind() {
        let s = mem().await;
        s.upsert_many(&[rec("0xA", "transparent", Some("0xAdmin"))])
            .await
            .unwrap();
        s.upsert_many(&[rec("0xA", "uups", None)]).await.unwrap();
        let all = s.list_proxies(false).await.unwrap();
        assert_eq!(all[0].kind, "uups");
        assert_eq!(all[0].admin, None);
    }

    #[tokio::test]
    async fn coverage_counts_only_covered_patterns() {
        let s = mem().await;
        s.upsert_many(&[
            rec("0xA", "transparent", Some("0xAdmin")),
            rec("0xB", "uups", None),
            rec("0xC", "zeppelin_os", None),
            rec("0xD", "eoa", None),
        ])
        .await
        .unwrap();
        let c = s.coverage().await.unwrap();
        assert_eq!(c.total_scanned, 4);
        assert_eq!(
            c.covered_proxies, 2,
            "zeppelin_os and eoa are not covered proxies"
        );
    }
}
