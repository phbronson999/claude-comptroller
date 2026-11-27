//! Pictures of the screens for the README, drawn from a seeded example database rather than
//! captured from a real window. tools/screenshots.ps1 sets everything up and runs this:
//!
//!   cargo test --manifest-path tui/Cargo.toml --bin claude-comptroller -- --ignored screenshots
//!
//! with COMPTROLLER_SHOTS_CONFIG (a comptroller.json whose database is seeded) and
//! COMPTROLLER_SHOTS_OUT (the folder for the PNGs). Each screen is drawn into a buffer, the
//! buffer becomes an SVG with every cell in place, and resvg turns that into a PNG. Block and
//! box-drawing characters are drawn as shapes, so bars and borders join up whatever the font.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::app::{App, Options, Screen};

const COLS: u16 = 164;
const ROWS: u16 = 44;
const CELL_W: f32 = 9.0;
const CELL_H: f32 = 19.0;
const FONT: f32 = 15.0;
/// The terminal's own background, which the theme leaves alone.
const BACKGROUND: (u8, u8, u8) = (12, 12, 12);
const FOREGROUND: (u8, u8, u8) = (236, 236, 236);

fn rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => default,
        Color::Black => (12, 12, 12),
        Color::White | Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::Red | Color::LightRed => (231, 72, 86),
        Color::Green | Color::LightGreen => (22, 198, 12),
        Color::Yellow | Color::LightYellow => (249, 241, 165),
        Color::Blue | Color::LightBlue => (59, 120, 255),
        Color::Magenta | Color::LightMagenta => (180, 0, 158),
        Color::Cyan | Color::LightCyan => (97, 214, 214),
        Color::Indexed(_) => default,
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Lower block elements as a share of the cell height, from the bottom.
fn lower_block(c: char) -> Option<f32> {
    match c {
        '▁' => Some(1.0 / 8.0),
        '▂' => Some(2.0 / 8.0),
        '▃' => Some(3.0 / 8.0),
        '▄' => Some(4.0 / 8.0),
        '▅' => Some(5.0 / 8.0),
        '▆' => Some(6.0 / 8.0),
        '▇' => Some(7.0 / 8.0),
        '█' => Some(1.0),
        _ => None,
    }
}

/// Box-drawing characters as the arms they have: left, right, up, down; and whether heavy.
fn box_arms(c: char) -> Option<([bool; 4], bool)> {
    Some(match c {
        '─' => ([true, true, false, false], false),
        '━' => ([true, true, false, false], true),
        '│' => ([false, false, true, true], false),
        '┃' => ([false, false, true, true], true),
        '┌' | '╭' => ([false, true, false, true], false),
        '┐' | '╮' => ([true, false, false, true], false),
        '└' | '╰' => ([false, true, true, false], false),
        '┘' | '╯' => ([true, false, true, false], false),
        '├' => ([false, true, true, true], false),
        '┤' => ([true, false, true, true], false),
        '┬' => ([true, true, false, true], false),
        '┴' => ([true, true, true, false], false),
        '┼' => ([true, true, true, true], false),
        _ => return None,
    })
}

/// The buffer as an SVG, one cell at a time.
pub fn svg(buffer: &Buffer) -> String {
    let (w, h) = (buffer.area.width as f32 * CELL_W, buffer.area.height as f32 * CELL_H);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="100%" height="100%" fill="{}"/>"#,
        hex(BACKGROUND)
    );
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let (px, py) = (x as f32 * CELL_W, y as f32 * CELL_H);
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (mut fg, mut bg) = (rgb(cell.fg, FOREGROUND), rgb(cell.bg, BACKGROUND));
            if reversed {
                std::mem::swap(&mut fg, &mut bg);
            }
            if bg != BACKGROUND {
                out.push_str(&format!(r#"<rect x="{px}" y="{py}" width="{}" height="{}" fill="{}"/>"#, CELL_W + 0.5, CELL_H + 0.5, hex(bg)));
            }
            let symbol = cell.symbol();
            let mut chars = symbol.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                if !symbol.trim().is_empty() {
                    text.push_str(&glyph(px, py, symbol, fg, cell.modifier));
                }
                continue;
            };
            if let Some(share) = lower_block(c) {
                let height = CELL_H * share;
                out.push_str(&format!(r#"<rect x="{px}" y="{}" width="{}" height="{height}" fill="{}"/>"#, py + CELL_H - height, CELL_W + 0.5, hex(fg)));
            } else if c == '▏' {
                out.push_str(&format!(r#"<rect x="{px}" y="{py}" width="1.5" height="{CELL_H}" fill="{}"/>"#, hex(fg)));
            } else if let Some(([left, right, up, down], heavy)) = box_arms(c) {
                let (cx, cy) = (px + CELL_W / 2.0, py + CELL_H / 2.0);
                let stroke = if heavy { 2.2 } else { 1.1 };
                let mut d = String::new();
                if left {
                    d.push_str(&format!("M{px} {cy}H{cx}"));
                }
                if right {
                    d.push_str(&format!("M{cx} {cy}H{}", px + CELL_W));
                }
                if up {
                    d.push_str(&format!("M{cx} {py}V{cy}"));
                }
                if down {
                    d.push_str(&format!("M{cx} {cy}V{}", py + CELL_H));
                }
                out.push_str(&format!(r#"<path d="{d}" stroke="{}" stroke-width="{stroke}" fill="none" stroke-linecap="square"/>"#, hex(fg)));
            } else if c != ' ' {
                text.push_str(&glyph(px, py, symbol, fg, cell.modifier));
            }
        }
    }
    out.push_str(&format!(r#"<g font-family="Cascadia Mono, Consolas, Segoe UI Symbol, monospace" font-size="{FONT}">{text}</g></svg>"#));
    out
}

fn glyph(px: f32, py: f32, symbol: &str, fg: (u8, u8, u8), modifier: Modifier) -> String {
    let weight = if modifier.contains(Modifier::BOLD) { r#" font-weight="bold""# } else { "" };
    format!(r#"<text x="{}" y="{}" fill="{}"{weight}>{}</text>"#, px + 0.2, py + CELL_H * 0.74, hex(fg), escape(symbol))
}

fn png(svg: &str, path: &Path) {
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &options).expect("valid SVG");
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).expect("a size");
    resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.save_png(path).expect("PNG written");
}

/// Pumps the app until `ready` says the poller has delivered what the screen needs.
fn wait(app: &mut App, ready: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        app.pump();
        if ready(app) {
            // One more round, so live figures settle.
            std::thread::sleep(Duration::from_millis(300));
            app.pump();
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("The {} screen didn't get its data.", app.screen.title());
}

#[test]
#[ignore = "makes the README pictures; run by tools/screenshots.ps1"]
fn screenshots() {
    let config = PathBuf::from(std::env::var("COMPTROLLER_SHOTS_CONFIG").expect("COMPTROLLER_SHOTS_CONFIG"));
    let out = PathBuf::from(std::env::var("COMPTROLLER_SHOTS_OUT").expect("COMPTROLLER_SHOTS_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    // Words that must never be in a picture: whoever and wherever this runs.
    let private: Vec<String> = ["USERNAME", "COMPUTERNAME", "USERDOMAIN"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .filter(|v| v.len() > 2)
        .map(|v| v.to_lowercase())
        .collect();

    // Two archives, the first one the baseline, made the way the Archives screen makes them.
    {
        use comptroller_admin::archive::{create, Content, New};
        use comptroller_admin::config::{Config, Place};
        use comptroller_admin::pricing::{Pricing, BUILT_IN};
        let cfg = Config::load(&config).unwrap();
        let place = Place { config_path: config.clone() };
        let offset = comptroller_admin::local_offset();
        let today = (comptroller_admin::health::now() + offset).div_euclid(86_400);
        let pricing = Pricing::parse(BUILT_IN).unwrap();
        let (db, folder) = (place.database(&cfg), place.archive_dir(&cfg));
        let _ = std::fs::remove_dir_all(&folder);
        let week = New { first_day: today - 14, last_day: today - 8, label: "typical week", content: Content::Summary };
        let baseline = create(&db, &folder, &week, &cfg.cost, Some((&pricing, BUILT_IN)), offset).unwrap();
        let day = New { first_day: today - 3, last_day: today - 3, label: "before the rollout", content: Content::Raw };
        create(&db, &folder, &day, &cfg.cost, Some((&pricing, BUILT_IN)), offset).unwrap();
        let index = comptroller_admin::settings::index_of("Archives.Baseline").unwrap();
        comptroller_admin::settings::save(&config, &[(index, serde_json::Value::String(baseline.file_name()))]).unwrap();
    }

    let mut app = App::new(Options {
        config: Some(config.clone()),
        service_name: None,
        // Where Install would put it, for the Install picture.
        install_dir: Some(PathBuf::from(r"C:\Program Files\claude-comptroller")),
    });
    app.show_commands = false;
    let mut terminal = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();

    type Setup = fn(&mut App);
    type Ready = fn(&App) -> bool;
    let open = |screen: Screen| move |app: &mut App| app.screen = screen;
    // In the order they're met: setting up, watching, then the records.
    let shots: Vec<(&str, Box<dyn Fn(&mut App)>, Ready, Option<Setup>)> = vec![
        ("terminal", Box::new(open(Screen::Terminal)), |a| a.snapshot.health.is_some() && a.snapshot.today.is_some(), None),
        ("install", Box::new(open(Screen::Install)), |a| a.snapshot.health.is_some(), None),
        (
            "preflight",
            Box::new(|a: &mut App| {
                a.screen = Screen::Preflight;
                a.run_preflight();
            }),
            |a| !a.preflight_running && !a.preflight.is_empty(),
            None,
        ),
        ("status", Box::new(open(Screen::Status)), |a| a.snapshot.upstream.is_some(), None),
        (
            "settings",
            Box::new(|a: &mut App| {
                let path = a.snapshot.place.config_path.clone();
                a.settings.load(&path);
                a.settings.group = comptroller_admin::settings::GROUPS.iter().position(|(g, _)| *g == "Proxy").unwrap_or(0);
                // The one an admin changes first, so developers can reach the service.
                a.settings.row = a.settings.rows().iter().position(|&i| comptroller_admin::settings::SETTINGS[i].key == "ListenUrl").unwrap_or(0);
                a.screen = Screen::Settings;
            }),
            |_| true,
            None,
        ),
        ("users", Box::new(open(Screen::Users)), |a| matches!(a.snapshot.users, Some(Ok(Some(_)))), None),
        (
            "seat-plans",
            Box::new(|a: &mut App| {
                a.screen = Screen::Cost;
                a.params.period = comptroller_admin::cost::Period::SeatYear;
                a.send_params();
            }),
            |a| a.snapshot.cost.as_ref().is_some_and(|c| c.report.is_ok()),
            // The plan editor, opened with s.
            Some(|a| a.on_key(KeyEvent::from(KeyCode::Char('s')))),
        ),
        (
            "dashboard",
            Box::new(|a: &mut App| {
                a.cost.plans = None;
                a.screen = Screen::Dashboard;
                a.params.days = 7;
                a.send_params();
            }),
            |a| matches!(a.snapshot.summary, Some(Ok(Some(_)))) && matches!(a.snapshot.baseline, Some(Ok(_))),
            None,
        ),
        ("radar", Box::new(open(Screen::Radar)), |a| matches!(a.snapshot.feed, Some(Ok(Some(_)))) && a.snapshot.today.is_some(), None),
        ("cost", Box::new(open(Screen::Cost)), |a| a.snapshot.cost.as_ref().is_some_and(|c| c.report.is_ok()) && a.snapshot.baseline.is_some(), None),
        ("black-box", Box::new(open(Screen::BlackBox)), |a| matches!(a.snapshot.black_box, Some(Ok(Some(_)))) && a.snapshot.health.is_some(), None),
        ("archives", Box::new(open(Screen::Archives)), |a| matches!(&a.snapshot.archives, Some(Ok(list)) if !list.is_empty()), None),
        (
            "new-archive",
            Box::new(open(Screen::Archives)),
            |a| matches!(&a.snapshot.archives, Some(Ok(list)) if !list.is_empty()),
            Some(|a| {
                crate::archives_screen::new_archive(a);
                if let Some(form) = &mut a.archives.form {
                    form.label = "last sprint".into();
                    form.field = 2;
                }
            }),
        ),
    ];
    for (name, go, ready, then) in shots {
        go(&mut app);
        wait(&mut app, ready);
        if let Some(then) = then {
            then(&mut app);
        }
        let buffer = terminal.draw(|f| crate::ui::draw(f, &app)).unwrap().buffer.clone();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect::<String>().to_lowercase();
        for word in &private {
            assert!(!text.contains(word.as_str()), "The {name} picture would show \"{word}\"; not writing it.");
        }
        png(&svg(&buffer), &out.join(format!("{name}.png")));
    }
}
