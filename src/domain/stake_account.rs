use serde::Serialize;
use solana_pubkey::Pubkey;

use super::serde_pubkey;

/// Read-only view of a stake account for `stake-account show`.
#[derive(Debug, Clone, Serialize)]
pub struct StakeAccountInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub stake_account: Pubkey,
    /// `uninitialized`, `initialized`, `delegated` or `rewards_pool`.
    pub state: String,
    pub balance_lamports: u64,
    /// Present once the account is initialized.
    pub rent_exempt_reserve_lamports: Option<u64>,
    #[serde(serialize_with = "serde_pubkey::option::serialize")]
    pub staker: Option<Pubkey>,
    /// Vault index when the staker is one of the multisig's vaults.
    pub staker_vault_index: Option<u8>,
    #[serde(serialize_with = "serde_pubkey::option::serialize")]
    pub withdrawer: Option<Pubkey>,
    /// Vault index when the withdrawer is one of the multisig's vaults.
    pub withdrawer_vault_index: Option<u8>,
    pub lockup: Option<StakeLockupInfo>,
    pub delegation: Option<StakeDelegationInfo>,
    pub current_epoch: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StakeLockupInfo {
    pub unix_timestamp: i64,
    pub epoch: u64,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub custodian: Pubkey,
    pub in_force: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StakeDelegationInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub vote_account: Pubkey,
    pub stake_lamports: u64,
    pub activation_epoch: u64,
    /// `None` while the stake is not deactivating.
    pub deactivation_epoch: Option<u64>,
}
