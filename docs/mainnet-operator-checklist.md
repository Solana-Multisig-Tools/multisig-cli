# Mainnet Operator Checklist

Use this checklist before signing transactions that can move real assets.

## 1. Install And Verify

Install from a release artifact and verify its SHA-256 checksum:

```sh
export MSIG_VERSION=v0.1.0
export MSIG_TARGET=aarch64-apple-darwin

curl -LO "https://github.com/Solana-Multisig-Tools/v4-cli/releases/download/${MSIG_VERSION}/msig-${MSIG_VERSION}-${MSIG_TARGET}"
curl -LO "https://github.com/Solana-Multisig-Tools/v4-cli/releases/download/${MSIG_VERSION}/msig-${MSIG_VERSION}-${MSIG_TARGET}.sha256"
shasum -a 256 -c "msig-${MSIG_VERSION}-${MSIG_TARGET}.sha256"
chmod +x "msig-${MSIG_VERSION}-${MSIG_TARGET}"
sudo install -m 0755 "msig-${MSIG_VERSION}-${MSIG_TARGET}" /usr/local/bin/msig
msig --version
```

Set `MSIG_TARGET` to the artifact for your machine: `aarch64-apple-darwin`, `x86_64-apple-darwin`, or `x86_64-unknown-linux-gnu`.

Or install from source with locked dependencies:

```sh
git clone https://github.com/Solana-Multisig-Tools/v4-cli.git
cd v4-cli
cargo install --path . --locked
msig --version
```

The first mainnet release is CLI-only. Release binaries intentionally do not include the experimental TUI, and mainnet operator procedures should use the CLI and JSON/table outputs below.

## 2. Review Local Trust

Run these from the directory where you will operate. This makes any auto-loaded `.msig.toml` visible.

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> config doctor
msig --cluster mainnet --ledger --multisig <MULTISIG> --output json config preflight
```

Do not set `MSIG_TRUST_PROJECT_CONFIG=1` unless you reviewed the current directory's `.msig.toml`. Custom RPC URLs, localhost RPC, custom program IDs, keypair paths, and priority fees are sensitive.

## 3. Inspect The Proposal

Use both the human review and JSON output. The human output is for operator review; JSON is for capture, audit logs, and scripts.

```sh
msig --cluster mainnet --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS>
msig --cluster mainnet --multisig <MULTISIG> proposal show <INDEX|PROPOSAL_ADDRESS> --verbose
msig --cluster mainnet --multisig <MULTISIG> --output json proposal show <INDEX|PROPOSAL_ADDRESS>
```

For vault transactions, confirm every inner instruction:

- program ID
- decoded instruction, when available
- writable and signer account metas
- raw data bytes in `--verbose`
- vault index
- proposal address and transaction index

## 4. Simulate Execution

Simulation uses the configured RPC. Treat a custom RPC as part of the trust surface.

```sh
msig --cluster mainnet --multisig <MULTISIG> proposal simulate <INDEX|PROPOSAL_ADDRESS>
msig --cluster mainnet --multisig <MULTISIG> proposal simulate <INDEX|PROPOSAL_ADDRESS> --verbose
msig --cluster mainnet --multisig <MULTISIG> --output json proposal simulate <INDEX|PROPOSAL_ADDRESS>
```

Review SOL lamport deltas, SPL Token or Token-2022 balance deltas, account creations/closures, owner changes, data length changes, and logs when verbose.

## 5. Dry Run The Signing Action

Dry-run the exact action before asking a hardware wallet to sign.

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> --dry-run proposal approve <INDEX|PROPOSAL_ADDRESS>
msig --cluster mainnet --ledger --multisig <MULTISIG> --dry-run proposal execute <INDEX|PROPOSAL_ADDRESS>
```

For an approval, execute only after the proposal has enough approvals and simulation still matches expectations.

## 6. Compare Ledger Message Hash

Ledger Solana blind signing displays a Base58 SHA-256 hash of the raw Solana message bytes. `msig` prints the same `Message Hash` immediately before Ledger approval.

When running the real signing command, compare the CLI `Message Hash` with the hash displayed on Ledger before approving:

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> proposal approve <INDEX|PROPOSAL_ADDRESS>
```

For execution:

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> proposal execute <INDEX|PROPOSAL_ADDRESS>
```

If the hashes do not match, reject on Ledger and stop.

## 7. Archive Evidence

For high-value operations, save the JSON artifacts:

```sh
msig --cluster mainnet --multisig <MULTISIG> --output json config preflight > preflight.json
msig --cluster mainnet --multisig <MULTISIG> --output json proposal show <INDEX|PROPOSAL_ADDRESS> > proposal.json
msig --cluster mainnet --multisig <MULTISIG> --output json proposal simulate <INDEX|PROPOSAL_ADDRESS> > simulation.json
```

If signing offline, export and verify the `.sqds` file before signatures are collected:

```sh
msig --cluster mainnet --ledger --multisig <MULTISIG> tx export <INDEX> --action approve --file proposal-approve.sqds
msig --cluster mainnet --output json tx status proposal-approve.sqds > proposal-approve-status.json
```
