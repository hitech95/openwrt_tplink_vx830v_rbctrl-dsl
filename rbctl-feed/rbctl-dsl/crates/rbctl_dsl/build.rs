//! Build script — embed the git sha and build date into the binary as
//! `RBCTL_GIT_SHA` / `RBCTL_BUILD_DATE` (consumed by `src/version.rs`).
//!
//! Both are best-effort: outside a git repo (source tarball builds) or
//! without `git`/`date` on PATH, the values degrade to `unknown` and the
//! build still succeeds.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RBCTL_GIT_SHA={sha}");

    let date = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RBCTL_BUILD_DATE={date}");
}

/// `<short-sha>` or `<short-sha>-dirty` (any uncommitted change).
fn git_short_sha() -> Option<String> {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    if sha.is_empty() {
        return None;
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    Some(if dirty { format!("{sha}-dirty") } else { sha })
}
