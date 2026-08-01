use crate::browser::launch_browser_with_cookies;
use crate::db::config::load_tracked_accounts;
use crate::db::logger::LogLevel::Console;
use crate::discover::{fetch_new_fav, fetch_new_videos};
use crate::{
    browser::is_headless,
    db::{
        config::{load_config, Config},
        logger::{Log, LogLevel},
        poll_health::{maybe_critical_fail_on_poll_error, record_poll_success},
        video::load_all,
    },
    download::download_pending,
};
use anyhow::Result;
use rand::seq::SliceRandom;
use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};
use tokio::time::sleep;

fn is_tiktok_access_denied(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("err_http_response_code_failure")
        || detail.contains("tiktok access denied")
        || detail.contains("verification required")
}

pub async fn timeout(wait_secs: u16, level: LogLevel) {
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

//v1
async fn main_loop(usernames: Vec<String>, config: Config) -> Result<()> {
    let loop_start = Instant::now();
    let user_count = usernames.len();
    Log::console(format!("Checking {} account(s)", user_count));
    Log::dev(format!(
        "[main_loop] start: {} account(s), download_fav={}",
        user_count, config.download_fav
    ));

    let t0 = Instant::now();
    Log::dev("[main_loop] loading video database".to_string());
    let mut seen = load_all()?;
    let tracked: usize = seen.values().map(|v| v.len()).sum();
    Log::dev(format!(
        "[main_loop] database loaded: {} users, {} videos ({}ms)",
        seen.len(),
        tracked,
        t0.elapsed().as_millis()
    ));

    let t0 = Instant::now();
    let headless = is_headless();
    Log::info(format!(
        "poll work starting: accounts={user_count} download_fav={} headless={headless}",
        config.download_fav
    ));
    Log::dev(format!(
        "[main_loop] launching browser (headless={headless})"
    ));
    let session = launch_browser_with_cookies("https://www.tiktok.com", headless)?;
    Log::dev(format!(
        "[main_loop] browser ready ({}ms)",
        t0.elapsed().as_millis()
    ));

    if config.download_fav {
        let t0 = Instant::now();
        Log::console("Favorites".to_string());
        Log::dev("[main_loop] favorites fetch starting (first work item)".to_string());
        if let Err(e) = fetch_new_fav(&session, &mut seen) {
            Log::error(format!("favorites fetch failed: {:#}", e));
            Log::console(format!("Favorites — failed: {e:#}"));
        }
        Log::dev(format!(
            "[main_loop] favorites fetch finished ({}ms)",
            t0.elapsed().as_millis()
        ));
    } else {
        Log::dev("[main_loop] favorites fetch skipped (download_fav=false)".to_string());
    }

    for (index, username) in usernames.iter().enumerate() {
        let user_start = Instant::now();
        Log::console(format!("@{username}"));
        Log::dev(format!(
            "[main_loop] user {}/{}: @{} — fetch starting",
            index + 1,
            user_count,
            username
        ));
        if let Err(e) = fetch_new_videos(username, &session, &mut seen).await {
            let detail = format!("@{username} fetch failed: {:#}", e);
            Log::error(detail.clone());
            Log::console(format!("@{username} — failed: {e:#}"));
            if is_tiktok_access_denied(&detail) {
                return Err(anyhow::anyhow!(
                    "TikTok denied the shared browser session while fetching @{username}: {e:#}"
                ));
            }
        }
        Log::dev(format!(
            "[main_loop] user {}/{}: @{} — fetch finished ({}ms)",
            index + 1,
            user_count,
            username,
            user_start.elapsed().as_millis()
        ));
        Log::dev(format!(
            "[main_loop] user {}/{}: @{} — waiting 20s before next",
            index + 1,
            user_count,
            username
        ));
        timeout(20, Console).await;
    }

    let t0 = Instant::now();
    Log::console("Downloading".to_string());
    Log::dev("[main_loop] download pending videos starting".to_string());
    download_pending(&mut seen)?;
    Log::dev(format!(
        "[main_loop] download pending finished ({}ms)",
        t0.elapsed().as_millis()
    ));
    Log::console("Done".to_string());
    Log::info(format!(
        "poll work finished: accounts={user_count} elapsed_ms={}",
        loop_start.elapsed().as_millis()
    ));
    Log::dev(format!(
        "[main_loop] complete ({}ms total)",
        loop_start.elapsed().as_millis()
    ));
    Log::dev_timing("main_loop", loop_start);
    Ok(())
}

//v1
pub async fn default_loop() {
    loop {
        let cycle_start = Instant::now();
        Log::console("Poll cycle".to_string());
        Log::info("poll cycle starting".to_string());
        Log::dev("poll cycle start".to_string());
        let accounts = match load_tracked_accounts() {
            Ok(accounts) => accounts,
            Err(e) => {
                let detail = format!("failed to load accounts: {:#}", e);
                Log::error(detail.clone());
                maybe_critical_fail_on_poll_error(&detail);
                timeout(5, LogLevel::Error).await;
                continue;
            }
        };
        let mut accounts = accounts;
        accounts.shuffle(&mut rand::rng());
        Log::dev(format!(
            "poll account order: {}",
            accounts
                .iter()
                .map(|username| format!("@{username}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let config = match load_config() {
            Ok(c) => c,
            Err(e) => {
                let detail = format!("config failed to load: {:#}", e);
                Log::error(detail.clone());
                maybe_critical_fail_on_poll_error(&detail);
                timeout(5u16, LogLevel::Error).await;
                continue;
            }
        };
        if let Err(e) = main_loop(accounts, config).await {
            let detail = format!("main loop failed: {:#}", e);
            Log::error(detail.clone());
            Log::info(format!(
                "poll cycle failed: elapsed_ms={} error={detail}",
                cycle_start.elapsed().as_millis()
            ));
            Log::console(format!("Poll cycle failed: {e:#}"));
            if is_tiktok_access_denied(&detail) {
                Log::critical_tiktok_access_denied(format!(
                    "TikTok denied this browser session (HTTP 403 / response-code failure). Polling stopped after the first rejected request. Complete TikTok verification or refresh the signed-in session before restarting.\n\nLast error:\n{detail}"
                ));
            } else {
                maybe_critical_fail_on_poll_error(&detail);
            }
        } else {
            record_poll_success();
            Log::info(format!(
                "poll cycle succeeded: elapsed_ms={}",
                cycle_start.elapsed().as_millis()
            ));
        }
        Log::dev_timing("poll_cycle", cycle_start);
        timeout(5 * 60, LogLevel::Console).await;
    }
}
