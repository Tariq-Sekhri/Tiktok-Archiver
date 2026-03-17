pub mod seen_video;
pub mod account;
pub mod browser;
pub mod logger;
pub mod config;

use std::{fs, path::{PathBuf, Path}};
use std::collections::{HashMap, HashSet};
use std::process::Command;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::OnceLock;
use crate::{print_how_to_use_and_exit, RunMode};
use crate::db::browser::cookies_have_any;
use crate::db::seen_video::{append_seen_videos, save_all_seen_videos, seen_videos_file};
use crate::db::config::{load_config, save_config, account_name, is_tracked, Config};
use crate::db::account::{account_file, add_account, load_accounts};
use crate::db::logger::{log, Event, LogLevel};
use anyhow::Result;
use anyhow::anyhow;
use crate::discover::first_discovery;
use tokio::io::AsyncWriteExt;

static YT_DLP_READY: OnceLock<()> = OnceLock::new();

pub fn state_dir() -> PathBuf {
    let base_dir = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    } else {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        exe.parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
    };
    let state_dir = base_dir.join("state");
    if !state_dir.exists() {
        if let Err(e) = fs::create_dir_all(&state_dir) {
            print_how_to_use_and_exit(&format!(
                "Failed to create state directory {}: {}",
                state_dir.display(),
                e
            ));
        }
    }
    state_dir
}

pub fn ensure_file(path: &PathBuf, default_contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        if fs::metadata(path)?.len() == 0 {
            fs::write(path, default_contents)?;
        }
        return Ok(());
    }
    fs::write(path, default_contents)?;
    Ok(())
}


pub async fn check_state(mode: &RunMode) {
    let (cookies_path, mut config) = general_check();

    match mode {
        RunMode::Login => {}
        RunMode::Default => {
            if !cookies_path.exists() {
                print_how_to_use_and_exit("Missing `state/saved_cookies.json` (cookies not saved yet).");
            }

            if !cookies_have_any(&cookies_path) {
                print_how_to_use_and_exit("`state/saved_cookies.json` has no cookies (or invalid JSON). Run `cargo run login` first.");
            }
            if let Err(e) = ensure_yt_dlp().await {
                print_how_to_use_and_exit(&format!("yt-dlp check/install failed: {}", e));
            }
            config_and_accounts_sync(&mut config).await;

        }
    }
}

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
        println!(
            "[sync] starting reconciliation: config_all_names={:?}, state_names={:?}",
            config_all_names, state_names
        );

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


        let msg = format!(
            "Pre-Reconciling accounts: config_all_names={:?}, state_names={:?}, config_only_tracked={:?}, state_only={:?}",
            config_all_names, state_names, config_only_tracked, state_only
        );
        log(Event::new(msg, LogLevel::Info));

        for name in config_only_tracked {
            println!("[sync] first_discovery start for @{}", name);
            match first_discovery(name.clone()).await {
                Ok((acc,vids))=>{
                    if append_seen_videos(&acc.name.to_string(), &vids).is_err() {
                        println!("Error Appending");
                        if let Err(e) = save_all_seen_videos(&HashMap::from([(acc.name.clone(), vids.clone())])) {
                            print_how_to_use_and_exit(&format!("Failed to save seen videos: {}", e));
                        }
                    };
                    if let Err(e) = add_account(&acc) {
                        print_how_to_use_and_exit(&format!("Failed to add account: {}", e));
                    }
                    println!("Added Account: {:?}", acc);
                }
                Err(e)=>{print_how_to_use_and_exit(&format!("First discovery failed for @{}: {}", name, e)); }
            }
            println!("[sync] first_discovery done for @{}", name);
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
            println!("[sync] reconciliation updated config.yaml");
        }

        println!("[sync] reconciliation finished");
    }
}

fn general_check() -> (PathBuf, Config) {
    let state_dir = state_dir();

    if let Err(e) = seen_videos_file() {
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

fn resolve_executable_path(default_name: &str) -> PathBuf {
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

    PathBuf::from(default_name)
}

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

async fn ensure_yt_dlp() -> Result<()> {
    if YT_DLP_READY.get().is_some() {
        return Ok(());
    }

    let ytdlp_path = resolve_executable_path("yt-dlp.exe");
    let ready = is_ytdlp_runnable(&ytdlp_path);

    if !ready {
        let target = state_dir().join("yt-dlp.exe");
        download_yt_dlp(&target).await?;
        if !is_ytdlp_runnable(&target) {
            return Err(anyhow!(format!("yt-dlp downloaded but not runnable: {}", target.display())));
        }
    }

    let _ = YT_DLP_READY.set(());
    Ok(())
}

fn is_ytdlp_runnable(path: &PathBuf) -> bool {
    let mut check_cmd = Command::new(path);
    check_cmd.arg("--version");
    #[cfg(windows)]
    check_cmd.creation_flags(0x08000000);
    match check_cmd.output() {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}