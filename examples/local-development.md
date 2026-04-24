# Local Installation And Development

This guide covers installing and running `msig` on a local machine for development, devnet testing, and mainnet preflight review.

## Example Values

| Value | Example |
|---|---|
| Devnet RPC | `https://api.devnet.solana.com` |
| Mainnet RPC | `https://api.mainnet-beta.solana.com` |
| Keypair | `~/.config/solana/id.json` |
| Multisig | `SMPLDaGKqbPfi8NhZMNGH2fRYU3WbNRZVj3xnTjEjXc` |
| Vault index | `0` |

## Prerequisites

- [Rust](https://rustup.rs/) stable.
- The repository pins its Rust version in `rust-toolchain.toml`; run commands from the repo so `rustup` uses that toolchain.
- A Solana keypair JSON file for keypair signing, or a Ledger with the Solana app installed.
- Solana CLI if you want to run `scripts/devnet-smoke.sh`; the smoke uses `solana` to fund the vault and `spl-token` for the optional devnet USDC segment.
- `jq` and `python3` if you want to run `scripts/devnet-smoke.sh`.
- `cargo-audit` and `cargo-deny` if you want to run release-equivalent checks manually. The release dry-run script installs pinned versions automatically.

## Option A: Install From This Checkout

### 1. Clone

```sh
git clone https://github.com/Solana-Multisig-Tools/v4-cli.git
cd v4-cli
```

### 2. Install

```sh
cargo install --path . --locked
```

This installs the CLI:

```sh
msig --version
```

Use `msig` for day-to-day commands.

### 3. Configure

```sh
msig config set default.cluster https://api.devnet.solana.com
msig config set default.keypair ~/.config/solana/id.json
msig config set default.commitment confirmed
msig config set default.vault_index 0
msig config set default.multisig <MULTISIG>
msig config show
```

For mainnet Ledger signing:

```sh
msig config set default.cluster https://api.mainnet-beta.solana.com
msig config set default.multisig <MULTISIG>
msig --ledger config doctor
```

### 4. Try Read-Only Commands

```sh
msig multisig info
msig vault balance
msig proposal list
msig proposal pending
msig proposal executable
msig proposal needs-me
```

### 5. Review A Proposal

```sh
msig proposal show <INDEX|PROPOSAL_ADDRESS>
msig proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig proposal simulate <INDEX|PROPOSAL_ADDRESS>
msig --output json proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

## Option B: Run Without Installing

Use this when you are editing the CLI and want to avoid reinstalling after every change:

```sh
cargo run --locked -- --help
cargo run --locked -- config doctor
cargo run --locked -- --cluster devnet --keypair ~/.config/solana/id.json proposal list --multisig <MULTISIG>
```

For a faster edit loop:

```sh
cargo build --locked
target/debug/msig --help
target/debug/msig config doctor
```

The TUI is not shipped in release binaries yet. For local TUI development only:

```sh
MSIG_ENABLE_EXPERIMENTAL_TUI=1 cargo run --locked --no-default-features --features tui -- tui
```

## Option C: Devnet Smoke Test

The smoke script sends real devnet transactions. Use a funded devnet keypair.

It intentionally creates devnet artifacts: a multisig, proposals, vault funding, and, when devnet USDC is available, temporary normal SPL token accounts. These accounts spend devnet rent and are not cleaned up automatically.

### 1. Configure Inputs

```sh
export MSIG_DEVNET_KEYPAIR=~/.config/solana/id.json
export MSIG_DEVNET_RPC=https://api.devnet.solana.com
```

If `solana-keygen` is not available, set the keypair public key explicitly:

```sh
export MSIG_DEVNET_MEMBER=<KEYPAIR_PUBKEY>
```

### 2. Run With Human Pacing

```sh
scripts/devnet-smoke.sh
```

The script prints each user-facing command before running it, then waits briefly between steps.

It also funds the new Squads vault with a small SOL amount so the smoke can execute a system-program transfer from the vault. If the keypair has devnet USDC (`4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`) and `spl-token` is installed, the script creates temporary normal token accounts, sends a tiny amount into the vault-owned token account, and executes an SPL Token transfer proposal. If that token setup is not available, it prints `USDC token smoke: not available` and continues.

### 3. Run Without Delay

```sh
scripts/devnet-smoke.sh --no-delay
```

or:

```sh
MSIG_DEVNET_NO_DELAY=1 scripts/devnet-smoke.sh
```

## Option D: Release Dry Run

Run the release validation path locally:

```sh
scripts/release-dry-run.sh
```

With a tag, the script requires the tag to point at `HEAD` and requires a clean worktree:

```sh
scripts/release-dry-run.sh --tag v0.1.0
```

Artifacts and SHA-256 checksums are written to:

```text
target/release-dry-run/
```

Release artifacts are built with the `ledger` feature only. The experimental TUI feature is compiled and tested in CI, but is not included in release binaries.

## Testing

```sh
cargo test --locked
cargo test --locked --no-default-features
cargo test --locked --all-features
cargo test --locked --no-default-features --features ledger
cargo test --locked --no-default-features --features tui
```

## Linting And Supply Chain Checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo audit --deny warnings
cargo deny check advisories bans licenses sources
```

## Mainnet Preflight Checklist

```sh
msig --cluster mainnet --multisig <MULTISIG> --ledger config doctor
msig --multisig <MULTISIG> multisig info
msig --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig --multisig <MULTISIG> proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

Before approving on Ledger, compare the CLI `Message Hash` with the hash displayed by Ledger blind signing.
