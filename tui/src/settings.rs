//! Every setting in comptroller.json the admin terminal can change: what it means, its
//! default and limits, and whether the service needs a restart to use it. The limits match
//! the service's own checks (ComptrollerConfig.Validate), so a file saved here is one the
//! service accepts.
//!
//! Saving edits the file in place: comments, formatting and anything this terminal doesn't
//! know about are kept. The previous file is copied to `.bak` first, and the new one is
//! written beside it and then moved over it, so the service never reads half a file.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use jsonc_parser::ParseOptions;
use serde_json::Value;

use crate::activity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Must not be empty.
    Required,
    Url,
    Int { min: i64, max: i64 },
    Bool,
    Choice(&'static [&'static str]),
    /// A list of CIDRs, typed comma-separated.
    Cidrs,
    /// An HTTP header name.
    Header,
    /// A time of day, typed and shown the local way, stored as HH:mm.
    Time,
}

#[derive(Debug)]
pub struct Setting {
    /// The JSON section and property, as in comptroller.json.
    pub section: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// The default, written the way it's typed.
    pub default: &'static str,
    /// The service reads it only when it starts.
    pub restart: bool,
    /// Asked before editing, for settings that change things for every developer.
    pub caution: Option<&'static str>,
}

impl Setting {
    pub fn path(&self) -> String {
        format!("{}.{}", self.section, self.key)
    }
}

/// The groups shown on the Settings screen, in order. Groups whose settings aren't built
/// yet say when they arrive.
pub const GROUPS: [(&str, Option<&str>); 10] = [
    ("General", None),
    ("Proxy", None),
    ("Access", None),
    ("Identity", None),
    ("Data", None),
    ("Logging", None),
    ("Cost", None),
    ("Dashboard", None),
    ("Archives", None),
    ("TLS", Some("later")),
];

const LEVELS: &[&str] = &["Verbose", "Debug", "Information", "Warning", "Error", "Fatal"];
const MODES: &[&str] = &["windows", "manual"];

pub const SETTINGS: &[Setting] = &[
    Setting {
        section: "General",
        key: "OrganizationName",
        label: "Organization name",
        help: "Shown in the tray app and this terminal's header. Leave empty for none.",
        kind: Kind::Text,
        default: "",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Proxy",
        key: "ListenUrl",
        label: "Listen URL",
        help: "Where developers point ANTHROPIC_BASE_URL. Use http://+:8080 to listen on every address.",
        kind: Kind::Url,
        default: "http://localhost:8080",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Proxy",
        key: "AdminPort",
        label: "Admin port",
        help: "Port for /health, on 127.0.0.1 only. This terminal finds the service through it.",
        kind: Kind::Int { min: 1, max: 65535 },
        default: "8081",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Proxy",
        key: "UpstreamUrl",
        label: "Upstream URL",
        help: "Where requests are forwarded. Change it only to test against a fake upstream.",
        kind: Kind::Url,
        default: "https://api.anthropic.com",
        restart: false,
        caution: Some("Every developer's requests will go here instead. Change it only to test against a fake upstream."),
    },
    Setting {
        section: "Proxy",
        key: "ActivityTimeoutMinutes",
        label: "Activity timeout",
        help: "Minutes a reply may go quiet before the proxy gives up on it.",
        kind: Kind::Int { min: 1, max: 60 },
        default: "10",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Proxy",
        key: "ShutdownTimeoutSeconds",
        label: "Shutdown timeout",
        help: "Seconds a stop waits for replies in flight to finish.",
        kind: Kind::Int { min: 5, max: 600 },
        default: "60",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Proxy",
        key: "StripForwardedHeaders",
        label: "Strip X-Forwarded",
        help: "Don't tell the upstream the developer's address. Leave on.",
        kind: Kind::Bool,
        default: "true",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Access",
        key: "AllowedCidrs",
        label: "Allowed addresses",
        help: "Networks that may use the proxy, comma-separated, such as 10.0.0.0/24. Empty allows every address.",
        kind: Kind::Cidrs,
        default: "127.0.0.1/32, ::1/128",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Identity",
        key: "Mode",
        label: "Sign-in mode",
        help: "windows: silent Windows sign-in and a group check. manual: one-time codes from the Users screen.",
        kind: Kind::Choice(MODES),
        default: "windows",
        restart: false,
        caution: Some("This changes how every developer's computer signs in. Computers already connected keep working until their token expires."),
    },
    Setting {
        section: "Identity",
        key: "AllowedGroup",
        label: "Allowed group",
        help: "Windows group allowed to connect: a name such as Claude Code Users, or a SID.",
        kind: Kind::Text,
        default: "Claude Code Users",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Identity",
        key: "TokenLifetimeDays",
        label: "Token lifetime",
        help: "Days a device token lasts. The tray app renews it daily, rechecking the group.",
        kind: Kind::Int { min: 1, max: 90 },
        default: "7",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Identity",
        key: "CodeLifetimeHours",
        label: "Code lifetime",
        help: "Hours a one-time code from the Users screen stays usable (manual mode).",
        kind: Kind::Int { min: 1, max: 168 },
        default: "24",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Identity",
        key: "TokenHeaderName",
        label: "Token header",
        help: "The header that carries each device's token.",
        kind: Kind::Header,
        default: "X-Comptroller-Token",
        restart: false,
        caution: Some("Connected computers send their token under the current name. They fail until each one connects again."),
    },
    Setting {
        section: "Data",
        key: "Folder",
        label: "Data folder",
        help: "Where the database lives, relative to comptroller.json. Moving it doesn't move the database.",
        kind: Kind::Required,
        default: "data",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Data",
        key: "RetentionDays",
        label: "Retention days",
        help: "How many days of requests the black box keeps: today and the days before it. Older ones are deleted each night.",
        kind: Kind::Int { min: 1, max: 365 },
        default: "14",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Data",
        key: "PruneTime",
        label: "Nightly prune time",
        help: "When old requests are deleted each night, on the server's clock.",
        kind: Kind::Time,
        default: "02:00",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Logging",
        key: "Folder",
        label: "Log folder",
        help: "Where log files go, relative to comptroller.json.",
        kind: Kind::Required,
        default: "logs",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Logging",
        key: "Level",
        label: "Log level",
        help: "Information logs one line per request. Debug and Verbose log more.",
        kind: Kind::Choice(LEVELS),
        default: "Information",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Logging",
        key: "RetainedFiles",
        label: "Log files kept",
        help: "How many log files to keep before the oldest is deleted.",
        kind: Kind::Int { min: 1, max: 365 },
        default: "14",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Logging",
        key: "FileSizeLimitMb",
        label: "Log file size",
        help: "MB per log file before a new one starts.",
        kind: Kind::Int { min: 1, max: 1024 },
        default: "20",
        restart: true,
        caution: None,
    },
    Setting {
        section: "Cost",
        key: "Currency",
        label: "Currency symbol",
        help: "Shown before amounts. API prices are in US dollars, so enter seat prices in dollars too.",
        kind: Kind::Required,
        default: "$",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Cost",
        key: "MonthStartDay",
        label: "Month starts on",
        help: "Day of the month a billing month starts, for the Cost screen's \"this month\".",
        kind: Kind::Int { min: 1, max: 28 },
        default: "1",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Cost",
        key: "PricingFile",
        label: "Pricing table",
        help: "API list prices per model, relative to comptroller.json. If the file is missing, the table built into this terminal is used.",
        kind: Kind::Required,
        default: "pricing.json",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Dashboard",
        key: "RefreshSeconds",
        label: "Refresh",
        help: "Seconds between updates on the Dashboard and Radar.",
        kind: Kind::Int { min: 1, max: 60 },
        default: "2",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Dashboard",
        key: "P95WarningSeconds",
        label: "Slow warning",
        help: "Seconds of p95 latency above which it shows orange.",
        kind: Kind::Int { min: 1, max: 600 },
        default: "10",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Dashboard",
        key: "ErrorRateWarningPercent",
        label: "Error warning",
        help: "Percent of requests failing above which errors show pink.",
        kind: Kind::Int { min: 1, max: 100 },
        default: "5",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Archives",
        key: "Folder",
        label: "Archive folder",
        help: "Where archives are saved, relative to comptroller.json. The nightly prune never touches it.",
        kind: Kind::Required,
        default: "data\\archives",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Archives",
        key: "DefaultContent",
        label: "New archives keep",
        help: "summary: figures, tokens and cost only (a few KB). raw: also a copy of every request in the range.",
        kind: Kind::Choice(CONTENTS),
        default: "summary",
        restart: false,
        caution: None,
    },
    Setting {
        section: "Archives",
        key: "Baseline",
        label: "Baseline",
        help: "The archive file the Dashboard and Cost compare against. Set it on the Archives screen with b; empty for none.",
        kind: Kind::Text,
        default: "",
        restart: false,
        caution: None,
    },
];

const CONTENTS: &[&str] = &["summary", "raw"];

/// The index of a setting by its `Section.Key` path.
pub fn index_of(path: &str) -> Option<usize> {
    SETTINGS.iter().position(|s| s.path() == path)
}

pub fn in_group(group: &str) -> impl Iterator<Item = (usize, &'static Setting)> + '_ {
    SETTINGS.iter().enumerate().filter(move |(_, s)| s.section == group)
}

// ── values ───────────────────────────────────────────────────────────────────

/// Checks one typed value and turns it into JSON.
pub fn parse(setting: &Setting, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match setting.kind {
        Kind::Text => Ok(Value::String(text.into())),
        Kind::Required if text.is_empty() => Err("Enter a value".into()),
        Kind::Required => Ok(Value::String(text.into())),
        Kind::Url => {
            let ok = ["http://", "https://"].iter().any(|scheme| text.len() > scheme.len() && text.to_ascii_lowercase().starts_with(scheme));
            if ok && !text.contains(char::is_whitespace) {
                Ok(Value::String(text.into()))
            } else {
                Err("Enter a URL starting with http:// or https://".into())
            }
        }
        Kind::Int { min, max } => match text.parse::<i64>() {
            Ok(n) if (min..=max).contains(&n) => Ok(Value::from(n)),
            _ => Err(format!("Enter a number from {min} to {max}")),
        },
        Kind::Bool => match text.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" => Ok(Value::Bool(true)),
            "false" | "off" | "no" => Ok(Value::Bool(false)),
            _ => Err("on or off".into()),
        },
        Kind::Choice(choices) => choices
            .iter()
            .find(|c| c.eq_ignore_ascii_case(text))
            .map(|c| Value::String((*c).into()))
            .ok_or_else(|| format!("One of: {}", choices.join(", "))),
        Kind::Cidrs => {
            let mut list = Vec::new();
            for part in text.split([',', ' ', ';']).filter(|p| !p.is_empty()) {
                if !is_cidr(part) {
                    return Err(format!("\"{part}\" isn't a network such as 10.0.0.0/24"));
                }
                list.push(Value::String(part.into()));
            }
            Ok(Value::Array(list))
        }
        Kind::Time => crate::dates::parse_time(text)
            .map(|(h, m)| Value::String(format!("{h:02}:{m:02}")))
            .ok_or_else(|| format!("Enter a time such as {}", crate::dates::format_time(2, 0))),
        Kind::Header => {
            let ok = !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
            if ok {
                Ok(Value::String(text.into()))
            } else {
                Err("Letters, digits and dashes only, such as X-Comptroller-Token".into())
            }
        }
    }
}

fn is_cidr(text: &str) -> bool {
    let Some((address, bits)) = text.split_once('/') else { return false };
    let Ok(bits) = bits.parse::<u8>() else { return false };
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => bits <= 32,
        Ok(IpAddr::V6(_)) => bits <= 128,
        Err(_) => false,
    }
}

/// A JSON value written the way it's typed.
pub fn show(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        Value::Array(items) => items.iter().map(show).collect::<Vec<_>>().join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A setting's value as shown and edited: times the local way, everything else as typed.
pub fn display(setting: &Setting, value: &Value) -> String {
    match (setting.kind, value.as_str().and_then(crate::dates::parse_time)) {
        (Kind::Time, Some((h, m))) => crate::dates::format_time(h, m),
        _ => show(value),
    }
}

pub fn default_value(setting: &Setting) -> Value {
    parse(setting, setting.default).expect("defaults are valid")
}

/// Finds a property the way the service does: ignoring case.
fn get_ci<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a Value> {
    object.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// The file's value, or the default when it isn't set.
pub fn current(doc: &Value, setting: &Setting) -> Value {
    doc.as_object()
        .and_then(|o| get_ci(o, setting.section))
        .and_then(Value::as_object)
        .and_then(|section| get_ci(section, setting.key))
        .cloned()
        .unwrap_or_else(|| default_value(setting))
}

/// Checks that span settings: the values in effect after `changes`.
pub fn check_together(doc: &Value, changes: &[(usize, Value)]) -> Vec<String> {
    let value = |key: &str| {
        let index = SETTINGS.iter().position(|s| s.path() == key).expect("known setting");
        changes.iter().find(|(i, _)| *i == index).map(|(_, v)| v.clone()).unwrap_or_else(|| current(doc, &SETTINGS[index]))
    };
    let mut problems = Vec::new();
    for (index, setting) in SETTINGS.iter().enumerate() {
        if changes.iter().any(|(i, _)| *i == index) {
            continue;
        }
        if let Err(e) = parse(setting, &show(&current(doc, setting))) {
            problems.push(format!("{} in the file: {e}", setting.label));
        }
    }
    let listen = value("Proxy.ListenUrl");
    let port = value("Proxy.AdminPort").as_i64().unwrap_or(0);
    if url_port(listen.as_str().unwrap_or_default()) == Some(port) {
        problems.push("The admin port must differ from the port in the listen URL.".into());
    }
    if value("Identity.Mode") == "windows" && value("Identity.AllowedGroup").as_str().is_none_or(|g| g.trim().is_empty()) {
        problems.push("Windows sign-in needs an allowed group.".into());
    }
    problems
}

fn url_port(url: &str) -> Option<i64> {
    let rest = url.split_once("://")?.1;
    let host = rest.split('/').next()?;
    match host.rsplit_once(':') {
        Some((_, port)) if !port.contains(']') => port.parse().ok(),
        _ if url.to_ascii_lowercase().starts_with("https") => Some(443),
        _ => Some(80),
    }
}

// ── the file ─────────────────────────────────────────────────────────────────

fn options() -> ParseOptions {
    ParseOptions { allow_comments: true, allow_trailing_commas: true, ..Default::default() }
}

/// Reads comptroller.json as JSON, allowing the comments and trailing commas the service allows.
pub fn read(path: &Path) -> Result<Value, String> {
    to_value(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
}

fn to_value(text: &str) -> Result<Value, String> {
    let root = CstRootNode::parse(text.trim_start_matches('\u{feff}'), &options()).map_err(|e| format!("not valid JSON: {e}"))?;
    match root.to_serde_value() {
        Some(value) if value.is_object() => Ok(value),
        _ => Err("isn't a JSON object".into()),
    }
}

/// Whether this process may write the file, without changing it.
pub fn writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

fn input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(items) => CstInputValue::Array(items.iter().map(input).collect()),
        Value::Object(map) => CstInputValue::Object(map.iter().map(|(k, v)| (k.clone(), input(v))).collect()),
    }
}

/// The property named `name`, ignoring case, as the service matches names.
fn prop_ci(object: &CstObject, name: &str) -> Option<jsonc_parser::cst::CstObjectProp> {
    object.properties().into_iter().find(|p| p.name().and_then(|n| n.decoded_value().ok()).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// The file's text with `changes` applied, keeping everything else as it was.
pub fn apply(text: &str, changes: &[(usize, Value)]) -> Result<String, String> {
    let text = text.trim_start_matches('\u{feff}');
    let root = CstRootNode::parse(text, &options()).map_err(|e| format!("not valid JSON: {e}"))?;
    let object = root.object_value().ok_or("comptroller.json isn't a JSON object")?;
    for (index, value) in changes {
        let setting = &SETTINGS[*index];
        let section = match prop_ci(&object, setting.section) {
            Some(prop) => prop.object_value().ok_or_else(|| format!("\"{}\" in comptroller.json isn't an object", setting.section))?,
            None => object.append(setting.section, CstInputValue::Object(Vec::new())).object_value().expect("just added"),
        };
        match prop_ci(&section, setting.key) {
            Some(prop) => prop.set_value(input(value)),
            None => {
                section.append(setting.key, input(value));
            }
        }
    }
    Ok(root.to_string())
}

/// The file's text with `Cost.SeatPlans` replaced, keeping everything else as it was.
pub fn apply_plans(text: &str, plans: &[crate::config::SeatPlan]) -> Result<String, String> {
    let text = text.trim_start_matches('\u{feff}');
    let root = CstRootNode::parse(text, &options()).map_err(|e| format!("not valid JSON: {e}"))?;
    let object = root.object_value().ok_or("comptroller.json isn't a JSON object")?;
    let cost = match prop_ci(&object, "Cost") {
        Some(prop) => prop.object_value().ok_or("\"Cost\" in comptroller.json isn't an object")?,
        None => object.append("Cost", CstInputValue::Object(Vec::new())).object_value().expect("just added"),
    };
    let value = input(&serde_json::to_value(plans).map_err(|e| e.to_string())?);
    match prop_ci(&cost, "SeatPlans") {
        Some(prop) => prop.set_value(value),
        None => {
            cost.append("SeatPlans", value);
        }
    }
    Ok(root.to_string())
}

/// Saves the seat plans the way [`save`] saves settings: a `.bak` first, then a whole new file.
pub fn save_plans(path: &Path, plans: &[crate::config::SeatPlan]) -> Result<(), String> {
    let old = std::fs::read_to_string(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    let new = apply_plans(&old, plans)?;
    crate::config::Config::parse(&serde_json::to_string(&to_value(&new)?).unwrap_or_default())?;
    write_with_backup(path, &old, &new, format!("set Cost.SeatPlans ({} plans) in \"{}\"", plans.len(), path.display()), "seat plans saved".into())
}

fn write_with_backup(path: &Path, old: &str, new: &str, command: String, done: String) -> Result<(), String> {
    let backup = backup_path(path);
    activity::track(format!("copy \"{}\" \"{}\"", path.display(), backup.display()), || std::fs::write(&backup, old), |_| "backup written".into())
        .map_err(|e| format!("Can't write the backup {}: {e}", backup.display()))?;
    let temp = path.with_extension("json.new");
    activity::track(
        command,
        || {
            std::fs::write(&temp, new)?;
            // Replaces the old file in one step.
            std::fs::rename(&temp, path)
        },
        |_| done.clone(),
    )
    .map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Can't write {}: {e}", path.display())
    })
}

/// Saves `changes` to the file: a `.bak` of the old file first, then the new file written
/// beside it and moved over it. Each step goes to the Commands panel.
pub fn save(path: &Path, changes: &[(usize, Value)]) -> Result<(), String> {
    let old = std::fs::read_to_string(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    let new = apply(&old, changes)?;

    // What the service will read must pass the same checks.
    let doc = to_value(&new)?;
    if let Some(problem) = check_together(&doc, &[]).into_iter().next() {
        return Err(problem);
    }
    crate::config::Config::parse(&serde_json::to_string(&doc).unwrap_or_default())?;

    let summary: Vec<String> = changes.iter().map(|(i, v)| format!("{} = {}", SETTINGS[*i].path(), show(v))).collect();
    let done = if changes.len() == 1 { "1 setting saved".to_string() } else { format!("{} settings saved", changes.len()) };
    write_with_backup(path, &old, &new, format!("set {} in \"{}\"", summary.join(", "), path.display()), done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SAMPLE;

    fn index(path: &str) -> usize {
        SETTINGS.iter().position(|s| s.path() == path).unwrap()
    }

    #[test]
    fn every_setting_is_in_the_sample_with_its_default() {
        let doc: Value = serde_json::from_str(SAMPLE).unwrap();
        for setting in SETTINGS {
            let section = doc.get(setting.section).unwrap_or_else(|| panic!("{} missing", setting.section));
            let value = section.get(setting.key).unwrap_or_else(|| panic!("{} missing from the sample", setting.path()));
            assert_eq!(value, &default_value(setting), "{}", setting.path());
        }
        assert!(check_together(&doc, &[]).is_empty());
    }

    #[test]
    fn checks_values_like_the_service() {
        let days = &SETTINGS[index("Identity.TokenLifetimeDays")];
        assert_eq!(parse(days, "14").unwrap(), Value::from(14));
        assert!(parse(days, "0").is_err() && parse(days, "91").is_err() && parse(days, "x").is_err());
        let cidrs = &SETTINGS[index("Access.AllowedCidrs")];
        assert_eq!(show(&parse(cidrs, "10.0.0.0/24, ::1/128").unwrap()), "10.0.0.0/24, ::1/128");
        assert!(parse(cidrs, "10.0.0.0/33").is_err() && parse(cidrs, "10.0.0.0").is_err());
        assert_eq!(parse(&SETTINGS[index("Identity.Mode")], "Manual").unwrap(), "manual");
        assert!(parse(&SETTINGS[index("Proxy.ListenUrl")], "localhost:8080").is_err());
    }

    #[test]
    fn the_admin_port_must_differ_from_the_listen_port() {
        let doc: Value = serde_json::from_str(SAMPLE).unwrap();
        assert!(!check_together(&doc, &[(index("Proxy.AdminPort"), Value::from(8080))]).is_empty());
    }

    #[test]
    fn saving_keeps_comments_unknown_keys_and_other_values() {
        let text = "{\n  // who we are\n  \"general\": { \"OrganizationName\": \"\" },\n  \"Proxy\": { \"AdminPort\": 8081, \"Extra\": 1 }\n}\n";
        let out = apply(text, &[(index("General.OrganizationName"), Value::from("Example")), (index("Identity.TokenLifetimeDays"), Value::from(14))]).unwrap();
        assert!(out.contains("// who we are") && out.contains("\"Extra\": 1") && out.contains("\"AdminPort\": 8081"));
        let doc = to_value(&out).unwrap();
        assert_eq!(doc["general"]["OrganizationName"], "Example", "edits the existing section whatever its case");
        assert_eq!(doc["Identity"]["TokenLifetimeDays"], 14, "adds a missing section");
    }

    #[test]
    fn seat_plans_replace_the_list_and_keep_the_rest() {
        use crate::config::{Config, SeatPlan};
        let plans = vec![
            SeatPlan { name: "Team standard".into(), billing: "annual".into(), price: Some(300.0), renewal_date: Some("2027-03-01".into()) },
            SeatPlan { name: "Max".into(), billing: "monthly".into(), price: Some(100.0), renewal_date: None },
        ];
        let out = apply_plans(SAMPLE, &plans).unwrap();
        let config = Config::parse(&serde_json::to_string(&to_value(&out).unwrap()).unwrap()).unwrap();
        assert_eq!(config.cost.seat_plans, plans);
        assert_eq!(config.cost.currency, "$");
        assert_eq!(config.proxy.admin_port, 8081);
        assert!(apply_plans("{}", &plans).unwrap().contains("SeatPlans"));
    }

    #[test]
    fn save_writes_a_backup_and_refuses_what_the_service_would_reject() {
        let dir = std::env::temp_dir().join(format!("comptroller-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("comptroller.json");
        std::fs::write(&path, SAMPLE).unwrap();

        save(&path, &[(index("Data.Folder"), Value::from("D:\\comptroller"))]).unwrap();
        assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), SAMPLE);
        assert_eq!(read(&path).unwrap()["Data"]["Folder"], "D:\\comptroller");

        assert!(save(&path, &[(index("Proxy.AdminPort"), Value::from(8080))]).is_err());
        assert_eq!(read(&path).unwrap()["Proxy"]["AdminPort"], 8081);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
