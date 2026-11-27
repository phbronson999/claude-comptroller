//! The radar scope on the Terminal screen: the logo, and a glance at the service's state.
//! The beam sweeps while the service runs, sweeps orange while it changes, and stops when it
//! stops. Each computer that sent a request in the last 10 minutes is a blip, in the same
//! place every time; orange if its person hit a plan limit today. Blips light up as the beam
//! passes and fade until it comes round again.

use std::f64::consts::PI;

use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::Color,
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Circle, Line as Segment, Points},
        Paragraph,
    },
    Frame,
};

use crate::theme;

/// Rows the logo takes at the top of the right-hand column.
pub const ROWS: u16 = 9;
/// The narrowest column it fits: the scope plus the name beside it.
pub const MIN_WIDTH: u16 = 42;
const SCOPE_WIDTH: u16 = 20;
/// "controls the traffic", the longest line beside it.
const TEXT_WIDTH: u16 = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Beam {
    /// Running: a cyan sweep.
    Sweeping,
    /// Starting, stopping or installing: an orange sweep.
    Changing,
    /// Stopped: the beam is still.
    Stopped,
    /// Not installed: an empty scope.
    Off,
}

/// A computer on the scope: a stable key for its place, and whether it hit a plan limit.
pub struct Blip {
    pub key: String,
    pub limit_hit: bool,
}

/// Where a blip sits: the same angle and distance for the same computer, every time.
fn place(key: &str) -> (f64, f64) {
    // FNV-1a: stable across runs and builds, unlike the standard hasher.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    let angle = (hash % 3600) as f64 / 3600.0 * 2.0 * PI;
    let distance = 0.35 + ((hash >> 16) % 50) as f64 / 100.0;
    (angle, distance)
}

pub fn draw(f: &mut Frame, area: Rect, beam: Beam, tick: u64, blips: &[Blip]) {
    // The scope and the name sit together, centred in the space above "At a glance".
    let [scope, text] = Layout::horizontal([Constraint::Length(SCOPE_WIDTH), Constraint::Length(TEXT_WIDTH)])
        .spacing(2)
        .flex(Flex::Center)
        .areas(area);
    draw_scope(f, scope, beam, tick, blips);

    let lines = vec![
        Line::from(Span::styled("claude-comptroller", theme::bold(theme::ACCENT))),
        Line::from(Span::styled("controls the traffic", theme::muted())),
        Line::from(Span::styled("keeps the books", theme::muted())),
    ];
    let pad = text.height.saturating_sub(lines.len() as u16) / 2;
    let [_, text] = Layout::vertical([Constraint::Length(pad), Constraint::Fill(1)]).areas(text);
    f.render_widget(Paragraph::new(lines), text);
}

fn mix(a: Color, b: Color, t: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else { return b };
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
}

fn draw_scope(f: &mut Frame, area: Rect, beam: Beam, tick: u64, blips: &[Blip]) {
    let blips: Vec<(f64, f64, Color)> = blips
        .iter()
        .map(|b| {
            let (angle, distance) = place(&b.key);
            (angle, distance, if b.limit_hit { theme::ORANGE } else { theme::OK })
        })
        .collect();
    // Braille cells are 2 × 4 dots, and a terminal cell is about twice as tall as wide,
    // so these bounds make one unit one dot in both directions and the circle round.
    let (w, h) = (area.width as f64, area.height as f64);
    let r = w.min(2.0 * h) - 2.0;
    let angle = match beam {
        Beam::Sweeping | Beam::Changing => -(tick as f64) * 0.07,
        Beam::Stopped | Beam::Off => PI / 4.0,
    };
    let (head, tail) = match beam {
        Beam::Sweeping => (theme::ACCENT, Color::Rgb(20, 50, 64)),
        Beam::Changing => (theme::ORANGE, Color::Rgb(64, 40, 12)),
        Beam::Stopped => (theme::PINK, Color::Rgb(64, 20, 36)),
        Beam::Off => (theme::FAINT, theme::FAINT),
    };
    let ring = Color::Rgb(62, 62, 62);

    let canvas = Canvas::default().marker(Marker::Braille).x_bounds([-w, w]).y_bounds([-2.0 * h, 2.0 * h]).paint(move |ctx| {
        for k in [0.33, 0.66, 1.0] {
            let color = if k == 1.0 { Color::Rgb(80, 80, 80) } else { ring };
            ctx.draw(&Circle { x: 0.0, y: 0.0, radius: r * k, color });
        }
        ctx.draw(&Segment { x1: -r, y1: 0.0, x2: r, y2: 0.0, color: ring });
        ctx.draw(&Segment { x1: 0.0, y1: -r, x2: 0.0, y2: r, color: ring });
        if beam == Beam::Off {
            return;
        }
        // A fading trail behind the beam while it moves; a single line when it's still.
        let trail = if beam == Beam::Stopped { 0 } else { 7 };
        for k in (0..=trail).rev() {
            let a = angle + k as f64 * 0.06;
            let color = mix(head, tail, k as f64 / 8.0);
            ctx.draw(&Segment { x1: 0.0, y1: 0.0, x2: r * a.cos(), y2: r * a.sin(), color });
        }
        // Brightest just after the beam passes. With the beam still, they stay dim.
        for (at, distance, color) in &blips {
            let since = if beam == Beam::Stopped { PI } else { (at - angle).rem_euclid(2.0 * PI) };
            let glow = (1.0 - since / (2.0 * PI)).powf(2.5).max(0.18);
            let (x, y) = (r * distance * at.cos(), r * distance * at.sin());
            let color = mix(theme::PANEL, *color, glow);
            ctx.draw(&Points { coords: &[(x, y), (x + 1.0, y), (x, y + 1.0), (x + 1.0, y + 1.0)], color });
        }
    });
    f.render_widget(canvas, area);
}
