//! The Black box: the recorded requests the service keeps, day by day, and their retention.
//! p prunes now (after a confirmation), r goes to the retention settings.

use comptroller_admin::cost::civil;
use comptroller_admin::health::{now, parse_utc};
use comptroller_admin::settings::{GROUPS, SETTINGS};
use comptroller_admin::{blackbox, bytes, dates, local_offset};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Confirm, Screen, ToastKind};
use crate::theme;
use crate::ui::panel;
use crate::widgets::{ratio_bar, section};

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn today() -> i64 {
    (now() + local_offset()).div_euclid(86_400)
}

/// Date and time the local way, for seconds since 1970.
fn when(utc: i64) -> String {
    let local = utc + local_offset();
    let (y, m, d) = civil(local.div_euclid(86_400));
    let seconds = local.rem_euclid(86_400);
    format!("{} {}", dates::format(y, m, d), dates::format_time((seconds / 3600) as u32, (seconds % 3600 / 60) as u32))
}

fn database(app: &App) -> Option<std::path::PathBuf> {
    app.snapshot.config.as_ref().ok().map(|c| app.snapshot.place.database(c))
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Terminal,
        KeyCode::Char('p') => {
            let Some(db) = database(app).filter(|db| db.exists()) else {
                return app.toast(ToastKind::Info, "No database yet.");
            };
            let days = app.snapshot.config_or_default().data.retention_days;
            match blackbox::prunable(&db, today(), days, local_offset()) {
                Ok(0) => app.toast(ToastKind::Info, format!("Nothing is older than {days} days.")),
                Ok(n) if !app.needs_admin("Pruning the black box") => app.confirm = Some(Confirm::Prune(n)),
                Ok(_) => {}
                Err(_) if !app.elevated => {
                    app.needs_admin("Pruning the black box");
                }
                Err(e) => app.toast(ToastKind::Warn, e),
            }
        }
        KeyCode::Char('r') => {
            // Straight to Settings → Data → Retention days.
            let path = app.snapshot.place.config_path.clone();
            app.settings.load(&path);
            app.settings.group = GROUPS.iter().position(|(name, _)| *name == "Data").unwrap_or(0);
            app.settings.row = app.settings.rows().iter().position(|&i| SETTINGS[i].key == "RetentionDays").unwrap_or(0);
            app.screen = Screen::Settings;
        }
        KeyCode::Char('n') => app.toast(ToastKind::Info, "Archives arrive in build step 10."),
        _ => {}
    }
}

/// After the confirmation.
pub fn prune(app: &mut App) {
    let Some(db) = database(app) else { return };
    let days = app.snapshot.config_or_default().data.retention_days;
    match blackbox::prune(&db, today(), days, local_offset()) {
        Ok((removed, freed)) => app.toast(ToastKind::Success, format!("Pruned {removed} requests, freed {}.", bytes(freed))),
        Err(e) => app.toast(ToastKind::Warn, e),
    }
    app.refresh_now();
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).spacing(1).areas(body);
    let config = app.snapshot.config_or_default();
    let retention = config.data.retention_days;
    let first_kept = today() - retention + 1;

    let block = panel("Black box", true);
    let width = block.inner(left).width as usize;
    let mut lines = vec![section("PER DAY", Some("size · requests".into()), width)];
    let data = match &app.snapshot.black_box {
        Some(Ok(Some(b))) => Some(b),
        Some(Ok(None)) => {
            lines.push(Line::from(Span::styled("No database yet. The service creates it when it first starts.", theme::muted())));
            None
        }
        Some(Err(e)) => {
            lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::ORANGE))));
            if !app.elevated {
                lines.push(Line::from(Span::styled("The database is readable only by the service and administrators: reopen as administrator.", theme::muted())));
            }
            None
        }
        None => {
            lines.push(Line::from(Span::styled("Reading…", theme::muted())));
            None
        }
    };
    if let Some(b) = data {
        let top = b.days.iter().map(|d| d.1).max().unwrap_or(1).max(1);
        let bar = width.saturating_sub(34).clamp(8, 40);
        for &(day, count) in &b.days {
            let (_, m, d) = civil(day);
            let old = day < first_kept;
            let mut spans = vec![Span::styled(format!("{} {d:<4}", MONTHS[m as usize - 1]), if old { theme::fg(theme::PINK) } else { theme::muted() })];
            spans.extend(ratio_bar(count as f64 / top as f64, bar, if old { theme::PINK } else { theme::ACCENT }));
            spans.push(Span::styled(format!("{:>10}", bytes(b.estimate(count))), theme::text()));
            spans.push(Span::styled(format!("{count:>8} req"), theme::faint()));
            if old {
                spans.push(Span::styled("  goes at the next prune", theme::fg(theme::PINK)));
            }
            lines.push(Line::from(spans));
        }
        if b.days.is_empty() {
            lines.push(Line::from(Span::styled("Nothing recorded yet.", theme::faint())));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Sizes are each day's share of the file, by its share of the rows.", theme::faint())));
    }
    f.render_widget(Paragraph::new(lines).block(block), left);

    // Summary
    let block = panel("Summary", false);
    let width = block.inner(right).width as usize;
    let kv = |k: &str, v: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{k:<14}"), theme::muted())];
        spans.extend(v);
        Line::from(spans)
    };
    let text = |s: String| vec![Span::styled(s, theme::text())];
    let mut lines = vec![kv("Retention", text(format!("{retention} days · today and the {} before", retention - 1)))];
    if let Some(b) = data {
        lines.push(kv("Rows", text(b.rows.to_string())));
        lines.push(kv("Size on disk", text(bytes(b.size))));
        lines.push(kv("Oldest", text(b.oldest.map(when).unwrap_or_else(|| "—".into()))));
        lines.push(kv("Newest", text(b.newest.map(when).unwrap_or_else(|| "—".into()))));
    }
    match app.snapshot.health.as_ref().and_then(|h| h.retention.as_ref()) {
        Some(r) => {
            lines.push(kv("Next prune", text(parse_utc(&r.next_prune_utc).map(when).unwrap_or_default())));
            lines.push(kv("Last prune", match &r.last_prune {
                Some(p) if p.error.is_some() => vec![Span::styled(p.error.clone().unwrap_or_default(), theme::fg(theme::PINK))],
                Some(p) => vec![
                    Span::styled(parse_utc(&p.finished_utc).map(when).unwrap_or_default(), theme::text()),
                    Span::styled(format!(" · {} removed · freed {}", p.removed, bytes(p.freed_bytes)), theme::faint()),
                ],
                None => vec![Span::styled("not yet since the service started", theme::faint())],
            }));
        }
        None => lines.push(kv("Next prune", vec![Span::styled(format!("{} nightly, while the service runs", config.data.prune_time), theme::faint())])),
    }

    lines.push(Line::default());
    lines.push(section("WHAT'S RECORDED", None, width));
    lines.push(Line::from(Span::styled(
        "Time, person, computer, address, request path, status, time taken, size, and for model replies the model and token counts.",
        theme::muted(),
    )));
    lines.push(Line::from(Span::styled("Never prompts or replies.", theme::bold(theme::TEXT))));
    lines.push(Line::default());
    lines.push(section("KEEP SOMETHING LONGER", None, width));
    lines.push(Line::from(Span::styled(
        format!("Requests older than {retention} days are deleted each night. Archives (build step 10) will save a range first."),
        theme::muted(),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), right);
}
