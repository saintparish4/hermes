//! The kept authority graph: writing what each scan observed, and reading it back.
//!
//! Everything a batch establishes is written in one transaction: the proxy rows, the edges their
//! slots and probes show, the probes themselves, and the events that differ from what was stored
//! before. A batch the canary refuses writes none of it, and a row the never-overwrite-live-code
//! guard refuses contributes no edges and no events, so a lying node cannot close an edge any
//! more than it can blank a proxy.

use crate::authority::{AuthorityProbe, MAX_DEPTH, authority_kind, edges};
use crate::chain::{Chain, Node};
use crate::graph::{self, Cause, Change, EdgeSets, Field, MODEL_VERSION, Relation};
use crate::store::{
    ProxyRecord, Store, checksum, covered_kinds, proxy_columns, proxy_on, row_to_record, upsert_row,
};
use alloy::primitives::Address;
use serde::Serialize;
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeSet, HashMap, HashSet};

macro_rules! edge_columns {
    () => {
        "from_chain, from_address, relation, to_chain, to_address, first_seen_block, \
         first_seen_at, last_seen_block, last_seen_at, closed_block, closed_at"
    };
}

macro_rules! event_columns {
    () => {
        "id, observation_id, subject_type, subject_chain, subject_address, field, old_value, \
         new_value, last_old_block, first_new_block, at_block, cause, model_version, observed_at"
    };
}

/// The optional narrowing every event query shares. `?6` includes reinterpretations; `?7` is
/// the one cause shown when it does not.
macro_rules! event_filter {
    () => {
        "(?1 IS NULL OR subject_address = ?1) AND (?2 IS NULL OR subject_chain = ?2) \
         AND (?3 IS NULL OR observed_at >= ?3) \
         AND (?4 IS NULL OR first_new_block > ?4) AND (?5 IS NULL OR first_new_block <= ?5) \
         AND (?6 OR cause = ?7)"
    };
}

/// One scan run: when it happened, and the blocks every read in it was pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Observation {
    pub id: i64,
    pub base_block: Option<i64>,
    pub ethereum_block: Option<i64>,
    pub observed_at: i64,
    pub model_version: i64,
}

impl Observation {
    pub fn block(&self, chain: Chain) -> Option<i64> {
        match chain {
            Chain::Base => self.base_block,
            Chain::Ethereum => self.ethereum_block,
        }
    }
}

/// What one batch wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    pub rows: u64,
    pub nodes: usize,
    pub events: usize,
}

/// A probed address as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredNode {
    pub node: Node,
    pub kind: String,
    pub code: String,
    pub threshold: Option<i64>,
    pub owner_count: Option<i64>,
    pub min_delay: Option<i64>,
    #[serde(skip)]
    pub probe: AuthorityProbe,
    pub model_version: i64,
    /// First seen *by Hermes*. The chain may have held this state long before.
    pub first_seen_block: Option<i64>,
    pub first_seen_at: i64,
    pub last_seen_block: Option<i64>,
    pub last_seen_at: i64,
}

/// One stretch of time an edge was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEdge {
    pub from: Node,
    pub relation: Relation,
    pub to: Node,
    pub first_seen_block: Option<i64>,
    pub first_seen_at: i64,
    pub last_seen_block: Option<i64>,
    pub last_seen_at: i64,
    /// Set once a settled observation no longer showed the edge.
    pub closed_block: Option<i64>,
    pub closed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectType {
    Proxy,
    Node,
}

impl SubjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Node => "node",
        }
    }
}

/// One change, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEvent {
    pub id: i64,
    pub observation_id: i64,
    pub subject_type: SubjectType,
    pub subject: Node,
    #[serde(flatten)]
    pub change: Change,
    /// `rose` or `fell` for a number known on both sides.
    pub direction: Option<&'static str>,
    /// The last block the old value was seen at, when the old observation was pinned.
    pub last_old_block: Option<i64>,
    /// The first block the new value was seen at.
    pub first_new_block: Option<i64>,
    /// The first block the new value holds at, once bisection has found it.
    pub at_block: Option<i64>,
    pub cause: Cause,
    pub model_version: i64,
    pub observed_at: i64,
}

/// Which events to read back. Every field narrows; the default is every chain event.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub address: Option<String>,
    pub chain: Option<Chain>,
    /// Unix seconds: only events observed at or after this.
    pub since: Option<i64>,
    /// Base blocks: only events whose new value was first seen inside `(after_block, to_block]`.
    pub after_block: Option<i64>,
    pub to_block: Option<i64>,
    pub include_reinterpretations: bool,
    pub limit: Option<i64>,
    /// Oldest first rather than newest first.
    pub ascending: bool,
}

/// One Safe an address is an owner of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Membership {
    pub safe: Node,
    /// The account that sits in the Safe: the queried address on the Safe's chain.
    pub member: Node,
    pub member_kind: Option<String>,
    pub threshold: Option<i64>,
    pub owner_count: Option<i64>,
}

/// Two Safes that share owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedSigners {
    pub a: Node,
    pub b: Node,
    pub shared: i64,
    pub keys: Vec<String>,
}

struct PendingEvent {
    subject_type: SubjectType,
    subject: Node,
    change: Change,
    cause: Cause,
    last_old_block: Option<i64>,
    first_new_block: Option<i64>,
}

fn node_from(chain: &str, address: &str) -> Option<Node> {
    Some(Node {
        chain: Chain::parse(chain)?,
        address: address.parse::<Address>().ok()?,
    })
}

impl Store {
    /// Record that a scan run is starting at these blocks.
    pub async fn begin_observation(
        &self,
        base_block: Option<i64>,
        ethereum_block: Option<i64>,
        observed_at: i64,
    ) -> anyhow::Result<Observation> {
        let id = sqlx::query(
            "INSERT INTO observation (base_block, ethereum_block, observed_at, model_version) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(base_block)
        .bind(ethereum_block)
        .bind(observed_at)
        .bind(MODEL_VERSION)
        .execute(self.pool())
        .await?
        .last_insert_rowid();
        Ok(Observation {
            id,
            base_block,
            ethereum_block,
            observed_at,
            model_version: MODEL_VERSION,
        })
    }

    /// The most recent scan run, or `None` before the first one.
    pub async fn latest_observation(&self) -> anyhow::Result<Option<Observation>> {
        Ok(sqlx::query(
            "SELECT id, base_block, ethereum_block, observed_at, model_version \
             FROM observation ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await?
        .map(|r| Observation {
            id: r.get("id"),
            base_block: r.get("base_block"),
            ethereum_block: r.get("ethereum_block"),
            observed_at: r.get("observed_at"),
            model_version: r.get("model_version"),
        }))
    }

    /// Write one batch of what a scan observed, in one transaction.
    ///
    /// `probes` are settled probes only. An address the scanner could not read is absent from
    /// the map, so nothing about it is written: its edges stay open and its last-seen block
    /// does not move, because an outage has told me nothing.
    pub async fn write_batch(
        &self,
        obs: &Observation,
        records: &[ProxyRecord],
        probes: &HashMap<Node, AuthorityProbe>,
    ) -> anyhow::Result<Written> {
        let mut tx = self.pool().begin().await?;
        let mut written = Written::default();
        let mut events = Vec::new();
        for r in records {
            if let Some(changes) = write_proxy(&mut tx, obs, r).await? {
                written.rows += 1;
                events.extend(changes);
            }
        }
        let mut nodes: Vec<_> = probes.iter().collect();
        nodes.sort_by_key(|(n, _)| **n);
        for (node, probe) in nodes {
            events.extend(write_node(&mut tx, obs, *node, probe).await?);
            written.nodes += 1;
        }
        for e in &events {
            insert_event(&mut tx, obs, e).await?;
        }
        written.events = events.len();
        tx.commit().await?;
        Ok(written)
    }

    pub async fn node(&self, node: Node) -> anyhow::Result<Option<StoredNode>> {
        let mut conn = self.pool().acquire().await?;
        Ok(node_on(&mut conn, node).await?)
    }

    /// The address as a node on every chain the store has it on.
    pub async fn nodes_at(&self, address: &str) -> anyhow::Result<Vec<StoredNode>> {
        let mut out = Vec::new();
        let Ok(address) = address.parse::<Address>() else {
            return Ok(out);
        };
        for chain in [Chain::Base, Chain::Ethereum] {
            out.extend(self.node(Node { chain, address }).await?);
        }
        Ok(out)
    }

    /// Every stored probe reachable from `starts`, gathered the way the scanner gathers them.
    /// Walking these with `resolve` answers offline what the scan answered online.
    pub async fn probes_from(
        &self,
        starts: &[Node],
    ) -> anyhow::Result<HashMap<Node, AuthorityProbe>> {
        let mut conn = self.pool().acquire().await?;
        let mut probes = HashMap::new();
        let mut seen: HashSet<Node> = HashSet::new();
        let mut frontier: Vec<Node> = starts.iter().copied().filter(|n| seen.insert(*n)).collect();
        for _ in 0..=MAX_DEPTH {
            let mut next = Vec::new();
            for n in frontier {
                if let Some(stored) = node_on(&mut conn, n).await? {
                    next.extend(
                        edges(n, &stored.probe)
                            .into_iter()
                            .filter(|e| seen.insert(*e)),
                    );
                    probes.insert(n, stored.probe);
                }
            }
            frontier = next;
        }
        Ok(probes)
    }

    /// Every stored probe. For questions that start from the bottom of the graph, like what a
    /// key reaches, which have to look at every walk that could pass through it.
    pub async fn all_probes(&self) -> anyhow::Result<HashMap<Node, AuthorityProbe>> {
        Ok(sqlx::query("SELECT chain, address, probe FROM node")
            .fetch_all(self.pool())
            .await?
            .iter()
            .filter_map(|r| {
                let node = node_from(r.get("chain"), r.get("address"))?;
                let probe = serde_json::from_str(r.get("probe")).ok()?;
                Some((node, probe))
            })
            .collect())
    }

    /// Edges leaving `node`, open ones only unless `history` asks for closed ones too.
    pub async fn edges_from(&self, node: Node, history: bool) -> anyhow::Result<Vec<StoredEdge>> {
        self.edges_where("from", node, history).await
    }

    /// Edges arriving at `node`.
    pub async fn edges_to(&self, node: Node, history: bool) -> anyhow::Result<Vec<StoredEdge>> {
        self.edges_where("to", node, history).await
    }

    async fn edges_where(
        &self,
        end: &str,
        node: Node,
        history: bool,
    ) -> anyhow::Result<Vec<StoredEdge>> {
        let sql = if end == "from" {
            concat!(
                "SELECT ",
                edge_columns!(),
                " FROM edge WHERE from_chain = ?1 AND from_address = ?2 \
                 AND (?3 OR closed_at IS NULL) ORDER BY relation, first_seen_at, id"
            )
        } else {
            concat!(
                "SELECT ",
                edge_columns!(),
                " FROM edge WHERE to_chain = ?1 AND to_address = ?2 \
                 AND (?3 OR closed_at IS NULL) ORDER BY relation, first_seen_at, id"
            )
        };
        Ok(sqlx::query(sql)
            .bind(node.chain.as_str())
            .bind(checksum(node.address))
            .bind(history)
            .fetch_all(self.pool())
            .await?
            .iter()
            .filter_map(row_to_edge)
            .collect())
    }

    pub async fn events(&self, filter: &EventFilter) -> anyhow::Result<Vec<StoredEvent>> {
        let sql = if filter.ascending {
            concat!(
                "SELECT ",
                event_columns!(),
                " FROM authority_event WHERE ",
                event_filter!(),
                " ORDER BY observed_at, id LIMIT COALESCE(?8, -1)"
            )
        } else {
            concat!(
                "SELECT ",
                event_columns!(),
                " FROM authority_event WHERE ",
                event_filter!(),
                " ORDER BY observed_at DESC, id DESC LIMIT COALESCE(?8, -1)"
            )
        };
        Ok(sqlx::query(sql)
            .bind(&filter.address)
            .bind(filter.chain.map(Chain::as_str))
            .bind(filter.since)
            .bind(filter.after_block)
            .bind(filter.to_block)
            .bind(filter.include_reinterpretations)
            .bind(Cause::Chain.as_str())
            .bind(filter.limit)
            .fetch_all(self.pool())
            .await?
            .iter()
            .filter_map(row_to_event)
            .collect())
    }

    /// Chain events bisection has not pinned to a block yet, oldest first. Only fields that are
    /// one read at a block qualify; a root or a key count is a verdict over many reads and keeps
    /// its bracketing blocks.
    pub async fn unpinned_events(&self, limit: Option<i64>) -> anyhow::Result<Vec<StoredEvent>> {
        Ok(sqlx::query(concat!(
            "SELECT ",
            event_columns!(),
            " FROM authority_event WHERE at_block IS NULL AND cause = 'chain' \
             AND last_old_block IS NOT NULL AND first_new_block IS NOT NULL \
             AND field IN ('implementation', 'admin', 'beacon', 'owner', 'safe_threshold', \
                           'timelock_delay', 'safe_owner_added', 'safe_owner_removed') \
             ORDER BY id LIMIT COALESCE(?1, -1)"
        ))
        .bind(limit)
        .fetch_all(self.pool())
        .await?
        .iter()
        .filter_map(row_to_event)
        .collect())
    }

    pub async fn pin_event(&self, id: i64, at_block: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE authority_event SET at_block = ?2 WHERE id = ?1")
            .bind(id)
            .bind(at_block)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Every Safe `address` is currently an owner of, on either chain.
    ///
    /// Both chains, because a key is a key everywhere: the private key behind an EOA signs for
    /// the same address on Base and on Ethereum. Each row says which chain's account sits in the
    /// Safe and what that account is there, so a contract that merely shares the address on the
    /// other chain is not mistaken for the same key.
    pub async fn memberships(&self, address: &str) -> anyhow::Result<Vec<Membership>> {
        Ok(sqlx::query(
            "SELECT e.from_chain fc, e.from_address fa, e.to_chain tc, e.to_address ta, \
                    k.kind mk, s.threshold t, s.owner_count oc \
             FROM edge e \
             LEFT JOIN node s ON s.chain = e.from_chain AND s.address = e.from_address \
             LEFT JOIN node k ON k.chain = e.to_chain AND k.address = e.to_address \
             WHERE e.relation = 'safe_owner' AND e.closed_at IS NULL AND e.to_address = ?1 \
             ORDER BY e.from_chain, e.from_address",
        )
        .bind(address)
        .fetch_all(self.pool())
        .await?
        .iter()
        .filter_map(|r| {
            Some(Membership {
                safe: node_from(r.get("fc"), r.get("fa"))?,
                member: node_from(r.get("tc"), r.get("ta"))?,
                member_kind: r.get("mk"),
                threshold: r.get("t"),
                owner_count: r.get("oc"),
            })
        })
        .collect())
    }

    /// Pairs of Safes sharing at least `min_shared` owners.
    ///
    /// Owners on different chains count as shared only when both are keys: the same private
    /// key signs on every chain, but two contracts at one address on two chains are strangers.
    pub async fn shared_signers(&self, min_shared: i64) -> anyhow::Result<Vec<SharedSigners>> {
        Ok(sqlx::query(
            "SELECT a.from_chain ac, a.from_address aa, b.from_chain bc, b.from_address ba, \
                    COUNT(*) shared, GROUP_CONCAT(a.to_address, ',') keys \
             FROM edge a \
             JOIN edge b ON b.to_address = a.to_address \
                AND b.relation = 'safe_owner' AND b.closed_at IS NULL \
                AND (a.from_chain < b.from_chain \
                     OR (a.from_chain = b.from_chain AND a.from_address < b.from_address)) \
             LEFT JOIN node ka ON ka.chain = a.to_chain AND ka.address = a.to_address \
             LEFT JOIN node kb ON kb.chain = b.to_chain AND kb.address = b.to_address \
             WHERE a.relation = 'safe_owner' AND a.closed_at IS NULL \
               AND (a.to_chain = b.to_chain OR (ka.kind = 'eoa' AND kb.kind = 'eoa')) \
             GROUP BY ac, aa, bc, ba HAVING shared >= ?1 \
             ORDER BY shared DESC, ac, aa, bc, ba",
        )
        .bind(min_shared)
        .fetch_all(self.pool())
        .await?
        .iter()
        .filter_map(|r| {
            let mut keys: Vec<String> = r
                .get::<String, _>("keys")
                .split(',')
                .map(str::to_string)
                .collect();
            keys.sort();
            Some(SharedSigners {
                a: node_from(r.get("ac"), r.get("aa"))?,
                b: node_from(r.get("bc"), r.get("ba"))?,
                shared: r.get("shared"),
                keys,
            })
        })
        .collect())
    }

    /// Every covered proxy with the node its upgrade walk starts from, where it has one.
    pub async fn upgrade_entries(&self) -> anyhow::Result<Vec<(ProxyRecord, Option<Node>)>> {
        Ok(sqlx::query(concat!(
            "SELECT ",
            proxy_columns!(),
            " FROM proxy WHERE kind IN ",
            covered_kinds!(),
            " ORDER BY address"
        ))
        .fetch_all(self.pool())
        .await?
        .iter()
        .map(row_to_record)
        .map(|r| {
            let entry = graph::entry_of(&r);
            (r, entry)
        })
        .collect())
    }
}

/// Upsert one proxy row and its slot edges. `None` when the row was refused, in which case
/// nothing else it said is trusted either.
async fn write_proxy(
    conn: &mut SqliteConnection,
    obs: &Observation,
    r: &ProxyRecord,
) -> sqlx::Result<Option<Vec<PendingEvent>>> {
    let old = proxy_on(conn, &r.address).await?;
    if upsert_row(conn, r).await? == 0 {
        return Ok(None);
    }
    let Some((subject, sets)) = graph::slot_edges(r) else {
        return Ok(Some(Vec::new()));
    };
    write_edges(conn, obs, subject, &sets).await?;
    let Some(old) = old else {
        // First sight is not a change.
        return Ok(Some(Vec::new()));
    };
    Ok(Some(
        graph::proxy_changes(&old, r)
            .into_iter()
            .map(|change| PendingEvent {
                subject_type: SubjectType::Proxy,
                subject,
                cause: Cause::of(change.field, old.model_version),
                last_old_block: old.scanned_block,
                first_new_block: obs.block(Chain::Base),
                change,
            })
            .collect(),
    ))
}

/// Upsert one probed node and the edges its probe shows.
async fn write_node(
    conn: &mut SqliteConnection,
    obs: &Observation,
    node: Node,
    probe: &AuthorityProbe,
) -> sqlx::Result<Vec<PendingEvent>> {
    let old = node_on(conn, node).await?;
    let block = obs.block(node.chain);
    sqlx::query(
        "INSERT INTO node (chain, address, kind, code, threshold, owner_count, min_delay, probe, \
                           model_version, first_seen_block, first_seen_at, last_seen_block, \
                           last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?10, ?11) \
         ON CONFLICT(chain, address) DO UPDATE SET \
           kind = excluded.kind, code = excluded.code, threshold = excluded.threshold, \
           owner_count = excluded.owner_count, min_delay = excluded.min_delay, \
           probe = excluded.probe, model_version = excluded.model_version, \
           last_seen_block = excluded.last_seen_block, last_seen_at = excluded.last_seen_at",
    )
    .bind(node.chain.as_str())
    .bind(checksum(node.address))
    .bind(authority_kind(node, probe).as_str())
    .bind(graph::code_str(probe.code))
    .bind(probe.threshold.map(i64::from))
    .bind(probe.owners.as_ref().map(|o| o.len() as i64))
    .bind(
        probe
            .min_delay
            .map(|d| i64::try_from(d).unwrap_or(i64::MAX)),
    )
    .bind(serde_json::to_string(probe).expect("a probe always serializes"))
    .bind(MODEL_VERSION)
    .bind(block)
    .bind(obs.observed_at)
    .execute(&mut *conn)
    .await?;
    write_edges(conn, obs, node, &graph::probe_edges(node, probe)).await?;
    let Some(old) = old else {
        return Ok(Vec::new());
    };
    Ok(graph::probe_changes(node, &old.probe, probe)
        .into_iter()
        .map(|change| PendingEvent {
            subject_type: SubjectType::Node,
            subject: node,
            cause: Cause::of(change.field, Some(old.model_version)),
            last_old_block: old.last_seen_block,
            first_new_block: block,
            change,
        })
        .collect())
}

/// Make the open edges of `subject` match `sets`, one relation at a time.
///
/// Every relation in `sets` is a complete answer for that relation, so its open edges that the
/// answer no longer shows close, and the ones it still shows are marked seen. The set moves as a
/// unit inside the caller's transaction: a Safe whose owners changed never has the old owners and
/// the new ones open together, and a new root never inherits an old root's owners.
async fn write_edges(
    conn: &mut SqliteConnection,
    obs: &Observation,
    subject: Node,
    sets: &EdgeSets,
) -> sqlx::Result<()> {
    let block = obs.block(subject.chain);
    for (relation, targets) in sets {
        let open = sqlx::query(
            "SELECT id, to_chain, to_address FROM edge \
             WHERE from_chain = ?1 AND from_address = ?2 AND relation = ?3 AND closed_at IS NULL",
        )
        .bind(subject.chain.as_str())
        .bind(checksum(subject.address))
        .bind(relation.as_str())
        .fetch_all(&mut *conn)
        .await?;
        let mut unseen: BTreeSet<Node> = targets.clone();
        for row in open {
            let id: i64 = row.get("id");
            let still_there = node_from(row.get("to_chain"), row.get("to_address"))
                .is_some_and(|to| unseen.remove(&to));
            let sql = if still_there {
                "UPDATE edge SET last_seen_block = ?2, last_seen_at = ?3 WHERE id = ?1"
            } else {
                "UPDATE edge SET closed_block = ?2, closed_at = ?3 WHERE id = ?1"
            };
            sqlx::query(sql)
                .bind(id)
                .bind(block)
                .bind(obs.observed_at)
                .execute(&mut *conn)
                .await?;
        }
        for to in unseen {
            sqlx::query(
                "INSERT INTO edge (from_chain, from_address, relation, to_chain, to_address, \
                                   first_seen_block, first_seen_at, last_seen_block, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, ?7)",
            )
            .bind(subject.chain.as_str())
            .bind(checksum(subject.address))
            .bind(relation.as_str())
            .bind(to.chain.as_str())
            .bind(checksum(to.address))
            .bind(block)
            .bind(obs.observed_at)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

async fn insert_event(
    conn: &mut SqliteConnection,
    obs: &Observation,
    e: &PendingEvent,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO authority_event (observation_id, subject_type, subject_chain, \
             subject_address, field, old_value, new_value, last_old_block, first_new_block, \
             cause, model_version, observed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(obs.id)
    .bind(e.subject_type.as_str())
    .bind(e.subject.chain.as_str())
    .bind(checksum(e.subject.address))
    .bind(e.change.field.as_str())
    .bind(&e.change.old)
    .bind(&e.change.new)
    .bind(e.last_old_block)
    .bind(e.first_new_block)
    .bind(e.cause.as_str())
    .bind(MODEL_VERSION)
    .bind(obs.observed_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn node_on(conn: &mut SqliteConnection, node: Node) -> sqlx::Result<Option<StoredNode>> {
    Ok(sqlx::query(
        "SELECT kind, code, threshold, owner_count, min_delay, probe, model_version, \
                first_seen_block, first_seen_at, last_seen_block, last_seen_at \
         FROM node WHERE chain = ?1 AND address = ?2",
    )
    .bind(node.chain.as_str())
    .bind(checksum(node.address))
    .fetch_optional(&mut *conn)
    .await?
    .and_then(|r| {
        Some(StoredNode {
            node,
            kind: r.get("kind"),
            code: r.get("code"),
            threshold: r.get("threshold"),
            owner_count: r.get("owner_count"),
            min_delay: r.get("min_delay"),
            // A probe that no longer parses is treated as absent: no events against it, and
            // the next write replaces it.
            probe: serde_json::from_str(r.get("probe")).ok()?,
            model_version: r.get("model_version"),
            first_seen_block: r.get("first_seen_block"),
            first_seen_at: r.get("first_seen_at"),
            last_seen_block: r.get("last_seen_block"),
            last_seen_at: r.get("last_seen_at"),
        })
    }))
}

fn row_to_edge(r: &sqlx::sqlite::SqliteRow) -> Option<StoredEdge> {
    Some(StoredEdge {
        from: node_from(r.get("from_chain"), r.get("from_address"))?,
        relation: Relation::parse(r.get("relation"))?,
        to: node_from(r.get("to_chain"), r.get("to_address"))?,
        first_seen_block: r.get("first_seen_block"),
        first_seen_at: r.get("first_seen_at"),
        last_seen_block: r.get("last_seen_block"),
        last_seen_at: r.get("last_seen_at"),
        closed_block: r.get("closed_block"),
        closed_at: r.get("closed_at"),
    })
}

fn row_to_event(r: &sqlx::sqlite::SqliteRow) -> Option<StoredEvent> {
    let change = Change {
        field: Field::parse(r.get("field"))?,
        old: r.get("old_value"),
        new: r.get("new_value"),
    };
    Some(StoredEvent {
        id: r.get("id"),
        observation_id: r.get("observation_id"),
        subject_type: match r.get::<&str, _>("subject_type") {
            "proxy" => SubjectType::Proxy,
            "node" => SubjectType::Node,
            _ => return None,
        },
        subject: node_from(r.get("subject_chain"), r.get("subject_address"))?,
        direction: change.direction(),
        change,
        last_old_block: r.get("last_old_block"),
        first_new_block: r.get("first_new_block"),
        at_block: r.get("at_block"),
        cause: Cause::parse(r.get("cause"))?,
        model_version: r.get("model_version"),
        observed_at: r.get("observed_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Code, resolve};
    use alloy::primitives::address;

    const PROXY: Address = address!("9Caa0e7277ce86A4644F2D10b72561080531b674");
    const BEACON: Address = address!("e68ED13998fd48497EAA3b52e20823605D8d7706");
    const EOA: Address = address!("21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc");
    const SAFE: Address = address!("6454cf0127a153295435160768C85225Cd19Bf15");
    const K1: Address = address!("00000000000000000000000000000000000000a1");
    const K2: Address = address!("00000000000000000000000000000000000000b2");
    const K3: Address = address!("00000000000000000000000000000000000000c3");

    async fn mem() -> Store {
        Store::open("sqlite::memory:").await.unwrap()
    }

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

    fn beacon_proxy(root: Address, kind: &str, keys: i64, block: i64) -> ProxyRecord {
        ProxyRecord {
            address: checksum(PROXY),
            kind: "beacon".into(),
            beacon: Some(checksum(BEACON)),
            code_size: 508,
            scanned_at: block,
            terminal_authority: Some(checksum(root)),
            terminal_chain: Some("base".into()),
            authority_kind: Some(kind.into()),
            compromise_depth: Some(keys),
            timelock_seconds: Some(0),
            resolution_confidence: Some("high".into()),
            upgrade_path: Some("beacon".into()),
            scanned_block: Some(block),
            model_version: Some(MODEL_VERSION),
            ..Default::default()
        }
    }

    fn map(entries: &[(Address, AuthorityProbe)]) -> HashMap<Node, AuthorityProbe> {
        entries
            .iter()
            .map(|(a, p)| (Node::base(*a), p.clone()))
            .collect()
    }

    async fn observe(store: &Store, block: i64) -> Observation {
        store
            .begin_observation(Some(block), Some(block / 2), block)
            .await
            .unwrap()
    }

    /// The Sep 29 structure before the transfer: beacon owned by one key.
    fn before() -> (Vec<ProxyRecord>, HashMap<Node, AuthorityProbe>) {
        (
            vec![beacon_proxy(EOA, "eoa", 1, 51_927_177)],
            map(&[(BEACON, owned_by(EOA)), (EOA, key())]),
        )
    }

    /// And after: the same beacon owned by a 2-of-5 Safe the key is one owner of.
    fn after() -> (Vec<ProxyRecord>, HashMap<Node, AuthorityProbe>) {
        (
            vec![beacon_proxy(SAFE, "safe", 2, 51_927_178)],
            map(&[
                (BEACON, owned_by(SAFE)),
                (SAFE, safe(2, &[K1, K2, K3, EOA])),
                (K1, key()),
                (K2, key()),
                (K3, key()),
                (EOA, key()),
            ]),
        )
    }

    async fn write(
        store: &Store,
        block: i64,
        batch: (Vec<ProxyRecord>, HashMap<Node, AuthorityProbe>),
    ) -> Written {
        let obs = observe(store, block).await;
        store.write_batch(&obs, &batch.0, &batch.1).await.unwrap()
    }

    #[tokio::test]
    async fn a_scan_keeps_the_graph_it_walked() {
        let s = mem().await;
        let written = write(&s, 51_927_178, after()).await;
        assert_eq!(written.rows, 1);
        assert_eq!(written.nodes, 6);
        assert_eq!(written.events, 0, "first sight is not a change");

        let beacon = s.node(Node::base(BEACON)).await.unwrap().unwrap();
        assert_eq!(beacon.kind, "ownable");
        assert_eq!(beacon.first_seen_block, Some(51_927_178));
        let from_proxy = s.edges_from(Node::base(PROXY), false).await.unwrap();
        assert_eq!(from_proxy.len(), 1);
        assert_eq!(from_proxy[0].relation, Relation::Beacon);
        let owners = s.edges_from(Node::base(SAFE), false).await.unwrap();
        assert_eq!(owners.len(), 4);
        assert!(owners.iter().all(|e| e.relation == Relation::SafeOwner));
        assert_eq!(
            s.latest_observation().await.unwrap().unwrap().base_block,
            Some(51_927_178)
        );
    }

    /// The point of keeping the probes whole: an offline walk over the store must give exactly
    /// the answer a walk over the scan's own map gave.
    #[tokio::test]
    async fn a_walk_over_the_store_answers_what_the_scan_answered() {
        let s = mem().await;
        let (_, live) = after();
        write(&s, 1, after()).await;
        let stored = s.probes_from(&[Node::base(BEACON)]).await.unwrap();
        assert_eq!(stored, live);
        assert_eq!(
            resolve(Node::base(BEACON), &stored),
            resolve(Node::base(BEACON), &live)
        );
    }

    /// The Sep 29 transfer, end to end through the store: the beacon's owner changes, the
    /// proxy's root and key count change with it, and nothing else is reported.
    #[tokio::test]
    async fn a_beacon_changing_hands_is_recorded_as_three_chain_events() {
        let s = mem().await;
        write(&s, 51_927_177, before()).await;
        let written = write(&s, 51_927_178, after()).await;
        assert_eq!(written.events, 3);

        let events = s
            .events(&EventFilter {
                ascending: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let summary: Vec<(SubjectType, Field, Option<&str>)> = events
            .iter()
            .map(|e| (e.subject_type, e.change.field, e.direction))
            .collect();
        assert_eq!(
            summary,
            vec![
                (SubjectType::Proxy, Field::Root, None),
                (SubjectType::Proxy, Field::KeysRequired, Some("rose")),
                (SubjectType::Node, Field::Owner, None),
            ]
        );
        let owner = &events[2];
        assert_eq!(owner.subject, Node::base(BEACON));
        assert_eq!(owner.change.old.as_deref(), Some(checksum(EOA).as_str()));
        assert_eq!(owner.change.new.as_deref(), Some(checksum(SAFE).as_str()));
        assert_eq!(owner.last_old_block, Some(51_927_177));
        assert_eq!(owner.first_new_block, Some(51_927_178));
        assert_eq!(owner.cause, Cause::Chain);
        assert!(
            events.iter().all(|e| e.at_block.is_none()),
            "not bisected yet"
        );

        let history = s.edges_from(Node::base(BEACON), true).await.unwrap();
        let owner_edges: Vec<_> = history
            .iter()
            .filter(|e| e.relation == Relation::Owner)
            .collect();
        assert_eq!(owner_edges.len(), 2, "one closed, one open");
        assert_eq!(owner_edges[0].to, Node::base(EOA));
        assert_eq!(owner_edges[0].closed_block, Some(51_927_178));
        assert_eq!(owner_edges[1].to, Node::base(SAFE));
        assert_eq!(owner_edges[1].closed_block, None);
    }

    #[tokio::test]
    async fn a_safe_losing_one_owner_closes_exactly_that_edge() {
        let s = mem().await;
        write(&s, 10, (vec![], map(&[(SAFE, safe(2, &[K1, K2, K3]))]))).await;
        write(&s, 20, (vec![], map(&[(SAFE, safe(2, &[K1, K2]))]))).await;
        let open = s.edges_from(Node::base(SAFE), false).await.unwrap();
        let all = s.edges_from(Node::base(SAFE), true).await.unwrap();
        assert_eq!(open.len(), 2);
        assert_eq!(all.len(), 3);
        let closed: Vec<_> = all.iter().filter(|e| e.closed_block.is_some()).collect();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].to, Node::base(K3));
        assert!(open.iter().all(|e| e.last_seen_block == Some(20)));

        let events = s.events(&EventFilter::default()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].change.field, Field::SafeOwnerRemoved);
    }

    /// An outage has told me nothing: a node the scan could not read keeps its open edges and
    /// its last-seen block, and produces no events.
    #[tokio::test]
    async fn an_unread_node_keeps_its_edges_and_its_last_seen_block() {
        let s = mem().await;
        write(&s, 10, (vec![], map(&[(SAFE, safe(2, &[K1, K2, K3]))]))).await;
        write(&s, 20, (vec![], map(&[(K1, key())]))).await;
        let open = s.edges_from(Node::base(SAFE), false).await.unwrap();
        assert_eq!(open.len(), 3);
        assert!(open.iter().all(|e| e.last_seen_block == Some(10)));
        let node = s.node(Node::base(SAFE)).await.unwrap().unwrap();
        assert_eq!(node.last_seen_block, Some(10));
        assert!(s.events(&EventFilter::default()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn two_observations_of_one_node_are_one_row_seen_twice() {
        let s = mem().await;
        write(&s, 10, (vec![], map(&[(EOA, key())]))).await;
        write(&s, 20, (vec![], map(&[(EOA, key())]))).await;
        let n = s.node(Node::base(EOA)).await.unwrap().unwrap();
        assert_eq!(
            (n.first_seen_block, n.last_seen_block),
            (Some(10), Some(20))
        );
        assert_eq!(s.latest_observation().await.unwrap().unwrap().id, 2);
    }

    /// The never-overwrite-live-code guard extends to the graph: a row it refuses must not
    /// close the proxy's slot edges or report its admin as gone.
    #[tokio::test]
    async fn a_refused_row_closes_no_edges_and_reports_nothing() {
        let s = mem().await;
        write(&s, 10, before()).await;
        let blank = ProxyRecord {
            kind: "eoa".into(),
            beacon: None,
            code_size: 0,
            terminal_authority: None,
            unresolved_reason: None,
            ..beacon_proxy(EOA, "eoa", 1, 20)
        };
        let written = write(&s, 20, (vec![blank], HashMap::new())).await;
        assert_eq!(written.rows, 0);
        assert_eq!(written.events, 0);
        assert_eq!(
            s.edges_from(Node::base(PROXY), false).await.unwrap().len(),
            1
        );
    }

    /// A row resolved before model versions existed is compared, but a different root under a
    /// different model is Hermes changing its mind, and is labelled so. A slot is never that.
    #[tokio::test]
    async fn a_changed_answer_under_an_older_model_is_a_reinterpretation() {
        let s = mem().await;
        let mut old = beacon_proxy(EOA, "eoa", 1, 10);
        old.model_version = None;
        old.scanned_block = None;
        s.upsert_many(&[old]).await.unwrap();
        let mut new = beacon_proxy(SAFE, "safe", 2, 20);
        new.beacon = Some(checksum(K1));
        write(&s, 20, (vec![new], HashMap::new())).await;

        let shown = s.events(&EventFilter::default()).await.unwrap();
        assert_eq!(shown.len(), 1, "only the slot change is shown by default");
        assert_eq!(shown[0].change.field, Field::Beacon);
        assert_eq!(shown[0].cause, Cause::Chain);
        assert_eq!(
            shown[0].last_old_block, None,
            "the old row was never pinned to a block"
        );

        let all = s
            .events(&EventFilter {
                include_reinterpretations: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        assert!(
            all.iter()
                .filter(|e| e.change.field != Field::Beacon)
                .all(|e| e.cause == Cause::Reinterpretation)
        );
        assert!(
            s.unpinned_events(None).await.unwrap().is_empty(),
            "an event without both blocks cannot be bisected"
        );
    }

    #[tokio::test]
    async fn an_edge_seen_again_after_closing_opens_a_new_stretch() {
        let s = mem().await;
        write(&s, 10, (vec![], map(&[(BEACON, owned_by(EOA))]))).await;
        write(&s, 20, (vec![], map(&[(BEACON, owned_by(SAFE))]))).await;
        write(&s, 30, (vec![], map(&[(BEACON, owned_by(EOA))]))).await;
        let history: Vec<(Node, Option<i64>, Option<i64>)> = s
            .edges_from(Node::base(BEACON), true)
            .await
            .unwrap()
            .into_iter()
            .map(|e| (e.to, e.first_seen_block, e.closed_block))
            .collect();
        assert_eq!(
            history,
            vec![
                (Node::base(EOA), Some(10), Some(20)),
                (Node::base(SAFE), Some(20), Some(30)),
                (Node::base(EOA), Some(30), None),
            ]
        );
    }

    #[tokio::test]
    async fn events_narrow_by_subject_time_and_block() {
        let s = mem().await;
        write(&s, 10, before()).await;
        write(&s, 20, after()).await;
        let by_beacon = EventFilter {
            address: Some(checksum(BEACON).to_lowercase()),
            ..Default::default()
        };
        assert_eq!(s.events(&by_beacon).await.unwrap().len(), 1);
        let later = EventFilter {
            since: Some(21),
            ..Default::default()
        };
        assert!(s.events(&later).await.unwrap().is_empty());
        let in_range = EventFilter {
            after_block: Some(10),
            to_block: Some(20),
            ..Default::default()
        };
        assert_eq!(s.events(&in_range).await.unwrap().len(), 3);
        let before_range = EventFilter {
            to_block: Some(19),
            ..Default::default()
        };
        assert!(s.events(&before_range).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_pinned_event_leaves_the_unpinned_list() {
        let s = mem().await;
        write(&s, 10, before()).await;
        write(&s, 20, after()).await;
        let unpinned = s.unpinned_events(None).await.unwrap();
        assert_eq!(
            unpinned.len(),
            1,
            "the root and key count are verdicts, not reads"
        );
        assert_eq!(unpinned[0].change.field, Field::Owner);
        s.pin_event(unpinned[0].id, 15).await.unwrap();
        assert!(s.unpinned_events(None).await.unwrap().is_empty());
    }

    /// A key sits in Safes on both chains, and the same five signers behind two Safes is a
    /// dependency no single contract page shows.
    #[tokio::test]
    async fn a_key_in_two_safes_on_two_chains_is_one_signer_shared() {
        let s = mem().await;
        let obs = observe(&s, 10).await;
        let mut probes = map(&[
            (SAFE, safe(2, &[K1, K2, K3])),
            (K1, key()),
            (K2, key()),
            (K3, key()),
        ]);
        probes.insert(Node::ethereum(BEACON), safe(2, &[K1, K2]));
        probes.insert(Node::ethereum(K1), key());
        probes.insert(Node::ethereum(K2), key());
        s.write_batch(&obs, &[], &probes).await.unwrap();

        let memberships = s.memberships(&checksum(K1).to_lowercase()).await.unwrap();
        assert_eq!(memberships.len(), 2);
        assert_eq!(memberships[0].safe, Node::base(SAFE));
        assert_eq!(memberships[1].safe, Node::ethereum(BEACON));
        assert_eq!(memberships[1].member_kind.as_deref(), Some("eoa"));
        assert_eq!(memberships[0].threshold, Some(2));

        let shared = s.shared_signers(2).await.unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].shared, 2);
        assert_eq!(shared[0].keys, vec![checksum(K1), checksum(K2)]);
        assert!(s.shared_signers(3).await.unwrap().is_empty());
    }

    /// Two contracts that happen to share an address on two chains are strangers. Only keys
    /// carry across.
    #[tokio::test]
    async fn contracts_at_one_address_on_two_chains_are_not_a_shared_signer() {
        let s = mem().await;
        let obs = observe(&s, 10).await;
        let mut probes = map(&[
            (SAFE, safe(1, &[K1, K2])),
            (K1, owned_by(K3)),
            (K2, owned_by(K3)),
        ]);
        probes.insert(Node::ethereum(BEACON), safe(1, &[K1, K2]));
        probes.insert(Node::ethereum(K1), key());
        probes.insert(Node::ethereum(K2), key());
        s.write_batch(&obs, &[], &probes).await.unwrap();
        assert!(s.shared_signers(1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn every_covered_proxy_comes_back_with_where_its_walk_starts() {
        let s = mem().await;
        write(&s, 10, before()).await;
        let entries = s.upgrade_entries().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, Some(Node::base(BEACON)));
    }
}
