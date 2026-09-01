#!/usr/bin/env python3
"""Regenerate seed candidates from live Base traffic.

This is a one-off developer tool, not part of the product. It samples recent blocks, keeps
the addresses that actually have code, probes their ERC-1967 / EIP-1822 slots, and prints
ready-to-paste `SeedEntry` lines. Nothing it emits is guessed: every address printed was
answered for by the node.

Curated seed sources replace this once discovery is worth automating.

    python3 scripts/discover_seed.py --blocks 150 --limit 60
"""
import argparse, collections, json, time, urllib.request

RPC_DEFAULT = "https://mainnet.base.org"
SLOTS = [
    ("impl",   "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"),
    ("admin",  "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"),
    ("beacon", "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"),
    ("p1822",  "0xc5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7"),
    ("zos",    "0x7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3"),
]
# The public endpoint rejects batches larger than this with -32014.
MAX_BATCH = 10


def post(rpc, batch):
    req = urllib.request.Request(
        rpc, data=json.dumps(batch).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "hermes-seed/0.1"})
    with urllib.request.urlopen(req, timeout=90) as r:
        return json.load(r)


def call(rpc, batch, tries=8):
    """Retry until every sub-call in the batch has a concrete result.

    Partial batch responses are common on the public endpoint (~38% of reads during the run
    that produced the checked-in seed), so a missing element is retried rather than believed.
    """
    assert len(batch) <= MAX_BATCH
    for attempt in range(tries):
        try:
            d = post(rpc, batch)
            if isinstance(d, dict):
                raise RuntimeError(d.get("error"))
            res = {r["id"]: r.get("result") for r in d}
            if len(res) == len(batch) and all(v is not None for v in res.values()):
                return res
        except Exception:
            pass
        time.sleep(min(0.7 * (2 ** attempt), 15))
    return None


def sample_blocks(rpc, n):
    head = int(call(rpc, [{"jsonrpc": "2.0", "id": 0, "method": "eth_blockNumber",
                           "params": []}])[0], 16)
    counts = collections.Counter()
    for i in range(0, n, MAX_BATCH):
        batch = [{"jsonrpc": "2.0", "id": j, "method": "eth_getBlockByNumber",
                  "params": [hex(head - i - j), True]}
                 for j in range(min(MAX_BATCH, n - i))]
        res = call(rpc, batch)
        if not res:
            continue
        for blk in res.values():
            for tx in blk.get("transactions", []):
                if tx.get("to"):
                    counts[tx["to"].lower()] += 1
    return counts


def word_to_addr(w):
    return None if (not w or int(w, 16) == 0) else "0x" + w[-40:]


def probe(rpc, addr):
    batch = [{"jsonrpc": "2.0", "id": i, "method": "eth_getStorageAt",
              "params": [addr, slot, "latest"]} for i, (_, slot) in enumerate(SLOTS)]
    batch.append({"jsonrpc": "2.0", "id": 5, "method": "eth_getCode",
                  "params": [addr, "latest"]})
    res = call(rpc, batch)
    if not res or len(res[5]) <= 2:
        return None
    impl, admin = word_to_addr(res[0]), word_to_addr(res[1])
    beacon, p1822, zos = word_to_addr(res[2]), word_to_addr(res[3]), word_to_addr(res[4])
    if impl and admin:  kind = "transparent"
    elif impl:          kind = "uups"
    elif beacon:        kind = "beacon"
    elif p1822:         kind = "eip1822"
    elif admin:         kind = "admin_only"
    elif zos:           kind = "zeppelin_os"
    else:               kind = "not_upgradeable"
    return {"address": addr, "kind": kind, "impl": impl, "admin": admin}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", default=RPC_DEFAULT)
    ap.add_argument("--blocks", type=int, default=150)
    ap.add_argument("--limit", type=int, default=60, help="seed entries to print")
    ap.add_argument("--candidates", type=int, default=800)
    args = ap.parse_args()

    print(f"# sampling {args.blocks} blocks…", flush=True)
    counts = sample_blocks(args.rpc, args.blocks)
    cands = [a for a, _ in counts.most_common(args.candidates)]
    print(f"# {len(cands)} candidate addresses", flush=True)

    found = []
    for i, a in enumerate(cands):
        r = probe(args.rpc, a)
        if r and r["kind"] in ("transparent", "uups", "beacon", "eip1822", "admin_only"):
            found.append(r)
            if len(found) >= args.limit:
                break
        if i % 25 == 0:
            print(f"#   {i}/{len(cands)} probed, {len(found)} proxies", flush=True)

    print(f"\n# {len(found)} proxies. Paste into crates/hermes-scan/src/seed.rs:\n")
    for r in found:
        print(f'    SeedEntry {{ address: "{r["address"]}", label: None }}, // {r["kind"]}')
    print("\n# NOTE: addresses print lowercase. Checksum them (or leave them lowercase —")
    print("#       alloy parses both, and the store checksums on write).")


if __name__ == "__main__":
    main()