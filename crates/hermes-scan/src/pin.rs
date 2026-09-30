//! Finding the block a change happened at.
//!
//! A scan knows a value was one thing at the last observation and another at this one. The
//! public endpoint answers `eth_call` and `eth_getStorageAt` at any historical block, so the
//! first block the new value holds at can be found by bisection: about sixteen reads for a day
//! of Base blocks and one more per doubling of the bracket. Reading logs instead costs a request
//! per 2,000 blocks (the most `eth_getLogs` range the endpoint accepts, measured 2026-09-30):
//! about 22 for a day, but about 200 for the nine days between the Sep 20 and Sep 29 scans. And
//! logs need the event each kind of contract emits for each kind of change; a read at a block
//! needs only the getter the scan already uses.
//!
//! Bisection finds a boundary, not a history. If the value changed and changed back between two
//! scans, or changed twice, the block found is one where the new value holds after one where it
//! did not, and the event keeps saying "at least one change" in its bracketing blocks. A read
//! the node would not answer stops the search; the event stays unpinned and is tried again
//! next time, because a guessed block is worse than none.

use crate::resolve::{
    decode_address_array, is_revert, selector, word_to_address_strict, word_to_u256,
};
use crate::rpc::{ChainRpc, LiveRpc};
use alloy::primitives::{Address, B256, Bytes};
use alloy::providers::DynProvider;
use hermes_core::graph_store::{StoredEvent, SubjectType};
use hermes_core::store::checksum;
use hermes_core::{ADMIN_SLOT, BEACON_SLOT, Chain, Field, IMPL_SLOT};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// A reader at any block of either chain.
pub trait Archive: Send + Sync {
    fn at(&self, chain: Chain, block: u64) -> Arc<dyn ChainRpc>;
}

/// Both chains' public endpoints, which serve historical state.
pub struct LiveArchive {
    pub base: DynProvider,
    pub ethereum: DynProvider,
}

impl Archive for LiveArchive {
    fn at(&self, chain: Chain, block: u64) -> Arc<dyn ChainRpc> {
        let provider = match chain {
            Chain::Base => self.base.clone(),
            Chain::Ethereum => self.ethereum.clone(),
        };
        Arc::new(LiveRpc::at_block(provider, block))
    }
}

/// The first block in `(lo, hi]` at which `holds` is true, given that it is false at `lo` and
/// true at `hi`. Both ends are checked before searching, because an event whose brackets no
/// longer read the way the scan saw them cannot be pinned honestly. `None` whenever a read is
/// unanswered or the ends disagree with the event.
pub async fn first_block_where<F, Fut>(mut lo: u64, mut hi: u64, mut holds: F) -> Option<u64>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Option<bool>>,
{
    if lo >= hi || holds(lo).await? || !holds(hi).await? {
        return None;
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if holds(mid).await? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// Whether bisection can say anything about this field: it has to be one read, not a verdict
/// the resolver assembled from many.
pub fn pinnable(e: &StoredEvent) -> bool {
    matches!(
        (e.subject_type, e.change.field),
        (
            SubjectType::Proxy,
            Field::Implementation | Field::Admin | Field::Beacon
        ) | (
            SubjectType::Node,
            Field::Owner
                | Field::SafeThreshold
                | Field::TimelockDelay
                | Field::SafeOwnerAdded
                | Field::SafeOwnerRemoved
        )
    )
}

const RETRIES: u32 = 5;

/// Waits between retries. The public endpoint's `eth_call` limiter stays tripped until it has
/// seen about seven seconds of quiet (measured 2026-09-19), and a retry inside that window
/// extends it: backing off from half a second lost every read of a bisection on 2026-09-30.
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(2_000 << attempt.min(4))
}

async fn call(rpc: &dyn ChainRpc, to: Address, input: Vec<u8>) -> Option<Option<Bytes>> {
    for attempt in 0..RETRIES {
        match rpc.call(to, Bytes::from(input.clone())).await {
            Ok(out) => return Some((!out.is_empty()).then_some(out)),
            Err(e) if is_revert(&e.to_string()) => return Some(None),
            Err(e) => {
                tracing::debug!(%to, attempt, error = %e, "archive call failed, backing off");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    }
    None
}

async fn storage(rpc: &dyn ChainRpc, at: Address, slot: B256) -> Option<B256> {
    for attempt in 0..RETRIES {
        match rpc.storage(at, slot).await {
            Ok(word) => return Some(word),
            Err(e) => {
                tracing::debug!(%at, attempt, error = %e, "archive read failed, backing off");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    }
    None
}

/// The field's value at the reader's block, in the form events store it. `None` when the node
/// would not answer; `Some(None)` when the field is absent there.
pub async fn read_field(rpc: &dyn ChainRpc, e: &StoredEvent) -> Option<Option<String>> {
    let at = e.subject.address;
    let slot = match e.change.field {
        Field::Implementation => Some(IMPL_SLOT),
        Field::Admin => Some(ADMIN_SLOT),
        Field::Beacon => Some(BEACON_SLOT),
        _ => None,
    };
    if let Some(slot) = slot {
        let word = storage(rpc, at, slot).await?;
        return Some(word_to_address_strict(word.as_slice()).map(checksum));
    }
    let number = |b: Option<Bytes>| b.and_then(|b| word_to_u256(&b)).map(|v| v.to_string());
    Some(match e.change.field {
        Field::Owner => call(rpc, at, selector("owner()").to_vec())
            .await?
            .and_then(|b| word_to_address_strict(&b))
            .map(checksum),
        Field::SafeThreshold => number(call(rpc, at, selector("getThreshold()").to_vec()).await?),
        Field::TimelockDelay => number(call(rpc, at, selector("getMinDelay()").to_vec()).await?),
        Field::SafeOwnerAdded | Field::SafeOwnerRemoved => {
            let signer = e.change.new.as_ref().or(e.change.old.as_ref())?;
            let owners = call(rpc, at, selector("getOwners()").to_vec())
                .await?
                .and_then(|b| decode_address_array(&b))
                .unwrap_or_default();
            owners
                .iter()
                .any(|o| checksum(*o).eq_ignore_ascii_case(signer))
                .then(|| signer.clone())
        }
        _ => return None,
    })
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (a, b) => a.is_none() && b.is_none(),
    }
}

/// The first block the event's new value holds at, or `None` when that cannot be established.
/// `interval` spaces the reads, for an endpoint that refuses bursts.
pub async fn pin(archive: &dyn Archive, e: &StoredEvent, interval: Duration) -> Option<u64> {
    if !pinnable(e) {
        return None;
    }
    let lo = u64::try_from(e.last_old_block?).ok()?;
    let hi = u64::try_from(e.first_new_block?).ok()?;
    let chain = e.subject.chain;
    first_block_where(lo, hi, |block| async move {
        tokio::time::sleep(interval).await;
        let rpc = archive.at(chain, block);
        let value = read_field(rpc.as_ref(), e).await?;
        Some(same(&value, &e.change.new))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use futures::future::BoxFuture;
    use hermes_core::graph_store::SubjectType;
    use hermes_core::{Cause, Change, Node};

    #[tokio::test]
    async fn bisection_finds_the_first_block_the_new_value_holds_at() {
        let at = |switch: u64| move |b: u64| async move { Some(b >= switch) };
        assert_eq!(first_block_where(100, 200, at(137)).await, Some(137));
        assert_eq!(first_block_where(100, 200, at(200)).await, Some(200));
        assert_eq!(first_block_where(100, 200, at(101)).await, Some(101));
        assert_eq!(
            first_block_where(51_927_177 - 400_000, 51_927_178 + 50_000, at(51_927_178)).await,
            Some(51_927_178)
        );
    }

    #[tokio::test]
    async fn ends_that_disagree_with_the_event_pin_nothing() {
        let already = |_b: u64| async { Some(true) };
        assert_eq!(first_block_where(100, 200, already).await, None);
        let never = |_b: u64| async { Some(false) };
        assert_eq!(first_block_where(100, 200, never).await, None);
        let nothing_between = |b: u64| async move { Some(b >= 100) };
        assert_eq!(first_block_where(100, 100, nothing_between).await, None);
    }

    /// A guessed block is worse than none: one unanswered read abandons the search.
    #[tokio::test]
    async fn an_unanswered_read_abandons_the_search() {
        let flaky = |b: u64| async move { if b == 150 { None } else { Some(b >= 137) } };
        assert_eq!(first_block_where(100, 200, flaky).await, None);
    }

    const BEACON: Address = address!("e68ED13998fd48497EAA3b52e20823605D8d7706");
    const EOA: Address = address!("21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc");
    const SAFE: Address = address!("6454cf0127a153295435160768C85225Cd19Bf15");

    /// The beacon of 2026-09-29, answering `owner()` as the chain did on either side of the
    /// transfer at 51,927,178.
    struct Sep29(u64);

    impl ChainRpc for Sep29 {
        fn storage(&self, _: Address, _: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
            Box::pin(async { anyhow::bail!("not asked") })
        }
        fn code(&self, _: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
            Box::pin(async { anyhow::bail!("not asked") })
        }
        fn call(&self, to: Address, input: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
            let owner = if self.0 >= 51_927_178 { SAFE } else { EOA };
            Box::pin(async move {
                assert_eq!(to, BEACON);
                assert_eq!(input.as_ref(), selector("owner()"));
                Ok(Bytes::from(owner.into_word().to_vec()))
            })
        }
    }

    struct Sep29Archive;

    impl Archive for Sep29Archive {
        fn at(&self, chain: Chain, block: u64) -> Arc<dyn ChainRpc> {
            assert_eq!(chain, Chain::Base);
            Arc::new(Sep29(block))
        }
    }

    fn owner_event(last_old: i64, first_new: i64) -> StoredEvent {
        StoredEvent {
            id: 1,
            observation_id: 2,
            subject_type: SubjectType::Node,
            subject: Node::base(BEACON),
            change: Change {
                field: Field::Owner,
                old: Some(checksum(EOA)),
                new: Some(checksum(SAFE)),
            },
            direction: None,
            last_old_block: Some(last_old),
            first_new_block: Some(first_new),
            at_block: None,
            cause: Cause::Chain,
            model_version: 1,
            observed_at: 0,
        }
    }

    /// The Sep 20 and Sep 29 scans bracket the transfer by some 400,000 blocks. Bisection
    /// narrows it to the block found by hand.
    #[tokio::test]
    async fn the_sep29_transfer_pins_to_the_block_found_by_hand() {
        let e = owner_event(51_588_474, 51_998_874);
        assert_eq!(
            pin(&Sep29Archive, &e, Duration::ZERO).await,
            Some(51_927_178)
        );
    }

    #[tokio::test]
    async fn a_verdict_assembled_from_many_reads_is_not_pinned() {
        let mut e = owner_event(1, 2);
        e.subject_type = SubjectType::Proxy;
        e.change.field = Field::KeysRequired;
        assert!(!pinnable(&e));
        assert_eq!(pin(&Sep29Archive, &e, Duration::ZERO).await, None);
    }
}
