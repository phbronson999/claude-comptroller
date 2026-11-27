//! claude-comptroller: the admin terminal. The only command admins type.
//!
//! There are no subcommands: every action is a key on the Terminal screen.
//! For testing, these point it somewhere other than the default install:
//!   --config <path>         the comptroller.json to read
//!   --service-name <name>   the Windows service to manage
//!   --install-dir <dir>     where Install puts the service

mod app;
mod black_box_screen;
mod cost_screen;
mod dashboard_screen;
mod logo;
mod radar_screen;
mod settings_screen;
// Copied as-is from Code Keepsake; not every color is used here.
#[allow(dead_code)]
mod theme;
mod ui;
mod users_screen;
mod widgets;

use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

fn options() -> app::Options {
    let mut options = app::Options { config: None, service_name: None, install_dir: None };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => options.config = args.next().map(|p| std::path::absolute(PathBuf::from(p)).unwrap_or_default()),
            "--service-name" => options.service_name = args.next(),
            "--install-dir" => options.install_dir = args.next().map(PathBuf::from),
            "--version" => {
                println!("claude-comptroller {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("claude-comptroller takes no commands: everything is on the Terminal screen.\nUnknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    options
}

fn main() -> std::io::Result<()> {
    let mut app = app::App::new(options());
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        // Never exit half way through installing, starting or stopping.
        while !app.quit || app.working() {
            app.pump();
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if event::poll(Duration::from_millis(80))? {
                // Windows reports key releases too; act on presses only.
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key);
                    }
                }
            }
            app.tick = app.tick.wrapping_add(1);
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::Terminal;

    use super::app::{App, Options, Screen};

    /// Every screen and overlay draws at a roomy and a cramped size without panicking.
    #[test]
    fn every_screen_draws_at_any_size() {
        let dir = std::env::temp_dir().join("comptroller-tui-draw");
        let mut app = App::new(Options {
            config: Some(dir.join("comptroller.json")),
            service_name: Some("comptroller-tui-test-not-installed".into()),
            install_dir: Some(dir.clone()),
        });
        for (width, height) in [(200, 50), (80, 24), (40, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for screen in [Screen::Terminal, Screen::Status, Screen::Preflight, Screen::Install, Screen::Uninstall, Screen::Settings, Screen::Users, Screen::Radar, Screen::Dashboard, Screen::Cost, Screen::BlackBox] {
                app.screen = screen;
                terminal.draw(|f| super::ui::draw(f, &app)).unwrap();
                for key in [KeyCode::Char('?'), KeyCode::Esc] {
                    app.on_key(KeyEvent::from(key));
                    terminal.draw(|f| super::ui::draw(f, &app)).unwrap();
                }
            }
            app.screen = Screen::Terminal;
            for key in [KeyCode::Down, KeyCode::Down, KeyCode::Char('a'), KeyCode::Char('o'), KeyCode::Char('t'), KeyCode::Char('o'), KeyCode::Char('u')] {
                app.on_key(KeyEvent::from(key));
                terminal.draw(|f| super::ui::draw(f, &app)).unwrap();
            }
            assert_eq!(app.screen, Screen::Terminal);

            // Settings with a real file: every group, an edit, then leaving without saving.
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("comptroller.json"), comptroller_admin::config::SAMPLE).unwrap();
            let mut keys = vec![KeyCode::Char('c')];
            keys.extend([KeyCode::Right; 10]);
            keys.extend([KeyCode::Enter, KeyCode::Char('X'), KeyCode::Enter, KeyCode::Esc]);
            for key in keys {
                app.on_key(KeyEvent::from(key));
                terminal.draw(|f| super::ui::draw(f, &app)).unwrap();
            }
            assert_eq!(app.settings.pending.len(), 1, "the edited organization name");
            app.on_key(KeyEvent::from(KeyCode::Char('d')));
            assert!(app.settings.pending.is_empty());
            assert_eq!(app.screen, Screen::Terminal);
        }
        let buffer = Terminal::new(TestBackend::new(200, 50)).unwrap().draw(|f| super::ui::draw(f, &app)).unwrap().buffer.clone();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("claude-comptroller") && text.contains("not installed") && text.contains("Preflight test"));
    }
}
