//! Turning an admin address into the governance structure behind it.
//!
//! Pure: everything here walks probes that were already collected from the chain. Keeping the
//! graph logic away from the I/O is what makes cycles, depth limits and the key arithmetic
//! testable without a network, and those are precisely the places edge cases hide.
//!
//! Every value a probe carries was returned by a contract a stranger deployed, so nothing in
//! here trusts its input structurally.

use crate::chain::Node;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How many links I will walk before giving up on a chain.
///
/// This is a safety control, not a budget. The edges come from contracts I do not control, so
/// an unbounded walk is an abort waiting to be deployed against me.
pub const MAX_DEPTH: usize = 4;

/// What an address turned out to be, as far as code goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
}

/// What one address answered when probed for the interfaces I recognize.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityProbe {
    pub code: Code,
    pub owners: Option<Vec<Address>>,
    pub threshold: Option<u32>,
    pub owner: Option<Address>,
    pub min_delay: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// No code on any chain that could act as it. Terminal, and one key away from control.
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
            Self::Unknown => "unknown",
        }
    }
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
}

/// Decide what an address is from what it answered, plus whether the answers conflict.
///
/// Precedence is documented rather than incidental, because probing is duck typing and duck
/// typing has no uniqueness guarantee. A contract answering both the Safe and the Timelock
/// probe resolves as a Safe, and says so by giving up High confidence.
fn classify(probe: &AuthorityProbe) -> (AuthorityKind, bool) {
    match probe.code {
        Code::Absent => return (AuthorityKind::Eoa, false),
        Code::L1Alias(_) => return (AuthorityKind::L1Alias, false),
        Code::Present => {}
    }
    let is_safe = probe.owners.is_some() && probe.threshold.is_some();
    let is_timelock = probe.min_delay.is_some();
    match (is_safe, is_timelock, probe.owner.is_some()) {
        (true, ambiguous, _) => (AuthorityKind::Safe, ambiguous),
        (false, true, _) => (AuthorityKind::Timelock, false),
        (false, false, true) => (AuthorityKind::Ownable, false),
        (false, false, false) => (AuthorityKind::Unknown, false),
    }
}

/// Where control passes from a node that is not terminal.
///
/// An owner lives on the chain it was read from. The alias is the one edge that crosses from
/// Base to Ethereum.
fn successor(node: Node, probe: &AuthorityProbe) -> Option<Node> {
    match probe.code {
        Code::L1Alias(l1) => Some(Node::ethereum(l1)),
        Code::Present | Code::Absent => probe.owner.map(|address| Node {
            chain: node.chain,
            address,
        }),
    }
}

/// Every node whose answers could change how `node` resolves: its Safe owners and its
/// successor. Collection uses this to decide what to probe next, so the scanner and the walk
/// can never disagree about which chain an owner lives on.
pub fn edges(node: Node, probe: &AuthorityProbe) -> Vec<Node> {
    let mut out: Vec<Node> = probe
        .owners
        .iter()
        .flatten()
        .map(|&address| Node {
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
            return stop;
        };

        let (kind, ambiguous) = classify(probe);
        stop.kind = kind;
        if ambiguous {
            stop.confidence = stop.confidence.min(Confidence::Medium);
        }

        match kind {
            AuthorityKind::Eoa | AuthorityKind::Safe => return stop,
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
    let cost = match classify(probe).0 {
        AuthorityKind::Eoa => Some(1),
        AuthorityKind::Safe => safe_keys_required(node, probe, probes, depth, seen),
        AuthorityKind::Ownable | AuthorityKind::Timelock | AuthorityKind::L1Alias => {
            keys_required(successor(node, probe)?, probes, depth + 1, seen)
        }
        AuthorityKind::Unknown => None,
    };
    seen.remove(&node);
    cost
}

/// An m-of-n Safe costs the sum of the **m cheapest** owners, not the first m in array order.
///
/// Taking array order is a plausible bug that produces plausible numbers, which is the worst
/// kind. If any owner's cost is unknown the whole total is unknown: an unrecognized owner
/// could be a single EOA, so reporting the cheapest m of the ones I do understand would
/// overstate how many keys an attacker actually needs.
fn safe_keys_required(
    node: Node,
    probe: &AuthorityProbe,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Node>,
) -> Option<u32> {
    let owners = probe.owners.as_ref()?;
    let threshold = probe.threshold? as usize;
    if threshold == 0 || threshold > owners.len() {
        return None;
    }
    let mut costs = owners
        .iter()
        .map(|&address| {
            let owner = Node {
                chain: node.chain,
                address,
            };
            keys_required(owner, probes, depth + 1, seen)
        })
        .collect::<Option<Vec<u32>>>()?;
    costs.sort_unstable();
    Some(
        costs
            .iter()
            .take(threshold)
            .fold(0u32, |acc, c| acc.saturating_add(*c)),
    )
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
