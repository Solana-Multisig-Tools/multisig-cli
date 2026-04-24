//! Serde helpers for serializing `solana_pubkey::Pubkey` as base58 strings.

use serde::{self, Serializer};
use solana_pubkey::Pubkey;

pub fn serialize<S>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&pubkey.to_string())
}

/// For Option<Pubkey>
pub mod option {
    use super::*;

    pub fn serialize<S>(pubkey: &Option<Pubkey>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match pubkey {
            Some(pk) => serializer.serialize_some(&pk.to_string()),
            None => serializer.serialize_none(),
        }
    }
}

/// For Vec<Pubkey>
pub mod vec {
    use super::*;

    pub fn serialize<S>(pubkeys: &[Pubkey], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(pubkeys.len()))?;
        for pk in pubkeys {
            seq.serialize_element(&pk.to_string())?;
        }
        seq.end()
    }
}
