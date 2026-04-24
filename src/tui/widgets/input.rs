use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::theme::Theme;

/// Render a single-line text input field with cursor and rounded borders.
pub fn render_input(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    title: &str,
    value: &str,
    cursor: usize,
    focused: bool,
) {
    let border_style = if focused {
        theme.title_style()
    } else {
        theme.border_style()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(theme.border_type())
        .border_style(border_style)
        .title_style(theme.title_style());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build text with cursor indicator
    let (before, after) = if cursor <= value.len() {
        (&value[..cursor], &value[cursor..])
    } else {
        (value, "")
    };

    let cursor_char = if after.is_empty() { " " } else { &after[..1] };
    let rest = if after.len() > 1 { &after[1..] } else { "" };

    let spans = if focused {
        vec![
            Span::styled(" ", theme.normal_style()),
            Span::styled(before, theme.normal_style()),
            Span::styled(cursor_char, theme.selected_style()),
            Span::styled(rest, theme.normal_style()),
        ]
    } else {
        vec![
            Span::styled(" ", theme.normal_style()),
            Span::styled(value, theme.dim_style()),
        ]
    };

    let text = Paragraph::new(Line::from(spans));
    frame.render_widget(text, inner);
}
