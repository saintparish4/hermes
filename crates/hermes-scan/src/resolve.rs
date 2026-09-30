//! Collecting the authority probes that resolution walks.
//!
//! Probing is duck typing over `eth_call`: I call a selector and see whether anything sane
//! comes back. That makes decoding the boundary where a hostile contract gets to influence
//! me, so every value is bounded here rather than trusted downstream.
//!
//! Collection is breadth-first by design. Walking depth-first would interleave network round
//! trips with graph decisions and make the whole thing untestable; gathering a level at a
//! time keeps all the I/O here and leaves the graph logic pure.
//!
//! Two chains are read. Base is where the proxies live, and Ethereum is where a codeless Base
//! authority may really live: an L1 contract acts on L2 through an aliased address that has no
//! code of its own, so "no code on Base" is a question for L1 before it is an answer.

use crate::rpc::{ChainRpc, delegation};
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use futures::stream::{self, StreamExt};
use hermes_core::authority::{AccountOwners, RoleGate};
use hermes_core::{
    AuthorityKind, AuthorityProbe, Chain, Code, IMPL_SLOT, MAX_DEPTH, Node, authority_kind, edges,
    undo_l1_to_l2_alias,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// The most owners I will read off one contract.
///
/// A length prefix is just a number a stranger returned. Without a bound, `getOwners()`
/// claiming a billion entries is an allocation big enough to end the scan.
pub const MAX_OWNERS: usize = 256;

pub(crate) fn selector(signature: &str) -> [u8; 4] {
    let hash: B256 = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// Decode a 32-byte word as an address, rejecting anything with dirty upper bytes.
///
/// `Address::from_word` would silently take the low 20 bytes. A word with non-zero upper
/// bytes was not written by any contract I recognize, and truncating it fabricates an
/// authority out of whatever noise happened to be there.
pub(crate) fn word_to_address_strict(word: &[u8]) -> Option<Address> {
    if word.len() != 32 || word[..12].iter().any(|b| *b != 0) {
        return None;
    }
    let addr = Address::from_slice(&word[12..]);
    (!addr.is_zero()).then_some(addr)
}

pub(crate) fn word_to_u256(word: &[u8]) -> Option<U256> {
    (word.len() == 32).then(|| U256::from_be_slice(word))
}

/// `selector` followed by 32-byte arguments.
pub(crate) fn calldata(signature: &str, args: &[[u8; 32]]) -> Vec<u8> {
    let mut out = selector(signature).to_vec();
    for a in args {
        out.extend_from_slice(a);
    }
    out
}

/// A count a contract returned, as a length I am willing to allocate for.
fn bounded_count(data: &[u8]) -> Option<usize> {
    let n: usize = word_to_u256(data)?.try_into().ok()?;
    (n <= MAX_OWNERS).then_some(n)
}

/// Decode `bytes` return data, with the offset and length checked against what is present.
pub(crate) fn decode_bytes(data: &[u8]) -> Option<&[u8]> {
    let offset: usize = word_to_u256(data.get(..32)?)?.try_into().ok()?;
    let len_at = offset.checked_add(32)?;
    let len: usize = word_to_u256(data.get(offset..len_at)?)?.try_into().ok()?;
    data.get(len_at..len_at.checked_add(len)?)
}

/// `type(IAccessControl).interfaceId`, which a test derives from the five selectors it XORs.
const IACCESS_CONTROL: [u8; 4] = [0x79, 0x65, 0xdb, 0x0b];

/// Decode `address[]` return data.
///
/// The offset and the length both come from the callee, so both are checked against the
/// bytes actually present before anything is allocated.
pub(crate) fn decode_address_array(data: &[u8]) -> Option<Vec<Address>> {
    let offset = word_to_u256(data.get(..32)?)?;
    let offset: usize = offset.try_into().ok()?;
    let len_at = offset.checked_add(32)?;
    let len = word_to_u256(data.get(offset..len_at)?)?;
    let len: usize = len.try_into().ok()?;
    if len > MAX_OWNERS {
        return None;
    }
    let end = len_at.checked_add(len.checked_mul(32)?)?;
    let body = data.get(len_at..end)?;
    body.as_chunks::<32>()
        .0
        .iter()
        .map(|word| word_to_address_strict(word))
        .collect()
}

/// How many times to re-ask before accepting that I will not find out.
const RETRIES: u32 = 5;

/// How long to wait before re-reading an empty code answer, matching the slot scan.
const CONFIRM_DELAY: Duration = Duration::from_millis(150);

/// What one `eth_call` established.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallOutcome {
    /// The contract returned data.
    Answered(Bytes),
    /// The contract does not have this function. A real, usable "no".
    NoAnswer,
    /// The node would not tell me. Not a "no", and must never be read as one.
    Undetermined,
}

impl CallOutcome {
    /// `None` when nothing was established, so `?` propagates "I do not know" rather than
    /// letting it decay into "the interface is absent".
    fn settled(self) -> Option<Option<Bytes>> {
        match self {
            Self::Answered(b) => Some(Some(b)),
            Self::NoAnswer => Some(None),
            Self::Undetermined => None,
        }
    }
}

/// The public endpoint answers `over rate limit` and then stays angry for a while, so the
/// waits have to get genuinely long rather than politely long.
async fn backoff(attempt: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(400u64 << attempt.min(6))).await;
}

fn note_retry(node: Node, what: &str, attempt: u32, error: &dyn std::fmt::Display) {
    tracing::debug!(?node, what, attempt, %error, "read failed, backing off");
}

fn note_undetermined(node: Node, what: &str) {
    tracing::warn!(?node, what, "undetermined after retries");
}

/// Distinguish a contract saying "no such function" from the node saying nothing useful.
pub(crate) fn is_revert(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("execution reverted") || e.contains("invalid opcode") || e.contains("out of gas")
}

/// What a Base address with no code is, given what L1 said about its unaliased address.
///
/// `None` when L1 would not answer. That is the honest outcome and the whole reason this is a
/// function: an unread L1 must never fall through to "a key", because "a key" is the most
/// alarming verdict available and it would be stated about an address I learned nothing about.
fn codeless_on_base(l1: Address, l1_code_is_empty: Option<bool>) -> Option<Code> {
    Some(if l1_code_is_empty? {
        Code::Absent
    } else {
        Code::L1Alias(l1)
    })
}

/// One chain's connection, and how far apart requests to it have to be.
///
/// Measured on `mainnet.base.org`, 2026-09-19: about ten back-to-back `eth_call`s succeed, then
/// the endpoint answers HTTP 429 until it has seen roughly seven seconds of quiet. Retries fired
/// into that window extend it, so a resolver calling as fast as it can loses whole nodes to
/// `Undetermined`, and which nodes it loses changes from run to run. One call every 800ms went
/// 30 for 30. Spacing requests is what makes coverage stable; retrying harder does not.
#[derive(Clone)]
pub struct Endpoint {
    rpc: Arc<dyn ChainRpc>,
    interval: Duration,
    next_slot: Arc<Mutex<Instant>>,
}

impl Endpoint {
    pub fn new(rpc: Arc<dyn ChainRpc>, interval: Duration) -> Self {
        Self {
            rpc,
            interval,
            next_slot: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// The reader, once this endpoint's turn has come round.
    async fn paced(&self) -> &dyn ChainRpc {
        let mut next = self.next_slot.lock().await;
        tokio::time::sleep_until(*next).await;
        *next = Instant::now() + self.interval;
        self.rpc.as_ref()
    }
}

#[derive(Clone)]
pub struct AuthorityScanner {
    base: Endpoint,
    ethereum: Endpoint,
    concurrency: usize,
    /// Every settled answer this scanner has had, so a run that resolves in batches walks a
    /// shared ProxyAdmin or Safe once rather than once per batch. Only settled answers are kept:
    /// a node that would not answer gets asked again next time.
    probes: Arc<std::sync::Mutex<HashMap<Node, AuthorityProbe>>>,
    uups: Arc<std::sync::Mutex<HashMap<Address, bool>>>,
}

impl AuthorityScanner {
    pub fn new(base: Endpoint, ethereum: Endpoint, concurrency: usize) -> Self {
        Self {
            base,
            ethereum,
            concurrency,
            probes: Arc::default(),
            uups: Arc::default(),
        }
    }

    fn remembered(&self, node: Node) -> Option<AuthorityProbe> {
        self.probes
            .lock()
            .expect("probe cache poisoned")
            .get(&node)
            .cloned()
    }

    /// `probe`, answered from memory when this scanner has already settled `node`.
    async fn probe_once(&self, node: Node) -> Option<AuthorityProbe> {
        if let Some(probe) = self.remembered(node) {
            return Some(probe);
        }
        let probe = self.probe(node).await?;
        self.probes
            .lock()
            .expect("probe cache poisoned")
            .insert(node, probe.clone());
        Some(probe)
    }

    async fn rpc(&self, chain: Chain) -> &dyn ChainRpc {
        match chain {
            Chain::Base => self.base.paced().await,
            Chain::Ethereum => self.ethereum.paced().await,
        }
    }

    /// One `eth_call`.
    ///
    /// The three outcomes must stay distinct. Collapsing `Undetermined` into "no answer" is
    /// how a rate-limited `getOwners()` turns a Safe into an `Ownable` — the threshold
    /// vanishes, the owner set vanishes, and the key count silently drops to one.
    async fn call(&self, node: Node, input: &[u8]) -> CallOutcome {
        let input = Bytes::from(input.to_vec());
        for attempt in 0..RETRIES {
            match self
                .rpc(node.chain)
                .await
                .call(node.address, input.clone())
                .await
            {
                Ok(out) if !out.is_empty() => return CallOutcome::Answered(out),
                // An empty return is a contract answering without saying anything, which is
                // not the interface I asked about. Retrying that learns nothing.
                Ok(_) => return CallOutcome::NoAnswer,
                // A revert is the contract telling me the function is not there. Anything
                // else is the node failing, and confusing the two is what fabricates a
                // governance structure out of an outage.
                Err(e) if is_revert(&e.to_string()) => return CallOutcome::NoAnswer,
                Err(e) => {
                    note_retry(node, "eth_call", attempt, &e);
                    backoff(attempt).await;
                }
            }
        }
        note_undetermined(node, "eth_call");
        CallOutcome::Undetermined
    }

    async fn read_code(&self, node: Node) -> Option<Bytes> {
        for attempt in 0..RETRIES {
            match self.rpc(node.chain).await.code(node.address).await {
                Ok(code) => return Some(code),
                Err(e) => {
                    note_retry(node, "eth_getCode", attempt, &e);
                    backoff(attempt).await;
                }
            }
        }
        note_undetermined(node, "eth_getCode");
        None
    }

    /// An address's code, with an empty answer read twice before I believe it.
    ///
    /// The public Base endpoint returns `0x` for contracts that have code often enough under
    /// load that one sighting of nothing proves nothing, which is the rule the slot scan
    /// follows too. `None` when the node would not tell me.
    async fn confirmed_code(&self, node: Node) -> Option<Bytes> {
        let first = self.read_code(node).await?;
        if !first.is_empty() {
            return Some(first);
        }
        tokio::time::sleep(CONFIRM_DELAY).await;
        self.read_code(node).await
    }

    /// Whether an address has no code, or `None` when the node would not tell me.
    ///
    /// This distinction is the whole ballgame. A failed code read defaulting to "empty" would
    /// classify the address as an EOA — a *terminal* answer costing exactly one key. A rate
    /// limit would silently become the most alarming possible verdict, stated with High
    /// confidence, on an address I learned nothing about.
    async fn code_is_empty(&self, node: Node) -> Option<bool> {
        Some(self.confirmed_code(node).await?.is_empty())
    }

    /// What an address is as far as code goes: code, an EIP-7702 delegation, or none on its
    /// own chain and, for a codeless Base address, on Ethereum behind its alias.
    async fn code(&self, node: Node) -> Option<Code> {
        let code = self.confirmed_code(node).await?;
        if let Some(delegate) = delegation(&code) {
            return Some(Code::Delegated(delegate));
        }
        if !code.is_empty() {
            return Some(Code::Present);
        }
        match node.chain {
            Chain::Ethereum => Some(Code::Absent),
            Chain::Base => {
                let l1 = undo_l1_to_l2_alias(node.address);
                codeless_on_base(l1, self.code_is_empty(Node::ethereum(l1)).await)
            }
        }
    }

    /// Ask one address every question I know how to ask.
    ///
    /// `None` means I could not establish anything, and the address is deliberately left out
    /// of the probe map so resolution treats it as unknown rather than as an answer.
    ///
    /// The calls run one after another rather than concurrently. Firing all five at once
    /// multiplies the caller's concurrency limit by five, and the public endpoint starts
    /// answering `over rate limit` — which arrives here as "this interface is absent" and
    /// turns a Safe into an unresolved shrug.
    pub async fn probe(&self, node: Node) -> Option<AuthorityProbe> {
        let code = self.code(node).await?;
        if code != Code::Present {
            return Some(AuthorityProbe {
                code,
                ..Default::default()
            });
        }
        let owners = self.call(node, &selector("getOwners()")).await.settled()?;
        let threshold = self
            .call(node, &selector("getThreshold()"))
            .await
            .settled()?;
        let owner = self.call(node, &selector("owner()")).await.settled()?;
        let min_delay = self
            .call(node, &selector("getMinDelay()"))
            .await
            .settled()?;
        let mut probe = AuthorityProbe {
            code,
            owners: owners.and_then(|b| decode_address_array(&b)),
            // Saturating rather than truncating: a `u256 -> u32` cast that wraps turns
            // "needs four billion keys" into "needs three".
            threshold: threshold
                .and_then(|b| word_to_u256(&b))
                .map(|v| v.saturating_to::<u32>()),
            owner: owner.and_then(|b| word_to_address_strict(&b)),
            min_delay: min_delay
                .and_then(|b| word_to_u256(&b))
                .map(|v| v.saturating_to::<u64>()),
            ..Default::default()
        };
        if authority_kind(node, &probe) == AuthorityKind::Unknown {
            self.probe_further(node, &mut probe).await?;
        }
        Some(probe)
    }

    /// The kinds recognized after the first four, asked only of a contract that answered none
    /// of those. Nothing already resolved pays for them, and they cannot shadow an answer the
    /// walk relied on before they existed.
    async fn probe_further(&self, node: Node, probe: &mut AuthorityProbe) -> Option<()> {
        probe.entry_point = self
            .call(node, &selector("entryPoint()"))
            .await
            .settled()?
            .and_then(|b| word_to_address_strict(&b));
        if probe.entry_point.is_some() {
            probe.account_owners = self.account_owners(node).await?;
            return Some(());
        }
        probe.roles = self.role_gate(node).await?;
        Some(())
    }

    /// The signers of a MultiOwnable account (Coinbase Smart Wallet): `ownerAtIndex(i)` for
    /// every index below `nextOwnerIndex()`, where 32 bytes is an address, 64 bytes a passkey,
    /// and nothing a removed owner. `Some(None)` for an account that is not MultiOwnable, or one
    /// whose list I cannot read in full; a partial signer list is not a signer list.
    async fn account_owners(&self, node: Node) -> Option<Option<AccountOwners>> {
        let Some(next) = self
            .call(node, &selector("nextOwnerIndex()"))
            .await
            .settled()?
        else {
            return Some(None);
        };
        let Some(n) = bounded_count(&next) else {
            return Some(None);
        };
        let mut owners = AccountOwners::default();
        for i in 0..n {
            let input = calldata("ownerAtIndex(uint256)", &[U256::from(i).to_be_bytes()]);
            let Some(raw) = self.call(node, &input).await.settled()? else {
                return Some(None);
            };
            match decode_bytes(&raw) {
                Some([]) => {}
                Some(word) if word.len() == 32 => match word_to_address_strict(word) {
                    Some(a) => owners.addresses.push(a),
                    None => return Some(None),
                },
                Some(key) if key.len() == 64 => owners.passkeys += 1,
                _ => return Some(None),
            }
        }
        Some(Some(owners))
    }

    /// An OpenZeppelin `AccessControl` contract: `supportsInterface` says so and
    /// `DEFAULT_ADMIN_ROLE()` is the zero word, both exactly. Its upgrade role is
    /// `UPGRADER_ROLE()` when it has one, else the admin role.
    async fn role_gate(&self, node: Node) -> Option<Option<RoleGate>> {
        let mut interface = [0u8; 32];
        interface[..4].copy_from_slice(&IACCESS_CONTROL);
        let supports = self
            .call(node, &calldata("supportsInterface(bytes4)", &[interface]))
            .await
            .settled()?;
        if !supports.is_some_and(|b| word_to_u256(&b) == Some(U256::from(1))) {
            return Some(None);
        }
        let admin = self
            .call(node, &selector("DEFAULT_ADMIN_ROLE()"))
            .await
            .settled()?;
        if !admin.is_some_and(|b| b.len() == 32 && b.iter().all(|x| *x == 0)) {
            return Some(None);
        }
        let role = self
            .call(node, &selector("UPGRADER_ROLE()"))
            .await
            .settled()?
            .filter(|b| b.len() == 32)
            .map_or(B256::ZERO, |b| B256::from_slice(&b));
        let Some(admin_role) = self.role_admin(node, role).await? else {
            return Some(Some(RoleGate {
                role,
                admin_role: B256::ZERO,
                members: None,
            }));
        };
        Some(Some(RoleGate {
            role,
            admin_role,
            members: self.role_and_admin_members(node, role, admin_role).await?,
        }))
    }

    /// `getRoleAdmin(role)`, or `Some(None)` when it is not answered as a role.
    async fn role_admin(&self, node: Node, role: B256) -> Option<Option<B256>> {
        let answer = self
            .call(node, &calldata("getRoleAdmin(bytes32)", &[role.0]))
            .await
            .settled()?;
        Some(
            answer
                .filter(|b| b.len() == 32)
                .map(|b| B256::from_slice(&b)),
        )
    }

    /// Holders of the upgrade role and of the role that administers it. Only when the admin
    /// role administers itself (as `DEFAULT_ADMIN_ROLE` does) or is the upgrade role: a third
    /// role granting the admin role would be one more link I do not follow, and a member list
    /// that stopped short of it would undercount.
    async fn role_and_admin_members(
        &self,
        node: Node,
        role: B256,
        admin_role: B256,
    ) -> Option<Option<Vec<Address>>> {
        if admin_role != role && self.role_admin(node, admin_role).await? != Some(admin_role) {
            return Some(None);
        }
        let Some(mut members) = self.role_members(node, role).await? else {
            return Some(None);
        };
        if admin_role != role {
            let Some(admins) = self.role_members(node, admin_role).await? else {
                return Some(None);
            };
            for a in admins {
                if !members.contains(&a) {
                    members.push(a);
                }
            }
        }
        Some(Some(members))
    }

    /// Every holder of `role`, when the contract enumerates them (`AccessControlEnumerable`).
    /// `Some(None)` when it does not: a plain `AccessControl` keeps no list, and its members
    /// can only be recovered from its logs.
    async fn role_members(&self, node: Node, role: B256) -> Option<Option<Vec<Address>>> {
        let count = calldata("getRoleMemberCount(bytes32)", &[role.0]);
        let Some(count) = self.call(node, &count).await.settled()? else {
            return Some(None);
        };
        let Some(n) = bounded_count(&count) else {
            return Some(None);
        };
        let mut members = Vec::with_capacity(n);
        for i in 0..n {
            let input = calldata(
                "getRoleMember(bytes32,uint256)",
                &[role.0, U256::from(i).to_be_bytes()],
            );
            let Some(raw) = self.call(node, &input).await.settled()? else {
                return Some(None);
            };
            match word_to_address_strict(&raw) {
                Some(a) => members.push(a),
                None => return Some(None),
            }
        }
        Some(Some(members))
    }

    /// Whether `implementation` says it is UUPS: `proxiableUUID()` returning the ERC-1967
    /// implementation slot, as OpenZeppelin's and Solady's `UUPSUpgradeable` both do. Asked of
    /// the implementation directly, because OpenZeppelin marks it `notDelegated` and it
    /// reverts through the proxy. `None` when the node would not tell me.
    pub async fn is_uups(&self, implementation: Address) -> Option<bool> {
        if let Some(known) = self
            .uups
            .lock()
            .expect("uups cache poisoned")
            .get(&implementation)
        {
            return Some(*known);
        }
        let answer = self
            .call(Node::base(implementation), &selector("proxiableUUID()"))
            .await
            .settled()?;
        let is_uups = answer.is_some_and(|b| b.as_ref() == IMPL_SLOT.as_slice());
        self.uups
            .lock()
            .expect("uups cache poisoned")
            .insert(implementation, is_uups);
        Some(is_uups)
    }

    /// Gather every probe reachable from `roots` within the depth limit.
    ///
    /// Levels `0..=MAX_DEPTH`, because that is how deep the key count reads: a Safe standing
    /// at the last link still needs its owners probed to be counted. Stopping one level short
    /// made every such Safe's key count unknown for want of a read, not for want of an answer.
    pub async fn collect(&self, roots: Vec<Node>) -> HashMap<Node, AuthorityProbe> {
        let mut probes: HashMap<Node, AuthorityProbe> = HashMap::new();
        let mut seen: HashSet<Node> = HashSet::new();
        let mut frontier: Vec<Node> = roots.into_iter().filter(|n| seen.insert(*n)).collect();

        for _ in 0..=MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let level: Vec<(Node, AuthorityProbe)> = stream::iter(frontier.clone())
                .map(|n| async move { self.probe_once(n).await.map(|p| (n, p)) })
                .buffer_unordered(self.concurrency)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .flatten()
                .collect();

            frontier = level
                .iter()
                .flat_map(|(n, p)| edges(*n, p))
                .filter(|n| seen.insert(*n))
                .collect();
            probes.extend(level);
        }
        probes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// Derived, not restated. A wrong selector silently answers "this interface is absent"
    /// for every contract on the chain, and every downstream test would still pass.
    #[test]
    fn selectors_match_their_signatures() {
        assert_eq!(selector("getOwners()"), [0xa0, 0xe6, 0x7e, 0x2b]);
        assert_eq!(selector("getThreshold()"), [0xe7, 0x52, 0x35, 0xb8]);
        assert_eq!(selector("owner()"), [0x8d, 0xa5, 0xcb, 0x5b]);
        assert_eq!(selector("getMinDelay()"), [0xf2, 0x7a, 0x0c, 0x92]);
        assert_eq!(selector("proxiableUUID()"), [0x52, 0xd1, 0x90, 0x2d]);
        assert_eq!(selector("entryPoint()"), [0xb0, 0xd6, 0x91, 0xfe]);
        assert_eq!(selector("nextOwnerIndex()"), [0xd9, 0x48, 0xfd, 0x2e]);
        assert_eq!(selector("DEFAULT_ADMIN_ROLE()"), [0xa2, 0x17, 0xfd, 0xdf]);
    }

    /// `type(IAccessControl).interfaceId` is the XOR of the interface's five selectors. A wrong
    /// constant answers "not AccessControl" for every contract, silently.
    #[test]
    fn the_access_control_interface_id_is_derived_from_its_selectors() {
        let id = [
            "hasRole(bytes32,address)",
            "getRoleAdmin(bytes32)",
            "grantRole(bytes32,address)",
            "revokeRole(bytes32,address)",
            "renounceRole(bytes32,address)",
        ]
        .iter()
        .map(|s| selector(s))
        .fold([0u8; 4], |acc, s| {
            [acc[0] ^ s[0], acc[1] ^ s[1], acc[2] ^ s[2], acc[3] ^ s[3]]
        });
        assert_eq!(id, IACCESS_CONTROL);
    }

    #[test]
    fn bytes_return_data_is_bounds_checked() {
        let mut data = word("20");
        data.extend(word("20"));
        data.extend(word("00000000000000000000000000000000000000a1"));
        assert_eq!(decode_bytes(&data).map(<[u8]>::len), Some(32));
        let mut short = word("20");
        short.extend(word("40"));
        short.extend(word("01"));
        assert_eq!(decode_bytes(&short), None, "claims 64 bytes, supplies 32");
        assert_eq!(decode_bytes(&word("ffff")), None, "offset past the end");
        let mut empty = word("20");
        empty.extend(word("00"));
        assert_eq!(decode_bytes(&empty), Some(&[][..]), "a removed owner");
    }

    #[test]
    fn a_count_too_large_to_be_a_signer_list_is_refused() {
        assert_eq!(bounded_count(&word("03")), Some(3));
        assert_eq!(bounded_count(&word("ffffffff")), None);
    }

    fn word(hex_tail: &str) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        let bytes = alloy::hex::decode(hex_tail).unwrap();
        w[32 - bytes.len()..].copy_from_slice(&bytes);
        w
    }

    #[test]
    fn decodes_a_well_formed_owner_array() {
        let mut data = word("20"); // offset
        data.extend(word("02")); // length
        data.extend(word("00000000000000000000000000000000000000a1"));
        data.extend(word("00000000000000000000000000000000000000b2"));
        let owners = decode_address_array(&data).unwrap();
        assert_eq!(
            owners,
            vec![
                address!("00000000000000000000000000000000000000a1"),
                address!("00000000000000000000000000000000000000b2"),
            ]
        );
    }

    /// The classic unbounded-allocation blowup: a length prefix with no payload behind it.
    #[test]
    fn a_length_prefix_longer_than_the_payload_is_rejected() {
        let mut data = word("20");
        data.extend(word("05")); // claims five, supplies one
        data.extend(word("00000000000000000000000000000000000000a1"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn an_absurd_owner_count_is_refused_before_allocating() {
        let mut data = word("20");
        data.extend(word("ffffffffffffffff"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn an_offset_pointing_past_the_payload_is_rejected() {
        let mut data = word("ffffffff");
        data.extend(word("01"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn truncated_and_empty_return_data_are_rejected() {
        assert_eq!(decode_address_array(&[]), None);
        assert_eq!(decode_address_array(&[0u8; 16]), None);
        assert_eq!(decode_address_array(&word("20")), None);
    }

    /// A word with dirty upper bytes was not written by a contract I recognize. Truncating to
    /// the low 20 bytes would fabricate an authority out of whatever noise was there.
    #[test]
    fn a_word_with_dirty_upper_bytes_is_not_an_address() {
        let mut w = word("00000000000000000000000000000000000000a1");
        w[0] = 0xff;
        assert_eq!(word_to_address_strict(&w), None);
    }

    #[test]
    fn the_zero_address_is_absent_not_an_owner() {
        assert_eq!(word_to_address_strict(&word("00")), None);
    }

    #[test]
    fn a_dirty_owner_entry_rejects_the_whole_array() {
        let mut data = word("20");
        data.extend(word("01"));
        let mut dirty = word("00000000000000000000000000000000000000a1");
        dirty[0] = 0x01;
        data.extend(dirty);
        assert_eq!(
            decode_address_array(&data),
            None,
            "one fabricated owner would change the key count"
        );
    }

    /// A node that will not answer must not decay into "the contract said no". This is the
    /// difference between reporting an unresolved authority and reporting a Safe as a
    /// single-key Ownable because a rate limiter ate `getOwners()`.
    #[test]
    fn an_undetermined_call_does_not_become_a_negative_answer() {
        assert_eq!(CallOutcome::Undetermined.settled(), None);
        assert_eq!(CallOutcome::NoAnswer.settled(), Some(None));
        let bytes = Bytes::from(vec![1u8]);
        assert_eq!(
            CallOutcome::Answered(bytes.clone()).settled(),
            Some(Some(bytes))
        );
    }

    #[test]
    fn a_revert_is_a_real_no_and_a_transport_failure_is_not() {
        assert!(is_revert("server returned an error: execution reverted"));
        assert!(is_revert("Execution Reverted"));
        assert!(is_revert("invalid opcode: INVALID"));
        assert!(!is_revert("over rate limit"));
        assert!(!is_revert("error sending request for url"));
        assert!(!is_revert("connection closed before message completed"));
        assert!(!is_revert("504 Gateway Timeout"));
    }

    /// The step-1 honesty rule, as a function: a codeless Base address whose L1 twin could
    /// not be read is not a key.
    #[test]
    fn an_unread_l1_leaves_a_codeless_base_address_undetermined() {
        let l1 = address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c");
        assert_eq!(codeless_on_base(l1, None), None);
    }

    #[test]
    fn a_codeless_base_address_is_a_key_only_when_l1_is_confirmed_empty_too() {
        let l1 = address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c");
        assert_eq!(codeless_on_base(l1, Some(true)), Some(Code::Absent));
        assert_eq!(codeless_on_base(l1, Some(false)), Some(Code::L1Alias(l1)));
    }
}
