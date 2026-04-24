use crate::error::ParseError;

pub struct SafeReader<'a> {
    data: &'a [u8],
    pos: usize,
}

#[allow(dead_code)]
impl<'a> SafeReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.remaining() < n {
            return Err(ParseError::Eof {
                offset: self.pos,
                need: n,
                have: self.remaining(),
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), ParseError> {
        if self.remaining() < n {
            return Err(ParseError::Eof {
                offset: self.pos,
                need: n,
                have: self.remaining(),
            });
        }
        self.pos += n;
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8, ParseError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, ParseError> {
        let b = self.read_u8()?;
        match b {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ParseError::InvalidTag {
                tag: b,
                type_name: "bool",
            }),
        }
    }

    pub fn read_u16_le(&mut self) -> Result<u16, ParseError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32_le(&mut self) -> Result<u32, ParseError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_u64_le(&mut self) -> Result<u64, ParseError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn read_i64_le(&mut self) -> Result<i64, ParseError> {
        let bytes = self.read_bytes(8)?;
        Ok(i64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn read_pubkey(&mut self) -> Result<[u8; 32], ParseError> {
        let bytes = self.read_bytes(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    pub fn read_pubkey_base58(&mut self) -> Result<String, ParseError> {
        let pk = self.read_pubkey()?;
        Ok(bs58::encode(pk).into_string())
    }

    pub fn read_solana_pubkey(&mut self) -> Result<solana_pubkey::Pubkey, ParseError> {
        let pk = self.read_pubkey()?;
        Ok(solana_pubkey::Pubkey::from(pk))
    }

    pub fn read_option<T, F>(&mut self, f: F) -> Result<Option<T>, ParseError>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError>,
    {
        let tag = self.read_u8()?;
        match tag {
            0 => Ok(None),
            1 => Ok(Some(f(self)?)),
            _ => Err(ParseError::InvalidTag {
                tag,
                type_name: "Option",
            }),
        }
    }

    pub fn read_vec<T, F>(&mut self, cap: u32, f: F) -> Result<Vec<T>, ParseError>
    where
        F: Fn(&mut Self) -> Result<T, ParseError>,
    {
        let len = self.read_u32_le()?;
        if len > cap {
            return Err(ParseError::VecTooLong { len, cap });
        }
        let mut out = Vec::with_capacity(len as usize);
        for _ in 0..len {
            out.push(f(self)?);
        }
        Ok(out)
    }

    pub fn read_discriminator(&mut self, expected: &[u8; 8]) -> Result<(), ParseError> {
        let bytes = self.read_bytes(8)?;
        let mut got = [0u8; 8];
        got.copy_from_slice(bytes);
        if &got != expected {
            return Err(ParseError::InvalidDiscriminator {
                expected: *expected,
                got,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_reader_eof() {
        let data = [0u8; 3];
        let mut r = SafeReader::new(&data);
        assert!(r.read_u32_le().is_err());
    }

    #[test]
    fn test_safe_reader_read_advances() {
        let data = [1, 2, 3, 4, 5];
        let mut r = SafeReader::new(&data);
        assert_eq!(r.read_u8().ok(), Some(1));
        assert_eq!(r.position(), 1);
        assert_eq!(r.remaining(), 4);
    }

    #[test]
    fn test_safe_reader_vec_too_long() {
        let mut data = Vec::new();
        data.extend_from_slice(&100u32.to_le_bytes());
        let mut r = SafeReader::new(&data);
        let result: Result<Vec<u8>, _> = r.read_vec(10, |rr| rr.read_u8());
        assert!(matches!(
            result,
            Err(ParseError::VecTooLong { len: 100, cap: 10 })
        ));
    }

    #[test]
    fn test_safe_reader_discriminator_ok() {
        let expected: [u8; 8] = [0xe0, 0x74, 0x79, 0xba, 0x44, 0xa1, 0x4f, 0xec];
        let mut r = SafeReader::new(&expected);
        assert!(r.read_discriminator(&expected).is_ok());
    }

    #[test]
    fn test_safe_reader_option() {
        let mut r = SafeReader::new(&[0]);
        assert_eq!(r.read_option(|rr| rr.read_u8()).ok(), Some(None));

        let mut r = SafeReader::new(&[1, 42]);
        assert_eq!(r.read_option(|rr| rr.read_u8()).ok(), Some(Some(42)));
    }
}
