//! Reading who made a past upgrade, from its receipt and its input.
//!
//! The judging is pure (`hermes_core::backtest`); this is the reading. A receipt names the
//! contracts that executed (a Safe's `ExecutionSuccess`, a module's `ExecutionFromModuleSuccess`,
//! an account's `UserOperationEvent`, a timelock's `CallExecuted`), and a Safe called directly
//! carries its signatures in the input, which recover to the owners who signed. None of that
//! needs a trace, and all of it is served by the public endpoint.
//!
//! An event in the constructor is a deployment, not an upgrade: the proxy had no code the block
//! before. Those are skipped, since the deployer setting a first implementation says nothing
//! about who controls the second.

use crate::resolve::{selector, word_to_address_strict};
use alloy::primitives::{Address, B256, Signature, U256, keccak256};
use alloy::providers::{DynProvider, Provider};
use hermes_core::backtest::{Observed, SafeExecution};
use serde_json::Value;
use std::time::Duration;

fn topic(signature: &str) -> B256 {
    keccak256(signature.as_bytes())
}

/// A JSON-RPC read with the same patience as resolution: paced, and backed off from two
/// seconds up, because the public endpoint's limiter outlasts shorter waits.
pub struct Reader {
    provider: DynProvider,
    interval: Duration,
}

impl Reader {
    pub fn new(provider: DynProvider, interval: Duration) -> Self {
        Self { provider, interval }
    }

    pub async fn request(&self, method: &'static str, params: Value) -> anyhow::Result<Value> {
        let mut last = None;
        for attempt in 0..6u32 {
            tokio::time::sleep(self.interval).await;
            match self
                .provider
                .raw_request::<_, Value>(method.into(), params.clone())
                .await
            {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(Duration::from_millis(2000 << attempt)).await;
        }
        Err(last.map_or_else(|| anyhow::anyhow!("no attempt"), Into::into))
    }

    /// Hashes of the transactions that emitted `Upgraded` from `address` in `block`.
    pub async fn upgrade_transactions(
        &self,
        address: Address,
        block: u64,
    ) -> anyhow::Result<Vec<B256>> {
        let filter = serde_json::json!({
            "address": address,
            "fromBlock": format!("{block:#x}"),
            "toBlock": format!("{block:#x}"),
            "topics": [crate::discover::UPGRADED],
        });
        let logs = self
            .request("eth_getLogs", serde_json::json!([filter]))
            .await?;
        let mut hashes: Vec<B256> = logs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["transactionHash"].as_str()?.parse().ok())
            .collect();
        hashes.dedup();
        Ok(hashes)
    }

    /// Whether `address` had code at `block`. `None` when the node would not say.
    pub async fn had_code(&self, address: Address, block: u64) -> Option<bool> {
        let code = self
            .request(
                "eth_getCode",
                serde_json::json!([address, format!("{block:#x}")]),
            )
            .await
            .ok()?;
        Some(code.as_str()? != "0x")
    }

    /// The transaction and its receipt.
    pub async fn transaction(&self, hash: B256) -> anyhow::Result<(Value, Value)> {
        let tx = self
            .request("eth_getTransactionByHash", serde_json::json!([hash]))
            .await?;
        let receipt = self
            .request("eth_getTransactionReceipt", serde_json::json!([hash]))
            .await?;
        anyhow::ensure!(!tx.is_null() && !receipt.is_null(), "{hash} not found");
        Ok((tx, receipt))
    }
}

fn hex_bytes(v: &Value) -> Option<Vec<u8>> {
    alloy::hex::decode(v.as_str()?).ok()
}

fn address_of(v: &Value) -> Option<Address> {
    v.as_str()?.parse().ok()
}

/// Every actor a receipt names, with each Safe's signers where the transaction called the Safe
/// directly. `threshold_of` says how many signatures to read from a Safe, which is the
/// threshold the block before: Safe reads exactly that many and ignores the rest.
pub fn observed(
    tx: &Value,
    receipt: &Value,
    threshold_of: &dyn Fn(Address) -> Option<u32>,
) -> Option<Observed> {
    let mut seen = Observed {
        sender: address_of(&tx["from"])?,
        ..Default::default()
    };
    let to = address_of(&tx["to"]);
    let input = hex_bytes(&tx["input"]).unwrap_or_default();
    let success = topic("ExecutionSuccess(bytes32,uint256)");
    let module = topic("ExecutionFromModuleSuccess(address)");
    let user_op = topic("UserOperationEvent(bytes32,address,address,uint256,bool,uint256,uint256)");
    let executed = topic("CallExecuted(bytes32,uint256,address,uint256,bytes)");
    for log in receipt["logs"].as_array()? {
        let emitter = address_of(&log["address"])?;
        let topics: Vec<B256> = log["topics"]
            .as_array()?
            .iter()
            .filter_map(|t| t.as_str()?.parse().ok())
            .collect();
        let data = hex_bytes(&log["data"]).unwrap_or_default();
        match topics.first() {
            Some(t) if *t == success => {
                // `txHash` is unindexed in 1.3 and 1.4; read it from either place.
                let hash = topics
                    .get(1)
                    .copied()
                    .or_else(|| data.get(..32).map(B256::from_slice));
                let signers = (to == Some(emitter))
                    .then(|| safe_signers(&input, hash?, threshold_of(emitter)?))
                    .flatten();
                seen.safe_executions.push(SafeExecution {
                    safe: emitter,
                    signers,
                });
            }
            Some(t) if *t == module => {
                let m = topics
                    .get(1)
                    .and_then(|w| word_to_address_strict(w.as_slice()));
                if let Some(m) = m {
                    seen.module_executions.push((emitter, m));
                }
            }
            Some(t) if *t == user_op => {
                if let Some(sender) = topics
                    .get(2)
                    .and_then(|w| word_to_address_strict(w.as_slice()))
                {
                    seen.account_operations.push(sender);
                }
            }
            Some(t) if *t == executed => seen.timelock_executions.push(emitter),
            _ => {}
        }
    }
    Some(seen)
}

/// The owners whose signatures an `execTransaction` input carries, or `None` when any of the
/// first `threshold` cannot be attributed.
///
/// Four forms, by `v`: 0 is a contract signature and 1 an on-chain approval, both naming the
/// owner in `r`; above 30 is `eth_sign` over the hash, with `v` shifted by 4; anything else is
/// ECDSA over the hash itself.
pub fn safe_signers(input: &[u8], hash: B256, threshold: u32) -> Option<Vec<Address>> {
    if input.get(..4)?
        != selector(
            "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
        )
    {
        return None;
    }
    let args = &input[4..];
    let word = |i: usize| -> Option<usize> {
        U256::from_be_slice(args.get(i..i.checked_add(32)?)?)
            .try_into()
            .ok()
    };
    let offset = word(9 * 32)?;
    let len = word(offset)?;
    let start = offset.checked_add(32)?;
    let sigs = args.get(start..start.checked_add(len)?)?;
    let mut out = Vec::new();
    for i in 0..threshold as usize {
        let sig = sigs.get(i * 65..i * 65 + 65)?;
        let (r, s, v) = (&sig[..32], &sig[32..64], sig[64]);
        let owner = match v {
            0 | 1 => word_to_address_strict(r)?,
            31 | 32 => recover(&alloy::primitives::eip191_hash_message(hash), r, s, v - 4)?,
            27 | 28 => recover(&hash, r, s, v)?,
            _ => return None,
        };
        out.push(owner);
    }
    Some(out)
}

fn recover(prehash: &B256, r: &[u8], s: &[u8], v: u8) -> Option<Address> {
    let sig = Signature::new(U256::from_be_slice(r), U256::from_be_slice(s), v == 28);
    sig.recover_address_from_prehash(prehash).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn exec_input(sigs: &[u8]) -> Vec<u8> {
        let mut input = selector(
            "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
        )
        .to_vec();
        let mut head = vec![[0u8; 32]; 10];
        head[2] = U256::from(320).to_be_bytes();
        head[9] = U256::from(352).to_be_bytes();
        for w in &head {
            input.extend(w);
        }
        input.extend([0u8; 32]); // data: empty bytes
        input.extend(U256::from(sigs.len()).to_be_bytes::<32>());
        input.extend(sigs);
        input.resize(4 + (input.len() - 4).div_ceil(32) * 32, 0);
        input
    }

    #[test]
    fn approved_and_contract_signatures_name_the_owner_in_r() {
        let (a, b) = (
            address!("00000000000000000000000000000000000000a1"),
            address!("00000000000000000000000000000000000000a2"),
        );
        let mut sigs = Vec::new();
        for (owner, v) in [(a, 1u8), (b, 0u8)] {
            sigs.extend(owner.into_word());
            sigs.extend([0u8; 32]);
            sigs.push(v);
        }
        let input = exec_input(&sigs);
        assert_eq!(safe_signers(&input, B256::ZERO, 2), Some(vec![a, b]));
        assert_eq!(
            safe_signers(&input, B256::ZERO, 3),
            None,
            "a third is not there"
        );
    }

    /// Private key 1 signed `keccak256("hermes")`, raw and as `eth_sign`, with `cast wallet
    /// sign` (offline, deterministic). Both forms must recover to its address, the second only
    /// once Safe's `v + 4` is undone.
    #[test]
    fn ecdsa_and_eth_sign_signatures_recover_to_their_signer() {
        let signer = address!("7E5F4552091A69125d5DfCb7b8C2659029395Bdf");
        let hash = keccak256("hermes");
        let raw = alloy::hex::decode("8a8bef33c5698c9b13ceb92a9f1bfe1b270d2b930cbd937901abe7ca87a931b922b24eeec68dd99d92bcbb6c8b7fc0c507c6a9ca7e4367951aec71d592447a521b").unwrap();
        let mut eth_sign = alloy::hex::decode("012e4747bdf1c5246983684fc9cb959b68c5e88f3103a58441c4c4eec000603875b9f7c20c459e2c62258ac02293ddfb11d8d91037f363be7fe2444e7a810bc91b").unwrap();
        eth_sign[64] += 4;
        let mut sigs = raw.clone();
        sigs.extend(&eth_sign);
        assert_eq!(
            safe_signers(&exec_input(&sigs), hash, 2),
            Some(vec![signer, signer])
        );
        let mut wrong = raw;
        wrong[64] = 0x1c;
        let other = safe_signers(&exec_input(&wrong), hash, 1).unwrap();
        assert_ne!(other, vec![signer], "the other parity is someone else");
    }

    #[test]
    fn an_input_that_is_not_exec_transaction_has_no_signers() {
        assert_eq!(safe_signers(&[1, 2, 3, 4], B256::ZERO, 1), None);
    }

    #[test]
    fn a_signature_list_shorter_than_its_length_word_is_refused() {
        let mut input = exec_input(&[0u8; 65]);
        let at = 4 + 352;
        input[at..at + 32].copy_from_slice(&U256::from(10_000).to_be_bytes::<32>());
        assert_eq!(safe_signers(&input, B256::ZERO, 1), None);
    }
}
