use solana_pubkey::Pubkey;

use crate::error::MsigError;

/// Borsh-encode an `Option<&str>` into `buf`.
///
/// Layout: 1-byte tag (`0x00` for `None`, `0x01` for `Some`), followed by a
/// 4-byte little-endian length prefix and the UTF-8 bytes when `Some`.
/// Returns an error only when the byte length doesn't fit in `u32`, which is
/// the borsh wire-format limit. The caller passes the `&str` so we never
/// re-encode the bytes.
pub(crate) fn borsh_write_option_string(
    buf: &mut Vec<u8>,
    value: Option<&str>,
) -> Result<(), MsigError> {
    match value {
        None => {
            buf.push(0x00);
            Ok(())
        }
        Some(s) => {
            let len = u32::try_from(s.len()).map_err(|_| {
                MsigError::Transaction(format!(
                    "memo is {} bytes; max supported is u32::MAX",
                    s.len()
                ))
            })?;
            buf.push(0x01);
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
}

/// Borsh-decode a borsh `Option<String>` from a trailing slice.
///
/// Returns `(value, consumed_bytes)`. Used by tests to verify that builders
/// emit a memo encoding the parser will round-trip; not used at runtime
/// because the on-chain memo is write-only from the client's perspective.
#[cfg(test)]
pub(crate) fn borsh_read_option_string(buf: &[u8]) -> Result<(Option<String>, usize), MsigError> {
    let tag = buf
        .first()
        .copied()
        .ok_or_else(|| MsigError::Transaction("empty memo encoding".into()))?;
    match tag {
        0x00 => Ok((None, 1)),
        0x01 => {
            let len_bytes: [u8; 4] = buf
                .get(1..5)
                .ok_or_else(|| MsigError::Transaction("memo length prefix truncated".into()))?
                .try_into()
                .map_err(|_| MsigError::Transaction("memo length slice not 4 bytes".into()))?;
            let len = u32::from_le_bytes(len_bytes) as usize;
            let body = buf
                .get(5..5 + len)
                .ok_or_else(|| MsigError::Transaction("memo body truncated".into()))?;
            let s = std::str::from_utf8(body)
                .map_err(|e| MsigError::Transaction(format!("memo not valid UTF-8: {e}")))?
                .to_string();
            Ok((Some(s), 5 + len))
        }
        other => Err(MsigError::Transaction(format!(
            "invalid Option tag {other} for memo"
        ))),
    }
}

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

#[cfg(feature = "instruction-builder")]
impl From<solana_instruction::Instruction> for Instruction {
    fn from(ix: solana_instruction::Instruction) -> Self {
        Self {
            program_id: ix.program_id,
            accounts: ix.accounts.into_iter().map(Into::into).collect(),
            data: ix.data,
        }
    }
}

/// Test helper: assert that `data_some` is byte-identical to `data_none`
/// except that the trailing `None`-tag byte is replaced with a full
/// borsh `Some(memo)` encoding. Catches builders that forget the memo
/// argument, place it at the wrong offset, or double-encode it.
#[cfg(test)]
pub(crate) fn assert_memo_replaces_none_tail(data_none: &[u8], data_some: &[u8], memo: &str) {
    assert!(
        matches!(data_none.last(), Some(0x00)),
        "data_none must end with the None tag (0x00)"
    );
    let prefix_without_tag = &data_none[..data_none.len() - 1];
    assert_eq!(
        &data_some[..prefix_without_tag.len()],
        prefix_without_tag,
        "non-memo prefix must be identical between None and Some encodings"
    );
    let (decoded, consumed) = borsh_read_option_string(&data_some[prefix_without_tag.len()..])
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(decoded.as_deref(), Some(memo));
    assert_eq!(
        consumed,
        data_some.len() - prefix_without_tag.len(),
        "memo encoding must consume exactly the trailing bytes"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks in the borsh wire-format for `Option<String>` so memo encoding
    /// can't silently drift away from what the v4 program expects.
    #[test]
    fn borsh_option_string_none() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(buf, vec![0x00]);
    }

    #[test]
    fn borsh_option_string_empty() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, Some("")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(buf, vec![0x01, 0, 0, 0, 0]);
    }

    #[test]
    fn borsh_option_string_ascii() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, Some("hi")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(buf, vec![0x01, 2, 0, 0, 0, b'h', b'i']);
    }

    #[test]
    fn borsh_option_string_utf8_byte_length() {
        // "héllo" is 6 bytes in UTF-8 (é is two bytes), not 5 codepoints.
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, Some("héllo")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(buf[0], 0x01);
        assert_eq!(&buf[1..5], &6u32.to_le_bytes());
        assert_eq!(&buf[5..], "héllo".as_bytes());
    }

    #[test]
    fn borsh_option_string_round_trip_none() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, None).unwrap_or_else(|e| panic!("{e}"));
        let (decoded, consumed) =
            borsh_read_option_string(&buf).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(decoded, None);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn borsh_option_string_round_trip_some_ascii() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, Some("payment for invoice 42"))
            .unwrap_or_else(|e| panic!("{e}"));
        let (decoded, consumed) =
            borsh_read_option_string(&buf).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(decoded.as_deref(), Some("payment for invoice 42"));
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn borsh_option_string_round_trip_some_utf8() {
        let mut buf = Vec::new();
        borsh_write_option_string(&mut buf, Some("héllo 🦀 wörld"))
            .unwrap_or_else(|e| panic!("{e}"));
        let (decoded, consumed) =
            borsh_read_option_string(&buf).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(decoded.as_deref(), Some("héllo 🦀 wörld"));
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn borsh_option_string_decode_rejects_invalid_tag() {
        let buf = [0x02u8];
        assert!(borsh_read_option_string(&buf).is_err());
    }

    #[test]
    fn borsh_option_string_decode_rejects_truncated_body() {
        let buf = [0x01u8, 5, 0, 0, 0, b'h', b'i'];
        assert!(borsh_read_option_string(&buf).is_err());
    }
}
