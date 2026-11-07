use serde::Serialize;
use solana_pubkey::Pubkey;

use super::serde_pubkey;

/// Read-only view of a vote account for `vote-account show`.
#[derive(Debug, Clone, Serialize)]
pub struct VoteAccountInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub vote_account: Pubkey,
    pub balance_lamports: u64,
    pub rent_exempt_minimum_lamports: u64,
    pub version: String,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub validator_identity: Pubkey,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub authorized_withdrawer: Pubkey,
    /// Vault index when the withdrawer is one of the active multisig's vaults.
    pub authorized_withdrawer_vault_index: Option<u8>,
    /// V1_14_11 / V3 only.
    pub commission_percent: Option<u8>,
    /// V4 only.
    pub inflation_rewards_commission_bps: Option<u16>,
    /// V4 only.
    pub block_revenue_commission_bps: Option<u16>,
    #[serde(serialize_with = "serde_pubkey::option::serialize")]
    pub inflation_rewards_collector: Option<Pubkey>,
    #[serde(serialize_with = "serde_pubkey::option::serialize")]
    pub block_revenue_collector: Option<Pubkey>,
    pub bls_key_registered: bool,
}
