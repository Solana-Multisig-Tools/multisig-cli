# CLI Reference

This page keeps the detailed command and output reference out of the README.

## Output Contract

- JSON output is always written to stdout and is the supported machine-readable interface.
- Tables and read-only human output are written to stdout.
- Signing previews, confirmation prompts, Ledger review prompts, simulation logs from failed signing preflights, and confirmation progress are written to stderr.
- Errors are written to stderr. With `--output json`, error reports are JSON on stderr.

## Configuration

Configuration is resolved in this order:

1. built-in defaults
2. user config at `~/.config/msig/config.toml`
3. active user profile
4. current-directory `.msig.toml`
5. `MSIG_*` environment variables
6. CLI flags

## Global Options

| Option | Description |
|---|---|
| `--cluster <URL|MONIKER>` | RPC endpoint. Supports `mainnet`, `devnet`, `testnet`, `localhost`, or a full URL. |
| `--keypair <FILE>` | Path to a Solana keypair JSON file. |
| `--ledger [N]` | Use a Ledger hardware wallet, optionally with an account index or Ledger URI. |
| `--multisig <ADDR>` | Squads v4 multisig address. |
| `--vault-index <N>` | Vault index, default `0`. |
| `--output <json|table>` | Output mode. |
| `--commitment <LEVEL>` | `processed`, `confirmed`, or `finalized`. |
| `--priority-fee <MICRO>` | Compute unit price in microlamports. |
| `--dry-run` | Simulate write commands without sending. |
| `-y`, `--yes` | Skip confirmation prompts. |
| `--no-color` | Disable ANSI color. |

## Environment Variables

| Variable | Description |
|---|---|
| `MSIG_CLUSTER` | Overrides configured cluster. |
| `MSIG_KEYPAIR` | Overrides configured keypair. |
| `MSIG_MULTISIG` | Overrides configured multisig. |
| `MSIG_PRIORITY_FEE` | Overrides configured priority fee. |
| `MSIG_PROGRAM_ID` | Overrides the Squads program ID. |
| `MSIG_TRUST_PROJECT_CONFIG` | Set to `1` only after reviewing current-directory `.msig.toml`. |

## Commands

| Command | Purpose |
|---|---|
| `msig multisig create` | Create a Squads v4 multisig. |
| `msig multisig info` | Show multisig config and members. |
| `msig multisig set-threshold` | Propose a threshold change. |
| `msig multisig set-timelock` | Propose a time-lock change. |
| `msig multisig add-spending-limit` | Propose a vault spending limit. |
| `msig multisig remove-spending-limit` | Propose removing a spending limit. |
| `msig vault balance` | Show SOL and token balances for a vault. |
| `msig member list` | List members and permissions. |
| `msig member add` | Propose adding a member. |
| `msig member remove` | Propose removing a member. |
| `msig proposal list` | List recent proposals. |
| `msig proposal pending` | List active, approved, or executing proposals. |
| `msig proposal executable` | List approved proposals ready to execute. |
| `msig proposal needs-me` | List active proposals the signer has not voted on. |
| `msig proposal show` | Show proposal details and decoded instructions. |
| `msig proposal simulate` | Simulate execution and show lamport/token diffs. |
| `msig proposal approve` | Vote to approve. |
| `msig proposal reject` | Vote to reject. |
| `msig proposal cancel` | Cancel a proposal. |
| `msig proposal execute` | Execute an approved proposal. |
| `msig transfer sol` | Create a SOL transfer proposal. |
| `msig transfer spl` | Create an SPL token transfer proposal. |
| `msig template inspect` | Show template inputs and SHA-256. |
| `msig template validate` | Compile and preview a template without proposing. |
| `msig template run` | Create a proposal from a TOML template. |
| `msig tx create` | Create a one-off custom vault transaction proposal. |
| `msig tx show` | Show transaction details. |
| `msig tx list` | List recent transactions. |
| `msig tx export` | Export a signable `.sqds` transaction. |
| `msig tx status` | Verify and inspect a `.sqds` file. |
| `msig tx combine` | Merge signatures from matching `.sqds` files. |
| `msig tx import` | Sign or submit an offline transaction. |
| `msig program upgrade` | Create a program upgrade proposal. |
| `msig rent set-collector` | Propose a rent collector update. |
| `msig rent reclaim` | Reclaim rent from closed accounts. |
| `msig config doctor` | Check local trust and mainnet-readiness settings. |

## Program

- Program ID: `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`
- Network: Solana mainnet, devnet, testnet, localnet, or a custom trusted RPC
- Framework: Anchor-compatible account and instruction layouts parsed locally

## Local Squads Layouts

This CLI intentionally does not import the Squads SDK for core account, instruction, and offline-message parsing paths. The local layouts keep the binary dependency tree small, make offline import independent from advisory metadata, and let CI enforce exactly which transitive crates enter the release.

The duplicated layouts are verified by focused unit tests and should be updated deliberately if the Squads v4 program layout changes.
