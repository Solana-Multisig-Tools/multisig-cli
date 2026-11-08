# Validator Operations

`msig vote-account` lets a validator run vote account withdrawer operations from a Squads vault, without a UI; the vault is the vote account's authorized withdrawer. `msig stake-account` does the same for stake accounts whose staker and/or withdrawer is a vault; see [Stake accounts](#stake-accounts). Every operation becomes one Squads vault-transaction proposal that the multisig members review, approve and execute like any other proposal, the same granularity as the `solana` CLI.

Each `vote-account` command creates exactly one proposal containing one Vote program instruction, the same granularity as the `solana` CLI. The inner signer is the vault, except for `set-identity`, where the new validator identity must also sign (see [Changing The Validator Identity](#changing-the-validator-identity)). `stake-account` commands sign with the vault too; `create` and `split` also use one ephemeral signer PDA of the proposal's transaction as the new stake account (see [Stake accounts](#stake-accounts)).

Not supported yet:

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
| `msig vote-account set-identity <VOTE_ACCOUNT> <NEW_IDENTITY>` | `UpdateValidatorIdentity`. The new identity must co-sign the execution. |

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
- `set-identity`: refuses the current identity, the vote account itself, and any of this multisig's vaults.

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

`proposal show` decodes Vote program instructions (withdraw amount and recipient, commission, authorize role and new authority, collector, new validator identity) and prints a `WARNING` line when a withdrawer authority or a collector is set to a key outside the multisig's vaults, or when a Vote instruction cannot be decoded.

## Changing The Validator Identity

`UpdateValidatorIdentity` needs two signatures: the authorized withdrawer (the vault, signed by the Squads program at execution) and the new validator identity. The new identity is not part of the multisig, so it signs the execute transaction itself:

```sh
# 1. Propose
msig --multisig <MULTISIG> --vault-index 0 vote-account set-identity <VOTE_ACCOUNT> <NEW_IDENTITY>

# 2. Review and approve as usual
msig --multisig <MULTISIG> proposal show <INDEX>
msig --multisig <MULTISIG> proposal approve <INDEX>

# 3. Simulate (marks the new identity as signer; nothing is signed)
msig --multisig <MULTISIG> proposal simulate <INDEX>

# 4. Execute with the new identity keypair as extra signer
msig --multisig <MULTISIG> proposal execute <INDEX> --extra-signer <NEW_IDENTITY_KEYPAIR>
```

The new identity's keypair file must be available on the machine that runs `proposal execute`. Ledger is not supported for the identity key in this version: `--extra-signer` only accepts a keypair file. If the identity keypair is not passed with `--extra-signer`, `proposal execute` stops before signing and names the missing key. With `--output json`, co-signing also requires `--yes`, because JSON mode shows no preview. The execute preview lists the extra signer and the decoded `UpdateValidatorIdentity` it co-signs. The executor cannot be the new identity: the CLI refuses to let the fee payer signature double as the identity signature, so execute with another member.

Execution is online only. `msig tx export <INDEX> --action execute` refuses a proposal that needs a co-signer outside the multisig, and points to `proposal execute --extra-signer`. Offline co-signing (for example with the identity key on a Ledger or an air-gapped machine) is a planned follow-up.

After execution, check the new identity with `msig vote-account show <VOTE_ACCOUNT>`, and make sure the validator node runs with the new identity keypair. On V4 vote accounts, also check the block-revenue collector shown there.

## Closing A Vote Account

`withdraw --close` withdraws the full balance, which closes the account. The vote program only allows this when the vote account earned no credits in the current or the previous epoch. While the validator is still earning credits, the inner simulation at propose time already fails with `ActiveVoteAccountClose`, so `withdraw --close` refuses to propose unless you pass `--allow-simulation-failure`. Even a proposal that simulated successfully when it was created can still fail at execution if the validator voted in the meantime. Stop voting, wait out the epochs, then run `proposal simulate` before executing.

`withdraw --amount` refuses an amount equal to the whole balance: closing the account must be requested explicitly with `--close`.

## BLS Voter

On V4 vote accounts with a BLS key registered, the vote program (SIMD-0387) rejects a plain `Authorize(Voter)`, so `set-voter` refuses before proposing. The current voter can rotate itself:

```sh
solana vote-authorize-voter-checked <VOTE_ACCOUNT> <CURRENT_VOTER_KEYPAIR> <NEW_VOTER_KEYPAIR>
```

Changing the voter from the multisig needs the new voter's BLS key and proof of possession, which msig does not support yet.

## Stake Accounts

`msig stake-account` runs stake account operations from a vault that is the stake account's staker and/or withdrawer. Each command creates exactly one proposal, like the `solana` CLI command of the same name.

### Setup

Create stake accounts through msig: `stake-account create` makes the vault both staker and withdrawer, with no lockup.

For an existing stake account, hand the authorities to the vault (get the vault address as in [Setup](#setup)):

```sh
solana stake-authorize <STAKE_ACCOUNT> --new-stake-authority <VAULT> --new-withdraw-authority <VAULT> \
  --stake-authority <CURRENT_STAKER_KEYPAIR> --withdraw-authority <CURRENT_WITHDRAWER_KEYPAIR>
```

Use `stake-authorize`, not `stake-authorize-checked`: the checked variant needs the new authority to sign, and a vault cannot sign outside a Squads execution. Keeping the staker on a hot key and only the withdrawer on the vault also works; `delegate`, `deactivate`, `split` and `merge` then stay with the hot key.

Check the result:

```sh
msig --multisig <MULTISIG> stake-account show <STAKE_ACCOUNT>
```

The staker and withdrawer are labelled `(vault N)` when they are vaults of the multisig.

### Commands

All proposal commands require an explicit global `--vault-index <N>` and accept the global `--memo`, `--dry-run`, `--yes` and `--output json` flags, plus `--allow-simulation-failure`.

| Command | Vault must be | Proposes |
|---|---|---|
| `msig stake-account show <STAKE_ACCOUNT>` | | Read only. State, balance, rent-exempt reserve, staker, withdrawer, lockup, delegation (vote account, stake, activation and deactivation epochs) and the current epoch. |
| `msig stake-account create <AMOUNT_SOL> [--vote-account <VOTE_ACCOUNT>]` | funder | System `Transfer` of the amount from the vault to the new account, `Allocate` 200 bytes, `Assign` to the Stake program, `Initialize` with staker = withdrawer = vault and no lockup, and with `--vote-account` also `DelegateStake`. The amount includes the rent-exempt reserve, as in `solana create-stake-account`. |
| `msig stake-account delegate <STAKE_ACCOUNT> <VOTE_ACCOUNT>` | staker | `DelegateStake`, also used to redelegate. |
| `msig stake-account deactivate <STAKE_ACCOUNT>` | staker | `Deactivate`. |
| `msig stake-account withdraw <STAKE_ACCOUNT> <RECIPIENT> (--amount <SOL> \| --all)` | withdrawer | `Withdraw`. `--all` withdraws the full balance, which closes the account. |
| `msig stake-account split <STAKE_ACCOUNT> <AMOUNT_SOL>` | staker | System `Transfer` of the new account's rent-exempt reserve from the vault, `Allocate` 200 bytes, `Assign` to the Stake program, then `Split`, as in `solana split-stake`. |
| `msig stake-account merge <DESTINATION> <SOURCE>` | staker of both | `Merge` of SOURCE into DESTINATION. Both must have the same staker, withdrawer and lockup. |
| `msig stake-account set-staker <STAKE_ACCOUNT> <NEW_STAKER>` | staker or withdrawer | `Authorize(Staker)`. |
| `msig stake-account set-withdrawer <STAKE_ACCOUNT> <NEW_WITHDRAWER> [--allow-external-authority]` | withdrawer | `Authorize(Withdrawer)`. |

`create` and `split` put the new stake account at the proposal transaction's ephemeral signer address (index 0), which the Squads program signs for at execution. There is no keypair to keep. The command prints the address (`new_stake_account` in JSON output) before the proposal is approved. The address depends on the proposal's transaction index: if another proposal is created while the command runs, it stops without proposing, and you run it again.

`create` funds the new account with `Transfer` + `Allocate` + `Assign` rather than the single `CreateAccount` that `solana create-stake-account` uses. The new address is predictable as soon as the proposal exists, and `CreateAccount` fails on an address that already holds lamports, so anyone could block the proposal by sending it dust before execution. `Transfer`, `Allocate` and `Assign` still succeed on such an account, which stays system-owned with no data; any extra lamports end up in the new stake account. At propose time the command still refuses an address that already holds an account.

Before anything is proposed, every command except `create` (which has no existing stake account):

1. fetches the stake account, checks that it is owned by the Stake program and parses it
2. checks that the vault at `--vault-index` holds the authority in the table

Then every command, `create` included:

3. runs the command-specific checks below
4. simulates the inner instructions on their own, with the proposer as fee payer

Command-specific checks:

- `create`: the amount must be at least the rent-exempt minimum for 200 bytes, plus the stake minimum delegation (`getStakeMinimumDelegation`) when `--vote-account` is given; the vault must hold the amount and keep either nothing or at least its own rent-exempt minimum (0-byte account); `--vote-account` must be owned by the Vote program.
- `delegate`: the vote account must be owned by the Vote program.
- `deactivate`: the stake must be delegated and not already deactivating.
- `withdraw`: refuses zero, more than the balance, an `--amount` equal to the whole balance (use `--all`), a remainder below the rent-exempt reserve, and, while the stake is delegated and not deactivating, anything above the balance minus the delegated stake and the reserve. Refuses while a lockup is in force.
- `split`: refuses zero or more than the balance; the vault must hold the new account's rent-exempt reserve and keep either nothing or at least its own rent-exempt minimum.
- `merge`: destination and source must differ, and both must have the same staker, withdrawer and lockup.
- `set-withdrawer`: refuses while a lockup is in force, and refuses a new withdrawer that is not one of this multisig's 256 vaults unless `--allow-external-authority` is passed. `set-staker` takes no such flag: stakers are routinely hot keys.

`proposal show` decodes Stake program instructions (initialize, authorize role and new authority, delegation, split, withdraw amount and recipient, deactivate, merge, lockup changes, move stake/lamports) and prints a `WARNING` line when a withdrawer is set to a key outside the multisig's vaults, when a lockup or custodian changes, or when a Stake instruction cannot be decoded.

### Epochs And Cooldown

Stake activates and deactivates at epoch boundaries, and large amounts can take several epochs to warm up or cool down. To take stake out:

1. `stake-account deactivate <STAKE_ACCOUNT>`, approve and execute
2. wait until the stake has cooled down (`stake-account show` shows the deactivation epoch; `solana stake-account <STAKE_ACCOUNT>` shows the remaining active stake)
3. `stake-account withdraw <STAKE_ACCOUNT> <RECIPIENT> --all`

While the stake is still cooling down, the inner simulation of `withdraw` fails and the command stops. `--allow-simulation-failure` proposes anyway, for a withdrawal that will only be executed after the cooldown; run `proposal simulate` before executing it.

### Split And Merge

- `split` moves lamports into a new stake account with the same authorities, lockup and delegation state. If the source is delegated, both the split amount and what stays in the source must meet the minimum delegation; the Stake program enforces this and the inner simulation reports it.
- `merge` requires both accounts to have the same staker, withdrawer and lockup, which the command checks before proposing, and compatible delegation states, for example both inactive, or both fully active and delegated to the same vote account. Accounts that are activating or deactivating partway cannot always be merged. The Stake program enforces delegation compatibility and the inner simulation reports it.

### Minimum Delegation

The Stake program refuses delegations below the cluster's minimum delegation. `create --vote-account` checks it before proposing; `delegate` and `split` leave it to the inner simulation.

### Lockups

Lockups and custodians are not supported. Stake accounts created by msig have no lockup. For an existing account with a lockup in force, `withdraw` and `set-withdrawer` are refused because they would need the custodian's signature; the other commands work as usual.
