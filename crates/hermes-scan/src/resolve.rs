//! Collecting the authority probes that resolution walks.
//!
//! Probing is duck typing over `eth_call`: I call a selector and see whether anything sane
//! comes back. That makes decoding the boundary where a hostile contract gets to influence
//! me, so every value is bounded here rather than trusted downstream.
//!
//! Collection is breadth-first by design. Walking depth-first would interleave network round
//! trips with graph decisions and make the whole thing untestable; gathering a level at a
//! time keeps all the I/O here and leaves the graph logic pure.

use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::TransactionRequest;
use futures::stream::{self, StreamExt};
use hermes_core::{AuthorityProbe, MAX_DEPTH};
use std::collections::{HashMap, HashSet};

/// The most owners I will read off one contract.
///
/// A length prefix is just a number a stranger returned. Without a bound, `getOwners()`
/// claiming a billion entries is an allocation big enough to end the scan.
pub const MAX_OWNERS: usize = 256;

fn selector(signature: &str) -> [u8; 4] {
    let hash: B256 = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// Decode a 32-byte word as an address, rejecting anything with dirty upper bytes.
///
/// `Address::from_word` would silently take the low 20 bytes. A word with non-zero upper
/// bytes was not written by any contract I recognize, and truncating it fabricates an
/// authority out of whatever noise happened to be there.
fn word_to_address_strict(word: &[u8]) -> Option<Address> {
    if word.len() != 32 || word[..12].iter().any(|b| *b != 0) {
        return None;
    }
    let addr = Address::from_slice(&word[12..]);
    (!addr.is_zero()).then_some(addr)
}

fn word_to_u256(word: &[u8]) -> Option<U256> {
    (word.len() == 32).then(|| U256::from_be_slice(word))
}

/// Decode `address[]` return data.
///
/// The offset and the length both come from the callee, so both are checked against the
/// bytes actually present before anything is allocated.
fn decode_address_array(data: &[u8]) -> Option<Vec<Address>> {
    let offset = word_to_u256(data.get(..32)?)?;
    let offset: usize = offset.try_into().ok()?;
    let len_at = offset.checked_add(32)?;
    let len = word_to_u256(data.get(offset..len_at)?)?;
    let len: usize = len.try_into().ok()?;
    if len > MAX_OWNERS {
        return None;
    }
    let end = len_at.checked_add(len.checked_mul(32)?)?;
    let body = data.get(len_at..end)?;
    body.chunks_exact(32).map(word_to_address_strict).collect()
}

/// How many times to re-ask before accepting that I will not find out.
const RETRIES: u32 = 5;

/// What one `eth_call` established.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallOutcome {
    /// The contract returned data.
    Answered(Bytes),
    /// The contract does not have this function. A real, usable "no".
    NoAnswer,
    /// The node would not tell me. Not a "no", and must never be read as one.
    Undetermined,
}

impl CallOutcome {
    /// `None` when nothing was established, so `?` propagates "I do not know" rather than
    /// letting it decay into "the interface is absent".
    fn settled(self) -> Option<Option<Bytes>> {
        match self {
            Self::Answered(b) => Some(Some(b)),
            Self::NoAnswer => Some(None),
            Self::Undetermined => None,
        }
    }
}

/// The public endpoint answers `over rate limit` and then stays angry for a while, so the
/// waits have to get genuinely long rather than politely long.
async fn backoff(attempt: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(400u64 << attempt.min(6))).await;
}

/// Distinguish a contract saying "no such function" from the node saying nothing useful.
fn is_revert(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("execution reverted") || e.contains("invalid opcode") || e.contains("out of gas")
}

#[derive(Clone)]
pub struct AuthorityScanner {
    provider: DynProvider,
    concurrency: usize,
}

impl AuthorityScanner {
    pub fn new(provider: DynProvider, concurrency: usize) -> Self {
        Self {
            provider,
            concurrency,
        }
    }

    /// One `eth_call`.
    ///
    /// The three outcomes must stay distinct. Collapsing `Undetermined` into "no answer" is
    /// how a rate-limited `getOwners()` turns a Safe into an `Ownable` — the threshold
    /// vanishes, the owner set vanishes, and the key count silently drops to one.
    async fn call(&self, to: Address, sel: [u8; 4]) -> CallOutcome {
        let tx = TransactionRequest::default()
            .to(to)
            .input(Bytes::from(sel.to_vec()).into());
        for attempt in 0..RETRIES {
            match self.provider.call(tx.clone()).await {
                Ok(out) if !out.is_empty() => return CallOutcome::Answered(out),
                // An empty return is a contract answering without saying anything, which is
                // not the interface I asked about. Retrying that learns nothing.
                Ok(_) => return CallOutcome::NoAnswer,
                // A revert is the contract telling me the function is not there. Anything
                // else is the node failing, and confusing the two is what fabricates a
                // governance structure out of an outage.
                Err(e) if is_revert(&e.to_string()) => return CallOutcome::NoAnswer,
                Err(_) => backoff(attempt).await,
            }
        }
        CallOutcome::Undetermined
    }

    /// Whether an address has no code, or `None` when the node would not tell me.
    ///
    /// This distinction is the whole ballgame. A failed code read defaulting to "empty" would
    /// classify the address as an EOA — a *terminal* answer costing exactly one key. A rate
    /// limit would silently become the most alarming possible verdict, stated with High
    /// confidence, on an address I learned nothing about.
    async fn code_is_empty(&self, addr: Address) -> Option<bool> {
        for attempt in 0..RETRIES {
            if let Ok(code) = self.provider.get_code_at(addr).await {
                return Some(code.is_empty());
            }
            backoff(attempt).await;
        }
        None
    }

    /// Ask one address every question I know how to ask.
    ///
    /// `None` means I could not establish anything, and the address is deliberately left out
    /// of the probe map so resolution treats it as unknown rather than as an answer.
    ///
    /// The calls run one after another rather than concurrently. Firing all five at once
    /// multiplies the caller's concurrency limit by five, and the public endpoint starts
    /// answering `over rate limit` — which arrives here as "this interface is absent" and
    /// turns a Safe into an unresolved shrug.
    pub async fn probe(&self, addr: Address) -> Option<AuthorityProbe> {
        if self.code_is_empty(addr).await? {
            return Some(AuthorityProbe {
                code_empty: true,
                ..Default::default()
            });
        }
        let owners = self.call(addr, selector("getOwners()")).await.settled()?;
        let threshold = self
            .call(addr, selector("getThreshold()"))
            .await
            .settled()?;
        let owner = self.call(addr, selector("owner()")).await.settled()?;
        let min_delay = self.call(addr, selector("getMinDelay()")).await.settled()?;
        Some(AuthorityProbe {
            code_empty: false,
            owners: owners.and_then(|b| decode_address_array(&b)),
            // Saturating rather than truncating: a `u256 -> u32` cast that wraps turns
            // "needs four billion keys" into "needs three".
            threshold: threshold
                .and_then(|b| word_to_u256(&b))
                .map(|v| v.saturating_to::<u32>()),
            owner: owner.and_then(|b| word_to_address_strict(&b)),
            min_delay: min_delay
                .and_then(|b| word_to_u256(&b))
                .map(|v| v.saturating_to::<u64>()),
        })
    }

    /// Gather every probe reachable from `roots` within the depth limit.
    pub async fn collect(&self, roots: Vec<Address>) -> HashMap<Address, AuthorityProbe> {
        let mut probes: HashMap<Address, AuthorityProbe> = HashMap::new();
        let mut seen: HashSet<Address> = HashSet::new();
        let mut frontier: Vec<Address> = roots.into_iter().filter(|a| seen.insert(*a)).collect();

        for _ in 0..MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let level: Vec<(Address, AuthorityProbe)> = stream::iter(frontier.clone())
                .map(|a| async move { self.probe(a).await.map(|p| (a, p)) })
                .buffer_unordered(self.concurrency)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .flatten()
                .collect();

            frontier = level
                .iter()
                .flat_map(|(_, p)| children(p))
                .filter(|a| seen.insert(*a))
                .collect();
            probes.extend(level);
        }
        probes
    }
}

/// Addresses worth probing next, given what one address answered.
fn children(probe: &AuthorityProbe) -> Vec<Address> {
    let mut out = probe.owners.clone().unwrap_or_default();
    out.extend(probe.owner);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// Derived, not restated. A wrong selector silently answers "this interface is absent"
    /// for every contract on the chain, and every downstream test would still pass.
    #[test]
    fn selectors_match_their_signatures() {
        assert_eq!(selector("getOwners()"), [0xa0, 0xe6, 0x7e, 0x2b]);
        assert_eq!(selector("getThreshold()"), [0xe7, 0x52, 0x35, 0xb8]);
        assert_eq!(selector("owner()"), [0x8d, 0xa5, 0xcb, 0x5b]);
        assert_eq!(selector("getMinDelay()"), [0xf2, 0x7a, 0x0c, 0x92]);
    }

    fn word(hex_tail: &str) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        let bytes = alloy::hex::decode(hex_tail).unwrap();
        w[32 - bytes.len()..].copy_from_slice(&bytes);
        w
    }

    #[test]
    fn decodes_a_well_formed_owner_array() {
        let mut data = word("20"); // offset
        data.extend(word("02")); // length
        data.extend(word("00000000000000000000000000000000000000a1"));
        data.extend(word("00000000000000000000000000000000000000b2"));
        let owners = decode_address_array(&data).unwrap();
        assert_eq!(
            owners,
            vec![
                address!("00000000000000000000000000000000000000a1"),
                address!("00000000000000000000000000000000000000b2"),
            ]
        );
    }

    /// The classic unbounded-allocation blowup: a length prefix with no payload behind it.
    #[test]
    fn a_length_prefix_longer_than_the_payload_is_rejected() {
        let mut data = word("20");
        data.extend(word("05")); // claims five, supplies one
        data.extend(word("00000000000000000000000000000000000000a1"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn an_absurd_owner_count_is_refused_before_allocating() {
        let mut data = word("20");
        data.extend(word("ffffffffffffffff"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn an_offset_pointing_past_the_payload_is_rejected() {
        let mut data = word("ffffffff");
        data.extend(word("01"));
        assert_eq!(decode_address_array(&data), None);
    }

    #[test]
    fn truncated_and_empty_return_data_are_rejected() {
        assert_eq!(decode_address_array(&[]), None);
        assert_eq!(decode_address_array(&[0u8; 16]), None);
        assert_eq!(decode_address_array(&word("20")), None);
    }

    /// A word with dirty upper bytes was not written by a contract I recognize. Truncating to
    /// the low 20 bytes would fabricate an authority out of whatever noise was there.
    #[test]
    fn a_word_with_dirty_upper_bytes_is_not_an_address() {
        let mut w = word("00000000000000000000000000000000000000a1");
        w[0] = 0xff;
        assert_eq!(word_to_address_strict(&w), None);
    }

    #[test]
    fn the_zero_address_is_absent_not_an_owner() {
        assert_eq!(word_to_address_strict(&word("00")), None);
    }

    #[test]
    fn a_dirty_owner_entry_rejects_the_whole_array() {
        let mut data = word("20");
        data.extend(word("01"));
        let mut dirty = word("00000000000000000000000000000000000000a1");
        dirty[0] = 0x01;
        data.extend(dirty);
        assert_eq!(
            decode_address_array(&data),
            None,
            "one fabricated owner would change the key count"
        );
    }

    /// A node that will not answer must not decay into "the contract said no". This is the
    /// difference between reporting an unresolved authority and reporting a Safe as a
    /// single-key Ownable because a rate limiter ate `getOwners()`.
    #[test]
    fn an_undetermined_call_does_not_become_a_negative_answer() {
        assert_eq!(CallOutcome::Undetermined.settled(), None);
        assert_eq!(CallOutcome::NoAnswer.settled(), Some(None));
        let bytes = Bytes::from(vec![1u8]);
        assert_eq!(
            CallOutcome::Answered(bytes.clone()).settled(),
            Some(Some(bytes))
        );
    }

    #[test]
    fn a_revert_is_a_real_no_and_a_transport_failure_is_not() {
        assert!(is_revert("server returned an error: execution reverted"));
        assert!(is_revert("Execution Reverted"));
        assert!(is_revert("invalid opcode: INVALID"));
        assert!(!is_revert("over rate limit"));
        assert!(!is_revert("error sending request for url"));
        assert!(!is_revert("connection closed before message completed"));
        assert!(!is_revert("504 Gateway Timeout"));
    }

    #[test]
    fn children_are_the_owners_plus_the_owner() {
        let p = AuthorityProbe {
            owners: Some(vec![address!("00000000000000000000000000000000000000a1")]),
            owner: Some(address!("00000000000000000000000000000000000000b2")),
            ..Default::default()
        };
        assert_eq!(children(&p).len(), 2);
        assert!(children(&AuthorityProbe::default()).is_empty());
    }
}
