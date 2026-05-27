//v0
pub mod logger;
pub mod config;
pub mod critical_alert;
pub mod video;

use std::{fs, path::{Path, PathBuf}, process::Command, sync::OnceLock, };
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use anyhow::{anyhow, Result};
use tokio::io::AsyncWriteExt;
use crate::{browser::{cookies_have_any, log_auth_storage_status}, db::{config::{load_config, Config}, logger::Log, video::videos_file, }, discover::login, print_how_to_use_and_exit, RunMode, };

static YT_DLP_READY: OnceLock<()> = OnceLock::new();
//v1
fn ensure_state_dir(state_dir: &Path) {
    if state_dir.exists() {
        return;
    }
    if let Err(e) = fs::create_dir_all(state_dir) {
        print_how_to_use_and_exit(&format!(
            "Failed to create state directory {}: {}",
            state_dir.display(),
            e
        ));
    }
}
//v1
pub fn state_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        let manifest_state = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("state");
        ensure_state_dir(&manifest_state);
        return manifest_state;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let exe_state = exe.parent().unwrap_or_else(|| Path::new(".")).join("state");
    ensure_state_dir(&exe_state);
    exe_state
}

//v1
pub fn ensure_file(path: &PathBuf, default_contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        if fs::metadata(path)?.len() == 0 {
            atomic_write_text(path, default_contents)?;
        }
        return Ok(());
    }
    atomic_write_text(path, default_contents)?;
    Ok(())
}

//v1
pub fn atomic_write_text(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        std::io::Write::write_all(&mut file, contents.as_bytes())?;
        std::io::Write::flush(&mut file)?;
        file.sync_all()?;
    }
    match fs::rename(&tmp_path, path) {
        Ok(_) => {}
        Err(_) => {
            if path.exists() {
                fs::remove_file(path)?;
            }
            fs::rename(&tmp_path, path)?;
        }
    }
    Ok(())
}

//v1
pub async fn check_state(mode: &RunMode) {
    let (cookies_path, _) = general_check();
    match mode {
        RunMode::Login => {
            log_auth_storage_status();
        }
        RunMode::Default | RunMode::Dev => {
            log_auth_storage_status();
            if !cookies_have_any(&cookies_path) {
                println!("No TikTok login detected, starting login flow.");
                if let Err(e) = login().await {
                    print_how_to_use_and_exit(&format!("Login failed: {}", e));
                }
                if !cookies_have_any(&cookies_path) {
                    print_how_to_use_and_exit("Login completed but no cookies were saved. Please try again.");
                }
            }
            if let Err(e) = ensure_yt_dlp().await {
                print_how_to_use_and_exit(&format!("yt-dlp check/install failed: {}", e));
            }
        }
    }
}

//v0
fn general_check() -> (PathBuf, Config) {
    let state_dir = state_dir();

    if let Err(e) = videos_file() {
        print_how_to_use_and_exit(&format!("Failed to init seen_videos.json: {}", e));
    }
    let cookies_path = state_dir.join("saved_cookies.json");
    if let Err(e) = ensure_file(&cookies_path, "{\n  \"cookies\": []\n}\n") {
        print_how_to_use_and_exit(&format!("Failed to init saved_cookies.json: {}", e));
    }
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            print_how_to_use_and_exit(&format!("Failed to load config.yaml: {}", e));
        }
    };


    if config.accounts.iter().all(|a| a.trim().is_empty()) {
        print_how_to_use_and_exit("No accounts configured in config.yaml. Add at least one username under `accounts:`.");
    }
    (cookies_path, config)
}

//v1
pub fn resolve_executable_path(default_name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("state").join(default_name);
            if candidate.exists() {
                return candidate;
            }
            let candidate = dir.join(default_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    if cfg!(debug_assertions) {
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let candidate = PathBuf::from(manifest).join("state").join(default_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    PathBuf::from(default_name)
}
//v1
async fn download_yt_dlp(dest: &PathBuf) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = dest.with_extension("download");
    let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

    let resp = reqwest::get(url).await?;
    if !resp.status().is_success() {
        return Err(anyhow!(format!("download failed: http {}", resp.status())));
    }

    let bytes = resp.bytes().await?;
    let mut file = tokio::fs::File::create(&tmp).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    drop(file);

    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

//v1
async fn ensure_yt_dlp() -> Result<()> {
    if YT_DLP_READY.get().is_some() {
        return Ok(());
    }

    let ytdlp_path = resolve_executable_path("yt-dlp.exe");
    let ready = is_ytdlp_runnable(&ytdlp_path);

    if !ready {
        Log::dev("yt-dlp get".to_string());
        let target = state_dir().join("yt-dlp.exe");
        download_yt_dlp(&target).await?;
        if !is_ytdlp_runnable(&target) {
            return Err(anyhow!(format!("yt-dlp downloaded but not runnable: {}", target.display())));
        }
    }

    let _ = YT_DLP_READY.set(());
    Ok(())
}
//v1
fn is_ytdlp_runnable(path: &PathBuf) -> bool   {
    let mut check_cmd = Command::new(path);
    check_cmd.arg("--version");
    //suppress cmd showing
    #[cfg(windows)]
    check_cmd.creation_flags(0x08000000);
    match check_cmd.output() {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}