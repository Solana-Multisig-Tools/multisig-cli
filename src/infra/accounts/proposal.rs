use solana_pubkey::Pubkey;

use super::discriminators;
use super::reader::SafeReader;
use crate::domain::proposal::ProposalStatus;
use crate::error::ParseError;

/// Raw on-chain proposal account.
#[derive(Debug, Clone)]
pub struct ProposalAccount {
    pub multisig: Pubkey,
    #[allow(dead_code)]
    pub transaction_index: u64,
    pub status: ProposalStatus,
    #[allow(dead_code)]
    pub bump: u8,
    pub approved: Vec<Pubkey>,
    pub rejected: Vec<Pubkey>,
    pub cancelled: Vec<Pubkey>,
}

impl ProposalAccount {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = SafeReader::new(data);
        r.read_discriminator(&discriminators::PROPOSAL)?;

        let multisig = r.read_solana_pubkey()?;
        let transaction_index = r.read_u64_le()?;
        // Note: no rent_collector field in Squads v4 Proposal account
        let status = parse_proposal_status(&mut r)?;
        let bump = r.read_u8()?;
        let approved = r.read_vec(65535, |rr| rr.read_solana_pubkey())?;
        let rejected = r.read_vec(65535, |rr| rr.read_solana_pubkey())?;
        let cancelled = r.read_vec(65535, |rr| rr.read_solana_pubkey())?;

        Ok(ProposalAccount {
            multisig,
            transaction_index,
            status,
            bump,
            approved,
            rejected,
            cancelled,
        })
    }
}

fn parse_proposal_status(r: &mut SafeReader<'_>) -> Result<ProposalStatus, ParseError> {
    let tag = r.read_u8()?;
    match tag {
        0 => Ok(ProposalStatus::Draft {
            timestamp: r.read_i64_le()?,
        }),
        1 => Ok(ProposalStatus::Active {
            timestamp: r.read_i64_le()?,
        }),
        2 => Ok(ProposalStatus::Rejected {
            timestamp: r.read_i64_le()?,
        }),
        3 => Ok(ProposalStatus::Approved {
            timestamp: r.read_i64_le()?,
        }),
        4 => Ok(ProposalStatus::Executing),
        5 => Ok(ProposalStatus::Executed {
            timestamp: r.read_i64_le()?,
        }),
        6 => Ok(ProposalStatus::Cancelled {
            timestamp: r.read_i64_le()?,
        }),
        _ => Err(ParseError::InvalidTag {
            tag,
            type_name: "ProposalStatus",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_proposal_data(status_tag: u8, has_timestamp: bool) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&discriminators::PROPOSAL);
        buf.extend_from_slice(&[1u8; 32]); // multisig
        buf.extend_from_slice(&42u64.to_le_bytes());
        buf.push(status_tag);
        if has_timestamp {
            buf.extend_from_slice(&1000i64.to_le_bytes());
        }
        buf.push(1); // bump
        buf.extend_from_slice(&0u32.to_le_bytes()); // approved
        buf.extend_from_slice(&0u32.to_le_bytes()); // rejected
        buf.extend_from_slice(&0u32.to_le_bytes()); // cancelled
        buf
    }

    #[test]
    fn proposal_status_active() {
        let data = build_proposal_data(1, true);
        let p = ProposalAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.status.is_active());
        assert_eq!(p.status.label(), "Active");
    }

    #[test]
    fn proposal_status_executing_no_payload() {
        let data = build_proposal_data(4, false);
        let p = ProposalAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(p.status, ProposalStatus::Executing);
    }

    #[test]
    fn proposal_invalid_status_tag() {
        let data = build_proposal_data(7, true);
        assert!(matches!(
            ProposalAccount::parse(&data),
            Err(ParseError::InvalidTag { tag: 7, .. })
        ));
    }
}
