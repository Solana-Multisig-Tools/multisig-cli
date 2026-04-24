use std::collections::HashMap;

use crate::error::MsigError;
use crate::infra::rpc::RpcProvider;

/// Information about a resolved token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    pub symbol: Option<String>,
    pub mint: String,
    pub decimals: u8,
}

/// Built-in token registry entry.
struct BuiltinToken {
    symbol: &'static str,
    mint: &'static str,
    decimals: u8,
}

const BUILTIN_TOKENS: &[BuiltinToken] = &[
    BuiltinToken {
        symbol: "SOL",
        mint: "native",
        decimals: 9,
    },
    BuiltinToken {
        symbol: "USDC",
        mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        decimals: 6,
    },
    BuiltinToken {
        symbol: "USDT",
        mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
        decimals: 6,
    },
];

/// Resolve a token identifier to a `TokenInfo`.
///
/// Priority:
/// 1. Check built-in registry (case-insensitive symbol match)
/// 2. Check config `[tokens]` section
/// 3. Treat as a raw mint address
pub fn resolve_token(
    input: &str,
    config_tokens: &HashMap<String, String>,
) -> Result<TokenInfo, MsigError> {
    // 1. Check built-in registry (case-insensitive)
    let input_upper = input.to_uppercase();
    for token in BUILTIN_TOKENS {
        if token.symbol == input_upper {
            return Ok(TokenInfo {
                symbol: Some(token.symbol.to_string()),
                mint: token.mint.to_string(),
                decimals: token.decimals,
            });
        }
    }

    // 2. Check config tokens (case-insensitive key match)
    for (symbol, mint) in config_tokens {
        if symbol.to_uppercase() == input_upper {
            // Check if this config token matches a built-in (to get decimals)
            for builtin in BUILTIN_TOKENS {
                if builtin.mint == mint.as_str() {
                    return Ok(TokenInfo {
                        symbol: Some(symbol.clone()),
                        mint: mint.clone(),
                        decimals: builtin.decimals,
                    });
                }
            }
            return Ok(TokenInfo {
                symbol: Some(symbol.clone()),
                mint: mint.clone(),
                decimals: 0, // unknown decimals, caller must fetch from RPC
            });
        }
    }

    // 3. Treat as a mint address
    Ok(TokenInfo {
        symbol: None,
        mint: input.to_string(),
        decimals: 0, // unknown decimals, caller must fetch from RPC
    })
}

/// Fetch the decimals for an SPL token mint from the on-chain account.
///
/// SPL Token mint layout: decimals is at byte offset 44 (1 byte).
pub fn fetch_token_decimals(rpc: &dyn RpcProvider, mint_address: &str) -> Result<u8, MsigError> {
    let account = rpc.get_account_info(mint_address)?.ok_or_else(|| {
        MsigError::Config(format!(
            "Unknown token. Mint account not found: {mint_address}. \
                 Add to config with: msig config set token <SYMBOL> {mint_address}"
        ))
    })?;

    // SPL Token mint account must be at least 82 bytes; decimals at offset 44.
    if account.data.len() < 82 {
        return Err(MsigError::Config(format!(
            "Unknown token. Account {mint_address} does not appear to be a valid SPL token mint. \
             Add to config with: msig config set token <SYMBOL> {mint_address}"
        )));
    }

    Ok(account.data[44])
}

/// Parse a human-readable amount string into the smallest token unit (lamports, etc.).
///
/// Uses integer-only arithmetic -- NEVER f64.
///
/// Examples with decimals=6:
///   "100"      -> 100_000_000
///   "0.000001" -> 1
///   "1.5"      -> 1_500_000
pub fn parse_human_amount(input: &str, decimals: u8) -> Result<u64, MsigError> {
    let input = input.trim();

    // Reject negative
    if input.starts_with('-') {
        return Err(MsigError::Config(format!(
            "negative amounts are not allowed: {input}"
        )));
    }

    // Reject empty
    if input.is_empty() {
        return Err(MsigError::Config("amount cannot be empty".into()));
    }

    let (integer_str, frac_str) = if let Some(dot_pos) = input.find('.') {
        let (int_part, rest) = input.split_at(dot_pos);
        let frac_part = &rest[1..]; // skip the '.'

        // Reject trailing dot with no fraction like "100."
        // Actually, "100." is fine, means "100.0"

        // Reject too many decimal places
        if frac_part.len() > decimals as usize {
            return Err(MsigError::Config(format!(
                "too many decimal places in {input:?}: \
                 max {decimals} decimals allowed, got {}",
                frac_part.len()
            )));
        }

        (int_part, frac_part)
    } else {
        (input, "")
    };

    // Parse integer part
    let integer: u64 = if integer_str.is_empty() {
        0
    } else {
        integer_str
            .parse::<u64>()
            .map_err(|_| MsigError::Config(format!("invalid amount: {input:?}")))?
    };

    // Parse fractional part, padded to `decimals` places
    let frac_value: u64 = if decimals == 0 {
        if !frac_str.is_empty() {
            return Err(MsigError::Config(format!(
                "token has 0 decimals but amount has fractional part: {input:?}"
            )));
        }
        0
    } else {
        let mut padded = frac_str.to_string();
        while padded.len() < decimals as usize {
            padded.push('0');
        }
        padded
            .parse::<u64>()
            .map_err(|_| MsigError::Config(format!("invalid fractional amount: {input:?}")))?
    };

    // multiplier = 10^decimals
    let multiplier: u64 = 10u64
        .checked_pow(u32::from(decimals))
        .ok_or_else(|| MsigError::Config(format!("decimal overflow for {decimals} decimals")))?;

    let base = integer
        .checked_mul(multiplier)
        .ok_or_else(|| MsigError::Config(format!("amount overflow: {input:?}")))?;

    let total = base
        .checked_add(frac_value)
        .ok_or_else(|| MsigError::Config(format!("amount overflow: {input:?}")))?;

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_builtin_tokens() {
        let empty = HashMap::new();

        let sol = resolve_token("SOL", &empty);
        assert!(sol.is_ok());
        let sol = sol.ok();
        assert_eq!(sol.as_ref().map(|t| t.mint.as_str()), Some("native"));
        assert_eq!(sol.as_ref().map(|t| t.decimals), Some(9));

        let usdc = resolve_token("usdc", &empty);
        assert!(usdc.is_ok());
        let usdc = usdc.ok();
        assert_eq!(
            usdc.as_ref().map(|t| t.mint.as_str()),
            Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
        );
        assert_eq!(usdc.as_ref().map(|t| t.decimals), Some(6));
    }

    #[test]
    fn test_resolve_config_token() {
        let mut config = HashMap::new();
        config.insert(
            "BONK".to_string(),
            "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        );

        let bonk = resolve_token("bonk", &config);
        assert!(bonk.is_ok());
        let bonk = bonk.ok();
        assert_eq!(
            bonk.as_ref().map(|t| t.mint.as_str()),
            Some("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263")
        );
    }

    #[test]
    fn test_resolve_raw_mint() {
        let empty = HashMap::new();
        let result = resolve_token("SomeRandomMintAddress", &empty);
        assert!(result.is_ok());
        let info = result.ok();
        assert_eq!(
            info.as_ref().map(|t| t.mint.as_str()),
            Some("SomeRandomMintAddress")
        );
        assert_eq!(info.as_ref().map(|t| t.symbol.as_ref()), Some(None));
    }

    #[test]
    fn test_parse_human_amount_whole_number() {
        assert_eq!(parse_human_amount("100", 6).ok(), Some(100_000_000));
    }

    #[test]
    fn test_parse_human_amount_smallest_unit() {
        assert_eq!(parse_human_amount("0.000001", 6).ok(), Some(1));
    }

    #[test]
    fn test_parse_human_amount_fractional() {
        assert_eq!(parse_human_amount("1.5", 6).ok(), Some(1_500_000));
    }

    #[test]
    fn test_parse_human_amount_u64_max() {
        // u64::MAX = 18446744073709551615
        // With 9 decimals: 18446744073.709551615
        assert_eq!(
            parse_human_amount("18446744073.709551615", 9).ok(),
            Some(u64::MAX)
        );
    }

    #[test]
    fn test_parse_human_amount_too_many_decimals() {
        assert!(parse_human_amount("100.0000001", 6).is_err());
    }

    #[test]
    fn test_parse_human_amount_negative() {
        assert!(parse_human_amount("-1", 6).is_err());
    }

    #[test]
    fn test_parse_human_amount_overflow() {
        assert!(parse_human_amount("18446744073709551616", 0).is_err());
    }

    #[test]
    fn test_parse_human_amount_zero_decimals() {
        assert_eq!(parse_human_amount("42", 0).ok(), Some(42));
        assert!(parse_human_amount("42.1", 0).is_err());
    }
}
