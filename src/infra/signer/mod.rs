pub mod keypair;
#[cfg(feature = "ledger")]
pub mod ledger;
pub mod offline;

use crate::error::MsigError;

pub trait Signer: Send + Sync {
    fn pubkey(&self) -> solana_pubkey::Pubkey;
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, MsigError>;

    /// True for signers where signing triggers a separate trusted-device review.
    fn requires_device_review(&self) -> bool {
        false
    }
}

/// Resolve a signer from global options and config.
/// Priority: --ledger > --keypair > config keypair
pub fn resolve_signer(
    ledger_flag: Option<&str>,
    keypair_flag: Option<&str>,
    config_keypair: Option<&str>,
) -> Result<Box<dyn Signer>, MsigError> {
    if let Some(ledger_val) = ledger_flag {
        #[cfg(feature = "ledger")]
        {
            let signer = ledger::LedgerSigner::from_flag(ledger_val)?;
            return Ok(Box::new(signer));
        }
        #[cfg(not(feature = "ledger"))]
        {
            let _ = ledger_val;
            return Err(MsigError::Config(
                "Ledger support requires the 'ledger' feature. Rebuild with `cargo build --features ledger`.".to_string(),
            ));
        }
    }

    if let Some(path) = keypair_flag {
        return resolve_keypair_value(path);
    }

    if let Some(val) = config_keypair {
        return resolve_keypair_value(val);
    }

    Err(MsigError::Config(
        "no signer available. Use --keypair <FILE>, --ledger, or set 'keypair' in config."
            .to_string(),
    ))
}

/// Load a raw keypair file for an additional transaction signer (for example
/// the new validator identity at `proposal execute --extra-signer`).
///
/// Only keypair files are accepted: the transaction pipeline signs extra
/// signers with raw key material, so a Ledger cannot be used here.
pub fn load_extra_signer_keypair(value: &str) -> Result<zeroize::Zeroizing<[u8; 64]>, MsigError> {
    if value.starts_with("usb://") {
        return Err(MsigError::Usage(format!(
            "extra signer '{value}' is a hardware wallet URI; extra signers must be keypair \
             files. A Ledger cannot be an extra signer in this version."
        )));
    }
    keypair::read_keypair_bytes(&expand_home(value))
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = crate::infra::config::file::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    value.to_string()
}

fn resolve_keypair_value(value: &str) -> Result<Box<dyn Signer>, MsigError> {
    if value.starts_with("usb://ledger") {
        #[cfg(feature = "ledger")]
        {
            let (account, change) = ledger::parse_ledger_uri(value)?;
            return Ok(Box::new(ledger::LedgerSigner::new(account, change)));
        }
        #[cfg(not(feature = "ledger"))]
        {
            return Err(MsigError::Config(
                "keypair value uses 'usb://ledger' but the 'ledger' feature is not enabled. Rebuild with `cargo build --features ledger`.".to_string(),
            ));
        }
    }

    let signer = keypair::KeypairSigner::from_file(&expand_home(value))?;
    Ok(Box::new(signer))
}
