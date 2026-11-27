//! Radar: today's figures and a live feed of requests. Requests in flight come from /health
//! and show first with a spinner; finished ones come from the database. Space pauses the
//! feed, f picks a person, e shows only errors and plan limits, enter shows one request.

use comptroller_admin::health::{now, parse_utc, FlyingRequest};
use comptroller_admin::traffic::{Request, Today};
use comptroller_admin::{bytes, short_duration};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Screen, ToastKind};
use crate::theme;
use crate::ui::{panel, truncate};
use crate::widgets::{centered, keycap, spinner};

#[derive(Default)]
pub struct State {
    /// The feed as it was when paused. The filters are in `app.params`.
    pub paused: Option<(Vec<Request>, Vec<FlyingRequest>)>,
    pub row: usize,
    pub details: bool,
}

/// What the feed shows now: live, or as paused.
fn feed(app: &App) -> (Vec<Request>, Vec<FlyingRequest>) {
    if let Some((rows, flying)) = &app.radar.paused {
        return (rows.clone(), flying.clone());
    }
    let rows = match &app.snapshot.feed {
        Some(Ok(Some(rows))) => rows.clone(),
        _ => Vec::new(),
    };
    let mut flying: Vec<FlyingRequest> = app.snapshot.health.as_ref().map(|h| h.in_flight.requests.clone()).unwrap_or_default();
    flying.retain(|f| app.params.feed_user.is_none() || f.user == app.params.feed_user);
    if app.params.errors_only {
        flying.clear();
    }
    // Newest first, like the rest of the feed.
    flying.reverse();
    (rows, flying)
}

fn today(app: &App) -> Option<&Today> {
    match &app.snapshot.today {
        Some(Ok(Some(t))) => Some(t),
        _ => None,
    }
}

/// Everyone who sent something today, for the filter and the colors.
pub fn people(app: &App) -> Vec<String> {
    let mut people: Vec<String> = today(app).map(|t| t.by_user.keys().cloned().collect()).unwrap_or_default();
    if let Some(user) = &app.params.feed_user {
        if !people.contains(user) {
            people.push(user.clone());
        }
    }
    people.sort_by_key(|p| p.to_lowercase());
    people
}

const PALETTE: [Color; 6] = [theme::ACCENT, theme::ORANGE, theme::PINK, theme::OK, Color::Rgb(180, 150, 255), Color::Rgb(250, 215, 90)];

/// Each person's color, the same on every screen that lists them in the same order.
pub fn color_of(person: &str, people: &[String]) -> Color {
    people.iter().position(|p| p == person).map(|i| PALETTE[i % PALETTE.len()]).unwrap_or(theme::MUTED)
}

/// `DOMAIN\name` without the domain, for narrow columns.
pub fn short_name(user: &str) -> &str {
    user.rsplit('\\').next().unwrap_or(user)
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    if app.radar.details {
        app.radar.details = false;
        return;
    }
    let (rows, flying) = feed(app);
    let count = rows.len() + flying.len();
    match key.code {
        KeyCode::Esc => {
            app.radar.paused = None;
            app.screen = Screen::Terminal;
        }
        KeyCode::Char(' ') => {
            app.radar.paused = match app.radar.paused {
                Some(_) => None,
                None => Some((rows, flying)),
            };
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            let people = people(app);
            let at = app.params.feed_user.as_ref().and_then(|u| people.iter().position(|p| p == u)).map(|i| i + 1).unwrap_or(0);
            // 0 is everyone; then each person in turn.
            let next = if key.code == KeyCode::Char('f') { (at + 1) % (people.len() + 1) } else { (at + people.len()) % (people.len() + 1) };
            app.params.feed_user = if next == 0 { None } else { Some(people[next - 1].clone()) };
            app.radar.row = 0;
            app.radar.paused = None;
            app.send_params();
        }
        KeyCode::Char('e') => {
            app.params.errors_only = !app.params.errors_only;
            app.radar.row = 0;
            app.radar.paused = None;
            app.send_params();
        }
        KeyCode::Up | KeyCode::Char('k') if count > 0 => app.radar.row = app.radar.row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') if count > 0 => app.radar.row = (app.radar.row + 1).min(count - 1),
        KeyCode::Home => app.radar.row = 0,
        KeyCode::Enter => {
            if app.radar.row >= flying.len() && app.radar.row < count {
                app.radar.details = true;
            } else if count > 0 {
                app.toast(ToastKind::Info, "Still in flight: its details are recorded when it finishes.");
            }
        }
        KeyCode::Char('d') => {
            app.radar.paused = None;
            app.screen = Screen::Dashboard;
        }
        _ => {}
    }
}

// ── drawing ─────────────────────────────────────────────────────────────────

pub fn seconds(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        short_duration((ms / 1000) as u64)
    }
}

/// 1.2k, 34k, 1.5M.
pub fn tokens(n: i64) -> String {
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 10_000 => format!("{:.1}k", n as f64 / 1000.0),
        n if n < 1_000_000 => format!("{}k", n / 1000),
        n => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

use comptroller_admin::local_clock as clock;

/// The note for a finished request: what went wrong, or what it used.
fn note(r: &Request) -> (String, Color) {
    match (r.status, r.proxy_error.as_deref()) {
        (429, _) => ("plan limit".into(), theme::PINK),
        (_, Some("RequestCanceled")) => ("cancelled by the client".into(), theme::FAINT),
        (_, Some("RequestTimedOut")) => ("upstream timeout".into(), theme::PINK),
        (_, Some(error)) => (format!("no reply: {error}"), theme::PINK),
        (s, _) if s >= 500 => ("upstream error".into(), theme::PINK),
        (401 | 403, _) => ("refused upstream".into(), theme::ORANGE),
        _ => match (r.usage.as_deref(), r.input_tokens, r.output_tokens) {
            (Some("parsed"), Some(input), Some(output)) => {
                let cached = r.cache_read_tokens.unwrap_or(0) + r.cache_write_tokens.unwrap_or(0);
                let model = r.model.as_deref().unwrap_or("").trim_start_matches("claude-");
                let cache = if cached > 0 { format!(" · {} cache", tokens(cached)) } else { String::new() };
                (format!("{model} · {} in · {} out{cache}", tokens(input), tokens(output)), theme::MUTED)
            }
            (Some("missing"), _, _) => (format!("{} · usage not read", bytes(r.response_bytes as u64)), theme::ORANGE),
            _ => (bytes(r.response_bytes as u64), theme::FAINT),
        },
    }
}

fn card(f: &mut Frame, area: Rect, title: &str, value: Span<'static>, detail: Vec<Span<'static>>) {
    let block = panel(title, false);
    f.render_widget(Paragraph::new(vec![Line::from(value), Line::from(detail)]).block(block), area);
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [cards, list] = Layout::vertical([Constraint::Length(5), Constraint::Fill(1)]).spacing(1).areas(body);
    let people = people(app);
    draw_cards(f, app, cards, &people);
    draw_feed(f, app, list, &people);
    if app.radar.details {
        draw_details(f, app, f.area());
    }
}

/// The four cards Radar and the Dashboard share.
pub struct Figures {
    /// "TODAY" or "7 DAYS".
    pub period: String,
    pub requests: i64,
    pub p95_ms: Option<i64>,
    pub limit_hits: i64,
    pub limit_hits_by_user: Vec<(String, i64)>,
    pub errors: i64,
}

fn draw_cards(f: &mut Frame, app: &App, area: Rect, people: &[String]) {
    let figures = today(app).map(|t| Figures {
        period: "TODAY".into(),
        requests: t.requests,
        p95_ms: t.p95_ms,
        limit_hits: t.limit_hits,
        limit_hits_by_user: t.limit_hits_by_user.clone(),
        errors: t.errors,
    });
    draw_figures(f, app, area, figures.as_ref(), people);
}

pub fn draw_figures(f: &mut Frame, app: &App, area: Rect, figures: Option<&Figures>, people: &[String]) {
    let areas: [Rect; 4] = Layout::horizontal([Constraint::Fill(1); 4]).spacing(1).areas(area);
    let dash = || Span::styled("—", theme::faint());
    let limits = app.snapshot.config_or_default().dashboard;
    let t = figures;

    card(
        f,
        areas[0],
        &format!("REQUESTS {}", t.map(|t| t.period.as_str()).unwrap_or("TODAY")),
        t.map(|t| Span::styled(t.requests.to_string(), theme::bold(theme::TEXT))).unwrap_or_else(dash),
        vec![Span::styled(if t.is_some_and(|t| t.period != "TODAY") { "local days" } else { "since midnight" }, theme::faint())],
    );
    let slow = |ms: i64| ms > limits.p95_warning_seconds as i64 * 1000;
    card(
        f,
        areas[1],
        "P95 LATENCY",
        t.and_then(|t| t.p95_ms).map(|ms| Span::styled(seconds(ms), theme::bold(if slow(ms) { theme::ORANGE } else { theme::TEXT }))).unwrap_or_else(dash),
        vec![Span::styled(format!("of complete replies · warn over {} s", limits.p95_warning_seconds), theme::faint())],
    );
    let mut hits = vec![Span::styled("429s ", theme::faint())];
    for (user, n) in t.map(|t| t.limit_hits_by_user.clone()).unwrap_or_default().into_iter().take(3) {
        hits.push(Span::styled(format!("· {} {n} ", short_name(&user)), theme::fg(color_of(&user, people))));
    }
    card(
        f,
        areas[2],
        "HOLDING PATTERN",
        t.map(|t| Span::styled(t.limit_hits.to_string(), theme::bold(if t.limit_hits > 0 { theme::ORANGE } else { theme::TEXT }))).unwrap_or_else(dash),
        hits,
    );
    let warn = limits.error_rate_warning_percent;
    let (value, detail) = match t {
        Some(t) if t.requests > 0 => {
            let share = t.errors as f64 * 100.0 / t.requests as f64;
            let over = share > warn;
            (
                Span::styled(format!("{share:.1}%"), theme::bold(if over { theme::PINK } else { theme::TEXT })),
                vec![
                    Span::styled(format!("{} of {} · ", t.errors, t.requests), theme::faint()),
                    Span::styled(format!("{} {warn}%", if over { "over" } else { "under" }), theme::fg(if over { theme::PINK } else { theme::OK })),
                ],
            )
        }
        _ => (dash(), vec![Span::styled("server errors and timeouts", theme::faint())]),
    };
    card(f, areas[3], "ERRORS", value, detail);
}

const TIME: usize = 10;
const WHO: usize = 18;
const REQUEST: usize = 34;
const STATUS: usize = 5;
const MS: usize = 9;

fn draw_feed(f: &mut Frame, app: &App, area: Rect, people: &[String]) {
    let block = panel("Radar", true);
    let inner = block.inner(area);
    let width = inner.width as usize;
    let (rows, flying) = feed(app);

    // Filter bar
    let mut bar = vec![Span::styled("filter ", theme::faint())];
    let chip = |text: String, on: bool, color: Color| {
        if on {
            Span::styled(format!(" {text} "), Style::new().fg(theme::TEXT).bg(theme::SELECTION))
        } else {
            Span::styled(format!(" {text} "), theme::fg(color))
        }
    };
    bar.push(chip("all".into(), app.params.feed_user.is_none(), theme::TEXT));
    for person in people {
        bar.push(chip(short_name(person).to_string(), app.params.feed_user.as_deref() == Some(person), color_of(person, people)));
    }
    bar.push(Span::styled(" │ ", theme::faint()));
    bar.push(chip("errors only".into(), app.params.errors_only, theme::TEXT));
    let in_flight = app.snapshot.health.as_ref().map(|h| h.in_flight.count).unwrap_or(0);
    let status = match (&app.radar.paused, app.snapshot.health.is_some()) {
        (Some(_), _) => vec![Span::styled("❚❚ paused", theme::fg(theme::ORANGE)), Span::styled(" · space resumes", theme::faint())],
        (None, true) => vec![
            Span::styled("● live", theme::fg(theme::OK)),
            Span::styled(format!(" · {} s refresh · {in_flight} in flight", app.live_secs()), theme::faint()),
        ],
        (None, false) => vec![Span::styled("○ service not answering", theme::faint())],
    };
    let used: usize = bar.iter().chain(status.iter()).map(|s| s.content.chars().count()).sum();
    bar.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    bar.extend(status);

    let mut lines = vec![Line::from(bar), Line::default()];
    lines.push(Line::from(Span::styled(
        format!("{:<TIME$}{:<WHO$}{:<REQUEST$}{:>STATUS$}{:>MS$}  {}", "TIME", "WHO", "REQUEST", "ST", "TIME", "NOTE"),
        theme::label(),
    )));

    match &app.snapshot.feed {
        Some(Err(e)) => {
            lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::ORANGE))));
            if !app.elevated {
                lines.push(Line::from(Span::styled("The database is readable only by the service and administrators: reopen as administrator.", theme::muted())));
            }
        }
        Some(Ok(None)) => lines.push(Line::from(Span::styled("No database yet. The service creates it when it first starts.", theme::muted()))),
        _ if rows.is_empty() && flying.is_empty() && app.radar.paused.is_none() => {
            let text = if app.params.errors_only || app.params.feed_user.is_some() { "Nothing matches the filter." } else { "No requests recorded yet. They appear here as Claude Code is used." };
            lines.push(Line::from(Span::styled(text, theme::muted())));
        }
        _ => {}
    }

    let selected = app.radar.row.min((rows.len() + flying.len()).saturating_sub(1));
    let who = |user: Option<&str>, device: Option<&str>| -> Vec<Span<'static>> {
        match user {
            Some(user) => {
                let name = short_name(user);
                let device = device.unwrap_or("");
                let text = truncate(&format!("{name} {device}"), WHO - 1);
                let name_len = name.chars().count().min(text.chars().count());
                let (a, b) = text.split_at(text.char_indices().nth(name_len).map(|(i, _)| i).unwrap_or(text.len()));
                vec![
                    Span::styled(a.to_string(), theme::fg(color_of(user, people))),
                    Span::styled(format!("{b:<0$}", WHO - a.chars().count()), theme::faint()),
                ]
            }
            None => vec![Span::styled(format!("{:<WHO$}", "—"), theme::faint())],
        }
    };

    let mut index = 0;
    for f_req in &flying {
        let started = parse_utc(&f_req.started_utc).unwrap_or(now());
        let mut spans = vec![Span::styled(format!("{:<TIME$}", clock(started)), theme::text())];
        spans.extend(who(f_req.user.as_deref(), f_req.device.as_deref()));
        spans.push(Span::styled(format!("{:<REQUEST$}", truncate(&format!("{:<5}{}", f_req.method, f_req.path), REQUEST - 1)), theme::text()));
        spans.push(Span::styled(format!("{:>STATUS$}", spinner(app.tick)), theme::fg(theme::ORANGE)));
        spans.push(Span::styled(format!("{:>MS$}", seconds((now() - started).max(0) * 1000)), theme::fg(theme::ORANGE)));
        spans.push(Span::styled("  in flight", theme::fg(theme::ORANGE)));
        lines.push(row_line(spans, index == selected, width));
        index += 1;
    }
    for r in &rows {
        let (note, note_color) = note(r);
        let status_color = match r.status {
            429 => theme::PINK,
            s if s >= 400 => theme::PINK,
            _ => theme::TEXT,
        };
        let faint_probe = r.user.is_none();
        let mut spans = vec![Span::styled(format!("{:<TIME$}", clock(r.started)), if faint_probe { theme::faint() } else { theme::text() })];
        spans.extend(who(r.user.as_deref(), r.device.as_deref()));
        spans.push(Span::styled(
            format!("{:<REQUEST$}", truncate(&format!("{:<5}{}", r.method, r.path), REQUEST - 1)),
            if faint_probe { theme::faint() } else { theme::text() },
        ));
        spans.push(Span::styled(format!("{:>STATUS$}", r.status), theme::fg(status_color)));
        spans.push(Span::styled(format!("{:>MS$}", seconds(r.duration_ms)), theme::text()));
        spans.push(Span::styled(format!("  {}", truncate(&note, width.saturating_sub(TIME + WHO + REQUEST + STATUS + MS + 2).max(8))), theme::fg(note_color)));
        lines.push(row_line(spans, index == selected, width));
        index += 1;
    }

    // Keep the selected row in view.
    let scroll = (selected + 3 + 3).saturating_sub(inner.height as usize) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), area);
}

fn row_line(mut spans: Vec<Span<'static>>, selected: bool, width: usize) -> Line<'static> {
    if !selected {
        return Line::from(spans);
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(Style::new().bg(theme::SELECTION))
}

fn draw_details(f: &mut Frame, app: &App, screen: Rect) {
    let (rows, flying) = feed(app);
    let Some(r) = app.radar.row.checked_sub(flying.len()).and_then(|i| rows.get(i)) else { return };
    let kv = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<16}"), theme::faint()), Span::styled(v, theme::text())]);
    let opt = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_else(|| "—".into());
    let (note, color) = note(r);
    let mut lines = vec![
        kv("When", format!("{} local · {} ago", clock(r.started), short_duration((now() - r.started).max(0) as u64))),
        kv("Person", r.user.clone().unwrap_or_else(|| "— (Claude Code's connectivity check)".into())),
        kv("Computer", r.device.clone().unwrap_or_else(|| "—".into())),
        kv("From address", r.client_ip.clone().unwrap_or_else(|| "—".into())),
        kv("Request", format!("{} {}", r.method, r.path)),
        kv("Status", r.status.to_string()),
        kv("Took", seconds(r.duration_ms)),
        kv("Sent · received", format!("{} · {}", r.request_bytes.map(|b| bytes(b as u64)).unwrap_or_else(|| "—".into()), bytes(r.response_bytes as u64))),
        Line::from(vec![Span::styled(format!("{:<16}", "Note"), theme::faint()), Span::styled(note, theme::fg(color))]),
    ];
    if r.usage.is_some() {
        lines.push(Line::default());
        lines.push(kv("Model", r.model.clone().unwrap_or_else(|| "—".into())));
        lines.push(kv("Input tokens", opt(r.input_tokens)));
        lines.push(kv("Output tokens", opt(r.output_tokens)));
        lines.push(kv("Cache writes", opt(r.cache_write_tokens)));
        lines.push(kv("Cache reads", opt(r.cache_read_tokens)));
        if r.usage.as_deref() == Some("missing") {
            lines.push(Line::from(Span::styled("The reply's token counts couldn't be read. Status counts these.", theme::fg(theme::ORANGE))));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Only this is recorded: never the prompt or the reply.", theme::faint())));
    lines.push(Line::from(keycap("any key", "close")).right_aligned());

    let area = centered(screen, 74, lines.len() as u16 + 4);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(theme::BORDER_FOCUS))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(Span::styled(format!(" Request {} ", r.id), theme::bold(theme::TEXT))))
        .padding(Padding::new(2, 2, 1, 0));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}
