# msig Project Conventions

## Language & Toolchain
- Rust 2021 edition
- Strict clippy: `#[deny(clippy::unwrap_used, clippy::expect_used)]`
- No tokio — sync only (ureq for HTTP, std::thread for TUI background)

## Architecture
- `core/` returns domain structs, never formatted strings
- Hand-written Borsh for critical account types (Multisig, Proposal), derive for the rest
- Integer-only amount parsing — never f64

## Security
- All on-chain data sanitized before display
- Labels are ASCII-only
- `zeroize` on all key material
- This project manages 20B+ AUM; security is paramount

## Error Handling
- Ranged exit codes (see src/error.rs)
- Every error variant has a concrete fix_suggestion()
- JSON error output available via ErrorReport

## Dependencies
- Granular Solana crates (NOT solana-sdk)
- Minimal dependency tree
- TUI and Ledger support are feature-gated
