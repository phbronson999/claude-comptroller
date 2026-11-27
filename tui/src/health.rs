//! The service's own report, from `GET http://127.0.0.1:<admin port>/health`.

use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub version: String,
    pub started_utc: String,
    pub uptime_seconds: u64,
    pub windows_service: bool,
    pub config: HealthConfig,
    pub schema_version: u32,
    pub listen_url: String,
    pub upstream_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthConfig {
    pub status: String,
    pub error: Option<String>,
    pub path: String,
    pub loaded_utc: String,
}

impl Health {
    pub fn config_ok(&self) -> bool {
        self.config.status == "ok"
    }
}

pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(timeout).timeout(timeout * 5).redirects(0).build()
}

pub fn url(admin_port: u16) -> String {
    format!("http://127.0.0.1:{admin_port}/health")
}

/// None when nothing answers: the service isn't running.
pub fn fetch(admin_port: u16) -> Option<Health> {
    crate::activity::track(
        format!("curl.exe -s {}", url(admin_port)),
        || -> Result<Health, String> {
            let response = agent(Duration::from_millis(800)).get(&url(admin_port)).call().map_err(|_| "no answer: the service isn't running".to_string())?;
            response.into_json().map_err(|e| e.to_string())
        },
        |h| format!("version {} · config {}", h.version, h.config.status),
    )
    .ok()
}

/// How long a request through the proxy to the upstream takes. Uses Claude Code's own
/// connectivity probe, which carries no credentials and uses no seat.
pub fn upstream_latency(local_url: &str) -> Result<(u16, Duration), String> {
    crate::activity::track(
        format!("curl.exe -I {local_url}/api/hello"),
        || upstream_latency_inner(local_url),
        |(code, took)| format!("{code} · {} ms", took.as_millis()),
    )
}

fn upstream_latency_inner(local_url: &str) -> Result<(u16, Duration), String> {
    let started = Instant::now();
    match agent(Duration::from_secs(3)).head(&format!("{local_url}/api/hello")).call() {
        Ok(response) => Ok((response.status(), started.elapsed())),
        Err(ureq::Error::Status(status, _)) => Ok((status, started.elapsed())),
        Err(error) => Err(error.to_string()),
    }
}

/// Seconds since 1970 for the service's `yyyy-MM-ddTHH:mm:ss.fffZ`.
pub fn parse_utc(text: &str) -> Option<i64> {
    let part = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (part(0, 4)?, part(5, 7)?, part(8, 10)?);
    let (hour, minute, second) = (part(11, 13)?, part(14, 16)?, part(17, 19)?);
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// The service's time format, `yyyy-MM-ddTHH:mm:ss.fffZ`, for seconds since 1970.
pub fn format_utc(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in contracts/api.md must parse, so the contract and this code agree.
    #[test]
    fn reads_the_documented_example() {
        let doc = include_str!("../../contracts/api.md");
        let section = &doc[doc.find("## Health").expect("health section")..];
        let start = section.find("```json").expect("json block") + "```json".len();
        let end = start + section[start..].find("```").unwrap();
        let health: Health = serde_json::from_str(&section[start..end]).unwrap();
        assert!(health.config_ok());
        assert!(health.windows_service);
        assert_eq!(health.schema_version, crate::SCHEMA_VERSION);
    }

    #[test]
    fn parses_service_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_utc("2026-12-04T09:30:15.123Z"), Some(1_796_376_615));
        assert_eq!(format_utc(1_796_376_615), "2026-12-04T09:30:15.000Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00.000Z");
    }
}
