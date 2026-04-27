use solana_pubkey::Pubkey;

use crate::domain::proposal::Vote;
use crate::domain::transaction::ConfigAction;
use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::accounts::proposal::ProposalAccount;
use crate::infra::accounts::vault_tx::{
    TransactionMessage as VaultTransactionMessage, VaultTransactionAccount,
};
use crate::infra::address_lookup_table;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::pipeline::{execute_transaction, execute_transaction_quiet, PreparedTransaction};

const VAULT_TX_EXECUTE_DISC: [u8; 8] = [0xc2, 0x08, 0xa1, 0x57, 0x99, 0xa4, 0x19, 0xab];
const CONFIG_TX_EXECUTE_DISC: [u8; 8] = [0x72, 0x92, 0xf4, 0xbd, 0xfc, 0x8c, 0x24, 0x28];
const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");

#[derive(Debug, Clone)]
pub struct ExecuteProposalPlan {
    pub instruction: Instruction,
    pub watched_accounts: Vec<Pubkey>,
}

pub fn build_vote_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    member: Pubkey,
    vote: Vote,
) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&vote.discriminator());
    data.push(0x00);

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(multisig, false),
            AccountMeta::new(member, true),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_vault_transaction_execute_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    message: &VaultTransactionMessage,
    lookup_table_accounts: &[Pubkey],
    loaded_lookup_accounts: &[AccountMeta],
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new_readonly(multisig, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(transaction, false),
        AccountMeta::new_readonly(executor, true),
    ];

    for lookup_table in lookup_table_accounts {
        accounts.push(AccountMeta::new_readonly(*lookup_table, false));
    }

    let num_signers = message.num_signers as usize;
    let num_writable_signers = message.num_writable_signers as usize;
    let num_writable_non_signers = message.num_writable_non_signers as usize;

    for (i, key_bytes) in message.account_keys.iter().enumerate() {
        let key = Pubkey::from(*key_bytes);
        let is_writable = if i < num_signers {
            i < num_writable_signers
        } else {
            (i - num_signers) < num_writable_non_signers
        };
        if is_writable {
            accounts.push(AccountMeta::new(key, false));
        } else {
            accounts.push(AccountMeta::new_readonly(key, false));
        }
    }

    accounts.extend_from_slice(loaded_lookup_accounts);

    Instruction {
        program_id,
        accounts,
        data: VAULT_TX_EXECUTE_DISC.to_vec(),
    }
}

pub fn build_config_transaction_execute_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    actions: &[ConfigAction],
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(multisig, false),
        AccountMeta::new_readonly(executor, true),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(transaction, false),
        AccountMeta::new(executor, true),
        AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
    ];

    for action in actions {
        match action {
            ConfigAction::AddSpendingLimit { create_key, .. } => {
                let (spending_limit, _) =
                    pda::spending_limit_pda(&multisig, create_key, &program_id);
                accounts.push(AccountMeta::new(spending_limit, false));
            }
            ConfigAction::RemoveSpendingLimit { spending_limit } => {
                accounts.push(AccountMeta::new(*spending_limit, false));
            }
            _ => {}
        }
    }

    Instruction {
        program_id,
        accounts,
        data: CONFIG_TX_EXECUTE_DISC.to_vec(),
    }
}

pub fn prepare_execute_proposal(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    index: u64,
    executor: Pubkey,
    config: &Config,
) -> Result<ExecuteProposalPlan, MsigError> {
    let program_id = config.program_id;
    let program_id_str = program_id.to_string();
    let multisig_str = multisig_addr.to_string();

    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, index, &program_id);
    let prop_addr = proposal_pubkey.to_string();
    let (tx_pubkey, _) = pda::transaction_pda(multisig_addr, index, &program_id);
    let tx_addr = tx_pubkey.to_string();

    let batch = rpc.get_multiple_accounts(&[&multisig_str, &prop_addr, &tx_addr])?;
    let mut batch_iter = batch.into_iter();

    let ms_account = batch_iter
        .next()
        .flatten()
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    if ms_account.owner != program_id_str {
        return Err(MsigError::Transaction(format!(
            "account {} not owned by Squads program",
            multisig_str
        )));
    }
    let ms = MultisigAccount::parse(&ms_account.data)?;

    let prop_account = batch_iter
        .next()
        .flatten()
        .ok_or(MsigError::ProposalNotFound(index))?;
    let prop = ProposalAccount::parse(&prop_account.data)?;

    if !prop.status.is_approved() {
        return Err(MsigError::Transaction(format!(
            "proposal #{index} is '{}', must be 'Approved' to execute",
            prop.status.label()
        )));
    }

    let member = ms.members.iter().find(|m| m.key == executor);
    match member {
        Some(m) if !m.permissions.can_execute() => {
            return Err(MsigError::PermissionDenied(
                "signer does not have Execute permission".into(),
            ))
        }
        None => {
            return Err(MsigError::PermissionDenied(
                "signer is not a member of this multisig".into(),
            ))
        }
        _ => {}
    }

    let tx_account = batch_iter
        .next()
        .flatten()
        .ok_or(MsigError::Transaction(format!(
            "transaction account not found for proposal #{index}"
        )))?;

    let instruction = if let Ok(vtx) = VaultTransactionAccount::parse(&tx_account.data) {
        let (lookup_table_accounts, loaded_lookup_accounts) =
            resolve_message_lookup_accounts(rpc, &vtx.message)?;
        build_vault_transaction_execute_instruction(
            program_id,
            *multisig_addr,
            proposal_pubkey,
            tx_pubkey,
            executor,
            &vtx.message,
            &lookup_table_accounts,
            &loaded_lookup_accounts,
        )
    } else if let Ok(ctx) =
        crate::infra::accounts::config_tx::ConfigTransactionAccount::parse(&tx_account.data)
    {
        build_config_transaction_execute_instruction(
            program_id,
            *multisig_addr,
            proposal_pubkey,
            tx_pubkey,
            executor,
            &ctx.actions,
        )
    } else {
        return Err(MsigError::Transaction(format!(
            "transaction account for proposal #{index} is neither a vault nor config transaction"
        )));
    };

    let watched_accounts = unique_pubkeys(instruction.accounts.iter().map(|meta| meta.pubkey));
    Ok(ExecuteProposalPlan {
        instruction,
        watched_accounts,
    })
}

fn resolve_message_lookup_accounts(
    rpc: &dyn RpcProvider,
    message: &VaultTransactionMessage,
) -> Result<(Vec<Pubkey>, Vec<AccountMeta>), MsigError> {
    if message.address_table_lookups.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let lookup_table_accounts: Vec<Pubkey> = message
        .address_table_lookups
        .iter()
        .map(|lookup| Pubkey::from(lookup.account_key))
        .collect();
    let lookup_table_strings: Vec<String> = lookup_table_accounts
        .iter()
        .map(ToString::to_string)
        .collect();
    let lookup_table_refs: Vec<&str> = lookup_table_strings.iter().map(String::as_str).collect();
    let account_data = rpc.get_multiple_accounts(&lookup_table_refs)?;

    let mut loaded_accounts = Vec::new();
    for (lookup_idx, lookup) in message.address_table_lookups.iter().enumerate() {
        let lookup_table = lookup_table_accounts[lookup_idx];
        let account = account_data
            .get(lookup_idx)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                MsigError::Transaction(format!("address lookup table {lookup_table} not found"))
            })?;
        let addresses = address_lookup_table::parse_addresses(&lookup_table, account)?;

        for index in &lookup.writable_indexes {
            let address = addresses.get(usize::from(*index)).copied().ok_or_else(|| {
                MsigError::Transaction(format!(
                    "address lookup table {lookup_table} does not contain writable index {index}"
                ))
            })?;
            loaded_accounts.push(AccountMeta::new(address, false));
        }
        for index in &lookup.readonly_indexes {
            let address = addresses.get(usize::from(*index)).copied().ok_or_else(|| {
                MsigError::Transaction(format!(
                    "address lookup table {lookup_table} does not contain readonly index {index}"
                ))
            })?;
            loaded_accounts.push(AccountMeta::new_readonly(address, false));
        }
    }

    Ok((lookup_table_accounts, loaded_accounts))
}

fn unique_pubkeys(keys: impl IntoIterator<Item = Pubkey>) -> Vec<Pubkey> {
    let mut out = Vec::new();
    for key in keys {
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn create_vote_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    vote: Vote,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    create_vote_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        vote,
        config,
        dry_run,
        skip_confirm,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_vote_proposal_quiet(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    vote: Vote,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    create_vote_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        vote,
        config,
        dry_run,
        skip_confirm,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_vote_proposal_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    vote: Vote,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let program_id = config.program_id;
    let program_id_str = program_id.to_string();
    let multisig_str = multisig_addr.to_string();

    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, index, &program_id);
    let prop_addr = proposal_pubkey.to_string();

    let batch = rpc.get_multiple_accounts(&[&multisig_str, &prop_addr])?;
    let mut batch_iter = batch.into_iter();

    let ms_account = batch_iter
        .next()
        .flatten()
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    if ms_account.owner != program_id_str {
        return Err(MsigError::Transaction(format!(
            "account {} not owned by Squads program (owner: {})",
            multisig_str, ms_account.owner
        )));
    }
    let ms = MultisigAccount::parse(&ms_account.data)?;

    let prop_account = batch_iter
        .next()
        .flatten()
        .ok_or(MsigError::ProposalNotFound(index))?;
    if prop_account.owner != program_id_str {
        return Err(MsigError::Transaction(format!(
            "proposal account {} not owned by Squads program",
            prop_addr
        )));
    }
    let prop = ProposalAccount::parse(&prop_account.data)?;

    if !prop.status.is_active() {
        return Err(MsigError::Transaction(format!(
            "proposal #{index} is '{}', must be 'Active' to vote",
            prop.status.label()
        )));
    }

    let signer_pubkey = signer.pubkey();
    let member = ms.members.iter().find(|m| m.key == signer_pubkey);
    match member {
        Some(m) if !m.permissions.can_vote() => {
            return Err(MsigError::PermissionDenied(
                "signer does not have Vote permission".into(),
            ))
        }
        None => {
            return Err(MsigError::PermissionDenied(
                "signer is not a member of this multisig".into(),
            ))
        }
        _ => {}
    }

    if vote == Vote::Approve && prop.approved.contains(&signer_pubkey) {
        return Err(MsigError::Transaction(format!(
            "you have already approved proposal #{index}"
        )));
    }
    if vote == Vote::Reject && prop.rejected.contains(&signer_pubkey) {
        return Err(MsigError::Transaction(format!(
            "you have already rejected proposal #{index}"
        )));
    }

    let instruction = build_vote_instruction(
        program_id,
        *multisig_addr,
        proposal_pubkey,
        signer_pubkey,
        vote,
    );
    let prepared = PreparedTransaction {
        instructions: vec![instruction],
        description: format!("{} proposal #{index}", vote.label()),
        review_instructions: vec![],
        extra_signers: vec![],
    };

    let result = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };

    if result.is_some() && !quiet {
        match vote {
            Vote::Approve => {
                let new_count = prop.approved.len() + 1;
                if new_count >= ms.threshold as usize {
                    println!("Proposal #{index} is now approved ({}/{}). Execute: msig proposal execute {index}", new_count, ms.threshold);
                } else {
                    println!(
                        "Voted to approve proposal #{index} ({}/{})",
                        new_count, ms.threshold
                    );
                }
            }
            Vote::Reject => println!("Voted to reject proposal #{index}"),
            Vote::Cancel => println!("Cancelled proposal #{index}"),
        }
    }

    Ok(result)
}

pub fn execute_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        config,
        dry_run,
        skip_confirm,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_proposal_quiet(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        config,
        dry_run,
        skip_confirm,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_proposal_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let plan = prepare_execute_proposal(rpc, multisig_addr, index, signer.pubkey(), config)?;
    let prepared = PreparedTransaction {
        instructions: vec![plan.instruction],
        description: format!("Execute proposal #{index}"),
        review_instructions: vec![],
        extra_signers: vec![],
    };

    let result = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };
    if result.is_some() && !quiet {
        println!("Proposal #{index} executed successfully.");
    }
    Ok(result)
}
