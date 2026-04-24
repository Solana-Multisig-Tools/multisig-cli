# Security

Please report security issues privately through the repository's configured GitHub security advisory channel:

https://github.com/Solana-Multisig-Tools/msig/security/advisories/new

The CLI's trust model is documented in [docs/trust-policy.md](docs/trust-policy.md). The important defaults are:

- `.sqds` import decodes transaction messages independently instead of trusting advisory metadata.
- Auto-loaded `.msig.toml` cannot silently set keypairs, custom RPC URLs, program IDs, or priority fees unless `MSIG_TRUST_PROJECT_CONFIG=1` is set.
- Templates are explicit files only and cannot execute code, call RPC, or mark non-vault accounts as signers.
- One-off custom instructions are explicit CLI invocations and cannot mark non-vault accounts as signers.
- Proposal simulation shows account, SOL, and token balance diffs before execution.
- Address lookup table backed vault transactions are resolved from on-chain lookup table accounts before show/simulate/execute.
- Release artifacts are built from locked dependencies and published with SHA-256 checksums.
