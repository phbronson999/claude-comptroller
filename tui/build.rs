//! Embeds the published service exe and tray app when they exist, so `claude-comptroller`
//! can install the service and offer the tray app for download. build.ps1 publishes both
//! first. Without them (plain `cargo build`, CI) everything else still works: Install says
//! how to get a build with the service, and the download page says the tray app isn't there.

use std::path::PathBuf;

fn main() {
    // The service exe, which Install registers as the Windows service.
    embed("COMPTROLLER_SERVICE_EXE", "../publish/service/claude-comptroller-service.exe", "embedded_service", "COMPTROLLER_SERVICE_EXE_PATH");
    // The tray app, which Install puts where the service's download page serves it.
    embed("COMPTROLLER_CONNECT_EXE", "../publish/connect/claude-comptroller-connect.exe", "embedded_connect", "COMPTROLLER_CONNECT_EXE_PATH");
}

/// Embeds the file named by the environment variable `var`, or else the one at `default`
/// (relative to this crate), when it exists: sets cfg `flag` and env `path_var` to its path.
fn embed(var: &str, default: &str, flag: &str, path_var: &str) {
    println!("cargo::rustc-check-cfg=cfg({flag})");
    println!("cargo:rerun-if-env-changed={var}");
    let exe = std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(default));
    println!("cargo:rerun-if-changed={}", exe.display());
    if exe.is_file() {
        println!("cargo:rustc-cfg={flag}");
        println!("cargo:rustc-env={path_var}={}", exe.canonicalize().unwrap().display());
    }
}
