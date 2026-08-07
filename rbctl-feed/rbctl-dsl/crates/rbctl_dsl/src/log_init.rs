//! Logging initialisation — stderr (CLI / foreground) or syslog (daemon under procd).

use log::{Level, LevelFilter, Log, Metadata, Record};

extern "C" {
    fn syslog(priority: libc::c_int, format: *const libc::c_char, ...);
}

// ── shared RUST_LOG parsing ─────────────────────────────────────────────
// Both sinks honour RUST_LOG (default `info`). `trace` is folded onto `debug`
// since the syslog side has no finer granularity than LOG_DEBUG anyway.

fn rust_log_filter() -> LevelFilter {
    match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" | "trace" => LevelFilter::Debug,
        _ => LevelFilter::Info,
    }
}

fn rust_log_level() -> nanologger::LogLevel {
    match rust_log_filter() {
        LevelFilter::Error => nanologger::LogLevel::Error,
        LevelFilter::Warn => nanologger::LogLevel::Warn,
        LevelFilter::Info => nanologger::LogLevel::Info,
        LevelFilter::Debug => nanologger::LogLevel::Debug,
        LevelFilter::Trace | LevelFilter::Off => nanologger::LogLevel::Trace,
    }
}

// ── stderr logger (CLI / foreground daemon) ────────────────────────────
// nanologger defaults to stderr with colour (auto-disabled when not a TTY) and
// no timestamps; we only override the level from RUST_LOG for parity with the
// syslog path. `init()` returns Err only if the logger is already set.

pub fn init_stderr() {
    nanologger::LoggerBuilder::new()
        .level(rust_log_level())
        .init()
        .ok();
}

// ── syslog logger ───────────────────────────────────────────────────────

struct SyslogLogger;

impl Log for SyslogLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let prio = match record.level() {
            Level::Error => libc::LOG_ERR,
            Level::Warn => libc::LOG_WARNING,
            Level::Info => libc::LOG_INFO,
            Level::Debug => libc::LOG_DEBUG,
            Level::Trace => libc::LOG_DEBUG,
        };
        let target = record.target().rsplit("::").next().unwrap_or(record.target());
        let msg = format!("[{target}] {}", record.args());
        if let Ok(c_msg) = std::ffi::CString::new(msg) {
            let fmt = b"%s\0".as_ptr() as *const libc::c_char;
            unsafe { syslog(prio, fmt, c_msg.as_ptr()); }
        }
    }

    fn flush(&self) {}
}

pub fn init_syslog() {
    let ident = std::ffi::CString::new("rbctl-dsl").unwrap();
    unsafe {
        libc::openlog(ident.as_ptr(), libc::LOG_PID, libc::LOG_DAEMON);
    }
    log::set_boxed_logger(Box::new(SyslogLogger)).ok();
    log::set_max_level(rust_log_filter());
}
