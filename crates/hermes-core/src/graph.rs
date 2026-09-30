//! The authority graph as it is kept between scans, and what changed between two looks at it.
//!
//! The resolver walks a probe map and forgets it. Keeping it is what lets Hermes say how a proxy
//! reaches its root, what else a key reaches, and what changed since the last scan. The store
//! keeps two forms on purpose. Each node's probe is stored whole, because it is the exact input
//! the resolver walks, and a lossy projection would let an offline answer drift from the one the
//! scan published. Its edges are stored as rows too, because "which Safes is this key in" and
//! "what does this beacon control" are joins, and joins need rows.
//!
//! Pure: nothing here reads a chain or a database.

use crate::authority::{AuthorityProbe, Code, Resolution, authority_kind, resolve};
use crate::chain::Node;
use crate::store::{ProxyRecord, checksum};
use crate::upgrade::UpgradeEntry;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Which interpretation of the chain produced a stored answer.
///
/// Bumped by hand whenever the same chain state could now be read differently: a new interface
/// probed, a new kind recognized, a changed rule for the key count. A difference between two
/// observations made under different versions is Hermes changing its mind, not the chain
/// changing, and it must never be reported as the second. The deploy that re-derived every root
/// after aliasing was modelled would otherwise have read as 28 authority changes in one night.
///
/// 1. The graph is kept (2026-09-30). Resolution as of the chain-wide index of 2026-09-20.
/// 2. EIP-7702 delegated accounts read as keys; ERC-4337 accounts, `AccessControl` contracts
///    and sentinel addresses recognized; timelock roots say their roles are unread.
pub const MODEL_VERSION: i64 = 2;

/// How one address stands to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// A proxy's ERC-1967 implementation slot. Not a control edge: the implementation decides
    /// what the proxy does, not who may change it.
    Implementation,
    /// A proxy's ERC-1967 admin slot.
    Admin,
    /// A proxy's ERC-1967 beacon slot.
    Beacon,
    /// What `owner()` answered.
    Owner,
    /// One entry of `getOwners()`.
    SafeOwner,
    /// A codeless Base address to the Ethereum contract acting through it.
    L1Alias,
    /// One address signer of a MultiOwnable smart account.
    AccountOwner,
    /// One holder of an `AccessControl` contract's upgrade role.
    RoleMember,
}

impl Relation {
    pub const ALL: [Relation; 8] = [
        Self::Implementation,
        Self::Admin,
        Self::Beacon,
        Self::Owner,
        Self::SafeOwner,
        Self::L1Alias,
        Self::AccountOwner,
        Self::RoleMember,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Admin => "admin",
            Self::Beacon => "beacon",
            Self::Owner => "owner",
            Self::SafeOwner => "safe_owner",
            Self::L1Alias => "l1_alias",
            Self::AccountOwner => "account_owner",
            Self::RoleMember => "role_member",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// Whether the target of this edge decides what the source does: its admin, its beacon, its
    /// owner, or the L1 contract behind it. A signer is deliberately not one, of a Safe, an
    /// account or a role. One signer of several takes part in control, and whether it *has*
    /// control is a threshold question that only `blast::falls` answers.
    pub fn is_control(self) -> bool {
        matches!(
            self,
            Self::Admin | Self::Beacon | Self::Owner | Self::L1Alias
        )
    }
}

/// Every edge one subject was seen to have, by relation.
///
/// A relation present with an empty set is a claim: this subject has no such edge now, so any
/// open one closes. A relation absent from the map was not looked at, and nothing about it
/// changes. That difference is what stops an unanswered read from closing edges.
pub type EdgeSets = BTreeMap<Relation, BTreeSet<Node>>;

/// The edges a settled probe shows. Every probe answers for all of its relations: a codeless
/// address was not asked `owner()` because it cannot have one.
pub fn probe_edges(node: Node, probe: &AuthorityProbe) -> EdgeSets {
    let on_same_chain = |address: &Address| Node {
        chain: node.chain,
        address: *address,
    };
    let alias = match probe.code {
        Code::L1Alias(l1) => BTreeSet::from([Node::ethereum(l1)]),
        Code::Present | Code::Absent | Code::Delegated(_) => BTreeSet::new(),
    };
    EdgeSets::from([
        (
            Relation::Owner,
            probe.owner.iter().map(on_same_chain).collect(),
        ),
        (
            Relation::SafeOwner,
            probe.owners.iter().flatten().map(on_same_chain).collect(),
        ),
        (Relation::L1Alias, alias),
        (
            Relation::AccountOwner,
            probe
                .account_owners
                .iter()
                .flat_map(|o| &o.addresses)
                .map(on_same_chain)
                .collect(),
        ),
        (
            Relation::RoleMember,
            probe
                .roles
                .iter()
                .flat_map(|r| r.members.iter().flatten())
                .map(on_same_chain)
                .collect(),
        ),
    ])
}

/// The slot edges of a proxy row, or `None` when the row carries an address I cannot parse.
/// Rows are written by this program, so that would be a bug, and a bug should not close edges.
pub fn slot_edges(row: &ProxyRecord) -> Option<(Node, EdgeSets)> {
    let subject = Node::base(row.address.parse().ok()?);
    let slot = |value: &Option<String>| -> Option<BTreeSet<Node>> {
        match value {
            None => Some(BTreeSet::new()),
            Some(a) => Some(BTreeSet::from([Node::base(a.parse().ok()?)])),
        }
    };
    Some((
        subject,
        EdgeSets::from([
            (Relation::Implementation, slot(&row.implementation)?),
            (Relation::Admin, slot(&row.admin)?),
            (Relation::Beacon, slot(&row.beacon)?),
        ]),
    ))
}

/// A property of a proxy or an authority that can change between two scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Implementation,
    Admin,
    Beacon,
    /// The proxy's classification.
    Kind,
    /// The resolved root, as `chain:address`.
    Root,
    KeysRequired,
    TimelockSeconds,
    /// `present`, `absent` or `l1_alias`.
    Code,
    AuthorityKind,
    Owner,
    SafeOwnerAdded,
    SafeOwnerRemoved,
    SafeThreshold,
    TimelockDelay,
    AccountOwnerAdded,
    AccountOwnerRemoved,
    RoleMemberAdded,
    RoleMemberRemoved,
}

impl Field {
    pub const ALL: [Field; 18] = [
        Self::Implementation,
        Self::Admin,
        Self::Beacon,
        Self::Kind,
        Self::Root,
        Self::KeysRequired,
        Self::TimelockSeconds,
        Self::Code,
        Self::AuthorityKind,
        Self::Owner,
        Self::SafeOwnerAdded,
        Self::SafeOwnerRemoved,
        Self::SafeThreshold,
        Self::TimelockDelay,
        Self::AccountOwnerAdded,
        Self::AccountOwnerRemoved,
        Self::RoleMemberAdded,
        Self::RoleMemberRemoved,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Admin => "admin",
            Self::Beacon => "beacon",
            Self::Kind => "kind",
            Self::Root => "root",
            Self::KeysRequired => "keys_required",
            Self::TimelockSeconds => "timelock_seconds",
            Self::Code => "code",
            Self::AuthorityKind => "authority_kind",
            Self::Owner => "owner",
            Self::SafeOwnerAdded => "safe_owner_added",
            Self::SafeOwnerRemoved => "safe_owner_removed",
            Self::SafeThreshold => "safe_threshold",
            Self::TimelockDelay => "timelock_delay",
            Self::AccountOwnerAdded => "account_owner_added",
            Self::AccountOwnerRemoved => "account_owner_removed",
            Self::RoleMemberAdded => "role_member_added",
            Self::RoleMemberRemoved => "role_member_removed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// Whether this field is Hermes's reading of the chain rather than a word read off it.
    ///
    /// Slot words are read, not interpreted, so a changed admin slot is the chain moving
    /// whatever version of Hermes looked. Everything else passes through probes and the
    /// resolver, and a new version can change it with the chain standing still.
    pub fn is_interpreted(self) -> bool {
        !matches!(self, Self::Implementation | Self::Admin | Self::Beacon)
    }

    /// Whether this field holds a number, so a change to it has a direction.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::KeysRequired | Self::TimelockSeconds | Self::SafeThreshold | Self::TimelockDelay
        )
    }
}

/// One field of one subject, before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub field: Field,
    pub old: Option<String>,
    pub new: Option<String>,
}

impl Change {
    /// `rose` or `fell` for a numeric field whose both sides are known. Arithmetic, not a
    /// judgment: whether a rising key count is good news is the reader's call.
    pub fn direction(&self) -> Option<&'static str> {
        if !self.field.is_numeric() {
            return None;
        }
        let old: u128 = self.old.as_deref()?.parse().ok()?;
        let new: u128 = self.new.as_deref()?.parse().ok()?;
        match new.cmp(&old) {
            std::cmp::Ordering::Greater => Some("rose"),
            std::cmp::Ordering::Less => Some("fell"),
            std::cmp::Ordering::Equal => None,
        }
    }
}

/// Why a stored answer moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// The chain answered differently.
    Chain,
    /// Hermes reads the chain differently now. Kept, labelled, and never shown as a change.
    Reinterpretation,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chain => "chain",
            Self::Reinterpretation => "reinterpretation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Chain, Self::Reinterpretation]
            .into_iter()
            .find(|c| c.as_str() == s)
    }

    /// An interpreted field that last moved under another model version (or under none, for a
    /// row stored before versions existed) cannot be told apart from Hermes changing its mind.
    pub fn of(field: Field, stored_model: Option<i64>) -> Self {
        if field.is_interpreted() && stored_model != Some(MODEL_VERSION) {
            Self::Reinterpretation
        } else {
            Self::Chain
        }
    }
}

fn differs(old: &Option<String>, new: &Option<String>) -> bool {
    match (old, new) {
        (Some(a), Some(b)) => !a.eq_ignore_ascii_case(b),
        (a, b) => a.is_some() != b.is_some(),
    }
}

fn push(out: &mut Vec<Change>, field: Field, old: Option<String>, new: Option<String>) {
    if differs(&old, &new) {
        out.push(Change { field, old, new });
    }
}

fn root(r: &ProxyRecord) -> Option<String> {
    let address = r.terminal_authority.as_ref()?;
    Some(format!(
        "{}:{address}",
        r.terminal_chain.as_deref().unwrap_or("base")
    ))
}

/// What differs between the stored row for a proxy and the one a scan just produced.
///
/// Resolution fields are compared only when both sides are settled. A row whose last attempt
/// was an outage, or that was never resolved, holds no answer, and going from no answer to an
/// answer is Hermes finding out, not the chain changing. A key count or delay is compared only
/// when both sides have a root; a root that changed says the rest.
pub fn proxy_changes(old: &ProxyRecord, new: &ProxyRecord) -> Vec<Change> {
    let mut out = Vec::new();
    push(
        &mut out,
        Field::Implementation,
        old.implementation.clone(),
        new.implementation.clone(),
    );
    push(&mut out, Field::Admin, old.admin.clone(), new.admin.clone());
    push(
        &mut out,
        Field::Beacon,
        old.beacon.clone(),
        new.beacon.clone(),
    );
    push(
        &mut out,
        Field::Kind,
        Some(old.kind.clone()),
        Some(new.kind.clone()),
    );
    if !(old.resolution_is_settled() && new.resolution_is_settled()) {
        return out;
    }
    push(&mut out, Field::Root, root(old), root(new));
    if old.terminal_authority.is_some() && new.terminal_authority.is_some() {
        let number = |v: Option<i64>| v.map(|n| n.to_string());
        push(
            &mut out,
            Field::KeysRequired,
            number(old.compromise_depth),
            number(new.compromise_depth),
        );
        push(
            &mut out,
            Field::TimelockSeconds,
            number(old.timelock_seconds),
            number(new.timelock_seconds),
        );
    }
    out
}

/// Where a stored proxy's upgrade walk starts, by the rules the scan used.
///
/// A UUPS proxy is walked from itself only once its implementation confirmed it is UUPS. The
/// row carries the proof: a root reached through `uups_owner`, or a walk that ended at something
/// unrecognized, which it could only have done if it started.
pub fn upgrade_entry_of(row: &ProxyRecord) -> Option<UpgradeEntry> {
    let parse = |a: &Option<String>| a.as_deref()?.parse::<Address>().ok();
    match row.kind.as_str() {
        "transparent" | "admin_only" => parse(&row.admin).map(UpgradeEntry::AdminSlot),
        "beacon" => parse(&row.beacon).map(UpgradeEntry::Beacon),
        "uups"
            if row.upgrade_path.as_deref() == Some("uups_owner")
                || row.unresolved_reason.as_deref() == Some("unrecognized_interface") =>
        {
            row.address
                .parse::<Address>()
                .ok()
                .map(UpgradeEntry::UupsOwner)
        }
        _ => None,
    }
}

/// The node `upgrade_entry_of` starts from.
pub fn entry_of(row: &ProxyRecord) -> Option<Node> {
    upgrade_entry_of(row).map(UpgradeEntry::start)
}

/// A stored proxy's walk, replayed over `probes` and capped the way the scan capped it.
pub fn replay(
    row: &ProxyRecord,
    probes: &HashMap<Node, AuthorityProbe>,
) -> Option<(UpgradeEntry, Resolution)> {
    let entry = upgrade_entry_of(row)?;
    Some((
        entry,
        resolve(entry.start(), probes).capped(entry.ceiling()),
    ))
}

pub fn code_str(code: Code) -> &'static str {
    match code {
        Code::Present => "present",
        Code::Absent => "absent",
        Code::L1Alias(_) => "l1_alias",
        Code::Delegated(_) => "delegated",
    }
}

/// Signers that joined and left between two probes, as one field each way.
fn membership(
    out: &mut Vec<Change>,
    (added, removed): (Field, Field),
    before: BTreeSet<Address>,
    after: BTreeSet<Address>,
) {
    for a in after.difference(&before) {
        out.push(Change {
            field: added,
            old: None,
            new: Some(checksum(*a)),
        });
    }
    for r in before.difference(&after) {
        out.push(Change {
            field: removed,
            old: Some(checksum(*r)),
            new: None,
        });
    }
}

/// What differs between two settled probes of one address.
pub fn probe_changes(node: Node, old: &AuthorityProbe, new: &AuthorityProbe) -> Vec<Change> {
    let mut out = Vec::new();
    let text = |s: &str| Some(s.to_string());
    push(
        &mut out,
        Field::Code,
        text(code_str(old.code)),
        text(code_str(new.code)),
    );
    push(
        &mut out,
        Field::AuthorityKind,
        text(authority_kind(node, old).as_str()),
        text(authority_kind(node, new).as_str()),
    );
    push(
        &mut out,
        Field::Owner,
        old.owner.map(checksum),
        new.owner.map(checksum),
    );
    let set = |v: Option<&Vec<Address>>| -> BTreeSet<Address> {
        v.into_iter().flatten().copied().collect()
    };
    membership(
        &mut out,
        (Field::SafeOwnerAdded, Field::SafeOwnerRemoved),
        set(old.owners.as_ref()),
        set(new.owners.as_ref()),
    );
    push(
        &mut out,
        Field::SafeThreshold,
        old.threshold.map(|t| t.to_string()),
        new.threshold.map(|t| t.to_string()),
    );
    push(
        &mut out,
        Field::TimelockDelay,
        old.min_delay.map(|d| d.to_string()),
        new.min_delay.map(|d| d.to_string()),
    );
    membership(
        &mut out,
        (Field::AccountOwnerAdded, Field::AccountOwnerRemoved),
        set(old.account_owners.as_ref().map(|o| &o.addresses)),
        set(new.account_owners.as_ref().map(|o| &o.addresses)),
    );
    membership(
        &mut out,
        (Field::RoleMemberAdded, Field::RoleMemberRemoved),
        set(old.roles.as_ref().and_then(|r| r.members.as_ref())),
        set(new.roles.as_ref().and_then(|r| r.members.as_ref())),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const A: Address = address!("00000000000000000000000000000000000000a1");
    const B: Address = address!("00000000000000000000000000000000000000b2");
    const C: Address = address!("00000000000000000000000000000000000000c3");

    fn safe(threshold: u32, owners: &[Address]) -> AuthorityProbe {
        AuthorityProbe {
            owners: Some(owners.to_vec()),
            threshold: Some(threshold),
            ..Default::default()
        }
    }

    fn row() -> ProxyRecord {
        ProxyRecord {
            address: checksum(A),
            kind: "beacon".into(),
            beacon: Some(checksum(B)),
            terminal_authority: Some(checksum(C)),
            terminal_chain: Some("base".into()),
            authority_kind: Some("eoa".into()),
            compromise_depth: Some(1),
            timelock_seconds: Some(0),
            resolution_confidence: Some("high".into()),
            upgrade_path: Some("beacon".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_relation_and_a_field_read_back_from_their_names() {
        for r in Relation::ALL {
            assert_eq!(Relation::parse(r.as_str()), Some(r));
        }
        for f in Field::ALL {
            assert_eq!(Field::parse(f.as_str()), Some(f));
        }
    }

    /// A Safe owner takes part in control without having it, so it must never be walked as a
    /// control edge; the implementation decides nothing about who upgrades.
    #[test]
    fn only_admin_beacon_owner_and_alias_edges_are_control() {
        assert!(Relation::Admin.is_control());
        assert!(Relation::Beacon.is_control());
        assert!(Relation::Owner.is_control());
        assert!(Relation::L1Alias.is_control());
        assert!(!Relation::SafeOwner.is_control());
        assert!(!Relation::Implementation.is_control());
    }

    /// Every relation a probe answers for is present, empty or not: an empty set is what closes
    /// an owner edge when a contract renounces.
    #[test]
    fn a_probe_answers_for_every_relation_it_could_have() {
        let renounced = probe_edges(Node::base(A), &AuthorityProbe::default());
        assert_eq!(renounced.len(), 5);
        assert!(renounced.values().all(BTreeSet::is_empty));

        let sets = probe_edges(Node::ethereum(A), &safe(1, &[B, C]));
        assert_eq!(
            sets[&Relation::SafeOwner],
            BTreeSet::from([Node::ethereum(B), Node::ethereum(C)]),
            "an owner read on Ethereum is an Ethereum address"
        );
    }

    #[test]
    fn an_alias_edge_crosses_to_ethereum() {
        let alias = AuthorityProbe {
            code: Code::L1Alias(B),
            ..Default::default()
        };
        assert_eq!(
            probe_edges(Node::base(A), &alias)[&Relation::L1Alias],
            BTreeSet::from([Node::ethereum(B)])
        );
    }

    #[test]
    fn a_proxy_row_answers_for_all_three_slots() {
        let (subject, sets) = slot_edges(&row()).unwrap();
        assert_eq!(subject, Node::base(A));
        assert_eq!(sets[&Relation::Beacon], BTreeSet::from([Node::base(B)]));
        assert!(sets[&Relation::Admin].is_empty());
        assert!(sets[&Relation::Implementation].is_empty());
    }

    #[test]
    fn a_row_with_an_unparseable_slot_closes_nothing() {
        let mut bad = row();
        bad.admin = Some("0xnot-an-address".into());
        assert_eq!(slot_edges(&bad), None);
    }

    /// The Sep 29 transfer, as a pair of rows: same beacon, a new root, one key more.
    #[test]
    fn a_beacon_changing_hands_is_a_root_and_a_key_count_change() {
        let before = row();
        let after = ProxyRecord {
            terminal_authority: Some(checksum(address!(
                "6454cf0127a153295435160768C85225Cd19Bf15"
            ))),
            authority_kind: Some("safe".into()),
            compromise_depth: Some(2),
            ..row()
        };
        let changes = proxy_changes(&before, &after);
        let fields: Vec<Field> = changes.iter().map(|c| c.field).collect();
        assert_eq!(fields, vec![Field::Root, Field::KeysRequired]);
        assert_eq!(changes[1].direction(), Some("rose"));
    }

    #[test]
    fn the_same_row_in_another_case_is_no_change() {
        let mut shouting = row();
        shouting.beacon = shouting
            .beacon
            .map(|b| b.to_ascii_uppercase().replace("0X", "0x"));
        assert!(proxy_changes(&row(), &shouting).is_empty());
    }

    /// Going from no answer to an answer is Hermes finding out. An outage in between must not
    /// read as the root disappearing and coming back.
    #[test]
    fn an_unsettled_side_compares_no_resolution_fields() {
        let unresolved = ProxyRecord {
            terminal_authority: None,
            terminal_chain: None,
            authority_kind: None,
            compromise_depth: None,
            unresolved_reason: Some("rpc_undetermined".into()),
            ..row()
        };
        assert!(proxy_changes(&row(), &unresolved).is_empty());
        assert!(proxy_changes(&unresolved, &row()).is_empty());
        let never_attempted = ProxyRecord {
            unresolved_reason: None,
            ..unresolved.clone()
        };
        assert!(proxy_changes(&never_attempted, &row()).is_empty());
    }

    #[test]
    fn a_root_giving_way_to_an_unrecognized_contract_is_a_root_change_and_nothing_more() {
        let unrecognized = ProxyRecord {
            terminal_authority: None,
            terminal_chain: None,
            authority_kind: None,
            compromise_depth: None,
            timelock_seconds: None,
            unresolved_reason: Some("unrecognized_interface".into()),
            ..row()
        };
        let changes = proxy_changes(&row(), &unrecognized);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, Field::Root);
        assert_eq!(changes[0].new, None);
    }

    #[test]
    fn a_safe_swapping_a_signer_and_its_threshold_says_exactly_that() {
        let changes = probe_changes(Node::base(A), &safe(2, &[A, B]), &safe(1, &[A, C]));
        let fields: Vec<Field> = changes.iter().map(|c| c.field).collect();
        assert_eq!(
            fields,
            vec![
                Field::SafeOwnerAdded,
                Field::SafeOwnerRemoved,
                Field::SafeThreshold
            ]
        );
        assert_eq!(changes[0].new.as_deref(), Some(checksum(C).as_str()));
        assert_eq!(changes[2].direction(), Some("fell"));
    }

    #[test]
    fn owner_order_is_not_a_change() {
        assert!(probe_changes(Node::base(A), &safe(2, &[A, B]), &safe(2, &[B, A])).is_empty());
    }

    #[test]
    fn an_ownership_transfer_is_an_owner_change() {
        let from = AuthorityProbe {
            owner: Some(A),
            ..Default::default()
        };
        let to = AuthorityProbe {
            owner: Some(B),
            ..Default::default()
        };
        let changes = probe_changes(Node::base(C), &from, &to);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, Field::Owner);
        assert_eq!(changes[0].direction(), None, "an address has no direction");
    }

    /// Slot words are read, not interpreted, so their changes are the chain moving under any
    /// version. Everything else under an older (or no) version is Hermes changing its mind.
    #[test]
    fn a_change_under_another_model_version_is_a_reinterpretation_unless_it_is_a_slot() {
        assert_eq!(Cause::of(Field::Admin, None), Cause::Chain);
        assert_eq!(Cause::of(Field::Admin, Some(0)), Cause::Chain);
        assert_eq!(Cause::of(Field::Root, None), Cause::Reinterpretation);
        assert_eq!(
            Cause::of(Field::Owner, Some(MODEL_VERSION - 1)),
            Cause::Reinterpretation
        );
        assert_eq!(Cause::of(Field::Owner, Some(MODEL_VERSION)), Cause::Chain);
    }

    #[test]
    fn a_walk_starts_where_the_scan_started_it() {
        assert_eq!(
            entry_of(&row()),
            Some(Node::base(B)),
            "a beacon proxy, from its beacon"
        );
        let transparent = ProxyRecord {
            kind: "transparent".into(),
            admin: Some(checksum(C)),
            beacon: None,
            ..row()
        };
        assert_eq!(entry_of(&transparent), Some(Node::base(C)));
        let confirmed = ProxyRecord {
            kind: "uups".into(),
            beacon: None,
            upgrade_path: Some("uups_owner".into()),
            ..row()
        };
        assert_eq!(entry_of(&confirmed), Some(Node::base(A)), "from itself");
        let unconfirmed = ProxyRecord {
            terminal_authority: None,
            upgrade_path: None,
            unresolved_reason: Some("uups_unconfirmed".into()),
            ..confirmed.clone()
        };
        assert_eq!(
            entry_of(&unconfirmed),
            None,
            "an unproven UUPS path is no path"
        );
        let not_covered = ProxyRecord {
            kind: "zeppelin_os".into(),
            ..row()
        };
        assert_eq!(entry_of(&not_covered), None);
    }

    #[test]
    fn a_direction_needs_two_known_numbers() {
        let unknown = Change {
            field: Field::KeysRequired,
            old: Some("1".into()),
            new: None,
        };
        assert_eq!(unknown.direction(), None, "null is not zero");
    }
}
