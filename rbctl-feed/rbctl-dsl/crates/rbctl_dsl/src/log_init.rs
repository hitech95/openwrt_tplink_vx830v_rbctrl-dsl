//! Logging initialisation — stdout (interactive) or syslog (daemon under procd).

use log::{Level, LevelFilter, Log, Metadata, Record};

extern "C" {
    fn syslog(priority: libc::c_int, format: *const libc::c_char, ...);
}

// ── stdout logger ───────────────────────────────────────────────────────

pub fn init_stdout() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    )
    .target(env_logger::Target::Stdout)
    .format(|buf, record| {
        let target = record.target().rsplit("::").next().unwrap_or(record.target());
        use std::io::Write;
        writeln!(buf, "[{} {}] {}", record.level(), target, record.args())
    })
    .init();
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
    let max = match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" | "trace" => LevelFilter::Debug,
        _ => LevelFilter::Info,
    };
    log::set_max_level(max);
}
