use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;

use crate::domain::transaction::SpendingLimitPeriod;
use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::pipeline::{execute_transaction, execute_transaction_quiet, PreparedTransaction};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const CONFIG_TX_CREATE_DISC: [u8; 8] = [0x9b, 0xec, 0x57, 0xe4, 0x89, 0x4b, 0x51, 0x27];
const PROPOSAL_CREATE_DISC: [u8; 8] = [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f];
const PROPOSAL_ACTIVATE_DISC: [u8; 8] = [0x0b, 0x22, 0x5c, 0xf8, 0x9a, 0x1b, 0x33, 0x6a];
const CONFIG_ACTION_ADD_MEMBER: u8 = 0;
const CONFIG_ACTION_REMOVE_MEMBER: u8 = 1;
const CONFIG_ACTION_CHANGE_THRESHOLD: u8 = 2;
const CONFIG_ACTION_SET_TIME_LOCK: u8 = 3;
const CONFIG_ACTION_ADD_SPENDING_LIMIT: u8 = 4;
const CONFIG_ACTION_REMOVE_SPENDING_LIMIT: u8 = 5;
const CONFIG_ACTION_SET_RENT_COLLECTOR: u8 = 6;

#[derive(Debug, Clone)]
pub struct SpendingLimitProposalResult {
    pub signature: Option<String>,
    pub spending_limit: Pubkey,
    pub create_key: Pubkey,
}

#[derive(Debug, Clone)]
enum ConfigTransactionAction {
    AddMember {
        member: Pubkey,
        permissions: u8,
    },
    RemoveMember {
        member: Pubkey,
    },
    ChangeThreshold {
        new_threshold: u16,
    },
    SetTimeLock {
        new_time_lock: u32,
    },
    AddSpendingLimit {
        create_key: Pubkey,
        vault_index: u8,
        mint: Pubkey,
        amount: u64,
        period: SpendingLimitPeriod,
        members: Vec<Pubkey>,
        destinations: Vec<Pubkey>,
    },
    RemoveSpendingLimit {
        spending_limit: Pubkey,
    },
    SetRentCollector {
        new_rent_collector: Option<Pubkey>,
    },
}

fn build_config_transaction_create_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    transaction: Pubkey,
    creator: Pubkey,
    action: ConfigTransactionAction,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&CONFIG_TX_CREATE_DISC);
    data.extend_from_slice(&1u32.to_le_bytes());
    match action {
        ConfigTransactionAction::AddMember {
            member,
            permissions,
        } => {
            data.push(CONFIG_ACTION_ADD_MEMBER);
            data.extend_from_slice(member.as_ref());
            data.push(permissions);
        }
        ConfigTransactionAction::RemoveMember { member } => {
            data.push(CONFIG_ACTION_REMOVE_MEMBER);
            data.extend_from_slice(member.as_ref());
        }
        ConfigTransactionAction::ChangeThreshold { new_threshold } => {
            data.push(CONFIG_ACTION_CHANGE_THRESHOLD);
            data.extend_from_slice(&new_threshold.to_le_bytes());
        }
        ConfigTransactionAction::SetTimeLock { new_time_lock } => {
            data.push(CONFIG_ACTION_SET_TIME_LOCK);
            data.extend_from_slice(&new_time_lock.to_le_bytes());
        }
        ConfigTransactionAction::AddSpendingLimit {
            create_key,
            vault_index,
            mint,
            amount,
            period,
            members,
            destinations,
        } => {
            data.push(CONFIG_ACTION_ADD_SPENDING_LIMIT);
            data.extend_from_slice(create_key.as_ref());
            data.push(vault_index);
            data.extend_from_slice(mint.as_ref());
            data.extend_from_slice(&amount.to_le_bytes());
            data.push(period_tag(period));
            push_pubkey_vec(&mut data, &members);
            push_pubkey_vec(&mut data, &destinations);
        }
        ConfigTransactionAction::RemoveSpendingLimit { spending_limit } => {
            data.push(CONFIG_ACTION_REMOVE_SPENDING_LIMIT);
            data.extend_from_slice(spending_limit.as_ref());
        }
        ConfigTransactionAction::SetRentCollector { new_rent_collector } => {
            data.push(CONFIG_ACTION_SET_RENT_COLLECTOR);
            match new_rent_collector {
                Some(rc) => {
                    data.push(0x01);
                    data.extend_from_slice(rc.as_ref());
                }
                None => data.push(0x00),
            }
        }
    }
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

fn push_pubkey_vec(data: &mut Vec<u8>, values: &[Pubkey]) {
    data.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values {
        data.extend_from_slice(value.as_ref());
    }
}

fn period_tag(period: SpendingLimitPeriod) -> u8 {
    match period {
        SpendingLimitPeriod::OneTime => 0,
        SpendingLimitPeriod::Day => 1,
        SpendingLimitPeriod::Week => 2,
        SpendingLimitPeriod::Month => 3,
    }
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
pub fn create_member_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    member_addr: &str,
    permissions: u8,
    add: bool,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let member_pubkey: Pubkey = member_addr
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid member address: '{member_addr}'")))?;

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

    let existing = ms.members.iter().any(|m| m.key == member_pubkey);
    if add && existing {
        return Err(MsigError::Usage(format!(
            "member {} is already in the multisig",
            member_addr
        )));
    }
    if !add && !existing {
        return Err(MsigError::Usage(format!(
            "member {} is not in the multisig",
            member_addr
        )));
    }

    let next_index = ms.transaction_index.saturating_add(1);
    let (transaction_pubkey, _) = pda::transaction_pda(multisig_addr, next_index, &program_id);
    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, next_index, &program_id);
    let creator = signer.pubkey();
    let action_label = if add { "Add" } else { "Remove" };

    let mut instructions = Vec::new();

    let action = if add {
        ConfigTransactionAction::AddMember {
            member: member_pubkey,
            permissions,
        }
    } else {
        ConfigTransactionAction::RemoveMember {
            member: member_pubkey,
        }
    };
    instructions.push(build_config_transaction_create_instruction(
        program_id,
        *multisig_addr,
        transaction_pubkey,
        creator,
        action,
    ));

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
        description: format!("{action_label} member {member_addr} (permissions={permissions})"),
        review_instructions: vec![],
        extra_signers: vec![],
    };

    execute_config_proposal(
        rpc,
        signer,
        prepared,
        config,
        dry_run,
        skip_confirm,
        next_index,
        quiet,
    )
}

/// Create a proposal to set or clear the rent collector.
#[allow(clippy::too_many_arguments)]
pub fn create_set_rent_collector_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    rent_collector: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let rent_collector_pubkey: Option<Pubkey> = rent_collector
        .map(|rc| {
            rc.parse()
                .map_err(|_| MsigError::Usage(format!("invalid rent-collector address: '{rc}'")))
        })
        .transpose()?;

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
    let (transaction_pubkey, _) = pda::transaction_pda(multisig_addr, next_index, &program_id);
    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, next_index, &program_id);
    let creator = signer.pubkey();

    let description = match &rent_collector_pubkey {
        Some(rc) => format!("Set rent collector to {rc}"),
        None => "Clear rent collector".to_string(),
    };

    let instructions = vec![
        build_config_transaction_create_instruction(
            program_id,
            *multisig_addr,
            transaction_pubkey,
            creator,
            ConfigTransactionAction::SetRentCollector {
                new_rent_collector: rent_collector_pubkey,
            },
        ),
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
        review_instructions: vec![],
        extra_signers: vec![],
    };

    execute_config_proposal(
        rpc,
        signer,
        prepared,
        config,
        dry_run,
        skip_confirm,
        next_index,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_config_action_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    action: ConfigTransactionAction,
    description: String,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let program_id = config.program_id;
    let multisig_str = multisig_addr.to_string();

    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    if let ConfigTransactionAction::ChangeThreshold { new_threshold } = &action {
        if usize::from(*new_threshold) > ms.members.len() {
            return Err(MsigError::Usage(format!(
                "threshold {new_threshold} exceeds member count {}",
                ms.members.len()
            )));
        }
    }

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
    let (transaction_pubkey, _) = pda::transaction_pda(multisig_addr, next_index, &program_id);
    let (proposal_pubkey, _) = pda::proposal_pda(multisig_addr, next_index, &program_id);
    let creator = signer.pubkey();

    let instructions = vec![
        build_config_transaction_create_instruction(
            program_id,
            *multisig_addr,
            transaction_pubkey,
            creator,
            action,
        ),
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
        review_instructions: vec![],
        extra_signers: vec![],
    };

    execute_config_proposal(
        rpc,
        signer,
        prepared,
        config,
        dry_run,
        skip_confirm,
        next_index,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_config_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    prepared: PreparedTransaction,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    next_index: u64,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let result = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };
    if !quiet && result.is_some() {
        println!(
            "Config proposal #{} created. Approve: msig proposal approve {}",
            next_index, next_index
        );
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub fn create_change_threshold_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    new_threshold: u16,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    if new_threshold == 0 {
        return Err(MsigError::Usage("threshold must be greater than 0".into()));
    }
    create_config_action_proposal(
        rpc,
        signer,
        multisig_addr,
        ConfigTransactionAction::ChangeThreshold { new_threshold },
        format!("Change multisig threshold to {new_threshold}"),
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_set_time_lock_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    new_time_lock: u32,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    create_config_action_proposal(
        rpc,
        signer,
        multisig_addr,
        ConfigTransactionAction::SetTimeLock { new_time_lock },
        format!("Set multisig time lock to {new_time_lock}s"),
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_add_spending_limit_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    vault_index: u8,
    mint: Pubkey,
    amount: u64,
    period: SpendingLimitPeriod,
    members: Vec<Pubkey>,
    destinations: Vec<Pubkey>,
    create_key_override: Option<Pubkey>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<SpendingLimitProposalResult, MsigError> {
    if amount == 0 {
        return Err(MsigError::Usage(
            "spending limit amount must be greater than 0".into(),
        ));
    }
    if members.is_empty() {
        return Err(MsigError::Usage(
            "spending limit requires at least one member".into(),
        ));
    }
    ensure_unique_pubkeys(&members, "member")?;
    ensure_unique_pubkeys(&destinations, "destination")?;

    let multisig_str = multisig_addr.to_string();
    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    for member in &members {
        if !ms.members.iter().any(|existing| existing.key == *member) {
            return Err(MsigError::Usage(format!(
                "spending limit member {member} is not in the multisig"
            )));
        }
    }

    let next_index = ms.transaction_index.saturating_add(1);
    let signer_pubkey = signer.pubkey();
    let create_key = create_key_override.unwrap_or_else(|| {
        derive_spending_limit_create_key(
            multisig_addr,
            next_index,
            &signer_pubkey,
            vault_index,
            &mint,
            amount,
            period,
            &members,
            &destinations,
        )
    });
    let (spending_limit, _) =
        pda::spending_limit_pda(multisig_addr, &create_key, &config.program_id);

    let signature = create_config_action_proposal(
        rpc,
        signer,
        multisig_addr,
        ConfigTransactionAction::AddSpendingLimit {
            create_key,
            vault_index,
            mint,
            amount,
            period,
            members,
            destinations,
        },
        format!("Add spending limit {spending_limit}"),
        config,
        dry_run,
        skip_confirm,
        quiet,
    )?;

    Ok(SpendingLimitProposalResult {
        signature,
        spending_limit,
        create_key,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn create_remove_spending_limit_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    spending_limit: Pubkey,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    create_config_action_proposal(
        rpc,
        signer,
        multisig_addr,
        ConfigTransactionAction::RemoveSpendingLimit { spending_limit },
        format!("Remove spending limit {spending_limit}"),
        config,
        dry_run,
        skip_confirm,
        quiet,
    )
}

fn ensure_unique_pubkeys(values: &[Pubkey], label: &str) -> Result<(), MsigError> {
    let mut seen = Vec::with_capacity(values.len());
    for value in values {
        if seen.contains(value) {
            return Err(MsigError::Usage(format!(
                "duplicate spending limit {label}: {value}"
            )));
        }
        seen.push(*value);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn derive_spending_limit_create_key(
    multisig: &Pubkey,
    transaction_index: u64,
    creator: &Pubkey,
    vault_index: u8,
    mint: &Pubkey,
    amount: u64,
    period: SpendingLimitPeriod,
    members: &[Pubkey],
    destinations: &[Pubkey],
) -> Pubkey {
    let mut hasher = Sha256::new();
    hasher.update(b"msig:spending-limit-create-key:v1");
    hasher.update(multisig.as_ref());
    hasher.update(transaction_index.to_le_bytes());
    hasher.update(creator.as_ref());
    hasher.update([vault_index]);
    hasher.update(mint.as_ref());
    hasher.update(amount.to_le_bytes());
    hasher.update([period_tag(period)]);
    for member in members {
        hasher.update(member.as_ref());
    }
    hasher.update([0xff]);
    for destination in destinations {
        hasher.update(destination.as_ref());
    }
    let bytes: [u8; 32] = hasher.finalize().into();
    Pubkey::from(bytes)
}

pub fn parse_permissions(input: &str) -> Result<u8, MsigError> {
    let mut mask: u8 = 0;
    for part in input.split(',') {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "initiate" => mask |= 1,
            "vote" => mask |= 2,
            "execute" => mask |= 4,
            other => {
                return Err(MsigError::Usage(format!(
                    "unknown permission: '{other}'. Valid: initiate, vote, execute"
                )))
            }
        }
    }
    if mask == 0 {
        return Err(MsigError::Usage(
            "at least one permission is required".into(),
        ));
    }
    Ok(mask)
}
