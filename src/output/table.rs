use super::should_use_color;

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

/// Wrap `s` in bold ANSI escape codes if color is enabled.
pub fn bold(s: &str) -> String {
    if should_use_color() {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Wrap `s` in dim ANSI escape codes if color is enabled.
pub fn dim(s: &str) -> String {
    if should_use_color() {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Wrap `s` in the given ANSI color code if color is enabled.
///
/// Common color codes:
/// - `"31"` = red
/// - `"32"` = green
/// - `"33"` = yellow
/// - `"1;32"` = green + bold
pub fn colored(s: &str, color: &str) -> String {
    if should_use_color() {
        format!("\x1b[{color}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Table printing
// ---------------------------------------------------------------------------

/// Strip ANSI escape sequences to get the visible length of a string.
fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            len += 1;
        }
    }
    len
}

/// Pad a string that may contain ANSI codes to a visible width.
fn pad_visible(s: &str, width: usize) -> String {
    let vis = visible_len(s);
    if vis >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - vis))
    }
}

/// Print a simple aligned table with auto-calculated column widths.
/// Handles ANSI color codes correctly when calculating alignment.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let col_count = headers.len();

    // Calculate the maximum VISIBLE width for each column.
    let mut widths = vec![0usize; col_count];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(visible_len(cell));
            }
        }
    }

    // Print header row.
    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let padded = format!("{:<width$}", h, width = widths[i]);
            bold(&padded)
        })
        .collect();
    println!("{}", header_line.join("  "));

    // Print data rows with ANSI-aware padding.
    for row in rows {
        let line: Vec<String> = (0..col_count)
            .map(|i| {
                let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
                pad_visible(cell, widths[i])
            })
            .collect();
        println!("{}", line.join("  "));
    }
}

// ---------------------------------------------------------------------------
// Domain-specific formatters
// ---------------------------------------------------------------------------

/// Colorize a proposal status string.
///
/// - `"Active"` = yellow
/// - `"Approved"` = green
/// - `"Executed"` = green + bold
/// - `"Rejected"` = red
/// - `"Cancelled"` = dim
/// - `"Draft"` = dim
/// - anything else = unchanged
pub fn format_status(status: &str) -> String {
    match status {
        "Active" => colored(status, "33"),
        "Approved" => colored(status, "32"),
        "Executed" => colored(status, "1;32"),
        "Rejected" => colored(status, "31"),
        "Cancelled" => dim(status),
        "Draft" => dim(status),
        other => other.to_string(),
    }
}

/// Format a permission bitmask into a human-readable string.
///
/// Bit 0 (1) = Initiate
/// Bit 1 (2) = Vote
/// Bit 2 (4) = Execute
pub fn format_permissions(permissions: u8) -> String {
    let mut parts = Vec::new();
    if permissions & 1 != 0 {
        parts.push("Initiate");
    }
    if permissions & 2 != 0 {
        parts.push("Vote");
    }
    if permissions & 4 != 0 {
        parts.push("Execute");
    }
    if parts.is_empty() {
        "None".to_string()
    } else {
        parts.join(", ")
    }
}

/// Format a Unix timestamp as a relative time string.
/// Uses the standard library to avoid a chrono dependency.
pub fn format_relative_time(timestamp: i64) -> String {
    let now = current_unix_time();
    if timestamp <= 0 || now <= 0 {
        return String::new();
    }
    let diff = now - timestamp;
    if diff < 0 {
        return "just now".to_string();
    }
    let diff = diff as u64;
    if diff < 60 {
        return format!("{diff}s ago");
    }
    if diff < 3600 {
        return format!("{}m ago", diff / 60);
    }
    if diff < 86400 {
        return format!("{}h ago", diff / 3600);
    }
    if diff < 2592000 {
        return format!("{}d ago", diff / 86400);
    }
    format!("{}mo ago", diff / 2592000)
}

fn current_unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_permissions() {
        assert_eq!(format_permissions(0), "None");
        assert_eq!(format_permissions(1), "Initiate");
        assert_eq!(format_permissions(3), "Initiate, Vote");
        assert_eq!(format_permissions(7), "Initiate, Vote, Execute");
        assert_eq!(format_permissions(6), "Vote, Execute");
    }
}
