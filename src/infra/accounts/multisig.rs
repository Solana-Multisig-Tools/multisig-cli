use solana_pubkey::Pubkey;

use super::discriminators;
use super::reader::SafeReader;
use crate::domain::multisig::{Member, Permissions};
use crate::error::ParseError;

/// Raw on-chain multisig account data.
/// The application layer enriches this into `domain::multisig::MultisigInfo`
/// by adding the address, vault PDA, and vault balance from RPC.
#[derive(Debug, Clone)]
pub struct MultisigAccount {
    pub create_key: Pubkey,
    pub config_authority: Pubkey,
    pub threshold: u16,
    pub time_lock: u32,
    pub transaction_index: u64,
    pub stale_transaction_index: u64,
    pub rent_collector: Option<Pubkey>,
    pub bump: u8,
    pub members: Vec<Member>,
}

impl MultisigAccount {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = SafeReader::new(data);
        r.read_discriminator(&discriminators::MULTISIG)?;

        let create_key = r.read_solana_pubkey()?;
        let config_authority = r.read_solana_pubkey()?;
        let threshold = r.read_u16_le()?;
        let time_lock = r.read_u32_le()?;
        let transaction_index = r.read_u64_le()?;
        let stale_transaction_index = r.read_u64_le()?;
        let rent_collector = r.read_option(|rr| rr.read_solana_pubkey())?;
        let bump = r.read_u8()?;
        let members = r.read_vec(65535, |rr| {
            let key = rr.read_solana_pubkey()?;
            let permissions = Permissions(rr.read_u8()?);
            Ok(Member { key, permissions })
        })?;

        Ok(MultisigAccount {
            create_key,
            config_authority,
            threshold,
            time_lock,
            transaction_index,
            stale_transaction_index,
            rent_collector,
            bump,
            members,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_multisig_data(member_count: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&discriminators::MULTISIG);
        buf.extend_from_slice(&[1u8; 32]); // create_key
        buf.extend_from_slice(&[2u8; 32]); // config_authority
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&5u64.to_le_bytes());
        buf.extend_from_slice(&3u64.to_le_bytes());
        buf.push(0); // rent_collector = None
        buf.push(255); // bump
        buf.extend_from_slice(&member_count.to_le_bytes());
        for i in 0..member_count {
            let mut key = [0u8; 32];
            key[0] = (i + 10) as u8;
            buf.extend_from_slice(&key);
            buf.push(0b111);
        }
        buf
    }

    #[test]
    fn parse_multisig() {
        let data = build_multisig_data(3);
        let ms = MultisigAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ms.threshold, 2);
        assert_eq!(ms.transaction_index, 5);
        assert_eq!(ms.members.len(), 3);
        assert!(ms.members[0].permissions.can_initiate());
        assert!(ms.members[0].permissions.can_vote());
        assert!(ms.members[0].permissions.can_execute());
    }

    #[test]
    fn parse_multisig_wrong_discriminator() {
        let mut data = build_multisig_data(1);
        data[0] = 0xFF;
        assert!(matches!(
            MultisigAccount::parse(&data),
            Err(ParseError::InvalidDiscriminator { .. })
        ));
    }

    #[test]
    fn parse_multisig_truncated() {
        let data = build_multisig_data(1);
        let truncated = &data[..20];
        assert!(matches!(
            MultisigAccount::parse(truncated),
            Err(ParseError::Eof { .. })
        ));
    }
}
