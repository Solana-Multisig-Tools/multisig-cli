use serde::Serialize;
use solana_pubkey::Pubkey;

use super::serde_pubkey;

/// Vault transaction info decoded from on-chain data.
#[derive(Debug, Clone, Serialize)]
pub struct VaultTxInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub creator: Pubkey,
    pub vault_index: u8,
    pub instruction_count: usize,
    pub instructions: Vec<InstructionSummary>,
}

/// A reference to an account in a compiled instruction, with signer/writable flags.
#[derive(Debug, Clone, Serialize)]
pub struct AccountRef {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub address: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

/// Per-instruction metadata for display.
#[derive(Debug, Clone, Serialize)]
pub struct InstructionSummary {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub program_id: Pubkey,
    pub program_name: String,
    pub accounts: Vec<AccountRef>,
    pub data_hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded: Option<String>,
}

/// Config transaction info.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigTxInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub creator: Pubkey,
    pub action_count: usize,
    pub actions: Vec<ConfigAction>,
}

/// Decoded config action from a ConfigTransaction.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ConfigAction {
    AddMember {
        #[serde(serialize_with = "serde_pubkey::serialize")]
        member: Pubkey,
        permissions: u8,
    },
    RemoveMember {
        #[serde(serialize_with = "serde_pubkey::serialize")]
        member: Pubkey,
    },
    ChangeThreshold {
        new_threshold: u16,
    },
    SetTimeLock {
        new_time_lock: u32,
    },
    AddSpendingLimit {
        #[serde(serialize_with = "serde_pubkey::serialize")]
        create_key: Pubkey,
        vault_index: u8,
        #[serde(serialize_with = "serde_pubkey::serialize")]
        mint: Pubkey,
        amount: u64,
        period: SpendingLimitPeriod,
        members: Vec<Pubkey>,
        destinations: Vec<Pubkey>,
    },
    RemoveSpendingLimit {
        #[serde(serialize_with = "serde_pubkey::serialize")]
        spending_limit: Pubkey,
    },
    SetRentCollector {
        #[serde(serialize_with = "serde_pubkey::option::serialize")]
        new_rent_collector: Option<Pubkey>,
    },
    Unknown {
        tag: u8,
        raw: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendingLimitPeriod {
    OneTime,
    Day,
    Week,
    Month,
}
