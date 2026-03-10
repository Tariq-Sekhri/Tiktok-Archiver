use std::cmp::PartialEq;
use chrono::NaiveDateTime;
use serde_json::{json, Value};
use std::fmt;
use std::fs;
use std::process;

use crate::db::{ensure_file, state_dir};

pub struct Event {
    message: String,
    log_level: LogLevel,
    timestamp: NaiveDateTime,
}

impl Event {
    pub fn new(message: String, log_level: LogLevel) -> Self {
        Event {
            message,
            log_level,
            timestamp: chrono::Local::now().naive_local(),
        }
    }
}



pub fn log(event: Event) {
    let path = state_dir().join("log.json");
    let _ = ensure_file(&path, "[]\n");

    let mut logs: Vec<Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let ts_display = event.timestamp.format("%Y-%m-%d %I:%M:%S%.f %p");

    let formatted = format!(
        "[{}]({}): {}",
        event.log_level, ts_display, event.message
    );
    eprintln!("{}", formatted);

    logs.insert(
        0,
        json!({
            "level": event.log_level.to_string(),
            "timestamp": ts_display.to_string(),
            "message": event.message
        }),
    );

    fs::write(&path, serde_json::to_string_pretty(&logs).expect("Failed to serialize logs"))
        .expect("Failed to write log file");
    if event.log_level == LogLevel::CriticalFail{
        process::exit(1);
    }
}

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