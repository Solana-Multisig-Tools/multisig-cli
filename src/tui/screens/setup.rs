use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::SetupState;
use crate::tui::format;
use crate::tui::theme::Theme;
use crate::tui::widgets::input::render_input;

const FIELD_LABELS: &[&str] = &["RPC Cluster", "Keypair Path", "Multisig Address"];

const FIELD_HINTS: &[&str] = &[
    "mainnet, devnet, or full URL",
    "path to keypair JSON, or usb://ledger",
    "your Squads multisig address",
];

pub fn render_setup(frame: &mut Frame, area: Rect, theme: &Theme, state: &SetupState) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Length(4), // field 0
        Constraint::Length(4), // field 1
        Constraint::Length(4), // field 2
        Constraint::Length(1), // status
        Constraint::Min(0),    // spacer
        Constraint::Length(1), // help
    ])
    .split(area);

    // Header — tight
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" msig", theme.title_style()),
                Span::styled(" \u{2014} Setup", theme.dim_style()),
            ]),
            Line::from(""),
            Line::styled(
                " Configure defaults. Tab between fields, Enter to save.",
                theme.dim_style(),
            ),
        ]),
        chunks[0],
    );

    // Fields
    let fields = [&state.cluster, &state.keypair, &state.multisig];
    for (i, field_value) in fields.iter().enumerate() {
        let is_active = state.active_field == i;
        let field_area = chunks[1 + i];

        let inner = Layout::vertical([
            Constraint::Length(1), // label
            Constraint::Length(3), // input
        ])
        .split(field_area);

        let step = format!("{}/{}", i + 1, fields.len());
        let indicator = if is_active { "\u{25B8}" } else { " " };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {indicator} "),
                    if is_active {
                        theme.title_style()
                    } else {
                        theme.dim_style()
                    },
                ),
                Span::styled(
                    format!("{} ", FIELD_LABELS[i]),
                    if is_active {
                        theme.title_style()
                    } else {
                        theme.normal_style()
                    },
                ),
                Span::styled(format!("({step}) "), theme.dim_style()),
                Span::styled(FIELD_HINTS[i], theme.dim_style()),
            ])),
            inner[0],
        );

        let cursor = if is_active { state.cursor } else { 0 };
        render_input(
            frame,
            inner[1],
            theme,
            &format!(" {} ", FIELD_LABELS[i]),
            field_value,
            cursor,
            is_active,
        );
    }

    // Status
    if let Some(ref msg) = state.message {
        let (icon, style) = if state.message_is_error {
            ("\u{2717}", theme.error_style())
        } else {
            ("\u{2713}", theme.success_style())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {icon} "), style),
                Span::styled(msg.as_str(), style),
            ])),
            chunks[4],
        );
    }

    // Help
    frame.render_widget(
        Paragraph::new(Line::from(format::help_spans(
            &[("Tab", "next"), ("Enter", "save"), ("Esc", "skip")],
            theme,
        ))),
        chunks[6],
    );
}
