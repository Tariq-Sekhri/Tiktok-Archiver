use crate::api::{video_count_from_html, videos_from_anchor_links};
use anyhow::{Context, Result};
use std::io;
use std::time::Duration;
use crate::db::browser::{
    clear_tiktok_profile, cookie_params_have_session, cookie_to_param, discovery_headless,
    launch_browser, load_cookie_params, save_cookies, scroll_to_bottom, TIKTOK_ORIGIN,
};
use crate::db::account::Account;
use crate::db::logger::Log;
use crate::db::video::Video;

const WAIT_AFTER_LOAD_S: u64 = 2;

pub async fn first_discovery(username:String) -> Result<(Account, Vec<Video>)> {
    Log::dev(format!(
        "[discover] first_discovery @{} headless={}",
        username,
        discovery_headless()
    ));
    let session = launch_browser(&format!("https://www.tiktok.com/@{}", &username), discovery_headless())?;
    let result = (|| {
        scroll_to_bottom(&session)?;
        let html = session.tab().get_content().context("get_content")?;
        let new_vids = videos_from_anchor_links(&html)?;
        Log::dev(format!(
            "[discover] first_discovery @{} anchor links={}",
            username,
            new_vids.len()
        ));

        if new_vids.is_empty() {
            return Err(anyhow::anyhow!("No new video"));
        }

        let count = video_count_from_html(&html)?;
        let diff = count - new_vids.len() as i64;
        Log::dev(format!(
            "[discover] first_discovery @{} videoCount={} diff={}",
            username,
            count,
            diff
        ));

        Ok((Account::new(username.to_string(), count, diff), new_vids))
    })();
    session.finish();
    result
}


pub async fn login() -> Result<()> {
    let cookies = load_cookie_params()?;
    if !cookies.is_empty() {
        println!("We Already have Cookies");
        println!("continuing will wipe current cookies");
        println!("Press Enter To Continue:");
        let mut asd = String::new();
        io::stdin().read_line(&mut asd)?;
        clear_tiktok_profile()?;
    }

    let session = launch_browser(
        "https://www.tiktok.com/login/qrcode",
        false,
    )?;
    println!("Log in in the browser window, then wait until you see your feed.");
    println!("Press Enter here to save your cookies.");
    let mut asd = String::new();
    io::stdin().read_line(&mut asd)?;

    session
        .tab()
        .navigate_to(TIKTOK_ORIGIN)
        .context("navigate to tiktok.com before saving cookies")?;
    session
        .tab()
        .wait_until_navigated()
        .context("timed out waiting for tiktok.com after login")?;
    std::thread::sleep(Duration::from_secs(2));

    let cookies = cookie_to_param(session.tab().get_cookies().context("get_cookies")?);
    if cookies.is_empty() {
        return Err(anyhow::anyhow!(
            "no tiktok cookies found in browser — finish logging in and try again"
        ));
    }
    if !cookie_params_have_session(&cookies) {
        return Err(anyhow::anyhow!(
            "session cookies missing — make sure you are fully logged in before pressing Enter"
        ));
    }
    save_cookies(&cookies)?;
    let path = crate::db::browser::cookies_path()?;
    println!("Saved {} TikTok cookies to {}", cookies.len(), path);
    println!("You can now run `cargo run` to start the default watcher.");
    session.finish();
    Ok(())
}

pub fn fetch_newest_videos_sync(account: &Account) -> Result<Vec<Video>> {
    let url = format!("https://www.tiktok.com/@{}", account.name);
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} url={} wait_after_load_s={}",
        account.name,
        url,
        WAIT_AFTER_LOAD_S
    ));
    let session = launch_browser(&url, discovery_headless())?;
    std::thread::sleep(Duration::from_secs(WAIT_AFTER_LOAD_S));
    let vids = videos_from_anchor_links(&session.tab().get_content().context("get_content")?)?;
    Log::dev(format!(
        "[discover] fetch_newest_videos @{} parsed {} anchor links",
        account.name,
        vids.len()
    ));
    session.finish();
    Ok(vids)
}

pub async fn fetch_newest_videos(account: &Account) -> Result<Vec<Video>> {
    let account = account.clone();
    match tokio::task::spawn_blocking(move || fetch_newest_videos_sync(&account)).await {
        Ok(r) => r,
        Err(e) => Err(anyhow::anyhow!("fetch_newest_videos task failed: {}", e)),
    }
}
















