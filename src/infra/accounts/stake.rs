//! Hand-written parsers for a Stake program account (`StakeStateV2`) and the
//! Clock sysvar.
//!
//! `StakeStateV2` (bincode, little-endian, 200 bytes):
//! - `0..4`     u32 tag: 0 Uninitialized, 1 Initialized(Meta), 2 Stake(Meta,
//!   Stake, StakeFlags), 3 RewardsPool
//! - Meta: `4..12` rent_exempt_reserve u64, `12..44` staker, `44..76`
//!   withdrawer, `76..84` lockup.unix_timestamp i64, `84..92` lockup.epoch
//!   u64, `92..124` lockup.custodian
//! - Stake (tag 2): `124..156` delegation.voter_pubkey, `156..164`
//!   delegation.stake u64, `164..172` activation_epoch u64, `172..180`
//!   deactivation_epoch u64 (u64::MAX = not deactivating), `180..188`
//!   warmup_cooldown_rate f64 (deprecated; skipped, never read as a float),
//!   `188..196` credits_observed u64, `196` StakeFlags u8
//!
//! Clock sysvar: slot u64, epoch_start_timestamp i64, epoch u64,
//! leader_schedule_epoch u64, unix_timestamp i64.

use solana_pubkey::Pubkey;

use super::reader::SafeReader;
use crate::error::MsigError;

/// `deactivation_epoch` value of a stake that is not deactivating.
pub const NOT_DEACTIVATING: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lockup {
    pub unix_timestamp: i64,
    pub epoch: u64,
    pub custodian: Pubkey,
}

impl Lockup {
    /// Whether the lockup still restricts withdrawals and withdrawer changes.
    /// The custodian is ignored: msig does not support custodian signatures.
    pub fn is_in_force(&self, clock: &Clock) -> bool {
        self.unix_timestamp > clock.unix_timestamp || self.epoch > clock.epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakeMeta {
    pub rent_exempt_reserve: u64,
    pub staker: Pubkey,
    pub withdrawer: Pubkey,
    pub lockup: Lockup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delegation {
    pub voter: Pubkey,
    pub stake: u64,
    pub activation_epoch: u64,
    pub deactivation_epoch: u64,
    pub credits_observed: u64,
}

impl Delegation {
    pub fn is_deactivating(&self) -> bool {
        self.deactivation_epoch != NOT_DEACTIVATING
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakeAccountState {
    Uninitialized,
    Initialized(StakeMeta),
    Stake {
        meta: StakeMeta,
        delegation: Delegation,
        flags: u8,
    },
    RewardsPool,
}

impl StakeAccountState {
    pub fn parse(data: &[u8]) -> Result<Self, MsigError> {
        let mut r = SafeReader::new(data);
        match r.read_u32_le()? {
            0 => Ok(Self::Uninitialized),
            1 => Ok(Self::Initialized(read_meta(&mut r)?)),
            2 => {
                let meta = read_meta(&mut r)?;
                let voter = r.read_solana_pubkey()?;
                let stake = r.read_u64_le()?;
                let activation_epoch = r.read_u64_le()?;
                let deactivation_epoch = r.read_u64_le()?;
                // warmup_cooldown_rate: deprecated f64, unused.
                r.skip(8)?;
                let credits_observed = r.read_u64_le()?;
                let flags = r.read_u8()?;
                Ok(Self::Stake {
                    meta,
                    delegation: Delegation {
                        voter,
                        stake,
                        activation_epoch,
                        deactivation_epoch,
                        credits_observed,
                    },
                    flags,
                })
            }
            3 => Ok(Self::RewardsPool),
            other => Err(MsigError::Transaction(format!(
                "unknown stake account state {other}; expected 0 to 3"
            ))),
        }
    }

    pub fn meta(&self) -> Option<&StakeMeta> {
        match self {
            Self::Initialized(meta) | Self::Stake { meta, .. } => Some(meta),
            Self::Uninitialized | Self::RewardsPool => None,
        }
    }

    pub fn delegation(&self) -> Option<&Delegation> {
        match self {
            Self::Stake { delegation, .. } => Some(delegation),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Initialized(_) => "initialized",
            Self::Stake { .. } => "delegated",
            Self::RewardsPool => "rewards_pool",
        }
    }
}

fn read_meta(r: &mut SafeReader<'_>) -> Result<StakeMeta, MsigError> {
    Ok(StakeMeta {
        rent_exempt_reserve: r.read_u64_le()?,
        staker: r.read_solana_pubkey()?,
        withdrawer: r.read_solana_pubkey()?,
        lockup: Lockup {
            unix_timestamp: r.read_i64_le()?,
            epoch: r.read_u64_le()?,
            custodian: r.read_solana_pubkey()?,
        },
    })
}

/// The fields of the Clock sysvar msig uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub slot: u64,
    pub epoch: u64,
    pub unix_timestamp: i64,
}

impl Clock {
    pub fn parse(data: &[u8]) -> Result<Self, MsigError> {
        let mut r = SafeReader::new(data);
        let slot = r.read_u64_le()?;
        let _epoch_start_timestamp = r.read_i64_le()?;
        let epoch = r.read_u64_le()?;
        let _leader_schedule_epoch = r.read_u64_le()?;
        let unix_timestamp = r.read_i64_le()?;
        Ok(Self {
            slot,
            epoch,
            unix_timestamp,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::ParseError;

    pub(crate) fn meta_bytes(meta: &StakeMeta) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&meta.rent_exempt_reserve.to_le_bytes());
        data.extend_from_slice(meta.staker.as_ref());
        data.extend_from_slice(meta.withdrawer.as_ref());
        data.extend_from_slice(&meta.lockup.unix_timestamp.to_le_bytes());
        data.extend_from_slice(&meta.lockup.epoch.to_le_bytes());
        data.extend_from_slice(meta.lockup.custodian.as_ref());
        data
    }

    /// A 200-byte `StakeStateV2` buffer for `state`.
    pub(crate) fn stake_account_bytes(state: &StakeAccountState) -> Vec<u8> {
        let mut data = Vec::new();
        match state {
            StakeAccountState::Uninitialized => data.extend_from_slice(&0u32.to_le_bytes()),
            StakeAccountState::Initialized(meta) => {
                data.extend_from_slice(&1u32.to_le_bytes());
                data.extend_from_slice(&meta_bytes(meta));
            }
            StakeAccountState::Stake {
                meta,
                delegation,
                flags,
            } => {
                data.extend_from_slice(&2u32.to_le_bytes());
                data.extend_from_slice(&meta_bytes(meta));
                data.extend_from_slice(delegation.voter.as_ref());
                data.extend_from_slice(&delegation.stake.to_le_bytes());
                data.extend_from_slice(&delegation.activation_epoch.to_le_bytes());
                data.extend_from_slice(&delegation.deactivation_epoch.to_le_bytes());
                // warmup_cooldown_rate 0.25 as an f64 bit pattern.
                data.extend_from_slice(&0x3FD0_0000_0000_0000u64.to_le_bytes());
                data.extend_from_slice(&delegation.credits_observed.to_le_bytes());
                data.push(*flags);
            }
            StakeAccountState::RewardsPool => data.extend_from_slice(&3u32.to_le_bytes()),
        }
        data.resize(200, 0);
        data
    }

    pub(crate) fn sample_meta() -> StakeMeta {
        StakeMeta {
            rent_exempt_reserve: 2_282_880,
            staker: Pubkey::new_from_array([1u8; 32]),
            withdrawer: Pubkey::new_from_array([2u8; 32]),
            lockup: Lockup {
                unix_timestamp: 0,
                epoch: 0,
                custodian: Pubkey::default(),
            },
        }
    }

    pub(crate) fn sample_delegation() -> Delegation {
        Delegation {
            voter: Pubkey::new_from_array([3u8; 32]),
            stake: 5_000_000_000,
            activation_epoch: 100,
            deactivation_epoch: NOT_DEACTIVATING,
            credits_observed: 42,
        }
    }

    pub(crate) fn clock(epoch: u64, unix_timestamp: i64) -> Clock {
        Clock {
            slot: 1,
            epoch,
            unix_timestamp,
        }
    }

    #[test]
    fn parses_each_state() {
        let meta = sample_meta();
        let delegation = sample_delegation();
        for state in [
            StakeAccountState::Uninitialized,
            StakeAccountState::Initialized(meta),
            StakeAccountState::Stake {
                meta,
                delegation,
                flags: 1,
            },
            StakeAccountState::RewardsPool,
        ] {
            let data = stake_account_bytes(&state);
            assert_eq!(data.len(), 200);
            let parsed = StakeAccountState::parse(&data).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn reads_fields_at_documented_offsets() {
        let meta = sample_meta();
        let delegation = Delegation {
            deactivation_epoch: 120,
            ..sample_delegation()
        };
        let data = stake_account_bytes(&StakeAccountState::Stake {
            meta,
            delegation,
            flags: 0,
        });
        assert_eq!(&data[12..44], meta.staker.as_ref());
        assert_eq!(&data[44..76], meta.withdrawer.as_ref());
        assert_eq!(&data[124..156], delegation.voter.as_ref());
        assert_eq!(data[172..180], 120u64.to_le_bytes());
        assert_eq!(data[188..196], 42u64.to_le_bytes());
        let parsed = StakeAccountState::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.meta(), Some(&meta));
        assert!(parsed.delegation().is_some_and(Delegation::is_deactivating));
        assert_eq!(parsed.label(), "delegated");
    }

    #[test]
    fn rejects_unknown_tag_and_short_data() {
        let mut data = stake_account_bytes(&StakeAccountState::Uninitialized);
        data[0] = 4;
        assert!(matches!(
            StakeAccountState::parse(&data),
            Err(MsigError::Transaction(_))
        ));
        assert!(matches!(
            StakeAccountState::parse(&[1, 0]),
            Err(MsigError::Parse(_))
        ));
        let mut data = stake_account_bytes(&StakeAccountState::Initialized(sample_meta()));
        data.truncate(100);
        assert!(matches!(
            StakeAccountState::parse(&data),
            Err(MsigError::Parse(ParseError::Eof { .. }))
        ));
        let mut data = stake_account_bytes(&StakeAccountState::Stake {
            meta: sample_meta(),
            delegation: sample_delegation(),
            flags: 0,
        });
        data.truncate(196);
        assert!(matches!(
            StakeAccountState::parse(&data),
            Err(MsigError::Parse(ParseError::Eof { offset: 196, .. }))
        ));
    }

    #[test]
    fn lockup_in_force_rules() {
        let mut lockup = sample_meta().lockup;
        let now = clock(500, 1_700_000_000);
        assert!(!lockup.is_in_force(&now));
        lockup.epoch = 501;
        assert!(lockup.is_in_force(&now));
        lockup.epoch = 500;
        assert!(!lockup.is_in_force(&now));
        lockup.unix_timestamp = 1_700_000_001;
        assert!(lockup.is_in_force(&now));
        lockup.unix_timestamp = 1_700_000_000;
        assert!(!lockup.is_in_force(&now));
        // A custodian alone does not put a lockup in force.
        lockup.custodian = Pubkey::new_from_array([9u8; 32]);
        assert!(!lockup.is_in_force(&now));
    }

    #[test]
    fn parses_clock_sysvar() {
        let mut data = Vec::new();
        data.extend_from_slice(&123u64.to_le_bytes());
        data.extend_from_slice(&(-5i64).to_le_bytes());
        data.extend_from_slice(&456u64.to_le_bytes());
        data.extend_from_slice(&457u64.to_le_bytes());
        data.extend_from_slice(&1_700_000_000i64.to_le_bytes());
        let clock = Clock::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(clock.slot, 123);
        assert_eq!(clock.epoch, 456);
        assert_eq!(clock.unix_timestamp, 1_700_000_000);
        assert!(Clock::parse(&data[..39]).is_err());
    }
}
