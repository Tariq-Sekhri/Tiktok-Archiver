use std::cmp::PartialEq;
use chrono::NaiveDateTime;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;

use crate::db::critical_alert::alert_critical_failure;
use crate::db::{atomic_write_text, ensure_file, state_dir};
use crate::DEV_MODE;

pub fn dev_mode_enabled() -> bool {
    *DEV_MODE.get().unwrap_or(&false)
}

#[derive(Clone, PartialEq)]
pub enum LogLevel {
    CriticalFail,
    Error,
    Info,
    Console,
    Dev,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LogLevel::CriticalFail => write!(f, "CriticalFail"),
            LogLevel::Error => write!(f, "Error"),
            LogLevel::Info => write!(f, "Info"),
            LogLevel::Console => write!(f, "Console"),
            LogLevel::Dev => write!(f, "Dev"),
        }
    }
}

pub struct Log {
    message: String,
    level: LogLevel,
    timestamp: NaiveDateTime,
}
impl Log {
    pub fn info(message: String) {
        let log = Self {
            message,
            level: LogLevel::Info,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
    pub fn critical_fail(message: String)-> ! {
        alert_critical_failure(&message);
        let log = Self {
            message,
            level: LogLevel::CriticalFail,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
        process::exit(1);
    }
    pub fn error(message: String) {
        let log = Self {
            message,
            level: LogLevel::Error,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
    pub fn console(message: String) {
        let log = Self {
            message,
            level: LogLevel::Console,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
    pub fn dev(message: String) {
        if !dev_mode_enabled() {
            return;
        }
        let log = Self {
            message,
            level: LogLevel::Dev,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }

    pub fn dev_timing(label: &str, started: std::time::Instant) {
        if !dev_mode_enabled() {
            return;
        }
        let ms = started.elapsed().as_millis();
        Self::dev(format!("[timing] {} {}ms", label, ms));
    }
}

fn level_log_path(level: &LogLevel) -> Option<PathBuf> {
    match level {
        LogLevel::Info => Some(state_dir().join("info.log")),
        LogLevel::Error => Some(state_dir().join("error.log")),
        LogLevel::CriticalFail => Some(state_dir().join("criticalfail.log")),
        _ => None,
    }
}

fn prepend_level_log(path: &PathBuf, line: &str) {
    let _ = ensure_file(path, "");
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<&str> = content.lines().collect();
    lines.insert(0, line);
    let body = lines.join("\n");
    let written = if body.is_empty() {
        String::new()
    } else {
        format!("{}\n", body)
    };
    let _ = atomic_write_text(path, &written);
}

fn log_helper(log: Log) {
    let ts_display = log.timestamp.format("%Y-%m-%d %I:%M:%S%.f %p");
    let formatted = format!("[{}]({}): {}", log.level, ts_display, log.message);

    if log.level == LogLevel::Console || log.level == LogLevel::Info {
        println!("{}", formatted);
        let _ = std::io::stdout().flush();
        if log.level == LogLevel::Info {
            if let Some(path) = level_log_path(&log.level) {
                let file_line = format!("[{}]: {}", ts_display, log.message);
                prepend_level_log(&path, &file_line);
            }
        }
        return;
    }
    if log.level == LogLevel::Dev {
        eprintln!("{}", formatted);
        return;
    }

    eprintln!("{}", formatted);

    if let Some(path) = level_log_path(&log.level) {
        let file_line = format!("[{}]: {}", ts_display, log.message);
        prepend_level_log(&path, &file_line);
    }
}
