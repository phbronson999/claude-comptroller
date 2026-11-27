//! A neutral-gray palette with cyan, orange and lime accents. Panels are filled
//! with a slightly lighter gray than the terminal background and selections are
//! a deep blue. The terminal background itself is left alone.

use ratatui::style::{Color, Modifier, Style};

pub const TEXT: Color = Color::Rgb(236, 236, 236);
pub const MUTED: Color = Color::Rgb(172, 172, 172);
pub const FAINT: Color = Color::Rgb(122, 122, 122);
pub const BORDER: Color = Color::Rgb(58, 58, 58);
pub const BORDER_FOCUS: Color = Color::Rgb(86, 196, 238);
pub const PANEL: Color = Color::Rgb(32, 32, 32);
pub const SELECTION: Color = Color::Rgb(16, 66, 108);

pub const ACCENT: Color = Color::Rgb(86, 196, 238);
pub const ORANGE: Color = Color::Rgb(255, 152, 40);
pub const PINK: Color = Color::Rgb(255, 72, 136);

pub const KEPT: Color = ACCENT;
pub const OK: Color = Color::Rgb(140, 230, 30);
pub const WARN: Color = ORANGE;
pub const BAD: Color = PINK;

pub const BUILD: Color = ORANGE;
pub const GIT: Color = PINK;
pub const SOURCE: Color = ACCENT;
pub const OTHER: Color = Color::Rgb(122, 122, 122);

pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

pub fn text() -> Style {
    fg(TEXT)
}

pub fn muted() -> Style {
    fg(MUTED)
}

pub fn faint() -> Style {
    fg(FAINT)
}

pub fn label() -> Style {
    bold(FAINT)
}
