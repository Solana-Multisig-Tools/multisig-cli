//! Public SDK surface for building Squads multisig v4 instructions.
//!
//! Enabled via the `instruction-builder` feature. When enabled, this module
//! re-exports the canonical Squads v4 data types and exposes thin wrappers
//! around the internal instruction builders that return
//! [`solana_instruction::Instruction`] directly, so downstream client SDKs can
//! produce v4 instructions byte-for-byte identical to those produced by this
//! CLI without copying the implementation.
//!
//! In addition to instruction builders, this module re-exports
//! [`Multisig`] and its [`Multisig::parse`] constructor so that
//! a client can fetch the raw on-chain account data via its own RPC provider
//! and deserialize the bytes into the canonical typed account struct.
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

use crate::{
    application::{config_tx, multisig, program_upgrade, proposal, rent, vault_tx},
    error::MsigError,
    infra::instruction::{AccountMeta, Instruction as InternalInstruction},
};

pub use crate::{
    application::{config_tx::ConfigTransactionAction, rent::TxKind},
    domain::{
        multisig::{Member, MultisigInfo, Permissions},
        proposal::{ProposalDetail, ProposalStatus, ProposalSummary, TransactionType, Vote},
        transaction::{
            AccountRef, ConfigAction, ConfigTxInfo, InstructionSummary, SpendingLimitPeriod,
            VaultTxInfo,
        },
    },
    error::ParseError,
    infra::{
        accounts::{
            multisig::MultisigAccount as Multisig,
            vault_tx::TransactionMessage as VaultTransactionMessage,
        },
        pda::{
            batch_transaction_pda, ephemeral_signer_pda, multisig_pda, program_config_pda,
            proposal_pda, spending_limit_pda, transaction_buffer_pda, transaction_pda, vault_pda,
            PROGRAM_ID,
        },
    },
};

/// Build the `proposalVote` instruction for a member casting `vote` on `proposal`.
///
/// `memo` is an optional UTF-8 string stored on-chain as the `ProposalVoteArgs.memo`
/// field; pass `None` to omit it (the default for v4 SDK callers).
pub fn vote(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    member: Pubkey,
    vote: Vote,
    memo: Option<&str>,
) -> Result<SolInstruction, MsigError> {
    proposal::build_vote_instruction(program_id, multisig, proposal, member, vote, memo)
        .map(Into::into)
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

/// Serialize inner instructions into the wire-format `transaction_message`
/// bytes expected by [`vault_transaction_create`].
///
/// The bytes use the v4 program's `SmallVec<L, T>` length prefixes (u8 for
/// vector counts, u16 for instruction data). The first account in the
/// resulting message is always `vault` as a writable signer; remaining
/// accounts are deduplicated and ordered (signers-then-non-signers,
/// writables-first within each group) per Solana's canonical message layout.
pub fn serialize_vault_transaction_message(
    instructions: &[SolInstruction],
    vault: &Pubkey,
) -> Result<Vec<u8>, MsigError> {
    let internal: Vec<InternalInstruction> = instructions.iter().cloned().map(Into::into).collect();
    crate::application::pipeline::serialize_vault_transaction_message(&internal, vault)
}

/// Build the `vaultTransactionCreate` instruction. `transaction_message` is the
/// pre-serialized inner message bytes (see the v4 program's `TransactionMessage`
/// layout); use [`serialize_vault_transaction_message`] to produce them from
/// `solana_instruction::Instruction` values.
///
/// `memo` is an optional UTF-8 string stored on-chain as the
/// `VaultTransactionCreateArgs.memo` field.
#[allow(clippy::too_many_arguments)]
pub fn vault_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
    memo: Option<&str>,
) -> Result<SolInstruction, MsigError> {
    vault_tx::build_vault_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        vault_index,
        transaction_message,
        memo,
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
    proposal::build_proposal_create_instruction(
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
    proposal::build_proposal_activate_instruction(program_id, multisig, proposal, creator).into()
}

/// Build the `configTransactionCreate` instruction.
///
/// `memo` is an optional UTF-8 string stored on-chain as the
/// `ConfigTransactionCreateArgs.memo` field.
#[allow(clippy::too_many_arguments)]
pub fn config_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    action: config_tx::ConfigTransactionAction,
    memo: Option<&str>,
) -> Result<SolInstruction, MsigError> {
    config_tx::build_config_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        action,
        memo,
    )
    .map(Into::into)
}

/// Build the `multisigCreateV2` instruction. `program_config` and `treasury`
/// are PDAs of the program-level config account and the on-chain treasury;
/// resolve them from the deployed program before calling.
///
/// `memo` is an optional UTF-8 string stored on-chain as the
/// `MultisigCreateArgsV2.memo` field.
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
    memo: Option<&str>,
) -> Result<SolInstruction, MsigError> {
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
        memo,
    )
    .map(Into::into)
}

/// Build the `vaultTransactionAccountsClose` or `configTransactionAccountsClose`
/// instruction (selected by `kind`) for reclaiming rent on finalized transactions.
pub fn transaction_accounts_close(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    rent_collector: Pubkey,
    kind: TxKind,
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

/// Anchor-v4-SDK-style alias for [`Multisig::parse`]: decode raw on-chain
/// account bytes (e.g. from an `RpcClient::get_account(...)?.data` call) into
/// a [`Multisig`], verifying the 8-byte account discriminator and borsh-decoding
/// the body in one step.
///
/// The original v4 SDK exposes the same operation through Anchor's
/// `AccountDeserialize::try_deserialize(&mut &[u8])` trait method. We take the
/// slice by value because v4 is frozen and the account data has no trailing
/// bytes for a cursor to leave behind, so callers don't need to track how much
/// of the buffer was consumed.
pub fn try_deserialize(data: &[u8]) -> Result<Multisig, ParseError> {
    Multisig::parse(data)
}

/// Build the `vaultTransactionCreate` instruction with the program-upgrade
/// transaction message layout.
///
/// `memo` is an optional UTF-8 string stored on-chain as the
/// `VaultTransactionCreateArgs.memo` field.
#[allow(clippy::too_many_arguments)]
pub fn program_upgrade_vault_transaction_create(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
    memo: Option<&str>,
) -> Result<SolInstruction, MsigError> {
    program_upgrade::build_program_upgrade_vault_transaction_create_instruction(
        program_id,
        multisig,
        transaction,
        creator,
        vault_index,
        transaction_message,
        memo,
    )
    .map(Into::into)
}
