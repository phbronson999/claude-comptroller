//! Writes ANTHROPIC_BASE_URL and ANTHROPIC_CUSTOM_HEADERS into the Claude Code and VS Code
//! settings files. Both files are edited in place, so comments, order and everything else
//! the user has there survive. Never writes an API key or auth token: that would switch
//! Claude Code to API billing.

use std::fs;
use std::path::{Path, PathBuf};

use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use jsonc_parser::ParseOptions;
use serde::Serialize;
use serde_json::Value;

use crate::paths::Paths;

const BASE_URL: &str = "ANTHROPIC_BASE_URL";
const CUSTOM_HEADERS: &str = "ANTHROPIC_CUSTOM_HEADERS";
const VSCODE_KEY: &str = "claudeCode.environmentVariables";
const BOM: char = '\u{feff}';

/// What both files should contain.
#[derive(Clone, Debug)]
pub struct Desired {
    pub base_url: String,
    pub header_name: String,
    pub token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileState {
    /// The file has our values.
    Ok,
    /// Missing or different.
    Missing,
    /// VS Code isn't installed for this user.
    NotInstalled,
    /// The file can't be read as JSON, so it is left alone.
    Unreadable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    pub claude: FileState,
    pub vscode: FileState,
    /// Settings that would put Claude Code on API billing instead of the seat.
    pub billing_warning: Option<String>,
}

#[derive(Clone, Copy)]
enum Target {
    Claude,
    VsCode,
}

impl Target {
    fn path(self, paths: &Paths) -> PathBuf {
        match self {
            Target::Claude => paths.claude_settings.clone(),
            Target::VsCode => paths.vscode_settings(),
        }
    }

    fn installed(self, paths: &Paths) -> bool {
        match self {
            Target::Claude => true,
            Target::VsCode => paths.vscode_user_dir.is_dir(),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Target::Claude => "claude-settings",
            Target::VsCode => "vscode-settings",
        }
    }
}

/// Writes both files. Files already correct aren't touched.
pub fn apply(paths: &Paths, desired: &Desired) -> Result<(), String> {
    let mut errors = Vec::new();
    for target in [Target::Claude, Target::VsCode] {
        if !target.installed(paths) {
            continue;
        }
        if let Err(error) = apply_one(paths, target, desired) {
            errors.push(format!("{}: {error}", target.path(paths).display()));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join(" ")) }
}

fn apply_one(paths: &Paths, target: Target, desired: &Desired) -> Result<(), String> {
    let path = target.path(paths);
    let original = read(&path)?;
    let (root, object) = parse(original.as_deref().unwrap_or(""))?;

    match target {
        Target::Claude => {
            let env = child_object(&object, "env")?;
            set_string(&env, BASE_URL, &desired.base_url);
            let headers = merge_headers(string_of(&env, CUSTOM_HEADERS).as_deref(), desired);
            set_string(&env, CUSTOM_HEADERS, &headers);
        }
        Target::VsCode => {
            if object.get(VSCODE_KEY).is_some() && object.array_value(VSCODE_KEY).is_none() {
                return Err(format!("\"{VSCODE_KEY}\" isn't a list."));
            }
            set_variable(&object, BASE_URL, &desired.base_url);
            let headers = merge_headers(variable_of(&object, CUSTOM_HEADERS).as_deref(), desired);
            set_variable(&object, CUSTOM_HEADERS, &headers);
        }
    }

    let updated = root.to_string();
    if original.as_deref().map(strip_bom) == Some(updated.as_str()) {
        return Ok(());
    }

    back_up(paths, target, original.as_deref())?;
    let with_bom = original.as_deref().is_some_and(|text| text.starts_with(BOM));
    write(&path, &updated, with_bom)
}

pub fn check(paths: &Paths, desired: Option<&Desired>) -> Checks {
    let state = |target: Target| {
        if !target.installed(paths) {
            return FileState::NotInstalled;
        }
        let Ok(Some(value)) = value_of(&target.path(paths)) else {
            return if target.path(paths).exists() { FileState::Unreadable } else { FileState::Missing };
        };
        let Some(desired) = desired else { return FileState::Missing };
        let (base, headers) = match target {
            Target::Claude => (
                value["env"][BASE_URL].as_str().map(str::to_owned),
                value["env"][CUSTOM_HEADERS].as_str().map(str::to_owned),
            ),
            Target::VsCode => (variable_value(&value, BASE_URL), variable_value(&value, CUSTOM_HEADERS)),
        };
        let line = header_line(desired);
        if base.as_deref() == Some(&desired.base_url) && headers.is_some_and(|h| h.lines().any(|l| l == line)) {
            FileState::Ok
        } else {
            FileState::Missing
        }
    };

    Checks { claude: state(Target::Claude), vscode: state(Target::VsCode), billing_warning: billing_warning(paths) }
}

fn billing_warning(paths: &Paths) -> Option<String> {
    let claude = value_of(&paths.claude_settings).ok().flatten().unwrap_or(Value::Null);
    let vscode = value_of(&paths.vscode_settings()).ok().flatten().unwrap_or(Value::Null);
    let mut found = Vec::new();
    for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
        if !claude["env"][key].is_null() || variable_value(&vscode, key).is_some() {
            found.push(key);
        }
    }
    if !claude["apiKeyHelper"].is_null() {
        found.push("apiKeyHelper");
    }
    (!found.is_empty()).then(|| format!("{} is set, which bills the API instead of your seat.", found.join(", ")))
}

/// Takes our entries out again, putting back whatever was there before the first write.
/// Changes the user made since are kept.
pub fn remove(paths: &Paths, header_name: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    for target in [Target::Claude, Target::VsCode] {
        if let Err(error) = remove_one(paths, target, header_name) {
            errors.push(format!("{}: {error}", target.path(paths).display()));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join(" ")) }
}

fn remove_one(paths: &Paths, target: Target, header_name: &str) -> Result<(), String> {
    let (saved, absent) = backup_paths(paths, target);
    // No backup means we never wrote this file.
    if !saved.exists() && !absent.exists() {
        return Ok(());
    }

    let path = target.path(paths);
    if let Some(current) = read(&path)? {
        let before = value_of(&saved)?.unwrap_or(Value::Null);
        let (root, object) = parse(&current)?;
        let is_ours = |line: &str| line_is_header(line, header_name);

        match target {
            Target::Claude => {
                if let Some(env) = object.object_value("env") {
                    let old = |key: &str| before["env"][key].as_str().map(str::to_owned);
                    restore_string(&env, BASE_URL, old(BASE_URL));
                    let headers = restore_headers(string_of(&env, CUSTOM_HEADERS), old(CUSTOM_HEADERS), is_ours);
                    restore_string(&env, CUSTOM_HEADERS, headers);
                    if env.properties().is_empty() && before.get("env").is_none() {
                        if let Some(prop) = object.get("env") {
                            prop.remove();
                        }
                    }
                }
            }
            Target::VsCode => {
                let old = |key: &str| variable_value(&before, key);
                restore_variable(&object, BASE_URL, old(BASE_URL));
                let headers = restore_headers(variable_of(&object, CUSTOM_HEADERS), old(CUSTOM_HEADERS), is_ours);
                restore_variable(&object, CUSTOM_HEADERS, headers);
                if object.array_value(VSCODE_KEY).is_some_and(|a| a.elements().is_empty()) && before.get(VSCODE_KEY).is_none() {
                    if let Some(prop) = object.get(VSCODE_KEY) {
                        prop.remove();
                    }
                }
            }
        }

        if absent.exists() && object.properties().is_empty() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        } else {
            write(&path, &root.to_string(), current.starts_with(BOM))?;
        }
    }

    for file in [saved, absent] {
        if file.exists() {
            fs::remove_file(file).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---- files ----

fn read(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix(BOM).unwrap_or(text)
}

/// Atomic, and follows a symlinked settings file instead of replacing the link.
fn write(path: &Path, text: &str, with_bom: bool) -> Result<(), String> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut temp = path.clone().into_os_string();
    temp.push(".comptroller-tmp");
    let body = if with_bom { format!("{BOM}{text}") } else { text.to_owned() };
    fs::write(&temp, body).map_err(|e| e.to_string())?;
    fs::rename(&temp, &path).map_err(|e| e.to_string())
}

fn backup_paths(paths: &Paths, target: Target) -> (PathBuf, PathBuf) {
    let folder = paths.data_dir.join("backups");
    (folder.join(format!("{}.original.json", target.name())), folder.join(format!("{}.absent", target.name())))
}

/// Keeps the file as it was before we first touched it. Never overwritten until removal.
fn back_up(paths: &Paths, target: Target, original: Option<&str>) -> Result<(), String> {
    let (saved, absent) = backup_paths(paths, target);
    if saved.exists() || absent.exists() {
        return Ok(());
    }
    fs::create_dir_all(saved.parent().expect("backup folder")).map_err(|e| e.to_string())?;
    match original {
        Some(text) => fs::write(saved, text),
        None => fs::write(absent, "This file didn't exist before claude-comptroller wrote it."),
    }
    .map_err(|e| e.to_string())
}

// ---- JSON ----

fn options() -> ParseOptions {
    ParseOptions { allow_comments: true, allow_trailing_commas: true, ..Default::default() }
}

fn parse(text: &str) -> Result<(CstRootNode, CstObject), String> {
    let text = strip_bom(text);
    let text = if text.trim().is_empty() { "{}\n" } else { text };
    let root = CstRootNode::parse(text, &options()).map_err(|e| format!("not valid JSON ({e}). Left unchanged."))?;
    let object = match root.value() {
        // Only comments in the file.
        None => root.object_value_or_set(),
        Some(_) => root.object_value().ok_or("the file isn't a JSON object. Left unchanged.")?,
    };
    Ok((root, object))
}

fn value_of(path: &Path) -> Result<Option<Value>, String> {
    let Some(text) = read(path)? else { return Ok(None) };
    let (root, _) = parse(&text)?;
    Ok(root.to_serde_value())
}

/// Gets or creates an object property, refusing to replace a value of another kind.
fn child_object(parent: &CstObject, name: &str) -> Result<CstObject, String> {
    if parent.get(name).is_some() && parent.object_value(name).is_none() {
        return Err(format!("\"{name}\" isn't an object. Left unchanged."));
    }
    Ok(parent.object_value_or_set(name))
}

fn string_of(object: &CstObject, name: &str) -> Option<String> {
    object.get(name)?.value()?.to_serde_value()?.as_str().map(str::to_owned)
}

fn set_string(object: &CstObject, name: &str, value: &str) {
    match object.get(name) {
        Some(_) if string_of(object, name).as_deref() == Some(value) => {}
        Some(prop) => prop.set_value(CstInputValue::String(value.to_owned())),
        None => {
            object.append(name, CstInputValue::String(value.to_owned()));
        }
    }
}

fn restore_string(object: &CstObject, name: &str, value: Option<String>) {
    match value {
        Some(value) => set_string(object, name, &value),
        None => {
            if let Some(prop) = object.get(name) {
                prop.remove();
            }
        }
    }
}

// ---- VS Code's list of {name, value} ----

fn find_variable(object: &CstObject, name: &str) -> Option<CstObject> {
    object
        .array_value(VSCODE_KEY)?
        .elements()
        .into_iter()
        .filter_map(|element| element.as_object())
        .find(|entry| string_of(entry, "name").as_deref() == Some(name))
}

fn variable_of(object: &CstObject, name: &str) -> Option<String> {
    string_of(&find_variable(object, name)?, "value")
}

fn variable_value(settings: &Value, name: &str) -> Option<String> {
    settings[VSCODE_KEY]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == name)?["value"]
        .as_str()
        .map(str::to_owned)
}

fn set_variable(object: &CstObject, name: &str, value: &str) {
    if let Some(entry) = find_variable(object, name) {
        set_string(&entry, "value", value);
        return;
    }
    object.array_value_or_set(VSCODE_KEY).append(CstInputValue::Object(vec![
        ("name".to_owned(), CstInputValue::String(name.to_owned())),
        ("value".to_owned(), CstInputValue::String(value.to_owned())),
    ]));
}

fn restore_variable(object: &CstObject, name: &str, value: Option<String>) {
    match value {
        Some(value) => set_variable(object, name, &value),
        None => {
            if let Some(entry) = find_variable(object, name) {
                entry.remove();
            }
        }
    }
}

// ---- ANTHROPIC_CUSTOM_HEADERS holds one "Name: value" per line ----

fn header_line(desired: &Desired) -> String {
    format!("{}: {}", desired.header_name, desired.token)
}

fn line_is_header(line: &str, header_name: &str) -> bool {
    line.split_once(':').is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(header_name))
}

/// The user's other headers stay; ours is replaced.
fn merge_headers(existing: Option<&str>, desired: &Desired) -> String {
    let ours = header_line(desired);
    let mut lines: Vec<&str> = existing
        .unwrap_or("")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line_is_header(line, &desired.header_name))
        .collect();
    lines.push(&ours);
    lines.join("\n")
}

fn restore_headers(current: Option<String>, before: Option<String>, is_ours: impl Fn(&str) -> bool) -> Option<String> {
    let mut lines: Vec<String> = current?.lines().filter(|l| !l.trim().is_empty() && !is_ours(l)).map(str::to_owned).collect();
    lines.extend(before.unwrap_or_default().lines().filter(|l| is_ours(l)).map(str::to_owned));
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox {
        root: PathBuf,
        paths: Paths,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("connect-settings-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let paths = Paths::sandbox(&root);
            Self { root, paths }
        }

        fn put(&self, path: &Path, text: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn desired(token: &str) -> Desired {
        Desired { base_url: "https://comptroller.example.internal".into(), header_name: "X-Comptroller-Token".into(), token: token.into() }
    }

    const CLAUDE: &str = r#"{
  "theme": "dark",
  "env": {
    "MY_OWN": "1",
    "ANTHROPIC_CUSTOM_HEADERS": "X-Team: red"
  }
}
"#;

    const VSCODE: &str = r#"{
    // my editor
    "editor.fontSize": 14, /* keep */
    "claudeCode.environmentVariables": [
        { "name": "MY_OWN", "value": "1" },
    ],
}
"#;

    #[test]
    fn writes_both_files_and_keeps_everything_else() {
        let sandbox = Sandbox::new("merge");
        sandbox.put(&sandbox.paths.claude_settings, CLAUDE);
        sandbox.put(&sandbox.paths.vscode_settings(), VSCODE);

        apply(&sandbox.paths, &desired("abc")).unwrap();

        let claude = value_of(&sandbox.paths.claude_settings).unwrap().unwrap();
        assert_eq!(claude["theme"], "dark");
        assert_eq!(claude["env"]["MY_OWN"], "1");
        assert_eq!(claude["env"][BASE_URL], "https://comptroller.example.internal");
        assert_eq!(claude["env"][CUSTOM_HEADERS], "X-Team: red\nX-Comptroller-Token: abc");

        let text = fs::read_to_string(sandbox.paths.vscode_settings()).unwrap();
        assert!(text.contains("// my editor") && text.contains("/* keep */"), "{text}");
        let vscode = value_of(&sandbox.paths.vscode_settings()).unwrap().unwrap();
        assert_eq!(variable_value(&vscode, "MY_OWN").as_deref(), Some("1"));
        assert_eq!(variable_value(&vscode, CUSTOM_HEADERS).as_deref(), Some("X-Comptroller-Token: abc"));

        let checks = check(&sandbox.paths, Some(&desired("abc")));
        assert_eq!((checks.claude, checks.vscode), (FileState::Ok, FileState::Ok));
        assert!(checks.billing_warning.is_none());
    }

    #[test]
    fn a_new_token_replaces_the_old_line_only() {
        let sandbox = Sandbox::new("replace");
        sandbox.put(&sandbox.paths.claude_settings, CLAUDE);

        apply(&sandbox.paths, &desired("first")).unwrap();
        apply(&sandbox.paths, &desired("second")).unwrap();

        let claude = value_of(&sandbox.paths.claude_settings).unwrap().unwrap();
        assert_eq!(claude["env"][CUSTOM_HEADERS], "X-Team: red\nX-Comptroller-Token: second");
        assert_eq!(check(&sandbox.paths, Some(&desired("first"))).claude, FileState::Missing);
    }

    #[test]
    fn removal_restores_the_original_files() {
        let sandbox = Sandbox::new("restore");
        sandbox.put(&sandbox.paths.claude_settings, CLAUDE);
        sandbox.put(&sandbox.paths.vscode_settings(), VSCODE);
        apply(&sandbox.paths, &desired("abc")).unwrap();
        apply(&sandbox.paths, &desired("def")).unwrap();

        remove(&sandbox.paths, "X-Comptroller-Token").unwrap();

        let value = |text: &str| parse(text).unwrap().0.to_serde_value().unwrap();
        for (path, original) in [(sandbox.paths.claude_settings.clone(), CLAUDE), (sandbox.paths.vscode_settings(), VSCODE)] {
            let after = fs::read_to_string(&path).unwrap();
            assert_eq!(value(&after), value(original), "{after}");
        }
        assert!(fs::read_to_string(sandbox.paths.vscode_settings()).unwrap().contains("// my editor"));
    }

    #[test]
    fn removal_keeps_later_changes_and_puts_back_an_earlier_base_url() {
        let sandbox = Sandbox::new("later");
        sandbox.put(&sandbox.paths.claude_settings, r#"{ "env": { "ANTHROPIC_BASE_URL": "https://old.example" } }"#);
        apply(&sandbox.paths, &desired("abc")).unwrap();
        let (root, object) = parse(&fs::read_to_string(&sandbox.paths.claude_settings).unwrap()).unwrap();
        object.append("added", CstInputValue::Bool(true));
        fs::write(&sandbox.paths.claude_settings, root.to_string()).unwrap();

        remove(&sandbox.paths, "X-Comptroller-Token").unwrap();

        let claude = value_of(&sandbox.paths.claude_settings).unwrap().unwrap();
        assert_eq!(claude["added"], true);
        assert_eq!(claude["env"][BASE_URL], "https://old.example");
        assert!(claude["env"].get(CUSTOM_HEADERS).is_none());
    }

    #[test]
    fn files_that_did_not_exist_are_removed_again() {
        let sandbox = Sandbox::new("absent");
        apply(&sandbox.paths, &desired("abc")).unwrap();
        assert!(sandbox.paths.claude_settings.exists() && sandbox.paths.vscode_settings().exists());

        remove(&sandbox.paths, "X-Comptroller-Token").unwrap();

        assert!(!sandbox.paths.claude_settings.exists() && !sandbox.paths.vscode_settings().exists());
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        let sandbox = Sandbox::new("broken");
        sandbox.put(&sandbox.paths.claude_settings, "{ not json");

        assert!(apply(&sandbox.paths, &desired("abc")).is_err());

        assert_eq!(fs::read_to_string(&sandbox.paths.claude_settings).unwrap(), "{ not json");
        assert_eq!(check(&sandbox.paths, None).claude, FileState::Unreadable);
        // The other file is still written.
        assert!(sandbox.paths.vscode_settings().exists());
    }

    #[test]
    fn warns_about_api_billing_settings() {
        let sandbox = Sandbox::new("billing");
        sandbox.put(&sandbox.paths.claude_settings, r#"{ "env": { "ANTHROPIC_API_KEY": "x" } }"#);

        assert!(check(&sandbox.paths, None).billing_warning.unwrap().contains("ANTHROPIC_API_KEY"));
    }
}
