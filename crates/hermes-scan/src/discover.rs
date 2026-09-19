//! Finding proxies chain-wide by the standard Hermes already reads.
//!
//! ERC-1967 proxies announce themselves: `Upgraded(address)` when an implementation is set,
//! `BeaconUpgraded(address)` when a beacon is, `AdminChanged(address,address)` when an admin is.
//! Paging `eth_getLogs` for those three topics finds proxies by the same standard the slot
//! reads classify them by, which fits the thesis better than any curated list.
//!
//! Two measurements (2026-09-19, `mainnet.base.org`) shape it:
//!
//! * **The endpoint caps a log response by size, not by range.** 1,000 blocks of these topics
//!   came back fine; 10,000 answered HTTP 413. A window that is still too large is split.
//! * **Recent history is mostly clones.** In one recent 1,000-block window, 248 of 374
//!   upgrades pointed at one implementation. So windows are spread evenly over the whole
//!   history, and each family (an implementation, beacon or admin) admits only a few
//!   addresses. Over 103 windows: 24,664 distinct proxies in 478 families; at three per
//!   family, 813 addresses. Without the cap the index would be a few contracts copied
//!   hundreds of times, each owned by whoever deployed that copy.

use crate::resolve::word_to_address_strict;
use alloy::primitives::{Address, B256, b256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::{Filter, Log};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// `keccak256("Upgraded(address)")`
pub const UPGRADED: B256 =
    b256!("bc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b");
/// `keccak256("BeaconUpgraded(address)")`
pub const BEACON_UPGRADED: B256 =
    b256!("1cf3b03a6cf19fa2baba4df148e9dcabedea7f8a5c07840e207e5c089be95d3e");
/// `keccak256("AdminChanged(address,address)")`
pub const ADMIN_CHANGED: B256 =
    b256!("7e644d79422f17c01e4894b5f4f588d331ebfa28653d42ae832dc59e38c9798f");

/// Cursor names in the store.
pub const NEXT_WINDOW: &str = "discover.next_window";
pub const STRIDE: &str = "discover.stride";
pub const WINDOW: &str = "discover.window";

/// A block range to read logs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub index: u64,
    pub from: u64,
    pub to: u64,
}

/// Windows from `start` on that lie wholly at or below `head`. Window `i` covers
/// `[i * stride, i * stride + size)`.
///
/// Fixed positions are what make discovery resumable and incremental at once: a resumed run
/// carries on at the next index, and as the chain grows new windows simply come into range.
pub fn windows(start: u64, stride: u64, size: u64, head: u64) -> Vec<Window> {
    if stride == 0 || size == 0 {
        return Vec::new();
    }
    (start..)
        .map(|index| Window {
            index,
            from: index * stride,
            to: index * stride + size - 1,
        })
        .take_while(|w| w.to <= head)
        .collect()
}

/// One proxy seen in one log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub address: Address,
    /// What the per-family cap counts against.
    pub family: String,
    pub block: Option<u64>,
}

/// What one log says, or `None` for a log that is not one of the three events or is malformed.
///
/// Logs are chain data a stranger wrote, so topics are counted before they are indexed and
/// addresses are decoded strictly: dirty upper bytes or zero mean no sighting.
pub fn sighting(log: &Log) -> Option<Sighting> {
    let topics = log.topics();
    let family = match *topics.first()? {
        t if t == UPGRADED => format!(
            "impl:{:#x}",
            word_to_address_strict(topics.get(1)?.as_slice())?
        ),
        t if t == BEACON_UPGRADED => {
            format!(
                "beacon:{:#x}",
                word_to_address_strict(topics.get(1)?.as_slice())?
            )
        }
        t if t == ADMIN_CHANGED => {
            // Neither argument is indexed: data is (previousAdmin, newAdmin).
            let new_admin = log.data().data.get(32..64)?;
            format!("admin:{:#x}", word_to_address_strict(new_admin)?)
        }
        _ => return None,
    };
    Some(Sighting {
        address: log.address(),
        family,
        block: log.block_number,
    })
}

/// The sightings that make it into the seed: each address once, and at most `cap` per family,
/// counting what earlier runs already admitted. `counts` and `seeded` are updated in place so a
/// run spanning many windows keeps one running tally.
pub fn admit(
    sightings: impl IntoIterator<Item = Sighting>,
    cap: i64,
    counts: &mut HashMap<String, i64>,
    seeded: &mut HashSet<String>,
) -> Vec<Sighting> {
    let mut admitted = Vec::new();
    for s in sightings {
        let key = format!("{:#x}", s.address);
        if seeded.contains(&key) {
            continue;
        }
        let n = counts.entry(s.family.clone()).or_insert(0);
        if *n >= cap {
            continue;
        }
        *n += 1;
        seeded.insert(key);
        admitted.push(s);
    }
    admitted
}

/// How many times to ask for one range before giving up on the run.
const RETRIES: u32 = 5;

/// The smallest range worth splitting down to before calling a window unreadable.
const MIN_SPLIT: u64 = 16;

/// Whether an error says the answer was too big, which splitting fixes, as opposed to the
/// endpoint being busy, which waiting fixes. `over rate limit` must not land here: splitting in
/// answer to it doubles the request rate against a limiter that is already refusing.
fn too_large(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("413")
        || e.contains("too large")
        || e.contains("returned more than")
        || e.contains("block range")
        || e.contains("blocks range")
}

/// Reads logs for windows from one endpoint.
pub struct Discoverer {
    provider: DynProvider,
    interval: Duration,
}

impl Discoverer {
    pub fn new(provider: DynProvider, interval: Duration) -> Self {
        Self { provider, interval }
    }

    /// Every proxy event in `[from, to]`, halving the range while the endpoint says the answer
    /// is too large. An error means the range could not be read at all, and the caller stops
    /// rather than skip it: a skipped window is a silent hole in the sample.
    pub async fn logs(&self, from: u64, to: u64) -> anyhow::Result<Vec<Log>> {
        let mut pending = vec![(from, to)];
        let mut out = Vec::new();
        while let Some((a, b)) = pending.pop() {
            match self.range(a, b).await {
                Ok(logs) => out.extend(logs),
                Err(e) if too_large(&e.to_string()) && b - a >= MIN_SPLIT => {
                    let mid = a + (b - a) / 2;
                    pending.push((mid + 1, b));
                    pending.push((a, mid));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    async fn range(&self, from: u64, to: u64) -> anyhow::Result<Vec<Log>> {
        let filter = Filter::new()
            .from_block(from)
            .to_block(to)
            .event_signature(vec![UPGRADED, BEACON_UPGRADED, ADMIN_CHANGED]);
        let mut last = None;
        for attempt in 0..RETRIES {
            tokio::time::sleep(self.interval).await;
            match self.provider.get_logs(&filter).await {
                Ok(logs) => return Ok(logs),
                Err(e) if too_large(&e.to_string()) => return Err(e.into()),
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(Duration::from_millis(1000 << attempt.min(5))).await;
                }
            }
        }
        Err(last.map_or_else(|| anyhow::anyhow!("no attempt made"), Into::into))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, LogData, address, keccak256};

    /// Derived, not restated: a wrong topic finds nothing, and nothing looks exactly like an
    /// empty stretch of chain.
    #[test]
    fn topics_match_their_event_signatures() {
        assert_eq!(UPGRADED, keccak256("Upgraded(address)"));
        assert_eq!(BEACON_UPGRADED, keccak256("BeaconUpgraded(address)"));
        assert_eq!(ADMIN_CHANGED, keccak256("AdminChanged(address,address)"));
    }

    fn log(emitter: Address, topics: Vec<B256>, data: Vec<u8>) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: emitter,
                data: LogData::new_unchecked(topics, Bytes::from(data)),
            },
            block_number: Some(7),
            ..Default::default()
        }
    }

    const PROXY: Address = address!("00000000000000000000000000000000000000a1");
    const TARGET: Address = address!("00000000000000000000000000000000000000b2");

    #[test]
    fn each_event_names_its_family() {
        let up = log(PROXY, vec![UPGRADED, TARGET.into_word()], vec![]);
        let beacon = log(PROXY, vec![BEACON_UPGRADED, TARGET.into_word()], vec![]);
        let mut data = vec![0u8; 32];
        data.extend_from_slice(TARGET.into_word().as_slice());
        let admin = log(PROXY, vec![ADMIN_CHANGED], data);
        assert_eq!(
            sighting(&up).unwrap().family,
            "impl:0x00000000000000000000000000000000000000b2"
        );
        assert!(sighting(&beacon).unwrap().family.starts_with("beacon:"));
        assert!(sighting(&admin).unwrap().family.starts_with("admin:"));
        assert_eq!(sighting(&up).unwrap().address, PROXY);
        assert_eq!(sighting(&up).unwrap().block, Some(7));
    }

    #[test]
    fn a_malformed_or_foreign_log_is_not_a_sighting() {
        let mut dirty = TARGET.into_word();
        dirty.0[0] = 0xff;
        assert_eq!(
            sighting(&log(PROXY, vec![UPGRADED], vec![])),
            None,
            "missing topic"
        );
        assert_eq!(sighting(&log(PROXY, vec![UPGRADED, dirty], vec![])), None);
        assert_eq!(
            sighting(&log(PROXY, vec![ADMIN_CHANGED], vec![0; 40])),
            None,
            "short data"
        );
        assert_eq!(sighting(&log(PROXY, vec![B256::ZERO], vec![])), None);
        assert_eq!(sighting(&log(PROXY, vec![], vec![])), None);
    }

    fn seen(address: Address, family: &str) -> Sighting {
        Sighting {
            address,
            family: family.into(),
            block: None,
        }
    }

    #[test]
    fn a_family_admits_no_more_than_the_cap_across_runs() {
        let mut counts = HashMap::from([("impl:x".to_string(), 1)]);
        let mut seeded = HashSet::new();
        let batch = (1..=5u8).map(|i| seen(Address::with_last_byte(i), "impl:x"));
        let admitted = admit(batch, 3, &mut counts, &mut seeded);
        assert_eq!(admitted.len(), 2, "one was admitted by an earlier run");
        assert_eq!(counts["impl:x"], 3);
    }

    #[test]
    fn an_address_is_admitted_once_whatever_its_events() {
        let mut counts = HashMap::new();
        let mut seeded = HashSet::from([format!("{:#x}", Address::with_last_byte(9))]);
        let batch = vec![
            seen(Address::with_last_byte(1), "impl:x"),
            seen(Address::with_last_byte(1), "admin:y"),
            seen(Address::with_last_byte(9), "impl:z"),
        ];
        let admitted = admit(batch, 3, &mut counts, &mut seeded);
        assert_eq!(admitted.len(), 1);
        assert_eq!(
            counts.get("impl:z"),
            None,
            "an already-seeded address costs nothing"
        );
    }

    #[test]
    fn a_rate_limit_is_waited_out_not_split() {
        assert!(too_large("HTTP error 413 Payload Too Large"));
        assert!(too_large("query returned more than 10000 results"));
        assert!(too_large("eth_getLogs is limited to 0 - 50 blocks range"));
        assert!(!too_large(
            "server returned an error response: error code -32016: over rate limit"
        ));
        assert!(!too_large("HTTP error 429 Too Many Requests"));
    }

    #[test]
    fn windows_sit_at_fixed_positions_and_stop_at_the_head() {
        let w = windows(0, 500_000, 1000, 1_600_000);
        assert_eq!(w.len(), 4);
        assert_eq!((w[3].from, w[3].to), (1_500_000, 1_500_999));
        assert_eq!(
            windows(2, 500_000, 1000, 1_600_000)[0].index,
            2,
            "resumes, not restarts"
        );
        assert!(
            windows(0, 500_000, 1000, 1_500_500)
                .iter()
                .all(|w| w.to <= 1_500_500),
            "a window the head has not passed yet is not read half-empty"
        );
        assert!(windows(0, 0, 1000, 10).is_empty());
    }
}
