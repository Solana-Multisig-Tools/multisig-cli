use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::domain::proposal::{ProposalDetail, ProposalSummary};
use crate::output::format_addr;
use crate::tui::app::{Loadable, ProposalDetailState, ProposalsState};
use crate::tui::format;
use crate::tui::theme::Theme;

pub fn render_proposals(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &ProposalsState,
    truncate: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // table (fills)
        Constraint::Length(1), // help
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(" Proposals", theme.title_style()))),
        chunks[0],
    );

    let inner = chunks[1];

    match &state.proposals {
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
                frame.render_widget(
                    Paragraph::new(" No proposals.").style(theme.dim_style()),
                    inner,
                );
            } else {
                render_table(
                    frame,
                    inner,
                    theme,
                    proposals,
                    state.selected_index,
                    state.scroll_offset,
                    truncate,
                );
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(format::help_spans(
            &[
                ("Space", "actions"),
                ("j/k", "nav"),
                ("Enter", "detail"),
                ("r", "refresh"),
                ("Esc", "back"),
                ("q", "quit"),
            ],
            theme,
        ))),
        chunks[2],
    );
}

fn render_table(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    proposals: &[ProposalSummary],
    selected: usize,
    scroll_offset: usize,
    truncate: bool,
) {
    let header = Row::new(vec!["#", "Status", "Votes", "Rejected", "Address"])
        .style(theme.header_style())
        .height(1);

    let visible = (area.height as usize).saturating_sub(1);
    let end = (scroll_offset + visible).min(proposals.len());

    let rows: Vec<Row> = proposals[scroll_offset..end]
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let abs = scroll_offset + i;
            let (status_text, _) = format::status_display(p.status.label());
            let bar = vote_bar_str(p.approved_count, p.threshold);
            let addr = format_addr(&p.address.to_string(), truncate);

            let style = if abs == selected {
                theme.selected_style()
            } else {
                theme.normal_style()
            };

            Row::new(vec![
                format!("{}", p.index),
                status_text.to_string(),
                bar,
                format!("{}", p.rejected_count),
                addr,
            ])
            .style(style)
            .height(1)
        })
        .collect();

    let widths = [
        Constraint::Length(5),
        Constraint::Length(14),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Min(11),
    ];

    let mut table_state = TableState::default();
    if selected >= scroll_offset && selected < end {
        table_state.select(Some(selected - scroll_offset));
    }

    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .row_highlight_style(theme.selected_style()),
        area,
        &mut table_state,
    );
}

pub fn render_proposal_detail(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &ProposalDetailState,
    truncate: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // detail (fills)
        Constraint::Length(1), // help
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" Proposal #{}", state.index),
            theme.title_style(),
        ))),
        chunks[0],
    );

    let inner = chunks[1];

    match &state.detail {
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
        Loadable::Loaded(detail) => {
            render_detail(frame, inner, theme, detail, state.scroll_offset, truncate);
            if let Some(ref msg) = state.action_message {
                let style = if state.action_message_is_error {
                    theme.error_style()
                } else {
                    theme.success_style()
                };
                let msg_area = Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                };
                frame.render_widget(Paragraph::new(format!(" {msg}")).style(style), msg_area);
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(format::help_spans(
            &[
                ("Space", "actions"),
                ("j/k", "scroll"),
                ("Esc", "back"),
                ("q", "quit"),
            ],
            theme,
        ))),
        chunks[2],
    );
}

fn render_detail(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    detail: &ProposalDetail,
    scroll_offset: usize,
    truncate: bool,
) {
    let s = &detail.summary;
    let (status_text, _) = format::status_display(s.status.label());
    let addr = format_addr(&s.address.to_string(), truncate);
    let ms = format_addr(&detail.multisig.to_string(), truncate);

    let mut lines = vec![
        kv("Status", status_text, theme),
        kv("Address", &addr, theme),
        kv("Multisig", &ms, theme),
        kv(
            "Votes",
            &format!("{}/{}", s.approved_count, s.threshold),
            theme,
        ),
        kv("Type", detail.transaction_type.label(), theme),
    ];

    // Approved
    lines.push(Line::from(Span::styled(" Approved", theme.title_style())));
    if detail.approved.is_empty() {
        lines.push(Line::from(Span::styled("   (none)", theme.dim_style())));
    } else {
        for pk in &detail.approved {
            lines.push(Line::from(Span::styled(
                format!("   \u{2713} {}", format_addr(&pk.to_string(), truncate)),
                theme.success_style(),
            )));
        }
    }

    // Rejected
    lines.push(Line::from(Span::styled(" Rejected", theme.title_style())));
    if detail.rejected.is_empty() {
        lines.push(Line::from(Span::styled("   (none)", theme.dim_style())));
    } else {
        for pk in &detail.rejected {
            lines.push(Line::from(Span::styled(
                format!("   \u{2717} {}", format_addr(&pk.to_string(), truncate)),
                theme.error_style(),
            )));
        }
    }

    // Cancelled
    if !detail.cancelled.is_empty() {
        lines.push(Line::from(Span::styled(" Cancelled", theme.title_style())));
        for pk in &detail.cancelled {
            lines.push(Line::from(Span::styled(
                format!("   \u{2298} {}", format_addr(&pk.to_string(), truncate)),
                theme.warning_style(),
            )));
        }
    }

    // Instructions
    if let Some(ref vtx) = detail.vault_tx {
        lines.push(Line::from(Span::styled(
            format!(" Instructions ({})", vtx.instruction_count),
            theme.title_style(),
        )));
        for (i, ix) in vtx.instructions.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!("   [{i}] "), theme.dim_style()),
                Span::styled(&ix.program_name, theme.title_style()),
                Span::styled(
                    format!(" {}", format_addr(&ix.program_id.to_string(), truncate)),
                    theme.dim_style(),
                ),
            ]));
        }
    }

    if let Some(ref ctx) = detail.config_tx {
        lines.push(Line::from(Span::styled(
            format!(" Config actions: {}", ctx.action_count),
            theme.title_style(),
        )));
    }

    // Apply scroll
    let h = area.height as usize;
    let off = scroll_offset.min(lines.len().saturating_sub(1));
    let end = (off + h).min(lines.len());
    frame.render_widget(Paragraph::new(lines[off..end].to_vec()), area);
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
