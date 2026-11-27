//! Windows Service Control: what state the service is in, and starting and stopping it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_service::service::{ServiceAccess, ServiceActionType, ServiceFailureResetPeriod, ServiceStartType, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::activity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Stopping,
    Other,
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub state: State,
    pub exe: PathBuf,
    /// `--config <path>` from the service's command line, if any.
    pub config_arg: Option<PathBuf>,
    pub start_type: String,
    pub account: String,
    pub recovery: String,
}

/// Splits a service command line into the exe and the value of `--config`.
pub fn parse_command_line(line: &str) -> (PathBuf, Option<PathBuf>) {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    let exe = parts.first().map(PathBuf::from).unwrap_or_default();
    let config = parts.iter().position(|p| p == "--config").and_then(|i| parts.get(i + 1)).map(PathBuf::from);
    (exe, config)
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
    ServiceManager::local_computer(None::<&str>, access).map_err(|e| describe(&e))
}

/// Turns Windows' error codes into something an admin can act on.
pub fn describe(error: &windows_service::Error) -> String {
    match error {
        windows_service::Error::Winapi(io) => match io.raw_os_error() {
            Some(5) => "Access denied: this needs an administrator terminal.".into(),
            Some(1060) => "The service isn't installed.".into(),
            Some(1072) => "The service is marked for deletion. Close Services (services.msc) and try again.".into(),
            Some(1073) => "A service with that name already exists.".into(),
            _ => io.to_string(),
        },
        other => other.to_string(),
    }
}

pub fn query(name: &str) -> Result<Option<Installed>, String> {
    activity::track(format!("sc.exe query {name}"), || query_inner(name), |found| match found {
        Some(service) => format!("{:?}", service.state).to_uppercase(),
        None => "not installed".into(),
    })
}

fn query_inner(name: &str) -> Result<Option<Installed>, String> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = match manager.open_service(name, ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG) {
        Ok(service) => service,
        Err(windows_service::Error::Winapi(io)) if io.raw_os_error() == Some(1060) => return Ok(None),
        Err(error) => return Err(describe(&error)),
    };
    let status = service.query_status().map_err(|e| describe(&e))?;
    let config = service.query_config().map_err(|e| describe(&e))?;
    let state = match status.current_state {
        ServiceState::Stopped => State::Stopped,
        ServiceState::StartPending => State::Starting,
        ServiceState::Running => State::Running,
        ServiceState::StopPending => State::Stopping,
        _ => State::Other,
    };
    let (exe, config_arg) = parse_command_line(&config.executable_path.to_string_lossy());
    let delayed = delayed_auto_start(name);
    let start_type = match config.start_type {
        ServiceStartType::AutoStart if delayed => "automatic (delayed)".into(),
        ServiceStartType::AutoStart => "automatic".into(),
        ServiceStartType::OnDemand => "manual".into(),
        ServiceStartType::Disabled => "disabled".into(),
        _ => "boot / system".into(),
    };
    let recovery = match service.get_failure_actions() {
        Ok(actions) => {
            let restarts: Vec<String> = actions
                .actions
                .unwrap_or_default()
                .iter()
                .map(|a| match a.action_type {
                    ServiceActionType::Restart => format!("{} s", a.delay.as_secs()),
                    _ => "–".into(),
                })
                .collect();
            if restarts.is_empty() {
                "none".into()
            } else {
                let reset = match actions.reset_period {
                    ServiceFailureResetPeriod::After(d) if d.as_secs() > 0 => format!(", reset after {} h", d.as_secs() / 3600),
                    _ => String::new(),
                };
                format!("restart after {}{reset}", restarts.join(", "))
            }
        }
        Err(_) => "unknown".into(),
    };
    let account = match config.account_name.map(|a| a.to_string_lossy().into_owned()) {
        Some(a) if a.eq_ignore_ascii_case("LocalSystem") => "Local System".into(),
        Some(a) => a,
        None => "Local System".into(),
    };
    Ok(Some(Installed { state, exe, config_arg, start_type, account, recovery }))
}

fn delayed_auto_start(name: &str) -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"SYSTEM\CurrentControlSet\Services\{name}"))
        .and_then(|key| key.get_value::<u32, _>("DelayedAutostart"))
        .is_ok_and(|v| v == 1)
}

/// Waits until the service reaches `wanted`, or gives up after `timeout`.
pub fn wait_for(name: &str, wanted: State, timeout: Duration) -> Result<(), String> {
    let began = Instant::now();
    loop {
        let state = query(name)?.map(|s| s.state).unwrap_or(State::NotInstalled);
        if state == wanted {
            return Ok(());
        }
        if began.elapsed() > timeout {
            return Err(format!("Still {state:?} after {} s.", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn start(name: &str) -> Result<(), String> {
    activity::track(format!("sc.exe start {name}"), || start_inner(name), |_| "start requested".into())
}

fn start_inner(name: &str) -> Result<(), String> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(name, ServiceAccess::START | ServiceAccess::QUERY_STATUS).map_err(|e| describe(&e))?;
    service.start::<OsString>(&[]).map_err(|e| describe(&e))
}

/// Asks the service to stop. It finishes requests in flight first.
pub fn stop(name: &str) -> Result<(), String> {
    activity::track(format!("sc.exe stop {name}"), || stop_inner(name), |_| "stop requested".into())
}

fn stop_inner(name: &str) -> Result<(), String> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(name, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS).map_err(|e| describe(&e))?;
    match service.stop() {
        Ok(_) => Ok(()),
        // Already stopped.
        Err(windows_service::Error::Winapi(io)) if io.raw_os_error() == Some(1062) => Ok(()),
        Err(error) => Err(describe(&error)),
    }
}

/// Whether this process can change services: an elevated administrator.
pub fn is_admin() -> bool {
    // SAFETY: no arguments, returns a BOOL.
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

/// Starts this program again with "Run as administrator". Windows asks the user first.
pub fn relaunch_as_admin() -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let args: Vec<String> = std::env::args().skip(1).map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a }).collect();
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    activity::start(format!("Start-Process \"{}\" -Verb RunAs", exe.display()));
    // SAFETY: every string outlives the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("runas"),
            &HSTRING::from(exe.as_os_str()),
            &HSTRING::from(args.join(" ")),
            &HSTRING::from(cwd.as_os_str()),
            SW_SHOWNORMAL,
        )
    };
    let _ = PCWSTR::null();
    if result.0 as usize > 32 { Ok(()) } else { Err("Windows didn't start it as administrator.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_service_command_lines() {
        let (exe, config) = parse_command_line(r#""C:\Program Files\claude-comptroller\claude-comptroller-service.exe" --service"#);
        assert_eq!(exe, PathBuf::from(r"C:\Program Files\claude-comptroller\claude-comptroller-service.exe"));
        assert_eq!(config, None);
        let (_, config) = parse_command_line(r#""C:\svc\a.exe" --service --config "D:\conf\comptroller.json""#);
        assert_eq!(config, Some(PathBuf::from(r"D:\conf\comptroller.json")));
    }
}
