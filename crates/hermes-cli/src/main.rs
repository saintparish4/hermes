//! `hermes` — one binary.
//!
//! One binary rather than two: I want a single deployable artifact, and a scheduled job can
//! run `hermes scan` as easily as it could run a separate entrypoint. Two binaries would buy
//! nothing and cost a second build target. `record` and `verify` are developer tools that ride
//! along because they have to run exactly the pipeline the deployment runs.

use alloy::primitives::Address;
use alloy::providers::Provider;
use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use hermes_core::store::{Coverage, DISCOVERY_CAP};
use hermes_core::{Chain, SeedRow, Store};
use hermes_scan::discover::{self, Discoverer};
use hermes_scan::verify::{differences, load_table};
use hermes_scan::{
    AuthorityScanner, ChainRpc, Endpoint, Fixture, LiveRpc, RecordingRpc, RunReport, SEED, Scanner,
    Target, scan_and_resolve, scan_into,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The public Ethereum endpoint took a 60-call burst without a single refusal, so a light gap
/// between resolution calls to it is courtesy rather than necessity.
const L1_CALL_INTERVAL: Duration = Duration::from_millis(100);

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

/// Where the chain is read from.
#[derive(Args, Clone)]
struct Endpoints {
    #[arg(
        long,
        env = "HERMES_RPC_URL",
        default_value = "https://mainnet.base.org"
    )]
    rpc_url: String,
    /// Ethereum mainnet, read only to learn what stands behind a codeless Base authority.
    #[arg(
        long,
        env = "HERMES_L1_RPC_URL",
        default_value = "https://eth.drpc.org"
    )]
    l1_rpc_url: String,
    /// Minimum gap between resolution calls to the Base endpoint. The public endpoint needs
    /// about a second; a keyed endpoint can take 0.
    #[arg(long, env = "HERMES_CALL_INTERVAL_MS", default_value_t = 1000)]
    call_interval_ms: u64,
}

impl Endpoints {
    fn scanners(
        &self,
        base: Arc<dyn ChainRpc>,
        ethereum: Arc<dyn ChainRpc>,
        concurrency: usize,
    ) -> (Scanner, AuthorityScanner) {
        // Resolution runs at concurrency 1 regardless of what the slot scan uses. `eth_call` is
        // far more expensive to the public endpoint than `eth_getStorageAt`, and once its rate
        // limiter trips it stays tripped for seconds.
        let authorities = AuthorityScanner::new(
            Endpoint::new(
                Arc::clone(&base),
                Duration::from_millis(self.call_interval_ms),
            ),
            Endpoint::new(ethereum, L1_CALL_INTERVAL),
            1,
        );
        (Scanner::new(base, concurrency), authorities)
    }

    async fn live(&self, concurrency: usize) -> anyhow::Result<(Scanner, AuthorityScanner)> {
        let base = LiveRpc::new(hermes_scan::connect(&self.rpc_url).await?);
        let ethereum = LiveRpc::new(hermes_scan::connect(&self.l1_rpc_url).await?);
        Ok(self.scanners(Arc::new(base), Arc::new(ethereum), concurrency))
    }
}

#[derive(Subcommand)]
enum Command {
    /// Scan every seeded address that is due, resolve what it finds, and write it in batches.
    Scan {
        #[command(flatten)]
        endpoints: Endpoints,
        /// Concurrent in-flight readers. 3 is what the public Base endpoint tolerates.
        #[arg(long, env = "HERMES_CONCURRENCY", default_value_t = 3)]
        concurrency: usize,
        /// Scan at most this many due addresses in this run.
        #[arg(long)]
        limit: Option<i64>,
        /// Skip addresses scanned within this many hours. This is what makes a killed run
        /// resume where it stopped instead of starting over.
        #[arg(long, default_value_t = 20)]
        fresh_hours: i64,
        /// Addresses per batch. Each batch is written before the next begins.
        #[arg(long, default_value_t = 50)]
        batch: usize,
        /// Classify only and resolve nothing: the quick pass a fresh deployment makes before it
        /// opens its port.
        #[arg(long)]
        classify_only: bool,
    },
    /// Find proxies chain-wide from their ERC-1967 events and add them to the seed.
    Discover {
        #[command(flatten)]
        endpoints: Endpoints,
        /// Blocks between the starts of consecutive windows.
        #[arg(long, default_value_t = 500_000)]
        stride: u64,
        /// Blocks per window. The public endpoint refuses ten thousand; one thousand is safe.
        #[arg(long, default_value_t = 1_000)]
        window: u64,
        /// The most addresses admitted per implementation, beacon or admin.
        #[arg(long, default_value_t = 3)]
        per_family: i64,
        /// Stop after this many windows.
        #[arg(long)]
        max_windows: Option<usize>,
    },
    /// Run the pipeline for one address at a pinned block and save every answer the chain gave
    /// as a replayable fixture. Prints what Hermes concluded, which is never the expectation:
    /// expectations in `tests/verified.json` are checked by hand.
    Record {
        address: Address,
        /// Fixture file stem, e.g. `l2-standard-bridge`.
        #[arg(long)]
        name: String,
        /// Base block to read at. Defaults to the current head.
        #[arg(long)]
        block: Option<u64>,
        /// Ethereum block to read at. Defaults to the current head.
        #[arg(long)]
        l1_block: Option<u64>,
        #[arg(long, default_value = "tests/fixtures")]
        out_dir: PathBuf,
        #[command(flatten)]
        endpoints: Endpoints,
    },
    /// Re-check every hand-verified address against the live chain. A failure means Hermes
    /// regressed or the chain moved; re-recording the fixture and diffing says which.
    Verify {
        #[arg(long, default_value = "tests/verified.json")]
        table: PathBuf,
        #[command(flatten)]
        endpoints: Endpoints,
    },
    /// Bring the database up to the current schema, then exit.
    ///
    /// Every subcommand that opens the database migrates it, safely even when two open at once.
    /// This exists so the container can migrate once, alone, before the scan and the server
    /// start together.
    Migrate,
    /// Serve the JSON API and the static page.
    Serve {
        #[arg(long, env = "PORT", default_value_t = 8080)]
        port: u16,
        #[arg(long, env = "HERMES_STATIC_DIR", default_value = "static")]
        static_dir: PathBuf,
    },
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn open(url: &str) -> anyhow::Result<Store> {
    Store::open(url)
        .await
        .with_context(|| format!("opening database at {url}"))
}

/// The hand-curated list, as seed rows. It bootstraps every database, and it is the only
/// source of labels.
fn curated_seeds(discovered_at: i64) -> Vec<SeedRow> {
    SEED.iter()
        .map(|e| SeedRow {
            address: e.address.to_string(),
            label: e.label.map(str::to_string),
            source: "curated".into(),
            family: None,
            first_block: None,
            discovered_at,
        })
        .collect()
}

fn report(run: RunReport, cov: &Coverage) {
    let counts = run.counts;
    tracing::info!(
        ok = counts.ok,
        failed = counts.failed,
        rereads = counts.rereads,
        unconfirmed = counts.unconfirmed,
        resolved = counts.resolved,
        written = run.written,
        batches = run.batches,
        "scan complete"
    );
    println!(
        "this run: {} ok / {} failed / {} unconfirmed ({} needed a confirming re-read), \
         {} resolved, {} rows written in {} batches\n\
         store: {} covered proxies of {} scanned, {} resolved across {} distinct roots",
        counts.ok,
        counts.failed,
        counts.unconfirmed,
        counts.rereads,
        counts.resolved,
        run.written,
        run.batches,
        cov.covered_proxies,
        cov.total_scanned,
        cov.resolved_proxies,
        cov.distinct_authorities
    );
    for (kind, n) in &cov.by_kind {
        println!("  {kind:<16} {n}");
    }
}

/// How a scan run is shaped.
struct ScanPlan {
    concurrency: usize,
    limit: Option<i64>,
    fresh_hours: i64,
    batch: usize,
    classify_only: bool,
}

async fn scan(database_url: &str, endpoints: &Endpoints, plan: ScanPlan) -> anyhow::Result<()> {
    let store = open(database_url).await?;
    let started = now();
    store.add_seeds(&curated_seeds(started)).await?;
    let targets: Vec<Target> = store
        .due_for_scan(started - plan.fresh_hours * 3600, plan.limit)
        .await?
        .into_iter()
        .map(|(address, label)| {
            Ok(Target {
                address: address.parse::<Address>()?,
                label,
            })
        })
        .collect::<anyhow::Result<_>>()
        .context("the seed table holds a malformed address")?;

    tracing::info!(
        due = targets.len(),
        rpc = %endpoints.rpc_url,
        concurrency = plan.concurrency,
        classify_only = plan.classify_only,
        "starting scan"
    );
    let (scanner, authorities) = endpoints.live(plan.concurrency).await?;
    let resolver = (!plan.classify_only).then_some(&authorities);
    let run = scan_into(&store, &scanner, resolver, &targets, plan.batch, started).await?;

    let cov = store.coverage().await?;
    report(run, &cov);
    // Fail loudly if the store holds nothing — a silently empty scan that still serves a page
    // is the failure mode that makes a public dashboard lie.
    if cov.covered_proxies == 0 {
        anyhow::bail!("the store holds no covered proxies; refusing to report success");
    }
    Ok(())
}

/// A window cursor only means something for the stride and size it was counted in.
async fn discovery_start(store: &Store, stride: u64, window: u64) -> anyhow::Result<u64> {
    let same_grid = store.cursor(discover::STRIDE).await? == Some(stride as i64)
        && store.cursor(discover::WINDOW).await? == Some(window as i64);
    if !same_grid {
        store.set_cursor(discover::STRIDE, stride as i64).await?;
        store.set_cursor(discover::WINDOW, window as i64).await?;
        store.set_cursor(discover::NEXT_WINDOW, 0).await?;
    }
    Ok(store.cursor(discover::NEXT_WINDOW).await?.unwrap_or(0) as u64)
}

/// Read one window, seed what it admits, and move the cursor past it. The cursor moves only
/// after the seeds are written, so a run killed between the two re-reads the window rather than
/// skipping it; admitting is idempotent, so re-reading costs nothing but the request.
async fn read_window(
    store: &Store,
    reader: &Discoverer,
    w: &discover::Window,
    per_family: i64,
    (counts, seeded): &mut (HashMap<String, i64>, HashSet<String>),
) -> anyhow::Result<usize> {
    let logs = reader.logs(w.from, w.to).await?;
    let admitted = discover::admit(
        logs.iter().filter_map(discover::sighting),
        per_family,
        counts,
        seeded,
    );
    let rows: Vec<SeedRow> = admitted
        .iter()
        .map(|s| SeedRow {
            address: s.address.to_checksum(None),
            label: None,
            source: "event".into(),
            family: Some(s.family.clone()),
            first_block: s.block.map(|b| b as i64),
            discovered_at: now(),
        })
        .collect();
    store.add_seeds(&rows).await?;
    store
        .set_cursor(discover::NEXT_WINDOW, w.index as i64 + 1)
        .await?;
    tracing::info!(
        window = w.index,
        from = w.from,
        logs = logs.len(),
        admitted = rows.len(),
        "window read"
    );
    Ok(rows.len())
}

struct DiscoverPlan {
    stride: u64,
    window: u64,
    per_family: i64,
    max_windows: Option<usize>,
}

async fn discover(
    database_url: &str,
    endpoints: &Endpoints,
    plan: DiscoverPlan,
) -> anyhow::Result<()> {
    let store = open(database_url).await?;
    let provider = hermes_scan::connect(&endpoints.rpc_url).await?;
    let head = provider.get_block_number().await?;
    let start = discovery_start(&store, plan.stride, plan.window).await?;
    store.set_cursor(DISCOVERY_CAP, plan.per_family).await?;

    let mut windows = discover::windows(start, plan.stride, plan.window, head);
    windows.truncate(plan.max_windows.unwrap_or(usize::MAX));
    let mut tally = (
        store.family_counts().await?,
        store.seeded_addresses().await?,
    );
    let reader = Discoverer::new(provider, Duration::from_millis(endpoints.call_interval_ms));
    let mut admitted_total = 0;

    for w in &windows {
        admitted_total += read_window(&store, &reader, w, plan.per_family, &mut tally).await?;
    }
    println!(
        "read {} windows up to block {head}; admitted {admitted_total} new addresses \
         (at most {} per family)",
        windows.len(),
        plan.per_family
    );
    Ok(())
}

async fn record(
    address: Address,
    name: &str,
    blocks: (Option<u64>, Option<u64>),
    out_dir: &std::path::Path,
    endpoints: &Endpoints,
) -> anyhow::Result<()> {
    let base_provider = hermes_scan::connect(&endpoints.rpc_url).await?;
    let l1_provider = hermes_scan::connect(&endpoints.l1_rpc_url).await?;
    let base_block = match blocks.0 {
        Some(b) => b,
        None => base_provider.get_block_number().await?,
    };
    let l1_block = match blocks.1 {
        Some(b) => b,
        None => l1_provider.get_block_number().await?,
    };

    let base = Arc::new(RecordingRpc::new(LiveRpc::at_block(
        base_provider,
        base_block,
    )));
    let ethereum = Arc::new(RecordingRpc::new(LiveRpc::at_block(l1_provider, l1_block)));
    let (scanner, authorities) = endpoints.scanners(base.clone(), ethereum.clone(), 1);
    let target = Target {
        address,
        label: None,
    };
    let scanned = scan_and_resolve(&scanner, &authorities, &[target], now()).await;

    let fixture = Fixture {
        name: name.to_string(),
        target: address,
        blocks: [(Chain::Base, base_block), (Chain::Ethereum, l1_block)]
            .into_iter()
            .collect(),
        recorded_at: now(),
        reads: [
            (Chain::Base, base.reads()),
            (Chain::Ethereum, ethereum.reads()),
        ]
        .into_iter()
        .collect(),
    };
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&fixture)? + "\n")?;

    println!(
        "recorded {} at Base block {base_block} / Ethereum block {l1_block} to {}",
        address,
        path.display()
    );
    println!(
        "Hermes concluded (check this independently before it goes anywhere near tests/verified.json):"
    );
    println!("{}", serde_json::to_string_pretty(&scanned.records)?);
    Ok(())
}

async fn verify(table: &std::path::Path, endpoints: &Endpoints) -> anyhow::Result<()> {
    let rows = load_table(table)?;
    let (scanner, authorities) = endpoints.live(1).await?;
    let mut failed = 0;
    for row in &rows {
        let target = Target {
            address: row.address,
            label: None,
        };
        let scanned = scan_and_resolve(&scanner, &authorities, &[target], now()).await;
        let diffs = differences(&row.expected, scanned.records.first());
        if diffs.is_empty() {
            println!("PASS {:<28} {}", row.name, row.label);
        } else {
            failed += 1;
            println!("FAIL {:<28} {}", row.name, row.label);
            for d in diffs {
                println!("       {d}");
            }
        }
    }
    if failed > 0 {
        anyhow::bail!(
            "{failed} of {} verified rows no longer match the live chain",
            rows.len()
        );
    }
    println!("all {} verified rows match the live chain", rows.len());
    Ok(())
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
    match cli.command {
        Command::Scan {
            endpoints,
            concurrency,
            limit,
            fresh_hours,
            batch,
            classify_only,
        } => {
            let plan = ScanPlan {
                concurrency,
                limit,
                fresh_hours,
                batch,
                classify_only,
            };
            scan(&cli.database_url, &endpoints, plan).await
        }
        Command::Discover {
            endpoints,
            stride,
            window,
            per_family,
            max_windows,
        } => {
            let plan = DiscoverPlan {
                stride,
                window,
                per_family,
                max_windows,
            };
            discover(&cli.database_url, &endpoints, plan).await
        }
        Command::Record {
            address,
            name,
            block,
            l1_block,
            out_dir,
            endpoints,
        } => record(address, &name, (block, l1_block), &out_dir, &endpoints).await,
        Command::Verify { table, endpoints } => verify(&table, &endpoints).await,
        Command::Migrate => {
            open(&cli.database_url).await?;
            println!("database at {} is on the current schema", cli.database_url);
            Ok(())
        }
        Command::Serve { port, static_dir } => {
            hermes_api::serve(open(&cli.database_url).await?, static_dir, port).await
        }
    }
}
