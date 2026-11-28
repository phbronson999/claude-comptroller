//! Install and uninstall, one step at a time. Each step carries the command an admin would
//! type to do the same by hand, which the screen shows.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::activity;
use crate::config::{self, Config, Place, CONFIG_FILE, SERVICE_EXE};
use crate::service::{self, describe, State};

/// The service exe built into this program, if this build has one.
#[cfg(embedded_service)]
pub const EMBEDDED: Option<&[u8]> = Some(include_bytes!(env!("COMPTROLLER_SERVICE_EXE_PATH")));
#[cfg(not(embedded_service))]
pub const EMBEDDED: Option<&[u8]> = None;

/// The tray app built into this program, if this build has one.
#[cfg(embedded_connect)]
pub const EMBEDDED_CONNECT: Option<&[u8]> = Some(include_bytes!(env!("COMPTROLLER_CONNECT_EXE_PATH")));
#[cfg(not(embedded_connect))]
pub const EMBEDDED_CONNECT: Option<&[u8]> = None;

pub const CONNECT_EXE: &str = "claude-comptroller-connect.exe";
/// Beside comptroller.json: the service serves the tray app from here at /_comptroller/download.
pub const DOWNLOADS: &str = "downloads";

const RESTART_DELAYS: [u64; 3] = [5, 5, 30];

#[derive(Clone, Debug)]
pub struct Step {
    pub title: String,
    pub detail: String,
    /// What an admin would type for the same thing.
    pub command: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Progress {
    Started(usize),
    Done(usize, String),
    Failed(usize, String),
    Finished,
}

pub struct Plan {
    pub name: String,
    pub dir: PathBuf,
    pub steps: Vec<Step>,
}

fn account(name: &str) -> String {
    format!(r"NT SERVICE\{name}")
}

fn step(title: &str, detail: String, command: Option<String>) -> Step {
    Step { title: title.into(), detail, command }
}

pub fn install_plan(name: &str, dir: &Path) -> Plan {
    let exe = dir.join(SERVICE_EXE);
    let account = account(name);
    let steps = vec![
        step("Check admin rights", "needs an elevated terminal".into(), None),
        step("Write service exe", exe.display().to_string(), None),
        step(
            "Write tray app",
            format!(r"{}\{CONNECT_EXE} · served at /_comptroller/download", dir.join(DOWNLOADS).display()),
            None,
        ),
        step("Write default config", format!("{} · kept if it already exists", dir.join(CONFIG_FILE).display()), None),
        step(
            "Register service",
            format!("runs as {account}, a virtual account with few rights"),
            Some(format!(r#"sc.exe create {name} binPath= "…\{SERVICE_EXE} --service" start= delayed-auto obj= "{account}""#)),
        ),
        step(
            "Set crash recovery",
            "restart after 5 s, 5 s, 30 s".into(),
            Some(format!("sc.exe failure {name} reset= 86400 actions= restart/5000/restart/5000/restart/30000")),
        ),
        step(
            "Create data folders",
            r"data\  data\archives\  logs\  · data\ readable only by the service and administrators".into(),
            Some(format!(r#"icacls.exe data /inheritance:r /grant *S-1-5-32-544:(OI)(CI)F *S-1-5-18:(OI)(CI)F "{account}:(OI)(CI)M""#)),
        ),
        step("Takeoff", "start the service and wait for /health".into(), Some(format!("sc.exe start {name}"))),
    ];
    Plan { name: name.into(), dir: dir.into(), steps }
}

pub fn uninstall_plan(name: &str, place: &Place, config: &Config, remove_data: bool) -> Plan {
    let mut steps = vec![
        step("Stop the service", "requests in flight finish first".into(), Some(format!("sc.exe stop {name}"))),
        step("Delete the service", name.into(), Some(format!("sc.exe delete {name}"))),
        step("Remove service exe", format!(r"{} · and {DOWNLOADS}\{CONNECT_EXE}", place.folder().join(SERVICE_EXE).display()), None),
    ];
    if remove_data {
        steps.push(step(
            "Remove data, logs and config",
            format!("{}  {}  {}", place.data_dir(config).display(), place.log_dir(config).display(), place.config_path.display()),
            None,
        ));
    }
    Plan { name: name.into(), dir: place.folder(), steps }
}

/// Runs the install plan, reporting each step. Stops at the first failure.
pub fn install(plan: &Plan, progress: &Sender<Progress>) {
    let name = plan.name.as_str();
    let dir = plan.dir.as_path();
    let exe = dir.join(SERVICE_EXE);
    let config_path = dir.join(CONFIG_FILE);

    let steps: Vec<Box<dyn Fn() -> Result<String, String>>> = vec![
        Box::new(|| if service::is_admin() { Ok("running elevated".into()) } else { Err("Not elevated. Reopen claude-comptroller as administrator.".into()) }),
        Box::new(|| {
            let bytes = EMBEDDED.ok_or("This build has no service exe built in. Build it with build.ps1.")?;
            activity::track(
                format!("copy the service exe built into claude-comptroller.exe to \"{}\"", exe.display()),
                || -> Result<(), String> {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    fs::write(&exe, bytes).map_err(|e| format!("{}: {e}", exe.display()))
                },
                |_| crate::bytes(bytes.len() as u64),
            )?;
            Ok(format!("{} · {}", exe.display(), crate::bytes(bytes.len() as u64)))
        }),
        Box::new(|| {
            let Some(bytes) = EMBEDDED_CONNECT else {
                return Ok("not in this build: the download page will say so. build.ps1 includes it.".into());
            };
            let folder = dir.join(DOWNLOADS);
            let file = folder.join(CONNECT_EXE);
            activity::track(
                format!("copy the tray app built into claude-comptroller.exe to \"{}\"", file.display()),
                || -> Result<(), String> {
                    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    fs::write(&file, bytes).map_err(|e| format!("{}: {e}", file.display()))
                },
                |_| crate::bytes(bytes.len() as u64),
            )?;
            Ok(format!("{} · {}", file.display(), crate::bytes(bytes.len() as u64)))
        }),
        Box::new(|| {
            if config_path.exists() {
                Config::load(&config_path).map_err(|e| format!("The existing {CONFIG_FILE} is {e}"))?;
                return Ok(format!("{} · kept the existing file", config_path.display()));
            }
            activity::track(
                format!("write the default \"{}\"", config_path.display()),
                || fs::write(&config_path, config::SAMPLE).map_err(|e| e.to_string()),
                |_| "written".into(),
            )?;
            let listen = Config::parse(config::SAMPLE).map(|c| c.proxy.listen_url).unwrap_or_default();
            Ok(format!("{CONFIG_FILE} · listen {listen}"))
        }),
        Box::new(|| {
            activity::track(
                format!(r#"sc.exe create {name} binPath= "\"{}\" --service" start= delayed-auto obj= "{}""#, exe.display(), account(name)),
                || register(name, &exe),
                |_| "created".into(),
            )
            .map(|_| format!("{} · automatic (delayed)", account(name)))
        }),
        Box::new(|| {
            activity::track(
                format!("sc.exe failure {name} reset= 86400 actions= restart/5000/restart/5000/restart/30000"),
                || recovery(name),
                |_| "set".into(),
            )
            .map(|_| "restart after 5 s, 5 s, 30 s".into())
        }),
        Box::new(|| {
            let config = Config::load(&config_path)?;
            let place = Place { config_path: config_path.clone() };
            folders(name, &place, &config)
        }),
        Box::new(|| takeoff(name, &config_path)),
    ];
    run(&steps, progress);
}

pub fn uninstall(plan: &Plan, place: &Place, config: &Config, progress: &Sender<Progress>) {
    let name = plan.name.as_str();
    let remove_data = plan.steps.len() > 3;
    let mut steps: Vec<Box<dyn Fn() -> Result<String, String>>> = vec![
        Box::new(|| {
            if service::query(name)?.is_none() {
                return Ok("not installed".into());
            }
            service::stop(name)?;
            let wait = Duration::from_secs(config.proxy.shutdown_timeout_seconds as u64 + 30);
            service::wait_for(name, State::Stopped, wait).map(|_| "stopped".into())
        }),
        Box::new(|| {
            let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| describe(&e))?;
            activity::track(format!("sc.exe delete {name}"), || match manager.open_service(name, ServiceAccess::DELETE) {
                Ok(service) => service.delete().map_err(|e| describe(&e)).map(|_| "deleted".to_string()),
                Err(windows_service::Error::Winapi(io)) if io.raw_os_error() == Some(1060) => Ok("already gone".into()),
                Err(error) => Err(describe(&error)),
            }, |text| text.clone())
        }),
        Box::new(|| {
            let exe = place.folder().join(SERVICE_EXE);
            let id = activity::start(format!("del \"{}\"", exe.display()));
            let result = remove_exe(&exe);
            activity::finish(id, result.is_ok(), result.clone().unwrap_or_else(|e| e));
            result?;
            // The tray app the download page served, and its folder if nothing else is in it.
            let connect = place.folder().join(DOWNLOADS).join(CONNECT_EXE);
            if connect.exists() {
                activity::track(format!("del \"{}\"", connect.display()), || fs::remove_file(&connect), |_| "removed".into())
                    .map_err(|e| format!("Couldn't remove {}: {e}", connect.display()))?;
                let _ = fs::remove_dir(place.folder().join(DOWNLOADS));
            }
            Ok("removed".into())
        }),
    ];
    fn remove_exe(exe: &Path) -> Result<String, String> {
        {
            // Windows releases the file a moment after the process ends.
            for _ in 0..20 {
                match fs::remove_file(&exe) {
                    Ok(()) => return Ok("removed".into()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("already gone".into()),
                    Err(_) => std::thread::sleep(Duration::from_millis(250)),
                }
            }
            Err(format!("Couldn't remove {}: it's still in use.", exe.display()))
        }
    }
    if remove_data {
        steps.push(Box::new(|| {
            for dir in [place.data_dir(config), place.log_dir(config)] {
                if dir.exists() {
                    activity::track(
                        format!("rmdir /s /q \"{}\"", dir.display()),
                        || fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display())),
                        |_| "removed".into(),
                    )?;
                }
            }
            for file in [place.config_path.clone(), place.config_path.with_extension("json.bak")] {
                if file.exists() {
                    activity::track(
                        format!("del \"{}\"", file.display()),
                        || fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display())),
                        |_| "removed".into(),
                    )?;
                }
            }
            // The install folder, if nothing else is left in it.
            let _ = fs::remove_dir(place.folder());
            Ok("removed".into())
        }));
    }
    run(&steps, progress);
}

/// Starts the service and waits until /health answers. When it can't, says why: a console
/// run already on its ports, or the service's own last fatal log line.
pub fn takeoff(name: &str, config_path: &Path) -> Result<String, String> {
    let config = Config::load(config_path).unwrap_or_default();
    let port = config.proxy.admin_port;
    // Both would want the same ports, and the console run got there first.
    if let Some(h) = crate::health::fetch(port) {
        if !h.windows_service {
            return Err(format!(
                "A console run of the service (such as dev.ps1) already answers on port {port}. Stop it (Ctrl+C in its window) first: the two can't share ports."
            ));
        }
    }
    let place = Place { config_path: config_path.to_path_buf() };
    let why = |what: String| match last_fatal(&place.log_dir(&config)) {
        Some(reason) => format!("{what} The service's log says: {reason}"),
        None => format!("{what} See the log folder."),
    };
    service::start(name)?;
    service::wait_for(name, State::Running, Duration::from_secs(30)).map_err(why)?;
    for _ in 0..20 {
        if let Some(health) = crate::health::fetch(port) {
            return Ok(format!("running · version {}", health.version));
        }
        // It started, then stopped: no point waiting longer.
        if service::query(name).ok().flatten().is_some_and(|s| s.state == State::Stopped) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(why(format!("The service started but /health doesn't answer on port {port}.")))
}

/// The newest fatal line in the service's log folder, as plain text.
pub fn last_fatal(log_dir: &Path) -> Option<String> {
    let newest = fs::read_dir(log_dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let text = fs::read_to_string(newest.path()).ok()?;
    // Compact JSON, one event a line: "@mt" is the template, its holes are properties.
    text.lines().rev().take(500).find_map(|line| {
        let event: serde_json::Value = serde_json::from_str(line).ok()?;
        if event.get("@l")?.as_str()? != "Fatal" {
            return None;
        }
        let mut message = event.get("@mt")?.as_str()?.to_string();
        if let Some(props) = event.as_object() {
            for (key, value) in props.iter().filter(|(k, _)| !k.starts_with('@')) {
                let shown = value.as_str().map(String::from).unwrap_or_else(|| value.to_string());
                // {Name} or {Name:format}.
                while let Some(start) = message.find(&format!("{{{key}")) {
                    let Some(end) = message[start..].find('}') else { break };
                    message.replace_range(start..start + end + 1, &shown);
                }
            }
        }
        Some(message)
    })
}

fn run(steps: &[Box<dyn Fn() -> Result<String, String> + '_>], progress: &Sender<Progress>) {
    for (i, action) in steps.iter().enumerate() {
        let _ = progress.send(Progress::Started(i));
        match action() {
            Ok(detail) => {
                let _ = progress.send(Progress::Done(i, detail));
            }
            Err(error) => {
                let _ = progress.send(Progress::Failed(i, error));
                return;
            }
        }
    }
    let _ = progress.send(Progress::Finished);
}

fn register(name: &str, exe: &Path) -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .map_err(|e| describe(&e))?;
    let info = ServiceInfo {
        name: name.into(),
        display_name: OsString::from(name),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: vec!["--service".into()],
        dependencies: vec![],
        account_name: Some(account(name).into()),
        account_password: None,
    };
    let service = manager.create_service(&info, ServiceAccess::CHANGE_CONFIG).map_err(|e| describe(&e))?;
    service.set_delayed_auto_start(true).map_err(|e| describe(&e))?;
    let _ = service.set_description("Forwards Claude Code traffic to Anthropic and records who sent it. Managed with claude-comptroller.");
    Ok(())
}

fn recovery(name: &str) -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| describe(&e))?;
    let service = manager
        .open_service(name, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| describe(&e))?;
    let actions = RESTART_DELAYS
        .iter()
        .map(|&secs| ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(secs) })
        .collect();
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
            reboot_msg: None,
            command: None,
            actions: Some(actions),
        })
        .map_err(|e| describe(&e))?;
    // Also restart when it exits with an error, not only on a crash.
    service.set_failure_actions_on_non_crash_failures(true).map_err(|e| describe(&e))
}

fn folders(name: &str, place: &Place, config: &Config) -> Result<String, String> {
    let account = account(name);
    let data = place.data_dir(config);
    let logs = place.log_dir(config);
    for dir in [&data, &data.join("archives"), &logs] {
        activity::track(
            format!("mkdir \"{}\"", dir.display()),
            || fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())),
            |_| "ok".into(),
        )?;
    }
    let grant = |dir: &Path, args: &[&str]| -> Result<(), String> {
        let shown: Vec<String> = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.to_string() }).collect();
        activity::track(
            format!("icacls.exe \"{}\" {}", dir.display(), shown.join(" ")),
            || {
                let output = Command::new("icacls.exe").arg(dir).args(args).output().map_err(|e| e.to_string())?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(format!("icacls {}: {}", dir.display(), String::from_utf8_lossy(&output.stdout).trim()))
                }
            },
            |_| "permissions set".into(),
        )
    };
    let modify = format!("{account}:(OI)(CI)M");
    grant(&logs, &["/grant", &modify])?;
    // The database holds users and token hashes: administrators, SYSTEM and the service only.
    grant(&data, &["/inheritance:r", "/grant", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", &modify])?;
    // The service reads its config and may need to write a .bak later.
    grant(&place.folder(), &["/grant", &format!("{account}:(RX)")])?;
    // A certificate already chosen in a kept config: the new account must read its key.
    let thumbprint = config.tls.certificate_thumbprint.trim();
    if !thumbprint.is_empty() && config.tls.certificate_store == "LocalMachine" {
        crate::certs::grant_key_access(thumbprint, &account).map_err(|e| format!("The HTTPS certificate's private key: {e}"))?;
        return Ok(format!(r"{}  {}  {} · certificate key readable", data.display(), data.join("archives").display(), logs.display()));
    }
    Ok(format!(r"{}  {}  {}", data.display(), data.join("archives").display(), logs.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_show_the_equivalent_commands() {
        let plan = install_plan("claude-comptroller", Path::new(r"C:\Program Files\claude-comptroller"));
        assert_eq!(plan.steps.len(), 8);
        assert!(plan.steps[2].detail.ends_with("served at /_comptroller/download"));
        let register = plan.steps[4].command.as_deref().unwrap();
        assert!(register.contains("start= delayed-auto") && register.contains(r#"obj= "NT SERVICE\claude-comptroller""#));
        assert!(plan.steps[5].command.as_deref().unwrap().ends_with("restart/5000/restart/5000/restart/30000"));

        let place = Place { config_path: PathBuf::from(r"C:\svc\comptroller.json") };
        assert_eq!(uninstall_plan("x", &place, &Config::default(), false).steps.len(), 3);
        assert_eq!(uninstall_plan("x", &place, &Config::default(), true).steps.len(), 4);
    }

    #[test]
    fn the_last_fatal_log_line_says_why_the_service_stopped() {
        let dir = std::env::temp_dir().join(format!("comptroller-fatal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("comptroller-20261127.log"),
            concat!(
                r#"{"@t":"2026-11-27T23:00:00Z","@mt":"Listening on {ListenUrl}","ListenUrl":"http://localhost:8080"}"#, "\n",
                r#"{"@t":"2026-11-27T23:01:00Z","@mt":"Can't start: {Reason} Stop it.","@l":"Fatal","Reason":"Failed to bind to address http://127.0.0.1:8081."}"#, "\n",
                r#"{"@t":"2026-11-27T23:01:01Z","@mt":"Stopped","@l":"Information"}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(last_fatal(&dir).as_deref(), Some("Can't start: Failed to bind to address http://127.0.0.1:8081. Stop it."));
        assert_eq!(last_fatal(&dir.join("missing")), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
