use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::db::{atomic_write_text, ensure_file, state_dir};

#[derive(Debug, Default, Serialize, Deserialize)]
struct CriticalRecoveryState {
    #[serde(default)]
    recovery_pending: bool,
}

fn state_path() -> PathBuf {
    state_dir().join("critical_recovery.json")
}

fn load() -> CriticalRecoveryState {
    fs::File::open(state_path())
        .ok()
        .and_then(|file| serde_json::from_reader(file).ok())
        .unwrap_or_default()
}

fn save(state: &CriticalRecoveryState) -> Result<()> {
    let path = state_path();
    ensure_file(&path, "{}")?;
    atomic_write_text(&path, &serde_json::to_string_pretty(state)?)
}

/// Returns true only when another critical failure occurs before a successful poll.
pub fn recover_or_request_alert() -> Result<bool> {
    if load().recovery_pending {
        return Ok(true);
    }
    save(&CriticalRecoveryState {
        recovery_pending: true,
    })?;
    let killed = kill_archiver_owned_chrome()?;
    eprintln!("[CriticalRecovery] first critical failure: terminated {killed} Chrome process(es) using the archiver profile");
    Ok(false)
}

pub fn record_poll_success() {
    let path = state_path();
    if path.exists() {
        if let Err(e) = save(&CriticalRecoveryState::default()) {
            eprintln!(
                "[CriticalRecovery] failed to clear recovery state after a successful poll: {e:#}"
            );
        }
    }
}

#[cfg(windows)]
fn kill_archiver_owned_chrome() -> Result<usize> {
    let profile = state_dir().join("tiktok_profile");
    let script = "$profile=$env:TTA_ARCHIVER_PROFILE; Get-CimInstance Win32_Process | Where-Object { $_.Name -ieq 'chrome.exe' -and $_.CommandLine -and $_.CommandLine.Contains($profile) } | Select-Object -ExpandProperty ProcessId";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("TTA_ARCHIVER_PROFILE", profile.as_os_str())
        .output()
        .context("failed to enumerate Chrome processes for critical recovery")?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to enumerate Chrome processes for critical recovery: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    for pid in &pids {
        let result = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .with_context(|| format!("failed to terminate archiver-owned Chrome pid {pid}"))?;
        if !result.status.success() {
            eprintln!(
                "[CriticalRecovery] Chrome pid {pid} could not be terminated: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }
    }
    Ok(pids.len())
}

#[cfg(not(windows))]
fn kill_archiver_owned_chrome() -> Result<usize> {
    Ok(0)
}
