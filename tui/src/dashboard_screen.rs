//! The Dashboard: the day (or the last 7 or 14 days) at a glance. The same four cards as
//! Radar, requests per hour or per day, each person's share with their plan-limit hits and
//! latency, and the latest requests. p steps through today, 7 and 14 days; r opens Radar.

use comptroller_admin::traffic::Summary;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Color,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app::{App, Screen};
use crate::radar_screen::{color_of, draw_figures, seconds, short_name, Figures};
use crate::theme;
use crate::ui::{panel, truncate};
use crate::widgets::{section, stacked_bar};

pub fn on_key(app: &mut App, key: KeyEvent) {
    let days = match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Terminal;
            return;
        }
        KeyCode::Char('r') => {
            app.screen = Screen::Radar;
            return;
        }
        KeyCode::Char('1') => 1,
        KeyCode::Char('7') => 7,
        // p steps through today, 7 days and 14 days.
        KeyCode::Char('p') | KeyCode::Tab => match app.params.days {
            1 => 7,
            7 => 14,
            _ => 1,
        },
        _ => return,
    };
    if app.params.days != days {
        app.params.days = days;
        app.send_params();
    }
}

fn summary(app: &App) -> Option<&Summary> {
    match &app.snapshot.summary {
        Some(Ok(Some(s))) => Some(s),
        _ => None,
    }
}

/// Everyone in the period, in the order Radar uses, so colors match.
fn people(app: &App) -> Vec<String> {
    let mut people: Vec<String> = summary(app).map(|s| s.people.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
    people.sort_by_key(|p| p.to_lowercase());
    people
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [cards, rest] = Layout::vertical([Constraint::Length(5), Constraint::Fill(1)]).spacing(1).areas(body);
    let [tower, side] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).spacing(1).areas(rest);
    let people = people(app);
    let s = summary(app);

    let figures = s.map(|s| Figures {
        period: if s.days == 1 { "TODAY".into() } else { format!("· {} DAYS", s.days) },
        requests: s.requests,
        p95_ms: s.p95_ms,
        limit_hits: s.limit_hits,
        limit_hits_by_user: s.people.iter().filter(|p| p.limit_hits > 0).map(|p| (p.name.clone(), p.limit_hits)).collect(),
        errors: s.errors,
    });
    draw_figures(f, app, cards, figures.as_ref(), &people);
    draw_tower(f, app, tower, &people);
    draw_people(f, app, side, &people);
}

/// The comparison bars: the period before.
const EARLIER: Color = Color::Rgb(72, 72, 72);

/// Cells per slot: the comparison bar, the current bar one cell to its right overlapping it
/// (as in an IBCS chart), and a gap.
fn slot_width(bar: usize) -> usize {
    bar + 2
}

/// Overlapping vertical bars `height` rows tall, in eighths of a row: `current` in front, and
/// `previous` behind it, one cell to the left.
fn bars(current: &[i64], previous: &[i64], height: usize, bar: usize) -> Vec<Line<'static>> {
    const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let max = current.iter().chain(previous).copied().max().unwrap_or(0).max(1) as f64;
    let level = |v: i64| ((v as f64 / max) * (height * 8) as f64).round() as usize;
    // A slot with requests always shows at least a sliver.
    let part = |level: usize, row: usize, value: i64| {
        let p = level.saturating_sub(row * 8).min(8);
        if row == 0 && p == 0 && value > 0 { 1 } else { p }
    };
    (0..height)
        .rev()
        .map(|row| {
            let mut spans = Vec::new();
            for (i, &now) in current.iter().enumerate() {
                let before = previous.get(i).copied().unwrap_or(0);
                let (a, c) = (part(level(now), row, now), part(level(before), row, before));
                for col in 0..slot_width(bar) {
                    let in_current = (1..=bar).contains(&col);
                    let in_earlier = col < bar;
                    let span = if in_current && a > 0 {
                        // The current bar in front; the earlier one shows above it in the same cell.
                        let style = if in_earlier && c == 8 && a < 8 { theme::fg(theme::ACCENT).bg(EARLIER) } else { theme::fg(theme::ACCENT) };
                        Span::styled(EIGHTHS[a], style)
                    } else if in_earlier && c > 0 {
                        Span::styled(EIGHTHS[c], theme::fg(EARLIER))
                    } else {
                        Span::raw(" ")
                    };
                    spans.push(span);
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn draw_tower(f: &mut Frame, app: &App, area: Rect, people: &[String]) {
    let block = panel("Tower", true);
    let inner = block.inner(area);
    let width = inner.width as usize;
    f.render_widget(block, area);
    let Some(s) = summary(app) else {
        let text = match &app.snapshot.summary {
            Some(Err(e)) => e.clone(),
            Some(Ok(None)) => "No database yet. The service creates it when it first starts.".into(),
            _ => "Reading…".into(),
        };
        f.render_widget(Paragraph::new(Span::styled(text, theme::muted())), inner);
        return;
    };

    let [chart, recent] = Layout::vertical([Constraint::Percentage(60), Constraint::Fill(1)]).spacing(1).areas(inner);
    let per = if s.days == 1 { "HOUR" } else { "DAY" };
    let (now_label, before_label) = if s.days == 1 { ("today".to_string(), "yesterday".to_string()) } else { (format!("last {} days", s.days), format!("the {} before", s.days)) };
    let legend = format!("■ {now_label}  ■ {before_label}");
    let mut heading = section(&format!("REQUESTS / {per}"), None, width.saturating_sub(legend.chars().count() + 1));
    heading.spans.push(Span::styled("■ ", theme::fg(theme::ACCENT)));
    heading.spans.push(Span::styled(format!("{now_label}  "), theme::muted()));
    heading.spans.push(Span::styled("■ ", theme::fg(EARLIER)));
    heading.spans.push(Span::styled(before_label, theme::muted()));
    let mut lines = vec![heading];

    // For one day: the hours with traffic today or yesterday, and the hour now.
    let range = if s.days == 1 {
        let now_hour = ((comptroller_admin::health::now() + comptroller_admin::local_offset()).rem_euclid(86_400) / 3600) as usize;
        let busy = |i: usize| s.buckets[i].1 > 0 || s.previous.get(i).is_some_and(|&n| n > 0);
        let first = (0..24).find(|&i| busy(i)).unwrap_or(now_hour).min(now_hour);
        let last = (0..24).rev().find(|&i| busy(i)).unwrap_or(now_hour).max(now_hour);
        first..last + 1
    } else {
        0..s.buckets.len()
    };
    let values = &s.buckets[range.clone()];
    let previous = s.previous.get(range).unwrap_or_default();
    let bar = (width / values.len().max(1)).saturating_sub(2).clamp(1, 4);
    let height = (chart.height as usize).saturating_sub(3).max(1);
    let counts: Vec<i64> = values.iter().map(|v| v.1).collect();
    lines.extend(bars(&counts, previous, height, bar));
    let labels: String = values
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            // Under the current bar; every label when there's room, else every other one.
            let show = bar >= 2 || i % 2 == 0;
            let text = if show { label.as_str() } else { "" };
            format!(" {text:<0$}", slot_width(bar) - 1)
        })
        .collect();
    lines.push(Line::from(Span::styled(labels, theme::faint())));
    f.render_widget(Paragraph::new(lines), chart);

    // The latest requests
    let mut lines = vec![section("RECENT", Some("r opens Radar".into()), width)];
    let rows = match &app.snapshot.feed {
        Some(Ok(Some(rows))) => rows.clone(),
        _ => Vec::new(),
    };
    for r in rows.iter().take((recent.height as usize).saturating_sub(1)) {
        let who = r.user.as_deref().map(short_name).unwrap_or("—");
        let color = r.user.as_deref().map(|u| color_of(u, people)).unwrap_or(theme::FAINT);
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", comptroller_admin::local_clock(r.started)), theme::text()),
            Span::styled(format!("{:<14}", truncate(who, 13)), theme::fg(color)),
            Span::styled(format!("{:<34}", truncate(&format!("{:<5}{}", r.method, r.path), 33)), theme::text()),
            Span::styled(format!("{:>4}", r.status), theme::fg(if r.status >= 400 { theme::PINK } else { theme::TEXT })),
            Span::styled(format!("{:>10}", seconds(r.duration_ms)), theme::muted()),
        ]));
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled("Nothing recorded yet.", theme::faint())));
    }
    f.render_widget(Paragraph::new(lines), recent);
}

fn draw_people(f: &mut Frame, app: &App, area: Rect, people: &[String]) {
    let block = panel("By person", false);
    let width = block.inner(area).width as usize;
    let Some(s) = summary(app) else {
        f.render_widget(block, area);
        return;
    };
    let mut lines = vec![section(if s.days == 1 { "SHARE TODAY" } else { "SHARE" }, Some(format!("{} people", s.people.len())), width)];
    let segments: Vec<(u64, ratatui::style::Color)> = s.people.iter().map(|p| (p.requests as u64, color_of(&p.name, people))).collect();
    lines.push(Line::from(stacked_bar(&segments, width, "█")));
    let total = s.requests.max(1) as f64;
    let warn_ms = app.snapshot.config_or_default().dashboard.p95_warning_seconds as i64 * 1000;
    for p in &s.people {
        let color = color_of(&p.name, people);
        let hits = if p.limit_hits > 0 {
            Span::styled(format!("{:<11}", format!("429 ×{}", p.limit_hits)), theme::fg(theme::ORANGE))
        } else {
            Span::styled(format!("{:<11}", "no 429s"), theme::faint())
        };
        lines.push(Line::from(vec![
            Span::styled("■ ", theme::fg(color)),
            Span::styled(format!("{:<14}", truncate(short_name(&p.name), 13)), theme::text()),
            Span::styled(format!("{:>6}", p.requests), theme::text()),
            Span::styled(format!("{:>5.0}% ", p.requests as f64 * 100.0 / total), theme::muted()),
            hits,
            Span::styled(
                p.p95_ms.map(|ms| format!("p95 {}", seconds(ms))).unwrap_or_default(),
                theme::fg(if p.p95_ms.is_some_and(|ms| ms > warn_ms) { theme::ORANGE } else { theme::MUTED }),
            ),
        ]));
    }
    if s.people.is_empty() {
        lines.push(Line::from(Span::styled("Nobody yet in this period.", theme::faint())));
    }

    lines.push(Line::default());
    lines.push(section("LATENCY", Some("complete replies".into()), width));
    let kv = |k: &str, v: Option<i64>, note: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<6}"), theme::faint()),
            Span::styled(format!("{:<10}", v.map(seconds).unwrap_or_else(|| "—".into())), theme::text()),
            Span::styled(note.to_string(), theme::faint()),
        ])
    };
    lines.push(kv("p50", s.p50_ms, "half of replies were quicker"));
    lines.push(kv("p95", s.p95_ms, "all but 1 in 20 were quicker"));
    lines.push(kv("max", s.max_ms, "the longest: usually a long agent turn"));

    lines.push(Line::default());
    lines.push(section("BASELINE", None, width));
    lines.push(Line::from(Span::styled("Comparisons with a baseline week arrive with Archives (build step 10).", theme::faint())));
    f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(block), area);
}
