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
///
/// Resolution order:
/// 1. `RBCTL_GIT_SHA` env (explicit override, e.g. CI);
/// 2. `rbctl_git_sha` at the workspace root — written by the OpenWrt
///    package Makefile's Build/Prepare (the SDK builds from a .git-less
///    copy, so this is the only way an apk build knows its sha);
/// 3. `git rev-parse` next to the crate (direct cargo builds in a repo).
fn git_short_sha() -> Option<String> {
    if let Ok(sha) = std::env::var("RBCTL_GIT_SHA") {
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let baked = std::path::Path::new(&dir).join("../../rbctl_git_sha");
        println!("cargo:rerun-if-changed={}", baked.display());
        if let Ok(s) = std::fs::read_to_string(&baked) {
            let s = s.trim();
            if !s.is_empty() && s != "unknown" {
                return Some(s.to_string());
            }
        }
    }
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
