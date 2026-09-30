//! Turning an admin address into the governance structure behind it.
//!
//! Pure: everything here walks probes that were already collected from the chain. Keeping the
//! graph logic away from the I/O is what makes cycles, depth limits and the key arithmetic
//! testable without a network, and those are precisely the places edge cases hide.
//!
//! Every value a probe carries was returned by a contract a stranger deployed, so nothing in
//! here trusts its input structurally.

use crate::chain::Node;
use alloy::primitives::{Address, B256, address};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How many links I will walk before giving up on a chain.
///
/// This is a safety control, not a budget. The edges come from contracts I do not control, so
/// an unbounded walk is an abort waiting to be deployed against me.
pub const MAX_DEPTH: usize = 4;

/// What an address turned out to be, as far as code goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// Code on its own chain, so the interface answers mean something.
    #[default]
    Present,
    /// No code here, and none at the L1 address that could act as this one. A key.
    Absent,
    /// No code on Base, but the address it unaliases to has code on Ethereum.
    ///
    /// Nobody holds a private key for this address. The L1 contract is the authority, and this
    /// is only how it appears on L2. Reading "no code" as "EOA" here is the mistake that made
    /// a 2-of-2 Safe eleven keys deep look like one key.
    L1Alias(Address),
    /// An EIP-7702 delegation designator (`0xef0100` and an address): an externally owned
    /// account that has pointed its code at another contract.
    ///
    /// Still a key. The private key behind it signs for it directly whatever the delegate
    /// allows, and can re-delegate at will, so the delegate only ever adds ways in. Reading the
    /// delegate's interface as this account's authority would describe the lock and miss the
    /// key: 23 of 87 "smart accounts" behind unresolved proxies on 2026-09-30 were these.
    Delegated(Address),
}

/// The signers a MultiOwnable account lists (Coinbase Smart Wallet): any one of them signs
/// for it alone. Owners that are passkeys have no address, so they are counted, not listed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountOwners {
    pub addresses: Vec<Address>,
    pub passkeys: u32,
}

/// An OpenZeppelin `AccessControl` contract and the role taken to gate its upgrades.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleGate {
    /// `UPGRADER_ROLE()` when the contract has one, else `DEFAULT_ADMIN_ROLE` (zero). A
    /// convention, not a proof, which is why nothing concluded through a role is High.
    pub role: B256,
    /// `getRoleAdmin(role)`: whoever holds it can grant themselves `role` in one transaction,
    /// so its holders are upgrade authority as surely as `role`'s own. An empty upgrade role
    /// under a held admin role is not "nobody can upgrade" (measured on Base, 2026-09-30).
    pub admin_role: B256,
    /// Every holder of `role` or `admin_role`, when the contract enumerates them. `None` when
    /// it does not, since members of a plain `AccessControl` are only recoverable from its
    /// logs, and when the admin role is itself administered by a third role I do not follow.
    pub members: Option<Vec<Address>>,
}

/// What one address answered when probed for the interfaces I recognize.
///
/// Serializable because the store keeps each one whole: it is the exact input the resolver
/// walks, so an offline walk over stored probes gives the answer the scan published. Fields
/// added later default to absent, which is what an older probe that never asked means.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthorityProbe {
    pub code: Code,
    pub owners: Option<Vec<Address>>,
    pub threshold: Option<u32>,
    pub owner: Option<Address>,
    pub min_delay: Option<u64>,
    /// What `entryPoint()` answered: an ERC-4337 account.
    pub entry_point: Option<Address>,
    /// The signer list of a MultiOwnable account, when it is one.
    pub account_owners: Option<AccountOwners>,
    /// Present when the contract is an OpenZeppelin `AccessControl`.
    pub roles: Option<RoleGate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// No code on any chain that could act as it, or only an EIP-7702 delegation. Terminal, and
    /// one key away from control.
    Eoa,
    /// Answered both `getOwners()` and `getThreshold()`. Terminal: a Safe is a governance
    /// structure in its own right, and its owners feed the key count rather than the chain.
    Safe,
    /// Answered `owner()`. The owner is the real authority, so the chain continues.
    Ownable,
    /// Answered `getMinDelay()`.
    Timelock,
    /// The L2 face of an Ethereum contract. The chain continues on Ethereum, so this is only
    /// ever terminal when the walk ran out of depth standing on it.
    L1Alias,
    /// Answered `entryPoint()`: an ERC-4337 account, whose own signers are the authority.
    /// Terminal. Its keys are counted only when its signer scheme is one I read.
    SmartAccount,
    /// An OpenZeppelin `AccessControl` contract: whoever holds the upgrade role is the
    /// authority, any one of them alone. Terminal.
    RoleGated,
    /// An address nobody can hold a key for: a burn address or a precompile. Terminal, with no
    /// key count, because there is no key to count.
    Sentinel,
    /// Nothing I recognize answered. Never guessed at.
    Unknown,
}

impl AuthorityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eoa => "eoa",
            Self::Safe => "safe",
            Self::Ownable => "ownable",
            Self::Timelock => "timelock",
            Self::L1Alias => "l1_alias",
            Self::SmartAccount => "smart_account",
            Self::RoleGated => "role_gated",
            Self::Sentinel => "sentinel",
            Self::Unknown => "unknown",
        }
    }
}

/// Addresses no one can hold a key for: the conventional burn addresses and the precompiles.
///
/// `owner()` pointing at one of these means the owner renounced, and calling it "one key" would
/// put a renounced contract at the top of a ranking of keys. The list is explicit rather than a
/// rule like "any tiny address" because it only has to name what contracts actually renounce
/// to: `0x…dEaD`, `0xff…ff`, the precompiles every EVM chain has had since Cancun (`0x01` to
/// `0x0a`) and Base's P-256 verifier (`0x100`).
pub fn is_sentinel(a: Address) -> bool {
    const DEAD: Address = address!("000000000000000000000000000000000000dEaD");
    const P256: Address = address!("0000000000000000000000000000000000000100");
    let precompile = a.as_slice()[..19].iter().all(|b| *b == 0) && (1..=0x0a).contains(&a[19]);
    a == DEAD || a == P256 || a == Address::repeat_byte(0xff) || precompile
}

/// How much of a resolution I am willing to stand behind.
///
/// Ordered so that `min` expresses the rule that matters: confidence can only ever fall as a
/// chain is walked. One unrecognized node poisons everything downstream of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Unknown,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The root of the chain, never the immediate admin, and on whichever chain it lives.
    pub terminal: Node,
    pub kind: AuthorityKind,
    /// Fewest distinct keys that have to be compromised to exercise this authority.
    ///
    /// `None` rather than a number whenever any part of the chain was not positively
    /// identified. A guess here reads as a safety margin that does not exist.
    pub compromise_depth: Option<u32>,
    /// Seconds of delay standing in the way. `0` means no timelock was found, which is a
    /// different claim from not knowing.
    pub timelock_seconds: u64,
    pub confidence: Confidence,
    /// Every node walked, starting at the admin.
    pub path: Vec<Node>,
    /// The walk hit `MAX_DEPTH` before reaching a terminal node.
    pub truncated: bool,
    /// The walk re-entered an address it had already visited.
    pub cycle: bool,
    /// The walk stopped at a node nobody answered for. Different from a node that answered
    /// and was not recognized: one is the network's failure, the other is a real finding.
    pub unanswered: bool,
    /// Why the root's own keys cannot be counted, when that is the reason. `None` leaves the
    /// reason to the flags above, or to an owner further down.
    pub root_gap: Option<DepthGap>,
}

/// Why a resolution has no root I will publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// A covered kind with no upgrade entry point I model.
    NoUpgradePath,
    /// Implementation set, no admin, but the implementation did not answer `proxiableUUID()`
    /// with the ERC-1967 slot, so I cannot say the upgrade logic lives there.
    UupsUnconfirmed,
    /// The walk reached a contract that answered none of the interfaces I recognize.
    UnrecognizedInterface,
    /// A node would not answer a read the walk needed.
    RpcUndetermined,
}

impl Unresolved {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoUpgradePath => "no_upgrade_path",
            Self::UupsUnconfirmed => "uups_unconfirmed",
            Self::UnrecognizedInterface => "unrecognized_interface",
            Self::RpcUndetermined => "rpc_undetermined",
        }
    }
}

/// Why a published root has no key count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthGap {
    Truncated,
    Cycle,
    /// Some owner under a Safe could not be identified. One unknown owner could be a single
    /// key, so the whole count is unknown rather than counted without it.
    OwnersUnknown,
    /// The root is a timelock or an `AccessControl` contract whose role holders it does not
    /// list. They are only recoverable from its logs.
    RolesUnread,
    /// The root is a smart account whose signer scheme I do not read.
    AccountKeysUnread,
    /// The root is an address no one holds a key for.
    NoKnownKey,
}

impl DepthGap {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::Cycle => "cycle",
            Self::OwnersUnknown => "owners_unknown",
            Self::RolesUnread => "roles_unread",
            Self::AccountKeysUnread => "account_keys_unread",
            Self::NoKnownKey => "no_known_key",
        }
    }
}

impl Resolution {
    /// Why there is no root to publish, or `None` when there is one.
    pub fn unresolved(&self) -> Option<Unresolved> {
        (self.confidence == Confidence::Unknown).then_some(if self.unanswered {
            Unresolved::RpcUndetermined
        } else {
            Unresolved::UnrecognizedInterface
        })
    }

    /// Why a root that is published has no key count, or `None` when it has one.
    pub fn depth_gap(&self) -> Option<DepthGap> {
        if self.unresolved().is_some() || self.compromise_depth.is_some() {
            return None;
        }
        Some(if self.cycle {
            DepthGap::Cycle
        } else if self.truncated {
            DepthGap::Truncated
        } else {
            self.root_gap.unwrap_or(DepthGap::OwnersUnknown)
        })
    }

    /// The same resolution, trusted no more than `ceiling`. Confidence only ever falls.
    pub fn capped(mut self, ceiling: Confidence) -> Self {
        self.confidence = self.confidence.min(ceiling);
        self
    }
}

/// Decide what an address is from what it answered, plus whether the answers conflict.
///
/// Precedence is documented rather than incidental, because probing is duck typing and duck
/// typing has no uniqueness guarantee. A contract answering both the Safe and the Timelock
/// probe resolves as a Safe, and says so by giving up High confidence. The kinds added later
/// (`RoleGated`, `SmartAccount`) come last: the scanner only asks about them when nothing
/// earlier answered, so they can never shadow an answer the walk already relied on.
fn classify(node: Node, probe: &AuthorityProbe) -> (AuthorityKind, bool) {
    match probe.code {
        Code::Absent | Code::Delegated(_) if is_sentinel(node.address) => {
            return (AuthorityKind::Sentinel, false);
        }
        Code::Absent | Code::Delegated(_) => return (AuthorityKind::Eoa, false),
        Code::L1Alias(_) => return (AuthorityKind::L1Alias, false),
        Code::Present => {}
    }
    let is_safe = probe.owners.is_some() && probe.threshold.is_some();
    let is_timelock = probe.min_delay.is_some();
    match (is_safe, is_timelock, probe.owner.is_some()) {
        (true, ambiguous, _) => (AuthorityKind::Safe, ambiguous),
        (false, true, _) => (AuthorityKind::Timelock, false),
        (false, false, true) => (AuthorityKind::Ownable, false),
        (false, false, false) if probe.roles.is_some() => (AuthorityKind::RoleGated, false),
        (false, false, false) if probe.entry_point.is_some() => {
            (AuthorityKind::SmartAccount, false)
        }
        (false, false, false) => (AuthorityKind::Unknown, false),
    }
}

/// What `probe` makes the address at `node`, by the same precedence the walk uses.
pub fn authority_kind(node: Node, probe: &AuthorityProbe) -> AuthorityKind {
    classify(node, probe).0
}

/// Where control passes from a node that is not terminal.
///
/// An owner lives on the chain it was read from. The alias is the one edge that crosses from
/// Base to Ethereum.
pub fn successor(node: Node, probe: &AuthorityProbe) -> Option<Node> {
    match probe.code {
        Code::L1Alias(l1) => Some(Node::ethereum(l1)),
        Code::Delegated(_) => None,
        Code::Present | Code::Absent => probe.owner.map(|address| Node {
            chain: node.chain,
            address,
        }),
    }
}

/// The addresses whose keys a terminal node's own key count is made of: a Safe's owners, a
/// MultiOwnable account's address signers, the holders of an upgrade role.
pub fn signers(probe: &AuthorityProbe) -> impl Iterator<Item = Address> + '_ {
    probe
        .owners
        .iter()
        .flatten()
        .chain(probe.account_owners.iter().flat_map(|o| &o.addresses))
        .chain(probe.roles.iter().flat_map(|r| r.members.iter().flatten()))
        .copied()
}

/// Every node whose answers could change how `node` resolves: its signers and its successor.
/// Collection uses this to decide what to probe next, so the scanner and the walk can never
/// disagree about which chain an owner lives on.
pub fn edges(node: Node, probe: &AuthorityProbe) -> Vec<Node> {
    let mut out: Vec<Node> = signers(probe)
        .map(|address| Node {
            chain: node.chain,
            address,
        })
        .collect();
    out.extend(successor(node, probe));
    out
}

/// Where a walk stopped and why.
struct Stop {
    kind: AuthorityKind,
    confidence: Confidence,
    timelock_seconds: u64,
    truncated: bool,
    cycle: bool,
    unanswered: bool,
}

/// Follow ownership until something terminal, unrecognized, cyclic or too deep stops it.
fn walk(start: Node, probes: &HashMap<Node, AuthorityProbe>, path: &mut Vec<Node>) -> Stop {
    let mut seen = HashSet::new();
    let mut stop = Stop {
        kind: AuthorityKind::Unknown,
        confidence: Confidence::High,
        timelock_seconds: 0,
        truncated: false,
        cycle: false,
        unanswered: false,
    };
    let mut current = start;

    loop {
        if !seen.insert(current) {
            stop.cycle = true;
            stop.confidence = stop.confidence.min(Confidence::Medium);
            return stop;
        }
        path.push(current);

        let Some(probe) = probes.get(&current) else {
            stop.kind = AuthorityKind::Unknown;
            stop.confidence = Confidence::Unknown;
            stop.unanswered = true;
            return stop;
        };

        let (kind, ambiguous) = classify(current, probe);
        stop.kind = kind;
        if ambiguous {
            stop.confidence = stop.confidence.min(Confidence::Medium);
        }

        match kind {
            AuthorityKind::Eoa | AuthorityKind::Safe | AuthorityKind::Sentinel => return stop,
            // Answering `entryPoint()` proves what a contract is, not who may upgrade what it
            // controls, and a role taken to be the upgrade role is a convention. Either way the
            // root is real and the reading of it is not certain.
            AuthorityKind::SmartAccount | AuthorityKind::RoleGated => {
                stop.confidence = stop.confidence.min(Confidence::Medium);
                return stop;
            }
            AuthorityKind::Unknown => {
                stop.confidence = Confidence::Unknown;
                return stop;
            }
            AuthorityKind::Timelock => {
                stop.timelock_seconds = stop.timelock_seconds.max(probe.min_delay.unwrap_or(0));
                // Proposer and executor are distinct role sets with different key
                // requirements, and I model the timelock as one node. Anything concluded
                // through it is an approximation, so it cannot stay High.
                stop.confidence = stop.confidence.min(Confidence::Medium);
            }
            AuthorityKind::Ownable | AuthorityKind::L1Alias => {}
        }

        match successor(current, probe) {
            Some(next) if path.len() < MAX_DEPTH => current = next,
            Some(_) => {
                stop.truncated = true;
                stop.confidence = stop.confidence.min(Confidence::Medium);
                return stop;
            }
            None => return stop,
        }
    }
}

/// Fewest distinct keys needed to exercise the authority at `node`.
///
/// `None` means I could not determine it, which is a different answer from a large number and
/// must not be rendered as one.
fn keys_required(
    node: Node,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Node>,
) -> Option<u32> {
    if depth > MAX_DEPTH || !seen.insert(node) {
        return None;
    }
    let probe = probes.get(&node)?;
    let cost = match classify(node, probe).0 {
        AuthorityKind::Eoa => Some(1),
        AuthorityKind::Safe => {
            let threshold = probe.threshold? as usize;
            cheapest(
                node,
                probe.owners.as_ref()?,
                0,
                threshold,
                probes,
                depth,
                seen,
            )
        }
        AuthorityKind::SmartAccount => {
            let signers = probe.account_owners.as_ref()?;
            cheapest(
                node,
                &signers.addresses,
                signers.passkeys,
                1,
                probes,
                depth,
                seen,
            )
        }
        AuthorityKind::RoleGated => {
            let members = probe.roles.as_ref()?.members.as_ref()?;
            cheapest(node, members, 0, 1, probes, depth, seen)
        }
        AuthorityKind::Ownable | AuthorityKind::Timelock | AuthorityKind::L1Alias => {
            keys_required(successor(node, probe)?, probes, depth + 1, seen)
        }
        AuthorityKind::Sentinel | AuthorityKind::Unknown => None,
    };
    seen.remove(&node);
    cost
}

/// An m-of-n set of signers costs the sum of its **m cheapest** members, not the first m in
/// array order. A Safe is m-of-n; a MultiOwnable account and an upgrade role are 1-of-n.
///
/// Taking array order is a plausible bug that produces plausible numbers, which is the worst
/// kind. If any member's cost is unknown the whole total is unknown: an unrecognized member
/// could be a single EOA, so reporting the cheapest m of the ones I do understand would
/// overstate how many keys an attacker actually needs. `passkeys` are signers without an
/// address, each exactly one key.
fn cheapest(
    node: Node,
    members: &[Address],
    passkeys: u32,
    threshold: usize,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Node>,
) -> Option<u32> {
    if threshold == 0 || threshold > members.len() + passkeys as usize {
        return None;
    }
    let mut costs = members
        .iter()
        .map(|&address| {
            let member = Node {
                chain: node.chain,
                address,
            };
            keys_required(member, probes, depth + 1, seen)
        })
        .collect::<Option<Vec<u32>>>()?;
    costs.extend(std::iter::repeat_n(1, passkeys as usize));
    costs.sort_unstable();
    Some(
        costs
            .iter()
            .take(threshold)
            .fold(0u32, |acc, c| acc.saturating_add(*c)),
    )
}

/// Why the root's own keys cannot be counted, when the root's kind is the reason.
fn root_gap(root: Node, probe: Option<&AuthorityProbe>) -> Option<DepthGap> {
    let probe = probe?;
    match classify(root, probe).0 {
        AuthorityKind::Sentinel => Some(DepthGap::NoKnownKey),
        AuthorityKind::Timelock => Some(DepthGap::RolesUnread),
        AuthorityKind::RoleGated => match &probe.roles.as_ref()?.members {
            None => Some(DepthGap::RolesUnread),
            // Nobody holds the role or the role that grants it: renounced, as far as any key
            // I can see goes.
            Some(members) if members.is_empty() => Some(DepthGap::NoKnownKey),
            Some(_) => None,
        },
        AuthorityKind::SmartAccount if probe.account_owners.is_none() => {
            Some(DepthGap::AccountKeysUnread)
        }
        _ => None,
    }
}

/// Resolve an admin to the authority that actually stands behind it.
pub fn resolve(admin: Node, probes: &HashMap<Node, AuthorityProbe>) -> Resolution {
    let mut path = Vec::new();
    let stop = walk(admin, probes, &mut path);
    let terminal = *path.last().unwrap_or(&admin);

    // A truncated, cyclic or unrecognized chain has no trustworthy key count, and a number
    // here would read as a safety margin rather than as the guess it would be.
    let compromise_depth = if stop.truncated || stop.cycle || stop.confidence == Confidence::Unknown
    {
        None
    } else {
        keys_required(admin, probes, 0, &mut HashSet::new())
    };

    Resolution {
        terminal,
        kind: stop.kind,
        compromise_depth,
        timelock_seconds: stop.timelock_seconds,
        confidence: stop.confidence,
        path,
        truncated: stop.truncated,
        cycle: stop.cycle,
        unanswered: stop.unanswered,
        root_gap: root_gap(terminal, probes.get(&terminal)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Chain, undo_l1_to_l2_alias};
    use alloy::primitives::address;

    const A: Address = address!("00000000000000000000000000000000000000a1");
    const B: Address = address!("00000000000000000000000000000000000000b2");
    const C: Address = address!("00000000000000000000000000000000000000c3");
    const D: Address = address!("00000000000000000000000000000000000000d4");
    const E: Address = address!("00000000000000000000000000000000000000e5");

    fn eoa() -> AuthorityProbe {
        AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        }
    }

    fn ownable(owner: Address) -> AuthorityProbe {
        AuthorityProbe {
            owner: Some(owner),
            ..Default::default()
        }
    }

    fn safe(threshold: u32, owners: &[Address]) -> AuthorityProbe {
        AuthorityProbe {
            owners: Some(owners.to_vec()),
            threshold: Some(threshold),
            ..Default::default()
        }
    }

    fn timelock(delay: u64, owner: Option<Address>) -> AuthorityProbe {
        AuthorityProbe {
            min_delay: Some(delay),
            owner,
            ..Default::default()
        }
    }

    fn alias_of(l1: Address) -> AuthorityProbe {
        AuthorityProbe {
            code: Code::L1Alias(l1),
            ..Default::default()
        }
    }

    /// A graph that lives entirely on Base, which is every graph before aliasing.
    fn graph(entries: &[(Address, AuthorityProbe)]) -> HashMap<Node, AuthorityProbe> {
        entries
            .iter()
            .map(|(a, p)| (Node::base(*a), p.clone()))
            .collect()
    }

    fn resolve_base(admin: Address, g: &HashMap<Node, AuthorityProbe>) -> Resolution {
        resolve(Node::base(admin), g)
    }

    #[test]
    fn an_eoa_admin_is_one_key_away() {
        let g = graph(&[(A, eoa())]);
        let r = resolve_base(A, &g);
        assert_eq!(r.terminal, Node::base(A));
        assert_eq!(r.kind, AuthorityKind::Eoa);
        assert_eq!(r.compromise_depth, Some(1));
        assert_eq!(r.confidence, Confidence::High);
        assert_eq!(r.timelock_seconds, 0);
    }

    /// The detail worth the most: two proxies under different ProxyAdmins owned by one Safe
    /// must land on the Safe, not on their respective ProxyAdmins. Grouping on the immediate
    /// admin fragments the picture and understates exposure.
    #[test]
    fn distinct_proxy_admins_under_one_safe_resolve_to_that_safe() {
        let g = graph(&[
            (A, ownable(C)),
            (B, ownable(C)),
            (C, safe(2, &[D, E])),
            (D, eoa()),
            (E, eoa()),
        ]);
        let from_a = resolve_base(A, &g);
        let from_b = resolve_base(B, &g);
        assert_eq!(from_a.terminal, Node::base(C));
        assert_eq!(from_b.terminal, Node::base(C));
        assert_eq!(
            from_a.terminal, from_b.terminal,
            "entry point must not change where a shared subgraph terminates"
        );
    }

    #[test]
    fn a_two_of_three_safe_over_eoas_costs_two_keys() {
        let g = graph(&[(A, safe(2, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        let r = resolve_base(A, &g);
        assert_eq!(r.kind, AuthorityKind::Safe);
        assert_eq!(r.compromise_depth, Some(2));
    }

    #[test]
    fn a_one_of_n_safe_costs_one_key() {
        let g = graph(&[(A, safe(1, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        assert_eq!(resolve_base(A, &g).compromise_depth, Some(1));
    }

    /// Taking the first m owners in array order is a very plausible bug that produces very
    /// plausible numbers, so the cheap owner is deliberately placed last.
    #[test]
    fn a_nested_safe_costs_the_cheapest_owners_not_the_first_ones() {
        let g = graph(&[
            (A, safe(1, &[B, C])),
            // An expensive owner first, a single key last.
            (B, safe(2, &[D, E])),
            (C, eoa()),
            (D, eoa()),
            (E, eoa()),
        ]);
        assert_eq!(
            resolve_base(A, &g).compromise_depth,
            Some(1),
            "the cheapest owner is the one an attacker picks"
        );
    }

    #[test]
    fn nested_safes_sum_the_cheapest_owner_costs() {
        // 2-of-2 over a 2-of-2 Safe and an EOA: 2 + 1.
        let g = graph(&[
            (A, safe(2, &[B, C])),
            (B, safe(2, &[D, E])),
            (C, eoa()),
            (D, eoa()),
            (E, eoa()),
        ]);
        assert_eq!(resolve_base(A, &g).compromise_depth, Some(3));
    }

    #[test]
    fn a_timelock_delay_is_captured_and_resolution_continues_through_it() {
        let g = graph(&[(A, timelock(172_800, Some(B))), (B, eoa())]);
        let r = resolve_base(A, &g);
        assert_eq!(r.timelock_seconds, 172_800);
        assert_eq!(r.terminal, Node::base(B));
        assert_eq!(
            r.confidence,
            Confidence::Medium,
            "modelling a timelock as one node is an approximation, so it cannot read as High"
        );
    }

    /// A self-owning ProxyAdmin is live on Base at 0x42..0018, not a hypothetical.
    #[test]
    fn a_self_owning_contract_terminates_with_a_named_cycle() {
        let g = graph(&[(A, ownable(A))]);
        let r = resolve_base(A, &g);
        assert!(
            r.cycle,
            "the cycle must be reported, not inferred from a shrug"
        );
        assert_eq!(r.terminal, Node::base(A));
        assert_eq!(r.confidence, Confidence::Medium);
        assert_eq!(
            r.compromise_depth, None,
            "a cycle has no trustworthy key count"
        );
    }

    #[test]
    fn a_longer_cycle_also_terminates() {
        let g = graph(&[(A, ownable(B)), (B, ownable(C)), (C, ownable(A))]);
        let r = resolve_base(A, &g);
        assert!(r.cycle);
        assert!(r.path.len() <= MAX_DEPTH);
    }

    #[test]
    fn a_chain_longer_than_the_cap_is_truncated_rather_than_followed() {
        let long = [A, B, C, D, E];
        let mut entries: Vec<_> = long
            .windows(2)
            .map(|w| (w[0], ownable(w[1])))
            .collect::<Vec<_>>();
        entries.push((E, eoa()));
        let r = resolve_base(A, &graph(&entries));
        assert!(r.truncated);
        assert_eq!(r.path.len(), MAX_DEPTH);
        assert_ne!(r.confidence, Confidence::High);
        assert_eq!(r.compromise_depth, None);
    }

    #[test]
    fn an_unrecognized_node_makes_the_whole_resolution_unknown() {
        let g = graph(&[(A, ownable(B)), (B, AuthorityProbe::default())]);
        let r = resolve_base(A, &g);
        assert_eq!(r.kind, AuthorityKind::Unknown);
        assert_eq!(r.confidence, Confidence::Unknown);
        assert_eq!(r.compromise_depth, None);
        assert_eq!(r.terminal, Node::base(B));
    }

    #[test]
    fn an_address_that_was_never_probed_is_unknown_not_assumed() {
        let g = graph(&[(A, ownable(B))]);
        let r = resolve_base(A, &g);
        assert_eq!(r.confidence, Confidence::Unknown);
        assert_eq!(r.compromise_depth, None);
        assert_eq!(r.unresolved(), Some(Unresolved::RpcUndetermined));
    }

    /// The two kinds of Unknown are different findings. "The node would not say" is an outage;
    /// "it said, and I do not recognize it" is a gap in the model worth naming.
    #[test]
    fn an_unrecognized_contract_and_an_unanswered_read_are_different_reasons() {
        let unrecognized = graph(&[(A, ownable(B)), (B, AuthorityProbe::default())]);
        let unanswered = graph(&[(A, ownable(B))]);
        assert_eq!(
            resolve_base(A, &unrecognized).unresolved(),
            Some(Unresolved::UnrecognizedInterface)
        );
        assert_eq!(
            resolve_base(A, &unanswered).unresolved(),
            Some(Unresolved::RpcUndetermined)
        );
    }

    #[test]
    fn a_published_root_without_a_key_count_says_why() {
        let cycle = graph(&[(A, ownable(A))]);
        let long = graph(&[
            (A, ownable(B)),
            (B, ownable(C)),
            (C, ownable(D)),
            (D, ownable(E)),
            (E, eoa()),
        ]);
        let unknown_owner = graph(&[(A, safe(1, &[B, C])), (B, eoa())]);
        assert_eq!(resolve_base(A, &cycle).depth_gap(), Some(DepthGap::Cycle));
        assert_eq!(
            resolve_base(A, &long).depth_gap(),
            Some(DepthGap::Truncated)
        );
        let r = resolve_base(A, &unknown_owner);
        assert_eq!(r.unresolved(), None, "the Safe itself is a real root");
        assert_eq!(r.depth_gap(), Some(DepthGap::OwnersUnknown));
        assert_eq!(resolve_base(A, &graph(&[(A, eoa())])).depth_gap(), None);
    }

    #[test]
    fn a_cap_lowers_confidence_and_never_raises_it() {
        let g = graph(&[(A, eoa())]);
        assert_eq!(
            resolve_base(A, &g).capped(Confidence::Medium).confidence,
            Confidence::Medium
        );
        let unknown = graph(&[(A, AuthorityProbe::default())]);
        assert_eq!(
            resolve_base(A, &unknown)
                .capped(Confidence::High)
                .confidence,
            Confidence::Unknown
        );
    }

    #[test]
    fn confidence_never_rises_along_a_chain() {
        // A timelock (Medium) sitting above a perfectly ordinary EOA must stay Medium.
        let g = graph(&[(A, ownable(B)), (B, timelock(100, Some(C))), (C, eoa())]);
        assert_eq!(resolve_base(A, &g).confidence, Confidence::Medium);
    }

    #[test]
    fn a_contract_answering_both_safe_and_timelock_probes_resolves_deterministically() {
        let mut both = safe(2, &[B, C]);
        both.min_delay = Some(600);
        let g = graph(&[(A, both), (B, eoa()), (C, eoa())]);
        let r = resolve_base(A, &g);
        assert_eq!(r.kind, AuthorityKind::Safe, "documented precedence");
        assert_eq!(
            r.confidence,
            Confidence::Medium,
            "an ambiguous interface must cost confidence, not be silently picked"
        );
    }

    #[test]
    fn a_threshold_larger_than_the_owner_set_is_rejected_not_trusted() {
        let g = graph(&[(A, safe(9, &[B, C])), (B, eoa()), (C, eoa())]);
        assert_eq!(resolve_base(A, &g).compromise_depth, None);
    }

    #[test]
    fn a_zero_threshold_is_rejected() {
        let g = graph(&[(A, safe(0, &[B])), (B, eoa())]);
        assert_eq!(resolve_base(A, &g).compromise_depth, None);
    }

    #[test]
    fn an_enormous_threshold_saturates_instead_of_wrapping() {
        let owners: Vec<Address> = (0..3).map(|_| B).collect();
        let g = graph(&[(A, safe(u32::MAX, &owners)), (B, eoa())]);
        assert_eq!(
            resolve_base(A, &g).compromise_depth,
            None,
            "four billion keys is not three"
        );
    }

    #[test]
    fn depth_is_at_least_one_wherever_it_is_known() {
        let graphs = [
            graph(&[(A, eoa())]),
            graph(&[(A, safe(1, &[B])), (B, eoa())]),
            graph(&[(A, ownable(B)), (B, eoa())]),
        ];
        for g in &graphs {
            if let Some(d) = resolve_base(A, g).compromise_depth {
                assert!(d >= 1, "control always costs at least one key");
            }
        }
    }

    #[test]
    fn owner_ordering_does_not_change_the_outcome() {
        let forward = graph(&[(A, safe(2, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        let reversed = graph(&[(A, safe(2, &[D, C, B])), (B, eoa()), (C, eoa()), (D, eoa())]);
        assert_eq!(
            resolve_base(A, &forward).compromise_depth,
            resolve_base(A, &reversed).compromise_depth
        );
    }

    #[test]
    fn edges_are_the_owners_plus_the_successor_on_the_same_chain() {
        let p = AuthorityProbe {
            owners: Some(vec![A]),
            owner: Some(B),
            ..Default::default()
        };
        assert_eq!(
            edges(Node::ethereum(C), &p),
            vec![Node::ethereum(A), Node::ethereum(B)],
            "an owner read on Ethereum is an Ethereum address"
        );
        assert!(edges(Node::base(C), &AuthorityProbe::default()).is_empty());
    }

    #[test]
    fn an_alias_has_exactly_one_edge_and_it_crosses_to_ethereum() {
        assert_eq!(edges(Node::base(A), &alias_of(B)), vec![Node::ethereum(B)]);
    }

    /// The live structure behind the 20 OP Stack predeploys, as read on 2026-09-19 (Ethereum
    /// block 26,012,852). The ProxyAdmin's owner has no code on Base because it is the alias of
    /// a 2-of-2 Safe on Ethereum, whose owners are a 3-of-6 and an 8-of-11 Safe over EOAs.
    mod predeploy_authority {
        use super::*;

        pub const PROXY_ADMIN: Address = address!("4200000000000000000000000000000000000018");
        pub const ALIAS: Address = address!("8cC51c3008b3f03Fe483B28B8Db90e19cF076a6d");
        pub const L1_SAFE: Address = address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c");
        pub const COORDINATOR: Address = address!("9855054731540A48b28990B63DcF4f33d8AE46A1");
        pub const COUNCIL: Address = address!("20AcF55A3DCfe07fC4cecaCFa1628F788EC8A4Dd");

        fn leaves(first: u8, n: u8) -> Vec<Address> {
            (first..first + n).map(Address::with_last_byte).collect()
        }

        /// `leaf` decides what each of the 17 owners at the bottom is.
        pub fn graph(
            leaf: fn(Address) -> Vec<(Node, AuthorityProbe)>,
        ) -> HashMap<Node, AuthorityProbe> {
            let coordinator_owners = leaves(0x10, 6);
            let council_owners = leaves(0x20, 11);
            let mut g: HashMap<Node, AuthorityProbe> = [
                (Node::base(PROXY_ADMIN), ownable(ALIAS)),
                (Node::base(ALIAS), alias_of(L1_SAFE)),
                (Node::ethereum(L1_SAFE), safe(2, &[COORDINATOR, COUNCIL])),
                (Node::ethereum(COORDINATOR), safe(3, &coordinator_owners)),
                (Node::ethereum(COUNCIL), safe(8, &council_owners)),
            ]
            .into_iter()
            .collect();
            for owner in coordinator_owners.into_iter().chain(council_owners) {
                g.extend(leaf(owner));
            }
            g
        }

        pub fn eoa_leaf(a: Address) -> Vec<(Node, AuthorityProbe)> {
            vec![(Node::ethereum(a), eoa())]
        }
    }

    /// The headline correction. Before aliasing was modelled, this resolved to one EOA at
    /// `0x8cC5…` with a key count of 1 and High confidence.
    #[test]
    fn the_predeploy_admin_resolves_to_a_safe_on_ethereum_eleven_keys_deep() {
        use predeploy_authority::*;
        assert_eq!(
            undo_l1_to_l2_alias(ALIAS),
            L1_SAFE,
            "the fixture is the real pair"
        );

        let r = resolve(Node::base(PROXY_ADMIN), &graph(eoa_leaf));
        assert_eq!(r.terminal, Node::ethereum(L1_SAFE));
        assert_eq!(r.terminal.chain, Chain::Ethereum);
        assert_eq!(r.kind, AuthorityKind::Safe);
        assert_eq!(
            r.compromise_depth,
            Some(3 + 8),
            "the cheapest way in is both inner thresholds, not one key"
        );
        assert_eq!(r.confidence, Confidence::High);
        assert_eq!(
            r.path,
            vec![
                Node::base(PROXY_ADMIN),
                Node::base(ALIAS),
                Node::ethereum(L1_SAFE)
            ]
        );
    }

    /// ProxyAdmin → alias → L1 Safe → inner Safes → leaf keys puts the leaves at exactly
    /// `MAX_DEPTH`. One more level must come back unknown rather than be quietly counted, and
    /// if the real leaves ever turn out to be Safes that is a reason to revisit the cap on
    /// purpose.
    #[test]
    fn the_predeploy_chain_lands_exactly_on_the_depth_cap() {
        use predeploy_authority::*;
        fn one_of_one_safe_leaf(a: Address) -> Vec<(Node, AuthorityProbe)> {
            let key = Address::from_word(alloy::primitives::keccak256(a));
            vec![
                (Node::ethereum(a), safe(1, &[key])),
                (Node::ethereum(key), eoa()),
            ]
        }
        assert_eq!(
            resolve(Node::base(PROXY_ADMIN), &graph(eoa_leaf)).compromise_depth,
            Some(11)
        );
        let deeper = resolve(Node::base(PROXY_ADMIN), &graph(one_of_one_safe_leaf));
        assert_eq!(deeper.terminal, Node::ethereum(L1_SAFE));
        assert_eq!(
            deeper.compromise_depth, None,
            "a key one level past the cap is not a key I counted"
        );
    }

    #[test]
    fn an_alias_whose_l1_contract_was_never_probed_is_unknown() {
        use predeploy_authority::*;
        let g: HashMap<Node, AuthorityProbe> = [
            (Node::base(PROXY_ADMIN), ownable(ALIAS)),
            (Node::base(ALIAS), alias_of(L1_SAFE)),
        ]
        .into_iter()
        .collect();
        let r = resolve(Node::base(PROXY_ADMIN), &g);
        assert_eq!(r.confidence, Confidence::Unknown);
        assert_eq!(r.compromise_depth, None);
        assert_ne!(r.kind, AuthorityKind::Eoa, "an unread L1 is never a key");
    }

    /// The same 20 bytes on two chains are two accounts. Following the alias has to land on
    /// the Ethereum probe even when Base has an unrelated answer for the same address.
    #[test]
    fn an_alias_follows_the_ethereum_account_not_the_base_one_at_the_same_address() {
        let mut g = graph(&[(A, ownable(B)), (B, alias_of(C)), (C, eoa())]);
        g.insert(Node::ethereum(C), safe(2, &[D, E]));
        g.insert(Node::ethereum(D), eoa());
        g.insert(Node::ethereum(E), eoa());
        let r = resolve_base(A, &g);
        assert_eq!(r.terminal, Node::ethereum(C));
        assert_eq!(r.compromise_depth, Some(2), "not the Base EOA's one key");
    }

    fn account(owners: &[Address], passkeys: u32) -> AuthorityProbe {
        AuthorityProbe {
            entry_point: Some(address!("0000000071727De22E5E9d8BAf0edAc6f37da032")),
            account_owners: Some(AccountOwners {
                addresses: owners.to_vec(),
                passkeys,
            }),
            ..Default::default()
        }
    }

    fn role_gated(members: Option<&[Address]>) -> AuthorityProbe {
        AuthorityProbe {
            roles: Some(RoleGate {
                role: B256::ZERO,
                admin_role: B256::ZERO,
                members: members.map(<[Address]>::to_vec),
            }),
            ..Default::default()
        }
    }

    /// An EOA that delegated its code under EIP-7702 is still its key. Its delegate answering
    /// `entryPoint()` must not turn it into an account whose keys are unread.
    #[test]
    fn a_delegated_account_is_its_own_key_whatever_its_delegate_answers() {
        let delegated = AuthorityProbe {
            code: Code::Delegated(address!("7702cb554e6bfb442cb743a7df23154544a7176c")),
            entry_point: Some(B),
            ..Default::default()
        };
        let r = resolve_base(A, &graph(&[(A, delegated)]));
        assert_eq!(r.kind, AuthorityKind::Eoa);
        assert_eq!(r.compromise_depth, Some(1));
        assert_eq!(r.confidence, Confidence::High);
    }

    #[test]
    fn a_renounced_owner_is_a_root_with_no_key_to_count() {
        for sentinel in [
            Address::repeat_byte(0xff),
            address!("000000000000000000000000000000000000dEaD"),
            Address::with_last_byte(1),
        ] {
            let r = resolve_base(A, &graph(&[(A, ownable(sentinel)), (sentinel, eoa())]));
            assert_eq!(r.kind, AuthorityKind::Sentinel, "{sentinel}");
            assert_eq!(r.terminal, Node::base(sentinel));
            assert_eq!(r.compromise_depth, None, "no one holds this key");
            assert_eq!(r.depth_gap(), Some(DepthGap::NoKnownKey));
        }
    }

    #[test]
    fn only_addresses_nobody_can_hold_a_key_for_are_sentinels() {
        for sentinel in [
            address!("000000000000000000000000000000000000000a"),
            address!("0000000000000000000000000000000000000100"),
            address!("000000000000000000000000000000000000dEaD"),
        ] {
            assert!(is_sentinel(sentinel), "{sentinel}");
        }
        for key in [
            Address::ZERO,
            address!("000000000000000000000000000000000000000b"),
            address!("0000000000000000000000000000000000000101"),
            address!("21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc"),
        ] {
            assert!(!is_sentinel(key), "{key}");
        }
    }

    #[test]
    fn a_smart_account_whose_scheme_is_unread_is_a_medium_root_without_keys() {
        let unread = AuthorityProbe {
            entry_point: Some(B),
            ..Default::default()
        };
        let r = resolve_base(A, &graph(&[(C, ownable(A)), (A, unread)]));
        assert_eq!(r.kind, AuthorityKind::SmartAccount);
        assert_eq!(r.terminal, Node::base(A));
        assert_eq!(r.confidence, Confidence::Medium);
        assert_eq!(r.compromise_depth, None);
        assert_eq!(r.unresolved(), None, "the account is a real root");
        assert_eq!(r.depth_gap(), Some(DepthGap::AccountKeysUnread));
    }

    /// Any one signer of a MultiOwnable account signs alone, so it costs its cheapest signer.
    /// A passkey is one key with no address.
    #[test]
    fn a_multi_owner_account_costs_its_cheapest_signer() {
        let passkey = graph(&[
            (A, account(&[B], 1)),
            (B, safe(2, &[C, D])),
            (C, eoa()),
            (D, eoa()),
        ]);
        assert_eq!(resolve_base(A, &passkey).compromise_depth, Some(1));
        let only_a_safe = graph(&[
            (A, account(&[B], 0)),
            (B, safe(2, &[C, D])),
            (C, eoa()),
            (D, eoa()),
        ]);
        assert_eq!(resolve_base(A, &only_a_safe).compromise_depth, Some(2));
        let unknown_signer = graph(&[(A, account(&[B], 1))]);
        assert_eq!(
            resolve_base(A, &unknown_signer).compromise_depth,
            None,
            "one unknown signer poisons the count, as it does for a Safe"
        );
        assert_eq!(
            resolve_base(A, &unknown_signer).depth_gap(),
            Some(DepthGap::OwnersUnknown)
        );
    }

    #[test]
    fn an_upgrade_role_costs_its_cheapest_holder_and_an_unlisted_one_is_unread() {
        let listed = graph(&[
            (A, role_gated(Some(&[B, C]))),
            (B, safe(2, &[D, E])),
            (C, eoa()),
            (D, eoa()),
            (E, eoa()),
        ]);
        let r = resolve_base(A, &listed);
        assert_eq!(r.kind, AuthorityKind::RoleGated);
        assert_eq!(r.compromise_depth, Some(1));
        assert_eq!(
            r.confidence,
            Confidence::Medium,
            "the upgrade role is a convention"
        );
        let unlisted = resolve_base(A, &graph(&[(A, role_gated(None))]));
        assert_eq!(unlisted.compromise_depth, None);
        assert_eq!(unlisted.depth_gap(), Some(DepthGap::RolesUnread));
        let empty = resolve_base(A, &graph(&[(A, role_gated(Some(&[])))]));
        assert_eq!(
            empty.compromise_depth, None,
            "a role nobody holds has no key count"
        );
        assert_eq!(empty.depth_gap(), Some(DepthGap::NoKnownKey));
    }

    #[test]
    fn a_timelock_at_the_root_says_its_roles_are_unread() {
        let r = resolve_base(A, &graph(&[(A, timelock(172_800, None))]));
        assert_eq!(r.kind, AuthorityKind::Timelock);
        assert_eq!(r.depth_gap(), Some(DepthGap::RolesUnread));
    }

    #[test]
    fn signers_of_every_kind_are_edges_to_probe() {
        let e = edges(Node::base(A), &account(&[B], 2));
        assert_eq!(e, vec![Node::base(B)]);
        let e = edges(Node::ethereum(A), &role_gated(Some(&[C, D])));
        assert_eq!(e, vec![Node::ethereum(C), Node::ethereum(D)]);
    }

    /// Kinds recognized later rank after the ones the walk always relied on, so a contract
    /// that answers `owner()` is still walked through its owner.
    #[test]
    fn an_owned_contract_that_is_also_an_account_is_walked_through_its_owner() {
        let mut both = account(&[C], 0);
        both.owner = Some(B);
        let r = resolve_base(A, &graph(&[(A, both), (B, eoa()), (C, eoa())]));
        assert_eq!(r.terminal, Node::base(B));
        assert_eq!(r.kind, AuthorityKind::Eoa);
    }

    /// Running out of depth on the alias itself leaves the alias as the answer, and says so.
    #[test]
    fn a_walk_truncated_on_an_alias_names_the_alias_rather_than_calling_it_a_key() {
        let g = graph(&[
            (A, ownable(B)),
            (B, ownable(C)),
            (C, ownable(D)),
            (D, alias_of(E)),
        ]);
        let r = resolve_base(A, &g);
        assert!(r.truncated);
        assert_eq!(r.kind, AuthorityKind::L1Alias);
        assert_eq!(r.compromise_depth, None);
    }
}
