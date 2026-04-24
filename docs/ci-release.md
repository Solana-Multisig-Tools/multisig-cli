# CI And Release Enforcement

This repository keeps the release path boring: locked dependencies, pinned workflow actions, explicit supply-chain checks, and checksums for produced artifacts.

## Pull Requests And Main Branch Pushes

`.github/workflows/ci.yml` runs on every pull request and on pushes to `main` or `master`.

It enforces:

- SHA-pinned `actions/checkout`
- read-only GitHub token permissions
- the pinned Rust toolchain from `rust-toolchain.toml`
- `cargo fetch --locked`
- pinned `cargo-audit`
- pinned `cargo-deny`
- `cargo audit --deny warnings`
- `cargo deny check advisories bans licenses sources`
- `cargo fmt --all --check`
- `cargo test --locked`
- `cargo test --locked --all-features`
- `cargo test --locked --no-default-features`
- `cargo test --locked --no-default-features --features ledger`
- `cargo test --locked --no-default-features --features tui`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo package --locked`

The CI path intentionally does not require Solana localnet or the Squads SDK. Hand-written layouts and offline-message import are covered by focused local tests, and devnet behavior is covered by the explicit smoke script.

The experimental TUI is compiled and tested in CI so it stays buildable, but it is not included in release binaries.

## Tagged Releases

`.github/workflows/release.yml` runs on strict `vMAJOR.MINOR.PATCH` tags and manual workflow dispatch. Suffixes such as `-rc.1` are intentionally rejected by the release path.

The validation job enforces:

- the release tag format
- the tag points at the checked-out commit
- SHA-pinned `actions/checkout`
- the pinned Rust toolchain from `rust-toolchain.toml`
- locked dependency fetch
- pinned `cargo-audit`
- pinned `cargo-deny`
- RustSec advisory checks
- repository dependency/license/source policy
- all-feature tests
- all-target clippy
- `cargo package --locked`
- crate SHA-256 checksum generation

The build jobs produce release binaries for:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Release binaries are built with:

```sh
cargo build --release --locked --no-default-features --features ledger
```

That intentionally excludes the experimental TUI feature from shipped artifacts.

Each binary gets its own SHA-256 checksum. Non-dry-run releases are created as draft GitHub releases from existing tags.

The release workflow defaults to read-only repository permissions and grants `contents: write` only to release jobs that create or upload draft release artifacts.

## Local Release Dry Run

Run this before cutting a tag:

```sh
scripts/release-dry-run.sh
```

Run this after creating a local tag and before pushing it:

```sh
scripts/release-dry-run.sh --tag v0.1.0
```

The tagged dry run requires a clean worktree and requires the tag to point at `HEAD`. It mirrors the release validation path locally and writes host-platform artifacts plus `SHA256SUMS` under:

```text
target/release-dry-run/
```

## Devnet Smoke

Run this separately when validating real RPC behavior:

```sh
scripts/devnet-smoke.sh
```

The smoke sends real devnet transactions and intentionally creates devnet artifacts. It is not part of PR CI because it depends on a funded devnet keypair, live devnet availability, Solana CLI tooling, and optional devnet USDC balance.
