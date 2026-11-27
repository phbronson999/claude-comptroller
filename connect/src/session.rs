//! The connection to the server: what we remember, what we can do, and what to show.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::http::{self, Request, Server};
use crate::paths::Paths;
use crate::settings::{self, Checks, Desired};

const DEFAULT_HEADER: &str = "X-Comptroller-Token";
/// Refresh at least this often while the app runs.
const REFRESH_EVERY_SECS: u64 = 24 * 60 * 60;
const EXPIRING_SOON_SECS: i64 = 24 * 60 * 60;

/// Saved between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub server: Option<String>,
    pub token: Option<String>,
    pub token_header: Option<String>,
    pub mode: Option<String>,
    pub organization: String,
    pub user: Option<String>,
    pub device: Option<String>,
    pub expires_utc: Option<String>,
    pub refreshed_at: u64,
    pub last_contact: Option<u64>,
    pub disconnected_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    NotConnected,
    Connected,
    /// The user disconnected this computer.
    Disconnected,
    /// Suspended or revoked by an administrator, or expired.
    Blocked,
    Unreachable,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Today {
    pub requests: Option<u64>,
    pub limit_hits: Option<u64>,
    pub last_request_utc: Option<String>,
}

/// Everything the popup shows. It never holds the token.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub status: Status,
    pub expiring_soon: bool,
    pub server: Option<String>,
    pub server_host: Option<String>,
    pub exposed: bool,
    pub organization: String,
    pub mode: Option<String>,
    pub allowed_group: Option<String>,
    pub user: Option<String>,
    pub device: String,
    pub expires_utc: Option<String>,
    pub seconds_left: Option<i64>,
    pub last_contact: Option<u64>,
    pub disconnected_at: Option<u64>,
    pub today: Today,
    pub checks: Checks,
    pub message: Option<String>,
    pub sandboxed: bool,
    /// Who Windows sign-in will use, before connecting.
    pub windows_user: String,
}

pub struct Session {
    pub paths: Paths,
    pub saved: Saved,
    device: String,
    status: Status,
    today: Today,
    allowed_group: Option<String>,
    /// The last thing that went wrong, or why the server refuses this computer.
    message: Option<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds since 1970 for the service's `yyyy-MM-ddTHH:mm:ss.fffZ`.
pub fn parse_utc(text: &str) -> Option<i64> {
    let part = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (part(0, 4)?, part(5, 7)?, part(8, 10)?);
    let (hour, minute, second) = (part(11, 13)?, part(14, 16)?, part(17, 19)?);
    // Days from civil date (proleptic Gregorian).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// The service's error text, which is written for developers to read.
fn error_message(response: &http::Response) -> String {
    serde_json::from_str::<Value>(&response.body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("The server answered {}.", response.status))
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

impl Session {
    pub fn load(paths: Paths, device: String, server: Option<String>) -> Self {
        let mut saved: Saved = fs::read_to_string(paths.data_dir.join("state.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // A different server means a different token.
        if let Some(server) = server.and_then(|s| Server::parse(&s).ok()).map(|s| s.url()) {
            if saved.server.as_deref() != Some(&server) {
                saved = Saved { server: Some(server), ..Saved::default() };
            }
        }
        let status = if saved.token.is_none() { Status::NotConnected } else { Status::Unreachable };
        Self { paths, saved, device, status, today: Today::default(), allowed_group: None, message: None }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    fn save(&self) -> Result<(), String> {
        fs::create_dir_all(&self.paths.data_dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&self.saved).map_err(|e| e.to_string())?;
        fs::write(self.paths.data_dir.join("state.json"), text).map_err(|e| e.to_string())
    }

    fn header(&self) -> String {
        self.saved.token_header.clone().unwrap_or_else(|| DEFAULT_HEADER.into())
    }

    fn server(&self) -> Result<Server, String> {
        Server::parse(self.saved.server.as_deref().ok_or("Enter the server address.")?)
    }

    fn desired(&self) -> Option<Desired> {
        Some(Desired { base_url: self.saved.server.clone()?, header_name: self.header(), token: self.saved.token.clone()? })
    }

    pub fn view(&self) -> View {
        let server = self.server().ok();
        let seconds_left = self.saved.expires_utc.as_deref().and_then(parse_utc).map(|at| at - now() as i64);
        View {
            status: self.status,
            expiring_soon: self.status == Status::Connected && seconds_left.is_some_and(|s| s < EXPIRING_SOON_SECS),
            server: self.saved.server.clone(),
            server_host: server.as_ref().map(|s| s.host.clone()),
            exposed: server.as_ref().is_some_and(Server::is_exposed),
            organization: self.saved.organization.clone(),
            mode: self.saved.mode.clone(),
            allowed_group: self.allowed_group.clone(),
            user: self.saved.user.clone(),
            device: self.saved.device.clone().unwrap_or_else(|| self.device.clone()),
            expires_utc: self.saved.expires_utc.clone(),
            seconds_left,
            last_contact: self.saved.last_contact,
            disconnected_at: self.saved.disconnected_at,
            today: self.today.clone(),
            checks: settings::check(&self.paths, self.desired().as_ref()),
            message: self.message.clone(),
            sandboxed: self.paths.sandboxed,
            windows_user: match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
                (Ok(domain), Ok(user)) => format!("{domain}\\{user}"),
                (_, Ok(user)) => user,
                _ => String::new(),
            },
        }
    }

    /// Records the outcome of something the user asked for, so the popup shows it.
    fn finish(&mut self, result: Result<(), String>) -> Result<(), String> {
        if let Err(message) = &result {
            self.message = Some(message.clone());
        }
        result
    }

    fn call(&mut self, method: &str, path: &str, body: Option<&str>, windows_auth: bool, token: bool) -> Result<http::Response, String> {
        let server = self.server()?;
        let header = self.header();
        let mut headers = Vec::new();
        if token {
            if let Some(token) = self.saved.token.as_deref() {
                headers.push((header.as_str(), token));
            }
        }
        let result = http::send(&server, &Request { method, path, headers: &headers, body, windows_auth });
        match &result {
            Ok(_) => self.saved.last_contact = Some(now()),
            Err(_) if self.saved.token.is_some() => self.status = Status::Unreachable,
            Err(_) => {}
        }
        result.map_err(|e| format!("Can't reach {}: {}", server.host, e.trim()))
    }

    /// Asks the server how to sign in.
    pub fn discover(&mut self, server: Option<&str>) -> Result<(), String> {
        let result = self.try_discover(server);
        self.finish(result)
    }

    fn try_discover(&mut self, server: Option<&str>) -> Result<(), String> {
        if let Some(server) = server.filter(|s| !s.trim().is_empty()) {
            let url = Server::parse(server)?.url();
            if self.saved.server.as_deref() != Some(&url) {
                self.saved = Saved { server: Some(url), ..Saved::default() };
                self.status = Status::NotConnected;
            }
        }
        let response = self.call("GET", "/_comptroller/info", None, false, false)?;
        if response.status != 200 {
            return Err(error_message(&response));
        }
        let info: Value = serde_json::from_str(&response.body).map_err(|_| "That server isn't claude-comptroller.")?;
        let (Some(mode), Some(header)) = (info["mode"].as_str(), info["tokenHeader"].as_str()) else {
            return Err("That server isn't claude-comptroller.".into());
        };
        self.saved.mode = Some(mode.to_owned());
        self.saved.token_header = Some(header.to_owned());
        self.saved.organization = info["organizationName"].as_str().unwrap_or("").to_owned();
        self.allowed_group = info["allowedGroup"].as_str().map(str::to_owned);
        self.message = None;
        self.save()
    }

    /// First connection. Manual mode needs the one-time code.
    pub fn connect(&mut self, code: Option<&str>) -> Result<(), String> {
        let result = self.try_connect(code);
        self.finish(result)
    }

    fn try_connect(&mut self, code: Option<&str>) -> Result<(), String> {
        self.try_discover(None)?;
        let response = if self.saved.mode.as_deref() == Some("manual") {
            let code = code.map(str::trim).filter(|c| !c.is_empty()).ok_or("Enter the code from your administrator.")?;
            let body = json!({ "code": code }).to_string();
            self.call("POST", "/_comptroller/connect", Some(&body), false, false)?
        } else {
            let path = format!("/_comptroller/connect?device={}", encode(&self.device));
            let mut response = self.call("GET", &path, None, true, true)?;
            // Disconnected earlier and the local state is gone: connecting means reconnecting.
            if response.status == 403 && error_message(&response).contains("Click Reconnect") {
                let path = format!("/_comptroller/reconnect?device={}", encode(&self.device));
                response = self.call("POST", &path, None, true, true)?;
            }
            response
        };
        self.accept(&response)
    }

    /// Takes the token from a connect, refresh or reconnect answer and writes the settings.
    fn accept(&mut self, response: &http::Response) -> Result<(), String> {
        if response.status != 200 {
            return Err(match response.status {
                401 if self.saved.mode.as_deref() == Some("windows") =>
                    "Windows couldn't sign you in. Is this computer joined to your company's domain?".into(),
                _ => error_message(response),
            });
        }
        let session: Value = serde_json::from_str(&response.body).map_err(|_| "Unexpected answer from the server.")?;
        let text = |name: &str| session[name].as_str().map(str::to_owned);
        let token = text("token").ok_or("The server sent no token.")?;
        self.saved.token = Some(token);
        self.saved.user = text("user");
        self.saved.device = text("device");
        self.saved.expires_utc = text("expiresUtc");
        self.saved.refreshed_at = now();
        self.saved.disconnected_at = None;
        self.status = Status::Connected;
        self.message = None;
        self.save()?;
        self.write_settings()
    }

    pub fn write_settings(&mut self) -> Result<(), String> {
        match self.desired() {
            Some(desired) => settings::apply(&self.paths, &desired),
            None => Ok(()),
        }
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        let result = self.try_refresh();
        self.finish(result)
    }

    fn try_refresh(&mut self) -> Result<(), String> {
        let response = if self.saved.mode.as_deref() == Some("manual") {
            self.call("POST", "/_comptroller/connect", None, false, true)?
        } else {
            let path = format!("/_comptroller/connect?device={}", encode(self.saved.device.as_deref().unwrap_or(&self.device)));
            self.call("GET", &path, None, true, true)?
        };
        // A refusal (left the group, suspended) shows up in the next poll too.
        let outcome = self.accept(&response);
        if outcome.is_err() {
            self.saved.refreshed_at = now();
            self.try_poll()?;
        }
        outcome
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        let result = (|| {
            let response = self.call("POST", "/_comptroller/disconnect", None, false, true)?;
            if response.status != 200 {
                return Err(error_message(&response));
            }
            self.saved.disconnected_at = Some(now());
            self.save()?;
            // The settings stay: removing them would send Claude Code straight to Anthropic.
            self.try_poll()
        })();
        self.finish(result)
    }

    pub fn reconnect(&mut self) -> Result<(), String> {
        let result = (|| {
            let response = if self.saved.mode.as_deref() == Some("manual") {
                self.call("POST", "/_comptroller/reconnect", None, false, true)?
            } else {
                let path = format!("/_comptroller/reconnect?device={}", encode(self.saved.device.as_deref().unwrap_or(&self.device)));
                self.call("POST", &path, None, true, true)?
            };
            self.accept(&response)
        })();
        self.finish(result)
    }

    /// Asks the server how this computer stands.
    pub fn poll(&mut self) -> Result<(), String> {
        let result = self.try_poll();
        self.finish(result)
    }

    fn try_poll(&mut self) -> Result<(), String> {
        if self.saved.token.is_none() {
            self.status = Status::NotConnected;
            return Ok(());
        }
        let response = self.call("GET", "/_comptroller/me", None, false, true)?;
        if response.status == 401 {
            // The server no longer knows this token: start over.
            self.forget();
            return Err("This computer's sign-in is no longer valid. Connect again.".into());
        }
        if response.status != 200 {
            self.status = Status::Unreachable;
            return Err(error_message(&response));
        }

        let me: Value = serde_json::from_str(&response.body).map_err(|_| "Unexpected answer from the server.")?;
        let status = me["status"].as_str().unwrap_or("");
        let tray = me["suspendedBy"].as_str() == Some("tray");
        let expired = me["expired"].as_bool().unwrap_or(false);
        self.saved.user = me["user"].as_str().map(str::to_owned);
        self.saved.device = me["device"].as_str().map(str::to_owned);
        self.saved.expires_utc = me["expiresUtc"].as_str().map(str::to_owned);
        self.today = Today {
            requests: me["today"]["requests"].as_u64(),
            limit_hits: me["today"]["limitHits"].as_u64(),
            last_request_utc: me["today"]["lastRequestUtc"].as_str().map(str::to_owned),
        };

        let device = self.saved.device.clone().unwrap_or_default();
        (self.status, self.message) = match status {
            "revoked" => (Status::Blocked, Some("This computer was revoked. Ask your administrator.".into())),
            "suspended" if tray => (Status::Disconnected, None),
            "suspended" => (Status::Blocked, Some(format!("{device} was suspended by an administrator."))),
            _ if expired => (Status::Blocked, Some("The sign-in has expired. Refresh to sign in again.".into())),
            _ => (Status::Connected, None),
        };
        if self.status != Status::Disconnected {
            self.saved.disconnected_at = None;
        }
        self.save()
    }

    /// The periodic round: poll, refresh when due, and keep the settings files correct.
    pub fn tick(&mut self) {
        if self.saved.token.is_none() {
            return;
        }
        if self.poll().is_err() {
            return;
        }
        let expired = self.saved.expires_utc.as_deref().and_then(parse_utc).is_some_and(|at| at <= now() as i64);
        let due = now().saturating_sub(self.saved.refreshed_at) >= REFRESH_EVERY_SECS
            // Windows sign-in can revive an expired token; a code can't.
            || (expired && self.saved.mode.as_deref() == Some("windows") && self.status == Status::Blocked);
        if due && matches!(self.status, Status::Connected | Status::Blocked) {
            let _ = self.refresh();
        }
        if let Err(message) = self.write_settings() {
            self.message = Some(message);
        }
    }

    fn forget(&mut self) {
        self.saved.token = None;
        self.saved.user = None;
        self.saved.expires_utc = None;
        self.saved.disconnected_at = None;
        self.today = Today::default();
        self.status = Status::NotConnected;
        let _ = self.save();
    }

    /// Takes this computer back to talking to Anthropic directly.
    pub fn remove_settings(&mut self) -> Result<(), String> {
        let result = settings::remove(&self.paths, &self.header());
        if result.is_ok() {
            self.forget();
            self.message = Some("Settings removed. Claude Code now talks to Anthropic directly.".into());
        }
        self.finish(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00.000Z"), Some(951_868_800));
        assert_eq!(parse_utc("2026-12-04T09:30:15.123Z"), Some(1_796_376_615));
        assert_eq!(parse_utc("nonsense"), None);
    }

    #[test]
    fn encodes_device_names() {
        assert_eq!(encode("LAPTOP-1"), "LAPTOP-1");
        assert_eq!(encode("my laptop&x=1"), "my%20laptop%26x%3D1");
    }
}
