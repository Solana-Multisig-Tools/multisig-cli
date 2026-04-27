//! Public SDK surface for building Squads multisig v4 instructions.
//!
//! Enabled via the `instruction-builder` feature. When enabled, this module
//! re-exports the canonical Squads v4 data types and exposes thin wrappers
//! around the internal instruction builders that return
//! [`solana_instruction::Instruction`] directly, so downstream client SDKs can
//! produce v4 instructions byte-for-byte identical to those produced by this
//! CLI without copying the implementation.
//!
//! Add the dependency with default features off to keep the Ledger transport
//! and the rest of the CLI-only surface out of your build:
//!
//! ```toml
//! [dependencies]
//! msig = { git = "https://github.com/Solana-Multisig-Tools/multisig-cli", default-features = false, features = ["instruction-builder"] }
//! ```

use solana_instruction::{AccountMeta as SolAccountMeta, Instruction as SolInstruction};
use solana_pubkey::Pubkey;

use crate::{error::MsigError, infra::instruction::AccountMeta};

pub use crate::{
    application::{config_tx, multisig, program_upgrade, proposal, rent, vault_tx},
    domain::{
        multisig::{Member, MultisigInfo, Permissions, TokenBalance, VaultBalances},
        proposal::{ProposalDetail, ProposalStatus, ProposalSummary, TransactionType, Vote},
        transaction::{
            AccountRef, ConfigAction, ConfigTxInfo, InstructionSummary, SpendingLimitPeriod,
            VaultTxInfo,
        },
    },
    infra::accounts::vault_tx::TransactionMessage as VaultTransactionMessage,
};

/// Build the `proposalVote` instruction for a member casting `vote` on `proposal`.
pub fn vote(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    member: Pubkey,
    vote: Vote,
) -> SolInstruction {
    proposal::build_vote_instruction(program_id, multisig, proposal, member, vote).into()
}

/// Build the `vaultTransactionExecute` instruction for an approved vault transaction.
///
/// `loaded_lookup_accounts` are the additional accounts referenced through the
/// transaction's address-lookup-table entries, in the order they appear in the
/// stored message; the caller is responsible for resolving them from the
/// referenced lookup tables.
#[allow(clippy::too_many_arguments)]
pub fn vault_transaction_execute(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    message: &VaultTransactionMessage,
    lookup_table_accounts: &[Pubkey],
    loaded_lookup_accounts: &[SolAccountMeta],
) -> SolInstruction {
    let loaded: Vec<AccountMeta> = loaded_lookup_accounts
        .iter()
        .cloned()
        .map(Into::into)
        .collect();
    proposal::build_vault_transaction_execute_instruction(
        program_id,
        multisig,
        proposal,
        transaction,
        executor,
        message,
        lookup_table_accounts,
        &loaded,
    )
    .into()
}

/// Build the `configTransactionExecute` instruction for an approved config transaction.
pub fn config_transaction_execute(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    actions: &[ConfigAction],
) -> SolInstruction {
    proposal::build_config_transaction_execute_instruction(
        program_id,
        multisig,
        proposal,
        transaction,
        executor,
        actions,
    )
    .into()
}

/// Build the `vaultTransactionCreate` instruction. `transaction_message` is the
/// pre-serialized inner message bytes (see the v4 program's `TransactionMessage`
/// layout).
pub fn vault_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
) -> Result<SolInstruction, MsigError> {
    vault_tx::build_vault_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        vault_index,
        transaction_message,
    )
    .map(Into::into)
}

/// Build the `proposalCreate` instruction. Pass `draft = false` for proposals
/// that should be created in the `Active` state immediately.
pub fn proposal_create(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    creator: Pubkey,
    transaction_index: u64,
    draft: bool,
) -> SolInstruction {
    vault_tx::build_proposal_create_instruction(
        program_id,
        multisig,
        proposal,
        creator,
        transaction_index,
        draft,
    )
    .into()
}

/// Build the `proposalActivate` instruction (transitions a draft proposal to active).
pub fn proposal_activate(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    creator: Pubkey,
) -> SolInstruction {
    vault_tx::build_proposal_activate_instruction(program_id, multisig, proposal, creator).into()
}

/// Build the `configTransactionCreate` instruction.
pub fn config_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    action: config_tx::ConfigTransactionAction,
) -> SolInstruction {
    config_tx::build_config_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        action,
    )
    .into()
}

/// Build the `multisigCreateV2` instruction. `program_config` and `treasury`
/// are PDAs of the program-level config account and the on-chain treasury;
/// resolve them from the deployed program before calling.
#[allow(clippy::too_many_arguments)]
pub fn multisig_create_v2(
    program_id: Pubkey,
    program_config: Pubkey,
    treasury: Pubkey,
    multisig: Pubkey,
    create_key: Pubkey,
    creator: Pubkey,
    threshold: u16,
    members: &[Pubkey],
    rent_collector: Option<Pubkey>,
) -> SolInstruction {
    multisig::build_multisig_create_v2_instruction(
        program_id,
        program_config,
        treasury,
        multisig,
        create_key,
        creator,
        threshold,
        members,
        rent_collector,
    )
    .into()
}

/// Build the `vaultTransactionAccountsClose` or `configTransactionAccountsClose`
/// instruction (selected by `kind`) for reclaiming rent on finalized transactions.
pub fn transaction_accounts_close(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    rent_collector: Pubkey,
    kind: rent::TxKind,
) -> SolInstruction {
    rent::build_transaction_accounts_close_instruction(
        program_id,
        multisig,
        proposal,
        transaction,
        rent_collector,
        kind,
    )
    .into()
}

/// Build the `vaultTransactionCreate` instruction with the program-upgrade
/// transaction message layout.
pub fn program_upgrade_vault_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
) -> SolInstruction {
    program_upgrade::build_program_upgrade_vault_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        vault_index,
        transaction_message,
    )
    .into()
}
