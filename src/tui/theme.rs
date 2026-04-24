use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

/// TUI color theme — white-forward, minimal color.
pub struct Theme {
    pub fg: Color,
    pub fg_dim: Color,
    pub bg: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
}

impl Theme {
    /// Clean white theme — white text, minimal color accents.
    pub fn dark() -> Self {
        Self {
            fg: Color::White,
            fg_dim: Color::DarkGray,
            bg: Color::Reset,
            accent: Color::White, // was Cyan — now white
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
        }
    }

    pub fn title_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn normal_style(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.fg_dim).bg(self.bg)
    }

    pub fn selected_style(&self) -> Style {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_bar_style(&self) -> Style {
        Style::default().fg(self.fg_dim).bg(self.bg)
    }

    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error)
    }

    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success)
    }

    pub fn warning_style(&self) -> Style {
        Style::default().fg(self.warning)
    }

    pub fn border_style(&self) -> Style {
        Style::default().fg(self.fg_dim)
    }

    pub fn header_style(&self) -> Style {
        Style::default()
            .fg(self.fg)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    pub fn border_type(&self) -> BorderType {
        BorderType::Rounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_uses_reset_background() {
        let theme = Theme::dark();
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(theme.accent, Color::White);
    }

    #[test]
    fn border_type_is_rounded() {
        let theme = Theme::dark();
        assert_eq!(theme.border_type(), BorderType::Rounded);
    }
}
