mod api;
mod db;
mod discover;
mod download;

use crate::api::videos_from_anchor_links;
use crate::api::{fetch_counts_on_session, finish_poll_session, open_poll_session};
use crate::db::account::{load_tracked_accounts, update_account_state, Account, CountEvent};
use crate::db::browser::{open_favorites_on_session, scroll_x_times, BrowserSession};
use crate::db::check_state;
use crate::db::config::load_config;
use crate::db::logger::{dev_mode_enabled, Log, LogLevel};
use crate::db::video::{
    append_videos_in_memory, bucket_count, load_all, save_all, seen_videos_is_compact,
    update_download_status_in_memory,
    DownloadStatus, Video,
};
use crate::discover::{fetch_newest_videos_sync, login};
use crate::download::{download_pending_favorites_with_map, link_fav_video, video_on_disk, VIDEO_EXT};
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::time::Instant;
use std::{env, fs, io, io::Write, process};
use tokio::time::{sleep, Duration};

const FAVORITE_BUCKET: &str = "favorite";

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
    eprintln!("  dev     = default mode with visible browser and verbose console tracing");
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

async fn timeout(wait_secs: u8, level: LogLevel) {
    if !io::stdout().is_terminal() || level == LogLevel::Dev {
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

fn reconcile_account_state(account: &Account, new_count: i64, unavailable: i64, total_seen: usize) {
    let total_seen_videos_count = total_seen as i64;
    Log::dev(format!(
        "@{} reconcile enter: new_count={} unavailable_in={} stored_count={} stored_diff={} stored_unavailable={}",
        account.name,
        new_count,
        unavailable,
        account.count,
        account.diff,
        account.unavailable
    ));
    let diff = new_count + unavailable - total_seen_videos_count;
    Log::dev(format!(
        "@{} reconcile: total_seen={} diff={} (formula: {} + {} - {} = {})",
        account.name,
        total_seen_videos_count,
        diff,
        new_count,
        unavailable,
        total_seen_videos_count,
        diff
    ));

    if diff < 0 {
        let msg = format!(
            "{}: diff became negative (count_now={}, unavailable={}, total_seen={})",
            account.name, new_count, unavailable, total_seen_videos_count
        );
        Log::dev(format!("@{} reconcile CRITICAL: {}", account.name, msg));
        Log::critical_fail(msg);
    }

    let invariant_lhs = new_count + unavailable - diff;

    if invariant_lhs != total_seen_videos_count {
        let msg = format!(
            "{}: invariant violated (lhs={}, rhs={})",
            account.name, invariant_lhs, total_seen_videos_count
        );
        Log::dev(format!(
            "@{} reconcile invariant mismatch: {}",
            account.name, msg
        ));
        Log::error(msg);
    }

    Log::dev(format!(
        "@{} reconcile saving accounts.json: count={} diff={} unavailable={}",
        account.name, new_count, diff, unavailable
    ));
    if let Err(e) = update_account_state(account, new_count, diff, unavailable) {
        let msg = format!("Error updating state for @{}: {}", account.name, e);
        Log::critical_fail(msg);
    }
}

fn fav_scroll_budget(pass: u32) -> u32 {
    const CAP: u32 = 800;
    match pass {
        1 => 3,
        2 => 12,
        p => {
            let exp = (p - 3) as i32;
            let v = 30.0 * 3.0_f64.powi(exp);
            (v.round() as u32).clamp(30, CAP)
        }
    }
}

fn fav_with_seen(
    session: &BrowserSession,
    seen: &mut HashMap<String, Vec<Video>>,
    download_dir: &str,
) -> anyhow::Result<bool> {
    let fav_cycle_t0 = Instant::now();
    let mut seen_dirty = false;
    let mut done_ids: HashSet<i64> = HashSet::new();

    let mut existing_fav_ids: HashSet<i64> = seen
        .get(FAVORITE_BUCKET)
        .map(|v| v.iter().map(|x| x.video_id).collect())
        .unwrap_or_default();

    let mut pass = 0u32;
    loop {
        pass += 1;

        Log::console(format!("fav pass {}", pass));
        Log::dev(format!("[fav] pass {}: reading page", pass));
        let read_t0 = Instant::now();
        let html = session.tab().get_content().context("get_content")?;
        let fav_vids: Vec<Video> = videos_from_anchor_links(&html)?
            .into_iter()
            .filter(|vid| !done_ids.contains(&vid.video_id))
            .collect();
        Log::dev_timing("fav_pass_read", read_t0);
        Log::console(format!("fav pass {} links {}", pass, fav_vids.len()));
        Log::dev(format!(
            "[fav] pass {}: found {} videos on page",
            pass,
            fav_vids.len()
        ));

        let process_t0 = Instant::now();
        let mut new_count = 0u32;
        let mut batch_new: Vec<Video> = Vec::new();
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
                    Log::dev(format!(
                        "[fav] hard_link @{} id={}",
                        fav.username, fav.video_id
                    ));
                    link_fav_video(fav)?;
                }
                mark_downloaded.push(fav.video_id);
            } else {
                Log::dev(format!(
                    "[fav] new favorite @{} id={}",
                    fav.username, fav.video_id
                ));
            }

            batch_new.push(fav_video);
            existing_fav_ids.insert(fav.video_id);
        }

        if !batch_new.is_empty() {
            let favorite_videos = seen.entry(FAVORITE_BUCKET.to_string()).or_default();
            for v in batch_new {
                favorite_videos.push(v);
            }
            seen_dirty = true;
        }
        for video_id in mark_downloaded {
            if update_download_status_in_memory(
                seen,
                FAVORITE_BUCKET,
                video_id,
                DownloadStatus::Downloaded,
            ) {
                seen_dirty = true;
            }
        }
        Log::dev_timing("fav_pass_process", process_t0);

        if new_count > 0 {
            Log::console(format!("fav pass {} new {}", pass, new_count));
        }
        Log::dev(format!(
            "[fav] pass {}: processed new={}",
            pass, new_count
        ));

        if new_count == 0 {
            Log::console(format!("fav done {}", pass));
            Log::dev(format!("[fav] pass {}: no new items, done", pass));
            break;
        }

        let budget = fav_scroll_budget(pass);
        Log::console(format!("fav pass {} scroll {}", pass, budget));
        Log::dev(format!("[fav] pass {}: scroll_x_times {}", pass, budget));
        let scroll_t0 = Instant::now();
        scroll_x_times(budget, session)?;
        Log::dev_timing("fav_pass_scroll", scroll_t0);
    }

    Log::dev(format!("[fav] finished after {} passes", pass));
    Log::dev_timing("fav_cycle", fav_cycle_t0);
    Ok(seen_dirty)
}

fn run_poll_fav_cycle(
    accounts: Vec<Account>,
    names: Vec<String>,
    download_fav: bool,
    download_dir: String,
) -> anyhow::Result<()> {
    let load_t0 = Instant::now();
    let mut seen = load_all()?;
    let rewrite_format = if dev_mode_enabled() {
        seen_videos_is_compact().unwrap_or(false)
    } else {
        seen_videos_is_compact().map(|c| !c).unwrap_or(false)
    };
    if rewrite_format && dev_mode_enabled() {
        Log::dev("seen_videos.json is compact; will rewrite pretty on save".to_string());
    }
    Log::dev_timing("load_all", load_t0);

    let session = open_poll_session()?;
    let mut seen_dirty = false;

    let count_results = fetch_counts_on_session(&session, &names)?;
    for (account, count_result) in accounts.into_iter().zip(count_results) {
        Log::dev(format!("@{} polling tiktok video count", account.name));
        let new_count = match count_result.1 {
            Ok(n) => n,
            Err(e) => {
                let msg = format!("{}", e);
                Log::error(format!("{}: {}", account.name, msg));
                Log::dev(format!("@{} get_new_count failed: {}", account.name, msg));
                Log::console(format!("{}: fail", account.name));
                continue;
            }
        };
        Log::dev(format!(
            "@{} tiktok count={} (stored={})",
            account.name, new_count, account.count
        ));

        let existing_videos: Vec<Video> = match seen.get(&account.name) {
            Some(v) => v.clone(),
            None => {
                Log::error(format!(
                    "{}: no entry in seen_videos, using empty list",
                    account.name
                ));
                Vec::new()
            }
        };

        let existing_ids: HashSet<i64> = existing_videos.iter().map(|v| v.video_id).collect();
        Log::dev(format!(
            "@{} seen_videos entries={} unique_ids={}",
            account.name,
            existing_videos.len(),
            existing_ids.len()
        ));

        let (unavailable, new_videos): (i64, Vec<Video>) =
            match CountEvent::observe(account.count, new_count) {
                CountEvent::Same => {
                    Log::console(format!("{}: same", account.name));
                    (account.unavailable, Vec::new())
                }
                CountEvent::Increased => {
                    Log::dev(format!(
                        "@{} count increased {} -> {}",
                        account.name, account.count, new_count
                    ));
                    let fetched_videos = match fetch_newest_videos_sync(&account) {
                        Ok(v) => v,
                        Err(e) => {
                            Log::error(format!(
                                "{}: fetch_newest_videos failed: {}",
                                account.name, e
                            ));
                            Log::dev(format!(
                                "@{} fetch_newest_videos failed: {}",
                                account.name, e
                            ));
                            Log::console(format!("{}: fail", account.name));
                            continue;
                        }
                    };
                    Log::dev(format!(
                        "@{} page anchor links parsed: {}",
                        account.name,
                        fetched_videos.len()
                    ));
                    let new_v: Vec<Video> = fetched_videos
                        .into_iter()
                        .filter(|v| !existing_ids.contains(&v.video_id))
                        .collect();
                    Log::dev(format!(
                        "@{} new videos after id filter: {}",
                        account.name,
                        new_v.len()
                    ));
                    for v in &new_v {
                        Log::dev(format!("@{} new video id={}", account.name, v.video_id));
                    }
                    Log::console(format!("{}: increase", account.name));
                    (account.unavailable, new_v)
                }
                CountEvent::Decreased => {
                    let unavailable = account.unavailable + (account.count - new_count);
                    Log::dev(format!(
                        "@{} count decreased {} -> {}, unavailable {} -> {} (delta={})",
                        account.name,
                        account.count,
                        new_count,
                        account.unavailable,
                        unavailable,
                        account.count - new_count
                    ));
                    Log::console(format!("{}: decrease", account.name));
                    (unavailable, Vec::new())
                }
            };

        if !new_videos.is_empty() {
            Log::dev(format!(
                "@{} appending {} videos to seen_videos",
                account.name,
                new_videos.len()
            ));
            append_videos_in_memory(&mut seen, &account.name, &new_videos);
            seen_dirty = true;
        }

        let total_seen = bucket_count(&seen, &account.name);
        reconcile_account_state(&account, new_count, unavailable, total_seen);
    }

    if download_fav {
        Log::console("fav start".to_string());
        let fav_t0 = Instant::now();
        match open_favorites_on_session(&session) {
            Ok(()) => match fav_with_seen(&session, &mut seen, &download_dir) {
                Ok(fav_dirty) => {
                    if fav_dirty {
                        seen_dirty = true;
                    }
                }
                Err(e) => Log::error(format!("Fav Error: {}", e)),
            },
            Err(e) => Log::error(format!("Fav open Error: {}", e)),
        }
        Log::dev_timing("fav_total", fav_t0);

        let dl_t0 = Instant::now();
        download_pending_favorites_with_map(&mut seen)?;
        Log::dev_timing("download_pending_fav", dl_t0);
    }

    finish_poll_session(session);

    if seen_dirty || rewrite_format {
        let save_t0 = Instant::now();
        save_all(&seen)?;
        Log::dev_timing("seen_save", save_t0);
    }

    Ok(())
}

async fn default_loop() {
    loop {
        let cycle_start = Instant::now();
        Log::dev(format!("poll cycle start"));
        let accounts = match load_tracked_accounts() {
            Ok(accounts) => accounts,
            Err(e) => {
                Log::error(format!("Failed to load accounts: {}", e));
                timeout(5u8, LogLevel::Error).await;
                continue;
            }
        };
        Log::dev(format!("tracked accounts loaded: count={}", accounts.len()));
        for account in &accounts {
            Log::dev(format!(
                "@{} stored count={} diff={} unavailable={}",
                account.name, account.count, account.diff, account.unavailable
            ));
        }

        let config = match load_config() {
            Ok(c) => c,
            Err(e) => {
                Log::error(format!("Config Failed to load: {}", e));
                timeout(5u8, LogLevel::Error).await;
                continue;
            }
        };

        let names: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
        Log::console("poll".to_string());

        let download_fav = config.download_fav;
        let download_dir = config.download_dir.clone();

        match tokio::task::spawn_blocking(move || {
            run_poll_fav_cycle(accounts, names, download_fav, download_dir)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                Log::error(format!("poll/fav cycle failed: {}", e));
                Log::console("poll fail".to_string());
            }
            Err(e) => Log::error(format!("poll/fav task failed: {}", e)),
        }

        Log::dev_timing("poll_cycle", cycle_start);
        timeout(60, LogLevel::Console).await;
    }
}

#[tokio::main]
async fn main() {
    let mode = parse_args();
    Log::console(format!("Tiktok-Archiver 1.1.0 | Run Mode:{:?}", mode));
    if matches!(mode, RunMode::Dev) {
        env::set_var("TTA_SHOW_BROWSER", "1");
        env::set_var(crate::db::logger::DEV_MODE_ENV, "1");
        Log::dev("dev mode enabled (console trace only, not written to log.json)".to_string());
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
