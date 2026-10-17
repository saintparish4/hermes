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
| 2026-09-29 | Daily scan, no deploy | live | 964 | 939 | 531 | 207 | 163 (321) | Safe 2-of-5 on Base, 63 proxies, 2 keys |
| 2026-10-17 | Model 4: Safe modules, guards, pending owners (`authority-graph`) | local, the Sep 20 sample rescanned at Base 52,734,116 | 964 | 939 | **760** | 376 | 207 (423) | Safe 2-of-5 on Base, 65 proxies, 2 keys |

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

The 2026-10-17 row is every model from 2 to 4 at once: 760 resolved where model 1 had 531, from
the EIP-7702 keys, smart accounts and `AccessControl` roles of model 2. Of the 760, 602 have a key
count; 89 are `roles_unread`, 59 `account_keys_unread`, 8 `safe_modules`, 2 `no_known_key`.
Unresolved: 116 `unrecognized_interface`, 63 `uups_unconfirmed`.

The 2026-09-29 row is the same deploy as the one above it. The largest root changed because the
chain did (see the findings log), and nothing in the store recorded that it had.

## Engineering

| Date | Milestone | Offline tests | Flaky-test failure rate | Full scan wall clock | Notes |
|---|---|---|---|---|---|
| 2026-09-16 | Baseline | 84 | 18/60 isolated runs of `concurrent_opens…` (measured 2026-09-19) | not recorded | |
| 2026-09-19 | Step 1 | 102 | unchanged (step 2 fixes it) | 2m54s | Resolution paced at 1 call/s to Base; unpaced runs resolved 25 or 28 depending on the rate limiter |
| 2026-09-19 | Step 2: reliable gate, verified table | 114 | **0/200** (20/60 with the old code put back) | 2m54s | 11 hand-verified addresses replayed offline in 2.8s; live `hermes verify` 11/11 in 1m39s |
| 2026-09-19 | Step 3: UUPS and beacon paths | 127 | 0 | 6m37s | 13 verified rows; more nodes probed at 1 call/s, so the scan more than doubled |
| 2026-09-20 | Step 4: chain-wide index | **148** | 0 | 2h15m31s (964 addresses) | Discovery 2m07s for 104 windows. Resumability re-proven: SIGKILL at 912s left 100 rows durable, the resumed pass wrote the remaining 864 in 18 batches, 0 failed, 0 unconfirmed, 21 confirming re-reads |
| 2026-09-30 | The authority graph: kept graph, history, reach, new kinds, policy check (`authority-graph` branch) | **239** | 0 | see the model 4 scan row | 22 verified rows (was 13). A live `hermes pin` bisected the Sep 20→29 bracket (~410,000 blocks) to 51,927,178 in 38s, six of those seconds rate-limit backoff |
| 2026-10-17 | Model 4, the fork oracle, the backtest, mutation testing | **319** (+2 ignored live checks) | 0 | 1h51m (964 addresses, sharing the endpoint with a backtest) | `cargo mutants` over `authority.rs`, `blast.rs`, `graph.rs`: 303 mutants, 4 alive and equivalent. Oracle and backtest results in `docs/audit/2026-10-17` |

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
- **2026-09-29: the largest authority changed hands, and Hermes could not say so.** EOA
  `0x21eb…B5fc` owned the beacons behind 65 indexed proxies on 2026-09-20. On 2026-09-29 it
  handed 27 of its 28 beacons to `0x6454…Bf15`, a 2-of-5 Safe of which it is one owner. Bisecting
  `owner()` of beacon `0xe68E…7706` by hand put the first transfer at Base block 51,927,178
  (01:01:43Z); three more beacons checked by hand moved in later transactions. The scan stored the
  new state and nothing about the old one. The Sep 20 figure survived only because this log wrote
  it down. It is the case that made history part of the product, and both sides are now verified
  rows (`sep29-before`, `sep29-after`).
- **2026-09-30: 63% of the unresolved gap is recognizable.** Following `owner()` from each of the
  345 `unrecognized_interface` proxies to where it stops and asking 13 selectors (a throwaway
  script, not Hermes): 147 end at an ERC-4337 account (`entryPoint()`), 70 at OpenZeppelin
  `AccessControl`, 122 at nothing recognized, 3 at the CREATE2 deployer (which burns all gas on
  any call). Only 3 of the 70 enumerate role members.
- **2026-09-30: a quarter of those "smart accounts" are keys.** 23 of 87 are EOAs with an EIP-7702
  delegation designator (`0xef0100` + address), which answer their delegate's interface. Two of
  them, `0x7fa6…d27c` and `0x6668…75b9`, each stand behind 25 indexed proxies. 16 more expose
  Coinbase's MultiOwnable signer list; 55 use schemes Hermes does not read. Three indexed "UUPS
  proxies" are themselves 7702-delegated EOAs whose storage holds a Coinbase Smart Wallet
  implementation.
- **2026-09-30: an empty upgrade role is not "nobody can upgrade".** `0x66d6…6Af0` is
  AccessControlEnumerable with no holder of `UPGRADER_ROLE`, but its admin role (which can grant
  the upgrade role) has one holder, a key. Counting only the upgrade role would have reported no
  key count; counting it naively would have suggested no one can upgrade.
- **2026-09-30: USD+ is role-gated, not unrecognized.** The verified row for "UUPS, access control
  not recognized" (`0xB79D…4376`) answers `supportsInterface(IAccessControl)` and
  `DEFAULT_ADMIN_ROLE() = 0`; its members are not enumerable. Under model 2 its root is its own
  admin role with keys unknown (`roles_unread`), re-checked by hand at the original block.
- **2026-09-30: Multicall3 is unsafe for probes.** Batching 13 probes for 382 nodes took about a
  minute against ~25 sequential, but one gas-burning subcall starves the ones after it (and at
  the node's 600M gas cap took a whole batch down), turning real answers into false "no"s.
  Not adopted.
- **2026-09-30: the public Base endpoint refuses `eth_getLogs` over ~2,000 blocks even for one
  address** (HTTP 413 at 3,000). Full-history backfill per address is ~26,000 calls; history is
  forward from first sight until a keyed endpoint exists.
- **2026-09-30: both public endpoints serve `finalized` and historical state.** Base's finalized
  block lags the head by ~886 blocks (~30 min), Ethereum's by ~79. drpc answered `eth_call` 50,000
  blocks back. Scans now pin to finalized.
- **2026-10-17: a Safe's threshold is not the only way in, four times.** Of 44 Safes the index
  roots at or nests (read without Hermes, `docs/audit/2026-10-17/safe-setups.json`), 4 have an
  enabled module and none a guard; every singleton is one of Safe's own. Two modules are Safe's
  Allowance Module; two are Zodiac Roles modifiers, one of them with four members of its own, two
  Safes below the root. Model 4 leaves those counts unknown (`safe_modules`, 8 proxies).
- **2026-10-17: the EVM agrees with every count reached through a ProxyAdmin, a beacon or a Safe,
  and with fewer than half reached through UUPS `owner()`.** Forking Base at the scan's block and
  acting as exactly the named keys (`hermes oracle`, 12 rows per stratum): admin slot, beacon and
  UUPS-under-a-Safe strata confirmed 36 of 36 decided rows. UUPS rows whose `owner()` is a key
  confirmed 5 of 12 (one only re-installing the current code), and whose owner is a smart account
  0 of 7. The 14 contradictions, each repeated on a second fork and read by hand
  (`docs/audit/2026-10-17/contradictions.md`): 3 refuse the caller (`Only Mamo Strategy Registry
  can call`), 7 accept only an implementation a registry approves, 4 revert bare. Two of the 14
  are rows of the hand-verified table (`uups-owner`, `multi-owner-account`), whose expectations a
  person reading `owner()` set; the table could not catch it. UUPS roots were already capped at
  Medium for this reason; now there is a rate.
- **2026-10-17: past upgrades came from the named authority.** 15 sighted upgrades of indexed
  proxies and beacons that were not deployments (104 were), judged from their receipts against
  Hermes at the block before (`hermes backtest`): 14 consistent, 1 with no root to test, none
  contradicted (95% Wilson for consistent: 78% to 100%). Three apparent contradictions on the way
  were the backtest's own mistakes and are tests now: an unlisted role holder, and two upgrades a
  relayer sent for an EIP-7702 key.
- **2026-10-17: the public Base endpoint refuses every `eth_getLogs`** (`-32011 request limit
  reached`), one block for one address included, while other methods answer. `hermes discover`
  in the container loop depends on it. `base.drpc.org` served 100-block windows; the Ethereum
  default `eth.drpc.org` did not connect at all, and `rpc.mevblocker.io` served Ethereum archive
  state.
- **2026-10-17: the fork oracle found its own bugs first.** A missing receipt read as a revert
  (every trial failed, a stranger looked refused), an upgrade function the code lacks read as a
  refusal, an allowlist refusing the planted code read as the keys failing, and one fork refused
  an upgrade the next accepted. Each became a rule: only a status decides, the dispatcher is read
  first, the current code is re-installed, and a refusal stands only when a second fork repeats it.
- **2026-09-20: `0xFFfF…FfFF` is ranked as a one-key EOA.** It is the `owner()` of one UUPS
  proxy (`0xF1CC…1D41`, Medium). It has no code, so by the probe rules it is an EOA, but it is
  a sentinel no one is known to hold a key for. Hermes reports what the chain says and does
  not infer "renounced"; whether well-known sentinels deserve their own kind is open.
  *Resolved 2026-09-30:* sentinels are a kind with no key count (`no_known_key`); this proxy is the
  verified row `renounced-to-sentinel`.
