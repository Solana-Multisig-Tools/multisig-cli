# msig

Minimal Squads MultisigV4 CLI for operators who need inspectable signing flows, hardware-wallet review, offline signing, proposal simulation, and fixed workflow templates without the Squads SDK dependency tree.

## Architecture

```text
Operator shell -> msig CLI -> Solana RPC -> Squads MultisigV4 Program
                         \-> Ledger or keypair signer
                         \-> .sqds offline signing files
                         \-> TOML templates for repeatable vault transactions
```

The CLI is a single Rust binary with hand-written Squads v4 account and instruction parsing. It resolves configuration locally, fetches only the on-chain accounts needed for each command, previews the exact message hash before signing, and keeps templates as static data files that cannot execute code or call RPC.

Builds use the pinned Rust toolchain in `rust-toolchain.toml` so CI, release, and local dry-runs do not drift with the moving `stable` channel.

## Project Structure

```text
msig/
├── README.md
├── docs/
│   ├── ci-release.md            # PR, release, and local dry-run enforcement
│   ├── mainnet-operator-checklist.md
│   ├── templates.md              # Template format and examples
│   └── trust-policy.md           # Signing, import, RPC, and release trust boundaries
├── examples/
│   └── local-development.md      # Local installation and development guide
├── scripts/
│   ├── devnet-smoke.sh           # End-to-end devnet command smoke test
│   └── release-dry-run.sh        # Local release validation and checksum build
├── src/
│   ├── application/              # Command workflows and transaction builders
│   ├── cli/                      # Argument parsing and command dispatch
│   ├── domain/                   # Multisig/proposal/transaction domain types
│   ├── infra/                    # RPC, config, PDA, account parsing, signer backends
│   ├── output/                   # JSON/table output helpers
│   └── tui/                      # Experimental dashboard, not shipped in release binaries
├── Cargo.toml
├── Cargo.lock
└── deny.toml                     # Supply-chain, license, source, and dependency policy
```

## Quick Start

### Local Install

```sh
git clone https://github.com/Solana-Multisig-Tools/v4-cli.git
cd v4-cli
cargo install --path . --locked
msig --version
msig config doctor
```

The installed command is `msig`.

### Configure

```sh
msig config set default.cluster https://api.mainnet-beta.solana.com
msig config set default.keypair ~/.config/solana/id.json
msig config set default.commitment confirmed
msig config set default.vault_index 0
msig config set default.multisig <MULTISIG>
msig config show
```

For Ledger signing, skip `default.keypair` and pass `--ledger` on signing commands.

## Output Contract

- JSON output is always written to stdout and is the supported machine-readable interface.
- Tables and read-only human output are written to stdout.
- Signing previews, confirmation prompts, Ledger review prompts, simulation logs from failed signing preflights, and confirmation progress are written to stderr.
- Errors are written to stderr. With `--output json`, error reports are JSON on stderr.

### Inspect And Review

```sh
msig --multisig <MULTISIG> multisig info
msig --multisig <MULTISIG> vault balance
msig --multisig <MULTISIG> proposal list
msig --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS>
msig --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig --multisig <MULTISIG> proposal simulate <INDEX|PROPOSAL_ADDRESS>
msig --multisig <MULTISIG> --output json proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

### Create Proposals

```sh
msig --cluster devnet --keypair ~/.config/solana/id.json multisig create \
  --threshold 2 \
  --members <MEMBER_A>,<MEMBER_B>,<MEMBER_C>

msig --multisig <MULTISIG> transfer sol 1.5 <RECIPIENT>

msig --multisig <MULTISIG> multisig add-spending-limit \
  --vault-index 0 \
  --mint native \
  --amount 1000000000 \
  --period day \
  --members <MEMBER_A>,<MEMBER_B>
```

### Templates

Templates are static TOML files for repeatable Squads vault transaction workflows. They are meant for the middle ground between built-in commands such as `transfer sol` and fully raw transaction construction: the template author fixes the program, accounts, account flags, and instruction data layout, while the operator supplies only the inputs that should vary for that workflow.

This lets a team or the community publish reusable workflows without asking this CLI to grow a custom command for every protocol action. A template can describe one instruction, multiple instructions, or a repeated instruction layout over a list of pubkeys.

The template security model is intentionally narrow:

- templates are explicit files passed on the command line; they are never auto-loaded
- templates cannot execute code, call RPC, read environment variables, or include other files
- account aliases resolve from constants, operator inputs, or the active Squads context
- only the Squads vault context account may be marked as a signer
- `inspect` prints the declared inputs, instruction count, and template SHA-256
- `validate` compiles the exact instructions with real inputs without creating a proposal

The normal review flow is:

```sh
msig template inspect workflow.toml
msig template validate workflow.toml --input recipient=<PUBKEY>
msig template run workflow.toml --input recipient=<PUBKEY>
```

Templates have four main parts:

```toml
id = "system.transfer"
version = "1"
description = "Transfer SOL from the Squads vault"

[inputs.recipient]
type = "pubkey"

[inputs.lamports]
type = "u64"

[accounts.system_program]
const = "11111111111111111111111111111111"

[accounts.vault]
context = "vault"

[accounts.destination]
input = "recipient"

[[instructions]]
program = "system_program"
accounts = [
  { pubkey = "vault", writable = true, signer = true },
  { pubkey = "destination", writable = true },
]
data = [
  { const_hex = "02000000" },
  { input = "lamports", encoding = "u64_le" },
]
```

Inputs declare what the operator must provide. Supported types are `pubkey`, `pubkey[]`, `bytes`, `string`, `u8`, `u16`, `u32`, `u64`, `i64`, and `bool`. Inputs can be passed as `--input key=value` or as direct flags such as `--recipient <PUBKEY> --lamports 1000000`.

Accounts are named aliases. Each alias must come from exactly one source:

- `const = "<PUBKEY>"` for fixed program IDs or fixed accounts
- `input = "<INPUT_NAME>"` for operator-provided pubkeys
- `context = "vault"`, `context = "multisig"`, or `context = "program_id"` for active Squads context accounts

Instruction data is assembled from fixed bytes and typed input bytes. Integer inputs default to little-endian when their native type is used, and explicit encodings such as `u64_le`, `u32_be`, `bool_u8`, `pubkey`, `utf8`, and `bytes` are available. `bytes` inputs accept hex by default, `base64:<DATA>`, or `utf8:<TEXT>`.

For batch workflows, `for_each` repeats an instruction over a `pubkey[]` input:

```toml
[inputs.recipients]
type = "pubkey[]"

[[instructions]]
program = "some_program"
for_each = "recipients"
accounts = [
  { pubkey = "$item", writable = true },
]
data = [
  { const_hex = "07000000" },
]
```

After `template run` creates the proposal, operators should still use the normal proposal review path:

```sh
msig proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

See [docs/templates.md](docs/templates.md) for the full template reference, raw data examples, and simulation review guidance.

### Raw Transactions

For one-off workflows where writing a template is not worth it, `tx create` can create an arbitrary vault transaction directly from program/account/data flags:

```sh
msig tx create \
  --vault-index 0 \
  --program <PROGRAM_ID> \
  --account vault:writable:signer \
  --account <ACCOUNT>:writable \
  --data <HEX|base64:DATA|utf8:TEXT>
```

Use templates when a workflow will be reused or shared. Use `tx create` when the instruction is genuinely one-off. Use `--vault-message <HEX|base64:DATA|JSON_BYTES>` only for pre-compiled vault transaction messages where the bytes already came from a trusted builder.

## Configuration

Configuration is resolved in this order:

1. built-in defaults
2. user config at `~/.config/msig/config.toml`
3. active user profile
4. current-directory `.msig.toml`
5. `MSIG_*` environment variables
6. CLI flags

### Global Options

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

### Environment Variables

| Variable | Description |
|---|---|
| `MSIG_CLUSTER` | Overrides configured cluster. |
| `MSIG_KEYPAIR` | Overrides configured keypair. |
| `MSIG_MULTISIG` | Overrides configured multisig. |
| `MSIG_PRIORITY_FEE` | Overrides configured priority fee. |
| `MSIG_PROGRAM_ID` | Overrides the Squads program ID. |
| `MSIG_TRUST_PROJECT_CONFIG` | Set to `1` only after reviewing current-directory `.msig.toml`. |

### Mainnet Preflight

Run this before mainnet signing to review the resolved local trust surface without making RPC calls:

```sh
msig --cluster mainnet --multisig <MULTISIG> --ledger config doctor
msig --output json config preflight
```

For the full signing runbook, see [docs/mainnet-operator-checklist.md](docs/mainnet-operator-checklist.md).

## Command Glossary

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

## TUI Status

The TUI is intentionally not part of the first mainnet release. Release binaries are built with the `ledger` feature only, `msig tui` is hidden from help, and direct calls return an error unless the binary was built with `--features tui` and `MSIG_ENABLE_EXPERIMENTAL_TUI=1` is set.

For this release, use the CLI commands and JSON output as the supported operator interface.

## Trust Defaults

- `.sqds` import independently decodes the Solana message instead of trusting exported advisory metadata.
- `.msig.toml` is only auto-loaded from the current directory, and sensitive fields such as custom RPC URLs, keypair paths, program IDs, localhost, and priority fees require explicit trust.
- Templates are explicit TOML files. They cannot execute code, call RPC, read environment variables, or mark non-vault accounts as signers.
- Address lookup table backed vault transactions are resolved from on-chain lookup table accounts during show/simulate/execute.
- CI and release run RustSec advisory checks plus the repository dependency policy in `deny.toml`.

See [docs/trust-policy.md](docs/trust-policy.md), [docs/templates.md](docs/templates.md), [docs/ci-release.md](docs/ci-release.md), [docs/mainnet-operator-checklist.md](docs/mainnet-operator-checklist.md), and [examples/local-development.md](examples/local-development.md).

## Ledger Review

Ledger Solana blind signing displays a Base58 SHA-256 hash of the raw Solana message bytes. Before asking the Ledger to sign, `msig` prints the same `Message Hash` for the exact message being signed. Compare the CLI hash with the Ledger hash before approving.

## Development

```sh
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo deny check advisories bans licenses sources
```

## Devnet Smoke

The smoke script sends real devnet transactions. Use a funded devnet keypair.

It intentionally creates devnet artifacts: a multisig, proposals, vault funding, and, when devnet USDC is available, temporary normal SPL token accounts. These accounts spend devnet rent and are not cleaned up automatically.

```sh
MSIG_DEVNET_KEYPAIR=/path/to/devnet-keypair.json \
MSIG_DEVNET_MEMBER=<KEYPAIR_PUBKEY> \
scripts/devnet-smoke.sh
```

The smoke covers table and JSON output, memo instructions, system-program transfer instructions, and SPL Token transfer instructions with devnet USDC (`4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`) when the keypair has that token available. For the token segment it creates temporary normal token accounts, funds the vault-owned token account from the keypair, and executes a raw Token Program transfer back to a member-owned token account. If devnet USDC is not available, the script prints `USDC token smoke: not available` and skips only that token segment.

## Release Dry Run

```sh
scripts/release-dry-run.sh
scripts/release-dry-run.sh --tag v0.1.0
```

Release artifacts are built without the experimental TUI feature.

## Squads v4 Program

- Program ID: `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`
- Network: Solana mainnet, devnet, testnet, localnet, or a custom trusted RPC
- Framework: Anchor-compatible account and instruction layouts parsed locally

## Why The Squads Types Are Duplicated

This CLI intentionally does not import the Squads SDK for its core account, instruction, and offline-message parsing paths. The local layouts keep the binary dependency tree small, make offline import independent from advisory metadata, and let CI enforce exactly which transitive crates enter the release.

The duplicated layouts are verified by focused unit tests and should be updated deliberately if the Squads v4 program layout changes.

## License

MIT OR Apache-2.0
