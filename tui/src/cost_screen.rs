//! The Cost screen: are the seats worth it compared with paying API list prices for the same
//! tokens? For this billing month, the last 30 days or the seat year so far, per person and
//! by model, with the token mix and the pricing table it used. `s` edits the seat plans.

use comptroller_admin::config::SeatPlan;
use comptroller_admin::cost::{Period, Report};
use comptroller_admin::pricing::{self, Source};
use comptroller_admin::{dates, settings};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Screen, ToastKind};
use crate::radar_screen::{color_of, short_name, tokens};
use crate::theme;
use crate::ui::{panel, truncate};
use crate::widgets::{centered, keycap, ratio_bar, section, stacked_bar};

#[derive(Default)]
pub struct State {
    pub plans: Option<PlanEditor>,
}

impl State {
    /// The seat-plan editor is open: it takes every key.
    pub fn editing(&self) -> bool {
        self.plans.is_some()
    }
}

pub struct PlanEditor {
    pub plans: Vec<SeatPlan>,
    pub row: usize,
    /// 0 name, 1 billing, 2 price, 3 renewal date.
    pub column: usize,
    /// The cell being typed into, and why the last value was refused.
    pub typing: Option<(String, Option<String>)>,
    pub dirty: bool,
    /// Esc was pressed once with unsaved changes.
    pub warned: bool,
}

const COLUMNS: [&str; 4] = ["NAME", "BILLING", "PRICE", "RENEWAL DATE"];

fn cell(plan: &SeatPlan, column: usize) -> String {
    match column {
        0 => plan.name.clone(),
        1 => plan.billing.clone(),
        2 => plan.price.map(|p| format!("{p}")).unwrap_or_default(),
        _ => plan.renewal_date.as_deref().map(dates::show_iso).unwrap_or_default(),
    }
}

/// An example date in this computer's format, for hints.
fn date_example() -> String {
    dates::format(2027, 3, 31)
}

/// Puts typed text into a plan, or says why it can't.
fn set_cell(plan: &mut SeatPlan, column: usize, text: &str) -> Result<(), String> {
    let text = text.trim();
    match column {
        0 if text.is_empty() => return Err("Enter a name".into()),
        0 => plan.name = text.to_string(),
        2 if text.is_empty() => plan.price = None,
        2 => match text.trim_start_matches(['$', '€', '£']).replace(',', "").parse::<f64>() {
            Ok(p) if p >= 0.0 => plan.price = Some(p),
            _ => return Err("Enter a price such as 300, or nothing".into()),
        },
        3 if text.is_empty() => plan.renewal_date = None,
        3 => {
            // Typed the local way (or yyyy-mm-dd); stored as yyyy-mm-dd.
            let (y, m, d) = dates::parse(text).ok_or_else(|| format!("Enter a date such as {}, or nothing", date_example()))?;
            plan.renewal_date = Some(format!("{y:04}-{m:02}-{d:02}"));
        }
        _ => {}
    }
    Ok(())
}

pub fn on_key(app: &mut App, key: KeyEvent) {
    if app.cost.plans.is_some() {
        return on_editor_key(app, key);
    }
    let period = match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Terminal;
            return;
        }
        KeyCode::Char('m') => Period::Month,
        KeyCode::Char('3') => Period::Last30Days,
        KeyCode::Char('y') => Period::SeatYear,
        KeyCode::Char('v') => {
            // Seat plans are assigned per person on the Users screen.
            app.screen = Screen::Users;
            return;
        }
        KeyCode::Char('s') => {
            let plans = app.snapshot.config_or_default().cost.seat_plans;
            app.cost.plans = Some(PlanEditor { plans, row: 0, column: 0, typing: None, dirty: false, warned: false });
            return;
        }
        _ => return,
    };
    if app.params.period != period {
        app.params.period = period;
        app.send_params();
    }
}

fn on_editor_key(app: &mut App, key: KeyEvent) {
    let Some(editor) = &mut app.cost.plans else { return };
    if let Some((text, error)) = &mut editor.typing {
        match key.code {
            KeyCode::Esc => editor.typing = None,
            KeyCode::Enter => {
                let text = text.clone();
                if let Some(plan) = editor.plans.get_mut(editor.row) {
                    match set_cell(plan, editor.column, &text) {
                        Ok(()) => {
                            editor.typing = None;
                            editor.dirty = true;
                        }
                        Err(e) => *error = Some(e),
                    }
                }
            }
            KeyCode::Backspace => {
                text.pop();
                *error = None;
            }
            KeyCode::Char(c) => {
                text.push(c);
                *error = None;
            }
            _ => {}
        }
        return;
    }

    let count = editor.plans.len();
    match key.code {
        KeyCode::Esc if editor.dirty && !editor.warned => {
            editor.warned = true;
            app.toast(ToastKind::Warn, "Seat plans not saved: s saves, esc again discards.");
        }
        KeyCode::Esc => app.cost.plans = None,
        KeyCode::Up | KeyCode::Char('k') if count > 0 => editor.row = (editor.row + count - 1) % count,
        KeyCode::Down | KeyCode::Char('j') if count > 0 => editor.row = (editor.row + 1) % count,
        KeyCode::Left | KeyCode::Char('h') => editor.column = (editor.column + 3) % 4,
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => editor.column = (editor.column + 1) % 4,
        KeyCode::Enter if count > 0 => {
            if editor.column == 1 {
                let plan = &mut editor.plans[editor.row];
                plan.billing = if plan.billing == "annual" { "monthly".into() } else { "annual".into() };
                editor.dirty = true;
            } else {
                editor.typing = Some((cell(&editor.plans[editor.row], editor.column), None));
            }
        }
        KeyCode::Char('a') => {
            editor.plans.push(SeatPlan { name: format!("Plan {}", count + 1), billing: "annual".into(), price: None, renewal_date: None });
            editor.row = count;
            editor.column = 0;
            editor.typing = Some((String::new(), None));
            editor.dirty = true;
        }
        KeyCode::Delete | KeyCode::Char('d') if count > 0 => {
            editor.plans.remove(editor.row);
            editor.row = editor.row.min(editor.plans.len().saturating_sub(1));
            editor.dirty = true;
        }
        KeyCode::Char('s') => save_plans(app),
        _ => {}
    }
}

fn save_plans(app: &mut App) {
    let Some(editor) = &app.cost.plans else { return };
    let mut names: Vec<&str> = editor.plans.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0].eq_ignore_ascii_case(w[1])) {
        return app.toast(ToastKind::Warn, "Two plans have the same name.");
    }
    let path = app.snapshot.place.config_path.clone();
    if !settings::writable(&path) {
        if !app.needs_admin("Saving seat plans") {
            app.toast(ToastKind::Warn, format!("Can't write {}.", path.display()));
        }
        return;
    }
    let plans = editor.plans.clone();
    match settings::save_plans(&path, &plans) {
        Ok(()) => {
            app.cost.plans = None;
            app.toast(ToastKind::Success, "Seat plans saved. Put people on them on the Users screen (p).");
            app.send_params();
        }
        Err(e) => app.toast(ToastKind::Warn, e),
    }
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// $1,284 for large amounts, $12.34 for small ones.
pub fn money(symbol: &str, amount: f64) -> String {
    let negative = amount < 0.0;
    let amount = amount.abs();
    let text = if amount >= 100.0 {
        let whole = amount.round() as i64;
        let digits = whole.to_string();
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        grouped
    } else {
        format!("{amount:.2}")
    };
    format!("{}{symbol}{text}", if negative { "-" } else { "" })
}

fn card(f: &mut Frame, area: Rect, title: &str, value: Span<'static>, detail: Span<'static>) {
    f.render_widget(Paragraph::new(vec![Line::from(value), Line::from(detail)]).block(panel(title, false)), area);
}

pub fn draw(f: &mut Frame, app: &App, body: Rect) {
    let [cards, rest] = Layout::vertical([Constraint::Length(5), Constraint::Fill(1)]).spacing(1).areas(body);
    let [left, right] = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).spacing(1).areas(rest);
    let config = app.snapshot.config_or_default();
    let symbol = config.cost.currency.clone();
    let view = app.snapshot.cost.as_ref();
    let report = view.and_then(|v| v.report.as_ref().ok());

    let priced = app.snapshot.config_or_default().cost.seat_plans.iter().any(|p| p.price.is_some());
    draw_cards(f, cards, report, &symbol, app.params.period, priced);
    draw_people(f, app, left, report, &symbol);
    draw_breakdown(f, app, right, report, &symbol);

    if let Some(editor) = &app.cost.plans {
        draw_editor(f, editor, &symbol, f.area());
    }
}

/// `priced`: some seat plan has a price, so a missing seat total means no one is on one.
fn draw_cards(f: &mut Frame, area: Rect, report: Option<&Report>, symbol: &str, period: Period, priced: bool) {
    let areas: [Rect; 4] = Layout::horizontal([Constraint::Fill(1); 4]).spacing(1).areas(area);
    let dash = || Span::styled("—", theme::faint());
    let period_name = match period {
        Period::Month => "THIS MONTH",
        Period::Last30Days => "30 DAYS",
        Period::SeatYear => "SEAT YEAR",
    };
    let Some(r) = report else {
        for (i, title) in [format!("SEATS · {period_name}"), "AT API PRICES".into(), "DIFFERENCE".into(), "TOKENS".into()].iter().enumerate() {
            card(f, areas[i], title, dash(), Span::raw(""));
        }
        return;
    };

    let days = r.range.as_ref().map(|r| r.days()).unwrap_or(0);
    card(
        f,
        areas[0],
        &format!("SEATS · {period_name}"),
        Span::styled(money(symbol, r.seat), theme::bold(theme::TEXT)),
        Span::styled(format!("{} seat{} · {days} days, prorated", r.seats, if r.seats == 1 { "" } else { "s" }), theme::faint()),
    );
    card(f, areas[1], "AT API PRICES", Span::styled(money(symbol, r.api), theme::bold(theme::TEXT)), Span::styled("same tokens, list prices", theme::faint()));
    let (value, detail) = match r.ratio() {
        Some(ratio) if ratio >= 1.0 => (
            Span::styled(money(symbol, r.api - r.seat), theme::bold(theme::OK)),
            Span::styled(format!("seats are {ratio:.1}× cheaper"), theme::fg(theme::OK)),
        ),
        Some(ratio) => (
            Span::styled(money(symbol, r.seat - r.api), theme::bold(theme::ORANGE)),
            Span::styled(format!("API would be {:.1}× cheaper", 1.0 / ratio), theme::fg(theme::ORANGE)),
        ),
        None if r.seats == 0 && priced => (dash(), Span::styled("no one on a seat plan: press v", theme::fg(theme::ORANGE))),
        None if r.seats == 0 => (dash(), Span::styled("no seat prices yet: press s", theme::faint())),
        None => (dash(), Span::styled("no priced tokens yet", theme::faint())),
    };
    card(f, areas[2], "DIFFERENCE", value, detail);
    let cache_share = if r.tokens.total() > 0 { r.tokens.cache_read as f64 * 100.0 / r.tokens.total() as f64 } else { 0.0 };
    card(
        f,
        areas[3],
        "TOKENS",
        Span::styled(tokens(r.tokens.total()), theme::bold(theme::TEXT)),
        Span::styled(format!("{cache_share:.0}% cache reads"), theme::faint()),
    );
}

fn draw_people(f: &mut Frame, app: &App, area: Rect, report: Option<&Report>, symbol: &str) {
    let block = panel("Cost · by person", true);
    let width = block.inner(area).width as usize;
    let mut lines = Vec::new();
    let Some(r) = report else {
        let text = match app.snapshot.cost.as_ref().map(|v| &v.report) {
            Some(Err(e)) => e.clone(),
            _ => "Reading…".into(),
        };
        lines.push(Line::from(Span::styled(text, theme::fg(theme::ORANGE))));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("m this month · 3 last 30 days · y seat year · s seat plans", theme::faint())));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
        return;
    };
    let label = r.range.as_ref().map(|r| r.label.to_uppercase()).unwrap_or_default();
    lines.push(section(&label, None, width));

    let names: Vec<String> = {
        let mut n: Vec<String> = r.people.iter().map(|p| p.name.clone()).collect();
        n.sort_by_key(|p| p.to_lowercase());
        n
    };
    let top = r.people.iter().map(|p| p.api.max(p.seat.unwrap_or(0.0))).fold(0.0, f64::max).max(0.01);
    let verdicts: Vec<Span> = r
        .people
        .iter()
        .map(|p| match (p.seat, &p.plan) {
            (_, None) => Span::styled("no plan: press v", theme::faint()),
            (None, Some(plan)) => Span::styled(format!("{plan} has no price"), theme::fg(theme::ORANGE)),
            (Some(seat), _) if seat <= p.api => Span::styled(format!("seat saves {}", money(symbol, p.api - seat)), theme::fg(theme::OK)),
            (Some(seat), _) => Span::styled(format!("API saves {}", money(symbol, seat - p.api)), theme::fg(theme::ORANGE)),
        })
        .collect();
    // The bars take what's left after the figures and the longest verdict, so nothing wraps.
    let longest = verdicts.iter().map(|v| v.content.chars().count()).max().unwrap_or(7).max(7);
    let bar = width.saturating_sub(36 + 2 + 2 + longest).clamp(6, 22);
    lines.push(Line::from(Span::styled(
        format!("{:<16}{:>9}{:>9}  {:<w$}  {}", "PERSON", "SEAT", "API", "SEAT · vs API", "VERDICT", w = bar),
        theme::label(),
    )));
    for (p, verdict) in r.people.iter().zip(verdicts) {
        let color = color_of(&p.name, &names);
        let seat = p.seat.map(|s| money(symbol, s)).unwrap_or_else(|| "—".into());
        lines.push(Line::from(
            vec![
                Span::styled(format!("{:<16}", truncate(short_name(&p.name), 15)), theme::fg(color)),
                Span::styled(format!("{seat:>9}"), theme::text()),
                Span::styled(format!("{:>9}  ", money(symbol, p.api)), theme::text()),
            ]
            .into_iter()
            .chain(ratio_bar(p.seat.unwrap_or(0.0) / top, bar, theme::MUTED))
            .chain([Span::raw("  "), verdict])
            .collect::<Vec<_>>(),
        ));
        lines.push(Line::from([Span::raw(" ".repeat(36))].into_iter().chain(ratio_bar(p.api / top, bar, color)).collect::<Vec<_>>()));
        if p.unpriced_tokens > 0 {
            lines.push(Line::from(Span::styled(format!("{}{} tokens on models without a price", " ".repeat(36), tokens(p.unpriced_tokens)), theme::faint())));
        }
    }
    if r.people.is_empty() {
        lines.push(Line::from(Span::styled("No one on a seat plan and no recorded usage in this period.", theme::faint())));
    }
    if r.people_without_plan > 0 && !app.snapshot.config_or_default().cost.seat_plans.is_empty() {
        let n = r.people_without_plan;
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            if n == 1 {
                "1 person isn't on a seat plan, so their seat isn't counted. Press v for Users, pick them, then p.".to_string()
            } else {
                format!("{n} people aren't on a seat plan, so their seats aren't counted. Press v for Users, pick a person, then p.")
            },
            theme::fg(theme::ORANGE),
        )));
    }
    if !r.plans_without_price.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(format!("No price yet for {}: press s.", r.plans_without_price.join(", ")), theme::fg(theme::ORANGE))));
    }

    if let (Some((year_seats, projected)), Some(range)) = (r.projection, &r.range) {
        lines.push(Line::default());
        lines.push(section("TO THE END OF THE PERIOD", None, width));
        let (y, m, d) = comptroller_admin::cost::civil(range.end_day);
        let api_label = format!("API at today's daily rate, by {}", dates::format(y, m, d));
        let pad = api_label.chars().count() + 2;
        let figure = |s: f64| format!("{:>9}", money(symbol, s));
        lines.push(Line::from(vec![
            Span::styled(format!("{:<pad$}", "Seats for the whole period"), theme::muted()),
            Span::styled(figure(year_seats), theme::text()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{api_label:<pad$}"), theme::muted()),
            Span::styled(figure(projected), theme::fg(if projected >= year_seats { theme::OK } else { theme::ORANGE })),
        ]));
        if app.params.period == Period::SeatYear {
            lines.push(Line::from(Span::styled("That's the comparison to look at before renewing.", theme::faint())));
        }
    }

    lines.push(Line::default());
    lines.push(section("HOW TO READ IT", None, width));
    lines.push(Line::from(Span::styled(
        "API = what the same requests would cost on pay-as-you-go API pricing, from the token counts in each reply. Seat = each person's plan price, spread evenly per day from the day they first connected. Annual plans are paid up front; their cost is spread over the seat year.",
        theme::muted(),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn draw_breakdown(f: &mut Frame, app: &App, area: Rect, report: Option<&Report>, symbol: &str) {
    let block = panel("Where the cost goes", false);
    let width = block.inner(area).width as usize;
    let mut lines = vec![section("BY MODEL", Some("API equivalent".into()), width)];
    if let Some(r) = report {
        let top = r.models.first().map(|m| m.1).unwrap_or(0.0).max(0.01);
        for (name, cost, _) in &r.models {
            let bar = width.saturating_sub(30).max(6);
            lines.push(Line::from(
                [Span::styled(format!("{:<18}", truncate(name.trim_start_matches("Claude "), 17)), theme::text())]
                    .into_iter()
                    .chain(ratio_bar(cost / top, bar, theme::ACCENT))
                    .chain([Span::styled(format!("{:>10}", money(symbol, *cost)), theme::text())])
                    .collect::<Vec<_>>(),
            ));
        }
        if r.models.is_empty() {
            lines.push(Line::from(Span::styled("No priced replies in this period.", theme::faint())));
        }
        for (model, n) in &r.unpriced {
            lines.push(Line::from(Span::styled(format!("{model}: {} tokens, not in the pricing table", tokens(*n)), theme::fg(theme::ORANGE))));
        }

        lines.push(Line::default());
        lines.push(section("TOKEN MIX", None, width));
        let t = &r.tokens;
        let mix: [(&str, i64, Color); 4] = [
            ("input", t.input, theme::ACCENT),
            ("output", t.output, theme::ORANGE),
            ("cache write", t.cache_write(), theme::PINK),
            ("cache read", t.cache_read, theme::FAINT),
        ];
        lines.push(Line::from(stacked_bar(&mix.iter().map(|m| (m.1.max(0) as u64, m.2)).collect::<Vec<_>>(), width, "█")));
        let total = t.total().max(1) as f64;
        let mut legend = Vec::new();
        for (name, n, color) in mix {
            legend.push(Span::styled("■ ", Style::new().fg(color)));
            legend.push(Span::styled(format!("{name} {:.0}%  ", n as f64 * 100.0 / total), theme::muted()));
        }
        lines.push(Line::from(legend));
    }

    lines.push(Line::default());
    lines.push(section("PRICING TABLE", None, width));
    match app.snapshot.cost.as_ref().map(|v| &v.pricing) {
        Some(Ok((table, source))) => {
            let age = table.age_days(comptroller_admin::health::now()).unwrap_or(0);
            lines.push(Line::from(vec![
                Span::styled("updated ", theme::muted()),
                Span::styled(table.last_updated.clone(), if age > pricing::STALE_DAYS { theme::fg(theme::ORANGE) } else { theme::text() }),
                Span::styled(
                    match source {
                        Source::File(path) => format!(" · {}", path.display()),
                        Source::BuiltIn => " · built into this terminal".into(),
                    },
                    theme::faint(),
                ),
            ]));
            if age > pricing::STALE_DAYS {
                lines.push(Line::from(Span::styled(format!("Over {} days old: check the prices against the source.", pricing::STALE_DAYS), theme::fg(theme::ORANGE))));
            }
            lines.push(Line::from(Span::styled(format!("from {}", table.source), theme::faint())));
            if matches!(source, Source::BuiltIn) {
                lines.push(Line::from(Span::styled(
                    format!("To change prices, save a copy as {} beside comptroller.json (Settings → Cost).", app.snapshot.config_or_default().cost.pricing_file),
                    theme::faint(),
                )));
            }
        }
        Some(Err(e)) => lines.push(Line::from(Span::styled(e.clone(), theme::fg(theme::PINK)))),
        None => {}
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Claude Code traffic only: Chat and Cowork use the same seats but don't pass through claude-comptroller. The API figure is an estimate at list prices: no discounts, batch or priority tiers.",
        theme::faint(),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn draw_editor(f: &mut Frame, editor: &PlanEditor, symbol: &str, screen: Rect) {
    let widths = [22usize, 10, 12, 14];
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Price per billing period, in {symbol}. Annual plans need a renewal date, such as {}.", date_example()),
            theme::muted(),
        )),
        Line::default(),
        Line::from(
            COLUMNS
                .iter()
                .zip(widths)
                .map(|(c, w)| Span::styled(format!("  {c:<0$}", w), theme::label()))
                .collect::<Vec<_>>(),
        ),
    ];
    for (row, plan) in editor.plans.iter().enumerate() {
        let mut spans = Vec::new();
        for (column, width) in widths.iter().enumerate() {
            let selected = row == editor.row && column == editor.column;
            let text = match (&editor.typing, selected) {
                (Some((typed, _)), true) => format!("{typed}▏"),
                _ => match column {
                    2 => plan.price.map(|p| money(symbol, p)).unwrap_or_else(|| "not set".into()),
                    3 if plan.billing == "monthly" => "—".into(),
                    3 => plan.renewal_date.as_deref().map(dates::show_iso).unwrap_or_else(|| "not set".into()),
                    _ => cell(plan, column),
                },
            };
            let style = if selected { Style::new().fg(theme::TEXT).bg(theme::SELECTION) } else if text == "not set" { theme::fg(theme::ORANGE) } else { theme::text() };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(format!("{:<width$}", truncate(&text, *width)), style));
        }
        lines.push(Line::from(spans));
    }
    if editor.plans.is_empty() {
        lines.push(Line::from(Span::styled("No plans. Press a to add one.", theme::faint())));
    }
    if let Some((_, Some(error))) = &editor.typing {
        lines.push(Line::from(Span::styled(error.clone(), theme::fg(theme::PINK))));
    }
    lines.push(Line::default());
    let keys: Vec<Span> = if editor.typing.is_some() {
        [keycap("enter", "keep"), keycap("esc", "cancel")].concat()
    } else {
        [keycap("←→", "column"), keycap("enter", "edit"), keycap("a", "add"), keycap("del", "remove"), keycap("s", "save"), keycap("esc", "close")].concat()
    };
    lines.push(Line::from(keys).right_aligned());

    // Room for the hint to wrap onto a second line.
    let area = centered(screen, 84, lines.len() as u16 + 5);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(theme::fg(theme::BORDER_FOCUS))
        .style(Style::new().bg(theme::PANEL))
        .title(Line::from(Span::styled(if editor.dirty { " Seat plans · not saved " } else { " Seat plans " }, theme::bold(theme::TEXT))))
        .padding(Padding::new(2, 2, 1, 0));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_reads_well() {
        assert_eq!(money("$", 1284.4), "$1,284");
        assert_eq!(money("$", 12.345), "$12.35");
        assert_eq!(money("€", 1_234_567.0), "€1,234,567");
        assert_eq!(money("$", -450.0), "-$450");
    }

    #[test]
    fn plan_cells_are_checked() {
        let mut plan = SeatPlan { name: "Team".into(), billing: "annual".into(), price: None, renewal_date: None };
        assert!(set_cell(&mut plan, 2, "$1,200").is_ok());
        assert_eq!(plan.price, Some(1200.0));
        assert!(set_cell(&mut plan, 2, "abc").is_err());
        assert!(set_cell(&mut plan, 3, "2027-03-01").is_ok());
        assert_eq!(plan.renewal_date.as_deref(), Some("2027-03-01"));
        // The local format, whatever it is on the computer running the tests.
        assert!(set_cell(&mut plan, 3, &dates::format(2027, 9, 9)).is_ok());
        assert_eq!(plan.renewal_date.as_deref(), Some("2027-09-09"));
        assert!(set_cell(&mut plan, 3, "soon").is_err());
        assert!(set_cell(&mut plan, 0, " ").is_err());
    }
}
