use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::rpc::AccountData;

pub const ADDRESS_LOOKUP_TABLE_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("AddressLookupTab1e1111111111111111111111111");

const LOOKUP_TABLE_META_SIZE: usize = 56;
const LOOKUP_TABLE_STATE_TAG: [u8; 4] = [1, 0, 0, 0];

pub fn parse_addresses(
    account_key: &Pubkey,
    account: &AccountData,
) -> Result<Vec<Pubkey>, MsigError> {
    if account.owner != ADDRESS_LOOKUP_TABLE_PROGRAM.to_string() {
        return Err(MsigError::Transaction(format!(
            "address lookup table {} is owned by {}, expected {}",
            account_key, account.owner, ADDRESS_LOOKUP_TABLE_PROGRAM
        )));
    }
    parse_addresses_from_data(account_key, &account.data)
}

pub fn parse_addresses_from_data(
    account_key: &Pubkey,
    data: &[u8],
) -> Result<Vec<Pubkey>, MsigError> {
    if data.len() < LOOKUP_TABLE_META_SIZE {
        return Err(MsigError::Transaction(format!(
            "address lookup table {} is too small: {} bytes",
            account_key,
            data.len()
        )));
    }
    if data.get(..4) != Some(&LOOKUP_TABLE_STATE_TAG) {
        return Err(MsigError::Transaction(format!(
            "address lookup table {} is not initialized",
            account_key
        )));
    }

    let address_bytes = &data[LOOKUP_TABLE_META_SIZE..];
    if !address_bytes.len().is_multiple_of(32) {
        return Err(MsigError::Transaction(format!(
            "address lookup table {} has malformed address data length {}",
            account_key,
            address_bytes.len()
        )));
    }

    let mut addresses = Vec::with_capacity(address_bytes.len() / 32);
    for chunk in address_bytes.chunks_exact(32) {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(chunk);
        addresses.push(Pubkey::from(bytes));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_data(addresses: &[Pubkey]) -> Vec<u8> {
        let mut data = vec![0u8; LOOKUP_TABLE_META_SIZE];
        data[..4].copy_from_slice(&LOOKUP_TABLE_STATE_TAG);
        for address in addresses {
            data.extend_from_slice(address.as_ref());
        }
        data
    }

    #[test]
    fn parses_lookup_table_addresses_after_meta() {
        let table = Pubkey::from([9u8; 32]);
        let a = Pubkey::from([1u8; 32]);
        let b = Pubkey::from([2u8; 32]);

        let addresses = parse_addresses_from_data(&table, &table_data(&[a, b]))
            .unwrap_or_else(|err| panic!("{err}"));

        assert_eq!(addresses, vec![a, b]);
    }

    #[test]
    fn rejects_uninitialized_lookup_table() {
        let table = Pubkey::from([9u8; 32]);
        let data = vec![0u8; LOOKUP_TABLE_META_SIZE];

        assert!(matches!(
            parse_addresses_from_data(&table, &data),
            Err(MsigError::Transaction(_))
        ));
    }
}
