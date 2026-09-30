//! What an authority reaches, and what falls to it alone.
//!
//! "What does this key control" hides two questions. A key that owns a ProxyAdmin controls every
//! proxy under it. A key that is one of five owners of a 2-of-5 Safe controls nothing by itself,
//! and still takes part in everything the Safe controls. Counting the second as the first
//! overstates one compromise; leaving it out hides the dependency. So they are kept apart, and
//! a third list holds the walks that cannot say which. Nothing here adds them up.
//!
//! Pure: walks stored probes, reads nothing.

use crate::authority::{
    AuthorityKind, AuthorityProbe, Code, MAX_DEPTH, authority_kind, edges, successor,
};
use crate::chain::{Chain, Node};
use crate::store::ProxyRecord;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Whether holding every node in `compromised` is enough to exercise the authority at `node`.
///
/// `None` when that cannot be established: an unrecognized node, an unread probe, a threshold
/// that lies, a cycle, or a walk past the depth cap. Never `Some(false)` for any of those,
/// because "does not fall" is a safety claim and the most comfortable wrong answer available.
/// An unknown Safe owner only makes the answer unknown when it could tip the threshold.
pub fn falls(
    node: Node,
    compromised: &HashSet<Node>,
    probes: &HashMap<Node, AuthorityProbe>,
) -> Option<bool> {
    falls_at(node, compromised, probes, 0, &mut HashSet::new())
}

fn falls_at(
    node: Node,
    compromised: &HashSet<Node>,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Node>,
) -> Option<bool> {
    if compromised.contains(&node) {
        return Some(true);
    }
    if depth > MAX_DEPTH || !seen.insert(node) {
        return None;
    }
    let answer = probes
        .get(&node)
        .and_then(|probe| match authority_kind(probe) {
            AuthorityKind::Eoa => Some(false),
            AuthorityKind::Safe => safe_falls(node, probe, compromised, probes, depth, seen),
            AuthorityKind::Ownable | AuthorityKind::Timelock | AuthorityKind::L1Alias => falls_at(
                successor(node, probe)?,
                compromised,
                probes,
                depth + 1,
                seen,
            ),
            AuthorityKind::Unknown => None,
        });
    seen.remove(&node);
    answer
}

fn safe_falls(
    node: Node,
    probe: &AuthorityProbe,
    compromised: &HashSet<Node>,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Node>,
) -> Option<bool> {
    let owners = probe.owners.as_ref()?;
    let threshold = probe.threshold? as usize;
    if threshold == 0 || threshold > owners.len() {
        return None;
    }
    let (mut taken, mut unknown) = (0usize, 0usize);
    for &address in owners {
        let owner = Node {
            chain: node.chain,
            address,
        };
        match falls_at(owner, compromised, probes, depth + 1, seen) {
            Some(true) => taken += 1,
            Some(false) => {}
            None => unknown += 1,
        }
    }
    if taken >= threshold {
        Some(true)
    } else if taken + unknown >= threshold {
        None
    } else {
        Some(false)
    }
}

/// Every node a walk from `start`, or the key count under it, could read.
pub fn reachable(start: Node, probes: &HashMap<Node, AuthorityProbe>) -> HashSet<Node> {
    let mut seen = HashSet::from([start]);
    let mut frontier = vec![start];
    for _ in 0..=MAX_DEPTH {
        frontier = frontier
            .iter()
            .filter_map(|n| probes.get(n).map(|p| edges(*n, p)))
            .flatten()
            .filter(|n| seen.insert(*n))
            .collect();
    }
    seen
}

fn other(chain: Chain) -> Chain {
    match chain {
        Chain::Base => Chain::Ethereum,
        Chain::Ethereum => Chain::Base,
    }
}

/// What one compromise of `target` hands over.
///
/// A key is the same key on every chain: the private key behind an EOA signs for its address on
/// Base and on Ethereum alike, so taking one takes both, unless the other chain has shown code at
/// that address. A contract is only itself.
pub fn compromise_of(target: Node, probes: &HashMap<Node, AuthorityProbe>) -> HashSet<Node> {
    let mut taken = HashSet::from([target]);
    let is_key = probes.get(&target).is_some_and(|p| p.code == Code::Absent);
    let twin = Node {
        chain: other(target.chain),
        address: target.address,
    };
    if is_key && probes.get(&twin).is_none_or(|p| p.code == Code::Absent) {
        taken.insert(twin);
    }
    taken
}

/// One proxy an authority reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reached {
    pub proxy: String,
    pub label: Option<String>,
    pub kind: String,
    /// Where the proxy's upgrade walk starts.
    pub entry: Node,
    pub upgrade_path: Option<String>,
    /// The proxy's published root and key count, for context.
    pub root: Option<Node>,
    pub keys_required: Option<i64>,
}

/// Every indexed proxy whose upgrade walk passes through a compromise, split by whether the
/// compromise alone is enough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlastRadius {
    pub target: Node,
    /// What the compromise hands over: the target, and its twin on the other chain for a key.
    pub compromised: Vec<Node>,
    /// Proxies this compromise could upgrade on its own.
    pub controls: Vec<Reached>,
    /// Proxies whose authority it takes part in, without being enough alone.
    pub participates: Vec<Reached>,
    /// Proxies it reaches where the walk cannot say whether it is enough alone.
    pub undetermined: Vec<Reached>,
}

fn reached(row: &ProxyRecord, entry: Node) -> Reached {
    let root = row
        .terminal_authority
        .as_deref()
        .and_then(|a| a.parse().ok())
        .map(|address| Node {
            chain: row
                .terminal_chain
                .as_deref()
                .and_then(Chain::parse)
                .unwrap_or(Chain::Base),
            address,
        });
    Reached {
        proxy: row.address.clone(),
        label: row.label.clone(),
        kind: row.kind.clone(),
        entry,
        upgrade_path: row.upgrade_path.clone(),
        root,
        keys_required: row.compromise_depth,
    }
}

/// Which of `entries` a compromise of `target` reaches, and on what footing.
///
/// Counts are within the index, which is a sample. A proxy the index does not hold is not in
/// any list, however surely the target controls it.
pub fn blast_radius(
    target: Node,
    entries: &[(ProxyRecord, Option<Node>)],
    probes: &HashMap<Node, AuthorityProbe>,
) -> BlastRadius {
    let taken = compromise_of(target, probes);
    let mut verdicts: HashMap<Node, Option<Option<bool>>> = HashMap::new();
    let mut radius = BlastRadius {
        target,
        compromised: {
            let mut v: Vec<Node> = taken.iter().copied().collect();
            v.sort();
            v
        },
        controls: Vec::new(),
        participates: Vec::new(),
        undetermined: Vec::new(),
    };
    for (row, entry) in entries {
        let Some(entry) = *entry else { continue };
        let verdict = *verdicts.entry(entry).or_insert_with(|| {
            let reach = reachable(entry, probes);
            taken
                .iter()
                .any(|n| reach.contains(n))
                .then(|| falls(entry, &taken, probes))
        });
        let list = match verdict {
            None => continue,
            Some(Some(true)) => &mut radius.controls,
            Some(Some(false)) => &mut radius.participates,
            Some(None) => &mut radius.undetermined,
        };
        list.push(reached(row, entry));
    }
    radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, address};

    const A: Address = address!("00000000000000000000000000000000000000a1");
    const B: Address = address!("00000000000000000000000000000000000000b2");
    const C: Address = address!("00000000000000000000000000000000000000c3");
    const D: Address = address!("00000000000000000000000000000000000000d4");
    const E: Address = address!("00000000000000000000000000000000000000e5");

    fn key() -> AuthorityProbe {
        AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        }
    }

    fn owned_by(owner: Address) -> AuthorityProbe {
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

    fn graph(entries: &[(Address, AuthorityProbe)]) -> HashMap<Node, AuthorityProbe> {
        entries
            .iter()
            .map(|(a, p)| (Node::base(*a), p.clone()))
            .collect()
    }

    fn base(nodes: &[Address]) -> HashSet<Node> {
        nodes.iter().map(|a| Node::base(*a)).collect()
    }

    #[test]
    fn a_key_takes_what_it_owns_and_nothing_it_does_not() {
        let g = graph(&[(A, owned_by(B)), (B, key()), (C, key())]);
        assert_eq!(falls(Node::base(A), &base(&[B]), &g), Some(true));
        assert_eq!(falls(Node::base(A), &base(&[C]), &g), Some(false));
    }

    #[test]
    fn a_safe_falls_at_its_threshold_and_not_one_key_before() {
        let g = graph(&[(A, safe(2, &[B, C, D])), (B, key()), (C, key()), (D, key())]);
        assert_eq!(falls(Node::base(A), &base(&[B]), &g), Some(false));
        assert_eq!(falls(Node::base(A), &base(&[B, D]), &g), Some(true));
    }

    /// An unknown owner matters only if it could tip the threshold. Two known keys taken of a
    /// 2-of-3 is enough whatever the third is; one taken is unknown, not "safe".
    #[test]
    fn an_unknown_owner_makes_the_answer_unknown_only_when_it_could_decide_it() {
        let g = graph(&[(A, safe(2, &[B, C, D])), (B, key()), (C, key())]);
        assert_eq!(falls(Node::base(A), &base(&[B, C]), &g), Some(true));
        assert_eq!(falls(Node::base(A), &base(&[B]), &g), None);
        let two_of_three_with_one_unknown =
            graph(&[(A, safe(3, &[B, C, D])), (B, key()), (C, key())]);
        assert_eq!(
            falls(
                Node::base(A),
                &HashSet::new(),
                &two_of_three_with_one_unknown
            ),
            Some(false),
            "no owner taken cannot reach three, whatever the unknown one is"
        );
    }

    #[test]
    fn an_unreadable_walk_never_answers_does_not_fall() {
        let unrecognized = graph(&[(A, owned_by(B)), (B, AuthorityProbe::default())]);
        assert_eq!(falls(Node::base(A), &base(&[C]), &unrecognized), None);
        let unread = graph(&[(A, owned_by(B))]);
        assert_eq!(falls(Node::base(A), &base(&[C]), &unread), None);
        let cycle = graph(&[(A, owned_by(A))]);
        assert_eq!(falls(Node::base(A), &base(&[C]), &cycle), None);
        let lying = graph(&[(A, safe(5, &[B])), (B, key())]);
        assert_eq!(falls(Node::base(A), &base(&[B]), &lying), None);
    }

    /// The predeploy authority as read on 2026-09-19: a 2-of-2 over a 3-of-6 and an 8-of-11.
    /// Eleven keys take it, and ten in the wrong places do not.
    #[test]
    fn the_predeploy_authority_falls_to_eleven_keys_in_the_right_places() {
        let coordinator: Vec<Address> = (0x10..0x16).map(Address::with_last_byte).collect();
        let council: Vec<Address> = (0x20..0x2b).map(Address::with_last_byte).collect();
        let (l1_safe, coord, cncl) = (A, B, C);
        let mut g: HashMap<Node, AuthorityProbe> = HashMap::from([
            (Node::base(D), owned_by(E)),
            (
                Node::base(E),
                AuthorityProbe {
                    code: Code::L1Alias(l1_safe),
                    ..Default::default()
                },
            ),
            (Node::ethereum(l1_safe), safe(2, &[coord, cncl])),
            (Node::ethereum(coord), safe(3, &coordinator)),
            (Node::ethereum(cncl), safe(8, &council)),
        ]);
        for k in coordinator.iter().chain(&council) {
            g.insert(Node::ethereum(*k), key());
        }
        let eth = |ks: &[Address]| {
            ks.iter()
                .map(|a| Node::ethereum(*a))
                .collect::<HashSet<_>>()
        };
        let eleven: Vec<Address> = coordinator[..3]
            .iter()
            .chain(&council[..8])
            .copied()
            .collect();
        assert_eq!(falls(Node::base(D), &eth(&eleven), &g), Some(true));
        let short_on_council: Vec<Address> = coordinator[..3]
            .iter()
            .chain(&council[..7])
            .copied()
            .collect();
        assert_eq!(
            falls(Node::base(D), &eth(&short_on_council), &g),
            Some(false)
        );
        let all_coordinator: Vec<Address> =
            coordinator.iter().chain(&council[..5]).copied().collect();
        assert_eq!(
            falls(Node::base(D), &eth(&all_coordinator), &g),
            Some(false),
            "eleven keys in the wrong places are not eleven keys"
        );
    }

    #[test]
    fn a_key_is_taken_on_both_chains_and_a_contract_only_on_its_own() {
        let mut g = graph(&[(A, key()), (B, owned_by(C))]);
        assert_eq!(
            compromise_of(Node::base(A), &g),
            HashSet::from([Node::base(A), Node::ethereum(A)])
        );
        assert_eq!(
            compromise_of(Node::base(B), &g),
            HashSet::from([Node::base(B)])
        );
        g.insert(Node::ethereum(A), owned_by(C));
        assert_eq!(
            compromise_of(Node::base(A), &g),
            HashSet::from([Node::base(A)]),
            "code at the address on Ethereum means the twin is someone else's contract"
        );
    }

    fn row(address: Address, entry_kind: &str) -> ProxyRecord {
        ProxyRecord {
            address: address.to_checksum(None),
            kind: entry_kind.into(),
            ..Default::default()
        }
    }

    /// The Sep 29 shape: a key that was the sole owner of a beacon, and is now one of five
    /// owners of the Safe that owns it, went from controlling to participating.
    #[test]
    fn one_owner_of_a_safe_participates_and_the_safe_controls() {
        let proxy = Address::with_last_byte(0x99);
        let g = graph(&[
            (A, owned_by(B)),
            (B, safe(2, &[C, D, E])),
            (C, key()),
            (D, key()),
            (E, key()),
        ]);
        let entries = vec![(row(proxy, "beacon"), Some(Node::base(A)))];
        let by_key = blast_radius(Node::base(C), &entries, &g);
        assert!(by_key.controls.is_empty());
        assert_eq!(by_key.participates.len(), 1);
        assert_eq!(by_key.compromised.len(), 2, "the key on both chains");
        let by_safe = blast_radius(Node::base(B), &entries, &g);
        assert_eq!(by_safe.controls.len(), 1);
        let by_beacon = blast_radius(Node::base(A), &entries, &g);
        assert_eq!(by_beacon.controls.len(), 1, "the entry itself controls");
    }

    #[test]
    fn a_proxy_the_target_never_reaches_is_in_no_list() {
        let g = graph(&[(A, owned_by(B)), (B, key()), (C, key())]);
        let entries = vec![
            (row(D, "transparent"), Some(Node::base(A))),
            (row(E, "uups"), None),
        ];
        let radius = blast_radius(Node::base(C), &entries, &g);
        assert!(radius.controls.is_empty() && radius.participates.is_empty());
        assert!(radius.undetermined.is_empty());
    }

    #[test]
    fn a_reached_proxy_whose_walk_is_unreadable_is_undetermined() {
        let g = graph(&[(A, safe(2, &[B, C])), (B, key())]);
        let entries = vec![(row(D, "transparent"), Some(Node::base(A)))];
        let radius = blast_radius(Node::base(B), &entries, &g);
        assert_eq!(radius.undetermined.len(), 1);
    }
}
