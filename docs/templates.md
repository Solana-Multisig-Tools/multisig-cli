# Templates

`msig template run <FILE>` creates a Squads vault transaction proposal from a static TOML template. Templates are meant for repeatable workflows where the instruction layout is fixed and operators provide only the minimum accounts or data inputs.

Templates are explicit files only. They are not auto-loaded, cannot execute code, cannot call RPC, cannot read environment variables, and cannot include other files.

## Inputs

Supported input types:

- `pubkey`
- `pubkey[]`
- `bytes`
- `string`
- `u8`
- `u16`
- `u32`
- `u64`
- `i64`
- `bool`

Pass inputs either as `--input KEY=VALUE` or direct flags:

```sh
msig template inspect merge-stake.toml
msig template validate merge-stake.toml --validator <VOTE_ACCOUNT>
msig template run transfer.toml --recipient <PUBKEY> --lamports 1000000
msig template run memo.toml --input memo=utf8:reviewed
```

`inspect` shows declared inputs, instruction count, and a SHA-256 hash of the file. `validate` compiles the template with real inputs and previews the instructions without creating a proposal.

`bytes` inputs parse hex by default. Prefix with `base64:` for Base64 or `utf8:` for literal UTF-8 bytes.

## Account Aliases

Account aliases must set exactly one source:

```toml
[accounts.system_program]
const = "11111111111111111111111111111111"

[accounts.vault]
context = "vault"

[accounts.destination]
input = "recipient"
```

Supported context accounts are `vault`, `multisig`, and `program_id`.

## Data Parts

Instruction data is a list of fixed bytes and typed inputs:

```toml
data = [
  { const_hex = "02000000" },
  { input = "lamports", encoding = "u64_le" },
]
```

Default input encodings are:

- `bytes` -> raw bytes
- `string` -> UTF-8
- `pubkey` -> 32 pubkey bytes
- integer types -> their matching little-endian encoding
- `bool` -> `0` or `1`

Explicit encodings include `u8`, `u16_le`, `u16_be`, `u32_le`, `u32_be`, `u64_le`, `u64_be`, `i64_le`, `i64_be`, `bool_u8`, `pubkey`, `utf8`, and `bytes`.

## Example: SOL Transfer

```toml
id = "system.transfer"
version = "1"
description = "Template SOL transfer from the Squads vault"

[inputs.recipient]
type = "pubkey"

[inputs.lamports]
type = "u64"

[accounts.system_program]
const = "11111111111111111111111111111111"

[accounts.vault]
context = "vault"

[[instructions]]
program = "system_program"
accounts = [
  { pubkey = "vault", writable = true, signer = true },
  { pubkey = "recipient", writable = true },
]
data = [
  { const_hex = "02000000" },
  { input = "lamports", encoding = "u64_le" },
]
```

## Example: Raw Data Input

```toml
id = "memo.raw"
version = "1"
description = "Write memo data from a bytes input"

[inputs.memo]
type = "bytes"

[accounts.memo_program]
const = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"

[[instructions]]
program = "memo_program"
data = [
  { input = "memo" },
]
```

Run it with:

```sh
msig template run memo.toml --memo utf8:reviewed
```

## Repeating Instructions

Use `for_each` with a `pubkey[]` input to repeat one instruction layout:

```toml
[inputs.accounts]
type = "pubkey[]"

[[instructions]]
program = "some_program"
for_each = "accounts"
accounts = [
  { pubkey = "$item", writable = true },
]
data = [
  { const_hex = "07000000" },
]
```

## One-Off Custom Instruction

For a one-off workflow where creating a template is not worth it, use `msig tx create`:

```sh
msig tx create \
  --vault-index 0 \
  --program 11111111111111111111111111111111 \
  --account vault:writable:signer \
  --account <RECIPIENT>:writable \
  --data 02000000e803000000000000
```

`--vault-index` is optional. If it is omitted, the command uses the global `--vault-index` value when provided, then falls back to config/defaults.

Account specs default to readonly non-signer. Add `:writable` and/or `:signer` when needed. The special account names `vault`, `multisig`, and `program_id` resolve to the active Squads context. Only `vault` may be marked signer.

Data accepts hex by default, plus `base64:<DATA>` or `utf8:<TEXT>`.

For pre-compiled workflows, `tx create` also accepts serialized vault transaction message bytes:

```sh
msig tx create --vault-index 0 --vault-message base64:<DATA>
```

The message may be hex, `base64:<DATA>`, or a JSON byte array. Serialized messages can include address lookup tables. This mode is the least ergonomic and least inspectable path; prefer templates or `--program/--account/--data` when humans need to review what is being built.

## Proposal Review And Simulation

Use `proposal show` before voting:

```sh
msig proposal show 42
msig proposal show 42 --verbose
msig proposal show <PROPOSAL_ADDRESS> --verbose
```

Default table output shows proposal status, votes, type, vault index, and each instruction with parsed intent when available. `proposal list` shows both `Index` and `Proposal`; index is the shortest operator path, and proposal address input is accepted when copying from an explorer or another tool. `--verbose` expands account metas, signer/writable flags, and raw instruction data.

After approval, simulate execution before sending:

```sh
msig proposal simulate 42
msig proposal simulate 42 --verbose
msig proposal simulate <PROPOSAL_ADDRESS> --output json
```

Simulation shows whether execution succeeds, compute units, SOL lamport deltas, token account amount deltas for SPL Token and Token-2022 accounts, owner changes, account creation/closure, and logs when verbose or when the simulation fails.
