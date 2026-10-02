#!/usr/bin/env python3
"""Print what a chain says about some addresses at one block, and conclude nothing.

Dev-only, stdlib-only, read-only. This is how the expectations in tests/verified.json are
established: by reading the chain without Hermes. It prints code, the proxy slots and the answer
to each probe selector for exactly the addresses it is given. It does not follow an owner,
classify a proxy, undo an alias or count a key: the person reading the output does that. A
reader that resolved anything would be a second resolver, sharing Hermes's blind spots, and a
table checked against it would have learned its answers from a program again.

    python3 scripts/raw_reads.py base 51526000 0xd9aA...b6CA 0xd94E...f94f
    python3 scripts/raw_reads.py ethereum 26013200 0x7bB4...595c > tests/evidence/proxy-admin.l1.txt

Slots and selectors are written out here rather than derived, so they do not come from Hermes's
constants. An unanswered read prints UNANSWERED, which is not a revert and not a zero.
"""

import json
import sys
import time
import urllib.request

URLS = {"base": "https://mainnet.base.org", "ethereum": "https://eth.drpc.org"}

SLOTS = {
    "eip1967.implementation": "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc",
    "eip1967.admin": "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103",
    "eip1967.beacon": "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50",
    "PROXIABLE": "0xc5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7",
    "org.zeppelinos.proxy.implementation": "0x7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3",
}

ZERO = "0" * 64
CALLS = {
    "owner()": "0x8da5cb5b",
    "getOwners()": "0xa0e67e2b",
    "getThreshold()": "0xe75235b8",
    "getMinDelay()": "0xf27a0c92",
    "proxiableUUID()": "0x52d1902d",
    "entryPoint()": "0xb0d691fe",
    "nextOwnerIndex()": "0xd948fd2e",
    "supportsInterface(0x7965db0b)": "0x01ffc9a7" + "7965db0b" + "0" * 56,
    "DEFAULT_ADMIN_ROLE()": "0xa217fddf",
    "UPGRADER_ROLE()": "0xf72c0d8b",
    "getRoleAdmin(0)": "0x248a9ca3" + ZERO,
    "getRoleMemberCount(0)": "0xca15c873" + ZERO,
    "getRoleMember(0,0)": "0x9010d07c" + ZERO + ZERO,
}


def rpc(url, method, params):
    message = "no attempt"
    for attempt in range(4):
        time.sleep(1 + 3 * attempt)
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = urllib.request.Request(
            url, body, {"content-type": "application/json", "user-agent": "hermes-raw-reads"}
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                answer = json.load(r)
        except Exception as e:  # noqa: BLE001 - an unanswered read is reported, not handled
            message = e.__class__.__name__
            continue
        if "result" in answer:
            return answer["result"]
        message = str(answer.get("error", {}).get("message", ""))
        if "revert" in message.lower():
            return "REVERT"
    return f"UNANSWERED ({message[:60]})"


def main():
    if len(sys.argv) < 4 or sys.argv[1] not in URLS:
        sys.exit(__doc__)
    chain, block, addresses = sys.argv[1], hex(int(sys.argv[2])), sys.argv[3:]
    url = URLS[chain]
    print(f"# {chain} at block {int(block, 16)} via {url}")
    for a in addresses:
        print(f"\n{a}")
        code = rpc(url, "eth_getCode", [a, block])
        size = (len(code) - 2) // 2 if code.startswith("0x") else code
        print(f"  code: {size} bytes  {code[:48]}")
        for name, slot in SLOTS.items():
            print(f"  slot {name}: {rpc(url, 'eth_getStorageAt', [a, slot, block])}")
        for name, data in CALLS.items():
            print(f"  call {name}: {rpc(url, 'eth_call', [{'to': a, 'data': data}, block])}")


main()
