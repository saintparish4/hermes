//! The hand-verified table: what a handful of addresses must resolve to, as established by
//! reading the chain *without* Hermes.
//!
//! The expectations in `tests/verified.json` are written by a person from independent reads and
//! explorer pages, never copied from Hermes's own output. A table that learned its answers from
//! the program under test could not catch that program being wrong, and the aliasing error this
//! table was built after was exactly a confident wrong answer.

use alloy::primitives::Address;
use hermes_core::ProxyRecord;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedRow {
    /// Fixture file stem under `tests/fixtures/`.
    pub name: String,
    pub address: Address,
    pub label: String,
    /// The shape of authority this row exists to exercise.
    pub shape: String,
    pub expected: Expected,
    pub checked: Checked,
}

/// Every column Hermes publishes about a proxy's authority. `None` is a claim too: it says the
/// answer must be absent, so a row that expects "unresolved" fails if Hermes starts guessing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expected {
    pub kind: String,
    pub terminal_authority: Option<Address>,
    pub terminal_chain: Option<String>,
    pub authority_kind: Option<String>,
    pub compromise_depth: Option<i64>,
    pub timelock_seconds: Option<i64>,
    pub confidence: Option<String>,
}

/// How the expectation was established, so a failure can be re-checked by hand in a minute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checked {
    pub on: String,
    pub base_block: u64,
    pub ethereum_block: Option<u64>,
    pub how: String,
    pub urls: Vec<String>,
}

pub fn load_table(path: &Path) -> anyhow::Result<Vec<VerifiedRow>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// Every way `actual` differs from the hand-checked answer. Empty means it matches.
pub fn differences(expected: &Expected, actual: Option<&ProxyRecord>) -> Vec<String> {
    let Some(actual) = actual else {
        return vec!["the scan produced no verdict for this address".into()];
    };
    let mut out = Vec::new();
    let mut check = |field: &str, want: String, got: String| {
        if want != got {
            out.push(format!("{field}: expected {want}, got {got}"));
        }
    };
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "null".into());
    check("kind", expected.kind.clone(), actual.kind.clone());
    check(
        "terminal_authority",
        show(&expected.terminal_authority.map(|a| a.to_checksum(None))),
        show(
            &actual
                .terminal_authority
                .as_deref()
                .and_then(|a| a.parse::<Address>().ok())
                .map(|a| a.to_checksum(None)),
        ),
    );
    check(
        "terminal_chain",
        show(&expected.terminal_chain),
        show(&actual.terminal_chain),
    );
    check(
        "authority_kind",
        show(&expected.authority_kind),
        show(&actual.authority_kind),
    );
    check(
        "compromise_depth",
        show(&expected.compromise_depth.map(|d| d.to_string())),
        show(&actual.compromise_depth.map(|d| d.to_string())),
    );
    check(
        "timelock_seconds",
        show(&expected.timelock_seconds.map(|d| d.to_string())),
        show(&actual.timelock_seconds.map(|d| d.to_string())),
    );
    check(
        "confidence",
        show(&expected.confidence),
        show(&actual.resolution_confidence),
    );
    out
}

fn short(a: &Address) -> String {
    let s = a.to_checksum(None);
    format!("`{}…{}`", &s[..6], &s[s.len() - 4..])
}

/// The table as it appears in `docs/verification.md`. A test holds the document to this, so
/// the page a human reads cannot drift from the file the suite checks.
pub fn render_markdown(rows: &[VerifiedRow]) -> String {
    let mut out = String::from(
        "| # | Contract | Shape | Kind | Root | Chain | Keys | Timelock | Confidence | Checked at | Evidence |\n\
         |---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let dash = || "—".to_string();
    for (i, r) in rows.iter().enumerate() {
        let e = &r.expected;
        let blocks = match r.checked.ethereum_block {
            Some(l1) => format!("Base {} / L1 {}", r.checked.base_block, l1),
            None => format!("Base {}", r.checked.base_block),
        };
        let links = r
            .checked
            .urls
            .iter()
            .enumerate()
            .map(|(n, u)| format!("[{}]({u})", n + 1))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!(
            "| {} | {} {} | {} | `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
            i + 1,
            r.label,
            short(&r.address),
            r.shape,
            e.kind,
            e.terminal_authority
                .map(|a| {
                    let kind = e.authority_kind.as_deref().unwrap_or("?");
                    format!("{kind} {}", short(&a))
                })
                .unwrap_or_else(|| "unresolved".into()),
            e.terminal_chain.clone().unwrap_or_else(dash),
            e.compromise_depth
                .map(|d| d.to_string())
                .unwrap_or_else(|| if e.terminal_authority.is_some() {
                    "unknown".into()
                } else {
                    dash()
                }),
            e.timelock_seconds
                .map(|t| format!("{t}s"))
                .unwrap_or_else(dash),
            e.confidence.clone().unwrap_or_else(dash),
            blocks,
            links,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    fn record() -> ProxyRecord {
        ProxyRecord {
            address: "0x4200000000000000000000000000000000000010".into(),
            kind: "transparent".into(),
            terminal_authority: Some("0x7bb41c3008b3f03fe483b28b8db90e19cf07595c".into()),
            terminal_chain: Some("ethereum".into()),
            authority_kind: Some("safe".into()),
            compromise_depth: Some(11),
            timelock_seconds: Some(0),
            resolution_confidence: Some("high".into()),
            ..Default::default()
        }
    }

    fn expected() -> Expected {
        Expected {
            kind: "transparent".into(),
            terminal_authority: Some(address!("7bB41C3008B3f03FE483B28b8DB90e19Cf07595c")),
            terminal_chain: Some("ethereum".into()),
            authority_kind: Some("safe".into()),
            compromise_depth: Some(11),
            timelock_seconds: Some(0),
            confidence: Some("high".into()),
        }
    }

    #[test]
    fn a_matching_record_has_no_differences_whatever_the_address_case() {
        assert!(differences(&expected(), Some(&record())).is_empty());
    }

    /// The exact regression the table exists for: the old resolver's answer for the predeploys.
    #[test]
    fn the_pre_alias_answer_fails_on_every_column_it_got_wrong() {
        let mut old = record();
        old.terminal_authority = Some("0x8cC51c3008b3f03Fe483B28B8Db90e19cF076a6d".into());
        old.terminal_chain = Some("base".into());
        old.authority_kind = Some("eoa".into());
        old.compromise_depth = Some(1);
        let diffs = differences(&expected(), Some(&old));
        assert_eq!(diffs.len(), 4, "{diffs:?}");
    }

    /// Expecting "unresolved" is a claim. Hermes starting to guess must fail it.
    #[test]
    fn an_expected_null_fails_when_hermes_supplies_an_answer() {
        let unresolved = Expected {
            kind: "transparent".into(),
            ..Default::default()
        };
        assert_eq!(differences(&unresolved, Some(&record())).len(), 6);
    }

    #[test]
    fn a_missing_verdict_is_a_difference() {
        assert_eq!(differences(&expected(), None).len(), 1);
    }
}
