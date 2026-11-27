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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Data {
    pub folder: String,
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
        Self { mode: "windows".into(), allowed_group: "Claude Code Users".into() }
    }
}

impl Default for Data {
    fn default() -> Self {
        Self { folder: "data".into() }
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
    }
}
