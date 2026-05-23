//v0
pub mod account;
pub mod logger;
pub mod config;
pub mod critical_alert;
pub mod video;

use std::{fs, path::{Path, PathBuf}};
use std::collections::HashSet;
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::OnceLock;
use crate::{print_how_to_use_and_exit, RunMode};
use crate::browser::{cookies_have_any, is_headless, launch_browser, log_auth_storage_status, TIKTOK_ORIGIN};
use crate::db::config::{account_name, is_tracked, load_config, save_config, Config};
use crate::db::account::{account_file, add_account, load_accounts, update_account_state};
use crate::db::logger::Log;
use anyhow::Result;
use anyhow::anyhow;
use crate::discover::{first_discovery, login};
use tokio::io::AsyncWriteExt;
use crate::db::video::{append_videos, load_all, save_all, videos_file};

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
//v0
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

//v0
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

//v0
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

//v0
pub async fn check_state(mode: &RunMode) {
    let (cookies_path, mut config) = general_check();
    Log::dev("init ok".to_string());

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
            config_and_accounts_sync(&mut config).await;
        }
    }
}
//v0
async fn config_and_accounts_sync(config: &mut Config) {
    let accounts = match load_accounts() {
        Ok(a) => a,
        Err(e) => {
            print_how_to_use_and_exit(&format!("Failed to load accounts.json: {}", e));
        }
    };

    let mut config_all_names: HashSet<String> = HashSet::new();
    let mut config_tracked_names: HashSet<String> = HashSet::new();


    for account in &config.accounts {
        let name = account_name(account).to_string();
        config_all_names.insert(name.clone());
        if is_tracked(account) {
            config_tracked_names.insert(name);
        }
    }
    let state_names: HashSet<String> = accounts.iter().map(|a| a.name.clone()).collect();
    if config_all_names != state_names {
        Log::dev(format!(
            "[sync] starting reconciliation: config_all_names={:?}, state_names={:?}",
            config_all_names, state_names
        ));

        let config_only_tracked: Vec<String> = config_tracked_names
            .iter()
            .filter(|name| !state_names.contains(*name))
            .cloned()
            .collect();


        let state_only: Vec<String> = state_names
            .iter()
            .filter(|name| !config_all_names.contains(*name))
            .cloned()
            .collect();

        let ran_discovery = !config_only_tracked.is_empty();

        Log::dev(format!(
            "[sync] Pre-Reconciling accounts: config_all_names={:?}, state_names={:?}, config_only_tracked={:?}, state_only={:?}",
            config_all_names, state_names, config_only_tracked, state_only
        ));
        let session = launch_browser(TIKTOK_ORIGIN, is_headless()).unwrap();
        for name in config_only_tracked {
            Log::console(format!("sync {}", name));
            Log::dev(format!("[sync] first_discovery start for @{}", name));
            match first_discovery(name.clone(), &session).await {
                Ok((acc,vids))=>{
                    Log::dev(format!(
                        "[sync] first_discovery success for @{}: count={}, diff={}, unavailable={}, vids={}",
                        acc.name,
                        acc.count,
                        acc.diff,
                        acc.unavailable,
                        vids.len()
                    ));
                    let mut seen_vids = load_all().unwrap();
                     append_videos(&mut seen_vids,&acc.name.to_string(), &vids);
                    save_all(&seen_vids).unwrap();
                    if let Err(e) = add_account(&acc) {
                        if e.to_string().contains("account already exists") {
                            Log::dev(format!(
                                "[sync] account @{} already exists, applying first_discovery state",
                                acc.name
                            ));
                            if let Err(update_err) =
                                update_account_state(&acc, acc.count, acc.diff, acc.unavailable)
                            {
                                print_how_to_use_and_exit(&format!(
                                    "Failed to update existing account @{}: {}",
                                    acc.name, update_err
                                ));
                            }
                        } else {
                            print_how_to_use_and_exit(&format!("Failed to add account: {}", e));
                        }
                    } else {
                        Log::dev(format!("[sync] added new account @{}", acc.name));
                    }
                    Log::dev(format!("[sync] added account: {:?}", acc));
                }
                Err(e)=>{print_how_to_use_and_exit(&format!("First discovery failed for @{}: {}", name, e)); }
            }
            Log::dev(format!("[sync] first_discovery done for @{}", name));
        }

        let mut config_updated = false;
        for name in state_only {
            if !config_all_names.contains(&name) {
                config.accounts.push(format!("{}:false", name));
                config_updated = true;
            }
        }

        if config_updated {
            if let Err(e) = save_config(config) {
                print_how_to_use_and_exit(&format!("Failed to save config.yaml during reconciliation: {}", e));
            }
            Log::dev("[sync] reconciliation updated config.yaml".to_string());
        }

        Log::dev("[sync] reconciliation finished".to_string());
        if ran_discovery || config_updated {
            Log::console("sync ok".to_string());
        }
    }
}


//v0
fn general_check() -> (PathBuf, Config) {
    let state_dir = state_dir();

    if let Err(e) = videos_file() {
        print_how_to_use_and_exit(&format!("Failed to init seen_videos.json: {}", e));
    }
    if let Err(e) = account_file() {
        print_how_to_use_and_exit(&format!("Failed to init accounts.json: {}", e));
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