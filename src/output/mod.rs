pub mod json;
pub mod table;

pub use crate::error::OutputMode;

pub fn detect_output_mode(flag: Option<&str>) -> OutputMode {
    match flag {
        Some("json") => OutputMode::Json,
        Some("table") | Some(_) => OutputMode::Text,
        None => {
            if stdout_is_tty() {
                OutputMode::Text
            } else {
                OutputMode::Json
            }
        }
    }
}

pub fn should_use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn stdout_is_tty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

/// Format lamports as a trimmed SOL string (no trailing zeros).
///
/// Example: `1_500_000_000` -> `"1.5"`, `0` -> `"0"`, `1_000_000_000` -> `"1"`
pub fn format_sol(lamports: u64) -> String {
    let whole = lamports / 1_000_000_000;
    let frac = lamports % 1_000_000_000;
    if frac == 0 {
        return format!("{whole}");
    }
    let frac_str = format!("{frac:09}");
    let trimmed = frac_str.trim_end_matches('0');
    format!("{whole}.{trimmed}")
}

/// Abbreviated pubkey address format, displaying
/// on the beginning and end of the base58-encoded string
///
/// Example `7nE9GvcwsqzjiaKchVRvG4F6BaLqGmZ9` -> `7nE9...GmZ9`
pub fn abbreviate_addr(pubkey: &str) -> String {
    if pubkey.len() <= 11 {
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
    fn test_format_sol() {
        assert_eq!(format_sol(0), "0");
        assert_eq!(format_sol(1_000_000_000), "1");
        assert_eq!(format_sol(1_234_567_890), "1.23456789");
        assert_eq!(format_sol(500_000), "0.0005");
        assert_eq!(format_sol(1_500_000_000), "1.5");
        assert_eq!(format_sol(450_000_000), "0.45");
    }

    #[test]
    fn abbreviate_addr_formats_correctly() {
        let addr = "7nE9GvcwsqzjiaKchVRvG4F6BaLqGmZ9";
        assert_eq!(abbreviate_addr(addr), "7nE9...GmZ9");
    }

    #[test]
    fn abbreviate_addr_short_passthrough() {
        assert_eq!(abbreviate_addr("AbCd"), "AbCd");
    }
}
