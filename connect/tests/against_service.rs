//! Drives the tray app's session against a real service and the mock upstream, including
//! Windows sign-in through WinHTTP. Needs `dotnet build service/Comptroller.slnx` first.

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use connect_core::http::{send, Request, Server};
use connect_core::paths::Paths;
use connect_core::session::{Session, Status};
use connect_core::settings::FileState;

struct Service {
    root: PathBuf,
    url: String,
    processes: Vec<Child>,
}

impl Drop for Service {
    fn drop(&mut self) {
        for process in &mut self.processes {
            let _ = process.kill();
            let _ = process.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn built(relative: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(relative);
    assert!(path.exists(), "build it first: dotnet build service/Comptroller.slnx ({})", path.display());
    path
}

fn start(mode: &str) -> Service {
    let (public, admin, upstream) = (free_port(), free_port(), free_port());
    let root = std::env::temp_dir().join(format!("connect-service-{mode}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let url = format!("http://localhost:{public}");
    let config = root.join("comptroller.json");
    fs::write(&config, format!(
        r#"{{ "Proxy": {{ "ListenUrl": "{url}", "AdminPort": {admin}, "UpstreamUrl": "http://localhost:{upstream}" }},
             "Identity": {{ "Mode": "{mode}", "AllowedGroup": "S-1-5-32-545" }} }}"#,
    )).unwrap();

    let dotnet = |dll: PathBuf, args: &[&str]| {
        Command::new("dotnet").arg(dll).args(args).stdout(Stdio::null()).spawn().unwrap()
    };
    let service = Service {
        url: url.clone(),
        processes: vec![
            dotnet(built("tools/mock-anthropic/bin/Debug/net10.0/mock-anthropic.dll"), &["--urls", &format!("http://localhost:{upstream}")]),
            dotnet(built("service/Comptroller.Service/bin/Debug/net10.0/claude-comptroller-service.dll"), &["--config", config.to_str().unwrap()]),
        ],
        root,
    };

    let server = Server::parse(&url).unwrap();
    let began = Instant::now();
    while send(&server, &Request { method: "GET", path: "/_comptroller/info", headers: &[], body: None, windows_auth: false }).is_err() {
        assert!(began.elapsed() < Duration::from_secs(30), "the service didn't start");
        std::thread::sleep(Duration::from_millis(200));
    }
    service
}

/// A proxied request using only what the app wrote into the Claude Code settings.
fn claude_request(paths: &Paths) -> u32 {
    let settings: serde_json::Value = serde_json::from_str(&fs::read_to_string(&paths.claude_settings).unwrap()).unwrap();
    let server = Server::parse(settings["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap()).unwrap();
    let (name, value) = settings["env"]["ANTHROPIC_CUSTOM_HEADERS"].as_str().unwrap().split_once(": ").unwrap();
    send(&server, &Request { method: "GET", path: "/v1/models", headers: &[(name, value)], body: None, windows_auth: false })
        .unwrap()
        .status
}

fn walk_through(session: &mut Session) {
    assert_eq!(session.status(), Status::Connected);
    let view = session.view();
    assert_eq!((view.checks.claude, view.checks.vscode), (FileState::Ok, FileState::Ok));
    assert_eq!(claude_request(&session.paths), 200);

    let token = session.saved.token.clone();
    session.refresh().unwrap();
    assert_eq!(session.saved.token, token, "a refresh keeps the token");

    session.disconnect().unwrap();
    assert_eq!(session.status(), Status::Disconnected);
    assert_eq!(claude_request(&session.paths), 403, "the settings stay, and the server refuses");
    session.tick();
    assert_eq!(session.status(), Status::Disconnected, "the periodic round doesn't reconnect");

    session.reconnect().unwrap();
    assert_eq!(session.status(), Status::Connected);
    assert_eq!(claude_request(&session.paths), 200);

    session.remove_settings().unwrap();
    assert_eq!(session.status(), Status::NotConnected);
    assert!(!session.paths.claude_settings.exists());
}

#[test]
fn windows_sign_in_through_winhttp() {
    let service = start("windows");
    let paths = Paths::sandbox(&service.root.join("home"));
    let mut session = Session::load(paths, "rust-test".into(), Some(service.url.clone()));

    session.connect(None).unwrap();

    assert!(session.view().user.unwrap().to_lowercase().ends_with(&std::env::var("USERNAME").unwrap().to_lowercase()));
    walk_through(&mut session);
}

#[test]
fn one_time_code() {
    let service = start("manual");
    let paths = Paths::sandbox(&service.root.join("home"));
    let mut session = Session::load(paths, "ignored".into(), Some(service.url.clone()));
    let output = Command::new("dotnet")
        .arg(built("service/Comptroller.Service/bin/Debug/net10.0/claude-comptroller-service.dll"))
        .args(["--config", service.root.join("comptroller.json").to_str().unwrap(), "--create-code", "jdoe", "LAPTOP-1"])
        .output()
        .unwrap();
    let code = String::from_utf8_lossy(&output.stdout).split_whitespace().last().unwrap().to_owned();

    assert!(session.connect(Some("WRONG-CODE-HERE")).is_err());
    session.connect(Some(&code)).unwrap();

    assert_eq!(session.view().device, "LAPTOP-1");
    walk_through(&mut session);
}

#[test]
fn an_unreachable_server_is_reported_and_settings_are_untouched() {
    let paths = Paths::sandbox(&std::env::temp_dir().join(format!("connect-unreachable-{}", std::process::id())));
    let mut session = Session::load(paths.clone(), "x".into(), Some(format!("http://localhost:{}", free_port())));

    assert!(session.connect(None).unwrap_err().contains("Can't reach"));
    assert!(!paths.claude_settings.exists());
}
