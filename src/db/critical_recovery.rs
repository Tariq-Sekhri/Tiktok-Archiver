use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::db::{atomic_write_text, ensure_file, state_dir};

#[derive(Debug, Default, Serialize, Deserialize)]
struct CriticalRecoveryState {
    #[serde(default)]
    recovery_pending: bool,
}

fn state_path() -> PathBuf {
    state_dir().join("critical_recovery.json")
}

fn save(state: &CriticalRecoveryState) -> Result<()> {
    let path = state_path();
    ensure_file(&path, "{}")?;
    atomic_write_text(&path, &serde_json::to_string_pretty(state)?)
}

pub fn kill_archiver_chrome() -> Result<usize> {
    kill_archiver_owned_chrome()
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
    let mut output_cmd = Command::new("powershell.exe");
    output_cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script]);
    output_cmd.env("TTA_ARCHIVER_PROFILE", profile.as_os_str());
    output_cmd.creation_flags(0x08000000);
    let output = output_cmd
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
        let mut kill_cmd = Command::new("taskkill");
        kill_cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        kill_cmd.creation_flags(0x08000000);
        let result = kill_cmd
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
