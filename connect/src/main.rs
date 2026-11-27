//! claude-comptroller-connect: the tray app on each developer computer.
//!
//!   --server <url>          the comptroller's address (otherwise asked for on first run)
//!   --settings-root <dir>   write settings under <dir> instead of the real profile (also
//!                           COMPTROLLER_CONNECT_SETTINGS_ROOT); never touches the registry
//!   --device-name <name>    register as another device, for testing

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connect_core::paths::Paths;
use connect_core::session::{Session, Status, View};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, WebviewWindow, WindowEvent, Wry};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "claude-comptroller-connect";
const TRAY: &str = "tray";

struct Shared {
    session: Mutex<Session>,
    menu: MenuItems,
    hidden_at: Mutex<Option<Instant>>,
}

struct MenuItems {
    refresh: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    startup: CheckMenuItem<Wry>,
}

struct Args {
    server: Option<String>,
    settings_root: Option<PathBuf>,
    device: String,
}

fn args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args {
        server: std::env::var("COMPTROLLER_SERVER").ok(),
        settings_root: std::env::var_os("COMPTROLLER_CONNECT_SETTINGS_ROOT").map(PathBuf::from),
        device: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this-computer".into()),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => parsed.server = args.next(),
            "--settings-root" => parsed.settings_root = args.next().map(PathBuf::from),
            "--device-name" => parsed.device = args.next().unwrap_or(parsed.device),
            _ => {}
        }
    }
    parsed
}

// ---- small helpers ----

fn log(paths: &Paths, line: &str) {
    let dir = paths.log_dir();
    let _ = fs::create_dir_all(&dir);
    let file = dir.join("connect.log");
    if fs::metadata(&file).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = fs::rename(&file, dir.join("connect.old.log"));
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut out) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(out, "{secs} {line}");
    }
}

/// One instance per settings location, so two apps never write the same files.
fn already_running(paths: &Paths) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let key: String = paths.data_dir.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    // SAFETY: plain Win32 call; the handle is kept for the life of the process.
    unsafe {
        match CreateMutexW(None, true, &HSTRING::from(format!("Local\\claude-comptroller-connect-{key}"))) {
            Ok(handle) => {
                // HANDLE has no Drop: the mutex stays held until the process exits.
                let _ = handle;
                GetLastError() == ERROR_ALREADY_EXISTS
            }
            Err(_) => false,
        }
    }
}

fn startup_enabled(paths: &Paths) -> bool {
    if paths.sandboxed {
        return paths.data_dir.join("start-with-windows").exists();
    }
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

/// In a sandbox, only a marker file changes: tests never touch the registry.
fn set_startup(paths: &Paths, enabled: bool) -> Result<(), String> {
    if paths.sandboxed {
        let marker = paths.data_dir.join("start-with-windows");
        let _ = fs::create_dir_all(&paths.data_dir);
        return if enabled { fs::write(marker, "") } else { fs::remove_file(marker).or(Ok(())) }.map_err(|e| e.to_string());
    }
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(RUN_KEY).map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display())).map_err(|e| e.to_string())
    } else {
        key.delete_value(RUN_VALUE).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }).map_err(|e| e.to_string())
    }
}

fn icon_name(view: &View) -> &'static str {
    match view.status {
        Status::Connected if view.expiring_soon => "attention",
        Status::Connected => "connected",
        Status::Disconnected | Status::Blocked => "disconnected",
        Status::Unreachable => "unreachable",
        Status::NotConnected => "not-connected",
    }
}

macro_rules! tray_pngs {
    ($($state:literal),*) => {
        fn tray_png(state: &str, size: u32) -> &'static [u8] {
            match (state, size) {
                $(
                    ($state, 16) => include_bytes!(concat!("../icons/png/", $state, "-16.png")),
                    ($state, 20) => include_bytes!(concat!("../icons/png/", $state, "-20.png")),
                    ($state, 24) => include_bytes!(concat!("../icons/png/", $state, "-24.png")),
                    ($state, 32) => include_bytes!(concat!("../icons/png/", $state, "-32.png")),
                    ($state, _) => include_bytes!(concat!("../icons/png/", $state, "-48.png")),
                )*
                _ => include_bytes!("../icons/png/not-connected-32.png"),
            }
        }
    };
}
tray_pngs!("connected", "attention", "disconnected", "unreachable", "not-connected");

/// The drawn size closest to the tray's 16 px at this display's scaling.
fn tray_icon(app: &AppHandle, state: &str) -> Image<'static> {
    let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
    let wanted = (16.0 * scale).round() as u32;
    let size = [16, 20, 24, 32, 48].into_iter().find(|&s| s >= wanted).unwrap_or(48);
    Image::from_bytes(tray_png(state, size)).expect("bundled icon")
}

fn status_word(view: &View) -> &'static str {
    match view.status {
        Status::Connected if view.expiring_soon => "Connected, sign-in expires soon",
        Status::Connected => "Connected",
        Status::Disconnected => "Disconnected",
        Status::Blocked => "Blocked",
        Status::Unreachable => "Server unreachable",
        Status::NotConnected => "Not connected yet",
    }
}

fn days_left(view: &View) -> String {
    match view.seconds_left {
        Some(s) if s <= 0 => "expired".into(),
        Some(s) if s < 86_400 => format!("{} h left", (s / 3_600).max(1)),
        Some(s) => format!("{} days left", s / 86_400),
        None => String::new(),
    }
}

/// Puts the tray, menu and popup in line with the session.
fn publish(app: &AppHandle, view: &View) {
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_icon(Some(tray_icon(app, icon_name(view))));
        let _ = tray.set_tooltip(Some(format!("claude-comptroller · {}", status_word(view))));
    }
    let shared = app.state::<Shared>();
    let left = days_left(view);
    let _ = shared.menu.refresh.set_text(if left.is_empty() { "Refresh token".into() } else { format!("Refresh token ({left})") });
    let _ = shared.menu.refresh.set_enabled(view.status != Status::NotConnected);
    let reconnect = view.status == Status::Disconnected;
    let _ = shared.menu.toggle.set_text(if reconnect { "Reconnect" } else { "Disconnect" });
    let _ = shared.menu.toggle.set_enabled(matches!(view.status, Status::Connected | Status::Disconnected));
    let _ = app.emit("view", view);
}

/// Runs a session action off the UI thread, then shows the result everywhere.
async fn act(app: AppHandle, what: &'static str, action: impl FnOnce(&mut Session) -> Result<(), String> + Send + 'static) -> Result<View, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<Shared>();
        let mut session = shared.session.lock().unwrap_or_else(|p| p.into_inner());
        let result = action(&mut session);
        let view = session.view();
        let paths = session.paths.clone();
        drop(session);
        // Metadata only: never the token.
        log(&paths, &format!("{what}: {} {:?}", if result.is_ok() { "ok" } else { "failed" }, view.status));
        if let Err(error) = &result {
            log(&paths, &format!("  {error}"));
        }
        publish(&app, &view);
        view
    })
    .await
    .map_err(|e| e.to_string())
}

// ---- popup placement ----

/// Bottom-right, above the taskbar.
fn place(window: &WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else { return };
    let area = monitor.work_area();
    let Ok(size) = window.outer_size() else { return };
    let margin = (12.0 * monitor.scale_factor()) as i32;
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

fn show_popup(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    place(&window);
    let _ = window.show();
    let _ = window.set_focus();
    let view = app.state::<Shared>().session.lock().unwrap_or_else(|p| p.into_inner()).view();
    let _ = app.emit("view", &view);
}

fn toggle_popup(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    // Clicking the tray icon takes focus from the popup first, which hides it.
    let just_hidden = app.state::<Shared>().hidden_at.lock().unwrap().is_some_and(|at| at.elapsed() < Duration::from_millis(400));
    if window.is_visible().unwrap_or(false) || just_hidden {
        let _ = window.hide();
    } else {
        show_popup(app);
    }
}

// ---- commands the popup calls ----

#[tauri::command]
async fn view(app: AppHandle) -> Result<View, String> {
    act(app, "view", |_| Ok(())).await
}

#[tauri::command]
async fn set_server(app: AppHandle, server: String) -> Result<View, String> {
    act(app, "set server", move |s| s.discover(Some(&server))).await
}

#[tauri::command]
async fn connect(app: AppHandle, code: Option<String>) -> Result<View, String> {
    let view = act(app.clone(), "connect", move |s| s.connect(code.as_deref())).await?;
    // Start with Windows is on by default once connected.
    if view.status == Status::Connected {
        let paths = app.state::<Shared>().session.lock().unwrap().paths.clone();
        if !paths.data_dir.join("startup-chosen").exists() {
            let _ = fs::write(paths.data_dir.join("startup-chosen"), "");
            let _ = set_startup(&paths, true);
            let _ = app.state::<Shared>().menu.startup.set_checked(true);
        }
    }
    Ok(view)
}

#[tauri::command]
async fn refresh(app: AppHandle) -> Result<View, String> {
    act(app, "refresh", Session::refresh).await
}

#[tauri::command]
async fn disconnect(app: AppHandle) -> Result<View, String> {
    act(app, "disconnect", Session::disconnect).await
}

#[tauri::command]
async fn reconnect(app: AppHandle) -> Result<View, String> {
    act(app, "reconnect", Session::reconnect).await
}

#[tauri::command]
async fn retry(app: AppHandle) -> Result<View, String> {
    act(app, "retry", Session::poll).await
}

#[tauri::command]
async fn remove_settings(app: AppHandle) -> Result<View, String> {
    act(app, "remove settings", Session::remove_settings).await
}

/// The popup reports its height; the window follows it and stays anchored.
#[tauri::command]
fn fit(window: WebviewWindow, height: f64) {
    let _ = window.set_size(LogicalSize::new(360.0, height.clamp(200.0, 900.0)));
    place(&window);
}

#[tauri::command]
fn hide(window: WebviewWindow) {
    let _ = window.hide();
}

fn diagnostics(app: &AppHandle) -> String {
    let shared = app.state::<Shared>();
    let session = shared.session.lock().unwrap_or_else(|p| p.into_inner());
    let view = session.view();
    format!(
        "claude-comptroller-connect {}\nlog folder: {}\nstart with Windows: {}\n{}",
        env!("CARGO_PKG_VERSION"),
        session.paths.log_dir().display(),
        startup_enabled(&session.paths),
        // The view never holds the token.
        serde_json::to_string_pretty(&view).unwrap_or_default()
    )
}

fn background(app: AppHandle) {
    std::thread::spawn(move || {
        // At logon: refresh straight away, which also rechecks the group.
        let ticked = |app: &AppHandle, first: bool| {
            let shared = app.state::<Shared>();
            let mut session = shared.session.lock().unwrap_or_else(|p| p.into_inner());
            if first && session.saved.token.is_some() {
                let _ = session.refresh();
            }
            session.tick();
            let view = session.view();
            drop(session);
            publish(app, &view);
            view.status
        };
        let mut status = ticked(&app, true);
        loop {
            // Every 30 s while the server is unreachable, otherwise every 5 minutes.
            let wait = if status == Status::Unreachable { 30 } else { 300 };
            std::thread::sleep(Duration::from_secs(wait));
            status = ticked(&app, false);
        }
    });
}

fn main() {
    let args = args();
    let paths = match &args.settings_root {
        Some(root) => Paths::sandbox(root),
        None => Paths::real().expect("user profile folders"),
    };
    if already_running(&paths) {
        return;
    }
    log(&paths, &format!("start {} sandboxed={}", env!("CARGO_PKG_VERSION"), paths.sandboxed));
    let session = Session::load(paths.clone(), args.device.clone(), args.server.clone());

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![view, set_server, connect, refresh, disconnect, reconnect, retry, remove_settings, fit, hide])
        .setup(move |app| {
            let handle = app.handle();
            let item = |id: &str, text: &str| MenuItem::with_id(handle, id, text, true, None::<&str>);
            let menu = MenuItems {
                refresh: item("refresh", "Refresh token")?,
                toggle: item("toggle", "Disconnect")?,
                startup: CheckMenuItem::with_id(handle, "startup", "Start with Windows", true, startup_enabled(&paths), None::<&str>)?,
            };
            let separator = || PredefinedMenuItem::separator(handle);
            let tray_menu = Menu::with_items(handle, &[
                &item("open", "Open claude-comptroller")?,
                &menu.refresh,
                &item("diagnostics", "Copy diagnostics")?,
                &separator()?,
                &item("logs", "Open log folder")?,
                &menu.startup,
                &separator()?,
                &menu.toggle,
                &item("remove", "Remove comptroller settings…")?,
                &item("quit", "Quit tray app")?,
            ])?;

            app.manage(Shared { session: Mutex::new(session), menu, hidden_at: Mutex::new(None) });

            TrayIconBuilder::with_id(TRAY)
                .icon(tray_icon(handle, "not-connected"))
                .tooltip("claude-comptroller")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        toggle_popup(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| {
                    let app = app.clone();
                    match event.id().as_ref() {
                        "open" => show_popup(&app),
                        "refresh" => { tauri::async_runtime::spawn(async move { let _ = refresh(app).await; }); }
                        "diagnostics" => {
                            let text = diagnostics(&app);
                            let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
                        }
                        "logs" => {
                            let dir = app.state::<Shared>().session.lock().unwrap().paths.log_dir();
                            let _ = fs::create_dir_all(&dir);
                            let _ = std::process::Command::new("explorer").arg(dir).spawn();
                        }
                        "startup" => {
                            let shared = app.state::<Shared>();
                            let paths = shared.session.lock().unwrap().paths.clone();
                            let enabled = !startup_enabled(&paths);
                            let _ = fs::write(paths.data_dir.join("startup-chosen"), "");
                            if let Err(error) = set_startup(&paths, enabled) {
                                log(&paths, &format!("start with Windows: {error}"));
                            }
                            let _ = shared.menu.startup.set_checked(startup_enabled(&paths));
                        }
                        "toggle" => {
                            let disconnected = app.state::<Shared>().session.lock().unwrap().status() == Status::Disconnected;
                            if disconnected {
                                tauri::async_runtime::spawn(async move { let _ = reconnect(app).await; });
                            } else {
                                // Disconnecting blocks Claude, so it goes through the popup's confirmation.
                                show_popup(&app);
                                let _ = app.emit("confirm", "disconnect");
                            }
                        }
                        "remove" => {
                            show_popup(&app);
                            let _ = app.emit("confirm", "remove");
                        }
                        // Quitting doesn't disconnect: the token keeps working until it expires.
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;

            background(handle.clone());
            // First run: open the popup so the user sees how to connect.
            if app.state::<Shared>().session.lock().unwrap().saved.token.is_none() {
                show_popup(handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) => {
                *window.state::<Shared>().hidden_at.lock().unwrap() = Some(Instant::now());
                let _ = window.hide();
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("start the tray app");

    app.run(|_, event| {
        // Hiding the popup must not end the app; only Quit does.
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
