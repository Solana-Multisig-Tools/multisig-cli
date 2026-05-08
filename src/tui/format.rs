use crate::output::format_addr;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Build a vote progress bar: `[███░░] 2/3`
///
/// Returns a vec of styled spans.
#[allow(dead_code)]
pub fn vote_bar<'a>(approved: usize, threshold: u16, total: usize, width: usize) -> Vec<Span<'a>> {
    let filled = approved.min(width);
    let empty = width.saturating_sub(filled);

    let bar_filled: String = "\u{2588}".repeat(filled); // █
    let bar_empty: String = "\u{2591}".repeat(empty); // ░

    let (fill_color, label_style) = if approved >= threshold as usize {
        (Color::Green, Style::default().fg(Color::Green))
    } else if approved > 0 {
        (Color::Yellow, Style::default().fg(Color::Yellow))
    } else {
        (Color::DarkGray, Style::default().fg(Color::DarkGray))
    };

    vec![
        Span::styled(bar_filled, Style::default().fg(fill_color)),
        Span::styled(bar_empty, Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {}/{}", approved, total), label_style),
    ]
}

/// Return a status symbol and color for a proposal status label.
pub fn status_display(status: &str) -> (&'static str, Color) {
    match status {
        "Executed" => ("\u{2713} Executed", Color::Green), // ✓
        "Executing" => ("\u{25D0} Executing", Color::Green), // ◐
        "Approved" => ("\u{25C6} Approved", Color::Green), // ◆
        "Active" => ("\u{25D0} Active", Color::Yellow),    // ◐
        "Rejected" => ("\u{2717} Rejected", Color::Red),   // ✗
        "Cancelled" => ("\u{2298} Cancelled", Color::Red), // ⊘
        "Draft" => ("\u{2591} Draft", Color::DarkGray),    // ░
        _ => ("\u{25CF} Unknown", Color::DarkGray),        // ●
    }
}

/// Loading spinner frame using braille dot pattern.
pub fn spinner_frame(tick: u64) -> char {
    const FRAMES: &[char] = &[
        '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280F}',
    ];
    FRAMES[(tick as usize) % FRAMES.len()]
}

/// Build a help bar from key-label pairs.
pub fn help_spans<'a>(
    pairs: &[(&'a str, &'a str)],
    theme: &crate::tui::theme::Theme,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme.dim_style()));
        }
        spans.push(Span::styled(*key, theme.title_style()));
        spans.push(Span::styled(format!(" {label}"), theme.dim_style()));
    }
    spans
}

/// Member bullet: `● AbCd...WxYz  Initiate, Vote, Execute`
#[allow(dead_code)]
pub fn member_line<'a>(
    addr: &str,
    permissions: &[&str],
    theme: &crate::tui::theme::Theme,
    truncate: bool,
) -> Vec<Span<'a>> {
    let perms_str = permissions.join(", ");
    vec![
        Span::styled(
            format!("  \u{25CF} {}  ", format_addr(addr, truncate)),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(perms_str, theme.dim_style()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles() {
        let c0 = spinner_frame(0);
        let c1 = spinner_frame(1);
        assert_ne!(c0, c1);
        // Wraps around
        assert_eq!(spinner_frame(0), spinner_frame(10));
    }

    #[test]
    fn status_display_known() {
        let (label, color) = status_display("Executed");
        assert!(label.contains("Executed"));
        assert_eq!(color, Color::Green);
    }

    #[test]
    fn vote_bar_returns_spans() {
        let spans = vote_bar(2, 3, 3, 5);
        assert_eq!(spans.len(), 3);
    }
}
