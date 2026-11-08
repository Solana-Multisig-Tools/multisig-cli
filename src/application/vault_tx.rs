use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::{
    pipeline::{
        execute_transaction, execute_transaction_quiet, serialize_vault_transaction_message,
        PreparedTransaction,
    },
    proposal::{build_proposal_activate_instruction, build_proposal_create_instruction},
};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const VAULT_TX_CREATE_DISC: [u8; 8] = [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3];

#[derive(Debug, Clone)]
pub struct VaultProposalResult {
    pub signature: Option<String>,
    pub transaction_index: u64,
    pub vault: Pubkey,
    pub transaction: Pubkey,
    pub proposal: Pubkey,
}

/// `ephemeral_signers` is the number of ephemeral signer PDAs the inner
/// message uses (`VaultTransactionCreateArgs.ephemeral_signers`); the Squads
/// program signs for them at execution.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_vault_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    ephemeral_signers: u8,
    transaction_message: &[u8],
    memo: Option<&str>,
) -> Result<Instruction, MsigError> {
    let message_len = u32::try_from(transaction_message.len()).map_err(|_| {
        MsigError::Transaction(format!(
            "vault transaction message is {} bytes; max supported is u32::MAX",
            transaction_message.len()
        ))
    })?;

    let mut data = Vec::new();
    data.extend_from_slice(&VAULT_TX_CREATE_DISC);
    data.push(vault_index);
    data.push(ephemeral_signers);
    data.extend_from_slice(&message_len.to_le_bytes());
    data.extend_from_slice(transaction_message);
    crate::infra::instruction::borsh_write_option_string(&mut data, memo)?;

    Ok(Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(multisig, false),
            AccountMeta::new(transaction, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    })
}

/// Ephemeral signer PDAs used by an inner message. They are derived from the
/// transaction PDA, so they are only valid for the transaction index they
/// were derived for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EphemeralSigners {
    pub count: u8,
    pub transaction_index: u64,
}

/// The index the next vault transaction of this multisig will get. Commands
/// that need ephemeral signer PDAs derive them from it before building their
/// inner instructions, then pass it back through [`EphemeralSigners`].
pub fn next_transaction_index(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
) -> Result<u64, MsigError> {
    let multisig_str = multisig_addr.to_string();
    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;
    Ok(ms.transaction_index.saturating_add(1))
}

/// Ephemeral signer PDA `index` of the vault transaction at `transaction_index`.
pub fn ephemeral_signer_address(
    multisig_addr: &Pubkey,
    transaction_index: u64,
    index: u8,
    program_id: &Pubkey,
) -> Pubkey {
    let (transaction, _) = pda::transaction_pda(multisig_addr, transaction_index, program_id);
    pda::ephemeral_signer_pda(&transaction, index, program_id).0
}

/// Refuse inner instructions that ask anything but the vault or one of the
/// declared ephemeral signer PDAs to sign.
fn check_inner_signers(
    inner_instructions: &[Instruction],
    vault: &Pubkey,
    allowed_ephemeral: &[Pubkey],
) -> Result<(), MsigError> {
    for ix in inner_instructions {
        for meta in ix.accounts.iter().filter(|m| m.is_signer) {
            if meta.pubkey != *vault && !allowed_ephemeral.contains(&meta.pubkey) {
                return Err(MsigError::Transaction(format!(
                    "inner instruction marks {} as signer, but only the vault {vault} and the \
                     transaction's ephemeral signers can sign",
                    meta.pubkey
                )));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn create_vault_transaction_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    inner_instructions: Vec<Instruction>,
    vault_index: u8,
    description: String,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<VaultProposalResult, MsigError> {
    create_vault_transaction_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        inner_instructions,
        vault_index,
        None,
        description,
        memo,
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

/// Like [`create_vault_transaction_proposal`], for inner instructions that
/// also need the transaction's ephemeral signer PDAs (indexes
/// `0..ephemeral.count`, derived for `ephemeral.transaction_index`). Refuses
/// if the multisig's next transaction index moved in the meantime, since the
/// PDAs in the message would then not be the ones the program signs for.
#[allow(clippy::too_many_arguments)]
pub fn create_vault_transaction_proposal_with_ephemeral_signers(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    inner_instructions: Vec<Instruction>,
    vault_index: u8,
    ephemeral: EphemeralSigners,
    description: String,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<VaultProposalResult, MsigError> {
    create_vault_transaction_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        inner_instructions,
        vault_index,
        Some(ephemeral),
        description,
        memo,
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_vault_transaction_proposal_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    inner_instructions: Vec<Instruction>,
    vault_index: u8,
    ephemeral: Option<EphemeralSigners>,
    description: String,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<VaultProposalResult, MsigError> {
    if inner_instructions.is_empty() {
        return Err(MsigError::Usage(
            "vault transaction requires at least one inner instruction".into(),
        ));
    }

    let program_id = config.program_id;
    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, &program_id);
    if let Some(ephemeral) = ephemeral {
        let allowed: Vec<Pubkey> = (0..ephemeral.count)
            .map(|index| {
                ephemeral_signer_address(
                    multisig_addr,
                    ephemeral.transaction_index,
                    index,
                    &program_id,
                )
            })
            .collect();
        check_inner_signers(&inner_instructions, &vault_pubkey, &allowed)?;
    }
    let transaction_message =
        serialize_vault_transaction_message(&inner_instructions, &vault_pubkey)?;

    create_vault_transaction_message_proposal_with_review(
        rpc,
        signer,
        multisig_addr,
        transaction_message,
        inner_instructions,
        vault_index,
        ephemeral,
        description,
        memo,
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_vault_transaction_message_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    transaction_message: Vec<u8>,
    vault_index: u8,
    description: String,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<VaultProposalResult, MsigError> {
    create_vault_transaction_message_proposal_with_review(
        rpc,
        signer,
        multisig_addr,
        transaction_message,
        vec![],
        vault_index,
        None,
        description,
        memo,
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_vault_transaction_message_proposal_with_review(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    transaction_message: Vec<u8>,
    review_instructions: Vec<Instruction>,
    vault_index: u8,
    ephemeral: Option<EphemeralSigners>,
    description: String,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<VaultProposalResult, MsigError> {
    if transaction_message.is_empty() {
        return Err(MsigError::Usage(
            "vault transaction message cannot be empty".into(),
        ));
    }

    let program_id = config.program_id;
    let multisig_str = multisig_addr.to_string();
    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    let signer_pubkey = signer.pubkey();
    let member = ms.members.iter().find(|m| m.key == signer_pubkey);
    match member {
        Some(m) if !m.permissions.can_initiate() => {
            return Err(MsigError::PermissionDenied(
                "signer does not have Initiate permission".into(),
            ))
        }
        None => {
            return Err(MsigError::PermissionDenied(
                "signer is not a member of this multisig".into(),
            ))
        }
        _ => {}
    }

    let next_index = ms.transaction_index.saturating_add(1);
    if let Some(ephemeral) = ephemeral {
        if ephemeral.transaction_index != next_index {
            return Err(MsigError::Transaction(format!(
                "the multisig's next transaction index moved from {} to {next_index} while the \
                 proposal was being prepared (another proposal was created). The ephemeral \
                 signer address would not match. Nothing was proposed; run the command again.",
                ephemeral.transaction_index
            )));
        }
    }
    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, &program_id);
    let (transaction_pubkey, _) = pda::transaction_pda(multisig_addr, next_index, &program_id);
    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, next_index, &program_id);
    let creator = signer.pubkey();

    let instructions = vec![
        build_vault_transaction_create_instruction(
            program_id,
            *multisig_addr,
            transaction_pubkey,
            creator,
            vault_index,
            ephemeral.map_or(0, |e| e.count),
            &transaction_message,
            memo,
        )?,
        build_proposal_create_instruction(
            program_id,
            *multisig_addr,
            proposal_pubkey,
            creator,
            next_index,
            true,
        ),
        build_proposal_activate_instruction(program_id, *multisig_addr, proposal_pubkey, creator),
    ];

    let prepared = PreparedTransaction {
        instructions,
        description,
        review_instructions,
        extra_signers: vec![],
    };

    let signature = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };

    if signature.is_some() && !quiet {
        println!(
            "Proposal #{} created. Approve: msig proposal approve {}",
            next_index, next_index
        );
    }

    Ok(VaultProposalResult {
        signature,
        transaction_index: next_index,
        vault: vault_pubkey,
        transaction: transaction_pubkey,
        proposal: proposal_pubkey,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::instruction::assert_memo_replaces_none_tail;

    #[test]
    fn vault_transaction_create_encodes_some_memo_at_tail() {
        let program_id = Pubkey::new_from_array([1u8; 32]);
        let multisig = Pubkey::new_from_array([2u8; 32]);
        let transaction = Pubkey::new_from_array([3u8; 32]);
        let creator = Pubkey::new_from_array([4u8; 32]);
        let message = b"\x00\x00\x00\x00\x00\x00\x00".to_vec(); // small dummy payload
        let memo = "vault transfer to treasury";

        let none = build_vault_transaction_create_instruction(
            program_id,
            multisig,
            transaction,
            creator,
            0,
            0,
            &message,
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let some = build_vault_transaction_create_instruction(
            program_id,
            multisig,
            transaction,
            creator,
            0,
            0,
            &message,
            Some(memo),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_memo_replaces_none_tail(&none.data, &some.data, memo);
    }

    #[test]
    fn vault_transaction_create_encodes_ephemeral_signer_count() {
        let key = |b| Pubkey::new_from_array([b; 32]);
        let build = |ephemeral| {
            build_vault_transaction_create_instruction(
                key(1),
                key(2),
                key(3),
                key(4),
                7,
                ephemeral,
                b"msg",
                None,
            )
            .unwrap_or_else(|e| panic!("{e}"))
        };
        // discriminator, vault_index, ephemeral_signers, message length...
        assert_eq!(build(0).data[8..10], [7, 0]);
        assert_eq!(build(1).data[8..10], [7, 1]);
        assert_eq!(build(0).data[10..], build(1).data[10..]);
    }

    #[test]
    fn ephemeral_signer_address_matches_pda_derivation() {
        let multisig = Pubkey::new_from_array([5u8; 32]);
        let program_id = pda::PROGRAM_ID;
        let (transaction, _) = pda::transaction_pda(&multisig, 42, &program_id);
        assert_eq!(
            ephemeral_signer_address(&multisig, 42, 0, &program_id),
            pda::ephemeral_signer_pda(&transaction, 0, &program_id).0
        );
        assert_ne!(
            ephemeral_signer_address(&multisig, 42, 0, &program_id),
            ephemeral_signer_address(&multisig, 43, 0, &program_id)
        );
    }

    #[test]
    fn inner_signers_limited_to_vault_and_declared_ephemerals() {
        let vault = Pubkey::new_from_array([1u8; 32]);
        let ephemeral = Pubkey::new_from_array([2u8; 32]);
        let stranger = Pubkey::new_from_array([3u8; 32]);
        let ix = |signer| Instruction {
            program_id: SYSTEM_PROGRAM,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(signer, true),
            ],
            data: vec![],
        };
        assert!(check_inner_signers(&[ix(ephemeral)], &vault, &[ephemeral]).is_ok());
        assert!(check_inner_signers(&[ix(stranger)], &vault, &[ephemeral]).is_err());
        assert!(check_inner_signers(&[ix(ephemeral)], &vault, &[]).is_err());
        // Non-signer accounts are not restricted.
        let readonly = Instruction {
            program_id: SYSTEM_PROGRAM,
            accounts: vec![AccountMeta::new_readonly(stranger, false)],
            data: vec![],
        };
        assert!(check_inner_signers(&[readonly], &vault, &[]).is_ok());
    }
}
