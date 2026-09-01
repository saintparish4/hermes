//! Proxy classification. Pure: no I/O, no clock, no network.
//!
//! Everything here is a total function of five storage words plus a code-emptiness flag,
//! which is what makes it exhaustively testable.

use crate::slots::word_to_address;
use alloy::primitives::{Address, B256};
use serde::{Deserialize, Serialize};

/// The raw storage words read from one address, before interpretation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotReads {
    pub implementation: B256,
    pub admin: B256,
    pub beacon: B256,
    pub proxiable: B256,
    pub zos_implementation: B256,
    /// `eth_getCode` returned 0 bytes — the address is an EOA, not a contract.
    pub code_empty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    /// Implementation and admin both set — OZ `TransparentUpgradeableProxy`.
    Transparent,
    /// Implementation set, admin slot empty — upgrade authority lives in the implementation.
    Uups,
    /// Beacon slot set — implementation is resolved through the beacon.
    Beacon,
    /// EIP-1822 `PROXIABLE` set but no ERC-1967 implementation.
    Eip1822,
    /// ERC-1967 **admin** slot set while the implementation slot is empty.
    ///
    /// Found on Base: most OP Stack predeploys sit at this state. The contract is a real
    /// ERC-1967 proxy whose implementation has simply never been pointed anywhere — and the
    /// admin can point it anywhere at any time. Reporting these as `NotUpgradeable` would
    /// drop a live upgrade authority on the floor, which is the exact understatement this
    /// tool exists to prevent.
    AdminOnly,
    /// Pre-1967 OpenZeppelin slot. Recorded, but **not** counted as covered in v1.
    ZeppelinOs,
    /// A contract with none of the above slots set.
    NotUpgradeable,
    /// No code at the address.
    Eoa,
}

impl ProxyKind {
    /// Whether v1 claims coverage of this pattern. Drives the coverage number, so it decides
    /// how honest that number is.
    pub fn is_covered_proxy(self) -> bool {
        matches!(
            self,
            Self::Transparent | Self::Uups | Self::Beacon | Self::Eip1822 | Self::AdminOnly
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transparent => "transparent",
            Self::Uups => "uups",
            Self::Beacon => "beacon",
            Self::Eip1822 => "eip1822",
            Self::AdminOnly => "admin_only",
            Self::ZeppelinOs => "zeppelin_os",
            Self::NotUpgradeable => "not_upgradeable",
            Self::Eoa => "eoa",
        }
    }
}

/// What a classification produced, including the addresses recovered from the slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub kind: ProxyKind,
    pub implementation: Option<Address>,
    pub admin: Option<Address>,
    pub beacon: Option<Address>,
}

/// Classify one address from its storage words.
///
/// Precedence is deliberate and load-bearing:
/// 1. No code wins over everything — an EOA has storage, but reading it is meaningless.
/// 2. A set implementation slot decides Transparent vs UUPS.
/// 3. Beacon is consulted only when the implementation slot is empty, because that is the
///    only configuration ERC-1967 actually defines for a beacon proxy. A contract with both
///    set is non-standard; I follow the implementation, which is what `delegatecall` uses.
/// 4. EIP-1822 is a fallback probe.
pub fn classify(reads: SlotReads) -> Classified {
    let implementation = word_to_address(reads.implementation);
    let admin = word_to_address(reads.admin);
    let beacon = word_to_address(reads.beacon);

    if reads.code_empty {
        return Classified {
            kind: ProxyKind::Eoa,
            implementation: None,
            admin: None,
            beacon: None,
        };
    }

    let kind = match (implementation, admin, beacon) {
        (Some(_), Some(_), _) => ProxyKind::Transparent,
        (Some(_), None, _) => ProxyKind::Uups,
        (None, _, Some(_)) => ProxyKind::Beacon,
        (None, Some(_), None) => ProxyKind::AdminOnly,
        (None, None, None) => {
            if word_to_address(reads.proxiable).is_some() || !reads.proxiable.is_zero() {
                ProxyKind::Eip1822
            } else if word_to_address(reads.zos_implementation).is_some() {
                ProxyKind::ZeppelinOs
            } else {
                ProxyKind::NotUpgradeable
            }
        }
    };

    Classified {
        kind,
        implementation,
        admin,
        beacon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};

    const ZERO: B256 = B256::ZERO;
    fn word(a: Address) -> B256 {
        a.into_word()
    }
    const A: Address = address!("6d9dd143e42b6338f4f6a7c0c26d124658f641cb");
    const B: Address = address!("31e99e05fee3dce580af777c3fd63ee1b3b40c17");

    fn reads() -> SlotReads {
        SlotReads::default()
    }

    #[test]
    fn implementation_and_admin_is_transparent() {
        let c = classify(SlotReads {
            implementation: word(A),
            admin: word(B),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Transparent);
        assert_eq!(c.implementation, Some(A));
        assert_eq!(c.admin, Some(B));
    }

    #[test]
    fn implementation_without_admin_is_uups() {
        let c = classify(SlotReads {
            implementation: word(A),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Uups);
        assert_eq!(c.admin, None);
    }

    #[test]
    fn beacon_only_is_beacon() {
        let c = classify(SlotReads {
            beacon: word(A),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Beacon);
        assert_eq!(c.beacon, Some(A));
    }

    #[test]
    fn beacon_with_admin_is_still_beacon() {
        let c = classify(SlotReads {
            beacon: word(A),
            admin: word(B),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Beacon);
        assert_eq!(
            c.admin,
            Some(B),
            "a beacon proxy can still carry an upgrade admin"
        );
    }

    #[test]
    fn implementation_wins_over_beacon_when_both_set() {
        // Non-standard, but delegatecall follows the implementation slot.
        let c = classify(SlotReads {
            implementation: word(A),
            beacon: word(B),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Uups);
    }

    #[test]
    fn proxiable_only_is_eip1822() {
        let c = classify(SlotReads {
            proxiable: word(A),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Eip1822);
    }

    #[test]
    fn zeppelinos_slot_is_recorded_but_not_covered() {
        let c = classify(SlotReads {
            zos_implementation: word(A),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::ZeppelinOs);
        assert!(
            !c.kind.is_covered_proxy(),
            "v1 must not claim coverage it does not have"
        );
    }

    #[test]
    fn all_slots_zero_with_code_is_not_upgradeable() {
        assert_eq!(classify(reads()).kind, ProxyKind::NotUpgradeable);
    }

    #[test]
    fn empty_code_is_eoa_regardless_of_slots() {
        let c = classify(SlotReads {
            implementation: word(A),
            admin: word(B),
            code_empty: true,
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::Eoa);
        assert_eq!(
            c.implementation, None,
            "EOA storage must not be reported as an implementation"
        );
    }

    #[test]
    fn admin_set_without_implementation_is_still_an_authority() {
        // 0x4200000000000000000000000000000000000001 on Base is exactly this shape, as are
        // most OP Stack predeploys: a real ERC-1967 proxy whose implementation was never set.
        let c = classify(SlotReads {
            admin: word(B),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::AdminOnly);
        assert_eq!(
            c.admin,
            Some(B),
            "the upgrade authority must survive classification"
        );
        assert!(
            c.kind.is_covered_proxy(),
            "an unset implementation does not make the admin powerless"
        );
    }

    #[test]
    fn covered_kinds_are_exactly_the_patterns_claimed() {
        for k in [
            ProxyKind::Transparent,
            ProxyKind::Uups,
            ProxyKind::Beacon,
            ProxyKind::Eip1822,
            ProxyKind::AdminOnly,
        ] {
            assert!(k.is_covered_proxy(), "{k:?} is a pattern I claim to cover");
        }
        for k in [
            ProxyKind::ZeppelinOs,
            ProxyKind::NotUpgradeable,
            ProxyKind::Eoa,
        ] {
            assert!(
                !k.is_covered_proxy(),
                "{k:?} is not a covered proxy pattern"
            );
        }
    }

    /// The real read from Base mainnet for 0x402E0d314fD6F55348Df7CC478bAb811826e3e91.
    #[test]
    fn regression_live_transparent_proxy_from_base() {
        let c = classify(SlotReads {
            implementation: b256!(
                "0000000000000000000000006d9dd143e42b6338f4f6a7c0c26d124658f641cb"
            ),
            admin: b256!("00000000000000000000000031e99e05fee3dce580af777c3fd63ee1b3b40c17"),
            beacon: ZERO,
            proxiable: ZERO,
            zos_implementation: ZERO,
            code_empty: false,
        });
        assert_eq!(c.kind, ProxyKind::Transparent);
        assert_eq!(c.implementation, Some(A));
        assert_eq!(c.admin, Some(B));
    }

    /// USDC on Base: every ERC-1967 slot is empty because it predates the standard.
    #[test]
    fn regression_usdc_on_base_is_not_erc1967() {
        let c = classify(SlotReads {
            zos_implementation: word(A),
            ..reads()
        });
        assert_eq!(c.kind, ProxyKind::ZeppelinOs);
        assert!(!c.kind.is_covered_proxy());
    }
}
