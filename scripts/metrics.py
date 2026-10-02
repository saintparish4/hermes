#!/usr/bin/env python3
"""Print one row of docs/progress.md from a running Hermes instance.

Dev-only, stdlib-only, read-only: it asks the public JSON API what it currently serves, so
the numbers in the progress log are ones a stranger could reproduce with curl.

The answers are saved under docs/snapshots/ before anything is counted, and the row is computed
from that file. A live instance moves on with the next scan; without the file, a row in the log
could not be recomputed or compared with the one after it.

    python3 scripts/metrics.py                         # the live deployment
    python3 scripts/metrics.py http://localhost:8080   # a local `hermes serve`
    python3 scripts/metrics.py --from docs/snapshots/2026-10-02-live.json
"""

import json
import pathlib
import sys
import urllib.request
from datetime import datetime, timezone

LIVE = "https://hermes-production-29bf.up.railway.app"
SNAPSHOTS = pathlib.Path(__file__).resolve().parent.parent / "docs" / "snapshots"


def get(base, path):
    req = urllib.request.Request(base + path, headers={"user-agent": "hermes-metrics"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def fetch(base):
    snap = {
        "source": base,
        "fetched_at": int(datetime.now(timezone.utc).timestamp()),
        "coverage": get(base, "/coverage"),
        "authorities": get(base, "/authorities")["authorities"],
    }
    # Instances deployed before the kept graph have no /v1; everything else still reproduces.
    try:
        snap["changes_24h"] = get(base, "/v1/changes?since=24h&limit=1000")["count"]
    except Exception as e:  # noqa: BLE001 - reporting, not handling
        snap["changes_24h"] = None
        snap["changes_error"] = e.__class__.__name__
    return snap


def save(snap):
    day = datetime.fromtimestamp(snap["fetched_at"], timezone.utc).strftime("%Y-%m-%d")
    where = "live" if snap["source"] == LIVE else "local"
    SNAPSHOTS.mkdir(parents=True, exist_ok=True)
    path = SNAPSHOTS / f"{day}-{where}.json"
    path.write_text(json.dumps(snap, indent=1, sort_keys=True) + "\n")
    return path


args = sys.argv[1:]
if args[:1] == ["--from"]:
    path = pathlib.Path(args[1])
else:
    path = save(fetch((args[0] if args else LIVE).rstrip("/")))
snap = json.loads(path.read_text())
BASE, cov, auth = snap["source"], snap["coverage"], snap["authorities"]

single_key = [a for a in auth if a.get("compromise_depth") == 1]
unknown_depth = [a for a in auth if a.get("compromise_depth") is None]
top = auth[0] if auth else None
last = cov.get("last_scan")
when = datetime.fromtimestamp(last, timezone.utc).strftime("%Y-%m-%d %H:%MZ") if last else "never"

print(f"source: {BASE}   last scan: {when}   snapshot: {path}")
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

# A resolver that guessed would raise "resolved" too. The split by confidence is what shows
# whether new answers are ones Hermes is sure of.
by_confidence = {}
for a in auth:
    roots, proxies = by_confidence.get(a.get("confidence"), (0, 0))
    by_confidence[a.get("confidence")] = (roots + 1, proxies + a["proxy_count"])
print(f"\nroots (proxies) by confidence: {sorted(by_confidence.items(), key=lambda kv: str(kv[0]))}")

if snap.get("changes_24h") is None:
    print(f"\n/v1 not served here ({snap.get('changes_error')}); no change counts")
else:
    print(f"\nchain changes in the last 24h: {snap['changes_24h']}")
    print(f"sighted by discovery: {cov.get('proxies_sighted')} proxies in {cov.get('families_sighted')} families")
