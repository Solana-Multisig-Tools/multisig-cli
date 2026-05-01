use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::MsigError;

/// Raw TOML structure matching the config file format.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub default: DefaultSection,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct DefaultSection {
    pub cluster: Option<String>,
    pub keypair: Option<String>,
    pub multisig: Option<String>,
    pub commitment: Option<String>,
    pub vault_index: Option<u8>,
    pub priority_fee: Option<u64>,
    pub program_id: Option<String>,
    pub truncate_addresses: Option<bool>,
}

/// Resolve a cluster moniker to a full RPC URL.
///
/// "devnet" | "d" -> devnet URL
/// "mainnet" | "m" -> mainnet URL
/// "testnet" | "t" -> testnet URL
/// "localhost" | "l" -> localhost URL
/// anything else -> treated as a URL
pub fn resolve_cluster_moniker(input: &str) -> String {
    match input {
        "devnet" | "d" => "https://api.devnet.solana.com".to_string(),
        "mainnet" | "mainnet-beta" | "m" => "https://api.mainnet-beta.solana.com".to_string(),
        "testnet" | "t" => "https://api.testnet.solana.com".to_string(),
        "localhost" | "l" => "http://127.0.0.1:8899".to_string(),
        other => other.to_string(),
    }
}

/// Return true for cluster values that are safe for untrusted project config.
///
/// This deliberately excludes localhost and arbitrary RPC URLs. Use
/// MSIG_TRUST_PROJECT_CONFIG=1, user config, env vars, or CLI flags for those.
pub fn is_auto_load_safe_cluster(input: &str) -> bool {
    matches!(
        input,
        "devnet"
            | "d"
            | "mainnet"
            | "mainnet-beta"
            | "m"
            | "testnet"
            | "t"
            | "https://api.devnet.solana.com"
            | "https://api.mainnet-beta.solana.com"
            | "https://api.testnet.solana.com"
    )
}

/// Return the path to the user-level config file (~/.config/msig/config.toml).
pub fn user_config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("msig").join("config.toml"))
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
        })
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))
}

/// Return the path to the project-level config file (.msig.toml in cwd).
pub fn project_config_path() -> Option<PathBuf> {
    std::env::current_dir().ok().map(|d| d.join(".msig.toml"))
}

/// Interpolate `${ENV_VAR}` references in a string value.
fn interpolate_env(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var_name.push(ch);
            }
            if let Ok(val) = std::env::var(&var_name) {
                result.push_str(&val);
            }
            // If env var not found, replace with empty string
        } else {
            result.push(c);
        }
    }

    result
}

/// Interpolate env vars in the [default] section only.
///
/// Labels and tokens are NOT interpolated to prevent information disclosure
/// if a malicious .msig.toml is placed in the working directory
/// (e.g. `"${AWS_SECRET_ACCESS_KEY}"` as a label value).
fn interpolate_config(cfg: &mut ConfigFile) {
    if let Some(ref mut v) = cfg.default.cluster {
        *v = interpolate_env(v);
    }
    if let Some(ref mut v) = cfg.default.keypair {
        *v = interpolate_env(v);
    }
    if let Some(ref mut v) = cfg.default.multisig {
        *v = interpolate_env(v);
    }
    if let Some(ref mut v) = cfg.default.commitment {
        *v = interpolate_env(v);
    }
    if let Some(ref mut v) = cfg.default.program_id {
        *v = interpolate_env(v);
    }
}

fn load_config_file_inner(
    path: &Path,
    interpolate_default_env: bool,
) -> Result<Option<ConfigFile>, MsigError> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| MsigError::Config(format!("failed to read {}: {e}", path.display())))?;

    let mut cfg: ConfigFile = toml::from_str(&raw)
        .map_err(|e| MsigError::Config(format!("failed to parse {}: {e}", path.display())))?;

    if interpolate_default_env {
        interpolate_config(&mut cfg);
    }

    Ok(Some(cfg))
}

/// Load and parse a trusted TOML config file, with env var interpolation.
pub fn load_config_file(path: &Path) -> Result<Option<ConfigFile>, MsigError> {
    load_config_file_inner(path, true)
}

/// Load and parse an auto-loaded project config file.
///
/// Project config is read from the current working directory and may come from
/// a repo checkout, so it never receives environment-variable interpolation.
pub fn load_project_config_file(path: &Path) -> Result<Option<ConfigFile>, MsigError> {
    load_config_file_inner(path, false)
}

/// Ensure the config directory exists, then save a single key=value to the
/// user config file. Supports dotted keys like "default.cluster".
pub fn save_config_value(key: &str, value: &str) -> Result<(), MsigError> {
    let path = user_config_path()
        .ok_or_else(|| MsigError::Config("cannot determine config directory".into()))?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            MsigError::Config(format!(
                "failed to create config directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Load existing or create new
    let mut cfg = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| MsigError::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str::<ConfigFile>(&raw)
            .map_err(|e| MsigError::Config(format!("failed to parse {}: {e}", path.display())))?
    } else {
        ConfigFile::default()
    };

    // Apply the value
    match key {
        "cluster" | "default.cluster" => cfg.default.cluster = Some(value.to_string()),
        "keypair" | "default.keypair" => cfg.default.keypair = Some(value.to_string()),
        "multisig" | "default.multisig" => cfg.default.multisig = Some(value.to_string()),
        "commitment" | "default.commitment" => cfg.default.commitment = Some(value.to_string()),
        "priority_fee" | "default.priority_fee" => {
            let fee: u64 = value
                .parse()
                .map_err(|_| MsigError::Config(format!("invalid priority_fee: {value}")))?;
            cfg.default.priority_fee = Some(fee);
        }
        "program_id" | "default.program_id" => {
            value
                .parse::<solana_pubkey::Pubkey>()
                .map_err(|_| MsigError::Config(format!("invalid program_id: {value}")))?;
            cfg.default.program_id = Some(value.to_string());
        }
        "vault_index" | "default.vault_index" => {
            let idx: u8 = value
                .parse()
                .map_err(|_| MsigError::Config(format!("invalid vault_index: {value}")))?;
            cfg.default.vault_index = Some(idx);
        }
        "truncate_addresses" | "default.truncate_addresses" => {
            let parsed = match value {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => {
                    return Err(MsigError::Config(format!(
                        "invalid truncate_addresses: '{value}'. Expected true or false."
                    )));
                }
            };
            cfg.default.truncate_addresses = Some(parsed);
        }
        other => {
            // Handle labels.NAME and tokens.NAME
            if let Some(label_key) = other.strip_prefix("labels.") {
                // Validate label value is ASCII-safe
                super::labels::validate_label(value)?;
                cfg.labels.insert(label_key.to_string(), value.to_string());
            } else if let Some(token_key) = other.strip_prefix("tokens.") {
                cfg.tokens.insert(token_key.to_string(), value.to_string());
            } else {
                return Err(MsigError::Config(format!("unknown config key: {other}")));
            }
        }
    }

    let serialized = toml::to_string_pretty(&cfg)
        .map_err(|e| MsigError::Config(format!("failed to serialize config: {e}")))?;

    std::fs::write(&path, serialized)
        .map_err(|e| MsigError::Config(format!("failed to write {}: {e}", path.display())))?;

    // Set file mode 0600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).map_err(|e| {
            MsigError::Config(format!(
                "failed to set permissions on {}: {e}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

/// Return the path to the state file (~/.config/msig/state.toml).
pub fn state_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("msig").join("state.toml"))
}

/// Read the active profile name from state.toml, if any.
pub fn read_active_profile() -> Option<String> {
    let path = state_file_path()?;
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    let table: toml::Table = toml::from_str(&raw).ok()?;
    table
        .get("active_profile")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Validate that a profile exists in the user config file.
///
/// Profiles are defined as `[profile.<NAME>]` sections.
pub fn validate_profile_exists(profile_name: &str) -> Result<(), MsigError> {
    let path = user_config_path()
        .ok_or_else(|| MsigError::Config("cannot determine config directory".into()))?;

    if !path.exists() {
        return Err(MsigError::Config(format!(
            "config file not found at {}. Create it first with 'msig config set'.",
            path.display()
        )));
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| MsigError::Config(format!("failed to read {}: {e}", path.display())))?;

    // Parse as raw TOML table to check for profile sections
    let table: toml::Table = toml::from_str(&raw)
        .map_err(|e| MsigError::Config(format!("failed to parse {}: {e}", path.display())))?;

    // Check for [profile.<NAME>] section
    if let Some(profiles) = table.get("profile") {
        if let Some(profiles_table) = profiles.as_table() {
            if profiles_table.contains_key(profile_name) {
                return Ok(());
            }
        }
    }

    // Also allow "default" as a special profile name (always exists)
    if profile_name == "default" {
        return Ok(());
    }

    Err(MsigError::Config(format!(
        "profile '{profile_name}' not found. Define it as [profile.{profile_name}] in {}.",
        path.display()
    )))
}

/// Save the active profile name to state.toml.
pub fn save_active_profile(profile_name: &str) -> Result<(), MsigError> {
    let path = state_file_path()
        .ok_or_else(|| MsigError::Config("cannot determine config directory".into()))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            MsigError::Config(format!(
                "failed to create config directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let content = format!("active_profile = \"{profile_name}\"\n");
    std::fs::write(&path, content)
        .map_err(|e| MsigError::Config(format!("failed to write {}: {e}", path.display())))?;

    Ok(())
}

/// Load a profile's config section and apply it on top of the given config.
pub fn apply_profile(config: &mut super::Config, profile_name: &str) -> Result<(), MsigError> {
    let path = match user_config_path() {
        Some(p) if p.exists() => p,
        _ => return Ok(()), // No config file, nothing to apply
    };

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| MsigError::Config(format!("failed to read {}: {e}", path.display())))?;

    let table: toml::Table = toml::from_str(&raw)
        .map_err(|e| MsigError::Config(format!("failed to parse {}: {e}", path.display())))?;

    let profile_section = table
        .get("profile")
        .and_then(|p| p.as_table())
        .and_then(|p| p.get(profile_name))
        .and_then(|p| p.as_table());

    if let Some(section) = profile_section {
        if let Some(v) = section.get("cluster").and_then(|v| v.as_str()) {
            config.cluster = resolve_cluster_moniker(v);
        }
        if let Some(v) = section.get("keypair").and_then(|v| v.as_str()) {
            config.keypair = Some(v.to_string());
        }
        if let Some(v) = section.get("multisig").and_then(|v| v.as_str()) {
            config.multisig = Some(v.to_string());
        }
        if let Some(v) = section.get("commitment").and_then(|v| v.as_str()) {
            config.commitment = v.to_string();
        }
        if let Some(v) = section.get("vault_index").and_then(|v| v.as_integer()) {
            if let Ok(idx) = u8::try_from(v) {
                config.vault_index = idx;
            }
        }
        if let Some(v) = section.get("priority_fee").and_then(|v| v.as_integer()) {
            if let Ok(fee) = u64::try_from(v) {
                config.priority_fee = fee;
            }
        }
        if let Some(v) = section.get("program_id").and_then(|v| v.as_str()) {
            config.program_id = v.parse().map_err(|_| {
                MsigError::Config(format!(
                    "invalid program_id in profile '{profile_name}': {v}"
                ))
            })?;
        }
        if let Some(v) = section.get("truncate_addresses").and_then(|v| v.as_bool()) {
            config.truncate_addresses = v;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_cluster_moniker() {
        assert_eq!(
            resolve_cluster_moniker("devnet"),
            "https://api.devnet.solana.com"
        );
        assert_eq!(
            resolve_cluster_moniker("d"),
            "https://api.devnet.solana.com"
        );
        assert_eq!(
            resolve_cluster_moniker("mainnet"),
            "https://api.mainnet-beta.solana.com"
        );
        assert_eq!(
            resolve_cluster_moniker("m"),
            "https://api.mainnet-beta.solana.com"
        );
        assert_eq!(
            resolve_cluster_moniker("https://custom.rpc.com"),
            "https://custom.rpc.com"
        );
    }

    #[test]
    fn test_interpolate_env() {
        std::env::set_var("MSIG_TEST_VAR_12345", "hello");
        assert_eq!(interpolate_env("${MSIG_TEST_VAR_12345}"), "hello");
        assert_eq!(
            interpolate_env("pre-${MSIG_TEST_VAR_12345}-post"),
            "pre-hello-post"
        );
        assert_eq!(interpolate_env("no vars here"), "no vars here");
        std::env::remove_var("MSIG_TEST_VAR_12345");
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = load_config_file(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_ok());
        assert!(result.ok().flatten().is_none());
    }

    #[test]
    fn test_project_config_does_not_interpolate_env() {
        std::env::set_var("MSIG_TEST_PROJECT_CLUSTER", "devnet");
        let path = std::env::temp_dir().join(format!(
            "msig-project-config-no-env-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "[default]\ncluster = \"${MSIG_TEST_PROJECT_CLUSTER}\"\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let cfg = load_project_config_file(&path)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("project config should load"));

        assert_eq!(
            cfg.default.cluster.as_deref(),
            Some("${MSIG_TEST_PROJECT_CLUSTER}")
        );

        let _ = std::fs::remove_file(path);
        std::env::remove_var("MSIG_TEST_PROJECT_CLUSTER");
    }
}
