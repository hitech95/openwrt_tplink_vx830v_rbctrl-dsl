//! Simple prefix-based logger for stdout output.

/// Prints lines with a `[tag]` prefix.
pub struct Logger {
    tag: String,
}

impl Logger {
    pub fn new(tag: &str) -> Self {
        Self { tag: tag.to_string() }
    }

    pub fn line(&self, msg: impl std::fmt::Display) {
        println!("[{}] {}", self.tag, msg);
    }

    pub fn pass(&self, label: &str, msg: impl std::fmt::Display) {
        println!("[{}] {label}: PASS {msg}", self.tag);
    }

    pub fn fail(&self, label: &str, msg: impl std::fmt::Display) {
        println!("[{}] {label}: FAIL {msg}", self.tag);
    }
}
