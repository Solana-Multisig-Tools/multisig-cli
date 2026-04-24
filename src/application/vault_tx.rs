use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::pipeline::{
    execute_transaction, execute_transaction_quiet, serialize_vault_transaction_message,
    PreparedTransaction,
};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const VAULT_TX_CREATE_DISC: [u8; 8] = [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3];
const PROPOSAL_CREATE_DISC: [u8; 8] = [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f];
const PROPOSAL_ACTIVATE_DISC: [u8; 8] = [0x0b, 0x22, 0x5c, 0xf8, 0x9a, 0x1b, 0x33, 0x6a];

#[derive(Debug, Clone)]
pub struct VaultProposalResult {
    pub signature: Option<String>,
    pub transaction_index: u64,
    pub vault: Pubkey,
    pub transaction: Pubkey,
    pub proposal: Pubkey,
}

fn build_vault_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
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
    data.push(0x00);

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

fn build_proposal_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    creator: Pubkey,
    transaction_index: u64,
    draft: bool,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&PROPOSAL_CREATE_DISC);
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.push(u8::from(draft));

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(multisig, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

fn build_proposal_activate_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    creator: Pubkey,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&PROPOSAL_ACTIVATE_DISC);

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(multisig, false),
            AccountMeta::new(creator, true),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_vault_transaction_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    inner_instructions: Vec<Instruction>,
    vault_index: u8,
    description: String,
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
