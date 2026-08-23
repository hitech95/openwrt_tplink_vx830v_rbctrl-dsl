//! Build/version identification for logs.
//!
//! The daemon prints one banner line at startup so field logs can be
//! mapped to an exact source tree (see plans/wire-format-and-bringup-fixes-plan.md
//! §2.4): session-2 debugging was slowed by not knowing which build the
//! tester ran.

/// Short git sha of the build commit (`unknown` outside a git repo, e.g.
/// source tarballs). Set by `build.rs` via `RBCTL_GIT_SHA`.
pub const GIT_SHA: &str = match option_env!("RBCTL_GIT_SHA") {
    Some(sha) => sha,
    None => "unknown",
};

/// Build date (`YYYY-MM-DD`, host clock at build time). Set by `build.rs`
/// via `RBCTL_BUILD_DATE`.
pub const BUILD_DATE: &str = match option_env!("RBCTL_BUILD_DATE") {
    Some(d) => d,
    None => "unknown",
};

/// One-line version banner, e.g.
/// `rbctl-dsl 0.4.0 (git abc1234, built 2026-08-23)`.
pub fn banner() -> String {
    format!(
        "rbctl-dsl {} (git {}, built {})",
        env!("CARGO_PKG_VERSION"),
        GIT_SHA,
        BUILD_DATE
    )
}

/// Log the banner at info level (daemon startup / selftest header).
pub fn log_banner() {
    log::info!("{}", banner());
}

#[cfg(test)]
mod tests {
    #[test]
    fn banner_contains_version_and_sha() {
        let b = super::banner();
        assert!(b.starts_with("rbctl-dsl "), "got: {b}");
        assert!(b.contains(env!("CARGO_PKG_VERSION")), "got: {b}");
        assert!(b.contains("git "), "got: {b}");
        assert!(b.contains("built "), "got: {b}");
    }
}
