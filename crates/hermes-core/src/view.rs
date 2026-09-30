//! What Hermes knows about one address, assembled from the store once for every reader.
//!
//! The CLI prints these and the API serializes them, so the two cannot disagree about a proxy's
//! path or what a key reaches. Every view carries its scope: the index is a sample, the graph is
//! as of one observation, and "first seen" means first seen by Hermes.

use crate::authority::{AuthorityKind, AuthorityProbe, MAX_DEPTH, authority_kind, successor};
use crate::blast::{BlastRadius, blast_radius};
use crate::chain::{Chain, Node};
use crate::graph::{self, Field, MODEL_VERSION, Relation};
use crate::graph_store::{
    Membership, Observation, StoredEdge, StoredEvent, StoredNode, SubjectType,
};
use crate::store::{DISCOVERY_CAP, ProxyRecord, Store};
use alloy::primitives::Address;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// What every answer is an answer about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scope {
    /// `sample`: discovery reads fixed windows of history and admits a few addresses per
    /// implementation, beacon or admin. Every count is within it.
    pub index: &'static str,
    pub per_family: Option<i64>,
    /// The observation the newest data comes from.
    pub as_of: Option<Observation>,
    pub model_version: i64,
    pub first_seen_means: &'static str,
}

pub async fn scope(store: &Store) -> anyhow::Result<Scope> {
    Ok(Scope {
        index: "sample",
        per_family: store.cursor(DISCOVERY_CAP).await?,
        as_of: store.latest_observation().await?,
        model_version: MODEL_VERSION,
        first_seen_means: "first seen by hermes, not first set on chain",
    })
}

/// One address in a walk, and everything under it that decides who controls it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeNode {
    pub node: Node,
    /// How this node stands to the one above it: `admin`, `beacon`, `uups_self`, `owner`,
    /// `safe_owner` or `l1_alias`, or `start`.
    pub relation: String,
    /// What it answered, or `None` when it was never read.
    pub kind: Option<String>,
    /// `present`, `absent`, `l1_alias` or `delegated` (EIP-7702).
    pub code: Option<String>,
    pub threshold: Option<u32>,
    pub owner_count: Option<usize>,
    pub min_delay: Option<u64>,
    /// Signers of a smart account that are passkeys, which have no address to show.
    pub passkeys: Option<u32>,
    /// The upgrade role of an `AccessControl` contract, and whether its holders were listed.
    pub role: Option<alloy::primitives::B256>,
    pub members_listed: Option<bool>,
    pub children: Vec<TreeNode>,
    /// Why the tree stops here when the node is not terminal: `not_read`, `cycle` or `depth`.
    pub stopped: Option<&'static str>,
}

/// The walk under `start`: the successor of an owned contract or alias, every owner of a Safe.
pub fn tree(start: Node, relation: &str, probes: &HashMap<Node, AuthorityProbe>) -> TreeNode {
    grow(start, relation, probes, 0, &mut HashSet::new())
}

fn grow(
    node: Node,
    relation: &str,
    probes: &HashMap<Node, AuthorityProbe>,
    depth: usize,
    path: &mut HashSet<Node>,
) -> TreeNode {
    let mut t = grow_leaf(node, relation);
    let Some(probe) = probes.get(&node) else {
        t.stopped = Some("not_read");
        return t;
    };
    let kind = authority_kind(node, probe);
    t.kind = Some(kind.as_str().to_string());
    t.code = Some(graph::code_str(probe.code).to_string());
    t.threshold = probe.threshold;
    t.owner_count = probe.owners.as_ref().map(Vec::len);
    t.min_delay = probe.min_delay;
    t.passkeys = probe.account_owners.as_ref().map(|o| o.passkeys);
    t.role = probe.roles.as_ref().map(|r| r.role);
    t.members_listed = probe.roles.as_ref().map(|r| r.members.is_some());
    let signers = |list: Vec<Address>, r: Relation| -> Vec<(Node, &'static str)> {
        list.into_iter()
            .map(|address| {
                (
                    Node {
                        chain: node.chain,
                        address,
                    },
                    r.as_str(),
                )
            })
            .collect()
    };
    let next: Vec<(Node, &str)> = match kind {
        AuthorityKind::Safe => signers(
            probe.owners.clone().unwrap_or_default(),
            Relation::SafeOwner,
        ),
        AuthorityKind::SmartAccount => signers(
            probe
                .account_owners
                .as_ref()
                .map(|o| o.addresses.clone())
                .unwrap_or_default(),
            Relation::AccountOwner,
        ),
        AuthorityKind::RoleGated => signers(
            probe
                .roles
                .as_ref()
                .and_then(|r| r.members.clone())
                .unwrap_or_default(),
            Relation::RoleMember,
        ),
        AuthorityKind::Ownable | AuthorityKind::Timelock => successor(node, probe)
            .map(|n| (n, Relation::Owner.as_str()))
            .into_iter()
            .collect(),
        AuthorityKind::L1Alias => successor(node, probe)
            .map(|n| (n, Relation::L1Alias.as_str()))
            .into_iter()
            .collect(),
        AuthorityKind::Eoa | AuthorityKind::Sentinel | AuthorityKind::Unknown => Vec::new(),
    };
    if next.is_empty() {
        return t;
    }
    if depth >= MAX_DEPTH {
        t.stopped = Some("depth");
        return t;
    }
    if !path.insert(node) {
        t.stopped = Some("cycle");
        return t;
    }
    t.children = next
        .into_iter()
        .map(|(n, r)| {
            if path.contains(&n) {
                TreeNode {
                    stopped: Some("cycle"),
                    ..grow_leaf(n, r)
                }
            } else {
                grow(n, r, probes, depth + 1, path)
            }
        })
        .collect();
    path.remove(&node);
    t
}

fn grow_leaf(node: Node, relation: &str) -> TreeNode {
    TreeNode {
        node,
        relation: relation.to_string(),
        kind: None,
        code: None,
        threshold: None,
        owner_count: None,
        min_delay: None,
        passkeys: None,
        role: None,
        members_listed: None,
        children: Vec::new(),
        stopped: None,
    }
}

/// A stored proxy's resolution, walked again over the stored graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Replayed {
    pub root: Node,
    pub kind: String,
    pub keys_required: Option<u32>,
    pub timelock_seconds: u64,
    pub confidence: String,
    pub path: Vec<Node>,
    /// Whether this agrees with the published row. It can differ when a node on the path was
    /// re-read by a later batch, or when the resolver changed since the row was written.
    pub matches_row: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProxyView {
    pub proxy: ProxyRecord,
    /// Where the upgrade walk starts: `admin_slot`, `beacon` or `uups_owner`.
    pub upgrade_entry: Option<&'static str>,
    pub tree: Option<TreeNode>,
    pub replayed: Option<Replayed>,
    pub scope: Scope,
}

fn matches(row: &ProxyRecord, r: &crate::authority::Resolution) -> bool {
    row.terminal_authority
        .as_deref()
        .is_some_and(|a| a.eq_ignore_ascii_case(&r.terminal.address.to_checksum(None)))
        && row.terminal_chain.as_deref() == Some(r.terminal.chain.as_str())
        && row.compromise_depth == r.compromise_depth.map(i64::from)
}

pub async fn proxy_view(store: &Store, address: &str) -> anyhow::Result<Option<ProxyView>> {
    let Some(proxy) = store.get_proxy(address).await? else {
        return Ok(None);
    };
    let entry = graph::upgrade_entry_of(&proxy);
    let mut view = ProxyView {
        upgrade_entry: entry.map(|e| e.as_str()),
        tree: None,
        replayed: None,
        scope: scope(store).await?,
        proxy,
    };
    let Some(entry) = entry else {
        return Ok(Some(view));
    };
    let probes = store.probes_from(&[entry.start()]).await?;
    let relation = match entry {
        crate::upgrade::UpgradeEntry::AdminSlot(_) => "admin",
        crate::upgrade::UpgradeEntry::Beacon(_) => "beacon",
        crate::upgrade::UpgradeEntry::UupsOwner(_) => "uups_self",
    };
    view.tree = Some(tree(entry.start(), relation, &probes));
    if let Some((_, r)) = graph::replay(&view.proxy, &probes)
        && r.unresolved().is_none()
    {
        view.replayed = Some(Replayed {
            root: r.terminal,
            kind: r.kind.as_str().to_string(),
            keys_required: r.compromise_depth,
            timelock_seconds: r.timelock_seconds,
            confidence: r.confidence.as_str().to_string(),
            matches_row: matches(&view.proxy, &r),
            path: r.path,
        });
    }
    Ok(Some(view))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeView {
    pub node: StoredNode,
    pub tree: TreeNode,
    /// Contracts this node is the admin, beacon, owner or L1 contract of, now.
    pub controls_directly: Vec<StoredEdge>,
    /// Safes this address is an owner of, on either chain.
    pub owner_of: Vec<Membership>,
    /// Edges into or out of this node that have since closed.
    pub history: Vec<StoredEdge>,
    pub blast_radius: BlastRadius,
    /// How many proxies discovery has seen using this address as their beacon or admin,
    /// admitted to the index or not. A floor: discovery reads windows, not all of history.
    pub sighted_as_beacon: i64,
    pub sighted_as_admin: i64,
    pub scope: Scope,
}

pub async fn node_view(store: &Store, node: Node) -> anyhow::Result<Option<NodeView>> {
    let Some(stored) = store.node(node).await? else {
        return Ok(None);
    };
    let probes = store.all_probes().await?;
    let entries = store.upgrade_entries().await?;
    let into = store.edges_to(node, true).await?;
    let out = store.edges_from(node, true).await?;
    let (open_in, closed_in): (Vec<_>, Vec<_>) =
        into.into_iter().partition(|e| e.closed_at.is_none());
    let mut history: Vec<StoredEdge> = closed_in;
    history.extend(out.into_iter().filter(|e| e.closed_at.is_some()));
    history.sort_by_key(|e| (e.closed_at, e.first_seen_at));
    Ok(Some(NodeView {
        tree: tree(node, "start", &probes),
        controls_directly: open_in
            .into_iter()
            .filter(|e| e.relation.is_control())
            .collect(),
        owner_of: store.memberships(&node.address.to_checksum(None)).await?,
        history,
        blast_radius: blast_radius(node, &entries, &probes),
        sighted_as_beacon: store
            .family_size(&format!("beacon:{:#x}", node.address))
            .await?,
        sighted_as_admin: store
            .family_size(&format!("admin:{:#x}", node.address))
            .await?,
        scope: scope(store).await?,
        node: stored,
    }))
}

/// Every stored node at `address`, preferring Base, for commands that take an address alone.
pub async fn pick_node(
    store: &Store,
    address: &str,
    chain: Option<Chain>,
) -> anyhow::Result<Option<Node>> {
    let Ok(address) = address.parse::<Address>() else {
        return Ok(None);
    };
    let chains = match chain {
        Some(c) => vec![c],
        None => vec![Chain::Base, Chain::Ethereum],
    };
    for chain in chains {
        let node = Node { chain, address };
        if store.node(node).await?.is_some() {
            return Ok(Some(node));
        }
    }
    Ok(None)
}

/// A key: an address seen as a signer, on whichever chains it was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyView {
    pub address: String,
    pub nodes: Vec<StoredNode>,
    pub owner_of: Vec<Membership>,
    /// From the Base node when there is one, else the Ethereum one. A key's compromise takes
    /// its address on both chains, so either gives the same answer.
    pub blast_radius: Option<BlastRadius>,
    pub scope: Scope,
}

pub async fn key_view(store: &Store, address: &str) -> anyhow::Result<Option<KeyView>> {
    let Ok(parsed) = address.parse::<Address>() else {
        return Ok(None);
    };
    let checksummed = parsed.to_checksum(None);
    let nodes = store.nodes_at(&checksummed).await?;
    let owner_of = store.memberships(&checksummed).await?;
    if nodes.is_empty() && owner_of.is_empty() {
        return Ok(None);
    }
    let blast = match nodes.first() {
        Some(n) => {
            let probes = store.all_probes().await?;
            let entries = store.upgrade_entries().await?;
            Some(blast_radius(n.node, &entries, &probes))
        }
        None => None,
    };
    Ok(Some(KeyView {
        address: checksummed,
        nodes,
        owner_of,
        blast_radius: blast,
        scope: scope(store).await?,
    }))
}

fn value(field: Field, v: &Option<String>) -> String {
    match (field, v) {
        (_, None) => "none".into(),
        (Field::TimelockSeconds | Field::TimelockDelay, Some(s)) => s
            .parse::<u64>()
            .map_or_else(|_| s.clone(), crate::time::duration),
        (_, Some(s)) => s.clone(),
    }
}

/// One change as a line of text: what moved, from what to what, and which way a number went.
/// Direction words are arithmetic. Nothing here says whether a change is good.
pub fn event_line(e: &StoredEvent) -> String {
    let f = e.change.field;
    let what = match f {
        Field::SafeOwnerAdded => format!("signer added: {}", value(f, &e.change.new)),
        Field::SafeOwnerRemoved => format!("signer removed: {}", value(f, &e.change.old)),
        _ => format!(
            "{} {} → {}",
            f.as_str().replace('_', " "),
            value(f, &e.change.old),
            value(f, &e.change.new)
        ),
    };
    let direction = e.direction.map_or(String::new(), |d| format!(" ({d})"));
    let subject = match e.subject_type {
        SubjectType::Proxy => "proxy",
        SubjectType::Node => "authority",
    };
    format!(
        "{subject} {}: {what}{direction}",
        e.subject.address.to_checksum(None)
    )
}

/// Where a change happened, in blocks: pinned when bisection has run, bracketed until then.
pub fn event_when(e: &StoredEvent) -> String {
    let chain = e.subject.chain.as_str();
    match (e.at_block, e.last_old_block, e.first_new_block) {
        (Some(at), _, _) => format!("at {chain} block {at}"),
        (None, Some(a), Some(b)) => format!("between {chain} blocks {a} and {b}"),
        (None, None, Some(b)) => format!("by {chain} block {b}"),
        _ => "at an unpinned block".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::Code;
    use alloy::primitives::address;

    const A: Address = address!("00000000000000000000000000000000000000a1");
    const B: Address = address!("00000000000000000000000000000000000000b2");
    const C: Address = address!("00000000000000000000000000000000000000c3");

    fn key() -> AuthorityProbe {
        AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        }
    }

    #[test]
    fn a_tree_follows_owners_of_a_safe_and_the_owner_of_anything_else() {
        let probes = HashMap::from([
            (
                Node::base(A),
                AuthorityProbe {
                    owner: Some(B),
                    ..Default::default()
                },
            ),
            (
                Node::base(B),
                AuthorityProbe {
                    owners: Some(vec![C]),
                    threshold: Some(1),
                    // A Safe that also answers owner(): the walk ignores it, and so does the tree.
                    owner: Some(A),
                    ..Default::default()
                },
            ),
            (Node::base(C), key()),
        ]);
        let t = tree(Node::base(A), "admin", &probes);
        assert_eq!(t.kind.as_deref(), Some("ownable"));
        assert_eq!(t.children.len(), 1);
        let safe = &t.children[0];
        assert_eq!(safe.relation, "owner");
        assert_eq!(safe.kind.as_deref(), Some("safe"));
        assert_eq!(safe.children[0].relation, "safe_owner");
        assert_eq!(safe.children[0].kind.as_deref(), Some("eoa"));
    }

    #[test]
    fn a_change_reads_as_a_fact_with_its_direction_and_its_blocks() {
        let e = StoredEvent {
            id: 1,
            observation_id: 2,
            subject_type: SubjectType::Node,
            subject: Node::base(A),
            change: crate::graph::Change {
                field: Field::TimelockDelay,
                old: Some("172800".into()),
                new: Some("0".into()),
            },
            direction: Some("fell"),
            last_old_block: Some(10),
            first_new_block: Some(20),
            at_block: None,
            cause: crate::graph::Cause::Chain,
            model_version: 1,
            observed_at: 0,
        };
        assert_eq!(
            event_line(&e),
            format!(
                "authority {}: timelock delay 48h → none (fell)",
                A.to_checksum(None)
            )
        );
        assert_eq!(event_when(&e), "between base blocks 10 and 20");
        let pinned = StoredEvent {
            at_block: Some(15),
            ..e
        };
        assert_eq!(event_when(&pinned), "at base block 15");
    }

    #[test]
    fn a_tree_says_where_and_why_it_stops() {
        let unread = tree(
            Node::base(A),
            "admin",
            &HashMap::from([(
                Node::base(A),
                AuthorityProbe {
                    owner: Some(B),
                    ..Default::default()
                },
            )]),
        );
        assert_eq!(unread.children[0].stopped, Some("not_read"));
        let cycle = tree(
            Node::base(A),
            "admin",
            &HashMap::from([(
                Node::base(A),
                AuthorityProbe {
                    owner: Some(A),
                    ..Default::default()
                },
            )]),
        );
        assert_eq!(cycle.children[0].stopped, Some("cycle"));
    }
}
