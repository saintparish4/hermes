# Progress log

One row per milestone, newest last. Every figure is reproducible: the scan columns come from
`python3 scripts/metrics.py <url>` against the instance named in the row, the test columns
from `cargo test --workspace` on the named commit.

"Live" means `https://hermes-production-29bf.up.railway.app`. "Local" means a fresh
`hermes scan` against the public endpoints into an empty database on the named date. A local
row is what the next deploy will serve; it is not live until it is deployed.

## Scan

| Date | Milestone | Where | Scanned | Covered | Resolved | Roots | Single-key roots (proxies) | Largest root |
|---|---|---|---|---|---|---|---|---|
| 2026-09-16 | Baseline (`0202bec`) | live | 62 | 58 | 28 | 9 | 6 (25), **wrong** | "EOA", 20 proxies, 1 key, **wrong** |
| 2026-09-19 | Step 1: L1→L2 aliasing | local | 62 | 58 | 28 | 9 | 5 (5) | Safe on Ethereum, 20 proxies, 11 keys |
| 2026-09-19 | Step 3: UUPS, beacon, reasons | local | 62 | 58 | **43** | 16 | 9 (11) | Safe on Ethereum, 20 proxies, 11 keys |
| 2026-09-20 | Step 4: chain-wide index | local | **964** | **939** | **531** | **206** | 163 (384) | EOA on Base, 65 proxies, 1 key |
| 2026-09-20 | Step 4 deployed (`9d1ef16`) | live | 964 | 939 | 531 | 206 | 163 (384) | EOA on Base, 65 proxies, 1 key |

Step 3 breakdown: resolved by path, admin slot 28, UUPS `owner()` 9 (all Medium), beacon 6.
Unresolved by reason: `unrecognized_interface` 14 (7 under `0x31e9…0c17`, 5 UUPS without an
`owner()`, 2 beacons whose controller answers nothing), `uups_unconfirmed` 1. Second-largest
root: a 2-of-3 Safe on Base behind six beacon proxies.

Step 4 breakdown: 903 addresses from ERC-1967 event discovery plus the 61 curated seeds, at
most 3 per implementation/beacon/admin family. Covered by kind: UUPS 472, beacon 259,
transparent 206, `admin_only` 2. Resolved by path: beacon 199, admin slot 171, UUPS `owner()`
161. Unresolved by reason: `unrecognized_interface` 345, `uups_unconfirmed` 63. Confidence of
the 531 resolved: High 348, Medium 183. Of the 206 roots, 161 are EOAs, 39 Safes, 6 timelocks;
16 have an unknown key count (50 proxy rows, all `owners_unknown`). Single-key roots split
56 roots / 246 proxies at High confidence and 107 roots / 138 proxies at Medium.

The index is a sample, not a census: discovery reads 1,000-block windows at fixed positions
every 500,000 blocks and caps each family at 3, so every count here is within that sample.

## Engineering

| Date | Milestone | Offline tests | Flaky-test failure rate | Full scan wall clock | Notes |
|---|---|---|---|---|---|
| 2026-09-16 | Baseline | 84 | 18/60 isolated runs of `concurrent_opens…` (measured 2026-09-19) | not recorded | |
| 2026-09-19 | Step 1 | 102 | unchanged (step 2 fixes it) | 2m54s | Resolution paced at 1 call/s to Base; unpaced runs resolved 25 or 28 depending on the rate limiter |
| 2026-09-19 | Step 2: reliable gate, verified table | 114 | **0/200** (20/60 with the old code put back) | 2m54s | 11 hand-verified addresses replayed offline in 2.8s; live `hermes verify` 11/11 in 1m39s |
| 2026-09-19 | Step 3: UUPS and beacon paths | 127 | 0 | 6m37s | 13 verified rows; more nodes probed at 1 call/s, so the scan more than doubled |
| 2026-09-20 | Step 4: chain-wide index | **148** | 0 | 2h15m31s (964 addresses) | Discovery 2m07s for 104 windows. Resumability re-proven: SIGKILL at 912s left 100 rows durable, the resumed pass wrote the remaining 864 in 18 batches, 0 failed, 0 unconfirmed, 21 confirming re-reads |

## PRD §8 minimum bar

| Bar | State |
|---|---|
| Deployed, public, no login | Yes |
| ≥500 proxies indexed and ranked | **Yes: 939 covered**, live since 2026-09-20 |
| Ten protocols hand-verified | **Yes: 13**, by shape, replayed in CI ([docs/verification.md](verification.md)). The human "open every link" pass is still Sharif's |
| README a stranger can follow | Yes, and the headline claim is now correct |
| One published write-up | No |

## Findings log

- **2026-09-19: the headline was an aliasing artifact.** The `ProxyAdmin` owner behind 20
  OP Stack predeploys, `0x8cC5…6a6d`, has no code on Base because it is the L2 alias of
  `0x7bB4…595c`, a 2-of-2 Safe on Ethereum. Its owners are a 3-of-6 (`0x9855…46a1`) and an
  8-of-11 (`0x20ac…a4dd`) Safe, and all 17 of their owners have no code on Ethereum (block
  26,012,852). Taking control takes 11 keys, not 1. Matches L2BEAT's description of Base's
  upgrade path (Coordinator multisig plus Security Council).
- **2026-09-19: the other five single-key roots are genuine.** Each one's unaliased address
  has no code on Ethereum, on two independent endpoints.
- **2026-09-19: `0x31e9…0c17`, the admin of 7 proxies, is a trading contract**, not an
  authority interface Hermes knows. Its selectors decode to `getAllTesseraPools()`,
  `swapAmount(...)`, `killContract()` and a bespoke `multiSigOwner()` that returns a 1-of-2
  Safe. Nothing establishes that getter gates upgrades, so it stays `unrecognized_interface`
  rather than becoming a guess.
- **2026-09-19: one EOA (`0xb045…7adb`) is `owner()` of three UUPS proxies**, Medium
  confidence because `owner()` answering does not prove it gates upgrades.
- **2026-09-19: the `database is locked` flake was the WAL switch, not the migration.** All 29
  captured failures were at connect: sqlx issued `journal_mode = WAL` on every pooled
  connection, and SQLite will not wait for that lock through the busy timeout. One switch with
  a bounded retry, then the migration in one `BEGIN IMMEDIATE`: 0 of 200.
- **2026-09-19: USDbC's 3-of-6 Safe on Base shares five of six signers** with the 3-of-6 Safe
  on Ethereum behind the predeploys.
- **2026-09-19: `SELECT *` after a migration panicked sqlx** in 7 of 60 runs: a pooled
  connection with a cached pre-`ALTER` schema prepared 13 columns and was handed 14. Named
  columns: 0 of 100. Production would have hit this on the deploy that adds `terminal_chain`.
- **2026-09-19: the public Base endpoint's `eth_call` limiter** allows about ten back-to-back
  calls, then answers HTTP 429 until it sees roughly seven seconds of quiet. That, not chain
  state, is why the resolved count wandered between scans.
- **2026-09-20: at chain-wide scale the largest single-key authorities are much larger than
  the seed suggested.** `0x21eb…B5fc` is the beacon controller for **65** proxies and
  `0xDecA…b3B1` is the admin of **62**, both resolved at High confidence — the first through
  the beacon path, the second through the admin slot, neither through the Medium-capped UUPS
  path. Both were checked against the aliasing rule before being called keys: each has empty
  code on Base *and* empty code at its unaliased address (`addr - 0x1111…1111`) on two
  independent Ethereum endpoints. They are genuine EOAs, not the mistake of 2026-09-19.
- **2026-09-20: the seed was not representative of Base.** In the 62-address curated seed,
  transparent proxies outnumbered UUPS 33 to 15. In the 964-address chain-wide sample the
  order inverts: UUPS 472, beacon 259, transparent 206. Step 3's UUPS and beacon paths, worth
  15 resolutions against the seed, are worth 360 here — most of the index would have been
  unresolvable without them.
- **2026-09-20: `unrecognized_interface` is the dominant gap at scale**, 345 of the 408
  unresolved. The long tail of Base does not answer `owner()`, `getOwners()` or
  `getMinDelay()`, and naming that gap is the honest answer; the seed's 41% unresolved rate
  understated it because the seed was picked for protocols with recognizable interfaces.
- **2026-09-20: resumability held under a real kill.** SIGKILL at 912s left 100 rows durably
  written; the resumed pass read the cursor and wrote the remaining 864 with 0 failures. The
  same run had been lost earlier that day because it wrote to `/tmp` and the machine
  rebooted — the resumability is only worth what the filesystem under it is worth.
- **2026-09-20: the first deploy since 2026-08-31 retracted before it re-resolved.** The
  `terminal_chain` migration's `on_add` hook NULLed all 28 resolutions made by the
  pre-aliasing resolver, so the live site served 58 covered / 0 resolved until the background
  scan re-derived each root with its chain. Serving nothing for two hours was preferred to
  serving answers already known to be wrong. The service had not redeployed in three weeks
  because it was deployed once from the CLI and never connected to the repository.
- **2026-09-20: the live scan reproduced the local one to the row**: 964 / 939 / 531 / 206,
  the same kind mix, the same reasons and the same top of the ranking, hours apart and from a
  different network.
- **2026-09-20: `0xFFfF…FfFF` is ranked as a one-key EOA.** It is the `owner()` of one UUPS
  proxy (`0xF1CC…1D41`, Medium). It has no code, so by the probe rules it is an EOA, but it is
  a sentinel no one is known to hold a key for. Hermes reports what the chain says and does
  not infer "renounced"; whether well-known sentinels deserve their own kind is open.
