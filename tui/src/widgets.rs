//! Small drawing helpers, from Code Keepsake's TUI (github.com/phbronson999/Code-Keepsake,
//! MIT). `stacked_bar` takes plain segments instead of Keepsake's size breakdown.

use ratatui::{
    layout::Rect,
    style::Color,
    text::{Line, Span},
};

use crate::theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick as usize) % SPINNER.len()]
}

/// Splits `width` cells between `values` by largest remainder. Any non-zero
/// value gets at least one cell when there's room, so small slices stay visible.
pub fn allocate(values: &[u64], width: usize) -> Vec<usize> {
    let total: u64 = values.iter().sum();
    if total == 0 || width == 0 {
        return vec![0; values.len()];
    }
    let mut cells = Vec::with_capacity(values.len());
    let mut remainders = Vec::with_capacity(values.len());
    for (i, &v) in values.iter().enumerate() {
        let exact = v as f64 / total as f64 * width as f64;
        cells.push(exact.floor() as usize);
        remainders.push((i, exact - exact.floor()));
    }
    let mut left = width - cells.iter().sum::<usize>();
    remainders.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (i, _) in remainders {
        if left == 0 {
            break;
        }
        cells[i] += 1;
        left -= 1;
    }
    for i in 0..values.len() {
        if values[i] > 0 && cells[i] == 0 {
            if let Some(big) = (0..cells.len()).filter(|&j| cells[j] > 1).max_by_key(|&j| cells[j]) {
                cells[big] -= 1;
                cells[i] = 1;
            }
        }
    }
    cells
}

/// A bar split into colored segments in proportion to their values.
#[allow(dead_code)]
pub fn stacked_bar(segments: &[(u64, Color)], width: usize, glyph: &str) -> Vec<Span<'static>> {
    let values: Vec<u64> = segments.iter().map(|s| s.0).collect();
    if values.iter().sum::<u64>() == 0 {
        return vec![Span::styled("─".repeat(width), theme::faint())];
    }
    allocate(&values, width)
        .into_iter()
        .zip(segments)
        .filter(|(n, _)| *n > 0)
        .map(|(n, (_, color))| Span::styled(glyph.repeat(n), theme::fg(*color)))
        .collect()
}

/// A filled/unfilled bar for a ratio between 0 and 1.
pub fn ratio_bar(ratio: f64, width: usize, color: Color) -> Vec<Span<'static>> {
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    vec![
        Span::styled("━".repeat(filled), theme::fg(color)),
        Span::styled("━".repeat(width - filled), theme::fg(theme::BORDER)),
    ]
}

/// ` key ` styled as a keycap, followed by a label.
pub fn keycap(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!(" {key} "), theme::bold(theme::ORANGE)),
        Span::styled(format!(" {label}  "), theme::text()),
    ]
}

/// A rectangle of `width` x `height` centered in `area`.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A section heading like `WATCH ─────── 9.1 GB`.
pub fn section(title: &str, right: Option<String>, width: usize) -> Line<'static> {
    let right = right.unwrap_or_default();
    let used = title.chars().count() + right.chars().count() + 2;
    let rule = width.saturating_sub(used).max(1);
    Line::from(vec![
        Span::styled(title.to_string(), theme::label()),
        Span::styled(format!(" {} ", "─".repeat(rule)), theme::fg(theme::BORDER)),
        Span::styled(right, theme::muted()),
    ])
}

#[cfg(test)]
mod tests {
    use super::allocate;

    #[test]
    fn allocates_exactly_and_keeps_small_slices() {
        let cells = allocate(&[900, 1, 99, 0], 20);
        assert_eq!(cells.iter().sum::<usize>(), 20);
        assert_eq!(cells[1], 1);
        assert_eq!(cells[3], 0);
    }
}
