
mod db;
mod api;
mod discover;
mod download;

use crate::db::logger::{log, Event, LogLevel};
use tokio::time::{sleep, Duration};
use std::{env, io, io::Write, process};
use api::get_new_count;
use discover::login;
use crate::db::check_state;
use crate::db::account::{load_tracked_accounts, update_account_state, Account, CountEvent};
use crate::db::seen_video::{append_seen_videos, load_all_seen_videos,  total_seen_videos, SeenVideo};
use crate::discover::fetch_newest_videos;
use crate::download::download_pending;
use std::io::IsTerminal;

#[derive(Debug)]
pub enum RunMode {
    Login,
    Default,
}


fn print_usage_and_exit() -> ! {
    eprintln!("Usage: cargo run [run mode]");
    eprintln!("  no args = default mode (auto login on first run if needed)");
    eprintln!("  login   = explicitly run login flow (for switching accounts or refreshing cookies)");
    process::exit(1);
}

fn parse_args() -> RunMode {
    let args: Vec<String> = env::args().collect();
    if let Some(arg) = args.get(1) {
        match arg.as_str() {
            "login" => {
                RunMode::Login
            },
            _ => print_usage_and_exit(),
        }
    } else {
        RunMode::Default
    }
}

fn print_how_to_use_and_exit(reason: &str) -> ! {
    log(Event::new(reason.to_string(), LogLevel::CriticalFail));
    eprintln!("\n[State Check] {}\n", reason);
    eprintln!("How to use, in order:");
    eprintln!("  1) cargo run");
    eprintln!("     - On first run, this will prompt you to log in and save cookies into `state/saved_cookies.json` if none are present.");
    eprintln!("  2) update config.yaml");
    eprintln!("     - Choose which accounts you want to track and optionally change download_dir.");
    eprintln!("  3) cargo run");
    eprintln!("     - Default mode: poll for new videos + download pending using your saved login.");
    eprintln!("  4) cargo run login");
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
        let Ok(accounts) = load_tracked_accounts() else {
            log(Event::new("Failed to load accounts".to_string(), LogLevel::CriticalFail));
            continue;
        };

        for account in accounts {
            let new_count = match get_new_count(&account.name).await {
                Ok(n) => n,
                Err(_) => {
                    continue;
                }
            };

            let seen_map = match load_all_seen_videos() {
                Ok(m) => m,
                Err(e) => {
                    let msg = format!("{}: load_all_seen_videos failed: {}", account.name, e);
                    log(Event::new(msg, LogLevel::CriticalFail));
                    unreachable!()
                }
            };

            let existing_videos: Vec<SeenVideo> = match seen_map.get(&account.name) {
                Some(v) => v.clone(),
                None => {
                    log(Event::new(format!("{}: no entry in seen_videos, using empty list", account.name), LogLevel::Error));
                    Vec::new()
                }
            };

            let existing_ids: std::collections::HashSet<i64> =
                existing_videos.iter().map(|v| v.video_id).collect();

            let (unavailable, new_videos): (i64, Vec<SeenVideo>) = match CountEvent::observe(account.count, new_count) {
                CountEvent::Same => {
                    println!("{}: Same", account.name);
                    (account.unavailable, Vec::new())
                },
                CountEvent::Increased => {
                    let fetched_videos = match fetch_newest_videos(&account).await {
                        Ok(v) => v,
                        Err(e) => {
                            log(Event::new(
                                format!("{}: fetch_newest_videos failed: {}", account.name, e),
                                LogLevel::Error,
                            ));
                            continue;
                        }
                    };
                    let new_v: Vec<SeenVideo> = fetched_videos
                        .into_iter()
                        .filter(|v| !existing_ids.contains(&v.video_id))
                        .collect();
                    (account.unavailable, new_v)
                },
                CountEvent::Decreased => {
                    let unavailable = account.unavailable + (account.count - new_count);
                    println!(
                        "[main] {}: count decreased, unavailable incremented by {} -> {}",
                        account.name,
                        account.count - new_count,
                        unavailable
                    );
                    (unavailable, Vec::new())
                },
            };

            if !new_videos.is_empty() {
                if let Err(e) = append_seen_videos(&account.name, &new_videos) {
                    let msg = format!("{}: append_seen_videos failed: {}", account.name, e);
                    log(Event::new(msg, LogLevel::CriticalFail));
                    continue;
                }
            }


            reconcile_account_state(&account, new_count, unavailable);

            if let Err(e) = download_pending() {
                let msg = format!("Error downloading for {}: {}", account.name, e);
                log(Event::new(msg, LogLevel::Error));
            }
            sleep(Duration::from_secs(1)).await;

        }

        timeout(60u8).await;
    }
}

fn reconcile_account_state(account: &Account, new_count: i64, unavailable: i64) {
    let totals = match total_seen_videos() {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("{}: total_seen_videos failed: {}", account.name, e);
            log(Event::new(msg, LogLevel::CriticalFail));
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
        log(Event::new(msg, LogLevel::CriticalFail));
    }

    let invariant_lhs = new_count + unavailable - diff;

    if invariant_lhs != total_seen_videos_count {
        let msg = format!(
            "{}: invariant violated (lhs={}, rhs={})",
            account.name, invariant_lhs, total_seen_videos_count
        );
        log(Event::new(msg, LogLevel::Error));
    }

    if let Err(e) = update_account_state(account, new_count, diff, unavailable) {
        let msg = format!("Error updating state for @{}: {}", account.name, e);
        log(Event::new(msg, LogLevel::CriticalFail));
    }
}

#[tokio::main]
async fn main() {
    let mode = parse_args();
    println!("Run Mode:{:?}", mode);
    check_state(&mode).await;
    match mode {
        RunMode::Login => login().await.unwrap_or_else(|e| {
            let msg = format!("Error logging in: {}", e);
            log(Event::new(msg.clone(), LogLevel::CriticalFail));
        }),
        RunMode::Default => default_loop().await,
    }
}

