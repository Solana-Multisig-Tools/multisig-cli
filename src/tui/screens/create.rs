use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::app::{CreatePhase, CreateState};
use crate::tui::format;
use crate::tui::theme::Theme;
use crate::tui::widgets::input::render_input;

pub fn render_create(frame: &mut Frame, area: Rect, theme: &Theme, state: &CreateState) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(4), // recipient
        Constraint::Length(4), // amount
        Constraint::Length(5), // review/status
        Constraint::Min(0),    // spacer
        Constraint::Length(1), // help
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Create Transaction",
            theme.title_style(),
        ))),
        chunks[0],
    );

    render_labeled_input(
        frame,
        chunks[1],
        theme,
        state,
        0,
        "Recipient",
        "destination wallet address",
        &state.recipient,
    );
    render_labeled_input(
        frame,
        chunks[2],
        theme,
        state,
        1,
        "Amount SOL",
        "integer-safe decimal amount",
        &state.amount_sol,
    );

    render_review(frame, chunks[3], theme, state);

    frame.render_widget(
        Paragraph::new(Line::from(format::help_spans(
            &[
                ("Tab", "field"),
                ("Enter", "review/submit"),
                ("e", "edit"),
                ("Esc", "back"),
                ("q", "quit"),
            ],
            theme,
        ))),
        chunks[5],
    );
}

#[allow(clippy::too_many_arguments)]
fn render_labeled_input(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    state: &CreateState,
    index: usize,
    label: &str,
    hint: &str,
    value: &str,
) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).split(area);
    let is_active = state.phase == CreatePhase::Editing && state.active_field == index;
    let indicator = if is_active { "\u{25B8}" } else { " " };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {indicator} "), theme.title_style()),
            Span::styled(format!("{label} "), theme.normal_style()),
            Span::styled(hint, theme.dim_style()),
        ])),
        chunks[0],
    );
    let cursor = if is_active { state.cursor } else { 0 };
    render_input(
        frame,
        chunks[1],
        theme,
        &format!(" {label} "),
        value,
        cursor,
        is_active,
    );
}

fn render_review(frame: &mut Frame, area: Rect, theme: &Theme, state: &CreateState) {
    let block = Block::default()
        .title(" Review ")
        .borders(Borders::ALL)
        .border_type(theme.border_type())
        .border_style(theme.border_style())
        .title_style(theme.title_style());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let phase = match state.phase {
        CreatePhase::Editing => "Fill recipient and amount, then press Enter.",
        CreatePhase::Review => "Press Enter to submit this SOL transfer proposal.",
        CreatePhase::Submitting => "Submitting. Keep this terminal open.",
        CreatePhase::Submitted => "Done. Press e to edit another transfer or Esc to go back.",
    };
    let mut lines = vec![
        Line::from(Span::styled(format!(" {phase}"), theme.dim_style())),
        Line::from(vec![
            Span::styled(" Recipient ", theme.dim_style()),
            Span::styled(
                format::short_addr(state.recipient.trim()),
                theme.normal_style(),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Amount    ", theme.dim_style()),
            Span::styled(
                format!("{} SOL", state.amount_sol.trim()),
                theme.normal_style(),
            ),
        ]),
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
