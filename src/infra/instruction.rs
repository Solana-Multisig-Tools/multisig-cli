use solana_pubkey::Pubkey;

/// Minimal Solana account metadata used for legacy message compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn new(pubkey: Pubkey, is_signer: bool) -> Self {
        Self {
            pubkey,
            is_signer,
            is_writable: true,
        }
    }

    pub fn new_readonly(pubkey: Pubkey, is_signer: bool) -> Self {
        Self {
            pubkey,
            is_signer,
            is_writable: false,
        }
    }
}

/// Minimal Solana instruction representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

#[cfg(feature = "instruction-builder")]
impl From<AccountMeta> for solana_instruction::AccountMeta {
    fn from(m: AccountMeta) -> Self {
        Self {
            pubkey: m.pubkey,
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        }
    }
}

#[cfg(feature = "instruction-builder")]
impl From<solana_instruction::AccountMeta> for AccountMeta {
    fn from(m: solana_instruction::AccountMeta) -> Self {
        Self {
            pubkey: m.pubkey,
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        }
    }
}

#[cfg(feature = "instruction-builder")]
impl From<Instruction> for solana_instruction::Instruction {
    fn from(ix: Instruction) -> Self {
        Self {
            program_id: ix.program_id,
            accounts: ix.accounts.into_iter().map(Into::into).collect(),
            data: ix.data,
        }
    }
}
