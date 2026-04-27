use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::pipeline::{
    execute_transaction, serialize_vault_transaction_message, PreparedTransaction,
};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const BPF_LOADER_UPGRADEABLE: Pubkey =
    solana_pubkey::pubkey!("BPFLoaderUpgradeab1e11111111111111111111111");
const RENT_SYSVAR: Pubkey = solana_pubkey::pubkey!("SysvarRent111111111111111111111111111111111");
const CLOCK_SYSVAR: Pubkey = solana_pubkey::pubkey!("SysvarC1ock11111111111111111111111111111111");

const VAULT_TX_CREATE_DISC: [u8; 8] = [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3];
const PROPOSAL_CREATE_DISC: [u8; 8] = [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f];
const PROPOSAL_ACTIVATE_DISC: [u8; 8] = [0x0b, 0x22, 0x5c, 0xf8, 0x9a, 0x1b, 0x33, 0x6a];

fn derive_program_data(program_id: &Pubkey) -> Pubkey {
    let (pda, _) = pda::find_program_address(&[program_id.as_ref()], &BPF_LOADER_UPGRADEABLE);
    pda
}

fn build_bpf_upgrade_instruction(
    program: Pubkey,
    buffer: Pubkey,
    spill: Pubkey,
    authority: Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&3u32.to_le_bytes());

    Instruction {
        program_id: BPF_LOADER_UPGRADEABLE,
        accounts: vec![
            AccountMeta::new(derive_program_data(&program), false),
            AccountMeta::new(program, false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(spill, false),
            AccountMeta::new_readonly(RENT_SYSVAR, false),
            AccountMeta::new_readonly(CLOCK_SYSVAR, false),
            AccountMeta::new_readonly(authority, true),
        ],
        data,
    }
}

pub fn build_program_upgrade_vault_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&VAULT_TX_CREATE_DISC);
    data.push(vault_index);
    data.push(0u8);
    data.extend_from_slice(&(transaction_message.len() as u32).to_le_bytes());
    data.extend_from_slice(transaction_message);
    data.push(0x00);

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(multisig, false),
            AccountMeta::new(transaction, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

pub fn build_program_upgrade_proposal_create_instruction(
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

pub fn build_program_upgrade_proposal_activate_instruction(
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
pub fn create_program_upgrade_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    program_addr: &str,
    buffer_addr: &str,
    spill_addr: &str,
    vault_index: u8,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    let program_pubkey: Pubkey = program_addr
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid program address: '{program_addr}'")))?;
    let buffer_pubkey: Pubkey = buffer_addr
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid buffer address: '{buffer_addr}'")))?;
    let spill_pubkey: Pubkey = spill_addr
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid spill address: '{spill_addr}'")))?;

    let squads_program_id = config.program_id;
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
    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, &squads_program_id);
    let (transaction_pubkey, _) =
        pda::transaction_pda(multisig_addr, next_index, &squads_program_id);
    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, next_index, &squads_program_id);
    let creator = signer.pubkey();
    let inner_ix =
        build_bpf_upgrade_instruction(program_pubkey, buffer_pubkey, spill_pubkey, vault_pubkey);

    let mut instructions = Vec::new();

    let inner_msg = serialize_vault_transaction_message(&[inner_ix], &vault_pubkey)?;
    instructions.push(build_program_upgrade_vault_transaction_create_instruction(
        squads_program_id,
        *multisig_addr,
        transaction_pubkey,
        creator,
        vault_index,
        &inner_msg,
    ));

    instructions.push(build_program_upgrade_proposal_create_instruction(
        squads_program_id,
        *multisig_addr,
        proposal_pubkey,
        creator,
        next_index,
        true,
    ));

    instructions.push(build_program_upgrade_proposal_activate_instruction(
        squads_program_id,
        *multisig_addr,
        proposal_pubkey,
        creator,
    ));

    let prepared = PreparedTransaction {
        instructions,
        description: format!(
            "Upgrade program {} using buffer {}",
            program_addr, buffer_addr
        ),
        review_instructions: vec![],
        extra_signers: vec![],
    };

    let result = execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?;
    if result.is_some() {
        println!(
            "Proposal #{} created for program upgrade. Approve: msig proposal approve {}",
            next_index, next_index
        );
    }
    Ok(result)
}
