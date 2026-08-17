# ava: Ultrafast JavaScript runtime + toolkit, shipped as a single native binary

Ava is a Node.js-compatible package manager (with a runtime, bundler, and test runner planned for later phases) intended as a dramatically faster replacement for npm, yarn, and pnpm. It's being built as a standalone tool that works in existing Node.js projects — if a project has a `package.json`, `ava install` should work on it.

| Component        | Status      |
|------------------|-------------|
| Package Manager  | In progress |
| Runtime          | Deferred    |
| Bundler          | Deferred    |
| Test Runner      | Deferred    |

Only the package manager (`crates/package-manager`) is active. Do not add `runtime`, `bundler`, or `test-runner` to the workspace until those phases start.

## Requirements

- Rust, `stable` channel. The repo pins the toolchain via `rust-toolchain.toml`, so you do not pick a version by hand — `rustup` will install the correct one automatically the first time you run `cargo` in this repo.
- The `rustfmt` and `clippy` components (also pinned in `rust-toolchain.toml`).
- No Node.js is required to build or work on Ava itself.

## Installation

There is no released binary yet — building from source is currently the only way to get `ava`.

```bash
git clone https://github.com/bluesky/ava.git
cd ava
cargo build -p ava-package-manager
```

Once end-user releases exist, installation will be via a standalone installer script (see [Deployment](#deployment)) — `cargo install ava` is not, and will not be, the primary install path for end users. It remains fine for contributors building from source.

## Development

Workspace layout:

```
crates/
  package-manager/   # active — this is the `ava` binary
  runtime/            # deferred, not a workspace member
  bundler/            # deferred
  test-runner/        # deferred
```

Build the package manager:

```bash
cargo build -p ava-package-manager
```

Run the CLI from source (args after `--` belong to Ava, not Cargo):

```bash
cargo run -p ava-package-manager -- --version
cargo run -p ava-package-manager -- add react
```

The binary name is `ava`. Until a release exists, `cargo run` is how you exercise its commands:

- `ava add` — add packages to your project
- `ava remove` — remove dependencies from your project
- `ava update` — update dependencies to the newest versions their ranges allow
- `ava duplicate` — remove duplicate versions of packages from `ava.lock`
- `ava snip` — remove packages not in `ava.lock` from `node_modules`
- `avax` — auto-install and run a package from npm (Ava's equivalent of `npx`/`yarn dlx`)
- `ava publish` — pack and publish a package to the configured registry
- `ava outdated` — list dependencies with newer versions available
- `ava why` — explain why a package is installed
- `ava audit` — check installed packages for known vulnerabilities
- `ava info` — show package metadata from the registry

A typical change loop:

```
edit crates/package-manager
        ↓
cargo fmt
        ↓
cargo clippy -p ava-package-manager
        ↓
cargo test -p ava-package-manager
        ↓
cargo run -p ava-package-manager -- <command>
```

## Testing

Compiling is not sufficient — Ava is a resolve → lock → download → verify → cache → `node_modules` pipeline, and a change can build while still producing the wrong lockfile or tree.

Run the same checks locally that CI runs:

```bash
cargo fmt --check
cargo clippy -p ava-package-manager -- -D warnings
cargo test -p ava-package-manager
cargo build -p ava-package-manager
```

Tests are organized in three layers (see `.cursor/rules/testing.mdc` for the full policy):

- **Unit** — isolated resolver/semver/lockfile/integrity logic, no network or real registry. Determinism (same inputs → same `ava.lock`, on every OS) is treated as a unit-test concern.
- **Integration** — real boundaries with fakes only at the edge (fixture registry, temp dir): resolver ↔ registry, downloader ↔ cache, installer ↔ filesystem, lockfile ↔ disk. Cross-platform path/case/symlink behavior is covered here.
- **E2E** — the CLI run the way a user would (`ava add`, `ava install`, `ava remove`, `ava update`), asserting on `ava.lock` and the resulting `node_modules` tree, against a local fixture registry. Live-npm E2E tests, if they ever exist, run in a separate nightly lane and never block a PR.

What "not broken" means in practice:

- a version range resolves to a version that satisfies it
- the same inputs produce a stable `ava.lock`
- tarball integrity matches (content-addressed cache)
- the install layout on disk matches what the lockfile says

CI runs the same commands above on every PR — it's the backstop, not the first place you should learn something broke.

## Environment Variables

None currently.

## Architecture

Ava is a dependency resolution + artifact distribution system. The core pipeline:

```
package.json
      ↓
registry metadata
      ↓
semver resolution
      ↓
dependency graph
      ↓
lockfile
      ↓
download
      ↓
integrity verification
      ↓
global cache
      ↓
node_modules
```

The cache is content-addressed and shared across projects, so identical package artifacts are only downloaded once:

```
~/.ava/
├── bin/
│   └── ava
├── cache/
│   └── sha256/
│       ├── ab/...
│       └── cd/...
└── config/
```

For example, `ava add react` can check "I already have this exact package artifact" and skip the download if it's already in the cache.

## Deployment

Not yet implemented — there are no published releases. The planned shipping model:

- Ava compiles to a single, self-contained native CLI binary per target (the package manager compiled directly in): `ava-linux-x64`, `ava-linux-arm64`, `ava-macos-x64`, `ava-macos-arm64`, `ava-windows-x64.exe`.
- CI builds all release targets and publishes them, with checksums (`SHA256SUMS`), as GitHub Releases.
- End users install via a platform-detecting installer script (`curl -fsSL https://ava.dev/install.sh | bash` — domain not finalized) that downloads the correct binary and adds it to `PATH`. Users should never need Rust installed.
- Package-manager integrations (Homebrew, Scoop, WinGet, npm, Docker) may be added later, but GitHub Release binaries remain the source of truth — those integrations would just wrap them.

## Contributing

## LICENSE

MIT © 2026 Sharif Parish / Bluesky Labs. See [LICENSE](./LICENSE).
