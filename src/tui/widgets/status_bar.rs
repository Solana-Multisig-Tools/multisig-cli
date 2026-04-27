use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::output::abbreviate_addr;
use crate::tui::theme::Theme;

/// Render the bottom status bar showing multisig, cluster, and signer info.
pub fn render_status_bar(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    multisig: Option<&str>,
    multisig_label: Option<&str>,
    cluster: &str,
    keypair: Option<&str>,
) {
    let style = theme.status_bar_style();

    let multisig_display = match (multisig_label, multisig) {
        (Some(label), Some(addr)) => format!("{label} ({})", abbreviate_addr(addr)),
        (None, Some(addr)) => abbreviate_addr(addr),
        _ => "none".to_string(),
    };

    let cluster_short = cluster_display(cluster);

    let signer = match keypair {
        Some(path) => {
            // Show just the filename
            let name = path.rsplit('/').next().unwrap_or(path);
            name.to_string()
        }
        None => "no signer".to_string(),
    };

    // Connection indicator
    let conn_indicator = "\u{25CF}"; // ●

    let spans = vec![
        Span::styled(" ", style),
        Span::styled(conn_indicator, theme.success_style()),
        Span::styled(" ", style),
        Span::styled(multisig_display, theme.title_style()),
        Span::styled(" \u{2502} ", theme.dim_style()), // │
        Span::styled(cluster_short, style),
        Span::styled(" \u{2502} ", theme.dim_style()), // │
        Span::styled(signer, style),
    ];

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

fn cluster_display(url: &str) -> &str {
    if url.contains("mainnet") {
        "mainnet"
    } else if url.contains("devnet") {
        "devnet"
    } else if url.contains("testnet") {
        "testnet"
    } else if url.contains("localhost") || url.contains("127.0.0.1") {
        "localnet"
    } else {
        "custom"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_display_mainnet() {
        assert_eq!(
            cluster_display("https://api.mainnet-beta.solana.com"),
            "mainnet"
        );
    }

    #[test]
    fn cluster_display_devnet() {
        assert_eq!(cluster_display("https://api.devnet.solana.com"), "devnet");
    }

    #[test]
    fn cluster_display_custom() {
        assert_eq!(cluster_display("https://my-rpc.example.com"), "custom");
    }
}
