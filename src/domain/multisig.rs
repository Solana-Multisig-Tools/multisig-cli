use serde::Serialize;
use solana_pubkey::Pubkey;

use super::serde_pubkey;

/// Bit-packed member permissions for Squads v4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(pub u8);

impl Serialize for Permissions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("Permissions", 2)?;
        s.serialize_field("mask", &self.0)?;
        s.serialize_field("labels", &self.labels())?;
        s.end()
    }
}

impl Permissions {
    pub fn can_initiate(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn can_vote(self) -> bool {
        self.0 & 2 != 0
    }
    pub fn can_execute(self) -> bool {
        self.0 & 4 != 0
    }

    pub fn labels(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.can_initiate() {
            out.push("Initiate");
        }
        if self.can_vote() {
            out.push("Vote");
        }
        if self.can_execute() {
            out.push("Execute");
        }
        out
    }
}

/// A multisig member: pubkey + permissions.
#[derive(Debug, Clone, Serialize)]
pub struct Member {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub key: Pubkey,
    pub permissions: Permissions,
}

/// Full multisig state as a domain object.
#[derive(Debug, Clone, Serialize)]
pub struct MultisigInfo {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub address: Pubkey,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub create_key: Pubkey,
    #[serde(serialize_with = "serialize_config_authority")]
    pub config_authority: Pubkey,
    pub threshold: u16,
    pub time_lock: u32,
    pub transaction_index: u64,
    #[allow(dead_code)]
    #[serde(skip)]
    pub stale_transaction_index: u64,
    #[serde(serialize_with = "serde_pubkey::option::serialize")]
    pub rent_collector: Option<Pubkey>,
    #[allow(dead_code)]
    #[serde(skip)]
    pub bump: u8,
    pub members: Vec<Member>,
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub vault_address: Pubkey,
    pub vault_balance_lamports: u64,
}

/// Serialize `config_authority` as `null` when it equals the system program (zero address).
fn serialize_config_authority<S: serde::Serializer>(
    pubkey: &Pubkey,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if *pubkey == Pubkey::default() {
        serializer.serialize_none()
    } else {
        serializer.serialize_some(&pubkey.to_string())
    }
}

/// Balance info for a single token.
#[derive(Debug, Clone, Serialize)]
pub struct TokenBalance {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub mint: Pubkey,
    pub symbol: Option<String>,
    pub raw_amount: u64,
    pub ui_amount: String,
}

/// All balances for a vault.
#[derive(Debug, Clone, Serialize)]
pub struct VaultBalances {
    #[serde(serialize_with = "serde_pubkey::serialize")]
    pub vault_address: Pubkey,
    pub sol_lamports: u64,
    pub token_balances: Vec<TokenBalance>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_bitfield() {
        let p = Permissions(0b101);
        assert!(p.can_initiate());
        assert!(!p.can_vote());
        assert!(p.can_execute());
        assert_eq!(p.labels(), vec!["Initiate", "Execute"]);
    }

    #[test]
    fn permissions_none() {
        let p = Permissions(0);
        assert!(!p.can_initiate());
        assert!(!p.can_vote());
        assert!(!p.can_execute());
        assert!(p.labels().is_empty());
    }

    #[test]
    fn permissions_all() {
        assert_eq!(Permissions(7).labels(), vec!["Initiate", "Vote", "Execute"]);
    }
}
