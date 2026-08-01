use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::db::{atomic_write_text, critical_recovery, ensure_file, logger::Log, state_dir};

const SUSTAINED_FAILURE_COUNT: u64 = 5;

#[derive(Debug, Serialize, Deserialize, Default)]
struct PollHealth {
    #[serde(default)]
    streak_started_at: Option<DateTime<Local>>,
    #[serde(default)]
    consecutive_failures: u64,
    #[serde(default)]
    last_error: Option<String>,
}

fn health_path() -> PathBuf {
    state_dir().join("poll_health.json")
}

fn load() -> PollHealth {
    let path = health_path();
    if let Ok(file) = fs::File::open(&path) {
        if let Ok(state) = serde_json::from_reader(file) {
            return state;
        }
    }
    PollHealth::default()
}

fn save(state: &PollHealth) -> Result<()> {
    let path = health_path();
    ensure_file(&path, "{}")?;
    let body = serde_json::to_string_pretty(state)?;
    atomic_write_text(&path, &body)
}

pub fn record_poll_success() {
    let _ = save(&PollHealth::default());
    critical_recovery::record_poll_success();
}

fn record_poll_failure(error: &str) -> Option<String> {
    let mut state = load();
    if state.streak_started_at.is_none() {
        state.streak_started_at = Some(Local::now());
    }
    state.consecutive_failures += 1;
    state.last_error = Some(error.chars().take(2000).collect());
    let count = state.consecutive_failures;
    let _ = save(&state);

    if count < SUSTAINED_FAILURE_COUNT {
        return None;
    }

    Some(format!(
        "Every poll cycle has failed {} times in a row (threshold is {}). Last error:\n{}\n\nSee state/error.log — fix the issue and restart.",
        count,
        SUSTAINED_FAILURE_COUNT,
        error
    ))
}

pub fn maybe_critical_fail_on_poll_error(error: &str) {
    if let Some(message) = record_poll_failure(error) {
        Log::critical_poll_fail(message);
    }
}
