use std::{io::{self, IsTerminal, Write}, time::{Duration, Instant}};
use tokio::time::sleep;
use crate::{browser::{is_headless}, db::{config::{load_config, Config}, logger::{Log, LogLevel}, video::load_all},  download::download_pending};
use anyhow::Result;
use crate::browser::{launch_browser_with_cookies};
use crate::db::config::load_tracked_accounts;
use crate::db::logger::LogLevel::Console;
use crate::discover::{fetch_new_fav, fetch_new_videos};

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
    Log::console(format!(
        "[main_loop] start: {} account(s), download_fav={}",
        user_count, config.download_fav
    ));

    let t0 = Instant::now();
    Log::console("[main_loop] loading video database".to_string());
    let mut seen = load_all()?;
    let tracked: usize = seen.values().map(|v| v.len()).sum();
    Log::console(format!(
        "[main_loop] database loaded: {} users, {} videos ({}ms)",
        seen.len(),
        tracked,
        t0.elapsed().as_millis()
    ));

    let t0 = Instant::now();
    Log::console(format!(
        "[main_loop] launching browser (headless={})",
        is_headless()
    ));
    let session = launch_browser_with_cookies("https://www.tiktok.com", is_headless())?;
    Log::console(format!(
        "[main_loop] browser ready ({}ms)",
        t0.elapsed().as_millis()
    ));

    for (index, username) in usernames.iter().enumerate() {
        let user_start = Instant::now();
        Log::console(format!(
            "[main_loop] user {}/{}: @{} — fetch starting",
            index + 1,
            user_count,
            username
        ));
        match fetch_new_videos(username, &session, &mut seen).await {
            Ok(()) => Log::console(format!(
                "[main_loop] user {}/{}: @{} — fetch ok ({}ms)",
                index + 1,
                user_count,
                username,
                user_start.elapsed().as_millis()
            )),
            Err(e) => {
                Log::error(format!("User Error Error: {}", e));
                Log::console(format!(
                    "[main_loop] user {}/{}: @{} — fetch failed ({}ms): {}",
                    index + 1,
                    user_count,
                    username,
                    user_start.elapsed().as_millis(),
                    e
                ));
            }
        }
        Log::console(format!(
            "[main_loop] user {}/{}: @{} — waiting 20s before next",
            index + 1,
            user_count,
            username
        ));
        timeout(20, Console).await;
    }

    if config.download_fav {
        let t0 = Instant::now();
        Log::console("[main_loop] favorites fetch starting".to_string());
        match fetch_new_fav(&session, &mut seen) {
            Ok(()) => Log::console(format!(
                "[main_loop] favorites fetch ok ({}ms)",
                t0.elapsed().as_millis()
            )),
            Err(e) => {
                Log::error(format!("Fav Error: {}", e));
                Log::console(format!(
                    "[main_loop] favorites fetch failed ({}ms): {}",
                    t0.elapsed().as_millis(),
                    e
                ));
            }
        }
    } else {
        Log::console("[main_loop] favorites fetch skipped (download_fav=false)".to_string());
    }

    let t0 = Instant::now();
    Log::console("[main_loop] download pending videos starting".to_string());
    download_pending(&mut seen)?;
    Log::console(format!(
        "[main_loop] download pending finished ({}ms)",
        t0.elapsed().as_millis()
    ));
    Log::console(format!(
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
        Log::dev("poll cycle start".to_string());
        let accounts = match load_tracked_accounts() {
            Ok(accounts) => accounts,
            Err(e) => {
                Log::error(format!("Failed to load accounts: {}", e));
                timeout(5, LogLevel::Error).await;
                continue;
            }
        };
        let config = match load_config() {
            Ok(c) => c,
            Err(e) => {
                Log::error(format!("Config Failed to load: {}", e));
                timeout(5u16, LogLevel::Error).await;
                continue;
            }
        };
        if let Err(e) = main_loop(accounts, config).await {
            Log::error(format!("main loop failed: {}", e));
        }
        Log::dev_timing("poll_cycle", cycle_start);
        timeout(5*60, LogLevel::Console).await;
    }
}