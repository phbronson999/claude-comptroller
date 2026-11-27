//! Drawing. The frame (header · body · footer), panels and keycaps follow Code Keepsake's TUI.

use comptroller_admin::config::SERVICE_EXE;
use comptroller_admin::{activity, health};
use comptroller_admin::install::EMBEDDED;
use comptroller_admin::service::State;
use comptroller_admin::{bytes, long_duration, short_duration, SCHEMA_VERSION};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{menu_items, Action, App, Busy, Confirm, Screen, StepState, ToastKind, SECTIONS};
use crate::{logo, theme};
use crate::widgets::{centered, keycap, ratio_bar, section, spinner};

pub fn draw(f: &mut Frame, app: &App) {
    // The Commands panel needs room: hidden on short terminals even when switched on.
    let commands_height = if app.show_commands && f.area().height >= 30 { 8 } else { 0 };
    let [header, body, commands, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(commands_height),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(f.area());
    if commands_height > 0 {
        draw_commands(f, app, commands);
    }

    draw_header(f, app, header);
    match app.screen {
        Screen::Terminal => draw_terminal(f, app, body),
        Screen::Status => draw_status(f, app, body),
        Screen::Preflight => draw_preflight(f, app, body),
        Screen::Install | Screen::Uninstall => draw_steps(f, app, body),
    }
    draw_footer(f, app, footer);
    draw_toast(f, app, body);
    if let Some(confirm) = &app.confirm {
        draw_confirm(f, app, confirm, f.area());
    }
    if app.show_help {
        draw_help(f, f.area());
    }
}

fn panel(title: &str, focus: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(if focus { theme::BORDER_FOCUS } else { theme::BORDER }))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(Span::styled(format!(" {title} "), theme::text())))
        .padding(Padding::new(2, 2, 1, 0))
}

fn kv(key: &str, width: usize, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{key:<width$}"), theme::muted())];
    spans.extend(value);
    Line::from(spans)
}

fn plain(text: impl Into<String>) -> Vec<Span<'static>> {
    vec![Span::styled(text.into(), theme::text())]
}

fn colored(text: impl Into<String>, color: Color) -> Vec<Span<'static>> {
    vec![Span::styled(text.into(), theme::fg(color))]
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s => format!("{} ago", short_duration(s as u64)),
    }
}

// ── header ──────────────────────────────────────────────────────────────────

/// The one-word state in the header and its color, with a spinner while something changes.
fn status(app: &App) -> (String, Color, bool) {
    let s = &app.snapshot;
    if let Some(job) = &app.steps {
        if !job.finished {
            let word = if app.screen == Screen::Install { "installing…" } else { "uninstalling…" };
            return (word.into(), theme::ORANGE, true);
        }
    }
    match &app.busy {
        Some(Busy::Starting) => return ("starting…".into(), theme::ORANGE, true),
        Some(Busy::Stopping) => return ("stopping…".into(), theme::ORANGE, true),
        None => {}
    }
    if s.config.is_err() || s.health.as_ref().is_some_and(|h| !h.config_ok()) {
        if s.health.is_some() || s.state() == State::Running {
            return ("● config error".into(), theme::ORANGE, false);
        }
    }
    match s.state() {
        _ if s.console() => ("● running · console".into(), theme::OK, false),
        State::Running => ("● running".into(), theme::OK, false),
        State::Starting => ("starting…".into(), theme::ORANGE, true),
        State::Stopping => ("stopping…".into(), theme::ORANGE, true),
        State::Stopped | State::Other => ("● stopped".into(), theme::PINK, false),
        State::NotInstalled => ("○ not installed".into(), theme::FAINT, false),
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![
        Span::raw(" "),
        Span::styled("claude-comptroller", theme::bold(theme::ACCENT)),
        Span::styled(" · ", theme::faint()),
        Span::styled(app.screen.title(), theme::text()),
    ];
    let org = app.snapshot.config_or_default().general.organization_name;
    if !org.is_empty() {
        left.push(Span::styled(format!("   {org}"), theme::muted()));
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);

    let (word, color, spinning) = status(app);
    let mut right = Vec::new();
    if spinning {
        right.push(Span::styled(format!("{} ", spinner(app.tick)), theme::fg(color)));
    }
    right.push(Span::styled(word, theme::fg(color)));
    if let Some(h) = app.snapshot.health.as_ref().filter(|_| !spinning) {
        right.push(Span::styled(format!(" · up {} ", short_duration(h.uptime_seconds)), theme::faint()));
    } else {
        right.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(right).right_aligned()), area);
}

// ── terminal (home) ─────────────────────────────────────────────────────────

fn description(app: &App, action: Action) -> (String, bool) {
    let s = &app.snapshot;
    match action {
        Action::Later(step) => (format!("Arrives in build step {step}"), false),
        Action::Open(Screen::Status) => ("Service, config and connection health".into(), true),
        Action::Open(Screen::Preflight) => ("Send one request end to end".into(), true),
        Action::Open(Screen::Install) => match s.installed() {
            Some(i) => (format!("Installed · {}", i.exe.parent().map(|p| p.display().to_string()).unwrap_or_default()), false),
            None => ("Write the service, register it, start it".into(), true),
        },
        Action::Uninstall => ("Stop and remove the service".into(), s.installed().is_some()),
        Action::Takeoff => match s.state() {
            State::Running => ("Start the service · already running".into(), false),
            _ if s.console() => ("Start the service · running in a console".into(), false),
            State::NotInstalled => ("Start the service · not installed".into(), false),
            _ => ("Start the service".into(), true),
        },
        Action::Terminate => (
            "Stop the service gracefully".into(),
            s.state() == State::Running,
        ),
        Action::Open(_) => (String::new(), true),
    }
}

fn draw_terminal(f: &mut Frame, app: &App, body: Rect) {
    let [menu, side] = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).spacing(1).areas(body);

    let block = panel("Terminal", true);
    let width = block.inner(menu).width as usize;
    let mut lines = Vec::new();
    let mut index = 0;
    for (name, items) in SECTIONS {
        lines.push(section(name, None, width));
        for item in items {
            let (desc, enabled) = description(app, item.action);
            let selected = index == app.selected;
            let title_style = if enabled { theme::text() } else { theme::faint() };
            let mut spans = vec![
                Span::styled(if selected { "▍" } else { " " }, theme::fg(theme::ACCENT)),
                Span::styled(format!(" {} ", item.key), theme::bold(theme::ORANGE)),
                Span::styled(format!("{:<16}", item.title), title_style),
                Span::styled(desc, if enabled { theme::muted() } else { theme::faint() }),
            ];
            if selected {
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            }
            let line = Line::from(spans);
            lines.push(if selected { line.style(Style::new().bg(theme::SELECTION)) } else { line });
            index += 1;
        }
        lines.push(Line::default());
    }
    debug_assert_eq!(index, menu_items().count());
    f.render_widget(Paragraph::new(lines).block(block), menu);

    // The radar scope above "At a glance", when there's room for both.
    let side = if side.width >= logo::MIN_WIDTH && side.height >= logo::ROWS + 14 {
        let [top, rest] = Layout::vertical([Constraint::Length(logo::ROWS), Constraint::Fill(1)]).spacing(1).areas(side);
        logo::draw(f, top, beam(app), app.tick);
        rest
    } else {
        side
    };
    draw_glance(f, app, side);
}

fn beam(app: &App) -> logo::Beam {
    let s = &app.snapshot;
    match s.state() {
        _ if status(app).2 => logo::Beam::Changing,
        _ if s.console() => logo::Beam::Sweeping,
        State::Running => logo::Beam::Sweeping,
        State::NotInstalled => logo::Beam::Off,
        _ => logo::Beam::Stopped,
    }
}

fn draw_glance(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snapshot;
    let config = s.config_or_default();
    let block = panel("At a glance", false);
    let width = block.inner(area).width as usize;
    let w = 14;

    let listen = s.health.as_ref().map(|h| h.listen_url.clone()).unwrap_or(config.proxy.listen_url.clone());
    let config_line = match (&s.config, &s.health) {
        (Err(e), _) => colored(format!("{e}"), theme::ORANGE),
        (_, Some(h)) if !h.config_ok() => colored(format!("rejected · {}", h.config.error.clone().unwrap_or_default()), theme::ORANGE),
        (_, Some(h)) => colored(
            format!("ok · loaded {}", health::parse_utc(&h.config.loaded_utc).map(|t| ago(health::now() - t)).unwrap_or_default()),
            theme::OK,
        ),
        _ => plain(s.place.config_path.display().to_string()),
    };
    let mut lines = vec![
        kv("Listening", w, plain(listen)),
        kv("Upstream", w, plain(config.proxy.upstream_url.clone())),
        kv("Sign-in", w, plain(match config.identity.mode.as_str() {
            "windows" => format!("Windows · {}", config.identity.allowed_group),
            other => other.to_string(),
        })),
        kv("Black box", w, plain(s.db_size.map(bytes).unwrap_or_else(|| "empty".into()))),
        kv("Config", w, config_line),
        Line::default(),
        section("NEED TO KNOW", None, width),
    ];

    let mut notes: Vec<(Color, String)> = Vec::new();
    if let Err(e) = &s.service {
        notes.push((theme::ORANGE, e.clone()));
    }
    match s.state() {
        _ if s.console() => notes.push((theme::ACCENT, "Running in a console window, not as a Windows Service.".into())),
        State::NotInstalled => notes.push((theme::FAINT, "Not installed yet: press i.".into())),
        State::Stopped => notes.push((theme::PINK, "Stopped. Claude Code on developer computers can't connect: press t.".into())),
        _ => {}
    }
    if let Some(h) = &s.health {
        if h.schema_version != SCHEMA_VERSION {
            notes.push((theme::ORANGE, format!("Database schema {} but this terminal expects {SCHEMA_VERSION}. Update both together.", h.schema_version)));
        }
    }
    if let Some(Err(e)) = &s.upstream {
        notes.push((theme::PINK, format!("Can't reach the upstream through the proxy: {e}")));
    }
    if !app.elevated {
        notes.push((theme::FAINT, "Not elevated: Install, Takeoff, Terminate and Uninstall will offer to reopen as administrator.".into()));
    }
    if EMBEDDED.is_none() && s.installed().is_none() {
        notes.push((theme::ORANGE, format!("This build has no {SERVICE_EXE} built in, so it can't install. Use build.ps1.")));
    }
    if let Some((passed, at)) = app.last_preflight {
        let when = ago(at.elapsed().as_secs() as i64);
        notes.push(if passed { (theme::OK, format!("Preflight passed {when}")) } else { (theme::PINK, format!("Preflight failed {when}: press p")) });
    }
    if notes.is_empty() {
        notes.push((theme::FAINT, "Nothing needs attention.".into()));
    }
    for (color, text) in notes {
        lines.push(Line::from(vec![Span::styled("● ", theme::fg(color)), Span::styled(text, theme::text())]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

// ── status ──────────────────────────────────────────────────────────────────

fn draw_status(f: &mut Frame, app: &App, body: Rect) {
    let s = &app.snapshot;
    let config = s.config_or_default();
    let [left, right] = Layout::horizontal([Constraint::Percentage(49), Constraint::Percentage(51)]).spacing(1).areas(body);
    let [service_area, proxy_area] = Layout::vertical([Constraint::Length(11), Constraint::Min(6)]).spacing(1).areas(left);
    let w = 16;

    let (word, color, _) = status(app);
    let installed = s.installed();
    let not_installed = || plain(if s.console() { "— · console run" } else { "—" });
    let service_lines = vec![
        kv("State", w, colored(word, color)),
        kv("Uptime", w, plain(s.health.as_ref().map(|h| long_duration(h.uptime_seconds)).unwrap_or_else(|| "—".into()))),
        kv("Version", w, plain(format!(
            "service {} · tui {}",
            s.health.as_ref().map(|h| h.version.as_str()).unwrap_or("—"),
            env!("CARGO_PKG_VERSION")
        ))),
        kv("Service name", w, plain(s.service_name.clone())),
        kv("Start type", w, installed.map(|i| plain(i.start_type.clone())).unwrap_or_else(not_installed)),
        kv("On crash", w, installed.map(|i| plain(i.recovery.clone())).unwrap_or_else(not_installed)),
        kv("Runs as", w, installed.map(|i| plain(i.account.clone())).unwrap_or_else(not_installed)),
    ];
    f.render_widget(Paragraph::new(service_lines).block(panel("Service", false)), service_area);

    let listen = s.health.as_ref().map(|h| h.listen_url.clone()).unwrap_or(config.proxy.listen_url.clone());
    let mut proxy_lines = vec![
        kv("Listen URL", w, plain(listen.clone())),
        kv("Admin port", w, plain(format!("127.0.0.1:{}", config.proxy.admin_port))),
        kv("Upstream", w, plain(config.proxy.upstream_url.clone())),
        kv("Allowed CIDRs", w, if config.access.allowed_cidrs.is_empty() {
            colored("any address (empty list)", theme::ORANGE)
        } else {
            plain(config.access.allowed_cidrs.join(", "))
        }),
        kv("Certificate", w, if listen.starts_with("https") { plain("configured outside comptroller.json") } else { colored("none · plain http", theme::FAINT) }),
        kv("Config file", w, plain(s.place.config_path.display().to_string())),
    ];
    if let Some(h) = &s.health {
        if h.listen_url != config.proxy.listen_url {
            proxy_lines.push(Line::default());
            proxy_lines.push(Line::from(Span::styled(
                format!("The file says {}: restart the service to use it.", config.proxy.listen_url),
                theme::fg(theme::ORANGE),
            )));
        }
    }
    f.render_widget(Paragraph::new(proxy_lines).wrap(Wrap { trim: false }).block(panel("Proxy", false)), proxy_area);

    // Health
    let block = panel("Health", true);
    let width = block.inner(right).width as usize;
    let check = |ok: Option<bool>, name: &str, detail: String| {
        let (mark, color) = match ok {
            Some(true) => ("✓", theme::OK),
            Some(false) => ("!", theme::ORANGE),
            None => ("·", theme::FAINT),
        };
        Line::from(vec![
            Span::styled(format!("{mark} "), theme::fg(color)),
            Span::styled(format!("{name:<22}"), theme::text()),
            Span::styled(detail, theme::muted()),
        ])
    };
    let file = s.place.config_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let mut lines = vec![match (&s.config, &s.health) {
        (Err(e), _) => check(Some(false), "Config", format!("{file} is {e}")),
        (_, Some(h)) if !h.config_ok() => check(Some(false), "Config rejected", h.config.error.clone().unwrap_or_default()),
        (_, Some(h)) => check(Some(true), "Config loaded", format!(
            "{file} · {}",
            health::parse_utc(&h.config.loaded_utc).map(|t| ago(health::now() - t)).unwrap_or_default()
        )),
        _ => check(None, "Config", format!("{file} · service not answering")),
    }];
    lines.push(match &s.health {
        Some(h) if h.schema_version == SCHEMA_VERSION => check(Some(true), "Database schema", format!("version {} · matches TUI", h.schema_version)),
        Some(h) => check(Some(false), "Database schema", format!("version {} · TUI expects {SCHEMA_VERSION}", h.schema_version)),
        None => check(None, "Database schema", "service not answering".into()),
    });
    let upstream_host = config.proxy.upstream_url.split("://").nth(1).unwrap_or(&config.proxy.upstream_url).trim_end_matches('/').to_string();
    lines.push(match &s.upstream {
        Some(Ok((code, took))) if *code < 500 => check(Some(true), "Upstream reachable", format!("{upstream_host} · {} ms", took.as_millis())),
        Some(Ok((code, _))) => check(Some(false), "Upstream reachable", format!("{upstream_host} · proxy answered {code}")),
        Some(Err(e)) => check(Some(false), "Upstream reachable", e.clone()),
        None => check(None, "Upstream reachable", "service not answering".into()),
    });
    lines.push(check(
        s.db_size.map(|_| true),
        "Black box size",
        s.db_size.map(bytes).unwrap_or_else(|| "no database yet".into()),
    ));
    lines.push(Line::default());
    lines.push(section("UNDER THE HOOD", None, width));
    lines.push(Line::from(Span::styled("This screen reads:", theme::faint())));
    for command in [
        format!("sc.exe query {}", s.service_name),
        format!("sc.exe qc {}", s.service_name),
        format!("GET {}", health::url(config.proxy.admin_port)),
        format!("HEAD {}/api/hello", comptroller_admin::config::local_url(&listen)),
    ] {
        lines.push(Line::from(vec![Span::styled("$ ", theme::faint()), Span::styled(command, theme::text())]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), right);
}

// ── preflight ───────────────────────────────────────────────────────────────

fn draw_preflight(f: &mut Frame, app: &App, body: Rect) {
    let block = panel("Preflight test", true);
    let width = block.inner(body).width as usize;
    let total = 5;
    let passed = app.preflight.iter().filter(|c| c.passed()).count();
    let summary = match (app.preflight_running, app.preflight_took) {
        (true, _) => format!("{} of {total}", app.preflight.len()),
        (false, Some(took)) => format!("{passed} of {} passed · {} ms", app.preflight.len(), took.as_millis()),
        _ => String::new(),
    };
    let mut lines = vec![section("CHECKS", Some(summary), width)];
    for check in &app.preflight {
        let (mark, color, outcome) = match &check.result {
            Ok(text) => ("✓", theme::OK, text.clone()),
            Err(_) => ("✗", theme::PINK, "failed".to_string()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), theme::fg(color)),
            Span::styled(format!("{:<32}", check.name), theme::text()),
            Span::styled(format!("{:<44}", truncate(&check.detail, 42)), theme::muted()),
            Span::styled(format!("{:<18}", outcome), theme::text()),
            Span::styled(format!("{} ms", check.took.as_millis()), theme::faint()),
        ]));
        if let Err(error) = &check.result {
            lines.push(Line::from(Span::styled(format!("  {error}"), theme::fg(theme::PINK))));
        }
    }
    if app.preflight_running {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", spinner(app.tick)), theme::fg(theme::ACCENT)),
            Span::styled("checking…", theme::muted()),
        ]));
    }
    f.render_widget(Paragraph::new(lines.clone()).block(block.clone()), body);

    let inner = block.inner(body);
    let mut y = inner.y + lines.len() as u16 + 1;
    if !app.preflight_running && !app.preflight.is_empty() {
        let ok = app.preflight.iter().all(|c| c.passed());
        let color = if ok { theme::OK } else { theme::PINK };
        let text = if ok {
            "The path from a developer computer to the upstream works, and replies stream without buffering. The test sends no credentials, so it never uses anyone's seat."
        } else {
            "Fix the first failed check and run it again. Status shows more detail."
        };
        let height = 4 + (text.len() as u16 / inner.width.max(1));
        let area = Rect { x: inner.x, y, width: inner.width, height: height.min(inner.bottom().saturating_sub(y)) };
        let summary = Block::bordered().border_type(BorderType::Plain).border_style(theme::fg(color)).padding(Padding::horizontal(2));
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(if ok { "Preflight passed" } else { "Preflight failed" }, theme::bold(color))),
                Line::from(Span::styled(text, theme::muted())),
            ])
            .wrap(Wrap { trim: false })
            .block(summary),
            area,
        );
        y += area.height + 1;
    }

    let config = app.snapshot.config_or_default();
    let base = comptroller_admin::config::local_url(
        &app.snapshot.health.as_ref().map(|h| h.listen_url.clone()).unwrap_or(config.proxy.listen_url),
    );
    if y + 4 < inner.bottom() {
        let area = Rect { x: inner.x, y, width: inner.width, height: inner.bottom() - y };
        let lines = vec![
            section("THE TEST REQUESTS", None, inner.width as usize),
            Line::from(vec![Span::styled("$ ", theme::faint()), Span::styled(format!("curl -I {base}/api/hello"), theme::text())]),
            Line::from(vec![
                Span::styled("$ ", theme::faint()),
                Span::styled(format!("curl -N -H \"X-Comptroller-Client: preflight\" {base}/_comptroller/preflight/stream"), theme::text()),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

// ── install / uninstall ─────────────────────────────────────────────────────

fn draw_steps(f: &mut Frame, app: &App, body: Rect) {
    let title = app.screen.title();
    let block = panel(title, true);
    let width = block.inner(body).width as usize;

    let plan = app.steps.as_ref().map(|j| &j.plan);
    let preview;
    let (steps, states): (&[_], Vec<StepState>) = match (&app.steps, plan) {
        (Some(job), Some(plan)) => (&plan.steps, job.states.clone()),
        _ => {
            preview = comptroller_admin::install::install_plan(&app.snapshot.service_name, &app.install_dir);
            (&preview.steps[..], vec![StepState::Pending; preview.steps.len()])
        }
    };

    let done = states.iter().filter(|s| matches!(s, StepState::Done(_))).count();
    let mut lines = vec![section("STEPS", Some(format!("{done} of {}", steps.len())), width)];
    lines.push(Line::from(ratio_bar(done as f64 / steps.len().max(1) as f64, width, theme::ACCENT)));
    lines.push(Line::default());

    for (step, state) in steps.iter().zip(&states) {
        let (mark, mark_color, title_style) = match state {
            StepState::Done(_) => ("✓".to_string(), theme::OK, theme::text()),
            StepState::Running => (spinner(app.tick).to_string(), theme::ORANGE, theme::text()),
            StepState::Failed(_) => ("✗".to_string(), theme::PINK, theme::text()),
            StepState::Pending => ("·".to_string(), theme::FAINT, theme::faint()),
        };
        lines.push(Line::from(vec![Span::styled(format!("{mark} "), theme::fg(mark_color)), Span::styled(step.title.clone(), title_style)]));
        let detail_style = if matches!(state, StepState::Pending) { theme::faint() } else { theme::muted() };
        match state {
            StepState::Done(detail) => lines.push(Line::from(Span::styled(format!("  {detail}"), detail_style))),
            StepState::Failed(error) => lines.push(Line::from(Span::styled(format!("  {error}"), theme::fg(theme::PINK)))),
            _ => lines.push(Line::from(Span::styled(format!("  {}", step.detail), detail_style))),
        }
        if let Some(command) = &step.command {
            lines.push(Line::from(Span::styled(format!("  {command}"), theme::faint())));
        }
        lines.push(Line::default());
    }

    match &app.steps {
        None if app.screen == Screen::Install => {
            if EMBEDDED.is_none() {
                lines.push(Line::from(Span::styled(
                    format!("This build has no {SERVICE_EXE} built in. Run build.ps1, or publish the service first and rebuild."),
                    theme::fg(theme::ORANGE),
                )));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("Installs to ", theme::muted()),
                    Span::styled(app.install_dir.display().to_string(), theme::text()),
                    Span::styled(". Press enter to start.", theme::muted()),
                ]));
            }
        }
        Some(job) if job.finished && job.failed() => {
            lines.push(Line::from(Span::styled("Stopped at the failed step. Nothing after it was done.", theme::fg(theme::PINK))));
        }
        Some(job) if job.finished => {
            lines.push(Line::from(Span::styled(format!("{title} finished."), theme::fg(theme::OK))));
        }
        _ => {}
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), body);
}

// ── commands ────────────────────────────────────────────────────────────────

fn draw_commands(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(theme::BORDER))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(vec![
            Span::styled(" Commands ", theme::text()),
            Span::styled("· what's running, as you'd type it · ↻ = routine status check ", theme::faint()),
        ]))
        .title(Line::from(Span::styled(" o hide ", theme::faint())).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let width = inner.width as usize;
    let lines: Vec<Line> = activity::recent(inner.height as usize)
        .into_iter()
        .map(|entry| {
            let (mark, color, summary) = match &entry.outcome {
                None => (spinner(app.tick).to_string(), theme::ORANGE, "running…".to_string()),
                Some(o) if o.ok => ("✓".into(), theme::OK, o.summary.clone()),
                Some(o) => ("✗".into(), theme::PINK, o.summary.clone()),
            };
            let repeats = match (entry.background, entry.repeats) {
                (true, _) => format!(" ↻ every {} s", crate::app::POLL_SECS),
                (false, n) if n > 1 => format!(" ×{n}"),
                _ => String::new(),
            };
            let right = format!("{summary}{repeats}");
            let room = width.saturating_sub(right.chars().count() + 15);
            let command = truncate(&entry.command, room.max(10));
            let pad = width.saturating_sub(command.chars().count() + right.chars().count() + 13);
            Line::from(vec![
                Span::styled(format!("{} ", entry.clock), theme::faint()),
                Span::styled(format!("{mark} "), theme::fg(color)),
                Span::styled("$ ", theme::faint()),
                Span::styled(command, if entry.background { theme::muted() } else { theme::bold(theme::TEXT) }),
                Span::raw(" ".repeat(pad)),
                Span::styled(summary, if entry.outcome.as_ref().is_some_and(|o| !o.ok) { theme::fg(theme::PINK) } else { theme::muted() }),
                Span::styled(repeats, theme::faint()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

// ── footer, toast, overlays ─────────────────────────────────────────────────

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let keys: Vec<(&str, &str)> = if app.confirm.is_some() {
        match app.confirm {
            Some(Confirm::Uninstall { .. }) => vec![("y", "uninstall"), ("d", "also remove data"), ("n", "cancel")],
            Some(Confirm::Elevate(_)) => vec![("y", "reopen as administrator"), ("n", "cancel")],
            _ => vec![("y", "terminate"), ("n", "cancel")],
        }
    } else {
        match app.screen {
            Screen::Terminal => vec![("↑↓", "move"), ("enter", "open"), ("s", "Status"), ("p", "Preflight"), ("t", "Takeoff"), ("x", "Terminate"), ("?", "help"), ("q", "quit")],
            Screen::Status => vec![("esc", "Terminal"), ("p", "Preflight test"), ("t", "Takeoff"), ("x", "Terminate"), ("l", "open logs"), ("r", "refresh")],
            Screen::Preflight => vec![("esc", "Terminal"), ("enter", "run again"), ("s", "Status")],
            Screen::Install if app.steps.is_none() => vec![("enter", "install"), ("esc", "back")],
            Screen::Install | Screen::Uninstall if app.working() => vec![("l", "open logs")],
            Screen::Install | Screen::Uninstall => vec![("esc", "Terminal"), ("l", "open logs")],
        }
    };
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in keys {
        spans.extend(keycap(key, label));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if app.confirm.is_none() {
        let label = if app.show_commands { "hide commands" } else { "show commands" };
        f.render_widget(Paragraph::new(Line::from(keycap("o", label)).right_aligned()), area);
    }
}

fn draw_toast(f: &mut Frame, app: &App, body: Rect) {
    let Some(toast) = &app.toast else { return };
    let (icon, color) = match toast.kind {
        ToastKind::Info => ("●", theme::ACCENT),
        ToastKind::Success => ("✓", theme::OK),
        ToastKind::Warn => ("◆", theme::WARN),
    };
    let width = (toast.text.chars().count() as u16 + 8).min(body.width.saturating_sub(4));
    if width < 10 || body.height < 4 {
        return;
    }
    let area = Rect { x: body.right().saturating_sub(width + 2), y: body.bottom().saturating_sub(4), width, height: 3 };
    f.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Plain).border_style(theme::fg(color)).padding(Padding::horizontal(1));
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(format!("{icon} "), theme::fg(color)), Span::styled(toast.text.clone(), theme::text())])).block(block),
        area,
    );
}

fn draw_confirm(f: &mut Frame, app: &App, confirm: &Confirm, screen: Rect) {
    let name = app.snapshot.service_name.clone();
    let config = app.snapshot.config_or_default();
    let (title, color, mut lines, yes) = match confirm {
        Confirm::Terminate => (
            "Terminate",
            theme::PINK,
            vec![
                Line::from(Span::styled(format!("Stop the {name} service?"), theme::text())),
                Line::default(),
                Line::from(vec![
                    Span::styled("Requests in flight", theme::fg(theme::ORANGE)),
                    Span::styled(format!(" finish first (up to {} s). ", config.proxy.shutdown_timeout_seconds), theme::muted()),
                ]),
                Line::from(Span::styled("Until Takeoff, Claude Code on every developer computer will fail to connect.", theme::muted())),
                Line::default(),
                Line::from(vec![Span::styled("Equivalent: ", theme::muted()), Span::styled(format!("sc.exe stop {name}"), theme::text())]),
            ],
            "terminate",
        ),
        Confirm::Uninstall { remove_data } => {
            let place = &app.snapshot.place;
            let mut lines = vec![
                Line::from(Span::styled(format!("Stop and remove the {name} service?"), theme::text())),
                Line::default(),
                Line::from(Span::styled("Claude Code on developer computers stops working until it's installed again.", theme::muted())),
                Line::default(),
                Line::from(vec![
                    Span::styled(if *remove_data { "[x] " } else { "[ ] " }, theme::fg(if *remove_data { theme::PINK } else { theme::MUTED })),
                    Span::styled("Also remove data, logs and config (d)", theme::text()),
                ]),
            ];
            let kept = format!("  {}  ·  {}", place.data_dir(&config).display(), place.config_path.display());
            lines.push(Line::from(Span::styled(if *remove_data { format!("{kept}  will be deleted") } else { format!("{kept}  are kept") }, theme::faint())));
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled("Equivalent: ", theme::muted()), Span::styled(format!("sc.exe stop {name} && sc.exe delete {name}"), theme::text())]));
            ("Uninstall", theme::PINK, lines, "uninstall")
        }
        Confirm::Elevate(what) => (
            "Administrator needed",
            theme::ORANGE,
            vec![
                Line::from(Span::styled(format!("{what} changes a Windows service, which needs an elevated terminal."), theme::text())),
                Line::default(),
                Line::from(Span::styled("Reopen claude-comptroller as administrator? Windows will ask first.", theme::muted())),
            ],
            "reopen",
        ),
    };
    lines.push(Line::default());
    lines.push(
        Line::from(vec![
            Span::styled(" n ", theme::bold(theme::ORANGE)),
            Span::styled(" cancel    ", theme::text()),
            Span::styled(" y ", Style::new().fg(theme::TEXT).bg(color)),
            Span::styled(format!(" {yes} "), theme::bold(color)),
        ])
        .right_aligned(),
    );
    let area = centered(screen, 86, lines.len() as u16 + 4);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(theme::BORDER_FOCUS))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(Span::styled(format!(" {title} "), theme::bold(theme::TEXT))))
        .padding(Padding::new(2, 2, 1, 0));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn draw_help(f: &mut Frame, screen: Rect) {
    let area = centered(screen, 70, 22);
    f.render_widget(Clear, area);
    let block = panel("Keys", true).padding(Padding::new(2, 2, 1, 1));
    let row = |key: &str, text: &str| {
        let mut spans = keycap(key, "");
        spans.pop();
        spans.push(Span::raw(" ".repeat(9usize.saturating_sub(key.chars().count() + 2))));
        spans.push(Span::styled(text.to_string(), theme::text()));
        Line::from(spans)
    };
    let mut lines = vec![
        row("↑ ↓", "choose on the Terminal (also j k, tab)"),
        row("enter", "open the highlighted item"),
        row("letter", "each item's key opens it directly"),
        row("esc", "back to the Terminal"),
        row("o", "hide or show the Commands panel"),
        row("q", "quit (waits for an install or stop to finish)"),
        Line::default(),
    ];
    for (name, items) in SECTIONS {
        let keys: Vec<String> = items.iter().map(|i| format!("{} {}", i.key, i.title)).collect();
        lines.push(Line::from(vec![Span::styled(format!("{name:<10}"), theme::label()), Span::styled(keys.join(" · "), theme::muted())]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Press any key to close", theme::faint())).centered());
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

