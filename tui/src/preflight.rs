//! The preflight test: is the path from a developer computer to Anthropic working?
//! It sends no credentials, so it never uses anyone's seat.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::{check_url, Config};
use crate::health;
use crate::service::{self, State};

#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub detail: String,
    pub result: Result<String, String>,
    pub took: Duration,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.result.is_ok()
    }
}

/// Runs every check, calling `report` after each so the screen can show progress.
pub fn run(config_path: &Path, service_name: &str, mut report: impl FnMut(&Check)) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut check = |name: &'static str, detail: String, action: &mut dyn FnMut() -> Result<String, String>| {
        let started = Instant::now();
        let result = action();
        let check = Check { name, detail, result, took: started.elapsed() };
        report(&check);
        checks.push(check);
    };

    let config = Config::load(config_path);
    let file = config_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let admin_port = config.as_ref().map(|c| c.proxy.admin_port).unwrap_or(8081);
    let health = health::fetch(admin_port);
    check("Config valid", file, &mut || match (&config, &health) {
        (Err(e), _) => Err(format!("The file is {e}")),
        (_, Some(h)) if !h.config_ok() => Err(format!("The service rejected it: {}", h.config.error.clone().unwrap_or_default())),
        _ => Ok("ok".into()),
    });

    check("Service running", service_name.to_owned(), &mut || match (service::query(service_name), &health) {
        (Ok(Some(s)), Some(_)) if s.state == State::Running => Ok("yes".into()),
        (Ok(None), Some(h)) if !h.windows_service => Ok("console".into()),
        (Ok(Some(s)), _) => Err(format!("The service is {:?}. Use Takeoff.", s.state).to_lowercase()),
        (Ok(None), None) => Err("Not installed, and nothing answers on the admin port.".into()),
        (Err(e), _) => Err(e),
        (Ok(None), Some(_)) => Ok("yes".into()),
    });

    // What the service actually listens on, falling back to the file.
    let listen = health
        .as_ref()
        .map(|h| h.listen_url.clone())
        .or_else(|| config.as_ref().ok().map(|c| c.proxy.listen_url.clone()))
        .unwrap_or_default();
    // Over HTTPS, by the certificate's own name, so it's checked the way clients check it.
    let tls = health.as_ref().and_then(|h| h.tls.clone());
    let base = check_url(&listen, tls.as_ref().and_then(|t| t.host()));
    let agent = health::agent(Duration::from_secs(5));

    if base.starts_with("https://") {
        let detail = tls.as_ref().map(|t| t.subject.clone()).unwrap_or_else(|| "none reported".into());
        check("Certificate", detail, &mut || match &tls {
            None => Err("The service listens on https:// but reported no certificate. Is it the current version?".into()),
            Some(t) => match t.days_left() {
                Some(days) if days < 0 => Err(format!("It expired on {}. Clients refuse it: choose a new one in Settings → TLS.", &t.not_after_utc[..10])),
                Some(days) => Ok(format!("{days} days left")),
                None => Ok("valid".into()),
            },
        });
    }

    let mut reached = false;
    check("Proxy answering", base.clone(), &mut || {
        let command = format!("curl.exe -H \"X-Comptroller-Client: preflight\" {base}/_comptroller/info");
        let id = crate::activity::start(command);
        let result = agent.get(&format!("{base}/_comptroller/info")).set("X-Comptroller-Client", "preflight").call();
        // Any HTTP answer means the proxy is there, even a refusal.
        reached = matches!(result, Ok(_) | Err(ureq::Error::Status(..)));
        crate::activity::finish(id, result.is_ok(), match &result {
            Ok(r) => r.status().to_string(),
            Err(ureq::Error::Status(code, _)) => code.to_string(),
            Err(e) => e.to_string(),
        });
        match result {
            Ok(_) => Ok("yes".into()),
            Err(ureq::Error::Status(403, _)) => Err("403: this computer isn't in Access.AllowedCidrs.".into()),
            Err(ureq::Error::Status(code, _)) => Err(format!("answered {code}")),
            Err(e) if base.starts_with("https://") && is_tls_error(&e) => Err(tls_problem(&e.to_string(), &base)),
            Err(e) => Err(e.to_string()),
        }
    });
    // The rest go through the proxy: no point repeating why it can't be reached.
    let reachable = reached;
    let skipped = || Err("Skipped: the proxy couldn't be reached (above).".to_string());

    let upstream = health.as_ref().map(|h| h.upstream_url.clone()).unwrap_or_default();
    check("Reached upstream through proxy", "HEAD /api/hello".into(), &mut || match health::upstream_latency(&base) {
        _ if !reachable => skipped(),
        Ok((502 | 504, _)) => Err(format!("The proxy couldn't reach {upstream}.")),
        Ok((code, took)) => Ok(format!("{code} · {} ms", took.as_millis())),
        Err(e) => Err(e),
    });

    check("Streaming not buffered", "first event before last".into(), &mut || {
        if !reachable {
            return skipped();
        }
        let id = crate::activity::start(format!("curl.exe -N -H \"X-Comptroller-Client: preflight\" {base}/_comptroller/preflight/stream"));
        let result = streamed(&agent, &base);
        crate::activity::finish(id, result.is_ok(), match &result {
            Ok(text) => format!("events arrived one by one: {text}"),
            Err(e) => e.clone(),
        });
        result
    });

    checks
}

/// A TLS failure in plain words, from Windows' message.
fn tls_problem(error: &str, base: &str) -> String {
    let text = error.to_ascii_lowercase();
    let host = base.trim_start_matches("https://").split([':', '/']).next().unwrap_or_default();
    let what = if text.contains("root certificate which is not trusted") || text.contains("untrusted root") || text.contains("not trusted") {
        "Windows on this computer doesn't trust the certificate's issuer. Use a certificate from your company's CA or a public one, or trust its root"
    } else if text.contains("principal name") || text.contains("name") && text.contains("match") {
        "The certificate isn't for the name clients use"
    } else if text.contains("expired") || text.contains("validity period") {
        "The certificate has expired"
    } else if text.contains("revocation") {
        "Windows couldn't check whether the certificate was revoked"
    } else {
        "The secure connection failed"
    };
    format!("{what} ({host}). Claude Code and the tray app would refuse it too.")
}

fn is_tls_error(error: &ureq::Error) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    ["certificate", "tls", "ssl", "handshake", "trust"].iter().any(|w| text.contains(w))
}

fn streamed(agent: &ureq::Agent, base: &str) -> Result<String, String> {
    {
        let response = agent
            .get(&format!("{base}/_comptroller/preflight/stream"))
            .set("X-Comptroller-Client", "preflight")
            .call()
            .map_err(|e| e.to_string())?;
        let mut reader = response.into_reader();
        let started = Instant::now();
        let mut buffer = [0u8; 256];
        let mut first = None;
        let mut total = 0;
        loop {
            let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            total += read;
            first.get_or_insert(started.elapsed());
        }
        let first = first.ok_or("The stream was empty.")?;
        let spread = started.elapsed().saturating_sub(first);
        // The service spaces its five events 200 ms apart.
        if total > 0 && spread >= Duration::from_millis(500) {
            Ok("yes".into())
        } else {
            Err(format!("Everything arrived at once ({} ms apart). Something between here and the service buffers replies.", spread.as_millis()))
        }
    }
}
