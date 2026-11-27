//! The Users screen: each person and the computers they connected, straight from the
//! service's database. Suspend, revoke or remove a computer, revoke a person, and in
//! manual sign-in mode create one-time codes. Changes apply to the next request.

use comptroller_admin::directory::{self, Code, Device, Directory, User};
use comptroller_admin::health::now;
use comptroller_admin::short_duration;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Confirm, Screen, ToastKind, UserChange};
use crate::theme;
use crate::ui::{ago, panel, truncate};
use crate::widgets::{centered, keycap, section};

#[derive(Default)]
pub struct State {
    pub row: usize,
    pub form: Option<CodeForm>,
    pub code: Option<NewCode>,
}

impl State {
    /// A dialog that takes every key is open.
    pub fn typing(&self) -> bool {
        self.form.is_some() || self.code.is_some()
    }
}

#[derive(Default)]
pub struct CodeForm {
    pub user: String,
    pub device: String,
    /// 0: user, 1: device.
    pub field: usize,
    pub error: Option<String>,
}

pub struct NewCode {
    pub code: String,
    pub user: String,
    pub device: String,
    pub hours: i64,
}

pub enum Row<'a> {
    User(&'a User),
    Device(&'a User, &'a Device),
    Code(&'a Code),
}

pub fn rows(directory: &Directory) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for user in &directory.users {
        rows.push(Row::User(user));
        rows.extend(user.devices.iter().map(|d| Row::Device(user, d)));
    }
    rows.extend(directory.codes.iter().map(Row::Code));
    rows
}

fn directory(app: &App) -> Option<&Directory> {
    match &app.snapshot.users {
        Some(Ok(Some(d))) => Some(d),
        _ => None,
    }
}

fn database(app: &App) -> Option<std::path::PathBuf> {
    app.snapshot.config.as_ref().ok().map(|c| app.snapshot.place.database(c))
}

/// What the selected row would change, if anything; `None` with a reason to show.
fn plan(app: &App, key: char) -> Result<Option<UserChange>, String> {
    let Some(d) = directory(app) else { return Err("No users to change.".into()) };
    let rows = rows(d);
    let Some(row) = rows.get(app.users.row.min(rows.len().saturating_sub(1))) else { return Err("Nobody has connected yet.".into()) };
    Ok(match (key, row) {
        ('r', Row::Device(u, d)) if d.status == "revoked" => return Err(format!("{} is already revoked for {}. del removes it.", d.name, u.name)),
        ('r', Row::Device(u, d)) => Some(UserChange::RevokeDevice { id: d.id, device: d.name.clone(), user: u.name.clone() }),
        ('r', Row::User(u)) if !u.revoked => Some(UserChange::RevokeUser { id: u.id, user: u.name.clone() }),
        ('r', Row::User(_)) => None,
        ('d', Row::Device(u, d)) => Some(UserChange::RemoveDevice { id: d.id, device: d.name.clone(), user: u.name.clone() }),
        ('d', Row::User(u)) => Some(UserChange::RemoveUser { id: u.id, user: u.name.clone() }),
        ('d', Row::Code(c)) => Some(UserChange::CancelCode { hash: c.hash.clone(), device: c.device.clone(), user: c.user.clone() }),
        _ => return Err("That key does nothing on this row.".into()),
    })
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    if app.users.code.is_some() {
        match key.code {
            KeyCode::Char('c') => copy_code(app),
            KeyCode::Enter | KeyCode::Esc => app.users.code = None,
            _ => {}
        }
        return;
    }
    if let Some(form) = &mut app.users.form {
        match key.code {
            KeyCode::Esc => app.users.form = None,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => form.field = 1 - form.field,
            KeyCode::Backspace => {
                if form.field == 0 { form.user.pop() } else { form.device.pop() };
                form.error = None;
            }
            KeyCode::Char(c) => {
                if form.field == 0 { form.user.push(c) } else { form.device.push(c) };
                form.error = None;
            }
            KeyCode::Enter if form.field == 0 => form.field = 1,
            KeyCode::Enter => create_code(app),
            _ => {}
        }
        return;
    }

    let count = directory(app).map(|d| rows(d).len()).unwrap_or(0);
    match key.code {
        KeyCode::Esc => app.screen = Screen::Terminal,
        KeyCode::Up | KeyCode::Char('k') if count > 0 => app.users.row = (app.users.row.min(count - 1) + count - 1) % count,
        KeyCode::Down | KeyCode::Char('j') if count > 0 => app.users.row = (app.users.row + 1) % count,
        KeyCode::Enter if matches!(app.snapshot.users, Some(Err(_))) => {
            if !app.needs_admin("Seeing users") {
                app.refresh_now();
            }
        }
        KeyCode::Char('s') => suspend_or_resume(app),
        KeyCode::Char('r') => ask(app, 'r'),
        KeyCode::Char('d') | KeyCode::Delete => ask(app, 'd'),
        KeyCode::Char('p') => next_plan(app),
        KeyCode::Char('n') => {
            if app.snapshot.config_or_default().identity.mode != "manual" {
                app.toast(ToastKind::Info, "One-time codes are for manual sign-in. In Windows mode people appear when they connect.");
            } else {
                app.users.form = Some(CodeForm::default());
            }
        }
        _ => {}
    }
}

fn ask(app: &mut App, key: char) {
    match plan(app, key) {
        Ok(Some(change)) => app.confirm = Some(Confirm::User(change)),
        // Restoring a revoked person needs no confirmation.
        Ok(None) => restore_user(app),
        Err(reason) => app.toast(ToastKind::Info, reason),
    }
}

/// Runs a database change and reports it; offers an administrator terminal when that's what's missing.
fn run(app: &mut App, what: &'static str, done: String, work: impl FnOnce(&std::path::Path) -> Result<usize, String>) {
    let Some(db) = database(app) else { return app.toast(ToastKind::Warn, "comptroller.json can't be read.") };
    match work(&db) {
        Ok(0) => app.toast(ToastKind::Info, "Nothing changed: it was already like that."),
        Ok(_) => app.toast(ToastKind::Success, done),
        Err(e) if !app.elevated && e.contains("administrator") => {
            app.needs_admin(what);
        }
        Err(e) => app.toast(ToastKind::Warn, e),
    }
    app.refresh_now();
}

fn suspend_or_resume(app: &mut App) {
    let Some(d) = directory(app) else { return };
    let rows = rows(d);
    let Some(Row::Device(user, device)) = rows.get(app.users.row) else {
        return app.toast(ToastKind::Info, "Choose a computer: s suspends or resumes one.");
    };
    let (id, name, user) = (device.id, device.name.clone(), user.name.clone());
    match (device.status.as_str(), device.suspended_by.as_deref()) {
        ("revoked", _) => app.toast(ToastKind::Info, format!("{name} is revoked. del removes it so it can connect again.")),
        ("suspended", Some("tray")) => {
            app.toast(ToastKind::Info, format!("{user} disconnected {name} from the tray app. Only they can reconnect it."))
        }
        ("suspended", _) => run(app, "Changing users", format!("Resumed {name}."), |db| directory::resume_device(db, id)),
        _ => run(app, "Changing users", format!("Suspended {name}: its next request is refused."), |db| directory::suspend_device(db, id)),
    }
}

/// Puts the selected person on the next seat plan, then none, for the Cost screen.
fn next_plan(app: &mut App) {
    let plans: Vec<String> = app.snapshot.config_or_default().cost.seat_plans.into_iter().map(|p| p.name).collect();
    let Some(d) = directory(app) else { return };
    let rows = rows(d);
    let Some(Row::User(user)) = rows.get(app.users.row) else {
        return app.toast(ToastKind::Info, "Choose a person: p puts them on a seat plan.");
    };
    if plans.is_empty() {
        return app.toast(ToastKind::Info, "No seat plans yet. Add them on the Cost screen (4 on the Terminal, then s).");
    }
    let at = user.seat_plan.as_ref().and_then(|p| plans.iter().position(|n| n == p)).map(|i| i + 1).unwrap_or(0);
    let next = plans.get(at % (plans.len() + 1)).cloned();
    let next = if at == plans.len() { None } else { next };
    let (id, name) = (user.id, user.name.clone());
    let done = match &next {
        Some(plan) => format!("{name} is on {plan}."),
        None => format!("{name} has no seat plan."),
    };
    run(app, "Changing users", done, |db| directory::set_seat_plan(db, id, next.as_deref()));
}

fn restore_user(app: &mut App) {
    let Some(d) = directory(app) else { return };
    let rows = rows(d);
    let Some(Row::User(user)) = rows.get(app.users.row) else { return };
    let (id, name) = (user.id, user.name.clone());
    run(app, "Changing users", format!("Restored {name}."), |db| directory::set_user_revoked(db, id, false));
}

/// After the confirmation.
pub fn apply(app: &mut App, change: UserChange) {
    match change {
        UserChange::RevokeDevice { id, device, user } => {
            run(app, "Changing users", format!("Revoked {device} for {user}."), |db| directory::revoke_device(db, id))
        }
        UserChange::RemoveDevice { id, device, .. } => run(app, "Changing users", format!("Removed {device}."), |db| directory::remove_device(db, id)),
        UserChange::RevokeUser { id, user } => run(app, "Changing users", format!("Revoked {user} on every computer."), |db| directory::set_user_revoked(db, id, true)),
        UserChange::RemoveUser { id, user } => run(app, "Changing users", format!("Removed {user}."), |db| directory::remove_user(db, id)),
        UserChange::CancelCode { hash, device, .. } => run(app, "Changing users", format!("Cancelled the code for {device}."), |db| directory::cancel_code(db, &hash)),
    }
}

fn create_code(app: &mut App) {
    let Some(form) = &app.users.form else { return };
    let (user, device) = (form.user.trim().to_string(), form.device.trim().to_string());
    if !directory::valid_name(&user) || !directory::valid_name(&device) {
        if let Some(form) = &mut app.users.form {
            form.error = Some("Enter a person and a computer name, up to 64 characters each.".into());
        }
        return;
    }
    let Some(db) = database(app) else { return };
    if !db.exists() {
        if let Some(form) = &mut app.users.form {
            form.error = Some("No database yet: start the service once (Takeoff), then try again.".into());
        }
        return;
    }
    let hours = app.snapshot.config_or_default().identity.code_lifetime_hours;
    match directory::create_code(&db, &user, &device, hours) {
        Ok(code) => {
            app.users.form = None;
            app.users.code = Some(NewCode { code, user, device, hours });
            app.refresh_now();
        }
        Err(e) if !app.elevated && e.contains("administrator") => {
            app.users.form = None;
            app.needs_admin("Creating a code");
        }
        Err(e) => {
            if let Some(form) = &mut app.users.form {
                form.error = Some(e);
            }
        }
    }
}

fn copy_code(app: &mut App) {
    let Some(code) = app.users.code.as_ref().map(|c| c.code.clone()) else { return };
    let copied = comptroller_admin::activity::track(
        "clip.exe",
        || {
            use std::io::Write;
            let mut child = std::process::Command::new("clip.exe").stdin(std::process::Stdio::piped()).spawn()?;
            child.stdin.take().expect("piped").write_all(code.as_bytes())?;
            child.wait()
        },
        |_| "copied the code".into(),
    );
    match copied {
        Ok(_) => app.toast(ToastKind::Success, "Copied the code."),
        Err(e) => app.toast(ToastKind::Warn, format!("Couldn't copy: {e}")),
    }
}

// ── drawing ─────────────────────────────────────────────────────────────────

fn device_status(user: &User, device: &Device) -> (&'static str, Color) {
    match (device.status.as_str(), device.suspended_by.as_deref()) {
        _ if user.revoked => ("person revoked", theme::PINK),
        ("revoked", _) => ("revoked", theme::PINK),
        ("suspended", Some("tray")) => ("disconnected by user", theme::ORANGE),
        ("suspended", _) => ("suspended by admin", theme::PINK),
        _ if device.expired() => ("token expired", theme::FAINT),
        _ => ("connected", theme::OK),
    }
}

fn seen(time: Option<i64>) -> String {
    time.map(|t| ago(now() - t)).unwrap_or_else(|| "never".into())
}

const NAME: usize = 30;
const STATUS: usize = 24;
const SEEN: usize = 12;
const TODAY: usize = 6;

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).spacing(1).areas(body);
    let config = app.snapshot.config_or_default();
    let manual = config.identity.mode == "manual";

    let count = directory(app).map(|d| d.users.len()).unwrap_or(0);
    let block = panel("Users", true).title(Line::from(Span::styled(format!(" {count} "), theme::faint())).right_aligned());
    let inner = block.inner(left);
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_at = 0;

    match &app.snapshot.users {
        None => lines.push(Line::from(Span::styled("Reading…", theme::muted()))),
        Some(Err(e)) => {
            lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::ORANGE))));
            if !app.elevated {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("The database is readable only by the service and administrators. Press enter to reopen as administrator.", theme::muted())));
            }
        }
        Some(Ok(None)) => lines.push(Line::from(Span::styled("No database yet. The service creates it when it first starts.", theme::muted()))),
        Some(Ok(Some(d))) if d.users.is_empty() && d.codes.is_empty() => {
            lines.push(Line::from(Span::styled(
                if manual { "Nobody yet. Press n to make a one-time code for someone's computer." } else { "Nobody has connected yet. People appear here when they open claude-comptroller-connect." },
                theme::muted(),
            )));
        }
        Some(Ok(Some(d))) => {
            lines.push(Line::from(Span::styled(
                format!("  {:<NAME$}{:<STATUS$}{:>TODAY$}  {:<SEEN$}{}", "PERSON · COMPUTER", "STATUS", "TODAY", "LAST SEEN", "TOKEN"),
                theme::label(),
            )));
            let today = match &app.snapshot.today {
                Some(Ok(Some(t))) => Some(t),
                _ => None,
            };
            let count = |n: Option<&i64>| match (today, n) {
                (None, _) => format!("{:>TODAY$}  ", "—"),
                (Some(_), n) => format!("{:>TODAY$}  ", n.copied().unwrap_or(0)),
            };
            let selected_row = app.users.row.min(rows(d).len().saturating_sub(1));
            let mut codes_heading = false;
            for (index, row) in rows(d).into_iter().enumerate() {
                let selected = index == selected_row;
                let mut spans = vec![Span::styled(if selected { "▍ " } else { "  " }, theme::fg(theme::ACCENT))];
                match row {
                    Row::User(u) => {
                        if index > 0 {
                            lines.push(Line::default());
                        }
                        // How they sign in, beside the name: "windows" or "code".
                        let source = if u.source == "manual" { "code" } else { "windows" };
                        let name = truncate(&u.name, NAME - 3 - source.len());
                        let (status, color) = if u.revoked {
                            ("revoked".to_string(), theme::PINK)
                        } else {
                            let computers = format!("{} computer{}", u.devices.len(), if u.devices.len() == 1 { "" } else { "s" });
                            let text = match &u.seat_plan {
                                Some(plan) => format!("{computers} · {plan}"),
                                None => computers,
                            };
                            (truncate(&text, STATUS - 1), theme::MUTED)
                        };
                        let gap = NAME.saturating_sub(name.chars().count() + source.len() + 1);
                        spans.push(Span::styled(name, theme::bold(if u.revoked { theme::FAINT } else { theme::TEXT })));
                        spans.push(Span::styled(format!(" {source}{}", " ".repeat(gap)), theme::faint()));
                        spans.push(Span::styled(format!("{status:<STATUS$}"), theme::fg(color)));
                        spans.push(Span::styled(count(today.and_then(|t| t.by_user.get(&u.name))), theme::text()));
                        spans.push(Span::styled(seen(u.last_seen()), theme::muted()));
                    }
                    Row::Device(u, dev) => {
                        let (status, color) = device_status(u, dev);
                        let name = truncate(&dev.name, NAME - 4);
                        let token = if dev.expired() { "expired".to_string() } else { format!("{} left", short_duration((dev.expires - now()).max(0) as u64)) };
                        spans.push(Span::styled("└ ", theme::faint()));
                        spans.push(Span::styled(format!("{name:<0$}", NAME - 2), theme::text()));
                        spans.push(Span::styled(format!("{status:<STATUS$}"), theme::fg(color)));
                        spans.push(Span::styled(count(today.and_then(|t| t.by_device.get(&(u.name.clone(), dev.name.clone())))), theme::muted()));
                        spans.push(Span::styled(format!("{:<SEEN$}", seen(dev.last_seen)), theme::muted()));
                        spans.push(Span::styled(token, theme::faint()));
                    }
                    Row::Code(c) => {
                        if !codes_heading {
                            codes_heading = true;
                            lines.push(Line::default());
                            lines.push(section("ONE-TIME CODES NOT USED YET", None, width));
                        }
                        let name = truncate(&format!("{} · {}", c.user, c.device), NAME - 2);
                        spans.push(Span::styled(format!("{name:<NAME$}"), theme::text()));
                        spans.push(Span::styled(format!("{:<STATUS$}", "waiting"), theme::fg(theme::ACCENT)));
                        spans.push(Span::raw(" ".repeat(TODAY + 2)));
                        spans.push(Span::styled(format!("{:<SEEN$}", ago(now() - c.created)), theme::muted()));
                        spans.push(Span::styled(format!("{} left", short_duration((c.expires - now()).max(0) as u64)), theme::faint()));
                    }
                }
                if selected {
                    selected_at = lines.len();
                    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
                    lines.push(Line::from(spans).style(Style::new().bg(theme::SELECTION)));
                } else {
                    lines.push(Line::from(spans));
                }
            }
        }
    }
    let scroll = (selected_at + 3).saturating_sub(inner.height as usize) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), left);

    draw_about(f, app, right, manual, &config.identity.allowed_group);

    if let Some(form) = &app.users.form {
        draw_form(f, app, form, f.area());
    }
    if let Some(code) = &app.users.code {
        draw_code(f, code, f.area());
    }
}

fn draw_about(f: &mut Frame, app: &App, area: Rect, manual: bool, group: &str) {
    let block = panel("About", false);
    let width = block.inner(area).width as usize;
    let mut lines = vec![section("HOW SIGN-IN WORKS", None, width)];
    lines.push(Line::from(Span::styled(
        if manual {
            "Manual mode. Press n to make a one-time code for a person and computer. They type it into claude-comptroller-connect, once.".to_string()
        } else {
            format!("Windows mode. Anyone in {group} who opens claude-comptroller-connect signs in by themselves and appears here, with each computer they use.")
        },
        theme::muted(),
    )));
    lines.push(Line::default());

    lines.push(section("SELECTED", None, width));
    let kv = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<12}"), theme::faint()), Span::styled(v, theme::text())]);
    // Local date the way Windows shows it, and local time.
    let date = |t: i64| {
        let (y, m, d) = comptroller_admin::cost::civil((t + comptroller_admin::local_offset()).div_euclid(86_400));
        comptroller_admin::dates::format(y, m, d)
    };
    if let Some(d) = directory(app) {
        let rows = rows(d);
        match rows.get(app.users.row.min(rows.len().saturating_sub(1))) {
            Some(Row::User(u)) => {
                lines.push(kv("Person", u.name.clone()));
                lines.push(kv("Added", format!("{} · {}", date(u.created), if u.source == "manual" { "by code" } else { "Windows sign-in" })));
                lines.push(kv("Computers", u.devices.len().to_string()));
                lines.push(kv("Seat plan", u.seat_plan.clone().unwrap_or_else(|| "none · p sets it".into())));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    if u.revoked { "r restores them. del removes them and their computers." } else { "r revokes them on every computer. del removes them and their computers." },
                    theme::muted(),
                )));
            }
            Some(Row::Device(u, dev)) => {
                let (status, color) = device_status(u, dev);
                lines.push(kv("Computer", dev.name.clone()));
                lines.push(kv("Person", u.name.clone()));
                lines.push(Line::from(vec![Span::styled(format!("{:<12}", "Status"), theme::faint()), Span::styled(status, theme::fg(color))]));
                lines.push(kv("Connected", date(dev.created)));
                lines.push(kv("Token ends", format!("{} {}", date(dev.expires), &comptroller_admin::local_clock(dev.expires)[..5])));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    "s suspends or resumes it at once. r revokes it for good. del forgets it, so it can connect again as new.",
                    theme::muted(),
                )));
            }
            Some(Row::Code(c)) => {
                lines.push(kv("Code for", format!("{} · {}", c.user, c.device)));
                lines.push(kv("Made", ago(now() - c.created)));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("Only its fingerprint is stored, so it can't be shown again. del cancels it.", theme::muted())));
            }
            None => lines.push(Line::from(Span::styled("Nothing yet.", theme::faint()))),
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Changes apply to the next request. No restart needed.", theme::faint())));
    lines.push(Line::from(Span::styled("TODAY counts requests since midnight. Radar (r on the Terminal) shows each one.", theme::faint())));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn dialog(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(theme::BORDER_FOCUS))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(Span::styled(format!(" {title} "), theme::bold(theme::TEXT))))
        .padding(Padding::new(2, 2, 1, 0))
}

fn draw_form(f: &mut Frame, app: &App, form: &CodeForm, screen: Rect) {
    let field = |label: &str, value: &str, example: &str, active: bool| {
        let mut spans = vec![Span::styled(format!("{label:<10}"), theme::muted()), Span::raw(" ")];
        spans.push(Span::styled(value.to_string(), if active { theme::bold(theme::TEXT) } else { theme::text() }));
        if active {
            spans.push(Span::styled(" ", Style::new().bg(theme::ACCENT)));
        }
        if value.is_empty() {
            spans.push(Span::styled(format!(" {example}"), theme::faint()));
        }
        Line::from(spans)
    };
    let hours = app.snapshot.config_or_default().identity.code_lifetime_hours;
    let mut lines = vec![
        Line::from(Span::styled("Who is it for, and which computer? The code connects that computer once.", theme::muted())),
        Line::default(),
        field("Person", &form.user, "their name or sign-in, such as jdoe", form.field == 0),
        field("Computer", &form.device, "the machine's name, such as LAPTOP-1", form.field == 1),
        Line::default(),
        Line::from(Span::styled(format!("It works for {hours} h (Settings → Identity → Code lifetime)."), theme::faint())),
    ];
    if let Some(error) = &form.error {
        lines.push(Line::from(Span::styled(error.clone(), theme::fg(theme::PINK))));
    }
    lines.push(Line::default());
    let mut keys = keycap("tab", "next field");
    keys.extend(keycap("enter", "create"));
    keys.extend(keycap("esc", "cancel"));
    lines.push(Line::from(keys).right_aligned());
    let area = centered(screen, 76, lines.len() as u16 + 4);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(dialog("New one-time code")), area);
}

fn draw_code(f: &mut Frame, code: &NewCode, screen: Rect) {
    let mut lines = vec![
        Line::from(Span::styled(code.code.clone(), theme::bold(theme::ACCENT))).centered(),
        Line::default(),
        Line::from(Span::styled(
            format!("Give this to {} to type into claude-comptroller-connect on {}. It works once, within {} h.", code.user, code.device, code.hours),
            theme::text(),
        )),
        Line::default(),
        Line::from(Span::styled("It can't be shown again: only its fingerprint is stored.", theme::fg(theme::ORANGE))),
        Line::default(),
    ];
    let mut keys = keycap("c", "copy");
    keys.extend(keycap("enter", "done"));
    lines.push(Line::from(keys).right_aligned());
    let area = centered(screen, 72, lines.len() as u16 + 5);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(dialog("One-time code")), area);
}
