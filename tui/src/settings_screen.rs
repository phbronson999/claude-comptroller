//! The Settings screen: every value in comptroller.json, grouped, with its default and a
//! line on what it does. Changes wait until `s` saves them all at once; settings the
//! service reads only at startup are listed under NEEDS A RESTART, and saving one offers
//! to restart the service.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use comptroller_admin::activity;
use comptroller_admin::service::State as ServiceState;
use comptroller_admin::settings::{self, Kind, GROUPS, SETTINGS};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use serde_json::Value;

use crate::app::{App, Confirm, Screen, ToastKind};
use crate::theme;
use crate::ui::panel;
use crate::widgets::section;

#[derive(Default)]
pub struct State {
    pub group: usize,
    pub row: usize,
    /// Changes not saved yet: setting index and new value.
    pub pending: Vec<(usize, Value)>,
    pub editing: Option<Editor>,
    /// Settings whose caution was accepted this session.
    pub acknowledged: HashSet<usize>,
    /// The file as last read.
    pub doc: Option<Result<Value, String>>,
    pub writable: bool,
    pub path: PathBuf,
}

pub struct Editor {
    pub text: String,
    /// In characters.
    pub cursor: usize,
    pub error: Option<String>,
}

impl State {
    pub fn load(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.doc = Some(activity::track(format!("type \"{}\"", path.display()), || settings::read(path), |_| "read".into()));
        self.writable = settings::writable(path);
    }

    /// The selected group's settings: those that apply at once, then those that need a restart.
    pub fn rows(&self) -> Vec<usize> {
        let (name, _) = GROUPS[self.group];
        let mut rows: Vec<usize> = settings::in_group(name).filter(|(_, s)| !s.restart).map(|(i, _)| i).collect();
        rows.extend(settings::in_group(name).filter(|(_, s)| s.restart).map(|(i, _)| i));
        rows
    }

    pub fn selected(&self) -> Option<usize> {
        self.rows().get(self.row).copied()
    }

    fn doc(&self) -> Option<&Value> {
        self.doc.as_ref().and_then(|d| d.as_ref().ok())
    }

    pub fn saved(&self, index: usize) -> Value {
        self.doc().map(|d| settings::current(d, &SETTINGS[index])).unwrap_or_else(|| settings::default_value(&SETTINGS[index]))
    }

    pub fn pending_value(&self, index: usize) -> Option<&Value> {
        self.pending.iter().find(|(i, _)| *i == index).map(|(_, v)| v)
    }

    pub fn value(&self, index: usize) -> Value {
        self.pending_value(index).cloned().unwrap_or_else(|| self.saved(index))
    }

    fn set(&mut self, index: usize, value: Value) {
        self.pending.retain(|(i, _)| *i != index);
        if value != self.saved(index) {
            self.pending.push((index, value));
        }
    }
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    if app.settings.editing.is_some() {
        return on_editor_key(app, key);
    }
    let count = app.settings.rows().len();
    let s = &mut app.settings;
    match key.code {
        KeyCode::Esc if !s.pending.is_empty() => app.confirm = Some(Confirm::Unsaved),
        KeyCode::Esc => app.screen = Screen::Terminal,
        KeyCode::Left | KeyCode::Char('h') => {
            s.group = (s.group + GROUPS.len() - 1) % GROUPS.len();
            s.row = 0;
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            s.group = (s.group + 1) % GROUPS.len();
            s.row = 0;
        }
        KeyCode::Up | KeyCode::Char('k') if count > 0 => s.row = (s.row + count - 1) % count,
        KeyCode::Down | KeyCode::Char('j') if count > 0 => s.row = (s.row + 1) % count,
        KeyCode::Enter | KeyCode::Char('e') => {
            let Some(index) = s.selected() else { return };
            if let Some(Err(e)) = &s.doc {
                let e = e.clone();
                app.toast(ToastKind::Warn, format!("comptroller.json is {e}. Fix it in a text editor first."));
            } else if !s.writable {
                let path = s.path.display().to_string();
                if !app.needs_admin("Changing settings") {
                    app.toast(ToastKind::Warn, format!("Can't write {path}."));
                }
            } else if SETTINGS[index].caution.is_some() && !s.acknowledged.contains(&index) {
                app.confirm = Some(Confirm::Caution(index));
            } else {
                edit(app);
            }
        }
        KeyCode::Char('z') => {
            if let Some(index) = s.selected() {
                s.set(index, settings::default_value(&SETTINGS[index]));
            }
        }
        KeyCode::Char('u') => {
            if let Some(index) = s.selected() {
                s.pending.retain(|(i, _)| *i != index);
            }
        }
        KeyCode::Char('s') => save(app),
        KeyCode::Char('r') if s.pending.is_empty() => {
            let path = s.path.clone();
            s.load(&path);
            app.toast(ToastKind::Info, "Read comptroller.json again.");
        }
        _ => {}
    }
}

/// Starts editing the selected setting. On/off and choices switch to the next value at once.
pub fn edit(app: &mut App) {
    let s = &mut app.settings;
    let Some(index) = s.selected() else { return };
    let value = s.value(index);
    match SETTINGS[index].kind {
        Kind::Bool => s.set(index, Value::Bool(!value.as_bool().unwrap_or(false))),
        Kind::Choice(choices) => {
            let at = choices.iter().position(|c| value.as_str() == Some(c)).map(|p| p + 1).unwrap_or(0);
            s.set(index, Value::String(choices[at % choices.len()].into()));
        }
        _ => {
            let text = settings::show(&value);
            s.editing = Some(Editor { cursor: text.chars().count(), text, error: None });
        }
    }
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(text.len())
}

fn on_editor_key(app: &mut App, key: KeyEvent) {
    let s = &mut app.settings;
    let Some(index) = s.selected() else {
        s.editing = None;
        return;
    };
    let Some(editor) = &mut s.editing else { return };
    match key.code {
        KeyCode::Esc => s.editing = None,
        KeyCode::Enter => match settings::parse(&SETTINGS[index], &editor.text) {
            Ok(value) => {
                s.editing = None;
                s.set(index, value);
            }
            Err(e) => editor.error = Some(e),
        },
        KeyCode::Left => editor.cursor = editor.cursor.saturating_sub(1),
        KeyCode::Right => editor.cursor = (editor.cursor + 1).min(editor.text.chars().count()),
        KeyCode::Home => editor.cursor = 0,
        KeyCode::End => editor.cursor = editor.text.chars().count(),
        KeyCode::Backspace if editor.cursor > 0 => {
            editor.cursor -= 1;
            let at = byte_at(&editor.text, editor.cursor);
            editor.text.remove(at);
            editor.error = None;
        }
        KeyCode::Delete if editor.cursor < editor.text.chars().count() => {
            let at = byte_at(&editor.text, editor.cursor);
            editor.text.remove(at);
            editor.error = None;
        }
        KeyCode::Char(c) => {
            let at = byte_at(&editor.text, editor.cursor);
            editor.text.insert(at, c);
            editor.cursor += 1;
            editor.error = None;
        }
        _ => {}
    }
}

/// Saves every pending change, then says whether the service already uses them.
pub fn save(app: &mut App) {
    let s = &app.settings;
    if s.pending.is_empty() {
        return app.toast(ToastKind::Info, "Nothing to save.");
    }
    let Some(doc) = s.doc() else {
        return app.toast(ToastKind::Warn, "comptroller.json can't be read, so nothing was saved.");
    };
    if let Some(problem) = settings::check_together(doc, &s.pending).into_iter().next() {
        return app.toast(ToastKind::Warn, problem);
    }
    if !s.writable {
        let path = s.path.display().to_string();
        if !app.needs_admin("Saving settings") {
            app.toast(ToastKind::Warn, format!("Can't write {path}."));
        }
        return;
    }

    let path = s.path.clone();
    if let Err(e) = settings::save(&path, &s.pending) {
        return app.toast(ToastKind::Warn, e);
    }
    let restart: Vec<&'static str> = s.pending.iter().filter(|(i, _)| SETTINGS[*i].restart).map(|(i, _)| SETTINGS[*i].label).collect();
    let count = s.pending.len();
    app.settings.pending.clear();
    app.settings.load(&path);
    app.refresh_now();

    let running = app.snapshot.state() == ServiceState::Running || app.snapshot.console();
    let saved = if count == 1 { "Saved 1 setting".to_string() } else { format!("Saved {count} settings") };
    if restart.is_empty() {
        let tail = if running { ". The service uses it straight away." } else { "." };
        return app.toast(ToastKind::Success, format!("{saved}{tail}"));
    }
    if !running {
        return app.toast(ToastKind::Success, format!("{saved}. Takeoff starts the service with them."));
    }
    let mut waiting = app.restart_waiting.take().map(|(labels, _)| labels).unwrap_or_default();
    for label in &restart {
        if !waiting.contains(label) {
            waiting.push(label);
        }
    }
    app.restart_waiting = Some((waiting, Instant::now()));
    if app.snapshot.console() {
        app.toast(ToastKind::Info, format!("{saved}. Restart the console run to use: {}.", restart.join(", ")));
    } else {
        app.confirm = Some(Confirm::Restart(restart));
    }
}

// ── drawing ─────────────────────────────────────────────────────────────────

const LABEL: usize = 22;

/// Splits `text` into lines no wider than `width`, at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width.max(10) {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let s = &app.settings;
    let [left, right] = Layout::horizontal([Constraint::Length(28), Constraint::Fill(1)]).spacing(1).areas(body);

    // Groups
    let block = panel("Groups", false);
    let width = block.inner(left).width as usize;
    let mut lines = Vec::new();
    for (index, (name, later)) in GROUPS.iter().enumerate() {
        let selected = index == s.group;
        let unsaved = s.pending.iter().any(|(i, _)| SETTINGS[*i].section == *name);
        let mut spans = vec![
            Span::styled(if selected { "▍" } else { " " }, theme::fg(theme::ACCENT)),
            Span::styled(format!(" {name}"), if later.is_some() { theme::faint() } else { theme::text() }),
        ];
        if unsaved {
            spans.push(Span::styled(" •", theme::fg(theme::ORANGE)));
        }
        if let Some(tag) = later {
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            spans.push(Span::styled(format!("{}{tag}", " ".repeat(width.saturating_sub(used + tag.len()))), theme::faint()));
        }
        if selected {
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            lines.push(Line::from(spans).style(Style::new().bg(theme::SELECTION)));
        } else {
            lines.push(Line::from(spans));
        }
    }
    lines.push(Line::default());
    let file = s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    lines.push(Line::from(Span::styled(file, theme::faint())));
    lines.push(Line::from(Span::styled("backup: .bak", theme::faint())));
    if !s.writable && matches!(s.doc, Some(Ok(_))) {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Read-only here: saving needs an administrator terminal.", theme::fg(theme::ORANGE))));
    }
    if !s.pending.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(format!("{} unsaved", s.pending.len()), theme::fg(theme::ORANGE)),
            Span::styled(" · s saves", theme::faint()),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), left);

    // The group's settings
    let (name, later) = GROUPS[s.group];
    let block = panel(name, true);
    let inner = block.inner(right);
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_at = 0;

    if let Some(when) = later {
        let text = match when.strip_prefix("step ") {
            Some(step) => format!("These settings arrive in build step {step}."),
            None => "These settings aren't built yet.".into(),
        };
        lines.push(Line::from(Span::styled(text, theme::muted())));
    } else if let Some(Err(e)) = &s.doc {
        lines.push(Line::from(Span::styled(format!("{} is {e}.", s.path.display()), theme::fg(theme::PINK))));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Fix it in a text editor, or restore comptroller.json.bak. The service keeps its last good settings meanwhile.", theme::muted())));
    } else if s.doc.is_none() {
        lines.push(Line::from(Span::styled("Not read yet.", theme::muted())));
    } else {
        let mut restart_heading = false;
        for (row, index) in s.rows().into_iter().enumerate() {
            let setting = &SETTINGS[index];
            if setting.restart && !restart_heading {
                restart_heading = true;
                if row > 0 {
                    lines.push(Line::default());
                }
                lines.push(section("NEEDS A RESTART", None, width));
                lines.push(Line::default());
            }
            let selected = row == s.row;
            if selected {
                selected_at = lines.len();
            }
            let start = lines.len();

            let default = format!("default {}", match settings::show(&settings::default_value(setting)) {
                d if d.is_empty() => "empty".to_string(),
                d => d,
            });
            let mut first = vec![Span::styled(format!("{:<LABEL$}", setting.label), theme::text())];
            match (&s.editing, selected) {
                (Some(editor), true) => {
                    let at = byte_at(&editor.text, editor.cursor);
                    let (before, after) = editor.text.split_at(at);
                    let mut rest = after.chars();
                    let under = rest.next().map(String::from).unwrap_or_else(|| " ".into());
                    first.push(Span::styled("✎ ", theme::fg(theme::ACCENT)));
                    first.push(Span::styled(before.to_string(), theme::bold(theme::TEXT)));
                    first.push(Span::styled(under, Style::new().fg(theme::PANEL).bg(theme::ACCENT)));
                    first.push(Span::styled(rest.collect::<String>(), theme::bold(theme::TEXT)));
                }
                _ => {
                    let shown = settings::show(&s.value(index));
                    let pending = s.pending_value(index).is_some();
                    let style = if pending { theme::fg(theme::ORANGE) } else { theme::fg(theme::ACCENT) };
                    first.push(if shown.is_empty() { Span::styled("(empty)", theme::faint()) } else { Span::styled(shown, style) });
                    if pending {
                        first.push(Span::styled("  • unsaved", theme::fg(theme::ORANGE)));
                    }
                }
            }
            let used: usize = first.iter().map(|s| s.content.chars().count()).sum();
            if used + default.len() + 2 <= width {
                first.push(Span::styled(format!("{}{default}", " ".repeat(width - used - default.len())), theme::faint()));
            }
            lines.push(Line::from(first));

            let indent = " ".repeat(LABEL);
            for text in wrap(setting.help, width.saturating_sub(LABEL)) {
                lines.push(Line::from(vec![Span::raw(indent.clone()), Span::styled(text, theme::muted())]));
            }
            if let (Some(Editor { error: Some(error), .. }), true) = (&s.editing, selected) {
                lines.push(Line::from(vec![Span::raw(indent.clone()), Span::styled(error.clone(), theme::fg(theme::PINK))]));
            }
            if selected && s.editing.is_none() {
                if let (Some(caution), false) = (setting.caution, s.acknowledged.contains(&index)) {
                    lines.push(Line::from(vec![Span::raw(indent.clone()), Span::styled(format!("Asks first: {}", wrap(caution, 200)[0]), theme::faint())]));
                }
            }
            if selected {
                for line in &mut lines[start..] {
                    let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                    line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
                    line.style = Style::new().bg(theme::SELECTION);
                }
            }
            lines.push(Line::default());
        }

        let pending_restart: Vec<&str> = app.restart_waiting.iter().flat_map(|(labels, _)| labels.iter().copied()).collect();
        if !pending_restart.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Saved, waiting for a restart: {}.", pending_restart.join(", ")),
                theme::fg(theme::ORANGE),
            )));
        }
    }

    // Keep the selected setting in view.
    let height = inner.height as usize;
    let scroll = (selected_at + 6).saturating_sub(height) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), right);
}
