use crate::browser::launch_browser_with_cookies;
use crate::db::config::load_tracked_accounts;
use crate::db::logger::LogLevel::Console;
use crate::discover::{fetch_new_fav, fetch_new_videos};
use crate::{
    browser::is_headless,
    db::{
        config::{load_config, Config},
        logger::{Log, LogLevel},
        poll_health::{
            handle_poll_cycle_errors, is_tiktok_slow_down_error,
            maybe_critical_fail_on_poll_error,
        },
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

async fn log_exit_country() {
    match reqwest::get("https://ipapi.co/json/").await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let ip = body.get("ip").and_then(|v| v.as_str()).unwrap_or("?");
                let country = body
                    .get("country_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let city = body.get("city").and_then(|v| v.as_str()).unwrap_or("?");
                Log::info(format!("exit IP: {ip} ({country}, {city})"));
            }
        }
        Ok(resp) => Log::dev(format!("exit IP check failed: http {}", resp.status())),
        Err(e) => Log::dev(format!("exit IP check failed: {e:#}")),
    }
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

async fn main_loop(usernames: Vec<String>, config: Config) -> Result<(), Vec<String>> {
    let loop_start = Instant::now();
    let user_count = usernames.len();
    let mut errors = Vec::new();
    if config.poll_accounts {
        Log::console(format!("Checking {} account(s)", user_count));
    }
    Log::dev(format!(
        "[main_loop] start: {} account(s), download_fav={}, poll_accounts={}",
        user_count, config.download_fav, config.poll_accounts
    ));

    let t0 = Instant::now();
    Log::dev("[main_loop] loading video database".to_string());
    let mut seen = match load_all() {
        Ok(seen) => seen,
        Err(e) => return Err(vec![format!("failed to load video database: {:#}", e)]),
    };
    let tracked: usize = seen.values().map(|v| v.len()).sum();
    Log::dev(format!(
        "[main_loop] database loaded: {} users, {} videos ({}ms)",
        seen.len(),
        tracked,
        t0.elapsed().as_millis()
    ));

    let needs_browser = config.download_fav || config.poll_accounts;
    let headless = is_headless();
    Log::info(format!(
        "poll work starting: accounts={user_count} download_fav={} poll_accounts={} headless={headless}",
        config.download_fav, config.poll_accounts
    ));

    if needs_browser {
        let t0 = Instant::now();
        Log::dev(format!(
            "[main_loop] launching browser (headless={headless})"
        ));
        let session = match launch_browser_with_cookies("https://www.tiktok.com", headless) {
            Ok(session) => session,
            Err(e) => return Err(vec![format!("browser launch failed: {:#}", e)]),
        };
        Log::dev(format!(
            "[main_loop] browser ready ({}ms)",
            t0.elapsed().as_millis()
        ));

        if config.download_fav {
            let t0 = Instant::now();
            Log::console("Favorites".to_string());
            Log::dev("[main_loop] favorites fetch starting (first work item)".to_string());
            if let Err(e) = fetch_new_fav(&session, &mut seen) {
                let detail = format!("favorites fetch failed: {:#}", e);
                Log::error(detail.clone());
                Log::console(format!("Favorites — failed: {e:#}"));
                errors.push(detail.clone());
                if is_tiktok_slow_down_error(&detail) {
                    Log::info(
                        "TikTok slow-down (HTTP response failure): ending poll immediately"
                            .to_string(),
                    );
                    Log::console("Poll stopped — TikTok rate limit signal".to_string());
                    return Err(errors);
                }
            }
            Log::dev(format!(
                "[main_loop] favorites fetch finished ({}ms)",
                t0.elapsed().as_millis()
            ));
        } else {
            Log::dev("[main_loop] favorites fetch skipped (download_fav=false)".to_string());
        }

        if config.poll_accounts {
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
                    errors.push(detail.clone());
                    if is_tiktok_slow_down_error(&detail) {
                        Log::info(format!(
                            "TikTok slow-down (HTTP response failure) at @{username}: ending poll immediately"
                        ));
                        Log::console(format!(
                            "@{username} — TikTok rate limit signal, stopping poll"
                        ));
                        return Err(errors);
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
        } else {
            Log::dev("[main_loop] account fetch skipped (poll_accounts=false)".to_string());
        }
    } else {
        Log::dev(
            "[main_loop] browser skipped (poll_accounts=false, download_fav=false)".to_string(),
        );
    }

    let t0 = Instant::now();
    Log::console("Downloading".to_string());
    Log::dev("[main_loop] download pending videos starting".to_string());
    if let Err(e) = download_pending(&mut seen) {
        let detail = format!("download pending failed: {:#}", e);
        Log::error(detail.clone());
        errors.push(detail);
    }
    Log::dev(format!(
        "[main_loop] download pending finished ({}ms)",
        t0.elapsed().as_millis()
    ));
    Log::console("Done".to_string());
    Log::info(format!(
        "poll work finished: accounts={user_count} elapsed_ms={} errors={}",
        loop_start.elapsed().as_millis(),
        errors.len()
    ));
    Log::dev(format!(
        "[main_loop] complete ({}ms total)",
        loop_start.elapsed().as_millis()
    ));
    Log::dev_timing("main_loop", loop_start);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub async fn default_loop() {
    loop {
        let cycle_start = Instant::now();
        Log::console("Poll cycle".to_string());
        Log::info("poll cycle starting".to_string());
        log_exit_country().await;
        Log::dev("poll cycle start".to_string());
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
        let accounts = if config.poll_accounts {
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
            accounts
        } else {
            Log::dev("account polling disabled (poll_accounts=false)".to_string());
            Vec::new()
        };
        match main_loop(accounts, config).await {
            Ok(()) => {
                handle_poll_cycle_errors(&[]);
                Log::info(format!(
                    "poll cycle succeeded: elapsed_ms={}",
                    cycle_start.elapsed().as_millis()
                ));
            }
            Err(errors) => {
                Log::info(format!(
                    "poll cycle finished with {} error(s): elapsed_ms={}",
                    errors.len(),
                    cycle_start.elapsed().as_millis()
                ));
                Log::console(format!(
                    "Poll cycle had {} error(s): {}",
                    errors.len(),
                    errors.first().map(String::as_str).unwrap_or("unknown")
                ));
                handle_poll_cycle_errors(&errors);
            }
        }
        Log::dev_timing("poll_cycle", cycle_start);
        timeout(5 * 60, LogLevel::Console).await;
    }
}
