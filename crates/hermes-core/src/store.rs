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
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use std::str::FromStr;
use std::time::Duration;

/// How long a statement waits on another process's lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times to ask for WAL before giving up. With the delays in `enable_wal` this is
/// about six seconds, which is several lifetimes of any migration transaction.
const WAL_ATTEMPTS: u32 = 12;

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
    /// `base` or `ethereum`. The root of a Base proxy can be a Safe on Ethereum, acting
    /// through its L1→L2 alias, and an address alone would make it look like a Base contract.
    pub terminal_chain: Option<String>,
    pub authority_kind: Option<String>,
    /// Null when the chain could not be resolved. Never zero — that would read as free.
    pub compromise_depth: Option<i64>,
    pub timelock_seconds: Option<i64>,
    pub resolution_confidence: Option<String>,
    /// Where the walk started: `admin_slot`, `beacon` or `uups_owner`.
    pub upgrade_path: Option<String>,
    /// Why a covered proxy has no root: `no_upgrade_path`, `uups_unconfirmed`,
    /// `unrecognized_interface` or `rpc_undetermined`. Null whenever there is a root.
    pub unresolved_reason: Option<String>,
    /// Why a root has no key count: `truncated`, `cycle` or `owners_unknown`.
    pub depth_unknown_reason: Option<String>,
}

impl ProxyRecord {
    /// Whether this row's resolution is an answer about the chain: a root, or a real reason
    /// there is none. A row the resolver never reached, or one it could not read, is not.
    pub fn resolution_is_settled(&self) -> bool {
        self.terminal_authority.is_some()
            || self
                .unresolved_reason
                .as_deref()
                .is_some_and(|reason| reason != "rpc_undetermined")
    }
}

/// One resolved authority and what it controls.
#[derive(Debug, Clone, Serialize)]
pub struct AuthorityRow {
    pub address: String,
    pub chain: Option<String>,
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
    /// Covered proxies with no root, by why. The gap should explain itself, not just be a
    /// number.
    pub unresolved_by_reason: Vec<(String, i64)>,
    /// Resolved proxies whose root has no key count, by why.
    pub depth_unknown_by_reason: Vec<(String, i64)>,
    /// Resolved proxies by where their walk started.
    pub resolved_by_path: Vec<(String, i64)>,
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
    terminal_chain         TEXT,
    authority_kind         TEXT,
    compromise_depth       INTEGER,
    timelock_seconds       INTEGER,
    resolution_confidence  TEXT,
    upgrade_path           TEXT,
    unresolved_reason      TEXT,
    depth_unknown_reason   TEXT
);
-- Exposure gets grouped by authority, and the admin column is what it groups on, so it is
-- indexed from the start.
CREATE INDEX IF NOT EXISTS idx_proxy_admin ON proxy(admin_addr);
CREATE INDEX IF NOT EXISTS idx_proxy_kind  ON proxy(kind);
"#;

/// Indexes over columns that `migrate` may still be adding.
///
/// Deliberately not part of `SCHEMA`. On a database that already exists the
/// `CREATE TABLE IF NOT EXISTS` above does nothing, so indexing a newly added column before
/// its `ALTER TABLE` has run fails with `no such column` and takes the whole boot down with
/// it. Ordering here is load-bearing, not cosmetic.
const LATE_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_proxy_terminal ON proxy(terminal_authority);
"#;

/// A column added after the first deployment, and what has to happen to existing rows when
/// it arrives.
struct AddedColumn {
    alter: &'static str,
    /// Runs only in the process whose `ALTER` actually added the column, so it happens once
    /// per database rather than on every boot.
    on_add: Option<&'static str>,
}

const fn column(alter: &'static str) -> AddedColumn {
    AddedColumn {
        alter,
        on_add: None,
    }
}

/// Columns added after the first deployment, in the order they were added.
///
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, and there is a populated database on a mounted
/// volume, so existing rows have to survive this rather than be recreated. Statements are
/// static so no SQL is ever assembled at runtime.
const ADDED_COLUMNS: &[AddedColumn] = &[
    column("ALTER TABLE proxy ADD COLUMN terminal_authority TEXT"),
    column("ALTER TABLE proxy ADD COLUMN authority_kind TEXT"),
    column("ALTER TABLE proxy ADD COLUMN compromise_depth INTEGER"),
    column("ALTER TABLE proxy ADD COLUMN timelock_seconds INTEGER"),
    column("ALTER TABLE proxy ADD COLUMN resolution_confidence TEXT"),
    AddedColumn {
        alter: "ALTER TABLE proxy ADD COLUMN terminal_chain TEXT",
        // Every resolution stored before this column existed was made by a resolver that read
        // "no code on Base" as "one key", without asking L1 whether a contract stood behind the
        // alias. That is how a 2-of-2 Safe eleven keys deep was published as a single EOA.
        // None of those answers is one I stand behind any more, so they are retracted rather
        // than left to be served until the next scan happens to overwrite them.
        on_add: Some(
            "UPDATE proxy SET terminal_authority = NULL, authority_kind = NULL, \
             compromise_depth = NULL, timelock_seconds = NULL, resolution_confidence = NULL",
        ),
    },
    column("ALTER TABLE proxy ADD COLUMN upgrade_path TEXT"),
    column("ALTER TABLE proxy ADD COLUMN unresolved_reason TEXT"),
    column("ALTER TABLE proxy ADD COLUMN depth_unknown_reason TEXT"),
];

/// Every column `row_to_record` reads, named rather than `*`.
///
/// `SELECT *` broke under a schema change. A pooled connection holding a cached schema from
/// before an `ALTER TABLE` prepares `*` as the old column list, SQLite silently re-prepares it
/// with the new one when it runs, and sqlx indexes the extra column against metadata it cached
/// at prepare time: "index out of bounds: the len is 13 but the index is 13", as a panic in the
/// driver's worker thread. Naming the columns makes a stale schema a prepare-time miss, which
/// SQLite answers by re-reading the schema instead of by widening the row underneath me.
macro_rules! proxy_columns {
    () => {
        "address, label, kind, impl_addr, admin_addr, beacon_addr, code_size, scanned_at, \
         terminal_authority, terminal_chain, authority_kind, compromise_depth, \
         timelock_seconds, resolution_confidence, upgrade_path, unresolved_reason, \
         depth_unknown_reason"
    };
}

/// The kinds v1 counts as covered, as SQL. Must match `ProxyKind::is_covered_proxy`; a test
/// holds the two together.
macro_rules! covered_kinds {
    () => {
        "('transparent','uups','beacon','eip1822','admin_only')"
    };
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// `url` is a SQLite URL, e.g. `sqlite://hermes.db`. The file is created if absent.
    ///
    /// The scan and the server open this file as separate processes at the same time, so
    /// everything that changes the file's shape happens once, on one connection, before the
    /// pool fans out: the switch to WAL, then the schema, the migration and the late indexes
    /// as a single `BEGIN IMMEDIATE` transaction. `BEGIN IMMEDIATE` waits on the busy timeout,
    /// so a second process opening at the same moment queues behind the first instead of
    /// interleaving `ALTER`s with it.
    pub async fn open(url: &str) -> anyhow::Result<Self> {
        // No `journal_mode` option here. sqlx would issue it on every pooled connection, and
        // that is where concurrent opens died with `database is locked` (29 of 29 failures,
        // all at connect): see `enable_wal`.
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        let mut conn = pool.acquire().await?;
        enable_wal(&mut conn).await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
        migrate(&mut tx).await?;
        sqlx::raw_sql(LATE_INDEXES).execute(&mut *tx).await?;
        tx.commit().await?;
        drop(conn);

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
                                      terminal_authority,terminal_chain,authority_kind,compromise_depth,
                                      timelock_seconds,resolution_confidence,upgrade_path,
                                      unresolved_reason,depth_unknown_reason)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                   ON CONFLICT(address) DO UPDATE SET
                     label=COALESCE(excluded.label, proxy.label),
                     kind=excluded.kind,
                     impl_addr=excluded.impl_addr,
                     admin_addr=excluded.admin_addr,
                     beacon_addr=excluded.beacon_addr,
                     code_size=excluded.code_size,
                     scanned_at=excluded.scanned_at,
                     -- ?18 is whether the incoming resolution is settled (see
                     -- `resolution_is_settled`). Settled answers replace every resolution column as
                     -- one unit: per-column COALESCE let a new root inherit the old root's key
                     -- count, and a root whose admin moved to an unrecognized contract has to
                     -- stop being published. An incoming row that is not settled (the node would
                     -- not answer, or resolution never ran) keeps the last known answer, because
                     -- an outage has told me nothing about the chain.
                     terminal_authority=CASE WHEN ?18 THEN excluded.terminal_authority ELSE proxy.terminal_authority END,
                     terminal_chain=CASE WHEN ?18 THEN excluded.terminal_chain ELSE proxy.terminal_chain END,
                     authority_kind=CASE WHEN ?18 THEN excluded.authority_kind ELSE proxy.authority_kind END,
                     compromise_depth=CASE WHEN ?18 THEN excluded.compromise_depth ELSE proxy.compromise_depth END,
                     timelock_seconds=CASE WHEN ?18 THEN excluded.timelock_seconds ELSE proxy.timelock_seconds END,
                     resolution_confidence=CASE WHEN ?18 THEN excluded.resolution_confidence ELSE proxy.resolution_confidence END,
                     upgrade_path=CASE WHEN ?18 THEN excluded.upgrade_path ELSE proxy.upgrade_path END,
                     depth_unknown_reason=CASE WHEN ?18 THEN excluded.depth_unknown_reason ELSE proxy.depth_unknown_reason END,
                     -- A row that keeps an old root has no reason to be unresolved; a row that
                     -- has none keeps its last settled reason over a fresh "undetermined".
                     unresolved_reason=CASE WHEN ?18 THEN excluded.unresolved_reason
                       WHEN proxy.terminal_authority IS NOT NULL THEN NULL
                       ELSE COALESCE(proxy.unresolved_reason, excluded.unresolved_reason) END
                   WHERE NOT (proxy.code_size > 0 AND excluded.code_size = 0)"#,
            )
            .bind(&r.address).bind(&r.label).bind(&r.kind)
            .bind(&r.implementation).bind(&r.admin).bind(&r.beacon)
            .bind(r.code_size).bind(r.scanned_at)
            .bind(&r.terminal_authority).bind(&r.terminal_chain).bind(&r.authority_kind)
            .bind(r.compromise_depth).bind(r.timelock_seconds).bind(&r.resolution_confidence)
            .bind(&r.upgrade_path).bind(&r.unresolved_reason).bind(&r.depth_unknown_reason)
            .bind(r.resolution_is_settled())
            .execute(&mut *tx).await?;
            n += res.rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Proxies only (covered patterns), most recently scanned first.
    pub async fn list_proxies(&self, only_covered: bool) -> anyhow::Result<Vec<ProxyRecord>> {
        let sql = if only_covered {
            concat!(
                "SELECT ",
                proxy_columns!(),
                " FROM proxy WHERE kind IN ",
                covered_kinds!(),
                " ORDER BY code_size DESC"
            )
        } else {
            concat!(
                "SELECT ",
                proxy_columns!(),
                " FROM proxy ORDER BY code_size DESC"
            )
        };
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_record).collect())
    }

    pub async fn get_proxy(&self, address: &str) -> anyhow::Result<Option<ProxyRecord>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            proxy_columns!(),
            " FROM proxy WHERE address = ?1 COLLATE NOCASE"
        ))
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
    /// The chain is part of the key: the same address on Base and on Ethereum is two accounts.
    ///
    /// Unresolved proxies are excluded rather than bucketed under a placeholder — they are
    /// reported by `coverage` instead, so they stay visible without being counted as an
    /// authority I understand.
    pub async fn authority_rollup(&self) -> anyhow::Result<Vec<AuthorityRow>> {
        let rows = sqlx::query(
            "SELECT terminal_authority a, terminal_chain ch, COUNT(*) c, \
                    MAX(authority_kind) k, \
                    MAX(compromise_depth) d, \
                    MAX(timelock_seconds) t, \
                    MIN(resolution_confidence) conf \
             FROM proxy WHERE terminal_authority IS NOT NULL \
             GROUP BY terminal_authority, terminal_chain ORDER BY c DESC, a ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| AuthorityRow {
                address: r.get("a"),
                chain: r.get("ch"),
                proxy_count: r.get("c"),
                kind: r.get("k"),
                compromise_depth: r.get("d"),
                timelock_seconds: r.get("t"),
                confidence: r.get("conf"),
            })
            .collect())
    }

    /// Every proxy that resolves to one authority, optionally only on one chain.
    pub async fn proxies_for_authority(
        &self,
        authority: &str,
        chain: Option<&str>,
    ) -> anyhow::Result<Vec<ProxyRecord>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            proxy_columns!(),
            " FROM proxy WHERE terminal_authority = ?1 COLLATE NOCASE \
             AND (?2 IS NULL OR terminal_chain = ?2) ORDER BY code_size DESC"
        ))
        .bind(authority)
        .bind(chain)
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
        let covered = self
            .scalar(concat!(
                "SELECT COUNT(*) FROM proxy WHERE kind IN ",
                covered_kinds!()
            ))
            .await?;
        let row = sqlx::query(
            "SELECT COUNT(*) total, COUNT(DISTINCT admin_addr) admins, MAX(scanned_at) last, \
                    COUNT(terminal_authority) resolved, \
                    COUNT(DISTINCT COALESCE(terminal_chain, '') || ':' || terminal_authority) authorities \
             FROM proxy",
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
            // A covered proxy with no root and no reason was stored before reasons existed.
            unresolved_by_reason: self
                .counts(concat!(
                    "SELECT COALESCE(unresolved_reason, 'not_attempted') k, COUNT(*) c FROM proxy \
                     WHERE terminal_authority IS NULL AND kind IN ",
                    covered_kinds!(),
                    " GROUP BY k ORDER BY c DESC, k"
                ))
                .await?,
            depth_unknown_by_reason: self
                .counts(
                    "SELECT COALESCE(depth_unknown_reason, 'unrecorded') k, COUNT(*) c FROM proxy \
                     WHERE terminal_authority IS NOT NULL AND compromise_depth IS NULL \
                     GROUP BY k ORDER BY c DESC, k",
                )
                .await?,
            resolved_by_path: self
                .counts(
                    "SELECT COALESCE(upgrade_path, 'unrecorded') k, COUNT(*) c FROM proxy \
                     WHERE terminal_authority IS NOT NULL GROUP BY k ORDER BY c DESC, k",
                )
                .await?,
        })
    }

    async fn scalar(&self, sql: &'static str) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar(sql).fetch_one(&self.pool).await?)
    }

    async fn counts(&self, sql: &'static str) -> anyhow::Result<Vec<(String, i64)>> {
        Ok(sqlx::query(sql)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| (r.get::<String, _>("k"), r.get::<i64, _>("c")))
            .collect())
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
        terminal_chain: r.get("terminal_chain"),
        authority_kind: r.get("authority_kind"),
        compromise_depth: r.get("compromise_depth"),
        timelock_seconds: r.get("timelock_seconds"),
        resolution_confidence: r.get("resolution_confidence"),
        upgrade_path: r.get("upgrade_path"),
        unresolved_reason: r.get("unresolved_reason"),
        depth_unknown_reason: r.get("depth_unknown_reason"),
    }
}

/// Add any column the running schema is missing, leaving existing rows intact.
///
/// Deliberately attempts every `ALTER TABLE` rather than checking `PRAGMA table_info` first.
/// The scan and the server open this database as separate processes at the same time, so a
/// check-then-add lets both read "column absent", both issue the `ALTER`, and the loser die
/// on `duplicate column name`. Treating that specific error as success makes the migration
/// idempotent by construction instead of by timing.
///
/// It now also runs inside `open`'s `BEGIN IMMEDIATE`, which serializes migrators. The
/// attempt-every-`ALTER` rule stays anyway: it is what keeps an older binary, or a database
/// someone migrated by hand, from turning into a crash loop.
async fn migrate(conn: &mut SqliteConnection) -> anyhow::Result<()> {
    for added in ADDED_COLUMNS {
        match sqlx::raw_sql(added.alter).execute(&mut *conn).await {
            Ok(_) => {
                if let Some(statement) = added.on_add {
                    sqlx::raw_sql(statement).execute(&mut *conn).await?;
                }
            }
            Err(e) if is_duplicate_column(&e) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Put the file in WAL mode, once, retrying while another process holds it.
///
/// WAL is what lets the server keep answering reads while a scan commits, instead of both
/// sides taking turns behind a lock. It is a property of the file, so it only has to be set
/// once, and switching into it needs an exclusive lock that SQLite will not wait for through
/// the busy timeout: a second opener gets `SQLITE_BUSY` straight back (sqlx's own source notes
/// the same). Measured before this existed, eight concurrent opens failed 18 times in 60 runs,
/// every one of them there. A bounded retry is the only way to wait for this lock.
async fn enable_wal(conn: &mut SqliteConnection) -> anyhow::Result<()> {
    let mut delay = Duration::from_millis(10);
    for _ in 0..WAL_ATTEMPTS {
        match sqlx::query_scalar::<_, String>("PRAGMA journal_mode = WAL")
            .fetch_one(&mut *conn)
            .await
        {
            // An in-memory database answers `memory`; it has no file to put in WAL mode.
            Ok(mode) if mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory") => {
                return Ok(());
            }
            Ok(_) => {}
            Err(e) if is_busy(&e) => {}
            Err(e) => return Err(e.into()),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(1));
    }
    anyhow::bail!("could not switch the database to WAL: another process kept it locked")
}

/// `SQLITE_BUSY` and its extended forms (`_RECOVERY`, `_SNAPSHOT`, `_TIMEOUT`) share the low
/// byte 5.
fn is_busy(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|e| e.code())
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| code & 0xff == 5)
}

/// Only ever swallow the one error that means "another process already did this".
fn is_duplicate_column(error: &sqlx::Error) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("duplicate column name")
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
            terminal_chain: Some("base".into()),
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

    /// The failure this guards: the predeploy root re-resolving from "one EOA, 1 key" to "a Safe
    /// on Ethereum" while the old key count survives underneath the new answer. A new root has
    /// to bring its own depth, even when that depth is unknown.
    #[tokio::test]
    async fn a_new_resolution_replaces_every_resolution_column_together() {
        let s = mem().await;
        let mut before = resolved("0xA", "0xAlias", Some(1));
        before.authority_kind = Some("eoa".into());
        s.upsert_many(&[before]).await.unwrap();

        let mut after = resolved("0xA", "0xL1Safe", None);
        after.terminal_chain = Some("ethereum".into());
        after.resolution_confidence = Some("medium".into());
        s.upsert_many(&[after]).await.unwrap();

        let p = s.get_proxy("0xA").await.unwrap().unwrap();
        assert_eq!(p.terminal_authority.as_deref(), Some("0xL1Safe"));
        assert_eq!(p.terminal_chain.as_deref(), Some("ethereum"));
        assert_eq!(p.authority_kind.as_deref(), Some("safe"));
        assert_eq!(
            p.compromise_depth, None,
            "the old root's key count must not outlive it"
        );
        assert_eq!(p.resolution_confidence.as_deref(), Some("medium"));
    }

    /// The same 20 bytes on Base and on Ethereum are two accounts, so they are two rows.
    #[tokio::test]
    async fn the_same_address_on_two_chains_is_two_authorities() {
        let s = mem().await;
        let on_base = resolved("0xA", "0xSame", Some(1));
        let mut on_ethereum = resolved("0xB", "0xSame", Some(2));
        on_ethereum.terminal_chain = Some("ethereum".into());
        s.upsert_many(&[on_base, on_ethereum]).await.unwrap();

        assert_eq!(s.authority_rollup().await.unwrap().len(), 2);
        assert_eq!(s.coverage().await.unwrap().distinct_authorities, 2);
        assert_eq!(
            s.proxies_for_authority("0xSame", Some("ethereum"))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.proxies_for_authority("0xSame", None).await.unwrap().len(),
            2
        );
    }

    /// Opening the database the live deployment has: resolution columns present, no
    /// `terminal_chain`, and an "EOA" that was never checked against L1. That answer must not
    /// survive the upgrade, and the retraction must happen once, not on every boot.
    #[tokio::test]
    async fn resolutions_stored_before_aliasing_was_modelled_are_retracted_once() {
        let path = std::env::temp_dir().join(format!("hermes-prealias-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}", path.display());

        let old = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE proxy (
                address TEXT PRIMARY KEY NOT NULL, label TEXT, kind TEXT NOT NULL,
                impl_addr TEXT, admin_addr TEXT, beacon_addr TEXT,
                code_size INTEGER NOT NULL DEFAULT 0, scanned_at INTEGER NOT NULL,
                terminal_authority TEXT, authority_kind TEXT, compromise_depth INTEGER,
                timelock_seconds INTEGER, resolution_confidence TEXT);
             INSERT INTO proxy VALUES ('0xPredeploy','L1Block','transparent',NULL,
                '0x4200000000000000000000000000000000000018',NULL,100,1700000000,
                '0x8cC51c3008b3f03Fe483B28B8Db90e19cF076a6d','eoa',1,0,'high');",
        )
        .execute(&old)
        .await
        .unwrap();
        old.close().await;

        let store = Store::open(&url).await.unwrap();
        let p = store.get_proxy("0xPredeploy").await.unwrap().unwrap();
        assert_eq!(
            p.terminal_authority, None,
            "an unchecked EOA verdict is retracted"
        );
        assert_eq!(p.compromise_depth, None);
        assert_eq!(
            p.label.as_deref(),
            Some("L1Block"),
            "the proxy itself survives"
        );
        assert_eq!(store.authority_rollup().await.unwrap().len(), 0);

        store
            .upsert_many(&[ProxyRecord {
                terminal_authority: Some("0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c".into()),
                terminal_chain: Some("ethereum".into()),
                authority_kind: Some("safe".into()),
                compromise_depth: Some(11),
                timelock_seconds: Some(0),
                resolution_confidence: Some("high".into()),
                ..p
            }])
            .await
            .unwrap();
        drop(store);

        let reopened = Store::open(&url).await.unwrap();
        let p = reopened.get_proxy("0xPredeploy").await.unwrap().unwrap();
        assert_eq!(
            p.compromise_depth,
            Some(11),
            "a resolution made after the upgrade must survive the next boot"
        );
        let _ = std::fs::remove_file(&path);
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
        assert_eq!(
            s.proxies_for_authority("0xSafe", None).await.unwrap().len(),
            2
        );
        assert_eq!(
            s.proxies_for_authority("0xOther", None)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            s.proxies_for_authority("0xNobody", None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// The real upgrade path, exercised the way the deployment does it: a database file
    /// written by the previous schema, then opened with `Store::open`.
    ///
    /// The earlier version of this test called `migrate` directly and passed while the
    /// deployment crash-looped. `Store::open` runs the schema *before* the migration, so an
    /// index over a not-yet-added column fails with `no such column` and never reaches the
    /// `ALTER TABLE` that would have fixed it. Going through the real entry point is the
    /// whole point of the test.
    #[tokio::test]
    async fn opening_a_pre_resolution_database_migrates_it_without_losing_rows() {
        let path = std::env::temp_dir().join(format!("hermes-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}", path.display());

        let old = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE proxy (
                address TEXT PRIMARY KEY NOT NULL, label TEXT, kind TEXT NOT NULL,
                impl_addr TEXT, admin_addr TEXT, beacon_addr TEXT,
                code_size INTEGER NOT NULL DEFAULT 0, scanned_at INTEGER NOT NULL);
             INSERT INTO proxy (address,label,kind,code_size,scanned_at)
             VALUES ('0xOld','Aerodrome','transparent',100,1700000000);",
        )
        .execute(&old)
        .await
        .unwrap();
        old.close().await;

        let store = Store::open(&url)
            .await
            .expect("opening an old database must migrate it");
        let all = store.list_proxies(false).await.unwrap();
        assert_eq!(all.len(), 1, "the existing row must survive");
        assert_eq!(all[0].label.as_deref(), Some("Aerodrome"));
        assert_eq!(all[0].terminal_authority, None);

        // Resolution has to work against the migrated table, not just be present in it.
        let mut resolved = all[0].clone();
        resolved.terminal_authority = Some("0xSafe".into());
        resolved.compromise_depth = Some(2);
        store.upsert_many(&[resolved]).await.unwrap();
        assert_eq!(store.authority_rollup().await.unwrap().len(), 1);

        drop(store);
        Store::open(&url)
            .await
            .expect("every subsequent boot must open it too");
        let _ = std::fs::remove_file(&path);
    }

    /// The scan and the server open this database as separate processes at the same time, so
    /// the migration has to survive being run concurrently against the same file.
    ///
    /// The check-then-add version passed every single-threaded test and then took the scan
    /// down on the first boot after the columns landed: both openers read "column absent",
    /// both issued the ALTER, and the loser died on `duplicate column name`.
    #[tokio::test]
    async fn concurrent_opens_do_not_collide_on_the_migration() {
        let path = std::env::temp_dir().join(format!("hermes-race-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}", path.display());

        let seed = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .create_if_missing(true),
            )
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
        .execute(&seed)
        .await
        .unwrap();
        seed.close().await;

        // Spawned, not awaited in sequence: awaiting them one at a time would never
        // overlap and would pass against the broken version too.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let url = url.clone();
            handles.push(tokio::spawn(async move { Store::open(&url).await.is_ok() }));
        }
        for (i, h) in handles.into_iter().enumerate() {
            assert!(h.await.unwrap(), "concurrent open {i} failed to migrate");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The covered list lives in two languages. If they drift, `/coverage` and `/proxies`
    /// disagree with the classifier about what a proxy is.
    #[test]
    fn the_sql_covered_list_matches_the_classifier() {
        use crate::classify::ProxyKind::*;
        let all = [
            Transparent,
            Uups,
            Beacon,
            Eip1822,
            AdminOnly,
            ZeppelinOs,
            NotUpgradeable,
            Eoa,
        ];
        let sql = covered_kinds!();
        for kind in all {
            assert_eq!(
                sql.contains(&format!("'{}'", kind.as_str())),
                kind.is_covered_proxy(),
                "{}",
                kind.as_str()
            );
        }
    }

    fn unresolved(addr: &str, reason: &str) -> ProxyRecord {
        ProxyRecord {
            unresolved_reason: Some(reason.into()),
            ..rec(addr, "transparent", Some("0xAdmin"))
        }
    }

    /// An outage has told me nothing about the chain, so a root survives it.
    #[tokio::test]
    async fn an_undetermined_resolution_keeps_the_last_known_root() {
        let s = mem().await;
        s.upsert_many(&[resolved("0xA", "0xSafe", Some(3))])
            .await
            .unwrap();
        s.upsert_many(&[unresolved("0xA", "rpc_undetermined")])
            .await
            .unwrap();
        let p = s.get_proxy("0xA").await.unwrap().unwrap();
        assert_eq!(p.terminal_authority.as_deref(), Some("0xSafe"));
        assert_eq!(p.compromise_depth, Some(3));
        assert_eq!(
            p.unresolved_reason, None,
            "a row with a root has nothing to explain"
        );
    }

    /// A real finding does replace a root. The admin moved to something I do not recognize, and
    /// going on publishing the old Safe would be a confident answer about the wrong contract.
    #[tokio::test]
    async fn a_settled_unresolved_answer_replaces_a_stale_root() {
        let s = mem().await;
        s.upsert_many(&[resolved("0xA", "0xSafe", Some(3))])
            .await
            .unwrap();
        s.upsert_many(&[unresolved("0xA", "unrecognized_interface")])
            .await
            .unwrap();
        let p = s.get_proxy("0xA").await.unwrap().unwrap();
        assert_eq!(p.terminal_authority, None);
        assert_eq!(p.compromise_depth, None);
        assert_eq!(
            p.unresolved_reason.as_deref(),
            Some("unrecognized_interface")
        );
    }

    /// With no root either way, an outage does not overwrite the last real reason.
    #[tokio::test]
    async fn an_outage_does_not_overwrite_a_settled_reason() {
        let s = mem().await;
        s.upsert_many(&[unresolved("0xA", "unrecognized_interface")])
            .await
            .unwrap();
        s.upsert_many(&[
            unresolved("0xA", "rpc_undetermined"),
            unresolved("0xB", "rpc_undetermined"),
        ])
        .await
        .unwrap();
        let a = s.get_proxy("0xA").await.unwrap().unwrap();
        let b = s.get_proxy("0xB").await.unwrap().unwrap();
        assert_eq!(
            a.unresolved_reason.as_deref(),
            Some("unrecognized_interface")
        );
        assert_eq!(b.unresolved_reason.as_deref(), Some("rpc_undetermined"));
    }

    #[tokio::test]
    async fn coverage_explains_every_gap() {
        let s = mem().await;
        let mut beacon = resolved("0xA", "0xSafe", Some(2));
        beacon.upgrade_path = Some("beacon".into());
        let mut cyclic = resolved("0xB", "0xSelf", None);
        cyclic.upgrade_path = Some("admin_slot".into());
        cyclic.depth_unknown_reason = Some("cycle".into());
        s.upsert_many(&[
            beacon,
            cyclic,
            unresolved("0xC", "unrecognized_interface"),
            unresolved("0xD", "unrecognized_interface"),
            unresolved("0xE", "uups_unconfirmed"),
            rec("0xF", "transparent", Some("0xNeverResolved")),
            rec("0x10", "zeppelin_os", None),
        ])
        .await
        .unwrap();
        let c = s.coverage().await.unwrap();
        assert_eq!(
            c.unresolved_by_reason,
            vec![
                ("unrecognized_interface".to_string(), 2),
                ("not_attempted".to_string(), 1),
                ("uups_unconfirmed".to_string(), 1),
            ],
            "every covered proxy without a root is accounted for, and nothing else is"
        );
        assert_eq!(c.depth_unknown_by_reason, vec![("cycle".to_string(), 1)]);
        assert_eq!(
            c.resolved_by_path,
            vec![("admin_slot".to_string(), 1), ("beacon".to_string(), 1)]
        );
        let explained: i64 = c.unresolved_by_reason.iter().map(|(_, n)| n).sum();
        assert_eq!(explained, c.covered_proxies - c.resolved_proxies);
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
