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

pub(crate) fn build_vault_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
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
    data.push(0u8);
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
    if inner_instructions.is_empty() {
        return Err(MsigError::Usage(
            "vault transaction requires at least one inner instruction".into(),
        ));
    }

    let program_id = config.program_id;
    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, &program_id);
    let transaction_message =
        serialize_vault_transaction_message(&inner_instructions, &vault_pubkey)?;

    create_vault_transaction_message_proposal_with_review(
        rpc,
        signer,
        multisig_addr,
        transaction_message,
        inner_instructions,
        vault_index,
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
            program_id, multisig, transaction, creator, 0, &message, None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let some = build_vault_transaction_create_instruction(
            program_id,
            multisig,
            transaction,
            creator,
            0,
            &message,
            Some(memo),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_memo_replaces_none_tail(&none.data, &some.data, memo);
    }
}
