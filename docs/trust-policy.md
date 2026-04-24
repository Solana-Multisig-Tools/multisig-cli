# Trust Policy

This CLI treats signing inputs as security boundaries. Convenience metadata is never enough to authorize a signature.

## Offline `.sqds` Files

- Import verifies the envelope signature over canonical transaction-critical fields: format version, cluster, multisig address, transaction index, vault index, config state hash, and serialized message bytes.
- Import independently parses the serialized Solana legacy message to derive required signers, blockhash, program IDs, account counts, and instruction data lengths.
- `transaction.decoded_instructions` is advisory exporter metadata. It may be included for UX, but import does not trust it for display or signing. If advisory program/count/length fields disagree with the parsed message, import rejects the file.
- Advisory decoded instruction names are not independently derivable from the Solana message alone and are not trusted on import.
- `tx combine` only merges signatures from `.sqds` files with identical transaction-critical metadata and message bytes, and re-verifies every merged signature against the stored message.

## Ledger Message Hash

Ledger Solana blind signing displays a Base58 SHA-256 hash of the raw Solana message bytes. Before any Ledger signing prompt, the CLI prints the same `Message Hash` value from the exact message it is about to sign. Operators should compare the CLI hash with the Ledger hash before approval.

## Templates

Templates are explicit TOML files passed to `msig template run`; they are never auto-loaded.

Templates may declare:

- typed inputs
- fixed account aliases
- fixed instruction layouts
- fixed or input-backed instruction data
- `for_each` expansion over `pubkey[]` inputs

Templates may not run code, call RPC, read environment variables, or load other files. A compiled template can only produce inner Squads vault transaction instructions. Because template-created inner messages currently use zero ephemeral signers, the CLI rejects any template account marked signer unless it is the active Squads vault.

## One-Off Custom Instructions

`msig tx create` is the explicit escape hatch for operators who do not want to create a template. It accepts a single program ID, repeated account metas, and optional instruction data from CLI flags.

Custom instructions have the same signer boundary as templates: only the active Squads vault may be marked as a signer. The command previews the exact program, accounts, account flags, vault, and data byte length before the Squads proposal transaction is signed.

`msig tx create --vault-index <N>` selects the vault for the proposal. If omitted, the command uses the global `--vault-index` value when provided, then the configured default.

`msig tx create --vault-message` also accepts pre-serialized vault transaction message bytes as hex, Base64, or a JSON byte array. This path is intentionally explicit and previews only byte length because the CLI does not reconstruct high-level instruction intent from arbitrary serialized message bytes.

Vault messages that use address lookup tables are supported during proposal inspection, simulation, and execution. The CLI fetches each lookup table account, verifies it is owned by the Solana address lookup table program, resolves referenced indexes, and includes the resolved remaining accounts in Squads execution.

## Proposal Review And Simulation

`msig proposal show <INDEX>` decodes each vault instruction in text output by default. `--verbose` expands signer/writable account metas and raw instruction bytes.

`msig proposal simulate <INDEX>` builds the same Squads execute instruction as `proposal execute`, simulates it through the configured RPC, and requests post-simulation account snapshots for all execution accounts. Text output highlights changed accounts, SOL lamport deltas, and SPL Token or Token-2022 balance deltas.

Config transactions decode spending-limit actions instead of treating them as opaque bytes. `proposal execute` and `proposal simulate` include required spending-limit remaining accounts for add/remove actions.

## Auto-Loaded `.msig.toml`

The CLI auto-loads `.msig.toml` only from the current working directory. It does not walk parent directories.

Precedence is:

1. built-in defaults
2. user config and active user profile
3. current-directory `.msig.toml`
4. `MSIG_*` environment variables
5. CLI flags

Untrusted `.msig.toml` may set:

- `default.cluster`, only when it is an official Solana moniker or exact official URL for devnet, testnet, or mainnet-beta
- `default.multisig`
- `default.commitment`
- `default.vault_index`
- `[labels]`
- `[tokens]`

Untrusted `.msig.toml` may not set:

- `default.keypair`
- `default.program_id`
- `default.priority_fee`
- localhost RPC
- arbitrary custom RPC URLs

Project config never expands `${ENV_VAR}` placeholders, even when trusted.

To trust a reviewed project config for the current command, set `MSIG_TRUST_PROJECT_CONFIG=1`. This allows the same fields as user config, including custom RPC URLs, localhost, keypair path, program ID, and priority fee.

## Custom RPC Trust

Custom RPC endpoints can lie or be stale about account state, proposal state, balances, simulations, blockhashes, transaction status, and prioritization behavior. They can influence what the CLI prepares for signing.

Use custom RPCs only through an explicit trust surface:

- `--cluster <URL>` for one command
- `MSIG_CLUSTER=<URL>` for the current shell
- user config for a persistent operator-owned endpoint
- `MSIG_TRUST_PROJECT_CONFIG=1` after reviewing the current repo's `.msig.toml`

For mainnet signing, prefer a provider or self-hosted endpoint you control and compare critical proposal details before approving.

## CI And Release

- CI and release workflows use SHA-pinned official GitHub actions.
- Workflows use minimal GitHub token permissions: read-only for CI and by default in release, write only on release jobs that create or upload draft artifacts.
- CI, release, and local release dry-runs use the pinned Rust toolchain in `rust-toolchain.toml`.
- Cargo commands run with `--locked`; release artifacts come from `Cargo.lock`.
- Every pull request runs RustSec advisory checks, the repository dependency policy in `deny.toml`, formatting, feature-matrix tests, clippy with warnings denied, and `cargo package --locked`.
- Release builds rerun tests and clippy, package the crate, build Linux x86_64 and macOS x86_64/aarch64 binaries, and emit SHA-256 checksums.
- Release binaries are built with the `ledger` feature only; the experimental TUI feature is tested in CI but not shipped.
- The release workflow publishes only from an existing strict `vMAJOR.MINOR.PATCH` tag and creates a draft GitHub release. Workflow dispatch can run the same path as a dry run without publishing.
- `scripts/release-dry-run.sh` mirrors the release validation path locally and emits host-platform checksums under `target/release-dry-run`.
- Squads SDK and SPL SDK crates are intentionally not test dependencies for this CLI.

See [docs/ci-release.md](ci-release.md) for the exact PR, release, and local dry-run command matrix.
