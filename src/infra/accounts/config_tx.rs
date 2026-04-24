use solana_pubkey::Pubkey;

use super::discriminators;
use super::reader::SafeReader;
use crate::domain::transaction::{ConfigAction, SpendingLimitPeriod};
use crate::error::ParseError;

#[derive(Debug, Clone)]
pub struct ConfigTransactionAccount {
    #[allow(dead_code)]
    pub multisig: Pubkey,
    pub creator: Pubkey,
    #[allow(dead_code)]
    pub index: u64,
    #[allow(dead_code)]
    pub bump: u8,
    pub actions: Vec<ConfigAction>,
}

impl ConfigTransactionAccount {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = SafeReader::new(data);
        r.read_discriminator(&discriminators::CONFIG_TRANSACTION)?;

        let multisig = r.read_solana_pubkey()?;
        let creator = r.read_solana_pubkey()?;
        let index = r.read_u64_le()?;
        let bump = r.read_u8()?;

        let action_count = r.read_u32_le()?;
        if action_count > 256 {
            return Err(ParseError::VecTooLong {
                len: action_count,
                cap: 256,
            });
        }

        let mut actions = Vec::with_capacity(action_count as usize);
        for _ in 0..action_count {
            actions.push(parse_config_action(&mut r)?);
        }

        Ok(ConfigTransactionAccount {
            multisig,
            creator,
            index,
            bump,
            actions,
        })
    }
}

fn parse_config_action(r: &mut SafeReader<'_>) -> Result<ConfigAction, ParseError> {
    let tag = r.read_u8()?;
    match tag {
        0 => {
            let member = r.read_solana_pubkey()?;
            let permissions = r.read_u8()?;
            Ok(ConfigAction::AddMember {
                member,
                permissions,
            })
        }
        1 => {
            let member = r.read_solana_pubkey()?;
            Ok(ConfigAction::RemoveMember { member })
        }
        2 => {
            let new_threshold = r.read_u16_le()?;
            Ok(ConfigAction::ChangeThreshold { new_threshold })
        }
        3 => {
            let new_time_lock = r.read_u32_le()?;
            Ok(ConfigAction::SetTimeLock { new_time_lock })
        }
        4 => {
            let create_key = r.read_solana_pubkey()?;
            let vault_index = r.read_u8()?;
            let mint = r.read_solana_pubkey()?;
            let amount = r.read_u64_le()?;
            let period = parse_period(r.read_u8()?)?;
            let members = read_pubkey_vec(r, 256)?;
            let destinations = read_pubkey_vec(r, 256)?;
            Ok(ConfigAction::AddSpendingLimit {
                create_key,
                vault_index,
                mint,
                amount,
                period,
                members,
                destinations,
            })
        }
        5 => {
            let spending_limit = r.read_solana_pubkey()?;
            Ok(ConfigAction::RemoveSpendingLimit { spending_limit })
        }
        6 => {
            let new_rent_collector = r.read_option(|rr| rr.read_solana_pubkey())?;
            Ok(ConfigAction::SetRentCollector { new_rent_collector })
        }
        _ => {
            let remaining = r.remaining();
            let raw = r.read_bytes(remaining)?.to_vec();
            Ok(ConfigAction::Unknown { tag, raw })
        }
    }
}

fn parse_period(tag: u8) -> Result<SpendingLimitPeriod, ParseError> {
    match tag {
        0 => Ok(SpendingLimitPeriod::OneTime),
        1 => Ok(SpendingLimitPeriod::Day),
        2 => Ok(SpendingLimitPeriod::Week),
        3 => Ok(SpendingLimitPeriod::Month),
        _ => Err(ParseError::InvalidTag {
            tag,
            type_name: "SpendingLimitPeriod",
        }),
    }
}

fn read_pubkey_vec(r: &mut SafeReader<'_>, cap: u32) -> Result<Vec<Pubkey>, ParseError> {
    let len = r.read_u32_le()?;
    if len > cap {
        return Err(ParseError::VecTooLong { len, cap });
    }
    let mut out = Vec::with_capacity(len as usize);
    for _ in 0..len {
        out.push(r.read_solana_pubkey()?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_config_tx_data(action_count: u32, action_bytes: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&discriminators::CONFIG_TRANSACTION);
        buf.extend_from_slice(&[1u8; 32]); // multisig
        buf.extend_from_slice(&[2u8; 32]); // creator
        buf.extend_from_slice(&10u64.to_le_bytes());
        buf.push(200); // bump
        buf.extend_from_slice(&action_count.to_le_bytes());
        buf.extend_from_slice(action_bytes);
        buf
    }

    #[test]
    fn parse_no_actions() {
        let data = build_config_tx_data(0, &[]);
        let ctx = ConfigTransactionAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ctx.index, 10);
        assert_eq!(ctx.actions.len(), 0);
    }

    #[test]
    fn parse_add_member_action() {
        let mut action_bytes = Vec::new();
        action_bytes.push(0); // AddMember
        action_bytes.extend_from_slice(&[0xAA; 32]);
        action_bytes.push(0b111);

        let data = build_config_tx_data(1, &action_bytes);
        let ctx = ConfigTransactionAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            &ctx.actions[0],
            ConfigAction::AddMember {
                permissions: 0b111,
                ..
            }
        ));
    }

    #[test]
    fn parse_change_threshold() {
        let mut action_bytes = Vec::new();
        action_bytes.push(2);
        action_bytes.extend_from_slice(&3u16.to_le_bytes());

        let data = build_config_tx_data(1, &action_bytes);
        let ctx = ConfigTransactionAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            &ctx.actions[0],
            ConfigAction::ChangeThreshold { new_threshold: 3 }
        ));
    }

    #[test]
    fn parse_spending_limit_actions_without_consuming_following_action() {
        let mut action_bytes = Vec::new();
        action_bytes.push(4);
        action_bytes.extend_from_slice(&[3u8; 32]); // create_key
        action_bytes.push(2); // vault_index
        action_bytes.extend_from_slice(&[4u8; 32]); // mint
        action_bytes.extend_from_slice(&500u64.to_le_bytes());
        action_bytes.push(1); // day
        action_bytes.extend_from_slice(&1u32.to_le_bytes());
        action_bytes.extend_from_slice(&[5u8; 32]); // member
        action_bytes.extend_from_slice(&0u32.to_le_bytes()); // destinations
        action_bytes.push(5);
        action_bytes.extend_from_slice(&[6u8; 32]); // remove spending_limit

        let data = build_config_tx_data(2, &action_bytes);
        let ctx = ConfigTransactionAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));

        assert!(matches!(
            &ctx.actions[0],
            ConfigAction::AddSpendingLimit {
                vault_index: 2,
                amount: 500,
                ..
            }
        ));
        assert!(matches!(
            &ctx.actions[1],
            ConfigAction::RemoveSpendingLimit { .. }
        ));
    }

    #[test]
    fn wrong_discriminator() {
        let mut data = build_config_tx_data(0, &[]);
        data[0] = 0xFF;
        assert!(matches!(
            ConfigTransactionAccount::parse(&data),
            Err(ParseError::InvalidDiscriminator { .. })
        ));
    }
}
