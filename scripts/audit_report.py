#!/usr/bin/env python3
"""Turn an audit folder's JSON into the report a person reads, and compute nothing else.

Dev-only, stdlib-only, read-only. Every number in the report comes from the files: the oracle's
verdicts (`hermes oracle`), the backtest's judgements (`hermes backtest`), and the Safe setups
read without Hermes (`scripts/safe_setups.py`). Written by hand, a report drifts from its data;
generated, it can be checked by running this again.

    python3 -I scripts/audit_report.py docs/audit/2026-10-17 > docs/audit/2026-10-17/report.md

Rates carry 95% Wilson intervals. "Undetermined" and "ambiguous" are reported as counts and never
folded into a rate: a trial that could not run is not a pass.
"""

import json
import math
import pathlib
import sys
from collections import Counter, defaultdict

Z = 1.959963984540054


def wilson(k, n):
    if n == 0:
        return None
    p = k / n
    centre = (p + Z * Z / (2 * n)) / (1 + Z * Z / n)
    half = Z * math.sqrt(p * (1 - p) / n + Z * Z / (4 * n * n)) / (1 + Z * Z / n)
    return p, max(0.0, centre - half), min(1.0, centre + half)


def rate(k, n):
    w = wilson(k, n)
    if w is None:
        return "no decided trial"
    p, lo, hi = w
    return f"{k}/{n} = {p:.0%} [{lo:.0%}, {hi:.0%}]"


def short(a):
    return f"{a[:6]}…{a[-4:]}"


def safe_setups(path):
    d = json.loads(path.read_text())
    safes = d["safes"]
    roots = [s for s in safes if s["parent"] is None]
    with_door = [s for s in safes if s["modules"] or s["guard"]]
    print("## Safe modules and guards (read without Hermes)\n")
    print(f"Blocks: Base {int(d['blocks']['base'], 16):,}, Ethereum {int(d['blocks']['ethereum'], 16):,}. "
          f"{len(safes)} Safes: {len(roots)} roots of the index and {len(safes) - len(roots)} Safes among their owners.\n")
    print(f"- Enabled modules: **{sum(1 for s in safes if s['modules'])}** Safes; guards: **{sum(1 for s in safes if s['guard'])}**; "
          f"module guards: {sum(1 for s in safes if s.get('module_guard'))}.")
    print(f"- Roots with a module or guard: {sum(1 for s in roots if s['modules'] or s['guard'])} of {len(roots)}.\n")
    if with_door:
        print("| Safe | Chain | Where | Threshold | Modules |")
        print("|---|---|---|---|---|")
        for s in with_door:
            where = "root" if s["parent"] is None else f"owner of {short(s['parent'])}"
            mods = ", ".join(short(m) for m in s["modules"])
            print(f"| `{short(s['address'])}` | {s['chain']} | {where} | {s['threshold']}-of-{len(s['owners'])} | {mods} |")
        print()


def oracle(path):
    d = json.loads(path.read_text())
    frame = d["frame"]
    by = defaultdict(Counter)
    undetermined = Counter()
    contradicted = []
    for stratum, v in d["verdicts"]:
        by[stratum][v["outcome"]] += 1
        if v["outcome"] == "undetermined":
            undetermined[v["undriveable"]] += 1
        if v["outcome"] == "contradicted":
            contradicted.append((stratum, v))
    print("## The EVM oracle (`hermes oracle`)\n")
    print(f"Model {d['model_version']}, seed {d['seed']}, up to {d['per_stratum']} rows per stratum, each forked at its own block. "
          "Confirmed means the named keys upgraded the proxy, no set one key short did, and a stranger could not. "
          "Restricted means the contract refused the planted implementation and re-installing the one in place "
          "went the same way: the keys are the gate, and the contract limits what they can install.\n")
    print("| Stratum (path / root kind) | Rows with a count | Drawn | Confirmed | Restricted | Contradicted | Undetermined | Gate confirmed among decided |")
    print("|---|---|---|---|---|---|---|---|")
    weighted, total, all_decided = 0.0, 0, True
    for stratum, n in sorted(frame.items()):
        c = by[stratum]
        r, x, u = c["restricted"], c["contradicted"], c["undetermined"]
        k = c["confirmed"] + r
        print(f"| {stratum} | {n} | {k + x + u} | {c['confirmed']} | {r} | {x} | {u} | {rate(k, k + x)} |")
        total += n
        if k + x:
            weighted += n * k / (k + x)
        else:
            all_decided = False
    print()
    if all_decided and total:
        print(f"Frame-weighted share of decided counts whose gate the fork confirmed: **{weighted / total:.1%}**.\n")
    else:
        print("No frame-wide rate: at least one stratum had no decided trial, and its rate would be a guess.\n")
    if undetermined:
        print("Why trials could not run: " + ", ".join(f"`{r}` {n}" for r, n in undetermined.most_common()) + ".\n")
    if contradicted:
        print("Contradicted rows (each needs reading by hand before it is called a finding):\n")
        for stratum, v in contradicted:
            what = [w for w, ok in (("sufficient", v["sufficient"]), ("minimal", v["minimal"]), ("stranger refused", v["stranger_refused"])) if ok is False]
            print(f"- `{v['proxy']}` ({stratum}, {v['keys_required']} keys): failed {', '.join(what)}")
        print()


def label(j):
    if j == "unresolved":
        return "unresolved", ""
    (kind, why), = j.items()
    return kind, why


def backtest(path):
    d = json.loads(path.read_text())
    rows = d["judged"]
    print("## The backtest (`hermes backtest`)\n")
    print(f"Model {d['model_version']}, seed {d['seed']}. {d['candidates']} upgrade sightings of indexed proxies and their beacons; "
          f"{d['deployments_skipped']} were deployments and set aside; {d['unreadable']} could not be read; {len(rows)} upgrades judged "
          "against what Hermes reads the block before each.\n")
    matrix = defaultdict(Counter)
    whys = Counter()
    for r in rows:
        if r["root"] is None:
            hermes = "no root"
        elif r["keys_required"] is not None:
            hermes = "root with a count"
        else:
            hermes = f"root, count unknown ({r['depth_unknown_reason']})"
        kind, why = label(r["judgement"])
        matrix[hermes][kind] += 1
        if why:
            whys[f"{kind}: {why}"] += 1
    cols = ["consistent", "contradicted", "ambiguous", "unresolved"]
    print("| Hermes the block before | " + " | ".join(cols) + " |")
    print("|---|" + "---|" * len(cols))
    for hermes in sorted(matrix):
        print(f"| {hermes} | " + " | ".join(str(matrix[hermes][c]) for c in cols) + " |")
    print()
    k = sum(m["consistent"] for m in matrix.values())
    x = sum(m["contradicted"] for m in matrix.values())
    print(f"Consistent among decided: {rate(k, k + x)}.\n")
    print("By reason: " + ", ".join(f"{w} {n}" for w, n in sorted(whys.items())) + ".\n")
    bad = [r for r in rows if label(r["judgement"])[0] == "contradicted"]
    if bad:
        print("Contradicted upgrades:\n")
        for r in bad:
            print(f"- `{r['address']}` at block {r['block']:,} (`{r['transaction']}`): {label(r['judgement'])[1]}")
        print()


def main(folder):
    folder = pathlib.Path(folder)
    print(f"# Audit, {folder.name}\n")
    print("Generated by `scripts/audit_report.py` from the JSON beside it. Within the index, which is a sample of Base.\n")
    for name, section in (("safe-setups.json", safe_setups), ("oracle.json", oracle), ("backtest.json", backtest)):
        if (folder / name).exists():
            section(folder / name)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
