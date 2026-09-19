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

## Engineering

| Date | Milestone | Offline tests | Flaky-test failure rate | Full scan wall clock | Notes |
|---|---|---|---|---|---|
| 2026-09-16 | Baseline | 84 | 18/60 isolated runs of `concurrent_opens…` (measured 2026-09-19) | not recorded | |
| 2026-09-19 | Step 1 | 102 | unchanged (step 2 fixes it) | 2m54s | Resolution paced at 1 call/s to Base; unpaced runs resolved 25 or 28 depending on the rate limiter |
| 2026-09-19 | Step 2: reliable gate, verified table | 114 | **0/200** (20/60 with the old code put back) | 2m54s | 11 hand-verified addresses replayed offline in 2.8s; live `hermes verify` 11/11 in 1m39s |

## PRD §8 minimum bar

| Bar | State |
|---|---|
| Deployed, public, no login | Yes |
| ≥500 proxies indexed and ranked | No: 58 covered |
| Ten protocols hand-verified | **Yes: 11**, by shape, replayed in CI ([docs/verification.md](verification.md)). The human "open every link" pass is still Sharif's |
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
