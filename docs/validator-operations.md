# Validator Operations

`msig vote-account` lets a validator run vote account withdrawer operations from a Squads vault, without a UI. The vault is the vote account's authorized withdrawer; every operation becomes one Squads vault-transaction proposal that the multisig members review, approve and execute like any other proposal.

Each command creates exactly one proposal containing one Vote program instruction, the same granularity as the `solana` CLI. The only inner signer is the vault.

Not supported yet:

- changing the validator identity (`UpdateValidatorIdentity`)
- stake account operations
- changing the voter of a vote account that has a BLS key registered (see [BLS voter](#bls-voter))

## Setup

Get the vault address for the vault index you want to use:

```sh
msig --multisig <MULTISIG> --vault-index 0 vault balance
```

The first line prints `Vault [0]: <VAULT>`.

For a new vote account, make the vault the authorized withdrawer at creation:

```sh
solana create-vote-account <VOTE_KEYPAIR> <IDENTITY_KEYPAIR> <VAULT>
```

For an existing vote account, hand the withdrawer authority to the vault:

```sh
solana vote-authorize-withdrawer <VOTE_ACCOUNT> <CURRENT_WITHDRAWER_KEYPAIR> <VAULT>
```

Use `vote-authorize-withdrawer`, not `vote-authorize-withdrawer-checked`: the checked variant needs the new authority to sign, and a vault cannot sign outside a Squads execution.

Check the result:

```sh
msig --multisig <MULTISIG> vote-account show <VOTE_ACCOUNT>
```

The authorized withdrawer is labelled `(vault N)` when it is one of the multisig's vaults.

## Commands

All proposal commands require an explicit global `--vault-index <N>`, the index of the vault that is the authorized withdrawer. Unlike `tx create` there is no fallback to the configured default. They accept the global `--memo`, `--dry-run`, `--yes` and `--output json` flags.

| Command | Proposes |
|---|---|
| `msig vote-account show <VOTE_ACCOUNT>` | Read only. Balance, rent-exempt minimum, version, validator identity, authorized withdrawer, commission, V4 collectors, and whether a BLS key is registered. |
| `msig vote-account withdraw <VOTE_ACCOUNT> <RECIPIENT> (--amount <SOL> \| --all \| --close)` | `Withdraw`. `--all` withdraws everything above the rent-exempt minimum; `--close` withdraws the full balance. |
| `msig vote-account set-commission <VOTE_ACCOUNT> <PERCENT>` | `UpdateCommission`, 0 to 100. On V4 vote accounts this sets the inflation-rewards commission only; the block-revenue commission is unchanged. |
| `msig vote-account set-voter <VOTE_ACCOUNT> <NEW_VOTER>` | `Authorize(Voter)`. |
| `msig vote-account set-withdrawer <VOTE_ACCOUNT> <NEW_WITHDRAWER> [--allow-external-authority]` | `Authorize(Withdrawer)`. |
| `msig vote-account set-collector <VOTE_ACCOUNT> <inflation-rewards\|block-revenue> <COLLECTOR> [--allow-external-authority]` | `UpdateCommissionCollector` (SIMD-0232). |

Before anything is proposed, every proposal command:

1. fetches the vote account, checks that it is owned by the Vote program and parses it (V1_14_11, V3 and V4 layouts)
2. checks that the authorized withdrawer is the vault at `--vault-index`
3. runs the command-specific checks below
4. simulates the inner vote instruction on its own, with the proposer as fee payer

Command-specific checks:

- `withdraw`: refuses zero, more than the balance, or an amount that would leave a non-zero balance below the rent-exempt minimum.
- `set-voter`: refuses V4 accounts with a BLS key registered.
- `set-withdrawer`: refuses a new withdrawer that is not one of this multisig's 256 vaults unless `--allow-external-authority` is passed.
- `set-collector`: the collector must be the vote account itself, or an existing system-owned account holding at least its rent-exempt minimum. A collector that is neither the vote account nor one of this multisig's vaults is refused unless `--allow-external-authority` is passed.

With `--allow-external-authority`, the CLI prints a warning and adds the external authority to the description shown in your signing preview. That description is not stored on chain: other approvers see the `WARNING` line that `msig proposal show` prints for the instruction.

If the inner simulation fails, the CLI prints the error and logs and stops. `--allow-simulation-failure` proposes anyway, for operations that fail now but are expected to succeed at execution time, for example a commission change outside the part of the epoch where the vote program allows it, or an authority change that is already queued.

## Propose, Review, Approve, Execute

```sh
# 1. Propose (prints the proposal index)
msig --multisig <MULTISIG> --vault-index 0 vote-account withdraw <VOTE_ACCOUNT> <RECIPIENT> --all

# 2. Review the decoded instruction
msig --multisig <MULTISIG> proposal show <INDEX>
msig --multisig <MULTISIG> proposal show <INDEX> --verbose

# 3. Approve (each member, up to the threshold)
msig --multisig <MULTISIG> proposal approve <INDEX>

# 4. Simulate execution right before executing
msig --multisig <MULTISIG> proposal simulate <INDEX>

# 5. Execute
msig --multisig <MULTISIG> proposal execute <INDEX>
```

`proposal show` decodes Vote program instructions (withdraw amount and recipient, commission, authorize role and new authority, collector) and prints a `WARNING` line when a withdrawer authority or a collector is set to a key outside the multisig's vaults, or when a Vote instruction cannot be decoded.

## Closing A Vote Account

`withdraw --close` withdraws the full balance, which closes the account. The vote program only allows this when the vote account earned no credits in the current or the previous epoch. While the validator is still earning credits, the inner simulation at propose time already fails with `ActiveVoteAccountClose`, so `withdraw --close` refuses to propose unless you pass `--allow-simulation-failure`. Even a proposal that simulated successfully when it was created can still fail at execution if the validator voted in the meantime. Stop voting, wait out the epochs, then run `proposal simulate` before executing.

`withdraw --amount` refuses an amount equal to the whole balance: closing the account must be requested explicitly with `--close`.

## BLS Voter

On V4 vote accounts with a BLS key registered, the vote program (SIMD-0387) rejects a plain `Authorize(Voter)`, so `set-voter` refuses before proposing. The current voter can rotate itself:

```sh
solana vote-authorize-voter-checked <VOTE_ACCOUNT> <CURRENT_VOTER_KEYPAIR> <NEW_VOTER_KEYPAIR>
```

Changing the voter from the multisig needs the new voter's BLS key and proof of possession, which msig does not support yet.
