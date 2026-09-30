//! What changed, when, and at which block.
//!
//! `diff` is the monitoring product: it prints the changes in a range and exits non-zero when
//! there is at least one, so a cron job or CI step can act on it without parsing anything. The
//! scheduling, the paging and the deciding what matters stay with whoever runs it.

use alloy::providers::DynProvider;
use hermes_core::graph_store::{EventFilter, StoredEvent};
use hermes_core::time::{parse_since, short};
use hermes_core::view::{event_line, event_when};
use hermes_core::{Cause, Chain, Store};
use hermes_scan::pin::{LiveArchive, pin, pinnable};
use std::process::ExitCode;
use std::time::Duration;

fn print_event(e: &StoredEvent) {
    let tag = match e.cause {
        Cause::Chain => "",
        Cause::Reinterpretation => "  [reinterpretation: Hermes reads this differently now]",
    };
    println!(
        "{}  {}  {}{tag}",
        short(e.observed_at),
        event_line(e),
        event_when(e)
    );
}

pub async fn history(
    store: &Store,
    address: &str,
    chain: Option<Chain>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    let events = store
        .events(&EventFilter {
            address: Some(address.to_string()),
            chain,
            include_reinterpretations: all,
            ascending: true,
            ..Default::default()
        })
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }
    if events.is_empty() {
        println!(
            "no recorded changes for {address}. History starts when Hermes first saw it; \
             anything before that is not in the store."
        );
    }
    for e in &events {
        print_event(e);
    }
    Ok(())
}

/// Which changes `diff` looks at.
pub struct Range {
    pub since: Option<String>,
    pub after_block: Option<i64>,
    pub to_block: Option<i64>,
    pub all: bool,
}

/// Print the changes in `range` and say whether there were any chain changes, which is the
/// exit code: 1 when something moved, 0 when nothing did.
pub async fn diff(store: &Store, range: Range, now: i64, json: bool) -> anyhow::Result<ExitCode> {
    let since = match &range.since {
        Some(s) => Some(parse_since(s, now).ok_or_else(|| {
            anyhow::anyhow!("cannot read --since {s}; use 24h, 7d, a date, or Unix seconds")
        })?),
        None => None,
    };
    let events = store
        .events(&EventFilter {
            since,
            after_block: range.after_block,
            to_block: range.to_block,
            include_reinterpretations: range.all,
            ascending: true,
            ..Default::default()
        })
        .await?;
    let moved = events.iter().filter(|e| e.cause == Cause::Chain).count();
    if json {
        println!("{}", serde_json::to_string_pretty(&events)?);
    } else {
        let mut subject = None;
        for e in &events {
            if subject != Some(e.subject) {
                subject = Some(e.subject);
                println!();
            }
            print_event(e);
        }
        println!(
            "\n{moved} change{} on chain in range{}",
            if moved == 1 { "" } else { "s" },
            if range.all {
                format!(", {} reinterpretation(s)", events.len() - moved)
            } else {
                String::new()
            }
        );
    }
    Ok(if moved > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Bisect every chain event not yet pinned to a block.
pub async fn pin_events(
    store: &Store,
    base: DynProvider,
    ethereum: DynProvider,
    interval: Duration,
    limit: Option<i64>,
) -> anyhow::Result<()> {
    let archive = LiveArchive { base, ethereum };
    let events = store.unpinned_events(limit).await?;
    let (mut pinned, mut skipped, mut failed) = (0, 0, 0);
    for e in events.iter() {
        if !pinnable(e) {
            skipped += 1;
            continue;
        }
        match pin(&archive, e, interval).await {
            Some(block) => {
                store.pin_event(e.id, block as i64).await?;
                pinned += 1;
                println!("pinned  {}  at block {block}", event_line(e));
            }
            None => {
                failed += 1;
                println!(
                    "unpinned  {}  (left bracketed; tried again next run)",
                    event_line(e)
                );
            }
        }
    }
    println!(
        "{pinned} pinned, {failed} could not be pinned this time, {skipped} are verdicts \
         assembled from many reads and keep their bracketing blocks"
    );
    Ok(())
}
