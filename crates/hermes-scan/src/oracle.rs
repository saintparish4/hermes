//! Asking the EVM whether a key count is true.
//!
//! A key count is my arithmetic over what contracts said about themselves. Here the count is
//! put to the chain instead: fork Base at the block the graph was read at, act as exactly the
//! keys the count names, and send the real upgrade through every layer. A Safe's owners approve
//! its transaction hash and a relayer executes it; a ProxyAdmin's owner calls it; a beacon's
//! owner upgrades it. Then the same again with each one of those keys missing, and once as a
//! stranger. If the named keys move the implementation, no set one short of them does, and a
//! stranger cannot, the count stands on something other than my reading of it.
//!
//! The fork does the deciding. My model only chooses who to try: which owners approve, which
//! upgrade function to call. A threshold that lied, an `owner()` that is not what
//! `_authorizeUpgrade` checks, an upgrade function anyone can call: each shows up as the EVM
//! disagreeing.
//!
//! A layer I do not know how to drive (an owned contract that is not a Safe, a timelock, the
//! Ethereum contract behind an alias) leaves the row undetermined. Undetermined is never
//! confirmed.

use crate::resolve::selector;
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use alloy::providers::{DynProvider, Provider};
use futures::future::BoxFuture;
use hermes_core::authority::{AuthorityKind, AuthorityProbe, MAX_DEPTH, authority_kind, pending};
use hermes_core::blast::falls;
use hermes_core::{Chain, IMPL_SLOT, Node, UpgradeEntry, successor};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Where the replacement implementation is planted on the fork. Derived from a name, so it
/// collides with nothing deployed.
pub fn planted_implementation() -> Address {
    Address::from_slice(&keccak256("hermes.oracle.implementation")[12..])
}

/// Sends the transactions no key is needed for: `execTransaction` on a Safe whose owners have
/// already approved. Anyone may relay it, so it proves nothing about who did.
pub fn relayer() -> Address {
    Address::from_slice(&keccak256("hermes.oracle.relayer")[12..])
}

/// An address that holds no role anywhere. If it can upgrade a proxy, nobody's keys guard it.
pub fn stranger() -> Address {
    Address::from_slice(&keccak256("hermes.oracle.stranger")[12..])
}

/// Runtime code for the planted implementation.
///
/// It answers every call with the ERC-1967 implementation slot, which is what `proxiableUUID()`
/// must return for a UUPS upgrade to accept it, and a delegated empty call harmlessly. Called
/// as `upgradeTo(address)` it writes that address to the slot, because OpenZeppelin's older
/// UUPS (4.1 to 4.4) proves a new implementation can upgrade again by delegating exactly that
/// call to it and checking the slot came back. Without it a real owner's upgrade would revert
/// and read as a contradiction that is only my stub.
pub fn planted_code() -> Bytes {
    let slot = IMPL_SLOT.as_slice();
    let upgrade_to = selector("upgradeTo(address)");
    let mut code = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c, 0x63];
    code.extend_from_slice(&upgrade_to);
    // EQ, PUSH1 <jumpdest>, JUMPI; the destination is patched below.
    code.extend_from_slice(&[0x14, 0x60, 0x00, 0x57]);
    let patch = code.len() - 2;
    code.push(0x7f);
    code.extend_from_slice(slot);
    code.extend_from_slice(&[0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
    let dest = code.len();
    code[patch] = u8::try_from(dest).expect("the stub is under 256 bytes");
    code.extend_from_slice(&[0x5b, 0x60, 0x04, 0x35, 0x7f]);
    code.extend_from_slice(slot);
    code.extend_from_slice(&[0x55, 0x00]);
    Bytes::from(code)
}

/// The EVM the oracle drives. Production is an anvil fork; tests script one.
pub trait Evm: Send + Sync {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<U256>>;
    fn revert(&self, id: U256) -> BoxFuture<'_, anyhow::Result<()>>;
    /// One transaction from `from`, impersonated. `true` when it did not revert.
    fn send(&self, from: Address, to: Address, data: Bytes) -> BoxFuture<'_, anyhow::Result<bool>>;
    fn call(&self, to: Address, data: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>>;
    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>>;
    fn set_code(&self, address: Address, code: Bytes) -> BoxFuture<'_, anyhow::Result<()>>;
    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>>;
}

/// An anvil fork, reached over JSON-RPC and started with `--auto-impersonate`.
pub struct AnvilEvm {
    provider: DynProvider,
}

impl AnvilEvm {
    pub fn new(provider: DynProvider) -> Self {
        Self { provider }
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        let raw: serde_json::Value = self.provider.raw_request(method.into(), params).await?;
        Ok(serde_json::from_value(raw)?)
    }
}

/// Enough gas for an upgrade behind a Safe behind a Safe, and well under a block.
const TRIAL_GAS: u64 = 15_000_000;

impl Evm for AnvilEvm {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<U256>> {
        Box::pin(async move { self.request("evm_snapshot", serde_json::json!([])).await })
    }

    fn revert(&self, id: U256) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            let reverted: bool = self.request("evm_revert", serde_json::json!([id])).await?;
            anyhow::ensure!(reverted, "evm_revert({id}) refused");
            Ok(())
        })
    }

    fn send(&self, from: Address, to: Address, data: Bytes) -> BoxFuture<'_, anyhow::Result<bool>> {
        Box::pin(async move {
            let rich = U256::from(10u64).pow(U256::from(21u64));
            let _: serde_json::Value = self
                .request("anvil_setBalance", serde_json::json!([from, rich]))
                .await?;
            let tx = serde_json::json!({
                "from": from,
                "to": to,
                "data": data,
                "gas": format!("{TRIAL_GAS:#x}"),
            });
            let hash: B256 = self
                .request("eth_sendTransaction", serde_json::json!([tx]))
                .await?;
            // Anvil hands back the hash before the receipt can be read. A missing receipt is
            // not a revert: reading it as one failed every trial, the honest ones included,
            // and made a stranger look refused. Only a status decides.
            for _ in 0..50 {
                let receipt: serde_json::Value = self
                    .request("eth_getTransactionReceipt", serde_json::json!([hash]))
                    .await?;
                match receipt["status"].as_str() {
                    Some("0x1") => return Ok(true),
                    Some("0x0") => return Ok(false),
                    _ => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                }
            }
            anyhow::bail!("no receipt for {hash}")
        })
    }

    fn call(&self, to: Address, data: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            self.request(
                "eth_call",
                serde_json::json!([{ "to": to, "data": data }, "latest"]),
            )
            .await
        })
    }

    fn storage(&self, address: Address, slot: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
        Box::pin(async move {
            self.request(
                "eth_getStorageAt",
                serde_json::json!([address, slot, "latest"]),
            )
            .await
        })
    }

    fn set_code(&self, address: Address, code: Bytes) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            let _: serde_json::Value = self
                .request("anvil_setCode", serde_json::json!([address, code]))
                .await?;
            Ok(())
        })
    }

    fn code(&self, address: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
        Box::pin(async move {
            self.request("eth_getCode", serde_json::json!([address, "latest"]))
                .await
        })
    }
}

/// An anvil process forking Base at one block, killed when dropped.
pub struct Anvil {
    child: std::process::Child,
    pub evm: AnvilEvm,
    pub block: u64,
}

impl Anvil {
    /// Start anvil and wait until it answers. Starting reads the fork block from the upstream
    /// endpoint, which on the public one took most of a minute, so the wait is generous.
    ///
    /// The upstream is paced by anvil itself (`--compute-units-per-second`), at about the one
    /// call a second the resolver uses, because a fork that outruns the public endpoint's rate
    /// limiter reads failures as empty storage, which would fail an honest trial.
    pub async fn start(
        binary: &str,
        fork_url: &str,
        block: u64,
        port: u16,
    ) -> anyhow::Result<Self> {
        let child = std::process::Command::new(binary)
            .args([
                "--fork-url",
                fork_url,
                "--fork-block-number",
                &block.to_string(),
                "--port",
                &port.to_string(),
                "--auto-impersonate",
                "--silent",
                "--compute-units-per-second",
                "20",
                "--retries",
                "10",
                "--fork-retry-backoff",
                "2000",
                "--timeout",
                "60000",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let url = format!("http://127.0.0.1:{port}");
        let provider = crate::probe::connect(&url).await?;
        let mut anvil = Self {
            child,
            evm: AnvilEvm::new(provider),
            block,
        };
        for _ in 0..180 {
            if anvil.evm.provider.get_chain_id().await.is_ok() {
                return Ok(anvil);
            }
            if let Some(status) = anvil.child.try_wait()? {
                anyhow::bail!("anvil exited before answering: {status}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        anyhow::bail!("anvil did not answer within three minutes")
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Why a trial could not be run, which is never the same as it failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Undriveable {
    /// The walk crosses to Ethereum. A fork of Base cannot act as an L1 contract.
    CrossChain,
    /// A node the trial needs was never read.
    NotRead,
    /// An owned contract has to make a call. Owning a contract does not make it call things,
    /// and how it can be made to is its own code.
    OwnedContract,
    /// A timelock would have to schedule and execute, through roles that are not listed.
    Timelock,
    /// An `AccessControl` contract would have to make a call.
    RoleContract,
    /// A smart account whose signers are not read.
    Account,
    /// The node answered nothing recognized, or is a sentinel.
    Unrecognized,
    /// Past the depth cap.
    Depth,
    /// There is no set of keys the count names to act as.
    NoWitness,
    /// The fork itself failed: the node behind it would not answer.
    Fork,
    /// A node under this row was last read at another block than the row, so no one fork
    /// shows the graph the count was made from.
    NotAtOneBlock,
    /// Walking the stored graph again does not give the published count, so there is no
    /// single claim to test.
    ReplayDiffers,
    /// None of the upgrade functions I know is in the code that would run it. A call to a
    /// function that is not there reverts for everyone, which says nothing about keys.
    UpgradeFunctionUnknown,
}

impl Undriveable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrossChain => "cross_chain",
            Self::NotRead => "not_read",
            Self::OwnedContract => "owned_contract",
            Self::Timelock => "timelock",
            Self::RoleContract => "role_contract",
            Self::Account => "account",
            Self::Unrecognized => "unrecognized",
            Self::Depth => "depth",
            Self::NoWitness => "no_witness",
            Self::Fork => "fork",
            Self::NotAtOneBlock => "not_at_one_block",
            Self::ReplayDiffers => "replay_differs",
            Self::UpgradeFunctionUnknown => "upgrade_function_unknown",
        }
    }
}

/// One argument of an ABI call.
enum Arg {
    Word([u8; 32]),
    Bytes(Vec<u8>),
}

fn word_of(a: Address) -> Arg {
    Arg::Word(a.into_word().0)
}

fn uint(n: U256) -> Arg {
    Arg::Word(n.to_be_bytes())
}

/// Head-and-tail ABI encoding for static words and dynamic `bytes`, which is every call the
/// oracle makes.
fn encode(signature: &str, args: &[Arg]) -> Bytes {
    let mut head = selector(signature).to_vec();
    let mut tail: Vec<u8> = Vec::new();
    let head_len = 32 * args.len();
    for arg in args {
        match arg {
            Arg::Word(w) => head.extend_from_slice(w),
            Arg::Bytes(b) => {
                head.extend_from_slice(&U256::from(head_len + tail.len()).to_be_bytes::<32>());
                tail.extend_from_slice(&U256::from(b.len()).to_be_bytes::<32>());
                tail.extend_from_slice(b);
                tail.resize(tail.len().div_ceil(32) * 32, 0);
            }
        }
    }
    head.extend(tail);
    Bytes::from(head)
}

/// The ten arguments every Safe transaction hash and execution share, for a plain call.
fn safe_tx(to: Address, data: &Bytes) -> Vec<Arg> {
    vec![
        word_of(to),
        uint(U256::ZERO),
        Arg::Bytes(data.to_vec()),
        uint(U256::ZERO),
        uint(U256::ZERO),
        uint(U256::ZERO),
        uint(U256::ZERO),
        word_of(Address::ZERO),
        word_of(Address::ZERO),
    ]
}

/// Pre-approved signatures, one per owner in ascending order, as `checkNSignatures` reads them:
/// `r` is the owner, `s` is zero, `v` is 1, meaning "this owner approved the hash on chain".
pub fn approved_signatures(mut owners: Vec<Address>) -> Vec<u8> {
    owners.sort();
    owners
        .iter()
        .flat_map(|o| {
            let mut sig = o.into_word().to_vec();
            sig.extend([0u8; 32]);
            sig.push(1);
            sig
        })
        .collect()
}

/// The upgrade calls to try at an entry, as (target, calldata), and the actor that has to send
/// them. Several, because ProxyAdmin and proxy versions disagree on the function name: OZ 4
/// has `upgrade` and `upgradeTo`, OZ 5 only the `AndCall` forms.
fn upgrade_calls(
    entry: UpgradeEntry,
    proxy: Address,
    entry_is_contract_with_owner: bool,
) -> Vec<(Address, Bytes)> {
    let new = planted_implementation();
    let on_proxy = vec![
        (
            proxy,
            encode(
                "upgradeToAndCall(address,bytes)",
                &[word_of(new), Arg::Bytes(Vec::new())],
            ),
        ),
        (proxy, encode("upgradeTo(address)", &[word_of(new)])),
    ];
    match entry {
        // The second and third are not OpenZeppelin's, and are here because beacons on Base use
        // them: the Sep 29 handover's 27 beacons upgrade through `updateImplementation`.
        UpgradeEntry::Beacon(beacon) => [
            "upgradeTo(address)",
            "updateImplementation(address)",
            "setImplementation(address)",
        ]
        .iter()
        .map(|sig| (beacon, encode(sig, &[word_of(new)])))
        .collect(),
        UpgradeEntry::UupsOwner(_) => on_proxy,
        UpgradeEntry::AdminSlot(admin) if entry_is_contract_with_owner => vec![
            (
                admin,
                encode("upgrade(address,address)", &[word_of(proxy), word_of(new)]),
            ),
            (
                admin,
                encode(
                    "upgradeAndCall(address,address,bytes)",
                    &[word_of(proxy), word_of(new), Arg::Bytes(Vec::new())],
                ),
            ),
        ],
        UpgradeEntry::AdminSlot(_) => on_proxy,
    }
}

/// Acts as one set of keys on the fork.
struct Driver<'a> {
    evm: &'a dyn Evm,
    probes: &'a HashMap<Node, AuthorityProbe>,
    keys: &'a HashSet<Node>,
}

type Acted = Result<bool, Undriveable>;

impl Driver<'_> {
    fn probe(&self, node: Node) -> Result<&AuthorityProbe, Undriveable> {
        if node.chain != Chain::Base {
            return Err(Undriveable::CrossChain);
        }
        self.probes.get(&node).ok_or(Undriveable::NotRead)
    }

    /// Whether the held keys take `node`, by my model: who is worth asking. The fork decides
    /// whether asking works.
    fn takes(&self, node: Node) -> bool {
        falls(node, self.keys, self.probes) == Some(true)
    }

    /// Make `actor` the sender of one call: `Ok(true)` when it went through.
    fn perform(&self, actor: Node, to: Address, data: Bytes, depth: usize) -> BoxFuture<'_, Acted> {
        Box::pin(async move {
            if depth > MAX_DEPTH {
                return Err(Undriveable::Depth);
            }
            let probe = self.probe(actor)?;
            match authority_kind(actor, probe) {
                AuthorityKind::Eoa if self.keys.contains(&actor) => {
                    self.evm.send(actor.address, to, data).await.map_err(fork)
                }
                AuthorityKind::Eoa => Ok(false),
                AuthorityKind::Safe => self.through_safe(actor, probe, to, data, depth).await,
                AuthorityKind::SmartAccount => {
                    self.through_account(actor, probe, to, data, depth).await
                }
                AuthorityKind::L1Alias => Err(Undriveable::CrossChain),
                AuthorityKind::Ownable => Err(Undriveable::OwnedContract),
                AuthorityKind::Timelock => Err(Undriveable::Timelock),
                AuthorityKind::RoleGated => Err(Undriveable::RoleContract),
                AuthorityKind::Sentinel | AuthorityKind::Unknown => Err(Undriveable::Unrecognized),
            }
        })
    }

    /// Owners the held keys take approve the transaction hash on chain, in ascending order, until
    /// the threshold I read is met or I run out; then a relayer executes with whatever approvals
    /// there are. Short of the threshold the Safe itself must refuse, which is the point.
    async fn through_safe(
        &self,
        safe: Node,
        probe: &AuthorityProbe,
        to: Address,
        data: Bytes,
        depth: usize,
    ) -> Acted {
        let mut owners = probe.owners.clone().ok_or(Undriveable::NotRead)?;
        owners.sort();
        let threshold = probe.threshold.ok_or(Undriveable::NotRead)? as usize;
        let nonce = self
            .evm
            .call(safe.address, Bytes::from(selector("nonce()").to_vec()))
            .await
            .map_err(fork)?;
        let mut hash_args = safe_tx(to, &data);
        hash_args.push(Arg::Word(
            nonce
                .get(..32)
                .and_then(|w| <[u8; 32]>::try_from(w).ok())
                .ok_or(Undriveable::Fork)?,
        ));
        let hash = self
            .evm
            .call(
                safe.address,
                encode(
                    "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
                    &hash_args,
                ),
            )
            .await
            .map_err(fork)?;
        let hash: [u8; 32] = hash
            .get(..32)
            .and_then(|w| w.try_into().ok())
            .ok_or(Undriveable::Fork)?;
        let mut approved = Vec::new();
        for owner in owners {
            if approved.len() == threshold {
                break;
            }
            let node = Node {
                chain: safe.chain,
                address: owner,
            };
            if !self.takes(node) {
                continue;
            }
            let approve = encode("approveHash(bytes32)", &[Arg::Word(hash)]);
            if self.perform(node, safe.address, approve, depth + 1).await? {
                approved.push(owner);
            }
        }
        let mut exec_args = safe_tx(to, &data);
        exec_args.push(Arg::Bytes(approved_signatures(approved)));
        let exec = encode(
            "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
            &exec_args,
        );
        self.evm
            .send(relayer(), safe.address, exec)
            .await
            .map_err(fork)
    }

    /// A MultiOwnable account executes for any one of its owners.
    async fn through_account(
        &self,
        account: Node,
        probe: &AuthorityProbe,
        to: Address,
        data: Bytes,
        depth: usize,
    ) -> Acted {
        let signers = probe.account_owners.as_ref().ok_or(Undriveable::Account)?;
        let execute = encode(
            "execute(address,uint256,bytes)",
            &[word_of(to), uint(U256::ZERO), Arg::Bytes(data.to_vec())],
        );
        for &address in &signers.addresses {
            let node = Node {
                chain: account.chain,
                address,
            };
            if self.takes(node) {
                return self
                    .perform(node, account.address, execute.clone(), depth + 1)
                    .await;
            }
        }
        Ok(false)
    }

    /// Who sends the upgrade call at `entry`, and whether the entry is a contract its owner
    /// calls (a ProxyAdmin, a beacon) rather than the authority itself.
    async fn actor_at(&self, entry: UpgradeEntry) -> Result<(Node, bool), Undriveable> {
        let start = entry.start();
        let probe = self.probe(start)?;
        match authority_kind(start, probe) {
            AuthorityKind::Ownable => {
                let owner = successor(start, probe).ok_or(Undriveable::NotRead)?;
                match pending(start, probe) {
                    Some(next) if !self.takes(owner) && self.takes(next) => {
                        let accept = Bytes::from(selector("acceptOwnership()").to_vec());
                        self.perform(next, start.address, accept, 1).await?;
                        Ok((next, true))
                    }
                    _ => Ok((owner, true)),
                }
            }
            AuthorityKind::Eoa | AuthorityKind::Safe | AuthorityKind::SmartAccount => {
                Ok((start, false))
            }
            AuthorityKind::RoleGated => self.role_holder(start, probe).await,
            AuthorityKind::L1Alias => Err(Undriveable::CrossChain),
            AuthorityKind::Timelock => Err(Undriveable::Timelock),
            AuthorityKind::Sentinel | AuthorityKind::Unknown => Err(Undriveable::Unrecognized),
        }
    }

    /// The member who sends the upgrade at an `AccessControl` entry. A holder of the admin role
    /// grants itself the upgrade role first, which is the one transaction that makes it as good
    /// as a holder; if the role is not really the one `_authorizeUpgrade` checks, the fork says
    /// so. With no member the held keys take, the first listed one is tried, and must fail.
    async fn role_holder(
        &self,
        start: Node,
        probe: &AuthorityProbe,
    ) -> Result<(Node, bool), Undriveable> {
        let gate = probe.roles.as_ref().ok_or(Undriveable::NotRead)?;
        let members = gate.members.as_ref().ok_or(Undriveable::RoleContract)?;
        let at = |address: Address| Node {
            chain: start.chain,
            address,
        };
        let chosen = members
            .iter()
            .map(|a| at(*a))
            .find(|n| self.takes(*n))
            .or_else(|| members.first().map(|a| at(*a)))
            .ok_or(Undriveable::RoleContract)?;
        let has = encode(
            "hasRole(bytes32,address)",
            &[Arg::Word(gate.role.0), word_of(chosen.address)],
        );
        let holds = self.evm.call(start.address, has).await.map_err(fork)?;
        if holds.last() != Some(&1) {
            let grant = encode(
                "grantRole(bytes32,address)",
                &[Arg::Word(gate.role.0), word_of(chosen.address)],
            );
            self.perform(chosen, start.address, grant, 1).await?;
        }
        Ok((chosen, true))
    }
}

/// Every selector a Solidity dispatcher compares the call against: a `PUSH4` followed by the
/// comparison or the duplication a dispatcher uses. Read only to rule a candidate in; a selector
/// missed here only costs a trial, never invents one.
pub fn dispatched_selectors(code: &[u8]) -> HashSet<[u8; 4]> {
    let mut out = HashSet::new();
    let mut i = 0;
    while i + 5 < code.len() {
        let op = code[i];
        if op == 0x63 && matches!(code[i + 5], 0x14 | 0x80 | 0x81 | 0x10 | 0x11) {
            out.insert([code[i + 1], code[i + 2], code[i + 3], code[i + 4]]);
        }
        // Skip push data so a constant's bytes are never read as opcodes.
        i += if (0x60..=0x7f).contains(&op) {
            usize::from(op - 0x5f) + 1
        } else {
            1
        };
    }
    out
}

/// The selectors dispatched at `to`, and at the implementation it delegates to when it is a
/// proxy.
async fn selectors_at(evm: &dyn Evm, to: Address) -> Result<HashSet<[u8; 4]>, Undriveable> {
    let mut selectors = dispatched_selectors(&evm.code(to).await.map_err(fork)?);
    let implementation = Address::from_word(evm.storage(to, IMPL_SLOT).await.map_err(fork)?);
    if !implementation.is_zero() {
        let code = evm.code(implementation).await.map_err(fork)?;
        selectors.extend(dispatched_selectors(&code));
    }
    Ok(selectors)
}

/// The candidates whose function is in the code that would run them: the target's own, and
/// for a proxy the implementation it delegates to.
async fn present(
    evm: &dyn Evm,
    calls: Vec<(Address, Bytes)>,
) -> Result<Vec<(Address, Bytes)>, Undriveable> {
    let mut known: HashMap<Address, HashSet<[u8; 4]>> = HashMap::new();
    let mut kept = Vec::new();
    for (to, data) in calls {
        if let std::collections::hash_map::Entry::Vacant(slot) = known.entry(to) {
            slot.insert(selectors_at(evm, to).await?);
        }
        let selector: [u8; 4] = data[..4]
            .try_into()
            .expect("calldata starts with a selector");
        if known[&to].contains(&selector) {
            kept.push((to, data));
        }
    }
    if kept.is_empty() {
        return Err(Undriveable::UpgradeFunctionUnknown);
    }
    Ok(kept)
}

fn fork(e: anyhow::Error) -> Undriveable {
    tracing::warn!(error = %e, "fork call failed");
    Undriveable::Fork
}

/// Whether the proxy now runs the planted implementation.
async fn moved(evm: &dyn Evm, entry: UpgradeEntry, proxy: Address) -> Acted {
    let new = planted_implementation().into_word();
    match entry {
        UpgradeEntry::Beacon(beacon) => {
            let answer = evm
                .call(beacon, Bytes::from(selector("implementation()").to_vec()))
                .await
                .map_err(fork)?;
            Ok(answer.as_ref() == new.as_slice())
        }
        UpgradeEntry::AdminSlot(_) | UpgradeEntry::UupsOwner(_) => {
            Ok(evm.storage(proxy, IMPL_SLOT).await.map_err(fork)? == new)
        }
    }
}

/// Try to upgrade `proxy` holding exactly `keys`, leaving the fork as it was found.
async fn trial(
    evm: &dyn Evm,
    probes: &HashMap<Node, AuthorityProbe>,
    entry: UpgradeEntry,
    proxy: Address,
    keys: &HashSet<Node>,
) -> Acted {
    let driver = Driver { evm, probes, keys };
    let outer = evm.snapshot().await.map_err(fork)?;
    let result = attempt(&driver, entry, proxy).await;
    evm.revert(outer).await.map_err(fork)?;
    result
}

async fn attempt(driver: &Driver<'_>, entry: UpgradeEntry, proxy: Address) -> Acted {
    let (actor, owned) = driver.actor_at(entry).await?;
    for (to, data) in present(driver.evm, upgrade_calls(entry, proxy, owned)).await? {
        let inner = driver.evm.snapshot().await.map_err(fork)?;
        let sent = driver.perform(actor, to, data, 1).await;
        let upgraded = match sent {
            Ok(true) => moved(driver.evm, entry, proxy).await?,
            Ok(false) => false,
            Err(e) => {
                driver.evm.revert(inner).await.map_err(fork)?;
                return Err(e);
            }
        };
        driver.evm.revert(inner).await.map_err(fork)?;
        if upgraded {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The same upgrade calls, sent by an address that holds nothing.
async fn stranger_trial(evm: &dyn Evm, entry: UpgradeEntry, proxy: Address) -> Acted {
    let mut calls = upgrade_calls(entry, proxy, true);
    calls.extend(upgrade_calls(entry, proxy, false));
    let calls = match present(evm, calls).await {
        Ok(calls) => calls,
        // Nothing I know how to call is there, so a stranger has nothing to try; the witness
        // trial has already said whether that leaves the row undetermined.
        Err(Undriveable::UpgradeFunctionUnknown) => return Ok(false),
        Err(e) => return Err(e),
    };
    for (to, data) in calls {
        let snap = evm.snapshot().await.map_err(fork)?;
        let sent = evm.send(stranger(), to, data).await.map_err(fork)?;
        let upgraded = sent && moved(evm, entry, proxy).await?;
        evm.revert(snap).await.map_err(fork)?;
        if upgraded {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What the fork said about one proxy's count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The named keys upgrade it, no set one short of them does, and a stranger cannot.
    Confirmed,
    /// The fork disagreed with the count somewhere.
    Contradicted,
    /// Some trial could not be run.
    Undetermined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub proxy: Address,
    pub entry: &'static str,
    pub keys_required: u32,
    /// The keys acted as: a set of the counted size that my model says takes the root.
    pub witness: Vec<Node>,
    /// Whether the witness upgraded the proxy.
    pub sufficient: Option<bool>,
    /// Whether every set of the witness minus one key failed to.
    pub minimal: Option<bool>,
    /// Whether a stranger failed to.
    pub stranger_refused: Option<bool>,
    pub outcome: Outcome,
    /// Why some trial could not be run.
    pub undriveable: Option<Undriveable>,
}

/// Put one proxy's key count to the fork.
pub async fn check(
    evm: &dyn Evm,
    probes: &HashMap<Node, AuthorityProbe>,
    entry: UpgradeEntry,
    proxy: Address,
    keys_required: u32,
) -> Verdict {
    let mut verdict = Verdict {
        proxy,
        entry: entry.as_str(),
        keys_required,
        witness: Vec::new(),
        sufficient: None,
        minimal: None,
        stranger_refused: None,
        outcome: Outcome::Undetermined,
        undriveable: None,
    };
    let Some(witness) = hermes_core::authority::witness(entry.start(), keys_required, probes)
    else {
        verdict.undriveable = Some(Undriveable::NoWitness);
        return verdict;
    };
    verdict.witness = witness.clone();
    if let Err(e) = evm.set_code(planted_implementation(), planted_code()).await {
        verdict.undriveable = Some(fork(e));
        return verdict;
    }
    let all: HashSet<Node> = witness.iter().copied().collect();
    match trial(evm, probes, entry, proxy, &all).await {
        Ok(upgraded) => verdict.sufficient = Some(upgraded),
        Err(why) => {
            verdict.undriveable = Some(why);
            return verdict;
        }
    }
    let mut minimal = true;
    for key in &witness {
        let mut short = all.clone();
        short.remove(key);
        match trial(evm, probes, entry, proxy, &short).await {
            Ok(true) => minimal = false,
            Ok(false) => {}
            Err(why) => {
                verdict.undriveable = Some(why);
                return verdict;
            }
        }
    }
    verdict.minimal = Some(minimal);
    match stranger_trial(evm, entry, proxy).await {
        Ok(upgraded) => verdict.stranger_refused = Some(!upgraded),
        Err(why) => {
            verdict.undriveable = Some(why);
            return verdict;
        }
    }
    verdict.outcome = if verdict.sufficient == Some(true)
        && verdict.minimal == Some(true)
        && verdict.stranger_refused == Some(true)
    {
        Outcome::Confirmed
    } else {
        Outcome::Contradicted
    };
    verdict
}

/// A verdict for a row that could not be put to the fork at all.
pub fn not_run(
    proxy: Address,
    entry: UpgradeEntry,
    keys_required: u32,
    why: Undriveable,
) -> Verdict {
    Verdict {
        proxy,
        entry: entry.as_str(),
        keys_required,
        witness: Vec::new(),
        sufficient: None,
        minimal: None,
        stranger_refused: None,
        outcome: Outcome::Undetermined,
        undriveable: Some(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use hermes_core::Code;
    use hermes_core::authority::{CANONICAL_SAFE_SINGLETONS, PendingOwner, SafeSetup};
    use std::sync::Mutex;

    #[test]
    fn selectors_the_oracle_sends_match_their_signatures() {
        assert_eq!(selector("approveHash(bytes32)"), [0xd4, 0xd9, 0xbd, 0xcd]);
        assert_eq!(selector("nonce()"), [0xaf, 0xfe, 0xd0, 0xe0]);
        assert_eq!(
            selector(
                "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)"
            ),
            [0x6a, 0x76, 0x12, 0x02]
        );
        assert_eq!(
            selector(
                "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)"
            ),
            [0xd8, 0xd1, 0x1f, 0x78]
        );
        assert_eq!(
            selector("upgradeToAndCall(address,bytes)"),
            [0x4f, 0x1e, 0xf2, 0x86]
        );
        assert_eq!(selector("upgradeTo(address)"), [0x36, 0x59, 0xcf, 0xe6]);
        assert_eq!(
            selector("upgrade(address,address)"),
            [0x99, 0xa8, 0x8e, 0xc4]
        );
    }

    /// Safe reads signatures in ascending owner order and rejects any other, so an unsorted
    /// list would make every honest trial fail and read as a contradiction.
    #[test]
    fn approved_signatures_are_sorted_and_read_as_pre_approved() {
        let (a, b) = (Address::with_last_byte(2), Address::with_last_byte(1));
        let sigs = approved_signatures(vec![a, b]);
        assert_eq!(sigs.len(), 130);
        assert_eq!(&sigs[12..32], b.as_slice());
        assert_eq!(&sigs[32..64], &[0u8; 32]);
        assert_eq!(sigs[64], 1);
        assert_eq!(&sigs[65 + 12..65 + 32], a.as_slice());
    }

    #[test]
    fn dynamic_bytes_are_encoded_after_the_head_and_padded() {
        let data = encode(
            "upgradeToAndCall(address,bytes)",
            &[
                word_of(Address::with_last_byte(9)),
                Arg::Bytes(vec![0xab; 33]),
            ],
        );
        let body = &data[4..];
        assert_eq!(U256::from_be_slice(&body[32..64]), U256::from(64));
        assert_eq!(U256::from_be_slice(&body[64..96]), U256::from(33));
        assert_eq!(body.len(), 32 * 2 + 32 + 64);
    }

    /// The stub is hand-assembled bytecode, so it is checked by reading it back: the jump lands
    /// on a JUMPDEST and the slot appears where both branches use it.
    #[test]
    fn the_planted_code_jumps_to_its_jumpdest_and_carries_the_slot_twice() {
        let code = planted_code();
        let dest = code[13] as usize;
        assert_eq!(code[dest], 0x5b);
        let slot = IMPL_SLOT.as_slice();
        assert_eq!(code.windows(32).filter(|w| *w == slot).count(), 2);
    }

    /// A toy EVM: an upgrade lands when sent by `admin`, or by the relayer once `need` of the
    /// Safe's owners have approved. Enough to drive the oracle's control flow, nothing more.
    struct Toy {
        safe: Address,
        admin: Address,
        owners: Vec<Address>,
        need: usize,
        anyone_upgrades: bool,
        /// The functions its code dispatches.
        functions: &'static [&'static str],
        state: Mutex<ToyState>,
    }

    #[derive(Clone, Default)]
    struct ToyState {
        approvals: HashSet<Address>,
        implementation: B256,
        snapshots: Vec<(HashSet<Address>, B256)>,
    }

    impl Toy {
        fn upgrade_lands(&self, from: Address, data: &Bytes) -> bool {
            let is_upgrade = data.starts_with(&selector("upgradeToAndCall(address,bytes)"));
            is_upgrade && (self.anyone_upgrades || from == self.admin)
        }
    }

    impl Evm for Toy {
        fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<U256>> {
            let mut s = self.state.lock().unwrap();
            let saved = (s.approvals.clone(), s.implementation);
            s.snapshots.push(saved);
            let id = U256::from(s.snapshots.len() - 1);
            Box::pin(async move { Ok(id) })
        }
        fn revert(&self, id: U256) -> BoxFuture<'_, anyhow::Result<()>> {
            let mut s = self.state.lock().unwrap();
            let i: usize = id.to();
            let (approvals, implementation) = s.snapshots[i].clone();
            s.snapshots.truncate(i);
            s.approvals = approvals;
            s.implementation = implementation;
            Box::pin(async { Ok(()) })
        }
        fn send(
            &self,
            from: Address,
            to: Address,
            data: Bytes,
        ) -> BoxFuture<'_, anyhow::Result<bool>> {
            let mut s = self.state.lock().unwrap();
            let ok = if to == self.safe && data.starts_with(&selector("approveHash(bytes32)")) {
                if self.owners.contains(&from) {
                    s.approvals.insert(from);
                }
                self.owners.contains(&from)
            } else if to == self.safe {
                let lands = s.approvals.len() >= self.need;
                if lands {
                    s.implementation = planted_implementation().into_word();
                }
                lands
            } else if self.upgrade_lands(from, &data) {
                s.implementation = planted_implementation().into_word();
                true
            } else {
                false
            };
            Box::pin(async move { Ok(ok) })
        }
        fn call(&self, _to: Address, _data: Bytes) -> BoxFuture<'_, anyhow::Result<Bytes>> {
            Box::pin(async { Ok(Bytes::from(vec![0u8; 32])) })
        }
        fn storage(&self, _a: Address, _slot: B256) -> BoxFuture<'_, anyhow::Result<B256>> {
            let word = self.state.lock().unwrap().implementation;
            Box::pin(async move { Ok(word) })
        }
        fn set_code(&self, _a: Address, _code: Bytes) -> BoxFuture<'_, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn code(&self, _a: Address) -> BoxFuture<'_, anyhow::Result<Bytes>> {
            let mut code = Vec::new();
            for sig in self.functions {
                code.push(0x63);
                code.extend(selector(sig));
                code.push(0x14);
            }
            Box::pin(async move { Ok(Bytes::from(code)) })
        }
    }

    const PROXY: Address = address!("00000000000000000000000000000000000000f1");
    const SAFE: Address = address!("00000000000000000000000000000000000000c1");
    const K1: Address = address!("00000000000000000000000000000000000000a1");
    const K2: Address = address!("00000000000000000000000000000000000000a2");
    const K3: Address = address!("00000000000000000000000000000000000000a3");

    fn key() -> AuthorityProbe {
        AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        }
    }

    fn safe_graph(threshold: u32) -> HashMap<Node, AuthorityProbe> {
        HashMap::from([
            (
                Node::base(PROXY),
                AuthorityProbe {
                    owner: Some(SAFE),
                    pending_owner: Some(PendingOwner::Nobody),
                    ..Default::default()
                },
            ),
            (
                Node::base(SAFE),
                AuthorityProbe {
                    owners: Some(vec![K1, K2, K3]),
                    threshold: Some(threshold),
                    safe_setup: Some(SafeSetup {
                        singleton: Some(CANONICAL_SAFE_SINGLETONS[0]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            (Node::base(K1), key()),
            (Node::base(K2), key()),
            (Node::base(K3), key()),
        ])
    }

    fn toy(need: usize, anyone_upgrades: bool) -> Toy {
        Toy {
            safe: SAFE,
            admin: SAFE,
            owners: vec![K1, K2, K3],
            need,
            anyone_upgrades,
            functions: &["upgradeToAndCall(address,bytes)"],
            state: Mutex::default(),
        }
    }

    #[tokio::test]
    async fn a_safe_whose_threshold_is_what_it_said_is_confirmed() {
        let evm = toy(2, false);
        let v = check(
            &evm,
            &safe_graph(2),
            UpgradeEntry::UupsOwner(PROXY),
            PROXY,
            2,
        )
        .await;
        assert_eq!(v.outcome, Outcome::Confirmed, "{v:?}");
        assert_eq!(v.witness.len(), 2);
        assert!(
            evm.state.lock().unwrap().snapshots.is_empty(),
            "every trial was rolled back"
        );
    }

    /// The EVM, not my reading, decides: a Safe that executes on one approval while claiming
    /// two is a count of two that one key beats.
    #[tokio::test]
    async fn a_threshold_that_lied_is_a_contradiction() {
        let evm = toy(1, false);
        let v = check(
            &evm,
            &safe_graph(2),
            UpgradeEntry::UupsOwner(PROXY),
            PROXY,
            2,
        )
        .await;
        assert_eq!(v.minimal, Some(false));
        assert_eq!(v.outcome, Outcome::Contradicted);
    }

    #[tokio::test]
    async fn an_upgrade_anyone_can_call_is_a_contradiction() {
        let evm = toy(2, true);
        let v = check(
            &evm,
            &safe_graph(2),
            UpgradeEntry::UupsOwner(PROXY),
            PROXY,
            2,
        )
        .await;
        assert_eq!(v.stranger_refused, Some(false));
        assert_eq!(v.outcome, Outcome::Contradicted);
    }

    #[tokio::test]
    async fn keys_that_cannot_upgrade_are_a_contradiction() {
        let evm = toy(3, false);
        let v = check(
            &evm,
            &safe_graph(2),
            UpgradeEntry::UupsOwner(PROXY),
            PROXY,
            2,
        )
        .await;
        assert_eq!(v.sufficient, Some(false));
        assert_eq!(v.outcome, Outcome::Contradicted);
    }

    /// An owned contract cannot be made to call anything by owning it, so a walk through one
    /// is never confirmed by pretending it can.
    #[tokio::test]
    async fn an_owned_contract_in_the_way_leaves_the_row_undetermined() {
        let mut g = safe_graph(1);
        let owned = address!("00000000000000000000000000000000000000d1");
        g.insert(
            Node::base(owned),
            AuthorityProbe {
                owner: Some(SAFE),
                pending_owner: Some(PendingOwner::Nobody),
                ..Default::default()
            },
        );
        g.get_mut(&Node::base(SAFE)).unwrap().owners = Some(vec![owned]);
        g.insert(
            Node::base(PROXY),
            AuthorityProbe {
                owner: Some(SAFE),
                pending_owner: Some(PendingOwner::Nobody),
                ..Default::default()
            },
        );
        let evm = toy(1, false);
        let v = check(&evm, &g, UpgradeEntry::UupsOwner(PROXY), PROXY, 1).await;
        assert_eq!(v.outcome, Outcome::Undetermined, "{v:?}");
    }

    /// A beacon with its own name for the upgrade function reverted for everyone and read as
    /// "the keys cannot upgrade" until the code was read first (Sep 29 beacon, 2026-10-17).
    #[tokio::test]
    async fn an_upgrade_function_the_code_does_not_have_leaves_the_row_undetermined() {
        let mut evm = toy(2, false);
        evm.functions = &["somethingElse(address)"];
        let v = check(
            &evm,
            &safe_graph(2),
            UpgradeEntry::UupsOwner(PROXY),
            PROXY,
            2,
        )
        .await;
        assert_eq!(
            v.undriveable,
            Some(Undriveable::UpgradeFunctionUnknown),
            "{v:?}"
        );
        assert_eq!(v.outcome, Outcome::Undetermined);
    }

    #[test]
    fn a_dispatcher_selector_is_found_and_push_data_is_not_read_as_code() {
        let sel = selector("upgradeTo(address)");
        let mut code = vec![0x7f];
        code.extend([0x63, sel[0], sel[1], sel[2], sel[3], 0x14]);
        code.extend([0u8; 26]);
        assert!(
            dispatched_selectors(&code).is_empty(),
            "inside a PUSH32 constant"
        );
        let mut code = vec![0x60, 0x00, 0x35, 0x63];
        code.extend(sel);
        code.push(0x14);
        code.push(0x00);
        assert!(dispatched_selectors(&code).contains(&sel));
    }

    #[tokio::test]
    async fn an_alias_in_the_way_leaves_the_row_undetermined() {
        let l1 = address!("00000000000000000000000000000000000000e1");
        let g = HashMap::from([
            (
                Node::base(PROXY),
                AuthorityProbe {
                    owner: Some(SAFE),
                    pending_owner: Some(PendingOwner::Nobody),
                    ..Default::default()
                },
            ),
            (
                Node::base(SAFE),
                AuthorityProbe {
                    code: Code::L1Alias(l1),
                    ..Default::default()
                },
            ),
            (Node::ethereum(l1), key()),
        ]);
        let v = check(&toy(1, false), &g, UpgradeEntry::UupsOwner(PROXY), PROXY, 1).await;
        assert_eq!(v.undriveable, Some(Undriveable::CrossChain), "{v:?}");
        assert_eq!(v.outcome, Outcome::Undetermined);
    }
}
