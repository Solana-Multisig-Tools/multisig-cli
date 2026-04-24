use std::collections::HashMap;

use crate::error::MsigError;

/// Validate that a label contains only ASCII alphanumeric characters, hyphens, and underscores.
pub fn validate_label(label: &str) -> Result<(), MsigError> {
    if label.is_empty() {
        return Err(MsigError::Config("label cannot be empty".into()));
    }
    for ch in label.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            return Err(MsigError::Config(format!(
                "label {label:?} contains invalid character {ch:?}: \
                 only ASCII alphanumeric, '-', and '_' are allowed"
            )));
        }
    }
    Ok(())
}

/// Check if `input` is a valid base58-encoded 32-byte public key.
fn is_valid_pubkey(input: &str) -> bool {
    match bs58::decode(input).into_vec() {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

/// Resolve a user-provided address input to a base58 pubkey string.
///
/// Priority:
/// 1. If `input` is a valid base58 pubkey (32 bytes), return as-is.
/// 2. Look up `input` as a label (reverse lookup: find key where value == input).
/// 3. Error if not found.
pub fn resolve_address(input: &str, labels: &HashMap<String, String>) -> Result<String, MsigError> {
    // Try as base58 pubkey first
    if is_valid_pubkey(input) {
        return Ok(input.to_string());
    }

    // Reverse lookup: labels maps pubkey -> label, so find the pubkey whose label matches input
    for (pubkey, label) in labels {
        if label == input {
            return Ok(pubkey.clone());
        }
    }

    Err(MsigError::Config(format!(
        "cannot resolve address: {input:?} is neither a valid pubkey nor a known label"
    )))
}

/// Format a pubkey for display, optionally including its label.
///
/// If the pubkey has a label: "label (first4...last4)"
/// Otherwise: "first4...last4"
pub fn format_address(pubkey: &str, labels: &HashMap<String, String>) -> String {
    let short = abbreviate_pubkey(pubkey);
    if let Some(label) = labels.get(pubkey) {
        format!("{label} ({short})")
    } else {
        short
    }
}

fn abbreviate_pubkey(pubkey: &str) -> String {
    if pubkey.len() <= 8 {
        pubkey.to_string()
    } else {
        let first4 = &pubkey[..4];
        let last4 = &pubkey[pubkey.len() - 4..];
        format!("{first4}...{last4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_label_valid() {
        assert!(validate_label("alice").is_ok());
        assert!(validate_label("alice-1").is_ok());
        assert!(validate_label("bob_2").is_ok());
        assert!(validate_label("A-Z_09").is_ok());
    }

    #[test]
    fn test_validate_label_reject_non_ascii() {
        assert!(validate_label("al\u{00ef}ce").is_err()); // "alice" with i-umlaut
        assert!(validate_label("").is_err());
        assert!(validate_label("hello world").is_err()); // space
        assert!(validate_label("alice!").is_err());
    }

    #[test]
    fn test_resolve_address_base58_priority() {
        // A valid 32-byte base58 pubkey (all 1s = 32 zero bytes)
        let valid_pubkey = "11111111111111111111111111111111";
        let mut labels = HashMap::new();
        // Even if the pubkey is also a label value, base58 takes priority
        labels.insert(
            "SomeOtherKey1111111111111111111111111111111111".to_string(),
            valid_pubkey.to_string(),
        );

        let result = resolve_address(valid_pubkey, &labels);
        assert!(result.is_ok());
        assert_eq!(result.ok(), Some(valid_pubkey.to_string()));
    }

    #[test]
    fn test_resolve_address_label_lookup() {
        let mut labels = HashMap::new();
        labels.insert(
            "7nE9GvcwsqzjRKRFbWaCN2LNpHMiQk7Q2oJsySWZ6gKm".to_string(),
            "alice".to_string(),
        );

        let result = resolve_address("alice", &labels);
        assert!(result.is_ok());
        assert_eq!(
            result.ok(),
            Some("7nE9GvcwsqzjRKRFbWaCN2LNpHMiQk7Q2oJsySWZ6gKm".to_string())
        );
    }

    #[test]
    fn test_resolve_address_not_found() {
        let labels = HashMap::new();
        let result = resolve_address("unknown", &labels);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_address_with_label() {
        let mut labels = HashMap::new();
        labels.insert(
            "7nE9GvcwsqzjRKRFbWaCN2LNpHMiQk7Q2oJsySWZ6gKm".to_string(),
            "alice".to_string(),
        );

        let formatted = format_address("7nE9GvcwsqzjRKRFbWaCN2LNpHMiQk7Q2oJsySWZ6gKm", &labels);
        assert_eq!(formatted, "alice (7nE9...6gKm)");
    }

    #[test]
    fn test_format_address_without_label() {
        let labels = HashMap::new();
        let formatted = format_address("7nE9GvcwsqzjRKRFbWaCN2LNpHMiQk7Q2oJsySWZ6gKm", &labels);
        assert_eq!(formatted, "7nE9...6gKm");
    }
}
