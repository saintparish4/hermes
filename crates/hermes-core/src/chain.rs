//! Which chain an address lives on, and the L1→L2 alias that joins Base to Ethereum.
//!
//! On Base, "no code" does not mean "a key". When a contract on Ethereum sends a deposit
//! transaction, OP Stack presents it on L2 at its own address plus a fixed offset, and nothing
//! lives at that shifted address: no code, and no private key anyone holds. The only way to act
//! as it is to be the L1 contract. I once published exactly this kind of address as "one EOA
//! controls 20 contracts" when it was a 2-of-2 Safe on Ethereum eleven keys deep, so the
//! arithmetic lives here, pure and tested against that real pair.

use alloy::primitives::{Address, U160, address};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chain {
    Base,
    Ethereum,
}

impl Chain {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Ethereum => "ethereum",
        }
    }
}

/// An address together with the chain it lives on.
///
/// The same 20 bytes on Base and on Ethereum are two unrelated accounts. A Safe deployed with
/// the same salt on both can have different owners on each, so an address alone is not an
/// identity once the walk crosses chains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Node {
    pub chain: Chain,
    pub address: Address,
}

impl Node {
    pub fn base(address: Address) -> Self {
        Self {
            chain: Chain::Base,
            address,
        }
    }

    pub fn ethereum(address: Address) -> Self {
        Self {
            chain: Chain::Ethereum,
            address,
        }
    }
}

/// What OP Stack adds to an L1 contract's address when that contract acts on L2.
pub const L1_TO_L2_ALIAS_OFFSET: Address = address!("1111000000000000000000000000000000001111");

fn to_u160(a: Address) -> U160 {
    U160::from_be_bytes(a.into_array())
}

fn from_u160(v: U160) -> Address {
    Address::from(v.to_be_bytes::<20>())
}

/// The L2 address an L1 contract acts through. Addition wraps modulo 2^160, as it does in
/// `AddressAliasHelper`.
pub fn apply_l1_to_l2_alias(l1: Address) -> Address {
    from_u160(to_u160(l1).wrapping_add(to_u160(L1_TO_L2_ALIAS_OFFSET)))
}

/// The L1 address that would appear on L2 as `l2`.
///
/// Every L2 address has one. Whether a contract actually lives there is a question for L1,
/// and only a confirmed-empty answer lets `l2` be called a key.
pub fn undo_l1_to_l2_alias(l2: Address) -> Address {
    from_u160(to_u160(l2).wrapping_sub(to_u160(L1_TO_L2_ALIAS_OFFSET)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pair behind the headline correction: `ProxyAdmin.owner()` on Base, and the 2-of-2
    /// Safe on Ethereum it is the alias of.
    const BASE_PREDEPLOY_OWNER: Address = address!("8cC51c3008b3f03Fe483B28B8Db90e19cF076a6d");
    const L1_SAFE: Address = address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c");

    #[test]
    fn unaliasing_the_predeploy_owner_gives_the_l1_safe() {
        assert_eq!(undo_l1_to_l2_alias(BASE_PREDEPLOY_OWNER), L1_SAFE);
    }

    #[test]
    fn aliasing_the_l1_safe_gives_the_predeploy_owner() {
        assert_eq!(apply_l1_to_l2_alias(L1_SAFE), BASE_PREDEPLOY_OWNER);
    }

    /// Unchecked subtraction would panic or saturate here instead of wrapping, and an address
    /// near zero is exactly what a hostile deployer would pick to find that out.
    #[test]
    fn unaliasing_wraps_below_zero() {
        assert_eq!(
            undo_l1_to_l2_alias(Address::ZERO),
            address!("eeeeffffffffffffffffffffffffffffffffeeef")
        );
    }

    #[test]
    fn aliasing_wraps_past_the_top_of_the_address_space() {
        assert_eq!(
            apply_l1_to_l2_alias(Address::repeat_byte(0xff)),
            address!("1111000000000000000000000000000000001110")
        );
    }

    #[test]
    fn unaliasing_inverts_aliasing() {
        for a in [
            Address::ZERO,
            Address::repeat_byte(0xff),
            L1_SAFE,
            BASE_PREDEPLOY_OWNER,
            L1_TO_L2_ALIAS_OFFSET,
        ] {
            assert_eq!(undo_l1_to_l2_alias(apply_l1_to_l2_alias(a)), a);
            assert_eq!(apply_l1_to_l2_alias(undo_l1_to_l2_alias(a)), a);
        }
    }
}
