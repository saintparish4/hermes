//! A team's rules for who may upgrade its own contracts, checked against what Hermes resolved.
//!
//! Fails closed. An authority Hermes could not identify, or a key count or delay it could not
//! read, violates the policy unless the policy says in writing that unknown may pass. A check
//! that passes on "could not tell" is worse than no check, because it is the one that gets
//! trusted.

use crate::store::ProxyRecord;
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

/// The `[policy]` table of a policy file. Every rule is off unless set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Fail when the authority at the root is a single key.
    #[serde(default)]
    pub disallow_eoa_upgrade_admin: bool,
    /// The fewest keys an upgrade may take. Nested Safes count through: a 2-of-2 over two
    /// 3-of-5s is six, where its threshold alone would say two.
    pub minimum_keys_required: Option<u32>,
    /// The shortest delay a timelock must impose, as `24h`, `2d` or seconds.
    pub minimum_timelock: Option<String>,
    /// Whether the root may live on Ethereum, acting on Base through its L1 alias.
    #[serde(default = "yes")]
    pub allow_cross_chain_authority: bool,
    /// Whether the root may be an ERC-4337 account.
    #[serde(default = "yes")]
    pub allow_smart_account_authority: bool,
    /// Whether an answer Hermes could not establish may pass. Off unless written down.
    #[serde(default)]
    pub allow_unknown: bool,
}

/// A policy file: the `[policy]` table and nothing else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    pub policy: Policy,
}

/// Read a policy file. A misspelled rule is an error rather than a rule silently left off.
pub fn parse(text: &str) -> Result<Policy, String> {
    let file: PolicyFile = toml::from_str(text).map_err(|e| e.to_string())?;
    file.policy.minimum_timelock_seconds()?;
    Ok(file.policy)
}

/// `24h`, `2d`, `90m` or plain seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (n, unit) = match s.char_indices().last()? {
        (i, 'd') => (&s[..i], 86_400),
        (i, 'h') => (&s[..i], 3_600),
        (i, 'm') => (&s[..i], 60),
        (i, 's') => (&s[..i], 1),
        _ => (s, 1),
    };
    n.trim().parse::<u64>().ok()?.checked_mul(unit)
}

impl Policy {
    /// The policy's minimum delay in seconds, or an error naming the value it cannot read.
    pub fn minimum_timelock_seconds(&self) -> Result<Option<u64>, String> {
        self.minimum_timelock
            .as_deref()
            .map(|s| parse_duration(s).ok_or_else(|| format!("cannot read minimum_timelock {s:?}")))
            .transpose()
    }
}

/// One broken rule, stated as what the policy wanted and what the chain showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub rule: &'static str,
    pub expected: String,
    pub observed: String,
}

fn unknown(rule: &'static str, expected: String, why: &str) -> Violation {
    Violation {
        rule,
        expected,
        observed: format!("unknown ({why})"),
    }
}

/// Every rule `record` breaks. Empty means it passes.
pub fn evaluate(
    policy: &Policy,
    min_timelock: Option<u64>,
    record: &ProxyRecord,
) -> Vec<Violation> {
    let mut out = Vec::new();
    if !crate::classify::ProxyKind::is_covered_str(&record.kind) {
        if record.kind != "not_upgradeable" && !policy.allow_unknown {
            out.push(unknown(
                "upgrade_path",
                "an upgrade path Hermes can read".into(),
                &format!("{} is outside what Hermes classifies", record.kind),
            ));
        }
        return out;
    }
    let Some(root) = &record.terminal_authority else {
        if !policy.allow_unknown {
            out.push(unknown(
                "authority",
                "an identified upgrade authority".into(),
                record
                    .unresolved_reason
                    .as_deref()
                    .unwrap_or("not resolved"),
            ));
        }
        return out;
    };
    let kind = record.authority_kind.as_deref().unwrap_or("unknown");
    let chain = record.terminal_chain.as_deref().unwrap_or("base");
    if policy.disallow_eoa_upgrade_admin && kind == "eoa" {
        out.push(Violation {
            rule: "disallow_eoa_upgrade_admin",
            expected: "an authority that is not a single key".into(),
            observed: format!("key {root}"),
        });
    }
    if !policy.allow_cross_chain_authority && chain != "base" {
        out.push(Violation {
            rule: "allow_cross_chain_authority",
            expected: "an authority on Base".into(),
            observed: format!("{kind} {root} on {chain}"),
        });
    }
    if !policy.allow_smart_account_authority && kind == "smart_account" {
        out.push(Violation {
            rule: "allow_smart_account_authority",
            expected: "an authority that is not an ERC-4337 account".into(),
            observed: format!("smart account {root}"),
        });
    }
    if let Some(min) = policy.minimum_keys_required {
        let expected = format!("keys required >= {min}");
        match record.compromise_depth {
            Some(k) if k < i64::from(min) => out.push(Violation {
                rule: "minimum_keys_required",
                expected,
                observed: format!("keys required = {k}"),
            }),
            Some(_) => {}
            None if !policy.allow_unknown => out.push(unknown(
                "minimum_keys_required",
                expected,
                record
                    .depth_unknown_reason
                    .as_deref()
                    .unwrap_or("not counted"),
            )),
            None => {}
        }
    }
    if let Some(min) = min_timelock {
        let expected = format!("timelock >= {}", crate::time::duration(min));
        match record.timelock_seconds {
            Some(t) if (t as u64) < min => out.push(Violation {
                rule: "minimum_timelock",
                expected,
                observed: format!("timelock = {}", crate::time::duration(t as u64)),
            }),
            Some(_) => {}
            None if !policy.allow_unknown => {
                out.push(unknown("minimum_timelock", expected, "not read"));
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict() -> Policy {
        Policy {
            disallow_eoa_upgrade_admin: true,
            minimum_keys_required: Some(3),
            minimum_timelock: Some("24h".into()),
            allow_cross_chain_authority: false,
            allow_smart_account_authority: false,
            allow_unknown: false,
        }
    }

    fn resolved(kind: &str, keys: Option<i64>, delay: i64, chain: &str) -> ProxyRecord {
        ProxyRecord {
            address: "0xA".into(),
            kind: "transparent".into(),
            terminal_authority: Some("0xRoot".into()),
            terminal_chain: Some(chain.into()),
            authority_kind: Some(kind.into()),
            compromise_depth: keys,
            timelock_seconds: Some(delay),
            resolution_confidence: Some("high".into()),
            ..Default::default()
        }
    }

    fn rules(v: &[Violation]) -> Vec<&'static str> {
        v.iter().map(|x| x.rule).collect()
    }

    fn check(p: &Policy, r: &ProxyRecord) -> Vec<Violation> {
        evaluate(p, p.minimum_timelock_seconds().unwrap(), r)
    }

    #[test]
    fn a_three_key_safe_behind_a_two_day_timelock_passes_a_strict_policy() {
        assert!(check(&strict(), &resolved("safe", Some(3), 172_800, "base")).is_empty());
    }

    #[test]
    fn a_single_key_with_no_delay_breaks_every_rule_it_touches() {
        let v = check(&strict(), &resolved("eoa", Some(1), 0, "base"));
        assert_eq!(
            rules(&v),
            vec![
                "disallow_eoa_upgrade_admin",
                "minimum_keys_required",
                "minimum_timelock"
            ]
        );
        assert_eq!(v[2].expected, "timelock >= 24h");
        assert_eq!(v[2].observed, "timelock = none");
    }

    /// The fail-closed rule: an unresolved authority, or a key count Hermes could not read,
    /// fails unless the policy lets unknown pass in writing.
    #[test]
    fn unknown_fails_closed_unless_the_policy_says_otherwise() {
        let unresolved = ProxyRecord {
            address: "0xA".into(),
            kind: "uups".into(),
            unresolved_reason: Some("unrecognized_interface".into()),
            ..Default::default()
        };
        let v = check(&strict(), &unresolved);
        assert_eq!(rules(&v), vec!["authority"]);
        assert!(v[0].observed.contains("unrecognized_interface"));
        let lenient = Policy {
            allow_unknown: true,
            ..strict()
        };
        assert!(check(&lenient, &unresolved).is_empty());

        let uncounted = ProxyRecord {
            depth_unknown_reason: Some("owners_unknown".into()),
            ..resolved("safe", None, 172_800, "base")
        };
        assert_eq!(
            rules(&check(&strict(), &uncounted)),
            vec!["minimum_keys_required"]
        );
        assert!(check(&lenient, &uncounted).is_empty());
    }

    #[test]
    fn a_root_on_ethereum_and_a_smart_account_fail_only_when_disallowed() {
        let l1 = resolved("safe", Some(11), 172_800, "ethereum");
        assert_eq!(
            rules(&check(&strict(), &l1)),
            vec!["allow_cross_chain_authority"]
        );
        let open = Policy {
            allow_cross_chain_authority: true,
            ..strict()
        };
        assert!(check(&open, &l1).is_empty());
        let account = resolved("smart_account", Some(3), 172_800, "base");
        assert_eq!(
            rules(&check(&strict(), &account)),
            vec!["allow_smart_account_authority"]
        );
    }

    #[test]
    fn a_contract_nobody_can_upgrade_passes_and_one_hermes_cannot_classify_does_not() {
        let fixed = ProxyRecord {
            kind: "not_upgradeable".into(),
            ..Default::default()
        };
        assert!(check(&strict(), &fixed).is_empty());
        let zos = ProxyRecord {
            kind: "zeppelin_os".into(),
            ..Default::default()
        };
        assert_eq!(rules(&check(&strict(), &zos)), vec!["upgrade_path"]);
    }

    #[test]
    fn an_empty_policy_passes_everything_it_does_not_mention() {
        let empty = parse("[policy]\n").unwrap();
        assert!(check(&empty, &resolved("eoa", Some(1), 0, "ethereum")).is_empty());
    }

    #[test]
    fn the_documented_example_policy_reads_as_written() {
        let p = parse(
            "[policy]\n\
             disallow_eoa_upgrade_admin = true\n\
             minimum_keys_required = 3\n\
             minimum_timelock = \"24h\"\n\
             allow_cross_chain_authority = false\n",
        )
        .unwrap();
        assert_eq!(
            p,
            Policy {
                allow_smart_account_authority: true,
                ..strict()
            }
        );
    }

    #[test]
    fn a_misspelled_rule_or_an_unreadable_delay_is_refused_rather_than_ignored() {
        let typo = parse("[policy]\nminimum_key_required = 3\n");
        assert!(typo.unwrap_err().contains("minimum_key_required"));
        assert!(parse("[policy]\nminimum_timelock = \"a while\"\n").is_err());
        assert!(
            parse("minimum_keys_required = 3\n").is_err(),
            "outside [policy]"
        );
    }

    #[test]
    fn durations_read_the_way_people_write_them() {
        assert_eq!(parse_duration("24h"), Some(86_400));
        assert_eq!(parse_duration("2d"), Some(172_800));
        assert_eq!(parse_duration("90m"), Some(5_400));
        assert_eq!(parse_duration("3600"), Some(3_600));
        assert_eq!(parse_duration("soon"), None);
        let bad = Policy {
            minimum_timelock: Some("a while".into()),
            ..strict()
        };
        assert!(bad.minimum_timelock_seconds().is_err());
    }
}
