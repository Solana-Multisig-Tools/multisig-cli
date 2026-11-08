use crate::error::MsigError;
use crate::infra::signer::Signer;
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

pub struct KeypairSigner {
    pubkey: solana_pubkey::Pubkey,
    secret: Zeroizing<Vec<u8>>,
}

impl KeypairSigner {
    pub fn from_file(path: &str) -> Result<Self, MsigError> {
        let bytes = read_keypair_bytes(path)?;
        let pubkey =
            solana_pubkey::Pubkey::from(<[u8; 32]>::try_from(&bytes[32..64]).map_err(|_| {
                MsigError::Signing("failed to extract public key bytes".to_string())
            })?);
        let mut secret = Zeroizing::new(Vec::with_capacity(32));
        secret.extend_from_slice(&bytes[..32]);

        Ok(Self { pubkey, secret })
    }
}

/// Read and validate a Solana keypair JSON file into zeroized raw bytes
/// (32-byte secret followed by the 32-byte public key). Refuses files readable
/// by group or others, and files whose public key does not match the secret.
pub fn read_keypair_bytes(path: &str) -> Result<Zeroizing<[u8; 64]>, MsigError> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(MsigError::Signing(format!(
            "keypair file not found: {path}"
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = p
            .metadata()
            .map_err(|e| MsigError::Signing(format!("cannot stat keypair file {path}: {e}")))?
            .mode();
        if mode & 0o077 != 0 {
            return Err(MsigError::Signing(format!(
                "keypair file {path} has insecure permissions {:o}. Run: chmod 600 {path}",
                mode & 0o777
            )));
        }
    }

    let contents = Zeroizing::new(
        std::fs::read_to_string(p)
            .map_err(|e| MsigError::Signing(format!("failed to read keypair file {path}: {e}")))?,
    );

    let bytes: Zeroizing<Vec<u8>> =
        Zeroizing::new(serde_json::from_str(&contents).map_err(|e| {
            MsigError::Signing(format!("failed to parse keypair JSON from {path}: {e}"))
        })?);

    if bytes.len() != 64 {
        return Err(MsigError::Signing(format!(
            "invalid keypair file {path}: expected 64 bytes, got {}",
            bytes.len()
        )));
    }

    let secret_arr = Zeroizing::new(
        <[u8; 32]>::try_from(&bytes[..32])
            .map_err(|_| MsigError::Signing("failed to extract secret key bytes".to_string()))?,
    );
    let stored_pubkey = <[u8; 32]>::try_from(&bytes[32..64])
        .map_err(|_| MsigError::Signing("failed to extract public key bytes".to_string()))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_arr);
    let derived_pubkey = signing_key.verifying_key().to_bytes();
    if derived_pubkey != stored_pubkey {
        return Err(MsigError::Signing(format!(
            "invalid keypair file {path}: public key does not match secret key"
        )));
    }

    let mut out = Zeroizing::new([0u8; 64]);
    out.copy_from_slice(&bytes);
    Ok(out)
}

impl Signer for KeypairSigner {
    fn pubkey(&self) -> solana_pubkey::Pubkey {
        self.pubkey
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, MsigError> {
        let mut secret_bytes = Zeroizing::new(
            <[u8; 32]>::try_from(self.secret.as_slice())
                .map_err(|_| MsigError::Signing("invalid secret key length".to_string()))?,
        );

        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
        secret_bytes.zeroize();
        let signature: ed25519_dalek::Signature =
            ed25519_dalek::Signer::sign(&signing_key, message);

        Ok(signature.to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_file_not_found() {
        let result = KeypairSigner::from_file("/nonexistent/path.json");
        assert!(result.is_err());
    }

    fn write_temp_keypair(bytes: &[u8]) -> std::path::PathBuf {
        // Tests run in parallel and the clock can repeat, so a counter keeps
        // each file name unique within the process.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = format!(
            "msig-keypair-test-{}-{}-{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(&path, serde_json::to_string(bytes).unwrap_or_default())
            .unwrap_or_else(|e| panic!("{e}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .unwrap_or_else(|e| panic!("{e}"));
        }
        path
    }

    #[test]
    fn from_file_rejects_mismatched_public_key() {
        let secret = [9u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&secret);
        bytes.extend_from_slice(signing_key.verifying_key().as_bytes());
        bytes[63] ^= 1;

        let path = write_temp_keypair(&bytes);
        let result = KeypairSigner::from_file(&path.to_string_lossy());
        let _ = std::fs::remove_file(path);

        assert!(matches!(result, Err(MsigError::Signing(_))));
    }

    #[test]
    fn read_keypair_bytes_matches_signer() {
        let secret = [7u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&secret);
        bytes.extend_from_slice(signing_key.verifying_key().as_bytes());

        let path = write_temp_keypair(&bytes);
        let path_str = path.to_string_lossy().into_owned();
        let raw = read_keypair_bytes(&path_str);
        let signer = KeypairSigner::from_file(&path_str);
        let refused = crate::infra::signer::load_extra_signer_keypair("usb://ledger?key=0");
        let _ = std::fs::remove_file(path);

        let raw = raw.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(&raw[..], &bytes[..]);
        let signer = signer.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(signer.pubkey().to_bytes()[..], bytes[32..]);
        assert!(matches!(refused, Err(MsigError::Usage(_))));
    }
}
