# Hermes

**Know who can change the code.** Hermes traces who can upgrade a deployed contract on Base
through every layer of indirection (ProxyAdmin, beacon, UUPS owner, Safe, timelock,
AccessControl role, smart account, and a contract on Ethereum acting through its L1→L2 alias)
down to the keys behind it. It keeps that graph between scans, so it can also say what else an
authority reaches, which Safes share signers, and when any of it changed.

It reads ERC-1967 storage slots and calls interface functions. It does not look for bugs, it
states what *can* upgrade a contract and never whether anyone will, and it has no score and no
dollar figures.

**Live:** [hermes-production-29bf.up.railway.app](https://hermes-production-29bf.up.railway.app) · [/authorities](https://hermes-production-29bf.up.railway.app/authorities) · [/coverage](https://hermes-production-29bf.up.railway.app/coverage) · [methodology](https://hermes-production-29bf.up.railway.app/methodology)

## What it has found

Figures are dated and every one is reproducible; the log is [docs/progress.md](docs/progress.md).
The index is a **sample** of Base, not a census (see [where the addresses come from](#where-the-addresses-come-from)),
so every count is within it.

- **The OP Stack predeploys answer to eleven keys, not one.** Twenty system contracts sit under a
  `ProxyAdmin` whose owner has no code on Base. That owner is the L2 alias of a 2-of-2 Safe on
  Ethereum whose owners are a 3-of-6 and an 8-of-11 Safe (output abridged):

  ```
  $ hermes authority 0x4200000000000000000000000000000000000010
  admin  0x4200000000000000000000000000000000000018  owned contract
  └─ owner  0x8cC51c3008b3f03Fe483B28B8Db90e19cF076a6d  L2 alias of an Ethereum contract
     └─ acts for  0x7bB41C3008B3f03FE483B28b8DB90e19Cf07595c (ethereum)  Safe 2-of-2
        ├─ signer  0x9855054731540A48b28990B63DcF4f33d8AE46A1 (ethereum)  Safe 3-of-6
        │  └─ …six keys
        └─ signer  0x20AcF55A3DCfe07fC4cecaCFa1628F788EC8A4Dd (ethereum)  Safe 8-of-11
           └─ …eleven keys
  keys        11
  ```

- **The largest authority in the index changed hands on 2026-09-29.** A single key owned 28
  beacons behind 65 indexed proxies. Starting at Base block 51,927,178 it handed 27 of them to a
  2-of-5 Safe it is one owner of, and kept one. Hermes records each change, and bisection over
  archive state pins it to its block (abridged):

  ```
  $ hermes history 0xe68ED13998fd48497EAA3b52e20823605D8d7706
  authority 0xe68E…7706: owner 0x21eb…B5fc → 0x6454…Bf15  at base block 51927178
  ```

- **Two single keys behind 25 indexed proxies each were being reported as unrecognized.** They
  are EOAs with EIP-7702 delegated code, so they answer their delegate's interface. Hermes now
  reads a delegation designator as the key it is.

**Correction, kept on purpose.** Until 2026-09-19 this README said one *EOA* controlled the 20
predeploys. The `ProxyAdmin`'s owner has no code on Base because it is an L1 alias, and Hermes
read "no code" as "one key". It now checks the unaliased address on Ethereum before calling
anything a key.

## Quick start

No API key and no configuration:

```bash
cargo run -p hermes-cli -- scan      # the curated seed against public Base and Ethereum endpoints
cargo run -p hermes-cli -- serve     # http://localhost:8080
```

To index a real share of Base, discover first (about two minutes), then scan (long, resumable):

```bash
cargo run -p hermes-cli -- discover
cargo run -p hermes-cli -- scan
```

Requirements: stable Rust via `rustup`. `foundry`'s `cast` is handy for checking reads by hand.

## Commands

One binary, `hermes`. Commands that only read the database work offline and answer as of the
last scan, which they say.

| Command | Does |
|---|---|
| `scan` | Scan every seeded address that is due, resolve its upgrade authority, and store the graph, all at pinned finalized blocks, in batches of 50 |
| `discover` | Find proxies chain-wide from their ERC-1967 events and seed a sample of them |
| `pin` | Bisect each recorded change to the first block its new value holds at |
| `authority <address>` | How a proxy reaches its root, or what an authority is, drawn as a tree |
| `blast-radius <address>` | Every indexed proxy a compromise of this address reaches: alone, as one of several, or undetermined |
| `key <address>` | The Safes a key signs for on each chain, and what it reaches |
| `owners <address>` | A Safe's owners, one per line |
| `graph <address>` | The same walk as Graphviz DOT or JSON |
| `signers` | Pairs of Safes that share signers, across both chains |
| `history <address>` | Every recorded change to a proxy or authority, oldest first |
| `diff --since 24h` | Every change in a range; **exits 1 when anything on chain changed**, so a cron job can act on it |
| `check deployments.json --policy policy.toml` | A team's contracts against its policy; exits 1 on any violation |
| `serve` | The JSON API and the pages |
| `migrate` | Bring the database to the current schema |
| `record`, `verify` | Developer tools: record a block-pinned fixture; re-check the hand-verified table live |

```
$ hermes blast-radius 0x21ebc2f23a91fD7eB8406CDCE2FD653de280B5fc     # abridged
compromising 0x21eb…B5fc takes 0x21eb…B5fc and 0x21eb…B5fc (ethereum)
controls alone (0):
takes part in, not enough alone (1):
  0x9Caa…b674 · beacon via 0xe68E…7706 · root 0x6454…Bf15 · 2 keys
```

A key is taken on both chains when it is compromised, because one private key signs for its
address everywhere. "Takes part in" is never added to "controls": one signer of a 2-of-5
controls nothing alone.

## Checking your own contracts in CI

`hermes check` resolves each listed contract live, at pinned blocks, and evaluates a policy:

```toml
# policy.toml — every rule is off unless set
[policy]
disallow_eoa_upgrade_admin = true
minimum_keys_required = 3          # nested Safes count through: a 2-of-2 over two 3-of-5s is 6
minimum_timelock = "24h"
allow_cross_chain_authority = true
allow_smart_account_authority = false
allow_unknown = false              # the default: what Hermes cannot read fails
```

```
FAIL  0x9Caa0e7277ce86A4644F2D10b72561080531b674  Safe 0x6454…Bf15 · keys 2 · timelock none
      rule      minimum_keys_required
      expected  keys required >= 3
      observed  keys required = 2
      path      0x9Caa…b674 (beacon) → 0xe68E…7706 → 0x6454…Bf15
```

The check **fails closed**: an unresolved authority, a key count Hermes could not read, or a
contract it could not read at all is a violation unless the policy writes `allow_unknown = true`.
A misspelled rule is an error, not a rule silently left off. As a GitHub Action:

```yaml
- uses: saintparish4/hermes@<commit sha>
  with:
    deployments: deployments.json    # ["0x…"] or [{"name": "Vault", "address": "0x…"}]
    policy: policy.toml
```

The action builds Hermes from its own checkout, so a pinned commit runs exactly that code.
Examples are in [docs/check/](docs/check/).

## How it works

The full account, including every reason an answer can be missing, is the
[methodology page](https://hermes-production-29bf.up.railway.app/methodology). What Hermes cannot
see is in [LIMITATIONS.md](LIMITATIONS.md).

### Where the addresses come from

- **Curated:** 62 hand-picked addresses (`crates/hermes-scan/src/seed.rs`), including USDC, EURC
  and WETH9, which are there *because* Hermes cannot classify them. The only source of labels.
- **Discovered:** `hermes discover` pages `eth_getLogs` for `Upgraded`, `BeaconUpgraded` and
  `AdminChanged`, so proxies are found by the standard their slots are read by. It reads
  1,000-block windows at fixed positions every 500,000 blocks (the public endpoint caps a log
  response by size) and admits at most three addresses per implementation, beacon or admin.
  Every sighting is kept, admitted or not, so a beacon can say how many proxies discovery saw
  using it.

**The index is a sample.** In one recent window, 248 of 374 upgrades pointed at one
implementation; admitted unfiltered, the index would be a few contracts copied hundreds of
times. `/coverage` reports the cap and how many proxies and families discovery has sighted.

### Where upgrade rights live

| Proxy | Walk starts at | Most confidence |
|---|---|---|
| Transparent, admin-only | the ERC-1967 admin | High |
| Beacon | the beacon, whose controller upgrades every proxy using it | High |
| UUPS | the proxy itself, only after its implementation answers `proxiableUUID()` with the ERC-1967 slot | Medium |

### What each address is

| Answers | Kind | Keys counted as |
|---|---|---|
| no code on Base or at its unaliased address on Ethereum | key | 1 |
| an EIP-7702 delegation designator | key | 1 |
| no code on Base, code at `address − 0x1111…1111` on Ethereum | L1 alias | continues on Ethereum |
| `getOwners()` and `getThreshold()` | Safe | sum of the m cheapest owners |
| `getMinDelay()` | timelock | continues through `owner()`, else unknown (`roles_unread`) |
| `owner()` | owned contract | continues to the owner |
| OpenZeppelin `AccessControl` | role-gated | cheapest holder of the upgrade role or its admin role, when listed |
| `entryPoint()` | ERC-4337 smart account | cheapest signer when MultiOwnable, else unknown |
| `0x…dEaD`, `0xff…ff`, a precompile | sentinel | none: no one holds the key |

Walks stop at four links and on cycles. `null` is never zero: a covered proxy with no root says
why (`unrecognized_interface`, `uups_unconfirmed`, `no_upgrade_path`, `rpc_undetermined`), and so
does a root with no key count (`truncated`, `cycle`, `owners_unknown`, `roles_unread`,
`account_keys_unread`, `no_known_key`). A node that will not answer is "undetermined", a third
outcome that never becomes "no". Confidence only falls along a walk.

### The graph and its history

Every scan reads both chains at their **finalized** blocks, which cannot be reorged away, and
writes, in one transaction per batch:

- each proxy row, stamped with its block;
- each probed address with the whole probe it answered, which is the resolver's exact input, so
  an offline walk reproduces what the scan published (a test holds every verified fixture to it);
- each edge (admin, beacon, owner, Safe signer, account signer, role holder, alias), with the
  blocks it was first and last seen and the block it closed. An edge seen again after closing
  opens a new row, so the edge table is its own history;
- each change against what was stored, bracketed by the two observations' blocks.

An address the node would not read changes nothing: its edges stay open and nothing is reported
about it. A row the never-overwrite-live-code guard refuses writes no edges either. A change made
under a different model version (Hermes reading the chain differently, not the chain moving) is
recorded as a **reinterpretation** and hidden unless asked for. `hermes pin` then bisects each
single-read change (an owner, threshold, delay, signer or slot) to the first block its new value
holds at; a root or key count keeps its bracketing blocks. History starts when Hermes first saw
an address.

### API

The original routes serve the main page and are unchanged: `/proxies`, `/proxies/{address}`,
`/authorities`, `/authorities/{address}`, `/coverage`, and `/healthz`, which never touches SQLite.

`/v1` answers from the graph, and every response carries a `scope` (the index is a sample, the
graph is as of one observation and model version, first seen means first seen by Hermes):

- `GET /v1/proxies/{address}`: the row, the upgrade path as a tree, and the root replayed over the stored graph
- `GET /v1/nodes/{chain}/{address}`: any address a walk reached, what it controls directly, the Safes it signs for, closed edges, and its blast radius
- `GET /v1/authorities`, `GET /v1/authorities/{chain}/{address}`: roots ranked by indexed proxies, and one root with everything under it
- `GET /v1/keys/{address}`: a key on both chains
- `GET /v1/signers?min_shared=2`: Safes that share signers
- `GET /v1/changes?since=24h&address=…` and `GET /v1/changes.atom`: changes, newest first, as JSON or a feed
- `GET /v1/coverage`

The pages are static HTML over the same API: `/`, `/authority.html?address=…` for any proxy,
admin, Safe or key, and `/methodology`.

### Repository layout

```
crates/
  hermes-core/   pure: slots, classification, aliasing, the resolver, graph and change rules,
                 blast radius, policy; plus the SQLite store (the only I/O here)
  hermes-scan/   all network I/O: the RPC seam (live, recording, replay), slot and authority
                 probes, the pipeline, discovery, bisection, the verified table
  hermes-api/    axum routes and the static fallback
  hermes-cli/    the `hermes` binary
static/          the pages that are served; index.html is the permanent fallback
tests/           verified.json (hand-checked expectations) and block-pinned fixtures
docs/            progress log, verification table, prior-art search, check examples
action.yml       the GitHub Action
home/            a Next.js landing page, not part of the deploy
```

Stack: Rust with `alloy`, `tokio`, `axum` and `sqlx` on SQLite, one binary, no external services.

## Testing

`cargo test --workspace` runs 239 tests, all offline: pure functions, `sqlite::memory:`, temp
files, or recorded chain answers replayed from `tests/fixtures/`. CI runs `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings` (with the nursery `cognitive_complexity`
lint denied), the tests, and a locked build, on Ubuntu and macOS.

- **The hand-verified table.** 22 addresses chosen by shape ([docs/verification.md](docs/verification.md)),
  each checked by reading the chain *without* Hermes and replayed through the production pipeline
  from a fixture recorded at the same blocks. It covers the L1-aliased predeploys, a self-owned
  `ProxyAdmin`, Safes on Base, a genuine key, an honestly unknown admin, UUPS paths confirmed and
  denied, a beacon, USDC and WETH9, both sides of the 2026-09-29 transfer, EIP-7702 keys, a
  MultiOwnable account, an account whose signers are unread, an AccessControl contract whose
  upgrade role is empty but whose admin role is held, a renounced owner, and a 72-hour timelock.
  Expectations are never copied from Hermes's own output: a table that learned from the program
  could not catch it.
- **The graph and its history.** The write rules (an unread node closes nothing, a refused row
  writes nothing, owner sets move as a unit, first sight is not a change, a model change is a
  reinterpretation), the 2026-09-29 transfer replayed as two observations, bisection pinning it
  to its block, and an offline walk of the stored graph reproducing every published root.
- **The resolver and blast radius.** Cheapest-m-of-n arithmetic, cycles, the depth cap,
  confidence that only falls, lying thresholds, aliasing to the depth cap, and `falls`, the dual
  of the key count: eleven keys in the right places take the predeploys, eleven in the wrong
  places do not.
- **The RPC boundary.** Undetermined never becomes "no", empty reads are confirmed, replay is
  keyed rather than ordered, and a read a fixture never saw fails loudly.
- **Policy.** Each rule passing and failing, unknown failing closed, and a misspelled rule refused.

Every fix was checked by putting the bug back and watching its test fail. `hermes verify` re-checks
the table against the live chain daily in a separate workflow that is allowed to fail: red there
means Hermes regressed *or the chain moved*.

## Environment variables

All optional; every one has a working default.

| Variable | Default | Used by |
|---|---|---|
| `HERMES_DB` | `sqlite://hermes.db` | every command |
| `HERMES_RPC_URL` | `https://mainnet.base.org` | `scan`, `discover`, `pin`, `check` |
| `HERMES_L1_RPC_URL` | `https://eth.drpc.org` | the same, to read Ethereum behind aliased authorities |
| `HERMES_CONCURRENCY` | `3` | `scan` slot reads |
| `HERMES_CALL_INTERVAL_MS` | `1000` | gap between calls to Base when resolving and bisecting |
| `HERMES_STATIC_DIR` | `static` | `serve` |
| `PORT` | `8080` | `serve` |
| `HERMES_SCAN_INTERVAL` | `86400` | the container's refresh loop |

The public Base endpoint allows about ten back-to-back `eth_call`s, then answers HTTP 429 until
it has seen roughly seven seconds of quiet, and under concurrency it returns empty code and zero
storage for contracts that have both. Three concurrent slot readers and one resolution call per
second are what it tolerates. Set the interval to `0` against a keyed endpoint.

## Deployment

One Rust binary serves the API and the pages, with SQLite as a file on a volume.

The container scans before opening its port only when the database is empty, and then only a
40-second classification of the curated seed, so the first boot fits a 60-second health check.
Otherwise it runs `hermes migrate` once and opens the port. Either way a background loop runs
`discover`, `scan` and `pin` straight away and every `HERMES_SCAN_INTERVAL` after that. A
redeploy that kills a scan loses at most one batch.

### Railway

```bash
railway init
railway volume add --mount-path /data     # required
railway up
```

Without the volume, every redeploy starts from an empty database. `HERMES_DB` already points at
`sqlite:///data/hermes.db`. If the image's non-root user cannot write the volume, set
`RAILWAY_RUN_UID=0`. The refresh loop lives in the container because Railway allows one volume
per service. `/healthz` never touches SQLite, so a long scan cannot fail the health check.

### Anywhere else

```bash
docker build -t hermes .
docker run -p 8080:8080 -v hermes-data:/data hermes
```

## Contributing

Open a PR against `master`, after the same checks CI runs:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked
```

## License

[MIT](LICENSE) © 2026 Sharif Parish / Bluesky Labs
