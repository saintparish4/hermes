//! `hermes check`: a team's deployments against its authority policy, as a CI step.
//!
//! Reads a list of addresses, resolves each one, evaluates the policy, prints each violation as
//! what was expected, what was observed and the upgrade path that produced it, and exits 1 on
//! any violation. An address Hermes could not read is a violation too: the check fails closed.

use hermes_core::graph::replay;
use hermes_core::policy::{self, Policy, Violation};
use hermes_core::{AuthorityProbe, Node, ProxyRecord, Store};
use hermes_scan::{AuthorityScanner, Scanner, Target, scan_and_resolve};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Address(String),
    Named {
        name: Option<String>,
        address: String,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Deployments {
    List(Vec<Entry>),
    Wrapped { contracts: Vec<Entry> },
}

/// `["0x…"]`, `[{"name": "Vault", "address": "0x…"}]`, or either under `{"contracts": …}`.
fn read_deployments(path: &Path) -> anyhow::Result<Vec<(Option<String>, String)>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let entries = match serde_json::from_str::<Deployments>(&text)? {
        Deployments::List(e) | Deployments::Wrapped { contracts: e } => e,
    };
    Ok(entries
        .into_iter()
        .map(|e| match e {
            Entry::Address(a) => (None, a),
            Entry::Named { name, address } => (name, address),
        })
        .collect())
}

pub fn read_policy(path: &Path) -> anyhow::Result<Policy> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    policy::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// One contract's result.
#[derive(Serialize)]
struct Checked {
    name: Option<String>,
    address: String,
    /// What Hermes resolved, or `None` when it could not read the contract at all.
    resolution: Option<ProxyRecord>,
    /// Every node walked from the upgrade entry to the root.
    path: Vec<Node>,
    violations: Vec<Violation>,
}

/// Where the answers come from.
pub enum Source<'a> {
    /// A fresh scan at pinned blocks: what CI wants.
    Live(&'a Scanner, &'a AuthorityScanner),
    /// The database a previous scan wrote.
    Db(&'a Store),
}

async fn resolve_one(
    source: &Source<'_>,
    address: &str,
) -> anyhow::Result<Option<(ProxyRecord, HashMap<Node, AuthorityProbe>)>> {
    match source {
        Source::Live(scanner, authorities) => {
            let target = Target {
                address: address
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{address} is not an address"))?,
                label: None,
            };
            let scanned = scan_and_resolve(scanner, authorities, &[target], 0).await;
            Ok(scanned
                .records
                .into_iter()
                .next()
                .map(|r| (r, scanned.probes)))
        }
        Source::Db(store) => {
            let Some(row) = store.get_proxy(address).await? else {
                return Ok(None);
            };
            let probes = match hermes_core::graph::entry_of(&row) {
                Some(entry) => store.probes_from(&[entry]).await?,
                None => HashMap::new(),
            };
            Ok(Some((row, probes)))
        }
    }
}

fn describe_path(record: &ProxyRecord, path: &[Node]) -> String {
    let mut steps = vec![format!("{} ({})", record.address, record.kind)];
    steps.extend(path.iter().map(|n| match n.chain {
        hermes_core::Chain::Base => n.address.to_checksum(None),
        hermes_core::Chain::Ethereum => format!("{} (ethereum)", n.address.to_checksum(None)),
    }));
    steps.dedup();
    steps.join(" → ")
}

fn print(c: &Checked) {
    let name = c
        .name
        .as_deref()
        .map_or(String::new(), |n| format!("{n}  "));
    let Some(r) = &c.resolution else {
        println!("FAIL  {name}{}", c.address);
        for v in &c.violations {
            println!("      {}", v.observed);
        }
        return;
    };
    let summary = match &r.terminal_authority {
        Some(root) => format!(
            "{} {root} · keys {} · timelock {}",
            crate::inspect::kind_name(r.authority_kind.as_deref().unwrap_or("?")),
            r.compromise_depth
                .map_or("unknown".into(), |k| k.to_string()),
            r.timelock_seconds
                .map_or("unknown".into(), |t| hermes_core::time::duration(t as u64))
        ),
        None => format!(
            "no root: {}",
            r.unresolved_reason.as_deref().unwrap_or(&r.kind)
        ),
    };
    let verdict = if c.violations.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };
    println!("{verdict}  {name}{}  {summary}", c.address);
    for v in &c.violations {
        println!("      rule      {}", v.rule);
        println!("      expected  {}", v.expected);
        println!("      observed  {}", v.observed);
    }
    if !c.violations.is_empty() {
        println!("      path      {}", describe_path(r, &c.path));
    }
}

pub async fn check(
    deployments: &Path,
    policy_path: &Path,
    source: Source<'_>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let policy = read_policy(policy_path)?;
    let min_timelock = policy
        .minimum_timelock_seconds()
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut results = Vec::new();
    for (name, address) in read_deployments(deployments)? {
        let checked = match resolve_one(&source, &address).await? {
            Some((record, probes)) => Checked {
                violations: policy::evaluate(&policy, min_timelock, &record),
                path: replay(&record, &probes)
                    .map(|(_, r)| r.path)
                    .unwrap_or_default(),
                resolution: Some(record),
                name,
                address,
            },
            None => Checked {
                violations: vec![Violation {
                    rule: "readable",
                    expected: "a contract Hermes can read".into(),
                    observed: "no verdict: the scan could not read it, or it is not in the \
                               database"
                        .into(),
                }],
                resolution: None,
                path: Vec::new(),
                name,
                address,
            },
        };
        results.push(checked);
    }
    let failed = results.iter().filter(|c| !c.violations.is_empty()).count();
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        println!("policy {}\n", policy_path.display());
        results.iter().for_each(print);
        println!(
            "\n{} of {} contracts violate the policy",
            failed,
            results.len()
        );
    }
    Ok(if failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("hermes-check-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn deployments_read_as_a_list_named_entries_or_a_wrapped_list() {
        let a = "0x9Caa0e7277ce86A4644F2D10b72561080531b674";
        for text in [
            format!(r#"["{a}"]"#),
            format!(r#"[{{"name": "Vault", "address": "{a}"}}]"#),
            format!(r#"{{"contracts": [{{"name": "Vault", "address": "{a}"}}]}}"#),
        ] {
            let path = write("deployments.json", &text);
            let read = read_deployments(&path).unwrap();
            assert_eq!(read.len(), 1);
            assert_eq!(read[0].1, a);
            let _ = std::fs::remove_file(path);
        }
    }

    /// The examples the README and the Action point at have to stay valid.
    #[test]
    fn the_documented_examples_parse() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/check");
        let policy = read_policy(&docs.join("policy.toml")).unwrap();
        assert_eq!(policy.minimum_keys_required, Some(3));
        assert!(!policy.allow_unknown, "the example must show the fail-closed default");
        assert_eq!(read_deployments(&docs.join("deployments.json")).unwrap().len(), 3);
    }

    /// A contract the database does not hold fails, rather than passing for want of evidence.
    #[tokio::test]
    async fn a_contract_with_no_verdict_fails_the_check() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let deployments = write(
            "unknown.json",
            r#"["0x00000000000000000000000000000000000000a1"]"#,
        );
        let policy = write("policy.toml", "[policy]\nallow_unknown = true\n");
        let code = check(&deployments, &policy, Source::Db(&store), true)
            .await
            .unwrap();
        assert_eq!(code, ExitCode::from(1));
        let _ = std::fs::remove_file(deployments);
        let _ = std::fs::remove_file(policy);
    }
}
