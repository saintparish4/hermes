//! The one seam between the scanner and a node.
//!
//! Every read in this crate goes through `ChainRpc`, which is what lets one address's reads be
//! recorded against a live node at a pinned block and replayed offline forever after. Replay is
//! keyed by (chain, method, address, slot or calldata), never by arrival order:
//! `buffer_unordered` interleaves addresses, and a first-in-first-out mock would make any test
//! over more than one address flaky by construction.

use alloy::eips::BlockId;
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use futures::future::BoxFuture;
use hermes_core::Chain;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The three reads Hermes makes. Nothing else touches a node.
pub trait ChainRpc: Send + Sync {
    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>>;
    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>>;
    /// A revert comes back as an error whose text says so; `resolve::is_revert` tells it apart
    /// from a node that failed to answer.
    fn call(&self, to: Address, input: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>>;
}

/// A real node, reading `latest` or one pinned block.
pub struct LiveRpc {
    provider: DynProvider,
    block: Option<u64>,
}

impl LiveRpc {
    pub fn new(provider: DynProvider) -> Self {
        Self {
            provider,
            block: None,
        }
    }

    /// Every read at `block`, so a recording describes one moment of the chain rather than
    /// however many blocks the recording happened to span.
    pub fn at_block(provider: DynProvider, block: u64) -> Self {
        Self {
            provider,
            block: Some(block),
        }
    }

    fn block_id(&self) -> BlockId {
        self.block.map_or(BlockId::latest(), BlockId::number)
    }
}

impl ChainRpc for LiveRpc {
    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        Box::pin(async move {
            let word = self
                .provider
                .get_storage_at(address, U256::from_be_bytes(slot.0))
                .block_id(self.block_id())
                .await?;
            Ok(B256::from(word))
        })
    }

    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            Ok(self
                .provider
                .get_code_at(address)
                .block_id(self.block_id())
                .await?)
        })
    }

    fn call(&self, to: Address, input: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            let tx = TransactionRequest::default().to(to).input(input.into());
            Ok(self.provider.call(tx).block(self.block_id()).await?)
        })
    }
}

/// What a code read established. The scanner only ever asks whether code exists and how big
/// it is, so that is what a fixture keeps; the hash is there so a human can check it against
/// an explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRead {
    pub size: usize,
    pub keccak256: B256,
}

/// How an `eth_call` settled. Transport failures are never recorded: they are the node's
/// state, not the chain's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallRead {
    Ok(Bytes),
    Revert(String),
}

/// Every answer one chain gave, keyed so replay never depends on order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainReads {
    #[serde(default)]
    pub storage: BTreeMap<Address, BTreeMap<B256, B256>>,
    #[serde(default)]
    pub code: BTreeMap<Address, CodeRead>,
    #[serde(default)]
    pub call: BTreeMap<Address, BTreeMap<Bytes, CallRead>>,
}

/// A live reader that keeps every settled answer it passes through.
pub struct RecordingRpc {
    inner: LiveRpc,
    reads: Mutex<ChainReads>,
}

impl RecordingRpc {
    pub fn new(inner: LiveRpc) -> Self {
        Self {
            inner,
            reads: Mutex::new(ChainReads::default()),
        }
    }

    pub fn reads(&self) -> ChainReads {
        self.reads.lock().expect("recording lock poisoned").clone()
    }

    fn keep(&self, f: impl FnOnce(&mut ChainReads)) {
        f(&mut self.reads.lock().expect("recording lock poisoned"));
    }
}

impl ChainRpc for RecordingRpc {
    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        Box::pin(async move {
            let word = self.inner.storage(address, slot).await?;
            self.keep(|r| {
                r.storage.entry(address).or_default().insert(slot, word);
            });
            Ok(word)
        })
    }

    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            let code = self.inner.code(address).await?;
            let read = CodeRead {
                size: code.len(),
                keccak256: keccak256(&code),
            };
            self.keep(|r| {
                r.code.insert(address, read);
            });
            Ok(code)
        })
    }

    fn call(&self, to: Address, input: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            let result = self.inner.call(to, input.clone()).await;
            let settled = match &result {
                Ok(out) => Some(CallRead::Ok(out.clone())),
                Err(e) if crate::resolve::is_revert(&e.to_string()) => {
                    Some(CallRead::Revert(e.to_string()))
                }
                Err(_) => None,
            };
            if let Some(read) = settled {
                self.keep(|r| {
                    r.call.entry(to).or_default().insert(input, read);
                });
            }
            result
        })
    }
}

/// Answers from a recording, and nothing else.
///
/// A read the recording does not contain panics rather than erroring. An error would look like
/// a node outage, and the scanner would spend a minute backing off before reporting the address
/// as undetermined: a slow, indirect failure. The real meaning is that the code now asks
/// something it did not ask when the fixture was recorded, and the fixture needs re-recording.
pub struct ReplayRpc {
    label: String,
    reads: ChainReads,
    count: AtomicUsize,
}

impl ReplayRpc {
    pub fn new(label: impl Into<String>, reads: ChainReads) -> Self {
        Self {
            label: label.into(),
            reads,
            count: AtomicUsize::new(0),
        }
    }

    /// How many reads have been answered, so a test can hold the scan to a call budget.
    pub fn reads_answered(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn miss(&self, what: String) -> ! {
        panic!(
            "fixture {} has no recorded answer for {what}; re-record it with `hermes record`",
            self.label
        )
    }
}

impl ChainRpc for ReplayRpc {
    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        self.count.fetch_add(1, Ordering::Relaxed);
        let word = self
            .reads
            .storage
            .get(&address)
            .and_then(|slots| slots.get(&slot))
            .copied()
            .unwrap_or_else(|| self.miss(format!("eth_getStorageAt {address} {slot}")));
        Box::pin(async move { Ok(word) })
    }

    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        self.count.fetch_add(1, Ordering::Relaxed);
        let read = self
            .reads
            .code
            .get(&address)
            .copied()
            .unwrap_or_else(|| self.miss(format!("eth_getCode {address}")));
        // The scanner reads only emptiness and length, so a stand-in of the right size is a
        // faithful replay of everything it can observe.
        Box::pin(async move { Ok(Bytes::from(vec![0u8; read.size])) })
    }

    fn call(&self, to: Address, input: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        self.count.fetch_add(1, Ordering::Relaxed);
        let read = self
            .reads
            .call
            .get(&to)
            .and_then(|calls| calls.get(&input))
            .cloned()
            .unwrap_or_else(|| self.miss(format!("eth_call {to} {input}")));
        Box::pin(async move {
            match read {
                CallRead::Ok(out) => Ok(out),
                CallRead::Revert(message) => Err(anyhow::anyhow!(message)),
            }
        })
    }
}

/// One address's reads on every chain, at the blocks they were taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub target: Address,
    pub blocks: BTreeMap<Chain, u64>,
    /// Unix seconds.
    pub recorded_at: i64,
    pub reads: BTreeMap<Chain, ChainReads>,
}

impl Fixture {
    pub fn replay(&self, chain: Chain) -> ReplayRpc {
        ReplayRpc::new(
            format!("{} ({})", self.name, chain.as_str()),
            self.reads.get(&chain).cloned().unwrap_or_default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    fn sample() -> Fixture {
        let a = address!("4200000000000000000000000000000000000018");
        let mut base = ChainReads::default();
        base.storage
            .entry(a)
            .or_default()
            .insert(B256::ZERO, B256::repeat_byte(1));
        base.code.insert(
            a,
            CodeRead {
                size: 3,
                keccak256: b256!(
                    "0000000000000000000000000000000000000000000000000000000000000001"
                ),
            },
        );
        base.call.entry(a).or_default().insert(
            Bytes::from_static(&[0x8d, 0xa5, 0xcb, 0x5b]),
            CallRead::Ok(Bytes::from_static(&[7])),
        );
        base.call.entry(a).or_default().insert(
            Bytes::from_static(&[0xe7, 0x52, 0x35, 0xb8]),
            CallRead::Revert("execution reverted".into()),
        );
        Fixture {
            name: "sample".into(),
            target: a,
            blocks: [(Chain::Base, 1), (Chain::Ethereum, 2)]
                .into_iter()
                .collect(),
            recorded_at: 0,
            reads: [(Chain::Base, base)].into_iter().collect(),
        }
    }

    #[test]
    fn a_fixture_survives_a_round_trip_through_json() {
        let f = sample();
        let json = serde_json::to_string_pretty(&f).unwrap();
        assert_eq!(serde_json::from_str::<Fixture>(&json).unwrap(), f);
    }

    /// Keyed, not queued: asking in a different order than the recording must give the same
    /// answers, or concurrent scans could not be replayed at all.
    #[tokio::test]
    async fn replay_answers_by_key_regardless_of_order() {
        let f = sample();
        let rpc = f.replay(Chain::Base);
        let a = f.target;
        let reverted = rpc
            .call(a, Bytes::from_static(&[0xe7, 0x52, 0x35, 0xb8]))
            .await
            .unwrap_err();
        assert!(crate::resolve::is_revert(&reverted.to_string()));
        assert_eq!(
            rpc.call(a, Bytes::from_static(&[0x8d, 0xa5, 0xcb, 0x5b]))
                .await
                .unwrap(),
            Bytes::from_static(&[7])
        );
        assert_eq!(rpc.code(a).await.unwrap().len(), 3);
        assert_eq!(
            rpc.storage(a, B256::ZERO).await.unwrap(),
            B256::repeat_byte(1)
        );
        assert_eq!(rpc.reads_answered(), 4);
    }

    #[tokio::test]
    #[should_panic(expected = "re-record")]
    async fn a_read_the_recording_never_saw_fails_loudly() {
        let f = sample();
        let _ = f.replay(Chain::Base).code(Address::ZERO).await;
    }
}
