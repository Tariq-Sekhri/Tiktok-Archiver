use crate::api::{get_new_count,  videos_from_anchor_links};
use anyhow::{Context, Result};
use std::io;
use std::time::Duration;
use tokio::time::sleep;
use crate::db::browser::{launch_browser, load_cookie_params, save_cookies, cookie_to_param, CookiesMode, scroll_to_bottom};
use crate::db::account::Account;
use crate::db::logger::{log, Event, LogLevel};
use crate::db::seen_video::SeenVideo;

const WAIT_AFTER_LOAD_S: u64 = 2;

pub async fn first_discovery(username:String) -> Result<(Account, Vec<SeenVideo>)> {
    let session = launch_browser(&format!("https://www.tiktok.com/@{}", &username), CookiesMode::Persistent, true)?;
    scroll_to_bottom(&session)?;
    let html = session.tab.get_content().context("get_content")?;
    let new_vids = videos_from_anchor_links(&html, &username)?;

    if new_vids.is_empty() {
        return Err(anyhow::anyhow!("No new video"));
    }

    let count: i64;
    loop {
        match get_new_count(&username).await {
            Ok(n) => {
                count = n;
                break;
            }
            Err(e) => {
                log(Event::new(
                    format!("get_new_count failed for {}: {}", username, e),
                    LogLevel::Error,
                ));
            }
        }
        sleep(Duration::from_secs(2)).await;
    }

    let acc = Account::new(
        username.to_string(),
        count,
        count - new_vids.len() as i64
    );


    Ok((acc, new_vids))
}


pub async fn login() -> Result<()> {
    let cookies = load_cookie_params()?;
    if !cookies.is_empty() {
        println!("We Already have Cookies");
        println!("continuing will wipe current cookies");
        println!("Press Enter To Continue:");
        let mut asd = String::new();
        io::stdin().read_line(&mut asd)?;
    }

    let session = launch_browser("https://www.tiktok.com/login/qrcode", CookiesMode::None, false)?;
    println!("Once you are logged in, press Enter here to save your cookies.");
    let mut asd = String::new();
    io::stdin().read_line(&mut asd)?;
    let cookies = cookie_to_param(session.tab.get_cookies().context("get_cookies")?);
    save_cookies(&cookies)?;
    println!("Saved TikTok cookies to state/saved_cookies.json");
    println!("You can now run `cargo run` to start the default watcher.");
    Ok(())
}

pub async fn fetch_newest_videos(account: &Account) -> Result<Vec<SeenVideo>> {
    let url = format!("https://www.tiktok.com/@{}", account.name);
    let session = launch_browser(&url, CookiesMode::Persistent, true)?;
    sleep(Duration::from_secs(WAIT_AFTER_LOAD_S)).await;
    videos_from_anchor_links(&session.tab.get_content()?, &account.name)
}















