use std::cmp::PartialEq;
use chrono::NaiveDateTime;
use serde_json::{json, Value};
use std::fmt;
use std::fs;
use std::process;

use crate::db::critical_alert::alert_critical_failure;
use crate::db::{atomic_write_text, ensure_file, state_dir};


#[derive(Clone, PartialEq)]
pub enum LogLevel {
    CriticalFail,
    Error,
    Info,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LogLevel::CriticalFail => write!(f, "CriticalFail"),
            LogLevel::Error => write!(f, "Error"),
            LogLevel::Info => write!(f, "Info"),
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
            level:LogLevel::Info,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
    pub fn critical_fail(message:String){
        let log = Self {
            message,
            level:LogLevel::CriticalFail,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
    pub fn error(message:String){
        let log = Self {
            message,
            level:LogLevel::Error,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log);
    }
}



fn log_helper(log: Log) {
    let path = state_dir().join("log.json");
    let _ = ensure_file(&path, "[]\n");

    let mut logs: Vec<Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let ts_display = log.timestamp.format("%Y-%m-%d %I:%M:%S%.f %p");

    let formatted = format!(
        "[{}]({}): {}",
        log.level, ts_display, log.message
    );
    eprintln!("{}", formatted);

    logs.insert(
        0,
        json!({
            "level": log.level.to_string(),
            "timestamp": ts_display.to_string(),
            "message": log.message
        }),
    );

    const MAX_LOG_ENTRIES: usize = 2000;
    if logs.len() > MAX_LOG_ENTRIES {
        logs.truncate(MAX_LOG_ENTRIES);
    }

    if let Ok(serialized) = serde_json::to_string_pretty(&logs) {
        let _ = atomic_write_text(&path, &serialized);
    }
    if log.level == LogLevel::CriticalFail {
        alert_critical_failure(&log.message);
        process::exit(1);
    }
}

