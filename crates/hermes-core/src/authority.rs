//! Turning an admin address into the governance structure behind it.
//!
//! Pure: everything here walks probes that were already collected from the chain. Keeping the
//! graph logic away from the I/O is what makes cycles, depth limits and the key arithmetic
//! testable without a network, and those are precisely the places edge cases hide.
//!
//! Every value a probe carries was returned by a contract a stranger deployed, so nothing in
//! here trusts its input structurally.

use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How many links I will walk before giving up on a chain.
///
/// This is a safety control, not a budget. The edges come from contracts I do not control, so
/// an unbounded walk is an abort waiting to be deployed against me.
pub const MAX_DEPTH: usize = 4;

/// What one address answered when probed for the interfaces I recognize.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityProbe {
    /// `eth_getCode` returned nothing, so this is an EOA.
    pub code_empty: bool,
    pub owners: Option<Vec<Address>>,
    pub threshold: Option<u32>,
    pub owner: Option<Address>,
    pub min_delay: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// No code. Terminal, and one key away from control.
    Eoa,
    /// Answered both `getOwners()` and `getThreshold()`. Terminal: a Safe is a governance
    /// structure in its own right, and its owners feed the key count rather than the chain.
    Safe,
    /// Answered `owner()`. The owner is the real authority, so the chain continues.
    Ownable,
    /// Answered `getMinDelay()`.
    Timelock,
    /// Nothing I recognize answered. Never guessed at.
    Unknown,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub terminal_authority: Address,
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
    /// Every address walked, starting at the admin.
    pub chain: Vec<Address>,
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
    if probe.code_empty {
        return (AuthorityKind::Eoa, false);
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

/// Where a walk stopped and why.
struct Stop {
    kind: AuthorityKind,
    confidence: Confidence,
    timelock_seconds: u64,
    truncated: bool,
    cycle: bool,
}

/// Follow ownership until something terminal, unrecognized, cyclic or too deep stops it.
fn walk(
    start: Address,
    probes: &HashMap<Address, AuthorityProbe>,
    chain: &mut Vec<Address>,
) -> Stop {
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
        chain.push(current);

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

        let next = match kind {
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
                probe.owner
            }
            AuthorityKind::Ownable => probe.owner,
        };

        match next {
            Some(addr) if chain.len() < MAX_DEPTH => current = addr,
            Some(_) => {
                stop.truncated = true;
                stop.confidence = stop.confidence.min(Confidence::Medium);
                return stop;
            }
            None => return stop,
        }
    }
}

/// Fewest distinct keys needed to exercise the authority at `addr`.
///
/// `None` means I could not determine it, which is a different answer from a large number and
/// must not be rendered as one.
fn keys_required(
    addr: Address,
    probes: &HashMap<Address, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Address>,
) -> Option<u32> {
    if depth > MAX_DEPTH || !seen.insert(addr) {
        return None;
    }
    let probe = probes.get(&addr)?;
    let cost = match classify(probe).0 {
        AuthorityKind::Eoa => Some(1),
        AuthorityKind::Safe => safe_keys_required(probe, probes, depth, seen),
        AuthorityKind::Ownable | AuthorityKind::Timelock => {
            keys_required(probe.owner?, probes, depth + 1, seen)
        }
        AuthorityKind::Unknown => None,
    };
    seen.remove(&addr);
    cost
}

/// An m-of-n Safe costs the sum of the **m cheapest** owners, not the first m in array order.
///
/// Taking array order is a plausible bug that produces plausible numbers, which is the worst
/// kind. If any owner's cost is unknown the whole total is unknown: an unrecognized owner
/// could be a single EOA, so reporting the cheapest m of the ones I do understand would
/// overstate how many keys an attacker actually needs.
fn safe_keys_required(
    probe: &AuthorityProbe,
    probes: &HashMap<Address, AuthorityProbe>,
    depth: usize,
    seen: &mut HashSet<Address>,
) -> Option<u32> {
    let owners = probe.owners.as_ref()?;
    let threshold = probe.threshold? as usize;
    if threshold == 0 || threshold > owners.len() {
        return None;
    }
    let mut costs = owners
        .iter()
        .map(|o| keys_required(*o, probes, depth + 1, seen))
        .collect::<Option<Vec<u32>>>()?;
    costs.sort_unstable();
    Some(
        costs
            .iter()
            .take(threshold)
            .fold(0u32, |acc, c| acc.saturating_add(*c)),
    )
}

/// Resolve an admin address to the authority that actually stands behind it.
pub fn resolve(admin: Address, probes: &HashMap<Address, AuthorityProbe>) -> Resolution {
    let mut chain = Vec::new();
    let stop = walk(admin, probes, &mut chain);
    let terminal = *chain.last().unwrap_or(&admin);

    // A truncated, cyclic or unrecognized chain has no trustworthy key count, and a number
    // here would read as a safety margin rather than as the guess it would be.
    let compromise_depth = if stop.truncated || stop.cycle || stop.confidence == Confidence::Unknown
    {
        None
    } else {
        keys_required(admin, probes, 0, &mut HashSet::new())
    };

    Resolution {
        terminal_authority: terminal,
        kind: stop.kind,
        compromise_depth,
        timelock_seconds: stop.timelock_seconds,
        confidence: stop.confidence,
        chain,
        truncated: stop.truncated,
        cycle: stop.cycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const A: Address = address!("00000000000000000000000000000000000000a1");
    const B: Address = address!("00000000000000000000000000000000000000b2");
    const C: Address = address!("00000000000000000000000000000000000000c3");
    const D: Address = address!("00000000000000000000000000000000000000d4");
    const E: Address = address!("00000000000000000000000000000000000000e5");

    fn eoa() -> AuthorityProbe {
        AuthorityProbe {
            code_empty: true,
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

    fn graph(entries: &[(Address, AuthorityProbe)]) -> HashMap<Address, AuthorityProbe> {
        entries.iter().cloned().collect()
    }

    #[test]
    fn an_eoa_admin_is_one_key_away() {
        let g = graph(&[(A, eoa())]);
        let r = resolve(A, &g);
        assert_eq!(r.terminal_authority, A);
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
        let from_a = resolve(A, &g);
        let from_b = resolve(B, &g);
        assert_eq!(from_a.terminal_authority, C);
        assert_eq!(from_b.terminal_authority, C);
        assert_eq!(
            from_a.terminal_authority, from_b.terminal_authority,
            "entry point must not change where a shared subgraph terminates"
        );
    }

    #[test]
    fn a_two_of_three_safe_over_eoas_costs_two_keys() {
        let g = graph(&[(A, safe(2, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        let r = resolve(A, &g);
        assert_eq!(r.kind, AuthorityKind::Safe);
        assert_eq!(r.compromise_depth, Some(2));
    }

    #[test]
    fn a_one_of_n_safe_costs_one_key() {
        let g = graph(&[(A, safe(1, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        assert_eq!(resolve(A, &g).compromise_depth, Some(1));
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
            resolve(A, &g).compromise_depth,
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
        assert_eq!(resolve(A, &g).compromise_depth, Some(3));
    }

    #[test]
    fn a_timelock_delay_is_captured_and_resolution_continues_through_it() {
        let g = graph(&[(A, timelock(172_800, Some(B))), (B, eoa())]);
        let r = resolve(A, &g);
        assert_eq!(r.timelock_seconds, 172_800);
        assert_eq!(r.terminal_authority, B);
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
        let r = resolve(A, &g);
        assert!(
            r.cycle,
            "the cycle must be reported, not inferred from a shrug"
        );
        assert_eq!(r.terminal_authority, A);
        assert_eq!(r.confidence, Confidence::Medium);
        assert_eq!(
            r.compromise_depth, None,
            "a cycle has no trustworthy key count"
        );
    }

    #[test]
    fn a_longer_cycle_also_terminates() {
        let g = graph(&[(A, ownable(B)), (B, ownable(C)), (C, ownable(A))]);
        let r = resolve(A, &g);
        assert!(r.cycle);
        assert!(r.chain.len() <= MAX_DEPTH);
    }

    #[test]
    fn a_chain_longer_than_the_cap_is_truncated_rather_than_followed() {
        let long = [A, B, C, D, E];
        let mut entries: Vec<_> = long
            .windows(2)
            .map(|w| (w[0], ownable(w[1])))
            .collect::<Vec<_>>();
        entries.push((E, eoa()));
        let r = resolve(A, &graph(&entries));
        assert!(r.truncated);
        assert_eq!(r.chain.len(), MAX_DEPTH);
        assert_ne!(r.confidence, Confidence::High);
        assert_eq!(r.compromise_depth, None);
    }

    #[test]
    fn an_unrecognized_node_makes_the_whole_resolution_unknown() {
        let g = graph(&[(A, ownable(B)), (B, AuthorityProbe::default())]);
        let r = resolve(A, &g);
        assert_eq!(r.kind, AuthorityKind::Unknown);
        assert_eq!(r.confidence, Confidence::Unknown);
        assert_eq!(r.compromise_depth, None);
        assert_eq!(r.terminal_authority, B);
    }

    #[test]
    fn an_address_that_was_never_probed_is_unknown_not_assumed() {
        let g = graph(&[(A, ownable(B))]);
        let r = resolve(A, &g);
        assert_eq!(r.confidence, Confidence::Unknown);
        assert_eq!(r.compromise_depth, None);
    }

    #[test]
    fn confidence_never_rises_along_a_chain() {
        // A timelock (Medium) sitting above a perfectly ordinary EOA must stay Medium.
        let g = graph(&[(A, ownable(B)), (B, timelock(100, Some(C))), (C, eoa())]);
        assert_eq!(resolve(A, &g).confidence, Confidence::Medium);
    }

    #[test]
    fn a_contract_answering_both_safe_and_timelock_probes_resolves_deterministically() {
        let mut both = safe(2, &[B, C]);
        both.min_delay = Some(600);
        let g = graph(&[(A, both), (B, eoa()), (C, eoa())]);
        let r = resolve(A, &g);
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
        assert_eq!(resolve(A, &g).compromise_depth, None);
    }

    #[test]
    fn a_zero_threshold_is_rejected() {
        let g = graph(&[(A, safe(0, &[B])), (B, eoa())]);
        assert_eq!(resolve(A, &g).compromise_depth, None);
    }

    #[test]
    fn an_enormous_threshold_saturates_instead_of_wrapping() {
        let owners: Vec<Address> = (0..3).map(|_| B).collect();
        let g = graph(&[(A, safe(u32::MAX, &owners)), (B, eoa())]);
        assert_eq!(
            resolve(A, &g).compromise_depth,
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
            if let Some(d) = resolve(A, g).compromise_depth {
                assert!(d >= 1, "control always costs at least one key");
            }
        }
    }

    #[test]
    fn owner_ordering_does_not_change_the_outcome() {
        let forward = graph(&[(A, safe(2, &[B, C, D])), (B, eoa()), (C, eoa()), (D, eoa())]);
        let reversed = graph(&[(A, safe(2, &[D, C, B])), (B, eoa()), (C, eoa()), (D, eoa())]);
        assert_eq!(
            resolve(A, &forward).compromise_depth,
            resolve(A, &reversed).compromise_depth
        );
    }
}
