use chrono::NaiveDateTime;
use std::cmp::PartialEq;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::db::critical_alert::alert_critical_failure;
use crate::db::state_dir;
use crate::DEV_MODE;

fn attention_ack_path() -> PathBuf {
    state_dir().join("attention_ack")
}

fn wait_for_attention_ack() {
    let ack_path = attention_ack_path();
    let _ = fs::remove_file(&ack_path);
    if io::stdin().is_terminal() {
        Log::console(
            "Fix the issue above, then press Enter to resume polling.".to_string(),
        );
        let mut buf = String::new();
        let _ = io::stdin().read_line(&mut buf);
        return;
    }
    Log::info(
        "Polling paused in background. Fix the issue, then create an empty state/attention_ack file or restart the archiver to resume.".to_string(),
    );
    loop {
        if ack_path.is_file() {
            let _ = fs::remove_file(&ack_path);
            return;
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

static LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
        Self::write(message, LogLevel::Info);
    }
    pub fn error(message: String) {
        Self::write(message, LogLevel::Error);
    }
    pub fn console(message: String) {
        Self::write(message, LogLevel::Console);
    }

    pub fn critical_fail(message: String) -> ! {
        // Evidence must be durable before any UI work or process exit.
        Self::write_durable(message.clone(), LogLevel::CriticalFail);
        alert_critical_failure(&message);
        process::exit(1);
    }

    pub fn pause_for_user_attention(message: String) {
        Self::write_durable(message.clone(), LogLevel::CriticalFail);
        alert_critical_failure(&message);
        wait_for_attention_ack();
        Self::info("Resuming polling after user acknowledgment.".to_string());
    }

    pub fn dev(message: String) {
        Self::write(message, LogLevel::Dev);
    }

    pub fn dev_timing(label: &str, started: std::time::Instant) {
        Self::dev(format!(
            "[timing] {label} {}ms",
            started.elapsed().as_millis()
        ));
    }

    fn write(message: String, level: LogLevel) {
        let log = Self {
            message,
            level,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log, false);
    }

    fn write_durable(message: String, level: LogLevel) {
        let log = Self {
            message,
            level,
            timestamp: chrono::Local::now().naive_local(),
        };
        log_helper(log, true);
    }
}

fn level_log_path(level: &LogLevel) -> Option<PathBuf> {
    match level {
        LogLevel::Info => Some(state_dir().join("info.log")),
        LogLevel::Error => Some(state_dir().join("error.log")),
        LogLevel::CriticalFail => Some(state_dir().join("criticalfail.log")),
        LogLevel::Dev => Some(state_dir().join("diagnostic.log")),
        LogLevel::Console => None,
    }
}

fn append_level_log(path: &PathBuf, line: &str, durable: bool) -> std::io::Result<()> {
    let _guard = LOG_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("log write mutex poisoned");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    if durable {
        file.sync_data()?;
    }
    Ok(())
}

fn log_helper(log: Log, durable: bool) {
    let ts_display = log.timestamp.format("%Y-%m-%d %I:%M:%S%.f %p");
    let formatted = format!("[{}]({}): {}", log.level, ts_display, log.message);

    match log.level {
        LogLevel::Info | LogLevel::Console => {
            println!("{formatted}");
            let _ = std::io::stdout().flush();
        }
        LogLevel::Dev if dev_mode_enabled() => eprintln!("{formatted}"),
        LogLevel::Dev => {}
        _ => eprintln!("{formatted}"),
    }

    if let Some(path) = level_log_path(&log.level) {
        let file_line = format!("[{}]: {}", ts_display, log.message);
        if let Err(e) = append_level_log(&path, &file_line, durable) {
            eprintln!("[LoggingFailure] path={} error={e}", path.display());
        }
    }
}
