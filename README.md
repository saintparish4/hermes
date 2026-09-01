# Hermes

- A Base-native scanner that ranks every privileged key on the chain by what it controls: if an authority is compromised tomorrow, how much money moves, and how long do you have to react? Hermes ignores contract code and models the capability surface instead — who can upgrade what, how many keys that takes, whether a timelock stands in the way, and what the resulting exposure is in dollars. The unit of analysis is the authority, not the contract.

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

Then open [http://localhost:8080](http://localhost:8080). The scan probes the seeded addresses against the public Base endpoint and writes `hermes.db`; the server reads it. Neither step needs an API key.

## Development:

**Frontend:**

```bash
cd home
npm run dev
```

Open [http://localhost:3000](http://localhost:3000).

**Backend:**

```bash
cargo run -p hermes-cli -- scan --concurrency 3   # 3 is what the public Base endpoint tolerates
cargo run -p hermes-cli -- serve --port 8080
```

`hermes scan` fails loudly rather than reporting success when a run stores zero covered proxies — a scan that quietly stores nothing, followed by a server that cheerfully serves an empty table, is how a public dashboard starts lying.

CI (`.github/workflows/ci.yml`) runs on every push/PR to `master` across Ubuntu and macOS:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked
```

Clippy's `cognitive_complexity` lint is denied workspace-wide with the threshold set in `clippy.toml`. It is a nursery lint, so `-D warnings` alone would never reach it.

## Testing:

`cargo test --workspace`. Every test is offline — pure functions or `sqlite::memory:` — because network-dependent tests are flaky tests.

Shipped today:

- **Slot-constant derivation** — the constants are re-derived from `keccak256` in the test rather than restated. One wrong nibble in `IMPL_SLOT` classifies every address on Base as not upgradeable while the scan, the coverage number and the rest of the suite all stay green.
- **Classification** — every branch, including the precedence rules and the coverage set.
- **Negative-read merging** — the exact spurious-blank case that would otherwise turn a live proxy into "not upgradeable".
- **Store and API** — upsert idempotency, label preservation, the many-proxies-one-admin grouping property, default filtering, 404-not-500, case-insensitive lookup.

Still to come, alongside the work that needs them:

- **Fixture-based tests** for authority resolution — real Safe and Timelock responses from Base, block-pinned and checked in, tested against fixtures rather than live RPC.
- **Hand-verification suite** — ten protocols with expected `terminal_authority`, `compromise_depth`, and `timelock_seconds` recorded in a checked-in table, curated by shape rather than by TVL.
- **Aggregation test** — two proxies under distinct `ProxyAdmin` contracts sharing one Safe owner must collapse to a single authority.
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
| `HERMES_CONCURRENCY` | `3` | `scan` |
| `HERMES_STATIC_DIR` | `static` | `serve` |
| `PORT` | `8080` | `serve` |

On `HERMES_CONCURRENCY`: 3 concurrent readers complete a scan against the public Base endpoint with zero failures, where 8 fail *silently* — the endpoint returns empty code and zero storage words for contracts that demonstrably have both. Raise it only against a paid endpoint.

## Architecture:

### Core stack

- **Language**: Rust for all backend code.
- **Chain access**: [`alloy`](https://github.com/alloy-rs/alloy) — provider, ABI encoding, primitive types. Chosen over `ethers-rs`, which is deprecated in favor of alloy.
- **Async**: `tokio`, with `futures::stream::buffer_unordered` for bounded RPC concurrency.
- **Storage**: SQLite via `sqlx`. Chosen over Postgres deliberately — the dataset is small (<10⁵ rows) and it removes a deployment dependency.
- **Serving**: `axum`, handling both the JSON API and the static frontend bundle.
- **Frontend**: Next.js, static export only (`output: 'export'`) — no SSR, no API routes, no server actions, no auth.
- **Pricing**: DeFiLlama coins API — chain-prefixed addresses (`base:0x…`), batch queries, no API key.

### Repository layout

```
hermes/
├── crates/
│   ├── hermes-core/     # slot constants, proxy classification, shared types
│   ├── hermes-scan/     # RPC pipeline: authority resolution, balances, pricing
│   ├── hermes-api/      # axum server, JSON endpoints, static file serving
│   └── hermes-cli/      # scan orchestration entrypoint
├── home/                # Next.js landing page, static export
├── static/
│   └── index.html       # minimal fallback page, served independently of the frontend build
└── Cargo.toml           # workspace root
```

### Proxy discovery

ERC-1967 standardizes three storage slots, each computed as `bytes32(uint256(keccak256(name)) - 1)`, and each readable via a single `eth_getStorageAt` call:

| Slot | Holds |
|---|---|
| Implementation | Logic contract the proxy delegates to |
| Admin | Address permitted to upgrade the logic contract |
| Beacon | Beacon contract, when used instead of a direct logic address |

Implementation + admin set → Transparent proxy. Implementation set, admin zero → UUPS. Beacon set → Beacon proxy. All zero → probe the EIP-1822 `PROXIABLE` slot, otherwise not upgradeable.

### Authority resolution

An admin address is resolved to a governance structure by probing interfaces and recursing to depth 4 with cycle detection:

- `getOwners()` + `getThreshold()` ⇒ Gnosis Safe
- `owner()` ⇒ OZ `ProxyAdmin` or `Ownable`; recurse on the result
- `getMinDelay()` ⇒ `TimelockController`; capture the delay
- Empty code ⇒ EOA, terminal
- No interface matches ⇒ `Unknown`, flagged rather than guessed

Each proxy resolves to a `terminal_authority` (the root of the chain, not the immediate admin), a `compromise_depth` (minimum distinct key compromises required to execute an upgrade), a `timelock_seconds`, and a `resolution_confidence`.

### Exposure aggregation

Per proxy, `direct_custody` = native ETH + Σ(ERC-20 balance × price), read via `get_balance` and `balanceOf` batched through Multicall3. Per authority, `authority_var` = Σ `direct_custody` across every proxy whose `terminal_authority` is that authority. If an authority can replace a proxy's implementation, its exposure is that proxy's full custody — arbitrary code replacement subsumes every other capability.

### Pipeline

```
DeFiLlama list + Basescan export
      ↓
dedupe by checksummed address
      ↓
eth_getStorageAt × 3 slots → proxy classification → admin address
      ↓
recursive authority resolution → terminal_authority
      ↓
Multicall3 balance reads → DeFiLlama price lookup → direct_custody
      ↓
GROUP BY terminal_authority → authority_var
      ↓
SQLite → JSON API + leaderboard
```

### API

Served today: `GET /proxies`, `GET /proxies/{address}`, `GET /authorities`, `GET /coverage`, `GET /healthz`. `/authorities` currently groups by each proxy's *immediate* admin and says so in its response — authority resolution is what turns that into a terminal authority, and the route keeps its shape when it lands.

The full v1 surface:

- `GET /authorities` — ranked list of authorities by `authority_var`.
- `GET /authorities/:address` — one authority: type, threshold, `compromise_depth`, timelock, and every proxy it controls.
- `GET /proxies` — flat proxy list with `direct_custody` and `terminal_authority`.
- `GET /proxies/:address` — one proxy and its full resolution chain.
- `GET /coverage` — scan statistics and confidence distribution.
- `GET /methodology` — what is covered, what is not, and how exposure is computed.

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

The scanner runs as a scheduled job (`hermes scan`) writing to the same SQLite file the API reads from.

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
