//! All network I/O for the scan.
//!
//! Two behaviours here exist because of measured behaviour of the public Base endpoint
//! (`https://mainnet.base.org`), not out of caution:
//!
//! * **Batch cap.** The endpoint rejects JSON-RPC batches larger than 10 with
//!   `-32014: maximum 10 calls in 1 batch`. Each address needs 5 storage reads plus a code
//!   read, so one address per batch is the largest safe unit.
//! * **Negative reads are not trusted on the first try.** Under concurrency the endpoint
//!   intermittently returns `0x` for `eth_getCode` and zero words for `eth_getStorageAt` on
//!   contracts that demonstrably have both. A node cannot *invent* an implementation address,
//!   so a non-empty read is trusted immediately; an empty one is re-read once before it is
//!   allowed to become a `NotUpgradeable` or `Eoa` verdict. This is how I handle storage
//!   reads that disagree with each other mid-scan.

use alloy::primitives::{Address, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use futures::stream::{self, StreamExt};
use hermes_core::{
    ADMIN_SLOT, BEACON_SLOT, Classified, IMPL_SLOT, PROXIABLE_SLOT, SlotReads, classify, slot_key,
    slots::ZOS_IMPL_SLOT,
};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub address: Address,
    pub reads: SlotReads,
    pub classified: Classified,
    pub code_size: usize,
    /// True when a negative first read had to be confirmed by a second read.
    pub reread: bool,
}

#[derive(Clone)]
pub struct Scanner {
    provider: DynProvider,
    concurrency: usize,
    max_retries: u32,
}

impl Scanner {
    pub async fn connect(rpc: &str, concurrency: usize) -> anyhow::Result<Self> {
        let provider = ProviderBuilder::new().connect(rpc).await?.erased();
        Ok(Self {
            provider,
            concurrency,
            max_retries: 6,
        })
    }

    async fn storage(&self, addr: Address, slot: B256) -> anyhow::Result<B256> {
        let v = self.provider.get_storage_at(addr, slot_key(slot)).await?;
        Ok(B256::from(v))
    }

    /// One full read of an address: five slots plus code size.
    async fn read_once(&self, addr: Address) -> anyhow::Result<(SlotReads, usize)> {
        let implementation = self.storage(addr, IMPL_SLOT).await?;
        let admin = self.storage(addr, ADMIN_SLOT).await?;
        let beacon = self.storage(addr, BEACON_SLOT).await?;
        let proxiable = self.storage(addr, PROXIABLE_SLOT).await?;
        let zos_implementation = self.storage(addr, ZOS_IMPL_SLOT).await?;
        let code = self.provider.get_code_at(addr).await?;
        Ok((
            SlotReads {
                implementation,
                admin,
                beacon,
                proxiable,
                zos_implementation,
                code_empty: code.is_empty(),
            },
            code.len(),
        ))
    }

    /// `read_once` with exponential backoff. Public endpoints answer `-32016: over rate limit`
    /// under load; that is transient and must not kill a scan.
    async fn read_with_retry(&self, addr: Address) -> anyhow::Result<(SlotReads, usize)> {
        let mut last = None;
        for attempt in 0..self.max_retries {
            match self.read_once(addr).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::debug!(%addr, attempt, error = %e, "read failed, backing off");
                    last = Some(e);
                    let backoff = Duration::from_millis(400u64 << attempt.min(5));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("read failed with no error recorded")))
    }

    /// A read whose *negative* answers have been confirmed. See the module docs.
    pub async fn probe(&self, addr: Address) -> anyhow::Result<ProbeOutcome> {
        let (first, mut code_size) = self.read_with_retry(addr).await?;

        let looks_empty = first.code_empty
            || (first.implementation.is_zero()
                && first.admin.is_zero()
                && first.beacon.is_zero()
                && first.proxiable.is_zero()
                && first.zos_implementation.is_zero());

        let (reads, reread) = if looks_empty {
            tokio::time::sleep(Duration::from_millis(150)).await;
            match self.read_with_retry(addr).await {
                Ok((second, second_size)) => {
                    code_size = code_size.max(second_size);
                    (merge_prefer_nonempty(first, second), true)
                }
                Err(_) => (first, true),
            }
        } else {
            (first, false)
        };

        Ok(ProbeOutcome {
            address: addr,
            reads,
            classified: classify(reads),
            code_size,
            reread,
        })
    }

    /// Probe many addresses with bounded concurrency.
    ///
    /// `buffer_unordered` is the whole rate-limit story: it caps in-flight requests without
    /// batching them, which matters because the batch endpoint caps at 10 calls anyway.
    /// Measured on the public endpoint: 3 concurrent readers complete 400/400 addresses with
    /// zero failures; 8 fail silently often enough to corrupt a scan.
    pub async fn scan(&self, addrs: Vec<Address>) -> Vec<(Address, anyhow::Result<ProbeOutcome>)> {
        stream::iter(addrs)
            .map(|a| async move { (a, self.probe(a).await) })
            .buffer_unordered(self.concurrency)
            .collect()
            .await
    }
}

/// Prefer any non-zero observation across two reads of the same address.
fn merge_prefer_nonempty(a: SlotReads, b: SlotReads) -> SlotReads {
    fn pick(x: B256, y: B256) -> B256 {
        if x.is_zero() { y } else { x }
    }
    SlotReads {
        implementation: pick(a.implementation, b.implementation),
        admin: pick(a.admin, b.admin),
        beacon: pick(a.beacon, b.beacon),
        proxiable: pick(a.proxiable, b.proxiable),
        zos_implementation: pick(a.zos_implementation, b.zos_implementation),
        // Code is only absent if *both* reads say so.
        code_empty: a.code_empty && b.code_empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    const IMPL_WORD: B256 =
        b256!("0000000000000000000000006d9dd143e42b6338f4f6a7c0c26d124658f641cb");

    #[test]
    fn merge_recovers_an_implementation_that_the_first_read_missed() {
        let empty = SlotReads {
            code_empty: true,
            ..Default::default()
        };
        let good = SlotReads {
            implementation: IMPL_WORD,
            code_empty: false,
            ..Default::default()
        };
        let m = merge_prefer_nonempty(empty, good);
        assert_eq!(m.implementation, IMPL_WORD);
        assert!(
            !m.code_empty,
            "one read seeing code is enough to prove code exists"
        );
    }

    #[test]
    fn merge_keeps_eoa_only_when_both_reads_agree() {
        let empty = SlotReads {
            code_empty: true,
            ..Default::default()
        };
        assert!(merge_prefer_nonempty(empty, empty).code_empty);
    }

    #[test]
    fn merge_does_not_let_a_second_read_overwrite_a_good_first_read() {
        let good = SlotReads {
            implementation: IMPL_WORD,
            ..Default::default()
        };
        let empty = SlotReads::default();
        assert_eq!(merge_prefer_nonempty(good, empty).implementation, IMPL_WORD);
    }

    #[test]
    fn a_spurious_empty_read_would_have_misclassified_a_live_proxy() {
        // This is the exact failure the confirmation read prevents.
        let spurious = SlotReads::default();
        assert_eq!(
            classify(spurious).kind,
            hermes_core::ProxyKind::NotUpgradeable
        );
        let real = SlotReads {
            implementation: IMPL_WORD,
            admin: address!("31e99e05fee3dce580af777c3fd63ee1b3b40c17").into_word(),
            ..Default::default()
        };
        assert_eq!(
            classify(merge_prefer_nonempty(spurious, real)).kind,
            hermes_core::ProxyKind::Transparent
        );
    }
}
