//! Whether a past upgrade was made by the authority Hermes would have named the block before.
//!
//! The rest of Hermes reasons forward: here is the graph, so here is who could act. This runs
//! the other way. The chain recorded who did act, in the transaction that emitted `Upgraded`,
//! and that is evidence nothing in Hermes produced. An upgrade made by someone outside the
//! authority Hermes named is a false negative no forward check can find.
//!
//! What a receipt shows is partial, so there are three answers. Consistent: an actor the
//! receipt names is inside the named authority, and where it signed, it signed as an owner, at
//! the threshold. Contradicted: an actor outside it moved the proxy. Ambiguous: the receipt does
//! not say who decided, which is what a call trace would be for.
//!
//! Pure: walks stored probes and what a receipt was decoded into.

use crate::authority::{AuthorityKind, AuthorityProbe, Code, DepthGap, Resolution, authority_kind};
use crate::blast::reachable;
use crate::chain::Node;
use alloy::primitives::Address;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// One Safe transaction the receipt shows executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SafeExecution {
    pub safe: Address,
    /// The owners whose signatures the call carried, when the transaction called the Safe
    /// directly and every signature could be attributed. `None` otherwise.
    pub signers: Option<Vec<Address>>,
}

/// Who a transaction shows acting, decoded from its receipt and input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Observed {
    /// The account that sent the transaction. For a relayed Safe execution or a bundled user
    /// operation this is whoever paid gas, which proves nothing.
    pub sender: Address,
    pub safe_executions: Vec<SafeExecution>,
    /// `ExecutionFromModuleSuccess`: (Safe, module).
    pub module_executions: Vec<(Address, Address)>,
    /// `UserOperationEvent` senders: the smart accounts that acted.
    pub account_operations: Vec<Address>,
    /// `CallExecuted` emitters: timelocks that executed.
    pub timelock_executions: Vec<Address>,
    /// Every address that emitted a log in the transaction. An EIP-7702 account that emits ran
    /// its delegate's code as itself, which is how a relayer submits what its key signed.
    #[serde(default)]
    pub emitters: Vec<Address>,
    /// For a root that is an `AccessControl` contract not listing its members: whether the
    /// sender held its upgrade role or that role's admin role the block before, asked of the
    /// contract with `hasRole`. `None` when not asked or not answered.
    pub sender_held_root_role: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Judgement {
    /// Hermes named no root the block before, so there was no claim to test.
    Unresolved,
    Consistent(Why),
    Contradicted(Why),
    Ambiguous(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// A key in the named authority sent it.
    KeySent,
    /// A Safe in the named authority executed it, signed by at least its threshold of owners.
    SafeSigned,
    /// A Safe in the named authority executed it; the signatures were not attributable.
    SafeExecuted,
    /// A Safe in the named authority executed it through a module the probe had seen.
    SafeModule,
    /// A smart account in the named authority executed it as a user operation.
    AccountOperation,
    /// A timelock in the named authority executed it.
    TimelockExecuted,
    /// The sender held the root's upgrade role, or its admin role, the block before.
    RoleHolderSent,
    /// A key in the named authority, delegated under EIP-7702, executed as itself: a relayer
    /// sent what the key authorized through its delegate.
    DelegatedKeyExecuted,
    /// A Safe in the named authority executed it, but signed by addresses that were not its
    /// owners, or fewer than its threshold, the block before.
    SignersNotTheOwners,
    /// A Safe in the named authority executed it through a module the probe did not show.
    ModuleNotSeen,
    /// Nothing in the named authority acted, and the transaction was sent by an account outside
    /// it.
    ActorOutsideAuthority,
    /// The named authority was admittedly incomplete the block before: the walk stopped, or
    /// some members or doors were never read. "Outside" may only be "among what I said I had
    /// not read".
    AuthorityIncomplete,
}

/// Whether a root's key count was unknown because part of the authority went unread, so an
/// actor outside what was read is not evidence against it. `no_known_key` is the exception:
/// it says nobody holds a key, and somebody upgrading contradicts exactly that.
fn admittedly_incomplete(r: &Resolution) -> bool {
    match r.depth_gap() {
        None | Some(DepthGap::NoKnownKey) | Some(DepthGap::SharedSigners) => false,
        Some(
            DepthGap::Truncated
            | DepthGap::Cycle
            | DepthGap::OwnersUnknown
            | DepthGap::RolesUnread
            | DepthGap::AccountKeysUnread
            | DepthGap::SafeSetupUnread
            | DepthGap::SafeModules
            | DepthGap::SafeGuard
            | DepthGap::PendingOwnerUnread,
        ) => true,
    }
}

/// Judge one upgrade against the resolution made at the block before it.
pub fn judge(
    entry: Node,
    probes: &HashMap<Node, AuthorityProbe>,
    resolution: Option<&Resolution>,
    seen: &Observed,
) -> Judgement {
    let Some(resolution) = resolution.filter(|r| r.unresolved().is_none()) else {
        return Judgement::Unresolved;
    };
    let tree: HashSet<Node> = reachable(entry, probes);
    let inside = |a: &Address| tree.contains(&Node::base(*a));
    // A node can be in the tree without a probe of its own: an edge target never read.
    if let Some(exec) = seen.safe_executions.iter().find(|e| inside(&e.safe)) {
        let probe = probes.get(&Node::base(exec.safe));
        return match (&exec.signers, probe) {
            (Some(signers), Some(p)) if signed_as_owners(p, signers) => {
                Judgement::Consistent(Why::SafeSigned)
            }
            (Some(_), Some(_)) => Judgement::Contradicted(Why::SignersNotTheOwners),
            _ => Judgement::Consistent(Why::SafeExecuted),
        };
    }
    if let Some((safe, module)) = seen.module_executions.iter().find(|(s, _)| inside(s)) {
        let seen_module = probes
            .get(&Node::base(*safe))
            .and_then(|p| p.safe_setup.as_ref())
            .is_some_and(|s| s.modules.contains(module));
        return if seen_module {
            Judgement::Consistent(Why::SafeModule)
        } else {
            Judgement::Contradicted(Why::ModuleNotSeen)
        };
    }
    if seen.account_operations.iter().any(inside) {
        return Judgement::Consistent(Why::AccountOperation);
    }
    if seen.timelock_executions.iter().any(inside) {
        return Judgement::Consistent(Why::TimelockExecuted);
    }
    let delegated_key_acted = seen.emitters.iter().any(|a| {
        let n = Node::base(*a);
        tree.contains(&n)
            && probes
                .get(&n)
                .is_some_and(|p| matches!(p.code, Code::Delegated(_)))
    });
    if delegated_key_acted {
        return Judgement::Consistent(Why::DelegatedKeyExecuted);
    }
    let sender = Node::base(seen.sender);
    let sender_is_named_key = tree.contains(&sender)
        && probes
            .get(&sender)
            .is_some_and(|p| authority_kind(sender, p) == AuthorityKind::Eoa);
    if sender_is_named_key {
        return Judgement::Consistent(Why::KeySent);
    }
    if seen.sender_held_root_role == Some(true) {
        return Judgement::Consistent(Why::RoleHolderSent);
    }
    if admittedly_incomplete(resolution) {
        return Judgement::Ambiguous(Why::AuthorityIncomplete);
    }
    let anything_acted = !seen.safe_executions.is_empty()
        || !seen.module_executions.is_empty()
        || !seen.account_operations.is_empty()
        || !seen.timelock_executions.is_empty();
    if anything_acted {
        // Something executed, just not anything I named. Without a trace I cannot say whether
        // it stood between the sender and the proxy.
        Judgement::Ambiguous(Why::ActorOutsideAuthority)
    } else {
        Judgement::Contradicted(Why::ActorOutsideAuthority)
    }
}

/// Whether every signer was an owner and there were at least threshold of them.
fn signed_as_owners(probe: &AuthorityProbe, signers: &[Address]) -> bool {
    let (Some(owners), Some(threshold)) = (&probe.owners, probe.threshold) else {
        return false;
    };
    let distinct: HashSet<&Address> = signers.iter().collect();
    distinct.len() >= threshold as usize && distinct.iter().all(|s| owners.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{CANONICAL_SAFE_SINGLETONS, PendingOwner, SafeSetup, resolve};
    use alloy::primitives::address;

    const ADMIN: Address = address!("00000000000000000000000000000000000000a0");
    const SAFE: Address = address!("00000000000000000000000000000000000000c1");
    const K1: Address = address!("00000000000000000000000000000000000000a1");
    const K2: Address = address!("00000000000000000000000000000000000000a2");
    const OUTSIDER: Address = address!("00000000000000000000000000000000000000ee");
    const MODULE: Address = address!("00000000000000000000000000000000000000d1");

    fn graph(modules: Vec<Address>) -> HashMap<Node, AuthorityProbe> {
        let key = || AuthorityProbe {
            code: Code::Absent,
            ..Default::default()
        };
        HashMap::from([
            (
                Node::base(ADMIN),
                AuthorityProbe {
                    owner: Some(SAFE),
                    pending_owner: Some(PendingOwner::Nobody),
                    ..Default::default()
                },
            ),
            (
                Node::base(SAFE),
                AuthorityProbe {
                    owners: Some(vec![K1, K2]),
                    threshold: Some(2),
                    safe_setup: Some(SafeSetup {
                        modules,
                        guard: None,
                        singleton: Some(CANONICAL_SAFE_SINGLETONS[0]),
                    }),
                    ..Default::default()
                },
            ),
            (Node::base(K1), key()),
            (Node::base(K2), key()),
            (Node::base(OUTSIDER), key()),
        ])
    }

    fn run(g: &HashMap<Node, AuthorityProbe>, seen: &Observed) -> Judgement {
        let r = resolve(Node::base(ADMIN), g);
        judge(Node::base(ADMIN), g, Some(&r), seen)
    }

    fn executed(signers: Option<Vec<Address>>) -> Observed {
        Observed {
            sender: OUTSIDER,
            safe_executions: vec![SafeExecution {
                safe: SAFE,
                signers,
            }],
            ..Default::default()
        }
    }

    /// A relayer paying gas for the named Safe is the Safe acting, not the relayer.
    #[test]
    fn a_relayed_execution_by_the_named_safe_at_its_threshold_is_consistent() {
        let g = graph(vec![]);
        assert_eq!(
            run(&g, &executed(Some(vec![K1, K2]))),
            Judgement::Consistent(Why::SafeSigned)
        );
        assert_eq!(
            run(&g, &executed(None)),
            Judgement::Consistent(Why::SafeExecuted)
        );
    }

    #[test]
    fn signatures_short_of_the_threshold_or_from_strangers_contradict_the_reading() {
        let g = graph(vec![]);
        assert_eq!(
            run(&g, &executed(Some(vec![K1]))),
            Judgement::Contradicted(Why::SignersNotTheOwners)
        );
        assert_eq!(
            run(&g, &executed(Some(vec![K1, OUTSIDER]))),
            Judgement::Contradicted(Why::SignersNotTheOwners)
        );
    }

    /// The case model 4 exists for, seen from the other side: a module the probe did not show
    /// moving the proxy is the count missing a door.
    #[test]
    fn a_module_execution_is_consistent_only_when_the_probe_saw_the_module() {
        let seen = Observed {
            sender: OUTSIDER,
            module_executions: vec![(SAFE, MODULE)],
            ..Default::default()
        };
        assert_eq!(
            run(&graph(vec![MODULE]), &seen),
            Judgement::Consistent(Why::SafeModule)
        );
        assert_eq!(
            run(&graph(vec![]), &seen),
            Judgement::Contradicted(Why::ModuleNotSeen)
        );
    }

    #[test]
    fn a_sender_outside_the_named_authority_with_nothing_executing_is_a_contradiction() {
        let seen = Observed {
            sender: OUTSIDER,
            ..Default::default()
        };
        assert_eq!(
            run(&graph(vec![]), &seen),
            Judgement::Contradicted(Why::ActorOutsideAuthority)
        );
    }

    #[test]
    fn a_named_key_sending_is_consistent_and_an_unresolved_row_tests_nothing() {
        let mut g = graph(vec![]);
        g.get_mut(&Node::base(ADMIN)).unwrap().owner = Some(K1);
        let seen = Observed {
            sender: K1,
            ..Default::default()
        };
        assert_eq!(run(&g, &seen), Judgement::Consistent(Why::KeySent));
        assert_eq!(
            judge(Node::base(ADMIN), &g, None, &seen),
            Judgement::Unresolved
        );
    }

    /// Found on the first live run (0xCb56…2086 at 22,500,009): an `AccessControl` root whose
    /// holders are not listed, upgraded by one of them. Hermes said it had not read the holders,
    /// so an unlisted sender is not a contradiction.
    #[test]
    fn an_outside_sender_under_an_authority_with_unread_members_is_ambiguous() {
        let mut g = graph(vec![]);
        g.insert(
            Node::base(ADMIN),
            AuthorityProbe {
                roles: Some(crate::authority::RoleGate {
                    members: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let mut seen = Observed {
            sender: OUTSIDER,
            ..Default::default()
        };
        assert_eq!(
            run(&g, &seen),
            Judgement::Ambiguous(Why::AuthorityIncomplete)
        );
        seen.sender_held_root_role = Some(true);
        assert_eq!(run(&g, &seen), Judgement::Consistent(Why::RoleHolderSent));
    }

    /// Found on the first live run (0xFEc3…5f9a and 0x1339…6F93 at 36,500,016): the owner is
    /// an EIP-7702 key, its delegate's `executeWithSig` was submitted by a relayer, and the key's
    /// account emitted. That is the named key acting, not someone outside it.
    #[test]
    fn a_relayed_call_executed_by_a_named_delegated_key_is_consistent() {
        let delegate = address!("00000000000000000000000000000000000000de");
        let mut g = graph(vec![]);
        g.get_mut(&Node::base(ADMIN)).unwrap().owner = Some(K1);
        g.insert(
            Node::base(K1),
            AuthorityProbe {
                code: Code::Delegated(delegate),
                ..Default::default()
            },
        );
        let mut seen = Observed {
            sender: OUTSIDER,
            ..Default::default()
        };
        assert_eq!(
            run(&g, &seen),
            Judgement::Contradicted(Why::ActorOutsideAuthority)
        );
        seen.emitters = vec![K1];
        assert_eq!(
            run(&g, &seen),
            Judgement::Consistent(Why::DelegatedKeyExecuted)
        );
    }

    /// The other side: a root Hermes said no one holds a key for, upgraded by someone.
    #[test]
    fn an_upgrade_under_a_root_with_no_known_key_is_a_contradiction() {
        let dead = address!("000000000000000000000000000000000000dEaD");
        let mut g = graph(vec![]);
        g.get_mut(&Node::base(ADMIN)).unwrap().owner = Some(dead);
        g.insert(
            Node::base(dead),
            AuthorityProbe {
                code: Code::Absent,
                ..Default::default()
            },
        );
        let seen = Observed {
            sender: OUTSIDER,
            ..Default::default()
        };
        assert_eq!(
            run(&g, &seen),
            Judgement::Contradicted(Why::ActorOutsideAuthority)
        );
    }

    /// An outside Safe executing could be standing between the sender and the proxy; only a
    /// trace says, so it is not called a contradiction.
    #[test]
    fn an_unnamed_contract_executing_is_ambiguous_not_contradicted() {
        let other_safe = address!("00000000000000000000000000000000000000c9");
        let seen = Observed {
            sender: OUTSIDER,
            safe_executions: vec![SafeExecution {
                safe: other_safe,
                signers: None,
            }],
            ..Default::default()
        };
        assert_eq!(
            run(&graph(vec![]), &seen),
            Judgement::Ambiguous(Why::ActorOutsideAuthority)
        );
    }
}
