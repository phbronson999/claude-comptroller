//! Archives: save a range of the black box to its own file before the nightly prune deletes
//! it, pick the baseline the Dashboard and Cost compare against, and delete old ones.
//! n opens the form for a new archive, b sets (or clears) the baseline, del deletes, and
//! enter shows an archive's cost report on the Cost screen.

use std::path::{Path, PathBuf};

use comptroller_admin::archive::{self, Archive, Content, New};
use comptroller_admin::cost::{civil, day_number};
use comptroller_admin::health::now;
use comptroller_admin::pricing::{self, Source};
use comptroller_admin::{bytes, dates, local_offset, settings};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Confirm, Screen, ToastKind};
use crate::cost_screen::money;
use crate::radar_screen::seconds;
use crate::theme;
use crate::ui::{panel, truncate};
use crate::widgets::section;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

#[derive(Default)]
pub struct State {
    pub row: usize,
    /// Select this archive once the list has it: one just saved.
    pub focus: Option<PathBuf>,
    pub form: Option<Form>,
}

pub struct Form {
    pub from: String,
    pub to: String,
    pub label: String,
    pub content: Content,
    /// 0 from, 1 to, 2 label, 3 content.
    pub field: usize,
    pub error: Option<String>,
    /// Requests in the range and the size of a raw archive, for the dates as typed.
    pub estimate: Option<(i64, u64)>,
    pub first_recorded: Option<i64>,
}

fn today() -> i64 {
    (now() + local_offset()).div_euclid(86_400)
}

fn short(day: i64) -> String {
    let (_, m, d) = civil(day);
    format!("{} {d}", MONTHS[m as usize - 1])
}

fn typed(day: i64) -> String {
    let (y, m, d) = civil(day);
    dates::format(y, m, d)
}

fn parse_day(text: &str) -> Option<i64> {
    dates::parse(text).map(|(y, m, d)| day_number(y, m, d))
}

fn archives(app: &App) -> &[Archive] {
    match &app.snapshot.archives {
        Some(Ok(list)) => list,
        _ => &[],
    }
}

/// The selected row: the archive just saved if the list has it, else the row moved to.
fn selected(app: &App) -> usize {
    let list = archives(app);
    app.archives.focus.as_ref().and_then(|p| list.iter().position(|a| &a.path == p)).unwrap_or(app.archives.row).min(list.len().saturating_sub(1))
}

fn database(app: &App) -> Option<PathBuf> {
    app.snapshot.config.as_ref().ok().map(|c| app.snapshot.place.database(c))
}

fn is_baseline(app: &App, a: &Archive) -> bool {
    let name = app.snapshot.config_or_default().archives.baseline;
    !name.trim().is_empty() && a.file_name().eq_ignore_ascii_case(name.trim())
}

/// Whether this process can write files in `dir`, creating it if needed.
fn folder_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Opens the form for a new archive: by default everything the black box keeps, the
/// retention days up to today, or as much of them as is recorded.
pub fn new_archive(app: &mut App) {
    let config = app.snapshot.config_or_default();
    let first_recorded = database(app).filter(|db| db.exists()).and_then(|db| archive::first_recorded_day(&db, local_offset()).ok().flatten());
    let to = today();
    let first_kept = to - config.data.retention_days.max(1) + 1;
    let from = first_kept.max(first_recorded.unwrap_or(first_kept)).min(to);
    app.archives.form = Some(Form {
        from: typed(from),
        to: typed(to),
        label: String::new(),
        content: if config.archives.default_content == "raw" { Content::Raw } else { Content::Summary },
        field: 0,
        error: None,
        estimate: None,
        first_recorded,
    });
    app.screen = Screen::Archives;
    update_estimate(app);
}

fn update_estimate(app: &mut App) {
    let db = database(app);
    let Some(form) = &mut app.archives.form else { return };
    form.estimate = match (db, parse_day(&form.from), parse_day(&form.to)) {
        (Some(db), Some(from), Some(to)) if db.exists() && from <= to => archive::estimate(&db, from, to, local_offset()).ok(),
        _ => None,
    };
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    if app.archives.form.is_some() {
        return on_form_key(app, key);
    }
    let count = archives(app).len();
    let row = selected(app);
    match key.code {
        KeyCode::Esc => app.screen = Screen::Terminal,
        KeyCode::Up | KeyCode::Char('k') if count > 0 => {
            app.archives.focus = None;
            app.archives.row = (row + count - 1) % count;
        }
        KeyCode::Down | KeyCode::Char('j') if count > 0 => {
            app.archives.focus = None;
            app.archives.row = (row + 1) % count;
        }
        KeyCode::Char('n') => new_archive(app),
        KeyCode::Char('b') => set_baseline(app),
        KeyCode::Char('d') | KeyCode::Delete => {
            let Some(a) = archives(app).get(row) else { return app.toast(ToastKind::Info, "No archives yet.") };
            let (path, title) = (a.path.clone(), a.title());
            let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
            if folder_writable(&folder) || !app.needs_admin("Deleting an archive") {
                app.confirm = Some(Confirm::DeleteArchive(path, title));
            }
        }
        KeyCode::Enter => {
            let Some(a) = archives(app).get(row) else { return };
            if a.cost.is_none() {
                return app.toast(ToastKind::Info, "This archive was saved without a pricing table, so it has no cost report.");
            }
            app.params.cost_archive = Some(a.path.clone());
            app.send_params();
            app.screen = Screen::Cost;
        }
        _ => {}
    }
}

fn on_form_key(app: &mut App, key: KeyEvent) {
    let Some(form) = &mut app.archives.form else { return };
    let mut dates_changed = false;
    match key.code {
        KeyCode::Esc => {
            app.archives.form = None;
            return;
        }
        KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 4,
        KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 3) % 4,
        KeyCode::Enter => return save(app),
        KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right if form.field == 3 => {
            form.content = if form.content == Content::Summary { Content::Raw } else { Content::Summary };
        }
        KeyCode::Backspace => {
            match form.field {
                0 => form.from.pop(),
                1 => form.to.pop(),
                2 => form.label.pop(),
                _ => None,
            };
            dates_changed = form.field < 2;
        }
        KeyCode::Char(c) => {
            match form.field {
                0 => form.from.push(c),
                1 => form.to.push(c),
                2 => form.label.push(c),
                _ => {}
            }
            dates_changed = form.field < 2;
        }
        _ => {}
    }
    if let Some(form) = &mut app.archives.form {
        form.error = None;
    }
    if dates_changed {
        update_estimate(app);
    }
}

fn save(app: &mut App) {
    let Some(form) = &app.archives.form else { return };
    let example = typed(today());
    let (Some(first_day), Some(last_day)) = (parse_day(&form.from), parse_day(&form.to)) else {
        let field = if parse_day(&form.from).is_none() { "start" } else { "end" };
        let message = format!("Type the {field} date like {example}.");
        if let Some(form) = &mut app.archives.form {
            form.error = Some(message);
        }
        return;
    };
    let (label, content) = (form.label.clone(), form.content);
    let Ok(config) = app.snapshot.config.clone() else { return app.toast(ToastKind::Warn, "comptroller.json can't be read.") };
    let place = app.snapshot.place.clone();
    let db = place.database(&config);
    let folder = place.archive_dir(&config);
    if !db.exists() {
        return app.toast(ToastKind::Info, "No database yet: nothing has been recorded.");
    }
    if !folder_writable(&folder) && app.needs_admin("Saving an archive") {
        return;
    }

    // The prices used are kept in the archive.
    let loaded = pricing::load(&place.resolve(&config.cost.pricing_file));
    let text = match &loaded {
        Ok((_, Source::File(path))) => std::fs::read_to_string(path).ok(),
        Ok((_, Source::BuiltIn)) => Some(pricing::BUILT_IN.to_string()),
        Err(_) => None,
    };
    let pricing = match (&loaded, &text) {
        (Ok((table, _)), Some(text)) => Some((table, text.as_str())),
        _ => None,
    };
    let new = New { first_day, last_day, label: &label, content };
    match archive::create(&db, &folder, &new, &config.cost, pricing, local_offset()) {
        Ok(a) => {
            let note = if a.cost.is_none() { " No pricing table could be read, so it has no cost report." } else { "" };
            app.toast(ToastKind::Success, format!("Saved {} · {} requests.{note}", a.file_name(), a.traffic.requests));
            app.archives.focus = Some(a.path);
            app.archives.form = None;
            app.refresh_now();
        }
        Err(e) => {
            if let Some(form) = &mut app.archives.form {
                form.error = Some(e);
            }
        }
    }
}

fn set_baseline(app: &mut App) {
    let row = selected(app);
    let Some(a) = archives(app).get(row) else { return app.toast(ToastKind::Info, "No archives yet. n saves one.") };
    let clearing = is_baseline(app, a);
    let (name, title) = (if clearing { String::new() } else { a.file_name() }, a.title());
    if write_baseline(app, &name, "Setting the baseline") {
        app.toast(
            ToastKind::Success,
            if clearing { format!("\"{title}\" is no longer the baseline.") } else { format!("\"{title}\" is the baseline: the Dashboard and Cost compare against it.") },
        );
    }
}

/// Writes `Archives.Baseline`. False when it didn't (and says why).
fn write_baseline(app: &mut App, name: &str, what: &'static str) -> bool {
    let path = app.snapshot.place.config_path.clone();
    if !settings::writable(&path) {
        if !app.needs_admin(what) {
            app.toast(ToastKind::Warn, format!("Can't write {}.", path.display()));
        }
        return false;
    }
    let index = settings::index_of("Archives.Baseline").expect("known setting");
    match settings::save(&path, &[(index, serde_json::Value::String(name.to_string()))]) {
        Ok(()) => {
            app.refresh_now();
            true
        }
        Err(e) => {
            app.toast(ToastKind::Warn, e);
            false
        }
    }
}

/// After the confirmation.
pub fn delete(app: &mut App, path: &Path) {
    let was_baseline = archives(app).iter().find(|a| a.path == path).is_some_and(|a| is_baseline(app, a));
    match archive::delete(path) {
        Ok(()) => {
            if was_baseline {
                write_baseline(app, "", "Clearing the baseline");
            }
            app.toast(ToastKind::Success, if was_baseline { "Archive deleted. There's no baseline now." } else { "Archive deleted." });
        }
        Err(e) => app.toast(ToastKind::Warn, e),
    }
    app.refresh_now();
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// "+12%" or "−8%" from `before` to `now`, plain: more traffic isn't good or bad by itself.
pub fn change(now: f64, before: f64) -> Span<'static> {
    if before <= 0.0 {
        return Span::styled(if now > 0.0 { "new" } else { "" }, theme::faint());
    }
    let percent = (now - before) * 100.0 / before;
    let text = if percent.abs() < 0.5 { "same".to_string() } else if percent > 0.0 { format!("+{percent:.0}%") } else { format!("−{:.0}%", -percent) };
    Span::styled(text, theme::muted())
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(54), Constraint::Percentage(46)]).spacing(1).areas(body);
    let list = archives(app);
    let row = selected(app);

    let block = panel("Archives", app.archives.form.is_none());
    let width = block.inner(left).width as usize;
    let mut lines = vec![Line::from(Span::styled(format!("  {:<18}{:<22}{:<10}{:>9}", "RANGE", "LABEL", "CONTENT", "SIZE"), theme::label()))];
    match &app.snapshot.archives {
        Some(Err(e)) => lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::ORANGE)))),
        None => lines.push(Line::from(Span::styled("Reading…", theme::muted()))),
        Some(Ok(_)) if list.is_empty() => {
            lines.push(Line::from(Span::styled("  No archives yet.", theme::faint())));
        }
        Some(Ok(_)) => {}
    }
    for (i, a) in list.iter().enumerate() {
        let star = if is_baseline(app, a) { Span::styled("★ ", theme::fg(theme::ACCENT)) } else { Span::raw("  ") };
        let range = if a.first_day == a.last_day { short(a.first_day) } else { format!("{} → {}", short(a.first_day), short(a.last_day)) };
        let line = Line::from(vec![
            star,
            Span::styled(format!("{:<18}", truncate(&range, 17)), theme::text()),
            Span::styled(format!("{:<22}", truncate(if a.label.is_empty() { "—" } else { &a.label }, 21)), theme::text()),
            Span::styled(format!("{:<10}", a.content.name()), if a.content == Content::Raw { theme::fg(theme::ORANGE) } else { theme::muted() }),
            Span::styled(format!("{:>9}", bytes(a.size)), theme::text()),
        ]);
        lines.push(if i == row && app.archives.form.is_none() { line.style(Style::new().bg(theme::SELECTION)) } else { line });
    }
    lines.push(Line::default());
    lines.push(section("NEW ARCHIVE", None, width));
    match &app.archives.form {
        Some(form) => draw_form(&mut lines, form),
        None => {
            let days = app.snapshot.config_or_default().data.retention_days;
            lines.push(Line::from(vec![
                Span::styled("n ", theme::bold(theme::ORANGE)),
                Span::styled(format!("saves a range of the black box. It keeps the last {days} days; an archive keeps a range for as long as you like."), theme::muted()),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), left);

    draw_details(f, app, right, list.get(row));
}

fn draw_form(lines: &mut Vec<Line<'static>>, form: &Form) {
    let field = |text: &str, active: bool, width: usize| {
        let shown = if active { format!(" {text}▏") } else { format!(" {text}") };
        Span::styled(format!("{shown:<width$}"), if active { Style::new().fg(theme::TEXT).bg(theme::SELECTION) } else { theme::text() })
    };
    let label = |s: &str| Span::styled(format!("{s:<10}"), theme::muted());
    lines.push(Line::from(vec![label("From"), field(&form.from, form.field == 0, 14), Span::styled("  to  ", theme::muted()), field(&form.to, form.field == 1, 14)]));
    lines.push(Line::from(vec![label("Label"), field(&form.label, form.field == 2, 34)]));
    let radio = |on: bool| if on { Span::styled("(●) ", theme::fg(theme::ACCENT)) } else { Span::styled("( ) ", theme::faint()) };
    let estimate = form.estimate.map(|(_, b)| format!("~{}", bytes(b))).unwrap_or_default();
    lines.push(Line::from(vec![
        label("Content"),
        Span::styled(if form.field == 3 { "▸ " } else { "  " }, theme::fg(theme::ORANGE)),
        radio(form.content == Content::Summary),
        Span::styled("summary ", theme::text()),
        Span::styled("a few KB    ", theme::faint()),
        radio(form.content == Content::Raw),
        Span::styled("raw rows ", theme::text()),
        Span::styled(estimate, theme::faint()),
    ]));
    lines.push(Line::default());
    let mut note = match form.estimate {
        Some((n, _)) => format!("{n} requests in the range."),
        None => format!("Dates like {}.", typed(today())),
    };
    if let Some(first) = form.first_recorded {
        note.push_str(&format!(" The black box starts on {}.", typed(first)));
    }
    lines.push(Line::from(Span::styled(note, theme::faint())));
    lines.push(Line::from(Span::styled(
        "A summary keeps the figures, tokens and cost, with the prices used. Raw also copies every request (never prompts or replies).",
        theme::faint(),
    )));
    if let Some(e) = &form.error {
        lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::PINK))));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![Span::styled(" enter ", theme::bold(theme::ORANGE)), Span::styled(" save archive", theme::text())]));
}

fn draw_details(f: &mut Frame, app: &App, area: Rect, a: Option<&Archive>) {
    let Some(a) = a else {
        let block = panel("Archive", false);
        let text = vec![
            Line::from(Span::styled("An archive is a range of the black box saved to its own file, so it outlives the nightly prune.", theme::muted())),
            Line::default(),
            Line::from(Span::styled("Make a typical week the baseline (b) and the Dashboard and Cost compare against it.", theme::muted())),
        ];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(block), area);
        return;
    };
    let title = if a.first_day == a.last_day { short(a.first_day) } else { format!("{} → {}", short(a.first_day), short(a.last_day)) };
    let block = panel(&title, false);
    let width = block.inner(area).width as usize;
    let kv = |k: &str, v: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{k:<17}"), theme::muted())];
        spans.extend(v);
        Line::from(spans)
    };
    let text = |s: String| vec![Span::styled(s, theme::text())];
    let t = &a.traffic;
    let baseline = is_baseline(app, a);
    let mut lines = vec![
        kv("Range", text(format!("{} → {} · {} day{}", typed(a.first_day), typed(a.last_day), a.days(), if a.days() == 1 { "" } else { "s" }))),
        kv("Label", text(if a.label.is_empty() { "—".into() } else { a.label.clone() })),
        kv("Baseline", if baseline { vec![Span::styled("★ yes", theme::fg(theme::ACCENT))] } else { vec![Span::styled("no · b makes it the baseline", theme::faint())] }),
        kv("Requests", text(format!("{} · {:.0} a day", t.requests, t.per_day()))),
        kv("Per hour (busy)", text(format!("{:.0}", t.per_busy_hour()))),
        kv("p50 / p95", text(format!("{} / {}", t.p50_ms.map(seconds).unwrap_or_else(|| "—".into()), t.p95_ms.map(seconds).unwrap_or_else(|| "—".into())))),
        kv("Errors", text(format!("{:.1}%", t.error_rate()))),
        kv("Holding pattern", vec![Span::styled(format!("{} × 429", t.limit_hits), if t.limit_hits > 0 { theme::fg(theme::ORANGE) } else { theme::text() })]),
        kv("People", text(t.people.len().to_string())),
    ];
    match &a.cost {
        Some(r) => {
            lines.push(kv("At API prices", text(money(&a.currency, r.api))));
            lines.push(kv("Seats", text(if r.seats > 0 { money(&a.currency, r.seat) } else { "no seat plans then".into() })));
            lines.push(kv("Priced with", vec![Span::styled(format!("pricing table of {}", a.pricing_updated.clone().unwrap_or_else(|| "—".into())), theme::faint())]));
        }
        None => lines.push(kv("Cost", vec![Span::styled("not kept: no pricing table when it was saved", theme::faint())])),
    }
    lines.push(kv("Content", text(match a.content {
        Content::Summary => "summary".into(),
        Content::Raw => "summary and every request".into(),
    })));
    let file = a.path.parent().and_then(|p| p.file_name()).map(|d| format!("{}\\{}", d.to_string_lossy(), a.file_name())).unwrap_or_else(|| a.file_name());
    lines.push(kv("File", vec![Span::styled(file, theme::faint())]));

    lines.push(Line::default());
    lines.push(section("BASELINE", None, width));
    lines.push(Line::from(Span::styled(
        if baseline {
            "The Dashboard compares today with this archive's average for the same weekday, and Cost shows the change per day. Only one archive is the baseline; b again clears it."
        } else {
            "b makes this the baseline: the Dashboard compares today with its average for the same weekday, and Cost shows the change per day. Only one archive is the baseline."
        },
        theme::muted(),
    )));
    if a.cost.is_some() {
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled("enter ", theme::bold(theme::ORANGE)), Span::styled("shows its cost report on the Cost screen.", theme::muted())]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}
