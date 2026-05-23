//v1

use std::{collections::HashSet, io::{self, IsTerminal, Write}, time::{Duration, Instant}};
use tokio::time::sleep;
use crate::{browser::{is_headless, launch_browser}, db::{account::{load_tracked_accounts, update_account_state, Account, CountEvent}, config::{load_config, Config}, logger::{Log, LogLevel}, video::{append_videos, bucket_count, load_all, save_all, Video}}, discover::{fav, fetch_newest_videos}, download::download_pending};
use crate::discover::get_new_count;
use anyhow::Result;

//v1
pub async fn timeout(wait_secs: u8, level: LogLevel) {
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
async fn run_poll_fav_cycle(accounts: Vec<Account>,config:Config) -> Result<()> {
    let mut seen_vids = load_all()?;
    let headless= is_headless();
    Log::dev(format!("headless:{}", headless) );
    let session = launch_browser("https://www.tiktok.com", headless)?;
    let mut have_new_vids = false;

    let count_results:Vec<Result<i64>> = accounts.iter().map(|account| get_new_count(&session, &account.name)).collect();

    for (account, new_count) in accounts.into_iter().zip(count_results) {
        Log::dev(format!("@{} polling tiktok video count", account.name));
        let new_count = match new_count {
            Ok(n) => n,
            Err(e) => {
                Log::error(format!("{}: {}", account.name, e));
                continue;
            }
        };
        let existing_videos: Vec<Video> = match seen_vids.get(&account.name) {
            Some(v) => v.clone(),
            None => {
                Log::error(format!(
                    "{}: no entry in seen_videos, using empty list",
                    account.name
                ));
                Vec::new()
            }
        };

        let existing_ids: HashSet<i64> = existing_videos.iter().map(|v| v.id).collect();
        let (unavailable, new_videos): (i64, Vec<Video>) =
            match CountEvent::observe(account.count, new_count) {
                CountEvent::Same => {
                    Log::console(format!("{}: same", account.name));
                    (account.unavailable, Vec::new())
                }
                CountEvent::Increased => {
                    Log::console(format!(
                        "@{} count increased {} -> {}",
                        account.name, account.count, new_count
                    ));
                    let fetched_videos = match fetch_newest_videos(&account, &session) {
                        Ok(v) => v,
                        Err(e) => {
                            Log::error(format!(
                                "{}: fetch_newest_videos failed: {}",
                                account.name, e
                            ));
                            continue;
                        }
                    };
                    let new_v: Vec<Video> = fetched_videos.into_iter()
                        .filter(|v| !existing_ids.contains(&v.id)).collect();
                    (account.unavailable, new_v)
                }
                CountEvent::Decreased => {
                    let unavailable = account.unavailable + (account.count - new_count);
                    Log::console(format!(
                        "@{} count decreased {} -> {}, unavailable {} -> {} (delta={})",
                        account.name,
                        account.count,
                        new_count,
                        account.unavailable,
                        unavailable,
                        account.count - new_count
                    ));
                    (unavailable, Vec::new())
                }
            };

        if !new_videos.is_empty() {
            Log::dev(format!(
                "@{} appending {} videos to seen_videos",
                account.name,
                new_videos.len()
            ));
            append_videos(&mut seen_vids, &account.name, &new_videos);
            have_new_vids = true;
        }

        let total_seen = bucket_count(&seen_vids, &account.name);
        reconcile_account_state(&account, new_count, unavailable, total_seen);
    }
    if config.download_fav {
            match fav(&session, &mut seen_vids) {
            Ok(fav_dirty) => {
                if fav_dirty {
                    have_new_vids = true;
                }
            }
            Err(e) => Log::error(format!("Fav Error: {}", e)),
        }
    }

    if have_new_vids {
        download_pending(&mut seen_vids)?;
        save_all(&seen_vids)?;
    }
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
                timeout(5u8, LogLevel::Error).await;
                continue;
            }
        };
        let config = match load_config() {
            Ok(c) => c,
            Err(e) => {
                Log::error(format!("Config Failed to load: {}", e));
                timeout(5u8, LogLevel::Error).await;
                continue;
            }
        };
        if let Err(e) = run_poll_fav_cycle(accounts,  config).await {
            Log::error(format!("poll/fav cycle failed: {}", e));
        }
        Log::dev_timing("poll_cycle", cycle_start);
        timeout(60, LogLevel::Console).await;
    }
}
//v1
fn reconcile_account_state(account: &Account, new_count: i64, unavailable: i64, total_seen: usize) {
    let total_seen_videos_count = total_seen as i64;
    Log::dev(format!("@{} reconcile enter: new_count={} unavailable_in={} stored_count={} stored_diff={} stored_unavailable={}", account.name, new_count, unavailable,        account.count,        account.diff,        account.unavailable));
    let diff = new_count + unavailable - total_seen_videos_count;
    Log::dev(format!("@{} reconcile: total_seen={} diff={} (formula: {} + {} - {} = {})", account.name,        total_seen_videos_count,diff,new_count,        unavailable, total_seen_videos_count, diff));

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