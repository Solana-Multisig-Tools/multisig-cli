//! Hand-written parser for the fixed-offset header of a Solana vote account.
//!
//! Only the fields msig needs are read. solana-vote-interface 2.x cannot parse
//! V4 accounts, and pulling it in would add a large dependency tree for a few
//! offsets, so the layout is decoded here by offset.
//!
//! Layout (bincode, little-endian):
//! - `0..4`    u32 version tag: 0 = V0_23_5, 1 = V1_14_11, 2 = V3, 3 = V4
//! - `4..36`   node_pubkey (validator identity)
//! - `36..68`  authorized_withdrawer
//! - V1_14_11 / V3: `68` commission (u8 percent)
//! - V4: `68..100` inflation_rewards_collector, `100..132` block_revenue_collector,
//!   `132..134` inflation_rewards_commission_bps (u16), `134..136`
//!   block_revenue_commission_bps (u16), `136..144` pending_delegator_rewards
//!   (u64), `144` `Option<[u8; 48]>` BLS public key tag.

use solana_pubkey::Pubkey;

use super::reader::SafeReader;
use crate::error::MsigError;

const BLS_PUBKEY_COMPRESSED_LEN: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteStateVersion {
    V1_14_11,
    V3,
    V4,
}

impl VoteStateVersion {
    pub fn label(&self) -> &'static str {
        match self {
            Self::V1_14_11 => "V1_14_11",
            Self::V3 => "V3",
            Self::V4 => "V4",
        }
    }
}

/// Commission as stored on chain: a whole percent before V4, basis points
/// split between inflation rewards and block revenue from V4 on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteCommission {
    Percent(u8),
    Bps {
        inflation_rewards: u16,
        block_revenue: u16,
    },
}

/// SIMD-0232 collectors, only present on V4 accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoteCollectors {
    pub inflation_rewards: Pubkey,
    pub block_revenue: Pubkey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteAccountState {
    pub version: VoteStateVersion,
    pub node_pubkey: Pubkey,
    pub authorized_withdrawer: Pubkey,
    pub commission: VoteCommission,
    pub collectors: Option<VoteCollectors>,
    /// Whether a BLS public key is registered (V4 only; always false before).
    pub has_bls_key: bool,
}

impl VoteAccountState {
    pub fn parse(data: &[u8]) -> Result<Self, MsigError> {
        let mut r = SafeReader::new(data);
        let version = match r.read_u32_le()? {
            0 => {
                return Err(MsigError::Transaction(
                    "vote account uses the legacy V0_23_5 layout, which msig does not support"
                        .into(),
                ))
            }
            1 => VoteStateVersion::V1_14_11,
            2 => VoteStateVersion::V3,
            3 => VoteStateVersion::V4,
            other => {
                return Err(MsigError::Transaction(format!(
                    "unknown vote account version {other}; msig supports V1_14_11, V3 and V4"
                )))
            }
        };

        let node_pubkey = r.read_solana_pubkey()?;
        let authorized_withdrawer = r.read_solana_pubkey()?;

        let (commission, collectors, has_bls_key) = match version {
            VoteStateVersion::V1_14_11 | VoteStateVersion::V3 => {
                (VoteCommission::Percent(r.read_u8()?), None, false)
            }
            VoteStateVersion::V4 => {
                let inflation_rewards = r.read_solana_pubkey()?;
                let block_revenue = r.read_solana_pubkey()?;
                let inflation_bps = r.read_u16_le()?;
                let block_bps = r.read_u16_le()?;
                let _pending_delegator_rewards = r.read_u64_le()?;
                let bls = r.read_option(|rr| rr.skip(BLS_PUBKEY_COMPRESSED_LEN))?;
                (
                    VoteCommission::Bps {
                        inflation_rewards: inflation_bps,
                        block_revenue: block_bps,
                    },
                    Some(VoteCollectors {
                        inflation_rewards,
                        block_revenue,
                    }),
                    bls.is_some(),
                )
            }
        };

        Ok(Self {
            version,
            node_pubkey,
            authorized_withdrawer,
            commission,
            collectors,
            has_bls_key,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::ParseError;

    /// First 193 bytes of a real V4 vote account captured from
    /// solana-test-validator 4.3.
    pub(crate) const V4_ACCOUNT_PREFIX_HEX: &str = "03000000d05f8abd03b59764f3c4d964a67f0987336e94646be9901890017cd9fa0fcb4a9f9401d408389f43f0aa8bd41aa555bc77575d793f6d5b51062d45af2b7b6f79f4e04af402fc8fd0b9baff4be886724bbd85b7a275431efe1e6e9a59406024239f9401d408389f43f0aa8bd41aa555bc77575d793f6d5b51062d45af2b7b6f792c011027000000000000000001b46aeaea3f9ce88ad316bd6da7e94bb6ebb2445592895d65569404d6587515e3baf09ac287a0772d4b07e677fda5558c";

    pub(crate) fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|e| panic!("{e}")))
            .collect()
    }

    fn pk(s: &str) -> Pubkey {
        s.parse().unwrap_or_else(|e| panic!("{e:?}"))
    }

    pub(crate) fn synthetic(
        version: u32,
        node: Pubkey,
        withdrawer: Pubkey,
        tail: &[u8],
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&version.to_le_bytes());
        data.extend_from_slice(node.as_ref());
        data.extend_from_slice(withdrawer.as_ref());
        data.extend_from_slice(tail);
        data
    }

    #[test]
    fn parses_real_v4_prefix() {
        let state =
            VoteAccountState::parse(&hex(V4_ACCOUNT_PREFIX_HEX)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(state.version, VoteStateVersion::V4);
        assert_eq!(
            state.node_pubkey,
            pk("F2QJLZ9tBKT1vAs8pRs6WupW5As9uuzpknk3Au7dpZ9b")
        );
        assert_eq!(
            state.authorized_withdrawer,
            pk("BjvitWuqmZCcpmVKnoeWWvtCEPevfL4dqwqEaZ4RsKQC")
        );
        assert_eq!(
            state.collectors,
            Some(VoteCollectors {
                inflation_rewards: pk("HUtqwyyi7JHXMwv3GbKZMgURZBRB8cRgfpwgsrQuamuc"),
                block_revenue: pk("BjvitWuqmZCcpmVKnoeWWvtCEPevfL4dqwqEaZ4RsKQC"),
            })
        );
        assert_eq!(
            state.commission,
            VoteCommission::Bps {
                inflation_rewards: 300,
                block_revenue: 10_000,
            }
        );
        assert!(state.has_bls_key);
    }

    #[test]
    fn parses_v4_without_bls_key() {
        let mut data = hex(V4_ACCOUNT_PREFIX_HEX);
        data.truncate(145);
        data[144] = 0;
        let state = VoteAccountState::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert!(!state.has_bls_key);
    }

    #[test]
    fn rejects_v4_with_truncated_bls_key() {
        let mut data = hex(V4_ACCOUNT_PREFIX_HEX);
        data.truncate(150);
        assert!(matches!(
            VoteAccountState::parse(&data),
            Err(MsigError::Parse(ParseError::Eof { .. }))
        ));
    }

    #[test]
    fn parses_v1_and_v3_commission_percent() {
        let node = Pubkey::new_from_array([1u8; 32]);
        let withdrawer = Pubkey::new_from_array([2u8; 32]);
        for (version, expected) in [
            (1u32, VoteStateVersion::V1_14_11),
            (2, VoteStateVersion::V3),
        ] {
            // Byte 144 of a V1/V3 account is vote history; set it to 1 to prove
            // it is never read as a BLS tag.
            let mut tail = vec![0u8; 200];
            tail[0] = 7;
            tail[144 - 68] = 1;
            let state = VoteAccountState::parse(&synthetic(version, node, withdrawer, &tail))
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(state.version, expected);
            assert_eq!(state.node_pubkey, node);
            assert_eq!(state.authorized_withdrawer, withdrawer);
            assert_eq!(state.commission, VoteCommission::Percent(7));
            assert_eq!(state.collectors, None);
            assert!(!state.has_bls_key);
        }
    }

    #[test]
    fn rejects_v0_23_5() {
        let data = synthetic(0, Pubkey::default(), Pubkey::default(), &[0u8; 100]);
        match VoteAccountState::parse(&data) {
            Err(MsigError::Transaction(msg)) => assert!(msg.contains("V0_23_5")),
            other => panic!("expected V0_23_5 rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_version() {
        let data = synthetic(4, Pubkey::default(), Pubkey::default(), &[0u8; 200]);
        assert!(matches!(
            VoteAccountState::parse(&data),
            Err(MsigError::Transaction(_))
        ));
    }

    #[test]
    fn rejects_short_data() {
        assert!(matches!(
            VoteAccountState::parse(&[2, 0, 0]),
            Err(MsigError::Parse(_))
        ));
        let data = synthetic(2, Pubkey::default(), Pubkey::default(), &[]);
        assert!(matches!(
            VoteAccountState::parse(&data),
            Err(MsigError::Parse(ParseError::Eof { offset: 68, .. }))
        ));
    }
}
