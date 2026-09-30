#!/usr/bin/env python3
"""Print one row of docs/progress.md from a running Hermes instance.

Dev-only, stdlib-only, read-only: it asks the public JSON API what it currently serves, so
the numbers in the progress log are ones a stranger could reproduce with curl.

    python3 scripts/metrics.py                       # the live deployment
    python3 scripts/metrics.py http://localhost:8080 # a local `hermes serve`
"""

import json
import sys
import urllib.request
from datetime import datetime, timezone

BASE = (sys.argv[1] if len(sys.argv) > 1 else "https://hermes-production-29bf.up.railway.app").rstrip("/")


def get(path):
    req = urllib.request.Request(BASE + path, headers={"user-agent": "hermes-metrics"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


cov = get("/coverage")
auth = get("/authorities")["authorities"]

single_key = [a for a in auth if a.get("compromise_depth") == 1]
unknown_depth = [a for a in auth if a.get("compromise_depth") is None]
top = auth[0] if auth else None
last = cov.get("last_scan")
when = datetime.fromtimestamp(last, timezone.utc).strftime("%Y-%m-%d %H:%MZ") if last else "never"

print(f"source: {BASE}   last scan: {when}")
print()
print("| scanned | covered | resolved | roots | single-key roots (proxies) | depth unknown | largest root |")
print("|---|---|---|---|---|---|---|")
largest = (
    f"{top['kind']} on {top.get('chain') or 'base'}, {top['proxy_count']} proxies, "
    f"{top['compromise_depth'] if top['compromise_depth'] is not None else 'unknown'} keys"
    if top
    else "none"
)
print(
    f"| {cov['total_scanned']} | {cov['covered_proxies']} | {cov['resolved_proxies']} "
    f"| {cov['distinct_authorities']} "
    f"| {len(single_key)} ({sum(a['proxy_count'] for a in single_key)}) "
    f"| {len(unknown_depth)} | {largest} |"
)
for key in ("unresolved_by_reason", "depth_unknown_by_reason", "resolved_by_path"):
    if key in cov:
        print(f"\n{key}: {cov[key]}")

kinds = {}
for a in auth:
    kinds[a.get("kind")] = kinds.get(a.get("kind"), 0) + 1
print(f"\nroots by kind: {sorted(kinds.items(), key=lambda kv: -kv[1])}")

# Instances deployed before the kept graph have no /v1; everything above still reproduces.
try:
    changes = get("/v1/changes?since=24h&limit=1000")
    print(f"\nchain changes in the last 24h: {changes['count']}")
    print(f"sighted by discovery: {cov.get('proxies_sighted')} proxies in {cov.get('families_sighted')} families")
except Exception as e:  # noqa: BLE001 - reporting, not handling
    print(f"\n/v1 not served here ({e.__class__.__name__}); no change counts")
