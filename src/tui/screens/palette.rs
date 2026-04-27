use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::output::abbreviate_addr;
use crate::tui::app::{App, CommandPaletteState, ConfirmActionState, ConfirmPhase};
use crate::tui::format;
use crate::tui::theme::Theme;

pub fn render_command_palette(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &CommandPaletteState,
) {
    let width = 64u16.min(area.width.saturating_sub(4)).max(24);
    let height = (state.entries.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(6);
    let popup = centered_rect(area, width, height);

    let block = Block::default()
        .title(" Actions ")
        .borders(Borders::ALL)
        .border_type(theme.border_type())
        .border_style(theme.border_style())
        .title_style(theme.title_style());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let visible = inner.height.saturating_sub(1) as usize;
    let rows = state
        .entries
        .iter()
        .take(visible)
        .enumerate()
        .map(|(idx, entry)| {
            let selected = idx == state.selected_index;
            let style = if selected {
                theme.selected_style()
            } else {
                theme.normal_style()
            };
            Line::from(vec![
                Span::styled(
                    format!(" {} ", if selected { ">" } else { " " }),
                    theme.dim_style(),
                ),
                Span::styled(entry.label.clone(), style),
                Span::styled(format!("  {}", entry.hint), theme.dim_style()),
            ])
        })
        .collect::<Vec<_>>();

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    frame.render_widget(Paragraph::new(rows), chunks[0]);
    frame.render_widget(
        Paragraph::new(Line::from(format::help_spans(
            &[("j/k", "move"), ("Enter", "run"), ("Esc", "close")],
            theme,
        )))
        .alignment(Alignment::Center),
        chunks[1],
    );
}

pub fn render_confirm_action(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &ConfirmActionState,
    app: &App,
) {
    let width = 68u16.min(area.width.saturating_sub(4)).max(28);
    let popup = centered_rect(area, width, 12u16.min(area.height.saturating_sub(2)).max(8));
    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_type(theme.border_type())
        .border_style(theme.border_style())
        .title_style(theme.title_style());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let multisig = app
        .multisig_address
        .as_deref()
        .map(abbreviate_addr)
        .unwrap_or_else(|| "(none)".to_string());
    let signer = app
        .config
        .keypair
        .as_deref()
        .map(|s| {
            if s.starts_with("usb://") {
                "Ledger".to_string()
            } else {
                s.to_string()
            }
        })
        .or_else(|| app.ledger.as_ref().map(|_| "Ledger".to_string()))
        .unwrap_or_else(|| "(not configured)".to_string());

    let phase = match state.phase {
        ConfirmPhase::Review => "Review every field. Enter submits.",
        ConfirmPhase::Submitting => "Submitting. Keep this terminal open.",
        ConfirmPhase::Submitted => "Done. Enter refreshes the proposal.",
    };

    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {}", state.action.label()),
            theme.title_style(),
        )),
        Line::from(Span::styled(
            format!(" {}", state.action.summary()),
            theme.dim_style(),
        )),
        Line::from(""),
        kv("Proposal", &format!("#{}", state.proposal_index), theme),
        kv("Multisig", &multisig, theme),
        kv("Cluster", &cluster_label(&app.config.cluster), theme),
        kv("Signer", &signer, theme),
        Line::from(""),
        Line::from(Span::styled(format!(" {phase}"), theme.dim_style())),
    ];

    if let Some(ref msg) = state.message {
        let style = if state.message_is_error {
            theme.error_style()
        } else {
            theme.success_style()
        };
        lines.push(Line::from(Span::styled(format!(" {msg}"), style)));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .split(area);
    Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .split(vertical[1])[1]
}

fn kv<'a>(label: &'a str, value: &str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {label:<9}"), theme.dim_style()),
        Span::styled(value.to_string(), theme.normal_style()),
    ])
}

fn cluster_label(cluster: &str) -> String {
    if cluster.contains("mainnet") {
        "mainnet".to_string()
    } else if cluster.contains("devnet") {
        "devnet".to_string()
    } else if cluster.contains("testnet") {
        "testnet".to_string()
    } else {
        cluster.to_string()
    }
}
