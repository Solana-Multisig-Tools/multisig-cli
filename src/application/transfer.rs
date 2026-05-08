use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;
use crate::output::format_sol;

use super::{
    pipeline::{
        execute_transaction, execute_transaction_quiet, serialize_vault_transaction_message,
        PreparedTransaction,
    },
    proposal::{build_proposal_activate_instruction, build_proposal_create_instruction},
};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const ATA_PROGRAM: Pubkey = solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

const VAULT_TX_CREATE_DISC: [u8; 8] = [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3];

pub(crate) fn build_vault_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    vault_index: u8,
    transaction_message: &[u8],
    memo: Option<&str>,
) -> Result<Instruction, MsigError> {
    let mut data = Vec::new();
    data.extend_from_slice(&VAULT_TX_CREATE_DISC);
    data.push(vault_index);
    data.push(0u8);
    data.extend_from_slice(&(transaction_message.len() as u32).to_le_bytes());
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
pub fn create_transfer_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    amount: u64,
    token_mint: &str,
    recipient: &str,
    vault_index: u8,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    create_transfer_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        amount,
        token_mint,
        recipient,
        vault_index,
        memo,
        config,
        dry_run,
        skip_confirm,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_transfer_proposal_quiet(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    amount: u64,
    token_mint: &str,
    recipient: &str,
    vault_index: u8,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    create_transfer_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        amount,
        token_mint,
        recipient,
        vault_index,
        memo,
        config,
        dry_run,
        skip_confirm,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_transfer_proposal_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    amount: u64,
    token_mint: &str,
    recipient: &str,
    vault_index: u8,
    memo: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let recipient_pubkey: Pubkey = recipient
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid recipient address: '{recipient}'")))?;

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

    let description = if token_mint == "native" {
        format!(
            "Transfer {} SOL from vault to {}",
            format_sol(amount),
            recipient
        )
    } else {
        format!(
            "Transfer {} tokens ({}) from vault to {}",
            amount, token_mint, recipient
        )
    };

    let inner_instructions = if token_mint == "native" {
        vec![build_sol_transfer_instruction(
            &vault_pubkey,
            &recipient_pubkey,
            amount,
        )]
    } else {
        let mint_pubkey: Pubkey = token_mint
            .parse()
            .map_err(|_| MsigError::Usage(format!("invalid mint address: '{token_mint}'")))?;
        let token_program = resolve_token_program(rpc, &mint_pubkey)?;
        let mut ixs = Vec::new();

        let dest_ata =
            pda::derive_ata_with_token_program(&recipient_pubkey, &mint_pubkey, &token_program);
        let dest_ata_str = dest_ata.to_string();
        let ata_exists = rpc.get_account_info(&dest_ata_str)?.is_some();

        if !ata_exists {
            let token_symbol = config
                .tokens
                .iter()
                .find(|(_, v)| v.as_str() == token_mint)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| token_mint.to_string());
            if !quiet {
                eprintln!("Recipient does not have a {token_symbol} account. The vault will create one (~0.002 SOL rent).");
            }
            ixs.push(build_create_ata_idempotent_instruction(
                &vault_pubkey,
                &recipient_pubkey,
                &mint_pubkey,
                &token_program,
            ));
        }

        ixs.push(build_spl_transfer_instruction(
            &vault_pubkey,
            &recipient_pubkey,
            &mint_pubkey,
            &token_program,
            amount,
        ));
        ixs
    };

    let mut instructions = Vec::new();

    let inner_msg = serialize_vault_transaction_message(&inner_instructions, &vault_pubkey)?;
    instructions.push(build_vault_transaction_create_instruction(
        program_id,
        *multisig_addr,
        transaction_pubkey,
        creator,
        vault_index,
        &inner_msg,
        memo,
    )?);

    instructions.push(build_proposal_create_instruction(
        program_id,
        *multisig_addr,
        proposal_pubkey,
        creator,
        next_index,
        true,
    ));

    instructions.push(build_proposal_activate_instruction(
        program_id,
        *multisig_addr,
        proposal_pubkey,
        creator,
    ));

    let prepared = PreparedTransaction {
        instructions,
        description,
        review_instructions: inner_instructions,
        extra_signers: vec![],
    };
    let result = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };

    if result.is_some() && !quiet {
        println!(
            "Proposal #{} created. Approve: msig proposal approve {}",
            next_index, next_index
        );
    }

    Ok(result)
}

fn build_sol_transfer_instruction(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

fn build_create_ata_idempotent_instruction(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let ata = pda::derive_ata_with_token_program(wallet, mint, token_program);
    Instruction {
        program_id: ATA_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1],
    }
}

fn build_spl_transfer_instruction(
    owner: &Pubkey,
    recipient: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
    amount: u64,
) -> Instruction {
    let source_ata = pda::derive_ata_with_token_program(owner, mint, token_program);
    let dest_ata = pda::derive_ata_with_token_program(recipient, mint, token_program);
    let mut data = Vec::with_capacity(9);
    data.push(3);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(source_ata, false),
            AccountMeta::new(dest_ata, false),
            AccountMeta::new_readonly(*owner, true),
        ],
        data,
    }
}

fn resolve_token_program(rpc: &dyn RpcProvider, mint: &Pubkey) -> Result<Pubkey, MsigError> {
    let mint_str = mint.to_string();
    let account = rpc
        .get_account_info(&mint_str)?
        .ok_or_else(|| MsigError::Transaction(format!("token mint {mint} not found")))?;
    match account.owner.as_str() {
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" => Ok(TOKEN_PROGRAM),
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => Ok(TOKEN_2022_PROGRAM),
        owner => Err(MsigError::Transaction(format!(
            "mint {mint} is owned by {owner}, not the SPL Token or Token-2022 program"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::instruction::assert_memo_replaces_none_tail;

    /// `transfer.rs` carries its own `build_vault_transaction_create_instruction`
    /// duplicate of the one in `vault_tx.rs`. Cover it independently so the
    /// memo plumbing can't drift between the two.
    #[test]
    fn transfer_vault_transaction_create_encodes_some_memo_at_tail() {
        let program_id = Pubkey::new_from_array([1u8; 32]);
        let multisig = Pubkey::new_from_array([2u8; 32]);
        let transaction = Pubkey::new_from_array([3u8; 32]);
        let creator = Pubkey::new_from_array([4u8; 32]);
        let message = vec![0u8; 16];
        let memo = "send 1 SOL to ops vault";

        let none = build_vault_transaction_create_instruction(
            program_id,
            multisig,
            transaction,
            creator,
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
            &message,
            Some(memo),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_memo_replaces_none_tail(&none.data, &some.data, memo);
    }
}
