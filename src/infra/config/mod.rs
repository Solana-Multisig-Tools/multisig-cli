pub mod file;
pub mod labels;
pub mod tokens;

use crate::error::MsigError;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct GlobalFlags {
    pub cluster: Option<String>,
    pub keypair: Option<String>,
    pub multisig: Option<String>,
    pub commitment: Option<String>,
    pub vault_index: Option<u8>,
    pub priority_fee: Option<u64>,
    pub program_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cluster: String,
    pub keypair: Option<String>,
    pub multisig: Option<String>,
    pub commitment: String,
    pub vault_index: u8,
    pub priority_fee: u64,
    pub labels: HashMap<String, String>,
    pub tokens: HashMap<String, String>,
    pub program_id: solana_pubkey::Pubkey,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cluster: "https://api.mainnet-beta.solana.com".to_string(),
            keypair: None,
            multisig: None,
            commitment: "confirmed".to_string(),
            vault_index: 0,
            priority_fee: 0,
            labels: HashMap::new(),
            tokens: HashMap::new(),
            program_id: crate::infra::pda::PROGRAM_ID,
        }
    }
}

pub fn load_config(global_flags: &GlobalFlags) -> Result<Config, MsigError> {
    let mut config = Config::default();

    if let Some(path) = file::user_config_path() {
        if let Some(user_cfg) = file::load_config_file(&path)? {
            apply_file_config(&mut config, &user_cfg)?;
        }
    }

    if let Some(profile_name) = file::read_active_profile() {
        if profile_name != "default" {
            file::apply_profile(&mut config, &profile_name)?;
        }
    }

    if let Some(path) = file::project_config_path() {
        if let Some(proj_cfg) = file::load_project_config_file(&path)? {
            apply_project_file_config(&mut config, &proj_cfg, &path)?;
        }
    }

    if let Ok(val) = std::env::var("MSIG_CLUSTER") {
        config.cluster = file::resolve_cluster_moniker(&val);
    }
    if let Ok(val) = std::env::var("MSIG_KEYPAIR") {
        config.keypair = Some(val);
    }
    if let Ok(val) = std::env::var("MSIG_MULTISIG") {
        config.multisig = Some(val);
    }
    if let Ok(val) = std::env::var("MSIG_PRIORITY_FEE") {
        config.priority_fee = val
            .parse()
            .map_err(|_| MsigError::Config(format!("invalid MSIG_PRIORITY_FEE: '{val}'")))?;
    }
    if let Ok(val) = std::env::var("MSIG_PROGRAM_ID") {
        config.program_id = val
            .parse()
            .map_err(|_| MsigError::Config(format!("invalid MSIG_PROGRAM_ID: '{val}'")))?;
    }

    if let Some(ref val) = global_flags.cluster {
        config.cluster = file::resolve_cluster_moniker(val);
    }
    if let Some(ref val) = global_flags.keypair {
        config.keypair = Some(val.clone());
    }
    if let Some(ref val) = global_flags.multisig {
        config.multisig = Some(val.clone());
    }
    if let Some(ref val) = global_flags.commitment {
        config.commitment = val.clone();
    }
    if let Some(val) = global_flags.vault_index {
        config.vault_index = val;
    }
    if let Some(val) = global_flags.priority_fee {
        config.priority_fee = val;
    }
    if let Some(ref val) = global_flags.program_id {
        config.program_id = val
            .parse()
            .map_err(|_| MsigError::Config(format!("invalid program_id: '{val}'")))?;
    }

    validate_config(&config)?;
    Ok(config)
}

fn apply_file_config(config: &mut Config, file_cfg: &file::ConfigFile) -> Result<(), MsigError> {
    if let Some(ref v) = file_cfg.default.cluster {
        config.cluster = file::resolve_cluster_moniker(v);
    }
    if let Some(ref v) = file_cfg.default.keypair {
        config.keypair = Some(v.clone());
    }
    if let Some(ref v) = file_cfg.default.multisig {
        config.multisig = Some(v.clone());
    }
    if let Some(ref v) = file_cfg.default.commitment {
        config.commitment = v.clone();
    }
    if let Some(v) = file_cfg.default.vault_index {
        config.vault_index = v;
    }
    if let Some(v) = file_cfg.default.priority_fee {
        config.priority_fee = v;
    }
    if let Some(ref v) = file_cfg.default.program_id {
        config.program_id = v
            .parse()
            .map_err(|_| MsigError::Config(format!("invalid program_id in config: '{v}'")))?;
    }

    for (k, v) in &file_cfg.labels {
        labels::validate_label(v)?;
        k.parse::<solana_pubkey::Pubkey>()
            .map_err(|_| MsigError::Config(format!("invalid label pubkey in config: '{k}'")))?;
        config.labels.insert(k.clone(), v.clone());
    }
    for (k, v) in &file_cfg.tokens {
        validate_token_symbol(k)?;
        validate_token_mint(v)?;
        config.tokens.insert(k.clone(), v.clone());
    }
    Ok(())
}

fn apply_project_file_config(
    config: &mut Config,
    file_cfg: &file::ConfigFile,
    path: &std::path::Path,
) -> Result<(), MsigError> {
    apply_project_file_config_with_trust(config, file_cfg, path, trust_project_config())
}

fn apply_project_file_config_with_trust(
    config: &mut Config,
    file_cfg: &file::ConfigFile,
    path: &std::path::Path,
    trusted: bool,
) -> Result<(), MsigError> {
    if trusted {
        return apply_file_config(config, file_cfg);
    }

    if let Some(ref v) = file_cfg.default.cluster {
        if !file::is_auto_load_safe_cluster(v) {
            return Err(MsigError::Config(format!(
                "refusing to auto-load cluster '{v}' from {}. Custom RPC URLs and localhost require MSIG_TRUST_PROJECT_CONFIG=1, user config, an env var, or --cluster.",
                path.display()
            )));
        }
    }
    if file_cfg.default.keypair.is_some() {
        return Err(MsigError::Config(format!(
            "refusing to auto-load default.keypair from {}. Set it in user config, MSIG_KEYPAIR, --keypair, or explicitly trust this project with MSIG_TRUST_PROJECT_CONFIG=1.",
            path.display()
        )));
    }
    if file_cfg.default.priority_fee.is_some() {
        return Err(MsigError::Config(format!(
            "refusing to auto-load default.priority_fee from {}. Set it in user config, MSIG_PRIORITY_FEE, --priority-fee, or explicitly trust this project with MSIG_TRUST_PROJECT_CONFIG=1.",
            path.display()
        )));
    }
    if file_cfg.default.program_id.is_some() {
        return Err(MsigError::Config(format!(
            "refusing to auto-load default.program_id from {}. Set it in user config, MSIG_PROGRAM_ID, --program-id, or explicitly trust this project with MSIG_TRUST_PROJECT_CONFIG=1.",
            path.display()
        )));
    }

    if let Some(ref v) = file_cfg.default.cluster {
        config.cluster = file::resolve_cluster_moniker(v);
    }
    if let Some(ref v) = file_cfg.default.multisig {
        config.multisig = Some(v.clone());
    }
    if let Some(ref v) = file_cfg.default.commitment {
        config.commitment = v.clone();
    }
    if let Some(v) = file_cfg.default.vault_index {
        config.vault_index = v;
    }

    for (k, v) in &file_cfg.labels {
        labels::validate_label(v)?;
        k.parse::<solana_pubkey::Pubkey>()
            .map_err(|_| MsigError::Config(format!("invalid label pubkey in config: '{k}'")))?;
        config.labels.insert(k.clone(), v.clone());
    }
    for (k, v) in &file_cfg.tokens {
        validate_token_symbol(k)?;
        validate_token_mint(v)?;
        config.tokens.insert(k.clone(), v.clone());
    }

    Ok(())
}

fn trust_project_config() -> bool {
    std::env::var("MSIG_TRUST_PROJECT_CONFIG")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn validate_config(config: &Config) -> Result<(), MsigError> {
    validate_commitment(&config.commitment)
}

fn validate_commitment(commitment: &str) -> Result<(), MsigError> {
    match commitment {
        "processed" | "confirmed" | "finalized" => Ok(()),
        other => Err(MsigError::Config(format!(
            "invalid commitment '{other}'. Expected processed, confirmed, or finalized."
        ))),
    }
}

fn validate_token_symbol(symbol: &str) -> Result<(), MsigError> {
    if symbol.is_empty()
        || !symbol
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(MsigError::Config(format!(
            "invalid token symbol '{symbol}': use ASCII letters, numbers, '-' or '_'"
        )));
    }
    Ok(())
}

fn validate_token_mint(mint: &str) -> Result<(), MsigError> {
    if mint == "native" {
        return Ok(());
    }
    mint.parse::<solana_pubkey::Pubkey>()
        .map_err(|_| MsigError::Config(format!("invalid token mint in config: '{mint}'")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.cluster, "https://api.mainnet-beta.solana.com");
        assert_eq!(cfg.commitment, "confirmed");
        assert_eq!(cfg.vault_index, 0);
    }

    #[test]
    fn validates_commitment_values() {
        assert!(validate_commitment("processed").is_ok());
        assert!(validate_commitment("confirmed").is_ok());
        assert!(validate_commitment("finalized").is_ok());
        assert!(validate_commitment("rooted").is_err());
    }

    #[test]
    fn untrusted_project_config_rejects_custom_rpc() {
        let mut cfg = Config::default();
        let mut file_cfg = file::ConfigFile::default();
        file_cfg.default.cluster = Some("https://rpc.example.com".to_string());

        let err = match apply_project_file_config_with_trust(
            &mut cfg,
            &file_cfg,
            std::path::Path::new(".msig.toml"),
            false,
        ) {
            Ok(_) => panic!("custom RPC should require trust"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("MSIG_TRUST_PROJECT_CONFIG=1"));
    }

    #[test]
    fn untrusted_project_config_rejects_keypair_program_and_fee() {
        for (field, cfg_file) in [
            ("keypair", {
                let mut cfg_file = file::ConfigFile::default();
                cfg_file.default.keypair = Some("/tmp/id.json".to_string());
                cfg_file
            }),
            ("program_id", {
                let mut cfg_file = file::ConfigFile::default();
                cfg_file.default.program_id = Some(crate::infra::pda::PROGRAM_ID.to_string());
                cfg_file
            }),
            ("priority_fee", {
                let mut cfg_file = file::ConfigFile::default();
                cfg_file.default.priority_fee = Some(1000);
                cfg_file
            }),
        ] {
            let mut cfg = Config::default();
            let err = match apply_project_file_config_with_trust(
                &mut cfg,
                &cfg_file,
                std::path::Path::new(".msig.toml"),
                false,
            ) {
                Ok(_) => panic!("sensitive project config field should require trust"),
                Err(err) => err,
            };
            assert!(format!("{err}").contains(field));
        }
    }

    #[test]
    fn untrusted_project_config_applies_safe_fields() {
        let mut cfg = Config::default();
        let mut file_cfg = file::ConfigFile::default();
        let multisig = solana_pubkey::Pubkey::from([3u8; 32]).to_string();
        let labeled = solana_pubkey::Pubkey::from([4u8; 32]).to_string();
        let mint = solana_pubkey::Pubkey::from([5u8; 32]).to_string();
        file_cfg.default.cluster = Some("devnet".to_string());
        file_cfg.default.multisig = Some(multisig.clone());
        file_cfg.default.commitment = Some("finalized".to_string());
        file_cfg.default.vault_index = Some(2);
        file_cfg.labels.insert(labeled.clone(), "ops".to_string());
        file_cfg.tokens.insert("USDC".to_string(), mint.clone());

        apply_project_file_config_with_trust(
            &mut cfg,
            &file_cfg,
            std::path::Path::new(".msig.toml"),
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(cfg.cluster, "https://api.devnet.solana.com");
        assert_eq!(cfg.multisig.as_deref(), Some(multisig.as_str()));
        assert_eq!(cfg.commitment, "finalized");
        assert_eq!(cfg.vault_index, 2);
        assert_eq!(cfg.labels.get(&labeled).map(String::as_str), Some("ops"));
        assert_eq!(
            cfg.tokens.get("USDC").map(String::as_str),
            Some(mint.as_str())
        );
    }

    #[test]
    fn trusted_project_config_applies_sensitive_fields() {
        let mut cfg = Config::default();
        let mut file_cfg = file::ConfigFile::default();
        let program_id = solana_pubkey::Pubkey::from([6u8; 32]);
        file_cfg.default.cluster = Some("https://rpc.example.com".to_string());
        file_cfg.default.keypair = Some("/tmp/id.json".to_string());
        file_cfg.default.priority_fee = Some(1000);
        file_cfg.default.program_id = Some(program_id.to_string());

        apply_project_file_config_with_trust(
            &mut cfg,
            &file_cfg,
            std::path::Path::new(".msig.toml"),
            true,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(cfg.cluster, "https://rpc.example.com");
        assert_eq!(cfg.keypair.as_deref(), Some("/tmp/id.json"));
        assert_eq!(cfg.priority_fee, 1000);
        assert_eq!(cfg.program_id, program_id);
    }
}
