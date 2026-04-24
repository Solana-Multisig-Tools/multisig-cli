use serde::Serialize;
use solana_pubkey::Pubkey;

use super::discriminators;
use super::reader::SafeReader;
use crate::error::ParseError;

#[derive(Debug, Clone, Serialize)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub account_indexes: Vec<u8>,
    pub data: Vec<u8>,
}

impl CompiledInstruction {
    fn parse(r: &mut SafeReader<'_>) -> Result<Self, ParseError> {
        let program_id_index = r.read_u8()?;
        let account_indexes = r.read_vec(256, |rr| rr.read_u8())?;
        let data = r.read_vec(65535, |rr| rr.read_u8())?;
        Ok(Self {
            program_id_index,
            account_indexes,
            data,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AddressTableLookup {
    pub account_key: [u8; 32],
    pub writable_indexes: Vec<u8>,
    pub readonly_indexes: Vec<u8>,
}

impl AddressTableLookup {
    fn parse(r: &mut SafeReader<'_>) -> Result<Self, ParseError> {
        let account_key = r.read_pubkey()?;
        let writable_indexes = r.read_vec(256, |rr| rr.read_u8())?;
        let readonly_indexes = r.read_vec(256, |rr| rr.read_u8())?;
        Ok(Self {
            account_key,
            writable_indexes,
            readonly_indexes,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionMessage {
    pub num_signers: u8,
    pub num_writable_signers: u8,
    pub num_writable_non_signers: u8,
    pub account_keys: Vec<[u8; 32]>,
    pub instructions: Vec<CompiledInstruction>,
    pub address_table_lookups: Vec<AddressTableLookup>,
}

impl TransactionMessage {
    fn parse(r: &mut SafeReader<'_>) -> Result<Self, ParseError> {
        let num_signers = r.read_u8()?;
        let num_writable_signers = r.read_u8()?;
        let num_writable_non_signers = r.read_u8()?;
        let account_keys = r.read_vec(256, |rr| rr.read_pubkey())?;
        let instructions = r.read_vec(256, |rr| CompiledInstruction::parse(rr))?;
        let address_table_lookups = r.read_vec(256, |rr| AddressTableLookup::parse(rr))?;
        Ok(Self {
            num_signers,
            num_writable_signers,
            num_writable_non_signers,
            account_keys,
            instructions,
            address_table_lookups,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultTransactionAccount {
    pub multisig: Pubkey,
    pub creator: Pubkey,
    pub index: u64,
    pub bump: u8,
    pub vault_index: u8,
    pub vault_bump: u8,
    pub ephemeral_signer_bumps: Vec<u8>,
    pub message: TransactionMessage,
}

impl VaultTransactionAccount {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = SafeReader::new(data);
        r.read_discriminator(&discriminators::VAULT_TRANSACTION)?;

        let multisig = r.read_solana_pubkey()?;
        let creator = r.read_solana_pubkey()?;
        let index = r.read_u64_le()?;
        let bump = r.read_u8()?;
        let vault_index = r.read_u8()?;
        let vault_bump = r.read_u8()?;
        let ephemeral_signer_bumps = r.read_vec(256, |rr| rr.read_u8())?;
        let message = TransactionMessage::parse(&mut r)?;

        Ok(VaultTransactionAccount {
            multisig,
            creator,
            index,
            bump,
            vault_index,
            vault_bump,
            ephemeral_signer_bumps,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_vault_tx_data() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&discriminators::VAULT_TRANSACTION);
        buf.extend_from_slice(&[1u8; 32]); // multisig
        buf.extend_from_slice(&[2u8; 32]); // creator
        buf.extend_from_slice(&7u64.to_le_bytes());
        buf.push(254);
        buf.push(0);
        buf.push(253);
        buf.extend_from_slice(&0u32.to_le_bytes()); // ephemeral
        buf.push(1);
        buf.push(1);
        buf.push(0); // msg header
        buf.extend_from_slice(&1u32.to_le_bytes()); // 1 key
        buf.extend_from_slice(&[4u8; 32]);
        buf.extend_from_slice(&1u32.to_le_bytes()); // 1 ix
        buf.push(0);
        buf.extend_from_slice(&0u32.to_le_bytes()); // 0 account_indexes
        buf.extend_from_slice(&2u32.to_le_bytes()); // 2 bytes data
        buf.push(0xAA);
        buf.push(0xBB);
        buf.extend_from_slice(&0u32.to_le_bytes()); // 0 lookups
        buf
    }

    #[test]
    fn parse_vault_tx() {
        let data = build_vault_tx_data();
        let vtx = VaultTransactionAccount::parse(&data).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(vtx.index, 7);
        assert_eq!(vtx.vault_index, 0);
        assert_eq!(vtx.message.instructions.len(), 1);
        assert_eq!(vtx.message.instructions[0].data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn vault_tx_wrong_discriminator() {
        let mut data = build_vault_tx_data();
        data[0] = 0xFF;
        assert!(matches!(
            VaultTransactionAccount::parse(&data),
            Err(ParseError::InvalidDiscriminator { .. })
        ));
    }
}
