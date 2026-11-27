//! The preflight test against a real service in a console and the mock upstream.
//! Needs `dotnet build service/Comptroller.slnx` first.

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use comptroller_admin::{health, preflight};

struct Running(Vec<Child>, PathBuf);

impl Drop for Running {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.1);
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

#[test]
fn preflight_passes_against_a_console_service() {
    let (public, admin, upstream) = (free_port(), free_port(), free_port());
    let root = std::env::temp_dir().join(format!("comptroller-tui-preflight-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config = root.join("comptroller.json");
    fs::write(&config, format!(
        r#"{{ "Proxy": {{ "ListenUrl": "http://localhost:{public}", "AdminPort": {admin}, "UpstreamUrl": "http://localhost:{upstream}" }},
             "Identity": {{ "Mode": "manual" }} }}"#
    )).unwrap();

    let spawn = |dll: &str, args: &[&str]| Command::new("dotnet").arg(built(dll)).args(args).stdout(Stdio::null()).spawn().unwrap();
    let _running = Running(vec![
        spawn("tools/mock-anthropic/bin/Debug/net10.0/mock-anthropic.dll", &["--urls", &format!("http://localhost:{upstream}")]),
        spawn("service/Comptroller.Service/bin/Debug/net10.0/claude-comptroller-service.dll", &["--config", config.to_str().unwrap()]),
    ], root.clone());

    let began = Instant::now();
    while health::fetch(admin).is_none() {
        assert!(began.elapsed() < Duration::from_secs(30), "the service didn't start");
        std::thread::sleep(Duration::from_millis(200));
    }

    let checks = preflight::run(&config, "comptroller-tui-test-not-installed", |_| {});

    for check in &checks {
        assert!(check.passed(), "{}: {:?}", check.name, check.result);
    }
    assert_eq!(checks.len(), 5);
    assert_eq!(checks[1].result.as_deref(), Ok("console"));
}
