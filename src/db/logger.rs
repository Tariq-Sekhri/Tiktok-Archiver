use chrono::NaiveDateTime;
use std::cmp::PartialEq;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::{Mutex, OnceLock};

use crate::db::critical_alert::alert_critical_failure;
use crate::db::{critical_recovery, state_dir};
use crate::DEV_MODE;

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

    /// Critical recovery is deliberately restricted to sustained poll failures.
    pub fn critical_poll_fail(message: String) -> ! {
        Self::write_durable(message.clone(), LogLevel::CriticalFail);
        match critical_recovery::recover_or_request_alert() {
            Ok(true) => alert_critical_failure(&message),
            Ok(false) => Self::info(
                "critical failure recorded; killed archiver-owned Chrome. A repeat before a successful poll will show an alert".to_string(),
            ),
            Err(e) => {
                eprintln!("[CriticalFail] recovery bookkeeping failed: {e:#}; showing alert to avoid a silent critical failure");
                alert_critical_failure(&message);
            }
        }
        process::exit(1);
    }

    /// TikTok explicitly denied the browser session. This needs immediate
    /// user attention, rather than waiting for a second critical poll failure.
    pub fn critical_tiktok_access_denied(message: String) -> ! {
        Self::write_durable(message.clone(), LogLevel::CriticalFail);
        if let Err(e) = critical_recovery::recover_or_request_alert() {
            eprintln!("[CriticalFail] Chrome recovery failed after TikTok access denial: {e:#}");
        }
        alert_critical_failure(&message);
        Self::info("TikTok access denied: polling is suspended. Re-login or complete verification, then restart the archiver.".to_string());
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60 * 60));
        }
    }

    // Diagnostic events are always persisted; stderr remains quiet outside dev mode.
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
