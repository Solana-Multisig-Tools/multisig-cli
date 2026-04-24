use serde::Serialize;
use solana_pubkey::Pubkey;

use super::serde_pubkey;

/// The status of a proposal, kept as a typed enum end-to-end.
/// Never flattened to String except at the output boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProposalStatus {
    Draft { timestamp: i64 },
    Active { timestamp: i64 },
    Rejected { timestamp: i64 },
    Approved { timestamp: i64 },
    Executing,
    Executed { timestamp: i64 },
    Cancelled { timestamp: i64 },
}

impl ProposalStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Draft { .. } => "Draft",
            Self::Active { .. } => "Active",
            Self::Rejected { .. } => "Rejected",
            Self::Approved { .. } => "Approved",
            Self::Executing => "Executing",
            Self::Executed { .. } => "Executed",
            Self::Cancelled { .. } => "Cancelled",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved { .. })
    }

    pub fn timestamp(&self) -> Option<i64> {
        match self {
            Self::Draft { timestamp }
            | Self::Active { timestamp }
            | Self::Rejected { timestamp }
            | Self::Approved { timestamp }
            | Self::Executed { timestamp }
            | Self::Cancelled { timestamp } => Some(*timestamp),
            Self::Executing => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Executed { .. } | Self::Cancelled { .. } | Self::Rejected { .. }
        )
    }
}

/// The three vote actions a member can take on a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    Approve,
    Reject,
    Cancel,
}

impl Vote {
    /// Return the 8-byte instruction discriminator for this vote.
    pub fn discriminator(self) -> [u8; 8] {
        match self {
            Self::Approve => [0x90, 0x25, 0xa4, 0x88, 0xbc, 0xd8, 0x2a, 0xf8],
            Self::Reject => [0xf3, 0x3e, 0x86, 0x9c, 0xe6, 0x6a, 0xf6, 0x87],
            Self::Cancel => [0x1b, 0x2a, 0x7f, 0xed, 0x26, 0xa3, 0x54, 0xcb],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
            Self::Cancel => "Cancel",
        }
    }
}

/// Summary view of a proposal (for list display).
#[derive(Debug, Clone, Serialize)]
pub struct ProposalSummary {
    pub index: u64,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub address: Pubkey,
    pub status: ProposalStatus,
    pub approved_count: usize,
    pub rejected_count: usize,
    pub threshold: u16,
}

/// Full proposal detail including voter lists and transaction info.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalDetail {
    pub summary: ProposalSummary,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub multisig: Pubkey,
    #[serde(serialize_with = "serde_pubkey::vec::serialize")]
    pub approved: Vec<Pubkey>,
    #[serde(serialize_with = "serde_pubkey::vec::serialize")]
    pub rejected: Vec<Pubkey>,
    #[serde(serialize_with = "serde_pubkey::vec::serialize")]
    pub cancelled: Vec<Pubkey>,
    pub transaction_type: TransactionType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_tx: Option<super::transaction::VaultTxInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_tx: Option<super::transaction::ConfigTxInfo>,
}

/// Whether the underlying transaction is a vault tx or config tx.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TransactionType {
    Vault,
    Config,
    Unknown,
}

impl TransactionType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Vault => "vault",
            Self::Config => "config",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels() {
        assert_eq!(ProposalStatus::Active { timestamp: 0 }.label(), "Active");
        assert_eq!(ProposalStatus::Executing.label(), "Executing");
    }

    #[test]
    fn status_predicates() {
        assert!(ProposalStatus::Active { timestamp: 0 }.is_active());
        assert!(!ProposalStatus::Active { timestamp: 0 }.is_approved());
        assert!(ProposalStatus::Executed { timestamp: 0 }.is_terminal());
        assert!(!ProposalStatus::Active { timestamp: 0 }.is_terminal());
    }

    #[test]
    fn vote_discriminators_are_8_bytes() {
        assert_eq!(Vote::Approve.discriminator().len(), 8);
        assert_eq!(Vote::Reject.discriminator().len(), 8);
        assert_eq!(Vote::Cancel.discriminator().len(), 8);
    }
}
