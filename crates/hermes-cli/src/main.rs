//! `hermes` — one binary, two subcommands.
//!
//! One binary rather than two: I want a single deployable artifact, and a scheduled job can
//! run `hermes scan` as easily as it could run a separate entrypoint. Two binaries would buy
//! nothing and cost a second build target.

use alloy::primitives::Address;
use anyhow::Context;
use clap::{Parser, Subcommand};
use hermes_core::{AuthorityKind, Confidence, ProxyRecord, Store, resolve, store::checksum};
use hermes_scan::{AuthorityScanner, SEED, Scanner};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "hermes", version, about = "Base authority scanner")]
struct Cli {
    /// SQLite database URL.
    #[arg(
        long,
        env = "HERMES_DB",
        default_value = "sqlite://hermes.db",
        global = true
    )]
    database_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Probe the seed list and write classifications to the database.
    Scan {
        #[arg(
            long,
            env = "HERMES_RPC_URL",
            default_value = "https://mainnet.base.org"
        )]
        rpc_url: String,
        /// Concurrent in-flight readers. 3 is what the public Base endpoint tolerates.
        #[arg(long, env = "HERMES_CONCURRENCY", default_value_t = 3)]
        concurrency: usize,
        /// Scan only the first N seed entries.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Serve the JSON API and the static page.
    Serve {
        #[arg(long, env = "PORT", default_value_t = 8080)]
        port: u16,
        #[arg(long, env = "HERMES_STATIC_DIR", default_value = "static")]
        static_dir: PathBuf,
    },
}

fn authority_kind_str(kind: AuthorityKind) -> &'static str {
    match kind {
        AuthorityKind::Eoa => "eoa",
        AuthorityKind::Safe => "safe",
        AuthorityKind::Ownable => "ownable",
        AuthorityKind::Timelock => "timelock",
        AuthorityKind::Unknown => "unknown",
    }
}

fn confidence_str(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Unknown => "unknown",
    }
}

/// Walk every distinct admin to its root and write the answer back onto each proxy.
///
/// Done as a second pass rather than inline so the probe collection can be batched across
/// every admin at once. Admins are shared heavily — one ProxyAdmin governs twenty contracts
/// on Base — so resolving per proxy would re-walk the same subgraph twenty times.
async fn resolve_authorities(scanner: &AuthorityScanner, records: &mut [ProxyRecord]) -> usize {
    let admins: Vec<_> = records
        .iter()
        .filter_map(|r| r.admin.as_ref()?.parse().ok())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if admins.is_empty() {
        return 0;
    }

    tracing::info!(count = admins.len(), "resolving authorities");
    let probes = scanner.collect(admins).await;
    let mut resolved = 0;

    for record in records.iter_mut() {
        let Some(admin) = record.admin.as_ref().and_then(|a| a.parse().ok()) else {
            continue;
        };
        let r = resolve(admin, &probes);
        // An unresolved chain leaves the columns untouched, so the store keeps whatever it
        // already knew instead of being told the authority disappeared.
        if r.confidence == Confidence::Unknown {
            continue;
        }
        record.terminal_authority = Some(checksum(r.terminal_authority));
        record.authority_kind = Some(authority_kind_str(r.kind).into());
        record.compromise_depth = r.compromise_depth.map(i64::from);
        record.timelock_seconds = Some(r.timelock_seconds as i64);
        record.resolution_confidence = Some(confidence_str(r.confidence).into());
        resolved += 1;
    }
    resolved
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hermes=info,hermes_scan=info,hermes_api=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let store = Store::open(&cli.database_url)
        .await
        .with_context(|| format!("opening database at {}", cli.database_url))?;

    match cli.command {
        Command::Scan {
            rpc_url,
            concurrency,
            limit,
        } => {
            let entries = match limit {
                Some(n) => &SEED[..n.min(SEED.len())],
                None => SEED,
            };
            let addrs: Vec<Address> = entries
                .iter()
                .map(|e| e.address.parse::<Address>())
                .collect::<Result<_, _>>()
                .context("seed list contains a malformed address")?;

            tracing::info!(count = addrs.len(), rpc = %rpc_url, concurrency, "starting scan");
            let scanner = Scanner::connect(&rpc_url, concurrency).await?;
            let results = scanner.scan(addrs).await;

            let scanned_at = now();
            let mut records = Vec::new();
            let (mut ok, mut failed, mut rereads, mut unconfirmed) = (0usize, 0usize, 0, 0usize);

            for (addr, outcome) in &results {
                match outcome {
                    Ok(o) => {
                        if o.needed_reread() {
                            rereads += 1;
                        }
                        // An unconfirmed empty read would classify as `NotUpgradeable` or
                        // `Eoa`, overwrite a stored proxy, and report a live upgrade
                        // authority as safe. Leaving the previous row untouched and stale is
                        // the better of the two wrong answers available here.
                        let Some(c) = o.verdict() else {
                            unconfirmed += 1;
                            tracing::warn!(
                                %addr,
                                "empty read never confirmed; keeping any previous verdict"
                            );
                            continue;
                        };
                        ok += 1;
                        let label = SEED
                            .iter()
                            .find(|e| e.address.eq_ignore_ascii_case(&checksum(*addr)))
                            .and_then(|e| e.label)
                            .map(str::to_string);
                        records.push(ProxyRecord {
                            address: checksum(*addr),
                            label,
                            kind: c.kind.as_str().to_string(),
                            implementation: c.implementation.map(checksum),
                            admin: c.admin.map(checksum),
                            beacon: c.beacon.map(checksum),
                            code_size: o.code_size as i64,
                            scanned_at,
                            // Filled in by the resolution pass below.
                            ..Default::default()
                        });
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(%addr, error = %e, "probe failed; address skipped");
                    }
                }
            }

            // Resolution runs at concurrency 1 regardless of what the slot scan uses.
            // `eth_call` is far more expensive to the public endpoint than
            // `eth_getStorageAt`, and once its rate limiter trips it stays tripped for
            // seconds. The admin set is small enough that serialising it costs little.
            let authority_scanner = AuthorityScanner::new(scanner.provider(), 1);
            let resolved = resolve_authorities(&authority_scanner, &mut records).await;

            let written = store.upsert_many(&records).await?;
            let cov = store.coverage().await?;
            tracing::info!(
                ok,
                failed,
                rereads,
                unconfirmed,
                resolved,
                written,
                "scan complete"
            );
            println!(
                "scanned {ok} ok / {failed} failed / {unconfirmed} unconfirmed \
                 ({rereads} needed a confirming re-read)\n\
                 stored {written} rows\n\
                 covered proxies: {}/{}\n\
                 resolved to an authority: {resolved} across {} distinct roots",
                cov.covered_proxies, cov.total_scanned, cov.distinct_authorities
            );
            for (kind, n) in &cov.by_kind {
                println!("  {kind:<16} {n}");
            }
            // Fail loudly if a scan produced nothing — a silently empty scan that still
            // serves a page is the failure mode that makes a public dashboard lie.
            if cov.covered_proxies == 0 {
                anyhow::bail!("scan stored no covered proxies; refusing to report success");
            }
        }

        Command::Serve { port, static_dir } => {
            hermes_api::serve(store, static_dir, port).await?;
        }
    }
    Ok(())
}
