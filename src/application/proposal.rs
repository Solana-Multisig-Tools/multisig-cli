use solana_pubkey::Pubkey;
use zeroize::Zeroizing;

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

use super::pipeline::{
    execute_transaction, execute_transaction_quiet, extra_signer_pubkey, order_extra_signers,
    PreparedTransaction,
};

const VAULT_TX_EXECUTE_DISC: [u8; 8] = [0xc2, 0x08, 0xa1, 0x57, 0x99, 0xa4, 0x19, 0xab];
const CONFIG_TX_EXECUTE_DISC: [u8; 8] = [0x72, 0x92, 0xf4, 0xbd, 0xfc, 0x8c, 0x24, 0x28];
const PROPOSAL_CREATE_DISC: [u8; 8] = [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f];
const PROPOSAL_ACTIVATE_DISC: [u8; 8] = [0x0b, 0x22, 0x5c, 0xf8, 0x9a, 0x1b, 0x33, 0x6a];
const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");

#[derive(Debug, Clone)]
pub struct ExecuteProposalPlan {
    pub instruction: Instruction,
    pub watched_accounts: Vec<Pubkey>,
    /// Inner-message signers marked as signers of the execute instruction,
    /// in message order. Empty unless the vault message needs external signers.
    pub external_signers: Vec<Pubkey>,
    /// Decoded inner instructions, shown in the signing preview when the
    /// execution is co-signed by external signers.
    pub review_instructions: Vec<Instruction>,
}

/// How `vault_transaction_execute` treats inner-message signers that are
/// neither the transaction's vault nor one of its ephemeral signer PDAs.
///
/// Such signers must sign the outer transaction. They are never marked as
/// signers just because the stored message says so: a key is marked only when
/// the operator explicitly supplied it, and never when it is the fee payer,
/// whose single signature would otherwise silently authorize the inner
/// instruction (`build_message` merges per-key signer flags).
#[derive(Debug, Clone, Copy)]
pub enum ExternalSignerMode<'a> {
    /// Real execution: every required external signer must be in `keys`, and
    /// every key must be required. `flag` names the CLI flag in errors.
    Supplied {
        keys: &'a [Pubkey],
        flag: &'static str,
    },
    /// Simulation without signature verification: mark every required
    /// external signer so the simulation runs as the real execution would.
    Simulate,
}

/// Inner-message signers of a vault transaction that are neither its vault
/// (at the transaction's vault index) nor one of its ephemeral signer PDAs, in
/// message order. These must sign the execute transaction themselves.
pub fn required_external_signers(
    vtx: &VaultTransactionAccount,
    multisig: &Pubkey,
    transaction: &Pubkey,
    program_id: &Pubkey,
) -> Vec<Pubkey> {
    let (vault, _) = pda::vault_pda(multisig, vtx.vault_index, program_id);
    let ephemeral: Vec<Pubkey> = (0..=u8::MAX)
        .take(vtx.ephemeral_signer_bumps.len())
        .map(|index| pda::ephemeral_signer_pda(transaction, index, program_id).0)
        .collect();
    let num_signers = usize::from(vtx.message.num_signers);
    let mut out = Vec::new();
    for key_bytes in vtx.message.account_keys.iter().take(num_signers) {
        let key = Pubkey::from(*key_bytes);
        if key != vault && !ephemeral.contains(&key) && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Decide which required external signers to mark as signers of the execute
/// instruction. Refuses a required signer that is the fee payer, a required
/// signer that was not supplied, and a supplied key that is not required.
pub fn select_external_signers(
    required: &[Pubkey],
    fee_payer: &Pubkey,
    mode: ExternalSignerMode<'_>,
) -> Result<Vec<Pubkey>, MsigError> {
    if required.contains(fee_payer) {
        return Err(MsigError::Transaction(format!(
            "the vault transaction message requires {fee_payer}, the executor and fee payer of \
             this transaction, to sign one of its inner instructions. msig refuses to let the \
             fee payer signature double as an inner-instruction signature. Review the proposal \
             with `msig proposal show <INDEX> --verbose`; to execute it anyway, use a different \
             member as executor and supply this key explicitly as an extra signer."
        )));
    }
    let (keys, flag) = match mode {
        ExternalSignerMode::Simulate => return Ok(required.to_vec()),
        ExternalSignerMode::Supplied { keys, flag } => (keys, flag),
    };
    for (i, key) in keys.iter().enumerate() {
        if keys[..i].contains(key) {
            return Err(MsigError::Usage(format!(
                "{key} was passed twice with {flag}"
            )));
        }
        if !required.contains(key) {
            return Err(MsigError::Usage(format!(
                "{key} was passed with {flag} but is not a required signer of this proposal's \
                 vault message; refusing to add a signature that is not needed"
            )));
        }
    }
    if let Some(missing) = required.iter().find(|key| !keys.contains(key)) {
        return Err(MsigError::Usage(format!(
            "this proposal's vault message requires {missing} to sign at execution. Pass \
             {flag} <KEYPAIR_FILE> (the keypair file of that key) for it; nothing was signed."
        )));
    }
    Ok(required.to_vec())
}

pub(crate) fn build_proposal_create_instruction(
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

pub(crate) fn build_proposal_activate_instruction(
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

pub(crate) fn build_vote_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    member: Pubkey,
    vote: Vote,
    memo: Option<&str>,
) -> Result<Instruction, MsigError> {
    let mut data = Vec::with_capacity(9 + memo.map_or(0, |m| m.len() + 4));
    data.extend_from_slice(&vote.discriminator());
    crate::infra::instruction::borsh_write_option_string(&mut data, memo)?;

    Ok(Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(multisig, false),
            AccountMeta::new(member, true),
            AccountMeta::new(proposal, false),
        ],
        data,
    })
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
    build_vault_transaction_execute_instruction_with_signers(
        program_id,
        multisig,
        proposal,
        transaction,
        executor,
        message,
        lookup_table_accounts,
        loaded_lookup_accounts,
        &[],
    )
}

/// Same as [`build_vault_transaction_execute_instruction`], but marks the
/// inner-message signer keys listed in `external_signers` as signers. Callers
/// get that list from [`select_external_signers`]; no other key is marked.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_vault_transaction_execute_instruction_with_signers(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    message: &VaultTransactionMessage,
    lookup_table_accounts: &[Pubkey],
    loaded_lookup_accounts: &[AccountMeta],
    external_signers: &[Pubkey],
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
        let is_signer = i < num_signers && external_signers.contains(&key);
        if is_writable {
            accounts.push(AccountMeta::new(key, is_signer));
        } else {
            accounts.push(AccountMeta::new_readonly(key, is_signer));
        }
    }

    accounts.extend_from_slice(loaded_lookup_accounts);

    Instruction {
        program_id,
        accounts,
        data: VAULT_TX_EXECUTE_DISC.to_vec(),
    }
}

/// The execute instruction for a vault transaction, plus the external signers
/// it marks and, when there are any, the decoded inner instructions to review.
#[derive(Debug, Clone)]
pub(crate) struct VaultExecutePlan {
    pub instruction: Instruction,
    pub external_signers: Vec<Pubkey>,
    pub review_instructions: Vec<Instruction>,
}

/// Build `vault_transaction_execute` for a parsed vault transaction: resolve
/// its address lookup tables and apply the external signer rules of `mode`.
/// Shared by `proposal execute`, `proposal simulate` and `tx export`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_vault_transaction_execute(
    rpc: &dyn RpcProvider,
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    executor: Pubkey,
    vtx: &VaultTransactionAccount,
    mode: ExternalSignerMode<'_>,
) -> Result<VaultExecutePlan, MsigError> {
    let required = required_external_signers(vtx, &multisig, &transaction, &program_id);
    let external_signers = select_external_signers(&required, &executor, mode)?;
    let (lookup_table_accounts, loaded_lookup_accounts) =
        resolve_message_lookup_accounts(rpc, &vtx.message)?;
    let instruction = build_vault_transaction_execute_instruction_with_signers(
        program_id,
        multisig,
        proposal,
        transaction,
        executor,
        &vtx.message,
        &lookup_table_accounts,
        &loaded_lookup_accounts,
        &external_signers,
    );
    let review_instructions = if external_signers.is_empty() {
        Vec::new()
    } else {
        inner_instructions(&vtx.message, &loaded_lookup_accounts)
    };
    Ok(VaultExecutePlan {
        instruction,
        external_signers,
        review_instructions,
    })
}

/// Reconstruct the inner instructions of a vault message for review. Lookup
/// table accounts follow the static keys as the Squads program indexes them:
/// every loaded writable account, then every loaded read-only account.
/// Unresolvable indexes map to the default pubkey and show up as such.
pub(crate) fn inner_instructions(
    message: &VaultTransactionMessage,
    loaded_lookup_accounts: &[AccountMeta],
) -> Vec<Instruction> {
    let num_signers = usize::from(message.num_signers);
    let num_writable_signers = usize::from(message.num_writable_signers);
    let num_writable_non_signers = usize::from(message.num_writable_non_signers);
    let mut keys: Vec<AccountMeta> = message
        .account_keys
        .iter()
        .enumerate()
        .map(|(i, key_bytes)| {
            let key = Pubkey::from(*key_bytes);
            let is_signer = i < num_signers;
            let is_writable = if is_signer {
                i < num_writable_signers
            } else {
                (i - num_signers) < num_writable_non_signers
            };
            if is_writable {
                AccountMeta::new(key, is_signer)
            } else {
                AccountMeta::new_readonly(key, is_signer)
            }
        })
        .collect();
    keys.extend(
        loaded_lookup_accounts
            .iter()
            .filter(|m| m.is_writable)
            .cloned(),
    );
    keys.extend(
        loaded_lookup_accounts
            .iter()
            .filter(|m| !m.is_writable)
            .cloned(),
    );

    let meta_at = |index: u8| {
        keys.get(usize::from(index))
            .cloned()
            .unwrap_or_else(|| AccountMeta::new_readonly(Pubkey::default(), false))
    };
    message
        .instructions
        .iter()
        .map(|ix| Instruction {
            program_id: meta_at(ix.program_id_index).pubkey,
            accounts: ix.account_indexes.iter().map(|i| meta_at(*i)).collect(),
            data: ix.data.clone(),
        })
        .collect()
}

pub(crate) fn build_config_transaction_execute_instruction(
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
    signer_mode: ExternalSignerMode<'_>,
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

    let plan = if let Ok(vtx) = VaultTransactionAccount::parse(&tx_account.data) {
        plan_vault_transaction_execute(
            rpc,
            program_id,
            *multisig_addr,
            proposal_pubkey,
            tx_pubkey,
            executor,
            &vtx,
            signer_mode,
        )?
    } else if let Ok(ctx) =
        crate::infra::accounts::config_tx::ConfigTransactionAccount::parse(&tx_account.data)
    {
        reject_extra_signers_for_config(signer_mode)?;
        VaultExecutePlan {
            instruction: build_config_transaction_execute_instruction(
                program_id,
                *multisig_addr,
                proposal_pubkey,
                tx_pubkey,
                executor,
                &ctx.actions,
            ),
            external_signers: Vec::new(),
            review_instructions: Vec::new(),
        }
    } else {
        return Err(MsigError::Transaction(format!(
            "transaction account for proposal #{index} is neither a vault nor config transaction"
        )));
    };

    let watched_accounts = unique_pubkeys(plan.instruction.accounts.iter().map(|meta| meta.pubkey));
    Ok(ExecuteProposalPlan {
        instruction: plan.instruction,
        watched_accounts,
        external_signers: plan.external_signers,
        review_instructions: plan.review_instructions,
    })
}

/// Config transactions have no inner signers; extra signers are a mistake.
pub(crate) fn reject_extra_signers_for_config(
    mode: ExternalSignerMode<'_>,
) -> Result<(), MsigError> {
    if let ExternalSignerMode::Supplied { keys, flag } = mode {
        if let Some(key) = keys.first() {
            return Err(MsigError::Usage(format!(
                "{key} was passed with {flag}, but this proposal is a config transaction, \
                 which has no inner signers"
            )));
        }
    }
    Ok(())
}

pub(crate) fn resolve_message_lookup_accounts(
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
    memo: Option<&str>,
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
        memo,
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
    memo: Option<&str>,
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
        memo,
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
    memo: Option<&str>,
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
        memo,
    )?;
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

/// Execute an approved proposal. `extra_signers` are raw keypairs for the
/// vault message's external signers (see [`ExternalSignerMode`]); every
/// required one must be supplied, and nothing else.
#[allow(clippy::too_many_arguments)]
pub fn execute_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    extra_signers: Vec<Zeroizing<[u8; 64]>>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        extra_signers,
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
    extra_signers: Vec<Zeroizing<[u8; 64]>>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_proposal_inner(
        rpc,
        signer,
        multisig_addr,
        index,
        extra_signers,
        config,
        dry_run,
        skip_confirm,
        true,
    )
}

/// Build the prepared execute transaction: plan the instruction under the
/// supplied-signer rules, order the extra keypairs as the message expects,
/// and show the inner instructions and extra signers in the preview.
fn prepare_execute_transaction(
    rpc: &dyn RpcProvider,
    executor: Pubkey,
    multisig_addr: &Pubkey,
    index: u64,
    extra_signers: Vec<Zeroizing<[u8; 64]>>,
    config: &Config,
) -> Result<PreparedTransaction, MsigError> {
    let supplied = extra_signers
        .iter()
        .map(|keypair| extra_signer_pubkey(keypair))
        .collect::<Result<Vec<_>, _>>()?;
    let mode = ExternalSignerMode::Supplied {
        keys: &supplied,
        flag: "--extra-signer",
    };
    let plan = prepare_execute_proposal(rpc, multisig_addr, index, executor, config, mode)?;
    let description = execute_description(index, &plan);
    let instructions = vec![plan.instruction];
    let extra_signers = order_extra_signers(&instructions, &executor, extra_signers)?;
    Ok(PreparedTransaction {
        instructions,
        description,
        review_instructions: plan.review_instructions,
        extra_signers,
    })
}

/// The pipeline shows the preview and prompts only outside quiet (JSON) mode,
/// so co-signing with extra signers in quiet mode would skip review entirely.
/// Require an explicit `--yes` there.
fn check_quiet_cosign(
    quiet: bool,
    skip_confirm: bool,
    extra_signer_count: usize,
) -> Result<(), MsigError> {
    if quiet && !skip_confirm && extra_signer_count > 0 {
        return Err(MsigError::Usage(
            "co-signing with --extra-signer in JSON mode requires --yes; review first with \
             `msig proposal execute` in table mode or `msig proposal show <INDEX> --verbose`"
                .into(),
        ));
    }
    Ok(())
}

/// Preview description. With extra signers, it names each one and the inner
/// instructions its signature authorizes (1-based, as listed in the preview).
fn execute_description(index: u64, plan: &ExecuteProposalPlan) -> String {
    let mut description = format!("Execute proposal #{index}");
    if plan.external_signers.is_empty() {
        return description;
    }
    let signers: Vec<String> = plan
        .external_signers
        .iter()
        .map(|key| {
            let positions: Vec<String> = plan
                .review_instructions
                .iter()
                .enumerate()
                .filter(|(_, ix)| ix.accounts.iter().any(|m| m.is_signer && m.pubkey == *key))
                .map(|(i, _)| (i + 1).to_string())
                .collect();
            if positions.is_empty() {
                format!("{key} (no inner instruction references it)")
            } else {
                format!("{key} signs inner instruction(s) {}", positions.join(", "))
            }
        })
        .collect();
    description.push_str(&format!(
        "\n  Extra signers required by the vault message: {}",
        signers.join("; ")
    ));
    description
}

#[allow(clippy::too_many_arguments)]
fn execute_proposal_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    index: u64,
    extra_signers: Vec<Zeroizing<[u8; 64]>>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    check_quiet_cosign(quiet, skip_confirm, extra_signers.len())?;
    let prepared = prepare_execute_transaction(
        rpc,
        signer.pubkey(),
        multisig_addr,
        index,
        extra_signers,
        config,
    )?;

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::infra::accounts::vault_tx::{CompiledInstruction, TransactionMessage};
    use crate::infra::instruction::assert_memo_replaces_none_tail;
    use crate::infra::rpc::AccountData;

    /// RPC that fails every call: vault messages without lookup tables must
    /// be planned without touching the network.
    pub(crate) struct NoRpc;

    impl RpcProvider for NoRpc {
        fn get_account_info(&self, _: &str) -> Result<Option<AccountData>, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn get_multiple_accounts(&self, _: &[&str]) -> Result<Vec<Option<AccountData>>, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn get_minimum_balance_for_rent_exemption(&self, _: usize) -> Result<u64, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn send_transaction(&self, _: &str) -> Result<String, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn simulate_transaction(
            &self,
            _: &str,
        ) -> Result<crate::infra::rpc::types::SimulationResult, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
        fn get_signature_statuses(
            &self,
            _: &[&str],
        ) -> Result<Vec<Option<crate::infra::rpc::types::TxStatus>>, MsigError> {
            Err(MsigError::Transaction("unexpected RPC call".into()))
        }
    }

    /// Raw ed25519 keypair bytes (secret || public) from a fixed seed.
    pub(crate) fn test_keypair(seed: u8) -> Zeroizing<[u8; 64]> {
        let secret = [seed; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();
        let mut out = Zeroizing::new([0u8; 64]);
        out[..32].copy_from_slice(&secret);
        out[32..].copy_from_slice(&public);
        out
    }

    pub(crate) fn keypair_pubkey(keypair: &[u8; 64]) -> Pubkey {
        extra_signer_pubkey(keypair).unwrap_or_else(|e| panic!("{e}"))
    }

    /// A vault transaction whose message needs, besides the vault and one
    /// ephemeral signer PDA, the external signers `writable_external`
    /// (writable) and `readonly_external` (read-only).
    pub(crate) struct ExternalSignerFixture {
        pub multisig: Pubkey,
        pub transaction: Pubkey,
        pub proposal: Pubkey,
        pub vault: Pubkey,
        pub ephemeral: Pubkey,
        pub target: Pubkey,
        pub inner_program: Pubkey,
        pub vtx: VaultTransactionAccount,
    }

    pub(crate) fn external_signer_fixture(
        writable_external: &[Pubkey],
        readonly_external: &[Pubkey],
    ) -> ExternalSignerFixture {
        let program_id = pda::PROGRAM_ID;
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let index = 5;
        let transaction = pda::transaction_pda(&multisig, index, &program_id).0;
        let proposal = pda::proposal_pda(&multisig, index, &program_id).0;
        let vault = pda::vault_pda(&multisig, 2, &program_id).0;
        let ephemeral = pda::ephemeral_signer_pda(&transaction, 0, &program_id).0;
        let target = Pubkey::new_from_array([20u8; 32]);
        let inner_program = Pubkey::new_from_array([21u8; 32]);

        let mut keys = vec![vault];
        keys.extend_from_slice(writable_external);
        keys.push(ephemeral);
        keys.extend_from_slice(readonly_external);
        let num_signers = keys.len();
        keys.push(target);
        keys.push(inner_program);
        let account_indexes: Vec<u8> = (0..num_signers + 1).map(|i| i as u8).collect();

        let vtx = VaultTransactionAccount {
            multisig,
            creator: Pubkey::new_from_array([8u8; 32]),
            index,
            bump: 255,
            vault_index: 2,
            vault_bump: 255,
            ephemeral_signer_bumps: vec![254],
            message: TransactionMessage {
                num_signers: num_signers as u8,
                num_writable_signers: (1 + writable_external.len()) as u8,
                num_writable_non_signers: 1,
                account_keys: keys.iter().map(|k| k.to_bytes()).collect(),
                instructions: vec![CompiledInstruction {
                    program_id_index: (num_signers + 1) as u8,
                    account_indexes,
                    data: vec![1, 2, 3],
                }],
                address_table_lookups: vec![],
            },
        };
        ExternalSignerFixture {
            multisig,
            transaction,
            proposal,
            vault,
            ephemeral,
            target,
            inner_program,
            vtx,
        }
    }

    /// Serialize a vault transaction account the way the Squads program
    /// stores it (Borsh, u32 vector lengths), without lookup tables.
    pub(crate) fn encode_vault_tx(vtx: &VaultTransactionAccount) -> Vec<u8> {
        let len = |n: usize| (n as u32).to_le_bytes();
        let mut buf = Vec::new();
        buf.extend_from_slice(&crate::infra::accounts::discriminators::VAULT_TRANSACTION);
        buf.extend_from_slice(vtx.multisig.as_ref());
        buf.extend_from_slice(vtx.creator.as_ref());
        buf.extend_from_slice(&vtx.index.to_le_bytes());
        buf.push(vtx.bump);
        buf.push(vtx.vault_index);
        buf.push(vtx.vault_bump);
        buf.extend_from_slice(&len(vtx.ephemeral_signer_bumps.len()));
        buf.extend_from_slice(&vtx.ephemeral_signer_bumps);
        let msg = &vtx.message;
        buf.extend_from_slice(&[
            msg.num_signers,
            msg.num_writable_signers,
            msg.num_writable_non_signers,
        ]);
        buf.extend_from_slice(&len(msg.account_keys.len()));
        for key in &msg.account_keys {
            buf.extend_from_slice(key);
        }
        buf.extend_from_slice(&len(msg.instructions.len()));
        for ix in &msg.instructions {
            buf.push(ix.program_id_index);
            buf.extend_from_slice(&len(ix.account_indexes.len()));
            buf.extend_from_slice(&ix.account_indexes);
            buf.extend_from_slice(&len(ix.data.len()));
            buf.extend_from_slice(&ix.data);
        }
        buf.extend_from_slice(&len(0));
        buf
    }

    fn signer_keys(ix: &Instruction) -> Vec<Pubkey> {
        ix.accounts
            .iter()
            .filter(|m| m.is_signer)
            .map(|m| m.pubkey)
            .collect()
    }

    #[test]
    fn required_external_signers_exclude_vault_and_ephemeral_pdas() {
        let ext_w = Pubkey::new_from_array([30u8; 32]);
        let ext_r = Pubkey::new_from_array([31u8; 32]);
        let f = external_signer_fixture(&[ext_w], &[ext_r]);
        let required =
            required_external_signers(&f.vtx, &f.multisig, &f.transaction, &pda::PROGRAM_ID);
        assert_eq!(required, vec![ext_w, ext_r]);

        // Only the vault of the transaction's vault index is exempt.
        let mut other_index = f.vtx.clone();
        other_index.vault_index = 0;
        let required =
            required_external_signers(&other_index, &f.multisig, &f.transaction, &pda::PROGRAM_ID);
        assert_eq!(required, vec![f.vault, ext_w, ext_r]);

        // Without the ephemeral signer bump, that PDA is an ordinary key.
        let mut no_ephemeral = f.vtx.clone();
        no_ephemeral.ephemeral_signer_bumps.clear();
        let required =
            required_external_signers(&no_ephemeral, &f.multisig, &f.transaction, &pda::PROGRAM_ID);
        assert_eq!(required, vec![ext_w, f.ephemeral, ext_r]);

        // A vault-only message needs nobody else.
        let f = external_signer_fixture(&[], &[]);
        assert!(
            required_external_signers(&f.vtx, &f.multisig, &f.transaction, &pda::PROGRAM_ID)
                .is_empty()
        );
    }

    #[test]
    fn execute_instruction_marks_only_selected_signers() {
        let ext_w = Pubkey::new_from_array([30u8; 32]);
        let ext_r = Pubkey::new_from_array([31u8; 32]);
        let executor = Pubkey::new_from_array([40u8; 32]);
        let f = external_signer_fixture(&[ext_w], &[ext_r]);
        let build = |signers: &[Pubkey]| {
            build_vault_transaction_execute_instruction_with_signers(
                pda::PROGRAM_ID,
                f.multisig,
                f.proposal,
                f.transaction,
                executor,
                &f.vtx.message,
                &[],
                &[],
                signers,
            )
        };

        // Default (SDK) builder: only the executor signs.
        let plain = build_vault_transaction_execute_instruction(
            pda::PROGRAM_ID,
            f.multisig,
            f.proposal,
            f.transaction,
            executor,
            &f.vtx.message,
            &[],
            &[],
        );
        assert_eq!(signer_keys(&plain), vec![executor]);
        assert_eq!(plain, build(&[]));

        let ix = build(&[ext_r]);
        assert_eq!(signer_keys(&ix), vec![executor, ext_r]);
        let ix = build(&[ext_w, ext_r]);
        assert_eq!(signer_keys(&ix), vec![executor, ext_w, ext_r]);
        // Writability is unchanged; vault and ephemeral PDA never sign.
        let meta = |key: Pubkey| {
            ix.accounts
                .iter()
                .find(|m| m.pubkey == key)
                .cloned()
                .unwrap_or_else(|| panic!("{key} missing"))
        };
        assert!(meta(ext_w).is_writable);
        assert!(!meta(ext_r).is_writable);
        assert!(!meta(f.vault).is_signer);
        assert!(!meta(f.ephemeral).is_signer);
        // A non-signer key of the message is never marked, even if listed.
        let ix = build(&[f.target]);
        assert_eq!(signer_keys(&ix), vec![executor]);
    }

    #[test]
    fn select_external_signers_rules() {
        let a = Pubkey::new_from_array([30u8; 32]);
        let b = Pubkey::new_from_array([31u8; 32]);
        let payer = Pubkey::new_from_array([40u8; 32]);
        fn supplied(keys: &[Pubkey]) -> ExternalSignerMode<'_> {
            ExternalSignerMode::Supplied {
                keys,
                flag: "--extra-signer",
            }
        }
        let required = [a, b];

        // Supplied in any order: marked in message order.
        let keys = [b, a];
        assert_eq!(
            select_external_signers(&required, &payer, supplied(&keys)).ok(),
            Some(vec![a, b])
        );
        // Nothing required, nothing supplied.
        assert_eq!(
            select_external_signers(&[], &payer, supplied(&[])).ok(),
            Some(vec![])
        );

        // Missing signer: names the key and the flag.
        let keys = [a];
        match select_external_signers(&required, &payer, supplied(&keys)) {
            Err(MsigError::Usage(msg)) => {
                assert!(msg.contains(&b.to_string()), "{msg}");
                assert!(msg.contains("--extra-signer <KEYPAIR_FILE>"), "{msg}");
            }
            other => panic!("expected missing-signer error, got {other:?}"),
        }

        // Unexpected supplied key.
        let stranger = Pubkey::new_from_array([50u8; 32]);
        let keys = [a, b, stranger];
        match select_external_signers(&required, &payer, supplied(&keys)) {
            Err(MsigError::Usage(msg)) => {
                assert!(msg.contains(&stranger.to_string()));
                assert!(msg.contains("not a required signer"));
            }
            other => panic!("expected unexpected-signer error, got {other:?}"),
        }
        let keys = [stranger];
        assert!(select_external_signers(&[], &payer, supplied(&keys)).is_err());

        // Duplicate supplied key.
        let keys = [a, a, b];
        assert!(matches!(
            select_external_signers(&required, &payer, supplied(&keys)),
            Err(MsigError::Usage(_))
        ));

        // Simulation marks every required signer without keys.
        assert_eq!(
            select_external_signers(&required, &payer, ExternalSignerMode::Simulate).ok(),
            Some(vec![a, b])
        );

        // The fee payer as an inner signer is refused in every mode, even when
        // explicitly supplied.
        let required = [a, payer];
        let keys = [a, payer];
        for mode in [supplied(&keys), ExternalSignerMode::Simulate] {
            match select_external_signers(&required, &payer, mode) {
                Err(MsigError::Transaction(msg)) => {
                    assert!(msg.contains(&payer.to_string()));
                    assert!(msg.contains("fee payer"));
                }
                other => panic!("expected fee-payer refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn plan_vault_execute_applies_rules_and_reviews_inner_instructions() {
        let ext = Pubkey::new_from_array([31u8; 32]);
        let executor = Pubkey::new_from_array([40u8; 32]);
        let f = external_signer_fixture(&[], &[ext]);
        let plan = |mode| {
            plan_vault_transaction_execute(
                &NoRpc,
                pda::PROGRAM_ID,
                f.multisig,
                f.proposal,
                f.transaction,
                executor,
                &f.vtx,
                mode,
            )
        };

        // Missing extra signer: refused before anything is signed.
        let mode = ExternalSignerMode::Supplied {
            keys: &[],
            flag: "--extra-signer",
        };
        assert!(matches!(plan(mode), Err(MsigError::Usage(_))));

        let keys = [ext];
        let mode = ExternalSignerMode::Supplied {
            keys: &keys,
            flag: "--extra-signer",
        };
        let planned = plan(mode).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(planned.external_signers, vec![ext]);
        assert_eq!(signer_keys(&planned.instruction), vec![executor, ext]);
        assert_eq!(planned.review_instructions.len(), 1);
        let inner = &planned.review_instructions[0];
        assert_eq!(inner.program_id, f.inner_program);
        assert_eq!(inner.data, vec![1, 2, 3]);
        let inner_keys: Vec<Pubkey> = inner.accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(inner_keys, vec![f.vault, f.ephemeral, ext, f.target]);

        let simulated = plan(ExternalSignerMode::Simulate).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(simulated.instruction, planned.instruction);

        // Vault-only message: no review instructions, same as before.
        let f = external_signer_fixture(&[], &[]);
        let planned = plan_vault_transaction_execute(
            &NoRpc,
            pda::PROGRAM_ID,
            f.multisig,
            f.proposal,
            f.transaction,
            executor,
            &f.vtx,
            ExternalSignerMode::Supplied {
                keys: &[],
                flag: "--extra-signer",
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(planned.external_signers.is_empty());
        assert!(planned.review_instructions.is_empty());
        assert_eq!(signer_keys(&planned.instruction), vec![executor]);
    }

    #[test]
    fn execute_description_names_extra_signers_and_their_instructions() {
        let ext = Pubkey::new_from_array([31u8; 32]);
        let executor = Pubkey::new_from_array([40u8; 32]);
        let f = external_signer_fixture(&[], &[ext]);
        let planned = plan_vault_transaction_execute(
            &NoRpc,
            pda::PROGRAM_ID,
            f.multisig,
            f.proposal,
            f.transaction,
            executor,
            &f.vtx,
            ExternalSignerMode::Simulate,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let plan = ExecuteProposalPlan {
            instruction: planned.instruction,
            watched_accounts: vec![],
            external_signers: planned.external_signers,
            review_instructions: planned.review_instructions,
        };
        let text = execute_description(9, &plan);
        assert!(text.starts_with("Execute proposal #9"));
        assert!(text.contains(&format!("{ext} signs inner instruction(s) 1")));

        let plain = ExecuteProposalPlan {
            external_signers: vec![],
            review_instructions: vec![],
            ..plan
        };
        assert_eq!(execute_description(9, &plain), "Execute proposal #9");
    }

    #[test]
    fn quiet_mode_cosigning_requires_yes() {
        match check_quiet_cosign(true, false, 1) {
            Err(MsigError::Usage(msg)) => {
                assert!(msg.contains("--yes"));
                assert!(msg.contains("proposal show <INDEX> --verbose"));
            }
            other => panic!("expected refusal, got {other:?}"),
        }
        assert!(check_quiet_cosign(true, true, 1).is_ok());
        assert!(check_quiet_cosign(false, false, 1).is_ok());
        assert!(check_quiet_cosign(true, false, 0).is_ok());

        // Refused before any RPC call (NoRpc fails every call).
        let result = execute_proposal_quiet(
            &NoRpc,
            &FixedSigner(Pubkey::new_from_array([40u8; 32])),
            &Pubkey::new_from_array([7u8; 32]),
            5,
            vec![test_keypair(11)],
            &Config::default(),
            false,
            false,
        );
        assert!(matches!(result, Err(MsigError::Usage(msg)) if msg.contains("--yes")));
    }

    /// Signer that must never be asked to sign.
    struct FixedSigner(Pubkey);

    impl Signer for FixedSigner {
        fn pubkey(&self) -> Pubkey {
            self.0
        }
        fn sign(&self, _: &[u8]) -> Result<Vec<u8>, MsigError> {
            Err(MsigError::Signing("unexpected signature request".into()))
        }
    }

    #[test]
    fn config_transactions_refuse_extra_signers() {
        let key = [Pubkey::new_from_array([30u8; 32])];
        assert!(
            reject_extra_signers_for_config(ExternalSignerMode::Supplied {
                keys: &key,
                flag: "--extra-signer",
            })
            .is_err()
        );
        assert!(
            reject_extra_signers_for_config(ExternalSignerMode::Supplied {
                keys: &[],
                flag: "--extra-signer",
            })
            .is_ok()
        );
    }

    #[test]
    fn encoded_fixture_round_trips() {
        let f = external_signer_fixture(&[Pubkey::new_from_array([30u8; 32])], &[]);
        let parsed = VaultTransactionAccount::parse(&encode_vault_tx(&f.vtx))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.message.account_keys, f.vtx.message.account_keys);
        assert_eq!(parsed.ephemeral_signer_bumps, vec![254]);
    }

    fn fixture_keys() -> (Pubkey, Pubkey, Pubkey, Pubkey) {
        (
            Pubkey::new_from_array([1u8; 32]),
            Pubkey::new_from_array([2u8; 32]),
            Pubkey::new_from_array([3u8; 32]),
            Pubkey::new_from_array([4u8; 32]),
        )
    }

    #[test]
    fn vote_instruction_encodes_some_memo_at_tail() {
        let (program_id, multisig, proposal, member) = fixture_keys();
        let memo = "approve invoice 137";
        let none =
            build_vote_instruction(program_id, multisig, proposal, member, Vote::Approve, None)
                .unwrap_or_else(|e| panic!("{e}"));
        let some = build_vote_instruction(
            program_id,
            multisig,
            proposal,
            member,
            Vote::Approve,
            Some(memo),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_memo_replaces_none_tail(&none.data, &some.data, memo);
        // Account meta lists must be untouched by the memo argument.
        assert_eq!(none.accounts, some.accounts);
        assert_eq!(none.program_id, some.program_id);
    }

    #[test]
    fn vote_instruction_memo_round_trips_per_vote_kind() {
        let (program_id, multisig, proposal, member) = fixture_keys();
        let memo = "rejected: inputs do not match RFC";
        for vote in [Vote::Approve, Vote::Reject, Vote::Cancel] {
            let none = build_vote_instruction(program_id, multisig, proposal, member, vote, None)
                .unwrap_or_else(|e| panic!("{e}"));
            let some =
                build_vote_instruction(program_id, multisig, proposal, member, vote, Some(memo))
                    .unwrap_or_else(|e| panic!("{e}"));
            assert_memo_replaces_none_tail(&none.data, &some.data, memo);
        }
    }
}
