//! Vote account withdrawer operations run from a Squads vault.
//!
//! Each operation becomes one vault-transaction proposal whose inner signer is
//! the vault that is the vote account's authorized withdrawer. The one
//! exception is `UpdateValidatorIdentity`, where the new identity must also
//! sign; it is supplied at execution (`proposal execute --extra-signer`).
//! Vote program instructions are encoded by hand (bincode: u32 LE enum tag
//! followed by LE fields) and checked against bytes dumped from the solana CLI.

use std::collections::HashMap;

use solana_pubkey::Pubkey;

use crate::domain::vote_account::VoteAccountInfo;
use crate::error::MsigError;
use crate::infra::accounts::vote::{VoteAccountState, VoteCommission, VoteStateVersion};
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::{AccountData, RpcProvider};
use crate::infra::signer::Signer;
use crate::output::{format_addr, format_sol};
use crate::sanitize::sanitize_display;

use super::pipeline::compile_unsigned_legacy_transaction_base64;
use super::vault_tx::{create_vault_transaction_proposal, VaultProposalResult};

pub const VOTE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("Vote111111111111111111111111111111111111111");
const SYSVAR_CLOCK: Pubkey = solana_pubkey::pubkey!("SysvarC1ock11111111111111111111111111111111");
const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");

const IX_INITIALIZE_ACCOUNT: u32 = 0;
const IX_AUTHORIZE: u32 = 1;
const IX_WITHDRAW: u32 = 3;
const IX_UPDATE_VALIDATOR_IDENTITY: u32 = 4;
const IX_UPDATE_COMMISSION: u32 = 5;
const IX_AUTHORIZE_CHECKED: u32 = 7;
const IX_AUTHORIZE_WITH_SEED: u32 = 10;
const IX_AUTHORIZE_CHECKED_WITH_SEED: u32 = 11;
const IX_UPDATE_COMMISSION_COLLECTOR: u32 = 17;
/// `VoteAuthorize::VoterWithBLS` (SIMD-0387); its arguments follow the tag inline.
const ROLE_VOTER_WITH_BLS: u32 = 2;

/// `VoteAuthorize` role (bincode u32 tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteAuthorize {
    Voter,
    Withdrawer,
}

impl VoteAuthorize {
    fn tag(self) -> u32 {
        match self {
            Self::Voter => 0,
            Self::Withdrawer => 1,
        }
    }
}

/// SIMD-0232 `CommissionKind` (bincode u32 tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorKind {
    InflationRewards,
    BlockRevenue,
}

impl CollectorKind {
    fn tag(self) -> u32 {
        match self {
            Self::InflationRewards => 0,
            Self::BlockRevenue => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::InflationRewards => "inflation-rewards",
            Self::BlockRevenue => "block-revenue",
        }
    }

    pub fn parse(s: &str) -> Result<Self, MsigError> {
        match s {
            "inflation-rewards" => Ok(Self::InflationRewards),
            "block-revenue" => Ok(Self::BlockRevenue),
            other => Err(MsigError::Usage(format!(
                "invalid collector kind '{other}'. Expected inflation-rewards or block-revenue."
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Instruction builders
// ---------------------------------------------------------------------------

/// `VoteInstruction::Withdraw(u64)`: [vote w, recipient w, withdrawer signer].
pub fn build_withdraw_instruction(
    vote: Pubkey,
    recipient: Pubkey,
    withdrawer: Pubkey,
    lamports: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&IX_WITHDRAW.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: VOTE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(vote, false),
            AccountMeta::new(recipient, false),
            AccountMeta::new_readonly(withdrawer, true),
        ],
        data,
    }
}

/// `VoteInstruction::UpdateValidatorIdentity`:
/// [vote w, new identity read-only signer, withdrawer read-only signer].
///
/// The new identity is an inner signer that is not the vault, so the proposal
/// can only be executed with that key supplied as an extra signer.
pub fn build_update_validator_identity_instruction(
    vote: Pubkey,
    new_identity: Pubkey,
    withdrawer: Pubkey,
) -> Instruction {
    Instruction {
        program_id: VOTE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(vote, false),
            AccountMeta::new_readonly(new_identity, true),
            AccountMeta::new_readonly(withdrawer, true),
        ],
        data: IX_UPDATE_VALIDATOR_IDENTITY.to_le_bytes().to_vec(),
    }
}

/// `VoteInstruction::UpdateCommission(u8)`: [vote w, withdrawer signer].
pub fn build_update_commission_instruction(
    vote: Pubkey,
    withdrawer: Pubkey,
    percent: u8,
) -> Instruction {
    let mut data = Vec::with_capacity(5);
    data.extend_from_slice(&IX_UPDATE_COMMISSION.to_le_bytes());
    data.push(percent);
    Instruction {
        program_id: VOTE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(vote, false),
            AccountMeta::new_readonly(withdrawer, true),
        ],
        data,
    }
}

/// `VoteInstruction::Authorize(Pubkey, VoteAuthorize)`:
/// [vote w, clock sysvar, current authority signer].
pub fn build_authorize_instruction(
    vote: Pubkey,
    current_authority: Pubkey,
    new_authority: Pubkey,
    role: VoteAuthorize,
) -> Instruction {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&IX_AUTHORIZE.to_le_bytes());
    data.extend_from_slice(new_authority.as_ref());
    data.extend_from_slice(&role.tag().to_le_bytes());
    Instruction {
        program_id: VOTE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(vote, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK, false),
            AccountMeta::new_readonly(current_authority, true),
        ],
        data,
    }
}

/// `VoteInstruction::UpdateCommissionCollector(CommissionKind)` (SIMD-0232):
/// [vote w, collector w, withdrawer signer].
///
/// Not available in solana CLI 4.3, so there is no CLI dump to compare with.
/// The layout follows solana-vote-interface 6 (instruction tag 17, then the
/// kind as a u32 tag) and was validated on chain.
pub fn build_update_commission_collector_instruction(
    vote: Pubkey,
    collector: Pubkey,
    withdrawer: Pubkey,
    kind: CollectorKind,
) -> Instruction {
    let mut data = Vec::with_capacity(8);
    data.extend_from_slice(&IX_UPDATE_COMMISSION_COLLECTOR.to_le_bytes());
    data.extend_from_slice(&kind.tag().to_le_bytes());
    Instruction {
        program_id: VOTE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(vote, false),
            AccountMeta::new(collector, false),
            AccountMeta::new_readonly(withdrawer, true),
        ],
        data,
    }
}

// ---------------------------------------------------------------------------
// Preflight checks
// ---------------------------------------------------------------------------

/// A fetched and parsed vote account.
#[derive(Debug, Clone)]
pub struct VoteAccountSnapshot {
    pub address: Pubkey,
    pub lamports: u64,
    pub data_len: usize,
    pub state: VoteAccountState,
}

/// Check the owner and parse an already-fetched vote account.
pub fn parse_vote_account(
    vote: &Pubkey,
    account: &AccountData,
) -> Result<VoteAccountSnapshot, MsigError> {
    if account.owner != VOTE_PROGRAM_ID.to_string() {
        return Err(MsigError::Transaction(format!(
            "{vote} is not a vote account: it is owned by {}, not the Vote program {VOTE_PROGRAM_ID}",
            sanitize_display(&account.owner)
        )));
    }
    Ok(VoteAccountSnapshot {
        address: *vote,
        lamports: account.lamports,
        data_len: account.data.len(),
        state: VoteAccountState::parse(&account.data)?,
    })
}

/// Fetch, owner-check and parse a vote account. RPC errors are returned as is.
pub fn fetch_vote_account(
    rpc: &dyn RpcProvider,
    vote: &Pubkey,
) -> Result<VoteAccountSnapshot, MsigError> {
    let account = rpc.get_account_info(&vote.to_string())?.ok_or_else(|| {
        MsigError::Transaction(format!(
            "vote account {vote} not found on this cluster. Check the address and --cluster."
        ))
    })?;
    parse_vote_account(vote, &account)
}

/// Refuse unless the vault at `vault_index` is the authorized withdrawer.
pub fn check_vault_is_withdrawer(
    snapshot: &VoteAccountSnapshot,
    vault: &Pubkey,
    vault_index: u8,
) -> Result<(), MsigError> {
    let actual = snapshot.state.authorized_withdrawer;
    if actual != *vault {
        return Err(MsigError::Transaction(format!(
            "the authorized withdrawer of vote account {} is {actual}, not vault {vault} \
             (vault index {vault_index}). Pass the --vault-index of the vault that is the \
             withdrawer, or make the vault the withdrawer first.",
            snapshot.address
        )));
    }
    Ok(())
}

/// What `vote-account withdraw` was asked to take out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawAmount {
    Lamports(u64),
    /// Everything above the rent-exempt minimum.
    All,
    /// The full balance, closing the account.
    Close,
}

/// Resolve a withdraw request to lamports, refusing amounts the vote program
/// would reject (overdraw, or a remainder that is neither zero nor rent exempt).
pub fn resolve_withdraw_amount(
    amount: WithdrawAmount,
    balance: u64,
    rent_exempt_minimum: u64,
) -> Result<u64, MsigError> {
    let lamports = match amount {
        // An --amount equal to the balance would close the account, which approvers
        // cannot tell from the decoded instruction; make that intent explicit.
        WithdrawAmount::Lamports(lamports) if lamports == balance => {
            return Err(MsigError::Usage(format!(
                "{} SOL is the whole balance and would close the vote account; \
                 use --close to close it, or --all to keep it rent exempt",
                format_sol(lamports)
            )));
        }
        WithdrawAmount::Lamports(lamports) => lamports,
        WithdrawAmount::All => balance.saturating_sub(rent_exempt_minimum),
        WithdrawAmount::Close => balance,
    };
    if lamports == 0 {
        return Err(MsigError::Usage(format!(
            "nothing to withdraw: the amount is zero (balance {} SOL, rent-exempt minimum {} SOL)",
            format_sol(balance),
            format_sol(rent_exempt_minimum)
        )));
    }
    if lamports > balance {
        return Err(MsigError::Usage(format!(
            "cannot withdraw {} SOL: the vote account balance is {} SOL",
            format_sol(lamports),
            format_sol(balance)
        )));
    }
    let remaining = balance - lamports;
    if remaining > 0 && remaining < rent_exempt_minimum {
        return Err(MsigError::Usage(format!(
            "withdrawing {} SOL would leave {} SOL, below the rent-exempt minimum of {} SOL. \
             Use --all to keep the account rent exempt, or --close to withdraw everything.",
            format_sol(lamports),
            format_sol(remaining),
            format_sol(rent_exempt_minimum)
        )));
    }
    Ok(lamports)
}

/// All 256 vault PDAs of a multisig, keyed by address.
pub fn vault_addresses(multisig: &Pubkey, program_id: &Pubkey) -> HashMap<Pubkey, u8> {
    (0..=u8::MAX)
        .map(|index| (pda::vault_pda(multisig, index, program_id).0, index))
        .collect()
}

/// A new withdrawer outside the multisig hands the vote account to someone else.
pub fn is_external_withdrawer(new_withdrawer: &Pubkey, vaults: &HashMap<Pubkey, u8>) -> bool {
    !vaults.contains_key(new_withdrawer)
}

/// A collector that is neither the vote account nor a vault diverts rewards.
pub fn is_external_collector(
    collector: &Pubkey,
    vote: &Pubkey,
    vaults: &HashMap<Pubkey, u8>,
) -> bool {
    collector != vote && !vaults.contains_key(collector)
}

/// Refuse a new validator identity that cannot or need not be set: the
/// current identity (a no-op), the vote account itself, or one of the
/// multisig's vaults (a vault is a PDA and cannot run a validator).
pub fn check_new_identity(
    snapshot: &VoteAccountSnapshot,
    new_identity: &Pubkey,
    vaults: &HashMap<Pubkey, u8>,
) -> Result<(), MsigError> {
    if *new_identity == snapshot.state.node_pubkey {
        return Err(MsigError::Usage(format!(
            "{new_identity} is already the validator identity of vote account {}",
            snapshot.address
        )));
    }
    if *new_identity == snapshot.address {
        return Err(MsigError::Usage(
            "the new validator identity cannot be the vote account itself".into(),
        ));
    }
    if let Some(index) = vaults.get(new_identity) {
        return Err(MsigError::Usage(format!(
            "the new validator identity {new_identity} is vault {index} of this multisig. A vault \
             is a PDA and cannot run a validator; pass the identity keypair's public key."
        )));
    }
    Ok(())
}

/// SIMD-0232 collector rules: the vote account itself, or an existing
/// system-owned account holding at least its rent-exempt minimum.
pub fn validate_collector(
    collector: &Pubkey,
    vote: &Pubkey,
    account: Option<&AccountData>,
    rent_exempt_minimum: u64,
) -> Result<(), MsigError> {
    if collector == vote {
        return Ok(());
    }
    let account = account.ok_or_else(|| {
        MsigError::Transaction(format!(
            "collector {collector} does not exist. Fund it with at least the rent-exempt minimum \
             first, or use the vote account itself."
        ))
    })?;
    if account.owner != SYSTEM_PROGRAM.to_string() {
        return Err(MsigError::Transaction(format!(
            "collector {collector} is owned by {}; a collector must be the vote account or a \
             system-owned account",
            sanitize_display(&account.owner)
        )));
    }
    if account.lamports < rent_exempt_minimum {
        return Err(MsigError::Transaction(format!(
            "collector {collector} holds {} SOL, below its rent-exempt minimum of {} SOL",
            format_sol(account.lamports),
            format_sol(rent_exempt_minimum)
        )));
    }
    Ok(())
}

fn check_collector_on_chain(
    rpc: &dyn RpcProvider,
    collector: &Pubkey,
    vote: &Pubkey,
) -> Result<(), MsigError> {
    if collector == vote {
        return Ok(());
    }
    let account = rpc.get_account_info(&collector.to_string())?;
    let rent_exempt_minimum = match &account {
        Some(account) => rpc.get_minimum_balance_for_rent_exemption(account.data.len())?,
        None => 0,
    };
    validate_collector(collector, vote, account.as_ref(), rent_exempt_minimum)
}

/// Simulate the inner instructions on their own, fee payer = proposer, before
/// anything is proposed. The Squads pipeline only simulates the outer
/// proposal-creation transaction, which succeeds whatever the inner ops do.
///
/// `allow_failure` exists because some operations legitimately fail now but
/// succeed at execution time (a commission change outside the allowed part of
/// the epoch, an authority change that is already queued).
pub fn simulate_inner_instructions(
    rpc: &dyn RpcProvider,
    payer: &Pubkey,
    instructions: &[Instruction],
    allow_failure: bool,
) -> Result<(), MsigError> {
    let (blockhash, _) = rpc.get_latest_blockhash()?;
    let tx = compile_unsigned_legacy_transaction_base64(instructions, payer, &blockhash)?;
    let result = rpc.simulate_transaction(&tx)?;
    let Some(err) = result.err else {
        return Ok(());
    };

    let err = sanitize_display(&err.to_string());
    eprintln!("Inner instruction simulation failed: {err}");
    if !result.logs.is_empty() {
        eprintln!("Simulation logs:");
        for log in &result.logs {
            eprintln!("  {}", sanitize_display(log));
        }
    }
    if allow_failure {
        eprintln!(
            "Warning: proposing anyway because --allow-simulation-failure was passed. \
             The proposal may fail at execution."
        );
        return Ok(());
    }
    Err(MsigError::SimulationFailed(format!(
        "inner vote instructions failed: {err}. Nothing was proposed. Pass \
         --allow-simulation-failure only if the operation is expected to succeed at execution time."
    )))
}

// ---------------------------------------------------------------------------
// Proposal creation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteOperation {
    Withdraw {
        recipient: Pubkey,
        amount: WithdrawAmount,
    },
    SetCommission {
        percent: u8,
    },
    SetVoter {
        new_voter: Pubkey,
    },
    SetWithdrawer {
        new_withdrawer: Pubkey,
    },
    SetCollector {
        kind: CollectorKind,
        collector: Pubkey,
    },
    SetIdentity {
        new_identity: Pubkey,
    },
}

pub struct VoteProposalRequest<'a> {
    pub multisig: Pubkey,
    pub vault_index: u8,
    pub vote_account: Pubkey,
    pub operation: VoteOperation,
    pub allow_external_authority: bool,
    pub allow_simulation_failure: bool,
    pub memo: Option<&'a str>,
    pub dry_run: bool,
    pub skip_confirm: bool,
    pub quiet: bool,
}

pub(crate) const BLS_VOTER_REFUSAL: &str =
    "this vote account has a BLS key registered, and the vote program (SIMD-0387) \
rejects a plain Authorize(Voter) on it. The current voter can rotate itself with \
`solana vote-authorize-voter-checked <VOTE> <CURRENT_VOTER_KEYPAIR> <NEW_VOTER_KEYPAIR>`. \
Changing the voter from the multisig needs the new voter's BLS key and proof of possession, \
which msig does not support yet.";

/// Run every preflight check and build the single inner instruction plus the
/// proposal description. Returns the external-authority reason, if any.
fn prepare_operation(
    rpc: &dyn RpcProvider,
    config: &Config,
    req: &VoteProposalRequest<'_>,
    vault: &Pubkey,
) -> Result<(Instruction, String, Option<String>), MsigError> {
    let vote = req.vote_account;
    let snapshot = fetch_vote_account(rpc, &vote)?;
    check_vault_is_withdrawer(&snapshot, vault, req.vault_index)?;

    let prepared = match req.operation {
        VoteOperation::Withdraw { recipient, amount } => {
            let rent_exempt_minimum =
                rpc.get_minimum_balance_for_rent_exemption(snapshot.data_len)?;
            let lamports = resolve_withdraw_amount(amount, snapshot.lamports, rent_exempt_minimum)?;
            let closing = if lamports == snapshot.lamports {
                " (closes the account)"
            } else {
                ""
            };
            (
                build_withdraw_instruction(vote, recipient, *vault, lamports),
                format!(
                    "Withdraw {} SOL from vote account {vote} to {recipient}{closing}",
                    format_sol(lamports)
                ),
                None,
            )
        }
        VoteOperation::SetCommission { percent } => {
            if percent > 100 {
                return Err(MsigError::Usage(format!(
                    "commission must be between 0 and 100, got {percent}"
                )));
            }
            (
                build_update_commission_instruction(vote, *vault, percent),
                if snapshot.state.version == VoteStateVersion::V4 {
                    // On V4 accounts UpdateCommission sets only the inflation-rewards
                    // commission; the block-revenue commission is left as is.
                    format!(
                        "Set inflation-rewards commission of vote account {vote} to {percent}% \
                         (block-revenue commission unchanged)"
                    )
                } else {
                    format!("Set commission of vote account {vote} to {percent}%")
                },
                None,
            )
        }
        VoteOperation::SetVoter { new_voter } => {
            if snapshot.state.version == VoteStateVersion::V4 && snapshot.state.has_bls_key {
                return Err(MsigError::Transaction(BLS_VOTER_REFUSAL.into()));
            }
            (
                build_authorize_instruction(vote, *vault, new_voter, VoteAuthorize::Voter),
                format!("Set authorized voter of vote account {vote} to {new_voter}"),
                None,
            )
        }
        VoteOperation::SetWithdrawer { new_withdrawer } => {
            let vaults = vault_addresses(&req.multisig, &config.program_id);
            let external = is_external_withdrawer(&new_withdrawer, &vaults).then(|| {
                format!(
                    "new withdrawer {new_withdrawer} is not a vault of multisig {}",
                    req.multisig
                )
            });
            (
                build_authorize_instruction(
                    vote,
                    *vault,
                    new_withdrawer,
                    VoteAuthorize::Withdrawer,
                ),
                format!("Set authorized withdrawer of vote account {vote} to {new_withdrawer}"),
                external,
            )
        }
        VoteOperation::SetCollector { kind, collector } => {
            let vaults = vault_addresses(&req.multisig, &config.program_id);
            let external = is_external_collector(&collector, &vote, &vaults).then(|| {
                format!(
                    "{} collector {collector} is neither the vote account nor a vault of multisig {}",
                    kind.label(),
                    req.multisig
                )
            });
            check_collector_on_chain(rpc, &collector, &vote)?;
            (
                build_update_commission_collector_instruction(vote, collector, *vault, kind),
                format!(
                    "Set {} collector of vote account {vote} to {collector}",
                    kind.label()
                ),
                external,
            )
        }
        VoteOperation::SetIdentity { new_identity } => {
            let vaults = vault_addresses(&req.multisig, &config.program_id);
            check_new_identity(&snapshot, &new_identity, &vaults)?;
            (
                build_update_validator_identity_instruction(vote, new_identity, *vault),
                format!(
                    "Set validator identity of vote account {vote} to {new_identity} \
                     (the new identity must co-sign the execution)"
                ),
                None,
            )
        }
    };
    Ok(prepared)
}

/// Create one vault-transaction proposal for a vote account operation.
pub fn create_vote_account_proposal(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    config: &Config,
    req: &VoteProposalRequest<'_>,
) -> Result<VaultProposalResult, MsigError> {
    let (vault, _) = pda::vault_pda(&req.multisig, req.vault_index, &config.program_id);
    let (instruction, mut description, external) = prepare_operation(rpc, config, req, &vault)?;

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

    let instructions = vec![instruction];
    simulate_inner_instructions(
        rpc,
        &signer.pubkey(),
        &instructions,
        req.allow_simulation_failure,
    )?;

    let result = create_vault_transaction_proposal(
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
    )?;

    if let VoteOperation::SetIdentity { new_identity } = req.operation {
        if result.signature.is_some() && !req.quiet {
            println!(
                "{}",
                set_identity_next_step(result.transaction_index, &new_identity)
            );
        }
    }
    Ok(result)
}

/// What to run once a set-identity proposal is approved: the new identity has
/// to sign the execute transaction.
pub fn set_identity_next_step(index: u64, new_identity: &Pubkey) -> String {
    format!(
        "After approval, execute with the new identity as extra signer ({new_identity}):\n  \
         msig proposal execute {index} --extra-signer <NEW_IDENTITY_KEYPAIR>\n\
         The identity keypair file must be on the executing machine; offline execution \
         and Ledger are not supported for this proposal."
    )
}

/// Read-only summary of a vote account. `vaults` labels the withdrawer when it
/// is one of the active multisig's vaults.
pub fn fetch_vote_account_info(
    rpc: &dyn RpcProvider,
    vote: &Pubkey,
    vaults: Option<&HashMap<Pubkey, u8>>,
) -> Result<VoteAccountInfo, MsigError> {
    let snapshot = fetch_vote_account(rpc, vote)?;
    let rent_exempt_minimum = rpc.get_minimum_balance_for_rent_exemption(snapshot.data_len)?;
    Ok(vote_account_info(&snapshot, rent_exempt_minimum, vaults))
}

fn vote_account_info(
    snapshot: &VoteAccountSnapshot,
    rent_exempt_minimum: u64,
    vaults: Option<&HashMap<Pubkey, u8>>,
) -> VoteAccountInfo {
    let state = &snapshot.state;
    let (commission_percent, inflation_bps, block_bps) = match state.commission {
        VoteCommission::Percent(p) => (Some(p), None, None),
        VoteCommission::Bps {
            inflation_rewards,
            block_revenue,
        } => (None, Some(inflation_rewards), Some(block_revenue)),
    };
    VoteAccountInfo {
        vote_account: snapshot.address,
        balance_lamports: snapshot.lamports,
        rent_exempt_minimum_lamports: rent_exempt_minimum,
        version: state.version.label().to_string(),
        validator_identity: state.node_pubkey,
        authorized_withdrawer: state.authorized_withdrawer,
        authorized_withdrawer_vault_index: vaults
            .and_then(|v| v.get(&state.authorized_withdrawer).copied()),
        commission_percent,
        inflation_rewards_commission_bps: inflation_bps,
        block_revenue_commission_bps: block_bps,
        inflation_rewards_collector: state.collectors.map(|c| c.inflation_rewards),
        block_revenue_collector: state.collectors.map(|c| c.block_revenue),
        bls_key_registered: state.has_bls_key,
    }
}

// ---------------------------------------------------------------------------
// Review decoding (proposal show and the create preview)
// ---------------------------------------------------------------------------

/// Human-readable decoding of one Vote program instruction plus review warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedVoteInstruction {
    pub text: String,
    pub warnings: Vec<String>,
}

const UNRECOGNISED: &str = "Vote: unrecognised instruction";

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn read_pubkey(data: &[u8], off: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(off..off + 32)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

fn role_label(tag: u32) -> Option<&'static str> {
    match tag {
        0 => Some("voter"),
        1 => Some("withdrawer"),
        2 => Some("voter-with-BLS"),
        _ => None,
    }
}

fn other_instruction_name(tag: u32) -> Option<&'static str> {
    match tag {
        2 => Some("Vote"),
        6 => Some("VoteSwitch"),
        8 => Some("UpdateVoteState"),
        9 => Some("UpdateVoteStateSwitch"),
        12 => Some("CompactUpdateVoteState"),
        13 => Some("CompactUpdateVoteStateSwitch"),
        14 => Some("TowerSync"),
        15 => Some("TowerSyncSwitch"),
        _ => None,
    }
}

/// Decode a Vote program instruction. `vaults` is the proposal multisig's
/// vault set; when given, authority and collector targets outside it are
/// flagged. `accounts` are the instruction's account keys in order.
pub fn decode_vote_instruction(
    data: &[u8],
    accounts: &[Pubkey],
    vaults: Option<&HashMap<Pubkey, u8>>,
    truncate: bool,
) -> DecodedVoteInstruction {
    let addr = |pk: &Pubkey| format_addr(&pk.to_string(), truncate);
    let acct = |idx: usize| accounts.get(idx).map(addr).unwrap_or_else(|| "?".into());
    let mut warnings = Vec::new();

    let text = (|| {
        let tag = read_u32(data, 0)?;
        match tag {
            IX_WITHDRAW => {
                let lamports = read_u64(data, 4)?;
                let recipient_vault = accounts
                    .get(1)
                    .and_then(|recipient| vaults.and_then(|vaults| vaults.get(recipient)))
                    .map(|index| format!(" (vault {index})"))
                    .unwrap_or_default();
                Some(format!(
                    "Vote Withdraw {} SOL from {} to {}{recipient_vault}, authority={}",
                    format_sol(lamports),
                    acct(0),
                    acct(1),
                    acct(2)
                ))
            }
            IX_UPDATE_COMMISSION => {
                let percent = *data.get(4)?;
                Some(format!("Vote UpdateCommission {percent}% on {}", acct(0)))
            }
            IX_AUTHORIZE => {
                let new_authority = read_pubkey(data, 4)?;
                let role_tag = read_u32(data, 36)?;
                let role = role_label(role_tag)?;
                if role_tag == VoteAuthorize::Withdrawer.tag() {
                    warn_external_withdrawer(&new_authority, vaults, &mut warnings, truncate);
                }
                Some(format!(
                    "Vote Authorize {role} on {}: new={}",
                    acct(0),
                    addr(&new_authority)
                ))
            }
            IX_AUTHORIZE_CHECKED => {
                let role_tag = read_u32(data, 4)?;
                let role = role_label(role_tag)?;
                let new_authority = accounts.get(3)?;
                if role_tag == VoteAuthorize::Withdrawer.tag() {
                    warn_external_withdrawer(new_authority, vaults, &mut warnings, truncate);
                }
                Some(format!(
                    "Vote AuthorizeChecked {role} on {}: new={}",
                    acct(0),
                    addr(new_authority)
                ))
            }
            IX_UPDATE_VALIDATOR_IDENTITY => Some(format!(
                "Vote UpdateValidatorIdentity on {}: new={}",
                acct(0),
                acct(1)
            )),
            IX_UPDATE_COMMISSION_COLLECTOR => {
                let kind = match read_u32(data, 4)? {
                    0 => CollectorKind::InflationRewards,
                    1 => CollectorKind::BlockRevenue,
                    _ => return None,
                };
                let vote = accounts.first()?;
                let collector = accounts.get(1)?;
                if let Some(vaults) = vaults {
                    if is_external_collector(collector, vote, vaults) {
                        warnings.push(format!(
                            "{} collector {} is neither the vote account nor a vault of this multisig",
                            kind.label(),
                            addr(collector)
                        ));
                    }
                }
                Some(format!(
                    "Vote UpdateCommissionCollector {} on {}: collector={}",
                    kind.label(),
                    addr(vote),
                    addr(collector)
                ))
            }
            IX_INITIALIZE_ACCOUNT => {
                // VoteInit { node_pubkey, authorized_voter, authorized_withdrawer, commission }
                let withdrawer = read_pubkey(data, 68)?;
                warn_external_withdrawer(&withdrawer, vaults, &mut warnings, truncate);
                Some(format!(
                    "Vote InitializeAccount {}: identity={}, voter={}, withdrawer={}",
                    acct(0),
                    addr(&read_pubkey(data, 4)?),
                    addr(&read_pubkey(data, 36)?),
                    addr(&withdrawer)
                ))
            }
            IX_AUTHORIZE_WITH_SEED | IX_AUTHORIZE_CHECKED_WITH_SEED => {
                // { authorization_type, derived key owner, seed: String (u64 len + bytes),
                //   new_authority (unchecked variant only; account 3 for the checked one) }
                let role_tag = read_u32(data, 4)?;
                // VoterWithBLS carries extra bytes before the owner; not decoded here.
                if role_tag == ROLE_VOTER_WITH_BLS {
                    return None;
                }
                let role = role_label(role_tag)?;
                let seed_len = usize::try_from(read_u64(data, 40)?).ok()?;
                let after_seed = 48usize.checked_add(seed_len)?;
                let (name, new_authority) = if tag == IX_AUTHORIZE_WITH_SEED {
                    ("AuthorizeWithSeed", read_pubkey(data, after_seed)?)
                } else {
                    ("AuthorizeCheckedWithSeed", *accounts.get(3)?)
                };
                if role_tag == VoteAuthorize::Withdrawer.tag() {
                    warn_external_withdrawer(&new_authority, vaults, &mut warnings, truncate);
                }
                Some(format!(
                    "Vote {name} {role} on {}: new={}",
                    acct(0),
                    addr(&new_authority)
                ))
            }
            other => other_instruction_name(other).map(|name| format!("Vote {name}")),
        }
    })();

    match text {
        Some(text) => DecodedVoteInstruction { text, warnings },
        None => DecodedVoteInstruction {
            text: UNRECOGNISED.to_string(),
            warnings: vec![
                "unrecognised Vote program instruction; review the raw data with --verbose"
                    .to_string(),
            ],
        },
    }
}

fn warn_external_withdrawer(
    new_authority: &Pubkey,
    vaults: Option<&HashMap<Pubkey, u8>>,
    warnings: &mut Vec<String>,
    truncate: bool,
) {
    if let Some(vaults) = vaults {
        if is_external_withdrawer(new_authority, vaults) {
            warnings.push(format!(
                "new withdrawer {} is not a vault of this multisig",
                format_addr(&new_authority.to_string(), truncate)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::accounts::vote::tests::{hex, synthetic, V4_ACCOUNT_PREFIX_HEX};

    fn pk(s: &str) -> Pubkey {
        s.parse().unwrap_or_else(|e| panic!("{e:?}"))
    }

    // Pubkeys used for the solana CLI 4.3 `--dump-transaction-message` fixtures.
    fn vote() -> Pubkey {
        pk("3mZFQNXybi8rdSShmGETLerdheV8vHZTKcYuXBnrjseH")
    }
    fn withdrawer() -> Pubkey {
        pk("B6bgbkE67GcoPjXoeV6rt8Fprj3BjdJrzhNP6qWVU32G")
    }
    fn recipient() -> Pubkey {
        pk("4rWPt5UwRg4Dc5nxKf9Ld2s2LSSjTdBrpLEHAhuuvoEB")
    }
    fn new_authority() -> Pubkey {
        pk("3vYr3QnjjxouirYjAcdZLwVKWafhQCXHmqfYorQYC6nn")
    }

    fn metas(ix: &Instruction) -> Vec<(Pubkey, bool, bool)> {
        ix.accounts
            .iter()
            .map(|m| (m.pubkey, m.is_writable, m.is_signer))
            .collect()
    }

    fn account(owner: Pubkey, lamports: u64, data: Vec<u8>) -> AccountData {
        AccountData {
            lamports,
            data,
            owner: owner.to_string(),
        }
    }

    fn keys(ix: &Instruction) -> Vec<Pubkey> {
        ix.accounts.iter().map(|m| m.pubkey).collect()
    }

    #[test]
    fn withdraw_matches_cli_dump() {
        let ix = build_withdraw_instruction(vote(), recipient(), withdrawer(), 1_500_000_000);
        assert_eq!(ix.program_id, VOTE_PROGRAM_ID);
        assert_eq!(ix.data, hex("03000000002f685900000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (vote(), true, false),
                (recipient(), true, false),
                (withdrawer(), false, true),
            ]
        );
    }

    #[test]
    fn update_validator_identity_matches_cli_dump() {
        let ix = build_update_validator_identity_instruction(vote(), new_authority(), withdrawer());
        assert_eq!(ix.program_id, VOTE_PROGRAM_ID);
        assert_eq!(ix.data, hex("04000000"));
        assert_eq!(
            metas(&ix),
            vec![
                (vote(), true, false),
                (new_authority(), false, true),
                (withdrawer(), false, true),
            ]
        );
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), None, false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote UpdateValidatorIdentity on {}: new={}",
                vote(),
                new_authority()
            )
        );
    }

    #[test]
    fn update_commission_matches_cli_dump() {
        let ix = build_update_commission_instruction(vote(), withdrawer(), 7);
        assert_eq!(ix.program_id, VOTE_PROGRAM_ID);
        assert_eq!(ix.data, hex("0500000007"));
        assert_eq!(
            metas(&ix),
            vec![(vote(), true, false), (withdrawer(), false, true)]
        );
    }

    #[test]
    fn authorize_matches_cli_dump() {
        let key_hex = "2b6f2a238faf9a5f1a6624fdb72a08ed698a85d8167f475510e80b4b1859025b";
        for (role, role_hex) in [
            (VoteAuthorize::Withdrawer, "01000000"),
            (VoteAuthorize::Voter, "00000000"),
        ] {
            let ix = build_authorize_instruction(vote(), withdrawer(), new_authority(), role);
            assert_eq!(ix.program_id, VOTE_PROGRAM_ID);
            assert_eq!(ix.data, hex(&format!("01000000{key_hex}{role_hex}")));
            assert_eq!(
                metas(&ix),
                vec![
                    (vote(), true, false),
                    (SYSVAR_CLOCK, false, false),
                    (withdrawer(), false, true),
                ]
            );
        }
    }

    #[test]
    fn update_commission_collector_layout() {
        for (kind, byte) in [
            (CollectorKind::InflationRewards, 0u8),
            (CollectorKind::BlockRevenue, 1u8),
        ] {
            let ix = build_update_commission_collector_instruction(
                vote(),
                recipient(),
                withdrawer(),
                kind,
            );
            assert_eq!(ix.program_id, VOTE_PROGRAM_ID);
            assert_eq!(ix.data, vec![17, 0, 0, 0, byte, 0, 0, 0]);
            assert_eq!(
                metas(&ix),
                vec![
                    (vote(), true, false),
                    (recipient(), true, false),
                    (withdrawer(), false, true),
                ]
            );
        }
    }

    #[test]
    fn withdraw_amount_rules() {
        let balance = 10_000;
        let rent = 2_000;
        let ok = |a| resolve_withdraw_amount(a, balance, rent).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ok(WithdrawAmount::All), 8_000);
        assert_eq!(ok(WithdrawAmount::Close), 10_000);
        assert_eq!(ok(WithdrawAmount::Lamports(8_000)), 8_000);
        assert_eq!(ok(WithdrawAmount::Lamports(1)), 1);

        for bad in [
            WithdrawAmount::Lamports(0),
            WithdrawAmount::Lamports(10_000),
            WithdrawAmount::Lamports(10_001),
            WithdrawAmount::Lamports(8_001),
            WithdrawAmount::Lamports(9_999),
        ] {
            assert!(
                matches!(
                    resolve_withdraw_amount(bad, balance, rent),
                    Err(MsigError::Usage(_))
                ),
                "{bad:?} should be refused"
            );
        }
        match resolve_withdraw_amount(WithdrawAmount::Lamports(9_000), balance, rent) {
            Err(MsigError::Usage(msg)) => {
                assert!(msg.contains("--all") && msg.contains("--close"))
            }
            other => panic!("expected rent refusal, got {other:?}"),
        }
        // Nothing above rent, and an empty account.
        assert!(resolve_withdraw_amount(WithdrawAmount::All, 2_000, rent).is_err());
        assert!(resolve_withdraw_amount(WithdrawAmount::All, 1_000, rent).is_err());
        assert!(resolve_withdraw_amount(WithdrawAmount::Close, 0, rent).is_err());
    }

    #[test]
    fn authority_check_matches_vault() {
        let vote_pk = pk("HUtqwyyi7JHXMwv3GbKZMgURZBRB8cRgfpwgsrQuamuc");
        let withdrawer = pk("BjvitWuqmZCcpmVKnoeWWvtCEPevfL4dqwqEaZ4RsKQC");
        let data = hex(V4_ACCOUNT_PREFIX_HEX);
        let snapshot = parse_vote_account(&vote_pk, &account(VOTE_PROGRAM_ID, 5, data.clone()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(snapshot.lamports, 5);
        assert_eq!(snapshot.data_len, data.len());
        check_vault_is_withdrawer(&snapshot, &withdrawer, 3).unwrap_or_else(|e| panic!("{e}"));

        let other_vault = Pubkey::new_from_array([9u8; 32]);
        match check_vault_is_withdrawer(&snapshot, &other_vault, 3) {
            Err(MsigError::Transaction(msg)) => {
                assert!(msg.contains(&other_vault.to_string()));
                assert!(msg.contains("vault index 3"));
                assert!(msg.contains(&withdrawer.to_string()));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn authority_check_rejects_wrong_owner() {
        let data = synthetic(2, Pubkey::default(), Pubkey::default(), &[0u8; 100]);
        match parse_vote_account(&vote(), &account(SYSTEM_PROGRAM, 5, data)) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("not a vote account")),
            other => panic!("expected owner refusal, got {other:?}"),
        }
    }

    #[test]
    fn external_authority_detection() {
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let program_id = pda::PROGRAM_ID;
        let vaults = vault_addresses(&multisig, &program_id);
        assert_eq!(vaults.len(), 256);
        let vault0 = pda::vault_pda(&multisig, 0, &program_id).0;
        let vault255 = pda::vault_pda(&multisig, 255, &program_id).0;
        assert_eq!(vaults.get(&vault255), Some(&255));

        assert!(!is_external_withdrawer(&vault0, &vaults));
        assert!(!is_external_withdrawer(&vault255, &vaults));
        assert!(is_external_withdrawer(&recipient(), &vaults));

        assert!(!is_external_collector(&vote(), &vote(), &vaults));
        assert!(!is_external_collector(&vault0, &vote(), &vaults));
        assert!(is_external_collector(&recipient(), &vote(), &vaults));
    }

    #[test]
    fn collector_validation() {
        let collector = recipient();
        // The vote account itself needs no account lookup.
        validate_collector(&vote(), &vote(), None, 0).unwrap_or_else(|e| panic!("{e}"));
        // Missing account.
        assert!(validate_collector(&collector, &vote(), None, 890_880).is_err());
        // Wrong owner.
        let wrong = account(VOTE_PROGRAM_ID, 1_000_000, vec![]);
        assert!(validate_collector(&collector, &vote(), Some(&wrong), 890_880).is_err());
        // Below rent exemption.
        let poor = account(SYSTEM_PROGRAM, 890_879, vec![]);
        assert!(validate_collector(&collector, &vote(), Some(&poor), 890_880).is_err());
        // System-owned and rent exempt.
        let good = account(SYSTEM_PROGRAM, 890_880, vec![]);
        validate_collector(&collector, &vote(), Some(&good), 890_880)
            .unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn decodes_withdraw_and_commission() {
        let ix = build_withdraw_instruction(vote(), recipient(), withdrawer(), 1_500_000_000);
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), None, false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote Withdraw 1.5 SOL from {} to {}, authority={}",
                vote(),
                recipient(),
                withdrawer()
            )
        );
        assert!(decoded.warnings.is_empty());

        // A recipient that is one of the multisig's vaults is labelled.
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let vaults = vault_addresses(&multisig, &pda::PROGRAM_ID);
        let vault2 = pda::vault_pda(&multisig, 2, &pda::PROGRAM_ID).0;
        let ix = build_withdraw_instruction(vote(), vault2, withdrawer(), 1_500_000_000);
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert!(decoded.text.contains(&format!("to {vault2} (vault 2),")));

        let ix = build_update_commission_instruction(vote(), withdrawer(), 7);
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), None, false);
        assert_eq!(
            decoded.text,
            format!("Vote UpdateCommission 7% on {}", vote())
        );
    }

    #[test]
    fn decodes_authorize_with_withdrawer_warning() {
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let vaults = vault_addresses(&multisig, &pda::PROGRAM_ID);
        let vault1 = pda::vault_pda(&multisig, 1, &pda::PROGRAM_ID).0;

        // Withdrawer handed to an outside key: warned.
        let ix = build_authorize_instruction(
            vote(),
            withdrawer(),
            new_authority(),
            VoteAuthorize::Withdrawer,
        );
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote Authorize withdrawer on {}: new={}",
                vote(),
                new_authority()
            )
        );
        assert_eq!(decoded.warnings.len(), 1);
        assert!(decoded.warnings[0].contains("not a vault"));

        // Withdrawer moved to another vault of the same multisig: no warning.
        let ix =
            build_authorize_instruction(vote(), withdrawer(), vault1, VoteAuthorize::Withdrawer);
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert!(decoded.warnings.is_empty());

        // Voter changes are never flagged.
        let ix = build_authorize_instruction(
            vote(),
            withdrawer(),
            new_authority(),
            VoteAuthorize::Voter,
        );
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert!(decoded.text.starts_with("Vote Authorize voter"));
        assert!(decoded.warnings.is_empty());

        // VoterWithBLS role tag.
        let mut data = ix.data.clone();
        data[36] = 2;
        let decoded = decode_vote_instruction(&data, &keys(&ix), Some(&vaults), false);
        assert!(decoded.text.starts_with("Vote Authorize voter-with-BLS"));
    }

    #[test]
    fn decodes_authorize_checked_and_identity() {
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let vaults = vault_addresses(&multisig, &pda::PROGRAM_ID);
        let accounts = vec![vote(), SYSVAR_CLOCK, withdrawer(), new_authority()];
        let decoded =
            decode_vote_instruction(&[7, 0, 0, 0, 1, 0, 0, 0], &accounts, Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote AuthorizeChecked withdrawer on {}: new={}",
                vote(),
                new_authority()
            )
        );
        assert_eq!(decoded.warnings.len(), 1);

        let accounts = vec![vote(), new_authority(), withdrawer()];
        let decoded = decode_vote_instruction(&[4, 0, 0, 0], &accounts, Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote UpdateValidatorIdentity on {}: new={}",
                vote(),
                new_authority()
            )
        );
        assert!(decoded.warnings.is_empty());
    }

    #[test]
    fn decodes_collector_with_warning() {
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let vaults = vault_addresses(&multisig, &pda::PROGRAM_ID);

        let ix = build_update_commission_collector_instruction(
            vote(),
            recipient(),
            withdrawer(),
            CollectorKind::BlockRevenue,
        );
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote UpdateCommissionCollector block-revenue on {}: collector={}",
                vote(),
                recipient()
            )
        );
        assert_eq!(decoded.warnings.len(), 1);

        let ix = build_update_commission_collector_instruction(
            vote(),
            vote(),
            withdrawer(),
            CollectorKind::InflationRewards,
        );
        let decoded = decode_vote_instruction(&ix.data, &keys(&ix), Some(&vaults), false);
        assert!(decoded.warnings.is_empty());
    }

    /// Data of `AuthorizeWithSeed` (10) or `AuthorizeCheckedWithSeed` (11):
    /// role, derived key owner, seed (u64 length + bytes), then the new
    /// authority for the unchecked variant only.
    fn seed_authorize_data(tag: u8, role: u8, new_authority: Option<&Pubkey>) -> Vec<u8> {
        let mut data = vec![tag, 0, 0, 0, role, 0, 0, 0];
        data.extend_from_slice(Pubkey::new_from_array([9u8; 32]).as_ref());
        data.extend_from_slice(&4u64.to_le_bytes());
        data.extend_from_slice(b"seed");
        if let Some(new_authority) = new_authority {
            data.extend_from_slice(new_authority.as_ref());
        }
        data
    }

    #[test]
    fn decodes_seed_authorize_and_initialize_with_withdrawer_warning() {
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let vaults = vault_addresses(&multisig, &pda::PROGRAM_ID);
        let vault1 = pda::vault_pda(&multisig, 1, &pda::PROGRAM_ID).0;
        let clock = Pubkey::new_from_array([8u8; 32]);
        let base = [vote(), clock, withdrawer()];

        // AuthorizeWithSeed: new authority in the data.
        let data = seed_authorize_data(10, 1, Some(&new_authority()));
        let decoded = decode_vote_instruction(&data, &base, Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote AuthorizeWithSeed withdrawer on {}: new={}",
                vote(),
                new_authority()
            )
        );
        assert_eq!(decoded.warnings.len(), 1);
        let data = seed_authorize_data(10, 1, Some(&vault1));
        let decoded = decode_vote_instruction(&data, &base, Some(&vaults), false);
        assert!(decoded.warnings.is_empty());

        // AuthorizeCheckedWithSeed: new authority is account 3.
        let data = seed_authorize_data(11, 1, None);
        let accounts = [vote(), clock, withdrawer(), new_authority()];
        let decoded = decode_vote_instruction(&data, &accounts, Some(&vaults), false);
        assert_eq!(
            decoded.text,
            format!(
                "Vote AuthorizeCheckedWithSeed withdrawer on {}: new={}",
                vote(),
                new_authority()
            )
        );
        assert_eq!(decoded.warnings.len(), 1);

        // A voter change through a seed is not flagged; VoterWithBLS is not decoded.
        let data = seed_authorize_data(10, 0, Some(&new_authority()));
        let decoded = decode_vote_instruction(&data, &base, Some(&vaults), false);
        assert!(decoded.warnings.is_empty());
        let data = seed_authorize_data(10, 2, Some(&new_authority()));
        let decoded = decode_vote_instruction(&data, &base, Some(&vaults), false);
        assert_eq!(decoded.text, UNRECOGNISED);

        // InitializeAccount: VoteInit { identity, voter, withdrawer, commission }.
        let mut data = vec![0, 0, 0, 0];
        data.extend_from_slice(Pubkey::new_from_array([1u8; 32]).as_ref());
        data.extend_from_slice(Pubkey::new_from_array([2u8; 32]).as_ref());
        data.extend_from_slice(new_authority().as_ref());
        data.push(5);
        let decoded = decode_vote_instruction(&data, &[vote()], Some(&vaults), false);
        assert!(decoded.text.starts_with("Vote InitializeAccount"));
        assert!(decoded
            .text
            .ends_with(&format!("withdrawer={}", new_authority())));
        assert_eq!(decoded.warnings.len(), 1);
    }

    #[test]
    fn decodes_known_names_and_unrecognised() {
        let decoded = decode_vote_instruction(&[14, 0, 0, 0, 9, 9], &[vote()], None, false);
        assert_eq!(decoded.text, "Vote TowerSync");
        assert!(decoded.warnings.is_empty());

        for data in [
            vec![],
            vec![3, 0, 0, 0, 1],
            vec![99, 0, 0, 0],
            vec![1, 0, 0, 0],
            vec![17, 0, 0, 0, 5, 0, 0, 0],
        ] {
            let decoded = decode_vote_instruction(&data, &[vote()], None, false);
            assert_eq!(decoded.text, UNRECOGNISED, "{data:?}");
            assert_eq!(decoded.warnings.len(), 1);
        }
    }

    /// Serves one vote account; everything else is absent.
    struct VoteAccountRpc {
        vote: Pubkey,
        account: AccountData,
    }

    impl RpcProvider for VoteAccountRpc {
        fn get_account_info(&self, pubkey: &str) -> Result<Option<AccountData>, MsigError> {
            Ok((pubkey == self.vote.to_string()).then(|| self.account.clone()))
        }
        fn get_multiple_accounts(
            &self,
            pubkeys: &[&str],
        ) -> Result<Vec<Option<AccountData>>, MsigError> {
            pubkeys.iter().map(|pk| self.get_account_info(pk)).collect()
        }
        fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError> {
            Err(MsigError::Transaction(
                "unexpected blockhash request".into(),
            ))
        }
        fn get_minimum_balance_for_rent_exemption(&self, _: usize) -> Result<u64, MsigError> {
            Err(MsigError::Transaction("unexpected rent request".into()))
        }
        fn send_transaction(&self, _: &str) -> Result<String, MsigError> {
            Err(MsigError::Transaction("unexpected send".into()))
        }
        fn simulate_transaction(
            &self,
            _: &str,
        ) -> Result<crate::infra::rpc::types::SimulationResult, MsigError> {
            Err(MsigError::Transaction("unexpected simulation".into()))
        }
        fn get_signature_statuses(
            &self,
            _: &[&str],
        ) -> Result<Vec<Option<crate::infra::rpc::types::TxStatus>>, MsigError> {
            Err(MsigError::Transaction("unexpected status request".into()))
        }
    }

    fn identity_request(
        multisig: Pubkey,
        vault_index: u8,
        new_identity: Pubkey,
    ) -> VoteProposalRequest<'static> {
        VoteProposalRequest {
            multisig,
            vault_index,
            vote_account: pk("HUtqwyyi7JHXMwv3GbKZMgURZBRB8cRgfpwgsrQuamuc"),
            operation: VoteOperation::SetIdentity { new_identity },
            allow_external_authority: false,
            allow_simulation_failure: false,
            memo: None,
            dry_run: true,
            skip_confirm: true,
            quiet: true,
        }
    }

    #[test]
    fn set_identity_preflight() {
        let vote_pk = pk("HUtqwyyi7JHXMwv3GbKZMgURZBRB8cRgfpwgsrQuamuc");
        // The fixture's authorized withdrawer stands in for the vault.
        let vault = pk("BjvitWuqmZCcpmVKnoeWWvtCEPevfL4dqwqEaZ4RsKQC");
        let current_identity = pk("F2QJLZ9tBKT1vAs8pRs6WupW5As9uuzpknk3Au7dpZ9b");
        let multisig = Pubkey::new_from_array([7u8; 32]);
        let config = Config::default();
        let rpc = VoteAccountRpc {
            vote: vote_pk,
            account: account(VOTE_PROGRAM_ID, 42, hex(V4_ACCOUNT_PREFIX_HEX)),
        };

        let req = identity_request(multisig, 0, new_authority());
        let (ix, description, external) =
            prepare_operation(&rpc, &config, &req, &vault).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            ix,
            build_update_validator_identity_instruction(vote_pk, new_authority(), vault)
        );
        assert!(description.contains(&new_authority().to_string()));
        assert!(description.contains("co-sign"));
        assert_eq!(external, None);

        // The vault must be the withdrawer.
        let other_vault = Pubkey::new_from_array([9u8; 32]);
        assert!(matches!(
            prepare_operation(&rpc, &config, &req, &other_vault),
            Err(MsigError::Transaction(_))
        ));

        // Current identity, the vote account itself, and a vault are refused.
        let vault3 = pda::vault_pda(&multisig, 3, &config.program_id).0;
        for bad in [current_identity, vote_pk, vault3] {
            let req = identity_request(multisig, 0, bad);
            match prepare_operation(&rpc, &config, &req, &vault) {
                Err(MsigError::Usage(_)) => {}
                other => panic!("{bad} should be refused, got {other:?}"),
            }
        }

        // Wrong owner.
        let rpc = VoteAccountRpc {
            vote: vote_pk,
            account: account(SYSTEM_PROGRAM, 42, hex(V4_ACCOUNT_PREFIX_HEX)),
        };
        assert!(prepare_operation(&rpc, &config, &req, &vault).is_err());
    }

    #[test]
    fn set_identity_next_step_is_online_execute() {
        let text = set_identity_next_step(12, &new_authority());
        assert!(text.contains("msig proposal execute 12 --extra-signer <NEW_IDENTITY_KEYPAIR>"));
        assert!(text.contains(&new_authority().to_string()));
        assert!(!text.contains("tx export"));
    }

    #[test]
    fn show_info_labels_vault_withdrawer() {
        let vote_pk = pk("HUtqwyyi7JHXMwv3GbKZMgURZBRB8cRgfpwgsrQuamuc");
        let withdrawer = pk("BjvitWuqmZCcpmVKnoeWWvtCEPevfL4dqwqEaZ4RsKQC");
        let snapshot = parse_vote_account(
            &vote_pk,
            &account(VOTE_PROGRAM_ID, 42, hex(V4_ACCOUNT_PREFIX_HEX)),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let vaults: HashMap<Pubkey, u8> = [(withdrawer, 2u8)].into_iter().collect();
        let info = vote_account_info(&snapshot, 7, Some(&vaults));
        assert_eq!(info.authorized_withdrawer_vault_index, Some(2));
        assert_eq!(info.inflation_rewards_commission_bps, Some(300));
        assert_eq!(info.block_revenue_commission_bps, Some(10_000));
        assert_eq!(info.commission_percent, None);
        assert_eq!(info.inflation_rewards_collector, Some(vote_pk));
        assert!(info.bls_key_registered);
        assert_eq!(
            vote_account_info(&snapshot, 7, None).authorized_withdrawer_vault_index,
            None
        );
    }
}
