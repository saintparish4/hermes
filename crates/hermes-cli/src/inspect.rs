//! Offline questions about the kept graph: how a proxy reaches its root, what an address
//! reaches, who sits in a Safe, which Safes share signers.
//!
//! Everything here reads the database and nothing else, so it answers the same whether or not
//! a node is reachable, and it answers as of the last scan, which it says.

use hermes_core::blast::BlastRadius;
use hermes_core::graph_store::Membership;
use hermes_core::time::{duration, short};
use hermes_core::view::{
    NodeView, ProxyView, Scope, TreeNode, key_view, node_view, pick_node, proxy_view,
};
use hermes_core::{Chain, Node, Store};
use std::fmt::Write;

fn show(n: &Node) -> String {
    match n.chain {
        Chain::Base => n.address.to_checksum(None),
        Chain::Ethereum => format!("{} (ethereum)", n.address.to_checksum(None)),
    }
}

/// An authority kind as the rest of the output words it.
fn kind_name(kind: &str) -> &str {
    match kind {
        "safe" => "Safe",
        "eoa" => "key",
        "ownable" => "owned contract",
        "l1_alias" => "L2 alias",
        "unknown" => "unrecognized contract",
        "smart_account" => "smart account",
        "role_gated" => "AccessControl contract",
        "sentinel" => "sentinel address",
        other => other,
    }
}

fn describe(t: &TreeNode) -> String {
    let Some(kind) = t.kind.as_deref() else {
        return "not read".into();
    };
    match kind {
        "safe" => match (t.threshold, t.owner_count) {
            (Some(m), Some(n)) => format!("Safe {m}-of-{n}"),
            _ => "Safe".into(),
        },
        "eoa" if t.code.as_deref() == Some("delegated") => "key (EIP-7702 delegated)".into(),
        "eoa" => "key".into(),
        "smart_account" => match t.passkeys {
            Some(p) => {
                let n = t.children.len() + p as usize;
                let keys = if p > 0 {
                    format!(", {p} of them passkeys")
                } else {
                    String::new()
                };
                format!("smart account: any one of {n} signers{keys}")
            }
            None => "smart account: signers not read".into(),
        },
        "role_gated" => match t.members_listed {
            Some(true) if t.children.is_empty() => {
                "AccessControl: no one holds the upgrade role or its admin role".into()
            }
            Some(true) => format!(
                "AccessControl: any one of {} holders of the upgrade role or its admin",
                t.children.len()
            ),
            _ => "AccessControl: role holders not listed on chain".into(),
        },
        "sentinel" => "sentinel: no one holds a key for this address".into(),
        "ownable" => "owned contract".into(),
        "timelock" => format!("timelock, delay {}", duration(t.min_delay.unwrap_or(0))),
        "l1_alias" => "L2 alias of an Ethereum contract".into(),
        "unknown" => "unrecognized: answers no interface Hermes reads".into(),
        other => other.to_string(),
    }
}

fn relation(r: &str) -> &str {
    match r {
        "uups_self" => "the proxy itself (UUPS)",
        "safe_owner" | "account_owner" => "signer",
        "role_member" => "role holder",
        "l1_alias" => "acts for",
        other => other,
    }
}

fn render(t: &TreeNode, prefix: &str, branch: &str, out: &mut String) {
    let stopped = match t.stopped {
        Some("cycle") => "  [cycle: already on this path]",
        Some("depth") => "  [depth cap: not followed further]",
        _ => "",
    };
    let _ = writeln!(
        out,
        "{prefix}{branch}{}  {}  {}{stopped}",
        relation(&t.relation),
        show(&t.node),
        describe(t)
    );
    let child_prefix = match branch {
        "├─ " => format!("{prefix}│  "),
        "└─ " => format!("{prefix}   "),
        _ => prefix.to_string(),
    };
    for (i, c) in t.children.iter().enumerate() {
        let branch = if i + 1 == t.children.len() {
            "└─ "
        } else {
            "├─ "
        };
        render(c, &child_prefix, branch, out);
    }
}

fn scope_line(s: &Scope) -> String {
    let as_of = match &s.as_of {
        Some(o) => format!(
            "graph as of Base block {} / Ethereum block {} ({})",
            o.base_block.map_or("?".into(), |b| b.to_string()),
            o.ethereum_block.map_or("?".into(), |b| b.to_string()),
            short(o.observed_at)
        ),
        None => "no scan recorded yet".into(),
    };
    let cap = s
        .per_family
        .map_or(String::new(), |c| format!(", at most {c} per family"));
    format!(
        "scope: a sample of Base{cap}; {as_of}; model {}; first seen means first seen by Hermes",
        s.model_version
    )
}

fn print_proxy(v: &ProxyView) {
    let p = &v.proxy;
    let label = p
        .label
        .as_deref()
        .map_or(String::new(), |l| format!(" ({l})"));
    println!("{}{label} · {} proxy on base", p.address, p.kind);
    let block = p
        .scanned_block
        .map_or("an unpinned block".into(), |b| format!("Base block {b}"));
    println!("scanned at {block} · {}\n", short(p.scanned_at));
    match &v.tree {
        Some(t) => {
            let mut out = String::new();
            render(t, "", "", &mut out);
            print!("{out}");
        }
        None => println!("no upgrade path Hermes models"),
    }
    println!();
    match (&p.terminal_authority, &p.unresolved_reason) {
        (Some(root), _) => {
            let chain = p.terminal_chain.as_deref().unwrap_or("base");
            let kind = kind_name(p.authority_kind.as_deref().unwrap_or("?"));
            println!("root        {kind} {root} on {chain}");
            let keys = p.compromise_depth.map_or_else(
                || {
                    format!(
                        "unknown ({})",
                        p.depth_unknown_reason.as_deref().unwrap_or("unrecorded")
                    )
                },
                |k| k.to_string(),
            );
            println!("keys        {keys}");
            let delay = p.timelock_seconds.map_or("unknown".into(), |t| {
                if t == 0 {
                    "none found".into()
                } else {
                    duration(t as u64)
                }
            });
            println!("timelock    {delay}");
            println!(
                "confidence  {} (walked from the {})",
                p.resolution_confidence.as_deref().unwrap_or("?"),
                v.upgrade_entry.unwrap_or("?").replace('_', " ")
            );
        }
        (None, reason) => println!(
            "root        none published: {}",
            reason.as_deref().unwrap_or("resolution not attempted")
        ),
    }
    if let Some(r) = &v.replayed
        && !r.matches_row
    {
        println!(
            "note        the stored graph now resolves to {} with {} keys; the row above is \
             what the scan published",
            show(&r.root),
            r.keys_required.map_or("unknown".into(), |k| k.to_string())
        );
    }
    println!("\n{}", scope_line(&v.scope));
}

fn print_radius_summary(b: &BlastRadius) {
    println!(
        "blast radius  controls {} indexed proxies alone, takes part in {} more, {} undetermined",
        b.controls.len(),
        b.participates.len(),
        b.undetermined.len()
    );
}

fn print_memberships(ms: &[Membership]) {
    for m in ms {
        let size = match (m.threshold, m.owner_count) {
            (Some(t), Some(n)) => format!("{t}-of-{n}"),
            _ => "?".into(),
        };
        println!(
            "  signer of Safe {} ({size}) as {}",
            show(&m.safe),
            m.member_kind.as_deref().unwrap_or("unread")
        );
    }
}

fn print_node(v: &NodeView) {
    let n = &v.node;
    println!(
        "{} · {} on {}",
        n.node.address.to_checksum(None),
        describe(&v.tree),
        n.node.chain.as_str()
    );
    println!(
        "first seen by Hermes at block {} ({}), last seen at block {} ({})\n",
        n.first_seen_block.map_or("?".into(), |b| b.to_string()),
        short(n.first_seen_at),
        n.last_seen_block.map_or("?".into(), |b| b.to_string()),
        short(n.last_seen_at)
    );
    let mut out = String::new();
    render(&v.tree, "", "", &mut out);
    print!("{out}");
    if !v.controls_directly.is_empty() {
        println!("\ncontrols directly ({}):", v.controls_directly.len());
        for e in &v.controls_directly {
            match e.relation {
                hermes_core::Relation::L1Alias => {
                    println!("  acts on Base as {}", show(&e.from));
                }
                r => println!("  {} of {}", r.as_str(), show(&e.from)),
            }
        }
    }
    if !v.owner_of.is_empty() {
        println!("\nowner of:");
        print_memberships(&v.owner_of);
    }
    if !v.history.is_empty() {
        println!("\nclosed edges:");
        for e in &v.history {
            println!(
                "  {} {} {} · seen {} to {}, gone by block {}",
                show(&e.from),
                e.relation.as_str(),
                show(&e.to),
                e.first_seen_block.map_or("?".into(), |b| b.to_string()),
                e.last_seen_block.map_or("?".into(), |b| b.to_string()),
                e.closed_block.map_or("?".into(), |b| b.to_string())
            );
        }
    }
    println!();
    print_radius_summary(&v.blast_radius);
    println!("\n{}", scope_line(&v.scope));
}

fn not_found(address: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{address} is not in the index: neither a scanned proxy nor an address a resolution walked"
    )
}

pub async fn authority(
    store: &Store,
    address: &str,
    chain: Option<Chain>,
    json: bool,
) -> anyhow::Result<()> {
    if chain != Some(Chain::Ethereum)
        && let Some(v) = proxy_view(store, address).await?
    {
        return emit(json, &v, || print_proxy(&v));
    }
    let node = pick_node(store, address, chain)
        .await?
        .ok_or_else(|| not_found(address))?;
    let v = node_view(store, node)
        .await?
        .ok_or_else(|| not_found(address))?;
    emit(json, &v, || print_node(&v))
}

fn emit<T: serde::Serialize>(json: bool, v: &T, text: impl FnOnce()) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        text();
    }
    Ok(())
}

fn print_reached(title: &str, list: &[hermes_core::blast::Reached]) {
    println!("{title} ({}):", list.len());
    for r in list {
        let label = r
            .label
            .as_deref()
            .map_or(String::new(), |l| format!(" ({l})"));
        let root = r.root.as_ref().map_or("no published root".into(), |n| {
            format!(
                "root {} · {} keys",
                show(n),
                r.keys_required.map_or("unknown".into(), |k| k.to_string())
            )
        });
        println!(
            "  {}{label} · {} via {} · {root}",
            r.proxy,
            r.kind,
            show(&r.entry)
        );
    }
}

pub async fn blast_radius(
    store: &Store,
    address: &str,
    chain: Option<Chain>,
    json: bool,
) -> anyhow::Result<()> {
    let node = pick_node(store, address, chain)
        .await?
        .ok_or_else(|| not_found(address))?;
    let v = node_view(store, node)
        .await?
        .ok_or_else(|| not_found(address))?;
    let b = &v.blast_radius;
    emit(json, b, || {
        let taken: Vec<String> = b.compromised.iter().map(show).collect();
        println!(
            "compromising {} takes {}\n",
            show(&b.target),
            taken.join(" and ")
        );
        print_reached("controls alone", &b.controls);
        print_reached("takes part in, not enough alone", &b.participates);
        print_reached("reaches, cannot tell whether alone", &b.undetermined);
        println!(
            "\nOnly proxies in the index are counted. It is a sample of Base, so this is a floor."
        );
        println!("{}", scope_line(&v.scope));
    })
}

pub async fn owners(store: &Store, address: &str, chain: Option<Chain>) -> anyhow::Result<()> {
    let node = pick_node(store, address, chain)
        .await?
        .ok_or_else(|| not_found(address))?;
    let v = node_view(store, node)
        .await?
        .ok_or_else(|| not_found(address))?;
    if v.tree.kind.as_deref() != Some("safe") {
        anyhow::bail!("{} is not a Safe: it is {}", show(&node), describe(&v.tree));
    }
    println!("# {} · {}", show(&node), describe(&v.tree));
    for owner in &v.tree.children {
        println!(
            "{}\t{}",
            owner.node.address.to_checksum(None),
            describe(owner)
        );
    }
    Ok(())
}

pub async fn signers(store: &Store, min_shared: i64, json: bool) -> anyhow::Result<()> {
    let pairs = store.shared_signers(min_shared).await?;
    emit(json, &pairs, || {
        println!(
            "pairs of Safes sharing at least {min_shared} signers: {}",
            pairs.len()
        );
        for p in &pairs {
            println!("\n{} and {} share {}:", show(&p.a), show(&p.b), p.shared);
            for k in &p.keys {
                println!("  {k}");
            }
        }
    })
}

fn dot_id(n: &Node) -> String {
    format!("\"{}:{}\"", n.chain.as_str(), n.address.to_checksum(None))
}

fn dot_tree(t: &TreeNode, out: &mut String) {
    let a = t.node.address.to_checksum(None);
    let _ = writeln!(
        out,
        "  {} [label=\"{}…{}\\n{}\"];",
        dot_id(&t.node),
        &a[..6],
        &a[a.len() - 4..],
        describe(t).replace('"', "'")
    );
    for c in &t.children {
        let _ = writeln!(
            out,
            "  {} -> {} [label=\"{}\"];",
            dot_id(&t.node),
            dot_id(&c.node),
            relation(&c.relation)
        );
        dot_tree(c, out);
    }
}

pub async fn graph(
    store: &Store,
    address: &str,
    chain: Option<Chain>,
    format: &str,
) -> anyhow::Result<()> {
    let tree = if chain != Some(Chain::Ethereum)
        && let Some(v) = proxy_view(store, address).await?
    {
        v.tree
            .ok_or_else(|| anyhow::anyhow!("{address} has no upgrade path Hermes models"))?
    } else {
        let node = pick_node(store, address, chain)
            .await?
            .ok_or_else(|| not_found(address))?;
        node_view(store, node)
            .await?
            .ok_or_else(|| not_found(address))?
            .tree
    };
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&tree)?),
        "dot" => {
            let mut out = String::from("digraph hermes {\n  rankdir=TB;\n  node [shape=box];\n");
            dot_tree(&tree, &mut out);
            out.push_str("}\n");
            print!("{out}");
        }
        other => anyhow::bail!("unknown format {other}; use dot or json"),
    }
    Ok(())
}

pub async fn key(store: &Store, address: &str, json: bool) -> anyhow::Result<()> {
    let v = key_view(store, address)
        .await?
        .ok_or_else(|| not_found(address))?;
    emit(json, &v, || {
        println!("{}", v.address);
        for n in &v.nodes {
            println!("  on {}: {}", n.node.chain.as_str(), n.kind);
        }
        if !v.owner_of.is_empty() {
            println!("\nowner of:");
            print_memberships(&v.owner_of);
        }
        if let Some(b) = &v.blast_radius {
            println!();
            print_radius_summary(b);
        }
        println!("\n{}", scope_line(&v.scope));
    })
}
