//! Embeds the published service exe when it exists, so `claude-comptroller` can install it.
//! Without it (plain `cargo build`, CI) everything else still works and Install says how to
//! get a build that includes it.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_service)");
    println!("cargo:rerun-if-env-changed=COMPTROLLER_SERVICE_EXE");
    let exe = std::env::var_os("COMPTROLLER_SERVICE_EXE").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../publish/service/claude-comptroller-service.exe")
    });
    println!("cargo:rerun-if-changed={}", exe.display());
    if exe.is_file() {
        println!("cargo:rustc-cfg=embedded_service");
        println!("cargo:rustc-env=COMPTROLLER_SERVICE_EXE_PATH={}", exe.canonicalize().unwrap().display());
    }
}
