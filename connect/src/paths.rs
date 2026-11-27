use std::path::{Path, PathBuf};

/// Where the app reads and writes. A sandbox root keeps tests away from the real files.
#[derive(Clone, Debug)]
pub struct Paths {
    pub claude_settings: PathBuf,
    pub vscode_user_dir: PathBuf,
    /// The app's own state, backups and logs.
    pub data_dir: PathBuf,
    pub sandboxed: bool,
}

impl Paths {
    /// `root` stands in for the user profile, with `AppData\Roaming` under it.
    pub fn sandbox(root: &Path) -> Self {
        let roaming = root.join("AppData").join("Roaming");
        let paths = Self::under(root, &roaming, true);
        // The sandbox pretends VS Code is installed.
        let _ = std::fs::create_dir_all(&paths.vscode_user_dir);
        paths
    }

    pub fn real() -> Result<Self, String> {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from).ok_or(format!("{name} isn't set."));
        Ok(Self::under(&var("USERPROFILE")?, &var("APPDATA")?, false))
    }

    fn under(profile: &Path, roaming: &Path, sandboxed: bool) -> Self {
        Self {
            claude_settings: profile.join(".claude").join("settings.json"),
            vscode_user_dir: roaming.join("Code").join("User"),
            data_dir: roaming.join("claude-comptroller"),
            sandboxed,
        }
    }

    pub fn vscode_settings(&self) -> PathBuf {
        self.vscode_user_dir.join("settings.json")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}
