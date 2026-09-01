//! ERC-1967 / EIP-1822 storage slot constants.
//!
//! Each ERC-1967 slot is `bytes32(uint256(keccak256(name)) - 1)`. The `-1` exists so no
//! preimage of the slot is known, which guarantees the compiler will never allocate it.
//! EIP-1822's `PROXIABLE` slot is the bare `keccak256("PROXIABLE")` with **no** `-1`.
//!
//! All four constants below were verified against `web3_sha3` on Base mainnet.

use alloy::primitives::{Address, B256, U256, b256};

/// `keccak256("eip1967.proxy.implementation") - 1`
pub const IMPL_SLOT: B256 =
    b256!("360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");

/// `keccak256("eip1967.proxy.admin") - 1`
pub const ADMIN_SLOT: B256 =
    b256!("b53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103");

/// `keccak256("eip1967.proxy.beacon") - 1`
pub const BEACON_SLOT: B256 =
    b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");

/// `keccak256("PROXIABLE")` — EIP-1822. No `-1` offset.
pub const PROXIABLE_SLOT: B256 =
    b256!("c5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7");

/// `keccak256("org.zeppelinos.proxy.implementation")` — pre-1967 OpenZeppelin layout.
/// Not classified as a proxy in v1; recorded only so coverage reporting can name it.
pub const ZOS_IMPL_SLOT: B256 =
    b256!("7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3");

/// `eth_getStorageAt` takes the slot as a `U256` key, not a `B256`.
pub fn slot_key(slot: B256) -> U256 {
    U256::from_be_bytes(slot.0)
}

/// Interpret a storage word as an address: low 20 bytes, `None` when zero.
pub fn word_to_address(word: B256) -> Option<Address> {
    let addr = Address::from_word(word);
    (!addr.is_zero()).then_some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::keccak256;

    /// The ERC-1967 derivation, spelled out: `bytes32(uint256(keccak256(name)) - 1)`.
    fn erc1967_slot(name: &str) -> B256 {
        let hash = U256::from_be_bytes(keccak256(name.as_bytes()).0);
        B256::from((hash - U256::from(1)).to_be_bytes::<32>())
    }

    /// Re-derive rather than restate. Asserting a literal against the same literal proves
    /// nothing, and one wrong nibble here classifies every address on Base as
    /// `NotUpgradeable` while the scan, the coverage number and the rest of the suite all
    /// stay green — every downstream fixture was built from the same constant.
    #[test]
    fn erc1967_slots_are_the_keccak_hash_minus_one() {
        assert_eq!(IMPL_SLOT, erc1967_slot("eip1967.proxy.implementation"));
        assert_eq!(ADMIN_SLOT, erc1967_slot("eip1967.proxy.admin"));
        assert_eq!(BEACON_SLOT, erc1967_slot("eip1967.proxy.beacon"));
    }

    /// EIP-1822 defines the bare hash. The `assert_ne!` is the load-bearing half: a refactor
    /// that unifies all four constants behind one `slot_from_name()` helper would apply the
    /// `-1` here and detect zero EIP-1822 proxies, silently and forever.
    #[test]
    fn proxiable_slot_takes_no_offset() {
        assert_eq!(PROXIABLE_SLOT, keccak256(b"PROXIABLE"));
        assert_ne!(PROXIABLE_SLOT, erc1967_slot("PROXIABLE"));
    }

    /// Same shape as `proxiable_slot_takes_no_offset`, and the same refactor breaks it.
    #[test]
    fn zeppelinos_slot_takes_no_offset() {
        const NAME: &str = "org.zeppelinos.proxy.implementation";
        assert_eq!(ZOS_IMPL_SLOT, keccak256(NAME.as_bytes()));
        assert_ne!(ZOS_IMPL_SLOT, erc1967_slot(NAME));
    }

    /// Two patterns sharing a slot would let one mask the other during classification.
    #[test]
    fn every_slot_constant_is_distinct() {
        let slots = [
            IMPL_SLOT,
            ADMIN_SLOT,
            BEACON_SLOT,
            PROXIABLE_SLOT,
            ZOS_IMPL_SLOT,
        ];
        for (i, a) in slots.iter().enumerate() {
            for b in &slots[i + 1..] {
                assert_ne!(a, b, "slot constants must not collide");
            }
        }
    }

    #[test]
    fn slot_key_preserves_the_big_endian_bytes() {
        assert_eq!(slot_key(IMPL_SLOT).to_be_bytes::<32>(), IMPL_SLOT.0);
    }

    #[test]
    fn word_to_address_reads_the_low_twenty_bytes() {
        let expected = alloy::primitives::address!("6d9dd143e42b6338f4f6a7c0c26d124658f641cb");
        assert_eq!(word_to_address(expected.into_word()), Some(expected));
    }

    #[test]
    fn word_to_address_treats_zero_as_absent() {
        assert_eq!(word_to_address(B256::ZERO), None);
    }
}
