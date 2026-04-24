use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::app::SelectorState;
use crate::tui::format;
use crate::tui::theme::Theme;
use crate::tui::widgets::input::render_input;

/// Squads logo — octagon with rounded square cutout, rendered in white block chars.
const LOGO: &[&str] = &[
    "    ████████████    ",
    "  ██            ██  ",
    " █  ╭────────╮  █  ",
    "██  │        │  ██  ",
    "██  │        │  ██  ",
    "██  │        │  ██  ",
    " █  ╰────────╯  █  ",
    "  ██            ██  ",
    "    ████████████    ",
];

pub fn render_selector(frame: &mut Frame, area: Rect, theme: &Theme, state: &SelectorState) {
    if state.saved_multisigs.is_empty() {
        render_empty_selector(frame, area, theme, state);
    } else {
        render_list_selector(frame, area, theme, state);
    }
}

/// Clean centered view when no saved multisigs exist.
fn render_empty_selector(frame: &mut Frame, area: Rect, theme: &Theme, state: &SelectorState) {
    let logo_height = LOGO.len() as u16;

    // Center vertically
    let outer = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(logo_height + 8), // logo + gap + input + hint + error + help hint
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .split(area);

    // Center horizontally
    let content_width = 56u16.min(area.width.saturating_sub(4));
    let h_chunks = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(content_width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .split(outer[1]);

    let center = h_chunks[1];

    let inner = Layout::vertical([
        Constraint::Length(logo_height), // logo
        Constraint::Length(1),           // gap
        Constraint::Length(3),           // input
        Constraint::Length(1),           // hint
        Constraint::Length(1),           // error
        Constraint::Length(1),           // gap
        Constraint::Length(1),           // help hint
    ])
    .split(center);

    // Logo — white on default background
    let logo_style = Style::default().fg(Color::White);
    let logo_lines: Vec<Line> = LOGO
        .iter()
        .map(|line| Line::styled(*line, logo_style))
        .collect();
    frame.render_widget(
        Paragraph::new(logo_lines).alignment(Alignment::Center),
        inner[0],
    );

    // Input
    render_input(
        frame,
        inner[2],
        theme,
        " Multisig address ",
        &state.input,
        state.cursor,
        true,
    );

    // Hint
    frame.render_widget(
        Paragraph::new("Paste your Squads multisig address and press Enter")
            .style(theme.dim_style())
            .alignment(Alignment::Center),
        inner[3],
    );

    // Error
    if let Some(ref err) = state.error_msg {
        frame.render_widget(
            Paragraph::new(format!("\u{2717} {err}"))
                .style(theme.error_style())
                .alignment(Alignment::Center),
            inner[4],
        );
    }

    // Subtle help — dim, right-aligned feel but centered is fine
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" connect  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ]))
        .alignment(Alignment::Center),
        inner[6],
    );
}

/// List view when saved multisigs exist.
fn render_list_selector(frame: &mut Frame, area: Rect, theme: &Theme, state: &SelectorState) {
    let list_height = (state.saved_multisigs.len() as u16 + 2).min(area.height / 2);

    let chunks = Layout::vertical([
        Constraint::Length(1),           // title
        Constraint::Length(3),           // input
        Constraint::Length(1),           // error/gap
        Constraint::Length(list_height), // saved list (content-sized)
        Constraint::Min(0),              // spacer
        Constraint::Length(1),           // help
    ])
    .split(area);

    // Title
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" msig", theme.title_style()),
            Span::styled(" \u{2014} Select Multisig", theme.dim_style()),
        ])),
        chunks[0],
    );

    // Input
    render_input(
        frame,
        chunks[1],
        theme,
        " Address ",
        &state.input,
        state.cursor,
        true,
    );

    // Error
    if let Some(ref err) = state.error_msg {
        frame.render_widget(
            Paragraph::new(format!(" \u{2717} {err}")).style(theme.error_style()),
            chunks[2],
        );
    }

    // Saved multisigs — content-sized box
    let block = Block::default()
        .title(" Saved ")
        .borders(Borders::ALL)
        .border_type(theme.border_type())
        .border_style(theme.border_style())
        .title_style(theme.title_style());

    let inner = block.inner(chunks[3]);
    frame.render_widget(block, chunks[3]);

    let visible = inner.height as usize;
    let lines: Vec<Line> = state
        .saved_multisigs
        .iter()
        .take(visible)
        .enumerate()
        .map(|(i, (addr, label))| {
            let selected = i == state.selected_index;
            let indicator = if selected { "\u{25B8}" } else { " " };
            let style = if selected {
                theme.selected_style()
            } else {
                theme.normal_style()
            };
            let display = match label {
                Some(l) => format!(" {indicator} {l} ({})", format::short_addr(addr)),
                None => format!(" {indicator} {}", format::short_addr(addr)),
            };
            Line::styled(display, style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);

    // Help — subtle
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("j/k", Style::default().fg(Color::DarkGray)),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ])),
        chunks[5],
    );
}
