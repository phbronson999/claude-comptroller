//! The admin terminal against a real service in a console and the mock upstream.
//! Needs `dotnet build service/Comptroller.slnx` first.

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use comptroller_admin::{directory, health, preflight};

struct Running {
    children: Vec<Child>,
    root: PathBuf,
    config: PathBuf,
    public: u16,
}

impl Drop for Running {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
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

/// The mock upstream and the service in manual sign-in mode, in a folder of their own.
fn start(name: &str) -> Running {
    let (public, admin, upstream) = (free_port(), free_port(), free_port());
    let root = std::env::temp_dir().join(format!("comptroller-tui-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = root.join("comptroller.json");
    fs::write(&config, format!(
        r#"{{ "Proxy": {{ "ListenUrl": "http://localhost:{public}", "AdminPort": {admin}, "UpstreamUrl": "http://localhost:{upstream}" }},
             "Identity": {{ "Mode": "manual" }} }}"#
    )).unwrap();

    let spawn = |dll: &str, args: &[&str]| Command::new("dotnet").arg(built(dll)).args(args).stdout(Stdio::null()).spawn().unwrap();
    let running = Running {
        children: vec![
            spawn("tools/mock-anthropic/bin/Debug/net10.0/mock-anthropic.dll", &["--urls", &format!("http://localhost:{upstream}")]),
            spawn("service/Comptroller.Service/bin/Debug/net10.0/claude-comptroller-service.dll", &["--config", config.to_str().unwrap()]),
        ],
        root,
        config,
        public,
    };

    let began = Instant::now();
    while health::fetch(admin).is_none() {
        assert!(began.elapsed() < Duration::from_secs(30), "the service didn't start");
        std::thread::sleep(Duration::from_millis(200));
    }
    running
}

#[test]
fn preflight_passes_against_a_console_service() {
    let service = start("preflight");
    let checks = preflight::run(&service.config, "comptroller-tui-test-not-installed", |_| {});

    for check in &checks {
        assert!(check.passed(), "{}: {:?}", check.name, check.result);
    }
    assert_eq!(checks.len(), 5);
    assert_eq!(checks[1].result.as_deref(), Ok("console"));
}

/// A code made by the terminal connects a computer, and the terminal's suspend, resume and
/// remove apply to that computer's very next request.
#[test]
fn codes_and_suspensions_from_the_terminal_reach_the_service() {
    let service = start("users");
    let base = format!("http://localhost:{}", service.public);
    let db = service.root.join("data").join("comptroller.db");

    let code = directory::create_code(&db, "Pat", "LAPTOP-9", 24).unwrap();
    let session: serde_json::Value = ureq::post(&format!("{base}/_comptroller/connect"))
        .set("X-Comptroller-Client", "test")
        .send_json(serde_json::json!({ "code": code }))
        .expect("the service accepts the terminal's code")
        .into_json()
        .unwrap();
    let token = session["token"].as_str().unwrap().to_string();
    assert_eq!(session["user"], "Pat");
    assert_eq!(session["device"], "LAPTOP-9");

    let status = || match ureq::get(&format!("{base}/v1/models")).set("X-Comptroller-Token", &token).call() {
        Ok(response) => response.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(status(), 200);

    let listed = directory::load(&db).unwrap().unwrap();
    assert!(listed.codes.is_empty(), "a used code is no longer listed");
    let device = listed.users[0].devices[0].id;

    directory::suspend_device(&db, device).unwrap();
    assert_eq!(status(), 403);
    directory::resume_device(&db, device).unwrap();
    assert_eq!(status(), 200);
    directory::remove_device(&db, device).unwrap();
    assert_eq!(status(), 401);
}
