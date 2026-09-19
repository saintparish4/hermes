# Hermes

A Base-native scanner that ranks upgrade authorities by what they control: if an authority is compromised tomorrow, what can it replace, and how many keys does that take? Hermes ignores contract code and models the capability surface instead — who can upgrade what, how many keys that takes, and whether a timelock stands in the way. The unit of analysis is the authority, not the contract. (Dollar exposure is the next question and is not built yet.)

**Live:** [hermes-production-29bf.up.railway.app](https://hermes-production-29bf.up.railway.app) · [/authorities](https://hermes-production-29bf.up.railway.app/authorities) · [/coverage](https://hermes-production-29bf.up.railway.app/coverage)

20 of the 62 contracts currently indexed — the OP Stack predeploys — answer to **one authority: a 2-of-2 Safe on Ethereum**, acting on Base through its L1→L2 alias. Its owners are a 3-of-6 and an 8-of-11 Safe, so taking control takes **11 keys**. A contract-indexed tool shows 62 rows of equal weight; this is the difference.

The admin slot is only one of three places upgrade rights live, so Hermes follows all three: the ERC-1967 admin, a beacon proxy's beacon, and a UUPS proxy's own `owner()`. Six beacon proxies in the index answer to one 2-of-3 Safe through a shared beacon; one EOA is `owner()` of three UUPS proxies. Of 58 covered proxies, 43 resolve to a root, and each of the other 15 carries a named reason (see `/coverage`).

**Correction.** Until 2026-09-19 this README, and the live `/authorities` page, said that one *EOA* controlled those 20 contracts. That was wrong. The `ProxyAdmin`'s owner has no code on Base because it is the L2 alias of a contract on Ethereum, and Hermes read "no code" as "a single key". The resolver now checks the unaliased address on L1 before it will call anything a key. The five remaining single-key authorities were re-checked the same way and are genuine.

## Requirements:

- Rust (stable toolchain via `rustup`)
- Node.js (LTS) and `npm` — for the Next.js frontend (`home/`) only; the backend has no Node dependency
- A Base RPC endpoint — a public endpoint to start, an Alchemy or QuickNode free tier once rate limits bite
- `sqlite3` CLI for inspecting the store during development
- [`foundry`](https://getfoundry.sh/) (specifically `cast`) — for verifying storage-slot reads by hand against Basescan
- Target platforms: macOS, Linux, or WSL2

## Installation:

```bash
git clone <repo-url>
cd hermes
```

**Frontend** (`home/`, the Next.js landing page):

```bash
cd home
npm install
```

**Backend** (Rust workspace) — scan Base and serve the result, no configuration required:

```bash
cargo run -p hermes-cli -- scan && cargo run -p hermes-cli -- serve
```

Then open [http://localhost:8080](http://localhost:8080). The scan probes the 62 hand-curated addresses against the public Base endpoint and writes `hermes.db`; the server reads it. Neither step needs an API key. To index a real share of Base, discover first:

```bash
cargo run -p hermes-cli -- discover   # ~2 minutes: finds proxies chain-wide from their ERC-1967 events
cargo run -p hermes-cli -- scan       # then scans everything seeded; long, resumable
```

## Development:

**Frontend:**

```bash
cd home
npm run dev
```

Open [http://localhost:3000](http://localhost:3000).

**Backend:**

```bash
cargo run -p hermes-cli -- discover                 # add proxies found by event to the seed table
cargo run -p hermes-cli -- scan --concurrency 3     # 3 is what the public Base endpoint tolerates
cargo run -p hermes-cli -- scan --classify-only     # slots only, no resolution: seconds, not minutes
cargo run -p hermes-cli -- serve --port 8080
```

`hermes scan` works through whatever is due in batches of 50, writing each batch before starting the next. An address scanned in the last 20 hours (`--fresh-hours`) is not due, so a scan killed anywhere resumes where it stopped instead of starting over. A covered proxy that has never had resolution attempted is always due. Each batch has to pass a canary first: if more than a fifth of the proxies it already knew as covered suddenly come back as something else, the run stops and writes nothing from that batch. An endpoint failing is far likelier than a fifth of a batch being re-pointed at once.

`hermes scan` fails loudly rather than reporting success when a run stores zero covered proxies — a scan that quietly stores nothing, followed by a server that cheerfully serves an empty table, is how a public dashboard starts lying.

Three developer subcommands ride in the same binary, because they have to run exactly the pipeline the deployment runs:

```bash
cargo run -p hermes-cli -- record 0x4200000000000000000000000000000000000010 --name l2-standard-bridge \
    --block 51526000 --l1-block 26013200   # saves every chain answer to tests/fixtures/<name>.json
cargo run -p hermes-cli -- verify          # re-checks tests/verified.json against the live chain
cargo run -p hermes-cli -- migrate         # brings the database to the current schema and exits
```

`record` prints what Hermes concluded, but that output never becomes an expectation. Expectations in `tests/verified.json` are established by reading the chain without Hermes (see [docs/verification.md](docs/verification.md)); a table that learned its answers from the program under test could not catch that program being wrong.

CI (`.github/workflows/ci.yml`) runs on every push/PR to `master` across Ubuntu and macOS:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked
```

Clippy's `cognitive_complexity` lint is denied workspace-wide with the threshold set in `clippy.toml`. It is a nursery lint, so `-D warnings` alone would never reach it.

A second workflow (`.github/workflows/verify-live.yml`) runs `hermes verify` daily against the live chain and is allowed to fail. The two are kept apart on purpose: CI replays pinned fixtures, so red there means Hermes regressed; red in the live job means Hermes regressed *or the chain moved*. Re-recording the fixture at the new head and diffing it says which.

## Testing:

`cargo test --workspace`. Every test is offline — pure functions, `sqlite::memory:`, a temp file, or recorded chain answers replayed from `tests/fixtures/` — because network-dependent tests are flaky tests.

Shipped today:

- **Slot-constant derivation** — the constants are re-derived from `keccak256` in the test rather than restated. One wrong nibble in `IMPL_SLOT` classifies every address on Base as not upgradeable while the scan, the coverage number and the rest of the suite all stay green.
- **Classification** — every branch, including the precedence rules and the coverage set.
- **Negative-read merging** — the exact spurious-blank case that would otherwise turn a live proxy into "not upgradeable".
- **The resolver** — cheapest-m-of-n key arithmetic, cycles, depth truncation, confidence that only falls, thresholds that lie, and the aggregation property: two proxies under distinct `ProxyAdmin`s sharing one Safe collapse to a single authority.
- **L1→L2 aliasing** — the alias arithmetic against the real `0x8cC5…6a6d ↔ 0x7bB4…595c` pair, including wrap-around at both ends of the address space; the live predeploy structure resolving to a Safe on Ethereum at 11 keys; its leaf keys landing exactly on the depth cap; an unread L1 staying undetermined rather than becoming a key; and the same address on two chains being two accounts.
- **The hand-verified table** — thirteen addresses chosen by shape ([docs/verification.md](docs/verification.md)): the L1-aliased predeploy authority, the self-administered `ProxyAdmin`, an `AdminOnly` predeploy, a Safe as direct admin, a `ProxyAdmin` under a Safe, a genuine single key, an admin that is honestly unknown, a UUPS proxy resolved through `owner()`, one whose implementation denies being UUPS, one that is UUPS but not owner-gated, a beacon proxy resolved through its beacon, USDC (not covered) and WETH9 (not upgradeable). Each has a fixture recorded at the block its expectation was checked at, and CI replays every one through the production pipeline. Replay is keyed by (chain, method, address, slot or calldata), never by arrival order, and a read the recording never saw fails loudly instead of looking like an outage.
- **Concurrent opens** — eight processes' worth of `Store::open` against one file, spawned rather than awaited in turn. It used to fail 18 times in 60 with `database is locked`: switching a file into WAL needs a lock SQLite will not wait for through the busy timeout, and every pooled connection was asking. The switch now happens once, with a bounded retry, and the migration runs in one `BEGIN IMMEDIATE` transaction. 0 failures in 200 runs; 20 in 60 with the old per-connection switch put back.
- **Upgrade paths and reasons** — which entry each proxy kind walks from, the UUPS `proxiableUUID()` gate and Medium ceiling, the two kinds of Unknown kept apart (an outage vs. a contract that answered nothing recognizable), and the store rule that an outage keeps a stored root while a real finding replaces it.
- **Store and API** — upsert idempotency, label preservation, the grouping properties, resolution columns replaced as a unit (so a new root never inherits the old root's key count), one-time retraction of resolutions made before aliasing was modelled, reads that name their columns (a `SELECT *` on a pooled connection with a pre-migration schema panicked the driver 7 times in 60), default filtering, 404-not-500, 409 on a cross-chain ambiguous authority, case-insensitive lookup.

Each fix above was checked by putting the bug back and watching its test fail.

Still to come, alongside the work that needs them:

- **A timelock row** in the verified table, once the index contains an admin chain that has one.
- **Property tests** (`proptest`) for the recursive resolver — cycle detection, depth limiting, and `compromise_depth` arithmetic over contracts that lie.

**Frontend:**

```bash
cd home
npm run lint
```

## Environment Variables:

All optional — every one has a working default.

| Variable | Default | Used by |
|---|---|---|
| `HERMES_DB` | `sqlite://hermes.db` | both |
| `HERMES_RPC_URL` | `https://mainnet.base.org` | `scan` |
| `HERMES_L1_RPC_URL` | `https://eth.drpc.org` | `scan` (Ethereum reads behind aliased authorities) |
| `HERMES_CONCURRENCY` | `3` | `scan` (slot reads) |
| `HERMES_CALL_INTERVAL_MS` | `1000` | `scan` (gap between resolution calls to Base) |
| `HERMES_STATIC_DIR` | `static` | `serve` |
| `PORT` | `8080` | `serve` |
| `HERMES_SCAN_INTERVAL` | `86400` | container refresh loop |

On `HERMES_CONCURRENCY`: 3 concurrent readers complete a scan against the public Base endpoint with zero failures, where 8 fail *silently* — the endpoint returns empty code and zero storage words for contracts that demonstrably have both. Raise it only against a paid endpoint.

On `HERMES_CALL_INTERVAL_MS`: the public Base endpoint allows about ten back-to-back `eth_call`s, then answers HTTP 429 until it has seen roughly seven seconds of quiet (measured 2026-09-19). Unpaced, the resolver lost different nodes on different runs, so the resolved count wandered between scans (25 on one run, 28 on the next). At one call per second it resolved 28 on consecutive runs with no undetermined reads. Set it to `0` against a keyed endpoint.

## Architecture:

### Core stack

- **Language**: Rust for all backend code.
- **Chain access**: [`alloy`](https://github.com/alloy-rs/alloy) — provider, ABI encoding, primitive types. Chosen over `ethers-rs`, which is deprecated in favor of alloy.
- **Async**: `tokio`, with `futures::stream::buffer_unordered` for bounded RPC concurrency.
- **Storage**: SQLite via `sqlx`. Chosen over Postgres deliberately — the dataset is small (<10⁵ rows) and it removes a deployment dependency.
- **Serving**: `axum`, handling both the JSON API and the static frontend bundle.
- **Frontend**: Next.js, static export only (`output: 'export'`) — no SSR, no API routes, no server actions, no auth.
- **Pricing** (planned, not built): DeFiLlama coins API — chain-prefixed addresses (`base:0x…`), batch queries, no API key.

### Repository layout

```
hermes/
├── crates/
│   ├── hermes-core/     # slot constants, classification, L1→L2 aliasing, the pure resolver, SQLite store
│   ├── hermes-scan/     # all network I/O: slot probing, authority probing on Base and Ethereum
│   ├── hermes-api/      # axum server, JSON endpoints, static file serving
│   └── hermes-cli/      # scan orchestration entrypoint
├── home/                # Next.js landing page, static export
├── static/
│   └── index.html       # minimal fallback page, served independently of the frontend build
└── Cargo.toml           # workspace root
```

### Where the addresses come from

Two sources feed one `seed` table:

- **Curated** — 62 hand-picked addresses (`crates/hermes-scan/src/seed.rs`): the OP Stack predeploys, a sample of live proxies, and USDC, EURC and WETH9, which are there *because* Hermes cannot classify them. They bootstrap every database and are the only source of labels.
- **Discovered** — `hermes discover` pages `eth_getLogs` for the three ERC-1967 events (`Upgraded`, `BeaconUpgraded`, `AdminChanged`), so proxies are found by the same standard the slot reads classify them by. The public endpoint caps a log response by size (1,000 blocks is fine, 10,000 is HTTP 413), so it reads 1,000-block windows at fixed positions every 500,000 blocks across Base's whole history. Fixed positions make it resumable (a cursor names the next window) and incremental (new windows come into range as the chain grows). No API key, no Basescan export.

**The index is a sample, and this is how it is drawn.** Recent history is mostly clones: in one 1,000-block window, 248 of 374 upgrades pointed at one implementation. Admitted unfiltered, the index would be a few contracts copied hundreds of times, each owned by whoever deployed that copy. So each *family* — one implementation, beacon or admin — admits at most three addresses (`--per-family`). Measured on 2026-09-19: 104 windows, 903 addresses admitted from 555 families. `/coverage` reports the cap it was drawn with. The consequence to keep in mind: "one authority controls N proxies" is a count within this sample, and for a clone family it is at most three.

### Proxy discovery

ERC-1967 standardizes three storage slots, each computed as `bytes32(uint256(keccak256(name)) - 1)`, and each readable via a single `eth_getStorageAt` call:

| Slot | Holds |
|---|---|
| Implementation | Logic contract the proxy delegates to |
| Admin | Address permitted to upgrade the logic contract |
| Beacon | Beacon contract, when used instead of a direct logic address |

Implementation + admin set → Transparent proxy. Implementation set, admin zero → UUPS. Beacon set → Beacon proxy. All zero → probe the EIP-1822 `PROXIABLE` slot, otherwise not upgradeable.

### Where upgrade rights live

Before walking anything, Hermes decides where the power to upgrade each proxy actually sits:

| Proxy | Walk starts at | Confidence ceiling |
|---|---|---|
| Transparent, AdminOnly | the ERC-1967 admin | High |
| Beacon | the beacon, whose controller upgrades every proxy pointing at it | High |
| UUPS | the proxy itself, asked `owner()` through its implementation, **only after** the implementation answers `proxiableUUID()` with the ERC-1967 slot | Medium: answering `owner()` does not prove `owner` is what `_authorizeUpgrade` checks |
| EIP-1822 without an ERC-1967 implementation | nowhere: `no_upgrade_path` | — |

A covered proxy with no root always says why: `unrecognized_interface` (the walk reached a contract answering nothing Hermes recognizes), `uups_unconfirmed` (the implementation denies being UUPS), `no_upgrade_path`, or `rpc_undetermined` (a node would not answer). A root with no key count says why too: `truncated`, `cycle`, or `owners_unknown`. An outage never replaces a stored root; a real finding does.

### Authority resolution

An admin address is resolved to a governance structure by probing interfaces and recursing to depth 4 with cycle detection:

- `getOwners()` + `getThreshold()` ⇒ Gnosis Safe
- `owner()` ⇒ OZ `ProxyAdmin` or `Ownable`; recurse on the result
- `getMinDelay()` ⇒ `TimelockController`; capture the delay
- Empty code on Base ⇒ **not yet an answer.** An Ethereum contract acts on Base at its own address plus `0x1111…1111`, and that aliased address has no code. So Hermes reads the unaliased address on Ethereum: code there ⇒ follow the walk onto Ethereum; confirmed empty there too ⇒ EOA, terminal; unreadable ⇒ `Unknown`, never EOA
- No interface matches ⇒ `Unknown`, flagged rather than guessed

Each proxy resolves to a `terminal_authority` (the root of the chain, not the immediate admin), a `terminal_chain` (`base` or `ethereum`), a `compromise_depth` (minimum distinct key compromises required to execute an upgrade, `null` when any part of the chain is unknown), a `timelock_seconds`, and a `resolution_confidence`. An m-of-n Safe costs the sum of its *m cheapest* owners.

The predeploy authority walks four links — `ProxyAdmin` (Base) → alias (Base) → 2-of-2 Safe (Ethereum) → a 3-of-6 and an 8-of-11 Safe → 17 keys — which lands its leaf keys exactly on the depth cap. A test pins that, so if those leaves ever turn out to be Safes the answer becomes `null` rather than a quietly miscounted number.

### Exposure aggregation (planned, not built)

Nothing below is implemented yet; it is the v1.1 design. Per proxy, `direct_custody` = native ETH + Σ(ERC-20 balance × price), read via `get_balance` and `balanceOf` batched through Multicall3. Per authority, `authority_var` = Σ `direct_custody` across every proxy whose `terminal_authority` is that authority. If an authority can replace a proxy's implementation, its exposure is that proxy's full custody — arbitrary code replacement subsumes every other capability.

### Pipeline

Built today:

```
hand-curated seed (62) + eth_getLogs for ERC-1967 events (windows across history, ≤3 per family)
      ↓
seed table → due for scan (not scanned in 20h, or never resolved) → batches of 50
      ↓
eth_getStorageAt × 5 slots + eth_getCode → proxy classification → admin address
      ↓
upgrade entry per proxy: admin slot, beacon, or proxiableUUID()-confirmed UUPS owner()
      ↓
recursive authority resolution on Base, crossing to Ethereum through L1→L2 aliases
      ↓
canary per batch → SQLite, one transaction per batch
      ↓
GROUP BY (terminal_authority, terminal_chain) → JSON API + static page
```

Planned: Multicall3 balance reads and DeFiLlama prices for `direct_custody` and `authority_var`.

### API

Served today:

- `GET /authorities` — resolved roots ranked by how many proxies each controls, each with its `chain`, `kind`, `compromise_depth`, `timelock_seconds` and `confidence`. Unresolved proxies are left out rather than bucketed under a placeholder; `/coverage` counts them.
- `GET /authorities/{address}` — one authority and every proxy it controls. If the same address is a root on both Base and Ethereum, pass `?chain=base` or `?chain=ethereum`; without it the answer is a 409 naming both, not whichever row sorts first.
- `GET /proxies` — covered proxies with their classification and resolved root, paged: `?limit=` (default 100, at most 1,000) and `?offset=`, with `total` in the response. `?all=true` includes non-proxies.
- `GET /proxies/{address}` — one proxy.
- `GET /coverage` — scan counts, resolved count, distinct roots, last scan time, the gap explained (`unresolved_by_reason`, `depth_unknown_by_reason`, `resolved_by_path`), and how the index was sampled (`seeded_by_source`, `discovery_per_family`).
- `GET /healthz` — plain `ok`, never touches SQLite.

Planned: ranking by `authority_var` once exposure exists, and `GET /methodology`.

## Deployment:

Hermes ships as a single Rust binary serving the API and the statically exported frontend, with SQLite as an embedded file — no external services required.

```
hermes source (Rust)          home/ (Next.js)
      ↓                             ↓
Rust compiler                 next build (output: 'export')
      ↓                             ↓
      └────────── embed / serve ────┘
                     ↓
      native executable + SQLite file
```

The container refreshes on a loop behind the server, writing to the same SQLite file the API reads from. Two boot paths, depending on whether there is anything to serve:

- **Database empty** — classify the 62 curated addresses first (about twenty seconds, no resolution), then open the port. Serving an empty table is worse than making the first visitor wait, and this is the state a first deploy starts in, or every deploy if the volume is ever missing. A full first scan would not fit: resolution is paced to what the public endpoint tolerates and reads Ethereum as well as Base, so it takes minutes, far past the 60-second health check.
- **Database populated** — run `hermes migrate` once, alone, then open the port immediately and refresh in the background. `Store::open` is safe to race, but a deploy that adds a column is exactly when the scan and the server would otherwise both be changing the schema.

Either way, the background loop then runs `hermes discover` and a full `hermes scan` straight away, and every `HERMES_SCAN_INTERVAL` after that. Refreshes never run in front of the port. A redeploy that kills a scan loses at most one batch; the next boot's scan skips everything written in the last twenty hours.

### Railway

```bash
railway init
railway volume add --mount-path /data     # required — see below
railway up
```

**The volume is not optional.** Without it the SQLite file lives in the container filesystem, and every redeploy silently starts from an empty scan — the API answers, the page renders, and there is nothing in it. `HERMES_DB` already points at `sqlite:///data/hermes.db`, so mounting at `/data` is all it takes.

Two things that will bite otherwise:

- **Volume permissions.** The image runs as UID 10001 rather than root, and Railway documents that non-root images hit permission errors on an attached volume. If the first deploy cannot write to `/data`, set `RAILWAY_RUN_UID=0` as a service variable.
- **The refresh loop lives in the container.** Railway allows one volume per service, so a separate scheduled service could not reach this database at all. `HERMES_SCAN_INTERVAL` controls the period and defaults to 86400 seconds.

`healthcheckPath` points at `/healthz`, which deliberately does not touch SQLite. A health check that queried the database would get the container killed during a scan, on exactly the runs that matter most.

### Anywhere else

The image has no Railway-specific anything in it — mount a volume at `/data`, set `PORT`, and it runs.

```bash
docker build -t hermes .
docker run -p 8080:8080 -v hermes-data:/data hermes
```

## Contributing:

Open a PR against `master`. CI must pass before merge — run the same checks locally first:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked
```

CI runs this matrix across Ubuntu and macOS on every push and pull request to `master`. Dependabot keeps Cargo and GitHub Actions dependencies current on a weekly schedule.

## License:

[MIT](LICENSE) © 2026 Sharif Parish / Bluesky Labs
