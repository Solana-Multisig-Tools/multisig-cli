//! Stake account operations run from a Squads vault.
//!
//! Each operation becomes one vault-transaction proposal. The vault signs as
//! the stake account's staker and/or withdrawer; `create` and `split` also use
//! the transaction's ephemeral signer PDA 0 as the new stake account, which the
//! Squads program signs for at execution. Stake and System program
//! instructions are encoded by hand (bincode: u32 LE enum tag followed by LE
//! fields) and checked against bytes dumped from the solana CLI.

use std::collections::HashMap;

use solana_pubkey::Pubkey;

use crate::domain::stake_account::{StakeAccountInfo, StakeDelegationInfo, StakeLockupInfo};
use crate::error::MsigError;
use crate::infra::accounts::stake::{Clock, StakeAccountState, StakeMeta};
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::{AccountData, RpcProvider};
use crate::infra::signer::Signer;
use crate::output::{format_addr, format_sol};
use crate::sanitize::sanitize_display;

use super::validator_common::{
    is_external_withdrawer, read_i64, read_pubkey, read_u32, read_u64, simulate_inner_instructions,
    vault_addresses, DecodedInstruction, SYSVAR_CLOCK,
};
use super::vault_tx::{
    create_vault_transaction_proposal, create_vault_transaction_proposal_with_ephemeral_signers,
    ephemeral_signer_address, next_transaction_index, EphemeralSigners, VaultProposalResult,
};
use super::vote_account::VOTE_PROGRAM_ID;

pub const STAKE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("Stake11111111111111111111111111111111111111");
const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const SYSVAR_OWNER: Pubkey = solana_pubkey::pubkey!("Sysvar1111111111111111111111111111111111111");
const SYSVAR_RENT: Pubkey = solana_pubkey::pubkey!("SysvarRent111111111111111111111111111111111");
const SYSVAR_STAKE_HISTORY: Pubkey =
    solana_pubkey::pubkey!("SysvarStakeHistory1111111111111111111111111");
const STAKE_CONFIG: Pubkey = solana_pubkey::pubkey!("StakeConfig11111111111111111111111111111111");

/// Size of a `StakeStateV2` account.
pub const STAKE_ACCOUNT_SPACE: u64 = 200;
const STAKE_ACCOUNT_DATA_LEN: usize = 200;
/// Ephemeral signer index used for the new stake account of `create`/`split`.
const NEW_STAKE_EPHEMERAL_INDEX: u8 = 0;

const SYS_IX_ASSIGN: u32 = 1;
const SYS_IX_TRANSFER: u32 = 2;
const SYS_IX_ALLOCATE: u32 = 8;

const IX_INITIALIZE: u32 = 0;
const IX_AUTHORIZE: u32 = 1;
const IX_DELEGATE_STAKE: u32 = 2;
const IX_SPLIT: u32 = 3;
const IX_WITHDRAW: u32 = 4;
const IX_DEACTIVATE: u32 = 5;
const IX_SET_LOCKUP: u32 = 6;
const IX_MERGE: u32 = 7;
const IX_AUTHORIZE_WITH_SEED: u32 = 8;
const IX_INITIALIZE_CHECKED: u32 = 9;
const IX_AUTHORIZE_CHECKED: u32 = 10;
const IX_AUTHORIZE_CHECKED_WITH_SEED: u32 = 11;
const IX_SET_LOCKUP_CHECKED: u32 = 12;
const IX_GET_MINIMUM_DELEGATION: u32 = 13;
const IX_DEACTIVATE_DELINQUENT: u32 = 14;
const IX_REDELEGATE: u32 = 15;
const IX_MOVE_STAKE: u32 = 16;
const IX_MOVE_LAMPORTS: u32 = 17;

/// `StakeAuthorize` role (bincode u32 tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakeAuthorize {
    Staker,
    Withdrawer,
}

impl StakeAuthorize {
    fn tag(self) -> u32 {
        match self {
            Self::Staker => 0,
            Self::Withdrawer => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Staker => "staker",
            Self::Withdrawer => "withdrawer",
        }
    }

    fn from_tag(tag: u32) -> Option<Self> {
        match tag {
            0 => Some(Self::Staker),
            1 => Some(Self::Withdrawer),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Instruction builders
// ---------------------------------------------------------------------------

/// `SystemInstruction::Transfer`: [from w s, to w].
pub fn build_system_transfer_instruction(from: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&SYS_IX_TRANSFER.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data,
    }
}

/// `SystemInstruction::Allocate`: [account w s].
pub fn build_system_allocate_instruction(account: Pubkey, space: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&SYS_IX_ALLOCATE.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(account, true)],
        data,
    }
}

/// `SystemInstruction::Assign`: [account w s].
pub fn build_system_assign_instruction(account: Pubkey, owner: Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&SYS_IX_ASSIGN.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![AccountMeta::new(account, true)],
        data,
    }
}

/// `StakeInstruction::Initialize(Authorized, Lockup)` with no lockup:
/// [stake w, rent sysvar].
pub fn build_initialize_instruction(
    stake: Pubkey,
    staker: Pubkey,
    withdrawer: Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(116);
    data.extend_from_slice(&IX_INITIALIZE.to_le_bytes());
    data.extend_from_slice(staker.as_ref());
    data.extend_from_slice(withdrawer.as_ref());
    data.extend_from_slice(&0i64.to_le_bytes()); // lockup.unix_timestamp
    data.extend_from_slice(&0u64.to_le_bytes()); // lockup.epoch
    data.extend_from_slice(Pubkey::default().as_ref()); // lockup.custodian
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new_readonly(SYSVAR_RENT, false),
        ],
        data,
    }
}

/// `StakeInstruction::DelegateStake`:
/// [stake w, vote, clock, stake history, stake config, staker s].
pub fn build_delegate_instruction(stake: Pubkey, vote: Pubkey, staker: Pubkey) -> Instruction {
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new_readonly(vote, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(SYSVAR_STAKE_HISTORY, false),
            AccountMeta::new_readonly(STAKE_CONFIG, false),
            AccountMeta::new_readonly(staker, true),
        ],
        data: IX_DELEGATE_STAKE.to_le_bytes().to_vec(),
    }
}

/// `StakeInstruction::Deactivate`: [stake w, clock, staker s].
pub fn build_deactivate_instruction(stake: Pubkey, staker: Pubkey) -> Instruction {
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(staker, true),
        ],
        data: IX_DEACTIVATE.to_le_bytes().to_vec(),
    }
}

/// `StakeInstruction::Withdraw(u64)`:
/// [stake w, recipient w, clock, stake history, withdrawer s]. No custodian.
pub fn build_withdraw_instruction(
    stake: Pubkey,
    recipient: Pubkey,
    withdrawer: Pubkey,
    lamports: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&IX_WITHDRAW.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new(recipient, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(SYSVAR_STAKE_HISTORY, false),
            AccountMeta::new_readonly(withdrawer, true),
        ],
        data,
    }
}

/// `StakeInstruction::Split(u64)`: [stake w, destination w, staker s].
pub fn build_split_instruction(
    stake: Pubkey,
    staker: Pubkey,
    lamports: u64,
    destination: Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&IX_SPLIT.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new(destination, false),
            AccountMeta::new_readonly(staker, true),
        ],
        data,
    }
}

/// `StakeInstruction::Merge`:
/// [destination w, source w, clock, stake history, staker s].
pub fn build_merge_instruction(destination: Pubkey, source: Pubkey, staker: Pubkey) -> Instruction {
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(destination, false),
            AccountMeta::new(source, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(SYSVAR_STAKE_HISTORY, false),
            AccountMeta::new_readonly(staker, true),
        ],
        data: IX_MERGE.to_le_bytes().to_vec(),
    }
}

/// `StakeInstruction::Authorize(Pubkey, StakeAuthorize)`:
/// [stake w, clock, current authority s]. No custodian.
pub fn build_authorize_instruction(
    stake: Pubkey,
    current_authority: Pubkey,
    new_authority: Pubkey,
    role: StakeAuthorize,
) -> Instruction {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&IX_AUTHORIZE.to_le_bytes());
    data.extend_from_slice(new_authority.as_ref());
    data.extend_from_slice(&role.tag().to_le_bytes());
    Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(stake, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(current_authority, true),
        ],
        data,
    }
}

/// A new stake account funded by `vault`: Transfer of the full amount,
/// Allocate, Assign to the Stake program, then Initialize with staker =
/// withdrawer = vault and no lockup, then optionally DelegateStake.
///
/// `solana create-stake-account` uses a single CreateAccount instead, which
/// fails if the new address already holds lamports. The new address here is
/// the proposal transaction's ephemeral signer PDA, which anyone can predict
/// once the proposal exists; Transfer + Allocate + Assign still succeed if
/// someone sends it lamports before execution (it stays system-owned with no
/// data), so a dust transfer cannot block the proposal.
pub fn create_stake_account_instructions(
    vault: Pubkey,
    new_stake: Pubkey,
    lamports: u64,
    vote_account: Option<Pubkey>,
) -> Vec<Instruction> {
    let mut instructions = vec![
        build_system_transfer_instruction(vault, new_stake, lamports),
        build_system_allocate_instruction(new_stake, STAKE_ACCOUNT_SPACE),
        build_system_assign_instruction(new_stake, STAKE_PROGRAM_ID),
        build_initialize_instruction(new_stake, vault, vault),
    ];
    if let Some(vote) = vote_account {
        instructions.push(build_delegate_instruction(new_stake, vote, vault));
    }
    instructions
}

/// `solana split-stake` layout: the vault pays the destination's rent-exempt
/// reserve, the destination is allocated and assigned to the Stake program,
/// then Split moves `lamports` into it.
pub fn split_stake_instructions(
    vault: Pubkey,
    stake: Pubkey,
    destination: Pubkey,
    lamports: u64,
    destination_reserve: u64,
) -> Vec<Instruction> {
    vec![
        build_system_transfer_instruction(vault, destination, destination_reserve),
        build_system_allocate_instruction(destination, STAKE_ACCOUNT_SPACE),
        build_system_assign_instruction(destination, STAKE_PROGRAM_ID),
        build_split_instruction(stake, vault, lamports, destination),
    ]
}

// ---------------------------------------------------------------------------
// Preflight checks
// ---------------------------------------------------------------------------

/// A fetched and parsed stake account.
#[derive(Debug, Clone)]
pub struct StakeAccountSnapshot {
    pub address: Pubkey,
    pub lamports: u64,
    pub state: StakeAccountState,
}

/// Check the owner and parse an already-fetched stake account.
pub fn parse_stake_account(
    stake: &Pubkey,
    account: &AccountData,
) -> Result<StakeAccountSnapshot, MsigError> {
    if account.owner != STAKE_PROGRAM_ID.to_string() {
        return Err(MsigError::Transaction(format!(
            "{stake} is not a stake account: it is owned by {}, not the Stake program {STAKE_PROGRAM_ID}",
            sanitize_display(&account.owner)
        )));
    }
    Ok(StakeAccountSnapshot {
        address: *stake,
        lamports: account.lamports,
        state: StakeAccountState::parse(&account.data)?,
    })
}

/// Fetch, owner-check and parse a stake account. RPC errors are returned as is.
pub fn fetch_stake_account(
    rpc: &dyn RpcProvider,
    stake: &Pubkey,
) -> Result<StakeAccountSnapshot, MsigError> {
    let account = rpc.get_account_info(&stake.to_string())?.ok_or_else(|| {
        MsigError::Transaction(format!(
            "stake account {stake} not found on this cluster. Check the address and --cluster."
        ))
    })?;
    parse_stake_account(stake, &account)
}

/// Owner-check and parse the Clock sysvar.
pub fn parse_clock_account(account: &AccountData) -> Result<Clock, MsigError> {
    if account.owner != SYSVAR_OWNER.to_string() {
        return Err(MsigError::Transaction(format!(
            "the Clock sysvar returned by the RPC is owned by {}, not {SYSVAR_OWNER}",
            sanitize_display(&account.owner)
        )));
    }
    Clock::parse(&account.data)
}

pub fn fetch_clock(rpc: &dyn RpcProvider) -> Result<Clock, MsigError> {
    let account = rpc
        .get_account_info(&SYSVAR_CLOCK.to_string())?
        .ok_or_else(|| MsigError::Transaction("the Clock sysvar was not returned".into()))?;
    parse_clock_account(&account)
}

/// Which authority an operation needs the vault to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredAuthority {
    Staker,
    Withdrawer,
    StakerOrWithdrawer,
}

impl RequiredAuthority {
    fn label(self) -> &'static str {
        match self {
            Self::Staker => "staker",
            Self::Withdrawer => "withdrawer",
            Self::StakerOrWithdrawer => "staker or withdrawer",
        }
    }
}

/// Refuse unless the stake account is initialized and the vault at
/// `vault_index` holds the required authority. Returns the account's meta.
pub fn check_vault_authority(
    snapshot: &StakeAccountSnapshot,
    vault: &Pubkey,
    vault_index: u8,
    required: RequiredAuthority,
) -> Result<StakeMeta, MsigError> {
    let meta = *snapshot.state.meta().ok_or_else(|| {
        MsigError::Transaction(format!(
            "stake account {} is {}; it has no staker or withdrawer to act with",
            snapshot.address,
            snapshot.state.label()
        ))
    })?;
    let holds = match required {
        RequiredAuthority::Staker => meta.staker == *vault,
        RequiredAuthority::Withdrawer => meta.withdrawer == *vault,
        RequiredAuthority::StakerOrWithdrawer => meta.staker == *vault || meta.withdrawer == *vault,
    };
    if !holds {
        return Err(MsigError::Transaction(format!(
            "vault {vault} (vault index {vault_index}) is not the {} of stake account {} \
             (staker {}, withdrawer {}). Pass the --vault-index of the vault that holds that \
             authority, or hand the authority to the vault first.",
            required.label(),
            snapshot.address,
            meta.staker,
            meta.withdrawer
        )));
    }
    Ok(meta)
}

/// Refuse while a lockup is in force: msig cannot provide the custodian
/// signature that withdrawals and withdrawer changes would need.
pub fn check_no_lockup(
    stake: &Pubkey,
    meta: &StakeMeta,
    clock: &Clock,
    action: &str,
) -> Result<(), MsigError> {
    if meta.lockup.is_in_force(clock) {
        return Err(MsigError::Transaction(format!(
            "stake account {stake} has a lockup in force (until epoch {} / unix time {}; current \
             epoch {}, unix time {}). {action} needs the lockup custodian {}, which msig does not \
             support. Wait for the lockup to expire.",
            meta.lockup.epoch,
            meta.lockup.unix_timestamp,
            clock.epoch,
            clock.unix_timestamp,
            meta.lockup.custodian
        )));
    }
    Ok(())
}

/// What `stake-account withdraw` was asked to take out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakeWithdrawAmount {
    Lamports(u64),
    /// The full balance, closing the account.
    All,
}

/// Resolve a withdraw request to lamports, refusing amounts the stake program
/// would reject: overdraw, a remainder below the rent-exempt reserve, or
/// lamports still delegated by a stake that is not deactivating. Cooldown of a
/// deactivating stake is left to the simulation.
pub fn resolve_withdraw_amount(
    amount: StakeWithdrawAmount,
    snapshot: &StakeAccountSnapshot,
) -> Result<u64, MsigError> {
    let balance = snapshot.lamports;
    let reserve = snapshot
        .state
        .meta()
        .map(|meta| meta.rent_exempt_reserve)
        .unwrap_or_default();
    let lamports = match amount {
        // Make closing the account an explicit request, as vote-account does.
        StakeWithdrawAmount::Lamports(lamports) if lamports == balance => {
            return Err(MsigError::Usage(format!(
                "{} SOL is the whole balance and would close the stake account; use --all to \
                 withdraw everything (a delegated stake must be deactivated and cooled down first)",
                format_sol(lamports)
            )));
        }
        StakeWithdrawAmount::Lamports(lamports) => lamports,
        StakeWithdrawAmount::All => balance,
    };
    if lamports == 0 {
        return Err(MsigError::Usage(
            "nothing to withdraw: the amount is zero".into(),
        ));
    }
    if lamports > balance {
        return Err(MsigError::Usage(format!(
            "cannot withdraw {} SOL: the stake account balance is {} SOL",
            format_sol(lamports),
            format_sol(balance)
        )));
    }
    if let Some(delegation) = snapshot.state.delegation() {
        if !delegation.is_deactivating() {
            let free = balance.saturating_sub(reserve.saturating_add(delegation.stake));
            if lamports > free {
                return Err(MsigError::Usage(format!(
                    "stake account {} is delegated ({} SOL staked): only {} SOL above the stake \
                     and the rent-exempt reserve can be withdrawn. Run `stake-account deactivate` \
                     first and withdraw after the cooldown.",
                    snapshot.address,
                    format_sol(delegation.stake),
                    format_sol(free)
                )));
            }
        }
    }
    let remaining = balance - lamports;
    if remaining > 0 && remaining < reserve {
        return Err(MsigError::Usage(format!(
            "withdrawing {} SOL would leave {} SOL, below the rent-exempt reserve of {} SOL. \
             Use --all to withdraw everything.",
            format_sol(lamports),
            format_sol(remaining),
            format_sol(reserve)
        )));
    }
    Ok(lamports)
}

/// The vault must hold `spend`, and what it keeps must be zero or at least the
/// rent-exempt minimum of a 0-byte account (the runtime's rent-state check
/// rejects a transaction that leaves it in between).
fn check_vault_can_spend(
    vault_balance: u64,
    spend: u64,
    vault_rent_exempt_minimum: u64,
    what: &str,
) -> Result<(), MsigError> {
    if vault_balance < spend {
        return Err(MsigError::Usage(format!(
            "the vault holds {} SOL, less than the {} SOL {what}",
            format_sol(vault_balance),
            format_sol(spend)
        )));
    }
    let remaining = vault_balance - spend;
    if remaining > 0 && remaining < vault_rent_exempt_minimum {
        return Err(MsigError::Usage(format!(
            "paying {} SOL would leave the vault with {} SOL, below its rent-exempt minimum of \
             {} SOL. Fund the vault, or spend its whole balance.",
            format_sol(spend),
            format_sol(remaining),
            format_sol(vault_rent_exempt_minimum)
        )));
    }
    Ok(())
}

/// `create` funding: the amount (rent reserve included) must cover the
/// rent-exempt minimum, plus the minimum delegation when delegating, and the
/// vault must be able to pay it.
pub fn check_create_funding(
    lamports: u64,
    rent_exempt_minimum: u64,
    minimum_delegation: Option<u64>,
    vault_balance: u64,
    vault_rent_exempt_minimum: u64,
) -> Result<(), MsigError> {
    if lamports < rent_exempt_minimum {
        return Err(MsigError::Usage(format!(
            "{} SOL is below the rent-exempt minimum of {} SOL for a stake account",
            format_sol(lamports),
            format_sol(rent_exempt_minimum)
        )));
    }
    if let Some(minimum) = minimum_delegation {
        let stake = lamports - rent_exempt_minimum;
        if stake < minimum {
            return Err(MsigError::Usage(format!(
                "{} SOL leaves {} SOL to delegate after the {} SOL rent-exempt reserve, below the \
                 minimum delegation of {} SOL",
                format_sol(lamports),
                format_sol(stake),
                format_sol(rent_exempt_minimum),
                format_sol(minimum)
            )));
        }
    }
    check_vault_can_spend(vault_balance, lamports, vault_rent_exempt_minimum, "needed")
}

/// `split` funding: a non-zero amount no larger than the source balance, and
/// a vault that can pay the destination's rent-exempt reserve.
pub fn check_split_funding(
    lamports: u64,
    source_balance: u64,
    destination_reserve: u64,
    vault_balance: u64,
    vault_rent_exempt_minimum: u64,
) -> Result<(), MsigError> {
    if lamports == 0 {
        return Err(MsigError::Usage(
            "split amount must be greater than zero".into(),
        ));
    }
    if lamports > source_balance {
        return Err(MsigError::Usage(format!(
            "cannot split {} SOL: the stake account balance is {} SOL",
            format_sol(lamports),
            format_sol(source_balance)
        )));
    }
    check_vault_can_spend(
        vault_balance,
        destination_reserve,
        vault_rent_exempt_minimum,
        "rent-exempt reserve it pays for the new stake account",
    )
}

/// The Stake program only merges accounts with the same staker, withdrawer
/// and lockup. Delegation-state compatibility is left to the simulation.
pub fn check_merge_compatible(
    destination: &Pubkey,
    destination_meta: &StakeMeta,
    source: &Pubkey,
    source_meta: &StakeMeta,
) -> Result<(), MsigError> {
    let mut differences = Vec::new();
    if destination_meta.staker != source_meta.staker {
        differences.push(format!(
            "staker {} vs {}",
            destination_meta.staker, source_meta.staker
        ));
    }
    if destination_meta.withdrawer != source_meta.withdrawer {
        differences.push(format!(
            "withdrawer {} vs {}",
            destination_meta.withdrawer, source_meta.withdrawer
        ));
    }
    let (d, s) = (&destination_meta.lockup, &source_meta.lockup);
    if d != s {
        differences.push(format!(
            "lockup (epoch {}, unix time {}, custodian {}) vs (epoch {}, unix time {}, \
             custodian {})",
            d.epoch, d.unix_timestamp, d.custodian, s.epoch, s.unix_timestamp, s.custodian
        ));
    }
    if differences.is_empty() {
        return Ok(());
    }
    Err(MsigError::Transaction(format!(
        "stake accounts {destination} and {source} cannot be merged: the Stake program requires \
         the same staker, withdrawer and lockup, and they differ in {}",
        differences.join("; ")
    )))
}

/// `deactivate` needs a delegated stake that is not already deactivating.
pub fn check_can_deactivate(snapshot: &StakeAccountSnapshot) -> Result<(), MsigError> {
    match snapshot.state.delegation() {
        None => Err(MsigError::Transaction(format!(
            "stake account {} is not delegated; there is nothing to deactivate",
            snapshot.address
        ))),
        Some(delegation) if delegation.is_deactivating() => Err(MsigError::Transaction(format!(
            "stake account {} is already deactivating (since epoch {})",
            snapshot.address, delegation.deactivation_epoch
        ))),
        Some(_) => Ok(()),
    }
}

fn check_vote_account_owner(rpc: &dyn RpcProvider, vote: &Pubkey) -> Result<(), MsigError> {
    let account = rpc.get_account_info(&vote.to_string())?.ok_or_else(|| {
        MsigError::Transaction(format!(
            "vote account {vote} not found on this cluster. Check the address and --cluster."
        ))
    })?;
    if account.owner != VOTE_PROGRAM_ID.to_string() {
        return Err(MsigError::Transaction(format!(
            "{vote} is not a vote account: it is owned by {}, not the Vote program {VOTE_PROGRAM_ID}",
            sanitize_display(&account.owner)
        )));
    }
    Ok(())
}

fn vault_balance(rpc: &dyn RpcProvider, vault: &Pubkey) -> Result<u64, MsigError> {
    Ok(rpc
        .get_account_info(&vault.to_string())?
        .map(|account| account.lamports)
        .unwrap_or(0))
}

/// Refuse at propose time an ephemeral PDA that already holds an account. The
/// instructions tolerate lamports sent to it later, but an account with data or
/// another owner would make Allocate/Assign fail.
fn check_new_account_absent(rpc: &dyn RpcProvider, address: &Pubkey) -> Result<(), MsigError> {
    if rpc.get_account_info(&address.to_string())?.is_some() {
        return Err(MsigError::Transaction(format!(
            "the new stake account address {address} already holds an account; nothing was \
             proposed. Run the command again after the next proposal is created."
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Proposal creation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakeOperation {
    Create {
        lamports: u64,
        vote_account: Option<Pubkey>,
    },
    Delegate {
        stake: Pubkey,
        vote_account: Pubkey,
    },
    Deactivate {
        stake: Pubkey,
    },
    Withdraw {
        stake: Pubkey,
        recipient: Pubkey,
        amount: StakeWithdrawAmount,
    },
    Split {
        stake: Pubkey,
        lamports: u64,
    },
    Merge {
        destination: Pubkey,
        source: Pubkey,
    },
    SetStaker {
        stake: Pubkey,
        new_staker: Pubkey,
    },
    SetWithdrawer {
        stake: Pubkey,
        new_withdrawer: Pubkey,
    },
}

pub struct StakeProposalRequest<'a> {
    pub multisig: Pubkey,
    pub vault_index: u8,
    pub operation: StakeOperation,
    pub allow_external_authority: bool,
    pub allow_simulation_failure: bool,
    pub memo: Option<&'a str>,
    pub dry_run: bool,
    pub skip_confirm: bool,
    pub quiet: bool,
}

#[derive(Debug, Clone)]
pub struct StakeProposalResult {
    pub proposal: VaultProposalResult,
    /// The stake account created by `create` or `split`.
    pub new_stake_account: Option<Pubkey>,
}

/// Everything a stake operation needs to be proposed.
#[derive(Debug, Clone)]
struct PreparedStakeOperation {
    instructions: Vec<Instruction>,
    description: String,
    external: Option<String>,
    ephemeral: Option<EphemeralSigners>,
    new_stake_account: Option<Pubkey>,
}

impl PreparedStakeOperation {
    fn simple(instruction: Instruction, description: String) -> Self {
        Self {
            instructions: vec![instruction],
            description,
            external: None,
            ephemeral: None,
            new_stake_account: None,
        }
    }
}

/// The new stake account of `create`/`split`: ephemeral signer PDA 0 of the
/// next vault transaction.
fn new_stake_account_address(
    rpc: &dyn RpcProvider,
    multisig: &Pubkey,
    program_id: &Pubkey,
) -> Result<(Pubkey, EphemeralSigners), MsigError> {
    let transaction_index = next_transaction_index(rpc, multisig)?;
    let address = ephemeral_signer_address(
        multisig,
        transaction_index,
        NEW_STAKE_EPHEMERAL_INDEX,
        program_id,
    );
    Ok((
        address,
        EphemeralSigners {
            count: 1,
            transaction_index,
        },
    ))
}

/// Run every preflight check (authority first) and build the inner
/// instructions plus the proposal description.
fn prepare_operation(
    rpc: &dyn RpcProvider,
    config: &Config,
    req: &StakeProposalRequest<'_>,
    vault: &Pubkey,
) -> Result<PreparedStakeOperation, MsigError> {
    let vault_index = req.vault_index;
    let prepared = match req.operation {
        StakeOperation::Create {
            lamports,
            vote_account,
        } => {
            let rent_exempt_minimum =
                rpc.get_minimum_balance_for_rent_exemption(STAKE_ACCOUNT_DATA_LEN)?;
            let minimum_delegation = match vote_account {
                Some(vote) => {
                    check_vote_account_owner(rpc, &vote)?;
                    Some(rpc.get_stake_minimum_delegation()?)
                }
                None => None,
            };
            check_create_funding(
                lamports,
                rent_exempt_minimum,
                minimum_delegation,
                vault_balance(rpc, vault)?,
                rpc.get_minimum_balance_for_rent_exemption(0)?,
            )?;
            let (new_stake, ephemeral) =
                new_stake_account_address(rpc, &req.multisig, &config.program_id)?;
            check_new_account_absent(rpc, &new_stake)?;
            let delegation = vote_account
                .map(|vote| format!(", delegated to vote account {vote}"))
                .unwrap_or_default();
            PreparedStakeOperation {
                instructions: create_stake_account_instructions(
                    *vault,
                    new_stake,
                    lamports,
                    vote_account,
                ),
                description: format!(
                    "Create stake account {new_stake} with {} SOL from vault {vault} (staker and \
                     withdrawer = vault){delegation}",
                    format_sol(lamports)
                ),
                external: None,
                ephemeral: Some(ephemeral),
                new_stake_account: Some(new_stake),
            }
        }
        StakeOperation::Delegate {
            stake,
            vote_account,
        } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            check_vault_authority(&snapshot, vault, vault_index, RequiredAuthority::Staker)?;
            check_vote_account_owner(rpc, &vote_account)?;
            let verb = if snapshot.state.delegation().is_some() {
                "Redelegate"
            } else {
                "Delegate"
            };
            PreparedStakeOperation::simple(
                build_delegate_instruction(stake, vote_account, *vault),
                format!("{verb} stake account {stake} to vote account {vote_account}"),
            )
        }
        StakeOperation::Deactivate { stake } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            check_vault_authority(&snapshot, vault, vault_index, RequiredAuthority::Staker)?;
            check_can_deactivate(&snapshot)?;
            PreparedStakeOperation::simple(
                build_deactivate_instruction(stake, *vault),
                format!("Deactivate stake account {stake}"),
            )
        }
        StakeOperation::Withdraw {
            stake,
            recipient,
            amount,
        } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            let meta = check_vault_authority(
                &snapshot,
                vault,
                vault_index,
                RequiredAuthority::Withdrawer,
            )?;
            check_no_lockup(&stake, &meta, &fetch_clock(rpc)?, "Withdrawing")?;
            let lamports = resolve_withdraw_amount(amount, &snapshot)?;
            let closing = if lamports == snapshot.lamports {
                " (closes the account)"
            } else {
                ""
            };
            PreparedStakeOperation::simple(
                build_withdraw_instruction(stake, recipient, *vault, lamports),
                format!(
                    "Withdraw {} SOL from stake account {stake} to {recipient}{closing}",
                    format_sol(lamports)
                ),
            )
        }
        StakeOperation::Split { stake, lamports } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            check_vault_authority(&snapshot, vault, vault_index, RequiredAuthority::Staker)?;
            let reserve = rpc.get_minimum_balance_for_rent_exemption(STAKE_ACCOUNT_DATA_LEN)?;
            check_split_funding(
                lamports,
                snapshot.lamports,
                reserve,
                vault_balance(rpc, vault)?,
                rpc.get_minimum_balance_for_rent_exemption(0)?,
            )?;
            let (destination, ephemeral) =
                new_stake_account_address(rpc, &req.multisig, &config.program_id)?;
            check_new_account_absent(rpc, &destination)?;
            PreparedStakeOperation {
                instructions: split_stake_instructions(
                    *vault,
                    stake,
                    destination,
                    lamports,
                    reserve,
                ),
                description: format!(
                    "Split {} SOL from stake account {stake} into new stake account \
                     {destination} (vault pays its {} SOL rent-exempt reserve)",
                    format_sol(lamports),
                    format_sol(reserve)
                ),
                external: None,
                ephemeral: Some(ephemeral),
                new_stake_account: Some(destination),
            }
        }
        StakeOperation::Merge {
            destination,
            source,
        } => {
            if destination == source {
                return Err(MsigError::Usage(
                    "merge destination and source must be different stake accounts".into(),
                ));
            }
            let destination_snapshot = fetch_stake_account(rpc, &destination)?;
            let destination_meta = check_vault_authority(
                &destination_snapshot,
                vault,
                vault_index,
                RequiredAuthority::Staker,
            )?;
            let source_snapshot = fetch_stake_account(rpc, &source)?;
            let source_meta = check_vault_authority(
                &source_snapshot,
                vault,
                vault_index,
                RequiredAuthority::Staker,
            )?;
            check_merge_compatible(&destination, &destination_meta, &source, &source_meta)?;
            PreparedStakeOperation::simple(
                build_merge_instruction(destination, source, *vault),
                format!("Merge stake account {source} into {destination}"),
            )
        }
        StakeOperation::SetStaker { stake, new_staker } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            check_vault_authority(
                &snapshot,
                vault,
                vault_index,
                RequiredAuthority::StakerOrWithdrawer,
            )?;
            PreparedStakeOperation::simple(
                build_authorize_instruction(stake, *vault, new_staker, StakeAuthorize::Staker),
                format!("Set staker of stake account {stake} to {new_staker}"),
            )
        }
        StakeOperation::SetWithdrawer {
            stake,
            new_withdrawer,
        } => {
            let snapshot = fetch_stake_account(rpc, &stake)?;
            let meta = check_vault_authority(
                &snapshot,
                vault,
                vault_index,
                RequiredAuthority::Withdrawer,
            )?;
            check_no_lockup(&stake, &meta, &fetch_clock(rpc)?, "Changing the withdrawer")?;
            let vaults = vault_addresses(&req.multisig, &config.program_id);
            let external = is_external_withdrawer(&new_withdrawer, &vaults).then(|| {
                format!(
                    "new withdrawer {new_withdrawer} is not a vault of multisig {}",
                    req.multisig
                )
            });
            PreparedStakeOperation {
                external,
                ..PreparedStakeOperation::simple(
                    build_authorize_instruction(
                        stake,
                        *vault,
                        new_withdrawer,
                        StakeAuthorize::Withdrawer,
                    ),
                    format!("Set withdrawer of stake account {stake} to {new_withdrawer}"),
                )
            }
        }
    };
    Ok(prepared)
}

/// Create one vault-transaction proposal for a stake account operation.
pub fn create_stake_account_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    config: &Config,
    req: &StakeProposalRequest<'_>,
) -> Result<StakeProposalResult, MsigError> {
    let (vault, _) = pda::vault_pda(&req.multisig, req.vault_index, &config.program_id);
    let PreparedStakeOperation {
        instructions,
        mut description,
        external,
        ephemeral,
        new_stake_account,
    } = prepare_operation(rpc, config, req, &vault)?;

    if let Some(reason) = external {
        if !req.allow_external_authority {
            return Err(MsigError::Usage(format!(
                "{reason}. This hands control outside the multisig; pass \
                 --allow-external-authority to propose it anyway."
            )));
        }
        eprintln!(
            "Warning: {reason}. Proposing anyway because --allow-external-authority was passed."
        );
        description.push_str(&format!(" [EXTERNAL AUTHORITY: {reason}]"));
    }

    simulate_inner_instructions(
        rpc,
        &signer.pubkey(),
        &instructions,
        req.allow_simulation_failure,
    )?;

    let proposal = match ephemeral {
        Some(ephemeral) => create_vault_transaction_proposal_with_ephemeral_signers(
            rpc,
            signer,
            &req.multisig,
            instructions,
            req.vault_index,
            ephemeral,
            description,
            req.memo,
            config,
            req.dry_run,
            req.skip_confirm,
            req.quiet,
        )?,
        None => create_vault_transaction_proposal(
            rpc,
            signer,
            &req.multisig,
            instructions,
            req.vault_index,
            description,
            req.memo,
            config,
            req.dry_run,
            req.skip_confirm,
            req.quiet,
        )?,
    };
    Ok(StakeProposalResult {
        proposal,
        new_stake_account,
    })
}

/// Read-only summary of a stake account. `vaults` labels the staker and
/// withdrawer when they are vaults of the active multisig.
pub fn fetch_stake_account_info(
    rpc: &dyn RpcProvider,
    stake: &Pubkey,
    vaults: Option<&HashMap<Pubkey, u8>>,
) -> Result<StakeAccountInfo, MsigError> {
    let snapshot = fetch_stake_account(rpc, stake)?;
    let clock = fetch_clock(rpc)?;
    Ok(stake_account_info(&snapshot, &clock, vaults))
}

fn stake_account_info(
    snapshot: &StakeAccountSnapshot,
    clock: &Clock,
    vaults: Option<&HashMap<Pubkey, u8>>,
) -> StakeAccountInfo {
    let meta = snapshot.state.meta();
    let vault_of = |pk: &Pubkey| vaults.and_then(|v| v.get(pk).copied());
    StakeAccountInfo {
        stake_account: snapshot.address,
        state: snapshot.state.label().to_string(),
        balance_lamports: snapshot.lamports,
        rent_exempt_reserve_lamports: meta.map(|m| m.rent_exempt_reserve),
        staker: meta.map(|m| m.staker),
        staker_vault_index: meta.and_then(|m| vault_of(&m.staker)),
        withdrawer: meta.map(|m| m.withdrawer),
        withdrawer_vault_index: meta.and_then(|m| vault_of(&m.withdrawer)),
        lockup: meta.map(|m| StakeLockupInfo {
            unix_timestamp: m.lockup.unix_timestamp,
            epoch: m.lockup.epoch,
            custodian: m.lockup.custodian,
            in_force: m.lockup.is_in_force(clock),
        }),
        delegation: snapshot.state.delegation().map(|d| StakeDelegationInfo {
            vote_account: d.voter,
            stake_lamports: d.stake,
            activation_epoch: d.activation_epoch,
            deactivation_epoch: d.is_deactivating().then_some(d.deactivation_epoch),
        }),
        current_epoch: clock.epoch,
    }
}

// ---------------------------------------------------------------------------
// Review decoding (proposal show and the create preview)
// ---------------------------------------------------------------------------

const UNRECOGNISED: &str = "Stake: unrecognised instruction";

fn other_instruction_name(tag: u32) -> Option<&'static str> {
    match tag {
        IX_GET_MINIMUM_DELEGATION => Some("GetMinimumDelegation"),
        IX_DEACTIVATE_DELINQUENT => Some("DeactivateDelinquent"),
        IX_REDELEGATE => Some("Redelegate"),
        _ => None,
    }
}

/// Decode a Stake program instruction. `vaults` is the proposal multisig's
/// vault set; when given, withdrawer authorities outside it are flagged and
/// vault recipients are labelled. `accounts` are the instruction's account
/// keys in order.
pub fn decode_stake_instruction(
    data: &[u8],
    accounts: &[Pubkey],
    vaults: Option<&HashMap<Pubkey, u8>>,
    truncate: bool,
) -> DecodedInstruction {
    let addr = |pk: &Pubkey| format_addr(&pk.to_string(), truncate);
    let acct = |idx: usize| accounts.get(idx).map(addr).unwrap_or_else(|| "?".into());
    let vault_label = |idx: usize| {
        accounts
            .get(idx)
            .and_then(|pk| vaults.and_then(|vaults| vaults.get(pk)))
            .map(|index| format!(" (vault {index})"))
            .unwrap_or_default()
    };
    let mut warnings = Vec::new();

    let text = (|| {
        let tag = read_u32(data, 0)?;
        match tag {
            IX_INITIALIZE => {
                // Authorized { staker, withdrawer }, Lockup { unix_timestamp, epoch, custodian }
                let staker = read_pubkey(data, 4)?;
                let withdrawer = read_pubkey(data, 36)?;
                let lockup_timestamp = read_i64(data, 68)?;
                let lockup_epoch = read_u64(data, 76)?;
                let custodian = read_pubkey(data, 84)?;
                warn_external_withdrawer(
                    "withdrawer",
                    &withdrawer,
                    vaults,
                    &mut warnings,
                    truncate,
                );
                let lockup = if lockup_timestamp != 0 || lockup_epoch != 0 {
                    warnings.push(format!(
                        "sets a lockup (epoch {lockup_epoch}, unix time {lockup_timestamp}, \
                         custodian {}) on the new stake account",
                        addr(&custodian)
                    ));
                    format!(
                        ", lockup epoch={lockup_epoch} unix_time={lockup_timestamp} custodian={}",
                        addr(&custodian)
                    )
                } else {
                    String::new()
                };
                Some(format!(
                    "Stake Initialize {}: staker={}, withdrawer={}{lockup}",
                    acct(0),
                    addr(&staker),
                    addr(&withdrawer)
                ))
            }
            IX_INITIALIZE_CHECKED => {
                // [stake, rent, staker, withdrawer (signer)]
                let withdrawer = accounts.get(3)?;
                warn_external_withdrawer("withdrawer", withdrawer, vaults, &mut warnings, truncate);
                Some(format!(
                    "Stake InitializeChecked {}: staker={}, withdrawer={}",
                    acct(0),
                    acct(2),
                    addr(withdrawer)
                ))
            }
            IX_AUTHORIZE => {
                let new_authority = read_pubkey(data, 4)?;
                let role = StakeAuthorize::from_tag(read_u32(data, 36)?)?;
                Some(authorize_text(
                    "Authorize",
                    role,
                    &acct(0),
                    &new_authority,
                    vaults,
                    &mut warnings,
                    truncate,
                ))
            }
            IX_AUTHORIZE_CHECKED => {
                // [stake, clock, current authority, new authority (signer)]
                let role = StakeAuthorize::from_tag(read_u32(data, 4)?)?;
                let new_authority = *accounts.get(3)?;
                Some(authorize_text(
                    "AuthorizeChecked",
                    role,
                    &acct(0),
                    &new_authority,
                    vaults,
                    &mut warnings,
                    truncate,
                ))
            }
            IX_AUTHORIZE_WITH_SEED => {
                // { new_authorized_pubkey, stake_authorize, authority_seed, authority_owner }
                let new_authority = read_pubkey(data, 4)?;
                let role = StakeAuthorize::from_tag(read_u32(data, 36)?)?;
                Some(authorize_text(
                    "AuthorizeWithSeed",
                    role,
                    &acct(0),
                    &new_authority,
                    vaults,
                    &mut warnings,
                    truncate,
                ))
            }
            IX_AUTHORIZE_CHECKED_WITH_SEED => {
                // { stake_authorize, authority_seed, authority_owner };
                // [stake, base, clock, new authority (signer)]
                let role = StakeAuthorize::from_tag(read_u32(data, 4)?)?;
                let new_authority = *accounts.get(3)?;
                Some(authorize_text(
                    "AuthorizeCheckedWithSeed",
                    role,
                    &acct(0),
                    &new_authority,
                    vaults,
                    &mut warnings,
                    truncate,
                ))
            }
            IX_DELEGATE_STAKE => Some(format!(
                "Stake DelegateStake {} to vote account {}, authority={}",
                acct(0),
                acct(1),
                acct(5)
            )),
            IX_SPLIT => {
                let lamports = read_u64(data, 4)?;
                Some(format!(
                    "Stake Split {} SOL from {} to {}, authority={}",
                    format_sol(lamports),
                    acct(0),
                    acct(1),
                    acct(2)
                ))
            }
            IX_WITHDRAW => {
                let lamports = read_u64(data, 4)?;
                Some(format!(
                    "Stake Withdraw {} SOL from {} to {}{}, authority={}",
                    format_sol(lamports),
                    acct(0),
                    acct(1),
                    vault_label(1),
                    acct(4)
                ))
            }
            IX_DEACTIVATE => Some(format!(
                "Stake Deactivate {}, authority={}",
                acct(0),
                acct(2)
            )),
            IX_MERGE => Some(format!(
                "Stake Merge {} into {}, authority={}",
                acct(1),
                acct(0),
                acct(4)
            )),
            IX_SET_LOCKUP | IX_SET_LOCKUP_CHECKED => {
                let name = if tag == IX_SET_LOCKUP {
                    "SetLockup"
                } else {
                    "SetLockupChecked"
                };
                warnings.push(format!(
                    "changes the lockup or custodian of stake account {}",
                    acct(0)
                ));
                Some(format!("Stake {name} on {}", acct(0)))
            }
            IX_MOVE_STAKE | IX_MOVE_LAMPORTS => {
                let lamports = read_u64(data, 4)?;
                let name = if tag == IX_MOVE_STAKE {
                    "MoveStake"
                } else {
                    "MoveLamports"
                };
                Some(format!(
                    "Stake {name} {} SOL from {} to {}",
                    format_sol(lamports),
                    acct(0),
                    acct(1)
                ))
            }
            other => other_instruction_name(other).map(|name| format!("Stake {name}")),
        }
    })();

    match text {
        Some(text) => DecodedInstruction { text, warnings },
        None => DecodedInstruction {
            text: UNRECOGNISED.to_string(),
            warnings: vec![
                "unrecognised Stake program instruction; review the raw data with --verbose"
                    .to_string(),
            ],
        },
    }
}

fn authorize_text(
    name: &str,
    role: StakeAuthorize,
    stake: &str,
    new_authority: &Pubkey,
    vaults: Option<&HashMap<Pubkey, u8>>,
    warnings: &mut Vec<String>,
    truncate: bool,
) -> String {
    if role == StakeAuthorize::Withdrawer {
        warn_external_withdrawer("new withdrawer", new_authority, vaults, warnings, truncate);
    }
    format!(
        "Stake {name} {} on {stake}: new={}",
        role.label(),
        format_addr(&new_authority.to_string(), truncate)
    )
}

fn warn_external_withdrawer(
    what: &str,
    withdrawer: &Pubkey,
    vaults: Option<&HashMap<Pubkey, u8>>,
    warnings: &mut Vec<String>,
    truncate: bool,
) {
    if let Some(vaults) = vaults {
        if is_external_withdrawer(withdrawer, vaults) {
            warnings.push(format!(
                "{what} {} is not a vault of this multisig",
                format_addr(&withdrawer.to_string(), truncate)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::infra::accounts::discriminators;
    use crate::infra::accounts::stake::tests::{
        clock, sample_delegation, sample_meta, stake_account_bytes,
    };
    use crate::infra::accounts::stake::{Delegation, Lockup, NOT_DEACTIVATING};
    use crate::infra::accounts::vote::tests::hex;
    use crate::infra::rpc::types::{SimulationResult, TxStatus};

    fn pk(s: &str) -> Pubkey {
        s.parse().unwrap_or_else(|e| panic!("{e:?}"))
    }

    // Pubkeys used for the solana CLI 4.3 `--dump-transaction-message` fixtures.
    fn authority() -> Pubkey {
        pk("B6bgbkE67GcoPjXoeV6rt8Fprj3BjdJrzhNP6qWVU32G")
    }
    fn vote() -> Pubkey {
        pk("3mZFQNXybi8rdSShmGETLerdheV8vHZTKcYuXBnrjseH")
    }
    fn recipient() -> Pubkey {
        pk("4rWPt5UwRg4Dc5nxKf9Ld2s2LSSjTdBrpLEHAhuuvoEB")
    }
    fn new_authority() -> Pubkey {
        pk("3vYr3QnjjxouirYjAcdZLwVKWafhQCXHmqfYorQYC6nn")
    }
    fn stake() -> Pubkey {
        pk("6zGSMU5hg8t3oysVKk1R8CbEuc7QkbfMfkBNqXBdu5T1")
    }
    fn stake2() -> Pubkey {
        pk("6dHf2KpVto5apWXp2DCaTJNfrkASn2MaCiArLvDmemxj")
    }
    fn split_destination() -> Pubkey {
        pk("GrqLSbrJr2pZoPmq36qxvLXDCLsE12kWPiRGgbFZCHBj")
    }
    fn new_stake() -> Pubkey {
        pk("DA4H2auNqZLywhqRM68grMpw9LsqTxLGXsepWuMCvu8t")
    }

    const STAKE_PROGRAM_HEX: &str =
        "06a1d8179137542a983437bdfe2a7ab2557f535c8a78722b68a49dc000000000";

    fn metas(ix: &Instruction) -> Vec<(Pubkey, bool, bool)> {
        ix.accounts
            .iter()
            .map(|m| (m.pubkey, m.is_writable, m.is_signer))
            .collect()
    }

    fn keys(ix: &Instruction) -> Vec<Pubkey> {
        ix.accounts.iter().map(|m| m.pubkey).collect()
    }

    /// Message-level (writable, signer) flags: the union over all
    /// instructions, which is what the solana CLI dump shows.
    fn merged_flags(ixs: &[Instruction], key: &Pubkey) -> (bool, bool) {
        ixs.iter()
            .flat_map(|ix| ix.accounts.iter())
            .filter(|m| m.pubkey == *key)
            .fold((false, false), |(w, s), m| {
                (w || m.is_writable, s || m.is_signer)
            })
    }

    #[test]
    fn program_id_bytes_match_dump() {
        assert_eq!(STAKE_PROGRAM_ID.as_ref(), hex(STAKE_PROGRAM_HEX).as_slice());
    }

    #[test]
    fn delegate_matches_cli_dump() {
        let ix = build_delegate_instruction(stake(), vote(), authority());
        assert_eq!(ix.program_id, STAKE_PROGRAM_ID);
        assert_eq!(ix.data, hex("02000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (stake(), true, false),
                (vote(), false, false),
                (SYSVAR_CLOCK, false, false),
                (SYSVAR_STAKE_HISTORY, false, false),
                (STAKE_CONFIG, false, false),
                (authority(), false, true),
            ]
        );
    }

    #[test]
    fn deactivate_matches_cli_dump() {
        let ix = build_deactivate_instruction(stake(), authority());
        assert_eq!(ix.data, hex("05000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (stake(), true, false),
                (SYSVAR_CLOCK, false, false),
                (authority(), false, true),
            ]
        );
    }

    #[test]
    fn withdraw_matches_cli_dump() {
        let ix = build_withdraw_instruction(stake(), recipient(), authority(), 1_500_000_000);
        assert_eq!(ix.data, hex("04000000002f685900000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (stake(), true, false),
                (recipient(), true, false),
                (SYSVAR_CLOCK, false, false),
                (SYSVAR_STAKE_HISTORY, false, false),
                (authority(), false, true),
            ]
        );
    }

    #[test]
    fn merge_matches_cli_dump() {
        let ix = build_merge_instruction(stake(), stake2(), authority());
        assert_eq!(ix.data, hex("07000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (stake(), true, false),
                (stake2(), true, false),
                (SYSVAR_CLOCK, false, false),
                (SYSVAR_STAKE_HISTORY, false, false),
                (authority(), false, true),
            ]
        );
    }

    #[test]
    fn authorize_matches_cli_dump() {
        let key_hex = "2b6f2a238faf9a5f1a6624fdb72a08ed698a85d8167f475510e80b4b1859025b";
        for (role, role_hex) in [
            (StakeAuthorize::Staker, "00000000"),
            (StakeAuthorize::Withdrawer, "01000000"),
        ] {
            let ix = build_authorize_instruction(stake(), authority(), new_authority(), role);
            assert_eq!(ix.program_id, STAKE_PROGRAM_ID);
            assert_eq!(ix.data, hex(&format!("01000000{key_hex}{role_hex}")));
            assert_eq!(
                metas(&ix),
                vec![
                    (stake(), true, false),
                    (SYSVAR_CLOCK, false, false),
                    (authority(), false, true),
                ]
            );
        }
    }

    #[test]
    fn split_matches_cli_dump() {
        let ixs = split_stake_instructions(
            authority(),
            stake(),
            split_destination(),
            1_500_000_000,
            2_282_880,
        );
        assert_eq!(ixs.len(), 4);
        let dest = split_destination();

        assert_eq!(ixs[0].program_id, SYSTEM_PROGRAM);
        assert_eq!(ixs[0].data, hex("0200000080d5220000000000"));
        assert_eq!(
            metas(&ixs[0]),
            vec![(authority(), true, true), (dest, true, false)]
        );

        assert_eq!(ixs[1].program_id, SYSTEM_PROGRAM);
        assert_eq!(ixs[1].data, hex("08000000c800000000000000"));
        assert_eq!(metas(&ixs[1]), vec![(dest, true, true)]);

        assert_eq!(ixs[2].program_id, SYSTEM_PROGRAM);
        assert_eq!(ixs[2].data, hex(&format!("01000000{STAKE_PROGRAM_HEX}")));
        assert_eq!(metas(&ixs[2]), vec![(dest, true, true)]);

        assert_eq!(ixs[3].program_id, STAKE_PROGRAM_ID);
        assert_eq!(ixs[3].data, hex("03000000002f685900000000"));
        assert_eq!(
            metas(&ixs[3]),
            vec![
                (stake(), true, false),
                (dest, true, false),
                (authority(), false, true)
            ]
        );

        // Message level, as in the dump: the destination is a writable signer.
        assert_eq!(merged_flags(&ixs, &dest), (true, true));
        assert_eq!(merged_flags(&ixs, &stake()), (true, false));
    }

    #[test]
    fn create_uses_transfer_allocate_assign_initialize() {
        // Encodings match the System fixtures of the split dump and the Stake
        // Initialize of the create-stake-account dump. The solana CLI itself
        // uses CreateAccount here; msig does not (see the builder's comment).
        let new = new_stake();
        let ixs = create_stake_account_instructions(authority(), new, 2_000_000_000, None);
        assert_eq!(ixs.len(), 4);
        // No System CreateAccount (tag 0).
        assert!(ixs
            .iter()
            .all(|ix| ix.program_id != SYSTEM_PROGRAM || ix.data[..4] != [0, 0, 0, 0]));

        assert_eq!(ixs[0].program_id, SYSTEM_PROGRAM);
        // Transfer 2 SOL (same layout as the split dump's Transfer).
        assert_eq!(ixs[0].data, hex("020000000094357700000000"));
        assert_eq!(
            ixs[0],
            build_system_transfer_instruction(authority(), new, 2_000_000_000)
        );
        assert_eq!(
            metas(&ixs[0]),
            vec![(authority(), true, true), (new, true, false)]
        );

        assert_eq!(ixs[1].program_id, SYSTEM_PROGRAM);
        assert_eq!(ixs[1].data, hex("08000000c800000000000000"));
        assert_eq!(metas(&ixs[1]), vec![(new, true, true)]);

        assert_eq!(ixs[2].program_id, SYSTEM_PROGRAM);
        assert_eq!(ixs[2].data, hex(&format!("01000000{STAKE_PROGRAM_HEX}")));
        assert_eq!(metas(&ixs[2]), vec![(new, true, true)]);

        assert_eq!(ixs[3].program_id, STAKE_PROGRAM_ID);
        let mut expected = hex("00000000");
        expected.extend_from_slice(authority().as_ref());
        expected.extend_from_slice(authority().as_ref());
        expected.extend_from_slice(&[0u8; 48]);
        assert_eq!(ixs[3].data, expected);
        // The create dump shows the authority as 9603fd3c...0e29.
        assert_eq!(&ixs[3].data[4..8], hex("9603fd3c").as_slice());
        assert_eq!(&ixs[3].data[34..36], hex("0e29").as_slice());
        assert_eq!(
            metas(&ixs[3]),
            vec![(new, true, false), (SYSVAR_RENT, false, false)]
        );

        // The new account signs (Allocate/Assign) at the message level.
        assert_eq!(merged_flags(&ixs, &new), (true, true));

        let ixs = create_stake_account_instructions(authority(), new, 2_000_000_000, Some(vote()));
        assert_eq!(ixs.len(), 5);
        assert_eq!(ixs[4], build_delegate_instruction(new, vote(), authority()));
    }

    #[test]
    fn transfer_matches_split_dump_encoding() {
        let ix = build_system_transfer_instruction(authority(), split_destination(), 2_282_880);
        assert_eq!(ix.data, hex("0200000080d5220000000000"));
    }

    // -- preflight ----------------------------------------------------------

    fn stake_data_account(state: &StakeAccountState, lamports: u64) -> AccountData {
        AccountData {
            lamports,
            data: stake_account_bytes(state),
            owner: STAKE_PROGRAM_ID.to_string(),
        }
    }

    fn snapshot(state: StakeAccountState, lamports: u64) -> StakeAccountSnapshot {
        parse_stake_account(&stake(), &stake_data_account(&state, lamports))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn meta_with(staker: Pubkey, withdrawer: Pubkey) -> StakeMeta {
        StakeMeta {
            staker,
            withdrawer,
            ..sample_meta()
        }
    }

    #[test]
    fn owner_check_refuses_non_stake_accounts() {
        let mut account = stake_data_account(&StakeAccountState::Initialized(sample_meta()), 5);
        account.owner = SYSTEM_PROGRAM.to_string();
        match parse_stake_account(&stake(), &account) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("not a stake account")),
            other => panic!("expected owner refusal, got {other:?}"),
        }
    }

    #[test]
    fn authority_checks_per_role() {
        let vault = Pubkey::new_from_array([7u8; 32]);
        let other = Pubkey::new_from_array([8u8; 32]);
        let check = |meta: StakeMeta, role| {
            check_vault_authority(
                &snapshot(StakeAccountState::Initialized(meta), 10),
                &vault,
                4,
                role,
            )
        };
        use RequiredAuthority::*;
        // Vault is the staker only.
        assert!(check(meta_with(vault, other), Staker).is_ok());
        assert!(check(meta_with(vault, other), StakerOrWithdrawer).is_ok());
        assert!(check(meta_with(vault, other), Withdrawer).is_err());
        // Vault is the withdrawer only.
        assert!(check(meta_with(other, vault), Withdrawer).is_ok());
        assert!(check(meta_with(other, vault), StakerOrWithdrawer).is_ok());
        assert!(check(meta_with(other, vault), Staker).is_err());
        // Neither.
        match check(meta_with(other, other), StakerOrWithdrawer) {
            Err(MsigError::Transaction(msg)) => {
                assert!(msg.contains("vault index 4"));
                assert!(msg.contains("staker or withdrawer"));
                assert!(msg.contains(&other.to_string()));
            }
            r => panic!("expected refusal, got {r:?}"),
        }
        // Delegated accounts carry the same meta.
        let delegated = snapshot(
            StakeAccountState::Stake {
                meta: meta_with(vault, vault),
                delegation: sample_delegation(),
                flags: 0,
            },
            10,
        );
        assert!(check_vault_authority(&delegated, &vault, 0, Staker).is_ok());
        // No authorities at all.
        for state in [
            StakeAccountState::Uninitialized,
            StakeAccountState::RewardsPool,
        ] {
            assert!(matches!(
                check_vault_authority(&snapshot(state, 10), &vault, 0, StakerOrWithdrawer),
                Err(MsigError::Transaction(_))
            ));
        }
    }

    #[test]
    fn lockup_refusal() {
        let mut meta = sample_meta();
        let now = clock(500, 1_700_000_000);
        assert!(check_no_lockup(&stake(), &meta, &now, "Withdrawing").is_ok());
        meta.lockup = Lockup {
            unix_timestamp: 0,
            epoch: 600,
            custodian: Pubkey::new_from_array([5u8; 32]),
        };
        match check_no_lockup(&stake(), &meta, &now, "Withdrawing") {
            Err(MsigError::Transaction(msg)) => {
                assert!(msg.contains("lockup in force") && msg.contains("custodian"))
            }
            other => panic!("expected lockup refusal, got {other:?}"),
        }
    }

    #[test]
    fn withdraw_rules_for_initialized_account() {
        let s = snapshot(StakeAccountState::Initialized(sample_meta()), 10_000_000);
        let ok = |a| resolve_withdraw_amount(a, &s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ok(StakeWithdrawAmount::All), 10_000_000);
        assert_eq!(
            ok(StakeWithdrawAmount::Lamports(10_000_000 - 2_282_880)),
            7_717_120
        );
        for bad in [
            StakeWithdrawAmount::Lamports(0),
            // Whole balance via --amount: must use --all.
            StakeWithdrawAmount::Lamports(10_000_000),
            StakeWithdrawAmount::Lamports(10_000_001),
            // Leaves less than the rent reserve.
            StakeWithdrawAmount::Lamports(7_717_121),
        ] {
            assert!(
                matches!(resolve_withdraw_amount(bad, &s), Err(MsigError::Usage(_))),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn withdraw_rules_for_delegated_account() {
        let meta = sample_meta();
        let active = Delegation {
            stake: 5_000_000_000,
            ..sample_delegation()
        };
        let balance = 5_000_000_000 + 2_282_880 + 1_000;
        let s = snapshot(
            StakeAccountState::Stake {
                meta,
                delegation: active,
                flags: 0,
            },
            balance,
        );
        // Only the excess above stake + reserve.
        assert_eq!(
            resolve_withdraw_amount(StakeWithdrawAmount::Lamports(1_000), &s).ok(),
            Some(1_000)
        );
        match resolve_withdraw_amount(StakeWithdrawAmount::Lamports(1_001), &s) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("deactivate")),
            other => panic!("expected refusal, got {other:?}"),
        }
        assert!(resolve_withdraw_amount(StakeWithdrawAmount::All, &s).is_err());

        // Deactivating: the cooldown is left to the simulation.
        let s = snapshot(
            StakeAccountState::Stake {
                meta,
                delegation: Delegation {
                    deactivation_epoch: 120,
                    ..active
                },
                flags: 0,
            },
            balance,
        );
        assert_eq!(
            resolve_withdraw_amount(StakeWithdrawAmount::All, &s).ok(),
            Some(balance)
        );
    }

    #[test]
    fn create_funding_rules() {
        let rent = 2_282_880;
        let min = 1_000_000_000;
        let v0 = RENT_0;
        assert!(check_create_funding(rent, rent, None, rent, v0).is_ok());
        assert!(check_create_funding(rent - 1, rent, None, u64::MAX, v0).is_err());
        assert!(check_create_funding(rent + min, rent, Some(min), rent + min, v0).is_ok());
        assert!(check_create_funding(rent + min - 1, rent, Some(min), u64::MAX, v0).is_err());
        match check_create_funding(2 * rent, rent, None, 2 * rent - 1, v0) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("vault holds")),
            other => panic!("expected vault balance refusal, got {other:?}"),
        }
        // The vault keeps nothing, or at least its own rent-exempt minimum.
        assert!(check_create_funding(rent, rent, None, rent + v0, v0).is_ok());
        match check_create_funding(rent, rent, None, rent + v0 - 1, v0) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("rent-exempt minimum")),
            other => panic!("expected vault rent refusal, got {other:?}"),
        }
        assert!(check_create_funding(rent, rent, None, rent + 1, v0).is_err());
    }

    #[test]
    fn split_funding_rules() {
        let reserve = 2_282_880;
        let v0 = RENT_0;
        assert!(check_split_funding(1, 10, reserve, reserve, v0).is_ok());
        assert!(check_split_funding(10, 10, reserve, reserve, v0).is_ok());
        assert!(check_split_funding(0, 10, reserve, reserve, v0).is_err());
        assert!(check_split_funding(11, 10, reserve, reserve, v0).is_err());
        match check_split_funding(1, 10, reserve, reserve - 1, v0) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("rent-exempt reserve")),
            other => panic!("expected vault balance refusal, got {other:?}"),
        }
        assert!(check_split_funding(1, 10, reserve, reserve + v0, v0).is_ok());
        match check_split_funding(1, 10, reserve, reserve + v0 - 1, v0) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("rent-exempt minimum")),
            other => panic!("expected vault rent refusal, got {other:?}"),
        }
    }

    #[test]
    fn merge_requires_same_authorities_and_lockup() {
        let a = Pubkey::new_from_array([7u8; 32]);
        let b = Pubkey::new_from_array([8u8; 32]);
        let base = meta_with(a, b);
        let ok = check_merge_compatible(&stake(), &base, &stake2(), &base);
        assert!(ok.is_ok());
        // Different rent reserves do not matter.
        let other_reserve = StakeMeta {
            rent_exempt_reserve: 1,
            ..base
        };
        assert!(check_merge_compatible(&stake(), &base, &stake2(), &other_reserve).is_ok());

        let refused = |source: StakeMeta, needle: &str| match check_merge_compatible(
            &stake(),
            &base,
            &stake2(),
            &source,
        ) {
            Err(MsigError::Transaction(msg)) => {
                assert!(
                    msg.contains("cannot be merged") && msg.contains(needle),
                    "{msg}"
                )
            }
            other => panic!("expected merge refusal, got {other:?}"),
        };
        refused(meta_with(b, b), "staker");
        refused(meta_with(a, a), "withdrawer");
        let mut locked = base;
        locked.lockup.epoch = 9;
        refused(locked, "lockup");
        let mut custodian = base;
        custodian.lockup.custodian = a;
        refused(custodian, "lockup");
    }

    #[test]
    fn deactivate_rules() {
        let meta = sample_meta();
        assert!(check_can_deactivate(&snapshot(StakeAccountState::Initialized(meta), 1)).is_err());
        let delegated = |deactivation_epoch| {
            snapshot(
                StakeAccountState::Stake {
                    meta,
                    delegation: Delegation {
                        deactivation_epoch,
                        ..sample_delegation()
                    },
                    flags: 0,
                },
                1,
            )
        };
        assert!(check_can_deactivate(&delegated(NOT_DEACTIVATING)).is_ok());
        match check_can_deactivate(&delegated(120)) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("already deactivating")),
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn clock_owner_check() {
        let mut data = vec![0u8; 40];
        data[16..24].copy_from_slice(&77u64.to_le_bytes());
        let mut account = AccountData {
            lamports: 1,
            data,
            owner: SYSVAR_OWNER.to_string(),
        };
        assert_eq!(
            parse_clock_account(&account).ok().map(|c| c.epoch),
            Some(77)
        );
        account.owner = SYSTEM_PROGRAM.to_string();
        assert!(parse_clock_account(&account).is_err());
    }

    // -- prepare_operation against a mock RPC -------------------------------

    struct MockRpc {
        accounts: HashMap<String, AccountData>,
        calls: RefCell<Vec<String>>,
    }

    const RENT_200: u64 = 2_282_880;
    const RENT_0: u64 = 890_880;
    const MIN_DELEGATION: u64 = 1_000_000_000;
    const MULTISIG_TX_INDEX: u64 = 41;

    impl MockRpc {
        fn new() -> Self {
            Self {
                accounts: HashMap::new(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, address: Pubkey, account: AccountData) -> Self {
            self.accounts.insert(address.to_string(), account);
            self
        }
    }

    fn unsupported<T>() -> Result<T, MsigError> {
        Err(MsigError::Transaction("not supported by the mock".into()))
    }

    impl RpcProvider for MockRpc {
        fn get_account_info(&self, pubkey: &str) -> Result<Option<AccountData>, MsigError> {
            self.calls.borrow_mut().push(pubkey.to_string());
            Ok(self.accounts.get(pubkey).cloned())
        }
        fn get_multiple_accounts(&self, _: &[&str]) -> Result<Vec<Option<AccountData>>, MsigError> {
            unsupported()
        }
        fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError> {
            unsupported()
        }
        fn get_minimum_balance_for_rent_exemption(
            &self,
            data_len: usize,
        ) -> Result<u64, MsigError> {
            match data_len {
                0 => Ok(RENT_0),
                200 => Ok(RENT_200),
                other => panic!("unexpected rent query for {other} bytes"),
            }
        }
        fn get_stake_minimum_delegation(&self) -> Result<u64, MsigError> {
            Ok(MIN_DELEGATION)
        }
        fn send_transaction(&self, _: &str) -> Result<String, MsigError> {
            unsupported()
        }
        fn simulate_transaction(&self, _: &str) -> Result<SimulationResult, MsigError> {
            unsupported()
        }
        fn get_signature_statuses(&self, _: &[&str]) -> Result<Vec<Option<TxStatus>>, MsigError> {
            unsupported()
        }
    }

    fn multisig_account() -> AccountData {
        multisig_account_with_members(&[])
    }

    fn multisig_account_with_members(members: &[Pubkey]) -> AccountData {
        let mut data = Vec::new();
        data.extend_from_slice(&discriminators::MULTISIG);
        data.extend_from_slice(&[1u8; 32]); // create_key
        data.extend_from_slice(&[0u8; 32]); // config_authority
        data.extend_from_slice(&1u16.to_le_bytes()); // threshold
        data.extend_from_slice(&0u32.to_le_bytes()); // time_lock
        data.extend_from_slice(&MULTISIG_TX_INDEX.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // stale_transaction_index
        data.push(0); // rent_collector = None
        data.push(255); // bump
        data.extend_from_slice(&u32::try_from(members.len()).unwrap_or(0).to_le_bytes());
        for member in members {
            data.extend_from_slice(member.as_ref());
            data.push(0b111); // initiate, vote, execute
        }
        AccountData {
            lamports: 1,
            data,
            owner: pda::PROGRAM_ID.to_string(),
        }
    }

    fn system_account(lamports: u64) -> AccountData {
        AccountData {
            lamports,
            data: vec![],
            owner: SYSTEM_PROGRAM.to_string(),
        }
    }

    fn vote_owned_account() -> AccountData {
        AccountData {
            lamports: 1,
            data: vec![],
            owner: VOTE_PROGRAM_ID.to_string(),
        }
    }

    fn clock_account(epoch: u64) -> AccountData {
        let mut data = vec![0u8; 40];
        data[16..24].copy_from_slice(&epoch.to_le_bytes());
        AccountData {
            lamports: 1,
            data,
            owner: SYSVAR_OWNER.to_string(),
        }
    }

    fn multisig() -> Pubkey {
        Pubkey::new_from_array([11u8; 32])
    }

    fn vault0() -> Pubkey {
        pda::vault_pda(&multisig(), 0, &pda::PROGRAM_ID).0
    }

    fn request(operation: StakeOperation) -> StakeProposalRequest<'static> {
        StakeProposalRequest {
            multisig: multisig(),
            vault_index: 0,
            operation,
            allow_external_authority: false,
            allow_simulation_failure: false,
            memo: None,
            dry_run: true,
            skip_confirm: true,
            quiet: true,
        }
    }

    fn prepare(
        rpc: &MockRpc,
        operation: StakeOperation,
    ) -> Result<PreparedStakeOperation, MsigError> {
        prepare_operation(rpc, &Config::default(), &request(operation), &vault0())
    }

    /// Ephemeral signer PDA 0 of the multisig's next vault transaction.
    fn expected_ephemeral() -> Pubkey {
        let (transaction, _) =
            pda::transaction_pda(&multisig(), MULTISIG_TX_INDEX + 1, &pda::PROGRAM_ID);
        pda::ephemeral_signer_pda(&transaction, 0, &pda::PROGRAM_ID).0
    }

    #[test]
    fn create_uses_ephemeral_signer_of_next_transaction() {
        let rpc = MockRpc::new()
            .with(multisig(), multisig_account())
            .with(vault0(), system_account(10_000_000_000))
            .with(vote(), vote_owned_account());
        let lamports = RENT_200 + MIN_DELEGATION;
        let prepared = prepare(
            &rpc,
            StakeOperation::Create {
                lamports,
                vote_account: Some(vote()),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let new_stake = expected_ephemeral();
        assert_eq!(prepared.new_stake_account, Some(new_stake));
        assert_eq!(
            prepared.ephemeral,
            Some(EphemeralSigners {
                count: 1,
                transaction_index: MULTISIG_TX_INDEX + 1,
            })
        );
        assert_eq!(
            prepared.instructions,
            create_stake_account_instructions(vault0(), new_stake, lamports, Some(vote()))
        );
        assert!(prepared.description.contains(&new_stake.to_string()));

        // Below the minimum delegation, or more than the vault holds.
        assert!(prepare(
            &rpc,
            StakeOperation::Create {
                lamports: lamports - 1,
                vote_account: Some(vote()),
            },
        )
        .is_err());
        assert!(prepare(
            &rpc,
            StakeOperation::Create {
                lamports: 10_000_000_001,
                vote_account: None,
            },
        )
        .is_err());
        // Delegating to an account the Vote program does not own.
        assert!(prepare(
            &rpc,
            StakeOperation::Create {
                lamports,
                vote_account: Some(recipient()),
            },
        )
        .is_err());
    }

    #[test]
    fn create_refuses_existing_ephemeral_account() {
        let rpc = MockRpc::new()
            .with(multisig(), multisig_account())
            .with(vault0(), system_account(10_000_000_000))
            .with(expected_ephemeral(), system_account(1));
        match prepare(
            &rpc,
            StakeOperation::Create {
                lamports: RENT_200,
                vote_account: None,
            },
        ) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("already holds an account")),
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn split_uses_ephemeral_signer_and_checks_staker_first() {
        let vault = vault0();
        let staked = StakeAccountState::Stake {
            meta: meta_with(vault, vault),
            delegation: sample_delegation(),
            flags: 0,
        };
        let rpc = MockRpc::new()
            .with(multisig(), multisig_account())
            .with(vault, system_account(RENT_200))
            .with(stake(), stake_data_account(&staked, 6_000_000_000));
        let prepared = prepare(
            &rpc,
            StakeOperation::Split {
                stake: stake(),
                lamports: 1_500_000_000,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let destination = expected_ephemeral();
        assert_eq!(prepared.new_stake_account, Some(destination));
        assert_eq!(
            prepared.instructions,
            split_stake_instructions(vault, stake(), destination, 1_500_000_000, RENT_200)
        );

        // A vault that is not the staker is refused before anything else is read.
        let other = Pubkey::new_from_array([8u8; 32]);
        let not_staker = StakeAccountState::Initialized(meta_with(other, vault));
        let rpc = MockRpc::new()
            .with(multisig(), multisig_account())
            .with(stake(), stake_data_account(&not_staker, 6_000_000_000));
        assert!(prepare(
            &rpc,
            StakeOperation::Split {
                stake: stake(),
                lamports: 1,
            },
        )
        .is_err());
        assert_eq!(*rpc.calls.borrow(), vec![stake().to_string()]);
    }

    #[test]
    fn simple_verbs_build_one_instruction_with_vault_authority() {
        let vault = vault0();
        let other = Pubkey::new_from_array([8u8; 32]);
        let staked = StakeAccountState::Stake {
            meta: meta_with(vault, vault),
            delegation: sample_delegation(),
            flags: 0,
        };
        let rpc = MockRpc::new()
            .with(stake(), stake_data_account(&staked, 6_000_000_000))
            .with(stake2(), stake_data_account(&staked, 1_000_000_000))
            .with(vote(), vote_owned_account())
            .with(SYSVAR_CLOCK, clock_account(500));

        let one = |op| {
            let p = prepare(&rpc, op).unwrap_or_else(|e| panic!("{e}"));
            assert!(p.ephemeral.is_none() && p.new_stake_account.is_none());
            assert_eq!(p.instructions.len(), 1);
            (p.instructions[0].clone(), p.external, p.description)
        };

        let (ix, _, description) = one(StakeOperation::Delegate {
            stake: stake(),
            vote_account: vote(),
        });
        assert_eq!(ix, build_delegate_instruction(stake(), vote(), vault));
        assert!(description.starts_with("Redelegate"));

        let (ix, ..) = one(StakeOperation::Deactivate { stake: stake() });
        assert_eq!(ix, build_deactivate_instruction(stake(), vault));

        let (ix, ..) = one(StakeOperation::Merge {
            destination: stake(),
            source: stake2(),
        });
        assert_eq!(ix, build_merge_instruction(stake(), stake2(), vault));

        let (ix, external, _) = one(StakeOperation::SetStaker {
            stake: stake(),
            new_staker: other,
        });
        assert_eq!(
            ix,
            build_authorize_instruction(stake(), vault, other, StakeAuthorize::Staker)
        );
        assert!(external.is_none());

        let (ix, external, _) = one(StakeOperation::SetWithdrawer {
            stake: stake(),
            new_withdrawer: other,
        });
        assert_eq!(
            ix,
            build_authorize_instruction(stake(), vault, other, StakeAuthorize::Withdrawer)
        );
        assert!(external.is_some_and(|reason| reason.contains("not a vault")));

        let vault1 = pda::vault_pda(&multisig(), 1, &pda::PROGRAM_ID).0;
        let (_, external, _) = one(StakeOperation::SetWithdrawer {
            stake: stake(),
            new_withdrawer: vault1,
        });
        assert!(external.is_none());

        // Merging an account into itself is refused.
        assert!(matches!(
            prepare(
                &rpc,
                StakeOperation::Merge {
                    destination: stake(),
                    source: stake(),
                },
            ),
            Err(MsigError::Usage(_))
        ));
    }

    #[test]
    fn merge_refuses_mismatched_withdrawer_before_proposing() {
        let vault = vault0();
        let other = Pubkey::new_from_array([8u8; 32]);
        let rpc = MockRpc::new()
            .with(
                stake(),
                stake_data_account(&StakeAccountState::Initialized(meta_with(vault, vault)), 5),
            )
            .with(
                stake2(),
                stake_data_account(&StakeAccountState::Initialized(meta_with(vault, other)), 5),
            );
        match prepare(
            &rpc,
            StakeOperation::Merge {
                destination: stake(),
                source: stake2(),
            },
        ) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("withdrawer"), "{msg}"),
            other => panic!("expected merge refusal, got {other:?}"),
        }
    }

    #[test]
    fn withdraw_and_set_withdrawer_refuse_lockup_in_force() {
        let vault = vault0();
        let mut meta = meta_with(vault, vault);
        meta.lockup.epoch = 501;
        let rpc = MockRpc::new()
            .with(
                stake(),
                stake_data_account(&StakeAccountState::Initialized(meta), 6_000_000_000),
            )
            .with(SYSVAR_CLOCK, clock_account(500));
        for op in [
            StakeOperation::Withdraw {
                stake: stake(),
                recipient: recipient(),
                amount: StakeWithdrawAmount::All,
            },
            StakeOperation::SetWithdrawer {
                stake: stake(),
                new_withdrawer: vault,
            },
        ] {
            match prepare(&rpc, op) {
                Err(MsigError::Transaction(msg)) => assert!(msg.contains("lockup"), "{msg}"),
                other => panic!("expected lockup refusal, got {other:?}"),
            }
        }
        // Expired lockup: allowed.
        let rpc = MockRpc::new()
            .with(
                stake(),
                stake_data_account(&StakeAccountState::Initialized(meta), 6_000_000_000),
            )
            .with(SYSVAR_CLOCK, clock_account(501));
        let prepared = prepare(
            &rpc,
            StakeOperation::Withdraw {
                stake: stake(),
                recipient: recipient(),
                amount: StakeWithdrawAmount::All,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            prepared.instructions,
            vec![build_withdraw_instruction(
                stake(),
                recipient(),
                vault,
                6_000_000_000
            )]
        );
        assert!(prepared.description.contains("closes the account"));
    }

    struct TestSigner(Pubkey);

    impl Signer for TestSigner {
        fn pubkey(&self) -> Pubkey {
            self.0
        }
        fn sign(&self, _: &[u8]) -> Result<Vec<u8>, MsigError> {
            unsupported()
        }
    }

    #[test]
    fn proposal_refuses_when_transaction_index_moved() {
        let member = Pubkey::new_from_array([12u8; 32]);
        let rpc = MockRpc::new().with(multisig(), multisig_account_with_members(&[member]));
        // Derived for an index that is no longer the next one.
        let stale_index = MULTISIG_TX_INDEX;
        let ephemeral = ephemeral_signer_address(&multisig(), stale_index, 0, &pda::PROGRAM_ID);
        let instructions = create_stake_account_instructions(vault0(), ephemeral, RENT_200, None);
        let result = create_vault_transaction_proposal_with_ephemeral_signers(
            &rpc,
            &TestSigner(member),
            &multisig(),
            instructions,
            0,
            EphemeralSigners {
                count: 1,
                transaction_index: stale_index,
            },
            "test".into(),
            None,
            &Config::default(),
            true,
            true,
            true,
        );
        match result {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("moved from"), "{msg}"),
            other => panic!("expected index refusal, got {other:?}"),
        }

        // A signer other than the vault and the declared ephemeral PDA is refused.
        let stranger = Pubkey::new_from_array([13u8; 32]);
        let instructions = create_stake_account_instructions(vault0(), stranger, RENT_200, None);
        let result = create_vault_transaction_proposal_with_ephemeral_signers(
            &rpc,
            &TestSigner(member),
            &multisig(),
            instructions,
            0,
            EphemeralSigners {
                count: 1,
                transaction_index: MULTISIG_TX_INDEX + 1,
            },
            "test".into(),
            None,
            &Config::default(),
            true,
            true,
            true,
        );
        match result {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("marks"), "{msg}"),
            other => panic!("expected signer refusal, got {other:?}"),
        }
    }

    // -- review decoding ----------------------------------------------------

    fn vaults_of(multisig: &Pubkey) -> HashMap<Pubkey, u8> {
        vault_addresses(multisig, &pda::PROGRAM_ID)
    }

    #[test]
    fn decodes_operations_built_by_msig() {
        let ms = Pubkey::new_from_array([7u8; 32]);
        let vaults = vaults_of(&ms);
        let vault2 = pda::vault_pda(&ms, 2, &pda::PROGRAM_ID).0;
        let decode =
            |ix: &Instruction| decode_stake_instruction(&ix.data, &keys(ix), Some(&vaults), false);

        let d = decode(&build_withdraw_instruction(
            stake(),
            vault2,
            authority(),
            1_500_000_000,
        ));
        assert_eq!(
            d.text,
            format!(
                "Stake Withdraw 1.5 SOL from {} to {vault2} (vault 2), authority={}",
                stake(),
                authority()
            )
        );
        assert!(d.warnings.is_empty());

        let d = decode(&build_delegate_instruction(stake(), vote(), authority()));
        assert_eq!(
            d.text,
            format!(
                "Stake DelegateStake {} to vote account {}, authority={}",
                stake(),
                vote(),
                authority()
            )
        );

        let d = decode(&build_split_instruction(
            stake(),
            authority(),
            1_500_000_000,
            split_destination(),
        ));
        assert_eq!(
            d.text,
            format!(
                "Stake Split 1.5 SOL from {} to {}, authority={}",
                stake(),
                split_destination(),
                authority()
            )
        );

        let d = decode(&build_deactivate_instruction(stake(), authority()));
        assert_eq!(
            d.text,
            format!("Stake Deactivate {}, authority={}", stake(), authority())
        );

        let d = decode(&build_merge_instruction(stake(), stake2(), authority()));
        assert_eq!(
            d.text,
            format!(
                "Stake Merge {} into {}, authority={}",
                stake2(),
                stake(),
                authority()
            )
        );

        // Initialize to the vault: no warning. To an outside key: warned.
        let d = decode(&build_initialize_instruction(new_stake(), vault2, vault2));
        assert_eq!(
            d.text,
            format!(
                "Stake Initialize {}: staker={vault2}, withdrawer={vault2}",
                new_stake()
            )
        );
        assert!(d.warnings.is_empty());
        let d = decode(&build_initialize_instruction(
            new_stake(),
            vault2,
            authority(),
        ));
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].contains("withdrawer"));

        // Initialize with a lockup is flagged.
        let mut ix = build_initialize_instruction(new_stake(), vault2, vault2);
        ix.data[76..84].copy_from_slice(&900u64.to_le_bytes());
        let d = decode(&ix);
        assert!(d.text.contains("lockup epoch=900"));
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn decodes_authorize_variants_with_withdrawer_warning() {
        let ms = Pubkey::new_from_array([7u8; 32]);
        let vaults = vaults_of(&ms);
        let vault1 = pda::vault_pda(&ms, 1, &pda::PROGRAM_ID).0;
        let decode = |data: &[u8], accounts: &[Pubkey]| {
            decode_stake_instruction(data, accounts, Some(&vaults), false)
        };

        let ix = build_authorize_instruction(
            stake(),
            authority(),
            new_authority(),
            StakeAuthorize::Withdrawer,
        );
        let d = decode(&ix.data, &keys(&ix));
        assert_eq!(
            d.text,
            format!(
                "Stake Authorize withdrawer on {}: new={}",
                stake(),
                new_authority()
            )
        );
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].contains("not a vault"));

        let ix =
            build_authorize_instruction(stake(), authority(), vault1, StakeAuthorize::Withdrawer);
        assert!(decode(&ix.data, &keys(&ix)).warnings.is_empty());

        // Staker changes are never flagged.
        let ix = build_authorize_instruction(
            stake(),
            authority(),
            new_authority(),
            StakeAuthorize::Staker,
        );
        let d = decode(&ix.data, &keys(&ix));
        assert!(d.text.starts_with("Stake Authorize staker"));
        assert!(d.warnings.is_empty());

        // AuthorizeChecked: role in data, new authority is account 3.
        let checked_accounts = [stake(), SYSVAR_CLOCK, authority(), new_authority()];
        let d = decode(&[10, 0, 0, 0, 1, 0, 0, 0], &checked_accounts);
        assert_eq!(
            d.text,
            format!(
                "Stake AuthorizeChecked withdrawer on {}: new={}",
                stake(),
                new_authority()
            )
        );
        assert_eq!(d.warnings.len(), 1);
        assert!(decode(&[10, 0, 0, 0, 0, 0, 0, 0], &checked_accounts)
            .warnings
            .is_empty());

        // AuthorizeWithSeed: { new authority, role, seed, owner }.
        let mut data = vec![8, 0, 0, 0];
        data.extend_from_slice(new_authority().as_ref());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&4u64.to_le_bytes());
        data.extend_from_slice(b"seed");
        data.extend_from_slice(Pubkey::new_from_array([9u8; 32]).as_ref());
        let d = decode(&data, &[stake(), authority(), SYSVAR_CLOCK]);
        assert_eq!(
            d.text,
            format!(
                "Stake AuthorizeWithSeed withdrawer on {}: new={}",
                stake(),
                new_authority()
            )
        );
        assert_eq!(d.warnings.len(), 1);

        // AuthorizeCheckedWithSeed: { role, seed, owner }; new authority is account 3.
        let mut data = vec![11, 0, 0, 0, 1, 0, 0, 0];
        data.extend_from_slice(&4u64.to_le_bytes());
        data.extend_from_slice(b"seed");
        data.extend_from_slice(Pubkey::new_from_array([9u8; 32]).as_ref());
        let d = decode(&data, &[stake(), authority(), SYSVAR_CLOCK, vault1]);
        assert_eq!(
            d.text,
            format!(
                "Stake AuthorizeCheckedWithSeed withdrawer on {}: new={vault1}",
                stake()
            )
        );
        assert!(d.warnings.is_empty());

        // Without multisig context (the create preview) nothing is flagged.
        let ix = build_authorize_instruction(
            stake(),
            authority(),
            new_authority(),
            StakeAuthorize::Withdrawer,
        );
        assert!(decode_stake_instruction(&ix.data, &keys(&ix), None, false)
            .warnings
            .is_empty());
    }

    #[test]
    fn decodes_lockup_move_names_and_unrecognised() {
        let accounts = [stake(), stake2(), authority()];
        let d = decode_stake_instruction(&[6, 0, 0, 0, 0, 0, 0], &accounts, None, false);
        assert_eq!(d.text, format!("Stake SetLockup on {}", stake()));
        assert_eq!(d.warnings.len(), 1);
        let d = decode_stake_instruction(&[12, 0, 0, 0, 0, 0], &accounts, None, false);
        assert!(d.text.starts_with("Stake SetLockupChecked"));
        assert_eq!(d.warnings.len(), 1);

        let mut data = vec![16, 0, 0, 0];
        data.extend_from_slice(&2_000_000_000u64.to_le_bytes());
        let d = decode_stake_instruction(&data, &accounts, None, false);
        assert_eq!(
            d.text,
            format!("Stake MoveStake 2 SOL from {} to {}", stake(), stake2())
        );
        data[0] = 17;
        let d = decode_stake_instruction(&data, &accounts, None, false);
        assert!(d.text.starts_with("Stake MoveLamports 2 SOL"));
        assert!(d.warnings.is_empty());

        for (tag, name) in [
            (13u8, "GetMinimumDelegation"),
            (14, "DeactivateDelinquent"),
            (15, "Redelegate"),
        ] {
            let d = decode_stake_instruction(&[tag, 0, 0, 0], &accounts, None, false);
            assert_eq!(d.text, format!("Stake {name}"));
            assert!(d.warnings.is_empty());
        }

        for data in [
            vec![],
            vec![4, 0, 0, 0, 1],
            vec![99, 0, 0, 0],
            vec![1, 0, 0, 0],
            // Authorize with an unknown role.
            {
                let mut d = vec![1, 0, 0, 0];
                d.extend_from_slice(&[0u8; 32]);
                d.extend_from_slice(&7u32.to_le_bytes());
                d
            },
        ] {
            let d = decode_stake_instruction(&data, &accounts, None, false);
            assert_eq!(d.text, UNRECOGNISED, "{data:?}");
            assert_eq!(d.warnings.len(), 1);
        }
        // AuthorizeChecked without its new-authority account.
        let d = decode_stake_instruction(&[10, 0, 0, 0, 1, 0, 0, 0], &accounts, None, false);
        assert_eq!(d.text, UNRECOGNISED);
    }

    #[test]
    fn show_info_labels_vaults_and_lockup() {
        let vault = Pubkey::new_from_array([7u8; 32]);
        let mut meta = meta_with(vault, Pubkey::new_from_array([8u8; 32]));
        meta.lockup.epoch = 600;
        let s = snapshot(
            StakeAccountState::Stake {
                meta,
                delegation: Delegation {
                    deactivation_epoch: 550,
                    ..sample_delegation()
                },
                flags: 0,
            },
            42,
        );
        let vaults: HashMap<Pubkey, u8> = [(vault, 3u8)].into_iter().collect();
        let info = stake_account_info(&s, &clock(500, 0), Some(&vaults));
        assert_eq!(info.state, "delegated");
        assert_eq!(info.staker_vault_index, Some(3));
        assert_eq!(info.withdrawer_vault_index, None);
        assert_eq!(info.rent_exempt_reserve_lamports, Some(2_282_880));
        assert!(info.lockup.as_ref().is_some_and(|l| l.in_force));
        let delegation = info
            .delegation
            .as_ref()
            .unwrap_or_else(|| panic!("delegation"));
        assert_eq!(delegation.deactivation_epoch, Some(550));
        assert_eq!(info.current_epoch, 500);

        let info = stake_account_info(
            &snapshot(StakeAccountState::Uninitialized, 1),
            &clock(1, 0),
            None,
        );
        assert_eq!(info.state, "uninitialized");
        assert!(info.staker.is_none() && info.lockup.is_none() && info.delegation.is_none());

        let json = serde_json::to_value(stake_account_info(&s, &clock(500, 0), None))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["staker"], vault.to_string());
        assert_eq!(json["delegation"]["deactivation_epoch"], 550);
    }
}
