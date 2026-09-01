//! SQLite persistence.
//!
//! Deliberately uses the runtime `sqlx::query` API rather than the `query!` macros: the macros
//! need a live `DATABASE_URL` (or a checked-in `.sqlx` cache) at *compile* time, which would
//! make `cargo build` fail in CI for a project whose CI has no database. Runtime queries cost
//! compile-time verification and buy a workspace that always builds.

use crate::classify::ProxyKind;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// The root of the ownership chain, not the immediate admin. This is the column exposure
    /// groups on, so it is what makes two proxies under different ProxyAdmins owned by one
    /// Safe count as a single authority.
    pub terminal_authority: Option<String>,
    pub authority_kind: Option<String>,
    /// Null when the chain could not be resolved. Never zero — that would read as free.
    pub compromise_depth: Option<i64>,
    pub timelock_seconds: Option<i64>,
    pub resolution_confidence: Option<String>,
}

/// One resolved authority and what it controls.
#[derive(Debug, Clone, Serialize)]
pub struct AuthorityRow {
    pub address: String,
    pub proxy_count: i64,
    pub kind: Option<String>,
    /// Null when the chain could not be resolved. Never rendered as zero.
    pub compromise_depth: Option<i64>,
    pub timelock_seconds: Option<i64>,
    pub confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    pub total_scanned: i64,
    pub covered_proxies: i64,
    pub by_kind: Vec<(String, i64)>,
    pub distinct_admins: i64,
    /// Proxies whose admin resolved to a root. The gap against `covered_proxies` is the
    /// honest measure of how much of the authority graph I actually understand.
    pub resolved_proxies: i64,
    pub distinct_authorities: i64,
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
    scanned_at  INTEGER NOT NULL,
    terminal_authority     TEXT,
    authority_kind         TEXT,
    compromise_depth       INTEGER,
    timelock_seconds       INTEGER,
    resolution_confidence  TEXT
);
-- Exposure gets grouped by authority, and the admin column is what it groups on, so it is
-- indexed from the start.
CREATE INDEX IF NOT EXISTS idx_proxy_admin ON proxy(admin_addr);
CREATE INDEX IF NOT EXISTS idx_proxy_kind  ON proxy(kind);
CREATE INDEX IF NOT EXISTS idx_proxy_terminal ON proxy(terminal_authority);
"#;

/// Columns added after the first deployment. SQLite has no `ADD COLUMN IF NOT EXISTS`, and
/// there is a populated database on a mounted volume, so the existing rows have to survive
/// this rather than be recreated.
/// Column name paired with the exact statement that adds it. Both are static so no SQL is
/// ever assembled at runtime.
const ADDED_COLUMNS: &[(&str, &str)] = &[
    (
        "terminal_authority",
        "ALTER TABLE proxy ADD COLUMN terminal_authority TEXT",
    ),
    (
        "authority_kind",
        "ALTER TABLE proxy ADD COLUMN authority_kind TEXT",
    ),
    (
        "compromise_depth",
        "ALTER TABLE proxy ADD COLUMN compromise_depth INTEGER",
    ),
    (
        "timelock_seconds",
        "ALTER TABLE proxy ADD COLUMN timelock_seconds INTEGER",
    ),
    (
        "resolution_confidence",
        "ALTER TABLE proxy ADD COLUMN resolution_confidence TEXT",
    ),
];

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// `url` is a SQLite URL, e.g. `sqlite://hermes.db`. The file is created if absent.
    pub async fn open(url: &str) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            // A scan and the server run against this file at the same time in the deployed
            // container. WAL is what lets the server keep answering reads while a scan
            // commits, instead of both sides taking turns behind a lock.
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        migrate(&pool).await?;
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
                r#"INSERT INTO proxy (address,label,kind,impl_addr,admin_addr,beacon_addr,code_size,scanned_at,
                                      terminal_authority,authority_kind,compromise_depth,timelock_seconds,resolution_confidence)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                   ON CONFLICT(address) DO UPDATE SET
                     label=COALESCE(excluded.label, proxy.label),
                     kind=excluded.kind,
                     impl_addr=excluded.impl_addr,
                     admin_addr=excluded.admin_addr,
                     beacon_addr=excluded.beacon_addr,
                     code_size=excluded.code_size,
                     scanned_at=excluded.scanned_at,
                     -- Resolution runs after classification and can be absent on a run that
                     -- only re-classified. COALESCE keeps the last known answer rather than
                     -- blanking a resolved authority back to unresolved.
                     terminal_authority=COALESCE(excluded.terminal_authority, proxy.terminal_authority),
                     authority_kind=COALESCE(excluded.authority_kind, proxy.authority_kind),
                     compromise_depth=COALESCE(excluded.compromise_depth, proxy.compromise_depth),
                     timelock_seconds=COALESCE(excluded.timelock_seconds, proxy.timelock_seconds),
                     resolution_confidence=COALESCE(excluded.resolution_confidence, proxy.resolution_confidence)
                   WHERE NOT (proxy.code_size > 0 AND excluded.code_size = 0)"#,
            )
            .bind(&r.address).bind(&r.label).bind(&r.kind)
            .bind(&r.implementation).bind(&r.admin).bind(&r.beacon)
            .bind(r.code_size).bind(r.scanned_at)
            .bind(&r.terminal_authority).bind(&r.authority_kind)
            .bind(r.compromise_depth).bind(r.timelock_seconds).bind(&r.resolution_confidence)
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

    /// Groups by the *immediate* admin. Kept alongside the resolved rollup so the two can be
    /// compared, which is how I can see resolution actually collapsing distinct admins into
    /// one authority rather than just trusting that it did.
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

    /// Groups by the resolved root. Two proxies under different ProxyAdmin contracts owned by
    /// one Safe collapse to a single row here, which is the entire point of resolving at all.
    ///
    /// Unresolved proxies are excluded rather than bucketed under a placeholder — they are
    /// reported by `coverage` instead, so they stay visible without being counted as an
    /// authority I understand.
    pub async fn authority_rollup(&self) -> anyhow::Result<Vec<AuthorityRow>> {
        let rows = sqlx::query(
            "SELECT terminal_authority a, COUNT(*) c, \
                    MAX(authority_kind) k, \
                    MAX(compromise_depth) d, \
                    MAX(timelock_seconds) t, \
                    MIN(resolution_confidence) conf \
             FROM proxy WHERE terminal_authority IS NOT NULL \
             GROUP BY terminal_authority ORDER BY c DESC, a ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| AuthorityRow {
                address: r.get("a"),
                proxy_count: r.get("c"),
                kind: r.get("k"),
                compromise_depth: r.get("d"),
                timelock_seconds: r.get("t"),
                confidence: r.get("conf"),
            })
            .collect())
    }

    /// Every proxy that resolves to one authority.
    pub async fn proxies_for_authority(&self, authority: &str) -> anyhow::Result<Vec<ProxyRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM proxy WHERE terminal_authority = ?1 COLLATE NOCASE ORDER BY code_size DESC",
        )
        .bind(authority)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_record).collect())
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
            "SELECT COUNT(*) total, COUNT(DISTINCT admin_addr) admins, MAX(scanned_at) last, \
                    COUNT(terminal_authority) resolved, \
                    COUNT(DISTINCT terminal_authority) authorities FROM proxy",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Coverage {
            total_scanned: row.get::<i64, _>("total"),
            covered_proxies: covered,
            by_kind,
            distinct_admins: row.get::<i64, _>("admins"),
            resolved_proxies: row.get::<i64, _>("resolved"),
            distinct_authorities: row.get::<i64, _>("authorities"),
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
        terminal_authority: r.get("terminal_authority"),
        authority_kind: r.get("authority_kind"),
        compromise_depth: r.get("compromise_depth"),
        timelock_seconds: r.get("timelock_seconds"),
        resolution_confidence: r.get("resolution_confidence"),
    }
}

/// Add any column the running schema is missing, leaving existing rows intact.
async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let present: Vec<String> = sqlx::query("PRAGMA table_info(proxy)")
        .fetch_all(pool)
        .await?
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for (name, statement) in ADDED_COLUMNS {
        if !present.iter().any(|c| c == name) {
            sqlx::raw_sql(*statement).execute(pool).await?;
        }
    }
    Ok(())
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
            ..Default::default()
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

    fn resolved(addr: &str, authority: &str, depth: Option<i64>) -> ProxyRecord {
        ProxyRecord {
            terminal_authority: Some(authority.into()),
            authority_kind: Some("safe".into()),
            compromise_depth: depth,
            timelock_seconds: Some(0),
            resolution_confidence: Some("high".into()),
            ..rec(addr, "transparent", Some("0xAdmin"))
        }
    }

    /// The whole reason resolution exists: two proxies under *different* immediate admins
    /// that share one root must be a single row. Grouping on the admin would show two
    /// authorities at half the exposure each.
    #[tokio::test]
    async fn distinct_admins_sharing_a_root_collapse_to_one_authority() {
        let s = mem().await;
        let mut a = resolved("0xA", "0xSafe", Some(2));
        a.admin = Some("0xProxyAdmin1".into());
        let mut b = resolved("0xB", "0xSafe", Some(2));
        b.admin = Some("0xProxyAdmin2".into());
        s.upsert_many(&[a, b]).await.unwrap();

        assert_eq!(
            s.admin_rollup().await.unwrap().len(),
            2,
            "the immediate admins really are distinct"
        );
        let authorities = s.authority_rollup().await.unwrap();
        assert_eq!(authorities.len(), 1, "but they answer to one root");
        assert_eq!(authorities[0].proxy_count, 2);
        assert_eq!(authorities[0].compromise_depth, Some(2));
    }

    /// An unresolved proxy must not vanish. It is excluded from the authority ranking, where
    /// it would be a fabricated row, and counted in coverage, where it is the honest gap.
    #[tokio::test]
    async fn an_unresolved_proxy_is_excluded_from_the_ranking_but_stays_counted() {
        let s = mem().await;
        s.upsert_many(&[
            resolved("0xA", "0xSafe", Some(2)),
            rec("0xB", "transparent", Some("0xMystery")),
        ])
        .await
        .unwrap();

        assert_eq!(s.authority_rollup().await.unwrap().len(), 1);
        let c = s.coverage().await.unwrap();
        assert_eq!(c.covered_proxies, 2, "both are still proxies");
        assert_eq!(c.resolved_proxies, 1, "only one has a known root");
        assert_eq!(c.distinct_authorities, 1);
    }

    /// Null is not zero. A depth of 0 would read as "free to compromise"; unknown must stay
    /// unknown all the way through the store.
    #[tokio::test]
    async fn an_unknown_compromise_depth_stays_null_rather_than_becoming_zero() {
        let s = mem().await;
        s.upsert_many(&[resolved("0xA", "0xCycle", None)])
            .await
            .unwrap();
        let rows = s.authority_rollup().await.unwrap();
        assert_eq!(rows[0].compromise_depth, None);
        let p = s.get_proxy("0xA").await.unwrap().unwrap();
        assert_eq!(p.compromise_depth, None);
    }

    /// Resolution runs after classification, so a re-scan that only reclassifies carries no
    /// authority fields. Those must not blank out an answer already established.
    #[tokio::test]
    async fn a_rescan_without_resolution_keeps_the_last_known_authority() {
        let s = mem().await;
        s.upsert_many(&[resolved("0xA", "0xSafe", Some(3))])
            .await
            .unwrap();
        s.upsert_many(&[rec("0xA", "transparent", Some("0xAdmin"))])
            .await
            .unwrap();
        let p = s.get_proxy("0xA").await.unwrap().unwrap();
        assert_eq!(p.terminal_authority.as_deref(), Some("0xSafe"));
        assert_eq!(p.compromise_depth, Some(3));
    }

    #[tokio::test]
    async fn proxies_for_authority_returns_only_that_authoritys_proxies() {
        let s = mem().await;
        s.upsert_many(&[
            resolved("0xA", "0xSafe", Some(2)),
            resolved("0xB", "0xSafe", Some(2)),
            resolved("0xC", "0xOther", Some(1)),
        ])
        .await
        .unwrap();
        assert_eq!(s.proxies_for_authority("0xSafe").await.unwrap().len(), 2);
        assert_eq!(s.proxies_for_authority("0xOther").await.unwrap().len(), 1);
        assert!(
            s.proxies_for_authority("0xNobody")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// There is a populated database on a mounted volume, so opening a store built by the
    /// previous schema has to add the columns without touching the rows.
    #[tokio::test]
    async fn opening_a_pre_resolution_database_migrates_it_without_losing_rows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE proxy (
                address TEXT PRIMARY KEY NOT NULL, label TEXT, kind TEXT NOT NULL,
                impl_addr TEXT, admin_addr TEXT, beacon_addr TEXT,
                code_size INTEGER NOT NULL DEFAULT 0, scanned_at INTEGER NOT NULL);
             INSERT INTO proxy (address,kind,code_size,scanned_at)
             VALUES ('0xOld','transparent',100,1700000000);",
        )
        .execute(&pool)
        .await
        .unwrap();

        migrate(&pool).await.unwrap();
        let store = Store { pool };
        let all = store.list_proxies(false).await.unwrap();
        assert_eq!(all.len(), 1, "the existing row must survive the migration");
        assert_eq!(all[0].address, "0xOld");
        assert_eq!(all[0].terminal_authority, None);
        assert!(
            migrate(store.pool()).await.is_ok(),
            "migration must be safe to run again on every boot"
        );
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
