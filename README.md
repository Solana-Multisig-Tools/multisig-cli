# msig

CLI for Squads Multisig v4 on Solana.

`msig` is built for operators who need to review, simulate, sign, and execute Squads proposals from a small Rust binary. It uses hand-written Squads v4 account and instruction parsing instead of pulling in the Squads SDK dependency tree.

## What It Does

- Inspect multisigs, vaults, proposals, and transactions.
- Show decoded proposal instructions and verbose account metas before signing.
- Simulate proposal execution and surface SOL/token balance diffs.
- Create transfer, member, config, program-upgrade, rent, template, and raw vault transaction proposals.
- Sign with a keypair, Ledger, or offline `.sqds` workflow.
- Print the same Solana message hash that Ledger blind signing displays.

## Install

```sh
git clone https://github.com/Solana-Multisig-Tools/v4-cli.git
cd v4-cli
cargo install --path . --locked
msig --version
```

For release artifact installation and mainnet signing steps, use the [mainnet operator checklist](docs/mainnet-operator-checklist.md).

## Configure

```sh
msig config set default.cluster https://api.mainnet-beta.solana.com
msig config set default.multisig <MULTISIG>
msig config set default.vault_index 0
msig config show
```

For Ledger signing, do not configure a local keypair. Pass `--ledger` on signing commands.

Before mainnet signing:

```sh
msig --cluster mainnet --multisig <MULTISIG> --ledger config doctor
msig --cluster mainnet --multisig <MULTISIG> --ledger --output json config preflight
```

## Review A Proposal

```sh
msig --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS>
msig --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig --multisig <MULTISIG> proposal simulate <INDEX|PROPOSAL_ADDRESS>
msig --multisig <MULTISIG> --output json proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

`proposal show --verbose` expands each instruction with parsed data when available, raw data, account metas, writable flags, and signer flags. `proposal simulate` shows execution success, compute units, logs, SOL lamport deltas, SPL Token and Token-2022 balance deltas, and account owner/data changes.

## Create Proposals

```sh
msig --cluster devnet --keypair ~/.config/solana/id.json multisig create \
  --threshold 2 \
  --members <MEMBER_A>,<MEMBER_B>,<MEMBER_C>

msig --multisig <MULTISIG> transfer sol 1.5 <RECIPIENT>

msig --multisig <MULTISIG> member add <MEMBER> --permissions all
```

For custom vault transactions:

```sh
msig template inspect workflow.toml
msig template validate workflow.toml --input recipient=<PUBKEY>
msig template run workflow.toml --input recipient=<PUBKEY>
```

```sh
msig tx create \
  --vault-index 0 \
  --program <PROGRAM_ID> \
  --account vault:writable:signer \
  --account <ACCOUNT>:writable \
  --data <HEX|base64:DATA|utf8:TEXT>
```

Use templates for repeatable workflows that should be shared or reviewed. Use `tx create` for one-off custom instructions. Full template reference: [docs/templates.md](docs/templates.md).

## Signing

Every signing command previews the transaction and prints a `Message Hash` before requesting a signature. Ledger Solana blind signing displays the Base58 SHA-256 hash of the raw Solana message bytes; compare it with the CLI hash before approving.

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> proposal approve <INDEX>
msig --cluster mainnet --ledger --multisig <MULTISIG> proposal execute <INDEX>
```

Offline signing uses `.sqds` files:

```sh
msig --cluster mainnet --multisig <MULTISIG> tx export <INDEX> --action approve --file approve.sqds
msig --output json tx status approve.sqds
msig tx combine approve-a.sqds approve-b.sqds --file approve-combined.sqds
msig tx import approve-combined.sqds
```

## Safety Defaults

- `.sqds` import independently decodes the Solana message instead of trusting advisory metadata.
- `.msig.toml` is only auto-loaded from the current directory; custom RPC URLs, keypair paths, program IDs, localhost, and priority fees require explicit trust.
- Templates are explicit TOML files and cannot execute code, call RPC, read environment variables, include other files, or mark non-vault accounts as signers.
- Release binaries are built with the `ledger` feature only; the experimental TUI is not shipped.
- CI and release run locked dependency checks, RustSec advisory checks, dependency policy checks, formatting, tests, clippy, package verification, and checksum builds.

More detail: [docs/trust-policy.md](docs/trust-policy.md) and [docs/ci-release.md](docs/ci-release.md).

## Library API: instruction-builder feature

`msig` doubles as a Rust library that publishes the canonical Squads multisig v4 data types and instruction builders, so other client SDKs can produce v4 instructions byte-for-byte identical to those produced by this CLI without copying the implementation or pulling in the Squads SDK dependency tree.

Add the dependency with default features off so the Ledger transport, the binary entry point, and the rest of the CLI-only surface stay out of your build:

```toml
[dependencies]
msig = { git = "https://github.com/Solana-Multisig-Tools/v4-cli", default-features = false, features = ["instruction-builder"] }
```

The `instruction-builder` feature pulls in `solana-instruction` and exposes the `msig::instruction_builder` module, which re-exports the pure v4 data types from `msig::domain` and provides Solana-typed instruction builders that return `solana_instruction::Instruction`:

```rust
use msig::instruction_builder as v4;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

fn approve_proposal(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    member: Pubkey,
) -> Instruction {
    v4::vote(program_id, multisig, proposal, member, v4::Vote::Approve)
}
```

Building this CLI as a binary is unaffected: the feature is opt-in, and the binary build pulls zero additional dependencies.

## Docs

- [Mainnet operator checklist](docs/mainnet-operator-checklist.md)
- [Templates](docs/templates.md)
- [CLI reference](docs/cli-reference.md)
- [Trust policy](docs/trust-policy.md)
- [CI and release](docs/ci-release.md)
- [Local development](examples/local-development.md)

## TUI Status

The TUI is intentionally not part of the first mainnet release. Release binaries exclude it, `msig tui` is hidden from help, and direct calls return an error unless the binary was built with `--features tui` and `MSIG_ENABLE_EXPERIMENTAL_TUI=1` is set.

## Development

```sh
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
scripts/release-dry-run.sh
```

The devnet smoke script sends real devnet transactions and creates devnet artifacts:

```sh
MSIG_DEVNET_KEYPAIR=/path/to/devnet-keypair.json \
MSIG_DEVNET_MEMBER=<KEYPAIR_PUBKEY> \
scripts/devnet-smoke.sh
```

## License

MIT OR Apache-2.0
