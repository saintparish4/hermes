#!/usr/bin/env python3
"""Read every Safe an index roots at, and the Safes among their owners, for modules and guards.

Dev-only, stdlib-only, read-only, and independent of Hermes: plain JSON-RPC at each chain's
finalized block, paced at about a call a second. It reads what a Safe's threshold does not say:
its enabled modules (`getModulesPaginated` from the sentinel), its guard slot
(`keccak256("guard_manager.guard.address")`), its module-guard slot (Safe 1.5) and its singleton
(slot 0). It concludes nothing about keys.

    python3 -I scripts/safe_setups.py hermes.db docs/audit/2026-10-17/safe-setups.json

Resumable: each Safe is appended to `<out>l` (JSON lines) as it is read, and a re-run skips
those. This is how model 4 was sized before it was built (docs/progress.md, 2026-10-17).
"""

import json, sqlite3, sys, time, urllib.request

RPC = {"base": "https://mainnet.base.org", "ethereum": "https://rpc.mevblocker.io"}
SENTINEL = "0x" + "0" * 39 + "1"
GUARD_SLOT = "0x4a204f620c8c5ccdca3fd54d003badd85ba500436a431f0cbda4f558c93c34c8"
MODULE_GUARD_SLOT = "0xb104e0b93118902c651344349b610029d694cfdec91c589c91ebafbcd0289947"
SEL = {
    "getOwners": "0xa0e67e2b",
    "getThreshold": "0xe75235b8",
    "getModulesPaginated": "0xcc2f8452",
    "VERSION": "0xffa1ad74",
}
last = {}


def rpc(chain, method, params):
    for attempt in range(6):
        wait = 1.1 - (time.time() - last.get(chain, 0))
        if wait > 0:
            time.sleep(wait)
        last[chain] = time.time()
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = urllib.request.Request(RPC[chain], body, {"content-type": "application/json", "user-agent": "hermes-measure"})
        try:
            out = json.load(urllib.request.urlopen(req, timeout=30))
        except Exception as e:
            time.sleep(2 ** (attempt + 1))
            continue
        if "error" in out:
            msg = str(out["error"]).lower()
            if "revert" in msg or "execution" in msg:
                return None
            time.sleep(2 ** (attempt + 1))
            continue
        return out["result"]
    raise RuntimeError(f"undetermined {chain} {method} {params}")


def words(hexdata):
    h = hexdata[2:]
    return [h[i:i + 64] for i in range(0, len(h), 64)]


def addr(word):
    return "0x" + word[24:]


def call(chain, block, to, data):
    return rpc(chain, "eth_call", [{"to": to, "data": data}, block])


def address_array(data):
    w = words(data)
    off = int(w[0], 16) // 32
    n = int(w[off], 16)
    return [addr(x) for x in w[off + 1: off + 1 + n]]


def safe_facts(chain, block, a):
    owners = call(chain, block, a, SEL["getOwners"])
    thr = call(chain, block, a, SEL["getThreshold"])
    if not owners or owners == "0x" or not thr or thr == "0x":
        return None
    page = SEL["getModulesPaginated"] + SENTINEL[2:].rjust(64, "0") + hex(50)[2:].rjust(64, "0")
    mods = call(chain, block, a, page)
    modules = None if mods in (None, "0x") else address_array(mods)
    guard = rpc(chain, "eth_getStorageAt", [a, GUARD_SLOT, block])
    mguard = rpc(chain, "eth_getStorageAt", [a, MODULE_GUARD_SLOT, block])
    singleton = rpc(chain, "eth_getStorageAt", [a, "0x0", block])
    version = call(chain, block, a, SEL["VERSION"])
    return {
        "owners": address_array(owners),
        "threshold": int(thr, 16),
        "modules": modules,
        "guard": None if int(guard, 16) == 0 else addr(guard[2:]),
        "module_guard": None if int(mguard, 16) == 0 else addr(mguard[2:]),
        "singleton": addr(singleton[2:]),
        "version": version,
    }


def main(db, out_path):
    c = sqlite3.connect(db)
    roots = c.execute(
        "select terminal_authority, terminal_chain, count(*) from proxy "
        "where authority_kind='safe' group by 1, 2"
    ).fetchall()
    blocks = {ch: rpc(ch, "eth_getBlockByNumber", ["finalized", False])["number"] for ch in RPC}
    seen, results = set(), []
    done = {}
    try:
        for line in open(out_path + "l"):
            r = json.loads(line)
            done[(r["address"].lower(), r["chain"])] = r
    except FileNotFoundError:
        pass
    sink = open(out_path + "l", "a")
    queue = [(r[0], r[1] or "base", r[2], None) for r in roots]
    while queue:
        a, chain, proxies, parent = queue.pop(0)
        if (a.lower(), chain) in seen:
            continue
        seen.add((a.lower(), chain))
        if (a.lower(), chain) in done:
            results.append(done[(a.lower(), chain)])
            for o in done[(a.lower(), chain)].get("contract_owners", []):
                queue.append((o, chain, proxies, a))
            continue
        facts = safe_facts(chain, blocks[chain], a)
        if facts is None:
            continue
        row = {"address": a, "chain": chain, "proxies": proxies, "parent": parent, "block": blocks[chain], **facts}
        print(a, chain, facts["threshold"], len(facts["owners"]), facts["modules"], facts["guard"], flush=True)
        row["contract_owners"] = []
        for o in facts["owners"]:
            code = rpc(chain, "eth_getCode", [o, blocks[chain]])
            if code not in ("0x", None) and not code.startswith("0xef0100"):
                row["contract_owners"].append(o)
                queue.append((o, chain, proxies, a))
        results.append(row)
        sink.write(json.dumps(row) + "\n")
        sink.flush()
    json.dump({"blocks": blocks, "safes": results}, open(out_path, "w"), indent=1)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
