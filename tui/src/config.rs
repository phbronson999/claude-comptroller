//! Reads comptroller.json: the same file the service reads. Only the parts the admin
//! terminal needs; unknown parts are ignored. Defaults match the service's.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_SERVICE_NAME: &str = "claude-comptroller";
pub const SERVICE_EXE: &str = "claude-comptroller-service.exe";
pub const CONFIG_FILE: &str = "comptroller.json";
/// The sample shipped in contracts/, written as the default config on install.
pub const SAMPLE: &str = include_str!("../../contracts/comptroller.sample.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Config {
    pub general: General,
    pub proxy: Proxy,
    pub access: Access,
    pub identity: Identity,
    pub data: Data,
    pub logging: Logging,
    pub service: Service,
    pub cost: Cost,
    pub dashboard: Dashboard,
    pub archives: Archives,
    /// Written `TLS`; the service matches names in either case.
    #[serde(rename = "TLS", alias = "Tls", alias = "tls")]
    pub tls: Tls,
}

/// The certificate for an https:// listen URL. The service reads it at startup.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Tls {
    pub certificate_thumbprint: String,
    /// `LocalMachine` for the Windows service, `CurrentUser` for a console run.
    pub certificate_store: String,
}

impl Default for Tls {
    fn default() -> Self {
        Self { certificate_thumbprint: String::new(), certificate_store: "LocalMachine".into() }
    }
}

/// The admin terminal's own settings: the service ignores them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Archives {
    /// Relative to comptroller.json.
    pub folder: String,
    /// `summary` or `raw`: what a new archive keeps unless chosen otherwise.
    pub default_content: String,
    /// The baseline archive's file name in the folder. Empty: none.
    pub baseline: String,
}

impl Default for Archives {
    fn default() -> Self {
        Self { folder: "data\\archives".into(), default_content: "summary".into(), baseline: String::new() }
    }
}

/// The admin terminal's own settings: the service ignores them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Cost {
    pub currency: String,
    /// The day a "month" starts, for billing cycles that don't start on the 1st.
    pub month_start_day: u32,
    /// Relative to comptroller.json. Missing: the table built into this terminal.
    pub pricing_file: String,
    pub seat_plans: Vec<SeatPlan>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeatPlan {
    pub name: String,
    /// `annual` or `monthly`.
    #[serde(default = "annual")]
    pub billing: String,
    /// For the billing period, in `Cost.Currency`. None: not entered yet.
    #[serde(default)]
    pub price: Option<f64>,
    /// `yyyy-mm-dd`, for annual plans: any renewal date, past or next.
    #[serde(default)]
    pub renewal_date: Option<String>,
}

fn annual() -> String {
    "annual".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Dashboard {
    pub refresh_seconds: u64,
    pub p95_warning_seconds: u64,
    pub error_rate_warning_percent: f64,
}

impl Default for Cost {
    fn default() -> Self {
        Self {
            currency: "$".into(),
            month_start_day: 1,
            pricing_file: "pricing.json".into(),
            seat_plans: vec![SeatPlan { name: "Team standard".into(), billing: "annual".into(), price: None, renewal_date: None }],
        }
    }
}

impl Default for Dashboard {
    fn default() -> Self {
        Self { refresh_seconds: 2, p95_warning_seconds: 10, error_rate_warning_percent: 5.0 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct General {
    pub organization_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Proxy {
    pub listen_url: String,
    pub admin_port: u16,
    pub upstream_url: String,
    pub shutdown_timeout_seconds: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Access {
    pub allowed_cidrs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Identity {
    pub mode: String,
    pub allowed_group: String,
    pub code_lifetime_hours: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Data {
    pub folder: String,
    pub retention_days: i64,
    /// HH:mm, local.
    pub prune_time: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Logging {
    pub folder: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Service {
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            proxy: Proxy::default(),
            access: Access::default(),
            identity: Identity::default(),
            data: Data::default(),
            logging: Logging::default(),
            service: Service::default(),
            cost: Cost::default(),
            dashboard: Dashboard::default(),
            archives: Archives::default(),
            tls: Tls::default(),
        }
    }
}

impl Default for Proxy {
    fn default() -> Self {
        Self {
            listen_url: "http://localhost:8080".into(),
            admin_port: 8081,
            upstream_url: "https://api.anthropic.com".into(),
            shutdown_timeout_seconds: 60,
        }
    }
}

impl Default for Access {
    fn default() -> Self {
        Self { allowed_cidrs: vec!["127.0.0.1/32".into(), "::1/128".into()] }
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self { mode: "windows".into(), allowed_group: "Claude Code Users".into(), code_lifetime_hours: 24 }
    }
}

impl Default for Data {
    fn default() -> Self {
        Self { folder: "data".into(), retention_days: 14, prune_time: "02:00".into() }
    }
}

impl Default for Logging {
    fn default() -> Self {
        Self { folder: "logs".into() }
    }
}

impl Default for Service {
    fn default() -> Self {
        Self { name: DEFAULT_SERVICE_NAME.into() }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("not valid: {e}"))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }
}

/// Where the service's files are. Relative folders in the config are relative to the
/// config file, as the service resolves them.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub config_path: PathBuf,
}

impl Place {
    pub fn folder(&self) -> PathBuf {
        self.config_path.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    pub fn resolve(&self, relative: &str) -> PathBuf {
        self.folder().join(relative)
    }

    pub fn data_dir(&self, config: &Config) -> PathBuf {
        self.resolve(&config.data.folder)
    }

    pub fn log_dir(&self, config: &Config) -> PathBuf {
        self.resolve(&config.logging.folder)
    }

    pub fn database(&self, config: &Config) -> PathBuf {
        self.data_dir(config).join("comptroller.db")
    }

    pub fn archive_dir(&self, config: &Config) -> PathBuf {
        self.resolve(&config.archives.folder)
    }

    /// The baseline archive's file, if one is set.
    pub fn baseline(&self, config: &Config) -> Option<PathBuf> {
        let name = config.archives.baseline.trim();
        (!name.is_empty()).then(|| self.archive_dir(config).join(name))
    }

    /// The database with its write-ahead log, which holds recent writes.
    pub fn database_size(&self, config: &Config) -> Option<u64> {
        let db = self.database(config);
        let main = std::fs::metadata(&db).ok()?.len();
        let wal = std::fs::metadata(db.with_extension("db-wal")).map(|m| m.len()).unwrap_or(0);
        Some(main + wal)
    }
}

/// The default install folder: `%ProgramFiles%\claude-comptroller`.
pub fn default_install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join(DEFAULT_SERVICE_NAME)
}

/// The base URL the admin terminal's checks use. Over HTTPS a wildcard or local address is
/// reached by the certificate's own host name (`cert_host`), so the certificate is checked
/// the way developers' computers check it; otherwise as [`local_url`].
pub fn check_url(listen_url: &str, cert_host: Option<&str>) -> String {
    let local = local_url(listen_url);
    let Some(host) = cert_host else { return local };
    let Some(rest) = local.strip_prefix("https://") else { return local };
    let port = rest.rsplit_once(':').filter(|(_, p)| !p.contains(']')).map(|(_, p)| p);
    let current = port.map_or(rest, |p| &rest[..rest.len() - p.len() - 1]);
    if !matches!(current, "localhost" | "127.0.0.1" | "[::1]") {
        return local;
    }
    match port {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    }
}

/// The base URL to reach the service on this computer. A wildcard listen address
/// (`http://+:8080`, `http://0.0.0.0:8080`) is reached through localhost.
pub fn local_url(listen_url: &str) -> String {
    let url = listen_url.trim().trim_end_matches('/');
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_owned() };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, Some(port)),
        _ => (rest, None),
    };
    let host = match host {
        "+" | "*" | "0.0.0.0" | "[::]" => "localhost",
        other => other,
    };
    match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shipped_sample() {
        let config = Config::parse(SAMPLE).unwrap();
        assert_eq!(config.proxy.listen_url, "http://localhost:8080");
        assert_eq!(config.proxy.admin_port, 8081);
        assert_eq!(config.proxy.upstream_url, "https://api.anthropic.com");
        assert_eq!(config.identity.mode, "windows");
        assert_eq!(config.service.name, DEFAULT_SERVICE_NAME);
        assert!(!config.access.allowed_cidrs.is_empty());
        assert_eq!(config.cost.seat_plans.len(), 1);
        assert_eq!(config.cost.seat_plans[0].price, None);
        assert_eq!(config.dashboard.refresh_seconds, 2);
    }

    #[test]
    fn an_empty_file_means_defaults_and_a_broken_one_is_an_error() {
        assert_eq!(Config::parse("{}").unwrap().proxy.admin_port, 8081);
        assert!(Config::parse("{ broken").is_err());
    }

    #[test]
    fn resolves_folders_next_to_the_config() {
        let place = Place { config_path: PathBuf::from(r"C:\svc\comptroller.json") };
        let config = Config::default();
        assert_eq!(place.data_dir(&config), PathBuf::from(r"C:\svc\data"));
        assert_eq!(place.database(&config), PathBuf::from(r"C:\svc\data\comptroller.db"));
    }

    #[test]
    fn wildcard_listen_addresses_become_localhost() {
        assert_eq!(local_url("http://+:8080"), "http://localhost:8080");
        assert_eq!(local_url("https://0.0.0.0:8443/"), "https://localhost:8443");
        assert_eq!(local_url("http://localhost:8080"), "http://localhost:8080");
        assert_eq!(local_url("https://comptroller.example.internal"), "https://comptroller.example.internal");
        // HTTPS checks go to the certificate's name, so it's checked as clients check it.
        let host = Some("claude.example.internal");
        assert_eq!(check_url("https://+:8443", host), "https://claude.example.internal:8443");
        assert_eq!(check_url("https://localhost", host), "https://claude.example.internal");
        assert_eq!(check_url("https://other.example.internal:8443", host), "https://other.example.internal:8443");
        assert_eq!(check_url("http://+:8080", host), "http://localhost:8080");
        assert_eq!(check_url("https://+:8443", None), "https://localhost:8443");
    }
}
