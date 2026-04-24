use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use ratatui::Frame;

use crate::domain::multisig::MultisigInfo;
use crate::domain::proposal::ProposalSummary;
use crate::tui::app::{DashboardState, Loadable};
use crate::tui::format;
use crate::tui::theme::Theme;

pub fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &DashboardState,
    multisig_addr: Option<&str>,
) {
    // Compute content-sized heights
    let info_height = match &state.multisig_info {
        Loadable::Loaded(info) => (7 + info.members.len() as u16).min(18),
        _ => 2,
    };

    let proposals_height = match &state.proposals {
        Loadable::Loaded(proposals) if !proposals.is_empty() => {
            (proposals.len() as u16 + 2).min(12)
        }
        Loadable::Loaded(_) => 0, // empty: don't show the box at all
        _ => 2,
    };

    let mut constraints = vec![
        Constraint::Length(1),           // header
        Constraint::Length(info_height), // multisig info
    ];

    if proposals_height > 0 {
        constraints.push(Constraint::Length(proposals_height));
    }

    constraints.push(Constraint::Min(0)); // spacer
    constraints.push(Constraint::Length(1)); // help

    let chunks = Layout::vertical(constraints).split(area);

    // Header
    let addr_display = multisig_addr
        .map(format::short_addr)
        .unwrap_or_else(|| "(none)".to_string());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Dashboard", theme.title_style()),
            Span::styled(" ", theme.dim_style()),
            Span::styled(addr_display, theme.dim_style()),
        ])),
        chunks[0],
    );

    // Multisig info
    render_multisig_info(frame, chunks[1], theme, &state.multisig_info);

    // Proposals — only if there's something to show
    if proposals_height > 0 {
        render_recent_proposals(frame, chunks[2], theme, &state.proposals);
    }

    // Help — subtle at bottom
    let help_idx = chunks.len() - 1;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" p", Style::default().fg(Color::DarkGray)),
            Span::styled(" proposals  ", Style::default().fg(Color::DarkGray)),
            Span::styled("space", Style::default().fg(Color::DarkGray)),
            Span::styled(" actions  ", Style::default().fg(Color::DarkGray)),
            Span::styled("r", Style::default().fg(Color::DarkGray)),
            Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)),
            Span::styled("s", Style::default().fg(Color::DarkGray)),
            Span::styled(" switch  ", Style::default().fg(Color::DarkGray)),
            Span::styled("c", Style::default().fg(Color::DarkGray)),
            Span::styled(" create  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::DarkGray)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ])),
        chunks[help_idx],
    );
}

fn render_multisig_info(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    info: &Loadable<MultisigInfo>,
) {
    match info {
        Loadable::Idle | Loadable::Loading => {
            frame.render_widget(
                Paragraph::new(format!(" {} Loading...", format::spinner_frame(0)))
                    .style(theme.dim_style()),
                area,
            );
        }
        Loadable::Failed(msg) => {
            frame.render_widget(
                Paragraph::new(format!(" \u{2717} {msg}")).style(theme.error_style()),
                area,
            );
        }
        Loadable::Loaded(info) => {
            let bal = format::lamports_to_sol(info.vault_balance_lamports);
            let vault = format::short_addr(&info.vault_address.to_string());
            let mc = info.members.len();

            let mut lines = vec![
                kv("Threshold", &format!("{}/{mc}", info.threshold), theme),
                kv("Vault", &vault, theme),
                kv("Balance", &format!("{bal} SOL"), theme),
                kv("Tx Index", &info.transaction_index.to_string(), theme),
                kv("Time Lock", &format!("{}s", info.time_lock), theme),
                Line::from(""),
                Line::from(Span::styled(" Members", theme.title_style())),
            ];

            for m in &info.members {
                let addr = m.key.to_string();
                let perms = m.permissions.labels().join(", ");
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("   {} ", format::short_addr(&addr)),
                        theme.normal_style(),
                    ),
                    Span::styled(perms, theme.dim_style()),
                ]));
            }

            frame.render_widget(Paragraph::new(lines), area);
        }
    }
}

fn render_recent_proposals(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    proposals: &Loadable<Vec<ProposalSummary>>,
) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Recent proposals",
            theme.title_style(),
        ))),
        chunks[0],
    );
    let inner = chunks[1];

    match proposals {
        Loadable::Idle | Loadable::Loading => {
            frame.render_widget(
                Paragraph::new(format!(" {} Loading...", format::spinner_frame(0)))
                    .style(theme.dim_style()),
                inner,
            );
        }
        Loadable::Failed(msg) => {
            frame.render_widget(
                Paragraph::new(format!(" \u{2717} {msg}")).style(theme.error_style()),
                inner,
            );
        }
        Loadable::Loaded(proposals) => {
            if proposals.is_empty() {
                return; // caller already handles this by not showing the box
            }

            let header = Row::new(vec!["#", "Status", "Votes", "Address"])
                .style(theme.header_style())
                .height(1);

            let visible = (inner.height as usize).saturating_sub(1);
            let rows: Vec<Row> = proposals
                .iter()
                .take(visible)
                .map(|p| {
                    let (status_text, _) = format::status_display(p.status.label());
                    let bar = vote_bar_str(p.approved_count, p.threshold);
                    let addr = format::short_addr(&p.address.to_string());
                    Row::new(vec![
                        format!("{}", p.index),
                        status_text.to_string(),
                        bar,
                        addr,
                    ])
                    .style(theme.normal_style())
                    .height(1)
                })
                .collect();

            let widths = [
                Constraint::Length(5),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Min(11),
            ];

            frame.render_stateful_widget(
                Table::new(rows, widths).header(header).column_spacing(1),
                inner,
                &mut ratatui::widgets::TableState::default(),
            );
        }
    }
}

fn kv<'a>(label: &'a str, value: &str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {label:<10}"), theme.dim_style()),
        Span::styled(value.to_string(), theme.normal_style()),
    ])
}

fn vote_bar_str(approved: usize, threshold: u16) -> String {
    let w = 5usize;
    let t = threshold as usize;
    let filled = approved
        .checked_mul(w)
        .and_then(|value| value.checked_div(t))
        .unwrap_or(0)
        .min(w);
    let empty = w.saturating_sub(filled);
    format!(
        "{}{} {}/{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty),
        approved,
        threshold
    )
}
