//! Where the power to upgrade a proxy actually lives.
//!
//! The ERC-1967 admin slot is one of three places. A UUPS proxy has no admin at all:
//! `upgradeToAndCall` lives in the implementation and runs through the proxy, so whoever the
//! implementation's access control admits can upgrade it. A beacon proxy takes its
//! implementation from a beacon, so whoever controls the beacon upgrades every proxy pointing
//! at it in one transaction. Following only the admin slot left 23 of the first 58 covered
//! proxies without so much as an attempt.

use crate::authority::{Confidence, Unresolved};
use crate::chain::Node;
use crate::classify::{Classified, ProxyKind};
use alloy::primitives::Address;

/// The address a resolution walk starts from, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeEntry {
    /// The ERC-1967 admin: by the standard, the only caller the proxy lets upgrade it.
    AdminSlot(Address),
    /// The beacon every instance reads its implementation from.
    Beacon(Address),
    /// The proxy itself, asked `owner()` through its UUPS implementation.
    UupsOwner(Address),
}

impl UpgradeEntry {
    pub fn start(self) -> Node {
        match self {
            Self::AdminSlot(a) | Self::Beacon(a) | Self::UupsOwner(a) => Node::base(a),
        }
    }

    /// The most confidence a resolution through this entry can carry.
    ///
    /// A UUPS implementation answering `owner()` does not prove that `owner` is what
    /// `_authorizeUpgrade` checks; roles are just as common. So a root found that way is
    /// reported, and never as High. The admin slot and a beacon's own controller are where the
    /// upgrade call is actually made, which is the same footing the ProxyAdmin path has always
    /// stood on.
    pub fn ceiling(self) -> Confidence {
        match self {
            Self::AdminSlot(_) | Self::Beacon(_) => Confidence::High,
            Self::UupsOwner(_) => Confidence::Medium,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdminSlot(_) => "admin_slot",
            Self::Beacon(_) => "beacon",
            Self::UupsOwner(_) => "uups_owner",
        }
    }
}

/// The implementation to ask `proxiableUUID()`, when the answer decides the upgrade path.
pub fn uups_implementation(c: &Classified) -> Option<Address> {
    (c.kind == ProxyKind::Uups)
        .then_some(c.implementation)
        .flatten()
}

/// Where to start walking for one proxy.
///
/// `uups_confirmed` is what its implementation said to `proxiableUUID()`: `Some(true)` for the
/// ERC-1967 implementation slot, `Some(false)` for anything else including a revert, `None`
/// when the node would not tell me. `None` overall for a kind that is not a covered proxy,
/// since it has no upgrade path to explain.
pub fn upgrade_entry(
    proxy: Address,
    c: &Classified,
    uups_confirmed: Option<bool>,
) -> Option<Result<UpgradeEntry, Unresolved>> {
    let entry = match c.kind {
        ProxyKind::Transparent | ProxyKind::AdminOnly => c
            .admin
            .map(UpgradeEntry::AdminSlot)
            .ok_or(Unresolved::NoUpgradePath),
        ProxyKind::Beacon => c
            .beacon
            .map(UpgradeEntry::Beacon)
            .ok_or(Unresolved::NoUpgradePath),
        ProxyKind::Uups => match uups_confirmed {
            Some(true) => Ok(UpgradeEntry::UupsOwner(proxy)),
            Some(false) => Err(Unresolved::UupsUnconfirmed),
            None => Err(Unresolved::RpcUndetermined),
        },
        ProxyKind::Eip1822 => Err(Unresolved::NoUpgradePath),
        ProxyKind::ZeppelinOs | ProxyKind::NotUpgradeable | ProxyKind::Eoa => return None,
    };
    Some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const PROXY: Address = address!("00000000000000000000000000000000000000a1");
    const ADMIN: Address = address!("00000000000000000000000000000000000000b2");
    const BEACON: Address = address!("00000000000000000000000000000000000000c3");
    const IMPL: Address = address!("00000000000000000000000000000000000000d4");

    fn classified(kind: ProxyKind) -> Classified {
        Classified {
            kind,
            implementation: Some(IMPL),
            admin: matches!(kind, ProxyKind::Transparent | ProxyKind::AdminOnly).then_some(ADMIN),
            beacon: (kind == ProxyKind::Beacon).then_some(BEACON),
        }
    }

    #[test]
    fn an_admin_slot_is_walked_from_the_admin() {
        for kind in [ProxyKind::Transparent, ProxyKind::AdminOnly] {
            let entry = upgrade_entry(PROXY, &classified(kind), None)
                .unwrap()
                .unwrap();
            assert_eq!(entry, UpgradeEntry::AdminSlot(ADMIN));
            assert_eq!(entry.start(), Node::base(ADMIN));
            assert_eq!(entry.ceiling(), Confidence::High);
        }
    }

    #[test]
    fn a_beacon_proxy_is_walked_from_its_beacon_not_from_itself() {
        let entry = upgrade_entry(PROXY, &classified(ProxyKind::Beacon), None)
            .unwrap()
            .unwrap();
        assert_eq!(entry.start(), Node::base(BEACON));
    }

    /// A UUPS proxy is only walked once its implementation has said it is one. Without that,
    /// `owner()` answering through the proxy is a guess about where upgrades are checked.
    #[test]
    fn a_uups_proxy_is_walked_from_itself_only_once_confirmed() {
        let c = classified(ProxyKind::Uups);
        assert_eq!(uups_implementation(&c), Some(IMPL));
        let entry = upgrade_entry(PROXY, &c, Some(true)).unwrap().unwrap();
        assert_eq!(entry.start(), Node::base(PROXY));
        assert_eq!(
            entry.ceiling(),
            Confidence::Medium,
            "owner() answering does not prove owner() gates upgrades"
        );
        assert_eq!(
            upgrade_entry(PROXY, &c, Some(false)).unwrap(),
            Err(Unresolved::UupsUnconfirmed)
        );
        assert_eq!(
            upgrade_entry(PROXY, &c, None).unwrap(),
            Err(Unresolved::RpcUndetermined),
            "an unread proxiableUUID is an outage, not a finding"
        );
    }

    #[test]
    fn only_uups_proxies_need_the_proxiable_check() {
        for kind in [
            ProxyKind::Transparent,
            ProxyKind::Beacon,
            ProxyKind::AdminOnly,
        ] {
            assert_eq!(uups_implementation(&classified(kind)), None);
        }
    }

    #[test]
    fn a_pattern_outside_coverage_has_no_upgrade_path_to_explain() {
        for kind in [
            ProxyKind::ZeppelinOs,
            ProxyKind::NotUpgradeable,
            ProxyKind::Eoa,
        ] {
            assert_eq!(upgrade_entry(PROXY, &classified(kind), None), None);
        }
        assert_eq!(
            upgrade_entry(PROXY, &classified(ProxyKind::Eip1822), None),
            Some(Err(Unresolved::NoUpgradePath))
        );
    }
}
