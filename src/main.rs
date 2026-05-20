mod api;
mod db;
mod discover;
mod download;

use crate::api::videos_from_anchor_links;
use crate::db::account::{load_tracked_accounts, update_account_state, Account, CountEvent};
use crate::db::browser::{launch_browser, scroll_x_times, BrowserSession};
use crate::db::check_state;
use crate::db::config::load_config;
use crate::db::video::{append_videos, load_all, save_all, total_videos, DownloadStatus, Video};
use crate::discover::{fetch_newest_videos, login};
use crate::db::video::update_download_status;
use crate::download::{download_pending, link_fav_video, video_on_disk, VIDEO_EXT};
use anyhow::Context;
use api::get_new_count;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::{env, fs, io, io::Write, process};
use tokio::time::{sleep, Duration};
use crate::db::logger::Log;

#[derive(Debug)]
pub enum RunMode {
    Login,
    Default,
    Dev,
}

fn print_usage_and_exit() -> ! {
    eprintln!("  no args = default mode");
    eprintln!(
        "  login   = explicitly run login flow (for switching accounts or refreshing cookies)"
    );
    eprintln!("  dev     = default mode with visible browser windows for debugging");
    process::exit(1);
}

fn parse_args() -> RunMode {
    let args: Vec<String> = env::args().collect();
    if let Some(arg) = args.get(1) {
        match arg.as_str() {
            "login" => RunMode::Login,
            "dev" => RunMode::Dev,
            _ => print_usage_and_exit(),
        }
    } else {
        RunMode::Default
    }
}

fn print_how_to_use_and_exit(reason: &str) -> ! {
    Log::critical_fail(reason.to_string()); 
    eprintln!("\n[State Check] {}\n", reason);
    eprintln!("How to use, in order:");
    eprintln!("  1) run");
    eprintln!("     - On first run, this will prompt you to log in and save cookies into `state/saved_cookies.json`");
    eprintln!("  2) update config.yaml");
    eprintln!("     - Choose which accounts you want to track and optionally change download_dir.");
    eprintln!("  3) run");
    eprintln!(
        "     - Default mode: poll for new videos + download pending using your saved login."
    );
    eprintln!("  4) cargo run dev");
    eprintln!("     - Debug mode: run default loop but show browser windows.");
    eprintln!("  5) cargo run login");
    eprintln!("     - Explicitly run the login flow to switch accounts or refresh cookies.");
    process::exit(1);
}

async fn timeout(wait_secs: u8) {
    if !io::stdout().is_terminal() {
        sleep(Duration::from_secs(wait_secs as u64)).await;
        return;
    }

    for remaining in (1..=wait_secs).rev() {
        print!("\rwaiting {}s  ", remaining);
        let _ = io::stdout().flush();
        sleep(Duration::from_secs(1)).await;
    }
    print!("\rdone.        \n");
}
async fn default_loop() {
    loop {
        let accounts = match load_tracked_accounts() {
            Ok(accounts) => accounts,

            Err(e) => {
                Log::error(format!("Failed to load accounts: {}", e));
                timeout(5u8).await;
                continue;
            }
        };
        println!("{:?}", accounts);
        for account in accounts {
            let new_count = match get_new_count(&account.name).await {
                Ok(n) => n,
                Err(e) => {
                    Log::error(format!("error getting count{e}"));
                    continue;
                }
            };
            let seen_map = match load_all() {
                Ok(m) => m,
                Err(e) => {
                    let msg = format!("{}: load_all_seen_videos failed: {}", account.name, e);
                    Log::critical_fail(msg);
                    unreachable!()
                }
            };

            let existing_videos: Vec<Video> = match seen_map.get(&account.name) {
                Some(v) => v.clone(),
                None => {
                    Log::error(format!(
                        "{}: no entry in seen_videos, using empty list",
                        account.name
                    ));
                    Vec::new()
                }
            };

            let existing_ids: std::collections::HashSet<i64> =
                existing_videos.iter().map(|v| v.video_id).collect();

            let (unavailable, new_videos): (i64, Vec<Video>) =
                match CountEvent::observe(account.count, new_count) {
                    CountEvent::Same => {
                        println!("{}: Same", account.name);
                        (account.unavailable, Vec::new())
                    }
                    CountEvent::Increased => {
                        let fetched_videos = match fetch_newest_videos(&account).await {
                            Ok(v) => v,
                            Err(e) => {
                                Log::error(format!("{}: fetch_newest_videos failed: {}", account.name, e));
                                continue;
                            }
                        };
                        let new_v: Vec<Video> = fetched_videos
                            .into_iter()
                            .filter(|v| !existing_ids.contains(&v.video_id))
                            .collect();
                        (account.unavailable, new_v)
                    }
                    CountEvent::Decreased => {
                        let unavailable = account.unavailable + (account.count - new_count);
                        println!(
                            "[main] {}: count decreased, unavailable incremented by {} -> {}",
                            account.name,
                            account.count - new_count,
                            unavailable
                        );
                        (unavailable, Vec::new())
                    }
                };

            if !new_videos.is_empty() {
                if let Err(e) = append_videos(&account.name, &new_videos) {
                    let msg = format!("{}: append_seen_videos failed: {}", account.name, e);
                    Log::critical_fail(msg);
                    continue;
                }
            }

            reconcile_account_state(&account, new_count, unavailable);

            if let Err(e) = download_pending() {
                let msg = format!("Error downloading for {}: {}", account.name, e);
                Log::error(msg);
            }
            sleep(Duration::from_secs(1)).await;
        }
        if let Ok(config) = load_config() {
            println!("fav: {}", config.download_fav );
            if config.download_fav {

                if let Err(e) = fav().await{
                    Log::error(format!("Fav Error:{}", e));
                };

            }
        } else {
            Log::error("Config Failed to load".to_string());
        }
        timeout(60).await;
    }
}

fn reconcile_account_state(account: &Account, new_count: i64, unavailable: i64) {
    let totals = match total_videos() {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("{}: total_seen_videos failed: {}", account.name, e);
            Log::critical_fail(msg);
            unreachable!()
        }
    };

    let total_seen_videos_count = *totals.get(&account.name).unwrap_or(&0) as i64;

    let diff = new_count + unavailable - total_seen_videos_count;

    if diff < 0 {
        let msg = format!(
            "{}: diff became negative (count_now={}, unavailable={}, total_seen={})",
            account.name, new_count, unavailable, total_seen_videos_count
        );
        Log::critical_fail(msg);
    }

    let invariant_lhs = new_count + unavailable - diff;

    if invariant_lhs != total_seen_videos_count {
        let msg = format!(
            "{}: invariant violated (lhs={}, rhs={})",
            account.name, invariant_lhs, total_seen_videos_count
        );
        Log::error(msg);
    }

    if let Err(e) = update_account_state(account, new_count, diff, unavailable) {
        let msg = format!("Error updating state for @{}: {}", account.name, e);
        Log::critical_fail(msg);
    }
}
async fn open_profile() -> BrowserSession {
    println!("[fav] launching browser...");
    let session = launch_browser("https://www.tiktok.com", false).unwrap();

    println!("[fav] opening profile...");
    timeout(3).await;
    session
        .tab
        .wait_for_element(r#"[data-e2e="nav-profile"]"#)
        .expect("didnw wait")
        .click()
        .expect("counlt click");
    timeout(3).await;
    println!("[fav] opening favorites tab...");
    session
        .tab
        .wait_for_xpath(r#"//span[text()="Favorites"]/ancestor::p[@role="tab"]"#)
        .unwrap()
        .click()
        .unwrap();
    timeout(5).await;
    println!("[fav] favorites page ready");
    session
}

async fn fav()->anyhow::Result<()>{
    let session = open_profile().await;
    let mut pass = 0u32;
    let mut done_ids: HashSet<i64> = HashSet::new();
    let download_dir = load_config()?.download_dir;
    loop {
        pass += 1;
        println!("[fav] pass {}: reading page...", pass);
        let html = session.tab.get_content().context("get_content")?;
        let fav_vids: Vec<Video> = videos_from_anchor_links(&html)?
            .into_iter()
            .filter(|vid| !done_ids.contains(&vid.video_id))
            .collect();
        println!("[fav] pass {}: found {} videos on page", pass, fav_vids.len());
        let mut seen_vids = load_all()?;
        let favorite_videos = seen_vids.entry("favorite".to_string()).or_default();
        let existing_fav_ids: HashSet<i64> = favorite_videos.iter().map(|v| v.video_id).collect();
        let mut new_count = 0;
        let mut mark_downloaded: Vec<i64> = Vec::new();
        for fav in &fav_vids {
            done_ids.insert(fav.video_id);

            if existing_fav_ids.contains(&fav.video_id) {
                continue;
            }

            new_count += 1;
            let mut fav_video = fav.clone();
            fav_video.is_fav = true;

            if video_on_disk(&fav.username, fav.video_id)? {
                let fav_path = format!("{}/favs/{}.{}", download_dir, fav.video_id, VIDEO_EXT);
                if !fs::exists(&fav_path)? {
                    println!(
                        "[fav] hard_link @{} id={}",
                        fav.username, fav.video_id
                    );
                    link_fav_video(fav)?;
                }
                mark_downloaded.push(fav.video_id);
            } else {
                println!(
                    "[fav] new favorite @{} id={}",
                    fav.username, fav.video_id
                );
            }

            favorite_videos.push(fav_video);
        }
        save_all(&seen_vids)?;
        for video_id in mark_downloaded {
            update_download_status("favorite", video_id, DownloadStatus::Downloaded)?;
        }
        println!(
            "[fav] pass {}: saved seen db, new_or_updated={}",
            pass, new_count
        );
        if new_count >= 1 {
            println!("[fav] pass {}: scrolling for more...", pass);
            scroll_x_times(1+pass*pass*pass, &session)?;
        } else {
            println!("[fav] pass {}: no new items, done", pass);
            println!("[fav] finished after {} passes", pass);
            return Ok(());
        }
    }
}

#[tokio::main]
async fn main() {

    let mode = parse_args();
    println!("Run Mode:{:?}", mode);
    match mode {
        RunMode::Default | RunMode::Dev | RunMode::Login => {
            env::set_var("TTA_SHOW_BROWSER", "1");
        }
    }
    check_state(&mode).await;
    match mode {
        RunMode::Login => login().await.unwrap_or_else(|e| {
            let msg = format!("Error logging in: {}", e);
            Log::critical_fail(msg.clone());
        }),
        RunMode::Default | RunMode::Dev => default_loop().await,
    }
}
