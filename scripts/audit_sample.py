#!/usr/bin/env python3
"""Draw a random sample of indexed proxies to check by hand, then grade Hermes against it.

Dev-only, stdlib-only, read-only. tests/verified.json holds rows chosen by shape, which catches
regressions and says nothing about how often the index is right. This measures that: a simple
random sample of covered proxies, each read by hand, compared with what Hermes published.

    python3 scripts/audit_sample.py draw 60 --seed 20261002
    python3 scripts/audit_sample.py grade docs/audit/2026-10-02

`draw` writes two files. `worksheet.json` lists the addresses with empty answers, to be filled
from `scripts/raw_reads.py` and the explorers. `hermes.json` holds what Hermes said; do not open
it until the worksheet is full. An answer written after seeing Hermes's is a copy of it, and a
copy agrees. Leave `"checked": false` on a row that could not be established by hand: it is
reported as unchecked, never counted as agreement.

The rate is within the sample of proxies Hermes indexes, which is itself a sample of the chain.
"""

import json
import math
import pathlib
import random
import sys
import urllib.request
from datetime import datetime, timezone

LIVE = "https://hermes-production-29bf.up.railway.app"
AUDITS = pathlib.Path(__file__).resolve().parent.parent / "docs" / "audit"
FIELDS = ["kind", "terminal_authority", "terminal_chain", "authority_kind", "compromise_depth", "timelock_seconds"]


def proxies(base):
    out, offset = [], 0
    while True:
        req = urllib.request.Request(
            f"{base}/proxies?limit=200&offset={offset}", headers={"user-agent": "hermes-audit"}
        )
        with urllib.request.urlopen(req, timeout=30) as r:
            page = json.load(r)
        out += page["proxies"]
        offset += page["count"]
        if page["count"] == 0 or offset >= page["total"]:
            return out


def draw(n, seed, base):
    population = sorted(proxies(base), key=lambda p: p["address"].lower())
    picked = random.Random(seed).sample(population, min(n, len(population)))
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    out = AUDITS / day
    out.mkdir(parents=True, exist_ok=True)
    meta = {"source": base, "seed": seed, "population": len(population), "drawn": len(picked)}
    sheet = [
        {"address": p["address"], "checked": False, "block": None, "how": "", **{f: None for f in FIELDS}}
        for p in picked
    ]
    (out / "worksheet.json").write_text(json.dumps({"meta": meta, "rows": sheet}, indent=1) + "\n")
    (out / "hermes.json").write_text(json.dumps({"meta": meta, "rows": picked}, indent=1) + "\n")
    print(f"drew {len(picked)} of {len(population)} covered proxies (seed {seed}) into {out}")


def upper_bound(errors, n):
    """Wilson 95% upper bound on the error rate."""
    if n == 0:
        return None
    z, p = 1.96, errors / n
    centre = p + z * z / (2 * n)
    spread = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return (centre + spread) / (1 + z * z / n)


def same(field, hand, hermes):
    if field == "terminal_authority" and hand and hermes:
        return hand.lower() == hermes.lower()
    return hand == hermes


def grade(folder):
    folder = pathlib.Path(folder)
    sheet = json.loads((folder / "worksheet.json").read_text())["rows"]
    hermes = {p["address"].lower(): p for p in json.loads((folder / "hermes.json").read_text())["rows"]}
    checked = [r for r in sheet if r["checked"]]
    wrong = []
    for row in checked:
        said = hermes[row["address"].lower()]
        diffs = [f for f in FIELDS if not same(f, row[f], said.get(f))]
        if diffs:
            wrong.append((row["address"], diffs))
    for address, diffs in wrong:
        print(f"DIFFERS {address}: {', '.join(diffs)}")
    n = len(checked)
    print(f"\n{len(sheet)} drawn, {n} checked by hand, {len(sheet) - n} unchecked")
    if n:
        bound = upper_bound(len(wrong), n)
        print(f"{len(wrong)} of {n} differ; error rate at most {bound:.1%} (95%, Wilson), within the index sample")


args = sys.argv[1:]
if args[:1] == ["draw"] and len(args) >= 2:
    seed = int(args[args.index("--seed") + 1]) if "--seed" in args else int(datetime.now(timezone.utc).timestamp())
    draw(int(args[1]), seed, LIVE)
elif args[:1] == ["grade"] and len(args) == 2:
    grade(args[1])
else:
    sys.exit(__doc__)
